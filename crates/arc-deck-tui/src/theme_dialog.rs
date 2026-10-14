//! Theme picker dialog: searchable list, live preview, Enter confirms, Esc restores.
//! Copyright (c) 2025 opencode. MIT notice: ../LICENSE-OpenCode.
//!
//! Ported from OpenCode v2.0.27 `packages/tui/src/component/dialog-theme-list.tsx`, whose
//! behaviour is four lines of wiring around `DialogSelect`:
//!
//! - `initial = themes.selected` is captured on open; `onCleanup` re-applies it unless the
//!   dialog was confirmed. That is the whole of Esc's semantics — there is no separate undo
//!   stack, and a preview is a real theme change that Esc reverses.
//! - `onMove` calls `themes.set(...)`, so ↑/↓ preview live rather than waiting for Enter.
//! - `onSelect` sets `confirmed` and closes.
//! - `onFilter` restores `initial` when the query becomes empty, otherwise previews the first
//!   match. Typing therefore previews without any arrow key.
//!
//! Kept in its own file on purpose: the model dialog worker is editing `app.rs`/`view.rs` at
//! the same time, so all of this dialog's state and rendering lives here and the two shared
//! files only route to it.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Clear, Paragraph},
};

use crate::app::App;
use crate::theme::{Feedback, Mode, State, Theme, Variant, theme_names};

/// What the dialog wants the app to do after it has updated its own cursor/filter state.
/// Returned rather than performed in place: `preview` needs `&mut App` while the dialog itself
/// is borrowed out of `App`, so the two borrows have to be sequenced by the caller.
pub(crate) enum ThemeDialogAction {
    Preview(String),
    Commit(String),
}

pub(crate) struct ThemeDialog {
    filter: String,
    choice: usize,
    /// The theme in effect when the dialog opened. Esc must put it back.
    initial: String,
    confirmed: bool,
}

impl ThemeDialog {
    pub(crate) fn open(app: &App) -> Self {
        let names = ordered_names();
        let initial = app.preferences.theme_name().to_string();
        // `DialogSelect` focuses `current`, so the cursor starts on the theme already in use.
        let choice = names.iter().position(|name| *name == initial).unwrap_or(0);
        Self { filter: String::new(), choice, initial, confirmed: false }
    }

