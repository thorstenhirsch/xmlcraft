//! Three-way merge of texts: a base and two versions made from it (ours and theirs) become one text with both sets of changes, and
//! the places where the two disagree are conflicts to decide.

use std::ops::Range;

use crate::lines::{LineOptions, split_lines};
use crate::seq::matches;

/// A place where both versions changed the same lines of the base, differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub base: String,
    pub ours: String,
    pub theirs: String,
}

/// A part of the merged text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Region {
    /// Lines all three have.
    Same(String),
    /// Lines only our version changed (this is our text).
    Ours(String),
    /// Lines only their version changed (this is their text).
    Theirs(String),
    /// Lines both changed in the same way.
    Both(String),
    Conflict(Conflict),
}

/// What to put in the text for a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Ours,
    Theirs,
    /// Ours, then theirs.
    Both,
    Base,
    /// The lines of each between `<<<<<<<`, `|||||||`, `=======` and `>>>>>>>` lines.
    Markers,
}

/// The merge of three texts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge3 {
    pub regions: Vec<Region>,
}

impl Merge3 {
    /// How many conflicts there are.
    pub fn conflict_count(&self) -> usize {
        self.regions.iter().filter(|r| matches!(r, Region::Conflict(_))).count()
    }

    /// The merged text with every conflict settled the same way.
    pub fn render(&self, choice: Choice) -> String {
        self.render_with(&|_| choice)
    }

    /// The merged text; `choice` says what to do with the conflict of that number (from 0).
    pub fn render_with(&self, choice: &dyn Fn(usize) -> Choice) -> String {
        let mut out = String::new();
        let mut number = 0;
        for region in &self.regions {
            match region {
                Region::Same(t) | Region::Ours(t) | Region::Theirs(t) | Region::Both(t) => push_chunk(&mut out, t),
                Region::Conflict(c) => {
                    match choice(number) {
                        Choice::Ours => push_chunk(&mut out, &c.ours),
                        Choice::Theirs => push_chunk(&mut out, &c.theirs),
                        Choice::Both => {
                            push_chunk(&mut out, &c.ours);
                            push_chunk(&mut out, &c.theirs);
                        }
                        Choice::Base => push_chunk(&mut out, &c.base),
                        Choice::Markers => {
                            out.push_str("<<<<<<< ours\n");
                            push_chunk(&mut out, &c.ours);
                            out.push_str("||||||| base\n");
                            push_chunk(&mut out, &c.base);
                            out.push_str("=======\n");
                            push_chunk(&mut out, &c.theirs);
                            out.push_str(">>>>>>> theirs\n");
                        }
                    }
                    number += 1;
                }
            }
        }
        out
    }
}

/// Add lines to the text; a chunk that does not end its last line (only the end of a text does not) gets a break when something follows.
fn push_chunk(out: &mut String, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    if !out.is_empty() && !out.ends_with(['\n', '\r']) {
        out.push('\n');
    }
    out.push_str(chunk);
}

/// The text of the lines `range` (with their line breaks).
fn slice(text: &str, lines: &[Range<usize>], range: Range<usize>) -> String {
    if range.start >= range.end {
        return String::new();
    }
    let start = lines.get(range.start).map_or(text.len(), |l| l.start);
    let end = lines.get(range.end).map_or(text.len(), |l| l.start);
    text.get(start..end).unwrap_or_default().to_owned()
}

struct Side<'a> {
    text: &'a str,
    lines: Vec<Range<usize>>,
    keys: Vec<u32>,
}

/// Merge `ours` and `theirs`, both made from `base`.
pub fn merge3(base: &str, ours: &str, theirs: &str, options: &LineOptions) -> Merge3 {
    let mut ids: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut side = |text| -> Side<'_> {
        let lines = split_lines(text);
        let keys = lines
            .iter()
            .map(|r| {
                let line = text.get(r.clone()).unwrap_or_default();
                let key = crate::lines::line_key(line, options);
                let next = u32::try_from(ids.len()).unwrap_or(u32::MAX);
                *ids.entry(key).or_insert(next)
            })
            .collect();
        Side { text, lines, keys }
    };
    let (o, a, b) = (side(base), side(ours), side(theirs));
    let match_a = matches(&o.keys, &a.keys, options.budget);
    let match_b = matches(&o.keys, &b.keys, options.budget);
    let mut to_a = vec![None; o.keys.len()];
    let mut to_b = vec![None; o.keys.len()];
    for (i, j) in match_a {
        if let Some(slot) = to_a.get_mut(i) {
            *slot = Some(j);
        }
    }
    for (i, j) in match_b {
        if let Some(slot) = to_b.get_mut(i) {
            *slot = Some(j);
        }
    }
    // The lines of the base that both versions kept, in order, then the end of all three.
    let mut sync: Vec<(usize, usize, usize)> = (0..o.keys.len()).filter_map(|i| Some((i, (*to_a.get(i)?)?, (*to_b.get(i)?)?))).collect();
    sync.push((o.keys.len(), a.keys.len(), b.keys.len()));
    let mut regions: Vec<Region> = Vec::new();
    let add_same = |regions: &mut Vec<Region>, text: String| {
        if text.is_empty() {
            return;
        }
        match regions.last_mut() {
            Some(Region::Same(t)) => t.push_str(&text),
            _ => regions.push(Region::Same(text)),
        }
    };
    let (mut oi, mut ai, mut bi) = (0, 0, 0);
    for (so, sa, sb) in sync {
        // The lines between the last kept line and this one.
        if so > oi || sa > ai || sb > bi {
            let base_part = oi..so;
            let (ours_part, theirs_part) = (ai..sa, bi..sb);
            let same_as_base = |side: &Side<'_>, part: &Range<usize>| -> bool {
                side.keys.get(part.clone()) == o.keys.get(base_part.clone()) && part.len() == base_part.len()
            };
            let ours_text = slice(a.text, &a.lines, ours_part.clone());
            let theirs_text = slice(b.text, &b.lines, theirs_part.clone());
            let ours_same = same_as_base(&a, &ours_part);
            let theirs_same = same_as_base(&b, &theirs_part);
            if ours_same && theirs_same {
                add_same(&mut regions, slice(o.text, &o.lines, base_part));
            } else if ours_same {
                regions.push(Region::Theirs(theirs_text));
            } else if theirs_same {
                regions.push(Region::Ours(ours_text));
            } else if a.keys.get(ours_part.clone()) == b.keys.get(theirs_part.clone()) {
                regions.push(Region::Both(ours_text));
            } else {
                regions.push(Region::Conflict(Conflict { base: slice(o.text, &o.lines, base_part), ours: ours_text, theirs: theirs_text }));
            }
        }
        if so < o.keys.len() {
            // The kept line itself (ours' version of it).
            add_same(&mut regions, slice(a.text, &a.lines, sa..sa + 1));
        }
        oi = so + 1;
        ai = sa + 1;
        bi = sb + 1;
    }
    Merge3 { regions }
}
