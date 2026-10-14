//! 探活：`status.json` 的写与读（PRODUCT_PLAN §6.5）。
//!
//! 写者（worker）只写**事实**：时间戳、pid、此刻在做什么、计数。健康由**读者**在读的那一刻
//! 按当前时间算 —— 写者死了就写不出「我死了」，所以「死了」只能由读者从心跳过期推出来。
//!
//! 这条分工取代 Ether runner 的「输出 30 分钟不增长」看门狗：那个判据把 20 分钟的
//! `cargo build` 和一次挂住的模型请求混为一谈，而这里分得开（工具在跑 vs 在等模型）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const STATUS_FILE: &str = "status.json";

/// 此刻在做什么。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// `starting` / `model` / `tool` / `verify` / `idle`。
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 毫秒时间戳。
    pub since: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    pub steps: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commits: Option<u64>,
    /// 工具调用被当正文吐出（DSML 等）的次数 —— DSH_FINDINGS E1 的数据来源。
    #[serde(default)]
    pub tool_call_as_text: u64,
    /// **中途**发生过的模型侧失败次数（DSH 的 llm-retry 在步边界重试）。
    ///
    /// 与 `last_error` 是两件事：这些不一定是失败 —— 只要最终自起了，任务就是成功的。
    /// 但监控程序要**实时**看见它：限流与额度用尽就是这样开始变坏的，等到终态才知道
    /// 就晚了（参 PRODUCT_PLAN §6.5.7）。
    #[serde(default)]
    pub model_retries: u64,
}

/// 最后一次值得调度者注意的**中途**异常。
///
/// 与 `last_error` 的区别：`last_error` 是终态原因；这个说的是「此刻正在出问题」——
/// 任务可能还在跑、还可能自己好。监控程序靠它实时反应（例如正在被限流时先别再派活儿）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    /// 毫秒时间戳。
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub heartbeat_s: u64,
    pub model_idle_s: u64,
    pub tool_stall_s: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastError {
    pub code: String,
    pub message: String,
    /// 能不能靠「再派一次」解决（调度者的下一个决定）。缺席 = false。
    ///
    /// 是给调度者的**建议**，不是自动动作：Arc Deck 自己不会因为它去重试。
    /// 判定表在 [`arc_deck_core::failure::retryable`]。
    #[serde(default)]
    pub retryable: bool,
}

/// `status.json`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusFile {
    pub schema_version: u32,
    pub task_id: String,
    /// `accepted/starting/running/verifying` 或终态名。
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<bool>,
    pub host: String,
    pub pid: u32,
    /// 监工进程自己的**启动时刻**（毫秒）。用来把 pid 复用与「同一个人」分开。
    ///
    /// 缺席 = 这个平台查不到（或这条记录是代写的）—— 读者就退回到只看心跳。
    /// 取法见 [`process_start_ms`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_pid: Option<u32>,
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 谁派的这件任务 —— 派工关系的**边**。缺席 = 没记录。
    ///
    /// 外部监控程序（`cute-agents-monitor`）拿它把「任务书流向」画出来：
    /// 每个工人一条边指回派它的人，任意深度都由这一条边重建。
    /// 放在 `status.json` 而不是 `task.json`，是为了让那个程序**只解析一个文件**。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_by: Option<String>,
    /// 任务书标题（取自指令首行）。给监控程序当节点标签；缺席就显示 `task_id`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub started_at: u64,
    pub heartbeat_at: u64,
    pub progress_at: u64,
    pub activity: Activity,
    /// 此刻正在出问题的东西（可能自己好）。缺席 = 没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<Notice>,
    pub counters: Counters,
    pub attempt: u32,
    pub max_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<LastError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<arc_deck_core::result::Usage>,
    pub limits: Limits,
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_string()))
        .unwrap_or_else(|| "unknown".into())
}

/// 写者。节流写盘：状态变化立刻写，进度更新至多每秒一次，心跳按间隔写。
pub struct StatusWriter {
    path: PathBuf,
    pub status: StatusFile,
    last_write: Option<Instant>,
    dirty: bool,
}

impl StatusWriter {
    pub fn new(run_dir: &Path, status: StatusFile) -> Self {
        let mut writer = Self { path: run_dir.join(STATUS_FILE), status, last_write: None, dirty: true };
        writer.flush(true);
        writer
    }

