//! What a formatter needs to know about the layout of a YAML text: which parts are scalars or flow collections that run over several
//! lines (whose inner lines are not structure), where the block scalars are, and a fingerprint of the events the parser reads, so that a
//! change of white space can be checked not to have changed the document.

use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::hash::Hasher;
use std::ops::Range;

use saphyr_parser::{Event, Parser, ScalarStyle, Span as YamlSpan, SpannedEventReceiver};

use crate::parser::Offsets;

/// The places of a text that are not made of lines of block structure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// Plain and quoted scalars and flow collections (`[…]`, `{…}`) that run over more than one line, outermost only, ascending. The
    /// lines inside one belong to it: the indentation of a line inside is not the structure of the document.
    pub multiline: Vec<Range<usize>>,
    /// Block scalars (`|`, `>`), ascending: from the first character of the content to the next token (or the end of the text), so the
    /// blank lines after the content are inside. The header (`|`, `>-` …) is before the range.
    pub blocks: Vec<Range<usize>>,
    /// A fingerprint of the events (the structure, the anchors, the tags, the style and the text of every scalar): equal fingerprints
    /// are two readings of the same document.
    pub fingerprint: u64,
}

/// Why a text has no layout: the parser's message and the byte offset it stopped at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutError {
    pub message: String,
    pub offset: usize,
}

/// The layout of `text`, or the first syntax error. Never panics; the parser's work is proportional to the text.
pub fn layout(text: &str) -> Result<Layout, LayoutError> {
    if let Some(offset) = crate::parser::too_deep(text) {
        return Err(LayoutError { message: format!("values nested deeper than {} levels are not read", crate::parser::PARSER_DEPTH_LIMIT), offset });
    }
    let offsets = Offsets::new(text);
    let mut collector =
        Collector { text, offsets: &offsets, multiline: Vec::new(), blocks: Vec::new(), open: Vec::new(), hasher: DefaultHasher::new() };
    let mut parser = Parser::new_from_str(text);
    if let Err(error) = parser.load(&mut collector, true) {
        return Err(LayoutError { message: error.info().to_owned(), offset: offsets.byte(*error.marker()) });
    }
    collector.multiline.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    let mut outer: Vec<Range<usize>> = Vec::new();
    for range in collector.multiline {
        if outer.last().is_none_or(|last| range.start >= last.end) {
            outer.push(range);
        }
    }
    collector.blocks.sort_by_key(|r| r.start);
    Ok(Layout { multiline: outer, blocks: collector.blocks, fingerprint: collector.hasher.finish() })
}

struct Collector<'a> {
    text: &'a str,
    offsets: &'a Offsets<'a>,
    multiline: Vec<Range<usize>>,
    blocks: Vec<Range<usize>>,
    /// The starts of the collections that are open.
    open: Vec<usize>,
    hasher: DefaultHasher,
}

/// Writes what `Debug` says into a hasher.
struct HashWriter<'a>(&'a mut DefaultHasher);

impl std::fmt::Write for HashWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

impl<'input> SpannedEventReceiver<'input> for Collector<'_> {
    fn on_event(&mut self, event: Event<'input>, span: YamlSpan) {
        // (Writing into a hasher cannot fail.)
        let _ = write!(HashWriter(&mut self.hasher), "{event:?};");
        let range = self.offsets.span(span);
        match &event {
            Event::Scalar(_, style, _, _) => match style {
                ScalarStyle::Literal | ScalarStyle::Folded => self.blocks.push(range),
                _ => {
                    if self.text.get(range.clone()).is_some_and(|t| t.contains(['\n', '\r'])) {
                        self.multiline.push(range);
                    }
                }
            },
            Event::SequenceStart(..) | Event::MappingStart(..) => self.open.push(range.start),
            Event::SequenceEnd | Event::MappingEnd => {
                let Some(start) = self.open.pop() else { return };
                // A flow collection ends with its bracket; a block collection ends where the next token starts.
                let flow = self.text.get(range.clone()).is_some_and(|t| t == "]" || t == "}");
                if flow && start < range.end && self.text.get(start..range.end).is_some_and(|t| t.contains(['\n', '\r'])) {
                    self.multiline.push(start..range.end);
                }
            }
            _ => {}
        }
    }
}
