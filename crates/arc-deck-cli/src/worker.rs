//! worker 主流程。
//!
//! 顺序固定为：校验（任务书 / 配置 / Agent / 授权）→ 拉起运行时 → 握手 →
//! 提交 → 消费事件直到 terminal → 收尾 → 出结论。
//!
//! 这里最重要的三个判断，都写在代码注释里：
//!
//! 1. **`session/prompt` 的回执只表示入队**，不代表模型回答完了。完成判定必须
//!    依赖「观察到 `turn/end`」加上「`session/wait` 确认空闲」两件事。
//! 2. **缺失 terminal 一律按 `unknown`**。进程提前退出、只空闲不出 terminal，
//!    都不能算成功 —— 那正是「看起来跑完了」最危险的一种。
//! 3. **验收证据取自工具执行记录**，不由 worker 自己再跑一遍命令。
//!    自己跑会绕过授权（验收身份不是提权理由），也拿不到模型实际的执行上下文。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_deck_core::agent::{AgentRegistry, SystemPrompt};
use arc_deck_core::authz::ExecutionPlan;
use arc_deck_core::config::{RuntimeSpec, WorkerConfig};
use arc_deck_core::error::{code, SpecError};
use arc_deck_core::event::WorkerState;
use arc_deck_core::mode::AgentId;
use arc_deck_core::result::{
    TaskResult, TaskStatus, VerificationKind, VerificationOutcome, VerificationRecord,
};
use arc_deck_core::task::{Acceptance, TaskSpec};
use arc_deck_core::EXIT_SPEC_ERROR;
use arc_deck_dsh::process::RuntimeProcess;
use arc_deck_dsh::projection::Projection;
use arc_deck_dsh::protocol::{self, method, InitializeParams, SessionEventParams, SessionPromptParams};
use arc_deck_dsh::transport::{self, DshError, Inbound, Transport};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStderr;
use tokio::sync::{mpsc, Mutex};

use crate::args::{TaskSource, WorkerArgs};
use crate::cancel::{CancelSignal, CancelWatcher};
use crate::report::{write_result_file, EventSink, WriteOutcome};
use crate::status::{self, StatusFile, StatusWriter};

/// 取消文件的轮询间隔。
const CANCEL_POLL: Duration = Duration::from_millis(100);
/// 等到空闲但还没见到 terminal 时，再给 terminal 留的宽限。
const TERMINAL_GRACE: Duration = Duration::from_secs(3);
/// 收尾时等待进程退出的上限。
const TERMINATE_GRACE: Duration = Duration::from_secs(10);

/// 通过校验、可以执行的任务。
struct Prepared {
    config: WorkerConfig,
    plan: ExecutionPlan,
    system_prompt: SystemPrompt,
    /// 已经解析好的运行时启动参数。
    ///
    /// 在 `prepare` 阶段就解析完，是为了让「运行时不可用」这类拒绝发生在
    /// **建立任何状态之前** —— 否则主控会先在 stdout 上看到半条事件流、
    /// 然后才收到退出码 64，那种顺序很难解释。
    command: RuntimeCommand,
    run_dir: PathBuf,
    cancel_file: Option<PathBuf>,
    result_path: Option<PathBuf>,
    task_label: String,
    /// 派工关系的边与节点标签：只在 `status.json` 里落一次，不改任何判定。
    assigned_by: Option<String>,
    title: Option<String>,
}

/// 运行时的启动参数。
struct RuntimeCommand {
    program: PathBuf,
    args: Vec<String>,
    /// 含密钥（真实 DSH）。只在内存里，从不落盘。
    env: BTreeMap<String, String>,
    inherit_env: bool,
    /// 状态目录建好之后才写的文件（DSH patch）。
    pending_files: Vec<(PathBuf, String)>,
}

/// 入口：返回进程退出码。
pub async fn run(args: WorkerArgs) -> i32 {
    let prepared = match prepare(&args) {
        Ok(prepared) => prepared,
        Err(error) => {
            emit_spec_error(&error);
            return EXIT_SPEC_ERROR;
        }
    };
    execute(prepared).await
}

fn prepare(args: &WorkerArgs) -> Result<Prepared, SpecError> {
    let config = WorkerConfig::load(&args.config)?;
    let (task, task_label) = load_task(&args.task)?;

    let registry = AgentRegistry::load(&config.agents.dir, &config.agents.prompts_dir)?;
    let agent = match &task.agent {
        Some(raw) => AgentId::parse(raw)?,
        None => AgentId::Echo,
    };
    // Agent 定义里的 permission_profile 是第三层上限：只读角色不会因为
    // 启动配置宽松就拿到写权限。
    let ceiling = registry.definition(agent)?.permission_profile;
    let plan = ExecutionPlan::resolve_with_ceiling(&config, &task, Some(ceiling))?;
    // 派工关系与标题在**任何状态建立之前**取好；它们不进授权、不进 prompt。
    let assigned_by = task.assigned_by.clone();
    let title = status::derive_title(&task.instruction);
    // 工作区指令由 DSH 的既有机制装载（PRODUCT_PLAN §5.3），M1 不重复注入。
    let system_prompt = registry.compose_system_prompt(plan.policy.agent, plan.policy.mode, None)?;

    let run_dir = config.state_root.join("runs").join(&plan.task_id);
    // 运行时能力在建立任何状态之前就查清。
    let command = runtime_command(&config, &plan, &system_prompt, &run_dir)?;

    let watcher = CancelWatcher::new(args.cancel_file.clone(), &plan.task_id);
    watcher.preflight()?;

    let settled = run_dir.join("result.json");
    if settled.is_file() {
        return Err(SpecError::new(
            code::TASK_ALREADY_SEEN,
            format!(
                "状态目录里已经有 task_id `{}` 的结论：{}。换一个 task_id，或先清掉该状态目录",
                plan.task_id,
                settled.display()
            ),
        ));
    }
    std::fs::create_dir_all(&run_dir).map_err(|e| {
        SpecError::new(code::CONFIG_INVALID, format!("状态目录不可写：{} —— {e}", run_dir.display()))
    })?;
    std::fs::write(run_dir.join("system-prompt.md"), system_prompt.text()).map_err(|e| {
        SpecError::new(code::CONFIG_INVALID, format!("写入 system-prompt.md 失败：{e}"))
    })?;
    for (path, text) in &command.pending_files {
        std::fs::write(path, text).map_err(|e| {
            SpecError::new(code::CONFIG_INVALID, format!("写入 {} 失败：{e}", path.display()))
        })?;
    }

    Ok(Prepared {
        config,
        plan,
        system_prompt,
        command,
        run_dir,
        cancel_file: args.cancel_file.clone(),
        result_path: args.result.clone(),
        task_label,
        assigned_by,
        title,
    })
}

