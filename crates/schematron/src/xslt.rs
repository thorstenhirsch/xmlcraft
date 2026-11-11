//! The XSLT functions of a schema (`xsl:function` at the top level, as the XSLT query bindings allow) as XPath 3.1 inline functions.
//!
//! A function's body is a sequence constructor. The part of XSLT that functions in rule sets use is turned into one XPath expression:
//! `xsl:variable` (with `select`, with text, or with a sequence constructor) becomes `let … return`, `xsl:sequence` its `select`,
//! `xsl:choose`/`xsl:when`/`xsl:otherwise` and `xsl:if` become `if … then … else`. Anything else (a template call, a `for-each`, an
//! element constructor, text) makes the function untranslatable; it is kept with the reason, so that a rule calling it is reported as
//! not evaluated, never as passed or failed.
//!
//! The functions of a schema are one map, from the key of each function (`{namespace}local#arity`) to the inline function; each takes
//! the map as its first argument, so that functions can call one another and themselves (the engine stops runaway recursion with an
//! error of its own).

use xmlcraft_xsd::dom::{Dom, El};

use crate::xpath::{FUNCTIONS_VAR, string_literal};

/// The XSLT namespace.
pub(crate) const XSLT_NS: &str = "http://www.w3.org/1999/XSL/Transform";

/// How deeply the instructions of a function may nest.
const MAX_DEPTH: usize = 32;

/// A function of the schema, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Function {
    /// The name as written (`u:gln`), for messages.
    pub name: String,
    /// `{namespace}local#arity`.
    pub key: String,
    /// The parameters: their names and declared types.
    pub params: Vec<(String, Option<String>)>,
    pub as_type: Option<String>,
    /// The body as one XPath expression (calls not rewritten yet), or why it cannot be.
    pub body: Result<String, String>,
}

/// The key of a function: its expanded name and arity.
pub(crate) fn key(namespace: &str, local: &str, arity: usize) -> String {
    format!("{{{namespace}}}{local}#{arity}")
}

fn attr<'a>(el: &'a El, name: &str) -> Option<&'a str> {
    el.attrs.iter().find(|a| a.namespace.is_none() && a.local == name).map(|a| a.value.as_str())
}

fn is_xsl(el: &El, local: &str) -> bool {
    el.local == local && el.namespace.as_deref() == Some(XSLT_NS)
}

/// Read the function `el` (an `xsl:function`) of `tree`. `None` when it has no usable name. A prefix the document does not declare is
/// looked up in `namespaces` (the `ns` elements of the schema, which the Schematron compilers declare in the stylesheet they write).
pub(crate) fn read(tree: &Dom, at: usize, namespaces: &[(String, String)]) -> Option<Function> {
    let el = tree.elements.get(at)?;
    let name = attr(el, "name")?.trim();
    let (prefix, local) = name.split_once(':')?;
    // A function of a stylesheet is always in a namespace.
    let namespace = tree.lookup(at, Some(prefix)).or_else(|| namespaces.iter().find(|(p, _)| p == prefix).map(|(_, uri)| uri.clone()))?;
    let children: Vec<&El> = el.children.iter().filter_map(|i| tree.elements.get(*i)).collect();
    let params: Vec<(String, Option<String>)> = children
        .iter()
        .filter(|c| is_xsl(c, "param"))
        .filter_map(|c| Some((attr(c, "name")?.trim().to_owned(), attr(c, "as").map(|t| t.trim().to_owned()))))
        .collect();
    let body_items: Vec<usize> = el.children.iter().copied().filter(|i| tree.elements.get(*i).is_some_and(|c| !is_xsl(c, "param"))).collect();
    let body = if !el.text.trim().is_empty() {
        Err(format!("the function {name} makes text, which an XPath expression cannot"))
    } else {
        sequence(tree, &body_items, name, 0)
    };
    Some(Function {
        name: name.to_owned(),
        key: key(&namespace, local, params.len()),
        params,
        as_type: attr(el, "as").map(|t| t.trim().to_owned()),
        body,
    })
}

/// The sequence constructor made of the elements `items` (in order) as one expression.
fn sequence(tree: &Dom, items: &[usize], function: &str, depth: usize) -> Result<String, String> {
    if depth > MAX_DEPTH {
        return Err(format!("the function {function} nests its instructions too deeply"));
    }
    let mut parts: Vec<String> = Vec::new();
    for (n, index) in items.iter().enumerate() {
        let Some(el) = tree.elements.get(*index) else { continue };
        if el.namespace.as_deref() != Some(XSLT_NS) {
            return Err(format!("the function {function} constructs the element {}, which an XPath expression cannot", el.qname));
        }
        match el.local.as_str() {
            "variable" => {
                let name = attr(el, "name").map(str::trim).filter(|n| !n.is_empty());
                let Some(name) = name else { return Err(format!("a variable of the function {function} has no name")) };
                let value = value_of(tree, el, function, depth)?;
                let rest = sequence(tree, items.get(n + 1..).unwrap_or_default(), function, depth + 1)?;
                parts.push(format!("let ${name} := ({value}) return ({rest})"));
                // (The variable is in scope for what follows it, which is now inside the `let`.)
                return Ok(join(parts));
            }
            "sequence" => parts.push(value_of(tree, el, function, depth)?),
            "choose" => {
                let mut branches: Vec<(String, String)> = Vec::new();
                let mut otherwise = "()".to_owned();
                for i in &el.children {
                    let Some(branch) = tree.elements.get(*i) else { continue };
                    let inner = sequence(tree, &branch.children, function, depth + 1)?;
                    if is_xsl(branch, "when") {
                        let Some(test) = attr(branch, "test") else { return Err(format!("an xsl:when of the function {function} has no test")) };
                        branches.push((test.to_owned(), inner));
                    } else if is_xsl(branch, "otherwise") {
                        otherwise = inner;
                    } else {
                        return Err(format!("the function {function} has {} in an xsl:choose", branch.qname));
                    }
                }
                let mut expression = otherwise;
                for (test, then) in branches.into_iter().rev() {
                    expression = format!("if ({test}) then ({then}) else ({expression})");
                }
                parts.push(expression);
            }
            "if" => {
                let Some(test) = attr(el, "test") else { return Err(format!("an xsl:if of the function {function} has no test")) };
                let inner = sequence(tree, &el.children, function, depth + 1)?;
                parts.push(format!("if ({test}) then ({inner}) else ()"));
            }
            other => return Err(format!("the function {function} uses xsl:{other}, which is not turned into XPath")),
        }
    }
    Ok(join(parts))
}

