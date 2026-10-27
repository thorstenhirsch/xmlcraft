//! The parser through its public API: the tree is lossless, and each kind of problem is reported where it is.

use proptest::prelude::*;
use xmlcraft_syntax::{Code, Limits, Severity, SyntaxKind, parse, parse_with};

/// The problems of `src` as (code, the text they point at).
fn problems(src: &str) -> Vec<(Code, String)> {
    let parsed = parse(src);
    assert_eq!(parsed.syntax().text().to_string(), src, "the tree must be lossless");
    parsed.diagnostics().iter().map(|d| (d.code, src[d.span.clone()].to_owned())).collect()
}

fn p(code: Code, text: &str) -> (Code, String) {
    (code, text.to_owned())
}

#[test]
fn well_formed_documents_have_no_problems() {
    for src in [
        "<a/>",
        "<a></a>",
        "  \n<a/>\n  ",
        "\u{FEFF}<a/>",
        "\u{FEFF}\n<a/>",
        "\u{FEFF} \t<a/>",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<a b=\"1\" c='2'>t<!--c--><![CDATA[x<y]]><?pi data?>&amp;&#65;&#x41;</a>\n<!-- after -->\n",
        "<a xmlns=\"urn:x\" xmlns:p=\"urn:p\" p:b=\"1\"><p:c/></a>",
        "<a xmlns=\"urn:x\"><b xmlns=\"\"/></a>",
        "<a xml:lang=\"en\" xmlns:xml=\"http://www.w3.org/XML/1998/namespace\"/>",
        "<!DOCTYPE a [<!ENTITY e \"x\"><!ELEMENT a (#PCDATA)><!-- c --><?p q?>]><a>&e;</a>",
        "<!DOCTYPE a SYSTEM \"a.dtd\"><a/>",
        "<!DOCTYPE a PUBLIC \"-//X//Y\" \"a.dtd\"><a/>",
        "<日本 é=\"1\">😀</日本>",
        "<a b = \"x\"\n   c\n = 'y' />",
        "<?xml version=\"1.0\"?><?xml-stylesheet href=\"a.xsl\"?><a/>",
    ] {
        assert_eq!(problems(src), vec![], "{src:?}");
    }
}

#[test]
fn the_shape_of_the_tree() {
    let src = "<?xml version=\"1.0\"?><a x=\"1\">t<b/><!--c--></a>";
    let root = parse(src).syntax();
    let nodes: Vec<SyntaxKind> = root.descendants().map(|n| n.kind()).collect();
    use SyntaxKind::*;
    assert_eq!(nodes, [Document, XmlDecl, Element, StartTag, Attribute, AttrValue, Element, StartTag, Comment, EndTag]);
    let element = root.children().find(|n| n.kind() == Element).expect("an element");
    assert_eq!(element.text().to_string(), "<a x=\"1\">t<b/><!--c--></a>");
    let attr = element.descendants().find(|n| n.kind() == Attribute).expect("an attribute");
    assert_eq!(attr.text().to_string(), "x=\"1\"");
}

#[test]
fn element_structure_problems() {
    assert_eq!(problems("<a></b>"), [p(Code::MismatchedEndTag, "</b")]);
    assert_eq!(problems("<a><b></a>"), [p(Code::UnclosedElement, "b")]);
    assert_eq!(problems("<a><b></c></a>"), [p(Code::MismatchedEndTag, "</c")], "a typo closes the innermost element");
    assert_eq!(problems("<a><b></a></b>"), [p(Code::UnclosedElement, "b"), p(Code::UnmatchedEndTag, "</b")]);
    assert_eq!(problems("<a>"), [p(Code::UnclosedElement, "a")]);
    assert_eq!(problems("</a>"), [p(Code::UnmatchedEndTag, "</a"), p(Code::NoRootElement, "")]);
    assert_eq!(problems("<a></>"), [p(Code::MissingEndTagName, "</")]);
    assert_eq!(problems("<a></a b>"), [p(Code::UnexpectedCharacter, "b")]);
    assert_eq!(problems("<a></a"), [p(Code::UnterminatedEndTag, "</")]);
    assert_eq!(problems("<a><b></a><"), [p(Code::UnclosedElement, "b"), p(Code::BareLessThan, "<")]);
    assert_eq!(problems("<a/><b/>"), [p(Code::MultipleRoots, "<")]);
    assert_eq!(problems(""), [p(Code::NoRootElement, "")]);
    assert_eq!(problems("<!-- only a comment -->"), [p(Code::NoRootElement, "")]);
    assert_eq!(problems("text<a/>"), [p(Code::ContentOutsideRoot, "text")]);
    assert_eq!(problems("<a/>text&amp;more"), [p(Code::ContentOutsideRoot, "text&amp;more")]);
    assert_eq!(problems("<a/><![CDATA[x]]>"), [p(Code::ContentOutsideRoot, "<![CDATA[")]);
}

