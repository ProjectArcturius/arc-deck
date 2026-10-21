# DSH 的 DeepSeek 优势从哪来，对非 DS 模型是否成立

日期：2026-10-14 · 证据级别：**源码静态阅读 + Ether 工人运行记录**。没有跑过对照实验，
下文凡是「推断」都标出来了；能把推断变成证据的实验见文末，列入 PRODUCT_PLAN 的 M2d。

源码快照：`REFS/deepseek-harness` @ `d743267388`（`0.2.1-alpha.2`）。
**本机安装的 `dsh` 是 `0.1.7-rc.2`** —— 下面的机制在 0.1.7 里是否都已存在**没有核对**，M2a 的第一步就是钉版本。

## 1. 结论

1. **优势分成两半。** 一半只在 DeepSeek 官方路由上生效（协议、缓存保持），另一半是与 provider 无关的
   agent loop 工程（上下文管理、并行工具、重试、溢出落盘……）。
2. **对非 DS 模型：中立的那一半适用，DS 专属的那一半不适用。** 非 DS 模型通过 DSH 的 `llm-pi-ai`
   路由接入（Anthropic / OpenAI / Gemini / OpenAI 兼容网关），走的是同一个 loop。
   **但「DSH 跑非 DS 模型比 OpenCode 跑同一模型更好」没有任何证据** —— 仓里没有这种对照，`BENCHMARK.md` 只讲怎么跑。
3. **「DeepSeek 模型是照着 DSH 训练的」这个说法源码不支持。** DSH 里写明的 RL agent 组合是
   **Claude SWE 兼容**的两工具（持久 `bash` + `str_replace_editor`，persona 就一句
   `You are a helpful software engineer assistant.`），见 `.agents/notes/archived/bug-fix/2026-08-10-minimal-preset-owns-rl-composition.md`。
   todo 工具的形状也是刻意选的「claude-code V1 / opencode / codex 共有、模型最熟悉的形状」
   （`.agents/notes/archived/feature/2026-06-29-todo-write-tool.md`）。DSH 的工具设计策略是**用通用形状**，
   这一点对所有模型都成立，不是 DS 独占。
4. **Ether 的记录是一个反证：DSML 泄漏跟着端点走，不跟着 harness 走。** `dsh + intern-ai 网关`
   出现过把并行工具调用以 `<｜DSML｜tool_calls>` 文本吐出、dsh 直接收工（Ether `docs/DISPATCHER.md` §三）；
   `opencode` 走网关时也偶发同一现象。也就是说 **DSH 没能在第三方网关上挽救 DeepSeek**。
   推断：官方路由用 Anthropic Messages 协议，工具调用由 DeepSeek 服务端解析成 `tool_use` 块，
   所以不该出现这种泄漏 —— **这条尚未实测**。
5. **决策（按用户给的规则「若有，底层全部采用 DSH」）：底层只用 DSH 一个内核。**
   非 DS 模型走 DSH 的 pi-ai 路由；OpenCode 与 Claude Code 只作为 TUI、Agent、工具定义的设计参考，
   不作执行内核。保留一道对照门（M2d）：若同一非 DS 模型、同一批任务上 DSH 明显劣于 OpenCode，
   再讨论是否给非 DS 路由加第二个内核 —— 在那之前不建第二个。

## 2. 证据：DS 专属的机制

| 机制 | 作用 | 源码 |
|---|---|---|
| **Anthropic Messages 协议**（默认 `https://api.deepseek.com/anthropic`），不是 OpenAI chat completions | 文本、thinking、tool call、tool result 都是 content block；回放保留模型与 thinking **signature** —— 多步工具调用之间推理链不被丢掉 | `packages/llm/llm-deepseek/README.md`「Endpoint and wire format」 |
| `systemPromptUpdate: in-history` | 系统提示变化时**追加到已缓存历史之后**，而不是改写开头 —— 前缀缓存不断。默认只有 `deepseek-flash` 声明 | 同上「Minimal configuration」；`packages/core/system-prompt/README.md`「KV Cache effect」 |
| `toolUpdate: addition-only` + `mid-conversation-tool-changes-2026-07-01` beta + `defer_loading` | 工具集中途变化以 `tool_addition`/`tool_removal` 块表达，不重写工具表 —— 同样是保缓存 | `packages/llm/llm-deepseek/README.md` |
| `reasoningEffort`：`off/low/high/max` → `output_config.effort` | 推理强度是一等配置；不支持的值在发请求前就报 `UNSUPPORTED_REASONING_EFFORT` | 同上「Streaming with thinking and images」 |
| SDK `initialize` 自动挂 adapter | **只会自动挂 DeepSeek**；非 DS 必须在 profile 里预先注册 pi-ai 路由 | `packages/sdk/server/README.md`「Known Limitations」 |

