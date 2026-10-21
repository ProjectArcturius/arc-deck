//! M3 的 live 验收：对**本机已安装的真实 DSH 运行时**组合 `--profile sdk` + Saiku
//! 生成的 patch，用 `--dump-config` 断言两件事：
//!
//! 1. DSH **接受**这份 patch（不报「这一行找不到」，退出码 0）；
//! 2. 组合之后的配置里限制**确实在**（按行核对取值，不只看退出码）。
//!
//! 四条纪律：
//!
//! - **不发 prompt、不消耗模型配额。** `--dump-config` 按设计「组合并校验配置但不启动、
//!   不发模型请求」（`apps/cli/src/dump-config.ts`）。
//! - **`DSH_HOME` 是临时目录。** 不碰 DSH Desktop 的真实 home（参 AGENTS.md P15）。
//! - **退出码 0 不是证据。** 实测（本轮）：patch 里写一个不存在的 row id 时，DSH 只在
//!   stderr 上打一句 `patch: entry "…" not found`，退出码照样是 0。所以还要断言 stderr
//!   干净，并在 dump 里按行核对 —— 见 `m3_live_unknown_row_id_is_reported_on_stderr`
//!   那条对照实验：它证明「stderr 干净」这个断言有牙齿，不是空过。
//! - **这里不证明「真实工具调用被拒」。** 那要一个脚本化的假模型端点，
//!   在 `m3_negative_live.rs` 里（同一目录）——它驱动的是真实内核 + 假 provider，
//!   断言的是工具结果与文件系统。本文件的边界写在 docs/M3_GUARD.md §6。
//!
//! 名字一律以 `m3_live_` 开头且 `#[ignore]`：它们要一台装了运行时的机器，不进日常离线套件。

use std::path::{Path, PathBuf};
use std::process::Command;

use arc_deck_core::authz::{ExecutionGuard, ExecutionPlan, Permission};
use arc_deck_core::config::{RouteSpec, RouteModel, WorkerConfig};
use arc_deck_core::task::TaskSpec;
use arc_deck_core::userconfig::{default_runtime_dir, PINNED_DSH_VERSION};
use arc_deck_dsh::runtime::{render_patch, PatchInput, RuntimeLayout};
use serde_json::{json, Value};

/// 本机运行时入口；没装就明确报「先装」而不是静默跳过。
fn entry() -> PathBuf {
    let layout = RuntimeLayout::new(default_runtime_dir(PINNED_DSH_VERSION), PINNED_DSH_VERSION);
    let entry = layout.entry();
    assert!(
        entry.is_file(),
        "本机没有钉住的 DSH 运行时（{PINNED_DSH_VERSION}）：找不到 {}。\
         先运行 `saiku runtime install` 再跑这些 live 用例",
        entry.display()
    );
    entry
}

fn node() -> String {
    std::env::var("SAIKU_TEST_NODE").unwrap_or_else(|_| "node".to_string())
}

/// 造一份真实的启动配置 + 任务书，走到 `ExecutionGuard`。
///
/// 与 `worker::dsh_runtime_command` 同一条链：`EffectivePolicy::execution_guard(startup)`，
/// 其中 `startup` 是从启动配置的 `runtime.sandbox` 反解出来的级别。这里不重写判定规则，
/// 只是把同一段调用搬进测试 —— 复制判定规则才是会漂移的那种做法。
fn guard_for(
    workspace: &Path,
    profile: &str,
    sandbox: &str,
    agent: &str,
    read: &[&str],
    write: &[&str],
    network: bool,
) -> ExecutionGuard {
    let ws = workspace.display().to_string();
    let config = json!({
        "schema_version": 1,
        "runtime": {
            "kind": "dsh-sdk",
            "entry": entry().display().to_string(),
            "version": PINNED_DSH_VERSION,
            "profile": "sdk",
            "sandbox": sandbox,
        },
        "state_root": workspace.join("state").display().to_string(),
        "authorization": {
            "workspace": ws,
            "profile": profile,
            "read_paths": read,
            "write_paths": write,
            "network": network,
            "allow_commit": false,
            "allow_push": false,
            "allowed_agents": ["echo", "build", "plan"],
        },
        "agents": { "dir": "agents", "prompts_dir": "prompts" },
        // dsh-sdk 的启动配置必须显式给出路由 —— 没有默认路由（config.rs 的校验）。
        // 这里只是为了让配置解析通过：live 用例不发任何请求。
        "model": { "provider": "opencode-go", "model": "m3-live-placeholder" },
    })
    .to_string();
    let task = json!({
        "schema_version": 1,
        "task_id": "m3-live",
        "workspace": ws,
        "agent": agent,
        "instruction": "m3 live 验收：只组合配置，不发请求",
    })
    .to_string();
    let config = WorkerConfig::parse(&config, workspace).expect("启动配置应能解析");
    let task = TaskSpec::parse(&task, workspace).expect("任务书应能解析");
    let plan = ExecutionPlan::resolve_with_ceiling(&config, &task, None).expect("策略应能解析");
    plan.policy
        .execution_guard(Permission::from_sandbox(sandbox).expect("沙箱档应能反解"))
        .expect("执行层应能表达这份策略")
}

