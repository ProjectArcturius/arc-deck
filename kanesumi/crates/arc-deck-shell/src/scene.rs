// Saiku Deck · 渲染契约：几何 + Scene 绘制命令
//
// 这一层是 Saiku Deck 的「圆心」—— 与平台无关、与 DSH 无关、与 Kanesumi 无关。
// 它刻意**镜像** Kanesumi 的 `kanesumi-canvas`：同样的八个 `SceneCommand` 变体、
// 同样的画家算法（扁平 `Vec`，无保留视觉树）。这样将来若要直接消费上游
// `kanesumi-canvas`，替换是逐变体一一对应的，不需要重写绘制路径。
//
// 见 `docs/ARCHITECTURE.md` 与 `docs/DESIGN_SYSTEM.md`。

/// 颜色 —— 线性 sRGB f32 通道，与 Kanesumi `Color` 同构。
///
/// 纯色无语义：本设计语言**禁止渐变**，且这一约定在命令模型里是结构性的
/// （见 [`SceneCommand`] —— 没有任何渐变/画笔变体）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    pub const TRANSPARENT: Self = Self::rgba(0.0, 0.0, 0.0, 0.0);

    /// 不透明 RGB。
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// 带 alpha 的 RGB。
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// 由 `0xRRGGBB` 字面量构造不透明色。
    ///
    /// 设计正典里的色值都以十六进制书写，这个构造器让 token 表能与正典逐字对齐。
    pub const fn from_hex(rgb: u32) -> Self {
        Self::rgb(
            ((rgb >> 16) & 0xFF) as f32 / 255.0,
            ((rgb >> 8) & 0xFF) as f32 / 255.0,
            (rgb & 0xFF) as f32 / 255.0,
        )
    }

    /// 替换 alpha 通道。
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// 通道线性插值。`t` 未 clamp —— 调用方负责语义。
    ///
    /// 主题交叉淡入淡出就靠它：整套调色板是一个 0→1 标量的纯函数。
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }
}

/// 矩形 —— 逻辑像素，原点在客户区左上。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(&self) -> f32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// 内缩四边。
    pub const fn inset(&self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.w - 2.0 * dx, self.h - 2.0 * dy)
    }

    pub const fn center(&self) -> Point {
        Point::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub const fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }
}

/// 点 —— 逻辑像素。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// 尺寸 —— 逻辑像素。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    pub const fn new(w: f32, h: f32) -> Self {
        Self { w, h }
    }
}

/// 文本水平锚点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// 一条绘制命令。
///
/// **八个变体，且刻意只有八个。** 没有渐变、没有阴影、没有圆角：
///
/// - **无渐变** —— 正典铁律 6 在此结构性成立（不存在画笔/渐变变体）
/// - **无阴影** —— 纵深靠明暗，不靠阴影（正典铁律 6）
/// - **无圆角** —— 直角是几何身份；若确需 2px，由调用方在 `FillRect` 之上自行叠加，
///   而不是给命令加一个 `radius` 字段让 99% 的调用点携带一个恒为 0 的参数
#[derive(Debug, Clone, PartialEq)]
pub enum SceneCommand {
    /// 实心矩形 —— 最常用的命令。
    FillRect { rect: Rect, color: Color },

    /// 空心描边矩形。线宽以像素计。
    ///
    /// 注意：Saiku Deck 的约定是**描边向内**（inside）—— 与 Kanesumi 一致，
    /// 也与「贴边」哲学一致：描边不侵占相邻元素的像素。
    StrokeRect { rect: Rect, color: Color, width: f32 },

    /// 一段文本。
    ///
    /// 排版（断行 / 塑形 / BiDi）在**提交命令之前**完成 —— 渲染器只负责把
    /// 已定位的字形画出来。这与 Kanesumi 的 `TextEngine` 分工一致。
    Text {
        pos: Point,
        text: String,
        size: f32,
        color: Color,
        align: TextAlign,
    },

