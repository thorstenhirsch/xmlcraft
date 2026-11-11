//! The validation of an electronic invoice, stage by stage, against what ships with XmlCraft:
//!
//! 1. **well-formedness** (the document must be read to go on);
//! 2. **detection**: the syntax (UBL, CII, FatturaPA) and the specification the document claims (`CustomizationID`, the guideline
//!    parameter of CII);
//! 3. the **XML Schema** of the syntax (the bundled UBL 2.1 or CII D16B schemas);
//! 4. the **EN 16931** business rules for the syntax (the CEN Schematron);
//! 5. the rules of the **CIUS** the document claims: XRechnung (the KoSIT Schematron). Peppol BIS Billing 3 is recognised but its rules
//!    are not bundled; FatturaPA has an XML Schema only, which is not bundled either (AGENTS.md section 1.6).
//!
//! A stage runs when the stages before it did not fail fatally: problems of the XML Schema do not stop the rules, a document that cannot
//! be read stops everything. Nothing is fetched: the schemas and rules are the bundled ones, read through the resolver the caller gives
//! (which should be offline). Every check of a rule set comes out as passed, failed, not fired, or **not evaluated** with the reason
//! (`xmlcraft_schematron::CheckState`); a check that was not evaluated is counted on its own and never as passed or failed.

use std::ops::Range;

use xmlcraft_resolve::Resolver;
use xmlcraft_schematron::{CheckKind, CheckState, Limits, Schema, Severity as RuleSeverity, Totals};
use xmlcraft_syntax::{Severity as SyntaxSeverity, parse};
use xmlcraft_text::Buffer;
use xmlcraft_xsd::ResolvingLoader;

use crate::{Detection, Standard, Syntax, bundled_schema, detect};

/// The most findings a report lists by default (the rest are counted).
pub const MAX_FINDINGS: usize = 1_000;
/// The most checks that were not evaluated a report lists (the rest are counted).
pub const MAX_NOT_EVALUATED: usize = 1_000;
/// The largest document that is validated.
pub const MAX_BYTES: usize = 32 * 1024 * 1024;

/// The version of the CEN artefacts that ship, and of the XRechnung rules.
pub const EN16931_VERSION: &str = "1.3.17";
pub const XRECHNUNG_RULES_VERSION: &str = "2.6.0 (XRechnung 3.0.2)";

/// A stage of the validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    WellFormedness,
    Detection,
    Schema,
    En16931,
    Cius,
}

impl Stage {
    pub const ALL: [Stage; 5] = [Stage::WellFormedness, Stage::Detection, Stage::Schema, Stage::En16931, Stage::Cius];

    /// Its name in reports and in the arguments of the tool.
    pub fn key(self) -> &'static str {
        match self {
            Stage::WellFormedness => "well-formedness",
            Stage::Detection => "detection",
            Stage::Schema => "schema",
            Stage::En16931 => "en16931",
            Stage::Cius => "cius",
        }
    }

    pub fn from_key(key: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// How a stage came out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// It ran and found no fatal problem (there may be warnings).
    Passed,
    /// It ran and found fatal problems.
    Failed,
    /// It did not run, and why (an earlier stage failed fatally, there is nothing bundled for the document, it was not asked for).
    NotRun(String),
    /// It could not be carried out, and why (a rule set or a schema that could not be read).
    Error(String),
}

/// What one stage did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageReport {
    pub stage: Stage,
    pub status: Status,
    /// What was checked against, in words (`EN 16931 1.3.17, UBL`, `UBL 2.1 Invoice`).
    pub against: Option<String>,
    /// For a stage of rules, how its checks came out.
    pub totals: Option<Totals>,
    /// How many findings it had (all of them, listed or not).
    pub findings: usize,
}

/// How serious a finding is: as the rule's `flag` says (`fatal`, `warning`, `information`), and fatal for the problems of the earlier
/// stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Information,
    Warning,
    Fatal,
}

impl Severity {
    pub fn key(self) -> &'static str {
        match self {
            Severity::Information => "information",
            Severity::Warning => "warning",
            Severity::Fatal => "fatal",
        }
    }
}

