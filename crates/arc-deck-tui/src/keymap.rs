//! Key binding parsing and context-aware resolution.
//!
//! Shape is taken from OpenCode v1.19.0 `config/keybind.ts`: a binding is a string such as
//! `ctrl+x`, `shift+tab`, `<leader>n`, a comma-separated list of those, or `none` to disable.
//! Users may override any action by id in `keybinds.json` next to `tui-preferences.json`.
//!
//! Resolution is by focus, not by a single global table: Ctrl+F inside the model dialog toggles
//! a favourite, while the same physical key in the composer belongs to nobody (§7.4 says
//! conflicts resolve by context, never by globally overriding one of them).

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::commands::{self, Action, Context};

/// 2000 ms: OpenCode's `LeaderTimeoutDefault` (`config/index.tsx`). Copied rather than invented —
/// the point of the migration is that the leader feels like the reference.
pub(crate) const LEADER_TIMEOUT_MS: u64 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Binding {
    Key { mods: KeyModifiers, code: KeyCode },
    Leader(KeyCode),
}

impl Binding {
    fn matches(self, key: &KeyEvent) -> bool {
        let Binding::Key { mods, code } = self else {
            return false;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if ctrl != mods.contains(KeyModifiers::CONTROL) || alt != mods.contains(KeyModifiers::ALT) {
            return false;
        }
        match code {
            // Shift is already encoded in a printable character: `?` is Shift+/ on most layouts,
            // and requiring the modifier bit as well would make every shifted binding unfireable.
            KeyCode::Char(expected) => match key.code {
                KeyCode::Char(actual) => actual == expected || actual.eq_ignore_ascii_case(&expected),
                _ => false,
            },
            // Terminals disagree about Shift+Tab: crossterm reports `BackTab`, some report
            // `Tab` with the SHIFT bit. Both spellings of the same physical chord must work.
            KeyCode::Tab => {
                key.code == KeyCode::BackTab
                    || (key.code == KeyCode::Tab
                        && key.modifiers.contains(KeyModifiers::SHIFT))
            }
            _ => {
                code == key.code
                    && mods.contains(KeyModifiers::SHIFT)
                        == key.modifiers.contains(KeyModifiers::SHIFT)
            }
        }
    }

    fn is_leader(self) -> bool {
        matches!(self, Binding::Leader(_))
    }
}

/// Which bindings are allowed to fire right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Composer,
    ModelDialog,
    OtherDialog,
}

impl Focus {
    fn allows(self, context: Context) -> bool {
        match context {
            Context::Anywhere => true,
            Context::Composer => self == Focus::Composer,
            Context::ModelDialog => self == Focus::ModelDialog,
        }
    }
}

pub(crate) struct Keymap {
    bindings: Vec<(Action, Binding)>,
    /// The chord that opens a leader combination. Overridable with `{"leader": "…"}`.
    leader: Binding,
    warnings: Vec<String>,
}

impl Keymap {
    pub(crate) fn defaults() -> Self {
        let leader = parse_one("ctrl+x").expect("the default leader is a valid chord");
        let mut bindings = Vec::new();
        let mut warnings = Vec::new();
        for entry in commands::ENTRIES {
            for text in entry.keys {
                match parse_list(text) {
                    Ok(parsed) => bindings.extend(parsed.into_iter().map(|b| (entry.action, b))),
                    Err(error) => warnings.push(format!("{}：{error}", entry.id)),
                }
            }
        }
        Self {
            bindings,
            leader,
            warnings,
        }
    }

    /// Loads `keybinds.json` next to the preferences file. A missing file is normal; a broken
    /// one is a warning and the defaults stay — a typo must not lock the user out of the TUI.
    pub(crate) fn load(preferences: &Path) -> Self {
        let Some(path) = preferences.parent().map(|dir| dir.join("keybinds.json")) else {
            return Self::defaults();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::defaults();
        };
        Self::with_overrides(&text)
    }

