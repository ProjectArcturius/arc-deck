//! DSH 0.2.1 的会话事件 → [`arc_deck_core::conversation::Conversation`]。
//!
//! 输入有三种，都来自 `session/follow`（与 `session/page`）：
//!
//! | 输入 | 是什么 | 这里怎么用 |
//! |---|---|---|
//! | `snapshot` | 打开时的一页历史 + projections + 进行中的流（P11：快照就是 opening 的全部） | [`ConversationFold::apply_snapshot`] |
//! | `{type:'event', event}` | 落盘的事件，seq 稠密递增 | [`ConversationFold::apply_event`] |
//! | `{type:'assistant-stream', frame}` | **进程内**的流式帧，不落盘（`assistantStream: true` 才有） | [`ConversationFold::apply_stream`] |
//!
//! ## 几条判断（每条都有对应的坑）
//!
//! - **0.2.1 没有落盘的 `assistant/delta`**。流式正文只在 `assistant-stream` 帧里；权威全文是
//!   `assistant/message`。两者都用会写两遍（P4），所以权威全文到达时**整块替换**同一 step 的流式块。
//! - **`end` 帧在权威事件之后到**（2026-10-19 真实抓帧：`assistant/message` seq 17 先到，
//!   `end{committed, seq:17}` 后到）。所以已提交的 step 之后再来的 chunk 一律丢掉，不能让它把
//!   正文又长回去。
//! - **只有 `source.kind == "user"` 的 `user/message` 是人说的话**（P6）。同一轮里内核还写了
//!   runtime-context、skill-catalog、time-context 三条 `user/message`（抓帧实测），全画出来对话就被淹了。
//! - **按 turn 归并**（P8）：一轮里所有 step 的思考、正文、工具进同一条助手消息。
//! - **模型失败只有一个出口**（P3）：`turn/end` 的 `reason.error`，以及失败 attempt 的 finish 块。
//!   前者决定这一轮的状态；后者只计重试次数 —— 重试后成功的轮次不该挂一个红色失败块。
//!
//! ## 往回翻页
//!
//! 快照只是一页（P13）。往回翻得到的是**更早**的事件，增量折叠接不上，所以这里保留一份
//! 「与对话有关的」落盘事件（按 seq 排序），翻页后整体重放一遍。被丢掉的那些（系统提示词、
//! 工具清单、request/header —— 抓帧里单这一条就有几十 KB）不进这份副本。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use arc_deck_core::conversation::{
    AssistantTurn, Block, Conversation, Message, ModelChoice, TextPart, TokenUsage, ToolCall,
    ToolStatus, TurnState, UserMessage,
};
use serde_json::Value;

use crate::projection::exit_code_marker;

/// 进 [`ConversationFold`] 那份副本的事件类型。其余的不影响对话视图。
const KEPT_EVENTS: &[&str] = &[
    "turn/start",
    "turn/end",
    "user/message",
    "assistant/message",
    "assistant/attempt",
    "tool/call",
    "tool/result",
    "session/title",
    "model/selection",
];

/// 一个流式 attempt 的身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AttemptKey {
    turn: u32,
    step: u32,
}

#[derive(Debug, Default)]
pub struct ConversationFold {
    view: Conversation,
    /// 与对话有关的落盘事件，按 seq。翻页后据此重放。
    events: BTreeMap<u64, Value>,
    /// 最小的已知 seq（往回翻的 `beforeSeq`）。
    first_seq: Option<u64>,
    /// 快照游标 —— 翻页的 `throughSeq` 必须取自打开时的那一帧（`SessionPageRequest` 的注释），
    /// 所以实时事件不推进它。
    cursor: Option<u64>,
    /// 正在进行的轮次。
    current_turn: u32,
    /// 取消后的旧流帧不能把已经结束的轮次重新标成 running。
    ended_turn: u32,
    /// 已经有权威全文的 step —— 之后到的流式 chunk 丢掉。
    committed: BTreeSet<(u32, u32)>,
    /// 流式 attempt：id → 它属于哪一轮哪一步。
    attempts: HashMap<String, AttemptKey>,
    /// 还没提交的流式 chunk，按 attempt 记着：翻页重放之后要把它们重新叠上去。
    pending_stream: Vec<(String, Value)>,
    /// 视图每变一次加一。界面据此决定要不要重画、IPC 据此去重。
    revision: u64,
}

