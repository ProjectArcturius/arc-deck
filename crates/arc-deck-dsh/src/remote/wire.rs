//! Remote 层的**线上形状**：一元调用的信封、mux 的帧、失败结构。
//!
//! 这里是纯编解码，不碰 IO —— 这样「信封对不对」可以在没有内核、没有 socket
//! 的情况下被断言。协议出处逐条写在 `docs/REMOTE_0.2.1.md`，不在这里复述。
//!
//! 两条**必须逐字照抄**的约束（0.2.1 源码核对过，不是照抄 C#）：
//!
//! 1. `payload` **有且只有**一个 `args` 字段，且它必须是普通对象。
//!    Gateway 那一侧先查 `Reflect.ownKeys(payload).length !== 1`，再查 `args` 是不是
//!    普通对象（`api/gateway/src/index.ts:1138-1146`）—— 多一个字段、少一个字段、
//!    或者把 args 写成数组，都在**校验参数名之前**就被拒。
//! 2. `args` 里的字段名要与 Host 形参**逐字**一致，多一个少一个都报
//!    `gateway/arguments-invalid`（`api/gateway/src/index.ts:1478-1504`）。
//!    所以 `session/list` 传 `{"_request": {}}`（形参就叫 `_request`），
//!    而 `session/follow` 传 `{"request": {…}}` —— 两者不通用。

use serde_json::{json, Value};

use super::RemoteError;
// `RemoteFailure` 定义在 `mod.rs`（它是 `RemoteError` 的载荷），这里**转出去**
// 而不是复制一份：曾经的写法是 `pub use wire::RemoteFailure`，而 wire 那边只有
// 一个私有 `use`，于是 crate 直接编不过（E0603）—— 上一个工人是在链接前被
// ENOSPC 打断的，这一条没被编译过。
pub use super::RemoteFailure;

/// mux 的 WebSocket 路径（`packages/api/gateway/src/stream-protocol.ts:7`）。
pub const MUX_PATH: &str = "/api/remote.mux";

/// 一元调用的路径前缀（`packages/client/connection/src/api-path.ts`）。
pub const API_PATH: &str = "/api";

/// 一元方法名的形状：**恰好两段** `<ns>/<method>`。
///
/// 段数不是「至少两段」而是「正好两段」：Gateway 在 `remoteRequest` 里先
/// `endpoint.split('/')` 再查 `segments.length !== 2`（`api/gateway/src/index.ts:1133-1136`，
/// 同一个判断在 `index.ts:327-328` 的路径解析里也有），`session` 与 `a/b/c` 都
/// 在查参数名之前就被拒（`invalid Remote endpoint`）。
///
/// 每段的字符集来自 `ENDPOINT_SEGMENT_PATTERN`（`connection/src/rpc-host.ts:36`）。
/// `.` / `..` 被显式排掉：它们是路径段，进了 URL 会被 HTTP 层重新解释。
pub fn valid_method(method: &str) -> bool {
    let mut segments = method.split('/');
    let (Some(namespace), Some(name), None) = (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    [namespace, name].into_iter().all(|segment| {
        !matches!(segment, "" | "." | "..")
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | b'.' | b'-'))
    })
}

/// `args` 必须是普通对象 —— 见本模块顶部第 1 条。
fn ensure_args_object(args: &Value) -> Result<(), RemoteError> {
    if args.is_object() {
        Ok(())
    } else {
        Err(RemoteError::Protocol(format!(
            "args 必须是普通对象，收到 {}",
            kind_of(args)
        )))
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "布尔",
        Value::Number(_) => "数字",
        Value::String(_) => "字符串",
        Value::Array(_) => "数组",
        Value::Object(_) => "对象",
    }
}

/// 一元调用的请求信封。
///
/// 形状来自 `client/connection/src/rpc.ts:80-85`（`ClientRequest`）与
/// `rpc-schema.ts:35-40`（线上校验）：`type` 只能是 `client-request`，
/// `payload` 是端点自己的负载（这里就是那个只剩 `args` 的对象）。
pub fn encode_call(rpc_id: &str, method: &str, args: &Value) -> Result<Value, RemoteError> {
    ensure_args_object(args)?;
    Ok(json!({
        "type": "client-request",
        "rpcId": rpc_id,
        "method": method,
        "payload": { "args": args },
    }))
}

