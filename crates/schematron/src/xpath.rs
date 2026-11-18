//! A light reading of XPath text, enough to find what an expression refers to without compiling it: the variables it names (`$name`) and
//! the calls of the functions a schema defines (`u:gln(…)`), which are rewritten into calls of the inline functions they become
//! (`xslt.rs`), and `current()`, which XPath does not have and XSLT does.
//!
//! String literals (`'…'`, `"…"`, with doubled quotes) and comments (`(: … :)`, nested) are skipped, so a `$` or a name inside them is
//! not a reference. Every delimiter is ASCII, so the byte positions the scanner stops at are char boundaries.

use std::collections::BTreeSet;

/// How deep the arguments of rewritten calls may nest (a deeper expression is not rewritten: it is reported, not followed).
const MAX_DEPTH: usize = 64;

/// The variable that holds the functions of a schema, as a map from their keys to inline functions (each takes the map first).
pub(crate) const FUNCTIONS_VAR: &str = "xmlcraft-fns";
/// The variable that holds the node a rule fired on (`current()` in XSLT).
pub(crate) const CURRENT_VAR: &str = "xmlcraft-current";

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_name_char(b: u8) -> bool {
    is_name_start(b) || b.is_ascii_digit() || b == b'-' || b == b'.'
}

/// Where a string literal or a comment that starts at `at` ends (just past it); `None` when none starts there. An unterminated one runs to
/// the end of the text.
fn skip_literal(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at)? {
        q @ (b'\'' | b'"') => {
            let mut i = at + 1;
            loop {
                match bytes.get(i) {
                    None => return Some(bytes.len()),
                    Some(c) if c == q => {
                        if bytes.get(i + 1) == Some(q) {
                            i += 2; // a doubled quote is the quote itself
                        } else {
                            return Some(i + 1);
                        }
                    }
                    Some(_) => i += 1,
                }
            }
        }
        b'(' if bytes.get(at + 1) == Some(&b':') => {
            let mut depth = 0usize;
            let mut i = at;
            while i < bytes.len() {
                if bytes.get(i) == Some(&b'(') && bytes.get(i + 1) == Some(&b':') {
                    depth += 1;
                    i += 2;
                } else if bytes.get(i) == Some(&b':') && bytes.get(i + 1) == Some(&b')') {
                    depth = depth.saturating_sub(1);
                    i += 2;
                    if depth == 0 {
                        return Some(i);
                    }
                } else {
                    i += 1;
                }
            }
            Some(bytes.len())
        }
        _ => None,
    }
}

/// The end of the (possibly prefixed) name that starts at `at`: `NCName` or `NCName:NCName`.
fn name_end(bytes: &[u8], at: usize) -> usize {
    let mut i = at;
    while bytes.get(i).is_some_and(|b| is_name_char(*b)) {
        i += 1;
    }
    if bytes.get(i) == Some(&b':') && bytes.get(i + 1).is_some_and(|b| is_name_start(*b)) {
        i += 1;
        while bytes.get(i).is_some_and(|b| is_name_char(*b)) {
            i += 1;
        }
    }
    i
}

