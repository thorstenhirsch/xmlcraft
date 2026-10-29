//! The loop of the two generators: a schema inferred from documents gives samples that are valid against it, whatever the documents.

use proptest::prelude::*;
use xmlcraft_dtd::{NoLoader as NoSubsets, read_doctype};
use xmlcraft_infer::{Layout, Options as InferOptions, Output, infer};
use xmlcraft_sample::{ChoicePolicy, Fill, Options, sample_from_dtd, sample_from_schema, schema_roots};
use xmlcraft_syntax::{Severity, parse};
use xmlcraft_validate::validate_against_schema;
use xmlcraft_xsd::{NoLoader, assemble, load};

#[derive(Clone, Debug)]
enum Node {
    Element(El),
    Text(&'static str),
}

#[derive(Clone, Debug)]
struct El {
    name: &'static str,
    attributes: Vec<(&'static str, &'static str)>,
    children: Vec<Node>,
}

fn element(depth: u32) -> BoxedStrategy<El> {
    let name = prop::sample::select(vec!["a", "b", "c", "d"]);
    let attributes = prop::collection::btree_map(
        prop::sample::select(vec!["x", "y", "z"]),
        prop::sample::select(vec!["1", "2", "x", "true", "2024-01-02", "", "a b", "1.5"]),
        0..3,
    )
    .prop_map(|m| m.into_iter().collect::<Vec<_>>());
    let text = prop::sample::select(vec!["t", "42", "", " ", "2024-01-02"]);
    let children: BoxedStrategy<Vec<Node>> = if depth == 0 {
        prop::collection::vec(text.clone().prop_map(Node::Text), 0..2).boxed()
    } else {
        prop::collection::vec(prop_oneof![3 => element(depth - 1).prop_map(Node::Element), 1 => text.prop_map(Node::Text)], 0..5).boxed()
    };
    (name, attributes, children).prop_map(|(name, attributes, children)| El { name, attributes, children }).boxed()
}

fn write(el: &El, out: &mut String) {
    out.push_str(&format!("<{}", el.name));
    for (k, v) in &el.attributes {
        out.push_str(&format!(" {k}=\"{v}\""));
    }
    out.push('>');
    for child in &el.children {
        match child {
            Node::Element(e) => write(e, out),
            Node::Text(t) => out.push_str(t),
        }
    }
    out.push_str(&format!("</{}>", el.name));
}

fn document() -> impl Strategy<Value = String> {
    element(3).prop_map(|e| {
        let mut s = String::new();
        write(&e, &mut s);
        s
    })
}

fn option_sets() -> Vec<Options> {
    vec![
        Options::default(),
        Options { optional: false, ..Options::default() },
        Options { repeat: 3, choice: ChoicePolicy::Rotate, fill: Fill::Named, max_depth: 2, ..Options::default() },
        Options { repeat: 0, max_depth: 0, ..Options::default() },
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// A schema inferred from documents gives a valid document, from any root, in every layout and with every option set.
    #[test]
    fn a_sample_of_an_inferred_xsd_is_valid(docs in prop::collection::vec(document(), 1..3)) {
        let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
        for layout in [Layout::RussianDoll, Layout::SalamiSlice, Layout::VenetianBlind] {
            let generated = infer(&refs, &InferOptions { output: Output::Xsd { layout }, ..InferOptions::default() }).expect("well-formed documents");
            let documents = load(&generated.files[0].text, "mem:schema.xsd", &mut NoLoader);
            let assembly = assemble(&documents);
            prop_assert!(documents.diagnostics.is_empty() && assembly.diagnostics.is_empty(), "{layout:?}\n{}", generated.files[0].text);
            for root in schema_roots(&assembly.schema) {
                for options in option_sets() {
                    let made = sample_from_schema(&assembly.schema, Some(&root), &options).expect("a sample");
                    let parsed = parse(&made.text);
                    let found: Vec<String> = parsed.diagnostics().iter().filter(|d| d.severity() == Severity::Error).map(|d| d.message()).chain(
                        validate_against_schema(&parsed.syntax(), &assembly.schema).iter().filter(|d| d.severity() == Severity::Error).map(|d| d.message())).collect();
                    prop_assert!(found.is_empty() && made.warnings.is_empty(), "{layout:?} {options:?}: {found:?} {:?}\n{}\n{}", made.warnings, generated.files[0].text, made.text);
                }
            }
        }
    }

    /// The same for the DTD.
    #[test]
    fn a_sample_of_an_inferred_dtd_is_valid(docs in prop::collection::vec(document(), 1..3)) {
        let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
        let generated = infer(&refs, &InferOptions { output: Output::Dtd, ..InferOptions::default() }).expect("well-formed documents");
        let dtd_text = &generated.files[0].text;
        let probe = format!("<!DOCTYPE r [\n{dtd_text}]><r/>");
        let parsed = parse(&probe);
        let (read, _) = read_doctype(&parsed.syntax(), &probe, "", &mut NoSubsets).expect("a DOCTYPE");
        for root in xmlcraft_sample::dtd_roots(&read.dtd) {
            for options in option_sets() {
                let made = sample_from_dtd(&read.dtd, Some(&root), &options).expect("a sample");
                let at = made.text.find("?>").map(|i| i + 3).unwrap_or(0);
                let text = format!("{}<!DOCTYPE {root} [\n{dtd_text}]>\n{}", &made.text[..at], &made.text[at..]);
                let parsed = parse(&text);
                let found = xmlcraft_validate::check(&parsed.syntax(), &text, parsed.diagnostics(), "", &mut NoSubsets);
                let problems: Vec<String> = found.well_formedness.iter().chain(&found.validity).filter(|p| p.diagnostic.severity() == Severity::Error).map(|p| p.diagnostic.message()).collect();
                prop_assert!(problems.is_empty() && made.warnings.is_empty(), "{options:?}: {problems:?} {:?}\n{dtd_text}\n{}", made.warnings, made.text);
            }
        }
    }
}