/// One finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub stage: Stage,
    /// The rule's id (`BR-CO-15`, `BR-DE-1`); for the XML Schema the code of the problem.
    pub rule: Option<String>,
    pub severity: Severity,
    pub message: String,
    /// Where: the byte range of the node the rule fired on (or of the problem), and its line and column (1-based; the column counts
    /// characters).
    pub span: Range<usize>,
    pub line: usize,
    pub column: usize,
    /// The node, as `fn:path` writes it, for a rule.
    pub path: Option<String>,
    /// The rule's test expression and the context of its rule.
    pub test: Option<String>,
    pub context: Option<String>,
}

/// A check that could not be evaluated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotEvaluated {
    pub stage: Stage,
    pub rule: Option<String>,
    pub test: String,
    pub context: String,
    pub reason: String,
}

/// What came of the validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// What the document is, when it was recognised.
    pub detection: Option<Detection>,
    pub stages: Vec<StageReport>,
    /// The findings, in the order of the stages and, within one, of the rules.
    pub findings: Vec<Finding>,
    /// How many findings there are (more than `findings.len()` when the list was cut).
    pub findings_total: usize,
    /// The checks of all the rule stages that ran.
    pub totals: Totals,
    pub not_evaluated: Vec<NotEvaluated>,
    pub not_evaluated_total: usize,
}

impl Report {
    /// Whether no stage that ran failed (stages that could not be carried out are not a pass: see [`Status::Error`]).
    pub fn valid(&self) -> bool {
        self.stages.iter().all(|s| matches!(s.status, Status::Passed | Status::NotRun(_)))
            && self.stages.iter().any(|s| s.stage == Stage::Detection && s.status == Status::Passed)
    }

    pub fn stage(&self, stage: Stage) -> Option<&StageReport> {
        self.stages.iter().find(|s| s.stage == stage)
    }
}

/// How to validate.
pub struct Options<'a> {
    /// The stages to run (all when `None`). Well-formedness and detection always run: the others depend on them.
    pub stages: Option<Vec<Stage>>,
    pub max_findings: usize,
    /// Asked between expressions: `true` stops the rules (a time budget the caller keeps; the checks not reached are not evaluated).
    pub stop: Option<&'a dyn Fn() -> bool>,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self { stages: None, max_findings: MAX_FINDINGS, stop: None }
    }
}

/// The bundled rule set for a syntax and a stage: its main file and what it is called.
fn rule_set(stage: Stage, syntax: Syntax) -> Option<(&'static str, String)> {
    match (stage, syntax) {
        (Stage::En16931, Syntax::Ubl) => Some(("en16931/ubl/EN16931-UBL-validation.sch", format!("EN 16931 {EN16931_VERSION}, UBL"))),
        (Stage::En16931, Syntax::Cii) => Some(("en16931/cii/EN16931-CII-validation.sch", format!("EN 16931 {EN16931_VERSION}, CII"))),
        (Stage::Cius, Syntax::Ubl) => {
            Some(("xrechnung/ubl/XRechnung-UBL-validation.sch", format!("XRechnung Schematron {XRECHNUNG_RULES_VERSION}, UBL")))
        }
        (Stage::Cius, Syntax::Cii) => {
            Some(("xrechnung/cii/XRechnung-CII-validation.sch", format!("XRechnung Schematron {XRECHNUNG_RULES_VERSION}, CII")))
        }
        _ => None,
    }
}

/// Read a bundled rule set (its includes from the bundle too).
pub fn bundled_rules(path: &str) -> Result<Schema, String> {
    let text = xmlcraft_schemas::get_text(path).ok_or_else(|| format!("the rule set {path} is not bundled"))?;
    Schema::parse_located(&text, path, &|href| xmlcraft_schemas::get_text(href)).map_err(|e| format!("the rule set {path} cannot be read: {e}"))
}

/// The bundled rule sets, by name, with their main files: for the measurement of what the engine evaluates.
pub const RULE_SETS: &[(&str, &str)] = &[
    ("EN 16931 UBL", "en16931/ubl/EN16931-UBL-validation.sch"),
    ("EN 16931 CII", "en16931/cii/EN16931-CII-validation.sch"),
    ("XRechnung UBL", "xrechnung/ubl/XRechnung-UBL-validation.sch"),
    ("XRechnung CII", "xrechnung/cii/XRechnung-CII-validation.sch"),
];

