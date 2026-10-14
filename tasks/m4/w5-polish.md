# 任务书 w5-polish：日常可用的收尾（信息栏真实数据、首次运行、异常恢复、提示）

（先读 `C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/_common.md`，全部条款适用。）

## 前置

会话、命令、输入、工具视图、模式都已接通。规格：PRODUCT_PLAN §7.3「信息栏」「Home」、§7.5。
参考 v1.19.0：`routes/session/sidebar.tsx`、`feature-plugins/sidebar/{context,files,footer,todo}.tsx`、`feature-plugins/home/*`、
`ui/toast.tsx`、`feature-plugins/system/notifications.ts`。

## 要做的

1. **信息栏只显示真实数据**：会话标题、cwd、git 分支与脏状态、当前模型 / effort / Agent / 授权级别、本会话 token 用量
   （provider 报的）、上下文占用（有模型 contextWindow 时才算百分比，没有就不显示）、本会话改动过的文件、todo（DSH 有 todo 工具时）。
   **没有数据的区域省略**，不填演示数字（§7.3）。宽 > 120 列自动并排、否则按需覆盖（现有行为保持）。
2. **Home**：品牌区、提示（照 tips，内容换成 Arc Deck 的真实功能与键位）、页脚（cwd、版本、Remote 状态）。
3. **首次运行与故障**：没有配置 → 提示 `arcdeck config init`；运行时没装 → `arcdeck runtime install`；缺密钥 → `arcdeck auth set <provider>`；
   Remote 内核意外退出 → 界面报错并提供重连，不卡死。全部给出可复制的命令。
4. **终端恢复**：panic hook 恢复终端（离开 alternate screen、关 raw mode、关鼠标捕获与键盘增强、显示光标）；Ctrl+C / 关闭窗口的收尾；
   resize 时布局正确重排。
5. **toast**：短暂通知（照 toast.tsx），用于「已复制」「已切换模型（下一轮生效）」之类；错误类通知不自动消失。
6. **复制**：选中消息 / 代码块复制到剪贴板（OSC 52；Windows 不支持时说明）。鼠标点击选择消息（若做不到写进未做项）。
7. **性能**：长会话（≥ 500 条消息、单条 ≥ 128 KiB）下滚动与流式不卡顿 —— 渲染只处理可见区域；写一个基准测试或计时测试证明。
8. 测试：信息栏各项在有 / 无数据时的显示；首次运行三种故障的提示；panic 后终端恢复（单测可断言恢复序列被写出）；toast 生命周期；
   长会话渲染只遍历可见行。

## 验收

```
cargo test -p arc-deck-tui --offline
cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings
cargo test -p arc-deck-cli --features test-runtime --offline
```
