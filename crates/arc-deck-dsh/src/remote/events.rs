//! 内核 → 客户端的**转发的 Cordis 事件**（`$events` 流），以及把回答送回去的一元调用。
//!
//! 为什么审批要单独一条流：`approval/request` 是 Host 的 **waterfall** —— 它把问题交给
//! 连着的客户端，**等客户端回答**（`packages/api/remotes/src/remote-events.ts` 里
//! `{ event: 'approval/request', mode: 'waterfall' }`；客户端那一侧是
//! `packages/api/gateway/src/client/remote-events.ts`）。没有客户端开这条流时，
//! `ApprovalService` 按 `unavailable` 失败关闭 —— 工具被拒，但会话不会挂住。
//!
//! 线形状（`packages/api/gateway/src/stream-protocol.ts`）：
//!
//! ```text
//! WS  open  endpoint="$events" payload={"args":{}}
//!   ← {"type":"ready","clientId":…,"host":{"home":…}}
//!   ← {"type":"waterfall","event":"approval/request","eventId":…,"agentId":…,"request":{…}}
//!   ← {"type":"emit","event":"api-session/activity","args":[…]}   （通知，不需要回答）
//!   ← {"type":"cancel","eventId":…}                                （那条请求作废）
//! POST /api/$events/result  {"args":{"clientId":…,"eventId":…,"outcome":{…}}}
//! ```
//!
//! 三条纪律都来自协议本身，写在这里省得下一个人再猜：
//!
//! 1. **不认识的 waterfall 必须回 `next`**。回 `next` 是「我不接，交给 Host 链上的下一个」；
//!    不回就等于把它短路掉 —— 那会让内核里别的应答者（如果有）永远等不到自己的轮次。
//! 2. **`cancel` 不是错误**。它表示那条请求已经不需要回答了（这一轮被取消）。
//! 3. **应答要带 `clientId`**：Host 用它把结果绑定到开这条流的那个客户端世代上。

use serde_json::{json, Value};

use arc_deck_core::conversation::{ApprovalOutcome, ApprovalRequest};

use super::{RemoteError, RemoteFrame, RemoteMux, RemoteRpc, RemoteStream};

/// 转发事件流的端点名（`REMOTE_EVENT_STREAM_ENDPOINT`）。
pub const EVENTS_ENDPOINT: &str = "$events";
/// 送回一条结果的端点名（`REMOTE_EVENT_RESULT_ENDPOINT`）。
pub const EVENTS_RESULT_ENDPOINT: &str = "$events/result";

/// 一条打开着的转发事件流。
pub struct EventSubscription {
    stream: RemoteStream,
    client_id: String,
}

/// [`EventSubscription::next`] 的结果。
#[derive(Debug, Clone, PartialEq)]
pub enum Forwarded {
    /// 内核在等人回答一个审批。
    Approval(ApprovalRequest),
    /// 别的 waterfall。调用方**要回 `Answer::Next`**（见模块头第 1 条）。
    Waterfall { request_id: String, event: String },
    /// 通知类事件：记下来就够了，不需要回答。
    Emit { event: String },
    /// 那条请求作废了（例如这一轮被取消）。
    Cancelled { request_id: String },
    /// 认不出的帧。**不是错误**：新版本内核加一种帧不该让审批通道整个断掉。
    Ignored,
    /// 流结束了，原因原样带出。
    Ended(String),
}

/// 送回去的回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    AllowOnce,
    Reject,
    /// 不接这一条，交回 Host 的链。
    Next,
}

