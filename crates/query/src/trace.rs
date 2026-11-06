//! A trace of an XPath path expression: what each step leaves, how many items and how long it took.
//!
//! A path such as `//order[@id > 3]/item/name` is a chain of steps. Evaluating the path up to each step gives the items that step leaves:
//! the count says where a path loses (or gains) its items, and the time says which step is the costly one. This is the way to see what
//! an expression does without an engine with hooks: every prefix is an expression of its own, evaluated on the same document.
//!
//! The steps are found by cutting at the `/` that are not inside brackets, parentheses or quotes. An expression that is not a path (a
//! function call, a union, an arithmetic expression, a `for` or `let`) is one step: the whole of it.

use std::time::Instant;

use crate::{Options, Prepared, QueryError};

/// The most steps traced.
const MAX_STEPS: usize = 200;

/// One step of the trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The path up to and including this step.
    pub expression: String,
    /// Just this step (`item[@n > 1]`, or `//` as `/descendant-or-self::node()/`).
    pub step: String,
    /// How many items the path leaves here.
    pub count: usize,
    /// How long evaluating the path up to here took, in microseconds.
    pub micros: u64,
    /// Why this step could not be evaluated (the later ones are not traced).
    pub error: Option<QueryError>,
}

/// The trace of an expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trace {
    pub steps: Vec<Step>,
    /// The whole expression was cut into steps (it is a path); otherwise there is the one step.
    pub is_path: bool,
}

/// Where the steps of `expression` end: the byte offsets after each step (the last is the length).
fn step_ends(expression: &str) -> Option<Vec<usize>> {
    // A union, a comparison, a function at the top or a keyword expression is not a path to be cut.
    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;
    let mut quote: Option<char> = None;
    let mut ends: Vec<usize> = Vec::new();
    let mut chars = expression.char_indices().peekable();
    // (A leading `/` or `//` belongs to the first step.)
    let mut first_content = false;
    while let Some((i, c)) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '(' => depth_paren += 1,
            ')' => depth_paren = depth_paren.saturating_sub(1),
            '[' => depth_bracket += 1,
            ']' => depth_bracket = depth_bracket.saturating_sub(1),
            '|' | ',' | '=' | '<' | '>' | '!' | '+' if depth_paren == 0 && depth_bracket == 0 && first_content => return None,
            '/' if depth_paren == 0 && depth_bracket == 0 => {
                // `//` is one separator.
                let doubled = chars.peek().is_some_and(|(_, n)| *n == '/');
                if first_content {
                    ends.push(i);
                    if doubled {
                        chars.next();
                    }
                } else if doubled {
                    chars.next();
                }
            }
            c if !c.is_whitespace() => first_content = true,
            _ => {}
        }
    }
    if quote.is_some() || depth_paren != 0 || depth_bracket != 0 {
        return None;
    }
    ends.push(expression.len());
    Some(ends)
}

fn has_keyword_start(expression: &str) -> bool {
    let t = expression.trim_start();
    ["for ", "let ", "if ", "some ", "every ", "function(", "fn:", "(", "map{", "map {", "["].iter().any(|k| t.starts_with(k))
}

/// Trace `expression` on `prepared`: each step of a path with the number of items it leaves and the time it took.
pub fn trace(prepared: &mut Prepared, expression: &str, options: &Options) -> Trace {
    let cuts = if has_keyword_start(expression) { None } else { step_ends(expression) };
    let (ends, is_path) = match cuts {
        Some(ends) if ends.len() > 1 && ends.len() <= MAX_STEPS => (ends, true),
        _ => (vec![expression.len()], false),
    };
    let mut steps = Vec::new();
    let mut previous = 0usize;
    for end in ends {
        let Some(prefix) = expression.get(..end) else { break };
        let step = expression.get(previous..end).unwrap_or_default().trim().to_owned();
        let started = Instant::now();
        let outcome = prepared.evaluate(prefix, options);
        let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        match outcome {
            Ok(done) => steps.push(Step { expression: prefix.to_owned(), step, count: done.count, micros, error: None }),
            Err(e) => {
                steps.push(Step { expression: prefix.to_owned(), step, count: 0, micros, error: Some(e) });
                break;
            }
        }
        previous = end;
    }
    Trace { steps, is_path }
}
