//! 离线 worker 的端到端验收。
//!
//! 这些测试**真的拉起一个子进程**，用假 SDK 对端跑完整条链路。用进程而不是
//! 进程内调用，是因为超时、取消与「运行时提前退出」这三类行为只有在真的
//! 进程边界上才成立。
//!
//! 每个测试自建一份配置与任务书，落在自己的临时目录里 —— 共用一个状态目录会
//! 让「重复 task_id 不重跑」那条规则把相邻测试判失败。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_arcdeck")
}

struct Case {
    dir: tempfile::TempDir,
    config: PathBuf,
    task: PathBuf,
}

impl Case {
    /// 建一个场景：`scenario` 是夹具场景，`acceptance` 是任务书的验收数组，
    /// `task_extra` 覆盖任务书字段，`config_extra` 覆盖启动配置字段。
    fn new(scenario: &str, acceptance: Value, task_extra: Value) -> Self {
        Self::build(
            json!({ "kind": "fixture-sdk", "scenario": scenario }),
            acceptance,
            task_extra,
            json!({}),
        )
    }

    fn new_full(scenario: &str, acceptance: Value, task_extra: Value, config_extra: Value) -> Self {
        Self::build(
            json!({ "kind": "fixture-sdk", "scenario": scenario }),
            acceptance,
            task_extra,
            config_extra,
        )
    }

    /// 需要换掉整个 runtime 变体时用这个。
    ///
    /// `runtime` 刻意**整体传入**而不是走 `merge`：递归合并会把
    /// `dsh-sdk` 合到 `fixture-sdk` 上，留下一个 `scenario` 字段，
    /// 于是测试测的是「配置拼错了」而不是「运行时不受支持」。
    fn with_runtime(runtime: Value, acceptance: Value, task_extra: Value, config_extra: Value) -> Self {
        Self::build(runtime, acceptance, task_extra, config_extra)
    }

    fn build(runtime: Value, acceptance: Value, task_extra: Value, config_extra: Value) -> Self {
        let dir = tempfile::tempdir().expect("建临时目录");
        let root = repo_root();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let mut config = json!({
            "schema_version": 1,
            "runtime": runtime,
            "state_root": dir.path().join("state").display().to_string(),
            "authorization": {
                "workspace": workspace.display().to_string(),
                "profile": "workspace-development",
                "read_paths": ["."],
                "write_paths": ["."],
                "allowed_agents": ["echo", "build", "plan"]
            },
            "agents": {
                "dir": root.join("agents").display().to_string(),
                "prompts_dir": root.join("prompts").display().to_string()
            },
            "timeouts": {
                "initialize_seconds": 15,
                "prompt_seconds": 15,
                "wait_seconds": 10,
                "default_task_seconds": 10
            }
        });

        let mut task = json!({
            "schema_version": 1,
            "task_id": "t1",
            "workspace": workspace.display().to_string(),
            "agent": "echo",
            "mode": "build",
            "instruction": "离线夹具任务",
            "acceptance": acceptance,
            "timeout_seconds": 10
        });
        merge(&mut task, task_extra);
        merge(&mut config, config_extra);

        let config_path = dir.path().join("worker.json");
        let task_path = dir.path().join("task.json");
        std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        std::fs::write(&task_path, serde_json::to_string_pretty(&task).unwrap()).unwrap();

        Self { dir, config: config_path, task: task_path }
    }

    fn result_path(&self) -> PathBuf {
        self.dir.path().join("result.json")
    }

    fn run(&self) -> Run {
        self.run_with(&[])
    }

    fn run_with(&self, extra: &[&str]) -> Run {
        let mut command = Command::new(binary());
        command
            .arg("worker")
            .arg("run")
            .arg("--config")
            .arg(&self.config)
            .arg("--task")
            .arg(&self.task)
            .arg("--format")
            .arg("jsonl")
            .arg("--result")
            .arg(self.result_path());
        for arg in extra {
            command.arg(arg);
        }
        let output = command.output().expect("运行 worker");
        Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }
}

