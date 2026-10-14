//! Agent 定义加载与提示词装配（PRODUCT_PLAN §5.1–§5.3）。
//!
//! Agent 定义是**可解析、可校验的文件**，不是散在代码里的字符串。这样
//! 「Echo 到底被赋予了哪些工具」是一个 grep 就能回答的问题，而不是读一遍
//! 几百行的拼装逻辑。
//!
//! 格式刻意做得窄（行式 `key: value` 头 + Markdown 正文），因为它的作用是
//! 版本化角色定义，不是又造一个配置语言。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::authz::Profile;
use crate::error::{code, SpecError, SpecResult};
use crate::mode::{AgentId, Mode};
use crate::SCHEMA_VERSION;

/// 首版允许出现在 Agent 定义里的逻辑工具名（PRODUCT_PLAN §5.4）。
///
/// 收窄成一个常量是有意的：写错工具名是**定义错误**，必须在加载时炸，
/// 而不是等到模型调用一个不存在的工具。
pub const KNOWN_TOOLS: &[&str] = &[
    "read", "glob", "grep", "edit", "shell", "todo", "question", "subagent",
];

/// Agent 在系统里的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    Primary,
    Subagent,
}

impl AgentRole {
    fn parse(raw: &str) -> SpecResult<Self> {
        match raw {
            "primary" => Ok(AgentRole::Primary),
            "subagent" => Ok(AgentRole::Subagent),
            other => Err(SpecError::new(
                code::AGENT_PARSE,
                format!("role 只能是 primary 或 subagent，得到 `{other}`"),
            )),
        }
    }
}

/// 一份加载好的 Agent 定义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub schema_version: u32,
    pub id: AgentId,
    pub description: String,
    pub role: AgentRole,
    pub default_mode: Mode,
    pub tools: Vec<String>,
    pub permission_profile: Profile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 风格提示词的文件名（不含 `.md`），位于 `prompts_dir`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// 角色与工作纪律正文。
    pub body: String,
    /// 定义来自哪个文件（诊断用）。
    #[serde(skip)]
    pub source_path: PathBuf,
}

impl AgentDefinition {
    /// 从 Markdown 文本解析。`origin` 仅用于错误信息。
    pub fn parse(raw: &str, origin: &Path) -> SpecResult<Self> {
        let (head, body) = split_front_matter(raw).ok_or_else(|| {
            SpecError::new(
                code::AGENT_PARSE,
                format!("{} 缺少 `---` 包裹的定义头", origin.display()),
            )
        })?;

        let mut fields: BTreeMap<String, String> = BTreeMap::new();
        for (lineno, line) in head.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once(':').ok_or_else(|| {
                SpecError::new(
                    code::AGENT_PARSE,
                    format!("{} 定义头第 {} 行不是 `key: value`：{line:?}", origin.display(), lineno + 1),
                )
            })?;
            let key = key.trim().to_string();
            if fields.contains_key(&key) {
                return Err(SpecError::new(
                    code::AGENT_PARSE,
                    format!("{} 定义头字段重复：`{key}`", origin.display()),
                ));
            }
            fields.insert(key, value.trim().to_string());
        }

        let get = |key: &str| -> Option<&str> { fields.get(key).map(|s| s.as_str()) };
        let require = |key: &str| -> SpecResult<&str> {
            get(key).ok_or_else(|| {
                SpecError::new(
                    code::AGENT_PARSE,
                    format!("{} 定义头缺少必需字段 `{key}`", origin.display()),
                )
            })
        };

        let schema_version: u32 = require("schema_version")
            .and_then(|v| {
                v.parse().map_err(|_| {
                    SpecError::new(code::AGENT_PARSE, format!("schema_version 不是整数：{v:?}"))
                })
            })?;
        if schema_version != SCHEMA_VERSION {
            return Err(SpecError::new(
                code::AGENT_UNSUPPORTED,
                format!(
                    "{} 的 schema_version={schema_version}，本客户端只支持 {SCHEMA_VERSION}",
                    origin.display()
                ),
            ));
        }

