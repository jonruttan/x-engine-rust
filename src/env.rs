//! Environments, ON THE HEAP.
//!
//! TRANSCRIBED from the reference (increment D1 of ARCHITECTURE-PORT.md). An
//! environment there is not a struct anywhere — it is a spine of ordinary pair
//! cells, `((sym . val) . rest)`, each spine cell tagged FRAME, shared
//! structurally with its parent (`x_env_extend` conses and never mutates). The
//! collector needs no special knowledge of it: the cells trace like any pair.
//!
//! This file used to hold a Rust `Vec<Frame>` with `EnvId` as an index — and
//! everything that representation forced: a mark/sweep FIXPOINT between objects
//! and frames, a frame free-list, dead-frame traps, X_GC_POISON withholding of
//! slots, and a hand-enumerated root for every frame. All of that machinery
//! existed to compensate for the state living outside the tree. It is gone:
//! frames are heap data now, and collection is plain marking again.
//!
//! The HANDLE. `EnvId` survives as a newtype over the env-holder object so the
//! evaluator's signatures did not all change in the same commit that changed
//! the representation. A holder is three slots: the chain head, the parent
//! holder, and the base the environment serves. `def` conses a FRAME cell and
//! moves the head — the holder mutates, the cells never do, which preserves
//! this engine's existing activation semantics while the cells themselves are
//! the reference's. The holder's remaining distance from the reference — where
//! the current env is BASE STATE under a save/restore protocol and needs no
//! holder at all — is increment D2's to close.
//!
//! THE INDEX. The reference pays for global lookup with a heap BST
//! (`env_global_tree`); until D2 transcribes it, a Rust-side map shadows any
//! holder that outgrows a scan. It is a CACHE over the heap truth — every
//! write goes through `bind`/`set_existing`, so it can never disagree — and it
//! holds nothing the collector needs to see, because everything it points at
//! is reachable through the chain it mirrors.

use crate::obj::{EnvId, Obj, NIL};
use crate::objects::{Objects, FLAG_ENVH};
use std::collections::HashMap;

/// Holder slots.
const CHAIN: u64 = 0;
const PARENT: u64 = 1;
const BASE: u64 = 2;

/// Where a chain stops being scanned and gains a shadow map.
const INDEX_AT: usize = 16;

pub struct Envs {
    /// Shadow maps for large frames, keyed by holder. A pure cache: the chain
    /// is the truth, and every write path updates both.
    index: HashMap<Obj, HashMap<Obj, Obj>>,
    /// How many holders have been made — `frame_count` reporting only.
    made: usize,
}

impl Envs {
    pub fn new() -> Self {
        Envs {
            index: HashMap::new(),
            made: 0,
        }
    }

    /// A fresh root environment: no parent, no base yet (the spine is built
    /// after its env exists, then stamped via `set_base`).
    pub fn push_root(&mut self, o: &mut Objects) -> EnvId {
        self.make(o, NIL, NIL)
    }

    /// A fresh environment under `parent`, serving the parent's base.
    pub fn push(&mut self, o: &mut Objects, parent: EnvId) -> EnvId {
        let base = o.data(parent.obj(), BASE).as_obj();
        self.make(o, parent.obj(), base)
    }

    fn make(&mut self, o: &mut Objects, parent: Obj, base: Obj) -> EnvId {
        let h = o.alloc(FLAG_ENVH, 3);
        o.set_data(h, CHAIN, NIL.word());
        o.set_data(h, PARENT, parent.word());
        o.set_data(h, BASE, base.word());
        self.made += 1;
        EnvId::from_obj(h)
    }

    pub fn set_base(&mut self, o: &mut Objects, id: EnvId, base: Obj) {
        o.set_data(id.obj(), BASE, base.word());
    }

    pub fn base_of(&self, o: &Objects, id: EnvId) -> Obj {
        o.data(id.obj(), BASE).as_obj()
    }

    pub fn parent_of(&self, o: &Objects, id: EnvId) -> Option<EnvId> {
        let p = o.data(id.obj(), PARENT).as_obj();
        if p.is_nil() {
            None
        } else {
            Some(EnvId::from_obj(p))
        }
    }

    /// Bind in THIS environment, shadowing any outer binding of the same name.
    ///
    /// A FRAME cell is consed onto the chain — `((sym . val) . rest)`, the
    /// reference's `x_env_extend` shape — and the holder's head moves. A
    /// REBINDING of a name already in this frame updates the existing cell
    /// instead, which is what keeps a chain from growing with every `def` of
    /// the same name at the REPL.
    pub fn bind(&mut self, o: &mut Objects, env: EnvId, name: Obj, value: Obj) {
        let h = env.obj();
        if let Some(cell) = self.find_in_frame(o, env, name) {
            o.set_data(cell, 1, value.word());
            return;
        }
        let pair = o.spair(name, value);
        let head = o.data(h, CHAIN).as_obj();
        let cell = o.spair(pair, head);
        o.set_data(h, CHAIN, cell.word());
        if let Some(m) = self.index.get_mut(&h) {
            m.insert(name, pair);
        } else if self.chain_len(o, h) > INDEX_AT {
            let mut m = HashMap::new();
            let mut at = o.data(h, CHAIN).as_obj();
            while !at.is_nil() {
                let p = o.first(at);
                m.entry(o.first(p)).or_insert(p);
                at = o.rest(at);
            }
            self.index.insert(h, m);
        }
    }

