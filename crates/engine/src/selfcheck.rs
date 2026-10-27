//! The self-check the fuzzer runs on one input (`xmlcraft-cli check-one`, `cargo xtask fuzz`): open the bytes as a document and hold
//! the engine to the promises it makes about any text.
//!
//! A panic, an abort or a hang is for the caller to notice (the fuzzer runs this in a child process). What is checked here are the
//! promises that a wrong answer breaks without any crash:
//!
//! - a document nobody edited is saved as the bytes it was opened from (one that is flagged lossy, because its bytes were not valid in
//!   its encoding, is not: saving it would make that permanent);
//! - the ranges the analysis reports (outline, problems, folds) are inside the text and on character boundaries;
//! - formatting a well-formed document keeps it well-formed, changes nothing but white space, is idempotent, and is undone by one undo;
//! - with `edit`: after a run of every kind of editing (typing, deleting, indenting, commenting, selecting, folding, replacing, undo and
//!   redo), the carets are inside the text and on character boundaries, undoing every step gives back the original text, and redoing
//!   every step gives back the edited one.

use std::ops::Range;

use xmlcraft_editor::find::{Matcher, Query};
use xmlcraft_editor::{Editor, Motion};
use xmlcraft_text::Buffer;

use crate::Document;
use crate::analysis::{Analysis, analyze};
use crate::folding::{FoldAction, apply};

/// How many editing steps `edit` mode takes.
const STEPS: usize = 160;

/// Check `bytes`; with `edit`, edit the document too (up to 64 KiB of text). `Err` says which promise was broken; a document too large
/// to open is one.
pub fn check(bytes: &[u8], edit: bool) -> Result<(), String> {
    let doc = Document::from_bytes(1, None, bytes, None).map_err(|e| e.to_string())?;
    let original = doc.editor().text();
    if !doc.is_lossy() {
        match doc.to_bytes() {
            Ok(saved) if saved == bytes => {}
            Ok(_) => return Err("saving a document nobody edited changed its bytes".to_owned()),
            Err(e) => return Err(format!("saving a document nobody edited failed: {e}")),
        }
    }
    // The analysis that is not guarded: a panic in it is the fuzzer's to see, not a "skipped" for the window.
    let analysis = analyze(&original, 1);
    check_analysis(&original, &analysis)?;
    if analysis.skipped.is_none() && analysis.error_count() == 0 {
        check_formatting(bytes, &original)?;
    }
    if edit {
        stress(bytes)?;
    }
    Ok(())
}

fn on_boundary(text: &str, offset: usize) -> bool {
    offset <= text.len() && text.is_char_boundary(offset)
}

fn in_text(text: &str, what: &str, range: &Range<usize>) -> Result<(), String> {
    if range.start <= range.end && on_boundary(text, range.start) && on_boundary(text, range.end) {
        Ok(())
    } else {
        Err(format!("{what} {range:?} is not a stretch of the {} bytes of text", text.len()))
    }
}

/// The ranges of an analysis are inside the text and on character boundaries, and the fold ranges are inside its lines.
fn check_analysis(text: &str, analysis: &Analysis) -> Result<(), String> {
    for diagnostic in &analysis.diagnostics {
        in_text(text, "a problem", &diagnostic.span)?;
    }
    for item in &analysis.outline {
        in_text(text, "an element", &item.range)?;
        in_text(text, "a start tag", &item.start_tag)?;
        in_text(text, "a name", &item.name_range)?;
        for range in item.end_tag.iter().chain(&item.end_name_range) {
            in_text(text, "an end tag", range)?;
        }
    }
    let lines = text.split('\n').count();
    for fold in &analysis.folds {
        if !(fold.first_line < fold.last_line && fold.last_line <= fold.end_line && fold.end_line < lines && fold.level >= 1) {
            return Err(format!("the fold {fold:?} is not inside the {lines} lines of the text"));
        }
    }
    // The pair of any offset is made of ranges of the text too.
    for offset in (0..=text.len()).step_by((text.len() / 64).max(1)) {
        if let Some(found) = analysis.matching_tag(offset) {
            for range in [&found.tag, &found.name, &found.partner, &found.partner_name] {
                in_text(text, "a tag of a pair", range)?;
            }
        }
    }
    Ok(())
}

