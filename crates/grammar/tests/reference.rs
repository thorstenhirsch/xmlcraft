//! The automaton against a matcher written straight from the meaning of a content model: the set of places in the input where a
//! particle can end, counting its occurrences by iteration. The two agree on which sequences are valid, on which prefixes can still
//! become valid, on what may come next and what is mandatory, and on the shortest way to finish.

use std::collections::BTreeSet;

use proptest::prelude::*;
use xmlcraft_grammar::{Automaton, Name, Occurs, Particle, ParticleKind, Term};

const ALPHABET: [&str; 3] = ["a", "b", "c"];

/// The places where a match of `p` that starts at `from` can end.
fn ends(p: &Particle, input: &[&str], from: usize) -> BTreeSet<usize> {
    let once = |at: usize| -> BTreeSet<usize> {
        match &p.kind {
            ParticleKind::Term { term: Term::Element(name), .. } => {
                if input.get(at).is_some_and(|word| *word == name.local) {
                    BTreeSet::from([at + 1])
                } else {
                    BTreeSet::new()
                }
            }
            ParticleKind::Term { .. } => BTreeSet::new(),
            ParticleKind::Sequence(items) => {
                let mut here = BTreeSet::from([at]);
                for item in items {
                    here = here.iter().flat_map(|&i| ends(item, input, i)).collect();
                }
                here
            }
            ParticleKind::Choice(items) => items.iter().flat_map(|item| ends(item, input, at)).collect(),
        }
    };
    let Occurs { min, max } = p.occurs;
    let mut result = BTreeSet::new();
    let mut layer = BTreeSet::from([from]);
    let mut k = 0u32;
    loop {
        if k >= min && max.is_none_or(|m| k <= m) {
            result.extend(layer.iter().copied());
        }
        if max.is_some_and(|m| k >= m) {
            break;
        }
        let next: BTreeSet<usize> = layer.iter().flat_map(|&i| once(i)).collect();
        k += 1;
        // Past the minimum, a layer that adds nothing new adds nothing later either.
        if k >= min && max.is_none() && next.is_subset(&result) {
            break;
        }
        if next.is_empty() {
            break;
        }
        layer = next;
    }
    result
}

fn reference_accepts(p: &Particle, input: &[&str]) -> bool {
    ends(p, input, 0).contains(&input.len())
}

fn occurs() -> impl Strategy<Value = Occurs> {
    prop::sample::select(vec![
        Occurs::ONCE,
        Occurs::ONCE,
        Occurs::OPTIONAL,
        Occurs::ZERO_OR_MORE,
        Occurs::ONE_OR_MORE,
        Occurs::new(2, Some(2)),
        Occurs::new(0, Some(2)),
        Occurs::new(1, Some(3)),
        Occurs::new(2, None),
    ])
}

fn particle() -> impl Strategy<Value = Particle> {
    let leaf =
        (prop::sample::select(ALPHABET.to_vec()), occurs()).prop_map(|(name, occurs)| Particle::element(Name::local(name), 0).occurring(occurs));
    leaf.prop_recursive(3, 14, 3, |inner| {
        prop_oneof![
            (prop::collection::vec(inner.clone(), 0..4), occurs()).prop_map(|(items, occurs)| Particle::sequence(items).occurring(occurs)),
            (prop::collection::vec(inner, 0..4), occurs()).prop_map(|(items, occurs)| Particle::choice(items).occurring(occurs)),
        ]
    })
}

/// Every word over the alphabet of at most `len` letters.
fn words(len: usize) -> Vec<Vec<&'static str>> {
    let mut all: Vec<Vec<&'static str>> = vec![Vec::new()];
    let mut layer: Vec<Vec<&'static str>> = vec![Vec::new()];
    for _ in 0..len {
        layer = layer
            .iter()
            .flat_map(|word| {
                ALPHABET.iter().map(move |letter| {
                    let mut longer = word.clone();
                    longer.push(letter);
                    longer
                })
            })
            .collect();
        all.extend(layer.iter().cloned());
    }
    all
}

