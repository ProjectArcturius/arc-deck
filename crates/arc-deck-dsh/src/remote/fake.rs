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
            .name("arcdeck-dsh-fake-mux".into())
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
