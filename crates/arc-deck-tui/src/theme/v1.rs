//! v1 flat palette → v2 semantic tokens.
//! Copyright (c) 2025 opencode. MIT notice: ../LICENSE-OpenCode.
//!
//! Ported from OpenCode v2.0.27 `packages/theme/src/tui/v1-migrate.ts`. The built-in themes
//! under `crates/arc-deck-tui/themes/*.json` are v1 palettes (no state roles), so this is the
//! only path that turns them into the token shape `theme.rs` speaks. Upstream keeps it for the
//! same reason: `packages/tui/src/theme/index.ts` calls `migrateV1` whenever a theme document
//! has a `theme` key.
//!
//! Shape of the migration: (1) resolve the flat palette twice, once per mode; (2) infer which
//! chromatic hue each *semantic* v1 colour is nearest to in OKLCh and build a 9-step scale
//! around it; (3) anchor a neutral scale on background/text/panel; (4) write the v2 token
//! groups as `$hue.*` references.
//!
//! Two upstream branches matter for parity and are easy to get wrong:
//! - `migrateV1` short-circuits when light and dark resolve to the same background *and*
//!   `detectMode` agrees; the shared definition then becomes `base` for both modes.
//! - `inferHues` runs twice over the accent/primary pair, so accent wins a hue collision.

use std::collections::HashMap;

use serde::Deserialize;

use super::{
    BackgroundDef, BackgroundFeedbackDef, BaseDef, DialogDef, DiffBackgroundDef, DiffDef, DiffHighlightDef,
    DiffLineNumberDef, DiffTextDef, FeedbackTextDef, HueValue, MarkdownDef, Mode, ModeDef, RaisedDef, Ref,
    ResolvedDefinition, Result, SingleDef, StatefulDef, SyntaxDef, TextDef, TextFeedbackDef, ThemeDocument, ThemeError,
    VariantsDef, HUE_STEPS,
};
use crate::color::{Rgb, ansi_to_rgb, interpolate_oklch, oklch_to_hex, rgb_to_oklch};

type ChromaticHue = &'static str;
const CHROMATIC_HUES: [ChromaticHue; 7] = ["red", "orange", "yellow", "green", "cyan", "blue", "purple"];
/// Upstream `categoricalTokens`, in the order it dedupes them.
const CATEGORICAL_TOKENS: [&str; 7] = ["secondary", "accent", "success", "warning", "primary", "error", "info"];
const MINIMUM_CHROMA: f64 = 0.03;
const LIGHT_THRESHOLD: f64 = 0.6;

/// The canonical swatches upstream copied out of the original default-theme classifier.
///
/// The bool is "this is the **light** swatch", matching `hueAngles.light` — the lookup keys off
/// `oklch.l >= lightThreshold`, and upstream picks `hueAngles.light` in that case. The tuple was
/// first written the other way round, which made every bright colour (lightness ≥ 0.6) compare
/// against the *dark* swatches; the scales then got filed under the wrong hue name. In the v1
/// `opencode` palette that renamed the hue carrying `primary`/`warning` from `orange` to
/// `yellow`, which is what the parity test against `assets/v2/opencode.json` caught.
const HUE_REFERENCES: [(bool, ChromaticHue, &str); 14] = [
    (true, "red", "#fca5a5"),
    (true, "orange", "#fdba74"),
    (true, "yellow", "#fde047"),
    (true, "green", "#86efac"),
    (true, "cyan", "#67e8f9"),
    (true, "blue", "#93c5fd"),
    (true, "purple", "#d8b4fe"),
    (false, "red", "#b91c1c"),
    (false, "orange", "#c2410c"),
    (false, "yellow", "#a16207"),
    (false, "green", "#15803d"),
    (false, "cyan", "#0e7490"),
    (false, "blue", "#1d4ed8"),
    (false, "purple", "#7e22ce"),
];

