//! 交互内核的启动参数：用户配置 → `WebKernelOptions`。
//!
//! 与 headless 的 `worker::dsh_runtime_command` 同一套规矩（路由由 `provider::resolve` 解析、
//! 密钥只走环境变量、授权级别经 `execution_guard` 投影成 patch 行），差别只有三处：
//!
//! 1. **profile 是 `web`**，不是 `sdk`：会话列表、历史、单会话取消只在 Remote 层有（§9.1）。
//! 2. **`DSH_HOME` 是长期的**（`<本机数据目录>/saiku/dsh-home`），不是每任务一份 ——
//!    交互会话要能在下次打开时恢复。它**绝不是** DSH Desktop 的 home（P15：follow 会接管会话）。
//! 3. **一份 home 只许一个内核**：两个内核写同一份会话日志，正是 P14 那种「倒退 4」的坏日志的
//!    候选成因。所以这里拿一把锁，拿不到就明说「另一个 saiku 正在用」，而不是悄悄并发。
//!
//! persona 由调用方装配好再给（`saiku` 的 `serve::interactive_persona`）：§5.3.1 人格默认关，
//! 关着时是空串、一段都不加。`confirm` 授权已经接通：审批策略写成 `ask`，审批请求经
//! `$events` 流送到界面（`events.rs`）。

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use arc_deck_core::authz::{EffectivePolicy, Permission};
use arc_deck_core::config::RouteSpec;
use arc_deck_core::error::{code, SpecError, SpecResult};
use arc_deck_core::mode::{AgentId, Mode};
use arc_deck_core::provider;
use arc_deck_core::userconfig::{self, UserConfig};

use crate::runtime::{render_patch_with_routes, runtime_env, PatchInput, RuntimeLayout};

use super::host::WebKernelOptions;

/// 交互面的默认 home。跨 TUI 重启保持不变，会话才能恢复。
pub fn default_dsh_home() -> PathBuf {
    userconfig::local_data_dir().join("saiku").join("dsh-home")
}

/// 启动一个交互内核所需的全部东西。
pub struct InteractiveLaunch {
    pub options: WebKernelOptions,
    /// 配置里的默认路由，`provider` 是 DSH 认的路由名（官方 DS 是 `deepseek-official`）。
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
    /// 授权说得比执行层细、因此只留档的项（同 headless）。
    pub guard_notes: Vec<String>,
    /// 持有期间这份 home 归本进程。**放掉它之前先收掉内核。**
    pub lock: HomeLock,
}

/// 调用方给的、不从配置里读的输入。
pub struct InteractiveInput<'a> {
    pub config: &'a UserConfig,
    pub workspace: &'a Path,
    pub permission: Permission,
    /// 当前 Agent 与模式。Plan 落到执行层是**只读**（写入与 shell 都被拒绝），
    /// 不只是换一段提示词。
    pub agent: AgentId,
    pub mode: Mode,
    pub dsh_home: &'a Path,
    /// 进 system prompt 的 persona 后缀。个性化关着时是空串（§5.3.1：一段都不注入）。
    pub persona_suffix: &'a str,
    /// 这个内核的标识，用于 OpenCode Go 的会话头与 `{task_id}` 模板。
    pub kernel_id: &'a str,
}

