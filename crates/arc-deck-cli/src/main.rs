//! Arc Deck headless worker 命令。
//!
//! 两个入口：
//!
//! - `arc-deck worker run ...` —— 公开命令，见 `args.rs` 的用法文本。
//! - `arc-deck __fixture-sdk ...` —— 隐藏子命令，把自己作为**假 SDK 对端**
//!   启动。它只在 `test-runtime` feature 下存在。
//!
//! 为什么假对端是「重新执行自身」而不是另一个二进制：文档里的离线验收只有
//! 一条命令（`cargo run -p arc-deck-cli --features test-runtime -- worker run ...`），
//! 而 `cargo run` 只保证构建被点到的那一个 bin。用隐藏子命令可以让这条命令
//! 真的能跑起来，不需要先手工构建第二个二进制。

mod args;
mod cancel;
mod commands;
mod report;
mod serve;
mod status;
mod tui;
mod worker;

use std::process::ExitCode;

use arc_deck_core::EXIT_SPEC_ERROR;

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let command = match args::parse(&argv) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            return ExitCode::from(EXIT_SPEC_ERROR as u8);
        }
    };

    match command {
        args::Command::Help => {
            print!("{}", args::USAGE);
            ExitCode::SUCCESS
        }
        args::Command::Version => {
            println!("arcdeck {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        args::Command::Tui => {
            ExitCode::from(tui::run().await as u8)
        }
        // 退出码都在 u8 范围内（0..=5、10、11 与 64）。
        args::Command::Ops(argv) => ExitCode::from(commands::dispatch(&argv).await as u8),
        args::Command::FixtureSdk { scenario } => fixture_entry(&scenario).await,
        args::Command::WorkerRun(worker_args) => {
            let code = worker::run(worker_args).await;
            // 退出码都在 u8 范围内（0..=5 与 64）。
            ExitCode::from(code as u8)
        }
    }
}

#[cfg(feature = "test-runtime")]
async fn fixture_entry(scenario: &str) -> ExitCode {
    let Some(scenario) = arc_deck_dsh::fixture::Scenario::parse(scenario) else {
        eprintln!(
            "未知夹具场景 `{scenario}`；可用：{}",
            arc_deck_dsh::fixture::Scenario::ALL.join(", ")
        );
        return ExitCode::from(2);
    };
    match arc_deck_dsh::fixture::serve(scenario, tokio::io::stdin(), tokio::io::stdout()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("夹具对端失败：{e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(feature = "test-runtime"))]
async fn fixture_entry(_scenario: &str) -> ExitCode {
    eprintln!("本构建没有启用 test-runtime feature，假 SDK 对端不可用");
    ExitCode::from(2)
}