/// 一份占位路由：live 用例只组合配置，不发请求，所以密钥环境变量只是个名字。
fn route() -> RouteSpec {
    RouteSpec {
        provider_id: "opencode-go".into(),
        adapter: "pi-ai".into(),
        api: Some("anthropic-messages".into()),
        base_url: Some("https://example.invalid".into()),
        headers: Default::default(),
        models: vec![RouteModel { id: "m3-live-placeholder".into(), name: None, context_window: None }],
        display_name: None,
        key_env: "SAIKU_KEY_M3_LIVE".into(),
        key_source: "env:SAIKU_KEY_M3_LIVE".into(),
    }
}

fn patch_for(guard: &ExecutionGuard) -> String {
    render_patch(&PatchInput {
        route: &route(),
        persona_suffix: "M3 live 验收用的 persona 占位",
        session_log_upload: false,
        session_search: false,
        guard,
    })
}

/// `--dump-config` 的结局。
struct Dump {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// 组合一份 patch：临时 DSH_HOME，`--profile sdk --patch <file> --dump-config`。
fn dump_config(patch: &str) -> (Dump, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("建临时目录");
    let patch_path = dir.path().join("dsh-patch.json");
    std::fs::write(&patch_path, patch).expect("写 patch");
    let home = dir.path().join("dsh-home");

    let out = Command::new(node())
        .arg(entry())
        .args(["--profile", "sdk", "--patch"])
        .arg(&patch_path)
        .arg("--dump-config")
        // 临时 home：`prepareProfile` 会往里写 profiles/sdk，绝不碰真实 home。
        .env("DSH_HOME", &home)
        .env("DSH_TELEMETRY_DISABLED", "1")
        .output()
        .expect("跑不起 node —— 检查 node 是否在 PATH 上（或用 SAIKU_TEST_NODE 指定）");
    (
        Dump {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        dir,
    )
}

/// 断言 DSH 真的接受了这份 patch。
///
/// `--dump-config` 对「row id 不存在」只警告不改退出码（实测），所以两件事都要断言：
/// 退出码 0 **且** stderr 里没有那一类警告。少了后半句，一个 id 写错的 patch 也能绿。
fn assert_accepted(dump: &Dump) {
    assert_eq!(dump.code, Some(0), "DSH 拒绝了这份 patch：\n{}", dump.stderr);
    for needle in ["not found", "patch: entry", "failed to parse", "failed to read"] {
        assert!(
            !dump.stderr.contains(needle),
            "DSH 在 stderr 上报了 `{needle}` —— patch 没有被完整接受：\n{}",
            dump.stderr
        );
    }
}

/// 从 dump 里取一行（`- id: <id>` 起到下一个条目 / 下一个 `# ==` 分组注释为止），
/// 连同它所属的分组标签。返回 `(分组标签, 这一行的正文)`。
///
/// 按文本取而不是解析 YAML：解析要引依赖（本轮不许），而 dump 的形状很稳 ——
/// 每个连续分组前有一行 `# == <来源>, patched by <改过它的层>`。
fn dump_row(dump: &str, id: &str) -> (String, String) {
    let lines: Vec<&str> = dump.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with(&format!("- id: {id}")))
        .unwrap_or_else(|| panic!("dump 里没有 `{id}` 这一行"));
    let mut label = String::new();
    for line in lines[..start].iter().rev() {
        if line.starts_with("# ==") {
            label = line.to_string();
            break;
        }
    }
    let mut body = Vec::new();
    for line in &lines[start..] {
        if !body.is_empty() && (line.starts_with("# ==") || line.trim_start().starts_with("- id:")) {
            break;
        }
        body.push(*line);
    }
    (label, body.join("\n"))
}

/// 一行被我们的 patch 改过 —— 分组标签要写出 patch 文件。
fn assert_patched_by_us(label: &str, patch_path: &str) {
    assert!(
        label.contains(patch_path),
        "这一行不是被本次 patch 改的：`{label}` 里没有 `{patch_path}`"
    );
}

#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_readonly_patch_is_accepted_and_pins_read_only() {
    let ws = tempfile::tempdir().unwrap();
    // `--permission readonly` 落在配置里的样子：档位 workspace-readonly + 沙箱 read-only。
    let guard = guard_for(ws.path(), "workspace-readonly", "read-only", "echo", &["."], &[], true);
    let patch = patch_for(&guard);
    let (dump, dir) = dump_config(&patch);
    assert_accepted(&dump);
    let patch_path = dir.path().join("dsh-patch.json").display().to_string();

