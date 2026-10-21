// Saiku Deck · 骨架 demo
//
// 用途有两个，都很具体：
//
// 1. **证明通路**：`App` 产出 [`Scene`] → GDI 光栅化上屏，端到端跑通。
// 2. **验证 IME**：窗口中有一个可输入的文本行，用微软拼音打字能看到组合串
//    （带下划线）与提交结果 —— 这就是 `docs/ARCHITECTURE.md` 里那条
//    「组合串不进文档」的纪律在真实窗口里的可观测量。
//
// 它刻意用 Kanesumi Design 的真实 token 值，于是肉眼也能确认配色方向。

use arc_deck_shell::{
    App, AppConfig, Color, InputEvent, Key, Point, Rect, Scene, Size, TextAlign, TextEngine,
    platform,
};

// ── Kanesumi Design token（Rust/Ether 桌面调色板）─────────────────────
// 正典：Ether/docs/KANESUMI_DESIGN.md
// 注意：正典写「OLED 纯黑 #000000」，Android 扇区严格遵守，而 Ether 桌面用
// #1A1A1A。Saiku Deck 跟随桌面扇区 —— 这个分歧必须在项目里显式记录，不能默认。

/// 背景 —— Ether 桌面取值。
const BG: Color = Color::from_hex(0x1A1A1A);
/// 面板。
const SURFACE: Color = Color::from_hex(0x242424);
/// 分隔线。
const DIVIDER: Color = Color::from_hex(0x3A3A3A);
/// 单一强调色（正典：一屏一色）。
const PRIMARY: Color = Color::from_hex(0xE57812);
/// 主文本。
const ON_BG: Color = Color::from_hex(0xF0F0F0);
/// 次文本。
const ON_SURFACE_VARIANT: Color = Color::from_hex(0x9AA0A6);
/// 按下反馈（白 10%）。
const PRESS_TINT: Color = Color::rgba(1.0, 1.0, 1.0, 0.10);

/// 一处可点区域 —— 没有 `Sense`/`Response` 时的替代：App 自己登记、自己命中。
struct HitZone {
    rect: Rect,
    action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    ToggleAccent,
    ClearText,
}

struct DemoApp {
    /// 已提交文本（IME 结果 + 直接键入）。
    committed: String,
    /// 当前组合串（IME 预编辑）—— **不属于文档**。
    preedit: String,
    /// 光标本帧是否可见（闪烁）。
    caret_visible: bool,
    /// 强调色开关 —— 演示 token 与状态驱动渲染。
    accent_on: bool,
    zones: Vec<HitZone>,
    pressed: Option<Action>,
    /// 输入区在上一帧的位置 —— 供 IME 候选窗定位。
    field_rect: Rect,
}

impl DemoApp {
    fn new() -> Self {
        Self {
            committed: String::new(),
            preedit: String::new(),
            caret_visible: true,
            accent_on: true,
            zones: Vec::new(),
            pressed: None,
            field_rect: Rect::default(),
        }
    }

    fn accent(&self) -> Color {
        if self.accent_on {
            PRIMARY
        } else {
            ON_SURFACE_VARIANT
        }
    }

    /// 命中测试 —— 逆序遍历，后画的在上。
    fn hit(&self, p: Point) -> Option<Action> {
        self.zones
            .iter()
            .rev()
            .find(|z| z.rect.contains(p))
            .map(|z| z.action)
    }
}

impl App for DemoApp {
    fn config(&self) -> AppConfig {
        AppConfig::new("io.github.arcdeck.demo", "Saiku Deck · 骨架", 900.0, 420.0)
            .with_min_size(640.0, 320.0)
    }

    fn ime_caret(&self) -> Point {
        // 候选窗锚在输入行起点 —— App 才知道文本框画在哪，外壳不知道。
        Point::new(self.field_rect.x + 12.0, self.field_rect.bottom())
    }

