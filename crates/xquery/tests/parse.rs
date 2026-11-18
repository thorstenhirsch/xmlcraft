//! The XQuery 3.1 parser: the grammar's constructs (examples written from the specification's prose), static errors with their
//! codes, spans, limits, and no panics on any input.

use proptest::prelude::*;
use xmlcraft_xquery::ast::{Clause, Decl, Expr, ExprS};
use xmlcraft_xquery::parse::parse;

fn ok(q: &str) -> xmlcraft_xquery::Module {
    match parse(q, &[]) {
        Ok(m) => m,
        Err(e) => panic!("{q:?} should parse: {} {} at {:?} (expected {:?})", e.code, e.message, e.span, e.expected),
    }
}

fn code(q: &str) -> &'static str {
    match parse(q, &[]) {
        Ok(_) => panic!("{q:?} should not parse"),
        Err(e) => e.code,
    }
}

const VALID: &[&str] = &[
    // XPath 3.1 as XQuery
    "1",
    "1, 2.5, 3e2, .5, 'a''b', \"c\"\"d\"",
    "()",
    "(1, 2)[. gt 1]",
    "/",
    "/r/i[@n = '1']//text()",
    "//i/@n",
    "child::a/descendant-or-self::node()/parent::*/..",
    "@*, *:local, xs:*, Q{urn:x}*, Q{urn:x}a",
    "ancestor-or-self::a | following-sibling::b union preceding::c intersect self::d except attribute::e",
    "1 + 2 - 3 * 4 div 5 idiv 6 mod 7",
    "-1, +2, - - 3",
    "1 to 10",
    "'a' || 'b' || 'c'",
    "1 = 2, 1 != 2, 1 < 2, 1 <= 2, 1 > 2, 1 >= 2, 1 eq 2, 1 ne 2, 1 lt 2, 1 le 2, 1 gt 2, 1 ge 2",
    "$a is $b, $a << $b, $a >> $b",
    "true() and false() or true()",
    "if (1) then 2 else 3",
    "some $x in (1, 2), $y in (3, 4) satisfies $x lt $y",
    "every $x as xs:integer in (1, 2) satisfies $x gt 0",
    "for $x in 1 to 3 return $x",
    "let $x := 1, $y := 2 return $x + $y",
    "(1, 2) ! (. * 2)",
    "'abc' => upper-case() => substring(2)",
    "$f => $g(1)",
    "map { 'a': 1, 'b': (2, 3) }?a",
    "[1, 2, 3]?2, [1]?*, map{}?('k')",
    "array { 1 to 3 }, array {}",
    "?name",
    "function($x as xs:integer) as xs:integer { $x + 1 }(2)",
    "%private function() { 1 }",
    "substring(?, 1, 2)('abc')",
    "concat#3, Q{http://www.w3.org/2005/xpath-functions}concat#2",
    "1 instance of xs:integer, 1 treat as xs:integer, 1 castable as xs:string?, '1' cast as xs:integer",
    "$x instance of element(a, xs:anyType)?, $x instance of function(*), $x instance of map(xs:string, item()*)",
    "$x instance of array(xs:integer)+, $x instance of empty-sequence(), $x instance of document-node(element(r))",
    "(: a (: nested :) comment :) 1",
    // FLWOR
    "for $x at $i in ('a', 'b') return $i",
    "for $x allowing empty in () return 1",
    "for $x as xs:integer allowing empty at $p in (1, 2) return $p",
    "for $x in 1 to 3 let $y := $x where $y gt 1 order by $y descending empty greatest return $y",
    "for $x in 1 to 3 stable order by $x ascending empty least collation 'http://www.w3.org/2005/xpath-functions/collation/codepoint', -$x return $x",
    "for $x in 1 to 10 group by $k := $x mod 2 return count($x)",
    "for $x in 1 to 10 let $k := $x mod 2 group by $k, $j as xs:integer := 1 collation 'c' return $k",
    "for $x in 1 to 3 count $c return $c",
    "for tumbling window $w in 1 to 10 start at $s when true() end at $e when $e - $s eq 2 return [$w]",
    "for tumbling window $w as xs:integer* in 1 to 10 start $f previous $p next $n when $f mod 3 eq 1 return $w",
    "for sliding window $w in 1 to 5 start $s at $sp when true() only end $e at $ep when $ep - $sp eq 1 return sum($w)",
    // switch, typeswitch, try
    "switch (1) case 1 case 2 return 'a' case 3 return 'b' default return 'c'",
    "typeswitch (1) case $i as xs:integer | xs:decimal return $i case xs:string return 0 default $d return $d",
    "try { 1 div 0 } catch err:FOAR0001 | err:XPTY0004 { $err:code } catch * { 0 }",
    "try { error() } catch Q{http://www.w3.org/2005/xqt-errors}* { 1 } catch *:FOER0000 { 2 }",
    "try {} catch * {}",
    // constructors
    "<a/>",
    "<a b='1' c=\"{1 + 1}x{{y}}\">text {1} &lt; &#x41; &#65; <b>{2}</b><![CDATA[<raw>]]><!-- c --><?pi data?></a>",
    "<p:a xmlns:p='urn:p' p:b='1'><p:c/></p:a>",
    "<a xmlns='urn:d'><b xmlns=''/></a>",
    "<a>{}</a>",
    "<a>  </a>, <a> {1} </a>",
    "document { <a/> }",
    "element a { 1 }, element { 'b' } { }, element xs:c {} ",
    "attribute a { 1 }, attribute { 'b' } { 2 }",
    "text { 'x' }, comment { 'c' }, processing-instruction pi { 'd' }, processing-instruction { 'p' } {}",
    "namespace p { 'urn:p' }, namespace { 'q' } { 'urn:q' }",
    "<a>{ for $i in 1 to 3 return <b n='{$i}'/> }</a>",
    "``[Hello `{ 'world' }`!]``",
    "ordered { 1 }, unordered { 2 }",
    "(# Q{urn:x}pragma some content #) { 1 }",
    // names that are keywords
    "for, let, return, if, element, div, child, text",
    "/for/let/where",
    "div div div",
    // the prolog
    "xquery version '3.1'; 1",
    "xquery version \"3.0\" encoding 'UTF-8'; 1",
    "xquery encoding 'utf-8'; 1",
    "declare namespace p = 'urn:p'; <p:a/>",
    "declare default element namespace 'urn:d'; a",
    "declare default function namespace 'urn:f'; Q{http://www.w3.org/2005/xpath-functions}true()",
    "declare boundary-space preserve; declare default collation 'http://www.w3.org/2005/xpath-functions/collation/codepoint'; 1",
    "declare base-uri 'http://example.com/'; declare construction strip; declare ordering unordered; 1",
    "declare default order empty greatest; declare copy-namespaces no-preserve, inherit; 1",
    "declare decimal-format d decimal-separator = ',' grouping-separator = '.'; declare default decimal-format NaN = 'x'; 1",
    "declare variable $x := 1; declare variable $y as xs:integer external; declare variable $z external := 2; $x",
    "declare %private variable $v := 1; $v",
    "declare context item as element() external; .",
    "declare function local:f($a, $b as xs:integer) as xs:integer { $a + $b }; local:f(1, 2)",
    "declare %public %Q{urn:x}ann('v', 1) function local:g() { () }; local:g()",
    "declare function local:ext() external; 1",
    "declare option Q{urn:x}opt 'value'; 1",
    "import schema namespace s = 'urn:s' at 's.xsd'; 1",
    "import schema default element namespace 'urn:s'; 1",
    "import module namespace m = 'urn:m' at 'm.xq', 'm2.xq'; 1",
    "module namespace m = 'urn:m'; declare function m:f() { 1 };",
    "declare function local:fact($n) { if ($n le 1) then 1 else $n * local:fact($n - 1) }; local:fact(5)",
    "declare(1)",
];

