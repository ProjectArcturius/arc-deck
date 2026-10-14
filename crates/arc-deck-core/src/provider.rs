//! 供应商管理（PRODUCT_PLAN §9.2b）。
//!
//! 形状照抄 OpenCode 的 `opencode.json` 的 `provider` 块，让用户已有的供应商配置可以
//! 原样搬过来：`provider.<id> = { name, npm | api, options: { baseURL, apiKey, headers }, models }`。
//! 模型写作 `<provider>/<model>`，按**第一个** `/` 切分（与 OpenCode `util/model.ts` 一致，
//! modelID 里带 `/` 也能用）。
//!
//! 执行仍归 DSH：这里只把供应商翻译成 DSH 的路由 —— DeepSeek 官方走 DSH 原生 Messages
//! 适配（`deepseek-official`），其余走 DSH 的 pi-ai 路由。`npm` 字段只用来推断协议，
//! **不加载任何 npm 包**。
//!
//! 凭据与配置分开（同 OpenCode 的 `auth.json`），密钥只以环境变量形式交给运行时子进程，
//! 生成的 patch 里只写变量名。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{code, SpecError, SpecResult};

/// DSH 原生 DeepSeek 适配注册的路由名。
pub const DEEPSEEK_OFFICIAL_ROUTE: &str = "deepseek-official";

/// 一个供应商的配置（OpenCode 形状）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// OpenCode 的 AI SDK 包名，只用来推断协议。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm: Option<String>,
    /// 显式协议（pi-ai 的 `api`），优先于 `npm` 推断。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default)]
    pub options: ProviderOptions,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, ModelConfig>,
}

/// 供应商选项。未知字段（OpenCode 的 `setCacheKey` 等）保留但不使用。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderOptions {
    #[serde(default, rename = "baseURL", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// 字面值、`{env:VAR}` 或 `{file:路径}`。省略时查 `auth.json`。
    #[serde(default, rename = "apiKey", skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, rename = "contextWindow", skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// 翻译后交给 DSH 的路由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshAdapter {
    /// DSH 原生 Messages 适配。
    DeepSeekOfficial { base_url: Option<String> },
    /// DSH 的 pi-ai 路由。
    PiAi {
        api: String,
        base_url: Option<String>,
        headers: BTreeMap<String, String>,
        models: Vec<(String, ModelConfig)>,
        display_name: Option<String>,
    },
}

/// 一次运行要用的完整路由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    /// 用户写的供应商 ID（`opencode-go`）。
    pub provider_id: String,
    /// `initialize` 里交给 DSH 的路由名（官方 DS 是 `deepseek-official`）。
    pub dsh_provider: String,
    pub model: String,
    pub adapter: DshAdapter,
    /// 运行时子进程里承载密钥的环境变量名。
    pub key_env: String,
    /// 密钥从哪来；**不是**密钥本身。
    pub key_source: KeySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    Literal,
    Env(String),
    File(PathBuf),
    AuthStore,
}

impl KeySource {
    pub fn describe(&self) -> String {
        match self {
            KeySource::Literal => "配置里的字面值".into(),
            KeySource::Env(v) => format!("环境变量 {v}"),
            KeySource::File(p) => format!("文件 {}", p.display()),
            KeySource::AuthStore => "arcdeck auth.json".into(),
        }
    }
}

/// 内置供应商：用户不写配置也能用。用户写了同名的块，以用户的为准（整块替换）。
pub fn builtin(id: &str) -> Option<ProviderConfig> {
    match id {
        "deepseek" => Some(ProviderConfig {
            name: Some("DeepSeek（官方）".into()),
            api: Some("deepseek".into()),
            ..Default::default()
        }),
        // OpenCode Go 订阅网关。实测：它的 `/v1/messages` 是 Anthropic Messages 协议，
        // 与 DeepSeek 官方路由同形；缺 `x-opencode-session` 头时返回 400 MissingSessionID。
        "opencode-go" => Some(ProviderConfig {
            name: Some("OpenCode Go".into()),
            api: Some("anthropic-messages".into()),
            options: ProviderOptions {
                base_url: Some("https://opencode.ai/zen/go".into()),
                headers: BTreeMap::from([(
                    "x-opencode-session".to_string(),
                    "{task_id}".to_string(),
                )]),
                ..Default::default()
            },
            ..Default::default()
        }),
        _ => None,
    }
}

