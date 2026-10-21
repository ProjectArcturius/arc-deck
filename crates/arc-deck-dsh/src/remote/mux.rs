//! mux：**一条 WebSocket 上的多条逻辑流**（`/api/remote.mux`）。
//!
//! ## 为什么这里用的是 tungstenite 的同步 API
//!
//! `tokio-tungstenite` 对外**只**实现 `Stream` / `Sink`，没有任何 inherent 的
//! `next()` / `send()`（`tokio-tungstenite-0.30.0/src/lib.rs:297,339`），
//! 而这两个 trait 来自 `futures-core` / `futures-sink` —— 要用它们就得把
//! `futures-util` 加成本 crate 的直接依赖，而这次任务明确不许再引新依赖。
//!
//! 所以这里把**同一个 crate 重新导出的** `tungstenite` 同步 API
//! （`tokio_tungstenite::tungstenite`，见 `lib.rs:13`）放在**专用线程**上驱动：
//! 线程自己拥有连接，用短读超时轮询「有没有要发的帧」与「有没有读到的帧」。
//! 对外仍然是 `async` 接口（帧经 tokio 的 channel 进出），所以调用方看不出来。
//!
//! 读超时**不会撕坏帧**：`FrameCodec` 把已经解析出来的帧头存在 `self.header` 里、
//! 半截载荷留在 `in_buffer` 里，`WouldBlock` 原样上抛
//! （`tungstenite-0.30.0/src/protocol/frame/mod.rs:170-208,234-241`），
//! 下一次 `read()` 从断点续上。`WouldBlock` 也不会被 `check_connection_reset`
//! 改写（`protocol/mod.rs:835-848`）。
//!
//! ## 三条纪律（都是踩过的坑）
//!
//! 1. **先登记信箱再发 `open`。** 反过来的话，一条回得很快的流（快照就是第一个
//!    item）可能在登记之前就到了，那一帧被当成「没有登记的流」丢掉 ——
//!    症状是「流打开了但一条都不来」。
//! 2. **每条流一个独立队列，队列无界。** 共用一个队列在只有一条流时**完全看不出来**；
//!    而 `open` 与调用方第一次 `next()` 之间到达的帧不能丢。
//! 3. **连接断了要让每一条流都收到明确的结束原因。** 静默挂住是最难查的一种失败：
//!    调用方看不出「内核还在想」与「连接早没了」的区别。
//!
//! ## 心跳
//!
//! Host 每 `websocketHeartbeatIntervalMs`（默认 2 秒）发一个 **Ping 控制帧**，
//! 连着两次没收到 Pong 就 `terminate()`
//! （`api/gateway/src/stream-server.ts:40,105-124`）—— 也就是大约 6 秒没有 Pong
//! 就会掉线。Pong 由 tungstenite 在 `read()` 里自动排队并刷出
//! （`protocol/mod.rs:245-250,668-675`），所以只要读循环在跑就没事；这一点
//! 也是「不许拿读超时当停止条件、读循环必须一直转到连接结束」的理由。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::client::ClientRequestBuilder;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocket};
use tokio_tungstenite::tungstenite::Error as WsError;

use super::wire::{self, ServerFrame};
use super::{RemoteError, RemoteFailure, Secret};

/// 连接的握手上限。
///
/// **必须有**：没有它的话，握手被拒（cookie 不被接受之类）会让调用方**永久挂住**，
/// 而那种卡死看起来像「内核在思考」。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// 读一次的超时。它同时是「出站帧最多延迟多久发出去」的上界。
///
/// 取 20ms：足够小，让人感觉不到 `open`/`cancel` 的延迟；足够大，不会把
/// 一个空闲连接变成忙等。
const READ_POLL: Duration = Duration::from_millis(20);

/// 最近若干条诊断（收/发/丢），有上限。
///
/// 留着它的理由：排查「流开了但一个 item 都没有」时，唯一能问的问题就是
/// 「线上到底有没有东西」—— 猜不出来。C# 那一版为此加过一份同样的轨迹。
const TRACE_LIMIT: usize = 64;

/// 一条逻辑流收到的一帧。
///
/// **四种终止都是显式的**：`End`（对端说完了）、`Failure`（对端报错并终止）、
/// `Disconnected`（连接没了）、以及被调用方自己 `close()`（之后 `next()` 返回 `None`）。
/// 没有「默默不再返回」这一档。
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteFrame {
    Item(Value),
    End,
    Failure(RemoteFailure),
    Disconnected(String),
}

