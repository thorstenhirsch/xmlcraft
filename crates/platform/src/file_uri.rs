//! `file:` URIs: the path one names.

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

/// The `file:` URI of the file at `path` (a path with a scheme, such as an `http:` URI, is returned as it is). Characters a URI cannot
/// hold as they are, such as spaces and non-ASCII letters, are percent-escaped; on Windows backslashes become slashes and a drive path
/// gets its leading slash (`C:\x` is `file:///C:/x`), elsewhere a backslash is a character of the name and is escaped.
pub fn uri_of(path: &str) -> String {
    let has_scheme =
        path.split_once(':').is_some_and(|(scheme, _)| scheme.len() > 1 && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)));
    if has_scheme {
        return path.to_owned();
    }
    // A backslash separates folders only on Windows; elsewhere it is a character of the name (and is escaped below).
    let slashed = if cfg!(windows) { path.replace('\\', "/") } else { path.to_owned() };
    let rooted = if slashed.starts_with('/') { slashed } else { format!("/{slashed}") };
    let mut out = String::from("file://");
    for b in rooted.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_becomes_a_file_uri_that_names_it_again() {
        let mut paths = vec!["/tmp/a b/ä#c.xml", "/x/y.xml"];
        if cfg!(windows) {
            paths.push("C:\\dir\\f.xml");
        }
        for path in paths {
            let uri = uri_of(path);
            assert!(uri.starts_with("file:///"), "{uri}");
            assert!(uri.is_ascii() && !uri.contains(' ') && !uri.contains('#'), "{uri}");
            let back = path_of(&uri).expect("a file uri");
            assert_eq!(back.trim_start_matches('/'), path.replace('\\', if cfg!(windows) { "/" } else { "\\" }).trim_start_matches('/'), "{uri}");
        }
        assert_eq!(uri_of("http://x/y"), "http://x/y", "a location with a scheme stays as it is");
        assert_eq!(uri_of(""), "file:///");
        if !cfg!(windows) {
            assert_eq!(uri_of("/tmp/a\\b.xml"), "file:///tmp/a%5Cb.xml", "a backslash is a letter of a Unix name");
            assert_eq!(path_of("file:///tmp/a%5Cb.xml").as_deref(), Some("/tmp/a\\b.xml"));
        }
    }
    #[test]
    fn file_uris_are_paths_and_other_schemes_are_not() {
        assert_eq!(path_of("file:///etc/xml/cat%20alog.xml").as_deref(), Some("/etc/xml/cat alog.xml"));
        assert_eq!(path_of("file:///C:/dir/x.xml").as_deref(), Some("C:/dir/x.xml"));
        assert_eq!(path_of("FILE://localhost/a/b").as_deref(), Some("/a/b"));
        assert_eq!(path_of("file://other-host/a/b"), None);
        assert_eq!(path_of("http://x/y"), None);
        assert_eq!(path_of("é"), None, "no panic on a character that straddles the scheme length");
    }
}