/// 从 OpenCode 的 `npm` 包名推断 pi-ai 协议。
fn api_from_npm(npm: &str) -> Option<&'static str> {
    match npm {
        "@ai-sdk/openai-compatible" => Some("openai-completions"),
        "@ai-sdk/anthropic" => Some("anthropic-messages"),
        "@ai-sdk/openai" => Some("openai-responses"),
        "@ai-sdk/google" => Some("google-generative-ai"),
        "@ai-sdk/deepseek" => Some("deepseek"),
        _ => None,
    }
}

/// 把 `provider/model` 切开。
pub fn split_model(raw: &str) -> SpecResult<(String, String)> {
    match raw.split_once('/') {
        Some((p, m)) if !p.trim().is_empty() && !m.trim().is_empty() => {
            Ok((p.trim().to_string(), m.trim().to_string()))
        }
        _ => Err(SpecError::new(
            code::CONFIG_INVALID,
            format!("模型 `{raw}` 不是 `<provider>/<model>` 形式"),
        )),
    }
}

/// 环境变量名：`ARCDECK_KEY_` + 大写、非字母数字换成 `_`。
pub fn key_env_name(provider_id: &str) -> String {
    let tail: String = provider_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect();
    format!("ARCDECK_KEY_{tail}")
}

/// 解析路由。`providers` 是用户配置里的块；`task_id` 用来展开头部模板。
pub fn resolve(
    model: &str,
    providers: &BTreeMap<String, ProviderConfig>,
    task_id: &str,
) -> SpecResult<ResolvedRoute> {
    let (provider_id, model_id) = split_model(model)?;
    let config = providers
        .get(&provider_id)
        .cloned()
        .or_else(|| builtin(&provider_id))
        .ok_or_else(|| {
            SpecError::new(
                code::CONFIG_INVALID,
                format!(
                    "未知供应商 `{provider_id}`：配置的 provider 块里没有，也不是内置的（deepseek、opencode-go）"
                ),
            )
        })?;
    let api = config
        .api
        .clone()
        .or_else(|| config.npm.as_deref().and_then(api_from_npm).map(str::to_string))
        .ok_or_else(|| {
            SpecError::new(
                code::CONFIG_INVALID,
                format!(
                    "供应商 `{provider_id}` 没有 `api`，`npm`（{}）也推断不出协议",
                    config.npm.as_deref().unwrap_or("未写")
                ),
            )
        })?;
    let key_source = match config.options.api_key.as_deref() {
        None => KeySource::AuthStore,
        Some(raw) => {
            if let Some(var) = raw.strip_prefix("{env:").and_then(|r| r.strip_suffix('}')) {
                KeySource::Env(var.to_string())
            } else if let Some(path) = raw.strip_prefix("{file:").and_then(|r| r.strip_suffix('}')) {
                KeySource::File(PathBuf::from(path))
            } else {
                KeySource::Literal
            }
        }
    };
    let headers: BTreeMap<String, String> = config
        .options
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), v.replace("{task_id}", task_id)))
        .collect();
    let (dsh_provider, adapter) = if api == "deepseek" {
        (
            DEEPSEEK_OFFICIAL_ROUTE.to_string(),
            DshAdapter::DeepSeekOfficial { base_url: config.options.base_url.clone() },
        )
    } else {
        let mut models: Vec<(String, ModelConfig)> =
            config.models.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        // pi-ai 的 `models` 是整表替换；所选模型不在表里时补上，否则请求会 UNKNOWN_MODEL。
        if !models.iter().any(|(id, _)| id == &model_id) {
            models.push((model_id.clone(), ModelConfig::default()));
        }
        (
            provider_id.clone(),
            DshAdapter::PiAi {
                api,
                base_url: config.options.base_url.clone(),
                headers,
                models,
                display_name: config.name.clone(),
            },
        )
    };
    Ok(ResolvedRoute {
        key_env: key_env_name(&provider_id),
        provider_id,
        dsh_provider,
        model: model_id,
        adapter,
        key_source,
    })
}

