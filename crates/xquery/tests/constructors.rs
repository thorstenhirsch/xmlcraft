//! Node constructors: direct and computed, namespaces, attributes, content rules and their errors.

use xmlcraft_xquery::{Options, Serialization, evaluate};

fn run(query: &str) -> String {
    let options = Options { serialize: Some(Serialization::Xml), ..Options::default() };
    match evaluate(query, None, &options) {
        Ok(o) => o.serialized.unwrap_or_default(),
        Err(e) => panic!("{query:?} failed: {} {}", e.code, e.message),
    }
}

fn fails(query: &str) -> String {
    let options = Options { serialize: Some(Serialization::Xml), ..Options::default() };
    match evaluate(query, None, &options) {
        Ok(o) => panic!("{query:?} should fail, gave {:?}", o.serialized),
        Err(e) => e.code,
    }
}

#[test]
fn direct_elements_have_attributes_text_and_children() {
    assert_eq!(run("<a x='1' y='{1 + 1}z'>t<b/>{'u'}</a>"), r#"<a x="1" y="2z">t<b/>u</a>"#);
    assert_eq!(run("<a>{1, 2}{3}</a>"), "<a>1 23</a>");
    assert_eq!(run("<a>&lt;&amp;&#65;<![CDATA[<x>]]></a>"), "<a>&lt;&amp;A&lt;x&gt;</a>");
    assert_eq!(run("<a>{{literal}}</a>"), "<a>{literal}</a>");
    assert_eq!(run("<a><!--c--><?p d?></a>"), "<a><!--c--><?p d?></a>");
}

#[test]
fn boundary_whitespace_is_dropped_unless_preserved() {
    assert_eq!(run("<a> <b/> {1} </a>"), "<a><b/>1</a>");
    assert_eq!(run("declare boundary-space preserve; <a> <b/> </a>"), "<a> <b/> </a>");
    assert_eq!(run("<a> x </a>"), "<a> x </a>");
    assert_eq!(run("<a>&#32;</a>"), "<a> </a>");
}

#[test]
fn namespaces_and_prefixes_are_kept() {
    assert_eq!(run("<p:a xmlns:p='urn:p'><p:b/></p:a>"), r#"<p:a xmlns:p="urn:p"><p:b/></p:a>"#);
    assert_eq!(run("declare namespace q = 'urn:q'; <q:a/>"), r#"<q:a xmlns:q="urn:q"/>"#);
    assert_eq!(run("<a xmlns='urn:d'><b/></a>/*:b ! namespace-uri(.)"), "urn:d");
    assert_eq!(run("namespace-uri(<a xmlns='urn:d'/>)"), "urn:d");
}

#[test]
fn content_nodes_are_copied() {
    // the node bound to $x is not moved into the new element
    assert_eq!(run("let $x := <x/> return (<w>{$x}</w>, count($x/..))"), "<w><x/></w>0");
    assert_eq!(run("let $d := <r><i>1</i><i>2</i></r> return <c>{$d/i}</c>"), "<c><i>1</i><i>2</i></c>");
    // a document's children are added, an array is flattened
    assert_eq!(run("<a>{document { <b/>, 'x' }}</a>"), "<a><b/>x</a>");
    assert_eq!(run("<a>{[1, [2, 3]]}</a>"), "<a>1 2 3</a>");
}

#[test]
fn content_errors_have_their_codes() {
    assert_eq!(fails("<a>{<b/>, attribute c {1}}</a>"), "XQTY0024");
    assert_eq!(fails("<a>{attribute c {1}, attribute c {2}}</a>"), "XQDY0025");
    assert_eq!(fails("<a c='1'>{attribute c {2}}</a>"), "XQDY0025");
    assert_eq!(fails("<a>{function() {1}}</a>"), "XQTY0105");
    assert_eq!(fails("document { attribute a {1} }"), "XPTY0004");
}

#[test]
fn computed_constructors_build_every_kind_of_node() {
    assert_eq!(run("element a { attribute b { 1, 2 }, 'x' }"), r#"<a b="1 2">x</a>"#);
    assert_eq!(run("element { 'n' || 1 } {}"), "<n1/>");
    assert_eq!(run("element { QName('urn:z', 'z:e') } {} ! namespace-uri(.)"), "urn:z");
    assert_eq!(run("declare namespace p = 'urn:p'; element { 'p:e' } {} ! namespace-uri(.)"), "urn:p");
    assert_eq!(run("text { 'a', 'b' }"), "a b");
    assert_eq!(run("count(text { () })"), "0");
    assert_eq!(run("comment { 'c' }"), "<!--c-->");
    assert_eq!(run("processing-instruction p { '  d' }"), "<?p d?>");
    assert_eq!(run("document { <r/> } instance of document-node()"), "true");
    assert_eq!(run("<a>{ namespace p { 'urn:p' } }</a> ! in-scope-prefixes(.) = 'p'"), "true");
}

#[test]
fn computed_constructor_errors_have_their_codes() {
    assert_eq!(fails("element { 'bad name' } {}"), "XQDY0074");
    assert_eq!(fails("element { 'u:e' } {}"), "XQDY0074");
    assert_eq!(fails("attribute { 'xmlns' } {}"), "XQDY0044");
    assert_eq!(fails("comment { 'a--b' }"), "XQDY0072");
    assert_eq!(fails("processing-instruction { 'xml' } {}"), "XQDY0064");
    assert_eq!(fails("processing-instruction p { '?>' }"), "XQDY0026");
    assert_eq!(fails("namespace xmlns { 'urn:x' }"), "XQDY0101");
}

#[test]
fn constructed_trees_can_be_queried() {
    assert_eq!(run("let $t := <r><i n='1'/><i n='2'/></r> return sum($t/i/@n)"), "3");
    assert_eq!(run("let $t := <r>{for $i in 1 to 3 return <i>{$i}</i>}</r> return count($t//i)"), "3");
    assert_eq!(run("<r>{for $i in 1 to 3 order by $i descending return <i>{$i}</i>}</r>"), "<r><i>3</i><i>2</i><i>1</i></r>");
}