/// Why the EN 16931 rules do not apply to what the document claims, if they do not.
fn en16931_not_applicable(d: &Detection) -> Option<String> {
    match d.standard {
        Standard::En16931 | Standard::XRechnung | Standard::XRechnungExtension | Standard::PeppolBis | Standard::Unspecified => None,
        Standard::FacturX | Standard::Zugferd2 => match d.profile.as_deref() {
            Some("EN 16931" | "basic" | "extended") => None,
            Some(p) => Some(format!("the {p} profile of {} is not an EN 16931 invoice", d.standard.name())),
            None => Some(format!("the profile of {} is not known", d.standard.name())),
        },
        other => Some(format!("{} is not a syntax binding of EN 16931", other.name())),
    }
}

/// Why the CIUS stage does not run for the document, if it does not.
fn cius_not_applicable(d: &Detection) -> Option<String> {
    match d.standard {
        Standard::XRechnung | Standard::XRechnungExtension => match d.version.as_deref() {
            None => None,
            Some(v) if v == "3.0" || v.starts_with("3.0.") => None,
            Some(v) => Some(format!("the document claims XRechnung {v}; the bundled rules are for XRechnung 3.0")),
        },
        Standard::PeppolBis => {
            Some("the Peppol BIS Billing 3 rules are not bundled: OpenPeppol's terms do not allow them to be redistributed".to_owned())
        }
        Standard::FatturaPa => Some("FatturaPA has no rules beyond its XML Schema".to_owned()),
        other => Some(format!("{} has no CIUS rules that ship with XmlCraft", other.name())),
    }
}

struct Collector<'t> {
    buffer: Buffer,
    text: &'t str,
    max_findings: usize,
    findings: Vec<Finding>,
    findings_total: usize,
    not_evaluated: Vec<NotEvaluated>,
    not_evaluated_total: usize,
}

impl Collector<'_> {
    fn place(&self, span: &Range<usize>) -> (Range<usize>, usize, usize) {
        let start = span.start.min(self.text.len());
        let end = span.end.clamp(start, self.text.len());
        let p = self.buffer.position(start);
        (start..end, p.line.saturating_add(1), p.col.saturating_add(1))
    }

    fn push(&mut self, finding: Finding) {
        self.findings_total = self.findings_total.saturating_add(1);
        if self.findings.len() < self.max_findings {
            self.findings.push(finding);
        }
    }
}

