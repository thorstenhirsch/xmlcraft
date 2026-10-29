//! What is known about a document's text: its problems and its outline, from one parse.
//!
//! [`analyze`] is pure and total. [`Analyzer`] runs it on a worker thread so that typing never waits for it: a request replaces any
//! older one for the same document, a panic in the analysis becomes a failed result instead of taking the app down, and a result says
//! which version of the text it is about.

use std::ops::Range;
use xmlcraft_syntax::{Diagnostic, Limits, Severity, SyntaxKind, parse_with};
use xmlcraft_validate::SchemaHint;

use crate::folding::{FoldRange, fold_ranges};
use crate::worker::Worker;
use xmlcraft_text::Buffer;

/// Documents larger than this are not analysed on every edit (large-file mode, M1.15, takes over).
pub const MAX_ANALYSED_BYTES: usize = 32 * 1024 * 1024;
/// The outline shows at most this many elements.
pub const MAX_OUTLINE_ITEMS: usize = 50_000;

/// One element in the outline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    /// Nesting depth, 0 for the root element.
    pub depth: usize,
    /// The element's name as written (with any prefix).
    pub name: String,
    /// A short hint from its `id`, `name` or `key` attribute, if it has one.
    pub label: Option<String>,
    /// The element, start tag to end tag, as a byte range of the text.
    pub range: Range<usize>,
    /// The name in the start tag.
    pub name_range: Range<usize>,
    /// The start tag, `<` to `>` (or `/>`).
    pub start_tag: Range<usize>,
    /// The end tag, `</` to `>`; none for an empty-element tag and for an element that was never closed.
    pub end_tag: Option<Range<usize>>,
    /// The name in the end tag, if it has one.
    pub end_name_range: Option<Range<usize>>,
}

/// An element's start tag and end tag, as the caret sees them: the tag it is in, and the one that goes with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagMatch {
    /// The tag the offset is in or next to, and its name.
    pub tag: Range<usize>,
    pub name: Range<usize>,
    /// The other tag of the element, and its name.
    pub partner: Range<usize>,
    pub partner_name: Range<usize>,
}

/// The result of analysing one version of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analysis {
    /// The version of the text this is about (`Editor::version`).
    pub version: u64,
    pub diagnostics: Vec<Diagnostic>,
    pub outline: Vec<OutlineItem>,
    /// The outline stopped at [`MAX_OUTLINE_ITEMS`].
    pub outline_truncated: bool,
    /// What can be folded, in the order it starts, and whether there was more than [`crate::folding::MAX_FOLD_RANGES`] of it.
    pub folds: Vec<FoldRange>,
    pub folds_truncated: bool,
    /// The DOCTYPE declaration, if the text has one: a byte range of the text.
    pub doctype: Option<Range<usize>>,
    /// The schemas the root element names with `xsi:schemaLocation` and `xsi:noNamespaceSchemaLocation` (a DOCTYPE, when there is
    /// one, is what the document is checked against, and these are not looked at).
    pub schema_hints: Vec<SchemaHint>,
    /// The namespace the root element is in (as the element declares it): the schema known for it, in the catalogs or the bundle, is what
    /// a document that names none can be offered things from.
    pub root_namespace: Option<String>,
    /// The text was not analysed, and why.
    pub skipped: Option<Skipped>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skipped {
    /// Larger than [`MAX_ANALYSED_BYTES`].
    TooLarge,
    /// The analysis itself failed (a bug); the document is untouched.
    Failed,
}

