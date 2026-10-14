//! M3 负例验收：用**脚本化的假模型端点**（`arc_deck_dsh::fake_provider`）驱动**真实的
//! DSH 运行时**，把「模型提出越权工具调用」变成可编程的输入，然后断言执行层真的拦住了。
//!
//! 为什么必须这样做：`m3_guard_live.rs` 那批用例只走 `--dump-config`，证明的是
//! 「配置行写对了」。它证明不了「工具真的被拒」——`docs/M3_GUARD.md` §6 把那条明确记成
//! 「推测」。真实内核 + 假模型是唯一能把那条推测变成实测的组合：内核、沙箱、工具注册表、
//! 审批服务全是真的，只有「模型说什么」是脚本。
//!
//! 四条纪律（与 `m3_guard_live.rs` 同源）：
//!
//! - **不发真实模型请求、不消耗配额。** provider 的 baseURL 指向本机 127.0.0.1 上的假端点。
//! - **`DSH_HOME`、workspace、状态目录都是临时目录**（worker 自己建 `<run_dir>/dsh-home`）。
//! - **「被拒」必须配正例对照**（`m3_live_full_pwsh_runs_inside`、`m3_live_write_inside_workspace_succeeds`）：
//!   没有它们，「被拒」可能只是工具本来就坏了（AGENTS.md P9 的教训）。
//! - **两侧分开断言**：模型**看不到**某个工具（可见性收窄，读假端点的请求日志）与
//!   调用**被执行层拒**（沙箱/审批，读工具结果）是两件不同的事。
//!
//! 全部以 `m3_live_` 开头且 `#[ignore]`：它们要一台装了钉住运行时的机器。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use arc_deck_core::userconfig::PINNED_DSH_VERSION;
use arc_deck_dsh::fake_provider::{self, Provider, Script};
use serde_json::{json, Value};

fn arcdeck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_arcdeck")
}

/// 假端点在请求里看到的模型名。与配置里的 `models` 表一致 —— 不一致时 DSH 会在
/// 发请求前就拒（`UNKNOWN_MODEL`），症状离原因很远。
const MODEL: &str = "fake-model";

fn runtime_dir() -> PathBuf {
    arc_deck_core::userconfig::default_runtime_dir(PINNED_DSH_VERSION)
}

/// 钉住的运行时入口。没装就明确报「先装」，不静默跳过 —— live 用例的失败要可解释。
fn runtime_entry() -> PathBuf {
    let entry = runtime_dir().join("node_modules/@deepseek-ai/dsh/lib/bin.js");
    assert!(
        entry.is_file(),
        "本机没有钉住的 DSH 运行时（{PINNED_DSH_VERSION}）：找不到 {}。\
         先运行 `arcdeck runtime install` 再跑这些 live 用例",
        entry.display()
    );
    entry
}

/// 用户主目录：**不在任何写根下**的位置（写根是 workspace + `/tmp` + `os.tmpdir()`）。
///
/// 负例必须挑这样的地方。踩过一次：第一版拿 `%TEMP%` 下的路径当「workspace 外面」，
/// 而 `writableRoots` 里本来就有 `os.tmpdir()`，于是那次「越权写成功」其实完全合法，
/// 差点被写成一条「沙箱有洞」的假结论（见 `m3_live_write_inside_temp_root_is_allowed`）。
fn home_dir() -> PathBuf {
    arc_deck_core::userconfig::home_dir()
}

/// 平台上的 shell 工具名。§2.3 那张表里 shell 是两行（`tool-bash` / `tool-pwsh`），
/// Windows 上只有 `pwsh` 真的挂上 —— 断言不能写死 `bash`。
fn shell_tool() -> &'static str {
    if cfg!(windows) { "pwsh" } else { "bash" }
}

fn shell_args(command: &str) -> Value {
    json!({ "description": "M3 负例：越权命令", "command": command })
}

/// 一次真实运行的结局。
struct Outcome {
    code: i32,
    stderr: String,
    run_dir: PathBuf,
    /// 假端点收到的请求（每次一行 JSON）。
    requests: Vec<Value>,
    /// 本次运行用的授权级别。终态断言要看它 —— `readonly` 档不能带验收命令
    /// （`ACCEPTANCE_NOT_ALLOWED` 会在建立状态之前就拒掉整次运行）。
    permission: String,
    #[allow(dead_code)]
    stdout: String,
}

impl Outcome {
    fn events(&self) -> Vec<Value> {
        let path = self.run_dir.join("events.jsonl");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()))
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).expect("events.jsonl 每行都该是 JSON"))
            .collect()
    }

    fn event(&self, kind: &str) -> Vec<Value> {
        self.events().into_iter().filter(|e| e["type"] == kind).map(|e| e["data"].clone()).collect()
    }

    /// `result.json` —— 终态的唯一权威来源。
    fn result(&self) -> Value {
        let path = self.run_dir.join("result.json");
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("读不到 {}：{e}\nstderr:\n{}", path.display(), self.stderr)
        }))
        .expect("result.json 该是 JSON")
    }

    /// 工具结果（`tool_result` 事件）的 `data`。
    fn tool_results(&self) -> Vec<Value> {
        self.event("tool_result")
    }

    /// 第一次请求里 DSH 交给模型的工具名清单 —— 可见性收窄的唯一证据。
    fn first_tools(&self) -> Vec<String> {
        let body = &self.requests.first().expect("假端点至少该收到一次请求")["body"];
        fake_provider::tool_names(body)
    }

    /// 第 `index` 次请求的工具清单（子会话用它：第 1 次请求属于子 Agent）。
    fn tools_at(&self, index: usize) -> Vec<String> {
        let request = self.requests.get(index).unwrap_or_else(|| {
            panic!("假端点只收到 {} 次请求，取不到第 {index} 次", self.requests.len())
        });
        fake_provider::tool_names(&request["body"])
    }

    /// 请求里带回来的工具结果原文（逐条）。
    fn result_texts(&self) -> Vec<String> {
        let mut out = Vec::new();
        for request in &self.requests {
            for message in fake_provider::messages(&request["body"]) {
                let Some(blocks) = message.get("content").and_then(Value::as_array) else { continue };
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    out.push(match &block["content"] {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    });
                }
            }
        }
        out
    }

    /// 所有请求的转录合起来的全文，供「某个短语有没有回到模型」这类断言。
    fn transcripts(&self) -> String {
        self.requests.iter().map(|r| fake_provider::transcript(&r["body"])).collect::<Vec<_>>().join("\n")
    }

    /// 断言某次工具调用**真的没生效**：工具结果是 error，且正文里有给定片段。
    ///
    /// 「工具结果是拒绝」与「文件没被改」是两件事，两个都要断言 —— 只看文件的话，
    /// 一个「工具坏了所以什么都没做」的实现也会绿。
    fn assert_denied(&self, tool: &str, needle: &str) -> Value {
        let results = self.tool_results();
        let hit = results
            .iter()
            .find(|r| r["tool"] == tool)
            .unwrap_or_else(|| panic!("没有 `{tool}` 的工具结果：{results:#?}"));
        assert_eq!(hit["status"], "error", "`{tool}` 没有被拒：{hit}");
        let summary = hit["summary"].as_str().unwrap_or_default();
        assert!(
            summary.contains(needle),
            "拒因里没有 `{needle}`：{summary}\n（全部结果：{results:#?}）"
        );
        hit.clone()
    }
}

