//! Machine arithmetic, comparison and bit operations — the `raw-op` group.
//!
//! Eleven of these are just their operator. They were eleven near-identical
//! four-line functions, each unwrapping two integers, applying one operator and
//! re-boxing the result; the preamble now lives once in the dispatcher and the
//! operator is the primitive.
//!
//! ALL THIRTEEN are, now. Division by zero answers zero rather than raising --
//! x-engine-c was asked, and that is its answer. Trapping would be a policy
//! decision, and policy belongs to x-lang.

use crate::diag::Cond;
use crate::obj::Obj;
use crate::objects::Objects;
use crate::prim::PrimDef;

/// The count is masked to the word, so an over-wide shift wraps instead of
/// panicking — Rust's `<<` panics on overflow in debug builds, which would turn a
/// program's arithmetic mistake into an engine abort, and an aborting engine
/// reports nothing at all.
fn shl(x: i64, y: i64) -> i64 {
    ((x as u64) << (y as u64 & 63)) as i64
}

/// ARITHMETIC, as the reference's signed `>>` lands on every target it
/// builds for — `(>> -1 63)` is -1, probed, and the emitted lane's `asrv`
/// agrees. The count masks to the word so an over-wide shift wraps.
fn shr(x: i64, y: i64) -> i64 {
    x >> (y as u64 & 63)
}

/// Division by zero answers ZERO, which is what x-engine-c answers -- asked,
/// not guessed. `checked_div` is how that is spelled without Rust panicking,
/// and it also covers `i64::MIN / -1`, which overflows.
///
/// Trapping here would be a policy decision, and policy is x-lang's.
fn div(x: i64, y: i64) -> i64 {
    x.checked_div(y).unwrap_or(0)
}

fn rem(x: i64, y: i64) -> i64 {
    x.checked_rem(y).unwrap_or(0)
}

fn char_to_int(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let c = a_.as_char(a[0]);
    Ok(a_.int(c as i64))
}

fn int_to_char(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let v = a_.as_int(a[0]);
    Ok(a_.char_new(v as u32))
}

crate::uniform_tower2!(u_op_1, "+", i64::wrapping_add);
crate::uniform_tower2!(u_op_3, "*", i64::wrapping_mul);

