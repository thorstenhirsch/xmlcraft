//! Reading the external parts of a DTD from files: the system identifier of the external subset or of a parameter entity is a path
//! relative to the text that has it, or a `file:` URI. Nothing is downloaded.

use std::path::{Path, PathBuf};

use xmlcraft_dtd::{LoadRequest, Loaded, SubsetLoader};
use xmlcraft_text::encoding::{decode, sniff};

use crate::files;

/// The largest external text read.
pub const MAX_EXTERNAL_BYTES: u64 = 16 * 1024 * 1024;

/// The path a `file:` URI names, or `None` for any other URI, for one with an authority that is not this machine, and for a path that
/// is not valid text. The query and the fragment are not part of the path; the percent-escapes of the rest are decoded.
pub fn path_of(uri: &str) -> Option<String> {
    let rest = uri.get(..5).filter(|scheme| scheme.eq_ignore_ascii_case("file:")).map(|_| &uri[5..])?;
    let path = match rest.strip_prefix("//") {
        Some(after) => {
            let end = after.find(['/', '?', '#']).unwrap_or(after.len());
            let (authority, path) = after.split_at(end);
            if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
                return None;
            }
            path
        }
        None => rest,
    };
    let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        let escaped = (byte == b'%')
            .then(|| bytes.get(i + 1..i + 3).and_then(|hex| std::str::from_utf8(hex).ok()).and_then(|hex| u8::from_str_radix(hex, 16).ok()))
            .flatten();
        match escaped {
            Some(value) => {
                out.push(value);
                i += 3;
            }
            None => {
                out.push(byte);
                i += 1;
            }
        }
    }
    let path = String::from_utf8(out).ok()?;
    // `/C:/dir/file` is a Windows path.
    let windows = path.as_bytes().get(2) == Some(&b':') && path.starts_with('/');
    Some(if windows { path[1..].to_owned() } else { path })
}

/// Whether a system identifier starts with a URI scheme (`http:`, `ftp:`, …): it names no file. (A single letter before the colon is a
/// drive letter of Windows.)
pub fn has_scheme(system: &str) -> bool {
    let Some((scheme, _)) = system.split_once(':') else { return false };
    let mut chars = scheme.chars();
    scheme.len() > 1
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// What turns a path into the file to read (or refuses it): the check of a confinement, or nothing.
type Resolve = Box<dyn Fn(&str) -> Result<PathBuf, String> + Send>;

/// Reads the external texts of a DTD from files and keeps what it read, to say where the problems in them are.
pub struct FileSubsetLoader {
    resolve: Resolve,
    read: Vec<(String, String)>,
}

impl FileSubsetLoader {
    /// Any file the person can read.
    pub fn unconfined() -> Self {
        Self::confined(|path| Ok(PathBuf::from(path)))
    }

    /// Files that `resolve` lets through (it may refuse a path, or change it to the real one).
    pub fn confined(resolve: impl Fn(&str) -> Result<PathBuf, String> + Send + 'static) -> Self {
        Self { resolve: Box::new(resolve), read: Vec::new() }
    }

    /// The texts that were read, as (where from, text).
    pub fn texts(&self) -> &[(String, String)] {
        &self.read
    }
}

impl SubsetLoader for FileSubsetLoader {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        let system = request.system;
        // A file: URI is a path; any other scheme is not read.
        let wanted: PathBuf = if let Some(path) = path_of(system) {
            PathBuf::from(path)
        } else if has_scheme(system) {
            return Err("only files are read, nothing is downloaded".to_owned());
        } else {
            let base = path_of(request.base).unwrap_or_else(|| request.base.to_owned());
            Path::new(&base).parent().unwrap_or(Path::new("")).join(system)
        };
        let path = (self.resolve)(&wanted.to_string_lossy())?;
        // The size first: a file over the limit is not read into memory only to be refused.
        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        if size > MAX_EXTERNAL_BYTES {
            return Err(format!("over {MAX_EXTERNAL_BYTES} bytes"));
        }
        let loaded = files::read_file(&path).map_err(|e| e.to_string())?;
        let decoded = decode(&loaded.bytes, &sniff(&loaded.bytes));
        if decoded.lossy {
            return Err("it has bytes that are not valid in its encoding".to_owned());
        }
        let location = path.to_string_lossy().into_owned();
        self.read.push((location.clone(), decoded.text.clone()));
        Ok(Loaded { text: decoded.text, location })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_are_paths_and_other_schemes_are_not() {
        assert_eq!(path_of("file:///etc/xml/cat%20alog.xml").as_deref(), Some("/etc/xml/cat alog.xml"));
        assert_eq!(path_of("file:///C:/dir/x.xml").as_deref(), Some("C:/dir/x.xml"));
        assert_eq!(path_of("FILE://localhost/a/b").as_deref(), Some("/a/b"));
        assert_eq!(path_of("file://other-host/a/b"), None);
        assert_eq!(path_of("http://x/y"), None);
        assert_eq!(path_of("é"), None, "no panic on a character that straddles the scheme length");
        for yes in ["http://x/y.dtd", "ftp:y", "x+y-z.1:a", "HTTP://X"] {
            assert!(has_scheme(yes), "{yes}");
        }
        for no in ["y.dtd", "dir/y.dtd", "C:\\dir\\y.dtd", "C:/dir/y.dtd", "a/b:c", ":a", "1x:a", ""] {
            assert!(!has_scheme(no), "{no}");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_relative_identifier_is_taken_from_the_folder_of_the_text_that_has_it() {
        let dir = tempfile::tempdir().expect("dir");
        std::fs::create_dir_all(dir.path().join("sub")).expect("folder");
        std::fs::write(dir.path().join("sub/a.dtd"), "<!ELEMENT a EMPTY>").expect("writes");
        std::fs::write(dir.path().join("b.dtd"), b"<!ELEMENT b EMPTY> \xFF").expect("writes");
        let base = dir.path().join("sub/doc.xml");
        let mut loader = FileSubsetLoader::unconfined();
        let ask = |loader: &mut FileSubsetLoader, system: &str, base: &Path| {
            loader.load(&LoadRequest { kind: "external subset", public: None, system, base: &base.to_string_lossy() })
        };
        assert_eq!(ask(&mut loader, "a.dtd", &base).map(|l| l.text), Ok("<!ELEMENT a EMPTY>".to_owned()));
        assert_eq!(ask(&mut loader, "../sub/a.dtd", &base).map(|l| l.text), Ok("<!ELEMENT a EMPTY>".to_owned()));
        assert!(ask(&mut loader, "../b.dtd", &base).is_err(), "not valid in its encoding");
        assert!(ask(&mut loader, "http://example.invalid/a.dtd", &base).is_err());
        assert!(ask(&mut loader, "missing.dtd", &base).is_err());
        assert_eq!(loader.texts().len(), 2);
        // A resolver can refuse a path.
        let mut refusing = FileSubsetLoader::confined(|_| Err("not allowed".to_owned()));
        assert_eq!(ask(&mut refusing, "a.dtd", &base).err().as_deref(), Some("not allowed"));
    }
}