/// 一个用例：临时目录 + workspace + 状态目录 + 一次运行。
struct Case {
    dir: tempfile::TempDir,
    workspace: PathBuf,
    state: PathBuf,
    id: String,
}

impl Case {
    fn new(id: &str) -> Self {
        let dir = tempfile::tempdir().expect("建临时目录");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let state = dir.path().join("state");
        Self { dir, workspace, state, id: id.to_string() }
    }

    fn workspace(&self) -> &Path {
        &self.workspace
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// 脚本 JSON 在启动前先自检：脚本写错时的症状是「DSH 说模型失败」，而原因在脚本里 ——
    /// 分开报才查得动。
    fn provider(&self, script: Value) -> Provider {
        let script = Script::from_value(&script).unwrap_or_else(|e| panic!("假端点脚本不合法：{e}"));
        Provider::start(script, self.path("requests.jsonl")).expect("起假端点")
    }

    /// 跑一次 `arcdeck run`：真实内核 + 假模型端点。
    ///
    /// `readonly` 档**不带验收命令**：`ACCEPTANCE_NOT_ALLOWED` 会在建立任何状态之前
    /// 拒掉整次运行（「验收身份不能绕过授权」），于是走不到工具被拒那一步。
    fn run(&self, script: Value, permission: &str) -> Outcome {
        let provider = self.provider(script);
        let accept = permission != "readonly";
        self.run_with_provider(&provider, permission, &[], accept)
    }

    /// 跑一次 `arcdeck worker run`：配置由测试**手写**。
    ///
    /// 只有 `network = false` 需要这条路：`arcdeck run` 物化启动配置时 `network` 恒为 true
    /// （产品层还没有关网络的开关），而 `worker run` 收的就是启动配置本身 ——
    /// 于是照样走真实内核，只是配置不是物化的那一份。
    fn run_worker(&self, provider: &Provider, config: Value) -> Outcome {
        let config_path = self.path("worker.json");
        std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        let task_path = self.write_task(json!({}), Value::Null);
        let output = Command::new(arcdeck_bin())
            .arg("worker")
            .arg("run")
            .arg("--config")
            .arg(&config_path)
            .arg("--task")
            .arg(&task_path)
            .arg("--format")
            .arg("jsonl")
            .arg("--result")
            .arg(self.path("result.json"))
            .env("ARCDECK_TEST_FAKE_KEY", "fake-key-not-a-secret")
            .env("DSH_TELEMETRY_DISABLED", "1")
            .output()
            .expect("跑不起 arcdeck worker");
        Outcome {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            run_dir: self.state.join("runs").join(&self.id),
            requests: provider.requests(),
            permission: "readonly".to_string(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        }
    }

    /// 写一份 JSON 任务书，返回路径。`extra` 用于覆盖字段（例如 `constraints`）。
    fn write_task(&self, extra: Value, constraints: Value) -> PathBuf {
        let mut task = json!({
            "schema_version": 1,
            "task_id": self.id,
            "workspace": self.workspace.display().to_string(),
            "agent": "echo",
            "instruction": "M3 负例：按脚本提出工具调用",
            "timeout_seconds": 120,
        });
        if let (Some(obj), Value::Object(extra)) = (task.as_object_mut(), extra) {
            for (k, v) in extra {
                obj.insert(k, v);
            }
        }
        if !constraints.is_null() {
            task["constraints"] = constraints;
        }
        let path = self.path("task.json");
        std::fs::write(&path, serde_json::to_string_pretty(&task).unwrap()).unwrap();
        path
    }

    fn run_with_provider(
        &self,
        provider: &Provider,
        permission: &str,
        extra_args: &[&str],
        accept: bool,
    ) -> Outcome {
        let task_path = self.write_task(json!({}), Value::Null);

        let config_path = self.path("arcdeck.json");
        std::fs::write(&config_path, self.config_json(&provider.url())).unwrap();

        let mut command = Command::new(arcdeck_bin());
        command
            .arg("run")
            .arg(&task_path)
            .arg("--id")
            .arg(&self.id)
            .arg("--config")
            .arg(&config_path)
            .arg("--workspace")
            .arg(&self.workspace)
            .arg("--state-root")
            .arg(&self.state)
            .arg("--permission")
            .arg(permission)
            .arg("--format")
            .arg("jsonl")
            // 假端点的路由只要求「有一个非空的密钥」；给一个显式的假值，
            // 免得测试撞上本机 `auth.json` 的真实内容（live 用例不该依赖它）。
            .env("ARCDECK_TEST_FAKE_KEY", "fake-key-not-a-secret")
            .env("DSH_TELEMETRY_DISABLED", "1");
        if accept {
            // 验收命令模型从不执行（脚本里没有那条调用），于是终态必然是 blocked +
            // ACCEPTANCE_UNFINISHED —— 这本身就是「结果如实」的一条断言。
            command.arg("--accept").arg("cargo --version");
        }
        for arg in extra_args {
            command.arg(arg);
        }
        let output = command.output().expect("跑不起 arcdeck");
        Outcome {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            run_dir: self.state.join("runs").join(&self.id),
            requests: provider.requests(),
            permission: permission.to_string(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        }
    }

    /// 物化的启动配置：用户配置 + 指向假端点的 provider 块。
    fn config_json(&self, base_url: &str) -> String {
        serde_json::to_string_pretty(&json!({
            "model": format!("fake-endpoint/{MODEL}"),
            "provider": {
                "fake-endpoint": {
                    "name": "M3 负例的假端点",
                    "api": "anthropic-messages",
                    "options": {
                        "baseURL": base_url,
                        "apiKey": "{env:ARCDECK_TEST_FAKE_KEY}",
                    },
                    "models": { MODEL: { "name": "Fake", "contextWindow": 100000 } },
                }
            },
            // 运行时目录由测试显式给出：不吃本机 `arcdeck.json` 里的设置。
            "runtime": {
                "dir": runtime_dir().display().to_string().replace('\\', "/"),
                "node": "node",
                "session_log_upload": false,
            },
            "state_root": self.state.display().to_string().replace('\\', "/"),
            "personalization": { "enabled": false },
            // 一次就够：续跑会让「第 N 次请求回什么」不再对应脚本条目。
            "worker": { "permission": "workspace", "timeout_seconds": 120, "max_attempts": 1 },
        }))
        .unwrap()
    }

    /// 手写一份 worker 启动配置（`network = false` 那条路用）。
    ///
    /// 字段名照 `core::config` 的结构逐字写（那一侧全是 `deny_unknown_fields`，
    /// 写错一个键就会在预检里以 `CONFIG_INVALID` 退 64 —— 症状是「假端点一次请求都没收到」）。
    fn worker_config(&self, base_url: &str, sandbox: &str, profile: &str, network: bool) -> Value {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf();
        // 档位与写范围必须自洽：`read-only` 下 `write_paths` 非空会被预检拒（实测报
        // 「档位 `workspace-readonly` 不允许写入，但 authorization.write_paths 非空」）。
        let write_paths = if sandbox == "read-only" { json!([]) } else { json!(["."]) };
        json!({
            "schema_version": 1,
            "runtime": {
                "kind": "dsh-sdk",
                "node": "node",
                "entry": runtime_entry().display().to_string().replace('\\', "/"),
                "version": PINNED_DSH_VERSION,
                "profile": "sdk",
                "sandbox": sandbox,
                "route": {
                    "provider_id": "fake-endpoint",
                    "adapter": "pi-ai",
                    "api": "anthropic-messages",
                    "base_url": base_url,
                    "models": [{ "id": MODEL, "name": "Fake", "context_window": 100000 }],
                    "key_env": "ARCDECK_TEST_FAKE_KEY",
                    "key_source": "env:ARCDECK_TEST_FAKE_KEY",
                },
            },
            "state_root": self.state.display().to_string().replace('\\', "/"),
            "authorization": {
                "workspace": self.workspace.display().to_string().replace('\\', "/"),
                "profile": profile,
                "read_paths": ["."],
                "write_paths": write_paths,
                "network": network,
                "allow_commit": false,
                "allow_push": false,
                "allowed_agents": ["echo", "build", "plan"],
            },
            "agents": {
                "dir": root.join("agents").display().to_string().replace('\\', "/"),
                "prompts_dir": root.join("prompts").display().to_string().replace('\\', "/"),
            },
            "model": { "provider": "fake-endpoint", "model": MODEL },
            "timeouts": { "initialize_seconds": 60, "prompt_seconds": 60, "wait_seconds": 60,
                          "default_task_seconds": 120 },
            "liveness": { "heartbeat_seconds": 15, "model_idle_seconds": 120,
                          "tool_stall_seconds": 600, "max_attempts": 1 },
        })
    }
}

// ── 脚本构造的小工具 ────────────────────────────────────────────────────

fn call(tool: &str, arguments: Value) -> Value {
    json!({ "tool": tool, "arguments": arguments })
}

fn text(body: &str) -> Value {
    json!({ "text": body })
}

/// 「提一个工具调用，然后收尾」—— 绝大多数负例都是这个形状。
///
/// 第二条是必需的：脚本用完又没 `default` 时假端点会回 500（刻意不重复最后一条），
/// 而 DSH 会把那当成模型失败，于是「工具被拒」和「模型报错」混在一起。
fn call_then_close(tool: &str, arguments: Value) -> Value {
    json!({ "responses": [
        { "blocks": [call(tool, arguments)] },
        { "blocks": [text("收尾：工具结果已收到。")] },
    ]})
}

/// 每条负例的终态断言。
///
/// **`readonly` 档不能带验收命令**：`ACCEPTANCE_NOT_ALLOWED` 会在建立任何状态之前拒掉
/// 整次运行（「验收身份不能绕过授权」），于是根本走不到工具被拒那一步。所以：
///
/// - `workspace` / `full`：带 `--accept`，模型从不执行 → `blocked` + `ACCEPTANCE_UNFINISHED`。
///   这是「模型没跑验收不会被写成 completed」的一条断言。
/// - `readonly`：不带验收 → `completed`，且 `unverified` 里明说「没有声明验收项」。
///
/// 两种形状都要求结果如实；`readonly` 那种**不**断言终态是 blocked 是刻意的：
/// 它的越权效果由工具结果与文件系统两条断言负责。
fn assert_honest_terminal(outcome: &Outcome) {
    let result = outcome.result();
    match outcome.permission.as_str() {
        "readonly" => {
            assert_eq!(result["status"], "completed", "{result}\nstderr:\n{}", outcome.stderr);
            let unverified = result["unverified"].as_array().cloned().unwrap_or_default();
            assert!(
                unverified.iter().any(|u| u.as_str().unwrap_or("").contains("没有声明验收项")),
                "没有验收项这件事要如实报出来：{unverified:?}"
            );
            assert_eq!(outcome.code, 0, "readonly 档的 completed 退出码是 0。stderr:\n{}", outcome.stderr);
        }
        _ => {
            assert_eq!(result["status"], "blocked", "{result}\nstderr:\n{}", outcome.stderr);
            assert_eq!(result["verification"][0]["outcome"], "not_run", "{result}");
            let blockers = result["blockers"].as_array().cloned().unwrap_or_default();
            assert!(
                blockers.iter().any(|b| b["code"] == "ACCEPTANCE_UNFINISHED"),
                "「验收没执行」要如实报进 blocker：{blockers:?}"
            );
            // `blocked` 不是失败：退出码是 2（PRODUCT_PLAN §6.5.3）。
            assert_eq!(outcome.code, 2, "blocked 的退出码应当是 2。stderr:\n{}", outcome.stderr);
        }
    }
}

// ── 1 · read-only 的写被拒 ──────────────────────────────────────────────

/// §7 第 1 条：`readonly` 会话里 `write` 目标 `<workspace>/x.txt`。
///
/// 期望：`FS_SANDBOX_DENIED`，正文含 `[sandbox: file access denied under read-only mode]`；
/// 文件不存在。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_readonly_write_inside_is_denied_and_file_absent() {
    let case = Case::new("ro-write");
    let outcome = case.run(
        call_then_close("write", json!({ "file_path": "x.txt", "content": "should not exist\n" })),
        "readonly",
    );

    outcome.assert_denied("write", "[sandbox: file access denied under read-only mode]");
    assert!(
        !case.workspace().join("x.txt").exists(),
        "被拒的写入竟然落盘了：{}",
        case.workspace().join("x.txt").display()
    );
    // 拒因要**回到模型**：模型的第二次请求里带着那条工具结果。
    // 只断言 events.jsonl 的话，一个「拒了但没告诉模型」的实现也会绿。
    assert!(
        outcome.transcripts().contains("file access denied under read-only mode"),
        "拒因没有回到模型：{:#?}",
        outcome.requests
    );
    assert_honest_terminal(&outcome);
}

// ── 2 · read-only 的 edit 被拒，且既有文件一字不变 ──────────────────────

/// §7 第 2 条：`readonly` 会话里 `edit` 一个已存在的文件。
///
/// 期望：同上，且文件内容一字不变。
///
/// 脚本里**先 `read` 再 `edit`**：DSH 的 `fs-observation-policy` 会先以
/// `file has not been read` 拒掉一个没读过的编辑 —— 那与沙箱无关，
/// 拿到那个形状会把这条用例变成在测观察策略。读一次之后，拒因才来自沙箱。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_readonly_edit_leaves_the_file_untouched() {
    let case = Case::new("ro-edit");
    let target = case.workspace().join("existing.txt");
    std::fs::write(&target, "原始内容\n").unwrap();
    let target_arg = target.display().to_string();

    let outcome = case.run(
        json!({ "responses": [
            { "blocks": [call("read", json!({ "file_path": target_arg }))] },
            { "blocks": [call("edit", json!({
                "file_path": target_arg, "old_string": "原始内容", "new_string": "改过了" }))] },
            { "blocks": [text("收尾")] },
        ]}),
        "readonly",
    );

    outcome.assert_denied("edit", "[sandbox: file access denied under read-only mode]");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "原始内容\n",
        "被拒的编辑竟然改动了文件"
    );
    assert_honest_transcripts_mention(outcome, "file access denied under read-only mode");
}

