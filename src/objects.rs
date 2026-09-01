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
//! Word 0 is the HEAP LINK, threading every live object into one chain so the
//! collector has something to sweep. It is the header's whole cost: x-lang never
//! reads it, and `tools/contract/obj-layout.x` does not name it, because the
//! chain is the engine's business and not the contract's.
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
/// The collector's chain link: every object is threaded here at birth.
pub const SLOT_HEAP: u64 = 0;
pub const SLOT_TYPE: u64 = 1;
pub const SLOT_FLAGS: u64 = 2;
/// How many DATA words follow. The sweep needs it to know where an object ends;
/// the reference packs the same fact into its flags word, which x-lang reads.
pub const SLOT_LEN: u64 = 3;
/// Header words before the data. Committed in `tools/contract/obj-layout.x` as
/// `%obj-meta-len`, which is where the library reads it from.
pub const META_LEN: u64 = 4;

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

/// EXTENDED META marker, 0x80: set on an object whose allocation prepended
/// meta words (source line, file id) BEFORE its header. Committed in
/// tools/contract/obj-layout.x as `%obj-flag-meta`; the library reads the raw
/// flags word and tests this bit, so it lives in STORAGE — `flags()` masks it
/// back out so kind tests stay exact.
pub const FLAG_META_BIT: u64 = 0x80;
/// Where the meta-word COUNT rides in the stored flags word — high byte, out
/// of reach of every kind bit and of the library's `& %obj-flag-meta` test.
/// Sweep keys the free list with it so a chunk is only reused by an
/// allocation wanting exactly the meta room it has.
pub const FLAG_META_SHIFT: u64 = 56;
/// The ALLOCATION MAGIC bit, set in every allocated object's stored flags
/// word and cleared when the sweep frees it. The mark phase refuses to
/// traverse or mark a word run that does not carry it: a conservative
/// reference (an instance slot holding a raw length, an offset that merely
/// looks aligned) must never have mark bits WRITTEN through it — that was
/// live-data corruption, surfacing as garbage digests mid-collect.
pub const FLAG_MAGIC_BIT: u64 = 1 << 62;

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
/// (the SYMBOL type)."
///
/// So a handle carries the atom tag while a symbol points at the SYMBOL type,
/// and the library derives both tags by probing a real handle and a real type.
/// With `type of` answering the TYPE instead, this engine made the two tags
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
/// An ENV HOLDER — chain head, parent holder, base. Its slots are all objects,
/// traced like any other; the FRAME cells on its chain are ordinary spairs.
pub const FLAG_ENVH: Flags = Flags::new(0x1001);

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
/// A buffer's inner bookkeeping pair — `(read . write)`. Its slots are RAW
/// MARKS, not objects, which is why it is its own kind: the tracer must mark
/// the object and refuse to traverse its slots, exactly as the reference's
/// buffer mark handler does ("don't traverse its slots since they're raw char
/// pointers, not objects").
pub const FLAG_BUFMARKS: Flags = Flags::new(0x20001);

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

    /// `#t` — the very object the name `#t` evaluates to, interned in the SHARED
    /// table so a child base cannot mint a second one. See [`Objects::truth`].
    true_obj: Obj,
    /// One type object per built-in shape, so `(type of 1)` and `(type of 2)`
    /// answer the SAME object. Simple values carry no type word, so the
    /// stability x-lang requires comes from here rather than from the header.
    /// The engine's input stream, refilling the interactive source buffer
    /// one byte at a time — the reference's `x_base_read` channel. None for
    /// an engine driven entirely by preloaded text.
    pub(crate) input: Option<Box<dyn std::io::Read>>,
    /// The interactive source region's capacity in bytes.
    pub(crate) input_cap: u64,
    /// The current base — the reference's `p_base`: allocation stamps
    /// resolve through its type-alist. NIL only during registration; the
    /// engine's boot and `in_base` bracket keep it current.
    pub(crate) base: Obj,
    /// The current base's evaluator-state spine nodes — save-stack, tco-expr,
    /// tco-env, error-handler — resolved once per base switch. Spine cells
    /// never move, so the addresses hold while the base is current.
    pub(crate) state_nodes: [Obj; 4],
    /// The LINE and OBJ-META-EXTRA spine nodes of the current base, cached at
    /// base switch: the line is written per evaluated form and the policy read
    /// per allocation, and walking the spine each time was the whole suite's
    /// runtime. The NODES are spine-stable; their inner cells swap (include
    /// pushes a fresh line cell), so the cache holds nodes, not cells.
    pub(crate) loc_nodes: [Obj; 2],
    /// The INT token reader instruction, installed with the analyser states.
    pub(crate) int_read: Obj,
    /// The tag every registered type TYPE carries in its own type word.
    ///
    /// x-lang derives this rather than being told it — `%reflect-spair-tw` is
    /// the type word of the first type-alist entry's type — and then uses it to
    /// check that a word really points at a type before walking one.
    pub(crate) spair_marker: Obj,
    /// The tag every type HANDLE carries, distinct from [`Objects::spair_marker`].
    ///
    /// x-lang probes it off `(type of 0)` and uses it to tell a handle from a
    /// thing that merely has a type. The two must not be equal.
    pub(crate) satom_marker: Obj,
    /// The newest allocation; every object links to the one before it.
    ///
    /// The C engine keeps the same chain in header word 0, and for the same
    /// reason: a flat heap has no other enumeration.
    pub(crate) heap_chain: Obj,
    /// Swept objects, by data length, waiting to be handed out again.
    ///
    /// Keyed by exact size because the collector is NON-MOVING: a reclaimed
    /// object's words stay where they are, so they can only serve an allocation
    /// that fits them exactly.
    pub(crate) free: HashMap<u64, Vec<Obj>>,
    /// Overwrite a swept object's flags, so a later read of it traps. Debug only.
    /// Objects on the heap chain right now, as against `heap count`, which is
    /// every object ever allocated and only ever rises.
    pub(crate) live: usize,
    pub(crate) poison_freed: bool,
    /// What a poisoned object used to be, so the trap can name it.
    pub(crate) freed_kind: HashMap<Obj, (u64, u64, u64)>,
    /// The engine's integer-analyser state objects, in the order
    /// `prims::tok::INT_STATES` declares: sign, prefix, base, digits,
    /// xdigits. A state prim answers the NEXT state by reading it here —
    /// a primitive cannot answer "self" the way a closure's self-binding
    /// can, so the chain goes through the store, as the reference's states
    /// chain through their state slots.
    pub(crate) int_states: [Obj; 5],
    /// ONE handle object per builtin kind, shared by every base's types — the
    /// reference keeps each builtin type's name as a C static atom, so a
    /// child's INTEGER entry is `eq?` to `(type of 0)`'s answer and
    /// apps/logo's alist prune can compare identities. `make-type` names stay
    /// fresh per call, as the reference's strndup'd atoms are.
    pub(crate) kind_handles: [Obj; STAMPED_KINDS.len()],
    /// Handles for kinds outside [`STAMPED_KINDS`] (FALSE, markers): rare,
    /// so a map is fine here — the array serves the per-allocation path.
    pub(crate) other_kind_handles: HashMap<Flags, Obj>,
    /// The engine's eval-handler objects: symbol, list. See prims::core.
    pub(crate) eval_handlers: [Obj; 2],
    /// The shared callable-call handler object, installed on every callable
    /// kind's type. See prims::core.
    pub(crate) callable_call_handler: Obj,
    /// The LIST type's call handler — indexing and slicing.
    pub(crate) list_call_handler: Obj,
    /// The one "BASE" tag atom every base's type word carries.
    pub(crate) base_tag_atom: Obj,
    /// The instruction-table indexes of the four callable ENTRIES —
    /// procedure, operative, wrap, continuation — as the words a
    /// constructor stamps into slot 0. Written once at registration.
    pub(crate) entry_words: [crate::obj::Word; 4],
}

