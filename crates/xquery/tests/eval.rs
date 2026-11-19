//! Evaluation of XQuery 3.1: the prolog, FLWOR, conditionals, errors, results, limits.

use xmlcraft_xquery::{Options, ParamValue, Phase, Serialization, Variable, evaluate};

fn with(query: &str, context: Option<&str>, options: Options) -> Result<String, (String, Phase)> {
    let options = Options { serialize: Some(options.serialize.unwrap_or(Serialization::Xml)), ..options };
    evaluate(query, context, &options).map(|o| o.serialized.unwrap_or_default()).map_err(|e| (e.code, e.phase))
}

fn run(query: &str) -> String {
    match with(query, None, Options::default()) {
        Ok(s) => s,
        Err(e) => panic!("{query:?} failed: {e:?}"),
    }
}

fn fails(query: &str) -> String {
    match with(query, None, Options::default()) {
        Ok(s) => panic!("{query:?} should fail, gave {s:?}"),
        Err((code, _)) => code,
    }
}

const DOC: &str = "<shop><item n='3' cat='b'>pear</item><item n='1' cat='a'>apple</item><item cat='a' n='2'>fig</item></shop>";

#[test]
fn the_prolog_declares_variables_and_functions() {
    assert_eq!(run("declare variable $x := 2; declare variable $y := $x * 3; $y"), "6");
    assert_eq!(run("declare function local:twice($n as xs:integer) as xs:integer { $n * 2 }; local:twice(21)"), "42");
    assert_eq!(run("declare namespace m = 'urn:m'; declare function m:f() { 'm' }; m:f()"), "m");
    // functions see the globals, and each other in any order
    assert_eq!(
        run("declare variable $base := 10; declare function local:a($n) { local:b($n) + $base }; declare function local:b($n) { $n }; local:a(1)"),
        "11"
    );
    // a function item of a declared function, and a partial application
    assert_eq!(
        run("declare function local:add($a, $b) { $a + $b }; (local:add#2(1, 2), local:add(10, ?)(5), for-each((1, 2), local:add(?, 100)))"),
        "3 15 101 102"
    );
}

#[test]
fn recursion_works_and_runaway_recursion_is_stopped() {
    assert_eq!(run("declare function local:fact($n) { if ($n le 1) then 1 else $n * local:fact($n - 1) }; local:fact(20)"), "2432902008176640000");
    assert_eq!(run("declare function local:len($s) { if (empty($s)) then 0 else 1 + local:len(tail($s)) }; local:len(1 to 1000)"), "1000");
    let code = fails("declare function local:loop($n) { local:loop($n + 1) }; local:loop(0)");
    assert!(code == "StackOverflow" || code == "XMLC0001", "{code}");
}

#[test]
fn external_variables_come_from_the_options() {
    let options = Options {
        variables: vec![
            Variable { name: "n".into(), value: ParamValue::Number("4".into()) },
            Variable { name: "s".into(), value: ParamValue::String("x".into()) },
        ],
        ..Options::default()
    };
    let q = "declare variable $n as xs:integer external; declare variable $s external; declare variable $d external := 'dflt'; ($n * 2, $s, $d)";
    assert_eq!(with(q, None, options).unwrap(), "8 x dflt");
    assert_eq!(fails("declare variable $n external; $n + 1"), "XPDY0002");
    assert_eq!(run("declare variable $unused external; 1"), "1");
    // a declared type is checked
    let options = Options { variables: vec![Variable { name: "n".into(), value: ParamValue::String("x".into()) }], ..Options::default() };
    assert_eq!(with("declare variable $n as xs:integer external; $n", None, options).unwrap_err().0, "XPTY0004");
}

