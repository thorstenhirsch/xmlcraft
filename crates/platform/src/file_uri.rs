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
    }
}
