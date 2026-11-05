//! Stream filters (ISO 32000-1 §7.4): the ones a PDF needs to be read here (Flate with its predictors, ASCIIHex, ASCII85). Anything
//! else is refused by name.

use crate::PdfError;
use crate::object::{Dict, Object, is_space};

/// The most samples in a row of predicted data (a row is allocated whole).
const MAX_ROW: usize = 16 * 1024 * 1024;

/// The most bytes a stream decodes to (a compressed stream can be a bomb).
pub const MAX_DECODED: usize = 256 * 1024 * 1024;

/// The filters of a stream dictionary (whose references are resolved), in the order they were applied when encoding (so decoding applies
/// them in this order), each with its decode parameters.
fn filters(dict: &Dict) -> Vec<(String, Option<&Dict>)> {
    let names: Vec<String> = match dict.get("Filter").or_else(|| dict.get("F")) {
        Some(Object::Name(n)) => vec![n.clone()],
        Some(Object::Array(items)) => items.iter().filter_map(|o| o.as_name().map(str::to_owned)).collect(),
        _ => Vec::new(),
    };
    let params: Vec<Option<&Dict>> = match dict.get("DecodeParms").or_else(|| dict.get("DP")) {
        Some(Object::Array(items)) => items.iter().map(Object::as_dict).collect(),
        Some(other) => vec![other.as_dict()],
        None => Vec::new(),
    };
    names.into_iter().enumerate().map(|(i, n)| (n, params.get(i).copied().flatten())).collect()
}

/// The bytes of a stream with its filters undone. `dict` has the references of `Filter` and `DecodeParms` resolved.
pub fn decode(raw: &[u8], dict: &Dict) -> Result<Vec<u8>, PdfError> {
    decode_with(raw, dict, MAX_DECODED)
}

/// [`decode`] with another bound on the size of what comes out.
fn decode_with(raw: &[u8], dict: &Dict, limit: usize) -> Result<Vec<u8>, PdfError> {
    // A stream with no filter is returned as it is: it is bounded by the file, but not more than the bound is copied.
    if raw.len() > limit {
        return Err(PdfError::Corrupt("a stream is larger than 256 MiB".to_owned()));
    }
    let mut data = raw.to_vec();
    for (name, params) in filters(dict) {
        data = match name.as_str() {
            "FlateDecode" | "Fl" => {
                let inflated = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&data, limit)
                    .map_err(|e| PdfError::Corrupt(format!("a stream cannot be inflated: {:?}", e.status)))?;
                predict(inflated, params, limit)?
            }
            "ASCIIHexDecode" | "AHx" => ascii_hex(&data),
            "ASCII85Decode" | "A85" => ascii85(&data, limit)?,
            other => return Err(PdfError::Unsupported(format!("the stream filter {other}"))),
        };
        if data.len() > limit {
            return Err(PdfError::Corrupt("a stream decodes to more than 256 MiB".to_owned()));
        }
    }
    Ok(data)
}

fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut high: Option<u8> = None;
    for &b in data {
        if b == b'>' {
            break;
        }
        if is_space(b) {
            continue;
        }
        if let Some(d) = (b as char).to_digit(16) {
            let d = d as u8;
            match high.take() {
                Some(h) => out.push((h << 4) | d),
                None => high = Some(d),
            }
        }
    }
    if let Some(h) = high {
        out.push(h << 4);
    }
    out
}

