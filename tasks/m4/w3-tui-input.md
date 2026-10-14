# 任务书 w3-tui-input：输入区达到日常可用（@文件、历史、粘贴、换行键、外部编辑器）

（先读 `C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/_common.md`，全部条款适用。）

## 前置

w3-commands 已做出通用补全弹层（触发字符 + 候选提供者）。规格：PRODUCT_PLAN §7.3「输入框」、§7.4 的提交 / 换行键。
参考 v1.19.0：`component/prompt/index.tsx`、`autocomplete.tsx`（`@` 部分）、`history.tsx`、`frecency.tsx`、`stash.tsx`、
`local-attachment.ts`、`prompt/part.ts`、`prompt/display.ts`。现有：`crates/arc-deck-tui/src/editor.rs`（字素级编辑）。

## 要做的

1. **`@` 文件引用**：输入 `@` 弹出工作区文件候选（模糊匹配路径，frecency 加权照上游），接受后插入为一个**整体的引用片段**
   （退格整体删除，照 `part.ts`）。文件列表优先用 `git ls-files`（加未跟踪但未忽略的文件）；不是 git 仓时有界遍历并跳过
   `target/`、`node_modules/`、`.git/`。发送时引用怎么交给 DSH：按 DSH 的 file-references（`session-controller/src/file-references.ts`）
   的形状传；做不到就作为路径文本插入并在最终回复说明。
2. **输入历史**：跨进程持久化（照 `history.tsx`，存在偏好目录），↑ 在首行 / ↓ 在末行时浏览（现在是 Ctrl+↑↓，保留为备选）；
   正在编辑的草稿不因浏览历史而丢失。
3. **大段粘贴**：超过阈值时显示为摘要片段（如 `[粘贴 128 行]`），发送时展开原文（照上游 paste summary 的行为）；
   可通过命令切换是否摘要。
4. **换行 / 提交键**：启用 crossterm 的键盘增强（kitty keyboard protocol，终端支持时）以识别 `Shift+Enter` 换行；不支持时保留
   现有方案（Windows：F6 发送 / Enter 换行；Linux：Enter 发送 / Ctrl+J 换行），并在帮助里如实说明。
   **Windows Terminal 上实测**能否识别 Shift+Enter，把结论写进最终回复（含 WT 版本）。
5. **外部编辑器**（`<leader>e`）：用 `$VISUAL` / `$EDITOR`（Windows 上缺省 `notepad`）编辑当前输入，返回后回填；
   暂停 / 恢复终端状态要正确（raw mode、alternate screen、鼠标捕获）。
6. **metadata 行**：输入区下方显示 Agent / 模型 / effort / 授权级别；宽度不足时按固定优先级依次省略（§7.3），规则写在注释里。
7. **草稿暂存**（stash，照 `stash.tsx`）：可选；做了就加命令进注册表，没做就在最终回复写明。
8. 测试：`@` 候选过滤与整体删除；历史持久化与草稿保留；粘贴摘要的展开；键盘增强不可用时的回退路径；metadata 省略顺序；
   CJK 与组合字符在引用片段前后的光标位置。

## 验收

```
cargo test -p arc-deck-tui --offline
cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings
cargo test -p arc-deck-cli --features test-runtime --offline
```
