//! Content-model automata: stepping, what is expected, how to finish, determinism, counts and limits.

use xmlcraft_grammar::{Ambiguity, Automaton, CompileError, MAX_DEPTH, MAX_POSITIONS, Name, NamespaceConstraint, Occurs, Particle, State, Term};

fn el(name: &str, token: usize) -> Particle {
    Particle::element(Name::local(name), token)
}

/// `(a, b?, c*)`
fn abc() -> Particle {
    Particle::sequence(vec![el("a", 0), el("b", 1).occurring(Occurs::OPTIONAL), el("c", 2).occurring(Occurs::ZERO_OR_MORE)])
}

/// Read the names; the state after the last, or the position of the first that was refused.
fn read(automaton: &Automaton, names: &[&str]) -> Result<State, usize> {
    let mut state = automaton.start();
    for (i, name) in names.iter().enumerate() {
        state = automaton.step(&state, &Name::local(name)).ok_or(i)?.state;
    }
    Ok(state)
}

fn accepts(automaton: &Automaton, names: &[&str]) -> bool {
    read(automaton, names).is_ok_and(|state| automaton.is_accepting(&state))
}

fn compile(p: &Particle) -> Automaton {
    Automaton::compile(p).expect("compiles")
}

#[test]
fn a_sequence_with_optional_and_repeated_parts_is_read_step_by_step() {
    let m = compile(&abc());
    assert!(accepts(&m, &["a"]) && accepts(&m, &["a", "b"]) && accepts(&m, &["a", "c", "c", "c"]) && accepts(&m, &["a", "b", "c"]));
    assert!(!accepts(&m, &[]) && !accepts(&m, &["b"]) && !accepts(&m, &["a", "b", "b"]) && !accepts(&m, &["a", "c", "b"]));
    // The first element that cannot come is the one refused.
    assert_eq!(read(&m, &["a", "c", "b", "c"]).err(), Some(2));
    assert_eq!(read(&m, &["x"]).err(), Some(0));
    // Not complete is not refused.
    assert!(read(&m, &[]).is_ok_and(|s| !m.is_accepting(&s)));
}

#[test]
fn a_step_says_which_particle_took_the_element() {
    let m = compile(&abc());
    let mut state = m.start();
    let mut tokens = Vec::new();
    for name in ["a", "b", "c", "c"] {
        let step = m.step(&state, &Name::local(name)).expect("steps");
        tokens.push(step.token);
        state = step.state;
    }
    assert_eq!(tokens, [0, 1, 2, 2]);
}

#[test]
fn what_may_come_next_and_what_is_mandatory() {
    let m = compile(&abc());
    let names = |state: &State| -> Vec<(String, bool)> { m.expected(state).iter().map(|e| (e.term.to_string(), e.mandatory)).collect() };
    assert_eq!(names(&m.start()), [("a".to_owned(), true)]);
    let after_a = read(&m, &["a"]).expect("reads");
    // Complete already: nothing is mandatory.
    assert_eq!(names(&after_a), [("b".to_owned(), false), ("c".to_owned(), false)]);
    // `(a, (b | c), d)`: after a, one of b and c must come, neither alone is mandatory; d is not next yet.
    let m = compile(&Particle::sequence(vec![el("a", 0), Particle::choice(vec![el("b", 1), el("c", 2)]), el("d", 3)]));
    let after_a = read(&m, &["a"]).expect("reads");
    let expected = m.expected(&after_a);
    assert_eq!(
        expected.iter().map(|e| (e.term.to_string(), e.mandatory, e.token)).collect::<Vec<_>>(),
        [("b".to_owned(), false, 1), ("c".to_owned(), false, 2)]
    );
    // `(a, b?, c)`: after a, c is mandatory and b is not.
    let m = compile(&Particle::sequence(vec![el("a", 0), el("b", 1).occurring(Occurs::OPTIONAL), el("c", 2)]));
    let after_a = read(&m, &["a"]).expect("reads");
    assert_eq!(
        m.expected(&after_a).iter().map(|e| (e.term.to_string(), e.mandatory)).collect::<Vec<_>>(),
        [("b".to_owned(), false), ("c".to_owned(), true)]
    );
    // A term that is in the model twice is said once.
    let m = compile(&Particle::sequence(vec![el("a", 0), el("b", 1).occurring(Occurs::OPTIONAL), el("a", 2)]));
    let after_a = read(&m, &["a"]).expect("reads");
    assert_eq!(m.expected(&after_a).len(), 2);
}

