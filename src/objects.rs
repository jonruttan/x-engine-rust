//! The object objects.
//!
//! Every object lives in one flat `Vec<u64>`, and an object reference is a BYTE
//! OFFSET into it. That is what lets this engine be written in safe Rust while
//! still meeting decision L1's word-addressable model: x-lang computes byte
//! offsets itself, from the descriptors in `tools/contract/obj-layout.x`, and
//! every offset it computes lands back inside this vector.
//!
//! Nothing outside the engine ever dereferences one of these "pointers", because
//! the `core` profile has no foreign door — no `dlopen`, no `ptr call`. Take that
//! away and this design stops working; it is a consequence of the profile, not a
//! trick.
//!
//! Layout, and it is NOT x-engine-c's (see obj-layout.x for why):
//!
//! ```text
//!   word 0   type      offset of the type object, or NIL
//!   word 1   flags     bitfield; carries the simple-type code
//!   word 2+  data
//! ```
//!
//! There is no heap-link word. This engine has no collector, so there is no chain
//! to thread — which also makes `gc/explicit-only` and `gc/non-moving` true for
//! free rather than by care.
//!
//! STORAGE ANSWERS `Word`, NOT MEANING. `data(o, i)` cannot know whether the slot
//! holds an object, an integer or an address, so it answers a raw `Word` and the
//! typed accessor above it decides. Every accessor here therefore says in its
//! return type what the slot means, which is the whole reason these are separate
//! types rather than all being `u64`.
//!
//! The bytes underneath live in [`Heap`], which knows nothing about objects.
//! This file is only the OBJECT MODEL: headers, constructors, accessors. It is
//! also SHARED — every base allocates into the same heap, because objects must
//! be handed between bases for `base bind` to mean anything.

use crate::heap::Heap;
use crate::obj::{Addr, Flags, Obj, Word, NIL, WORD};
use crate::symbols::Symbols;
use std::collections::HashMap;

// Header shape. These MUST agree with tools/contract/obj-layout.x: x-lang reads
// that file and computes offsets from it, so a disagreement here is not a Rust
// bug that Rust can catch — it is the engine lying about itself.
pub const SLOT_TYPE: u64 = 0;
pub const SLOT_FLAGS: u64 = 1;
pub const META_LEN: u64 = 2;

// Simple-type codes, held in the flags word. Same values as x-engine-c uses: the
// layout is this engine's to choose, but these bits are read by name from
// whichever engine booted, and differing without a reason would prove nothing.
//
// HEX, because the structure is positional and decimal hides it. The low nibble
// of 0x1_ is the simple-type code, and every marker above is a single bit:
//
//   0x0010 .. 0x0015   simple types, sharing the 0x10 marker
//   0x0100  pair        0x0200  symbol      0x0400  #f
//   0x0800  operative   0x1000  environment
//   0x2000  type        0x4000  iterator
//
// tools/contract/obj-layout.x notes the same values in hex beside its decimals,
// which is where the reading came from.
pub const FLAG_PRIM: Flags = Flags::new(0x10);
pub const FLAG_FN: Flags = Flags::new(0x11);
pub const FLAG_INT: Flags = Flags::new(0x12);
pub const FLAG_CHAR: Flags = Flags::new(0x13);
pub const FLAG_STR: Flags = Flags::new(0x14);
pub const FLAG_PTR: Flags = Flags::new(0x15);

/// Not simple-type codes: these have their own marker bits, because the low
/// nibble is reserved for per-type attributes.
pub const FLAG_PAIR: Flags = Flags::new(0x0100);

/// A STRUCTURAL pair: an interpreter spine, not an x-lang list.
///
/// The distinction is x-lang's, not an implementation detail. The reference
/// keeps two pair kinds and the library tells them apart by their TYPE WORD — a
/// list pair points at the pair/list type, a structural one carries the spair
/// tag. The C's ISA states it outright of `base bind`: it "allocates a
/// STRUCTURAL spair for the env spine, which X pair cannot make."
///
/// What rides on it: `pair?` answers #f for a spine, so the library's list
/// walkers do not wander into the interpreter's own structure, and `def-class`
/// can tell its member rows from the frames they live in.
///
/// Same layout as a list pair — `first` and `rest` work on both — so this costs
/// nothing but the tag.
pub const FLAG_SPAIR: Flags = Flags::new(0x0101);