impl RemoteFrame {
    /// item 的值。不是 item 时返回 `None` —— 调用方就不必自己写 match 了。
    pub fn value(&self) -> Option<&Value> {
        match self {
            RemoteFrame::Item(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self, RemoteFrame::Item(_))
    }
}

/// mux 的内部状态。读线程与调用方**都要**碰它，所以用 `std::sync::Mutex`
/// 而不是 tokio 的：读线程是同步代码，`lock().await` 在那儿用不了。
struct Shared {
    streams: Mutex<HashMap<String, UnboundedSender<RemoteFrame>>>,
    outbound: UnboundedSender<String>,
    next_id: AtomicU64,
    closing: AtomicBool,
    trace: Mutex<Vec<String>>,
    /// 连接结束的原因（读线程写、`open` 读）。**一次写，之后只读。**
    ended: Mutex<Option<String>>,
}

impl Shared {
    fn note(&self, line: &str) {
        if let Ok(mut trace) = self.trace.lock() {
            trace.push(line.to_string());
            if trace.len() > TRACE_LIMIT {
                trace.remove(0);
            }
        }
    }

    /// 给**每一条**还登记着的流投一帧终止帧，并把它们全部摘掉。
    ///
    /// 摘掉 = 丢掉 sender = 该流的 channel 关闭；读者把已入队的帧读完再收到 `None`，
    /// 所以「登记之后、第一次 `next()` 之前」到达的帧不会被这次收尾吞掉。
    fn terminate_streams(&self, frame: RemoteFrame) {
        let Ok(mut streams) = self.streams.lock() else {
            return;
        };
        if streams.is_empty() {
            return;
        }
        self.note(&format!("[close] {} 条流收到终止帧", streams.len()));
        for (_, sender) in streams.drain() {
            let _ = sender.send(frame.clone());
        }
    }

    fn hit_end(&self, reason: String) {
        let mut ended = match self.ended.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        if ended.is_none() {
            self.note(&format!("[end] {reason}"));
            *ended = Some(reason.clone());
        }
        drop(ended);
        self.terminate_streams(RemoteFrame::Disconnected(reason));
    }
}

/// 一条连到内核的 mux 连接。
pub struct RemoteMux {
    shared: Arc<Shared>,
    /// 关掉连接时用它打断读线程里那个**阻塞**的 `read()`。
    /// （不这么做就得等一个读超时，而且读超时本身不该是停止条件。）
    control: Mutex<Option<std::net::TcpStream>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl std::fmt::Debug for RemoteMux {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteMux")
            .field("streams", &self.shared.streams.lock().map(|s| s.len()).unwrap_or(0))
            .field("ended", &self.end_reason())
            .finish()
    }
}

impl RemoteMux {
    /// 连一个已经在跑的内核。
    pub async fn connect(kernel: &super::WebKernel) -> Result<RemoteMux, RemoteError> {
        RemoteMux::connect_to(kernel.origin(), kernel.port(), kernel.cookie()).await
    }

    /// 直接对一个已知的 `origin` + 端口 + cookie 连。
    ///
    /// 公开的理由不是「方便」：进程内的假服务端（离线测试）走的**就是**这条路径，
    /// 所以被验过的代码与产品用的代码是同一份。
    pub async fn connect_to(origin: &str, port: u16, cookie: &Secret) -> Result<RemoteMux, RemoteError> {
        let (outbound, outbound_rx) = unbounded_channel::<String>();
        let shared = Arc::new(Shared {
            streams: Mutex::new(HashMap::new()),
            outbound,
            next_id: AtomicU64::new(1),
            closing: AtomicBool::new(false),
            trace: Mutex::new(Vec::new()),
            ended: Mutex::new(None),
        });

        let request = handshake_request(port, cookie)?;
        let (ready_tx, ready_rx) = oneshot::channel::<Result<std::net::TcpStream, RemoteError>>();
        let thread_shared = Arc::clone(&shared);
        let host = origin.to_string();

        let thread = std::thread::Builder::new()
            .name("saiku-dsh-mux".into())
            .spawn(move || run_connection(thread_shared, host, port, request, outbound_rx, ready_tx))
            .map_err(|e| RemoteError::Io(format!("起 mux 线程失败：{e}")))?;

        match tokio::time::timeout(HANDSHAKE_TIMEOUT, ready_rx).await {
            Ok(Ok(Ok(control))) => Ok(RemoteMux {
                shared,
                control: Mutex::new(Some(control)),
                thread: Mutex::new(Some(thread)),
            }),
            Ok(Ok(Err(error))) => {
                let _ = thread.join();
                Err(error)
            }
            Ok(Err(_)) => Err(RemoteError::Closed("mux 线程在握手完成前就退出了".into())),
            Err(_) => {
                shared.hit_end("mux 握手超时".into());
                let _ = thread.join();
                Err(RemoteError::Timeout(format!(
                    "在 {} 秒内没有完成 /{} 的 WebSocket 握手",
                    HANDSHAKE_TIMEOUT.as_secs(),
                    wire::MUX_PATH.trim_start_matches('/')
                )))
            }
        }
    }

