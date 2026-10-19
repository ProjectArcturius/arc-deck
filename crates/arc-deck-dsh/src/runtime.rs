//! DSH 运行时的安装、定位与每任务 patch（PRODUCT_PLAN §9.2a）。
//!
//! **不用 submodule、不 vendor 源码、不从源码构建。** DSH 在 npm 上按包发布，
//! `dsh` CLI 只是按 profile 组合 bundle 的启动器；所以这里在用户数据目录下建一个
//! 私有目录，把 `@deepseek-ai/dsh` 钉成精确版本装进去，用 `node <bin.js> --profile sdk` 启动。
//! 全局 npm 的 `dsh` 不碰 —— 它与 DSH Desktop 共用，版本也不受我们控制。

use std::path::{Path, PathBuf};
use std::process::Command;

use arc_deck_core::authz::{ExecutionGuard, Permission};
use arc_deck_core::config::RouteSpec;
use serde_json::{json, Value};

/// 一个已知位置的运行时目录。
#[derive(Debug, Clone)]
pub struct RuntimeLayout {
    pub dir: PathBuf,
    pub version: String,
}

impl RuntimeLayout {
    pub fn new(dir: impl Into<PathBuf>, version: impl Into<String>) -> Self {
        Self { dir: dir.into(), version: version.into() }
    }

    fn package_dir(&self) -> PathBuf {
        self.dir.join("node_modules").join("@deepseek-ai").join("dsh")
    }

    /// `node` 要执行的入口。
    pub fn entry(&self) -> PathBuf {
        self.package_dir().join("lib").join("bin.js")
    }

    /// 实际装着的版本（读 `package.json`），没装返回 None。
    pub fn installed_version(&self) -> Option<String> {
        let raw = std::fs::read_to_string(self.package_dir().join("package.json")).ok()?;
        let v: Value = serde_json::from_str(&raw).ok()?;
        v.get("version").and_then(|v| v.as_str()).map(str::to_string)
    }

    /// 装好了且版本与钉住的一致。版本不一致不算可用 —— 钉版本的意义就在这里。
    pub fn check(&self) -> Result<(), String> {
        match self.installed_version() {
            None => Err(format!(
                "DSH 运行时未安装：{}。先运行 `arcdeck runtime install`",
                self.dir.display()
            )),
            Some(v) if v != self.version => Err(format!(
                "运行时目录里是 DSH {v}，但配置钉的是 {}；重新运行 `arcdeck runtime install`",
                self.version
            )),
            Some(_) if !self.entry().is_file() => {
                Err(format!("找不到入口 {}", self.entry().display()))
            }
            Some(_) => Ok(()),
        }
    }

    /// 写一份精确钉版本的 `package.json` 并 `npm install`。输出直接继承给调用者的终端。
    pub fn install(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("建目录 {} 失败：{e}", self.dir.display()))?;
        let manifest = json!({
            "name": "arcdeck-dsh-runtime",
            "private": true,
            "description": "Arc Deck 私有的 DSH 运行时；版本由 arcdeck 钉住，不要手改",
            "dependencies": { "@deepseek-ai/dsh": self.version },
        });
        std::fs::write(
            self.dir.join("package.json"),
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )
        .map_err(|e| format!("写 package.json 失败：{e}"))?;

        let args = ["install", "--omit=dev", "--no-audit", "--no-fund"];
        // Windows 上 npm 是 `npm.cmd`，`Command::new("npm")` 找不到它；经 cmd 转一道。
        let status = if cfg!(windows) {
            Command::new("cmd").arg("/C").arg("npm").args(args).current_dir(&self.dir).status()
        } else {
            Command::new("npm").args(args).current_dir(&self.dir).status()
        }
        .map_err(|e| format!("无法运行 npm（需要 Node.js 与 npm 在 PATH 上）：{e}"))?;
        if !status.success() {
            return Err(format!("npm install 失败（{status}）"));
        }
        self.check()
    }
}

/// 每任务 patch 需要的输入。
pub struct PatchInput<'a> {
    pub route: &'a RouteSpec,
    /// 进 persona 后缀的全文（Arc Deck 的 Agent 提示词 + 工人守则）。
    pub persona_suffix: &'a str,
    pub session_log_upload: bool,
    /// 由 `EffectivePolicy::execution_guard` 算出的执行层限制。
    ///
    /// 判定**不在这里做**：这一侧只按 `docs/M3_GUARD.md` 的映射表把 guard 翻成配置行。
    pub guard: &'a ExecutionGuard,
}

