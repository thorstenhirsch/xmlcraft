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

/// What typing a key does at one caret.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Plan {
    /// Replace the selection with the character.
    Literal,
    /// Write `text` (which has the typed character in it) at the caret, and leave the caret `caret` bytes into it.
    Write { text: String, caret: usize },
    /// The closing quote is already there: move over it.
    StepOver,
}

/// The longest stretch before the caret that is looked at to find the tag it is in.
const MAX_TAG_SCAN: usize = 64 * 1024;
/// A text longer than this before the caret is not parsed to find the element an end tag closes.
const MAX_PARSED_BEFORE: usize = 8 * 1024 * 1024;

fn quote_of(key: char) -> Option<Quote> {
    match key {
        '"' => Some(Quote::Double),
        '\'' => Some(Quote::Single),
        _ => None,
    }
}

fn quote_char(quote: Quote) -> char {
    match quote {
        Quote::Double => '"',
        Quote::Single => '\'',
    }
}

/// The name of the innermost element that is open at the end of `prefix` (a start tag with no end tag and no `/>`).
fn innermost_open_element(prefix: &str) -> Option<String> {
    use xmlcraft_syntax::{SyntaxKind, parse};
    let parsed = parse(prefix);
    let end = prefix.len();
    let mut innermost = None;
    for node in parsed.syntax().descendants().filter(|n| n.kind() == SyntaxKind::Element) {
        let reaches_the_end = usize::from(node.text_range().end()) == end;
        let closed = node.children().any(|n| n.kind() == SyntaxKind::EndTag);
        let start_tag = node.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
        let empty = start_tag.text().to_string().trim_end().ends_with("/>");
        // A start tag that is itself not finished (`<a b="x"`) opens nothing yet.
        let finished = start_tag.text().to_string().trim_end().ends_with('>');
        if reaches_the_end && !closed && !empty && finished {
            let name = start_tag.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
            innermost = Some(name.text().to_owned());
        }
    }
    innermost
}

impl Editor {
    /// What typing `text` on the keyboard does. It is [`Editor::type_text`], except for a few characters in XML:
    ///
    /// - A quote in a tag is paired (when [`crate::Settings::auto_pair`] is on): after `=` and before something that is not a name or a
    ///   quote, `"` writes `""` with the caret between; where the closing quote of that kind is next, typing it steps over it.
    /// - `>` that ends a start tag writes the end tag too (when [`crate::Settings::auto_close`] is on), caret between: `<a>` becomes
    ///   `<a>|</a>`; not for `/>`, for an end tag, or where the end tag is already there.
    /// - `/` after `<` completes the end tag of the element that is open (`</` becomes `</name>`).
    ///
    /// What an input method commits and what is pasted are text, not typing: they go through `type_text` and `paste`.
    pub fn type_input(&mut self, text: &str) -> bool {
        let mut chars = text.chars();
        let (Some(key), None) = (chars.next(), chars.next()) else { return self.type_text(text) };
        if !matches!(key, '"' | '\'' | '>' | '/') {
            return self.type_text(text);
        }
        let carets: Vec<Caret> = self.carets().to_vec();
        let plans: Vec<Plan> = carets.iter().map(|c| self.plan_key(c, key)).collect();
        if plans.iter().all(|p| *p == Plan::Literal) {
            return self.type_text(text);
        }
        let edits = coalesce(
            carets
                .iter()
                .zip(&plans)
                .filter_map(|(c, plan)| match plan {
                    Plan::Literal => Some(Edit::replace(c.start()..c.end(), text)),
                    Plan::Write { text, .. } => Some(Edit::insert(c.head(), text.clone())),
                    Plan::StepOver => None,
                })
                .collect(),
        );
        let after: Vec<Caret> = carets
            .iter()
            .zip(&plans)
            .map(|(c, plan)| match plan {
                Plan::Literal => Caret::at(map_offset(&edits, c.start(), Bias::Before) + text.len()),
                Plan::Write { caret, .. } => Caret::at(map_offset(&edits, c.head(), Bias::Before) + caret),
                Plan::StepOver => Caret::at(map_offset(&edits, c.head() + 1, Bias::Before)),
            })
            .collect();
        self.commit(edits, after, EditKind::Typing)
    }

