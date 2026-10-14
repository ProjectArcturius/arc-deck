//! Built-in theme registry.
//! Copyright (c) 2025 opencode. MIT notice: ../LICENSE-OpenCode.
//!
//! The JSON under `crates/arc-deck-tui/themes/` is copied verbatim from OpenCode v2.0.27
//! (`packages/tui/src/theme/assets/`); see `themes/README.md` for provenance. `include_str!`
//! keeps the TUI a single binary — §7.1 rules out shipping a second runtime, and a theme that
//! has to be found on disk at startup would be a deployment dependency.
//!
//! `v1: true` means the file is a flat v1 palette and must go through `theme/v1.rs`. Only
//! `v2/opencode.json` is a v2 document; everything else in `assets/` is v1, which is exactly
//! how upstream splits them (`packages/tui/src/theme/v1.ts` vs `assets/v2/opencode.json`).

pub(crate) struct ThemeInfo {
    /// Upstream asset file name without `.json`.
    pub name: &'static str,
    pub json: &'static str,
    /// Flat v1 palette needing migration, as opposed to a v2 semantic document.
    pub v1: bool,
}

/// OpenCode's default theme name. §7.3 asks for the OpenCode default in dark mode.
pub(crate) const DEFAULT_THEME: &str = "opencode";

macro_rules! v1_theme {
    ($name:literal) => {
        ThemeInfo {
            name: $name,
            json: include_str!(concat!("../../themes/", $name, ".json")),
            v1: true,
        }
    };
}

/// Sorted by name so the dialog's list is stable without a runtime sort. `localeCompare` with
/// `sensitivity: "base"` upstream is equivalent to a case-insensitive compare here because
/// every built-in name is already lowercase ASCII.
pub(crate) const THEMES: &[ThemeInfo] = &[
    v1_theme!("aura"),
    v1_theme!("ayu"),
    v1_theme!("carbonfox"),
    v1_theme!("catppuccin"),
    v1_theme!("catppuccin-frappe"),
    v1_theme!("catppuccin-macchiato"),
    v1_theme!("cobalt2"),
    v1_theme!("cursor"),
    v1_theme!("dracula"),
    v1_theme!("everforest"),
    v1_theme!("flexoki"),
    v1_theme!("github"),
    v1_theme!("gruvbox"),
    v1_theme!("kanagawa"),
    v1_theme!("lucent-orng"),
    v1_theme!("material"),
    v1_theme!("matrix"),
    v1_theme!("mercury"),
    v1_theme!("monokai"),
    v1_theme!("nightowl"),
    v1_theme!("nord"),
    v1_theme!("one-dark"),
    ThemeInfo {
        name: "opencode",
        json: include_str!("../../themes/v2/opencode.json"),
        v1: false,
    },
    v1_theme!("orng"),
    v1_theme!("osaka-jade"),
    v1_theme!("palenight"),
    v1_theme!("rosepine"),
    v1_theme!("solarized"),
    v1_theme!("synthwave84"),
    v1_theme!("tokyonight"),
    v1_theme!("vercel"),
    v1_theme!("vesper"),
    v1_theme!("zenburn"),
];

pub(crate) fn find(name: &str) -> Option<&'static ThemeInfo> {
    THEMES.iter().find(|theme| theme.name == name)
}

pub(crate) fn theme_names() -> Vec<&'static str> {
    THEMES.iter().map(|theme| theme.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `include_str!` fails the build on a missing file, but nothing catches a theme that was
    /// added to `themes/` and forgotten here — that would silently be unreachable in the UI.
    ///
    /// Two names are in the directory but not in the registry, by design: `assets/opencode.json`
    /// is the v1 palette that upstream superseded with `assets/v2/opencode.json` (both are still
    /// in the tag, and `packages/tui/src/theme/v1.ts` still ships the v1 file), and `v2/` is a
    /// directory. The registry is the source of truth for what the UI can offer.
    #[test]
    fn every_usable_asset_file_is_registered() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                if !entry.file_type().unwrap().is_file() {
                    return None;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                name.strip_suffix(".json").map(str::to_string)
            })
            .filter(|name| name != "opencode")
            .collect();
        on_disk.push("opencode".into());
        on_disk.sort();
        let mut registered: Vec<String> = theme_names().into_iter().map(str::to_string).collect();
        registered.sort();
        assert_eq!(on_disk, registered);
    }

    #[test]
    fn theme_names_are_unique_and_the_default_exists() {
        let mut names = theme_names();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate theme name in the registry");
        assert!(find(DEFAULT_THEME).is_some());
    }
}
