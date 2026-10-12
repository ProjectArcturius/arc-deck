//! 事件投影：把会话事件折成产品能直接消费的轮次视图。
//!
//! 这一层承担三条容易出错的判断，所以它独立于任何界面：
//!
//! 1. **助手的正文会出现两次**。流式增量用于观感，权威全文用于内容；
//!    两者都用会让正文写两遍。这里保留 `authoritative` 标记，取值时只取一个。
//! 2. **只有 `source.kind == "user"` 的 `user/message` 才是人说的话**。
//!    内核会把系统提示词、工作区指令、技能清单都写成 `user/message`，
//!    全画出来对话会被几百行英文淹没。判据是标记，不是长度。
//! 3. **工具调用与结果按 `call_id` 配对**。孤儿不是零容忍的判断 ——
//!    真实日志里稳定存在少量没有结果的调用，所以这里只**计数**，由调用方定阈值。
//!
//! 事件词汇取自 `REFS/deepseek-harness`（提交 `d7432673`）的 `SessionEventMap`。
//! 假 SDK 对端按同一形状发事件；M2 接真实内核时，对不上的地方改这里而不是改界面。

use std::collections::BTreeSet;

use serde_json::Value;

use crate::protocol::SessionEventParams;
use arc_deck_core::result::Usage;

/// 一个工具调用的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    pub is_error: bool,
    pub exit_code: Option<i32>,
    pub text: String,
}

/// 一次工具调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolView {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
    pub outcome: Option<ToolOutcome>,
}

impl ToolView {
    /// 命令执行类工具才有。参数里的 `command` 字段。
    pub fn command(&self) -> Option<&str> {
        self.arguments.get("command").and_then(|c| c.as_str())
    }

    pub fn cwd(&self) -> Option<&str> {
        self.arguments.get("cwd").and_then(|c| c.as_str())
    }
}

/// 一轮里助手输出的视图。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssistantView {
    /// 是否已经收到权威全文。收到之后流式增量不再参与最终取值。
    pub authoritative: bool,
    pub authoritative_text: String,
    pub streamed_text: String,
    pub steps: BTreeSet<u32>,
}

impl AssistantView {
    /// 该轮对外的正文。**只取一个来源**，这就是「正文不写两遍」的全部实现。
    pub fn text(&self) -> &str {
        if self.authoritative { &self.authoritative_text } else { &self.streamed_text }
    }

    pub fn is_empty(&self) -> bool {
        self.text().trim().is_empty()
    }
}

/// 一轮的结束原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnd {
    /// `completed` / `aborted` / `blocked` / `error` / `max-tokens` / `interrupted` / `forked`。
    pub kind: String,
    pub message: Option<String>,
}

/// 一轮。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnView {
    pub turn: u32,
    pub user_messages: Vec<String>,
    pub assistant: AssistantView,
    pub tools: Vec<ToolView>,
    pub end: Option<TurnEnd>,
}

/// 投影结果。
#[derive(Debug, Clone, Default)]
pub struct Projection {
    turns: Vec<TurnView>,
    /// 内部簿记。**不是**对话内容 —— 画出来会让真正的正文被埋掉。
    recorded: Vec<Value>,
    pub orphans: usize,
    /// 模型失败的唯一出口。可能来自 `finish.reason.failure.message`，
    /// 也可能来自轮次级 `reason.error.message`。
    pub failures: Vec<String>,
    pub attempts: usize,
    pub session_id: Option<String>,
    pub usage: Option<Usage>,
}