fn load_task(source: &TaskSource) -> Result<(TaskSpec, String), SpecError> {
    match source {
        TaskSource::File(path) => Ok((TaskSpec::load(path)?, path.display().to_string())),
        TaskSource::Stdin => {
            let mut raw = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw)
                .map_err(|e| SpecError::task_unreadable("<stdin>", e))?;
            let cwd = std::env::current_dir().map_err(|e| {
                SpecError::new(code::TASK_INVALID, format!("取当前目录失败：{e}"))
            })?;
            Ok((TaskSpec::parse(&raw, &cwd)?, "<stdin>".to_string()))
        }
    }
}

fn runtime_command(
    config: &WorkerConfig,
    plan: &ExecutionPlan,
    system_prompt: &SystemPrompt,
    run_dir: &Path,
) -> Result<RuntimeCommand, SpecError> {
    match &config.runtime {
        RuntimeSpec::DshSdk { node, entry, profile, route, sandbox, session_log_upload, preamble, .. } => {
            dsh_runtime_command(DshCommandInput {
                config,
                node,
                entry,
                profile,
                route: route.as_ref(),
                sandbox,
                session_log_upload: *session_log_upload,
                preamble: preamble.as_deref(),
                system_prompt,
                run_dir,
            })
        }
        RuntimeSpec::FixtureSdk { entry, scenario } => {
            fixture_runtime_command(entry, scenario, &config.env, plan)
        }
    }
}

struct DshCommandInput<'a> {
    config: &'a WorkerConfig,
    node: &'a Path,
    entry: &'a Path,
    profile: &'a str,
    route: Option<&'a arc_deck_core::config::RouteSpec>,
    sandbox: &'a str,
    session_log_upload: bool,
    preamble: Option<&'a Path>,
    system_prompt: &'a SystemPrompt,
    run_dir: &'a Path,
}

