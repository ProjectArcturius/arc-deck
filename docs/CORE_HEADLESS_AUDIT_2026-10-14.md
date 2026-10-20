# Core / headless 完备性审计与 TUI 第一段

日期：2026-10-14。依据当前 Rust 源码、PRODUCT_PLAN §0 / §6 / §7 / §9 / §10、
STATE_2026-10-14，以及本轮 Windows 离线与伪终端运行。历史 Arch / 真实模型证据只引用，
本轮没有复跑，也没有消耗模型配额。

> **历史快照**：本文件记录的是 2026-10-14 的审计与 M4a 第一段。Remote 会话、工具视图、
> Plan / confirm 审批等此后已接通；当前状态以 [M4_PLAN](M4_PLAN.md) 与
> [STATE_2026-10-14](STATE_2026-10-14.md) 的第六～八轮为准。

## 1. 结论

**单任务 headless 已可用；完整核心与 headless 尚未完备。** 可以继续日常派工，并开始
TUI 的第一段，但不能据此把 M2c/M2d/M3 或完整 M4 标成完成。

| 范围 | 判定 | 证据与剩余工作 |
|---|---|---|
| core 的任务、配置、结果、事件、Agent/模式矩阵 | 已有稳定的单任务契约 | core 单测与 schema 测试；包含授权只能收紧、Plan 验收负例、失败分类和提示词范围 |
| DSH SDK、权威消息、工具配对、UTF-8 分帧 | 已实现并有离线回归 | transport/projection 测试；真实版本核对记录见 PRODUCT_PLAN §9.2a / §10.2b |
| 前台/分离运行、状态、回报、取消、worktree | 已可用 | 原有 Windows/Arch 实测；本轮进程级离线套件重新通过 |
| stalled 有界续跑 | 代码和夹具已覆盖，真实验证未完 | 挂起夹具 → 三次 attempt → STALLED 已覆盖；真实 DSH 卡死续跑未复跑 |
| Ether 接入 | 尚未完整验收 | examples/ether 是示例；真实 Ether 仓派工及与旧 runner 对照尚未完成 |
| M2d 的非 DS 与工具呈现对照 | 未完成 | DSH_FINDINGS 的 E1–E3 仍是实验门，不能从单次跑通推导优劣 |
| Agent / 路径 / 提交权限执行限制 | **M3 未完成** | 下节的真实执行边界没有完全承接 core 策略；单测不等于实际拒绝工具 |
| 交互会话与模型目录 | 未实现 | Rust DSH 适配目前只有 SDK；Remote HTTP/WS、分页、assistant-stream、会话取消与模型目录待接 |
| TUI | 本轮开始 M4a | 独立运行与文件控制面的实际界面；不声称多轮对话或完整 OpenCode 移植 |
| 跨平台 | Windows/Arch headless 有历史证据 | 本轮只在 Windows 验证新增实现；Arch TUI、macOS 和真实终端视觉对照仍待验 |

## 2. 最需要先补的核心缺口：策略没有完整落到执行层

`ExecutionPlan::resolve_with_ceiling` 能算出 `shell=false`、收窄后的 `read_paths` /
`write_paths`、`allow_commit` / `allow_push` 和 Plan 模式。这些值进入结果留档与诊断。
但 `worker::runtime_command` 给真实 DSH 用的仍是启动配置里的 `sandbox`；
`dsh_runtime_command` 没有把最终的 `EffectivePolicy` 转成工具 execution guard。

`runtime::render_patch` 当前配置的是模型、persona 与日志上传开关，没有按 core 的
权限交集注册 guard。Agent 文档的逻辑工具清单也没有成为 DSH 工具执行的实际 allowlist。
所以：

- 把 Agent 换成 Plan，或在任务里收紧为不许 shell，不足以证明真实 DSH 拒绝 shell。
- `workspace` / `full` 有 DSH 的 `DSH_PERMISSION_MODE` 映射，但这不等于所有 Arc Deck
  路径、提交、推送及委派限制都已生效。
- `result.policy` 表达计算所得策略，不能单凭这份留档宣称执行隔离已验证。
- 当前 TUI 不启用 Plan、readonly 或 confirm；这只是对未实现能力的明确告知，
  **不是**以 UI 禁用替代执行限制。既有 headless 的这项缺口仍待 M3 修复。

这是源码核对结论。本轮未发真实模型请求做越权负例，因此不把潜在可执行行为写成
已实测发生的越权。M3 应使用 DSH 的 monotonic guard，并核对 native/PTC、scope-local、
MCP 与子任务路径，再用执行负例验证。仅隐藏工具、提示词劝阻或 git 命令前缀匹配都不够。

