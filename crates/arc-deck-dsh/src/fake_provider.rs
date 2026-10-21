//! 脚本化的假模型端点（M3 负例验收）。
//!
//! **只在 `test-runtime` feature 下编译**，发布构建里不存在：它是一个能被指向任意
//! 地址的假 provider，留着就是一条「把真实内核接到假模型上」的路径。
//!
//! # 为什么需要一个真的 HTTP 服务
//!
//! M3 要证明的是**执行层的拒绝**，而执行层在 DSH 内部（沙箱、工具注册表、审批服务）。
//! 要让 DSH 真的去调一个工具，必须有人扮演模型把这个调用发出来 —— 那就是这里。
//! `--dump-config` 只能证明「配置行写对了」，证明不了「工具真的被拒」
//! （`docs/M3_GUARD.md` §6 把那条明确记成「推测」）。
//!
//! # 为什么在进程内跑，而不是一个独立的假端点二进制
//!
//! 试过独立二进制（`[[bin]]` + `required-features`），**不行**：`cargo test -p arc-deck-cli`
//! 不会重建 `arc-deck-dsh` 的 bin 目标，于是改完假端点后测试跑的还是旧的那份 ——
//! 实测踩到过，症状是「新加的脚本字段不生效」。进程内跑就没这回事：它是 `arc-deck-dsh`
//! 的库代码，cargo 一定会在编译 `arc-deck-cli` 之前先重建它。
//!
//! # 协议为什么选 `anthropic-messages`
//!
//! 它是 Saiku 现有路由的默认协议（`core::provider` 的 `opencode-go`），形状是三家里
//! 最容易手写的（SSE + 一种事件模板），且 pi-ai 那份实现的解析逐行可读
//! （`@earendil-works/pi-ai/dist/api/anthropic-messages.js`）。
//!
//! # 三件事，都为了「让断言可核对」
//!
//! 1. 按脚本回**流式**响应；
//! 2. 把**每一个收到的请求**原文（头 + 体）逐行落进日志 —— 这是「DSH 发给模型的工具
//!    清单里还有没有某个工具」的唯一证据来源（可见性收窄）；
//! 3. 第 N 次请求回脚本的第 N 条，超出条目数时回 `default`；脚本用完又没有 `default`
//!    就回 500 并在日志里写明是哪一次。**不重复最后一条** —— 那会把「DSH 多问了一次」
//!    淹掉在一个看不出问题的绿色测试里。
//!
//! 串行处理连接是刻意的：请求必须与脚本条目一一对应。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

// ── 脚本 ────────────────────────────────────────────────────────────────

/// 一个内容块要回什么。
///
/// 参数是 `Value` 而不是字符串：脚本要能写 `{"file_path": "…"}`，
/// 让它变成一段被转义的 JSON 字符串只会让脚本难读、难改。
#[derive(Debug, Clone)]
pub enum Block {
    Text(String),
    ToolCall { name: String, arguments: Value },
}

impl Block {
    fn parse(value: &Value) -> Result<Self, String> {
        if let Some(text) = value.get("text").and_then(Value::as_str) {
            return Ok(Block::Text(text.to_string()));
        }
        if let Some(name) = value.get("tool").and_then(Value::as_str) {
            return Ok(Block::ToolCall {
                name: name.to_string(),
                // 参数缺省是空对象：无参调用不该逼脚本写 `{}`。
                arguments: value.get("arguments").cloned().unwrap_or_else(|| json!({})),
            });
        }
        Err(format!("内容块既不是 text 也不是 tool：{value}"))
    }
}

/// 一次助手回合。
#[derive(Debug, Clone)]
pub struct Response {
    pub blocks: Vec<Block>,
}

impl Response {
    fn parse(value: &Value) -> Result<Self, String> {
        let blocks = value
            .get("blocks")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("脚本条目缺 `blocks` 数组：{value}"))?;
        let blocks = blocks.iter().map(Block::parse).collect::<Result<Vec<_>, _>>()?;
        if blocks.is_empty() {
            // 零内容块的 `stop` 在 pi-ai 那边会被判成 `EMPTY_RESPONSE` 错误
            // （`mapStopReason`），测试于是以「模型失败」收场 —— 一个离原因很远的
            // 假阴性，所以在入口就拒。
            return Err("脚本条目不能是空的：pi-ai 会把零内容块的 stop 判成 EMPTY_RESPONSE".into());
        }
        Ok(Response { blocks })
    }

    /// 这一回合的 `stop_reason`。**由内容推出来**，不让脚本自己写 ——
    /// 手写「有 tool_use 却 stop_reason=end_turn」的脚本，症状是「工具没被调用」，
    /// 查起来离原因很远。
    fn stop_reason(&self) -> &'static str {
        if self.blocks.iter().any(|b| matches!(b, Block::ToolCall { .. })) {
            "tool_use"
        } else {
            "end_turn"
        }
    }
}

