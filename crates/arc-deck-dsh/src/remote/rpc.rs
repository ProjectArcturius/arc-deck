//! Remote 层的**一元调用**：`POST /api/<ns>/<method>`。
//!
//! 请求体是一个 Connection 信封（`client/connection/src/rpc.ts:80-85`）：
//!
//! ```text
//! { "type":"client-request", "rpcId":"<客户端铸的关联 id>", "method":"<ns>/<method>",
//!   "payload": { "args": { … } } }
//! ```
//!
//! 响应是 `{type:"server-response", rpcId, result}`，其中 `result` 是
//! `{ok:true,value}` 或 `{ok:false,error:{code,message,details}}`。
//! `ok:false` 的**结构化**带回是这一层的契约之一：`gateway/arguments-invalid`
//! 与 `session/not-found` 对调用方是两回事，拍平成一句话就把补救方向丢了。
//!
//! ## cookie 放在默认头上，不每次拼
//!
//! 漏一处就是 401，而 401 的表现是「什么都没有」—— 不指向 cookie。
//!
//! ## 已知未做
//!
//! **带附件的响应没实现。** 一元结果里含二进制时，内核回的是 `multipart/form-data`，
//! JSON 在 `metadata` part 里、字节在别的 part 里
//! （`api/gateway/src/index.ts:301-318`）。会话消息里的图片会走到那条路；
//! 当前这层只认 `application/json`，遇到 multipart 会**明说**不支持，
//! 而不是把 metadata 当成结果悄悄返回半个值。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;

use super::wire;
use super::{RemoteError, Secret};

/// 一元调用的默认上限。
///
/// 比内核自己的超时宽一点：`session/list` 在大会话目录上要扫盘，`session/page`
/// 要解压，但都不该到分钟级。超时是**这一侧**的判断，不冒充内核的结论。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct RemoteRpc {
    http: reqwest::Client,
    origin: String,
    cookie: Secret,
    next_id: AtomicU64,
}

impl std::fmt::Debug for RemoteRpc {
    /// 手写：派生会把 cookie 原样打出来。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteRpc")
            .field("origin", &self.origin)
            .field("cookie", &self.cookie)
            .finish()
    }
}

impl RemoteRpc {
    /// 对一个已经在跑的内核建一个调用句柄。
    pub fn for_kernel(kernel: &super::WebKernel) -> Result<Self, RemoteError> {
        RemoteRpc::new(kernel.origin(), kernel.cookie())
    }

    /// 直接对 `origin` + cookie 建句柄。假服务端（离线测试）走的就是这条路径。
    pub fn new(origin: &str, cookie: &Secret) -> Result<Self, RemoteError> {
        let http = reqwest::Client::builder()
            // 一律不跟随：`/api` 上的重定向都不是正常路径，跟着走只会把
            // 「内核报了 303」变成「拿回一个 HTML 页面」。
            .redirect(reqwest::redirect::Policy::none())
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| RemoteError::Io(e.without_url().to_string()))?;
        Ok(Self {
            http,
            origin: origin.trim_end_matches('/').to_string(),
            cookie: cookie.clone(),
            next_id: AtomicU64::new(1),
        })
    }

    /// 调一个一元方法（`"session/list"` 这种 `<ns>/<method>` 全名）。
    ///
    /// `args` 的字段名要与 Host 形参**逐字**一致 —— 见 [`crate::remote::wire`] 顶部。
    pub async fn call(&self, method: &str, args: Value) -> Result<Value, RemoteError> {
        if !wire::valid_method(method) {
            return Err(RemoteError::Protocol(format!("不是一个合法的 Remote 方法名：{method:?}")));
        }
        let rpc_id = format!(
            "arcdeck-{}-{}",
            std::process::id(),
            self.next_id.fetch_add(1, Ordering::SeqCst)
        );
        let envelope = wire::encode_call(&rpc_id, method, &args)?;
        let url = format!("{}{}/{}", self.origin, wire::API_PATH, method);

        let response = self
            .http
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::COOKIE, self.cookie.expose())
            .body(envelope.to_string())
            .send()
            .await
            // `without_url()`：reqwest 的错误默认带请求 URL。这条 URL 里没有凭据，
            // 但保持这个习惯 —— 下一次有人拿带票 URL 去请求时不会忘。
            .map_err(|e| {
                if e.is_timeout() {
                    RemoteError::Timeout(format!(
                        "`{method}` 在 {} 秒内没有响应",
                        DEFAULT_TIMEOUT.as_secs()
                    ))
                } else {
                    RemoteError::Io(format!("`{method}` 请求失败：{}", e.without_url()))
                }
            })?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(RemoteError::Unauthenticated(
                "内核回了 401 —— 浏览器会话 cookie 无效或已过期".into(),
            ));
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(RemoteError::Unauthenticated(format!(
                "内核回了 403 —— Host/Origin 栅栏不接受 {}",
                self.origin
            )));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(RemoteError::Protocol(format!(
                "内核回了 404 —— 它没有 `{method}` 这个 endpoint"
            )));
        }
        if !status.is_success() {
            return Err(RemoteError::Io(format!(
                "`{method}` 返回 HTTP {status}（{}）",
                response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("没有 content-type")
            )));
        }

        // 只认 JSON。带附件的 multipart 结果**明说**不支持，不假装成功。
        let media_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
            .unwrap_or_default();
        if media_type.starts_with("multipart/") {
            return Err(RemoteError::Protocol(format!(
                "`{method}` 的结果是 {media_type} —— 带附件的响应这一层还没实现\
                 （二进制在别的 part 里，见 remote/rpc.rs 顶部「已知未做」）"
            )));
        }

        let body = response
            .text()
            .await
            .map_err(|e| RemoteError::Io(format!("`{method}` 的响应体读不出来：{}", e.without_url())))?;
        let parsed: Value = serde_json::from_str(&body).map_err(|e| {
            RemoteError::Protocol(format!(
                "`{method}` 的响应不是 JSON（{e}）；前 200 字节：{}",
                first_bytes(&body, 200)
            ))
        })?;
        wire::decode_response(&rpc_id, &parsed)
    }
}