    let (label, body) = dump_row(&dump.stdout, "sandbox-policy");
    assert_patched_by_us(&label, &patch_path);
    assert!(body.contains("mode: read-only"), "{body}");
    assert!(
        body.contains(&ws.path().display().to_string().replace('\\', "/"))
            || body.contains(&ws.path().display().to_string()),
        "workspaceRoot 应当是任务的 workspace：{body}"
    );

    let (label, body) = dump_row(&dump.stdout, "approval");
    assert_patched_by_us(&label, &patch_path);
    assert!(body.contains("policy: never"), "{body}");

    // shell = false → 每一行 shell 类工具都必须真的被摘掉。
    for id in ["tool-bash", "tool-pwsh", "tool-jobs", "tool-workflow", "ptc-runtime", "workflow-ptc"] {
        let (label, body) = dump_row(&dump.stdout, id);
        assert_patched_by_us(&label, &patch_path);
        assert!(body.contains("disabled: true"), "`{id}` 没有被摘掉：{body}");
    }

    // Plan 的协作式 plan-mode 一贯不挂。
    let (label, body) = dump_row(&dump.stdout, "plan-mode");
    assert_patched_by_us(&label, &patch_path);
    assert!(body.contains("disabled: true"), "{body}");

    // network = true 时不该动 web 那一行 —— 收紧要精确，不能顺手多摘。
    let (label, _) = dump_row(&dump.stdout, "tool-web");
    assert!(!label.contains(&patch_path), "`tool-web` 不该被这次 patch 改：{label}");
}

#[test]
#[ignore = "live：需要本机已钉住安装的 DSH 运行时"]
fn m3_live_workspace_patch_keeps_shell_and_pins_workspace_write() {
    let ws = tempfile::tempdir().unwrap();
    let guard = guard_for(ws.path(), "workspace-development", "workspace-write", "build", &["."], &["."], true);
    let patch = patch_for(&guard);
    let (dump, dir) = dump_config(&patch);
    assert_accepted(&dump);
    let patch_path = dir.path().join("dsh-patch.json").display().to_string();

    let (label, body) = dump_row(&dump.stdout, "sandbox-policy");
    assert_patched_by_us(&label, &patch_path);
    assert!(body.contains("mode: workspace-write"), "{body}");

    // 预设表只留一格，且与上面那一行同值 —— 否则内核在建表时就会抛
    // `composed sandbox and approval defaults match no preset`。
    let (label, body) = dump_row(&dump.stdout, "permission");
    assert_patched_by_us(&label, &patch_path);
    assert!(body.contains("workspace:"), "预设名应当是产品层的级别名：{body}");
    assert!(body.contains("sandbox: workspace-write"), "{body}");
    assert!(body.contains("approval: never"), "{body}");
    assert!(!body.contains("danger-full-access"), "预设表里不该还有更宽的档：{body}");

    // shell = true 时不该去摘工具行。
    for id in ["tool-bash", "tool-pwsh", "tool-jobs", "tool-workflow", "ptc-runtime", "workflow-ptc"] {
        let (label, _) = dump_row(&dump.stdout, id);
        assert!(!label.contains(&patch_path), "`{id}` 不该被这次 patch 改：{label}");
    }
}

#[test]
#[ignore = "live：需要本机已钉住安装的 DSH 运行时"]
fn m3_live_plan_mode_patch_is_tightened_to_read_only() {
    let ws = tempfile::tempdir().unwrap();
    // Plan 的收窄发生在 core：即使启动级别给到 full、档位允许写，策略里也不许写。
    let guard = guard_for(ws.path(), "workspace-development", "danger-full-access", "plan", &["."], &["."], true);
    assert_eq!(guard.sandbox, Permission::Readonly, "Plan 必须落到 read-only");
    assert!(!guard.shell_tools, "Plan 必须没有 shell");

    let patch = patch_for(&guard);
    let (dump, dir) = dump_config(&patch);
    assert_accepted(&dump);
    let patch_path = dir.path().join("dsh-patch.json").display().to_string();

    let (label, body) = dump_row(&dump.stdout, "sandbox-policy");
    assert_patched_by_us(&label, &patch_path);
    assert!(body.contains("mode: read-only"), "启动级别是 full，Plan 也必须被收紧：{body}");

    let (label, body) = dump_row(&dump.stdout, "permission");
    assert_patched_by_us(&label, &patch_path);
    assert!(body.contains("readonly:"), "级别名用产品层的写法：{body}");
    assert!(body.contains("sandbox: read-only"), "{body}");
}

