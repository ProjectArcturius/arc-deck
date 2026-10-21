# 任务书 w4-m3-guard：把 Saiku 的授权策略落到 DSH 的执行层（M3 第一段）

派工者：claude。仓库：arc-deck（Rust workspace）。你在独立 git worktree 里工作，别的工人同时在改
`crates/arc-deck-tui` 与 `crates/arc-deck-dsh/src/remote/`（新模块）。**本任务动 `crates/arc-deck-core`、
`crates/arc-deck-dsh/src/runtime.rs`、`crates/arc-deck-cli/src/worker.rs` 与 `docs/`**；不要碰 `remote/` 与 TUI。

## 背景（先读）

`docs/CORE_HEADLESS_AUDIT_2026-10-14.md` §2：`ExecutionPlan::resolve_with_ceiling` 算出的 `EffectivePolicy`
（`shell`、收窄后的 `read_paths` / `write_paths`、`allow_commit` / `allow_push`、Plan 模式）**只进了结果留档**，
没有变成 DSH 的执行限制。真实 DSH 用的仍是启动配置里的 sandbox 档。也就是说：把 Agent 换成 Plan、或任务书收紧为
不许 shell，**真实 DSH 照样能写文件、跑 shell**。PRODUCT_PLAN §5.6（授权级别）、§5.3 / §5.4（Agent 与模式）、
§9（DSH 接法）、第 845 行附近（交集原则）是规格。

DSH 源码（只读，与钉住的运行时同版本）：`C:/Users/mc158/Documents/Projects/arc-deck/REFS/deepseek-harness/`。
已知的关键事实（**你要按源码核实**）：
- `packages/plan/plan-mode/README.md`：Plan 模式**不限制任何工具**，「use sandbox mode and approval prompts for enforced limits」。
  所以 Saiku 的 Plan 不能只靠开 DSH 的 plan-mode。
- `packages/sandbox/sandbox-policy/`：bash / 文件系统 / terminal 共用的文件效果策略（`read-only` / `workspace-write` / …、workspace 根）。
- `packages/core/tools/`：工具注册与执行管线，有 allow / deny / ask 策略与 per-agent 限制（可见工具集收窄）。
- `packages/guard/`（permission-presets、user-approval、timeout-policy）、`packages/sandbox/*`（bash-sandbox、pwsh-sandbox、
  sandbox-windows-acl …）。
- `apps/cli/`：`--patch` 叠加与 `--dump-config`（**组合并校验配置但不启动、不发模型请求**）。

## 要做的

1. **映射表** `docs/M3_GUARD.md`：`EffectivePolicy` 的每个字段（以及 Plan 模式、readonly / workspace / full 授权级别）
   → 落到 DSH 的哪一行配置（包名 / 配置键 / 取值）→ 保证范围（OS 沙箱强制 / 工具管线拒绝 / 仅可见性收窄 / 做不到）。
   **每一行注明源码出处（文件:行）**。做不到的（例如 DSH 若没有按路径列表限制写入、没有按 git 子命令区分提交与推送）
   要**如实写成缺口**，并说明 Saiku 现在怎么处理（拒绝启动？降级到更严的档？仅留档？）——
   **宁可拒绝启动，也不能静默放宽**（PRODUCT_PLAN 的交集原则：只能收紧）。
2. **实现**：`render_patch` 接收最终的 `EffectivePolicy`（扩展 `PatchInput`），按映射表生成限制行；`worker.rs` 的
   `dsh_runtime_command` 把 `prepared.plan.policy` 传进去。DSH 沙箱档从 `EffectivePolicy` 推导，而不是只看启动配置
   （`authz::Permission` 是映射的唯一出处，不要另写一份）。
   - Plan / readonly：文件写入与 shell 必须在执行层被拒（不是只靠提示词）。
   - `shell = false`：bash / pwsh / terminal 类工具不可执行。
   - 收窄的读写路径：DSH 能表达到什么程度就表达到什么程度；表达不了的按第 1 条处理。
   - 子 Agent / Skill / 插件 / project instructions 不得扩大权限：检查 DSH 有没有让它们绕过上述限制的路径，写进映射表。
3. **测试**
   - 单测：每种组合（Echo / Build / Plan × readonly / workspace / full × shell 开关 × 收窄路径）生成的 patch 断言到具体行与取值；
     「只能收紧」的负例（任务书想放宽时被拒，现有 authz 测试风格）。
   - **live 测试**（`#[ignore]`，名字以 `m3_live_` 开头）：对本机已安装的真实运行时，用 `--dump-config` 组合
     `--profile sdk` + 生成的 patch，断言 DSH **接受**这份 patch，且组合后的配置里限制确实在（解析 dump 出的配置）。
     临时目录作 `DSH_HOME`；**不发任何 prompt，不消耗模型配额**。
   - **不要求**本任务证明「真实工具调用被拒」—— 那需要一个脚本化的假模型端点，是下一段任务（m3b）。
     但请在 `docs/M3_GUARD.md` 末尾写出 m3b 应该覆盖的负例清单（每条：构造什么工具调用、期望 DSH 怎么拒）。

## 约束

- 注释写 why；标识符英文，注释可中文。不要重排你没改的代码。不许引入新依赖（`--offline`）。
- 不要提交；不要联网；不要碰 `REFS/`；不要改 DSH Desktop 的 home。
- 现有离线套件必须继续全绿（下面第二条验收）。

## 验收（派工者原样复跑）

```
cargo test -p arc-deck-core -p arc-deck-dsh --offline
cargo test -p arc-deck-cli --features test-runtime --offline
cargo test -p arc-deck-dsh -p arc-deck-cli --features arc-deck-cli/test-runtime --offline -- --ignored m3_live_
```

最终回复写：映射表摘要；哪些限制是真实强制、哪些只是可见性、哪些做不到；每条要求对应的测试；没做 / 拿不准的地方。
