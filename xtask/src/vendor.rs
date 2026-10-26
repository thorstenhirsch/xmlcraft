//! Keeps the vendored, patched crates (vendor/README.md) in the build: an eframe upgrade that asked for a newer winit or egui-wgpu
//! than the vendored copy would make cargo resolve it from crates.io again, silently dropping the patches. The checks are tests,
//! so `cargo xtask test` and the CI gate run them.

/// The vendored crates and their directory names under `vendor/`.
#[cfg(test)]
const VENDORED: &[&str] = &["winit", "egui-wgpu"];

/// The stanza of `Cargo.lock` for the package `name`: from its `name = ` line to the next blank line.
#[cfg(test)]
fn lock_stanza<'a>(lock: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("name = \"{name}\"\n");
    let start = lock.find(&marker)?;
    let rest = &lock[start..];
    Some(rest.split("\n\n").next().unwrap_or(rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::root;

    fn read(path: &str) -> String {
        std::fs::read_to_string(root().join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
    }

    #[test]
    fn every_vendored_crate_has_its_sources_and_licence() {
        for name in VENDORED {
            let dir = root().join("vendor").join(name);
            assert!(dir.join("Cargo.toml").is_file(), "vendor/{name}/Cargo.toml");
            assert!(dir.join("src").is_dir(), "vendor/{name}/src");
            let has_licence =
                std::fs::read_dir(&dir).expect("vendored dir").filter_map(Result::ok).any(|e| e.file_name().to_string_lossy().starts_with("LICENSE"));
            assert!(has_licence, "vendor/{name} must keep the upstream licence files (AGENTS.md section 2)");
        }
    }

    #[test]
    fn vendored_crates_are_patched_in() {
        let manifest = read("Cargo.toml");
        let patch = manifest.split("[patch.crates-io]").nth(1).expect("the root manifest has a [patch.crates-io] table");
        for name in VENDORED {
            assert!(patch.contains(&format!("{name} = {{ path = \"vendor/{name}\" }}")), "{name} is not patched in from vendor/{name}");
        }
        // The README documents every vendored crate.
        let readme = read("vendor/README.md");
        for name in VENDORED {
            assert!(readme.contains(&format!("| {name} |")), "vendor/README.md has no row for {name}");
        }
    }

    /// A path dependency has no `source` in Cargo.lock; a crates.io one does. This is what fails when an upgrade makes cargo
    /// resolve a vendored crate from the registry again.
    #[test]
    fn the_build_resolves_the_vendored_copies_not_crates_io() {
        let lock = read("Cargo.lock");
        for name in VENDORED {
            let stanza =
                lock_stanza(&lock, name).unwrap_or_else(|| panic!("{name} is not in Cargo.lock: nothing depends on it, so its patch is unused"));
            assert!(!stanza.contains("source ="), "{name} is resolved from a registry, not vendor/{name}:\n{stanza}");
        }
    }

    #[test]
    fn the_stanza_reader_finds_the_right_package() {
        let lock = "[[package]]\nname = \"a\"\nversion = \"1\"\nsource = \"registry\"\n\n[[package]]\nname = \"b\"\nversion = \"2\"\n";
        assert!(lock_stanza(lock, "a").is_some_and(|s| s.contains("source =")));
        assert!(lock_stanza(lock, "b").is_some_and(|s| !s.contains("source =")));
        assert_eq!(lock_stanza(lock, "c"), None);
        assert_eq!(lock_stanza("", "a"), None);
    }
}