/// 递归合并：两边都是对象就继续往下合，否则覆盖。
///
/// 浅覆盖会把整个 `authorization` 换掉，连带 workspace 一起丢掉 —— 那样
/// 「缩小 allowed_agents」的测试就变成了「配一份残缺配置」的测试。
fn merge(base: &mut Value, extra: Value) {
    let (Some(base), Value::Object(extra)) = (base.as_object_mut(), extra) else {
        return;
    };
    for (key, value) in extra {
        match base.get_mut(&key) {
            Some(existing) if existing.is_object() && value.is_object() => merge(existing, value),
            _ => {
                base.insert(key, value);
            }
        }
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn result(&self) -> Value {
        self.events()
            .into_iter()
            .filter(|e| e["type"] == "result")
            .next_back()
            .map(|e| e["data"].clone())
            .unwrap_or(Value::Null)
    }

    fn events(&self) -> Vec<Value> {
        self.stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<Value>(line)
                    .unwrap_or_else(|e| panic!("stdout 上有非 JSON 行：{line:?} —— {e}"))
            })
            .collect()
    }

    fn state_sequence(&self) -> Vec<String> {
        self.events()
            .into_iter()
            .filter(|e| e["type"] == "state")
            .map(|e| e["data"]["state"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

fn command_acceptance() -> Value {
    json!([{ "id": "check", "kind": "command", "command": "cargo test session_filter" }])
}

fn no_acceptance() -> Value {
    json!([])
}

/// 每个事件信封都必须带齐固定字段，且 `seq` 从 1 连续递增。
fn assert_envelope_invariants(run: &Run) {
    let events = run.events();
    assert!(!events.is_empty(), "没有任何事件");
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event["schema_version"], 1, "{event}");
        assert_eq!(event["task_id"], "t1", "{event}");
        assert_eq!(event["seq"].as_u64(), Some(index as u64 + 1), "seq 不连续：{event}");
    }
}

// ── 成功路径 ────────────────────────────────────────────────────────────

#[test]
fn success_reports_completed_with_evidence() {
    let case = Case::new("success", command_acceptance(), json!({}));
    let run = case.run();

    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    assert_envelope_invariants(&run);

    let result = run.result();
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(result["verification"][0]["outcome"], "passed", "{result}");
    assert_eq!(result["verification"][0]["exit_code"], 0);
    assert_eq!(result["usage"]["total_tokens"], 13);

    // 状态序列是契约的一部分。
    let states = run.state_sequence();
    assert_eq!(states.first().map(String::as_str), Some("accepted"));
    assert_eq!(states.last().map(String::as_str), Some("terminal"));

    // 结果文件必须与 stdout 上的 result 事件一致。
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(case.result_path()).unwrap()).unwrap();
    assert_eq!(written["status"], "completed");
    assert_eq!(written["task_id"], "t1");
}

#[test]
fn assistant_text_is_not_written_twice() {
    let case = Case::new("success", command_acceptance(), json!({}));
    let run = case.run();
    let summary = run.result()["summary"].as_str().unwrap_or_default().to_string();
    assert_eq!(summary, "离线完成");
}

#[test]
fn system_source_messages_stay_out_of_the_conversation() {
    // 夹具会发一条 source.kind=system 的 user/message；它不该出现在结果里。
    let case = Case::new("success", command_acceptance(), json!({}));
    let run = case.run();
    let summary = run.result()["summary"].as_str().unwrap_or_default().to_string();
    assert!(!summary.contains("系统提示词"), "系统来源的消息进了对话：{summary}");
}

