//! The re-indenter for YAML. White space is the structure of YAML, so this changes only the white space it can prove means nothing
//! more than "this is one level deeper": the indentation of the lines of block mappings and block sequences (to the document's indent
//! unit), the indentation of the content of block scalars (as a whole, so what is indented relatively inside stays), and the white
//! space at the end of lines. Everything else stays byte for byte: comments, anchors, aliases, tags, quoting, flow collections
//! (`[…]`, `{…}`; a line inside one moves with the line it starts on), the line breaks, the blank lines and the space inside a line.
//!
//! The edits are per line. A text with a syntax error is refused, and so is a result that does not read as the same events as the text
//! (the parser's view of anchors, tags, scalar styles and values), so a bug here can only ever refuse, never change a document.
//!
//! Idempotent: formatting the result again changes nothing.

use xmlcraft_text::Edit;
use xmlcraft_yaml_syntax::{Layout, layout};

use crate::json::MAX_FORMAT_BYTES;
use crate::{FormatError, FormatOptions, Indent, IndentSequences, MAX_INDENT};

/// The edits that re-indent `text` (a YAML stream) in the way of `options`: `options.indent` is the unit of a level (a tab setting
/// means two spaces, since YAML cannot be indented with tabs), `options.indent_sequences` says where a sequence that is the value of a
/// key goes. The line breaks and the blank lines are left as they are.
pub fn pretty_print(text: &str, options: &FormatOptions) -> Result<Vec<Edit>, FormatError> {
    if text.len() > MAX_FORMAT_BYTES {
        return Err(FormatError::Refused { message: format!("the text is larger than the {MAX_FORMAT_BYTES} bytes that are formatted") });
    }
    let found = layout(text).map_err(|e| FormatError::NotWellFormed { message: e.message, offset: e.offset })?;
    let unit = match options.indent {
        Indent::Spaces(n) => n.clamp(1, MAX_INDENT),
        Indent::Tab => 2,
    };
    let edits = Plan::new(text, &found, unit, options.indent_sequences).run()?;
    // The result must read as the same document.
    let result = xmlcraft_text::apply_edits(text, &edits);
    match layout(&result) {
        Ok(after) if after.fingerprint == found.fingerprint => Ok(edits),
        _ => {
            Err(FormatError::Refused { message: "this YAML cannot be re-indented without a risk of changing it, so it is left as it is".to_owned() })
        }
    }
}

#[derive(Clone, Copy)]
struct Line {
    start: usize,
    /// Before the line break.
    content_end: usize,
    /// After the line break.
    end: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Map,
    Seq,
}

/// A level of block structure that is open: the column its entries are at in the text, and the column they are given.
#[derive(Clone, Copy)]
struct Level {
    old: usize,
    new: usize,
    kind: Kind,
}

/// What is done to a line.
#[derive(Clone, Copy, Default)]
struct Action {
    /// The number of spaces the line is indented with after (none: left as it is).
    lead: Option<usize>,
    /// Remove the white space at the end.
    trim: bool,
}

struct Plan<'a> {
    text: &'a str,
    layout: &'a Layout,
    lines: Vec<Line>,
    unit: usize,
    style: IndentSequences,
    actions: Vec<Action>,
    /// The shift (new indentation minus old) of each line that was placed.
    shifts: Vec<isize>,
}

fn split_lines(text: &str) -> Vec<Line> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = if text.starts_with('\u{FEFF}') { '\u{FEFF}'.len_utf8() } else { 0 };
    let mut at = start;
    while at < bytes.len() {
        match bytes.get(at) {
            Some(b'\n') => {
                lines.push(Line { start, content_end: at, end: at + 1 });
                at += 1;
                start = at;
            }
            Some(b'\r') => {
                let end = if bytes.get(at + 1) == Some(&b'\n') { at + 2 } else { at + 1 };
                lines.push(Line { start, content_end: at, end });
                at = end;
                start = at;
            }
            _ => at += 1,
        }
    }
    if start < bytes.len() {
        lines.push(Line { start, content_end: bytes.len(), end: bytes.len() });
    }
    lines
}

impl<'a> Plan<'a> {
    fn new(text: &'a str, layout: &'a Layout, unit: usize, style: IndentSequences) -> Self {
        let lines = split_lines(text);
        let count = lines.len();
        Self { text, layout, lines, unit, style, actions: vec![Action::default(); count], shifts: vec![0; count] }
    }