/// The names of the variables `text` refers to (`$a`, `$p:b`), without the dollar sign. A variable bound in the expression itself (`for
/// $x in …`) is among them too: it is a name the caller may or may not know, which is harmless.
pub(crate) fn variables(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end) = skip_literal(bytes, i) {
            i = end;
            continue;
        }
        if bytes.get(i) == Some(&b'$') {
            let mut start = i + 1;
            while bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
                start += 1;
            }
            if bytes.get(start).is_some_and(|b| is_name_start(*b)) {
                let end = name_end(bytes, start);
                if let Some(name) = text.get(start..end) {
                    out.insert(name.to_owned());
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// What a call names, as the rewriter is told: a function of the schema (by the key it has in the map of functions), `current()`, a
/// function whose zero-argument form is written out (`normalize-space()` is `normalize-space(string(.))` by definition; the engine
/// refuses the short form on a node, a defect of its own), or nothing it knows (the call is left as it is).
pub(crate) enum Callee {
    Function(String),
    Current,
    OfContextString,
    Other,
}

/// The arguments of the call whose `(` is at `open`: their texts and the position just past the `)`. `None` when the parenthesis is
/// not closed.
fn arguments(text: &str, open: usize) -> Option<(Vec<&str>, usize)> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut args = Vec::new();
    let mut start = open + 1;
    let mut i = open;
    while i < bytes.len() {
        if let Some(end) = skip_literal(bytes, i) {
            i = end;
            continue;
        }
        match bytes.get(i) {
            Some(b'(' | b'[' | b'{') => depth += 1,
            Some(b')' | b']' | b'}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let last = text.get(start..i)?;
                    if !(args.is_empty() && last.trim().is_empty()) {
                        args.push(last);
                    }
                    return Some((args, i + 1));
                }
            }
            Some(b',') if depth == 1 => {
                args.push(text.get(start..i)?);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `text` with the calls `resolve` knows rewritten: a function of the schema `p:f(a, b)` becomes `$xmlcraft-fns('key')($xmlcraft-fns, a,
/// b)` (the key is added to `used`), and `current()` becomes `$xmlcraft-current` when `current` is allowed. `resolve` is asked with
/// the name as written and the number of arguments.
pub(crate) fn rewrite_calls(
    text: &str,
    resolve: &dyn Fn(&str, usize) -> Callee,
    current: bool,
    used: &mut BTreeSet<String>,
) -> Result<String, String> {
    rewrite_at(text, resolve, current, used, 0)
}

fn rewrite_at(
    text: &str,
    resolve: &dyn Fn(&str, usize) -> Callee,
    current: bool,
    used: &mut BTreeSet<String>,
    depth: usize,
) -> Result<String, String> {
    if depth > MAX_DEPTH {
        return Err("the expression nests calls too deeply to be read".to_owned());
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0; // the text before this has been written out
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end) = skip_literal(bytes, i) {
            i = end;
            continue;
        }
        let Some(&b) = bytes.get(i) else { break };
        if b == b'$' || b == b'@' {
            // A variable or an attribute: its name is no call.
            let mut j = i + 1;
            while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
                j += 1;
            }
            i = if bytes.get(j).is_some_and(|c| is_name_start(*c)) { name_end(bytes, j) } else { j };
            continue;
        }
        let after_name_char = i > 0 && bytes.get(i - 1).is_some_and(|c| is_name_char(*c) || *c == b':');
        if !is_name_start(b) || after_name_char {
            i += 1;
            continue;
        }
        let end = name_end(bytes, i);
        let mut open = end;
        while bytes.get(open).is_some_and(u8::is_ascii_whitespace) {
            open += 1;
        }
        if bytes.get(open) != Some(&b'(') || bytes.get(open + 1) == Some(&b':') {
            i = end;
            continue;
        }
        let name = text.get(i..end).unwrap_or_default();
        let Some((args, close)) = arguments(text, open) else {
            i = end;
            continue;
        };
        match resolve(name, args.len()) {
            Callee::Function(key) => {
                out.push_str(text.get(copied..i).unwrap_or_default());
                out.push_str(&format!("${FUNCTIONS_VAR}({})(${FUNCTIONS_VAR}", string_literal(&key)));
                for arg in args {
                    out.push(',');
                    if !arg.starts_with(char::is_whitespace) {
                        out.push(' ');
                    }
                    out.push_str(&rewrite_at(arg, resolve, current, used, depth + 1)?);
                }
                out.push(')');
                used.insert(key);
                copied = close;
                i = close;
            }
            Callee::OfContextString if args.is_empty() => {
                out.push_str(text.get(copied..close).unwrap_or_default().trim_end_matches(')'));
                out.push_str("string(.))");
                copied = close;
                i = close;
            }
            Callee::Current if current && args.is_empty() => {
                out.push_str(text.get(copied..i).unwrap_or_default());
                out.push('$');
                out.push_str(CURRENT_VAR);
                copied = close;
                i = close;
            }
            _ => i = end,
        }
    }
    out.push_str(text.get(copied..).unwrap_or_default());
    Ok(out)
}

/// The words after which `or` and `and` are names, not operators (`a eq or` compares with the child `or`).
const OPERATOR_WORDS: [&str; 22] = [
    "or",
    "and",
    "div",
    "idiv",
    "mod",
    "eq",
    "ne",
    "lt",
    "le",
    "gt",
    "ge",
    "is",
    "to",
    "union",
    "intersect",
    "except",
    "of",
    "as",
    "return",
    "then",
    "else",
    "satisfies",
];

/// A word of an expression at its own level (outside brackets and literals): where it starts and ends, and the word.
struct Word<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

/// What an expression holds at its own level: its words, the positions of its commas, and of colons that are neither in a name nor
/// `::` or `:=` (a map entry's). `None` when its brackets do not balance.
fn top_level(text: &str) -> Option<(Vec<Word<'_>>, Vec<usize>, Vec<usize>)> {
    let bytes = text.as_bytes();
    let (mut words, mut commas, mut colons) = (Vec::new(), Vec::new(), Vec::new());
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(end) = skip_literal(bytes, i) {
            i = end;
            continue;
        }
        let Some(&b) = bytes.get(i) else { break };
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
            b',' if depth == 0 => commas.push(i),
            b':' if matches!(bytes.get(i + 1), Some(b':' | b'=')) => {
                i += 2;
                continue;
            }
            b':' if depth == 0 && bytes.get(i + 1) != Some(&b'*') && i.checked_sub(1).and_then(|p| bytes.get(p)) != Some(&b'*') => colons.push(i),
            b'$' | b'@' => {
                // A variable or an attribute: its name is no operator.
                let mut j = i + 1;
                while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
                    j += 1;
                }
                i = if bytes.get(j).is_some_and(|c| is_name_start(*c)) { name_end(bytes, j) } else { j };
                continue;
            }
            c if is_name_char(c) => {
                let end = name_end(bytes, i).max(i + 1);
                if depth == 0
                    && let Some(word) = text.get(i..end)
                {
                    words.push(Word { start: i, end, text: word });
                }
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    (depth == 0).then_some((words, commas, colons))
}

/// Where the `operator` (`or` or `and`) splits `text` (a part without top-level commas), or `None` when it may not be split: it uses
/// `if`, `some`, `every`, `for` or `let` at its own level (their expressions run to the end of the part), or a map entry's colon.
fn logical_operators(text: &str, operator: &str) -> Option<Vec<(usize, usize)>> {
    let (words, commas, colons) = top_level(text)?;
    if !commas.is_empty() || !colons.is_empty() {
        return None;
    }
    let next_char = |at: usize| text.get(at..).and_then(|s| s.trim_start().bytes().next());
    let mut out = Vec::new();
    for (n, w) in words.iter().enumerate() {
        match (w.text, next_char(w.end)) {
            ("if", Some(b'(')) | ("some" | "every" | "for" | "let", Some(b'$')) => return None,
            _ => {}
        }
        if w.text != operator {
            continue;
        }
        // An operator follows an operand: the text before it ends with a name, a number, a literal, `)`, `]`, `.`, `*` or `?`, and not
        // with an axis, a step or another operator word (`a eq or` and `a/or` name a child `or`).
        let before = text.get(..w.start).unwrap_or_default().trim_end();
        let operand_end =
            before.bytes().last().is_some_and(|c| is_name_char(c) || matches!(c, b')' | b']' | b'}' | b'\'' | b'"' | b'.' | b'*' | b'?'));
        let after_operator_word = n
            .checked_sub(1)
            .and_then(|p| words.get(p))
            .is_some_and(|p| OPERATOR_WORDS.contains(&p.text) && text.get(p.end..w.start).is_some_and(|s| s.trim().is_empty()));
        let after_step = before.ends_with("::") || before.ends_with('/') || before.ends_with('@');
        if operand_end && !after_operator_word && !after_step {
            out.push((w.start, w.end));
        }
    }
    Some(out)
}

/// `text` split at the operators found by [`logical_operators`], each operand trimmed; `None` when an operand is empty.
fn operands<'a>(text: &'a str, at: &[(usize, usize)]) -> Option<Vec<&'a str>> {
    let mut out = Vec::with_capacity(at.len() + 1);
    let mut start = 0usize;
    for (s, e) in at {
        out.push(text.get(start..*s)?.trim());
        start = *e;
    }
    out.push(text.get(start..)?.trim());
    (!out.iter().any(|o| o.is_empty())).then_some(out)
}

