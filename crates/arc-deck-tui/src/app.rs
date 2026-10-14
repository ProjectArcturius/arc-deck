use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::catalog::{self, ModelPrefs, ModelRef};
use crate::model_dialog::{self, EffortDialogState, ModelDialogState, ProviderDialogState};
use crate::{
    Boot, Preferences, Request, RunDetail, RunRecord, StartRequest, Update,
    editor::Editor,
    theme::{DEFAULT_THEME, Theme},
    theme_dialog::{ThemeDialog, ThemeDialogAction},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    New,
    Runs,
    Agents,
    Models,
    Sidebar,
    Tools,
    Latest,
    Theme,
    Mode,
    Cancel,
    Usage,
    Evidence,
    Quit,
}

pub(crate) struct Command {
    pub action: Action,
    pub slash: &'static str,
    pub label: &'static str,
}

// Keyboard and slash dispatch share this registry, so labels cannot drift from actions.
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        action: Action::New,
        slash: "/new",
        label: "新的独立请求",
    },
    Command {
        action: Action::Runs,
        slash: "/sessions",
        label: "运行历史（当前工作区）",
    },
    Command {
        action: Action::Agents,
        slash: "/agents",
        label: "选择 Agent",
    },
    Command {
        action: Action::Models,
        slash: "/models",
        label: "选择模型（下一次请求生效）",
    },
    Command {
        action: Action::Sidebar,
        slash: "/sidebar",
        label: "切换信息栏",
    },
    Command {
        action: Action::Tools,
        slash: "/tools",
        label: "展开 / 折叠工具记录",
    },
    Command {
        action: Action::Latest,
        slash: "/latest",
        label: "回到最新",
    },
    Command {
        action: Action::Theme,
        slash: "/theme",
        label: "选择主题",
    },
    Command {
        action: Action::Mode,
        slash: "/mode",
        label: "切换深 / 浅色",
    },
    Command {
        action: Action::Cancel,
        slash: "/cancel",
        label: "取消当前运行",
    },
    Command {
        action: Action::Usage,
        slash: "/usage",
        label: "查看 provider 报告的用量",
    },
    Command {
        action: Action::Evidence,
        slash: "/evidence",
        label: "查看完整证据位置",
    },
    Command {
        action: Action::Quit,
        slash: "/exit",
        label: "退出（取消本次 TUI 发起的活动）",
    },
];