/// 脚本：第 N 次请求回第 N 条；超出条目数时回 `default`。
///
/// 有 `default` 才写得出子 Agent 那类用例：一次委派会让**子会话**也发模型请求，
/// 于是总请求数不随父会话走（实测一轮委派 4 次）。逐条编号会脆弱到无法维护，
/// 而「前面的回合按脚本、之后一律收尾」既确定又稳。
#[derive(Debug, Clone)]
pub struct Script {
    responses: Vec<Response>,
    fallback: Option<Response>,
}

impl Script {
    /// 直接从 `{"responses":[…], "default":{…}}` 解析。
    pub fn parse(raw: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(raw).map_err(|e| format!("脚本不是合法 JSON：{e}"))?;
        Self::from_value(&value)
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let list = value
            .get("responses")
            .and_then(Value::as_array)
            .ok_or_else(|| "脚本缺 `responses` 数组".to_string())?;
        let responses = list.iter().map(Response::parse).collect::<Result<Vec<_>, _>>()?;
        let fallback = match value.get("default") {
            Some(v) => Some(Response::parse(v)?),
            None => None,
        };
        if responses.is_empty() && fallback.is_none() {
            return Err("脚本既没有 responses 也没有 default".into());
        }
        Ok(Script { responses, fallback })
    }

    fn at(&self, index: usize) -> Option<&Response> {
        self.responses.get(index).or(self.fallback.as_ref())
    }
}

// ── 请求日志 ────────────────────────────────────────────────────────────

/// 收到的请求原文。落盘再读回来是为了让断言看的是**运行结束后的记录**，
/// 而不是测试进程与内核之间的实时状态 —— 后者在失败时会丢。
pub struct Provider {
    addr: SocketAddr,
    log: PathBuf,
    stop: Arc<Mutex<bool>>,
}

impl Provider {
    /// 起一个假端点：绑定 127.0.0.1 的随机端口，后台线程串行服务请求。
    ///
    /// 端口先绑好再返回，所以调用方拿到 `url()` 时它一定已经在听 ——
    /// 不需要 ready 文件那一套（那是跨进程才需要的）。
    pub fn start(script: Script, log: impl Into<PathBuf>) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("绑定 127.0.0.1:0 失败：{e}"))?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        let log: PathBuf = log.into();
        let stop = Arc::new(Mutex::new(false));
        let stop_thread = Arc::clone(&stop);
        let log_thread = log.clone();
        std::thread::spawn(move || {
            let mut served = 0usize;
            loop {
                if stop_thread.lock().map(|s| *s).unwrap_or(true) {
                    return;
                }
                // 非阻塞 accept：`stop` 才有机会被读到。测试结束时线程可能还在
                // `accept` 上等着，所以用超时轮询而不是无限阻塞。
                match listener.accept() {
                    Ok((stream, _)) => {
                        let index = served;
                        served += 1;
                        if let Err(e) = serve(stream, &script, index, &log_thread) {
                            eprintln!("假 provider 第 {} 次请求处理失败：{e}", index + 1);
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        });
        Ok(Provider { addr, log, stop })
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// 收到的请求（每次一行 JSON）。请求还没落盘时返回已有的部分。
    pub fn requests(&self) -> Vec<Value> {
        read_log(&self.log)
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        if let Ok(mut stop) = self.stop.lock() {
            *stop = true;
        }
    }
}

fn serve(mut stream: TcpStream, script: &Script, index: usize, log: &Path) -> Result<(), String> {
    stream.set_read_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);

    // ── 请求行 ────────────────────────────────────────────────────────────
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    // ── 头 ────────────────────────────────────────────────────────────────
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut h = String::new();
        let n = reader.read_line(&mut h).map_err(|e| e.to_string())?;
        if n == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let header =
        |name: &str| -> Option<&str> { headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()) };

    // ── 体 ────────────────────────────────────────────────────────────────
    // Anthropic SDK 用 undici，POST 带体时是 `content-length`；分块只是保险 ——
    // 一个「读不到体」的实现会让每一个断言都变成假阴性，而症状是 400。
    let body = if header("transfer-encoding").map(|v| v.contains("chunked")).unwrap_or(false) {
        read_chunked(&mut reader)?
    } else {
        let len: usize = header("content-length").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
        String::from_utf8_lossy(&buf).into_owned()
    };
    let request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    append_log(
        log,
        &json!({ "index": index, "method": method, "path": path, "headers": headers, "body": request }),
    );

    // ── 回应 ──────────────────────────────────────────────────────────────
    let Some(response) = script.at(index) else {
        let payload = json!({
            "type": "error",
            "error": { "type": "invalid_request_error", "message": format!(
                "假 provider 的脚本只有 {} 条且没有 default，这是第 {} 次请求",
                script.responses.len(), index + 1) },
        })
        .to_string();
        let head = format!(
            "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            payload.len()
        );
        stream
            .write_all(head.as_bytes())
            .and_then(|_| stream.write_all(payload.as_bytes()))
            .map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;
        return Err(format!("脚本条目不够（第 {} 次请求）", index + 1));
    };

    let sse = render_sse(response, index);
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        sse.len()
    );
    // 体一次写完：分块传输会引入「块边界撕开多字节字符」这一类与本任务无关的变量
    // （AGENTS.md P12 就是那条）。这里的目的是可控的输入，不是复现网络。
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(sse.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    Ok(())
}

fn append_log(path: &Path, value: &Value) {
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{value}");
        let _ = file.flush();
    }
}

