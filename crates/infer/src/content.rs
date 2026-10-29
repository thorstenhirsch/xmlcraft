//! The content model of an element, learned from the order and number of its children in the documents.
//!
//! The model is a sequence of *factors*; a factor is a choice of one or more child names, repeated between a least and a most number of
//! times (`a`, `a?`, `(b|c)+`, …). Every child name is in exactly one factor, so the model is deterministic (no Unique Particle
//! Attribution problem), and it accepts every element the documents had: the factors are put in an order every element keeps, and each
//! factor's bounds are the least and the greatest number of its names any element had. This is the class of "chain regular expressions"
//! that schema inference from XML has used (the algorithm here is derived from the published description: the order is the transitive
//! closure of "occurs before", names that occur before each other are one factor, names that never occur together are alternatives).

use crate::observe::{ChildStats, MAX_ORDERED_SYMBOLS};

/// A choice of names, with the number of times it occurs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Factor {
    /// Indices into [`ChildStats::symbols`], in the order first seen.
    pub symbols: Vec<usize>,
    /// The fewest and the most times the choice occurs in an element (the total over its names).
    pub min: usize,
    pub max: usize,
}

impl Factor {
    pub fn optional(&self) -> bool {
        self.min == 0
    }

    pub fn repeats(&self) -> bool {
        self.max > 1
    }
}

/// The sequence of factors that fits the children of an element; empty when no element had a child.
pub fn factors(stats: &ChildStats) -> Vec<Factor> {
    let n = stats.symbols.len();
    if n == 0 {
        return Vec::new();
    }
    if stats.order_unknown || n > MAX_ORDERED_SYMBOLS {
        // The order was not learned: any of the names, any number of times.
        return vec![Factor { symbols: (0..n).collect(), min: 0, max: usize::MAX }];
    }
    // `reach[a]` is the set of names an `a` comes before (in some element), transitively.
    let mut reach = vec![0u64; n];
    for &(a, b) in &stats.before {
        if let Some(row) = reach.get_mut(a) {
            *row |= 1u64 << b.min(63);
        }
    }
    for k in 0..n {
        let via = reach.get(k).copied().unwrap_or(0);
        for i in 0..n {
            if reach.get(i).is_some_and(|row| row & (1u64 << k) != 0)
                && let Some(row) = reach.get_mut(i)
            {
                *row |= via;
            }
        }
    }
    // Names that come before each other (and a name that comes before itself) are one class.
    let class_of = |a: usize| -> u64 {
        let ra = reach.get(a).copied().unwrap_or(0);
        (0..n)
            .filter(|&b| b == a || (ra & (1u64 << b) != 0 && reach.get(b).is_some_and(|rb| rb & (1u64 << a) != 0)))
            .fold(0u64, |m, b| m | (1u64 << b))
    };
    let mut classes: Vec<u64> = Vec::new();
    for a in 0..n {
        let class = class_of(a);
        if !classes.contains(&class) {
            classes.push(class);
        }
    }
    // A class comes before another when any of its names comes before any of the other's. The number of classes before a class orders
    // them (a class that is before another has fewer before it); classes with the same number, none before the other, are alternatives.
    let before = |c: u64, d: u64| -> bool { (0..n).any(|a| c & (1u64 << a) != 0 && reach.get(a).is_some_and(|r| r & d != 0)) && c != d };
    let rank_of = |c: u64| -> usize { classes.iter().filter(|&&other| other != c && before(other, c)).count() };
    let mut ranked: Vec<(usize, u64)> = classes.iter().map(|&c| (rank_of(c), c)).collect();
    ranked.sort_by_key(|&(rank, class)| (rank, class.trailing_zeros()));
    // Group by rank, then merge a group whose classes are all unrelated into one factor (they never occur together).
    let mut groups: Vec<(usize, u64)> = Vec::new();
    for (rank, class) in ranked {
        match groups.last_mut() {
            Some((last_rank, mask)) if *last_rank == rank => *mask |= class,
            _ => groups.push((rank, class)),
        }
    }
    groups
        .into_iter()
        .map(|(_, mask)| {
            let symbols: Vec<usize> = (0..n).filter(|a| mask & (1u64 << a) != 0).collect();
            let (min, max) = bounds(stats, &symbols);
            Factor { symbols, min, max }
        })
        .collect()
}

