//! What typing does beyond inserting the characters: the quotes of an attribute value.
//!
//! In a tag, a quote typed after `=` writes both quotes and leaves the caret between them; the closing quote typed next steps over the
//! one that is there; and Backspace between an empty pair removes both. The place is read with the lexer, so a quote in text, in a
//! comment or in a value opened by the other quote is only a character.

use xmlcraft_syntax::{Mode, Quote, is_name_char};
use xmlcraft_text::{Edit, EditKind};

use crate::Editor;
use crate::caret::Caret;
use crate::editor::{Bias, coalesce, map_offset};

/// What typing a quote does at one caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    /// Replace the selection with the character.
    Literal,
    /// Write the pair, caret between.
    Pair,
    /// The closing quote is already there: move over it.
    StepOver,
}

fn quote_of(text: &str) -> Option<Quote> {
    match text {
        "\"" => Some(Quote::Double),
        "'" => Some(Quote::Single),
        _ => None,
    }
}

fn quote_char(quote: Quote) -> char {
    match quote {
        Quote::Double => '"',
        Quote::Single => '\'',
    }
}

impl Editor {
    /// What typing `text` on the keyboard does. It is [`Editor::type_text`], except that typing a quote in a tag pairs it (when
    /// [`crate::Settings::auto_pair`] is on): after `=` and before something that is not a name or a quote, `"` writes `""` with the
    /// caret between; where the closing quote of that kind is next, typing it steps over it. What an input method commits and what is
    /// pasted are text, not typing: they go through `type_text` and `paste`.
    pub fn type_input(&mut self, text: &str) -> bool {
        let Some(quote) = quote_of(text) else { return self.type_text(text) };
        if !self.settings().auto_pair {
            return self.type_text(text);
        }
        let carets: Vec<Caret> = self.carets().to_vec();
        let plans: Vec<Plan> = carets.iter().map(|c| self.plan_quote(c, quote)).collect();
        if plans.iter().all(|p| *p == Plan::Literal) {
            return self.type_text(text);
        }
        let pair = format!("{0}{0}", quote_char(quote));
        let edits = coalesce(
            carets
                .iter()
                .zip(&plans)
                .filter_map(|(c, plan)| match plan {
                    Plan::Literal => Some(Edit::replace(c.start()..c.end(), text)),
                    Plan::Pair => Some(Edit::insert(c.head(), pair.clone())),
                    Plan::StepOver => None,
                })
                .collect(),
        );
        let after: Vec<Caret> = carets
            .iter()
            .zip(&plans)
            .map(|(c, plan)| match plan {
                Plan::Literal => Caret::at(map_offset(&edits, c.start(), Bias::Before) + text.len()),
                Plan::Pair => Caret::at(map_offset(&edits, c.head(), Bias::Before) + 1),
                Plan::StepOver => Caret::at(map_offset(&edits, c.head() + 1, Bias::Before)),
            })
            .collect();
        self.commit(edits, after, EditKind::Typing)
    }

    fn plan_quote(&mut self, caret: &Caret, quote: Quote) -> Plan {
        if !caret.is_empty() {
            return Plan::Literal;
        }
        let head = caret.head();
        let mode = self.lexer.mode_at(&self.buffer, head);
        let next = self.buffer.char_at(head);
        match mode {
            Mode::AttrValue(open) if open == quote && next == Some(quote_char(quote)) => Plan::StepOver,
            Mode::Tag if self.follows_equals(head) && next.is_none_or(|c| !is_name_char(c) && c != '"' && c != '\'') => Plan::Pair,
            _ => Plan::Literal,
        }
    }

    /// Whether the last thing before `offset` on its line, past any white space, is `=`.
    fn follows_equals(&self, offset: usize) -> bool {
        let line_start = self.buffer.line_start(self.buffer.line_of(offset)).unwrap_or(0);
        self.buffer.slice(line_start..offset).is_ok_and(|before| before.trim_end().ends_with('='))
    }

