//! Escape continuations.
//!
//! Mirrors the reference engine's `x-prim/callcc.c`. The boundary is not invented
//! here: x-engine-c drew it, and an engine that grouped these differently
//! would make the two implementations harder to read against each other for
//! no gain.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{EnvId, Obj};
use crate::prim::PrimDef;

/// `(call/cc f)` — ESCAPE-only continuations.
///
/// The continuation unwinds outward and cannot be re-entered. x-lang's library
/// never calls call/cc at all — only doc-prims.x documents it — so escape covers
/// everything the language does.
fn call_cc(e: &mut Engine, a: &[Obj]) -> EvalResult {
    e.with_escape(a[0], e.root_env())
}

/// `(%cc-invoke k v)` — begin an unwind that only k's own call/cc will stop.
fn cc_invoke(e: &mut Engine, a: &[Obj]) -> EvalResult {
    e.invoke_cont(a[0], a[1])
}

impl Engine {
    /// Begin an unwind that only its own `call/cc` will stop.
    pub fn invoke_cont(&mut self, k: Obj, v: Obj) -> EvalResult {
        let id = self.objects.cont_id(k);
        self.escaping = Some((id, v));
        Err(Cond::Raised(v))
    }

    /// Run `f` with a fresh escape continuation, catching only its own.
    pub fn with_escape(&mut self, f: Obj, env: EnvId) -> EvalResult {
        let id = self.next_cont;
        self.next_cont += 1;
        let k = self.objects.cont(id);
        match self.call_with_values(f, &[k], env) {
            Ok(v) => Ok(v),
            Err(e) => match self.escaping {
                // Ours: stop the unwind here and answer the thrown value.
                Some((eid, v)) if eid == id => {
                    self.escaping = None;
                    Ok(v)
                }
                _ => Err(e),
            },
        }
    }

    /// Is an escape passing through? `guard` asks, because catching one would
    /// strand it.
    pub fn is_escaping(&self) -> bool {
        self.escaping.is_some()
    }
}

pub const TABLE: &[PrimDef] = &[
    PrimDef::both_full("call/cc", "ctrl", "call/cc", 1, call_cc),
    PrimDef::bare_full("%cc-invoke", 2, cc_invoke),
];

#[cfg(test)]
mod tests {
    use crate::testkit::int_of;

    #[test]
    fn call_cc_returns_its_bodys_value_when_the_continuation_is_unused() {
        assert_eq!(int_of("(call/cc (fn (self k) 7))"), 7);
    }

    #[test]
    fn call_cc_gives_an_escape_continuation() {
        assert_eq!(int_of("(call/cc (fn (self k) (+ 1 (k 9))))"), 9);
    }

    /// THE SUBTLE ONE. An escaping continuation is not a condition, and a guard
    /// between the throw and its call/cc must let it through. Catching it would
    /// strand the escape at the wrong depth and silently turn a non-local exit
    /// into a handled error — the sort of bug that leaves every individual test
    /// passing.
    #[test]
    fn a_guard_does_not_catch_an_escaping_continuation() {
        assert_eq!(
            int_of("(call/cc (fn (self k) (guard (e 111) (k 9))))"),
            9,
            "the guard must not swallow the escape"
        );
    }

    /// And it still catches ordinary conditions raised inside the same call/cc.
    #[test]
    fn a_guard_still_catches_a_raise_inside_call_cc() {
        assert_eq!(
            int_of("(call/cc (fn (self k) (guard (e 111) (error 1))))"),
            111
        );
    }

    /// Two continuations do not catch each other's escapes.
    #[test]
    fn an_escape_passes_through_an_inner_call_cc() {
        assert_eq!(
            int_of("(call/cc (fn (self outer) (call/cc (fn (s2 inner) (outer 5)))))"),
            5
        );
    }
}