#[test]
fn tag_and_attribute_problems() {
    assert_eq!(problems("<a b=\"1\" b=\"2\"/>"), [p(Code::DuplicateAttribute, "b")]);
    assert_eq!(problems("<a b/>"), [p(Code::AttrWithoutValue, "b")]);
    assert_eq!(problems("<a b=/>"), [p(Code::MissingAttrValue, "b")]);
    assert_eq!(problems("<a b=c/>"), [p(Code::UnquotedAttrValue, "c")]);
    assert_eq!(problems("<a =\"1\"/>"), [p(Code::UnexpectedCharacter, "="), p(Code::MissingAttrName, "\"")]);
    assert_eq!(problems("<a 1b=\"x\"/>"), [p(Code::InvalidName, "1b")]);
    assert_eq!(problems("<a b=\"x/>"), [p(Code::UnterminatedAttrValue, "\""), p(Code::UnterminatedTag, "a"), p(Code::UnclosedElement, "a")]);
    assert_eq!(problems("<a href=\"x <b/></a>"), [p(Code::UnterminatedAttrValue, "\""), p(Code::UnterminatedTag, "a")]);
    assert_eq!(problems("<a <b/></a>"), [p(Code::UnterminatedTag, "a")]);
    assert_eq!(problems("<a b=\"1\" c></a>"), [p(Code::AttrWithoutValue, "c")]);
    assert_eq!(problems("<a b=\"&\"/>"), [p(Code::BareAmpersand, "&")]);
}

#[test]
fn whitespace_before_an_equals_sign_has_no_limit() {
    // An attribute and the `=` of its value, or of an XML declaration field, may be any number of lines apart.
    let blank = "\n".repeat(500);
    assert_eq!(problems(&format!("<a b{blank}= \"1\"/>")), vec![]);
    assert_eq!(problems(&format!("<a b {blank} =\n{blank}'1' c=\"2\"/>")), vec![]);
    assert_eq!(problems(&format!("<?xml version{blank}=\"1.0\"?><a/>")), vec![]);
    assert_eq!(problems(&format!("<a b{blank}/>")), [p(Code::AttrWithoutValue, "b")]);
}

#[test]
fn character_data_problems() {
    assert_eq!(problems("<a>&nbsp;</a>"), [p(Code::UndeclaredEntity, "&nbsp;")]);
    assert_eq!(problems("<a b=\"&nbsp;\"/>"), [p(Code::UndeclaredEntity, "&nbsp;")]);
    assert_eq!(problems("<!DOCTYPE a><a>&nbsp;</a>"), vec![], "with a DOCTYPE, entities are the DTD's business");
    assert_eq!(problems("<a>&lt;&gt;&amp;&quot;&apos;</a>"), vec![]);
    assert_eq!(problems("<a>&</a>"), [p(Code::BareAmpersand, "&")]);
    assert_eq!(problems("<a>a < b</a>"), [p(Code::BareLessThan, "<")]);
    assert_eq!(problems("<a>]]></a>"), [p(Code::CdataEndInContent, "]]>")]);
    for bad in ["&#0;", "&#xD800;", "&#x110000;", "&#1;", "&#xFFFE;", "&#99999999999;"] {
        assert_eq!(problems(&format!("<a>{bad}</a>")), [p(Code::InvalidCharRef, bad)], "{bad}");
    }
    assert_eq!(problems("<a>\u{1}</a>"), [p(Code::InvalidChar, "\u{1}")]);
    assert_eq!(problems("<a b=\"\u{B}\"/>"), [p(Code::InvalidChar, "\u{B}")]);
    assert_eq!(problems("<a><!-- \u{8} --></a>"), [p(Code::InvalidChar, "\u{8}")]);
    assert_eq!(problems("<a>\u{FFFE}</a>"), [p(Code::InvalidChar, "\u{FFFE}")]);
}

