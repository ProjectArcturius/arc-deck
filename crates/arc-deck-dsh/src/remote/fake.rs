//! 进程内的**假 mux 服务端** —— 离线测试用，不依赖真内核。
//!
//! 它是一条**真的** WebSocket（跟客户端同一个 tungstenite），只是把「内核」换成
//! 一段由测试驱动的脚本。理由：串帧、断线、分片撕开汉字这些事**只有在真 socket 上**
//! 才逼得出来 —— 一个假的帧对象骗不过自己的分发代码，而那正是被测的那一段。
//!
//! 与客户端一样用同步 API + 专用线程（理由见 `mux.rs` 顶部）。服务端只有一条连接：
//! 每一条测试都是「一条连接上开几条流」，正好对上。

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data as OpData, OpCode};
use tokio_tungstenite::tungstenite::protocol::frame::Frame;
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocket};
use tokio_tungstenite::tungstenite::{accept, Error as WsError};

/// 服务端线程读到「有要发的东西/要断线」的超时。
const POLL: Duration = Duration::from_millis(20);

/// 等客户端连上来的上限。
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);

/// `Drop` 等服务端线程退出的上限。
///
/// **这条上限是必须的**，理由是一次实测：并行跑整个离线套件时，偶尔会有一个 mux
/// 测试**挂到测试框架报「跑了 60 秒以上」**，而单跑或串行跑从来不复现。挂在那里
/// 的那句话什么也说明不了 —— 既看不出是哪条流，也看不出卡在服务器还是客户端。
/// `thread.join()` 是这套测试夹具里唯一的**无界**等待，所以给它加上界：真的超时了
/// 就 panic，名字与原因写在消息里，而不是把整个套件拖到被外部杀掉。
const JOIN_TIMEOUT: Duration = Duration::from_secs(15);

enum Command {
    Text(String),
    /// 把一条文本消息**从指定字节位置切成两个 WebSocket 分片**发出去。
    Fragments { text: String, split_at: usize },
    Drop,
}

pub struct FakeMux {
    addr: SocketAddr,
    /// 客户端发来的帧（已解析成 JSON）。
    frames: UnboundedReceiver<Value>,
    commands: UnboundedSender<Command>,
    control: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// 服务端线程退出时置位。`Drop` 靠它把「join 挂了」变成一次有界的失败。
    done: Arc<AtomicBool>,
}

impl FakeMux {
    pub fn start() -> FakeMux {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑一个空闲端口");
        listener.set_nonblocking(true).expect("非阻塞监听");
        let addr = listener.local_addr().unwrap();

        let (frames_tx, frames) = unbounded_channel::<Value>();
        let (commands, commands_rx) = unbounded_channel::<Command>();
        let control = Arc::new(Mutex::new(None));
        let done = Arc::new(AtomicBool::new(false));

        let thread_control = Arc::clone(&control);
        let thread_done = Arc::clone(&done);
        let thread = std::thread::Builder::new()
            .name("saiku-dsh-fake-mux".into())
            .spawn(move || {
                serve(listener, commands_rx, frames_tx, thread_control);
                thread_done.store(true, Ordering::SeqCst);
            })
            .expect("起假服务端线程");

        FakeMux { addr, frames, commands, control, thread: Some(thread), done }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }

    pub fn send(&self, value: Value) {
        let _ = self.commands.send(Command::Text(value.to_string()));
    }

    /// 分两片发一条文本消息，切点在 `split_at` 字节处。
    pub fn send_fragmented(&self, text: &str, split_at: usize) {
        let _ = self.commands.send(Command::Fragments {
            text: text.to_string(),
            split_at,
        });
    }

    /// 粗暴断开（不做关闭握手），模拟内核崩了 / 线路断了。
    pub fn drop_connection(&self) {
        let _ = self.commands.send(Command::Drop);
    }

    /// 读客户端发来的一帧。超时返回 `None`。
    pub async fn recv(&mut self) -> Option<Value> {
        tokio::time::timeout(Duration::from_secs(5), self.frames.recv())
            .await
            .ok()
            .flatten()
    }
}

impl Drop for FakeMux {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Drop);
        // 直接关掉底层 socket：服务端线程可能正阻塞在 read 上。
        if let Ok(mut guard) = self.control.lock()
            && let Some(socket) = guard.take() {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
        if let Some(thread) = self.thread.take() {
            // **有界 join**：见 `JOIN_TIMEOUT` 的注释。先把 join 交给另一个线程，
            // 这样超时之后我们还能给出结论（join 本身没有超时版本）。
            let done = Arc::clone(&self.done);
            let joiner = std::thread::spawn(move || thread.join());
            let deadline = Instant::now() + JOIN_TIMEOUT;
            while Instant::now() < deadline && !done.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                done.load(Ordering::SeqCst),
                "假 mux 服务端线程在 {JOIN_TIMEOUT:?} 内没有退出（端口 {}）—— \
                 这是**测试夹具**的挂死，不是断言的失败：调用方放弃连接之后，\
                 服务端线程应当立刻从 read 返回。",
                self.addr.port()
            );
            let _ = joiner.join();
        }
    }
}

