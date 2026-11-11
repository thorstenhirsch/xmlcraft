//! Schematron: rules read and fired, findings with their messages and places.

use xmlcraft_schematron::{CheckKind, CheckState, Limits, Schema, Severity, Totals};

const NS: &str = "http://purl.oclc.org/dsdl/schematron";

fn schema(body: &str) -> Schema {
    Schema::parse(&format!("<schema xmlns=\"{NS}\">{body}</schema>"), &|_| None).expect("reads")
}

const ORDER: &str = "<order><item n=\"1\" qty=\"2\">Tea</item><item n=\"2\" qty=\"0\">Milk</item><item n=\"3\">Oat</item></order>";

#[test]
fn an_assert_that_is_false_and_a_report_that_is_true_are_findings_at_their_nodes() {
    let s = schema(
        r#"<pattern><rule context="item">
             <assert test="@qty">The item <name/> has no quantity.</assert>
             <report test="@qty = 0">Item <value-of select="@n"/> is ordered zero times.</report>
           </rule></pattern>"#,
    );
    let r = s.validate(ORDER, None).expect("validates");
    assert_eq!((r.count, r.fired_rules, r.checks), (2, 1, 2), "{:?}", r.findings);
    let (a, b) = (&r.findings[0], &r.findings[1]);
    assert_eq!((a.kind, a.severity, a.message.as_str()), (CheckKind::Assert, Severity::Error, "The item item has no quantity."), "{a:?}");
    assert_eq!(a.path.as_deref(), Some("/order[1]/item[3]"));
    assert_eq!(&ORDER[a.span.clone().expect("a place")], "<item n=\"3\">Oat</item>");
    assert_eq!((b.kind, b.severity, b.message.as_str()), (CheckKind::Report, Severity::Info, "Item 2 is ordered zero times."), "{b:?}");
    assert_eq!(&ORDER[b.span.clone().expect("a place")], "<item n=\"2\" qty=\"0\">Milk</item>");
    assert!(r.problems.is_empty(), "{:?}", r.problems);
}

#[test]
fn a_node_is_matched_by_the_first_rule_of_a_pattern_only_but_each_pattern_has_its_say() {
    let s = schema(
        r#"<pattern id="a">
             <rule context="item[@n = 1]"><assert test="false()">first</assert></rule>
             <rule context="item"><assert test="false()">second</assert></rule>
           </pattern>
           <pattern id="b"><rule context="item[@n = 1]"><assert test="false()">other pattern</assert></rule></pattern>"#,
    );
    let r = s.validate(ORDER, None).expect("validates");
    let mut messages: Vec<(&str, Option<&str>)> = r.findings.iter().map(|f| (f.message.as_str(), f.pattern.as_deref())).collect();
    messages.sort();
    assert_eq!(
        messages,
        [("first", Some("a")), ("other pattern", Some("b")), ("second", Some("a")), ("second", Some("a"))],
        "item 1 is for the first rule of pattern a, items 2 and 3 for the second, and pattern b matches item 1 again"
    );
}

#[test]
fn lets_namespaces_and_phases() {
    let xml = "<p:doc xmlns:p=\"urn:p\"><p:a v=\"3\"/><p:a v=\"9\"/></p:doc>";
    let s = schema(
        r#"<ns prefix="q" uri="urn:p"/>
           <let name="limit" value="5"/>
           <phase id="strict"><active pattern="max"/></phase>
           <phase id="none"/>
           <pattern id="max"><let name="double" value="$limit * 2"/>
             <rule context="q:a"><let name="v" value="number(@v)"/>
               <assert test="$v &lt;= $limit">value <value-of select="$v"/> is over <value-of select="$limit"/> (twice is <value-of select="$double"/>)</assert></rule></pattern>
           <pattern id="other"><rule context="q:doc"><assert test="false()">other</assert></rule></pattern>"#,
    );
    let r = s.validate(xml, Some("strict")).expect("validates");
    assert_eq!(r.findings.len(), 1, "{:?} {:?}", r.findings, r.problems);
    assert_eq!(r.findings[0].message, "value 9 is over 5 (twice is 10)");
    assert_eq!(s.validate(xml, Some("none")).expect("validates").count, 0, "no pattern is active in that phase");
    assert_eq!(s.validate(xml, None).expect("validates").count, 2, "all the patterns without a phase");
    assert_eq!(s.validate(xml, Some("#ALL")).expect("validates").count, 2);
    assert!(s.validate(xml, Some("nope")).is_err());
    assert_eq!(s.phases(), ["strict", "none"]);
}

