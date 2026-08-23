//! Doubles, carried as bit patterns.
//!
//! This engine has no float. Its number is an integer, and x-lang's floats are
//! x-lang's — built one layer up out of the parts this layer hands over. `ffi
//! call` is where those parts are handed over: a double travels as the integer
//! holding its IEEE-754 bits, in both directions, which is what lets a bare
//! engine with no float type be tested for floating-point behaviour at all.
//!
//! Nothing here knows what an object is, and nothing here is unsafe. Most of the
//! `ffi call` conventions never call anything foreign — `d+d` adds two doubles,
//! `d->i` truncates one — and putting them behind the foreign door would have
//! meant writing them next to `unsafe` for no reason. Only `d->d`, `dd->d` and
//! `s0->d` actually cross, and they are the three that live in
//! [`crate::foreign`].

use crate::vocabulary::{
    CONV_ADD, CONV_DIV, CONV_EQ, CONV_GE, CONV_GT, CONV_LE, CONV_LT, CONV_MUL, CONV_SUB,
};

/// Reinterpret an integer's bits as the double they spell.
///
/// A CAST, not a conversion: `bits(1)` is 5e-324, not 1.0. That is the
/// representation `ffi call` uses, and the reference engine does the same thing
/// with a `memcpy` between an `x_int_t` and a `double`.
pub fn from_bits(bits: u64) -> f64 {
    f64::from_bits(bits)
}

pub fn to_bits(v: f64) -> u64 {
    v.to_bits()
}

/// `i->d` — the VALUE, converted. The one convention that is not a bit cast.
pub fn from_int(i: i64) -> u64 {
    (i as f64).to_bits()
}

/// `d->i` — truncation toward zero, which is C's conversion and not Rust's
/// rounding.
///
/// Rust's `as` saturates where C's is undefined on overflow. Saturating is the
/// better of the two behaviours and neither is x-lang's concern: the library
/// works in the range where they agree.
pub fn to_int(bits: u64) -> i64 {
    f64::from_bits(bits) as i64
}