impl ConversationFold {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn view(&self) -> &Conversation {
        &self.view
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 往回翻页时的 `throughSeq`（快照游标）与 `beforeSeq`（已知最早的 seq）。
    /// 历史已取全时返回 None。
    pub fn page_cursor(&self) -> Option<(u64, u64)> {
        if self.view.history_complete {
            return None;
        }
        Some((self.cursor?, self.first_seq?))
    }

    /// 打开会话时的快照（P11：它就是 opening 的全部，之后都是 live）。
    pub fn apply_snapshot(&mut self, snapshot: &Value) {
        *self = ConversationFold::default();
        let header = &snapshot["header"];
        self.view.session_id = header["id"].as_str().unwrap_or_default().to_string();
        self.view.cwd = header["cwd"].as_str().map(str::to_string);
        self.cursor = snapshot["cursor"].as_u64();
        self.view.history_complete = !snapshot["hasMore"].as_bool().unwrap_or(false);

        for record in snapshot["records"].as_array().into_iter().flatten() {
            self.keep(&record["event"]);
        }
        if let Some(first) = snapshot["records"].as_array().and_then(|r| r.first()) {
            self.first_seq = first["event"]["seq"].as_u64();
        }
        self.replay();

        // projections 是快照那一刻的**当前值**，比这一页里碰巧有的 `session/title` 更新 ——
        // 所以重放之后再盖上去（重放会清空视图）。
        let values = &snapshot["projections"]["values"];
        if let Some(title) = values["title"].as_str() {
            self.view.title = Some(title.to_string());
        }
        if let Some(choice) = model_choice(&values["modelSelection"]["next"]) {
            self.view.model = Some(choice);
        }

        // 打开时正在进行的流：紧凑记录（text-chunks / reasoning-chunks / 原始 chunk）展开后叠上去。
        let active = &snapshot["assistantStream"]["activeAttempt"];
        if let Some(attempt_id) = active["attemptId"].as_str() {
            let key = AttemptKey {
                turn: as_u32(&active["turn"]),
                step: as_u32(&active["step"]),
            };
            self.attempts.insert(attempt_id.to_string(), key);
            for record in active["stream"].as_array().into_iter().flatten() {
                for chunk in expand_compact(record) {
                    self.pending_stream.push((attempt_id.to_string(), chunk.clone()));
                    self.stream_chunk(attempt_id, &chunk);
                }
            }
        }
        self.bump();
    }

    /// 往回翻到的一页（更早的事件）。`has_more` 是那一页的 `hasMore`。
    pub fn apply_page(&mut self, records: &[Value], has_more: bool) {
        for record in records {
            self.keep(&record["event"]);
        }
        if let Some(first) = records.first()
            && let Some(seq) = first["event"]["seq"].as_u64() {
                self.first_seq = Some(self.first_seq.map_or(seq, |old| old.min(seq)));
            }
        // 「没有进展」也算到头：一页空的、或游标没有往回走，再翻只会原地打转（P13 的闸）。
        self.view.history_complete = !has_more || records.is_empty();
        let title = self.view.title.clone();
        let model = self.view.model.clone();
        self.replay();
        self.view.title = self.view.title.take().or(title);
        self.view.model = self.view.model.take().or(model);
        let pending = std::mem::take(&mut self.pending_stream);
        for (attempt, chunk) in &pending {
            self.stream_chunk(attempt, chunk);
        }
        self.pending_stream = pending;
        self.bump();
    }

    /// 一帧 follow 的 item（`snapshot` / `event` / `assistant-stream` 三种）。返回视图是否变了。
    pub fn apply_follow_item(&mut self, item: &Value) -> bool {
        let before = self.revision;
        match item["type"].as_str() {
            Some("snapshot") => self.apply_snapshot(item),
            Some("event") => self.apply_event(&item["event"]),
            Some("assistant-stream") => self.apply_stream(&item["frame"]),
            _ => {}
        }
        self.revision != before
    }

    /// 一条落盘事件（实时那一路）。
    pub fn apply_event(&mut self, event: &Value) {
        if !self.keep(event) {
            return;
        }
        self.fold(event);
        self.bump();
    }

