//! Completion in an XSLT stylesheet: after `<` the `xsl:` elements that may stand where the caret is (declarations at the top, instructions
//! inside a template, `when` inside `choose` …), in a start tag the attributes the element takes, inside the quotes the values an attribute
//! lists or the names the stylesheet itself defines (`call-template`, `mode`, `use-attribute-sets`), and in an XPath attribute the names of
//! functions. What may stand where comes from the public XSLT 3.0 specification (W3C), written down here as tables; the place of the caret
//! is found by reading the text before it (a tag that is being typed has no tree yet).

use std::ops::Range;

use crate::tag_context::{Place, is_name_char, tag_context};

/// The most items offered.
pub const MAX_ITEMS: usize = 300;
/// The most bytes of the start of a document searched for the declaration of the XSLT namespace.
const ROOT_LOOK: usize = 64 * 1024;
/// The deepest nesting followed when working out the parent of the caret.
const MAX_DEPTH: usize = 512;
/// The most bytes of a tag read when collecting the names a stylesheet defines.
const MAX_TAG: usize = 4 * 1024;
/// The namespace of XSLT.
pub const XSLT_NS: &str = "http://www.w3.org/1999/XSL/Transform";

/// What is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XsltKind {
    Element,
    Attribute,
    Value,
    Function,
}

/// One thing to offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XsltItem {
    pub label: String,
    /// What is written in place of `replace`, and where the caret goes in it (bytes from its start).
    pub insert: String,
    pub cursor: usize,
    pub detail: String,
}

/// What is offered at a place and what it replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XsltCompletion {
    pub kind: XsltKind,
    pub replace: Range<usize>,
    pub items: Vec<XsltItem>,
}

/// Where an element may stand.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Where {
    /// A declaration: a child of the stylesheet.
    Top,
    /// An instruction: inside a template, a function or another instruction.
    Instruction,
    /// Both (`variable`, `param` …).
    Both,
    /// Only inside the elements named (`when` in `choose`).
    Only(&'static [&'static str]),
}

struct Element {
    name: &'static str,
    /// The attributes it takes; the first is the one a person writes first.
    attributes: &'static [&'static str],
    /// It has no content.
    empty: bool,
    at: Where,
}