impl Projection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn turns(&self) -> &[TurnView] {
        &self.turns
    }

    pub fn recorded(&self) -> &[Value] {
        &self.recorded
    }

    /// 最后出现的轮次结束原因。收尾判定用这个，而不是「有没有收到过事件」——
    /// 收到一堆工具事件但没等到 `turn/end`，仍然是「没有 terminal」。
    pub fn last_end(&self) -> Option<&TurnEnd> {
        self.turns.iter().rev().find_map(|t| t.end.as_ref())
    }

    /// 全部轮次的助手正文，按顺序拼接。测试与结果摘要用。
    pub fn assistant_text(&self) -> String {
        let mut out = String::new();
        for turn in &self.turns {
            let text = turn.assistant.text();
            if text.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
        out
    }

    /// 应用一帧 `session/event` 通知。
    pub fn apply_notification(&mut self, params: &SessionEventParams) {
        self.session_id = Some(params.session_id.clone());
        self.apply(&params.event);
    }

    /// 应用一帧事件本体。
    ///
    /// `recorded` 的写入统一在这里发生 —— 让两个入口各写一半，迟早会出现
    /// 「走通知进来有记录、直接调用没记录」这种只有测试看得见的不一致。
    pub fn apply(&mut self, event: &Value) {
        let Some(kind) = event.get("type").and_then(|t| t.as_str()) else {
            self.recorded.push(event.clone());
            return;
        };
        let turn_no = event.get("turn").and_then(|t| t.as_u64()).map(|t| t as u32);
        let step_no = event.get("step").and_then(|t| t.as_u64()).map(|t| t as u32);
        let recognized = matches!(
            kind,
            "turn/start"
                | "turn/end"
                | "user/message"
                | "assistant/delta"
                | "assistant/message"
                | "assistant/attempt"
                | "tool/call"
                | "tool/result"
        );
        if !recognized {
            // 内部簿记（permission/preset、sandbox/mode、request/context …）。
            // 画出来会让真正的正文被埋掉，所以只留档、不进对话。
            self.recorded.push(event.clone());
        }

        // 模型失败与尝试数量与轮次无关，先扫一遍。
        scan_failures(event, &mut self.failures);

        match kind {
            "turn/start" => {
                if let Some(turn) = turn_no {
                    self.ensure_turn(turn);
                }
            }
            "turn/end" => {
                let reason = event.get("reason").cloned().unwrap_or(Value::Null);
                let kind = reason
                    .get("kind")
                    .and_then(|k| k.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let message = reason
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .or_else(|| reason.get("message").and_then(|m| m.as_str()))
                    .map(str::to_string);
                if let Some(turn) = turn_no {
                    let view = self.ensure_turn(turn);
                    view.end = Some(TurnEnd { kind, message });
                }
            }
            "user/message" => {
                let is_user = event
                    .get("source")
                    .and_then(|s| s.get("kind"))
                    .and_then(|k| k.as_str())
                    == Some("user");
                let text = collect_message_text(event.get("message").unwrap_or(event));
                if !is_user {
                    // 系统提示词、工作区指令、技能清单都走这条路，不进对话。
                    return;
                }
                if let Some(turn) = turn_no {
                    let text = text.unwrap_or_default();
                    self.ensure_turn(turn).user_messages.push(text);
                }
            }
            "assistant/delta" => {
                // 假 SDK 对端对真实 `assistant-stream` 帧的替身；M2 按真实形状改这里。
                if let (Some(turn), Some(step)) = (turn_no, step_no) {
                    let text = event
                        .get("chunk")
                        .and_then(|c| c.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    let view = self.ensure_turn(turn);
                    view.assistant.steps.insert(step);
                    view.assistant.streamed_text.push_str(text);
                }
            }
            "assistant/message" => {
                if let (Some(turn), Some(step)) = (turn_no, step_no) {
                    let text = collect_message_text(event.get("message").unwrap_or(event))
                        .unwrap_or_default();
                    let view = self.ensure_turn(turn);
                    view.assistant.steps.insert(step);
                    view.assistant.authoritative = true;
                    view.assistant.authoritative_text = text;
                    if let Some(usage) = event.get("usage") {
                        self.usage = Some(Usage {
                            prompt_tokens: usage.get("promptTokens").and_then(|v| v.as_u64()),
                            completion_tokens: usage.get("completionTokens").and_then(|v| v.as_u64()),
                            total_tokens: usage.get("totalTokens").and_then(|v| v.as_u64()),
                        });
                    }
                }
            }
            "assistant/attempt" => {
                self.attempts += 1;
            }
            "tool/call" => {
                if let Some(turn) = turn_no {
                    let call_id = event
                        .get("callId")
                        .and_then(|c| c.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = event.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
                    // arguments 在真实协议里是**未解析**的 JSON 字符串。
                    let arguments = match event.get("arguments").and_then(|a| a.as_str()) {
                        Some(raw) => serde_json::from_str(raw).unwrap_or(Value::String(raw.to_string())),
                        None => event.get("arguments").cloned().unwrap_or(Value::Null),
                    };
                    self.ensure_turn(turn).tools.push(ToolView {
                        call_id,
                        name,
                        arguments,
                        outcome: None,
                    });
                }
            }
            "tool/result" => {
                let call_id = event.get("callId").and_then(|c| c.as_str()).unwrap_or_default();
                let message = event.get("message").cloned().unwrap_or(Value::Null);
                let text = collect_message_text(&message).unwrap_or_default();
                let exit_code = event
                    .get("meta")
                    .and_then(|m| m.get("exitCode"))
                    .and_then(|c| c.as_i64())
                    .map(|c| c as i32);
                let outcome = ToolOutcome {
                    is_error: message.get("isError").and_then(|e| e.as_bool()).unwrap_or(false),
                    exit_code,
                    text,
                };
                let mut paired = false;
                for turn in self.turns.iter_mut().rev() {
                    if let Some(tool) = turn.tools.iter_mut().rev().find(|t| t.call_id == call_id) {
                        tool.outcome = Some(outcome.clone());
                        paired = true;
                        break;
                    }
                }
                if !paired {
                    // 真实日志里稳定存在少量没有结果的调用（也就会有反过来的情况），
                    // 所以这里只计数，不当错误抛。
                    self.orphans += 1;
                }
            }
            _ => {}
        }
    }

    fn ensure_turn(&mut self, turn: u32) -> &mut TurnView {
        if let Some(idx) = self.turns.iter().position(|t| t.turn == turn) {
            return &mut self.turns[idx];
        }
        self.turns.push(TurnView {
            turn,
            user_messages: Vec::new(),
            assistant: AssistantView::default(),
            tools: Vec::new(),
            end: None,
        });
        self.turns.last_mut().expect("刚 push 过")
    }
}

/// 从消息对象里取纯文本。`content` 是块数组，只拼接 `type == "text"` 的块。
fn collect_message_text(message: &Value) -> Option<String> {
    if let Some(text) = message.get("text").and_then(|t| t.as_str()) {
        return Some(text.to_string());
    }
    let content = message.get("content")?.as_array()?;
    let mut out = String::new();
    for block in content {
        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                out.push_str(text);
            }
        }
    }
    Some(out)
}

/// 扫出模型失败的原文。
///
/// 模型失败在协议里**不会**抛错，只会出现在 `finish.reason.failure.message` 里；
/// 界面上表现为「消息发出去以后什么都没有」。所以这条扫描是必需的，
/// 不是锦上添花。
fn scan_failures(value: &Value, out: &mut Vec<String>) {
    if let Some(stream) = value.get("stream").and_then(|s| s.as_array()) {
        for entry in stream {
            if let Some(message) = entry
                .get("chunk")
                .and_then(|c| c.get("finish"))
                .and_then(|f| f.get("reason"))
                .and_then(|r| r.get("failure"))
                .and_then(|f| f.get("message"))
                .and_then(|m| m.as_str())
            {
                out.push(message.to_string());
            }
        }
    }
    if let Some(message) = value
        .get("reason")
        .and_then(|r| r.get("error"))
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        out.push(message.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn authoritative_text_replaces_streamed_text() {
        let mut p = Projection::new();
        p.apply(&json!({"type":"turn/start","turn":1}));
        p.apply(&json!({"type":"assistant/delta","turn":1,"step":1,"chunk":{"type":"text-delta","text":"收"}}));
        p.apply(&json!({"type":"assistant/delta","turn":1,"step":1,"chunk":{"type":"text-delta","text":"到"}}));
        p.apply(&json!({
            "type":"assistant/message","turn":1,"step":1,
            "message":{"content":[{"type":"text","text":"收到"}]}
        }));

        let turn = &p.turns()[0];
        assert!(turn.assistant.authoritative);
        assert_eq!(turn.assistant.text(), "收到");
        // 关键负例：把两个来源都算进去会得到「收到收到」。
        let naive = format!("{}{}", turn.assistant.streamed_text, turn.assistant.authoritative_text);
        assert_eq!(naive, "收到收到");
        assert_ne!(turn.assistant.text(), naive);
    }

    #[test]
    fn streamed_text_is_used_until_authoritative_arrives() {
        let mut p = Projection::new();
        p.apply(&json!({"type":"assistant/delta","turn":1,"step":1,"chunk":{"text":"半"}}));
        assert_eq!(p.turns()[0].assistant.text(), "半");
    }

    #[test]
    fn non_user_messages_do_not_enter_the_conversation() {
        let mut p = Projection::new();
        p.apply(&json!({
            "type":"user/message","turn":1,
            "source":{"kind":"system"},
            "message":{"content":[{"type":"text","text":"你是……（几百行系统提示词）"}]}
        }));
        p.apply(&json!({
            "type":"user/message","turn":1,
            "source":{"kind":"user"},
            "message":{"content":[{"type":"text","text":"把筛选修好"}]}
        }));
        assert_eq!(p.turns()[0].user_messages, vec!["把筛选修好".to_string()]);
    }

    #[test]
    fn tools_pair_by_call_id_and_orphans_are_counted() {
        let mut p = Projection::new();
        p.apply(&json!({"type":"turn/start","turn":1}));
        p.apply(&json!({
            "type":"tool/call","turn":1,"step":1,"callId":"c1","name":"shell",
            "arguments":"{\"command\":\"cargo test\",\"cwd\":\".\"}"
        }));
        p.apply(&json!({
            "type":"tool/result","turn":1,"step":1,"callId":"c1",
            "message":{"isError":false,"content":[{"type":"text","text":"ok"}]},
            "meta":{"exitCode":0}
        }));
        p.apply(&json!({
            "type":"tool/result","turn":1,"step":1,"callId":"c-missing",
            "message":{"isError":false,"content":[{"type":"text","text":"?"}]}
        }));

        let tool = &p.turns()[0].tools[0];
        assert_eq!(tool.call_id, "c1");
        assert_eq!(tool.name, "shell");
        assert_eq!(tool.command(), Some("cargo test"));
        assert_eq!(tool.cwd(), Some("."));
        assert_eq!(tool.outcome.as_ref().unwrap().exit_code, Some(0));
        assert!(!tool.outcome.as_ref().unwrap().is_error);
        assert_eq!(p.orphans, 1);
    }

    #[test]
    fn turn_end_reason_is_extracted() {
        let mut p = Projection::new();
        p.apply(&json!({"type":"turn/start","turn":1}));
        p.apply(&json!({"type":"turn/end","turn":1,"reason":{"kind":"error","error":{"message":"模型炸了"}}}));
        let end = p.last_end().unwrap();
        assert_eq!(end.kind, "error");
        assert_eq!(end.message.as_deref(), Some("模型炸了"));
        assert_eq!(p.failures, vec!["模型炸了".to_string()]);
    }

    #[test]
    fn finish_failure_inside_stream_is_collected() {
        let mut p = Projection::new();
        p.apply(&json!({
            "type":"assistant/attempt","turn":1,"step":1,
            "stream":[{"chunk":{"finish":{"reason":{"failure":{"message":"The supported API model names are deepseek-flash"}}}}}]
        }));
        assert_eq!(p.attempts, 1);
        assert!(p.failures[0].contains("The supported API model names"));
    }

    #[test]
    fn turns_are_merged_not_one_message_per_step() {
        let mut p = Projection::new();
        for step in 1..=5u32 {
            p.apply(&json!({"type":"step/start","turn":1,"step":step}));
            p.apply(&json!({
                "type":"assistant/message","turn":1,"step":step,
                "message":{"content":[{"type":"text","text":"x"}]}
            }));
        }
        assert_eq!(p.turns().len(), 1, "一轮里的多个 step 不该变成多条消息");
        assert_eq!(p.turns()[0].assistant.steps.len(), 5);
    }

    #[test]
    fn unknown_events_are_recorded_not_messaged() {
        let mut p = Projection::new();
        p.apply(&json!({"type":"permission/preset","preset":"sdk"}));
        p.apply(&json!({"not_even_an_object_with_type": 1}));
        assert!(p.turns().is_empty());
        assert_eq!(p.recorded().len(), 2);
    }

    #[test]
    fn usage_is_picked_up_only_when_reported() {
        let mut p = Projection::new();
        p.apply(&json!({"type":"assistant/message","turn":1,"step":1,"message":{"content":[]}}));
        assert!(p.usage.is_none());

        p.apply(&json!({
            "type":"assistant/message","turn":1,"step":2,"message":{"content":[]},
            "usage":{"promptTokens":10,"completionTokens":4,"totalTokens":14}
        }));
        let usage = p.usage.unwrap();
        assert_eq!(usage.total_tokens, Some(14));
        assert_eq!(usage.prompt_tokens, Some(10));
    }
}