    /// 开一条逻辑流。
    ///
    /// 返回的 [`RemoteStream`] 是**阻塞式**的枚举：调用方自己决定读到哪一帧停。
    /// 刻意不做「收满 N 帧就关」那种便利接口 —— `session/follow` 是长连流，
    /// 「什么时候算够」只有调用方知道（界面是「用户切走」才停）。
    pub async fn open(&self, method: &str, args: Value) -> Result<RemoteStream, RemoteError> {
        if !wire::valid_method(method) {
            return Err(RemoteError::Protocol(format!("不是一个合法的 Remote 方法名：{method:?}")));
        }
        if let Some(reason) = self.end_reason() {
            return Err(RemoteError::Closed(reason));
        }

        let stream_id = format!(
            "saiku-{}-{}",
            std::process::id(),
            self.shared.next_id.fetch_add(1, Ordering::SeqCst)
        );
        let frame = wire::encode_open(&stream_id, method, &args)?;

        // **先登记信箱，再发 open** —— 见本模块顶部第 1 条。
        let (tx, rx) = unbounded_channel::<RemoteFrame>();
        {
            let mut streams = self
                .shared
                .streams
                .lock()
                .map_err(|_| RemoteError::Closed("mux 状态锁中毒".into()))?;
            streams.insert(stream_id.clone(), tx);
        }

        let text = frame.to_string();
        if self.shared.outbound.send(text).is_err() {
            let reason = self
                .end_reason()
                .unwrap_or_else(|| "mux 连接已经不在".into());
            if let Ok(mut streams) = self.shared.streams.lock() {
                streams.remove(&stream_id);
            }
            return Err(RemoteError::Closed(reason));
        }
        self.shared.note(&format!("[open] {method} streamId={stream_id}"));

        Ok(RemoteStream {
            id: stream_id,
            receiver: rx,
            shared: Arc::clone(&self.shared),
            closed: false,
        })
    }

    /// 连接结束的原因；还在连着就是 `None`。
    pub fn end_reason(&self) -> Option<String> {
        self.shared.ended.lock().ok().and_then(|guard| guard.clone())
    }

    /// 最近若干条诊断。排查「流为什么不动」时的第一手材料。
    pub fn trace(&self) -> Vec<String> {
        self.shared.trace.lock().map(|guard| guard.clone()).unwrap_or_default()
    }

    /// 收掉这条连接：通知所有还开着的流、关掉 socket、等线程退出。
    ///
    /// 与 [`Drop`] 的差别只有一个：这里会等线程真的结束，所以调用方可以确信
    /// 「连接没了」。幂等。
    pub async fn shutdown(&self) {
        let _ = tokio::task::spawn_blocking({
            // 借用跨不了 await，所以把要做的事抄成一份「只带走需要的东西」的闭包。
            let closing = Arc::clone(&self.shared);
            let handle = self.thread.lock().ok().and_then(|mut guard| guard.take());
            let control = self.control.lock().ok().and_then(|mut guard| guard.take());
            move || close_connection(&closing, handle, control)
        })
        .await;
    }
}

impl Drop for RemoteMux {
    /// Drop 只是兜底。它**同步**做同一件事，包括 join 那条线程 ——
    /// `close_connection` 先把 socket 关掉，所以 `read()` 会立刻返回，join 是有界的。
    fn drop(&mut self) {
        let handle = self.thread.lock().ok().and_then(|mut guard| guard.take());
        let control = self.control.lock().ok().and_then(|mut guard| guard.take());
        close_connection(&self.shared, handle, control);
    }
}

fn close_connection(
    shared: &Arc<Shared>,
    handle: Option<std::thread::JoinHandle<()>>,
    control: Option<std::net::TcpStream>,
) {
    shared.closing.store(true, Ordering::SeqCst);
    // 打断阻塞中的 `read()`：关掉底层 socket，读会立刻返回错误/0。
    if let Some(socket) = control {
        let _ = socket.shutdown(std::net::Shutdown::Both);
    }
    shared.hit_end("mux 连接已由调用方关闭".into());
    if let Some(handle) = handle {
        let _ = handle.join();
    }
}

/// 一条逻辑流。
pub struct RemoteStream {
    id: String,
    receiver: UnboundedReceiver<RemoteFrame>,
    shared: Arc<Shared>,
    closed: bool,
}

impl std::fmt::Debug for RemoteStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteStream")
            .field("id", &self.id)
            .field("closed", &self.closed)
            .finish()
    }
}

