//! DSH **Remote 层** —— 交互会话走的那一条路。
//!
//! 形状（逐条按 0.2.1-alpha.2 的源码核对，出处在 `docs/REMOTE_0.2.1.md`）：
//!
//! ```text
//! node <runtime>/lib/bin.js --profile web --patch <叠加层> --no-open --host 127.0.0.1 --port 0
//!   stdout: "dsh web: http://127.0.0.1:<port>/?token=<启动令牌>"
//!   GET 那个带票 URL                → 303 + Set-Cookie: dsh-auth-<…>=v1.…
//!   POST /api/<ns>/<method>         → 一元调用（带该 cookie）
//!   WS   /api/remote.mux            → 一条连接上多路复用的流（带该 cookie）
//! ```
//!
//! 与 SDK stdio 那条路（`transport.rs`）**并存而不是取代**：SDK 那条路只有单会话、
//! 没有会话列表与历史；web 这条路才有界面要的全部功能面。两条路都在用同一个
//! [`crate::projection`]，所以「界面上看到的」与「从日志读回来的」不会分叉。
//!
//! ## 这一层里没有一条结论是从 C# 那一版抄来的
//!
//! `arc-deck-dsh/src/ArcDeck.Dsh/Harness/` 对 **0.1.6** 实测过，而这里钉的是
//! **0.2.1-alpha.2**。形状相同的地方一律重新在源码里找到出处才用；不同的地方
//! （启动参数、换票路径、mux 的终止语义、心跳）在 `docs/REMOTE_0.2.1.md` 里列了差异表。
//!
//! ## 凭据纪律
//!
//! 带票 URL 是一份凭据：拿着它就能换到这个进程的浏览器会话 cookie。所以它只活在
//! 一个地方 —— [`host::WebKernel::start`] 里那一次换票调用；不进日志、不进错误消息、
//! `Debug` 里打码（[`Secret`]）。内核 stdout/stderr 的尾部会进启动失败的错误消息，
//! 那两处都先过一遍打码。
//!
//! ## `DSH_HOME` 必须由调用方给
//!
//! [`host::WebKernelOptions`] 里 `dsh_home` 是**必填**字段，没有默认值。这不是洁癖：
//! 指到 DSH Desktop 的真实 home 会**接管**那份 home 里的会话（`session/follow`
//! 会把冷会话 promote 成活动会话，见 AGENTS.md P15），而 Desktop 正开着时两个内核
//! 会碰同一份日志。

pub mod fold;
pub mod host;
pub mod launch;
pub mod mux;
pub mod rpc;
pub mod service;
pub mod session;
pub mod wire;

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod live;
#[cfg(test)]
mod quota;

pub use host::{WebKernel, WebKernelOptions};
pub use mux::{RemoteFrame, RemoteMux, RemoteStream};
pub use rpc::RemoteRpc;
pub use session::{OpenSession, SessionClient, SessionUpdate};
// `RemoteFailure` 就定义在本文件（见下），所以这里不能再 `pub use wire::…`：
// 那是重名，编不过（E0255）。
pub use wire::ServerFrame;

/// 一段凭据。
///
/// **故意不实现 `Display`，`Debug` 只打码** —— 这样它就没法随手进 `format!`、
/// 日志或错误消息。要用真值必须走 [`Secret::expose`]，而那个名字在 code review 里
/// 是看得见的。
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 取出真值。**调用点就是泄露点**，用之前想一眼它会不会进错误消息。
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Remote 层的一条结构化失败。
///
/// 与线上形状一一对应（`{code, message, details}`），**不拍平成一句话**：
/// `gateway/arguments-invalid` 与 `session/not-found` 对调用方是两回事，
/// 而 details 里常常带着是哪个字段出的问题。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("[{code}] {message}")]
pub struct RemoteFailure {
    pub code: String,
    pub message: String,
    pub details: serde_json::Value,
}

/// Remote 层错误。
///
/// 每一档都对应一个**不同的补救动作**，所以不合并成一个大字符串：
/// 启动失败要去看内核 stderr，未认证要去重新换票，超时要看内核是不是卡住了。
#[derive(Debug, Clone, thiserror::Error)]
pub enum RemoteError {
    /// 线上形状不对，或与我们对协议的理解不一致。
    #[error("协议错误：{0}")]
    Protocol(String),
    /// 与内核的 HTTP / TCP 交互失败。
    #[error("io 错误：{0}")]
    Io(String),
    #[error("等待 `{0}` 超时")]
    Timeout(String),
    /// 连接（或内核）已经结束，这次调用没有结果。
    #[error("连接已关闭：{0}")]
    Closed(String),
    /// 内核**结构化**地报告了一次失败。
    #[error("内核报告失败：{0}")]
    Remote(#[from] RemoteFailure),
    /// 内核没起来。消息里带着 stderr 尾部（已打码）。
    #[error("内核启动失败：{0}")]
    Startup(String),
    /// 换票或 cookie 没有通过校验。
    #[error("未认证：{0}")]
    Unauthenticated(String),
}

/// 把一段文本里所有已知凭据抹掉。
///
/// 用在两处：内核的诊断输出（stdout/stderr 尾部），以及任何可能捎带上
/// URL 的错误消息。**在凭据已知之前不要调用它** —— 它只能抹它认识的东西。
pub(crate) fn redact(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        if out.contains(secret) {
            out = out.replace(secret, "<已打码>");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_is_redacted() {
        let secret = Secret::new("dsh-auth-abc=v1.payload.signature");
        let printed = format!("{secret:?}");
        assert_eq!(printed, "Secret(<redacted>)");
        // 连片段都不该露出来。
        assert!(!printed.contains("dsh-auth"), "{printed}");
        assert!(!printed.contains("signature"), "{printed}");
    }

    #[test]
    fn redact_removes_every_known_secret() {
        let text = "dsh web: http://127.0.0.1:1/?token=T0KEN 和 cookie dsh-auth-x=v1.a.b";
        let cleaned = redact(text, &["T0KEN", "dsh-auth-x=v1.a.b"]);
        assert!(!cleaned.contains("T0KEN"), "{cleaned}");
        assert!(!cleaned.contains("v1.a.b"), "{cleaned}");
        assert!(cleaned.contains("127.0.0.1:1"), "不该连端口一起抹掉：{cleaned}");
    }
}
