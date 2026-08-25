//! Strings and symbols.
//!
//! x-lang rules that str values ARE C strings: the bytes past the NUL are
//! unobservable. Storing them NUL-terminated is what makes that true here rather
//! than something to remember at each site — `byte_len` stops at the NUL because
//! the representation says so.

use crate::diag::Cond;
use crate::obj::Obj;
use crate::objects::Objects;
use crate::prim::PrimDef;

/// A fresh n-byte region, space-filled and NUL-terminated so a byte-length read
/// sees n. NOT promised to be zeroed — x-lang's own conformance case writes every
/// offset before reading it for exactly that reason.
fn make(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let n = a_.as_int(a[0]).max(0) as usize;
    Ok(a_.str_make(n))
}

fn byte_len(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let s = a[0];
    let n = a_.byte_len(s) as i64;
    Ok(a_.int(n))
}

/// Answers a CHAR, not an integer. `char ->int` is the separate step, and
/// collapsing the two would lose the distinction the type system rests on.
fn byte_ref(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let s = a[0];
    let i = a_.as_int(a[1]).max(0) as u64;
    let at = a_.str_bytes(s);
    let b = a_.heap.byte(at.plus(i)) as u32;
    Ok(a_.char_new(b))
}
/// `(str byte-sub s off LEN)` — a length, not an end index. ADDRESSES raw
/// bytes rather than slicing the NUL-bounded value: a buffer a syscall filled
/// is binary, with real data past its first zero byte, and `byte-ref` beside
/// this addresses raw bytes too. Out-of-range reads answer 0 (`Heap::word`),
/// so no bound is needed.
fn byte_sub(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let s = a[0];
    let off = a_.as_int(a[1]).max(0) as u64;
    let len = a_.as_int(a[2]).max(0) as u64;
    let at = a_.str_bytes(s);
    let taken: Vec<u8> = (0..len).map(|i| a_.heap.byte(at.plus(off + i))).collect();
    Ok(a_.str_from_bytes(&taken))
}

fn append(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let x = a[0];
    let y = a[1];
    let mut bytes = a_.bytes_of(x);
    bytes.extend(a_.bytes_of(y));
    Ok(a_.str_from_bytes(&bytes))
}

/// INTERNS. The result must be `eq?` to the symbol of the same spelling written
/// as a literal, which is only true if both go through the one symbol table.
fn to_sym(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let s = a[0];
    let name = a_.str_val(s);
    Ok(a_.sym(&name))
}

fn sym_to_str(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let y = a[0];
    let name = a_.sym_name(y);
    Ok(a_.str_new(&name))
}

/// A list of byte values becomes a string. Chars are accepted as well as
/// integers: a caller assembling bytes has usually just read them with
/// `str byte-ref`, which answers chars.
fn bytes_to_str(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let items: Vec<Obj> = a_.list(a[0]).collect();
    let mut bytes: Vec<u8> = Vec::with_capacity(items.len());
    for v in items {
        bytes.push(a_.as_byte(v));
    }
    Ok(a_.str_from_bytes(&bytes))
}

crate::uniform_value!(make_u, make, 1);
crate::uniform_value!(byte_len_u, byte_len, 1);
crate::uniform_value!(byte_ref_u, byte_ref, 2);
crate::uniform_value!(byte_sub_u, byte_sub, 3);
crate::uniform_value!(append_u, append, 2);
crate::uniform_value!(to_sym_u, to_sym, 1);
crate::uniform_value!(sym_to_str_u, sym_to_str, 1);
crate::uniform_value!(bytes_to_str_u, bytes_to_str, 1);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(None, Some(("str", "make")), 1, make_u),
    PrimDef::row(Some("str-byte-len"), Some(("str", "byte-len")), 1, byte_len_u),
    PrimDef::row(Some("str-byte-ref"), Some(("str", "byte-ref")), 2, byte_ref_u),
    PrimDef::row(Some("str-byte-sub"), Some(("str", "byte-sub")), 3, byte_sub_u),
    PrimDef::row(Some("str-append"), Some(("str", "append")), 2, append_u),
    PrimDef::row(None, Some(("str", "->sym")), 1, to_sym_u),
    PrimDef::row(Some("symbol->str"), Some(("sym", "->str")), 1, sym_to_str_u),
    PrimDef::row(Some("bytes->str"), Some(("bytes", "->str")), 1, bytes_to_str_u),
];