/// Validate the e-invoice `text` (at `location`, which relative references are taken from; empty when it has none). `resolver` finds
/// the documents of the bundled XML Schema; give an offline one that knows the bundle.
pub fn validate(text: &str, location: &str, resolver: &Resolver, options: &Options<'_>) -> Report {
    let wanted = |stage: Stage| options.stages.as_ref().is_none_or(|s| s.contains(&stage));
    let mut c = Collector {
        buffer: Buffer::from_text(text),
        text,
        max_findings: options.max_findings,
        findings: Vec::new(),
        findings_total: 0,
        not_evaluated: Vec::new(),
        not_evaluated_total: 0,
    };
    let mut stages: Vec<StageReport> = Vec::new();
    let mut totals = Totals::default();
    let not_run = |stage: Stage, why: &str| StageReport { stage, status: Status::NotRun(why.to_owned()), against: None, totals: None, findings: 0 };
    let finish = |c: Collector<'_>, stages: Vec<StageReport>, totals: Totals, detection: Option<Detection>| Report {
        detection,
        stages,
        findings: c.findings,
        findings_total: c.findings_total,
        totals,
        not_evaluated: c.not_evaluated,
        not_evaluated_total: c.not_evaluated_total,
    };
    let rest_not_run = |stages: &mut Vec<StageReport>, from: usize, why: &str| {
        for stage in Stage::ALL.into_iter().skip(from) {
            stages.push(not_run(stage, why));
        }
    };

    // 1. Well-formedness.
    if text.len() > MAX_BYTES {
        stages.push(StageReport {
            stage: Stage::WellFormedness,
            status: Status::Error(format!("the document is larger than {MAX_BYTES} bytes")),
            against: None,
            totals: None,
            findings: 0,
        });
        rest_not_run(&mut stages, 1, "the document is too large to be validated");
        return finish(c, stages, totals, None);
    }
    let parsed = parse(text);
    let errors: Vec<_> = parsed.diagnostics().iter().filter(|d| d.severity() == SyntaxSeverity::Error).cloned().collect();
    for d in &errors {
        let (span, line, column) = c.place(&d.span);
        c.push(Finding {
            stage: Stage::WellFormedness,
            rule: Some(d.code.key().to_owned()),
            severity: Severity::Fatal,
            message: d.message(),
            span,
            line,
            column,
            path: None,
            test: None,
            context: None,
        });
    }
    let well_formed = errors.is_empty();
    stages.push(StageReport {
        stage: Stage::WellFormedness,
        status: if well_formed { Status::Passed } else { Status::Failed },
        against: None,
        totals: None,
        findings: errors.len(),
    });
    if !well_formed {
        rest_not_run(&mut stages, 1, "the document is not well-formed XML");
        return finish(c, stages, totals, None);
    }

    // 2. Detection.
    let Some(detection) = detect(text) else {
        stages.push(StageReport { stage: Stage::Detection, status: Status::Failed, against: None, totals: None, findings: 0 });
        rest_not_run(&mut stages, 2, "the document is not a known electronic invoice (UBL, CII or FatturaPA)");
        return finish(c, stages, totals, None);
    };
    stages.push(StageReport {
        stage: Stage::Detection,
        status: Status::Passed,
        against: Some(match &detection.profile {
            Some(p) => format!("{} ({p}), {}", detection.standard.name(), detection.document),
            None => format!("{}, {}", detection.standard.name(), detection.document),
        }),
        totals: None,
        findings: 0,
    });
    let is_invoice = matches!((detection.syntax, detection.document.as_str()), (Syntax::Ubl, "Invoice" | "Credit note") | (Syntax::Cii, "Invoice"));

    // 3. The XML Schema of the syntax.
    if !wanted(Stage::Schema) {
        stages.push(not_run(Stage::Schema, "not asked for"));
    } else {
        match bundled_schema(&detection) {
            None => {
                let why = match detection.syntax {
                    Syntax::FatturaPa => "the FatturaPA schema is not bundled: its publisher's terms do not allow it to be redistributed".to_owned(),
                    _ => format!("no XML Schema ships for {} {}", detection.standard.name(), detection.document),
                };
                stages.push(not_run(Stage::Schema, &why));
            }
            Some(path) => stages.push(schema_stage(&parsed, location, path, resolver, &mut c)),
        }
    }

    // 4. EN 16931, 5. the CIUS.
    for stage in [Stage::En16931, Stage::Cius] {
        if !wanted(stage) {
            stages.push(not_run(stage, "not asked for"));
            continue;
        }
        let why = if !is_invoice {
            Some(format!("there are no rules for a {} {}", syntax_name(detection.syntax), detection.document.to_lowercase()))
        } else if stage == Stage::En16931 {
            en16931_not_applicable(&detection)
        } else {
            cius_not_applicable(&detection)
        };
        if let Some(why) = why {
            stages.push(not_run(stage, &why));
            continue;
        }
        let Some((path, name)) = rule_set(stage, detection.syntax) else {
            stages.push(not_run(stage, "there are no rules for this syntax"));
            continue;
        };
        let report = rules_stage(stage, path, name, text, options, &mut c);
        if let Some(t) = &report.totals {
            totals.checks += t.checks;
            totals.passed += t.passed;
            totals.failed += t.failed;
            totals.not_evaluated += t.not_evaluated;
            totals.not_fired += t.not_fired;
        }
        stages.push(report);
    }
    finish(c, stages, totals, Some(detection))
}

fn syntax_name(syntax: Syntax) -> &'static str {
    match syntax {
        Syntax::Ubl => "UBL",
        Syntax::Cii => "CII",
        Syntax::CiiLegacy => "ZUGFeRD 1.0",
        Syntax::FatturaPa => "FatturaPA",
        Syntax::OrderX => "Order-X",
    }
}

