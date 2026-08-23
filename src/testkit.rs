//! Shared test scaffolding.
//!
//! Compiled only under `cfg(test)`. It exists so that the catalog walk — the
//! handful of lines every test needs to reach a coordinate before a library
//! exists — is written once instead of pasted into each module. That is the same
//! duplication this rewrite removed from the primitives themselves, and it grows
//! back just as easily in tests.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::obj::Obj;

/// Reaching a coordinate from a bare engine: walk `(%base)` to the prims
/// catalog and define `%coord`.
///
/// It WALKS THE DECLARED ROUTE from `base-paths.x`, exactly as
/// `tests/x/conformance/prelude.x` does. An earlier version hard-coded
/// `(first (first (%base)))`, which was the base's shape at the time — so
/// changing that shape broke thirty tests that were not about the base at all,
/// while the conformance suite sailed through because it had asked properly.
///
/// A test that reaches primitives by a private back door is not exercising the
/// route the language uses, and will disagree with the language the moment the
/// route moves.
pub const CATALOG: &str = r#"
(include "tools/contract/base-paths.x")
(def %assoc (fn (self k l)
  (match ((eq? l ()) ())
         ((eq? (first (first l)) k) (first l))
         (#t (self k (rest l))))))
(def %walk (fn (self steps o)
  (match ((eq? steps ()) o)
         ((eq? (first steps) (lit f)) (self (rest steps) (first o)))
         (#t (self (rest steps) (rest o))))))
(def %cat (first (%walk (rest (rest (%assoc (lit prims) %base-paths))) (%base))))
(def %coord (fn (self ns m) (rest (%assoc m (rest (%assoc ns %cat))))))
"#;

/// Bind coordinates to names, so a test says what it needs instead of writing
/// the same `(def %x (%coord ...))` block again.
///
/// This exists because the block WAS written again — three modules grew their
/// own `const DEFS` within an hour of this file being created to prevent exactly
/// that. Duplication grows back wherever there is no part to reach for.
pub fn coords(bindings: &[(&str, &str, &str)]) -> String {
    bindings
        .iter()
        .map(|(name, ns, method)| {
            format!("(def {} (%coord (lit {}) (lit {})))\n", name, ns, method)
        })
        .collect()
}

/// Evaluate source with the catalog walk and the named coordinates in scope.
pub fn with_coords(bindings: &[(&str, &str, &str)], body: &str) -> String {
    format!("{}\n{}", coords(bindings), body)
}

/// Evaluate source with the catalog walk already in scope.
pub fn eval(src: &str) -> (Engine, Result<Obj, Cond>) {
    let mut e = Engine::new();
    let full = format!("{}\n{}", CATALOG, src);
    let v = e.eval_str(&full);
    (e, v)
}

/// Evaluate and require success. A failure reports WHAT was raised, which is
/// only possible because `Cond` derives `Debug` — before it did, every test here
/// had to discard the condition with `.ok()` and report nothing but the source.
pub fn eval_ok(src: &str) -> (Engine, Obj) {
    let (e, v) = eval(src);
    match v {
        Ok(v) => (e, v),
        Err(cond) => panic!("{}\n  in: {}", cond.diagnostic(&e.objects), src),
    }
}

/// Did it raise?
pub fn raises(src: &str) -> bool {
    eval(src).1.is_err()
}

pub fn truthy(src: &str) -> bool {
    let (e, v) = eval_ok(src);
    e.objects.truthy(v)
}

pub fn int_of(src: &str) -> i64 {
    let (e, v) = eval_ok(src);
    assert!(e.objects.is_int(v), "expected an integer from: {}", src);
    e.objects.int_val(v)
}

/// The string a source evaluates to, asserting that it IS one.
///
/// Distinct from [`text_of`], which renders whatever it is given: a convention
/// that answered a symbol instead of a string would render identically and pass.
pub fn str_of(src: &str) -> String {
    let (e, v) = eval_ok(src);
    assert!(e.objects.is_str(v), "expected a string from: {}", src);
    String::from_utf8_lossy(&e.objects.bytes_of(v)).into_owned()
}

pub fn text_of(src: &str) -> String {
    let (e, v) = eval_ok(src);
    crate::diag::value_text(&e.objects, v)
}
