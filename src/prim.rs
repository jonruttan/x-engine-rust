//! Primitives as DATA.
//!
//! A primitive is a row — where it is reachable from, how many arguments it
//! takes, and a plain function. It is not an enum variant plus a hand-written
//! arm in a dispatcher, which is what this file replaces: that shape re-extracted
//! and re-checked its own arguments at every site, so the same four lines
//! appeared a hundred times and no primitive could be called without going
//! through the evaluator.
//!
//! Two consequences fall out of the row being data:
//!
//! * **Arity is checked once**, at the call boundary, instead of not at all.
//! * **The bare name and the catalog coordinate come from the SAME row**, so an
//!   instruction cannot be bound under one and missing under the other. That
//!   correspondence is what x-lang's conformance suite checks directly ("every
//!   arithmetic coordinate is filed and agrees with its bare binding"), and it
//!   now holds by construction rather than by remembering to file both.
//!
//! The row also mirrors `tools/contract/isa.x`: a `bare` entry is an `%isa-bare`
//! row and a `coord` entry is an `%isa-catalog` row. The manifest and the engine
//! say the same thing because they are the same list.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{EnvId, Obj};
use crate::objects::Objects;

/// How a primitive receives its arguments — the fexpr distinction, in the type
/// rather than in a convention.
///
/// x-lang's model is operative-by-default at the C level, and the conformance
/// suite tests the difference with an UNBOUND symbol: an engine that evaluated
/// an operative's arguments does not merely answer differently, it dies. Making
/// the two kinds separate variants means a primitive cannot silently be written
/// in the wrong discipline.
/// The variants are ordered by HOW MUCH CONTEXT the primitive can reach, and
/// that ordering is the point. A primitive is handed the least it can do its job
/// with, so its signature states its capability requirement: reading `Value`
/// tells you the instruction cannot evaluate anything, cannot touch an
/// environment, and cannot read the input stream, without having to read its
/// body.
///
/// Before this, every primitive took `&mut Engine`. Multiplying two integers was
/// handed the reader, the environment chain, the primitive table and the base
/// object, so nothing could be reasoned about locally and nothing could be
/// exercised without constructing an entire engine.
#[derive(Clone, Copy)]
pub enum Body {
    /// Needs only the OBJECT MODEL: allocate, read slots, compare. Most
    /// instructions are here — arithmetic conversions, strings, pointers, pairs,
    /// the type registry. They cannot reach the evaluator at all.
    Value(fn(&mut Objects, &[Obj]) -> Result<Obj, Cond>),
    /// Needs to reach back into EVALUATION or the input stream: iterators drive
    /// a step function, `io read` consumes the program's own text.
    Applicative(fn(&mut Engine, &[Obj]) -> EvalResult),
    /// Arguments arrive AS WRITTEN, with the environment they were written in.
    /// `lit`, `def`, `fn`, `op`, `match`, `guard`, `set!` — everything whose
    /// whole purpose is to decide what gets evaluated.
    Operative(fn(&mut Engine, Obj, EnvId) -> EvalResult),

    // The three below are PURE FUNCTIONS OF INTEGERS. They do not receive the
    // engine at all, which is the point: eleven of the thirteen machine
    // operations were bodies that differed from each other in exactly one
    // operator, and each one repeated the same unwrap-two-integers-and-rebox
    // preamble. Expressed this way the operator IS the primitive, and the
    // preamble is written once in the dispatcher.
    //
    // They are also the only primitives testable with no engine in existence.
    /// `(op a b)` on two integers.
    IntBinop(fn(i64, i64) -> i64),
    /// `(op a b)` answering a truth value.
    IntPred(fn(i64, i64) -> bool),
    /// `(op a)` on one integer.
    IntUnop(fn(i64) -> i64),
}

/// One instruction.
#[derive(Clone, Copy)]
pub struct PrimDef {
    /// The bare name, when it has one. `%isa-bare`.
    pub bare: Option<&'static str>,
    /// The catalog coordinate, when it has one. `%isa-catalog`.
    /// Some instructions have only this — the char door is reachable no other way.
    pub coord: Option<(&'static str, &'static str)>,
    /// How many operand slots the body indexes, and the most it reads.
    ///
    /// NOT a check. The dispatcher pads the operand list to this length with
    /// nil, because a body indexing a slot needs the slot to exist — but a
    /// missing operand is nil, not an error. Counting arguments and rejecting a
    /// call is x-lang's job, one layer up.
    pub arity: (usize, Option<usize>),
    pub body: Body,
}

impl PrimDef {
    /// A machine operation on two integers, bound bare and filed.
    pub const fn int2(
        bare: &'static str,
        ns: &'static str,
        method: &'static str,
        f: fn(i64, i64) -> i64,
    ) -> Self {
        PrimDef {
            bare: Some(bare),
            coord: Some((ns, method)),
            arity: (2, Some(2)),
            body: Body::IntBinop(f),
        }
    }