pub fn interactive(input: InteractiveInput<'_>) -> SpecResult<InteractiveLaunch> {
    let cfg = input.config;
    let layout = RuntimeLayout::new(cfg.runtime_dir(), cfg.dsh_version());
    layout.check().map_err(|m| SpecError::new(code::RUNTIME_ENTRY_MISSING, m))?;

    let lock = HomeLock::acquire(input.dsh_home)?;

    // 默认路由 + 用户配置里其余能解析、有密钥的 provider。后者解析失败**不阻断启动**：
    // 那只是模型对话框里少一组，默认路由能用就能对话。失败原因留档给界面看。
    let route = provider::resolve(&cfg.model, &cfg.provider, input.kernel_id)?;
    let default_spec = route.to_spec()?;
    let mut env = BTreeMap::new();
    env.insert(
        default_spec.key_env.clone(),
        provider::read_key_from_spec(&default_spec, &userconfig::auth_path())?,
    );
    let mut notes = Vec::new();
    notes.extend(lock.reaped.clone());
    let mut extra: Vec<RouteSpec> = Vec::new();
    for (id, block) in &cfg.provider {
        if *id == default_spec.provider_id {
            continue;
        }
        let Some(first) = block.models.keys().next() else {
            continue;
        };
        let attempt = provider::resolve(&format!("{id}/{first}"), &cfg.provider, input.kernel_id)
            .and_then(|r| r.to_spec())
            .and_then(|spec| {
                let key = provider::read_key_from_spec(&spec, &userconfig::auth_path())?;
                Ok((spec, key))
            });
        match attempt {
            Ok((spec, key)) => {
                env.insert(spec.key_env.clone(), key);
                extra.push(spec);
            }
            Err(error) => notes.push(format!("供应商 `{id}` 没有装进内核：{}", error.message)),
        }
    }

    // 交互面的策略：整个 workspace 按授权级别读写，shell 与网络开着。
    // 与 headless 一般模式相同 —— 收窄靠授权级别，不靠路径列表（DSH 表达不了，见 M3_GUARD §2）。
    // Plan 模式是**执行层**的只读：写入范围清空（→ `read-only` 沙箱）且 shell 行被摘掉。
    let policy = policy_for(input.permission, input.agent, input.mode, input.workspace);
    let guard = policy.execution_guard(input.permission)?;
    notes.extend(guard.recorded.iter().cloned());

    let patch = render_patch_with_routes(
        &PatchInput {
            route: &default_spec,
            persona_suffix: input.persona_suffix,
            session_log_upload: cfg.runtime.session_log_upload,
            guard: &guard,
            session_search: true,
        },
        &extra,
    );
    std::fs::create_dir_all(input.dsh_home).map_err(|e| {
        SpecError::new(code::CONFIG_INVALID, format!("建 {} 失败：{e}", input.dsh_home.display()))
    })?;
    let patch_path = input.dsh_home.join("saiku-web-patch.json");
    std::fs::write(&patch_path, patch).map_err(|e| {
        SpecError::new(code::CONFIG_INVALID, format!("写 {} 失败：{e}", patch_path.display()))
    })?;

    // 用户的 env（比如把 TEMP 挪到 D 盘）照 headless 的模板替换；固定项由 WebKernel 最后写。
    for (k, v) in &cfg.env {
        env.insert(
            k.clone(),
            v.replace("{task_id}", input.kernel_id)
                .replace("{workspace}", &input.workspace.display().to_string())
                .replace("{run_dir}", &input.dsh_home.display().to_string()),
        );
    }

    let node = cfg
        .runtime
        .node
        .clone()
        .or_else(|| super::host::find_node(&cfg.runtime_dir()))
        .unwrap_or_else(|| PathBuf::from("node"));
    let mut options = WebKernelOptions::new(node, layout.entry(), input.dsh_home);
    options.patch = Some(patch_path);
    options.cwd = Some(input.workspace.to_path_buf());
    options.env = env;
    // 进程级的档写成交集之后的那一个：与 patch 里的预设表是同一个值，
    // 这样 base bundle 里读 `DSH_PERMISSION_MODE` 的默认值不会与 patch 打架。
    for (k, v) in runtime_env(input.dsh_home, guard.sandbox) {
        if k == "DSH_PERMISSION_MODE" {
            options.permission_mode = Some(v);
        }
    }

    Ok(InteractiveLaunch {
        options,
        provider: route.dsh_provider.clone(),
        model: route.model.clone(),
        effort: cfg.effort.clone(),
        guard_notes: notes,
        lock,
    })
}

/// 交互面的有效授权：授权级别 + Agent/模式 → 执行层策略。
///
/// 为什么单独成一个函数：TUI 的 `/plan` `/build` `/permission` 都会走到这里，
/// 而「Plan 是只读」这件事必须是**执行层**的（写范围清空 → `read-only` 沙箱，
/// shell 行摘掉），不能只是换一段提示词（PRODUCT_PLAN §5.6、§7.4）。
pub fn policy_for(
    permission: Permission,
    agent: AgentId,
    mode: Mode,
    workspace: &Path,
) -> EffectivePolicy {
    let readonly = permission == Permission::Readonly;
    let plan = mode == Mode::Plan;
    let write = !readonly && !plan;
    EffectivePolicy {
        agent,
        mode,
        profile: if write { "workspace-development" } else { "workspace-readonly" }.into(),
        workspace: workspace.to_path_buf(),
        read_paths: vec![PathBuf::from(".")],
        write_paths: if write { vec![PathBuf::from(".")] } else { Vec::new() },
        // Plan 不许跑命令。只读 + 开着 shell 的话，`git commit` / `rm` 之类照样能改仓库，
        // 「只读」就只剩一句提示词。
        shell: !plan,
        network: true,
        commit: permission == Permission::Full && !plan,
        push: false,
    }
}

