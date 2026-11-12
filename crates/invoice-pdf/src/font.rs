//! The embedded faces of the invoice PDF: Inter Regular and SemiBold (SIL OFL 1.1, `assets/fonts/`, attributed in `ATTRIBUTION.toml`),
//! measured with their own advance widths and embedded as a subset.
//!
//! - Characters map to glyphs through the font's `cmap` (no shaping: Latin text, no ligatures or kerning). A character the face has
//!   no glyph for is drawn as the missing-glyph box (glyph 0) and reported, never replaced silently.
//! - The PDF font is a Type 0 font with an Identity-H encoding over a `CIDFontType2` whose CIDs are the glyph ids
//!   (`/CIDToGIDMap /Identity`), with `/W` widths for the glyphs used and a `/ToUnicode` map, so text can be extracted and searched
//!   (ISO 32000-1 §9.7, §9.10.3).
//! - The program is subset by keeping the outlines of the glyphs used (and the components of composite glyphs) and emptying the rest;
//!   glyph ids do not change, so the subset needs no renumbering. Tables the PDF does not use (layout, kerning, colour) are left out.

use std::collections::{BTreeMap, BTreeSet};

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, GlyphId, MetadataProvider};

static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

/// The face of a run of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Face {
    Regular,
    Bold,
}

/// A face's metrics and the glyphs used so far.
pub struct Font {
    pub face: Face,
    bytes: &'static [u8],
    units_per_em: f64,
    /// Advance widths by glyph id, in font units.
    advances: Vec<f64>,
    map: BTreeMap<char, u16>,
    pub ascent: f64,
    pub descent: f64,
    pub cap_height: f64,
    pub bbox: [f64; 4],
    pub italic_angle: f64,
    /// The glyphs drawn, with the text each stands for (for `/ToUnicode`).
    used: BTreeMap<u16, char>,
}

/// Why a font could not be read (the bundled faces always can; a test of a damaged face covers this path).
#[derive(Debug)]
pub struct FontError(pub String);

impl Font {
    pub fn load(face: Face) -> Result<Font, FontError> {
        let bytes = match face {
            Face::Regular => INTER_REGULAR,
            Face::Bold => INTER_SEMIBOLD,
        };
        Font::from_bytes(face, bytes)
    }

    fn from_bytes(face: Face, bytes: &'static [u8]) -> Result<Font, FontError> {
        let font = FontRef::new(bytes).map_err(|e| FontError(format!("the font cannot be read: {e}")))?;
        let location = LocationRef::default();
        let metrics = font.metrics(Size::unscaled(), location);
        let units_per_em = f64::from(metrics.units_per_em);
        if units_per_em <= 0.0 {
            return Err(FontError("the font has no units per em".into()));
        }
        let glyph_metrics = font.glyph_metrics(Size::unscaled(), location);
        let count = u32::from(metrics.glyph_count).min(u32::from(u16::MAX));
        let advances = (0..count).map(|g| f64::from(glyph_metrics.advance_width(GlyphId::new(g)).unwrap_or(0.0))).collect();
        let map = font
            .charmap()
            .mappings()
            .filter_map(|(cp, gid)| Some((char::from_u32(cp)?, u16::try_from(gid.to_u32()).ok()?)))
            .filter(|(_, g)| *g != 0)
            .collect();
        let scale = 1000.0 / units_per_em;
        let bounds = metrics.bounds.map_or([0.0, -250.0, 1000.0, 1000.0], |b| {
            [f64::from(b.x_min) * scale, f64::from(b.y_min) * scale, f64::from(b.x_max) * scale, f64::from(b.y_max) * scale]
        });
        Ok(Font {
            face,
            bytes,
            units_per_em,
            advances,
            map,
            ascent: f64::from(metrics.ascent) * scale,
            descent: f64::from(metrics.descent) * scale,
            cap_height: f64::from(metrics.cap_height.unwrap_or(metrics.ascent)) * scale,
            bbox: bounds,
            italic_angle: f64::from(metrics.italic_angle),
            used: BTreeMap::new(),
        })
    }

    /// The glyph of a character, if the face has one. A non-breaking space is drawn as a space.
    pub fn glyph(&self, c: char) -> Option<u16> {
        let c = if c == '\u{a0}' || c == '\u{202f}' { ' ' } else { c };
        self.map.get(&c).copied()
    }

