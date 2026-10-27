//! Quality gates: layering, wasm, tests, dependency policy and the CI bundle.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, anyhow, bail};

use crate::layers;

/// The repository root (the parent of `xtask/`).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(mut cmd: Command, what: &str) -> anyhow::Result<()> {
    eprintln!("$ {what}");
    let status = cmd.status().with_context(|| format!("{what}: failed to spawn"))?;
    if !status.success() {
        bail!("{what}: exited with {status}");
    }
    Ok(())
}

fn run_args(args: &[&str]) -> anyhow::Result<()> {
    let mut c = cargo();
    c.args(args);
    run(c, &format!("cargo {}", args.join(" ")))
}

fn metadata() -> anyhow::Result<serde_json::Value> {
    let out = cargo().args(["metadata", "--format-version", "1", "--no-deps"]).output().context("cargo metadata")?;
    if !out.status.success() {
        bail!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

pub fn layers(_: &[String]) -> anyhow::Result<()> {
    let crates = layers::from_metadata(&metadata()?).map_err(|e| anyhow!(e))?;
    println!("Dependency layering (docs/architecture.md)\n");
    println!("{:<24} {:<14} workspace deps", "crate", "layer");
    for c in &crates {
        let ws: Vec<String> = c
            .deps
            .iter()
            .filter(|d| d.workspace)
            .map(|d| format!("{}{}", layers::short_name(&d.name), if d.kind == layers::DepKind::Dev { " (dev)" } else { "" }))
            .collect();
        println!("{:<24} {:<14} {}", c.name, layers::describe(layers::classify(&c.name)), ws.join(", "));
    }
    let violations = layers::check(&crates);
    println!();
    if violations.is_empty() {
        println!("OK: {} crates, no layering violations.", crates.len());
        return Ok(());
    }
    for v in &violations {
        println!("  - {v}");
    }
    bail!("{} layering violation(s)", violations.len())
}

/// `cargo check` for `wasm32-unknown-unknown` over every crate in L0–L7. Native-only code (files, network, threads) must sit
/// behind `cfg(not(target_arch = "wasm32"))`, so a web build stays possible.
pub fn wasm(_: &[String]) -> anyhow::Result<()> {
    let crates = layers::from_metadata(&metadata()?).map_err(|e| anyhow!(e))?;
    let set: Vec<String> = crates
        .into_iter()
        .filter(|c| matches!(layers::classify(&c.name), Some(layers::Class::Layer(_) | layers::Class::Standalone(_))))
        .map(|c| c.name)
        .collect();
    if set.is_empty() {
        println!("wasm32: no L0-L7 crates yet, nothing to check");
        return Ok(());
    }
    // One call over every crate: they share one dependency build, and `--keep-going` reports each failing crate instead of
    // stopping at the first.
    let mut c = cargo();
    c.args(["check", "--target", "wasm32-unknown-unknown", "--keep-going"]);
    for pkg in &set {
        c.args(["-p", pkg]);
    }
    run(c, &format!("cargo check --target wasm32-unknown-unknown --keep-going ({} crates)", set.len()))?;
    println!("wasm32: {} crates ok", set.len());
    Ok(())
}

/// The workspace tests. Unit and integration tests run under cargo-nextest when it is installed (one process per test, so the
/// run is not held up by its slowest binary), else under `cargo test`; doctests run separately because nextest does not run
/// them. Examples are left out of the test build: clippy `--all-targets` already checks that they compile.
pub fn test(_: &[String]) -> anyhow::Result<()> {
    // `--tests` covers library, binary and integration-test targets, and unlike `--lib` it is not an error in a workspace that
    // has no library crate yet.
    let targets = ["--workspace", "--tests"];
    let nextest = cargo().args(["nextest", "--version"]).output().is_ok_and(|o| o.status.success());
    if nextest {
        run_args(&[&["nextest", "run", "--no-fail-fast"][..], &targets].concat())?;
    } else {
        eprintln!("test: cargo-nextest is not installed, using cargo test (cargo install cargo-nextest --locked)");
        run_args(&[&["test", "--no-fail-fast"][..], &targets].concat())?;
    }
    // `cargo test --doc` is an error in a workspace without a library target, so run it only once there is one.
    if has_library_target(&metadata()?) {
        run_args(&["test", "--workspace", "--doc"])
    } else {
        eprintln!("test: no library crates yet, skipping doctests");
        Ok(())
    }
}

/// Whether any workspace package has a target that doctests can run on.
fn has_library_target(meta: &serde_json::Value) -> bool {
    let targets = meta["packages"].as_array().into_iter().flatten().flat_map(|p| p["targets"].as_array().into_iter().flatten());
    targets
        .flat_map(|t| t["kind"].as_array().into_iter().flatten())
        .filter_map(serde_json::Value::as_str)
        .any(|k| matches!(k, "lib" | "rlib" | "proc-macro"))
}

/// Dependency licences, bans, sources and advisories (`deny.toml`). Skipped with a hint when cargo-deny is not installed, except
/// under CI (`CI` set), where it is required.
pub fn deny(_: &[String]) -> anyhow::Result<()> {
    let installed = Command::new("cargo-deny").arg("--version").output().is_ok_and(|o| o.status.success());
    if !installed {
        if std::env::var_os("CI").is_some() {
            bail!("cargo-deny is required in CI: cargo install cargo-deny --locked");
        }
        eprintln!("deny: SKIPPED, cargo-deny is not installed (cargo install cargo-deny --locked)");
        return Ok(());
    }
    run_args(&["deny", "--log-level", "error", "check"])
}

/// The full gate, in order, stopping at the first failure. Steps are added here as the commands behind them land (the CLI
/// without default features, i18n-report and schemas).
pub fn ci(_: &[String]) -> anyhow::Result<()> {
    type Step = (&'static str, Box<dyn Fn() -> anyhow::Result<()>>);
    let steps: Vec<Step> = vec![
        ("fmt", Box::new(|| run_args(&["fmt", "--all", "--", "--check"]))),
        ("clippy", Box::new(|| run_args(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]))),
        ("test", Box::new(|| test(&[]))),
        ("layers", Box::new(|| layers(&[]))),
        ("wasm", Box::new(|| wasm(&[]))),
        ("assets", Box::new(|| crate::assets::run(&[]))),
        ("parity", Box::new(|| crate::parity::run(&[]))),
        // Skips with a notice unless the suite has been fetched (`cargo xtask conformance xmlconf --fetch`).
        ("conformance", Box::new(|| crate::conformance::run(&["xmlconf".to_owned()]))),
        ("deny", Box::new(|| deny(&[]))),
    ];
    for (i, (name, f)) in steps.iter().enumerate() {
        eprintln!("\n=== ci: {name} ===");
        if let Err(e) = f() {
            println!("\nCI: {i} passed, FAILED at `{name}`: {e:#}");
            bail!("ci failed at `{name}`");
        }
    }
    println!("\nCI: all {} steps passed", steps.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::has_library_target;
    use serde_json::json;

    #[test]
    fn doctests_need_a_library_target() {
        let only_bin = json!({"packages": [{"targets": [{"kind": ["bin"]}]}]});
        assert!(!has_library_target(&only_bin));
        let with_lib = json!({"packages": [{"targets": [{"kind": ["bin"]}]}, {"targets": [{"kind": ["lib"]}]}]});
        assert!(has_library_target(&with_lib));
        assert!(has_library_target(&json!({"packages": [{"targets": [{"kind": ["proc-macro"]}]}]})));
        assert!(!has_library_target(&json!({})));
        assert!(!has_library_target(&json!({"packages": [{}]})));
    }
}
