//! The Unicode blocks that `\p{IsName}` names (XML Schema Part 2, appendix G): the block names of the Unicode version the schema
//! language was written against, and the code points each covers. Regex engines have no such property, so the table is here.
//! (Names are the block names with spaces removed.)

/// Block names, sorted, with their ranges (inclusive code point ranges).
const BLOCKS: &[(&str, &[(u32, u32)])] = &[
    ("AlphabeticPresentationForms", &[(0xFB00, 0xFB4F)]),
    ("Arabic", &[(0x0600, 0x06FF)]),
    ("ArabicPresentationForms-A", &[(0xFB50, 0xFDFF)]),
    ("ArabicPresentationForms-B", &[(0xFE70, 0xFEFE)]),
    ("Armenian", &[(0x0530, 0x058F)]),
    ("Arrows", &[(0x2190, 0x21FF)]),
    ("BasicLatin", &[(0x0000, 0x007F)]),
    ("Bengali", &[(0x0980, 0x09FF)]),
    ("BlockElements", &[(0x2580, 0x259F)]),
    ("Bopomofo", &[(0x3100, 0x312F)]),
    ("BopomofoExtended", &[(0x31A0, 0x31BF)]),
    ("BoxDrawing", &[(0x2500, 0x257F)]),
    ("BraillePatterns", &[(0x2800, 0x28FF)]),
    ("ByzantineMusicalSymbols", &[(0x1D000, 0x1D0FF)]),
    ("CJKCompatibility", &[(0x3300, 0x33FF)]),
    ("CJKCompatibilityForms", &[(0xFE30, 0xFE4F)]),
    ("CJKCompatibilityIdeographs", &[(0xF900, 0xFAFF)]),
    ("CJKCompatibilityIdeographsSupplement", &[(0x2F800, 0x2FA1F)]),
    ("CJKRadicalsSupplement", &[(0x2E80, 0x2EFF)]),
    ("CJKSymbolsandPunctuation", &[(0x3000, 0x303F)]),
    ("CJKUnifiedIdeographs", &[(0x4E00, 0x9FFF)]),
    ("CJKUnifiedIdeographsExtensionA", &[(0x3400, 0x4DB5)]),
    ("CJKUnifiedIdeographsExtensionB", &[(0x20000, 0x2A6D6)]),
    ("Cherokee", &[(0x13A0, 0x13FF)]),
    ("CombiningDiacriticalMarks", &[(0x0300, 0x036F)]),
    ("CombiningHalfMarks", &[(0xFE20, 0xFE2F)]),
    ("CombiningMarksforSymbols", &[(0x20D0, 0x20FF)]),
    ("ControlPictures", &[(0x2400, 0x243F)]),
    ("CurrencySymbols", &[(0x20A0, 0x20CF)]),
    ("Cyrillic", &[(0x0400, 0x04FF)]),
    ("Deseret", &[(0x10400, 0x1044F)]),
    ("Devanagari", &[(0x0900, 0x097F)]),
    ("Dingbats", &[(0x2700, 0x27BF)]),
    ("EnclosedAlphanumerics", &[(0x2460, 0x24FF)]),
    ("EnclosedCJKLettersandMonths", &[(0x3200, 0x32FF)]),
    ("Ethiopic", &[(0x1200, 0x137F)]),
    ("GeneralPunctuation", &[(0x2000, 0x206F)]),
    ("GeometricShapes", &[(0x25A0, 0x25FF)]),
    ("Georgian", &[(0x10A0, 0x10FF)]),
    ("Gothic", &[(0x10330, 0x1034F)]),
    ("Greek", &[(0x0370, 0x03FF)]),
    ("GreekExtended", &[(0x1F00, 0x1FFF)]),
    ("GreekandCoptic", &[(0x0370, 0x03FF)]),
    ("Gujarati", &[(0x0A80, 0x0AFF)]),
    ("Gurmukhi", &[(0x0A00, 0x0A7F)]),
    ("HalfwidthandFullwidthForms", &[(0xFF00, 0xFFEF)]),
    ("HangulCompatibilityJamo", &[(0x3130, 0x318F)]),
    ("HangulJamo", &[(0x1100, 0x11FF)]),
    ("HangulSyllables", &[(0xAC00, 0xD7A3)]),
    ("Hebrew", &[(0x0590, 0x05FF)]),
    ("HighPrivateUseSurrogates", &[(0xDB80, 0xDBFF)]),
    ("HighSurrogates", &[(0xD800, 0xDB7F)]),
    ("Hiragana", &[(0x3040, 0x309F)]),
    ("IPAExtensions", &[(0x0250, 0x02AF)]),
    ("IdeographicDescriptionCharacters", &[(0x2FF0, 0x2FFF)]),
    ("Kanbun", &[(0x3190, 0x319F)]),
    ("KangxiRadicals", &[(0x2F00, 0x2FDF)]),
    ("Kannada", &[(0x0C80, 0x0CFF)]),
    ("Katakana", &[(0x30A0, 0x30FF)]),
    ("Khmer", &[(0x1780, 0x17FF)]),
    ("Lao", &[(0x0E80, 0x0EFF)]),
    ("Latin-1Supplement", &[(0x0080, 0x00FF)]),
    ("LatinExtended-A", &[(0x0100, 0x017F)]),
    ("LatinExtended-B", &[(0x0180, 0x024F)]),
    ("LatinExtendedAdditional", &[(0x1E00, 0x1EFF)]),
    ("LetterlikeSymbols", &[(0x2100, 0x214F)]),
    ("LowSurrogates", &[(0xDC00, 0xDFFF)]),
    ("Malayalam", &[(0x0D00, 0x0D7F)]),
    ("MathematicalAlphanumericSymbols", &[(0x1D400, 0x1D7FF)]),
    ("MathematicalOperators", &[(0x2200, 0x22FF)]),
    ("MiscellaneousSymbols", &[(0x2600, 0x26FF)]),
    ("MiscellaneousTechnical", &[(0x2300, 0x23FF)]),
    ("Mongolian", &[(0x1800, 0x18AF)]),
    ("MusicalSymbols", &[(0x1D100, 0x1D1FF)]),
    ("Myanmar", &[(0x1000, 0x109F)]),
    ("NumberForms", &[(0x2150, 0x218F)]),
    ("Ogham", &[(0x1680, 0x169F)]),
    ("OldItalic", &[(0x10300, 0x1032F)]),
    ("OpticalCharacterRecognition", &[(0x2440, 0x245F)]),
    ("Oriya", &[(0x0B00, 0x0B7F)]),
    ("PrivateUse", &[(0xE000, 0xF8FF), (0xF0000, 0xFFFFD), (0x100000, 0x10FFFD)]),
    ("Runic", &[(0x16A0, 0x16FF)]),
    ("Sinhala", &[(0x0D80, 0x0DFF)]),
    ("SmallFormVariants", &[(0xFE50, 0xFE6F)]),
    ("SpacingModifierLetters", &[(0x02B0, 0x02FF)]),
    ("Specials", &[(0xFEFF, 0xFEFF), (0xFFF0, 0xFFFD)]),
    ("SuperscriptsandSubscripts", &[(0x2070, 0x209F)]),
    ("Syriac", &[(0x0700, 0x074F)]),
    ("Tags", &[(0xE0000, 0xE007F)]),
    ("Tamil", &[(0x0B80, 0x0BFF)]),
    ("Telugu", &[(0x0C00, 0x0C7F)]),
    ("Thaana", &[(0x0780, 0x07BF)]),
    ("Thai", &[(0x0E00, 0x0E7F)]),
    ("Tibetan", &[(0x0F00, 0x0FFF)]),
    ("UnifiedCanadianAboriginalSyllabics", &[(0x1400, 0x167F)]),
    ("YiRadicals", &[(0xA490, 0xA4CF)]),
    ("YiSyllables", &[(0xA000, 0xA48F)]),
];

