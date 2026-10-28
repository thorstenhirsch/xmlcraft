//! `hexBinary` and `base64Binary`.

/// The octets of a `hexBinary` lexical (an even number of hexadecimal digits, either case).
pub fn parse_hex(lexical: &str) -> Option<Vec<u8>> {
    let bytes = lexical.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks(2)
        .map(|pair| {
            let hi = hex_value(*pair.first()?)?;
            let lo = hex_value(*pair.get(1)?)?;
            Some(hi * 16 + lo)
        })
        .collect()
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The canonical `hexBinary` form: upper-case digits.
pub fn to_hex(bytes: &[u8]) -> String {
    let digit = |nibble: u8| char::from_digit(u32::from(nibble), 16).map_or('0', |c| c.to_ascii_uppercase());
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(digit(b >> 4));
        out.push(digit(b & 0x0F));
    }
    out
}

fn base64_value(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The octets of a `base64Binary` lexical: groups of four characters of the alphabet with `=` padding at the end, and, as the schema
/// language wants it, the unused bits of the last character zero (`QQ==` is a value, `QR==` is not). Single spaces between characters are
/// allowed (the lexical has been collapsed already).
pub fn parse_base64(lexical: &str) -> Option<Vec<u8>> {
    let symbols: Vec<u8> = lexical.bytes().filter(|b| *b != b' ').collect();
    if !symbols.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(symbols.len() / 4 * 3);
    let groups = symbols.len() / 4;
    for (index, group) in symbols.chunks(4).enumerate() {
        let last = index + 1 == groups;
        let (a, b, c, d) = (*group.first()?, *group.get(1)?, *group.get(2)?, *group.get(3)?);
        let (va, vb) = (base64_value(a)?, base64_value(b)?);
        match (c, d) {
            (b'=', b'=') if last => {
                if vb & 0x0F != 0 {
                    return None;
                }
                out.push(va << 2 | vb >> 4);
            }
            (_, b'=') if last => {
                let vc = base64_value(c)?;
                if vc & 0x03 != 0 {
                    return None;
                }
                out.push(va << 2 | vb >> 4);
                out.push(vb << 4 | vc >> 2);
            }
            _ => {
                let (vc, vd) = (base64_value(c)?, base64_value(d)?);
                out.push(va << 2 | vb >> 4);
                out.push(vb << 4 | vc >> 2);
                out.push(vc << 6 | vd);
            }
        }
    }
    Some(out)
}

/// The canonical `base64Binary` form: no white space, `=` padding.
pub fn to_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        let sextet = |shift: u32| ALPHABET.get(((n >> shift) & 0x3F) as usize).map_or('A', |c| char::from(*c));
        out.push(sextet(18));
        out.push(sextet(12));
        out.push(if chunk.len() > 1 { sextet(6) } else { '=' });
        out.push(if chunk.len() > 2 { sextet(0) } else { '=' });
    }
    out
}
