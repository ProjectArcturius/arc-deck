//! `arcdeck` 的 headless 命令面（PRODUCT_PLAN §6.0）。
//!
//! `run` 把用户级配置（`arcdeck.json`）与任务书（Ether 的 Markdown 任务书或 JSON 任务书）
//! **物化**成 M1 的启动配置与任务书，落在任务状态目录里，再交给同一套 worker 执行 ——
//! 这样每次运行实际用了什么路由、什么授权都留档可查，且 worker 只有一条执行路径。
//!
//! 其余命令只读写状态目录里的文件：`status.json`、`result.json`、`events.jsonl`、`cancel.json`。
//! 控制面是文件而不是服务，所以 `ssh ether arcdeck status --all` 就能看 Debian 上的工人。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arc_deck_core::agent::PromptScope;
use arc_deck_core::authz::Permission;
use arc_deck_core::config::{
    AgentSources, Authorization, Liveness, ModelRoute, RuntimeSpec, Timeouts, WorkerConfig,
};
use arc_deck_core::error::{code, SpecError};
use arc_deck_core::provider::{self, AuthStore};
use arc_deck_core::result::{TaskResult, TaskStatus, Usage};
use arc_deck_core::task::{Acceptance, TaskSpec};
use arc_deck_core::userconfig::{self, UserConfig};
use arc_deck_core::{EXIT_SPEC_ERROR, SCHEMA_VERSION};
use arc_deck_dsh::runtime::RuntimeLayout;

use crate::args::{TaskSource, WorkerArgs};
use crate::status::{self, StatusFile};

/// 内置的 Agent 定义与提示词。安装后的 `arcdeck` 不能依赖仓库路径，所以编进二进制，
/// 每次运行写进状态目录（顺带留档：这次运行用的是哪一版提示词）。
const EMBEDDED_AGENTS: &[(&str, &str)] = &[
    ("echo.md", include_str!("../../../agents/echo.md")),
    ("build.md", include_str!("../../../agents/build.md")),
    ("plan.md", include_str!("../../../agents/plan.md")),
];
const EMBEDDED_PROMPTS: &[(&str, &str)] = &[
    ("arc-deck-engineering.md", include_str!("../../../prompts/arc-deck-engineering.md")),
    ("build-behavior.md", include_str!("../../../prompts/build-behavior.md")),
    ("echo-style.md", include_str!("../../../prompts/echo-style.md")),
    ("plan-behavior.md", include_str!("../../../prompts/plan-behavior.md")),
];

type CmdResult = Result<i32, SpecError>;

/// 入口：`argv` 从子命令名开始。
pub async fn dispatch(argv: &[String]) -> i32 {
    let outcome = match argv[0].as_str() {
        "run" => cmd_run(&argv[1..]).await,
        "status" | "ls" => cmd_status(&argv[1..], argv[0] == "ls"),
        "wait" => cmd_wait(&argv[1..]).await,
        "report" => cmd_report(&argv[1..]),
        "logs" => cmd_logs(&argv[1..]),
        "cancel" => cmd_cancel(&argv[1..]).await,
        "usage" => cmd_usage(&argv[1..]),
        "auth" => cmd_auth(&argv[1..]),
        "runtime" => cmd_runtime(&argv[1..]),
        "config" => cmd_config(&argv[1..]),
        other => Err(SpecError::new(code::CONFIG_INVALID, format!("未知命令 `{other}`"))),
    };
    match outcome {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{}: {}", e.code, e.message);
            EXIT_SPEC_ERROR
        }
    }
}

// ── 参数小工具 ──────────────────────────────────────────────────────────

struct Flags {
    positional: Vec<String>,
    values: BTreeMap<String, Vec<String>>,
    switches: Vec<String>,
}

impl Flags {
    /// `value_flags` 是带取值的开关；其余以 `--` 开头的都当布尔开关，且必须在 `switch_flags` 里。
    fn parse(argv: &[String], value_flags: &[&str], switch_flags: &[&str]) -> Result<Self, SpecError> {
        let mut flags = Flags { positional: Vec::new(), values: BTreeMap::new(), switches: Vec::new() };
        let mut i = 0;
        while i < argv.len() {
            let a = &argv[i];
            if value_flags.contains(&a.as_str()) {
                let v = argv.get(i + 1).ok_or_else(|| {
                    SpecError::new(code::CONFIG_INVALID, format!("`{a}` 后面缺少取值"))
                })?;
                flags.values.entry(a.clone()).or_default().push(v.clone());
                i += 2;
                continue;
            }
            if a.starts_with("--") {
                if !switch_flags.contains(&a.as_str()) {
                    return Err(SpecError::new(code::CONFIG_INVALID, format!("不认识的参数 `{a}`")));
                }
                flags.switches.push(a.clone());
            } else {
                flags.positional.push(a.clone());
            }
            i += 1;
        }
        Ok(flags)
    }
    fn one(&self, name: &str) -> Option<&str> {
        self.values.get(name).and_then(|v| v.last()).map(String::as_str)
    }
    fn all(&self, name: &str) -> Vec<String> {
        self.values.get(name).cloned().unwrap_or_default()
    }
    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }
}

/// 状态目录：有配置用配置的，没有就用缺省 —— 只读命令不该因为没写配置而用不了。
fn state_root(flags: &Flags) -> PathBuf {
    if let Some(p) = flags.one("--state-root") {
        return PathBuf::from(p);
    }
    match UserConfig::locate_and_load(flags.one("--config").map(Path::new)) {
        Ok((cfg, _)) => cfg.state_root(),
        Err(_) => userconfig::default_state_root(),
    }
}

fn run_dir(root: &Path, id: &str) -> PathBuf {
    root.join("runs").join(id)
}

