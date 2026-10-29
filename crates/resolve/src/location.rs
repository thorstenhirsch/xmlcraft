//! Locations: URIs (`http://…`, `file:///…`, `bundled:/…`) and plain file paths, which is what a document's own location is when it was
//! opened from a disk. Relative references are resolved against either.

use xmlcraft_catalog::uri::resolve_reference;

/// The scheme of `location` (lower-cased), if it has one. A single letter followed by `:` and a path separator is a Windows drive, not a
/// scheme.
pub fn scheme_of(location: &str) -> Option<String> {
    let colon = location.find(':')?;
    let scheme = location.get(..colon)?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid {
        return None;
    }
    let is_drive = scheme.len() == 1 && location.get(colon + 1..).is_some_and(|rest| rest.starts_with('/') || rest.starts_with('\\'));
    (!is_drive).then(|| scheme.to_ascii_lowercase())
}

/// Whether `location` is a path (no scheme).
pub fn is_path(location: &str) -> bool {
    scheme_of(location).is_none()
}

fn is_absolute_path(path: &str) -> bool {
    let drive = path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) && path.as_bytes().get(1) == Some(&b':');
    path.starts_with('/') || path.starts_with('\\') || drive
}

/// A Windows path as the system gives it back from `canonicalize` (`\\?\C:\dir`, `\\?\UNC\server\share\dir`) in its ordinary spelling (`C:\dir`,
/// `\\server\share\dir`): the arithmetic on paths below is for those.
fn without_verbatim(path: &str) -> String {
    for prefix in ["\\\\?\\UNC\\", "//?/UNC/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return format!("\\\\{rest}");
        }
    }
    for prefix in ["\\\\?\\", "//?/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return rest.to_owned();
        }
    }
    path.to_owned()
}

/// The place `location` is, spelled so that two spellings of one place are equal: a `file:` URI as the path it names, a Windows path
/// without the verbatim prefix, with `/` for `\`, and (on Windows, where names are not case-sensitive) in lower case. For finding an open
/// document by the location a schema document was asked for with.
pub fn normalized(location: &str) -> String {
    let plain = file_path(location).unwrap_or_else(|| location.to_owned());
    let slashed = without_verbatim(&plain).replace('\\', "/");
    if cfg!(windows) { slashed.to_lowercase() } else { slashed }
}

/// `reference` resolved against `base`, which is a URI or a path (or empty). A reference that has a scheme, or is an absolute path
/// while the base is a path, stands as it is. A path base gives a path: with `/` as the separator, and anchored at the root when the base
/// is not itself absolute (a `..` that goes above it is dropped).
pub fn join(base: &str, reference: &str) -> String {
    if reference.is_empty() {
        return String::new();
    }
    if scheme_of(reference).is_some() {
        return resolve_reference("", reference);
    }
    if scheme_of(base).is_some() {
        return resolve_reference(base, reference);
    }
    if base.is_empty() || is_absolute_path(reference) {
        return reference.to_owned();
    }
    // Both are paths: do the arithmetic as a URI, on the path with forward slashes.
    let base = without_verbatim(base).replace('\\', "/");
    let reference = reference.replace('\\', "/");
    let rooted = if base.starts_with('/') { base.clone() } else { format!("/{base}") };
    let joined = resolve_reference(&format!("file://{rooted}"), &reference);
    let path = joined.strip_prefix("file://").unwrap_or(&joined);
    if base.starts_with('/') { path.to_owned() } else { path.strip_prefix('/').unwrap_or(path).to_owned() }
}

/// The file `location` names: a path as it is, a `file:` URI as the path it encodes (percent-escapes decoded; on Windows `file:///C:/x`
/// is `C:/x`). `None` for any other scheme, a host other than `localhost`, or an escape that is not UTF-8.
pub fn file_path(location: &str) -> Option<String> {
    match scheme_of(location) {
        None => Some(location.to_owned()),
        Some(scheme) if scheme == "file" => {
            let rest = location.get("file:".len()..)?;
            let rest = rest.split(['?', '#']).next().unwrap_or(rest);
            let path = match rest.strip_prefix("//") {
                Some(after) => {
                    let slash = after.find('/').unwrap_or(after.len());
                    let host = after.get(..slash)?;
                    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
                        return None;
                    }
                    after.get(slash..)?
                }
                None => rest,
            };
            let decoded = percent_decode(path)?;
            // `/C:/x` is a drive path.
            let bytes = decoded.as_bytes();
            let drive = bytes.first() == Some(&b'/') && bytes.get(2) == Some(&b':') && bytes.get(1).is_some_and(u8::is_ascii_alphabetic);
            Some(if drive { decoded.get(1..).unwrap_or_default().to_owned() } else { decoded })
        }
        Some(_) => None,
    }
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let value = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
            out.push(value);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `https://…` for an `http://…` location.
pub fn https_of(location: &str) -> Option<String> {
    let rest = location.get(..7).filter(|p| p.eq_ignore_ascii_case("http://")).and(location.get(7..))?;
    Some(format!("https://{rest}"))
}