    /// The width of a glyph in thousandths of an em.
    fn advance(&self, glyph: u16) -> f64 {
        self.advances.get(usize::from(glyph)).copied().unwrap_or(0.0) * 1000.0 / self.units_per_em
    }

    /// The width of `text` at `size` points (a character without a glyph is as wide as the missing-glyph box).
    pub fn width(&self, text: &str, size: f64) -> f64 {
        text.chars().map(|c| self.advance(self.glyph(c).unwrap_or(0))).sum::<f64>() * size / 1000.0
    }

    /// The characters of `text` the face has no glyph for.
    pub fn missing(&self, text: &str) -> Vec<char> {
        let mut out: Vec<char> = text.chars().filter(|c| !c.is_control() && self.glyph(*c).is_none()).collect();
        out.dedup();
        out
    }

    /// The glyph ids that draw `text`, two bytes each (Identity-H), remembering them for the subset.
    pub fn encode(&mut self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len().saturating_mul(2));
        for c in text.chars().filter(|c| !c.is_control()) {
            let g = self.glyph(c).unwrap_or(0);
            if g != 0 {
                self.used.entry(g).or_insert(if c == '\u{a0}' || c == '\u{202f}' { ' ' } else { c });
            }
            out.extend_from_slice(&g.to_be_bytes());
        }
        out
    }

    /// The glyph ids that draw `text`, without remembering them (for tests that look for a run in a content stream).
    pub fn glyph_bytes(&self, text: &str) -> Vec<u8> {
        text.chars().filter(|c| !c.is_control()).flat_map(|c| self.glyph(c).unwrap_or(0).to_be_bytes()).collect()
    }

    /// The `/W` array entries: `[first [w …]]` for each run of consecutive glyphs used.
    pub fn widths(&self) -> Vec<(u16, Vec<f64>)> {
        let mut runs: Vec<(u16, Vec<f64>)> = Vec::new();
        for g in self.used.keys().copied().chain(std::iter::once(0)) {
            let w = (self.advance(g) * 1000.0).round() / 1000.0;
            match runs.last_mut() {
                Some((first, ws)) if usize::from(*first).saturating_add(ws.len()) == usize::from(g) => ws.push(w),
                _ => runs.push((g, vec![w])),
            }
        }
        runs.sort_by_key(|(first, _)| *first);
        runs
    }

    /// The `/ToUnicode` CMap of the glyphs used (ISO 32000-1 §9.10.3).
    pub fn to_unicode(&self) -> Vec<u8> {
        let mut s = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
        );
        let entries: Vec<(&u16, &char)> = self.used.iter().collect();
        for chunk in entries.chunks(100) {
            s.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (g, c) in chunk {
                let utf16: String = c.encode_utf16(&mut [0u16; 2]).iter().map(|u| format!("{u:04X}")).collect();
                s.push_str(&format!("<{g:04X}> <{utf16}>\n"));
            }
            s.push_str("endbfchar\n");
        }
        s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        s.into_bytes()
    }

    /// The PostScript name of the subset: a six-letter tag from the glyphs used, then the face's name (ISO 32000-1 §9.6.4).
    pub fn subset_name(&self) -> String {
        let mut h: u32 = 2_166_136_261;
        for g in self.used.keys() {
            for b in g.to_be_bytes() {
                h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
            }
        }
        let tag: String = (0..6)
            .map(|i| {
                let v = (h >> (i * 5)) % 26;
                char::from(b'A'.saturating_add(u8::try_from(v).unwrap_or(0)))
            })
            .collect();
        let base = match self.face {
            Face::Regular => "Inter-Regular",
            Face::Bold => "Inter-SemiBold",
        };
        format!("{tag}+{base}")
    }

    /// The font program with only the glyphs used.
    pub fn subset(&self) -> Result<Vec<u8>, FontError> {
        let keep: BTreeSet<u16> = self.used.keys().copied().chain(std::iter::once(0)).collect();
        subset_truetype(self.bytes, &keep)
    }
}

// ---- TrueType subsetting (OpenType spec: the table directory, `head`, `maxp`, `loca`, `glyf`) ----

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    let b = data.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes([*b.first()?, *b.get(1)?]))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([*b.first()?, *b.get(1)?, *b.get(2)?, *b.get(3)?]))
}

