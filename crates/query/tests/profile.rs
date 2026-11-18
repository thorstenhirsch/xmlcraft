//! The profiler: how often each node of an expression was evaluated, what it produced and what it cost.

use proptest::prelude::*;
use xmlcraft_query::{Limits, Options, Profile, ProfileNode, profile};

/// A shop of `n` items; item `i` costs `i` (so `price > 10` holds for the items from 11 on).
fn shop(n: usize) -> String {
    let items: String = (1..=n).map(|i| format!("<item><price>{i}</price><name>n{i}</name></item>")).collect();
    format!("<shop>{items}</shop>")
}

fn node<'p>(p: &'p Profile, text: &str) -> &'p ProfileNode {
    p.nodes.iter().find(|n| n.text == text).unwrap_or_else(|| panic!("no node {text:?} in {:?}", p.nodes.iter().map(|n| &n.text).collect::<Vec<_>>()))
}

#[test]
fn a_predicate_is_hit_once_for_each_item_it_tests_and_a_step_once_for_each_context_item() {
    let n = 100;
    let p = profile(&shop(n), "//item[price > 10]/name", &Options::default(), &Limits::default());
    let result = p.result.as_ref().expect("runs");
    assert_eq!(result.count, 90);
    let predicate = node(&p, "price > 10");
    assert!(predicate.is_loop);
    assert_eq!(predicate.hits, 100, "the predicate is tested on each item");
    assert_eq!(predicate.items, 90, "and keeps the 90 that cost more than 10");
    assert_eq!(node(&p, "price").hits, 100, "its first operand is evaluated for each item too");
    assert_eq!(node(&p, "price").selected, 100, "and selects one price each");
    let name = node(&p, "name");
    assert_eq!(name.hits, 90, "the last step is evaluated for each item that passed");
    assert_eq!(name.items, 90);
    // `item[…]` is evaluated for each node `//` gives: the document, the shop, and an item, a price, its text, a name and its text for
    // each item.
    assert_eq!(node(&p, "item[price > 10]").hits, 2 + 5 * n as u64);
}

#[test]
fn the_tree_nests_by_the_text_and_totals_include_what_is_inside() {
    let p = profile(&shop(20), "count(//item[price > 10])", &Options::default(), &Limits::default());
    assert_eq!(p.result.as_ref().expect("runs").items[0].text, "10");
    let root = &p.nodes[0];
    assert_eq!(root.text, "count(//item[price > 10])");
    assert_eq!(root.parent, None);
    let predicate = p.nodes.iter().position(|n| n.text == "price > 10").expect("the predicate");
    let mut at = p.nodes[predicate].parent;
    let mut chain = Vec::new();
    while let Some(i) = at {
        chain.push(p.nodes[i].text.clone());
        at = p.nodes[i].parent;
    }
    assert_eq!(chain.first().map(String::as_str), Some("item[price > 10]"), "a predicate is in its step");
    assert_eq!(chain.last().map(String::as_str), Some("count(//item[price > 10])"));
    for (i, node) in p.nodes.iter().enumerate() {
        let inside: u64 = p.nodes.iter().filter(|c| c.parent == Some(i)).map(|c| c.total_instructions).sum();
        assert_eq!(node.total_instructions, node.self_instructions + inside, "{}", node.text);
        assert!(node.total_nanos >= node.self_nanos);
    }
    let accounted: u64 = p.nodes.iter().map(|n| n.self_instructions).sum();
    assert!(
        accounted <= p.instructions && accounted * 10 >= p.instructions * 9,
        "almost every instruction belongs to a node: {accounted} of {}",
        p.instructions
    );
}

#[test]
fn the_hot_spots_are_the_costliest_first_and_counts_are_the_same_every_run() {
    let doc = shop(200);
    let expression = "sum(//item[price mod 7 = 0]/price) + count(/shop)";
    let a = profile(&doc, expression, &Options::default(), &Limits::default());
    let b = profile(&doc, expression, &Options::default(), &Limits::default());
    let counts = |p: &Profile| p.nodes.iter().map(|n| (n.text.clone(), n.hits, n.items, n.self_instructions)).collect::<Vec<_>>();
    assert_eq!(counts(&a), counts(&b), "the counts do not depend on the run");
    assert_eq!(a.instructions, b.instructions);
    assert!(!a.hot.is_empty());
    assert!(a.timed);
    let hot: Vec<&ProfileNode> = a.hot.iter().map(|&i| &a.nodes[i]).collect();
    for pair in hot.windows(2) {
        assert!(pair[0].self_nanos >= pair[1].self_nanos, "hottest first");
    }
    // The predicate over 200 items costs far more than `/shop`, evaluated once.
    let place = |text: &str| hot.iter().position(|n| n.text == text);
    let predicate = place("price mod 7 = 0").expect("the predicate is hot");
    assert!(place("shop").is_none_or(|shop| predicate < shop), "{:?}", hot.iter().map(|n| &n.text).collect::<Vec<_>>());
}