    pub(crate) fn with_overrides(text: &str) -> Self {
        let mut keymap = Self::defaults();
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => {
                keymap
                    .warnings
                    .push(format!("keybinds.json 解析失败：{error}；已用默认键位"));
                return keymap;
            }
        };
        let Some(object) = value.as_object() else {
            keymap
                .warnings
                .push("keybinds.json 顶层必须是对象：{action id: 键位}；已用默认键位".into());
            return keymap;
        };
        for (id, value) in object {
            // `leader` is not an action; it is the chord every `<leader>x` binding depends on.
            if id == "leader" {
                match value.as_str().map(parse_one) {
                    Some(Ok(binding)) if !binding.is_leader() => keymap.leader = binding,
                    Some(Ok(_)) => keymap
                        .warnings
                        .push("keybinds.json：leader 不能是 <leader> 组合".into()),
                    Some(Err(error)) => {
                        keymap.warnings.push(format!("keybinds.json：leader {error}"))
                    }
                    None => keymap
                        .warnings
                        .push("keybinds.json：leader 必须是字符串".into()),
                }
                continue;
            }
            let Some(entry) = commands::ENTRIES.iter().find(|entry| entry.id == id) else {
                keymap
                    .warnings
                    .push(format!("keybinds.json：未知 action id `{id}`"));
                continue;
            };
            let Some(text) = value.as_str() else {
                keymap
                    .warnings
                    .push(format!("keybinds.json：`{id}` 的值必须是字符串"));
                continue;
            };
            match parse_list(text) {
                Ok(parsed) => {
                    keymap.bindings.retain(|(action, _)| *action != entry.action);
                    keymap
                        .bindings
                        .extend(parsed.into_iter().map(|binding| (entry.action, binding)));
                }
                Err(error) => keymap
                    .warnings
                    .push(format!("keybinds.json：`{id}` {error}；已保留默认值")),
            }
        }
        keymap
    }

    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// How the footer and which-key strip spell the leader chord. Follows `keybinds.json`: a
    /// rebound leader must not still be advertised as Ctrl+X.
    pub(crate) fn leader_label(&self) -> String {
        chord_label(&self.leader)
    }

    pub(crate) fn is_leader_key(&self, key: &KeyEvent) -> bool {
        self.leader.matches(key)
    }

    /// The action a key maps to under the current focus. Leader combinations are resolved by
    /// `leader_action` instead — this never returns one.
    pub(crate) fn resolve(&self, key: &KeyEvent, focus: Focus) -> Option<Action> {
        self.bindings
            .iter()
            .filter(|(action, _)| focus.allows(commands::entry(*action).context))
            .find(|(_, binding)| !binding.is_leader() && binding.matches(key))
            .map(|(action, _)| *action)
    }

    /// Resolves the key that follows an active leader.
    pub(crate) fn leader_action(&self, key: &KeyEvent) -> Option<Action> {
        self.bindings
            .iter()
            .filter(|(action, _)| Focus::Composer.allows(commands::entry(*action).context))
            .find_map(|(action, binding)| match binding {
                Binding::Leader(code)
                    if *code == key.code
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    Some(*action)
                }
                _ => None,
            })
    }

    /// Which-key rows: only leader combinations can follow a leader. Leader combinations are
    /// composer-only in this build — `App` refuses to start a leader while a dialog is open, so
    /// the strip never advertises a chord that cannot fire.
    pub(crate) fn leader_hints(&self) -> Vec<(String, Action)> {
        self.bindings
            .iter()
            .filter(|(action, _)| Focus::Composer.allows(commands::entry(*action).context))
            .filter_map(|(action, binding)| match binding {
                Binding::Leader(code) => Some((key_label(code), *action)),
                Binding::Key { .. } => None,
            })
            .collect()
    }

    /// What the UI prints for an action's bindings. `None` (or "none" in the file) means the
    /// action is reachable only via the palette or its slash command.
    pub(crate) fn display(&self, action: Action) -> Option<String> {
        let bindings: Vec<&Binding> = self
            .bindings
            .iter()
            .filter(|(bound, _)| *bound == action)
            .map(|(_, binding)| binding)
            .collect();
        if bindings.is_empty() {
            return None;
        }
        Some(
            bindings
                .into_iter()
                .map(|binding| match binding {
                    Binding::Leader(code) => format!(
                        "{} {}",
                        chord_label(&self.leader),
                        key_label(code)
                    ),
                    Binding::Key { .. } => chord_label(binding),
                })
                .collect::<Vec<_>>()
                .join(" / "),
        )
    }
}