/// `d->s` — C's `%.15g`, which Rust has no formatting directive for.
///
/// Reimplemented rather than approximated because this is the printed form of
/// every float x-lang shows, so a difference here is a difference in the
/// language's output. `%g` picks between fixed and exponential notation by the
/// value's exponent, then strips trailing zeros:
///
/// ```
/// use x_engine::dbl;
/// assert_eq!(dbl::to_str(1.5f64.to_bits()), "1.5");
/// assert_eq!(dbl::to_str(0.0f64.to_bits()), "0");
/// assert_eq!(dbl::to_str(100.0f64.to_bits()), "100");
/// // Past 15 significant digits it switches to exponential, as C does.
/// assert_eq!(dbl::to_str(1e20f64.to_bits()), "1e+20");
/// assert_eq!(dbl::to_str(1e-5f64.to_bits()), "1e-05");
/// ```
pub fn to_str(bits: u64) -> String {
    const P: i32 = 15;
    let v = f64::from_bits(bits);
    if v.is_nan() {
        return crate::vocabulary::NAN.into();
    }
    if v.is_infinite() {
        return if v < 0.0 {
            crate::vocabulary::NEG_INF.into()
        } else {
            crate::vocabulary::INF.into()
        };
    }
    // The exponent %e would print, obtained by asking for %e. Deriving it from
    // log10 would disagree with the formatter at the rounding boundaries.
    let sci = format!("{:.*e}", (P - 1) as usize, v);
    let (mant, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let x: i32 = exp.parse().unwrap_or(0);
    if !(-4..P).contains(&x) {
        // C pads the exponent to at least two digits and always signs it.
        format!(
            "{}e{}{:02}",
            trim(mant),
            if x < 0 { '-' } else { '+' },
            x.abs()
        )
    } else {
        trim(&format!("{:.*}", (P - 1 - x).max(0) as usize, v))
    }
}

/// Drop trailing zeros from a fractional part, then a bare trailing point.
fn trim(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The arithmetic conventions: two doubles in, one out, all as bits.
pub fn arith(op: &str, a: u64, b: u64) -> Option<u64> {
    let (x, y) = (f64::from_bits(a), f64::from_bits(b));
    let r = match op {
        CONV_ADD => x + y,
        CONV_SUB => x - y,
        CONV_MUL => x * y,
        // NOT guarded. Dividing by zero is infinity in IEEE-754 and that is an
        // answer, not an error — and even if it were, whether to refuse it is
        // x-lang's question, not this layer's.
        CONV_DIV => x / y,
        _ => return None,
    };
    Some(r.to_bits())
}

/// The comparison conventions: two doubles in, a truth value out.
///
/// `None` for a spelling that is not one of these, so the caller can tell "not a
/// comparison" from "false".
pub fn compare(op: &str, a: u64, b: u64) -> Option<bool> {
    let (x, y) = (f64::from_bits(a), f64::from_bits(b));
    match op {
        CONV_LT => Some(x < y),
        CONV_GT => Some(x > y),
        // IEEE equality, so NaN is equal to nothing including itself. The
        // reference engine uses C's `==` and inherits exactly this.
        CONV_EQ => Some(x == y),
        CONV_LE => Some(x <= y),
        CONV_GE => Some(x >= y),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bit patterns x-lang's own conformance case uses, so a disagreement
    /// here shows up as a unit failure before it shows up as a spec failure.
    #[test]
    fn the_conformance_bit_patterns_mean_what_the_spec_says() {
        assert_eq!(from_bits(4616189618054758400), 4.0);
        assert_eq!(from_bits(4611686018427387904), 2.0);
        assert_eq!(from_bits(4613937818241073152), 3.0);
        assert_eq!(from_bits(4621256167635550208), 9.0);
    }

    /// `i->d` converts the VALUE; everything else reinterprets BITS. Getting
    /// these two the same way round is the whole difficulty of this module.
    #[test]
    fn int_to_double_converts_while_the_rest_reinterpret() {
        assert_eq!(from_bits(from_int(1)), 1.0);
        assert_eq!(from_bits(1), 5e-324);
    }

    #[test]
    fn double_to_int_truncates_toward_zero() {
        assert_eq!(to_int(to_bits(3.9)), 3);
        assert_eq!(to_int(to_bits(-3.9)), -3);
        assert_eq!(to_int(to_bits(0.5)), 0);
    }

    #[test]
    fn the_arithmetic_conventions_are_ordinary_arithmetic() {
        let (a, b) = (to_bits(3.0), to_bits(2.0));
        assert_eq!(from_bits(arith("d+d", a, b).unwrap()), 5.0);
        assert_eq!(from_bits(arith("d-d", a, b).unwrap()), 1.0);
        assert_eq!(from_bits(arith("d*d", a, b).unwrap()), 6.0);
        assert_eq!(from_bits(arith("d/d", a, b).unwrap()), 1.5);
        assert!(arith("d?d", a, b).is_none());
    }

    /// IEEE-754 says division by zero is infinity, and this layer has no opinion
    /// about whether that is welcome.
    #[test]
    fn dividing_by_zero_is_infinity_rather_than_a_refusal() {
        let r = from_bits(arith("d/d", to_bits(1.0), to_bits(0.0)).unwrap());
        assert!(r.is_infinite() && r > 0.0);
    }

    #[test]
    fn the_comparison_conventions_answer_truth_values() {
        let (a, b) = (to_bits(1.0), to_bits(2.0));
        assert_eq!(compare("d<d", a, b), Some(true));
        assert_eq!(compare("d>d", a, b), Some(false));
        assert_eq!(compare("d=d", a, a), Some(true));
        assert_eq!(compare("d<=d", a, a), Some(true));
        assert_eq!(compare("d>=d", a, b), Some(false));
        assert_eq!(compare("d~d", a, b), None);
    }

    /// NaN compares equal to nothing, itself included. The reference engine uses
    /// C's `==` and inherits this; matching it deliberately.
    #[test]
    fn nan_is_equal_to_nothing_including_itself() {
        let n = to_bits(f64::NAN);
        assert_eq!(compare("d=d", n, n), Some(false));
        assert_eq!(compare("d<d", n, n), Some(false));
        assert_eq!(compare("d>=d", n, n), Some(false));
    }

    /// The printed form is the language's output, so these are checked against
    /// what C's `%.15g` produces rather than against what looks reasonable.
    #[test]
    fn the_printed_form_matches_c_percent_point_fifteen_g() {
        for (v, want) in [
            (0.0, "0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (1.5, "1.5"),
            (100.0, "100"),
            (0.1, "0.1"),
            (1.0 / 3.0, "0.333333333333333"),
            (1e15, "1e+15"),
            (1e14, "100000000000000"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1.5e300, "1.5e+300"),
            (-2.5e-11, "-2.5e-11"),
        ] {
            assert_eq!(to_str(to_bits(v)), want, "for {}", v);
        }
    }

    #[test]
    fn the_non_finite_forms_are_cs_spellings() {
        assert_eq!(to_str(to_bits(f64::NAN)), "nan");
        assert_eq!(to_str(to_bits(f64::INFINITY)), "inf");
        assert_eq!(to_str(to_bits(f64::NEG_INFINITY)), "-inf");
    }
}