fn assert_honest_transcripts_mention(outcome: Outcome, needle: &str) {
    assert!(
        outcome.transcripts().contains(needle),
        "拒因没有回到模型：{:#?}",
        outcome.requests
    );
    assert_honest_terminal(&outcome);
}

// ── 3 · shell=false 时工具**不存在**（不是「执行后被拒」） ───────────────

/// §7 第 3 条：`readonly` + `shell=false`（Plan 等效）调 shell 工具。
///
/// 期望：工具**不存在**（`UNKNOWN_TOOL`），而不是「执行后被拒」—— 这是摘行的判据。
/// 这条与第 1、2 条要分开记：前者证明的是**可见性/工具表**，后者才是沙箱。
///
/// 同时跑一次 `write`（相对路径 `plan.txt`，在 workspace 里）：即使模型编出 shell 调用，
/// 它也只能拿到 `UNKNOWN_TOOL`，而写仍然被沙箱拒 —— 两条路各自都在。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_shell_tools_are_absent_not_just_denied() {
    let case = Case::new("plan-shell");
    let outcome = case.run(
        json!({ "responses": [
            { "blocks": [call(shell_tool(), shell_args("Write-Output should-not-run"))] },
            { "blocks": [call("write", json!({ "file_path": "plan.txt", "content": "nope" }))] },
            { "blocks": [text("收尾")] },
        ]}),
        "readonly",
    );

    // 可见性：第一次请求的工具清单里根本没有 shell 工具。
    let tools = outcome.first_tools();
    assert!(
        !tools.iter().any(|t| t == shell_tool()),
        "`shell = false` 时 `{}` 不该出现在交给模型的工具清单里：{tools:?}",
        shell_tool()
    );
    // 执行：编出来的那个调用是 UNKNOWN_TOOL。
    outcome.assert_denied(shell_tool(), "unknown tool");
    // 写还是被沙箱拒，文件不存在。
    outcome.assert_denied("write", "[sandbox: file access denied under read-only mode]");
    assert!(!case.workspace().join("plan.txt").exists());
    assert_honest_terminal(&outcome);
}

