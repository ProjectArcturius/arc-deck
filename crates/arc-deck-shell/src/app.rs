// Arc Deck · App trait 与文本度量契约
//
// `App` 是「应用」与「平台外壳」之间**唯一**的边界。它刻意做成纯逻辑：
// 不持有任何窗口句柄、GPU 设备、或文件路径 —— 于是它能在没有窗口的环境下
// 被单元测试，也能在 Win32 与将来的 Wayland 外壳下复用同一份实现。
//
// 这个形状对齐 Kanesumi 的 `kanesumi_harness::App`（`app.rs`，782 行，零
// `cfg(target_os)` 门控）—— 上游已经证明这个契约足以支撑真实应用：Ether 的
// TopBar / Settings / Librarian / Launcher 全部实现它。

use crate::input::InputEvent;
use crate::scene::{Point, Scene, Size};

/// 应用配置 —— 身份与启动尺寸。app_id 用反向域名。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppConfig {
    pub app_id: &'static str,
    pub title: &'static str,
    pub width: f32,
    pub height: f32,
    pub min_width: f32,
    pub min_height: f32,
}

impl AppConfig {
    pub const fn new(
        app_id: &'static str,
        title: &'static str,
        width: f32,
        height: f32,
    ) -> Self {
        Self {
            app_id,
            title,
            width,
            height,
            min_width: width,
            min_height: height,
        }
    }

    /// 声明最小逻辑尺寸。未调用时保持启动尺寸。
    pub const fn with_min_size(mut self, w: f32, h: f32) -> Self {
        self.min_width = w;
        self.min_height = h;
        self
    }
}

/// 文本度量。
///
/// **排版是渲染之前的事** —— 塑形 / 断行 / BiDi 都在 App 提交 [`Scene`] 命令前
/// 完成，渲染器只画已定位的字形。这与 Kanesumi `kanesumi-canvas::TextEngine`
/// 的分工一致（`TextEngine` 用 fontdue + rustybuzz + unicode-linebreak +
/// unicode-bidi，当前零平台门控）。
///
/// 现阶段这是一个**度量契约**，不是排版实现 —— 它先把接口钉住，让 App 侧的
/// 布局代码可以先行编写与测试；真正的塑形/断行在接入 `kanesumi-canvas` 时替换。
/// 用 trait 而非具体类型，正是为了让这次替换是无痛的。
pub trait TextEngine {
    /// 单行文本的逻辑像素宽度。
    fn measure_width(&self, text: &str, size: f32) -> f32;

    /// 单行行高。CJK 需要比西文更紧的行距（正典 typography 明确记录此点），
    /// 因此行高不能简单等于字号 —— 必须由引擎给出。
    fn line_height(&self, size: f32) -> f32;
}

/// 过渡期度量器 —— 按字号线性估算，不经字体。
///
/// **明确是占位实现**：等宽优先、CJK 视为全宽。它的存在只为让布局代码可编译
/// 可测试；数值不精确，不得用于最终排版决策。
#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicTextEngine;

impl HeuristicTextEngine {
    /// 拉丁字符的平均宽度系数 —— 经验值，非度量结果。
    const LATIN_RATIO: f32 = 0.55;
    /// CJK 字符相对字号的宽度 —— 全宽，接近精确。
    const CJK_RATIO: f32 = 1.0;
    /// 行高系数 —— 对齐正典 `body` 样式：15.0 / 22.0 ≈ 1.467。
    const LINE_RATIO: f32 = 1.42;
}

impl TextEngine for HeuristicTextEngine {
    fn measure_width(&self, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|c| {
                if is_wide(c) {
                    size * Self::CJK_RATIO
                } else {
                    size * Self::LATIN_RATIO
                }
            })
            .sum()
    }

    fn line_height(&self, size: f32) -> f32 {
        size * Self::LINE_RATIO
    }
}