    /// Names that survive the filter, in registry order.
    ///
    /// Deliberate deviation from upstream: `DialogSelect` filters with `fuzzysort` (scored,
    /// reordered, `threshold`-limited), while this is a case-insensitive substring test that
    /// keeps registry order. Theme names are single lowercase words, so the visible difference
    /// is limited to non-contiguous queries (`"cc"` would fuzzy-match `catppuccin` upstream);
    /// the sibling model dialog already owns the repo's fuzzy scorer, and reaching into
    /// `catalog.rs` for it would collide with the worker editing that file.
    fn matches(&self) -> Vec<&'static str> {
        let query = self.filter.to_lowercase();
        if query.is_empty() {
            return ordered_names();
        }
        ordered_names()
            .into_iter()
            .filter(|name| name.to_lowercase().contains(&query))
            .collect()
    }

    pub(crate) fn selected(&self) -> Option<&'static str> {
        self.matches().get(self.choice).copied()
    }

    /// The preview target for the current cursor/filter, without touching `App`.
    ///
    /// A filter that matches *nothing* has no `filtered[0]`. Upstream leaves the previous
    /// preview in place in that case, which shows a theme the user can neither see nor select;
    /// restoring the opening theme says "nothing is selected" instead. This is a deliberate
    /// deviation, and the empty-query case is handled in `set_filter` where upstream handles it.
    fn preview_target(&self) -> ThemeDialogAction {
        match self.selected() {
            Some(name) => ThemeDialogAction::Preview(name.to_string()),
            None => ThemeDialogAction::Preview(self.initial.clone()),
        }
    }

    pub(crate) fn move_by(&mut self, delta: isize) -> ThemeDialogAction {
        let count = self.matches().len();
        if count == 0 {
            return self.preview_target();
        }
        self.choice = (self.choice as isize + delta).clamp(0, count as isize - 1) as usize;
        self.preview_target()
    }

    pub(crate) fn set_filter(&mut self, filter: String) -> ThemeDialogAction {
        self.filter = filter;
        // `onFilter`: an empty query puts the opening theme back — not the first entry of the
        // unfiltered list, which is what "reset the cursor to 0" would have previewed.
        // `DialogSelect` also re-focuses `current` when the query empties, so the cursor goes
        // back to the opening theme as well and ↑/↓ continue from where the dialog started.
        if self.filter.is_empty() {
            self.choice = self.matches().iter().position(|name| *name == self.initial).unwrap_or(0);
            return ThemeDialogAction::Preview(self.initial.clone());
        }
        // Otherwise `DialogSelect` resets the cursor whenever the filtered list changes, so the
        // first match is what gets previewed.
        self.choice = 0;
        self.preview_target()
    }

    pub(crate) fn confirm(&mut self) -> Option<ThemeDialogAction> {
        self.confirmed = true;
        self.selected().map(|name| ThemeDialogAction::Commit(name.to_string()))
    }

    /// Esc (or any non-confirming close). Upstream does this in `onCleanup`, guarded by
    /// `confirmed`, so restoring after Enter never happens.
    pub(crate) fn restore(&self) -> Option<ThemeDialogAction> {
        if self.confirmed {
            return None;
        }
        Some(ThemeDialogAction::Preview(self.initial.clone()))
    }

    /// Returns the action the caller must apply, or `None` when the key is not the dialog's.
    pub(crate) fn key(&mut self, code: crossterm::event::KeyCode, ctrl: bool) -> Option<ThemeDialogAction> {
        use crossterm::event::KeyCode;
        match code {
            KeyCode::Up => Some(self.move_by(-1)),
            KeyCode::Down => Some(self.move_by(1)),
            KeyCode::PageUp => Some(self.move_by(-8)),
            KeyCode::PageDown => Some(self.move_by(8)),
            KeyCode::Home => {
                self.choice = 0;
                Some(self.preview_target())
            }
            KeyCode::End => {
                self.choice = self.matches().len().saturating_sub(1);
                Some(self.preview_target())
            }
            KeyCode::Backspace => {
                let mut filter = self.filter.clone();
                filter.pop();
                Some(self.set_filter(filter))
            }
            KeyCode::Char(c) if !ctrl => {
                let mut filter = self.filter.clone();
                filter.push(c);
                Some(self.set_filter(filter))
            }
            _ => None,
        }
    }

    pub(crate) fn draw(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let names = self.matches();
        let mut lines: Vec<Line<'static>> = Vec::new();
        if names.is_empty() {
            lines.push(Line::styled(
                "没有匹配的主题",
                Style::default().fg(theme.text_feedback(Feedback::Warning, false).over(theme.background)),
            ));
        } else {
            let visible = area.height.saturating_sub(3) as usize;
            let start = self
                .choice
                .saturating_sub(visible.saturating_sub(1))
                .min(names.len().saturating_sub(visible.max(1)));
            for (index, name) in names.iter().enumerate().skip(start).take(visible.max(1)) {
                let selected = index == self.choice;
                // `•` marks the theme that was active before the dialog opened, so a preview stays
                // distinguishable from the saved choice; `›` marks the cursor.
                let marker = if *name == self.initial { "•" } else { " " };
                let label = format!("{} {name}", if selected { "›" } else { marker });
                let style = if selected {
                    // The same pair every other dialog highlights with (`view::row_focus`), so
                    // "the cursor is here" looks the same in both places.
                    let (fg, bg) = crate::view::row_focus(theme);
                    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
                } else if *name == self.initial {
                    Style::default().fg(theme.text_action(Variant::Primary, &[State::Selected]).over(theme.background))
                } else {
                    Style::default().fg(theme.text.to_ratatui())
                };
                lines.push(Line::styled(label, style));
            }
        }
        frame.render_widget(
            Paragraph::new(lines),
            Rect::new(area.x, area.y, area.width, area.height.saturating_sub(2)),
        );
    }

    pub(crate) fn filter(&self) -> &str {
        &self.filter
    }

    pub(crate) fn footer(&self, width: u16, theme: &Theme) -> Line<'static> {
        let mode = match theme.mode {
            Mode::Dark => "深色",
            Mode::Light => "浅色",
        };
        // The hints collapse to the one that matters when the frame is narrow: a truncated hint
        // reads as a rendering bug, and `view.rs` already varies its own footer by width.
        let hints = if width >= 48 { "↑↓ 预览 · Enter 确认 · Esc 还原 · /mode 深浅" } else { "↑↓ · Enter · Esc" };
        Line::styled(
            format!("{} · {mode} · {hints}", theme.name),
            Style::default().fg(theme.text_action(Variant::Secondary, &[]).over(theme.background)),
        )
    }
}

