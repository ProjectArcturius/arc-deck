# 任务书 w3-tool-views：工具调用的完整呈现（diff、shell 输出、折叠、错误）

（先读 `C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/_common.md`，全部条款适用。）

## 前置

w3-session-core 之后，工具调用经 Remote 进入视图。规格：PRODUCT_PLAN §7.3「工具」：inline 摘要与 block 输出；
运行 / 成功 / 失败 / 拒绝状态；长输出折叠、错误详情可达；**折叠不隐藏权限阻塞 / 失败**；完整输出有入口。
参考 v1.19.0：`routes/session/index.tsx` 里各工具的渲染分支、`util/collapse-tool-output.ts`、`util/tool-display.ts`、
`feature-plugins/system/diff-viewer*.tsx`（diff 查看器）。DSH 的工具名与参数形状按 `REFS/deepseek-harness` 源码核对
（文件读写、编辑、shell / pwsh、搜索、子 agent、run_code 等），**不要按 OpenCode 的工具名猜**。

## 要做的

1. 按工具类别的 inline 摘要（一行：图标 / 名称 / 关键参数 / 状态 / 耗时）与 block 输出：
   - 编辑类：**着色的 unified diff**（增删行、行号、hunk 头），长 diff 折叠，可展开；
   - shell 类：命令 + 输出（尾部 N 行，照 collapse-tool-output 的规则），退出码非 0 标红；
   - 读 / 搜索类：摘要（文件、命中数），展开看内容；
   - 子 agent：显示子会话标题与状态，可进入（打开对应会话）。
2. 状态：运行中（spinner，照 `ui/spinner.ts` 的帧）、成功、失败、被拒（权限 / guard）—— 失败与被拒在折叠态也可见。
3. 完整输出入口：选中某个工具块后一个键打开全屏查看器（可滚动、可搜索），Esc 返回。
4. diff 查看器（`/diff` 或在编辑块上按键）：本会话改动过的文件列表 + 逐文件 diff；不做 split 视图（写进未做项）也可以。
5. 测试：每类工具的摘要与 block 渲染（`TestBackend` 断言文本与样式）；折叠态下失败 / 被拒仍可见；长输出截断规则；
   查看器打开 / 滚动 / 返回；未知工具名有通用回退渲染而不是丢弃。

## 验收

```
cargo test -p arc-deck-tui --offline
cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings
```