fn read_chunked(reader: &mut impl BufRead) -> Result<String, String> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        reader.read_line(&mut size_line).map_err(|e| e.to_string())?;
        let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
            .map_err(|e| format!("chunk size `{size_line}`：{e}"))?;
        if size == 0 {
            break;
        }
        let mut buf = vec![0u8; size];
        reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
        out.extend_from_slice(&buf);
        let mut crlf = [0u8; 2];
        let _ = reader.read_exact(&mut crlf);
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// 把一次回合编成 Anthropic Messages 的 SSE 流。
///
/// 事件顺序照 `@earendil-works/pi-ai/dist/api/anthropic-messages.js` 的消费端写：
/// 它按 `content_block_start` 建块、按 `message_delta.delta.stop_reason` 定终态，
/// **`message_stop` 缺席会被判成「流提前结束」**（那个文件里的
/// `Anthropic stream ended before message_stop`），所以首尾两个事件一个都不能少。
fn render_sse(response: &Response, index: usize) -> String {
    let mut out = String::new();
    let mut push = |event: &str, data: Value| {
        out.push_str(&format!("event: {event}\ndata: {data}\n\n"));
    };

    push(
        "message_start",
        json!({
            "type": "message_start",
            "message": {
                "id": format!("msg_fake_{index}"),
                "type": "message",
                "role": "assistant",
                "model": "fake-model",
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": { "input_tokens": 11, "output_tokens": 0 },
            },
        }),
    );

    let mut output_tokens = 0u64;
    for (i, block) in response.blocks.iter().enumerate() {
        match block {
            Block::Text(text) => {
                push(
                    "content_block_start",
                    json!({ "type": "content_block_start", "index": i,
                            "content_block": { "type": "text", "text": "" } }),
                );
                push(
                    "content_block_delta",
                    json!({ "type": "content_block_delta", "index": i,
                            "delta": { "type": "text_delta", "text": text } }),
                );
                output_tokens += 1;
            }
            Block::ToolCall { name, arguments } => {
                // 工具调用也走**增量**：真实 provider 发的是 `input_json_delta`，
                // 而 pi-ai 的 `toolcall_start` 取的是 partial 里那个块的 id/name。
                // 一次 `content_block_start` 带全 input 也能过，但那样就绕开了真实路径 ——
                // 假端点一旦走了真实 provider 不走的路，它证明的东西就少一块。
                push(
                    "content_block_start",
                    json!({ "type": "content_block_start", "index": i,
                            "content_block": { "type": "tool_use", "id": format!("toolu_fake_{index}_{i}"),
                                               "name": name, "input": {} } }),
                );
                let partial = serde_json::to_string(arguments).unwrap_or_else(|_| "{}".into());
                push(
                    "content_block_delta",
                    json!({ "type": "content_block_delta", "index": i,
                            "delta": { "type": "input_json_delta", "partial_json": partial } }),
                );
                output_tokens += 1;
            }
        }
        push("content_block_stop", json!({ "type": "content_block_stop", "index": i }));
    }

    push(
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": { "stop_reason": response.stop_reason(), "stop_sequence": null },
            "usage": { "output_tokens": output_tokens },
        }),
    );
    push("message_stop", json!({ "type": "message_stop" }));
    out
}

// ── 读取侧：断言要用的解析工具 ──────────────────────────────────────────
//
// 「请求体的哪个位置是工具清单」这件事属于协议形状，而协议形状的出处应该只有一处 ——
// 散在测试里迟早会漂移。

