//! Semantic theme tokens from OpenCode v2.0.27 (PRODUCT_PLAN §7.1 / §7.3).
//! Copyright (c) 2025 opencode. MIT notice: ../LICENSE-OpenCode.
//!
//! §7.1 deliberately mixes two upstream tags: structure comes from v1.19.0, but the *theme*
//! comes from 2.0.27, whose `text` / `background` / `action` / `formfield` / `feedback` /
//! `border` groups carry state roles (hovered / focused / pressed / selected / disabled).
//! v1.19.0's themes are flat palettes with no state roles, so the 2.0.27 shape is the one
//! expressed here and the v1 built-in palettes reach it through the 2.0.27 migration.
//!
//! Ported surfaces (`git show v2.0.27:<path>`):
//! - `packages/theme/src/tui/schema.ts` — token names and state roles
//! - `packages/theme/src/tui/expand.ts` — defaults for unspecified states
//! - `packages/theme/src/tui/select.ts` / `resolve.ts` — mode selection, `$` reference resolution
//! - `packages/theme/src/tui/v1-migrate.ts` — flat v1 palette → v2 tokens (`theme/v1.rs`)
//! - `packages/tui/src/theme/resolve.ts` — v1 colour resolution (defs, `{dark,light}` variants)
//! - `packages/tui/src/theme/assets/*.json` (v1) and `assets/v2/opencode.json` (v2 default)
//!
//! Deliberately not ported:
//! - the `system` theme (`packages/tui/src/theme/system.ts`), which reads the terminal palette.
//!   Nothing here queries the terminal, so theme names are built-in or nothing.
//! - `ResolvedThemeTokens::surface()` for surfaces other than `@dialog`: `SurfaceName` has a
//!   single member upstream, so a general surface stack would have no second caller.

use std::collections::HashMap;
use std::fmt;

use ratatui::style::Color;
use serde::Deserialize;

use crate::color::Rgb;

mod assets;
mod v1;

pub(crate) use assets::{DEFAULT_THEME, find, theme_names};

/// Resolved colour. `Transparent` is a distinct variant rather than black: the v2 default
/// theme uses `transparent` for `background.action.primary.base`, and painting that cell
/// black would break the "action chip inherits the surface behind it" look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ink {
    Rgb(Rgb),
    Transparent,
}

impl Ink {
    fn from_rgb(rgb: Rgb) -> Self {
        if rgb.a == 0 { Ink::Transparent } else { Ink::Rgb(rgb) }
    }

    pub fn to_ratatui(self) -> Color {
        match self {
            Ink::Rgb(rgb) => rgb.to_ratatui(),
            Ink::Transparent => Color::Reset,
        }
    }

    pub fn rgb(self) -> Option<Rgb> {
        match self {
            Ink::Rgb(rgb) => Some(rgb),
            Ink::Transparent => None,
        }
    }

