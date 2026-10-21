//! `dsh --profile web` 的内核进程：起来、换票、收掉。
//!
//! ## 为什么不复用 SDK 那条路的启动方式
//!
//! 命令行形状一样（`node <bin.js> --profile <name> [--patch <文件>]`），但**参数的
//! 位置有讲究**：0.2.1 的 launcher 只解析自己的那几个 flag，**遇到第一个不认识的
//! token 就把剩下的全部原样交给 app**（`apps/cli/src/args.ts:159-166` 与
//! `args.ts:167` 的 `[args...]`）。所以 `--patch` 必须写在 `--no-open` 这类
//! app 参数**之前** —— 反过来写，内核会把 `--patch` 当成自己的参数、报一句
//! 与真正原因无关的用法错误。
//!
//! ## 换票
//!
//! 内核起来后往 stdout 打一行 `dsh web: http://127.0.0.1:<port>/?token=<令牌>`
//! （`packages/bundle/web-app/src/index.ts:280`）。换票只有一条路：
//! `GET /?token=…` 回 **303** + `Set-Cookie`，cookie 名是
//! `dsh-auth-<authority 的 sha256>`、值是 `v1.<body>.<sig>`
//! （`packages/client/connection/src/browser-auth.ts:250-292`）。
//!
//! **不跟随重定向**：`Set-Cookie` 挂在 303 那一跳上，跟随之后拿到的最终响应里
//! 没有它（C# 那一版第一次就是这么踩的，0.2.1 的代码路径一模一样，所以这条仍然成立）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc;

use crate::process::{kill_tree_by_pid, RuntimeProcess, Termination};

use super::{redact, RemoteError, Secret};

/// 启动期的默认上限。内核要装完一整棵插件树（实测本机几十秒量级，冷启动更久）。
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

/// 启动失败时带出来的 stderr 尾部长度上限。
const TAIL_LIMIT: usize = 4000;

/// 带票 URL 的 host 部分。我们**自己**传 `--host 127.0.0.1`，所以这里可以写死；
/// 写死也意味着「内核绑到了别处」会在这里被认出来（而不是连到一个陌生地址上）。
const LOOPBACK_PREFIX: &str = "http://127.0.0.1:";

/// 起一个 web 内核要的全部输入。
pub struct WebKernelOptions {
    /// `node` 可执行文件。用 [`find_node`] 找。
    pub node: PathBuf,
    /// `node` 要执行的入口，通常是 `RuntimeLayout::entry()`。
    pub entry: PathBuf,
    /// **必填**，且由调用方负责它是「这一次会话自己的 home」。
    pub dsh_home: PathBuf,
    /// 额外的 `--patch` 叠加层。`None` 就只跑 profile 自带的那几层。
    pub patch: Option<PathBuf>,
    /// 进程级权限档（`DSH_PERMISSION_MODE`）。
    ///
    /// **默认不设**：web 面的权限是**按会话**选的（界面上那排预设），进程级盖一个值
    /// 会把内核里那套按会话的选择按死。只有确实要一个统一档时才传。
    pub permission_mode: Option<String>,
    /// 额外环境变量（模型路由的假密钥之类）。会被 [`WebKernelOptions::dsh_home`]
    /// 推导出的固定项覆盖，见 [`WebKernel::start`]。
    pub env: BTreeMap<String, String>,
    /// 内核的工作目录。`None` 就是调用方的当前目录。
    pub cwd: Option<PathBuf>,
    pub startup_timeout: Duration,
}

impl WebKernelOptions {
    pub fn new(node: impl Into<PathBuf>, entry: impl Into<PathBuf>, dsh_home: impl Into<PathBuf>) -> Self {
        Self {
            node: node.into(),
            entry: entry.into(),
            dsh_home: dsh_home.into(),
            patch: None,
            permission_mode: None,
            env: BTreeMap::new(),
            cwd: None,
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
        }
    }
}