fn table<'a>(font: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let count = usize::from(u16_at(font, 4)?);
    (0..count).find_map(|i| {
        let rec = 12usize.checked_add(i.checked_mul(16)?)?;
        if font.get(rec..rec.checked_add(4)?)? != tag {
            return None;
        }
        let offset = usize::try_from(u32_at(font, rec.checked_add(8)?)?).ok()?;
        let length = usize::try_from(u32_at(font, rec.checked_add(12)?)?).ok()?;
        font.get(offset..offset.checked_add(length)?)
    })
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut word = [0u8; 4];
        for (d, s) in word.iter_mut().zip(c) {
            *d = *s;
        }
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// The tables a PDF TrueType program keeps (ISO 32000-1 §9.9: `cmap` is not needed with a `/CIDToGIDMap`, but harmless and small).
const KEPT: [&[u8; 4]; 12] = [b"OS/2", b"cmap", b"cvt ", b"fpgm", b"glyf", b"head", b"hhea", b"hmtx", b"loca", b"maxp", b"name", b"post"];

/// The most composite-glyph references followed (a composite cycle in a damaged font ends here).
const MAX_COMPONENTS: usize = 65_536;

fn subset_truetype(font: &[u8], keep: &BTreeSet<u16>) -> Result<Vec<u8>, FontError> {
    let err = |what: &str| FontError(format!("the font program cannot be subset: {what}"));
    let head = table(font, b"head").ok_or_else(|| err("no head table"))?;
    let maxp = table(font, b"maxp").ok_or_else(|| err("no maxp table"))?;
    let loca = table(font, b"loca").ok_or_else(|| err("no loca table"))?;
    let glyf = table(font, b"glyf").ok_or_else(|| err("no glyf table"))?;
    let long = u16_at(head, 50).ok_or_else(|| err("short head table"))? == 1;
    let glyphs = usize::from(u16_at(maxp, 4).ok_or_else(|| err("short maxp table"))?);
    let offset = |g: usize| -> Option<usize> {
        if long { usize::try_from(u32_at(loca, g.checked_mul(4)?)?).ok() } else { usize::from(u16_at(loca, g.checked_mul(2)?)?).checked_mul(2) }
    };
    let outline = |g: usize| -> Option<&[u8]> { glyf.get(offset(g)?..offset(g.checked_add(1)?)?) };
    // The glyphs kept, with the components of composite glyphs (glyf: flags, glyph index, arguments, transform).
    let mut all: BTreeSet<u16> = keep.iter().copied().filter(|g| usize::from(*g) < glyphs).collect();
    let mut todo: Vec<u16> = all.iter().copied().collect();
    let mut steps = 0usize;
    while let Some(g) = todo.pop() {
        let Some(data) = outline(usize::from(g)) else { continue };
        if data.len() < 10 || u16_at(data, 0).is_none_or(|n| n & 0x8000 == 0) {
            continue;
        }
        let mut at = 10usize;
        loop {
            steps = steps.saturating_add(1);
            if steps > MAX_COMPONENTS {
                return Err(err("too many composite glyph components"));
            }
            let (Some(flags), Some(component)) = (u16_at(data, at), u16_at(data, at.saturating_add(2))) else { break };
            if usize::from(component) < glyphs && all.insert(component) {
                todo.push(component);
            }
            let args = if flags & 0x0001 != 0 { 4 } else { 2 };
            let transform = if flags & 0x0008 != 0 {
                2
            } else if flags & 0x0040 != 0 {
                4
            } else if flags & 0x0080 != 0 {
                8
            } else {
                0
            };
            at = at.saturating_add(4).saturating_add(args).saturating_add(transform);
            if flags & 0x0020 == 0 {
                break;
            }
        }
    }
    // The new glyf (each kept outline, padded to four bytes) and a long loca.
    let mut new_glyf = Vec::new();
    let mut new_loca = Vec::with_capacity(glyphs.saturating_add(1).saturating_mul(4));
    for g in 0..glyphs {
        let start = u32::try_from(new_glyf.len()).map_err(|_| err("glyf too large"))?;
        new_loca.extend_from_slice(&start.to_be_bytes());
        if u16::try_from(g).is_ok_and(|g| all.contains(&g))
            && let Some(data) = outline(g)
        {
            new_glyf.extend_from_slice(data);
            while new_glyf.len() % 4 != 0 {
                new_glyf.push(0);
            }
        }
    }
    let end = u32::try_from(new_glyf.len()).map_err(|_| err("glyf too large"))?;
    new_loca.extend_from_slice(&end.to_be_bytes());
    let mut new_head = head.to_vec();
    // indexToLocFormat: long; checkSumAdjustment is set below.
    if let Some(slot) = new_head.get_mut(50..52) {
        slot.copy_from_slice(&1u16.to_be_bytes());
    }
    if let Some(slot) = new_head.get_mut(8..12) {
        slot.copy_from_slice(&[0; 4]);
    }
    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = Vec::new();
    for tag in KEPT {
        let data = match tag {
            b"glyf" => new_glyf.clone(),
            b"loca" => new_loca.clone(),
            b"head" => new_head.clone(),
            _ => match table(font, tag) {
                Some(t) => t.to_vec(),
                None => continue,
            },
        };
        tables.push((tag, data));
    }
    let count = u16::try_from(tables.len()).map_err(|_| err("too many tables"))?;
    let mut power = 1u16;
    let mut log = 0u16;
    while power.saturating_mul(2) <= count {
        power = power.saturating_mul(2);
        log = log.saturating_add(1);
    }
    let search = power.saturating_mul(16);
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&search.to_be_bytes());
    out.extend_from_slice(&log.to_be_bytes());
    out.extend_from_slice(&count.saturating_mul(16).saturating_sub(search).to_be_bytes());
    let mut offset_now = 12usize.saturating_add(tables.len().saturating_mul(16));
    let mut body = Vec::new();
    let mut head_at = None;
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&u32::try_from(offset_now).map_err(|_| err("font too large"))?.to_be_bytes());
        out.extend_from_slice(&u32::try_from(data.len()).map_err(|_| err("table too large"))?.to_be_bytes());
        if *tag == b"head" {
            head_at = Some(offset_now);
        }
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset_now = 12usize.saturating_add(tables.len().saturating_mul(16)).saturating_add(body.len());
    }
    out.extend_from_slice(&body);
    if let Some(at) = head_at {
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        if let Some(slot) = out.get_mut(at.saturating_add(8)..at.saturating_add(12)) {
            slot.copy_from_slice(&adjustment.to_be_bytes());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_faces_measure_and_map_latin_text() {
        let mut regular = Font::load(Face::Regular).expect("Inter Regular");
        let bold = Font::load(Face::Bold).expect("Inter SemiBold");
        assert!(regular.width("Rechnung", 10.0) > 30.0 && regular.width("Rechnung", 10.0) < 60.0);
        assert!(bold.width("Rechnung", 10.0) > regular.width("Rechnung", 10.0), "SemiBold is wider");
        assert_eq!(regular.width("", 10.0), 0.0);
        assert!(regular.missing("Grüße, Œuvre, Ελλάδα, Москва €").is_empty());
        assert_eq!(regular.missing("漢字"), vec!['漢', '字']);
        let bytes = regular.encode("Ab");
        assert_eq!(bytes.len(), 4);
        assert_eq!(regular.widths().iter().map(|(_, w)| w.len()).sum::<usize>(), 3, "A, b and the missing glyph");
        let cmap = String::from_utf8(regular.to_unicode()).expect("ascii");
        assert!(cmap.contains("<0041>") && cmap.contains("<0062>"), "{cmap}");
    }

    #[test]
    fn a_subset_keeps_the_glyphs_used_and_is_a_font_again() {
        let mut f = Font::load(Face::Regular).expect("Inter");
        f.encode("Rechnung 123,45 € Ä");
        let subset = f.subset().expect("subset");
        assert!(subset.len() < INTER_REGULAR.len() / 4, "{} of {}", subset.len(), INTER_REGULAR.len());
        let read = FontRef::new(&subset).expect("the subset is read again");
        let g = read.charmap().map('R').expect("R is mapped");
        let outlines = read.outline_glyphs();
        assert!(outlines.get(g).is_some());
        assert_eq!(checksum(&subset), 0xB1B0_AFBA, "the whole-font checksum is right");
        // A damaged program is an error, not a panic.
        assert!(subset_truetype(b"not a font", &BTreeSet::from([0])).is_err());
        assert!(subset_truetype(&subset[..100], &BTreeSet::from([0])).is_err());
    }
}