/// 真实 DSH（PRODUCT_PLAN §9.2a）：`node <bin.js> --profile sdk --patch <run_dir>/dsh-patch.json`，
/// `DSH_HOME` 是本任务独占的 `<run_dir>/dsh-home` —— 不碰 Desktop 的真实 home（参 AGENTS.md P15）。
fn dsh_runtime_command(input: DshCommandInput<'_>) -> Result<RuntimeCommand, SpecError> {
    if !input.entry.is_file() {
        return Err(SpecError::new(
            code::RUNTIME_ENTRY_MISSING,
            format!(
                "DSH 入口不存在：{}。先运行 `arcdeck runtime install`",
                input.entry.display()
            ),
        ));
    }
    let route = input.route.ok_or_else(|| {
        SpecError::new(code::CONFIG_INVALID, "runtime.dsh-sdk 缺 route：不知道该往 patch 里写哪个适配器")
    })?;
    // 密钥在这里取，取不到就以 64 退出 —— 不要等到模型请求时才发现。
    let key = arc_deck_core::provider::read_key_from_spec(route, &arc_deck_core::userconfig::auth_path())?;

    let mut persona = input.system_prompt.text();
    if let Some(path) = input.preamble {
        let text = std::fs::read_to_string(path).map_err(|e| {
            SpecError::new(code::CONFIG_INVALID, format!("读工人守则 {} 失败：{e}", path.display()))
        })?;
        persona.push_str("

---

");
        persona.push_str(&text);
    }
    let patch_path = input.run_dir.join("dsh-patch.json");
    let patch = arc_deck_dsh::runtime::render_patch(&arc_deck_dsh::runtime::PatchInput {
        route,
        persona_suffix: &persona,
        session_log_upload: input.session_log_upload,
    });

    let mut env = input.config.env.clone();
    for (k, v) in arc_deck_dsh::runtime::runtime_env(&input.run_dir.join("dsh-home"), input.sandbox) {
        env.insert(k, v);
    }
    env.insert(route.key_env.clone(), key);

    Ok(RuntimeCommand {
        program: input.node.to_path_buf(),
        args: vec![
            input.entry.display().to_string(),
            "--profile".into(),
            input.profile.to_string(),
            "--patch".into(),
            patch_path.display().to_string(),
        ],
        env,
        inherit_env: true,
        pending_files: vec![(patch_path, patch)],
    })
}

/// 假 SDK 对端的启动参数。
///
/// 分成两个实现是刻意的：默认构建里必须**编译不出**夹具，而不是带着一段
/// 只在运行时报错的死代码 —— 发布包不该有能力指向一个假对端。
#[cfg(feature = "test-runtime")]
fn fixture_runtime_command(
    entry: &Option<PathBuf>,
    scenario: &Option<String>,
    config_env: &BTreeMap<String, String>,
    plan: &ExecutionPlan,
) -> Result<RuntimeCommand, SpecError> {
    let program = match entry {
        Some(path) => path.clone(),
        None => std::env::current_exe().map_err(|e| {
            SpecError::new(
                code::RUNTIME_ENTRY_MISSING,
                format!("取当前可执行文件失败，无法回落到「重新执行自身」：{e}"),
            )
        })?,
    };
    let scenario = scenario.clone().unwrap_or_else(|| "success".to_string());
    if arc_deck_dsh::fixture::Scenario::parse(&scenario).is_none() {
        return Err(SpecError::new(
            code::CONFIG_INVALID,
            format!(
                "未知夹具场景 `{scenario}`；可用：{}",
                arc_deck_dsh::fixture::Scenario::ALL.join(", ")
            ),
        ));
    }
    let mut env = config_env.clone();
    if let Some(command) = first_command(plan) {
        env.insert(
            arc_deck_dsh::fixture::FIXTURE_COMMAND_ENV.to_string(),
            command,
        );
    }
    Ok(RuntimeCommand {
        program,
        args: vec!["__fixture-sdk".to_string(), "--scenario".to_string(), scenario],
        env,
        inherit_env: false,
        pending_files: Vec::new(),
    })
}

#[cfg(not(feature = "test-runtime"))]
fn fixture_runtime_command(
    entry: &Option<PathBuf>,
    scenario: &Option<String>,
    _config_env: &BTreeMap<String, String>,
    _plan: &ExecutionPlan,
) -> Result<RuntimeCommand, SpecError> {
    let _ = (entry, scenario);
    Err(SpecError::new(
        code::RUNTIME_NOT_SUPPORTED,
        "本构建没有启用 test-runtime feature，假 SDK 对端不可用；\
         离线验收请用 `cargo run -p arc-deck-cli --features test-runtime`",
    ))
}

#[cfg(feature = "test-runtime")]
fn first_command(plan: &ExecutionPlan) -> Option<String> {
    plan.acceptance.iter().find_map(|a| match a {
        Acceptance::Command { command, .. } => Some(command.clone()),
        Acceptance::Manual { .. } => None,
    })
}

async fn execute(prepared: Prepared) -> i32 {
    let task_id = prepared.plan.task_id.clone();
    let run_dir = prepared.run_dir.clone();
    let mut sink = EventSink::new(&task_id, Some(run_dir.join("events.jsonl")));
    let mut projection = Projection::new();
    let mut watcher = CancelWatcher::new(prepared.cancel_file.clone(), &task_id);
    let liveness = prepared.config.liveness.clone();
    let started = status::now_ms();
    let mut status = StatusWriter::new(
        &run_dir,
        StatusFile {
            schema_version: arc_deck_core::SCHEMA_VERSION,
            task_id: task_id.clone(),
            state: "accepted".into(),
            terminal: None,
            host: status::host_name(),
            pid: std::process::id(),
            runtime_pid: None,
            workspace: prepared.plan.workspace.display().to_string(),
            model: prepared.config.model.as_ref().map(|m| format!("{}/{}", m.provider, m.model)),
            assigned_by: prepared.assigned_by.clone(),
            title: prepared.title.clone(),
            started_at: started,
            heartbeat_at: started,
            progress_at: started,
            activity: status::Activity { kind: "starting".into(), name: None, summary: None, since: started },
            counters: Default::default(),
            attempt: 1,
            max_attempts: liveness.max_attempts,
            last_error: None,
            usage: None,
            limits: status::Limits {
                heartbeat_s: liveness.heartbeat_seconds,
                model_idle_s: liveness.model_idle_seconds,
                tool_stall_s: liveness.tool_stall_seconds,
            },
        },
    );

    let _ = sink.state(WorkerState::Accepted, "任务书与启动配置已通过校验");
    let _ = sink.diagnostic(
        "info",
        None,
        &format!(
            "任务书 {}；生效授权 agent={} mode={} profile={} shell={} write=[{}]",
            prepared.task_label,
            prepared.plan.policy.agent,
            prepared.plan.policy.mode,
            prepared.plan.policy.profile,
            prepared.plan.policy.shell,
            describe_paths(&prepared.plan.policy.write_paths)
        ),
    );
    // 这一行是「Echo 真的进了 system prompt」的可核对证据：片段的来源路径写出来。
    for section in &prepared.system_prompt.sections {
        let _ = sink.diagnostic(
            "info",
            Some("PROMPT_SECTION"),
            &format!("system prompt 片段 `{}` ← {}", section.name, section.source),
        );
    }

    // 整个任务一个墙钟预算，续跑不重置它。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(prepared.plan.timeout_seconds);
    let mut attempt: u32 = 1;
    let mut prompt = prepared.plan.instruction.clone();
    loop {
        status.status.attempt = attempt;
        status.flush(true);
        let ended = run_attempt(
            &prepared,
            &mut sink,
            &mut projection,
            &mut watcher,
            &mut status,
            &prompt,
            attempt,
            deadline,
        )
        .await;
        let retry_reason = match &ended {
            AttemptEnd::Ran { outcome: RunOutcome::Stalled(reason), .. } => Some(("STALLED", reason.clone())),
            AttemptEnd::Ran { outcome: RunOutcome::TurnEnded, .. } if tool_call_as_text(&projection) => {
                status.status.counters.tool_call_as_text += 1;
                Some(("TOOL_CALL_AS_TEXT", "模型把工具调用当成正文吐出后收工".to_string()))
            }
            _ => None,
        };
        match retry_reason {
            Some((code, reason)) if attempt < liveness.max_attempts => {
                status.error(code, &reason);
                let _ = sink.diagnostic(
                    "warn",
                    Some(code),
                    &format!("第 {attempt} 次运行：{reason}；开第 {} 次（共 {} 次）", attempt + 1, liveness.max_attempts),
                );
                attempt += 1;
                prompt = format!(
                    "{}\n\n---\n\n# 续跑说明（第 {attempt} 次）\n\n上一次运行被终止：{reason}。\
                     工作区里可能已有上一次的成果：先 `git status` 与 `git log --oneline -10` 看已完成的部分，\
                     接着做剩下的，不要重做已提交的功能块。",
                    prepared.plan.instruction
                );
                continue;
            }
            Some((code, reason)) => {
                status.error(code, &reason);
                return conclude(&prepared, sink, projection, status, ended, Some(code));
            }
            None => return conclude(&prepared, sink, projection, status, ended, None),
        }
    }
}

/// 一次运行（attempt）的结局。
enum AttemptEnd {
    /// 还没进入主循环就失败了：结果已经写好。
    Early { result: TaskResult, termination: ProcessTermination },
    Ran {
        outcome: RunOutcome,
        termination: ProcessTermination,
        runtime_version: Option<String>,
        session_id: String,
        wait_error: Option<String>,
    },
}

/// 拉起运行时 → 握手 → 提交 → 消费到终态 → 收尾。卡死时返回 `Stalled`，由调用方决定是否续跑。
#[allow(clippy::too_many_arguments)]
async fn run_attempt(
    prepared: &Prepared,
    sink: &mut EventSink,
    projection: &mut Projection,
    watcher: &mut CancelWatcher,
    status: &mut StatusWriter,
    prompt: &str,
    attempt: u32,
    deadline: tokio::time::Instant,
) -> AttemptEnd {
    let task_id = prepared.plan.task_id.clone();
    let command = &prepared.command;

    let _ = sink.state(WorkerState::Starting, &format!("拉起运行时（第 {attempt} 次）"));
    status.set_state("starting");
    status.set_activity("starting", None, None);
    let mut process = match RuntimeProcess::spawn(
        &command.program,
        &command.args,
        &command.env,
        Some(&prepared.plan.workspace),
        command.inherit_env,
    ) {
        Ok(process) => process,
        Err(e) => {
            let mut result = TaskResult::new(&task_id, TaskStatus::Failed, format!("无法拉起运行时：{e}"));
            result.blocker(
                "RUNTIME_SPAWN_FAILED",
                format!("{} 启动失败：{e}", command.program.display()),
            );
            result.policy = Some(prepared.plan.policy.clone());
            return AttemptEnd::Early { result, termination: process_termination_note(false, "未启动") };
        }
    };
    let pid = process.pid();
    status.status.runtime_pid = pid;
    let _ = sink.diagnostic(
        "info",
        None,
        &format!("运行时已启动：pid={pid:?} program={}", command.program.display()),
    );

    let (stdout, stdin, stderr) = process.take_stdio();
    let (transport, mut inbound) = transport::spawn(stdout, stdin);
    let stderr_tail = spawn_stderr_drain(stderr, prepared.run_dir.join("runtime.log"));
    let mut announced = AnnounceState::default();
    if let Some(path) = watcher.path() {
        let _ = sink.diagnostic(
            "info",
            None,
            &format!("取消文件：{}（启动时不存在，主控创建它即请求取消）", path.display()),
        );
    }

    let _ = sink.state(WorkerState::Running, "握手");
    status.set_state("running");
    let session_id = format!("arc-deck-{task_id}-{}", now_nanos());
    let route = prepared.config.model.clone();
    let (provider, model, reasoning_effort) = match &route {
        Some(route) => (route.provider.clone(), route.model.clone(), route.reasoning_effort.clone()),
        // 夹具不会真的调用模型，所以这里给的是显式占位而不是「一个编造的默认路由」。
        None => ("fixture".to_string(), "fixture".to_string(), None),
    };
    let init_params = serde_json::to_value(InitializeParams {
        cwd: prepared.plan.workspace.display().to_string(),
        provider,
        model,
        reasoning_effort,
    })
    .unwrap_or(Value::Null);

    let mut runtime_version = None;
    match call_watched(
        &transport,
        &mut inbound,
        sink,
        projection,
        watcher,
        &mut announced,
        status,
        method::INITIALIZE,
        init_params,
        Duration::from_secs(prepared.config.timeouts.initialize_seconds),
    )
    .await
    {
        CallOutcome::Value(value) => {
            runtime_version = serde_json::from_value::<protocol::InitializeResult>(value)
                .ok()
                .map(|r| format!("{}/{}", r.server_info.name, r.server_info.version));
            if let RuntimeSpec::DshSdk { version, .. } = &prepared.config.runtime {
                // 握手里的版本恒为 0.0.1、不能当判据（参 AGENTS.md P2）；真正钉住的是安装版本。
                runtime_version = Some(format!("dsh {version} ({})", runtime_version.unwrap_or_default()));
            }
            let _ = sink.diagnostic(
                "info",
                None,
                &format!("握手完成，对侧自报 {}", runtime_version.clone().unwrap_or_default()),
            );
        }
        outcome => {
            let mut result = TaskResult::new(&task_id, TaskStatus::Unknown, String::new());
            result.session_id = Some(session_id.clone());
            result.runtime_version = runtime_version.clone();
            result.policy = Some(prepared.plan.policy.clone());
            apply_call_failure(&mut result, "initialize", outcome);
            let termination = shutdown_and_terminate(&transport, &mut process).await;
            let _ = append_stderr(&mut result, &stderr_tail, sink).await;
            return AttemptEnd::Early { result, termination };
        }
    }

    let _ = sink.state(WorkerState::Running, "提交任务");
    let prompt_params = serde_json::to_value(SessionPromptParams {
        session_id: session_id.clone(),
        content_blocks: protocol::text_content_block(prompt),
    })
    .unwrap_or(Value::Null);

    match call_watched(
        &transport,
        &mut inbound,
        sink,
        projection,
        watcher,
        &mut announced,
        status,
        method::SESSION_PROMPT,
        prompt_params,
        Duration::from_secs(prepared.config.timeouts.prompt_seconds),
    )
    .await
    {
        CallOutcome::Value(_receipt) => {
            // 回执只是「已入队」。这里刻意不把它记成任何形式的完成。
            let _ = sink.diagnostic("info", None, "prompt 已入队（回执不代表模型已回答）");
            status.set_activity("model", None, None);
        }
        outcome => {
            let mut result = TaskResult::new(&task_id, TaskStatus::Unknown, String::new());
            result.session_id = Some(session_id.clone());
            result.runtime_version = runtime_version.clone();
            result.policy = Some(prepared.plan.policy.clone());
            apply_call_failure(&mut result, "session/prompt", outcome);
            let termination = shutdown_and_terminate(&transport, &mut process).await;
            let _ = append_stderr(&mut result, &stderr_tail, sink).await;
            return AttemptEnd::Early { result, termination };
        }
    }

    // `session/wait` 自己的上限只是保险；真正的上限是任务墙钟预算与卡死判定。
    let wait_fut = transport.call(
        method::SESSION_WAIT,
        json!({ "sessionId": session_id }),
        Duration::from_secs(prepared.config.timeouts.wait_seconds.max(prepared.plan.timeout_seconds)),
    );
    tokio::pin!(wait_fut);
    let mut wait_done = false;
    let mut wait_error: Option<String> = None;
    let mut grace: Option<tokio::time::Instant> = None;
    // 本次运行开始前已有的结束原因：续跑时投影里还留着上一次的，判「这一次结束了没有」要排除它。
    let ends_before = count_ends(projection);
    // 轮询定时器必须**建在循环外面**：在 select 里 `sleep()` 每次迭代都会重建，
    // 而事件密集时每 25 ms 就来一条，那个 100 ms 的 sleep 永远走不完 ——
    // 取消文件于是永远读不到。踩过一次，症状是「取消请求发出去了，任务照跑」。
    let mut cancel_poll = tokio::time::interval(CANCEL_POLL);
    cancel_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(prepared.config.liveness.heartbeat_seconds));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let outcome = loop {
        let ended_now = count_ends(projection) > ends_before;
        if wait_done && ended_now {
            break RunOutcome::TurnEnded;
        }
        if let Some(limit) = grace {
            if tokio::time::Instant::now() >= limit && !ended_now {
                break RunOutcome::NoTerminal;
            }
        }

        tokio::select! {
            biased;

            result = &mut wait_fut, if !wait_done => {
                wait_done = true;
                if let Err(e) = result {
                    wait_error = Some(e.to_string());
                }
                if count_ends(projection) <= ends_before {
                    grace = Some(tokio::time::Instant::now() + TERMINAL_GRACE);
                }
            }
            item = inbound.recv() => {
                match absorb(item, sink, projection, &mut announced, status) {
                    Absorbed::Ok => {}
                    Absorbed::Closed => break RunOutcome::RuntimeExited(
                        "运行时输出已关闭，但没有收到 turn/end".to_string()
                    ),
                    Absorbed::Fatal(reason) => break RunOutcome::Violation(reason),
                }
                status.flush(false);
                if !sink.stdout_alive() {
                    break RunOutcome::Cancelled("stdout 消费端已断开".to_string());
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                break RunOutcome::TimedOut;
            }
            _ = heartbeat.tick() => {
                status.status.usage = projection.usage.clone();
                if let Some(base) = &prepared.config.git_base {
                    status.status.counters.commits = git_commit_count(&prepared.plan.workspace, base);
                }
                status.flush(true);
                if let Some(reason) = status::stall_reason(&status.status, status::now_ms()) {
                    break RunOutcome::Stalled(reason);
                }
            }
            _ = cancel_poll.tick() => {
                match watcher.poll() {
                    CancelSignal::Requested(reason) => break RunOutcome::Cancelled(reason),
                    CancelSignal::Invalid(detail) => {
                        let _ = sink.diagnostic("warn", Some("CANCEL_FILE_INVALID"), &detail);
                    }
                    CancelSignal::None => {}
                }
            }
        }
    };

    let termination = shutdown_and_terminate(&transport, &mut process).await;
    let stderr_text = stderr_tail.lock().await.clone();
    if !stderr_text.trim().is_empty() {
        let _ = sink.diagnostic(
            "warn",
            Some("RUNTIME_STDERR"),
            &format!("运行时 stderr（截断到 32 KiB，全文见 runtime.log）：{}", stderr_text.trim()),
        );
    }
    AttemptEnd::Ran { outcome, termination, runtime_version, session_id, wait_error }
}

/// 已结束的轮次数。续跑的新会话同样从第 1 轮开始，投影会把它并进同一个轮次视图
/// 并覆盖上一次的结束原因 —— 所以这里另外加上被覆盖的次数，保证计数只增不减。
fn count_ends(projection: &Projection) -> usize {
    projection.turns().iter().filter(|t| t.end.is_some()).count() + projection.ends_overwritten
}

/// 把最后一次运行的结局折成结果。
fn conclude(
    prepared: &Prepared,
    mut sink: EventSink,
    projection: Projection,
    mut status: StatusWriter,
    ended: AttemptEnd,
    exhausted: Option<&str>,
) -> i32 {
    let task_id = prepared.plan.task_id.clone();
    let run_dir = prepared.run_dir.clone();
    let (outcome, termination, runtime_version, session_id, wait_error) = match ended {
        AttemptEnd::Early { mut result, termination } => {
            collect_git_facts(prepared, &mut result);
            return finish(&mut sink, prepared, result, termination, &mut status);
        }
        AttemptEnd::Ran { outcome, termination, runtime_version, session_id, wait_error } => {
            (outcome, termination, runtime_version, session_id, wait_error)
        }
    };

    if let Some(error) = &wait_error {
        let _ = sink.diagnostic("warn", Some("RUNTIME_WAIT_ERROR"), error);
    }
    let _ = sink.state(WorkerState::Verifying, "收尾并采集证据");
    status.set_state("verifying");

    let mut result = TaskResult::new(&task_id, TaskStatus::Unknown, String::new());
    result.session_id = Some(session_id);
    result.runtime_version = runtime_version;
    result.policy = Some(prepared.plan.policy.clone());
    result.usage = projection.usage.clone();
    result.summary = summary_text(&projection);
    result.verification = build_verification(&prepared.plan, &projection, &run_dir);
    let final_message = run_dir.join("final-message.md");
    let _ = std::fs::write(&final_message, projection.assistant_text());
    result.artifacts = vec![
        arc_deck_core::result::ArtifactRef {
            kind: "events".to_string(),
            path: run_dir.join("events.jsonl").display().to_string(),
        },
        arc_deck_core::result::ArtifactRef {
            kind: "final-message".to_string(),
            path: final_message.display().to_string(),
        },
    ];
    if projection.orphans > 0 {
        // 真实日志里稳定存在少量没有结果的调用，所以只报数字，不判失败。
        let _ = sink.diagnostic(
            "warn",
            Some("ORPHAN_TOOL_RESULTS"),
            &format!("有 {} 条工具结果没有对应的调用记录", projection.orphans),
        );
    }
    if !projection.failures.is_empty() {
        result.blocker("MODEL_FAILURE", projection.failures.join(" / "));
    }
    if prepared.plan.acceptance.is_empty() {
        result.unverified.push(
            "任务书没有声明验收项：completed 只表示内核正常终止，不代表行为已验证".to_string(),
        );
    }
    if status.status.attempt > 1 {
        result.unverified.push(format!(
            "本任务续跑过（共 {} 次运行）：续跑不是 exactly-once，前一次未提交的改动由后一次接手",
            status.status.attempt
        ));
    }
    collect_git_facts(prepared, &mut result);

    // 先记下来：下面的 match 会把 outcome 里的字符串移出去。
    let was_cancelled = matches!(outcome, RunOutcome::Cancelled(_));

    result.status = match outcome {
        RunOutcome::TurnEnded if exhausted == Some("TOOL_CALL_AS_TEXT") => {
            result.blocker("TOOL_CALL_AS_TEXT", "模型把工具调用当成正文吐出后收工，续跑次数已用尽");
            TaskStatus::Failed
        }
        RunOutcome::TurnEnded => match projection.last_end() {
            None => {
                result.blocker("NO_TERMINAL", "运行时已空闲，但没有收到 turn/end");
                TaskStatus::Unknown
            }
            Some(end) => match end.kind.as_str() {
                "completed" => result.derive_status_from_verification(),
                "blocked" => {
                    result.blocker(
                        "TURN_BLOCKED",
                        end.message.clone().unwrap_or_else(|| "运行时报告受阻".into()),
                    );
                    TaskStatus::Blocked
                }
                "error" => {
                    if projection.failures.is_empty() {
                        result.blocker(
                            "TURN_ERROR",
                            end.message.clone().unwrap_or_else(|| "运行时报告错误".into()),
                        );
                    }
                    TaskStatus::Failed
                }
                "max-tokens" => {
                    result.blocker("MAX_TOKENS", "至少一步撞到了输出上限，结果可能不完整");
                    TaskStatus::Blocked
                }
                "aborted" | "interrupted" => {
                    result.blocker(
                        "TURN_INTERRUPTED",
                        end.message.clone().unwrap_or_else(|| "轮次被中断".into()),
                    );
                    TaskStatus::Cancelled
                }
                other => {
                    result.blocker("TURN_UNKNOWN_REASON", format!("未处理的 turn/end 原因 `{other}`"));
                    TaskStatus::Failed
                }
            },
        },
        RunOutcome::Stalled(reason) => {
            result.blocker("STALLED", format!("{reason}；续跑次数已用尽"));
            TaskStatus::Failed
        }
        RunOutcome::Cancelled(reason) => {
            result.blocker("CANCELLED", reason);
            TaskStatus::Cancelled
        }
        RunOutcome::TimedOut => {
            result.blocker(
                "TASK_TIMEOUT",
                format!("超过墙钟预算 {} 秒", prepared.plan.timeout_seconds),
            );
            TaskStatus::TimedOut
        }
        RunOutcome::Violation(reason) => {
            result.blocker("PROTOCOL_ERROR", reason);
            TaskStatus::Failed
        }
        RunOutcome::RuntimeExited(reason) => {
            result.blocker("RUNTIME_EXITED", reason);
            TaskStatus::Unknown
        }
        RunOutcome::NoTerminal => {
            result.blocker("NO_TERMINAL", "运行时已空闲，但没有收到 turn/end");
            TaskStatus::Unknown
        }
    };

    if was_cancelled && !termination.exited {
        result.blocker("CANCEL_NOT_CONFIRMED", termination.detail.clone());
    }

    finish(&mut sink, prepared, result, termination, &mut status)
}

/// 工具调用被当正文吐出（Ether 实测：`<｜DSML｜tool_calls>` 没被解析，运行时把它当最终回复收工）。
/// 只看最后一轮正文的末尾：中途出现过但最终正常收尾的不算（Ether runner 同款判据）。
fn tool_call_as_text(projection: &Projection) -> bool {
    let Some(turn) = projection.turns().last() else { return false };
    if turn.end.as_ref().map(|e| e.kind.as_str()) != Some("completed") {
        return false;
    }
    let text = turn.assistant.text();
    let chars: Vec<char> = text.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(4000)..].iter().collect();
    tail.contains("DSML｜tool_calls") || tail.contains("<｜tool▁calls▁begin｜>")
}

