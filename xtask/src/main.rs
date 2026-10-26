#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! `cargo xtask <command>`: repository automation. The commands arrive with milestone M0.3 onwards.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("help" | "-h" | "--help") => {
            println!("cargo xtask <command>\n\nNo commands yet: version, layers, wasm, test, deny and ci arrive with M0.3.");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("xtask: unknown command `{other}` (try `cargo xtask help`)");
            ExitCode::from(2)
        }
    }
}
