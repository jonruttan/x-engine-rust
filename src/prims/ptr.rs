//! The objects door: pointer casts and unchecked memory.
//!
//! A "pointer" here is a BYTE OFFSET into the objects, never a machine address.
//! Nothing outside this engine dereferences one — the `core` profile has no
//! foreign door, no `dlopen` and no `ptr call` — so every offset x-lang computes
//! from `obj-layout.x` lands back inside the objects by construction. That is what
//! lets the whole engine be safe Rust, and it stops being true the day a foreign
//! door is added.
//!
//! Nearly everything here reaches through `objects.heap`, and the spelling is
//! deliberate: these instructions work BELOW the object model, on bytes and
//! words. A primitive that says `objects.heap` is saying which layer it is at.
//!
//! The block operations are UNCHECKED, like `first`/`rest`: the caller is trusted
//! with lengths. Guarding here would tax every byte of every codec in the
//! language, and the ruling is x-lang's, not this engine's to soften.

use crate::diag::Cond;
use crate::obj::{Addr, Word};
use crate::obj::{Obj, NIL};
use crate::objects::Objects;
use crate::prim::PrimDef;

fn obj_to_ptr(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.ptr(a[0].addr()))
}

fn ptr_to_obj(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.as_ptr(a[0]).as_obj())
}

fn ptr_to_int(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let v = a_.as_ptr(a[0]);
    Ok(a_.int(v.raw() as i64))
}

fn int_to_ptr(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let v = a_.as_int(a[0]);
    Ok(a_.ptr(Addr::new(v as u64)))
}

fn str_to_ptr(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let s = a[0];
    let at = a_.str_bytes(s);
    Ok(a_.ptr(at))
}

/// A COPY of the NUL-bounded bytes at the pointer, as the reference's
/// `x_mkstr` copies — the caller may free or reuse the region, and a view
/// would dangle with it.
fn ptr_to_str(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let at = a_.as_ptr(a[0]);
    let bytes = a_.heap.bytes_at(at);
    Ok(a_.str_from_bytes(&bytes))
}

/// `(ptr ref p off width)` — `width` bytes from `p+off`, assembled into the low
/// end of a zeroed integer.
///
/// LITTLE-ENDIAN without deciding to be: the objects is word storage, byte i of a
/// word is its i-th lowest, and a widening read assembles from the low end. On a
/// big-endian host the same code answers differently, which is exactly why
/// x-lang records `endian` as a constraint rather than legislating it.
fn ptr_ref(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let base = a_.as_ptr(a[0]);
    let off = a_.as_int(a[1]);
    let w = a_.as_int(a[2]).clamp(0, 8) as u32;
    let v = a_.heap.read_le(base.offset(off), w);
    Ok(a_.int(v as i64))
}

/// `(ptr set! p off value width)`
fn ptr_set(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let base = a_.as_ptr(a[0]);
    let off = a_.as_int(a[1]);
    let v = a_.as_int(a[2]) as u64;
    let w = a_.as_int(a[3]).clamp(0, 8) as u32;
    a_.heap.write_le(base.offset(off), v, w);
    Ok(NIL)
}

/// `(ptr ref-word p byte-off)` — the offset is in BYTES and may be NEGATIVE.
/// `lib/x/boot/reflect.x` multiplies slot indices by the word size itself, and
/// reads prepended meta units at offsets below the object. The name suggests a
/// word index; the caller settles it.
fn ptr_ref_word(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let base = a_.as_ptr(a[0]);
    let off = a_.as_int(a[1]);
    let at = base.offset(off);
    Ok(a_.int(a_.heap.word(at).as_i64()))
}

fn ptr_set_word(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let base = a_.as_ptr(a[0]);
    let off = a_.as_int(a[1]);
    let v = a_.as_int(a[2]) as u64;
    let at = base.offset(off);
    a_.heap.set_word(at, Word(v));
    Ok(a[0])
}