/// `text` with its logical operators made to short-circuit from left to right, as XPath 1.0 defines them and as the publishers'
/// processors evaluate them (XPath 3.1 §3.7 leaves the order to the implementation): `A or B` becomes
/// `(if (A) then true() else boolean(B))` and `A and B` becomes `(if (A) then boolean(B) else false())`, which have the same value
/// whenever both operands have one. The engine evaluates both operands of `or` and `and`, so a rule such as
/// `not(x and y) or u:f(x * y)` raised a type error where its left operand had already decided it (PEPPOL-EN16931-R040 of the
/// XRechnung rules, found by comparing with Saxon-HE, which short-circuits).
///
/// The parts in parentheses and square brackets are rewritten too. A part that uses `if`, `some`, `every`, `for` or `let` at its own
/// level, a map constructor's entries and the bodies of inline functions (in braces) are left as they are, and so is text that has no
/// logical operator.
pub(crate) fn short_circuit(text: &str) -> String {
    short_circuit_at(text, 0)
}

fn short_circuit_at(text: &str, depth: usize) -> String {
    if depth > MAX_DEPTH || !(text.contains("or") || text.contains("and")) {
        return text.to_owned();
    }
    // 1. The parts in parentheses and square brackets, each on its own.
    let bytes = text.as_bytes();
    let mut rebuilt = String::with_capacity(text.len());
    let mut copied = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(end) = skip_literal(bytes, i) {
            i = end;
            continue;
        }
        match bytes.get(i) {
            Some(open @ (b'(' | b'[')) => {
                let close = if *open == b'(' { b')' } else { b']' };
                let Some(end) = matching(bytes, i) else { return text.to_owned() };
                if bytes.get(end) == Some(&close) {
                    rebuilt.push_str(text.get(copied..=i).unwrap_or_default());
                    rebuilt.push_str(&short_circuit_at(text.get(i + 1..end).unwrap_or_default(), depth + 1));
                    copied = end;
                }
                i = end + 1;
            }
            Some(b'{') => i = matching(bytes, i).map_or(bytes.len(), |e| e + 1),
            _ => i += 1,
        }
    }
    rebuilt.push_str(text.get(copied..).unwrap_or_default());
    // 2. This level: each part between commas, split at `or`, each operand at `and`.
    let Some((_, commas, _)) = top_level(&rebuilt) else { return text.to_owned() };
    let mut out = String::with_capacity(rebuilt.len());
    let mut start = 0usize;
    for end in commas.iter().copied().chain(std::iter::once(rebuilt.len())) {
        out.push_str(&logical_part(rebuilt.get(start..end).unwrap_or_default()));
        out.push_str(rebuilt.get(end..end.saturating_add(1)).unwrap_or_default());
        start = end.saturating_add(1);
    }
    out
}