/// 找一份能跑内核的 `node`：先看运行时目录里自带的，再退到 `PATH`。
///
/// 顺序与 C# 的 `HarnessLocator.FindNode` 一致，理由也一样：运行时目录里那份
/// 与内核是同一批装进来的，`PATH` 上那份可能版本差很远。
pub fn find_node(runtime_dir: &Path) -> Option<PathBuf> {
    let bundled = runtime_dir
        .join("node_modules")
        .join("node")
        .join("bin")
        .join(if cfg!(windows) { "node.exe" } else { "node" });
    if bundled.is_file() {
        return Some(bundled);
    }
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// 一个**正在运行**的 web 内核，连同它的浏览器会话 cookie。
pub struct WebKernel {
    /// `Option` 只是为了让 Drop 能先收树再放手（Drop 里不能把字段移出去）。
    process: Option<RuntimeProcess>,
    /// 内核 stdin 的写端。**持有而不写**：留着它，内核就不会看到一个提前到来的 EOF。
    _stdin: Option<tokio::process::ChildStdin>,
    origin: String,
    port: u16,
    cookie: Secret,
    diagnostics: Arc<Diagnostics>,
}

impl std::fmt::Debug for WebKernel {
    /// 手写 `Debug`：默认派生会把 cookie 原样打出来，而 `Debug` 是最容易被
    /// 顺手塞进日志的那条路。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebKernel")
            .field("origin", &self.origin)
            .field("cookie", &self.cookie)
            .field("pid", &self.process.as_ref().and_then(RuntimeProcess::pid))
            .finish()
    }
}

impl WebKernel {
    /// 起内核 → 抓到带票 URL → 换出 cookie。
    ///
    /// 失败时给出**可读**的原因，并且**绝不**把带票 URL 或 cookie 写进消息里。
    pub async fn start(options: WebKernelOptions) -> Result<WebKernel, RemoteError> {
        if !options.entry.is_file() {
            return Err(RemoteError::Startup(format!(
                "找不到 DSH 入口 {}。先运行 `saiku runtime install`",
                options.entry.display()
            )));
        }

        let args = kernel_args(&options);
        let env = kernel_env(&options);

        let mut process = RuntimeProcess::spawn_no_window(
            &options.node,
            &args,
            &env,
            options.cwd.as_deref(),
            true,
        )
        .map_err(|e| {
            RemoteError::Startup(format!("起 `{}` 失败：{e}", options.node.display()))
        })?;

        let (stdout, stdin, stderr) = process.take_stdio();

        let diagnostics = Arc::new(Diagnostics::default());
        let found: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let ended = Arc::new(AtomicUsize::new(0));
        // 用 mpsc 当「有变化」的通知，而不是 `Notify`：`Notify::notified()` 只在
        // 被 poll 过之后才算登记，检查条件与登记之间有一段会丢唤醒的窗口，
        // 而那段窗口的代价是**整次启动卡到超时**。
        let (pulse_tx, mut pulse_rx) = mpsc::unbounded_channel::<()>();

        tokio::spawn(read_stdout(stdout, found.clone(), diagnostics.clone(), pulse_tx.clone()));
        tokio::spawn(read_stderr(stderr, found.clone(), diagnostics.clone(), pulse_tx, ended.clone()));

        let deadline = tokio::time::Instant::now() + options.startup_timeout;
        let token_url = loop {
            if let Some(url) = found.lock().expect("URL 锁没毒").clone() {
                break url;
            }
            if ended.load(Ordering::SeqCst) >= 1 {
                return Err(RemoteError::Startup(format!(
                    "内核在打印带票 URL 之前就退出了。它的 stderr 尾部：\n{}",
                    diagnostics.tail()
                )));
            }
            tokio::select! {
                _ = pulse_rx.recv() => {}
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(RemoteError::Startup(format!(
                        "等带票 URL 超时（{} 秒）。它的 stderr 尾部：\n{}",
                        options.startup_timeout.as_secs(),
                        diagnostics.tail()
                    )));
                }
            }
        };

        // 令牌一露头就记住它，之后所有对外文本都过一遍打码。
        diagnostics.remember(token_of(&token_url).unwrap_or_default());

