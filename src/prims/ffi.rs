//! The foreign and syscall doors.
//!
//! Mirrors the reference engine's `x-prim/ffi.c`. The unsafety lives in
//! [`crate::foreign`]; this file is marshalling — turning x-lang values into the
//! integers a C call wants, and back.
//!
//! These are the capabilities a sandboxed or wasm engine drops, which is why
//! `features.x` splits the ISA's `ffi` tag three ways: the pointer CASTS stay
//! mandatory because `lib/x/boot` needs them, while this family can be absent.

use crate::dbl;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::foreign::{self, Foreign};
use crate::obj::{Obj, NIL, WORD};
use crate::prim::PrimDef;
use crate::vocabulary::{
    CONV_DD_TO_D, CONV_D_TO_D, CONV_D_TO_I, CONV_D_TO_S, CONV_I_TO_D, CONV_S0_TO_D,
};

/// A NUL-terminated byte run for an operand, if it is a string.
fn cstr(e: &Engine, o: Obj) -> Option<Vec<u8>> {
    if !e.objects.is_str(o) {
        return None;
    }
    let mut b = e.objects.bytes_of(o);
    b.push(0);
    Some(b)
}

/// Marshal one operand into the machine word a C call receives.
///
/// A STRING becomes the real address of its bytes in this engine's heap. They
/// are already NUL-terminated there, so no copy is needed and `strlen` on the
/// result is the length x-lang would report — which is the whole reason the heap
/// stores strings terminated rather than counted.
fn marshal(e: &Engine, o: Obj) -> u64 {
    if e.objects.is_str(o) {
        let at = e.objects.str_bytes(o);
        return e.objects.heap.address_of(at);
    }
    if e.objects.is_foreign(o) {
        return e.objects.foreign_addr(o);
    }
    // A PTR INTO THIS ENGINE'S HEAP BECOMES A REAL ADDRESS, exactly as a string
    // does. Strings were converted and pointers were not, so a pointer reached
    // libc as a raw heap OFFSET — a small integer, not an address.
    //
    // `Sys wait` is the case that showed it: x-lang hands `waitpid` a four-byte
    // region as `(%str->ptr s)`, and waitpid wrote nowhere. The region kept the
    // space fill `str make` leaves, so `(%ptr-ref buf 0 4)` read 0x20202020 and
    // the decode read its low seven bits as "killed by signal 32" — every
    // `Proc run!` answered 160, whatever the child did, which is also the
    // `cc failed with status 160` behind the compiled tower.
    //
    // The two address spaces do not overlap, which is what makes the test sound:
    // a heap offset is bounded by the heap, and anything the foreign side hands
    // back (a malloc result, a dlsym) is a process address far above it.
    if e.objects.is_ptr(o) {
        let at = e.objects.as_ptr(o);
        if (at.raw() as usize) < e.objects.heap.words_len() * WORD {
            return e.objects.heap.address_of(at);
        }
        return at.raw();
    }
    e.objects.as_int(o) as u64
}

/// `(ffi dlopen path flags)` — nil path is the SELF handle.
fn dlopen(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let path = cstr(e, a[0]);
    let flags = e.objects.as_int(a[1]) as i32;
    let h = foreign::open(path.as_deref(), flags);
    Ok(if h.is_null() {
        NIL
    } else {
        e.objects.foreign(h.0)
    })
}

/// `(ffi dlsym lib "name")` — nil when unresolvable, because the library
/// branches on a nil handle rather than guarding every lookup.
fn dlsym(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let Some(name) = cstr(e, a[1]) else {
        return Ok(NIL);
    };
    let lib = Foreign(marshal(e, a[0]));
    let p = foreign::sym(lib, &name);
    Ok(if p.is_null() {
        NIL
    } else {
        e.objects.foreign(p.0)
    })
}

/// `(ptr call f args...)` — the integer convention, up to seven arguments.
fn ptr_call(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let f = Foreign(marshal(e, a[0]));
    let args: Vec<u64> = a[1..].iter().map(|&o| marshal(e, o)).collect();
    let r = foreign::call_ints(f, &args);
    Ok(e.objects.int(r as i64))
}

