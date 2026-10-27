//! Encodings: file bytes to text and back, without losing a byte.
//!
//! [`sniff`] decides how a file is encoded (byte-order mark, then the XML declaration, then UTF-8), [`decode`] reads it and says
//! whether anything had to be replaced, and [`encode`] writes it back. The encoding and the byte-order mark are remembered by the
//! caller, so an unedited file is written back identically. A character the encoding cannot hold is never replaced by `?`: [`encode`]
//! reports it, and the caller asks the user whether to write a character reference or to change the encoding.
//!
//! Known gap: `ISO-8859-1` is read as `windows-1252` (the WHATWG mapping that `encoding_rs` implements). The bytes round-trip; only
//! the display of 0x80..0x9F differs.

use std::ops::Range;

/// A character encoding the editor can read and write.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Encoding(&'static encoding_rs::Encoding);

impl std::fmt::Debug for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl Encoding {
    pub const UTF_8: Self = Self(encoding_rs::UTF_8);
    pub const UTF_16LE: Self = Self(encoding_rs::UTF_16LE);
    pub const UTF_16BE: Self = Self(encoding_rs::UTF_16BE);
    pub const WINDOWS_1252: Self = Self(encoding_rs::WINDOWS_1252);

    /// The encoding a label such as an XML declaration's `encoding="..."` names, if it is one we support.
    pub fn from_label(label: &str) -> Option<Self> {
        let enc = encoding_rs::Encoding::for_label(label.trim().as_bytes())?;
        // These two are web-platform oddities: one only decodes to U+FFFD, the other maps bytes to private-use characters.
        (enc != encoding_rs::REPLACEMENT && enc != encoding_rs::X_USER_DEFINED).then_some(Self(enc))
    }

    /// The canonical name, which is also what to write in an XML declaration (`UTF-8`, `UTF-16LE`, `windows-1252`, ...).
    pub fn name(self) -> &'static str {
        self.0.name()
    }

    pub fn is_utf8(self) -> bool {
        self == Self::UTF_8
    }

    pub fn is_utf16(self) -> bool {
        self == Self::UTF_16LE || self == Self::UTF_16BE
    }

    /// The byte-order mark of this encoding (empty for encodings that have none).
    fn bom_bytes(self) -> &'static [u8] {
        if self.is_utf8() {
            &[0xEF, 0xBB, 0xBF]
        } else if self == Self::UTF_16LE {
            &[0xFF, 0xFE]
        } else if self == Self::UTF_16BE {
            &[0xFE, 0xFF]
        } else {
            &[]
        }
    }
}

/// How a file turned out to be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub encoding: Encoding,
    /// The file starts with a byte-order mark (kept on save).
    pub bom: bool,
    /// What the XML declaration says, if it has an `encoding` (even when it is wrong or unknown: the caller shows the mismatch).
    pub declared: Option<String>,
}

impl Detected {
    /// UTF-8 without a byte-order mark: new documents.
    pub fn utf8() -> Self {
        Self { encoding: Encoding::UTF_8, bom: false, declared: None }
    }

    /// Whether the declared encoding names the encoding the file is really in (also true when nothing is declared).
    pub fn declaration_matches(&self) -> bool {
        self.declared.as_deref().is_none_or(|label| Encoding::from_label(label) == Some(self.encoding))
    }
}

/// How much of the start of a file is looked at for the XML declaration.
const DECLARATION_SCAN: usize = 1024;

/// Decide how `bytes` are encoded: a byte-order mark wins; without one, UTF-16 is recognised by `<?` (XML 1.0, appendix F), then
/// the declaration's `encoding` is used if it names an ASCII-compatible encoding; otherwise UTF-8. A UTF-32 file is not supported
/// (its mark reads as UTF-16LE with a NUL, which the well-formedness check then reports).
pub fn sniff(bytes: &[u8]) -> Detected {
    let (mut encoding, bom) = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (Encoding::UTF_8, true)
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        (Encoding::UTF_16LE, true)
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        (Encoding::UTF_16BE, true)
    } else if bytes.starts_with(&[0x3C, 0x00, 0x3F, 0x00]) {
        (Encoding::UTF_16LE, false)
    } else if bytes.starts_with(&[0x00, 0x3C, 0x00, 0x3F]) {
        (Encoding::UTF_16BE, false)
    } else {
        (Encoding::UTF_8, false)
    };
    let skip = if bom { encoding.bom_bytes().len() } else { 0 };
    let body = bytes.get(skip..).unwrap_or_default();
    let head = body.get(..body.len().min(DECLARATION_SCAN)).unwrap_or_default();
    let declared = if encoding.is_utf16() {
        // Read the first characters to find the declaration (an odd trailing byte is ignored).
        let even = head.get(..head.len() & !1).unwrap_or_default();
        let (text, _) = encoding.0.decode_without_bom_handling(even);
        declaration_value(text.as_bytes()).map(|(_, label)| label)
    } else {
        declaration_value(head).map(|(_, label)| label)
    };
    if !bom && !encoding.is_utf16() {
        // An ASCII-compatible file: the declaration decides, unless it names UTF-16 (which these bytes are not) or nothing we know.
        if let Some(declared) = declared.as_deref().and_then(Encoding::from_label).filter(|e| !e.is_utf16()) {
            encoding = declared;
        }
    }
    Detected { encoding, bom, declared }
}