fn absolute(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    // 去掉 `.` 与 `..`，但不解析符号链接（canonicalize 在 Windows 上会加 `\\?\` 前缀）。
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn sanitize_id(raw: &str) -> String {
    let id: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    id.trim_matches('-').to_string()
}

fn parse_duration(raw: &str) -> Result<u64, SpecError> {
    let bad = || SpecError::new(code::CONFIG_INVALID, format!("时长 `{raw}` 不合法（例：90、30m、2h）"));
    let (num, mul) = match raw.chars().last() {
        Some('s') => (&raw[..raw.len() - 1], 1),
        Some('m') => (&raw[..raw.len() - 1], 60),
        Some('h') => (&raw[..raw.len() - 1], 3600),
        _ => (raw, 1),
    };
    num.parse::<u64>().map(|n| n * mul).map_err(|_| bad())
}

// ── run ────────────────────────────────────────────────────────────────

async fn cmd_run(argv: &[String]) -> CmdResult {
    let flags = Flags::parse(
        argv,
        &["--config", "--workspace", "--id", "--accept", "--preamble", "--worktree", "--git-base", "--prompt",
          "--model", "--timeout", "--agent", "--state-root", "--by", "--format", "--permission",
          "--effort"],
        &["--detach", "--rerun"],
    )?;
    // TUI 粘贴可能超过 Windows 的命令行长度，也可能恰好是文件名。
    // 显式 prompt 从 stdin 读原文，避免把用户输入误当任务书或经 shell 插值。
    let literal_prompt = flags.one("--prompt");
    let task_arg = match (flags.positional.as_slice(), literal_prompt) {
        ([], Some("-")) => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)
                .map_err(|e| SpecError::new(code::TASK_UNREADABLE, e.to_string()))?;
            text
        }
        ([], Some(text)) => text.to_string(),
        ([arg], None) => arg.clone(),
        _ => return Err(SpecError::new(code::CONFIG_INVALID,
            "用法：arcdeck run <任务书|指令> 或 arcdeck run --prompt <原文|->；二者不能同时提供")),
    };
    if task_arg.trim().is_empty() {
        return Err(SpecError::new(code::TASK_INVALID, "指令不能为空"));
    }
    let (cfg, cfg_path) = UserConfig::locate_and_load(flags.one("--config").map(Path::new))?;
    let root = flags.one("--state-root").map(PathBuf::from).unwrap_or_else(|| cfg.state_root());

    // ── 一般模式 vs 工人模式 ───────────────────────────────────────────────
    //
    // 判据是**工人参数**，不是「参数是不是文件」：没有工人参数时，位置参数就是
    // 要对模型说的话（和 `opencode run "…"` 一样），stdout 换成给人看的回答。
    // **Arc Deck 不是一个工地** —— 它只是一个带工人优化方案的 agent（用户 2026-10-14）。
    //
    // 两种模式都落状态目录、都写 result.json：监控程序一视同仁，不必分两套。
    let worker_flags = !flags.all("--accept").is_empty()
        || flags.one("--worktree").is_some()
        || flags.one("--git-base").is_some()
        || flags.one("--by").is_some()
        || flags.one("--preamble").is_some();
    let is_task_book = literal_prompt.is_none() && (task_arg == "-" || Path::new(&task_arg).is_file());
    let (instruction, json_task, default_id) = if is_task_book {
        load_task_source(&task_arg)?
    } else {
        // 看着像路径却不存在，多半是任务书路径打错了。当成一句话发出去会得到一个
        // 莫名其妙的回答，所以这里明确拒绝，而不是替用户猜。
        if literal_prompt.is_none() && !task_arg.contains(char::is_whitespace)
            && (task_arg.ends_with(".md")
                || task_arg.ends_with(".json")
                || task_arg.contains('/')
                || task_arg.contains('\\'))
        {
            return Err(SpecError::new(
                code::TASK_UNREADABLE,
                format!("找不到任务书 `{task_arg}`。要把这句当指令原文用，请写成一个句子；要指文件请核对路径"),
            ));
        }
        (task_arg.to_string(), None, String::new())
    };
    let worker_mode = is_task_book || worker_flags;

    // ── 授权级别与模型强度 ────────────────────────────────────────────────
    //
    // **产品层只有一个说法**：授权级别是 `readonly / workspace / full / confirm`，
    // 模型强度叫 `effort`（opencode 叫 variant，那个名字不用 —— 用户 2026-10-14）。
    // 命令行显式给的优先，其次配置，最后缺省。
    //
    // **一次划定，运行中不再二次修改**：headless 没有交互入口，所以这条天然成立；
    // 工人那边由任务书**收紧**（不能放宽，见 §6.2）。
    let permission = match flags.one("--permission") {
        Some(raw) => Permission::parse(raw)?,
        None => cfg.worker.permission,
    };
    if permission.needs_responder() {
        return Err(SpecError::new(
            code::CONFIG_INVALID,
            "授权级别 `confirm` 需要一个应答者，headless 没有 —— \
             它是交互面（TUI / GUI）的默认；headless 请用 readonly / workspace / full",
        ));
    }
    let effort = flags.one("--effort").map(str::to_string).or_else(|| cfg.effort.clone());

    let mut task_id = match flags.one("--id") {
        Some(id) => sanitize_id(id),
        None => json_task.as_ref().map(|t| t.task_id.clone()).unwrap_or(default_id),
    };
    if task_id.is_empty() {
        // 位置参数是一句话（不是任务书）且没给 `--id`。缺省 ID 必须**每次唯一** ——
        // 同一句话问两遍、或同一句话派两件活儿，不该被「重复 task_id 不重跑」拒掉。
        // （任务书那条路不会走到这里：Markdown 取文件名，JSON 的 `task_id` 是必填。）
        task_id = format!("run-{}", status::now_ms());
    }
    // stdout 的呈现方式：工人模式是控制面 JSONL，一般模式是给人看的文本。
    let format = flags
        .one("--format")
        .map(str::to_string)
        .unwrap_or_else(|| if worker_mode { "jsonl".into() } else { "text".into() });

    // workspace 与 worktree。
    let mut workspace = match (flags.one("--workspace"), &json_task) {
        (Some(w), _) => absolute(Path::new(w)),
        (None, Some(t)) => t.workspace.clone(),
        (None, None) => absolute(Path::new(".")),
    };
    let mut git_base = flags.one("--git-base").map(str::to_string);
    if let Some(branch) = flags.one("--worktree") {
        let (wt, base) = ensure_worktree(&cfg, &workspace, branch, &task_id)?;
        workspace = wt;
        git_base.get_or_insert(base);
        // worktree 里的 `.git` 是个**指向主仓**的指针文件，而主仓在 workspace 之外。
        // 沙箱不是 danger-full-access 时，worktree 里的 git 命令一律失败。
        // 实测（Arch，2026-10-14）：`git status --short` 与 `git log --oneline -1`
        // 都返回 128，整轮以「验收未通过 accept-1（退出 128）」收场，白烧 8342 输入 token。
        // 这件事在派工的那一秒就能说清，不该让工人跑完才知道。
        if permission != Permission::Full {
            eprintln!(
                "arcdeck: 警告 —— permission = `{}` 时 worktree 里的 git 命令会全部失败\
                 （worktree 的 `.git` 指向主仓，主仓在沙箱外）。要提交请用 `--permission full`",
                permission.as_str()
            );
        }
    }

    // 状态目录：同一个 ID 不重跑，除非 --rerun（旧目录改名留档，不删）。
    let dir = run_dir(&root, &task_id);
    prepare_run_dir(&dir, &task_id, flags.has("--rerun"))?;

    // 内置 Agent 定义与提示词写进状态目录（顺带留档：这次运行用的是哪一版）。
    let agents_dir = dir.join("agents");
    let prompts_dir = dir.join("prompts");
    for (d, files) in [(&agents_dir, EMBEDDED_AGENTS), (&prompts_dir, EMBEDDED_PROMPTS)] {
        std::fs::create_dir_all(d).map_err(io_err(d))?;
        for (name, text) in files {
            std::fs::write(d.join(name), text).map_err(io_err(&d.join(name)))?;
        }
    }

    let timeout = match flags.one("--timeout") {
        Some(t) => parse_duration(t)?,
        None => json_task.as_ref().and_then(|t| t.timeout_seconds).unwrap_or(cfg.worker.timeout_seconds),
    };
    // 工人守则只在工人模式下带上：一般模式是普通 agent，不是工地。
    let preamble = if worker_mode {
        flags.one("--preamble").map(|p| absolute(Path::new(p))).or(cfg.worker.preamble.clone())
    } else {
        None
    };

    let mut env = BTreeMap::new();
    for (k, v) in &cfg.env {
        env.insert(
            k.clone(),
            v.replace("{task_id}", &task_id)
                .replace("{workspace}", &workspace.display().to_string())
                .replace("{run_dir}", &dir.display().to_string()),
        );
    }

    // ── 运行时与路由 ─────────────────────────────────────────────────────
    //
    // 测试专用出口：`run` 这条路上没有别的办法接假内核（物化的配置写死了 dsh-sdk），
    // 而它**正是派工的生产路径** —— 没有端到端覆盖就等于没人真走过。
    // 与隐藏子命令同一套纪律：只在 `test-runtime` 构建里存在，发布构建编译不出它。
    #[cfg(feature = "test-runtime")]
    let fixture_scenario = std::env::var("ARCDECK_FIXTURE_SCENARIO")
        .ok()
        .filter(|s| !s.trim().is_empty());
    #[cfg(not(feature = "test-runtime"))]
    let fixture_scenario: Option<String> = None;

    let (runtime_spec, model_route, model_label) = match fixture_scenario {
        // 假内核不碰模型，路由与密钥一律不需要。
        Some(scenario) => (
            RuntimeSpec::FixtureSdk { entry: None, scenario: Some(scenario) },
            None,
            "（假内核）".to_string(),
        ),
        None => {
            let model = flags.one("--model").unwrap_or(&cfg.model).to_string();
            let route = provider::resolve(&model, &cfg.provider, &task_id)?;
            let route_spec = route.to_spec()?;
            let layout = RuntimeLayout::new(cfg.runtime_dir(), cfg.dsh_version());
            layout.check().map_err(|m| SpecError::new(code::RUNTIME_ENTRY_MISSING, m))?;
            // 路由只解析一次：启动配置里写一份，派工失败时写进状态面一份，两边逐字相同。
            let spec = RuntimeSpec::DshSdk {
                node: cfg.runtime.node.clone().unwrap_or_else(|| PathBuf::from("node")),
                entry: layout.entry(),
                version: cfg.dsh_version(),
                profile: "sdk".into(),
                route: Some(route_spec),
                sandbox: permission.sandbox().to_string(),
                session_log_upload: cfg.runtime.session_log_upload,
                preamble,
            };
            let route = Some(ModelRoute {
                provider: route.dsh_provider.clone(),
                model: route.model.clone(),
                reasoning_effort: effort.clone(),
            });
            (spec, route, model)
        }
    };

    let profile = permission.profile();
    // 装什么进 system prompt：工人模式是功能契约；一般模式看个性化开关，
    // **关着就一段都不加**，按供应商本体行事（用户 2026-10-14）。
    let prompt_scope = if worker_mode {
        PromptScope::Worker
    } else if cfg.personalization.enabled {
        PromptScope::Personalized
    } else {
        PromptScope::Bare
    };
    let config = WorkerConfig {
        schema_version: SCHEMA_VERSION,
        runtime: runtime_spec,
        prompt_scope,
        state_root: root.clone(),
        authorization: Authorization {
            workspace: workspace.clone(),
            profile: profile.into(),
            read_paths: Vec::new(),
            write_paths: Vec::new(),
            network: true,
            allow_commit: cfg.worker.allow_commit,
            allow_push: false,
            allowed_agents: vec!["echo".into(), "build".into(), "plan".into()],
        },
        agents: AgentSources { dir: agents_dir, prompts_dir },
        model: model_route.clone(),
        env,
        timeouts: Timeouts { default_task_seconds: timeout, wait_seconds: timeout, ..Timeouts::default() },
        liveness: Liveness {
            heartbeat_seconds: 15,
            model_idle_seconds: cfg.worker.model_idle_seconds,
            tool_stall_seconds: cfg.worker.tool_stall_seconds,
            max_attempts: cfg.worker.max_attempts,
        },
        git_base,
    };

    let mut acceptance: Vec<Acceptance> = json_task.as_ref().map(|t| t.acceptance.clone()).unwrap_or_default();
    let accepts = flags.all("--accept");
    for (i, cmd) in accepts.iter().enumerate() {
        acceptance.push(Acceptance::Command {
            id: format!("accept-{}", i + 1),
            command: cmd.clone(),
            cwd: None,
            timeout_seconds: None,
        });
    }
    let mut instruction = instruction;
    let commands: Vec<&String> = acceptance
        .iter()
        .filter_map(|a| match a {
            Acceptance::Command { command, .. } => Some(command),
            Acceptance::Manual { .. } => None,
        })
        .collect();
    if !commands.is_empty() {
        // 验收证据取自工具执行记录（M1 规则），所以模型必须**原样**跑这些命令。
        instruction.push_str("\n\n---\n\n## 验收命令（Arc Deck 按实际执行记录判定）\n\n完成后必须用 shell 工具**原样**运行下面每一条，不要改写、不要合并成一条：\n\n");
        for c in &commands {
            instruction.push_str(&format!("- `{c}`\n"));
        }
    }
    let task = TaskSpec {
        schema_version: SCHEMA_VERSION,
        task_id: task_id.clone(),
        workspace: workspace.clone(),
        agent: flags.one("--agent").map(str::to_string).or_else(|| json_task.as_ref().and_then(|t| t.agent.clone())),
        mode: json_task.as_ref().and_then(|t| t.mode),
        assigned_by: flags
            .one("--by")
            .map(str::to_string)
            .or_else(|| json_task.as_ref().and_then(|t| t.assigned_by.clone())),
        instruction,
        constraints: json_task.as_ref().and_then(|t| t.constraints.clone()),
        acceptance,
        timeout_seconds: Some(timeout),
    };

    let config_path = dir.join("config.json");
    let task_path = dir.join("task.json");
    write_json(&config_path, &config)?;
    // `workspace` 在 TaskSpec 上不序列化（相对任务书解析），物化时显式写回。
    let mut task_value = serde_json::to_value(&task).unwrap_or_default();
    task_value["workspace"] = serde_json::json!(workspace.display().to_string());
    write_json(&task_path, &task_value)?;
    eprintln!("arcdeck: 配置 {} · 模型 {model_label} · workspace {}", cfg_path.display(), workspace.display());

    let cancel = dir.join("cancel.json");
    if flags.has("--detach") {
        // 派工握手要用到这些事实：工人还没起来，它自己写不出「谁派的、干什么」。
        let facts = DispatchFacts {
            task_id: task_id.clone(),
            workspace: workspace.display().to_string(),
            model: model_route.as_ref().map(|m| format!("{}/{}", m.provider, m.model)),
            assigned_by: task.assigned_by.clone(),
            title: status::derive_title(&task.instruction),
            format: format.clone(),
        };
        return spawn_detached(&dir, &config_path, &task_path, &cancel, &facts);
    }
    let args = WorkerArgs {
        config: config_path,
        task: TaskSource::File(task_path),
        format,
        result: None,
        cancel_file: Some(cancel),
    };
    Ok(crate::worker::run(args).await)
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> SpecError + '_ {
    move |e| SpecError::new(code::CONFIG_INVALID, format!("{} —— {e}", path.display()))
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), SpecError> {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    std::fs::write(path, text).map_err(io_err(path))
}

