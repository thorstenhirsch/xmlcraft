//! Dependency layering rules (docs/architecture.md), enforced by `cargo xtask layers`.
//!
//! The rule engine works on a small, metadata-independent model so it can be unit-tested; `from_metadata` builds that model from
//! `cargo metadata`. (Structure shared with PdfCraft.)

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Regular layered crate.
    Layer(u8),
    /// Foundation crate that may depend only on the workspace crates listed in `STANDALONE_DEPS`.
    Standalone(u8),
    /// Test tooling: dev-dependency only for other crates.
    Testkit,
    /// Binaries, build tooling: exempt.
    Exempt,
}

impl Class {
    fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) | Class::Standalone(l) => Some(l),
            Class::Testkit => Some(7),
            Class::Exempt => None,
        }
    }
}

/// Every workspace crate (package name without the `xmlcraft-` prefix) and its layer.
pub const TABLE: &[(&str, Class)] = &[
    // L0 foundation (standalone)
    ("text", Class::Standalone(0)),
    ("pattern", Class::Standalone(0)),
    ("schemas", Class::Standalone(0)),
    // L1 syntax and datatypes (each builds on one L0 crate)
    ("syntax", Class::Standalone(1)),
    ("datatypes", Class::Standalone(1)),
    // L2 core model
    ("grammar", Class::Layer(2)),
    ("catalog", Class::Layer(2)),
    // L3 core services
    ("dtd", Class::Layer(3)),
    ("xsd", Class::Layer(3)),
    ("resolve", Class::Layer(3)),
    ("json", Class::Layer(3)),
    // L4 features
    ("validate", Class::Layer(4)),
    ("infer", Class::Layer(4)),
    ("sample", Class::Layer(4)),
    ("format", Class::Layer(4)),
    ("complete", Class::Layer(4)),
    // L5 interaction
    ("editor", Class::Layer(5)),
    // L6 façade
    ("engine", Class::Layer(6)),
    // L7 platform + frontends
    ("platform", Class::Layer(7)),
    ("code-editor", Class::Layer(7)),
    ("ui-egui", Class::Layer(7)),
    ("automation", Class::Layer(7)),
    // test support
    ("testkit", Class::Testkit),
    ("oracle", Class::Testkit),
    // L8 apps + tooling
    ("xmlcraft", Class::Exempt),
    ("cli", Class::Exempt),
    ("xtask", Class::Exempt),
];

/// Workspace crates a standalone crate may use: `datatypes` compiles XSD patterns, and `syntax` sits on the text buffer.
pub const STANDALONE_DEPS: &[(&str, &[&str])] = &[("datatypes", &["pattern"]), ("syntax", &["text"])];

/// Allowed same-layer edges.
pub const SIDEWAYS: &[(&str, &str)] = &[
    // The schema readers load included and imported documents through the resolver's traits.
    ("dtd", "resolve"),
    ("xsd", "resolve"),
    ("json", "resolve"),
    // Completion asks the validator for the context at the cursor.
    ("complete", "validate"),
    // Generators write their output through the formatter, and their tests validate it.
    ("infer", "format"),
    ("sample", "format"),
    ("infer", "validate"),
    ("sample", "validate"),
    // The tests of the sample generator run it on the schemas that inference writes.
    ("sample", "infer"),
    // The shell hosts the editor widget and the platform services.
    ("ui-egui", "code-editor"),
    ("ui-egui", "platform"),
    ("automation", "platform"),
];

/// External crates that constitute a UI toolkit / windowing dependency (prefix match with `*`).
pub const UI_CRATES: &[&str] = &["egui", "eframe", "winit", "egui_kittest", "craft-ui", "egui_extras", "egui_dock", "egui_tiles", "rfd", "wgpu*"];
pub const UI_MIN_LAYER: u8 = 7;

pub fn short_name(pkg: &str) -> &str {
    pkg.strip_prefix("xmlcraft-").unwrap_or(pkg)
}