    fn plan_key(&mut self, caret: &Caret, key: char) -> Plan {
        if !caret.is_empty() {
            return Plan::Literal;
        }
        match key {
            '>' if self.settings().auto_close => self.plan_close_tag(caret.head()),
            '/' if self.settings().auto_close => self.plan_end_tag(caret.head()),
            _ => match quote_of(key) {
                Some(quote) if self.settings().auto_pair => self.plan_quote(caret.head(), quote),
                _ => Plan::Literal,
            },
        }
    }

    fn plan_quote(&mut self, head: usize, quote: Quote) -> Plan {
        let mode = self.lexer.mode_at(&self.buffer, head);
        let next = self.buffer.char_at(head);
        match mode {
            Mode::AttrValue(open) if open == quote && next == Some(quote_char(quote)) => Plan::StepOver,
            Mode::Tag if self.follows_equals(head) && next.is_none_or(|c| !is_name_char(c) && c != '"' && c != '\'') => {
                let q = quote_char(quote);
                Plan::Write { text: format!("{q}{q}"), caret: 1 }
            }
            _ => Plan::Literal,
        }
    }

    /// `>` typed at `head`: if it ends a start tag, write it with the end tag after it.
    fn plan_close_tag(&mut self, head: usize) -> Plan {
        if self.lexer.mode_at(&self.buffer, head) != Mode::Tag {
            return Plan::Literal;
        }
        let Some(name) = self.name_of_open_tag(head) else { return Plan::Literal };
        // The end tag is there already (the `>` of a tag being edited in front of it).
        let end_tag = format!("</{name}");
        let mut end = head;
        for _ in 0..end_tag.chars().count() + 1 {
            end = self.buffer.next_char(end);
        }
        let following = self.buffer.slice(head..end).unwrap_or_default();
        if following.strip_prefix(&end_tag).is_some_and(|rest| rest.starts_with(['>', ' ', '\t', '\r', '\n'])) {
            return Plan::Literal;
        }
        Plan::Write { text: format!(">{end_tag}>"), caret: 1 }
    }

    /// The name of the start tag that the text before `head` is the inside of (`<a b="c"` is `a`), if it is a start tag that a `>` closes
    /// (not an end tag, a declaration or a tag that already ends in `/`).
    fn name_of_open_tag(&self, head: usize) -> Option<String> {
        let mut from = head.saturating_sub(MAX_TAG_SCAN);
        while from < head && self.buffer.char_at(from).is_none() {
            from += 1;
        }
        let before = self.buffer.slice(from..head).ok()?;
        let tag = &before[before.rfind('<')? + 1..];
        let name: String = tag.chars().take_while(|c| is_name_char(*c)).collect();
        let starts_with_name = tag.chars().next().is_some_and(xmlcraft_syntax::is_name_start_char);
        (starts_with_name && !name.is_empty() && !tag.ends_with('/')).then_some(name)
    }

