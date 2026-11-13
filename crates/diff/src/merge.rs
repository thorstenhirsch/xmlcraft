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
                            marker(&mut out, "<<<<<<< ours\n");
                            push_chunk(&mut out, &c.ours);
                            marker(&mut out, "||||||| base\n");
                            push_chunk(&mut out, &c.base);
                            marker(&mut out, "=======\n");
                            push_chunk(&mut out, &c.theirs);
                            marker(&mut out, ">>>>>>> theirs\n");
                        }
                    }
                    number += 1;
                }
            }
        }
        out
    }
}

/// A line of its own, after a line that may have had no break.
fn marker(out: &mut String, line: &str) {
    if !out.is_empty() && !out.ends_with(['\n', '\r']) {
        out.push('\n');
    }
    out.push_str(line);
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
    /// Lines that are blank, when blank lines are ignored.
    blank: Vec<bool>,
}

impl Side<'_> {
    /// What the lines `part` are for comparing: their keys, without the blank lines that are ignored.
    fn kept(&self, part: &Range<usize>) -> Vec<u32> {
        part.clone().filter(|&i| self.blank.get(i) == Some(&false)).filter_map(|i| self.keys.get(i).copied()).collect()
    }
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
        let blank = lines.iter().map(|r| options.ignore_blank_lines && text.get(r.clone()).unwrap_or_default().trim().is_empty()).collect();
        Side { text, lines, keys, blank }
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
            let base_kept = o.kept(&base_part);
            let (ours_kept, theirs_kept) = (a.kept(&ours_part), b.kept(&theirs_part));
            let ours_text = slice(a.text, &a.lines, ours_part.clone());
            let theirs_text = slice(b.text, &b.lines, theirs_part.clone());
            let (ours_same, theirs_same) = (ours_kept == base_kept, theirs_kept == base_kept);
            if ours_same && theirs_same {
                // (Nothing changed that counts: the text of the side that did not keep the base's own wins, so that a difference the
                // options ignore is not undone by the merge.)
                let base_text = slice(o.text, &o.lines, base_part);
                add_same(&mut regions, if ours_text == base_text { theirs_text } else { ours_text });
            } else if ours_same {
                regions.push(Region::Theirs(theirs_text));
            } else if theirs_same {
                regions.push(Region::Ours(ours_text));
            } else if ours_kept == theirs_kept {
                regions.push(Region::Both(ours_text));
            } else {
                regions.push(Region::Conflict(Conflict { base: slice(o.text, &o.lines, base_part), ours: ours_text, theirs: theirs_text }));
            }
        }
        if so < o.keys.len() {
            // The kept line itself: ours' version, unless ours is the base's and theirs is not (a difference the options ignore, such
            // as case, then stays).
            let (ours_line, theirs_line) = (slice(a.text, &a.lines, sa..sa + 1), slice(b.text, &b.lines, sb..sb + 1));
            let base_line = slice(o.text, &o.lines, so..so + 1);
            add_same(&mut regions, if ours_line == base_line { theirs_line } else { ours_line });
        }
        oi = so + 1;
        ai = sa + 1;
        bi = sb + 1;
    }
    Merge3 { regions }
}

/// The most conflicts [`conflicts_in`] finds in one text.
pub const MAX_MARKED: usize = 10_000;

/// A conflict as it is marked in a text (by [`Choice::Markers`], or by another tool that writes the same lines): the lines from
/// `<<<<<<<` to `>>>>>>>`, with the parts between the markers. All are byte ranges of the text; the parts are whole lines with their
/// breaks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkedConflict {
    /// From the start of the `<<<<<<<` line to the end of the `>>>>>>>` line, its line break included.
    pub range: Range<usize>,
    pub ours: Range<usize>,
    /// The part after `|||||||` (absent when the markers have no base part).
    pub base: Option<Range<usize>>,
    pub theirs: Range<usize>,
}

impl MarkedConflict {
    /// What the marked lines become when the conflict is settled: the text of one part, ours then theirs for [`Choice::Both`], or the
    /// marked lines as they are for [`Choice::Markers`]. [`Choice::Base`] without a base part is the empty text.
    pub fn settled(&self, text: &str, choice: Choice) -> String {
        let part = |r: &Range<usize>| text.get(r.clone()).unwrap_or_default();
        match choice {
            Choice::Ours => part(&self.ours).to_owned(),
            Choice::Theirs => part(&self.theirs).to_owned(),
            Choice::Both => {
                let mut both = part(&self.ours).to_owned();
                push_chunk(&mut both, part(&self.theirs));
                both
            }
            Choice::Base => self.base.as_ref().map_or(String::new(), |b| part(b).to_owned()),
            Choice::Markers => part(&self.range).to_owned(),
        }
    }
}

