//! 假 SDK 对端。
//!
//! 只在 `test-runtime` feature 下编译。它按 SDK 的行式 JSON-RPC 说话，
//! 并按场景吐出**与真实事件同形**的 `session.event` 通知。
//!
//! 为什么需要一个真的子进程而不是纯内存替身：超时、取消与「进程提前退出」
//! 这三类行为只有在真的进程边界上才成立 —— 内存替身永远退不出、也永远
//! 杀不掉，于是那几条验收会退化成一个自己跟自己对答的测试。
//!
//! **事件形状是 M1 的约定，不是对真实内核的断言。** M2 接真实 DSH 时，
//! 以 `REFS/deepseek-harness` 的 `SessionEventMap` 为准核对这里，对不上的
//! 改投影与夹具，不要改判据。

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{watch, Mutex};

/// 环境变量：夹具用它伪造一条与任务验收命令匹配的工具记录。
///
/// 夹具不可能自己知道任务书里的命令，所以由主控在拉起夹具时显式传入。
pub const FIXTURE_COMMAND_ENV: &str = "ARC_DECK_FIXTURE_COMMAND";

/// 夹具场景。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// 正常收敛：一轮、有权威正文、有一致的工具记录、`turn/end completed`。
    Success,
    /// 内核报错结束。
    Failure,
    /// 需要主控介入。
    Blocked,
    /// 响应完 prompt 就退出进程，不发 terminal。
    Eof,
    /// 在 stdout 上写一行非 JSON。SDK 独占 stdout，所以这是协议违规。
    BadFrame,
    /// 用错位的 id 响应，制造「响应与请求对不上」。
    WrongId,
    /// 流式与权威正文里都含中文，且一帧的写入被拆在汉字中间。
    SplitUtf8,
    /// 只流式、不发 terminal —— 用于超时。
    Hang,
    /// 同 `Hang`，用于取消。
    Cancel,
    /// 正常收工，但验收命令非零退出 —— 验证「验收失败」在状态面上是一条
    /// 可判定的原因，而不是只有一个 `failed`。
    FailingAcceptance,
    /// 中途失败一次（限流），运行时自己重试后成功收工 —— 断网/限流之后的常态。
    /// 用来钉住「报错但自起了不算失败」。
    RetryThenSucceed,
    /// 轮次以结构化错误收尾：`reason.error = {code:"QUOTA", status:402}`。
    /// 真实会话就是这么写的（`Insufficient Balance`），分类要用结构化字段而不是猜英文。
    Quota,
}

impl Scenario {
    pub const ALL: &'static [&'static str] = &[
        "success",
        "failure",
        "blocked",
        "eof",
        "bad-frame",
        "wrong-id",
        "split-utf8",
        "hang",
        "cancel",
        "failing-acceptance",
        "retry-then-succeed",
        "quota",
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "success" => Scenario::Success,
            "failure" => Scenario::Failure,
            "blocked" => Scenario::Blocked,
            "eof" => Scenario::Eof,
            "bad-frame" => Scenario::BadFrame,
            "wrong-id" => Scenario::WrongId,
            "split-utf8" => Scenario::SplitUtf8,
            "hang" => Scenario::Hang,
            "cancel" => Scenario::Cancel,
            "failing-acceptance" => Scenario::FailingAcceptance,
            "retry-then-succeed" => Scenario::RetryThenSucceed,
            "quota" => Scenario::Quota,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Scenario::Success => "success",
            Scenario::Failure => "failure",
            Scenario::Blocked => "blocked",
            Scenario::Eof => "eof",
            Scenario::BadFrame => "bad-frame",
            Scenario::WrongId => "wrong-id",
            Scenario::SplitUtf8 => "split-utf8",
            Scenario::Hang => "hang",
            Scenario::Cancel => "cancel",
            Scenario::FailingAcceptance => "failing-acceptance",
            Scenario::RetryThenSucceed => "retry-then-succeed",
            Scenario::Quota => "quota",
        }
    }
}

const FIXTURE_SESSION: &str = "fixture-session";