impl ResolvedRoute {
    /// 物化成启动配置里的路由。字面值密钥不允许：物化文件会落在任务状态目录，
    /// 密钥不能因此多出一份拷贝 —— 让用户把它放进 auth.json。
    pub fn to_spec(&self) -> SpecResult<crate::config::RouteSpec> {
        let key_source = match &self.key_source {
            KeySource::AuthStore => "auth".to_string(),
            KeySource::Env(var) => format!("env:{var}"),
            KeySource::File(path) => format!("file:{}", path.display()),
            KeySource::Literal => {
                return Err(SpecError::new(
                    code::CONFIG_INVALID,
                    format!(
                        "供应商 `{}` 的 apiKey 是字面值；请改用 `arcdeck auth set {}`（或 {{env:VAR}}），配置里不放密钥",
                        self.provider_id, self.provider_id
                    ),
                ))
            }
        };
        let (adapter, api, base_url, headers, models, display_name) = match &self.adapter {
            DshAdapter::DeepSeekOfficial { base_url } => (
                DEEPSEEK_OFFICIAL_ROUTE.to_string(),
                None,
                base_url.clone(),
                BTreeMap::new(),
                Vec::new(),
                None,
            ),
            DshAdapter::PiAi { api, base_url, headers, models, display_name } => (
                "pi-ai".to_string(),
                Some(api.clone()),
                base_url.clone(),
                headers.clone(),
                models
                    .iter()
                    .map(|(id, m)| crate::config::RouteModel {
                        id: id.clone(),
                        name: m.name.clone(),
                        context_window: m.context_window,
                    })
                    .collect(),
                display_name.clone(),
            ),
        };
        Ok(crate::config::RouteSpec {
            provider_id: self.provider_id.clone(),
            adapter,
            api,
            base_url,
            headers,
            models,
            display_name,
            key_env: self.key_env.clone(),
            key_source,
        })
    }
}

/// 按物化后的 `key_source`（`auth` / `env:X` / `file:P`）取密钥。运行时启动前调用。
pub fn read_key_from_spec(spec: &crate::config::RouteSpec, auth_path: &Path) -> SpecResult<String> {
    let source = if spec.key_source == "auth" {
        KeySource::AuthStore
    } else if let Some(var) = spec.key_source.strip_prefix("env:") {
        KeySource::Env(var.to_string())
    } else if let Some(path) = spec.key_source.strip_prefix("file:") {
        KeySource::File(PathBuf::from(path))
    } else {
        return Err(SpecError::new(
            code::CONFIG_INVALID,
            format!("未知的 key_source `{}`", spec.key_source),
        ));
    };
    let route = ResolvedRoute {
        provider_id: spec.provider_id.clone(),
        dsh_provider: String::new(),
        model: String::new(),
        adapter: DshAdapter::DeepSeekOfficial { base_url: None },
        key_env: spec.key_env.clone(),
        key_source: source,
    };
    read_key(&route, None, auth_path)
}

/// 取出密钥。`literal` 是配置里的原值（只有 `KeySource::Literal` 用得上）。
pub fn read_key(
    route: &ResolvedRoute,
    literal: Option<&str>,
    auth_path: &Path,
) -> SpecResult<String> {
    let missing = |detail: String| {
        SpecError::new(
            code::CONFIG_INVALID,
            format!("供应商 `{}` 没有可用密钥：{detail}", route.provider_id),
        )
    };
    let key = match &route.key_source {
        KeySource::Literal => literal.unwrap_or_default().to_string(),
        KeySource::Env(var) => std::env::var(var).map_err(|_| missing(format!("环境变量 {var} 未设置")))?,
        KeySource::File(path) => std::fs::read_to_string(path)
            .map_err(|e| missing(format!("读 {} 失败：{e}", path.display())))?
            .trim()
            .to_string(),
        KeySource::AuthStore => {
            let store = AuthStore::load(auth_path)?;
            store.key(&route.provider_id).ok_or_else(|| {
                missing(format!(
                    "{} 里没有这一项；用 `arcdeck auth set {}` 写入",
                    auth_path.display(),
                    route.provider_id
                ))
            })?
        }
    };
    if key.trim().is_empty() {
        return Err(missing("值为空".into()));
    }
    Ok(key)
}

