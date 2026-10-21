# 任务书 w4-m3b-negative：用脚本化假模型端点证明越权工具调用被真实 DSH 拒绝

派工者：claude（或替班 sol）。仓库：arc-deck。独立 worktree。**动 `crates/arc-deck-dsh`（测试支撑）、`crates/arc-deck-cli`（测试）、`docs/M3_GUARD.md`。**

## 前置（先读）

`docs/M3_GUARD.md`（w4-m3-guard 的映射表，**末尾有本任务应覆盖的负例清单**）与它在 `runtime.rs` / `worker.rs` 里的实现。

## 思路

不花配额、结果确定：在测试里起一个本机 HTTP 服务，冒充模型 provider（协议选 DSH 的 pi-ai 支持且最容易脚本化的一种，
如 `openai-completions` 或 `anthropic-messages`，按 `REFS/deepseek-harness/packages/llm/llm-pi-ai/` 源码核对请求与流式响应格式）。
第一次请求回一个**越权的工具调用**（例如 Plan 模式下写文件、`shell=false` 时跑命令、写到收窄路径之外），
第二次请求（带着工具结果回来）回一段收尾文本并结束。然后检查：工具结果是拒绝、文件系统没被改、Saiku 的事件与结果如实反映。

## 要做的

1. 假 provider：测试支撑代码（`#[cfg(test)]` 或 test-runtime 特性后面，**发布构建里不存在**）。记录收到的请求，
   便于断言「DSH 发给模型的工具清单里还有没有被禁的工具」（可见性收窄）与「工具结果是拒绝」（执行拒绝）两件事分开验证。
2. 用 `saiku run`（工人模式、真实运行时、临时 `DSH_HOME`、用户配置里的 provider 指向假端点）跑 `M3_GUARD.md` 清单里的每个负例，
   每例断言：越权效果**没有发生**（文件不存在 / 未被修改；命令的副作用不存在）、DSH 返回的工具结果是拒绝、`result.json` 的终态与
   `events.jsonl` 如实；再加一个**正例对照**（同样的调用在允许的授权下确实生效）—— 否则「被拒」可能只是工具本来就坏了（P9 的教训）。
3. 发现映射表里「声称强制」但实测没拦住的：**不要改测试迁就**，在 `docs/M3_GUARD.md` 里把那一行改成实测结论，修实现，
   修不了就让 Saiku 在那种组合下**拒绝启动**并给出原因。
4. 文档：`docs/M3_GUARD.md` 每一行补上「实测：拦住 / 未拦住 / 未测」与对应测试名。

## 约束

`_common.md` 的通用条款适用（不提交、不联网、不碰 REFS、注释写 why、不引入新依赖）。测试只连 127.0.0.1。

## 验收

```
cargo test -p arc-deck-core -p arc-deck-dsh --offline
cargo test -p arc-deck-cli --features test-runtime --offline
cargo test -p arc-deck-dsh -p arc-deck-cli --features arc-deck-cli/test-runtime --offline -- --ignored m3_live_
```
（负例测试以 `m3_live_` 为前缀，需要本机已安装的运行时，所以是 `#[ignore]`。）