/// `git rev-list --count <base>..HEAD`。不是 git 仓或基线不存在时返回 None。
fn git_commit_count(workspace: &Path, base: &str) -> Option<u64> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["rev-list", "--count", &format!("{base}..HEAD")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn git_lines(workspace: &Path, args: &[&str]) -> Option<Vec<String>> {
    let out = std::process::Command::new("git").arg("-C").arg(workspace).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// 回报里「Arc Deck 采集的事实」：提交、相对基线的变更、未提交文件。不信模型自报。
fn collect_git_facts(prepared: &Prepared, result: &mut TaskResult) {
    let Some(base) = &prepared.config.git_base else { return };
    let ws = &prepared.plan.workspace;
    if let Some(commits) = git_lines(ws, &["log", "--format=%h %s", &format!("{base}..HEAD")]) {
        result.commits = commits;
    }
    let mut changes = Vec::new();
    if let Some(lines) = git_lines(ws, &["diff", "--name-status", &format!("{base}...HEAD")]) {
        for line in lines {
            let mut parts = line.split('\t');
            let code = parts.next().unwrap_or("");
            let path = parts.last().unwrap_or("").to_string();
            let kind = match code.chars().next() {
                Some('A') => "added",
                Some('D') => "deleted",
                Some('M') | Some('R') | Some('C') => "modified",
                _ => "unknown",
            };
            changes.push(arc_deck_core::result::ChangeRecord {
                path,
                kind: kind.to_string(),
                evidence: Some(format!("git diff --name-status {base}...HEAD")),
            });
        }
    }
    if let Some(lines) = git_lines(ws, &["status", "--porcelain"]) {
        for line in lines {
            changes.push(arc_deck_core::result::ChangeRecord {
                path: line.get(3..).unwrap_or("").to_string(),
                kind: "uncommitted".to_string(),
                evidence: Some(format!("git status --porcelain：{}", line.get(..2).unwrap_or(""))),
            });
        }
    }
    result.changes = changes;
}

/// 收尾阶段的结论。
enum RunOutcome {
    TurnEnded,
    Stalled(String),
    Cancelled(String),
    TimedOut,
    Violation(String),
    RuntimeExited(String),
    NoTerminal,
}

/// 入站消息的处理结果。
enum Absorbed {
    Ok,
    Closed,
    Fatal(String),
}

fn absorb(
    item: Option<Inbound>,
    sink: &mut EventSink,
    projection: &mut Projection,
    announced: &mut AnnounceState,
    status: &mut StatusWriter,
) -> Absorbed {
    match item {
        None | Some(Inbound::Eof) => Absorbed::Closed,
        Some(Inbound::Fatal { message }) => Absorbed::Fatal(format!("读取运行时输出失败：{message}")),
        // SDK 独占 stdout，非 JSON 行就是协议违规。静默跳过会把「协议已经不对了」
        // 这种状态留到后面以别的形式爆出来，那时原因就难找了。
        Some(Inbound::ProtocolError { message }) => Absorbed::Fatal(message),
        Some(Inbound::UnmatchedResponse { id }) => {
            Absorbed::Fatal(format!("收到没有对应在途请求的响应（id={id}）"))
        }
        Some(Inbound::ServerRequest { id, method }) => {
            let _ = sink.diagnostic(
                "warn",
                Some("SERVER_REQUEST"),
                &format!("运行时发来请求 `{method}`（id={id}）；已按协议回 -32601"),
            );
            Absorbed::Ok
        }
        Some(Inbound::Notification { method: name, params }) => {
            if name == protocol::notification::SESSION_EVENT {
                match serde_json::from_value::<SessionEventParams>(params) {
                    Ok(parsed) => {
                        status.observe(&parsed.event);
                        projection.apply_notification(&parsed);
                        mirror_tools(sink, projection, announced);
                    }
                    Err(e) => {
                        let _ = sink.diagnostic(
                            "warn",
                            Some("SESSION_EVENT_SHAPE"),
                            &format!("session.event 参数无法解析：{e}"),
                        );
                    }
                }
            }
            Absorbed::Ok
        }
    }
}

/// 已通报过的工具事件。工具调用与结果分别追踪：结果可能比调用晚到很多，
/// 用同一份集合会让「结果先到、调用后到」这种情况漏报。
#[derive(Default)]
struct AnnounceState {
    calls: BTreeSet<String>,
    results: BTreeSet<String>,
}

/// 把新出现的工具调用/结果镜像成产品事件。已通报过的不重复发。
fn mirror_tools(sink: &mut EventSink, projection: &Projection, announced: &mut AnnounceState) {
    for turn in projection.turns() {
        for tool in &turn.tools {
            if tool.call_id.is_empty() {
                continue;
            }
            if !announced.calls.contains(&tool.call_id) {
                let summary = tool
                    .command()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| format!("{} 调用", tool.name));
                if sink.tool_call(&tool.call_id, &tool.name, &summary) == WriteOutcome::StdoutBroken {
                    return;
                }
                announced.calls.insert(tool.call_id.clone());
            }
            if let Some(outcome) = &tool.outcome {
                if !announced.results.contains(&tool.call_id) {
                    let status = if outcome.is_error {
                        "error"
                    } else {
                        match outcome.exit_code {
                            Some(0) | None => "ok",
                            Some(_) => "failed",
                        }
                    };
                    let summary = match outcome.exit_code {
                        Some(code) => format!("退出码 {code}"),
                        None => outcome.text.lines().next().unwrap_or("").to_string(),
                    };
                    if sink.tool_result(&tool.call_id, &tool.name, status, &summary)
                        == WriteOutcome::StdoutBroken
                    {
                        return;
                    }
                    announced.results.insert(tool.call_id.clone());
                }
            }
        }
    }
}

/// 断言式地等一个请求的响应，同时在途消费入站消息。
///
/// 不能只 `await` 那个请求：那样一来，运行时的协议违规、进程退出与取消请求
/// 都要等到调用超时才被发现 —— 而它们本该立刻改变结论。
#[allow(clippy::too_many_arguments)]
async fn call_watched(
    transport: &Arc<Transport>,
    inbound: &mut mpsc::UnboundedReceiver<Inbound>,
    sink: &mut EventSink,
    projection: &mut Projection,
    watcher: &mut CancelWatcher,
    announced: &mut AnnounceState,
    status: &mut StatusWriter,
    method_name: &str,
    params: Value,
    timeout: Duration,
) -> CallOutcome {
    let future = transport.call(method_name, params, timeout);
    tokio::pin!(future);
    // 同主循环：定时器建在循环外面，否则事件密集时它永远走不完。
    let mut cancel_poll = tokio::time::interval(CANCEL_POLL);
    cancel_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            result = &mut future => {
                return match result {
                    Ok(value) => CallOutcome::Value(value),
                    Err(DshError::Timeout(m)) => CallOutcome::Failed(
                        "RUNTIME_TIMEOUT".to_string(),
                        format!("等待 `{m}` 响应超时（{} ms）", timeout.as_millis()),
                    ),
                    Err(DshError::Exited) => CallOutcome::Failed(
                        "RUNTIME_EXITED".to_string(),
                        format!("等待 `{method_name}` 时运行时进程退出了"),
                    ),
                    Err(DshError::Io(e)) => CallOutcome::Failed(
                        "RUNTIME_IO".to_string(),
                        format!("`{method_name}` 写入失败：{e}"),
                    ),
                    Err(DshError::Protocol(e)) => CallOutcome::Failed("PROTOCOL_ERROR".to_string(), e),
                    Err(DshError::Remote { code: rpc_code, message }) => CallOutcome::Failed(
                        "RUNTIME_REMOTE_ERROR".to_string(),
                        format!("`{method_name}` 被拒绝：code={rpc_code} message={message}"),
                    ),
                };
            }
            item = inbound.recv() => {
                match absorb(item, sink, projection, announced, status) {
                    Absorbed::Ok => {}
                    Absorbed::Closed => return CallOutcome::Failed(
                        "RUNTIME_EXITED".to_string(),
                        format!("等待 `{method_name}` 时运行时输出已关闭"),
                    ),
                    Absorbed::Fatal(reason) => return CallOutcome::Failed("PROTOCOL_ERROR".to_string(), reason),
                }
                if !sink.stdout_alive() {
                    return CallOutcome::Cancelled("stdout 消费端已断开".to_string());
                }
            }
            _ = cancel_poll.tick() => {
                match watcher.poll() {
                    CancelSignal::Requested(reason) => return CallOutcome::Cancelled(reason),
                    CancelSignal::Invalid(detail) => {
                        let _ = sink.diagnostic("warn", Some("CANCEL_FILE_INVALID"), &detail);
                    }
                    CancelSignal::None => {}
                }
            }
        }
    }
}

