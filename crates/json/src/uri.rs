//! Just enough of RFC 3986 for `$id` and `$ref`: split off the fragment, resolve a reference against a base, decode `%`-escapes.

/// `uri` without its fragment (`http://x/y#a` is `http://x/y`).
pub(crate) fn without_fragment(uri: &str) -> &str {
    uri.split_once('#').map_or(uri, |(before, _)| before)
}

/// The fragment of `uri` (after the `#`), or none.
pub(crate) fn fragment(uri: &str) -> Option<&str> {
    uri.split_once('#').map(|(_, fragment)| fragment)
}

/// The scheme of `uri` (`http` in `http://x`), if it has one.
fn scheme(uri: &str) -> Option<&str> {
    let (scheme, _) = uri.split_once(':')?;
    let mut chars = scheme.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    (first_ok && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))).then_some(scheme)
}

/// `reference` resolved against `base`. An opaque base (`urn:x`) only takes a fragment or an absolute reference.
pub(crate) fn join(base: &str, reference: &str) -> String {
    if scheme(reference).is_some() {
        return reference.to_owned();
    }
    let base_path = without_fragment(base);
    if reference.is_empty() {
        return base_path.to_owned();
    }
    if reference.starts_with('#') {
        return format!("{base_path}{reference}");
    }
    let Some(base_scheme) = scheme(base_path) else {
        // A base with no scheme is a path (or nothing): merge as paths do.
        return merge_path(base_path, reference);
    };
    let after_scheme = base_path.get(base_scheme.len() + 1..).unwrap_or_default();
    if !after_scheme.starts_with('/') {
        return reference.to_owned(); // opaque: urn:x + y has no meaning
    }
    if let Some(rest) = reference.strip_prefix("//") {
        return format!("{base_scheme}://{rest}");
    }
    let authority_end = after_scheme.strip_prefix("//").map(|rest| rest.find('/').map_or(after_scheme.len(), |at| at + 2));
    let (origin, path) = match authority_end {
        Some(end) => (format!("{base_scheme}:{}", after_scheme.get(..end).unwrap_or_default()), after_scheme.get(end..).unwrap_or_default()),
        None => (format!("{base_scheme}:"), after_scheme),
    };
    // (A base with a host and no path has the root as its path.)
    let path = if path.is_empty() && authority_end.is_some() { "/" } else { path };
    format!("{origin}{}", merge_path(path, reference))
}

/// `reference` against the path `base` (a reference that starts with `/` replaces the path).
fn merge_path(base: &str, reference: &str) -> String {
    let (reference_path, rest) = match reference.find(['?', '#']) {
        Some(at) => reference.split_at(at),
        None => (reference, ""),
    };
    let merged = if reference_path.starts_with('/') {
        reference_path.to_owned()
    } else {
        let dir = base.rfind('/').map_or("", |at| base.get(..=at).unwrap_or_default());
        format!("{dir}{reference_path}")
    };
    let absolute = merged.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = merged.split('/').collect();
    for (i, segment) in segments.iter().enumerate() {
        match *segment {
            "." => {}
            ".." => {
                if out.last().is_some_and(|last| !last.is_empty() || out.len() > 1) {
                    out.pop();
                }
            }
            other => out.push(other),
        }
        if i + 1 == segments.len() && matches!(*segment, "." | "..") {
            out.push("");
        }
    }
    let mut path = out.join("/");
    if absolute && !path.starts_with('/') {
        path.insert(0, '/');
    }
    format!("{path}{rest}")
}

/// The bytes of `text` with `%XX` escapes decoded (a bad escape stays as it is); invalid UTF-8 gives none.
pub(crate) fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let escaped = (b == b'%')
            .then(|| bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match escaped {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// A JSON Pointer token with `~0` and `~1` decoded; any other `~` is no pointer.
pub(crate) fn unescape_token(token: &str) -> Option<String> {
    let mut out = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(c) = chars.next() {
        if c == '~' {
            match chars.next()? {
                '0' => out.push('~'),
                '1' => out.push('/'),
                _ => return None,
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// A JSON Pointer token with `~` and `/` escaped.
pub(crate) fn escape_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_resolve_against_a_base() {
        let j = join;
        assert_eq!(j("http://x.test/a/b.json", "c.json"), "http://x.test/a/c.json");
        assert_eq!(j("http://x.test/a/b.json", "../c.json#/d"), "http://x.test/c.json#/d");
        assert_eq!(j("http://x.test/a/b.json", "/c"), "http://x.test/c");
        assert_eq!(j("http://x.test/a/b.json", "//y.test/c"), "http://y.test/c");
        assert_eq!(j("http://x.test/a/b.json#old", "#new"), "http://x.test/a/b.json#new");
        assert_eq!(j("http://x.test/a/b.json", ""), "http://x.test/a/b.json");
        assert_eq!(j("http://x.test/a/", "https://z.test/q"), "https://z.test/q");
        assert_eq!(j("http://x.test/a/b", "../../../c"), "http://x.test/c", "cannot climb above the root");
        assert_eq!(j("http://x.test", "c"), "http://x.test/c");
        assert_eq!(j("urn:uuid:1234", "#foo"), "urn:uuid:1234#foo");
        assert_eq!(j("urn:uuid:1234", "other"), "other");
        assert_eq!(j("", "a/b.json"), "a/b.json");
        assert_eq!(j("dir/x.json", "y.json"), "dir/y.json");
        assert_eq!(j("file:///d/x.json", "y/z.json"), "file:///d/y/z.json");
        assert_eq!(j("http://x.test/a/b", "./c/./d/.."), "http://x.test/a/c/");
    }

    #[test]
    fn fragments_and_escapes() {
        assert_eq!((without_fragment("a#b"), fragment("a#b"), fragment("a")), ("a", Some("b"), None));
        assert_eq!(percent_decode("a%25b%2Fc"), Some("a%b/c".to_owned()));
        assert_eq!(percent_decode("100%"), Some("100%".to_owned()));
        assert_eq!(percent_decode("%FF"), None);
        assert_eq!(escape_token("a/b~c"), "a~1b~0c");
    }

    #[test]
    fn nothing_panics_on_odd_input() {
        for base in ["", "x", ":", "http:", "http://", "http://x", "//", "a:b:c", "#", "?q"] {
            for reference in ["", "#", "..", "../..", "/", "//", "?q", "a:", "\u{1F600}", "%", "./", "x/../../.."] {
                let _ = join(base, reference);
            }
        }
    }
}
