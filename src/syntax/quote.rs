//! Quotation: `lit`.
//!
//! Mirrors the reference engine's `x-syntax/quote.c`. The boundary is not invented
//! here: x-engine-c drew it, and an engine that grouped these differently
//! would make the two implementations harder to read against each other for
//! no gain.

use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{EnvId, Obj};
use crate::prim::PrimDef;

fn lit(e: &mut Engine, args: Obj, _env: EnvId) -> EvalResult {
    Ok(e.nth(args, 0))
}

pub const TABLE: &[PrimDef] = &[PrimDef::op("lit", lit)];

#[cfg(test)]
mod tests {
    use crate::testkit::truthy;

    /// `lit` is the whole reason an engine is fexpr at this level: an
    /// applicative `lit` would evaluate its argument and be a no-op.
    #[test]
    fn lit_answers_its_argument_unevaluated() {
        assert!(truthy("(eq? (lit nope) (lit nope))"));
        assert!(truthy("(eq? (first (lit (a b))) (lit a))"));
    }

    #[test]
    fn lit_of_a_list_is_the_list_not_a_call() {
        assert!(truthy(
            "(eq? (first (lit (undefined-fn 1))) (lit undefined-fn))"
        ));
    }
}