// ── 4 · workspace-write 的越界写被拒 ────────────────────────────────────

/// §7 第 4 条：`workspace-write` 会话里 `write` 到 workspace **之外**（不在任何写根下）。
///
/// 期望：`FS_SANDBOX_DENIED`（`file access denied under workspace-write mode`），文件不存在。
///
/// 目标刻意选**用户主目录**：第一次实现拿 `%TEMP%` 下的路径当「外面」，
/// 而写根集合里有 `os.tmpdir()` —— 那次「越权成功」完全合法，不是沙箱的洞（见下一条正例）。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_workspace_write_outside_is_denied() {
    let outside = home_dir().join("arcdeck-m3b-outside-write.txt");
    let _ = std::fs::remove_file(&outside);
    assert!(!outside.exists(), "前置：{} 不该已经存在", outside.display());

    let case = Case::new("ws-outside");
    let outcome = case.run(
        call_then_close("write", json!({ "file_path": outside.display().to_string(), "content": "nope" })),
        "workspace",
    );

    outcome.assert_denied("write", "file access denied under workspace-write mode");
    assert!(!outside.exists(), "越界写入竟然落盘了：{}", outside.display());
    assert_honest_transcripts_mention(outcome, "file access denied under workspace-write mode");
}

/// §7 第 4 条的**正例对照**：同一个 `workspace-write` 档，写 workspace 内的文件 —— 必须成功。
///
/// 没有这一条，「被拒」可能只是写工具本来就坏了（AGENTS.md P9）。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_workspace_write_inside_succeeds() {
    let case = Case::new("ws-inside");
    let outcome = case.run(
        call_then_close("write", json!({ "file_path": "inside.txt", "content": "written by the fake model\n" })),
        "workspace",
    );

    let result = outcome.tool_results();
    assert_eq!(result[0]["status"], "ok", "workspace 内的写该成功：{result:#?}");
    assert_eq!(
        std::fs::read_to_string(case.workspace().join("inside.txt")).unwrap(),
        "written by the fake model\n",
        "工具报了成功但文件不对"
    );
    assert_honest_terminal(&outcome);
}

