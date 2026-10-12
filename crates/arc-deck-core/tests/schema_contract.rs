//! 公开契约的漂移防线。
//!
//! 单源是 Rust 类型本身（参 `schemas/README.md`），不是另一份手写的 schema。
//! 这个测试做两件事：
//!
//! 1. `schemas/*.example.json` 必须能被对应的 Rust 类型**解析**再**重新序列化**，
//!    并保持语义相等。字段改名、加必填字段、改枚举取值都会让它失败。
//! 2. 关键字段名逐个钉住 —— 改名时失败信息会直接指出是哪一个。
//!
//! 这条路子刻意不引入 JSON Schema 校验器：多一份 schema 就多一处会漂移的副本，
//! 而这里要防的正是「类型改了、文档没改」。

use std::path::{Path, PathBuf};

use arc_deck_core::agent::{AgentDefinition, AgentRegistry};
use arc_deck_core::config::WorkerConfig;
use arc_deck_core::event::EventEnvelope;
use arc_deck_core::mode::{AgentId, Mode};
use arc_deck_core::result::{TaskResult, TaskStatus, VerificationOutcome};
use arc_deck_core::task::{Acceptance, TaskSpec};
use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read_example(name: &str) -> String {
    let path = repo_root().join("schemas").join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读不到契约示例 {}：{e}", path.display()))
}

/// 重新序列化后与原值语义相等。
fn assert_round_trip<T>(original: &Value, value: &T, what: &str)
where
    T: serde::Serialize,
{
    let again = serde_json::to_value(value).expect("重新序列化");
    // `#[serde(skip)]` 的字段（workspace）不会出现在两边，比较时忽略键顺序。
    assert_eq!(
        normalize(&again),
        normalize(original),
        "{what} 往返后不再相等：类型的字段名或形状已经和契约示例不一致"
    );
}

/// `Value` 的 `PartialEq` 已经忽略对象键顺序，这里只用来显式表达意图。
fn normalize(value: &Value) -> Value {
    value.clone()
}

#[test]
fn task_example_matches_the_task_spec() {
    let raw = read_example("task.example.json");
    let original: Value = serde_json::from_str(&raw).unwrap();
    let task = TaskSpec::parse(&raw, Path::new("C:/work/project")).unwrap();

    assert_eq!(task.schema_version, 1);
    assert_eq!(task.task_id, "fix-session-filter");
    assert_eq!(task.agent.as_deref(), Some("echo"));
    assert_eq!(task.mode, Some(Mode::Build));
    assert_eq!(task.timeout_seconds, Some(900));
    assert_eq!(task.acceptance.len(), 2);
    assert!(task.acceptance[0].is_command());
    assert!(!task.acceptance[1].is_command());

    // 相对 workspace 已解析成绝对路径；序列化时跳过，所以两边一致。
    let mut expected = original.clone();
    expected["workspace"] = Value::String(task.workspace.display().to_string());
    assert_round_trip(&expected, &task, "TaskSpec");
}

#[test]
fn config_example_matches_the_worker_config() {
    let raw = read_example("config.example.json");
    let original: Value = serde_json::from_str(&raw).unwrap();
    let config = WorkerConfig::parse(&raw, Path::new("C:/arc-deck")).unwrap();

    assert_eq!(config.authorization.profile, "workspace-readonly");
    assert_eq!(config.authorization.allowed_agents.len(), 3);
    assert_eq!(config.model.as_ref().unwrap().model, "deepseek-flash");
    assert_eq!(config.timeouts.initialize_seconds, 120);

    let mut expected = original.clone();
    expected["state_root"] = Value::String(config.state_root.display().to_string());
    expected["authorization"]["workspace"] =
        Value::String(config.authorization.workspace.display().to_string());
    expected["agents"]["dir"] = Value::String(config.agents.dir.display().to_string());
    expected["agents"]["prompts_dir"] =
        Value::String(config.agents.prompts_dir.display().to_string());
    // 入口是绝对路径；相对入口才会被重新解析，这里本来就是绝对的。
    assert_round_trip(&expected, &config, "WorkerConfig");
}

#[test]
fn result_example_matches_the_task_result() {
    let raw = read_example("result.example.json");
    let original: Value = serde_json::from_str(&raw).unwrap();
    let result: TaskResult = serde_json::from_str(&raw).unwrap();

    assert_eq!(result.status, TaskStatus::Completed);
    assert_eq!(result.verification[0].outcome, VerificationOutcome::Passed);
    assert_eq!(result.verification[0].exit_code, Some(0));
    assert_eq!(result.usage.as_ref().unwrap().total_tokens, Some(13));
    assert_eq!(result.policy.as_ref().unwrap().agent, AgentId::Echo);

    assert_round_trip(&original, &result, "TaskResult");
}

#[test]
fn event_example_matches_the_event_envelope() {
    let raw = read_example("event.example.jsonl");
    let mut seqs = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let original: Value = serde_json::from_str(line).unwrap();
        let envelope: EventEnvelope = serde_json::from_str(line).unwrap();
        assert_eq!(envelope.schema_version, 1);
        assert_eq!(envelope.task_id, "example-task");
        assert!(original["data"].is_object());
        seqs.push(envelope.seq);
        assert_round_trip(&original, &envelope, "EventEnvelope");
    }
    // seq 必须是 1..n 的连续序列，示例本身也要守这条。
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
}

#[test]
fn example_task_parses_across_every_acceptance_shape() {
    let raw = read_example("task.example.json");
    let task = TaskSpec::parse(&raw, Path::new("C:/work/project")).unwrap();
    match &task.acceptance[0] {
        Acceptance::Command { id, .. } => assert_eq!(id, "filter-regression"),
        other => panic!("第一项应该是 command：{other:?}"),
    }
    match &task.acceptance[1] {
        Acceptance::Manual { id, .. } => assert_eq!(id, "human-look"),
        other => panic!("第二项应该是 manual：{other:?}"),
    }
}

#[test]
fn repo_agents_are_loadable_and_carry_the_three_expected_ids() {
    // Agent 定义是产品契约的一部分：三个 id 与它们的工具集合都写在这里。
    let root = repo_root();
    let registry = AgentRegistry::load(&root.join("agents"), &root.join("prompts")).unwrap();
    let ids = registry.ids();
    assert!(ids.contains(&AgentId::Echo));
    assert!(ids.contains(&AgentId::Build));
    assert!(ids.contains(&AgentId::Plan));
    assert_eq!(ids.len(), 3, "首版只有 echo / build / plan：{ids:?}");

    let echo = registry.definition(AgentId::Echo).unwrap();
    assert_eq!(echo.default_mode, Mode::Build);
    assert!(echo.style.is_some(), "Echo 必须挂风格文件");
    // Plan 与 Build 的工具集合必须不同 —— 否则「Plan 只读」就只是提示词。
    let plan = registry.definition(AgentId::Plan).unwrap();
    assert!(!plan.tools.contains(&"edit".to_string()), "Plan 不该有 edit");
    assert!(!plan.tools.contains(&"shell".to_string()), "Plan 不该有 shell");
}

#[test]
fn agent_definition_rejects_an_unknown_tool_by_name() {
    let raw = "---\nschema_version: 1\nid: echo\ndescription: d\nrole: primary\n\
               default_mode: build\ntools: read, telepathy\npermission_profile: workspace-readonly\n---\n\n正文。\n";
    let err = AgentDefinition::parse(raw, Path::new("echo.md")).unwrap_err();
    assert!(err.code.contains("AGENT_UNKNOWN_TOOL"), "{err}");
}
