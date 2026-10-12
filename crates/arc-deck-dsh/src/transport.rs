//! 按行分帧的 JSON-RPC 传输。
//!
//! 两条踩过坑的纪律写在这里，改动时不要破坏：
//!
//! 1. **先攒齐一行再解码**。逐次 `read` 的字节切片上直接 `String::from_utf8`
//!    会把跨读边界的多字节字符撕成两个替换字符，而且字符串还多出一个字符。
//!    这里在**字节**上找 `\n`（`0x0A` 永远不是 UTF-8 续字节），攒齐一行后才解码。
//! 2. **请求按 `id` 关联**。收到一个没有对应在途请求的响应要能说出来 ——
//!    静默丢弃会让「响应错位」表现为「调用永远超时」，查起来完全是另一个方向。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, Mutex};

/// 传输层错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum DshError {
    #[error("协议错误：{0}")]
    Protocol(String),
    #[error("等待 `{0}` 超时")]
    Timeout(String),
    #[error("io 错误：{0}")]
    Io(String),
    #[error("远端返回错误：code={code} message={message}")]
    Remote { code: i64, message: String },
    /// 运行时进程在我们还在等的时候退出了。
    #[error("运行时进程已退出")]
    Exited,
}

/// 从读线程交回给 worker 的入站消息。
#[derive(Debug)]
pub enum Inbound {
    /// 服务端通知。
    Notification { method: String, params: Value },
    /// 服务端发起的请求。真实 DSH 从不下发，这里也照协议回 `-32601`。
    ServerRequest { id: Value, method: String },
    /// 收到一个没有在途请求与之对应的响应。**必须**让 worker 看见：
    /// 这就是「响应错位」的唯一可观察信号。
    UnmatchedResponse { id: Value },
    /// 一行不是合法 JSON 文本。SDK 独占 stdout，所以这是真的协议违规。
    ProtocolError { message: String },
    /// 对端关闭了输出。
    Eof,
    /// 读取出错。
    Fatal { message: String },
}

type Writer = Arc<Mutex<Box<dyn AsyncWrite + Unpin + Send>>>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, DshError>>>>>;

/// 传输句柄。可以被多处共享（读线程与 worker 都要写）。
pub struct Transport {
    writer: Writer,
    pending: Pending,
    next_id: AtomicU64,
}

impl Transport {
    /// 发一个请求并等它的响应。
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, DshError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_frame(&frame).await?;

        match tokio::time::timeout(timeout, rx).await {
            // 超时了也要把在途项撤掉，否则一个迟到的响应会被误判成「错位响应」。
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(DshError::Timeout(method.to_string()))
            }
            Ok(Err(_)) => Err(DshError::Exited),
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(err))) => Err(err),
        }
    }

    async fn write_frame(&self, frame: &Value) -> Result<(), DshError> {
        let mut line = serde_json::to_string(frame).map_err(|e| DshError::Protocol(e.to_string()))?;
        line.push('\n');
        let mut writer = self.writer.lock().await;
        writer.write_all(line.as_bytes()).await.map_err(|e| DshError::Io(e.to_string()))?;
        writer.flush().await.map_err(|e| DshError::Io(e.to_string()))
    }
}

/// 在一对读写流上启动传输。
///
/// 返回共享句柄与入站队列。所有入站消息都从这里出，worker 只需要 `select!` 它。
pub fn spawn<R, W>(reader: R, writer: W) -> (Arc<Transport>, mpsc::UnboundedReceiver<Inbound>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let shared: Writer = Arc::new(Mutex::new(Box::new(writer)));
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let (tx, rx) = mpsc::unbounded_channel();
    let transport = Arc::new(Transport {
        writer: Arc::clone(&shared),
        pending: Arc::clone(&pending),
        next_id: AtomicU64::new(1),
    });
    tokio::spawn(read_loop(reader, tx, pending, shared));
    (transport, rx)
}

async fn read_loop<R>(
    mut reader: R,
    tx: mpsc::UnboundedSender<Inbound>,
    pending: Pending,
    writer: Writer,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut buf = vec![0u8; 16 * 1024];
    // 未完成行的字节缓冲。**只有**看到 `\n` 才会解码，见本模块顶部第 1 条。
    let mut acc: Vec<u8> = Vec::new();

    loop {
        match reader.read(&mut buf).await {
            Ok(0) => {
                // 进程退出时在途请求必须**立刻**失败。留着不管的话，调用方会
                // 一路等到自己的超时，把「进程已经死了」报成「超时」——
                // 那是两个完全不同的结论。
                fail_all_pending(&pending, DshError::Exited).await;
                let _ = tx.send(Inbound::Eof);
                return;
            }
            Ok(n) => {
                acc.extend_from_slice(&buf[..n]);
                while let Some(pos) = acc.iter().position(|&b| b == b'\n') {
                    let mut line: Vec<u8> = acc.drain(..=pos).collect();
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    if line.iter().all(|b| b.is_ascii_whitespace()) {
                        continue;
                    }
                    dispatch_line(&line, &tx, &pending, &writer).await;
                }
            }
            Err(e) => {
                fail_all_pending(&pending, DshError::Io(e.to_string())).await;
                let _ = tx.send(Inbound::Fatal { message: e.to_string() });
                return;
            }
        }
    }
}

