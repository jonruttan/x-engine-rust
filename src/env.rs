//! Environments.
//!
//! An environment is heap data: a HOLDER object of three slots — chain head,
//! parent holder, base — whose chain is a spine of ordinary spair cells,
//! `((sym . val) . rest)`. The collector traces holders and cells like any
//! other objects; no separate lifetime management exists.
//!
//! `EnvId` is a newtype over the holder object, so the evaluator passes
//! environments by handle while the representation stays on the heap.
//!
//! Binding conses a cell and moves the holder's head; rebinding a name already
//! present in the frame updates its cell in place, so a REPL that redefines
//! does not grow the chain. Lookup walks the chain, then the parent.
//!
//! Frames that outgrow a linear scan gain a shadow map, keyed by holder. The
//! map is a cache over the chain — every write path updates both. It holds
//! nothing the collector needs to KEEP: everything it points at is reachable
//! through the chain it mirrors. But it is keyed by holder ADDRESS, and an
//! address outlives its holder — the collector purges dead holders' entries
//! at every sweep, or a recycled chunk would inherit the dead frame's map.

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
    pub(crate) index: HashMap<Obj, HashMap<Obj, Obj>>,
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

    /// Bind in THIS frame, shadowing any outer binding of the same name.
    /// Rebinding a name already present updates its cell in place.
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

    /// Holders made since the engine started — reporting only.
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