enum CallOutcome {
    Value(Value),
    Failed(String, String),
    Cancelled(String),
}

fn apply_call_failure(result: &mut TaskResult, stage: &str, outcome: CallOutcome) {
    match outcome {
        CallOutcome::Value(_) => {}
        CallOutcome::Failed(blocker_code, message) => {
            result.summary = format!("在 `{stage}` 阶段失败：{message}");
            result.blocker(&blocker_code, message);
            // 运行时没能完成这个阶段（协议违规、进程退出、超时、远端拒绝），
            // 结论是 failed 而不是 unknown：unknown 留给「跑完了但看不出结果」。
            result.status = TaskStatus::Failed;
        }
        CallOutcome::Cancelled(reason) => {
            result.summary = format!("在 `{stage}` 阶段被取消：{reason}");
            result.status = TaskStatus::Cancelled;
            result.blocker("CANCELLED", reason);
        }
    }
}

/// 有序收尾：先请对端关闭，再结束进程树，并确认它真的退出了。
async fn shutdown_and_terminate(
    transport: &Arc<Transport>,
    process: &mut RuntimeProcess,
) -> ProcessTermination {
    // `shutdown` 是礼貌请求；对端不退，后面的结束才是实际手段。
    let _ = transport
        .call(method::SHUTDOWN, Value::Null, Duration::from_secs(5))
        .await;
    let termination = process.terminate_tree(TERMINATE_GRACE).await;
    ProcessTermination {
        exited: termination.exited,
        detail: termination.detail,
    }
}

