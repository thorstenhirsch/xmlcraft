#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-format: the pretty-printer and the minifier.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web too.
//! - Both work on the **white space between nodes** and nothing else. They return minimal [`Edit`]s of the text (ascending and
//!   disjoint, ready for `Editor::apply_edits`), never a rewritten document: every byte that is not white space between two nodes stays
//!   where it is, so quotes, attribute order, comments, CDATA sections, entity references and the DOCTYPE are exactly as they were.
//! - White space is only touched where it cannot mean anything: in the content of an element that has no text of its own (only
//!   elements, comments and processing instructions), outside the root element, and nowhere inside an element with
//!   `xml:space="preserve"` or an element with *mixed content* (text, character references, entity references or CDATA beside its child
//!   elements), whose subtrees are left alone. Without a schema that is the safe reading of what is "insignificant".
//! - A document that is not well-formed is refused ([`FormatError::NotWellFormed`]): the formatter does not guess what broken markup
//!   was meant to be.
//! - Formatting is idempotent: formatting the result again changes nothing.

use xmlcraft_syntax::{Parse, SyntaxElement, SyntaxKind, SyntaxNode, parse};
pub use xmlcraft_text::Edit;
pub use xmlcraft_text::eol::Eol;

/// How a level of nesting is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    /// This many spaces per level (at most [`MAX_INDENT`]).
    Spaces(usize),
    Tab,
}

/// What happens to blank lines between nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlankLines {
    /// A run of blank lines between two nodes becomes one blank line.
    Keep,
    /// No blank lines.
    Remove,
}

/// The most spaces per level.
pub const MAX_INDENT: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatOptions {
    pub indent: Indent,
    /// The line ending of every line break it writes.
    pub eol: Eol,
    pub blank_lines: BlankLines,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self { indent: Indent::Spaces(2), eol: Eol::Lf, blank_lines: BlankLines::Keep }
    }
}