        for key in fields.keys() {
            if !matches!(
                key.as_str(),
                "schema_version"
                    | "id"
                    | "description"
                    | "role"
                    | "default_mode"
                    | "tools"
                    | "permission_profile"
                    | "model"
                    | "style"
            ) {
                return Err(SpecError::new(
                    code::AGENT_UNSUPPORTED,
                    format!("{} 含未知定义头字段 `{key}`", origin.display()),
                ));
            }
        }

        let id = AgentId::parse(require("id")?)?;
        let role = AgentRole::parse(require("role")?)?;
        let default_mode = match require("default_mode")? {
            "build" => Mode::Build,
            "plan" => Mode::Plan,
            other => {
                return Err(SpecError::new(
                    code::AGENT_PARSE,
                    format!("default_mode 只能是 build 或 plan，得到 `{other}`"),
                ))
            }
        };
        // 默认模式必须落在该 Agent 的合法组合里，否则「不指定模式」会造出非法对。
        crate::mode::AgentMode::resolve(Some(id), Some(default_mode))?;

        let tools = parse_tools(require("tools")?)?;
        let permission_profile = Profile::parse(require("permission_profile")?)?;
        let style = get("style").map(|s| s.to_string());
        if let Some(style) = &style {
            if style.contains(['/', '\\', '.']) {
                return Err(SpecError::new(
                    code::AGENT_PARSE,
                    format!("style 只写文件名（不含扩展名与路径），得到 {style:?}"),
                ));
            }
        }

        let body = body.trim().to_string();
        if body.is_empty() {
            return Err(SpecError::new(
                code::AGENT_PARSE,
                format!("{} 的正文为空 —— Agent 定义不能只有头", origin.display()),
            ));
        }

        Ok(Self {
            schema_version,
            id,
            description: require("description")?.to_string(),
            role,
            default_mode,
            tools,
            permission_profile,
            model: get("model").map(|s| s.to_string()),
            style,
            body,
            source_path: origin.to_path_buf(),
        })
    }
}

fn parse_tools(raw: &str) -> SpecResult<Vec<String>> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for item in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !KNOWN_TOOLS.contains(&item) {
            return Err(SpecError::new(
                code::AGENT_UNKNOWN_TOOL,
                format!("未知工具 `{item}`；已知：{}", KNOWN_TOOLS.join(", ")),
            ));
        }
        if !seen.insert(item.to_string()) {
            return Err(SpecError::new(code::AGENT_PARSE, format!("工具重复：`{item}`")));
        }
        out.push(item.to_string());
    }
    if out.is_empty() {
        return Err(SpecError::new(code::AGENT_PARSE, "tools 不得为空"));
    }
    Ok(out)
}

fn split_front_matter(raw: &str) -> Option<(&str, &str)> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let rest = raw.strip_prefix("---")?;
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n'))?;
    let end = rest.find("\n---")?;
    let head = &rest[..end];
    let body = &rest[end + 4..];
    Some((head, body))
}

/// 装什么进 system prompt。**这三档是行为契约，不是风格偏好。**
///
/// 用户 2026-10-14：「人格特性生效于非 workers 场景，并且是用户开启个性化设置的情况下
/// 启用，否则不记录、不调用、不修改，直接按照模型供应商本体特性行事。」
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptScope {
    /// 工人：功能契约 —— 工程纪律 + 角色 + 模式行为。**不带风格。**
    ///
    /// 工人交出去的是结果与证据，风格不影响它们，而每一段提示词都要花掉上下文。
    Worker,
    /// 一般模式 + 个性化开：在功能契约之上加风格，成为「你的」agent。
    Personalized,
    /// 一般模式 + 个性化关（缺省）：**一段都不加**，按供应商本体的行为行事。
    Bare,
}

impl Default for PromptScope {
    /// 缺省是 [`PromptScope::Worker`]：给「没有这个字段」的旧启动配置用，
    /// 而那些配置都是工人模式下物化的。
    fn default() -> Self {
        Self::Worker
    }
}