fn chord_label(binding: &Binding) -> String {
    match binding {
        Binding::Leader(code) => format!("Leader {}", key_label(code)),
        Binding::Key { mods, code } => {
            let mut label = String::new();
            if mods.contains(KeyModifiers::CONTROL) {
                label.push_str("Ctrl+");
            }
            if mods.contains(KeyModifiers::ALT) {
                label.push_str("Alt+");
            }
            if mods.contains(KeyModifiers::SHIFT) && !matches!(code, KeyCode::Char(_)) {
                label.push_str("Shift+");
            }
            label.push_str(&key_label(code));
            label
        }
    }
}

fn key_label(code: &KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "Shift+Tab".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Insert => "Insert".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::PageUp => "PgUp".into(),
        KeyCode::PageDown => "PgDn".into(),
        KeyCode::Null => "Null".into(),
        KeyCode::CapsLock => "CapsLock".into(),
        KeyCode::ScrollLock => "ScrollLock".into(),
        KeyCode::NumLock => "NumLock".into(),
        KeyCode::PrintScreen => "PrintScreen".into(),
        KeyCode::Pause => "Pause".into(),
        KeyCode::Menu => "Menu".into(),
        KeyCode::KeypadBegin => "KeypadBegin".into(),
        KeyCode::Media(_) => "Media".into(),
        KeyCode::Modifier(_) => "Modifier".into(),
    }
}

/// `"none"` disables an action; anything else is a comma-separated chord list.
pub(crate) fn parse_list(text: &str) -> Result<Vec<Binding>, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("none") || text.is_empty() {
        return Ok(Vec::new());
    }
    text.split(',').map(|part| parse_one(part.trim())).collect()
}

pub(crate) fn parse_one(text: &str) -> Result<Binding, String> {
    let text = text.trim().to_ascii_lowercase();
    if text.is_empty() {
        return Err("空键位".into());
    }
    if let Some(rest) = text.strip_prefix("<leader>") {
        if rest.is_empty() {
            return Err("`<leader>` 后面缺少按键".into());
        }
        return Ok(Binding::Leader(parse_code(rest)?));
    }
    let mut parts: Vec<&str> = text.split('+').collect();
    let code = parts.pop().expect("split always yields one part");
    let mut mods = KeyModifiers::NONE;
    for part in parts {
        mods |= match part {
            "ctrl" | "control" => KeyModifiers::CONTROL,
            "shift" => KeyModifiers::SHIFT,
            "alt" | "meta" | "option" | "opt" => KeyModifiers::ALT,
            "super" | "cmd" | "command" | "win" => {
                return Err(format!("`{part}` 在本终端里无法可靠区分，请用 ctrl / alt / shift"));
            }
            other => return Err(format!("未知修饰键 `{other}`")),
        };
    }
    if code.is_empty() {
        return Err("缺少按键".into());
    }
    Ok(Binding::Key {
        mods,
        code: parse_code(code)?,
    })
}

