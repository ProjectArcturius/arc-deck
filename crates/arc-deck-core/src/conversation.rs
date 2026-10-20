//! 交互会话的**中立视图**：TUI 与 GUI 直接画的形状。
//!
//! 为什么放在 core 而不是 dsh：GUI 经版本化 IPC 消费它（PRODUCT_PLAN §4.1、§8），
//! 而「界面不直接解释 DSH 私有事件」是那条约束的全部意义。所以这里**不出现**任何 DSH
//! 的词汇（`assistant/attempt`、`surfaceOp`、`streamId`……）；把 DSH 折成它的代码在
//! `arc-deck-dsh::remote::fold`，换内核版本时改那里，界面与 IPC 契约不动。
//!
//! 全部 `Serialize + Deserialize`：IPC 的帧就是这些类型的 JSON。字段名用 `snake_case`，
//! 新增字段一律带 `#[serde(default)]` —— 新后端配旧前端（或反过来）时不能因为多一个字段就读不了，
//! 这是 GUI 能独立升级（热插拔）的前提之一。

use serde::{Deserialize, Serialize};

/// 一个会话的完整视图。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Conversation {
    pub session_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub messages: Vec<Message>,
    /// 历史是否已取全。打开会话时快照只是**一页**（AGENTS.md P13），往回翻完才为 true。
    pub history_complete: bool,
    /// 有一轮正在进行。
    pub running: bool,
    /// 下一轮会用的模型（内核的 `modelSelection.next`）。界面上「切换模型」作用于它。
    pub model: Option<ModelChoice>,
}

/// 一条消息。人说的话与助手的一轮分开 —— 一轮里助手的多个 step 并成一条（P8）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantTurn),
    /// 更新的后端发来的、本前端不认识的消息种类。**跳过而不是整帧读失败** ——
    /// 内部标签的枚举遇到未知标签默认报错，那会让旧 GUI 在新后端上整个会话打不开。
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserMessage {
    pub turn: u32,
    pub text: String,
    /// 附件之类没有文本的部分，按顺序记成占位说明（「[图片]」），不静默丢掉。
    pub attachments: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantTurn {
    pub turn: u32,
    pub blocks: Vec<Block>,
    pub state: TurnState,
    pub usage: Option<TokenUsage>,
    /// 这一轮里失败后被重试的模型调用次数。成功的轮次里它只是一个小注记；
    /// 失败的轮次里最后一次的原文在 [`TurnState::Failed`]。
    pub retries: u32,
}

impl AssistantTurn {
    /// 这一轮的正文（只取文本块，跳过思考与工具）。测试与复制用。
    pub fn text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            if let Block::Text(part) = block {
                if !out.is_empty() && !part.text.is_empty() {
                    out.push('\n');
                }
                out.push_str(&part.text);
            }
        }
        out
    }
}

/// 助手一轮里的一块。顺序就是发生顺序。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Text(TextPart),
    Reasoning(TextPart),
    Tool(ToolCall),
    /// 本前端不认识的块（理由同 [`Message::Unknown`]）。
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextPart {
    pub text: String,
    /// 还在流式增长、权威全文没到（P4）。界面可以据此画光标；**内容以后会被整块替换**。
    pub streaming: bool,
    /// 属于这一轮的第几个 step（模型的第几次调用）。
    pub step: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    /// 模型给的原始参数（JSON 字符串，**不保证**是合法 JSON —— 模型可能写坏）。
    pub arguments: String,
    pub status: ToolStatus,
    /// 结果正文（给模型看的那份）。长输出由界面折叠，这里不截。
    pub output: Option<String>,
    /// shell 类工具的 `[exit code: N]` 标记。没有标记不等于 0（见 dsh 的 `exit_code_marker`）。
    pub exit_code: Option<i32>,
    /// 内核给的失败原因（面向人，不在模型可见的正文里）。
    pub error: Option<String>,
    /// 工具私有的呈现载荷（例如文件工具附带的 diff）。形状归工具所有，界面按工具名认。
    pub detail: Option<serde_json::Value>,
    pub step: u32,
}

impl ToolCall {
    /// 参数里的某个字符串字段（`command`、`path`……）。参数写坏时返回 None，不 panic。
    pub fn argument(&self, key: &str) -> Option<String> {
        let value: serde_json::Value = serde_json::from_str(&self.arguments).ok()?;
        value.get(key)?.as_str().map(str::to_string)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    #[default]
    Running,
    Done,
    /// 工具报错，或被执行层拒绝（越权写、被摘掉的工具）。拒绝的原因在 `error` / `output` 里。
    Failed,
    /// 这一轮结束了它还没有结果（被取消、或内核没写结果）。真实日志里稳定存在少量这种调用（P9）。
    Abandoned,
    #[serde(other)]
    Unknown,
}

/// 一轮的状态。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TurnState {
    #[default]
    Running,
    Completed,
    /// 用户取消。
    Cancelled,
    /// 模型或内核失败。`message` 是上游原文（P3：模型失败的唯一出口），`code` 是上游分类。
    Failed { message: String, code: Option<String> },
    /// 达到输出上限。
    MaxTokens,
    /// 被阻塞（例如策略拒绝）。
    Blocked,
    /// 内核崩溃留下的半截轮次，事后被补上的收尾。
    Interrupted,
    /// 认不出的结束原因：原样带出来，不猜。
    Other { kind: String },
    /// 更新的后端加的状态，本前端不认识。
    #[serde(other)]
    Unknown,
}

impl TurnState {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, TurnState::Running)
    }
}