    /// Terminals without alpha need a concrete cell colour; `Transparent` keeps the base.
    pub fn over(self, base: Ink) -> Color {
        match (self, base) {
            (Ink::Transparent, base) => base.to_ratatui(),
            (Ink::Rgb(rgb), Ink::Rgb(base)) if rgb.a != 255 => rgb.over(base),
            (Ink::Rgb(rgb), _) => rgb.to_ratatui(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum State {
    Base,
    Hovered,
    Focused,
    Pressed,
    Selected,
    Disabled,
}

/// Upstream `theme.text.action.primary.state({ hovered: true })` picks the first match in
/// `ActionState.literals` order: disabled → pressed → focused → selected → hovered. Reusing
/// that order keeps a caller that sets two flags on one element from picking a state OpenCode
/// would not have picked.
pub(crate) const STATE_ORDER: [State; 5] = [
    State::Disabled,
    State::Pressed,
    State::Focused,
    State::Selected,
    State::Hovered,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Variant {
    Primary,
    Secondary,
    Destructive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Feedback {
    Error,
    Warning,
    Success,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }
}

/// Dark is the default mode: §7.3 asks for OpenCode's default theme in dark.
impl Default for Mode {
    fn default() -> Self {
        Mode::Dark
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThemeError {
    /// Upstream `themeDecodeError`; the payload is the failing JSON path or colour literal.
    Invalid { what: String, value: String },
    MissingReference(String),
    CircularReference(String),
    MissingHue(String),
    MissingMode,
    UnknownTheme(String),
}

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThemeError::Invalid { what, value } => {
                write!(f, "Invalid theme: {what} {value} is an invalid value")
            }
            ThemeError::MissingReference(path) => write!(f, "Theme reference \"${path}\" was not found"),
            ThemeError::CircularReference(chain) => write!(f, "Circular theme reference: {chain}"),
            ThemeError::MissingHue(name) => write!(f, "Hue \"{name}\" was not found"),
            ThemeError::MissingMode => write!(f, "Theme must provide at least one mode"),
            ThemeError::UnknownTheme(name) => write!(f, "Unknown theme \"{name}\""),
        }
    }
}

impl std::error::Error for ThemeError {}

type Result<T> = std::result::Result<T, ThemeError>;

/// Upstream `HueStep`. An array index rather than a map because every step is required by the
/// schema and the ordering *is* the semantics `increase`/`decrease` rely on.
pub(crate) const HUE_STEPS: [u16; 9] = [100, 200, 300, 400, 500, 600, 700, 800, 900];

// ---------------------------------------------------------------------------------------
// Definition layer: raw JSON, still full of `$hue.*` / `$text.*` references.
// ---------------------------------------------------------------------------------------

/// One colour slot in a definition: a hex literal, `"transparent"`, or a `$path` reference.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub(crate) struct Ref(pub(crate) String);

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct StatefulDef {
    pub(crate) base: Option<Ref>,
    #[serde(rename = "$hovered")]
    pub(crate) hovered: Option<Ref>,
    #[serde(rename = "$focused")]
    pub(crate) focused: Option<Ref>,
    #[serde(rename = "$pressed")]
    pub(crate) pressed: Option<Ref>,
    #[serde(rename = "$selected")]
    pub(crate) selected: Option<Ref>,
    #[serde(rename = "$disabled")]
    pub(crate) disabled: Option<Ref>,
}

impl StatefulDef {
    fn entry(&self, state: State) -> Option<&Ref> {
        match state {
            State::Base => self.base.as_ref(),
            State::Hovered => self.hovered.as_ref(),
            State::Focused => self.focused.as_ref(),
            State::Pressed => self.pressed.as_ref(),
            State::Selected => self.selected.as_ref(),
            State::Disabled => self.disabled.as_ref(),
        }
    }

    fn map_refs(&self, replace: &impl Fn(&Ref) -> Ref) -> Self {
        Self {
            base: self.base.as_ref().map(replace),
            hovered: self.hovered.as_ref().map(replace),
            focused: self.focused.as_ref().map(replace),
            pressed: self.pressed.as_ref().map(replace),
            selected: self.selected.as_ref().map(replace),
            disabled: self.disabled.as_ref().map(replace),
        }
    }

    /// `expandFormfield`: every unset state falls back to this group's own `base`.
    fn expand(&mut self, base_path: &str) {
        if self.base.is_none() {
            return;
        }
        let fallback = Ref(format!("${base_path}.base"));
        for slot in [
            &mut self.hovered,
            &mut self.focused,
            &mut self.pressed,
            &mut self.selected,
            &mut self.disabled,
        ] {
            if slot.is_none() {
                *slot = Some(fallback.clone());
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct VariantsDef {
    pub(crate) primary: Option<StatefulDef>,
    pub(crate) secondary: Option<StatefulDef>,
    pub(crate) destructive: Option<StatefulDef>,
}

impl VariantsDef {
    fn get(&self, variant: Variant) -> Option<&StatefulDef> {
        match variant {
            Variant::Primary => self.primary.as_ref(),
            Variant::Secondary => self.secondary.as_ref(),
            Variant::Destructive => self.destructive.as_ref(),
        }
    }

    fn get_mut(&mut self, variant: Variant) -> &mut Option<StatefulDef> {
        match variant {
            Variant::Primary => &mut self.primary,
            Variant::Secondary => &mut self.secondary,
            Variant::Destructive => &mut self.destructive,
        }
    }

    fn map_refs(&self, replace: &impl Fn(&Ref) -> Ref) -> Self {
        Self {
            primary: self.primary.as_ref().map(|value| value.map_refs(replace)),
            secondary: self.secondary.as_ref().map(|value| value.map_refs(replace)),
            destructive: self.destructive.as_ref().map(|value| value.map_refs(replace)),
        }
    }

    /// `expandActions`: a variant that has a `base` gets every unset state pointed at it.
    fn expand(&mut self, base_path: &str) {
        for (variant, name) in [
            (Variant::Primary, "primary"),
            (Variant::Secondary, "secondary"),
            (Variant::Destructive, "destructive"),
        ] {
            if let Some(slot) = self.get_mut(variant) {
                slot.expand(&format!("{base_path}.{name}"));
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct FeedbackTextDef {
    pub(crate) base: Option<Ref>,
    pub(crate) muted: Option<Ref>,
}

impl FeedbackTextDef {
    fn map_refs(&self, replace: &impl Fn(&Ref) -> Ref) -> Self {
        Self {
            base: self.base.as_ref().map(replace),
            muted: self.muted.as_ref().map(replace),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct FeedbackBackgroundDef {
    pub(crate) base: Option<Ref>,
}

impl FeedbackBackgroundDef {
    fn map_refs(&self, replace: &impl Fn(&Ref) -> Ref) -> Self {
        Self { base: self.base.as_ref().map(replace) }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct TextFeedbackDef {
    pub(crate) error: Option<FeedbackTextDef>,
    pub(crate) warning: Option<FeedbackTextDef>,
    pub(crate) success: Option<FeedbackTextDef>,
    pub(crate) info: Option<FeedbackTextDef>,
}

const FEEDBACK_KINDS: [Feedback; 4] = [Feedback::Error, Feedback::Warning, Feedback::Success, Feedback::Info];

impl TextFeedbackDef {
    fn entry(&self, kind: Feedback) -> Option<&FeedbackTextDef> {
        match kind {
            Feedback::Error => self.error.as_ref(),
            Feedback::Warning => self.warning.as_ref(),
            Feedback::Success => self.success.as_ref(),
            Feedback::Info => self.info.as_ref(),
        }
    }

    fn entry_mut(&mut self, kind: Feedback) -> &mut Option<FeedbackTextDef> {
        match kind {
            Feedback::Error => &mut self.error,
            Feedback::Warning => &mut self.warning,
            Feedback::Success => &mut self.success,
            Feedback::Info => &mut self.info,
        }
    }

    fn map_refs(&self, replace: &impl Fn(&Ref) -> Ref) -> Self {
        Self {
            error: self.error.as_ref().map(|value| value.map_refs(replace)),
            warning: self.warning.as_ref().map(|value| value.map_refs(replace)),
            success: self.success.as_ref().map(|value| value.map_refs(replace)),
            info: self.info.as_ref().map(|value| value.map_refs(replace)),
        }
    }

    /// `expandText`: `muted` defaults to the kind's own `base`, not to `$text.muted`.
    fn expand(&mut self) {
        for kind in FEEDBACK_KINDS {
            let name = feedback_name(kind);
            if let Some(slot) = self.entry_mut(kind)
                && slot.muted.is_none()
                && slot.base.is_some()
            {
                slot.muted = Some(Ref(format!("$text.feedback.{name}.base")));
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct BackgroundFeedbackDef {
    pub(crate) error: Option<FeedbackBackgroundDef>,
    pub(crate) warning: Option<FeedbackBackgroundDef>,
    pub(crate) success: Option<FeedbackBackgroundDef>,
    pub(crate) info: Option<FeedbackBackgroundDef>,
}

impl BackgroundFeedbackDef {
    fn entry(&self, kind: Feedback) -> Option<&FeedbackBackgroundDef> {
        match kind {
            Feedback::Error => self.error.as_ref(),
            Feedback::Warning => self.warning.as_ref(),
            Feedback::Success => self.success.as_ref(),
            Feedback::Info => self.info.as_ref(),
        }
    }

    fn entry_mut(&mut self, kind: Feedback) -> &mut Option<FeedbackBackgroundDef> {
        match kind {
            Feedback::Error => &mut self.error,
            Feedback::Warning => &mut self.warning,
            Feedback::Success => &mut self.success,
            Feedback::Info => &mut self.info,
        }
    }

    fn map_refs(&self, replace: &impl Fn(&Ref) -> Ref) -> Self {
        Self {
            error: self.error.as_ref().map(|value| value.map_refs(replace)),
            warning: self.warning.as_ref().map(|value| value.map_refs(replace)),
            success: self.success.as_ref().map(|value| value.map_refs(replace)),
            info: self.info.as_ref().map(|value| value.map_refs(replace)),
        }
    }
}

fn feedback_name(kind: Feedback) -> &'static str {
    match kind {
        Feedback::Error => "error",
        Feedback::Warning => "warning",
        Feedback::Success => "success",
        Feedback::Info => "info",
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct TextDef {
    pub(crate) base: Option<Ref>,
    pub(crate) muted: Option<Ref>,
    pub(crate) action: Option<VariantsDef>,
    pub(crate) formfield: Option<StatefulDef>,
    pub(crate) feedback: Option<TextFeedbackDef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct RaisedDef {
    pub(crate) base: Option<Ref>,
    pub(crate) high: Option<Ref>,
    pub(crate) max: Option<Ref>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct BackgroundDef {
    pub(crate) base: Option<Ref>,
    pub(crate) raised: Option<RaisedDef>,
    pub(crate) action: Option<VariantsDef>,
    pub(crate) formfield: Option<StatefulDef>,
    pub(crate) feedback: Option<BackgroundFeedbackDef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct SingleDef {
    pub(crate) base: Option<Ref>,
}

impl SingleDef {
    fn map_refs(&self, replace: &impl Fn(&Ref) -> Ref) -> Self {
        Self { base: self.base.as_ref().map(replace) }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct DiffTextDef {
    pub(crate) added: Option<Ref>,
    pub(crate) removed: Option<Ref>,
    pub(crate) context: Option<Ref>,
    #[serde(rename = "hunkHeader")]
    pub(crate) hunk_header: Option<Ref>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct DiffBackgroundDef {
    pub(crate) added: Option<Ref>,
    pub(crate) removed: Option<Ref>,
    pub(crate) context: Option<Ref>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct DiffHighlightDef {
    pub(crate) added: Option<Ref>,
    pub(crate) removed: Option<Ref>,
}

impl DiffHighlightDef {
    fn map_refs(&self, replace: &impl Fn(&Ref) -> Ref) -> Self {
        Self {
            added: self.added.as_ref().map(replace),
            removed: self.removed.as_ref().map(replace),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct DiffLineNumberDef {
    pub(crate) text: Option<Ref>,
    pub(crate) background: Option<DiffHighlightDef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct DiffDef {
    pub(crate) text: Option<DiffTextDef>,
    pub(crate) background: Option<DiffBackgroundDef>,
    pub(crate) highlight: Option<DiffHighlightDef>,
    #[serde(rename = "lineNumber")]
    pub(crate) line_number: Option<DiffLineNumberDef>,
}

/// `SyntaxToken`: a flat map of name → colour. Field names are snake_case in Rust and
/// camelCase on the wire, matching the `markdown_*` / `syntax_*` naming the Markdown worker
/// asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct SyntaxDef {
    pub(crate) comment: Option<Ref>,
    pub(crate) keyword: Option<Ref>,
    pub(crate) function: Option<Ref>,
    pub(crate) variable: Option<Ref>,
    pub(crate) string: Option<Ref>,
    pub(crate) number: Option<Ref>,
    #[serde(rename = "type")]
    pub(crate) type_: Option<Ref>,
    pub(crate) operator: Option<Ref>,
    pub(crate) punctuation: Option<Ref>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct MarkdownDef {
    pub(crate) text: Option<Ref>,
    pub(crate) heading: Option<Ref>,
    pub(crate) link: Option<Ref>,
    #[serde(rename = "linkText")]
    pub(crate) link_text: Option<Ref>,
    pub(crate) code: Option<Ref>,
    #[serde(rename = "blockQuote")]
    pub(crate) block_quote: Option<Ref>,
    pub(crate) emphasis: Option<Ref>,
    pub(crate) strong: Option<Ref>,
    #[serde(rename = "horizontalRule")]
    pub(crate) horizontal_rule: Option<Ref>,
    #[serde(rename = "listItem")]
    pub(crate) list_item: Option<Ref>,
    #[serde(rename = "listEnumeration")]
    pub(crate) list_enumeration: Option<Ref>,
    pub(crate) image: Option<Ref>,
    #[serde(rename = "imageText")]
    pub(crate) image_text: Option<Ref>,
    #[serde(rename = "codeBlock")]
    pub(crate) code_block: Option<Ref>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct DialogDef {
    pub(crate) text: Option<TextDef>,
    pub(crate) background: Option<BackgroundDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(crate) enum HueValue {
    Scale(HashMap<String, Ref>),
    Alias(Ref),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct ModeDef {
    pub(crate) hue: Option<HashMap<String, HueValue>>,
    pub(crate) categorical: Option<Vec<String>>,
    pub(crate) text: Option<TextDef>,
    pub(crate) background: Option<BackgroundDef>,
    pub(crate) border: Option<SingleDef>,
    pub(crate) scrollbar: Option<SingleDef>,
    pub(crate) diff: Option<DiffDef>,
    pub(crate) syntax: Option<SyntaxDef>,
    pub(crate) markdown: Option<MarkdownDef>,
    #[serde(rename = "@dialog")]
    pub(crate) dialog: Option<DialogDef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct BaseDef {
    pub(crate) categorical: Vec<String>,
    pub(crate) text: TextDef,
    pub(crate) background: BackgroundDef,
    pub(crate) border: Option<SingleDef>,
    pub(crate) scrollbar: Option<SingleDef>,
    pub(crate) diff: Option<DiffDef>,
    pub(crate) syntax: Option<SyntaxDef>,
    pub(crate) markdown: Option<MarkdownDef>,
    #[serde(rename = "@dialog")]
    pub(crate) dialog: Option<DialogDef>,
}

/// Built either by `serde` (v2 documents) or by the v1 migration, which is why the fields are
/// not read from JSON alone. `migrated` marks the latter: those definitions are full of literal
/// hex and still need upstream's `referenceHues` pass before they can be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThemeDocument {
    WithLight {
        base: BaseDef,
        light: ModeDef,
        dark: Option<ModeDef>,
        migrated: bool,
    },
    WithDark {
        base: BaseDef,
        light: Option<ModeDef>,
        dark: ModeDef,
        migrated: bool,
    },
}

/// Wire shape of a v2 theme file. Kept separate from `ThemeDocument` because the latter also
/// carries the `migrated` flag, which is not a field in any file.
#[derive(Debug, Clone, Deserialize)]
struct RawDocument {
    base: BaseDef,
    #[serde(default)]
    light: Option<ModeDef>,
    #[serde(default)]
    dark: Option<ModeDef>,
}

impl RawDocument {
    fn into_document(self) -> Result<ThemeDocument> {
        let Self { base, light, dark } = self;
        match (light, dark) {
            (Some(light), dark) => Ok(ThemeDocument::WithLight { base, light, dark, migrated: false }),
            (None, Some(dark)) => Ok(ThemeDocument::WithDark { base, light: None, dark, migrated: false }),
            (None, None) => Err(ThemeError::MissingMode),
        }
    }
}

impl ThemeDocument {
    fn migrated(&self) -> bool {
        match self {
            ThemeDocument::WithLight { migrated, .. } | ThemeDocument::WithDark { migrated, .. } => *migrated,
        }
    }
}

/// A mode fully merged onto `base` (upstream `selectThemeMode` + `expandTheme`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResolvedDefinition {
    pub(crate) hue: HashMap<String, HueValue>,
    pub(crate) categorical: Vec<String>,
    pub(crate) text: TextDef,
    pub(crate) background: BackgroundDef,
    pub(crate) border: Option<SingleDef>,
    pub(crate) scrollbar: Option<SingleDef>,
    pub(crate) diff: Option<DiffDef>,
    pub(crate) syntax: Option<SyntaxDef>,
    pub(crate) markdown: Option<MarkdownDef>,
    pub(crate) dialog: Option<DialogDef>,
    /// Which mode the tokens came from. A theme that only defines `light` still resolves when
    /// `dark` is requested, and the UI has to be able to say which one it actually got.
    pub(crate) mode: Mode,
}

impl ResolvedDefinition {
    /// Structural walk used by `$` references. `text.action.primary.$focused` and
    /// `hue.neutral.200` are the only two shapes the upstream resolver reads, and `hue.*` is
    /// short-circuited by the resolver's scale index before it ever gets here.
    fn lookup(&self, path: &str) -> Option<String> {
        let mut segments = path.split('.');
        let head = segments.next()?;
        let rest: Vec<&str> = segments.collect();
        match head {
            "text" => lookup_text(&self.text, &rest),
            "background" => lookup_background(&self.background, &rest),
            "border" => Some(self.border.as_ref()?.base.as_ref()?.0.clone()),
            "scrollbar" => Some(self.scrollbar.as_ref()?.base.as_ref()?.0.clone()),
            "diff" => lookup_diff(self.diff.as_ref()?, &rest),
            "syntax" => lookup_syntax(self.syntax.as_ref()?, &rest),
            "markdown" => lookup_markdown(self.markdown.as_ref()?, &rest),
            _ => None,
        }
    }

    /// The `referenceHues` rewrite, applied to every group except `hue` and `categorical` —
    /// upstream's replace walk skips exactly those two keys (`key === "hue" || key ===
    /// "categorical" ? value : replace(value)`). Rewriting the hue scales too would let a step
    /// whose hex coincides with a *different* hue's step be replaced by a `$hue.*` reference,
    /// which for a self-referential hit would make the scale point at itself.
    fn map_refs(&self, replace: &impl Fn(&Ref) -> Ref) -> Self {
        Self {
            mode: self.mode,
            hue: self.hue.clone(),
            categorical: self.categorical.clone(),
            text: TextDef {
                base: self.text.base.as_ref().map(replace),
                muted: self.text.muted.as_ref().map(replace),
                action: self.text.action.as_ref().map(|value| value.map_refs(replace)),
                formfield: self.text.formfield.as_ref().map(|value| value.map_refs(replace)),
                feedback: self.text.feedback.as_ref().map(|value| value.map_refs(replace)),
            },
            background: BackgroundDef {
                base: self.background.base.as_ref().map(replace),
                raised: self.background.raised.as_ref().map(|raised| RaisedDef {
                    base: raised.base.as_ref().map(replace),
                    high: raised.high.as_ref().map(replace),
                    max: raised.max.as_ref().map(replace),
                }),
                action: self.background.action.as_ref().map(|value| value.map_refs(replace)),
                formfield: self.background.formfield.as_ref().map(|value| value.map_refs(replace)),
                feedback: self.background.feedback.as_ref().map(|value| value.map_refs(replace)),
            },
            border: self.border.as_ref().map(|value| value.map_refs(replace)),
            scrollbar: self.scrollbar.as_ref().map(|value| value.map_refs(replace)),
            diff: self.diff.as_ref().map(|diff| DiffDef {
                text: diff.text.as_ref().map(|text| DiffTextDef {
                    added: text.added.as_ref().map(replace),
                    removed: text.removed.as_ref().map(replace),
                    context: text.context.as_ref().map(replace),
                    hunk_header: text.hunk_header.as_ref().map(replace),
                }),
                background: diff.background.as_ref().map(|background| DiffBackgroundDef {
                    added: background.added.as_ref().map(replace),
                    removed: background.removed.as_ref().map(replace),
                    context: background.context.as_ref().map(replace),
                }),
                highlight: diff.highlight.as_ref().map(|value| value.map_refs(replace)),
                line_number: diff.line_number.as_ref().map(|line_number| DiffLineNumberDef {
                    text: line_number.text.as_ref().map(replace),
                    background: line_number.background.as_ref().map(|value| value.map_refs(replace)),
                }),
            }),
            syntax: self.syntax.as_ref().map(|syntax| SyntaxDef {
                comment: syntax.comment.as_ref().map(replace),
                keyword: syntax.keyword.as_ref().map(replace),
                function: syntax.function.as_ref().map(replace),
                variable: syntax.variable.as_ref().map(replace),
                string: syntax.string.as_ref().map(replace),
                number: syntax.number.as_ref().map(replace),
                type_: syntax.type_.as_ref().map(replace),
                operator: syntax.operator.as_ref().map(replace),
                punctuation: syntax.punctuation.as_ref().map(replace),
            }),
            markdown: self.markdown.as_ref().map(|markdown| MarkdownDef {
                text: markdown.text.as_ref().map(replace),
                heading: markdown.heading.as_ref().map(replace),
                link: markdown.link.as_ref().map(replace),
                link_text: markdown.link_text.as_ref().map(replace),
                code: markdown.code.as_ref().map(replace),
                block_quote: markdown.block_quote.as_ref().map(replace),
                emphasis: markdown.emphasis.as_ref().map(replace),
                strong: markdown.strong.as_ref().map(replace),
                horizontal_rule: markdown.horizontal_rule.as_ref().map(replace),
                list_item: markdown.list_item.as_ref().map(replace),
                list_enumeration: markdown.list_enumeration.as_ref().map(replace),
                image: markdown.image.as_ref().map(replace),
                image_text: markdown.image_text.as_ref().map(replace),
                code_block: markdown.code_block.as_ref().map(replace),
            }),
            dialog: self.dialog.as_ref().map(|dialog| DialogDef {
                text: dialog.text.as_ref().map(|text| TextDef {
                    base: text.base.as_ref().map(replace),
                    muted: text.muted.as_ref().map(replace),
                    action: text.action.as_ref().map(|value| value.map_refs(replace)),
                    formfield: text.formfield.as_ref().map(|value| value.map_refs(replace)),
                    feedback: text.feedback.as_ref().map(|value| value.map_refs(replace)),
                }),
                background: dialog.background.as_ref().map(|background| BackgroundDef {
                    base: background.base.as_ref().map(replace),
                    raised: None,
                    action: background.action.as_ref().map(|value| value.map_refs(replace)),
                    formfield: background.formfield.as_ref().map(|value| value.map_refs(replace)),
                    feedback: background.feedback.as_ref().map(|value| value.map_refs(replace)),
                }),
            }),
        }
    }
}

fn variant_field<'a>(variants: &'a Option<VariantsDef>, name: &str) -> Option<&'a StatefulDef> {
    match name {
        "primary" => variants.as_ref()?.primary.as_ref(),
        "secondary" => variants.as_ref()?.secondary.as_ref(),
        "destructive" => variants.as_ref()?.destructive.as_ref(),
        _ => None,
    }
}

fn stateful_field(stateful: &StatefulDef, name: &str) -> Option<String> {
    let value = match name {
        "base" => stateful.base.as_ref(),
        "$hovered" => stateful.hovered.as_ref(),
        "$focused" => stateful.focused.as_ref(),
        "$pressed" => stateful.pressed.as_ref(),
        "$selected" => stateful.selected.as_ref(),
        "$disabled" => stateful.disabled.as_ref(),
        _ => None,
    }?;
    Some(value.0.clone())
}

fn lookup_text(text: &TextDef, rest: &[&str]) -> Option<String> {
    match rest {
        ["base"] => Some(text.base.as_ref()?.0.clone()),
        ["muted"] => Some(text.muted.as_ref()?.0.clone()),
        ["action", variant, state] => stateful_field(variant_field(&text.action, variant)?, state),
        ["formfield", state] => stateful_field(text.formfield.as_ref()?, state),
        ["feedback", kind, state] => {
            let entry = text.feedback.as_ref()?.entry(feedback_kind(kind)?)?;
            match *state {
                "base" => Some(entry.base.as_ref()?.0.clone()),
                "muted" => Some(entry.muted.as_ref()?.0.clone()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn lookup_background(background: &BackgroundDef, rest: &[&str]) -> Option<String> {
    match rest {
        ["base"] => Some(background.base.as_ref()?.0.clone()),
        ["raised", step] => {
            let raised = background.raised.as_ref()?;
            let value = match *step {
                "base" => raised.base.as_ref(),
                "high" => raised.high.as_ref(),
                "max" => raised.max.as_ref(),
                _ => None,
            }?;
            Some(value.0.clone())
        }
        ["action", variant, state] => stateful_field(variant_field(&background.action, variant)?, state),
        ["formfield", state] => stateful_field(background.formfield.as_ref()?, state),
        ["feedback", kind, "base"] => {
            let entry = background.feedback.as_ref()?.entry(feedback_kind(kind)?)?;
            Some(entry.base.as_ref()?.0.clone())
        }
        _ => None,
    }
}

fn feedback_kind(name: &str) -> Option<Feedback> {
    match name {
        "error" => Some(Feedback::Error),
        "warning" => Some(Feedback::Warning),
        "success" => Some(Feedback::Success),
        "info" => Some(Feedback::Info),
        _ => None,
    }
}

fn lookup_diff(diff: &DiffDef, rest: &[&str]) -> Option<String> {
    let value = match rest {
        ["text", key] => match *key {
            "added" => diff.text.as_ref()?.added.as_ref(),
            "removed" => diff.text.as_ref()?.removed.as_ref(),
            "context" => diff.text.as_ref()?.context.as_ref(),
            "hunkHeader" => diff.text.as_ref()?.hunk_header.as_ref(),
            _ => None,
        },
        ["background", key] => match *key {
            "added" => diff.background.as_ref()?.added.as_ref(),
            "removed" => diff.background.as_ref()?.removed.as_ref(),
            "context" => diff.background.as_ref()?.context.as_ref(),
            _ => None,
        },
        ["highlight", key] => match *key {
            "added" => diff.highlight.as_ref()?.added.as_ref(),
            "removed" => diff.highlight.as_ref()?.removed.as_ref(),
            _ => None,
        },
        ["lineNumber", "text"] => diff.line_number.as_ref()?.text.as_ref(),
        ["lineNumber", "background", key] => match *key {
            "added" => diff.line_number.as_ref()?.background.as_ref()?.added.as_ref(),
            "removed" => diff.line_number.as_ref()?.background.as_ref()?.removed.as_ref(),
            _ => None,
        },
        _ => None,
    }?;
    Some(value.0.clone())
}

fn lookup_syntax(syntax: &SyntaxDef, rest: &[&str]) -> Option<String> {
    let value = match rest {
        ["comment"] => syntax.comment.as_ref(),
        ["keyword"] => syntax.keyword.as_ref(),
        ["function"] => syntax.function.as_ref(),
        ["variable"] => syntax.variable.as_ref(),
        ["string"] => syntax.string.as_ref(),
        ["number"] => syntax.number.as_ref(),
        ["type"] => syntax.type_.as_ref(),
        ["operator"] => syntax.operator.as_ref(),
        ["punctuation"] => syntax.punctuation.as_ref(),
        _ => None,
    }?;
    Some(value.0.clone())
}

fn lookup_markdown(markdown: &MarkdownDef, rest: &[&str]) -> Option<String> {
    let value = match rest {
        ["text"] => markdown.text.as_ref(),
        ["heading"] => markdown.heading.as_ref(),
        ["link"] => markdown.link.as_ref(),
        ["linkText"] => markdown.link_text.as_ref(),
        ["code"] => markdown.code.as_ref(),
        ["blockQuote"] => markdown.block_quote.as_ref(),
        ["emphasis"] => markdown.emphasis.as_ref(),
        ["strong"] => markdown.strong.as_ref(),
        ["horizontalRule"] => markdown.horizontal_rule.as_ref(),
        ["listItem"] => markdown.list_item.as_ref(),
        ["listEnumeration"] => markdown.list_enumeration.as_ref(),
        ["image"] => markdown.image.as_ref(),
        ["imageText"] => markdown.image_text.as_ref(),
        ["codeBlock"] => markdown.code_block.as_ref(),
        _ => None,
    }?;
    Some(value.0.clone())
}

// ---------------------------------------------------------------------------------------
// Resolution layer
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stateful {
    base: Ink,
    states: HashMap<State, Ink>,
}

impl Stateful {
    fn get(&self, state: State) -> Ink {
        if state == State::Base {
            return self.base;
        }
        self.states.get(&state).copied().unwrap_or(self.base)
    }

    /// First flag that is set wins, in upstream `ActionState.literals` order.
    fn state_of(&self, active: &[State]) -> Ink {
        STATE_ORDER
            .iter()
            .find(|state| active.contains(state))
            .map(|state| self.get(*state))
            .unwrap_or(self.base)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FeedbackPair {
    pub base: Ink,
    pub muted: Ink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiffTokens {
    pub text_added: Ink,
    pub text_removed: Ink,
    pub text_context: Ink,
    pub text_hunk_header: Ink,
    pub background_added: Ink,
    pub background_removed: Ink,
    pub background_context: Ink,
    pub highlight_added: Ink,
    pub highlight_removed: Ink,
    pub line_number_text: Ink,
    pub line_number_added: Ink,
    pub line_number_removed: Ink,
}

/// Fully resolved theme: every colour the UI can ask for is concrete, and the hue scales are
/// kept so `hue_source` / `shift_hue` can still answer "which step of which hue is this" — the
/// mechanism OpenCode uses to nudge hover and pressed shades by one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Theme {
    pub name: String,
    pub mode: Mode,
    hue_scales: HashMap<String, Vec<Rgb>>,
    pub categorical: Vec<Vec<Rgb>>,
    pub text: Ink,
    pub text_muted: Ink,
    text_action: HashMap<Variant, Stateful>,
    text_formfield: Stateful,
    pub text_feedback: HashMap<Feedback, FeedbackPair>,
    pub background: Ink,
    pub background_raised: Ink,
    pub background_raised_high: Ink,
    pub background_raised_max: Ink,
    background_action: HashMap<Variant, Stateful>,
    background_formfield: Stateful,
    pub background_feedback: HashMap<Feedback, Ink>,
    pub border: Ink,
    pub scrollbar: Ink,
    pub diff: DiffTokens,
    /// Syntax and markdown stay as resolved lookups rather than fields: there are 23 of them,
    /// they are all read by name from the renderer, and a name that is absent has to fall back
    /// to `text` instead of failing to build a theme.
    syntax_tokens: HashMap<&'static str, Ink>,
    markdown_tokens: HashMap<&'static str, Ink>,
    dialog_background: Option<Ink>,
}

impl Theme {
    /// Loads a built-in theme. v1 palettes go through the 2.0.27 migration first.
    pub fn load(name: &str, mode: Mode) -> Result<Self> {
        let info = find(name).ok_or_else(|| ThemeError::UnknownTheme(name.to_string()))?;
        let document = if info.v1 {
            v1::migrate(info.json)?
        } else {
            let raw: RawDocument = serde_json::from_str(info.json).map_err(|error| ThemeError::Invalid {
                what: "theme".into(),
                value: error.to_string(),
            })?;
            raw.into_document()?
        };
        Self::from_document(name, &document, mode)
    }

    pub fn from_document(name: &str, document: &ThemeDocument, mode: Mode) -> Result<Self> {
        let mut definition = select(document, mode)?;
        // The v1 migration writes literal hex for `border`, `diff.*`, `syntax.*` and friends.
        // Upstream's `referenceHues` rewrites the ones that already equal a scale step into
        // `$hue.*` so later `increase`/`decrease` can shift them; without it a migrated theme
        // renders identically but stops responding to hue steps. Only v1 needs it — a v2
        // document was written with references from the start.
        if document.migrated() {
            let references = hue_references(&Resolver::new(&definition)?.hue_scales);
            definition = definition.map_refs(&|value: &Ref| {
                references
                    .get(&value.0.to_lowercase())
                    .map(|found| Ref(found.clone()))
                    .unwrap_or_else(|| value.clone())
            });
        }
        Self::from_definition(name, &definition)
    }

    fn from_definition(name: &str, definition: &ResolvedDefinition) -> Result<Self> {
        let mut resolver = Resolver::new(definition)?;
        let text = definition.text.clone();
        let background = definition.background.clone();

        let text_action = resolve_variants(resolver.expand_variants(text.action.clone(), "text.action"), &mut resolver)?;
        let text_formfield =
            resolve_stateful(resolver.expand_stateful(text.formfield.clone(), "text.formfield"), &mut resolver)?;
        let mut text_feedback = HashMap::new();
        for kind in FEEDBACK_KINDS {
            let entry = text.feedback.as_ref().and_then(|feedback| feedback.entry(kind));
            let base = resolver.resolve_required(
                entry.and_then(|entry| entry.base.as_ref()),
                &format!("text.feedback.{}.base", feedback_name(kind)),
            )?;
            let muted = match entry.and_then(|entry| entry.muted.as_ref()) {
                Some(value) => resolver.resolve(value)?,
                None => base,
            };
            text_feedback.insert(kind, FeedbackPair { base, muted });
        }

        let background_action = resolve_variants(
            resolver.expand_variants(background.action.clone(), "background.action"),
            &mut resolver,
        )?;
        let background_formfield =
            resolve_stateful(resolver.expand_stateful(background.formfield.clone(), "background.formfield"), &mut resolver)?;
        let mut background_feedback = HashMap::new();
        for kind in FEEDBACK_KINDS {
            let value = background
                .feedback
                .as_ref()
                .and_then(|feedback| feedback.entry(kind))
                .and_then(|entry| entry.base.as_ref());
            let ink = resolver.resolve_required(value, &format!("background.feedback.{}.base", feedback_name(kind)))?;
            background_feedback.insert(kind, ink);
        }

        let raised = background.raised.as_ref();
        let mut syntax_tokens = HashMap::new();
        for (token, value) in syntax_entries(definition.syntax.as_ref()) {
            if let Some(value) = value {
                syntax_tokens.insert(token, resolver.resolve(value)?);
            }
        }
        let mut markdown_tokens = HashMap::new();
        for (token, value) in markdown_entries(definition.markdown.as_ref()) {
            if let Some(value) = value {
                markdown_tokens.insert(token, resolver.resolve(value)?);
            }
        }
        let dialog_background = match definition.dialog.as_ref().and_then(|dialog| dialog.background.as_ref()) {
            Some(background) => match background.base.as_ref() {
                Some(value) => Some(resolver.resolve(value)?),
                None => None,
            },
            None => None,
        };

        Ok(Self {
            name: name.to_string(),
            mode: definition.mode,
            hue_scales: resolver.hue_scales.clone(),
            categorical: resolver.categorical.clone(),
            text: resolver.resolve_required(text.base.as_ref(), "text.base")?,
            text_muted: resolver.resolve_required(text.muted.as_ref(), "text.muted")?,
            text_action,
            text_formfield,
            text_feedback,
            background: resolver.resolve_required(background.base.as_ref(), "background.base")?,
            background_raised: resolver
                .resolve_required(raised.and_then(|raised| raised.base.as_ref()), "background.raised.base")?,
            background_raised_high: resolver
                .resolve_required(raised.and_then(|raised| raised.high.as_ref()), "background.raised.high")?,
            background_raised_max: resolver
                .resolve_required(raised.and_then(|raised| raised.max.as_ref()), "background.raised.max")?,
            background_action,
            background_formfield,
            background_feedback,
            border: resolver.resolve_required(
                definition.border.as_ref().and_then(|border| border.base.as_ref()),
                "border.base",
            )?,
            scrollbar: resolver.resolve_required(
                definition.scrollbar.as_ref().and_then(|scrollbar| scrollbar.base.as_ref()),
                "scrollbar.base",
            )?,
            diff: resolve_diff(definition.diff.as_ref(), &mut resolver)?,
            syntax_tokens,
            markdown_tokens,
            dialog_background,
        })
    }

    // -- typed accessors ------------------------------------------------------------------

    pub fn text_action(&self, variant: Variant, active: &[State]) -> Ink {
        self.text_action.get(&variant).map(|state| state.state_of(active)).unwrap_or(self.text)
    }

    pub fn text_formfield(&self, active: &[State]) -> Ink {
        self.text_formfield.state_of(active)
    }

    pub fn background_formfield(&self, active: &[State]) -> Ink {
        self.background_formfield.state_of(active)
    }

    pub fn text_feedback(&self, kind: Feedback, muted: bool) -> Ink {
        self.text_feedback
            .get(&kind)
            .map(|pair| if muted { pair.muted } else { pair.base })
            .unwrap_or(self.text)
    }

    /// The `@dialog` surface background. Upstream `surface("dialog")` falls back to the base
    /// tokens when a theme has no `@dialog` block, so `fallback` is the caller's own base.
    pub fn dialog_background(&self, fallback: Ink) -> Ink {
        self.dialog_background.unwrap_or(fallback)
    }

    /// Markdown colours for the renderer. `token` is the upstream `MarkdownToken` name in
    /// snake_case — the fourteen are `text`, `heading`, `link`, `link_text`, `code`,
    /// `block_quote`, `emphasis`, `strong`, `horizontal_rule`, `list_item`, `list_enumeration`,
    /// `image`, `image_text`, `code_block`. An unknown name returns `None` rather than panicking
    /// so a renderer bug cannot take down the TUI; the Markdown worker should treat `None` as
    /// "use `text`".
    pub fn markdown(&self, token: &str) -> Option<Ink> {
        self.markdown_tokens.get(token).copied()
    }

    /// A `Theme` that renders even if a definition is missing a group. The dialog's error path
    /// uses it so a broken theme cannot blank the screen.
    pub fn fallback(mode: Mode) -> Self {
        Self::load(DEFAULT_THEME, mode).expect("the built-in default theme must resolve")
    }
}

/// Accessors for the token groups this screen does not paint yet.
///
/// §7.1 asks for the 2.0.27 token set, not for the subset the current view happens to use: the
/// Markdown worker reads `markdown` / `syntax` from here, and `background.feedback` and the
/// hue-step helpers exist because upstream's own components reach for them. Resolving and
/// testing them now keeps the token layer complete, so `#[allow(dead_code)]` here means
/// "resolved, no caller yet" — not "forgotten". Drop an allow as soon as a caller appears.
#[allow(dead_code)]
impl Theme {
    /// `background.action.<variant>` with its states. Part of the token set, currently read only
    /// by the tests: the one chip this screen paints uses `background.raised.high` instead,
    /// because an action-chip fill is the wrong surface for a code block.
    pub fn background_action(&self, variant: Variant, active: &[State]) -> Ink {
        self.background_action
            .get(&variant)
            .map(|state| state.state_of(active))
            .unwrap_or(Ink::Transparent)
    }

    pub fn background_feedback(&self, kind: Feedback) -> Ink {
        self.background_feedback.get(&kind).copied().unwrap_or(self.background)
    }

    /// Sorted so a caller that renders the whole group (a debug view, a test) is stable.
    pub fn markdown_tokens(&self) -> Vec<(&'static str, Ink)> {
        let mut tokens: Vec<_> = self.markdown_tokens.iter().map(|(name, ink)| (*name, *ink)).collect();
        tokens.sort_by_key(|(name, _)| *name);
        tokens
    }

    /// Syntax colours for the renderer. The nine tokens are `comment`, `keyword`, `function`,
    /// `variable`, `string`, `number`, `type`, `operator`, `punctuation`. `None` means the theme
    /// left it unset — fall back to `text`.
    pub fn syntax(&self, token: &str) -> Option<Ink> {
        self.syntax_tokens.get(token).copied()
    }

    pub fn syntax_tokens(&self) -> Vec<(&'static str, Ink)> {
        let mut tokens: Vec<_> = self.syntax_tokens.iter().map(|(name, ink)| (*name, *ink)).collect();
        tokens.sort_by_key(|(name, _)| *name);
        tokens
    }

    /// Upstream `source()`: which hue and step a resolved colour came from, if any.
    pub fn hue_source(&self, color: Ink) -> Option<(&str, u16)> {
        let rgb = color.rgb()?;
        let mut names: Vec<&String> = self.hue_scales.keys().collect();
        names.sort();
        names.into_iter().find_map(|name| {
            self.hue_scales[name]
                .iter()
                .position(|entry| *entry == rgb)
                .map(|index| (name.as_str(), HUE_STEPS[index]))
        })
    }

    /// Upstream `increase` / `decrease`: shift by whole steps within the same hue, clamped.
    pub fn shift_hue(&self, color: Ink, steps: i32) -> Ink {
        let Some((name, step)) = self.hue_source(color) else {
            return color;
        };
        let Some(scale) = self.hue_scales.get(name) else {
            return color;
        };
        let position = HUE_STEPS.iter().position(|entry| *entry == step).unwrap_or(0);
        let next = (position as i32 + steps).clamp(0, HUE_STEPS.len() as i32 - 1) as usize;
        Ink::from_rgb(scale[next])
    }
}

fn syntax_entries(syntax: Option<&SyntaxDef>) -> [(&'static str, Option<&Ref>); 9] {
    [
        ("comment", syntax.and_then(|value| value.comment.as_ref())),
        ("keyword", syntax.and_then(|value| value.keyword.as_ref())),
        ("function", syntax.and_then(|value| value.function.as_ref())),
        ("variable", syntax.and_then(|value| value.variable.as_ref())),
        ("string", syntax.and_then(|value| value.string.as_ref())),
        ("number", syntax.and_then(|value| value.number.as_ref())),
        ("type", syntax.and_then(|value| value.type_.as_ref())),
        ("operator", syntax.and_then(|value| value.operator.as_ref())),
        ("punctuation", syntax.and_then(|value| value.punctuation.as_ref())),
    ]
}

fn markdown_entries(markdown: Option<&MarkdownDef>) -> [(&'static str, Option<&Ref>); 14] {
    [
        ("text", markdown.and_then(|value| value.text.as_ref())),
        ("heading", markdown.and_then(|value| value.heading.as_ref())),
        ("link", markdown.and_then(|value| value.link.as_ref())),
        ("link_text", markdown.and_then(|value| value.link_text.as_ref())),
        ("code", markdown.and_then(|value| value.code.as_ref())),
        ("block_quote", markdown.and_then(|value| value.block_quote.as_ref())),
        ("emphasis", markdown.and_then(|value| value.emphasis.as_ref())),
        ("strong", markdown.and_then(|value| value.strong.as_ref())),
        (
            "horizontal_rule",
            markdown.and_then(|value| value.horizontal_rule.as_ref()),
        ),
        ("list_item", markdown.and_then(|value| value.list_item.as_ref())),
        (
            "list_enumeration",
            markdown.and_then(|value| value.list_enumeration.as_ref()),
        ),
        ("image", markdown.and_then(|value| value.image.as_ref())),
        ("image_text", markdown.and_then(|value| value.image_text.as_ref())),
        ("code_block", markdown.and_then(|value| value.code_block.as_ref())),
    ]
}

fn resolve_diff(definition: Option<&DiffDef>, resolver: &mut Resolver) -> Result<DiffTokens> {
    let text = definition.and_then(|diff| diff.text.as_ref());
    let background = definition.and_then(|diff| diff.background.as_ref());
    let highlight = definition.and_then(|diff| diff.highlight.as_ref());
    let line_number = definition.and_then(|diff| diff.line_number.as_ref());
    Ok(DiffTokens {
        text_added: resolver.resolve_required(text.and_then(|value| value.added.as_ref()), "diff.text.added")?,
        text_removed: resolver.resolve_required(text.and_then(|value| value.removed.as_ref()), "diff.text.removed")?,
        text_context: resolver.resolve_required(text.and_then(|value| value.context.as_ref()), "diff.text.context")?,
        text_hunk_header: resolver
            .resolve_required(text.and_then(|value| value.hunk_header.as_ref()), "diff.text.hunkHeader")?,
        background_added: resolver
            .resolve_required(background.and_then(|value| value.added.as_ref()), "diff.background.added")?,
        background_removed: resolver
            .resolve_required(background.and_then(|value| value.removed.as_ref()), "diff.background.removed")?,
        background_context: resolver
            .resolve_required(background.and_then(|value| value.context.as_ref()), "diff.background.context")?,
        highlight_added: resolver
            .resolve_required(highlight.and_then(|value| value.added.as_ref()), "diff.highlight.added")?,
        highlight_removed: resolver
            .resolve_required(highlight.and_then(|value| value.removed.as_ref()), "diff.highlight.removed")?,
        line_number_text: resolver
            .resolve_required(line_number.and_then(|value| value.text.as_ref()), "diff.lineNumber.text")?,
        line_number_added: resolver.resolve_required(
            line_number
                .and_then(|value| value.background.as_ref())
                .and_then(|value| value.added.as_ref()),
            "diff.lineNumber.background.added",
        )?,
        line_number_removed: resolver.resolve_required(
            line_number
                .and_then(|value| value.background.as_ref())
                .and_then(|value| value.removed.as_ref()),
            "diff.lineNumber.background.removed",
        )?,
    })
}

fn resolve_variants(definition: VariantsDef, resolver: &mut Resolver) -> Result<HashMap<Variant, Stateful>> {
    let mut out = HashMap::new();
    for (variant, slot) in [
        (Variant::Primary, definition.primary),
        (Variant::Secondary, definition.secondary),
        (Variant::Destructive, definition.destructive),
    ] {
        if let Some(slot) = slot {
            out.insert(variant, resolve_stateful(Some(slot), resolver)?);
        }
    }
    Ok(out)
}

fn resolve_stateful(definition: Option<StatefulDef>, resolver: &mut Resolver) -> Result<Stateful> {
    let definition = definition.unwrap_or_default();
    let base = resolver.resolve_required(definition.base.as_ref(), "stateful.base")?;
    let mut states = HashMap::new();
    for state in [State::Hovered, State::Focused, State::Pressed, State::Selected, State::Disabled] {
        if let Some(value) = definition.entry(state) {
            states.insert(state, resolver.resolve(value)?);
        }
    }
    Ok(Stateful { base, states })
}

/// `select.ts` `selectThemeMode`: the requested mode when the document has it, otherwise the
/// first defined one. Upstream's `themeModes` checks light first.
fn select(document: &ThemeDocument, requested: Mode) -> Result<ResolvedDefinition> {
    let (base, light, dark) = match document {
        ThemeDocument::WithLight { base, light, dark, .. } => (base, Some(light), dark.as_ref()),
        ThemeDocument::WithDark { base, light, dark, .. } => (base, light.as_ref(), Some(dark)),
    };
    let (mode, chosen) = match (requested, light, dark) {
        (Mode::Dark, _, Some(dark)) => (Mode::Dark, dark),
        (Mode::Light, Some(light), _) => (Mode::Light, light),
        (_, Some(light), _) => (Mode::Light, light),
        (_, _, Some(dark)) => (Mode::Dark, dark),
        _ => return Err(ThemeError::MissingMode),
    };
    let mut definition = ResolvedDefinition {
        hue: chosen.hue.clone().unwrap_or_default(),
        categorical: base.categorical.clone(),
        text: base.text.clone(),
        background: base.background.clone(),
        border: base.border.clone(),
        scrollbar: base.scrollbar.clone(),
        diff: base.diff.clone(),
        syntax: base.syntax.clone(),
        markdown: base.markdown.clone(),
        dialog: base.dialog.clone(),
        mode,
    };
    merge_mode(&mut definition, chosen);
    expand(&mut definition);
    definition.mode = mode;
    Ok(definition)
}

fn merge_mode(target: &mut ResolvedDefinition, source: &ModeDef) {
    for (name, value) in source.hue.clone().unwrap_or_default() {
        target.hue.insert(name, value);
    }
    if let Some(categorical) = &source.categorical {
        target.categorical = categorical.clone();
    }
    merge_text(&mut target.text, source.text.as_ref());
    merge_background(&mut target.background, source.background.as_ref());
    merge_single(&mut target.border, source.border.as_ref());
    merge_single(&mut target.scrollbar, source.scrollbar.as_ref());
    if let Some(diff) = &source.diff {
        target.diff = Some(merge_diff(target.diff.take(), diff));
    }
    if let Some(syntax) = &source.syntax {
        target.syntax = Some(merge_syntax(target.syntax.take(), syntax));
    }
    if let Some(markdown) = &source.markdown {
        target.markdown = Some(merge_markdown(target.markdown.take(), markdown));
    }
    if let Some(dialog) = &source.dialog {
        target.dialog = Some(merge_dialog(target.dialog.take(), dialog));
    }
}

fn merge_single(target: &mut Option<SingleDef>, source: Option<&SingleDef>) {
    let Some(source) = source else {
        return;
    };
    let mut merged = target.clone().unwrap_or_default();
    if source.base.is_some() {
        merged.base = source.base.clone();
    }
    *target = Some(merged);
}

fn merge_stateful(target: Option<&StatefulDef>, source: &StatefulDef) -> StatefulDef {
    let mut merged = target.cloned().unwrap_or_default();
    for (slot, value) in [
        (&mut merged.base, &source.base),
        (&mut merged.hovered, &source.hovered),
        (&mut merged.focused, &source.focused),
        (&mut merged.pressed, &source.pressed),
        (&mut merged.selected, &source.selected),
        (&mut merged.disabled, &source.disabled),
    ] {
        if let Some(value) = value {
            *slot = Some(value.clone());
        }
    }
    merged
}

fn merge_variants(target: Option<&VariantsDef>, source: &VariantsDef) -> VariantsDef {
    let mut merged = target.cloned().unwrap_or_default();
    for variant in [Variant::Primary, Variant::Secondary, Variant::Destructive] {
        let Some(incoming) = source.get(variant) else {
            continue;
        };
        let merged_variant = merge_stateful(merged.get(variant), incoming);
        *merged.get_mut(variant) = Some(merged_variant);
    }
    merged
}

fn merge_text(target: &mut TextDef, source: Option<&TextDef>) {
    let Some(source) = source else {
        return;
    };
    if source.base.is_some() {
        target.base = source.base.clone();
    }
    if source.muted.is_some() {
        target.muted = source.muted.clone();
    }
    if let Some(action) = &source.action {
        target.action = Some(merge_variants(target.action.as_ref(), action));
    }
    if let Some(formfield) = &source.formfield {
        target.formfield = Some(merge_stateful(target.formfield.as_ref(), formfield));
    }
    if let Some(feedback) = &source.feedback {
        let mut merged = target.feedback.clone().unwrap_or_default();
        for kind in FEEDBACK_KINDS {
            let Some(incoming) = feedback.entry(kind) else {
                continue;
            };
            let mut entry = merged.entry_mut(kind).clone().unwrap_or_default();
            if incoming.base.is_some() {
                entry.base = incoming.base.clone();
            }
            if incoming.muted.is_some() {
                entry.muted = incoming.muted.clone();
            }
            *merged.entry_mut(kind) = Some(entry);
        }
        target.feedback = Some(merged);
    }
}

fn merge_background(target: &mut BackgroundDef, source: Option<&BackgroundDef>) {
    let Some(source) = source else {
        return;
    };
    if source.base.is_some() {
        target.base = source.base.clone();
    }
    if let Some(raised) = &source.raised {
        let mut merged = target.raised.clone().unwrap_or_default();
        for (slot, value) in [
            (&mut merged.base, &raised.base),
            (&mut merged.high, &raised.high),
            (&mut merged.max, &raised.max),
        ] {
            if let Some(value) = value {
                *slot = Some(value.clone());
            }
        }
        target.raised = Some(merged);
    }
    if let Some(action) = &source.action {
        target.action = Some(merge_variants(target.action.as_ref(), action));
    }
    if let Some(formfield) = &source.formfield {
        target.formfield = Some(merge_stateful(target.formfield.as_ref(), formfield));
    }
    if let Some(feedback) = &source.feedback {
        let mut merged = target.feedback.clone().unwrap_or_default();
        for kind in FEEDBACK_KINDS {
            let Some(incoming) = feedback.entry(kind) else {
                continue;
            };
            let mut entry = merged.entry_mut(kind).clone().unwrap_or_default();
            if incoming.base.is_some() {
                entry.base = incoming.base.clone();
            }
            *merged.entry_mut(kind) = Some(entry);
        }
        target.feedback = Some(merged);
    }
}

fn merge_diff(target: Option<DiffDef>, source: &DiffDef) -> DiffDef {
    let mut merged = target.unwrap_or_default();
    if let Some(text) = &source.text {
        let mut entry = merged.text.clone().unwrap_or_default();
        for (slot, value) in [
            (&mut entry.added, &text.added),
            (&mut entry.removed, &text.removed),
            (&mut entry.context, &text.context),
            (&mut entry.hunk_header, &text.hunk_header),
        ] {
            if let Some(value) = value {
                *slot = Some(value.clone());
            }
        }
        merged.text = Some(entry);
    }
    if let Some(background) = &source.background {
        let mut entry = merged.background.clone().unwrap_or_default();
        for (slot, value) in [
            (&mut entry.added, &background.added),
            (&mut entry.removed, &background.removed),
            (&mut entry.context, &background.context),
        ] {
            if let Some(value) = value {
                *slot = Some(value.clone());
            }
        }
        merged.background = Some(entry);
    }
    if let Some(highlight) = &source.highlight {
        let mut entry = merged.highlight.clone().unwrap_or_default();
        for (slot, value) in [
            (&mut entry.added, &highlight.added),
            (&mut entry.removed, &highlight.removed),
        ] {
            if let Some(value) = value {
                *slot = Some(value.clone());
            }
        }
        merged.highlight = Some(entry);
    }
    if let Some(line_number) = &source.line_number {
        let mut entry = merged.line_number.clone().unwrap_or_default();
        if line_number.text.is_some() {
            entry.text = line_number.text.clone();
        }
        if let Some(background) = &line_number.background {
            let mut merged_background = entry.background.clone().unwrap_or_default();
            for (slot, value) in [
                (&mut merged_background.added, &background.added),
                (&mut merged_background.removed, &background.removed),
            ] {
                if let Some(value) = value {
                    *slot = Some(value.clone());
                }
            }
            entry.background = Some(merged_background);
        }
        merged.line_number = Some(entry);
    }
    merged
}

fn merge_syntax(target: Option<SyntaxDef>, source: &SyntaxDef) -> SyntaxDef {
    let mut merged = target.unwrap_or_default();
    for (slot, value) in [
        (&mut merged.comment, &source.comment),
        (&mut merged.keyword, &source.keyword),
        (&mut merged.function, &source.function),
        (&mut merged.variable, &source.variable),
        (&mut merged.string, &source.string),
        (&mut merged.number, &source.number),
        (&mut merged.type_, &source.type_),
        (&mut merged.operator, &source.operator),
        (&mut merged.punctuation, &source.punctuation),
    ] {
        if let Some(value) = value {
            *slot = Some(value.clone());
        }
    }
    merged
}

fn merge_markdown(target: Option<MarkdownDef>, source: &MarkdownDef) -> MarkdownDef {
    let mut merged = target.unwrap_or_default();
    for (slot, value) in [
        (&mut merged.text, &source.text),
        (&mut merged.heading, &source.heading),
        (&mut merged.link, &source.link),
        (&mut merged.link_text, &source.link_text),
        (&mut merged.code, &source.code),
        (&mut merged.block_quote, &source.block_quote),
        (&mut merged.emphasis, &source.emphasis),
        (&mut merged.strong, &source.strong),
        (&mut merged.horizontal_rule, &source.horizontal_rule),
        (&mut merged.list_item, &source.list_item),
        (&mut merged.list_enumeration, &source.list_enumeration),
        (&mut merged.image, &source.image),
        (&mut merged.image_text, &source.image_text),
        (&mut merged.code_block, &source.code_block),
    ] {
        if let Some(value) = value {
            *slot = Some(value.clone());
        }
    }
    merged
}

fn merge_dialog(target: Option<DialogDef>, source: &DialogDef) -> DialogDef {
    let mut merged = target.unwrap_or_default();
    if let Some(text) = &source.text {
        let mut entry = merged.text.clone().unwrap_or_default();
        merge_text(&mut entry, Some(text));
        merged.text = Some(entry);
    }
    if let Some(background) = &source.background {
        let mut entry = merged.background.clone().unwrap_or_default();
        merge_background(&mut entry, Some(background));
        merged.background = Some(entry);
    }
    merged
}

/// `expand.ts`: fill the gaps the schema allows but a renderer would otherwise have to guess
/// at. `text.muted` defaults to `$text.base`, every unset state defaults to its own group's
/// base, and every feedback `muted` defaults to that kind's `base`.
fn expand(definition: &mut ResolvedDefinition) {
    if definition.text.muted.is_none() && definition.text.base.is_some() {
        definition.text.muted = Some(Ref("$text.base".into()));
    }
    if let Some(action) = &mut definition.text.action {
        action.expand("text.action");
    }
    if let Some(formfield) = &mut definition.text.formfield {
        formfield.expand("text.formfield");
    }
    if let Some(feedback) = &mut definition.text.feedback {
        feedback.expand();
    }
    if let Some(action) = &mut definition.background.action {
        action.expand("background.action");
    }
    if let Some(formfield) = &mut definition.background.formfield {
        formfield.expand("background.formfield");
    }
}

/// `$` reference resolver. Hue scales are indexed up front so `$hue.neutral.200` never
/// re-enters the JSON tree.
struct Resolver {
    hue_scales: HashMap<String, Vec<Rgb>>,
    hue_paths: HashMap<String, Rgb>,
    categorical: Vec<Vec<Rgb>>,
    cache: HashMap<String, Ink>,
    stack: Vec<String>,
    definition: ResolvedDefinition,
}

impl Resolver {
    fn new(definition: &ResolvedDefinition) -> Result<Self> {
        // Aliases (`"accent": "$hue.orange"`) must resolve before the scales that use them, and
        // JSON object order is not an ordering guarantee, so this walks to a fixed point.
        let mut scales: HashMap<String, Vec<Rgb>> = HashMap::new();
        let mut pending: Vec<(String, HueValue)> = definition.hue.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        while !pending.is_empty() {
            let mut next = Vec::new();
            let mut progressed = false;
            for (name, value) in pending {
                match value {
                    HueValue::Scale(steps) => {
                        let mut scale = Vec::with_capacity(HUE_STEPS.len());
                        for step in HUE_STEPS {
                            let entry = steps
                                .get(&step.to_string())
                                .ok_or_else(|| ThemeError::MissingHue(format!("{name}.{step}")))?;
                            scale.push(parse_hex(&entry.0)?);
                        }
                        scales.insert(name, scale);
                        progressed = true;
                    }
                    HueValue::Alias(alias) => {
                        let target = alias.0.strip_prefix("$hue.").unwrap_or(&alias.0).to_string();
                        match scales.get(&target) {
                            Some(scale) => {
                                scales.insert(name, scale.clone());
                                progressed = true;
                            }
                            None => next.push((name, HueValue::Alias(alias))),
                        }
                    }
                }
            }
            if !progressed && !next.is_empty() {
                return Err(ThemeError::CircularReference(
                    next.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>().join(" -> "),
                ));
            }
            pending = next;
        }

        let mut hue_paths = HashMap::new();
        for (name, scale) in &scales {
            for (index, rgb) in scale.iter().enumerate() {
                hue_paths.insert(format!("hue.{name}.{}", HUE_STEPS[index]), *rgb);
            }
        }
        let categorical = definition
            .categorical
            .iter()
            .map(|name| scales.get(name).cloned().ok_or_else(|| ThemeError::MissingHue(name.clone())))
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            hue_scales: scales,
            hue_paths,
            categorical,
            cache: HashMap::new(),
            stack: Vec::new(),
            definition: definition.clone(),
        })
    }

    fn expand_stateful(&mut self, definition: Option<StatefulDef>, path: &str) -> Option<StatefulDef> {
        let mut definition = definition?;
        definition.expand(path);
        Some(definition)
    }

    fn expand_variants(&mut self, definition: Option<VariantsDef>, path: &str) -> VariantsDef {
        let mut definition = definition.unwrap_or_default();
        definition.expand(path);
        definition
    }

    fn resolve(&mut self, value: &Ref) -> Result<Ink> {
        self.resolve_path(&value.0)
    }

    fn resolve_required(&mut self, value: Option<&Ref>, label: &str) -> Result<Ink> {
        match value {
            Some(value) => self.resolve(value),
            None => Err(ThemeError::MissingReference(label.to_string())),
        }
    }

    fn resolve_path(&mut self, value: &str) -> Result<Ink> {
        if value == "transparent" || value == "none" {
            return Ok(Ink::Transparent);
        }
        if value.starts_with('#') {
            return Ok(Ink::from_rgb(parse_hex(value)?));
        }
        let path = value.strip_prefix('$').unwrap_or(value);
        if let Some(cached) = self.cache.get(path) {
            return Ok(*cached);
        }
        if self.stack.iter().any(|entry| entry == path) {
            return Err(ThemeError::CircularReference(
                self.stack.iter().cloned().chain([path.to_string()]).collect::<Vec<_>>().join(" -> "),
            ));
        }
        let ink = match self.hue_paths.get(path) {
            Some(rgb) => Ink::from_rgb(*rgb),
            None => {
                let entry = self
                    .definition
                    .lookup(path)
                    .ok_or_else(|| ThemeError::MissingReference(path.to_string()))?;
                self.stack.push(path.to_string());
                let result = self.resolve_path(&entry);
                self.stack.pop();
                result?
            }
        };
        self.cache.insert(path.to_string(), ink);
        Ok(ink)
    }
}

/// `referenceHues` name index: colour value → `$hue.<name>.<step>`. Chromatic scales are
/// indexed first so a value shared by two hues resolves to the one the palette actually used;
/// `accent`/`interactive`/`neutral` are aliases and are allowed to overwrite.
fn hue_references(scales: &HashMap<String, Vec<Rgb>>) -> HashMap<String, String> {
    let mut references = HashMap::new();
    let index = |name: &str, overwrite: bool, references: &mut HashMap<String, String>| {
        let Some(scale) = scales.get(name) else {
            return;
        };
        for (position, color) in scale.iter().enumerate() {
            let key = color.hex().to_lowercase();
            if !overwrite && references.contains_key(&key) {
                continue;
            }
            references.insert(key, format!("$hue.{name}.{}", HUE_STEPS[position]));
        }
    };
    for name in ["red", "orange", "yellow", "green", "cyan", "blue", "purple"] {
        index(name, false, &mut references);
    }
    index("gray", false, &mut references);
    for name in ["accent", "interactive", "neutral"] {
        index(name, true, &mut references);
    }
    references
}

fn parse_hex(value: &str) -> Result<Rgb> {
    Rgb::from_hex(value).ok_or_else(|| ThemeError::Invalid { what: "color".into(), value: value.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const MODES: [Mode; 2] = [Mode::Dark, Mode::Light];
    const STATES: [State; 6] = [
        State::Base,
        State::Hovered,
        State::Focused,
        State::Pressed,
        State::Selected,
        State::Disabled,
    ];

    fn hex(ink: Ink) -> String {
        match ink {
            Ink::Rgb(rgb) => rgb.hex(),
            Ink::Transparent => "transparent".into(),
        }
    }

    fn state_name(state: State) -> &'static str {
        match state {
            State::Base => "base",
            State::Hovered => "$hovered",
            State::Focused => "$focused",
            State::Pressed => "$pressed",
            State::Selected => "$selected",
            State::Disabled => "$disabled",
        }
    }

    /// Every resolved token this layer can hand out, flattened to `path -> hex`.
    ///
    /// Flattening rather than comparing field by field is what makes the migration parity test
    /// worth running: a token one side defines and the other leaves unset shows up as a missing
    /// key instead of being silently skipped by a hand-written field list. `categorical` is
    /// absent because it is a set of hue *scales* the resolver keeps separately (see
    /// `categorical_is_the_one_known_divergence`).
    fn token_map(theme: &Theme) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        out.insert("text.base".into(), hex(theme.text));
        out.insert("text.muted".into(), hex(theme.text_muted));
        out.insert("background.base".into(), hex(theme.background));
        out.insert("background.raised.base".into(), hex(theme.background_raised));
        out.insert("background.raised.high".into(), hex(theme.background_raised_high));
        out.insert("background.raised.max".into(), hex(theme.background_raised_max));
        out.insert("border.base".into(), hex(theme.border));
        out.insert("scrollbar.base".into(), hex(theme.scrollbar));
        out.insert("@dialog.background.base".into(), hex(theme.dialog_background(Ink::Transparent)));
        for (variant, name) in [
            (Variant::Primary, "primary"),
            (Variant::Secondary, "secondary"),
            (Variant::Destructive, "destructive"),
        ] {
            for state in STATES {
                out.insert(
                    format!("text.action.{name}.{}", state_name(state)),
                    hex(theme.text_action(variant, &[state])),
                );
                out.insert(
                    format!("background.action.{name}.{}", state_name(state)),
                    hex(theme.background_action(variant, &[state])),
                );
            }
        }
        for state in STATES {
            out.insert(format!("text.formfield.{}", state_name(state)), hex(theme.text_formfield(&[state])));
            out.insert(
                format!("background.formfield.{}", state_name(state)),
                hex(theme.background_formfield(&[state])),
            );
        }
        for kind in FEEDBACK_KINDS {
            let name = feedback_name(kind);
            out.insert(format!("text.feedback.{name}.base"), hex(theme.text_feedback(kind, false)));
            out.insert(format!("text.feedback.{name}.muted"), hex(theme.text_feedback(kind, true)));
            out.insert(format!("background.feedback.{name}.base"), hex(theme.background_feedback(kind)));
        }
        let diff = &theme.diff;
        for (name, value) in [
            ("text.added", diff.text_added),
            ("text.removed", diff.text_removed),
            ("text.context", diff.text_context),
            ("text.hunkHeader", diff.text_hunk_header),
            ("background.added", diff.background_added),
            ("background.removed", diff.background_removed),
            ("background.context", diff.background_context),
            ("highlight.added", diff.highlight_added),
            ("highlight.removed", diff.highlight_removed),
            ("lineNumber.text", diff.line_number_text),
            ("lineNumber.background.added", diff.line_number_added),
            ("lineNumber.background.removed", diff.line_number_removed),
        ] {
            out.insert(format!("diff.{name}"), hex(value));
        }
        for (token, ink) in theme.markdown_tokens() {
            out.insert(format!("markdown.{token}"), hex(ink));
        }
        for (token, ink) in theme.syntax_tokens() {
            out.insert(format!("syntax.{token}"), hex(ink));
        }
        for (name, scale) in &theme.hue_scales {
            for (position, rgb) in scale.iter().enumerate() {
                out.insert(format!("hue.{name}.{}", HUE_STEPS[position]), rgb.hex());
            }
        }
        out
    }

    fn differences(left: &BTreeMap<String, String>, right: &BTreeMap<String, String>) -> Vec<String> {
        let mut out = Vec::new();
        for (key, value) in left {
            match right.get(key) {
                Some(other) if other == value => {}
                Some(other) => out.push(format!("{key}: {value} != {other}")),
                None => out.push(format!("{key}: only on the left ({value})")),
            }
        }
        for key in right.keys() {
            if !left.contains_key(key) {
                out.push(format!("{key}: only on the right ({})", right[key]));
            }
        }
        out
    }

    /// The v1 palette for `name`. `opencode` is the one name whose registry entry is the *v2*
    /// document (`themes/v2/opencode.json`); its superseded v1 palette is the sibling file, so
    /// the migration tests reach for that one explicitly.
    fn v1_json(name: &str) -> &'static str {
        match name {
            "opencode" => include_str!("../themes/opencode.json"),
            other => {
                let info = find(other).expect("built-in theme");
                assert!(info.v1, "{other} is not a v1 palette; this helper is only for migrated themes");
                info.json
            }
        }
    }

    fn migrated(name: &str, mode: Mode) -> Theme {
        let document = super::v1::migrate(v1_json(name)).expect("migration");
        Theme::from_document(name, &document, mode).expect("resolve")
    }

    /// The oracle for the migration is upstream's own: it checks in the result of migrating its
    /// v1 `assets/opencode.json` as `assets/v2/opencode.json`. Two independent implementations
    /// (Effect/Solid TS vs this Rust port) landing on the same 141-odd token values is evidence;
    /// a table of expected hex values transcribed from the same TS by hand would not be.
    ///
    /// What this does *not* prove: that our resolver is right. Both sides run through
    /// `from_definition`, so a resolver bug cancels out. The resolver is pinned separately by
    /// `migrated_anchors_follow_the_upstream_rule` and the literal expectations around it.
    #[test]
    fn the_v1_opencode_palette_migrates_to_upstreams_checked_in_v2_tokens() {
        for mode in MODES {
            let ours = token_map(&migrated("opencode", mode));
            let upstream = token_map(&Theme::load("opencode", mode).unwrap());
            let diff = differences(&ours, &upstream);
            assert!(
                diff.is_empty(),
                "migration diverged from assets/v2/opencode.json in {} mode ({} tokens):\n{}",
                mode.as_str(),
                diff.len(),
                diff.join("\n")
            );
        }
    }

    /// `categoricalTokens` gained `info` in `8683406690`, but the checked-in
    /// `assets/v2/opencode.json` predates that (it was created in `168aba6183` and only
    /// hand-edited afterwards), so it lists five hues where a fresh migration lists six.
    /// Upstream's own test asserts the six-hue list including `cyan`, which is the side this
    /// port follows — pinning it here keeps the divergence from being read as a migration bug
    /// in the test above.
    #[test]
    fn categorical_is_the_one_known_divergence() {
        let ours = migrated("opencode", Mode::Dark);
        let upstream = Theme::load("opencode", Mode::Dark).unwrap();
        // Step 200 of a hue scale *is* the source colour, so this is the palette the theme
        // actually classified rather than a hex transcribed by hand.
        let ours: Vec<String> = ours.categorical.iter().map(|scale| scale[1].hex()).collect();
        let upstream: Vec<String> = upstream.categorical.iter().map(|scale| scale[1].hex()).collect();
        assert_eq!(ours.len(), upstream.len() + 1, "ours {ours:?} vs upstream {upstream:?}");
        for colour in &upstream {
            assert!(ours.contains(colour), "the migration dropped {colour}: {ours:?}");
        }
        let extra: Vec<&String> = ours.iter().filter(|colour| !upstream.contains(colour)).collect();
        assert_eq!(extra, vec![&"#56b6c2".to_string()], "the extra entry must be v1 `cyan`");
    }

    /// Upstream `migrateMode` writes five named anchors into the resolved tokens verbatim and
    /// places the rest of the neutral scale by OKLCh interpolation between them:
    ///
    /// ```text
    /// text.base       = $hue.neutral.200   and neutral.200 is anchored on v1 `text`
    /// text.muted      = $hue.neutral.400   ← v1 `textMuted`
    /// background.base = $hue.neutral.800   ← v1 `background`
    /// raised.base     = $hue.neutral.700   ← v1 `backgroundPanel`
    /// raised.high     = $hue.neutral.600   ← v1 `backgroundElement`
    /// border/scrollbar = v1 `border` / `borderActive` (literal, no scale)
    /// ```
    ///
    /// `neutralAnchors` builds the scale dark-to-light and then flips it through `1000 - step`,
    /// which is why the v1 *text* colour lands on step 200 and the v1 *background* on 800.
    /// The expected values are the v1 palette entries resolved through `defs` and the
    /// `{dark, light}` variant objects — `git show v2.0.27:packages/tui/src/theme/assets/<name>.json`.
    #[test]
    fn migrated_anchors_follow_the_upstream_rule() {
        struct Expected {
            name: &'static str,
            text: &'static str,
            text_muted: &'static str,
            background: &'static str,
            raised: &'static str,
            raised_high: &'static str,
            border: &'static str,
            scrollbar: &'static str,
            feedback: [&'static str; 4],
        }
        let cases = [
            Expected {
                name: "opencode",
                text: "#eeeeee",
                text_muted: "#808080",
                background: "#0a0a0a",
                raised: "#141414",
                raised_high: "#1e1e1e",
                border: "#484848",
                scrollbar: "#606060",
                feedback: ["#e06c75", "#f5a742", "#7fd88f", "#56b6c2"],
            },
            Expected {
                name: "dracula",
                text: "#f8f8f2",
                text_muted: "#6272a4",
                background: "#282a36",
                raised: "#21222c",
                raised_high: "#44475a",
                border: "#44475a",
                scrollbar: "#bd93f9",
                feedback: ["#ff5555", "#f1fa8c", "#50fa7b", "#ffb86c"],
            },
            Expected {
                name: "nord",
                text: "#eceff4",
                text_muted: "#8b95a7",
                background: "#2e3440",
                raised: "#3b4252",
                raised_high: "#434c5e",
                border: "#434c5e",
                scrollbar: "#4c566a",
                feedback: ["#bf616a", "#d08770", "#a3be8c", "#88c0d0"],
            },
        ];
        for case in cases {
            let theme = migrated(case.name, Mode::Dark);
            let name = case.name;
            assert_eq!(hex(theme.text), case.text, "{name} text.base");
            assert_eq!(hex(theme.text_muted), case.text_muted, "{name} text.muted");
            assert_eq!(hex(theme.background), case.background, "{name} background.base");
            assert_eq!(hex(theme.background_raised), case.raised, "{name} background.raised.base");
            assert_eq!(hex(theme.background_raised_high), case.raised_high, "{name} raised.high");
            assert_eq!(hex(theme.border), case.border, "{name} border.base");
            assert_eq!(hex(theme.scrollbar), case.scrollbar, "{name} scrollbar.base");
            for (kind, expected) in FEEDBACK_KINDS.into_iter().zip(case.feedback) {
                assert_eq!(hex(theme.text_feedback(kind, false)), expected, "{name} feedback {kind:?}");
            }
            // The two groups below are asserted for every case because an earlier version of
            // the migration dropped 11 markdown tokens and 6 syntax tokens outright (see the
            // comment in `v1::migrate_mode`) and nothing noticed.
            assert_eq!(theme.markdown_tokens().len(), 14, "{name} must resolve all markdown tokens");
            assert_eq!(theme.syntax_tokens().len(), 9, "{name} must resolve all syntax tokens");
        }
        // Spot-check that the v1 markdown/syntax values are the ones that land, rather than the
        // `text`/`primary` stand-ins the dropped version substituted.
        let opencode = migrated("opencode", Mode::Dark);
        // `markdownHeading` and `syntaxKeyword` are the same colour in this palette, so assert
        // them against the v1 file rather than against each other.
        assert_eq!(hex(opencode.markdown("heading").unwrap()), "#9d7cd8");
        assert_eq!(hex(opencode.markdown("code_block").unwrap()), "#eeeeee");
        assert_eq!(hex(opencode.syntax("keyword").unwrap()), "#9d7cd8");
        assert_eq!(hex(opencode.syntax("variable").unwrap()), "#e06c75");
        assert_eq!(hex(opencode.syntax("punctuation").unwrap()), "#eeeeee");
    }

    /// Four built-in palettes resolve to the same background in both modes, which takes
    /// `migrateV1`'s short-circuit: one palette is migrated and the hue is stored under the
    /// single detected mode. `detectMode` returns "dark" when the *text* is brighter than the
    /// background — reading that condition as a "light" flag inverts which mode key the document
    /// gets, and therefore which mode `select` hands back.
    ///
    /// These four themes resolve *every* key identically in both modes (that is what the
    /// short-circuit means), so the palette half of the inversion is not observable here and the
    /// mode key is the whole symptom: with the branches swapped, `Theme::load("aura", Dark)`
    /// came back as a light-only document and reported `Mode::Light`.
    #[test]
    fn a_single_mode_palette_is_stored_under_its_detected_mode() {
        for (name, text, background) in [
            ("aura", "#edecee", "#0f0f0f"),
            ("ayu", "#bfbdb6", "#0b0e14"),
            ("catppuccin-frappe", "#c6d0f5", "#303446"),
            ("catppuccin-macchiato", "#cad3f5", "#24273a"),
        ] {
            let theme = migrated(name, Mode::Dark);
            assert_eq!(theme.mode, Mode::Dark, "{name} is a dark palette");
            // The anchors still come from the palette (the v1 file, via `defs`).
            assert_eq!(hex(theme.text), text, "{name} text");
            assert_eq!(hex(theme.background), background, "{name} background");
            // The document only has `dark`, so asking for light resolves to the only mode
            // present (`themeModes(document)[0]`), exactly as upstream's `selectThemeMode` does.
            assert_eq!(migrated(name, Mode::Light).mode, Mode::Dark);
        }
    }

    /// A synthetic v1 palette: every key the migration reads, set to one colour, then overridden.
    /// Only used to exercise the two v1 indirections (`defs` and `{dark, light}`) and the
    /// anchor fallbacks, where the rule — not the colour — is what the test is about.
    fn synthetic_v1(overrides: &[(&str, serde_json::Value)]) -> String {
        const KEYS: [&str; 49] = [
            "primary",
            "secondary",
            "accent",
            "error",
            "warning",
            "success",
            "info",
            "text",
            "textMuted",
            "background",
            "backgroundPanel",
            "backgroundElement",
            "border",
            "borderActive",
            "diffAdded",
            "diffRemoved",
            "diffContext",
            "diffHunkHeader",
            "diffHighlightAdded",
            "diffHighlightRemoved",
            "diffAddedBg",
            "diffRemovedBg",
            "diffContextBg",
            "diffLineNumber",
            "diffAddedLineNumberBg",
            "diffRemovedLineNumberBg",
            "markdownText",
            "markdownHeading",
            "markdownLink",
            "markdownLinkText",
            "markdownCode",
            "markdownBlockQuote",
            "markdownEmph",
            "markdownStrong",
            "markdownHorizontalRule",
            "markdownListItem",
            "markdownListEnumeration",
            "markdownImage",
            "markdownImageText",
            "markdownCodeBlock",
            "syntaxComment",
            "syntaxKeyword",
            "syntaxFunction",
            "syntaxVariable",
            "syntaxString",
            "syntaxNumber",
            "syntaxType",
            "syntaxOperator",
            "syntaxPunctuation",
        ];
        let mut theme = serde_json::Map::new();
        for key in KEYS {
            theme.insert(key.to_string(), serde_json::Value::String("#123456".into()));
        }
        for (key, value) in overrides {
            if value.is_null() {
                theme.remove(*key);
            } else {
                theme.insert((*key).to_string(), value.clone());
            }
        }
        serde_json::json!({ "theme": theme, "defs": { "ink": "#112233", "paper": "#f0f0f0" } }).to_string()
    }

    fn from_synthetic(json: &str, mode: Mode) -> Theme {
        let document = super::v1::migrate(json).expect("synthetic palette should migrate");
        Theme::from_document("synthetic", &document, mode).expect("synthetic palette should resolve")
    }

    /// `defs` chains and `{dark, light}` variant objects are the two v1 indirections.
    #[test]
    fn defs_and_mode_variants_resolve() {
        let json = synthetic_v1(&[
            ("text", serde_json::json!({ "dark": "ink", "light": "paper" })),
            ("textMuted", serde_json::json!("#777777")),
            ("background", serde_json::json!({ "dark": "#000000", "light": "#ffffff" })),
        ]);
        let dark = from_synthetic(&json, Mode::Dark);
        let light = from_synthetic(&json, Mode::Light);
        assert_eq!(hex(dark.text), "#112233", "dark must follow the `ink` def");
        assert_eq!(hex(light.text), "#f0f0f0", "light must follow the `paper` def");
        assert_eq!(hex(dark.background), "#000000");
        assert_eq!(hex(light.background), "#ffffff");
        // A plain literal is shared by both modes, which is what makes the two-mode document
        // shape (base + per-mode hue) worth keeping.
        assert_eq!(hex(dark.text_muted), "#777777");
        assert_eq!(hex(light.text_muted), "#777777");
    }

    /// Upstream writes `theme.backgroundElement || theme.backgroundMenu`, and an `RGBA` object is
    /// always truthy in JS — so the fallback fires when the key is *missing*, never when its
    /// value is transparent. Treating transparency as absence silently swaps a different colour
    /// into the neutral scale's 600 step.
    #[test]
    fn a_missing_background_element_falls_back_but_a_transparent_one_does_not() {
        let missing = synthetic_v1(&[
            ("backgroundElement", serde_json::Value::Null),
            ("backgroundMenu", serde_json::json!("#abcdef")),
        ]);
        assert_eq!(hex(from_synthetic(&missing, Mode::Dark).background_raised_high), "#abcdef");

        let transparent = synthetic_v1(&[
            ("backgroundElement", serde_json::json!("transparent")),
            ("backgroundMenu", serde_json::json!("#abcdef")),
        ]);
        let theme = from_synthetic(&transparent, Mode::Dark);
        assert_eq!(
            hex(theme.background_raised_high),
            "transparent",
            "a transparent element is still the element; only a missing one falls back"
        );
    }

    /// `selectedForeground`: v1's own `selectedListItemText` when the file sets one, otherwise
    /// the theme background — but only when the background is fully transparent does it reach
    /// the luminance branch.
    #[test]
    fn selected_foreground_prefers_the_v1_field_then_the_background() {
        let with_field = synthetic_v1(&[
            ("selectedListItemText", serde_json::json!("#00ff00")),
            ("error", serde_json::json!("#ff0000")),
        ]);
        assert_eq!(hex(from_synthetic(&with_field, Mode::Dark).text_action(Variant::Primary, &[State::Focused])), "#00ff00");

        let without = synthetic_v1(&[("error", serde_json::json!("#ff0000"))]);
        let theme = from_synthetic(&without, Mode::Dark);
        assert_eq!(
            hex(theme.text_action(Variant::Destructive, &[])),
            hex(theme.background),
            "without the field the destructive colour is the background"
        );
    }

    /// Upstream's `ActionState.literals` order is disabled → pressed → focused → selected →
    /// hovered, and `state()` picks the first flag that is set. An element can be both hovered
    /// and disabled, and which colour wins is visible.
    #[test]
    fn the_disabled_state_outranks_hovered() {
        let theme = Theme::load(DEFAULT_THEME, Mode::Dark).unwrap();
        let disabled = hex(theme.text_action(Variant::Primary, &[State::Disabled]));
        let hovered = hex(theme.text_action(Variant::Primary, &[State::Hovered]));
        assert_ne!(disabled, hovered, "the default theme must distinguish them");
        assert_eq!(
            hex(theme.text_action(Variant::Primary, &[State::Hovered, State::Disabled])),
            disabled
        );
        assert_eq!(hex(theme.text_action(Variant::Primary, &[State::Hovered])), hovered);
    }

    /// `transparent` is a distinct value, not black: the v2 default theme uses it for
    /// `background.action.primary.base`, and painting that cell black would break the chip.
    #[test]
    fn transparent_survives_resolution_and_blends_only_when_asked() {
        let theme = Theme::load(DEFAULT_THEME, Mode::Dark).unwrap();
        let chip = theme.background_action(Variant::Primary, &[]);
        assert_eq!(chip, Ink::Transparent);
        assert_eq!(chip.to_ratatui(), Color::Reset);
        assert_eq!(chip.over(theme.background), theme.background.to_ratatui());
        assert_eq!(chip.rgb(), None);
        // The selected state is a real colour, so the "always transparent" case is excluded.
        assert_ne!(theme.background_action(Variant::Primary, &[State::Focused]), Ink::Transparent);
    }

    /// Every built-in theme has to resolve in both modes: the picker previews a theme the moment
    /// the cursor lands on it, so a palette that only works in one mode blanks the screen.
    #[test]
    fn every_built_in_theme_resolves_in_both_modes() {
        let names = theme_names();
        assert!(names.len() > 20, "the registry shrank unexpectedly: {names:?}");
        for name in names {
            for mode in MODES {
                let theme = Theme::load(name, mode).unwrap_or_else(|error| panic!("{name} ({mode:?}): {error}"));
                assert!(!token_map(&theme).is_empty(), "{name} resolved to nothing");
                // The Markdown worker takes its colours from these two groups, so an empty one
                // is a silent regression for a different task.
                assert_eq!(theme.markdown_tokens().len(), 14, "{name} markdown group");
                assert_eq!(theme.syntax_tokens().len(), 9, "{name} syntax group");
                assert!(
                    matches!(theme.mode, Mode::Dark | Mode::Light),
                    "{name} reported a mode that is not one of the two"
                );
            }
        }
    }

    /// A v2 document with every group the resolver requires, so a test can vary just the field
    /// it is about. The values are meaningless on purpose — these tests are about which rule
    /// fires, not about a colour.
    fn minimal_document(mode: &str) -> serde_json::Value {
        let scale = serde_json::json!({
            "100": "#111111", "200": "#222222", "300": "#333333", "400": "#444444", "500": "#555555",
            "600": "#666666", "700": "#777777", "800": "#888888", "900": "#999999",
        });
        let feedback = serde_json::json!({
            "error": { "base": "#000000" }, "warning": { "base": "#000000" },
            "success": { "base": "#000000" }, "info": { "base": "#000000" },
        });
        let mut document = serde_json::json!({
            "base": {
                "categorical": ["gray"],
                "text": {
                    "base": "#ffffff",
                    "muted": "#eeeeee",
                    "action": { "primary": { "base": "#ffffff" } },
                    "formfield": { "base": "#ffffff" },
                    "feedback": {
                        "error": { "base": "#ff0000" }, "warning": { "base": "#ffff00" },
                        "success": { "base": "#00ff00" }, "info": { "base": "#00ffff" },
                    },
                },
                "background": {
                    "base": "#000000",
                    "raised": { "base": "#010101", "high": "#020202", "max": "#030303" },
                    "action": { "primary": { "base": "#010101" } },
                    "formfield": { "base": "#000000" },
                    "feedback": feedback,
                },
                "border": { "base": "#111111" },
                "scrollbar": { "base": "#111111" },
                "diff": {
                    "text": { "added": "#111111", "removed": "#111111", "context": "#111111", "hunkHeader": "#111111" },
                    "background": { "added": "#111111", "removed": "#111111", "context": "#111111" },
                    "highlight": { "added": "#111111", "removed": "#111111" },
                    "lineNumber": { "text": "#111111", "background": { "added": "#111111", "removed": "#111111" } },
                },
            },
        });
        if !mode.is_empty() {
            document[mode] = serde_json::json!({ "hue": { "gray": scale } });
        }
        document
    }

    fn parse_document(document: &serde_json::Value) -> Result<ThemeDocument> {
        let raw: RawDocument = serde_json::from_value(document.clone())
            .map_err(|error| ThemeError::Invalid { what: "theme".into(), value: error.to_string() })?;
        raw.into_document()
    }

    fn document(mode: &str) -> ThemeDocument {
        parse_document(&minimal_document(mode)).expect("test document should name a mode")
    }

    /// `$` references are the only way a v2 document names a colour, so all three failure modes
    /// have to be errors rather than a silent black.
    #[test]
    fn broken_references_are_errors_not_silent_black() {
        assert!(matches!(
            Theme::load("definitely-not-a-theme", Mode::Dark),
            Err(ThemeError::UnknownTheme(name)) if name == "definitely-not-a-theme"
        ));

        let mut missing = minimal_document("dark");
        missing["base"]["text"]["base"] = serde_json::json!("$text.nope");
        assert!(matches!(
            Theme::from_document("x", &document_of(&missing), Mode::Dark),
            Err(ThemeError::MissingReference(path)) if path == "text.nope"
        ));

        let mut circular = minimal_document("dark");
        circular["base"]["text"]["base"] = serde_json::json!("$text.muted");
        circular["base"]["text"]["muted"] = serde_json::json!("$text.base");
        assert!(matches!(
            Theme::from_document("x", &document_of(&circular), Mode::Dark),
            Err(ThemeError::CircularReference(_))
        ));

        let mut unknown_hue = minimal_document("dark");
        unknown_hue["base"]["categorical"] = serde_json::json!(["nope"]);
        assert!(matches!(
            Theme::from_document("x", &document_of(&unknown_hue), Mode::Dark),
            Err(ThemeError::MissingHue(name)) if name == "nope"
        ));

        let mut bad_hex = minimal_document("dark");
        bad_hex["base"]["border"]["base"] = serde_json::json!("#12345");
        assert!(matches!(
            Theme::from_document("x", &document_of(&bad_hex), Mode::Dark),
            Err(ThemeError::Invalid { what, .. }) if what == "color"
        ));
    }

    fn document_of(value: &serde_json::Value) -> ThemeDocument {
        parse_document(value).expect("test document should parse")
    }

    /// A document with only `light` still resolves when `dark` is asked for — `selectThemeMode`
    /// falls back to the first mode present — and the caller has to be able to see which one it
    /// actually got, because the UI paints differently for each.
    #[test]
    fn the_requested_mode_falls_back_to_the_only_one_present() {
        let theme = Theme::from_document("x", &document("light"), Mode::Dark).unwrap();
        assert_eq!(theme.mode, Mode::Light);

        assert!(matches!(parse_document(&minimal_document("")), Err(ThemeError::MissingMode)));
    }

    /// An unknown token name must return `None` rather than panic: the Markdown renderer is
    /// written against these names, and a typo there should not take down the TUI.
    #[test]
    fn unknown_markdown_and_syntax_tokens_are_none() {
        let theme = Theme::load(DEFAULT_THEME, Mode::Dark).unwrap();
        assert!(theme.markdown("markdown_heading").is_none(), "names are the upstream snake_case tokens");
        assert!(theme.markdown("heading").is_some());
        assert!(theme.markdown("nope").is_none());
        assert!(theme.syntax("punctuation").is_some());
        assert!(theme.syntax("nope").is_none());
    }

    /// `source` / `increase` / `decrease` are upstream's hue-step helpers: a resolved colour
    /// that came from a scale can be nudged by whole steps inside that same scale.
    #[test]
    fn hue_steps_move_within_the_scale_they_came_from() {
        let theme = Theme::load(DEFAULT_THEME, Mode::Dark).unwrap();
        let base = theme.background_raised_high;
        let (name, step) = theme.hue_source(base).expect("raised.high comes from a hue scale");
        assert_eq!(step, 600, "{name} should report the step it was anchored to");
        let darker = theme.shift_hue(base, 1);
        assert_eq!(theme.hue_source(darker).map(|(_, step)| step), Some(700));
        assert_eq!(theme.shift_hue(darker, -1), base);
        // Clamped at the ends instead of panicking or wrapping around.
        assert_eq!(theme.hue_source(theme.shift_hue(base, 99)).map(|(_, step)| step), Some(900));
        assert_eq!(theme.hue_source(theme.shift_hue(base, -99)).map(|(_, step)| step), Some(100));
        // A colour that is not from a scale is returned unchanged.
        assert_eq!(theme.shift_hue(Ink::Transparent, 1), Ink::Transparent);
    }

    /// The v1 migration marks its documents so `from_document` runs the `referenceHues` pass.
    /// Without it a migrated theme renders identically but stops responding to hue steps: a
    /// literal that happens to equal a scale step is not recognised as coming from that scale.
    #[test]
    fn migrated_documents_get_their_hues_referenced() {
        // `border` is written as a literal, but here it is also the neutral scale's 700 anchor
        // (`backgroundPanel`), which is exactly the case `referenceHues` exists to fold back
        // into `$hue.neutral.700`.
        let json = synthetic_v1(&[
            ("backgroundPanel", serde_json::json!("#202020")),
            ("border", serde_json::json!("#202020")),
        ]);
        let theme = from_synthetic(&json, Mode::Dark);
        assert_eq!(hex(theme.border), "#202020", "re-referencing must not change the colour");
        assert!(
            theme.hue_source(theme.border).is_some(),
            "a literal equal to a scale step must be re-referenced, or hue steps stop working"
        );
        assert_ne!(theme.shift_hue(theme.border, 1), theme.border);
    }
}
