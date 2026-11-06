//! Keeping the text of documents with unsaved changes on disk, so that a crash, a power cut or a kill does not lose it.
//!
//! A snapshot is one file in a folder the app owns: a header (what the document was called and where it was saved) and its text. A
//! snapshot is written atomically (a crash while writing leaves the older one whole), and removed when the document is saved or closed
//! or the app ends normally; what is left in the folder at the next start is what a session that did not end normally had unsaved.
//! Nothing here reads or changes the documents' own files.
//!
//! On the web there is no folder: every function does nothing and reports nothing found.

/// A snapshot of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// What names the file (`<run>-<document>`): unique to the document in the session that took it.
    pub key: String,
    /// What the document is called (its file name, or `Untitled-1`).
    pub name: String,
    /// The file it was read from and saved to, when it has one.
    pub path: Option<String>,
    pub text: String,
}

/// The most snapshots listed, and the most text one holds: more is not what a person was typing.
pub const MAX_SNAPSHOTS: usize = 50;
pub const MAX_TEXT_BYTES: usize = 32 * 1024 * 1024;

const MAGIC: &str = "XMLCRAFT-RECOVERY 1";
const EXTENSION: &str = "recover";

/// The text of a snapshot file.
fn encode(s: &Snapshot) -> String {
    let escape = |t: &str| t.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r");
    format!("{MAGIC}\nname: {}\npath: {}\n\n{}", escape(&s.name), s.path.as_deref().map(escape).unwrap_or_default(), s.text)
}

/// A snapshot from the text of its file (`key` is the file's name without its extension); none when it is not one.
fn decode(key: &str, text: &str) -> Option<Snapshot> {
    let (head, body) = text.split_once("\n\n")?;
    let mut lines = head.split('\n');
    if lines.next()? != MAGIC {
        return None;
    }
    let unescape = |t: &str| {
        let mut out = String::with_capacity(t.len());
        let mut chars = t.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => {}
            }
        }
        out
    };
    let name = unescape(lines.next()?.strip_prefix("name: ")?);
    let path = unescape(lines.next()?.strip_prefix("path: ")?);
    Some(Snapshot { key: key.to_owned(), name, path: (!path.is_empty()).then_some(path), text: body.to_owned() })
}