/// The text of an XML declaration's `encoding="..."` value and where it is, if `bytes` start with a declaration that has one.
fn declaration_value(bytes: &[u8]) -> Option<(Range<usize>, String)> {
    let is_space = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n');
    if !bytes.starts_with(b"<?xml") || !bytes.get(5).copied().is_some_and(is_space) {
        return None;
    }
    let mut i = 5;
    loop {
        while bytes.get(i).copied().is_some_and(is_space) {
            i += 1;
        }
        let name_start = i;
        while bytes.get(i).copied().is_some_and(|b| b.is_ascii_alphabetic()) {
            i += 1;
        }
        let name = bytes.get(name_start..i)?;
        if name.is_empty() {
            return None; // `?>`, or something that is not a pseudo-attribute
        }
        while bytes.get(i).copied().is_some_and(is_space) {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        while bytes.get(i).copied().is_some_and(is_space) {
            i += 1;
        }
        let quote = bytes.get(i).copied().filter(|q| matches!(q, b'"' | b'\''))?;
        let start = i + 1;
        let len = bytes.get(start..)?.iter().position(|&b| b == quote)?;
        let end = start + len;
        if name == b"encoding" {
            let label = std::str::from_utf8(bytes.get(start..end)?).ok()?;
            return Some((start..end, label.to_owned()));
        }
        i = end + 1;
    }
}

/// Where the encoding name is in the XML declaration of `text` (the range between the quotes), so that changing the encoding can
/// replace just that name. `None` when there is no declaration or it has no `encoding`.
pub fn declared_encoding_span(text: &str) -> Option<Range<usize>> {
    let head = text.as_bytes().get(..text.len().min(DECLARATION_SCAN)).unwrap_or_default();
    declaration_value(head).map(|(span, _)| span)
}

/// The text of a file, and whether reading it changed anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    /// Some bytes could not be read in the encoding and were replaced by U+FFFD, or writing the text back would not give the same
    /// bytes. The caller must keep the original bytes and must not overwrite them silently.
    pub lossy: bool,
}

/// Read `bytes` as `detected` says; a byte-order mark is dropped (it is remembered in `detected`).
pub fn decode(bytes: &[u8], detected: &Detected) -> Decoded {
    let enc = detected.encoding;
    let mark = enc.bom_bytes();
    let has_mark = detected.bom && !mark.is_empty() && bytes.starts_with(mark);
    let body = bytes.get(if has_mark { mark.len() } else { 0 }..).unwrap_or_default();
    // Told there is a mark, but there is none: writing the text back would add one.
    let mut lossy = detected.bom && !mark.is_empty() && !has_mark;
    let text = if enc.is_utf8() {
        match std::str::from_utf8(body) {
            Ok(s) => s.to_owned(),
            Err(_) => {
                lossy = true;
                String::from_utf8_lossy(body).into_owned()
            }
        }
    } else {
        let (text, had_errors) = enc.0.decode_without_bom_handling(body);
        lossy |= had_errors;
        let text = text.into_owned();
        if !lossy && !enc.is_utf16() {
            // A legacy encoding can have several byte sequences for one character: only an exact round trip is lossless.
            lossy = encode(&text, enc, false, Unmappable::Refuse).map_or(true, |again| again != body);
        }
        text
    };
    Decoded { text, lossy }
}

/// What to do with a character the encoding cannot hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unmappable {
    /// Stop and report it.
    Refuse,
    /// Write it as a character reference (`&#x20AC;`). Right in text and attribute values, wrong in comments and CDATA, so the
    /// user chooses.
    CharRefs,
}