/// The fewest and the most times the names of a factor occur together in one element.
fn bounds(stats: &ChildStats, symbols: &[usize]) -> (usize, usize) {
    if stats.signatures_overflow {
        return (0, usize::MAX);
    }
    let mut least = usize::MAX;
    let mut most = 0usize;
    for signature in stats.signatures.keys() {
        let total: usize = signature.iter().filter(|(s, _)| symbols.contains(s)).fold(0usize, |t, (_, c)| t.saturating_add(*c));
        least = least.min(total);
        most = most.max(total);
    }
    if least == usize::MAX {
        least = 0;
    }
    (least, most)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::{ChildStats, Name};

    /// The factors of the children that these words (each a string of one-letter names) show.
    fn learn(words: &[&str]) -> (ChildStats, Vec<String>) {
        let mut stats = ChildStats::default();
        for word in words {
            // `observe` is private to the crate's observation; go through the same entry the documents do.
            stats.observe_names(word.chars().map(|c| Name::new(None, &c.to_string())));
        }
        let shown = factors(&stats).iter().map(|f| render(&stats, f)).collect();
        (stats, shown)
    }

    fn render(stats: &ChildStats, f: &Factor) -> String {
        let names: Vec<&str> = f.symbols.iter().filter_map(|&s| stats.symbols.get(s)).map(|n| n.local.as_str()).collect();
        let body = if names.len() == 1 { names[0].to_owned() } else { format!("({})", names.join("|")) };
        let suffix = match (f.optional(), f.repeats()) {
            (false, false) => "",
            (true, false) => "?",
            (false, true) => "+",
            (true, true) => "*",
        };
        format!("{body}{suffix}")
    }

    #[test]
    fn a_fixed_sequence() {
        assert_eq!(learn(&["abc", "abc"]).1, ["a", "b", "c"]);
    }

    #[test]
    fn optional_and_repeated_names() {
        assert_eq!(learn(&["ab", "abb", "a"]).1, ["a", "b*"]);
        assert_eq!(learn(&["abb", "abbb"]).1, ["a", "b+"]);
        assert_eq!(learn(&["ac", "abc"]).1, ["a", "b?", "c"]);
    }

    #[test]
    fn names_that_never_occur_together_are_alternatives() {
        assert_eq!(learn(&["abd", "acd"]).1, ["a", "(b|c)", "d"]);
        assert_eq!(learn(&["a", "b"]).1, ["(a|b)"]);
        assert_eq!(learn(&["a", "b", ""]).1, ["(a|b)?"]);
    }

    #[test]
    fn names_that_come_in_any_order_are_one_repeated_choice() {
        assert_eq!(learn(&["abab", "ba"]).1, ["(a|b)+"]);
        assert_eq!(learn(&["ab", "ba"]).1, ["(a|b)+"]);
    }

    #[test]
    fn no_children_is_no_factor_and_an_element_with_none_makes_all_optional() {
        assert_eq!(learn(&["", ""]).1, Vec::<String>::new());
        assert_eq!(learn(&["ab", ""]).1, ["a?", "b?"]);
    }

    #[test]
    fn every_name_is_in_exactly_one_factor() {
        let (stats, _) = learn(&["abcd", "acbd", "dab", "cc"]);
        let all: Vec<usize> = factors(&stats).iter().flat_map(|f| f.symbols.clone()).collect();
        let mut sorted = all.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(all.len(), sorted.len(), "no name twice");
        assert_eq!(sorted.len(), stats.symbols.len(), "no name missing");
    }

    #[test]
    fn more_names_than_the_order_is_learned_for_are_a_free_choice() {
        let words: Vec<String> = (0..70).map(|i| format!("{}", char::from_u32(0x100 + i).unwrap_or('x'))).collect();
        let mut stats = ChildStats::default();
        let one: Vec<Name> = words.iter().map(|w| Name::new(None, w)).collect();
        stats.observe_names(one.into_iter());
        let f = factors(&stats);
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].symbols.len(), f[0].min, f[0].max), (70, 0, usize::MAX));
    }
}