#[test]
fn the_shortest_way_to_finish() {
    let m = compile(&Particle::sequence(vec![
        el("a", 0),
        Particle::choice(vec![Particle::sequence(vec![el("x", 1), el("y", 2)]), el("z", 3)]),
        el("d", 4).occurring(Occurs::new(2, Some(5))),
    ]));
    let names = |state: &State| m.shortest_completion(state).map(|c| c.iter().map(|s| s.term.to_string()).collect::<Vec<_>>());
    assert_eq!(names(&m.start()), Some(["a", "z", "d", "d"].map(str::to_owned).to_vec()));
    assert_eq!(names(&read(&m, &["a", "x"]).expect("reads")), Some(["y", "d", "d"].map(str::to_owned).to_vec()));
    assert_eq!(names(&read(&m, &["a", "z", "d", "d"]).expect("reads")), Some(Vec::new()));
    // The completion is accepted.
    let done: Vec<String> = m.shortest_completion(&m.start()).expect("a way").iter().map(|s| s.term.to_string()).collect();
    assert!(accepts(&m, &done.iter().map(String::as_str).collect::<Vec<_>>()));
    // The tokens come with it.
    assert_eq!(m.shortest_completion(&m.start()).expect("a way").iter().map(|s| s.token).collect::<Vec<_>>(), [0, 3, 4, 4]);
}