/// A character the encoding cannot hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncodeError {
    /// Where it is in the text, in bytes.
    pub offset: usize,
    pub ch: char,
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the character U+{:04X} at byte {} cannot be written in this encoding", self.ch as u32, self.offset)
    }
}

impl std::error::Error for EncodeError {}

/// Write `text` in `encoding`, with a byte-order mark if `bom` (only UTF-8 and UTF-16 have one).
pub fn encode(text: &str, encoding: Encoding, bom: bool, policy: Unmappable) -> Result<Vec<u8>, EncodeError> {
    encode_chunks([text], encoding, bom, policy)
}

/// Like [`encode`] for a text in pieces (a rope's chunks), without joining them first.
pub fn encode_chunks<'a>(
    chunks: impl IntoIterator<Item = &'a str>,
    encoding: Encoding,
    bom: bool,
    policy: Unmappable,
) -> Result<Vec<u8>, EncodeError> {
    let mut out = Vec::new();
    if bom {
        out.extend_from_slice(encoding.bom_bytes());
    }
    if encoding.is_utf8() {
        for chunk in chunks {
            out.extend_from_slice(chunk.as_bytes());
        }
    } else if encoding.is_utf16() {
        let little = encoding == Encoding::UTF_16LE;
        for chunk in chunks {
            for unit in chunk.encode_utf16() {
                out.extend_from_slice(&if little { unit.to_le_bytes() } else { unit.to_be_bytes() });
            }
        }
    } else {
        encode_legacy(encoding.0, chunks, policy, &mut out)?;
    }
    Ok(out)
}