    fn content(&self, line: &Line) -> &'a str {
        self.text.get(line.start..line.content_end).unwrap_or_default()
    }

    fn run(mut self) -> Result<Vec<Edit>, FormatError> {
        let mut budget = self.text.len().saturating_mul(4).saturating_add(1 << 20);
        let mut levels: Vec<Level> = Vec::new();
        // Comment lines whose indentation waits for the next line of structure: (line, old indentation).
        let mut pending: Vec<(usize, usize)> = Vec::new();
        // The block scalar whose content the lines are: (old indentation, new indentation, end).
        let mut active: Option<(usize, usize, usize)> = None;
        // The last line of structure (where a block scalar's header is).
        let mut last_structure: Option<usize> = None;
        let (mut next_multi, mut next_block) = (0usize, 0usize);
        let layout = self.layout;
        for index in 0..self.lines.len() {
            let Some(&line) = self.lines.get(index) else { break };
            let content = self.content(&line);
            let lead = content.bytes().take_while(|b| *b == b' ').count();
            let rest = content.get(lead..).unwrap_or_default();
            let blank = rest.bytes().all(|b| b == b' ' || b == b'\t');
            // 1. A line inside a scalar or flow collection that runs over lines moves with the line it starts on.
            while layout.multiline.get(next_multi).is_some_and(|m| m.end <= line.start) {
                next_multi += 1;
            }
            let node = layout.multiline.get(next_multi).filter(|m| m.start < line.start).cloned();
            // Whether the break of this line is inside a node (the white space before it is part of the document).
            let inside = layout.multiline.get(next_multi).is_some_and(|m| m.start < line.content_end && line.content_end < m.end);
            if let Some(node) = node {
                let owner = self.lines.partition_point(|l| l.start <= node.start).saturating_sub(1);
                let shift = self.shifts.get(owner).copied().unwrap_or(0);
                if !blank && shift != 0 {
                    self.set_lead(index, lead, shift, &mut budget)?;
                }
                // (An empty line inside is an empty line of the scalar; a line of white space is left as it is.)
                self.set_trim(index, !inside && !blank);
                continue;
            }
            // 2. The content of a block scalar.
            if let Some((old, new, end)) = active {
                let belongs = line.start < end && (blank || line.start + lead < end);
                if belongs {
                    self.block_line(index, lead, blank, old, new, &mut budget)?;
                    continue;
                }
                active = None;
            }
            // A block scalar with content starts in this line (the header is on the last line of structure).
            while layout.blocks.get(next_block).is_some_and(|b| b.start == b.end || b.end <= line.start) {
                next_block += 1;
            }
            // (An empty block scalar at the very end of the text is reported from its header on: its content does not start the line.)
            if layout.blocks.get(next_block).is_some_and(|b| {
                b.start >= line.start
                    && b.start < line.end
                    && !self.text.get(line.start..b.start).is_some_and(|before| before.bytes().all(|c| c == b' '))
            }) {
                next_block += 1;
            }
            if let Some(block) = layout.blocks.get(next_block).filter(|b| b.start >= line.start && b.start < line.end && !blank) {
                let base_old = block.start - line.start;
                let explicit = last_structure.and_then(|i| self.lines.get(i)).is_some_and(|h| header_is_explicit(self.content(h)));
                let base_new = match levels.iter().rev().find(|l| l.old < base_old) {
                    None => base_old,
                    Some(l) if explicit => (base_old + l.new).saturating_sub(l.old).max(l.new + 1),
                    Some(l) => l.new + self.unit,
                };
                active = Some((base_old, base_new, block.end));
                next_block += 1;
                self.block_line(index, lead, blank, base_old, base_new, &mut budget)?;
                continue;
            }
            // 3. Everything else: blank lines, comments, document markers and lines of structure.
            if blank {
                self.set_trim(index, !inside);
                continue;
            }
            if rest.starts_with('\t') {
                // A tab after the indentation: only a comment can be there. Left as it is.
                if rest.trim_start().starts_with('#') {
                    self.set_trim(index, !inside);
                    continue;
                }
                return Err(FormatError::Refused { message: "a line is indented with a tab, which YAML does not allow as indentation".to_owned() });
            }
            self.set_trim(index, !inside);
            if is_marker(content) {
                levels.clear();
                active = None;
                last_structure = None;
                // (Comments before the marker keep the place they got.)
                pending.clear();
                continue;
            }
            if rest.starts_with('#') {
                let place = self.comment_place(&levels, lead);
                // Provisional: a comment goes where the next line of structure at the same indentation goes.
                self.place(index, lead, place, &mut budget)?;
                pending.push((index, lead));
                continue;
            }
            // A line of structure.
            let dash = is_dash(rest);
            while let Some(top) = levels.last() {
                if top.old > lead || (top.old == lead && top.kind == Kind::Seq && !dash) {
                    levels.pop();
                } else {
                    break;
                }
            }
            let kind = if dash { Kind::Seq } else { Kind::Map };
            let new = match levels.last().copied() {
                None => {
                    levels.push(Level { old: lead, new: 0, kind });
                    0
                }
                Some(top) if top.old == lead && (top.kind == Kind::Seq) == dash => top.new,
                Some(top) if top.old == lead => {
                    // A sequence at the indentation of the key it is the value of.
                    let new = match self.style {
                        IndentSequences::Indent => top.new + self.unit,
                        IndentSequences::Keep | IndentSequences::Flush => top.new,
                    };
                    levels.push(Level { old: lead, new, kind });
                    new
                }
                Some(top) => {
                    let new = match (dash && top.kind == Kind::Map, self.style) {
                        (true, IndentSequences::Flush) => top.new,
                        _ => top.new + self.unit,
                    };
                    levels.push(Level { old: lead, new, kind });
                    new
                }
            };
            let shift = delta(new, lead);
            for (i, c) in pending.drain(..) {
                if c == lead {
                    self.place(i, c, new, &mut budget)?;
                }
            }
            self.place(index, lead, new, &mut budget)?;
            if let Some(s) = self.shifts.get_mut(index) {
                *s = shift;
            }
            last_structure = Some(index);
            for (col, kind) in chain(content, lead) {
                let moved = col.saturating_add_signed(shift);
                levels.push(Level { old: col, new: moved, kind });
            }
        }
        self.finish()
    }

    /// Where a comment at `lead` goes, by the levels that are open: the level it is at, or one deeper than the level above it.
    fn comment_place(&self, levels: &[Level], lead: usize) -> usize {
        let Some(at) = levels.iter().rposition(|l| l.old <= lead) else { return 0 };
        let Some(level) = levels.get(at) else { return 0 };
        if level.old == lead { level.new } else { levels.get(at + 1).map_or(level.new + self.unit, |deeper| deeper.new) }
    }

    fn place(&mut self, index: usize, old: usize, new: usize, budget: &mut usize) -> Result<(), FormatError> {
        self.set_lead(index, old, delta(new, old), budget)
    }

    /// Indent line `index`, which has `old` spaces, by `shift` more.
    fn set_lead(&mut self, index: usize, old: usize, shift: isize, budget: &mut usize) -> Result<(), FormatError> {
        let new = old.saturating_add_signed(shift);
        *budget = budget
            .checked_sub(new)
            .ok_or_else(|| FormatError::Refused { message: "the result would be too large: the nesting is too deep to indent".to_owned() })?;
        if let Some(action) = self.actions.get_mut(index) {
            action.lead = (new != old).then_some(new);
        }
        Ok(())
    }

    fn set_trim(&mut self, index: usize, trim: bool) {
        if let Some(action) = self.actions.get_mut(index) {
            action.trim = trim;
        }
    }

    /// A line of the content of a block scalar whose content is indented `old` columns: now `new`.
    fn block_line(&mut self, index: usize, lead: usize, blank: bool, old: usize, new: usize, budget: &mut usize) -> Result<(), FormatError> {
        let Some(&line) = self.lines.get(index) else { return Ok(()) };
        let content = self.content(&line);
        if blank {
            // A line of white space with no more than the indentation is an empty line; one with more has spaces in the text, so it moves.
            if lead == content.len() && lead <= old {
                self.set_lead(index, lead, delta(0, lead), budget)?;
            } else if lead == content.len() {
                self.set_lead(index, lead, delta(new, old), budget)?;
            }
            return Ok(());
        }
        if lead < old {
            return Err(FormatError::Refused { message: "a block scalar has a line indented less than its content".to_owned() });
        }
        if new != old {
            self.set_lead(index, lead, delta(new, old), budget)?;
        }
        Ok(())
    }

    /// The edits: the leading spaces and the trailing white space of each line, and the lines of white space at the end.
    fn finish(self) -> Result<Vec<Edit>, FormatError> {
        let mut edits = Vec::new();
        for (line, action) in self.lines.iter().zip(&self.actions) {
            let content = self.content(line);
            let lead = content.bytes().take_while(|b| *b == b' ').count();
            if let Some(new) = action.lead {
                edits.push(Edit::replace(line.start..line.start + lead, " ".repeat(new)));
            }
            if action.trim {
                let kept = content.trim_end_matches([' ', '\t']).len();
                // (A line of white space is emptied by its leading edit or by this one, never both.)
                let from = line.start + kept.max(if action.lead.is_some() { lead } else { 0 });
                if from < line.content_end {
                    edits.push(Edit::delete(from..line.content_end));
                }
            }
        }
        // Lines of white space at the end of the text collapse into the line break of the last line of content (unless that is inside a
        // scalar, where they may be part of the value).
        if let Some(last) = self.lines.iter().rposition(|l| !self.content(l).bytes().all(|b| b == b' ' || b == b'\t')) {
            let (end, inside) = self.lines.get(last).map_or((0, true), |l| {
                let first = l.start + self.content(l).bytes().take_while(|b| *b == b' ').count();
                let inside = self.layout.blocks.iter().any(|b| b.start <= first && first < b.end)
                    || self.layout.multiline.iter().any(|m| m.start < l.content_end && l.content_end < m.end);
                (l.end, inside)
            });
            let has_break = self.lines.get(last).is_some_and(|l| l.end > l.content_end);
            if has_break && !inside && end < self.text.len() {
                // The blank lines have their own edits (their white space); this one removes them with their breaks.
                edits.retain(|e| e.range.end <= end);
                edits.push(Edit::delete(end..self.text.len()));
            }
        }
        Ok(edits)
    }
}