impl RemoteStream {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 读这条流的下一帧。
    ///
    /// 读的是**它自己的**队列，所以同一条连接上别的流的流量不会插进来（也不会被它抢走）。
    ///
    /// `None` 只在「已经交付过一帧终止帧（或自己 `close()` 过）」之后出现。
    /// **不会**出现「既没有帧也没有终止原因就一直挂着」—— 连接断了会投
    /// [`RemoteFrame::Disconnected`]。
    // 名字与 `Iterator::next` 撞了，但签名不同（async、返回自己的帧型），
    // 而且这里刻意**不**实现 `Iterator`：流的结束带原因，迭代器表达不了。
    #[allow(clippy::should_implement_trait)]
    pub async fn next(&mut self) -> Option<RemoteFrame> {
        if self.closed {
            return None;
        }
        self.receiver.recv().await
    }

    /// 关掉**这一条**流（不影响同一条连接上别的流）。
    ///
    /// 发出的 `cancel` 会让内核中止对应的 Host 迭代器（`stream-server.ts:189-191`）。
    /// 本地的队列随后被封口：之后 `next()` 返回 `None`。
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.detach(true);
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    fn detach(&self, cancel: bool) {
        if let Ok(mut streams) = self.shared.streams.lock() {
            streams.remove(&self.id);
        }
        if cancel {
            let text = wire::encode_cancel(&self.id).to_string();
            let _ = self.shared.outbound.send(text);
            self.shared.note(&format!("[cancel] streamId={}", self.id));
        }
    }
}

impl Drop for RemoteStream {
    /// **Drop 会发 `cancel`，不是静默丢弃。**
    ///
    /// 不发的后果是实打实的：`session/follow` 那条 Host 迭代器会一直挂着，
    /// 直到整条连接断掉 —— 界面里「切走又切回」几十次就能在内核里堆几十条活流。
    fn drop(&mut self) {
        if !self.closed {
            self.closed = true;
            self.detach(true);
        }
    }
}

fn handshake_request(port: u16, cookie: &Secret) -> Result<ClientRequestBuilder, RemoteError> {
    let uri: Uri = format!("ws://127.0.0.1:{port}{}", wire::MUX_PATH)
        .parse()
        .map_err(|e| RemoteError::Protocol(format!("mux 的 Uri 不合法：{e}")))?;
    Ok(ClientRequestBuilder::new(uri).with_header("Cookie", cookie.expose().to_string()))
}