/// `(ffi call "conv" f args...)` — the double door, in fifteen spellings.
///
/// The set is CLOSED and the spellings are the reference engine's, because a
/// convention is chosen by the library at the call site: `lib/` asks for twelve
/// of these by name, and a spelling this engine does not recognise is not a
/// degraded answer but a nil where a number belongs.
///
/// Doubles travel as IEEE-754 bits through integers in both directions. That is
/// what makes the door usable from an engine with no float type — and it is why
/// most of these conventions do not call anything at all. `d+d` adds two
/// doubles; only `d->d`, `dd->d` and `s0->d` reach through the pointer, and only
/// those three touch [`crate::foreign`].
fn ffi_call(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let conv = e.objects.str_val(a[0]);
    let bits: Vec<u64> = a[2..].iter().map(|&o| marshal(e, o)).collect();
    let arg = |n: usize| bits.get(n).copied().unwrap_or(0);

    // The comparisons answer a truth value rather than a number, so they leave
    // before the shared tail below.
    if let Some(b) = dbl::compare(&conv, arg(0), arg(1)) {
        return Ok(e.objects.truth(b));
    }
    // As does the one that answers a string.
    if conv == CONV_D_TO_S {
        let text = dbl::to_str(arg(0));
        return Ok(e.objects.str_new(&text));
    }

    let r = match conv.as_str() {
        // --- through the pointer ---
        CONV_D_TO_D => foreign::call_doubles(Foreign(marshal(e, a[1])), &bits[..bits.len().min(1)]),
        CONV_DD_TO_D => {
            foreign::call_doubles(Foreign(marshal(e, a[1])), &bits[..bits.len().min(2)])
        }
        CONV_S0_TO_D => foreign::call_str_to_double(Foreign(marshal(e, a[1])), arg(0)),
        // --- casts ---
        CONV_I_TO_D => dbl::from_int(e.objects.as_int(a[2])),
        CONV_D_TO_I => return Ok(e.objects.int(dbl::to_int(arg(0)))),
        // --- arithmetic ---
        _ => match dbl::arith(&conv, arg(0), arg(1)) {
            Some(bits) => bits,
            // An unrecognised convention answers nil, as the reference does.
            // Guessing one would not give a wrong number here — it would read
            // the wrong registers.
            None => return Ok(NIL),
        },
    };
    Ok(e.objects.int(r as i64))
}

/// `(syscall n args...)` — bare, because `lib/x/sys/` reaches it by name.
fn syscall(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let n = e.objects.as_int(a[0]);
    let args: Vec<u64> = a[1..].iter().map(|&o| marshal(e, o)).collect();
    Ok(e.objects.int(foreign::kernel(n, &args)))
}

