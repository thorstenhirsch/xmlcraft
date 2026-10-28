//! The XPath subset of identity constraints (XML Schema Part 1, 3.11.6): the selector and the fields of `unique`, `key` and `keyref`.
//!
//! ```text
//! Selector ::= Path ('|' Path)*          Path ::= ('.//')? Step ('/' Step)*
//! Field    ::= FPath ('|' FPath)*        FPath ::= ('.//')? (Step '/')* (Step | '@' NameTest)
//! Step     ::= '.' | NameTest            NameTest ::= QName | '*' | NCName ':' '*'
//! ```
//!
//! A name without a prefix is in no namespace (the default namespace does not apply). The explicit axes `child::` and `attribute::` are
//! accepted for `Step` and `@`.

use xmlcraft_datatypes::is_ncname;

use crate::model::QName;

/// Which element or attribute names a step lets through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameTest {
    /// `*`
    Any,
    /// `prefix:*` (the namespace the prefix stands for; `None` is no namespace)
    Namespace(Option<String>),
    Name(QName),
}

impl NameTest {
    pub fn matches(&self, name: &QName) -> bool {
        match self {
            Self::Any => true,
            Self::Namespace(ns) => name.namespace == *ns,
            Self::Name(n) => n == name,
        }
    }
}

/// One of the paths joined by `|`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathAlternative {
    /// It starts with `.//`: the elements it names may be at any depth below the context.
    pub descendant: bool,
    /// The element steps (`.` steps are left out, they select the same node).
    pub steps: Vec<NameTest>,
    /// A field's last step, `@name`: the attribute of the element the steps lead to.
    pub attribute: Option<NameTest>,
}

impl PathAlternative {
    /// Does the chain of element names, from the child of the context element down to a node, lead to that node?
    pub fn matches_elements(&self, chain: &[&QName]) -> bool {
        let n = self.steps.len();
        let fits = if self.descendant { chain.len() >= n } else { chain.len() == n };
        if !fits {
            return false;
        }
        let tail = chain.get(chain.len() - n..).unwrap_or_default();
        self.steps.iter().zip(tail).all(|(test, name)| test.matches(name))
    }
}

/// A selector or a field: the alternatives are joined by `|`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    pub alternatives: Vec<PathAlternative>,
}

/// The expressions of an identity constraint, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintPaths {
    pub selector: Path,
    pub fields: Vec<Path>,
}

/// The namespace a prefix stands for where the expression is written.
pub type Namespaces = [(Option<String>, String)];

fn resolve(namespaces: &Namespaces, prefix: &str) -> Option<String> {
    namespaces.iter().rev().find(|(p, _)| p.as_deref() == Some(prefix)).map(|(_, uri)| uri.clone())
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
    namespaces: &'a Namespaces,
    field: bool,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        self.text.get(self.at..).unwrap_or_default()
    }

    fn skip_space(&mut self) {
        let trimmed = self.rest().trim_start_matches([' ', '\t', '\r', '\n']);
        self.at = self.text.len() - trimmed.len();
    }

    fn eat(&mut self, token: &str) -> bool {
        if self.rest().starts_with(token) {
            self.at += token.len();
            true
        } else {
            false
        }
    }

    /// A name: `*`, `prefix:*`, `name` or `prefix:name`.
    fn name_test(&mut self) -> Result<NameTest, String> {
        if self.eat("*") {
            return Ok(NameTest::Any);
        }
        let end = self.rest().find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))).unwrap_or(self.rest().len());
        let word = self.rest().get(..end).unwrap_or_default().to_owned();
        if word.is_empty() {
            return Err(format!("a name was expected at '{}'", self.rest().chars().take(10).collect::<String>()));
        }
        self.at += end;
        let mut word = word;
        if word.ends_with(':') && self.eat("*") {
            word.push('*');
        }
        match word.split_once(':') {
            None => {
                if is_ncname(&word) {
                    Ok(NameTest::Name(QName { namespace: None, local: word }))
                } else {
                    Err(format!("'{word}' is not a name"))
                }
            }
            Some((prefix, local)) => {
                if !is_ncname(prefix) {
                    return Err(format!("'{prefix}' is not a prefix"));
                }
                let namespace = resolve(self.namespaces, prefix).ok_or_else(|| format!("the prefix '{prefix}' is not declared"))?;
                if local == "*" {
                    Ok(NameTest::Namespace(Some(namespace)))
                } else if is_ncname(local) {
                    Ok(NameTest::Name(QName { namespace: Some(namespace), local: local.to_owned() }))
                } else {
                    Err(format!("'{local}' is not a name"))
                }
            }
        }
    }

    fn alternative(&mut self) -> Result<PathAlternative, String> {
        self.skip_space();
        let descendant = self.eat(".//");
        let mut steps = Vec::new();
        let mut attribute = None;
        loop {
            self.skip_space();
            if self.field && (self.rest().starts_with('@') || self.rest().starts_with("attribute::")) {
                if !self.eat("@") {
                    self.eat("attribute::");
                }
                self.skip_space();
                attribute = Some(self.name_test()?);
            } else if self.rest().starts_with('.') && !self.rest().starts_with(".//") {
                self.eat(".");
            } else {
                self.eat("child::");
                self.skip_space();
                steps.push(self.name_test()?);
            }
            self.skip_space();
            if attribute.is_some() || !self.eat("/") {
                break;
            }
            if self.rest().starts_with('/') {
                return Err("'//' can only start a path".to_owned());
            }
        }
        Ok(PathAlternative { descendant, steps, attribute })
    }

    fn path(&mut self) -> Result<Path, String> {
        let mut alternatives = vec![self.alternative()?];
        loop {
            self.skip_space();
            if self.eat("|") {
                alternatives.push(self.alternative()?);
            } else {
                break;
            }
        }
        self.skip_space();
        if self.rest().is_empty() {
            Ok(Path { alternatives })
        } else {
            Err(format!("unexpected '{}'", self.rest().chars().take(10).collect::<String>()))
        }
    }
}

/// Parse the `xpath` of a `selector`.
pub fn parse_selector(text: &str, namespaces: &Namespaces) -> Result<Path, String> {
    Parser { text, at: 0, namespaces, field: false }.path()
}

/// Parse the `xpath` of a `field`.
pub fn parse_field(text: &str, namespaces: &Namespaces) -> Result<Path, String> {
    Parser { text, at: 0, namespaces, field: true }.path()
}
