//! 命令行解析。
//!
//! 手写而不是引第三方解析器：公开面只有两条命令、六个开关，而每个开关的
//! 缺席行为都要**明确**（没有默认 runtime、没有默认 workspace）——
//! 用通用解析器的默认值反而更容易把「没给」悄悄变成「用了一个默认」。

use std::path::PathBuf;

use arc_deck_core::error::{code, SpecError, SpecResult};

/// 任务书来源。`-` 表示从 stdin 读。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskSource {
    File(PathBuf),
    Stdin,
}

/// 已解析的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    WorkerRun(WorkerArgs),
    /// 高层命令（run / status / wait / report / logs / cancel / usage / ls / auth / runtime / config），
    /// 原样转给 `commands.rs` 自己解析。
    Ops(Vec<String>),
    /// 无参数：交互界面。
    Tui,
    /// 隐藏子命令：把自己作为假 SDK 对端启动。只在 `test-runtime` 下可达。
    FixtureSdk { scenario: String },
    Help,
    Version,
}

/// `worker run` 的参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerArgs {
    pub config: PathBuf,
    pub task: TaskSource,
    pub format: String,
    pub result: Option<PathBuf>,
    pub cancel_file: Option<PathBuf>,
}

pub const USAGE: &str = "\
arcdeck —— Arc Deck（DSH 之上的编码 Agent）

  arcdeck                               当前目录进 TUI（独立运行、历史、工具进度、取消）
  arcdeck run <任务书.md|.json|-> [选项] 执行一件任务（前台：stdout 是 JSONL，退出码 = 终态）
      --detach                          分离运行；等工人接住（写出状态）才返回，接不住就报出原因
      --prompt <原文|->                 明确作为指令；- 从 stdin 读文本（不按任务书解析）
      --id <ID>                         任务 ID（缺省取任务书文件名）
      --by <谁>                         谁派的这件任务（claude / sol…），只记进状态，不进授权
      --workspace <目录>                缺省为当前目录
      --permission <级别>               readonly | workspace | full（一次划定，运行中不再改）
      --worktree <分支>                 在 workspace 仓上建/复用 git worktree 再跑
      --accept <命令>                   验收命令，可重复；按实际执行记录判定
      --preamble <文件>                 工人守则，进 system prompt
      --model <provider/model>          覆盖配置里的模型
      --effort <级别>                   模型强度（协议里叫 reasoningEffort；opencode 叫 variant，不用那个名）
      --format <jsonl|text>             stdout 形态：工人模式 jsonl，一般模式 text
      --timeout <90|30m|2h>             墙钟预算
      --rerun                           同 ID 已有结论时重跑（旧记录改名保留）
  arcdeck status [<id>…|--all] [--json] 每件任务一行；单件时退出码编码状态
  arcdeck wait <id> [--timeout 30m]     等到终态，打印一行
  arcdeck report <id> [--json]          紧凑回报（事实与模型陈述分开）
  arcdeck usage [--json]                各任务的 token 用量与合计（只有 provider 报告的，无费用）
  arcdeck logs <id> [--tail N] [--runtime|--console]
  arcdeck cancel <id>                   请求取消并等待收尾；监工已死时按 runtime_pid 收进程树
  arcdeck ls                            同 status --all
  arcdeck auth set|ls|rm|import-opencode         凭据（与 OpenCode auth.json 同形）
  arcdeck runtime install|status [--version V]   私有、钉版本的 DSH 运行时
  arcdeck config init|path              用户配置 arcdeck.json

status / wait 的退出码：
  0 完成 · 1 失败 · 2 受阻 · 3 已取消 · 4 超时 · 5 未知（含进程已死）· 10 运行中 · 11 卡住 · 64 用法错

底层入口（M1 契约，参数不变）：
  arcdeck worker run --config <配置文件> --task <任务书|-> [选项]

选项：
  --config <路径>       必需的启动配置（授权上限、运行时、状态目录）
  --task <路径|->       任务书；`-` 表示从 stdin 读一份 JSON
  --format <jsonl|text> 事件输出格式（缺省 jsonl）。text = 一般模式：stdout 给人看
  --result <路径>       把最终结果同时写到这里（完整写入后发布）
  --cancel-file <路径>  观察该文件请求取消；启动时必须不存在

退出码：
  0 完成 · 1 失败 · 2 受阻 · 3 已取消 · 4 超时 · 5 结果未知 · 64 任务书或配置无效
";

/// 解析参数（不含程序名）。
pub fn parse(argv: &[String]) -> SpecResult<Command> {
    let Some(first) = argv.first() else {
        return Ok(Command::Tui);
    };
    match first.as_str() {
        "run" | "status" | "wait" | "report" | "logs" | "cancel" | "usage" | "ls" | "auth" | "runtime" | "config" => {
            Ok(Command::Ops(argv.to_vec()))
        }
        "--help" | "-h" | "help" => Ok(Command::Help),
        "--version" | "-V" => Ok(Command::Version),
        "__fixture-sdk" => parse_fixture(&argv[1..]),
        "worker" => parse_worker(&argv[1..]),
        other => Err(SpecError::new(
            code::CONFIG_INVALID,
            format!("未知命令 `{other}`；用 --help 看用法"),
        )),
    }
}