/// 跑一个夹具对端，直到 stdin 关闭或场景要求退出。
pub async fn serve<R, W>(scenario: Scenario, reader: R, writer: W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let writer = Arc::new(Mutex::new(writer));
    let (done_tx, done_rx) = watch::channel(false);
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message.get("method").and_then(|m| m.as_str()).unwrap_or_default();
        let id = message.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => {
                if scenario == Scenario::WrongId {
                    let wrong = id.as_u64().unwrap_or(0) + 1;
                    write_line(
                        &writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": wrong,
                            "result": { "serverInfo": server_info() }
                        }),
                    )
                    .await?;
                } else {
                    respond(&writer, id, json!({ "serverInfo": server_info() })).await?;
                    if scenario == Scenario::BadFrame {
                        // SDK 独占 stdout：非 JSON 行就是违规，worker 必须报出来。
                        write_bytes(&writer, "这不是 一行 JSON\n".as_bytes()).await?;
                    }
                }
            }
            "session/prompt" => {
                respond(&writer, id, json!({ "messageId": "fixture-message-1" })).await?;
                if scenario == Scenario::Eof {
                    // 响应完就退出：worker 应该判「缺 terminal」而不是成功。
                    return Ok(());
                }
                let w = Arc::clone(&writer);
                let done = done_tx.clone();
                tokio::spawn(async move {
                    emit(scenario, &w).await;
                    let _ = done.send(true);
                });
            }
            "session/wait" => {
                // 不能在这个循环里等 —— 那样会卡住读循环，后续请求（含 shutdown）
                // 永远读不到。放到独立任务里等。
                let w = Arc::clone(&writer);
                let mut rx = done_rx.clone();
                tokio::spawn(async move {
                    while !*rx.borrow_and_update() {
                        if rx.changed().await.is_err() {
                            break;
                        }
                    }
                    let _ = respond(&w, id, json!({})).await;
                });
            }
            "shutdown" => {
                respond(&writer, id, json!({})).await?;
                return Ok(());
            }
            _ => {
                respond_error(&writer, id, -32601, "fixture 不支持这个方法").await?;
            }
        }
    }
    Ok(())
}

fn server_info() -> Value {
    // 刻意不冒充真实内核：`name` 与 `version` 都标出它是对端替身。
    json!({ "name": "arc-deck-fixture-sdk", "version": "fixture" })
}

/// 发一帧事件，自动补 `seq`。
macro_rules! send_event {
    ($writer:expr, $seq:ident, $event:expr) => {{
        $seq += 1;
        let mut event = $event;
        event["seq"] = serde_json::json!($seq);
        let _ = emit_event($writer, event).await;
    }};
}

