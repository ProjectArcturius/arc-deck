//! Arc Deck 产品模型。
//!
//! 这一层不认识 DSH、不画终端、不开子进程；它只定义 Arc Deck 自己的公开契约：
//! 任务书、启动配置、Agent 定义、授权交集、worker 事件与最终结果。
//!
//! **依赖方向**：core 不依赖 `arc-deck-dsh` 或 `arc-deck-cli`（参 PRODUCT_PLAN §4.1）。
//! 这样 schema 与授权规则可以在没有任何运行时的情况下被确定性测试。
//!
//! 所有公开类型都在 `docs/PRODUCT_PLAN.md` §5–§6 有对应条目；改动这里的字段
//! 等于改动对外契约，必须同步夹具与文档。

pub mod agent;
pub mod authz;
pub mod config;
pub mod error;
pub mod event;
pub mod mode;
pub mod paths;
pub mod provider;
pub mod result;
pub mod task;
pub mod userconfig;

/// 当前 schema 版本。任务书、启动配置、事件信封、结果全部共用一个版本号。
///
/// 上游 DSH 没有版本协商（握手只回 `0.0.1` 且不校验），所以版本判断只能靠
/// 客户端自己钉；`schema_version` 就是那个锚点 —— 不匹配一律拒绝，不做尽力解析。
pub const SCHEMA_VERSION: u32 = 1;

/// 任务书或启动配置无效时的进程退出码（参 PRODUCT_PLAN §6.3）。
///
/// 与 `TaskStatus::exit_code` 的取值空间刻意分开：`64` 表示「还没开始跑」，
/// 而 `0..=5` 表示「跑过，结论是这个」。
pub const EXIT_SPEC_ERROR: i32 = 64;

pub use error::SpecError;