/// The dialog frame, sized like the other popups in `view.rs` so the two cannot drift.
pub(crate) fn popup_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2)).max(4);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

pub(crate) fn draw(frame: &mut Frame, dialog: &ThemeDialog, area: Rect, theme: &Theme) {
    let rect = popup_rect(area, 72, 14);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::bordered()
            .title("主题")
            .border_style(Style::default().fg(theme.border.to_ratatui()))
            .style(Style::default().bg(theme.dialog_background(theme.background_raised).over(theme.background))),
        rect,
    );
    let inner = Rect::new(
        rect.x + 2,
        rect.y + 1,
        rect.width.saturating_sub(4),
        rect.height.saturating_sub(2),
    );
    frame.render_widget(
        Paragraph::new(format!("搜索：{}", dialog.filter()))
            .style(Style::default().fg(theme.text_action(Variant::Secondary, &[]).over(theme.background))),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    dialog.draw(
        frame,
        Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(1)),
        theme,
    );
    frame.render_widget(
        Paragraph::new(dialog.footer(inner.width, theme)),
        Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
    );
}

fn ordered_names() -> Vec<&'static str> {
    let mut names = theme_names();
    names.sort_unstable();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Boot, app::App, theme::DEFAULT_THEME};

    fn app(theme: Option<&str>) -> App {
        App::new(
            Boot {
                workspace: ".".into(),
                model: None,
                effort: None,
                config_error: None,
                preferences: "unused".into(),
                catalog: Vec::new(),
                model_prefs: "unused".into(),
            },
            crate::Preferences {
                theme: theme.map(str::to_string),
                ..crate::Preferences::default()
            },
            crate::catalog::ModelPrefs::default(),
        )
    }

    fn preview(action: ThemeDialogAction) -> String {
        match action {
            ThemeDialogAction::Preview(name) => name,
            ThemeDialogAction::Commit(name) => panic!("expected a preview, got a commit of {name}"),
        }
    }

    /// `DialogSelect` focuses `current`, which this dialog passes as the theme already in use.
    #[test]
    fn opens_on_the_theme_in_use() {
        let app = app(Some("dracula"));
        let dialog = ThemeDialog::open(&app);
        assert_eq!(dialog.selected(), Some("dracula"));
        assert_eq!(preview(dialog.restore().unwrap()), "dracula");
    }

    /// `onMove` calls `themes.set(...)`, so the arrow keys preview live rather than waiting for
    /// Enter — the preview *is* the only feedback that the key did anything.
    #[test]
    fn moving_previews_live_and_clamps_at_both_ends() {
        let app = app(Some(DEFAULT_THEME));
        let mut dialog = ThemeDialog::open(&app);
        let names = ordered_names();
        let index = names.iter().position(|name| *name == DEFAULT_THEME).unwrap();

        assert_eq!(preview(dialog.move_by(1)), names[index + 1]);
        assert_eq!(preview(dialog.move_by(-1)), names[index]);

        let _ = dialog.move_by(-1000);
        assert_eq!(dialog.selected(), Some(names[0]), "↑ must stop at the top, not wrap");
        let _ = dialog.move_by(1000);
        assert_eq!(dialog.selected(), Some(*names.last().unwrap()));
    }

    /// `current` is what `DialogSelect` re-focuses when the query empties, and `onFilter`
    /// restores the opening theme at the same moment. Resetting the cursor to 0 instead would
    /// leave ↑/↓ restarting from the alphabetically first name.
    #[test]
    fn emptying_the_filter_returns_to_the_opening_theme() {
        let app = app(Some("nord"));
        let mut dialog = ThemeDialog::open(&app);
        let _ = dialog.set_filter("dracula".into());
        assert_eq!(dialog.selected(), Some("dracula"));
        assert_eq!(preview(dialog.set_filter(String::new())), "nord");
        assert_eq!(dialog.selected(), Some("nord"));
    }

    /// `onFilter` previews `filtered[0]`, so a filtered list is browsed from its first match
    /// even though the cursor was somewhere else a keystroke earlier.
    #[test]
    fn filtering_previews_the_first_match_case_insensitively() {
        let app = app(Some(DEFAULT_THEME));
        let mut dialog = ThemeDialog::open(&app);
        assert_eq!(preview(dialog.set_filter("CAT".into())), "catppuccin");
        assert_eq!(preview(dialog.set_filter("ca".into())), "carbonfox");
        assert_eq!(dialog.filter(), "ca");
    }

    /// A query with no match has no `filtered[0]`. Upstream leaves the last preview in place;
    /// restoring the opening theme is the deliberate deviation documented on `preview_target`.
    #[test]
    fn a_query_with_no_match_restores_the_opening_theme() {
        let app = app(Some("gruvbox"));
        let mut dialog = ThemeDialog::open(&app);
        let _ = dialog.set_filter("zzzz".into());
        assert_eq!(dialog.selected(), None);
        assert_eq!(preview(dialog.set_filter("zzzzz".into())), "gruvbox");
    }

    /// `onCleanup` restores `initial` unless `onSelect` ran, so a confirmed dialog must never
    /// be undone by the close that follows it.
    #[test]
    fn esc_restores_and_enter_does_not() {
        let app = app(Some("nord"));
        let mut escaped = ThemeDialog::open(&app);
        let _ = escaped.move_by(3);
        assert_eq!(preview(escaped.restore().unwrap()), "nord");

        let mut confirmed = ThemeDialog::open(&app);
        let action = confirmed.move_by(3);
        let target = preview(action);
        match confirmed.confirm() {
            Some(ThemeDialogAction::Commit(name)) => assert_eq!(name, target),
            _ => panic!("confirm must commit the theme under the cursor"),
        }
        assert!(confirmed.restore().is_none(), "Enter must not be undone by Esc");
    }

    /// The dialog only claims the keys it uses, so anything else falls through to `App`.
    #[test]
    fn only_its_own_keys_are_consumed() {
        use crossterm::event::KeyCode;
        let app = app(Some(DEFAULT_THEME));
        let mut dialog = ThemeDialog::open(&app);
        assert!(dialog.key(KeyCode::Enter, false).is_none());
        assert!(dialog.key(KeyCode::Char('c'), true).is_none(), "^C must stay with the app");
        assert!(dialog.key(KeyCode::Backspace, false).is_some());
        assert!(dialog.key(KeyCode::Char('a'), false).is_some());
        assert_eq!(dialog.filter(), "a");
    }

    /// The picker is the only dialog whose title is drawn from a module other than `view.rs`;
    /// a missing or empty registry would render an empty frame that still looks like a popup.
    #[test]
    fn the_list_is_never_empty_and_holds_the_default() {
        let names = ordered_names();
        assert!(names.contains(&DEFAULT_THEME));
        assert!(names.len() > 20, "the built-in registry shrank: {names:?}");
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "the cursor order must be the display order");
    }

    /// The footer names the theme and mode the picker is previewing, and shortens its hint list
    /// on a narrow frame instead of letting it truncate mid-phrase.
    #[test]
    fn the_mode_in_the_footer_comes_from_the_resolved_theme() {
        let theme = Theme::load(DEFAULT_THEME, Mode::Light).unwrap();
        let app = app(Some(DEFAULT_THEME));
        let dialog = ThemeDialog::open(&app);
        let line = dialog.footer(80, &theme).to_string();
        assert!(line.contains("浅色"), "footer should name the mode it was given: {line}");
        assert!(line.contains(DEFAULT_THEME), "footer should name the theme: {line}");
        let narrow = dialog.footer(20, &theme).to_string();
        assert!(narrow.len() < line.len(), "narrow frames must get the short hint");
    }
}