    /// 圆弧 / 圆环扇形 —— 进度指示器用。
    Arc {
        center: Point,
        radius: f32,
        start_deg: f32,
        sweep_deg: f32,
        stroke_width: f32,
        color: Color,
    },

    /// 压入裁剪矩形。**必须与 [`SceneCommand::PopClip`] 成对**。
    PushClip { rect: Rect },

    /// 弹出最近一次裁剪。
    PopClip,

    /// 位图（RGBA8）。
    Image {
        rect: Rect,
        rgba: std::sync::Arc<[u8]>,
        width: u32,
        height: u32,
        /// 非白色则按 alpha 蒙版染色（图标着色）。
        tint: Color,
    },

    /// 三角形 —— 几何装饰。
    Triangle {
        a: Point,
        b: Point,
        c: Point,
        color: Color,
    },
}

/// 一帧的绘制命令序列。
///
/// 画家算法：按顺序绘制，后面的覆盖前面的。**没有保留视觉树** —— 每帧由 App
/// 从状态重新产出。这与 Kanesumi 的 `Scene` 同构。
#[derive(Debug, Default, Clone)]
pub struct Scene {
    pub commands: Vec<SceneCommand>,
}

impl Scene {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// 清空以便复用 —— 每帧调用，避免反复分配。
    pub fn clear(&mut self) {
        self.commands.clear();
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.commands.push(SceneCommand::FillRect { rect, color });
    }

    pub fn stroke_rect(&mut self, rect: Rect, color: Color, width: f32) {
        self.commands.push(SceneCommand::StrokeRect { rect, color, width });
    }

    pub fn text(&mut self, pos: Point, text: impl Into<String>, size: f32, color: Color) {
        self.commands.push(SceneCommand::Text {
            pos,
            text: text.into(),
            size,
            color,
            align: TextAlign::Left,
        });
    }

    pub fn text_aligned(
        &mut self,
        pos: Point,
        text: impl Into<String>,
        size: f32,
        color: Color,
        align: TextAlign,
    ) {
        self.commands.push(SceneCommand::Text {
            pos,
            text: text.into(),
            size,
            color,
            align,
        });
    }

    pub fn push_clip(&mut self, rect: Rect) {
        self.commands.push(SceneCommand::PushClip { rect });
    }

    pub fn pop_clip(&mut self) {
        self.commands.push(SceneCommand::PopClip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_matches_canon_literals() {
        // 正典里的色值都以十六进制书写，逐字对齐是 token 表可审计的前提。
        let c = Color::from_hex(0xE57812);
        assert!((c.r - 229.0 / 255.0).abs() < 1e-6);
        assert!((c.g - 120.0 / 255.0).abs() < 1e-6);
        assert!((c.b - 18.0 / 255.0).abs() < 1e-6);
        assert_eq!(c.a, 1.0);
    }

    #[test]
    fn lerp_endpoints_are_exact() {
        let a = Color::from_hex(0x1A1A1A);
        let b = Color::from_hex(0x000000);
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
    }

    #[test]
    fn rect_edges_and_containment() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(r.right(), 110.0);
        assert_eq!(r.bottom(), 70.0);
        assert!(r.contains(Point::new(10.0, 20.0)));
        // 右/下边界为开区间 —— 相邻矩形不重叠，贴边时不产生双重点击区。
        assert!(!r.contains(Point::new(110.0, 70.0)));
        assert_eq!(r.center(), Point::new(60.0, 45.0));
    }

    #[test]
    fn clip_commands_are_paired_by_construction() {
        let mut s = Scene::new();
        s.push_clip(Rect::new(0.0, 0.0, 10.0, 10.0));
        s.fill_rect(Rect::new(0.0, 0.0, 5.0, 5.0), Color::WHITE);
        s.pop_clip();
        assert_eq!(s.len(), 3);
        assert!(matches!(s.commands[0], SceneCommand::PushClip { .. }));
        assert!(matches!(s.commands[2], SceneCommand::PopClip));
    }
}