impl EventSubscription {
    /// 打开 `$events` 流并等到 `ready` 帧。
    ///
    /// `ready` 是**必需**的第一帧：没有它就没有 `clientId`，之后的回答会被 Host 拒掉。
    pub async fn open(mux: &RemoteMux) -> Result<Self, RemoteError> {
        let mut stream = mux.open(EVENTS_ENDPOINT, json!({})).await?;
        match stream.next().await {
            Some(RemoteFrame::Item(item)) if item["type"] == "ready" => {
                let client_id = item["clientId"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        RemoteError::Protocol(format!("$events 的 ready 帧没有 clientId：{item}"))
                    })?
                    .to_string();
                Ok(Self { stream, client_id })
            }
            Some(RemoteFrame::Item(item)) => Err(RemoteError::Protocol(format!(
                "$events 的第一帧不是 ready：{}",
                item["type"]
            ))),
            Some(RemoteFrame::Failure(failure)) => Err(RemoteError::Remote(failure)),
            Some(other) => Err(RemoteError::Closed(format!(
                "转发事件流在 ready 之前就结束了：{other:?}"
            ))),
            None => Err(RemoteError::Closed("转发事件流在 ready 之前就结束了".into())),
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// 等下一帧。结束之后继续调用会一直返回 `Ended`。
    pub async fn next(&mut self) -> Forwarded {
        match self.stream.next().await {
            Some(RemoteFrame::Item(item)) => parse_item(&item),
            Some(RemoteFrame::End) => Forwarded::Ended("内核结束了转发事件流".into()),
            Some(RemoteFrame::Failure(failure)) => Forwarded::Ended(failure.to_string()),
            Some(RemoteFrame::Disconnected(reason)) => Forwarded::Ended(reason),
            None => Forwarded::Ended("转发事件流已关闭".into()),
        }
    }

    pub fn close(&mut self) {
        self.stream.close();
    }
}

fn parse_item(item: &Value) -> Forwarded {
    match item["type"].as_str().unwrap_or_default() {
        "waterfall" => {
            let request_id = item["eventId"].as_str().unwrap_or_default().to_string();
            if request_id.is_empty() {
                return Forwarded::Ignored;
            }
            let event = item["event"].as_str().unwrap_or_default().to_string();
            if event == "approval/request" {
                let request = &item["request"];
                let display = request["displayReason"]
                    .as_object()
                    .and_then(|map| map.get("zh").or_else(|| map.get("en")))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                Forwarded::Approval(ApprovalRequest {
                    request_id,
                    tool_name: request["toolName"].as_str().unwrap_or_default().to_string(),
                    call_id: request["callId"].as_str().map(str::to_string),
                    reason: request["reason"].as_str().map(str::to_string),
                    display_reason: display,
                    agent: item["agentId"].as_str().map(str::to_string),
                })
            } else {
                Forwarded::Waterfall { request_id, event }
            }
        }
        "emit" => Forwarded::Emit {
            event: item["event"].as_str().unwrap_or_default().to_string(),
        },
        "cancel" => match item["eventId"].as_str() {
            Some(id) if !id.is_empty() => Forwarded::Cancelled { request_id: id.to_string() },
            _ => Forwarded::Ignored,
        },
        _ => Forwarded::Ignored,
    }
}

/// 把回答送回 Host。
pub async fn answer(
    rpc: &RemoteRpc,
    client_id: &str,
    request_id: &str,
    answer: Answer,
) -> Result<(), RemoteError> {
    let outcome = match answer {
        Answer::Next => json!({ "kind": "next" }),
        Answer::AllowOnce => json!({ "kind": "result", "value": "allowed-once" }),
        Answer::Reject => json!({ "kind": "result", "value": "rejected" }),
    };
    rpc.call(
        EVENTS_RESULT_ENDPOINT,
        json!({ "args": {
            "clientId": client_id,
            "eventId": request_id,
            "outcome": outcome,
        } }),
    )
    .await
    .map(|_| ())
}

/// 把界面上的应答翻成线上的值。`Unknown` 不给线上发（调用方应拒绝构造它）。
pub fn outcome_value(outcome: ApprovalOutcome) -> Answer {
    match outcome {
        ApprovalOutcome::AllowOnce => Answer::AllowOnce,
        ApprovalOutcome::Reject | ApprovalOutcome::Unknown => Answer::Reject,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(value: Value) -> Forwarded {
        parse_item(&value)
    }

    #[test]
    fn an_approval_waterfall_becomes_a_neutral_request() {
        let forwarded = item(json!({
            "type": "waterfall",
            "event": "approval/request",
            "eventId": "evt-1",
            "agentId": "agent-7",
            "request": {
                "toolName": "bash",
                "callId": "call-1",
                "reason": "hook says ask",
                "displayReason": { "en": "Allow it?", "zh": "允许吗？" },
            }
        }));
        let Forwarded::Approval(request) = forwarded else { panic!("{forwarded:?}") };
        assert_eq!(request.request_id, "evt-1");
        assert_eq!(request.tool_name, "bash");
        assert_eq!(request.call_id.as_deref(), Some("call-1"));
        assert_eq!(request.reason.as_deref(), Some("hook says ask"));
        assert_eq!(request.display_reason.as_deref(), Some("允许吗？"));
        assert_eq!(request.agent.as_deref(), Some("agent-7"));
    }

    /// 一个别家的 waterfall 必须能被认出来、回 `next`；把它当成审批弹出来会让人
    /// 回答一个不是审批的问题。
    #[test]
    fn another_waterfall_is_recognised_not_mistaken_for_an_approval() {
        let forwarded = item(json!({
            "type": "waterfall", "event": "user-questions/request", "eventId": "q",
            "agentId": "a", "request": { "prompt": "?" }
        }));
        assert_eq!(
            forwarded,
            Forwarded::Waterfall { request_id: "q".into(), event: "user-questions/request".into() }
        );
    }

    #[test]
    fn emits_cancels_and_unknown_frames_do_not_end_the_stream() {
        assert_eq!(item(json!({"type":"emit","event":"api-session/activity","args":[]})), Forwarded::Emit { event: "api-session/activity".into() });
        assert_eq!(item(json!({"type":"cancel","eventId":"evt-1"})), Forwarded::Cancelled { request_id: "evt-1".into() });
        assert_eq!(item(json!({"type":"from-the-future","x":1})), Forwarded::Ignored);
        assert_eq!(item(json!({"type":"waterfall","event":"x","eventId":""})), Forwarded::Ignored);
    }

    #[test]
    fn the_wire_outcomes_match_the_dsh_enum() {
        // 名字来自 `ApprovalOutcome`（`packages/interaction/user-approval/src/types.ts`）。
        assert_eq!(outcome_value(ApprovalOutcome::AllowOnce), Answer::AllowOnce);
        assert_eq!(outcome_value(ApprovalOutcome::Reject), Answer::Reject);
        assert_eq!(outcome_value(ApprovalOutcome::Unknown), Answer::Reject);
    }
}
