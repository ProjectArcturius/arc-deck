use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arc_deck_core::conversation::{Conversation, Message, ModelCatalog, ModelChoice, SessionSummary};
use arc_deck_core::service::{self, BackendState, Command, NoticeLevel};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::autocomplete::{self, Autocomplete};
use crate::catalog::{self, ModelPrefs, ModelRef};
use crate::commands::{self, Action, Category, Entry};
use crate::keymap::{self, Focus, Keymap};
use crate::files::FileIndex;
use crate::model_dialog::{self, EffortDialogState, ModelDialogState, ProviderDialogState};
use crate::{
    Boot, Preferences, Request, RunDetail, RunRecord, StartRequest, Update,
    editor::Editor,
    theme::{DEFAULT_THEME, Theme},
    theme_dialog::{ThemeDialog, ThemeDialogAction},
};

/// 弹层。带状态的那几个（模型 / provider / effort）的渲染与选择规则都在
/// `model_dialog` 里，这里只保留「当前开着哪个」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Dialog {
    Commands,
    Sessions,
    Runs,
    Agents,
    Permissions,
    Help,
    Models(ModelDialogState),
    Providers(ProviderDialogState),
    Efforts(EffortDialogState),
    Theme,
}

/// One `/help` row: the chord(s) the keymap actually binds, the registry title, where the
/// binding applies, and why the action cannot run in this build (`None` when it can).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HelpRow {
    pub category: Category,
    pub keys: String,
    pub title: String,
    pub context: commands::Context,
    pub reason: Option<String>,
}

pub(crate) struct App {
    pub boot: Boot,
    pub preferences: Preferences,
    /// Resolved theme tokens. Re-resolved only when the theme name or mode changes, never per
    /// frame: `Theme::load` parses and resolves the whole document, and the draw path runs on
    /// every keystroke.
    pub theme: Theme,
    pub editor: Editor,
    pub runs: Vec<RunRecord>,
    pub selected: Option<String>,
    pub detail: Option<Box<RunDetail>>,
    pub active: Option<String>,
    pub agent: String,
    pub permission: String,
    pub effort: Option<String>,
    /// 下一次请求要用的模型。启动时来自配置，模型对话框改动它。
    pub model: Option<ModelRef>,
    pub model_prefs: ModelPrefs,
    /// 收藏 / 最近 / effort 改过就置位；退出时按它决定要不要落盘。
    pub model_prefs_dirty: bool,
    pub dialog: Option<Dialog>,
    /// Non-`None` while the theme picker is open; it owns that dialog's filter and cursor.
    pub theme_dialog: Option<ThemeDialog>,
    pub filter: String,
    pub choice: usize,
    /// Parsed defaults plus the user's `keybinds.json`. Every key the composer reacts to comes
    /// from here, so the help page and the footer cannot disagree with what actually fires.
    pub keymap: Keymap,
    /// Completion popup over the composer; `None` when there is nothing to complete.
    pub autocomplete: Option<Autocomplete>,
    /// Set while the leader chord is waiting for its second key. A deadline, not a bool: a
    /// forgotten leader that never expired would swallow the next unrelated keystroke.
    leader_until: Option<Instant>,
    /// Workspace file list for `@` references. Loaded lazily on the first `@` — a `git ls-files`
    /// on every keystroke would make the popup feel broken in a large repository.
    pub files: FileIndex,
    /// Set by the editor action; `run` owns the terminal and performs the suspend/resume.
    pending_editor: bool,
    /// First visible help row. Help is longer than a 40×12 terminal allows, so it scrolls.
    pub help_scroll: usize,
    pub scroll: Option<usize>,
    pub scroll_max: usize,
    pub sidebar_overlay: bool,
    pub expanded: HashMap<String, bool>,
    pub notice: String,
    pub dirty: bool,
    serial: u64,
    /// 交互内核的状态。`Failed` 时提交退回独立运行（headless 那条路），界面说清楚为什么。
    pub backend: BackendState,
    /// 当前会话。`None` 且 `pending` 有值 = 新会话正在建。
    pub session_id: Option<String>,
    pub conversation: Option<Conversation>,
    /// 已发出、还没在会话视图里出现的那条（乐观回显）。看到同样的用户消息就清掉。
    pub pending: Option<String>,
    pub sessions: Vec<SessionSummary>,
}

impl App {
    pub fn new(boot: Boot, preferences: Preferences, model_prefs: ModelPrefs) -> Self {
        let (theme, theme_notice) = resolve_theme(&preferences);
        // 配置里的路由就是启动时的当前模型。解析不出来（配置缺失或被改坏）时留空 ——
        // 界面此时显示「未配置」，而不是编一条路由出来。
        let model = boot.model.as_deref().and_then(catalog::parse_route);
        let effort = model
            .as_ref()
            .and_then(|model| model_prefs.effort(model).map(str::to_string))
            .or_else(|| boot.effort.clone());
        // `keybinds.json` 是手改的文件：写坏了要报出来并回退默认，不能因此起不来。
        let keymap = Keymap::load(&boot.preferences);
        let keymap_notice = (!keymap.warnings().is_empty()).then(|| keymap.warnings().join("；"));
        let notice = boot
            .config_error
            .clone()
            .or(theme_notice)
            .or(keymap_notice)
            .unwrap_or_else(|| "交互内核启动中；可以先输入；/help 查看键位".into());
        let backend = match &boot.config_error {
            Some(reason) => BackendState::Failed { reason: reason.clone() },
            None => BackendState::Starting,
        };
        // History lives in the preferences file, so it has to be seeded into the editor at
        // startup and written back on exit; the cap keeps the file from growing without bound.
        let mut editor = Editor::default();
        let keep = preferences.history.len().saturating_sub(200);
        editor.history = preferences.history[keep..].to_vec();
        Self {
            notice,
            effort,
            boot,
            preferences,
            theme,
            editor,
            runs: Vec::new(),
            selected: None,
            detail: None,
            active: None,
            agent: "echo".into(),
            permission: "workspace".into(),
            model,
            model_prefs,
            model_prefs_dirty: false,
            dialog: None,
            theme_dialog: None,
            filter: String::new(),
            choice: 0,
            keymap,
            autocomplete: None,
            leader_until: None,
            files: FileIndex::default(),
            pending_editor: false,
            help_scroll: 0,
            scroll: None,
            scroll_max: 0,
            sidebar_overlay: false,
            expanded: HashMap::new(),
            dirty: true,
            serial: 0,
            backend,
            session_id: None,
            conversation: None,
            pending: None,
            sessions: Vec::new(),
        }
    }

    /// 主区显示会话（而不是 Home 或工人运行详情）。
    pub fn in_conversation(&self) -> bool {
        self.session_id.is_some() || self.pending.is_some()
    }

    /// 交互会话能不能用。`Starting` 也算：命令会在服务的队列里等内核就绪。
    fn sessions_usable(&self) -> bool {
        matches!(self.backend, BackendState::Starting | BackendState::Ready { .. })
    }

    fn conversation_running(&self) -> bool {
        self.conversation.as_ref().is_some_and(|c| c.running)
    }

