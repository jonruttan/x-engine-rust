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

use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{EnvId, Obj};

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
/// THE CALLING CONVENTION — one signature for every instruction, the
/// reference's `f(p_base, p_args)` with self in the call:
/// `(engine, callee, args, env)`. The callee is the primitive OBJECT itself
/// for an ordinary instruction (mostly ignored) and the callable being
/// applied for an entry row; args arrive AS WRITTEN, and a row that wants
/// values evaluates them itself, exactly as `x_eargs` does inside every C
/// primitive.
///
/// The leaf functions behind the rows keep their narrow shapes — an
/// integer operation is still a function of integers, testable with no
/// engine in existence — and the `uniform_*` macros generate the row
/// functions that wrap them. Fast paths live INSIDE a uniform row, never
/// as dispatcher kinds.
pub type PrimFn = fn(&mut Engine, Obj, Obj, EnvId) -> EvalResult;

/// Wrap a VALUE leaf — needs only the object model — as a uniform row.
#[macro_export]
macro_rules! uniform_value {
    ($name:ident, $leaf:path, $n:expr) => {
        fn $name(
            e: &mut $crate::engine::Engine,
            _c: $crate::obj::Obj,
            args: $crate::obj::Obj,
            env: $crate::obj::EnvId,
        ) -> $crate::eval::EvalResult {
            let vals = e.eargs(args, env, $n)?;
            $leaf(&mut e.objects, &vals)
        }
    };
}

/// Wrap an ENGINE leaf — evaluation, the reader — as a uniform row. The
/// base arrives as the reference's `p_base` does: the one that is running.
#[macro_export]
macro_rules! uniform_engine {
    ($name:ident, $leaf:path, $n:expr) => {
        fn $name(
            e: &mut $crate::engine::Engine,
            _c: $crate::obj::Obj,
            args: $crate::obj::Obj,
            env: $crate::obj::EnvId,
        ) -> $crate::eval::EvalResult {
            let vals = e.eargs(args, env, $n)?;
            let base = e.base;
            $leaf(e, base, &vals)
        }
    };
}

/// Wrap an OPERATIVE leaf: the spine as written, the caller's environment.
#[macro_export]
macro_rules! uniform_op {
    ($name:ident, $leaf:path) => {
        fn $name(
            e: &mut $crate::engine::Engine,
            _c: $crate::obj::Obj,
            args: $crate::obj::Obj,
            env: $crate::obj::EnvId,
        ) -> $crate::eval::EvalResult {
            $leaf(e, args, env)
        }
    };
}

/// A tower operator row: the type-ops registry is offered the operands
/// first, then the machine operation runs. Only `+ - * / %` and the
/// comparisons — ruling #52 keeps bitwise out of the tower.
#[macro_export]
macro_rules! uniform_tower2 {
    ($name:ident, $spell:literal, $op:expr) => {
        fn $name(
            e: &mut $crate::engine::Engine,
            _c: $crate::obj::Obj,
            args: $crate::obj::Obj,
            env: $crate::obj::EnvId,
        ) -> $crate::eval::EvalResult {
            let vals = e.eargs(args, env, 2)?;
            match e.op_try($spell, vals[0], vals[1])? {
                Some(v) => Ok(v),
                None => {
                    // After op_try, so a typed operand never reaches it: the
                    // raw prims are the only nil guard on the bare-core and
                    // child-base paths (#52, #239).
                    if vals[0].is_nil() || vals[1].is_nil() {
                        return Err(e.nil_operand($spell));
                    }
                    let f: fn(i64, i64) -> i64 = $op;
                    let (x, y) = (e.objects.as_int(vals[0]), e.objects.as_int(vals[1]));
                    Ok(e.objects.int(f(x, y)))
                }
            }
        }
    };
}

/// The comparison twin.
#[macro_export]
macro_rules! uniform_tower_pred {
    ($name:ident, $spell:literal, $op:expr) => {
        fn $name(
            e: &mut $crate::engine::Engine,
            _c: $crate::obj::Obj,
            args: $crate::obj::Obj,
            env: $crate::obj::EnvId,
        ) -> $crate::eval::EvalResult {
            let vals = e.eargs(args, env, 2)?;
            match e.op_try($spell, vals[0], vals[1])? {
                Some(v) => Ok(v),
                None => {
                    let f: fn(i64, i64) -> bool = $op;
                    let (x, y) = (e.objects.as_int(vals[0]), e.objects.as_int(vals[1]));
                    Ok(e.objects.truth(f(x, y)))
                }
            }
        }
    };
}

/// A bitwise row: raw machine integers, and the nil guard the reference
/// keeps in the prims themselves (#239) — no library wrapper fronts these
/// in a child base.
#[macro_export]
macro_rules! uniform_int2 {
    ($name:ident, $spell:literal, $op:expr) => {
        fn $name(
            e: &mut $crate::engine::Engine,
            _c: $crate::obj::Obj,
            args: $crate::obj::Obj,
            env: $crate::obj::EnvId,
        ) -> $crate::eval::EvalResult {
            let vals = e.eargs(args, env, 2)?;
            if vals[0].is_nil() || vals[1].is_nil() {
                return Err(e.nil_operand($spell));
            }
            let f: fn(i64, i64) -> i64 = $op;
            let (x, y) = (e.objects.as_int(vals[0]), e.objects.as_int(vals[1]));
            Ok(e.objects.int(f(x, y)))
        }
    };
}

/// Its one-operand twin.
#[macro_export]
macro_rules! uniform_int1 {
    ($name:ident, $spell:literal, $op:expr) => {
        fn $name(
            e: &mut $crate::engine::Engine,
            _c: $crate::obj::Obj,
            args: $crate::obj::Obj,
            env: $crate::obj::EnvId,
        ) -> $crate::eval::EvalResult {
            let vals = e.eargs(args, env, 1)?;
            if vals[0].is_nil() {
                return Err(e.nil_operand($spell));
            }
            let f: fn(i64) -> i64 = $op;
            Ok(e.objects.int(f(e.objects.as_int(vals[0]))))
        }
    };
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
    pub f: PrimFn,
}

impl PrimDef {
    /// One row. The names and arity are the ISA's; the function is the
    /// convention's.
    pub const fn row(
        bare: Option<&'static str>,
        coord: Option<(&'static str, &'static str)>,
        n: usize,
        f: PrimFn,
    ) -> Self {
        PrimDef {
            bare,
            coord,
            arity: (n, Some(n)),
            f,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never_row(_: &mut Engine, _: Obj, _: Obj, _: EnvId) -> EvalResult {
        unreachable!("the row is data here, never called")
    }

    /// Every row is ONE function shape; the names and arity are ISA data
    /// beside it, not a second dispatch.
    #[test]
    fn a_row_is_one_shape() {
        let d = PrimDef::row(Some("thing"), None, 2, never_row);
        assert_eq!(d.arity, (2, Some(2)));
        assert_eq!(d.bare, Some("thing"));
    }
}