#[test]
fn comment_cdata_and_processing_instruction_problems() {
    assert_eq!(problems("<!-- a -- b --><a/>"), [p(Code::DoubleHyphenInComment, "--")]);
    assert_eq!(problems("<!-- a ---><a/>"), [p(Code::DoubleHyphenInComment, "--")]);
    assert_eq!(problems("<!-- open"), [p(Code::UnterminatedComment, "<!--"), p(Code::NoRootElement, "")]);
    assert_eq!(problems("<a><![CDATA[x</a>"), [p(Code::UnterminatedCdata, "<![CDATA["), p(Code::UnclosedElement, "a")]);
    assert_eq!(problems("<a/><?pi x"), [p(Code::UnterminatedPi, "<?")]);
    assert_eq!(problems("<a/><??>"), [p(Code::MissingPiTarget, "<?")]);
    assert_eq!(problems("<?XML x?><a/>"), [p(Code::ReservedPiTarget, "XML")]);
    assert_eq!(problems("<a><?xMl x?></a>"), [p(Code::ReservedPiTarget, "xMl")]);
}

#[test]
fn xml_declaration_problems() {
    assert_eq!(problems("<a/><?xml version=\"1.0\"?>"), [p(Code::MisplacedXmlDecl, "<?xml")]);
    assert_eq!(problems(" <?xml version=\"1.0\"?><a/>"), [p(Code::MisplacedXmlDecl, "<?xml")]);
    assert_eq!(problems("<?xml?><a/>"), [p(Code::MissingVersion, "xml")]);
    assert_eq!(problems("<?xml version=\"2.0\"?><a/>"), [p(Code::BadVersion, "2.0")]);
    assert_eq!(problems("<?xml version=\"1.1\"?><a/>"), [p(Code::UnsupportedVersion, "1.1")]);
    assert!(!parse("<?xml version=\"1.1\"?><a/>").has_errors(), "a warning is not an error");
    assert_eq!(problems("<?xml version=\"1.0\" standalone=\"maybe\"?><a/>"), [p(Code::BadStandalone, "maybe")]);
    assert_eq!(problems("<?xml encoding=\"UTF-8\" version=\"1.0\"?><a/>"), [p(Code::PseudoAttrOrder, "version")]);
    assert_eq!(problems("<?xml version=\"1.0\" encoding=\"9x\"?><a/>"), [p(Code::BadEncodingName, "9x")]);
    assert_eq!(problems("<?xml version=\"1.0\" foo=\"x\"?><a/>"), [p(Code::UnknownPseudoAttr, "foo")]);
    assert_eq!(problems("<?xml version=\"1.0\" version=\"1.0\"?><a/>"), [p(Code::DuplicatePseudoAttr, "version")]);
    assert_eq!(problems("<?xml version=\"1.0\"<a/>"), [p(Code::UnterminatedXmlDecl, "<?")]);
    assert_eq!(problems("<?xml version=\"1.0?><a/>")[0], p(Code::UnterminatedLiteral, "\""));
}