    /// 一帧进程内的流式帧。
    pub fn apply_stream(&mut self, frame: &Value) {
        let Some(attempt_id) = frame["attemptId"].as_str() else {
            return;
        };
        match frame["type"].as_str() {
            Some("start") => {
                let key = AttemptKey { turn: as_u32(&frame["turn"]), step: as_u32(&frame["step"]) };
                if key.turn <= self.ended_turn {
                    return;
                }
                self.attempts.insert(attempt_id.to_string(), key);
                self.pending_stream.retain(|(id, _)| id != attempt_id);
                self.current_turn = self.current_turn.max(key.turn);
                self.view.running = true;
            }
            Some("chunk") => {
                self.pending_stream.push((attempt_id.to_string(), frame["chunk"].clone()));
                self.stream_chunk(attempt_id, &frame["chunk"]);
            }
            Some("end") => {
                let committed_message = frame["outcome"]["kind"] == "committed"
                    && frame["outcome"]["eventType"] == "assistant/message";
                if let Some(key) = self.attempts.remove(attempt_id)
                    && !committed_message {
                        // 失败 / 放弃的 attempt：它的流式块不会再被权威全文替换，留着就成了「幽灵正文」。
                        self.drop_streaming(key);
                    }
                self.pending_stream.retain(|(id, _)| id != attempt_id);
            }
            _ => return,
        }
        self.bump();
    }

    fn bump(&mut self) {
        self.revision += 1;
    }

    /// 把一条事件收进副本。不相关的类型返回 false。
    fn keep(&mut self, event: &Value) -> bool {
        let Some(kind) = event["type"].as_str() else {
            return false;
        };
        if !KEPT_EVENTS.contains(&kind) {
            return false;
        }
        if kind == "user/message" && !is_human(event) {
            return false;
        }
        match event["seq"].as_u64() {
            Some(seq) => {
                if self.events.contains_key(&seq) {
                    // 同一条事件重复到达（快照与翻页的边界重叠）：只认一次。
                    return false;
                }
                self.events.insert(seq, slim(event));
                true
            }
            None => true,
        }
    }

    /// 从副本整体重放（翻页之后）。
    fn replay(&mut self) {
        let keep_session = self.view.session_id.clone();
        let keep_cwd = self.view.cwd.clone();
        let complete = self.view.history_complete;
        self.view = Conversation {
            session_id: keep_session,
            cwd: keep_cwd,
            history_complete: complete,
            ..Conversation::default()
        };
        self.committed.clear();
        self.current_turn = 0;
        self.ended_turn = 0;
        let events: Vec<Value> = self.events.values().cloned().collect();
        for event in &events {
            self.fold(event);
        }
    }

    fn fold(&mut self, event: &Value) {
        let data = &event["data"];
        match event["type"].as_str().unwrap_or_default() {
            "turn/start" => {
                self.current_turn = as_u32(&data["turn"]);
                self.view.running = true;
            }
            "user/message" => {
                let (text, attachments) = user_content(&data["content"]);
                self.view.messages.push(Message::User(UserMessage {
                    turn: self.current_turn,
                    text,
                    attachments,
                }));
            }
            "assistant/message" => {
                let turn = as_u32(&data["turn"]);
                let step = as_u32(&data["step"]);
                self.committed.insert((turn, step));
                let usage = token_usage(&data["usage"]);
                let message = self.assistant(turn);
                replace_step_text(message, step, &data["message"]["content"]);
                if let Some(usage) = usage {
                    message.usage.get_or_insert_with(TokenUsage::default).add(&usage);
                }
            }
            "assistant/attempt" => {
                let turn = as_u32(&data["turn"]);
                let step = as_u32(&data["step"]);
                let message = self.assistant(turn);
                message.retries += 1;
                remove_streaming(message, step);
            }
            "tool/call" => {
                let turn = as_u32(&data["turn"]);
                let step = as_u32(&data["step"]);
                let call = ToolCall {
                    call_id: data["callId"].as_str().unwrap_or_default().to_string(),
                    name: data["name"].as_str().unwrap_or_default().to_string(),
                    arguments: match &data["arguments"] {
                        Value::String(raw) => raw.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    },
                    status: ToolStatus::Running,
                    step,
                    ..ToolCall::default()
                };
                self.assistant(turn).blocks.push(Block::Tool(call));
            }
            "tool/result" => {
                let turn = as_u32(&data["turn"]);
                let message = &data["message"];
                let call_id = message["toolCallId"]
                    .as_str()
                    .or_else(|| message["source"]["callId"].as_str())
                    .unwrap_or_default()
                    .to_string();
                let output = text_of(&message["content"]);
                let is_error = message["isError"].as_bool().unwrap_or(false);
                let error = data["error"]["reason"]
                    .as_str()
                    .or_else(|| data["error"]["code"].as_str())
                    .map(str::to_string);
                let detail = match &data["meta"] {
                    Value::Null => None,
                    other => Some(other.clone()),
                };
                let assistant = self.assistant(turn);
                let found = assistant.blocks.iter_mut().rev().find_map(|block| match block {
                    Block::Tool(call) if call.call_id == call_id => Some(call),
                    _ => None,
                });
                // 没有对应调用的结果（孤儿）：不编一个调用出来，只是不显示 —— 与 P9 的计数口径一致。
                if let Some(call) = found {
                    call.exit_code = exit_code_marker(&output);
                    call.status = if is_error { ToolStatus::Failed } else { ToolStatus::Done };
                    call.output = Some(output);
                    call.error = error;
                    call.detail = detail;
                }
            }
            "turn/end" => {
                let turn = as_u32(&data["turn"]);
                self.ended_turn = self.ended_turn.max(turn);
                let state = turn_state(&data["reason"]);
                let message = self.assistant(turn);
                for block in &mut message.blocks {
                    if let Block::Tool(call) = block
                        && call.status == ToolStatus::Running {
                            call.status = ToolStatus::Abandoned;
                        }
                }
                // 这一轮里没提交的流式块：内核说过「被取消的轮次若没有 assistant/message，
                // 就是没有可见内容」（SessionEventMap 的注释），所以它们不该留下来。
                message.blocks.retain(|block| !is_streaming(block));
                message.state = state;
                if turn >= self.current_turn {
                    self.view.running = false;
                }
                self.attempts.retain(|_, key| key.turn > turn);
                self.pending_stream.retain(|(id, _)| self.attempts.contains_key(id));
            }
            "session/title" => {
                if let Some(title) = data["title"].as_str() {
                    self.view.title = Some(title.to_string());
                }
            }
            "model/selection" => {
                if let Some(choice) = model_choice(data) {
                    self.view.model = Some(choice);
                }
            }
            _ => {}
        }
    }