#[test]
fn order_by_sorts_stably_with_every_modifier() {
    assert_eq!(run("for $x in (3, 1, 2) order by $x return $x"), "1 2 3");
    assert_eq!(run("for $x in (3, 1, 2) order by $x descending return $x"), "3 2 1");
    // two keys, mixed directions; ties keep their order
    let q = "for $p in ((1, 'b'), (2, 'a'), (1, 'a'), (2, 'b')) ! [.] order by $p(1) descending, $p(2) return $p(1) || $p(2)";
    let q = q.replace("((1, 'b'), (2, 'a'), (1, 'a'), (2, 'b')) ! [.]", "([1, 'b'], [2, 'a'], [1, 'a'], [2, 'b'])");
    assert_eq!(run(&q), "2a 2b 1a 1b");
    assert_eq!(run("for $x at $i in ('b', 'a', 'b', 'a') stable order by $x return $i"), "2 4 1 3");
    // empty keys
    assert_eq!(run("for $x in ([2], [()], [1]) order by $x(1) empty least return count($x(1))"), "0 1 1");
    assert_eq!(run("for $x in ([2], [()], [1]) order by $x(1) descending return count($x(1))"), "1 1 0");
    assert_eq!(run("for $x in ([2], [()], [1]) order by $x(1) empty greatest return count($x(1))"), "1 1 0");
    assert_eq!(run("declare default order empty greatest; for $x in ([2], [()], [1]) order by $x(1) return count($x(1))"), "1 1 0");
    // untyped values sort as strings, nodes by their value
    assert_eq!(with("for $i in //item order by $i/@n return string($i)", Some(DOC), Options::default()).unwrap(), "apple fig pear");
    assert_eq!(fails("for $x in 1 to 2 order by (1, 2) return $x"), "XPTY0004");
}

#[test]
fn group_by_binds_keys_and_joins_the_rest() {
    let q = "for $i in //item let $c := string($i/@cat) group by $c order by $c return $c || ':' || string-join($i, ',')";
    assert_eq!(with(q, Some(DOC), Options::default()).unwrap(), "a:apple,fig b:pear");
    assert_eq!(run("for $x in 1 to 10 group by $k := $x mod 3 order by $k return $k || '=' || sum($x)"), "0=18 1=22 2=15");
    assert_eq!(run("for $x in (1, 2, 3, 4) group by $odd := $x mod 2 = 1, $big := $x gt 2 order by $odd, $big return count($x)"), "1 1 1 1");
    assert_eq!(fails("for $x in 1 to 3 group by $nope return $x"), "XQST0094");
}

#[test]
fn count_numbers_the_tuples() {
    assert_eq!(run("for $x in ('a', 'b', 'c') order by $x descending count $c return $c || $x"), "1c 2b 3a");
}

#[test]
fn for_has_positions_and_allowing_empty() {
    assert_eq!(run("for $x at $i in ('a', 'b') return $i || $x"), "1a 2b");
    assert_eq!(run("for $x allowing empty at $i in () return 'none' || $i"), "none0");
    assert_eq!(run("for $x as xs:integer in (1, 2) return $x"), "1 2");
    assert_eq!(fails("for $x as xs:string in (1, 2) return $x"), "XPTY0004");
    assert_eq!(fails("let $x as xs:string := 1 return $x"), "XPTY0004");
}

#[test]
fn windows_are_found() {
    let q = "for tumbling window $w in 1 to 7 start at $s when $s mod 3 = 1 return string-join($w ! string(.), '')";
    assert_eq!(run(q), "123 456 7");
    let q = "for tumbling window $w in 1 to 7 start at $s when true() end at $e when $e - $s eq 1 return sum($w)";
    assert_eq!(run(q), "3 7 11 7");
    let q = "for tumbling window $w in 1 to 7 start at $s when true() only end at $e when $e - $s eq 1 return sum($w)";
    assert_eq!(run(q), "3 7 11");
    let q = "for sliding window $w in 1 to 4 start at $s when true() end at $e when $e - $s eq 1 return sum($w)";
    assert_eq!(run(q), "3 5 7 4");
    let q = "for sliding window $w in (2, 4, 6, 5) start $f previous $p when empty($p) or $f gt $p end $l next $n when empty($n) or $n lt $l return count($w)";
    assert_eq!(run(q), "3 2 1");
}

#[test]
fn switch_and_typeswitch_choose() {
    assert_eq!(run("switch (2) case 1 return 'a' case 2 case 3 return 'b' default return 'c'"), "b");
    assert_eq!(run("switch ('x') case 'y' return 1 default return 0"), "0");
    assert_eq!(run("switch (()) case () return 'empty' default return 'no'"), "empty");
    assert_eq!(
        run(
            "for $v in (1, 'a', 1.5, <e/>) return typeswitch ($v) case xs:integer return 'i' case $s as xs:string return $s case xs:decimal | xs:double return 'd' default $d return name($d)"
        ),
        "i a d e"
    );
}