impl Analysis {
    pub fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity() == Severity::Error).count()
    }

    pub fn warning_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity() == Severity::Warning).count()
    }

    /// Analysed, and no errors.
    pub fn is_well_formed(&self) -> bool {
        self.skipped.is_none() && self.error_count() == 0
    }

    /// The tag at `offset` (a byte offset of the text this is about) and the other tag of its element. The tag the offset is inside
    /// wins; then one that ends at the offset (a caret that has just typed the `>`), then one that starts there. `None` when the offset
    /// is in no tag, or the tag has no partner: an empty-element tag, or an element that was never closed.
    pub fn matching_tag(&self, offset: usize) -> Option<TagMatch> {
        // Start tags are in the order of the text, so those that touch the offset (at most two: one ends there, the next starts there)
        // are the last ones that start at or before it.
        let started = self.outline.get(..self.outline.partition_point(|item| item.start_tag.start <= offset))?;
        let starts = started.iter().rev().take(2).map(|item| (item, true));
        // An end tag that touches the offset belongs to the innermost element the offset is past the start of and not past the end of;
        // what the walk back passes over has ended. (An element that starts at the offset has not begun: the one before it may end here.)
        let end = started.iter().rev().find(|item| item.range.start < offset && offset <= item.range.end).map(|item| (item, false));
        starts
            .chain(end)
            .filter_map(|(item, is_start)| {
                let tag = if is_start { Some(&item.start_tag) } else { item.end_tag.as_ref() }?;
                Some((closeness(tag, offset)?, item, is_start))
            })
            .min_by_key(|(closeness, ..)| *closeness)
            .and_then(|(_, item, is_start)| item.tag_match(is_start))
    }

    /// The names of the start tag and the end tag of the element whose tag name `offset` is in or at the end of (what the editor keeps
    /// in step while one of them is typed in). `None` when the offset is elsewhere in a tag, or the element has no pair.
    pub fn linked_names(&self, offset: usize) -> Option<[Range<usize>; 2]> {
        let found = self.matching_tag(offset)?;
        (found.name.start <= offset && offset <= found.name.end).then_some([found.name, found.partner_name])
    }

    fn skipped(version: u64, why: Skipped) -> Self {
        Self {
            version,
            diagnostics: Vec::new(),
            outline: Vec::new(),
            outline_truncated: false,
            folds: Vec::new(),
            folds_truncated: false,
            doctype: None,
            schema_hints: Vec::new(),
            root_namespace: None,
            skipped: Some(why),
        }
    }
}

/// How near `offset` is to `tag`: 0 inside it, 1 just after it, 2 just before it, `None` otherwise.
fn closeness(tag: &Range<usize>, offset: usize) -> Option<u8> {
    if tag.start < offset && offset < tag.end {
        Some(0)
    } else if tag.end == offset {
        Some(1)
    } else if tag.start == offset {
        Some(2)
    } else {
        None
    }
}

impl TagMatch {
    /// Where the caret goes to follow `offset`, which is in or next to [`TagMatch::tag`]: from inside the name, to the same place in the
    /// other name (as far as it reaches); from anywhere else in the tag, to the start of the other name.
    pub fn jump_target(&self, offset: usize) -> usize {
        if self.name.start <= offset && offset <= self.name.end {
            self.partner_name.start + (offset - self.name.start).min(self.partner_name.len())
        } else {
            self.partner_name.start
        }
    }
}

impl OutlineItem {
    /// This element's start tag (`start`) or end tag, with the other one; `None` when there is no end tag to pair it with.
    fn tag_match(&self, start: bool) -> Option<TagMatch> {
        let end_tag = self.end_tag.clone()?;
        let end_name = self.end_name_range.clone()?;
        Some(if start {
            TagMatch { tag: self.start_tag.clone(), name: self.name_range.clone(), partner: end_tag, partner_name: end_name }
        } else {
            TagMatch { tag: end_tag, name: end_name, partner: self.start_tag.clone(), partner_name: self.name_range.clone() }
        })
    }
}