#[test]
fn echo_style_sections_are_recorded_as_evidence() {
    let case = Case::new("success", command_acceptance(), json!({}));
    let run = case.run();
    let sections: Vec<String> = run
        .events()
        .into_iter()
        .filter(|e| e["type"] == "diagnostic" && e["data"]["code"] == "PROMPT_SECTION")
        .map(|e| e["data"]["message"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        sections.iter().any(|s| s.contains("echo-style.md")),
        "没有记录到 Echo 风格片段：{sections:?}"
    );
    assert!(
        sections.iter().any(|s| s.contains("arc-deck-engineering.md")),
        "没有记录到工程规则片段：{sections:?}"
    );
}

#[test]
fn tool_events_are_mirrored_with_matching_call_ids() {
    let case = Case::new("success", command_acceptance(), json!({}));
    let run = case.run();
    let events = run.events();
    let call = events.iter().find(|e| e["type"] == "tool_call").expect("应有 tool_call");
    let result = events.iter().find(|e| e["type"] == "tool_result").expect("应有 tool_result");
    assert_eq!(call["data"]["call_id"], "fixture-call-1");
    assert_eq!(result["data"]["call_id"], "fixture-call-1");
    assert_eq!(result["data"]["status"], "ok");
}

#[test]
fn split_utf8_frame_survives_the_reader() {
    // 夹具会把一个汉字拆到两次写之间。按读边界解码的实现会在这里产出替换字符。
    let case = Case::new("split-utf8", no_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    let summary = run.result()["summary"].as_str().unwrap_or_default().to_string();
    assert_eq!(summary, "离线完成", "中文被撕坏了：{summary:?}");
    assert!(!summary.contains('\u{fffd}'), "出现替换字符：{summary:?}");
    // 夹具在这个场景里不发工具记录，也照样是 completed（没有验收项）。
    assert_eq!(run.result()["status"], "completed");
}

#[test]
fn empty_acceptance_is_reported_as_unverified() {
    let case = Case::new("success", no_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 0);
    let unverified = run.result()["unverified"].as_array().cloned().unwrap_or_default();
    assert!(
        unverified.iter().any(|v| v.as_str().unwrap_or("").contains("没有声明验收项")),
        "没有把「没有验收项」如实报出来：{unverified:?}"
    );
}

// ── 失败与受阻 ──────────────────────────────────────────────────────────

#[test]
fn failure_turn_reports_failed_and_surfaces_the_model_failure_exit() {
    let case = Case::new("failure", no_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 1, "stderr={}", run.stderr);
    let result = run.result();
    assert_eq!(result["status"], "failed");
    let blockers = result["blockers"].as_array().cloned().unwrap_or_default();
    // 模型失败的唯一出口是 finish.reason.failure.message，必须出现在结果里；
    // 而且要被归成一个可判定的码 —— `supported API model names` 是路由/模型名错，
    // 调度者据此知道「改配置，别重派」，不必去读英文原文。
    assert!(
        blockers
            .iter()
            .any(|b| b["code"] == "MODEL_ROUTE"
                && b["message"].as_str().unwrap_or("").contains("deepseek-official")),
        "没有把模型失败原文与它的分类报出来：{blockers:?}"
    );

    // 同一件事在状态面上也要能一眼看出来：码 + 能不能靠重派解决。
    let status: Value = serde_json::from_str(
        &std::fs::read_to_string(case.dir.path().join("state/runs/t1/status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(status["last_error"]["code"], "MODEL_ROUTE");
    assert_eq!(status["last_error"]["retryable"], false, "模型名错重派无用");
}

#[test]
fn failing_acceptance_carries_a_decidable_reason() {
    // 工人正常收工、验收命令却以非零退出 —— 这是最常见的一种「做完了但没做对」。
    // 没有这条 blocker 的话，状态面上只有一个 `failed`，调度者得回去读 result.json。
    let case = Case::new("failing-acceptance", command_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 1, "stderr={}", run.stderr);
    let result = run.result();
    assert_eq!(result["status"], "failed");
    assert_eq!(result["verification"][0]["outcome"], "failed");
    assert_eq!(result["verification"][0]["exit_code"], 101);

    let blockers = result["blockers"].as_array().cloned().unwrap_or_default();
    assert!(
        blockers.iter().any(|b| b["code"] == "ACCEPTANCE_FAILED"
            && b["message"].as_str().unwrap_or("").contains("check")),
        "验收失败没有给出可判定的原因：{blockers:?}"
    );

    let status: Value = serde_json::from_str(
        &std::fs::read_to_string(case.dir.path().join("state/runs/t1/status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(status["last_error"]["code"], "ACCEPTANCE_FAILED");
    assert_eq!(status["last_error"]["retryable"], false, "同一份活儿原样再派一次还是不过");
}

#[test]
fn blocked_turn_reports_blocked() {
    let case = Case::new("blocked", no_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 2, "stderr={}", run.stderr);
    assert_eq!(run.result()["status"], "blocked");
}

#[test]
fn manual_acceptance_is_pending_and_yields_blocked() {
    let case = Case::new(
        "success",
        json!([{ "id": "look", "kind": "manual", "description": "看一眼截图" }]),
        json!({}),
    );
    let run = case.run();
    assert_eq!(run.code, 2, "stderr={}", run.stderr);
    let result = run.result();
    assert_eq!(result["status"], "blocked");
    assert_eq!(result["verification"][0]["outcome"], "pending");
}

#[test]
fn missing_command_evidence_is_not_run_and_yields_blocked() {
    // 夹具这个场景不发工具记录，所以验收命令「没有观察到执行」。
    let case = Case::new("split-utf8", command_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 2, "stderr={}", run.stderr);
    let result = run.result();
    assert_eq!(result["verification"][0]["outcome"], "not_run");
    assert_eq!(result["status"], "blocked");
}

// ── 协议与进程异常 ──────────────────────────────────────────────────────

#[test]
fn runtime_exit_without_terminal_is_unknown_not_success() {
    let case = Case::new("eof", no_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 5, "stderr={}", run.stderr);
    let result = run.result();
    assert_eq!(result["status"], "unknown");
    let blockers = result["blockers"].as_array().cloned().unwrap_or_default();
    assert!(
        blockers.iter().any(|b| b["code"] == "RUNTIME_EXITED"),
        "没有报出运行时提前退出：{blockers:?}"
    );
}

#[test]
fn non_json_on_stdout_is_a_protocol_failure() {
    let case = Case::new("bad-frame", no_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 1, "stderr={}", run.stderr);
    let blockers = run.result()["blockers"].as_array().cloned().unwrap_or_default();
    assert!(
        blockers.iter().any(|b| b["code"] == "PROTOCOL_ERROR"),
        "坏帧没有被当作协议错误：{blockers:?}"
    );
}

#[test]
fn mismatched_response_id_is_a_protocol_failure() {
    let case = Case::new("wrong-id", no_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 1, "stderr={}", run.stderr);
    let blockers = run.result()["blockers"].as_array().cloned().unwrap_or_default();
    assert!(
        blockers.iter().any(|b| b["code"] == "PROTOCOL_ERROR"),
        "错位响应没有被识别：{blockers:?}"
    );
}

// ── 超时与取消 ──────────────────────────────────────────────────────────

#[test]
fn wall_clock_timeout_is_reported_as_timed_out() {
    let case = Case::new("hang", no_acceptance(), json!({ "timeout_seconds": 2 }));
    let run = case.run();
    assert_eq!(run.code, 4, "stderr={}", run.stderr);
    let blockers = run.result()["blockers"].as_array().cloned().unwrap_or_default();
    assert!(
        blockers.iter().any(|b| b["code"] == "TASK_TIMEOUT"),
        "没有报出超时：{blockers:?}"
    );
}

/// 卡死 → 杀掉运行时 → 续跑 → 再卡死 → 次数用尽判 failed（PRODUCT_PLAN §6.5.4）。
/// 阈值调到秒级，让「卡死」在测试里两三秒就出现；墙钟预算给得远大于它，
/// 证明结论来自卡死判定而不是超时。
#[test]
fn stalled_run_is_restarted_then_fails_with_stalled_when_attempts_run_out() {
    let case = Case::new_full(
        "hang",
        no_acceptance(),
        json!({ "timeout_seconds": 60 }),
        json!({ "liveness": { "heartbeat_seconds": 1, "model_idle_seconds": 1, "tool_stall_seconds": 60, "max_attempts": 2 } }),
    );
    let run = case.run();
    assert_eq!(run.code, 1, "stderr={}", run.stderr);
    let blockers = run.result()["blockers"].as_array().cloned().unwrap_or_default();
    assert!(blockers.iter().any(|b| b["code"] == "STALLED"), "没有报出卡死：{blockers:?}");
    // 续跑真的发生过：事件流里有一条 STALLED 诊断（第一次卡死后开第二次）。
    assert!(
        run.stdout.lines().any(|l| l.contains("\"STALLED\"") && l.contains("diagnostic")),
        "第一次卡死后没有续跑：{}",
        run.stdout
    );
    // 状态文件：终态、两次运行、最后的错误码。读者只看这个文件就能知道发生了什么。
    let status: Value = serde_json::from_str(
        &std::fs::read_to_string(case.dir.path().join("state/runs/t1/status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(status["terminal"], true);
    assert_eq!(status["state"], "failed");
    assert_eq!(status["attempt"], 2);
    assert_eq!(status["last_error"]["code"], "STALLED");
}

#[test]
fn status_file_tracks_a_successful_run_to_its_terminal_state() {
    // `assigned_by` 与标题是给外部监控程序画「任务书流向」用的，走的是
    // 任务书 → status.json 这条完整链路，所以在这里一并钉住。
    let case = Case::new(
        "success",
        no_acceptance(),
        json!({ "assigned_by": "sol", "instruction": "# 修会话筛选\n\n正文……" }),
    );
    let run = case.run();
    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    let status: Value = serde_json::from_str(
        &std::fs::read_to_string(case.dir.path().join("state/runs/t1/status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(status["terminal"], true);
    assert_eq!(status["state"], "completed");
    assert_eq!(status["attempt"], 1);
    assert_eq!(status["assigned_by"], "sol");
    assert_eq!(status["title"], "修会话筛选");
    assert!(status["heartbeat_at"].as_u64().unwrap() >= status["started_at"].as_u64().unwrap());
}

/// `run --detach` 的派工握手：**分离不等于成功**。
///
/// 只 `spawn` 就返回 0 的版本里，一次预检失败会打印「started」并退出 0，而
/// `status --all` 报「没有任务」—— 原因只留在 `console.err` 里，调度者只能回去
/// 翻日志。这条测试钉住两件事：失败要**报出来**（退出码与 stdout 都不冒充成功），
/// 而且**要落进状态面**（否则监控程序与 `status` 都看不见这条派工）。
///
/// 密钥故意走 `{env:…}` 且变量名不存在，这样无论本机有没有配过 auth.json，
/// 预检都必然失败 —— 测试不依赖运行环境。
#[test]
fn detached_dispatch_reports_a_preflight_failure_instead_of_printing_started() {
    let dir = tempfile::tempdir().expect("建临时目录");

    // 假运行时：只为让 `cmd_run` 的 `layout.check` 过去；预检会在取密钥那一步先失败。
    let runtime = dir.path().join("runtime/node_modules/@deepseek-ai/dsh");
    std::fs::create_dir_all(runtime.join("lib")).unwrap();
    std::fs::write(runtime.join("package.json"), r#"{"version":"0.2.1-alpha.2"}"#).unwrap();
    std::fs::write(runtime.join("lib/bin.js"), "// 不会被执行的假入口\n").unwrap();

    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let state = dir.path().join("state");

    let config = dir.path().join("arcdeck.json");
    std::fs::write(
        &config,
        serde_json::to_string_pretty(&json!({
            "model": "cam-no-key/some-model",
            "provider": {
                "cam-no-key": {
                    "name": "测试用：永远取不到密钥的供应商",
                    "api": "anthropic-messages",
                    "options": {
                        "baseURL": "http://127.0.0.1:1",
                        "apiKey": "{env:ARCDECK_TEST_ABSENT_KEY}"
                    },
                    "models": { "some-model": { "name": "无" } }
                }
            },
            "runtime": { "dir": dir.path().join("runtime").display().to_string() },
            "state_root": state.display().to_string()
        }))
        .unwrap(),
    )
    .unwrap();

    let task = dir.path().join("task.md");
    std::fs::write(&task, "# 派工握手测试\n\n正文……\n").unwrap();

    let output = Command::new(binary())
        .arg("run")
        .arg(&task)
        .arg("--id")
        .arg("dispatch-handshake")
        .arg("--by")
        .arg("claude")
        .arg("--config")
        .arg(&config)
        .arg("--workspace")
        .arg(&workspace)
        .arg("--state-root")
        .arg(&state)
        .arg("--detach")
        .output()
        .expect("派工");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert_eq!(output.status.code(), Some(64), "stdout={stdout} stderr={stderr}");
    assert!(stderr.contains("CONFIG_INVALID"), "stderr={stderr}");
    assert!(!stdout.contains("started"), "预检失败不该报 started：{stdout}");

    // 关键的一条：失败要落在状态面上。
    let status: Value = serde_json::from_str(
        &std::fs::read_to_string(state.join("runs/dispatch-handshake/status.json"))
            .expect("预检失败也要写出 status.json"),
    )
    .unwrap();
    assert_eq!(status["state"], "failed");
    assert_eq!(status["terminal"], true);
    assert_eq!(status["last_error"]["code"], "CONFIG_INVALID");
    // 派工的边与标签由派工方代写 —— 工人没跑到那一步就死了。
    assert_eq!(status["assigned_by"], "claude");
    assert_eq!(status["title"], "派工握手测试");
    assert_eq!(status["attempt"], 0, "一次都没真正跑起来");
}

#[test]
fn cancel_file_stops_the_run_and_confirms_the_process_exit() {
    let case = Case::new("cancel", no_acceptance(), json!({ "timeout_seconds": 30 }));
    let cancel = case.dir.path().join("cancel.json");

    let child = Command::new(binary())
        .arg("worker")
        .arg("run")
        .arg("--config")
        .arg(&case.config)
        .arg("--task")
        .arg(&case.task)
        .arg("--result")
        .arg(case.result_path())
        .arg("--cancel-file")
        .arg(&cancel)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("启动 worker");

    // 等它真的跑起来再请求取消 —— 太早的话进程还没观察到文件。
    std::thread::sleep(std::time::Duration::from_millis(1500));
    std::fs::write(
        &cancel,
        r#"{"schema_version":1,"task_id":"t1","action":"cancel","reason":"测试取消"}"#,
    )
    .unwrap();

    let output = child.wait_with_output().expect("等待 worker 退出");
    assert_eq!(output.status.code(), Some(3), "stderr={}", String::from_utf8_lossy(&output.stderr));

    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(case.result_path()).unwrap()).unwrap();
    assert_eq!(written["status"], "cancelled");
    let blockers = written["blockers"].as_array().cloned().unwrap_or_default();
    assert!(
        blockers.iter().any(|b| b["code"] == "CANCELLED"),
        "没有记下取消原因：{blockers:?}"
    );
    // 取消必须确认进程真的退出了，否则不能声称取消完成。
    assert!(
        !blockers.iter().any(|b| b["code"] == "CANCEL_NOT_CONFIRMED"),
        "进程没有被确认退出：{blockers:?}"
    );
}

#[test]
fn a_stale_cancel_file_is_refused_before_anything_starts() {
    let case = Case::new("success", no_acceptance(), json!({}));
    let cancel = case.dir.path().join("cancel.json");
    std::fs::write(&cancel, "{}").unwrap();
    let run = case.run_with(&["--cancel-file", cancel.to_str().unwrap()]);
    assert_eq!(run.code, 64, "stderr={}", run.stderr);
    assert!(run.stderr.contains("CONFIG_INVALID"), "stderr={}", run.stderr);
}

// ── 交付物与幂等 ────────────────────────────────────────────────────────

#[test]
fn result_write_failure_never_exits_zero() {
    let case = Case::new("success", command_acceptance(), json!({}));
    // 把结果写到目录上：写入必然失败。
    let run = case.run_with(&["--result", case.dir.path().to_str().unwrap()]);
    assert_eq!(run.code, 5, "写结果失败时不得退出 0；stderr={}", run.stderr);
    let blockers = run.result()["blockers"].as_array().cloned().unwrap_or_default();
    assert!(
        blockers.iter().any(|b| b["code"] == "RESULT_WRITE_FAILED"),
        "没有报出结果写失败：{blockers:?}"
    );
}

#[test]
fn the_same_task_id_is_not_rerun_in_the_same_state_root() {
    let case = Case::new("success", no_acceptance(), json!({}));
    let first = case.run();
    assert_eq!(first.code, 0, "stderr={}", first.stderr);

    let second = case.run();
    assert_eq!(second.code, 64, "第二次不该重跑；stderr={}", second.stderr);
    assert!(second.stderr.contains("TASK_ALREADY_SEEN"), "stderr={}", second.stderr);
}

#[test]
fn unauthorized_agent_is_refused_before_starting_the_runtime() {
    let case = Case::new_full(
        "success",
        no_acceptance(),
        json!({ "agent": "build" }),
        json!({ "authorization": { "allowed_agents": ["echo"] } }),
    );
    let run = case.run();
    assert_eq!(run.code, 64, "stderr={}", run.stderr);
    assert!(run.stderr.contains("AGENT_NOT_AUTHORIZED"), "stderr={}", run.stderr);
    // 没启动就不该有任何事件流。
    assert!(run.stdout.trim().is_empty(), "越权请求不该产生事件：{}", run.stdout);
}

#[test]
fn a_missing_dsh_runtime_is_refused_before_any_state_is_created() {
    // 真实 DSH 没装（入口文件不存在）：必须在**建立任何状态之前**失败，否则 stdout 上会先
    // 出现半条事件流、再以 64 退出，而状态目录里还会留下一条「跑过」的记录。
    let case = Case::with_runtime(
        json!({
            "kind": "dsh-sdk",
            "entry": "not-installed/bin.js",
            "version": "0.2.1-alpha.2",
            "profile": "sdk"
        }),
        no_acceptance(),
        json!({}),
        // dsh-sdk 需要显式模型路由，否则会先在配置层被拒 —— 那样测的就不是
        // 「运行时没装」了。
        json!({ "model": { "provider": "deepseek-official", "model": "deepseek-flash" } }),
    );
    let run = case.run();
    assert_eq!(run.code, 64, "stderr={}", run.stderr);
    assert!(run.stderr.contains("RUNTIME_ENTRY_MISSING"), "stderr={}", run.stderr);
    assert!(run.stderr.contains("arcdeck runtime install"), "要告诉用户怎么修：{}", run.stderr);
    assert!(run.stdout.trim().is_empty(), "被拒绝的配置不该产生事件流：{}", run.stdout);
    assert!(
        !case.dir.path().join("state/runs/t1").exists(),
        "被拒绝的配置不该留下运行目录"
    );
}

#[test]
fn an_unknown_fixture_scenario_is_refused_with_the_available_list() {
    let case = Case::new_full(
        "success",
        no_acceptance(),
        json!({}),
        json!({ "runtime": { "kind": "fixture-sdk", "scenario": "does-not-exist" } }),
    );
    let run = case.run();
    assert_eq!(run.code, 64, "stderr={}", run.stderr);
    assert!(run.stderr.contains("CONFIG_INVALID"), "stderr={}", run.stderr);
    assert!(run.stderr.contains("success"), "错误信息里应列出可用场景：{}", run.stderr);
    assert!(run.stdout.trim().is_empty(), "{}", run.stdout);
}

// ── 仓库里那份夹具 ──────────────────────────────────────────────────────

#[test]
fn checked_in_fixture_uses_relative_paths() {
    // 这份夹具用相对配置文件解析的路径，验证「相对路径的基准是文件所在目录」。
    let root = repo_root();
    let state = root.join("target/arc-deck-worker-state");
    let _ = std::fs::remove_dir_all(&state);

    let result = tempfile::NamedTempFile::new().unwrap();
    let output = Command::new(binary())
        .arg("worker")
        .arg("run")
        .arg("--config")
        .arg(root.join("tests/fixtures/worker-offline.json"))
        .arg("--task")
        .arg(root.join("tests/fixtures/worker-task.json"))
        .arg("--format")
        .arg("jsonl")
        .arg("--result")
        .arg(result.path())
        .output()
        .expect("运行仓库夹具");

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(result.path()).unwrap()).unwrap();
    assert_eq!(written["status"], "completed");
    assert_eq!(written["task_id"], "offline-fixture-smoke");
    assert_eq!(written["policy"]["agent"], "echo");
}