/// 返回（指令文本，JSON 任务书（如果是），缺省 ID）。
fn load_task_source(arg: &str) -> Result<(String, Option<TaskSpec>, String), SpecError> {
    if arg == "-" {
        let mut raw = String::new();
        std::io::stdin().read_to_string(&mut raw).map_err(|e| SpecError::task_unreadable("<stdin>", e))?;
        return Ok((raw, None, String::new()));
    }
    let path = absolute(Path::new(arg));
    let stem = sanitize_id(&path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
    if path.extension().map(|e| e == "json").unwrap_or(false) {
        let task = TaskSpec::load(&path)?;
        return Ok((task.instruction.clone(), Some(task), stem));
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| SpecError::task_unreadable(path.display(), e))?;
    if raw.trim().is_empty() {
        return Err(SpecError::new(code::TASK_INVALID, format!("任务书 {} 是空的", path.display())));
    }
    Ok((raw, None, stem))
}

/// 同一 ID：有结论且没给 --rerun → 拒绝；正在跑 → 拒绝；其余情况旧目录改名留档。
fn prepare_run_dir(dir: &Path, task_id: &str, rerun: bool) -> Result<(), SpecError> {
    if !dir.exists() {
        return std::fs::create_dir_all(dir).map_err(io_err(dir));
    }
    if let Some(st) = status::read(&dir.join(status::STATUS_FILE)) {
        let h = status::health(&st, status::now_ms());
        if h != "done" && h != "dead" {
            return Err(SpecError::new(
                code::TASK_ALREADY_SEEN,
                format!("任务 `{task_id}` 正在运行（{h}）。先 `arcdeck cancel {task_id}`，或换一个 --id"),
            ));
        }
    }
    let settled = dir.join("result.json").exists();
    if settled && !rerun {
        return Err(SpecError::new(
            code::TASK_ALREADY_SEEN,
            format!("任务 `{task_id}` 已经有结论（arcdeck report {task_id}）。要重跑加 --rerun（旧记录改名保留）"),
        ));
    }
    let aside = dir.with_file_name(format!("{task_id}.{}", status::now_ms()));
    std::fs::rename(dir, &aside).map_err(io_err(dir))?;
    eprintln!("arcdeck: 旧记录移到 {}", aside.display());
    std::fs::create_dir_all(dir).map_err(io_err(dir))
}

/// 建或复用 worktree（Ether runner 同款：分支已存在就复用，worktree 目录已存在就直接用）。
fn ensure_worktree(cfg: &UserConfig, repo: &Path, branch: &str, task_id: &str) -> Result<(PathBuf, String), SpecError> {
    let base = cfg.worktree.as_ref().map(|w| w.base.clone()).unwrap_or_else(|| "main".into());
    let template = cfg
        .worktree
        .as_ref()
        .map(|w| w.dir.clone())
        .unwrap_or_else(|| "{repo_parent}/.arcdeck/wt/{task_id}".into());
    let parent = repo.parent().unwrap_or(repo);
    let wt = absolute(Path::new(
        &template.replace("{repo_parent}", &parent.display().to_string()).replace("{task_id}", task_id),
    ));
    let git = |args: &[&str], cwd: &Path| -> Result<std::process::Output, SpecError> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .map_err(|e| SpecError::new(code::CONFIG_INVALID, format!("运行 git 失败：{e}")))
    };
    if !wt.exists() {
        if let Some(p) = wt.parent() {
            std::fs::create_dir_all(p).map_err(io_err(p))?;
        }
        let exists = git(&["show-ref", "--quiet", &format!("refs/heads/{branch}")], repo)?.status.success();
        let wt_s = wt.display().to_string();
        let out = if exists {
            git(&["worktree", "add", &wt_s, branch], repo)?
        } else {
            git(&["worktree", "add", "-b", branch, &wt_s, &base], repo)?
        };
        if !out.status.success() {
            return Err(SpecError::new(
                code::CONFIG_INVALID,
                format!("git worktree add 失败：{}", String::from_utf8_lossy(&out.stderr).trim()),
            ));
        }
        // 子模块：新 worktree 不带，工人自己拷会弄坏 .git 指针（Ether d1 实测）。失败不致命。
        if wt.join(".gitmodules").exists() {
            let out = git(&["submodule", "update", "--init", "-q"], &wt)?;
            if !out.status.success() {
                eprintln!("arcdeck: ⚠ 子模块初始化失败：{}", String::from_utf8_lossy(&out.stderr).trim());
            }
        }
        eprintln!("arcdeck: worktree {} （分支 {branch}{}）", wt.display(), if exists { "，复用" } else { "" });
    }
    Ok((wt, base))
}