pub fn classify(pkg: &str) -> Option<Class> {
    let s = short_name(pkg);
    TABLE.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
}

fn is_ui_crate(name: &str) -> bool {
    UI_CRATES.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

fn sideways_allowed(from: &str, to: &str) -> bool {
    SIDEWAYS.contains(&(short_name(from), short_name(to)))
}

fn standalone_allowed(from: &str, to: &str) -> bool {
    STANDALONE_DEPS.iter().any(|(f, ok)| *f == short_name(from) && ok.contains(&short_name(to)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
    pub workspace: bool,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unregistered { krate: String },
    Upward { krate: String, dep: String, from: u8, to: u8, kind: DepKind },
    StandaloneHasWorkspaceDep { krate: String, dep: String },
    TestkitAsNormalDep { krate: String, dep: String },
    UiBelowL7 { krate: String, dep: String, layer: u8 },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered { krate } => {
                write!(f, "{krate}: unknown workspace crate; register it in xtask/src/layers.rs TABLE (docs/architecture.md)")
            }
            Violation::Upward { krate, dep, from, to, kind } => {
                write!(f, "{krate} (L{from}) -> {dep} (L{to}) [{kind:?}]: may only depend on lower layers (or an allowed sideways edge)")
            }
            Violation::StandaloneHasWorkspaceDep { krate, dep } => write!(f, "{krate}: standalone crate must not depend on workspace crate {dep}"),
            Violation::TestkitAsNormalDep { krate, dep } => write!(f, "{krate}: {dep} may only be a dev-dependency"),
            Violation::UiBelowL7 { krate, dep, layer } => {
                write!(f, "{krate} (L{layer}) depends on UI crate `{dep}`; UI toolkits are only allowed in L7+")
            }
        }
    }
}

/// Layer and testkit rules for one dependency of `c` that is itself a workspace crate.
fn check_workspace_dep(c: &Crate, class: Class, layer: u8, d: &Dep, out: &mut Vec<Violation>) {
    let dep_class = classify(&d.name);
    // Test support is a dev-dependency only (`Class::Testkit`) and so never reaches the product graph a standalone crate
    // publishes. Checking it against the standalone allow-list would stop a foundation crate using the shared fixtures in its
    // own tests, which is the one thing those crates exist for.
    let testkit_dev = dep_class == Some(Class::Testkit) && d.kind == DepKind::Dev;
    if matches!(class, Class::Standalone(_)) && !testkit_dev && !standalone_allowed(&c.name, &d.name) {
        out.push(Violation::StandaloneHasWorkspaceDep { krate: c.name.clone(), dep: d.name.clone() });
        return;
    }
    match dep_class {
        None => {}
        Some(Class::Testkit) if d.kind != DepKind::Dev && class != Class::Testkit => {
            out.push(Violation::TestkitAsNormalDep { krate: c.name.clone(), dep: d.name.clone() });
        }
        Some(Class::Testkit) => {}
        Some(dc) => {
            let to = dc.layer().unwrap_or(u8::MAX);
            let ok = to < layer || (to == layer && sideways_allowed(&c.name, &d.name));
            if !ok {
                out.push(Violation::Upward { krate: c.name.clone(), dep: d.name.clone(), from: layer, to: to.min(8), kind: d.kind });
            }
        }
    }
}

pub fn check(crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates {
        let Some(class) = classify(&c.name) else {
            out.push(Violation::Unregistered { krate: c.name.clone() });
            continue;
        };
        if class == Class::Exempt {
            continue;
        }
        let layer = class.layer().unwrap_or(0);
        for d in &c.deps {
            if d.name == c.name {
                continue;
            }
            if d.workspace {
                check_workspace_dep(c, class, layer, d, &mut out);
            } else if layer < UI_MIN_LAYER && is_ui_crate(&d.name) && !(class == Class::Testkit || d.kind == DepKind::Dev && d.name == "egui_kittest")
            {
                out.push(Violation::UiBelowL7 { krate: c.name.clone(), dep: d.name.clone(), layer });
            }
        }
    }
    out.sort_by_key(ToString::to_string);
    out.dedup();
    out
}

pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let members: Vec<&str> = pkgs.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in pkgs {
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let mut deps = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let dname = d["name"].as_str().unwrap_or_default().to_owned();
            let kind = match d["kind"].as_str() {
                Some("dev") => DepKind::Dev,
                Some("build") => DepKind::Build,
                _ => DepKind::Normal,
            };
            let workspace = members.contains(&dname.as_str());
            deps.push(Dep { name: dname, kind, workspace });
        }
        out.push(Crate { name, deps });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn describe(class: Option<Class>) -> String {
    match class {
        Some(Class::Layer(l)) => format!("L{l}"),
        Some(Class::Standalone(l)) => format!("L{l} standalone"),
        Some(Class::Testkit) => "test support".into(),
        Some(Class::Exempt) => "exempt".into(),
        None => "UNREGISTERED".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::DepKind::*;
    use super::*;

    fn c(name: &str, deps: &[(&str, DepKind, bool)]) -> Crate {
        Crate { name: name.into(), deps: deps.iter().map(|(n, k, w)| Dep { name: (*n).into(), kind: *k, workspace: *w }).collect() }
    }

    #[test]
    fn current_graph_shape_passes() {
        let g = [
            c("xmlcraft-text", &[("crop", Normal, false)]),
            c("xmlcraft-syntax", &[("xmlcraft-text", Normal, true), ("rowan", Normal, false)]),
            c("xmlcraft-grammar", &[("xmlcraft-datatypes", Normal, true), ("xmlcraft-syntax", Normal, true)]),
            c("xmlcraft-engine", &[("xmlcraft-grammar", Normal, true)]),
            c("xmlcraft-ui-egui", &[("xmlcraft-engine", Normal, true), ("egui", Normal, false), ("egui_kittest", Dev, false)]),
            c("xmlcraft", &[("xmlcraft-ui-egui", Normal, true)]),
        ];
        assert!(check(&g).is_empty(), "{:?}", check(&g));
    }

    #[test]
    fn every_table_entry_is_unique() {
        let mut names: Vec<&str> = TABLE.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "a crate is registered twice in TABLE");
    }

    #[test]
    fn every_sideways_and_standalone_edge_names_registered_crates() {
        let known = |n: &str| TABLE.iter().any(|(t, _)| *t == n);
        for (a, b) in SIDEWAYS {
            assert!(known(a) && known(b), "SIDEWAYS names an unregistered crate: {a} -> {b}");
            assert_eq!(classify(a).and_then(Class::layer), classify(b).and_then(Class::layer), "{a} -> {b} is not a same-layer edge");
        }
        for (from, tos) in STANDALONE_DEPS {
            assert!(matches!(classify(from), Some(Class::Standalone(_))), "{from} is not standalone");
            for to in *tos {
                assert!(known(to), "STANDALONE_DEPS names an unregistered crate: {to}");
            }
        }
    }

    #[test]
    fn upward_and_sideways_edges_flagged() {
        let v = check(&[c("xmlcraft-grammar", &[("xmlcraft-engine", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 2, to: 6, .. }]));
        // The validator must not reach into the formatter: they are siblings with no listed edge.
        let v = check(&[c("xmlcraft-validate", &[("xmlcraft-format", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 4, to: 4, .. }]));
    }

    #[test]
    fn listed_sideways_edges_allowed_in_one_direction_only() {
        assert!(check(&[c("xmlcraft-complete", &[("xmlcraft-validate", Normal, true)])]).is_empty());
        assert!(!check(&[c("xmlcraft-validate", &[("xmlcraft-complete", Normal, true)])]).is_empty());
        assert!(check(&[c("xmlcraft-xsd", &[("xmlcraft-resolve", Normal, true)])]).is_empty());
        assert!(!check(&[c("xmlcraft-resolve", &[("xmlcraft-xsd", Normal, true)])]).is_empty());
    }

    #[test]
    fn standalone_crates_use_only_their_listed_foundation() {
        assert!(check(&[c("xmlcraft-datatypes", &[("xmlcraft-pattern", Normal, true)])]).is_empty());
        assert!(check(&[c("xmlcraft-syntax", &[("xmlcraft-text", Normal, true)])]).is_empty());
        let v = check(&[c("xmlcraft-datatypes", &[("xmlcraft-text", Normal, true)])]);
        assert!(matches!(v[..], [Violation::StandaloneHasWorkspaceDep { .. }]));
        let v = check(&[c("xmlcraft-text", &[("xmlcraft-pattern", Normal, true)])]);
        assert!(matches!(v[..], [Violation::StandaloneHasWorkspaceDep { .. }]));
    }

    #[test]
    fn ui_crates_below_l7_flagged() {
        for dep in ["egui", "eframe", "winit", "rfd", "wgpu", "wgpu-core", "egui_dock", "craft-ui"] {
            let v = check(&[c("xmlcraft-engine", &[(dep, Normal, false)])]);
            assert!(matches!(v[..], [Violation::UiBelowL7 { layer: 6, .. }]), "{dep}");
        }
        assert!(check(&[c("xmlcraft-platform", &[("winit", Normal, false)])]).is_empty());
        assert!(check(&[c("xmlcraft-code-editor", &[("egui", Normal, false), ("craft-ui", Normal, false)])]).is_empty());
        assert!(check(&[c("xmlcraft-ui-egui", &[("craft-ui", Normal, false)])]).is_empty());
    }

    #[test]
    fn test_support_only_as_dev_dependency() {
        let v = check(&[c("xmlcraft-validate", &[("xmlcraft-testkit", Normal, true)])]);
        assert!(!v.is_empty());
        assert!(check(&[c("xmlcraft-validate", &[("xmlcraft-oracle", Dev, true)])]).is_empty());
        let v = check(&[c("xmlcraft-validate", &[("xmlcraft-oracle", Normal, true)])]);
        assert!(matches!(v[..], [Violation::TestkitAsNormalDep { .. }]));

        // A standalone crate may use test support in its own tests: a dev-dependency is not part of the graph the standalone
        // rule protects.
        assert!(check(&[c("xmlcraft-text", &[("xmlcraft-testkit", Dev, true)])]).is_empty());
        // ...but only as a dev-dependency, and only for test support. Product crates are still off-limits to a standalone crate
        // however they are depended on.
        let v = check(&[c("xmlcraft-text", &[("xmlcraft-testkit", Normal, true)])]);
        assert!(matches!(v[..], [Violation::StandaloneHasWorkspaceDep { .. }]), "{v:?}");
        let v = check(&[c("xmlcraft-text", &[("xmlcraft-grammar", Dev, true)])]);
        assert!(matches!(v[..], [Violation::StandaloneHasWorkspaceDep { .. }]), "{v:?}");
    }

    #[test]
    fn unregistered_crate_is_error() {
        let v = check(&[c("xmlcraft-mystery", &[])]);
        assert!(matches!(&v[..], [Violation::Unregistered { krate }] if krate == "xmlcraft-mystery"));
    }

    #[test]
    fn apps_exempt() {
        for app in ["xmlcraft", "xmlcraft-cli", "xtask"] {
            assert!(check(&[c(app, &[("egui", Normal, false), ("xmlcraft-ui-egui", Normal, true)])]).is_empty());
        }
    }

    #[test]
    fn the_app_named_like_the_prefix_is_not_confused_with_a_library() {
        // `xmlcraft` (the binary) must classify as exempt, not as a crate called "" after prefix stripping.
        assert_eq!(classify("xmlcraft"), Some(Class::Exempt));
        assert_eq!(classify("xmlcraft-cli"), Some(Class::Exempt));
        assert_eq!(classify("xmlcraft-text"), Some(Class::Standalone(0)));
    }
}
