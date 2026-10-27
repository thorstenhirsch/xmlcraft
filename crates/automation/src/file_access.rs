//! `--root` confinement: every path a tool reads or writes goes through here.
//!
//! With a root set, a path must stay inside it however it is spelled: a relative path is taken from the root, `..` may not climb
//! out of it, an absolute path must already lie inside it, and a symbolic link may not lead out of it (the check is made on the
//! resolved path, so a link inside the root that points outside does not open a door). Without a root nothing is confined: that
//! is the CLI used by a person at their own terminal; agent-facing entry points (the MCP server) are meant to be started with one.

use std::path::{Component, Path, PathBuf};

use crate::ToolError;

/// The folder tools are confined to, if any.
#[derive(Clone, Debug, Default)]
pub struct Confinement {
    root: Option<PathBuf>,
}

impl Confinement {
    /// No confinement.
    pub fn none() -> Self {
        Self { root: None }
    }

    /// Confine to `root`, which must be an existing folder. It is resolved once, so a link in the root's own path is followed here and
    /// not again.
    pub fn new(root: &Path) -> Result<Self, ToolError> {
        let root = root.canonicalize().map_err(|e| ToolError::Failed(format!("the root {} cannot be used: {e}", root.display())))?;
        if !root.is_dir() {
            return Err(ToolError::Failed(format!("the root {} is not a folder", root.display())));
        }
        Ok(Self { root: Some(root) })
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The path to read: it must exist (and, with a root, resolve to a place inside it).
    pub fn resolve_read(&self, path: &str) -> Result<PathBuf, ToolError> {
        let candidate = self.candidate(path)?;
        let Some(root) = &self.root else { return Ok(candidate) };
        let real = candidate.canonicalize().map_err(|e| ToolError::Failed(format!("{path}: {e}")))?;
        self.inside(root, &real, path).map(|()| real)
    }

    /// The path to write: it may not exist yet, but the deepest part that does must resolve to a place inside the root.
    pub fn resolve_write(&self, path: &str) -> Result<PathBuf, ToolError> {
        let candidate = self.candidate(path)?;
        let Some(root) = &self.root else { return Ok(candidate) };
        // Resolve the deepest ancestor that exists, then put the not-yet-existing tail back on.
        let mut existing = candidate.as_path();
        let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
        let real_base = loop {
            match existing.canonicalize() {
                Ok(real) => break real,
                Err(_) => match (existing.file_name(), existing.parent()) {
                    (Some(name), Some(parent)) => {
                        tail.push(name);
                        existing = parent;
                    }
                    _ => return Err(ToolError::Failed(format!("{path}: no part of this path exists"))),
                },
            }
        };
        let mut real = real_base;
        for part in tail.iter().rev() {
            real.push(part);
        }
        self.inside(root, &real, path).map(|()| real)
    }

    /// The lexically normalised path, joined to the root when relative. Refuses an empty path, a NUL byte, and (with a root) a path
    /// whose `..` climbs above the root.
    fn candidate(&self, path: &str) -> Result<PathBuf, ToolError> {
        if path.is_empty() {
            return Err(ToolError::InvalidArgs("the path is empty".into()));
        }
        if path.contains('\0') {
            return Err(ToolError::InvalidArgs("the path contains a NUL byte".into()));
        }
        let given = Path::new(path);
        let joined = match (&self.root, given.is_absolute()) {
            (Some(root), false) => root.join(given),
            _ => given.to_path_buf(),
        };
        let mut out = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::ParentDir => {
                    if !out.pop() {
                        return Err(ToolError::Failed(format!("{path}: the path leaves the root")));
                    }
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        if let Some(root) = &self.root {
            // Lexical check first for a relative path, so one that climbs out is refused without touching the disk. An absolute path
            // is judged by where it resolves to (`inside`): `/var/x` and `/private/var/x` are the same place on macOS.
            if !given.is_absolute() && !out.starts_with(root) {
                return Err(ToolError::Failed(format!("{path}: the path is outside the root {}", root.display())));
            }
        }
        Ok(out)
    }

    fn inside(&self, root: &Path, real: &Path, path: &str) -> Result<(), ToolError> {
        if real.starts_with(root) {
            Ok(())
        } else {
            Err(ToolError::Failed(format!("{path}: the path resolves to a place outside the root {}", root.display())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jail() -> (tempfile::TempDir, Confinement) {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(dir.path().join("sub")).expect("sub");
        std::fs::write(dir.path().join("a.xml"), "<a/>").expect("file");
        let c = Confinement::new(dir.path()).expect("confinement");
        (dir, c)
    }

    #[test]
    fn without_a_root_nothing_is_confined() {
        let c = Confinement::none();
        let elsewhere = tempfile::tempdir().expect("temp dir");
        let file = elsewhere.path().join("x.xml");
        std::fs::write(&file, "<x/>").expect("file");
        assert_eq!(c.resolve_read(file.to_str().expect("utf-8")).expect("anywhere"), file);
        assert!(c.root().is_none());
        assert!(c.resolve_write("relative/new.xml").is_ok());
    }

    #[test]
    fn relative_paths_are_taken_from_the_root() {
        let (dir, c) = jail();
        let real = dir.path().canonicalize().expect("canonical");
        assert_eq!(c.resolve_read("a.xml").expect("inside"), real.join("a.xml"));
        assert_eq!(c.resolve_read("./sub/../a.xml").expect("inside"), real.join("a.xml"));
        assert_eq!(c.resolve_read(real.join("a.xml").to_str().expect("utf-8")).expect("absolute inside"), real.join("a.xml"));
    }

    #[test]
    fn dot_dot_cannot_climb_out() {
        let (_dir, c) = jail();
        for path in ["../a.xml", "sub/../../a.xml", "../../../../etc/passwd", ".."] {
            let err = c.resolve_read(path).expect_err(path).to_string();
            assert!(err.contains("outside the root") || err.contains("leaves the root"), "{path}: {err}");
            assert!(c.resolve_write(path).is_err(), "{path}");
        }
    }

    #[test]
    fn absolute_paths_outside_are_refused() {
        let (dir, c) = jail();
        let outside = tempfile::tempdir().expect("other dir");
        std::fs::write(outside.path().join("x.xml"), "").expect("file");
        for p in [outside.path().join("x.xml"), PathBuf::from(if cfg!(windows) { "C:\\Windows\\win.ini" } else { "/etc/hosts" })] {
            let p = p.to_str().expect("utf-8");
            assert!(c.resolve_read(p).is_err(), "{p}");
            assert!(c.resolve_write(p).is_err(), "{p}");
        }
        drop(dir);
    }

    #[test]
    fn a_write_may_name_a_file_that_does_not_exist_yet() {
        let (dir, c) = jail();
        let real = dir.path().canonicalize().expect("canonical");
        assert_eq!(c.resolve_write("new.xml").expect("inside"), real.join("new.xml"));
        assert_eq!(c.resolve_write("sub/deeper/new.xml").expect("inside"), real.join("sub").join("deeper").join("new.xml"));
        assert!(c.resolve_read("new.xml").is_err(), "but a read needs it to exist");
    }

    #[test]
    fn empty_and_nul_paths_are_refused() {
        let (_dir, c) = jail();
        assert!(matches!(c.resolve_read(""), Err(ToolError::InvalidArgs(_))));
        assert!(matches!(c.resolve_write("a\0b"), Err(ToolError::InvalidArgs(_))));
        assert!(matches!(Confinement::none().resolve_read(""), Err(ToolError::InvalidArgs(_))));
    }

    #[test]
    fn the_root_must_be_an_existing_folder() {
        let (dir, _c) = jail();
        assert!(Confinement::new(&dir.path().join("missing")).is_err());
        assert!(Confinement::new(&dir.path().join("a.xml")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_root_cannot_lead_out_of_it() {
        let (dir, c) = jail();
        let outside = tempfile::tempdir().expect("other dir");
        std::fs::write(outside.path().join("secret.xml"), "<s/>").expect("secret");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("door")).expect("link to a folder");
        std::os::unix::fs::symlink(outside.path().join("secret.xml"), dir.path().join("file-door")).expect("link to a file");
        for path in ["door/secret.xml", "file-door"] {
            let err = c.resolve_read(path).expect_err(path).to_string();
            assert!(err.contains("outside the root"), "{path}: {err}");
        }
        // Writing through the linked folder, even to a file that does not exist yet, is refused too.
        assert!(c.resolve_write("door/new.xml").is_err());
        assert!(c.resolve_write("door/deeper/new.xml").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_that_stays_inside_is_fine() {
        let (dir, c) = jail();
        std::os::unix::fs::symlink(dir.path().join("sub"), dir.path().join("alias")).expect("link");
        std::fs::write(dir.path().join("sub").join("in.xml"), "").expect("file");
        assert!(c.resolve_read("alias/in.xml").is_ok());
    }
}