    fn chain_len(&self, o: &Objects, h: Obj) -> usize {
        let mut n = 0;
        let mut at = o.data(h, CHAIN).as_obj();
        while !at.is_nil() {
            n += 1;
            at = o.rest(at);
        }
        n
    }

    /// The `(sym . val)` cell binding `name` in exactly this frame, if any.
    fn find_in_frame(&self, o: &Objects, env: EnvId, name: Obj) -> Option<Obj> {
        let h = env.obj();
        if let Some(m) = self.index.get(&h) {
            return m.get(&name).copied();
        }
        let mut at = o.data(h, CHAIN).as_obj();
        while !at.is_nil() {
            let p = o.first(at);
            if o.first(p) == name {
                return Some(p);
            }
            at = o.rest(at);
        }
        None
    }

    /// Walk out through the chain. `None` is unbound — an error, not nil.
    pub fn lookup(&self, o: &Objects, env: EnvId, name: Obj) -> Option<Obj> {
        let mut at = env;
        loop {
            if let Some(p) = self.find_in_frame(o, at, name) {
                return Some(o.rest(p));
            }
            at = self.parent_of(o, at)?;
        }
    }

    /// Rebind a name where it is ALREADY bound, answering whether one was found.
    pub fn set_existing(&mut self, o: &mut Objects, env: EnvId, name: Obj, value: Obj) -> bool {
        let mut at = env;
        loop {
            if let Some(p) = self.find_in_frame(o, at, name) {
                o.set_data(p, 1, value.word());
                return true;
            }
            match self.parent_of(o, at) {
                Some(up) => at = up,
                None => return false,
            }
        }
    }

    /// Holders made since the engine started — reporting only. The heap owns
    /// their lifetimes now; there is nothing to sweep here.
    pub fn frame_count(&self) -> usize {
        self.made
    }
}

impl Default for Envs {
    fn default() -> Self {
        Envs::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Objects, Envs) {
        (Objects::new(), Envs::new())
    }

    #[test]
    fn bind_and_lookup_walk_the_chain() {
        let (mut o, mut envs) = setup();
        let root = envs.push_root(&mut o);
        let child = envs.push(&mut o, root);
        let (a, b) = (o.sym("a"), o.sym("b"));
        let (one, two) = (o.int(1), o.int(2));
        envs.bind(&mut o, root, a, one);
        envs.bind(&mut o, child, b, two);
        assert_eq!(envs.lookup(&o, child, a), Some(one));
        assert_eq!(envs.lookup(&o, child, b), Some(two));
        assert_eq!(envs.lookup(&o, root, b), None);
    }

    #[test]
    fn a_child_shadows_and_the_parent_keeps_its_value() {
        let (mut o, mut envs) = setup();
        let root = envs.push_root(&mut o);
        let child = envs.push(&mut o, root);
        let a = o.sym("a");
        let (one, two) = (o.int(1), o.int(2));
        envs.bind(&mut o, root, a, one);
        envs.bind(&mut o, child, a, two);
        assert_eq!(envs.lookup(&o, child, a), Some(two));
        assert_eq!(envs.lookup(&o, root, a), Some(one));
    }

    #[test]
    fn set_existing_reaches_the_outer_frame() {
        let (mut o, mut envs) = setup();
        let root = envs.push_root(&mut o);
        let child = envs.push(&mut o, root);
        let a = o.sym("a");
        let (one, two) = (o.int(1), o.int(2));
        envs.bind(&mut o, root, a, one);
        assert!(envs.set_existing(&mut o, child, a, two));
        assert_eq!(envs.lookup(&o, root, a), Some(two));
    }

    /// Rebinding in the SAME frame updates the cell rather than growing the
    /// chain — a REPL that redefines must not leak a cell per definition.
    #[test]
    fn rebinding_updates_in_place() {
        let (mut o, mut envs) = setup();
        let root = envs.push_root(&mut o);
        let a = o.sym("a");
        for i in 0..40 {
            let v = o.int(i);
            envs.bind(&mut o, root, a, v);
        }
        let head = o.data(root.obj(), 0).as_obj();
        let mut n = 0;
        let mut at = head;
        while !at.is_nil() {
            n += 1;
            at = o.rest(at);
        }
        assert_eq!(n, 1);
    }

    /// The index shadows a grown frame and must agree with the chain.
    #[test]
    fn the_index_is_a_cache_not_a_truth() {
        let (mut o, mut envs) = setup();
        let root = envs.push_root(&mut o);
        let names: Vec<Obj> = (0..40).map(|i| o.sym(&format!("n{}", i))).collect();
        for (i, &n) in names.iter().enumerate() {
            let v = o.int(i as i64);
            envs.bind(&mut o, root, n, v);
        }
        for (i, &n) in names.iter().enumerate() {
            assert_eq!(
                envs.lookup(&o, root, n).map(|v| o.int_val(v)),
                Some(i as i64)
            );
        }
    }
}