async fn emit<W>(scenario: Scenario, writer: &Arc<Mutex<W>>)
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut seq: u64 = 0;

    match scenario {
        Scenario::Success => {
            send_event!(writer, seq, json!({"type":"turn/start","turn":1}));
            // 系统来源的一条不该进对话 —— 这条就是用来验证那件事的。
            send_event!(writer, seq, json!({"type":"user/message","turn":1,"source":{"kind":"system"},
                "message":{"content":[{"type":"text","text":"你是……（系统提示词，不该出现在对话里）"}]}}));
            send_event!(writer, seq, json!({"type":"user/message","turn":1,"source":{"kind":"user"},
                "message":{"content":[{"type":"text","text":"离线夹具指令"}]}}));
            send_event!(writer, seq, json!({"type":"assistant/delta","turn":1,"step":1,
                "chunk":{"type":"text-delta","text":"离"}}));
            send_event!(writer, seq, json!({"type":"assistant/delta","turn":1,"step":1,
                "chunk":{"type":"text-delta","text":"线完成"}}));
            if let Some(command) = fixture_command() {
                let arguments =
                    json!({"command": command, "cwd": ".", "timeout_ms": 120000}).to_string();
                send_event!(writer, seq, json!({"type":"tool/call","turn":1,"step":1,
                    "callId":"fixture-call-1","name":"shell","arguments":arguments}));
                send_event!(writer, seq, json!({"type":"tool/result","turn":1,"step":1,
                    "callId":"fixture-call-1",
                    "message":{"isError":false,"content":[{"type":"text","text":"夹具命令输出"}]},
                    "meta":{"exitCode":0}}));
            }
            send_event!(writer, seq, json!({"type":"assistant/message","turn":1,"step":1,
                "message":{"content":[{"type":"text","text":"离线完成"}]},
                "usage":{"promptTokens":11,"completionTokens":2,"totalTokens":13}}));
            send_event!(writer, seq, json!({"type":"turn/end","turn":1,"reason":{"kind":"completed"}}));
        }
        Scenario::Failure => {
            send_event!(writer, seq, json!({"type":"turn/start","turn":1}));
            // 模型失败的唯一出口：finish.reason.failure.message。
            // 它不会抛错，只会出现在这里 —— 界面上表现为「发出去什么都没有」。
            send_event!(writer, seq, json!({"type":"assistant/attempt","turn":1,"step":1,
                "stream":[{"chunk":{"finish":{"reason":{"failure":{
                    "message":"The supported API model names are deepseek-flash, deepseek-v4-pro, but you passed deepseek-official."
                }}}}}]}));
            send_event!(writer, seq, json!({"type":"turn/end","turn":1,
                "reason":{"kind":"error","error":{"message":"模型调用失败"}}}));
        }
        Scenario::Blocked => {
            send_event!(writer, seq, json!({"type":"turn/start","turn":1}));
            send_event!(writer, seq, json!({"type":"user/message","turn":1,"source":{"kind":"user"},
                "message":{"content":[{"type":"text","text":"离线夹具指令"}]}}));
            send_event!(writer, seq, json!({"type":"tool/call","turn":1,"step":1,
                "callId":"fixture-question","name":"question",
                "arguments":"{\"question\":\"需要主控决定的事\"}"}));
            send_event!(writer, seq, json!({"type":"turn/end","turn":1,
                "reason":{"kind":"blocked","message":"没有人能回答这个问题"}}));
        }
        Scenario::SplitUtf8 => {
            send_event!(writer, seq, json!({"type":"turn/start","turn":1}));
            seq += 1;
            let line = event_line(json!({"type":"assistant/delta","turn":1,"step":1,"seq":seq,
                "chunk":{"type":"text-delta","text":"离线"}}));
            let _ = write_split(writer, &line).await;
            seq += 1;
            let line = event_line(json!({"type":"assistant/message","turn":1,"step":1,"seq":seq,
                "message":{"content":[{"type":"text","text":"离线完成"}]}}));
            // 关键一帧：把一个汉字拆到两次写之间。
            let _ = write_split(writer, &line).await;
            send_event!(writer, seq, json!({"type":"turn/end","turn":1,"reason":{"kind":"completed"}}));
        }
        Scenario::Eof | Scenario::BadFrame | Scenario::WrongId => {
            // 这三个在 prompt 之前就已经把结论给出了。
        }
        Scenario::Hang | Scenario::Cancel => {
            send_event!(writer, seq, json!({"type":"turn/start","turn":1}));
            for _ in 0..600u32 {
                send_event!(writer, seq, json!({"type":"assistant/delta","turn":1,"step":1,
                    "chunk":{"type":"text-delta","text":"·"}}));
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            // 走到这里说明没人管它；仍然不发 terminal。
        }
        Scenario::FailingAcceptance => {
            send_event!(writer, seq, json!({"type":"turn/start","turn":1}));
            if let Some(command) = fixture_command() {
                let arguments =
                    json!({"command": command, "cwd": ".", "timeout_ms": 120000}).to_string();
                send_event!(writer, seq, json!({"type":"tool/call","turn":1,"step":1,
                    "callId":"fixture-call-1","name":"shell","arguments":arguments}));
                // `meta.exitCode` 与正文里的 `[exit code: N]` 都给上：前者是夹具的
                // 造法，后者是真实 DSH shell 工具的形状，两边都要走得通。
                send_event!(writer, seq, json!({"type":"tool/result","turn":1,"step":1,
                    "callId":"fixture-call-1",
                    "message":{"isError":false,"content":[{"type":"text","text":"夹具命令输出\n[exit code: 101]"}]},
                    "meta":{"exitCode":101}}));
            }
            send_event!(writer, seq, json!({"type":"assistant/message","turn":1,"step":1,
                "message":{"content":[{"type":"text","text":"我以为做完了"}]},
                "usage":{"promptTokens":11,"completionTokens":2,"totalTokens":13}}));
            send_event!(writer, seq, json!({"type":"turn/end","turn":1,"reason":{"kind":"completed"}}));
        }
        Scenario::RetryThenSucceed => {
            send_event!(writer, seq, json!({"type":"turn/start","turn":1}));
            send_event!(writer, seq, json!({"type":"assistant/attempt","turn":1,"step":1,
                "stream":[{"chunk":{"finish":{"reason":{"failure":{"message":"429 Too Many Requests: exhausted tpm limit"}}}}}]}));
            send_event!(writer, seq, json!({"type":"assistant/attempt","turn":1,"step":1,
                "stream":[{"chunk":{"finish":{"reason":{"kind":"stop"}}}}]}));
            send_event!(writer, seq, json!({"type":"assistant/message","turn":1,"step":1,
                "message":{"content":[{"type":"text","text":"重试之后好了"}]},
                "usage":{"promptTokens":11,"completionTokens":2,"totalTokens":13}}));
            send_event!(writer, seq, json!({"type":"turn/end","turn":1,"reason":{"kind":"completed"}}));
        }
        Scenario::Quota => {
            send_event!(writer, seq, json!({"type":"turn/start","turn":1}));
            send_event!(writer, seq, json!({"type":"assistant/attempt","turn":1,"step":1,
                "stream":[{"chunk":{"finish":{"reason":{"kind":"error",
                    "failure":{"message":"Insufficient Balance"}}}}}]}));
            send_event!(writer, seq, json!({"type":"turn/end","turn":1,
                "reason":{"kind":"error","error":{"message":"Insufficient Balance","code":"QUOTA","status":402}}}));
        }
    }
}