/// 写根集合里**确实有** `os.tmpdir()`（§2.1 的 `roots.ts:52-54`）—— 这条正例把它钉住。
///
/// 它的价值是**防止一次误读**：第一版拿 `%TEMP%` 下的路径当「workspace 外面」，
/// 观察到写成功，差点写成「沙箱有洞」。真相是那个位置本来就在写根里。
/// 记下来，下次不会再花一轮。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_write_inside_temp_root_is_allowed() {
    let allowed = std::env::temp_dir().join("arcdeck-m3b-inside-temp-root.txt");
    let _ = std::fs::remove_file(&allowed);

    let case = Case::new("temp-root");
    let outcome = case.run(
        call_then_close("write", json!({ "file_path": allowed.display().to_string(), "content": "allowed\n" })),
        "workspace",
    );

    let results = outcome.tool_results();
    assert_eq!(
        results[0]["status"], "ok",
        "临时区在写根集合里，写它是对的 —— 若这条挂了，说明写根的定义变了：{results:#?}"
    );
    assert!(allowed.exists(), "工具报了成功但文件不在：{}", allowed.display());
    let _ = std::fs::remove_file(&allowed);
    assert_honest_terminal(&outcome);
}

// ── 5 · workspace-write 的越界命令 ──────────────────────────────────────

/// §7 第 5 条：`workspace-write` 会话里用 shell 工具往 workspace 之外写。
///
/// **本轮实测的结论不是「命令被拒」，而是「命令根本没跑起来」**：DSH 在 workspace 上
/// 套 Windows ACL（受限令牌 + capability SID + Low 完整性标签）时失败
/// —— `SetNamedSecurityInfoW failed (Win32 5): grantWrite(<workspace>)`。
/// 见 `docs/M3_GUARD.md` §3 的残留缺口与 §6 的验证分级。
///
/// 所以这条断言的是**两件都成立**的事实：越权效果没有发生，工具结果是 error；
/// 至于拒在策略还是拒在沙箱初始化，由 `docs/M3_GUARD.md` 如实记下来。
/// 形状不写成「必须含某个具体字样」是刻意的 —— 那会把一次环境相关的失败固化成契约。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_workspace_write_outside_command_does_not_take_effect() {
    // 目标放在用户主目录，不放临时区：`os.tmpdir()` 本来就在 workspace-write 的写根里
    // （见上面临时区那条正例），放那里测的不是「越界」。派工者复跑时就因此得到不同结论 ——
    // 工人环境的 TEMP 在 D 盘、复跑环境在 C 盘，沙箱的表现跟着变。
    let outside = home_dir().join("arcdeck-m3b-cmd-outside.txt");
    let _ = std::fs::remove_file(&outside);

    let case = Case::new("ws-cmd-outside");
    let command = if cfg!(windows) {
        format!("Set-Content -Path '{}' -Value 'via-pwsh'", outside.display())
    } else {
        format!("echo via-bash > '{}'", outside.display())
    };
    let outcome = case.run(call_then_close(shell_tool(), shell_args(&command)), "workspace");

    let results = outcome.tool_results();
    let hit = results
        .iter()
        .find(|r| r["tool"] == shell_tool())
        .unwrap_or_else(|| panic!("没有 `{}` 的工具结果：{results:#?}", shell_tool()));
    // 只断言「不是成功」：拒在沙箱初始化（error）还是命令被拦后以非零退出（failed）
    // 取决于本机令牌与 ACL，两种都意味着越权没生效 —— 本条注释上方已说不把具体形状固化成契约。
    assert_ne!(hit["status"], "ok", "越界命令不该成功：{hit}");
    assert!(
        !outside.exists(),
        "越界命令的副作用真的发生了：{} 存在",
        outside.display()
    );
    assert_honest_terminal(&outcome);
}

/// §7 第 5 条的**正例对照**：同一次运行能力下的命令**在 workspace 内**要真的跑起来。
///
/// `--permission full` 下 DSH 完全不套沙箱（§2.1：`danger-full-access` 不设防），
/// 所以它证明的是「命令这条路是通的、工具是真的」—— 拦不住的原因不在工具坏。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_full_pwsh_runs_inside() {
    let case = Case::new("full-pwsh");
    let outcome = case.run(
        call_then_close(
            shell_tool(),
            shell_args("Set-Content -Path inside.txt -Value 'ran'; Get-Content inside.txt"),
        ),
        "full",
    );

    let results = outcome.tool_results();
    assert_eq!(results[0]["status"], "ok", "full 档下命令该跑起来：{results:#?}\n{}", outcome.stderr);
    assert_eq!(
        std::fs::read_to_string(case.workspace().join("inside.txt")).unwrap().trim(),
        "ran",
        "命令报了成功但没写出文件"
    );
    assert_honest_terminal(&outcome);
}