    fn render(&mut self, text: &dyn TextEngine, size: Size, scene: &mut Scene) {
        self.zones.clear();

        // ── 背景通铺（正典：内容通铺，无边框）──────────────────────
        scene.fill_rect(Rect::new(0.0, 0.0, size.w, size.h), BG);

        let pad = 24.0;
        let mut y = pad;

        // ── 顶栏 = 内容第一项（正典：「顶栏 = 滚动列表第一项」）──────
        // 不是独立 chrome，没有分割线，没有背板。
        scene.text(
            Point::new(pad, y),
            "Saiku Deck",
            28.0,
            ON_BG,
        );
        scene.text_aligned(
            Point::new(size.w - pad, y + 8.0),
            "based on Kanesumi Design",
            13.0,
            ON_SURFACE_VARIANT,
            TextAlign::Right,
        );
        y += 46.0;

        // 3px 强调条 —— 设计语言的签名图案（正典：直角切一切）。
        scene.fill_rect(Rect::new(pad, y, 3.0, 16.0), self.accent());
        scene.text(Point::new(pad + 14.0, y - 1.0), "骨架验证", 14.0, ON_BG);
        y += 34.0;

        // ── 输入区（IME 验证点）────────────────────────────────────
        let field = Rect::new(pad, y, size.w - pad * 2.0, 44.0);
        self.field_rect = field;
        scene.fill_rect(field, SURFACE);

        // 组合串以强调色显示并带下划线 —— 视觉上明确区分「未提交」。
        // 这是正典未定义、由平台惯例补齐的规格（参 IME_WIRING_PLAN §B）。
        let mut tx = field.x + 12.0;
        let ty = field.y + 13.0;
        if self.committed.is_empty() && self.preedit.is_empty() {
            scene.text(
                Point::new(tx, ty),
                "点击这里，用微软拼音输入中文…",
                15.0,
                ON_SURFACE_VARIANT,
            );
        } else {
            if !self.committed.is_empty() {
                scene.text(Point::new(tx, ty), &self.committed, 15.0, ON_BG);
                tx += text.measure_width(&self.committed, 15.0);
            }
            if !self.preedit.is_empty() {
                let w = text.measure_width(&self.preedit, 15.0);
                scene.text(Point::new(tx, ty), &self.preedit, 15.0, self.accent());
                // 组合串下划线 —— 60% 透明，一段 1px 描边。
                scene.fill_rect(
                    Rect::new(tx, ty + 18.0, w, 1.0),
                    self.accent().with_alpha(0.6),
                );
                tx += w;
            }
        }

        // 光标。
        if self.caret_visible && self.preedit.is_empty() {
            scene.fill_rect(Rect::new(tx, field.y + 11.0, 1.5, 22.0), ON_BG);
        }

        y += 44.0 + 18.0;

        // ── 按钮行 ────────────────────────────────────────────────
        let btn_w = 132.0;
        let btn_h = 34.0;

        let b1 = Rect::new(pad, y, btn_w, btn_h);
        // 直角的按下反馈 —— 取代 M3 ripple（正典铁律 6）。
        // 注意：此处是硬切。正典说「直角闪切」，而时序（100ms 进 / 200ms 出）
        // 需要动画层；上游 Rust 侧目前也是硬切（见 DESIGN_SYSTEM.md 的落差记录）。
        scene.fill_rect(
            b1,
            if self.pressed == Some(Action::ToggleAccent) {
                self.accent().lerp(PRESS_TINT, 0.4)
            } else {
                self.accent()
            },
        );
        scene.text_aligned(
            b1.center(),
            if self.accent_on { "强调色：开" } else { "强调色：关" },
            14.0,
            BG,
            TextAlign::Center,
        );
        self.zones.push(HitZone {
            // 文字垂直居中：Center 对齐以点为基准，需上移半行高。
            rect: b1,
            action: Action::ToggleAccent,
        });

        let b2 = Rect::new(pad + btn_w + 12.0, y, btn_w, btn_h);
        scene.fill_rect(b2, SURFACE);
        scene.fill_rect(Rect::new(b2.x, b2.y, 2.0, b2.h), DIVIDER);
        scene.text_aligned(b2.center(), "清空", 14.0, ON_BG, TextAlign::Center);
        self.zones.push(HitZone {
            rect: b2,
            action: Action::ClearText,
        });

        y += btn_h + 22.0;

        // ── 状态行 ────────────────────────────────────────────────
        // 组合串原文一并显示：这是「IME 通路是否真的通」的可观测量 ——
        // 输入 zhong 时这里应出现 [zhong]，选字后它清空、上方文本增加。
        let state = format!(
            "已提交 {} 字 · 组合 [{}] · 命中区域 {}",
            self.committed.chars().count(),
            self.preedit,
            self.zones.len()
        );
        scene.text(Point::new(pad, y), state, 12.0, ON_SURFACE_VARIANT);

        // 直角网格线 —— 演示描边（向内，1px）。
        scene.stroke_rect(
            Rect::new(pad, size.h - pad - 46.0, size.w - pad * 2.0, 46.0),
            DIVIDER,
            1.0,
        );
        scene.text(
            Point::new(pad + 12.0, size.h - pad - 32.0),
            "Canvas 直角 · 无圆角 · 无渐变 · 无阴影",
            12.0,
            ON_SURFACE_VARIANT,
        );
    }

    fn handle_input(&mut self, event: InputEvent) {
        match event {
            InputEvent::Preedit { text, .. } => {
                // 组合串只影响显示，绝不进入文档。
                self.preedit = text;
                self.caret_visible = true;
            }
            InputEvent::Commit { text } => {
                // 唯一进入文档的路径。
                self.committed.push_str(&text);
                self.preedit.clear();
                self.caret_visible = true;
            }
            InputEvent::KeyPressed {
                key: Key::Backspace,
                ..
            } => {
                self.committed.pop();
            }
            InputEvent::PointerPressed { at, .. } => {
                self.pressed = self.hit(at);
            }
            InputEvent::PointerReleased { at, .. } => {
                if let Some(action) = self.hit(at) {
                    if self.pressed == Some(action) {
                        match action {
                            Action::ToggleAccent => self.accent_on = !self.accent_on,
                            Action::ClearText => {
                                self.committed.clear();
                                self.preedit.clear();
                            }
                        }
                    }
                }
                self.pressed = None;
            }
            _ => {}
        }
    }

    fn update(&mut self, _dt: f64) {
        // 光标闪烁属于动画，应由 sokuou 的 Progress 驱动（0.5s 方波）。
        // 骨架阶段保持常亮，避免在这里手搓定时器 —— 那正是 PezMax 违反
        // 自身「一切动画走 Sokuou」规则的地方（upload_progress_toast.rs）。
        self.caret_visible = true;
    }
}

fn main() {
    if let Err(e) = platform::run(Box::new(DemoApp::new())) {
        eprintln!("Saiku Deck 启动失败: {e}");
        std::process::exit(1);
    }
}