fn names(word: &[&str]) -> Vec<Name> {
    word.iter().map(|w| Name::local(w)).collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// The automaton and the reference agree on every word, and a refused step is a prefix nothing can complete.
    #[test]
    fn it_accepts_what_the_reference_accepts_and_refuses_only_what_cannot_be_completed(
        p in particle(),
        word in prop::collection::vec(prop::sample::select(ALPHABET.to_vec()), 0..7),
    ) {
        let Ok(automaton) = Automaton::compile(&p) else { return Ok(()) };
        let mut state = automaton.start();
        let mut refused_at = None;
        for (i, name) in names(&word).iter().enumerate() {
            match automaton.step(&state, name) {
                Some(step) => state = step.state,
                None => {
                    refused_at = Some(i);
                    break;
                }
            }
        }
        match refused_at {
            None => prop_assert_eq!(automaton.is_accepting(&state), reference_accepts(&p, &word), "{:?} on {:?}", p, word),
            Some(i) => {
                prop_assert!(!reference_accepts(&p, &word));
                // No word that starts with what was read and the letter that was refused is valid, however it goes on.
                let prefix: Vec<&str> = word.iter().take(i + 1).copied().collect();
                for suffix in words(3) {
                    let mut whole = prefix.clone();
                    whole.extend(suffix.iter());
                    prop_assert!(!reference_accepts(&p, &whole), "{:?}: {:?} can go on to {:?}", p, prefix, whole);
                }
            }
        }
    }

    /// What is read without being refused can be finished, the shortest way is valid and no way is shorter, and what is expected is
    /// exactly what is not refused; what is mandatory comes in every valid ending.
    #[test]
    fn what_is_expected_and_how_to_finish_agree_with_the_reference(
        p in particle(),
        word in prop::collection::vec(prop::sample::select(ALPHABET.to_vec()), 0..5),
    ) {
        let Ok(automaton) = Automaton::compile(&p) else { return Ok(()) };
        // A model that allows nothing has no way to finish; the reference agrees that it has no word.
        if automaton.matches_nothing() {
            prop_assert!(words(5).iter().all(|w| !reference_accepts(&p, w)));
            return Ok(());
        }
        let mut state = automaton.start();
        for name in names(&word) {
            match automaton.step(&state, &name) {
                Some(step) => state = step.state,
                None => return Ok(()),
            }
        }
        // Expected is what can be stepped over.
        let expected = automaton.expected(&state);
        for letter in ALPHABET {
            let listed = expected.iter().any(|e| e.term == &Term::Element(Name::local(letter)));
            prop_assert_eq!(listed, automaton.step(&state, &Name::local(letter)).is_some(), "{} after {:?} in {:?}", letter, word, p);
        }
        // The shortest completion.
        let completion = automaton.shortest_completion(&state);
        let completion: Vec<&str> = match &completion {
            Some(symbols) => symbols.iter().map(|s| match s.term { Term::Element(n) => n.local.as_str(), Term::Wildcard(_) => "?" }).collect(),
            None => {
                prop_assert!(false, "{:?} read to {:?} cannot be finished", p, word);
                return Ok(());
            }
        };
        let finished = |tail: &[&str]| -> bool {
            let mut whole = word.clone();
            whole.extend(tail.iter());
            reference_accepts(&p, &whole)
        };
        prop_assert!(finished(&completion), "{:?} after {:?}: {:?} does not finish it", p, word, completion);
        prop_assert_eq!(completion.is_empty(), automaton.is_accepting(&state));
        if completion.len() <= 6 {
            for shorter in words(completion.len().saturating_sub(1)).into_iter().filter(|w| w.len() < completion.len()) {
                prop_assert!(!finished(&shorter), "{:?} after {:?}: {:?} is shorter than {:?}", p, word, shorter, completion);
            }
        }
        // Mandatory: in every valid ending that is found.
        for e in expected.iter().filter(|e| e.mandatory) {
            let Term::Element(name) = e.term else { continue };
            for tail in words(4) {
                if finished(&tail) {
                    prop_assert!(tail.contains(&name.local.as_str()), "{} is mandatory after {:?} in {:?} but {:?} finishes it", name, word, p, tail);
                }
            }
        }
    }

    /// A deterministic model is read at one position all the way.
    #[test]
    fn a_deterministic_model_is_never_at_two_positions(
        p in particle(),
        word in prop::collection::vec(prop::sample::select(ALPHABET.to_vec()), 0..7),
    ) {
        let Ok(automaton) = Automaton::compile(&p) else { return Ok(()) };
        if automaton.check_deterministic().is_err() {
            return Ok(());
        }
        let mut state = automaton.start();
        for name in names(&word) {
            match automaton.step(&state, &name) {
                Some(step) => state = step.state,
                None => break,
            }
            prop_assert_eq!(state.alternatives(), 1, "{:?} on {:?}", p, word);
        }
    }

    /// Whatever the model, compiling ends (in a model or an error) and does not panic.
    #[test]
    fn any_count_compiles_or_is_refused(min in 0u32..2000, extra in prop::option::of(0u32..3000), nested in 0usize..3) {
        let mut p = Particle::element(Name::local("a"), 0).occurring(Occurs::new(min, extra.map(|e| min.saturating_add(e))));
        for _ in 0..nested {
            p = Particle::sequence(vec![p, Particle::element(Name::local("b"), 1)]).occurring(Occurs::new(0, Some(3)));
        }
        let _ = Automaton::compile(&p);
    }
}