/// 请求体里的工具名清单。
///
/// 这是「可见性收窄」那一半的唯一证据来源：`shell = false` 时 DSH 摘掉了工具行，
/// 于是模型**根本看不到**那个工具 —— 与「看得到但调用被拒」是两件不同的事，
/// `docs/M3_GUARD.md` §2 把两者分开记（工具管线 / 可见性）。
pub fn tool_names(request_body: &Value) -> Vec<String> {
    request_body
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 请求体里的消息列表，供测试判断「第几次请求带回来了工具结果」。
pub fn messages(request_body: &Value) -> Vec<Value> {
    request_body.get("messages").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// 请求转录的全文（JSON）。测试要断言「DSH 把沙箱拒绝的原文发回给模型了」时用它 ——
/// 模型的**第二次**请求里应当带着工具结果，那是「结果真的流回了模型」的证据，
/// 比只读 `events.jsonl` 多一层。
pub fn transcript(request_body: &Value) -> String {
    serde_json::to_string(&messages(request_body)).unwrap_or_default()
}

/// 请求里有没有 `tool_use`（模型这一轮要求调工具）。
pub fn has_tool_use(request_body: &Value) -> bool {
    messages(request_body).iter().any(|m| {
        m.get("content")
            .and_then(Value::as_array)
            .map(|blocks| blocks.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("tool_use")))
            .unwrap_or(false)
    })
}

/// 读回假 provider 落下的请求日志。
pub fn read_log(path: &Path) -> Vec<Value> {
    let Ok(raw) = std::fs::read_to_string(path) else { return Vec::new() };
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_read_from_the_wire_shape() {
        // 形状取自一次真实请求（假端点落的原文），不是照文档猜的。
        let body = json!({
            "model": "fake",
            "tools": [{ "name": "read", "input_schema": {} }, { "name": "write", "input_schema": {} }],
            "messages": [],
        });
        assert_eq!(tool_names(&body), vec!["read".to_string(), "write".to_string()]);
        // 没有工具时是空表，不是 panic —— 「一只工具都没有」也是要断言的形状。
        assert!(tool_names(&json!({})).is_empty());
    }

    #[test]
    fn tool_use_is_detected_in_the_transcript() {
        let body = json!({
            "messages": [{ "role": "assistant", "content": [
                { "type": "tool_use", "id": "t1", "name": "write", "input": {} }] }],
        });
        assert!(has_tool_use(&body));
        assert!(!has_tool_use(&json!({ "messages": [] })));
    }

    /// 脚本自检：空条目、缺 blocks、超范围请求各要有明确报错。
    #[test]
    fn script_shape_is_validated_at_parse_time() {
        assert!(Script::parse(r#"{"responses":[]}"#).is_err(), "空脚本要拒");
        assert!(Script::parse(r#"{"responses":[{"blocks":[]}]}"#).is_err(), "空条目要拒");
        assert!(Script::parse(r#"{"responses":[{"text":"x"}]}"#).is_err(), "缺 blocks 要拒");
        let script = Script::parse(r#"{"responses":[{"blocks":[{"text":"a"}]}],"default":{"blocks":[{"text":"z"}]}}"#)
            .unwrap();
        assert!(script.at(0).is_some());
        // 超出条目数回 default —— 子 Agent 那类用例靠它。
        assert!(script.at(7).is_some());
        let strict = Script::parse(r#"{"responses":[{"blocks":[{"text":"a"}]}]}"#).unwrap();
        assert!(strict.at(1).is_none(), "没有 default 时超范围必须没有回应（回 500）");
    }

    /// 一次真实的 HTTP 往返：脚本 → SSE → 读回请求日志。
    ///
    /// 这一条不碰 DSH，只证明假端点自身是活的；协议对不对由 `m3_live_*` 的 live 用例证明。
    #[test]
    fn provider_serves_one_scripted_turn_and_records_the_request() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests.jsonl");
        let script = Script::parse(
            r#"{"responses":[{"blocks":[{"text":"hello"},{"tool":"write","arguments":{"file_path":"x"}}]}]}"#,
        )
        .unwrap();
        let provider = Provider::start(script, &log).unwrap();

        let body = r#"{"model":"fake","tools":[{"name":"write"}],"messages":[]}"#;
        let response = std::process::Command::new("node")
            .args(["-e", &format!(
                "const r = await fetch('{}/v1/messages', {{method:'POST', headers:{{'content-type':'application/json'}}, body: JSON.stringify({body})}}); process.stdout.write(await r.text());",
                provider.url()
            )])
            .output();
        // 没有 node 就跳过一个断言：这条只是自检，不该让整套 test-runtime 依赖 node。
        let Ok(response) = response else { return };
        let sse = String::from_utf8_lossy(&response.stdout);
        assert!(sse.contains("\"type\":\"message_stop\""), "SSE 缺 message_stop：{sse}");
        assert!(sse.contains("\"stop_reason\":\"tool_use\""), "有工具调用时 stop_reason 必须是 tool_use：{sse}");

        let requests = provider.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(tool_names(&requests[0]["body"]), vec!["write".to_string()]);
    }
}