/// `new - old`, saturating.
fn delta(new: usize, old: usize) -> isize {
    isize::try_from(new).unwrap_or(isize::MAX).saturating_sub(isize::try_from(old).unwrap_or(0))
}

fn is_dash(rest: &str) -> bool {
    rest.strip_prefix('-').is_some_and(|after| after.is_empty() || after.starts_with([' ', '\t']))
}

/// A `---` or `...` line (a document marker) or a `%` directive.
fn is_marker(content: &str) -> bool {
    if content.starts_with('%') {
        return true;
    }
    ["---", "..."].iter().any(|m| content.strip_prefix(m).is_some_and(|after| after.is_empty() || after.starts_with([' ', '\t'])))
}

/// The nested entries that start on the same line as the indicator before them (`- - a`, `- key: v`, `? - a`): the column each starts
/// at, and whether it is a sequence. A block scalar's header is not one (its content belongs to the indicator before it).
fn chain(content: &str, lead: usize) -> Vec<(usize, Kind)> {
    let bytes = content.as_bytes();
    let mut found = Vec::new();
    let mut at = lead;
    loop {
        let indicator = matches!(bytes.get(at), Some(b'-' | b'?' | b':')) && matches!(bytes.get(at + 1), None | Some(b' ' | b'\t'));
        if !indicator {
            break;
        }
        let mut next = at + 1;
        while matches!(bytes.get(next), Some(b' ' | b'\t')) {
            next += 1;
        }
        if matches!(bytes.get(next), None | Some(b'#')) {
            break;
        }
        if content.get(next..).is_some_and(starts_block_scalar) {
            break;
        }
        let kind = if content.get(next..).is_some_and(is_dash) { Kind::Seq } else { Kind::Map };
        found.push((next, kind));
        at = next;
    }
    found
}

