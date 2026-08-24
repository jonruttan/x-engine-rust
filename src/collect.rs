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
//! range and correctly aligned.
//!
//! That is a NARROW licence, and widening it was tried and reverted. Tracing
//! every kind's words this way looks safer — "never free a live object" — but it
//! is not: a word that passes the plausibility test may point into the MIDDLE of
//! an object, and marking it writes the mark bit over whatever lives there. The
//! failure mode is not retained garbage, it is a corrupted heap. Conservatism is
//! only safe where the alternative is not knowing, which is instances; for every
//! other kind the layout is known and precision is both cheaper and correct.

use crate::obj::{EnvId, Flags, Obj, Word};
use crate::objects::{
    Objects, FLAG_BUF, FLAG_ENV, FLAG_FALSE, FLAG_FN, FLAG_ITER, FLAG_OP, FLAG_PAIR, FLAG_SPAIR,
    FLAG_TOKBASE, FLAG_WRAP,
};

/// The mark, kept in a spare bit of the flags word.
///
/// The flags x-lang reads are small — the widest is `0x100000` — and it masks
/// what it wants (`%obj-flag-attr-mask`). The top bit is free, and using it
/// means the mark costs no extra memory and is cleared by the same sweep that
/// reads it.
const MARK: u64 = 1 << 63;