// ── 6 · 提权请求被 `never` 策略拒（不是「恰好没人应答」） ────────────────

/// §7 第 6 条：第 1 条之后补一次提权重试（带 `sandbox_permissions` + `justification`）。
///
/// 期望：被拒，理由**来自我们的 `never` 策略**，而不是 `no approval channel is available`
/// —— 两者形状不同，说明 `never` 是策略决定而不是「恰好没人应答」（§4 最后一行）。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_escalation_is_rejected_by_policy_not_by_missing_answerer() {
    let case = Case::new("escalation");
    let outcome = case.run(
        call_then_close(
            "write",
            json!({
                "file_path": "escalated.txt",
                "content": "nope",
                "sandbox_permissions": "danger-full-access",
                "justification": "M3 负例：请求把这一次写入提到全权",
            }),
        ),
        "readonly",
    );

    let hit = outcome.assert_denied("write", "rejected escalating");
    let summary = hit["summary"].as_str().unwrap_or_default();
    assert!(
        !summary.contains("no approval channel"),
        "拒因不像策略决定，而像「没有应答者」：{summary}"
    );
    assert!(!case.workspace().join("escalated.txt").exists());
    assert_honest_transcripts_mention(outcome, "rejected escalating");
}

// ── 7 · 正例对照：full 档下同样的越界写会成功 ───────────────────────────

/// §7 第 7 条：`--permission full`，同一个「写到 workspace 之外」的调用 —— **成功**。
///
/// 这是反向对照，证明前面几条不是「什么都失败」的假阳性。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_full_permission_write_outside_succeeds() {
    let outside = home_dir().join("arcdeck-m3b-full-outside-write.txt");
    let _ = std::fs::remove_file(&outside);

    let case = Case::new("full-outside");
    let outcome = case.run(
        call_then_close("write", json!({ "file_path": outside.display().to_string(), "content": "full access\n" })),
        "full",
    );

    let results = outcome.tool_results();
    assert_eq!(
        results[0]["status"], "ok",
        "full 档下越界写该成功 —— 若这条挂了，前面几条「被拒」就没有对照：{results:#?}"
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "full access\n");
    let _ = std::fs::remove_file(&outside);
    assert_honest_terminal(&outcome);
}

// ── 8 · commit / push 拦不住（已知缺口，量出来并如实记） ─────────────────

/// §7 第 8 条：`commit = false`（缺省）+ 非 worktree 的 git 仓，跑 `git commit`。
///
/// **会成功** —— 这就是 §3 的缺口。量出来写进结果，不要假装它被拦住了。
///
/// 命令这条路在 `workspace-write` 档下本机跑不起来（见第 5 条），所以这条用
/// `--permission full`：`commit` 那条**留档**与沙箱档无关（`allow_commit: false`
/// 是配置里的），越权的判定也因此与命令能否跑起来解耦。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_git_commit_is_not_blocked_by_the_guard() {
    let case = Case::new("git-commit");
    let repo = case.workspace();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "m3b@example.invalid"],
        vec!["config", "user.name", "M3B"],
    ] {
        let status = Command::new("git").args(&args).current_dir(repo).status().unwrap();
        assert!(status.success(), "git {args:?} 失败");
    }
    std::fs::write(repo.join("README.md"), "first\n").unwrap();
    assert!(Command::new("git").args(["add", "-A"]).current_dir(repo).status().unwrap().success());
    assert!(
        Command::new("git")
            .args(["commit", "-qm", "init"])
            .current_dir(repo)
            .status()
            .unwrap()
            .success()
    );

    let outcome = case.run(
        call_then_close(
            shell_tool(),
            shell_args("Set-Content -Path README.md -Value 'second'; git add -A; git commit -qm m3b-guard-probe; git log --oneline"),
        ),
        "full",
    );

    // 驱动这一条的前提是命令**真的跑了**：`full` 档不套沙箱，所以它必须成功。
    let results = outcome.tool_results();
    assert_eq!(
        results[0]["status"], "ok",
        "命令这条路没通 —— 这条用例就量不到 commit 缺口了：{results:#?}\n{}",
        outcome.stderr
    );
    let log = Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(repo)
        .output()
        .unwrap();
    let lines = String::from_utf8_lossy(&log.stdout).lines().count();
    assert_eq!(lines, 2, "commit 没有被执行层拦住 —— 这正是 §3 记下的缺口（实测确认）");

    // 缺口要**写在纸上**：`unverified` 里必须逐条报出来。
    let result = outcome.result();
    let unverified = result["unverified"].as_array().cloned().unwrap_or_default();
    assert!(
        unverified.iter().any(|u| u.as_str().unwrap_or("").contains("提交")),
        "commit 缺口没有进 unverified：{unverified:?}"
    );
    assert_honest_terminal(&outcome);
}

// ── 9 · 读取不受限（第二处缺口） ────────────────────────────────────────

/// §7 第 9 条：`readonly` 会话读 workspace 之外的文件。
///
/// **会成功** —— 第二处缺口（读范围）。DSH 三个档位都允许读任意绝对路径
/// （`fs-sandbox/src/index.ts:7-8` 的 `Reads pass through untouched`）。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_readoutside_workspace_is_allowed() {
    // Windows 上 `C:\Windows\win.ini` 是任何用户都读得到的稳定目标；Linux 用 /etc/hostname。
    let target = if cfg!(windows) {
        PathBuf::from("C:/Windows/win.ini")
    } else {
        PathBuf::from("/etc/hostname")
    };
    assert!(target.is_file(), "前置：{} 该存在", target.display());

    let case = Case::new("ro-read-outside");
    let outcome = case.run(
        call_then_close("read", json!({ "file_path": target.display().to_string() })),
        "readonly",
    );

    let results = outcome.tool_results();
    assert_eq!(
        results[0]["status"], "ok",
        "`readonly` 档读 workspace 之外竟然是**成功**的 —— 这是 §3 记下的第二处缺口，\
         本条就是把它量出来：{results:#?}"
    );
    assert_honest_terminal(&outcome);
}