/// 生成 `--patch` 文件内容。
///
/// 用 JSON 写：YAML 是 JSON 的超集，DSH 的 `--dump-config` 与启动都实测能读，
/// 生成端因此不需要 YAML 依赖。每一行都按 id 定位 base bundle 里的行；
/// 注意 patch 会**整块替换**目标行的 `config`，不做深合并（DSH CLI reference）。
///
/// 限制行的来源是 [`PatchInput::guard`]，映射表在 `docs/M3_GUARD.md`：
/// 每一行都能对回表里的一格（包名 / 配置键 / 取值 / 保证范围）。
pub fn render_patch(input: &PatchInput<'_>) -> String {
    render_patch_with_routes(input, &[])
}

/// 与 [`render_patch`] 相同，另外把 `extra_routes` 也装进内核。
///
/// 交互会话要用：worker 的路由在 `initialize` 时钉死（§7.2），一份 patch 一条路由足够；
/// 而交互内核里用户会在模型对话框里**跨 provider** 切换（`session/selectModel`），
/// 内核只认 patch 里装过的适配器 —— 没装的 provider 在目录里根本不出现。
pub fn render_patch_with_routes(input: &PatchInput<'_>, extra_routes: &[RouteSpec]) -> String {
    let mut rows: Vec<Value> = Vec::new();
    let guard = input.guard;

    // ── 执行层限制 ────────────────────────────────────────────────────────
    //
    // 沙箱档**显式写出**，不吃 base bundle 的 `DSH_PERMISSION_MODE` 默认值：
    // 那个默认只反映启动配置，而最终的档是「启动级别 ∩ 策略上限」（§5.6 / §9.2）。
    // `workspaceRoot` 也给绝对路径：base 用的是 `process.cwd()`，我们不让它猜。
    rows.push(json!({
        "id": "sandbox-policy",
        "config": {
            "mode": guard.sandbox_mode(),
            "workspaceRoot": guard.policy.workspace.display().to_string(),
        },
    }));

    // 审批策略恒为 `never`：headless 没有应答者（§5.6 里 `confirm` 被明确拒绝），
    // 而 `never` 在 `ApprovalService.decide` 里**先于任何 answerer 分派**就返回 rejected ——
    // 也就是说即使组合里混进了别的应答者，`sandbox_permissions` 那类提权请求也拿不到
    // `allowed-once`（`packages/interaction/user-approval/src/index.ts` 的 decide()）。
    rows.push(json!({ "id": "approval", "config": { "policy": "never" } }));

    // 预设表整块重写（patch 不做深合并）。只留一格，且与上面两行是同一组取值。
    //
    // 这一行不是可有可无的：`PermissionPresetService` 建表时用「组合默认值反查预设」，
    // 查不到就抛 `composed sandbox and approval defaults match no preset`，插件**激活失败**
    // （实测：stderr 上 `dsh: warning: 1 entry did not activate` + 那句抛错）。
    // 后果不是内核起不来（其余部分照常起来），而是会话拿不到钉进日志的
    // `sandbox/mode` / `approval/policy` —— 而子 Agent 继承的正是父会话**显式的那条**
    // （`captureDelegatedPolicyOverrides` 取 `sandboxPolicy.overrideOf(parent.session)`）。
    // 只留一格的第二个作用：`/permission` 没有能切到更宽档的入口。
    // 名字用产品层的级别名而不是 DSH 的档名（§5.6：说给人听的一律用产品名）。
    let level = guard.sandbox.as_str();
    let mut presets = serde_json::Map::new();
    presets.insert(
        level.to_string(),
        json!({
            "sandbox": guard.sandbox_mode(),
            "approval": "never",
            "name": level,
            "description": "Arc Deck 本次运行的授权级别；只能由启动配置与任务书收紧",
        }),
    );
    rows.push(json!({ "id": "permission", "config": { "presets": Value::Object(presets) } }));

    // Arc Deck 的 Plan 由自己管：DSH 的 plan-mode 是**协作状态**，官方 README 就写着
    // 「It does not restrict the agent: every tool stays callable」，它的出口
    // `exit_plan_mode` 又要一个人工评审通道 —— headless 没有（§5.3：不得不经适配直接
    // 要求 headless 调用其人工评审工具）。这里改由沙箱 + 提示词共同承担。
    rows.push(json!({ "id": "plan-mode", "disabled": true }));

    if !guard.shell_tools {
        // shell = false 时**摘掉工具行**：工具不存在就不可执行，不依赖提示词或前缀匹配。
        // 这六行覆盖 base bundle 里所有能跑命令或跑代码的模型可见入口（映射表 §2.3）：
        // bash / pwsh、后台任务、workflow 与它的 PTC 运行时（那两行是同一件事的两半）。
        for id in ["tool-bash", "tool-pwsh", "tool-jobs", "tool-workflow", "ptc-runtime", "workflow-ptc"] {
            rows.push(json!({ "id": id, "disabled": true }));
        }
    }
    if !guard.web_tools {
        // 唯一的网络开关就是这两行工具。底层 provider 行（web / web-search-* / web-fetch-*）
        // 自身不发起请求，摘掉工具即可 —— 但 `network: false` + shell 还开着时
        // 命令照样能联网，那种组合在 `execution_guard` 里已经拒绝启动。
        rows.push(json!({ "id": "tool-web", "disabled": true }));
    }

    // 会话日志随官方请求上传 DeepSeek 是 DSH 默认开的（DSH_FINDINGS §2）。不显式打开就关。
    if !input.session_log_upload {
        rows.push(json!({ "id": "session-log-deepseek", "disabled": true }));
    }
    // 插件清单上报：同一类请求元数据，对 worker 没有用处。
    rows.push(json!({ "id": "plugin-package-inventory-deepseek", "disabled": true }));

    rows.push(json!({
        "id": "system-prompt",
        "config": { "personaPrefix": "", "personaSuffix": escape_template(input.persona_suffix) },
    }));

    rows.extend(llm_rows(std::iter::once(input.route).chain(extra_routes.iter())));
    serde_json::to_string_pretty(&Value::Array(rows)).unwrap_or_default()
}

