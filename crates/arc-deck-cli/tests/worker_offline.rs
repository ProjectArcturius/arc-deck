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

/// 提示词片段要留痕：`system prompt 片段 <名字> ← <来源>`，这样「Echo 到底进没进
/// system prompt」是可核对的，不必去猜。
///
/// 2026-10-14 起契约改了：**工人不带风格**（人格生效于非 workers 场景），所以这里
/// 同时钉住两件事 —— 功能片段要记，风格片段**不能**记。
#[test]
fn prompt_sections_are_recorded_and_workers_carry_no_style() {
    let case = Case::new("success", command_acceptance(), json!({}));
    let run = case.run();
    let sections: Vec<String> = run
        .events()
        .into_iter()
        .filter(|e| e["type"] == "diagnostic" && e["data"]["code"] == "PROMPT_SECTION")
        .map(|e| e["data"]["message"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        sections.iter().any(|s| s.contains("arc-deck-engineering.md")),
        "没有记录到工程纪律片段：{sections:?}"
    );
    assert!(
        sections.iter().any(|s| s.contains("echo.md")),
        "没有记录到角色定义片段：{sections:?}"
    );
    assert!(
        !sections.iter().any(|s| s.contains("echo-style.md")),
        "工人不该带风格（人格生效于非 workers 场景）：{sections:?}"
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
    let case = Case::new("failing-acceptance", command_acceptance(), json!({ "assigned_by": "sol" }));
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

    // 调度者的第二个命令。一条 `report` 要把五件事答完：谁派的（派工 sol）、
    // 干什么（任务书标题）、什么样（failed + 验收明细）、为什么（ACCEPTANCE_FAILED）、
    // 要不要再派（不能）。答不完就得回去翻 result.json 或 events.jsonl —— 那正是要消掉的成本。
    let report = Command::new(binary())
        .arg("report")
        .arg("t1")
        .arg("--state-root")
        .arg(case.dir.path().join("state"))
        .output()
        .expect("report");
    assert_eq!(report.status.code(), Some(1));
    let text = String::from_utf8_lossy(&report.stdout).to_string();
    for needle in ["派工 sol", "离线夹具任务", "ACCEPTANCE_FAILED", "再派一次能解决吗：不能"] {
        assert!(text.contains(needle), "report 里没有 `{needle}`：\n{text}");
    }
}

/// 中途失败但**自己起来了**，不是失败（用户 2026-10-14 点名的场景）。
///
/// 断网五分钟、被限流、provider 抖一下，DSH 的 llm-retry 会在步边界重试并继续。
/// 这种任务不该报失败 —— 真正要报的是「从动到不动」。但重试这件事本身要如实记下来，
/// 而且要在**还在跑的时候**就让监控程序看得见。
#[test]
fn a_recovered_retry_is_not_a_failure_but_is_visible_while_running() {
    let case = Case::new("retry-then-succeed", no_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    let result = run.result();
    assert_eq!(result["status"], "completed");
    assert!(
        result["blockers"].as_array().map(|b| b.is_empty()).unwrap_or(true),
        "中途失败后自起了，不该有 blocker：{}",
        result["blockers"]
    );
    // 但「这条路径被打断过」要留痕。
    assert!(
        result["unverified"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u.as_str().unwrap_or("").contains("自行重试")),
        "{result}"
    );

    let status: Value = serde_json::from_str(
        &std::fs::read_to_string(case.dir.path().join("state/runs/t1/status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(status["counters"]["model_retries"], 1);
    // 中途的异常要**实时**可见，并且带「要不要担心」的判断。
    assert_eq!(status["notice"]["code"], "MODEL_RATE_LIMITED");
    assert_eq!(status["notice"]["retryable"], true);
    assert!(
        status["notice"]["message"].as_str().unwrap_or("").contains("tpm"),
        "原文要留着：{}",
        status["notice"]
    );
}

/// 轮次以**结构化**错误收尾时，分类走 `reason.error.code`/`status` 而不是猜英文。
/// 真实会话里额度用尽就是 `{message:"Insufficient Balance", code:"QUOTA", status:402}`。
#[test]
fn structured_error_fields_drive_the_classification() {
    let case = Case::new("quota", no_acceptance(), json!({}));
    let run = case.run();
    assert_eq!(run.code, 1, "stderr={}", run.stderr);
    let result = run.result();
    assert_eq!(result["status"], "failed");
    let blockers = result["blockers"].as_array().cloned().unwrap_or_default();
    assert!(
        blockers.iter().any(|b| b["code"] == "MODEL_QUOTA_EXHAUSTED"),
        "没有用结构化字段分类：{blockers:?}"
    );

    let status: Value = serde_json::from_str(
        &std::fs::read_to_string(case.dir.path().join("state/runs/t1/status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(status["last_error"]["code"], "MODEL_QUOTA_EXHAUSTED");
    assert_eq!(status["last_error"]["retryable"], false, "余额不足重派不会好");
}

// ── `run`：派工的生产路径 ────────────────────────────────────────────────
//
// `worker run` 是 M1 的底层入口，而 `run` 才是调度者真正用的那条（它负责物化配置、
// 建 worktree、决定一般模式还是工人模式）。所以它必须有自己的端到端覆盖。
//
// 接假内核靠 `ARCDECK_FIXTURE_SCENARIO` —— 那是 `test-runtime` 构建里唯一的出口
// （`cmd_run` 里 cfg 门控），发布构建编译不出它。

struct RunCase {
    dir: tempfile::TempDir,
    config: PathBuf,
    workspace: PathBuf,
}

impl RunCase {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("建临时目录");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let config = dir.path().join("arcdeck.json");
        std::fs::write(
            &config,
            serde_json::to_string_pretty(&json!({
                "model": "opencode-go/deepseek-v4.1-flash",
                "state_root": dir.path().join("state").display().to_string()
            }))
            .unwrap(),
        )
        .unwrap();
        Self { dir, config, workspace }
    }

    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn run(&self, extra: &[&str]) -> Run {
        self.run_in(&self.workspace, extra)
    }

    fn run_in(&self, workspace: &Path, extra: &[&str]) -> Run {
        let mut command = Command::new(binary());
        command.env("ARCDECK_FIXTURE_SCENARIO", "success").arg("run");
        for arg in extra {
            command.arg(arg);
        }
        let output = command
            .arg("--config")
            .arg(&self.config)
            .arg("--workspace")
            .arg(workspace)
            .output()
            .expect("run");
        Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }
}

/// 没有工人参数 = 一般模式：stdout 是**给人看的回答**，不是控制面协议。
/// 但状态照落 —— 监控程序对一般模式与工人模式一视同仁（用户 2026-10-14）。
#[test]
fn plain_mode_answers_in_text_and_still_records_state() {
    let case = RunCase::new();
    let run = case.run(&["解释一下这段报错"]);
    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    assert!(run.stdout.contains("离线完成"), "stdout={}", run.stdout);
    assert!(
        !run.stdout.contains("\"schema_version\""),
        "一般模式的 stdout 不该是 JSONL：{}",
        run.stdout
    );

    // 状态面必须齐全：谁派的没有（那是一般模式）、干什么有（标题就是那句话）、
    // 终态有。
    let runs = case.state().join("runs");
    let entry = std::fs::read_dir(&runs)
        .expect("状态目录里要有这次运行")
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with("run-"))
        .expect("缺省 ID 形如 run-<毫秒>");
    let status: Value =
        serde_json::from_str(&std::fs::read_to_string(entry.path().join("status.json")).unwrap())
            .unwrap();
    assert_eq!(status["state"], "completed");
    assert_eq!(status["title"], "解释一下这段报错");
    assert!(status["assigned_by"].is_null(), "一般模式没有派工者");

    let task: Value =
        serde_json::from_str(&std::fs::read_to_string(entry.path().join("task.json")).unwrap())
            .unwrap();
    assert_eq!(task["instruction"], "解释一下这段报错", "位置参数就是指令原文");

    // 个性化关（缺省）：**一段都不加** —— 这是「按供应商本体行事」的可判定形式。
    // 不是「加得少」，是空：调用方据此写出空的 personaSuffix。
    let prompt = std::fs::read_to_string(entry.path().join("system-prompt.md")).unwrap();
    assert!(prompt.trim().is_empty(), "一般模式 + 个性化关不该注入任何文本：{prompt}");
}

#[test]
fn explicit_prompt_does_not_open_a_file_or_inject_worker_rules() {
    let case = RunCase::new();
    let file = case.workspace.join("question.md");
    std::fs::write(&file, "这不是指令原文").unwrap();
    let literal = file.to_str().unwrap();
    let run = case.run(&["--prompt", literal, "--id", "literal"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let dir = case.state().join("runs/literal");
    let task: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("task.json")).unwrap()).unwrap();
    assert_eq!(task["instruction"], literal);
    assert!(std::fs::read_to_string(dir.join("system-prompt.md")).unwrap().is_empty());
}

#[test]
fn tui_style_stdin_prompt_survives_windows_argument_limits_and_shell_metacharacters() {
    let case = RunCase::new();
    let prompt = format!("{}\n`literal` $(never execute) ; & |\n/不是命令", "中文👩‍💻".repeat(8000));
    let mut child = Command::new(binary())
        .env("ARCDECK_FIXTURE_SCENARIO", "success")
        .args(["run", "--prompt", "-", "--format", "jsonl", "--id", "stdin-literal", "--agent", "echo", "--permission", "workspace"])
        .arg("--config").arg(&case.config).arg("--workspace").arg(&case.workspace)
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .spawn().unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), prompt.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    let events: Vec<Value> = String::from_utf8(output.stdout).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(events.last().unwrap()["type"], "result");
    let dir = case.state().join("runs/stdin-literal");
    let task: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("task.json")).unwrap()).unwrap();
    assert_eq!(task["instruction"], prompt);
    let config: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(config["prompt_scope"], "bare");
    assert!(std::fs::read_to_string(dir.join("system-prompt.md")).unwrap().is_empty());
}

#[test]
fn conflicting_literal_and_task_inputs_are_rejected_without_running() {
    let case = RunCase::new();
    let run = case.run(&["一条任务", "--prompt", "另一条"]);
    assert_eq!(run.code, 64);
    assert!(!case.state().join("runs").exists());
}

#[test]
fn no_arguments_without_a_terminal_reports_the_headless_alternative() {
    let case = RunCase::new();
    let output = Command::new(binary()).env("ARCDECK_CONFIG", &case.config)
        .current_dir(&case.workspace).output().unwrap();
    assert_eq!(output.status.code(), Some(64));
    assert!(String::from_utf8(output.stderr).unwrap().contains("交互终端"));
    assert!(!case.state().join("runs").exists());
}

/// 出现工人参数就切回工人模式：stdout 是控制面 JSONL，`--accept` 真的进了任务书。
#[test]
fn worker_flags_switch_back_to_the_jsonl_control_plane() {
    let case = RunCase::new();
    let run = case.run(&["修一下筛选", "--id", "w1", "--by", "sol", "--accept", "cargo test x"]);
    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    assert!(run.stdout.contains("\"type\":\"result\""), "stdout={}", run.stdout);

    let status: Value = serde_json::from_str(
        &std::fs::read_to_string(case.state().join("runs/w1/status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(status["assigned_by"], "sol");
    assert_eq!(status["title"], "修一下筛选");
    assert_eq!(status["terminal"], true);

    // 工人拿功能契约，但**不拿风格**（人格生效于非 workers 场景）。
    let sections: Vec<String> = run
        .events()
        .into_iter()
        .filter(|e| e["type"] == "diagnostic" && e["data"]["code"] == "PROMPT_SECTION")
        .map(|e| e["data"]["message"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(sections.iter().any(|s| s.contains("arc-deck-engineering.md")), "{sections:?}");
    assert!(!sections.iter().any(|s| s.contains("echo-style.md")), "工人不该带风格：{sections:?}");
}

/// 看着像路径却不存在 —— 明确拒绝，而不是把这串字符当问题发给模型。
#[test]
fn a_missing_task_book_path_is_refused_instead_of_sent_as_a_prompt() {
    let case = RunCase::new();
    let run = case.run(&["notes.md"]);
    assert_eq!(run.code, 64, "stdout={} stderr={}", run.stdout, run.stderr);
    assert!(run.stderr.contains("找不到任务书"), "stderr={}", run.stderr);
    assert_eq!(run.stdout.trim(), "", "拒绝时不该有半句回答");
}

/// 派工握手在**成功**路径上也要成立：工人接住了才算派工成功。
#[test]
fn detached_run_against_a_working_fixture_reaches_completed() {
    let case = RunCase::new();
    let run = case.run(&["另一件活儿", "--id", "d1", "--by", "claude", "--detach"]);
    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    assert!(run.stdout.contains("started"), "stdout={}", run.stdout);

    // 握手返回时状态文件已经在了 —— 这正是它对「分离不等于成功」的保证。
    let status: Value = serde_json::from_str(
        &std::fs::read_to_string(case.state().join("runs/d1/status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(status["assigned_by"], "claude");
    assert!(status["state"].is_string());
}

/// `--worktree` 配非全权沙箱时必须在**派工那一秒**就警告。
///
/// 实测（Arch，2026-10-14，真实模型）：worktree 里的每条 git 命令都返回 128
/// （worktree 的 `.git` 是个指向主仓的指针文件，主仓在 workspace 之外），
/// 整轮以「验收未通过 accept-1（退出 128）」收场 —— 工人跑了 34 秒、烧了
/// 8342 输入 token 才发现一件派工时就知道的事。
#[test]
fn worktree_without_full_sandbox_warns_at_dispatch_time() {
    let case = RunCase::new();
    // 得先有个 git 仓，`--worktree` 才有东西可建。
    let repo = case.dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    for args in [vec!["init", "-q"], vec!["config", "user.email", "t@t"], vec!["config", "user.name", "t"]] {
        let ok = Command::new("git").args(&args).current_dir(&repo).status().unwrap().success();
        assert!(ok, "git {args:?} 失败");
    }
    std::fs::write(repo.join("README.md"), "hi\n").unwrap();
    assert!(Command::new("git").args(["add", "-A"]).current_dir(&repo).status().unwrap().success());
    assert!(Command::new("git")
        .args(["commit", "-qm", "init"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    // `--worktree` 的基线缺省是 `main`（PRODUCT_PLAN §6.6）。
    assert!(Command::new("git").args(["branch", "-M", "main"]).current_dir(&repo).status().unwrap().success());

    let run = case.run_in(&repo, &["顺手改一下 README", "--id", "wt-warn", "--worktree", "dsh/wt-warn"]);
    assert!(
        run.stderr.contains("worktree 里的 git 命令会全部失败"),
        "派工时没有警告：stderr={}",
        run.stderr
    );
}

/// 授权级别是**产品层的唯一说法**：命令行显式划定，映射到策略档与 DSH 沙箱档。
/// 一次划定，运行中不再二次修改（headless 没有交互入口）。
///
/// 这里只断言**策略档**：夹具模式下物化的 `runtime` 是 `fixture-sdk`，没有 `sandbox` 字段
/// （假内核不碰沙箱）。级别到沙箱档的映射由 `authz` 的单测钉住。
#[test]
fn permission_flag_decides_the_profile() {
    let case = RunCase::new();
    let profile = |id: &str, level: &str| -> String {
        let run = case.run(&["看一眼", "--id", id, "--permission", level]);
        assert_eq!(run.code, 0, "stderr={}", run.stderr);
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(case.state().join(format!("runs/{id}/config.json"))).unwrap(),
        )
        .unwrap();
        config["authorization"]["profile"].as_str().unwrap_or_default().to_string()
    };
    assert_eq!(profile("p-ro", "readonly"), "workspace-readonly");
    assert_eq!(profile("p-ws", "workspace"), "workspace-development");
    assert_eq!(profile("p-full", "full"), "workspace-development");
}

/// `confirm` 需要一个应答者，headless 没有 —— **明确拒绝**，不静默降级成 `workspace`。
#[test]
fn confirm_permission_is_refused_in_headless() {
    let case = RunCase::new();
    let run = case.run(&["看一眼", "--id", "p-confirm", "--permission", "confirm"]);
    assert_eq!(run.code, 64, "stdout={} stderr={}", run.stdout, run.stderr);
    assert!(run.stderr.contains("应答者"), "stderr={}", run.stderr);
    assert!(
        !case.state().join("runs/p-confirm").exists(),
        "拒绝要发生在建立任何状态之前"
    );
}

/// 模型强度的**产品名是 `effort`**（opencode 叫 variant，不用那个名）。
///
/// 这里只证明它是**被认识的参数**且不影响运行；「它真的到了 provider」由真实运行验证过
/// （把一个不存在的级别传下去，运行时在 `initialize` 直接拒绝 —— 见 §10.2b）。
/// 夹具模式下物化配置没有 `model`（假内核不碰模型），所以断言不了路由。
#[test]
fn effort_is_a_known_flag_and_does_not_break_the_run() {
    let case = RunCase::new();
    let run = case.run(&["看一眼", "--id", "e1", "--effort", "high"]);
    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    assert!(
        !run.stderr.contains("不认识的参数"),
        "--effort 没被声明：{}",
        run.stderr
    );
}

/// `arcdeck usage` 把 provider 报告的用量加起来；**没有费用**。
#[test]
fn usage_sums_what_the_provider_reported() {
    let case = RunCase::new();
    assert_eq!(case.run(&["看一眼", "--id", "u1"]).code, 0);

    let mut command = Command::new(binary());
    let out = command
        .arg("usage")
        .arg("--state-root")
        .arg(case.state())
        .output()
        .expect("usage");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    // 夹具报的是 11 输入 / 2 输出。
    assert!(text.contains("输入 11"), "{text}");
    assert!(text.contains("输出 2"), "{text}");
    assert!(text.contains("没有费用估算"), "要说清不编费用：{text}");
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
