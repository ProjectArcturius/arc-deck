//! 运行时进程的启动与收尾。
//!
//! 收尾这件事必须真的做到「确认退出」才算数：SDK **没有单会话取消**，
//! 客户端放弃一个轮次的办法就是关掉运行时进程。所以「已请求取消」与
//! 「进程已退出」是两个不同的结论，不能混着报。

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

/// 一个已被拉起的运行时子进程，标准流全部接管。
pub struct RuntimeProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

/// 收尾结果。
#[derive(Debug, Clone)]
pub struct Termination {
    /// 是否**确认**进程已退出。为 false 时调用方不得声称取消已完成。
    pub exited: bool,
    pub detail: String,
}

/// 按 pid 结束一棵进程树 —— **不是我们自己拉起的那些**。
///
/// 为什么需要它：监工被 `kill -9` 之后，它拉起的运行时（以及运行时拉起的工具）会变成
/// 孤儿 —— `kill_on_drop` 与进程组都随监工一起没了，而监工已经死了、读不到取消文件。
/// **实测**（Arch，2026-10-14）：`kill -9` 监工之后，DSH 运行时仍是活的，没有任何东西会
/// 回收它。所以「取消」必须在监工已死时由调用方自己来收。
///
/// Unix：监工给运行时建过独立进程组（pgid = pid），整组发 SIGKILL，再**有界确认**
/// 组真的空了才算数。Windows：`taskkill /T /F`（本机无法验证）。
pub fn kill_tree_by_pid(pid: u32) -> Termination {
    #[cfg(unix)]
    {
        let group = format!("-{pid}");
        let signal = |sig: &str| {
            std::process::Command::new("kill")
                .args([sig, "--", &group])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        signal("-KILL");
        // 有界确认：进程组不在了才算「已结束」。信号发出去不等于没人活着。
        for _ in 0..40 {
            if !signal("-0") {
                return Termination { exited: true, detail: "运行时进程组已全部退出".into() };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        return Termination {
            exited: false,
            detail: format!("运行时进程组 {pid} 在 2 秒内没有全部退出"),
        };
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // 拿不到「组已空」的可比判据（taskkill 的退出码分不清「本来就没有」与「杀掉了」），
        // 所以这里如实报「已请求」，不冒充已确认。**本机没有 Windows 目标，未实测。**
        Termination { exited: false, detail: "已请求 taskkill /T /F（未确认）".into() }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Termination { exited: false, detail: "这个平台不支持按 pid 收进程树".into() }
    }
}

impl RuntimeProcess {
    /// 拉起子进程。
    ///
    /// `inherit_env = false`（夹具）时只传显式给出的环境变量：默认继承会把父进程的
    /// 凭据、代理与 `DSH_*` 变量悄悄带给一个本该隔离的运行时。
    ///
    /// 真实 DSH 用 `inherit_env = true`：它要执行的是工人的 shell 工具（cargo、git、
    /// 代理变量、`CARGO_TARGET_DIR` 都得在），而 Ether 的 runner 脚本本来也是整份继承。
    /// 继承时仍会去掉父进程里的 `DSH_*` 与 `ARCDECK_KEY_*`，由调用方显式重设 ——
    /// 免得调度者 shell 里残留的 `DSH_HOME` 把任务指到 Desktop 的真实 home 上。
    pub fn spawn(
        program: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: Option<&Path>,
        inherit_env: bool,
    ) -> std::io::Result<Self> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // 自成一个进程组，收尾时才能连子孙一起结束（Windows 用 taskkill /T）。
        #[cfg(unix)]
        command.process_group(0);
        if inherit_env {
            for (key, _) in std::env::vars_os() {
                let k = key.to_string_lossy();
                if k.starts_with("DSH_") || k.starts_with("ARCDECK_KEY_") {
                    command.env_remove(&key);
                }
            }
        } else {
            command.env_clear();
            // PATH 与系统根目录不是秘密，但缺了它们连子进程都起不来。
            for key in ["PATH", "SystemRoot", "windir", "TEMP", "TMP", "HOME", "USERPROFILE"] {
                if let Ok(value) = std::env::var(key) {
                    command.env(key, value);
                }
            }
        }
        for (key, value) in env {
            command.env(key, value);
        }
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("stdin 已 piped");
        let stdout = child.stdout.take().expect("stdout 已 piped");
        let stderr = child.stderr.take().expect("stderr 已 piped");
        Ok(Self { child, stdin: Some(stdin), stdout: Some(stdout), stderr: Some(stderr) })
    }

    /// 取走三条标准流。之后本对象只剩「等它退出 / 结束它」的职责。
    pub fn take_stdio(&mut self) -> (ChildStdout, ChildStdin, ChildStderr) {
        (
            self.stdout.take().expect("stdout 只能被取走一次"),
            self.stdin.take().expect("stdin 只能被取走一次"),
            self.stderr.take().expect("stderr 只能被取走一次"),
        )
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// 结束进程树并**确认**退出。
    ///
    /// Windows 上用 `taskkill /T /F` 连子孙一起结束 —— 只 `kill` 父进程会留下
    /// 仍在写日志或仍持有文件锁的孤儿。Unix 上对 spawn 时建立的进程组整组发
    /// SIGKILL；**Debian 上尚未实测**，自己 `setsid` 出去的孙进程不在组里。
    pub async fn terminate_tree(&mut self, grace: Duration) -> Termination {
        #[cfg(windows)]
        if let Some(pid) = self.child.id() {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        // Unix：spawn 时已自成进程组（pgid = pid），对整组发 SIGKILL。
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        let _ = self.child.start_kill();
        match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(Ok(status)) => Termination { exited: true, detail: format!("进程已退出（{status}）") },
            Ok(Err(e)) => Termination { exited: false, detail: format!("等待进程退出失败：{e}") },
            Err(_) => Termination {
                exited: false,
                detail: format!("等待进程退出超时（{} ms）", grace.as_millis()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 起一个自成一组的 `sleep`，并**立刻把它交给 init**。
    ///
    /// 为什么要绕这一道：如果测试进程自己持有这个子进程，杀掉它之后它会变成**我们**
    /// 的僵尸，`/proc` 里照样在，于是收树的确认窗口每次都要白等满 2 秒 —— 那验的就不
    /// 是真实情形了。真实的孤儿运行时正是「没有一个会 wait 它的父进程」。
    #[cfg(target_os = "linux")]
    fn spawn_detached_sleeper() -> u32 {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg("setsid sleep 60 >/dev/null 2>&1 & echo $!")
            .output()
            .expect("起一个分离的 sleep");
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().expect("拿到 pid");
        // 留一点时间让 `sh` 退干净、sleep 交出去。
        std::thread::sleep(Duration::from_millis(200));
        assert!(std::path::Path::new(&format!("/proc/{pid}")).exists(), "sleep 该活着");
        pid
    }

    /// 按 pid 收一棵**不是自己拉起的**进程树。
    #[cfg(target_os = "linux")]
    #[test]
    fn kill_tree_by_pid_reaps_a_process_group_we_do_not_own() {
        let pid = spawn_detached_sleeper();
        let termination = kill_tree_by_pid(pid);
        assert!(termination.exited, "没收干净：{}", termination.detail);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }

    /// 对一个早就不存在的 pid 收树：应当是「已经没东西了」，而不是报错或卡住。
    #[cfg(target_os = "linux")]
    #[test]
    fn reaping_an_already_gone_pid_is_immediate_and_not_an_error() {
        let pid = spawn_detached_sleeper();
        assert!(kill_tree_by_pid(pid).exited);

        let started = std::time::Instant::now();
        let termination = kill_tree_by_pid(pid);
        assert!(termination.exited, "{}", termination.detail);
        assert!(started.elapsed() < Duration::from_secs(1), "不该等满确认窗口");
    }
}