/// 连接线程：握手 → 循环（发 + 读）→ 收尾。
fn run_connection(
    shared: Arc<Shared>,
    host: String,
    port: u16,
    request: ClientRequestBuilder,
    outbound: UnboundedReceiver<String>,
    ready: oneshot::Sender<Result<std::net::TcpStream, RemoteError>>,
) {
    let socket = match std::net::TcpStream::connect(("127.0.0.1", port)) {
        Ok(socket) => socket,
        Err(error) => {
            let _ = ready.send(Err(RemoteError::Io(format!(
                "连不上 {host} 的 {port} 端口：{error}"
            ))));
            return;
        }
    };
    let _ = socket.set_nodelay(true);
    let control = match socket.try_clone() {
        Ok(control) => control,
        Err(error) => {
            let _ = ready.send(Err(RemoteError::Io(format!("复制 socket 句柄失败：{error}"))));
            return;
        }
    };

    // 握手的读写**必须有界**，而且界就是 HANDSHAKE_TIMEOUT 本身：握手读到超时会以
    // `HandshakeError::Interrupted` 返回，那正是我们要的「握手失败」。
    // 早先这里刻意**不**设超时（理由是「握手是一次阻塞读」），结果是：对端接了 TCP 却
    // 不回 HTTP 时，这条线程永远阻塞在握手里，而 `connect_to` 超时之后还要 `join` 它
    // —— 一次无界等待，在离线套件里表现为测试挂 60 秒以上（假服务端的同一缺陷触发）。
    let _ = socket.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = socket.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let mut websocket = match tokio_tungstenite::tungstenite::client::client(request, socket) {
        Ok((websocket, _response)) => websocket,
        Err(error) => {
            let _ = ready.send(Err(handshake_error(&host, error)));
            return;
        }
    };
    // 握完再上读超时：之后每一轮 `read()` 最多阻塞 READ_POLL。
    let _ = websocket.get_ref().set_read_timeout(Some(READ_POLL));
    let _ = websocket.get_ref().set_write_timeout(None);

    if ready.send(Ok(control)).is_err() {
        // 调用方已经不等了（超时/取消）：别把连接留着。
        let _ = websocket.close(None);
        return;
    }

    let reason = pump(&shared, &mut websocket, outbound);
    shared.hit_end(reason);
}

/// 主循环：把排队的出站帧发出去，再读一帧。
///
/// 返回结束原因（给所有还开着的流）。
fn pump(
    shared: &Arc<Shared>,
    websocket: &mut WebSocket<std::net::TcpStream>,
    mut outbound: UnboundedReceiver<String>,
) -> String {
    loop {
        if shared.closing.load(Ordering::SeqCst) {
            let _ = websocket.close(None);
            return "mux 连接已由调用方关闭".into();
        }

        while let Ok(text) = outbound.try_recv() {
            if let Err(error) = websocket.send(Message::text(text)) {
                return format!("发送 mux 帧失败：{}", describe(&error));
            }
        }

        match websocket.read() {
            Ok(Message::Text(text)) => dispatch(shared, text.as_str()),
            Ok(Message::Binary(_)) => {
                // 二进制帧在协议里是不允许的（`stream-server.ts:151-154` 会以 1003 关掉）。
                // 我们只发文本，所以真的收到二进制就说明对端不是内核。
                return "收到二进制帧 —— mux 只走文本".into();
            }
            Ok(Message::Close(frame)) => {
                return match frame {
                    Some(frame) => format!("内核关闭了连接：{} {}", frame.code, frame.reason),
                    None => "内核关闭了连接".into(),
                };
            }
            // Ping / Pong 由 tungstenite 自己处理（Pong 自动排队 + 刷出）。
            Ok(_) => {}
            Err(WsError::Io(error)) if is_would_block(&error) => {}
            Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => {
                return "内核关闭了连接".into();
            }
            Err(error) => return format!("mux 读取失败：{}", describe(&error)),
        }
    }
}

