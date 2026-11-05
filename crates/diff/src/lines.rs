//! Comparing two texts line by line, and the words inside the lines that differ.

use std::collections::HashMap;
use std::ops::Range;

use crate::seq::{DEFAULT_BUDGET, matches};

/// What counts as the same white space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Whitespace {
    /// Every character counts.
    #[default]
    Exact,
    /// White space at the ends of a line does not count.
    Ends,
    /// Runs of white space are one space, and the ends do not count.
    Collapse,
    /// No white space counts.
    All,
}

/// How lines are compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineOptions {
    pub whitespace: Whitespace,
    pub ignore_case: bool,
    /// Lines that are empty (or only white space) are left out of the comparison.
    pub ignore_blank_lines: bool,
    /// How much work the comparison may do (see [`crate::seq::DEFAULT_BUDGET`]).
    pub budget: usize,
}

impl Default for LineOptions {
    fn default() -> Self {
        Self { whitespace: Whitespace::Exact, ignore_case: false, ignore_blank_lines: false, budget: DEFAULT_BUDGET }
    }
}

/// The lines of a text: the byte range of each, without its line break (`\n`, `\r\n` or a lone `\r`).
pub fn split_lines(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\n' => {
                lines.push(start..i);
                i += 1;
                start = i;
            }
            b'\r' => {
                lines.push(start..i);
                i += if bytes.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        lines.push(start..bytes.len());
    }
    lines
}

pub(crate) fn line_key(line: &str, options: &LineOptions) -> String {
    let base = match options.whitespace {
        Whitespace::Exact => line.to_owned(),
        Whitespace::Ends => line.trim().to_owned(),
        Whitespace::Collapse => line.split_whitespace().collect::<Vec<_>>().join(" "),
        Whitespace::All => line.chars().filter(|c| !c.is_whitespace()).collect(),
    };
    if options.ignore_case { base.to_lowercase() } else { base }
}

/// One place where the texts differ: the lines `old` of the first text were replaced by the lines `new` of the second (either may be
/// empty: lines only added, lines only removed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

impl Hunk {
    pub fn kind(&self) -> HunkKind {
        match (self.old.is_empty(), self.new.is_empty()) {
            (true, _) => HunkKind::Added,
            (_, true) => HunkKind::Removed,
            _ => HunkKind::Changed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Removed,
    Changed,
}

/// The result of comparing two texts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineDiff {
    /// The byte range of each line of the first text.
    pub old_lines: Vec<Range<usize>>,
    /// The byte range of each line of the second text.
    pub new_lines: Vec<Range<usize>>,
    /// The differences, in order, none touching another.
    pub hunks: Vec<Hunk>,
    /// The pairs of lines (old, new) that are the same, in order.
    pub same: Vec<(usize, usize)>,
    /// The length of each text in bytes.
    pub old_len: usize,
    pub new_len: usize,
    /// Every place where the lines are not the same, with whether the options leave it a difference (blank lines may not be).
    gaps: Vec<(Hunk, bool)>,
}

impl LineDiff {
    pub fn is_identical(&self) -> bool {
        self.hunks.is_empty()
    }

    /// The lines of the first text a hunk stands for, as one byte range (empty, at the start of the line they would go before, for an
    /// addition).
    pub fn old_span(&self, hunk: &Hunk) -> Range<usize> {
        span_of(&self.old_lines, &hunk.old, self.old_len)
    }

    /// The same for the second text.
    pub fn new_span(&self, hunk: &Hunk) -> Range<usize> {
        span_of(&self.new_lines, &hunk.new, self.new_len)
    }
}

fn span_of(lines: &[Range<usize>], wanted: &Range<usize>, end: usize) -> Range<usize> {
    match (lines.get(wanted.start), wanted.end.checked_sub(1).and_then(|i| lines.get(i))) {
        (Some(first), Some(last)) if wanted.start < wanted.end => first.start..last.end,
        (Some(first), _) => first.start..first.start,
        _ => end..end,
    }
}

/// Compare two texts line by line.
pub fn diff_lines(old: &str, new: &str, options: &LineOptions) -> LineDiff {
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    // The lines that take part, as numbers (equal lines have equal numbers), remembering which line each was.
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut intern = |text: &str, lines: &[Range<usize>]| -> (Vec<u32>, Vec<usize>) {
        let mut numbers = Vec::new();
        let mut at = Vec::new();
        for (i, range) in lines.iter().enumerate() {
            let line = text.get(range.clone()).unwrap_or_default();
            if options.ignore_blank_lines && line.trim().is_empty() {
                continue;
            }
            let next = u32::try_from(ids.len()).unwrap_or(u32::MAX);
            numbers.push(*ids.entry(line_key(line, options)).or_insert(next));
            at.push(i);
        }
        (numbers, at)
    };
    let (a, a_at) = intern(old, &old_lines);
    let (b, b_at) = intern(new, &new_lines);
    let same: Vec<(usize, usize)> = matches(&a, &b, options.budget).into_iter().filter_map(|(i, j)| Some((*a_at.get(i)?, *b_at.get(j)?))).collect();
    let all = hunks_between(&same, old_lines.len(), new_lines.len());
    // A difference of nothing but blank lines is none, when they are ignored.
    let blank = |text: &str, lines: &[Range<usize>], r: &Range<usize>| {
        r.clone().all(|i| lines.get(i).is_none_or(|l| text.get(l.clone()).unwrap_or_default().trim().is_empty()))
    };
    let gaps: Vec<(Hunk, bool)> = all
        .into_iter()
        .map(|h| {
            let real = !(options.ignore_blank_lines && blank(old, &old_lines, &h.old) && blank(new, &new_lines, &h.new));
            (h, real)
        })
        .collect();
    let hunks = gaps.iter().filter(|(_, real)| *real).map(|(h, _)| h.clone()).collect();
    LineDiff { old_lines, new_lines, hunks, same, old_len: old.len(), new_len: new.len(), gaps }
}

/// The gaps between the matching lines.
pub(crate) fn hunks_between(same: &[(usize, usize)], old_len: usize, new_len: usize) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    for &(a, b) in same.iter().chain(std::iter::once(&(old_len, new_len))) {
        if a > i || b > j {
            hunks.push(Hunk { old: i..a.max(i), new: j..b.max(j) });
        }
        i = a + 1;
        j = b + 1;
    }
    hunks
}