const fn el(name: &'static str, attributes: &'static [&'static str], empty: bool, at: Where) -> Element {
    Element { name, attributes, empty, at }
}

use Where::{Both, Instruction, Only, Top};

const ELEMENTS: &[Element] = &[
    el("accumulator", &["name", "initial-value", "as", "streamable"], false, Top),
    el("accumulator-rule", &["match", "phase", "select"], false, Only(&["accumulator"])),
    el("analyze-string", &["select", "regex", "flags"], false, Instruction),
    el("apply-imports", &[], false, Instruction),
    el("apply-templates", &["select", "mode"], false, Instruction),
    el("assert", &["test", "select", "error-code"], false, Instruction),
    el("attribute", &["name", "namespace", "select", "separator", "type", "validation"], false, Instruction),
    el("attribute-set", &["name", "use-attribute-sets", "visibility"], false, Top),
    el("break", &["select"], false, Instruction),
    el("call-template", &["name"], false, Instruction),
    el("catch", &["errors", "select"], false, Only(&["try"])),
    el("character-map", &["name", "use-character-maps"], false, Top),
    el("choose", &[], false, Instruction),
    el("comment", &["select"], false, Instruction),
    el("context-item", &["as", "use"], true, Only(&["template", "function"])),
    el("copy", &["select", "copy-namespaces", "inherit-namespaces", "use-attribute-sets", "type", "validation"], false, Instruction),
    el("copy-of", &["select", "copy-accumulators", "copy-namespaces", "type", "validation"], true, Instruction),
    el(
        "decimal-format",
        &[
            "name",
            "decimal-separator",
            "grouping-separator",
            "infinity",
            "minus-sign",
            "NaN",
            "percent",
            "per-mille",
            "zero-digit",
            "digit",
            "pattern-separator",
            "exponent-separator",
        ],
        true,
        Top,
    ),
    el("document", &["validation", "type"], false, Instruction),
    el("element", &["name", "namespace", "inherit-namespaces", "use-attribute-sets", "type", "validation"], false, Instruction),
    el("evaluate", &["xpath", "as", "context-item", "base-uri", "with-params", "namespace-context", "schema-aware"], false, Instruction),
    el("fallback", &[], false, Instruction),
    el("for-each", &["select"], false, Instruction),
    el(
        "for-each-group",
        &["select", "group-by", "group-adjacent", "group-starting-with", "group-ending-with", "composite", "collation"],
        false,
        Instruction,
    ),
    el("fork", &[], false, Instruction),
    el("function", &["name", "as", "visibility", "override-extension-function", "new-each-time", "cache"], false, Top),
    el("global-context-item", &["as", "use"], true, Top),
    el("if", &["test"], false, Instruction),
    el("import", &["href"], true, Top),
    el("import-schema", &["namespace", "schema-location"], false, Top),
    el("include", &["href"], true, Top),
    el("iterate", &["select"], false, Instruction),
    el("key", &["name", "match", "use", "composite", "collation"], false, Top),
    el("map", &[], false, Instruction),
    el("map-entry", &["key", "select"], false, Instruction),
    el("matching-substring", &[], false, Only(&["analyze-string"])),
    el("merge", &[], false, Instruction),
    el("merge-action", &[], false, Only(&["merge"])),
    el("merge-key", &["select", "lang", "order", "collation", "case-order", "data-type"], false, Only(&["merge-source"])),
    el(
        "merge-source",
        &["name", "for-each-item", "for-each-source", "select", "use-accumulators", "streamable", "sort-before-merge", "validation", "type"],
        false,
        Only(&["merge"]),
    ),
    el("message", &["select", "terminate", "error-code"], false, Instruction),
    el(
        "mode",
        &[
            "name",
            "streamable",
            "use-accumulators",
            "on-no-match",
            "on-multiple-match",
            "warning-on-no-match",
            "warning-on-multiple-match",
            "typed",
            "visibility",
        ],
        true,
        Top,
    ),
    el("namespace", &["name", "select"], false, Instruction),
    el("namespace-alias", &["stylesheet-prefix", "result-prefix"], true, Top),
    el("next-iteration", &[], false, Only(&["iterate"])),
    el("next-match", &[], false, Instruction),
    el("non-matching-substring", &[], false, Only(&["analyze-string"])),
    el(
        "number",
        &[
            "value",
            "select",
            "level",
            "count",
            "from",
            "format",
            "lang",
            "letter-value",
            "ordinal",
            "start-at",
            "grouping-separator",
            "grouping-size",
        ],
        true,
        Instruction,
    ),
    el("on-completion", &["select"], false, Only(&["iterate"])),
    el("on-empty", &["select"], false, Instruction),
    el("on-non-empty", &["select"], false, Instruction),
    el("otherwise", &[], false, Only(&["choose"])),
    el(
        "output",
        &[
            "name",
            "method",
            "version",
            "encoding",
            "indent",
            "omit-xml-declaration",
            "standalone",
            "doctype-public",
            "doctype-system",
            "cdata-section-elements",
            "media-type",
            "byte-order-mark",
            "escape-uri-attributes",
            "include-content-type",
            "normalization-form",
            "undeclare-prefixes",
            "parameter-document",
            "html-version",
            "item-separator",
            "suppress-indentation",
            "use-character-maps",
            "build-tree",
        ],
        true,
        Top,
    ),
    el("output-character", &["character", "string"], true, Only(&["character-map"])),
    el("param", &["name", "select", "as", "required", "tunnel"], false, Both),
    el("perform-sort", &["select"], false, Instruction),
    el("preserve-space", &["elements"], true, Top),
    el("processing-instruction", &["name", "select"], false, Instruction),
    el("result-document", &["format", "href", "method", "indent", "encoding", "validation", "type"], false, Instruction),
    el("sequence", &["select"], false, Instruction),
    el(
        "sort",
        &["select", "lang", "order", "collation", "stable", "case-order", "data-type"],
        false,
        Only(&["apply-templates", "for-each", "for-each-group", "perform-sort"]),
    ),
    el("source-document", &["href", "streamable", "use-accumulators", "validation", "type"], false, Instruction),
    el("strip-space", &["elements"], true, Top),
    el("template", &["match", "name", "priority", "mode", "as", "visibility"], false, Top),
    el("text", &["disable-output-escaping"], false, Instruction),
    el("try", &["select", "rollback-output"], false, Instruction),
    el("use-package", &["name", "package-version"], false, Top),
    el("value-of", &["select", "separator"], true, Instruction),
    el("variable", &["name", "select", "as", "static", "visibility"], false, Both),
    el("when", &["test"], false, Only(&["choose"])),
    el("where-populated", &[], false, Instruction),
    el(
        "with-param",
        &["name", "select", "as", "tunnel"],
        false,
        Only(&["apply-templates", "call-template", "apply-imports", "next-match", "evaluate", "next-iteration"]),
    ),
];

/// Attributes of the root element (and of any `xsl:` element).
const ROOT_ATTRIBUTES: &[&str] = &[
    "version",
    "id",
    "default-mode",
    "default-validation",
    "input-type-annotations",
    "default-collation",
    "extension-element-prefixes",
    "exclude-result-prefixes",
    "expand-text",
    "use-when",
    "xpath-default-namespace",
];
/// Attributes any `xsl:` element may carry.
const COMMON_ATTRIBUTES: &[&str] =
    &["use-when", "xpath-default-namespace", "default-collation", "expand-text", "exclude-result-prefixes", "extension-element-prefixes"];

const YES_NO: &[&str] = &["yes", "no"];

/// The values an attribute lists.
fn fixed_values(attribute: &str) -> &'static [&'static str] {
    match attribute {
        "stable"
        | "indent"
        | "omit-xml-declaration"
        | "tunnel"
        | "required"
        | "disable-output-escaping"
        | "expand-text"
        | "inherit-namespaces"
        | "copy-namespaces"
        | "copy-accumulators"
        | "composite"
        | "static"
        | "terminate"
        | "escape-uri-attributes"
        | "include-content-type"
        | "byte-order-mark"
        | "undeclare-prefixes"
        | "streamable"
        | "new-each-time"
        | "schema-aware"
        | "warning-on-no-match"
        | "warning-on-multiple-match"
        | "build-tree"
        | "sort-before-merge"
        | "rollback-output" => YES_NO,
        "standalone" => &["yes", "no", "omit"],
        "order" => &["ascending", "descending"],
        "case-order" => &["upper-first", "lower-first"],
        "data-type" => &["text", "number"],
        "method" => &["xml", "html", "xhtml", "text", "json", "adaptive"],
        "level" => &["single", "multiple", "any"],
        "letter-value" => &["alphabetic", "traditional"],
        "validation" => &["strict", "lax", "preserve", "strip"],
        "default-validation" => &["preserve", "strip"],
        "input-type-annotations" => &["preserve", "strip", "unspecified"],
        "visibility" => &["public", "private", "final", "abstract", "hidden"],
        "on-no-match" => &["deep-copy", "shallow-copy", "deep-skip", "shallow-skip", "fail"],
        "on-multiple-match" => &["use-last", "fail"],
        "normalization-form" => &["NFC", "NFD", "NFKC", "NFKD", "fully-normalized", "none"],
        "phase" => &["start", "end"],
        "use" => &["required", "optional", "absent"],
        "version" => &["1.0", "2.0", "3.0"],
        "html-version" => &["5"],
        _ => &[],
    }
}

