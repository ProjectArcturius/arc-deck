use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arc_deck_core::conversation::{
    ApprovalOutcome, ApprovalRequest, Conversation, Message, ModelCatalog, ModelChoice, SearchHit,
    SessionSummary,
};
use arc_deck_core::mode::{AgentId, Mode};
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
    toolview,
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
    /// 运行中改模式 / Agent 之前的一句确认（§7.4 最后一段：先结束/取消再影响下一轮）。
    Confirm(ConfirmKind),
    /// 内核在等一个人回答的审批。`Esc` 只是收起它，**不等于允许**。
    Approval(Box<ApprovalRequest>),
    /// 会话内容检索的结果。
    Search,
    /// 本会话的用户消息时间线，选中后滚到那一条。
    Timeline,
    /// 子 agent 会话：当前会话派出的子会话；在子会话里时第一行是「回到父会话」。
    Subagents,
    /// 暂存的草稿，Enter 取回到输入框。
    Stash,
    /// 工具的完整输出 / 本会话的 diff（可滚动、可搜索）。
    Viewer(Viewer),
    Models(ModelDialogState),
    Providers(ProviderDialogState),
    Efforts(EffortDialogState),
    Theme,
}

/// 哪个改动在等确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfirmKind {
    Mode(Mode),
    Agent(AgentId),
}
/// 全屏查看器里的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Viewer {
    pub title: String,
    pub lines: Vec<String>,
    /// 已经滚到第几行（0 = 顶部）。
    pub scroll: usize,
    /// 过滤串：只显示命中的行。空串显示全部。
    pub filter: String,
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

/// 一条短暂提示。错误不过期 —— 「复制失败」这种话说一遍就没了，等于没说。
/// 子 agent 对话框的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubagentRow {
    pub session_id: String,
    pub label: String,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Toast {
    pub text: String,
    pub error: bool,
    pub until: Instant,
}

/// 工作区的 git 概况。只用来显示，不参与判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitStatus {
    pub branch: String,
    /// `git status --porcelain` 的行数（未跟踪也算，因为它同样是「工作区不干净」）。
    pub changed: usize,
}

/// 正文渲染缓存。
///
/// 为什么要有：一次绘制本该只处理可见区域，但 Markdown 排版是按宽度整体折行的，
/// 没法从中间开始算。缓存把「重排全文」降成「只在内容 / 宽度 / 展开态变了时重排」，
/// 滚动、按键、toast 到期都不再重算几百条消息（任务书 w5-polish 第 7 条）。
pub(crate) struct RenderCache {
    pub key: (u64, u16, bool, Option<String>, String, bool),
    pub lines: Vec<ratatui::text::Line<'static>>,
    pub offsets: Vec<(u32, usize)>,
}

