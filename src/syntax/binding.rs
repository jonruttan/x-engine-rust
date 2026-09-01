//! Binding: `def` and `set!`.
//!
//! Mirrors the reference engine's `x-syntax/binding.c`. The boundary is not invented
//! here: x-engine-c drew it, and an engine that grouped these differently
//! would make the two implementations harder to read against each other for
//! no gain.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{EnvId, Obj};
use crate::prim::PrimDef;

/// `(def name value)` — GLOBAL in tail position, local otherwise.
///
/// The split is x-lang's, not an implementation detail, and the reference draws
/// it by asking whether its save stack is empty. Nothing pending means the form
/// is in tail position from the top level, and its definition is meant to
/// persist; something pending means it is a step inside a larger evaluation, and
/// the definition is that evaluation's own.
///
/// It is load-bearing. `lib/x/doc/doc.x` wraps most of the library's definitions
/// and hands the reconstructed `(def …)` to `tail-eval` — its comment says the
/// call "must run in the op's own tail so it defines the symbol in the caller's
/// env". Binding into the env operand instead put `or`, `and` and every other
/// documented definition into an operative frame that died on return. Nothing
/// raised; the names were simply Unbound later.
///
/// Asked of x-engine-c rather than assumed:
///
/// ```text
/// (def myif (op (t th . el) e
///   (match ((eval t e) (tail-eval th e)) (#t (tail-eval (first el) e)))))
/// (def outer (op (x) e (myif #f 1 (def zz 7))))
/// (outer 0)
/// zz          =>  7
/// ```
///
/// The env handed down was `myif`'s caller frame, and the definition still
/// reached the top level.
fn def(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let name = e.nth(args, 0);
    let form = e.nth(args, 1);
    let depth = e.active_evals;
    e.control.push(crate::eval::ControlRec::Bind {
        name,
        env,
        set: false,
        depth,
    });
    let r = e.eval(form, env);
    e.control.pop();
    let v = r?;
    let target = if e.nothing_pending() {
        e.root_env()
    } else {
        env
    };
    e.envs.bind(&mut e.objects, target, name, v);
    Ok(name)
}

/// `(set! name value)` — rebinds where the name ALREADY lives, and refuses an
/// unbound one. Letting it bind would make a misspelling silently create a
/// variable nothing reads; `def` is how a name comes into being.
fn set(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let name = e.nth(args, 0);
    let form = e.nth(args, 1);
    let depth = e.active_evals;
    e.control.push(crate::eval::ControlRec::Bind {
        name,
        env,
        set: true,
        depth,
    });
    let r = e.eval(form, env);
    e.control.pop();
    let v = r?;
    if e.envs.set_existing(&mut e.objects, env, name, v) {
        Ok(v)
    } else {
        Err(Cond::Unbound(name))
    }
}

crate::uniform_op!(def_u, def);
crate::uniform_op!(set_u, set);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(Some("def"), None, 0, def_u),
    PrimDef::row(Some("set!"), None, 0, set_u),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{int_of, raises};

    #[test]
    fn set_rebinds_and_refuses_an_unbound_name() {
        assert_eq!(int_of("(def x 1) (set! x 2) x"), 2);
        assert!(raises("(set! never-bound 1)"));
    }

    /// Shadowing instead of rebinding would make this answer 1: the inner frame
    /// would gain its own `x` and the outer one would be untouched.
    #[test]
    fn set_reaches_the_frame_the_name_lives_in() {
        assert_eq!(
            int_of("(def x 1) (def bump (fn (self) (set! x 2))) (bump) x"),
            2
        );
    }

    /// A `def` at the true top level is global, which is the ordinary case.
    #[test]
    fn a_top_level_def_is_global() {
        assert_eq!(int_of("(def x 7) x"), 7);
    }

    /// And one reached through a chain of TAIL-EVALS is global too, even though
    /// the env handed along was an inner operative's frame.
    ///
    /// Asked of x-engine-c rather than assumed — it answers 7. The library is
    /// built on it: `lib/x/doc/doc.x` wraps most definitions and re-evaluates
    /// the rebuilt `(def …)` through `tail-eval`, so binding into the env
    /// operand instead left `or`, `and` and the rest Unbound with nothing
    /// raised at the point of loss.
    #[test]
    fn a_def_reached_through_tail_evals_is_global() {
        let src = "(def myif (op (t th . el) e
                     (match ((eval t e) (tail-eval th e)) (#t (tail-eval (first el) e)))))
                   (def outer (op (x) e (myif #f 1 (def zz 7))))
                   (outer 0)
                   zz";
        assert_eq!(int_of(src), 7);
    }

    /// `set!` refuses an unbound name: letting it bind would make a misspelling
    /// silently create a variable nothing reads.
    #[test]
    fn set_refuses_a_name_that_does_not_exist() {
        assert!(raises("(set! never-defined 1)"));
    }
}
