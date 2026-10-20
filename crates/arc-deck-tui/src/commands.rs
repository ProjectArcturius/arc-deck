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
    Sessions,
    Rename,
    Runs,
    Cancel,
    Latest,
    Tools,
    Usage,
    Evidence,
    /// 会话内容检索（`session/search`）。
    Search,
    /// 本会话的用户消息时间线。
    Timeline,
    /// 本会话派出的子 agent 会话（与回到父会话）。
    Subagents,
    /// 暂存当前草稿；输入框为空时列出暂存的草稿取回。
    Stash,
    /// 还有哪几条审批在等回答。
    Approvals,
    /// 本会话的 diff。
    Diff,
    /// 某个工具的完整输出。
    Output,
    /// 复制最后一条回复（OSC 52）。
    Copy,
    /// 内核起不来 / 掉了之后重新拉起。
    Reconnect,
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
    /// Argument hint shown by completion and help. `<…>` = **required** (`/rename <标题>`),
    /// `[…]` = optional (`/permission [级别]` without one opens the picker). The distinction
    /// decides whether Enter on a completion runs the command or waits for the argument —
    /// see [`Entry::arg_required`].
    pub arg: Option<&'static str>,
    pub keys: &'static [&'static str],
    pub context: Context,
    /// Build-level absence, with the reason the user is shown.
    pub unavailable: Option<&'static str>,
    /// Whether the command palette lists it. Dialog-local actions are reachable only in their
    /// dialog; listing them in the palette would promise something the palette cannot do.
    pub palette: bool,
}

impl Entry {
    /// 不带参数就没法执行（`/rename`、`/search`）。补全时 Enter 对这类命令只填进去等参数；
    /// 其余命令 Enter 直接执行 —— `/per` + Enter 就是 `/permission`，不该再按一次（用户 2026-10-20）。
    pub(crate) fn arg_required(&self) -> bool {
        self.arg.is_some_and(|arg| arg.starts_with('<'))
    }
}

/// The authorization levels (§5.6), with the picker's description.
///
/// `readonly` became selectable once the M3 live negatives proved the execution layer refuses
/// writes and commands under it (docs/M3_GUARD.md); its known gap — reads are not confined to
/// the workspace — is in the description rather than hidden.
///
/// `confirm` is selectable now that the approval channel is wired (M4 w4-tui-modes):
/// the kernel asks over `$events` and the TUI answers. It shares `workspace`'s sandbox,
/// so the only difference is that the kernel waits for a person before acting.
pub(crate) const PERMISSIONS: [(&str, &str, Option<&str>); 4] = [
    ("readonly", "只读：执行层拒绝写文件（读不限于工作区，见 M3_GUARD）", None),
    ("workspace", "工作区：可读写当前工作区", None),
    ("full", "完全：不受沙箱限制（--worktree 必需）", None),
    ("confirm", "确认：与 workspace 同权限，但内核问「能不能做」时要你回答", None),
];

pub(crate) const ENTRIES: &[Entry] = &[
    Entry {
        action: Action::New,
        id: "session.new",
        title: "新会话",
        category: Category::Session,
        slash: Some("/new"),
        arg: None,
        keys: &["<leader>n"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    // `session.list` 这个 id 留给会话列表：用户的 keybinds.json 里绑的是「列表」这个意思，
    // 而 M4a 那时还没有会话，只能先拿工人运行顶着。工人运行改用新 id。
    Entry {
        action: Action::Sessions,
        id: "session.list",
        title: "会话列表（当前工作区）",
        category: Category::Session,
        slash: Some("/sessions"),
        arg: None,
        keys: &["<leader>l"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Rename,
        id: "session.rename",
        title: "改名当前会话",
        category: Category::Session,
        slash: Some("/rename"),
        arg: Some("<标题>"),
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Runs,
        id: "runs.list",
        title: "工人运行（当前工作区）",
        category: Category::Session,
        slash: Some("/runs"),
        arg: None,
        keys: &[],
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
        action: Action::Search,
        id: "session.search",
        title: "检索会话内容",
        category: Category::Session,
        slash: Some("/search"),
        arg: Some("<词>"),
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Timeline,
        id: "session.timeline",
        title: "跳到本会话的某条消息",
        category: Category::Session,
        slash: Some("/timeline"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Subagents,
        id: "session.subagents",
        title: "子 agent 会话（打开 / 回到父会话）",
        category: Category::Session,
        slash: Some("/subagents"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Stash,
        id: "prompt.stash",
        title: "暂存草稿（输入框为空时取回）",
        category: Category::Session,
        slash: Some("/stash"),
        arg: None,
        keys: &["<leader>s"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Approvals,
        id: "session.approvals",
        title: "查看待答的审批",
        category: Category::Session,
        slash: Some("/approvals"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Diff,
        id: "session.diff",
        title: "本会话改动的 diff",
        category: Category::Session,
        slash: Some("/diff"),
        arg: None,
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Output,
        id: "session.output",
        title: "工具的完整输出",
        category: Category::Session,
        slash: Some("/output"),
        arg: Some("[序号，默认最后一条]"),
        keys: &[],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Copy,
        id: "session.copy",
        title: "复制最后一条回复（OSC 52）",
        category: Category::Session,
        slash: Some("/copy"),
        arg: None,
        keys: &["ctrl+y"],
        context: Context::Composer,
        unavailable: None,
        palette: true,
    },
    Entry {
        action: Action::Reconnect,
        id: "backend.reconnect",
        title: "重启交互内核（掉线后重连）",
        category: Category::Session,
        slash: Some("/reconnect"),
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
        arg: Some("[级别|default]"),
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
        arg: Some("[readonly|workspace|full|confirm]"),
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
        unavailable: None,
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
            Action::Sessions,
            Action::Rename,
            Action::Runs,
            Action::Cancel,
            Action::Latest,
            Action::Tools,
            Action::Usage,
            Action::Evidence,
            Action::Search,
            Action::Timeline,
            Action::Subagents,
            Action::Stash,
            Action::Approvals,
            Action::Diff,
            Action::Output,
            Action::Copy,
            Action::Reconnect,
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
            "/rename",
            "/runs",
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
            "/search",
            "/timeline",
            "/diff",
            "/output",
            "/help",
            "/exit",
        ] {
            let entry = by_slash(slap).unwrap_or_else(|| panic!("{slap} is missing"));
            assert_eq!(entry.slash, Some(slap));
        }
        // Plan 与 confirm 都接通了（M4 w4-tui-modes）：前者落到执行层只读，
        // 后者走 `$events` 的审批通道。没有哪一条还该挂着「不可用」。
        assert!(by_slash("/plan").unwrap().unavailable.is_none());
        assert!(PERMISSIONS.iter().any(|(name, _, _)| *name == "confirm"));
        assert!(PERMISSIONS.iter().all(|(_, _, reason)| reason.is_none()), "四档都可用");
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