/// A type HANDLE: the atom `type of` answers and the type-alist is keyed by.
///
/// NOT an ordinary symbol, and the difference is one x-lang reads.
/// `lib/x/boot/reflect.x` says it plainly: "The static-ATOM sentinel tag marks
/// type HANDLES (the name atoms `type of` returns) and other raw atoms. It is
/// NOT what #t/#f carry (nil-typed, tag 0) and NOT what interned symbols carry
/// (the SYMBOL type tree)."
///
/// So a handle carries the atom tag while a symbol points at the SYMBOL tree,
/// and the library derives both tags by probing a real handle and a real tree.
/// With `type of` answering the TREE instead, this engine made the two tags
/// identical — and its own base then read as a type handle.
pub const FLAG_HANDLE: Flags = Flags::new(0x0102);
pub const FLAG_SYM: Flags = Flags::new(0x0200);

/// `#f`. x-lang's falsy set is exactly {nil, #f} and that model is SETTLED — it
/// is not to be split, and nothing else is false: zero is true, the empty string
/// is true. One allocated object, compared by identity.
pub const FLAG_FALSE: Flags = Flags::new(0x0400);

/// An OPERATIVE. Separate from a closure because the difference is not a detail
/// of one kind of callable: an operative receives its arguments unevaluated and
/// is handed the caller's environment.
pub const FLAG_OP: Flags = Flags::new(0x0800);

/// A first-class environment. `(op (x) e ...)` binds one to `e`.
pub const FLAG_ENV: Flags = Flags::new(0x1000);

/// A TYPE object: a name and its handler list.
pub const FLAG_TYPE: Flags = Flags::new(0x2000);

/// An ITERATOR: a step function and the state it is currently at.
pub const FLAG_ITER: Flags = Flags::new(0x4000);

/// An APPLICATIVE WRAPPER around an operative. It holds the operative ITSELF,
/// not a copy: `(same? (unwrap (wrap o)) o)` must hold, and the library strips
/// and re-wraps combiners relying on exactly that.
pub const FLAG_WRAP: Flags = Flags::new(0x8000);

/// A FOREIGN CALLABLE: an address dressed as something callable.
///
/// `obj make-callable` turns a raw pointer — in practice a `dlsym` result — into
/// a value that can sit at the head of a form. This engine cannot call one: the
/// `core` profile has no foreign door, so there is nothing to jump to.
///
/// It is a flag of its own rather than a primitive carrying an address, because
/// a primitive's data word is an INDEX into the engine's instruction table and
/// an address would alias a real instruction. Making it a distinct kind means
/// the evaluator sees a callable it does not know how to call, which is the
/// truth, instead of dispatching to whichever primitive shares that number.
pub const FLAG_FOREIGN: Flags = Flags::new(0x80000);

/// A TOKENIZER BUFFER: the tape the analysers run over.
///
/// Its layout is dictated by x-lang, not chosen here. The conformance suite
/// reads the marks reflectively — `(ptr ref-word (obj ->ptr b) doff)` for the
/// retain mark and the same through `(rest b)` for the cursor — so:
///
/// ```text
///   data 0   retain    the token's start, a RAW integer in the word
///   data 1   cursor    an object whose data 0 is the read position
///   data 2   text      the string being read
/// ```
///
/// Word 0 holds the number itself rather than a boxed integer, because the
/// suite reads it as a word. Word 1 must be an OBJECT, because the suite
/// reaches it with `rest` and then reads ITS word 0.
pub const FLAG_BUF: Flags = Flags::new(0x20000);

/// A TOKENIZER BASE: a base with no bindings, carrying registered reader types.
pub const FLAG_TOKBASE: Flags = Flags::new(0x40000);

/// An escape CONTINUATION, identified by a serial number.
///
/// Escape-only: it unwinds outward and cannot be re-entered. x-lang's library
/// never calls call/cc -- only doc-prims.x documents it -- so escape semantics
/// cover everything the language actually does.
pub const FLAG_CONT: Flags = Flags::new(0x10000);

/// The clean end-of-input sentinel, bound to x-lang as `%token-eof`.
///
/// A KIND OF ITS OWN, where the reference engine uses an atom whose value points
/// at itself. Both arrive at the same property by different routes: exactly one
/// such object exists and it is compared by IDENTITY. A kind is the safer route
/// here — an atom that leaked into arithmetic would be read as a number, whereas
/// this is not confusable with anything, and decision L1 leaves the
/// representation to the engine.
pub const FLAG_TOKEOF: Flags = Flags::new(0x100000);