/// 内核把一个问题交给人：某个工具调用要执行，但策略要求先问。
///
/// 字段全部来自 DSH 的 `approval/request` 载荷（`packages/interaction/user-approval/src/types.ts`）：
/// `tool_name` / `call_id` / `reason` 原样带出来，界面不去猜参数 —— 工具参数在
/// 对话视图里已经有一份（[`ToolCall::arguments`]），需要时按 `call_id` 找。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalRequest {
    /// 应答时要带回来的 id（Host 的 `eventId`），**不是** DSH 的 `ApprovalRequestId`。
    pub request_id: String,
    pub tool_name: String,
    pub call_id: Option<String>,
    /// 提问方给的理由（hook 的 `permissionDecision` 之类）。
    pub reason: Option<String>,
    /// 本地化过的展示理由；有就优先显示。
    pub display_reason: Option<String>,
    /// 发起调用的 Agent（DSH 的 agent id），只用于显示。
    pub agent: Option<String>,
}

/// 审批的应答。DSH 有四种结果，我们只答两种：
/// 「允许这一次」与「拒绝」。`cancelled` / `unavailable` 是内核自己的判断，不是用户的选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalOutcome {
    AllowOnce,
    Reject,
    #[serde(other)]
    Unknown,
}

/// `session/search` 的一条命中：哪个会话、上下文片段。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchHit {
    pub session_id: String,
    pub title: Option<String>,
    /// 命中处的片段。命中词用 `\u{0}` 夹起来（内核的约定），界面负责分段着色。
    pub snippet: String,
}

/// provider 实际报告的 token 数。缺的字段是「没报告」，不是 0。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenUsage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
}

impl TokenUsage {
    /// 累加；任一边有值就有值。
    pub fn add(&mut self, other: &TokenUsage) {
        fn sum(a: Option<u64>, b: Option<u64>) -> Option<u64> {
            match (a, b) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
            }
        }
        self.input = sum(self.input, other.input);
        self.output = sum(self.output, other.output);
        self.cache_read = sum(self.cache_read, other.cache_read);
        self.cache_write = sum(self.cache_write, other.cache_write);
    }
}

/// 一条模型选择。`provider` 是内核认的路由名（官方 DeepSeek 是 `deepseek-official`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelChoice {
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
}

/// 会话列表的一行。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    /// 毫秒时间戳（内核的 `updatedAt`）。
    pub updated_at: u64,
    pub running: bool,
    /// 还没有任何一轮。
    pub blank: bool,
    /// 子 agent 会话的父会话。列表默认不显示子会话。
    pub parent_session_id: Option<String>,
    /// 子 agent 的模式（`one-shot` / `continuable`）。打开子会话必须用**父地址**，
    /// 而地址里的 `mode` 只有这里有（P13）。
    pub subagent_mode: Option<String>,
    /// 子 agent 自己的名字（`subagent.label`）—— 列表上比 id 有用。
    pub subagent_label: Option<String>,
}

/// 内核报告的模型目录（§7.2：权威来源是 DSH）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelCatalog {
    /// 内核自己的默认值。**不一定**是用户配置的那条 —— 实测它是 DSH 的出厂默认
    /// （`deepseek-official/deepseek-flash`），所以会话创建后要显式选一次。
    pub default: Option<ModelChoice>,
    pub providers: Vec<CatalogProvider>,
    /// 目录加载失败的 provider：显式列出来并说明原因（§7.2：不可用不是悄悄消失）。
    pub failures: Vec<CatalogFailure>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogProvider {
    pub id: String,
    pub name: String,
    pub models: Vec<CatalogModel>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// 内核报的上下文窗口（token 数）。没有报就不显示占用百分比（§7.3）。
    pub context_window: Option<u64>,
    pub efforts: Vec<CatalogEffort>,
    pub default_effort: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogEffort {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogFailure {
    pub id: String,
    pub name: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_frontend_reads_a_newer_backend_frame() {
        // 热插拔的前提：后端多了字段，旧前端照样读得出它认识的那部分。
        let raw = r#"{"session_id":"s","messages":[{"role":"assistant","turn":1,
            "blocks":[{"kind":"text","text":"hi","streaming":false,"step":1,"future_field":3}],
            "state":{"state":"completed"},"brand_new":true}],"also_new":1}"#;
        let view: Conversation = serde_json::from_str(raw).unwrap();
        let Message::Assistant(turn) = &view.messages[0] else { panic!() };
        assert_eq!(turn.text(), "hi");
        assert_eq!(turn.state, TurnState::Completed);
    }

    #[test]
    fn unknown_kinds_from_a_newer_backend_degrade_instead_of_failing() {
        let raw = r#"{"session_id":"s","messages":[
            {"role":"system_note","text":"新后端才有的消息种类"},
            {"role":"assistant","turn":1,"blocks":[{"kind":"image","url":"x"},{"kind":"text","text":"ok"}],
             "state":{"state":"paused_for_review"}}]}"#;
        let view: Conversation = serde_json::from_str(raw).expect("未知种类不该让整帧读失败");
        assert_eq!(view.messages[0], Message::Unknown);
        let Message::Assistant(turn) = &view.messages[1] else { panic!() };
        assert_eq!(turn.blocks[0], Block::Unknown);
        assert_eq!(turn.text(), "ok");
        assert_eq!(turn.state, TurnState::Unknown);
    }

    #[test]
    fn usage_addition_keeps_unreported_fields_absent() {
        let mut total = TokenUsage::default();
        total.add(&TokenUsage { input: Some(10), output: Some(2), ..TokenUsage::default() });
        total.add(&TokenUsage { input: Some(5), ..TokenUsage::default() });
        assert_eq!(total.input, Some(15));
        assert_eq!(total.output, Some(2));
        assert_eq!(total.cache_read, None, "没人报告过的缓存数不该变成 0");
    }
}
