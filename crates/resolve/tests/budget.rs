//! The budget of one load.

use xmlcraft_resolve::{Budget, MAX_RESOURCES, MAX_TOTAL_BYTES};

#[test]
fn a_load_may_use_exactly_what_it_is_given() {
    let mut budget = Budget::with_limits(3, 10);
    assert!(budget.charge(4).is_ok());
    assert!(budget.charge(6).is_ok(), "exactly the bytes there are");
    assert!(budget.charge(1).is_err(), "one byte more");
    assert!(budget.charge(0).is_ok(), "an empty resource costs a resource, not bytes");
    assert!(budget.charge(0).is_err(), "and the resources are used up");
    assert_eq!((budget.resources(), budget.bytes()), (3, 10));
}

#[test]
fn the_default_is_the_limit_of_the_design() {
    let mut budget = Budget::new();
    for _ in 0..MAX_RESOURCES {
        assert!(budget.charge(1).is_ok());
    }
    assert!(budget.charge(1).is_err());
    let mut big = Budget::new();
    assert!(big.charge(MAX_TOTAL_BYTES).is_ok());
    assert!(big.charge(1).is_err());
    assert!(Budget::new().charge(MAX_TOTAL_BYTES + 1).is_err());
}

#[test]
fn nothing_overflows() {
    let mut budget = Budget::with_limits(10, usize::MAX);
    assert!(budget.charge(usize::MAX).is_ok());
    assert!(budget.charge(1).is_err());
    assert_eq!(budget.bytes(), usize::MAX);
}
