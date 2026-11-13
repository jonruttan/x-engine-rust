//! The objects door: pointer casts and unchecked memory.
//!
//! A pointer is a real machine address. One inside the arena is read and
//! written through the heap's Vec; one outside it goes through the foreign
//! crate, which holds the engine's only `unsafe`.
//!
//! Nearly everything here reaches through `objects.heap`, and the spelling is
//! deliberate: these instructions work BELOW the object model, on bytes and
//! words. A primitive that says `objects.heap` is saying which layer it is at.
//!
//! The block operations are UNCHECKED, like `first`/`rest`: the caller is trusted
//! with lengths. Guarding here would tax every byte of every codec in the
//! language, and the ruling is x-lang's, not this engine's to soften.

use crate::diag::Cond;
use crate::obj::Addr;
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
    let at = a_.str_bytes(a[0]);
    Ok(a_.ptr(at))
}

/// A COPY of the NUL-bounded bytes at the pointer, as the reference's
/// `x_mkstr` copies — the caller may free or reuse the region, and a view
/// would dangle with it.
fn ptr_to_str(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let at = a_.as_ptr(a[0]);
    let mut bytes = Vec::new();
    let mut p = at.raw();
    loop {
        let b = rd_byte(a_, p);
        if b == 0 {
            break;
        }
        bytes.push(b);
        p += 1;
    }
    Ok(a_.str_from_bytes(&bytes))
}

/// Whether an accessor's address is inside the arena, where the heap's Vec
/// reaches it; anything else is process memory.
fn in_arena(a_: &Objects, at: u64) -> bool {
    a_.heap.from_real(at).is_some()
}

/// One byte at an operand address.
fn rd_byte(a_: &Objects, at: u64) -> u8 {
    if in_arena(a_, at) {
        a_.heap.byte(Addr::new(at))
    } else {
        crate::foreign::read_byte(at)
    }
}

fn wr_byte(a_: &mut Objects, at: u64, v: u8) {
    if in_arena(a_, at) {
        a_.heap.set_byte(Addr::new(at), v);
    } else {
        crate::foreign::write_byte(at, v);
    }
}

fn rd_word(a_: &Objects, at: u64) -> u64 {
    if in_arena(a_, at) {
        a_.heap.word(Addr::new(at)).raw()
    } else {
        crate::foreign::read_word(at)
    }
}

fn wr_word(a_: &mut Objects, at: u64, v: u64) {
    if in_arena(a_, at) {
        a_.heap.set_word(Addr::new(at), crate::obj::Word(v));
    } else {
        crate::foreign::write_word(at, v);
    }
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
    let at = base.offset(off);
    let mut v = 0u64;
    for i in 0..w as u64 {
        v |= (rd_byte(a_, at.raw() + i) as u64) << (8 * i);
    }
    Ok(a_.int(v as i64))
}

/// `(ptr set! p off value width)`
fn ptr_set(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let base = a_.as_ptr(a[0]);
    let off = a_.as_int(a[1]);
    let v = a_.as_int(a[2]) as u64;
    let w = a_.as_int(a[3]).clamp(0, 8) as u32;
    let at = base.offset(off);
    for i in 0..w as u64 {
        wr_byte(a_, at.raw() + i, ((v >> (8 * i)) & 0xff) as u8);
    }
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
    Ok(a_.int(rd_word(a_, at.raw()) as i64))
}

fn ptr_set_word(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let base = a_.as_ptr(a[0]);
    let off = a_.as_int(a[1]);
    let v = a_.as_int(a[2]) as u64;
    let at = base.offset(off);
    wr_word(a_, at.raw(), v);
    Ok(a[0])
}