#[test]
fn doctype_problems() {
    assert_eq!(problems("<a/><!DOCTYPE a>"), [p(Code::MisplacedDoctype, "<!DOCTYPE")]);
    assert_eq!(problems("<a><!DOCTYPE a></a>"), [p(Code::MisplacedDoctype, "<!DOCTYPE")]);
    assert_eq!(problems("<!DOCTYPE a><!DOCTYPE a><a/>"), [p(Code::MultipleDoctype, "<!DOCTYPE")]);
    assert_eq!(problems("<!DOCTYPE><a/>"), [p(Code::MissingDoctypeName, "<!DOCTYPE")]);
    assert_eq!(problems("<!DOCTYPE a SYSTEM><a/>"), [p(Code::BadDoctype, "SYSTEM")]);
    assert_eq!(problems("<!DOCTYPE a PUBLIC \"x\"><a/>"), [p(Code::BadDoctype, "PUBLIC")]);
    assert_eq!(problems("<!DOCTYPE a \"x\"><a/>"), [p(Code::BadDoctype, "<!DOCTYPE")]);
    assert_eq!(problems("<!DOCTYPE a FOO \"x\"><a/>"), [p(Code::BadDoctype, "FOO")]);
    assert!(problems("<!DOCTYPE a [<!ELEMENT a EMPTY><a/>").iter().any(|(c, _)| *c == Code::UnterminatedDoctype));
    assert_eq!(problems("<!DOCTYPE a [<!ELEMENT a\n<!ELEMENT b EMPTY>]><a/>"), [p(Code::UnterminatedDecl, "<!ELEMENT")]);
}

#[test]
fn namespace_problems() {
    assert_eq!(problems("<p:a/>"), [p(Code::UnboundPrefix, "p")]);
    assert_eq!(problems("<a p:b=\"1\"/>"), [p(Code::UnboundPrefix, "p")]);
    assert_eq!(problems("<a><b xmlns:p=\"urn:x\"/><p:c/></a>"), [p(Code::UnboundPrefix, "p")], "a declaration ends with its element");
    assert_eq!(problems("<a xmlns:p=\"urn:x\"><p:b/></a>"), vec![]);
    assert_eq!(problems("<a xmlns:p=\"\"/>"), [p(Code::EmptyNamespaceName, "xmlns:p")]);
    assert_eq!(problems("<a xmlns:xmlns=\"urn:x\"/>"), [p(Code::ReservedPrefix, "xmlns:xmlns")]);
    assert_eq!(problems("<a xmlns:xml=\"urn:x\"/>"), [p(Code::ReservedPrefix, "xmlns:xml")]);
    assert_eq!(problems("<a xmlns:p=\"http://www.w3.org/XML/1998/namespace\"/>"), [p(Code::ReservedNamespace, "xmlns:p")]);
    assert_eq!(problems("<a xmlns=\"http://www.w3.org/2000/xmlns/\"/>"), [p(Code::ReservedNamespace, "xmlns")]);
    assert_eq!(problems("<a xmlns:p=\"urn:x\" xmlns:q=\"urn:x\" p:b=\"1\" q:b=\"2\"/>"), [p(Code::DuplicateExpandedAttribute, "q:b")]);
    assert_eq!(problems("<a xmlns:p=\"urn:x\" xmlns:q=\"urn:y\" p:b=\"1\" q:b=\"2\"/>"), vec![]);
    assert_eq!(problems("<a:b:c/>"), [p(Code::InvalidQName, "a:b:c")]);
    assert_eq!(problems("<:a/>"), [p(Code::InvalidQName, ":a")]);
    assert_eq!(problems("<a b:=\"1\"/>"), [p(Code::InvalidQName, "b:")]);
    // Both parts of a qualified name must be NCNames, and so must the prefix of a declaration, whether or not it has a value.
    assert_eq!(problems("<a xmlns:p=\"urn:x\"><p:1/></a>"), [p(Code::InvalidQName, "p:1")]);
    assert_eq!(problems("<a xmlns:p=\"urn:x\" p:-b=\"1\"/>"), [p(Code::InvalidQName, "p:-b")]);
    assert_eq!(problems("<a xmlns:1p=\"urn:x\"/>"), [p(Code::InvalidQName, "xmlns:1p")]);
    assert_eq!(problems("<a xmlns:p:q=\"urn:x\"/>"), [p(Code::InvalidQName, "xmlns:p:q")]);
    assert_eq!(problems("<a xmlns:=\"urn:x\"/>"), [p(Code::InvalidQName, "xmlns:")]);
    assert_eq!(problems("<a xmlns:1p/>"), [p(Code::AttrWithoutValue, "xmlns:1p"), p(Code::InvalidQName, "xmlns:1p")]);
    assert_eq!(
        problems("<a xmlns:p:q=\"urn:x\"><p:q/></a>"),
        [p(Code::InvalidQName, "xmlns:p:q"), p(Code::UnboundPrefix, "p")],
        "a refused declaration binds nothing"
    );
    assert_eq!(
        problems("<a xmlns:p=\"a&amp;b\" xmlns:q=\"a&#38;b\" p:x=\"1\" q:x=\"2\"/>"),
        [p(Code::DuplicateExpandedAttribute, "q:x")],
        "references in a URI are resolved"
    );
}