#[test]
fn every_construct_of_the_grammar_parses() {
    for q in VALID {
        ok(q);
    }
}

const INVALID: &[(&str, &str)] = &[
    ("", "XPST0003"),
    ("1 +", "XPST0003"),
    ("(1", "XPST0003"),
    ("1 2", "XPST0003"),
    ("for $x in 1", "XPST0003"),
    ("for $x return 1", "XPST0003"),
    ("let $x = 1 return $x", "XPST0003"),
    ("if (1) then 2", "XPST0003"),
    ("switch (1) default return 2", "XPST0003"),
    ("try { 1 }", "XPST0003"),
    ("'abc", "XPST0003"),
    ("(: open comment", "XPST0003"),
    ("1div 2", "XPST0003"),
    ("<a>", "XPST0003"),
    ("<a></b>", "XQST0118"),
    ("<a b='1' b='2'/>", "XQST0040"),
    ("<a>}</a>", "XPST0003"),
    ("<a b='<'/>", "XPST0003"),
    ("<a xmlns:p='{1}'/>", "XQST0022"),
    ("<a xmlns:xml='urn:x'/>", "XQST0070"),
    ("<a xmlns:p=''/>", "XQST0085"),
    ("<!-- a -- b -->", "XPST0003"),
    ("<?xml version='1.0'?>", "XPST0003"),
    ("&bogus;", "XPST0003"),
    ("'&bogus;'", "XPST0003"),
    ("'&#0;'", "XQST0090"),
    ("p:a", "XPST0081"),
    ("declare namespace xml = 'urn:x'; 1", "XQST0070"),
    ("declare namespace p = 'urn:a'; declare namespace p = 'urn:b'; 1", "XQST0033"),
    ("xquery version '4.0'; 1", "XQST0031"),
    ("xquery version '3.1' encoding '#bad'; 1", "XQST0087"),
    ("declare function f() { 1 }; 1", "XQST0060"),
    ("declare function fn:f() { 1 }; 1", "XQST0045"),
    ("declare function local:f($a, $a) { 1 }; 1", "XQST0039"),
    ("for $x at $x in 1 return 1", "XQST0089"),
    ("declare decimal-format d NaN = 'a' NaN = 'b'; 1", "XQST0114"),
    ("(# p #) {}", "XQST0079"),
    ("1 instance of", "XPST0003"),
    ("$x instance of p:t", "XPST0081"),
    ("for sliding window $w in 1 start when true() return 1", "XPST0003"),
    ("``[unterminated", "XPST0003"),
];

