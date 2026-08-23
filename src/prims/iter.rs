//! Iterators.
//!
//! An iterator's operand is read as an iterator — its step function and state
//! are its two data words. Asking whether it "really is" one would be a type
//! judgement, and this layer does not make those.
//!
//! The step function answers `(value . next-state)`, or nil when there is
//! nothing left. All three doors are built on that one shape and they are NOT
//! interchangeable: `next` mutates, `empty?` peeks, `step` is functional. An
//! engine that made `step` a synonym for `next` would pass every single-pass
//! test and corrupt anything that iterates twice, which is why x-lang's suite
//! steps one iterator twice and expects the same element.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{Obj, NIL};
use crate::objects::Objects;
use crate::prim::PrimDef;

/// Making an iterator needs only the object model; DRIVING one needs the
/// evaluator, because the step function is a closure. The two sit in different
/// variants for that reason, and the signatures say which is which.
fn make(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.iter(a[0], a[1]))
}

/// ONE step, taken without touching the receiver: `Some((value, next-state))`,
/// or `None` when the sequence is exhausted. All three doors below are this plus
/// a decision about what to do with the result.
///
/// `Option` rather than a nil sentinel, and that is not decoration: a step
/// legitimately answering `(() . ())` is indistinguishable from exhaustion if
/// "both halves nil" is the test, and the first version of this file used
/// exactly that test.
///
/// The state is passed QUOTED. It is already a value, and an unquoted list state
/// would be evaluated as a call — a list of symbols would die on the first name.
fn peek(e: &mut Engine, it: Obj) -> Result<Option<(Obj, Obj)>, Cond> {
    let f = e.objects.iter_step(it);
    let st = e.objects.iter_state(it);
    let r = e.call_with_values(f, &[st], e.root_env())?;
    if r == NIL {
        return Ok(None);
    }
    Ok(Some((e.objects.first(r), e.objects.rest(r))))
}

/// MUTATES the iterator's state word — the imperative door.
fn next(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let it = a[0];
    match peek(e, it)? {
        None => Ok(NIL),
        Some((v, rest)) => {
            e.objects.set_iter_state(it, rest);
            Ok(v)
        }
    }
}

/// PEEKS. Asking whether an iterator is exhausted must not exhaust it, so the
/// state word is left exactly as it was found.
fn empty(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let it = a[0];
    let done = peek(e, it)?.is_none();
    Ok(e.objects.truth(done))
}

/// The FUNCTIONAL door: `(value . next-ITERATOR)`, receiver untouched.
fn step(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let it = a[0];
    match peek(e, it)? {
        None => Ok(NIL),
        Some((v, rest)) => {
            let f = e.objects.iter_step(it);
            let nit = e.objects.iter(f, rest);
            Ok(e.objects.pair(v, nit))
        }
    }
}

pub const TABLE: &[PrimDef] = &[
    PrimDef::filed("iter", "make", 2, make),
    PrimDef::filed_full("iter", "next", 1, next),
    PrimDef::filed_full("iter", "empty?", 1, empty),
    PrimDef::filed_full("iter", "step", 1, step),
];

#[cfg(test)]
mod tests {
    /// The coordinates these cases reach for.
    const COORDS: &[(&str, &str, &str)] = &[
        ("%mk", "iter", "make"),
        ("%next", "iter", "next"),
        ("%empty", "iter", "empty?"),
        ("%stepp", "iter", "step"),
    ];

    /// The list stepper every case below iterates with.
    const STEP: &str = "(def %step (fn (self st)\n      (match ((eq? st ()) ()) (#t (pair (first st) (rest st))))))";

    fn src(body: &str) -> String {
        format!("{}\n{}", with_coords(COORDS, STEP), body)
    }

    use crate::testkit::{int_of, truthy, with_coords};

    #[test]
    fn next_advances_in_order() {
        let s = src("(def it (%mk %step (pair 1 (pair 2 (pair 3 ())))))
             (%next it) (%next it) (%next it)");
        assert_eq!(int_of(&s), 3);
    }

    #[test]
    fn empty_is_false_before_exhaustion_and_true_after() {
        assert!(!truthy(&src(
            "(def it (%mk %step (pair 1 ()))) (%empty it)"
        )));
        assert!(truthy(&src(
            "(def it (%mk %step (pair 1 ()))) (%next it) (%empty it)"
        )));
        assert!(truthy(&src("(%empty (%mk %step ()))")));
    }

    /// The distinction an engine is most likely to lose. `step` twice on the same
    /// iterator must answer the same element both times; if it mutated, the
    /// second answer would be 2.
    #[test]
    fn step_is_functional_and_does_not_advance_the_receiver() {
        let s = src("(def it (%mk %step (pair 1 (pair 2 ()))))
             (def a (%stepp it)) (def b (%stepp it)) (first b)");
        assert_eq!(int_of(&s), 1);
    }

    /// And asking whether it is empty must not consume either.
    #[test]
    fn empty_does_not_consume() {
        let s = src("(def it (%mk %step (pair 1 (pair 2 ()))))
             (%empty it) (%empty it) (%next it)");
        assert_eq!(int_of(&s), 1);
    }
}
