//! Abstraction: `fn` and `op`.
//!
//! Mirrors the reference engine's `x-syntax/closure.c`. The boundary is not invented
//! here: x-engine-c drew it, and an engine that grouped these differently
//! would make the two implementations harder to read against each other for
//! no gain.

use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{EnvId, Obj};
use crate::prim::PrimDef;

/// `(fn (params...) body...)` — captures the environment it is written in.
fn func(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let params = e.nth(args, 0);
    let body = e.objects.rest(args);
    Ok(e.objects.closure(params, body, env))
}

/// `(op (params...) env-name body...)` — the third element names the CALLER's
/// environment, which is what forces environments to be first-class values.
fn operative(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let params = e.nth(args, 0);
    let envname = e.nth(args, 1);
    let rest = e.objects.rest(args);
    let body = e.objects.rest(rest);
    Ok(e.objects.operative(params, envname, body, env))
}

crate::uniform_op!(func_u, func);
crate::uniform_op!(operative_u, operative);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(Some("fn"), None, 0, func_u),
    PrimDef::row(Some("op"), None, 0, operative_u),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{int_of, truthy};

    /// The distinction that makes this a fexpr engine, tested the way x-lang
    /// tests it: with an UNBOUND symbol, so an engine that evaluated the argument
    /// dies rather than merely answering differently.
    #[test]
    fn op_receives_its_arguments_unevaluated() {
        assert!(truthy(
            "(def q (op (x) e x))
             (eq? (q no-such-binding-anywhere) (lit no-such-binding-anywhere))"
        ));
    }

    #[test]
    fn an_operative_can_evaluate_in_the_callers_environment() {
        assert_eq!(
            int_of("(def deref (op (x) e (eval x e))) (def y 9) (deref y)"),
            9
        );
    }

    /// A closure resolving names in the caller's environment would be dynamic
    /// scope wearing this syntax, and this is the case that tells them apart.
    #[test]
    fn closures_are_lexically_scoped() {
        assert_eq!(int_of("(def mk (fn (self n) (fn (self2) n))) ((mk 7))"), 7);
    }

    /// `fn` binds its first parameter to the closure itself, which is why every
    /// function in x-lang's conformance prelude starts with `self`.
    #[test]
    fn fn_binds_self_so_it_can_recurse_unnamed() {
        assert_eq!(
            int_of(
                "(def len (fn (self l n)
                   (match ((eq? l ()) n) (#t (self (rest l) (+ n 1))))))
                 (len (lit (a b c d)) 0)"
            ),
            4
        );
    }
}