/// 分离运行：子进程脱离当前终端与 ssh 会话，stdout/stderr 落文件。
/// 取代 Ether 手册里的 `nohup … &`（「只加 & 不加 nohup，命令结束时任务被一起杀掉」实测过）。
/// 派工时就要落进状态面的事实 —— 工人还没起来，这些只能由派工方带着。
struct DispatchFacts {
    task_id: String,
    workspace: String,
    model: Option<String>,
    assigned_by: Option<String>,
    title: Option<String>,
    /// 分离出去的子进程也要用同一种呈现方式，否则 `--detach` 会把它换回 JSONL。
    format: String,
}

/// 派工握手的等待上限。工人写出 `status.json` 通常只要几十毫秒（预检全是本地
/// 文件操作）；这个上限只会在**失败**路径上被走满。
const DISPATCH_HANDSHAKE: Duration = Duration::from_secs(10);
const DISPATCH_POLL: Duration = Duration::from_millis(20);

fn spawn_detached(dir: &Path, config: &Path, task: &Path, cancel: &Path, facts: &DispatchFacts) -> CmdResult {
    let exe = std::env::current_exe().map_err(|e| SpecError::new(code::CONFIG_INVALID, format!("{e}")))?;
    let out = std::fs::File::create(dir.join("console.jsonl")).map_err(io_err(dir))?;
    let err = std::fs::File::create(dir.join("console.err")).map_err(io_err(dir))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["worker", "run", "--config"])
        .arg(config)
        .arg("--task")
        .arg(task)
        .arg("--cancel-file")
        .arg(cancel)
        .arg("--format")
        .arg(&facts.format)
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    let mut child = {
        use std::os::windows::process::CommandExt;
        use arc_deck_dsh::process::CREATE_NO_WINDOW;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // **不用 DETACHED_PROCESS。** 与它同用时 CREATE_NO_WINDOW 被忽略（MSDN），监工就
        // 完全没有控制台；于是它拉起的每个控制台子程序 —— 整个任务期间都活着的 node
        // 运行时、刷新 commits 的 git —— 各自新开一个**可见**窗口。实测（2026-10-14）：
        // 监工 `AttachConsole` 失败，运行时 `console, window VISIBLE`。
        // 单用 CREATE_NO_WINDOW：监工拿一个隐藏的**新**控制台（不挂在派工终端上，关终端
        // 不连带杀它），子孙继承这个隐藏控制台，一个窗口都不开。
        no_inherit_std_handles();
        // 先试脱离作业对象（ssh / 终端关掉时连带杀作业里的进程）；作业不允许脱离时退回普通分离。
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        match cmd.spawn() {
            Ok(child) => child,
            Err(_) => {
                cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
                cmd.spawn()
                    .map_err(|e| SpecError::new(code::CONFIG_INVALID, format!("分离启动失败：{e}")))?
            }
        }
    };
    #[cfg(not(windows))]
    let mut child = cmd
        .spawn()
        .map_err(|e| SpecError::new(code::CONFIG_INVALID, format!("分离启动失败：{e}")))?;

    handshake(dir, &mut child, facts)
}