/// `text` without the four characters XML calls white space (a no-break space is text).
fn without_white_space(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n')).collect()
}

fn is_well_formed(text: &str) -> bool {
    let analysis = analyze(text, 1);
    analysis.skipped.is_none() && analysis.error_count() == 0
}

/// Pretty-printing and minifying a well-formed document: still well-formed, only white space changed, nothing more to do the second
/// time, and one undo gives the text back.
fn check_formatting(bytes: &[u8], original: &str) -> Result<(), String> {
    for (name, minify) in [("pretty-printing", false), ("minifying", true)] {
        let Ok(mut doc) = Document::from_bytes(2, None, bytes, None) else { continue };
        let run = |doc: &mut Document| if minify { doc.minify() } else { doc.pretty_print() };
        // A document it refuses is its own business; what it does accept it must do right.
        let Ok(changed) = run(&mut doc) else { continue };
        let after = doc.editor().text();
        if !is_well_formed(&after) {
            return Err(format!("{name} a well-formed document made it not well-formed"));
        }
        if without_white_space(&after) != without_white_space(original) {
            return Err(format!("{name} changed more than white space"));
        }
        if run(&mut doc) != Ok(false) {
            return Err(format!("{name} is not idempotent: the second time it changed the text again"));
        }
        // The second run changed nothing, so the one undo is the first run's.
        if changed && (!doc.editor_mut().undo() || doc.editor().text() != original) {
            return Err(format!("undoing {name} did not give back the text"));
        }
    }
    Ok(())
}

/// xorshift64*: small, deterministic, good enough for choosing steps.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { usize::try_from(self.next() % (n as u64)).unwrap_or(0) }
    }
}

fn fxhash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, x| (h ^ u64::from(*x)).wrapping_mul(0x100_0000_01b3))
}

/// What a keyboard types, and what is pasted.
const TYPED: &[&str] =
    &["<", ">", "/", "\"", "'", "&", ";", "a", "Z", "1", " ", "-", "!", "?", "[", "]", "=", ":", "é", "日", "\u{1F600}", "\t", "ab"];
const PASTED: &[&str] = &["<x/>", "<a>\r\n</a>", "text", "<!-- c -->", "<![CDATA[ x ]]>", "&amp;", "\n\n", "é日"];

/// Whether `offset` is a place in the text a caret can be: inside it or at its end, and not in the middle of a character.
fn is_a_place_in(buffer: &Buffer, offset: usize) -> bool {
    offset == buffer.len() || (offset < buffer.len() && buffer.char_at(offset).is_some())
}

fn carets_are_in_the_text(editor: &Editor) -> Result<(), String> {
    let buffer = editor.buffer();
    for caret in editor.carets() {
        for offset in [caret.sel.anchor, caret.sel.head] {
            if !is_a_place_in(buffer, offset) {
                return Err(format!("a caret at {offset} is not on a character boundary of the {} bytes of text", buffer.len()));
            }
        }
    }
    Ok(())
}

/// The longest document `edit` mode edits: the history keeps 64 MiB and then drops its oldest steps, and a run of steps that each touch
/// much of a large document would pass that, so undoing all of it could not give back the original. 160 steps of at most this much
/// (twice, for the inverses) stay well within.
const STRESS_UP_TO: usize = 64 * 1024;