fn mem_cmp(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let pa = a_.as_ptr(a[0]);
    let pb = a_.as_ptr(a[1]);
    let n = a_.as_int(a[2]).max(0) as u64;
    let mut r: i64 = 0;
    for i in 0..n {
        let x = a_.heap.byte(pa.plus(i));
        let y = a_.heap.byte(pb.plus(i));
        if x != y {
            r = if x < y { -1 } else { 1 };
            break;
        }
    }
    Ok(a_.int(r))
}

fn mem_copy(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let pd = a_.as_ptr(a[0]);
    let ps = a_.as_ptr(a[1]);
    let n = a_.as_int(a[2]).max(0) as u64;
    a_.heap.copy_bytes(pd, ps, n);
    Ok(NIL)
}

fn mem_set(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let pd = a_.as_ptr(a[0]);
    let v = a_.as_int(a[1]) as u8;
    let n = a_.as_int(a[2]).max(0) as u64;
    a_.heap.fill_bytes(pd, v, n);
    Ok(NIL)
}

fn mem_alloc(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let n = a_.as_int(a[0]).max(0) as usize;
    let at = a_.heap.alloc_bytes(n);
    Ok(a_.ptr(at))
}

/// A NO-OP, and honestly so: the objects never reuses a region, so releasing one
/// has nothing to do. An engine with a collector would owe something here; this
/// one owes nothing, for the same reason `gc/non-moving` is free rather than
/// earned.
fn mem_free(_a_: &mut Objects, _a: &[Obj]) -> Result<Obj, Cond> {
    Ok(NIL)
}

crate::uniform_value!(obj_to_ptr_u, obj_to_ptr, 1);
crate::uniform_value!(ptr_to_obj_u, ptr_to_obj, 1);
crate::uniform_value!(ptr_to_int_u, ptr_to_int, 1);
crate::uniform_value!(int_to_ptr_u, int_to_ptr, 1);
crate::uniform_value!(str_to_ptr_u, str_to_ptr, 1);
crate::uniform_value!(ptr_to_str_u, ptr_to_str, 1);
crate::uniform_value!(ptr_ref_u, ptr_ref, 3);
crate::uniform_value!(ptr_set_u, ptr_set, 4);
crate::uniform_value!(ptr_ref_word_u, ptr_ref_word, 2);
crate::uniform_value!(ptr_set_word_u, ptr_set_word, 3);
crate::uniform_value!(mem_cmp_u, mem_cmp, 3);
crate::uniform_value!(mem_copy_u, mem_copy, 3);
crate::uniform_value!(mem_set_u, mem_set, 3);
crate::uniform_value!(mem_alloc_u, mem_alloc, 1);
crate::uniform_value!(mem_free_u, mem_free, 1);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(Some("obj->ptr"), Some(("obj", "->ptr")), 1, obj_to_ptr_u),
    PrimDef::row(None, Some(("ptr", "->obj")), 1, ptr_to_obj_u),
    PrimDef::row(None, Some(("ptr", "->int")), 1, ptr_to_int_u),
    PrimDef::row(Some("int->ptr"), Some(("int", "->ptr")), 1, int_to_ptr_u),
    PrimDef::row(Some("str->ptr"), Some(("str", "->ptr")), 1, str_to_ptr_u),
    PrimDef::row(Some("ptr->str"), Some(("ptr", "->str")), 1, ptr_to_str_u),
    PrimDef::row(None, Some(("ptr", "ref")), 3, ptr_ref_u),
    PrimDef::row(None, Some(("ptr", "set!")), 4, ptr_set_u),
    PrimDef::row(None, Some(("ptr", "ref-word")), 2, ptr_ref_word_u),
    PrimDef::row(None, Some(("ptr", "set-word!")), 3, ptr_set_word_u),
    PrimDef::row(None, Some(("mem", "cmp")), 3, mem_cmp_u),
    PrimDef::row(None, Some(("mem", "copy")), 3, mem_copy_u),
    PrimDef::row(None, Some(("mem", "set")), 3, mem_set_u),
    PrimDef::row(None, Some(("mem", "alloc")), 1, mem_alloc_u),
    PrimDef::row(None, Some(("mem", "free")), 1, mem_free_u),
];