/// 提示词的一段。保留来源，才能回答「Echo 风格到底有没有进 system prompt」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSection {
    pub name: String,
    pub source: String,
    pub text: String,
}

/// 装配好的系统提示词。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPrompt {
    pub agent: AgentId,
    pub mode: Mode,
    pub sections: Vec<PromptSection>,
}

impl SystemPrompt {
    pub fn text(&self) -> String {
        let mut out = String::new();
        for section in &self.sections {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&section.text);
        }
        out
    }

    pub fn has_section(&self, name: &str) -> bool {
        self.sections.iter().any(|s| s.name == name)
    }

    pub fn section(&self, name: &str) -> Option<&PromptSection> {
        self.sections.iter().find(|s| s.name == name)
    }
}

/// 加载并持有一组 Agent 定义。
#[derive(Debug, Clone)]
pub struct AgentRegistry {
    definitions: BTreeMap<AgentId, AgentDefinition>,
    prompts_dir: PathBuf,
}

impl AgentRegistry {
    /// 从目录加载全部 `*.md`。任何一份定义不合法都会让整次加载失败。
    ///
    /// 「坏一份就整体失败」是有意的：如果跳过坏文件，一个写错的 Echo 定义会
    /// 表现为「默认 Agent 突然换成了别的角色」，而日志里只有一行警告。
    pub fn load(dir: &Path, prompts_dir: &Path) -> SpecResult<Self> {
        if !dir.is_dir() {
            return Err(SpecError::new(
                code::AGENT_DIR_MISSING,
                format!("Agent 定义目录不存在：{}", dir.display()),
            ));
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| SpecError::new(code::AGENT_DIR_MISSING, format!("{} —— {e}", dir.display())))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
            .collect();
        entries.sort();

        if entries.is_empty() {
            return Err(SpecError::new(
                code::AGENT_DIR_MISSING,
                format!("Agent 定义目录里没有任何 .md：{}", dir.display()),
            ));
        }

        let mut definitions = BTreeMap::new();
        for path in entries {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| SpecError::new(code::AGENT_PARSE, format!("{} —— {e}", path.display())))?;
            let def = AgentDefinition::parse(&raw, &path)?;
            if definitions.insert(def.id, def).is_some() {
                return Err(SpecError::new(
                    code::AGENT_DUPLICATE_ID,
                    format!("Agent id 重复，目录里有两份同 id 的定义：{}", path.display()),
                ));
            }
        }