/// The callable kinds and, for each, the instruction-table entry its
/// constructor stamps into slot 0.
pub(crate) const CALL_HANDLER_KINDS: &[(Flags, usize)] = &[
    (FLAG_FN, 0),
    (FLAG_WRAP, 0),
    (FLAG_OP, 1),
    (FLAG_PRIM, 2),
    (FLAG_CONT, 3),
];

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
    // "LIST", as the reference names its pair kind (X_TYPE_LIST_NAME).
    (FLAG_PAIR, "LIST"),
    (FLAG_PTR, "POINTER"),
    (FLAG_PRIM, "PRIMITIVE"),
    (FLAG_FN, "PROCEDURE"),
    (FLAG_OP, "OPERATIVE"),
    (FLAG_WRAP, "PROCEDURE"),
    (FLAG_ENV, "ENVIRONMENT"),
    (FLAG_TYPE, "TYPE"),
    (FLAG_ITER, "ITER"),
    (FLAG_BUF, "BUFFER"),
    (FLAG_CONT, "CONTINUATION"),
];

/// A kind's position in [`STAMPED_KINDS`], or None for one nobody stamps.
pub fn kind_index(flags: Flags) -> Option<usize> {
    STAMPED_KINDS.iter().position(|(f, _)| *f == flags)
}