/// Whether `rest` is properties (anchors, tags) and then the header of a block scalar.
fn starts_block_scalar(rest: &str) -> bool {
    let mut rest = rest;
    while rest.starts_with(['&', '!']) {
        let end = rest.find([' ', '\t']).unwrap_or(rest.len());
        rest = rest.get(end..).unwrap_or_default().trim_start_matches([' ', '\t']);
    }
    header_token(rest.split([' ', '\t']).next().unwrap_or_default()).is_some()
}

/// A block scalar header (`|`, `>-`, `|2+` …): whether it has an indentation indicator.
fn header_token(token: &str) -> Option<bool> {
    let rest = token.strip_prefix(['|', '>'])?;
    (rest.len() <= 2 && rest.bytes().all(|b| matches!(b, b'+' | b'-' | b'1'..=b'9'))).then(|| rest.bytes().any(|b| b.is_ascii_digit()))
}

/// Whether the line that holds a block scalar's header gives its indentation (`|2`).
fn header_is_explicit(line: &str) -> bool {
    let last_token = |s: &str| s.split([' ', '\t']).rfind(|t| !t.is_empty()).and_then(header_token);
    for (at, _) in line.match_indices('#') {
        if at > 0
            && line.get(..at).is_some_and(|before| before.ends_with([' ', '\t']))
            && let Some(explicit) = line.get(..at).and_then(last_token)
        {
            return explicit;
        }
    }
    last_token(line).unwrap_or(false)
}