struct ProcessTermination {
    exited: bool,
    detail: String,
}

fn process_termination_note(exited: bool, detail: &str) -> ProcessTermination {
    ProcessTermination { exited, detail: detail.to_string() }
}

fn spawn_stderr_drain(stderr: ChildStderr, log_path: PathBuf) -> Arc<Mutex<String>> {
    let buffer = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&buffer);
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        // 全文另存 runtime.log（追加：续跑的几次运行写在同一个文件里）。
        let mut log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path).ok();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    if let Some(file) = log.as_mut() {
                        let _ = file.write_all(line.as_bytes());
                    }
                    let mut guard = sink.lock().await;
                    // 上界：崩溃时的堆栈可能很长，但结果文件不该被它撑爆。
                    if guard.len() < 32 * 1024 {
                        guard.push_str(&line);
                    }
                }
                Err(_) => break,
            }
        }
    });
    buffer
}

async fn append_stderr(
    result: &mut TaskResult,
    stderr_tail: &Arc<Mutex<String>>,
    sink: &mut EventSink,
) -> Result<(), ()> {
    let text = stderr_tail.lock().await.clone();
    if text.trim().is_empty() {
        return Ok(());
    }
    let _ = sink.diagnostic(
        "warn",
        Some("RUNTIME_STDERR"),
        &format!("运行时 stderr（截断到 32 KiB）：{}", text.trim()),
    );
    result
        .unverified
        .push(format!("运行时 stderr 有输出，见事件日志：{}", text.trim().lines().next().unwrap_or("")));
    Ok(())
}

