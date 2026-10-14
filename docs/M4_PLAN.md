# M4 · TUI 完整落地的派工计划

日期：2026-10-14 起。用户裁定：TUI 做到 PRODUCT_PLAN §7 **完全完成**，用 `arcdeck` headless 派工人干
（`opencode-go/deepseek-v4.1-flash` · `effort high` · `--permission full --worktree dsh/<id>` · 每件都带 `--accept`）。
Claude 是派工者：写任务书、复跑验收、逐行审阅、在工人分支上提交后挑进 main。工人不提交（`allow_commit: false`）。

任务书在 `tasks/m4/`，共用条款在 `tasks/m4/_common.md`。**本文件只记计划与状态；证据在各提交正文里。**

## 「完全完成」的判据

PRODUCT_PLAN §7.3 的八个组件、§7.4 的键位与命令、§7.5 的视觉与运行验收。其中有三样不是 TUI crate 自己能完成的：

| 依赖 | 为什么 | 波次 |
|---|---|---|
| Rust Remote 适配（会话列表 / follow / 翻页 / 发消息 / 取消 / 模型目录） | 多轮会话、流式正文、思考折叠、会话列表、真实模型目录都只在 Remote 层有 | W2–W3 |
| M3 执行 guard（EffectivePolicy → DSH 工具执行限制） | `/plan`、readonly、确认审批在 TUI 上「可用」的前提是执行层真的拒绝 | W4 |
| 视觉对照 | §7.5 要同终端、同尺寸与 OpenCode v1.19.0 对照；**满意与否由用户判断** | W5 |

## 波次与状态

**派工手册**（队列依赖、派工命令、收工审阅流程、踩过的坑）：[`tasks/m4/README.md`](../tasks/m4/README.md)。
「日常可用」的七条判据也在那里 —— 用户 2026-10-14：做到能像 OpenCode 一样日常写代码、可以转去专注 GUI 的程度，
剩余调度可以交给替班（sol）。

| 顺序 | 任务 | 内容 | 状态 |
|---|---|---|---|
| 试点 | tui-model-catalog | 模型目录纯逻辑（§7.2） | ✅ 202b551 |
| 0 | w1-markdown | 助手正文 Markdown（pulldown-cmark） | 派出 |
| 0 | w1-theme | 2.0.27 语义 token、v1→v2 迁移、内置主题、主题对话框 | 派出 |
| 0 | w1-model-dialog | 模型 / provider / effort 对话框，F2 循环，偏好落盘 | 派出 |
| 0 | w2-remote-transport | Remote host / rpc / mux；0.2.1 协议核对文档 | 派出 |
| 0 | w4-m3-guard | EffectivePolicy → DSH 配置行，映射表 + `--dump-config` 验证 | 派出 |
| 1 | w3-commands | 命令系统：action 注册表、键位与上下文、slash 补全、面板、帮助 | 待 w1 全部合入 |
| 1 | w2-remote-session | 会话操作、事件投影、审批数据、模型目录 | 待 w2-remote-transport |
| 1 | w4-m3b-negative | 脚本化假模型端点驱动真实 DSH，证明越权被拒（零配额） | 待 w4-m3-guard |
| 2 | w3-tui-input | `@` 文件、历史、粘贴摘要、Shift+Enter、外部编辑器、metadata | 待 w3-commands |
| 2 | w3-session-core | TUI 改走 Remote：多轮、流式、思考、取消；`/runs` 保留看工人 | 待 w3-commands · w2-remote-session |
| 3 | w3-session-list | 会话列表 / 恢复 / 改名 / 时间线；真实模型目录 | 待 w3-session-core |
| 3 | w3-tool-views | diff、shell 输出、折叠、完整输出查看器 | 待 w3-session-core |
| 3 | w4-tui-modes | Agent / Plan / Build / 授权级别 / confirm 审批界面 | 待 w3-session-core · w4-m3b-negative |
| 4 | w5-polish | 信息栏真实数据、首次运行与故障提示、终端恢复、toast、复制、长会话性能 | 待顺序 3 全部 |
| — | 人工 | Arch 真终端、与 OpenCode v1.19.0 同终端对照（§7.5）；满意与否由用户判断 | 队列做完后 |

## 派工中发现并已修的产品问题

- `4cf84b0` 旧配置字段（`sandbox` → `permission`）的报错不说新名字。
- `b227176` Windows 上 headless 弹常驻 / 闪烁的终端窗口；`$(arcdeck run --detach)` 挂到任务结束。

## 派工中发现、尚未修的

- **磁盘写满被归为 `MODEL_FAILURE`**（w2-remote-transport，2026-10-14）：运行时因 `ENOSPC` 以 1 退出，`result.json` 的 blocker
  是 `MODEL_FAILURE: ENOSPC…`。根因是环境（磁盘），不是模型；调度者按 `retryable` 决策时会被误导。应在失败分类里认出
  `ENOSPC` / `no space left` 并给独立的码（如 `HOST_DISK_FULL`，不可重试直到腾出空间）。监工本身随之死亡，`status` 由读者
  判 `dead` —— 这一半是设计如期生效（进程死了不能自称健康）。
