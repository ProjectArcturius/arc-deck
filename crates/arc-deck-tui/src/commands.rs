//! The one registry behind the palette, slash commands, key bindings, help and which-key.
//!
//! A second table of titles or key labels is exactly how the TUI drifts: the palette says
//! "命令", the footer says `^P`, and the help page is written by hand a month later. Every
//! surface iterates `ENTRIES` instead. `keymap` owns the *parsed* bindings (users can override
//! them), this module owns the *defaults* and the copy.
//!
//! Unavailable is data, not omission: §7.4 says a capability the backend does not have yet must
//! stay visible with a reason. `Entry::unavailable` is for the build-level facts (Plan has no
//! read-only execution path at all); state-dependent reasons (nothing is running, no run is
//! selected) are `App::availability` in `app.rs`, because only the app knows them.

/// Every user-visible action. `keymap::Keymap` binds them, `app::App::execute` performs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    Palette,
    New,
    Runs,
    Cancel,
    Latest,
    Tools,
    Usage,
    Evidence,
    Agents,
    Models,
    Effort,
    CycleEffort,
    Permission,
    Plan,
    Build,
    Theme,
    Mode,
    Sidebar,
    Edit,
    PasteSummary,
    Help,
    Quit,
    /// Dialog-local: model dialog `Ctrl+F`.
    Favorite,
    /// Dialog-local: model dialog `Ctrl+A`.
    ProviderList,
    CycleModelNext,
    CycleModelReverse,
    CycleAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Category {
    Session,
    Model,
    View,
    Help,
}

impl Category {
    pub(crate) const ALL: [Category; 4] =
        [Category::Session, Category::Model, Category::View, Category::Help];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Category::Session => "会话",
            Category::Model => "Agent 与模型",
            Category::View => "界面",
            Category::Help => "帮助",
        }
    }
}

/// Where a binding is allowed to fire. Dialog-local bindings shadow the composer ones with the
/// same physical key (§7.4: Ctrl+F / Ctrl+A mean something else inside the model dialog).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    /// Fires even with a dialog open (quit, palette).
    Anywhere,
    /// Only while the composer has focus and no dialog is open.
    Composer,
    ModelDialog,
}

pub(crate) struct Entry {
    pub action: Action,
    /// Stable id for `keybinds.json`. Never rename casually: it is the user's file format.
    pub id: &'static str,
    pub title: &'static str,
    pub category: Category,
    pub slash: Option<&'static str>,
    /// Argument hint shown by completion and help, e.g. `<workspace|full>`.
    pub arg: Option<&'static str>,
    pub keys: &'static [&'static str],
    pub context: Context,
    /// Build-level absence, with the reason the user is shown.
    pub unavailable: Option<&'static str>,
    /// Whether the command palette lists it. Dialog-local actions are reachable only in their
    /// dialog; listing them in the palette would promise something the palette cannot do.
    pub palette: bool,
}

pub(crate) const PLAN_REASON: &str =
    "Plan 需要只读执行限制与确认应答者，本版只有 workspace / full，提示词不能保证只读";

/// Why `readonly` cannot be selected yet. Kept next to the registry because both `/permission`
/// and the picker row print it — one string, two surfaces.
pub(crate) const READONLY_REASON: &str =
    "只读授权需要 DSH 只读工具集与确认应答者，本版尚未接通；请用 workspace 或 full";

/// The authorization levels (§5.6), with the picker's description and, for the level the
/// execution layer cannot honour yet, the reason. Order is the order the picker shows.
pub(crate) const PERMISSIONS: [(&str, &str, Option<&str>); 3] = [
    ("readonly", "只读：不写文件、不执行命令", Some(READONLY_REASON)),
    ("workspace", "工作区：可读写当前工作区", None),
    ("full", "完全：不受沙箱限制（--worktree 必需）", None),
];