#[cfg(test)]
mod tests {
    /// The coordinates these cases reach for.
    const COORDS: &[(&str, &str, &str)] = &[
        ("%mk", "str", "make"),
        ("%len", "str", "byte-len"),
        ("%ref", "str", "byte-ref"),
        ("%sub", "str", "byte-sub"),
        ("%app", "str", "append"),
        ("%s2y", "str", "->sym"),
        ("%y2s", "sym", "->str"),
        ("%b2s", "bytes", "->str"),
        ("%c2i", "char", "->int"),
    ];

    fn src(body: &str) -> String {
        with_coords(COORDS, body)
    }

    use crate::testkit::{int_of, raises, text_of, truthy, with_coords};

    #[test]
    fn byte_len_counts_bytes() {
        assert_eq!(int_of(&src(r#"(%len "abc")"#)), 3);
        assert_eq!(int_of(&src(r#"(%len "")"#)), 0);
    }

    /// BYTE-SUB ADDRESSES BYTES; IT DOES NOT SLICE THE NUL-BOUNDED VALUE.
    ///
    /// A buffer handed to a syscall is binary: `(str make 4096)` filled by
    /// `getdirentries64` has a NUL in its fifth byte and real data for ninety
    /// more. Slicing the value stopped there, so x-lang's dirent decoder read
    /// every entry NAME as empty, `File list-dir` answered a list of empty
    /// strings, and pin's tree walk joined one onto its path and recursed into
    /// the same directory until the allocation ceiling — 138 s to walk five
    /// files. `byte-ref` beside this always addressed raw bytes; they must agree.
    #[test]
    fn byte_sub_reads_past_an_embedded_nul() {
        // "A\0B": the NUL is the value's end, the B is still addressable.
        assert_eq!(
            text_of(&src(r#"(%sub (%b2s (pair 65 (pair 0 (pair 66 ())))) 2 1)"#)),
            "B"
        );
        // And the value really is NUL-bounded, so this is not a slice.
        assert_eq!(
            int_of(&src(r#"(%len (%b2s (pair 65 (pair 0 (pair 66 ())))))"#)),
            1
        );
    }

    /// `str make` must be NUL-terminated at n, or byte-len reads past the region.
    #[test]
    fn make_gives_a_region_of_the_requested_length() {
        assert_eq!(int_of(&src("(%len (%mk 32))")), 32);
    }

    #[test]
    fn byte_ref_answers_a_char() {
        assert_eq!(int_of(&src(r#"(%c2i (%ref "abc" 0))"#)), 97);
        assert_eq!(int_of(&src(r#"(%c2i (%ref "abc" 2))"#)), 99);
    }

    #[test]
    fn append_concatenates() {
        assert_eq!(int_of(&src(r#"(%len (%app "ab" "cd"))"#)), 4);
        assert_eq!(int_of(&src(r#"(%c2i (%ref (%app "ab" "cd") 2))"#)), 99);
    }

    /// The third argument is a LENGTH. Reading it as an end index is the exact
    /// bug that shipped in x-lang's struct codec, so it is pinned here in the
    /// form that distinguishes the two: off 1, len 2 of "abcd" is "bc", which an
    /// end-index reading would make one byte long.
    #[test]
    fn byte_subs_third_argument_is_a_length() {
        assert_eq!(int_of(&src(r#"(%len (%sub "abcd" 1 2))"#)), 2);
        assert_eq!(int_of(&src(r#"(%c2i (%ref (%sub "abcd" 1 2) 0))"#)), 98);
        assert_eq!(int_of(&src(r#"(%c2i (%ref (%sub "abcd" 1 2) 1))"#)), 99);
    }

    /// Interning is what makes this eq? to the literal. A fresh symbol object
    /// would compare false and nothing else would notice.
    #[test]
    fn str_to_sym_interns() {
        assert!(truthy(&src(r#"(eq? (%s2y "alpha") (lit alpha))"#)));
    }

    #[test]
    fn sym_to_str_recovers_the_name() {
        assert_eq!(text_of(&src("(%y2s (lit alpha))")), "alpha");
    }

    #[test]
    fn bytes_to_str_builds_from_a_list() {
        assert_eq!(int_of(&src("(%len (%b2s (pair 97 (pair 98 ()))))")), 2);
        assert_eq!(
            int_of(&src("(%c2i (%ref (%b2s (pair 97 (pair 98 ()))) 1))")),
            98
        );
    }

    /// A non-string operand is READ, not refused. Its data word is taken as the
    /// address of some bytes and walked to a NUL — meaningless, and exactly what
    /// x-lang's contract means by unchecked. Deciding it is "not a string" would
    /// be a type judgement made one layer too low.
    #[test]
    fn a_non_string_operand_is_read_not_refused() {
        assert!(!raises(&src("(%len 5)")));
        assert!(!raises(&src("(%app 1 2)")));
    }
}
