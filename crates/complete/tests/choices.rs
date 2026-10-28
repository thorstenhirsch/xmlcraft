//! What a grammar allows at a place in a document, and the text to put for it.

use xmlcraft_complete::{Choices, choices_at, element_snippet};
use xmlcraft_dtd::{compile, parse_internal_subset};
use xmlcraft_grammar::Grammar;
use xmlcraft_syntax::parse;

fn grammar(dtd: &str) -> Grammar {
    let parsed = parse_internal_subset(dtd, 0);
    assert!(parsed.diagnostics.is_empty(), "{dtd}: {:?}", parsed.diagnostics);
    compile(&parsed.dtd, Some("r")).grammar
}

/// The choices at the place marked `|` in `text`.
fn at(g: &Grammar, marked: &str) -> Choices {
    let offset = marked.find('|').expect("a marker");
    let text = marked.replacen('|', "", 1);
    choices_at(&parse(&text).syntax(), offset, g)
}

fn names(c: &Choices) -> Vec<(String, bool)> {
    c.elements.iter().map(|e| (e.name.clone(), e.mandatory)).collect()
}

fn pair(n: &str, m: bool) -> (String, bool) {
    (n.to_owned(), m)
}

const MODEL: &str = "<!ELEMENT r (a, b?, c)><!ELEMENT a EMPTY><!ELEMENT b (#PCDATA)><!ELEMENT c ANY><!ELEMENT d EMPTY>";

#[test]
fn the_content_model_is_followed_through_the_children_before_the_position() {
    let g = grammar(MODEL);
    assert_eq!(names(&at(&g, "<r>|</r>")), [pair("a", true)]);
    assert_eq!(names(&at(&g, "<r><a/>|</r>")), [pair("b", false), pair("c", true)]);
    assert_eq!(names(&at(&g, "<r><a/><b/>|</r>")), [pair("c", true)]);
    assert_eq!(names(&at(&g, "<r><a/><c/>|</r>")), Vec::<(String, bool)>::new(), "the content is complete: nothing more may come");
    // Between children, only those before the position count.
    assert_eq!(names(&at(&g, "<r><a/>|<c/></r>")), [pair("b", false), pair("c", true)]);
    // An element that is not closed yet.
    assert_eq!(names(&at(&g, "<r><a/>|")), [pair("b", false), pair("c", true)]);
    // A child that does not fit ends the following.
    assert_eq!(names(&at(&g, "<r><c/><a/>|</r>")), [pair("a", true)], "the model is followed as far as it fits");
    assert_eq!(at(&g, "<r><a/>|</r>").parent.as_deref(), Some("r"));
}

#[test]
fn the_kinds_of_content_offer_what_they_allow() {
    let g =
        grammar("<!ELEMENT r (m, e, any)><!ELEMENT m (#PCDATA | x | y)*><!ELEMENT e EMPTY><!ELEMENT any ANY><!ELEMENT x EMPTY><!ELEMENT y EMPTY>");
    assert_eq!(names(&at(&g, "<r><m>|</m></r>")), [pair("x", false), pair("y", false)]);
    assert_eq!(names(&at(&g, "<r><m/><e>|</e></r>")), Vec::<(String, bool)>::new());
    let any: Vec<String> = at(&g, "<r><m/><e/><any>|</any></r>").elements.into_iter().map(|e| e.name).collect();
    assert_eq!(any, ["r", "m", "e", "any", "x", "y"]);
    let choice = at(&g, "<r><m>|</m></r>");
    assert!(choice.elements.iter().all(|e| e.empty), "x and y are EMPTY");
    // An element that is not declared, and a position nothing is offered at.
    assert!(at(&g, "<r><nope>|</nope></r>").elements.is_empty());
    assert!(at(&g, "<r></|r>").elements.is_empty() && at(&g, "<r></|r>").element.is_none());
}

#[test]
fn before_the_root_element_the_root_is_offered() {
    let g = grammar(MODEL);
    let c = at(&g, "|<r/>");
    assert_eq!(names(&c), [pair("r", false)]);
    assert!(at(&g, "<r/>|").elements.is_empty(), "after the root element there is nothing to write");
    assert_eq!(names(&at(&g, "|")), [pair("r", false)], "in an empty document too");
}