// ---------------------------------------------------------------------------------------
// v1 file shape
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct V1File {
    /// Required: upstream's `parseTheme` keys off exactly this field to tell a v1 palette from
    /// a v2 document, so a file without it is not a v1 theme.
    theme: HashMap<String, V1Color>,
    #[serde(default)]
    defs: HashMap<String, V1Color>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum V1Color {
    Hex(String),
    Ansi(i64),
    Variant { dark: Box<V1Color>, light: Box<V1Color> },
}

/// One mode of a resolved v1 palette. Named fields rather than a map so a missing required key
/// is a deserialize error instead of an `unwrap` on a lookup.
#[derive(Debug, Clone)]
struct V1Mode {
    primary: Rgb,
    secondary: Rgb,
    accent: Rgb,
    error: Rgb,
    warning: Rgb,
    success: Rgb,
    info: Rgb,
    text: Rgb,
    text_muted: Rgb,
    background: Rgb,
    background_panel: Rgb,
    /// `None` when the file omitted the key. Upstream writes `theme.backgroundElement ||
    /// theme.backgroundMenu`, and an `RGBA` object is always truthy in JS — so the fallback
    /// fires on *absence*, not on transparency. Treating a transparent `backgroundElement` as
    /// absent (which an earlier version of this port did) would silently swap in a different
    /// anchor colour.
    background_element: Option<Rgb>,
    background_menu: Rgb,
    border: Rgb,
    border_active: Rgb,
    diff_added: Rgb,
    diff_removed: Rgb,
    diff_context: Rgb,
    diff_hunk_header: Rgb,
    diff_highlight_added: Rgb,
    diff_highlight_removed: Rgb,
    diff_added_bg: Rgb,
    diff_removed_bg: Rgb,
    diff_context_bg: Rgb,
    diff_line_number: Rgb,
    diff_added_line_number_bg: Rgb,
    diff_removed_line_number_bg: Rgb,
    /// 14 `markdown*` slots and 9 `syntax*` slots. Every built-in v1 palette defines all of
    /// them (`ThemeV1Json` makes them required), and upstream's migration copies each one into
    /// its v2 token — so dropping the groups here would hand the Markdown renderer the wrong
    /// colours on all 33 migrated themes rather than "no colours".
    markdown: MarkdownColors,
    syntax: SyntaxColors,
    /// v1 `selectedListItemText`, or the background when the file did not set one. Only read by
    /// `selected_foreground`.
    selected_list_item_text: Rgb,
    has_selected_list_item_text: bool,
}

#[derive(Debug, Clone, Copy)]
struct MarkdownColors {
    text: Rgb,
    heading: Rgb,
    link: Rgb,
    link_text: Rgb,
    code: Rgb,
    block_quote: Rgb,
    emphasis: Rgb,
    strong: Rgb,
    horizontal_rule: Rgb,
    list_item: Rgb,
    list_enumeration: Rgb,
    image: Rgb,
    image_text: Rgb,
    code_block: Rgb,
}

#[derive(Debug, Clone, Copy)]
struct SyntaxColors {
    comment: Rgb,
    keyword: Rgb,
    function: Rgb,
    variable: Rgb,
    string: Rgb,
    number: Rgb,
    type_: Rgb,
    operator: Rgb,
    punctuation: Rgb,
}

pub(super) fn migrate(json: &str) -> Result<ThemeDocument> {
    let file: V1File = serde_json::from_str(json).map_err(|error| ThemeError::Invalid {
        what: "v1 theme".into(),
        value: error.to_string(),
    })?;
    let light = resolve_v1(&file, false)?;
    let dark = resolve_v1(&file, true)?;
    if light.background.has_alpha() && dark.background.has_alpha() && light.background == dark.background {
        let light_is_dark = is_dark(&light);
        let dark_is_dark = is_dark(&dark);
        if light_is_dark == dark_is_dark {
            // Upstream passes `lightMode` (the string "dark"/"light") as BOTH the palette
            // selector and the mode argument, then stores the hue under that same mode key.
            // The bool has to mean "dark", not "light": `detectMode` returns "dark" when the
            // text is brighter than the background. Reading it as a `light` flag inverts three
            // things at once — which palette is used, which mode key the document gets, and
            // therefore which mode `select()` hands back. Four built-in themes (aura, ayu,
            // catppuccin-frappe, catppuccin-macchiato) resolve to identical backgrounds in both
            // modes and do take this branch, so the inversion was observable, not latent.
            let definition = migrate_mode(if light_is_dark { &dark } else { &light }, light_is_dark)?;
            let base = base_of(&definition);
            let hue = hue_only(&definition);
            return Ok(if light_is_dark {
                ThemeDocument::WithDark { base, light: None, dark: hue, migrated: true }
            } else {
                ThemeDocument::WithLight { base, light: hue, dark: None, migrated: true }
            });
        }
    }
    let light_definition = migrate_mode(&light, false)?;
    let dark_definition = migrate_mode(&dark, true)?;
    let base = base_of(&light_definition);
    Ok(ThemeDocument::WithLight {
        base,
        light: hue_only(&light_definition),
        // The *whole* dark definition, not just its hue. `base` carries the light definition's
        // token groups, so a hue-only dark mode resolves every text/background/border/diff/
        // syntax/markdown token from the light palette — dark mode looks light. Upstream writes
        // `dark: darkDefinition` here for exactly this reason.
        dark: Some(full_mode(dark_definition)),
        migrated: true,
    })
}

/// Upstream `base()`: everything except `hue` becomes the shared base. `categorical` rides along
/// here because the definitions are already mode-specific and `ModeDef` has no such field.
fn base_of(definition: &ResolvedDefinition) -> BaseDef {
    BaseDef {
        categorical: definition.categorical.clone(),
        text: definition.text.clone(),
        background: definition.background.clone(),
        border: definition.border.clone(),
        scrollbar: definition.scrollbar.clone(),
        diff: definition.diff.clone(),
        syntax: definition.syntax.clone(),
        markdown: definition.markdown.clone(),
        dialog: definition.dialog.clone(),
    }
}

/// Upstream `{ hue: definition.hue }`: the per-mode half when `base` already holds this mode's
/// token groups.
fn hue_only(definition: &ResolvedDefinition) -> ModeDef {
    ModeDef { hue: Some(definition.hue.clone()), ..ModeDef::default() }
}

/// Upstream `dark: darkDefinition`: the per-mode half that has to carry the groups itself.
///
/// `categorical` is the one field upstream leaves behind. It writes `base: base(lightDefinition)`
/// plus a hue-only light mode, so *both* modes resolve the light-mode inference — and the names
/// in it are looked up in the mode's own hue map, which is a different set: `carbonfox` infers
/// `cyan` from its light palette but not from its dark one, so upstream's resolver throws
/// `Categorical hue "cyan" was not found` and rejects the whole theme. Carrying each mode's own
/// list keeps the invariant the field is supposed to have (one scale per inferred semantic
/// token) without inventing a palette. Deviates from upstream's document shape on purpose.
fn full_mode(definition: ResolvedDefinition) -> ModeDef {
    ModeDef {
        hue: Some(definition.hue),
        categorical: Some(definition.categorical),
        text: Some(definition.text),
        background: Some(definition.background),
        border: definition.border,
        scrollbar: definition.scrollbar,
        diff: definition.diff,
        syntax: definition.syntax,
        markdown: definition.markdown,
        dialog: definition.dialog,
    }
}

/// Upstream `detectMode`: "dark" when the text is brighter than the background, i.e. a dark
/// theme. Returns that as a bool because the only caller compares two of them; the *name* has to
/// stay "is dark" — see the collapse branch in `migrate`.
fn is_dark(mode: &V1Mode) -> bool {
    mode.text.luminance() > mode.background.luminance()
}

// ---------------------------------------------------------------------------------------
// Step 1: resolve the flat palette (v1 resolve.ts)
// ---------------------------------------------------------------------------------------

fn resolve_v1(file: &V1File, dark: bool) -> Result<V1Mode> {
    let color = |name: &str| -> Result<Rgb> {
        let value = file
            .theme
            .get(name)
            .ok_or_else(|| ThemeError::MissingReference(name.to_string()))?;
        resolve_color(file, value, dark)
    };
    let selected = match file.theme.get("selectedListItemText") {
        Some(value) => resolve_color(file, value, dark)?,
        None => color("background")?,
    };
    let background_menu = match file.theme.get("backgroundMenu") {
        Some(value) => resolve_color(file, value, dark)?,
        None => color("backgroundElement")?,
    };
    Ok(V1Mode {
        primary: color("primary")?,
        secondary: color("secondary")?,
        accent: color("accent")?,
        error: color("error")?,
        warning: color("warning")?,
        success: color("success")?,
        info: color("info")?,
        text: color("text")?,
        text_muted: color("textMuted")?,
        background: color("background")?,
        background_panel: color("backgroundPanel")?,
        background_element: match file.theme.get("backgroundElement") {
            Some(value) => Some(resolve_color(file, value, dark)?),
            None => None,
        },
        background_menu,
        border: color("border")?,
        border_active: color("borderActive")?,
        diff_added: color("diffAdded")?,
        diff_removed: color("diffRemoved")?,
        diff_context: color("diffContext")?,
        diff_hunk_header: color("diffHunkHeader")?,
        diff_highlight_added: color("diffHighlightAdded")?,
        diff_highlight_removed: color("diffHighlightRemoved")?,
        diff_added_bg: color("diffAddedBg")?,
        diff_removed_bg: color("diffRemovedBg")?,
        diff_context_bg: color("diffContextBg")?,
        diff_line_number: color("diffLineNumber")?,
        diff_added_line_number_bg: color("diffAddedLineNumberBg")?,
        diff_removed_line_number_bg: color("diffRemovedLineNumberBg")?,
        markdown: MarkdownColors {
            text: color("markdownText")?,
            heading: color("markdownHeading")?,
            link: color("markdownLink")?,
            link_text: color("markdownLinkText")?,
            code: color("markdownCode")?,
            block_quote: color("markdownBlockQuote")?,
            emphasis: color("markdownEmph")?,
            strong: color("markdownStrong")?,
            horizontal_rule: color("markdownHorizontalRule")?,
            list_item: color("markdownListItem")?,
            list_enumeration: color("markdownListEnumeration")?,
            image: color("markdownImage")?,
            image_text: color("markdownImageText")?,
            code_block: color("markdownCodeBlock")?,
        },
        syntax: SyntaxColors {
            comment: color("syntaxComment")?,
            keyword: color("syntaxKeyword")?,
            function: color("syntaxFunction")?,
            variable: color("syntaxVariable")?,
            string: color("syntaxString")?,
            number: color("syntaxNumber")?,
            type_: color("syntaxType")?,
            operator: color("syntaxOperator")?,
            punctuation: color("syntaxPunctuation")?,
        },
        selected_list_item_text: selected,
        has_selected_list_item_text: file.theme.contains_key("selectedListItemText"),
    })
}

fn resolve_color(file: &V1File, value: &V1Color, dark: bool) -> Result<Rgb> {
    resolve_color_chain(file, value, dark, &mut Vec::new())
}

fn resolve_color_chain(file: &V1File, value: &V1Color, dark: bool, chain: &mut Vec<String>) -> Result<Rgb> {
    match value {
        V1Color::Hex(text) if text == "transparent" || text == "none" => Ok(Rgb::from_ints(0, 0, 0, 0)),
        V1Color::Hex(text) if text.starts_with('#') => {
            Rgb::from_hex(text).ok_or_else(|| ThemeError::Invalid { what: "color".into(), value: text.clone() })
        }
        V1Color::Hex(name) => {
            if chain.iter().any(|entry| entry == name) {
                return Err(ThemeError::CircularReference(
                    chain.iter().cloned().chain([name.clone()]).collect::<Vec<_>>().join(" -> "),
                ));
            }
            let next = file.defs.get(name).or_else(|| file.theme.get(name));
            let next = next.ok_or_else(|| ThemeError::MissingReference(name.clone()))?;
            chain.push(name.clone());
            let result = resolve_color_chain(file, next, dark, chain);
            chain.pop();
            result
        }
        V1Color::Ansi(code) => Ok(if (0..=255).contains(code) { ansi_to_rgb(*code as u8) } else { ansi_to_rgb(0) }),
        V1Color::Variant { dark: dark_value, light } => {
            resolve_color_chain(file, if dark { dark_value } else { light }, dark, chain)
        }
    }
}

// ---------------------------------------------------------------------------------------
// Step 2: one mode → v2 tokens
// ---------------------------------------------------------------------------------------

fn migrate_mode(mode: &V1Mode, dark: bool) -> Result<ResolvedDefinition> {
    // These five anchor names, and the token groups below, are upstream's fixed choices; the
    // palette only influences which hex ends up at each step.
    let text = "$hue.neutral.200";
    let text_muted = "$hue.neutral.400";
    let primary = "$hue.interactive.200";
    let background = "$hue.neutral.800";
    let background_panel = "$hue.neutral.700";
    let background_menu = "$hue.neutral.600";
    let background_raised_max = "$hue.neutral.500";

    let selected = selected_foreground(mode, mode.primary).hex();
    let destructive = selected_foreground(mode, mode.error).hex();
    let inferred = infer_hues(mode);
    let categorical = {
        let mut seen: Vec<ChromaticHue> = Vec::new();
        for token in CATEGORICAL_TOKENS {
            let Some(name) = inferred.by_token.get(token) else {
                continue;
            };
            if !seen.contains(name) {
                seen.push(name);
            }
        }
        if seen.is_empty() {
            vec!["neutral".to_string()]
        } else {
            seen.into_iter().map(str::to_string).collect()
        }
    };

    let mut hue: HashMap<String, HueValue> = HashMap::new();
    hue.insert("gray".into(), HueValue::Scale(neutral_scale(mode)));
    for name in CHROMATIC_HUES {
        if let Some(found) = inferred.by_hue.get(name) {
            hue.insert(name.into(), HueValue::Scale(hue_scale(found.color, dark)));
        }
    }
    hue.insert("accent".into(), alias(hue_alias(&inferred, "accent")));
    hue.insert("interactive".into(), alias(hue_alias(&inferred, "primary")));
    hue.insert("neutral".into(), alias("$hue.gray".to_string()));

    Ok(ResolvedDefinition {
        hue,
        categorical,
        text: TextDef {
            base: Some(r(text)),
            muted: Some(r(text_muted)),
            action: Some(VariantsDef {
                primary: Some(StatefulDef {
                    base: Some(r("$text.base")),
                    disabled: Some(r(text_muted)),
                    focused: Some(r(&selected)),
                    selected: Some(r(primary)),
                    ..StatefulDef::default()
                }),
                secondary: Some(StatefulDef {
                    base: Some(r("$text.muted")),
                    hovered: Some(r("$text.base")),
                    ..StatefulDef::default()
                }),
                destructive: Some(StatefulDef {
                    base: Some(r(&destructive)),
                    disabled: Some(r(text_muted)),
                    ..StatefulDef::default()
                }),
            }),
            formfield: Some(StatefulDef {
                base: Some(r(text)),
                hovered: Some(r(primary)),
                focused: Some(r(primary)),
                pressed: Some(r(primary)),
                disabled: Some(r(text_muted)),
                selected: Some(r(primary)),
            }),
            feedback: Some(TextFeedbackDef {
                error: Some(feedback_text(&mode.error.hex())),
                warning: Some(feedback_text(&mode.warning.hex())),
                success: Some(feedback_text(&mode.success.hex())),
                info: Some(feedback_text(&mode.info.hex())),
            }),
        },
        background: BackgroundDef {
            base: Some(r(background)),
            raised: Some(RaisedDef {
                base: Some(r(background_panel)),
                high: Some(r(background_menu)),
                max: Some(r(background_raised_max)),
            }),
            action: Some(VariantsDef {
                primary: Some(StatefulDef {
                    base: Some(r("transparent")),
                    hovered: Some(r(background_panel)),
                    focused: Some(r(primary)),
                    selected: Some(r("transparent")),
                    ..StatefulDef::default()
                }),
                secondary: Some(StatefulDef { base: Some(r("transparent")), ..StatefulDef::default() }),
                destructive: Some(StatefulDef { base: Some(r(&mode.error.hex())), ..StatefulDef::default() }),
            }),
            formfield: Some(StatefulDef { base: Some(r("$background.base")), ..StatefulDef::default() }),
            feedback: Some(BackgroundFeedbackDef {
                error: Some(feedback_background("$background.base")),
                warning: Some(feedback_background("$background.base")),
                success: Some(feedback_background("$background.base")),
                info: Some(feedback_background("$background.base")),
            }),
        },
        border: Some(SingleDef { base: Some(r(&mode.border.hex())) }),
        scrollbar: Some(SingleDef { base: Some(r(&mode.border_active.hex())) }),
        diff: Some(DiffDef {
            text: Some(DiffTextDef {
                added: Some(r(&mode.diff_added.hex())),
                removed: Some(r(&mode.diff_removed.hex())),
                context: Some(r(&mode.diff_context.hex())),
                hunk_header: Some(r(&mode.diff_hunk_header.hex())),
            }),
            background: Some(DiffBackgroundDef {
                added: Some(r(&mode.diff_added_bg.hex())),
                removed: Some(r(&mode.diff_removed_bg.hex())),
                context: Some(r(&mode.diff_context_bg.hex())),
            }),
            highlight: Some(DiffHighlightDef {
                added: Some(r(&mode.diff_highlight_added.hex())),
                removed: Some(r(&mode.diff_highlight_removed.hex())),
            }),
            line_number: Some(DiffLineNumberDef {
                text: Some(r(&mode.diff_line_number.hex())),
                background: Some(DiffHighlightDef {
                    added: Some(r(&mode.diff_added_line_number_bg.hex())),
                    removed: Some(r(&mode.diff_removed_line_number_bg.hex())),
                }),
            }),
        }),
        // All nine v2 syntax tokens have a v1 counterpart with the same name, and upstream's
        // migration copies every one of them (`syntax: { comment: color("syntaxComment"), … }`).
        syntax: Some(SyntaxDef {
            comment: Some(r(&mode.syntax.comment.hex())),
            keyword: Some(r(&mode.syntax.keyword.hex())),
            function: Some(r(&mode.syntax.function.hex())),
            variable: Some(r(&mode.syntax.variable.hex())),
            string: Some(r(&mode.syntax.string.hex())),
            number: Some(r(&mode.syntax.number.hex())),
            type_: Some(r(&mode.syntax.type_.hex())),
            operator: Some(r(&mode.syntax.operator.hex())),
            punctuation: Some(r(&mode.syntax.punctuation.hex())),
        }),
        // Same shape for markdown: 14 v1 slots, 14 v2 tokens, one for one.
        markdown: Some(MarkdownDef {
            text: Some(r(&mode.markdown.text.hex())),
            heading: Some(r(&mode.markdown.heading.hex())),
            link: Some(r(&mode.markdown.link.hex())),
            link_text: Some(r(&mode.markdown.link_text.hex())),
            code: Some(r(&mode.markdown.code.hex())),
            block_quote: Some(r(&mode.markdown.block_quote.hex())),
            emphasis: Some(r(&mode.markdown.emphasis.hex())),
            strong: Some(r(&mode.markdown.strong.hex())),
            horizontal_rule: Some(r(&mode.markdown.horizontal_rule.hex())),
            list_item: Some(r(&mode.markdown.list_item.hex())),
            list_enumeration: Some(r(&mode.markdown.list_enumeration.hex())),
            image: Some(r(&mode.markdown.image.hex())),
            image_text: Some(r(&mode.markdown.image_text.hex())),
            code_block: Some(r(&mode.markdown.code_block.hex())),
        }),
        dialog: Some(DialogDef {
            text: None,
            background: Some(BackgroundDef {
                base: Some(r("$background.raised.base")),
                action: Some(VariantsDef {
                    primary: Some(StatefulDef {
                        hovered: Some(r("$background.raised.high")),
                        ..StatefulDef::default()
                    }),
                    ..VariantsDef::default()
                }),
                ..BackgroundDef::default()
            }),
        }),
        mode: if dark { Mode::Dark } else { Mode::Light },
    })
}

fn hue_alias(inferred: &Inferred, token: &str) -> String {
    match inferred.by_token.get(token) {
        Some(name) => format!("$hue.{name}"),
        None => "$hue.gray".to_string(),
    }
}

fn feedback_text(color: &str) -> FeedbackTextDef {
    FeedbackTextDef { base: Some(r(color)), muted: None }
}

fn feedback_background(color: &str) -> super::FeedbackBackgroundDef {
    super::FeedbackBackgroundDef { base: Some(r(color)) }
}

fn r(value: &str) -> Ref {
    Ref(value.to_string())
}

fn alias(value: String) -> HueValue {
    HueValue::Alias(Ref(value))
}

impl From<HashMap<String, HueValue>> for ModeDef {
    fn from(hue: HashMap<String, HueValue>) -> Self {
        ModeDef { hue: Some(hue), ..ModeDef::default() }
    }
}

// ---------------------------------------------------------------------------------------
// Hue inference
// ---------------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Inferred {
    by_hue: HashMap<ChromaticHue, HueMatch>,
    by_token: HashMap<&'static str, ChromaticHue>,
}

