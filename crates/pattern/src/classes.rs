//! Sets of characters: the categories, blocks and the multi-character escapes of XML Schema regular expressions.

use regex_syntax::ParserBuilder;
use regex_syntax::hir::{Class, ClassUnicode, ClassUnicodeRange, HirKind};

use crate::blocks::block;

/// The general categories `\p{..}` and `\P{..}` name (XML Schema Part 2, appendix F).
const CATEGORIES: &[&str] = &[
    "L", "Lu", "Ll", "Lt", "Lm", "Lo", "M", "Mn", "Mc", "Me", "N", "Nd", "Nl", "No", "P", "Pc", "Pd", "Ps", "Pe", "Pi", "Pf", "Po", "Z", "Zs", "Zl",
    "Zp", "S", "Sm", "Sc", "Sk", "So", "C", "Cc", "Cf", "Co", "Cn",
];

/// A set of code points, built from inclusive `(lo, hi)` ranges. Surrogates are not characters of a Rust string and are left out.
pub fn from_ranges(ranges: &[(u32, u32)]) -> ClassUnicode {
    let mut set = ClassUnicode::empty();
    for (lo, hi) in ranges {
        add_range(&mut set, *lo, *hi);
    }
    set
}

/// Add the code points `lo..=hi` to `set` (those that are characters).
pub fn add_range(set: &mut ClassUnicode, lo: u32, hi: u32) {
    const SURROGATES: (u32, u32) = (0xD800, 0xDFFF);
    let mut push = |lo: u32, hi: u32| {
        if let (Some(lo), Some(hi)) = (char::from_u32(lo), char::from_u32(hi))
            && lo <= hi
        {
            set.push(ClassUnicodeRange::new(lo, hi));
        }
    };
    if hi < SURROGATES.0 || lo > SURROGATES.1 {
        push(lo, hi);
    } else {
        if lo < SURROGATES.0 {
            push(lo, SURROGATES.0 - 1);
        }
        if hi > SURROGATES.1 {
            push(SURROGATES.1 + 1, hi);
        }
    }
}

/// The set of one character.
pub fn single(c: char) -> ClassUnicode {
    let mut set = ClassUnicode::empty();
    set.push(ClassUnicodeRange::new(c, c));
    set
}

/// The set a category name stands for (`L`, `Lu`, `Nd`, …); `None` for a name that is not one of the categories of the XSD syntax.
pub fn category(name: &str) -> Option<ClassUnicode> {
    if !CATEGORIES.contains(&name) {
        return None;
    }
    let hir = ParserBuilder::new().build().parse(&format!("\\p{{{name}}}")).ok()?;
    match hir.kind() {
        HirKind::Class(Class::Unicode(set)) => Some(set.clone()),
        _ => None,
    }
}

/// The set a block name (`IsBasicLatin`, with the `Is`) stands for.
pub fn block_set(name: &str) -> Option<ClassUnicode> {
    block(name.strip_prefix("Is")?).map(from_ranges)
}

/// The complement of a set, among all characters.
pub fn negated(set: &ClassUnicode) -> ClassUnicode {
    let mut out = set.clone();
    out.negate();
    out
}

/// `\s`: the four white space characters of XML.
pub fn space() -> ClassUnicode {
    from_ranges(&[(0x9, 0xA), (0xD, 0xD), (0x20, 0x20)])
}

/// `\i`: the characters a name may start with (XML 1.0, fifth edition, `NameStartChar`).
pub fn name_start() -> ClassUnicode {
    from_ranges(&NAME_START)
}

/// `\c`: the characters a name may have (`NameChar`).
pub fn name_char() -> ClassUnicode {
    let mut set = from_ranges(&NAME_START);
    for (lo, hi) in NAME_EXTRA {
        add_range(&mut set, lo, hi);
    }
    set
}

const NAME_START: [(u32, u32); 16] = [
    (0x3A, 0x3A),
    (0x41, 0x5A),
    (0x5F, 0x5F),
    (0x61, 0x7A),
    (0xC0, 0xD6),
    (0xD8, 0xF6),
    (0xF8, 0x2FF),
    (0x370, 0x37D),
    (0x37F, 0x1FFF),
    (0x200C, 0x200D),
    (0x2070, 0x218F),
    (0x2C00, 0x2FEF),
    (0x3001, 0xD7FF),
    (0xF900, 0xFDCF),
    (0xFDF0, 0xFFFD),
    (0x10000, 0xEFFFF),
];
/// What a name may have besides what it may start with: `-`, `.`, the digits, the middle dot and two ranges of combining marks.
const NAME_EXTRA: [(u32, u32); 5] = [(0x2D, 0x2E), (0x30, 0x39), (0xB7, 0xB7), (0x300, 0x36F), (0x203F, 0x2040)];

/// `\w`: every character except punctuation, separators and "other" (control, format, private use, unassigned).
pub fn word() -> Option<ClassUnicode> {
    let mut excluded = category("P")?;
    excluded.union(&category("Z")?);
    excluded.union(&category("C")?);
    let mut all = from_ranges(&[(0, 0x10FFFF)]);
    all.difference(&excluded);
    Some(all)
}

/// `\d`: decimal digits.
pub fn digit() -> Option<ClassUnicode> {
    category("Nd")
}

/// The set for the multi-character escape `\x` (`s S i I c C d D w W`); `None` for any other letter.
pub fn multi_char(letter: char) -> Option<ClassUnicode> {
    Some(match letter {
        's' => space(),
        'S' => negated(&space()),
        'i' => name_start(),
        'I' => negated(&name_start()),
        'c' => name_char(),
        'C' => negated(&name_char()),
        'd' => digit()?,
        'D' => negated(&digit()?),
        'w' => word()?,
        'W' => negated(&word()?),
        _ => return None,
    })
}
