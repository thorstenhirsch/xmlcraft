//! Quick fixes for the problems the JSON reader reports: each is a minimal edit of the text and changes nothing else. A problem with
//! more than one possible meaning (a missing value, a missing colon) has no fix: nothing is guessed.

use std::ops::Range;

use xmlcraft_json_syntax::{Dialect, TokenKind, tokens};
use xmlcraft_syntax::{Code, Diagnostic};
use xmlcraft_text::Edit;

use crate::language::{JsonFlavor, Language};
use crate::quickfix::QuickFix;

/// Texts with more tokens than this get no fix that needs them (the comment and control character fixes).
const MAX_TOKENS: usize = 4 * 1024 * 1024;

pub(crate) fn fixes(language: Language, text: &str, diagnostic: &Diagnostic, all: &[Diagnostic]) -> Vec<QuickFix> {
    let span = &diagnostic.span;
    let mut fixes = Vec::new();
    match diagnostic.code {
        Code::JsonTrailingComma => {
            if text.get(span.clone()) == Some(",") {
                fixes.push(QuickFix {
                    title: "Remove the trailing comma".to_owned(),
                    edits: vec![Edit::delete(span.clone())],
                    caret: Some(span.start),
                });
            }
        }
        Code::JsonExpectedComma => {
            if text.is_char_boundary(span.start) {
                fixes.push(QuickFix {
                    title: "Insert the missing comma".to_owned(),
                    edits: vec![Edit::insert(span.start, ",")],
                    caret: Some(span.start + 1),
                });
            }
        }
        Code::JsonSingleQuotes => {
            if let Some(double) = text.get(span.clone()).and_then(double_quoted) {
                let caret = span.start + double.len();
                fixes.push(QuickFix {
                    title: "Write the string in double quotes".to_owned(),
                    edits: vec![Edit::replace(span.clone(), double)],
                    caret: Some(caret),
                });
            }
        }
        Code::JsonUnquotedKey => {
            if let Some(name) = text.get(span.clone()).filter(|n| !n.is_empty() && !n.contains(['"', '\\']) && !n.chars().any(char::is_control)) {
                let quoted = format!("\"{name}\"");
                fixes.push(QuickFix {
                    title: "Put the name in double quotes".to_owned(),
                    caret: Some(span.start + quoted.len()),
                    edits: vec![Edit::replace(span.clone(), quoted)],
                });
            }
        }
        Code::JsonCommentNotAllowed => {
            let edits = comment_removals(text);
            if !edits.is_empty() {
                let caret = edits.first().map(|e| e.range.start);
                fixes.push(QuickFix { title: "Remove the comments (JSON has none)".to_owned(), edits, caret });
            }
        }
        Code::JsonUnclosedObject | Code::JsonUnclosedArray => fixes.extend(close_brackets(language, text, diagnostic, all)),
        Code::JsonControlCharacter => {
            let edits = control_escapes(text, span.start);
            if !edits.is_empty() {
                let caret = edits.first().map(|e| e.range.start);
                fixes.push(QuickFix { title: "Write the control characters of the string as escapes".to_owned(), edits, caret });
            }
        }
        _ => {}
    }
    fixes
}

/// A single-quoted string (`'it\'s "x"'`) as the double-quoted string with the same characters. None when it is not closed.
fn double_quoted(single: &str) -> Option<String> {
    let inner = single.strip_prefix('\'')?.strip_suffix('\'')?;
    if single.len() < 2 {
        return None;
    }
    let mut out = String::with_capacity(single.len() + 2);
    out.push('"');
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => match chars.next() {
                // `\'` is not an escape in JSON: the quote needs none inside double quotes.
                Some('\'') => out.push('\''),
                Some(next) => {
                    out.push('\\');
                    out.push(next);
                }
                None => return None,
            },
            other => out.push(other),
        }
    }
    out.push('"');
    Some(out)
}

