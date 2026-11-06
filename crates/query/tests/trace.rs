//! The trace of a path: what each step leaves.

use xmlcraft_query::{Options, Prepared, trace};

const DOC: &str = "<shop><order id=\"1\"><item>a</item><item>b</item></order><order id=\"2\"><item>c</item></order><order id=\"5\"/></shop>";

fn steps(expression: &str) -> (bool, Vec<(String, usize)>) {
    let mut prepared = Prepared::new(DOC).expect("reads");
    let t = trace(&mut prepared, expression, &Options::default());
    (t.is_path, t.steps.into_iter().map(|s| (s.step, s.count)).collect())
}

#[test]
fn each_step_of_a_path_says_how_many_items_it_leaves() {
    let (is_path, got) = steps("/shop/order[@id < 5]/item");
    assert!(is_path);
    assert_eq!(got, [("/shop".to_owned(), 1), ("/order[@id < 5]".to_owned(), 2), ("/item".to_owned(), 3)]);
}

#[test]
fn a_double_slash_is_one_separator_and_slashes_inside_brackets_or_quotes_do_not_cut() {
    let (_, got) = steps("//item");
    assert_eq!(got, [("//item".to_owned(), 3)], "one step: nothing before the first separator");
    let (is_path, got) = steps("//order[item/text() = 'a/b' or @id = 5]/@id");
    assert!(is_path);
    assert_eq!(got, [("//order[item/text() = 'a/b' or @id = 5]".to_owned(), 1), ("/@id".to_owned(), 1)], "the slash inside the quotes does not cut");
}

#[test]
fn what_is_not_a_path_is_one_step_and_an_error_stops_the_trace() {
    for e in ["count(//item)", "//item | //order", "1 + 2", "for $i in //item return $i", "(//item)[1]", "//item = 'a'"] {
        let (is_path, got) = steps(e);
        assert!(!is_path || got.len() == 1, "{e}: {got:?}");
        assert_eq!(got.len(), 1, "{e}");
    }
    let mut prepared = Prepared::new(DOC).expect("reads");
    let t = trace(&mut prepared, "/shop/order/nope:item/x", &Options::default());
    let last = t.steps.last().expect("a step");
    assert!(last.error.is_some(), "an unknown prefix fails at its step: {last:?}");
    assert!(t.steps.len() <= 3 && t.steps.iter().take(t.steps.len() - 1).all(|s| s.error.is_none()));
}

proptest::proptest! {
    #[test]
    fn any_expression_is_traced_or_fails_in_a_step_without_a_panic(expression in ".{0,60}") {
        let mut prepared = Prepared::new(DOC).expect("reads");
        let t = trace(&mut prepared, &expression, &Options::default());
        assert!(!t.steps.is_empty());
    }
}