/// 派工进程自己的 stdin/stdout/stderr 不许被监工继承。
///
/// Rust 在 Windows 上以 `bInheritHandles = TRUE` 建子进程，于是父进程里**所有**可继承句柄
/// 都会跟过去 —— 包括调度者 shell 递进来的 stdout 管道。监工握着它的写端，读的那边就等不到
/// EOF：实测 `arcdeck run … --detach | Out-Null` 要等整件任务跑完（22.5 s 的夹具任务等满
/// 22.5 s）才返回；`id=$(arcdeck run … --detach)` 会挂上几个小时。
/// 监工的标准流已经显式指向 `console.*` 文件与 null，所以去掉的只是泄漏，不影响它自己的输出；
/// 本进程之后照常写 stdout（继承标志不影响自身使用）。
#[cfg(windows)]
fn no_inherit_std_handles() {
    // 只为这两个 kernel32 函数不值得引入一个绑定 crate。
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut std::ffi::c_void;
        fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 0x1;
    // STD_INPUT_HANDLE / STD_OUTPUT_HANDLE / STD_ERROR_HANDLE 是 (DWORD)-10 / -11 / -12。
    for which in [-10i32, -11, -12] {
        // SAFETY: 两个函数都只读写句柄标志位；无效或空句柄时 SetHandleInformation 返回失败，
        // 而失败只意味着那个句柄本来就不存在 —— 没有可泄漏的东西，忽略即可。
        unsafe {
            let handle = GetStdHandle(which as u32);
            if !handle.is_null() && handle as isize != -1 {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// 派工握手：**分离不等于成功**。
///
/// 只 `spawn` 就返回 0，会把预检失败（缺密钥、运行时没装、Agent 不认）整个藏进
/// 子进程 —— 派工的人拿到「started」，而 `arcdeck status --all` 报「没有任务」，
/// 原因只留在 `console.err` 里。这是 §6.5 第四个问题（漏了就静默消失）的翻版。
///
/// 判据是确定的，不靠时间猜：状态文件出现 = 接住了；子进程先退出 = 没接住。
/// 只有「还活着但还没写出状态」才退回从前的行为 —— **慢不等于失败**。
fn handshake(dir: &Path, child: &mut std::process::Child, facts: &DispatchFacts) -> CmdResult {
    let pid = child.id();
    let status_path = dir.join(status::STATUS_FILE);
    let deadline = Instant::now() + DISPATCH_HANDSHAKE;
    let mut exited = false;
    loop {
        if status_path.is_file() {
            break;
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                exited = true;
                break;
            }
            Ok(None) => {}
            Err(e) => {
                return Err(SpecError::new(code::CONFIG_INVALID, format!("等待分离进程失败：{e}")));
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(DISPATCH_POLL);
    }

    // 复查一次：`try_wait` 与写盘不是原子的，中间可能有竞态。
    if status_path.is_file() {
        println!("{}  started  pid {pid}  · arcdeck status {}", facts.task_id, facts.task_id);
        return Ok(0);
    }
    if !exited {
        // 还活着，只是慢。报已启动，但把「还没写出状态」这件事说出来。
        eprintln!(
            "arcdeck: 分离进程已起（pid {pid}），但 {} 秒内还没写出状态；用 `arcdeck status {}` 跟踪",
            DISPATCH_HANDSHAKE.as_secs(),
            facts.task_id
        );
        println!("{}  started  pid {pid}  · arcdeck status {}", facts.task_id, facts.task_id);
        return Ok(0);
    }

    // 子进程在写出任何状态之前就退出了。这是**确定的失败**，把它的原因落进状态面
    // 再原样报上去，调度者不必去翻 console.err。
    let (failure_code, message) = read_preflight_error(dir).unwrap_or_else(|| {
        (
            "PREFLIGHT_FAILED".to_string(),
            format!("分离的工人进程在任何状态写出之前退出（pid {pid}）"),
        )
    });
    status::write_preflight_failure(
        dir,
        status::PreflightFailure {
            task_id: facts.task_id.clone(),
            workspace: facts.workspace.clone(),
            model: facts.model.clone(),
            assigned_by: facts.assigned_by.clone(),
            title: facts.title.clone(),
            code: failure_code.clone(),
            message: message.clone(),
        },
    );
    Err(SpecError::new(&failure_code, message))
}

/// 从 `console.err` 里取最后一条 `spec_error`（工人预检失败时写的）。取不到返回 None。
fn read_preflight_error(dir: &Path) -> Option<(String, String)> {
    let raw = std::fs::read_to_string(dir.join("console.err")).ok()?;
    for line in raw.lines().rev() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if value["type"] != "spec_error" {
            continue;
        }
        let code = value["data"]["code"].as_str()?.to_string();
        let message = value["data"]["message"].as_str().unwrap_or_default().to_string();
        return Some((code, message));
    }
    None
}

/// `arcdeck usage`：把状态目录里各任务的 token 用量加起来。
///
/// 只有 provider **实际报告**的用量，**没有费用** —— 没有数据就不填虚构成本（§6.3）。
/// 终态以 `result.json` 为准（那才是交出去的那一份）；未终态用 `status.json` 上此刻的数字，
/// 它只会涨，所以单独标出来。
fn cmd_usage(argv: &[String]) -> CmdResult {
    let flags = Flags::parse(argv, &["--config", "--state-root"], &["--json"])?;
    let root = state_root(&flags);
    let now = status::now_ms();

    struct Row {
        id: String,
        state: String,
        health: &'static str,
        terminal: bool,
        assigned_by: Option<String>,
        usage: Usage,
    }
    let mut rows = Vec::new();
    // 输入 / 缓存命中 / 输出。
    let mut sum = (0u64, 0u64, 0u64);

    for (id, st) in list_runs(&root) {
        let health = status::health(&st, now);
        let terminal = health == "done";
        let usage = if terminal {
            std::fs::read_to_string(run_dir(&root, &id).join("result.json"))
                .ok()
                .and_then(|raw| serde_json::from_str::<TaskResult>(&raw).ok())
                .and_then(|r| r.usage)
                .or_else(|| st.usage.clone())
        } else {
            st.usage.clone()
        };
        let usage = usage.unwrap_or_default();
        sum.0 += usage.prompt_tokens.unwrap_or(0);
        sum.1 += usage.cache_read_tokens.unwrap_or(0);
        sum.2 += usage.completion_tokens.unwrap_or(0);
        rows.push(Row {
            id,
            state: st.state.clone(),
            health,
            terminal,
            assigned_by: st.assigned_by.clone(),
            usage,
        });
    }

    if flags.has("--json") {
        let tasks: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "task_id": r.id,
                    "state": r.state,
                    "health": r.health,
                    "terminal": r.terminal,
                    "assigned_by": r.assigned_by,
                    "usage": r.usage,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "state_root": root.display().to_string(),
                "tasks": tasks,
                "total": {
                    "prompt_tokens": sum.0,
                    "cache_read_tokens": sum.1,
                    "completion_tokens": sum.2,
                },
            })
        );
        return Ok(0);
    }

    if rows.is_empty() {
        println!("（{} 下没有任务）", root.join("runs").display());
        return Ok(0);
    }
    let count = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_else(|| "-".into());
    for r in &rows {
        println!(
            "{}  {}  {}{}  输入 {} · 命中 {} · 输出 {}{}",
            r.id,
            r.state,
            r.health,
            if r.terminal { "" } else { "（未终态，数字还会涨）" },
            count(r.usage.prompt_tokens),
            count(r.usage.cache_read_tokens),
            count(r.usage.completion_tokens),
            r.assigned_by
                .as_ref()
                .map(|b| format!("  派工 {b}"))
                .unwrap_or_default(),
        );
    }
    println!(
        "合计 {} 件：输入 {} · 缓存命中 {} · 输出 {}",
        rows.len(),
        sum.0,
        sum.1,
        sum.2
    );
    println!("（只有 provider 报告的用量，没有费用估算 —— 没有数据就不编）");
    Ok(0)
}

// ── status / wait / report / logs / cancel ─────────────────────────────

fn list_runs(root: &Path) -> Vec<(String, StatusFile)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join("runs")) else { return out };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(st) = status::read(&entry.path().join(status::STATUS_FILE)) {
            // 改名留档的旧记录（`<id>.<毫秒>`）不列。
            if st.task_id == name {
                out.push((name, st));
            }
        }
    }
    out.sort_by_key(|(_, s)| s.started_at);
    out
}

