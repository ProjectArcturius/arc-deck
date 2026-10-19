//! Arc Deck 的 DSH 适配层。
//!
//! 这一层负责与运行时说话，并把它的事件折成产品视图；它**不**决定任务是否
//! 完成 —— 那件事由 `arc-deck-cli` 的 worker 按规则判定（参 PRODUCT_PLAN §6.3）。
//!
//! 依赖方向：`arc-deck-core` ← 本 crate ← `arc-deck-cli`。

pub mod process;
pub mod projection;
pub mod protocol;
pub mod remote;
pub mod runtime;
pub mod transport;

/// 假 SDK 对端。只在 `test-runtime` feature 下存在。
#[cfg(feature = "test-runtime")]
pub mod fixture;

/// 脚本化的假模型端点（M3 负例验收）。只在 `test-runtime` feature 下存在。
///
/// 与 `fixture` 的分工：`fixture` 假装的是**内核**（JSON-RPC 对端），
/// 这个假装的是**模型 provider**（HTTP + SSE）—— 真实内核照常运行，
/// 只有「模型说什么」是可编程的。执行层的拒绝只有在这条路上才看得见。
#[cfg(feature = "test-runtime")]
pub mod fake_provider;

pub use projection::{Projection, ToolOutcome, ToolView, TurnEnd, TurnView};
pub use protocol::{method, notification};
pub use transport::{DshError, Inbound, Transport};