/// 一元调用的响应信封 → 成功值，或**结构化**的远端失败。
///
/// 响应形状：`client/connection/src/rpc-schema.ts:43-47` 的 `serverResponseSchema`，
/// 其中 `result` 是 `{ok:true, value}` 或 `{ok:false, error:{code,message,details}}`。
///
/// `ok:true` 但**没有** `value` 字段是合法的：上游用「字段缺席」表示「没有业务返回值」
/// （`api/gateway/src/index.ts:689-692` 的注释）。这里把它读成 `null`，不是错误。
pub fn decode_response(expected_rpc_id: &str, body: &Value) -> Result<Value, RemoteError> {
    let object = body
        .as_object()
        .ok_or_else(|| RemoteError::Protocol("响应不是一个 JSON 对象".into()))?;

    match object.get("type").and_then(Value::as_str) {
        Some("server-response") => {}
        other => {
            return Err(RemoteError::Protocol(format!(
                "响应的 type 是 {other:?}，期望 \"server-response\""
            )));
        }
    }

    match object.get("rpcId").and_then(Value::as_str) {
        Some(id) if id == expected_rpc_id => {}
        other => {
            // 关联错位必须说出来。静默接受一个别人的响应，会把「响应错位」
            // 表现成「值不对」，查起来完全是另一个方向。
            return Err(RemoteError::Protocol(format!(
                "响应的 rpcId 是 {other:?}，期望 {expected_rpc_id:?}"
            )));
        }
    }

    let result = object
        .get("result")
        .and_then(Value::as_object)
        .ok_or_else(|| RemoteError::Protocol("响应缺少 result 对象".into()))?;

    match result.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(result.get("value").cloned().unwrap_or(Value::Null)),
        Some(false) => Err(RemoteError::Remote(decode_failure(result.get("error")))),
        None => Err(RemoteError::Protocol(
            "响应的 result 既不是 ok:true 也不是 ok:false".into(),
        )),
    }
}

/// 失败结构。`details` 在线上是**必需对象**（`rpc-schema.ts:10-14`），
/// 但这里对缺席/非对象**不报错**：一条 error 里最要紧的是 code 与 message，
/// 因为 details 长得不对就把整条错误降级成「协议错误」，是丢掉更多信息。
pub fn decode_failure(error: Option<&Value>) -> RemoteFailure {
    let Some(object) = error.and_then(Value::as_object) else {
        return RemoteFailure {
            code: "unknown".into(),
            message: "内核回了失败，但没有 error 对象".into(),
            details: json!({}),
        };
    };
    RemoteFailure {
        code: object
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        message: object
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("（内核回了失败但没有 message）")
            .to_string(),
        details: match object.get("details") {
            Some(Value::Object(map)) => Value::Object(map.clone()),
            _ => json!({}),
        },
    }
}

/// 客户端 → 服务端的 `open` 帧（`api/gateway/src/stream-protocol.ts:277-283`）。
pub fn encode_open(
    stream_id: &str,
    endpoint: &str,
    args: &Value,
) -> Result<Value, RemoteError> {
    ensure_args_object(args)?;
    Ok(json!({
        "type": "open",
        "streamId": stream_id,
        "endpoint": endpoint,
        "payload": { "args": args },
    }))
}

/// 客户端 → 服务端的 `cancel` 帧（`stream-protocol.ts:266-269`）。
///
/// `cancel` / `end` 都要求**恰好** `type` 与 `streamId` 两个字段 —— 多一个就整条
/// WebSocket 被以 1008 关掉（`stream-server.ts:155-159`）。所以这里只有两个键。
pub fn encode_cancel(stream_id: &str) -> Value {
    json!({ "type": "cancel", "streamId": stream_id })
}