## 3. headless 的实际边界

- Windows 的监工存活检查仍退回心跳，不具备 Linux `/proc` 的即时 pid + 启动时刻判死。
  当前 Windows 环境已经能构建，后续可以实现并在这里验证；本轮未改这一块。
- supervisor 仍在 CLI 内；计划中的 `arc-deck-run` 尚未拆。M4a 通过同一个二进制的
  headless 子命令复用它，没有复制 worker、健康算法或执行循环。Remote 接入时仍需要
  正式的运行/会话服务边界。
- 无验收项时 `completed` 只表示内核正常终止，不能等同工程行为通过验收。
- SDK attempt 开新会话，续跑凭工作区成果，不是历史恢复，也不是 exactly-once。
- 旧快照记录的偶发 `failure_turn_reports_failed...` 本轮未复现；一次通过不排除竞态。
- 凭据仍由用户数据目录的 auth.json / 环境引用读取，TUI 不另存密钥；DSH 日志上传缺省关闭。

## 4. 本轮落地的 M4a

入口：`arcdeck` 无参数打开 `arc-deck-tui`。非交互 stdin/stdout 返回 64，并提示使用
headless。现有子命令仍走原路径。

| 功能 | 当前行为 |
|---|---|
| Home | Arc Deck 品牌、居中输入区（最多 75 列）、当前目录；可直接输入 |
| 请求 | 每次提交创建独立 ID / DSH runtime；不自动拼历史来模拟多轮会话 |
| 正文 | 显示最终权威全文；把 Arc Deck 的事实与模型陈述分开；支持标题和 fenced code 的基本样式 |
| 进度 | 读取同一份 status / events；健康由现有 status 算法按当前时间计算 |
| 工具 | 以 call_id 配对摘要/结果，支持折叠；失败摘要不因折叠消失 |
| 历史 | 当前工作区最近 200 条记录，可搜索和重开查看；`/sessions` 是运行历史入口，尚非 Remote 会话列表 |
| 信息栏 | 宽度超过 120 列时并排 42 列；窄窗口显式打开覆盖栏；没有虚构 context、费用或 MCP 数据 |
| 命令 | Ctrl+P 可搜索面板；slash 与快捷键使用同一 action 定义；Ctrl+X leader 的 n/l/a/m/b |
| Agent | Echo / Build 独立于模型；仅影响下一次请求；Plan 明确显示不可用原因 |
| 模型 | 仅显示用户配置路由，注明由运行时校验；尚无目录、搜索、收藏、最近或切换 |
| 授权 / effort | `/permission workspace|full`、`/effort <级别|default>` 只影响下一次请求；运行中拒绝改动 |
| 取消 / 退出 | Esc 请求取消；退出有界等待本次 TUI 所发起的 worker 收尾，不取消其他历史任务 |
| 偏好 | 仅深浅色及信息栏开关持久化到数据目录的 tui-preferences.json，不改用户启动配置 |

`crates/arc-deck-cli/src/tui.rs` 是控制面桥接，renderer 不依赖 DSH、解释 DSH 私有事件
或自建 provider loop。桥接把原文写入 `arcdeck run --prompt -` 的 stdin；不经过 shell，
因此文件名、命令替换字符和超过 Windows 命令行长度的粘贴都能作为指令原文保留。
`--prompt` 是新增公开入口；不提供它时既有任务书/位置参数规则保持原样。

### Windows 键位的明确差异

> **2026-10-20 更正：这条已经改了。** 用户原话是「哪个神人会用 F6 发送」—— M4a 为了躲开
> 粘贴问题而把发送键改成 F6，代价是没人找得到发送键。现在 **Enter 发送**（与 §7.4、
> OpenCode 一致），Shift+Enter / Ctrl+J 换行；粘贴在 Windows 上仍然是**一串按键**，
> 所以运行循环把同一次 poll 里排队的按键整批交给 `App::input_batch`：一批里**按下**的键多于一个
> 时按粘贴处理，其中的 Enter 插入换行而不是提交（Release 不算：Windows 上每个键是 Press + Release
> 两条，按事件条数判的话一次快速的 Enter 就会被当成粘贴 —— 2026-10-20 第八轮修掉）。F6 只剩兼容别名，不出现在任何提示里。
> 代价：人手快到一个批里同时出现字符与 Enter 时会被当成粘贴（再按一次即可）。
> 证据：`enter_sends_and_shift_enter_or_ctrl_j_inserts_a_newline`、
> `a_pasted_batch_inserts_newlines_instead_of_submitting`。