    /// `/` typed at `head`: after `<` in content, write the name of the element that is open and the `>`.
    fn plan_end_tag(&mut self, head: usize) -> Plan {
        if head == 0 || head > MAX_PARSED_BEFORE || self.buffer.char_at(head - 1) != Some('<') {
            return Plan::Literal;
        }
        if self.lexer.mode_at(&self.buffer, head - 1) != Mode::Content {
            return Plan::Literal;
        }
        let Ok(before) = self.buffer.slice(0..head - 1) else { return Plan::Literal };
        match innermost_open_element(&before) {
            Some(name) => {
                let text = format!("/{name}>");
                Plan::Write { caret: text.len(), text }
            }
            None => Plan::Literal,
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

    // --- closing tags and indentation -----------------------------------------------------------------------------------------

    #[test]
    fn a_greater_than_that_ends_a_start_tag_writes_the_end_tag() {
        assert_eq!(typed("|", "<a>"), "<a>|</a>");
        assert_eq!(typed("|", "<a b=\"1\">"), "<a b=\"1\">|</a>", "a start tag with an attribute (the quotes are paired and stepped over)");
        assert_eq!(typed("|", "<r:a-b.c>"), "<r:a-b.c>|</r:a-b.c>");
        assert_eq!(typed("<a>|", "<b>"), "<a><b>|</b>");
        assert_eq!(typed("<a\n  b=\"1\"\n  |", ">"), "<a\n  b=\"1\"\n  >|</a>", "the tag goes over lines");
        assert_eq!(typed("<a b=\"x>y\"|", ">"), "<a b=\"x>y\">|</a>", "a `>` in a value does not end the tag, the one after the value does");
    }

    #[test]
    fn a_greater_than_anywhere_else_is_a_character() {
        assert_eq!(typed("</a|", ">"), "</a>|", "an end tag");
        assert_eq!(typed("<a/|", ">"), "<a/>|", "an empty-element tag");
        assert_eq!(typed("<a b=\"x|\"", ">"), "<a b=\"x>|\"", "in a value");
        assert_eq!(typed("a |", ">"), "a >|", "in text");
        assert_eq!(typed("<!-- <a|", ">"), "<!-- <a>|", "in a comment");
        assert_eq!(typed("<?xml version=\"1.0\"?|", ">"), "<?xml version=\"1.0\"?>|", "in the declaration");
        assert_eq!(typed("<!DOCTYPE a|", ">"), "<!DOCTYPE a>|");
        assert_eq!(typed("<|", ">"), "<>|", "no name");
        assert_eq!(typed("<![CDATA[ <a|", ">"), "<![CDATA[ <a>|");
        // The end tag is there already: the tag of `<a|</a>` is finished, not doubled.
        assert_eq!(typed("<a|</a>", ">"), "<a>|</a>");
        assert_eq!(typed("<a|</a >", ">"), "<a>|</a >");
        // Another name after it is not this tag's end tag.
        assert_eq!(typed("<a|</ab>", ">"), "<a>|</a></ab>");
    }

    #[test]
    fn a_selection_is_replaced_and_every_caret_decides_for_itself() {
        let mut e = at("<a|");
        e.set_selection(1, 2);
        e.type_input(">");
        assert_eq!(shown(&e), "<>|");
        let mut e = at("<a| <b|");
        e.type_input(">");
        assert_eq!(shown(&e), "<a>|</a> <b>|</b>");
        let mut e = at("x|\n<a|");
        e.type_input(">");
        assert_eq!(shown(&e), "x>|\n<a>|</a>", "the first caret is in text");
        assert!(e.undo(), "one step");
        assert_eq!(e.text(), "x\n<a");
    }

    #[test]
    fn a_slash_after_a_less_than_completes_the_end_tag_of_the_open_element() {
        assert_eq!(typed("<a><|", "/"), "<a></a>|");
        assert_eq!(typed("<a><b>x<|", "/"), "<a><b>x</b>|");
        assert_eq!(typed("<a><b>x</b><|", "/"), "<a><b>x</b></a>|", "the element that is still open");
        assert_eq!(typed("<a><b/><|", "/"), "<a><b/></a>|", "an empty-element tag opens nothing");
        assert_eq!(typed("<a>\n  <b>\n    x\n  <|", "/"), "<a>\n  <b>\n    x\n  </b>|", "over lines");
        assert_eq!(typed("<r:a x=\"1\"><|", "/"), "<r:a x=\"1\"></r:a>|");
        // Nothing open, in a comment, not after `<`, with a selection: a slash.
        assert_eq!(typed("<|", "/"), "</|");
        assert_eq!(typed("<a></a><|", "/"), "<a></a></|");
        assert_eq!(typed("<!-- <a><|", "/"), "<!-- <a></|");
        assert_eq!(typed("<a>x|", "/"), "<a>x/|");
        assert_eq!(typed("<a>1 < |", "/"), "<a>1 < /|");
        let mut e = at("<a><|x");
        e.set_selection(4, 5);
        e.type_input("/");
        assert_eq!(e.text(), "<a></", "the selection is replaced");
        // An unfinished start tag opens nothing yet.
        assert_eq!(typed("<a><b c=\"1\"<|", "/"), "<a><b c=\"1\"</|");
    }

    #[test]
    fn the_closing_aids_can_be_turned_off() {
        let mut e = Editor::with_settings("", Settings { auto_close: false, ..Settings::default() });
        for key in "<a><".chars() {
            e.type_input(&key.to_string());
        }
        e.type_input("/");
        assert_eq!(e.text(), "<a></");
    }

    #[test]
    fn enter_indents_after_a_start_tag_and_splits_a_pair() {
        let enter = |marked: &str| {
            let mut e = at(marked);
            e.insert_newline();
            shown(&e)
        };
        assert_eq!(enter("<a>|</a>"), "<a>\n  |\n</a>", "the content gets a line and the end tag its own");
        assert_eq!(enter("  <a>|</a>"), "  <a>\n    |\n  </a>");
        assert_eq!(enter("<a>|"), "<a>\n  |");
        assert_eq!(enter("  <a b=\"1\">|"), "  <a b=\"1\">\n    |");
        assert_eq!(enter("<a><b>|"), "<a><b>\n  |", "the last tag decides");
        assert_eq!(enter("<a>  |"), "<a>  \n  |", "white space after the tag does not matter");
        // No extra level: after text, an end tag, an empty-element tag, a comment, a declaration, outside a tag.
        assert_eq!(enter("<a>text|"), "<a>text\n|");
        assert_eq!(enter("<a>x</a>|"), "<a>x</a>\n|");
        assert_eq!(enter("<b/>|"), "<b/>\n|");
        assert_eq!(enter("<!-- c -->|"), "<!-- c -->\n|");
        assert_eq!(enter("<?xml version=\"1.0\"?>|"), "<?xml version=\"1.0\"?>\n|");
        assert_eq!(enter("<!-- <a>| -->"), "<!-- <a>\n| -->", "in a comment");
        assert_eq!(enter("<a b=\"x>|y\">"), "<a b=\"x>\n|y\">", "in a value");
        // A caret before something other than an end tag: only the extra level.
        assert_eq!(enter("<a>|<b/>"), "<a>\n  |<b/>");
        assert_eq!(enter("<a>|</b>"), "<a>\n  |\n</b>", "any end tag after it");
        // Several carets, and a selection is replaced.
        assert_eq!(enter("<a>|</a>\n<b>|</b>"), "<a>\n  |\n</a>\n<b>\n  |\n</b>");
        let mut e = at("<a>|xy|");
        e.set_selection(3, 5);
        e.insert_newline();
        assert_eq!(e.text(), "<a>\n  ");
    }

    #[test]
    fn enter_follows_the_documents_line_ending_and_indent_style() {
        let mut e = Editor::with_settings("<a></a>", Settings { eol: xmlcraft_text::eol::Eol::CrLf, use_tabs: true, ..Settings::default() });
        e.set_selection(3, 3);
        e.insert_newline();
        assert_eq!(e.text(), "<a>\r\n\t\r\n</a>");
        assert_eq!(e.primary().head(), 6, "on the middle line, after its tab");
        let mut e = Editor::with_settings("<a></a>", Settings { smart_indent: false, ..Settings::default() });
        e.set_selection(3, 3);
        e.insert_newline();
        assert_eq!(e.text(), "<a>\n</a>", "the setting turns it off");
    }
}