/// 一份 `DSH_HOME` 的独占锁。
///
/// 用「独占创建」的锁文件而不是 OS 文件锁：std 里没有跨平台的 `flock`，而本仓不许加依赖。
/// 代价是进程被硬杀后锁文件会留下 —— 所以锁里写 pid，拿锁时 pid 已经不在就当作遗留、接管。
pub struct HomeLock {
    path: PathBuf,
    _file: Option<File>,
    /// `<home>/saiku.kernel`：本进程拉起的内核 pid。**确认收掉内核之后才删**。
    kernel: PathBuf,
    /// 拿锁时收掉的遗留内核（给人看的一句话）。没有就是 `None`。
    pub reaped: Option<String>,
}

impl HomeLock {
    pub fn acquire(home: &Path) -> SpecResult<HomeLock> {
        std::fs::create_dir_all(home).map_err(|e| {
            SpecError::new(code::CONFIG_INVALID, format!("建 {} 失败：{e}", home.display()))
        })?;
        let path = home.join("saiku.lock");
        for _ in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    use std::io::Write as _;
                    let _ = write!(file, "{}", std::process::id());
                    let reaped = reap_recorded_kernel(home);
                    return Ok(HomeLock { path, _file: Some(file), kernel: home.join(KERNEL_FILE), reaped });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok());
                    match holder {
                        Some(pid) if crate::process::pid_alive(pid) => {
                            return Err(SpecError::new(
                                code::CONFIG_INVALID,
                                format!(
                                    "另一个 saiku（pid {pid}）正在使用交互会话目录 {}。\
                                     同一份会话日志不能有两个内核同时写（AGENTS.md P14）；先关掉那一个",
                                    home.display()
                                ),
                            ));
                        }
                        // 写锁的进程已经不在了（被硬杀），或锁文件残缺：接管。
                        _ => {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
                Err(error) => {
                    return Err(SpecError::new(
                        code::CONFIG_INVALID,
                        format!("建锁文件 {} 失败：{error}", path.display()),
                    ))
                }
            }
        }
        Err(SpecError::new(
            code::CONFIG_INVALID,
            format!("拿不到交互会话目录的锁 {}", path.display()),
        ))
    }
}

impl HomeLock {
    /// 记下本进程拉起的内核。下一个拿锁的进程若还看得见这条记录，说明这里没有确认收掉它。
    pub fn record_kernel(&self, pid: u32) {
        let _ = std::fs::write(&self.kernel, pid.to_string());
    }

    /// 内核**确认**退出了：删掉记录。没确认就留着，交给下一个拿锁的进程去收。
    pub fn kernel_exited(&self) {
        let _ = std::fs::remove_file(&self.kernel);
    }
}

const KERNEL_FILE: &str = "saiku.kernel";

/// 收掉上一个主人留下的内核。
///
/// 为什么需要（2026-10-20 实测）：TUI 被硬杀（关掉终端窗口不跑 Drop）之后，它拉起的 node 内核
/// 成了孤儿，**还活着**；下一个 saiku 看锁里的 pid 已死、照规矩接管 —— 然后起了**第二个**内核
/// 在同一份 home 上。那正是 P14「两个写者」的形状。锁文件只记录「谁拿着 home」，不记录
/// 「谁的内核还在跑」，所以另记一份内核 pid。
///
/// 收之前核对命令行（`--profile web` + 这份 home 的 patch 路径）：pid 会被复用，凭 pid 收进程
/// 可能杀到无关的进程（`pid_alive` 的注释说过这件事）。对不上就只删记录。
fn reap_recorded_kernel(home: &Path) -> Option<String> {
    let record = home.join(KERNEL_FILE);
    let pid = std::fs::read_to_string(&record).ok()?.trim().parse::<u32>().ok();
    let _ = std::fs::remove_file(&record);
    let pid = pid?;
    let normalize = |s: &str| s.replace('/', "\\").to_lowercase();
    let marker = normalize(&home.join("saiku-web-patch.json").display().to_string());
    let command = crate::process::command_line(pid)?;
    let command = normalize(&command);
    if !(command.contains("--profile web") && command.contains(&marker)) {
        return None;
    }
    let done = crate::process::kill_tree_by_pid(pid);
    Some(if done.exited {
        format!("收掉了上一个 saiku 留下的交互内核（pid {pid}）：它的主人没有正常退出")
    } else {
        format!("上一个 saiku 留下的交互内核（pid {pid}）没能确认收掉：{}", done.detail)
    })
}