/// 组装验收证据：**只读工具执行记录**，不自己再跑一遍命令。
fn build_verification(
    plan: &ExecutionPlan,
    projection: &Projection,
    run_dir: &Path,
) -> Vec<VerificationRecord> {
    let evidence = format!("原始事件：{}", run_dir.join("events.jsonl").display());
    plan.acceptance
        .iter()
        .map(|acceptance| match acceptance {
            Acceptance::Command { id, command, cwd, .. } => {
                let executed = projection
                    .turns()
                    .iter()
                    .flat_map(|turn| turn.tools.iter())
                    .filter(|tool| tool.command().is_some_and(|c| command_matches(c, command)))
                    .next_back();
                match executed {
                    None => VerificationRecord {
                        id: id.clone(),
                        kind: VerificationKind::Command,
                        command: Some(command.clone()),
                        cwd: cwd.as_ref().map(|c| c.display().to_string()),
                        exit_code: None,
                        evidence: Some(evidence.clone()),
                        outcome: VerificationOutcome::NotRun,
                        detail: Some("没有观察到与该命令匹配的工具执行记录".to_string()),
                    },
                    Some(tool) => {
                        let (outcome, exit_code, detail) = match &tool.outcome {
                            None => (
                                VerificationOutcome::NotRun,
                                None,
                                Some("工具被调用但没有结果记录".to_string()),
                            ),
                            Some(outcome) if outcome.is_error => (
                                VerificationOutcome::Failed,
                                outcome.exit_code,
                                Some("工具报错结束".to_string()),
                            ),
                            Some(outcome) => match outcome.exit_code {
                                Some(0) => (VerificationOutcome::Passed, Some(0), None),
                                Some(code) => (
                                    VerificationOutcome::Failed,
                                    Some(code),
                                    Some(format!("退出码 {code}")),
                                ),
                                None => (
                                    VerificationOutcome::NotRun,
                                    None,
                                    Some("结果里没有退出码，无法判定".to_string()),
                                ),
                            },
                        };
                        VerificationRecord {
                            id: id.clone(),
                            kind: VerificationKind::Command,
                            command: Some(command.clone()),
                            cwd: cwd.as_ref().map(|c| c.display().to_string()),
                            exit_code,
                            evidence: Some(evidence.clone()),
                            outcome,
                            detail,
                        }
                    }
                }
            }
            Acceptance::Manual { id, description } => VerificationRecord {
                id: id.clone(),
                kind: VerificationKind::Manual,
                command: None,
                cwd: None,
                exit_code: None,
                evidence: None,
                outcome: VerificationOutcome::Pending,
                detail: Some(format!("等待主控或人工判断：{description}")),
            },
        })
        .collect()
}