pub struct Objects {
    /// The bytes. Public because reaching for raw storage should SAY that it is
    /// reaching past the object model — `ptr ref-word` and the block operations
    /// genuinely work at that level, and hiding it behind delegating wrappers
    /// would be the object model pretending to own operations it does not.
    pub heap: Heap,
    /// Name to object. Symbol identity is pointer identity in x-lang, so this is
    /// a contract requirement rather than an optimisation.
    ///
    /// `pub(crate)` because the object kinds live in `crate::value`, one file
    /// each, and interning is text's business. Public to the crate, closed to
    /// the world.
    /// Symbols interned by the RUNNING PROGRAM — the reader and `str ->sym`.
    ///
    /// Per-base: `base eval` swaps this for the target base's table, so the same
    /// spelling interned on either side of a base boundary gives two different
    /// objects. That is what makes `base make` an isolation boundary rather than
    /// a second environment, and x-engine-c was asked rather than guessed at.
    pub(crate) symbols: Symbols,
    /// INSTRUCTION NAMES, shared by every base.
    ///
    /// The exception that makes the rest work. `(base eval B (lit (+ 2 3)))`
    /// hands the child the host's `+`, and the child's environment is keyed by
    /// that very object; without one table for instruction names, per-base
    /// interning plus identity lookup would leave every cross-base form unbound
    /// and the sandbox would be unusable rather than isolated.
    ///
    /// Consulted FIRST, so a name registered as an instruction can never be
    /// shadowed by a per-base intern of the same spelling.
    pub(crate) shared_symbols: Symbols,
    /// `#f`. x-lang's falsy set is exactly {nil, #f} and that model is settled.
    false_obj: Obj,
    /// Builtin types made on demand and not yet filed in a base's type-alist.
    /// See [`Objects::take_unfiled_types`].
    pub(crate) unfiled_types: Vec<Obj>,
    /// The symbol `t`. `eq?` answers with it rather than `#t` because nil is a
    /// legitimate value to compare: a predicate answering nil for "equal" could
    /// not say that `(eq? () ())` holds.
    sym_t: Obj,
    /// One type object per built-in shape, so `(type of 1)` and `(type of 2)`
    /// answer the SAME object. Simple values carry no type word, so the
    /// stability x-lang requires comes from here rather than from the header.
    pub(crate) builtin_types: HashMap<Flags, Obj>,
    /// The tag every registered type TREE carries in its own type word.
    ///
    /// x-lang derives this rather than being told it — `%reflect-spair-tw` is
    /// the type word of the first type-alist entry's tree — and then uses it to
    /// check that a word really points at a tree before walking one.
    pub(crate) spair_marker: Obj,
    /// The tag every type HANDLE carries, distinct from [`Objects::spair_marker`].
    ///
    /// x-lang probes it off `(type of 0)` and uses it to tell a handle from a
    /// thing that merely has a type. The two must not be equal.
    pub(crate) satom_marker: Obj,
}

/// The kinds whose type word is stamped at birth.
///
/// Every kind an ordinary value can be. The list is explicit rather than derived
/// because the types must exist BEFORE the objects that carry them, and a kind
/// left out does not fail where it is missing — it fails wherever something
/// tries to print one.
pub const STAMPED_KINDS: &[(Flags, &str)] = &[
    (FLAG_INT, "INTEGER"),
    (FLAG_CHAR, "CHARACTER"),
    (FLAG_STR, "STRING"),
    (FLAG_SYM, "SYMBOL"),
    (FLAG_PAIR, "PAIR"),
    (FLAG_PTR, "POINTER"),
    (FLAG_PRIM, "PRIMITIVE"),
    (FLAG_FN, "PROCEDURE"),
    (FLAG_OP, "OPERATIVE"),
    (FLAG_WRAP, "PROCEDURE"),
    (FLAG_ENV, "ENVIRONMENT"),
    (FLAG_TYPE, "TYPE"),
    (FLAG_ITER, "ITER"),
    (FLAG_BUF, "BUFFER"),
    (FLAG_TOKBASE, "TOKENBASE"),
    (FLAG_CONT, "CONTINUATION"),
    (FLAG_FOREIGN, "PRIMITIVE"),
];

/// The name a kind reports, or `BUILTIN` for one nobody has named.
///
/// These are the REFERENCE's names, and they are reachable from x-lang rather
/// than decorative: once a value carries a pointer to its type tree,
/// `%reflect-type-name` dereferences it and answers what it finds. Naming every
/// builtin type the same thing made every type-name comparison in the library
/// agree, which is worse than answering nothing at all — a name of "BUILTIN"
/// for both INTEGER and SYMBOL is not a missing answer, it is a wrong one.
pub fn kind_name(flags: Flags) -> &'static str {
    STAMPED_KINDS
        .iter()
        .find(|(f, _)| *f == flags)
        .map(|(_, n)| *n)
        .unwrap_or("BUILTIN")
}

