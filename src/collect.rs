//! Mark and sweep.
//!
//! # What the contract allows
//!
//! Two guarantees this engine declares shape the whole design.
//! `gc/explicit-only` means a collection happens when x-lang asks and at no
//! other time, so nothing has to survive a collection it did not expect.
//! `gc/non-moving` means an object's address never changes, so a sweep can only
//! RECLAIM space, never compact it — a freed object's words go on a free list
//! for an allocation of exactly that size.
//!
//! # Why an object can be found at all
//!
//! The heap is a flat `Vec<u64>` with no object table, so every allocation is
//! threaded onto a chain through header word 0 (see
//! `tools/contract/obj-layout.x`). Sweeping walks that chain.
//!
//! # The part that is conservative, and why
//!
//! Most kinds say exactly which of their words are references: a pair has two, a
//! closure has two and an environment id, a string has a byte offset that is not
//! an object at all. INSTANCES do not. `(obj make TYPE n)` gives x-lang n words
//! it writes what it likes into — `lib/x/type/vector.x` stores a length in slot
//! 0 and elements after it — and the reference engine reads a per-type `units`
//! count and a mark handler to tell them apart.
//!
//! This engine does not have those yet, so an instance's words are traced
//! CONSERVATIVELY: a word is followed only if it could be an object here — in
//! range, correctly aligned, on the chain. That errs the safe way. A live object
//! is never freed; some garbage survives because an integer happened to look
//! like an address. Retaining garbage costs memory, and freeing something live
//! costs correctness, so the choice is not a close one.

use crate::obj::{Flags, Obj, Word};
use crate::objects::{
    Objects, FLAG_BUF, FLAG_FN, FLAG_ITER, FLAG_OP, FLAG_PAIR, FLAG_SPAIR, FLAG_TOKBASE, FLAG_WRAP,
};

/// The mark, kept in a spare bit of the flags word.
///
/// The flags x-lang reads are small — the widest is `0x100000` — and it masks
/// what it wants (`%obj-flag-attr-mask`). The top bit is free, and using it
/// means the mark costs no extra memory and is cleared by the same sweep that
/// reads it.
const MARK: u64 = 1 << 63;

impl Objects {
    fn flags_word(&self, o: Obj) -> u64 {
        self.heap
            .word(o.addr().plus(crate::objects::SLOT_FLAGS * 8))
            .raw()
    }

    fn set_flags_word(&mut self, o: Obj, w: u64) {
        self.heap
            .set_word(o.addr().plus(crate::objects::SLOT_FLAGS * 8), Word(w));
    }

    fn is_marked(&self, o: Obj) -> bool {
        self.flags_word(o) & MARK != 0
    }

    fn set_mark(&mut self, o: Obj) {
        let w = self.flags_word(o);
        self.set_flags_word(o, w | MARK);
    }

    fn clear_mark(&mut self, o: Obj) {
        let w = self.flags_word(o);
        self.set_flags_word(o, w & !MARK);
    }

    /// The next object on the allocation chain.
    fn chain_next(&self, o: Obj) -> Obj {
        self.heap
            .word(o.addr().plus(crate::objects::SLOT_HEAP * 8))
            .as_obj()
    }

    fn set_chain_next(&mut self, o: Obj, next: Obj) {
        self.heap
            .set_word(o.addr().plus(crate::objects::SLOT_HEAP * 8), next.word());
    }

    /// Could this word be an object in this heap?
    ///
    /// Used only for the conservative cases. It is allowed to say yes to a
    /// non-object; it must never say no to a real one.
    fn plausible(&self, w: u64) -> bool {
        // Word-aligned, inside the heap, and with room for a header.
        w != 0
            && w % 8 == 0
            && (w / 8) as usize + crate::objects::META_LEN as usize <= self.heap.words_len()
    }