/// Attributes that hold an XPath expression (or a pattern).
const EXPRESSIONS: &[&str] = &[
    "select",
    "test",
    "match",
    "use",
    "group-by",
    "group-adjacent",
    "group-starting-with",
    "group-ending-with",
    "value",
    "from",
    "count",
    "xpath",
    "initial-value",
    "for-each-item",
    "for-each-source",
    "key",
];

/// The functions of XPath and XQuery 3.1 (`fn:`, `math:`, `map:`, `array:`) with their signatures, and the XSLT functions.
const FUNCTIONS: &[(&str, &str)] = &[
    ("abs", "abs($arg)"),
    ("accumulator-after", "accumulator-after($name)"),
    ("accumulator-before", "accumulator-before($name)"),
    ("adjust-date-to-timezone", "adjust-date-to-timezone($arg, $timezone?)"),
    ("adjust-dateTime-to-timezone", "adjust-dateTime-to-timezone($arg, $timezone?)"),
    ("adjust-time-to-timezone", "adjust-time-to-timezone($arg, $timezone?)"),
    ("analyze-string", "analyze-string($input, $pattern, $flags?)"),
    ("available-system-properties", "available-system-properties()"),
    ("avg", "avg($arg)"),
    ("base-uri", "base-uri($arg?)"),
    ("boolean", "boolean($arg)"),
    ("ceiling", "ceiling($arg)"),
    ("codepoint-equal", "codepoint-equal($a, $b)"),
    ("codepoints-to-string", "codepoints-to-string($arg)"),
    ("collation-key", "collation-key($key, $collation?)"),
    ("collection", "collection($arg?)"),
    ("compare", "compare($a, $b, $collation?)"),
    ("concat", "concat($a, $b, …)"),
    ("contains", "contains($arg1, $arg2, $collation?)"),
    ("contains-token", "contains-token($input, $token, $collation?)"),
    ("copy-of", "copy-of($input?)"),
    ("count", "count($arg)"),
    ("current", "current()"),
    ("current-date", "current-date()"),
    ("current-dateTime", "current-dateTime()"),
    ("current-group", "current-group()"),
    ("current-grouping-key", "current-grouping-key()"),
    ("current-merge-group", "current-merge-group($source?)"),
    ("current-merge-key", "current-merge-key()"),
    ("current-output-uri", "current-output-uri()"),
    ("current-time", "current-time()"),
    ("data", "data($arg?)"),
    ("dateTime", "dateTime($date, $time)"),
    ("day-from-date", "day-from-date($arg)"),
    ("day-from-dateTime", "day-from-dateTime($arg)"),
    ("days-from-duration", "days-from-duration($arg)"),
    ("deep-equal", "deep-equal($a, $b, $collation?)"),
    ("default-collation", "default-collation()"),
    ("default-language", "default-language()"),
    ("distinct-values", "distinct-values($arg, $collation?)"),
    ("doc", "doc($uri)"),
    ("doc-available", "doc-available($uri)"),
    ("document", "document($uri-sequence, $base-node?)"),
    ("document-uri", "document-uri($arg?)"),
    ("element-available", "element-available($name)"),
    ("element-with-id", "element-with-id($arg, $node?)"),
    ("empty", "empty($arg)"),
    ("encode-for-uri", "encode-for-uri($uri-part)"),
    ("ends-with", "ends-with($arg1, $arg2, $collation?)"),
    ("environment-variable", "environment-variable($name)"),
    ("error", "error($code?, $description?, $object?)"),
    ("escape-html-uri", "escape-html-uri($uri)"),
    ("exactly-one", "exactly-one($arg)"),
    ("exists", "exists($arg)"),
    ("false", "false()"),
    ("filter", "filter($seq, $f)"),
    ("floor", "floor($arg)"),
    ("fold-left", "fold-left($seq, $zero, $f)"),
    ("fold-right", "fold-right($seq, $zero, $f)"),
    ("for-each", "for-each($seq, $action)"),
    ("for-each-pair", "for-each-pair($seq1, $seq2, $action)"),
    ("format-date", "format-date($value, $picture, $language?, $calendar?, $place?)"),
    ("format-dateTime", "format-dateTime($value, $picture, $language?, $calendar?, $place?)"),
    ("format-integer", "format-integer($value, $picture, $lang?)"),
    ("format-number", "format-number($value, $picture, $decimal-format-name?)"),
    ("format-time", "format-time($value, $picture, $language?, $calendar?, $place?)"),
    ("function-arity", "function-arity($f)"),
    ("function-available", "function-available($name, $arity?)"),
    ("function-lookup", "function-lookup($name, $arity)"),
    ("function-name", "function-name($func)"),
    ("generate-id", "generate-id($node?)"),
    ("has-children", "has-children($node?)"),
    ("head", "head($arg)"),
    ("hours-from-dateTime", "hours-from-dateTime($arg)"),
    ("hours-from-duration", "hours-from-duration($arg)"),
    ("hours-from-time", "hours-from-time($arg)"),
    ("id", "id($arg, $node?)"),
    ("idref", "idref($arg, $node?)"),
    ("implicit-timezone", "implicit-timezone()"),
    ("in-scope-prefixes", "in-scope-prefixes($element)"),
    ("index-of", "index-of($seq, $search, $collation?)"),
    ("innermost", "innermost($nodes)"),
    ("insert-before", "insert-before($target, $position, $inserts)"),
    ("iri-to-uri", "iri-to-uri($iri)"),
    ("json-doc", "json-doc($href, $options?)"),
    ("json-to-xml", "json-to-xml($json-text, $options?)"),
    ("key", "key($name, $value, $top?)"),
    ("lang", "lang($testlang, $node?)"),
    ("last", "last()"),
    ("load-xquery-module", "load-xquery-module($module-uri, $options?)"),
    ("local-name", "local-name($arg?)"),
    ("local-name-from-QName", "local-name-from-QName($arg)"),
    ("lower-case", "lower-case($arg)"),
    ("matches", "matches($input, $pattern, $flags?)"),
    ("max", "max($arg, $collation?)"),
    ("min", "min($arg, $collation?)"),
    ("minutes-from-dateTime", "minutes-from-dateTime($arg)"),
    ("minutes-from-duration", "minutes-from-duration($arg)"),
    ("minutes-from-time", "minutes-from-time($arg)"),
    ("month-from-date", "month-from-date($arg)"),
    ("month-from-dateTime", "month-from-dateTime($arg)"),
    ("months-from-duration", "months-from-duration($arg)"),
    ("name", "name($arg?)"),
    ("namespace-uri", "namespace-uri($arg?)"),
    ("namespace-uri-for-prefix", "namespace-uri-for-prefix($prefix, $element)"),
    ("namespace-uri-from-QName", "namespace-uri-from-QName($arg)"),
    ("nilled", "nilled($arg?)"),
    ("node-name", "node-name($arg?)"),
    ("normalize-space", "normalize-space($arg?)"),
    ("normalize-unicode", "normalize-unicode($arg, $normalization-form?)"),
    ("not", "not($arg)"),
    ("number", "number($arg?)"),
    ("one-or-more", "one-or-more($arg)"),
    ("outermost", "outermost($nodes)"),
    ("parse-ietf-date", "parse-ietf-date($value)"),
    ("parse-json", "parse-json($json-text, $options?)"),
    ("parse-xml", "parse-xml($arg)"),
    ("parse-xml-fragment", "parse-xml-fragment($arg)"),
    ("path", "path($arg?)"),
    ("position", "position()"),
    ("prefix-from-QName", "prefix-from-QName($arg)"),
    ("random-number-generator", "random-number-generator($seed?)"),
    ("regex-group", "regex-group($group-number)"),
    ("remove", "remove($target, $position)"),
    ("replace", "replace($input, $pattern, $replacement, $flags?)"),
    ("resolve-QName", "resolve-QName($qname, $element)"),
    ("resolve-uri", "resolve-uri($relative, $base?)"),
    ("reverse", "reverse($arg)"),
    ("root", "root($arg?)"),
    ("round", "round($arg, $precision?)"),
    ("round-half-to-even", "round-half-to-even($arg, $precision?)"),
    ("seconds-from-dateTime", "seconds-from-dateTime($arg)"),
    ("seconds-from-duration", "seconds-from-duration($arg)"),
    ("seconds-from-time", "seconds-from-time($arg)"),
    ("serialize", "serialize($arg, $params?)"),
    ("snapshot", "snapshot($input?)"),
    ("sort", "sort($input, $collation?, $key?)"),
    ("starts-with", "starts-with($arg1, $arg2, $collation?)"),
    ("static-base-uri", "static-base-uri()"),
    ("string", "string($arg?)"),
    ("string-join", "string-join($input, $separator?)"),
    ("string-length", "string-length($arg?)"),
    ("string-to-codepoints", "string-to-codepoints($arg)"),
    ("subsequence", "subsequence($sourceSeq, $startingLoc, $length?)"),
    ("substring", "substring($sourceString, $start, $length?)"),
    ("substring-after", "substring-after($arg1, $arg2, $collation?)"),
    ("substring-before", "substring-before($arg1, $arg2, $collation?)"),
    ("sum", "sum($arg, $zero?)"),
    ("system-property", "system-property($name)"),
    ("tail", "tail($arg)"),
    ("timezone-from-date", "timezone-from-date($arg)"),
    ("timezone-from-dateTime", "timezone-from-dateTime($arg)"),
    ("timezone-from-time", "timezone-from-time($arg)"),
    ("tokenize", "tokenize($input, $pattern?, $flags?)"),
    ("trace", "trace($value, $label?)"),
    ("transform", "transform($options)"),
    ("translate", "translate($arg, $mapString, $transString)"),
    ("true", "true()"),
    ("type-available", "type-available($name)"),
    ("unordered", "unordered($sourceSeq)"),
    ("unparsed-entity-public-id", "unparsed-entity-public-id($entity-name)"),
    ("unparsed-entity-uri", "unparsed-entity-uri($entity-name)"),
    ("unparsed-text", "unparsed-text($href, $encoding?)"),
    ("unparsed-text-available", "unparsed-text-available($href, $encoding?)"),
    ("unparsed-text-lines", "unparsed-text-lines($href, $encoding?)"),
    ("upper-case", "upper-case($arg)"),
    ("uri-collection", "uri-collection($arg?)"),
    ("xml-to-json", "xml-to-json($input, $options?)"),
    ("year-from-date", "year-from-date($arg)"),
    ("year-from-dateTime", "year-from-dateTime($arg)"),
    ("years-from-duration", "years-from-duration($arg)"),
    ("zero-or-one", "zero-or-one($arg)"),
];