/// 读超时 / 非阻塞的两种表现。Windows 给 `TimedOut`，Unix 给 `WouldBlock`。
fn is_would_block(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn describe(error: &WsError) -> String {
    match error {
        WsError::Io(io) => format!("{io}"),
        other => format!("{other}"),
    }
}

fn handshake_error(
    host: &str,
    error: tokio_tungstenite::tungstenite::HandshakeError<
        tokio_tungstenite::tungstenite::handshake::client::ClientHandshake<std::net::TcpStream>,
    >,
) -> RemoteError {
    use tokio_tungstenite::tungstenite::HandshakeError;
    match error {
        HandshakeError::Failure(WsError::Http(response)) => {
            let status = response.status();
            // 401/403 是「浏览器信任栅栏」拒的，不是端口不对
            // （`api-request-trust.ts:121-151` 与 `stream-server.ts:433-444`）。
            match status.as_u16() {
                401 => RemoteError::Unauthenticated(
                    "mux 握手被拒为 401 —— 浏览器会话 cookie 无效或已过期".into(),
                ),
                403 => RemoteError::Unauthenticated(format!(
                    "mux 握手被拒为 403 —— Host/Origin 栅栏不接受 {host}"
                )),
                _ => RemoteError::Io(format!("mux 握手返回 HTTP {status}")),
            }
        }
        HandshakeError::Failure(other) => RemoteError::Io(format!("mux 握手失败：{}", describe(&other))),
        HandshakeError::Interrupted(_) => RemoteError::Io("mux 握手被中断".into()),
    }
}

/// 一帧文本 → 投给它的流。**连接级的诊断与流级的投递分开**：认不出的帧只记账。
fn dispatch(shared: &Arc<Shared>, text: &str) {
    match wire::decode_frame(text) {
        ServerFrame::Item { stream_id, value } => {
            deliver(shared, &stream_id, RemoteFrame::Item(value));
        }
        ServerFrame::End { stream_id } => {
            deliver(shared, &stream_id, RemoteFrame::End);
            // 终止帧交付之后把信箱摘掉：读者读完已入队的帧会拿到 `None`。
            if let Ok(mut streams) = shared.streams.lock() {
                streams.remove(&stream_id);
            }
        }
        ServerFrame::Failure { stream_id, failure } => {
            deliver(shared, &stream_id, RemoteFrame::Failure(failure));
            if let Ok(mut streams) = shared.streams.lock() {
                streams.remove(&stream_id);
            }
        }
        ServerFrame::Unrecognized { detail } => {
            shared.note(&format!("[drop] {detail}"));
        }
    }
}

fn deliver(shared: &Arc<Shared>, stream_id: &str, frame: RemoteFrame) {
    let Ok(streams) = shared.streams.lock() else {
        return;
    };
    match streams.get(stream_id) {
        Some(sender) => {
            // 队列无界，所以这里不会阻塞；收不到只可能是读者已经走了。
            let _ = sender.send(frame);
        }
        None => {
            // 没有登记的流：记一笔。「帧静静消失」只有线上轨迹能看出来。
            shared.note(&format!("[drop] 没有登记的流 {stream_id}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::fake::FakeMux;
    use serde_json::json;

    fn session_follow_args(session: &str) -> Value {
        json!({ "request": { "address": { "kind": "session", "sessionId": session } } })
    }

    async fn connect(server: &FakeMux) -> RemoteMux {
        RemoteMux::connect_to(&server.origin(), server.port(), &Secret::new("dsh-auth-x=v1.a.b"))
            .await
            .expect("连上假服务端")
    }

    #[tokio::test]
    async fn two_streams_on_one_socket_do_not_cross_frames() {
        let mut server = FakeMux::start();
        let mux = connect(&server).await;

        let mut a = mux.open("session/follow", session_follow_args("A")).await.unwrap();
        let mut b = mux.open("session/follow", session_follow_args("B")).await.unwrap();

        let open_a = server.recv().await.unwrap();
        let open_b = server.recv().await.unwrap();
        let id_a = open_a["streamId"].as_str().unwrap().to_string();
        let id_b = open_b["streamId"].as_str().unwrap().to_string();
        assert_ne!(id_a, id_b, "两条流必须各自一个 streamId");
        // open 帧的形状要逐字对：payload 里只有 args。
        assert_eq!(open_a["type"], "open");
        assert_eq!(open_a["endpoint"], "session/follow");
        assert_eq!(open_a["payload"]["args"]["request"]["address"]["sessionId"], "A");
        assert!(open_a["payload"].as_object().unwrap().len() == 1);
        assert_eq!(open_b["payload"]["args"]["request"]["address"]["sessionId"], "B");

        // **交错**发送：顺序读两条流的话，「共用一个队列」也有很大机会蒙对。
        server.send(json!({ "type": "item", "streamId": id_a, "value": { "marker": "A1" } }));
        server.send(json!({ "type": "item", "streamId": id_b, "value": { "marker": "B1" } }));
        server.send(json!({ "type": "item", "streamId": id_a, "value": { "marker": "A2" } }));

        // 先读 B：串帧的话这里就会读到 A 的东西。
        let first_b = b.next().await.unwrap();
        assert_eq!(first_b.value().unwrap()["marker"], "B1");
        assert_eq!(a.next().await.unwrap().value().unwrap()["marker"], "A1");
        assert_eq!(a.next().await.unwrap().value().unwrap()["marker"], "A2");
    }

    #[tokio::test]
    async fn frames_arriving_before_the_first_read_are_not_lost() {
        let mut server = FakeMux::start();
        let mux = connect(&server).await;
        let mut stream = mux.open("session/follow", session_follow_args("A")).await.unwrap();
        let id = server.recv().await.unwrap()["streamId"].as_str().unwrap().to_string();

        // 快照那一帧常常在 `open` 返回后立刻就到（对它来说那就是第一个 item）。
        server.send(json!({ "type": "item", "streamId": id, "value": { "type": "snapshot" } }));
        tokio::time::sleep(Duration::from_millis(80)).await;

        let frame = stream.next().await.expect("快照不该丢");
        assert_eq!(frame.value().unwrap()["type"], "snapshot");
    }

    #[tokio::test]
    async fn end_terminates_only_that_stream() {
        let mut server = FakeMux::start();
        let mux = connect(&server).await;
        let mut a = mux.open("session/follow", session_follow_args("A")).await.unwrap();
        let mut b = mux.open("session/follow", session_follow_args("B")).await.unwrap();
        let id_a = server.recv().await.unwrap()["streamId"].as_str().unwrap().to_string();
        let id_b = server.recv().await.unwrap()["streamId"].as_str().unwrap().to_string();

        server.send(json!({ "type": "end", "streamId": id_a }));
        assert_eq!(a.next().await, Some(RemoteFrame::End));
        assert_eq!(a.next().await, None, "终止帧之后必须收口，不能挂着");

        // B 不受影响。
        server.send(json!({ "type": "item", "streamId": id_b, "value": 1 }));
        assert_eq!(b.next().await.unwrap().value().unwrap(), &json!(1));
        assert!(!b.is_closed(), "关掉的是 A，B 不该跟着结束");
    }

    #[tokio::test]
    async fn error_frame_carries_the_structured_failure_and_ends_the_stream() {
        let mut server = FakeMux::start();
        let mux = connect(&server).await;
        let mut stream = mux.open("session/follow", session_follow_args("A")).await.unwrap();
        let id = server.recv().await.unwrap()["streamId"].as_str().unwrap().to_string();

        server.send(json!({
            "type": "error",
            "streamId": id,
            "error": {
                "code": "session/corrupt-log",
                "message": "corrupt Zstandard session log: torn JSONL record",
                "details": { "endpoint": "session/follow" },
            },
        }));

        match stream.next().await {
            Some(RemoteFrame::Failure(failure)) => {
                assert_eq!(failure.code, "session/corrupt-log");
                assert!(failure.message.contains("torn JSONL"), "{}", failure.message);
                assert_eq!(failure.details["endpoint"], "session/follow");
            }
            other => panic!("期望结构化失败，得到 {other:?}"),
        }
        // 0.2.1 里 error 就是终止帧，后面不再跟一个 end。
        assert_eq!(stream.next().await, None);
    }

    #[tokio::test]
    async fn a_dropped_connection_wakes_every_stream_with_a_reason() {
        let mut server = FakeMux::start();
        let mux = connect(&server).await;
        let mut a = mux.open("session/follow", session_follow_args("A")).await.unwrap();
        // 两条**不同**的 endpoint，用来证明「断线通知按流走」而不是只通知了最后一条。
        // 早先这里写的是 `"control"` —— 单段名字；Gateway 要求 endpoint 恰好两段
        // （`api/gateway/src/index.ts:1133-1136`），所以那是个非法名，被本地校验挡掉了。
        let mut b = mux.open("workspace/follow", json!({})).await.unwrap();
        server.recv().await.unwrap();
        server.recv().await.unwrap();

        server.drop_connection();

        for (name, stream) in [("A", &mut a), ("B", &mut b)] {
            match stream.next().await {
                Some(RemoteFrame::Disconnected(reason)) => {
                    assert!(!reason.is_empty(), "{name} 的结束原因不该是空的");
                }
                other => panic!("{name} 期望「连接断了」，得到 {other:?}"),
            }
            assert_eq!(stream.next().await, None);
        }
    }

    #[tokio::test]
    async fn opening_after_the_connection_died_fails_instead_of_hanging() {
        let server = FakeMux::start();
        let mux = connect(&server).await;
        server.drop_connection();
        // 等读线程收到断线。
        for _ in 0..200 {
            if mux.end_reason().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let error = mux.open("session/follow", session_follow_args("A")).await.unwrap_err();
        assert!(matches!(error, RemoteError::Closed(_)), "{error}");
    }

    #[tokio::test]
    async fn closing_one_stream_sends_cancel_and_leaves_the_other_alone() {
        let mut server = FakeMux::start();
        let mux = connect(&server).await;
        let mut a = mux.open("session/follow", session_follow_args("A")).await.unwrap();
        let mut b = mux.open("session/follow", session_follow_args("B")).await.unwrap();
        let id_a = server.recv().await.unwrap()["streamId"].as_str().unwrap().to_string();
        let id_b = server.recv().await.unwrap()["streamId"].as_str().unwrap().to_string();

        a.close();
        let cancel = server.recv().await.unwrap();
        assert_eq!(cancel["type"], "cancel");
        assert_eq!(cancel["streamId"], id_a);
        assert_eq!(a.next().await, None);

        // B 还在：给它一帧，它还得收得到。
        server.send(json!({ "type": "item", "streamId": id_b, "value": 2 }));
        assert_eq!(b.next().await.unwrap().value().unwrap(), &json!(2));
    }

    #[tokio::test]
    async fn dropping_a_stream_cancels_it_upstream() {
        // 静默丢弃会让 Host 那边的迭代器一直挂着 —— 界面里切来切去会堆出活流。
        let mut server = FakeMux::start();
        let mux = connect(&server).await;
        let stream = mux.open("session/follow", session_follow_args("A")).await.unwrap();
        let id = server.recv().await.unwrap()["streamId"].as_str().unwrap().to_string();
        drop(stream);

        let cancel = server.recv().await.unwrap();
        assert_eq!(cancel["type"], "cancel");
        assert_eq!(cancel["streamId"], id);
    }

    #[tokio::test]
    async fn a_multibyte_character_split_across_websocket_fragments_survives() {
        // 回归测试（AGENTS.md P12）：WebSocket 的分片边界会**撕开多字节字符**，
        // 逐片各自解 UTF-8 会把一个汉字变成两个 U+FFFD，字符串还多出一个字符。
        //
        // 实测抓到它的方式不是读代码而是对账：某个 147 块的消息里只有一个思考块
        // 长度 7799 而重放是 7798，差的那一个就是「隐」被撕成两个替换字符。
        // 这里把那个形状压成一小段：服务端把一帧 JSON **从汉字中间**切成两个分片。
        let mut server = FakeMux::start();
        let mux = connect(&server).await;
        let mut stream = mux.open("session/follow", session_follow_args("A")).await.unwrap();
        let id = server.recv().await.unwrap()["streamId"].as_str().unwrap().to_string();

        let payload = json!({
            "type": "item",
            "streamId": id,
            "value": { "text": "Delay → 隐藏" },
        })
        .to_string();
        let cut = split_inside_multibyte(payload.as_bytes()).expect("载荷里该有三字节的汉字");
        server.send_fragmented(&payload, cut);

        let frame = stream.next().await.expect("分片消息该被组装成一帧");
        let text = frame.value().unwrap()["text"].as_str().unwrap();
        assert_eq!(text, "Delay → 隐藏", "跨分片的汉字被撕坏了");
        assert!(!text.contains('\u{fffd}'), "出现替换字符：{text:?}");
    }

    /// 找一个**落在多字节字符内部**的字节切点。
    fn split_inside_multibyte(bytes: &[u8]) -> Option<usize> {
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            let width = if byte < 0x80 {
                1
            } else if byte >> 5 == 0b110 {
                2
            } else if byte >> 4 == 0b1110 {
                3
            } else if byte >> 3 == 0b11110 {
                4
            } else {
                1
            };
            if width == 3 && index + 1 < bytes.len() {
                return Some(index + 1);
            }
            index += width;
        }
        None
    }

    #[tokio::test]
    async fn the_mux_keeps_a_trace_of_what_crossed_the_wire() {
        // 「流开了但一个 item 都没有」这类问题，唯一能问的就是线上到底有没有东西。
        let mut server = FakeMux::start();
        let mux = connect(&server).await;
        let stream = mux.open("workspace/follow", json!({})).await.unwrap();
        server.recv().await.unwrap();
        // 一条没有登记的流发来的帧：不该被静默吞掉。
        server.send(json!({ "type": "item", "streamId": "who-is-this", "value": 1 }));
        tokio::time::sleep(Duration::from_millis(80)).await;

        let trace = mux.trace().join("\n");
        assert!(trace.contains("[open] workspace/follow"), "{trace}");
        assert!(trace.contains("没有登记的流"), "{trace}");
        drop(stream);
    }

    #[tokio::test]
    async fn invalid_methods_are_refused_before_opening_a_stream() {
        let server = FakeMux::start();
        let mux = connect(&server).await;
        let error = mux.open("session", json!({})).await.unwrap_err();
        assert!(matches!(error, RemoteError::Protocol(_)), "{error}");
    }
}