/// The edits that remove every comment: the whole line when a comment is alone on it, otherwise the comment and the white space before
/// it (a line comment) or just the comment.
fn comment_removals(text: &str) -> Vec<Edit> {
    let (found, _, more) = tokens(text, Dialect::Jsonc, MAX_TOKENS);
    if more {
        return Vec::new();
    }
    let mut edits: Vec<Edit> = Vec::new();
    for token in found.iter().filter(|t| matches!(t.kind, TokenKind::LineComment | TokenKind::BlockComment)) {
        let Range { start, end } = token.span.clone();
        let before = text.get(..start).unwrap_or_default();
        let line_start = before.rfind(['\n', '\r']).map_or(0, |i| i + 1);
        let alone_before = before.get(line_start..).is_some_and(|b| b.chars().all(|c| c == ' ' || c == '\t'));
        let after = text.get(end..).unwrap_or_default();
        let rest_of_line = after.find(['\n', '\r']).unwrap_or(after.len());
        let blank_after = after.get(..rest_of_line).is_some_and(|a| a.chars().all(|c| c == ' ' || c == '\t'));
        let range = if alone_before && blank_after {
            // The whole line, with its break.
            let mut stop = end + rest_of_line;
            if text.get(stop..).is_some_and(|t| t.starts_with("\r\n")) {
                stop += 2;
            } else if text.get(stop..).is_some_and(|t| t.starts_with(['\n', '\r'])) {
                stop += 1;
            }
            line_start..stop
        } else if token.kind == TokenKind::LineComment || blank_after {
            before.trim_end_matches([' ', '\t']).len()..end
        } else {
            start..end
        };
        // (A comment inside the line of a previous removal is not a second one.)
        if edits.last().is_none_or(|last| range.start >= last.range.end) {
            edits.push(Edit::delete(range));
        }
    }
    edits
}

/// The closing brackets for this unclosed object or array and those opened inside it, innermost first, at the end of the text (of the
/// line, for JSON Lines).
fn close_brackets(language: Language, text: &str, diagnostic: &Diagnostic, all: &[Diagnostic]) -> Option<QuickFix> {
    let lines = language == Language::Json(JsonFlavor::Lines);
    let (from, at) = if lines {
        let start = text.get(..diagnostic.span.start)?.rfind('\n').map_or(0, |i| i + 1);
        let end = text.get(start..)?.find(['\n', '\r']).map_or(text.len(), |i| start + i);
        (start, end)
    } else {
        (0, text.len())
    };
    let mut open: Vec<&Diagnostic> = all
        .iter()
        .filter(|d| matches!(d.code, Code::JsonUnclosedObject | Code::JsonUnclosedArray))
        .filter(|d| d.span.start >= diagnostic.span.start && d.span.start >= from && d.span.start < at.max(from + 1))
        .collect();
    open.sort_by_key(|d| std::cmp::Reverse(d.span.start));
    let closers: String = open.iter().map(|d| if d.code == Code::JsonUnclosedObject { '}' } else { ']' }).collect();
    if closers.is_empty() || !text.is_char_boundary(at) {
        return None;
    }
    let title = if diagnostic.code == Code::JsonUnclosedObject { "Close the object" } else { "Close the array" };
    let title = format!("{title} at the end of the {}", if lines { "line" } else { "document" });
    Some(QuickFix { title, caret: Some(at + closers.len()), edits: vec![Edit::insert(at, closers)] })
}