#[test]
fn problems_have_severities_and_messages() {
    let parsed = parse("<?xml version=\"1.1\"?><a></b>");
    let texts: Vec<_> = parsed.diagnostics().iter().map(|d| (d.severity(), d.message())).collect();
    assert_eq!(
        texts,
        [
            (Severity::Warning, "The document is checked as XML 1.0; version '1.1' is not fully supported.".to_owned()),
            (Severity::Error, "The end tag </b> does not match the start tag <a>.".to_owned()),
        ]
    );
    assert!(parsed.has_errors());
}

#[test]
fn the_limits_hold() {
    let small = Limits { max_bytes: 2, ..Limits::default() };
    let parsed = parse_with("<a/>", &small);
    assert_eq!(parsed.diagnostics().iter().map(|d| d.code).collect::<Vec<_>>(), [Code::TooLarge]);
    assert_eq!(parsed.syntax().kind(), SyntaxKind::Document);

    // A million unclosed elements: the problems are capped, the tree is still lossless, and nothing recurses.
    let deep = "<a>".repeat(300_000);
    let parsed = parse(&deep);
    assert_eq!(parsed.syntax().text().to_string(), deep);
    let limit = Limits::default().max_diagnostics;
    assert_eq!(parsed.diagnostics().len(), limit + 1);
    assert_eq!(parsed.diagnostics().last().map(|d| d.code), Some(Code::TooManyDiagnostics));
    assert!(parsed.diagnostics().iter().any(|d| d.code == Code::NestingTooDeep));

    // Properly nested far deeper than the tree nests: still well-formed, and matched.
    let nested = format!("{}{}", "<a>".repeat(100_000), "</a>".repeat(100_000));
    let parsed = parse(&nested);
    assert_eq!(parsed.syntax().text().to_string(), nested);
    assert_eq!(parsed.diagnostics().iter().map(|d| d.code).collect::<Vec<_>>(), [Code::NestingTooDeep]);
    assert!(!parsed.has_errors());

    // End tags that match nothing, against a deep stack: each takes the innermost element, with no search of the stack (this was
    // quadratic, minutes for this input).
    let unmatched = format!("{}{}", "<a>".repeat(200_000), "</b>".repeat(200_000));
    let started = std::time::Instant::now();
    let parsed = parse(&unmatched);
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
    assert_eq!(parsed.syntax().text().to_string(), unmatched);
    assert!(parsed.diagnostics().iter().any(|d| d.code == Code::MismatchedEndTag));

    // A mismatch past the nesting limit is still found.
    let mismatched = format!("{}</b>{}", "<a>".repeat(5_000), "</a>".repeat(4_999));
    assert!(parse(&mismatched).diagnostics().iter().any(|d| d.code == Code::MismatchedEndTag));
}