/// 凭据存储，与 OpenCode 的 `auth.json` 同形：`{ "<id>": { "type": "api", "key": "…" } }`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthStore {
    #[serde(flatten)]
    pub entries: BTreeMap<String, AuthEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthEntry {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl AuthStore {
    pub fn load(path: &Path) -> SpecResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path).map_err(|e| {
            SpecError::new(code::CONFIG_UNREADABLE, format!("{} —— {e}", path.display()))
        })?;
        serde_json::from_str(&raw).map_err(|e| {
            SpecError::new(code::CONFIG_INVALID, format!("{} 不是合法的 auth.json —— {e}", path.display()))
        })
    }

    pub fn key(&self, id: &str) -> Option<String> {
        self.entries
            .get(id)
            .filter(|e| e.kind == "api")
            .and_then(|e| e.key.clone())
    }

    pub fn set_api_key(&mut self, id: &str, key: &str) {
        self.entries.insert(
            id.to_string(),
            AuthEntry { kind: "api".into(), key: Some(key.to_string()), extra: BTreeMap::new() },
        );
    }

    /// 完整写入后替换，避免写到一半留下坏文件。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_splits_on_first_slash_only() {
        assert_eq!(split_model("a/b/c").unwrap(), ("a".into(), "b/c".into()));
        assert!(split_model("nomodel").is_err());
        assert!(split_model("/x").is_err());
    }

    #[test]
    fn builtin_opencode_go_is_anthropic_messages_with_session_header() {
        let route = resolve("opencode-go/deepseek-v4.1-flash", &BTreeMap::new(), "t1").unwrap();
        assert_eq!(route.dsh_provider, "opencode-go");
        assert_eq!(route.key_env, "ARCDECK_KEY_OPENCODE_GO");
        assert_eq!(route.key_source, KeySource::AuthStore);
        match route.adapter {
            DshAdapter::PiAi { api, headers, models, .. } => {
                assert_eq!(api, "anthropic-messages");
                assert_eq!(headers["x-opencode-session"], "t1");
                assert_eq!(models[0].0, "deepseek-v4.1-flash");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn deepseek_maps_to_the_native_route() {
        let route = resolve("deepseek/deepseek-flash", &BTreeMap::new(), "t").unwrap();
        assert_eq!(route.dsh_provider, DEEPSEEK_OFFICIAL_ROUTE);
        assert!(matches!(route.adapter, DshAdapter::DeepSeekOfficial { .. }));
    }

    #[test]
    fn opencode_json_provider_block_is_accepted_verbatim() {
        let raw = r#"{
            "intern": {
                "npm": "@ai-sdk/openai-compatible",
                "options": { "apiKey": "{env:INTERN_KEY}", "baseURL": "https://x/v1", "setCacheKey": true },
                "models": { "m1": { "name": "M1" } }
            }
        }"#;
        let providers: BTreeMap<String, ProviderConfig> = serde_json::from_str(raw).unwrap();
        let route = resolve("intern/m1", &providers, "t").unwrap();
        assert_eq!(route.key_source, KeySource::Env("INTERN_KEY".into()));
        match route.adapter {
            DshAdapter::PiAi { api, base_url, models, .. } => {
                assert_eq!(api, "openai-completions");
                assert_eq!(base_url.as_deref(), Some("https://x/v1"));
                assert_eq!(models.len(), 1);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_protocol_is_an_error_not_a_guess() {
        let raw = r#"{ "x": { "npm": "@ai-sdk/something-else" } }"#;
        let providers: BTreeMap<String, ProviderConfig> = serde_json::from_str(raw).unwrap();
        assert!(resolve("x/m", &providers, "t").is_err());
    }

    #[test]
    fn auth_store_round_trips_opencode_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, r#"{"a":{"type":"api","key":"k1"},"b":{"type":"oauth","refresh":"r"}}"#).unwrap();
        let mut store = AuthStore::load(&path).unwrap();
        assert_eq!(store.key("a").as_deref(), Some("k1"));
        assert_eq!(store.key("b"), None);
        store.set_api_key("c", "k3");
        store.save(&path).unwrap();
        let again = AuthStore::load(&path).unwrap();
        assert_eq!(again.key("c").as_deref(), Some("k3"));
        assert!(again.entries.contains_key("b"), "非 api 项必须原样保留");
    }
}