fn cmd_status(argv: &[String], all: bool) -> CmdResult {
    let flags = Flags::parse(argv, &["--config", "--state-root"], &["--json", "--all"])?;
    let root = state_root(&flags);
    let now = status::now_ms();
    let json = flags.has("--json");
    let print = |st: &StatusFile| {
        let h = status::health(st, now);
        if json {
            let mut v = serde_json::to_value(st).unwrap_or_default();
            v["health"] = serde_json::json!(h);
            println!("{v}");
        } else {
            println!("{}", status::one_line(st, h, now));
        }
        status::exit_code(st, h)
    };
    if all || flags.has("--all") || flags.positional.is_empty() {
        let runs = list_runs(&root);
        if runs.is_empty() && !json {
            println!("（{} 下没有任务）", root.join("runs").display());
        }
        for (_, st) in &runs {
            print(st);
        }
        return Ok(0);
    }
    let mut code = 0;
    for id in &flags.positional {
        match status::read(&run_dir(&root, id).join(status::STATUS_FILE)) {
            Some(st) => code = print(&st),
            None => {
                println!("{id}  不存在（{}）", run_dir(&root, id).display());
                code = EXIT_SPEC_ERROR;
            }
        }
    }
    Ok(code)
}

async fn cmd_wait(argv: &[String]) -> CmdResult {
    let flags = Flags::parse(argv, &["--config", "--state-root", "--timeout"], &["--json"])?;
    let [id] = flags.positional.as_slice() else {
        return Err(SpecError::new(code::CONFIG_INVALID, "用法：arcdeck wait <id> [--timeout 30m]"));
    };
    let root = state_root(&flags);
    let path = run_dir(&root, id).join(status::STATUS_FILE);
    let limit = flags.one("--timeout").map(parse_duration).transpose()?;
    let start = std::time::Instant::now();
    loop {
        let now = status::now_ms();
        let Some(st) = status::read(&path) else {
            println!("{id}  不存在（{}）", path.display());
            return Ok(EXIT_SPEC_ERROR);
        };
        let h = status::health(&st, now);
        let timed_out = limit.is_some_and(|l| start.elapsed() >= Duration::from_secs(l));
        if h == "done" || h == "dead" || timed_out {
            println!("{}", status::one_line(&st, h, now));
            return Ok(status::exit_code(&st, h));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn cmd_report(argv: &[String]) -> CmdResult {
    let flags = Flags::parse(argv, &["--config", "--state-root"], &["--json"])?;
    let [id] = flags.positional.as_slice() else {
        return Err(SpecError::new(code::CONFIG_INVALID, "用法：arcdeck report <id> [--json]"));
    };
    let dir = run_dir(&state_root(&flags), id);
    let now = status::now_ms();
    let st = status::read(&dir.join(status::STATUS_FILE));
    let Ok(raw) = std::fs::read_to_string(dir.join("result.json")) else {
        match st {
            Some(st) => {
                let h = status::health(&st, now);
                println!("{}", status::one_line(&st, h, now));
                println!("（还没有结果）");
                return Ok(status::exit_code(&st, h));
            }
            None => {
                println!("{id}  不存在（{}）", dir.display());
                return Ok(EXIT_SPEC_ERROR);
            }
        }
    };
    let result: TaskResult = serde_json::from_str(&raw)
        .map_err(|e| SpecError::new(code::CONFIG_INVALID, format!("result.json 解析失败：{e}")))?;
    if flags.has("--json") {
        println!("{raw}");
        return Ok(result.status.exit_code());
    }
    print!("{}", render_report(&result, st.as_ref(), &dir, now));
    Ok(result.status.exit_code())
}

/// 紧凑回报（PRODUCT_PLAN §6.5.5）：事实与模型陈述分开标。目标约 2 KB。
///
/// 开头四行回答派工的五件事：谁派的（`assigned_by`）、干什么（`title`）、
/// 结果（`status`）、要不要再派一次（`retryable`）—— 「做什么样」在下面的事实里。
fn render_report(r: &TaskResult, st: Option<&StatusFile>, dir: &Path, now: u64) -> String {
    let mut o = String::new();
    let elapsed = st.map(|s| status::human(s.heartbeat_at.saturating_sub(s.started_at) / 1000)).unwrap_or_default();
    let attempts = st.map(|s| s.attempt).unwrap_or(1);
    match st.and_then(|s| s.title.as_deref()) {
        Some(title) => o.push_str(&format!("# {}  {}\n", r.task_id, status::clip(title, 80))),
        None => o.push_str(&format!("# {}\n", r.task_id)),
    }
    o.push_str(&format!(
        "{} · 派工 {} · {elapsed} · 运行 {attempts} 次\n",
        r.status.as_str(),
        st.and_then(|s| s.assigned_by.as_deref()).unwrap_or("（未记录）"),
    ));
    // 「要不要再派一次」是调度者的下一个决定，所以放在最上面 —— 别让它翻到下面才看得到。
    if r.status != TaskStatus::Completed {
        // 单一来源：`status.json` 的 `last_error` 是终态时写下的那一条；没有就退回
        // 第一个 blocker（两者的码在 `finish()` 里是同一个）。
        let code = st
            .and_then(|s| s.last_error.as_ref())
            .map(|e| e.code.clone())
            .or_else(|| r.blockers.first().map(|b| b.code.clone()));
        if let Some(code) = code {
            let verdict = if arc_deck_core::failure::retryable(&code) {
                "可能 —— 这类失败通常是临时的，换一次再派，或等一会儿"
            } else {
                "不能 —— 重派不会让它变好；先改配置或看下面的事实"
            };
            o.push_str(&format!("再派一次能解决吗：{verdict}（{code}）\n"));
        }
    }
    let _ = now;
    o.push_str("\n## Arc Deck 采集的事实\n");
    for b in &r.blockers {
        o.push_str(&format!("- ✗ {}: {}\n", b.code, status::clip(&b.message, 200)));
    }
    for v in &r.verification {
        o.push_str(&format!(
            "- 验收 {} `{}` → {:?}{}\n",
            v.id,
            v.command.as_deref().unwrap_or("-"),
            v.outcome,
            v.exit_code.map(|c| format!("（退出 {c}）")).unwrap_or_default()
        ));
    }
    if !r.commits.is_empty() {
        o.push_str(&format!("- 提交 {} 个：\n", r.commits.len()));
        for c in r.commits.iter().take(12) {
            o.push_str(&format!("  - {}\n", status::clip(c, 100)));
        }
    }
    if !r.changes.is_empty() {
        let committed: Vec<_> = r.changes.iter().filter(|c| c.kind != "uncommitted").collect();
        let dirty: Vec<_> = r.changes.iter().filter(|c| c.kind == "uncommitted").collect();
        o.push_str(&format!("- 变更文件 {}，未提交 {}\n", committed.len(), dirty.len()));
        for c in committed.iter().take(15) {
            o.push_str(&format!("  - {} {}\n", c.kind, c.path));
        }
        for c in dirty.iter().take(8) {
            o.push_str(&format!("  - 未提交 {}\n", c.path));
        }
    }
    if let Some(u) = &r.usage {
        o.push_str(&format!(
            "- 用量：输入 {} · 缓存命中 {} · 输出 {}\n",
            u.prompt_tokens.map(|v| v.to_string()).unwrap_or("-".into()),
            u.cache_read_tokens.map(|v| v.to_string()).unwrap_or("-".into()),
            u.completion_tokens.map(|v| v.to_string()).unwrap_or("-".into()),
        ));
    }
    for u in &r.unverified {
        o.push_str(&format!("- 未验证：{}\n", status::clip(u, 160)));
    }
    o.push_str("\n## 模型的陈述（不是证据）\n");
    let summary = r.summary.trim();
    let chars: Vec<char> = summary.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(1200)..].iter().collect();
    if chars.len() > 1200 {
        o.push_str("…（只取末尾 1200 字；全文见 final-message.md）\n");
    }
    o.push_str(&tail);
    o.push_str(&format!("\n\n— 目录：{}\n", dir.display()));
    o
}

fn cmd_logs(argv: &[String]) -> CmdResult {
    let flags = Flags::parse(argv, &["--config", "--state-root", "--tail"], &["--runtime", "--console"])?;
    let [id] = flags.positional.as_slice() else {
        return Err(SpecError::new(code::CONFIG_INVALID, "用法：arcdeck logs <id> [--tail N] [--runtime|--console]"));
    };
    let dir = run_dir(&state_root(&flags), id);
    let file = if flags.has("--runtime") {
        "runtime.log"
    } else if flags.has("--console") {
        "console.err"
    } else {
        "events.jsonl"
    };
    let n: usize = flags.one("--tail").and_then(|t| t.parse().ok()).unwrap_or(40);
    let text = std::fs::read_to_string(dir.join(file)).map_err(io_err(&dir.join(file)))?;
    let lines: Vec<&str> = text.lines().collect();
    for line in &lines[lines.len().saturating_sub(n)..] {
        println!("{line}");
    }
    Ok(0)
}

async fn cmd_cancel(argv: &[String]) -> CmdResult {
    let flags = Flags::parse(argv, &["--config", "--state-root"], &[])?;
    let [id] = flags.positional.as_slice() else {
        return Err(SpecError::new(code::CONFIG_INVALID, "用法：arcdeck cancel <id>"));
    };
    let dir = run_dir(&state_root(&flags), id);
    let cancel = dir.join("cancel.json");
    // 先看监工还在不在。**不在的时候写取消文件是没用的 —— 没人读它**，而它拉起的
    // 运行时（以及运行时拉起的工具）会一直活着。实测（Arch，2026-10-14）：`kill -9`
    // 监工之后 DSH 运行时仍是活的孤儿，没有任何东西会回收它。
    let status_path = dir.join(status::STATUS_FILE);
    if let Some(st) = status::read(&status_path) {
        if st.terminal != Some(true) && status::supervisor_liveness(&st) == status::Liveness::Gone {
            return Ok(reap_orphan(id, &st));
        }
    }

    if !cancel.exists() {
        let body = serde_json::json!({ "schema_version": SCHEMA_VERSION, "task_id": id, "action": "cancel" });
        std::fs::write(&cancel, body.to_string()).map_err(io_err(&cancel))?;
    }
    // 有界等待收尾。
    for _ in 0..30 {
        if let Some(st) = status::read(&status_path) {
            let h = status::health(&st, status::now_ms());
            if h == "done" || h == "dead" {
                println!("{}", status::one_line(&st, h, status::now_ms()));
                return Ok(status::exit_code(&st, h));
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    // 30 秒都没收尾：再确认一次监工是不是这期间死了。是就自己收 —— 否则这个孤儿
    // 会一直占着工作区与配额。
    if let Some(st) = status::read(&status_path) {
        if status::supervisor_liveness(&st) == status::Liveness::Gone {
            return Ok(reap_orphan(id, &st));
        }
    }
    println!("{id}  已请求取消，30 秒内还没收尾；用 arcdeck status {id} 继续看");
    Ok(10)
}

/// 监工已经死了：取消文件没人读，只能按 `status.json` 里记的 `runtime_pid` 自己收树。
fn reap_orphan(id: &str, st: &StatusFile) -> i32 {
    let Some(pid) = st.runtime_pid else {
        println!("{id}  监工已不在，状态里也没有运行时 pid —— 没有可收的东西");
        return 5;
    };
    let termination = arc_deck_dsh::process::kill_tree_by_pid(pid);
    if termination.exited {
        println!("{id}  监工已不在（未写出终态，按 unknown 处理）；已收运行时进程树 pid {pid}");
        5
    } else {
        println!("{id}  监工已不在；收运行时进程树 pid {pid} 未确认：{}", termination.detail);
        10
    }
}

// ── auth / runtime / config ────────────────────────────────────────────

fn cmd_auth(argv: &[String]) -> CmdResult {
    let path = userconfig::auth_path();
    match argv.first().map(String::as_str) {
        Some("set") => {
            let Some(id) = argv.get(1) else {
                return Err(SpecError::new(code::CONFIG_INVALID, "用法：arcdeck auth set <provider>（密钥从 stdin 读）"));
            };
            eprintln!("arcdeck: 粘贴 `{id}` 的 API key 后回车（Ctrl+Z/Ctrl+D 结束）：");
            let mut key = String::new();
            std::io::stdin().read_to_string(&mut key).map_err(|e| SpecError::new(code::CONFIG_INVALID, e.to_string()))?;
            let key = key.trim();
            if key.is_empty() {
                return Err(SpecError::new(code::CONFIG_INVALID, "没读到密钥"));
            }
            let mut store = AuthStore::load(&path)?;
            store.set_api_key(id, key);
            store.save(&path).map_err(io_err(&path))?;
            println!("已写入 {}（{id}）", path.display());
            Ok(0)
        }
        Some("ls") | None => {
            let store = AuthStore::load(&path)?;
            println!("{}", path.display());
            for (id, e) in &store.entries {
                println!("  {id}  {}", e.kind);
            }
            Ok(0)
        }
        Some("rm") => {
            let Some(id) = argv.get(1) else {
                return Err(SpecError::new(code::CONFIG_INVALID, "用法：arcdeck auth rm <provider>"));
            };
            let mut store = AuthStore::load(&path)?;
            store.entries.remove(id);
            store.save(&path).map_err(io_err(&path))?;
            println!("已删除 {id}");
            Ok(0)
        }
        Some("import-opencode") => {
            let src = userconfig::opencode_auth_path();
            let from = AuthStore::load(&src)?;
            let mut store = AuthStore::load(&path)?;
            let mut n = 0;
            for (id, e) in &from.entries {
                if e.kind == "api" && !store.entries.contains_key(id) {
                    store.entries.insert(id.clone(), e.clone());
                    n += 1;
                    println!("  导入 {id}");
                }
            }
            store.save(&path).map_err(io_err(&path))?;
            println!("从 {} 导入 {n} 项 API 凭据（已有的不覆盖；OAuth 类不导入）", src.display());
            Ok(0)
        }
        Some(other) => Err(SpecError::new(code::CONFIG_INVALID, format!("auth 不认识 `{other}`（set / ls / rm / import-opencode）"))),
    }
}

fn cmd_runtime(argv: &[String]) -> CmdResult {
    let flags = Flags::parse(&argv[argv.len().min(1)..], &["--config", "--version", "--dir"], &[])?;
    let cfg = UserConfig::locate_and_load(flags.one("--config").map(Path::new)).ok().map(|(c, _)| c);
    let version = flags
        .one("--version")
        .map(str::to_string)
        .or_else(|| cfg.as_ref().map(|c| c.dsh_version()))
        .unwrap_or_else(|| userconfig::PINNED_DSH_VERSION.to_string());
    let dir = flags
        .one("--dir")
        .map(PathBuf::from)
        .or_else(|| cfg.as_ref().map(|c| c.runtime_dir()))
        .unwrap_or_else(|| userconfig::default_runtime_dir(&version));
    let layout = RuntimeLayout::new(&dir, &version);
    match argv.first().map(String::as_str) {
        Some("install") => {
            eprintln!("arcdeck: 安装 @deepseek-ai/dsh@{version} → {}", dir.display());
            layout.install().map_err(|m| SpecError::new(code::RUNTIME_ENTRY_MISSING, m))?;
            println!("DSH {version} 已就绪：{}", layout.entry().display());
            Ok(0)
        }
        Some("status") | None => match layout.check() {
            Ok(()) => {
                println!("DSH {version} 已就绪：{}", layout.entry().display());
                Ok(0)
            }
            Err(m) => {
                println!("{m}");
                Ok(1)
            }
        },
        Some(other) => Err(SpecError::new(code::CONFIG_INVALID, format!("runtime 不认识 `{other}`（install / status）"))),
    }
}

fn cmd_config(argv: &[String]) -> CmdResult {
    let path = match std::env::var_os("ARCDECK_CONFIG") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => userconfig::config_dir().join("arcdeck").join("arcdeck.json"),
    };
    match argv.first().map(String::as_str) {
        Some("init") => {
            if path.exists() && !argv.iter().any(|a| a == "--force") {
                println!("已存在：{}（覆盖用 --force）", path.display());
                return Ok(1);
            }
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p).map_err(io_err(p))?;
            }
            std::fs::write(&path, userconfig::starter_config()).map_err(io_err(&path))?;
            println!("已写入 {}", path.display());
            Ok(0)
        }
        Some("path") | None => {
            println!("{}", path.display());
            Ok(0)
        }
        Some(other) => Err(SpecError::new(code::CONFIG_INVALID, format!("config 不认识 `{other}`（init / path）"))),
    }
}

#[allow(dead_code)]
fn flush_stdout() {
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("90").unwrap(), 90);
        assert_eq!(parse_duration("30m").unwrap(), 1800);
        assert_eq!(parse_duration("2h").unwrap(), 7200);
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn ids_are_sanitized() {
        assert_eq!(sanitize_id("k-key repeat/2"), "k-key-repeat-2");
        assert_eq!(sanitize_id("中文"), "");
    }

    #[test]
    fn rerun_moves_the_old_record_aside() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("runs").join("t1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("result.json"), "{}").unwrap();
        let err = prepare_run_dir(&dir, "t1", false).unwrap_err();
        assert!(err.is(code::TASK_ALREADY_SEEN), "{err}");
        prepare_run_dir(&dir, "t1", true).unwrap();
        assert!(dir.exists() && !dir.join("result.json").exists());
        let kept = std::fs::read_dir(root.path().join("runs")).unwrap().count();
        assert_eq!(kept, 2, "旧记录必须保留");
    }
}