/// The prefix the XSLT namespace is bound to in the root element (empty for the default namespace); none when the document is not a
/// stylesheet. Only the start of the document is read.
pub fn xslt_prefix(text: &str) -> Option<String> {
    let mut end = text.len().min(ROOT_LOOK);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = text.get(..end)?;
    // The root element: the first `<` that begins a name.
    let mut from = 0;
    let root = loop {
        let lt = from + head.get(from..)?.find('<')?;
        let rest = head.get(lt + 1..)?;
        if let Some(after) = rest.strip_prefix("!--") {
            from = lt + 4 + after.find("-->")? + 3;
        } else if rest.starts_with('?') {
            from = lt + 1 + rest.find("?>")? + 2;
        } else if rest.starts_with('!') {
            // A declaration, which may hold an internal subset in brackets.
            let mut depth = 0usize;
            let mut stop = None;
            for (i, c) in rest.char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => depth = depth.saturating_sub(1),
                    '>' if depth == 0 => {
                        stop = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            from = lt + 1 + stop? + 1;
        } else {
            break lt;
        }
    };
    let tag_end = head.get(root..)?.find('>').map_or(head.len(), |n| root + n);
    let tag = head.get(root..tag_end)?;
    let mut found = None;
    for (name, value) in attributes_of_tag(tag) {
        if value == XSLT_NS {
            if name == "xmlns" {
                found = Some(String::new());
                break;
            }
            if let Some(prefix) = name.strip_prefix("xmlns:") {
                found = Some(prefix.to_owned());
                break;
            }
        }
    }
    found
}

/// The attributes of one start tag as (name, value), read leniently.
fn attributes_of_tag(tag: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = tag.get(tag.find(char::is_whitespace).unwrap_or(tag.len())..).unwrap_or_default();
    while found.len() < 256 {
        rest = rest.trim_start();
        let Some(eq) = rest.find('=') else { break };
        let name = rest.get(..eq).unwrap_or_default().trim();
        let after = rest.get(eq + 1..).unwrap_or_default().trim_start();
        let Some(quote) = after.chars().next().filter(|c| matches!(c, '"' | '\'')) else { break };
        let Some(close) = after.get(1..).and_then(|r| r.find(quote)) else { break };
        found.push((name.to_owned(), after.get(1..1 + close).unwrap_or_default().to_owned()));
        rest = after.get(1 + close + 1..).unwrap_or_default();
    }
    found
}

/// `prefix:local` (or `local` when the namespace is the default one).
fn qualified(prefix: &str, local: &str) -> String {
    if prefix.is_empty() { local.to_owned() } else { format!("{prefix}:{local}") }
}

/// The end of a start tag whose name ends at `from`: where its `>` is, and whether it is `/>`.
fn tag_end(text: &str, from: usize) -> Option<(usize, bool)> {
    let mut quote: Option<char> = None;
    let mut previous = ' ';
    for (i, c) in text.get(from..)?.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some((from + i, previous == '/')),
            _ => {}
        }
        previous = c;
    }
    None
}

