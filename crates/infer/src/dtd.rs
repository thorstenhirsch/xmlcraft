//! The DTD writer. A DTD has no namespaces, so the names are the ones the documents wrote, and the namespace declarations are
//! attributes like the others.

use std::collections::BTreeSet;

use crate::content::{Factor, factors};
use crate::observe::{ElementStats, Name, Observed};
use crate::values::Kind;
use crate::{GeneratedFile, Options};

pub fn write(obs: &Observed, options: &Options) -> GeneratedFile {
    let mut text = String::new();
    // The roots first, in the order they were met, then the others by name.
    let mut order: Vec<&Name> = obs.roots.iter().collect();
    let listed: BTreeSet<&Name> = order.iter().copied().collect();
    order.extend(obs.elements.keys().filter(|n| !listed.contains(n)));
    for name in order {
        let Some(stats) = obs.elements.get(name) else { continue };
        text.push_str(&format!("<!ELEMENT {} {}>\n", name.local, content_spec(stats)));
        write_attributes(&mut text, name, stats, options);
    }
    GeneratedFile { name: "schema.dtd".to_owned(), namespace: None, text }
}

fn content_spec(stats: &ElementStats) -> String {
    if stats.with_children == 0 {
        return if stats.has_text() { "(#PCDATA)".to_owned() } else { "EMPTY".to_owned() };
    }
    let symbols: Vec<&str> = stats.children.symbols.iter().map(|n| n.local.as_str()).collect();
    if stats.mixed > 0 || stats.has_text() {
        return format!("(#PCDATA|{})*", symbols.join("|"));
    }
    let parts: Vec<String> = factors(&stats.children).iter().map(|f| factor(f, &symbols)).collect();
    format!("({})", parts.join(", "))
}

fn factor(f: &Factor, symbols: &[&str]) -> String {
    let names: Vec<&str> = f.symbols.iter().filter_map(|&i| symbols.get(i).copied()).collect();
    let body = if let [only] = names.as_slice() { (*only).to_owned() } else { format!("({})", names.join("|")) };
    let suffix = match (f.optional(), f.repeats()) {
        (false, false) => "",
        (true, false) => "?",
        (false, true) => "+",
        (true, true) => "*",
    };
    format!("{body}{suffix}")
}

fn write_attributes(text: &mut String, element: &Name, stats: &ElementStats, options: &Options) {
    if stats.attributes.is_empty() {
        return;
    }
    text.push_str(&format!("<!ATTLIST {}", element.local));
    for (name, attribute) in &stats.attributes {
        let required = attribute.count >= stats.instances && stats.instances > 0;
        let kind = if name.local == "xml:space" {
            // The one attribute whose type a valid document is not free to choose.
            "(default|preserve)".to_owned()
        } else {
            match options.enumerations.and_then(|rule| attribute.values.enumeration(rule)) {
                Some(values) if attribute.values.all_name_tokens(&values) && attribute.values.kind() != Kind::Boolean => {
                    format!("({})", values.join("|"))
                }
                _ => "CDATA".to_owned(),
            }
        };
        text.push_str(&format!("\n  {} {} {}", name.local, kind, if required { "#REQUIRED" } else { "#IMPLIED" }));
    }
    text.push_str(">\n");
}