#[test]
fn static_errors_have_their_codes() {
    for (q, expected) in INVALID {
        assert_eq!(code(q), *expected, "{q:?}");
    }
}

#[test]
fn a_syntax_error_says_where_and_what_would_fit() {
    let e = parse("let $x := 1\nreturn $x +", &[]).unwrap_err();
    assert_eq!(e.code, "XPST0003");
    assert_eq!(e.span.start, 23);
    let e = parse("for $x in (1, 2) retrun $x", &[]).unwrap_err();
    assert!(e.expected.iter().any(|t| t == "'return'"), "{:?}", e.expected);
    assert!(e.message.contains("expected"), "{}", e.message);
}

fn body(m: &xmlcraft_xquery::Module) -> &ExprS {
    m.body.as_ref().expect("a body")
}

#[test]
fn every_node_has_the_span_of_its_text() {
    let q = "for $x in (1, 2) where $x gt 1 return <a n='{$x}'>{ $x + 1 }</a>";
    let m = ok(q);
    let b = body(&m);
    assert_eq!(&q[b.span.clone()], q);
    let Expr::Flwor { clauses, ret } = &b.value else { panic!("a FLWOR") };
    assert_eq!(&q[clauses[0].span.clone()], "$x in (1, 2)");
    let Clause::For { expr, .. } = &clauses[0].value else { panic!("for") };
    assert_eq!(&q[expr.span.clone()], "(1, 2)");
    let Clause::Where(w) = &clauses[1].value else { panic!("where") };
    assert_eq!(&q[w.span.clone()], "$x gt 1");
    assert_eq!(&q[ret.span.clone()], "<a n='{$x}'>{ $x + 1 }</a>");
    // every span is inside the query, and each child's inside its parent's
    let mut stack = vec![b];
    while let Some(e) = stack.pop() {
        assert!(e.span.start <= e.span.end && e.span.end <= q.len(), "{:?}", e.span);
        for c in e.value.children() {
            assert!(c.span.start >= e.span.start && c.span.end <= e.span.end, "{:?} in {:?}", c.span, e.span);
            stack.push(c);
        }
    }
}

#[test]
fn the_text_of_a_span_parses_again_to_the_same_kind_of_expression() {
    for q in VALID.iter().filter(|q| !q.contains(';')) {
        let m = ok(q);
        let b = body(&m);
        for c in b.value.children() {
            let text = &q[c.span.clone()];
            // (Steps of a path and predicates only parse in their place; constructors' content is not an expression.)
            if matches!(c.value, Expr::AxisStep { .. } | Expr::ContextItem) || text.trim().is_empty() {
                continue;
            }
            if let Ok(again) = parse(text, &[]) {
                let a = body(&again);
                assert_eq!(std::mem::discriminant(&a.value), std::mem::discriminant(&c.value), "{text:?} from {q:?}");
            }
        }
    }
}

#[test]
fn the_prolog_is_read_in_order() {
    let m = ok("declare namespace p = 'urn:p'; declare variable $p:v := 1; declare function p:f() { $p:v }; p:f()");
    assert_eq!(m.prolog.len(), 3);
    assert!(matches!(m.prolog[0].value, Decl::Namespace { .. }));
    let Decl::Variable { name, .. } = &m.prolog[1].value else { panic!("variable") };
    assert_eq!(xmlcraft_xquery::ast::Name::new("v".into(), "urn:p".into(), "p".into()), *name);
    assert!(matches!(m.prolog[2].value, Decl::Function(_)));
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    let deep = format!("{}1{}", "(".repeat(5000), ")".repeat(5000));
    assert_eq!(code(&deep), "XPST0003");
    let elements = format!("{}{}", "<a>".repeat(3000), "</a>".repeat(3000));
    assert_eq!(code(&elements), "XPST0003");
    let chain = vec!["1"; 20_000].join(" + ");
    assert_eq!(code(&chain), "XPST0003");
    let comments = format!("{}1{}", "(:".repeat(5000), ":)".repeat(5000));
    assert_eq!(code(&comments), "XPST0003");
    // a long flat sequence is fine
    ok(&vec!["1"; 20_000].join(", "));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    #[test]
    fn no_input_makes_the_parser_panic(s in "\\PC{0,64}") {
        let _ = parse(&s, &[]);
    }

    #[test]
    fn no_mutation_of_a_valid_query_makes_the_parser_panic(i in 0..VALID.len(), at in 0usize..200, len in 0usize..8, insert in "[ -~]{0,6}") {
        let q = VALID[i];
        let mut cut = at.min(q.len());
        while !q.is_char_boundary(cut) { cut -= 1; }
        let mut end = (cut + len).min(q.len());
        while !q.is_char_boundary(end) { end -= 1; }
        let mutated = format!("{}{}{}", &q[..cut], insert, &q[end..]);
        let _ = parse(&mutated, &[]);
    }
}