/// `(- a)` negates, `(- a b)` subtracts — the reference's `x_prim_sub`.
/// Unary keeps the int path (typed negation is the tower layer's concern,
/// not binary op dispatch); binary offers the type-ops registry first.
/// Nil operands raise on both lanes (#52).
fn u_op_2(
    e: &mut crate::engine::Engine,
    _c: Obj,
    args: Obj,
    env: crate::obj::EnvId,
) -> crate::eval::EvalResult {
    let vals = e.eargs(args, env, 1)?;
    if vals.len() < 2 {
        if vals[0].is_nil() {
            return Err(e.nil_operand("-"));
        }
        let x = e.objects.as_int(vals[0]);
        return Ok(e.objects.int(x.wrapping_neg()));
    }
    match e.op_try("-", vals[0], vals[1])? {
        Some(v) => Ok(v),
        None => {
            if vals[0].is_nil() || vals[1].is_nil() {
                return Err(e.nil_operand("-"));
            }
            let (x, y) = (e.objects.as_int(vals[0]), e.objects.as_int(vals[1]));
            Ok(e.objects.int(x.wrapping_sub(y)))
        }
    }
}
crate::uniform_int2!(u_op_4, "&", |x, y| x & y);
crate::uniform_int2!(u_op_5, "|", |x, y| x | y);
crate::uniform_int2!(u_op_6, "^", |x, y| x ^ y);
crate::uniform_int2!(u_op_7, "<<", shl);
crate::uniform_int2!(u_op_8, ">>", shr);
crate::uniform_int1!(u_op_9, "~", |x| !x);
crate::uniform_tower_pred!(u_op_10, "<", |x, y| x < y);
crate::uniform_tower_pred!(u_op_11, "=", |x, y| x == y);
crate::uniform_tower2!(u_op_12, "/", div);
crate::uniform_tower2!(u_op_13, "%", rem);
crate::uniform_value!(char_to_int_u, char_to_int, 1);
crate::uniform_value!(int_to_char_u, int_to_char, 1);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    // Wrapping throughout: x-lang's fixnums are machine integers, and overflow
    // is a value here rather than a crash.
    PrimDef::row(Some("+"), Some(("int", "+")), 2, u_op_1),
    PrimDef::row(Some("-"), Some(("int", "-")), 2, u_op_2),
    PrimDef::row(Some("*"), Some(("int", "*")), 2, u_op_3),
    PrimDef::row(Some("&"), Some(("int", "&")), 2, u_op_4),
    PrimDef::row(Some("|"), Some(("int", "|")), 2, u_op_5),
    PrimDef::row(Some("^"), Some(("int", "^")), 2, u_op_6),
    PrimDef::row(Some("<<"), Some(("int", "<<")), 2, u_op_7),
    PrimDef::row(Some(">>"), Some(("int", ">>")), 2, u_op_8),
    PrimDef::row(Some("~"), Some(("int", "~")), 1, u_op_9),
    PrimDef::row(Some("<"), Some(("int", "<")), 2, u_op_10),
    PrimDef::row(Some("="), Some(("int", "=")), 2, u_op_11),
    PrimDef::row(Some("/"), Some(("int", "/")), 2, u_op_12),
    PrimDef::row(Some("%"), Some(("int", "%")), 2, u_op_13),
    // The char door has no bare spelling in either direction: it is reachable
    // only through the catalog, which is the reference engine's arrangement.
    PrimDef::row(Some("char->integer"), Some(("char", "->int")), 1, char_to_int_u),
    PrimDef::row(Some("integer->char"), Some(("int", "->char")), 1, int_to_char_u),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{eval_ok, int_of, raises, truthy};

    /// These need NO ENGINE. That is the point of the operator being the
    /// primitive: the arithmetic can be checked as arithmetic, and the tests
    /// below that do build an engine are checking the plumbing, not the maths.
    mod pure {
        use super::super::{shl, shr};

        #[test]
        fn right_shift_is_arithmetic_as_the_reference_answers() {
            assert_eq!(shl(1, 4), 16);
            assert_eq!(shr(16, 4), 1);
            assert_eq!(shr(-1, 63), -1, "sign-propagating, probed against the C");
            assert_eq!(shr(-16, 2), -4);
        }

        /// Rust's `<<` panics on an over-wide count in debug builds. Masking is
        /// what keeps a program's mistake from becoming an engine abort.
        #[test]
        fn an_over_wide_shift_count_wraps() {
            assert_eq!(shl(1, 64), 1, "64 masks to 0");
            let _ = shl(1, 200);
            let _ = shr(1, 200);
        }

        #[test]
        fn arithmetic_wraps_rather_than_panicking() {
            assert_eq!(i64::MAX.wrapping_add(1), i64::MIN);
            assert_eq!(i64::MIN.wrapping_sub(1), i64::MAX);
        }
    }

    #[test]
    fn arithmetic() {
        assert_eq!(int_of("(+ 2 3)"), 5);
        assert_eq!(int_of("(- 9 4)"), 5);
        assert_eq!(int_of("(* 6 7)"), 42);
        assert_eq!(int_of("(/ 7 2)"), 3, "integer division truncates");
        assert_eq!(int_of("(% 7 2)"), 1);
    }

    #[test]
    fn bitwise() {
        assert_eq!(int_of("(& 12 10)"), 8);
        assert_eq!(int_of("(| 12 10)"), 14);
        assert_eq!(int_of("(^ 12 10)"), 6);
        assert_eq!(int_of("(~ 0)"), -1);
        assert_eq!(int_of("(<< 1 4)"), 16);
        assert_eq!(int_of("(>> 16 4)"), 1);
    }

    /// ZERO, not a raise. x-engine-c was asked and that is its answer; trapping
    /// would be a policy decision, and policy is x-lang's. `checked_div` is how
    /// that is spelled without Rust panicking.
    #[test]
    fn division_by_zero_answers_zero() {
        assert_eq!(int_of("(/ 1 0)"), 0);
        assert_eq!(int_of("(% 1 0)"), 0);
    }

    /// `i64::MIN / -1` overflows; `wrapping_div` is why this is a value and not
    /// an abort.
    #[test]
    fn division_overflow_wraps_rather_than_panicking() {
        assert!(!raises("(/ (- 0 9223372036854775807) (- 0 1))"));
    }

    #[test]
    fn comparison_answers_truthy_or_nil() {
        assert!(truthy("(< 1 2)"));
        assert!(!truthy("(< 2 1)"));
        assert!(truthy("(= 3 3)"));
        assert!(!truthy("(= 3 4)"));
    }

    /// The coordinate and the bare name are ONE object, not two that behave
    /// alike. Two separately-registered primitives would pass every behavioural
    /// test above and fail this one, which is the failure x-lang's suite looks
    /// for when it checks that a filed coordinate "agrees with its bare binding".
    #[test]
    fn coordinate_and_bare_binding_are_the_same_object() {
        let (e, v) = eval_ok("(same? + (%coord (lit int) (lit +)))");
        assert!(e.objects.truthy(v), "(int +) and + must be the same object");
    }

    /// A nil operand RAISES — #52 ruled, and the raw prims are the only
    /// guard on the bare-core and child-base paths (#239). A missing
    /// operand is nil, so `(+ 1)` raises too; the library's variadic fold
    /// is what answers 1 there, one layer up.
    #[test]
    fn a_nil_operand_raises() {
        assert!(raises("(+ 1)"));
        assert!(raises("(+ 1 ())"));
        assert!(raises("(- ())"));
        assert!(raises("(* () 2)"));
        assert!(raises("(/ 6 ())"));
        assert!(raises("(% () 2)"));
    }

    /// Unary minus negates; the tower layer owns typed negation.
    #[test]
    fn unary_minus_negates() {
        assert_eq!(int_of("(- 5)"), -5);
    }

    /// Extra operands are ignored: a body indexes the slots its arity declares.
    #[test]
    fn extra_operands_are_ignored() {
        assert_eq!(int_of("(+ 1 2 99)"), 3);
    }

    /// THE LAYER, stated as a test. A machine reads the operand word and applies
    /// its operator. Deciding a word is "not a number" is a type judgement, and
    /// types live one layer up in x-lang — x-engine-c runs this too.
    #[test]
    fn a_non_number_operand_is_read_not_refused() {
        assert!(!raises("(+ 1 (lit a))"));
        assert!(!raises(r#"(* 2 "s")"#));
    }
}