/// The position just after `pattern`, looking from `from`.
fn after(text: &str, from: usize, pattern: &str) -> Option<usize> {
    text.get(from..)?.find(pattern).map(|n| from + n + pattern.len())
}

/// The names of the elements open at `upto`, outermost first (those that were closed are gone). Read leniently: a stray end tag is
/// ignored, and the nesting followed is bounded.
fn open_elements(text: &str, upto: usize) -> Vec<String> {
    let mut stack: Vec<String> = Vec::new();
    let mut i = 0;
    while i < upto {
        let Some(rel) = text.get(i..upto).and_then(|s| s.find('<')) else { break };
        let lt = i + rel;
        let Some(rest) = text.get(lt + 1..) else { break };
        let next = if rest.starts_with("!--") {
            after(text, lt + 4, "-->")
        } else if rest.starts_with("![CDATA[") {
            after(text, lt + 9, "]]>")
        } else if rest.starts_with('?') {
            after(text, lt + 2, "?>")
        } else if rest.starts_with('!') {
            let mut depth = 0usize;
            let mut stop = None;
            for (n, c) in rest.char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => depth = depth.saturating_sub(1),
                    '>' if depth == 0 => {
                        stop = Some(lt + 1 + n + 1);
                        break;
                    }
                    _ => {}
                }
            }
            stop
        } else if let Some(end_tag) = rest.strip_prefix('/') {
            let name = end_tag.split(|c: char| !is_name_char(c)).next().unwrap_or_default();
            if let Some(at) = stack.iter().rposition(|n| n == name) {
                stack.truncate(at);
            }
            after(text, lt + 1, ">")
        } else {
            let name_len = rest.find(|c: char| !is_name_char(c)).unwrap_or(rest.len());
            let name = rest.get(..name_len).unwrap_or_default();
            match tag_end(text, lt + 1 + name_len) {
                Some((end, self_closing)) => {
                    if !self_closing && !name.is_empty() {
                        if stack.len() >= MAX_DEPTH {
                            return stack;
                        }
                        stack.push(name.to_owned());
                    }
                    Some(end + 1)
                }
                None => None,
            }
        };
        match next {
            Some(n) if n > i => i = n,
            _ => break,
        }
    }
    stack
}

/// The values of `attribute` on the `xsl:` elements named `locals` in the stylesheet, each word apart (a `mode` can list several); the
/// tag that starts at `skip` (the one being written) is left out.
fn defined_names(text: &str, prefix: &str, locals: &[&str], attribute: &str, skip: usize) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for local in locals {
        let needle = format!("<{}", qualified(prefix, local));
        for (at, _) in text.match_indices(&needle) {
            if at == skip {
                continue; // the tag being written
            }
            let after_name = at + needle.len();
            if !text.get(after_name..).is_some_and(|r| r.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/')) {
                continue;
            }
            let end = text.get(after_name..).and_then(|r| r.find('>')).map_or(text.len(), |n| after_name + n).min(after_name + MAX_TAG);
            let mut end = end;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let Some(tag) = text.get(at..end) else { continue };
            if let Some((_, value)) = attributes_of_tag(tag).into_iter().find(|(n, _)| n == attribute) {
                names.extend(value.split_whitespace().map(str::to_owned));
            }
            if names.len() > 4 * MAX_ITEMS {
                break;
            }
        }
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// The elements that take no instructions of their own, only the elements their table row lists for them.
fn takes_no_instructions(parent: &str) -> bool {
    matches!(
        parent,
        "stylesheet"
            | "transform"
            | "package"
            | "choose"
            | "accumulator"
            | "character-map"
            | "merge"
            | "merge-source"
            | "analyze-string"
            | "override"
    )
}

fn is_top(parent: &str) -> bool {
    matches!(parent, "stylesheet" | "transform" | "package")
}

/// Whether an element may stand in `parent` (the local name of an `xsl:` parent, none for a literal result element).
fn may_stand(element: &Element, parent: Option<&str>) -> bool {
    let instruction_place = parent.is_none_or(|p| !takes_no_instructions(p));
    match element.at {
        Top => parent.is_some_and(is_top),
        Instruction => instruction_place,
        Both => parent.is_some_and(is_top) || instruction_place,
        Only(list) => parent.is_some_and(|p| list.contains(&p)),
    }
}

fn element_item(prefix: &str, element: &Element) -> XsltItem {
    let name = qualified(prefix, element.name);
    let first = element.attributes.first().copied();
    let (insert, cursor) = match (element.empty, first) {
        (true, Some(a)) => (format!("<{name} {a}=\"\"/>"), name.len() + a.len() + 4),
        (true, None) => (format!("<{name}/>"), name.len() + 3),
        (false, Some(a)) => (format!("<{name} {a}=\"\"></{name}>"), name.len() + a.len() + 4),
        (false, None) => (format!("<{name}></{name}>"), name.len() + 2),
    };
    let detail = if matches!(element.at, Top) { "declaration".to_owned() } else { String::new() };
    XsltItem { label: name, insert, cursor, detail }
}

fn item(label: &str, detail: &str) -> XsltItem {
    XsltItem { label: label.to_owned(), insert: label.to_owned(), cursor: label.len(), detail: detail.to_owned() }
}

/// The element of the table that `name` (as written, with its prefix) is, when it is an `xsl:` element.
fn xsl_element<'a>(name: &'a str, prefix: &str) -> Option<(&'a str, Option<&'static Element>)> {
    let local = if prefix.is_empty() { (!name.contains(':')).then_some(name)? } else { name.strip_prefix(prefix)?.strip_prefix(':')? };
    Some((local, ELEMENTS.iter().find(|e| e.name == local)))
}