/// Analyse `text`, which is version `version` of a document.
pub fn analyze(text: &str, version: u64) -> Analysis {
    if text.len() > MAX_ANALYSED_BYTES {
        return Analysis::skipped(version, Skipped::TooLarge);
    }
    let parsed = parse_with(text, &Limits::default());
    // The problems of the text and of the declarations of its internal subset (which the DTD reader reads); the external parts of the
    // DTD are not read here.
    let mut diagnostics = xmlcraft_dtd::problems_in_text(&parsed.syntax(), text, parsed.diagnostics());
    diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
    let (outline, outline_truncated) = outline_of(&parsed.syntax(), text);
    let (folds, folds_truncated) = fold_ranges(&parsed.syntax(), text);
    let doctype = xmlcraft_dtd::doctype_parts(&parsed.syntax()).map(|p| p.range);
    let schema_hints = xmlcraft_validate::schema_hints(&parsed.syntax());
    let root_namespace = xmlcraft_validate::root_namespace(&parsed.syntax());
    Analysis { version, diagnostics, outline, outline_truncated, folds, folds_truncated, doctype, schema_hints, root_namespace, skipped: None }
}

fn outline_of(root: &xmlcraft_syntax::SyntaxNode, text: &str) -> (Vec<OutlineItem>, bool) {
    use rowan::WalkEvent;
    let mut items: Vec<OutlineItem> = Vec::new();
    let mut depth = 0usize;
    let mut truncated = false;
    for event in root.preorder() {
        match event {
            WalkEvent::Enter(node) if node.kind() == SyntaxKind::Element => {
                if items.len() >= MAX_OUTLINE_ITEMS {
                    truncated = true;
                } else if let Some(item) = item_of(&node, text, depth) {
                    items.push(item);
                }
                depth += 1;
            }
            WalkEvent::Leave(node) if node.kind() == SyntaxKind::Element => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    (items, truncated)
}

fn range_of(node: &xmlcraft_syntax::SyntaxNode) -> Range<usize> {
    usize::from(node.text_range().start())..usize::from(node.text_range().end())
}

fn item_of(node: &xmlcraft_syntax::SyntaxNode, text: &str, depth: usize) -> Option<OutlineItem> {
    let range = range_of(node);
    let tag = node.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    let name_token = tag.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
    let name_range = usize::from(name_token.text_range().start())..usize::from(name_token.text_range().end());
    let mut label = None;
    for attribute in tag.children().filter(|n| n.kind() == SyntaxKind::Attribute) {
        let Some(name) = attribute.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == SyntaxKind::Name) else { continue };
        if !matches!(name.text(), "id" | "name" | "key") {
            continue;
        }
        if let Some(value) = attribute.children().find(|n| n.kind() == SyntaxKind::AttrValue) {
            let r = usize::from(value.text_range().start())..usize::from(value.text_range().end());
            let inner = text.get(r.start..r.end).unwrap_or_default().trim_matches(['"', '\'']);
            if !inner.is_empty() {
                label = Some(inner.chars().take(40).collect());
                break;
            }
        }
    }
    let end = node.children().find(|n| n.kind() == SyntaxKind::EndTag);
    let end_tag = end.as_ref().map(range_of);
    let end_name_range = end
        .as_ref()
        .and_then(|e| e.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name))
        .map(|t| usize::from(t.text_range().start())..usize::from(t.text_range().end()));
    Some(OutlineItem { depth, name: name_token.text().to_owned(), label, range, name_range, start_tag: range_of(&tag), end_tag, end_name_range })
}

/// The largest text analysed in the caller's own thread. On the web there is no other thread and a frame must not wait long, so it
/// is small there; a native analyzer whose worker could not start has the same limit as the worker.
#[cfg(target_arch = "wasm32")]
const INLINE_LIMIT: usize = 1024 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const INLINE_LIMIT: usize = MAX_ANALYSED_BYTES;

/// Runs [`analyze`] on a worker thread. `submit` returns at once; `poll` collects what has finished.
pub struct Analyzer {
    worker: Worker<(u64, Buffer), Analysis>,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

fn run((version, text): (u64, Buffer)) -> Analysis {
    analyze_guarded(&text.text(), version)
}

impl Analyzer {
    pub fn new() -> Self {
        Self { worker: Worker::new("analysis", run) }
    }

    /// An analyzer with no thread: `submit` analyses in the caller's thread (what the web does, and handy for tests).
    pub fn inline() -> Self {
        Self { worker: Worker::inline(run) }
    }

