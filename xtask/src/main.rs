#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! Project automation for XmlCraft, run via `cargo xtask <command>`.

use std::process::ExitCode;

mod assets;
mod bench;
mod conformance;
mod contributors;
mod fuzz;
mod gates;
mod layers;
mod parity;
mod schemas;
mod vendor;
mod version;
mod xsdtests;

type Command = fn(&[String]) -> anyhow::Result<()>;

/// Every subcommand: name, one-line summary, entry point. Commands for later milestones (schemas,
/// oracle, screenshots, i18n-report) are added with the milestone that needs them.
const COMMANDS: &[(&str, &str, Command)] = &[
    ("version", "Print the workspace version, or `version set X.Y.Z[-pre]` to change it and refresh Cargo.lock", version_cmd),
    ("layers", "Enforce the crate dependency layering (docs/architecture.md)", gates::layers),
    ("wasm", "cargo check --target wasm32-unknown-unknown for every crate below L8, in one call", gates::wasm),
    ("deny", "Dependency licences, bans, sources and advisories (deny.toml; needs cargo-deny)", gates::deny),
    ("ci", "fmt --check, clippy -D warnings, test, layers, wasm, assets, schemas, parity, deny (stops at first failure)", gates::ci),
    ("test", "Workspace tests: cargo nextest (cargo test when it is missing), then doctests", gates::test),
    ("assets", "Enforce the asset policy (AGENTS.md §1) against ATTRIBUTION.toml; --write regenerates ATTRIBUTION.md", assets::run),
    (
        "conformance",
        "Run a W3C conformance suite (xmlconf, xsdtests) against the engines and hold the ratchet: `conformance <suite> [--fetch] [--write] [-v]`",
        conformance::run,
    ),
    (
        "bench",
        "Measure the editor's performance targets on synthetic documents and hold the budgets: `bench [--only NAME] [--list] [--json] [--no-release]`",
        bench::run,
    ),
    (
        "fuzz",
        "Mutation-fuzz open/save/analyse/format/edit in child processes and keep the findings in fuzz-out/: `fuzz [--time 300] [--iterations N] [--seed 1] [--jobs N] [--timeout 10]`",
        fuzz::run,
    ),
    ("schemas", "Check the bundled schemas against schemas/manifest.toml and ATTRIBUTION.toml: `schemas [--fetch] [--update [SET…]]`", schemas::run),
    (
        "contributors",
        "The credits of the About window from the history: `contributors [--write] [--no-github]` (contributors/contributors.json)",
        contributors::run,
    ),
    ("parity", "Validate parity/xmlspy-features.toml against the registry, tools and tests; report progress (--json, --partial)", parity::run),
];

fn version_cmd(args: &[String]) -> anyhow::Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    version::run(&gates::root(), &args).map_err(anyhow::Error::msg)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = args.first() else {
        print_help();
        return ExitCode::FAILURE;
    };
    if matches!(name.as_str(), "help" | "-h" | "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }
    let Some((_, _, command)) = COMMANDS.iter().find(|(n, _, _)| n == name) else {
        eprintln!("xtask: unknown command `{name}`\n");
        print_help();
        return ExitCode::FAILURE;
    };
    match command(args.get(1..).unwrap_or_default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask {name}: error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    eprintln!("Usage: cargo xtask <command> [args]\n\nCommands:");
    for (name, about, _) in COMMANDS {
        eprintln!("  {name:<12} {about}");
    }
    eprintln!("  {:<12} Show this list", "help");
}