fn ascii85(data: &[u8], limit: usize) -> Result<Vec<u8>, PdfError> {
    let mut out = Vec::new();
    let mut group = [0u32; 5];
    let mut n = 0;
    let mut bytes = data.iter().copied().filter(|b| !is_space(*b)).peekable();
    // An optional `<~` start.
    if bytes.peek() == Some(&b'<') {
        bytes.next();
        if bytes.peek() == Some(&b'~') {
            bytes.next();
        }
    }
    let flush = |group: &[u32; 5], n: usize, out: &mut Vec<u8>| {
        let mut padded = *group;
        for slot in padded.iter_mut().skip(n) {
            *slot = 84;
        }
        let value = padded.iter().fold(0u64, |acc, d| acc * 85 + u64::from(*d));
        let word = (value & 0xFFFF_FFFF) as u32;
        let be = word.to_be_bytes();
        out.extend_from_slice(be.get(..n.saturating_sub(1)).unwrap_or_default());
    };
    for b in bytes {
        match b {
            b'~' => break,
            b'z' if n == 0 => {
                if out.len() + 4 > limit {
                    return Err(PdfError::Corrupt("a stream decodes to more than 256 MiB".to_owned()));
                }
                out.extend_from_slice(&[0, 0, 0, 0]);
            }
            b'!'..=b'u' => {
                if let Some(slot) = group.get_mut(n) {
                    *slot = u32::from(b - b'!');
                }
                n += 1;
                if n == 5 {
                    if out.len() + 4 > limit {
                        return Err(PdfError::Corrupt("a stream decodes to more than 256 MiB".to_owned()));
                    }
                    flush(&group, 5, &mut out);
                    n = 0;
                }
            }
            _ => return Err(PdfError::Corrupt("an ASCII85 stream has a character it cannot have".to_owned())),
        }
    }
    if n > 1 {
        flush(&group, n, &mut out);
    }
    Ok(out)
}

