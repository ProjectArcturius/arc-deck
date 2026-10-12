//! DSH SDK 线上的方法与信封形状。
//!
//! 依据是 `REFS/deepseek-harness/packages/sdk/protocol/README.md`（提交
//! `d7432673`）：JSON-RPC 2.0，**每条消息一行、以 `\n` 结束**，
//! 不是 HTTP，也不是 Content-Length 分帧。
//!
//! 这里只放 worker 真正用到的六个方法与两类通知，不做完整协议镜像 ——
//! 镜像一份自己不消费的类型只会让它慢慢漂移。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 客户端发起的请求方法名。
pub mod method {
    pub const INITIALIZE: &str = "initialize";
    pub const SESSION_PROMPT: &str = "session/prompt";
    pub const SESSION_WAIT: &str = "session/wait";
    pub const SHUTDOWN: &str = "shutdown";
}

/// 服务端通知方法名。
pub mod notification {
    pub const SESSION_EVENT: &str = "session.event";
    pub const SESSION_STATUS: &str = "session.status";
    pub const SUBAGENT_STARTED: &str = "subagent.started";
    pub const SUBAGENT_FINISHED: &str = "subagent.finished";
}

/// `initialize` 的参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub cwd: String,
    pub provider: String,
    pub model: String,
}

/// `initialize` 的返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub server_info: ServerInfo,
}

/// 对侧的自我描述。
///
/// `version` 在真实 DSH 上恒为 `0.0.1` 且客户端不校验（协议无版本协商），
/// 所以它**不能**当作「对侧是什么」的判据 —— 版本靠客户端自己钉。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// `session/prompt` 的参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptParams {
    pub session_id: String,
    pub content_blocks: Vec<Value>,
}

/// `session/prompt` 的返回：**只是入队回执**，不代表模型已经回答。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptResult {
    pub message_id: String,
}

/// `session/wait` 的参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWaitParams {
    pub session_id: String,
}

/// `session.event` 通知的参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventParams {
    pub session_id: String,
    pub event: Value,
}

/// 构造一条纯文本的 `contentBlocks`。
pub fn text_content_block(text: &str) -> Vec<Value> {
    vec![serde_json::json!({ "type": "text", "text": text })]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_params_use_camel_case_on_the_wire() {
        let params = SessionPromptParams {
            session_id: "s1".into(),
            content_blocks: text_content_block("你好"),
        };
        let v = serde_json::to_value(&params).unwrap();
        assert_eq!(v["sessionId"], "s1");
        assert_eq!(v["contentBlocks"][0]["text"], "你好");
    }

    #[test]
    fn initialize_result_reads_real_sdk_shape() {
        let raw = r#"{"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"0.0.1"}}"#;
        let parsed: InitializeResult = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.server_info.name, "deepseek-harness-sdk-runtime");
        assert_eq!(parsed.server_info.version, "0.0.1");
    }
}
