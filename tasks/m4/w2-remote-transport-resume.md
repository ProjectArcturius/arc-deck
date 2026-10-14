# 续做：w2-remote-transport

上一个工人在做本任务时因**磁盘写满（ENOSPC）**中断，进程已退出。磁盘已清出空间。
**它的进度已作为 `wip(dsh)` 提交放在你的分支上**（`git log --oneline main..HEAD` 看，目前两个）。
第二次运行在用户叫停时被取消：当时 `cargo test -p arc-deck-dsh --offline --lib` 里**有一条测试会挂住不返回**
（mux / 假服务端 `remote/fake.rs` 相关，工人正在修）。**先找出是哪一条**（`-- --test-threads=1` 加 `--nocapture` 逐条跑），
给 `remote/` 里所有等待网络或流的测试加上超时，再继续。

1. 先读原任务书：`C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/w2-remote-transport.md`（它引用的 `_common.md` 等全部条款仍然适用）。
2. 再看已做的：`git log --stat main..HEAD` 与 `git diff main...HEAD`。判断哪些已完成、哪些半截、哪些没开始。
3. 在已有进度上**接着做完**原任务书的全部要求；不要推倒重来，也不要为了省事删掉上一个工人写好的测试。
   发现上一个工人写错的地方可以改，在最终回复里说明。
4. **省磁盘**：不要 `cargo clean` 之外再建新的构建目录；不要跑 `--release` 构建；大输出别写进文件。
5. 验收与最终回复的要求照原任务书。最终回复里分开写「上一个工人已完成的」与「你这次做的」。