#[cfg(test)]
mod tests {
    /// The coordinates these cases reach for.
    const COORDS: &[(&str, &str, &str)] = &[
        ("%mk", "str", "make"),
        ("%s2p", "str", "->ptr"),
        ("%o2p", "obj", "->ptr"),
        ("%p2o", "ptr", "->obj"),
        ("%p2i", "ptr", "->int"),
        ("%i2p", "int", "->ptr"),
        ("%ref", "ptr", "ref"),
        ("%set", "ptr", "set!"),
        ("%refw", "ptr", "ref-word"),
        ("%setw", "ptr", "set-word!"),
        ("%cmp", "mem", "cmp"),
        ("%copy", "mem", "copy"),
        ("%fill", "mem", "set"),
        ("%alloc", "mem", "alloc"),
    ];

    fn src(body: &str) -> String {
        with_coords(COORDS, body)
    }

    use crate::testkit::{int_of, truthy, with_coords};

    /// The round trip decision L1 rests on. It holds by construction here — an
    /// object IS its own offset — which is the point: nothing to get wrong.
    #[test]
    fn an_object_round_trips_through_a_pointer() {
        assert!(truthy(&src("(def p (pair 1 2)) (same? (%p2o (%o2p p)) p)")));
    }

    #[test]
    fn a_pointer_round_trips_through_an_integer() {
        assert!(truthy(&src(
            "(def p (%s2p (%mk 8))) (= (%p2i (%i2p (%p2i p))) (%p2i p))"
        )));
    }

    #[test]
    fn a_word_written_through_a_pointer_reads_back() {
        assert_eq!(
            int_of(&src(
                "(def p (%s2p (%mk 32))) (%setw p 0 12345) (%refw p 0)"
            )),
            12345
        );
    }

    /// Each offset is written before it is read: `str make` is not promised to
    /// return zeroed memory, so asserting on a neighbouring byte would be testing
    /// the allocator's mood.
    #[test]
    fn a_byte_written_through_a_pointer_reads_back_at_its_own_offset() {
        let s = src("(def p (%s2p (%mk 32))) (%set p 3 200 1) (%set p 4 7 1) (%ref p 3 1)");
        assert_eq!(int_of(&s), 200);
        let s = src("(def p (%s2p (%mk 32))) (%set p 3 200 1) (%set p 4 7 1) (%ref p 4 1)");
        assert_eq!(int_of(&s), 7);
    }

    /// A four-byte read of the bytes 1,0,0,0 is 1, not 16777216.
    #[test]
    fn a_widening_read_is_little_endian() {
        let s = src("(def p (%s2p (%mk 32)))
             (%set p 0 1 1) (%set p 1 0 1) (%set p 2 0 1) (%set p 3 0 1)
             (%ref p 0 4)");
        assert_eq!(int_of(&s), 1);
    }

    /// reflect.x reads prepended meta units BELOW the object, so a negative
    /// offset must address backwards rather than wrapping into nonsense.
    #[test]
    fn ref_word_accepts_a_negative_byte_offset() {
        let s = src("(def p (%s2p (%mk 64)))
             (%setw p 16 99)
             (def q (%i2p (+ (%p2i p) 24)))
             (%refw q (- 0 8))");
        assert_eq!(int_of(&s), 99);
    }

    #[test]
    fn memory_copies_and_compares() {
        let s = src(r#"(def a (%s2p (%mk 8))) (def b (%s2p (%mk 8)))
               (%fill a 65 8) (%copy b a 8) (%cmp a b 8)"#);
        assert_eq!(int_of(&s), 0, "identical blocks compare equal");
        let s = src(r#"(def a (%s2p (%mk 8))) (def b (%s2p (%mk 8)))
               (%fill a 65 8) (%fill b 66 8) (%cmp a b 8)"#);
        assert_eq!(int_of(&s), -1, "a lower byte sorts first");
    }

    #[test]
    fn malloced_memory_is_writable() {
        let s = src("(def p (%alloc 16)) (%setw p 0 7) (%refw p 0)");
        assert_eq!(int_of(&s), 7);
    }
}