pub const TABLE: &[PrimDef] = &[
    PrimDef::filed_full("ffi", "dlopen", 2, dlopen),
    PrimDef::filed_full("ffi", "dlsym", 2, dlsym),
    PrimDef::var_both("ptr-call", "ptr", "call", 1, ptr_call),
    PrimDef::var_both("ffi-call", "ffi", "call", 2, ffi_call),
    PrimDef::var_bare("syscall", 1, syscall),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{int_of, str_of, truthy, with_coords};

    const FFI: &[(&str, &str, &str)] = &[
        ("%dlopen", "ffi", "dlopen"),
        ("%dlsym", "ffi", "dlsym"),
        ("%pcall", "ptr", "call"),
        ("%fcall", "ffi", "call"),
    ];

    /// Every case needs the self handle, so the prelude opens it once — and
    /// wraps resolution in `%must`.
    ///
    /// A nil from `dlsym` that gets called anyway is a call through address
    /// zero, and a test that dies of SIGSEGV reports nothing a reader can act
    /// on: a Linux runner once turned a missing libm into "signal: 11" with no
    /// symbol name attached. Raising names the symbol instead.
    fn ffi(body: &str) -> String {
        with_coords(
            FFI,
            &format!(
                r#"(def %lib (%dlopen () 1))
                   (def %must (fn (self p) (match ((eq? p ()) (error "unresolved")) (#t p))))
                   {}"#,
                body
            ),
        )
    }

    #[test]
    fn the_process_handle_resolves_and_a_symbol_can_be_called() {
        // strlen of a five-byte string, through the pointer.
        assert_eq!(
            int_of(&ffi(r#"(%pcall (%must (%dlsym %lib "strlen")) "hello")"#)),
            5
        );
    }

    /// NIL, not a raise: the library branches on a nil handle in several places
    /// rather than guarding every lookup.
    #[test]
    fn an_unresolvable_symbol_is_nil() {
        assert!(truthy(&ffi(
            r#"(match ((eq? (%dlsym %lib "no_such_symbol_at_all") ()) 1) (#t ()))"#
        )));
    }

    /// The bit patterns are x-lang's own: sqrt(4.0) = 2.0.
    #[test]
    fn a_double_call_carries_bits_in_both_directions() {
        assert_eq!(
            int_of(&ffi(
                r#"(%fcall "d->d" (%must (%dlsym %lib "sqrt")) 4616189618054758400)"#
            )),
            4611686018427387904
        );
        // pow(3.0, 2.0) = 9.0
        assert_eq!(
            int_of(&ffi(
                r#"(%fcall "dd->d" (%must (%dlsym %lib "pow")) 4613937818241073152 4611686018427387904)"#
            )),
            4621256167635550208
        );
    }

    /// Most conventions call NOTHING. The function operand is ignored for them,
    /// and passing nil proves it rather than merely asserting it.
    #[test]
    fn the_arithmetic_conventions_call_nothing() {
        // 3.0 + 2.0 = 5.0
        assert_eq!(
            int_of(&ffi(
                r#"(%fcall "d+d" () 4613937818241073152 4611686018427387904)"#
            )),
            4617315517961601024
        );
        // 3.0 / 2.0 = 1.5
        assert_eq!(
            int_of(&ffi(
                r#"(%fcall "d/d" () 4613937818241073152 4611686018427387904)"#
            )),
            4609434218613702656
        );
    }

    /// The comparisons answer a TRUTH VALUE, not a number, which is a different
    /// return shape from every other convention.
    #[test]
    fn the_comparisons_answer_truth_values() {
        assert!(truthy(&ffi(
            r#"(%fcall "d<d" () 4611686018427387904 4613937818241073152)"#
        )));
        assert!(!truthy(&ffi(
            r#"(%fcall "d>d" () 4611686018427387904 4613937818241073152)"#
        )));
        assert!(truthy(&ffi(
            r#"(%fcall "d=d" () 4611686018427387904 4611686018427387904)"#
        )));
    }

    /// `i->d` converts a VALUE where the rest reinterpret BITS, and `d->i` goes
    /// back. Round-tripping is the check that they agree about direction.
    #[test]
    fn the_casts_round_trip_a_value() {
        assert_eq!(
            int_of(&ffi(r#"(%fcall "d->i" () (%fcall "i->d" () 42))"#)),
            42
        );
    }

    /// The printed form, which is what x-lang shows for a float.
    #[test]
    fn a_double_prints_as_c_would_print_it() {
        assert_eq!(
            str_of(&ffi(r#"(%fcall "d->s" () 4609434218613702656)"#)),
            "1.5"
        );
    }

    /// strtod, through the convention shaped for it.
    #[test]
    fn a_string_parses_to_a_double_through_the_pointer() {
        assert_eq!(
            int_of(&ffi(
                r#"(%fcall "s0->d" (%must (%dlsym %lib "strtod")) "1.5")"#
            )),
            4609434218613702656
        );
    }

    /// An unrecognised convention answers nil rather than guessing one.
    #[test]
    fn an_unknown_convention_is_nil() {
        assert!(truthy(&ffi(
            r#"(match ((eq? (%fcall "q->q" () 1) ()) 1) (#t ()))"#
        )));
    }

    /// The kernel door, bare. `getpid` cannot fail and answers something
    /// positive, which is checkable without depending on the value.
    #[test]
    fn the_kernel_door_is_reachable_bare() {
        let n = if cfg!(target_os = "macos") {
            0x2000014
        } else {
            39
        };
        assert!(int_of(&format!("(syscall {})", n)) > 0);
    }
}