/// Why nothing was done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// The text is not well-formed XML; this is the first problem (its message and byte offset).
    NotWellFormed { message: String, offset: usize },
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::NotWellFormed { message, offset } => {
                write!(f, "the document is not well-formed, so it is left as it is (at byte {offset}: {message})")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// The edits that pretty-print `text`: each node of an element's content on a line of its own, indented one level deeper than the
/// element, the end tag on a line of its own; one top-level node per line.
pub fn pretty_print(text: &str, options: &FormatOptions) -> Result<Vec<Edit>, FormatError> {
    run(text, Mode::Pretty(*options))
}

/// [`pretty_print`] for a document whose line ending is whichever it has most of: `options.eol` is the ending to write when the text
/// has no line break, and the one that most of the line breaks of the *result* use otherwise. That is not always the one most of the
/// breaks of `text` use: formatting drops blank lines, and with them the votes for their ending. Choosing it from the result is what
/// makes formatting the result again change nothing, however the text mixes endings.
pub fn pretty_print_settled(text: &str, options: &FormatOptions) -> Result<Vec<Edit>, FormatError> {
    use xmlcraft_text::eol::LineEndings;
    let own = LineEndings::count(text);
    let mut options = *options;
    if own.total() > 0 {
        options.eol = own.dominant();
    }
    let edits = pretty_print(text, &options)?;
    let result = endings_after(text, &edits, own);
    // The ending most of the result's breaks use, when that is another one: write that one.
    if result.total() > 0 && result.dominant() != options.eol {
        options.eol = result.dominant();
        return pretty_print(text, &options);
    }
    Ok(edits)
}

/// The line endings of `text` with `edits` applied (`own` are its own), counted from the endings the edits remove and write, without
/// building the result. The edits replace white space between nodes, whole runs of it, so none splits a `\r\n`.
fn endings_after(text: &str, edits: &[Edit], own: xmlcraft_text::eol::LineEndings) -> xmlcraft_text::eol::LineEndings {
    use xmlcraft_text::eol::LineEndings;
    let mut result = own;
    for edit in edits {
        let removed = text.get(edit.range.clone()).map(LineEndings::count).unwrap_or_default();
        let written = LineEndings::count(&edit.text);
        result.lf = result.lf.saturating_sub(removed.lf).saturating_add(written.lf);
        result.crlf = result.crlf.saturating_sub(removed.crlf).saturating_add(written.crlf);
        result.cr = result.cr.saturating_sub(removed.cr).saturating_add(written.cr);
    }
    result
}

/// The edits that remove the white space between nodes where it is insignificant (see the crate docs).
pub fn minify(text: &str) -> Result<Vec<Edit>, FormatError> {
    run(text, Mode::Minify)
}

/// `text` with `edits` applied. The edits must be ascending, disjoint and on character boundaries, as the functions of this crate
/// return them; anything else is left out rather than applied wrongly.
pub fn apply(text: &str, edits: &[Edit]) -> String {
    xmlcraft_text::apply_edits(text, edits)
}

#[derive(Clone, Copy)]
enum Mode {
    Pretty(FormatOptions),
    Minify,
}

fn run(text: &str, mode: Mode) -> Result<Vec<Edit>, FormatError> {
    let parsed = parse(text);
    refuse_broken(&parsed)?;
    let root = parsed.syntax();
    if usize::from(root.text_range().end()) != text.len() {
        // The parser gave up on the text (it was too large): there is no tree to go by.
        return Err(FormatError::NotWellFormed { message: "the text is too large to format".to_owned(), offset: 0 });
    }
    let mut edits = Vec::new();
    // Containers still to do: the document, then every element that can be formatted. A stack, not recursion.
    let mut work: Vec<(SyntaxNode, usize)> = vec![(root, 0)];
    while let Some((node, depth)) = work.pop() {
        let is_document = node.kind() == SyntaxKind::Document;
        let Some(content) = Content::of(&node, text, is_document) else { continue };
        content.gaps(text, depth, mode, &mut edits);
        // The root element is at depth 0, whatever the document around it holds.
        let child_depth = if is_document { 0 } else { depth + 1 };
        for child in content.elements {
            if space_of(&child, text) == Some(Space::Preserve) {
                // An element with `xml:space="preserve"` keeps its content as it is, and so does everything inside it, except an
                // element inside that says `xml:space="default"` again.
                default_islands(&child, child_depth, text, &mut work);
            } else {
                work.push((child, child_depth));
            }
        }
    }
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    Ok(edits)
}

fn refuse_broken(parsed: &Parse) -> Result<(), FormatError> {
    match parsed.diagnostics().iter().find(|d| d.severity() == xmlcraft_syntax::Severity::Error) {
        Some(d) => Err(FormatError::NotWellFormed { message: d.message(), offset: d.span.start }),
        None => Ok(()),
    }
}

fn range_of(node: &SyntaxNode) -> std::ops::Range<usize> {
    usize::from(node.text_range().start())..usize::from(node.text_range().end())
}

/// The nodes of a container and the stretches of white space around them.
struct Content {
    /// Where the content starts (just after the start tag; 0 for the document) and ends (the end tag; the end of the text).
    start: usize,
    end: usize,
    /// The nodes in order: elements, comments, processing instructions, and in the document the declaration and the DOCTYPE.
    nodes: Vec<std::ops::Range<usize>>,
    /// The elements among them, to be formatted in their turn.
    elements: Vec<SyntaxNode>,
    is_document: bool,
}

impl Content {
    /// The content of `node` if it can be formatted: it has child nodes, no text of its own, and is not flattened by the depth cap.
    fn of(node: &SyntaxNode, text: &str, is_document: bool) -> Option<Self> {
        let mut children: Vec<SyntaxElement> = node.children_with_tokens().collect();
        let (mut start, mut end) = (usize::from(node.text_range().start()), usize::from(node.text_range().end()));
        if !is_document {
            // `<start> ... </end>`; a self-closing element has no content, and one without an end tag is not well-formed.
            let first = children.first()?;
            let last = children.last()?;
            if first.kind() != SyntaxKind::StartTag || last.kind() != SyntaxKind::EndTag {
                return None;
            }
            start = usize::from(first.text_range().end());
            end = usize::from(last.text_range().start());
            children.remove(0);
            children.pop();
        }
        let mut nodes = Vec::new();
        let mut elements = Vec::new();
        for child in children {
            match child {
                SyntaxElement::Node(n) => match n.kind() {
                    SyntaxKind::Element => {
                        nodes.push(range_of(&n));
                        elements.push(n);
                    }
                    SyntaxKind::Comment | SyntaxKind::ProcessingInstruction => nodes.push(range_of(&n)),
                    SyntaxKind::XmlDecl | SyntaxKind::Doctype if is_document => nodes.push(range_of(&n)),
                    // A tag in the middle of the content: the tree was flattened past its depth cap. Leave it all alone.
                    _ => return None,
                },
                SyntaxElement::Token(t) => {
                    if t.kind() != SyntaxKind::Text
                        || !text.get(usize::from(t.text_range().start())..usize::from(t.text_range().end()))?.chars().all(is_space)
                    {
                        return None; // text, a reference or CDATA beside the nodes: mixed content (outside the root it would be an error)
                    }
                }
            }
        }
        (!nodes.is_empty()).then_some(Self { start, end, nodes, elements, is_document })
    }

    /// The edits for the white space before each node and after the last, for a container whose children are at `depth`.
    fn gaps(&self, text: &str, depth: usize, mode: Mode, edits: &mut Vec<Edit>) {
        let child_depth = if self.is_document { 0 } else { depth + 1 };
        let mut at = self.start;
        for (i, node) in self.nodes.iter().enumerate() {
            let gap = at..node.start;
            let desired = match mode {
                Mode::Minify => String::new(),
                Mode::Pretty(options) if self.is_document => {
                    if i == 0 {
                        String::new()
                    } else {
                        line_break(text, &gap, &options, 0, true)
                    }
                }
                Mode::Pretty(options) => line_break(text, &gap, &options, child_depth, i > 0),
            };
            push_if_different(text, gap, desired, edits);
            at = node.end;
        }
        let gap = at..self.end;
        let desired = match mode {
            Mode::Minify => String::new(),
            // After the last top-level node: one line break if there was white space (a file's final newline), none if there was none.
            Mode::Pretty(options) if self.is_document => {
                if gap.is_empty() {
                    String::new()
                } else {
                    options.eol.as_str().to_owned()
                }
            }
            Mode::Pretty(options) => line_break(text, &gap, &options, depth, false),
        };
        push_if_different(text, gap, desired, edits);
    }
}

fn push_if_different(text: &str, gap: std::ops::Range<usize>, desired: String, edits: &mut Vec<Edit>) {
    if text.get(gap.clone()) != Some(desired.as_str()) {
        edits.push(Edit::replace(gap, desired));
    }
}

/// A line break and the indentation of `depth` levels; with `keep_blank` and blank lines allowed, one blank line if `gap` had any.
fn line_break(text: &str, gap: &std::ops::Range<usize>, options: &FormatOptions, depth: usize, keep_blank: bool) -> String {
    let eol = options.eol.as_str();
    let breaks = count_line_breaks(text.get(gap.clone()).unwrap_or_default());
    let mut out = String::from(eol);
    if keep_blank && options.blank_lines == BlankLines::Keep && breaks >= 2 {
        out.push_str(eol);
    }
    match options.indent {
        Indent::Tab => out.extend(std::iter::repeat_n('\t', depth)),
        Indent::Spaces(n) => out.extend(std::iter::repeat_n(' ', depth.saturating_mul(n.min(MAX_INDENT)))),
    }
    out
}

fn count_line_breaks(gap: &str) -> usize {
    let mut count = 0;
    let mut chars = gap.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                count += 1;
            }
            '\n' => count += 1,
            _ => {}
        }
    }
    count
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// What an `xml:space` attribute says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Space {
    Preserve,
    Default,
}