/// Encode with an `encoding_rs` encoder (every encoding except UTF-8 and UTF-16).
fn encode_legacy<'a>(
    encoding: &'static encoding_rs::Encoding,
    chunks: impl IntoIterator<Item = &'a str>,
    policy: Unmappable,
    out: &mut Vec<u8>,
) -> Result<(), EncodeError> {
    let mut encoder = encoding.new_encoder();
    let mut buf = [0u8; 4096];
    // Bytes of the text before the current piece. The last call has no input and flushes the encoder.
    let mut base = 0usize;
    for (chunk, last) in chunks.into_iter().map(|c| (c, false)).chain(std::iter::once(("", true))) {
        let mut rest = chunk;
        let mut done = 0usize;
        loop {
            let (result, read, written) = encoder.encode_from_utf8_without_replacement(rest, &mut buf, last);
            out.extend_from_slice(buf.get(..written).unwrap_or_default());
            done = done.saturating_add(read);
            rest = rest.get(read..).unwrap_or_default();
            match result {
                encoding_rs::EncoderResult::InputEmpty => break,
                encoding_rs::EncoderResult::OutputFull => {
                    if read == 0 && written == 0 {
                        // No progress with an empty 4 KiB buffer cannot happen; stop rather than loop forever.
                        let ch = rest.chars().next().unwrap_or('\u{FFFD}');
                        return Err(EncodeError { offset: base.saturating_add(done), ch });
                    }
                }
                encoding_rs::EncoderResult::Unmappable(ch) => match policy {
                    // `read` includes the character the encoder refused.
                    Unmappable::Refuse => {
                        return Err(EncodeError { offset: base.saturating_add(done).saturating_sub(ch.len_utf8()), ch });
                    }
                    // The encoder is back in ASCII mode after refusing a character, so ASCII can be written directly.
                    Unmappable::CharRefs => out.extend_from_slice(format!("&#x{:X};", u32::from(ch)).as_bytes()),
                },
            }
        }
        base = base.saturating_add(chunk.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn det(encoding: Encoding, bom: bool) -> Detected {
        Detected { encoding, bom, declared: None }
    }

    #[test]
    fn a_byte_order_mark_decides() {
        assert_eq!(sniff(b"\xEF\xBB\xBF<a/>"), Detected { encoding: Encoding::UTF_8, bom: true, declared: None });
        assert_eq!(sniff(b"\xFF\xFE<\0a\0").encoding, Encoding::UTF_16LE);
        assert_eq!(sniff(b"\xFE\xFF\0<\0a").encoding, Encoding::UTF_16BE);
        assert!(sniff(b"\xFE\xFF\0<\0a").bom);
        // The mark wins over a declaration that says something else, and the mismatch is recorded.
        let d = sniff(b"\xEF\xBB\xBF<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>");
        assert_eq!((d.encoding, d.bom, d.declared.as_deref()), (Encoding::UTF_8, true, Some("ISO-8859-1")));
        assert!(!d.declaration_matches());
    }

    #[test]
    fn utf16_without_a_mark_is_recognised_by_the_declaration_start() {
        let le = encode("<?xml version=\"1.0\" encoding=\"UTF-16\"?><a/>", Encoding::UTF_16LE, false, Unmappable::Refuse).expect("ok");
        let d = sniff(&le);
        assert_eq!((d.encoding, d.bom, d.declared.as_deref()), (Encoding::UTF_16LE, false, Some("UTF-16")));
        let be = encode("<?xml version=\"1.0\"?><a/>", Encoding::UTF_16BE, false, Unmappable::Refuse).expect("ok");
        assert_eq!(sniff(&be), Detected { encoding: Encoding::UTF_16BE, bom: false, declared: None });
    }

    #[test]
    fn the_declaration_decides_without_a_mark() {
        let d = sniff(b"<?xml version='1.0' encoding='windows-1252'?><a/>");
        assert_eq!((d.encoding, d.bom), (Encoding::WINDOWS_1252, false));
        assert!(d.declaration_matches());
        assert_eq!(sniff(b"<?xml version=\"1.0\" encoding=\"utf-8\"?>").encoding, Encoding::UTF_8);
        assert_eq!(sniff(b"<?xml encoding = \"ISO-8859-1\" version=\"1.0\"?>").encoding, Encoding::WINDOWS_1252);
        assert_eq!(sniff(b"<a/>"), Detected::utf8());
        assert_eq!(sniff(b""), Detected::utf8());
    }

    #[test]
    fn an_unusable_declaration_means_utf8_and_is_recorded() {
        for (src, declared) in [
            (&b"<?xml version=\"1.0\" encoding=\"klingon\"?>"[..], "klingon"),
            (b"<?xml version=\"1.0\" encoding=\"UTF-16\"?>", "UTF-16"), // ASCII bytes cannot be UTF-16
            (b"<?xml version=\"1.0\" encoding=\"iso-2022-kr\"?>", "iso-2022-kr"), // only decodes to U+FFFD
        ] {
            let d = sniff(src);
            assert_eq!((d.encoding, d.declared.as_deref()), (Encoding::UTF_8, Some(declared)), "{}", String::from_utf8_lossy(src));
            assert!(!d.declaration_matches());
        }
    }

    #[test]
    fn only_a_real_declaration_at_the_very_start_counts() {
        for src in [
            &b" <?xml version=\"1.0\" encoding=\"windows-1252\"?>"[..],
            b"<?xml-stylesheet encoding=\"windows-1252\"?>",
            b"<?xml version=\"1.0\"?><!-- encoding=\"windows-1252\" -->",
            b"<?xml version=\"1.0\" encoding=windows-1252?>",
            b"<?xml version=\"1.0\" encoding=\"windows-1252?>",
            b"<?xml version",
            b"<?xml",
            b"<?xml ",
            b"<?xml ?>",
        ] {
            let d = sniff(src);
            assert_eq!((d.encoding, d.declared.as_deref()), (Encoding::UTF_8, None), "{}", String::from_utf8_lossy(src));
        }
    }

    #[test]
    fn the_span_of_the_declared_name_is_found_for_a_minimal_edit() {
        let text = "<?xml version=\"1.0\" encoding='UTF-8' standalone=\"yes\"?><a/>";
        let span = declared_encoding_span(text).expect("declared");
        assert_eq!(&text[span], "UTF-8");
        assert_eq!(declared_encoding_span("<?xml version=\"1.0\"?><a/>"), None);
        assert_eq!(declared_encoding_span("<a/>"), None);
        assert_eq!(declared_encoding_span(""), None);
        // A multi-byte character before the end of the scan window does not break the slicing.
        let long = format!("<?xml version=\"1.0\" comment=\"{}\" encoding=\"UTF-8\"?>", "é".repeat(600));
        assert_eq!(declared_encoding_span(&long), None, "past the scan window");
    }

    #[test]
    fn utf8_with_and_without_a_mark_round_trips() {
        for (bytes, bom) in [(&b"<a>h\xC3\xA9</a>"[..], false), (b"\xEF\xBB\xBF<a>h\xC3\xA9</a>", true)] {
            let d = sniff(bytes);
            assert_eq!(d.bom, bom);
            let decoded = decode(bytes, &d);
            assert_eq!(decoded.text, "<a>hé</a>");
            assert!(!decoded.lossy);
            assert_eq!(encode(&decoded.text, d.encoding, d.bom, Unmappable::Refuse).expect("ok"), bytes);
        }
    }

    #[test]
    fn invalid_utf8_is_replaced_and_flagged() {
        let d = sniff(b"<a>\xFF\xFE\xE9</a>");
        let decoded = decode(b"<a>\xFF\xFE\xE9</a>", &d);
        assert!(decoded.lossy);
        assert!(decoded.text.contains('\u{FFFD}'));
        // Reading the same bytes as windows-1252 is lossless.
        let latin = decode(b"<a>\xFF\xFE\xE9</a>", &det(Encoding::WINDOWS_1252, false));
        assert!(!latin.lossy);
        assert_eq!(latin.text, "<a>ÿþé</a>");
    }

    #[test]
    fn every_byte_round_trips_in_windows_1252() {
        let all: Vec<u8> = (0..=255).collect();
        let d = det(Encoding::WINDOWS_1252, false);
        let decoded = decode(&all, &d);
        assert!(!decoded.lossy, "the five undefined bytes still map to C1 controls");
        assert_eq!(encode(&decoded.text, Encoding::WINDOWS_1252, false, Unmappable::Refuse).expect("ok"), all);
    }

    #[test]
    fn utf16_round_trips_with_surrogates_and_both_orders() {
        let text = "<a>€ 😀 日本</a>";
        for enc in [Encoding::UTF_16LE, Encoding::UTF_16BE] {
            for bom in [false, true] {
                let bytes = encode(text, enc, bom, Unmappable::Refuse).expect("ok");
                assert_eq!(bytes.len(), usize::from(bom) * 2 + text.encode_utf16().count() * 2);
                let decoded = decode(&bytes, &det(enc, bom));
                assert_eq!((decoded.text.as_str(), decoded.lossy), (text, false), "{enc:?} bom={bom}");
            }
        }
    }

    #[test]
    fn broken_utf16_is_flagged_not_dropped() {
        let d = det(Encoding::UTF_16LE, false);
        assert!(decode(b"<\0a\0\x3D", &d).lossy, "an odd trailing byte");
        assert!(decode(b"<\0\x00\xD8a\0", &d).lossy, "a lone high surrogate");
    }

    #[test]
    fn a_mark_that_is_not_there_is_flagged() {
        // The caller said the file has a mark; writing it back would add one.
        assert!(decode(b"<a/>", &det(Encoding::UTF_8, true)).lossy);
        // A mark that is not declared stays in the text as U+FEFF and round-trips.
        let kept = decode(b"\xEF\xBB\xBF<a/>", &det(Encoding::UTF_8, false));
        assert!(!kept.lossy);
        assert!(kept.text.starts_with('\u{FEFF}'));
    }

    #[test]
    fn a_character_the_encoding_cannot_hold_is_reported_with_its_place() {
        let err = encode("<a>é€</a>", Encoding::WINDOWS_1252, false, Unmappable::Refuse);
        assert!(err.is_ok(), "the euro sign is in windows-1252");
        let err = encode("<a>é日</a>", Encoding::WINDOWS_1252, false, Unmappable::Refuse).expect_err("refused");
        assert_eq!(err, EncodeError { offset: 5, ch: '日' });
        assert!(err.to_string().contains("U+65E5"));
        let err = encode_chunks(["<a>é", "日</a>"], Encoding::WINDOWS_1252, false, Unmappable::Refuse).expect_err("refused");
        assert_eq!(err.offset, 5, "offsets run across pieces");
    }

    #[test]
    fn character_references_can_stand_in_for_unmappable_characters() {
        let bytes = encode("<a>é日😀</a>", Encoding::WINDOWS_1252, false, Unmappable::CharRefs).expect("ok");
        assert_eq!(bytes, b"<a>\xE9&#x65E5;&#x1F600;</a>");
        let shift_jis = Encoding::from_label("shift_jis").expect("supported");
        let bytes = encode("日本😀", shift_jis, false, Unmappable::CharRefs).expect("ok");
        assert!(bytes.ends_with(b"&#x1F600;"), "{bytes:?}");
    }

    #[test]
    fn legacy_multibyte_encodings_round_trip() {
        for label in ["shift_jis", "gbk", "big5", "euc-kr", "iso-8859-2", "koi8-r", "windows-1251", "iso-2022-jp"] {
            let enc = Encoding::from_label(label).expect(label);
            let sample = match label {
                "shift_jis" | "iso-2022-jp" => "日本語のテキスト <a/>",
                "gbk" => "中文文本 <a/>",
                "big5" => "中文文本 <a/>",
                "euc-kr" => "한국어 <a/>",
                "iso-8859-2" => "zażółć gęślą jaźń <a/>",
                _ => "Привет мир <a/>",
            };
            let bytes = encode(sample, enc, false, Unmappable::Refuse).expect(label);
            let decoded = decode(&bytes, &det(enc, false));
            assert_eq!((decoded.text.as_str(), decoded.lossy), (sample, false), "{label}");
        }
    }

    #[test]
    fn labels_resolve_and_unsupported_ones_do_not() {
        assert_eq!(Encoding::from_label(" UTF-8 "), Some(Encoding::UTF_8));
        assert_eq!(Encoding::from_label("latin1"), Some(Encoding::WINDOWS_1252));
        assert_eq!(Encoding::from_label("nonsense"), None);
        assert_eq!(Encoding::from_label("hz-gb-2312"), None);
        assert_eq!(Encoding::from_label("x-user-defined"), None);
        assert_eq!(Encoding::UTF_16BE.name(), "UTF-16BE");
        assert_eq!(format!("{:?}", Encoding::WINDOWS_1252), "windows-1252");
    }

    #[test]
    fn a_text_larger_than_the_output_buffer_encodes_completely() {
        let text = "é".repeat(10_000);
        let bytes = encode(&text, Encoding::WINDOWS_1252, false, Unmappable::Refuse).expect("ok");
        assert_eq!(bytes, vec![0xE9; 10_000]);
        let mixed = format!("{}日{}", "é".repeat(5_000), "é".repeat(5_000));
        let err = encode(&mixed, Encoding::WINDOWS_1252, false, Unmappable::Refuse).expect_err("refused");
        assert_eq!(err.offset, 10_000);
    }

    proptest! {
        #[test]
        fn any_text_round_trips_through_utf8_and_utf16(text in "\\PC{0,200}", bom: bool, which in 0usize..3) {
            let enc = [Encoding::UTF_8, Encoding::UTF_16LE, Encoding::UTF_16BE][which];
            let bytes = encode(&text, enc, bom, Unmappable::Refuse).expect("every character fits");
            let decoded = decode(&bytes, &det(enc, bom));
            prop_assert_eq!(&decoded.text, &text);
            prop_assert!(!decoded.lossy);
            if bom {
                let sniffed = sniff(&bytes);
                prop_assert_eq!((sniffed.encoding, sniffed.bom), (enc, true));
            }
        }

        /// The central promise: for any bytes whatsoever, either the read is flagged lossy or writing it back gives the same bytes.
        #[test]
        fn any_bytes_are_either_flagged_or_written_back_identically(
            bytes in proptest::collection::vec(any::<u8>(), 0..300),
            prefix in 0usize..6,
        ) {
            let prefixes: [&[u8]; 6] = [
                b"",
                b"\xEF\xBB\xBF",
                b"\xFF\xFE",
                b"\xFE\xFF",
                b"<?xml version=\"1.0\" encoding=\"windows-1252\"?>",
                b"<?xml version=\"1.0\" encoding=\"shift_jis\"?>",
            ];
            let mut input = prefixes[prefix].to_vec();
            input.extend_from_slice(&bytes);
            let detected = sniff(&input);
            let decoded = decode(&input, &detected);
            if !decoded.lossy {
                let again = encode(&decoded.text, detected.encoding, detected.bom, Unmappable::Refuse);
                prop_assert_eq!(again.as_deref(), Ok(input.as_slice()));
            }
        }

        #[test]
        fn encoding_in_pieces_equals_encoding_whole(text in "\\PC{0,120}", cut in 0usize..200, which in 0usize..4) {
            let enc = [Encoding::UTF_8, Encoding::UTF_16LE, Encoding::WINDOWS_1252, Encoding::from_label("shift_jis").expect("supported")][which];
            let mut at = cut.min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            let (a, b) = text.split_at(at);
            let whole = encode(&text, enc, false, Unmappable::CharRefs).expect("refs always fit");
            let pieces = encode_chunks([a, b], enc, false, Unmappable::CharRefs).expect("refs always fit");
            prop_assert_eq!(whole, pieces);
        }
    }
}