/// 一段文本的前若干**字符**（不是字节）：错误消息里带正文时，切在多字节字符中间
/// 会造出替换字符，把一条真错误变成一条看不出来的乱码。
fn first_bytes(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 一个只会按脚本回答一元 POST 的假内核：**手写 HTTP/1.1**，为了能精确摆出
    /// 401 / 404 / multipart / rpcId 不匹配这些真实形状。
    ///
    /// 响应体里的 `__RPCID__` 会被换成请求里那个 `rpcId` —— 真实的
    /// `server-response` 必须把它回显回来，写死一个就测不出「关联」这件事。
    struct FakeHttp {
        port: u16,
        requests: std::sync::mpsc::Receiver<String>,
        _thread: std::thread::JoinHandle<()>,
    }

    impl FakeHttp {
        fn start(body_template: &str) -> FakeHttp {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let (tx, requests) = std::sync::mpsc::channel();
            let template = body_template.to_string();
            let thread = std::thread::spawn(move || {
                use std::io::{Read, Write};
                // 一条测试一个请求就够：接一次、记下来、回一句。
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buffer = vec![0u8; 64 * 1024];
                let count = stream.read(&mut buffer).unwrap_or(0);
                let raw = String::from_utf8_lossy(&buffer[..count]).to_string();
                let rpc_id = raw
                    .split_once("\r\n\r\n")
                    .and_then(|(_, body)| serde_json::from_str::<Value>(body).ok())
                    .and_then(|value| value["rpcId"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "X".into());
                let _ = tx.send(raw);
                let body = template.replace("__RPCID__", &rpc_id);
                let response = if body.starts_with("HTTP/") {
                    body
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });
            FakeHttp { port, requests, _thread: thread }
        }

        fn origin(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        fn request(&self) -> String {
            self.requests
                .recv_timeout(Duration::from_secs(5))
                .expect("假内核没收到请求")
        }
    }

    fn cookie() -> Secret {
        Secret::new("dsh-auth-abc=v1.body.sig")
    }

    #[tokio::test]
    async fn the_request_is_a_connection_envelope_with_the_cookie() {
        let server = FakeHttp::start(
            r#"{"type":"server-response","rpcId":"__RPCID__","result":{"ok":true,"value":{"items":[]}}}"#,
        );
        let rpc = RemoteRpc::new(&server.origin(), &cookie()).unwrap();

        let result = rpc.call("session/list", json!({ "_request": {} })).await.unwrap();
        assert_eq!(result, json!({ "items": [] }));

        let raw = server.request();
        let (head, payload) = raw.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("POST /api/session/list HTTP/1.1"), "{head}");
        assert!(
            head.to_ascii_lowercase().contains("cookie: dsh-auth-abc=v1.body.sig"),
            "{head}"
        );
        assert!(
            head.to_ascii_lowercase().contains("content-type: application/json"),
            "{head}"
        );
        let sent: Value = serde_json::from_str(payload).unwrap();
        assert_eq!(sent["type"], "client-request");
        assert_eq!(sent["method"], "session/list");
        assert_eq!(sent["payload"]["args"]["_request"], json!({}));
        // payload 的键集合必须**恰好**是 {args}：Gateway 先查这个再查参数名。
        assert_eq!(sent["payload"].as_object().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_response_carrying_someone_elses_rpc_id_is_refused() {
        let server = FakeHttp::start(
            r#"{"type":"server-response","rpcId":"someone-else","result":{"ok":true,"value":1}}"#,
        );
        let rpc = RemoteRpc::new(&server.origin(), &cookie()).unwrap();
        let error = rpc.call("session/list", json!({ "_request": {} })).await.unwrap_err();
        let _ = server.request();
        assert!(matches!(error, RemoteError::Protocol(_)), "{error}");
        assert!(error.to_string().contains("rpcId"), "{error}");
    }

    #[tokio::test]
    async fn a_404_is_reported_as_a_missing_endpoint() {
        let server = FakeHttp::start("HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nnot found");
        let rpc = RemoteRpc::new(&server.origin(), &cookie()).unwrap();
        let error = rpc.call("nope/nope", json!({})).await.unwrap_err();
        let _ = server.request();
        assert!(matches!(error, RemoteError::Protocol(_)), "{error}");
        assert!(error.to_string().contains("404"), "{error}");
    }

    #[tokio::test]
    async fn a_401_is_reported_as_unauthenticated() {
        let server =
            FakeHttp::start("HTTP/1.1 401 Unauthorized\r\nContent-Length: 11\r\n\r\nunauthorized");
        let rpc = RemoteRpc::new(&server.origin(), &cookie()).unwrap();
        let error = rpc.call("session/list", json!({ "_request": {} })).await.unwrap_err();
        let _ = server.request();
        assert!(matches!(error, RemoteError::Unauthenticated(_)), "{error}");
    }

    #[tokio::test]
    async fn a_multipart_result_is_not_silently_read_as_json() {
        let server = FakeHttp::start(
            "HTTP/1.1 200 OK\r\nContent-Type: multipart/form-data; boundary=x\r\nContent-Length: 0\r\n\r\n",
        );
        let rpc = RemoteRpc::new(&server.origin(), &cookie()).unwrap();
        let error = rpc.call("session/attachment", json!({ "request": {} })).await.unwrap_err();
        let _ = server.request();
        assert!(matches!(error, RemoteError::Protocol(_)), "{error}");
        assert!(error.to_string().contains("multipart"), "{error}");
    }

    #[tokio::test]
    async fn bytes_that_are_not_json_are_reported_with_a_readable_head() {
        let body = "这不是 JSON，是中文字节";
        let server = FakeHttp::start(&format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        let rpc = RemoteRpc::new(&server.origin(), &cookie()).unwrap();
        let error = rpc.call("session/list", json!({ "_request": {} })).await.unwrap_err();
        let _ = server.request();
        let text = error.to_string();
        assert!(matches!(error, RemoteError::Protocol(_)), "{text}");
        // 切在多字节字符中间会造出替换字符，把一条真错误变成乱码。
        assert!(!text.contains('\u{fffd}'), "{text}");
        assert!(text.contains("这不是 JSON"), "{text}");
    }

    #[tokio::test]
    async fn connection_refused_is_an_io_error_not_a_timeout() {
        // 端口 1 上没人听。
        let rpc = RemoteRpc::new("http://127.0.0.1:1", &cookie()).unwrap();
        let error = rpc.call("session/list", json!({ "_request": {} })).await.unwrap_err();
        assert!(matches!(error, RemoteError::Io(_)), "{error}");
    }

    #[tokio::test]
    async fn a_structured_failure_survives_the_whole_path() {
        let server = FakeHttp::start(
            r#"{"type":"server-response","rpcId":"__RPCID__","result":{"ok":false,"error":{"code":"gateway/arguments-invalid","message":"args fields do not match the descriptor: missing \"_request\"","details":{"endpoint":"session/list"}}}}"#,
        );
        let rpc = RemoteRpc::new(&server.origin(), &cookie()).unwrap();
        let error = rpc.call("session/list", json!({})).await.unwrap_err();
        let _ = server.request();
        match error {
            RemoteError::Remote(failure) => {
                assert_eq!(failure.code, "gateway/arguments-invalid");
                assert!(failure.message.contains("_request"), "{}", failure.message);
                assert_eq!(failure.details["endpoint"], "session/list");
            }
            other => panic!("期望结构化远端失败，得到 {other}"),
        }
    }

    #[test]
    fn the_debug_of_an_rpc_handle_does_not_print_the_cookie() {
        let rpc = RemoteRpc::new("http://127.0.0.1:1", &cookie()).unwrap();
        let printed = format!("{rpc:?}");
        assert!(!printed.contains("v1.body.sig"), "{printed}");
        assert!(printed.contains("redacted"), "{printed}");
    }
}
