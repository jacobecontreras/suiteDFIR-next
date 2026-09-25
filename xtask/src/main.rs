//! Repository maintenance tasks, run as `cargo xtask <command>` (alias in `.cargo/config.toml`).
//!
//! The commands listed in DEVELOPMENT.md §2 are added by their roadmap tasks.

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <command>\n\nNo commands are implemented yet.";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("help" | "-h" | "--help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            ExitCode::from(2)
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
