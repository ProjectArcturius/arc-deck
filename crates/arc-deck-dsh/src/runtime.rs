//! DSH 运行时的安装、定位与每任务 patch（PRODUCT_PLAN §9.2a）。
//!
//! **不用 submodule、不 vendor 源码、不从源码构建。** DSH 在 npm 上按包发布，
//! `dsh` CLI 只是按 profile 组合 bundle 的启动器；所以这里在用户数据目录下建一个
//! 私有目录，把 `@deepseek-ai/dsh` 钉成精确版本装进去，用 `node <bin.js> --profile sdk` 启动。
//! 全局 npm 的 `dsh` 不碰 —— 它与 DSH Desktop 共用，版本也不受我们控制。

use std::path::{Path, PathBuf};
use std::process::Command;

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
}

/// 生成 `--patch` 文件内容。
///
/// 用 JSON 写：YAML 是 JSON 的超集，DSH 的 `--dump-config` 与启动都实测能读，
/// 生成端因此不需要 YAML 依赖。每一行都按 id 定位 base bundle 里的行；
/// 注意 patch 会**整块替换**目标行的 `config`，不做深合并（DSH CLI reference）。
pub fn render_patch(input: &PatchInput<'_>) -> String {
    let mut rows: Vec<Value> = Vec::new();

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

    let route = input.route;
    if route.adapter == arc_deck_core::provider::DEEPSEEK_OFFICIAL_ROUTE {
        let mut config = json!({ "apiKeyEnv": route.key_env });
        if let Some(base) = &route.base_url {
            config["baseURL"] = json!(base);
        }
        rows.push(json!({ "id": "llm-deepseek", "config": config }));
    } else {
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
        rows.push(json!({
            "id": "llm-pi-ai",
            "config": { "providers": { route.provider_id.clone(): provider } },
        }));
    }
    serde_json::to_string_pretty(&Value::Array(rows)).unwrap_or_default()
}

/// persona 模板会插值 `{{变量}}`，未定义的变量让整次组装失败（dsh-system-prompt README）。
/// 工人守则里出现字面 `{{` 时拆开，宁可多一个空格，也不能让每一步请求都组装失败。
fn escape_template(text: &str) -> String {
    text.replace("{{", "{ {").replace("}}", "} }")
}

/// 运行时进程要的固定环境。密钥另由调用方按 `route.key_env` 注入。
pub fn runtime_env(dsh_home: &Path, sandbox: &str) -> Vec<(String, String)> {
    vec![
        ("DSH_HOME".into(), dsh_home.display().to_string()),
        // OTel 只在用户主动反馈时上传；worker 没有反馈入口，直接关掉（base bundle 注释）。
        ("DSH_TELEMETRY_DISABLED".into(), "1".into()),
        // base bundle 的沙箱与审批默认值都读这个变量（`danger-full-access` 时审批为 never）。
        ("DSH_PERMISSION_MODE".into(), sandbox.into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_deck_core::config::RouteModel;
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

    #[test]
    fn pi_ai_patch_has_route_and_no_secret() {
        let route = pi_route();
        let text = render_patch(&PatchInput { route: &route, persona_suffix: "守则 {{x}}", session_log_upload: false });
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
        let text = render_patch(&PatchInput { route: &route, persona_suffix: "", session_log_upload: true });
        let v: Value = serde_json::from_str(&text).unwrap();
        let rows = v.as_array().unwrap();
        assert!(!rows.iter().any(|r| r["id"] == "session-log-deepseek"));
        let ds = rows.iter().find(|r| r["id"] == "llm-deepseek").unwrap();
        assert_eq!(ds["config"]["apiKeyEnv"], "ARCDECK_KEY_DEEPSEEK");
        assert!(!rows.iter().any(|r| r["id"] == "llm-pi-ai"));
    }

    #[test]
    fn missing_runtime_is_reported_with_the_fix() {
        let dir = tempfile::tempdir().unwrap();
        let layout = RuntimeLayout::new(dir.path(), "0.2.1-alpha.2");
        let err = layout.check().unwrap_err();
        assert!(err.contains("arcdeck runtime install"), "{err}");
    }
}