pub(crate) struct App {    pub boot: Boot,
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
    /// 扫描条的时钟（`spinner_frame`）与上一次画到第几帧：只在「在干活」时按帧重画。
    spinner_epoch: Instant,
    spinner_seen: usize,
    /// 通知上一次变化的时刻：普通通知 8 秒后收起（见 `tick`），**故障不收**。
    notice_seen: Option<(String, Instant)>,
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
    /// 当前工作模式。Plan 落到执行层是只读（改它要重启内核）。
    pub mode: Mode,
    /// 内核在等人回答的审批，按到达顺序。`Esc` 收起界面不会把请求从这里删掉。
    pub approvals: Vec<ApprovalRequest>,
    /// 上一次内容检索的结果。
    pub search: Vec<SearchHit>,
    pub search_query: String,
    /// 每条用户消息在正文里的起始行（渲染时填，时间线跳转用）。
    pub message_offsets: Vec<(u32, usize)>,
    /// 短暂提示（toast）。错误类不自动消失。
    pub toasts: Vec<Toast>,
    /// 当前工作区的 git 分支与脏文件数。`None` = 不是 git 仓库 / 读不到。
    pub git: Option<GitStatus>,
    /// 正文渲染缓存（见 [`RenderCache`]）。
    pub render_cache: Option<RenderCache>,
    /// 内容变一次加一；缓存键的一部分。
    pub render_revision: u64,
    /// 正文全文重排过几次。**诊断计数**：缓存是否真的生效靠它判定
    /// （测试 `the_body_is_only_rebuilt_when_it_changed`）。
    pub render_builds: u64,
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
        // NO_COLOR 是标准（no-color.org）；crossterm 会照它关掉所有颜色。TUI 只剩黑白时
        // 这是**第一嫌疑**，所以在界面上直接说出来 —— 我们自己的截图就被它坑过一次（2026-10-20）。
        let no_color_notice = std::env::var_os("NO_COLOR")
            .is_some_and(|value| !value.is_empty())
            .then(|| "NO_COLOR 已设置：界面不发颜色；清掉这个环境变量再启动即可".to_string());
        let notice = boot
            .config_error
            .as_ref()
            .map(|error| match failure_hint(error) {
                Some(hint) => format!("{error}；{hint}"),
                None => error.clone(),
            })
            .or(theme_notice)
            .or(keymap_notice)
            .or(no_color_notice)
            // 没有要说的就不说：内核状态在页脚右边，键位在占位符里（2026-10-20 用户：字太多）。
            .unwrap_or_default();
        let backend = match &boot.config_error {
            Some(reason) => BackendState::Failed { reason: reason.clone() },
            None => BackendState::Starting,
        };
        // History lives in the preferences file, so it has to be seeded into the editor at
        // startup and written back on exit; the cap keeps the file from growing without bound.
        let mut editor = Editor::default();
        let keep = preferences.history.len().saturating_sub(200);
        editor.history = preferences.history[keep..].to_vec();
        // git 概况在启动时读一次；之后每一轮结束再读（见 `update`）。**不在绘制路径上跑**：
        // `git status` 在大会话里几十毫秒，放每帧里就是肉眼可见的卡顿。
        let git = git_status(&boot.workspace);
        Self {
            notice,
            notice_seen: None,
            spinner_epoch: Instant::now(),
            spinner_seen: 0,
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
            mode: Mode::Build,
            approvals: Vec::new(),
            search: Vec::new(),
            search_query: String::new(),
            message_offsets: Vec::new(),
            toasts: Vec::new(),
            git,
            render_cache: None,
            render_revision: 0,
            render_builds: 0,
        }
    }

    /// 测试用：把「通知上一次变化」的时刻往回拨。
    #[cfg(test)]
    pub(crate) fn notice_seen_at_for_test(&mut self, at: Instant) {
        if let Some((_, seen)) = &mut self.notice_seen {
            *seen = at;
        }
    }

    /// 扫描条的帧号（40 ms 一帧，与 OpenCode 的 `interval={40}` 一致）。
    pub(crate) fn spinner_frame(&self) -> usize {
        (self.spinner_epoch.elapsed().as_millis() / 40) as usize
    }

    /// 有东西在跑：会话这一轮还没结束、消息还在发、或者有工人运行。
    pub(crate) fn working(&self) -> bool {
        self.conversation_running() || self.pending.is_some() || self.active.is_some()
    }

    /// 终端窗口标题：`会话标题 · Saiku`，没有会话时只有产品名。
    pub fn window_title(&self) -> String {
        match self.conversation.as_ref().and_then(|c| c.title.as_deref()) {
            Some(title) if self.in_conversation() => format!("{} · Saiku", crate::markdown::clean(title)),
            _ => "Saiku".into(),
        }
    }

    /// 主区显示会话（而不是 Home 或工人运行详情）。
    pub fn in_conversation(&self) -> bool {
        self.session_id.is_some() || self.pending.is_some()
    }

    /// 交互会话能不能用。`Starting` 也算：命令会在服务的队列里等内核就绪。
    pub(crate) fn sessions_usable(&self) -> bool {
        matches!(self.backend, BackendState::Starting | BackendState::Ready { .. })
    }

    pub(crate) fn conversation_running(&self) -> bool {
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
                        // 就绪不另发通知：页脚右边的「内核启动中」消失就是信号。
                    }
                    BackendState::Failed { reason } => {
                        // 首次运行 / 故障时给一条**可复制**的命令，而不是只把内核的原文贴出来
                        // （任务书 w5-polish 第 3 条）。
                        let hint = failure_hint(reason).map(|hint| format!("；{hint}")).unwrap_or_default();
                        self.notice = format!("交互会话不可用（{}）{hint}；提交改为独立请求", first_line(reason));
                        // 新会话还没建好就失败：那条消息没有去处，放回输入框而不是吞掉。
                        if self.session_id.is_none()
                            && let Some(text) = self.pending.take()
                        {
                            self.editor.set_text(text);
                        }
                    }
                    BackendState::Stopped if self.session_id.is_some() => {
                        self.notice = "交互内核已停止；/reconnect 重新拉起".into();
                    }
                    _ => {}
                }
                // 内核重启 / 停掉时，挂着的审批请求已经不可能被回答了 —— 留着只会让人
                // 对着一份死请求按 y。清掉，并在需要时说明。
                if matches!(
                    state,
                    BackendState::Starting | BackendState::Failed { .. } | BackendState::Stopped
                ) && !self.approvals.is_empty()
                {
                    self.approvals.clear();
                    if matches!(self.dialog, Some(Dialog::Approval(_))) {
                        self.dialog = None;
                    }
                    self.notice = format!("{}；待答的审批已随内核重启作废", self.notice);
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
                    // 一轮结束时刷一次 git：会话里刚改过的文件要让信息栏跟上。
                    // 放在这里而不是绘制里 —— `git status` 不是每帧都该跑的东西。
                    let was_running = self.conversation.as_ref().is_some_and(|c| c.running);
                    if was_running && !conversation.running {
                        self.git = git_status(&self.boot.workspace);
                    }
                    self.conversation = Some(*conversation);
                }
            }
            E::SessionClosed { session_id, reason } => {
                if self.session_id.as_deref() == Some(session_id.as_str()) {
                    self.notice = format!("会话订阅已结束：{}", first_line(&reason));
                }
            }
            E::Approval { request } => {
                // 审批不是通知：它**必须在屏幕上**。有弹层开着时先排队，但要说清楚有几个在等。
                let name = request.tool_name.clone();
                self.approvals.push(request.clone());
                if self.dialog.is_none() {
                    self.open_dialog(Dialog::Approval(Box::new(request)));
                } else {
                    self.notice = format!(
                        "内核在等审批：{}（共 {} 个待答，Esc 关闭当前弹层后会出现）",
                        name,
                        self.approvals.len()
                    );
                }
                self.toast(format!("审批请求：{name}"), true);
            }
            E::ApprovalClosed { request_id, reason } => {
                self.approvals.retain(|item| item.request_id != request_id);
                if matches!(&self.dialog, Some(Dialog::Approval(request)) if request.request_id == request_id)
                {
                    self.dialog = None;
                }
                self.notice = reason;
            }
            E::Search { query, items } => {
                self.search_query = query;
                self.search = items;
                let count = self.search.len();
                if self.dialog.is_none() {
                    self.open_dialog(Dialog::Search);
                }
                self.notice = format!("检索命中 {count} 条（Enter 打开；没有命中是正常的）");
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
                self.render_revision = self.render_revision.wrapping_add(1);
                self.dirty = true;
            }
            Update::Detail(detail)
                if self.selected.as_deref() == Some(&detail.record.id)
                    && self.detail.as_ref() != Some(&detail) =>
            {
                self.detail = Some(detail);
                self.render_revision = self.render_revision.wrapping_add(1);
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
                self.render_revision = self.render_revision.wrapping_add(1);
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
            Some(Dialog::Search) => self.search.len(),
            Some(Dialog::Timeline) => self.user_messages().len(),
            Some(Dialog::Subagents) => self.subagent_rows().len(),
            Some(Dialog::Stash) => self.preferences.stash.len(),
            Some(Dialog::Confirm(_) | Dialog::Approval(_) | Dialog::Viewer(_)) => 1,
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

    /// 本会话的用户消息（时间线用）。带 turn 与正文，界面只取首行。
    pub fn user_messages(&self) -> Vec<(u32, String)> {
        self.conversation
            .as_ref()
            .map(|conversation| {
                conversation
                    .messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::User(user) => Some((user.turn, user.text.clone())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
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
            Action::Timeline | Action::Diff | Action::Output | Action::Subagents if !self.in_conversation() => {
                Some("先打开一个会话".to_string())
            }
            Action::Search if !self.sessions_usable() => {
                Some("交互会话不可用；内容检索需要内核".to_string())
            }
            Action::Approvals if self.approvals.is_empty() => Some("没有待答的审批".to_string()),
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
        // 收不收参数看**注册表**的 `arg`，不另写一份名单：原来这里手写了
        // `Effort | Permission | Rename`，后加的 `/search <词>` 与 `/output <序号>` 都没进名单，
        // 敲出来一律被拒成「不接受参数」（console-probe 对着真内核实测出来，2026-10-20）。
        if arg.is_some() && commands::entry(action).arg.is_none() {
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
            Action::Build => self.change_mode(Mode::Build, requests),
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
                            "完整证据：{}；可用 saiku report {}",
                            d.evidence_dir.display(),
                            d.record.id
                        )
                    })
                    .unwrap_or_else(|| "先选择一条运行记录".into())
            }
            Action::Quit => return true,
            Action::Plan => self.change_mode(Mode::Plan, requests),
            Action::Search => match arg {
                Some(query) => {
                    if !self.sessions_usable() {
                        self.notice = "交互会话不可用；内容检索需要内核".into();
                    } else {
                        let _ = requests.send(Request::Session(Command::SearchSessions {
                            query: query.to_string(),
                        }));
                        self.notice = format!("正在检索「{query}」…");
                    }
                }
                None => self.notice = "用法：/search <词>".into(),
            },
            Action::Timeline => {
                if self.in_conversation() {
                    self.open_dialog(Dialog::Timeline);
                } else {
                    self.notice = "先打开一个会话".into();
                }
            }
            Action::Stash => {
                // 输入框里有字：收起来，输入框清空（OpenCode 的 stash）。空的：列出来取回。
                // 斜杠命令走到这里时输入框已经被 `/stash` 自己清掉了，所以 `/stash` = 取回，
                // `<leader>s` / 命令面板 = 暂存。
                let draft = self.editor.text.trim().to_string();
                if !draft.is_empty() {
                    self.editor.take();
                    self.autocomplete = None;
                    self.preferences.stash.retain(|item| *item != draft);
                    self.preferences.stash.insert(0, draft);
                    self.preferences.stash.truncate(20);
                    self.toast("已暂存草稿（/stash 取回）", false);
                } else if self.preferences.stash.is_empty() {
                    self.notice = "没有暂存的草稿；输入框里有字时按 ".to_string()
                        + &self.keymap.display(Action::Stash).unwrap_or_else(|| "/stash".into())
                        + " 暂存";
                } else {
                    self.open_dialog(Dialog::Stash);
                }
            }
            Action::Subagents => {
                if self.session_id.is_none() {
                    self.notice = "先打开一个会话".into();
                } else {
                    // 子会话是内核在这一轮里建的；打开前刷新一次列表，否则刚派出的那个不在里面。
                    let _ = requests.send(Request::Session(Command::ListSessions));
                    self.open_dialog(Dialog::Subagents);
                }
            }
            Action::Approvals => match self.approvals.first().cloned() {
                Some(request) => self.open_dialog(Dialog::Approval(Box::new(request))),
                None => self.notice = "没有待答的审批".into(),
            },
            Action::Diff => match &self.conversation {
                Some(conversation) => {
                    let lines = toolview::session_diff(conversation);
                    if lines.is_empty() {
                        self.notice = "本会话还没有带 diff 的编辑（write / edit）".into();
                    } else {
                        self.open_viewer("本会话的改动（/diff）", lines);
                    }
                }
                None => self.notice = "先打开一个会话".into(),
            },
            Action::Output => self.show_output(arg),
            Action::Copy => self.copy_last(),
            Action::Reconnect => {
                if !self.sessions_usable() && matches!(self.backend, BackendState::Starting) {
                    self.notice = "内核正在启动；等它给出结果再重连".into();
                } else {
                    self.notice = "正在重启交互内核…".into();
                    self.restart_backend(requests);
                }
            }
            Action::Favorite => self.toggle_favorite(),
            Action::ProviderList => {
                if let Some(Dialog::Models(back)) = self.dialog {
                    self.swap_dialog(Dialog::Providers(ProviderDialogState { back: Some(back) }));
                }
            }
            Action::CycleModelNext => self.cycle_model(1),
            Action::CycleModelReverse => self.cycle_model(-1),
            Action::CycleAgent => self.cycle_agent(requests),
        }
        false
    }
    /// 一条短暂提示。错误不过期（`until` 放到一天之后），普通提示 4 秒后消失。
    ///
    /// 为什么错误不过期：`确认`/`复制失败` 这类话说一遍就没了，等于没说 ——
    /// 而用户这时通常正低头看别处。
    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        let until = if error {
            Instant::now() + Duration::from_secs(86_400)
        } else {
            Instant::now() + Duration::from_secs(4)
        };
        // 同一句话在短时间里重复出现时只刷新计时，不叠成一列。
        let text = text.into();
        if let Some(existing) = self.toasts.iter_mut().find(|toast| toast.text == text) {
            existing.until = until;
            return;
        }
        self.toasts.push(Toast { text, error, until });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    /// 换模式（§7.4 的六种转换）。运行中先问一句 —— 换模式要重启内核，会打断这一轮。
    fn change_mode(&mut self, mode: Mode, requests: &Sender<Request>) {
        if self.mode == mode {
            self.notice = format!("已经是 {} 模式", mode.as_str());
            return;
        }
        if self.conversation_running() {
            self.open_dialog(Dialog::Confirm(ConfirmKind::Mode(mode)));
            return;
        }
        self.apply_mode(mode, requests);
    }

    /// §7.4：`/plan` 对 Echo 只切为 echo+plan、对 Build 切为 plan+plan、对 Plan 不变；
    /// `/build` 对 Echo 只切为 echo+build、对 Plan 切为 build+build。
    fn apply_mode(&mut self, mode: Mode, requests: &Sender<Request>) {
        match (self.agent.as_str(), mode) {
            ("build", Mode::Plan) => self.agent = "plan".into(),
            ("plan", Mode::Build) => self.agent = "build".into(),
            _ => {}
        }
        self.mode = mode;
        self.restart_backend(requests);
        self.notice = match mode {
            Mode::Plan => {
                "Plan：执行层只读（写入与 shell 都会被拒，参 §5.6）；交互内核重启中，当前会话会自动重新打开".into()
            }
            Mode::Build => {
                "Build：可以实施变更（仍受授权级别约束）；交互内核重启中，当前会话会自动重新打开".into()
            }
        };
    }

    fn select_agent(&mut self, agent: AgentId, requests: &Sender<Request>) {
        if agent.as_str() == self.agent {
            self.notice = format!("已经是 {} Agent", agent.as_str());
            return;
        }
        if self.conversation_running() {
            self.open_dialog(Dialog::Confirm(ConfirmKind::Agent(agent)));
            return;
        }
        self.apply_agent(agent, requests);
    }

    /// 显式选一个 Agent 取它的默认模式（Echo / Build → build，Plan → plan）。
    ///
    /// 个性化关着时交互面一段提示词都不注入（§5.3.1），Echo 与 Build 行为相同；开着时 persona
    /// 按 Agent 装进内核（`serve::interactive_persona`），所以换 Agent 也要重启内核 —— 只换名字
    /// 不重启，内核里就还是上一个 Agent 的提示词。Plan 的差别在执行层（只读）。
    fn apply_agent(&mut self, agent: AgentId, requests: &Sender<Request>) {
        let mode = if agent == AgentId::Plan { Mode::Plan } else { Mode::Build };
        let mode_changed = mode != self.mode;
        let restart = mode_changed || (self.boot.personalization && agent.as_str() != self.agent);
        self.agent = agent.as_str().into();
        self.mode = mode;
        if restart {
            self.restart_backend(requests);
        }
        let name = crate::view::agent_label(self);
        self.notice = match (agent, restart) {
            (AgentId::Plan, _) => format!("{name}：执行层只读；交互内核重启中"),
            (_, true) => format!("{name}：交互内核重启中，当前会话会自动重新打开"),
            // 说一句，不让人以为换了 Agent 就换了做法。
            (_, false) => format!("{name}：下一轮生效（个性化未开，Echo 与 Build 在交互面上行为相同）"),
        };
    }

    /// 授权级别 / Agent / 模式都写在交互内核的 patch 里，改了就重启内核。
    fn restart_backend(&mut self, requests: &Sender<Request>) {
        if !self.sessions_usable() {
            return;
        }
        let _ = requests.send(Request::RestartBackend {
            permission: self.permission.clone(),
            agent: self.agent.clone(),
            mode: self.mode.as_str().into(),
        });
        self.backend = BackendState::Starting;
    }


    /// 回答当前这条审批。答完立刻弹出下一条 —— 内核可能同时挂了几条。
    fn answer_approval(&mut self, outcome: ApprovalOutcome, requests: &Sender<Request>) {
        let Some(request) = self.approvals.first().cloned() else {
            self.notice = "没有待答的审批".into();
            return;
        };
        let _ = requests.send(Request::Session(Command::AnswerApproval {
            request_id: request.request_id.clone(),
            outcome,
        }));
        self.notice = match outcome {
            ApprovalOutcome::AllowOnce => format!("已允许一次：{}", request.tool_name),
            _ => format!("已拒绝：{}", request.tool_name),
        };
        self.toast(self.notice.clone(), false);
        self.approvals.retain(|item| item.request_id != request.request_id);
        self.dialog = None;
        if let Some(next) = self.approvals.first().cloned() {
            self.open_dialog(Dialog::Approval(Box::new(next)));
        }
    }

    /// 打开全屏查看器（工具的完整输出 / 本会话的 diff）。
    pub fn open_viewer(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.dialog = Some(Dialog::Viewer(Viewer { title: title.into(), lines, scroll: 0, filter: String::new() }));
        self.filter.clear();
    }

    /// 把用户消息时间线滚到第 `index` 条（`message_offsets` 由渲染时填）。
    fn jump_to_message(&mut self, index: usize) {
        match self.message_offsets.get(index) {
            Some((_, line)) => {
                self.scroll = Some((*line).min(self.scroll_max));
                self.notice = format!("已跳到第 {} 条消息", index + 1);
            }
            None => self.notice = "这条消息还没渲染出来；先等会话打开".into(),
        }
        self.dialog = None;
    }

    /// `/output [n]`：第 n 个工具调用的完整输出（1 起数；默认最后一条）。
    ///
    /// 为什么是序号而不是「选中的工具块」：对话视图没有块级光标（鼠标命中还没做），
    /// 编一个「选中态」会让人以为方向键能选。序号是诚实的：列表由 `/diff` 与工具行给出。
    fn show_output(&mut self, arg: Option<&str>) {
        let Some(conversation) = &self.conversation else {
            self.notice = "先打开一个会话".into();
            return;
        };
        let calls = toolview::tool_calls(conversation);
        if calls.is_empty() {
            self.notice = "本会话还没有工具调用".into();
            return;
        }
        let index = match arg {
            Some(raw) => match raw.parse::<usize>() {
                Ok(n) if n >= 1 && n <= calls.len() => n - 1,
                _ => {
                    self.notice = format!("序号要在 1..={} 之间；/output 不带参数看最后一条", calls.len());
                    return;
                }
            },
            None => calls.len() - 1,
        };
        let call = calls[index];
        let title = format!("工具输出 {}/{} · {}", index + 1, calls.len(), call.name);
        let lines = toolview::output_text(call);
        self.open_viewer(title, lines);
    }

    /// 复制最后一条助手回复（没回复时复制输入框）。走 OSC 52 —— 不引剪贴板依赖。
    ///
    /// Windows Terminal / tmux 支持它；不支持的终端会**静默忽略**这条序列（不是错误），
    /// 所以提示里说清楚「终端不支持时剪贴板不会变」，不假装一定成功。
    fn copy_last(&mut self) {
        let text = self
            .conversation
            .as_ref()
            .and_then(|conversation| {
                conversation.messages.iter().rev().find_map(|message| match message {
                    Message::Assistant(turn) if !turn.text().trim().is_empty() => Some(turn.text()),
                    _ => None,
                })
            })
            .or_else(|| (!self.editor.text.is_empty()).then(|| self.editor.text.clone()));
        let Some(text) = text else {
            self.notice = "没有可复制的内容".into();
            return;
        };
        use std::io::Write as _;
        let sequence = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
        let result = std::io::stdout()
            .write_all(sequence.as_bytes())
            .and_then(|()| std::io::stdout().flush());
        match result {
            Ok(()) => {
                self.notice = format!(
                    "已用 OSC 52 复制 {} 字符（终端不支持时剪贴板不会变）",
                    text.chars().count()
                );
                self.toast("已复制", false);
            }
            Err(error) => {
                self.notice = format!("复制失败：{error}");
                self.toast("复制失败", true);
            }
        }
    }

    fn set_permission(&mut self, arg: &str, requests: &Sender<Request>) {
        match arg {
            "readonly" | "workspace" | "full" | "confirm" => {
                if self.permission == arg {
                    self.notice = format!("授权级别已经是 {arg}");
                    return;
                }
                self.permission = arg.into();
                if self.sessions_usable() {
                    // 授权级别落在交互内核的启动参数里（沙箱档 + 预设表 + 审批策略），只能重启内核。
                    // 会话在独立的 DSH home 里，重启后桥会把当前会话重新打开。
                    self.restart_backend(requests);
                    self.notice = if arg == "confirm" {
                        "授权级别改为 confirm：内核问「能不能做」时会弹出审批界面；交互内核重启中"
                            .into()
                    } else {
                        format!("授权级别改为 {arg}：交互内核重启中，当前会话会自动重新打开")
                    };
                } else {
                    self.notice = format!("下一次独立请求的授权级别：{arg}");
                }
            }
            other => {
                self.notice = format!("未知授权级别 `{other}`；支持 readonly / workspace / full / confirm")
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
    /// Shift+Tab：Echo → Build → Plan → Echo，走与 Agent 对话框同一条路（运行中先确认、
    /// Plan 落到执行层）。之前它只在 echo / build 之间翻、不经过 `select_agent`，提示还写着
    /// 「下一次独立请求」—— 那是工人运行那条路的说法（2026-10-20 可用性检查发现）。
    fn cycle_agent(&mut self, requests: &Sender<Request>) {
        let current = if self.mode == Mode::Plan { "plan" } else { self.agent.as_str() };
        let next = match current {
            "echo" => AgentId::Build,
            "build" => AgentId::Plan,
            _ => AgentId::Echo,
        };
        self.select_agent(next, requests);
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
            // 子 agent 会话由父会话驱动；在这里发消息要么被内核拒（one-shot），要么绕过父会话
            // 的编排。界面先说清楚，草稿留在输入框里。
            if self.parent_session().is_some() {
                self.notice = "这是子 agent 会话，只能看；/subagents 回到父会话".into();
                return false;
            }
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

    /// 打开一个已有会话：先把旧的关掉（一个前端只订阅一个），再让后端打开新的。
    /// 当前会话若是子 agent 会话，它的父会话 id。
    pub(crate) fn parent_session(&self) -> Option<&str> {
        let id = self.session_id.as_deref()?;
        self.sessions
            .iter()
            .find(|s| s.session_id == id)
            .and_then(|s| s.parent_session_id.as_deref())
    }

    /// 子 agent 对话框的行：在子会话里时先是「回到父会话」，然后是当前会话的子会话（新的在前）。
    pub(crate) fn subagent_rows(&self) -> Vec<SubagentRow> {
        let mut rows = Vec::new();
        if let Some(parent) = self.parent_session() {
            let title = self
                .sessions
                .iter()
                .find(|s| s.session_id == parent)
                .and_then(|s| s.title.clone())
                .unwrap_or_else(|| "父会话".into());
            rows.push(SubagentRow { session_id: parent.to_string(), label: format!("↩ 回到 {title}"), running: false });
        }
        if let Some(id) = self.session_id.as_deref() {
            rows.extend(
                self.sessions
                    .iter()
                    .filter(|s| s.parent_session_id.as_deref() == Some(id))
                    .map(|s| SubagentRow {
                        session_id: s.session_id.clone(),
                        label: s
                            .subagent_label
                            .clone()
                            .or_else(|| s.title.clone())
                            .unwrap_or_else(|| s.session_id.clone()),
                        running: s.running,
                    }),
            );
        }
        rows
    }

    fn open_session(&mut self, id: String, requests: &Sender<Request>) {
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
        self.message_offsets.clear();
        let _ = requests.send(Request::Session(Command::OpenSession { session_id: id }));
    }

    /// 运行中改动的确认：接受就执行，拒绝就什么都不改。
    fn resolve_confirm(&mut self, kind: ConfirmKind, accept: bool, requests: &Sender<Request>) {
        self.dialog = None;
        if !accept {
            self.notice = "已取消改动；这一轮继续".into();
            return;
        }
        match kind {
            ConfirmKind::Mode(mode) => self.apply_mode(mode, requests),
            ConfirmKind::Agent(agent) => self.apply_agent(agent, requests),
        }
    }

    /// 本会话的上下文占用：最近一轮 provider 实报的输入 token / 当前模型的窗口。
    /// 窗口没报就不算（§7.3：有模型 contextWindow 时才算百分比，没有就不显示）。
    pub fn context_usage(&self) -> Option<(u64, u64)> {
        let conversation = self.conversation.as_ref()?;
        let model = self.model.as_ref()?;
        let window = self
            .boot
            .catalog
            .iter()
            .find(|provider| provider.id == model.provider_id)?
            .models
            .iter()
            .find(|entry| entry.id == model.model_id)?
            .context_window?;
        let used = conversation
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::Assistant(turn) => turn.usage.as_ref().and_then(|usage| {
                    // 上下文 = 这一轮送进去的输入（含缓存命中）；缺就退回总和。
                    usage.input.map(|input| input + usage.cache_read.unwrap_or(0))
                }),
                _ => None,
            })?;
        (window > 0).then_some((used, window))
    }

    /// 本会话最后的 todo 清单（DSH 的 `todo_write` 工具参数）。状态只有 done / 其余。
    pub fn task_todos(&self) -> Vec<(String, bool)> {
        let Some(conversation) = &self.conversation else {
            return Vec::new();
        };
        let Some(call) = toolview::tool_calls(conversation)
            .into_iter()
            .rev()
            .find(|call| call.name == "todo_write" || call.name == "todowrite")
        else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&call.arguments) else {
            return Vec::new();
        };
        let items = value
            .get("todos")
            .and_then(|todos| todos.as_array())
            .cloned()
            .unwrap_or_default();
        items
            .iter()
            .filter_map(|item| {
                let text = item
                    .get("content")
                    .or_else(|| item.get("text"))
                    .or_else(|| item.get("title"))
                    .and_then(|value| value.as_str())?;
                let done = item
                    .get("status")
                    .and_then(|value| value.as_str())
                    .is_some_and(|status| status == "completed" || status == "done");
                Some((text.to_string(), done))
            })
            .collect()
    }

    /// 本会话改动过的文件（信息栏用）。
    pub fn changed_files(&self) -> Vec<String> {
        self.conversation
            .as_ref()
            .map(toolview::changed_files)
            .unwrap_or_default()
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
                    self.open_session(id, requests);
                    self.dialog = None;
                    self.notice = "正在打开会话（历史较长时要翻页）…".into();
                }
            }
            Some(Dialog::Search) => {
                if let Some(hit) = self.search.get(self.choice).cloned() {
                    self.open_session(hit.session_id, requests);
                    self.dialog = None;
                    self.notice = "正在打开检索命中的会话…".into();
                }
            }
            Some(Dialog::Timeline) => self.jump_to_message(self.choice),
            Some(Dialog::Stash) if self.choice < self.preferences.stash.len() => {
                let draft = self.preferences.stash.remove(self.choice);
                self.dialog = None;
                // 取回不覆盖正在写的东西：输入框里有字时接在后面。
                if !self.editor.text.is_empty() {
                    self.editor.insert("\n");
                }
                self.editor.insert(&draft);
            }
            Some(Dialog::Stash) => {}
            Some(Dialog::Subagents) => {
                if let Some(row) = self.subagent_rows().get(self.choice).cloned() {
                    self.dialog = None;
                    self.open_session(row.session_id, requests);
                }
            }
            Some(Dialog::Confirm(kind)) => self.resolve_confirm(kind, true, requests),
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
                let agent = match self.choice {
                    0 => Some(AgentId::Echo),
                    1 => Some(AgentId::Build),
                    _ => Some(AgentId::Plan),
                };
                self.dialog = None;
                if let Some(agent) = agent {
                    self.select_agent(agent, requests);
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
            Some(Dialog::Approval(_) | Dialog::Viewer(_)) => {
                // 这两个的键在 `input` 里就地处理（审批的 y/n 与查看器的滚动/过滤），
                // 走到这里说明只是一个 Enter —— 不做事，免得把它当成「确认」。
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
    /// 一次 poll 里连续到达的事件。**Windows 的粘贴没有 `Event::Paste`**：crossterm 的
    /// bracketed-paste 解析只在 unix 侧（`event/sys/unix/parse.rs`），Windows 走的是
    /// `INPUT_RECORD`，整段文本会作为**一串按键**到达，其中每个换行都是一个 `Enter`。
    ///
    /// 所以一批里多于一个事件时按「粘贴」处理：Enter 插入换行而不是提交。否则多行粘贴会在
    /// 第一个换行处发出半句话 —— 当年正是为了这个把发送键改成了 F6，代价是没人找得到发送键
    /// （用户 2026-10-20：「哪个神人会用 F6 发送」）。现在 Enter 发送，粘贴靠批处理分辨。
    ///
    /// 代价说清楚：人手极快地在同一个批里同时打出字符与 Enter 时，那一下会被当成粘贴
    /// （插入换行、不发送）。再按一次 Enter 即可 —— 这比「粘贴把半句话发出去」便宜得多。
    pub fn input_batch(&mut self, events: Vec<Event>, requests: &Sender<Request>) -> bool {
        // 「粘贴」= 一批里**按下**的键不止一个。Release 不算：Windows 上每个键都是 Press +
        // Release 两条，按得快时两条落在同一次 poll 里 —— 原来按事件条数判，一次 Enter 就被当成
        // 粘贴，Enter 变换行、补全弹层被绕过（console-probe 实测，2026-10-20）。
        let presses = events
            .iter()
            .filter(|event| matches!(event, Event::Key(key) if key.kind != KeyEventKind::Release))
            .count();
        let pasted = presses > 1;
        for event in events {
            if self.input_inner(event, requests, pasted) {
                return true;
            }
        }
        false
    }

    /// 单个事件的入口（测试与「一次只有一个键」的调用方用）。运行循环走
    /// [`App::input_batch`]，因为 Windows 的粘贴要靠整批才分得出来。
    #[cfg(test)]
    pub fn input(&mut self, event: Event, requests: &Sender<Request>) -> bool {
        self.input_inner(event, requests, false)
    }

    fn input_inner(&mut self, event: Event, requests: &Sender<Request>, pasted: bool) -> bool {
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
                // 鼠标**只**用来滚轮。点选消息 / 块级命中**刻意不做**：用户 2026-10-20 裁定
                // 自己是纯键盘习惯，加鼠标点选既复杂又讨厌。要复制用 `/copy`（OSC 52）。
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
            // 审批的 Esc 只收起界面，**不回答**。把它当成「拒绝」会让一次误按替内核做了决定；
            // 当成「允许」更糟。请求留在 `approvals` 里，`/approvals` 能再打开。
            if matches!(self.dialog, Some(Dialog::Approval(_))) {
                self.dialog = None;
                self.notice = format!(
                    "审批还没回答：内核仍在等待（{} 个待答，/approvals 重新打开；Esc 不等于允许）",
                    self.approvals.len()
                );
                return false;
            }
            if matches!(self.dialog, Some(Dialog::Confirm(_))) {
                self.dialog = None;
                self.notice = "已取消改动；这一轮继续".into();
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
            // 审批与确认只有「准 / 不准」两种回答，键位写在对话框里。
            if let Some(Dialog::Confirm(kind)) = self.dialog {
                match key.code {
                    KeyCode::Enter | KeyCode::Char('y') => self.resolve_confirm(kind, true, requests),
                    KeyCode::Char('n') => self.resolve_confirm(kind, false, requests),
                    _ => {}
                }
                return false;
            }
            if matches!(self.dialog, Some(Dialog::Approval(_))) {
                match key.code {
                    KeyCode::Enter | KeyCode::Char('y') => {
                        self.answer_approval(ApprovalOutcome::AllowOnce, requests)
                    }
                    KeyCode::Char('n') | KeyCode::Backspace => {
                        self.answer_approval(ApprovalOutcome::Reject, requests)
                    }
                    _ => {}
                }
                return false;
            }
            if matches!(self.dialog, Some(Dialog::Viewer(_))) {
                let total = match &self.dialog {
                    Some(Dialog::Viewer(viewer)) => viewer.lines.len(),
                    _ => 0,
                };
                if let Some(Dialog::Viewer(viewer)) = self.dialog.as_mut() {
                    match key.code {
                        KeyCode::Up => viewer.scroll = viewer.scroll.saturating_sub(1),
                        KeyCode::Down => {
                            viewer.scroll = (viewer.scroll + 1).min(total.saturating_sub(1))
                        }
                        KeyCode::PageUp => viewer.scroll = viewer.scroll.saturating_sub(10),
                        KeyCode::PageDown => {
                            viewer.scroll = (viewer.scroll + 10).min(total.saturating_sub(1))
                        }
                        KeyCode::Backspace => {
                            viewer.filter.pop();
                            viewer.scroll = 0;
                        }
                        KeyCode::Char(c) if !ctrl && !alt => {
                            viewer.filter.push(c);
                            viewer.scroll = 0;
                        }
                        _ => {}
                    }
                }
                return false;
            }
            // Permissions is a fixed four-row list; typing must not filter it. The same holds
            // for the approval/confirm/search/timeline dialogs, which have their own cursors.
            let filterable = !matches!(
                self.dialog,
                Some(Dialog::Permissions | Dialog::Search | Dialog::Timeline | Dialog::Confirm(_) | Dialog::Approval(_) | Dialog::Viewer(_))
            );
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
        // or the submit path. A paste bypasses it: the popup is about the draft, and a pasted
        // blob is not a command being typed.
        if self.autocomplete.is_some() && !pasted {
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
                KeyCode::Enter => {
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
            // Shift+Enter 换行：Windows 的 `INPUT_RECORD` 带 SHIFT_PRESSED，所以 crossterm
            // 能区分它（不需要 kitty 协议）；终端分不出来时会当成普通 Enter 发送。
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.editor.insert("\n");
                self.refresh_autocomplete();
            }
            // 一次粘贴里的换行：插入，不提交。
            KeyCode::Enter if pasted => {
                self.editor.insert("\n");
                self.refresh_autocomplete();
            }
            // Enter 发送（§7.4 的「Enter 提交」；OpenCode 同）。F6 保留成兼容别名，
            // 不再出现在任何提示里。
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
        let takes_arg = item.action.is_some_and(|action| commands::entry(action).arg.is_some());
        if run && !item.insert.ends_with(' ') {
            let _ = self.submit(requests);
        } else if !run && takes_arg && !item.insert.ends_with(' ') {
            // Tab 是「补全、我还要写」：能带参数的命令补一个空格，接着就能打参数。
            self.editor.insert(" ");
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
        // 扫描条要动就得重画；只在干活时 —— 空闲时一帧都不多画（参 K4 的重绘门控）。
        if self.working() {
            let frame = self.spinner_frame();
            if frame != self.spinner_seen {
                self.spinner_seen = frame;
                self.dirty = true;
            }
        }
        // 通知到期。`notice` 有六十多处直接赋值，所以不在每处记时间，而是在这里看它变没变。
        // 内核不可用 / 配置坏了的那条不收：那是「现在用不了」，不是「刚刚发生了什么」。
        // 后端报的错误（`错误：…`）也不收：探针实测过，8 秒一到错误就没了，事后只剩一个空页脚。
        let sticky = matches!(self.backend, BackendState::Failed { .. })
            || self.boot.config_error.is_some()
            || self.notice.starts_with("错误");
        match &self.notice_seen {
            _ if self.notice.is_empty() => self.notice_seen = None,
            Some((text, at)) if *text == self.notice => {
                if !sticky && at.elapsed() >= Duration::from_secs(8) {
                    self.notice.clear();
                    self.notice_seen = None;
                    self.dirty = true;
                }
            }
            _ => self.notice_seen = Some((self.notice.clone(), Instant::now())),
        }
        // toast 到期。错误类把 `until` 放到一天之后，这里不会收掉它们。
        let now = Instant::now();
        let before = self.toasts.len();
        self.toasts.retain(|toast| toast.until > now);
        if self.toasts.len() != before {
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
                // 没有键位时写它的斜杠命令 —— 那才是「怎么用」；一列「—」只是噪声。
                let keys = self
                    .keymap
                    .display(entry.action)
                    .or_else(|| entry.slash.map(str::to_string))
                    .unwrap_or_default();
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

/// 工作区的 git 概况。不是 git 仓库、没装 git、命令失败都返回 `None` ——
/// 信息栏据此整块省掉，**不显示一个「unknown」占位**（§7.3：没有数据的区域省略）。
pub(crate) fn git_status(workspace: &std::path::Path) -> Option<GitStatus> {
    fn run(workspace: &std::path::Path, args: &[&str]) -> Option<String> {
        let mut command = std::process::Command::new("git");
        command
            .args(args)
            .current_dir(workspace)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        // Windows 上不带这个标志会闪一个控制台窗口（用户明确不接受，见 b227176）。
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let output = command.output().ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8(output.stdout).ok()
    }
    let branch = run(workspace, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
    if branch.is_empty() {
        return None;
    }
    let changed = run(workspace, &["status", "--porcelain"])
        .map(|text| text.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0);
    Some(GitStatus { branch, changed })
}

/// 路径比较用：大小写与分隔符不算差别（Windows 上 `C:\\ws` 与 `c:/ws` 是同一个目录）。
fn normalize_path(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// 标准 base64（RFC 4648）。只为了 OSC 52 —— 为一行复制引一个依赖不值得。
pub(crate) fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

fn first_line(text: &str) -> &str {
    text.lines().find(|line| !line.trim().is_empty()).unwrap_or(text)
}

/// 故障原因 → 一条可直接执行的命令。
///
/// 为什么按关键词而不是错误码：这些原因来自**运行时进程的 stderr 尾部**（DSH 没起来、
/// 密钥缺失、目录被占），不是我们自己的 `SpecError`，没有稳定码可匹配。命中不了就返回
/// `None`，原文照常显示 —— 不编一条可能不对的命令。
pub(crate) fn failure_hint(reason: &str) -> Option<&'static str> {
    let lower = reason.to_lowercase();
    if reason.contains("runtime install") || reason.contains("运行时") || lower.contains("dsh-runtime") || lower.contains("未安装") {
        Some("先运行 `saiku runtime install`（把钉住的 DSH 装进本机数据目录）")
    } else if lower.contains("api key") || lower.contains("auth") || reason.contains("密钥") || reason.contains("凭据") {
        Some("先运行 `saiku auth set <provider>` 存密钥")
    } else if reason.contains("config init") || lower.contains("saiku.json") || reason.contains("用户配置") {
        Some("先运行 `saiku config init` 生成配置")
    } else if reason.contains("另一个 saiku") {
        Some("先关掉占着会话目录的那个 saiku，再 /reconnect")
    } else {
        None
    }
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
                    context_window: model.context_window,
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
            personalization: false,
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
        // 行形如 `␣␣␣␣Free Model␣…␣Free␣␣`：面板没有边框（照 OpenCode），页脚右边只剩内边距。
        let line = text
            .lines()
            .find(|line| line.contains("Free Model"))
            .expect("免费模型那一行");
        assert!(
            line.trim_end().ends_with("Free"),
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

    /// 模型对话框里敲的字要画出来（此前只有光标在动）。
    #[test]
    fn the_model_dialog_shows_what_was_typed() {
        let mut app = model_dialog_app();
        assert!(buffer_text(&mut app, 120, 30).contains("搜索"), "空着时有占位");
        app.filter = "flash".into();
        let text = buffer_text(&mut app, 120, 30);
        assert!(text.contains("flash"), "{text}");
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
        assert!(text.contains("ctrl+f 收藏"));
        assert!(text.contains("ctrl+a provider"));
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
    /// Enter 就是发送（§7.4；OpenCode 同）。F6 那套只在 2026-10-14 的临时代码里存在过，
    /// 用户的原话是「哪个神人会用 F6 发送」。
    #[test]
    fn enter_sends_and_shift_enter_or_ctrl_j_inserts_a_newline() {
        let mut app = app();
        app.backend = BackendState::Failed { reason: "测试走独立请求那条路".into() };
        let (tx, rx) = mpsc::channel();
        app.editor.insert("跑一条");
        assert!(!app.input(key(KeyCode::Enter), &tx));
        let Request::Start(request) = rx.recv().unwrap() else { panic!("Enter 应该发送") };
        assert_eq!(request.prompt, "跑一条");

        app.editor.insert("第一行");
        app.input(key_with(KeyCode::Enter, KeyModifiers::SHIFT), &tx);
        app.input(key(KeyCode::Char('第')), &tx);
        app.input(key(KeyCode::Char('二')), &tx);
        app.input(key(KeyCode::Char('行')), &tx);
        app.input(key_with(KeyCode::Char('j'), KeyModifiers::CONTROL), &tx);
        assert_eq!(app.editor.text, "第一行\n第二行\n");
        assert!(rx.try_recv().is_err(), "换行不该发送");
    }

    /// Windows 的粘贴是一串按键（没有 `Event::Paste`）：同一批里的 Enter 必须是换行，
    /// 否则多行粘贴会在第一个换行处发出半句话。
    #[test]
    fn a_pasted_batch_inserts_newlines_instead_of_submitting() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        let mut batch = Vec::new();
        for c in "第一行\n第二行\n/exit".chars() {
            batch.push(if c == '\n' { key(KeyCode::Enter) } else { key(KeyCode::Char(c)) });
        }
        assert!(!app.input_batch(batch, &tx));
        assert_eq!(app.editor.text, "第一行\n第二行\n/exit");
        assert!(rx.try_recv().is_err(), "多行粘贴不能提交：{:?}", app.notice);

        // 单独一次 Enter 仍然发送。
        app.input(key(KeyCode::Enter), &tx);
        assert!(matches!(rx.recv().unwrap(), Request::Session(Command::Prompt { .. })));

        // Windows 上一次按键是 Press + Release 两条，按得快就同一批到 —— 那不是粘贴：
        // `/per` 之后这样一批 Enter 要执行补全（打开授权对话框），而不是换行。
        let mut app = self::app();
        type_text(&mut app, "/per", &tx);
        let mut release = crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(!app.input_batch(vec![key(KeyCode::Enter), Event::Key(release)], &tx));
        assert_eq!(app.dialog, Some(Dialog::Permissions), "editor={:?}", app.editor.text);
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
                personalization: false,
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

    /// `/per` + **一次** Enter 就是 `/permission`（打开授权对话框）；可选参数的命令不等参数。
    /// 必需参数的（`/search`）Enter 只填进去、等参数。Tab 永远只补全，能带参数的补一个空格。
    #[test]
    fn one_enter_on_a_completion_runs_commands_with_optional_arguments() {
        let (tx, _rx) = mpsc::channel();
        let mut app = self::app();
        type_text(&mut app, "/per", &tx);
        assert_eq!(app.autocomplete.as_ref().unwrap().current().unwrap().insert, "/permission");
        app.input(key(KeyCode::Enter), &tx);
        assert_eq!(app.dialog, Some(Dialog::Permissions), "一次 Enter 就执行：{}", app.notice);
        assert!(app.editor.text.is_empty());

        let mut app = self::app();
        type_text(&mut app, "/sear", &tx);
        app.input(key(KeyCode::Enter), &tx);
        assert_eq!(app.editor.text, "/search ", "必需参数：填进去等参数");
        assert_eq!(app.dialog, None);

        let mut app = self::app();
        type_text(&mut app, "/per", &tx);
        app.input(key(KeyCode::Tab), &tx);
        assert_eq!(app.editor.text, "/permission ", "Tab 只补全，补一个空格接参数");
    }

    /// Enter and F6 both accept-and-run through the completion popup, or the popup would stay
    /// open over the next screen. (F6 is only a compatibility alias now; Enter is the send key.)
    #[test]
    fn accepting_completion_with_the_send_key_runs_the_command() {
        let mut app = self::app();
        let (tx, _rx) = mpsc::channel();
        type_text(&mut app, "/the", &tx);
        assert!(app.autocomplete.is_some());
        app.input(key(KeyCode::Enter), &tx);
        assert_eq!(app.dialog, Some(Dialog::Theme));
        assert!(app.autocomplete.is_none());

        let mut app = self::app();
        let (tx, _rx) = mpsc::channel();
        type_text(&mut app, "/the", &tx);
        app.input(key(KeyCode::F(6)), &tx);
        assert_eq!(app.dialog, Some(Dialog::Theme));
        assert!(app.autocomplete.is_none());
    }

    /// `/plan` 与 `/permission confirm` 不再是「有能力但写着不可用」的占位：
    /// Plan 切到执行层只读，confirm 切到审批通道（M4 w4-tui-modes）。
    #[test]
    fn plan_and_confirm_are_wired_instead_of_being_stubbed() {
        let mut app = app();
        ready(&mut app);
        let (tx, rx) = mpsc::channel();
        app.editor.insert("/plan");
        app.input(key(KeyCode::F(6)), &tx);
        assert_eq!(app.mode, Mode::Plan);
        assert_eq!(app.agent, "echo", "Echo 只切模式，保留角色（§7.4）");
        assert!(
            matches!(rx.recv().unwrap(), Request::RestartBackend { mode, .. } if mode == "plan"),
            "Plan 要落到执行层（重启内核）"
        );

        let mut app = self::app();
        ready(&mut app);
        let (tx, rx) = mpsc::channel();
        app.editor.insert("/permission confirm");
        app.input(key(KeyCode::F(6)), &tx);
        assert_eq!(app.permission, "confirm");
        assert!(matches!(rx.recv().unwrap(), Request::RestartBackend { permission, .. } if permission == "confirm"));
        assert!(app.notice.contains("审批"), "{}", app.notice);
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

        // 收参数与否看注册表：`/search <词>` 敲出来要真的发检索（曾被手写名单拒掉）。
        let (tx, rx) = mpsc::channel();
        app.editor.insert("/search OK");
        app.input(key(KeyCode::Enter), &tx);
        assert!(
            rx.try_iter().any(|r| matches!(r, Request::Session(Command::SearchSessions { ref query }) if query == "OK")),
            "{}",
            app.notice
        );
        app.editor.insert("/output 1");
        app.input(key(KeyCode::Enter), &tx);
        assert!(!app.notice.contains("不接受参数"), "{}", app.notice);
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
        assert_eq!(plan.reason, None, "Plan 已经接通，不再挂「不可用」");
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
            state: BackendState::Failed { reason: "DSH 运行时未安装：先运行 `saiku runtime install`".into() },
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
        assert!(matches!(rx.recv().unwrap(), Request::RestartBackend { permission, .. } if permission == "readonly"));
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
        assert!(text.contains("+ 思考"), "思考默认折叠：{text}");
        assert!(!text.contains("先想一想"), "折叠时不显示思考原文：{text}");
        assert!(text.contains("✗ Write x.txt"), "{text}");
        assert!(text.contains("file access denied"), "失败在折叠态也要看得见：{text}");
        assert!(text.contains("[QUOTA] Insufficient Balance"), "模型失败原文要到界面上（P3）：{text}");
    }

    /// Shift+Tab：Echo → Build → Plan → Echo；Plan 重启内核（执行层只读），颜色跟着 Agent 走。
    /// 个性化开着时 Echo ↔ Build 也重启（persona 是按 Agent 装进内核的）。
    #[test]
    fn shift_tab_cycles_all_three_agents_through_the_real_path() {
        let (tx, rx) = mpsc::channel();
        let mut app = app();
        ready(&mut app);
        let restarts = |rx: &mpsc::Receiver<Request>| {
            rx.try_iter().filter(|r| matches!(r, Request::RestartBackend { .. })).count()
        };
        let colour = |app: &App| crate::view::agent_color(app, &app.theme);
        let echo = colour(&app);
        app.input(key(KeyCode::BackTab), &tx);
        assert_eq!((app.agent.as_str(), app.mode), ("build", Mode::Build));
        assert_eq!(restarts(&rx), 0, "个性化关着：Echo → Build 不重启");
        assert_ne!(colour(&app), echo);
        app.backend = BackendState::Ready { model: None };
        app.input(key(KeyCode::BackTab), &tx);
        assert_eq!(app.mode, Mode::Plan);
        assert_eq!(restarts(&rx), 1, "Plan 落到执行层，要重启");
        app.backend = BackendState::Ready { model: None };
        app.input(key(KeyCode::BackTab), &tx);
        assert_eq!((app.agent.as_str(), app.mode), ("echo", Mode::Build));
        assert_eq!(colour(&app), echo);

        let mut app = self::app();
        app.boot.personalization = true;
        ready(&mut app);
        while rx.try_recv().is_ok() {}
        app.input(key(KeyCode::BackTab), &tx);
        assert_eq!(restarts(&rx), 1, "个性化开着：换 Agent 就换 persona，要重启");
    }

    /// stash：有字时收起草稿并清空输入框；`/stash` 列出来，Enter 取回且从暂存里拿掉。
    #[test]
    fn a_draft_can_be_stashed_and_brought_back() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = app();
        app.editor.insert("写到一半的问题");
        let _ = app.execute(Action::Stash, None, &tx);
        assert!(app.editor.text.is_empty());
        assert_eq!(app.preferences.stash, ["写到一半的问题"]);
        // 斜杠命令先把输入框里的 `/stash` 吃掉，所以它总是「取回」。
        app.editor.insert("/stash");
        app.submit(&tx);
        assert_eq!(app.dialog, Some(Dialog::Stash));
        app.select(&tx);
        assert_eq!(app.editor.text, "写到一半的问题");
        assert!(app.preferences.stash.is_empty());
        let saved = serde_json::to_string(&app.preferences).unwrap();
        assert!(saved.contains("\"stash\""), "暂存跟偏好一起落盘：{saved}");
    }

    /// 子 agent：父会话里列出子会话；进了子会话第一行是回父会话，且不能直接发消息。
    #[test]
    fn subagent_sessions_open_from_the_parent_and_are_read_only() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = app();
        ready(&mut app);
        app.session_id = Some("p".into());
        let summary = |id: &str, parent: Option<&str>, label: Option<&str>| SessionSummary {
            session_id: id.into(),
            parent_session_id: parent.map(str::to_string),
            subagent_mode: parent.map(|_| "one-shot".to_string()),
            subagent_label: label.map(str::to_string),
            title: Some(format!("{id} 的标题")),
            ..SessionSummary::default()
        };
        app.update(Update::Session(service::Event::Sessions {
            sessions: vec![summary("p", None, None), summary("c1", Some("p"), Some("查依赖")), summary("other", Some("x"), None)],
        }));
        let _ = app.execute(Action::Subagents, None, &tx);
        assert_eq!(app.dialog, Some(Dialog::Subagents));
        let rows = app.subagent_rows();
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["查依赖"], "只列本会话的子会话");
        while rx.try_recv().is_ok() {}
        app.select(&tx);
        assert_eq!(app.session_id.as_deref(), Some("c1"));
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            sent.iter().any(|r| matches!(r, Request::Session(Command::OpenSession { session_id }) if session_id == "c1")),
            "{sent:?}"
        );
        // 子会话里：回父会话在第一行，发消息被挡下且草稿保留。
        assert_eq!(app.subagent_rows()[0].session_id, "p");
        app.editor.insert("你好");
        assert!(!app.submit(&tx));
        assert_eq!(app.editor.text, "你好");
        assert!(app.notice.contains("子 agent 会话"), "{}", app.notice);
    }

    // ── M4 w4-tui-modes：模式、授权与审批 ────────────────────────────────────

    fn approval(id: &str, tool: &str) -> ApprovalRequest {
        ApprovalRequest {
            request_id: id.into(),
            tool_name: tool.into(),
            call_id: Some(format!("call-{id}")),
            reason: Some("hook says ask".into()),
            display_reason: None,
            agent: Some("agent-1".into()),
        }
    }

    /// §7.4 的六种转换。矩阵是规格的一部分，逐格断言。
    #[test]
    fn plan_and_build_follow_the_six_transitions() {
        let cases = [
            ("echo", Mode::Build, "/plan", "echo", Mode::Plan),
            ("build", Mode::Build, "/plan", "plan", Mode::Plan),
            ("plan", Mode::Plan, "/plan", "plan", Mode::Plan),
            ("echo", Mode::Build, "/build", "echo", Mode::Build),
            ("plan", Mode::Plan, "/build", "build", Mode::Build),
            ("build", Mode::Build, "/build", "build", Mode::Build),
        ];
        for (agent, mode, command, expected_agent, expected_mode) in cases {
            let mut app = self::app();
            ready(&mut app);
            app.agent = agent.into();
            app.mode = mode;
            let (tx, rx) = mpsc::channel();
            app.editor.insert(command);
            app.input(key(KeyCode::F(6)), &tx);
            assert_eq!(
                (app.agent.as_str(), app.mode),
                (expected_agent, expected_mode),
                "{command} from {agent}+{mode:?}"
            );
            if (agent, mode) == (expected_agent, expected_mode) {
                assert!(rx.try_recv().is_err(), "没变化就不该重启内核");
            } else {
                assert!(
                    matches!(rx.recv().unwrap(), Request::RestartBackend { agent, mode, .. }
                        if agent == expected_agent && mode == expected_mode.as_str()),
                    "模式与 Agent 都要带到执行层"
                );
            }
        }
    }

    /// 运行中切换要先问：n 不改任何东西，y 才重启内核。
    #[test]
    fn changing_mode_mid_turn_asks_and_n_keeps_the_turn() {
        let mut app = self::app();
        ready(&mut app);
        app.session_id = Some("s1".into());
        app.update(conversation("s1", true, vec![user("hi")]));
        let (tx, rx) = mpsc::channel();
        app.editor.insert("/plan");
        app.input(key(KeyCode::F(6)), &tx);
        assert_eq!(app.dialog, Some(Dialog::Confirm(ConfirmKind::Mode(Mode::Plan))));
        assert_eq!(app.mode, Mode::Build, "还没确认");
        assert!(rx.try_recv().is_err(), "确认之前不发请求");
        app.input(key(KeyCode::Char('n')), &tx);
        assert_eq!(app.dialog, None);
        assert_eq!(app.mode, Mode::Build);
        assert!(rx.try_recv().is_err());

        app.editor.insert("/plan");
        app.input(key(KeyCode::F(6)), &tx);
        app.input(key(KeyCode::Char('y')), &tx);
        assert_eq!(app.mode, Mode::Plan);
        assert!(matches!(rx.recv().unwrap(), Request::RestartBackend { mode, .. } if mode == "plan"));
    }

    /// 审批必须在屏幕上；Esc 只是收起，不是允许。
    #[test]
    fn an_approval_is_visible_and_esc_is_not_consent() {
        let mut app = self::app();
        ready(&mut app);
        let (tx, rx) = mpsc::channel();
        app.update(Update::Session(service::Event::Approval { request: approval("e1", "bash") }));
        assert!(matches!(app.dialog, Some(Dialog::Approval(_))), "{:?}", app.dialog);
        let text = buffer_text(&mut app, 120, 30);
        assert!(text.contains("bash") && text.contains("允许"), "{text}");

        app.input(key(KeyCode::Esc), &tx);
        assert_eq!(app.dialog, None, "Esc 收起界面");
        assert_eq!(app.approvals.len(), 1, "请求还在等");
        assert!(rx.try_recv().is_err(), "Esc 不能替用户回答");
        assert!(app.notice.contains("不等于允许"), "{}", app.notice);

        app.editor.insert("/approvals");
        app.input(key(KeyCode::F(6)), &tx);
        assert!(matches!(app.dialog, Some(Dialog::Approval(_))), "能重新打开");
        app.input(key(KeyCode::Char('y')), &tx);
        assert!(matches!(
            rx.recv().unwrap(),
            Request::Session(Command::AnswerApproval { outcome: ApprovalOutcome::AllowOnce, .. })
        ));
        assert!(app.approvals.is_empty());
    }

    /// 拒绝要发 `rejected`；同时挂着的下一条立刻接上。
    #[test]
    fn rejecting_an_approval_pops_the_next_queued_one() {
        let mut app = self::app();
        ready(&mut app);
        let (tx, rx) = mpsc::channel();
        app.update(Update::Session(service::Event::Approval { request: approval("e1", "bash") }));
        app.update(Update::Session(service::Event::Approval { request: approval("e2", "write") }));
        assert_eq!(app.approvals.len(), 2);
        assert!(matches!(&app.dialog, Some(Dialog::Approval(request)) if request.request_id == "e1"));
        app.input(key(KeyCode::Char('n')), &tx);
        assert!(matches!(
            rx.recv().unwrap(),
            Request::Session(Command::AnswerApproval { outcome: ApprovalOutcome::Reject, ref request_id })
                if request_id == "e1"
        ));
        assert_eq!(app.approvals.len(), 1);
        assert!(matches!(&app.dialog, Some(Dialog::Approval(request)) if request.request_id == "e2"));
        // 内核撤回：收掉等待状态。
        app.update(Update::Session(service::Event::ApprovalClosed {
            request_id: "e2".into(),
            reason: "这一轮结束了".into(),
        }));
        assert!(app.approvals.is_empty());
        assert_eq!(app.dialog, None);
    }

    /// 内核重启会作废挂着的审批：不能让人对着一份死请求按 y。
    #[test]
    fn restarting_the_kernel_drops_pending_approvals() {
        let mut app = self::app();
        ready(&mut app);
        app.update(Update::Session(service::Event::Approval { request: approval("e1", "bash") }));
        assert_eq!(app.approvals.len(), 1);
        app.update(Update::Session(service::Event::Backend { state: BackendState::Starting }));
        assert!(app.approvals.is_empty());
        assert_eq!(app.dialog, None, "死请求的弹层要收起来");
    }

    #[test]
    fn search_hits_open_the_matching_session() {
        let mut app = self::app();
        ready(&mut app);
        let (tx, rx) = mpsc::channel();
        app.update(Update::Session(service::Event::Search {
            query: "会话".into(),
            items: vec![SearchHit {
                session_id: "hit-1".into(),
                title: None,
                snippet: "改\u{0}会话\u{0}列表".into(),
            }],
        }));
        assert_eq!(app.dialog, Some(Dialog::Search));
        let text = buffer_text(&mut app, 120, 30);
        assert!(text.contains("会话"), "{text}");
        app.input(key(KeyCode::Enter), &tx);
        assert!(matches!(rx.recv().unwrap(), Request::Session(Command::OpenSession { session_id }) if session_id == "hit-1"));
        assert_eq!(app.session_id.as_deref(), Some("hit-1"));
    }

    #[test]
    fn diff_and_output_open_the_scrollable_viewer() {
        use arc_deck_core::conversation::{AssistantTurn, Block, ToolCall, ToolStatus, TurnState};
        let mut app = self::app();
        ready(&mut app);
        app.session_id = Some("s1".into());
        let mut edit = ToolCall {
            call_id: "c1".into(),
            name: "edit".into(),
            arguments: r#"{"file_path":"x.rs"}"#.into(),
            status: ToolStatus::Done,
            output: Some("改好了".into()),
            ..ToolCall::default()
        };
        edit.detail = Some(serde_json::json!({
            "diffs": [{ "path": "x.rs", "oldText": "a\nb", "newText": "a\nB" }]
        }));
        app.update(conversation(
            "s1",
            false,
            vec![user("改一下"), Message::Assistant(AssistantTurn {
                turn: 1,
                blocks: vec![Block::Tool(edit)],
                state: TurnState::Completed,
                ..AssistantTurn::default()
            })],
        ));
        let (tx, _rx) = mpsc::channel();
        app.editor.insert("/diff");
        app.input(key(KeyCode::F(6)), &tx);
        let Some(Dialog::Viewer(viewer)) = &app.dialog else { panic!("{:?}", app.dialog) };
        assert!(viewer.lines.iter().any(|line| line.contains("+ B")), "{:?}", viewer.lines);
        // 滚动与过滤都改 viewer 自己的状态。
        app.input(key(KeyCode::Char('B')), &tx);
        let Some(Dialog::Viewer(viewer)) = &app.dialog else { panic!() };
        assert_eq!(viewer.filter, "B");
        app.input(key(KeyCode::Backspace), &tx);
        let Some(Dialog::Viewer(viewer)) = &app.dialog else { panic!() };
        assert_eq!(viewer.filter, "");
        app.input(key(KeyCode::Esc), &tx);
        assert_eq!(app.dialog, None);

        app.editor.insert("/output");
        app.input(key(KeyCode::F(6)), &tx);
        let Some(Dialog::Viewer(viewer)) = &app.dialog else { panic!("{:?}", app.dialog) };
        assert!(viewer.title.contains("edit"), "{}", viewer.title);
        assert!(viewer.lines.iter().any(|line| line.contains("改好了")));
    }

    /// 时间线选中之后要滚到那条消息的行（偏移由正文渲染填）。
    #[test]
    fn timeline_jumps_to_the_message_row() {
        let mut app = self::app();
        ready(&mut app);
        app.session_id = Some("s1".into());
        // 消息要多到超过一屏，`scroll_max` 才不是 0（否则跳转会被夹到 0，测不出偏移）。
        let messages: Vec<Message> = (0..10)
            .map(|index| user(&format!("第 {index} 条\n第二行\n第三行")))
            .collect();
        app.update(conversation("s1", false, messages));
        let _ = buffer_text(&mut app, 120, 30);
        assert_eq!(app.message_offsets.len(), 10);
        let target = app.message_offsets[2].1;
        let (tx, _rx) = mpsc::channel();
        app.editor.insert("/timeline");
        app.input(key(KeyCode::F(6)), &tx);
        assert_eq!(app.dialog, Some(Dialog::Timeline));
        app.input(key(KeyCode::Down), &tx);
        app.input(key(KeyCode::Down), &tx);
        app.input(key(KeyCode::Enter), &tx);
        assert_eq!(app.dialog, None);
        assert_eq!(app.scroll, Some(target));
    }

    /// toast：普通提示会到期，错误提示不会自己消失。
    #[test]
    fn toasts_expire_except_errors() {
        let mut app = self::app();
        app.toast("已复制", false);
        assert_eq!(app.toasts.len(), 1);
        app.toasts[0].until = Instant::now() - Duration::from_secs(1);
        app.tick();
        assert!(app.toasts.is_empty(), "普通提示到期就收");

        app.toast("复制失败", true);
        app.tick();
        assert_eq!(app.toasts.len(), 1, "错误不自动消失");

        // 同一句话重复出现只刷新，不叠成一列。
        app.toast("复制失败", true);
        assert_eq!(app.toasts.len(), 1);
    }

    #[test]
    fn failures_come_with_a_copyable_next_command() {
        assert!(failure_hint("DSH 运行时未安装：先运行 `saiku runtime install`").unwrap().contains("runtime install"));
        assert!(failure_hint("invalid api key for provider").unwrap().contains("auth set"));
        assert!(failure_hint("另一个 saiku（pid 42）正在使用交互会话目录").unwrap().contains("关掉"));
        assert_eq!(failure_hint("内核在启动时崩了"), None, "认不出就只显示原文，不编命令");
    }

    #[test]
    fn base64_encodes_osc52_payloads() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected, "{input:?}");
        }
        // 中文是多字节：按字节编码，不按字符。
        assert_eq!(base64("中".as_bytes()), "5Lit");
    }
}