/// A run of editing steps, chosen by the bytes themselves; then undo and redo all of it.
fn stress(bytes: &[u8]) -> Result<(), String> {
    let Ok(mut doc) = Document::from_bytes(3, None, bytes, None) else { return Ok(()) };
    if doc.editor().buffer().len() > STRESS_UP_TO {
        return Ok(());
    }
    let original = doc.editor().text();
    let mut rng = Rng(fxhash(bytes) | 1);
    for step in 0..STEPS {
        let len = doc.editor().buffer().len();
        let (at, other) = (rng.below(len.saturating_add(1)), rng.below(len.saturating_add(1)));
        let kind = rng.below(24);
        match kind {
            0 => doc.editor_mut().set_selection(at, other),
            1..=3 => drop(doc.editor_mut().type_input(TYPED.get(rng.below(TYPED.len())).copied().unwrap_or("a"))),
            4 => drop(doc.editor_mut().insert_newline()),
            5 => drop(doc.editor_mut().backspace()),
            6 => drop(doc.editor_mut().delete_forward()),
            7 => drop(doc.editor_mut().indent()),
            8 => drop(doc.editor_mut().outdent()),
            9 => drop(doc.editor_mut().toggle_comment()),
            10 => drop(doc.editor_mut().expand_selection()),
            11 => drop(doc.editor_mut().shrink_selection()),
            12 => drop(doc.editor_mut().paste(PASTED.get(rng.below(PASTED.len())).copied().unwrap_or("x"))),
            13 => drop(doc.editor_mut().undo()),
            14 => drop(doc.editor_mut().redo()),
            15 => {
                let analysis = analyze(&doc.editor().text(), doc.editor().version());
                let action = match rng.below(5) {
                    0 => FoldAction::Fold,
                    1 => FoldAction::Unfold,
                    2 => FoldAction::FoldAll,
                    3 => FoldAction::UnfoldAll,
                    _ => FoldAction::Level(1 + rng.below(3)),
                };
                let _ = apply(doc.editor_mut(), &analysis.folds, action);
            }
            16..=17 => {
                // Typing in a tag name: both names change.
                doc.link_tag_names_by(analyze);
                let _ = doc.editor_mut().type_input(TYPED.get(rng.below(TYPED.len())).copied().unwrap_or("a"));
            }
            18 => {
                let end = doc.editor().buffer().next_char(doc.editor().buffer().next_char(at));
                if let Ok(word) = doc.editor().buffer().slice(at..end)
                    && let Ok(matcher) = Matcher::new(&Query::text(&word))
                {
                    let _ = doc.editor_mut().replace_all(&matcher, "xy", rng.below(2) == 0);
                }
            }
            19 => drop(doc.editor_mut().delete_word_backward()),
            20 => drop(doc.editor_mut().cut()),
            21 => doc.editor_mut().move_carets(
                match rng.below(6) {
                    0 => Motion::Left,
                    1 => Motion::Right,
                    2 => Motion::Up,
                    3 => Motion::Down,
                    4 => Motion::LineStart,
                    _ => Motion::DocEnd,
                },
                rng.below(2) == 0,
            ),
            22 => drop(if rng.below(2) == 0 { doc.pretty_print() } else { doc.minify() }),
            _ => drop(doc.editor_mut().select_next_occurrence()),
        }
        carets_are_in_the_text(doc.editor()).map_err(|e| format!("step {step} (kind {kind}): {e}"))?;
    }
    // Saving after the edits must not panic; it may refuse (a character the encoding cannot hold).
    drop(doc.to_bytes());
    // The whole history, from the last step that was redone or undone: to its end, back to its start, and to its end again.
    walk(&mut doc, true);
    let newest = doc.editor().text();
    walk(&mut doc, false);
    if doc.editor().text() != original {
        return Err("undoing every step did not give back the original text".to_owned());
    }
    walk(&mut doc, true);
    if doc.editor().text() != newest {
        return Err("redoing every step did not give back the text the history ends with".to_owned());
    }
    Ok(())
}

