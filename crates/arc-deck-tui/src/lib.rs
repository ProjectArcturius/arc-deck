//! Terminal presentation only. Execution, health and cancellation belong to the host.
//! Interactive conversations go through the Remote session layer; `/runs` still reads
//! independent headless runs from the same state directory.

mod app;
mod autocomplete;
pub mod catalog;
mod color;
mod commands;
mod editor;
mod files;
mod keymap;
mod markdown;
mod model_dialog;
mod theme;
mod theme_dialog;
mod syntax;
mod toolview;
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
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute, SynchronizedUpdate,
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
    /// 配置里的 `personalization.enabled`。开着时 persona 随 Agent 装配进内核（§5.3.1），
    /// 所以换 Agent 也要重启内核；关着时 Echo 与 Build 在交互面上没有区别。
    pub personalization: bool,
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
    /// 独立运行（headless 那条路）。交互内核不可用时的退路，以及 `/runs` 里看工人。
    Start(StartRequest),
    Open(Option<String>),
    Cancel(String),
    Refresh,
    /// 交互会话。消息形状就是 GUI 将来经 IPC 用的那一份（`arc_deck_core::service`）——
    /// TUI 在进程内先用它，等于每天都在给那份契约做验收。
    Session(arc_deck_core::service::Command),
    /// 授权级别、Agent 与模式都是交互内核**启动时**定的（它们决定沙箱档、工具行与 patch 里的
    /// 预设表），改了就要重启内核。会话存在独立的 DSH home 里，重启之后能重新打开，
    /// 所以这不丢对话 —— 但**正在跑的那一轮会被取消**，所以界面先问一句。
    RestartBackend {
        permission: String,
        agent: String,
        mode: String,
    },
    Quit,
}

#[derive(Debug)]
pub enum Update {
    Runs(Vec<RunRecord>),
    Detail(Box<RunDetail>),
    Finished(String),
    Notice(String),
    Session(arc_deck_core::service::Event),
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
    /// Submitted drafts, oldest first. Input history has to survive a restart to be useful;
    /// the file is the same one the UI preferences live in so there is no second store to lose.
    history: Vec<String>,
    /// Most-recently-used `@` references, used to rank file candidates.
    recent_files: Vec<String>,
    /// Summarize a large paste into `[粘贴 N 行]` in the draft and expand it on submit.
    paste_summary: bool,
    /// 暂存的草稿，新的在前（OpenCode 的 prompt stash）。与历史同一个文件，重启不丢。
    stash: Vec<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            light: false,
            sidebar: true,
            theme: None,
            history: Vec::new(),
            recent_files: Vec::new(),
            paste_summary: true,
            stash: Vec::new(),
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
        let _ = execute!(
            io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste,
            DisableMouseCapture,
            crossterm::cursor::Show
        );
        ratatui::restore();
    }
}

/// panic 也要把终端还回去（任务书 w5-polish 第 4 条）。
///
/// 为什么不是只靠 `TerminalGuard`：panic 展开时会 drop 它，但 `run` 里的
/// `spawn_blocking` 线程 panic 之后，进程可能还活着并继续往一个已脱离 alternate screen
/// 的终端上画；更常见的是 panic 信息本身被画进 alternate screen，用户退出后看不到。
/// 先恢复、再把原来的 hook 打一遍，两件事都做。
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste,
            DisableMouseCapture,
            crossterm::cursor::Show
        );
        ratatui::restore();
        previous(info);
    }));
}

/// Hands the terminal back to an external editor and returns the edited draft.
///
/// The editor command is `$VISUAL` / `$EDITOR` (whitespace-split, so `code -w` works) or
/// `notepad` on Windows and `vi` elsewhere. Failure is reported, never fatal: the draft would
/// otherwise be lost because the user's editor was mistyped.
fn run_external_editor(draft: &str) -> io::Result<Option<String>> {
    let path = std::env::temp_dir().join(format!("saiku-draft-{}.md", std::process::id()));
    std::fs::write(&path, draft)?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| if cfg!(windows) { "notepad".into() } else { "vi".into() });
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or(if cfg!(windows) { "notepad" } else { "vi" });
    let status = std::process::Command::new(program)
        .args(parts)
        .arg(&path)
        .status();
    match status {
        Ok(status) if status.success() => {
            let text = std::fs::read_to_string(&path)?;
            // Editors append a final newline; the composer should not gain one from that.
            let text = text.trim_end_matches('\n').to_string();
            let _ = std::fs::remove_file(&path);
            Ok(Some(text))
        }
        Ok(status) => {
            let _ = std::fs::remove_file(&path);
            Err(io::Error::other(format!("外部编辑器以 {status} 退出")))
        }
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            Err(error)
        }
    }
}