    /// The offsets of the carets that are between the quotes of an empty attribute value (`b="|"`): Backspace there takes both.
    pub(crate) fn carets_in_empty_quotes(&mut self) -> Vec<usize> {
        if !self.settings().auto_pair {
            return Vec::new();
        }
        let heads: Vec<usize> = self.carets().iter().filter(|c| c.is_empty()).map(|c| c.head()).collect();
        heads
            .into_iter()
            .filter(|&head| {
                let (Some(before), Some(after)) = (head.checked_sub(1).and_then(|at| self.buffer.char_at(at)), self.buffer.char_at(head)) else {
                    return false;
                };
                let quote = match before {
                    '"' => Quote::Double,
                    '\'' => Quote::Single,
                    _ => return false,
                };
                // The same quote on both sides, and the one before opened the value (the lexer is in the value, not past a closing quote).
                after == before && self.lexer.mode_at(&self.buffer, head) == Mode::AttrValue(quote)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Settings;

    /// An editor on `marked` (without its `|`s) with a caret at each `|`.
    fn at(marked: &str) -> Editor {
        let text = marked.replace('|', "");
        let mut carets = Vec::new();
        let mut removed = 0;
        for (i, c) in marked.char_indices() {
            if c == '|' {
                carets.push(xmlcraft_text::Selection::caret(i - removed));
                removed += 1;
            }
        }
        let mut e = Editor::new(&text);
        e.set_selections(&carets);
        e
    }

    /// The editor's text with a `|` at each caret.
    fn shown(e: &Editor) -> String {
        let mut text = e.text();
        for c in e.carets().iter().rev() {
            text.insert(c.end(), '|');
            if !c.is_empty() {
                text.insert(c.start(), '[');
            }
        }
        text
    }

    fn typed(marked: &str, keys: &str) -> String {
        let mut e = at(marked);
        for key in keys.chars() {
            e.type_input(&key.to_string());
        }
        shown(&e)
    }

    #[test]
    fn a_quote_after_an_equals_sign_is_paired_and_the_closing_one_stepped_over() {
        assert_eq!(typed("<a b=|>", "\""), "<a b=\"|\">");
        assert_eq!(typed("<a b=|>", "\"x"), "<a b=\"x|\">");
        assert_eq!(typed("<a b=|>", "\"x\""), "<a b=\"x\"|>", "the closing quote is the one that was written");
        assert_eq!(typed("<a b=|>", "'x'"), "<a b='x'|>");
        assert_eq!(typed("<a b=|", "\""), "<a b=\"|\"", "at the end of the text");
        assert_eq!(typed("<a b = |/>", "\""), "<a b = \"|\"/>", "white space after the equals sign, a slash after the caret");
        assert_eq!(typed("<a b=|>", "\"\""), "<a b=\"\"|>", "an empty value is typed with two quotes");
        // Two values in a row.
        assert_eq!(typed("<a b=| c=>", "\"1\""), "<a b=\"1\"| c=>");
    }

    #[test]
    fn a_quote_anywhere_else_is_only_a_character() {
        assert_eq!(typed("<a |>", "\""), "<a \"|>", "not after an equals sign");
        assert_eq!(typed("text |", "\""), "text \"|", "in content");
        assert_eq!(typed("<a b=|c>", "\""), "<a b=\"|c>", "in front of an unquoted value");
        assert_eq!(typed("<a b=|\"c\">", "\""), "<a b=\"|\"c\">", "in front of a quote that is already there");
        assert_eq!(typed("<a b='x|'>", "\""), "<a b='x\"|'>", "the other quote inside a value");
        assert_eq!(typed("<a b=\"x|y\">", "\""), "<a b=\"x\"|y\">", "a quote in the middle of a value closes it, as typed");
        assert_eq!(typed("<!-- b=| -->", "\""), "<!-- b=\"| -->", "in a comment");
        assert_eq!(typed("<a b=\"x\">|", "\""), "<a b=\"x\">\"|", "after the tag");
        assert_eq!(typed("<?xml version=|?>", "\""), "<?xml version=\"|?>", "in the XML declaration the lexer is not in a tag");
        // The value on an earlier line is not what the caret follows.
        assert_eq!(typed("<a b=\n|>", "\""), "<a b=\n\"|>");
    }

    #[test]
    fn a_selection_is_replaced_and_other_text_is_typed_as_it_is() {
        let mut e = at("<a b=|x|>");
        e.set_selection(5, 6);
        e.type_input("\"");
        assert_eq!(shown(&e), "<a b=\"|>");
        let mut e = at("<a b=|>");
        e.type_input("ab\"");
        assert_eq!(shown(&e), "<a b=ab\"|>", "more than one character is text, not typing");
        let mut e = at("<a b=|>");
        e.type_input("é");
        assert_eq!(shown(&e), "<a b=é|>");
    }

    #[test]
    fn every_caret_decides_for_itself() {
        let mut e = at("<a b=|/><c d=|/>text|");
        e.type_input("\"");
        assert_eq!(shown(&e), "<a b=\"|\"/><c d=\"|\"/>text\"|");
        e.type_input("x");
        assert_eq!(shown(&e), "<a b=\"x|\"/><c d=\"x|\"/>text\"x|");
        // Two carets in values: one steps over, the other pairs.
        let mut e = at("<a b=\"x|\" c=|/>");
        e.type_input("\"");
        assert_eq!(shown(&e), "<a b=\"x\"| c=\"|\"/>");
    }

    #[test]
    fn one_undo_takes_back_what_was_typed_and_paired() {
        let mut e = at("<a b=|/>");
        e.type_input("\"");
        e.type_input("x");
        e.type_input("\"");
        assert_eq!(shown(&e), "<a b=\"x\"|/>");
        e.undo();
        assert_eq!(e.text(), "<a b=/>", "typing is one step, pair included");
        e.redo();
        assert_eq!(e.text(), "<a b=\"x\"/>");
    }

    #[test]
    fn backspace_between_an_empty_pair_takes_both_quotes() {
        let back = |marked: &str| {
            let mut e = at(marked);
            e.backspace();
            shown(&e)
        };
        assert_eq!(back("<a b=\"|\">"), "<a b=|>");
        assert_eq!(back("<a b='|'>"), "<a b=|>");
        assert_eq!(back("<a b=\"x|\">"), "<a b=\"|\">", "a value with something in it loses a character");
        assert_eq!(back("<a b=\"\"|>"), "<a b=\"|>", "after the pair it is an ordinary Backspace");
        assert_eq!(back("text \"|\""), "text |\"", "in content a pair of quotes is two characters");
        assert_eq!(back("<a b=\"'|'\">"), "<a b=\"|'\">", "single quotes inside a double-quoted value");
        assert_eq!(back("<a b=\"|\" c=\"|\">"), "<a b=| c=|>", "every caret");
        let mut e = Editor::with_settings("<a b=\"\">", Settings { auto_pair: false, ..Settings::default() });
        e.set_selection(6, 6);
        e.backspace();
        assert_eq!(e.text(), "<a b=\">", "the setting turns it off");
    }

    #[test]
    fn the_setting_turns_pairing_off() {
        let mut e = Editor::with_settings("<a b=>", Settings { auto_pair: false, ..Settings::default() });
        e.set_selection(5, 5);
        e.type_input("\"");
        assert_eq!(e.text(), "<a b=\">");
    }

    #[test]
    fn what_is_typed_after_an_edit_elsewhere_is_read_from_the_new_text() {
        // The lexer remembers the modes of the lines it was asked about; an edit above must make it forget them.
        let mut e = at("<a>\n<b c=|/>");
        e.type_input("\"");
        assert_eq!(shown(&e), "<a>\n<b c=\"|\"/>", "a pair in a tag");
        e.backspace();
        assert_eq!(shown(&e), "<a>\n<b c=|/>");
        // Open a comment above: the line is in it now.
        e.place_caret(0, false);
        e.type_text("<!--");
        e.place_caret(e.text().find("c=").map_or(0, |i| i + 2), false);
        e.type_input("'");
        assert_eq!(e.text(), "<!--<a>\n<b c='/>", "inside a comment the quote is a character, not a pair");
        // And close it again by undoing: back in a tag.
        e.undo();
        e.undo();
        e.place_caret(e.text().find("c=").map_or(0, |i| i + 2), false);
        e.type_input("'");
        assert_eq!(e.text(), "<a>\n<b c=''/>");
    }
}