#[derive(Debug, Clone, Copy)]
struct HueMatch {
    color: Rgb,
    distance: f64,
}

fn infer_hues(mode: &V1Mode) -> Inferred {
    let mut result = Inferred::default();
    for (token, color) in [
        ("accent", mode.accent),
        ("success", mode.success),
        ("warning", mode.warning),
        ("primary", mode.primary),
        ("error", mode.error),
        ("info", mode.info),
        ("secondary", mode.secondary),
    ] {
        if let Some(nearest) = infer_hue(color) {
            // `>=` keeps the earlier token: upstream only replaces a hue when the new colour is
            // strictly closer.
            let replace = match result.by_hue.get(nearest.name) {
                Some(current) => current.distance > nearest.distance,
                None => true,
            };
            if replace {
                result.by_hue.insert(nearest.name, HueMatch { color, distance: nearest.distance });
            }
            result.by_token.insert(token, nearest.name);
        }
    }
    // Upstream re-runs the accent/primary pair and overwrites unconditionally. This is what
    // makes `accent` win when both infer to the same hue.
    for (token, color) in [("accent", mode.accent), ("primary", mode.primary)] {
        if let Some(nearest) = infer_hue(color) {
            result.by_hue.insert(nearest.name, HueMatch { color, distance: nearest.distance });
            result.by_token.insert(token, nearest.name);
        }
    }
    result
}