/// Pieces that make interesting markup when glued together at random.
const PIECES: &[&str] = &[
    "<",
    "</",
    ">",
    "/>",
    "/",
    "\"",
    "'",
    "=",
    "&amp;",
    "&#65;",
    "&",
    ";",
    "<!--",
    "-->",
    "--",
    "<![CDATA[",
    "]]>",
    "<?xml",
    "<?",
    "?>",
    "<!DOCTYPE",
    "[",
    "]",
    "<!ELEMENT",
    "<!ENTITY",
    "%e;",
    " ",
    "\n",
    "\r\n",
    "a",
    "b",
    "p:a",
    "xmlns:p",
    "xmlns",
    "\"urn:x\"",
    "é",
    "日",
    "😀",
    "\u{1}",
    "version",
    "1.0",
];

fn markup() -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::sample::select(PIECES), 0..80).prop_map(|v| v.concat())
}

/// A well-formed element with attributes, text, references, comments, CDATA and processing instructions.
fn element() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        "[a-z é日😀\n]{0,8}".boxed(),
        Just("<!-- c -->".to_owned()).boxed(),
        Just("<![CDATA[x<y&]]>".to_owned()).boxed(),
        Just("<?pi data?>".to_owned()).boxed(),
        Just("&amp;&#x41;&lt;".to_owned()).boxed(),
    ];
    let content = leaf.prop_recursive(5, 80, 6, |inner| {
        let names = proptest::sample::select(&["a", "b", "p:c", "d-e", "日本"][..]);
        let attr_names = proptest::sample::subsequence(&["id", "x", "y", "p:z", "xml:lang"][..], 0..4);
        (names, attr_names, proptest::collection::vec(inner, 0..4), any::<bool>()).prop_map(|(name, attrs, kids, short)| {
            let attrs: String = attrs.iter().map(|a| format!(" {a}=\"v&amp;'\"")).collect();
            if kids.is_empty() && short { format!("<{name}{attrs}/>") } else { format!("<{name}{attrs}>{}</{name}>", kids.concat()) }
        })
    });
    content.prop_map(|body| format!("<r xmlns:p=\"urn:p\">{body}</r>"))
}

proptest! {
    /// Whatever the text, the tree is its text, and every problem points inside it at character boundaries.
    #[test]
    fn the_tree_is_lossless_and_problems_point_inside(src in prop_oneof![markup(), "\\PC{0,200}".boxed()]) {
        let parsed = parse(&src);
        prop_assert_eq!(parsed.syntax().text().to_string(), src.clone());
        for d in parsed.diagnostics() {
            prop_assert!(d.span.start <= d.span.end && d.span.end <= src.len(), "{:?}", d);
            prop_assert!(src.is_char_boundary(d.span.start) && src.is_char_boundary(d.span.end), "{:?}", d);
            prop_assert!(!d.message().is_empty());
        }
    }

    /// Documents built to be well-formed produce no problems at all.
    #[test]
    fn well_formed_documents_are_accepted(body in element(), prolog in prop::bool::ANY, epilog in prop::bool::ANY) {
        let doc = format!(
            "{}{}{}",
            if prolog { "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- start -->\n" } else { "" },
            body,
            if epilog { "\n<!-- end -->\n<?done?>\n" } else { "" },
        );
        let parsed = parse(&doc);
        prop_assert_eq!(parsed.diagnostics().to_vec(), vec![], "{}", doc);
        prop_assert_eq!(parsed.syntax().text().to_string(), doc);
    }

    /// Cutting a well-formed document anywhere gives a tree and problems, never a panic, and a prefix of a document with an
    /// unclosed element is reported.
    #[test]
    fn a_truncated_document_is_reported(body in element(), cut in 0usize..400) {
        let mut at = cut.min(body.len());
        while !body.is_char_boundary(at) {
            at -= 1;
        }
        let truncated = &body[..at];
        let parsed = parse(truncated);
        prop_assert_eq!(parsed.syntax().text().to_string(), truncated);
        if at < body.len() && !truncated.ends_with("</r>") {
            prop_assert!(parsed.has_errors(), "{}", truncated);
        }
    }
}