        let registry = Self { definitions, prompts_dir: prompts_dir.to_path_buf() };
        // 风格文件在加载时就检查：等到装配提示词时才发现缺文件，那时任务已经开始了。
        for def in registry.definitions.values() {
            if let Some(style) = &def.style {
                let path = registry.style_path(style);
                if !path.is_file() {
                    return Err(SpecError::new(
                        code::AGENT_REF_MISSING,
                        format!(
                            "Agent `{}` 引用的风格文件不存在：{}",
                            def.id,
                            path.display()
                        ),
                    ));
                }
            }
        }
        Ok(registry)
    }

    pub fn style_path(&self, style: &str) -> PathBuf {
        self.prompts_dir.join(format!("{style}.md"))
    }

    pub fn definition(&self, id: AgentId) -> SpecResult<&AgentDefinition> {
        self.definitions.get(&id).ok_or_else(|| {
            SpecError::new(code::AGENT_UNKNOWN, format!("Agent 定义 `{id}` 未加载"))
        })
    }

    pub fn ids(&self) -> Vec<AgentId> {
        self.definitions.keys().copied().collect()
    }

    /// 装配系统提示词（PRODUCT_PLAN §5.3 的顺序）。
    ///
    /// 顺序是契约的一部分：工程规则先于角色行为，角色行为先于风格。反过来的
    /// 话，一段强调「简短回答」的风格提示会覆盖掉「先取证再结论」的纪律。
    pub fn compose_system_prompt(
        &self,
        scope: PromptScope,
        id: AgentId,
        mode: Mode,
        workspace_instructions: Option<&str>,
    ) -> SpecResult<SystemPrompt> {
        // 一般模式 + 个性化关：**一段都不加**（用户 2026-10-14）。返回空集而不是
        // 「加了但很短」——「不加」必须是可判定的：调用方据此写出空 personaSuffix。
        if scope == PromptScope::Bare {
            return Ok(SystemPrompt { agent: id, mode, sections: Vec::new() });
        }

        let def = self.definition(id)?;
        // 组合合法性在这里再确认一次：调用方可能拿配置里的模式直接进来。
        crate::mode::AgentMode::resolve(Some(id), Some(mode))?;

        let mut sections = Vec::new();
        sections.push(self.prompt_section("engineering", "arc-deck-engineering")?);
        sections.push(PromptSection {
            name: "agent".to_string(),
            source: def.source_path.display().to_string(),
            text: def.body.clone(),
        });
        sections.push(self.prompt_section("mode", mode_prompt_name(mode))?);
        // 风格只在个性化那一档进 —— 工人交出去的是结果与证据，风格不影响它们，
        // 而每一段提示词都要花掉上下文（用户 2026-10-14：「人格生效于非 workers 场景」）。
        if scope == PromptScope::Personalized {
            if let Some(style) = &def.style {
                sections.push(self.prompt_section("style", style)?);
            }
        }
        if let Some(extra) = workspace_instructions {
            let extra = extra.trim();
            if !extra.is_empty() {
                sections.push(PromptSection {
                    name: "workspace".to_string(),
                    source: "(workspace instructions)".to_string(),
                    text: extra.to_string(),
                });
            }
        }

        Ok(SystemPrompt { agent: id, mode, sections })
    }

    fn prompt_section(&self, name: &str, file_stem: &str) -> SpecResult<PromptSection> {
        let path = self.prompts_dir.join(format!("{file_stem}.md"));
        let text = std::fs::read_to_string(&path).map_err(|e| {
            SpecError::new(
                code::AGENT_REF_MISSING,
                format!("提示词片段缺失或不可读：{} —— {e}", path.display()),
            )
        })?;
        Ok(PromptSection { name: name.to_string(), source: path.display().to_string(), text: text.trim().to_string() })
    }
}