        let (origin, cookie) = match exchange_token(&token_url).await {
            Ok(pair) => pair,
            Err(error) => {
                // 内核起来了但登录不上：把进程收掉再报，别留一个孤儿。
                let _ = process.terminate_tree(Duration::from_secs(5)).await;
                return Err(error);
            }
        };
        let port = parse_port(&origin).unwrap_or(0);

        Ok(WebKernel {
            process: Some(process),
            _stdin: Some(stdin),
            origin,
            port,
            cookie,
            diagnostics,
        })
    }

    /// 内核的 HTTP origin，例如 `http://127.0.0.1:51058`。
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// 内核的监听端口。mux 用它自己开 TCP 连接。
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// 换票得到的浏览器会话 cookie。所有 `/api` 请求与 WebSocket 都要带它。
    pub(crate) fn cookie(&self) -> &Secret {
        &self.cookie
    }

    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().and_then(RuntimeProcess::pid)
    }

    /// 收到的诊断尾巴（stderr，已打码）。排查「内核在抱怨什么」时唯一的第一手材料。
    pub fn stderr_tail(&self) -> String {
        self.diagnostics.tail()
    }

    /// 收掉整棵进程树并**确认**退出。
    ///
    /// 与 SDK 那条路同一个理由：内核自己没有「优雅停机」的口头接口，客户端的
    /// 收尾动作就是结束进程。返回的 [`Termination::exited`] 说得出「确认了没有」。
    pub async fn shutdown(&mut self) -> Termination {
        match self.process.as_mut() {
            Some(process) => process.terminate_tree(Duration::from_secs(10)).await,
            None => Termination { exited: true, detail: "进程已经收掉了".into() },
        }
    }
}

impl Drop for WebKernel {
    /// Drop 只是**兜底**：显式路径是 [`WebKernel::shutdown`]。
    ///
    /// 为什么不能只靠 `kill_on_drop`：那只收直接子进程，而 node 自己还会拉起
    /// 工具子进程；留下来的那些会继续持有文件锁。所以按**树**收
    /// （`process::kill_tree_by_pid`）。Drop 里只能同步做，Unix 上那一步带
    /// 有界确认（进程组通常一两百毫秒内就空了）。
    fn drop(&mut self) {
        if let Some(pid) = self.process.as_ref().and_then(RuntimeProcess::pid) {
            // 结果不在这里报：Drop 没有调用方可以给出结论。要结论就走 `shutdown`。
            let _ = kill_tree_by_pid(pid);
        }
    }
}

