//! Reading HTML: the structure browsers build, what is wrong with the markup, and that nothing runs away.

use proptest::prelude::*;
use xmlcraft_html::{Close, Document, MAX_DEPTH, parse};

fn codes(doc: &Document) -> Vec<&'static str> {
    doc.diagnostics.iter().map(|d| d.code.key()).collect()
}

fn names(doc: &Document) -> Vec<(&str, usize)> {
    doc.elements.iter().map(|e| (e.name.as_str(), e.depth)).collect()
}

#[test]
fn a_whole_document_has_its_elements_with_their_places_and_no_problems() {
    let text = "<!DOCTYPE html>\n<html lang=\"en\">\n<head><meta charset=utf-8><title>T</title></head>\n<body>\n<h1 id=top>Hi</h1>\n<p>One <b>two</b></p>\n</body>\n</html>\n";
    let doc = parse(text);
    assert_eq!(codes(&doc), Vec::<&str>::new(), "{:?}", doc.diagnostics);
    assert_eq!(doc.doctype.clone().map(|r| &text[r]), Some("<!DOCTYPE html>"));
    assert_eq!(names(&doc), [("html", 0), ("head", 1), ("meta", 2), ("title", 2), ("body", 1), ("h1", 2), ("p", 2), ("b", 3)]);
    let h1 = doc.elements.iter().find(|e| e.name == "h1").expect("h1");
    assert_eq!(&text[h1.span.clone()], "<h1 id=top>Hi</h1>");
    assert_eq!(&text[h1.name_span.clone()], "h1");
    assert_eq!(h1.attribute("id").and_then(|a| a.value.as_deref()), Some("top"));
    assert_eq!(doc.elements.iter().find(|e| e.name == "html").and_then(|e| e.attribute("lang")).and_then(|a| a.value.clone()), Some("en".to_owned()));
}

#[test]
fn void_elements_have_no_end_tag_and_one_that_is_written_is_a_problem() {
    let doc = parse("<p>a<br>b<img src=x alt=\"\"><input disabled></p>");
    assert_eq!(codes(&doc), Vec::<&str>::new(), "{:?}", doc.diagnostics);
    assert!(doc.elements.iter().filter(|e| matches!(e.name.as_str(), "br" | "img" | "input")).all(|e| e.close == Close::Void && e.depth == 1));
    let doc = parse("<p>a</br>b</p>");
    assert_eq!(codes(&doc), ["html.void-end-tag"]);
}

#[test]
fn end_tags_that_html_lets_a_person_leave_out_are_implied() {
    let doc = parse("<ul><li>a<li>b<li><p>c<p>d</ul>");
    assert_eq!(codes(&doc), Vec::<&str>::new(), "{:?}", doc.diagnostics);
    assert_eq!(doc.elements.iter().filter(|e| e.name == "li" && e.close == Close::Implied).count(), 3);
    assert_eq!(names(&doc), [("ul", 0), ("li", 1), ("li", 1), ("li", 1), ("p", 2), ("p", 2)], "items are siblings, not nested in each other");
    let doc = parse("<table><tr><td>1<td>2<tr><td>3</table>");
    assert_eq!(codes(&doc), Vec::<&str>::new(), "{:?}", doc.diagnostics);
    assert_eq!(doc.elements.iter().filter(|e| e.name == "td").map(|e| e.depth).collect::<Vec<_>>(), [2, 2, 2], "cells are not nested in each other");
    let doc = parse("<select><option>a<option>b</select><dl><dt>t<dd>d<dt>u</dl>");
    assert_eq!(codes(&doc), Vec::<&str>::new(), "{:?}", doc.diagnostics);
    // A block ends an open paragraph.
    let doc = parse("<p>text<div>block</div>");
    assert_eq!(names(&doc), [("p", 0), ("div", 0)]);
}

#[test]
fn elements_that_are_not_closed_are_said_where_they_open() {
    let text = "<div><span>x</div>";
    let doc = parse(text);
    assert_eq!(codes(&doc), ["html.unclosed-before-end-tag"]);
    assert_eq!(&text[doc.diagnostics[0].span.clone()], "span");
    let doc = parse("<div><b>x");
    assert_eq!(codes(&doc), ["html.unclosed-element", "html.unclosed-element"]);
    assert!(doc.elements.iter().all(|e| e.close == Close::Open));
    let doc = parse("<p>x</b></p>");
    assert_eq!(codes(&doc), ["html.stray-end-tag"]);
}

#[test]
fn the_text_of_script_style_title_and_textarea_is_not_markup() {
    let text = "<script>if (a < b && \"</div>\") { x = '<p>'; }</script><style>a > b { }</style><title>a <b> c</title><textarea><i></textarea>";
    let doc = parse(text);
    assert_eq!(codes(&doc), Vec::<&str>::new(), "{:?}", doc.diagnostics);
    assert_eq!(names(&doc), [("script", 0), ("style", 0), ("title", 0), ("textarea", 0)]);
    let script = &doc.elements[0];
    assert_eq!(&text[script.end_tag.clone().expect("end tag")], "</script>");
    // A script with no end tag takes the rest of the text.
    let doc = parse("<script>a<b>c");
    assert_eq!(names(&doc), [("script", 0)]);
    assert_eq!(codes(&doc), ["html.unclosed-element"]);
}