#[test]
fn a_for_counts_its_iterations_and_a_function_its_calls() {
    let p = profile("<r/>", "for $x in 1 to 5 return $x * 2", &Options::default(), &Limits::default());
    assert_eq!(p.result.as_ref().expect("runs").count, 5);
    assert_eq!(node(&p, "for $x in 1 to 5 return $x * 2").hits, 5);
    assert_eq!(node(&p, "$x * 2").hits, 5);
    assert_eq!(node(&p, "$x").hits, 5, "a variable the expression names is a node");
    let p = profile("<r/>", "let $f := function($a) { $a + 1 } return $f(1) + $f(2) + $f(3)", &Options::default(), &Limits::default());
    assert_eq!(p.result.as_ref().expect("runs").items[0].text, "9");
    assert_eq!(node(&p, "$a + 1").hits, 3, "the body of the function is evaluated once per call");
}

#[test]
fn a_run_over_its_limit_is_stopped_and_says_so() {
    let p = profile("<r/>", "count((1 to 1000000) ! (. * 2))", &Options::default(), &Limits { max_instructions: 10_000 });
    assert!(p.stopped);
    let error = p.result.as_ref().expect_err("stopped");
    assert!(error.message.contains("stopped"), "{error}");
    assert!(p.instructions <= 10_001);
    assert!(!p.nodes.is_empty(), "what was counted until then is kept");
}

#[test]
fn errors_come_back_in_the_profile() {
    let p = profile("<r/>", "//item[", &Options::default(), &Limits::default());
    assert_eq!(p.result.as_ref().expect_err("does not compile").code, "XPST0003");
    assert!(p.nodes.is_empty());
    let p = profile("<r/>", "1 div 0", &Options::default(), &Limits::default());
    assert_eq!(p.result.as_ref().expect_err("fails").code, "FOAR0001");
    let p = profile("not xml", "1", &Options::default(), &Limits::default());
    assert!(p.result.is_err());
}

#[test]
fn a_profiled_run_gives_the_same_result_as_an_evaluation() {
    let doc = shop(30);
    for e in ["//item[price > 10]/name", "string-join(//name, ',')", "sort(//price ! number(), (), function($p) { -$p })[1]", "(//item)[last()]/name"]
    {
        let evaluated = xmlcraft_query::evaluate(&doc, e, &Options::default()).expect("runs");
        let profiled = profile(&doc, e, &Options::default(), &Limits::default());
        assert_eq!(profiled.result.as_ref().expect("runs"), &evaluated, "{e}");
    }
}

#[test]
fn a_normal_evaluation_is_not_slowed_by_the_hook() {
    // A generous bound: a profiled run stops before every instruction and a normal one checks one `Option`, so the normal one is the
    // faster. (The fastest of five runs each, on a document read once: the reading is the same for both.)
    let mut prepared = xmlcraft_query::Prepared::new(&shop(3000)).expect("reads");
    let e = "count(//item[price mod 3 = 0][name])";
    let fastest = |run: &mut dyn FnMut()| {
        (0..5)
            .map(|_| {
                let started = std::time::Instant::now();
                run();
                started.elapsed()
            })
            .min()
            .unwrap_or_default()
    };
    let normal = fastest(&mut || {
        prepared.evaluate(e, &Options::default()).expect("runs");
    });
    let profiled = fastest(&mut || {
        let _ = prepared.profile(e, &Options::default(), &Limits::default());
    });
    assert!(normal < profiled, "normal {normal:?}, profiled {profiled:?}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]
    #[test]
    fn any_expression_is_profiled_without_a_panic(xml in ".{0,40}", expression in ".{0,40}") {
        let p = profile(&xml, &expression, &Options::default(), &Limits { max_instructions: 100_000 });
        prop_assert!(p.instructions <= 100_001);
        for n in &p.nodes {
            prop_assert!(n.total_instructions >= n.self_instructions);
            prop_assert!(n.span.end <= expression.len());
        }
    }

    #[test]
    fn any_expression_on_a_document_is_profiled_without_a_panic(expression in "[a-z/\\[\\]()@*.,=<> 0-9$!|+-]{0,30}") {
        let _ = profile(&shop(3), &expression, &Options::default(), &Limits { max_instructions: 100_000 });
    }
}

/// The overhead, measured (`cargo test --release -p xmlcraft-query --test profile -- --ignored --nocapture`): the fastest of seven
/// normal evaluations and of seven profiled runs of a predicate over 20,000 items. The numbers are in docs/target-app-parity.md.
#[test]
#[ignore = "a measurement, run by hand in release"]
fn overhead() {
    let mut prepared = xmlcraft_query::Prepared::new(&shop(20_000)).expect("reads");
    let e = "count(//item[price mod 3 = 0][name])";
    let fastest = |run: &mut dyn FnMut()| {
        (0..7)
            .map(|_| {
                let started = std::time::Instant::now();
                run();
                started.elapsed()
            })
            .min()
            .unwrap_or_default()
    };
    let normal = fastest(&mut || {
        prepared.evaluate(e, &Options::default()).expect("runs");
    });
    let mut instructions = 0;
    let profiled = fastest(&mut || {
        instructions = prepared.profile(e, &Options::default(), &Limits::default()).instructions;
    });
    println!("overhead: normal {normal:?}, profiled {profiled:?}, {instructions} instructions");
}