#[test]
#[ignore = "live：需要本机已钉住安装的 DSH 运行时"]
fn m3_live_network_off_drops_only_the_web_tool() {
    let ws = tempfile::tempdir().unwrap();
    // network=false 且 shell 已关：可表达，且只该摘 web 那一行。
    let guard = guard_for(ws.path(), "workspace-readonly", "read-only", "echo", &["."], &[], false);
    let patch = patch_for(&guard);
    let (dump, dir) = dump_config(&patch);
    assert_accepted(&dump);
    let patch_path = dir.path().join("dsh-patch.json").display().to_string();

    let (label, body) = dump_row(&dump.stdout, "tool-web");
    assert_patched_by_us(&label, &patch_path);
    assert!(body.contains("disabled: true"), "{body}");
}

/// 对照实验：证明上面「stderr 干净」这条断言有牙齿。
///
/// 一个 id 写错的 patch，DSH 的退出码**仍然是 0** —— 只警告一句。所以
/// `assert_accepted` 里那句 stderr 断言不是装饰；这条用例就是它的阴性对照。
#[test]
#[ignore = "live：需要本机已钉住安装的 DSH 运行时"]
fn m3_live_unknown_row_id_is_reported_on_stderr() {
    let bad = serde_json::to_string_pretty(&json!([
        { "id": "no-such-row-in-any-bundle", "disabled": true },
    ]))
    .unwrap();
    let (dump, _dir) = dump_config(&bad);
    assert_eq!(dump.code, Some(0), "这条对照实验的前提就是退出码仍为 0");
    assert!(
        dump.stderr.contains("not found"),
        "DSH 应当对未知 row id 报警告；没有的话说明这条断言过时了：{}",
        dump.stderr
    );
}