伪终端实测发现：Crossterm 0.29 的 Windows 原生输入会把多行粘贴拆成普通按键，
不能靠启用 `EnableBracketedPaste` 就宣称收到 `Event::Paste`。第一次试验中，粘贴的
换行确实把两行变成了两次请求。

修正后 **Windows 用 F6 发送，Enter 换行**；输入区和页脚明确显示这两个键。
选择面板内 Enter 仍是确认。Linux 保留 Enter 发送、Ctrl+J 换行和 bracketed paste，
F6 作为共同的发送替代键。Linux 这条路径本轮没有真实终端实测。
这是 M4a 与 PRODUCT_PLAN §7.4 的已知差异，尚未宣称参考交互完全迁移。

输入编辑以 Unicode 字素为单位，列宽按终端格计算。多行 ↑/↓ 移动光标；
Ctrl+↑/↓ 浏览本次进程的输入历史。PageUp/PageDown / 鼠标滚轮滚正文；上滚停止追随，
`/latest` 或 Ctrl+End 回到最新。鼠标点击、选择与命中尚未实现。

### 诚实保留的未完成项

Remote 多轮会话 / 历史恢复 / 实时 token / 思考块；模型目录、provider、Favorites、Recent
及 effort 档选择；Plan 与确认审批；完整 Markdown、代码高亮、工具完整输出组件；
精确语义 token 全集；真实终端截图与 OpenCode 同终端对照；Arch TUI 验收。

显示有界：事件只读最近 2 MiB，正文最多 128 KiB，遇截断注明完整证据位置。
工具组件展示的是现有产品事件的摘要，不声称它已经展示完整工具输出。
会话页签未加入；GUI 路线未改。

## 5. 验证与复现

```powershell
# 正常发布构建，无假 SDK 对端
cargo build --release -p arc-deck-cli --offline
.\target\release\arcdeck.exe

# 完整离线回归：Windows 本轮 204 项（Unix 另有平台专属测试）
cargo test -p arc-deck-core -p arc-deck-dsh -p arc-deck-cli -p arc-deck-tui `
    --features arc-deck-cli/test-runtime --offline

# 新 TUI 单独做严格 lint；其余既有 crate 仍有历史 lint 警告
cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings
```

本轮 Windows 伪终端（ConPTY）实际操作：

1. 无参数启动 → Home；Ctrl+P 搜索并选择 theme；界面切换。
2. 多行中文 + 反引号 / `$(literal)` 留在编辑区，不产生运行；F6 后仅新增一个运行目录，
   task.json 保留完整两行；夹具 completed，最终正文出现一次。
3. cancel 夹具：F6 启动 → running → Esc → cancelled；runtime_pid 对应进程不存在。
4. 第二件 cancel 夹具仍在跑时 `/exit` + F6 → cancelled；runtime 进程退出。
5. 退出输出中确认关闭 bracketed paste / 鼠标捕获、退出 alternate screen 并恢复光标。

最初错误粘贴试验的两个独立记录保留在 **target/tui-smoke/**，与修正后的试验一起作为
本地诊断数据；该目录被忽略、不进仓，也没有使用真实凭据或模型。

TestBackend 覆盖 40×12 / 80×24 / 120×30 / 160×40、Home/长正文/各面板、深浅色、
中文光标、滚动锚点、失败不被折叠隐藏、原文 stdin 与非交互退出。
文本格快照输出到 **target/tui-preview/**；它们是自动化 buffer 证据，**不是真实终端截图**。

## 6. 下一段的顺序

1. M3：把 EffectivePolicy 传到 DSH execution guard，补真实拒绝负例；优先 Plan。
2. Rust Remote 适配：隔离 home / 认证 / HTTP Connection / WS mux / 分页 / stream / 单会话取消；
   复用产品投影并用隔离数据对账，避免接管 Desktop 的真实 home。
3. M4b：将独立运行 UI 接到真正会话，补模型/provider/effort/本地收藏最近与交互状态。
4. Arch 真终端与 OpenCode v1.19.0 的同尺寸、同字体截图对照；用户判断视觉迁移是否满意。

Ether 验收和 M2d 仍在队列，不因开始 TUI 而被勾成完成。

依赖搭配核对自 [Ratatui 官方安装说明](https://ratatui.rs/installation/) 与
[Crossterm event 文档](https://docs.rs/crossterm/0.29.0/crossterm/event/index.html)。
OpenCode 的布局/调色参考保留 MIT notice 于 `crates/arc-deck-tui/LICENSE-OpenCode`。