fn join(parts: Vec<String>) -> String {
    match parts.len() {
        0 => "()".to_owned(),
        1 => parts.into_iter().next().unwrap_or_default(),
        _ => format!("({})", parts.join(", ")),
    }
}

/// The value of an `xsl:variable` or `xsl:sequence`: its `select`, its sequence constructor, or its text (a string).
fn value_of(tree: &Dom, el: &El, function: &str, depth: usize) -> Result<String, String> {
    if let Some(select) = attr(el, "select") {
        if !el.children.is_empty() || !el.text.trim().is_empty() {
            return Err(format!("an instruction of the function {function} has both a select and content"));
        }
        return Ok(select.to_owned());
    }
    if el.children.is_empty() {
        // Text content: a temporary tree in XSLT, whose string value is the text; used as a string here.
        return Ok(string_literal(&el.text));
    }
    if !el.text.trim().is_empty() {
        return Err(format!("an instruction of the function {function} mixes text and instructions"));
    }
    sequence(tree, &el.children, function, depth + 1)
}

/// The function as an inline function expression, with `body` (its calls already rewritten) as its body.
pub(crate) fn inline(function: &Function, body: &str) -> String {
    let mut params = vec![format!("${FUNCTIONS_VAR}")];
    for (name, as_type) in &function.params {
        params.push(match as_type {
            Some(t) if !t.is_empty() => format!("${name} as {t}"),
            _ => format!("${name}"),
        });
    }
    let returns = function.as_type.as_deref().filter(|t| !t.is_empty()).map(|t| format!(" as {t}")).unwrap_or_default();
    format!("function({}){returns} {{ {body} }}", params.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_first(body: &str) -> Function {
        let text = format!("<s xmlns:xsl=\"{XSLT_NS}\" xmlns:u=\"utils\"><xsl:function name=\"u:f\" as=\"xs:boolean\">{body}</xsl:function></s>");
        let parsed = xmlcraft_syntax::parse(&text);
        let tree = xmlcraft_xsd::dom::build(&parsed.syntax());
        read(&tree, 1, &[]).expect("a function")
    }

    #[test]
    fn params_variables_and_a_sequence_are_one_expression() {
        let f = read_first(
            "<xsl:param name=\"val\" as=\"xs:string\"/><xsl:param name=\"n\"/><xsl:variable name=\"len\" select=\"string-length($val)\"/><xsl:variable name=\"ok\">ABC</xsl:variable><xsl:sequence select=\"$len = $n and contains($ok, $val)\"/>",
        );
        assert_eq!(f.key, "{utils}f#2");
        assert_eq!(f.params, [("val".to_owned(), Some("xs:string".to_owned())), ("n".to_owned(), None)]);
        assert_eq!(f.body.as_deref(), Ok("let $len := (string-length($val)) return (let $ok := ('ABC') return ($len = $n and contains($ok, $val)))"));
        assert_eq!(inline(&f, "true()"), "function($xmlcraft-fns, $val as xs:string, $n) as xs:boolean { true() }");
    }

    #[test]
    fn choose_and_if_are_conditionals() {
        let f = read_first(
            "<xsl:param name=\"a\"/><xsl:choose><xsl:when test=\"$a = 1\"><xsl:sequence select=\"'one'\"/></xsl:when><xsl:when test=\"$a = 2\"><xsl:variable name=\"b\" as=\"xs:integer\"><xsl:sequence select=\"2\"/></xsl:variable><xsl:sequence select=\"$b\"/></xsl:when><xsl:otherwise><xsl:sequence select=\"false()\"/></xsl:otherwise></xsl:choose>",
        );
        assert_eq!(f.body.as_deref(), Ok("if ($a = 1) then ('one') else (if ($a = 2) then (let $b := (2) return ($b)) else (false()))"));
        let g = read_first("<xsl:if test=\"true()\"><xsl:sequence select=\"1\"/></xsl:if>");
        assert_eq!(g.body.as_deref(), Ok("if (true()) then (1) else ()"));
    }

    #[test]
    fn what_xpath_cannot_say_is_a_reason() {
        for body in [
            "<xsl:for-each select=\"1\"/>",
            "<out/>",
            "text",
            "<xsl:sequence select=\"1\">2</xsl:sequence>",
            "<xsl:variable select=\"1\"/>",
            "<xsl:choose><xsl:when><xsl:sequence select=\"1\"/></xsl:when></xsl:choose>",
        ] {
            assert!(read_first(body).body.is_err(), "{body}");
        }
        let deep = format!("{}<xsl:sequence select=\"1\"/>{}", "<xsl:if test=\"1\">".repeat(40), "</xsl:if>".repeat(40));
        assert!(read_first(&deep).body.is_err());
        assert_eq!(read_first("").body.as_deref(), Ok("()"));
    }
}