#[test]
fn flags_and_roles_make_findings_errors_warnings_or_information() {
    let s = schema(
        r#"<pattern><rule context="order">
             <assert test="false()" flag="fatal" id="BR-1">a</assert>
             <assert test="false()" role="warning">b</assert>
             <assert test="false()" flag="warning">c</assert>
             <report test="true()" role="information">d</report>
             <report test="true()">e</report>
             <assert test="false()">f</assert>
           </rule></pattern>"#,
    );
    let r = s.validate(ORDER, None).expect("validates");
    let got: Vec<(&str, Severity)> = r.findings.iter().map(|f| (f.message.as_str(), f.severity)).collect();
    assert_eq!(
        got,
        [
            ("a", Severity::Error),
            ("b", Severity::Warning),
            ("c", Severity::Warning),
            ("d", Severity::Info),
            ("e", Severity::Info),
            ("f", Severity::Error)
        ]
    );
    assert_eq!((r.findings[0].id.as_deref(), r.findings[0].flag.as_deref()), (Some("BR-1"), Some("fatal")));
    assert_eq!(r.error_count(), 2);
}

#[test]
fn abstract_rules_are_extended_and_abstract_patterns_take_parameters() {
    let s = schema(
        r#"<pattern abstract="true" id="positive"><rule context="$node"><assert test="number($attr) &gt; 0">$attr must be positive</assert></rule></pattern>
           <pattern is-a="positive"><param name="node" value="item"/><param name="attr" value="@qty"/></pattern>
           <pattern><rule abstract="true" id="named"><assert test="normalize-space(.) != ''">empty</assert></rule>
             <rule context="item"><extends rule="named"/></rule></pattern>"#,
    );
    let r = s.validate("<order><item qty=\"1\">x</item><item qty=\"-1\"> </item></order>", None).expect("validates");
    let messages: Vec<&str> = r.findings.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(messages, ["@qty must be positive", "empty"], "{:?} {:?}", r.findings, r.problems);
}

#[test]
fn an_include_is_read_through_the_function_the_caller_gives() {
    let main = format!("<schema xmlns=\"{NS}\"><include href=\"rules.sch\"/></schema>");
    let rules = format!("<pattern xmlns=\"{NS}\"><rule context=\"order\"><assert test=\"false()\">included</assert></rule></pattern>");
    let s = Schema::parse(&main, &|href| (href == "rules.sch").then(|| rules.clone())).expect("reads");
    assert_eq!(s.validate(ORDER, None).expect("validates").findings[0].message, "included");
    let e = Schema::parse(&main, &|_| None).expect_err("not found");
    assert!(e.message.contains("rules.sch"), "{e}");
    // An include of itself ends instead of looping.
    let looping = format!("<schema xmlns=\"{NS}\"><include href=\"self.sch\"/></schema>");
    let e = Schema::parse(&looping, &|_| Some(looping.clone())).expect_err("too deep");
    assert!(e.message.contains("too deeply"), "{e}");
}