/// Redo (or undo) until there is nothing left; a history that never ends is a bug too, so the number of steps is capped.
fn walk(doc: &mut Document, redo: bool) {
    for _ in 0..100_000 {
        let editor = doc.editor_mut();
        let more = if redo { editor.can_redo() && editor.redo() } else { editor.can_undo() && editor.undo() };
        if !more {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLES: &[&[u8]] = &[
        b"",
        b"<a/>",
        b"<?xml version=\"1.0\"?>\n<a b=\"1\">\n  <c>text &amp; more</c>\n  <!-- x -->\n  <![CDATA[ y ]]>\n</a>\n",
        b"<a>\r\n  <b/>\r\n</a>\r\n",
        b"<a><b></a>",
        b"<!DOCTYPE r [\n<!ENTITY e \"v\">\n<!ELEMENT r ANY>\n]>\n<r>&e;</r>",
        b"\xEF\xBB\xBF<a>\xC3\xA9</a>",
        b"\xFF\xFE<\0a\0/\0>\0",
        b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>\xE9</a>",
        b"<a xml:space=\"preserve\">  <b/>  mixed <c/> content  </a>",
        b"<r:a xmlns:r=\"urn:x\" r:b=\"1\"><r:c/></r:a>",
        b"\xFF\xFF\xFF not xml at all",
        // Found by the fuzzer: mixed line endings, which pretty-printing used to change twice.
        b"<?l?><!--\r\n\r\n\r\n\r\n\r\n-->\n<foo xmlns:a=\"r\">\n\n<a a=\"\"/>\n\n</foo>",
    ];

    #[test]
    fn the_samples_keep_every_promise_with_and_without_editing() {
        for sample in SAMPLES {
            check(sample, true).unwrap_or_else(|e| panic!("{}: {e}", String::from_utf8_lossy(sample)));
        }
    }

    #[test]
    fn broken_ranges_and_folds_are_reported() {
        let text = "<a>\n<b/>\n</a>";
        let mut analysis = analyze(text, 1);
        assert_eq!(check_analysis(text, &analysis), Ok(()));
        analysis.folds.push(crate::folding::FoldRange {
            first_line: 1,
            last_line: 9,
            end_line: 9,
            level: 1,
            kind: crate::folding::FoldKind::Element,
        });
        assert!(check_analysis(text, &analysis).is_err(), "a fold past the last line");
        let mut analysis = analyze("<é/>", 1);
        if let Some(item) = analysis.outline.first_mut() {
            item.name_range = 2..3;
        }
        assert!(check_analysis("<é/>", &analysis).is_err(), "a range inside a character");
    }

    #[test]
    fn a_place_for_a_caret_is_inside_the_text_or_at_its_end_and_not_inside_a_character() {
        let buffer = Buffer::from_text("aé日");
        // `a` 0, `é` 1..3, `日` 3..6.
        let places: Vec<usize> = (0..=8).filter(|offset| is_a_place_in(&buffer, *offset)).collect();
        assert_eq!(places, [0, 1, 3, 6]);
        let mut editor = Editor::new("é");
        editor.set_selection(1, 1);
        assert_eq!(carets_are_in_the_text(&editor), Ok(()), "the editor puts a caret on a boundary itself");
    }

    #[test]
    fn white_space_is_all_that_formatting_may_change() {
        assert_eq!(without_white_space(" <a>\r\n\t<b/> </a>\n"), "<a><b/></a>");
        assert_eq!(without_white_space("a\u{A0}b\u{2003}"), "a\u{A0}b\u{2003}", "a no-break space and an em space are text, not white space");
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

        /// Whatever pieces of markup are put together, every promise holds.
        #[test]
        fn pieces_of_markup_keep_every_promise(
            pieces in proptest::collection::vec(proptest::sample::select(&[
                "<", ">", "</", "/>", "<a>", "</a>", "<b x=\"1\">", "</b>", "<c/>", " ", "\n", "\r\n", "\t", "=", "\"", "'", "&", "&amp;", ";", "<!--", "-->",
                "<![CDATA[", "]]>", "<?p", "?>", "<!DOCTYPE d [", "]>", "<!ENTITY e \"v\">", "é", "日", "text", "xml:space=\"preserve\"",
            ][..]), 0..40),
        ) {
            let text = pieces.concat();
            proptest::prop_assert_eq!(check(text.as_bytes(), true), Ok(()), "{:?}", text);
        }
    }
}
