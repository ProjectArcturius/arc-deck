# 任务书 w1-model-dialog：模型对话框、provider 列表与 effort 选择

（先读同目录 `_common.md` 的通用约定，它的全部条款适用于本任务。路径：
`C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/_common.md`）

## 背景

`crates/arc-deck-tui/src/catalog.rs` 已经实现了模型目录的纯逻辑（Favorites / Recent / 分组 / 排序 / 模糊搜索 /
偏好读写），但没接界面。现在的 `/models` 对话框只显示用户配置里的那一条路由。规格：PRODUCT_PLAN §7.2（**先读**）、
§7.4 的模型相关键位。参考 v1.19.0：`component/dialog-model.tsx`、`dialog-provider.tsx`、`dialog-variant.tsx`、
`ui/dialog-select.tsx`、`context/local.tsx`。

## 要做的

1. **目录来源**（临时的，下一波由 DSH 的真实目录取代）：在 `crates/arc-deck-cli/src/tui.rs` 里从用户配置
   （`arc_deck_core::userconfig::UserConfig` 的 `provider` 块，OpenCode 形状：`provider.<id>.name`、
   `provider.<id>.models.<id>.name`）构造 `Vec<catalog::Provider>`，并保证**配置的默认模型一定在目录里**
   （provider 不在配置里时以 provider id 作名字）。通过 `Boot` 传给 TUI。在代码注释里写明这是临时来源。
2. `catalog::Model` 增加 `efforts: Vec<String>`（缺省空）。临时来源里一律为空 —— **不要硬编码强度档**
   （DSH 的强度档按模型不同，DeepSeek 官方是 off/low/high/max，pi-ai 路由来自它的目录；下一波接真实目录）。
3. **模型对话框**（照 `dialog-model.tsx` + `dialog-select.tsx`）：
   - 顶部搜索输入；列表来自 `catalog::options`；Favorites / Recent / 各 provider 有**分节标题**；
     当前生效的模型有标记；`footer`（Free）右对齐；`disabled` 的行置灰、显示原因且**不可选中确认**。
   - Enter：把它设为**下一次请求**用的模型（`StartRequest` 增加 `model`，桥接时传 `--model`），
     `touch_recent`，保存偏好；该模型 `efforts` 非空时进入 effort 对话框，否则关闭。
   - `Ctrl+F`：收藏 / 取消收藏当前行；`Ctrl+A`：进入 provider 列表。
   - **界面上要说清**：切换模型是下一次请求生效，不是热切换当前运行（§7.2 最后一条）。运行中也允许选，
     但要显示这条说明。
4. **provider 列表**（照 `dialog-provider.tsx`，只做列表与跳转，**不做**连接 / 登录 provider）：
   列出目录里的 provider，Enter 打开**只含该 provider** 的模型对话框（`only_provider`）。
5. **effort 对话框**（照 `dialog-variant.tsx`，产品名 effort）：列出 `efforts` + 一个「默认」项；选择后写进
   `ModelPrefs.effort` 与当前 `app.effort`，下一次请求带 `--effort`。现有 `/effort` 命令保留。
6. 会话级键位：`leader,m` 打开模型对话框；`F2` / `Shift+F2` 在 Recent 里前后切换当前模型（照 `local.tsx`
   的 `cycle`）。对话框内的 `Ctrl+F` / `Ctrl+A` 只在对话框上下文生效（§7.4：冲突按上下文解析）。
7. 偏好文件：`model.json`，与 `tui-preferences.json` 同目录（`Boot` 里加路径）。启动时读、改动时存。
8. 信息栏 / 输入区 metadata 显示当前模型（展示名）与 effort（有的话）。
9. **对话框代码写在新文件里**（如 `model_dialog.rs`），`app.rs` / `view.rs` 只加最少的接线 ——
   另一个工人同时在改主题对话框。
10. 测试：用 `TestBackend` 与按键序列驱动 `App`：分节标题与顺序、搜索过滤、禁用行不可确认、Enter 后下一次
    `StartRequest` 带上新模型、Recent 被更新并落盘、Ctrl+F 收藏、Ctrl+A → provider → 限定列表、
    有 / 无 efforts 两条路径、F2 / Shift+F2 循环、「下一次生效」的说明可见；`tui.rs` 从配置构造目录
    （含默认模型不在 provider 块里的情形）。`tui.rs` 的测试跑在 `cargo test -p arc-deck-cli` 下，
    所以本任务的验收比通用约定多一条（见下）。

## 验收（本任务额外一条）

```
cargo test -p arc-deck-tui --offline
cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings
cargo test -p arc-deck-cli --features test-runtime --offline
```
