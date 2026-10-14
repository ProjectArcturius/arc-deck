//! Terminal presentation only. Execution, health and cancellation belong to the host.
//! M4a uses independent headless runs; Remote conversations are a separate capability.

mod app;
pub mod catalog;
mod color;
mod editor;
mod markdown;
mod model_dialog;
mod theme;
mod theme_dialog;
mod view;

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use arc_deck_core::{
    event::EventEnvelope,
    result::{TaskResult, Usage},
};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    },
    execute,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Boot {
    pub workspace: PathBuf,
    /// 配置里写的那条路由，作为启动时的当前模型。运行中由 `App::model` 覆盖。
    pub model: Option<String>,
    pub effort: Option<String>,
    pub config_error: Option<String>,
    pub preferences: PathBuf,
    /// 模型目录（provider 列表）。权威来源是 DSH（§7.2）；当前由启动方从用户配置的
    /// `provider` 块临时构造 —— 见 `arc-deck-cli/src/tui.rs` 的 `catalog_from_config`。
    pub catalog: Vec<catalog::Provider>,
    /// 收藏 / 最近 / 按模型记的 effort，与 `preferences` 同目录，文件名 `model.json`。
    pub model_prefs: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    pub id: String,
    pub title: String,
    pub state: String,
    pub health: String,
    pub activity: String,
    pub model: Option<String>,
    pub started_at: u64,
    pub terminal: bool,
    pub usage: Option<Usage>,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunDetail {
    pub record: RunRecord,
    pub instruction: String,
    pub events: Vec<EventEnvelope>,
    pub result: Option<TaskResult>,
    pub final_text: Option<String>,
    pub evidence_dir: PathBuf,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct StartRequest {
    pub id: String,
    pub prompt: String,
    pub agent: String,
    pub permission: String,
    pub effort: Option<String>,
    /// 这一次请求用的路由，来自配置或模型对话框里选定的那条。
    ///
    /// 切换模型只影响**下一次**请求：worker 的路由在 `initialize` 时就被解析并钉住
    /// （§7.2），所以没有「热切换当前运行」这回事。
    pub model: Option<String>,
}

#[derive(Debug)]
pub enum Request {
    Start(StartRequest),
    Open(Option<String>),
    Cancel(String),
    Refresh,
    Quit,
}

#[derive(Debug)]
pub enum Update {
    Runs(Vec<RunRecord>),
    Detail(Box<RunDetail>),
    Finished(String),
    Notice(String),
}

/// UI preferences, persisted to `tui-preferences.json`. `serde(default)` is load-bearing: the
/// first released file only had `light` and `sidebar`, and dropping a field or adding one must
/// not make an older file unreadable.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Preferences {
    light: bool,
    sidebar: bool,
    /// Built-in theme name. `None` means "never chosen", which resolves to `DEFAULT_THEME`.
    /// Kept as written even when the name is unknown so the warning repeats instead of the
    /// typo being silently rewritten to the default.
    theme: Option<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            light: false,
            sidebar: true,
            theme: None,
        }
    }
}

impl Preferences {
    pub(crate) fn theme_name(&self) -> &str {
        self.theme.as_deref().unwrap_or(theme::DEFAULT_THEME)
    }

    pub(crate) fn mode(&self) -> theme::Mode {
        if self.light { theme::Mode::Light } else { theme::Mode::Dark }
    }
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
        ratatui::restore();
    }
}

/// Runs on one thread: Crossterm poll/read must share their event reader.
pub fn run(boot: Boot, requests: Sender<Request>, updates: Receiver<Update>) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "TUI 需要交互终端；脚本请用 arcdeck run / status",
        ));
    }
    let preferences = std::fs::read_to_string(&boot.preferences)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // 模型偏好与 UI 偏好分开存：坏掉的 `model.json` 只该丢掉收藏与最近，不该动别的。
    let model_prefs = catalog::ModelPrefs::load(&boot.model_prefs);
    // try_init may enter raw mode before failing to switch screens.
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init()?;
    execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
    let mut app = app::App::new(boot, preferences, model_prefs);
    let _ = requests.send(Request::Refresh);
    let result = (|| {
        loop {
            while let Ok(update) = updates.try_recv() {
                app.update(update);
            }
            if app.dirty {
                terminal.draw(|frame| view::draw(frame, &mut app))?;
                app.dirty = false;
            }
            if event::poll(Duration::from_millis(50))? && app.input(event::read()?, &requests) {
                return Ok(());
            }
        }
    })();
    let _ = requests.send(Request::Quit);
    // Preferences are UI data only; never rewrite the user's launch configuration.
    if let Some(parent) = app.boot.preferences.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // 模型偏好只在真的改过时写盘（`model_prefs_dirty`）：收藏与最近属于用户资产，
    // 每次退出都重写一遍不值当，而「写一半被打断」正是 `ModelPrefs::save` 用 rename 防的那件事。
    if app.model_prefs_dirty
        && let Err(error) = app.model_prefs.save(&app.boot.model_prefs)
    {
        drop(_guard);
        eprintln!("模型偏好保存失败：{error}");
        return result;
    }
    if let Ok(text) = serde_json::to_string_pretty(&app.preferences)
        && let Err(error) = std::fs::write(&app.boot.preferences, text)
    {
        // Restore before diagnostics, so an error cannot corrupt the screen.
        drop(_guard);
        eprintln!("TUI 偏好保存失败：{error}");
        return result;
    }
    result
}