/// M1 的命令匹配规则：把空白折叠后，整串相等，**或者某个 shell 片段相等**。
///
/// 片段按 `&&`、`;`、`|`、换行切开 —— 这样 `cd x && cargo test y` 能命中，
/// 而 `echo cargo test y` 不会（后缀匹配会把它当成命中，那是误报）。
/// 刻意保守而不是做模糊匹配：把「有点像」当成命中，会让验收在没人察觉的
/// 情况下通过。M2 接真实内核时按实际执行记录收紧。
fn command_matches(executed: &str, expected: &str) -> bool {
    fn normalize(raw: &str) -> String {
        raw.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    let expected = normalize(expected);
    if expected.is_empty() {
        return false;
    }
    let executed = normalize(executed);
    if executed == expected {
        return true;
    }
    executed
        .split([';', '\n'])
        .flat_map(|part| part.split("&&"))
        .flat_map(|part| part.split("||"))
        .flat_map(|part| part.split('|'))
        .map(str::trim)
        .any(|segment| segment == expected)
}

fn summary_text(projection: &Projection) -> String {
    let text = projection.assistant_text();
    let text = text.trim();
    if text.is_empty() {
        return "运行时没有产出助手正文".to_string();
    }
    const LIMIT: usize = 4000;
    if text.chars().count() <= LIMIT {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(LIMIT).collect();
        out.push_str("…（已截断）");
        out
    }
}

fn finish(
    sink: &mut EventSink,
    prepared: &Prepared,
    mut result: TaskResult,
    termination: ProcessTermination,
    status: &mut StatusWriter,
) -> i32 {
    let _ = sink.diagnostic(
        if termination.exited { "info" } else { "warn" },
        Some("RUNTIME_TERMINATION"),
        &termination.detail,
    );

    // 先把结果写盘，再发 result 事件：让 stdout 上的结论与磁盘上的文件永远一致。
    //
    // 两处写入都必须成功才允许退 0：
    // - 状态目录里的运行记录 —— 没有它，同一个 task_id 下一次会被**重跑一遍**，
    //   那正是「不自动重跑」这条规则要防的事。
    // - `--result` 指定的交付物 —— 主控拿不到就不能算完成。
    let mut write_failure: Option<String> = None;
    let run_record = prepared.run_dir.join("result.json");
    if let Err(e) = write_result_file(&run_record, &result) {
        write_failure = Some(format!("写入状态记录 {} 失败：{e}", run_record.display()));
    }
    if let Some(path) = &prepared.result_path {
        if let Err(e) = write_result_file(path, &result) {
            write_failure = Some(match write_failure {
                Some(previous) => format!("{previous}；写入交付物 {} 失败：{e}", path.display()),
                None => format!("写入交付物 {} 失败：{e}", path.display()),
            });
        }
    }
    if let Some(detail) = write_failure {
        result.blocker("RESULT_WRITE_FAILED", detail);
        result
            .unverified
            .push("结果没有完整落盘：不能仅凭 stdout 判定本任务完成".to_string());
        result.status = TaskStatus::Unknown;
    }

    let _ = sink.state(WorkerState::Terminal, &format!("终态 {}", result.status.as_str()));
    let _ = sink.result(&result);
    // 状态文件最后一个写：读者看到 terminal 时，result.json 一定已经在了。
    if result.status != TaskStatus::Completed {
        if let Some(b) = result.blockers.first() {
            status.error(&b.code, &b.message);
        }
    }
    status.status.usage = result.usage.clone();
    if prepared.config.git_base.is_some() {
        status.status.counters.commits = Some(result.commits.len() as u64);
    }
    status.status.terminal = Some(true);
    status.set_activity("idle", None, None);
    status.set_state(result.status.as_str());
    result.status.exit_code()
}

fn emit_spec_error(error: &SpecError) {
    let payload = json!({
        "schema_version": arc_deck_core::SCHEMA_VERSION,
        "type": "spec_error",
        "data": { "code": error.code, "message": error.message },
    });
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{payload}");
}

fn describe_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "（无）".to_string()
    } else {
        paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_matching_is_exact_or_per_shell_segment() {
        assert!(command_matches("cargo test session_filter", "cargo test session_filter"));
        assert!(command_matches("cargo   test  session_filter", "cargo test session_filter"));
        // 组合命令：命中其中一段就算命中。
        assert!(command_matches("cd crates && cargo test session_filter", "cargo test session_filter"));
        assert!(command_matches("cargo test session_filter; echo done", "cargo test session_filter"));
        // 关键负例：只沾一点边的不能算命中。
        assert!(!command_matches("echo cargo test session_filter", "cargo test session_filter"));
        assert!(!command_matches("cargo test", "cargo test session_filter"));
        assert!(!command_matches("anything", ""));
    }

    #[test]
    fn summary_is_truncated_with_a_visible_marker() {
        let mut projection = Projection::new();
        let long = "字".repeat(5000);
        projection.apply(&serde_json::json!({
            "type":"assistant/message","turn":1,"step":1,
            "message":{"content":[{"type":"text","text":long}]}
        }));
        let summary = summary_text(&projection);
        assert!(summary.ends_with("…（已截断）"));
        assert!(summary.chars().count() < 5000);
    }

    #[test]
    fn empty_projection_says_so_instead_of_faking_a_summary() {
        let projection = Projection::new();
        assert_eq!(summary_text(&projection), "运行时没有产出助手正文");
    }
}
