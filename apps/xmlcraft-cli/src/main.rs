#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! `xmlcraft-cli`: the command line of XmlCraft. See `cli.rs` for what it does.

mod cli;

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match cli::parse(&args) {
        Ok(command) => command,
        Err(message) => {
            // `eprintln!` panics on a closed standard error; a script must not crash the tool that reports its mistake.
            let _ = writeln!(std::io::stderr(), "xmlcraft-cli: {message}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    ExitCode::from(cli::execute(command, &mut out, &mut err))
}