    /// 这一轮的助手消息；没有就建一条。
    fn assistant(&mut self, turn: u32) -> &mut AssistantTurn {
        let index = self.view.messages.iter().rposition(|m| matches!(m, Message::Assistant(a) if a.turn == turn));
        let index = match index {
            Some(index) => index,
            None => {
                self.view.messages.push(Message::Assistant(AssistantTurn { turn, ..AssistantTurn::default() }));
                self.view.messages.len() - 1
            }
        };
        match &mut self.view.messages[index] {
            Message::Assistant(message) => message,
            _ => unreachable!("rposition 只找助手消息"),
        }
    }

    fn stream_chunk(&mut self, attempt_id: &str, chunk: &Value) {
        let Some(key) = self.attempts.get(attempt_id).copied() else {
            return;
        };
        if self.committed.contains(&(key.turn, key.step)) {
            // 权威全文已经到了（`end` 帧晚于 `assistant/message`，见模块头）。
            return;
        }
        let (reasoning, text) = match chunk["type"].as_str() {
            Some("text-delta") => (false, chunk["text"].as_str().unwrap_or_default()),
            Some("reasoning-delta") => (true, chunk["text"].as_str().unwrap_or_default()),
            _ => return,
        };
        if text.is_empty() {
            return;
        }
        self.view.running = true;
        let message = self.assistant(key.turn);
        // 同一 step、同一种类的最后一块流式块就是正在长的那一块。
        let target = message.blocks.iter_mut().rev().find_map(|block| match block {
            Block::Text(part) if !reasoning && part.streaming && part.step == key.step => Some(part),
            Block::Reasoning(part) if reasoning && part.streaming && part.step == key.step => Some(part),
            _ => None,
        });
        match target {
            Some(part) => part.text.push_str(text),
            None => {
                let part = TextPart { text: text.to_string(), streaming: true, step: key.step };
                message.blocks.push(if reasoning { Block::Reasoning(part) } else { Block::Text(part) });
            }
        }
    }

    fn drop_streaming(&mut self, key: AttemptKey) {
        if let Some(Message::Assistant(message)) = self
            .view
            .messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m, Message::Assistant(a) if a.turn == key.turn))
        {
            remove_streaming(message, key.step);
        }
    }
}

fn is_streaming(block: &Block) -> bool {
    matches!(block, Block::Text(p) | Block::Reasoning(p) if p.streaming)
}

fn remove_streaming(message: &mut AssistantTurn, step: u32) {
    message
        .blocks
        .retain(|block| !matches!(block, Block::Text(p) | Block::Reasoning(p) if p.streaming && p.step == step));
}