/// A key that cannot step out of the folder or name a file that is not ours.
fn key_is_safe(key: &str) -> bool {
    !key.is_empty() && key.len() <= 64 && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::fs;
    use std::io;
    use std::path::Path;

    use super::{EXTENSION, MAX_SNAPSHOTS, MAX_TEXT_BYTES, Snapshot, decode, encode, key_is_safe};

    /// Write `snapshot` in `dir` (made if it is not there).
    pub fn write(dir: &Path, snapshot: &Snapshot) -> io::Result<()> {
        if !key_is_safe(&snapshot.key) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "the key of a snapshot is letters, digits, - and _"));
        }
        if snapshot.text.len() > MAX_TEXT_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "the text is too large to keep"));
        }
        fs::create_dir_all(dir)?;
        crate::files::write_file(&dir.join(format!("{}.{EXTENSION}", snapshot.key)), encode(snapshot).as_bytes()).map(|_| ())
    }

    /// The snapshots in `dir`, oldest key first; files that are not snapshots (or are damaged) are left alone and not listed.
    pub fn list(dir: &Path) -> Vec<Snapshot> {
        let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
        let mut found: Vec<(String, std::path::PathBuf)> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == EXTENSION))
            .filter_map(|p| Some((p.file_stem()?.to_str()?.to_owned(), p)))
            .filter(|(key, _)| key_is_safe(key))
            .collect();
        found.sort();
        found
            .into_iter()
            .take(MAX_SNAPSHOTS)
            .filter_map(|(key, path)| {
                let bytes = fs::read(&path).ok().filter(|b| b.len() <= MAX_TEXT_BYTES + 4096)?;
                decode(&key, std::str::from_utf8(&bytes).ok()?)
            })
            .collect()
    }

    /// Remove the snapshot `key` (it is not an error that there is none).
    pub fn remove(dir: &Path, key: &str) {
        if key_is_safe(key) {
            let _ = fs::remove_file(dir.join(format!("{key}.{EXTENSION}")));
        }
    }

    /// Remove every snapshot whose key starts with `prefix` (the ones one session took).
    pub fn remove_with_prefix(dir: &Path, prefix: &str) {
        if !key_is_safe(prefix) {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else { return };
        for path in entries.filter_map(Result::ok).map(|e| e.path()) {
            let is_ours =
                path.extension().is_some_and(|x| x == EXTENSION) && path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.starts_with(prefix));
            if is_ours {
                let _ = fs::remove_file(path);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{list, remove, remove_with_prefix, write};

#[cfg(target_arch = "wasm32")]
pub fn write(_dir: &std::path::Path, _snapshot: &Snapshot) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

#[cfg(target_arch = "wasm32")]
pub fn list(_dir: &std::path::Path) -> Vec<Snapshot> {
    Vec::new()
}

#[cfg(target_arch = "wasm32")]
pub fn remove(_dir: &std::path::Path, _key: &str) {}

#[cfg(target_arch = "wasm32")]
pub fn remove_with_prefix(_dir: &std::path::Path, _prefix: &str) {}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn snapshot(key: &str, name: &str, path: Option<&str>, text: &str) -> Snapshot {
        Snapshot { key: key.to_owned(), name: name.to_owned(), path: path.map(str::to_owned), text: text.to_owned() }
    }

    #[test]
    fn a_snapshot_is_written_and_read_back_whole_with_odd_names_and_every_kind_of_text() {
        let dir = tempfile::tempdir().expect("dir");
        let odd = "line one\r\nline\ttwo\n\n\\n not an escape\n<?xml version=\"1.0\"?>\u{FEFF}é€😀\n\n";
        let a = snapshot("1a-1", "a\nb.xml", Some("/x/with\\back\nslash.xml"), odd);
        let b = snapshot("1a-2", "Untitled-1", None, "");
        write(dir.path(), &a).expect("writes a");
        write(dir.path(), &b).expect("writes b");
        assert_eq!(list(dir.path()), [a.clone(), b.clone()]);
        // Writing again replaces; removing removes (and removing what is not there is nothing).
        let a2 = snapshot("1a-1", "a.xml", None, "<a/>");
        write(dir.path(), &a2).expect("writes again");
        assert_eq!(list(dir.path()), [a2, b]);
        remove(dir.path(), "1a-1");
        remove(dir.path(), "1a-1");
        assert_eq!(list(dir.path()).len(), 1);
    }

    #[test]
    fn what_is_not_a_snapshot_or_is_damaged_is_not_listed_and_is_left_alone() {
        let dir = tempfile::tempdir().expect("dir");
        write(dir.path(), &snapshot("ok-1", "n", None, "t")).expect("writes");
        std::fs::write(dir.path().join("bad-1.recover"), "not a snapshot").expect("writes");
        std::fs::write(dir.path().join("bad-2.recover"), [0xFF, 0xFE, 0x00]).expect("writes");
        std::fs::write(dir.path().join("note.txt"), "x").expect("writes");
        std::fs::write(dir.path().join("..evil.recover"), "x").expect("writes");
        assert_eq!(list(dir.path()).iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), ["ok-1"]);
        assert!(dir.path().join("bad-1.recover").exists() && dir.path().join("note.txt").exists());
        assert!(list(&dir.path().join("nothing-here")).is_empty());
    }

    #[test]
    fn a_session_removes_the_snapshots_it_took_and_no_one_elses_and_keys_cannot_leave_the_folder() {
        let dir = tempfile::tempdir().expect("dir");
        for key in ["aaa-1", "aaa-2", "bbb-1"] {
            write(dir.path(), &snapshot(key, "n", None, "t")).expect("writes");
        }
        remove_with_prefix(dir.path(), "aaa");
        assert_eq!(list(dir.path()).iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), ["bbb-1"]);
        for bad in ["../x", "a/b", "", "a b", &"k".repeat(65)] {
            assert!(write(dir.path(), &snapshot(bad, "n", None, "t")).is_err(), "{bad:?}");
            remove(dir.path(), bad);
            remove_with_prefix(dir.path(), bad);
        }
        assert_eq!(list(dir.path()).len(), 1);
        let big = snapshot("big-1", "n", None, &"x".repeat(MAX_TEXT_BYTES + 1));
        assert!(write(dir.path(), &big).is_err(), "too large to keep");
    }
}
