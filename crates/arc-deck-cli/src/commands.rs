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
use std::time::Duration;

use arc_deck_core::config::{
    AgentSources, Authorization, Liveness, ModelRoute, RuntimeSpec, Timeouts, WorkerConfig,
};
use arc_deck_core::error::{code, SpecError};
use arc_deck_core::provider::{self, AuthStore};
use arc_deck_core::result::TaskResult;
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
        &["--config", "--workspace", "--id", "--accept", "--preamble", "--worktree", "--git-base",
          "--model", "--timeout", "--agent", "--state-root"],
        &["--detach", "--rerun"],
    )?;
    let [task_arg] = flags.positional.as_slice() else {
        return Err(SpecError::new(
            code::CONFIG_INVALID,
            "用法：arcdeck run <任务书.md|任务书.json|-> [--detach] [--id ID] [--worktree 分支] [--accept 命令]…",
        ));
    };
    let (cfg, cfg_path) = UserConfig::locate_and_load(flags.one("--config").map(Path::new))?;
    let root = flags.one("--state-root").map(PathBuf::from).unwrap_or_else(|| cfg.state_root());

    // 任务书：Markdown 原样作为指令；JSON 走 M1 的任务书 v1。
    let (instruction, json_task, default_id) = load_task_source(task_arg)?;
    let task_id = match flags.one("--id") {
        Some(id) => sanitize_id(id),
        None => json_task.as_ref().map(|t| t.task_id.clone()).unwrap_or(default_id),
    };
    if task_id.is_empty() {
        return Err(SpecError::new(code::TASK_INVALID, "取不到任务 ID；用 --id 指定"));
    }

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
    }

    // 状态目录：同一个 ID 不重跑，除非 --rerun（旧目录改名留档，不删）。
    let dir = run_dir(&root, &task_id);
    prepare_run_dir(&dir, &task_id, flags.has("--rerun"))?;

    // 路由与密钥位置。
    let model = flags.one("--model").unwrap_or(&cfg.model).to_string();
    let route = provider::resolve(&model, &cfg.provider, &task_id)?;
    let route_spec = route.to_spec()?;
    let layout = RuntimeLayout::new(cfg.runtime_dir(), cfg.dsh_version());
    layout.check().map_err(|m| SpecError::new(code::RUNTIME_ENTRY_MISSING, m))?;

    // 内置 Agent 定义写进状态目录。
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
    let preamble = flags.one("--preamble").map(|p| absolute(Path::new(p))).or(cfg.worker.preamble.clone());

    let mut env = BTreeMap::new();
    for (k, v) in &cfg.env {
        env.insert(
            k.clone(),
            v.replace("{task_id}", &task_id)
                .replace("{workspace}", &workspace.display().to_string())
                .replace("{run_dir}", &dir.display().to_string()),
        );
    }

    let profile = if cfg.worker.sandbox == "read-only" { "workspace-readonly" } else { "workspace-development" };
    let config = WorkerConfig {
        schema_version: SCHEMA_VERSION,
        runtime: RuntimeSpec::DshSdk {
            node: cfg.runtime.node.clone().unwrap_or_else(|| PathBuf::from("node")),
            entry: layout.entry(),
            version: cfg.dsh_version(),
            profile: "sdk".into(),
            route: Some(route_spec),
            sandbox: cfg.worker.sandbox.clone(),
            session_log_upload: cfg.runtime.session_log_upload,
            preamble,
        },
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
        model: Some(ModelRoute {
            provider: route.dsh_provider.clone(),
            model: route.model.clone(),
            reasoning_effort: cfg.reasoning_effort.clone(),
        }),
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
    eprintln!("arcdeck: 配置 {} · 模型 {model} · workspace {}", cfg_path.display(), workspace.display());

    let cancel = dir.join("cancel.json");
    if flags.has("--detach") {
        return spawn_detached(&dir, &config_path, &task_path, &cancel, &task_id);
    }
    let args = WorkerArgs {
        config: config_path,
        task: TaskSource::File(task_path),
        format: "jsonl".into(),
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
fn spawn_detached(dir: &Path, config: &Path, task: &Path, cancel: &Path, task_id: &str) -> CmdResult {
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
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // 先试脱离作业对象（ssh / 终端关掉时连带杀作业里的进程）；作业不允许脱离时退回普通分离。
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        if let Ok(child) = cmd.spawn() {
            println!("{task_id}  started  pid {}  · arcdeck status {task_id}", child.id());
            return Ok(0);
        }
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    let child = cmd
        .spawn()
        .map_err(|e| SpecError::new(code::CONFIG_INVALID, format!("分离启动失败：{e}")))?;
    println!("{task_id}  started  pid {}  · arcdeck status {task_id}", child.id());
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
fn render_report(r: &TaskResult, st: Option<&StatusFile>, dir: &Path, now: u64) -> String {
    let mut o = String::new();
    let elapsed = st.map(|s| status::human(s.heartbeat_at.saturating_sub(s.started_at) / 1000)).unwrap_or_default();
    let attempts = st.map(|s| s.attempt).unwrap_or(1);
    o.push_str(&format!("# {}  {}  · {elapsed} · 运行 {attempts} 次\n", r.task_id, r.status.as_str()));
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
    if !cancel.exists() {
        let body = serde_json::json!({ "schema_version": SCHEMA_VERSION, "task_id": id, "action": "cancel" });
        std::fs::write(&cancel, body.to_string()).map_err(io_err(&cancel))?;
    }
    // 有界等待收尾。
    for _ in 0..30 {
        if let Some(st) = status::read(&dir.join(status::STATUS_FILE)) {
            let h = status::health(&st, status::now_ms());
            if h == "done" || h == "dead" {
                println!("{}", status::one_line(&st, h, status::now_ms()));
                return Ok(status::exit_code(&st, h));
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    println!("{id}  已请求取消，30 秒内还没收尾；用 arcdeck status {id} 继续看");
    Ok(10)
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
