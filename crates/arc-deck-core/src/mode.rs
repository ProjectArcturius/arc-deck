//! Agent 与运行模式。
//!
//! Agent 与模式是**两个维度**：Echo 是一种角色，build/plan 是它当前的工作模式。
//! 两者可以组合，但组合受一张白名单矩阵约束 —— 见 [`AgentMode::resolve`]。
//! 参 PRODUCT_PLAN §5.1。

use serde::{Deserialize, Serialize};

use crate::error::{code, SpecError, SpecResult};

/// Agent 标识。首版固定三个，不做插件化 Agent 市场。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentId {
    /// 默认 Agent。角色 + 工作纪律 + 用户的 Echo 风格。
    Echo,
    /// 偏实施的工程角色。
    Build,
    /// 只研究与规划、不实施。
    Plan,
}

impl AgentId {
    pub const fn as_str(self) -> &'static str {
        match self {
            AgentId::Echo => "echo",
            AgentId::Build => "build",
            AgentId::Plan => "plan",
        }
    }

    pub fn parse(raw: &str) -> SpecResult<Self> {
        match raw {
            "echo" => Ok(AgentId::Echo),
            "build" => Ok(AgentId::Build),
            "plan" => Ok(AgentId::Plan),
            other => Err(SpecError::new(
                code::AGENT_UNKNOWN,
                format!("未知 Agent `{other}`；首版只有 echo / build / plan"),
            )),
        }
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 工作模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 可以实施变更（仍受启动授权约束）。
    Build,
    /// 只研究、只规划；执行层拒绝写入与任意 shell。
    Plan,
}

impl Mode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Build => "build",
            Mode::Plan => "plan",
        }
    }

    /// 反解。交互面的重启请求是字符串（一条 UI 消息），到执行层之前必须过这里，
    /// 让「未知模式」变成一句可读的错，而不是悄悄当成 build。
    pub fn parse(raw: &str) -> SpecResult<Self> {
        match raw {
            "build" => Ok(Mode::Build),
            "plan" => Ok(Mode::Plan),
            other => Err(SpecError::new(
                code::CONFIG_INVALID,
                format!("未知模式 `{other}`；只有 build / plan"),
            )),
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一个通过校验的 Agent + 模式组合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMode {
    pub agent: AgentId,
    pub mode: Mode,
}

impl AgentMode {
    /// 默认解析：未指定 Agent 取 `echo`，未指定模式取该 Agent 的默认模式。
    ///
    /// 矩阵（PRODUCT_PLAN §5.1）：
    ///
    /// | agent | 允许的 mode |
    /// |---|---|
    /// | echo | build, plan |
    /// | build | build |
    /// | plan | plan |
    ///
    /// `build + plan` 与 `plan + build` 明确拒绝，而不是静默改写 —— 静默改写会让
    /// 「我明明选了 Plan」和「它怎么开始写文件了」这两件事同时发生。
    pub fn resolve(agent: Option<AgentId>, mode: Option<Mode>) -> SpecResult<Self> {
        let agent = agent.unwrap_or(AgentId::Echo);
        let mode = mode.unwrap_or(Self::default_mode(agent));
        let allowed = Self::allowed_modes(agent);
        if !allowed.contains(&mode) {
            return Err(SpecError::new(
                code::MODE_CONFLICT,
                format!(
                    "Agent `{agent}` 与模式 `{mode}` 是冲突组合；`{agent}` 允许的模式：{}",
                    allowed.iter().map(|m| m.as_str()).collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        Ok(Self { agent, mode })
    }

    pub const fn default_mode(agent: AgentId) -> Mode {
        match agent {
            // Echo 默认按实施角色工作；Plan 是显式选择。
            AgentId::Echo => Mode::Build,
            AgentId::Build => Mode::Build,
            AgentId::Plan => Mode::Plan,
        }
    }

    pub const fn allowed_modes(agent: AgentId) -> &'static [Mode] {
        match agent {
            AgentId::Echo => &[Mode::Build, Mode::Plan],
            AgentId::Build => &[Mode::Build],
            AgentId::Plan => &[Mode::Plan],
        }
    }

    /// 交互端的 `/plan`。只切模式，尽量保住当前 Agent 与风格。
    pub fn switch_to_plan(self) -> SpecResult<Self> {
        match self.agent {
            AgentId::Echo => Self::resolve(Some(AgentId::Echo), Some(Mode::Plan)),
            AgentId::Build => Self::resolve(Some(AgentId::Plan), Some(Mode::Plan)),
            AgentId::Plan => Ok(self),
        }
    }

    /// 交互端的 `/build`。
    pub fn switch_to_build(self) -> SpecResult<Self> {
        match self.agent {
            AgentId::Echo => Self::resolve(Some(AgentId::Echo), Some(Mode::Build)),
            AgentId::Plan => Self::resolve(Some(AgentId::Build), Some(Mode::Build)),
            AgentId::Build => Ok(self),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_echo_build() {
        let am = AgentMode::resolve(None, None).unwrap();
        assert_eq!(am.agent, AgentId::Echo);
        assert_eq!(am.mode, Mode::Build);
    }

    #[test]
    fn echo_plus_plan_is_valid() {
        let am = AgentMode::resolve(Some(AgentId::Echo), Some(Mode::Plan)).unwrap();
        assert_eq!(am.mode, Mode::Plan);
    }

    #[test]
    fn plan_plus_build_is_rejected() {
        let err = AgentMode::resolve(Some(AgentId::Plan), Some(Mode::Build)).unwrap_err();
        assert!(err.is(code::MODE_CONFLICT), "{err}");
    }

    #[test]
    fn build_plus_plan_is_rejected() {
        let err = AgentMode::resolve(Some(AgentId::Build), Some(Mode::Plan)).unwrap_err();
        assert!(err.is(code::MODE_CONFLICT), "{err}");
    }

    #[test]
    fn unknown_agent_is_rejected() {
        let err = AgentId::parse("explore").unwrap_err();
        assert!(err.is(code::AGENT_UNKNOWN), "{err}");
    }

    #[test]
    fn slash_switches_never_produce_illegal_pairs() {
        for agent in [AgentId::Echo, AgentId::Build, AgentId::Plan] {
            for mode in [Mode::Build, Mode::Plan] {
                if let Ok(start) = AgentMode::resolve(Some(agent), Some(mode)) {
                    // 两个方向都必须回到合法组合，否则 /plan 与 /build 会互相造出非法对。
                    assert!(start.switch_to_plan().is_ok(), "{start:?}");
                    assert!(start.switch_to_build().is_ok(), "{start:?}");
                }
            }
        }
    }
}