/// 真起一次内核、走 `initialize` 握手。返回 `(有没有回答 initialize, stderr)`。
///
/// 抽出来是为了让「合法组合能起来」和「缺 `permission` 行会挂掉一个插件」共用同一条
/// 路径 —— 两份实现里只要有一份写错，对照实验就不再对照。
///
/// `stderr_stop` 是一个提前收工的标记：等到了它就不必把超时耗满。**阴性用例**靠它，
/// 否则「插件挂掉」这种结局只能等到超时才判定（实测那要 20 秒以上）。
fn boot_and_initialize(patch: &str, workspace: &Path, stderr_stop: Option<&str>) -> (bool, String) {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::sync::{mpsc, Arc, Mutex};

    let home = tempfile::tempdir().expect("建临时 DSH_HOME");
    let patch_path = home.path().join("dsh-patch.json");
    std::fs::write(&patch_path, patch).expect("写 patch");

    let mut child = Command::new(node())
        .arg(entry())
        .args(["--profile", "sdk", "--patch"])
        .arg(&patch_path)
        .env("DSH_HOME", home.path().join("dsh-home"))
        .env("DSH_TELEMETRY_DISABLED", "1")
        // cwd = workspace：DSH 的写入沙箱以会话 cwd 为根（§9.2a）。
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("拉不起 node —— 检查 node 是否在 PATH 上（或用 SAIKU_TEST_NODE 指定）");

    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let err_buf = Arc::new(Mutex::new(String::new()));
    let err_writer = Arc::clone(&err_buf);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Ok(mut buf) = err_writer.lock() {
                buf.push_str(&line);
                buf.push('\n');
            }
        }
    });

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "cwd": workspace.display().to_string(),
            "provider": "opencode-go",
            "model": "m3-live-placeholder",
        },
    });
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        let _ = writeln!(stdin, "{init}");
        let _ = stdin.flush();
    }

    // 45 秒：实测启动约 2 秒，留足余量即可；这里不该等 120 秒那么久。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    let mut answered = false;
    loop {
        if let Some(marker) = stderr_stop {
            if err_buf.lock().map(|b| b.contains(marker)).unwrap_or(false) {
                break;
            }
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(line) => {
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    // 只认**成功**的 initialize：插件挂掉时内核会用 `error` 回答，
                    // 那不算「组合挂起来了」。
                    if value["id"] == 1 && value["result"]["serverInfo"]["name"].is_string() {
                        answered = true;
                        break;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    (answered, err_buf.lock().map(|b| b.clone()).unwrap_or_default())
}

/// 组合同样得**真的挂得起来**，而且不能有插件激活失败。
///
/// `--dump-config` 只看组合后的行、**不挂插件**。实测（本轮）：一行写错时内核照样退出 0；
/// 而 `permission` 那一行缺失时，`PermissionPresetService` 抛
/// `composed sandbox and approval defaults match no preset`，插件**激活失败**，
/// 内核其余部分**照常起来并回答 initialize**。所以「挂起来了」与「挂得干净」是两件事，
/// 这里两件都断言：收到成功的 initialize，且 stderr 上没有 `did not activate`。
///
/// **不发 prompt、不发模型请求**：消耗的是本机几秒启动时间，不是配额。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_patch_boots_and_answers_initialize() {
    let ws = tempfile::tempdir().unwrap();
    let guard = guard_for(ws.path(), "workspace-readonly", "read-only", "echo", &["."], &[], true);
    let patch = patch_for(&guard);
    let (answered, stderr) = boot_and_initialize(&patch, ws.path(), None);
    assert!(answered, "内核没回答成功的 initialize —— 组合挂不起来。stderr：\n{stderr}");
    assert!(
        !stderr.contains("did not activate"),
        "有插件激活失败 —— 这份组合是残的，`--dump-config` 看不出来：\n{stderr}"
    );
    assert!(!stderr.contains("match no preset"), "{stderr}");
}

/// 对照实验：证明上一条的「stderr 干净」有牙齿 —— `permission` 那一行不是装饰。
///
/// 同样的 `sandbox-policy`（read-only）与 `approval`（never），**只把 `permission` 行去掉**：
/// 组合默认值反查不到任何预设，`permission` 插件激活失败（实测 stderr：
/// `dsh: warning: 1 entry did not activate` + `composed sandbox and approval defaults match no preset`）。
///
/// 这条实验纠正过一个想当然的说法（「漏了这行内核起不来」）：内核**照样起得来**，
/// 坏掉的是那个插件与会话日志里的授权钉点。文档与代码注释现在按实测写。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_missing_permission_row_breaks_the_permission_plugin() {
    let ws = tempfile::tempdir().unwrap();
    // 手工写这两行，**不带** permission 行。
    let patch = serde_json::to_string_pretty(&json!([
        {
            "id": "sandbox-policy",
            "config": { "mode": "read-only", "workspaceRoot": ws.path().display().to_string() },
        },
        { "id": "approval", "config": { "policy": "never" } },
    ]))
    .unwrap();

    let (_answered, stderr) = boot_and_initialize(&patch, ws.path(), Some("match no preset"));
    assert!(
        stderr.contains("match no preset"),
        "缺 permission 行时该看到预设反查失败的抛错 —— 没有的话，上面那条断言就过时了：\n{stderr}"
    );
    assert!(
        stderr.contains("did not activate"),
        "插件激活失败时内核会打一句 `did not activate` —— 那是上面那条断言真正在查的东西：\n{stderr}"
    );
}

#[test]
#[ignore = "live：需要本机已钉住安装的 DSH 运行时"]
fn m3_live_generated_patch_is_a_plain_json_row_list() {    let ws = tempfile::tempdir().unwrap();
    let guard = guard_for(ws.path(), "workspace-development", "workspace-write", "echo", &["."], &["."], true);
    let patch = patch_for(&guard);
    let rows: Value = serde_json::from_str(&patch).expect("patch 应当是合法 JSON");
    let rows = rows.as_array().expect("patch 应当是数组");
    assert!(rows.iter().all(|r| r["id"].is_string()), "每一行都必须有 id");
    // 密钥不进 patch（§9.2b）：patch 里只写承载密钥的**环境变量名**，不写值。
    // 这里挨个查 patch 全文，任何形状的 key/apiKey/keySource 字段都算违规。
    for forbidden in ["\"apiKey\"", "\"key\"", "\"keySource\"", "\"token\"", "\"secret\""] {
        assert!(!patch.contains(forbidden), "patch 里出现了 `{forbidden}`：密钥不该落盘");
    }
    assert!(patch.contains("apiKeyEnv"), "路由该写的是 apiKeyEnv 这个变量名");
}