fn parse_code(code: &str) -> Result<KeyCode, String> {
    let named = match code {
        "space" => Some(KeyCode::Char(' ')),
        "tab" => Some(KeyCode::Tab),
        "enter" | "return" => Some(KeyCode::Enter),
        "escape" | "esc" => Some(KeyCode::Esc),
        "backspace" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        "insert" | "ins" => Some(KeyCode::Insert),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "pageup" | "pgup" => Some(KeyCode::PageUp),
        "pagedown" | "pgdn" | "pgdown" => Some(KeyCode::PageDown),
        "backtab" => Some(KeyCode::BackTab),
        _ => None,
    };
    if let Some(named) = named {
        return Ok(named);
    }
    if let Some(number) = code.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=24).contains(&number)
    {
        return Ok(KeyCode::F(number));
    }
    let mut chars = code.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(KeyCode::Char(c));
    }
    Err(format!("无法识别的按键 `{code}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_with(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn parses_named_modified_and_leader_chords() {
        assert!(matches!(
            parse_one("ctrl+p"),
            Ok(Binding::Key { mods, code: KeyCode::Char('p') }) if mods == KeyModifiers::CONTROL
        ));
        assert!(matches!(
            parse_one("shift+tab"),
            Ok(Binding::Key { mods, code: KeyCode::Tab }) if mods == KeyModifiers::SHIFT
        ));
        assert!(matches!(
            parse_one("f2"),
            Ok(Binding::Key { code: KeyCode::F(2), .. })
        ));
        assert!(matches!(
            parse_one("<leader>n"),
            Ok(Binding::Leader(KeyCode::Char('n')))
        ));
        assert!(matches!(
            parse_one("escape"),
            Ok(Binding::Key { code: KeyCode::Esc, .. })
        ));
    }

    #[test]
    fn none_and_comma_lists() {
        assert_eq!(parse_list("none").unwrap(), Vec::new());
        assert_eq!(parse_list("").unwrap(), Vec::new());
        let list = parse_list("ctrl+c, ctrl+d , <leader>q").unwrap();
        assert_eq!(list.len(), 3);
        assert!(matches!(list[2], Binding::Leader(KeyCode::Char('q'))));
    }

    #[test]
    fn invalid_chords_are_reported_not_swallowed() {
        for text in ["ctrl+", "+x", "ctrl+nope", "hyper+a", "<leader>"] {
            assert!(parse_one(text).is_err(), "{text} should be rejected");
        }
        let error = parse_one("hyper+a").unwrap_err();
        assert!(error.contains("hyper"), "the message should name the offender: {error}");
    }

    /// A bad value must fall back to the default instead of disabling the action: the file is
    /// hand-edited, and a typo silently removing Ctrl+C would be worse than the typo itself.
    #[test]
    fn overrides_replace_and_bad_ones_warn_and_keep_the_default() {
        let keymap = Keymap::with_overrides(
            r#"{"theme.list":"ctrl+t","app.exit":"none","model.list":"hyper+m","nope":"f9"}"#,
        );
        assert_eq!(
            keymap.display(Action::Theme).as_deref(),
            Some("Ctrl+T"),
            "a valid override replaces the default"
        );
        assert_eq!(keymap.display(Action::Quit), None, "none disables every chord");
        assert_eq!(
            keymap.display(Action::Models).as_deref(),
            Some("Ctrl+X M"),
            "an invalid override keeps the default"
        );
        let warnings = keymap.warnings().join("\n");
        assert!(warnings.contains("nope"), "unknown ids are reported: {warnings}");
        assert!(warnings.contains("model.list"), "bad values name the action: {warnings}");
    }

    #[test]
    fn a_broken_file_falls_back_to_defaults_with_one_warning() {
        let keymap = Keymap::with_overrides("{not json");
        assert_eq!(keymap.display(Action::Palette).as_deref(), Some("Ctrl+P"));
        assert_eq!(keymap.warnings().len(), 1);
        assert!(keymap.warnings()[0].contains("解析失败"));
    }

    #[test]
    fn leader_can_be_rebound() {
        let keymap = Keymap::with_overrides(r#"{"leader":"ctrl+space"}"#);
        assert!(keymap.is_leader_key(&key_with(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert!(!keymap.is_leader_key(&key_with(KeyCode::Char('x'), KeyModifiers::CONTROL)));
        assert_eq!(keymap.display(Action::New).as_deref(), Some("Ctrl+Space N"));
    }

    /// §7.4: the same Ctrl+F must not fire the dialog action from the composer, and must not
    /// fire a session action from inside the dialog.
    #[test]
    fn ctrl_f_resolves_by_context() {
        let keymap = Keymap::defaults();
        let ctrl_f = key_with(KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(keymap.resolve(&ctrl_f, Focus::ModelDialog), Some(Action::Favorite));
        assert_eq!(keymap.resolve(&ctrl_f, Focus::Composer), None);
        assert_eq!(keymap.resolve(&ctrl_f, Focus::OtherDialog), None);
    }

    #[test]
    fn ctrl_p_works_everywhere_and_quit_works_in_dialogs() {
        let keymap = Keymap::defaults();
        let ctrl_p = key_with(KeyCode::Char('p'), KeyModifiers::CONTROL);
        for focus in [Focus::Composer, Focus::ModelDialog, Focus::OtherDialog] {
            assert_eq!(keymap.resolve(&ctrl_p, focus), Some(Action::Palette));
        }
        let ctrl_c = key_with(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(keymap.resolve(&ctrl_c, Focus::OtherDialog), Some(Action::Quit));
    }

    /// Shift+F2 must not be swallowed by plain F2: the two bindings circle in opposite
    /// directions, so a `mods` comparison that ignored SHIFT would make the reverse unfireable.
    #[test]
    fn f2_and_shift_f2_are_distinct() {
        let keymap = Keymap::defaults();
        assert_eq!(
            keymap.resolve(&key(KeyCode::F(2)), Focus::Composer),
            Some(Action::CycleModelNext)
        );
        assert_eq!(
            keymap.resolve(
                &key_with(KeyCode::F(2), KeyModifiers::SHIFT),
                Focus::Composer
            ),
            Some(Action::CycleModelReverse)
        );
    }

    /// crossterm reports Shift+Tab as BackTab; the binding is written `shift+tab`.
    #[test]
    fn backtab_matches_shift_tab() {
        let keymap = Keymap::defaults();
        assert_eq!(
            keymap.resolve(&key(KeyCode::BackTab), Focus::Composer),
            Some(Action::CycleAgent)
        );
        assert_eq!(
            keymap.resolve(
                &key_with(KeyCode::Tab, KeyModifiers::SHIFT),
                Focus::Composer
            ),
            Some(Action::CycleAgent)
        );
    }

    #[test]
    fn shifted_printable_characters_match_without_the_shift_bit() {
        let keymap = Keymap::defaults();
        // `?` is Shift+/ on a US layout; terminals commonly report only the resulting char.
        // It is a leader combination (`<leader>?`), never a bare composer key — a bare `?`
        // binding would steal a character users type into prompts.
        assert_eq!(
            keymap.leader_action(&key(KeyCode::Char('?'))),
            Some(Action::Help)
        );
        assert_eq!(keymap.resolve(&key(KeyCode::Char('?')), Focus::Composer), None);
    }

    #[test]
    fn leader_resolution_only_looks_at_leader_bindings() {
        let keymap = Keymap::defaults();
        assert_eq!(
            keymap.leader_action(&key(KeyCode::Char('n'))),
            Some(Action::New)
        );
        assert_eq!(
            keymap.leader_action(&key(KeyCode::Char('m'))),
            Some(Action::Models)
        );
        assert_eq!(keymap.leader_action(&key(KeyCode::Char('z'))), None);
        // Ctrl+P is not a leader combination even though the chord is Ctrl+….
        assert_eq!(
            keymap.leader_action(&key_with(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn which_key_rows_name_the_key_after_the_leader() {
        let keymap = Keymap::defaults();
        let hints = keymap.leader_hints();
        let keys: Vec<&str> = hints.iter().map(|(key, _)| key.as_str()).collect();
        for expected in ["N", "L", "A", "M", "B", "T", "Q", "?"] {
            assert!(keys.contains(&expected), "{expected} missing from {keys:?}");
        }
    }

    /// The defaults are constants, but a typo in one would disable that chord silently at
    /// startup; this is the check that keeps `defaults()` warning-free.
    #[test]
    fn the_default_keymap_has_no_warnings() {
        let keymap = Keymap::defaults();
        assert!(keymap.warnings().is_empty(), "{:?}", keymap.warnings());
    }

    #[test]
    fn every_registered_chord_is_shown_somewhere() {
        let keymap = Keymap::defaults();
        for entry in commands::ENTRIES {
            if entry.keys.is_empty() {
                continue;
            }
            let display = keymap
                .display(entry.action)
                .unwrap_or_else(|| panic!("{} has keys but no display", entry.id));
            assert!(!display.is_empty());
        }
    }
}