/// 模型适配器行。
///
/// **pi-ai 的所有 provider 必须并进同一行**：patch 对目标行是整块替换 `config`、不做深合并，
/// 分成两行写，后一行会把前一行的 provider 整个盖掉。同名 provider 只取第一次出现的。
fn llm_rows<'a>(routes: impl Iterator<Item = &'a RouteSpec>) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut official_done = false;
    let mut pi_providers = serde_json::Map::new();
    for route in routes {
        if route.adapter == arc_deck_core::provider::DEEPSEEK_OFFICIAL_ROUTE {
            if official_done {
                continue;
            }
            official_done = true;
            let mut config = json!({ "apiKeyEnv": route.key_env });
            if let Some(base) = &route.base_url {
                config["baseURL"] = json!(base);
            }
            rows.push(json!({ "id": "llm-deepseek", "config": config }));
            continue;
        }
        if pi_providers.contains_key(&route.provider_id) {
            continue;
        }
        let mut provider = json!({ "apiKeyEnv": route.key_env });
        if let Some(name) = &route.display_name {
            provider["displayName"] = json!(name);
        }
        if let Some(api) = &route.api {
            provider["api"] = json!(api);
        }
        if let Some(base) = &route.base_url {
            provider["baseURL"] = json!(base);
        }
        if !route.headers.is_empty() {
            provider["headers"] = json!(route.headers);
        }
        let models: Vec<Value> = route
            .models
            .iter()
            .map(|m| {
                let mut v = json!({ "id": m.id });
                if let Some(name) = &m.name {
                    v["name"] = json!(name);
                }
                if let Some(cw) = m.context_window {
                    v["contextWindow"] = json!(cw);
                }
                v
            })
            .collect();
        provider["models"] = Value::Array(models);
        pi_providers.insert(route.provider_id.clone(), provider);
    }
    if !pi_providers.is_empty() {
        rows.push(json!({
            "id": "llm-pi-ai",
            "config": { "providers": Value::Object(pi_providers) },
        }));
    }
    rows
}

/// persona 模板会插值 `{{变量}}`，未定义的变量让整次组装失败（dsh-system-prompt README）。
/// 工人守则里出现字面 `{{` 时拆开，宁可多一个空格，也不能让每一步请求都组装失败。
fn escape_template(text: &str) -> String {
    text.replace("{{", "{ {").replace("}}", "} }")
}