#[test]
fn try_catch_catches_by_code_and_binds_the_error() {
    assert_eq!(run("try { 1 div 0 } catch err:FOAR0001 { 'div' }"), "div");
    assert_eq!(run("try { 1 div 0 } catch * { $err:code }"), "err:FOAR0001");
    assert_eq!(run("try { error(QName('urn:x', 'x:mine'), 'my message') } catch Q{urn:x}mine { $err:description }"), "my message");
    assert_eq!(run("try { error(QName('urn:x', 'x:mine')) } catch err:FOAR0001 { 1 } catch *:mine { 2 }"), "2");
    // not caught by a clause that does not match
    assert_eq!(fails("try { 1 div 0 } catch err:XPTY0004 { 1 }"), "FOAR0001");
    // the context item is seen inside, and nested try works
    assert_eq!(with("try { count(//item) } catch * { -1 }", Some(DOC), Options::default()).unwrap(), "3");
    assert_eq!(run("try { try { 1 div 0 } catch err:XPTY0004 { 0 } } catch * { 'outer' }"), "outer");
    assert_eq!(run("for $x in (1, 0, 2) return try { 10 idiv $x } catch * { 'x' }"), "10 x 5");
    // the step budget is not caught
    let options = Options { max_steps: 5_000, ..Options::default() };
    let r = with("try { sum(for $i in 1 to 100000 return $i) } catch * { 'caught' }", None, options);
    assert_eq!(r.unwrap_err().0, "XMLC0001");
}

#[test]
fn string_constructors_interpolate() {
    assert_eq!(run("let $n := 'world' return ``[Hello `{$n}`, `{1 to 3}`!]``"), "Hello world, 1 2 3!");
}

#[test]
fn quantified_expressions_bind_several_variables() {
    assert_eq!(run("some $x in (1, 2), $y in (2, 3) satisfies $x eq $y"), "true");
    assert_eq!(run("every $x in (1, 2), $y in (3, 4) satisfies $x lt $y"), "true");
}

#[test]
fn unsupported_features_are_refused_with_their_codes() {
    assert_eq!(fails("import schema namespace s = 'urn:s'; 1"), "XQST0009");
    assert_eq!(fails("import module namespace m = 'urn:m'; 1"), "XQST0059");
    assert_eq!(fails("validate { <a/> }"), "XQST0075");
    assert_eq!(fails("declare default collation 'urn:other'; 1"), "XQST0038");
    assert_eq!(fails("declare function local:x() external; local:x()"), "XPST0017");
    assert_eq!(fails("$undeclared"), "XPST0008");
    assert_eq!(fails("local:nope()"), "XPST0017");
    assert_eq!(fails("declare function local:f() { 1 }; declare function local:f() { 2 }; 1"), "XQST0034");
    assert_eq!(fails("declare variable $v := 1; declare variable $v := 2; 1"), "XQST0049");
}

#[test]
fn documents_come_from_the_options_and_nowhere_else() {
    let options = Options { documents: vec![("urn:doc:a".into(), "<a><b/></a>".into())], ..Options::default() };
    assert_eq!(with("count(doc('urn:doc:a')//b)", None, options).unwrap(), "1");
    assert_eq!(fails("doc('file:///etc/hosts')"), "FODC0002");
}

#[test]
fn results_say_where_document_nodes_are() {
    let o = evaluate("(//item[@n = '1'], <made/>, 7)", Some(DOC), &Options::default()).unwrap();
    assert_eq!(o.result.count, 3);
    let item = &o.result.items[0];
    assert_eq!(item.kind, "element");
    assert_eq!(item.path.as_deref(), Some("/shop[1]/item[2]"));
    assert!(item.span.is_some());
    let made = &o.result.items[1];
    assert_eq!(made.kind, "element");
    assert_eq!(made.path, None);
    assert_eq!(made.span, None);
    assert!(!made.unmapped);
    assert_eq!(made.text, "<made/>");
    assert_eq!(o.result.items[2].kind, "xs:integer");
}

