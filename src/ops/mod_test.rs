use super::*;
use std::cell::Cell;

#[test]
fn or_put_back_leaves_a_step_that_worked_alone() {
    let called = Cell::new(false);
    let value = or_put_back(Ok(7), || {
        called.set(true);
        Ok(())
    })
    .unwrap();
    assert_eq!(value, 7);
    assert!(!called.get());
}

#[test]
fn or_put_back_puts_back_and_keeps_the_error_of_the_step() {
    let called = Cell::new(false);
    let err = or_put_back(Err::<(), _>(Error::msg("the step failed")), || {
        called.set(true);
        Ok(())
    })
    .unwrap_err();
    assert!(called.get());
    assert_eq!(err.to_string(), "the step failed");
}

#[test]
fn or_put_back_names_both_when_putting_back_fails_too() {
    let err = or_put_back(Err::<(), _>(Error::msg("the step failed")), || {
        Err(Error::msg("reset failed"))
    })
    .unwrap_err()
    .to_string();
    assert!(err.starts_with("the step failed\n"), "{err}");
    assert!(err.contains("reset failed"), "{err}");
}