#[test]
fn the_attributes_of_the_element_the_position_is_in_are_offered_with_what_is_given() {
    let g = grammar("<!ELEMENT r ANY><!ATTLIST r id ID #REQUIRED kind (x|y) 'x' note CDATA #IMPLIED>");
    let c = at(&g, "<r id=\"1\" |>text</r>");
    assert_eq!(c.element.as_deref(), Some("r"));
    assert!(c.in_start_tag && c.parent.is_none() && c.elements.is_empty(), "inside a tag no elements are offered");
    let by_name = |n: &str| c.attributes.iter().find(|a| a.name == n).cloned().expect("offered");
    assert_eq!((by_name("id").mandatory, by_name("id").used), (true, true));
    assert_eq!((by_name("kind").mandatory, by_name("kind").used, by_name("kind").values), (false, false, vec!["x".to_owned(), "y".to_owned()]));
    assert!(!by_name("note").used);
    // New attributes go after the last one, before `>`.
    let text = "<r id=\"1\" >text</r>";
    assert_eq!(c.attribute_at, Some(text.find("\"1\"").expect("there") + 3));
    let bare = at(&g, "<r|>x</r>");
    assert_eq!(bare.attribute_at, Some(2));
    let selfclosing = at(&g, "<r |/>");
    assert_eq!((selfclosing.in_start_tag, selfclosing.attribute_at), (true, Some(2)));
    // In the content of the element, its attributes are offered too (the helper adds to the start tag).
    let in_content = at(&g, "<r id=\"1\">te|xt</r>");
    assert_eq!((in_content.element.as_deref(), in_content.parent.as_deref(), in_content.attributes.len()), (Some("r"), Some("r"), 3));
    // An element with no declaration has none.
    assert!(at(&g, "<z |/>").attributes.is_empty());
}

#[test]
fn the_entities_that_can_be_referred_to_are_the_parsed_ones() {
    let g = grammar("<!ELEMENT r ANY><!ENTITY a 'text'><!ENTITY b SYSTEM 'b.xml'><!NOTATION n SYSTEM 'n'><!ENTITY img SYSTEM 'i.gif' NDATA n>");
    let c = at(&g, "<r>|</r>");
    assert_eq!(c.entities.iter().map(|e| (e.name.as_str(), e.value.as_deref())).collect::<Vec<_>>(), [("a", Some("text")), ("b", None)]);
}

#[test]
fn the_text_for_an_element_has_what_its_declaration_requires() {
    let g = grammar(
        "<!ELEMENT r (a, b?, c)><!ELEMENT a EMPTY><!ATTLIST a id ID #REQUIRED kind (p|q) #REQUIRED opt CDATA #IMPLIED>\
         <!ELEMENT b (#PCDATA)><!ELEMENT c (d)><!ELEMENT d (#PCDATA)><!ATTLIST d x CDATA #REQUIRED>",
    );
    let s = element_snippet(&g, "r", true);
    assert_eq!(s.text, "<r><a id=\"\" kind=\"p\"/><c><d x=\"\"></d></c></r>");
    assert_eq!(&s.text[..s.caret], "<r>", "the caret is in the content of the element: it has no attribute of its own");
    // Without the required content: just the element, and its required attributes.
    let a = element_snippet(&g, "a", false);
    assert_eq!(a.text, "<a id=\"\" kind=\"p\"/>");
    assert_eq!(&a.text[..a.caret], "<a id=\"", "the caret is in the first value to fill in");
    let b = element_snippet(&g, "b", false);
    assert_eq!((b.text.as_str(), &b.text[..b.caret]), ("<b></b>", "<b>"));
    let c = element_snippet(&g, "c", false);
    assert_eq!(c.text, "<c></c>");
    // A name with no declaration is written as an empty pair.
    assert_eq!(element_snippet(&g, "zzz", true).text, "<zzz></zzz>");
}

#[test]
fn nested_requirements_are_bounded() {
    // Each element requires the next, round and round.
    let g = grammar("<!ELEMENT r (a)><!ELEMENT a (b)><!ELEMENT b (a)>");
    let s = element_snippet(&g, "a", true);
    assert!(s.text.len() < xmlcraft_complete::MAX_SNIPPET_BYTES, "{}", s.text.len());
    assert!(s.text.starts_with("<a><b><a>") && s.text.ends_with("</a></b></a>"), "{}", s.text);
    // Wide ones are cut off too.
    let wide = format!(
        "<!ELEMENT r ({})>{}",
        (0..3000).map(|i| format!("e{i}")).collect::<Vec<_>>().join(", "),
        (0..3000).map(|i| format!("<!ELEMENT e{i} EMPTY>")).collect::<String>()
    );
    let s = element_snippet(&grammar(&wide), "r", true);
    assert!(s.text.len() > 10_000 && s.text.len() <= xmlcraft_complete::MAX_SNIPPET_BYTES + 64, "{}", s.text.len());
}

#[test]
fn any_position_in_any_text_gives_an_answer_without_a_panic() {
    let g = grammar(MODEL);
    for text in [
        "",
        "<",
        "<r",
        "<r>",
        "<r><",
        "<r><a",
        "<r></",
        "<r><a/></r>",
        "<!DOCTYPE r []><r/>",
        "<?pi?>",
        "é<r>é</r>",
        "<r a='>",
        "<r><![CDATA[<r>]]></r>",
    ] {
        let tree = parse(text).syntax();
        for offset in 0..=text.len() {
            if text.is_char_boundary(offset) {
                let _ = choices_at(&tree, offset, &g);
            }
        }
    }
}