#[test]
fn results_serialize_as_asked() {
    let json = Options { serialize: Some(Serialization::Json), ..Options::default() };
    assert_eq!(with("map { 'a': [1, 'x', true()] }", None, json).unwrap(), r#"{"a":[1,"x",true]}"#);
    let text = Options { serialize: Some(Serialization::Text), ..Options::default() };
    assert_eq!(with("(<a>x<b>y</b></a>, 1)", None, text).unwrap(), "xy 1");
    let adaptive = Options { serialize: Some(Serialization::Adaptive), ..Options::default() };
    assert_eq!(with("('a', 1, <e/>)", None, adaptive).unwrap(), "\"a\"\n1\n<e/>");
}

#[test]
fn the_step_budget_stops_a_long_run() {
    let options = Options { max_steps: 10_000, ..Options::default() };
    let e = evaluate("count(for $i in 1 to 1000000 return $i * 2)", None, &options).unwrap_err();
    assert_eq!(e.code, "XMLC0001");
    assert_eq!(e.phase, Phase::Run);
}

#[test]
fn errors_while_running_have_a_place() {
    let e = evaluate("let $x := 0 return\n  10 idiv $x", None, &Options::default()).unwrap_err();
    assert_eq!(e.code, "FOAR0001");
    assert_eq!(e.phase, Phase::Run);
    let span = e.span.expect("a place");
    assert!(span.start >= 20, "{span:?}");
}

#[test]
fn the_deepest_queries_the_parser_accepts_compile_and_run_on_a_small_stack() {
    // For each shape, the deepest query the parser accepts.
    let deepest = |make: &dyn Fn(usize) -> String| -> String {
        let mut n = 1;
        while n < 2000 && xmlcraft_xquery::parse::parse(&make(n + 1), &[]).is_ok() {
            n += 1;
        }
        make(n)
    };
    let nested = deepest(&|n| format!("{}1{}", "(".repeat(n), ")".repeat(n)));
    let chain = deepest(&|n| vec!["1"; n].join(" + "));
    let elements = deepest(&|n| format!("count({}{})", "<a>".repeat(n), "</a>".repeat(n)));
    let flwor = deepest(&|n| format!("{}1", "for $x in 1 return ".repeat(n)));
    // The limits leave room for real queries: parentheses 12 deep, constructors 16, FLWORs 8, chains of 120 operators.
    assert!(nested.matches('(').count() >= 12, "{nested}");
    assert!(elements.matches("<a>").count() >= 16, "{elements}");
    assert!(flwor.matches("for").count() >= 8, "{flwor}");
    assert!(chain.matches('+').count() >= 120, "{chain}");
    let handle = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || [nested, chain, elements, flwor].map(|q| evaluate(&q, None, &Options::default()).map(|o| o.result.count)))
        .expect("a thread");
    let results = handle.join().expect("no stack overflow");
    for r in results {
        assert_eq!(r.map_err(|e| e.code), Ok(1));
    }
}

mod properties {
    use proptest::prelude::*;
    use xmlcraft_xquery::{Options, evaluate};

    const QUERIES: &[&str] = &[
        "for $x in 1 to 5 let $y := $x * 2 where $y gt 2 order by $y descending count $c return <r c='{$c}'>{$y}</r>",
        "for $x in 1 to 9 group by $k := $x mod 3 return [$k, sum($x)]",
        "declare function local:f($n) { if ($n le 0) then () else ($n, local:f($n - 1)) }; local:f(5)",
        "try { error(QName('urn:x', 'x:e'), 'm') } catch * { $err:code, $err:description }",
        "for tumbling window $w in 1 to 10 start at $s when $s mod 2 = 1 end at $e when $e = $s + 1 return sum($w)",
        "typeswitch (<a/>) case element() return 1 default return 2",
        "switch (3) case 1 return 'a' default return 'b'",
        "element { 'e' } { attribute a { 1 }, text { 'x' }, comment { 'c' } }",
        "``[a `{ 1 to 3 }` b]``",
        "map { 'a': [1, 2] }?a?*",
    ];

    proptest! {
        #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

        #[test]
        fn no_mutated_query_makes_evaluation_panic(i in 0..QUERIES.len(), at in 0usize..160, len in 0usize..6, insert in "[ -~]{0,5}") {
            let q = QUERIES[i];
            let mut cut = at.min(q.len());
            while !q.is_char_boundary(cut) { cut -= 1; }
            let mut end = (cut + len).min(q.len());
            while !q.is_char_boundary(end) { end -= 1; }
            let mutated = format!("{}{}{}", &q[..cut], insert, &q[end..]);
            let options = Options { max_steps: 200_000, ..Options::default() };
            let _ = evaluate(&mutated, Some("<r><i>1</i></r>"), &options);
        }
    }
}
