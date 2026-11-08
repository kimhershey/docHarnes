//! DocuLens evaluation harness (design §3.11 평가·품질 관리, §4.1 P7).
//!
//! Three subcommands: `planner` (golden commands → plans, accuracy + regression
//! gate), `pipeline` (classification / OCR / KIE metrics over a golden manifest
//! with a real llama-server) and `scorecard` (merge results into the
//! quantisation scorecard). The design sketched this in Python; it lives in
//! the Rust workspace so one toolchain and one license gate cover it.

mod cli;
mod engine;
mod golden;
mod metrics;
mod pipeline;
mod planner;
mod scorecard;

use std::process::ExitCode;

fn usage() -> String {
    format!(
        "DocuLens 평가 하네스 (doculens-eval)\n\n사용법:\n  {}\n  {}\n  {}\n\n공통: --help\n종료 코드: 0 성공 · 1 회귀 게이트 실패 · 2 인자/입출력 오류\n자세한 설명: eval/README.md",
        planner::USAGE,
        pipeline::USAGE,
        scorecard::USAGE
    )
}

fn main() -> ExitCode {
    let args = cli::Args::from_env();
    let sub = args.subcommand();
    if sub.is_none() || (args.flag("help") && sub.is_none()) {
        println!("{}", usage());
        return ExitCode::from(if args.flag("help") { 0 } else { 2 });
    }
    let result = match sub {
        Some("planner") if args.flag("help") => {
            println!("{}", planner::USAGE);
            Ok(0)
        }
        Some("pipeline") if args.flag("help") => {
            println!("{}", pipeline::USAGE);
            Ok(0)
        }
        Some("scorecard") if args.flag("help") => {
            println!("{}", scorecard::USAGE);
            Ok(0)
        }
        Some("planner") => planner::run(&args),
        Some("pipeline") => pipeline::run(&args),
        Some("scorecard") => scorecard::run(&args),
        Some(other) => Err(format!("알 수 없는 하위 명령 {other:?}\n\n{}", usage())),
        None => unreachable!(),
    };
    match result {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("오류: {e}");
            ExitCode::from(2)
        }
    }
}
