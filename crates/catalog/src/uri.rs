//! The URI and identifier handling the catalog rules lean on: resolving a reference against a base (RFC 3986), normalising a system
//! identifier or URI for comparison (XML Catalogs 1.1 §6.3), unwrapping `urn:publicid:` and collapsing the white space of a public
//! identifier. All of it is pure text work.

/// The parts of a URI reference (RFC 3986, appendix B).
struct Parts<'a> {
    scheme: Option<&'a str>,
    authority: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

fn split(reference: &str) -> Parts<'_> {
    let (rest, fragment) = match reference.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (reference, None),
    };
    let (rest, query) = match rest.split_once('?') {
        Some((rest, query)) => (rest, Some(query)),
        None => (rest, None),
    };
    // A scheme is letters, digits, `+`, `-` and `.` after a letter, then `:`.
    let scheme_end = rest.find(':').filter(|&at| {
        let candidate = &rest[..at];
        candidate.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && candidate.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    let (scheme, rest) = match scheme_end {
        Some(at) => (Some(&rest[..at]), &rest[at + 1..]),
        None => (None, rest),
    };
    let (authority, path) = match rest.strip_prefix("//") {
        Some(after) => match after.find('/') {
            Some(at) => (Some(&after[..at]), &after[at..]),
            None => (Some(after), ""),
        },
        None => (None, rest),
    };
    Parts { scheme, authority, path, query, fragment }
}

fn join(parts: &Parts<'_>, path: &str) -> String {
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

/// RFC 3986 §5.2.4.
fn remove_dot_segments(path: &str) -> String {
    let mut output: Vec<&str> = Vec::new();
    let absolute = path.starts_with('/');
    let mut segments = path.split('/').peekable();
    if absolute {
        segments.next(); // the empty one before the first slash
    }
    while let Some(segment) = segments.next() {
        let last = segments.peek().is_none();
        match segment {
            "." => {
                if last {
                    output.push("");
                }
            }
            ".." => {
                output.pop();
                if last {
                    output.push("");
                }
            }
            other => output.push(other),
        }
    }
    let joined = output.join("/");
    if absolute { format!("/{joined}") } else { joined }
}

/// `reference` resolved against `base` (RFC 3986 §5.2.2). A reference with a scheme stands as it is; with no base (or a base that is
/// not absolute) the reference is returned unchanged.
pub fn resolve_reference(base: &str, reference: &str) -> String {
    let r = split(reference);
    if r.scheme.is_some() {
        return join(&Parts { path: "", ..r }, &remove_dot_segments(r.path));
    }
    let b = split(base);
    if b.scheme.is_none() {
        return reference.to_owned();
    }
    let (authority, path, query) = if r.authority.is_some() {
        (r.authority, remove_dot_segments(r.path), r.query)
    } else if r.path.is_empty() {
        (b.authority, b.path.to_owned(), r.query.or(b.query))
    } else if r.path.starts_with('/') {
        (b.authority, remove_dot_segments(r.path), r.query)
    } else {
        // Merge: the base's path up to its last slash, then the reference's.
        let merged = if b.authority.is_some() && b.path.is_empty() {
            format!("/{}", r.path)
        } else {
            match b.path.rfind('/') {
                Some(at) => format!("{}{}", &b.path[..=at], r.path),
                None => r.path.to_owned(),
            }
        };
        (b.authority, remove_dot_segments(&merged), r.query)
    };
    join(&Parts { scheme: b.scheme, authority, path: "", query, fragment: r.fragment }, &path)
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')')
}

/// What may stand unescaped in a normalised URI: the unreserved characters, the reserved ones and `#`.
fn is_plain(byte: u8) -> bool {
    is_unreserved(byte) || matches!(byte, b';' | b'/' | b'?' | b':' | b'@' | b'&' | b'=' | b'+' | b'$' | b',' | b'[' | b']' | b'#')
}

fn hex_value(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).and_then(|d| u8::try_from(d).ok())
}

/// A system identifier or URI as it is compared (XML Catalogs 1.1 §6.3): every character that may not stand in a URI is written as the
/// percent-escapes of its UTF-8 bytes; an escape of an unreserved character is replaced by the character; the hexadecimal digits of the
/// escapes that are left are upper case.
pub fn normalize_uri(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        let escape =
            (byte == b'%').then(|| bytes.get(i + 1).copied().and_then(hex_value).zip(bytes.get(i + 2).copied().and_then(hex_value))).flatten();
        match escape {
            Some((high, low)) => {
                let value = high * 16 + low;
                if is_unreserved(value) {
                    out.push(char::from(value));
                } else {
                    out.push_str(&format!("%{value:02X}"));
                }
                i += 3;
            }
            None if is_plain(byte) => {
                out.push(char::from(byte));
                i += 1;
            }
            None => {
                out.push_str(&format!("%{byte:02X}"));
                i += 1;
            }
        }
    }
    out
}