#[test]
fn what_cannot_be_evaluated_is_a_problem_and_the_other_rules_still_run() {
    let s = schema(
        r#"<pattern id="p"><rule context="item["><assert test="true()">never</assert></rule>
             <rule context="item"><assert test="false()">after a broken context</assert></rule></pattern>
           <pattern id="q"><rule context="order"><assert test="count(" id="T-1">broken test</assert><assert test="false()">still here</assert></rule></pattern>"#,
    );
    let r = s.validate(ORDER, None).expect("validates");
    assert_eq!(r.findings.iter().map(|f| f.message.as_str()).collect::<Vec<_>>(), ["still here"]);
    // The broken context, the rule after it in the same pattern (which nodes it gets is not known) and the broken test.
    assert_eq!(r.problems.len(), 3, "{:?}", r.problems);
    assert!(r.problems[0].contains("item["), "{:?}", r.problems);
    let states: Vec<&CheckState> = r.outcomes.iter().map(|o| &o.state).collect();
    assert!(matches!(states[0], CheckState::NotEvaluated(why) if why.contains("cannot be evaluated")), "{states:?}");
    assert!(matches!(states[1], CheckState::NotEvaluated(why) if why.contains("earlier rule")), "{states:?}");
    assert!(matches!(states[2], CheckState::NotEvaluated(why) if why.contains("XPST")), "{states:?}");
    assert_eq!(states[3], &CheckState::Failed(1));
    assert_eq!(r.totals(), Totals { checks: 4, not_fired: 0, passed: 0, failed: 1, not_evaluated: 3 });
}

