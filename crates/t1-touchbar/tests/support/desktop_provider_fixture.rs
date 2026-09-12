//! Process fixture for the versioned desktop-provider boundary.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

fn main() -> ExitCode {
    let arguments: Vec<_> = env::args_os().collect();
    if arguments.len() < 3 {
        return ExitCode::SUCCESS;
    }
    let invoked_path = PathBuf::from(&arguments[0]);
    let mode = invoked_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if mode.contains("timeout") {
        thread::sleep(Duration::from_secs(2));
        return ExitCode::SUCCESS;
    }
    if mode.contains("failed") {
        return ExitCode::FAILURE;
    }
    if arguments[1] == "v1" && arguments[2] == "status" {
        return status(mode);
    }
    let record = arguments[1..]
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    if fs::write(invoked_path.with_extension("log"), record).is_err() {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn status(mode: &str) -> ExitCode {
    if mode.contains("malformed") {
        println!("invalid");
    } else if mode.contains("oversized") {
        let bytes = [b'x'; 129];
        if io::stdout().write_all(&bytes).is_err() {
            return ExitCode::FAILURE;
        }
    } else if mode.contains("no-notification") {
        println!("T1BRIDGE-DESKTOP 1 3 42 0");
    } else if mode.contains("display-off") {
        println!("T1BRIDGE-DESKTOP 1 23 42 0 0");
    } else {
        println!("T1BRIDGE-DESKTOP 1 7 42 0");
    }
    ExitCode::SUCCESS
}
