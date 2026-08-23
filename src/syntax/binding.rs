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

fn def(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let name = e.nth(args, 0);
    let form = e.nth(args, 1);
    let v = e.eval(form, env)?;
    e.envs.bind(env, name, v);
    Ok(name)
}

/// `(set! name value)` — rebinds where the name ALREADY lives, and refuses an
/// unbound one. Letting it bind would make a misspelling silently create a
/// variable nothing reads; `def` is how a name comes into being.
fn set(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let name = e.nth(args, 0);
    let form = e.nth(args, 1);
    let v = e.eval(form, env)?;
    if e.envs.set_existing(env, name, v) {
        Ok(v)
    } else {
        Err(Cond::Unbound(name))
    }
}

pub const TABLE: &[PrimDef] = &[PrimDef::op("def", def), PrimDef::op("set!", set)];

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
}