**一条必须单独拎出来的：`dsh_session_log` 与 `dsh_plugin_packages` 默认开启。**
官方 DeepSeek 请求会附带**会话日志的增量**与插件清单作为请求元数据（不进模型输入、不影响 KV 缓存）。
这是**数据外发，不是性能机制** —— worker 跑的是 Ether 的源码。
源码：`packages/session/session-log-deepseek/README.md`（「Disable it only when the official API must not receive a Session-log suffix」）、
`packages/llm/deepseek-llm-api-extensions/README.md`。Saiku 生成的 profile 必须**显式**设置它，开或关由用户裁定（PRODUCT_PLAN §11）。

## 3. 证据：provider 中立的机制（非 DS 也适用）

| 机制 | 源码位置 |
|---|---|
| 系统提示分段按序号稳定排序、工具 schema 按配置或字典序排序 —— 字节稳定的前缀，任何有前缀缓存的 provider 都受益；pi-ai 侧有 `cacheRetention` 与 cache-control 格式 | `packages/core/system-prompt/`、`packages/llm/llm-pi-ai/src/catalog.ts` |
| 有界并行工具执行（`isConcurrencySafe` 失败即串行） | `packages/core/tools/`，`.agents/notes/implemented/feature/2026-07-10-parallel-tool-call-execution.md` |
| 上下文压缩：工具结果裁剪、图片卸载 | `packages/compaction/` |
| 大输出落盘只给模型预览（spill） | `packages/spill/` |
| 防循环与超时：重复工具调用提醒、超时策略 | `packages/guard/` |
| 在 durable step 边界重试模型请求 | `packages/llm/llm-retry/` |
| PTC 代码模式（模型写一段程序调用工具，只回传打印与返回值） | `packages/ptc-runtime/`，`.agents/notes/implemented/feature/2026-06-15-ptc.md` |
| 持久会话日志与重放、子 agent、skills、todo | `packages/session/`、`packages/subagent/`、`packages/skill/`、`packages/todo/` |

pi-ai 路由的已知短板（`packages/llm/llm-pi-ai/README.md`「Known Limitations」）：
中途提示/工具更新只对 catalog 里带兼容标记的模型生效，**手写声明的网关路由每次请求都发完整工具表与拼接后的 system**；
拿不到 provider 的 HTTP 状态码；不支持 stop 序列。所以非 DS 路由上的缓存表现会比官方 DS 路由差，这是结构性的。

## 4. 对 Saiku 的直接含义

1. **worker 默认路由是 `deepseek-official`（官方 key）**，不是 intern-ai / opencode-go 之类网关。
   在网关上 DSH 的 DS 优势不成立（§1 第 4 条）。
2. **稳定的前缀放进 system prompt**：Ether 的 `PREAMBLE.md`（工人守则）对每件任务都一样，
   应当进 profile 的 persona，而不是拼在每件任务的用户消息开头 —— 后者在新会话里同样可缓存，
   但放进 system 才和 DSH 的保缓存机制对齐，且不会被当成任务正文。
3. **`dsh_session_log` 显式设置**，不吃默认值。
4. **一个内核**：`arc-deck-dsh` 是唯一的执行适配；核心类型不依赖它（已是现状），
   这条接缝足够将来接第二个内核，现在不预先抽象。

## 5. 把推断变成证据的实验（M2d）

| 编号 | 问题 | 做法 | 判据 |
|---|---|---|---|
| E1 | 官方路由是否真的不泄漏 DSML | 同一批 Ether 任务书，`deepseek-official` 跑 N 件 | DSML 文本出现次数；以 0 为预期 |
| E2 | 完整工具集 vs RL 两工具组合，哪个对 DS 更好 | `sdk` 与 `sdk-minimal` profile 各跑同一批任务 | 完成率、返工率（调度者审查不通过次数）、缓存命中 token、费用 |
| E3 | 非 DS 模型上 DSH 是否不劣于 OpenCode | 同一非 DS 模型、同一批任务，DSH(pi-ai) vs OpenCode | 同 E2；DSH 明显更差才触发「第二内核」讨论 |

样本量和任务集在 M2d 开工前定；单样本结论不算数（Ether `DISPATCHER.md` 里 gemrelay 的「explore 更快」首测就没站住）。