/// 运行时进程要的固定环境。密钥另由调用方按 `route.key_env` 注入。
///
/// `mode` 收的是 [`Permission`] 而不是字符串：档名只有一个出处（`Permission::sandbox`），
/// 调用方也没有机会在这里塞进一个「只看启动配置」的值。
pub fn runtime_env(dsh_home: &Path, mode: Permission) -> Vec<(String, String)> {
    vec![
        ("DSH_HOME".into(), dsh_home.display().to_string()),
        // OTel 只在用户主动反馈时上传；worker 没有反馈入口，直接关掉（base bundle 注释）。
        ("DSH_TELEMETRY_DISABLED".into(), "1".into()),
        // base bundle 的沙箱与审批默认值都读这个变量（`danger-full-access` 时审批为 never）。
        // 这里给的是**交集之后**的档：base 的 `!!js` 默认只反映启动配置，不吃它。
        ("DSH_PERMISSION_MODE".into(), mode.sandbox().to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_deck_core::authz::{ExecutionPlan, Profile};
    use arc_deck_core::config::RouteModel;
    use arc_deck_core::error::{code, SpecResult};
    use arc_deck_core::task::TaskSpec;
    use std::collections::BTreeMap;

    fn pi_route() -> RouteSpec {
        RouteSpec {
            provider_id: "opencode-go".into(),
            adapter: "pi-ai".into(),
            api: Some("anthropic-messages".into()),
            base_url: Some("https://opencode.ai/zen/go".into()),
            headers: BTreeMap::from([("x-opencode-session".to_string(), "t1".to_string())]),
            models: vec![RouteModel { id: "deepseek-v4.1-flash".into(), name: None, context_window: None }],
            display_name: Some("OpenCode Go".into()),
            key_env: "ARCDECK_KEY_OPENCODE_GO".into(),
            key_source: "auth".into(),
        }
    }

    /// 矩阵里用的固定 workspace：两个平台上都得是绝对路径，否则配置解析就退了。
    fn workspace() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from("C:/work/project")
        } else {
            PathBuf::from("/work/project")
        }
    }

    /// 造一份**真实**的启动配置 + 任务书，走 `resolve_with_ceiling` 得到策略。
    ///
    /// 刻意不手写 `EffectivePolicy`：手写会绕开「第三层上限」与 Plan 模式那两条
    /// 收窄规则，而那两条正是这轮要接进执行层的东西 —— 绕开它们，测试再绿也说明不了。
    #[allow(clippy::too_many_arguments)]
    fn resolve(
        profile: &str,
        agent: &str,
        mode: Option<&str>,
        read: &[&str],
        write: &[&str],
        network: bool,
        commit: bool,
        push: bool,
        constraints: Value,
        ceiling: Option<Profile>,
    ) -> SpecResult<ExecutionPlan> {
        let ws = workspace().display().to_string();
        let config = json!({
            "schema_version": 1,
            "runtime": { "kind": "fixture-sdk" },
            "state_root": "state",
            "authorization": {
                "workspace": ws,
                "profile": profile,
                "read_paths": read,
                "write_paths": write,
                "network": network,
                "allow_commit": commit,
                "allow_push": push,
                "allowed_agents": ["echo", "build", "plan"],
            },
            "agents": { "dir": "agents", "prompts_dir": "prompts" },
        })
        .to_string();
        let task = json!({
            "schema_version": 1,
            "task_id": "t1",
            "workspace": ws,
            "agent": agent,
            "mode": mode,
            "instruction": "hello",
            "constraints": constraints,
        })
        .to_string();
        let cfg = arc_deck_core::config::WorkerConfig::parse(&config, Path::new("."))?;
        let spec = TaskSpec::parse(&task, Path::new("."))?;
        ExecutionPlan::resolve_with_ceiling(&cfg, &spec, ceiling)
    }

    fn guard(plan: &ExecutionPlan, startup: Permission) -> SpecResult<ExecutionGuard> {
        plan.policy.execution_guard(startup)
    }

    fn render(guard: &ExecutionGuard) -> Vec<Value> {
        let route = pi_route();
        let text = render_patch(&PatchInput {
            route: &route,
            persona_suffix: "",
            session_log_upload: false,
            guard,
        });
        serde_json::from_str::<Value>(&text).unwrap().as_array().unwrap().clone()
    }

    /// 取一行；缺了就报错而不是 `Option` —— 断言里到处 `unwrap()` 会把「没写这一行」
    /// 混进「值不对」里，两者的修法完全不同。
    fn row<'a>(rows: &'a [Value], id: &str) -> &'a Value {
        rows.iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("patch 里没有 `{id}` 这一行：{rows:#?}"))
    }

    fn is_disabled(rows: &[Value], id: &str) -> bool {
        rows.iter().any(|r| r["id"] == id && r["disabled"] == true)
    }

    /// shell 类工具行 —— `shell = false` 时这六行必须全部摘掉（映射表 §4）。
    const SHELL_ROWS: [&str; 6] =
        ["tool-bash", "tool-pwsh", "tool-jobs", "tool-workflow", "ptc-runtime", "workflow-ptc"];

    /// 一行 patch 的完整断言：档名、预设名、shell 行、web 行。
    fn assert_patch(rows: &[Value], sandbox: &str, level: &str, shell: bool, web: bool) {
        let policy = row(rows, "sandbox-policy");
        assert_eq!(policy["config"]["mode"], sandbox);
        // 路径比较用 `Path` 而不是字符串：Windows 上 `/` 与 `\` 是同一个分隔符，
        // 按字符串比会因为归一化形式不同而假失败。
        let root = Path::new(policy["config"]["workspaceRoot"].as_str().unwrap());
        assert!(root.is_absolute(), "workspaceRoot 必须是绝对路径，不能留给 base 的 process.cwd()");
        assert_eq!(root, workspace().as_path(), "workspaceRoot 必须就是本次任务的 workspace");
        assert_eq!(row(rows, "approval")["config"]["policy"], "never");

        // 预设表必须只有一格，且与 sandbox-policy / approval 是同一组取值：
        // `PermissionPresetService` 用「组合默认值反查预设」，查不到就抛在建表那一步。
        let presets = row(rows, "permission")["config"]["presets"].as_object().unwrap();
        assert_eq!(presets.len(), 1, "预设表只该留当前那一格：{presets:#?}");
        let (name, spec) = presets.iter().next().unwrap();
        assert_eq!(name, level);
        assert_eq!(spec["sandbox"], sandbox);
        assert_eq!(spec["approval"], "never");

        assert!(is_disabled(rows, "plan-mode"), "DSH 的协作式 plan-mode 一律不挂");
        for id in SHELL_ROWS {
            assert_eq!(is_disabled(rows, id), !shell, "`{id}` 的开关与 shell={shell} 不符");
        }
        assert_eq!(is_disabled(rows, "tool-web"), !web, "`tool-web` 的开关与 network={web} 不符");
    }

    #[test]
    fn pi_ai_patch_has_route_and_no_secret() {
        let route = pi_route();
        let plan = resolve(
            "workspace-development", "echo", None, &["."], &["."], true, false, false, Value::Null, None,
        )
        .unwrap();
        let guard = guard(&plan, Permission::Workspace).unwrap();
        let text = render_patch(&PatchInput {
            route: &route,
            persona_suffix: "守则 {{x}}",
            session_log_upload: false,
            guard: &guard,
        });
        let v: Value = serde_json::from_str(&text).unwrap();
        let rows = v.as_array().unwrap();
        assert!(rows.iter().any(|r| r["id"] == "session-log-deepseek" && r["disabled"] == true));
        let pi = rows.iter().find(|r| r["id"] == "llm-pi-ai").unwrap();
        let p = &pi["config"]["providers"]["opencode-go"];
        assert_eq!(p["api"], "anthropic-messages");
        assert_eq!(p["apiKeyEnv"], "ARCDECK_KEY_OPENCODE_GO");
        assert_eq!(p["headers"]["x-opencode-session"], "t1");
        assert_eq!(p["models"][0]["id"], "deepseek-v4.1-flash");
        let sp = rows.iter().find(|r| r["id"] == "system-prompt").unwrap();
        assert_eq!(sp["config"]["personaSuffix"], "守则 { {x} }");
    }

    #[test]
    fn official_route_patches_the_native_adapter_and_upload_can_be_opted_in() {
        let mut route = pi_route();
        route.adapter = "deepseek-official".into();
        route.provider_id = "deepseek".into();
        route.key_env = "ARCDECK_KEY_DEEPSEEK".into();
        route.base_url = None;
        let plan = resolve(
            "workspace-development", "echo", None, &["."], &["."], true, false, false, Value::Null, None,
        )
        .unwrap();
        let guard = guard(&plan, Permission::Workspace).unwrap();
        let text = render_patch(&PatchInput {
            route: &route,
            persona_suffix: "",
            session_log_upload: true,
            guard: &guard,
        });
        let v: Value = serde_json::from_str(&text).unwrap();
        let rows = v.as_array().unwrap();
        assert!(!rows.iter().any(|r| r["id"] == "session-log-deepseek"));
        let ds = rows.iter().find(|r| r["id"] == "llm-deepseek").unwrap();
        assert_eq!(ds["config"]["apiKeyEnv"], "ARCDECK_KEY_DEEPSEEK");
        assert!(!rows.iter().any(|r| r["id"] == "llm-pi-ai"));
    }

    /// 矩阵：Agent / 模式 × 授权级别 × shell 开关 × 收窄路径，逐一断言到具体行与取值。
    ///
    /// 每一格都是「启动配置 → Agent 档位 → 任务书 → 策略 → guard → patch 行」的全程，
    /// 不是拿手写的 `ExecutionGuard` 去测渲染 —— 后者只能证明渲染器会打字。
    #[test]
    fn every_policy_combination_lands_on_concrete_rows() {
        // (说明, 启动档, agent, mode, read, write, network, commit, push, constraints, 上限, 启动级别, 期望档, 期望级别名, shell, web)
        struct Case {
            what: &'static str,
            profile: &'static str,
            agent: &'static str,
            mode: Option<&'static str>,
            read: &'static [&'static str],
            write: &'static [&'static str],
            network: bool,
            commit: bool,
            push: bool,
            constraints: Value,
            ceiling: Option<Profile>,
            startup: Permission,
            sandbox: &'static str,
            level: &'static str,
            shell: bool,
            web: bool,
        }
        let cases = vec![
            Case {
                what: "workspace 档：可写可跑，档位原样",
                profile: "workspace-development",
                agent: "echo",
                mode: None,
                read: &["."],
                write: &["."],
                network: true,
                commit: false,
                push: false,
                constraints: Value::Null,
                ceiling: None,
                startup: Permission::Workspace,
                sandbox: "workspace-write",
                level: "workspace",
                shell: true,
                web: true,
            },
            Case {
                what: "full 档：提交需要它，写还开着时不许被降档",
                profile: "workspace-development",
                agent: "build",
                mode: None,
                read: &["."],
                write: &["."],
                network: true,
                commit: true,
                push: false,
                constraints: Value::Null,
                ceiling: None,
                startup: Permission::Full,
                sandbox: "danger-full-access",
                level: "full",
                shell: true,
                web: true,
            },
            Case {
                what: "readonly 档：写与 shell 全关",
                profile: "workspace-readonly",
                agent: "echo",
                mode: None,
                read: &["."],
                write: &[],
                network: true,
                commit: false,
                push: false,
                constraints: Value::Null,
                ceiling: None,
                startup: Permission::Workspace,
                sandbox: "read-only",
                level: "readonly",
                shell: false,
                web: true,
            },
            Case {
                what: "启动给 full、策略不许写：只能收紧成 read-only",
                profile: "workspace-readonly",
                agent: "build",
                mode: None,
                read: &["."],
                write: &[],
                network: true,
                commit: true,
                push: false,
                constraints: Value::Null,
                ceiling: None,
                startup: Permission::Full,
                sandbox: "read-only",
                level: "readonly",
                shell: false,
                web: true,
            },
            Case {
                what: "Plan 模式：即使档位允许写也不写、不跑",
                profile: "workspace-development",
                agent: "plan",
                mode: None,
                read: &["."],
                write: &["."],
                network: true,
                commit: false,
                push: false,
                constraints: Value::Null,
                ceiling: None,
                startup: Permission::Full,
                sandbox: "read-only",
                level: "readonly",
                shell: false,
                web: true,
            },
            Case {
                what: "Echo + plan 模式：同上",
                profile: "workspace-development",
                agent: "echo",
                mode: Some("plan"),
                read: &["."],
                write: &["."],
                network: true,
                commit: false,
                push: false,
                constraints: Value::Null,
                ceiling: None,
                startup: Permission::Workspace,
                sandbox: "read-only",
                level: "readonly",
                shell: false,
                web: true,
            },
            Case {
                what: "Agent 定义只读：比启动配置更严，写与 shell 被砍掉",
                profile: "workspace-development",
                agent: "build",
                mode: None,
                read: &["."],
                write: &["."],
                network: true,
                commit: false,
                push: false,
                constraints: Value::Null,
                ceiling: Some(Profile::WorkspaceReadonly),
                startup: Permission::Workspace,
                sandbox: "read-only",
                level: "readonly",
                shell: false,
                web: true,
            },
            Case {
                what: "任务书关 shell 但保留写：沙箱仍是 workspace-write，只摘 shell 行",
                profile: "workspace-development",
                agent: "echo",
                mode: None,
                read: &["."],
                write: &["."],
                network: true,
                commit: false,
                push: false,
                constraints: json!({ "shell_policy": "workspace-readonly" }),
                ceiling: None,
                startup: Permission::Workspace,
                sandbox: "workspace-write",
                level: "workspace",
                shell: false,
                web: true,
            },
            Case {
                what: "任务书关网络：只摘 web 行（shell 也归 off，否则拒绝启动，见下一个用例）",
                profile: "workspace-development",
                agent: "echo",
                mode: None,
                read: &["."],
                write: &["."],
                network: false,
                commit: false,
                push: false,
                constraints: json!({ "shell_policy": "workspace-readonly" }),
                ceiling: None,
                startup: Permission::Workspace,
                sandbox: "workspace-write",
                level: "workspace",
                shell: false,
                web: false,
            },
        ];

        for case in cases {
            let plan = resolve(
                case.profile,
                case.agent,
                case.mode,
                case.read,
                case.write,
                case.network,
                case.commit,
                case.push,
                case.constraints.clone(),
                case.ceiling,
            )
            .unwrap_or_else(|e| panic!("{}：解析策略就失败了 —— {e}", case.what));
            let guard = guard(&plan, case.startup)
                .unwrap_or_else(|e| panic!("{}：算 guard 失败了 —— {e}", case.what));
            let rows = render(&guard);
            assert_patch(&rows, case.sandbox, case.level, case.shell, case.web);
            // 档名只有一个出处：`DSH_PERMISSION_MODE` 与 patch 里那一行必须逐字相同。
            let env = runtime_env(Path::new("home"), guard.sandbox);
            assert_eq!(
                env.iter().find(|(k, _)| k == "DSH_PERMISSION_MODE").unwrap().1,
                case.sandbox,
                "{}：环境变量与 patch 的档名不一致",
                case.what
            );
        }
    }

    /// 收窄的写范围 DSH 表达不了 —— 必须**拒绝启动**，不能静默放宽成整个 workspace。
    #[test]
    fn narrowed_write_paths_refuse_to_start() {
        let plan = resolve(
            "workspace-development",
            "echo",
            None,
            &["."],
            &[".", "crates"],
            true,
            false,
            false,
            json!({ "write_paths": ["crates"] }),
            None,
        )
        .unwrap();
        // 配置允许写整个 workspace，任务书收窄到 crates/：策略层面合法（交集之内）。
        assert_eq!(plan.policy.write_paths, vec![PathBuf::from("crates")]);

        let err = guard(&plan, Permission::Workspace).unwrap_err();
        assert!(err.is(code::AUTH_GUARD_UNSUPPORTED), "{err}");
        assert!(err.message.contains("crates"), "报错要说清是哪一条路径：{err}");
    }

    /// 读范围同理：DSH 的读取不受沙箱限制，任何「只许读某处」都是空头承诺。
    #[test]
    fn narrowed_read_paths_refuse_to_start() {
        let plan = resolve(
            "workspace-development",
            "echo",
            None,
            &["."],
            &["."],
            true,
            false,
            false,
            json!({ "read_paths": ["crates"] }),
            None,
        )
        .unwrap();
        let err = guard(&plan, Permission::Workspace).unwrap_err();
        assert!(err.is(code::AUTH_GUARD_UNSUPPORTED), "{err}");
        assert!(err.message.contains("crates"), "{err}");
    }

    /// offline-test 档连读都不允许（`read_paths` 为空）—— DSH 没有「什么都不许读」的表达。
    #[test]
    fn offline_profile_has_no_dsh_expression() {
        let plan = resolve(
            "offline-test", "echo", None, &[], &[], false, false, false, Value::Null, None,
        )
        .unwrap();
        assert!(plan.policy.read_paths.is_empty());
        let err = guard(&plan, Permission::Readonly).unwrap_err();
        assert!(err.is(code::AUTH_GUARD_UNSUPPORTED), "{err}");
    }

    /// 不许联网但 shell 还开着：命令自己发请求拦不住 —— 拒绝启动，而不是假装摘掉两个工具就够了。
    #[test]
    fn network_off_with_shell_open_refuses_to_start() {
        let plan = resolve(
            "workspace-development", "echo", None, &["."], &["."], false, false, false, Value::Null, None,
        )
        .unwrap();
        assert!(plan.policy.shell);
        let err = guard(&plan, Permission::Workspace).unwrap_err();
        assert!(err.is(code::AUTH_GUARD_UNSUPPORTED), "{err}");
        assert!(err.message.contains("shell"), "{err}");
    }

    /// commit / push 是**已知缺口**：DSH 没有按 git 子命令区分权限的能力。
    /// 处理方式是留档 + 在结果里显式报出来，所以这里断言「报了」，不是「拦住了」。
    #[test]
    fn commit_and_push_limits_are_recorded_not_enforced() {
        let plan = resolve(
            "workspace-development", "echo", None, &["."], &["."], true, false, false, Value::Null, None,
        )
        .unwrap();
        let g = guard(&plan, Permission::Workspace).unwrap();
        assert_eq!(g.recorded.len(), 2, "{:#?}", g.recorded);
        assert!(g.recorded.iter().any(|n| n.contains("提交")), "{:#?}", g.recorded);
        assert!(g.recorded.iter().any(|n| n.contains("推送")), "{:#?}", g.recorded);

        // shell 关掉时这两条无从发生，也就不该再报 —— 报噪声会让人不再读它。
        let readonly = resolve(
            "workspace-readonly", "echo", None, &["."], &[], true, false, false, Value::Null, None,
        )
        .unwrap();
        let quiet = guard(&readonly, Permission::Workspace).unwrap();
        assert!(quiet.recorded.is_empty(), "{:#?}", quiet.recorded);
    }

    /// 「只能收紧」的负例：任务书想放宽的地方，authz 在策略那一层就拒了 ——
    /// guard 永远看不到一个「比启动配置更宽」的策略。
    #[test]
    fn task_cannot_widen_what_the_startup_level_allows() {
        // 任务书请求更高的策略档。
        let err = resolve(
            "workspace-readonly",
            "echo",
            None,
            &["."],
            &[],
            true,
            false,
            false,
            json!({ "shell_policy": "workspace-development" }),
            None,
        )
        .unwrap_err();
        assert!(err.is(code::AUTH_EXCEEDED), "{err}");

        // 任务书请求写范围之外的位置（配置的上限只有 `crates/`）。
        let err = resolve(
            "workspace-development",
            "echo",
            None,
            &["."],
            &["crates"],
            true,
            false,
            false,
            json!({ "write_paths": ["crates", "uwp"] }),
            None,
        )
        .unwrap_err();
        assert!(err.is(code::AUTH_EXCEEDED), "{err}");

        // 任务书请求提交，而启动配置没授权。
        let err = resolve(
            "workspace-development",
            "echo",
            None,
            &["."],
            &["."],
            true,
            false,
            false,
            json!({ "allow_commit": true }),
            None,
        )
        .unwrap_err();
        assert!(err.is(code::AUTH_EXCEEDED), "{err}");

        // Plan 模式请求写入。
        let err = resolve(
            "workspace-development",
            "echo",
            Some("plan"),
            &["."],
            &["."],
            true,
            false,
            false,
            json!({ "write_paths": ["."] }),
            None,
        )
        .unwrap_err();
        assert!(err.is(code::MODE_CONFLICT), "{err}");
    }

    #[test]
    fn missing_runtime_is_reported_with_the_fix() {
        let dir = tempfile::tempdir().unwrap();
        let layout = RuntimeLayout::new(dir.path(), "0.2.1-alpha.2");
        let err = layout.check().unwrap_err();
        assert!(err.contains("arcdeck runtime install"), "{err}");
    }

    /// 反解与正向映射必须互为逆（除 `confirm`：它与 `workspace` 同档，档名里不含这条信息）。
    #[test]
    fn sandbox_names_round_trip() {
        for level in [Permission::Readonly, Permission::Workspace, Permission::Full] {
            assert_eq!(Permission::from_sandbox(level.sandbox()).unwrap(), level);
        }
        assert_eq!(Permission::from_sandbox("workspace-write").unwrap(), Permission::Workspace);
        assert!(Permission::from_sandbox("yolo").is_err());
    }

    /// 交集只往严的那边走：`full` 只会被拉低，不会被抬高。
    #[test]
    fn stricter_only_tightens() {
        assert_eq!(Permission::Full.stricter(Permission::Readonly), Permission::Readonly);
        assert_eq!(Permission::Readonly.stricter(Permission::Full), Permission::Readonly);
        assert_eq!(Permission::Workspace.stricter(Permission::Full), Permission::Workspace);
        assert_eq!(Permission::Full.stricter(Permission::Workspace), Permission::Workspace);
    }
}