/// A conjunction (`A and B and C`) as nested `if`s; with `last`, its value is a boolean even when it is a single operand (the last
/// operand of a disjunction is taken by `boolean()`). `None` when it may not be split.
fn conjunction(text: &str, last: bool) -> Option<String> {
    let ands = logical_operators(text, "and")?;
    let terms = operands(text, &ands)?;
    let mut terms = terms.into_iter().rev();
    let final_term = terms.next()?;
    if ands.is_empty() {
        return Some(if last { format!("boolean({final_term})") } else { final_term.to_owned() });
    }
    let mut e = format!("boolean({final_term})");
    for t in terms {
        e = format!("(if ({t}) then {e} else false())");
    }
    Some(e)
}

/// One part (no top-level comma) with its `or` and `and` rewritten, or as it is when it has none or may not be split.
fn logical_part(part: &str) -> String {
    let trimmed = part.trim();
    let lead = part.get(..part.len() - part.trim_start().len()).unwrap_or_default();
    let trail = part.get(part.trim_end().len()..).unwrap_or_default();
    let Some(ors) = logical_operators(trimmed, "or") else { return part.to_owned() };
    if ors.is_empty() {
        if logical_operators(trimmed, "and").is_none_or(|a| a.is_empty()) {
            return part.to_owned();
        }
        return conjunction(trimmed, true).map_or_else(|| part.to_owned(), |e| format!("{lead}{e}{trail}"));
    }
    let Some(terms) = operands(trimmed, &ors) else { return part.to_owned() };
    let mut terms = terms.into_iter().rev();
    let Some(Some(mut e)) = terms.next().map(|t| conjunction(t, true)) else { return part.to_owned() };
    for t in terms {
        let Some(c) = conjunction(t, false) else { return part.to_owned() };
        e = format!("if ({c}) then true() else {e}");
    }
    format!("{lead}({e}){trail}")
}