/// 是否为全宽（CJK 及其标点、全角形式）码点。
///
/// 粗略区间判断，够占位实现使用；真值表请交给 `unicode-width` 或上游 TextEngine。
fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F      // 谚文字母
        | 0x2E80..=0x303E    // CJK 部首 / 康熙部首 / CJK 符号
        | 0x3041..=0x33FF    // 平假名 / 片假名 / CJK 兼容
        | 0x3400..=0x4DBF    // CJK 扩展 A
        | 0x4E00..=0x9FFF    // CJK 统一表意
        | 0xA000..=0xA4CF    // 彝文
        | 0xAC00..=0xD7A3    // 谚文音节
        | 0xF900..=0xFAFF    // CJK 兼容表意
        | 0xFE30..=0xFE6F    // CJK 兼容形式
        | 0xFF00..=0xFF60    // 全角形式
        | 0xFFE0..=0xFFE6    // 全角符号
        | 0x20000..=0x3FFFD  // CJK 扩展 B 及以上
    )
}

/// 应用 —— 纯逻辑，无平台句柄。
pub trait App {
    /// 身份与启动尺寸。
    fn config(&self) -> AppConfig;

    /// 字体文件路径。`None` 表示交由外壳按系统默认查找。
    ///
    /// 正典的不变量是「字体不得静默回退」—— 外壳在真正加载失败时必须硬失败，
    /// 而不是悄悄换成别的字体，否则中文字形会无声地变成豆腐块。
    fn font_path(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// IME 候选窗的锚点（逻辑像素）。
    ///
    /// **由 App 决定，而不是外壳** —— 外壳不知道文本框画在哪里。默认给客户区
    /// 左上角的合理内缩，这样即使 App 未实现，候选窗也不会跑到屏幕角上。
    fn ime_caret(&self) -> Point {
        Point::new(24.0, 64.0)
    }

    /// 由状态产出本帧的绘制命令。
    fn render(&mut self, text: &dyn TextEngine, size: Size, scene: &mut Scene);

    /// 处理一笔输入。
    fn handle_input(&mut self, event: InputEvent);

    /// 每帧推进动画。`dt` 单位为秒。
    ///
    /// 外壳必须传入**平滑后的** dt（非原始帧间隔）—— 弹簧的停滞快照阈值
    /// 依赖它，否则窗口拖拽或系统卡顿后会出现可见的「慢放尾巴」。
    fn update(&mut self, dt: f64);

    /// 是否需要重绘。默认 `false`（事件驱动）。
    ///
    /// App 在自己的 `update` / `handle_input` 里置脏，在 `render` 末尾清除。
    /// **这是 Arc Deck 的中心化重绘门控** —— 所有动画都汇总到这一处判断，
    /// 于是「新增了一个动画却忘了请求重绘」这类缺陷在结构上不可能发生。
    fn needs_redraw(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_keeps_start_size_as_minimum() {
        let c = AppConfig::new("io.github.arcdeck", "Arc Deck", 1200.0, 800.0);
        assert_eq!(c.min_width, 1200.0);
        assert_eq!(c.min_height, 800.0);
    }

    #[test]
    fn cjk_is_measured_full_width() {
        let e = HeuristicTextEngine;
        // 中文一字 = 一个字号宽；五个字 = 五倍。
        let w = e.measure_width("中文测试字", 16.0);
        assert!((w - 80.0).abs() < 1e-3, "expected 80.0, got {w}");
    }

    #[test]
    fn latin_is_narrower_than_cjk() {
        let e = HeuristicTextEngine;
        let latin = e.measure_width("abcde", 16.0);
        let cjk = e.measure_width("中文字符文", 16.0);
        assert!(latin < cjk, "latin={latin} should be < cjk={cjk}");
    }

    #[test]
    fn line_height_is_tighter_than_one_point_five() {
        // 正典对 CJK 的明确约束：行距必须比默认更紧（typography.rs 的断言）。
        let e = HeuristicTextEngine;
        assert!(e.line_height(15.0) / 15.0 < 1.5);
    }

    #[test]
    fn fullwidth_punctuation_counts_as_wide() {
        let e = HeuristicTextEngine;
        // 全角逗号也是全宽 —— 中文排版的常见坑。
        let w = e.measure_width("，", 20.0);
        assert!((w - 20.0).abs() < 1e-3, "expected 20.0, got {w}");
    }
}