/// What to offer at `offset` of the XSLT stylesheet `text`; none when the document is not a stylesheet or the caret is not at an element
/// name, an attribute name, an attribute value or an XPath function name of one of its `xsl:` elements. `explicit` is a request for
/// completion (Edit ▸ Complete): functions are then offered before anything is typed.
pub fn complete(text: &str, offset: usize, explicit: bool) -> Option<XsltCompletion> {
    let prefix = xslt_prefix(text)?;
    let context = tag_context(text, offset)?;
    match context.place {
        Place::Name { typed, replace } => {
            let stack = open_elements(text, context.lt);
            let parent = stack.last()?;
            let parent_local = xsl_element(parent, &prefix).map(|(local, _)| local);
            let mut items: Vec<XsltItem> = ELEMENTS
                .iter()
                .filter(|e| may_stand(e, parent_local))
                .map(|e| element_item(&prefix, e))
                .filter(|i| i.label.starts_with(&typed))
                .collect();
            items.truncate(MAX_ITEMS);
            (!items.is_empty()).then_some(XsltCompletion { kind: XsltKind::Element, replace, items })
        }
        Place::Attribute { typed, replace, given } => {
            let (local, known) = xsl_element(&context.element, &prefix)?;
            let own: &[&str] = known.map_or(&[], |e| e.attributes);
            let root = is_top(local);
            let mut items = Vec::new();
            let extra: &[&str] = if root { ROOT_ATTRIBUTES } else { COMMON_ATTRIBUTES };
            if known.is_none() && !root {
                return None;
            }
            for name in own.iter().chain(extra.iter()) {
                if name.starts_with(&typed) && !given.iter().any(|g| g == name) && !items.iter().any(|i: &XsltItem| i.label == *name) {
                    let detail = if own.contains(name) { "" } else { "any xsl element" };
                    items.push(XsltItem {
                        label: (*name).to_owned(),
                        insert: format!("{name}=\"\""),
                        cursor: name.len() + 2,
                        detail: detail.to_owned(),
                    });
                }
            }
            items.truncate(MAX_ITEMS);
            (!items.is_empty()).then_some(XsltCompletion { kind: XsltKind::Attribute, replace, items })
        }
        Place::Value { attribute, typed, replace } => {
            let (local, _) = xsl_element(&context.element, &prefix)?;
            // The names the stylesheet defines.
            let names: Option<(Vec<String>, &[&str])> = match (local, attribute.as_str()) {
                ("call-template", "name") => Some((defined_names(text, &prefix, &["template"], "name", context.lt), &[])),
                ("apply-templates", "mode") => {
                    let mut all = defined_names(text, &prefix, &["template", "apply-templates"], "mode", context.lt);
                    all.extend(defined_names(text, &prefix, &["mode"], "name", context.lt));
                    Some((all, &["#current", "#default"]))
                }
                ("template", "mode") => {
                    let mut all = defined_names(text, &prefix, &["template", "apply-templates"], "mode", context.lt);
                    all.extend(defined_names(text, &prefix, &["mode"], "name", context.lt));
                    Some((all, &["#all", "#default"]))
                }
                (_, "use-attribute-sets") => Some((defined_names(text, &prefix, &["attribute-set"], "name", context.lt), &[])),
                _ => None,
            };
            if let Some((mut defined, specials)) = names {
                // (A list of names: the word at the caret is what is completed.)
                let word_start = typed.rfind(char::is_whitespace).map_or(0, |n| n + 1);
                let start = replace.start + word_start;
                let word = typed.get(word_start..).unwrap_or_default();
                let end =
                    offset + text.get(offset..replace.end).and_then(|r| r.find(char::is_whitespace)).unwrap_or(replace.end.saturating_sub(offset));
                defined.sort_unstable();
                defined.dedup();
                let mut items: Vec<XsltItem> =
                    specials.iter().map(|s| item(s, "")).chain(defined.iter().map(|n| item(n, "in this stylesheet"))).collect();
                items.retain(|i| i.label.starts_with(word));
                items.truncate(MAX_ITEMS);
                return (!items.is_empty()).then_some(XsltCompletion { kind: XsltKind::Value, replace: start..end, items });
            }
            let fixed = fixed_values(&attribute);
            if !fixed.is_empty() {
                let items: Vec<XsltItem> = fixed.iter().filter(|v| v.starts_with(&typed)).map(|v| item(v, "")).collect();
                return (!items.is_empty()).then_some(XsltCompletion { kind: XsltKind::Value, replace, items });
            }
            if !EXPRESSIONS.contains(&attribute.as_str()) {
                return None;
            }
            // The name of a function in an expression.
            let ident = typed.rsplit(|c: char| !is_name_char(c)).next().unwrap_or_default();
            let before = typed.get(..typed.len() - ident.len()).unwrap_or_default();
            if ident.contains(':')
                || before.ends_with(['$', '@', '/'])
                || ident.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.')
                || (ident.is_empty() && !explicit)
            {
                return None;
            }
            let start = offset - ident.len();
            let tail = text.get(offset..replace.end).map_or(0, |r| r.find(|c: char| !is_name_char(c)).unwrap_or(r.len()));
            let end = offset + tail;
            let has_paren = text.get(end..).is_some_and(|r| r.starts_with('('));
            let items: Vec<XsltItem> = FUNCTIONS
                .iter()
                .filter(|(name, _)| name.starts_with(ident))
                .take(MAX_ITEMS)
                .map(|(name, signature)| {
                    let (insert, cursor) = if has_paren {
                        ((*name).to_owned(), name.len())
                    } else if signature.ends_with("()") {
                        (format!("{name}()"), name.len() + 2)
                    } else {
                        (format!("{name}()"), name.len() + 1)
                    };
                    XsltItem { label: (*name).to_owned(), insert, cursor, detail: (*signature).to_owned() }
                })
                .collect();
            (!items.is_empty()).then_some(XsltCompletion { kind: XsltKind::Function, replace: start..end, items })
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const HEAD: &str = "<?xml version=\"1.0\"?>\n<xsl:stylesheet version=\"3.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">\n";

    fn at(body: &str) -> Option<XsltCompletion> {
        let marked = format!("{HEAD}{body}");
        let offset = marked.find('|').expect("a caret");
        complete(&marked.replacen('|', "", 1), offset, false)
    }

    fn labels(c: &XsltCompletion) -> Vec<&str> {
        c.items.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn a_stylesheet_is_known_by_the_prefix_the_namespace_is_bound_to() {
        assert_eq!(xslt_prefix(&format!("{HEAD}</xsl:stylesheet>")).as_deref(), Some("xsl"));
        assert_eq!(
            xslt_prefix("<!-- c --><!DOCTYPE x [<!ENTITY a \"<b>\">]><t:transform xmlns:t='http://www.w3.org/1999/XSL/Transform'/>").as_deref(),
            Some("t")
        );
        assert_eq!(xslt_prefix("<stylesheet xmlns=\"http://www.w3.org/1999/XSL/Transform\"/>").as_deref(), Some(""));
        assert_eq!(xslt_prefix("<root xmlns:xsl=\"urn:other\"/>"), None);
        assert_eq!(xslt_prefix("<a/>"), None);
        assert_eq!(xslt_prefix(""), None);
    }

    #[test]
    fn at_the_top_the_declarations_are_offered_and_in_a_template_the_instructions() {
        let c = at("  <xsl:te|").expect("completion");
        assert_eq!((c.kind, labels(&c)), (XsltKind::Element, vec!["xsl:template"]));
        let template = &c.items[0];
        assert_eq!((template.insert.as_str(), template.detail.as_str()), ("<xsl:template match=\"\"></xsl:template>", "declaration"));
        assert_eq!(template.cursor, "xsl:template match=\"".len() + 1, "the caret is inside the quotes");
        let c = at("  <|").expect("completion");
        assert!(
            labels(&c).contains(&"xsl:template") && labels(&c).contains(&"xsl:import") && !labels(&c).contains(&"xsl:value-of"),
            "{:?}",
            labels(&c)
        );
        let c = at("  <xsl:template match=\"/\">\n    <xsl:va|").expect("completion");
        assert_eq!(labels(&c), ["xsl:value-of", "xsl:variable"]);
        let value_of = &c.items[0];
        assert_eq!((value_of.insert.as_str(), value_of.cursor), ("<xsl:value-of select=\"\"/>", "xsl:value-of select=\"".len() + 1));
        // A closed template is no parent any more, and a literal result element takes instructions.
        assert_eq!(at("  <xsl:template match=\"/\"/>\n  <xsl:ap|"), None, "a closed template is no parent: no instruction at the top");
        let c = at("  <xsl:template match=\"/\"><html><body><xsl:fo|").expect("completion");
        assert_eq!(labels(&c), ["xsl:for-each", "xsl:for-each-group", "xsl:fork"]);
    }

    #[test]
    fn some_elements_stand_only_inside_one_parent() {
        let c = at("  <xsl:template match=\"/\"><xsl:choose><|").expect("completion");
        assert_eq!(labels(&c), ["xsl:otherwise", "xsl:when"]);
        let c = at("  <xsl:template match=\"/\"><xsl:for-each select=\"a\"><xsl:so|").expect("completion");
        assert_eq!(labels(&c), ["xsl:sort", "xsl:source-document"]);
        let c = at("  <xsl:template match=\"/\"><xsl:if test=\"a\"><xsl:so|").expect("completion");
        assert_eq!(labels(&c), ["xsl:source-document"], "sort is not offered in an if");
        let c = at("  <xsl:template match=\"/\"><xsl:call-template name=\"n\"><xsl:wi|").expect("completion");
        assert_eq!(labels(&c), ["xsl:with-param"]);
        // Comments, CDATA and processing instructions do not count as elements.
        let c = at("  <xsl:template match=\"/\"><!-- <xsl:if> --><![CDATA[<xsl:if>]]><?pi <xsl:if>?><xsl:choose><xsl:wh|").expect("completion");
        assert_eq!(labels(&c), ["xsl:when"]);
    }

    #[test]
    fn in_a_start_tag_the_attributes_of_the_element_are_offered_without_those_it_has() {
        let c = at("  <xsl:template ma|").expect("completion");
        assert_eq!((c.kind, labels(&c)), (XsltKind::Attribute, vec!["match"]));
        assert_eq!(c.items[0].insert, "match=\"\"");
        let c = at("  <xsl:template match=\"/\" |").expect("completion");
        assert!(labels(&c).contains(&"name") && labels(&c).contains(&"mode") && labels(&c).contains(&"use-when") && !labels(&c).contains(&"match"));
        assert_eq!(c.items.iter().find(|i| i.label == "use-when").map(|i| i.detail.as_str()), Some("any xsl element"));
        assert_eq!(labels(&at("<xsl:stylesheet ve|").expect("completion")), ["version"], "the attributes of the root");
        // A literal result element has no xsl attributes to offer.
        assert_eq!(at("  <xsl:template match=\"/\"><div cl|"), None);
    }

    #[test]
    fn inside_the_quotes_the_values_of_an_attribute_or_the_names_of_the_stylesheet_are_offered() {
        let c = at("  <xsl:sort order=\"d|\"/>").expect("completion");
        assert_eq!((c.kind, labels(&c)), (XsltKind::Value, vec!["descending"]));
        let c = at("  <xsl:output method=\"|\"/>").expect("completion");
        assert!(labels(&c).contains(&"xml") && labels(&c).contains(&"html"));
        let body = "  <xsl:template name=\"header\"/><xsl:template name=\"footer\" mode=\"print screen\"/>\n  <xsl:template match=\"/\"><xsl:call-template name=\"f|\"/></xsl:template>";
        let c = at(body).expect("completion");
        assert_eq!(labels(&c), ["footer"]);
        let c = at(&body.replace("name=\"f|\"", "name=\"|\"")).expect("completion");
        assert_eq!(labels(&c), ["footer", "header"]);
        let c = at("  <xsl:template match=\"a\" mode=\"print\"/><xsl:apply-templates mode=\"p|\"/>").expect("completion");
        assert_eq!(labels(&c), ["print"]);
        let c = at("  <xsl:template match=\"a\" mode=\"print\"/><xsl:apply-templates mode=\"|\"/>").expect("completion");
        assert_eq!(labels(&c), ["#current", "#default", "print"]);
        let c = at("  <xsl:attribute-set name=\"a\"/><xsl:attribute-set name=\"b\"/><xsl:copy use-attribute-sets=\"a b|\"/>").expect("completion");
        assert_eq!(labels(&c), ["b"], "the word at the caret is completed");
        assert_eq!(at("  <xsl:template match=\"|\"/>"), None, "a pattern has no list");
    }

    #[test]
    fn in_an_expression_function_names_are_offered_with_their_parentheses() {
        let c = at("  <xsl:template match=\"/\"><xsl:value-of select=\"string-j|\"/>").expect("completion");
        assert_eq!((c.kind, labels(&c)), (XsltKind::Function, vec!["string-join"]));
        assert_eq!((c.items[0].insert.as_str(), c.items[0].cursor), ("string-join()", 12), "the caret is inside when there are arguments");
        assert_eq!(c.items[0].detail, "string-join($input, $separator?)");
        let c = at("  <xsl:template match=\"/\"><xsl:if test=\"tr|\"/>").expect("completion");
        assert_eq!(labels(&c), ["trace", "transform", "translate", "true"]);
        assert_eq!(c.items.iter().find(|i| i.label == "true").map(|i| (i.insert.as_str(), i.cursor)), Some(("true()", 6)));
        // The rest of a name is replaced, and an opening parenthesis already there is kept.
        let c = at("  <xsl:template match=\"/\"><xsl:if test=\"cou|nt(a)\"/>").expect("completion");
        assert_eq!((labels(&c), c.items[0].insert.as_str()), (vec!["count"], "count"));
        // Not after `$`, `@`, `/`, in an axis, or in a plain name.
        for marked in ["test=\"$co|\"", "test=\"@co|\"", "test=\"a/co|\"", "test=\"child::co|\"", "test=\"1|\"", "test=\"|\""] {
            assert_eq!(at(&format!("  <xsl:template match=\"/\"><xsl:if {marked}/>")), None, "{marked}");
        }
        let marked = format!("{HEAD}  <xsl:template match=\"/\"><xsl:if test=\"|\"/>");
        let offset = marked.find('|').expect("caret");
        let explicit = complete(&marked.replacen('|', "", 1), offset, true).expect("completion");
        assert!(explicit.items.len() > 100, "an explicit request lists them all");
    }

    #[test]
    fn nothing_is_offered_outside_a_stylesheet_in_text_or_end_tags_and_nothing_panics() {
        assert_eq!(complete("<a><b|", 6, false), None);
        for marked in ["  <xsl:template match=\"/\">text|</xsl:template>", "  </xsl:|", "  <!-- <xsl:te| -->"] {
            assert_eq!(at(marked), None, "{marked}");
        }
        for text in [
            "",
            "<",
            "<x xmlns:x='http://www.w3.org/1999/XSL/Transform'><",
            "<x xmlns:x='http://www.w3.org/1999/XSL/Transform'><x:",
            "<x xmlns=\"http://www.w3.org/1999/XSL/Transform\"><é ü=\"",
            "<x xmlns:x='http://www.w3.org/1999/XSL/Transform'><x:sort order=\"\u{e9}",
        ] {
            for offset in 0..=text.len() + 1 {
                let _ = complete(text, offset, true);
                let _ = complete(text, offset, false);
            }
        }
        // The nesting followed is bounded.
        let deep = format!("{HEAD}{}<xsl:", "<a>".repeat(2000));
        let _ = complete(&deep, deep.len(), false);
    }
}
