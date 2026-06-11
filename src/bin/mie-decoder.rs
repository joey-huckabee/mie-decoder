#![cfg_attr(not(test), warn(clippy::expect_used, clippy::unwrap_used))]

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    mie_decoder::cli::run(argv)
}