/// The position of the bracket that closes the one at `open` (literals and comments skipped), or `None` when it is not closed.
fn matching(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        if let Some(end) = skip_literal(bytes, i) {
            i = end;
            continue;
        }
        match bytes.get(i) {
            Some(b'(' | b'[' | b'{') => depth += 1,
            Some(b')' | b']' | b'}') => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether every branch of the pattern `text` (split at the `|` outside brackets and literals) is an absolute path (`/a`, `//b`): it
/// selects the same nodes from wherever it is evaluated, so it need not be evaluated from every node of the document.
pub(crate) fn is_absolute_pattern(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut i = 0usize;
    let mut branches: Vec<&str> = Vec::new();
    while i < bytes.len() {
        if let Some(end) = skip_literal(bytes, i) {
            i = end;
            continue;
        }
        match bytes.get(i) {
            Some(b'(' | b'[' | b'{') => depth += 1,
            Some(b')' | b']' | b'}') => depth = depth.saturating_sub(1),
            Some(b'|') if depth == 0 => {
                branches.push(text.get(start..i).unwrap_or_default());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    branches.push(text.get(start..).unwrap_or_default());
    // (`union`, `except` and `intersect` are words, not bars: such a pattern is not taken as absolute.)
    branches.iter().all(|b| {
        let b = b.trim();
        b.starts_with('/') && !b.contains(" union ") && !b.contains(" except ") && !b.contains(" intersect ")
    })
}

/// `text` as an XPath string literal.
pub(crate) fn string_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(name: &str, arity: usize) -> Callee {
        match (name, arity) {
            ("u:f", 1) => Callee::Function("{utils}f#1".into()),
            ("u:g", 2) => Callee::Function("{utils}g#2".into()),
            ("current", _) => Callee::Current,
            ("normalize-space", 0) => Callee::OfContextString,
            _ => Callee::Other,
        }
    }

    fn rewrite(text: &str) -> (String, BTreeSet<String>) {
        let mut used = BTreeSet::new();
        let out = rewrite_calls(text, &resolve, true, &mut used).expect("rewrites");
        (out, used)
    }

    #[test]
    fn variables_are_found_outside_literals_and_comments() {
        let found = variables("$a + $BR-CO-15 * $p:q and '$no' and \"$neither\" (: $nor :) and $ spaced");
        assert_eq!(found.into_iter().collect::<Vec<_>>(), ["BR-CO-15", "a", "p:q", "spaced"]);
        assert!(variables("'unterminated $x").is_empty());
        assert!(variables("$").is_empty() && variables("$1").is_empty());
    }

    #[test]
    fn calls_of_the_schemas_functions_are_rewritten_with_their_arguments() {
        let (out, used) = rewrite("u:f(cbc:ID) and not(u:g(1, u:f('a,b')))");
        assert_eq!(
            out,
            "$xmlcraft-fns('{utils}f#1')($xmlcraft-fns, cbc:ID) and not($xmlcraft-fns('{utils}g#2')($xmlcraft-fns, 1, $xmlcraft-fns('{utils}f#1')($xmlcraft-fns, 'a,b')))"
        );
        assert_eq!(used.into_iter().collect::<Vec<_>>(), ["{utils}f#1", "{utils}g#2"]);
    }

    #[test]
    fn what_is_not_a_call_of_a_known_function_stays() {
        for text in ["u:f(1, 2)", "$u:f(1)", "@u:f", "'u:f(1)'", "(: u:f(1) :)", "xu:f(1)", "u:f", "u:f(", "concat(u:h(1), 'x')", "a/u:f"] {
            assert_eq!(rewrite(text).0, text, "{text}");
        }
        assert_eq!(rewrite("current()/cbc:ID = current( )").0, "$xmlcraft-current/cbc:ID = $xmlcraft-current");
        assert_eq!(
            rewrite("normalize-space() = normalize-space( ) and normalize-space(.)").0,
            "normalize-space(string(.)) = normalize-space( string(.)) and normalize-space(.)"
        );
        let mut used = BTreeSet::new();
        assert_eq!(rewrite_calls("current()", &resolve, false, &mut used).expect("rewrites"), "current()", "not where it is not allowed");
    }

    #[test]
    fn logical_operators_become_short_circuiting_ifs() {
        assert_eq!(short_circuit("a or b"), "(if (a) then true() else boolean(b))");
        assert_eq!(short_circuit("a and b"), "(if (a) then boolean(b) else false())");
        assert_eq!(
            short_circuit("a or b and c or d"),
            "(if (a) then true() else if ((if (b) then boolean(c) else false())) then true() else boolean(d))"
        );
        assert_eq!(short_circuit("not(x and y) or f(x * y)"), "(if (not((if (x) then boolean(y) else false()))) then true() else boolean(f(x * y)))");
        assert_eq!(short_circuit("count(a[b or c]) = 1, d"), "count(a[(if (b) then true() else boolean(c))]) = 1, d");
        assert_eq!(short_circuit(" a or b "), " (if (a) then true() else boolean(b)) ");
    }

    #[test]
    fn what_is_no_logical_operator_or_may_not_be_split_stays() {
        for text in [
            "ram:or = 'or'",
            "a/or",
            "@or and",
            "$or",
            "child::and",
            "x eq or",
            "'a or b'",
            "(: a or b :) c",
            "or",
            "and b",
            "a or",
            "if (a) then b else c or d",
            "some $x in a satisfies $x or b",
            "for $x in a return $x and b",
            "let $x := a return $x or b",
            "map { 'k' : a or b }",
            "function($x) { $x or b }",
            "ram:SpecifiedTradeAllowanceCharge/ram:ChargeIndicator",
            "a or (b",
            "",
        ] {
            assert_eq!(short_circuit(text), text, "{text}");
        }
        // Each part of a sequence or argument list on its own, and the parts that may not be split left alone.
        assert_eq!(short_circuit("f(if (a) then b else c, d or e)"), "f(if (a) then b else c, (if (d) then true() else boolean(e)))");
    }

    #[test]
    fn absolute_patterns_are_told_from_relative_ones() {
        for text in ["/a", "//b", " /a | //b[c | d] ", "/a[x = '|b']"] {
            assert!(is_absolute_pattern(text), "{text}");
        }
        for text in ["a", "/a | b", "@x", "(/a)", "/a union b", "", "a[/b]"] {
            assert!(!is_absolute_pattern(text), "{text}");
        }
    }

    #[test]
    fn deep_nesting_is_refused_and_any_text_is_answered() {
        let deep = format!("{}1{}", "u:f(".repeat(200), ")".repeat(200));
        let mut used = BTreeSet::new();
        assert!(rewrite_calls(&deep, &resolve, true, &mut used).is_err());
        for text in ["", "(", ")", "'", "\"", "(:", "u:f(')", "u:f((:)", "é u:f(ü)", "$", "@", "u:"] {
            let _ = rewrite_calls(text, &resolve, true, &mut BTreeSet::new());
            let _ = variables(text);
        }
    }

    proptest::proptest! {
        #[test]
        fn any_text_is_read_without_a_panic(text in ".{0,200}") {
            let _ = rewrite_calls(&text, &resolve, true, &mut BTreeSet::new());
            let _ = variables(&text);
            let _ = short_circuit(&text);
        }

        #[test]
        fn any_logical_text_is_rewritten_without_a_panic(text in "[a-z()\\[\\]{},:'$@/ ]{0,60}( or | and )[a-z()\\[\\]{},:'$@/ ]{0,60}") {
            let _ = short_circuit(&text);
        }
    }
}
