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
    /// 上游给的错误分类与 HTTP 状态码（如 `code: "QUOTA"`、`status: 402`）。
    ///
    /// **比猜英文原文硬得多**：真实会话里这条就是
    /// `{"kind":"error","error":{"message":"Insufficient Balance","code":"QUOTA","status":402}}`。
    /// 分类优先用它，认不出才退回子串匹配（参 `arc_deck_core::failure`）。
    pub code: Option<String>,
    pub status: Option<i64>,
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
    /// 被后来的 `turn/end` 覆盖掉的结束原因个数。续跑开的新会话也从第 1 轮数起，
    /// 会并进同一个轮次视图；调用方靠它判断「这一次运行结束了没有」。
    pub ends_overwritten: usize,
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
        let flat = flatten(event);
        let event = &flat;
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
                // 结构化错误字段：上游在 `reason.error` 里给了分类与状态码。
                let error = reason.get("error");
                let code = error
                    .and_then(|e| e.get("code"))
                    .and_then(|c| c.as_str())
                    .map(str::to_string);
                let status = error.and_then(|e| e.get("status")).and_then(|s| s.as_i64());
                if let Some(turn) = turn_no {
                    let view = self.ensure_turn(turn);
                    let overwritten = view.end.is_some();
                    view.end = Some(TurnEnd { kind, message, code, status });
                    if overwritten {
                        self.ends_overwritten += 1;
                    }
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
                // 真实 DSH 的 `user/message` 不带 `turn`（它属于当前打开的那一轮，
                // 0.2.1-alpha.2 实测）；夹具带。没有就记到最近一轮上。
                let turn = turn_no.or_else(|| self.turns.last().map(|t| t.turn));
                if let Some(turn) = turn {
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
                        // 真实 DSH 每一步各带一份用量（`inputTokens`/`outputTokens`/`cacheReadTokens`），
                        // 整个任务的用量是逐步相加；`promptTokens` 等是夹具的旧拼写，一并认。
                        let field = |a: &str, b: &str| {
                            usage.get(a).or_else(|| usage.get(b)).and_then(|v| v.as_u64())
                        };
                        let add = |acc: Option<u64>, v: Option<u64>| match (acc, v) {
                            (None, None) => None,
                            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
                        };
                        let prev = self.usage.clone().unwrap_or_default();
                        self.usage = Some(Usage {
                            prompt_tokens: add(prev.prompt_tokens, field("inputTokens", "promptTokens")),
                            completion_tokens: add(prev.completion_tokens, field("outputTokens", "completionTokens")),
                            total_tokens: add(prev.total_tokens, field("totalTokens", "totalTokens")),
                            cache_read_tokens: add(prev.cache_read_tokens, field("cacheReadTokens", "cacheReadTokens")),
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
                let message = event.get("message").cloned().unwrap_or(Value::Null);
                // 真实 DSH 把配对 id 放在 `message.toolCallId`；夹具放在顶层 `callId`。
                let call_id = event
                    .get("callId")
                    .and_then(|c| c.as_str())
                    .or_else(|| message.get("toolCallId").and_then(|c| c.as_str()))
                    .unwrap_or_default()
                    .to_string();
                let call_id = call_id.as_str();
                let text = collect_message_text(&message).unwrap_or_default();
                // 退出码：`meta.exitCode` 优先；真实 DSH 的 shell 工具把它写成正文里的
                // `[exit code: N]` 标记（0.2.1-alpha.2 实测，meta 里只有 cwd）。
                let exit_code = event
                    .get("meta")
                    .and_then(|m| m.get("exitCode"))
                    .and_then(|c| c.as_i64())
                    .map(|c| c as i32)
                    .or_else(|| exit_code_marker(&text));
                let outcome = ToolOutcome {
                    is_error: message.get("isError").and_then(|e| e.as_bool()).unwrap_or(false),
                    exit_code,
                    text,
                };
                let mut paired = false;
                for turn in self.turns.iter_mut().rev() {
                    if let Some(tool) = turn.tools.iter_mut().rev().find(|t| t.call_id == call_id) {
                        let mut outcome = outcome.clone();
                        // DSH 的 shell 工具**只为非零退出**写 `[exit code: N]` 标记
                        // （tool-pwsh / tool-bash 的提示词原文：「Non-zero exits are reported as
                        // `[exit code: N]` markers」）。所以命令类调用没有标记、也没报错，就是 0。
                        // 2026-10-14 实测：不这么读，每条成功的验收命令都被判成「无法判定」。
                        if outcome.exit_code.is_none() && !outcome.is_error && tool.command().is_some() {
                            outcome.exit_code = Some(0);
                        }
                        tool.outcome = Some(outcome);
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

/// 真实 DSH 的事件形如 `{type, seq, time, data:{…}}`；夹具与 M1 测试用扁平形状。
/// 统一摊平成扁平形状再读，免得每个分支各自判断两种拼法。
fn flatten(event: &Value) -> Value {
    match (event.get("data").and_then(|d| d.as_object()), event.as_object()) {
        (Some(data), Some(outer)) => {
            let mut merged = data.clone();
            for key in ["type", "seq", "time"] {
                if let Some(v) = outer.get(key) {
                    merged.insert(key.to_string(), v.clone());
                }
            }
            Value::Object(merged)
        }
        _ => event.clone(),
    }
}

/// 取最后一个 `[exit code: N]` 标记。没有标记表示 0（DSH 只为非零退出写标记），
/// 但「没有标记」也可能是这根本不是 shell 工具 —— 所以无标记返回 None，由调用方按工具名判断。
pub fn exit_code_marker(text: &str) -> Option<i32> {
    let idx = text.rfind("[exit code: ")?;
    let rest = &text[idx + "[exit code: ".len()..];
    let end = rest.find(']')?;
    rest[..end].trim().parse().ok()
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

/// 一帧事件里带的模型失败原文（可能不止一条）。
///
/// 投影与 `StatusWriter::observe` 共用同一条扫描规则 —— 两处各写一遍的话，
/// 「什么算模型失败」会在实时与重放两条路上慢慢漂开。
pub fn failure_texts(event: &Value) -> Vec<String> {
    let mut out = Vec::new();
    scan_failures(event, &mut out);
    out
}

/// 扫出模型失败的原文。
///
/// 模型失败在协议里**不会**抛错，只会出现在 `finish.reason.failure.message` 里；
/// 界面上表现为「消息发出去以后什么都没有」。所以这条扫描是必需的，
/// 不是锦上添花。
fn scan_failures(value: &Value, out: &mut Vec<String>) {
    if let Some(stream) = value.get("stream").and_then(|s| s.as_array()) {
        for entry in stream {
            let Some(chunk) = entry.get("chunk") else { continue };
            // 两种拼法：夹具是 `chunk.finish.reason`，真实 DSH 是 `chunk:{type:"finish", reason}`。
            let reason = chunk
                .get("finish")
                .and_then(|f| f.get("reason"))
                .or_else(|| {
                    (chunk.get("type").and_then(|t| t.as_str()) == Some("finish"))
                        .then(|| chunk.get("reason"))
                        .flatten()
                });
            if let Some(message) = reason
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

    /// 2026-10-14 真实 DSH 0.2.1-alpha.2（经 OpenCode Go 的 deepseek-v4.1-flash）发出的事件，
    /// 去掉了 stream、路径与提示词。M1 的投影按扁平字段读，读这一组会什么都看不见 ——
    /// 这条测试就是钉住「字段在 data 里、配对 id 在 message.toolCallId、退出码在正文标记里」。
    #[test]
    fn real_dsh_0_2_1_events_are_projected() {
        let events = [
            json!({"type":"turn/start","seq":4,"time":1,"data":{"turn":1}}),
            json!({"type":"user/message","seq":8,"time":1,"data":{"content":[{"type":"text","text":"Run: cmd /c exit 3"}],"source":{"kind":"user"},"role":"user","id":"u1"}}),
            json!({"type":"tool/call","seq":15,"time":2,"data":{"turn":1,"step":1,"callId":"call_00_S","name":"pwsh","arguments":"{\"description\":\"Run cmd exit 3\",\"command\":\"cmd /c exit 3\"}"}}),
            json!({"type":"tool/result","seq":16,"time":3,"data":{"turn":1,"step":1,"message":{"role":"tool","source":{"kind":"tool","callId":"call_00_S"},"toolCallId":"call_00_S","content":[{"type":"text","text":"(no output)\n[exit code: 1]"}],"isError":false,"id":"r1"},"meta":{"cwd":"C:/ws"}}}),
            json!({"type":"assistant/message","seq":19,"time":4,"data":{"turn":1,"step":2,"message":{"role":"assistant","content":[{"type":"reasoning","text":"…"},{"type":"text","text":"done"}]},"stream":[],"usage":{"inputTokens":229,"outputTokens":113,"totalTokens":6742,"cacheReadTokens":6400}}}),
            json!({"type":"turn/end","seq":21,"time":5,"data":{"turn":1,"reason":{"kind":"completed"}}}),
        ];
        let mut p = Projection::new();
        for e in &events {
            p.apply(e);
        }
        let turn = &p.turns()[0];
        assert_eq!(turn.user_messages, vec!["Run: cmd /c exit 3".to_string()]);
        assert_eq!(turn.tools.len(), 1);
        assert_eq!(turn.tools[0].command(), Some("cmd /c exit 3"));
        let outcome = turn.tools[0].outcome.as_ref().expect("结果必须配对上");
        assert_eq!(outcome.exit_code, Some(1));

        // 成功的命令没有标记：读成 0，而不是「没有退出码」。
        let mut q = Projection::new();
        q.apply(&json!({"type":"turn/start","seq":1,"time":1,"data":{"turn":1}}));
        q.apply(&json!({"type":"tool/call","seq":2,"time":1,"data":{"turn":1,"step":1,"callId":"c2","name":"pwsh","arguments":"{\"command\":\"git log --oneline -1\"}"}}));
        q.apply(&json!({"type":"tool/result","seq":3,"time":1,"data":{"turn":1,"step":1,"message":{"role":"tool","toolCallId":"c2","content":[{"type":"text","text":"abc123 test"}],"isError":false}}}));
        assert_eq!(q.turns()[0].tools[0].outcome.as_ref().unwrap().exit_code, Some(0));
        assert_eq!(p.orphans, 0);
        assert_eq!(p.assistant_text(), "done");
        assert_eq!(p.last_end().unwrap().kind, "completed");
        let usage = p.usage.unwrap();
        assert_eq!(usage.cache_read_tokens, Some(6400));
        assert_eq!(usage.completion_tokens, Some(113));
    }

    #[test]
    fn real_quota_failure_reaches_failures() {
        let mut p = Projection::new();
        p.apply(&json!({"type":"assistant/attempt","seq":14,"time":1,"data":{"turn":1,"step":1,"stream":[
            {"type":"chunk","time":1,"chunk":{"type":"finish","reason":{"kind":"error","failure":{"message":"Insufficient Balance"}}}}]}}));
        p.apply(&json!({"type":"turn/end","seq":16,"time":2,"data":{"turn":1,"reason":{"kind":"error","error":{"message":"Insufficient Balance","code":"QUOTA","status":402}}}}));
        assert_eq!(p.last_end().unwrap().kind, "error");
        // 结构化字段要留住：分类优先用它，而不是猜英文原文。
        assert_eq!(p.last_end().unwrap().code.as_deref(), Some("QUOTA"));
        assert_eq!(p.last_end().unwrap().status, Some(402));
        assert!(p.failures.iter().any(|f| f.contains("Insufficient Balance")));
        assert_eq!(p.attempts, 1);
    }

    #[test]
    fn exit_code_marker_parses_the_last_marker() {
        assert_eq!(exit_code_marker("x\n[exit code: 101]"), Some(101));
        assert_eq!(exit_code_marker("ok"), None);
        assert_eq!(exit_code_marker("[exit code: 2] then [exit code: 3]"), Some(3));
    }
}