async fn fail_all_pending(pending: &Pending, error: DshError) {
    let mut guard = pending.lock().await;
    for (_, sender) in guard.drain() {
        let _ = sender.send(Err(error.clone()));
    }
}

async fn dispatch_line(
    line: &[u8],
    tx: &mpsc::UnboundedSender<Inbound>,
    pending: &Pending,
    writer: &Writer,
) {
    let text = match std::str::from_utf8(line) {
        Ok(text) => text,
        Err(e) => {
            let _ = tx.send(Inbound::ProtocolError {
                message: format!("一行不是合法 UTF-8：{e}"),
            });
            return;
        }
    };
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(e) => {
            let _ = tx.send(Inbound::ProtocolError {
                message: format!("一行不是合法 JSON：{e}"),
            });
            return;
        }
    };

    let id = value.get("id").cloned().filter(|v| !v.is_null());
    let method = value.get("method").and_then(|m| m.as_str()).map(str::to_string);

    match (id, method) {
        // 服务端请求：按协议回「方法不存在」。真实 DSH 不会发，但静默忽略
        // 会让对端一直等，比回错更难查。
        (Some(id), Some(method)) => {
            let _ = tx.send(Inbound::ServerRequest { id: id.clone(), method });
            let reply = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "Arc Deck 客户端不提供任何服务端请求方法" },
            });
            let mut text = serde_json::to_string(&reply).unwrap_or_default();
            text.push('\n');
            let mut guard = writer.lock().await;
            let _ = guard.write_all(text.as_bytes()).await;
            let _ = guard.flush().await;
        }
        // 响应：按 id 关联。
        (Some(id), None) => {
            let key = id.as_u64();
            let sender = match key {
                Some(key) => pending.lock().await.remove(&key),
                None => None,
            };
            match sender {
                Some(sender) => {
                    let result = match value.get("error") {
                        Some(error) => Err(DshError::Remote {
                            code: error.get("code").and_then(|c| c.as_i64()).unwrap_or(0),
                            message: error
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("远端返回了错误但没有 message")
                                .to_string(),
                        }),
                        None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = sender.send(result);
                }
                None => {
                    let _ = tx.send(Inbound::UnmatchedResponse { id });
                }
            }
        }
        // 通知。
        (None, Some(method)) => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let _ = tx.send(Inbound::Notification { method, params });
        }
        (None, None) => {
            let _ = tx.send(Inbound::ProtocolError {
                message: "一行既没有 id 也没有 method".to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn drive<F>(script: &str, run: F) -> Vec<Inbound>
    where
        F: for<'a> FnOnce(&'a Arc<Transport>, &'a mut mpsc::UnboundedReceiver<Inbound>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>>,
    {
        let (client, peer) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(client);
        let (transport, mut rx) = spawn(reader, writer);

        let payload = script.to_string();
        let peer_task = tokio::spawn(async move {
            let mut peer = peer;
            peer.write_all(payload.as_bytes()).await.unwrap();
            peer.flush().await.unwrap();
            // 保持连接开着，让读取端有机会处理完再收到 EOF。
            tokio::time::sleep(Duration::from_millis(80)).await;
        });

        run(&transport, &mut rx).await;
        peer_task.await.unwrap();

        let mut collected = Vec::new();
        while let Ok(item) = rx.try_recv() {
            collected.push(item);
        }
        collected
    }

    #[tokio::test]
    async fn notification_is_delivered_after_full_line() {
        let got = drive(
            "{\"jsonrpc\":\"2.0\",\"method\":\"session.status\",\"params\":{\"sessionId\":\"s1\",\"status\":\"running\"}}\n",
            |_t, _rx| Box::pin(async {}),
        )
        .await;
        let mut found = false;
        for item in &got {
            if let Inbound::Notification { method, params } = item {
                assert_eq!(method, "session.status");
                assert_eq!(params["sessionId"], "s1");
                found = true;
            }
        }
        assert!(found, "没收到通知：{got:?}");
    }

    #[tokio::test]
    async fn response_resolves_the_matching_request() {
        let got = drive(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n",
            |transport, _rx| Box::pin(async move {
                let value = transport
                    .call("initialize", json!({}), Duration::from_secs(2))
                    .await
                    .unwrap();
                assert_eq!(value["ok"], true);
            }),
        )
        .await;
        // 正常关联走的是一对一通道，不应该额外冒出「错位响应」。
        assert!(!got.iter().any(|i| matches!(i, Inbound::UnmatchedResponse { .. })), "{got:?}");
    }

    #[tokio::test]
    async fn response_with_unknown_id_is_reported() {
        let got = drive(
            "{\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{}}\n",
            |_t, _rx| Box::pin(async { tokio::time::sleep(Duration::from_millis(40)).await }),
        )
        .await;
        let unmatched: Vec<_> = got
            .iter()
            .filter_map(|i| match i {
                Inbound::UnmatchedResponse { id } => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(unmatched, vec![json!(99)], "{got:?}");
    }

    #[tokio::test]
    async fn non_json_line_is_a_protocol_error() {
        let got = drive(
            "这不是 JSON\n",
            |_t, _rx| Box::pin(async { tokio::time::sleep(Duration::from_millis(40)).await }),
        )
        .await;
        assert!(
            got.iter().any(|i| matches!(i, Inbound::ProtocolError { .. })),
            "{got:?}"
        );
    }

    #[tokio::test]
    async fn invalid_utf8_line_is_a_protocol_error() {
        let (client, peer) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let (_transport, mut rx) = spawn(reader, writer);
        let mut peer = peer;
        // 0xE4 后面跟一个非法续字节，再换行。
        peer.write_all(&[0xE4, 0x41, b'\n']).await.unwrap();
        peer.flush().await.unwrap();

        let item = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        match item {
            Inbound::ProtocolError { .. } => {}
            other => panic!("期望协议错误，得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_multibyte_character_split_across_reads_survives() {
        // 这条是回归测试：逐次 read 各自解码会把「到」撕成两个替换字符。
        let line = "{\"jsonrpc\":\"2.0\",\"method\":\"session.status\",\"params\":{\"text\":\"收到\"}}\n";
        let bytes = line.as_bytes();
        let split = find_three_byte_sequence(bytes).expect("测试载荷里应该有 3 字节 UTF-8 序列");

        let (client, peer) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let (_transport, mut rx) = spawn(reader, writer);

        let (head, tail) = bytes.split_at(split);
        let head = head.to_vec();
        let tail = tail.to_vec();
        let mut peer = peer;
        tokio::spawn(async move {
            peer.write_all(&head).await.unwrap();
            peer.flush().await.unwrap();
            // 故意在字符中间停一下，逼出「按读边界解码」的实现错误。
            tokio::time::sleep(Duration::from_millis(30)).await;
            peer.write_all(&tail).await.unwrap();
            peer.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(80)).await;
        });

        let item = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        match item {
            Inbound::Notification { params, .. } => {
                assert_eq!(params["text"], "收到", "中文被撕坏了：{params}");
                let text = params["text"].as_str().unwrap();
                assert!(!text.contains('\u{fffd}'), "出现替换字符：{text:?}");
            }
            other => panic!("期望通知，得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn server_request_gets_method_not_found() {
        let (client, mut peer) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let (_transport, mut rx) = spawn(reader, writer);

        peer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"server/ask\"}\n").await.unwrap();
        peer.flush().await.unwrap();

        let item = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(item, Inbound::ServerRequest { .. }), "{item:?}");

        let mut buf = vec![0u8; 256];
        let n = tokio::time::timeout(Duration::from_secs(2), tokio::io::AsyncReadExt::read(&mut peer, &mut buf))
            .await
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_slice(&buf[..n]).unwrap();
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn timeout_is_reported_when_nothing_answers() {
        let (client, peer) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let (transport, _rx) = spawn(reader, writer);
        // 对端保持安静，但连接不关。
        let _hold = peer;
        let err = transport
            .call("initialize", json!({}), Duration::from_millis(120))
            .await
            .unwrap_err();
        assert!(matches!(err, DshError::Timeout(_)), "{err}");
    }

    #[tokio::test]
    async fn pending_call_fails_immediately_when_the_peer_disconnects() {
        // 回归测试：对端退出时必须立刻让在途请求失败，而不是让调用方
        // 一路等到自己的超时 —— 那会把「进程已经死了」报成「超时」。
        //
        // 具体是 `Exited`（读端先看到 EOF）还是 `Io`（写端先撞上 broken pipe）
        // 取决于两个方向谁先被调度，两者都说明对端已经不在；不在这里赌顺序。
        let (client, peer) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let (transport, _rx) = spawn(reader, writer);
        drop(peer);

        let started = std::time::Instant::now();
        let err = transport
            .call("initialize", json!({}), Duration::from_secs(30))
            .await
            .unwrap_err();
        assert!(
            matches!(err, DshError::Exited | DshError::Io(_)),
            "期望「对端已不在」，得到 {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "应该立刻返回，实际用了 {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn remote_error_is_surfaced() {
        let got = drive(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32603,\"message\":\"boom\"}}\n",
            |transport, _rx| Box::pin(async move {
                let err = transport
                    .call("initialize", json!({}), Duration::from_secs(2))
                    .await
                    .unwrap_err();
                match err {
                    DshError::Remote { code, message } => {
                        assert_eq!(code, -32603);
                        assert_eq!(message, "boom");
                    }
                    other => panic!("期望远端错误，得到 {other}"),
                }
            }),
        )
        .await;
        // 对端随后关闭，`Eof` 是正常的；要断言的是没有别的入站消息。
        assert!(
            !got.iter().any(|i| matches!(
                i,
                Inbound::ProtocolError { .. } | Inbound::UnmatchedResponse { .. }
            )),
            "关联错误不该再产生异常入站消息：{got:?}"
        );
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
}