/// A piece of a line: a word, a run of white space, or one other character.
fn tokens(line: &str) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut previous: Option<u8> = None;
    for (i, c) in line.char_indices() {
        let class = if c.is_alphanumeric() || c == '_' {
            1
        } else if c.is_whitespace() {
            2
        } else {
            0
        };
        let end = i + c.len_utf8();
        match out.last_mut() {
            Some(last) if class != 0 && previous == Some(class) => last.end = end,
            _ => out.push(i..end),
        }
        previous = Some(class);
    }
    out
}

/// The parts of two lines that differ: the byte ranges in `old` and in `new`, merged where they touch.
pub fn diff_words(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (ta, tb) = (tokens(old), tokens(new));
    let text = |s: &str, r: &Range<usize>| s.get(r.clone()).unwrap_or_default().to_owned();
    let (a, b): (Vec<String>, Vec<String>) = (ta.iter().map(|r| text(old, r)).collect(), tb.iter().map(|r| text(new, r)).collect());
    let same = matches(&a, &b, DEFAULT_BUDGET / 10);
    let gaps = hunks_between(&same, ta.len(), tb.len());
    let ranges = |tokens: &[Range<usize>], pick: &dyn Fn(&Hunk) -> Range<usize>| -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        for gap in &gaps {
            let r = pick(gap);
            if let (Some(first), Some(last)) = (tokens.get(r.start), r.end.checked_sub(1).and_then(|i| tokens.get(i)))
                && r.start < r.end
            {
                match out.last_mut() {
                    Some(prev) if prev.end == first.start => prev.end = last.end,
                    _ => out.push(first.start..last.end),
                }
            }
        }
        out
    };
    (ranges(&ta, &|h| h.old.clone()), ranges(&tb, &|h| h.new.clone()))
}

impl LineDiff {
    /// The differences as a unified diff (`@@ -a,b +c,d @@` hunks with `context` lines around them), the way `diff -u` writes it. The
    /// texts must be those that were compared. Differences that the options make none (blank lines) are written where they stand in a
    /// block, so that its counts are right, but start no block of their own.
    pub fn unified(&self, old: &str, new: &str, old_name: &str, new_name: &str, context: usize) -> String {
        if self.hunks.is_empty() {
            return String::new();
        }
        let line = |text: &str, lines: &[Range<usize>], i: usize| text.get(lines.get(i).cloned().unwrap_or(0..0)).unwrap_or_default().to_owned();
        let mut out = format!("--- {old_name}\n+++ {new_name}\n");
        let reach = context.saturating_mul(2);
        // Differences no more than twice the context apart share a block.
        let mut blocks: Vec<Vec<&(Hunk, bool)>> = Vec::new();
        for g in &self.gaps {
            match blocks.last_mut() {
                Some(block) if block.last().is_some_and(|(p, _)| g.0.old.start.saturating_sub(p.old.end) <= reach) => block.push(g),
                _ => blocks.push(vec![g]),
            }
        }
        for block in blocks.into_iter().filter(|b| b.iter().any(|(_, real)| *real)) {
            let (Some((first, _)), Some((last, _))) = (block.first(), block.last()) else { continue };
            let from = first.old.start.saturating_sub(context);
            let to = last.old.end.saturating_add(context).min(self.old_lines.len());
            let new_from = first.new.start.saturating_sub(context);
            let new_to = last.new.end.saturating_add(context).min(self.new_lines.len());
            let count = |a: usize, b: usize| b.saturating_sub(a);
            let start = |from: usize, n: usize| if n == 0 { from } else { from + 1 };
            out.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                start(from, count(from, to)),
                count(from, to),
                start(new_from, count(new_from, new_to)),
                count(new_from, new_to)
            ));
            let mut i = from;
            for (h, _) in &block {
                for k in i..h.old.start {
                    out.push_str(&format!(" {}\n", line(old, &self.old_lines, k)));
                }
                for k in h.old.clone() {
                    out.push_str(&format!("-{}\n", line(old, &self.old_lines, k)));
                }
                for k in h.new.clone() {
                    out.push_str(&format!("+{}\n", line(new, &self.new_lines, k)));
                }
                i = h.old.end;
            }
            for k in i..to {
                out.push_str(&format!(" {}\n", line(old, &self.old_lines, k)));
            }
        }
        out
    }
}
