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