/// Written over a swept object's flags when poisoning is on. No real flags word
/// has this value, so reading one is proof of a use-after-free.
///
/// This is how the missing roots were found rather than argued about: with
/// reuse disabled, a freed object that something still holds keeps the poison,
/// and the next read of it traps with a backtrace naming the exact primitive.
pub const POISON: u64 = 0x0BAD_0BAD_0BAD_0BAD;

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
    /// Mark everything reachable from `stack`, reporting the ENVIRONMENTS found.
    ///
    /// Objects and environments reach each other — a closure holds a frame, a
    /// frame's bindings hold objects — so neither can be traced alone. This half
    /// walks objects and hands back the frames it met; `Engine::collect` runs the
    /// two to a fixpoint.
    fn mark(&mut self, stack: &mut Vec<Obj>, envs: &mut Vec<EnvId>) {
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
                // params and body are objects; the third word is the captured
                // ENVIRONMENT, which keeps that frame and its parents alive.
                f if f == FLAG_FN => {
                    stack.push(self.data(o, 0).as_obj());
                    stack.push(self.data(o, 1).as_obj());
                    envs.push(self.closure_env(o));
                }
                // params, env NAME, body — the fourth is an environment id.
                f if f == FLAG_OP => {
                    stack.push(self.data(o, 0).as_obj());
                    stack.push(self.data(o, 1).as_obj());
                    stack.push(self.data(o, 2).as_obj());
                    envs.push(self.op_env(o));
                }
                f if f == FLAG_WRAP || f == FLAG_TOKBASE => {
                    stack.push(self.data(o, 0).as_obj());
                }
                // An environment OBJECT names a frame and nothing else.
                f if f == FLAG_ENV => {
                    envs.push(self.env_id(o));
                }
                // THE FALSE SINGLETON IS SCRATCH SPACE. It looks like a value
                // with nothing in it, and x-lang hangs the include list off its
                // REST — lib/x/boot/module.x does that at boot and reads it for
                // the rest of the run.
                f if f == FLAG_FALSE => {
                    for i in 0..n {
                        stack.push(self.data(o, i).as_obj());
                    }
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
    pub fn sweep(&mut self) -> usize {
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
                if self.poison_freed {
                    let was = self.flags_word(at) & !MARK;
                    // Keep the first two data words as well: when the trap
                    // fires, WHAT the lost object held identifies it far faster
                    // than its address does.
                    let d0 = if n > 0 { self.data(at, 0).raw() } else { 0 };
                    let d1 = if n > 1 { self.data(at, 1).raw() } else { 0 };
                    self.freed_kind.insert(at, (was, d0, d1));
                    self.set_flags_word(at, POISON);
                }
                self.free.entry(n).or_default().push(at);
                freed += 1;
            }
            at = next;
        }
        self.heap_chain = kept;
        self.live -= freed;
        freed
    }

    /// Take a free object of exactly `n` data words, if one is waiting.
    pub(crate) fn take_free(&mut self, n: usize) -> Option<Obj> {
        // While poisoning, nothing is handed back: reuse overwrites the poison,
        // which is exactly what hides the bug being hunted.
        if self.poison_freed {
            return None;
        }
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

        // The evaluator's live values.
        r.extend(self.base_stack.iter().copied());
        r.extend(self.roots.iter().copied());

        // The library's OWN roots — `heap mark-root!`. The reference walks this
        // list as a third mark pass, beside the base tree and the root chain,
        // because its tree walk descends only spair-typed pairs and the list is
        // built from ordinary ones. This engine traces both kinds, so the list
        // was reached incidentally through the base; naming it here makes the
        // guarantee the instruction promises independent of that accident.
        let roots_list = crate::base::get(&self.objects, self.base, crate::base::MARK_ROOTS);
        r.extend(self.objects.list(roots_list));
        r
    }

    /// The environments the engine is holding directly.
    ///
    /// A frame the evaluator is running in is named by a Rust local and nothing
    /// else — the same problem the object roots have, with the same answer.
    fn env_root_set(&self) -> Vec<EnvId> {
        let mut e: Vec<EnvId> = vec![crate::base::env_of(&self.objects, self.base)];
        if let Some((_, env)) = self.tail {
            e.push(env);
        }
        e.extend(self.env_roots.iter().copied());
        e
    }

    /// `(heap collect)` — reclaim what nothing can reach. Answers the count.
    ///
    /// Objects and environments are traced TOGETHER, to a fixpoint. Neither can
    /// be done first: a closure keeps a frame alive, and a frame's bindings keep
    /// objects alive, so tracing one and then the other would miss whatever the
    /// second turned up for the first.
    ///
    /// Treating every frame as a root was the earlier, sound-but-thriftless
    /// answer, and it cost most of the collection: 364,717 frames survive a boot
    /// and each pinned everything it had ever bound, so only 46% of the heap
    /// could be reclaimed.
    pub fn collect(&mut self) -> usize {
        // HOOKS FIRST, BEFORE ANY MARKING. Not a detail — the reference paid for
        // this with a use-after-free. Everything a hook allocates is born
        // unmarked, so if the hooks ran after the mark passes, an allocation
        // that ESCAPED into reachable state (a `heap mark-root!` spine cell, an
        // int a hook stored through `set!`) would be freed by this same sweep,
        // leaving a reachable dangling pointer for the NEXT collection's mark
        // walk to follow. Usually silent, because the freed chunk is typically
        // recycled and the walk just traverses a reinterpreted live object.
        // Running them here means the later passes mark whatever escaped, while
        // transient hook garbage is still swept — and a root registered from
        // inside a hook counts in THIS cycle. See x_heap_mark_phase.
        //
        // Re-entrancy is the price: a hook evaluates x-lang, which allocates and
        // can trip the stress counter. `in_gc` makes the nested call collect
        // without re-running hooks rather than recursing on them.
        if !self.in_gc {
            self.in_gc = true;
            self.run_gc_hooks(crate::base::MARK_HOOKS);
        }

        let mut ostack = self.root_set();
        let mut estack = self.env_root_set();
        let mut seen = vec![false; self.envs.frame_count()];

        loop {
            self.objects.mark(&mut ostack, &mut estack);
            let Some(id) = estack.pop() else { break };
            if seen.get(id.index()).copied().unwrap_or(true) {
                continue;
            }
            seen[id.index()] = true;
            self.envs.bindings_of(id, &mut ostack);
            if let Some(p) = self.envs.parent_of(id) {
                estack.push(p);
            }
        }

        // Between mark and sweep, as the reference does it.
        self.run_gc_hooks(crate::base::FREE_HOOKS);

        let freed_frames = self.envs.sweep(&seen);
        let freed = self.objects.sweep();
        let _ = freed_frames;
        self.in_gc = false;
        freed
    }

    /// Invoke every callable on one of the base's hook lists, with NO arguments.
    ///
    /// The reference builds a one-cell call form per hook — `(hook)` — and runs
    /// it through the trampoline, so a `fn` sees only its self parameter. These
    /// lists are the library's, and an engine that merely COLLECTED registrations
    /// without ever calling them would satisfy every spec that asks whether a
    /// hook "survives a collect" while doing nothing at all. This engine did
    /// exactly that, and the gc-hooks spec passed 13/13 throughout.
    fn run_gc_hooks(&mut self, slot: usize) {
        let list = crate::base::get(&self.objects, self.base, slot);
        if list.is_nil() {
            return;
        }
        let hooks: Vec<Obj> = self.objects.list(list).collect();
        let env = self.root_env();
        for h in hooks {
            // A raising hook must not abort the collection.
            // A raising hook must not abort the collection.
            let _ = self.call_with_values(h, &[], env);
        }
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

    /// Hold a FRAME live across anything that might collect.
    pub(crate) fn env_root_push(&mut self, e: EnvId) {
        self.env_roots.push(e);
    }

    pub(crate) fn env_root_truncate(&mut self, to: usize) {
        self.env_roots.truncate(to);
    }

    pub(crate) fn env_root_mark(&self) -> usize {
        self.env_roots.len()
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;

    /// A call's frame is gone once the call returns.
    ///
    /// This is the whole point of reclaiming frames: an activation that captured
    /// nothing has no reason to outlive the call, and treating every frame as a
    /// root — the earlier, sound-but-thriftless answer — left 46% of the heap
    /// unreclaimable because each dead frame pinned everything it had bound.
    #[test]
    fn a_returned_call_leaves_no_frame_behind() {
        let mut e = Engine::new();
        e.eval_str("(def f (fn (self n) (+ n 1)))").unwrap();
        e.collect();
        let settled = e.envs.frame_count() - e.envs.free_count();

        e.eval_str("(f 1) (f 2) (f 3)").unwrap();
        e.collect();
        assert_eq!(e.envs.frame_count() - e.envs.free_count(), settled);
    }

    /// A CAPTURED frame is not: the closure `g` returns holds the frame that
    /// binds `n`, and reclaiming it would unbind a name that is still in scope.
    #[test]
    fn a_captured_frame_survives() {
        let mut e = Engine::new();
        e.eval_str("(def g (fn (self n) (fn (self2) n)))").unwrap();
        e.eval_str("(def held (g 42))").unwrap();
        e.collect();
        let v = e.eval_str("(held)").unwrap();
        assert_eq!(e.objects.as_int(v), 42);
    }

    /// And so is its PARENT chain, which is what a lookup walks. `outer` is not
    /// named by the closure directly — only by the frame the closure captured.
    #[test]
    fn the_parent_of_a_captured_frame_survives() {
        let mut e = Engine::new();
        e.eval_str("(def h (fn (self outer) ((fn (self2 inner) (fn (self3) outer)) 0)))")
            .unwrap();
        e.eval_str("(def held (h 7))").unwrap();
        e.collect();
        let v = e.eval_str("(held)").unwrap();
        assert_eq!(e.objects.as_int(v), 7);
    }

    /// Reclaimed slots are HANDED BACK. Without reuse the frame vector grows for
    /// the life of the process even while its contents are freed — which is what
    /// the first cut of this did, silently.
    #[test]
    fn reclaimed_slots_are_reused() {
        let mut e = Engine::new();
        e.eval_str("(def f (fn (self n) (+ n 1)))").unwrap();
        e.eval_str("(f 1)").unwrap();
        e.collect();
        let before = e.envs.frame_count();
        // Each round reclaims the last round's frames and takes them back, so a
        // process that calls forever does not grow the frame vector forever.
        for _ in 0..10 {
            e.eval_str("(f 1)").unwrap();
            e.collect();
        }
        assert_eq!(e.envs.frame_count(), before);
    }
}
