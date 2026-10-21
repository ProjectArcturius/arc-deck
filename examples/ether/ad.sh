#!/usr/bin/env bash
# ad.sh —— 用 saiku 派一件 Ether 工人任务（取代 tools/dispatch/dsh.sh + dsh-runner.sh）。
#
# 用法: ad.sh <win|arch> <ether|kanesumi> <id> <task.md> [验收命令 …]
#   立即返回；之后只用一行命令跟踪：
#     saiku status <id>
#     saiku wait <id> --timeout 2h
#     saiku report <id>
#
# 与 dsh-runner.sh 的对应（参 arc-deck docs/PRODUCT_PLAN.md §6.6）：
#   worktree + 分支 dsh/<id>、子模块初始化、CARGO_TARGET_DIR 每任务独占、PREAMBLE 进 system prompt、
#   卡死看门狗与续跑、后台化（不再需要 nohup）、收尾时的 git log / git status —— 都由 saiku 做。
#
# 三件必须知道的事：
#   1. **`--worktree` 要求授权级别 `full`**（启动配置里 `worker.permission`）。
#      worktree 的 `.git` 指向主仓、主仓在 workspace 之外，`workspace` 级别下里面每条
#      git 命令都返回 128 —— 实测整轮 34 秒、8342 输入 token 白烧。saiku 会在派工
#      那一秒警告，但别依赖它。隔离由 worktree 本身承担，与原 runner 一致。
#   2. **总是带验收命令。** 没有 `--accept` 时 `completed` 只表示内核正常结束。
#   3. **跨机器那一跳自己套。** 下面只做「在本机某个仓里派一件」。反向隧道、ssh、
#      Windows 改写过的任务书要去 CR —— 都是调度者那端的事，不是 worker 的职责
#      （DISPATCHER §三）。
#
# 退出码：0 = 派工成功（工人已接住）；64 = 派工失败（原因在 stderr）。
# 注意这与任务本身的终态无关 —— 任务成没成看 `saiku status <id>`。
set -euo pipefail

host=$1; repo=$2; id=$3; task=$4; shift 4

case "$host:$repo" in
  win:kanesumi)  dir="/c/Users/mc158/Documents/Projects/Ether/shared/kanesumi" ;;
  win:ether)     dir="/c/Users/mc158/Documents/Projects/Ether" ;;
  arch:kanesumi) dir="/home/rain/projects/Arcturius/Kanesumi" ;;
  arch:ether)    dir="/home/rain/projects/Arcturius/Ether" ;;
  *) echo "未知目标 $host:$repo" >&2; exit 2 ;;
esac

accept=()
for cmd in "$@"; do accept+=(--accept "$cmd"); done

cd "$dir"
# 2026-10-14 修订：Arch 那一侧就是本机，不再有 `ssh ether` 这一跳（那台 Debian 上的
# 自研项目暂时跑不起来，已从目标里去掉）。
saiku run "$task" --id "$id" --worktree "dsh/$id" "${accept[@]}" --detach