fn infer_hue(color: Rgb) -> Option<HueRef> {
    let value = rgb_to_oklch(color);
    // `ambiguous`: a fully transparent or near-grey colour names no hue.
    if color.a == 0 || value.c < MINIMUM_CHROMA {
        return None;
    }
    let light = value.l >= LIGHT_THRESHOLD;
    CHROMATIC_HUES
        .iter()
        .map(|name| HueRef { name, distance: hue_distance(value.h, hue_angle(light, name)) })
        .min_by(|first, second| {
            first
                .distance
                .partial_cmp(&second.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

#[derive(Debug, Clone, Copy)]
struct HueRef {
    name: ChromaticHue,
    distance: f64,
}

fn hue_distance(first: f64, second: f64) -> f64 {
    let difference = (first - second).abs();
    difference.min(360.0 - difference)
}

fn hue_angle(light: bool, name: ChromaticHue) -> f64 {
    let hex = HUE_REFERENCES
        .iter()
        .find(|(is_light, hue, _)| *is_light == light && *hue == name)
        .map(|(_, _, hex)| *hex)
        .expect("every chromatic hue has a light and a dark reference");
    rgb_to_oklch(Rgb::from_hex(hex).expect("reference swatches are hex literals")).h
}

/// `hueScale`: the source colour becomes step 200 and the scale fades toward the mode's
/// endpoint, losing half its chroma by step 900.
fn hue_scale(color: Rgb, dark: bool) -> HashMap<String, Ref> {
    let value = rgb_to_oklch(color);
    let anchor = 200.0_f64;
    let endpoint = if dark { value.l.min(0.18) } else { value.l.max(0.97) };
    let alpha = color.a;
    let mut scale = HashMap::new();
    for step in HUE_STEPS {
        let generated = if step == 200 {
            color.hex()
        } else {
            let progress = (f64::from(step) - anchor) / (900.0 - anchor);
            let hex = oklch_to_hex(crate::color::Oklch {
                l: value.l + (endpoint - value.l) * progress,
                c: value.c * (1.0 - progress * 0.5),
                h: value.h,
            });
            if alpha == 255 { hex } else { format!("{hex}{alpha:02x}") }
        };
        scale.insert(step.to_string(), Ref(generated));
    }
    scale
}

/// `neutralAnchors` + `neutralScale`. The light anchors are written dark-to-light by upstream
/// and then flipped through `1000 - step`, which is what puts `#0a0a0a` at 800 and `#ffffff`
/// at 100 in dark mode.
fn neutral_scale(mode: &V1Mode) -> HashMap<String, Ref> {
    let anchors = neutral_anchors(mode);
    let mut scale = HashMap::new();
    for step in HUE_STEPS {
        let color = match anchors.iter().find(|anchor| anchor.step == step) {
            Some(anchor) => anchor.color.hex(),
            None => {
                let first = anchors[0];
                let last = anchors[anchors.len() - 1];
                let (lower, upper) = if step < first.step {
                    (first, anchors[1])
                } else if step > last.step {
                    (anchors[anchors.len() - 2], last)
                } else {
                    let lower = *anchors
                        .iter()
                        .rev()
                        .find(|anchor| anchor.step < step)
                        .expect("a step between anchors has a lower neighbour");
                    let upper = *anchors
                        .iter()
                        .find(|anchor| anchor.step > step)
                        .expect("a step between anchors has an upper neighbour");
                    (lower, upper)
                };
                // Signed on purpose: step 100 sits *below* the lowest anchor (200), and upstream
                // extrapolates there rather than clamping (`(100 - 200) / (400 - 200)` = -0.5).
                // With u16 this subtraction underflowed — a panic in debug, and a wrapped
                // ~327x extrapolation in release — on every migrated theme, because every theme
                // anchors 200..800 and `HUE_STEPS` starts at 100.
                let amount = f64::from(step as i32 - lower.step as i32)
                    / f64::from(upper.step as i32 - lower.step as i32);
                interpolate_oklch(lower.color, upper.color, amount)
            }
        };
        scale.insert(step.to_string(), Ref(color));
    }
    scale
}

#[derive(Debug, Clone, Copy)]
struct Anchor {
    step: u16,
    color: Rgb,
}

fn neutral_anchors(mode: &V1Mode) -> Vec<Anchor> {
    let light = [
        Anchor { step: 200, color: mode.background },
        Anchor { step: 300, color: mode.background_panel },
        // See `V1Mode::background_element`: the fallback is for a missing key, not for a
        // transparent value.
        Anchor { step: 400, color: mode.background_element.unwrap_or(mode.background_menu) },
        Anchor { step: 600, color: mode.text_muted },
        Anchor { step: 800, color: mode.text },
    ];
    light
        .iter()
        .rev()
        .map(|anchor| Anchor { step: 1000 - anchor.step, color: anchor.color })
        .collect()
}

/// `selectedForeground`: v1's own `selectedListItemText` when the file set one, otherwise the
/// background — but only when the background is not fully transparent.
fn selected_foreground(mode: &V1Mode, background: Rgb) -> Rgb {
    if mode.has_selected_list_item_text {
        return mode.selected_list_item_text;
    }
    if mode.background.has_alpha() {
        return mode.background;
    }
    // `Rgb::luminance` is the 0..1 scale the 0.5 threshold assumes; upstream reads the
    // normalised `RGBA.r/g/b` floats here. Feeding it raw bytes (as this did first) makes every
    // colour except pure black "bright" and always picks black.
    if background.luminance() > 0.5 { Rgb::opaque(0, 0, 0) } else { Rgb::opaque(255, 255, 255) }
}