fn parse_fixture(rest: &[String]) -> SpecResult<Command> {
    let mut scenario = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--scenario" => {
                i += 1;
                scenario = Some(value_at(rest, i, "--scenario")?.to_string());
            }
            other => {
                return Err(SpecError::new(
                    code::CONFIG_INVALID,
                    format!("__fixture-sdk 不接受参数 `{other}`"),
                ))
            }
        }
        i += 1;
    }
    let scenario = scenario.ok_or_else(|| {
        SpecError::new(code::CONFIG_INVALID, "__fixture-sdk 需要 --scenario")
    })?;
    Ok(Command::FixtureSdk { scenario })
}

fn parse_worker(rest: &[String]) -> SpecResult<Command> {
    let Some(sub) = rest.first() else {
        return Err(SpecError::new(code::CONFIG_INVALID, "worker 需要一个子命令（run）"));
    };
    if sub != "run" {
        return Err(SpecError::new(
            code::CONFIG_INVALID,
            format!("worker 只支持 run，得到 `{sub}`"),
        ));
    }

    let mut config: Option<PathBuf> = None;
    let mut task: Option<TaskSource> = None;
    let mut format = "jsonl".to_string();
    let mut result: Option<PathBuf> = None;
    let mut cancel_file: Option<PathBuf> = None;

    let mut i = 1;
    while i < rest.len() {
        match rest[i].as_str() {
            "--config" => {
                i += 1;
                config = Some(PathBuf::from(value_at(rest, i, "--config")?));
            }
            "--task" => {
                i += 1;
                let raw = value_at(rest, i, "--task")?;
                task = Some(if raw == "-" { TaskSource::Stdin } else { TaskSource::File(PathBuf::from(raw)) });
            }
            "--format" => {
                i += 1;
                format = value_at(rest, i, "--format")?.to_string();
            }
            "--result" => {
                i += 1;
                result = Some(PathBuf::from(value_at(rest, i, "--result")?));
            }
            "--cancel-file" => {
                i += 1;
                cancel_file = Some(PathBuf::from(value_at(rest, i, "--cancel-file")?));
            }
            other => {
                return Err(SpecError::new(
                    code::CONFIG_INVALID,
                    format!("worker run 不认识参数 `{other}`"),
                ))
            }
        }
        i += 1;
    }

    if crate::report::SinkFormat::parse(&format).is_none() {
        return Err(SpecError::new(
            code::CONFIG_INVALID,
            format!("--format 只支持 jsonl 与 text，得到 `{format}`"),
        ));
    }

    Ok(Command::WorkerRun(WorkerArgs {
        config: config.ok_or_else(|| {
            SpecError::new(code::CONFIG_MISSING, "缺少 --config；启动配置没有默认值")
        })?,
        task: task.ok_or_else(|| {
            SpecError::new(code::TASK_UNREADABLE, "缺少 --task；任务书没有默认值")
        })?,
        format,
        result,
        cancel_file,
    }))
}

fn value_at<'a>(rest: &'a [String], index: usize, flag: &str) -> SpecResult<&'a str> {
    rest.get(index).map(String::as_str).ok_or_else(|| {
        SpecError::new(code::CONFIG_INVALID, format!("`{flag}` 后面缺少取值"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn worker_run_minimal() {
        let cmd = parse(&argv(&["worker", "run", "--config", "c.json", "--task", "t.json"])).unwrap();
        let Command::WorkerRun(args) = cmd else { panic!("期望 worker run") };
        assert_eq!(args.config, PathBuf::from("c.json"));
        assert_eq!(args.task, TaskSource::File(PathBuf::from("t.json")));
        assert_eq!(args.format, "jsonl");
        assert!(args.result.is_none());
        assert!(args.cancel_file.is_none());
    }

    #[test]
    fn dash_task_means_stdin() {
        let cmd = parse(&argv(&["worker", "run", "--config", "c.json", "--task", "-"])).unwrap();
        let Command::WorkerRun(args) = cmd else { panic!() };
        assert_eq!(args.task, TaskSource::Stdin);
    }

    #[test]
    fn config_is_required() {
        let err = parse(&argv(&["worker", "run", "--task", "t.json"])).unwrap_err();
        assert!(err.is(code::CONFIG_MISSING), "{err}");
    }

    #[test]
    fn task_is_required() {
        let err = parse(&argv(&["worker", "run", "--config", "c.json"])).unwrap_err();
        assert!(err.is(code::TASK_UNREADABLE), "{err}");
    }

    #[test]
    fn dangling_flag_is_rejected() {
        let err = parse(&argv(&["worker", "run", "--config"])).unwrap_err();
        assert!(err.is(code::CONFIG_INVALID), "{err}");
    }

    #[test]
    fn unknown_format_is_rejected() {
        let err = parse(&argv(&["worker", "run", "--config", "c", "--task", "t", "--format", "xml"]))
            .unwrap_err();
        assert!(err.is(code::CONFIG_INVALID), "{err}");
    }

    #[test]
    fn unknown_command_is_rejected() {
        let err = parse(&argv(&["daemon"])).unwrap_err();
        assert!(err.is(code::CONFIG_INVALID), "{err}");
    }

    #[test]
    fn fixture_subcommand_parses_scenario() {
        let cmd = parse(&argv(&["__fixture-sdk", "--scenario", "success"])).unwrap();
        assert_eq!(cmd, Command::FixtureSdk { scenario: "success".into() });
    }
}
