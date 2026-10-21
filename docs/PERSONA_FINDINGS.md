# 人格与风格怎么装：Claude Code 的做法（源码调查）

调查对象：`REFS/claude-code` @ `9b4f0d6`（用户 2026-10-14 补全的检出）。
问题：**人格／风格应该怎么装配、怎么分发、怎么不漂？**
结论先说：Anthropic 走过一条弯路，最终落在「**加在默认 system prompt 之上、可分发、会话开始时固定**」。

## 1. 有四个注入口，语义各不相同

| 手段 | 语义 | 源码依据 |
|---|---|---|
| **SessionStart hook** 的 `additionalContext` | **加**在默认 system prompt 之上 | `plugins/explanatory-output-style/hooks-handlers/session-start.sh` |
| **subagent** | **换**掉 system prompt | 同上目录的 README |
| **指令文件**（`CLAUDE.md` / `AGENTS.md`） | 由引擎渲染，带自己的 preamble 与框架，**按名字公告** | `mods/agents-md/README.md` |
| CLI `--system-prompt` / `--append-system-prompt` / `--system-prompt-file` | 直接换／加 | `CHANGELOG` 第 8118 行 |

对两者区别的原文（`plugins/explanatory-output-style/README.md`）：

> Subagents change the system prompt while SessionStart hooks add to the default system prompt.

> Output styles that involve tasks besides software development are better expressed as
> subagents, not as SessionStart hooks.

**这条分界对我们直接有用**：工人的那层（工程纪律 + 角色 + 模式）是**换**，一般模式的风格是**加**。

## 2. 他们自己的演进史（`CHANGELOG.md`，按行号倒序）

| 行 | 事件 |
|---|---|
| 8427 | 发布 output styles（内置 Explanatory／Learning） |
| 8118 | **弃用** output styles，建议改用 `--system-prompt-file`／`CLAUDE.md`／plugins |
| 8097 | **因社区反馈撤销弃用** |
| 8059 | plugins 支持分享与安装 output styles |
| 8066 / 6215 | 加 `keep-coding-instructions` frontmatter 选项 |
| 6796 | `/output-style` 弃用并入 `/config`；**「Output style is now fixed at session start for better prompt caching」** |
| 3300 | 修「output styles **中途漂回默认嗓音**」 |
| 1367 | `claude plugin validate` 开始校验 `outputStyles` 等路径 |

读出来的三件事：

1. **内置设置 → CLI 标志 → 插件**，驱动力是**可分发**（能像插件一样分享安装）。他们先弃用、又被社区骂回来，最后用插件/hook 重新实现同一件事。
2. **风格必须在会话开始时固定**，理由是 **prompt caching**。中途可变＝缓存失效。
3. 他们踩过一个具体的坑：**人格在会话中途漂回默认嗓音**（3300 行）。这正是 PRODUCT_PLAN §5.3 要求「Agent 切换与压缩后仍维持模式/风格」的那条，也是我们该测的。

## 3. `keep-coding-instructions` 与我们的 `PromptScope` 是同一个东西

`keep-coding-instructions` 让一个风格选择**保留还是丢掉编码指令** —— 也就是把「风格」与
「工程纪律」分开。我们 2026-10-14 独立走到的那一步（`PromptScope` 三档：
工人拿功能契约不带风格 / 个性化开才加风格 / 关就一段不加）是**同一个切法**，
只是他们做在 frontmatter 上，我们做在启动配置上。

## 4. 指令文件的语义（`mods/agents-md`）

这个 mod 是 `AGENTS.md` 按 `CLAUDE.md` 的读法读进上下文，四个可选模式：
`claude-md` / `claude-md-or-agents-md`（默认）/ `claude-md-and-agents-md` / `managed-only`。
它对 PRODUCT_PLAN §5.3「工作区 `AGENTS.md` 优先由 DSH 的既有机制装载」有两点参考：

- **要有「谁的指令文件说了算」的明确判据**，而不是两套规则并存。它的默认是
  「项目自己有 `CLAUDE.md` 就整个交给引擎，否则读 `AGENTS.md`」——
  一个二选一的默认，不是叠加。
- **它的「Where it still differs from CLAUDE.md」那一节值得抄体例**：把「做不到的八件事」
  逐条列出来，每条说明为什么做不到。这与本仓 AGENTS.md「不假装做了」是同一种纪律。

## 5. 对 Saiku 的取舍（建议，未实施）

| 他们的做法 | 我们该不该跟 | 理由 |
|---|---|---|
| 风格在会话开始时固定 | **跟** | 我们已经天然满足：人格写进每任务的 DSH patch，续跑开新会话但用的是同一份 patch |
| 风格以插件分发 | **暂不跟** | 我们还没有插件系统；先做成 `prompts/` 下的文件 + Agent 定义引用，等真有第三方再说 |
| `keep-coding-instructions` 的分层 | **已经跟了** | 就是 `PromptScope` 的三档 |
| SessionStart hook 做注入点 | **不跟** | 那是 Claude Code 的事件名；我们对应的是「生成 DSH patch 时决定 `personaSuffix`」 |
| 指令文件的二选一默认 | **未定** | DSH 自己怎么读 `AGENTS.md` 要先核实（M2 未做） |

## 6. 未核实

- 本调查只读了**公开检出**（`plugins/`、`mods/`、`CHANGELOG.md`、`mods/types/*.d.ts`），
  不是 Claude Code 的完整实现。真正的 system prompt 组装在引擎里，这里看不到。
- `CHANGELOG.md` 是时间线证据，不是设计文档；上表的「驱动力」是从行文推断的，
  **没有 RFC 或 issue 可依**。
- DSH 自己怎么装载工作区 `AGENTS.md`、有没有 `keep-coding-instructions` 的等价物 —— 未读。