#[test]
fn every_check_has_an_outcome_and_one_that_fails_on_some_nodes_only_still_finds_on_the_others() {
    let s = schema(
        r#"<pattern><rule context="item">
             <assert test="@qty ge 0" id="A">quantity</assert>
             <assert test="xs:integer(@qty) gt 0" id="B">the quantity is positive</assert>
             <assert test="true()" id="P">fine</assert>
           </rule><rule context="nothing"><assert test="false()" id="C">never fires</assert></rule></pattern>"#,
    );
    let r = s.validate(ORDER, None).expect("validates");
    let states: Vec<(Option<&str>, &CheckState)> = r.outcomes.iter().map(|o| (o.id.as_deref(), &o.state)).collect();
    // Item 3 has no quantity (`() ge 0` is false); item 2 has 0.
    assert_eq!(
        states,
        [
            (Some("A"), &CheckState::Failed(1)),
            (Some("B"), &CheckState::Failed(2)),
            (Some("P"), &CheckState::Passed),
            (Some("C"), &CheckState::NotFired)
        ]
    );
    assert_eq!((r.fired_rules, r.checks), (1, 3));
    // A test that fails at run time on one node: the others are still checked.
    let s = schema(r#"<pattern><rule context="item"><assert test="xs:integer(.) gt 0" id="D">numeric</assert></rule></pattern>"#);
    let r = s.validate("<order><item>1</item><item>x</item><item>0</item></order>", None).expect("validates");
    assert_eq!(r.outcomes[0].state, CheckState::Failed(1), "{:?}", r.outcomes);
    assert_eq!(r.findings.iter().map(|f| f.path.as_deref()).collect::<Vec<_>>(), [Some("/order[1]/item[3]")]);
    // When it gives no finding but fails somewhere, it is not evaluated: never passed, never failed.
    let r = s.validate("<order><item>1</item><item>x</item></order>", None).expect("validates");
    assert!(matches!(&r.outcomes[0].state, CheckState::NotEvaluated(why) if why.contains("FORG")), "{:?}", r.outcomes);
    assert!(r.findings.is_empty());
}

#[test]
fn includes_are_taken_from_the_file_that_names_them_and_abstract_patterns_cross_files() {
    let main = format!("<schema xmlns=\"{NS}\"><include href=\"sub/abstract.sch\"/><include href=\"sub/binding.sch\"/></schema>");
    let abstract_ = format!(
        "<schema xmlns=\"{NS}\"><include href=\"../common.sch\"/><pattern abstract=\"true\" id=\"model\"><rule context=\"$Item\"><assert test=\"$BR-1\" id=\"BR-1\">[BR-1] quantity</assert></rule></pattern></schema>"
    );
    let binding = format!(
        "<pattern xmlns=\"{NS}\" is-a=\"model\" id=\"bound\"><param name=\"Item\" value=\"item\"/><param name=\"BR-1\" value=\"@qty &gt; $min\"/></pattern>"
    );
    let common = format!("<pattern xmlns=\"{NS}\" id=\"variables\"><let name=\"min\" value=\"0\"/></pattern>");
    let asked = std::cell::RefCell::new(Vec::new());
    let s = Schema::parse(&main, &|href| {
        asked.borrow_mut().push(href.to_owned());
        match href {
            "sub/abstract.sch" => Some(abstract_.clone()),
            "sub/binding.sch" => Some(binding.clone()),
            "common.sch" => Some(common.clone()),
            _ => None,
        }
    })
    .expect("reads");
    assert_eq!(*asked.borrow(), ["sub/abstract.sch", "common.sch", "sub/binding.sch"]);
    // The variable of another pattern is a global variable.
    let r = s.validate(ORDER, None).expect("validates");
    assert_eq!(
        r.findings.iter().map(|f| (f.id.as_deref(), f.path.as_deref())).collect::<Vec<_>>(),
        [(Some("BR-1"), Some("/order[1]/item[2]")), (Some("BR-1"), Some("/order[1]/item[3]"))],
        "{:?}",
        r.problems
    );
}

#[test]
fn xslt_functions_of_the_schema_are_called_from_tests_and_from_each_other_and_current_is_the_rules_node() {
    let s = Schema::parse(
        &format!(
            r#"<schema xmlns="{NS}" xmlns:xsl="http://www.w3.org/1999/XSL/Transform" xmlns:u="utils" queryBinding="xslt2">
                 <ns prefix="u" uri="utils"/>
                 <let name="limit" value="2"/>
                 <xsl:function name="u:big" as="xs:boolean"><xsl:param name="n"/><xsl:sequence select="u:twice($n) &gt; $limit"/></xsl:function>
                 <xsl:function name="u:twice" as="xs:decimal"><xsl:param name="n" as="xs:decimal"/><xsl:variable name="two">2</xsl:variable><xsl:sequence select="$n * xs:decimal($two)"/></xsl:function>
                 <function xmlns="http://www.w3.org/1999/XSL/Transform" name="u:loop" as="xs:integer"><param name="n" as="xs:integer"/><sequence select="u:loop($n + 1)"/></function>
                 <xsl:function name="u:odd"><xsl:param name="n"/><xsl:for-each select="1"/></xsl:function>
                 <pattern><rule context="item">
                   <report test="u:big(@qty)" id="F">big</report>
                   <assert test="current()/@n = @n" id="G">current</assert>
                   <assert test="u:loop(1) gt 0" id="H">runaway</assert>
                   <assert test="u:odd(1)" id="I">untranslatable</assert>
                   <assert test="u:nope(1)" id="J">undefined</assert>
                 </rule></pattern></schema>"#
        ),
        &|_| None,
    )
    .expect("reads");
    assert_eq!(s.functions().iter().filter(|(_, why)| why.is_some()).count(), 1, "{:?}", s.functions());
    let r = s.validate(ORDER, None).expect("validates");
    let outcome = |id: &str| r.outcomes.iter().find(|o| o.id.as_deref() == Some(id)).map(|o| o.state.clone()).expect("an outcome");
    // Item 1 has qty 2: twice is 4, over the limit; item 3 has none, which is no decimal (a type error on that node alone).
    assert_eq!(outcome("F"), CheckState::Failed(1), "{:?}", r.problems);
    assert_eq!(outcome("G"), CheckState::Passed);
    for (id, why) in [("H", "Stack"), ("I", "xsl:for-each"), ("J", "XPST0017")] {
        assert!(matches!(outcome(id), CheckState::NotEvaluated(w) if w.contains(why)), "{id}: {:?}", outcome(id));
    }
    assert_eq!(r.findings.iter().map(|f| f.id.as_deref()).collect::<Vec<_>>(), [Some("F")]);
}

#[test]
fn a_validation_stops_when_the_caller_says_and_the_rest_is_not_evaluated() {
    let s = schema(
        r#"<pattern><rule context="item"><assert test="true()">a</assert><assert test="false()">b</assert></rule><rule context="order"><assert test="false()">c</assert></rule></pattern>"#,
    );
    let calls = std::cell::Cell::new(0);
    let stop = || {
        calls.set(calls.get() + 1);
        calls.get() > 2
    };
    let r = s.validate_with(ORDER, None, &Limits { max_findings: 10, stop: Some(&stop) }).expect("validates");
    let t = r.totals();
    assert_eq!((t.checks, t.passed, t.not_evaluated), (3, 1, 2), "{:?}", r.outcomes);
    assert!(r.findings.is_empty());
    // The findings are capped and still counted.
    let r = s.validate_with(ORDER, None, &Limits { max_findings: 1, stop: None }).expect("validates");
    assert_eq!((r.findings.len(), r.count), (1, 4));
}

#[test]
fn files_that_are_no_schematron_or_no_xml_are_refused_and_so_is_a_document_that_is_not_xml() {
    for (text, why) in [
        ("<a/>", "not a Schematron schema"),
        ("<schema xmlns=\"http://purl.oclc.org/dsdl/schematron\">", "not well-formed"),
        ("", "not well-formed"),
        (&*format!("<schema xmlns=\"{NS}\"><pattern><rule><assert test=\"1\"/></rule></pattern></schema>"), "no `context`"),
        (&*format!("<schema xmlns=\"{NS}\"><pattern><rule context=\"a\"><assert/></rule></pattern></schema>"), "no `test`"),
    ] {
        let e = Schema::parse(text, &|_| None).expect_err(text);
        assert!(e.message.contains(why), "{text:?}: {e}");
    }
    let s = schema("<pattern><rule context=\"a\"><assert test=\"true()\"/></rule></pattern>");
    assert!(s.validate("<a>", None).is_err());
}

#[test]
fn the_text_of_a_message_keeps_its_references_its_white_space_is_one_space_and_other_elements_are_text() {
    let s = schema(
        "<pattern><rule context=\"order\"><assert test=\"false()\">  A &amp; B\n   <emph>must</emph>   not\t be &lt;here&gt;  </assert></rule></pattern>",
    );
    assert_eq!(s.validate(ORDER, None).expect("validates").findings[0].message, "A & B must not be <here>");
}

proptest::proptest! {
    #[test]
    fn any_schema_text_and_any_document_are_answered_without_a_panic(schema_text in ".{0,200}", xml in ".{0,200}") {
        if let Ok(s) = Schema::parse(&schema_text, &|_| None) {
            let _ = s.validate(&xml, None);
        }
    }

    #[test]
    fn any_expression_in_a_rule_is_a_finding_or_a_problem(context in ".{0,40}", test in ".{0,40}") {
        let text = format!("<schema xmlns=\"{NS}\"><pattern><rule context=\"{}\"><assert test=\"{}\">m</assert></rule></pattern></schema>", context.replace(['"', '<', '&'], ""), test.replace(['"', '<', '&'], ""));
        if let Ok(s) = Schema::parse(&text, &|_| None) {
            let _ = s.validate(ORDER, None);
        }
    }
}

#[test]
fn a_document_of_thousands_of_nodes_with_a_few_dozen_checks_is_validated_in_reasonable_time() {
    let items: String = (0..3000).map(|i| format!("<line n=\"{i}\"><qty>{}</qty><price>1.5</price></line>", i % 7)).collect();
    let xml = format!("<invoice>{items}</invoice>");
    let checks: String = (0..30).map(|i| format!("<assert test=\"number(qty) &gt;= {}\">low {i}</assert>", i % 3)).collect();
    let s = schema(&format!(
        "<pattern><rule context=\"line\">{checks}</rule><rule context=\"invoice\"><assert test=\"count(line) = 3000\">count</assert></rule></pattern>"
    ));
    let started = std::time::Instant::now();
    let r = s.validate(&xml, None).expect("validates");
    let took = started.elapsed();
    assert!(r.count > 0 && r.problems.is_empty(), "{:?}", r.problems);
    eprintln!("validated 3000 lines x 30 checks in {took:?}; {} findings", r.count);
    assert!(took < std::time::Duration::from_secs(60), "{took:?}");
}