    /// 当前工作区里的会话（不含子 agent 会话与空会话），按搜索串过滤。
    pub fn filtered_sessions(&self) -> Vec<&SessionSummary> {
        let workspace = normalize_path(&self.boot.workspace.display().to_string());
        let query = self.filter.to_lowercase();
        self.sessions
            .iter()
            .filter(|s| s.parent_session_id.is_none() && !s.blank)
            .filter(|s| s.cwd.as_deref().map(normalize_path).as_deref() == Some(workspace.as_str()))
            .filter(|s| {
                query.is_empty()
                    || format!("{} {}", s.title.as_deref().unwrap_or(""), s.session_id)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect()
    }

    fn session_event(&mut self, event: service::Event) {
        use service::Event as E;
        match event {
            E::Backend { state } => {
                match &state {
                    BackendState::Ready { model } => {
                        // 配置里的路由用的是**用户的** provider id（`deepseek`），内核目录里是 DSH 的
                        // 路由名（`deepseek-official`）。用户还没动过模型时换成内核的说法，否则第一次
                        // selectModel 就会被拒。
                        let untouched = self.model.as_ref().map(ModelRef::route) == self.boot.model;
                        if untouched && let Some(choice) = model {
                            self.model = Some(ModelRef {
                                provider_id: choice.provider.clone(),
                                model_id: choice.model.clone(),
                            });
                        }
                        self.notice = "交互内核就绪：同一会话可以多轮继续；/sessions 打开以前的会话".into();
                    }
                    BackendState::Failed { reason } => {
                        self.notice = format!("交互会话不可用（{}）；提交改为独立请求", first_line(reason));
                        // 新会话还没建好就失败：那条消息没有去处，放回输入框而不是吞掉。
                        if self.session_id.is_none()
                            && let Some(text) = self.pending.take()
                        {
                            self.editor.set_text(text);
                        }
                    }
                    BackendState::Stopped if self.session_id.is_some() => {
                        self.notice = "交互内核已停止".into();
                    }
                    _ => {}
                }
                self.backend = state;
            }
            E::Sessions { sessions } => {
                self.sessions = sessions;
                self.choice = self.choice.min(self.selection_count().saturating_sub(1));
            }
            E::Catalog { catalog } => {
                if !catalog.failures.is_empty() {
                    let names: Vec<&str> = catalog.failures.iter().map(|f| f.name.as_str()).collect();
                    self.notice = format!("这些 provider 的模型目录读取失败：{}", names.join("、"));
                }
                let providers = catalog_from_kernel(&catalog);
                if !providers.is_empty() {
                    self.boot.catalog = providers;
                }
            }
            E::SessionStarted { session_id } => {
                if self.session_id.is_none() && self.pending.is_some() {
                    self.session_id = Some(session_id);
                }
            }
            E::Conversation { conversation } => {
                if self.session_id.as_deref() == Some(conversation.session_id.as_str()) {
                    if let Some(text) = &self.pending
                        && conversation
                            .messages
                            .iter()
                            .any(|m| matches!(m, Message::User(u) if u.text.trim() == text.trim()))
                    {
                        self.pending = None;
                    }
                    self.conversation = Some(*conversation);
                }
            }
            E::SessionClosed { session_id, reason } => {
                if self.session_id.as_deref() == Some(session_id.as_str()) {
                    self.notice = format!("会话订阅已结束：{}", first_line(&reason));
                }
            }
            E::Notice { level, text } => {
                self.notice = match level {
                    NoticeLevel::Error => format!("错误：{text}"),
                    _ => text,
                };
            }
            E::Welcome { .. } | E::Unknown => {}
        }
    }

    /// Re-resolves from preferences. Used by `/mode` and at startup; previews go through
    /// `preview_theme` instead so they do not silently persist.
    pub fn apply_preferences_theme(&mut self) {
        self.theme = Theme::load(self.preferences.theme_name(), self.preferences.mode())
            .unwrap_or_else(|_| Theme::fallback(self.preferences.mode()));
        self.dirty = true;
    }

    /// Live preview while the picker is open. Writes the preference in memory but never to
    /// disk before the dialog is confirmed — `main` persists once, on exit.
    pub fn preview_theme(&mut self, name: &str) {
        if self.preferences.theme.as_deref() == Some(name) && self.theme.name == name {
            return;
        }
        self.preferences.theme = Some(name.to_string());
        self.apply_preferences_theme();
    }

    pub fn commit_theme(&mut self, name: &str) {
        self.preview_theme(name);
        self.notice = format!("主题：{}（已保存）", self.theme.name);
    }

    pub fn update(&mut self, update: Update) {
        match update {
            Update::Runs(runs) if self.runs != runs => {
                self.runs = runs;
                self.dirty = true;
            }
            Update::Detail(detail)
                if self.selected.as_deref() == Some(&detail.record.id)
                    && self.detail.as_ref() != Some(&detail) =>
            {
                self.detail = Some(detail);
                self.dirty = true;
            }
            Update::Finished(id) if self.active.as_ref() == Some(&id) => {
                self.active = None;
                if self.notice.starts_with("正在启动") || self.notice.starts_with("已请求取消")
                {
                    self.notice = "运行已结束，结论见正文；/new 开始新的独立请求".into();
                }
                self.dirty = true;
            }
            Update::Notice(text) => {
                self.notice = text;
                self.dirty = true;
            }
            Update::Session(event) => {
                self.session_event(event);
                self.dirty = true;
            }
            _ => {}
        }
    }
    /// Palette rows, in registry order so the category headers the view draws stay grouped.
    /// Filtering is fuzzy but does not reorder: a search that reshuffles groups makes the
    /// headers meaningless.
    pub fn filtered_commands(&self) -> Vec<&'static Entry> {
        let query = self.filter.trim().to_lowercase();
        commands::palette_entries()
            .filter(|entry| {
                query.is_empty()
                    || autocomplete::fuzzy(&query, entry.slash.unwrap_or(entry.id)).is_some()
                    || autocomplete::fuzzy(&query, entry.title).is_some()
            })
            .collect()
    }
    pub fn filtered_runs(&self) -> Vec<&RunRecord> {
        let query = self.filter.to_lowercase();
        self.runs
            .iter()
            .filter(|r| {
                format!("{} {}", r.id, r.title)
                    .to_lowercase()
                    .contains(&query)
            })
            .collect()
    }
    pub fn open_dialog(&mut self, dialog: Dialog) {
        // Leaving the theme picker by any route other than Enter must undo its live preview,
        // exactly like Esc does. Ctrl+P from inside the picker used to leave the preview
        // applied but unconfirmed, which looked like a saved theme after a restart.
        if dialog != Dialog::Theme
            && let Some(open) = self.theme_dialog.take()
        {
            let action = open.restore();
            self.apply_theme_dialog_action(action);
        }
        // `Dialog` 带着模型对话框的状态，不是 Copy：先记下是不是主题选择器再移进去。
        let is_theme = dialog == Dialog::Theme;
        self.dialog = Some(dialog);
        self.filter.clear();
        self.choice = 0;
        self.autocomplete = None;
        // The picker snapshots the current theme on open so Esc can put it back; a stale
        // snapshot from a previous open would restore the wrong theme.
        self.theme_dialog = is_theme.then(|| ThemeDialog::open(self));
    }
    /// 换一个对话框但**保留搜索框**：模型 → provider → 限定的模型列表这条路
    /// 来回走的时候，用户刚打进去的过滤串不该每次都被清掉。
    fn swap_dialog(&mut self, dialog: Dialog) {
        self.dialog = Some(dialog);
        self.choice = 0;
    }
    fn selection_count(&self) -> usize {
        match &self.dialog {
            Some(Dialog::Commands) => self.filtered_commands().len(),
            Some(Dialog::Sessions) => self.filtered_sessions().len(),
            Some(Dialog::Runs) => self.filtered_runs().len(),
            Some(Dialog::Agents) => 3,
            Some(Dialog::Permissions) => commands::PERMISSIONS.len(),
            // Help scrolls instead of selecting; non-zero keeps the generic clamp harmless.
            Some(Dialog::Help) => 1,
            Some(Dialog::Models(state)) => self.model_rows(*state).len(),
            Some(Dialog::Providers(_)) => self.provider_rows().len(),
            Some(Dialog::Efforts(_)) => self.effort_count(),
            // The theme picker keeps its own cursor; this only has to be non-zero so the
            // generic `Up`/`Down` branch cannot clamp it away before the dialog sees the key.
            Some(Dialog::Theme) => 1,
            None => 0,
        }
    }
    /// The state-dependent half of availability. `Entry::unavailable` covers what this build
    /// cannot do at all; this covers what cannot be done *right now*.
    pub(crate) fn availability(&self, action: Action) -> Option<String> {
        if let Some(reason) = commands::entry(action).unavailable {
            return Some(reason.to_string());
        }
        match action {
            // `/new` clears the view; it does not cancel or queue anything, so it stays
            // available while a run is active (the submit path is what refuses a second one).
            Action::Cancel if !self.has_running() => Some("当前没有进行中的一轮".to_string()),
            Action::Rename if self.session_id.is_none() => {
                Some("还没有会话；先发一条消息，或用 /sessions 打开一个".to_string())
            }
            Action::Evidence if self.in_conversation() => {
                Some("证据目录只属于工人运行（/runs）；交互会话的日志在 DSH home 里".to_string())
            }
            Action::Usage | Action::Evidence | Action::Tools | Action::Latest
                if self.detail.is_none() && !self.in_conversation() =>
            {
                Some("先开始一个会话，或在 /runs 里选一条工人运行".to_string())
            }
            Action::Permission if self.conversation_running() => {
                Some("这一轮还在进行；先取消或等它结束，再修改授权（要重启交互内核）".to_string())
            }
            Action::Effort | Action::CycleEffort if self.active.is_some() => {
                Some("当前请求的模型强度已固定，请等它结束".to_string())
            }
            Action::Effort | Action::CycleEffort if self.model.is_none() => {
                Some("还没有当前模型；先用 /models 选一个".to_string())
            }
            Action::Permission if self.active.is_some() => {
                Some("请先取消并等待当前运行结束，再修改授权".to_string())
            }
            Action::Build | Action::CycleAgent if self.active.is_some() => {
                Some("请先取消并等待当前运行结束，再切换 Agent".to_string())
            }
            Action::CycleModelNext | Action::CycleModelReverse if self.model.is_none() => {
                Some("还没有当前模型；用 /models 选一个".to_string())
            }
            _ => None,
        }
    }
    fn has_running(&self) -> bool {
        self.selected_is_running() || self.active.is_some() || self.conversation_running()
    }
    /// Keymap focus. Provider/effort are steps inside the model flow but do not carry the
    /// model dialog's Ctrl+F/Ctrl+A, so they count as "other".
    fn focus(&self) -> Focus {
        match self.dialog {
            Some(Dialog::Models(_)) => Focus::ModelDialog,
            Some(_) => Focus::OtherDialog,
            None => Focus::Composer,
        }
    }
    /// Runs one action. `true` means "quit the TUI".
    ///
    /// Every path starts with the availability check, so a command the backend cannot serve
    /// says why instead of doing nothing (§7.4: 不可用能力有原因).
    fn execute(&mut self, action: Action, arg: Option<&str>, requests: &Sender<Request>) -> bool {
        if let Some(reason) = self.availability(action) {
            self.notice = reason;
            return false;
        }
        let arg = arg.map(str::trim).filter(|arg| !arg.is_empty());
        if arg.is_some() && !matches!(action, Action::Effort | Action::Permission | Action::Rename) {
            let entry = commands::entry(action);
            self.notice = format!(
                "{} 不接受参数；Ctrl+P 查看命令用法",
                entry.slash.unwrap_or(entry.title)
            );
            return false;
        }
        match action {
            Action::Palette => self.open_dialog(Dialog::Commands),
            Action::New => {
                self.selected = None;
                self.detail = None;
                self.scroll = None;
                if let Some(id) = self.session_id.take() {
                    let _ = requests.send(Request::Session(Command::CloseSession { session_id: id }));
                }
                self.conversation = None;
                self.pending = None;
                // A new request is a new session boundary: a completion popup left over from
                // the previous prompt would insert text that no longer starts a command.
                self.autocomplete = None;
                let _ = requests.send(Request::Open(None));
            }
            Action::Runs => self.open_dialog(Dialog::Runs),
            Action::Sessions => {
                if !self.sessions_usable() {
                    self.notice = "交互会话不可用；工人运行在 /runs".into();
                } else {
                    let _ = requests.send(Request::Session(Command::ListSessions));
                    self.open_dialog(Dialog::Sessions);
                }
            }
            Action::Rename => match (arg, &self.session_id) {
                (Some(title), Some(id)) => {
                    let _ = requests.send(Request::Session(Command::Rename {
                        session_id: id.clone(),
                        title: title.to_string(),
                    }));
                }
                _ => self.notice = "用法：/rename <新标题>".into(),
            },
            Action::Agents => self.open_dialog(Dialog::Agents),
            Action::Models => self.open_dialog(Dialog::Models(ModelDialogState::default())),
            Action::Effort => match arg {
                Some(arg) => self.set_effort(arg),
                None => self.open_effort_picker(),
            },
            Action::CycleEffort => self.cycle_effort(),
            Action::Permission => match arg {
                Some(arg) => self.set_permission(arg, requests),
                None => {
                    self.open_dialog(Dialog::Permissions);
                    self.choice = commands::PERMISSIONS
                        .iter()
                        .position(|(name, _, _)| *name == self.permission)
                        .unwrap_or(0);
                }
            },
            Action::Build => {
                self.agent = "build".into();
                self.notice = "下一次独立请求使用 Build".into();
            }
            Action::Theme => self.open_dialog(Dialog::Theme),
            Action::Mode => {
                self.preferences.light = !self.preferences.light;
                self.apply_preferences_theme();
                self.notice = format!("界面模式：{}", self.preferences.mode().as_str());
            }
            Action::Sidebar => {
                self.preferences.sidebar = !self.preferences.sidebar;
                self.sidebar_overlay = self.preferences.sidebar;
            }
            Action::Tools if self.in_conversation() => {
                let key = format!("session:{}", self.session_id.clone().unwrap_or_default());
                let open = self.expanded.entry(key).or_default();
                *open = !*open;
                self.notice = if *open { "已展开思考与工具输出".into() } else { "已折叠思考与工具输出".into() };
            }
            Action::Tools => {
                if let Some(id) = &self.selected {
                    let open = self.expanded.entry(id.clone()).or_default();
                    *open = !*open;
                }
            }
            Action::Latest => self.scroll = None,
            Action::Edit => {
                // The terminal has to be handed back to the editor process; `run` owns that,
                // so this only raises the flag and explains what is about to happen.
                self.pending_editor = true;
                self.notice = "正在打开外部编辑器…".into();
            }
            Action::PasteSummary => {
                self.preferences.paste_summary = !self.preferences.paste_summary;
                self.notice = if self.preferences.paste_summary {
                    "大段粘贴将折叠为 [粘贴 N 行]，发送时展开原文".into()
                } else {
                    "大段粘贴保持原文".into()
                };
            }
            Action::Help => {
                self.open_dialog(Dialog::Help);
                self.help_scroll = 0;
            }
            Action::Cancel if self.conversation_running() => {
                if let Some(id) = &self.session_id {
                    let _ = requests.send(Request::Session(Command::Cancel { session_id: id.clone() }));
                    self.notice = "已请求取消这一轮".into();
                }
            }
            Action::Cancel => {
                if let Some(id) = self
                    .selected
                    .as_ref()
                    .filter(|_| self.selected_is_running())
                    .or(self.active.as_ref())
                {
                    let _ = requests.send(Request::Cancel(id.clone()));
                    self.notice = "已请求取消，等待监工确认收尾".into();
                } else {
                    self.notice = "当前没有活动运行".into();
                }
            }
            Action::Usage if self.in_conversation() => {
                let mut total = arc_deck_core::conversation::TokenUsage::default();
                for message in self.conversation.iter().flat_map(|c| &c.messages) {
                    if let Message::Assistant(turn) = message
                        && let Some(usage) = &turn.usage
                    {
                        total.add(usage);
                    }
                }
                self.notice = if total.input.is_none() && total.output.is_none() {
                    "provider 尚未报告用量".into()
                } else {
                    format!(
                        "本会话合计：输入 {} · 缓存 {} · 输出 {} token（provider 实报）",
                        token(total.input),
                        token(total.cache_read),
                        token(total.output)
                    )
                };
            }
            Action::Usage => {
                self.notice = self
                    .detail
                    .as_ref()
                    .and_then(|d| d.record.usage.as_ref())
                    .map(|u| {
                        format!(
                            "输入 {} · 缓存 {} · 输出 {} token",
                            token(u.prompt_tokens),
                            token(u.cache_read_tokens),
                            token(u.completion_tokens)
                        )
                    })
                    .unwrap_or_else(|| "provider 尚未报告用量".into());
            }
            Action::Evidence => {
                self.notice = self
                    .detail
                    .as_ref()
                    .map(|d| {
                        format!(
                            "完整证据：{}；可用 arcdeck report {}",
                            d.evidence_dir.display(),
                            d.record.id
                        )
                    })
                    .unwrap_or_else(|| "先选择一条运行记录".into())
            }
            Action::Quit => return true,
            // `/plan` is behind `Entry::unavailable`, so this arm is unreachable from every
            // surface; it stays explicit instead of `_ =>`, which would hide a future mistake.
            Action::Plan => unreachable!("Plan is unavailable and filtered by availability"),
            Action::Favorite => self.toggle_favorite(),
            Action::ProviderList => {
                if let Some(Dialog::Models(back)) = self.dialog {
                    self.swap_dialog(Dialog::Providers(ProviderDialogState { back: Some(back) }));
                }
            }
            Action::CycleModelNext => self.cycle_model(1),
            Action::CycleModelReverse => self.cycle_model(-1),
            Action::CycleAgent => self.cycle_agent(),
        }
        false
    }
    fn set_permission(&mut self, arg: &str, requests: &Sender<Request>) {
        match arg {
            "readonly" | "workspace" | "full" => {
                if self.permission == arg {
                    self.notice = format!("授权级别已经是 {arg}");
                    return;
                }
                self.permission = arg.into();
                if self.sessions_usable() {
                    // 授权级别落在交互内核的启动参数里（沙箱档 + 预设表），只能重启内核。
                    // 会话在独立的 DSH home 里，重启后桥会把当前会话重新打开。
                    let _ = requests.send(Request::RestartBackend { permission: arg.into() });
                    self.backend = BackendState::Starting;
                    self.notice = format!("授权级别改为 {arg}：交互内核重启中，当前会话会自动重新打开");
                } else {
                    self.notice = format!("下一次独立请求的授权级别：{arg}");
                }
            }
            "confirm" => self.notice = commands::CONFIRM_REASON.into(),
            other => {
                self.notice = format!("未知授权级别 `{other}`；支持 readonly / workspace / full")
            }
        }
    }
    fn set_effort(&mut self, arg: &str) {
        let value = (arg != "default").then(|| arg.to_string());
        let mut notice = match &value {
            Some(_) => format!("下一次请求的 effort：{arg}（由运行时校验）"),
            None => "下一次请求恢复模型缺省强度".to_string(),
        };
        if let Some(model) = self.model.clone()
            && let Some(efforts) = model_dialog::efforts(&self.boot.catalog, &model)
        {
            // Validate before assigning: a rejected level must leave the current one alone.
            if let Some(value) = &value
                && !efforts.iter().any(|effort| effort == value)
            {
                self.notice = format!(
                    "`{arg}` 不是 {} 的强度档；可用：{}",
                    catalog::display_name(&self.boot.catalog, &model),
                    efforts.join(" / ")
                );
                return;
            }
            self.model_prefs.set_effort(&model, value.as_deref());
            self.model_prefs_dirty = true;
            notice.push_str("；已记住这个模型的强度");
        }
        self.effort = value;
        self.notice = notice;
    }
    fn open_effort_picker(&mut self) {
        let Some(model) = self.model.clone() else {
            return;
        };
        match model_dialog::efforts(&self.boot.catalog, &model) {
            Some(efforts) if !efforts.is_empty() => {
                self.open_dialog(Dialog::Efforts(EffortDialogState {
                    model,
                    efforts: efforts.len(),
                }));
            }
            _ => {
                self.notice = format!(
                    "{} 没有报告强度档；/effort default 清掉当前值",
                    catalog::display_name(&self.boot.catalog, &model)
                );
            }
        }
    }
    /// Ctrl+T: cycle the current model's effort levels, ending on "default" so one key reaches
    /// every level. The list comes from the catalog, never from a hard-coded table.
    fn cycle_effort(&mut self) {
        let Some(model) = self.model.clone() else {
            return;
        };
        let Some(efforts) = model_dialog::efforts(&self.boot.catalog, &model) else {
            self.notice = format!(
                "{} 没有报告强度档",
                catalog::display_name(&self.boot.catalog, &model)
            );
            return;
        };
        let current = self.effort.clone();
        let next = match current
            .as_ref()
            .and_then(|value| efforts.iter().position(|effort| effort == value))
        {
            None => efforts.first().cloned(),
            Some(index) => efforts.get(index + 1).cloned(),
        };
        self.effort = next;
        self.model_prefs.set_effort(&model, self.effort.as_deref());
        self.model_prefs_dirty = true;
        self.notice = match &self.effort {
            Some(effort) => format!("下一次请求的 effort：{effort}"),
            None => "下一次请求使用模型缺省强度".into(),
        };
    }
    fn cycle_agent(&mut self) {
        self.agent = if self.agent == "echo" { "build" } else { "echo" }.into();
        self.notice = format!("下一次独立请求的 Agent：{}", self.agent);
    }
    fn selected_is_running(&self) -> bool {
        self.detail
            .as_ref()
            .is_some_and(|d| !d.record.terminal && d.record.health != "dead")
            || self.active.is_some() && self.active == self.selected
    }
    /// 当前模型在界面上的展示名（`model.name` → id → provider 名 → 原样路由）。
    pub fn model_name(&self) -> String {
        let Some(model) = &self.model else {
            return "未配置".into();
        };
        catalog::display_name(&self.boot.catalog, model)
    }
    fn model_rows(&self, state: ModelDialogState) -> Vec<model_dialog::ModelRow> {
        model_dialog::rows(
            &self.boot.catalog,
            &self.model_prefs,
            state,
            &self.filter,
            self.model.as_ref(),
        )
    }
    fn provider_rows(&self) -> Vec<catalog::ProviderOption> {
        // 与渲染用同一个过滤函数，否则搜索时的选中下标会对到另一份列表上。
        model_dialog::provider_candidates(
            &self.boot.catalog,
            self.model.as_ref(),
            &self.filter.trim().to_lowercase(),
        )
    }
    fn effort_count(&self) -> usize {
        match &self.dialog {
            // 第一行是「默认」，其余是模型报出来的档。
            Some(Dialog::Efforts(state)) => state.efforts + 1,
            _ => 0,
        }
    }
    /// 模型对话框里的 Ctrl+F：收藏 / 取消收藏当前行。
    fn toggle_favorite(&mut self) {
        let Some(Dialog::Models(state)) = self.dialog else {
            return;
        };
        let Some(row) = model_dialog::selected(
            &self.boot.catalog,
            &self.model_prefs,
            state,
            &self.filter,
            self.model.as_ref(),
            self.choice,
        ) else {
            return;
        };
        let was = self.model_prefs.favorite.contains(&row.model);
        self.model_prefs.toggle_favorite(&row.model);
        self.model_prefs_dirty = true;
        self.notice = format!("{}{}", if was { "取消收藏 " } else { "已收藏 " }, row.title);
    }
    /// F2 / Shift+F2：在 Recent 里前后切换。**不改 Recent 的顺序** ——
    /// 每次切换都把目标挪到最前的话，连按两次 F2 就只会在两条之间来回跳，等于卡住。
    /// 上游的 `cycle` 同样只改当前模型；顺序由对话框选定与发请求时更新。
    fn cycle_model(&mut self, direction: isize) {
        let recent = self.model_prefs.recent.clone();
        let Some(current) = self.model.as_ref() else {
            self.notice = "还没有当前模型；用 leader,m 选一个".into();
            return;
        };
        let Some(index) = recent.iter().position(|item| *item == *current) else {
            self.notice = "当前模型不在 Recent 里；先用 leader,m 选一次".into();
            return;
        };
        let next = (index as isize + direction).rem_euclid(recent.len() as isize) as usize;
        let Some(target) = recent.get(next) else {
            return;
        };
        if *target == *current {
            return;
        }
        self.model = Some(target.clone());
        self.effort = self
            .model_prefs
            .effort(target)
            .map(str::to_string)
            .or_else(|| self.boot.effort.clone());
        self.notice = format!(
            "下一次请求使用 {}（{}）；运行中的请求不受影响",
            catalog::display_name(&self.boot.catalog, target),
            target.route()
        );
    }
    fn submit(&mut self, requests: &Sender<Request>) -> bool {
        let text = self.editor.text.trim().to_string();
        if text.is_empty() {
            return false;
        }
        if text.starts_with('/') {
            self.editor.take();
            self.autocomplete = None;
            // One parser for every surface: what the completion popup offers, what Enter runs
            // and what the help page documents all resolve through `commands::parse`.
            return match commands::parse(&text) {
                Some(parsed) => {
                    self.execute(parsed.entry.action, parsed.arg.as_deref(), requests)
                }
                None => {
                    let name = text.split_whitespace().next().unwrap_or("/");
                    self.notice = format!(
                        "未知命令 `{name}`；Ctrl+P 查看命令，/help 查看键位"
                    );
                    false
                }
            };
        }
        if self.boot.config_error.is_none() && self.sessions_usable() {
            return self.submit_to_session(requests);
        }
        if self.active.is_some() {
            self.notice = "已有活动运行；Esc 取消，结束后可发起新请求".into();
            return false;
        }
        if self.boot.config_error.is_some() {
            self.notice = self.boot.config_error.clone().unwrap();
            return false;
        }
        self.serial += 1;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let id = format!("tui-{stamp}-{}-{}", std::process::id(), self.serial);
        // 模型对话框选定的路由随这次请求走（桥接成 `--model`）。没选过就是配置里的那条。
        let model = self
            .model
            .as_ref()
            .map(ModelRef::route)
            .or_else(|| self.boot.model.clone());
        if let Some(model) = &self.model {
            // 用过的模型进 Recent —— 顺序在这里更新，而不是在 F2 切换时（见 cycle_model）。
            self.model_prefs.touch_recent(model);
            self.model_prefs_dirty = true;
        }
        let request = StartRequest {
            id: id.clone(),
            prompt: self.editor.take(),
            agent: self.agent.clone(),
            permission: self.permission.clone(),
            effort: self.effort.clone(),
            model,
        };
        if requests.send(Request::Start(request)).is_err() {
            self.notice = "执行端已断开".into();
            return false;
        }
        self.active = Some(id.clone());
        self.selected = Some(id);
        self.detail = None;
        self.scroll = None;
        self.notice = "正在启动独立请求；工具进度实时更新，正文在结束后显示".into();
        false
    }
    /// 会话这条路：没有当前会话就由服务新建；运行中也能发（内核按 `queue` 排到下一轮）。
    fn submit_to_session(&mut self, requests: &Sender<Request>) -> bool {
        if self.session_id.is_none() && self.pending.is_some() {
            self.notice = "上一条还在新建会话，请稍候".into();
            return false;
        }
        let text = self.editor.take();
        if let Some(model) = &self.model {
            self.model_prefs.touch_recent(model);
            self.model_prefs_dirty = true;
        }
        let model = self.model.as_ref().map(|m| ModelChoice {
            provider: m.provider_id.clone(),
            model: m.model_id.clone(),
            effort: self.effort.clone(),
        });
        let queued = self.conversation_running();
        let command = Command::Prompt { session_id: self.session_id.clone(), text: text.clone(), model };
        if requests.send(Request::Session(command)).is_err() {
            self.notice = "执行端已断开".into();
            return false;
        }
        self.pending = Some(text);
        self.selected = None;
        self.detail = None;
        self.scroll = None;
        self.notice = match (&self.backend, queued) {
            (BackendState::Starting, _) => "交互内核启动中，消息已排队".into(),
            (_, true) => "这一轮还在进行，消息排到下一轮".into(),
            _ => "已发送；Esc 取消这一轮".into(),
        };
        false
    }

    fn select(&mut self, requests: &Sender<Request>) -> bool {
        // 克隆而不是借用：下面几个分支都要 &mut self（打开新对话框、改偏好）。
        match self.dialog.clone() {
            Some(Dialog::Commands) => {
                let action = self.filtered_commands().get(self.choice).map(|entry| entry.action);
                self.dialog = None;
                if let Some(action) = action {
                    // Close first: an unavailable command reports into the notice line, which
                    // the palette itself would cover.
                    return self.execute(action, None, requests);
                }
            }
            Some(Dialog::Sessions) => {
                if let Some(id) = self.filtered_sessions().get(self.choice).map(|s| s.session_id.clone()) {
                    if let Some(old) = self.session_id.replace(id.clone())
                        && old != id
                    {
                        let _ = requests.send(Request::Session(Command::CloseSession { session_id: old }));
                    }
                    self.conversation = None;
                    self.pending = None;
                    self.selected = None;
                    self.detail = None;
                    self.scroll = None;
                    let _ = requests.send(Request::Session(Command::OpenSession { session_id: id }));
                    self.dialog = None;
                    self.notice = "正在打开会话（历史较长时要翻页）…".into();
                }
            }
            Some(Dialog::Runs) => {
                if let Some(id) = self.filtered_runs().get(self.choice).map(|r| r.id.clone()) {
                    // 看工人运行时离开会话视图（会话照常在后台订阅着，/sessions 能回去）。
                    self.session_id = None;
                    self.conversation = None;
                    self.pending = None;
                    self.selected = Some(id.clone());
                    self.detail = None;
                    self.scroll = None;
                    let _ = requests.send(Request::Open(Some(id)));
                    self.dialog = None;
                }
            }
            Some(Dialog::Agents) => {
                if self.active.is_some() {
                    self.notice = "请先取消并等待当前运行结束，再切换 Agent".into();
                } else {
                    match self.choice {
                        0 => {
                            self.agent = "echo".into();
                            self.dialog = None;
                            self.notice = "下一次独立请求的 Agent：echo".into();
                        }
                        1 => {
                            self.agent = "build".into();
                            self.dialog = None;
                            self.notice = "下一次独立请求的 Agent：build".into();
                        }
                        // Plan has no read-only execution path; the reason comes from the
                        // registry, so this row and `/plan` cannot drift apart.
                        _ => {
                            self.notice = commands::entry(Action::Plan)
                                .unavailable
                                .unwrap_or_default()
                                .into();
                        }
                    }
                }
            }
            Some(Dialog::Permissions) => {
                if let Some((name, _, reason)) = commands::PERMISSIONS.get(self.choice) {
                    match reason {
                        Some(reason) => self.notice = (*reason).into(),
                        None => {
                            self.dialog = None;
                            self.set_permission(name, requests);
                        }
                    }
                }
            }
            Some(Dialog::Help) => self.dialog = None,
            Some(Dialog::Models(state)) => self.select_model(state),
            Some(Dialog::Providers(_)) => {
                if let Some(provider) = self.provider_rows().get(self.choice).cloned() {
                    // 从 provider 列表跳进只含该 provider 的模型对话框（上游同样是 replace）。
                    let index = self
                        .boot
                        .catalog
                        .iter()
                        .position(|item| item.id == provider.id);
                    // 保留搜索串：用户可能刚在 provider 列表里打过字。
                    self.swap_dialog(Dialog::Models(ModelDialogState {
                        only_provider: index,
                    }));
                }
            }
            Some(Dialog::Efforts(state)) => self.select_effort(&state),
            // Enter inside the picker is handled by `ThemeDialog::confirm` before this runs;
            // reaching here means the dialog was already settled.
            Some(Dialog::Theme) => {}
            None => {}
        }
        false
    }
    /// 模型对话框的结果：设为**下一次请求**用的模型、更新 Recent、必要时进 effort。
    fn select_model(&mut self, state: ModelDialogState) {
        let Some(row) = model_dialog::selected(
            &self.boot.catalog,
            &self.model_prefs,
            state,
            &self.filter,
            self.model.as_ref(),
            self.choice,
        ) else {
            return;
        };
        if let Some(reason) = &row.disabled {
            // §7.2：不可用的模型显式禁用并说明原因，所以它留在列表里 ——
            // 但确认那一步必须拒绝，否则「显示了原因却还能选」比不显示更糟。
            self.notice = format!("{} 不可用：{reason}", row.title);
            return;
        }
        self.model = Some(row.model.clone());
        self.effort = self
            .model_prefs
            .effort(&row.model)
            .map(str::to_string)
            .or_else(|| self.boot.effort.clone());
        self.model_prefs.touch_recent(&row.model);
        self.model_prefs_dirty = true;
        // 有强度档的模型按需再问一次（上游 DialogModel → DialogVariant）；
        // 没有就直接关掉，不拿一个空对话框拦住用户。
        match model_dialog::efforts(&self.boot.catalog, &row.model) {
            Some(efforts) => {
                self.notice = format!(
                    "下一次请求使用 {}；该模型有强度档，请选择 effort",
                    catalog::display_name(&self.boot.catalog, &row.model)
                );
                // 同样保留搜索框里的串。
                self.swap_dialog(Dialog::Efforts(EffortDialogState {
                    model: row.model,
                    efforts: efforts.len(),
                }));
                self.filter.clear();
            }
            None => {
                self.notice = format!(
                    "下一次请求使用 {}（{}）；运行中的请求不受影响",
                    catalog::display_name(&self.boot.catalog, &row.model),
                    row.model.route()
                );
                self.dialog = None;
            }
        }
    }

    fn select_effort(&mut self, state: &EffortDialogState) {
        // 越界（列表正在变短）时按「默认」处理，与 effort 对话框第 0 行一致。
        let value = model_dialog::effort_at(&self.boot.catalog, state, self.choice);
        self.model_prefs.set_effort(&state.model, value.as_deref());
        self.model_prefs_dirty = true;
        self.effort = value.clone();
        self.notice = match value {
            Some(effort) => format!("下一次请求的 effort：{effort}"),
            None => "下一次请求恢复模型缺省强度".into(),
        };
        self.dialog = None;
    }
    pub fn input(&mut self, event: Event, requests: &Sender<Request>) -> bool {
        self.dirty = true;
        let key = match event {
            Event::Paste(text) => {
                if self.dialog.is_none() {
                    self.insert_paste(&text);
                    self.refresh_autocomplete();
                } else if self.theme_dialog.is_some() {
                    // A pasted theme name filters the list the same way typing does. Newlines
                    // are dropped rather than kept: the filter is a single-line field, and a
                    // pasted multi-line blob would otherwise match nothing.
                    let filter = format!("{}{}", self.theme_dialog_filter(), text.replace('\n', ""));
                    self.set_theme_dialog_filter(filter);
                }
                return false;
            }
            Event::Mouse(mouse) => {
                let delta = match mouse.kind {
                    MouseEventKind::ScrollUp => -3,
                    MouseEventKind::ScrollDown => 3,
                    _ => return false,
                };
                if self.theme_dialog.is_some() {
                    self.move_theme_dialog(delta);
                } else if delta < 0 {
                    self.scroll_up(3);
                } else {
                    self.scroll_down(3);
                }
                return false;
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => key,
            _ => return false,
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        // Esc layering (§7.4): popup → dialog → editing state → cancel. Cancel is last on
        // purpose: a stray Esc while typing must not kill a running job.
        if key.code == KeyCode::Esc {
            self.leader_until = None;
            if self.autocomplete.take().is_some() {
                return false;
            }
            // 从 provider 列表进来的那种：Esc 先退回模型对话框（上游是 replace 进去的），
            // 再按一次才关掉整个流程。对话框里也写了「Esc 返回模型对话框」。
            if let Some(Dialog::Providers(state)) = &self.dialog
                && let Some(back) = state.back
            {
                self.swap_dialog(Dialog::Models(back));
                return false;
            }
            if self.dialog.take().is_some() {
                if let Some(dialog) = self.theme_dialog.take() {
                    // Upstream restores the opening theme in `onCleanup` unless Enter confirmed.
                    let action = dialog.restore();
                    self.apply_theme_dialog_action(action);
                }
                return false;
            }
            if !self.editor.text.is_empty() {
                self.editor.text.clear();
                self.editor.cursor = 0;
                return false;
            }
            if self.has_running() {
                return self.execute(Action::Cancel, None, requests);
            }
            return false;
        }

        if self.leader_until.is_some() {
            self.leader_until = None;
            return match self.keymap.leader_action(&key) {
                Some(action) => self.execute(action, None, requests),
                None => {
                    self.notice = "没有这个 leader 组合；Ctrl+X 后看底部提示".into();
                    false
                }
            };
        }
        // A leader chord only starts in the composer: inside a dialog Ctrl+X would otherwise
        // swallow the next keystroke the dialog was waiting for.
        if self.dialog.is_none() && self.keymap.is_leader_key(&key) {
            self.leader_until =
                Some(Instant::now() + Duration::from_millis(keymap::LEADER_TIMEOUT_MS));
            return false;
        }

        if self.dialog.is_some() {
            // The keymap settles the "who owns this key" question before any dialog gets to
            // see it: Ctrl+F/Ctrl+A are only bindings inside the model dialog, and Ctrl+C is
            // global in every dialog.
            if let Some(action) = self.keymap.resolve(&key, self.focus()) {
                return self.execute(action, None, requests);
            }
            if self.theme_dialog.is_some() {
                match key.code {
                    KeyCode::Enter => {
                        let action = self.theme_dialog.as_mut().and_then(|dialog| dialog.confirm());
                        self.theme_dialog = None;
                        self.dialog = None;
                        self.apply_theme_dialog_action(action);
                        return false;
                    }
                    _ => {
                        let action = self.theme_dialog.as_mut().and_then(|dialog| dialog.key(key.code, ctrl));
                        if let Some(action) = action {
                            self.apply_theme_dialog_action(Some(action));
                            return false;
                        }
                    }
                }
            }
            if matches!(self.dialog, Some(Dialog::Help)) {
                match key.code {
                    KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                    KeyCode::Down => self.help_scroll = self.help_scroll.saturating_add(1),
                    KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(8),
                    KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(8),
                    KeyCode::Enter => self.dialog = None,
                    _ => {}
                }
                return false;
            }
            // Permissions is a fixed three-row list; typing must not filter it.
            let filterable = !matches!(self.dialog, Some(Dialog::Permissions));
            match key.code {
                KeyCode::Up => self.choice = self.choice.saturating_sub(1),
                KeyCode::Down => {
                    self.choice = (self.choice + 1).min(self.selection_count().saturating_sub(1))
                }
                KeyCode::Enter => return self.select(requests),
                KeyCode::Backspace if filterable => {
                    // effort 与 provider 列表也能搜：provider 是按名字找的，
                    // 而 effort 列表短到不需要搜，这里一并接受，键位才一致。
                    self.filter.pop();
                    self.choice = 0;
                }
                KeyCode::Char(c) if !ctrl && !alt && filterable => {
                    self.filter.push(c);
                    self.choice = 0;
                }
                _ => {}
            }
            // 搜索让列表变短时选中行可能越界；每按键一次都夹回范围。
            // 上游 `dialog-select` 的做法是选中项被过滤掉就重置到第一个命中项。
            self.choice = self.choice.min(self.selection_count().saturating_sub(1));
            return false;
        }

        // Completion owns ↑↓/Tab/Enter while it is open, so those keys do not reach history
        // or the submit path.
        if self.autocomplete.is_some() {
            match key.code {
                KeyCode::Up => {
                    if let Some(popup) = self.autocomplete.as_mut() {
                        popup.move_by(-1);
                    }
                    return false;
                }
                KeyCode::Down => {
                    if let Some(popup) = self.autocomplete.as_mut() {
                        popup.move_by(1);
                    }
                    return false;
                }
                KeyCode::Tab => {
                    self.accept_autocomplete(false, requests);
                    return false;
                }
                // Windows keeps Enter as a newline (F6 submits), so accepting with Enter there
                // would be the only place Enter submits — §4 of the audit says F6 is the send
                // key, and completion must not quietly reintroduce the ambiguity.
                KeyCode::Enter if !cfg!(windows) => {
                    self.accept_autocomplete(true, requests);
                    return false;
                }
                KeyCode::F(6) => {
                    self.accept_autocomplete(true, requests);
                    return false;
                }
                _ => {}
            }
        }

        if let Some(action) = self.keymap.resolve(&key, Focus::Composer) {
            return self.execute(action, None, requests);
        }

        match key.code {
            KeyCode::Char('j') if ctrl => self.editor.insert("\n"),
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.editor.insert("\n");
                self.refresh_autocomplete();
            }
            KeyCode::Enter if cfg!(windows) => {
                self.editor.insert("\n");
                self.refresh_autocomplete();
            }
            KeyCode::Enter | KeyCode::F(6) => return self.submit(requests),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.editor.insert(&c.to_string());
                self.refresh_autocomplete();
            }
            KeyCode::Backspace => {
                self.editor.backspace();
                self.refresh_autocomplete();
            }
            KeyCode::Delete => {
                self.editor.delete();
                self.refresh_autocomplete();
            }
            KeyCode::Left => self.editor.left(),
            KeyCode::Right => self.editor.right(),
            KeyCode::Home => self.editor.home(),
            KeyCode::End => self.editor.end(),
            KeyCode::Up if ctrl => {
                self.editor.history(true);
                self.refresh_autocomplete();
            }
            KeyCode::Down if ctrl => {
                self.editor.history(false);
                self.refresh_autocomplete();
            }
            KeyCode::Up if self.editor.text.contains('\n') => self.editor.vertical(true),
            KeyCode::Down if self.editor.text.contains('\n') => self.editor.vertical(false),
            KeyCode::Up => {
                self.editor.history(true);
                self.refresh_autocomplete();
            }
            KeyCode::Down => {
                self.editor.history(false);
                self.refresh_autocomplete();
            }
            KeyCode::PageUp => self.scroll_up(8),
            KeyCode::PageDown => self.scroll_down(8),
            _ => {}
        }
        false
    }

    /// Recomputes the completion popup from the draft. Called after every edit, never on
    /// submit: a command that was just executed must not leave its own completion behind.
    ///
    /// Two triggers share the popup: `/` at the start of the draft (commands) and `@` at the
    /// start of the current word (files). The trigger decides where accepting starts replacing.
    fn refresh_autocomplete(&mut self) {
        if self.dialog.is_some() {
            self.autocomplete = None;
            return;
        }
        if let Some(query) = autocomplete::trigger_query(&self.editor.text, '/') {
            let items =
                autocomplete::slash_items(query, &self.keymap, &|action| self.availability(action));
            self.autocomplete = Autocomplete::new('/', 0, query.to_string(), items);
            return;
        }
        if let Some((start, query)) =
            autocomplete::mention_query(&self.editor.text, self.editor.cursor, '@')
        {
            self.files.ensure(&self.boot.workspace);
            let recent = self.preferences.recent_files.clone();
            let items = autocomplete::file_items(&self.files, query, &recent, 50);
            self.autocomplete = Autocomplete::new('@', start, query.to_string(), items);
            return;
        }
        self.autocomplete = None;
    }

    /// Tab accepts; Enter accepts and immediately runs when the command takes no argument.
    /// An unavailable command is reported instead of inserted — inserting it would only move
    /// the same refusal one keystroke later.
    ///
    /// `@` references are inserted as one segment (see `Editor::insert_segment`), and Enter
    /// after one never submits: the user is still composing prose around the reference.
    fn accept_autocomplete(&mut self, run: bool, requests: &Sender<Request>) {
        let Some(popup) = self.autocomplete.take() else {
            return;
        };
        let Some(item) = popup.current().cloned() else {
            return;
        };
        if !item.available {
            self.notice = item
                .reason
                .clone()
                .unwrap_or_else(|| "该命令当前不可用".into());
            return;
        }
        let end = self.editor.cursor;
        if popup.trigger == '@' {
            // The segment is the reference only; the trailing space is ordinary prose so one
            // Backspace right after accepting removes the space, the next one the whole path.
            let display = item.insert.trim_end();
            self.editor
                .replace_with_segment(popup.start, end, display, None);
            self.editor.insert(" ");
            let path = item
                .insert
                .trim_start_matches('@')
                .trim_end()
                .to_string();
            self.touch_recent_file(&path);
            return;
        }
        self.editor.replace_range(popup.start, end, &item.insert);
        if run && !item.insert.ends_with(' ') {
            let _ = self.submit(requests);
        }
    }

    fn touch_recent_file(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        self.preferences.recent_files.retain(|item| item != path);
        self.preferences.recent_files.insert(0, path.to_string());
        self.preferences.recent_files.truncate(20);
    }

    /// A paste over the threshold becomes one atomic `[粘贴 N 行]` segment whose payload is the
    /// original text. Below it the text goes in literally: a two-line paste is usually part of
    /// the sentence being written, and hiding it would be worse than the rows it takes.
    fn insert_paste(&mut self, text: &str) {
        let lines = text.lines().count();
        if self.preferences.paste_summary && (lines >= 6 || text.len() > 600) {
            self.editor
                .insert_segment(&format!("[粘贴 {lines} 行]"), Some(text));
        } else {
            self.editor.insert(text);
        }
    }

    /// `run` owns the terminal, so the external-editor action only raises this flag.
    pub fn take_editor_request(&mut self) -> bool {
        std::mem::take(&mut self.pending_editor)
    }

    /// Copies the parts of the editor that the preferences file owns. Called right before the
    /// file is written so `history` can never be saved stale.
    pub fn persist_preferences(&mut self) {
        let keep = self.editor.history.len().saturating_sub(200);
        self.preferences.history = self.editor.history[keep..].to_vec();
    }

    /// 2000 ms after the leader chord with no second key, the leader is forgotten. Without a
    /// deadline it would eat the next keystroke the user typed for the composer.
    pub fn tick(&mut self) {
        if let Some(deadline) = self.leader_until
            && Instant::now() >= deadline
        {
            self.leader_until = None;
            self.dirty = true;
        }
    }

    pub fn leader_active(&self) -> bool {
        self.leader_until.is_some()
    }

    /// Keys shown in the which-key strip while the leader is waiting.
    pub fn leader_hints(&self) -> Vec<(String, String)> {
        self.keymap
            .leader_hints()
            .into_iter()
            .map(|(key, action)| (key, commands::entry(action).title.to_string()))
            .collect()
    }

    /// Every binding, grouped by category, for `/help`. Uses the same keymap the input path
    /// uses, so a rebound key is documented in its new form.
    pub fn help_rows(&self) -> Vec<HelpRow> {
        let mut rows = Vec::new();
        for category in Category::ALL {
            for entry in commands::ENTRIES
                .iter()
                .filter(|entry| entry.category == category)
            {
                let keys = self.keymap.display(entry.action).unwrap_or_else(|| "—".into());
                // Only the build-level reason belongs in a reference page; "nothing is
                // running right now" would change under the reader's feet.
                let reason = entry.unavailable.map(str::to_string);
                rows.push(HelpRow {
                    category,
                    keys,
                    title: entry.title.to_string(),
                    context: entry.context,
                    reason,
                });
            }
        }
        rows
    }

    fn scroll_up(&mut self, n: usize) {
        self.scroll = Some(self.scroll.unwrap_or(self.scroll_max).saturating_sub(n));
    }
    fn scroll_down(&mut self, n: usize) {
        let next = self.scroll.unwrap_or(self.scroll_max).saturating_add(n);
        self.scroll = if next >= self.scroll_max {
            None
        } else {
            Some(next)
        };
    }
    fn theme_dialog_filter(&self) -> String {
        self.theme_dialog.as_ref().map(|dialog| dialog.filter().to_string()).unwrap_or_default()
    }
    fn set_theme_dialog_filter(&mut self, filter: String) {
        let action = self.theme_dialog.as_mut().map(|dialog| dialog.set_filter(filter));
        self.apply_theme_dialog_action(action);
    }
    fn move_theme_dialog(&mut self, delta: isize) {
        let action = self.theme_dialog.as_mut().map(|dialog| dialog.move_by(delta));
        self.apply_theme_dialog_action(action);
    }
    fn apply_theme_dialog_action(&mut self, action: Option<ThemeDialogAction>) {
        match action {
            Some(ThemeDialogAction::Preview(name)) => self.preview_theme(&name),
            Some(ThemeDialogAction::Commit(name)) => self.commit_theme(&name),
            None => {}
        }
    }
}

/// Resolves the saved preference, falling back to the default theme in the requested mode.
/// An unknown theme name is a warning, not a startup failure: `tui-preferences.json` is a file
/// a user can edit by hand, and a typo must not stop the TUI from opening.
fn resolve_theme(preferences: &Preferences) -> (Theme, Option<String>) {
    let mode = preferences.mode();
    match Theme::load(preferences.theme_name(), mode) {
        Ok(theme) => (theme, None),
        Err(error) => (
            Theme::fallback(mode),
            Some(format!("{error}；已回退到默认主题 {DEFAULT_THEME}")),
        ),
    }
}

pub(crate) fn token(n: Option<u64>) -> String {
    n.map(|n| n.to_string()).unwrap_or_else(|| "未报告".into())
}

/// 路径比较用：大小写与分隔符不算差别（Windows 上 `C:\\ws` 与 `c:/ws` 是同一个目录）。
fn normalize_path(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

fn first_line(text: &str) -> &str {
    text.lines().find(|line| !line.trim().is_empty()).unwrap_or(text)
}

/// 内核报告的目录 → 模型对话框的形状。权威来源是 DSH（§7.2）；没有的数据（发布日期、免费与否）留空，不编。
fn catalog_from_kernel(catalog: &ModelCatalog) -> Vec<catalog::Provider> {
    catalog
        .providers
        .iter()
        .map(|provider| catalog::Provider {
            id: provider.id.clone(),
            name: provider.name.clone(),
            models: provider
                .models
                .iter()
                .map(|model| catalog::Model {
                    id: model.id.clone(),
                    name: Some(model.name.clone()).filter(|n| !n.is_empty()),
                    efforts: model.efforts.iter().map(|e| e.id.clone()).collect(),
                    ..catalog::Model::default()
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use std::path::PathBuf;
    use std::sync::mpsc;

    use crate::catalog::{Model, Provider};

    fn model(id: &str, name: &str, efforts: &[&str]) -> Model {
        Model {
            id: id.into(),
            name: Some(name.into()),
            efforts: efforts.iter().map(|effort| effort.to_string()).collect(),
            ..Model::default()
        }
    }

    /// 目录夹具：一个普通 provider、一个带 effort 的、一个含不可用模型的。
    /// 路由 `p/m`（配置里的当前模型）必须留在里面 —— 目录的来源保证这一点（见 tui.rs）。
    fn catalog() -> Vec<Provider> {
        let mut offline = model("m-offline", "Offline Model", &[]);
        offline.unavailable = Some("no credentials".into());
        let mut free = model("m-free", "Free Model", &[]);
        free.free = Some(true);
        vec![
            Provider {
                id: "p".into(),
                name: "Provider P".into(),
                models: vec![
                    model("m", "Model M", &[]),
                    free,
                    offline,
                    model("m-effort", "Effort Model", &["off", "low", "high", "max"]),
                ],
            },
            Provider {
                id: "q".into(),
                name: "Provider Q".into(),
                models: vec![model("n", "Model N", &[])],
            },
        ]
    }

    fn boot(model: Option<&str>, prefs: PathBuf) -> Boot {
        Boot {
            workspace: ".".into(),
            model: model.map(str::to_string),
            effort: None,
            config_error: None,
            preferences: "unused".into(),
            catalog: catalog(),
            model_prefs: prefs,
        }
    }

    fn app() -> App {
        App::new(
            boot(Some("p/m"), PathBuf::from("unused.json")),
            Preferences::default(),
            ModelPrefs::default(),
        )
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn key_with(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn type_text(app: &mut App, text: &str, tx: &Sender<Request>) {
        for c in text.chars() {
            app.input(key(KeyCode::Char(c)), tx);
        }
    }

    fn model_dialog_app() -> App {
        let mut app = app();
        app.open_dialog(Dialog::Models(ModelDialogState::default()));
        app
    }

    fn open_models(app: &mut App, tx: &Sender<Request>) {
        app.input(key_with(KeyCode::Char('x'), KeyModifiers::CONTROL), tx);
        app.input(key(KeyCode::Char('m')), tx);
    }

    fn model_dialog(app: &App) -> ModelDialogState {
        match app.dialog {
            Some(Dialog::Models(state)) => state,
            ref other => panic!("期望模型对话框，实际是 {other:?}"),
        }
    }

    fn sections(app: &App) -> Vec<crate::model_dialog::ModelBlock> {
        model_dialog::blocks(
            &app.boot.catalog,
            &app.model_prefs,
            &app.filter,
            model_dialog(app).provider_id(&app.boot.catalog),
            app.model.as_ref(),
        )
    }

    fn buffer_text(app: &mut App, width: u16, height: u16) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        use unicode_width::UnicodeWidthStr;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| crate::view::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..height {
            let mut x = 0;
            while x < width {
                let symbol = buffer[(x, y)].symbol();
                text.push_str(symbol);
                x += (symbol.width() as u16).max(1);
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn model_dialog_lists_favorites_recent_then_each_provider_in_order() {
        let mut app = model_dialog_app();
        app.model_prefs
            .toggle_favorite(&catalog::parse_route("q/n").unwrap());
        app.model_prefs
            .touch_recent(&catalog::parse_route("p/m-free").unwrap());

        // 收藏的 q/n 只在 Favorites 里出现，所以它那一节不再单独画出 Provider Q ——
        // 分节与「同一模型不重复列」是同一条规则的两个面。
        let titles: Vec<String> = sections(&app)
            .iter()
            .map(|block| block.title.clone())
            .collect();
        assert_eq!(titles, vec!["Favorites", "Recent", "Provider P"]);
        let flat: Vec<String> = sections(&app)
            .iter()
            .flat_map(|block| block.rows.iter())
            .map(|row| row.model.route())
            .collect();
        assert_eq!(flat.iter().filter(|route| *route == "q/n").count(), 1);
        assert_eq!(flat.iter().filter(|route| *route == "p/m-free").count(), 1);
        // 当前模型在平铺后的某一节里带 current 标记，且只出现一次。
        let marked: Vec<String> = sections(&app)
            .iter()
            .flat_map(|block| block.rows.iter())
            .filter(|row| row.current)
            .map(|row| row.model.route())
            .collect();
        assert_eq!(marked, vec!["p/m".to_string()]);

        // 用 120x40 而不是 120x30：对话框最多 22 行，30 行的终端里
        // 靠后的分节会被窗口裁掉，那是「没画出来」而不是「没排序」。
        let text = buffer_text(&mut app, 120, 40);
        for header in ["Favorites", "Recent", "Provider P"] {
            assert!(text.contains(header), "分节标题 {header} 没画出来:\n{text}");
        }
        assert!(
            text.find("Favorites").unwrap() < text.find("Recent").unwrap(),
            "Favorites 必须排在 Recent 之前"
        );
        assert!(text.find("Recent").unwrap() < text.find("Provider P").unwrap());
        // Free 页脚要落在这条对话框里最靠右的位置（右对齐），不是紧跟在标题后面。
        // 行形如 `│␣␣␣␣␣␣Free Model␣…␣Free␣␣␣│`：页脚右边只剩 3 格内边距 + 边框。
        let line = text
            .lines()
            .find(|line| line.contains("Free Model"))
            .expect("免费模型那一行");
        assert!(
            line.trim_end().ends_with("Free   │"),
            "footer 必须右对齐，还留在对话框最右侧：{line:?}"
        );
        let head_end = line.find("Free Model").unwrap() + "Free Model".len();
        assert_eq!(
            line[head_end..].trim().trim_end_matches('│').trim(),
            "Free",
            "标题与页脚之间只该是空白：{line:?}"
        );
    }

    #[test]
    fn model_dialog_search_filters_and_drops_the_section_headers() {
        let mut app = model_dialog_app();
        let (tx, _) = mpsc::channel();
        type_text(&mut app, "offline", &tx);
        let blocks = sections(&app);
        assert_eq!(blocks.len(), 1, "搜索命中不画分节标题");
        assert!(blocks[0].title.is_empty());
        assert_eq!(
            blocks[0]
                .rows
                .iter()
                .map(|row| row.model.route())
                .collect::<Vec<_>>(),
            vec!["p/m-offline".to_string()]
        );
        let text = buffer_text(&mut app, 120, 30);
        assert!(text.contains("Offline Model"));
        assert!(!text.contains("Favorites"), "搜索时不该再画分节标题");
        assert!(text.contains("不可用：no credentials"), "禁用原因要写出来");
    }

    #[test]
    fn disabled_rows_stay_listed_but_cannot_be_confirmed() {
        let mut app = model_dialog_app();
        let (tx, rx) = mpsc::channel();
        type_text(&mut app, "offline", &tx);
        assert!(sections(&app)[0].rows[0].disabled.is_some());
        app.input(key(KeyCode::Enter), &tx);
        assert_eq!(
            app.model,
            Some(catalog::parse_route("p/m").unwrap()),
            "不可用的模型不能变成当前模型"
        );
        assert!(matches!(app.dialog, Some(Dialog::Models(_))), "对话框不该关闭");
        assert!(rx.try_recv().is_err(), "没有请求该被发出去");
        assert!(app.notice.contains("不可用"), "要说明为什么没生效：{}", app.notice);
    }

    #[test]
    fn selecting_a_model_without_efforts_closes_and_applies_to_the_next_request() {
        let mut app = model_dialog_app();
        app.backend = BackendState::Failed { reason: "测试走独立请求那条路".into() };
        let (tx, rx) = mpsc::channel();
        // 搜索把候选缩到 "Model N"，省得依赖排序位置。
        type_text(&mut app, "Model N", &tx);
        app.input(key(KeyCode::Enter), &tx);
        assert_eq!(app.model, Some(catalog::parse_route("q/n").unwrap()));
        assert_eq!(app.dialog, None, "没有强度档就直接关闭（§7.2）");
        assert_eq!(app.model_prefs.recent[0].route(), "q/n");

        app.editor.insert("跑一条");
        app.input(key(KeyCode::F(6)), &tx);
        let Request::Start(request) = rx.recv().unwrap() else {
            panic!("应该是一次 Start")
        };
        assert_eq!(request.model.as_deref(), Some("q/n"));
        assert_eq!(request.effort, None);
    }

    #[test]
    fn request_carries_the_configured_route_even_before_the_dialog_is_used() {
        let mut app = app();
        app.backend = BackendState::Failed { reason: "测试走独立请求那条路".into() };
        let (tx, rx) = mpsc::channel();
        app.editor.insert("跑一条");
        app.input(key(KeyCode::F(6)), &tx);
        let Request::Start(request) = rx.recv().unwrap() else {
            panic!("应该是一次 Start")
        };
        assert_eq!(request.model.as_deref(), Some("p/m"));
    }

    #[test]
    fn selecting_a_model_with_efforts_opens_the_effort_dialog() {
        let mut app = model_dialog_app();
        app.backend = BackendState::Failed { reason: "测试走独立请求那条路".into() };
        let (tx, rx) = mpsc::channel();
        type_text(&mut app, "Effort Model", &tx);
        app.input(key(KeyCode::Enter), &tx);
        let Some(Dialog::Efforts(state)) = app.dialog.clone() else {
            panic!("有强度档就要进 effort 对话框");
        };
        assert_eq!(state.model.route(), "p/m-effort");
        assert_eq!(state.efforts, 4);
        assert_eq!(
            app.model,
            Some(catalog::parse_route("p/m-effort").unwrap()),
            "模型先定下来，effort 只是它的补充"
        );
        assert!(rx.try_recv().is_err(), "选模型本身不发请求");

        // 第 0 行是「默认」，第 2 行是 "low"。
        app.input(key(KeyCode::Down), &tx);
        app.input(key(KeyCode::Down), &tx);
        app.input(key(KeyCode::Enter), &tx);
        assert_eq!(app.dialog, None);
        assert_eq!(app.effort.as_deref(), Some("low"));
        assert_eq!(
            app.model_prefs.effort(&catalog::parse_route("p/m-effort").unwrap()),
            Some("low")
        );

        app.editor.insert("跑一条");
        app.input(key(KeyCode::F(6)), &tx);
        let Request::Start(request) = rx.recv().unwrap() else {
            panic!("应该是一次 Start")
        };
        assert_eq!(request.model.as_deref(), Some("p/m-effort"));
        assert_eq!(request.effort.as_deref(), Some("low"));
    }

    #[test]
    fn choosing_the_default_effort_row_clears_the_stored_level() {
        let mut app = model_dialog_app();
        let (tx, _) = mpsc::channel();
        let route = catalog::parse_route("p/m-effort").unwrap();
        app.model_prefs.set_effort(&route, Some("high"));
        app.open_dialog(Dialog::Efforts(EffortDialogState {
            model: route.clone(),
            efforts: 4,
        }));
        app.input(key(KeyCode::Enter), &tx);
        assert_eq!(app.effort, None);
        assert_eq!(app.model_prefs.effort(&route), None);
        assert_eq!(app.dialog, None);
    }

    #[test]
    fn ctrl_f_toggles_the_favorite_of_the_selected_row_only_inside_the_dialog() {
        let mut app = model_dialog_app();
        let (tx, _) = mpsc::channel();
        type_text(&mut app, "Model N", &tx);
        app.input(key_with(KeyCode::Char('f'), KeyModifiers::CONTROL), &tx);
        assert_eq!(app.model_prefs.favorite.len(), 1);
        assert_eq!(app.model_prefs.favorite[0].route(), "q/n");
        assert!(app.notice.contains("已收藏"));
        app.input(key_with(KeyCode::Char('f'), KeyModifiers::CONTROL), &tx);
        assert!(app.model_prefs.favorite.is_empty());
        assert!(app.notice.contains("取消收藏"));

        // 会话级没有 Ctrl+F：对话框关着的时候它不该改任何东西。
        app.dialog = None;
        app.input(key_with(KeyCode::Char('f'), KeyModifiers::CONTROL), &tx);
        assert!(app.model_prefs.favorite.is_empty());
        assert_eq!(app.editor.text, "", "Ctrl+F 不是输入");
    }

    #[test]
    fn ctrl_a_opens_the_provider_list_and_enter_limits_the_model_dialog() {
        let mut app = app();
        let (tx, _) = mpsc::channel();
        open_models(&mut app, &tx);
        assert!(matches!(app.dialog, Some(Dialog::Models(_))));

        app.input(key_with(KeyCode::Char('a'), KeyModifiers::CONTROL), &tx);
        assert!(matches!(app.dialog, Some(Dialog::Providers(_))));
        let text = buffer_text(&mut app, 120, 30);
        assert!(text.contains("选择 provider"), "provider 列表标题:\n{text}");
        assert!(text.contains("Provider P") && text.contains("Provider Q"));

        app.input(key(KeyCode::Down), &tx);
        app.input(key(KeyCode::Enter), &tx);
        let Some(Dialog::Models(state)) = app.dialog else {
            panic!("选定 provider 之后回到模型对话框");
        };
        assert_eq!(state.provider_id(&app.boot.catalog), Some("q"));
        let rows = sections(&app);
        assert_eq!(rows.len(), 1, "限定 provider 时不画 Favorites/Recent 分节");
        assert_eq!(
            rows[0]
                .rows
                .iter()
                .map(|row| row.model.route())
                .collect::<Vec<_>>(),
            vec!["q/n".to_string()],
            "只列该 provider 的模型"
        );
        let text = buffer_text(&mut app, 120, 30);
        assert!(text.contains("Provider Q"), "标题里要说明限定到了哪个 provider");
        assert!(!text.contains("Provider P  "), "别的 provider 的模型不该出现");
    }

    /// provider 搜索框里的串要跟着走进模型对话框（它是同一个搜索框），
    /// 否则用户刚打的 "Provider Q" 会在按 Enter 之后消失。
    #[test]
    fn the_provider_search_text_survives_the_jump_into_the_model_dialog() {
        let mut app = app();
        let (tx, _) = mpsc::channel();
        open_models(&mut app, &tx);
        app.input(key_with(KeyCode::Char('a'), KeyModifiers::CONTROL), &tx);
        type_text(&mut app, "Provider Q", &tx);
        assert_eq!(app.filter, "Provider Q");
        app.input(key(KeyCode::Enter), &tx);
        let Some(Dialog::Models(state)) = app.dialog else {
            panic!("应该回到模型对话框");
        };
        assert_eq!(state.provider_id(&app.boot.catalog), Some("q"));
        assert_eq!(app.filter, "Provider Q", "搜索串不该被清掉");
    }

    /// provider 列表里的 Esc 先退回模型对话框（对话框底部也是这么写的），
    /// 再按一次才关掉整个流程。
    #[test]
    fn escape_returns_from_the_provider_list_to_the_model_dialog() {
        let mut app = app();
        let (tx, _) = mpsc::channel();
        open_models(&mut app, &tx);
        app.input(key_with(KeyCode::Char('a'), KeyModifiers::CONTROL), &tx);
        assert!(matches!(app.dialog, Some(Dialog::Providers(_))));
        app.input(key(KeyCode::Esc), &tx);
        assert!(matches!(app.dialog, Some(Dialog::Models(_))));
        app.input(key(KeyCode::Esc), &tx);
        assert_eq!(app.dialog, None);
    }

    #[test]
    fn f2_cycles_recent_without_reordering_it_and_shift_f2_goes_back() {
        let mut app = app();
        let (tx, _) = mpsc::channel();
        let first = catalog::parse_route("p/m").unwrap();
        let second = catalog::parse_route("q/n").unwrap();
        app.model_prefs.touch_recent(&second);
        app.model_prefs.touch_recent(&first);

        app.input(key(KeyCode::F(2)), &tx);
        assert_eq!(app.model, Some(second.clone()));
        assert_eq!(
            app.model_prefs.recent,
            vec![first.clone(), second.clone()],
            "切换不能顺手改 Recent 的顺序，否则连按两次会原地跳"
        );
        app.input(key_with(KeyCode::F(2), KeyModifiers::SHIFT), &tx);
        assert_eq!(app.model, Some(first));
        assert!(app.notice.contains("下一次请求"), "要说明何时生效：{}", app.notice);

        // 环回到头的另一侧。
        app.input(key_with(KeyCode::F(2), KeyModifiers::SHIFT), &tx);
        assert_eq!(app.model, Some(second));
    }

    #[test]
    fn dialog_render_says_the_model_switch_applies_to_the_next_request() {
        let mut app = model_dialog_app();
        let text = buffer_text(&mut app, 120, 30);
        assert!(
            text.contains("下一次请求生效"),
            "§7.2 要求界面上说清不是热切换：\n{text}"
        );
        assert!(text.contains("不会热切换"));
        // 对话框底部要说清这两个上下文键位（§7.4）以及 Esc 之后仍然用哪个模型。
        assert!(text.contains("^F 收藏"));
        assert!(text.contains("^A provider"));
        assert!(text.contains("仍用 Model M"), "要说清 Esc 关闭后当前模型是谁");
    }

    #[test]
    fn current_model_reaches_the_disk_after_selecting_and_touching_recent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.json");
        let mut app = App::new(
            boot(Some("p/m"), path.clone()),
            Preferences::default(),
            ModelPrefs::default(),
        );
        let (tx, _) = mpsc::channel();
        app.open_dialog(Dialog::Models(ModelDialogState::default()));
        type_text(&mut app, "Model N", &tx);
        // 收藏要在对话框还开着的时候按（Enter 之后对话框就关了）。
        app.input(key_with(KeyCode::Char('f'), KeyModifiers::CONTROL), &tx);
        app.input(key(KeyCode::Enter), &tx);

        assert!(app.model_prefs_dirty);
        app.model_prefs.save(&path).expect("保存偏好");
        let saved = ModelPrefs::load(&path);
        assert_eq!(
            saved.recent.first().map(ModelRef::route).as_deref(),
            Some("q/n")
        );
        assert_eq!(
            saved.favorite.first().map(ModelRef::route).as_deref(),
            Some("q/n")
        );
    }

    #[test]
    fn info_line_and_metadata_show_the_current_model_name() {
        let mut app = app();
        assert_eq!(app.model_name(), "Model M");
        let text = buffer_text(&mut app, 120, 30);
        assert!(text.contains("Model M"), "输入区 metadata 要用展示名:\n{text}");
    }

    #[test]
    fn request_is_independent_and_current_execution_cannot_be_reconfigured() {
        let mut app = app();
        app.backend = BackendState::Failed { reason: "测试走独立请求那条路".into() };
        let (tx, rx) = mpsc::channel();
        app.editor.insert("第一条");
        app.input(key(KeyCode::F(6)), &tx);
        let Request::Start(first) = rx.recv().unwrap() else {
            panic!()
        };
        app.editor.insert("/permission full");
        app.input(key(KeyCode::F(6)), &tx);
        assert_eq!(app.permission, "workspace");
        app.update(Update::Finished(first.id.clone()));
        app.editor.insert("第二条");
        app.input(key(KeyCode::F(6)), &tx);
        let Request::Start(second) = rx.recv().unwrap() else {
            panic!()
        };
        assert_eq!(second.prompt, "第二条");
        assert_eq!(second.agent, "echo");
        assert_ne!(first.id, second.id);
    }
    /// §7.4's Esc layering: popup → dialog → editing state → cancel. The draft survives the
    /// dialog closing, so a second Esc is what clears it — and only a third cancels a running
    /// job. The old order cancelled first, which made a stray Esc while typing end the run.
    #[test]
    fn paste_does_not_dispatch_and_modal_escape_does_not_cancel() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        app.input(Event::Paste("一\n二".into()), &tx);
        assert!(rx.try_recv().is_err(), "a paste never dispatches");
        app.active = Some("r".into());
        app.selected = app.active.clone();
        app.open_dialog(Dialog::Commands);
        app.input(key(KeyCode::Esc), &tx);
        assert_eq!(app.dialog, None, "the first Esc closes the dialog");
        assert!(rx.try_recv().is_err());
        app.input(key(KeyCode::Esc), &tx);
        assert_eq!(app.editor.text, "", "the second Esc cancels the edit");
        assert!(rx.try_recv().is_err(), "clearing the draft must not cancel the run");
        app.input(key(KeyCode::Esc), &tx);
        assert!(matches!(rx.recv().unwrap(), Request::Cancel(id) if id == "r"));
    }
    #[test]
    fn plan_and_confirm_do_not_pretend_to_be_supported() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        for cmd in ["/plan", "/permission confirm"] {
            app.editor.insert(cmd);
            app.input(key(KeyCode::F(6)), &tx);
            assert!(rx.try_recv().is_err());
            assert_eq!(app.agent, "echo");
            assert_eq!(app.permission, "workspace");
        }
    }
    #[test]
    fn scrolling_stays_anchored_when_new_data_arrives() {
        let mut app = app();
        let (tx, _) = mpsc::channel();
        app.scroll_max = 30;
        app.input(key(KeyCode::PageUp), &tx);
        assert_eq!(app.scroll, Some(22));
        app.scroll_max = 40;
        app.update(Update::Runs(Vec::new()));
        assert_eq!(app.scroll, Some(22));
    }
    #[cfg(windows)]
    #[test]
    fn native_multiline_paste_cannot_dispatch_a_prompt() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        for text in ["第一行", "第二行", "", "/exit"] {
            for c in text.chars() {
                app.input(key(KeyCode::Char(c)), &tx);
            }
            assert!(!app.input(key(KeyCode::Enter), &tx));
        }
        assert!(rx.try_recv().is_err());
        assert_eq!(app.editor.text, "第一行\n第二行\n\n/exit\n");
    }

    fn with_theme(theme: Option<&str>, light: bool) -> App {
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
            Preferences { light, theme: theme.map(str::to_string), ..Preferences::default() },
            crate::catalog::ModelPrefs::default(),
        )
    }

    /// `tui-preferences.json` shipped with only `light` and `sidebar`. `#[serde(default)]` is
    /// what keeps that file readable after `theme` was added; without it the whole file fails to
    /// parse and every preference silently resets.
    #[test]
    fn an_older_preferences_file_still_loads() {
        let old: Preferences = serde_json::from_str(r#"{"light":true,"sidebar":false}"#).unwrap();
        assert!(old.light);
        assert!(!old.sidebar);
        assert_eq!(old.theme, None, "an absent theme means `never chosen`");
        assert_eq!(old.theme_name(), DEFAULT_THEME);
        assert_eq!(old.mode(), crate::theme::Mode::Light);

        // And the fields the picker writes round-trip.
        let text = serde_json::to_string(&Preferences {
            light: true,
            sidebar: false,
            theme: Some("nord".into()),
            ..Preferences::default()
        })
        .unwrap();
        let back: Preferences = serde_json::from_str(&text).unwrap();
        assert_eq!(back.theme_name(), "nord");
    }

    /// A name that is not in the registry is a warning, not a startup failure:
    /// `tui-preferences.json` is hand-editable, and a typo there must not stop the TUI opening.
    #[test]
    fn an_unknown_theme_name_falls_back_instead_of_failing_to_start() {
        for name in ["", "Nope", "opencode-v1", "一"] {
            let app = with_theme(Some(name), false);
            assert_eq!(app.theme.name, DEFAULT_THEME, "{name:?} should have fallen back");
            assert!(app.notice.contains(DEFAULT_THEME), "the notice should say what it used");
        }
        // The invalid name is kept as written so the warning repeats rather than the typo being
        // rewritten to the default behind the user's back.
        let app = with_theme(Some("Nope"), false);
        assert_eq!(app.preferences.theme.as_deref(), Some("Nope"));
    }

    /// `/theme` now opens the picker and `/mode` owns the dark/light toggle. Both rows live in
    /// `commands::ENTRIES`, the single source the slash parser, the palette and the keymap share.
    #[test]
    fn the_mode_toggle_moved_to_slash_mode() {
        let theme = commands::entry(Action::Theme);
        assert_eq!(theme.slash, Some("/theme"));
        let mode = commands::entry(Action::Mode);
        assert_eq!(mode.slash, Some("/mode"));

        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        assert_eq!(app.theme.mode, crate::theme::Mode::Dark);
        app.editor.insert("/mode");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(app.preferences.light);
        assert_eq!(app.theme.mode, crate::theme::Mode::Light);
        assert!(app.notice.contains("light"), "the notice should name the mode: {}", app.notice);
    }

    /// `onMove` previews live and `onCleanup` restores unless Enter confirmed. The preview is a
    /// real theme change through the same code path the rest of the UI uses, so it has to be
    /// visible on `App::theme` and reversible by Esc.
    #[test]
    fn the_picker_previews_live_and_esc_puts_the_theme_back() {
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        let initial = app.theme.name.clone();
        app.editor.insert("/theme");
        app.input(key(KeyCode::F(6)), &tx);
        assert_eq!(app.dialog, Some(Dialog::Theme));
        assert!(app.theme_dialog.is_some());

        app.input(key(KeyCode::Down), &tx);
        let previewed = app.theme.name.clone();
        assert_ne!(previewed, initial, "↓ must preview without an Enter");
        assert_eq!(app.preferences.theme.as_deref(), Some(previewed.as_str()));

        app.input(key(KeyCode::Esc), &tx);
        assert_eq!(app.theme.name, initial, "Esc must restore the theme that was open");
        assert_eq!(app.preferences.theme.as_deref(), Some(initial.as_str()));
        assert_eq!(app.dialog, None);
        assert!(app.theme_dialog.is_none());
    }

    /// Enter commits — and the commit is what survives the close, so `restore` must not undo it.
    #[test]
    fn enter_in_the_picker_commits_the_theme_under_the_cursor() {
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        app.open_dialog(Dialog::Theme);
        app.input(key(KeyCode::Down), &tx);
        app.input(key(KeyCode::Down), &tx);
        let chosen = app.theme.name.clone();
        app.input(key(KeyCode::Enter), &tx);
        assert_eq!(app.dialog, None);
        assert_eq!(app.theme.name, chosen);
        assert_eq!(app.preferences.theme.as_deref(), Some(chosen.as_str()));
        assert!(app.notice.contains(&chosen), "the notice should confirm the choice: {}", app.notice);
        // A second Esc (no dialog open) must not undo the commit.
        app.input(key(KeyCode::Esc), &tx);
        assert_eq!(app.theme.name, chosen);
    }

    /// Typing filters the list and previews the first match; clearing the query puts the opening
    /// theme back. Backspace is the only way to clear the query from the keyboard.
    #[test]
    fn typing_in_the_picker_filters_and_clearing_restores() {
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        let initial = app.theme.name.clone();
        app.open_dialog(Dialog::Theme);
        for c in "drac".chars() {
            app.input(key(KeyCode::Char(c)), &tx);
        }
        assert_eq!(app.theme.name, "dracula");
        for _ in 0..4 {
            app.input(key(KeyCode::Backspace), &tx);
        }
        assert_eq!(app.theme.name, initial);
        // A paste filters the same way typing does, newlines dropped.
        app.input(Event::Paste("nor\nd".into()), &tx);
        assert_eq!(app.theme.name, "nord");
    }

    /// The leader window is wall-clock, not "until the next key": `tick` is what the event loop
    /// calls between polls, and a leader that never expired would eat an unrelated keystroke.
    #[test]
    fn a_leader_left_alone_expires_after_the_timeout() {
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        app.input(key_with(KeyCode::Char('x'), KeyModifiers::CONTROL), &tx);
        assert!(app.leader_active());
        app.leader_until = Some(Instant::now() - Duration::from_millis(1));
        app.tick();
        assert!(!app.leader_active(), "an expired leader must be forgotten");
        // And the next keystroke goes back to the composer instead of being read as a chord.
        app.input(key(KeyCode::Char('h')), &tx);
        assert_eq!(app.editor.text, "h");
    }

    #[test]
    fn slash_completion_filters_accepts_with_tab_and_closes_on_escape() {
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        app.input(key(KeyCode::Char('/')), &tx);
        let popup = app.autocomplete.as_ref().expect("typing / opens completion");
        assert!(popup.items.len() >= 10, "every slash command is offered");
        type_text(&mut app, "hel", &tx);
        let popup = app.autocomplete.as_ref().unwrap();
        assert_eq!(popup.current().unwrap().insert, "/help");
        app.input(key(KeyCode::Tab), &tx);
        assert_eq!(app.editor.text, "/help", "Tab accepts without running");
        assert!(app.autocomplete.is_none(), "the popup closes after accepting");

        // Escape closes the popup without touching the draft. The trigger is the first
        // character of the draft, so this needs a fresh app rather than a second `/`.
        let mut app = self::app();
        app.input(key(KeyCode::Char('/')), &tx);
        assert!(app.autocomplete.is_some());
        app.input(key(KeyCode::Esc), &tx);
        assert!(app.autocomplete.is_none());
        assert_eq!(app.editor.text, "/", "Esc must not clear the draft while the popup is open");
        // A second Esc now clears the draft.
        app.input(key(KeyCode::Esc), &tx);
        assert_eq!(app.editor.text, "");
    }

    /// On Windows Enter is a newline, so F6 is the "accept and run" chord there. Both must
    /// route through the completion popup or the popup would stay open over the next screen.
    #[test]
    fn accepting_completion_with_the_send_key_runs_the_command() {
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        type_text(&mut app, "/the", &tx);
        assert!(app.autocomplete.is_some());
        app.input(key(KeyCode::F(6)), &tx);
        assert_eq!(app.dialog, Some(Dialog::Theme));
        assert!(app.autocomplete.is_none());
    }

    /// A command the build cannot serve still executes — and says why. Silently doing nothing
    /// is the failure mode §7.4 calls out.
    #[test]
    fn unavailable_commands_report_their_reason_instead_of_doing_nothing() {
        for (command, action) in [("/plan", Action::Plan), ("/permission confirm", Action::Permission)] {
            let mut app = app();
            let (tx, _rx) = mpsc::channel();
            app.editor.insert(command);
            app.input(key(KeyCode::F(6)), &tx);
            let expected = app.availability(action).or_else(|| {
                (action == Action::Permission).then(|| commands::CONFIRM_REASON.to_string())
            });
            assert_eq!(
                Some(app.notice.clone()),
                expected,
                "{command} must explain itself, got {}",
                app.notice
            );
        }
        // The same reason is what the palette shows when the row is selected.
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        app.open_dialog(Dialog::Commands);
        app.filter = "plan".into();
        app.choice = app
            .filtered_commands()
            .iter()
            .position(|entry| entry.action == Action::Plan)
            .unwrap();
        app.select(&tx);
        assert_eq!(app.notice, commands::PLAN_REASON);
        assert_eq!(app.dialog, None, "the palette closes so the reason is readable");
    }

    /// Walks every palette row through the palette. A row that panics, or that is present but
    /// not dispatched, fails here rather than at the user's keyboard.
    #[test]
    fn every_palette_action_can_be_triggered_from_the_palette() {
        for entry in commands::palette_entries() {
            let mut app = app();
            let (tx, _rx) = mpsc::channel();
            app.open_dialog(Dialog::Commands);
            app.filter = entry.slash.unwrap_or(entry.id).trim_start_matches('/').to_string();
            let rows = app.filtered_commands();
            let index = rows
                .iter()
                .position(|row| row.action == entry.action)
                .unwrap_or_else(|| panic!("{} is not reachable by searching its own name", entry.id));
            app.choice = index;
            // `select` returns true only for Quit; every other row must return false here.
            let quit = app.select(&tx);
            assert_eq!(quit, entry.action == Action::Quit, "{}", entry.id);
            if let Some(reason) = entry.unavailable {
                assert_eq!(app.notice, reason, "{} should report its reason", entry.id);
            }
        }
    }

    #[test]
    fn commands_with_arguments_validate_and_apply() {
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        app.editor.insert("/permission full");
        app.input(key(KeyCode::F(6)), &tx);
        assert_eq!(app.permission, "full");

        app.editor.insert("/permission bogus");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(app.notice.contains("未知授权级别"), "{}", app.notice);
        assert_eq!(app.permission, "full", "a rejected value must not be applied");

        app.editor.insert("/theme now");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(app.notice.contains("不接受参数"), "{}", app.notice);
    }

    /// `/effort` without an argument opens the picker; an argument is checked against what the
    /// model actually reports instead of being passed through blindly.
    #[test]
    fn effort_arguments_are_checked_against_the_model() {
        let mut app = App::new(
            boot(Some("p/m-effort"), PathBuf::from("unused.json")),
            Preferences::default(),
            ModelPrefs::default(),
        );
        let (tx, _rx) = mpsc::channel();
        app.editor.insert("/effort bogus");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(app.notice.contains("不是"), "{}", app.notice);
        assert!(app.notice.contains("high"), "the notice lists the real levels: {}", app.notice);
        assert_eq!(app.effort, None);

        app.editor.insert("/effort high");
        app.input(key(KeyCode::F(6)), &tx);
        assert_eq!(app.effort.as_deref(), Some("high"));

        app.editor.insert("/effort");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(matches!(app.dialog, Some(Dialog::Efforts(_))), "{:?}", app.dialog);
    }

    /// Ctrl+T walks the model's own levels and ends on the default, so one key reaches every
    /// value without a dialog.
    #[test]
    fn ctrl_t_cycles_effort_and_wraps_back_to_default() {
        let mut app = App::new(
            boot(Some("p/m-effort"), PathBuf::from("unused.json")),
            Preferences::default(),
            ModelPrefs::default(),
        );
        let (tx, _rx) = mpsc::channel();
        let mut seen = Vec::new();
        for _ in 0..5 {
            app.input(key_with(KeyCode::Char('t'), KeyModifiers::CONTROL), &tx);
            seen.push(app.effort.clone().unwrap_or_else(|| "default".into()));
        }
        assert_eq!(seen, ["off", "low", "high", "max", "default"]);
    }

    /// The same Ctrl+F must not toggle favourites from the composer. §7.4: keys resolve by
    /// context, never by globally overriding one of the meanings.
    #[test]
    fn ctrl_f_does_nothing_outside_the_model_dialog() {
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        let before = app.model_prefs.favorite.clone();
        app.input(key_with(KeyCode::Char('f'), KeyModifiers::CONTROL), &tx);
        assert_eq!(app.model_prefs.favorite, before);
        assert_eq!(app.editor.text, "", "a control chord must not insert a character");
    }

    /// `keybinds.json` sits next to the preferences file and rebinds by action id. The app must
    /// load it at startup and surface parse problems in the notice line, not refuse to start.
    #[test]
    fn keybinds_json_next_to_the_preferences_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let preferences = dir.path().join("tui-preferences.json");
        std::fs::write(dir.path().join("keybinds.json"), r#"{"theme.list":"ctrl+g"}"#).unwrap();
        let app = App::new(
            Boot {
                preferences,
                ..boot(Some("p/m"), PathBuf::from("unused.json"))
            },
            Preferences::default(),
            ModelPrefs::default(),
        );
        assert_eq!(app.keymap.display(Action::Theme).as_deref(), Some("Ctrl+G"));

        std::fs::write(dir.path().join("keybinds.json"), "{broken").unwrap();
        let app = App::new(
            Boot {
                preferences: dir.path().join("tui-preferences.json"),
                ..boot(Some("p/m"), PathBuf::from("unused.json"))
            },
            Preferences::default(),
            ModelPrefs::default(),
        );
        assert!(app.notice.contains("keybinds.json"), "{}", app.notice);
        assert_eq!(app.keymap.display(Action::Theme).as_deref(), Some("Ctrl+X T"));
    }

    /// Leaving the picker by another route must undo the live preview, exactly like Esc. The
    /// bug this guards against: Ctrl+P from the picker used to leave the previewed theme
    /// applied in memory, so the next exit persisted a theme the user never confirmed.
    #[test]
    fn opening_another_dialog_restores_the_theme_preview() {
        let mut app = app();
        let (tx, _rx) = mpsc::channel();
        let initial = app.theme.name.clone();
        app.open_dialog(Dialog::Theme);
        app.input(key(KeyCode::Down), &tx);
        assert_ne!(app.theme.name, initial, "the preview is live");
        app.input(key_with(KeyCode::Char('p'), KeyModifiers::CONTROL), &tx);
        assert_eq!(app.dialog, Some(Dialog::Commands));
        assert_eq!(app.theme.name, initial, "the unconfirmed preview must be undone");
        assert_eq!(app.preferences.theme.as_deref(), Some(initial.as_str()));
    }

    /// `@` completes against the workspace and inserts one atomic reference. The trailing
    /// space is outside the segment so the next word does not get swallowed by Backspace.
    #[test]
    fn at_references_complete_and_delete_as_one_segment() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "x").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "x").unwrap();
        let mut app = self::app();
        app.boot.workspace = dir.path().to_path_buf();
        let (tx, _rx) = mpsc::channel();
        type_text(&mut app, "看 @ma", &tx);
        let popup = app.autocomplete.as_ref().expect("@ opens files");
        assert_eq!(popup.trigger, '@');
        assert_eq!(popup.start, "看 ".len());
        assert!(
            popup.items.iter().any(|item| item.detail == "@main.rs"),
            "{:?}",
            popup.items
        );
        app.input(key(KeyCode::Tab), &tx);
        assert_eq!(app.editor.text, "看 @main.rs ");
        assert_eq!(app.editor.segments().len(), 1, "the reference is one unit");
        assert_eq!(app.preferences.recent_files.first().map(String::as_str), Some("main.rs"));

        // First Backspace eats the trailing space, second removes the whole reference.
        app.input(key(KeyCode::Backspace), &tx);
        assert_eq!(app.editor.text, "看 @main.rs");
        app.input(key(KeyCode::Backspace), &tx);
        assert_eq!(app.editor.text, "看 ");
        assert!(app.editor.segments().is_empty());
    }

    /// History is seeded from the preferences file and written back on exit; without that it
    /// would only survive as long as the process does.
    #[test]
    fn input_history_is_loaded_and_persisted_with_the_preferences() {
        let prefs = Preferences {
            history: vec!["上一次输入".into()],
            ..Preferences::default()
        };
        let mut app = App::new(
            boot(Some("p/m"), PathBuf::from("unused.json")),
            prefs,
            ModelPrefs::default(),
        );
        assert_eq!(app.editor.history, vec!["上一次输入".to_string()]);
        let (tx, _rx) = mpsc::channel();
        app.editor.insert("新输入");
        app.input(key(KeyCode::F(6)), &tx);
        app.persist_preferences();
        assert_eq!(
            app.preferences.history.last().map(String::as_str),
            Some("新输入")
        );
        // And the editor can walk back through history: the newest entry first, then the one
        // loaded from disk.
        app.editor.history(true);
        assert_eq!(app.editor.text, "新输入");
        app.editor.history(true);
        assert_eq!(app.editor.text, "上一次输入");
    }

    /// A large paste becomes a summary segment and expands to the original text on submit;
    /// `/paste` turns the summarization off. The draft never holds the paste twice.
    #[test]
    fn a_large_paste_is_summarized_and_expands_on_submit() {
        let mut app = self::app();
        app.backend = BackendState::Failed { reason: "测试走独立请求那条路".into() };
        let (tx, rx) = mpsc::channel();
        let big = (0..10).map(|i| format!("行{i}\n")).collect::<String>();
        app.input(Event::Paste(big.clone()), &tx);
        assert_eq!(app.editor.text, "[粘贴 10 行]");
        app.input(key(KeyCode::F(6)), &tx);
        let Request::Start(request) = rx.recv().unwrap() else {
            panic!()
        };
        assert_eq!(request.prompt.trim_end(), big.trim_end());

        // `/paste` toggles the behaviour; the same paste is then inserted literally.
        let mut app = self::app();
        let (tx, _rx) = mpsc::channel();
        app.editor.insert("/paste");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(!app.preferences.paste_summary);
        app.input(Event::Paste(big.clone()), &tx);
        assert_eq!(app.editor.text, big);
    }

    #[test]
    fn leader_e_asks_run_to_open_the_external_editor() {
        let mut app = self::app();
        let (tx, _rx) = mpsc::channel();
        app.input(key_with(KeyCode::Char('x'), KeyModifiers::CONTROL), &tx);
        app.input(key(KeyCode::Char('e')), &tx);
        assert!(app.take_editor_request());
        assert!(!app.take_editor_request(), "the request is consumed once");
    }

    /// `/help` is generated from the registry, so a rebound key shows its new chord and an
    /// unavailable action carries its reason — no second hand-written key list.
    #[test]
    fn help_rows_come_from_the_keymap_and_the_registry() {
        let app = app();
        let rows = app.help_rows();
        let quit = rows.iter().find(|row| row.title.contains("退出")).unwrap();
        assert!(quit.keys.contains("Ctrl+C"), "{}", quit.keys);
        let plan = rows.iter().find(|row| row.title.contains("Plan")).unwrap();
        assert_eq!(plan.reason.as_deref(), Some(commands::PLAN_REASON));
        // Context is part of the row: Ctrl+F is a model-dialog binding, not a global one.
        let favorite = rows.iter().find(|row| row.title.contains("收藏")).unwrap();
        assert_eq!(favorite.context, commands::Context::ModelDialog);
        // Every registry row is documented exactly once.
        assert_eq!(rows.len(), commands::ENTRIES.len());
    }

    // ── 交互会话（Remote）那条路 ──────────────────────────────────────────────

    fn ready(app: &mut App) {
        app.update(Update::Session(service::Event::Backend {
            state: BackendState::Ready {
                model: Some(ModelChoice { provider: "p".into(), model: "m".into(), effort: None }),
            },
        }));
    }

    fn conversation(id: &str, running: bool, messages: Vec<Message>) -> Update {
        Update::Session(service::Event::Conversation {
            conversation: Box::new(Conversation {
                session_id: id.into(),
                messages,
                running,
                history_complete: true,
                ..Conversation::default()
            }),
        })
    }

    fn user(text: &str) -> Message {
        Message::User(arc_deck_core::conversation::UserMessage { turn: 1, text: text.into(), ..Default::default() })
    }

    #[test]
    fn a_prompt_goes_to_the_session_service_and_continues_the_same_session() {
        let mut app = app();
        ready(&mut app);
        let (tx, rx) = mpsc::channel();
        app.editor.insert("第一问");
        app.input(key(KeyCode::F(6)), &tx);
        let Request::Session(Command::Prompt { session_id, text, model }) = rx.recv().unwrap() else {
            panic!("交互内核就绪时提交应当走会话")
        };
        assert_eq!(session_id, None, "没有当前会话：由服务新建");
        assert_eq!(text, "第一问");
        assert_eq!(model.map(|m| format!("{}/{}", m.provider, m.model)).as_deref(), Some("p/m"));
        assert!(app.in_conversation(), "发出去之后立刻进会话视图（乐观回显）");
        assert_eq!(app.pending.as_deref(), Some("第一问"));

        // 新会话建好、视图里出现了这条：回显清掉。
        app.update(Update::Session(service::Event::SessionStarted { session_id: "s1".into() }));
        app.update(conversation("s1", true, vec![user("第一问")]));
        assert_eq!(app.pending, None);
        assert_eq!(app.session_id.as_deref(), Some("s1"));

        // 运行中再发：同一个会话，内核排到下一轮。
        app.editor.insert("第二问");
        app.input(key(KeyCode::F(6)), &tx);
        let Request::Session(Command::Prompt { session_id, .. }) = rx.recv().unwrap() else { panic!() };
        assert_eq!(session_id.as_deref(), Some("s1"));
        assert!(app.notice.contains("下一轮"), "{}", app.notice);
    }

    #[test]
    fn escape_cancels_the_running_turn_of_the_current_session() {
        let mut app = app();
        ready(&mut app);
        let (tx, rx) = mpsc::channel();
        app.session_id = Some("s1".into());
        app.update(conversation("s1", true, vec![user("hi")]));
        app.input(key(KeyCode::Esc), &tx);
        assert!(matches!(rx.recv().unwrap(), Request::Session(Command::Cancel { session_id }) if session_id == "s1"));
        // 一轮结束之后 Esc 什么都不发。
        app.update(conversation("s1", false, vec![user("hi")]));
        app.input(key(KeyCode::Esc), &tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_kernel_that_fails_to_start_returns_the_draft_and_falls_back_to_independent_runs() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        app.editor.insert("还没起来就发了");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(matches!(rx.recv().unwrap(), Request::Session(Command::Prompt { .. })));
        app.update(Update::Session(service::Event::Backend {
            state: BackendState::Failed { reason: "DSH 运行时未安装：先运行 `arcdeck runtime install`".into() },
        }));
        assert_eq!(app.editor.text, "还没起来就发了", "没有去处的消息要放回输入框，不能吞掉");
        assert!(app.notice.contains("runtime install"), "{}", app.notice);
        app.input(key(KeyCode::F(6)), &tx);
        assert!(matches!(rx.recv().unwrap(), Request::Start(_)), "失败之后退回独立请求");
    }

    #[test]
    fn changing_the_permission_restarts_the_kernel_and_readonly_is_selectable() {
        let mut app = app();
        ready(&mut app);
        let (tx, rx) = mpsc::channel();
        app.editor.insert("/permission readonly");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(matches!(rx.recv().unwrap(), Request::RestartBackend { permission } if permission == "readonly"));
        assert_eq!(app.permission, "readonly");
        assert_eq!(app.backend, BackendState::Starting);
        // 一轮进行中不能改：要重启内核，会打断它。
        app.session_id = Some("s1".into());
        app.update(conversation("s1", true, vec![]));
        app.editor.insert("/permission full");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(rx.try_recv().is_err());
        assert_eq!(app.permission, "readonly");
    }

    #[test]
    fn the_session_list_shows_this_workspace_only_and_enter_opens_one() {
        let mut app = app();
        ready(&mut app);
        let (tx, rx) = mpsc::channel();
        let here = app.boot.workspace.display().to_string();
        let row = |id: &str, cwd: &str, parent: Option<&str>| SessionSummary {
            session_id: id.into(),
            title: Some(format!("标题 {id}")),
            cwd: Some(cwd.into()),
            updated_at: 1,
            parent_session_id: parent.map(str::to_string),
            ..SessionSummary::default()
        };
        app.update(Update::Session(service::Event::Sessions {
            sessions: vec![
                row("mine", &here.to_uppercase(), None),
                row("elsewhere", "/somewhere/else", None),
                row("child", &here, Some("mine")),
            ],
        }));
        app.editor.insert("/sessions");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(matches!(rx.recv().unwrap(), Request::Session(Command::ListSessions)));
        assert_eq!(app.dialog, Some(Dialog::Sessions));
        let ids: Vec<&str> = app.filtered_sessions().iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, ["mine"], "别的目录与子 agent 会话不列");
        app.input(key(KeyCode::Enter), &tx);
        assert!(matches!(rx.recv().unwrap(), Request::Session(Command::OpenSession { session_id }) if session_id == "mine"));
        assert_eq!(app.session_id.as_deref(), Some("mine"));
        assert!(app.in_conversation());
    }

    #[test]
    fn the_kernel_catalog_replaces_the_config_catalog_and_keeps_efforts() {
        let mut app = app();
        app.update(Update::Session(service::Event::Catalog {
            catalog: ModelCatalog {
                providers: vec![arc_deck_core::conversation::CatalogProvider {
                    id: "opencode-go".into(),
                    name: "OpenCode Go".into(),
                    models: vec![arc_deck_core::conversation::CatalogModel {
                        id: "deepseek-v4.1-flash".into(),
                        name: "DeepSeek V4.1 Flash".into(),
                        efforts: vec![arc_deck_core::conversation::CatalogEffort { id: "high".into(), ..Default::default() }],
                        ..Default::default()
                    }],
                }],
                ..ModelCatalog::default()
            },
        }));
        assert_eq!(app.boot.catalog.len(), 1);
        assert_eq!(app.boot.catalog[0].models[0].efforts, ["high"]);
    }

    #[test]
    fn the_conversation_renders_folded_thinking_visible_failures_and_the_model_error() {
        use arc_deck_core::conversation::{AssistantTurn, Block, TextPart, ToolCall, ToolStatus, TurnState};
        let mut app = app();
        ready(&mut app);
        app.session_id = Some("s1".into());
        let turn = AssistantTurn {
            turn: 1,
            blocks: vec![
                Block::Reasoning(TextPart { text: "先想一想".into(), streaming: false, step: 1 }),
                Block::Tool(ToolCall {
                    call_id: "c1".into(),
                    name: "write".into(),
                    arguments: r#"{"path":"x.txt"}"#.into(),
                    status: ToolStatus::Failed,
                    output: Some("[sandbox: file access denied under read-only mode]".into()),
                    ..ToolCall::default()
                }),
                Block::Text(TextPart { text: "写不了".into(), streaming: false, step: 2 }),
            ],
            state: TurnState::Failed { message: "Insufficient Balance".into(), code: Some("QUOTA".into()) },
            ..AssistantTurn::default()
        };
        app.update(conversation("s1", false, vec![user("写个文件"), Message::Assistant(turn)]));
        let text = buffer_text(&mut app, 120, 30);
        assert!(text.contains("写个文件") && text.contains("写不了"), "{text}");
        assert!(text.contains("思考 · 4 字"), "思考默认折叠：{text}");
        assert!(!text.contains("先想一想"), "折叠时不显示思考原文：{text}");
        assert!(text.contains("write x.txt · 失败"), "{text}");
        assert!(text.contains("file access denied"), "失败在折叠态也要看得见：{text}");
        assert!(text.contains("[QUOTA] Insufficient Balance"), "模型失败原文要到界面上（P3）：{text}");
    }
}