/// Undo the predictor of a Flate stream (§7.4.4.4): PNG (10–15) and TIFF 2 on 8-bit samples.
fn predict(data: Vec<u8>, params: Option<&Dict>, limit: usize) -> Result<Vec<u8>, PdfError> {
    let Some(params) = params else { return Ok(data) };
    let int = |key: &str, default: i64| params.get(key).and_then(Object::as_int).unwrap_or(default);
    let predictor = int("Predictor", 1);
    if predictor <= 1 {
        return Ok(data);
    }
    let columns = usize::try_from(int("Columns", 1))
        .ok()
        .filter(|c| (1..=1 << 24).contains(c))
        .ok_or_else(|| PdfError::Corrupt("a predictor has a bad width".to_owned()))?;
    let colors = usize::try_from(int("Colors", 1))
        .ok()
        .filter(|c| (1..=32).contains(c))
        .ok_or_else(|| PdfError::Corrupt("a predictor has a bad number of colours".to_owned()))?;
    let bits = usize::try_from(int("BitsPerComponent", 8)).unwrap_or(8);
    if bits != 8 {
        return Err(PdfError::Unsupported("a predictor on samples that are not 8 bits".to_owned()));
    }
    let row = columns.checked_mul(colors).filter(|r| *r <= MAX_ROW).ok_or_else(|| PdfError::Corrupt("a predictor row is too long".to_owned()))?;
    if predictor == 2 {
        let mut out = data;
        for chunk in out.chunks_mut(row) {
            for i in colors..chunk.len() {
                let left = chunk.get(i - colors).copied().unwrap_or(0);
                if let Some(b) = chunk.get_mut(i) {
                    *b = b.wrapping_add(left);
                }
            }
        }
        return Ok(out);
    }
    if !(10..=15).contains(&predictor) {
        return Err(PdfError::Unsupported(format!("the predictor {predictor}")));
    }
    // PNG: each row has a filter byte first.
    let stride = row + 1;
    // Only whole rows are rows (a last row that is cut short is not decoded); what comes out is no larger than what went in.
    let rows = data.len() / stride;
    if rows.saturating_mul(row) > limit {
        return Err(PdfError::Corrupt("predicted data would be larger than the bound".to_owned()));
    }
    let mut out: Vec<u8> = Vec::with_capacity(rows.saturating_mul(row).min(limit));
    let mut previous: Vec<u8> = vec![0; row];
    for chunk in data.chunks_exact(stride) {
        let (kind, raw) = match chunk.split_first() {
            Some((k, r)) => (*k, r),
            None => continue,
        };
        let mut current: Vec<u8> = raw.to_vec();
        current.resize(row, 0);
        for i in 0..row {
            let a = if i >= colors { current.get(i - colors).copied().unwrap_or(0) } else { 0 };
            let b = previous.get(i).copied().unwrap_or(0);
            let c = if i >= colors { previous.get(i - colors).copied().unwrap_or(0) } else { 0 };
            let add = match kind {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return Err(PdfError::Corrupt("a predictor row has an unknown filter".to_owned())),
            };
            if let Some(x) = current.get_mut(i) {
                *x = x.wrapping_add(add);
            }
        }
        out.extend_from_slice(&current);
        previous = current;
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i32::from(a), i32::from(b), i32::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::Parser;

    fn dict(text: &str) -> Dict {
        match Parser::new(text.as_bytes(), 0).object(&|_| None) {
            Some(Object::Dict(d)) => d,
            other => panic!("a dictionary: {other:?}"),
        }
    }

    #[test]
    fn each_png_row_filter_is_undone() {
        // Rows of three samples, filters None, Paeth, Sub and Average; the encoded rows were worked out by hand.
        let encoded = [0, 10, 20, 30, 4, 50, 221, 10, 1, 1, 1, 1, 3, 8, 4, 4];
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&encoded, 6);
        let d = dict("<< /Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns 3 >> >>");
        assert_eq!(decode(&packed, &d).expect("decodes"), [10, 20, 30, 60, 25, 40, 1, 2, 3, 8, 9, 10]);
    }

    #[test]
    fn the_tiff_predictor_adds_the_sample_on_the_left_and_a_chain_of_filters_runs_in_order() {
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&[1, 1, 1, 5, 1, 1], 6);
        let d = dict("<< /Filter /FlateDecode /DecodeParms << /Predictor 2 /Columns 3 >> >>");
        assert_eq!(decode(&packed, &d).expect("decodes"), [1, 2, 3, 5, 6, 7]);
        let hex: Vec<u8> = packed.iter().flat_map(|b| format!("{b:02X}").into_bytes()).chain(*b">").collect();
        let d = dict("<< /Filter [/ASCIIHexDecode /FlateDecode] /DecodeParms [null << /Predictor 2 /Columns 3 >>] >>");
        assert_eq!(decode(&hex, &d).expect("decodes"), [1, 2, 3, 5, 6, 7]);
    }

    #[test]
    fn what_comes_out_is_bounded_before_it_is_made() {
        let d = dict("<< /Filter /ASCII85Decode >>");
        assert!(decode_with(b"zzzz~>", &d, 8).is_err(), "each z is four bytes");
        assert!(decode_with(b"zz~>", &d, 8).is_ok());
        assert!(decode_with(b"87cURD]i,\"Ebo80~>", &d, 6).is_err(), "and so are the ordinary groups");
        assert!(decode_with(&[0; 9], &Dict::default(), 8).is_err(), "a stream with no filter is bounded too");
        // A row that is far too long is refused before it is allocated; a last row that is cut short is not decoded.
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&[0, 1, 2, 3, 0, 4], 6);
        let wide = dict("<< /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 10000000 /Colors 32 >> >>");
        assert!(matches!(decode(&packed, &wide), Err(PdfError::Corrupt(_))));
        let rows = dict("<< /Filter /FlateDecode /DecodeParms << /Predictor 10 /Columns 3 >> >>");
        assert_eq!(decode(&packed, &rows).expect("decodes"), [1, 2, 3], "the second row has one byte of three: it is not a row");
        let inflated = miniz_oxide::deflate::compress_to_vec_zlib(&[0; 64], 6);
        assert!(matches!(decode_with(&inflated, &dict("<< /Filter /FlateDecode >>"), 8), Err(PdfError::Corrupt(_))), "a bomb is cut off");
        assert!(matches!(
            decode_with(
                &miniz_oxide::deflate::compress_to_vec_zlib(&[0; 64], 6),
                &dict("<< /Filter /FlateDecode /DecodeParms << /Predictor 10 /Columns 7 >> >>"),
                8
            ),
            Err(PdfError::Corrupt(_))
        ));
    }

    #[test]
    fn ascii85_and_unknown_filters() {
        let d = dict("<< /Filter /ASCII85Decode >>");
        assert_eq!(decode(b"87cURD]i,\"Ebo80~>", &d).expect("decodes"), b"Hello World!");
        assert_eq!(decode(b"z~>", &d).expect("decodes"), [0, 0, 0, 0]);
        assert!(decode(b"87cUR\x01~>", &d).is_err());
        assert!(matches!(decode(b"x", &dict("<< /Filter /LZWDecode >>")), Err(PdfError::Unsupported(_))));
        assert!(matches!(decode(b"x", &dict("<< /Filter /FlateDecode >>")), Err(PdfError::Corrupt(_))));
        assert_eq!(decode(b"abc", &Dict::default()).expect("no filter"), b"abc");
    }
}