/// The kind a value REPORTS as, which is not always the one it carries.
///
/// See [`Objects::reported_flags`] — this is the same rule at the point of
/// allocation, where there is no object to ask yet.
pub fn reported_kind(flags: Flags) -> Flags {
    if flags == FLAG_FOREIGN {
        // A dlsym'd address is a POINTER on the reference — `(type of
        // %c-fork)` equals `(type of (%str->ptr "x"))` — its callability
        // is flag dispatch, not type.
        FLAG_PTR
    } else if flags == FLAG_WRAP || flags == FLAG_CONT {
        // The reference builds a wrap AS a procedure (`x_mkwrap` =
        // `x_make_procedure` with the WRAP flag) and a continuation as an
        // ordinary closure (`x_mkproc` in callcc.c), so all three kinds carry
        // the ONE registered PROCEDURE type. Safe to invite the call now:
        // a dead-extent invocation replays the capture's control records
        // instead of escaping uncaught.
        FLAG_FN
    } else {
        flags
    }
}

/// The name a kind reports, or `BUILTIN` for one nobody has named.
///
/// These are the REFERENCE's names, and they are reachable from x-lang rather
/// than decorative: once a value carries a pointer to its type,
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

            true_obj: NIL,
            input: None,
            input_cap: 0,
            base: NIL,
            state_nodes: [NIL; 4],
            loc_nodes: [NIL; 2],
            int_read: NIL,
            spair_marker: NIL,
            satom_marker: NIL,
            heap_chain: NIL,
            free: HashMap::new(),
            live: 0,
            poison_freed: std::env::var("X_GC_POISON").is_ok(),
            freed_kind: HashMap::new(),
            int_states: [crate::obj::NIL; 5],
            kind_handles: [NIL; STAMPED_KINDS.len()],
            other_kind_handles: HashMap::new(),
            eval_handlers: [crate::obj::NIL; 2],
            callable_call_handler: crate::obj::NIL,
            list_call_handler: crate::obj::NIL,
            base_tag_atom: crate::obj::NIL,
            entry_words: [crate::obj::Word(0); 4],
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
        a.true_obj = a.sym_shared(crate::vocabulary::TRUE);
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
        self.live += 1;
        let ty = self.stamp_for(flags);
        let meta = self.meta_extra();

        // A swept object of exactly this size AND meta room is reused before
        // the heap grows. Exactly, because the collector is NON-MOVING:
        // reclaimed words stay where they are and can only serve an
        // allocation that fits them — meta words included, since they live
        // BEFORE the header.
        if let Some(o) = self.take_free(n, meta) {
            self.write_header(o, ty, flags, n);
            let w = self
                .heap
                .word(o.addr().plus(SLOT_FLAGS * WORD as u64))
                .raw();
            let mbits = if meta > 0 {
                FLAG_META_BIT | ((meta as u64) << FLAG_META_SHIFT)
            } else {
                0
            };
            self.heap.set_word(
                o.addr().plus(SLOT_FLAGS * WORD as u64),
                Word(w | FLAG_MAGIC_BIT | mbits),
            );
            for i in 0..n as u64 {
                self.set_data(o, i, NIL.word());
            }
            self.heap_chain = o;
            return o;
        }

        // Meta units are PREPENDED: unit I at word -(I+1) from the object,
        // which is the layout obj-layout.x commits and reflect.x reads.
        for _ in 0..meta {
            self.heap.push(Word(0));
        }
        let at = self.heap.frontier();
        // THREADED ON THE CHAIN at birth. The collector has no other way to find
        // an object: the heap is a flat Vec of words with no object table, so
        // sweeping means walking this link from the newest allocation back.
        self.heap.push(self.heap_chain.word());
        self.heap.push(ty.word());
        let stored = flags.raw()
            | FLAG_MAGIC_BIT
            | if meta > 0 {
                FLAG_META_BIT | ((meta as u64) << FLAG_META_SHIFT)
            } else {
                0
            };
        self.heap.push(Word(stored));
        self.heap.push(Word(n as u64));
        for _ in 0..n {
            self.heap.push(NIL.word());
        }
        let o = at.as_obj();
        self.heap_chain = o;
        o
    }

    /// Whether this object's allocation prepended meta words — the raw
    /// flags word's META bit, which `flags()` masks out of kind tests.
    pub fn meta_stamped(&self, o: Obj) -> bool {
        !o.is_nil()
            && self
                .heap
                .word(o.addr().plus(SLOT_FLAGS * WORD as u64))
                .raw()
                & FLAG_META_BIT
                != 0
    }

    /// Meta unit `i`, at word -(i+1) from the object — obj-layout.x's
    /// committed placement. Zero for an object with none.
    pub fn meta_i(&self, o: Obj, i: u64) -> i64 {
        if !self.meta_stamped(o) {
            return 0;
        }
        let at = o.addr().raw().wrapping_sub((i + 1) * WORD as u64);
        self.heap.word(crate::obj::Addr::new(at)).raw() as i64
    }

    pub fn set_meta_i(&mut self, o: Obj, i: u64, v: i64) {
        if !self.meta_stamped(o) {
            return;
        }
        let at = o.addr().raw().wrapping_sub((i + 1) * WORD as u64);
        self.heap
            .set_word(crate::obj::Addr::new(at), Word(v as u64));
    }

    /// Stamp a freshly read object's source location: line in meta 0, file id
    /// in meta 1 — what the reference's `x_token_read` does per token.
    pub fn stamp_meta(&mut self, o: Obj, line: i64, file: i64) {
        self.set_meta_i(o, 0, line);
        self.set_meta_i(o, 1, file);
    }

    /// How many meta words the current base's policy cell arms, read live —
    /// the library writes the cell directly with `%set-cell-int!`. Capped so a
    /// clobbered cell cannot turn every allocation into a runaway.
    fn meta_extra(&self) -> usize {
        let node = self.loc_nodes[1];
        if node.is_nil() {
            return 0;
        }
        let cell = self.first(node);
        if cell.is_nil() {
            return 0;
        }
        (self.data(cell, 0).raw() as usize).min(16)
    }

    /// Write the live source line through the cached LINE node.
    pub(crate) fn set_live_line(&mut self, line: i64) {
        let node = self.loc_nodes[0];
        if node.is_nil() {
            return;
        }
        let cell = self.first(node);
        if cell.is_nil() {
            return;
        }
        self.set_data(cell, 0, Word(line as u64));
    }

    /// The type word a fresh object of these flags carries. See the type-word
    /// note above.
    /// The shared handle for a builtin kind's name.
    pub(crate) fn kind_handle(&mut self, flags: Flags, text: &str) -> Obj {
        let Some(i) = kind_index(flags) else {
            if let Some(&h) = self.other_kind_handles.get(&flags) {
                return h;
            }
            let h = self.handle(text);
            self.other_kind_handles.insert(flags, h);
            return h;
        };
        let h = self.kind_handles[i];
        if !h.is_nil() {
            return h;
        }
        let h = self.handle(text);
        self.kind_handles[i] = h;
        h
    }

    fn stamp_for(&mut self, flags: Flags) -> Obj {
        if flags == FLAG_SPAIR {
            self.spair_marker
        } else if flags == FLAG_HANDLE {
            self.satom_marker
        } else if self.base.is_nil() {
            // Registration era: the types do not exist yet, and the
            // registration backfill closes the gap.
            NIL
        } else {
            let base = self.base;
            self.builtin_type_in(base, flags)
        }
    }

    /// The base's registered type for a builtin kind — the reference's
    /// `x_type_struct_get`: found in the base's type-alist by the kind's
    /// handle, or built, filed there, and answered. The walk runs per typed
    /// allocation, as the reference's does.
    pub(crate) fn builtin_type_in(&mut self, base: Obj, flags: Flags) -> Obj {
        let key = reported_kind(flags);
        let handle = self.kind_handle(key, kind_name(key));
        let mut at = crate::base::get(self, base, crate::base::TYPE_ALIST);
        while !at.is_nil() {
            let entry = self.first(at);
            if self.first(entry) == handle {
                return self.rest(entry);
            }
            at = self.rest(at);
        }
        let t = self.builtin_type_new(key);
        let entry = self.spair(handle, t);
        let head = crate::base::get(self, base, crate::base::TYPE_ALIST);
        let cell = self.spair(entry, head);
        crate::base::set(self, base, crate::base::TYPE_ALIST, cell);
        t
    }

    /// One builtin kind's type, handlers installed — the reference's
    /// per-kind `x_type_*_struct` builders.
    pub(crate) fn builtin_type_new(&mut self, key: Flags) -> Obj {
        let name = self.kind_handle(key, kind_name(key));
        let t = self.type_new(name, NIL);
        if key == FLAG_INT {
            let sign = self.int_states[crate::prims::tok::ST_SIGN];
            self.type_set_handler(t, crate::vocabulary::Family::Analyse, sign);
            let read = self.int_read;
            self.type_set_handler(t, crate::vocabulary::Family::Read, read);
        }
        if key == FLAG_SYM {
            let h = self.eval_handlers[0];
            self.type_set_handler(t, crate::vocabulary::Family::Eval, h);
        }
        if key == FLAG_PAIR {
            let h = self.eval_handlers[1];
            self.type_set_handler(t, crate::vocabulary::Family::Eval, h);
            let c = self.list_call_handler;
            self.type_set_handler(t, crate::vocabulary::Family::Call, c);
        }
        for (cf, _) in CALL_HANDLER_KINDS {
            if *cf == key {
                let h = self.callable_call_handler;
                self.type_set_handler(t, crate::vocabulary::Family::Call, h);
            }
        }
        t
    }

    /// Write an object's four header words in place, for a reused slot.
    fn write_header(&mut self, o: Obj, ty: Obj, flags: Flags, n: usize) {
        let a = o.addr();
        let w = WORD as u64;
        self.heap
            .set_word(a.plus(SLOT_HEAP * w), self.heap_chain.word());
        self.heap.set_word(a.plus(SLOT_TYPE * w), ty.word());
        self.heap
            .set_word(a.plus(SLOT_FLAGS * w), Word(flags.raw()));
        self.heap.set_word(a.plus(SLOT_LEN * w), Word(n as u64));
    }

    // --- header ----------------------------------------------------------------

    pub fn flags(&self, o: Obj) -> Flags {
        #[cfg(debug_assertions)]
        if self.poison_freed
            && self
                .heap
                .word(o.addr().plus(SLOT_FLAGS * WORD as u64))
                .raw()
                == crate::collect::POISON
        {
            let what = self
                .freed_kind
                .get(&o)
                .map(|(f, a, b)| {
                    format!(
                        "{} first={} rest={}",
                        kind_name(Flags::from_word(Word(*f))),
                        self.describe_word(*a),
                        self.describe_word(*b)
                    )
                })
                .unwrap_or_else(|| "object".to_string());
            panic!(
                "read of a FREED {} at {:?}\n  still held by: {:?}\n  -- a root is missing",
                what,
                o,
                self.holders_of(o)
            );
        }
        Flags::from_word(Word(
            self.heap
                .word(o.addr().plus(SLOT_FLAGS * WORD as u64))
                .raw()
                & !FLAG_META_BIT
                & 0x00ff_ffff_ffff_ffff,
        ))
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
    #[cfg(debug_assertions)]
    fn flags_word_raw(&self, o: Obj) -> u64 {
        self.heap
            .word(
                o.addr()
                    .plus(crate::objects::SLOT_FLAGS * crate::obj::WORD as u64),
            )
            .raw()
    }

    pub fn data(&self, o: Obj, i: u64) -> Word {
        if o.is_nil() {
            return Word(0);
        }
        // Reads trap too under poison, in debug builds: first/rest are plain
        // data reads and touch neither the flags word nor the store path.
        #[cfg(debug_assertions)]
        if self.poison_freed && self.flags_word_raw(o) == crate::collect::POISON {
            let was = self.freed_kind.get(&o);
            panic!(
                "read of a FREED object at {:?} slot {} (was {:?})\n  held by: {:?}",
                o,
                i,
                was,
                self.holders_of(o)
            );
        }
        self.heap.word(Self::slot(o, i))
    }

    pub fn set_data(&mut self, o: Obj, i: u64, v: Word) {
        // TRAP THE STORE, not just the read. A freed object being written INTO a
        // live one means something held it in Rust across a collection — and the
        // backtrace here names that something, where the read-side trap only
        // names whoever stumbled over the result later.
        // Only where the slot really holds a REFERENCE. An environment object's
        // word is a frame id and a primitive's is a table index; a raw number
        // that happens to equal a freed address is not a use-after-free, and
        // trapping on one sends the hunt somewhere there is nothing to find.
        #[cfg(debug_assertions)]
        if self.poison_freed
            && self.freed_kind.contains_key(&v.as_obj())
            && matches!(
                self.flags(o),
                f if f == FLAG_PAIR || f == FLAG_SPAIR
            )
        {
            panic!(
                "storing a FREED object {:?} into slot {} of a live {} -- it was \
                 held across a collection",
                v.as_obj(),
                i,
                kind_name(self.flags(o))
            );
        }
        self.heap.set_word(Self::slot(o, i), v)
    }

    // --- truth -----------------------------------------------------------------

    /// The one `#f` object. Compared by identity, so there is exactly one.
    pub fn false_obj(&self) -> Obj {
        self.false_obj
    }

    /// The one `#t` object, and what `#t` itself evaluates to.
    pub fn true_obj(&self) -> Obj {
        self.true_obj
    }

    /// x-lang's truth answer: the `#t` and `#f` OBJECTS, never a symbol and
    /// never nil. A predicate's answer is a value that gets DISPLAYED, not just
    /// branched on; the reference returns its base's TRUE/FALSE fields
    /// (`x_prim_eq`, x-prim/pred.c), which child bases inherit.
    pub fn truth(&self, b: bool) -> Obj {
        if b {
            self.true_obj
        } else {
            self.false_obj
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
    /// How many data words an object has.
    pub fn data_len(&self, o: Obj) -> u64 {
        self.heap.word(o.addr().plus(SLOT_LEN * WORD as u64)).raw()
    }

    /// Every interned symbol in this base and in the shared table.
    pub fn interned(&self) -> Vec<Obj> {
        let mut out = self.symbols.all();
        out.extend(self.shared_symbols.all());
        out
    }

    /// Who still points at `o`? Walks the live chain looking for a data word
    /// holding it.
    ///
    /// When a whole structure is collected, naming one lost cell says nothing —
    /// the question is which live object was still holding the structure, since
    /// that is the reference the tracer failed to follow.
    #[cfg(debug_assertions)]
    pub(crate) fn holders_of(&self, target: Obj) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = self.heap_chain;
        while !at.is_nil() && out.len() < 4 {
            let n = self.data_len(at);
            for i in 0..n {
                if self.data(at, i).as_obj() == target {
                    out.push(format!("{} slot {}", kind_name(self.flags(at)), i));
                    break;
                }
            }
            at = self
                .heap
                .word(at.addr().plus(SLOT_HEAP * WORD as u64))
                .as_obj();
        }
        out
    }

    /// A one-line description of a raw word, for the collector's trap.
    pub(crate) fn describe_word(&self, w: u64) -> String {
        let o = Word(w).as_obj();
        if o.is_nil() {
            return "()".to_string();
        }
        if w % 8 != 0 || (w / 8) as usize + META_LEN as usize > self.heap.words_len() {
            return format!("raw:{}", w);
        }
        let f = self
            .heap
            .word(o.addr().plus(SLOT_FLAGS * WORD as u64))
            .raw();
        if f == crate::collect::POISON {
            return "<freed>".to_string();
        }
        let flags = Flags::from_word(Word(f));
        if flags == FLAG_SYM || flags == FLAG_STR || flags == FLAG_HANDLE {
            format!("{}:{}", kind_name(flags), self.str_val(o))
        } else if flags == FLAG_INT {
            format!("INT:{}", self.int_val(o))
        } else {
            kind_name(flags).to_string()
        }
    }

    pub fn heap_words(&self) -> usize {
        self.heap.words_len()
    }

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