#[test]
fn attributes_may_be_quoted_unquoted_or_bare_and_their_places_are_kept() {
    let text = "<a HREF=\"x?a=1&b=2\" class='c d' data-x=1 download target=_blank/>";
    let doc = parse(text);
    let a = &doc.elements[0];
    let attr = |n: &str| a.attribute(n).expect(n);
    assert_eq!(attr("href").value.as_deref(), Some("x?a=1&b=2"));
    assert_eq!(&text[attr("href").value_span.clone().expect("span")], "\"x?a=1&b=2\"");
    assert_eq!(attr("class").value.as_deref(), Some("c d"));
    assert_eq!(attr("data-x").value.as_deref(), Some("1"));
    assert_eq!((attr("download").value.clone(), attr("download").value_span.clone()), (None, None));
    assert_eq!(attr("target").value.as_deref(), Some("_blank/"), "an unquoted value runs to white space or `>`");
    let doc = parse("<p id=a ID=b class=c class=d>");
    assert_eq!(codes(&doc), ["html.duplicate-attribute", "html.duplicate-attribute"]);
    let doc = parse("<div title=\"never closed>x</div>");
    assert_eq!(doc.elements.len(), 1, "an unclosed quote runs to the end of the text");
}

#[test]
fn comments_doctype_and_a_lone_angle_bracket_are_read_and_an_open_comment_is_a_problem() {
    let text = "<!-- a <b> comment --><p>1 < 2 and 3 > 2</p><!-- never";
    let doc = parse(text);
    assert_eq!(codes(&doc), ["html.unterminated-comment"]);
    assert_eq!(doc.comments.len(), 2);
    assert_eq!(&text[doc.comments[0].clone()], "<!-- a <b> comment -->");
    assert_eq!(names(&doc), [("p", 0)]);
    let doc = parse("<p>x<?php echo 1 ?>y</p><![CDATA[ z ]]>");
    assert_eq!(codes(&doc), Vec::<&str>::new(), "{:?}", doc.diagnostics);
}

#[test]
fn svg_and_math_may_close_their_elements_with_a_slash_and_their_names_keep_their_case() {
    let doc = parse("<svg viewBox=\"0 0 1 1\"><path d=\"M0\"/><linearGradient><stop/></linearGradient></svg><p>after</p>");
    assert_eq!(codes(&doc), Vec::<&str>::new(), "{:?}", doc.diagnostics);
    assert!(doc.elements.iter().any(|e| e.name == "linearGradient" && e.foreign));
    assert_eq!(doc.elements.iter().find(|e| e.name == "p").map(|e| e.depth), Some(0), "the svg ended");
}

#[test]
fn what_is_unusual_is_said_unknown_obsolete_self_closing_duplicate_ids_images_and_the_doctype() {
    assert_eq!(codes(&parse("<blorp>x</blorp><my-widget>y</my-widget>")), ["html.unknown-element"]);
    assert_eq!(codes(&parse("<center>x</center>")), ["html.obsolete-element"]);
    assert_eq!(codes(&parse("<div/>text")), ["html.self-closing-ignored", "html.unclosed-element"]);
    assert_eq!(codes(&parse("<p id=a></p><p id=a></p>")), ["html.duplicate-id"]);
    assert_eq!(codes(&parse("<img src=x>")), ["html.missing-alt"]);
    assert_eq!(codes(&parse("<html><body></body></html>")), ["html.missing-doctype"]);
    assert_eq!(codes(&parse("<body></body>")), Vec::<&str>::new(), "a fragment needs no doctype");
}

#[test]
fn nothing_runs_away_deep_nesting_many_elements_and_a_tag_that_never_ends() {
    let deep = "<div>".repeat(MAX_DEPTH + 50);
    let doc = parse(&deep);
    assert!(doc.truncated && doc.elements.len() <= MAX_DEPTH);
    let many = "<i></i>".repeat(5_000);
    assert_eq!(parse(&many).elements.len(), 5_000);
    let doc = parse("<a href=\"x\" class");
    assert_eq!(codes(&doc), ["html.unfinished-tag", "html.unclosed-element"]);
    let attrs = format!("<p {}>", "a ".repeat(5_000));
    assert!(parse(&attrs).elements[0].attributes.len() <= 1_000);
    let noise = "<p>&&<<>>\"'=/".repeat(2_000);
    let _ = parse(&noise);
}

proptest! {
    #[test]
    fn any_text_is_read_with_spans_inside_it_and_on_character_boundaries(text in "[<>/!?=\"' a-zA-Z0-9&;-]{0,300}|\\PC{0,200}") {
        let doc = parse(&text);
        let ok = |r: &std::ops::Range<usize>| r.start <= r.end && r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end);
        for e in &doc.elements {
            prop_assert!(ok(&e.span) && ok(&e.start_tag) && ok(&e.name_span));
            prop_assert!(e.end_tag.as_ref().is_none_or(ok));
            prop_assert!(e.attributes.iter().all(|a| ok(&a.name_span) && a.value_span.as_ref().is_none_or(ok)));
            prop_assert!(e.parent.is_none_or(|p| p < doc.elements.len()));
        }
        for d in &doc.diagnostics {
            prop_assert!(ok(&d.span));
        }
    }
}
