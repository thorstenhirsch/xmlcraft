//! The pretty-printer and the minifier for JSON (and JSONC, JSON5): the same rule as for XML, only the white space between tokens
//! changes, as minimal edits. Strings, numbers, names, comments and the order of everything stay as they are.
//!
//! Pretty-printing puts each member of an object and each item of an array on a line of its own, indented one level deeper than the
//! bracket, `: ` after a name, nothing inside `{}` and `[]`. A comment that was on the line of the token before it stays there; one on
//! a line of its own stays on one. A blank line between members is kept (one at most). A text with a syntax error is refused.

use xmlcraft_json_syntax::{Dialect, Options, Severity, Token, TokenKind, parse, tokens};
use xmlcraft_text::Edit;

use crate::{BlankLines, FormatError, FormatOptions, Indent, MAX_INDENT};

/// Texts larger than this are not formatted (the tokens of a text are held in memory).
pub const MAX_FORMAT_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOKENS: usize = 4 * 1024 * 1024;
/// Containers nested deeper than this are not indented any further.
const MAX_INDENT_DEPTH: usize = 64;

/// The edits that pretty-print `text`, which is read as `dialect`. The line ending written is the one most of the line breaks of the
/// *result* use (as for XML), so that formatting the result again changes nothing, however the text mixes endings; a text with no line
/// break gets the option's.
pub fn pretty_print(text: &str, dialect: Dialect, options: &FormatOptions) -> Result<Vec<Edit>, FormatError> {
    use xmlcraft_text::eol::LineEndings;
    let (tokens, first) = checked_tokens(text, dialect)?;
    let own = LineEndings::count(text);
    let mut eol = if own.total() > 0 { own.dominant() } else { options.eol };
    let edits = layout(text, &tokens, first, options, eol.as_str())?;
    if own.is_mixed() {
        let result = crate::endings_after(text, &edits, own);
        if result.total() > 0 && result.dominant() != eol {
            eol = result.dominant();
            return layout(text, &tokens, first, options, eol.as_str());
        }
    }
    Ok(edits)
}

/// The edits that put the white space between `tokens` in its pretty-printed shape, with `eol` for line breaks.
fn layout(text: &str, tokens: &[Token], first: usize, options: &FormatOptions, eol: &str) -> Result<Vec<Edit>, FormatError> {
    let mut edits = Vec::new();
    let mut depth = 0usize;
    // What is written is no more than a few times the text: deep nesting cannot make it grow without bound.
    let mut budget = text.len().saturating_mul(4).saturating_add(1 << 20);
    let keep_blank_lines = options.blank_lines == BlankLines::Keep;
    for (i, token) in tokens.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| tokens.get(p));
        let gap = prev.map_or(first, |p| p.span.end)..token.span.start;
        let Some(old) = text.get(gap.clone()) else { continue };
        if token.kind == TokenKind::Close {
            depth = depth.saturating_sub(1);
        }
        let newlines = old.matches('\n').count().max(old.matches('\r').count().saturating_sub(old.matches("\r\n").count()));
        let line = |blank: bool| {
            let mut s = String::from(eol);
            if blank && keep_blank_lines && newlines >= 2 {
                s.push_str(eol);
            }
            s.push_str(&indent(options.indent, depth));
            s
        };
        let wanted: Option<String> = match (prev.map(|p| p.kind), token.kind) {
            (None, _) => Some(String::new()),
            (Some(TokenKind::Open), TokenKind::Close) => Some(String::new()),
            (_, TokenKind::Close) => Some(line(false)),
            (_, TokenKind::LineComment | TokenKind::BlockComment) => Some(if newlines > 0 { line(true) } else { " ".to_owned() }),
            (Some(TokenKind::Open), _) => Some(line(false)),
            (Some(TokenKind::LineComment), _) => Some(line(false)),
            (_, TokenKind::Colon | TokenKind::Comma) => Some(String::new()),
            (Some(TokenKind::Colon), _) => Some(" ".to_owned()),
            (Some(TokenKind::Comma), _) => Some(line(true)),
            (Some(TokenKind::BlockComment), _) if newlines > 0 => Some(line(true)),
            // A block comment before a value on its line: as it is.
            _ => None,
        };
        if token.kind == TokenKind::Open {
            depth = depth.saturating_add(1);
        }
        if let Some(wanted) = wanted
            && wanted != old
        {
            budget = budget
                .checked_sub(wanted.len())
                .ok_or_else(|| FormatError::Refused { message: "the result would be too large: the nesting is too deep to indent".to_owned() })?;
            edits.push(Edit::replace(gap, wanted));
        }
    }
    Ok(edits)
}