/// 弹层。带状态的那几个（模型 / provider / effort）的渲染与选择规则都在
/// `model_dialog` 里，这里只保留「当前开着哪个」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Dialog {
    Commands,
    Runs,
    Agents,
    Models(ModelDialogState),
    Providers(ProviderDialogState),
    Efforts(EffortDialogState),
    Theme,
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
    pub leader: bool,
    pub scroll: Option<usize>,
    pub scroll_max: usize,
    pub sidebar_overlay: bool,
    pub expanded: HashMap<String, bool>,
    pub notice: String,
    pub dirty: bool,
    serial: u64,
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
        let notice = boot
            .config_error
            .clone()
            .or(theme_notice)
            .unwrap_or_else(|| "每次提交是独立请求；多轮会话正在接入".into());
        Self {
            notice,
            effort,
            boot,
            preferences,
            theme,
            editor: Editor::default(),
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
            leader: false,
            scroll: None,
            scroll_max: 0,
            sidebar_overlay: false,
            expanded: HashMap::new(),
            dirty: true,
            serial: 0,
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
            _ => {}
        }
    }
    pub fn filtered_commands(&self) -> Vec<&'static Command> {
        let query = self.filter.to_lowercase();
        COMMANDS
            .iter()
            .filter(|c| {
                format!("{} {}", c.slash, c.label)
                    .to_lowercase()
                    .contains(&query)
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
        // `Dialog` 带着模型对话框的状态，不是 Copy：先记下是不是主题选择器再移进去。
        let is_theme = dialog == Dialog::Theme;
        self.dialog = Some(dialog);
        self.filter.clear();
        self.choice = 0;
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
            Some(Dialog::Runs) => self.filtered_runs().len(),
            Some(Dialog::Agents) => 3,
            Some(Dialog::Models(state)) => self.model_rows(*state).len(),
            Some(Dialog::Providers(_)) => self.provider_rows().len(),
            Some(Dialog::Efforts(_)) => self.effort_count(),
            // The theme picker keeps its own cursor; this only has to be non-zero so the
            // generic `Up`/`Down` branch cannot clamp it away before the dialog sees the key.
            Some(Dialog::Theme) => 1,
            None => 0,
        }
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
    fn action(&mut self, action: Action, requests: &Sender<Request>) -> bool {
        match action {
            Action::New => {
                self.selected = None;
                self.detail = None;
                self.scroll = None;
                let _ = requests.send(Request::Open(None));
            }
            Action::Runs => self.open_dialog(Dialog::Runs),
            Action::Agents => self.open_dialog(Dialog::Agents),
            Action::Models => self.open_dialog(Dialog::Models(ModelDialogState::default())),
            Action::Sidebar => {
                self.preferences.sidebar = !self.preferences.sidebar;
                self.sidebar_overlay = self.preferences.sidebar;
            }
            Action::Tools => {
                if let Some(id) = &self.selected {
                    let open = self.expanded.entry(id.clone()).or_default();
                    *open = !*open;
                }
            }
            Action::Latest => self.scroll = None,
            Action::Theme => self.open_dialog(Dialog::Theme),
            Action::Mode => {
                self.preferences.light = !self.preferences.light;
                self.apply_preferences_theme();
                self.notice = format!("界面模式：{}", self.preferences.mode().as_str());
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
        }
        false
    }
    fn submit(&mut self, requests: &Sender<Request>) -> bool {
        let text = self.editor.text.trim().to_string();
        if text.is_empty() {
            return false;
        }
        if text.starts_with('/') {
            self.editor.take();
            if let Some(command) = COMMANDS.iter().find(|c| c.slash == text) {
                return self.action(command.action, requests);
            }
            if text == "/plan" {
                self.notice = "Plan 暂不可用：M3 的执行 guard 还未接通，提示词不能保证只读".into();
            } else if text == "/build" {
                if self.active.is_some() {
                    self.notice = "请先取消并等待当前运行结束，再切换 Agent".into();
                } else {
                    self.agent = "build".into();
                    self.notice = "下一次独立请求使用 Build".into();
                }
            } else if text == "/permission" {
                self.notice = format!(
                    "当前授权 {}；/permission workspace|full 影响下一次请求",
                    self.permission
                );
            } else if text == "/effort" {
                self.notice = format!(
                    "当前 effort {}；/effort <级别|default> 影响下一次请求",
                    self.effort.as_deref().unwrap_or("模型缺省")
                );
            } else if let Some(level) = text.strip_prefix("/permission ") {
                if self.active.is_some() {
                    self.notice = "请先取消并等待当前运行结束，再修改授权".into();
                } else if matches!(level, "workspace" | "full") {
                    self.permission = level.into();
                    self.notice = format!("下一次独立请求的授权级别：{level}");
                } else {
                    self.notice =
                        "此版支持 workspace / full；readonly 执行限制与 confirm 应答者尚未接通"
                            .into();
                }
            } else if let Some(effort) = text.strip_prefix("/effort ") {
                if self.active.is_some() {
                    self.notice = "当前请求的模型强度已固定，请等它结束".into();
                } else {
                    self.effort = if effort == "default" {
                        None
                    } else {
                        Some(effort.into())
                    };
                    self.notice =
                        "effort 将用于下一次请求，由运行时校验；/effort default 恢复模型缺省值"
                            .into();
                }
            } else {
                self.notice =
                    "未知命令；Ctrl+P 查看命令，/permission workspace|full，/effort <级别|default>"
                        .into();
            }
            return false;
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
    fn select(&mut self, requests: &Sender<Request>) -> bool {
        // 克隆而不是借用：下面几个分支都要 &mut self（打开新对话框、改偏好）。
        match self.dialog.clone() {
            Some(Dialog::Commands) => {
                let action = self.filtered_commands().get(self.choice).map(|c| c.action);
                self.dialog = None;
                if let Some(action) = action {
                    return self.action(action, requests);
                }
            }
            Some(Dialog::Runs) => {
                if let Some(id) = self.filtered_runs().get(self.choice).map(|r| r.id.clone()) {
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
                } else if self.choice == 2 {
                    self.notice = "Plan 暂不可用：执行 guard 尚未接通".into();
                } else {
                    self.agent = if self.choice == 0 { "echo" } else { "build" }.into();
                    self.dialog = None;
                }
            }
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
                    self.editor.insert(&text);
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
        if ctrl && key.code == KeyCode::Char('c') {
            return true;
        }
        if key.code == KeyCode::Esc {
            self.leader = false;
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
            if self.selected_is_running() || self.active.is_some() {
                return self.action(Action::Cancel, requests);
            }
            self.editor.text.clear();
            self.editor.cursor = 0;
            return false;
        }
        if self.dialog.is_some() {
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
            // 对话框上下文的键位先判：Ctrl+F / Ctrl+A 只在模型对话框里有意义，
            // 与同名会话级绑定不冲突（§7.4：冲突按上下文解析，不能全局覆盖）。
            if ctrl && key.code == KeyCode::Char('f') {
                self.toggle_favorite();
                return false;
            }
            if ctrl && key.code == KeyCode::Char('a') {
                // 只在模型对话框里有这个上下文入口（上游的 model.dialog.provider）。
                if let Some(Dialog::Models(back)) = self.dialog {
                    self.swap_dialog(Dialog::Providers(ProviderDialogState { back: Some(back) }));
                }
                return false;
            }
            match key.code {
                KeyCode::Up => self.choice = self.choice.saturating_sub(1),
                KeyCode::Down => {
                    self.choice = (self.choice + 1).min(self.selection_count().saturating_sub(1))
                }
                KeyCode::Enter => return self.select(requests),
                KeyCode::Backspace => {
                    // effort 与 provider 列表也能搜：provider 是按名字找的，
                    // 而 effort 列表短到不需要搜，这里一并接受，键位才一致。
                    self.filter.pop();
                    self.choice = 0;
                }
                KeyCode::Char(c) if !ctrl => {
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
        if self.leader {
            self.leader = false;
            let action = match key.code {
                KeyCode::Char('n') => Some(Action::New),
                KeyCode::Char('l') => Some(Action::Runs),
                KeyCode::Char('a') => Some(Action::Agents),
                KeyCode::Char('m') => Some(Action::Models),
                KeyCode::Char('b') => Some(Action::Sidebar),
                KeyCode::Char('t') => Some(Action::Theme),
                _ => None,
            };
            if let Some(action) = action {
                return self.action(action, requests);
            }
            return false;
        }
        match key.code {
            KeyCode::Char('p') if ctrl => self.open_dialog(Dialog::Commands),
            KeyCode::Char('x') if ctrl => self.leader = true,
            KeyCode::Char('j') if ctrl => self.editor.insert("\n"),
            // F2 / Shift+F2 在 Recent 里前后循环（§7.4，采自上游 local.tsx 的 cycle）。
            // 运行中也允许 —— 它只改「下一次请求」用的路由，不碰正在跑的那个。
            KeyCode::F(2) if key.modifiers.contains(KeyModifiers::SHIFT) => self.cycle_model(-1),
            KeyCode::F(2) => self.cycle_model(1),
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.editor.insert("\n")
            }
            KeyCode::Enter if cfg!(windows) => self.editor.insert("\n"),
            KeyCode::Enter | KeyCode::F(6) => return self.submit(requests),
            KeyCode::BackTab => {
                if self.active.is_some() {
                    self.notice = "请先等待当前运行结束".into();
                } else {
                    self.agent = if self.agent == "echo" {
                        "build"
                    } else {
                        "echo"
                    }
                    .into();
                }
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.editor.insert(&c.to_string())
            }
            KeyCode::Backspace => self.editor.backspace(),
            KeyCode::Delete => self.editor.delete(),
            KeyCode::Left => self.editor.left(),
            KeyCode::Right => self.editor.right(),
            KeyCode::Home => self.editor.home(),
            KeyCode::End if ctrl => self.scroll = None,
            KeyCode::End => self.editor.end(),
            KeyCode::Up if ctrl => self.editor.history(true),
            KeyCode::Down if ctrl => self.editor.history(false),
            KeyCode::Up if self.editor.text.contains('\n') => self.editor.vertical(true),
            KeyCode::Down if self.editor.text.contains('\n') => self.editor.vertical(false),
            KeyCode::Up => self.editor.history(true),
            KeyCode::Down => self.editor.history(false),
            KeyCode::PageUp => self.scroll_up(8),
            KeyCode::PageDown => self.scroll_down(8),
            _ => {}
        }
        false
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
    #[test]
    fn paste_does_not_dispatch_and_modal_escape_does_not_cancel() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        app.input(Event::Paste("一\n二".into()), &tx);
        assert!(rx.try_recv().is_err());
        app.active = Some("r".into());
        app.selected = app.active.clone();
        app.open_dialog(Dialog::Commands);
        app.input(key(KeyCode::Esc), &tx);
        assert!(rx.try_recv().is_err());
        app.input(key(KeyCode::Esc), &tx);
        assert!(matches!(rx.recv().unwrap(), Request::Cancel(id) if id == "r"));
    }
    #[test]
    fn plan_and_confirm_do_not_pretend_to_be_supported() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        for cmd in ["/plan", "/permission confirm", "/permission readonly"] {
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

    /// `/theme` now opens the picker and `/mode` owns the dark/light toggle. Both are in
    /// `COMMANDS`, which is the single source the slash parser and the command palette share.
    #[test]
    fn the_mode_toggle_moved_to_slash_mode() {
        let theme = COMMANDS.iter().find(|command| command.action == Action::Theme).unwrap();
        assert_eq!(theme.slash, "/theme");
        let mode = COMMANDS.iter().find(|command| command.action == Action::Mode).unwrap();
        assert_eq!(mode.slash, "/mode");
        let mut slashes: Vec<&str> = COMMANDS.iter().map(|command| command.slash).collect();
        let count = slashes.len();
        slashes.sort_unstable();
        slashes.dedup();
        assert_eq!(slashes.len(), count, "two commands share a slash");

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
}