/// 权威全文替换同一 step 的流式块：插在原来流式块的位置（没有就放在这一 step 的工具之前，再没有就放最后）。
fn replace_step_text(message: &mut AssistantTurn, step: u32, content: &Value) {
    let first_streaming = message
        .blocks
        .iter()
        .position(|block| matches!(block, Block::Text(p) | Block::Reasoning(p) if p.streaming && p.step == step));
    remove_streaming(message, step);
    let first_tool = message
        .blocks
        .iter()
        .position(|block| matches!(block, Block::Tool(call) if call.step == step));
    let at = first_streaming.or(first_tool).unwrap_or(message.blocks.len()).min(message.blocks.len());

    let mut parts = Vec::new();
    for block in content.as_array().into_iter().flatten() {
        let text = block["text"].as_str().unwrap_or_default();
        // 空思考块真实存在（抓帧：`{"text":"","type":"reasoning"}`）—— 画出来是一个空的折叠条。
        if text.trim().is_empty() {
            continue;
        }
        let part = TextPart { text: text.to_string(), streaming: false, step };
        match block["type"].as_str() {
            Some("text") => parts.push(Block::Text(part)),
            Some("reasoning") => parts.push(Block::Reasoning(part)),
            _ => {}
        }
    }
    for (offset, part) in parts.into_iter().enumerate() {
        message.blocks.insert(at + offset, part);
    }
}

/// 人说的话：`source.kind == "user"`（P6）。不按长度、不按内容猜。
fn is_human(event: &Value) -> bool {
    event["data"]["source"]["kind"] == "user"
}

/// 副本里只留折叠要用的字段：`assistant/message` 的 `stream` 是整条原始流，比正文大得多。
fn slim(event: &Value) -> Value {
    let mut event = event.clone();
    if let Some(data) = event.get_mut("data").and_then(Value::as_object_mut) {
        data.remove("stream");
        if let Some(message) = data.get_mut("message").and_then(Value::as_object_mut)
            && let Some(source) = message.get_mut("source").and_then(Value::as_object_mut) {
                source.remove("replayState");
            }
    }
    event
}

fn user_content(content: &Value) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut attachments = Vec::new();
    for block in content.as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().unwrap_or_default()),
            Some("image") => attachments.push("[图片]".to_string()),
            Some("file") => attachments.push(format!(
                "[文件 {}]",
                block["name"].as_str().or_else(|| block["path"].as_str()).unwrap_or("")
            )),
            Some(other) => attachments.push(format!("[{other}]")),
            None => {}
        }
    }
    (text, attachments)
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn turn_state(reason: &Value) -> TurnState {
    match reason["kind"].as_str().unwrap_or_default() {
        "completed" => TurnState::Completed,
        "aborted" => TurnState::Cancelled,
        "error" => TurnState::Failed {
            message: reason["error"]["message"].as_str().unwrap_or("模型或内核失败（内核没有给原文）").to_string(),
            code: reason["error"]["code"].as_str().map(str::to_string),
        },
        "max-tokens" => TurnState::MaxTokens,
        "blocked" => TurnState::Blocked,
        "interrupted" => TurnState::Interrupted,
        other => TurnState::Other { kind: other.to_string() },
    }
}

fn token_usage(usage: &Value) -> Option<TokenUsage> {
    if !usage.is_object() {
        return None;
    }
    Some(TokenUsage {
        input: usage["inputTokens"].as_u64(),
        output: usage["outputTokens"].as_u64(),
        cache_read: usage["cacheReadTokens"].as_u64(),
        cache_write: usage["cacheWriteTokens"].as_u64(),
    })
}

pub(crate) fn model_choice(value: &Value) -> Option<ModelChoice> {
    Some(ModelChoice {
        provider: value["provider"].as_str()?.to_string(),
        model: value["model"].as_str()?.to_string(),
        effort: value["reasoningEffort"].as_str().map(str::to_string),
    })
}

