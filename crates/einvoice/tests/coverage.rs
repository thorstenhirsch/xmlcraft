//! How much of the bundled e-invoice rule sets the engine evaluates, measured: per rule set, how many assertions there are, how many
//! compile, and how they come out on the synthetic fixtures (passed, failed, not fired, not evaluated), with the reasons for what is
//! not evaluated. Run it with `cargo test -p xmlcraft-einvoice --test coverage -- --ignored --nocapture`; the numbers go into
//! `docs/target-app-parity.md` and `docs/gaps.md`.

use std::collections::BTreeMap;

use xmlcraft_einvoice::validate::{RULE_SETS, bundled_rules};
use xmlcraft_schematron::CheckState;

const UBL: &str = include_str!("fixtures/xrechnung-ubl.xml");
const CII: &str = include_str!("fixtures/xrechnung-cii.xml");

/// A reason in a few words, so that like reasons are counted together.
fn kind_of(reason: &str) -> String {
    let r = reason.to_lowercase();
    if r.contains("xpst0017") {
        "a function the engine does not have (XPST0017)".into()
    } else if r.contains("xpst0008") {
        "a variable that is not declared (XPST0008)".into()
    } else if r.contains("xpst0003") {
        "a syntax error (XPST0003)".into()
    } else if r.contains("earlier rule") {
        "after a rule of the pattern whose context failed".into()
    } else {
        reason.chars().take(90).collect()
    }
}

#[test]
#[ignore = "a measurement, not a check: run with --ignored --nocapture"]
fn measure_the_rule_sets() {
    for (name, path) in RULE_SETS {
        let schema = bundled_rules(path).expect("the rule set reads");
        let (rules, checks) = schema.size();
        let compiled = schema.compile_checks(Some("#ALL")).expect("compiles");
        let compiling = compiled.iter().filter(|c| c.outcome.is_ok()).count();
        let mut why: BTreeMap<String, usize> = BTreeMap::new();
        for c in compiled.iter().filter_map(|c| c.outcome.as_ref().err()) {
            *why.entry(kind_of(c)).or_default() += 1;
        }
        let untranslatable: Vec<_> = schema.functions().into_iter().filter(|(_, w)| w.is_some()).collect();
        println!("== {name} ({path})");
        println!("   rules {rules}, assertions {checks}, compile {compiling}, do not compile {}", checks - compiling);
        println!("   functions {} (untranslatable: {untranslatable:?})", schema.functions().len());
        for (k, n) in &why {
            println!("   not compiled: {n} x {k}");
        }
        for c in compiled.iter().filter(|c| c.outcome.is_err()).take(8) {
            println!("     e.g. {:?}: {}", c.id, c.outcome.as_ref().err().map(|e| e.chars().take(200).collect::<String>()).unwrap_or_default());
        }
        let fixture = if path.contains("ubl") { UBL } else { CII };
        let started = std::time::Instant::now();
        let report = schema.validate(fixture, Some("#ALL")).expect("validates");
        let t = report.totals();
        println!(
            "   on the fixture ({:?}): checks {}, fired {}, passed {}, failed {}, not fired {}, not evaluated {}",
            started.elapsed(),
            t.checks,
            t.passed + t.failed + report.outcomes.iter().filter(|o| matches!(o.state, CheckState::NotEvaluated(_))).count(),
            t.passed,
            t.failed,
            t.not_fired,
            t.not_evaluated
        );
        let mut why: BTreeMap<String, usize> = BTreeMap::new();
        for o in &report.outcomes {
            if let CheckState::NotEvaluated(r) = &o.state {
                *why.entry(kind_of(r)).or_default() += 1;
            }
        }
        for (k, n) in &why {
            println!("   not evaluated: {n} x {k}");
        }
        for f in report.findings.iter().take(20) {
            println!("   finding {:?} {:?} at {:?}: {}", f.id, f.flag, f.path, f.message.chars().take(140).collect::<String>());
        }
    }
}
