//! Schematron: rules read and fired, findings with their messages and places.

use xmlcraft_schematron::{CheckKind, Schema, Severity};

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
        r#"<pattern><rule context="item["><assert test="true()">never</assert></rule>
             <rule context="order"><assert test="count(">broken test</assert><assert test="false()">still here</assert></rule></pattern>"#,
    );
    let r = s.validate(ORDER, None).expect("validates");
    assert_eq!(r.findings.iter().map(|f| f.message.as_str()).collect::<Vec<_>>(), ["still here"]);
    assert_eq!(r.problems.len(), 2, "{:?}", r.problems);
    assert!(r.problems[0].contains("item["), "{:?}", r.problems);
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