/// Runs on one thread: Crossterm poll/read must share their event reader.
pub fn run(boot: Boot, requests: Sender<Request>, updates: Receiver<Update>) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "TUI 需要交互终端；脚本请用 saiku run / status",
        ));
    }
    let preferences = std::fs::read_to_string(&boot.preferences)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // 模型偏好与 UI 偏好分开存：坏掉的 `model.json` 只该丢掉收藏与最近，不该动别的。
    let model_prefs = catalog::ModelPrefs::load(&boot.model_prefs);
    install_panic_hook();
    // try_init may enter raw mode before failing to switch screens.
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init()?;
    execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
    // Kitty keyboard protocol (where the terminal supports it) is what makes Shift+Enter
    // distinguishable from Enter. Terminals that don't implement it ignore the sequence; on
    // Windows the Win32 input records already carry SHIFT_PRESSED, and Ctrl+J is the fallback.
    let _ = execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let mut app = app::App::new(boot, preferences, model_prefs);
    let _ = requests.send(Request::Refresh);
    let mut window_title = String::new();
    let result = (|| {
        loop {
            while let Ok(update) = updates.try_recv() {
                app.update(update);
            }
            // The leader chord expires on wall-clock time, not on the next keystroke; a poll
            // timeout is the only place that can notice the 2000 ms window closing.
            app.tick();
            if app.take_editor_request() {
                // The external editor needs a normal screen and normal keys; leave the
                // enhanced-key mode only if it was on.
                let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
                let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
                ratatui::restore();
                let draft = app.editor.text.clone();
                match run_external_editor(&draft) {
                    Ok(Some(text)) => {
                        app.editor.set_text(text);
                        app.notice = "已从外部编辑器回填".into();
                    }
                    Ok(None) => {}
                    Err(error) => app.notice = format!("外部编辑器失败：{error}"),
                }
                terminal = ratatui::try_init()?;
                execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
                let _ = execute!(
                    io::stdout(),
                    PushKeyboardEnhancementFlags(
                        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    )
                );
                app.dirty = true;
                continue;
            }
            if app.dirty {
                // 会话页没有标题栏（照 OpenCode），标题交给终端窗口 / 标签页 —— 窄终端上
                // 信息栏收起时，这是唯一还看得见「这是哪个会话」的地方。只在变了时写。
                let title = app.window_title();
                if title != window_title {
                    let _ = execute!(io::stdout(), crossterm::terminal::SetTitle(&title));
                    window_title = title;
                }
                // 扫描条每 40 ms 改页脚；可见光标跟随 buffer diff 的末尾移动，就会在
                // 左下角闪。先隐藏再画，并让支持同步刷新的终端一次提交整帧与输入框光标。
                io::stdout().sync_update(|_| -> io::Result<()> {
                    terminal.hide_cursor()?;
                    terminal.draw(|frame| view::draw(frame, &mut app))?;
                    Ok(())
                })??;
                app.dirty = false;
            }
            // 一次 poll 之后把**已经排队**的按键一起收走：Windows 上粘贴就是一整串按键
            // （没有 `Event::Paste`），只有看到整批才能把其中的 Enter 当换行而不是提交。
            if event::poll(Duration::from_millis(50))? {
                let mut batch = vec![event::read()?];
                while event::poll(Duration::ZERO)? {
                    batch.push(event::read()?);
                    // 上限只是防病态输入：正常粘贴不会有一万个按键排在队列里。
                    if batch.len() >= 4096 {
                        break;
                    }
                }
                if app.input_batch(batch, &requests) {
                    return Ok(());
                }
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
    // Input history and recent `@` files ride along with the UI preferences.
    app.persist_preferences();
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
