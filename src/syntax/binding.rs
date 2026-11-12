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

/// `(def name value)` — binds in the CURRENT environment, the reference's
/// `x_prim_define` over `x_env_bind`: rebinding in place, never touching a
/// parent, and answering the value.
///
/// The current environment is whichever the form is evaluated in: the root
/// at the top level and while a file loads, a call's own frame inside a
/// closure body, an operative's frame inside its body, and the environment
/// `eval` or `tail-eval` was handed. So an operative defines for its caller
/// with `(tail-eval (list 'def n v) e)` or `(eval (list 'def n v) e)`.
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
    e.envs.bind(&mut e.objects, env, name, v);
    Ok(v)
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

    /// An operative's def, evaluated in the caller's environment, binds in
    /// the caller's frame, and nowhere else (the reference's
    /// tests/bare/specs/env.spec.md).
    #[test]
    fn an_operative_defines_in_the_callers_frame() {
        let src = "(def wrap (op (f) e (eval f e)))
                   ((fn (_) (wrap (def b 2)) b))";
        assert_eq!(int_of(src), 2);
        assert!(raises(
            "(def wrap (op (f) e (eval f e))) ((fn (_) (wrap (def b 2)) ())) b"
        ));
        let tail = "(def myif (op (t th . el) e
                      (match ((eval t e) (tail-eval th e)) (#t (tail-eval (first el) e)))))
                    ((fn (_) (myif #f 1 (def zz 7)) zz))";
        assert_eq!(int_of(tail), 7);
    }

    /// `set!` refuses an unbound name: letting it bind would make a misspelling
    /// silently create a variable nothing reads.
    #[test]
    fn set_refuses_a_name_that_does_not_exist() {
        assert!(raises("(set! never-defined 1)"));
    }
}
