//! The syntax of XML Schema regular expressions (XML Schema Part 2, appendix F), read into a tree.
//!
//! What differs from the regular expressions most programmers know: the whole value must match (there are no anchors, and `^` and `$`
//! are ordinary characters), there is no way to ask for a lazy repetition, a group, a look-around or a back reference, `.` stands for
//! anything but a line end, a class can have another class taken out of it (`[a-z-[aeiou]]`), `\p{IsBlock}` names a Unicode block, and
//! `\i`, `\c` (and `\I`, `\C`) are the characters XML names are made of.

use regex_syntax::hir::ClassUnicode;

use crate::classes::{add_range, block_set, category, from_ranges, multi_char, negated, single};
use crate::error::{ErrorKind, PatternError};
use crate::{MAX_COUNT, MAX_DEPTH, MAX_PATTERN_CHARS};

/// A parsed pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Node {
    /// Matches the empty string.
    Empty,
    /// One character out of the set (a literal character is a set of one).
    Class(ClassUnicode),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
    },
}

/// Read `pattern`.
pub(crate) fn parse(pattern: &str) -> Result<Node, PatternError> {
    let chars: Vec<char> = pattern.chars().take(MAX_PATTERN_CHARS + 1).collect();
    if chars.len() > MAX_PATTERN_CHARS {
        return Err(PatternError::new(MAX_PATTERN_CHARS, ErrorKind::TooLong));
    }
    let mut parser = Parser { chars, pos: 0 };
    let node = parser.alternation(0)?;
    match parser.peek() {
        None => Ok(node),
        Some(_) => Err(PatternError::new(parser.pos, ErrorKind::UnbalancedClose)),
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

/// One thing in a character class: a character (that can start a range) or a set.
enum Item {
    Char(char),
    Set(ClassUnicode),
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn error<T>(&self, position: usize, kind: ErrorKind) -> Result<T, PatternError> {
        Err(PatternError::new(position, kind))
    }

    /// `branch ('|' branch)*`
    fn alternation(&mut self, depth: usize) -> Result<Node, PatternError> {
        let mut branches = vec![self.branch(depth)?];
        while self.peek() == Some('|') {
            self.pos += 1;
            branches.push(self.branch(depth)?);
        }
        Ok(if branches.len() == 1 { branches.remove(0) } else { Node::Alt(branches) })
    }

    /// `piece*`
    fn branch(&mut self, depth: usize) -> Result<Node, PatternError> {
        let mut pieces = Vec::new();
        while !matches!(self.peek(), None | Some('|' | ')')) {
            pieces.push(self.piece(depth)?);
        }
        Ok(match pieces.len() {
            0 => Node::Empty,
            1 => pieces.remove(0),
            _ => Node::Concat(pieces),
        })
    }

    /// `atom quantifier?`
    fn piece(&mut self, depth: usize) -> Result<Node, PatternError> {
        let atom = self.atom(depth)?;
        let Some((min, max)) = self.quantifier()? else { return Ok(atom) };
        if matches!(self.peek(), Some('?' | '*' | '+' | '{')) {
            return self.error(self.pos, ErrorKind::DoubleQuantifier);
        }
        Ok(Node::Repeat { node: Box::new(atom), min, max })
    }

    fn atom(&mut self, depth: usize) -> Result<Node, PatternError> {
        let start = self.pos;
        let Some(c) = self.peek() else { return self.error(start, ErrorKind::UnclosedGroup) };
        self.pos += 1;
        match c {
            '(' => {
                if depth + 1 > MAX_DEPTH {
                    return self.error(start, ErrorKind::TooDeep);
                }
                let inner = self.alternation(depth + 1)?;
                if self.peek() != Some(')') {
                    return self.error(start, ErrorKind::UnclosedGroup);
                }
                self.pos += 1;
                Ok(inner)
            }
            '[' => Ok(Node::Class(self.class(depth + 1, start)?)),
            '.' => Ok(Node::Class(negated(&from_ranges(&[(0x0A, 0x0A), (0x0D, 0x0D)])))),
            '\\' => Ok(Node::Class(match self.escape(start)? {
                Item::Char(c) => single(c),
                Item::Set(set) => set,
            })),
            '?' | '*' | '+' => self.error(start, ErrorKind::NothingToRepeat(c)),
            '{' | '}' | ']' => self.error(start, ErrorKind::NeedsEscape(c)),
            other => Ok(Node::Class(single(other))),
        }
    }

    /// `?`, `*`, `+`, `{n}`, `{n,}` or `{n,m}`, if there is one.
    fn quantifier(&mut self) -> Result<Option<(u32, Option<u32>)>, PatternError> {
        let start = self.pos;
        let counts = match self.peek() {
            Some('?') => (0, Some(1)),
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('{') => {
                self.pos += 1;
                let min = self.count(start)?;
                let max = if self.peek() == Some(',') {
                    self.pos += 1;
                    if self.peek() == Some('}') { None } else { Some(self.count(start)?) }
                } else {
                    Some(min)
                };
                if self.peek() != Some('}') {
                    return self.error(start, ErrorKind::BadQuantifier);
                }
                if max.is_some_and(|m| m < min) {
                    return self.error(start, ErrorKind::QuantifierRange);
                }
                (min, max)
            }
            _ => return Ok(None),
        };
        self.pos += 1;
        Ok(Some(counts))
    }

    /// Decimal digits, at least one.
    fn count(&mut self, start: usize) -> Result<u32, PatternError> {
        let mut value: u32 = 0;
        let mut any = false;
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(10)) {
            any = true;
            value = value.saturating_mul(10).saturating_add(digit);
            if value > MAX_COUNT {
                return self.error(start, ErrorKind::QuantifierTooLarge);
            }
            self.pos += 1;
        }
        if any { Ok(value) } else { self.error(start, ErrorKind::BadQuantifier) }
    }

    /// After a `\`: a single-character escape, or a set.
    fn escape(&mut self, start: usize) -> Result<Item, PatternError> {
        let Some(c) = self.peek() else { return self.error(start, ErrorKind::TrailingBackslash) };
        self.pos += 1;
        match c {
            'n' => Ok(Item::Char('\n')),
            'r' => Ok(Item::Char('\r')),
            't' => Ok(Item::Char('\t')),
            '\\' | '|' | '.' | '?' | '*' | '+' | '(' | ')' | '{' | '}' | '-' | '[' | ']' | '^' => Ok(Item::Char(c)),
            's' | 'S' | 'i' | 'I' | 'c' | 'C' | 'd' | 'D' | 'w' | 'W' => match multi_char(c) {
                Some(set) => Ok(Item::Set(set)),
                None => self.error(start, ErrorKind::UnknownEscape(c)),
            },
            'p' | 'P' => {
                if self.peek() != Some('{') {
                    return self.error(start, ErrorKind::BadProperty);
                }
                self.pos += 1;
                let from = self.pos;
                while self.peek().is_some_and(|c| c != '}') {
                    self.pos += 1;
                }
                if self.peek() != Some('}') || self.pos == from {
                    return self.error(start, ErrorKind::BadProperty);
                }
                let name: String = self.chars.get(from..self.pos).unwrap_or_default().iter().collect();
                self.pos += 1;
                let set = if name.starts_with("Is") {
                    block_set(&name).ok_or_else(|| PatternError::new(start, ErrorKind::UnknownBlock(name.clone())))?
                } else {
                    category(&name).ok_or_else(|| PatternError::new(start, ErrorKind::UnknownCategory(name.clone())))?
                };
                Ok(Item::Set(if c == 'P' { negated(&set) } else { set }))
            }
            other => self.error(start, ErrorKind::UnknownEscape(other)),
        }
    }

    /// The inside of `[…]`, after the `[` at `start`, through the closing `]`.
    fn class(&mut self, depth: usize, start: usize) -> Result<ClassUnicode, PatternError> {
        if depth > MAX_DEPTH {
            return self.error(start, ErrorKind::TooDeep);
        }
        let negate = self.peek() == Some('^');
        if negate {
            self.pos += 1;
        }
        let mut set = ClassUnicode::empty();
        let mut first = true;
        loop {
            let at = self.pos;
            let Some(c) = self.peek() else { return self.error(start, ErrorKind::UnclosedClass) };
            match c {
                ']' => {
                    if first {
                        return self.error(start, ErrorKind::EmptyClass);
                    }
                    self.pos += 1;
                    break;
                }
                '[' => return self.error(at, ErrorKind::NeedsEscape('[')),
                '-' if self.peek_at(1) == Some('[') && !first => {
                    // `-[…]` takes a class out of this one, and ends it.
                    self.pos += 2;
                    let sub = self.class(depth + 1, at + 1)?;
                    let mut base = if negate { negated(&set) } else { set };
                    base.difference(&sub);
                    if self.peek() != Some(']') {
                        return self.error(self.pos, ErrorKind::SubtractionNotLast);
                    }
                    self.pos += 1;
                    return Ok(base);
                }
                '-' if first || self.peek_at(1) == Some(']') => {
                    self.pos += 1;
                    add_range(&mut set, '-' as u32, '-' as u32);
                }
                '-' if self.peek_at(1).is_none() => return self.error(start, ErrorKind::UnclosedClass),
                '-' => return self.error(at, ErrorKind::BadRange),
                _ => {
                    let item = self.class_item(at)?;
                    match item {
                        Item::Set(more) => {
                            if self.peek() == Some('-') && !matches!(self.peek_at(1), Some(']' | '[')) {
                                return self.error(self.pos, ErrorKind::BadRange);
                            }
                            set.union(&more);
                        }
                        Item::Char(lo) => {
                            if self.peek() == Some('-') && !matches!(self.peek_at(1), Some(']' | '[') | None) {
                                let dash = self.pos;
                                self.pos += 1;
                                let end = self.pos;
                                let hi = match self.peek() {
                                    Some('-' | '[' | ']') | None => return self.error(dash, ErrorKind::BadRange),
                                    Some(_) => match self.class_item(end)? {
                                        Item::Char(hi) => hi,
                                        Item::Set(_) => return self.error(dash, ErrorKind::BadRange),
                                    },
                                };
                                if hi < lo {
                                    return self.error(dash, ErrorKind::BadRange);
                                }
                                add_range(&mut set, lo as u32, hi as u32);
                            } else {
                                add_range(&mut set, lo as u32, lo as u32);
                            }
                        }
                    }
                }
            }
            first = false;
        }
        Ok(if negate { negated(&set) } else { set })
    }

    /// One character or escape inside a class.
    fn class_item(&mut self, at: usize) -> Result<Item, PatternError> {
        let Some(c) = self.peek() else { return self.error(at, ErrorKind::UnclosedClass) };
        self.pos += 1;
        if c == '\\' { self.escape(at) } else { Ok(Item::Char(c)) }
    }
}
