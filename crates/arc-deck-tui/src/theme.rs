//! Semantic roles from PRODUCT_PLAN §7.1; palette adapted from OpenCode.
//! Copyright (c) 2025 opencode. MIT notice: ../LICENSE-OpenCode.
use ratatui::style::Color;

pub(crate) struct Theme {
    pub text: Color,
    pub text_muted: Color,
    pub background: Color,
    pub background_panel: Color,
    pub background_element: Color,
    pub action: Color,
    pub action_selected: Color,
    pub action_disabled: Color,
    pub formfield: Color,
    pub feedback_success: Color,
    pub feedback_error: Color,
    pub feedback_warning: Color,
    pub border: Color,
}
impl Theme {
    pub fn new(light: bool) -> Self {
        let c = |v: u32| Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        if light {
            Self {
                text: c(0x1a1a1a),
                text_muted: c(0x6b6b6b),
                background: c(0xffffff),
                background_panel: c(0xfafafa),
                background_element: c(0xebebeb),
                action: c(0x3b7dd8),
                action_selected: c(0x2968c3),
                action_disabled: c(0x8a8a8a),
                formfield: c(0xf5f5f5),
                feedback_success: c(0x3d9a57),
                feedback_error: c(0xd1383d),
                feedback_warning: c(0xb0851f),
                border: c(0xb8b8b8),
            }
        } else {
            Self {
                text: c(0xeeeeee),
                text_muted: c(0x808080),
                background: c(0x0a0a0a),
                background_panel: c(0x141414),
                background_element: c(0x282828),
                action: c(0xfab283),
                action_selected: c(0xffc09f),
                action_disabled: c(0x606060),
                formfield: c(0x1e1e1e),
                feedback_success: c(0x7fd88f),
                feedback_error: c(0xe06c75),
                feedback_warning: c(0xe5c07b),
                border: c(0x484848),
            }
        }
    }
}