fn schema_stage(parsed: &xmlcraft_syntax::Parse, location: &str, path: &str, resolver: &Resolver, c: &mut Collector<'_>) -> StageReport {
    let mut loader = ResolvingLoader::new(resolver);
    let uri = xmlcraft_schemas::uri_of(path);
    let root = parsed.syntax();
    let found = xmlcraft_validate::check_against_schema(&root, parsed.diagnostics(), location, Some(&uri), &mut loader);
    let against = Some(path.rsplit('/').next().unwrap_or(path).to_owned());
    match &found.outcome {
        xmlcraft_validate::SchemaOutcome::NotWellFormed => StageReport {
            stage: Stage::Schema,
            status: Status::NotRun("the document is not well-formed XML".into()),
            against,
            totals: None,
            findings: 0,
        },
        xmlcraft_validate::SchemaOutcome::NoSchema(why) => {
            StageReport { stage: Stage::Schema, status: Status::Error(why.clone()), against, totals: None, findings: 0 }
        }
        xmlcraft_validate::SchemaOutcome::Validated => {
            let mut count = 0usize;
            let mut fatal = false;
            for d in &found.validity {
                let severity = match d.severity() {
                    SyntaxSeverity::Error => Severity::Fatal,
                    SyntaxSeverity::Warning => Severity::Warning,
                    SyntaxSeverity::Info => Severity::Information,
                };
                fatal |= severity == Severity::Fatal;
                count += 1;
                let (span, line, column) = c.place(&d.span);
                c.push(Finding {
                    stage: Stage::Schema,
                    rule: Some(d.code.key().to_owned()),
                    severity,
                    message: d.message(),
                    span,
                    line,
                    column,
                    path: None,
                    test: None,
                    context: None,
                });
            }
            // A problem of the bundled schema itself would be ours: it makes the stage an error, not a verdict on the document.
            if let Some(p) = found.schema_problems.iter().find(|p| p.diagnostic.severity() == SyntaxSeverity::Error) {
                return StageReport {
                    stage: Stage::Schema,
                    status: Status::Error(format!("the bundled schema has a problem: {}", p.diagnostic.message())),
                    against,
                    totals: None,
                    findings: count,
                };
            }
            StageReport { stage: Stage::Schema, status: if fatal { Status::Failed } else { Status::Passed }, against, totals: None, findings: count }
        }
    }
}

fn rules_stage(stage: Stage, path: &str, name: String, text: &str, options: &Options<'_>, c: &mut Collector<'_>) -> StageReport {
    let schema = match bundled_rules(path) {
        Ok(s) => s,
        Err(why) => return StageReport { stage, status: Status::Error(why), against: Some(name), totals: None, findings: 0 },
    };
    let limits = Limits { max_findings: options.max_findings.saturating_sub(c.findings.len()), stop: options.stop };
    let report = match schema.validate_with(text, Some("#ALL"), &limits) {
        Ok(r) => r,
        Err(e) => return StageReport { stage, status: Status::Error(e.message), against: Some(name), totals: None, findings: 0 },
    };
    let mut fatal = false;
    for f in &report.findings {
        let severity = match (f.severity, f.flag.as_deref().map(str::to_ascii_lowercase).as_deref()) {
            (_, Some("fatal")) => Severity::Fatal,
            (RuleSeverity::Error, _) => Severity::Fatal,
            (RuleSeverity::Warning, _) => Severity::Warning,
            (RuleSeverity::Info, _) => Severity::Information,
        };
        fatal |= severity == Severity::Fatal;
        let (span, line, column) = c.place(&f.span.clone().unwrap_or(0..0));
        c.push(Finding {
            stage,
            rule: f.id.clone(),
            severity,
            message: f.message.clone(),
            span,
            line,
            column,
            path: f.path.clone(),
            test: Some(f.test.clone()),
            context: Some(f.context.clone()),
        });
    }
    // The findings past the list are counted, and one fatal among them fails the stage too.
    let unlisted = report.count.saturating_sub(report.findings.len());
    c.findings_total = c.findings_total.saturating_add(unlisted);
    let failed_fatal = report.outcomes.iter().any(|o| {
        matches!(o.state, CheckState::Failed(_))
            && (o.flag.as_deref().is_some_and(|f| f.eq_ignore_ascii_case("fatal") || f.eq_ignore_ascii_case("error"))
                || (o.flag.is_none() && o.kind == CheckKind::Assert))
    });
    for o in &report.outcomes {
        if let CheckState::NotEvaluated(reason) = &o.state {
            c.not_evaluated_total = c.not_evaluated_total.saturating_add(1);
            if c.not_evaluated.len() < MAX_NOT_EVALUATED {
                c.not_evaluated.push(NotEvaluated {
                    stage,
                    rule: o.id.clone(),
                    test: o.test.clone(),
                    context: o.context.clone(),
                    reason: reason.clone(),
                });
            }
        }
    }
    StageReport {
        stage,
        status: if fatal || failed_fatal { Status::Failed } else { Status::Passed },
        against: Some(name),
        totals: Some(report.totals()),
        findings: report.count,
    }
}