/// 服务端 → 客户端的一帧（`stream-protocol.ts:254-257` 的 `RemoteStreamServerMessage`）。
#[derive(Debug, Clone, PartialEq)]
pub enum ServerFrame {
    Item {
        stream_id: String,
        /// `value` 可以缺席（`stream-protocol.ts:295-299`）。缺席读成 `null`。
        value: Value,
    },
    /// 这条流正常结束。**之后不会再有帧**（`stream-server.ts:256-260`）。
    End { stream_id: String },
    /// 这条流以失败结束，**之后同样不会再有帧** —— 0.2.1 里 `error` 即是终止帧，
    /// 后面不再跟一个 `end`（`stream-server.ts:252-255` 直接 return）。
    Failure {
        stream_id: String,
        failure: RemoteFailure,
    },
    /// 认不出的帧。
    ///
    /// **不因此关闭连接**：协议没有版本协商（AGENTS.md P2），多出来的帧型不该让
    /// 整条连接陪葬。但也不静默丢掉 —— 叫调用方看得见。
    Unrecognized { detail: String },
}

/// 解一帧。传入的必须是**一条完整的文本消息**（tungstenite 已按消息组帧，
/// 见 `mux.rs` 顶部第 2 条；分片边界不会落到这里来）。
pub fn decode_frame(text: &str) -> ServerFrame {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return ServerFrame::Unrecognized { detail: "帧不是合法 JSON".into() };
    };
    let Some(object) = value.as_object() else {
        return ServerFrame::Unrecognized { detail: "帧不是 JSON 对象".into() };
    };
    let Some(stream_id) = object.get("streamId").and_then(Value::as_str) else {
        return ServerFrame::Unrecognized { detail: "帧没有 streamId".into() };
    };
    let stream_id = stream_id.to_string();

    match object.get("type").and_then(Value::as_str) {
        Some("item") => ServerFrame::Item {
            stream_id,
            value: object.get("value").cloned().unwrap_or(Value::Null),
        },
        Some("end") => ServerFrame::End { stream_id },
        Some("error") => ServerFrame::Failure {
            stream_id,
            failure: decode_failure(object.get("error")),
        },
        other => ServerFrame::Unrecognized {
            detail: format!("认不出的帧型 {other:?}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_envelope_has_exactly_one_args_field() {
        let frame = encode_call("r1", "session/list", &json!({ "_request": {} })).unwrap();
        assert_eq!(frame["type"], "client-request");
        assert_eq!(frame["rpcId"], "r1");
        assert_eq!(frame["method"], "session/list");
        // payload 的键集合必须**恰好**是 {args}：多一个都会被 Gateway 拒。
        let payload = frame["payload"].as_object().unwrap();
        assert_eq!(payload.len(), 1);
        assert_eq!(payload["args"]["_request"], json!({}));
    }

    #[test]
    fn non_object_args_are_refused_before_hitting_the_wire() {
        let err = encode_call("r1", "session/list", &json!([])).unwrap_err();
        assert!(matches!(err, RemoteError::Protocol(_)), "{err}");
        let err = encode_open("s1", "workspace/follow", &Value::Null).unwrap_err();
        assert!(matches!(err, RemoteError::Protocol(_)), "{err}");
    }

    #[test]
    fn success_response_yields_the_value() {
        let body = json!({
            "type": "server-response",
            "rpcId": "r1",
            "result": { "ok": true, "value": { "items": [] } },
        });
        assert_eq!(decode_response("r1", &body).unwrap(), json!({ "items": [] }));
    }

    #[test]
    fn absent_value_is_null_not_an_error() {
        // 上游用「字段缺席」表示「没有业务返回值」。
        let body = json!({ "type": "server-response", "rpcId": "r1", "result": { "ok": true } });
        assert_eq!(decode_response("r1", &body).unwrap(), Value::Null);
    }

    #[test]
    fn failure_keeps_code_message_and_details() {
        let body = json!({
            "type": "server-response",
            "rpcId": "r1",
            "result": { "ok": false, "error": {
                "code": "gateway/arguments-invalid",
                "message": "args fields do not match the descriptor: missing \"_request\"",
                "details": { "endpoint": "session/list" },
            } },
        });
        match decode_response("r1", &body).unwrap_err() {
            RemoteError::Remote(failure) => {
                assert_eq!(failure.code, "gateway/arguments-invalid");
                assert!(failure.message.contains("_request"), "{}", failure.message);
                assert_eq!(failure.details["endpoint"], "session/list");
            }
            other => panic!("期望结构化远端失败，得到 {other}"),
        }
    }

    #[test]
    fn a_mismatched_rpc_id_is_a_protocol_error() {
        let body = json!({
            "type": "server-response",
            "rpcId": "someone-else",
            "result": { "ok": true, "value": 1 },
        });
        let err = decode_response("r1", &body).unwrap_err();
        assert!(matches!(err, RemoteError::Protocol(_)), "{err}");
    }

    #[test]
    fn open_and_cancel_frames_match_the_0_2_1_shapes() {
        let open = encode_open("s1", "session/follow", &json!({ "request": {} })).unwrap();
        assert_eq!(
            open,
            json!({
                "type": "open",
                "streamId": "s1",
                "endpoint": "session/follow",
                "payload": { "args": { "request": {} } },
            })
        );
        // cancel 只能有 type 与 streamId 两个键：多一个会让内核以 1008 关掉整条连接。
        let cancel = encode_cancel("s1");
        assert_eq!(cancel.as_object().unwrap().len(), 2);
        assert_eq!(cancel, json!({ "type": "cancel", "streamId": "s1" }));
    }

    #[test]
    fn server_frames_decode_by_type() {
        assert_eq!(
            decode_frame(r#"{"type":"item","streamId":"s1","value":7}"#),
            ServerFrame::Item { stream_id: "s1".into(), value: json!(7) }
        );
        assert_eq!(
            decode_frame(r#"{"type":"item","streamId":"s1"}"#),
            ServerFrame::Item { stream_id: "s1".into(), value: Value::Null }
        );
        assert_eq!(decode_frame(r#"{"type":"end","streamId":"s1"}"#), ServerFrame::End {
            stream_id: "s1".into()
        });
        match decode_frame(
            r#"{"type":"error","streamId":"s1","error":{"code":"c","message":"m","details":{}}}"#,
        ) {
            ServerFrame::Failure { stream_id, failure } => {
                assert_eq!(stream_id, "s1");
                assert_eq!(failure.code, "c");
                assert_eq!(failure.message, "m");
            }
            other => panic!("期望失败帧，得到 {other:?}"),
        }
    }

    #[test]
    fn unknown_frames_are_reported_not_swallowed() {
        // 没有 streamId：认不出，但要说得出来。
        assert!(matches!(
            decode_frame(r#"{"type":"item","value":1}"#),
            ServerFrame::Unrecognized { .. }
        ));
        // 新帧型：同样如实报出来，而不是当成空帧丢掉。
        match decode_frame(r#"{"type":"who-knows","streamId":"s1"}"#) {
            ServerFrame::Unrecognized { detail } => assert!(detail.contains("who-knows"), "{detail}"),
            other => panic!("期望认不出，得到 {other:?}"),
        }
        assert!(matches!(
            decode_frame("这不是 JSON"),
            ServerFrame::Unrecognized { .. }
        ));
    }

    #[test]
    fn failure_without_details_still_keeps_code_and_message() {
        let failure = decode_failure(Some(&json!({ "code": "x", "message": "y" })));
        assert_eq!(failure.code, "x");
        assert_eq!(failure.message, "y");
        assert_eq!(failure.details, json!({}));
    }

    #[test]
    fn method_shape_matches_the_gateway_segment_rule() {
        assert!(valid_method("session/list"));
        assert!(valid_method("$events/result"));
        assert!(!valid_method("session"));
        assert!(!valid_method("session/"));
        assert!(!valid_method("/list"));
        assert!(!valid_method("session/../etc"));
        assert!(!valid_method("session/list?x=1"));
    }
}