/// `node <entry> --profile web [--patch <文件>] --no-open --host 127.0.0.1 --port 0`
///
/// `--port 0` 让内核自己挑空闲端口 —— 于是并排跑两个内核不会撞端口，端口从
/// stdout 那一行里读回来（`web-app/src/index.ts:113-121` 就是这么组 URL 的）。
fn kernel_args(options: &WebKernelOptions) -> Vec<String> {
    let mut args = vec![
        options.entry.display().to_string(),
        "--profile".into(),
        "web".into(),
    ];
    if let Some(patch) = &options.patch {
        args.push("--patch".into());
        args.push(patch.display().to_string());
    }
    // 从这里开始是 app 自己的参数：launcher 一遇到 `--no-open` 就停止解析，
    // 所以上面那些 launcher flag 必须已经在前面（args.ts:159-166）。
    args.extend([
        // 不带 `--no-open` 内核会去拉起用户的默认浏览器 —— 那正是我们要取代的那个界面。
        "--no-open".into(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        "0".into(),
    ]);
    args
}

/// 内核要的环境。
///
/// 与 `runtime::runtime_env` 同一套规矩（`DSH_HOME` 必给、遥测关掉），但**不默认写
/// `DSH_PERMISSION_MODE`** —— 理由见 [`WebKernelOptions::permission_mode`]。
///
/// 固定项**最后**写：调用方往 `env` 里塞 `DSH_HOME` 不该能覆盖掉参数里那一个。
/// 这条防的是一个真实事故面 —— 内核指到 DSH Desktop 的真实 home 上就会接管
/// 那份 home 里的会话（AGENTS.md P15）。
fn kernel_env(options: &WebKernelOptions) -> BTreeMap<String, String> {
    let mut env = options.env.clone();
    env.insert(
        "DSH_HOME".into(),
        options.dsh_home.display().to_string(),
    );
    // OTel 只在用户主动反馈时上传；客户端自己起的内核没有反馈入口（base bundle 注释）。
    env.insert("DSH_TELEMETRY_DISABLED".into(), "1".into());
    if let Some(mode) = &options.permission_mode {
        env.insert("DSH_PERMISSION_MODE".into(), mode.clone());
    }
    env
}

/// 从一行里认出带票 URL。
///
/// 只认形状，不认行首的 `dsh web: `：C# 那一版实测 0.1.6 上 stdout 与 stderr
/// 两处都见过同一份文本，扫两处比漏掉便宜（0.2.1 的 `web-app` 只在 stdout 打，
/// 但内核之外的层往后也可能转发它）。
fn find_token_url(text: &str) -> Option<&str> {
    let start = text.find(LOOPBACK_PREFIX)?;
    let rest = &text[start + LOOPBACK_PREFIX.len()..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after_port = &rest[digits..];
    let marker = "?token=";
    let at = after_port.find(marker)?;
    let token = &after_port[at + marker.len()..];
    let len = token
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
        .count();
    if len == 0 {
        return None;
    }
    Some(&text[start..start + LOOPBACK_PREFIX.len() + digits + at + marker.len() + len])
}

/// 带票 URL 里那一段令牌本身。启动令牌是 32 字节随机数的 base64url
/// （`browser-auth.ts:14,55`），所以字符集是 `[A-Za-z0-9_-]`。
fn token_of(url: &str) -> Option<&str> {
    let marker = "?token=";
    let at = url.find(marker)?;
    let token = &url[at + marker.len()..];
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn parse_port(origin: &str) -> Option<u16> {
    origin.rsplit(':').next()?.parse().ok()
}

/// 换票：`GET /?token=…` → 303 + `Set-Cookie`。
///
/// 单独一个函数是为了能在**没有内核、没有 node** 的情况下被测（见本文件的
/// `fake_http` 测试夹具）：这一段判错方向的代价很高 —— 表现为「HTTP 200 但没拿到
/// cookie」，而 200 看着像成功。
async fn exchange_token(token_url: &str) -> Result<(String, Secret), RemoteError> {
    let client = reqwest::Client::builder()
        // **不跟随重定向**：`Set-Cookie` 挂在 303 那一跳上。
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| RemoteError::Io(e.without_url().to_string()))?;

    let response = client.get(token_url).send().await.map_err(|e| {
        // `without_url()` 是必须的：reqwest 的错误消息里默认带请求 URL，而
        // 这个请求的 URL **就是凭据**。
        RemoteError::Io(format!("换票请求失败：{}", e.without_url()))
    })?;

    let status = response.status();
    let mut cookie = None;
    for value in response.headers().get_all(reqwest::header::SET_COOKIE) {
        if let Ok(text) = value.to_str() {
            // 只取 `name=value` 那一段：属性（Path/HttpOnly/SameSite/过期）拼进
            // 请求头会变成非法 token（`browser-auth.ts:129` 的 sessionCookie 正带这些）。
            if let Some(pair) = text.split(';').next() {
                let pair = pair.trim();
                if !pair.is_empty() {
                    cookie = Some(pair.to_string());
                    break;
                }
            }
        }
    }

    let Some(cookie) = cookie else {
        return Err(RemoteError::Unauthenticated(format!(
            "换票没有拿到 Set-Cookie（HTTP {status}）。启动令牌只对根路径 `/` 有效，\
             且令牌与内核进程一一对应 —— 内核是刚由我们自己起来的时候，这不该发生"
        )));
    };

    let origin = origin_of(token_url).ok_or_else(|| {
        RemoteError::Protocol("带票 URL 里读不出 origin".into())
    })?;
    Ok((origin, Secret::new(cookie)))
}

/// 从带票 URL 里切出 `http://host:port`，**丢掉带票的路径与查询串**。
fn origin_of(token_url: &str) -> Option<String> {
    let start = token_url.find(LOOPBACK_PREFIX)?;
    let rest = &token_url[start + LOOPBACK_PREFIX.len()..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    Some(format!("{LOOPBACK_PREFIX}{}", &rest[..digits]))
}

/// 内核诊断输出的有界尾部 + 已知凭据的清单。
#[derive(Default)]
struct Diagnostics {
    tail: Mutex<String>,
    token: Mutex<Option<String>>,
}

impl Diagnostics {
    fn remember(&self, token: &str) {
        if token.is_empty() {
            return;
        }
        let mut guard = self.token.lock().expect("token 锁没毒");
        if guard.is_none() {
            *guard = Some(token.to_string());
        }
    }

    fn push_stderr(&self, line: &str) {
        let mut guard = self.tail.lock().expect("诊断锁没毒");
        guard.push_str(line);
        guard.push('\n');
        if guard.chars().count() > TAIL_LIMIT {
            let keep: String = guard
                .chars()
                .rev()
                .take(TAIL_LIMIT)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            *guard = keep;
        }
    }

    /// 尾巴，**已打码**。这条文本会进错误消息，所以打码在这里做，而不是在调用点。
    fn tail(&self) -> String {
        let guard = self.tail.lock().expect("诊断锁没毒");
        let token = self.token.lock().expect("token 锁没毒").clone();
        match token {
            Some(token) => redact(&guard, &[&token]),
            None => guard.clone(),
        }
    }
}

async fn read_stdout<R>(
    reader: R,
    found: Arc<Mutex<Option<String>>>,
    diagnostics: Arc<Diagnostics>,
    pulse: mpsc::UnboundedSender<()>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        note_token(&line, &found, &diagnostics, &pulse);
        // stdout 的其余内容不进诊断尾巴：带票 URL 就打在它上面，而尾巴会被
        // 原样塞进错误消息。少一份材料，换掉一条凭据泄露的路。
    }
    let _ = pulse.send(());
}

async fn read_stderr<R>(
    reader: R,
    found: Arc<Mutex<Option<String>>>,
    diagnostics: Arc<Diagnostics>,
    pulse: mpsc::UnboundedSender<()>,
    ended: Arc<AtomicUsize>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        note_token(&line, &found, &diagnostics, &pulse);
        diagnostics.push_stderr(&line);
    }
    ended.fetch_add(1, Ordering::SeqCst);
    let _ = pulse.send(());
}

/// 两个读任务都要一直读到 EOF。
///
/// **不是洁癖**：C# 那一版实测踩到过 —— 登录成功之后不再读 stderr，内核往 stderr
/// 写满 64 KB 管道缓冲后**阻塞在写调用上**，表现为「`session/follow` 永远不给快照，
/// 而 stdout 上一条错都没有」。这类「对端卡在写诊断信息上」的停滞最难查。
fn note_token(
    line: &str,
    found: &Arc<Mutex<Option<String>>>,
    diagnostics: &Arc<Diagnostics>,
    pulse: &mpsc::UnboundedSender<()>,
) {
    let Some(url) = find_token_url(line) else {
        return;
    };
    let mut guard = found.lock().expect("URL 锁没毒");
    if guard.is_none() {
        *guard = Some(url.to_string());
        diagnostics.remember(token_of(url).unwrap_or_default());
    }
    drop(guard);
    let _ = pulse.send(());
}

/// 内核 stdout/stderr 里那一行是否长成我们认识的样子。测试用。
#[cfg(test)]
pub(crate) fn debug_find_token_url(text: &str) -> Option<String> {
    find_token_url(text).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    /// 0.2.1 的 `web-app` 打这一行时的前缀（`packages/bundle/web-app/src/index.ts:280`）。
    /// **认 URL 不看它** —— 这条常量只用于断言「那一行的形状没变」。
    const URL_LINE_PREFIX: &str = "dsh web: ";

    /// 是否打了 `dsh web: ` 这个前缀。
    fn has_url_line_prefix(text: &str) -> bool {
        text.trim_start().starts_with(URL_LINE_PREFIX)
    }

    const SAMPLE_TOKEN: &str = "Xk3-_9aQmZ0vLpT7wRb1cNdE4fGhIjKlMnOpQrStUv";
    const SAMPLE_LINE: &str = "dsh web: http://127.0.0.1:51058/?token=Xk3-_9aQmZ0vLpT7wRb1cNdE4fGhIjKlMnOpQrStUv";

    #[test]
    fn the_printed_line_is_recognized() {
        assert!(has_url_line_prefix(SAMPLE_LINE));
        assert_eq!(
            debug_find_token_url(SAMPLE_LINE).unwrap(),
            format!("http://127.0.0.1:51058/?token={SAMPLE_TOKEN}")
        );
        // 0.1.6 上实测 stderr 也出现过同一份文本，所以不按行首前缀认。
        assert_eq!(
            debug_find_token_url(&format!("[harness] {SAMPLE_LINE}")).unwrap(),
            format!("http://127.0.0.1:51058/?token={SAMPLE_TOKEN}")
        );
    }

    #[test]
    fn url_without_a_token_is_not_a_token_url() {
        assert!(debug_find_token_url("dsh web: http://127.0.0.1:51058/").is_none());
        assert!(debug_find_token_url("http://127.0.0.1:51058/?token=").is_none());
        assert!(debug_find_token_url("listening on http://0.0.0.0:51058").is_none());
    }

    #[test]
    fn origin_drops_the_credential_bearing_tail() {
        let url = format!("http://127.0.0.1:51058/?token={SAMPLE_TOKEN}");
        assert_eq!(origin_of(&url).unwrap(), "http://127.0.0.1:51058");
        assert_eq!(token_of(&url).unwrap(), SAMPLE_TOKEN);
    }

    #[test]
    fn launcher_flags_come_before_app_flags() {
        // 0.2.1 的 launcher 遇到第一个不认识的 token 就把剩下全部交给 app，
        // 所以 `--patch` 必须排在 `--no-open` 前面。顺序错了内核起不来。
        let mut options = WebKernelOptions::new("node", "bin.js", "home");
        options.patch = Some(PathBuf::from("patch.json"));
        let args = kernel_args(&options);
        let patch = args.iter().position(|a| a == "--patch").unwrap();
        let no_open = args.iter().position(|a| a == "--no-open").unwrap();
        assert!(patch < no_open, "{args:?}");
        assert_eq!(&args[1..3], &["--profile", "web"]);
        // 端口交给内核自己挑：并排跑两个内核不该撞端口。
        assert_eq!(args[args.len() - 1], "0");
    }

    #[test]
    fn dsh_home_always_wins_over_caller_env() {
        let mut options = WebKernelOptions::new("node", "bin.js", "/tmp/isolated-home");
        // 调用方（或从别处抄来的配置）塞进来的 DSH_HOME **不该**生效。
        options.env.insert("DSH_HOME".into(), "/real/desktop/home".into());
        options.env.insert("SAIKU_KEY_X".into(), "fake".into());
        let env = kernel_env(&options);
        assert_eq!(env["DSH_HOME"], "/tmp/isolated-home");
        assert_eq!(env["DSH_TELEMETRY_DISABLED"], "1");
        assert_eq!(env["SAIKU_KEY_X"], "fake");
        // web 面的权限按会话给，进程级默认不设。
        assert!(!env.contains_key("DSH_PERMISSION_MODE"));
    }

    #[test]
    fn permission_mode_is_only_set_when_asked_for() {
        let mut options = WebKernelOptions::new("node", "bin.js", "home");
        options.permission_mode = Some("danger-full-access".into());
        assert_eq!(kernel_env(&options)["DSH_PERMISSION_MODE"], "danger-full-access");
    }

    #[test]
    fn stderr_tail_is_bounded_and_redacted() {
        let diagnostics = Diagnostics::default();
        for index in 0..500 {
            diagnostics.push_stderr(&format!("第 {index} 行"));
        }
        assert!(diagnostics.tail().chars().count() <= TAIL_LIMIT + 16);
        assert!(diagnostics.tail().contains("第 499 行"));

        diagnostics.remember(SAMPLE_TOKEN);
        diagnostics.push_stderr(&format!("回显了 {SAMPLE_LINE}"));
        let tail = diagnostics.tail();
        assert!(!tail.contains(SAMPLE_TOKEN), "令牌进了诊断尾巴：{tail}");
        assert!(tail.contains("<已打码>"), "{tail}");
    }

    /// 换票那一段的假 HTTP 服务端：**手写 HTTP/1.1**，因为要验的正是
    /// 「303 那一跳上的 Set-Cookie」这种原始形状。真服务端能挡住的东西不一样 ——
    /// 一个会自动跟随重定向或自动管 cookie 的客户端库会把它修好，然后我们学不到东西。
    fn fake_http(raw_response: &'static str) -> (String, u16) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 2048];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(raw_response.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}/?token={SAMPLE_TOKEN}"), port)
    }

    #[tokio::test]
    async fn token_exchange_keeps_only_the_cookie_pair() {
        let (url, port) = fake_http(concat!(
            "HTTP/1.1 303 See Other\r\n",
            "Location: ./\r\n",
            "Set-Cookie: dsh-auth-abc123=v1.body.sig; Max-Age=2592000; Path=/; HttpOnly; SameSite=Strict\r\n",
            "Content-Length: 0\r\n\r\n",
        ));
        let (origin, cookie) = exchange_token(&url).await.unwrap();
        assert_eq!(origin, format!("http://127.0.0.1:{port}"));
        // 属性不能带进请求头 —— 那会变成一个非法 token。
        assert_eq!(cookie.expose(), "dsh-auth-abc123=v1.body.sig");
        assert!(!cookie.expose().contains("HttpOnly"), "{}", cookie.expose());
    }

    #[tokio::test]
    async fn redirect_is_not_followed_so_the_cookie_is_seen() {
        // 跟随重定向之后拿到的是最终响应，而 Set-Cookie 挂在这一跳上。
        // 假服务端的 303 指向一个没人监听的端口：一旦跟随，请求会失败而不是拿到 cookie。
        let (url, _port) = fake_http(concat!(
            "HTTP/1.1 303 See Other\r\n",
            "Location: http://127.0.0.1:1/\r\n",
            "Set-Cookie: dsh-auth-abc123=v1.body.sig; Path=/\r\n",
            "Content-Length: 0\r\n\r\n",
        ));
        let (_origin, cookie) = exchange_token(&url).await.unwrap();
        assert_eq!(cookie.expose(), "dsh-auth-abc123=v1.body.sig");
    }

    #[tokio::test]
    async fn a_401_without_a_cookie_is_reported_as_unauthenticated() {
        let (url, _port) = fake_http(concat!(
            "HTTP/1.1 401 Unauthorized\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n",
            "Content-Length: 2\r\n\r\n",
            "no",
        ));
        let error = exchange_token(&url).await.unwrap_err();
        assert!(matches!(error, RemoteError::Unauthenticated(_)), "{error}");
    }

    #[tokio::test]
    async fn exchange_errors_never_carry_the_token() {
        // 连到一个没人接的端口：reqwest 的错误默认带请求 URL，而那个 URL 是凭据。
        let error = exchange_token("http://127.0.0.1:1/?token=LEAKME").await.unwrap_err();
        let text = error.to_string();
        assert!(!text.contains("LEAKME"), "凭据进了错误消息：{text}");
        assert!(!text.contains("token="), "带票 URL 进了错误消息：{text}");
    }
}