/// The edits that write each control character of the string that holds `offset` as an escape.
fn control_escapes(text: &str, offset: usize) -> Vec<Edit> {
    let (found, _, more) = tokens(text, Dialect::Jsonc, MAX_TOKENS);
    if more {
        return Vec::new();
    }
    let Some(token) = found.iter().find(|t| t.kind == TokenKind::Scalar && t.span.start <= offset && offset < t.span.end) else {
        return Vec::new();
    };
    let Some(string) = text.get(token.span.clone()) else { return Vec::new() };
    if !string.starts_with(['"', '\'']) {
        return Vec::new();
    }
    string
        .char_indices()
        .filter(|(_, c)| (*c as u32) < 0x20)
        .map(|(i, c)| {
            let escape = match c {
                '\u{8}' => "\\b".to_owned(),
                '\t' => "\\t".to_owned(),
                '\n' => "\\n".to_owned(),
                '\u{c}' => "\\f".to_owned(),
                '\r' => "\\r".to_owned(),
                other => format!("\\u{:04x}", u32::from(other)),
            };
            Edit::replace(token.span.start + i..token.span.start + i + c.len_utf8(), escape)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json_analysis::analyze_json;
    use crate::quickfix::quick_fixes_for;

    fn problems(text: &str, flavor: JsonFlavor) -> Vec<Diagnostic> {
        analyze_json(text, 0, flavor).diagnostics
    }

    /// The text with the first fix of the first problem with `code` applied; and whether that problem is gone afterwards.
    fn fix(text: &str, flavor: JsonFlavor, code: Code) -> (String, bool) {
        let all = problems(text, flavor);
        let d = all.iter().find(|d| d.code == code).unwrap_or_else(|| panic!("{text:?} has no {code:?}: {all:?}"));
        let fixes = quick_fixes_for(Language::Json(flavor), text, d, &all);
        let first = fixes.first().unwrap_or_else(|| panic!("no fix for {code:?} in {text:?}"));
        let mut out = text.to_owned();
        for edit in first.edits.iter().rev() {
            out.replace_range(edit.range.clone(), &edit.text);
        }
        assert!(first.caret.is_none_or(|c| out.is_char_boundary(c)), "the caret is in the text");
        let gone = !problems(&out, flavor).iter().any(|p| p.code == code);
        (out, gone)
    }

    #[test]
    fn a_trailing_comma_is_removed_and_nothing_else_changes() {
        assert_eq!(fix("{\"a\": 1,}", JsonFlavor::Json, Code::JsonTrailingComma), ("{\"a\": 1}".to_owned(), true));
        assert_eq!(fix("[1, 2, ]\n", JsonFlavor::Json, Code::JsonTrailingComma), ("[1, 2 ]\n".to_owned(), true));
        assert_eq!(
            fix("{\"a\": [1,\n  2,\n  ],\n}", JsonFlavor::Json, Code::JsonTrailingComma),
            ("{\"a\": [1,\n  2\n  ],\n}".to_owned(), false),
            "the second comma is the next problem"
        );
    }

    #[test]
    fn a_missing_comma_is_inserted_where_the_value_ends() {
        assert_eq!(fix("{\"a\": 1 \"b\": 2}", JsonFlavor::Json, Code::JsonExpectedComma), ("{\"a\": 1, \"b\": 2}".to_owned(), true));
        assert_eq!(fix("[1 2]", JsonFlavor::Json, Code::JsonExpectedComma), ("[1, 2]".to_owned(), true));
        assert_eq!(fix("[\n  \"a\"\n  \"b\"\n]", JsonFlavor::Json, Code::JsonExpectedComma), ("[\n  \"a\",\n  \"b\"\n]".to_owned(), true));
    }

    #[test]
    fn a_single_quoted_string_is_written_in_double_quotes() {
        assert_eq!(fix("{\"a\": 'x'}", JsonFlavor::Json, Code::JsonSingleQuotes), ("{\"a\": \"x\"}".to_owned(), true));
        assert_eq!(fix("['it\\'s \"q\"']", JsonFlavor::Json, Code::JsonSingleQuotes), ("[\"it's \\\"q\\\"\"]".to_owned(), true));
        assert_eq!(fix("['a\\nb']", JsonFlavor::Json, Code::JsonSingleQuotes), ("[\"a\\nb\"]".to_owned(), true), "other escapes stay");
        assert_eq!(fix("{'k': 1}", JsonFlavor::Json, Code::JsonSingleQuotes).0, "{\"k\": 1}");
    }

    #[test]
    fn an_unquoted_key_is_put_in_quotes() {
        assert_eq!(fix("{a: 1, b2: 2}", JsonFlavor::Json, Code::JsonUnquotedKey), ("{\"a\": 1, b2: 2}".to_owned(), false), "one at a time");
        assert_eq!(fix("{ name : \"x\" }", JsonFlavor::Json, Code::JsonUnquotedKey), ("{ \"name\" : \"x\" }".to_owned(), true));
    }

    #[test]
    fn comments_are_removed_from_strict_json_and_a_line_alone_goes_whole() {
        let text = "// header\n{\n  // about a\n  \"a\": 1, // trailing\n  /* block */\n  \"b\": 2 /* inline */\n}\n";
        let (out, gone) = fix(text, JsonFlavor::Json, Code::JsonCommentNotAllowed);
        assert_eq!(out, "{\n  \"a\": 1,\n  \"b\": 2\n}\n");
        assert!(gone);
        // A block comment between two tokens leaves the white space around it.
        assert_eq!(fix("[1, /* x */ 2]", JsonFlavor::Json, Code::JsonCommentNotAllowed).0, "[1,  2]");
        // A comment inside a string is not a comment.
        assert_eq!(fix("{\"u\": \"http://x\"} // c", JsonFlavor::Json, Code::JsonCommentNotAllowed).0, "{\"u\": \"http://x\"}");
        // JSON with comments has nothing to fix.
        assert!(problems("{} // c", JsonFlavor::Jsonc).is_empty());
    }

    #[test]
    fn unclosed_brackets_are_closed_innermost_first_at_the_end() {
        let (out, gone) = fix("{\"a\": [1, {\"b\": 2", JsonFlavor::Json, Code::JsonUnclosedObject);
        assert_eq!(out, "{\"a\": [1, {\"b\": 2}]}");
        assert!(gone);
        // The inner one closes only itself.
        let text = "{\"a\": [1, 2";
        let all = problems(text, JsonFlavor::Json);
        let array = all.iter().find(|d| d.code == Code::JsonUnclosedArray).expect("an unclosed array");
        let fixes = quick_fixes_for(Language::Json(JsonFlavor::Json), text, array, &all);
        assert_eq!(fixes.first().map(|f| f.edits[0].text.as_str()), Some("]"));
        // JSON Lines: at the end of the line with the problem.
        let lines = "{\"a\": 1\n{\"b\": [2\n";
        let all = problems(lines, JsonFlavor::Lines);
        let first = all.iter().find(|d| d.code == Code::JsonUnclosedObject).expect("a problem");
        let fixes = quick_fixes_for(Language::Json(JsonFlavor::Lines), lines, first, &all);
        let mut out = lines.to_owned();
        out.insert_str(fixes[0].edits[0].range.start, &fixes[0].edits[0].text);
        assert_eq!(out, "{\"a\": 1}\n{\"b\": [2\n");
    }

    #[test]
    fn a_control_character_in_a_string_is_written_as_an_escape() {
        let (out, gone) = fix("{\"a\": \"x\ty\u{1}z\"}", JsonFlavor::Json, Code::JsonControlCharacter);
        assert_eq!(out, "{\"a\": \"x\\ty\\u0001z\"}");
        assert!(gone);
        // The characters of the next string are not touched.
        assert_eq!(fix("[\"a\u{7}\", \"b\tc\"]", JsonFlavor::Json, Code::JsonControlCharacter).0, "[\"a\\u0007\", \"b\tc\"]");
    }

    #[test]
    fn problems_that_have_more_than_one_meaning_get_no_fix_and_odd_texts_do_not_panic() {
        for text in ["{\"a\" 1}", "{\"a\": }", "[1,, 2]", "{\"a\": 1} {\"b\": 2}", "", "'", "{'", "[\"\u{1}", "{a", "/*", "[1 2 3", "{\"é\": 'ü\\'}"]
        {
            for flavor in [JsonFlavor::Json, JsonFlavor::Jsonc, JsonFlavor::Json5, JsonFlavor::Lines] {
                let all = problems(text, flavor);
                for d in &all {
                    for fix in quick_fixes_for(Language::Json(flavor), text, d, &all) {
                        let mut out = text.to_owned();
                        for edit in fix.edits.iter().rev() {
                            assert!(out.get(edit.range.clone()).is_some(), "{text:?}: {edit:?}");
                            out.replace_range(edit.range.clone(), &edit.text);
                        }
                        assert!(fix.caret.is_none_or(|c| out.is_char_boundary(c)), "{text:?}: caret {:?}", fix.caret);
                    }
                }
            }
        }
        let all = problems("{\"a\": }", JsonFlavor::Json);
        assert!(all.iter().all(|d| quick_fixes_for(Language::Json(JsonFlavor::Json), "{\"a\": }", d, &all).is_empty()));
    }
}