    /// Called whenever a result is ready (the app asks for a repaint).
    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    /// Ask for `text` (version `version` of the document with this id) to be analysed. A request that has not started is replaced by a
    /// newer one for the same document, so a slow analysis never leaves a queue behind it.
    pub fn submit(&self, document: u64, version: u64, text: &Buffer) {
        if !self.worker.threaded() && text.len() > INLINE_LIMIT {
            self.worker.finish(document, Analysis::skipped(version, Skipped::TooLarge));
            return;
        }
        self.worker.submit(document, (version, text.clone()));
    }

    /// The analyses that finished since the last call, oldest first.
    pub fn poll(&self) -> Vec<(u64, Analysis)> {
        self.worker.poll()
    }
}

/// [`analyze`], with a panic turned into a failed result (the last-resort guard: a bug in the analysis must not take the app down).
pub fn analyze_guarded(text: &str, version: u64) -> Analysis {
    crate::guard(|| analyze(text, version)).unwrap_or_else(|message| {
        log::error!("analysis failed: {message}");
        Analysis::skipped(version, Skipped::Failed)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_declarations_of_the_internal_subset_are_read_and_their_mistakes_are_problems() {
        use xmlcraft_syntax::Code;
        let codes = |text: &str| analyze(text, 1).diagnostics.iter().map(|d| d.code).collect::<Vec<_>>();
        assert_eq!(codes("<!DOCTYPE a [<!ELEMENT a EMPTY><!ATTLIST a x CDATA #IMPLIED><!ENTITY e 'v'>]><a/>"), []);
        assert_eq!(codes("<!DOCTYPE a [<!ELEMENT a>]><a/>"), [Code::DtdBadContentModel]);
        assert_eq!(codes("<!DOCTYPE a [<!ATTLIST a x BOGUS #IMPLIED>]><a/>"), [Code::DtdBadAttlist]);
        // (The tree ends the subset at the first `]`, so more than this one is reported for the rest.)
        assert!(codes("<!DOCTYPE a [<![INCLUDE[ ]]>]><a/>").contains(&Code::DtdConditionalInInternal));
        // The problem is where it is in the document.
        let text = "<?xml version='1.0'?>\n<!DOCTYPE a [\n<!ELEMENT a>\n]>\n<a/>";
        let found = analyze(text, 1);
        let at = found.diagnostics.first().map(|d| d.span.clone()).expect("a problem");
        assert!(at.start >= text.find("<!ELEMENT a>").expect("there") && at.end <= text.find("]>").expect("there"), "{at:?}");
        // One problem is one diagnostic: what the syntax tree finds inside the subset is not reported twice.
        assert_eq!(codes("<!DOCTYPE a [<!ELEMENT a EMPTY> text ]><a/>").len(), 1);
        // The external subset is not read here: an entity it may declare is not a mistake of the text.
        assert_eq!(codes("<!DOCTYPE a SYSTEM 'a.dtd' [%maybe; <!ELEMENT a EMPTY>]><a/>"), []);
        assert_eq!(codes("<!DOCTYPE a [%nowhere;]><a/>"), [Code::DtdUndeclaredPe]);
        // The problems count as errors of the document.
        assert!(!analyze("<!DOCTYPE a [<!ELEMENT a>]><a/>", 1).is_well_formed());
    }

    #[test]
    fn references_to_entities_are_checked_against_the_declarations_of_the_document() {
        use xmlcraft_syntax::Code;
        let codes = |text: &str| analyze(text, 1).diagnostics.iter().map(|d| d.code).collect::<Vec<_>>();
        assert_eq!(codes("<!DOCTYPE a [<!ENTITY e 'v'>]><a x='&e;'>&e;</a>"), []);
        assert_eq!(codes("<!DOCTYPE a [<!ELEMENT a ANY>]><a>&nope;</a>"), [Code::UndeclaredEntity]);
        assert_eq!(codes("<!DOCTYPE a [<!ENTITY e '&e;'>]><a>&e;</a>"), [Code::DtdRecursiveEntity]);
        assert_eq!(codes("<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a x='&e;'/>"), [Code::EntityExternalInAttribute]);
        // An external entity is not read here, and a declaration that may be outside the document is not missing.
        assert_eq!(codes("<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>"), []);
        assert_eq!(codes("<!DOCTYPE a SYSTEM 'a.dtd'><a>&nope;</a>"), []);
        // Without a DOCTYPE the parser has said it already, once.
        assert_eq!(codes("<a>&nope;</a>"), [Code::UndeclaredEntity]);
        assert!(!analyze("<!DOCTYPE a [<!ENTITY e '&e;'>]><a>&e;</a>", 1).is_well_formed());
    }

    use super::*;
    use proptest::prelude::*;

    #[test]
    fn a_well_formed_document_has_no_problems_and_an_outline() {
        let text = "<?xml version=\"1.0\"?>\n<library>\n  <book id=\"b1\" title=\"x\"><title>A</title></book>\n  <book name=\"two\"/>\n</library>";
        let a = analyze(text, 7);
        assert_eq!((a.version, a.is_well_formed(), a.error_count(), a.skipped), (7, true, 0, None));
        let names: Vec<(usize, &str, Option<&str>)> = a.outline.iter().map(|i| (i.depth, i.name.as_str(), i.label.as_deref())).collect();
        assert_eq!(names, [(0, "library", None), (1, "book", Some("b1")), (2, "title", None), (1, "book", Some("two"))]);
        let book = &a.outline[1];
        assert_eq!(&text[book.range.clone()], "<book id=\"b1\" title=\"x\"><title>A</title></book>");
        assert_eq!(&text[book.name_range.clone()], "book");
    }

    /// The tag texts a caret at the `|` in `marked` is matched with: (the tag it is at, the other one).
    fn matched(marked: &str) -> Option<(String, String)> {
        let offset = marked.find('|').expect("a caret");
        let text = marked.replacen('|', "", 1);
        let m = analyze(&text, 1).matching_tag(offset)?;
        Some((text[m.tag].to_owned(), text[m.partner].to_owned()))
    }

    fn pair(a: &str, b: &str) -> Option<(String, String)> {
        Some((a.to_owned(), b.to_owned()))
    }

    #[test]
    fn the_caret_in_a_tag_finds_the_other_tag_of_its_element() {
        // In the start tag: in its name, in an attribute, at its `<` and `>` (the `|` is the caret).
        assert_eq!(matched("<a><b i|d=\"1\">x</b></a>"), pair("<b id=\"1\">", "</b>"));
        assert_eq!(matched("<a><b|>x</b></a>"), pair("<b>", "</b>"));
        assert_eq!(matched("<a><|b>x</b></a>"), pair("<b>", "</b>"));
        // In the end tag.
        assert_eq!(matched("<a><b>x</b|></a>"), pair("</b>", "<b>"));
        assert_eq!(matched("<a><b>x</|b></a>"), pair("</b>", "<b>"));
        assert_eq!(matched("<a><b>x|</b></a>"), pair("</b>", "<b>"), "just before an end tag");
        // Next to a tag: after the `>` just typed, before the `<`.
        assert_eq!(matched("<a><b>|x</b></a>"), pair("<b>", "</b>"));
        assert_eq!(matched("<a>x|<b>x</b></a>"), pair("<b>", "</b>"));
        // Between two tags the one that ends at the caret wins over the one that starts there.
        assert_eq!(matched("<a><b></b>|</a>"), pair("</b>", "<b>"));
        assert_eq!(matched("<a>|<b></b></a>"), pair("<a>", "</a>"));
        assert_eq!(matched("<a><b>|<c></c></b></a>"), pair("<b>", "</b>"));
        assert_eq!(matched("<a><b></b><c>|</c></a>"), pair("<c>", "</c>"));
        // The root, and a namespace prefix.
        assert_eq!(matched("<r:a x:y='1'|>text</r:a>"), pair("<r:a x:y='1'>", "</r:a>"));
        assert_eq!(matched("<?xml version=\"1.0\"?>\n<a>\n  <b/>\n</a|>\n"), pair("</a>", "<a>"));
    }

    #[test]
    fn the_names_to_link_are_those_of_the_tag_the_caret_is_in_the_name_of() {
        let text = "<a><item id=\"1\">x</item><br/></a>";
        let analysis = analyze(text, 1);
        let (open, close) = (text.find("item").expect("there"), text.rfind("item").expect("there"));
        // In the name, at either end of it, and from the end tag.
        for column in 0..=4 {
            assert_eq!(analysis.linked_names(open + column), Some([open..open + 4, close..close + 4]), "start tag, column {column}");
            assert_eq!(analysis.linked_names(close + column), Some([close..close + 4, open..open + 4]), "end tag, column {column}");
        }
        // Elsewhere in the tag, in text, and where there is no pair.
        for (label, at) in
            [("an attribute", open + 8), ("text", text.find('x').expect("there")), ("an empty-element tag", text.find("br").expect("there"))]
        {
            assert_eq!(analysis.linked_names(at), None, "{label}");
        }
    }

    #[test]
    fn the_jump_keeps_the_place_in_the_name_where_it_can() {
        let text = "<a><item id=\"1\">x</item></a>";
        let m = analyze(text, 1).matching_tag(text.find("item").expect("there") + 1).expect("a match");
        // Inside the name `item`: the same column of the other `item`; at its start and end too.
        let name = text.find("item").expect("there");
        for column in 0..=4 {
            assert_eq!(m.jump_target(name + column), text.rfind("item").expect("there") + column);
        }
        // In an attribute: the start of the other name. From the end tag into a shorter start-tag name: not past it.
        assert_eq!(m.jump_target(name + 8), text.rfind("item").expect("there"));
        let text = "<a><b>x</bbbb></a>";
        let m = analyze(text, 1).matching_tag(text.find("bbbb").expect("there") + 3).expect("a match");
        assert_eq!(
            m.jump_target(text.find("bbbb").expect("there") + 3),
            text.find("<b>").expect("there") + 2,
            "one letter is all the other name has"
        );
    }

    #[test]
    fn at_the_boundary_between_siblings_the_end_tag_that_ends_there_wins_over_the_start_tag_that_starts_there() {
        assert_eq!(matched("<r><a></a>|<b></b></r>"), pair("</a>", "<a>"));
        assert_eq!(matched("<r><a/><b></b>|<c></c></r>"), pair("</b>", "<b>"));
        assert_eq!(matched("<r><a></a>|<b/></r>"), pair("</a>", "<a>"), "the empty-element tag after it has no other tag");
        assert_eq!(matched("<r>|<a></a><b></b></r>"), pair("<r>", "</r>"), "the start tag that ends here");
        assert_eq!(matched("<r><a></a><b>|</b></r>"), pair("<b>", "</b>"));
        // The same after a nested element ends, and at the end of the text.
        assert_eq!(matched("<r><a><x></x></a>|<b></b></r>"), pair("</a>", "<a>"));
        assert_eq!(matched("<a></a>|"), pair("</a>", "<a>"));
    }

    #[test]
    fn no_match_where_there_is_no_pair() {
        assert_eq!(matched("<a><b/|></a>"), None, "an empty-element tag has no other tag");
        assert_eq!(matched("<a>te|xt</a>"), None, "text");
        assert_eq!(matched("<a><!-- <b>| --></a>"), None, "a comment");
        assert_eq!(matched("<a><![CDATA[ <b> ]|]></a>"), None, "CDATA");
        assert_eq!(matched("<a><b|></a>"), None, "an element that was never closed");
        assert_eq!(matched("|"), None, "nothing");
        assert_eq!(matched("<a>x</a>|"), pair("</a>", "<a>"), "after the last tag");
        assert_eq!(matched("<a><b>x<c/>|</b></a>"), None, "the tag the caret is next to is an empty-element tag");
        // A `>` in an attribute value does not end the tag.
        assert_eq!(matched("<a><b x=\"y>z|\"></b></a>"), pair("<b x=\"y>z\">", "</b>"));
        assert_eq!(matched("<?xml version=\"1.0\"|?>\n<a/>"), None);
    }

    #[test]
    fn a_mismatched_end_tag_still_goes_with_its_element() {
        // While a name is being changed the pair is still the pair, whatever the names say.
        assert_eq!(matched("<a><bb|>x</b></a>"), pair("<bb>", "</b>"));
        assert_eq!(matched("<a><b>x</c|></a>"), pair("</c>", "<b>"));
    }

    #[test]
    fn the_other_tag_is_found_in_a_deep_and_a_wide_tree() {
        let deep = format!("{}x{}", "<a>".repeat(2000), "</a>".repeat(2000));
        let text = analyze(&deep, 1);
        let middle = 3 * 1000 + 1; // inside the 1001st start tag
        let m = text.matching_tag(middle).expect("a match");
        assert_eq!((&deep[m.tag.clone()], m.partner.start), ("<a>", 3 * 2000 + 1 + 4 * 999));
        let wide = format!("<r>{}</r>", "<i>x</i>".repeat(5000));
        let analysis = analyze(&wide, 1);
        let at = wide.rfind("</i>").expect("there") + 2;
        let m = analysis.matching_tag(at).expect("a match");
        assert_eq!((&wide[m.tag], &wide[m.partner]), ("</i>", "<i>"));
        let end = wide.len() - 2;
        let m = analysis.matching_tag(end).expect("the root's end tag");
        assert_eq!((&wide[m.tag], &wide[m.partner]), ("</r>", "<r>"));
    }

    #[test]
    fn problems_are_sorted_by_position_and_counted() {
        let a = analyze("<a><b></a>\n<c/>", 1);
        assert!(!a.is_well_formed());
        let starts: Vec<usize> = a.diagnostics.iter().map(|d| d.span.start).collect();
        assert!(starts.windows(2).all(|w| w[0] <= w[1]), "{starts:?}");
        assert!(a.error_count() >= 2, "an unclosed element and a second root");
        let warned = analyze("<?xml version=\"1.1\"?><a/>", 1);
        assert_eq!((warned.error_count(), warned.warning_count(), warned.is_well_formed()), (0, 1, true));
    }

    #[test]
    fn a_broken_document_still_has_an_outline() {
        let a = analyze("<a><b><c></b></a>", 1);
        assert_eq!(a.outline.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn deep_and_huge_documents_stay_bounded() {
        let deep = "<a>".repeat(100_000);
        let a = analyze(&deep, 1);
        // The syntax tree stops nesting at 1024 levels (deeper elements are checked but not nodes), so the outline does too.
        assert_eq!((a.outline.len(), a.outline.last().map(|i| i.depth)), (1024, Some(1023)));
        assert!(a.diagnostics.iter().any(|d| d.code == xmlcraft_syntax::Code::NestingTooDeep));
        let wide = format!("<r>{}</r>", "<x/>".repeat(MAX_OUTLINE_ITEMS + 10));
        let a = analyze(&wide, 1);
        assert_eq!((a.outline.len(), a.outline_truncated), (MAX_OUTLINE_ITEMS, true));
        assert!(a.outline.iter().all(|i| i.depth <= 1));
        let big = " ".repeat(MAX_ANALYSED_BYTES + 1);
        assert_eq!(analyze(&big, 1).skipped, Some(Skipped::TooLarge));
    }

    proptest! {
        /// Whatever the text, the outline's ranges are inside it and nest properly.
        #[test]
        fn outline_ranges_are_inside_the_text(pieces in proptest::collection::vec(proptest::sample::select(&["<a>", "</a>", "<b x=\"1\">", "</b>", "<c/>", "text", "&", "<!--", "-->", "<![CDATA[", "]]>", "\n", "é"][..]), 0..40)) {
            let text = pieces.concat();
            let a = analyze(&text, 1);
            for item in &a.outline {
                prop_assert!(item.range.start <= item.range.end && item.range.end <= text.len());
                prop_assert!(text.is_char_boundary(item.range.start) && text.is_char_boundary(item.range.end));
                prop_assert!(item.name_range.start >= item.range.start && item.name_range.end <= item.range.end);
                prop_assert_eq!(&text[item.name_range.clone()], item.name.as_str());
            }
            for d in &a.diagnostics {
                prop_assert!(d.span.end <= text.len());
            }
            for item in &a.outline {
                prop_assert!(item.range.start <= item.start_tag.start && item.start_tag.end <= item.range.end);
                prop_assert!(item.start_tag.start <= item.name_range.start && item.name_range.end <= item.start_tag.end);
                if let Some(end_tag) = &item.end_tag {
                    prop_assert!(item.start_tag.end <= end_tag.start && end_tag.end <= item.range.end);
                    prop_assert!(text.is_char_boundary(end_tag.start) && text.is_char_boundary(end_tag.end));
                }
                if let (Some(end_tag), Some(name)) = (&item.end_tag, &item.end_name_range) {
                    prop_assert!(end_tag.start <= name.start && name.end <= end_tag.end);
                }
                prop_assert_eq!(item.end_tag.is_some() || item.end_name_range.is_none(), true, "a name needs its tag");
            }
        }

        /// The tag pairs are symmetrical, and the caret is next to the tag it is matched with: for any text, any offset (also one past the
        /// end).
        #[test]
        fn matching_tags_are_symmetrical(
            pieces in proptest::collection::vec(proptest::sample::select(&["<a>", "</a>", "<b x=\"1>\">", "</b>", "<c/>", "text", "<!--", "-->", "\n", "é", "<![CDATA[", "]]>"][..]), 0..40),
            extra in 0usize..4,
        ) {
            let text = pieces.concat();
            let analysis = analyze(&text, 1);
            for offset in (0..text.len() + extra).filter(|o| text.is_char_boundary((*o).min(text.len()))) {
                let Some(m) = analysis.matching_tag(offset) else { continue };
                prop_assert!(m.tag.start <= offset && offset <= m.tag.end, "the caret is at its tag: {m:?} at {offset}");
                prop_assert!(m.tag.start <= m.name.start && m.name.end <= m.tag.end);
                prop_assert!(m.partner.start <= m.partner_name.start && m.partner_name.end <= m.partner.end);
                prop_assert!(m.tag.end <= m.partner.start || m.partner.end <= m.tag.start, "the tags are two places");
                // From inside the partner the match comes back the other way.
                let back = analysis.matching_tag(m.partner.start + 1).expect("the partner has a partner");
                prop_assert_eq!((back.tag, back.partner), (m.partner, m.tag));
            }
        }
    }

    #[test]
    fn the_worker_analyses_in_the_background_and_keeps_the_newest() {
        let analyzer = Analyzer::new();
        let (tx, rx) = std::sync::mpsc::channel();
        analyzer.on_done(move || {
            let _ = tx.send(());
        });
        let buffer = Buffer::from_text("<a><b></a>");
        analyzer.submit(1, 1, &buffer);
        analyzer.submit(1, 2, &Buffer::from_text("<a/>"));
        analyzer.submit(2, 5, &buffer);
        let mut results: Vec<(u64, Analysis)> = Vec::new();
        let have_everything =
            |results: &[(u64, Analysis)]| results.iter().any(|(d, a)| *d == 1 && a.version == 2) && results.iter().any(|(d, _)| *d == 2);
        while !have_everything(&results) {
            rx.recv_timeout(std::time::Duration::from_secs(10)).expect("a result arrives");
            results.extend(analyzer.poll());
        }
        let newest_of_one = results.iter().filter(|(d, _)| *d == 1).map(|(_, a)| a.version).max();
        assert_eq!(newest_of_one, Some(2));
        let two = results.iter().find(|(d, _)| *d == 2).map(|(_, a)| a).expect("document two");
        assert_eq!((two.version, two.is_well_formed()), (5, false));
        assert!(analyzer.poll().is_empty());
    }
}