impl Drop for HomeLock {
    fn drop(&mut self) {
        self._file = None;
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_deck_core::authz::ApprovalPolicy;

    /// Plan 必须在**执行层**是只读：写入清空 → read-only 沙箱，shell 行被摘掉。
    /// 只有提示词里的「不要写」不算（§5.6 第三条纪律）。
    #[test]
    fn plan_is_read_only_at_the_execution_layer() {
        for permission in [Permission::Workspace, Permission::Full] {
            let policy = policy_for(permission, AgentId::Echo, Mode::Plan, Path::new("C:/ws"));
            let guard = policy.execution_guard(permission).unwrap();
            assert_eq!(guard.sandbox, Permission::Readonly, "{permission:?}");
            assert!(!guard.shell_tools, "Plan 不能跑命令：{permission:?}");
            assert!(policy.write_paths.is_empty());
            assert_eq!(guard.approval, ApprovalPolicy::Never);
        }
    }

    /// Build 回到授权级别本身；`full` 只是沙箱更宽，不会顺手把审批拨成 ask。
    #[test]
    fn build_mode_keeps_the_permission_level() {
        let policy = policy_for(Permission::Full, AgentId::Build, Mode::Build, Path::new("C:/ws"));
        let guard = policy.execution_guard(Permission::Full).unwrap();
        assert_eq!(guard.sandbox, Permission::Full);
        assert!(guard.shell_tools);
        assert_eq!(guard.approval, ApprovalPolicy::Never);
    }

    /// `confirm` 与 `workspace` 同沙箱，差别只在**审批策略**：它要有人应答。
    #[test]
    fn confirm_keeps_the_workspace_sandbox_and_turns_asking_on() {
        let policy = policy_for(Permission::Confirm, AgentId::Echo, Mode::Build, Path::new("C:/ws"));
        let guard = policy.execution_guard(Permission::Confirm).unwrap();
        assert_eq!(guard.sandbox, Permission::Workspace);
        assert_eq!(guard.approval, ApprovalPolicy::Ask);
        assert!(guard.shell_tools);
    }

    #[test]
    fn a_second_lock_on_the_same_home_is_refused_while_the_first_lives() {
        let home = tempfile::tempdir().unwrap();
        let first = HomeLock::acquire(home.path()).unwrap();
        let error = HomeLock::acquire(home.path()).err().expect("第二把锁不该拿到");
        assert!(error.message.contains("另一个 saiku"), "{}", error.message);
        drop(first);
        HomeLock::acquire(home.path()).expect("第一把放掉之后可以再拿");
    }

    #[test]
    fn a_lock_left_by_a_dead_process_is_taken_over() {
        let home = tempfile::tempdir().unwrap();
        // 一个几乎不可能存活的 pid：被硬杀的那一次留下的锁文件。
        std::fs::write(home.path().join("saiku.lock"), "4294967294").unwrap();
        HomeLock::acquire(home.path()).expect("遗留锁应被接管");
    }

    /// 一个命令行里带着 `extra` 的长睡进程（替身「内核」）。不弹窗。
    fn sleeper(extra: &[String]) -> std::process::Child {
        #[cfg(windows)]
        let mut command = {
            let mut c = std::process::Command::new("powershell");
            // `#` 之后是注释：参数只为出现在命令行里，powershell 不执行它们。
            c.args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 120 #"]);
            c
        };
        #[cfg(unix)]
        let mut command = {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "sleep 120", "sh"]);
            c
        };
        crate::process::no_window(&mut command)
            .args(extra)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    /// 主人被硬杀后留下的内核：下一次拿锁时按记录收掉 —— 但只收**命令行对得上**的那个
    /// （pid 会被复用，2026-10-20 实测出孤儿内核那一次加的）。
    #[test]
    fn a_kernel_left_behind_is_reaped_only_when_its_command_line_matches() {
        let home = tempfile::tempdir().unwrap();
        let patch = home.path().join("saiku-web-patch.json").display().to_string();
        let mut stranger = sleeper(&["--profile".into(), "web".into(), "--patch".into(), "elsewhere.json".into()]);
        std::fs::write(home.path().join(KERNEL_FILE), stranger.id().to_string()).unwrap();
        let lock = HomeLock::acquire(home.path()).unwrap();
        assert!(lock.reaped.is_none(), "命令行对不上就不收");
        assert!(stranger.try_wait().unwrap().is_none(), "无关的进程必须还活着");
        let _ = stranger.kill();
        drop(lock);

        let mut orphan = sleeper(&["--profile".into(), "web".into(), "--patch".into(), patch]);
        std::fs::write(home.path().join(KERNEL_FILE), orphan.id().to_string()).unwrap();
        let lock = HomeLock::acquire(home.path()).unwrap();
        assert!(lock.reaped.as_deref().is_some_and(|text| text.contains("收掉了")), "{:?}", lock.reaped);
        let mut gone = false;
        for _ in 0..50 {
            if orphan.try_wait().unwrap().is_some() {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(gone, "遗留内核应被收掉");
        assert!(!home.path().join(KERNEL_FILE).exists(), "记录用过就删");
    }
}
