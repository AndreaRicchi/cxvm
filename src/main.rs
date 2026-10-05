use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cxvm::cli::Cli::parse();
    match cxvm::cli::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(err.exit_code())
        }
    }
}
