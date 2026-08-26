//! Control: `%seq`, `error`, `guard`, `match`.
//!
//! Mirrors the reference engine's `x-syntax/control.c`. The boundary is not invented
//! here: x-engine-c drew it, and an engine that grouped these differently
//! would make the two implementations harder to read against each other for
//! no gain.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{EnvId, Obj, NIL};
use crate::prim::PrimDef;

/// `(%seq forms...)` — evaluate each, answer the last.
///
/// A sequencing point, and the reason it exists at this level is that the
/// conformance suite needs to discard a value inside a `guard` without a `do`
/// form, which is x-lang's and not the engine's.
fn seq(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    e.eval_body_tail(args, env)
}

/// The only channel a bare engine has. A raise at top level ENDS the run —
/// checked against x-engine-c rather than assumed.
fn error(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let form = e.nth(args, 0);
    let v = e.eval(form, env)?;
    Err(Cond::Raised(v))
}

/// `(guard (name handler...) body...)` — on a raise, bind the RAISED VALUE to
/// `name` in a fresh frame and run the handler.
fn guard(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let spec = e.nth(args, 0);
    let body = e.objects.rest(args);
    // A handler is now active, which is what lets a pending interrupt become a
    // STOP rather than tearing the run down.
    e.handler_push(env);
    let outcome = e.eval_body(body, env);
    e.handler_pop();
    match outcome {
        Ok(v) => Ok(v),
        // An ESCAPING continuation is not a condition and must pass straight
        // through. A guard that caught one would strand the escape at the wrong
        // depth, silently turning a non-local exit into a handled error.
        Err(cond) if e.is_escaping() => Err(cond),
        Err(cond) => {
            let (name, handler) = if e.objects.is_cell(spec) {
                (e.objects.first(spec), e.objects.rest(spec))
            } else {
                (NIL, NIL)
            };
            // The HANDLER is a tail position; the BODY is not. Parking the body
            // would evaluate it in the caller's loop, outside this guard, and
            // the raise it exists to catch would sail straight past.
            let frame = e.envs.push(&mut e.objects, env);
            if !name.is_nil() {
                // The value is built HERE, not at the failure site. A condition
                // that is caught and ignored never allocates a message at all.
                let v = cond.value(&mut e.objects);
                e.envs.bind(&mut e.objects, frame, name, v);
            }
            e.eval_body_tail(handler, frame)
        }
    }
}

/// `(match (test expr...) ...)` — the first arm whose test is true wins and its
/// remaining forms are the value. Not `if`: x-lang's house rule is that
/// multi-branch dispatch is written with match.
fn match_(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let arms: Vec<Obj> = e.objects.list(args).collect();
    for arm in arms {
        // `is_pair` here asks whether the ARM is well formed, which is a
        // different question from walking the spine — that is the iterator's job
        // now. A malformed arm is skipped rather than raising, because a match
        // with no true arm is already a legitimate nil.
        if !e.objects.is_cell(arm) {
            continue;
        }
        let test = e.objects.first(arm);
        let v = e.eval(test, env)?;
        if e.objects.truthy(v) {
            let rest = e.objects.rest(arm);
            // The winning arm is in TAIL POSITION. Evaluating it here puts a
            // frame between a function and its own recursive call, which is
            // where x-lang's loops actually live: `(match (test (self ...)))`.
            return e.eval_body_tail(rest, env);
        }
    }
    Ok(NIL)
}

crate::uniform_op!(seq_u, seq);
crate::uniform_op!(error_u, error);
crate::uniform_op!(guard_u, guard);
crate::uniform_op!(match_op_u, match_);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(Some("%seq"), None, 0, seq_u),
    PrimDef::row(Some("error"), None, 0, error_u),
    PrimDef::row(Some("guard"), None, 0, guard_u),
    PrimDef::row(Some("match"), None, 0, match_op_u),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{int_of, truthy};

    #[test]
    fn guard_binds_the_raised_value() {
        assert!(truthy("(eq? (guard (e e) (error (lit boom))) (lit boom))"));
    }

    #[test]
    fn guard_lets_a_normal_value_through() {
        assert_eq!(int_of("(guard (e 0) 42)"), 42);
    }

    /// Falsy is exactly {nil, #f}. Zero and the empty string are TRUE, and an
    /// engine that treated zero as false would pass most tests and fail x-lang.
    #[test]
    fn match_uses_x_langs_falsy_set() {
        assert_eq!(int_of("(match (0 1) (#t 2))"), 1, "zero is true");
        assert_eq!(int_of("(match (#f 1) (#t 2))"), 2);
        assert_eq!(int_of("(match (() 1) (#t 2))"), 2);
    }

    #[test]
    fn match_with_no_true_arm_is_nil() {
        assert!(truthy("(eq? (match (#f 1)) ())"));
    }
}