fn fixture_command() -> Option<String> {
    std::env::var(FIXTURE_COMMAND_ENV).ok().filter(|v| !v.trim().is_empty())
}

fn event_line(event: Value) -> String {
    let frame = json!({
        "jsonrpc": "2.0",
        "method": "session.event",
        "params": { "sessionId": FIXTURE_SESSION, "event": event }
    });
    let mut line = serde_json::to_string(&frame).unwrap_or_default();
    line.push('\n');
    line
}

async fn emit_event<W>(writer: &Arc<Mutex<W>>, event: Value) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    write_bytes(writer, event_line(event).as_bytes()).await
}

async fn write_line<W>(writer: &Arc<Mutex<W>>, frame: &Value) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_string(frame).unwrap_or_default();
    line.push('\n');
    write_bytes(writer, line.as_bytes()).await
}

async fn respond<W>(writer: &Arc<Mutex<W>>, id: Value, result: Value) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    write_line(writer, &json!({ "jsonrpc": "2.0", "id": id, "result": result })).await
}

async fn respond_error<W>(
    writer: &Arc<Mutex<W>>,
    id: Value,
    code: i64,
    message: &str,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    write_line(writer, &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
        .await
}

async fn write_bytes<W>(writer: &Arc<Mutex<W>>, bytes: &[u8]) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut guard = writer.lock().await;
    guard.write_all(bytes).await?;
    guard.flush().await
}

/// 把一行在中途拆成两次写，切口落在一个多字节字符内部。
///
/// 这是 P12 那个 bug 的复现装置：实现若在每次 `read` 的字节上各自解码，
/// 这里就必然产出一个替换字符。**必须按字节写** —— 把 `tail` 当成 `&str`
/// 会在切口恰好落在字符中间时丢掉整段（它本来就不是合法 UTF-8）。
async fn write_split<W>(writer: &Arc<Mutex<W>>, line: &str) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let bytes = line.as_bytes();
    let Some(split) = find_three_byte_sequence(bytes) else {
        return write_bytes(writer, bytes).await;
    };
    let (head, tail) = bytes.split_at(split);
    write_bytes(writer, head).await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    write_bytes(writer, tail).await
}

fn find_three_byte_sequence(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let len = if b < 0x80 {
            1
        } else if b >> 5 == 0b110 {
            2
        } else if b >> 4 == 0b1110 {
            3
        } else if b >> 3 == 0b11110 {
            4
        } else {
            1
        };
        if len == 3 && i + 1 < bytes.len() {
            return Some(i + 1);
        }
        i += len;
    }
    None
}