// ── 10 · 子 Agent 不提权 ────────────────────────────────────────────────

/// §7 第 10 条：Plan 会话调 `subagent`（`subagent` 行还在），子 Agent 里再试 `write`。
///
/// 期望：子会话的 `sandbox/mode` 是 `read-only`（`source: 'delegation'`），
/// 写同样被拒；且子 Agent 的工具表里没有 shell 类工具。
///
/// 脚本形状：第 1 次请求（父）委派；第 2 次请求（**子**）试着写 —— 它被拒；
/// 之后一律 `default` 收尾（委派会让子会话也发请求，总次数不随父会话走）。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_subagent_cannot_write_and_has_no_shell() {
    let case = Case::new("subagent");
    let outcome = case.run(
        json!({
            "responses": [
                { "blocks": [call("subagent", json!({
                    "description": "M3 负例：子 Agent 越权",
                    "prompt": "在 workspace 里创建 subagent-child.txt" }))] },
                { "blocks": [call("write", json!({ "file_path": "subagent-child.txt", "content": "child" }))] },
            ],
            "default": { "blocks": [text("收尾")] },
        }),
        "readonly",
    );

    // 子 Agent 的请求也要走假端点 —— 没有第 2 次请求说明委派没发生，那不是「被拦住」。
    assert!(
        outcome.requests.len() >= 2,
        "只收到 {} 次请求，委派没有真的发生：{:#?}",
        outcome.requests.len(),
        outcome.requests.iter().map(|r| &r["path"]).collect::<Vec<_>>()
    );
    // 子会话的工具表同样窄：没有 shell 类工具。
    let child_tools = outcome.tools_at(1);
    assert!(
        !child_tools.iter().any(|t| t == shell_tool()),
        "子 Agent 的工具表里不该有 `{}`：{child_tools:?}",
        shell_tool()
    );
    // 子会话里的写被拒，文件不存在。
    let results = outcome.tool_results();
    assert!(
        results.iter().any(|r| r["tool"] == "write" && r["status"] == "error"),
        "子 Agent 里的写没有被拒：{results:#?}"
    );
    assert!(
        !case.workspace().join("subagent-child.txt").exists(),
        "子 Agent 的写入落盘了"
    );
    assert_honest_terminal(&outcome);
}

// ── 12 · network=false 时 web 工具不存在 ────────────────────────────────

/// §7 第 12 条：`network = false` + shell 关，调 `web_search`；再试 shell 工具。
///
/// 期望：web 工具**不存在**（`tool-web` 行被摘），shell 工具也不存在。
///
/// 这条走 `worker run`（配置手写）：`arcdeck run` 物化启动配置时 `network` 恒为 true，
/// 产品层还没有关网络的开关 —— 而 §3 的「`network=false` 但 shell 还开着 → 拒绝启动」
/// 那条规则要求调用方能表达这个组合，所以只有手写配置这条路能走到它。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_network_off_removes_web_and_shell_tools() {
    let case = Case::new("network-off");
    let provider = case.provider(json!({ "responses": [
        { "blocks": [call("web_search", json!({ "queries": ["m3 negative"] }))] },
        { "blocks": [call(shell_tool(), shell_args("Write-Output should-not-run"))] },
    ], "default": { "blocks": [text("收尾")] } }));
    let config = case.worker_config(&provider.url(), "read-only", "workspace-readonly", false);
    let outcome = case.run_worker(&provider, config);

    let requests_note = format!("worker 退出码 {}，stderr：\n{}", outcome.code, outcome.stderr);
    assert!(!outcome.requests.is_empty(), "假端点没收到请求 —— {requests_note}");
    let tools = outcome.first_tools();
    for absent in ["web_search", "web_fetch", shell_tool()] {
        assert!(
            !tools.iter().any(|t| t == absent),
            "`network = false` + shell 关时 `{absent}` 不该出现在工具清单里：{tools:?}"
        );
    }
    // 模型仍然可以提出这些调用 —— 拿到的必须是 UNKNOWN_TOOL（不是「执行后被拒」）。
    outcome.assert_denied("web_search", "unknown tool");
    outcome.assert_denied(shell_tool(), "unknown tool");
}

// ── 13 · 未知工具名 ────────────────────────────────────────────────────

/// §7 第 13 条：拼错的工具名。
///
/// 期望：`UNKNOWN_TOOL`。**这条不要当成拒权证据** —— 与第 3 条是同一类形状
/// （工具不存在），必须分开记。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_unknown_tool_name_is_not_a_denial() {
    let case = Case::new("unknown-tool");
    let outcome = case.run(call_then_close("definitely_not_a_tool", json!({})), "workspace");

    // 形状与沙箱拒绝不同：这里没有 `[sandbox: …]` 标记。
    let hit = outcome.assert_denied("definitely_not_a_tool", "unknown tool");
    let summary = hit["summary"].as_str().unwrap_or_default();
    assert!(
        !summary.contains("[sandbox:"),
        "未知工具被误当成沙箱拒绝：{summary}"
    );
    assert_honest_terminal(&outcome);
}

// ── 14 · 会话日志里的授权钉点 ──────────────────────────────────────────

