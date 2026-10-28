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

/// Drop the last segment of `output` and the slash before it (RFC 3986 §5.2.4 step 2C).
fn pop_segment(output: &mut String) {
    match output.rfind('/') {
        Some(at) => output.truncate(at),
        None => output.clear(),
    }
}

/// RFC 3986 §5.2.4, as the input and output buffers it describes.
fn remove_dot_segments(path: &str) -> String {
    let mut input = path.to_owned();
    let mut output = String::with_capacity(path.len());
    while !input.is_empty() {
        if input.starts_with("../") {
            input.drain(..3);
        } else if input.starts_with("./") {
            input.drain(..2);
        } else if input.starts_with("/./") {
            input.replace_range(..3, "/");
        } else if input == "/." {
            input = "/".to_owned();
        } else if input.starts_with("/../") {
            input.replace_range(..4, "/");
            pop_segment(&mut output);
        } else if input == "/.." {
            input = "/".to_owned();
            pop_segment(&mut output);
        } else if input == "." || input == ".." {
            input.clear();
        } else {
            // The first path segment, with its slash if it has one, moves to the output.
            let start = usize::from(input.starts_with('/'));
            let end = input.get(start..).and_then(|rest| rest.find('/')).map_or(input.len(), |at| at + start);
            output.push_str(input.get(..end).unwrap_or_default());
            input.drain(..end);
        }
    }
    output
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

/// What may stand unescaped in a normalised URI: ASCII letters and digits, the unreserved and reserved punctuation, `#`, `[`, `]` and `%`.
fn is_plain(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_'
                | b'.'
                | b'!'
                | b'~'
                | b'*'
                | b'\''
                | b'('
                | b')'
                | b';'
                | b'/'
                | b'?'
                | b':'
                | b'@'
                | b'&'
                | b'='
                | b'+'
                | b'$'
                | b','
                | b'['
                | b']'
                | b'#'
                | b'%'
        )
}

fn hex_value(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).and_then(|d| u8::try_from(d).ok())
}

/// A system identifier or URI as it is compared (XML Catalogs 1.1 §6.3): every character that may not stand in a URI (all of those
/// outside ASCII, and the ASCII ones RFC 2396 excludes: space, `"`, `<`, `>`, `\`, `^`, `` ` ``, `{`, `|`, `}`, and the controls) is written
/// as the percent-escapes of its UTF-8 bytes, with upper-case hexadecimal digits. Everything else, `%` included, is left as it is: what
/// is already escaped stays as written, so `%7e` and `~` are two names.
pub fn normalize_uri(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() && is_plain(c as u8) {
            out.push(c);
        } else {
            let mut buffer = [0u8; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                out.push_str(&format!("%{byte:02X}"));
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
            ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
            ("g;x=1/../y", "http://a/b/c/y"),
            ("../../../../g", "http://a/g"),
            ("/../g", "http://a/g"),
            ("g/..", "http://a/b/c/"),
            ("g/../..", "http://a/b/"),
            ("g/../../..", "http://a/"),
            ("/./g", "http://a/g"),
            ("g.", "http://a/b/c/g."),
            ("g/./h", "http://a/b/c/g/h"),
            ("g/../h", "http://a/b/c/h"),
        ] {
            assert_eq!(resolve_reference(base, reference), want, "{reference:?}");
        }
        assert_eq!(resolve_reference("file:///etc/xml/catalog.xml", "dtds/a.dtd"), "file:///etc/xml/dtds/a.dtd");
        assert_eq!(resolve_reference("http://host", "x.dtd"), "http://host/x.dtd");
        assert_eq!(resolve_reference("urn:base", "a/../b"), "urn:/b", "RFC 3986 §5.2.4: the slash of the first segment is kept");
        assert_eq!(resolve_reference("", "x.dtd"), "x.dtd", "no base: as it is");
        assert_eq!(resolve_reference("relative/base", "x.dtd"), "x.dtd", "a base that is not absolute: as it is");
    }

    #[test]
    fn identifiers_are_normalised_for_comparison() {
        assert_eq!(normalize_uri("http://x/a b/é"), "http://x/a%20b/%C3%A9");
        assert_eq!(normalize_uri("http://x/%7euser/%2f%3a"), "http://x/%7euser/%2f%3a", "what is already escaped stays as written");
        assert_ne!(normalize_uri("http://x/%7e"), normalize_uri("http://x/~"), "two names");
        assert_eq!(normalize_uri("a%zz%4"), "a%zz%4", "a % is allowed: it is not touched");
        assert_eq!(normalize_uri("a\\b^c`d{e|f}g\u{7}"), "a%5Cb%5Ec%60d%7Be%7Cf%7Dg%07");
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
