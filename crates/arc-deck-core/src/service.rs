//! 前端 ↔ 会话服务的消息：TUI 在进程内用它，GUI 将来经 IPC 用**同一组**消息。
//!
//! 为什么现在就把它定成一份 serde 协议，而不是先在 TUI 里写死函数调用：
//! GUI 要「热插拔」—— 独立安装、独立升级、随时连上 / 断开同一个后端（docs/GUI_PLUGGABLE.md）。
//! 那要求前后端之间只有**一份**可序列化的契约。TUI 先用它，等于每天都在对这份契约做验收；
//! 等 `arcdeck serve` 把它搬到 socket 上时，后端代码一行不改。
//!
//! 版本与兼容的规则（GUI 能独立升级的前提）：
//! - 握手交换 [`PROTOCOL_VERSION`]；**主版本不同拒绝连接**，次要变化只允许「加」。
//! - 加字段一律 `#[serde(default)]`；加命令 / 事件种类时，旧的一方以 `Unknown` 收下并忽略，
//!   不得整条连接报错（`#[serde(other)]`）。
//! - 不复用、不改义已有字段名 —— 改义就是主版本。

use serde::{Deserialize, Serialize};

use crate::conversation::{Conversation, ModelCatalog, ModelChoice, SessionSummary};

/// 协议主版本。改义或删除字段时加一；只「加」不算。
pub const PROTOCOL_VERSION: u32 = 1;

/// 前端 → 后端。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// IPC 握手。进程内不需要。
    Hello { protocol: u32, client: String },
    /// 刷新会话列表（回 [`Event::Sessions`]）。
    ListSessions,
    /// 打开一个已有会话并订阅它的更新（回 [`Event::Conversation`]，之后持续推送）。
    OpenSession { session_id: String },
    /// 不再订阅这个会话。
    CloseSession { session_id: String },
    /// 发一条消息。`session_id` 为空时**新建**会话（回 [`Event::SessionStarted`]）。
    /// `model` 有值时先为这个会话选模型 —— 作用于这一轮及以后（§7.2：不是热切换进行中的那一轮）。
    Prompt {
        #[serde(default)]
        session_id: Option<String>,
        text: String,
        #[serde(default)]
        model: Option<ModelChoice>,
    },
    /// 取消这个会话当前这一轮。
    Cancel { session_id: String },
    Rename { session_id: String, title: String },
    /// 重新读模型目录（回 [`Event::Catalog`]）。
    Catalog,
    /// 收掉后端（进程内：TUI 退出时）。IPC 上只断开本连接，不关服务。
    Shutdown,
    #[serde(other)]
    Unknown,
}

/// 后端 → 前端。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// IPC 握手的回答。主版本不同时 `accepted = false`，`reason` 说清楚谁该升级。
    Welcome {
        protocol: u32,
        accepted: bool,
        #[serde(default)]
        reason: Option<String>,
        /// 后端的产品版本（`arcdeck --version` 那个），给前端显示 / 诊断用。
        #[serde(default)]
        backend: String,
    },
    /// 后端状态。前端据此决定显示「正在启动内核」还是可操作的错误。
    Backend { state: BackendState },
    Sessions { sessions: Vec<SessionSummary> },
    Catalog { catalog: ModelCatalog },
    /// 一个新会话建好了（来自 `Prompt { session_id: None }`）。
    SessionStarted { session_id: String },
    /// 一个已订阅会话的完整视图。v1 发整份；变更粒度的增量是以后的「加」。
    Conversation { conversation: Box<Conversation> },
    /// 订阅结束（连接断了 / 内核报错 / 被关掉），原因原样带出。
    SessionClosed { session_id: String, reason: String },
    /// 给人看的一句话。`level` 决定颜色，不决定行为。
    Notice { level: NoticeLevel, text: String },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BackendState {
    Starting,
    /// `model` 是会话默认会用的那条（用户配置的路由，不是内核的出厂默认）。
    Ready { model: Option<ModelChoice> },
    /// 起不来。`reason` 要可操作（「先运行 `arcdeck runtime install`」），不是一段栈。
    Failed { reason: String },
    Stopped,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
    #[serde(other)]
    Unknown,
}

/// 握手判定：后端用它回答 `Hello`。
pub fn negotiate(client_protocol: u32, backend: &str) -> Event {
    if client_protocol == PROTOCOL_VERSION {
        Event::Welcome { protocol: PROTOCOL_VERSION, accepted: true, reason: None, backend: backend.into() }
    } else {
        let who = if client_protocol < PROTOCOL_VERSION { "前端" } else { "后端 arcdeck" };
        Event::Welcome {
            protocol: PROTOCOL_VERSION,
            accepted: false,
            reason: Some(format!(
                "协议主版本不一致：前端 {client_protocol}，后端 {PROTOCOL_VERSION}；请升级{who}"
            )),
            backend: backend.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_events_round_trip_as_tagged_json() {
        let command = Command::Prompt { session_id: None, text: "你好".into(), model: None };
        let raw = serde_json::to_string(&command).unwrap();
        assert!(raw.contains(r#""op":"prompt""#), "{raw}");
        assert_eq!(serde_json::from_str::<Command>(&raw).unwrap(), command);

        let event = Event::Backend { state: BackendState::Failed { reason: "x".into() } };
        let raw = serde_json::to_string(&event).unwrap();
        assert!(raw.contains(r#""event":"backend""#) && raw.contains(r#""state":"failed""#), "{raw}");
        assert_eq!(serde_json::from_str::<Event>(&raw).unwrap(), event);
    }

    #[test]
    fn a_newer_peer_is_tolerated_not_disconnected() {
        // 新后端发了一个旧前端不认识的事件；新前端发了一个旧后端不认识的命令。
        assert_eq!(serde_json::from_str::<Event>(r#"{"event":"todo_list","items":[]}"#).unwrap(), Event::Unknown);
        assert_eq!(serde_json::from_str::<Command>(r#"{"op":"fork","session_id":"s"}"#).unwrap(), Command::Unknown);
        // 已知命令多了一个字段：照常读。
        let prompt: Command = serde_json::from_str(r#"{"op":"prompt","text":"hi","attachments":[]}"#).unwrap();
        assert!(matches!(prompt, Command::Prompt { session_id: None, .. }));
    }

    #[test]
    fn a_major_version_mismatch_is_refused_with_who_should_upgrade() {
        let Event::Welcome { accepted, reason, .. } = negotiate(PROTOCOL_VERSION + 1, "0.1.0") else {
            panic!()
        };
        assert!(!accepted);
        assert!(reason.unwrap().contains("请升级后端"));
        assert!(matches!(negotiate(PROTOCOL_VERSION, "0.1.0"), Event::Welcome { accepted: true, .. }));
    }
}