impl Objects {
    pub fn new() -> Self {
        let mut a = Objects {
            heap: Heap::new(),
            symbols: Symbols::new(),
            shared_symbols: Symbols::new(),
            false_obj: NIL,
            unfiled_types: Vec::new(),
            sym_t: NIL,
            builtin_types: HashMap::new(),
            spair_marker: NIL,
            satom_marker: NIL,
        };
        // TWO data words, not zero. x-lang's boot uses the false singleton's
        // REST as scratch: lib/x/boot/module.x hangs the include list there with
        // (%set-rest! %false-stack …). With no room for it the write ran off the
        // end of the object and made `#f` itself truthy — the boot then took
        // every wrong branch, silently.
        // The two tags, before anything exists to be tagged. Distinct objects:
        // the library compares against both and behaves differently.
        a.spair_marker = a.alloc(Flags::new(0), 1);
        a.satom_marker = a.alloc(Flags::new(0), 1);

        a.false_obj = a.alloc(FLAG_FALSE, 2);
        a.sym_t = a.sym("t");
        a
    }

    // --- allocation ------------------------------------------------------------

    /// A header plus `n` data words; answers the object's reference.
    ///
    /// Never frees. That is the whole memory manager: the `core` profile has no
    /// isa/gc, so an engine aiming at it is not permitted a collector and does
    /// not need one.
    pub fn alloc(&mut self, flags: Flags, n: usize) -> Obj {
        self.heap.note_allocation();
        let at = self.heap.frontier();
        // THE TYPE WORD. x-lang reads it DIRECTLY — `%reflect-type-word` is a
        // raw word read — and lib/x/boot/printer.x dispatches on what it finds,
        // rendering NOTHING for a word of 0. So a value must point at its type
        // TREE before the library can print it, and `display` stays silent until
        // it does.
        //
        // SPINES and HANDLES are stamped; VALUES are not. That is a halfway
        // house and known to be one.
        //
        // Stamping values was tried per kind: ints, strings, symbols,
        // characters, primitives and closures are all fine. LIST PAIRS alone
        // break `def-class` — its member key comes out nil and every class
        // answers "no such static member".
        //
        // Three explanations were tried and all three were WRONG, which is worth
        // recording so the next attempt does not repeat them:
        //   * spine/list confusion — separating structural pairs did not fix it;
        //   * handle/tree conflation — separating those tags did not either;
        //   * lists becoming iterable — `%reflect-iter-new` answers nil on a
        //     list either way, and `%filter` behaves identically.
        // The cause is still unfound. It is somewhere in how `%flatten-class`
        // builds its member rows, and it is reproducible in two lines: load
        // x-core.x to line 187, define any class, read its static table.
        //
        // A STRUCTURAL pair carries the tree tag; everything else keeps a nil
        // word for now.
        let ty = if flags == FLAG_SPAIR {
            self.spair_marker
        } else if flags == FLAG_HANDLE {
            self.satom_marker
        } else {
            NIL
        };
        self.heap.push(ty.word());
        self.heap.push(Word(flags.raw())); // flags
        for _ in 0..n {
            self.heap.push(NIL.word());
        }
        at.as_obj()
    }

    // --- header ----------------------------------------------------------------

    pub fn flags(&self, o: Obj) -> Flags {
        Flags::from_word(self.heap.word(o.addr().plus(SLOT_FLAGS * WORD as u64)))
    }

    pub fn is(&self, o: Obj, flags: Flags) -> bool {
        !o.is_nil() && self.flags(o) == flags
    }

    /// The header's type word. Nil for anything whose type is implied by its
    /// flags rather than carried.
    pub fn type_of_word(&self, o: Obj) -> Obj {
        if o.is_nil() {
            NIL
        } else {
            self.heap
                .word(o.addr().plus(SLOT_TYPE * WORD as u64))
                .as_obj()
        }
    }

    pub fn set_type_word(&mut self, o: Obj, t: Obj) {
        self.heap
            .set_word(o.addr().plus(SLOT_TYPE * WORD as u64), t.word())
    }

    // --- data slots ------------------------------------------------------------

    /// The address of data slot `i`.
    fn slot(o: Obj, i: u64) -> Addr {
        o.addr().plus((META_LEN + i) * WORD as u64)
    }

    /// A raw slot read. Answers `Word` because storage does not know what the
    /// slot means; the accessors above it say what it means.
    ///
    /// NIL HAS NO SLOTS, so reading one answers zero. That is not a type check —
    /// it is the representation: nil is the absence of an object, and offset 0 is
    /// reserved precisely so nothing lives there. Without this, `data(NIL, 0)`
    /// computes offset 16 and reads whichever object happens to have been
    /// allocated there, so `(+ 1)` would answer a different number depending on
    /// what the engine had constructed at start-up.
    ///
    /// One rule in one place: `first` and `rest` had their own nil guards before
    /// this, which is the same rule written twice.
    pub fn data(&self, o: Obj, i: u64) -> Word {
        if o.is_nil() {
            return Word(0);
        }
        self.heap.word(Self::slot(o, i))
    }