/// What the start tag of `element` says with `xml:space` (`None`: nothing, or something that is neither value).
fn space_of(element: &SyntaxNode, text: &str) -> Option<Space> {
    let tag = element.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    tag.children().filter(|n| n.kind() == SyntaxKind::Attribute).find_map(|attribute| {
        let name = attribute.children_with_tokens().filter_map(SyntaxElement::into_token).find(|t| t.kind() == SyntaxKind::Name)?;
        let value = attribute.children().find(|n| n.kind() == SyntaxKind::AttrValue)?;
        if name.text() != "xml:space" {
            return None;
        }
        match text.get(range_of(&value))?.trim_matches(['"', '\'']).trim() {
            "preserve" => Some(Space::Preserve),
            "default" => Some(Space::Default),
            _ => None,
        }
    })
}

/// Inside the preserved element `root` (at `depth`): the elements that switch white space handling back to `default`, which are
/// formatted after all (what is inside them that preserves again stays as it is). They are put on `work` with their own depth. The walk
/// does not go through an element that cannot be formatted (mixed content, say): its subtree is left alone, islands included.
fn default_islands(root: &SyntaxNode, depth: usize, text: &str, work: &mut Vec<(SyntaxNode, usize)>) {
    let mut stack: Vec<(SyntaxNode, usize)> = vec![(root.clone(), depth)];
    while let Some((node, depth)) = stack.pop() {
        for child in node.children().filter(|n| n.kind() == SyntaxKind::Element) {
            if space_of(&child, text) == Some(Space::Default) {
                work.push((child, depth + 1));
            } else if Content::of(&child, text, false).is_some() {
                stack.push((child, depth + 1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endings_of_the_result_are_counted_without_building_it() {
        use xmlcraft_text::eol::LineEndings;
        for text in [
            "<a>\r\n  <b/>\r\n\r\n</a>",
            "<?l?><!--\r\n\r\n-->\n<foo>\n\n<a/>\n\n</foo>",
            "<a>\r<b/>\r\n<c/>\n</a>",
            "<a/>",
            "<a>\n\n\n<b>x</b>\n\n\n</a>\n\n",
        ] {
            for options in [FormatOptions::default(), FormatOptions { eol: Eol::CrLf, blank_lines: BlankLines::Remove, ..FormatOptions::default() }] {
                let edits = pretty_print(text, &options).expect("well-formed");
                assert_eq!(endings_after(text, &edits, LineEndings::count(text)), LineEndings::count(&apply(text, &edits)), "{text:?}");
            }
        }
    }

    fn pretty(text: &str) -> String {
        apply(text, &pretty_print(text, &FormatOptions::default()).expect("well-formed"))
    }

    fn mini(text: &str) -> String {
        apply(text, &minify(text).expect("well-formed"))
    }

    #[test]
    fn element_only_content_gets_one_node_per_line_and_one_level_per_depth() {
        assert_eq!(pretty("<a><b><c/></b><d/></a>"), "<a>\n  <b>\n    <c/>\n  </b>\n  <d/>\n</a>");
        assert_eq!(pretty("<a>\n\n\n<b/>\n</a>\n"), "<a>\n  <b/>\n</a>\n", "blank lines at the start and end of a block go");
        assert_eq!(pretty("<a>\n      <b/>\n   </a>"), "<a>\n  <b/>\n</a>", "wrong indentation is put right");
    }

    #[test]
    fn text_and_mixed_content_are_left_alone() {
        for src in [
            "<a>text</a>",
            "<a>one <b>two</b> three</a>",
            "<a><b>x</b>text<c/></a>",
            "<p>a<![CDATA[b]]>c<d/></p>",
            "<p>&amp;<d/><e/></p>",
            "<p>x&#65;<d/></p>",
        ] {
            assert_eq!(pretty(src), src, "{src}");
            assert_eq!(mini(src), src, "{src}");
        }
        // The element-only parent of a mixed one is still formatted; the mixed one's inside is not.
        assert_eq!(pretty("<r><p>a <b>b</b> c</p><q/></r>"), "<r>\n  <p>a <b>b</b> c</p>\n  <q/>\n</r>");
        // Even element-only descendants of a mixed element stay as they are.
        assert_eq!(pretty("<p>a<l><i/><i/></l></p>"), "<p>a<l><i/><i/></l></p>");
    }

    #[test]
    fn an_island_inside_mixed_content_is_left_alone_like_the_rest_of_it() {
        // The mixed element's subtree is left alone, also where `xml:space="default"` inside it would switch formatting back on in a preserved
        // region. (A property-test finding: the island was found by walking through the mixed element and then formatted.)
        let src = "<a xml:space=\"preserve\"><a><a xml:space='default' y='2'><a></a></a>text</a></a>";
        assert_eq!(pretty(src), src);
        assert_eq!(mini(src), src);
        // An element-only element between the preserved one and the island does not stop it.
        assert_eq!(
            pretty("<a xml:space=\"preserve\"><b><c xml:space='default'><d/></c></b></a>"),
            "<a xml:space=\"preserve\"><b><c xml:space='default'>\n      <d/>\n    </c></b></a>"
        );
        // The preserved element itself may have text: an island directly inside it is still formatted.
        assert_eq!(
            pretty("<a xml:space=\"preserve\">text<c xml:space='default'><d/></c></a>"),
            "<a xml:space=\"preserve\">text<c xml:space='default'>\n    <d/>\n  </c></a>"
        );
    }

    #[test]
    fn xml_space_preserve_protects_the_subtree_until_default_switches_it_back_on() {
        let src = "<a><pre xml:space=\"preserve\"><b/>\n   <c/></pre><d/></a>";
        assert_eq!(pretty(src), "<a>\n  <pre xml:space=\"preserve\"><b/>\n   <c/></pre>\n  <d/>\n</a>");
        assert_eq!(mini(src), "<a><pre xml:space=\"preserve\"><b/>\n   <c/></pre><d/></a>");
        // A single-quoted value counts, and so does a preserve on the root.
        assert_eq!(pretty("<a xml:space='preserve'><b/>\n  <c/></a>"), "<a xml:space='preserve'><b/>\n  <c/></a>");
        // Inside a preserved subtree an element that says `default` is formatted again (at its own depth), and a preserve inside it holds.
        assert_eq!(
            pretty("<a><pre xml:space=\"preserve\"><x xml:space=\"default\"><y/><p xml:space=\"preserve\"><q/> <r/></p></x></pre></a>"),
            "<a>\n  <pre xml:space=\"preserve\"><x xml:space=\"default\">\n      <y/>\n      <p xml:space=\"preserve\"><q/> <r/></p>\n    </x></pre>\n</a>"
        );
        // `default` is not `preserve`.
        assert_eq!(pretty("<a xml:space=\"default\"><b/><c/></a>"), "<a xml:space=\"default\">\n  <b/>\n  <c/>\n</a>");
    }

    #[test]
    fn comments_and_processing_instructions_are_nodes_and_keep_their_bytes() {
        assert_eq!(pretty("<a><!-- x  y --><?pi  data?><b/></a>"), "<a>\n  <!-- x  y -->\n  <?pi  data?>\n  <b/>\n</a>");
    }

    #[test]
    fn the_prolog_and_epilog_have_one_node_per_line() {
        assert_eq!(
            pretty("<?xml version=\"1.0\"?><!DOCTYPE a [<!ELEMENT a ANY>]><!--c--><a/><!--d-->"),
            "<?xml version=\"1.0\"?>\n<!DOCTYPE a [<!ELEMENT a ANY>]>\n<!--c-->\n<a/>\n<!--d-->"
        );
        assert_eq!(pretty("  \n<a/>   \n\n"), "<a/>\n", "leading white space goes and the final newline is one");
        assert_eq!(pretty("<a/>"), "<a/>", "no final newline is added");
        assert_eq!(pretty("<a/>\n\n\n<!--c-->\n"), "<a/>\n\n<!--c-->\n", "a run of blank lines between nodes becomes one");
    }

    #[test]
    fn options_change_the_indent_the_line_ending_and_the_blank_lines() {
        let src = "<a>\r\n\r\n<b><c/></b>\r\n\r\n<d/></a>";
        let tabs = FormatOptions { indent: Indent::Tab, eol: Eol::CrLf, blank_lines: BlankLines::Keep };
        assert_eq!(apply(src, &pretty_print(src, &tabs).expect("ok")), "<a>\r\n\t<b>\r\n\t\t<c/>\r\n\t</b>\r\n\r\n\t<d/>\r\n</a>");
        let four = FormatOptions { indent: Indent::Spaces(4), eol: Eol::Lf, blank_lines: BlankLines::Remove };
        assert_eq!(apply(src, &pretty_print(src, &four).expect("ok")), "<a>\n    <b>\n        <c/>\n    </b>\n    <d/>\n</a>");
        let huge = FormatOptions { indent: Indent::Spaces(1000), ..FormatOptions::default() };
        assert!(apply("<a><b/></a>", &pretty_print("<a><b/></a>", &huge).expect("ok")).contains(&" ".repeat(MAX_INDENT)));
    }

    #[test]
    fn minifying_removes_only_insignificant_white_space() {
        assert_eq!(
            mini("<?xml version=\"1.0\"?>\n<a>\n  <b> x </b>\n  <!-- c -->\n  <c/>\n</a>\n"),
            "<?xml version=\"1.0\"?><a><b> x </b><!-- c --><c/></a>"
        );
        assert_eq!(mini("<a>  </a>"), "<a>  </a>", "white space alone is not a gap between nodes");
    }

    #[test]
    fn edits_are_minimal_and_nothing_changes_when_nothing_is_wrong() {
        let src = "<a>\n  <b/>\n  <c>\n    <d/>\n  </c>\n</a>\n";
        assert_eq!(pretty_print(src, &FormatOptions::default()).expect("ok"), vec![], "already formatted");
        let sloppy = "<a>\n  <b/><c>\n<d/></c>\n</a>\n";
        let edits = pretty_print(sloppy, &FormatOptions::default()).expect("ok");
        assert!(edits.iter().all(|e| sloppy[e.range.clone()].chars().all(is_space)), "only white space is replaced: {edits:?}");
        assert!(edits.windows(2).all(|w| w[0].range.end <= w[1].range.start), "ascending and disjoint");
        assert_eq!(apply(sloppy, &edits), src);
    }

    #[test]
    fn formatting_twice_changes_nothing() {
        for src in
            ["<a><b/><c><d/>\n\n\n<e/></c></a>", "<?xml version=\"1.0\"?>\n\n\n<!--x--><r>  <a>t</a>  <b> <c/> </b></r>\n\n", "<a><!--c--><b/></a>"]
        {
            let once = pretty(src);
            assert_eq!(pretty(&once), once, "{src}");
            let small = mini(src);
            assert_eq!(mini(&small), small, "{src}");
        }
    }

    #[test]
    fn a_document_that_is_not_well_formed_is_refused() {
        for src in ["<a><b></a>", "<a>", "text", "<a></b>", "<a b=1/>", ""] {
            let error = pretty_print(src, &FormatOptions::default()).expect_err(src);
            assert!(matches!(error, FormatError::NotWellFormed { .. }), "{src}");
            assert!(minify(src).is_err());
            assert!(error.to_string().contains("not well-formed"));
        }
    }

    #[test]
    fn very_deep_documents_do_not_overflow_the_stack() {
        let depth = 5000;
        let src = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        // Past the tree's depth cap the elements stop nesting; the formatter leaves what it cannot place and does not crash.
        // Past the tree's depth cap the elements stop nesting: the document is still well-formed (it only has a warning), and what is
        // done to it keeps it so, with every element still there.
        let edits = pretty_print(&src, &FormatOptions::default()).expect("a deep document is well-formed");
        let out = apply(&src, &edits);
        assert!(!parse(&out).has_errors());
        assert_eq!(out.matches("<a>").count(), depth);
        let wide = format!("<r>{}</r>", "<x/>".repeat(20_000));
        let out = pretty(&wide);
        assert_eq!(out.lines().count(), 20_002);
    }

    #[test]
    fn apply_ignores_edits_that_do_not_fit() {
        let edits = [Edit::replace(1..2, "X".to_owned()), Edit::replace(100..200, "never".to_owned())];
        assert_eq!(apply("abc", &edits), "aXc");
        assert_eq!(apply("é", &[Edit::replace(1..2, "x".to_owned())]), "é", "not on a character boundary");
    }
}
