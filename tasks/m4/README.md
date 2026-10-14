# M4 任务书队列 · 派工手册（给接手调度的人：claude / sol）

目标：**Arc Deck TUI 能像 OpenCode 一样日常写代码**，之后产品重心可以转向 GUI。
状态表在 `docs/M4_PLAN.md`（每合一件就更新那一行）；本文件是**怎么派、怎么收**。

## 一、「日常可用」的判据（全部满足才算这条队列做完）

1. 在一个仓里敲 `arcdeck` → 输入 → **流式**回答（Markdown 正确），工具调用可见，同一会话**多轮**继续。
2. 能列出、搜索、**恢复**以前的会话并接着聊；改名；历史完整。
3. 能切换模型 / effort / Agent；Plan / Build / 授权级别**在执行层生效**；confirm 有审批界面。
4. 运行中能取消；模型失败、配额、缺密钥、运行时没装都有可读、可操作的提示。
5. 输入：多行、粘贴、历史、`@` 文件、`/` 命令补全、外部编辑器。
6. 编辑类工具显示 diff；shell 输出可折叠；失败 / 被拒在折叠态也可见。
7. 主题可选；信息栏只显示真实数据；Windows Terminal 与 Arch 的终端里都能用，退出 / 崩溃后终端恢复正常。

判据 7 的「Arch 真终端」与 OpenCode 同终端视觉对照（PRODUCT_PLAN §7.5）**需要人看**，不在派工队列里 —— 队列做完后由用户判断。

## 二、队列与依赖

| 顺序 | 任务书 | 依赖（必须先**合进 main**） | 改动范围 |
|---|---|---|---|
| 0 | w1-markdown · w1-theme · w1-model-dialog | — | tui |
| 0 | w2-remote-transport | — | dsh `remote/` |
| 0 | w4-m3-guard | — | core · dsh `runtime.rs` · cli `worker.rs` |
| 1 | **w3-commands** | 全部 w1 | tui |
| 1 | w2-remote-session | w2-remote-transport | dsh `remote/` |
| 1 | w4-m3b-negative | w4-m3-guard | dsh · cli 测试 |
| 2 | w3-tui-input | w3-commands | tui |
| 2 | w3-session-core | w3-commands · w2-remote-session | tui · cli `tui.rs` |
| 3 | w3-session-list | w3-session-core | tui · cli `tui.rs` |
| 3 | w3-tool-views | w3-session-core | tui |
| 3 | w4-tui-modes | w3-session-core · w4-m3b-negative | tui |
| 4 | w5-polish | 顺序 3 全部 | tui · cli `tui.rs` |

同一顺序里的任务可以并行（改动范围基本不重叠；重叠处在合并时解决）。**一次最多 5 件**（本机 16 核 / 32 GB 实测够用）。
依赖没合进 main 就派，工人会在旧代码上干活 —— worktree 从 main 的 HEAD 开分支。

## 三、派工

在仓库根目录（`arc-deck/`）执行。`T` 是任务书目录的绝对路径。

```bash
T=$(pwd)/tasks/m4
A_TUI="cargo test -p arc-deck-tui --offline"
A_CLIPPY="cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings"
A_CLI="cargo test -p arc-deck-cli --features test-runtime --offline"

# 模板：每份任务书末尾的「验收」节里有几条，就带几条 --accept，原样照抄
arcdeck run "$T/<id>.md" --id <id> --by <你是谁：claude|sol> --worktree dsh/<id> \
    --permission full --effort high --accept "<验收 1>" --accept "<验收 2>" … --detach

arcdeck status --all          # 每件一行；不要读长日志
arcdeck wait <id> --timeout 3h
arcdeck report <id>           # 终态后：Arc Deck 采集的事实 vs 模型的陈述
```

- `--permission full` 是 `--worktree` 的硬要求（worktree 的 `.git` 指向主仓，非 full 时里面每条 git 命令返回 128）。
- 模型用用户配置的默认值（`opencode-go/deepseek-v4.1-flash`）；不要换。
- 改了 arcdeck 自身的代码后，**先重编 release 并装到 `~/.cargo/bin` 再派工**（拿旧二进制派工，测的是旧代码）。

## 四、收工（每一件都要做，顺序不能省）

1. `arcdeck report <id>`：先看「Arc Deck 采集的事实」（验收结果、改动文件），再看「模型的陈述」（**不是证据**）。
2. **自己复跑验收**：在 `../.arcdeck/wt/<id>` 里把任务书的验收命令原样跑一遍。
3. **读 diff**（`git -C ../.arcdeck/wt/<id> status` / `diff`，新文件直接读）。重点看：
   - 是否越出任务书允许的文件范围；
   - 有没有为了让测试过而改弱断言、删测试、`#[ignore]` 掉失败的测试；
   - 有没有编数据（演示数字、硬编码的模型档位）；
   - 「不可用」是不是真的给了原因，而不是静默；
   - 有没有新的 Windows 子进程没带 `no_window`（会弹窗，用户明确不接受，见 commit b227176）。
   不合格：写一份**补充任务书**（`<id>-fix.md`，只写要改的点），用**同一个 id** 加 `--rerun` 派回去，或自己改：
   ```bash
   arcdeck run "$T/<id>-fix.md" --id <id> --rerun --by <你> --worktree dsh/<id> --permission full --effort high --accept … --detach
   ```
   **必须用同一个 id**：worktree 目录按 id 命名，同 id 时直接复用那个目录（未提交的改动都还在）；换个新 id 会新建目录，
   而分支已在原 worktree 里检出，`git worktree add` 会失败。`--rerun` 把旧的运行记录挪到一边（`arcdeck report` 只看新的这次）。
4. 在工人分支上提交（Conventional Commits，正文中文，写清**来源**：任务 id、模型、用时、步数，以及审阅者），然后：
   ```bash
   git -C ../.arcdeck/wt/<id> add -A && git -C ../.arcdeck/wt/<id> commit -F <消息文件>
   git cherry-pick <那个提交>              # 在主仓 main 上
   ```
   冲突时以 main 上已合并的接口为准改工人的代码，**改完重跑全量离线套件**。
5. 主仓全量离线套件：
   `cargo test -p arc-deck-core -p arc-deck-dsh -p arc-deck-cli -p arc-deck-tui --features arc-deck-cli/test-runtime --offline`
6. 清理：`git worktree remove --force ../.arcdeck/wt/<id>`（每个 worktree 的构建目录约 0.4–1 GB）；分支保留。
7. 更新 `docs/M4_PLAN.md` 的状态行（填提交号）。
8. **不推送**。推送等用户明确指示。

## 五、已经踩过的坑

| 现象 | 原因 / 做法 |
|---|---|
| 派工立刻 64：`unknown field sandbox` | 用户配置旧字段名；改成 `"permission": "workspace"`（4cf84b0 之后报错会说新名字） |
| 派工后屏幕上有常驻 / 闪烁的终端窗口 | 已修（b227176）；若再出现，查新加的子进程是否漏了 `CREATE_NO_WINDOW` |
| `$(arcdeck run --detach)` 一直不返回 | 已修（b227176），同一根因的另一半 |
| 工人「完成」但功能没生效 | 拿旧 release 二进制在跑；或改动没被编进去（AGENTS U16 同类） |
| `completed` 但验收没跑 | 没带 `--accept`。永远带 |
| live / quota 测试 | `remote_live_*`、`m3_live_*`、`tui_live_*` 起真实运行时但不发 prompt（零配额）；`remote_quota_*` 会发一条极短消息（用户允许少量 flash） |
