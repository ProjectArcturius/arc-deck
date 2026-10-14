use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::{
    Boot, Preferences, Request, RunDetail, RunRecord, StartRequest, Update, editor::Editor,
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
        label: "查看模型路由",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialog {
    Commands,
    Runs,
    Agents,
    Models,
}

pub(crate) struct App {
    pub boot: Boot,
    pub preferences: Preferences,
    pub editor: Editor,
    pub runs: Vec<RunRecord>,
    pub selected: Option<String>,
    pub detail: Option<Box<RunDetail>>,
    pub active: Option<String>,
    pub agent: String,
    pub permission: String,
    pub effort: Option<String>,
    pub dialog: Option<Dialog>,
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
    pub fn new(boot: Boot, preferences: Preferences) -> Self {
        Self {
            notice: boot
                .config_error
                .clone()
                .unwrap_or_else(|| "每次提交是独立请求；多轮会话正在接入".into()),
            effort: boot.effort.clone(),
            boot,
            preferences,
            editor: Editor::default(),
            runs: Vec::new(),
            selected: None,
            detail: None,
            active: None,
            agent: "echo".into(),
            permission: "workspace".into(),
            dialog: None,
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
        self.dialog = Some(dialog);
        self.filter.clear();
        self.choice = 0;
    }
    fn selection_count(&self) -> usize {
        match self.dialog {
            Some(Dialog::Commands) => self.filtered_commands().len(),
            Some(Dialog::Runs) => self.filtered_runs().len(),
            Some(Dialog::Agents) => 3,
            Some(Dialog::Models) => 1,
            None => 0,
        }
    }
    fn selected_is_running(&self) -> bool {
        self.detail
            .as_ref()
            .is_some_and(|d| !d.record.terminal && d.record.health != "dead")
            || self.active.is_some() && self.active == self.selected
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
            Action::Models => self.open_dialog(Dialog::Models),
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
            Action::Theme => self.preferences.light = !self.preferences.light,
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
        let request = StartRequest {
            id: id.clone(),
            prompt: self.editor.take(),
            agent: self.agent.clone(),
            permission: self.permission.clone(),
            effort: self.effort.clone(),
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
        match self.dialog {
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
            Some(Dialog::Models) => {
                self.notice = "当前仅显示配置路由；DSH 模型目录与切换正在接入".into()
            }
            None => {}
        }
        false
    }
    pub fn input(&mut self, event: Event, requests: &Sender<Request>) -> bool {
        self.dirty = true;
        let key = match event {
            Event::Paste(text) => {
                if self.dialog.is_none() {
                    self.editor.insert(&text);
                }
                return false;
            }
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.scroll_up(3),
                    MouseEventKind::ScrollDown => self.scroll_down(3),
                    _ => {}
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
            if self.dialog.take().is_some() {
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
            match key.code {
                KeyCode::Up => self.choice = self.choice.saturating_sub(1),
                KeyCode::Down => {
                    self.choice = (self.choice + 1).min(self.selection_count().saturating_sub(1))
                }
                KeyCode::Enter => return self.select(requests),
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.choice = 0;
                }
                KeyCode::Char(c) if !ctrl => {
                    self.filter.push(c);
                    self.choice = 0;
                }
                _ => {}
            }
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
}

pub(crate) fn token(n: Option<u64>) -> String {
    n.map(|n| n.to_string()).unwrap_or_else(|| "未报告".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use std::sync::mpsc;
    fn app() -> App {
        App::new(
            Boot {
                workspace: ".".into(),
                model: Some("p/m".into()),
                effort: None,
                config_error: None,
                preferences: "unused".into(),
            },
            Preferences::default(),
        )
    }
    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
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
}