#[test]
fn counts_are_unrolled() {
    let counted = |min, max| compile(&el("a", 0).occurring(Occurs::new(min, max)));
    let lengths = |m: &Automaton| -> Vec<usize> { (0..8).filter(|&n| accepts(m, &vec!["a"; n])).collect() };
    assert_eq!(lengths(&counted(2, Some(4))), [2, 3, 4]);
    assert_eq!(lengths(&counted(0, Some(2))), [0, 1, 2]);
    assert_eq!(lengths(&counted(3, None)), [3, 4, 5, 6, 7]);
    assert_eq!(lengths(&counted(1, None)), [1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(lengths(&counted(0, None)), (0..8).collect::<Vec<_>>());
    assert_eq!(lengths(&counted(0, Some(0))), [0], "an element that may occur no times is not in the model");
    assert_eq!(lengths(&counted(1, Some(1))), [1]);
    // Counts of a group.
    let m = compile(&Particle::sequence(vec![el("a", 0), el("b", 1)]).occurring(Occurs::new(2, Some(3))));
    assert!(accepts(&m, &["a", "b", "a", "b"]) && accepts(&m, &["a", "b", "a", "b", "a", "b"]));
    assert!(!accepts(&m, &["a", "b"]) && !accepts(&m, &["a", "b", "a", "b", "a", "b", "a", "b"]));
    // A group that is nullable inside a count.
    let m = compile(&Particle::sequence(vec![el("a", 0).occurring(Occurs::OPTIONAL)]).occurring(Occurs::new(2, None)));
    assert!(accepts(&m, &[]) && accepts(&m, &["a", "a", "a"]));
}

#[test]
fn nothing_and_the_empty_sequence() {
    let empty = compile(&Particle::sequence(vec![]));
    assert!(accepts(&empty, &[]) && !accepts(&empty, &["a"]));
    assert!(!empty.matches_nothing());
    // A choice of nothing allows nothing, and what follows it cannot be reached.
    let none = compile(&Particle::choice(vec![]));
    assert!(none.matches_nothing() && !accepts(&none, &[]) && !accepts(&none, &["a"]));
    let sequence = compile(&Particle::sequence(vec![el("a", 0), Particle::choice(vec![]), el("b", 1)]));
    assert!(sequence.matches_nothing());
    // Unless it may occur no times.
    let optional = compile(&Particle::sequence(vec![el("a", 0), Particle::choice(vec![]).occurring(Occurs::OPTIONAL), el("b", 1)]));
    assert!(accepts(&optional, &["a", "b"]) && !accepts(&optional, &["a"]));
    // A step that works is on the way to a valid content: `a` followed by nothing possible is refused at once.
    let dead = compile(&Particle::choice(vec![Particle::sequence(vec![el("a", 0), Particle::choice(vec![])]), el("b", 1)]));
    assert_eq!(read(&dead, &["a"]).err(), Some(0));
    assert!(accepts(&dead, &["b"]));
    // Empty groups, however often they occur, are the empty sequence.
    let many = compile(&Particle::sequence(vec![Particle::sequence(vec![]).occurring(Occurs::new(1_000_000_000, Some(u32::MAX))), el("a", 0)]));
    assert!(accepts(&many, &["a"]));
    assert!(compile(&Particle::choice(vec![]).occurring(Occurs::new(3, None))).matches_nothing());
}

#[test]
fn a_model_that_is_not_deterministic_is_found_and_both_particles_are_named() {
    let ambiguous = |p: &Particle| Automaton::compile(p).expect("compiles").check_deterministic();
    let ok = |p: &Particle| assert_eq!(ambiguous(p), Ok(()));
    ok(&abc());
    ok(&Particle::sequence(vec![el("a", 0), el("b", 1)]).occurring(Occurs::ONE_OR_MORE));
    ok(&el("a", 0).occurring(Occurs::new(2, Some(5))));
    ok(&Particle::choice(vec![el("a", 0), el("b", 1), el("c", 2)]).occurring(Occurs::ZERO_OR_MORE));
    // `(a | a)`, `(a?, a)`, `(a*, a)`, `((a, b) | (a, c))`.
    assert_eq!(
        ambiguous(&Particle::choice(vec![el("a", 0), el("a", 1)])),
        Err(Ambiguity { first: 0, second: 1, term: Term::Element(Name::local("a")) })
    );
    assert_eq!(
        ambiguous(&Particle::sequence(vec![el("a", 0).occurring(Occurs::OPTIONAL), el("a", 1)])),
        Err(Ambiguity { first: 0, second: 1, term: Term::Element(Name::local("a")) })
    );
    assert_eq!(
        ambiguous(&Particle::sequence(vec![el("a", 0).occurring(Occurs::ZERO_OR_MORE), el("a", 1)])),
        Err(Ambiguity { first: 0, second: 1, term: Term::Element(Name::local("a")) })
    );
    assert!(
        ambiguous(&Particle::choice(vec![Particle::sequence(vec![el("a", 0), el("b", 1)]), Particle::sequence(vec![el("a", 2), el("c", 3)])]))
            .is_err()
    );
    // A counted group whose copies compete: `(a?){1,3}`.
    assert!(ambiguous(&Particle::sequence(vec![el("a", 0).occurring(Occurs::OPTIONAL)]).occurring(Occurs::new(1, Some(3)))).is_err());
    // The same when the whole count is optional, and when the competing copies are of a group.
    assert!(ambiguous(&Particle::sequence(vec![el("a", 0).occurring(Occurs::OPTIONAL)]).occurring(Occurs::new(0, Some(3)))).is_err());
    assert!(ambiguous(&Particle::sequence(vec![el("a", 0), el("b", 1).occurring(Occurs::OPTIONAL)]).occurring(Occurs::new(0, Some(3)))).is_ok());
    // Wildcards compete with the elements they let in, and with each other.
    let any = || Particle::wildcard(NamespaceConstraint::Any, 9);
    assert_eq!(
        ambiguous(&Particle::sequence(vec![el("a", 0).occurring(Occurs::OPTIONAL), any()])),
        Err(Ambiguity { first: 0, second: 9, term: Term::Element(Name::local("a")) })
    );
    let ns = |n: &str| Particle::wildcard(NamespaceConstraint::Set(vec![Some(n.to_owned())]), 5);
    ok(&Particle::choice(vec![ns("urn:a"), ns("urn:b")]));
    assert!(ambiguous(&Particle::choice(vec![ns("urn:a"), ns("urn:a")])).is_err());
    ok(&Particle::sequence(vec![Particle::element(Name::new(Some("urn:x"), "e"), 0).occurring(Occurs::OPTIONAL), ns("urn:y")]));
}

#[test]
fn wildcards_take_the_elements_of_the_namespaces_they_let_in() {
    let wildcard = |c: NamespaceConstraint| {
        compile(&Particle::sequence(vec![Particle::element(Name::local("head"), 0), Particle::wildcard(c, 1).occurring(Occurs::ZERO_OR_MORE)]))
    };
    let takes = |m: &Automaton, ns: Option<&str>| {
        let head = m.step(&m.start(), &Name::local("head")).expect("head").state;
        m.step(&head, &Name::new(ns, "x")).map(|s| s.token)
    };
    let any = wildcard(NamespaceConstraint::Any);
    assert_eq!((takes(&any, None), takes(&any, Some("urn:a"))), (Some(1), Some(1)));
    let other = wildcard(NamespaceConstraint::Other(Some("urn:t".to_owned())));
    assert_eq!((takes(&other, None), takes(&other, Some("urn:t")), takes(&other, Some("urn:a"))), (None, None, Some(1)));
    let other_of_none = wildcard(NamespaceConstraint::Other(None));
    assert_eq!((takes(&other_of_none, None), takes(&other_of_none, Some("urn:a"))), (None, Some(1)));
    let set = wildcard(NamespaceConstraint::Set(vec![None, Some("urn:a".to_owned())]));
    assert_eq!((takes(&set, None), takes(&set, Some("urn:a")), takes(&set, Some("urn:b"))), (Some(1), Some(1), None));
    let nothing = wildcard(NamespaceConstraint::Set(vec![]));
    assert_eq!(takes(&nothing, None), None);
    // What is expected says a wildcard is a wildcard.
    let head = any.step(&any.start(), &Name::local("head")).expect("head").state;
    assert!(matches!(any.expected(&head).first().map(|e| e.term), Some(Term::Wildcard(NamespaceConstraint::Any))));
}

#[test]
fn long_lists_are_looked_up_by_name() {
    // `(e0 | e1 | … | e199 | ##other)*`
    let mut items: Vec<Particle> = (0..200).map(|i| el(&format!("e{i}"), i)).collect();
    items.push(Particle::wildcard(NamespaceConstraint::Other(None), 1000));
    let m = compile(&Particle::choice(items).occurring(Occurs::ZERO_OR_MORE));
    let mut state = m.start();
    for (name, token) in [("e0", 0), ("e199", 199), ("e57", 57), ("e57", 57)] {
        let step = m.step(&state, &Name::local(name)).expect("takes it");
        assert_eq!(step.token, token);
        state = step.state;
    }
    // An element of a namespace falls to the wildcard; one in none, that is not listed, is refused.
    assert_eq!(m.step(&state, &Name::new(Some("urn:other"), "e0")).map(|s| s.token), Some(1000));
    assert_eq!(m.step(&state, &Name::local("nope")).map(|s| s.token), None);
    assert!(m.is_accepting(&state));
    // The wildcard lets in only elements in a namespace, so it does not compete with the listed ones.
    assert!(m.check_deterministic().is_ok());
}

#[test]
fn a_model_with_a_bad_count_is_refused() {
    assert_eq!(Automaton::compile(&el("a", 0).occurring(Occurs::new(3, Some(2)))).err(), Some(CompileError::BadOccurs { min: 3, max: 2 }));
    // Wherever it is.
    let nested = Particle::sequence(vec![el("x", 0), Particle::choice(vec![el("a", 1).occurring(Occurs::new(2, Some(1)))])]);
    assert!(matches!(Automaton::compile(&nested), Err(CompileError::BadOccurs { .. })));
}

#[test]
fn a_model_that_is_too_big_or_too_deep_is_refused_quickly() {
    let started = std::time::Instant::now();
    assert_eq!(Automaton::compile(&el("a", 0).occurring(Occurs::new(1, Some(MAX_POSITIONS as u32 + 1)))).err(), Some(CompileError::TooLarge));
    assert_eq!(Automaton::compile(&el("a", 0).occurring(Occurs::new(u32::MAX, None))).err(), Some(CompileError::TooLarge));
    assert_eq!(Automaton::compile(&el("a", 0).occurring(Occurs::new(0, Some(u32::MAX)))).err(), Some(CompileError::TooLarge));
    // Counts inside counts multiply.
    let nested = el("a", 0).occurring(Occurs::new(1, Some(1000)));
    let nested = Particle::sequence(vec![nested]).occurring(Occurs::new(1, Some(1000)));
    let nested = Particle::sequence(vec![nested]).occurring(Occurs::new(1, Some(1000)));
    assert_eq!(Automaton::compile(&nested).err(), Some(CompileError::TooLarge));
    // As many as allowed is fine, and is a flat chain, not a deep recursion.
    let big = Automaton::compile(&el("a", 0).occurring(Occurs::new(0, Some(MAX_POSITIONS as u32)))).expect("fits");
    assert_eq!(big.position_count(), MAX_POSITIONS);
    assert!(accepts(&big, &vec!["a"; 5000]));
    assert!(!accepts(&big, &vec!["a"; MAX_POSITIONS + 1]));
    // So many nullable copies that the follow lists would be enormous: refused, not slow.
    let nullable = Particle::sequence(vec![el("a", 0).occurring(Occurs::OPTIONAL)]).occurring(Occurs::new(0, Some(50_000)));
    assert_eq!(Automaton::compile(&nullable).err(), Some(CompileError::TooLarge));
    // Nesting.
    let mut deep = el("a", 0);
    for _ in 0..MAX_DEPTH + 10 {
        deep = Particle::sequence(vec![deep]);
    }
    assert_eq!(Automaton::compile(&deep).err(), Some(CompileError::TooDeep));
    let mut fine = el("a", 0);
    for _ in 0..MAX_DEPTH - 1 {
        fine = Particle::sequence(vec![fine]);
    }
    assert!(Automaton::compile(&fine).is_ok());
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
}

#[test]
fn a_deterministic_model_is_at_one_position() {
    let m =
        compile(&Particle::sequence(vec![el("a", 0), Particle::choice(vec![el("b", 1), el("c", 2)]).occurring(Occurs::ZERO_OR_MORE), el("d", 3)]));
    assert!(m.check_deterministic().is_ok());
    let mut state = m.start();
    for name in ["a", "b", "c", "b", "d"] {
        state = m.step(&state, &Name::local(name)).expect("steps").state;
        assert_eq!(state.alternatives(), 1);
    }
    // A model that is not deterministic is still read, at several positions.
    let m = compile(&Particle::sequence(vec![el("a", 0).occurring(Occurs::ZERO_OR_MORE), el("a", 1), el("b", 2)]));
    let state = m.step(&m.start(), &Name::local("a")).expect("steps").state;
    assert_eq!(state.alternatives(), 2);
    assert!(accepts(&m, &["a", "a", "a", "b"]) && !accepts(&m, &["a", "a"]) && accepts(&m, &["a", "b"]));
}