/// 紧凑流记录（`AssistantStreamRecord`）展开成 chunk。只展开正文与思考 —— 工具调用的增量
/// 等落盘的 `tool/call` 就行，界面不需要看模型一个字一个字地写参数。
fn expand_compact(record: &Value) -> Vec<Value> {
    match record["type"].as_str() {
        Some(kind @ ("text-chunks" | "reasoning-chunks")) => {
            let delta = if kind == "text-chunks" { "text-delta" } else { "reasoning-delta" };
            let text: String = record["texts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            vec![serde_json::json!({ "type": delta, "index": record["index"], "text": text })]
        }
        Some("chunk") => vec![record["chunk"].clone()],
        _ => Vec::new(),
    }
}

fn as_u32(value: &Value) -> u32 {
    value.as_u64().unwrap_or(0).min(u32::MAX as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 2026-10-19 真实抓帧（`remote_quota_one_turn_is_captured_end_to_end`），路径已改成 `C:\ws`，
    /// `request/header` 的工具清单与系统提示词已截短 —— 它们本来就不进对话视图。
    const ONE_TURN: &str = include_str!("../../tests/fixtures/remote/one-turn.jsonl");

    fn frames(raw: &str) -> Vec<Value> {
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect()
    }

    fn assistant(view: &Conversation, index: usize) -> &AssistantTurn {
        match &view.messages[index] {
            Message::Assistant(turn) => turn,
            other => panic!("第 {index} 条不是助手消息：{other:?}"),
        }
    }

    #[test]
    fn the_real_capture_folds_into_one_question_and_one_answer() {
        let mut fold = ConversationFold::new();
        for frame in frames(ONE_TURN) {
            fold.apply_follow_item(&frame);
        }
        let view = fold.view();
        assert_eq!(view.messages.len(), 2, "{:#?}", view.messages);
        match &view.messages[0] {
            Message::User(user) => assert_eq!(user.text, "只回复两个字：收到"),
            other => panic!("{other:?}"),
        }
        let answer = assistant(view, 1);
        assert_eq!(answer.text(), "收到", "正文写了两遍或没替换：{:?}", answer.blocks);
        assert_eq!(answer.state, TurnState::Completed);
        assert!(answer.blocks.iter().all(|b| !is_streaming(b)), "权威全文到了还挂着流式块");
        // 空思考块不画（抓帧里真有一个 `text:""` 的 reasoning）。
        assert!(!answer.blocks.iter().any(|b| matches!(b, Block::Reasoning(_))));
        assert_eq!(answer.usage.as_ref().and_then(|u| u.input), Some(9048));
        assert!(!view.running);
        // 标题：先是首句兜底，后被模型生成的标题替换。
        assert_eq!(view.title.as_deref(), Some("简单确认回复测试"));
        assert_eq!(view.model.as_ref().map(|m| m.provider.as_str()), Some("opencode-go"));
        assert!(view.history_complete);
    }

    #[test]
    fn a_late_stream_start_cannot_revive_an_aborted_turn() {
        let mut fold = ConversationFold::new();
        fold.apply_event(&json!({"type":"turn/start","seq":0,"data":{"turn":1}}));
        fold.apply_stream(&json!({"type":"start","attemptId":"original","turn":1,"step":1}));
        fold.apply_event(&json!({"type":"turn/end","seq":1,"data":{"turn":1,"reason":{"kind":"aborted","reason":{"kind":"user"}}}}));
        fold.apply_stream(&json!({"type":"chunk","attemptId":"original","chunk":{"type":"text-delta","text":"ghost"}}));
        assert!(fold.view().messages.iter().all(|m| !matches!(m, Message::Assistant(a) if a.blocks.iter().any(is_streaming))));
        fold.apply_stream(&json!({"type":"start","attemptId":"late","turn":1,"step":1}));
        assert!(!fold.view().running, "取消后迟到的流不能重新启动扫描条");
        fold.apply_stream(&json!({"type":"start","attemptId":"next","turn":2,"step":1}));
        assert!(fold.view().running, "新一轮仍然必须启动");
    }

    #[test]
    fn mid_stream_the_text_is_visible_and_marked_streaming() {
        // 只喂到 text-delta 为止：流式正文已可见，但还没有权威全文。
        let mut fold = ConversationFold::new();
        for frame in frames(ONE_TURN) {
            let is_message = frame["type"] == "event" && frame["event"]["type"] == "assistant/message";
            if is_message {
                break;
            }
            fold.apply_follow_item(&frame);
        }
        let answer = assistant(fold.view(), 1);
        assert_eq!(answer.text(), "收到");
        assert!(answer.blocks.iter().any(is_streaming));
        assert!(fold.view().running);
    }

    #[test]
    fn chunks_arriving_after_the_authoritative_message_do_not_grow_the_text_back() {
        let mut fold = ConversationFold::new();
        fold.apply_follow_item(&json!({"type":"snapshot","header":{"id":"s"},"cursor":0,"records":[],"hasMore":false,"projections":{"values":{}}}));
        fold.apply_event(&json!({"type":"turn/start","seq":1,"data":{"turn":1}}));
        fold.apply_stream(&json!({"type":"start","attemptId":"a","turn":1,"step":1}));
        fold.apply_stream(&json!({"type":"chunk","attemptId":"a","chunk":{"type":"text-delta","index":0,"text":"半"}}));
        fold.apply_event(&json!({"type":"assistant/message","seq":2,"data":{"turn":1,"step":1,"message":{"content":[{"type":"text","text":"全文"}]}}}));
        // 迟到的 chunk：属于已经提交的 step。
        fold.apply_stream(&json!({"type":"chunk","attemptId":"a","chunk":{"type":"text-delta","index":0,"text":"多余"}}));
        assert_eq!(assistant(fold.view(), 0).text(), "全文");
    }

    #[test]
    fn a_failed_attempt_that_was_retried_leaves_no_ghost_text_and_no_failure() {
        let mut fold = ConversationFold::new();
        fold.apply_follow_item(&json!({"type":"snapshot","header":{"id":"s"},"cursor":0,"records":[],"hasMore":false,"projections":{"values":{}}}));
        fold.apply_event(&json!({"type":"turn/start","seq":1,"data":{"turn":1}}));
        fold.apply_stream(&json!({"type":"start","attemptId":"a1","turn":1,"step":1}));
        fold.apply_stream(&json!({"type":"chunk","attemptId":"a1","chunk":{"type":"text-delta","index":0,"text":"断在半"}}));
        fold.apply_event(&json!({"type":"assistant/attempt","seq":2,"data":{"turn":1,"step":1,"stream":[]}}));
        fold.apply_stream(&json!({"type":"end","attemptId":"a1","outcome":{"kind":"committed","eventType":"assistant/attempt","seq":2}}));
        fold.apply_stream(&json!({"type":"start","attemptId":"a2","turn":1,"step":1}));
        fold.apply_event(&json!({"type":"assistant/message","seq":3,"data":{"turn":1,"step":1,"message":{"content":[{"type":"text","text":"好了"}]}}}));
        fold.apply_event(&json!({"type":"turn/end","seq":4,"data":{"turn":1,"reason":{"kind":"completed"}}}));
        let answer = assistant(fold.view(), 0);
        assert_eq!(answer.text(), "好了");
        assert_eq!(answer.retries, 1);
        assert_eq!(answer.state, TurnState::Completed);
    }

    #[test]
    fn a_model_failure_is_the_turn_state_with_the_upstream_text() {
        // P3：错误的模型名只在轮次内部报错 —— 原文必须到界面上。
        let mut fold = ConversationFold::new();
        fold.apply_event(&json!({"type":"turn/start","seq":1,"data":{"turn":1}}));
        fold.apply_event(&json!({"type":"turn/end","seq":2,"data":{"turn":1,"reason":{"kind":"error",
            "error":{"message":"Insufficient Balance","code":"QUOTA","status":402}}}}));
        match &assistant(fold.view(), 0).state {
            TurnState::Failed { message, code } => {
                assert_eq!(message, "Insufficient Balance");
                assert_eq!(code.as_deref(), Some("QUOTA"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tools_pair_by_call_id_and_unfinished_ones_are_abandoned_on_cancel() {
        let mut fold = ConversationFold::new();
        fold.apply_event(&json!({"type":"turn/start","seq":1,"data":{"turn":1}}));
        fold.apply_event(&json!({"type":"tool/call","seq":2,"data":{"turn":1,"step":1,"callId":"c1","name":"pwsh","arguments":"{\"command\":\"cmd /c exit 3\"}"}}));
        fold.apply_event(&json!({"type":"tool/call","seq":3,"data":{"turn":1,"step":1,"callId":"c2","name":"read","arguments":"{}"}}));
        fold.apply_event(&json!({"type":"tool/result","seq":4,"data":{"turn":1,"step":1,"message":{"toolCallId":"c1","isError":false,
            "content":[{"type":"text","text":"(no output)\n[exit code: 3]"}]},"meta":{"cwd":"C:/ws"}}}));
        fold.apply_event(&json!({"type":"turn/end","seq":5,"data":{"turn":1,"reason":{"kind":"aborted","reason":"user"}}}));
        let answer = assistant(fold.view(), 0);
        let tools: Vec<&ToolCall> = answer.blocks.iter().filter_map(|b| match b { Block::Tool(c) => Some(c), _ => None }).collect();
        assert_eq!(tools[0].status, ToolStatus::Done);
        assert_eq!(tools[0].exit_code, Some(3));
        assert_eq!(tools[0].argument("command").as_deref(), Some("cmd /c exit 3"));
        assert_eq!(tools[0].detail, Some(json!({"cwd":"C:/ws"})));
        assert_eq!(tools[1].status, ToolStatus::Abandoned, "取消时没有结果的调用不该永远转圈");
        assert_eq!(answer.state, TurnState::Cancelled);
    }

    #[test]
    fn internal_user_messages_never_become_conversation() {
        // P6：同一轮里内核写的三条 user/message（runtime-context / skill-catalog / time-context）。
        let mut fold = ConversationFold::new();
        fold.apply_event(&json!({"type":"turn/start","seq":1,"data":{"turn":1}}));
        for (seq, kind) in [(2, "runtime-context"), (3, "skill-catalog"), (4, "time-context")] {
            fold.apply_event(&json!({"type":"user/message","seq":seq,"data":{"source":{"kind":kind},"content":[{"type":"text","text":"长长的英文"}]}}));
        }
        fold.apply_event(&json!({"type":"user/message","seq":5,"data":{"source":{"kind":"user"},"content":[{"type":"text","text":"你好"}]}}));
        assert_eq!(fold.view().messages.len(), 1);
    }

    #[test]
    fn paging_back_prepends_older_turns_and_keeps_the_live_tail() {
        // 快照里只有第 2 轮；往回翻拿到第 1 轮。翻完之后顺序正确、没有重复。
        let mut fold = ConversationFold::new();
        fold.apply_snapshot(&json!({"type":"snapshot","header":{"id":"s"},"cursor":20,"hasMore":true,
            "projections":{"values":{"title":"旧标题"}},
            "records":[
                {"type":"event","event":{"type":"turn/start","seq":10,"data":{"turn":2}}},
                {"type":"event","event":{"type":"user/message","seq":11,"data":{"source":{"kind":"user"},"content":[{"type":"text","text":"第二问"}]}}},
                {"type":"event","event":{"type":"assistant/message","seq":12,"data":{"turn":2,"step":1,"message":{"content":[{"type":"text","text":"第二答"}]}}}},
                {"type":"event","event":{"type":"turn/end","seq":13,"data":{"turn":2,"reason":{"kind":"completed"}}}}
            ]}));
        assert_eq!(fold.page_cursor(), Some((20, 10)));
        fold.apply_page(&[
            json!({"type":"event","event":{"type":"turn/start","seq":1,"data":{"turn":1}}}),
            json!({"type":"event","event":{"type":"user/message","seq":2,"data":{"source":{"kind":"user"},"content":[{"type":"text","text":"第一问"}]}}}),
            json!({"type":"event","event":{"type":"assistant/message","seq":3,"data":{"turn":1,"step":1,"message":{"content":[{"type":"text","text":"第一答"}]}}}}),
            json!({"type":"event","event":{"type":"turn/end","seq":4,"data":{"turn":1,"reason":{"kind":"completed"}}}}),
            // 边界重叠：翻页结果里又出现快照里已有的一条。
            json!({"type":"event","event":{"type":"turn/start","seq":10,"data":{"turn":2}}}),
        ], false);
        let texts: Vec<String> = fold.view().messages.iter().map(|m| match m {
            Message::User(u) => u.text.clone(),
            Message::Assistant(a) => a.text(),
            Message::Unknown => String::new(),
        }).collect();
        assert_eq!(texts, ["第一问", "第一答", "第二问", "第二答"]);
        assert!(fold.view().history_complete);
        assert_eq!(fold.page_cursor(), None);
        assert_eq!(fold.view().title.as_deref(), Some("旧标题"), "翻页重放不该丢掉 projections 里的标题");
    }

    #[test]
    fn an_empty_page_stops_paging_even_if_the_kernel_says_has_more() {
        let mut fold = ConversationFold::new();
        fold.apply_snapshot(&json!({"type":"snapshot","header":{"id":"s"},"cursor":5,"hasMore":true,
            "projections":{"values":{}},
            "records":[{"type":"event","event":{"type":"turn/start","seq":3,"data":{"turn":1}}}]}));
        fold.apply_page(&[], true);
        assert!(fold.view().history_complete, "空页还说有更多：再翻只会原地打转");
    }
}
