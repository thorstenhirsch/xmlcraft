//! Keeps the patched window stack in the build: winit and the egui crates are the storytold forks at immutable revisions (the ones
//! PdfCraft builds against; see vendor/README.md). An eframe upgrade that asked for another winit or egui than the pinned fork would
//! make cargo resolve it from crates.io again, silently dropping the patches. The vendored copies of winit and egui-wgpu are
//! provenance for the patches XmlCraft carried before the forks. `xee-interpreter` is a path patch with the XPath NaN comparison fix. The checks are tests, so `cargo xtask test` and the CI gate run them.

/// The vendored crates and their directory names under `vendor/`.
#[cfg(test)]
const VENDORED: &[&str] = &["winit", "egui-wgpu", "xee-interpreter"];

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

    /// The crates `[patch.crates-io]` takes from the storytold forks, and the repository and revision each is pinned to.
    const FORKED: &[(&str, &str, &str)] = &[
        ("egui", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("egui-winit", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("egui-wgpu", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("eframe", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("ecolor", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("emath", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("epaint", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("epaint_default_fonts", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("egui_glow", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("egui_extras", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("egui_kittest", "https://github.com/storytold/egui.git", "6d518fc799681571963842f95b55a6b68431b15a"),
        ("winit", "https://github.com/storytold/winit.git", "a517b3846ee8457d84ee4975110bfda6797c44dc"),
    ];

    #[test]
    fn the_window_stack_is_patched_in_from_the_pinned_forks() {
        let manifest = read("Cargo.toml");
        let patch = manifest.split("[patch.crates-io]").nth(1).expect("the root manifest has a [patch.crates-io] table");
        for (name, repository, revision) in FORKED {
            let line = format!("{name} = {{ git = \"{repository}\", rev = \"{revision}\" }}");
            assert!(patch.contains(&line), "{name} is not patched in from {repository} at {revision}");
        }
        // The README documents every retained vendored copy.
        let readme = read("vendor/README.md");
        for name in VENDORED {
            assert!(readme.contains(&format!("| {name} |")), "vendor/README.md has no row for {name}");
        }
    }

    /// What `Cargo.lock` resolved: the fork at the pinned revision, never the registry. This is what fails when an upgrade makes cargo
    /// resolve one of them from crates.io again.
    #[test]
    fn the_build_resolves_the_forks_not_crates_io() {
        let lock = read("Cargo.lock");
        for (name, repository, revision) in FORKED {
            let stanza =
                lock_stanza(&lock, name).unwrap_or_else(|| panic!("{name} is not in Cargo.lock: nothing depends on it, so its patch is unused"));
            let source = format!("source = \"git+{repository}?rev={revision}#{revision}\"");
            assert!(stanza.contains(&source), "{name} is not resolved from {repository} at {revision}:\n{stanza}");
        }
    }

    /// The XPath engine is the vendored copy with the NaN comparison fix (vendor/README.md), not the registry crate.
    #[test]
    fn the_xpath_interpreter_is_the_vendored_copy() {
        let manifest = read("Cargo.toml");
        let patch = manifest.split("[patch.crates-io]").nth(1).expect("the root manifest has a [patch.crates-io] table");
        assert!(patch.contains("xee-interpreter = { path = \"vendor/xee-interpreter\" }"), "xee-interpreter is not patched in from vendor/");
        let lock = read("Cargo.lock");
        let stanza = lock_stanza(&lock, "xee-interpreter").expect("xee-interpreter is in Cargo.lock");
        assert!(!stanza.contains("source ="), "xee-interpreter resolves from a registry, not vendor/:\n{stanza}");
        assert!(read("vendor/xee-interpreter/src/atomic/op_gt.rs").contains("XmlCraft patch"), "the vendored op_gt carries the patch");
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
