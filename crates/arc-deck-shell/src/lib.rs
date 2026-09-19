// Arc Deck · 渲染契约与平台外壳
//
// 分层（依赖方向单向，无环）：
//
// ```text
// scene / input / ime(ImeState)   —— 平台无关，零依赖，可单测
//        ▲
//        │
// app::App / TextEngine           —— 应用与外壳的唯一边界
//        ▲
//        │
// platform::{windows, …}          —— 平台外壳：窗口 / 消息泵 / IME 绑定 / 光栅化
// ```
//
// **`App` 不持有任何平台句柄。** 于是同一份 App 逻辑能跑在 Win32 外壳下，
// 也能跑在将来的 Wayland 外壳下 —— 这正是 Kanesumi 的 `kanesumi-harness`
// 已经验证过的结构（其 `app.rs` 零 `cfg(target_os)` 门控，而平台代码全部
// 隔离在 `platform.rs` / `render.rs` 内）。

pub mod app;
pub mod ime;
pub mod input;
pub mod scene;

#[cfg(windows)]
pub mod platform;

pub use app::{App, AppConfig, HeuristicTextEngine, TextEngine};
pub use ime::ImeState;
pub use input::{InputEvent, Key, Modifiers, PointerButton};
pub use scene::{Color, Point, Rect, Scene, SceneCommand, Size, TextAlign};