    /// 原子替换：先写临时文件再改名，读者永远读不到半个 JSON。
    pub fn flush(&mut self, force: bool) {
        if !force && !self.dirty {
            return;
        }
        if !force {
            if let Some(last) = self.last_write {
                if last.elapsed() < Duration::from_secs(1) {
                    return;
                }
            }
        }
        self.status.heartbeat_at = now_ms();
        let tmp = self.path.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string(&self.status) {
            if std::fs::write(&tmp, text).is_ok() {
                // Windows 上 rename 覆盖已存在文件在读者恰好打开时可能失败；失败就下一次再写。
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
        self.last_write = Some(Instant::now());
        self.dirty = false;
    }

    pub fn set_state(&mut self, state: &str) {
        self.status.state = state.to_string();
        self.flush(true);
    }

    pub fn set_activity(&mut self, kind: &str, name: Option<String>, summary: Option<String>) {
        let now = now_ms();
        self.status.activity = Activity { kind: kind.into(), name, summary: summary.map(|s| clip(&s, 80)), since: now };
        self.status.progress_at = now;
        self.dirty = true;
    }

    pub fn progress(&mut self) {
        self.status.progress_at = now_ms();
        self.dirty = true;
    }

    pub fn error(&mut self, code: &str, message: &str) {
        self.status.last_error = Some(LastError {
            code: code.into(),
            message: clip(message, 200),
            retryable: arc_deck_core::failure::retryable(code),
        });
        self.dirty = true;
    }

    /// 从一帧 DSH 事件里提取进度。只认「实质」事件：模型输出、工具开始与结束、步与轮的边界。
    pub fn observe(&mut self, event: &Value) {
        let data = event.get("data").unwrap_or(event);
        match event.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "step/start" => {
                self.status.counters.steps += 1;
                self.set_activity("model", None, None);
            }
            "assistant/message" | "assistant/delta" => self.progress(),
            "assistant/attempt" => {
                // 中途的模型失败要**立刻**可见。重试次数涨起来的时候任务还在跑 ——
                // 监控程序据此决定「先别再派活儿」，而不是等一个终态。
                for failure in arc_deck_dsh::projection::failure_details(event) {
                    // 结构化 code 优先 —— 与终态那条走同一个分类器，实时与收尾才不会
                    // 对同一件事给出两个判断（实测：一条月度额度用尽，message 含 "429"）。
                    let class = arc_deck_core::failure::classify_with(
                        None,
                        failure.code.as_deref(),
                        &failure.message,
                    );
                    self.status.counters.model_retries += 1;
                    self.status.notice = Some(Notice {
                        code: class.code.to_string(),
                        message: clip(&failure.message, 200),
                        retryable: class.retryable,
                        at: now_ms(),
                    });
                    self.dirty = true;
                }
                self.progress();
            }
            "tool/call" => {
                self.status.counters.tool_calls += 1;
                let name = data.get("name").and_then(|n| n.as_str()).map(str::to_string);
                let args = data
                    .get("arguments")
                    .and_then(|a| a.as_str())
                    .and_then(|a| serde_json::from_str::<Value>(a).ok());
                let summary = args.as_ref().and_then(|a| {
                    a.get("command")
                        .or_else(|| a.get("description"))
                        .or_else(|| a.get("path"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                });
                self.set_activity("tool", name, summary);
            }
            "tool/result" => {
                let is_error = data
                    .get("message")
                    .and_then(|m| m.get("isError"))
                    .and_then(|e| e.as_bool())
                    .unwrap_or(false);
                if is_error {
                    self.status.counters.tool_errors += 1;
                }
                self.set_activity("model", None, None);
            }
            "turn/end" => self.set_activity("idle", None, None),
            _ => {}
        }
    }
}

/// 监工进程还在不在。
///
/// **只在同一台机器上才可信** —— pid 只在它自己的机器上有意义，所以先比 `host`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    /// 确认不在了：pid 不存在，或存在但启动时刻对不上（pid 被复用）。
    Gone,
    /// 这个平台查不到，或记录与读者不在同一台机器。**不要把它当成 Gone。**
    Unknown,
}

/// 查监工进程的状态。
///
/// 支持面：**Linux**（读 `/proc`，纯 `std::fs`，没有 FFI）。其余平台一律返回
/// `Unknown` 并退回到心跳判据 —— 本仓目前在 Windows 上没有可编译验证的手段
/// （本机没装 Windows target，写了连 `cargo check` 都过不去），所以**不写**。
/// 这条降级是刻意的，不是遗漏。
///
/// 为什么值得有：心跳过期要等 `3 × heartbeat_s`（缺省 45 秒）才能判「死了」。
/// 进程确认不在时，这里立刻就能判。
pub fn supervisor_liveness(status: &StatusFile) -> Liveness {
    // 代写的记录（`pid = 0`）本来就没有监工。
    if status.pid == 0 {
        return Liveness::Unknown;
    }
    if status.host != host_name() {
        return Liveness::Unknown;
    }
    let Some(actual) = process_start_ms(status.pid) else {
        // 查不到这个 pid 的启动时刻：在 Linux 上就是「进程不存在」。
        return platform_liveness(status.pid);
    };
    match status.pid_started_at {
        // 记录的启动时刻与现在这个 pid 的对不上 —— 这是被复用的 pid，不是同一个人。
        // 容差取 2 秒：不同来源（jiffies 换算、时钟粒度）会有小差异。
        Some(recorded) if actual.abs_diff(recorded) > 2000 => Liveness::Gone,
        _ => Liveness::Alive,
    }
}

#[cfg(target_os = "linux")]
fn platform_liveness(pid: u32) -> Liveness {
    if std::path::Path::new(&format!("/proc/{pid}")).exists() {
        Liveness::Alive
    } else {
        Liveness::Gone
    }
}

#[cfg(not(target_os = "linux"))]
fn platform_liveness(_pid: u32) -> Liveness {
    Liveness::Unknown
}

/// 某个进程的启动时刻（毫秒，自 Unix 纪元）。查不到返回 `None`。
///
/// Linux: `/proc/<pid>/stat` 的第 22 个字段 `starttime`（自开机起的时钟节拍）
/// 加上 `/proc/stat` 的 `btime`。`USER_HZ` 在 Linux 上恒为 100。
#[cfg(target_os = "linux")]
pub fn process_start_ms(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `comm` 里可以有空格和括号，所以从**最后一个** `)` 之后再切字段。
    let rest = stat.rsplit_once(')')?.1;
    let starttime: u64 = rest.split_whitespace().nth(19)?.parse().ok()?;
    let btime = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(btime * 1000 + starttime * 10)
}

#[cfg(not(target_os = "linux"))]
pub fn process_start_ms(_pid: u32) -> Option<u64> {
    None
}

/// 卡死判定（写者自救与读者显示共用一条规则）。返回卡死原因。
pub fn stall_reason(status: &StatusFile, now: u64) -> Option<String> {
    let since_progress = now.saturating_sub(status.progress_at) / 1000;
    let in_activity = now.saturating_sub(status.activity.since) / 1000;
    match status.activity.kind.as_str() {
        "tool" if in_activity > status.limits.tool_stall_s => Some(format!(
            "工具 {} 已运行 {}，超过 {}",
            status.activity.name.as_deref().unwrap_or("?"),
            human(in_activity),
            human(status.limits.tool_stall_s)
        )),
        "model" | "starting" | "idle" if since_progress > status.limits.model_idle_s => Some(format!(
            "{} 无进展，超过 {}",
            human(since_progress),
            human(status.limits.model_idle_s)
        )),
        _ => None,
    }
}

/// 读者侧的健康。
pub fn health(status: &StatusFile, now: u64) -> &'static str {
    if status.terminal == Some(true) {
        return "done";
    }
    // 快路径：进程**确认**不在了就不必等心跳过期 —— 心跳那条要等 3 × heartbeat_s
    // （缺省 45 秒）。判据同时看 pid 与它的启动时刻，所以 pid 复用骗不过去。
    if supervisor_liveness(status) == Liveness::Gone {
        return "dead";
    }
    let since_heartbeat = now.saturating_sub(status.heartbeat_at) / 1000;
    if since_heartbeat > 3 * status.limits.heartbeat_s.max(1) {
        return "dead";
    }
    if stall_reason(status, now).is_some() {
        return "stalled";
    }
    if status.activity.kind == "tool" && now.saturating_sub(status.activity.since) > 5 * 60 * 1000 {
        return "busy";
    }
    "ok"
}

/// 单件任务 `status` / `wait` 的退出码（PRODUCT_PLAN §6.5.3）。
pub fn exit_code(status: &StatusFile, health: &str) -> i32 {
    if health == "dead" {
        return 5;
    }
    match status.state.as_str() {
        "completed" => 0,
        "failed" => 1,
        "blocked" => 2,
        "cancelled" => 3,
        "timed_out" => 4,
        "unknown" => 5,
        _ if health == "stalled" => 11,
        _ => 10,
    }
}

/// 一行状态。目标约 120 字节；这一行就是调度者每次检查要付的全部成本。
pub fn one_line(status: &StatusFile, health: &str, now: u64) -> String {
    // 终态之后用最后一次写盘的时间，否则「用时」会在任务结束后一直往上涨。
    let end = if health == "done" { status.heartbeat_at } else { now };
    let elapsed = human(end.saturating_sub(status.started_at) / 1000);
    let c = &status.counters;
    let mut tail = format!("steps {} · tools {}", c.steps, c.tool_calls);
    if c.tool_errors > 0 {
        tail.push_str(&format!(" ({} err)", c.tool_errors));
    }
    if let Some(n) = c.commits {
        tail.push_str(&format!(" · commits {n}"));
    }
    tail.push_str(&format!(" · {elapsed}"));
    if status.max_attempts > 1 {
        tail.push_str(&format!(" · att {}/{}", status.attempt, status.max_attempts));
    }
    let what = match health {
        "done" => match &status.last_error {
            Some(e) if status.state != "completed" => format!("{}: {}", e.code, clip(&e.message, 60)),
            _ => format!("详情：arcdeck report {}", status.task_id),
        },
        "dead" => match supervisor_liveness(status) {
            // 快路径：进程**确认**不在了。这条比「心跳过期」信息量更大，所以分开说 ——
            // 它同时意味着需要有人去收运行时进程树（`arcdeck cancel` 会做）。
            Liveness::Gone => "监工进程已不在（未写出终态，按 unknown 处理；`arcdeck cancel` 可收残余进程）".to_string(),
            _ => format!(
                "心跳停了 {}（监工没有回应，按 unknown 处理）",
                human(now.saturating_sub(status.heartbeat_at) / 1000)
            ),
        },
        "stalled" => stall_reason(status, now).unwrap_or_default(),
        _ => {
            let a = &status.activity;
            let dur = human(now.saturating_sub(a.since) / 1000);
            match (a.kind.as_str(), &a.name, &a.summary) {
                ("tool", Some(n), Some(s)) => format!("tool:{n} {dur} \"{}\"", clip(s, 48)),
                ("tool", Some(n), None) => format!("tool:{n} {dur}"),
                (k, _, _) => format!("{k} {dur}"),
            }
        }
    };
    format!("{}  {}  {}  {} · {}", status.task_id, status.state, health, what, tail)
}

pub fn read(path: &Path) -> Option<StatusFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 派工方替「还没开始就死了的工人」写下的终态。
///
/// **为什么由派工方写**：工人是在预检（读配置、查运行时、取密钥）里死的，而预检
/// 跑在第一个状态写手建立**之前** —— 它写不出自己的失败。没有这一条，一次配置错的
/// 派工在状态面上完全不存在：`arcdeck status --all` 报「没有任务」，真正的原因
/// 只躺在 `console.err` 里，调度者只能回去翻日志。那正是 §6.5 列的第四个问题
/// （后台化漏了就静默消失）换了个形式。
#[derive(Debug, Clone)]
pub struct PreflightFailure {
    pub task_id: String,
    pub workspace: String,
    pub model: Option<String>,
    pub assigned_by: Option<String>,
    pub title: Option<String>,
    pub code: String,
    pub message: String,
}

/// 原子写进 `<run_dir>/status.json`。目录不存在就什么都不做（返回 false）。
pub fn write_preflight_failure(run_dir: &Path, failure: PreflightFailure) -> bool {
    if !run_dir.is_dir() {
        return false;
    }
    let now = now_ms();
    let status = StatusFile {
        schema_version: arc_deck_core::SCHEMA_VERSION,
        task_id: failure.task_id,
        state: "failed".into(),
        terminal: Some(true),
        host: host_name(),
        // 没有监工进程可言：这一条记录是**代写的**，不冒充一个活着的写者。
        pid: 0,
        pid_started_at: None,
        runtime_pid: None,
        workspace: failure.workspace,
        model: failure.model,
        assigned_by: failure.assigned_by,
        title: failure.title,
        started_at: now,
        heartbeat_at: now,
        progress_at: now,
        activity: Activity { kind: "idle".into(), name: None, summary: None, since: now },
        notice: None,
        counters: Counters::default(),
        // 0 次：一次都没真正跑起来。`report` 里的「运行 N 次」要如实反映这一点。
        attempt: 0,
        max_attempts: 1,
        last_error: Some(LastError {
            code: failure.code.clone(),
            message: clip(&failure.message, 200),
            retryable: arc_deck_core::failure::retryable(&failure.code),
        }),
        usage: None,
        limits: Limits { heartbeat_s: 15, model_idle_s: 300, tool_stall_s: 1800 },
    };
    StatusWriter::new(run_dir, status);
    true
}

pub fn human(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn clip(text: &str, max_chars: usize) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max_chars {
        one_line
    } else {
        let mut out: String = one_line.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// 任务书标题：指令的第一行非空内容，去掉 Markdown 的 `#` 前缀。
///
/// 只做这一件事 —— **不解析任务书的语义**。PRODUCT_PLAN §6.6 已经立过同款规矩：
/// 不去猜「验证」段里的命令。取标题是另一回事，它不改任何判定，取错了最多是
/// 界面上少一个好看的标签。
pub fn derive_title(instruction: &str) -> Option<String> {
    let line = instruction.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line = line.trim_start_matches('#').trim();
    if line.is_empty() {
        None
    } else {
        Some(clip(line, 80))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(now: u64) -> StatusFile {
        StatusFile {
            schema_version: 1,
            task_id: "fix-filter".into(),
            state: "running".into(),
            terminal: None,
            host: "h".into(),
            pid: 1,
            pid_started_at: None,
            runtime_pid: None,
            workspace: "/w".into(),
            model: None,
            assigned_by: None,
            title: None,
            started_at: now - 18 * 60_000,
            heartbeat_at: now - 5_000,
            progress_at: now - 10_000,
            activity: Activity { kind: "model".into(), name: None, summary: None, since: now - 10_000 },
            notice: None,
            counters: Counters { steps: 42, tool_calls: 118, tool_errors: 0, commits: Some(2), tool_call_as_text: 0, model_retries: 0 },
            attempt: 1,
            max_attempts: 3,
            last_error: None,
            usage: None,
            limits: Limits { heartbeat_s: 15, model_idle_s: 600, tool_stall_s: 1800 },
        }
    }
    const NOW: u64 = 10_000_000_000;

    #[test]
    fn fresh_heartbeat_and_progress_is_ok() {
        let s = sample(NOW);
        assert_eq!(health(&s, NOW), "ok");
        assert_eq!(exit_code(&s, "ok"), 10);
    }

    #[test]
    fn stale_heartbeat_is_dead_even_if_the_file_says_running() {
        let mut s = sample(NOW);
        s.heartbeat_at = NOW - 60_000;
        assert_eq!(health(&s, NOW), "dead");
        assert_eq!(exit_code(&s, "dead"), 5);
    }

    #[test]
    fn long_build_is_busy_not_stalled() {
        let mut s = sample(NOW);
        s.activity = Activity { kind: "tool".into(), name: Some("pwsh".into()), summary: Some("cargo build".into()), since: NOW - 20 * 60_000 };
        s.progress_at = NOW - 20 * 60_000;
        assert_eq!(health(&s, NOW), "busy");
    }

    #[test]
    fn waiting_on_model_too_long_is_stalled() {
        let mut s = sample(NOW);
        s.progress_at = NOW - 11 * 60_000;
        s.activity.since = s.progress_at;
        assert_eq!(health(&s, NOW), "stalled");
        assert_eq!(exit_code(&s, "stalled"), 11);
    }

    #[test]
    fn title_comes_from_the_first_heading() {
        assert_eq!(derive_title("# 修复会话筛选\n\n正文……").as_deref(), Some("修复会话筛选"));
        // 没有 `#` 也行 —— 标题只是标签，不是语义解析。
        assert_eq!(derive_title("\n  离线夹具任务  \n").as_deref(), Some("离线夹具任务"));
        // 一行的空白会被压平，超长会截断并留省略号。
        let long = format!("# {}", "很长".repeat(60));
        assert!(derive_title(&long).unwrap().chars().count() <= 80);
        assert_eq!(derive_title("   \n\n").as_deref(), None);
    }

    #[test]
    fn assigned_by_round_trips_through_the_status_file() {
        let mut s = sample(NOW);
        s.assigned_by = Some("sol".into());
        s.title = Some("修复会话筛选".into());
        let text = serde_json::to_string(&s).unwrap();
        let back: StatusFile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.assigned_by.as_deref(), Some("sol"));
        assert_eq!(back.title.as_deref(), Some("修复会话筛选"));
        // 缺席时不写进文件 —— 监控程序据此判断「没有这条边」。
        let bare = serde_json::to_string(&sample(NOW)).unwrap();
        assert!(!bare.contains("assigned_by"));
        assert!(!bare.contains("\"title\""));
    }

    /// 起一个子进程、杀掉它、再问它还在不在 —— 不靠假 pid，靠操作系统给答案。
    #[cfg(target_os = "linux")]
    #[test]
    fn a_killed_process_is_confirmed_gone_without_waiting_for_the_heartbeat() {
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("起一个 sleep");
        let pid = child.id();
        assert!(process_start_ms(pid).is_some(), "活着的进程要能查到启动时刻");
        child.kill().expect("杀");
        child.wait().expect("收尸");

        assert_eq!(platform_liveness(pid), Liveness::Gone);

        // 关键那条：心跳**刚刚写过**，但进程已经确认不在 —— 立刻判 dead，
        // 不必等 3 × heartbeat_s（缺省 45 秒）。
        let mut s = sample(NOW);
        s.host = host_name();
        s.pid = pid;
        s.pid_started_at = None;
        assert_eq!(health(&s, NOW), "dead");
        assert_eq!(exit_code(&s, "dead"), 5);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_reused_pid_is_not_the_same_supervisor() {
        // 自己这个进程肯定活着，但记录里写的是另一个启动时刻 —— 那就是 pid 被复用了。
        let me = std::process::id();
        let actual = process_start_ms(me).expect("查自己");
        let mut s = sample(NOW);
        s.host = host_name();
        s.pid = me;
        s.pid_started_at = Some(actual.saturating_sub(60_000));
        assert_eq!(supervisor_liveness(&s), Liveness::Gone);

        // 对得上的时候才是 Alive。
        s.pid_started_at = Some(actual);
        assert_eq!(supervisor_liveness(&s), Liveness::Alive);
    }

    #[test]
    fn another_machines_pid_is_never_trusted() {
        // pid 只在它自己的机器上有意义：host 对不上就不下结论，退回心跳判据。
        let mut s = sample(NOW);
        s.host = "some-other-box".into();
        s.pid = 1;
        assert_eq!(supervisor_liveness(&s), Liveness::Unknown);
        s.heartbeat_at = NOW - 5_000;
        assert_eq!(health(&s, NOW), "ok");
    }

    #[test]
    fn a_supervisor_written_record_has_no_pid_to_check() {
        // 派工方代写的失败记录 `pid = 0`：本来就没有监工，别去查它。
        let mut s = sample(NOW);
        s.host = host_name();
        s.pid = 0;
        assert_eq!(supervisor_liveness(&s), Liveness::Unknown);
    }

    #[test]
    fn terminal_states_map_to_run_exit_codes() {
        let mut s = sample(NOW);
        s.terminal = Some(true);
        for (state, code) in [("completed", 0), ("failed", 1), ("blocked", 2), ("cancelled", 3), ("timed_out", 4), ("unknown", 5)] {
            s.state = state.into();
            assert_eq!(exit_code(&s, health(&s, NOW)), code, "{state}");
        }
    }

    #[test]
    fn one_line_stays_short() {
        let mut s = sample(NOW);
        s.activity = Activity {
            kind: "tool".into(),
            name: Some("pwsh".into()),
            summary: Some("cargo test -p sysstate --offline -- --nocapture some_really_long_filter_name_here".into()),
            since: NOW - 252_000,
        };
        let line = one_line(&s, "ok", NOW);
        assert!(line.contains("tool:pwsh 4m12s"), "{line}");
        assert!(line.chars().count() <= 160, "{} 字：{line}", line.chars().count());
    }
}