fn mem_cmp(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let pa = a_.as_ptr(a[0]);
    let pb = a_.as_ptr(a[1]);
    let n = a_.as_int(a[2]).max(0) as u64;
    let mut r: i64 = 0;
    for i in 0..n {
        let x = rd_byte(a_, pa.raw() + i);
        let y = rd_byte(a_, pb.raw() + i);
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
    for i in 0..n {
        let b = rd_byte(a_, ps.raw() + i);
        wr_byte(a_, pd.raw() + i, b);
    }
    Ok(NIL)
}

fn mem_set(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let pd = a_.as_ptr(a[0]);
    let v = a_.as_int(a[1]) as u8;
    let n = a_.as_int(a[2]).max(0) as u64;
    for i in 0..n {
        wr_byte(a_, pd.raw() + i, v);
    }
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

/// The pointer an operand names, or nil for a null answer.
fn ptr_or_nil(a_: &mut Objects, at: u64) -> Obj {
    if at == 0 {
        NIL
    } else {
        a_.ptr(Addr::new(at))
    }
}

/// `(ptr alloc n)` — malloc(3): a region C owns, or nil.
fn ptr_alloc(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let n = a_.as_int(a[0]).max(0) as usize;
    Ok(ptr_or_nil(a_, crate::foreign::alloc_c(n)))
}

/// `(ptr free! p)` — free(3) of what `ptr alloc` or C handed out.
fn ptr_free(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    crate::foreign::free_c(a_.as_ptr(a[0]).raw());
    Ok(NIL)
}

/// `(ptr copy! dst src n)` — answers dst.
fn ptr_copy(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    mem_copy(a_, a)?;
    Ok(a[0])
}

/// `(ptr fill! p byte n)` — answers p.
fn ptr_fill(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    mem_set(a_, a)?;
    Ok(a[0])
}

/// Bytes from `at` up to (not including) the NUL.
fn c_len(a_: &Objects, at: u64) -> u64 {
    let mut n = 0;
    while rd_byte(a_, at + n) != 0 {
        n += 1;
    }
    n
}

/// strncmp(3) over at most `n` bytes: the difference of the first unequal
/// pair, as unsigned bytes.
fn c_cmp(a_: &Objects, pa: u64, pb: u64, n: u64) -> i64 {
    for i in 0..n {
        let x = rd_byte(a_, pa + i);
        let y = rd_byte(a_, pb + i);
        if x != y || x == 0 {
            return x as i64 - y as i64;
        }
    }
    0
}

/// `(ptr strlen p)`
fn ptr_strlen(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let n = c_len(a_, a_.as_ptr(a[0]).raw());
    Ok(a_.int(n as i64))
}

/// `(ptr strcmp a b)`
fn ptr_strcmp(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let (pa, pb) = (a_.as_ptr(a[0]).raw(), a_.as_ptr(a[1]).raw());
    let r = c_cmp(a_, pa, pb, u64::MAX);
    Ok(a_.int(r))
}

/// `(ptr strncmp a b n)`
fn ptr_strncmp(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let (pa, pb) = (a_.as_ptr(a[0]).raw(), a_.as_ptr(a[1]).raw());
    let n = a_.as_int(a[2]).max(0) as u64;
    let r = c_cmp(a_, pa, pb, n);
    Ok(a_.int(r))
}

/// `(ptr strchr p c)` — the first byte `c`, the terminator included as
/// strchr(3) finds it, or nil.
fn ptr_strchr(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let p = a_.as_ptr(a[0]).raw();
    let c = a_.as_int(a[1]) as u8;
    let mut at = p;
    loop {
        let b = rd_byte(a_, at);
        if b == c {
            return Ok(a_.ptr(Addr::new(at)));
        }
        if b == 0 {
            return Ok(NIL);
        }
        at += 1;
    }
}

/// `(ptr strndup p n)` — at most `n` bytes into a malloc'd, NUL-terminated
/// region the caller owns, or nil.
fn ptr_strndup(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let p = a_.as_ptr(a[0]).raw();
    let n = c_len(a_, p).min(a_.as_int(a[1]).max(0) as u64);
    let dst = crate::foreign::alloc_c(n as usize + 1);
    if dst == 0 {
        return Ok(NIL);
    }
    for i in 0..n {
        let b = rd_byte(a_, p + i);
        wr_byte(a_, dst + i, b);
    }
    wr_byte(a_, dst + n, 0);
    Ok(a_.ptr(Addr::new(dst)))
}

/// `(int abs n)` — abs(3) of the value as a C int, as the reference takes it.
fn int_abs(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let n = a_.as_int(a[0]) as i32;
    Ok(a_.int(n.wrapping_abs() as i64))
}

crate::uniform_value!(ptr_alloc_u, ptr_alloc, 1);
crate::uniform_value!(ptr_free_u, ptr_free, 1);
crate::uniform_value!(ptr_copy_u, ptr_copy, 3);
crate::uniform_value!(ptr_fill_u, ptr_fill, 3);
crate::uniform_value!(ptr_strlen_u, ptr_strlen, 1);
crate::uniform_value!(ptr_strcmp_u, ptr_strcmp, 2);
crate::uniform_value!(ptr_strncmp_u, ptr_strncmp, 3);
crate::uniform_value!(ptr_strchr_u, ptr_strchr, 2);
crate::uniform_value!(ptr_strndup_u, ptr_strndup, 2);
crate::uniform_value!(int_abs_u, int_abs, 1);
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
    PrimDef::row(None, Some(("ptr", "alloc")), 1, ptr_alloc_u),
    PrimDef::row(None, Some(("ptr", "strlen")), 1, ptr_strlen_u),
    PrimDef::row(None, Some(("ptr", "strcmp")), 2, ptr_strcmp_u),
    PrimDef::row(None, Some(("ptr", "strncmp")), 3, ptr_strncmp_u),
    PrimDef::row(None, Some(("ptr", "strchr")), 2, ptr_strchr_u),
    PrimDef::row(None, Some(("ptr", "strndup")), 2, ptr_strndup_u),
    PrimDef::row(None, Some(("int", "abs")), 1, int_abs_u),
    PrimDef::row(None, Some(("ptr", "free!")), 1, ptr_free_u),
    PrimDef::row(None, Some(("ptr", "copy!")), 3, ptr_copy_u),
    PrimDef::row(None, Some(("ptr", "fill!")), 3, ptr_fill_u),
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