const fn mode_prompt_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Build => "build-behavior",
        Mode::Plan => "plan-behavior",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def_raw(id: &str, extra: &str) -> String {
        format!(
            "---\nschema_version: 1\nid: {id}\ndescription: d\nrole: primary\ndefault_mode: build\ntools: read, glob\npermission_profile: workspace-development\n{extra}---\n\n正文。\n"
        )
    }

    #[test]
    fn parses_a_definition() {
        let def = AgentDefinition::parse(&def_raw("echo", ""), Path::new("echo.md")).unwrap();
        assert_eq!(def.id, AgentId::Echo);
        assert_eq!(def.tools, vec!["read", "glob"]);
        assert_eq!(def.permission_profile, Profile::WorkspaceDevelopment);
        assert_eq!(def.body, "正文。");
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let raw = def_raw("echo", "").replace("tools: read, glob", "tools: read, telepathy");
        let err = AgentDefinition::parse(&raw, Path::new("echo.md")).unwrap_err();
        assert!(err.is(code::AGENT_UNKNOWN_TOOL), "{err}");
    }

    #[test]
    fn unknown_head_field_is_rejected() {
        let raw = def_raw("echo", "colour: yellow\n");
        let err = AgentDefinition::parse(&raw, Path::new("echo.md")).unwrap_err();
        assert!(err.is(code::AGENT_UNSUPPORTED), "{err}");
    }

    #[test]
    fn plan_agent_with_build_default_is_rejected() {
        // plan 的合法模式只有 plan；定义里写 default_mode: build 是冲突组合，
        // 加载时就该炸 —— 否则「没指定模式」会静默造出非法对。
        let raw = def_raw("plan", "");
        let err = AgentDefinition::parse(&raw, Path::new("plan.md")).unwrap_err();
        assert!(err.is(code::MODE_CONFLICT), "{err}");
    }

    #[test]
    fn missing_front_matter_is_rejected() {
        let err = AgentDefinition::parse("没有头的正文", Path::new("x.md")).unwrap_err();
        assert!(err.is(code::AGENT_PARSE), "{err}");
    }

    #[test]
    fn style_reference_must_not_contain_paths() {
        let raw = def_raw("echo", "style: ../secret\n");
        let err = AgentDefinition::parse(&raw, Path::new("echo.md")).unwrap_err();
        assert!(err.is(code::AGENT_PARSE), "{err}");
    }

    #[test]
    fn registry_loads_repo_definitions_and_composes_prompt() {
        let root = repo_root();
        let registry = AgentRegistry::load(&root.join("agents"), &root.join("prompts")).unwrap();
        assert!(registry.ids().contains(&AgentId::Echo));

        let prompt = registry
            .compose_system_prompt(PromptScope::Personalized, AgentId::Echo, Mode::Build, Some("工作区规则"))
            .unwrap();
        assert!(prompt.has_section("engineering"));
        assert!(prompt.has_section("agent"));
        assert!(prompt.has_section("style"));
        assert!(prompt.has_section("workspace"));
        assert!(prompt.section("mode").unwrap().source.ends_with("build-behavior.md"));

        // 风格真的进了正文，而不只是被列举出来。
        let style = std::fs::read_to_string(root.join("prompts/echo-style.md")).unwrap();
        let first_rule = style.lines().find(|l| l.trim_start().starts_with('-')).unwrap().trim();
        assert!(prompt.text().contains(first_rule), "风格首条规则没有进入提示词");
    }

    /// 三档的人格边界（用户 2026-10-14 的裁定）。
    #[test]
    fn prompt_scope_decides_whether_style_and_discipline_are_injected() {
        let root = repo_root();
        let registry = AgentRegistry::load(&root.join("agents"), &root.join("prompts")).unwrap();

        let worker = registry
            .compose_system_prompt(PromptScope::Worker, AgentId::Echo, Mode::Build, None)
            .unwrap();
        assert!(worker.has_section("engineering"));
        assert!(worker.has_section("agent"));
        assert!(worker.has_section("mode"));
        assert!(!worker.has_section("style"), "工人不带风格");

        let personalized = registry
            .compose_system_prompt(PromptScope::Personalized, AgentId::Echo, Mode::Build, None)
            .unwrap();
        assert!(personalized.has_section("style"), "个性化开了才有风格");

        // 关的时候是**一段都不加**，不是「加得少」。这条要能被程序判定 ——
        // 调用方据此写出空的 personaSuffix。
        let bare = registry
            .compose_system_prompt(PromptScope::Bare, AgentId::Echo, Mode::Build, Some("工作区规则"))
            .unwrap();
        assert!(bare.sections.is_empty(), "个性化关：{bare:?}");
        assert_eq!(bare.text(), "");
    }

    #[test]
    fn plan_composition_differs_from_build() {
        let root = repo_root();
        let registry = AgentRegistry::load(&root.join("agents"), &root.join("prompts")).unwrap();
        let build = registry
            .compose_system_prompt(PromptScope::Worker, AgentId::Echo, Mode::Build, None)
            .unwrap();
        let plan = registry
            .compose_system_prompt(PromptScope::Worker, AgentId::Echo, Mode::Plan, None)
            .unwrap();
        assert_ne!(build.text(), plan.text());
        assert!(plan.section("mode").unwrap().source.ends_with("plan-behavior.md"));
    }

    #[test]
    fn missing_style_file_is_a_load_error() {
        let root = repo_root();
        let raw = def_raw("echo", "style: does-not-exist\n");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("echo.md"), raw).unwrap();
        let err = AgentRegistry::load(dir.path(), &root.join("prompts")).unwrap_err();
        assert!(err.is(code::AGENT_REF_MISSING), "{err}");
    }

    fn repo_root() -> PathBuf {
        // CARGO_MANIFEST_DIR = <repo>/crates/arc-deck-core
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
    }
}