/// The code point ranges of the block `name` (`BasicLatin`, `Latin-1Supplement`: what follows `Is` in `\p{IsBasicLatin}`).
pub fn block(name: &str) -> Option<&'static [(u32, u32)]> {
    BLOCKS.binary_search_by(|(n, _)| (*n).cmp(name)).ok().and_then(|i| BLOCKS.get(i)).map(|(_, ranges)| *ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_are_sorted_and_unique_and_the_ranges_are_well_formed() {
        for pair in BLOCKS.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{} before {}", pair[0].0, pair[1].0);
        }
        for (name, ranges) in BLOCKS {
            assert!(!ranges.is_empty(), "{name}");
            for (lo, hi) in *ranges {
                assert!(lo <= hi && *hi <= 0x10FFFF, "{name}: {lo:X}-{hi:X}");
            }
        }
    }

    #[test]
    fn no_two_blocks_overlap_except_the_alias() {
        let mut all: Vec<(u32, u32, &str)> =
            BLOCKS.iter().filter(|(n, _)| *n != "GreekandCoptic").flat_map(|(n, r)| r.iter().map(move |(a, b)| (*a, *b, *n))).collect();
        all.sort();
        for pair in all.windows(2) {
            assert!(pair[0].1 < pair[1].0, "{} and {} overlap", pair[0].2, pair[1].2);
        }
    }

    #[test]
    fn a_block_is_found_by_its_name() {
        assert_eq!(block("BasicLatin"), Some(&[(0, 0x7F)][..]));
        assert_eq!(block("Latin-1Supplement"), Some(&[(0x80, 0xFF)][..]));
        assert_eq!(block("Greek"), block("GreekandCoptic"));
        assert_eq!(block("basiclatin"), None);
        assert_eq!(block("Nope"), None);
        assert_eq!(block(""), None);
    }
}