    pub fn set_data(&mut self, o: Obj, i: u64, v: Word) {
        self.heap.set_word(Self::slot(o, i), v)
    }

    // --- truth -----------------------------------------------------------------

    /// The one `#f` object. Compared by identity, so there is exactly one.
    pub fn false_obj(&self) -> Obj {
        self.false_obj
    }

    /// x-lang's truth answer: the symbol `t`, or nil.
    pub fn truth(&self, b: bool) -> Obj {
        if b {
            self.sym_t
        } else {
            NIL
        }
    }

    pub fn is_false(&self, o: Obj) -> bool {
        self.is(o, FLAG_FALSE)
    }

    /// x-lang's truth test, in one place so it cannot drift: everything is true
    /// except nil and `#f`. Zero is TRUE. The empty string is TRUE.
    /// Swap in another base's symbol table, answering the one displaced.
    ///
    /// `base eval` brackets a call with this. Threading the table rather than
    /// the base is what keeps the reader and `str ->sym` base-aware without
    /// every one of them taking a base argument — the C threads `p_base`
    /// everywhere for the same reason.
    /// How many objects have been allocated — what `heap count` answers.
    pub fn alloc_count(&self) -> usize {
        self.heap.allocations()
    }

    pub fn swap_symbols(&mut self, table: Symbols) -> Symbols {
        std::mem::replace(&mut self.symbols, table)
    }

    pub fn truthy(&self, o: Obj) -> bool {
        !o.is_nil() && !self.is_false(o)
    }
}

impl Default for Objects {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing may be allocated at offset 0, or a real object would be
    /// indistinguishable from nil.
    #[test]
    fn no_object_lands_on_nil() {
        let mut a = Objects::new();
        assert!(!a.int(0).is_nil());
        assert!(!a.pair(NIL, NIL).is_nil());
        assert!(!a.str_new("").is_nil());
    }

    /// A slot holds bits; what they mean is the accessor's business. Reading an
    /// integer's slot as an object is nonsense but must not panic — this pins
    /// that storage stays inert.
    /// Nil has no slots. Without this the read lands on whatever object was
    /// allocated at offset 16 during start-up, and machine operations on a
    /// missing operand would answer a number that depends on construction order.
    #[test]
    fn reading_a_slot_of_nil_answers_zero() {
        let mut a = Objects::new();
        let _ = a.int(7);
        assert_eq!(a.data(NIL, 0), Word(0));
        assert_eq!(a.data(NIL, 1), Word(0));
        assert!(a.first(NIL).is_nil());
        assert!(a.rest(NIL).is_nil());
    }

    #[test]
    fn a_data_slot_is_just_a_word() {
        let mut a = Objects::new();
        let n = a.int(-7);
        assert_eq!(a.data(n, 0).as_i64(), -7);
        let _ = a.data(n, 0).as_obj();
    }

    #[test]
    fn interning_makes_one_object_per_spelling() {
        let mut a = Objects::new();
        assert_eq!(a.sym("alpha"), a.sym("alpha"));
        assert_ne!(a.sym("alpha"), a.sym("beta"));
    }

    /// The C-string ruling: a string's length stops at the NUL, whatever follows.
    #[test]
    fn byte_len_stops_at_the_nul() {
        let mut a = Objects::new();
        let s = a.str_new("abc");
        assert_eq!(a.byte_len(s), 3);
        let at = a.str_bytes(s);
        a.heap.set_byte(at.plus(1), 0);
        assert_eq!(a.byte_len(s), 1, "bytes past the NUL are unobservable");
    }

    #[test]
    fn str_make_is_space_filled_and_terminated() {
        let mut a = Objects::new();
        let s = a.str_make(4);
        assert_eq!(a.byte_len(s), 4);
        assert_eq!(a.str_val(s), "    ");
    }

    /// Falsy is exactly {nil, #f}.
    #[test]
    fn only_nil_and_false_are_falsy() {
        let mut a = Objects::new();
        let f = a.false_obj();
        assert!(!a.truthy(NIL));
        assert!(!a.truthy(f));
        let zero = a.int(0);
        assert!(a.truthy(zero), "zero is TRUE in x-lang");
        let empty = a.str_new("");
        assert!(a.truthy(empty), "the empty string is TRUE");
    }
}