/// The edits that remove the white space between tokens. A line comment keeps the line break that ends it.
pub fn minify(text: &str, dialect: Dialect) -> Result<Vec<Edit>, FormatError> {
    let (tokens, first) = checked_tokens(text, dialect)?;
    let mut edits = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| tokens.get(p));
        let gap = prev.map_or(first, |p| p.span.end)..token.span.start;
        let Some(old) = text.get(gap.clone()) else { continue };
        let wanted = if prev.is_some_and(|p| p.kind == TokenKind::LineComment) { first_line_break(old) } else { "" };
        if wanted != old {
            edits.push(Edit::replace(gap, wanted));
        }
    }
    Ok(edits)
}

/// [`minify`] for JSON Lines: each line is a value of its own, so each is minified alone and the line breaks stay.
pub fn minify_lines(text: &str, dialect: Dialect) -> Result<Vec<Edit>, FormatError> {
    if text.len() > MAX_FORMAT_BYTES {
        return Err(FormatError::Refused { message: format!("the text is larger than the {MAX_FORMAT_BYTES} bytes that are formatted") });
    }
    let mut edits = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if !is_blank(content, dialect) {
            match minify(content, dialect) {
                Ok(found) => edits.extend(found.into_iter().map(|e| Edit::replace(e.range.start + start..e.range.end + start, e.text))),
                Err(FormatError::NotWellFormed { message, offset }) => return Err(FormatError::NotWellFormed { message, offset: offset + start }),
                Err(other) => return Err(other),
            }
        }
        start += line.len();
    }
    Ok(edits)
}

/// A line with nothing in it: only the white space the dialect knows (so other Unicode spaces are text, and a syntax error).
fn is_blank(line: &str, dialect: Dialect) -> bool {
    line.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r') || (dialect == Dialect::Json5 && c.is_whitespace()))
}

/// The first line break of `gap` as it is written (a gap after a line comment has one).
fn first_line_break(gap: &str) -> &str {
    if let Some(at) = gap.find(['\r', '\n']) {
        let rest = gap.get(at..).unwrap_or_default();
        return if rest.starts_with("\r\n") { "\r\n" } else { rest.get(..1).unwrap_or_default() };
    }
    ""
}

fn indent(style: Indent, depth: usize) -> String {
    let depth = depth.min(MAX_INDENT_DEPTH);
    match style {
        Indent::Tab => "\t".repeat(depth),
        Indent::Spaces(n) => " ".repeat(n.min(MAX_INDENT).saturating_mul(depth)),
    }
}

/// The tokens of a text that is valid in the dialect (a problem is the error), and where the first may start.
fn checked_tokens(text: &str, dialect: Dialect) -> Result<(Vec<Token>, usize), FormatError> {
    if text.len() > MAX_FORMAT_BYTES {
        return Err(FormatError::Refused { message: format!("the text is larger than the {MAX_FORMAT_BYTES} bytes that are formatted") });
    }
    let read = parse(text, &Options { dialect, ..Options::default() });
    if let Some(problem) = read.diagnostics.iter().find(|d| d.severity() == Severity::Error) {
        return Err(FormatError::NotWellFormed { message: problem.message(), offset: problem.span.start });
    }
    let (found, first, more) = tokens(text, dialect, MAX_TOKENS);
    if more {
        return Err(FormatError::Refused { message: "the text has too many tokens to format".to_owned() });
    }
    Ok((found, first))
}