pub(crate) const ENTRIES: &[Entry] = &[
    Entry {
        action: Action::New,
        id: "session.new",
        title: "新的独立请求",
        category: Category::Session,
        slash: Some("/new"),
        arg: None,
        keys: &["<leader>n"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Runs,
        id: "session.list",
        title: "运行历史（当前工作区）",
        category: Category::Session,
        slash: Some("/sessions"),
        arg: None,
        keys: &["<leader>l"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Cancel,
        id: "session.cancel",
        title: "取消当前运行",
        category: Category::Session,
        slash: Some("/cancel"),
        arg: None,
        keys: &["escape"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Latest,
        id: "session.latest",
        title: "回到最新",
        category: Category::Session,
        slash: Some("/latest"),
        arg: None,
        keys: &["ctrl+end"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Tools,
        id: "session.tools",
        title: "展开 / 折叠工具记录",
        category: Category::Session,
        slash: Some("/tools"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Usage,
        id: "session.usage",
        title: "查看 provider 报告的用量",
        category: Category::Session,
        slash: Some("/usage"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Evidence,
        id: "session.evidence",
        title: "查看完整证据位置",
        category: Category::Session,
        slash: Some("/evidence"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Agents,
        id: "agent.list",
        title: "选择 Agent",
        category: Category::Model,
        slash: Some("/agents"),
        arg: None,
        keys: &["<leader>a"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Models,
        id: "model.list",
        title: "选择模型（下一次请求生效）",
        category: Category::Model,
        slash: Some("/models"),
        arg: None,
        keys: &["<leader>m"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Effort,
        id: "model.effort",
        title: "选择模型强度（effort）",
        category: Category::Model,
        slash: Some("/effort"),
        arg: Some("<级别|default>"),
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::CycleEffort,
        id: "model.effort.cycle",
        title: "循环模型强度（effort）",
        category: Category::Model,
        slash: None,
        arg: None,
        keys: &["ctrl+t"],
        context: Context::Composer,
        unavailable: None,
        palette: false,
    },
    Entry {
        action: Action::Permission,
        id: "permission.set",
        title: "授权级别（下一次请求生效）",
        category: Category::Model,
        slash: Some("/permission"),
        arg: Some("<readonly|workspace|full>"),
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Plan,
        id: "agent.plan",
        title: "切到 Plan（只读规划）",
        category: Category::Model,
        slash: Some("/plan"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: Some(PLAN_REASON),
        palette: true,
    },
    Entry {
        action: Action::Build,
        id: "agent.build",
        title: "切到 Build（实施）",
        category: Category::Model,
        slash: Some("/build"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Theme,
        id: "theme.list",
        title: "选择主题",
        category: Category::View,
        slash: Some("/theme"),
        arg: None,
        keys: &["<leader>t"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Mode,
        id: "theme.mode",
        title: "切换深 / 浅色",
        category: Category::View,
        slash: Some("/mode"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Sidebar,
        id: "sidebar.toggle",
        title: "切换信息栏",
        category: Category::View,
        slash: Some("/sidebar"),
        arg: None,
        keys: &["<leader>b"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Edit,
        id: "input.editor",
        title: "用外部编辑器编辑输入",
        category: Category::View,
        slash: None,
        arg: None,
        keys: &["<leader>e"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::PasteSummary,
        id: "input.paste",
        title: "切换大段粘贴摘要",
        category: Category::View,
        slash: Some("/paste"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Help,
        id: "help.show",
        title: "帮助：按键与命令",
        category: Category::Help,
        slash: Some("/help"),
        arg: None,
        keys: &["f1", "<leader>?"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Quit,
        id: "app.exit",
        title: "退出（取消本次 TUI 发起的活动）",
        category: Category::Help,
        slash: Some("/exit"),
        arg: None,
        keys: &["ctrl+c", "ctrl+d", "<leader>q"],
        context: Context::Anywhere,
        unavailable: None,
        palette: true,
    },
    // Not a slash command and not in the palette: the palette is how you find commands, so
    // listing "open the palette" there is noise. The key still has to come from the registry.
    Entry {
        action: Action::Palette,
        id: "command.palette",
        title: "命令面板",
        category: Category::Help,
        slash: None,
        arg: None,
        keys: &["ctrl+p"],
        context: Context::Anywhere,
        unavailable: None,
        palette: false,
    },
    Entry {
        action: Action::Favorite,
        id: "model.favorite",
        title: "收藏 / 取消收藏当前模型",
        category: Category::Model,
        slash: None,
        arg: None,
        keys: &["ctrl+f"],
        context: Context::ModelDialog,
        unavailable: None,
        palette: false,
    },
    Entry {
        action: Action::ProviderList,
        id: "model.provider",
        title: "从模型对话框进入 provider 列表",
        category: Category::Model,
        slash: None,
        arg: None,
        keys: &["ctrl+a"],
        context: Context::ModelDialog,
        unavailable: None,
        palette: false,
    },
    Entry {
        action: Action::CycleModelNext,
        id: "model.cycle.next",
        title: "下一个最近使用的模型",
        category: Category::Model,
        slash: None,
        arg: None,
        keys: &["f2"],
        context: Context::Composer,
        unavailable: None,
        palette: false,
    },
    Entry {
        action: Action::CycleModelReverse,
        id: "model.cycle.reverse",
        title: "上一个最近使用的模型",
        category: Category::Model,
        slash: None,
        arg: None,
        keys: &["shift+f2"],
        context: Context::Composer,
        unavailable: None,
        palette: false,
    },
    Entry {
        action: Action::CycleAgent,
        id: "agent.cycle",
        title: "切换 Agent",
        category: Category::Model,
        slash: None,
        arg: None,
        keys: &["shift+tab"],
        context: Context::Composer,
        unavailable: None,
        palette: false,
    },
];

/// Panics only when a variant was added without a registry row, which `every_action_is_registered`
/// catches before anything ships. Callers therefore never deal with `Option<&Entry>`.
pub(crate) fn entry(action: Action) -> &'static Entry {
    ENTRIES
        .iter()
        .find(|entry| entry.action == action)
        .expect("every Action has an Entry")
}

pub(crate) fn by_slash(name: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.slash == Some(name))
}

pub(crate) fn palette_entries() -> impl Iterator<Item = &'static Entry> {
    ENTRIES.iter().filter(|entry| entry.palette)
}

/// `"/effort high"` → the `/effort` row plus `"high"`. `None` for an unknown command, which the
/// caller reports rather than guessing a prefix match.
pub(crate) struct Parsed {
    pub entry: &'static Entry,
    pub arg: Option<String>,
}

pub(crate) fn parse(input: &str) -> Option<Parsed> {
    let input = input.trim();
    if !input.starts_with('/') {
        return None;
    }
    let (name, rest) = match input.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (input, ""),
    };
    let entry = by_slash(name)?;
    Some(Parsed {
        entry,
        arg: (!rest.is_empty()).then(|| rest.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The palette test walks actions; a variant without a row would silently never be reachable.
    #[test]
    fn every_action_is_registered() {
        let actions: HashSet<Action> = ENTRIES.iter().map(|entry| entry.action).collect();
        assert_eq!(actions.len(), ENTRIES.len(), "two entries share one action");
        for action in [
            Action::Palette,
            Action::New,
            Action::Runs,
            Action::Cancel,
            Action::Latest,
            Action::Tools,
            Action::Usage,
            Action::Evidence,
            Action::Agents,
            Action::Models,
            Action::Effort,
            Action::CycleEffort,
            Action::Permission,
            Action::Plan,
            Action::Build,
            Action::Theme,
            Action::Mode,
            Action::Sidebar,
            Action::Edit,
            Action::PasteSummary,
            Action::Help,
            Action::Quit,
            Action::Favorite,
            Action::ProviderList,
            Action::CycleModelNext,
            Action::CycleModelReverse,
            Action::CycleAgent,
        ] {
            assert!(actions.contains(&action), "{action:?} has no registry row");
        }
    }

    /// §7.4 names the first-release commands. They must all be present, and only the ones the
    /// backend cannot do yet may carry a reason.
    #[test]
    fn the_spec_commands_are_all_registered() {
        for slap in [
            "/new",
            "/sessions",
            "/agents",
            "/models",
            "/effort",
            "/permission",
            "/plan",
            "/build",
            "/cancel",
            "/usage",
            "/theme",
            "/mode",
            "/sidebar",
            "/tools",
            "/latest",
            "/evidence",
            "/help",
            "/exit",
        ] {
            let entry = by_slash(slap).unwrap_or_else(|| panic!("{slap} is missing"));
            assert_eq!(entry.slash, Some(slap));
        }
        assert!(by_slash("/plan").unwrap().unavailable.is_some(), "Plan has no read-only path");
    }

    #[test]
    fn ids_and_slashes_are_unique() {
        let mut ids = HashSet::new();
        let mut slashes = HashSet::new();
        for entry in ENTRIES {
            assert!(ids.insert(entry.id), "duplicate id {}", entry.id);
            if let Some(slash) = entry.slash {
                assert!(slashes.insert(slash), "duplicate slash {slash}");
            }
        }
    }

    #[test]
    fn slash_parsing_keeps_the_argument() {
        let parsed = parse("  /effort high  ").unwrap();
        assert_eq!(parsed.entry.action, Action::Effort);
        assert_eq!(parsed.arg.as_deref(), Some("high"));

        let bare = parse("/effort").unwrap();
        assert_eq!(bare.arg, None);

        assert!(parse("/nope").is_none());
        assert!(parse("hello").is_none(), "plain text is a prompt, not a command");
        assert!(parse("/").is_none());
    }

    /// Titles must not drift between surfaces; this is the same reason the registry exists.
    #[test]
    fn every_palette_row_has_a_title_and_a_slash_or_a_key() {
        for entry in palette_entries() {
            assert!(!entry.title.is_empty(), "{} has no title", entry.id);
            assert!(
                entry.slash.is_some() || !entry.keys.is_empty(),
                "{} is in the palette but can never be typed or pressed",
                entry.id
            );
        }
    }
}
