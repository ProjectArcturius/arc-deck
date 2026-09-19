// Arc Deck · 输入模型
//
// 平台无关的输入事件。Win32 侧把 `WM_*` / `WM_IME_*` 翻译成这些变体，App 侧只
// 认识这一套 —— 于是 App 逻辑与「IME 走 IMM32 还是 TSF」完全解耦。
//
// IME 相关变体（[`InputEvent::Preedit`] / [`InputEvent::Commit`]）刻意与普通按键
// 分开：组合串**不进文本、不进撤销栈、不影响选区**，这是 IMM32 与 Wayland
// `zwp_text_input_v3` 共同的语义要求，也是 2026 年多数 Rust GUI 框架出错的点。

use crate::scene::Point;

/// 指针按钮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

/// 键盘修饰键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// 逻辑按键 —— 只列 Arc Deck 真正用到的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// 其余按键以平台扫描码透传，App 目前不关心。
    Other(u32),
}

/// 一笔输入事件。
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// 指针移动（物理像素已换算为逻辑像素）。
    PointerMoved(Point),

    /// 指针进入 / 离开窗口 —— 用于 hover 态与重绘门控。
    PointerEntered,
    PointerLeft,

    PointerPressed {
        at: Point,
        button: PointerButton,
    },
    PointerReleased {
        at: Point,
        button: PointerButton,
    },

    /// 滚轮。正 y 向下。
    Scroll {
        at: Point,
        dx: f32,
        dy: f32,
    },

    KeyPressed {
        key: Key,
        mods: Modifiers,
    },

    /// IME 组合串（预编辑文本）。
    ///
    /// 例如输入 `zhong` 时的下划线串。**不进入文档**，只用于显示。
    /// `cursor` 是组合串内的光标偏移（以字符计），`None` 表示无光标信息。
    Preedit {
        text: String,
        cursor: Option<usize>,
    },

    /// IME 提交串 —— 用户选字完成后的最终文本。
    ///
    /// **这是唯一会进入文档的 IME 事件。**
    Commit { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preedit_and_commit_are_distinct_events() {
        // 语义分立的回归保护：组合串绝不与提交串混为一种事件，
        // 否则「组合期间不得改动文档」这条不变量无处安放。
        let pre = InputEvent::Preedit {
            text: "zhong".into(),
            cursor: Some(5),
        };
        let com = InputEvent::Commit {
            text: "中".into(),
        };
        assert_ne!(pre, com);
        assert!(matches!(pre, InputEvent::Preedit { .. }));
        assert!(matches!(com, InputEvent::Commit { .. }));
    }

    #[test]
    fn modifiers_default_is_clean() {
        let m = Modifiers::default();
        assert!(!m.shift && !m.ctrl && !m.alt);
    }
}