/// §7 第 14 条：断言会话日志里 `permission/preset`、`sandbox/mode`、`approval/policy`
/// 三条事件与本次 guard 逐字一致。
///
/// 这是「钉点真的写进去了」的唯一直接证据 —— 本轮之前只有源码推断（§6 的「推测」那一格）。
/// 会话日志是 zstd 压的（AGENTS.md P7），这里用本机的 `zstd` 命令行解压（不许引新依赖）。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_session_log_records_the_pinned_guard_events() {
    let case = Case::new("session-log");
    let outcome = case.run(
        call_then_close("write", json!({ "file_path": "x.txt", "content": "nope" })),
        "readonly",
    );
    outcome.assert_denied("write", "[sandbox: file access denied under read-only mode]");

    let lines = session_log_lines(&outcome).unwrap_or_else(|e| {
        panic!(
            "{e}\n（会话日志是唯一直接读到内核授权钉点的地方；读不到就不能声称它写进去了）"
        )
    });
    let event = |kind: &str| -> Value {
        lines
            .iter()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|v| v["type"] == kind)
            .unwrap_or_else(|| panic!("会话日志里没有 `{kind}` 事件：{lines:#?}"))
    };

    assert_eq!(event("permission/preset")["data"]["preset"], "readonly");
    assert_eq!(event("sandbox/mode")["data"]["mode"], "read-only");
    assert_eq!(event("approval/policy")["data"]["policy"], "never");
}

/// 会话日志是 zstd 压的 `session.jsonl.zstd`（AGENTS.md P7），用本机的 `zstd` 命令行解。
///
/// 找不到 `zstd` 就明确报出来 —— 让调用方看见「这条没测成」，而不是静默跳过。
fn session_log_lines(outcome: &Outcome) -> Result<Vec<String>, String> {
    let sessions = outcome.run_dir.join("dsh-home").join("sessions");
    let found = walk(&outcome.run_dir);
    let listing: Vec<String> = found
        .iter()
        .map(|p| p.strip_prefix(&outcome.run_dir).unwrap_or(p).display().to_string())
        .collect();
    // 文件名带版本号（本轮实测：`session.v4.jsonl.zstd`）。按**后缀**匹配，
    // 不写死 `session.jsonl.zstd` —— 那个名字是 AGENTS.md P7 记的旧形状，
    // 写死会让这条用例在升级 DSH 后以「找不到日志」的形式假失败。
    let log = found
        .into_iter()
        .find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().ends_with(".jsonl.zstd"))
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            format!("{} 下没有 *.jsonl.zstd；run_dir 里实际有：{listing:#?}", sessions.display())
        })?;
    let zstd = std::env::var("ARCDECK_TEST_ZSTD").unwrap_or_else(|_| "zstd".to_string());
    let out = Command::new(&zstd)
        .args(["-d", "-c"])
        .arg(&log)
        .output()
        .map_err(|e| format!("跑不起 `{zstd}`（{e}）；用 $ARCDECK_TEST_ZSTD 指定路径"))?;
    if !out.status.success() {
        return Err(format!("`{zstd} -d -c` 退出 {}：{}", out.status, String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect())
}

/// 递归列出文件（只用 `std`：不许引新依赖）。
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

// ── 量形状的探针（不是断言） ────────────────────────────────────────────

/// 不是断言，是**量形状**：把真实请求里的工具清单与工具结果原文打出来。
///
/// 留着它是有用的：`docs/M3_GUARD.md` 的每一条结论都要能追溯到一次真实运行，
/// 而形状一旦变了（升 DSH 版本、换平台），这条用例会第一时间把它摆出来。
#[test]
#[ignore = "live：需要本机已安装钉住的 DSH 运行时"]
fn m3_live_probe_shows_the_real_request_and_result_shapes() {
    let genuinely_outside = home_dir().join("arcdeck-m3b-probe-outside.txt");
    let temp_root_target = std::env::temp_dir().join("arcdeck-m3b-probe-temp-root.txt");
    for path in [&genuinely_outside, &temp_root_target] {
        let _ = std::fs::remove_file(path);
    }

    let mut cases: BTreeMap<&str, (Value, &str)> = BTreeMap::new();
    cases.insert(
        "readonly-write",
        (call_then_close("write", json!({ "file_path": "x.txt", "content": "ok" })), "readonly"),
    );
    cases.insert(
        "workspace-write-outside",
        (
            call_then_close("write", json!({ "file_path": genuinely_outside.display().to_string(), "content": "no" })),
            "workspace",
        ),
    );
    cases.insert(
        "workspace-write-temp-root",
        (
            call_then_close("write", json!({ "file_path": temp_root_target.display().to_string(), "content": "yes" })),
            "workspace",
        ),
    );
    cases.insert(
        "readonly-escalation",
        (
            call_then_close(
                "write",
                json!({ "file_path": "x.txt", "content": "ok", "sandbox_permissions": "danger-full-access",
                        "justification": "M3 探针：请求提权" }),
            ),
            "readonly",
        ),
    );
    cases.insert("unknown-tool", (call_then_close("definitely_not_a_tool", json!({})), "workspace"));
    cases.insert(
        "workspace-pwsh",
        (call_then_close(shell_tool(), shell_args("Write-Output hello-m3b")), "workspace"),
    );
    cases.insert(
        "full-pwsh",
        (call_then_close(shell_tool(), shell_args("Write-Output hello-m3b")), "full"),
    );
    cases.insert(
        "readonly-read-outside",
        (call_then_close("read", json!({ "file_path": "C:/Windows/win.ini" })), "readonly"),
    );

    for (name, (script, permission)) in cases {
        let case = Case::new(name);
        let outcome = case.run(script, permission);
        println!("\n===== {name}（permission={permission}）退出码 {} =====", outcome.code);
        for (index, request) in outcome.requests.iter().enumerate() {
            println!(
                "-- 第 {index} 次请求的工具清单（{}）-- {:?}",
                fake_provider::tool_names(&request["body"]).len(),
                fake_provider::tool_names(&request["body"])
            );
        }
        println!("-- 工具结果事件 --");
        for data in outcome.tool_results() {
            println!("{data}");
        }
        println!("-- 回到模型的工具结果原文 --");
        for text in outcome.result_texts() {
            println!("{text}");
        }
        println!("-- workspace 里有什么 --");
        let mut entries: Vec<String> = std::fs::read_dir(case.workspace())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        entries.sort();
        println!("{entries:?}");
        println!("-- result.json 终态 --");
        let result = outcome.result();
        println!("status={} blockers={}", result["status"], result["blockers"]);
    }
    let _ = std::fs::remove_file(&genuinely_outside);
    let _ = std::fs::remove_file(&temp_root_target);
}