/// A public identifier as it is compared: runs of white space are one space, and none is at either end.
pub fn normalize_public(text: &str) -> String {
    text.split([' ', '\t', '\r', '\n']).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The public identifier a `urn:publicid:` URN stands for (RFC 3151), or `None` when `text` is not one: `+` is a space, `:` is `//`,
/// `;` is `::`, and the escapes %2B %3A %2F %3B %27 %3F %23 %25 are their characters.
pub fn unwrap_publicid_urn(text: &str) -> Option<String> {
    let rest = text.get(..13).filter(|head| head.eq_ignore_ascii_case("urn:publicid:")).map(|_| &text[13..])?;
    let bytes = rest.as_bytes();
    let mut out = String::with_capacity(rest.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        match byte {
            b'+' => out.push(' '),
            b':' => out.push_str("//"),
            b';' => out.push_str("::"),
            b'%' => {
                let value = bytes.get(i + 1).copied().and_then(hex_value).zip(bytes.get(i + 2).copied().and_then(hex_value)).map(|(h, l)| h * 16 + l);
                match value {
                    Some(v @ (0x2B | 0x3A | 0x2F | 0x3B | 0x27 | 0x3F | 0x23 | 0x25)) => {
                        out.push(char::from(v));
                        i += 3;
                        continue;
                    }
                    _ => out.push('%'),
                }
            }
            _ => {
                // A character of several bytes is copied whole.
                let width = rest.get(i..).and_then(|tail| tail.chars().next()).map_or(1, char::len_utf8);
                if let Some(chunk) = rest.get(i..i + width) {
                    out.push_str(chunk);
                }
                i += width;
                continue;
            }
        }
        i += 1;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_are_resolved_as_rfc_3986_says() {
        let base = "http://a/b/c/d;p?q";
        for (reference, want) in [
            ("g:h", "g:h"),
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("g#s", "http://a/b/c/g#s"),
            ("", "http://a/b/c/d;p?q"),
            (".", "http://a/b/c/"),
            ("./", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../", "http://a/b/"),
            ("../g", "http://a/b/g"),
            ("../..", "http://a/"),
            ("../../g", "http://a/g"),
            ("../../../g", "http://a/g"),
            ("/./g", "http://a/g"),
            ("g.", "http://a/b/c/g."),
            ("g/./h", "http://a/b/c/g/h"),
            ("g/../h", "http://a/b/c/h"),
        ] {
            assert_eq!(resolve_reference(base, reference), want, "{reference:?}");
        }
        assert_eq!(resolve_reference("file:///etc/xml/catalog.xml", "dtds/a.dtd"), "file:///etc/xml/dtds/a.dtd");
        assert_eq!(resolve_reference("http://host", "x.dtd"), "http://host/x.dtd");
        assert_eq!(resolve_reference("", "x.dtd"), "x.dtd", "no base: as it is");
        assert_eq!(resolve_reference("relative/base", "x.dtd"), "x.dtd", "a base that is not absolute: as it is");
    }

    #[test]
    fn identifiers_are_normalised_for_comparison() {
        assert_eq!(normalize_uri("http://x/a b/é"), "http://x/a%20b/%C3%A9");
        assert_eq!(normalize_uri("http://x/%7euser/%2f%3a"), "http://x/~user/%2F%3A".replace("%3a", "%3A"));
        assert_eq!(normalize_uri("a%zz%4"), "a%25zz%254", "a % that does not start an escape is escaped itself");
        assert_eq!(normalize_uri("x\"<>"), "x%22%3C%3E");
        assert_eq!(normalize_public("  -//A//B\n\t C  "), "-//A//B C");
        assert_eq!(normalize_public(""), "");
    }

    #[test]
    fn a_publicid_urn_is_unwrapped() {
        assert_eq!(
            unwrap_publicid_urn("urn:publicid:ISO%2FIEC+10179%3A1996:DTD+DSSSL+Architecture:EN").as_deref(),
            Some("ISO/IEC 10179:1996//DTD DSSSL Architecture//EN")
        );
        assert_eq!(unwrap_publicid_urn("URN:PUBLICID:-:OASIS:DTD+DocBook+XML+V4.1.2:EN").as_deref(), Some("-//OASIS//DTD DocBook XML V4.1.2//EN"));
        assert_eq!(unwrap_publicid_urn("urn:publicid:a;b").as_deref(), Some("a::b"));
        assert_eq!(unwrap_publicid_urn("urn:publicid:100%25%zz é").as_deref(), Some("100%%zz é"));
        assert_eq!(unwrap_publicid_urn("http://x"), None);
        assert_eq!(unwrap_publicid_urn("urn:publicid"), None);
        assert_eq!(unwrap_publicid_urn("é"), None, "no panic on a character that straddles the prefix length");
    }

    proptest::proptest! {
        /// Nothing panics, and a normalised URI is normal: normalising it again changes nothing, and only plain characters and escapes are in it.
        #[test]
        fn normalising_is_total_and_idempotent(text in "\\PC{0,40}") {
            let once = normalize_uri(&text);
            proptest::prop_assert_eq!(normalize_uri(&once), once.clone());
            proptest::prop_assert!(once.bytes().all(|b| is_plain(b) || b == b'%'));
            let _ = unwrap_publicid_urn(&text);
            let _ = resolve_reference("http://a/b/c", &text);
            let _ = resolve_reference(&text, "x");
        }
    }
}
