## 通用约定（每份 M4 任务书都适用）

派工者：claude。仓库：arc-deck（Rust workspace）。你在一个独立 git worktree 里工作，别的工人同时在改别的 worktree。

- **先读**：`AGENTS.md` 顶部「当前产品方向」、`docs/PRODUCT_PLAN.md` §7（TUI 规格）、`crates/arc-deck-tui/src/` 现有代码。
- **参考源码**（OpenCode，MIT）不在你的 worktree 里，只能这样读，**只读不改**：
  `git -C C:/Users/mc158/Documents/Projects/arc-deck/REFS/opencode show v1.19.0:<路径>`（结构与交互基线）
  `git -C C:/Users/mc158/Documents/Projects/arc-deck/REFS/opencode show v2.0.27:<路径>`（主题语义 token）
  列文件：`git -C … ls-tree -r --name-only v1.19.0 packages/tui/src`
- 移植的是**行为与设计**，不是逐行翻译 TSX；Solid/OpenTUI 的写法在 Rust + ratatui 里要按 Rust 的方式表达。
  有意与上游不同的地方，在代码注释里写明理由。
- **注释写 why 不写 what**；标识符英文，注释可中文；仿照现有文件风格。不要重排你没改的代码。
- **不许引入新依赖**（构建是 `--offline` 的）。现有可用：ratatui 0.30、crossterm 0.29、unicode-width、
  unicode-segmentation、serde/serde_json、pulldown-cmark 0.13（无默认特性）。
- **不要提交（commit）**，改动留在工作区；派工者审阅后提交。不要运行联网或消耗模型配额的命令。
- 界面上的命名：模型强度叫 **effort**（不用 `variant`）；权限叫**授权级别**（不说 DSH 沙箱档名）。
- 没有真实数据的地方**不编**（不填演示数字、不假装功能可用）；做不到的写进最终回复。
- **不做 session tabs**（会话页签）—— 用户明确不要（PRODUCT_PLAN §7.1 补记）。

最终回复写：做了什么；每条要求对应哪个测试；偏离任务书的地方与理由；没做或拿不准的地方。

验收（派工者会原样复跑）：

```
cargo test -p arc-deck-tui --offline
cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings
```