    /// Mark `o` and everything it reaches.
    fn mark(&mut self, root: Obj) {
        let mut stack = vec![root];
        while let Some(o) = stack.pop() {
            if o.is_nil() || !self.plausible(o.word().raw()) || self.is_marked(o) {
                continue;
            }
            self.set_mark(o);

            // The type word is a reference like any other.
            let ty = self.type_of_word(o);
            if !ty.is_nil() {
                stack.push(ty);
            }

            let flags = Flags::from_word(Word(self.flags_word(o) & !MARK));
            let n = self.data_len(o);
            match flags {
                // Both words are references.
                f if f == FLAG_PAIR || f == FLAG_SPAIR || f == FLAG_ITER => {
                    stack.push(self.data(o, 0).as_obj());
                    stack.push(self.data(o, 1).as_obj());
                }
                // params, body — the third word is an environment id, not an object.
                f if f == FLAG_FN => {
                    stack.push(self.data(o, 0).as_obj());
                    stack.push(self.data(o, 1).as_obj());
                }
                // params, env NAME, body — the fourth is an environment id.
                f if f == FLAG_OP => {
                    stack.push(self.data(o, 0).as_obj());
                    stack.push(self.data(o, 1).as_obj());
                    stack.push(self.data(o, 2).as_obj());
                }
                f if f == FLAG_WRAP || f == FLAG_TOKBASE => {
                    stack.push(self.data(o, 0).as_obj());
                }
                // retain (raw), cursor CELL, text.
                f if f == FLAG_BUF => {
                    stack.push(self.data(o, 1).as_obj());
                    stack.push(self.data(o, 2).as_obj());
                }
                // Everything else: either a raw value (int, char, prim index,
                // foreign address, string byte offset) or an INSTANCE whose words
                // are x-lang's to use. The raw kinds have no references to miss;
                // the instances are traced conservatively.
                _ => {
                    if flags.raw() == 0 {
                        for i in 0..n {
                            let w = self.data(o, i).raw();
                            if self.plausible(w) {
                                stack.push(Word(w).as_obj());
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Objects {
    /// Reclaim every object not reachable from `roots`.
    ///
    /// Answers how many were freed. The chain is rebuilt as it is walked, so a
    /// swept object is off it before its words are handed out again.
    pub fn sweep_from(&mut self, roots: &[Obj]) -> usize {
        for &r in roots {
            self.mark(r);
        }

        let mut freed = 0usize;
        let mut kept = crate::obj::NIL;
        let mut at = self.heap_chain;
        while !at.is_nil() {
            let next = self.chain_next(at);
            if self.is_marked(at) {
                self.clear_mark(at);
                self.set_chain_next(at, kept);
                kept = at;
            } else {
                // NON-MOVING: the words stay where they are and go back on a
                // free list for an allocation of exactly this size. Nothing is
                // compacted, so every address x-lang is holding stays valid.
                let n = self.data_len(at);
                self.free.entry(n).or_default().push(at);
                freed += 1;
            }
            at = next;
        }
        self.heap_chain = kept;
        freed
    }

    /// Take a free object of exactly `n` data words, if one is waiting.
    pub(crate) fn take_free(&mut self, n: usize) -> Option<Obj> {
        self.free.get_mut(&(n as u64)).and_then(|v| v.pop())
    }

    /// How many objects are on the free list.
    pub fn free_count(&self) -> usize {
        self.free.values().map(|v| v.len()).sum()
    }
}

impl crate::engine::Engine {
    /// Everything a collection must start from.
    ///
    /// The awkward ones are the evaluator's own. A form being evaluated lives in
    /// a Rust local and in nothing else — it came from the reader and no object
    /// points at it — so without `roots` a collection during its evaluation
    /// would free the code that is running. `gc/explicit-only` narrows when that
    /// can happen; it does not remove it, because x-lang can call
    /// `(heap collect)` from anywhere.
    fn root_set(&self) -> Vec<Obj> {
        // The engine's own singletons and tables.
        let mut r: Vec<Obj> = vec![
            self.base,
            self.token_eof,
            self.sigint_flag,
            self.catalog,
            self.objects.false_obj(),
            self.objects.spair_marker,
            self.objects.satom_marker,
        ];
        r.extend(self.objects.builtin_types.values().copied());
        r.extend(self.objects.unfiled_types.iter().copied());
        r.extend(self.objects.interned());
        for (name, obj) in &self.prim_bindings {
            r.push(*name);
            r.push(*obj);
        }
        for syms in self.base_syms.values() {
            r.extend(syms.all());
        }

        // The parked tail: a form waiting to be evaluated is as live as one
        // being evaluated.
        if let Some((f, _)) = self.tail {
            r.push(f);
        }

        // The reader's text objects — a buffer views their bytes.
        r.push(self.reader.text_obj_if_made());
        for rd in &self.loading {
            r.push(rd.text_obj_if_made());
        }

        // EVERY environment frame. Frames live outside the heap, so their
        // bindings are references the collector cannot otherwise see. Nothing
        // reclaims frames yet, which makes this sound and not yet thrifty.
        r.extend(self.envs.all_bindings());

        // The evaluator's live values.
        r.extend(self.roots.iter().copied());
        r
    }

    /// `(heap collect)` — reclaim what nothing can reach. Answers the count.
    pub fn collect(&mut self) -> usize {
        let roots = self.root_set();
        self.objects.sweep_from(&roots)
    }

    /// Hold `o` live across anything that might collect.
    pub(crate) fn root_push(&mut self, o: Obj) {
        self.roots.push(o);
    }

    pub(crate) fn root_truncate(&mut self, to: usize) {
        self.roots.truncate(to);
    }

    pub(crate) fn root_mark(&self) -> usize {
        self.roots.len()
    }
}