    /// A machine comparison on two integers.
    pub const fn int_pred(
        bare: &'static str,
        ns: &'static str,
        method: &'static str,
        f: fn(i64, i64) -> bool,
    ) -> Self {
        PrimDef {
            bare: Some(bare),
            coord: Some((ns, method)),
            arity: (2, Some(2)),
            body: Body::IntPred(f),
        }
    }

    /// A machine operation on one integer.
    pub const fn int1(
        bare: &'static str,
        ns: &'static str,
        method: &'static str,
        f: fn(i64) -> i64,
    ) -> Self {
        PrimDef {
            bare: Some(bare),
            coord: Some((ns, method)),
            arity: (1, Some(1)),
            body: Body::IntUnop(f),
        }
    }

    /// An object-model instruction bound bare and filed at a coordinate.
    pub const fn both(
        bare: &'static str,
        ns: &'static str,
        method: &'static str,
        n: usize,
        f: fn(&mut Objects, &[Obj]) -> Result<Obj, Cond>,
    ) -> Self {
        PrimDef {
            bare: Some(bare),
            coord: Some((ns, method)),
            arity: (n, Some(n)),
            body: Body::Value(f),
        }
    }

    /// An object-model instruction reachable only through the catalog.
    pub const fn filed(
        ns: &'static str,
        method: &'static str,
        n: usize,
        f: fn(&mut Objects, &[Obj]) -> Result<Obj, Cond>,
    ) -> Self {
        PrimDef {
            bare: None,
            coord: Some((ns, method)),
            arity: (n, Some(n)),
            body: Body::Value(f),
        }
    }

    /// An object-model instruction bound bare only.
    pub const fn bare(
        bare: &'static str,
        n: usize,
        f: fn(&mut Objects, &[Obj]) -> Result<Obj, Cond>,
    ) -> Self {
        PrimDef {
            bare: Some(bare),
            coord: None,
            arity: (n, Some(n)),
            body: Body::Value(f),
        }
    }

    /// An evaluator-reaching instruction bound bare AND filed at a coordinate.
    pub const fn both_full(
        bare: &'static str,
        ns: &'static str,
        method: &'static str,
        n: usize,
        f: fn(&mut Engine, &[Obj]) -> EvalResult,
    ) -> Self {
        PrimDef {
            bare: Some(bare),
            coord: Some((ns, method)),
            arity: (n, Some(n)),
            body: Body::Applicative(f),
        }
    }

    /// An evaluator-reaching instruction reachable only through the catalog.
    pub const fn filed_full(
        ns: &'static str,
        method: &'static str,
        n: usize,
        f: fn(&mut Engine, &[Obj]) -> EvalResult,
    ) -> Self {
        PrimDef {
            bare: None,
            coord: Some((ns, method)),
            arity: (n, Some(n)),
            body: Body::Applicative(f),
        }
    }

    /// A VARIADIC evaluator-reaching instruction, filed at a coordinate.
    ///
    /// `ptr call` and `ffi call` take a function and however many arguments the
    /// callee wants, so a fixed arity would cap what the library can call.
    pub const fn var_full(
        ns: &'static str,
        method: &'static str,
        min: usize,
        f: fn(&mut Engine, &[Obj]) -> EvalResult,
    ) -> Self {
        PrimDef {
            bare: None,
            coord: Some((ns, method)),
            arity: (min, None),
            body: Body::Applicative(f),
        }
    }

    /// A VARIADIC evaluator-reaching instruction bound bare.
    pub const fn var_bare(
        bare: &'static str,
        min: usize,
        f: fn(&mut Engine, &[Obj]) -> EvalResult,
    ) -> Self {
        PrimDef {
            bare: Some(bare),
            coord: None,
            arity: (min, None),
            body: Body::Applicative(f),
        }
    }

    /// An evaluator-reaching instruction bound bare only.
    pub const fn bare_full(
        bare: &'static str,
        n: usize,
        f: fn(&mut Engine, &[Obj]) -> EvalResult,
    ) -> Self {
        PrimDef {
            bare: Some(bare),
            coord: None,
            arity: (n, Some(n)),
            body: Body::Applicative(f),
        }
    }

    /// An operative bound bare. Arity is not checked for these: an operative's
    /// argument spine is its syntax, and `match` legitimately takes any number of
    /// arms.
    pub const fn op(bare: &'static str, f: fn(&mut Engine, Obj, EnvId) -> EvalResult) -> Self {
        PrimDef {
            bare: Some(bare),
            coord: None,
            arity: (0, None),
            body: Body::Operative(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never_op(_: &mut Engine, _: Obj, _: EnvId) -> EvalResult {
        unreachable!("arity is not checked for operatives")
    }

    /// An operative declares no operand slots: its spine is its syntax, and
    /// `match` legitimately takes any number of arms.
    #[test]
    fn an_operative_declares_no_slots() {
        let d = PrimDef::op("thing", never_op);
        assert_eq!(d.arity, (0, None));
    }
}