fn serve(
    listener: TcpListener,
    mut commands: UnboundedReceiver<Command>,
    frames: UnboundedSender<Value>,
    control: Arc<Mutex<Option<TcpStream>>>,
) {
    let Some(stream) = accept_within(&listener) else {
        return;
    };
    // **Windows 上 accept 出来的 socket 继承监听端的非阻塞模式**（Linux 不继承）。
    // 不改回阻塞的话，下面的握手读到 WouldBlock 就以 Interrupted 退出，而 `control`
    // 里那份克隆句柄让 TCP 连接继续开着 —— 客户端于是一直等一个永远不来的握手回应。
    // 这正是「并行跑全套时偶尔挂 60 秒以上、单跑不复现」的那条：只有客户端的请求
    // 晚于服务端第一次读到达时才触发，负载越高越容易。
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    if let Ok(clone) = stream.try_clone()
        && let Ok(mut guard) = control.lock() {
            *guard = Some(clone);
        }

    // 握手必须**不带**读超时：那是一次阻塞的握手读。
    let Ok(mut websocket) = accept(stream) else {
        return;
    };
    // 握完再上：之后每一轮 read 最多阻塞 POLL，好让出站命令有机会发出去。
    let _ = websocket.get_ref().set_read_timeout(Some(POLL));

    loop {
        while let Ok(command) = commands.try_recv() {
            let failed = match command {
                Command::Text(text) => websocket.send(Message::text(text)).is_err(),
                Command::Fragments { text, split_at } => send_fragmented(&mut websocket, &text, split_at),
                Command::Drop => {
                    // 不做关闭握手：要的就是「对面没了」这种最粗暴的断法。
                    if let Ok(socket) = websocket.get_ref().try_clone() {
                        let _ = socket.shutdown(std::net::Shutdown::Both);
                    }
                    return;
                }
            };
            if failed {
                return;
            }
        }

        match websocket.read() {
            Ok(Message::Text(text)) => {
                let value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                if frames.send(value).is_err() {
                    return;
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(WsError::Io(error)) if is_would_block(&error) => {}
            Err(_) => return,
        }
    }
}

/// 把一条文本消息切成两片：第一片 `Text` 且 `FIN=0`，第二片 `Continue` 且 `FIN=1`。
///
/// 这是 P12 的成因所在 —— 分片边界可以落在**任何字节**上，包括一个三字节汉字的中间。
/// 客户端那一侧必须攒完整条消息才解码。
fn send_fragmented(
    websocket: &mut WebSocket<TcpStream>,
    text: &str,
    split_at: usize,
) -> bool {
    let bytes = text.as_bytes();
    let split_at = split_at.min(bytes.len());
    let (head, tail) = bytes.split_at(split_at);
    let head = Frame::message(head.to_vec(), OpCode::Data(OpData::Text), false);
    let tail = Frame::message(tail.to_vec(), OpCode::Data(OpData::Continue), true);
    for frame in [head, tail] {
        if websocket.write(Message::Frame(frame)).is_err() {
            return true;
        }
    }
    websocket.flush().is_err()
}

fn accept_within(listener: &TcpListener) -> Option<TcpStream> {
    let deadline = Instant::now() + ACCEPT_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

/// 让「整个测试进程都退出了」与「这里真的没东西」区分开来，而不是靠一个计时器。
fn is_would_block(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// 进程内的**假一元接口**：按脚本回 `server-response`，并记下收到的原始请求。
///
/// 为什么不用 `rpc.rs` 里那个只接一次的假内核：审批的测试要「开流 → 收请求 → 回答」，
/// 回答是一元调用，和开的流在同一套测试里发生。接一次的那个每次都要重开一个服务端。
pub struct FakeRpc {
    port: u16,
    requests: UnboundedReceiver<String>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeRpc {
    pub fn start() -> FakeRpc {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑一个空闲端口");
        listener.set_nonblocking(true).expect("非阻塞监听");
        let port = listener.local_addr().unwrap().port();
        let (tx, requests) = unbounded_channel::<String>();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("saiku-dsh-fake-rpc".into())
            .spawn(move || {
                use std::io::{Read, Write};
                while !thread_stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            // Windows 上 accept 出来的连接**继承**监听端的非阻塞：不切回阻塞的话，
                            // 请求还没到时 `read` 立刻返回空，假服务端就拿 `rpcId: "X"` 回话、
                            // 客户端判它答非所问。只在机器忙时出现（2026-10-20 全量套件里偶发一次）。
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                            // 一次 `read` 也不保证读全：头和体可能分两段到。读到头结束、
                            // 再按 Content-Length 读满体。
                            let mut bytes: Vec<u8> = Vec::new();
                            let mut chunk = vec![0u8; 64 * 1024];
                            loop {
                                let text = String::from_utf8_lossy(&bytes);
                                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                                    let length = head
                                        .lines()
                                        .find_map(|line| {
                                            let (name, value) = line.split_once(':')?;
                                            name.eq_ignore_ascii_case("content-length")
                                                .then(|| value.trim().parse::<usize>().ok())
                                                .flatten()
                                        })
                                        .unwrap_or(0);
                                    if body.len() >= length {
                                        break;
                                    }
                                }
                                match stream.read(&mut chunk) {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                                }
                            }
                            let raw = String::from_utf8_lossy(&bytes).to_string();
                            let rpc_id = raw
                                .split_once("\r\n\r\n")
                                .and_then(|(_, body)| serde_json::from_str::<Value>(body).ok())
                                .and_then(|value| value["rpcId"].as_str().map(str::to_string))
                                .unwrap_or_else(|| "X".into());
                            let _ = tx.send(raw);
                            let body = format!(
                                r#"{{"type":"server-response","rpcId":"{rpc_id}","result":{{"ok":true,"value":{{}}}}}}"#
                            );
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = stream.write_all(response.as_bytes());
                            let _ = stream.flush();
                        }
                        Err(error) if is_would_block(&error) => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            })
            .expect("起假一元接口线程");
        FakeRpc { port, requests, stop, thread: Some(thread) }
    }

    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 下一条请求的原文（HTTP 头 + 体）。
    pub async fn request(&mut self) -> Option<String> {
        tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
            .await
            .ok()
            .flatten()
    }
}

impl Drop for FakeRpc {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
