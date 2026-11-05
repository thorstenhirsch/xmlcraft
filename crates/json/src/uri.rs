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

/// A URI cut into its parts (RFC 3986, section 3).
struct Parts<'a> {
    scheme: Option<&'a str>,
    authority: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

fn split(uri: &str) -> Parts<'_> {
    let (rest, fragment) = match uri.split_once('#') {
        Some((before, fragment)) => (before, Some(fragment)),
        None => (uri, None),
    };
    let (rest, query) = match rest.split_once('?') {
        Some((before, query)) => (before, Some(query)),
        None => (rest, None),
    };
    let scheme = scheme(rest);
    let rest = scheme.map_or(rest, |s| rest.get(s.len() + 1..).unwrap_or_default());
    let (authority, path) = match rest.strip_prefix("//") {
        Some(after) => {
            let end = after.find('/').unwrap_or(after.len());
            (Some(after.get(..end).unwrap_or_default()), after.get(end..).unwrap_or_default())
        }
        None => (None, rest),
    };
    Parts { scheme, authority, path, query, fragment }
}

fn unsplit(parts: &Parts<'_>, path: &str) -> String {
    let mut out = String::new();
    if let Some(scheme) = parts.scheme {
        out.push_str(scheme);
        out.push(':');
    }
    if let Some(authority) = parts.authority {
        out.push_str("//");
        out.push_str(authority);
    }
    out.push_str(path);
    if let Some(query) = parts.query {
        out.push('?');
        out.push_str(query);
    }
    if let Some(fragment) = parts.fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

/// `reference` resolved against `base` (RFC 3986, section 5.2). A base that is opaque (`urn:x`) takes only a query or a fragment or an
/// absolute reference.
pub(crate) fn join(base: &str, reference: &str) -> String {
    let r = split(reference);
    if r.scheme.is_some() {
        return reference.to_owned();
    }
    let b = split(base);
    let opaque = b.scheme.is_some() && b.authority.is_none() && !b.path.starts_with('/');
    if opaque {
        // Only the same document with another query or fragment makes sense against an opaque base.
        return if r.authority.is_none() && r.path.is_empty() {
            let target = Parts { scheme: b.scheme, authority: None, path: b.path, query: r.query.or(b.query), fragment: r.fragment };
            unsplit(&target, b.path)
        } else {
            reference.to_owned()
        };
    }
    let (authority, path, query) = if r.authority.is_some() {
        (r.authority, remove_dots(r.path), r.query)
    } else if r.path.is_empty() {
        (b.authority, b.path.to_owned(), r.query.or(b.query))
    } else if r.path.starts_with('/') {
        (b.authority, remove_dots(r.path), r.query)
    } else {
        // (A base with a host and no path has the root as its path.)
        let base_path = if b.path.is_empty() && b.authority.is_some() { "/" } else { b.path };
        let dir = base_path.rfind('/').map_or("", |at| base_path.get(..=at).unwrap_or_default());
        (b.authority, remove_dots(&format!("{dir}{}", r.path)), r.query)
    };
    let target = Parts { scheme: b.scheme, authority, path: "", query, fragment: r.fragment };
    unsplit(&target, &path)
}

/// `.` and `..` segments taken out of a path. A relative path keeps the `..` that go above where it starts; an absolute one cannot
/// climb above its root.
fn remove_dots(path: &str) -> String {
    let absolute = path.starts_with('/');
    let segments: Vec<&str> = path.split('/').collect();
    let mut out: Vec<&str> = Vec::new();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *segment {
            "." => {
                if last {
                    out.push("");
                }
            }
            ".." => {
                let can_pop = out.last().is_some_and(|l| *l != ".." && !(absolute && out.len() == 1 && l.is_empty()));
                if can_pop {
                    out.pop();
                } else if !absolute {
                    out.push("..");
                }
                if last {
                    out.push("");
                }
            }
            other => out.push(other),
        }
    }
    let joined = out.join("/");
    if absolute && !joined.starts_with('/') { format!("/{joined}") } else { joined }
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
        assert_eq!(j("http://x.test/a/b?x=1", "?y=2"), "http://x.test/a/b?y=2", "a query alone keeps the path");
        assert_eq!(j("http://x.test/a/b?x=1", ""), "http://x.test/a/b?x=1");
        assert_eq!(j("http://x.test?q=1", "c"), "http://x.test/c", "a query right after the host is not part of the host");
        assert_eq!(j("http://x.test?q=1#f", "#g"), "http://x.test?q=1#g");
        assert_eq!(j("http://x.test/a/b?x=1#f", "c?z"), "http://x.test/a/c?z");
        assert_eq!(j("urn:uuid:1?q", "?r#s"), "urn:uuid:1?r#s");
        assert_eq!(j("urn:uuid:1?q", "#s"), "urn:uuid:1?q#s", "a fragment keeps the query of an opaque base");
        assert_eq!(j("a/b/c.json", "../../../d.json"), "../d.json", "a relative base keeps what climbs out of it");
        assert_eq!(j("", "../d.json"), "../d.json");
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
