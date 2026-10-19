//! 交互内核的启动参数：用户配置 → `WebKernelOptions`。
//!
//! 与 headless 的 `worker::dsh_runtime_command` 同一套规矩（路由由 `provider::resolve` 解析、
//! 密钥只走环境变量、授权级别经 `execution_guard` 投影成 patch 行），差别只有三处：
//!
//! 1. **profile 是 `web`**，不是 `sdk`：会话列表、历史、单会话取消只在 Remote 层有（§9.1）。
//! 2. **`DSH_HOME` 是长期的**（`<本机数据目录>/arcdeck/dsh-home`），不是每任务一份 ——
//!    交互会话要能在下次打开时恢复。它**绝不是** DSH Desktop 的 home（P15：follow 会接管会话）。
//! 3. **一份 home 只许一个内核**：两个内核写同一份会话日志，正是 P14 那种「倒退 4」的坏日志的
//!    候选成因。所以这里拿一把锁，拿不到就明说「另一个 arcdeck 正在用」，而不是悄悄并发。
//!
//! 还**没有**的（如实写下）：`confirm` 授权需要审批界面（M4 的 w4-tui-modes），这里遇到就拒绝启动，
//! 不把它悄悄当成 `workspace`（那等于把「每次问我」放宽成「直接做」）。

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

/// 交互内核的默认 home。跨 TUI 重启保持不变，会话才能恢复。
pub fn default_dsh_home() -> PathBuf {
    userconfig::local_data_dir().join("arcdeck").join("dsh-home")
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
    pub dsh_home: &'a Path,
    /// 进 system prompt 的 persona 后缀。个性化关着时是空串（§5.3.1：一段都不注入）。
    pub persona_suffix: &'a str,
    /// 这个内核的标识，用于 OpenCode Go 的会话头与 `{task_id}` 模板。
    pub kernel_id: &'a str,
}

pub fn interactive(input: InteractiveInput<'_>) -> SpecResult<InteractiveLaunch> {
    let cfg = input.config;
    if input.permission.needs_responder() {
        return Err(SpecError::new(
            code::CONFIG_INVALID,
            "授权级别 `confirm` 需要审批界面，交互会话还没有（M4 w4-tui-modes）；\
             请暂用 readonly / workspace / full",
        ));
    }

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
    let readonly = input.permission == Permission::Readonly;
    let policy = EffectivePolicy {
        agent: AgentId::Echo,
        mode: if readonly { Mode::Plan } else { Mode::Build },
        profile: input.permission.profile().into(),
        workspace: input.workspace.to_path_buf(),
        read_paths: vec![PathBuf::from(".")],
        write_paths: if readonly { Vec::new() } else { vec![PathBuf::from(".")] },
        shell: true,
        network: true,
        commit: input.permission == Permission::Full,
        push: false,
    };
    let guard = policy.execution_guard(input.permission)?;
    notes.extend(guard.recorded.iter().cloned());

    let patch = render_patch_with_routes(
        &PatchInput {
            route: &default_spec,
            persona_suffix: input.persona_suffix,
            session_log_upload: cfg.runtime.session_log_upload,
            guard: &guard,
        },
        &extra,
    );
    std::fs::create_dir_all(input.dsh_home).map_err(|e| {
        SpecError::new(code::CONFIG_INVALID, format!("建 {} 失败：{e}", input.dsh_home.display()))
    })?;
    let patch_path = input.dsh_home.join("arcdeck-web-patch.json");
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

/// 一份 `DSH_HOME` 的独占锁。
///
/// 用「独占创建」的锁文件而不是 OS 文件锁：std 里没有跨平台的 `flock`，而本仓不许加依赖。
/// 代价是进程被硬杀后锁文件会留下 —— 所以锁里写 pid，拿锁时 pid 已经不在就当作遗留、接管。
pub struct HomeLock {
    path: PathBuf,
    _file: Option<File>,
}

impl HomeLock {
    pub fn acquire(home: &Path) -> SpecResult<HomeLock> {
        std::fs::create_dir_all(home).map_err(|e| {
            SpecError::new(code::CONFIG_INVALID, format!("建 {} 失败：{e}", home.display()))
        })?;
        let path = home.join("arcdeck.lock");
        for _ in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    use std::io::Write as _;
                    let _ = write!(file, "{}", std::process::id());
                    return Ok(HomeLock { path, _file: Some(file) });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok());
                    match holder {
                        Some(pid) if crate::process::pid_alive(pid) => {
                            return Err(SpecError::new(
                                code::CONFIG_INVALID,
                                format!(
                                    "另一个 arcdeck（pid {pid}）正在使用交互会话目录 {}。\
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

impl Drop for HomeLock {
    fn drop(&mut self) {
        self._file = None;
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_on_the_same_home_is_refused_while_the_first_lives() {
        let home = tempfile::tempdir().unwrap();
        let first = HomeLock::acquire(home.path()).unwrap();
        let error = HomeLock::acquire(home.path()).err().expect("第二把锁不该拿到");
        assert!(error.message.contains("另一个 arcdeck"), "{}", error.message);
        drop(first);
        HomeLock::acquire(home.path()).expect("第一把放掉之后可以再拿");
    }

    #[test]
    fn a_lock_left_by_a_dead_process_is_taken_over() {
        let home = tempfile::tempdir().unwrap();
        // 一个几乎不可能存活的 pid：被硬杀的那一次留下的锁文件。
        std::fs::write(home.path().join("arcdeck.lock"), "4294967294").unwrap();
        HomeLock::acquire(home.path()).expect("遗留锁应被接管");
    }
}
