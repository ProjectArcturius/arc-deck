#!/usr/bin/env bash
# ad.sh —— 用 arcdeck 派一件 Ether 工人任务（取代 tools/dispatch/dsh.sh + dsh-runner.sh）。
#
# 用法: ad.sh <win|arch> <ether|kanesumi> <id> <task.md> [验收命令 …]
#   立即返回；之后只用一行命令跟踪：
#     arcdeck status <id>            （Debian：ssh ether arcdeck status <id>）
#     arcdeck wait <id> --timeout 2h
#     arcdeck report <id>
#
# 与 dsh-runner.sh 的对应（参 arc-deck docs/PRODUCT_PLAN.md §6.6）：
#   worktree + 分支 dsh/<id>、子模块初始化、CARGO_TARGET_DIR 每任务独占、PREAMBLE 进 system prompt、
#   卡死看门狗与续跑、后台化（不再需要 nohup）、收尾时的 git log / git status —— 都由 arcdeck 做。
#   沙箱用 danger-full-access：worktree 里提交要写主仓的 .git/worktrees/<id>，workspace-write 会拒
#   （arc-deck 2026-10-14 实测）。隔离由 worktree 本身承担，与原 runner 一致。
#
# 不做：代理与反向隧道（那是调度者那端的网络，仍按 DISPATCHER §三 建好再派）。
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

if [ "$host" = win ]; then
  cd "$dir"
  arcdeck run "$task" --id "$id" --worktree "dsh/$id" "${accept[@]}" --detach
else
  base="$(dirname "$dir")/.oc"
  ssh ether "mkdir -p $base/tasks"
  # 发出的副本去掉 CR（同 dsh.sh：Windows 改写过的文件会变 CRLF）。
  tr -d '\015' <"$task" | ssh ether "cat > $base/tasks/$id.md"
  q=""; for a in "${accept[@]}"; do q+=" $(printf '%q' "$a")"; done
  ssh ether "cd $dir && arcdeck run $base/tasks/$id.md --id $id --worktree dsh/$id$q --detach"
fi
