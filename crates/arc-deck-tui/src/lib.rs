//! Terminal presentation only. Execution, health and cancellation belong to the host.
//! M4a uses independent headless runs; Remote conversations are a separate capability.

mod app;
pub mod catalog;
mod editor;
mod markdown;
mod theme;
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
    pub model: Option<String>,
    pub effort: Option<String>,
    pub config_error: Option<String>,
    pub preferences: PathBuf,
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    light: bool,
    sidebar: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            light: false,
            sidebar: true,
        }
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
    // try_init may enter raw mode before failing to switch screens.
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init()?;
    execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
    let mut app = app::App::new(boot, preferences);
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