/// The conflicts marked in a text, in order (at most [`MAX_MARKED`]). A `<<<<<<<` line without its `=======` and `>>>>>>>` lines is not
/// one; a new `<<<<<<<` line inside a conflict starts over there.
pub fn conflicts_in(text: &str) -> Vec<MarkedConflict> {
    let lines = split_lines(text);
    let line_text = |i: usize| lines.get(i).and_then(|l| text.get(l.clone())).unwrap_or_default();
    // The start of line `i`, and the start of the line after it (its break included).
    let start = |i: usize| lines.get(i).map_or(text.len(), |l| l.start);
    let after = |i: usize| lines.get(i + 1).map_or(text.len(), |l| l.start);
    let mut out = Vec::new();
    let mut i = 0;
    'outer: while i < lines.len() && out.len() < MAX_MARKED {
        if !line_text(i).starts_with("<<<<<<<") {
            i += 1;
            continue;
        }
        let open = i;
        let (mut bar, mut equals) = (None, None);
        let mut j = i + 1;
        while j < lines.len() {
            let l = line_text(j);
            if l.starts_with("<<<<<<<") {
                i = j;
                continue 'outer;
            }
            if equals.is_none() && bar.is_none() && l.starts_with("|||||||") {
                bar = Some(j);
            } else if equals.is_none() && l == "=======" {
                equals = Some(j);
            } else if let Some(eq) = equals
                && l.starts_with(">>>>>>>")
            {
                let ours_end = bar.unwrap_or(eq);
                out.push(MarkedConflict {
                    range: start(open)..after(j),
                    ours: after(open)..start(ours_end),
                    base: bar.map(|b| after(b)..start(eq)),
                    theirs: after(eq)..start(j),
                });
                i = j + 1;
                continue 'outer;
            }
            j += 1;
        }
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settle every marked conflict of `text` (from the last, so the places stay right) as `choice` says for its number.
    fn settle_all(text: &str, choice: &dyn Fn(usize) -> Choice) -> String {
        let mut out = text.to_owned();
        for (n, c) in conflicts_in(text).iter().enumerate().rev() {
            out.replace_range(c.range.clone(), &c.settled(text, choice(n)));
        }
        out
    }

    #[test]
    fn marked_conflicts_are_found_with_their_parts() {
        let m = merge3("a\nb\nc\nd\n", "a\nX\nc\nY\n", "a\nP\nc\nQ\n", &LineOptions::default());
        let text = m.render(Choice::Markers);
        let found = conflicts_in(&text);
        assert_eq!(found.len(), 2);
        assert_eq!(&text[found[0].ours.clone()], "X\n");
        assert_eq!(&text[found[0].base.clone().expect("a base part")], "b\n");
        assert_eq!(&text[found[0].theirs.clone()], "P\n");
        assert!(text[found[1].range.clone()].starts_with("<<<<<<< ours\nY\n") && text[found[1].range.clone()].ends_with(">>>>>>> theirs\n"));
        assert_eq!(settle_all(&text, &|n| if n == 0 { Choice::Theirs } else { Choice::Ours }), "a\nP\nc\nY\n");
    }

    #[test]
    fn markers_without_a_base_part_or_incomplete_ones() {
        let text = "x\n<<<<<<< HEAD\none\n=======\ntwo\n>>>>>>> branch\ny\n";
        let found = conflicts_in(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].base, None);
        assert_eq!(found[0].settled(text, Choice::Both), "one\ntwo\n");
        assert_eq!(found[0].settled(text, Choice::Base), "");
        assert!(conflicts_in("<<<<<<< a\nb\n").is_empty(), "never closed");
        assert!(conflicts_in("<<<<<<< a\nb\n>>>>>>> c\n").is_empty(), "no =======");
        let restarted = "<<<<<<< a\nstray\n<<<<<<< ours\n1\n=======\n2\n>>>>>>> theirs\n";
        let found = conflicts_in(restarted);
        assert_eq!((found.len(), &restarted[found[0].ours.clone()]), (1, "1\n"), "the later start wins");
        assert!(conflicts_in("").is_empty());
    }

    #[test]
    fn crlf_markers_are_found_too() {
        let text = "<<<<<<< ours\r\nA\r\n=======\r\nB\r\n>>>>>>> theirs\r\nz\r\n";
        let found = conflicts_in(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].settled(text, Choice::Theirs), "B\r\n");
        assert_eq!(&text[found[0].range.end..], "z\r\n");
    }

    fn version() -> impl proptest::strategy::Strategy<Value = Vec<String>> {
        proptest::collection::vec(proptest::string::string_regex("[abc]").expect("regex"), 0..8)
    }

    proptest::proptest! {
        #[test]
        fn settling_the_markers_is_rendering_with_the_choice(base in version(), ours in version(), theirs in version(), pick in 0u8..4) {
            let text = |v: &[String]| v.iter().map(|l| format!("{l}\n")).collect::<String>();
            let m = merge3(&text(&base), &text(&ours), &text(&theirs), &LineOptions::default());
            let choice = [Choice::Ours, Choice::Theirs, Choice::Both, Choice::Base][usize::from(pick)];
            let marked = m.render(Choice::Markers);
            proptest::prop_assert_eq!(conflicts_in(&marked).len(), m.conflict_count());
            proptest::prop_assert_eq!(settle_all(&marked, &|_| choice), m.render(choice));
        }
    }
}
