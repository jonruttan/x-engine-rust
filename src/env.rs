//! Environments, as the reference's `x-env.c` makes them.
//!
//! An environment is ONE structural pair, `(bindings . parent)`. The root's
//! bindings are a tree whose nodes are `(entry . (left . right))`, ordered by
//! the name's spelling, and its parent is nil; every other environment the
//! engine makes keeps an alist of `(name . value)` entries and its parent is
//! the environment it was made in. An environment whose spine is NOT a
//! structural pair -- a scope the language made with `pair` -- keeps a tree
//! too, as the root does.
//!
//! The value IS the pair: an operative receives it, `(Fn env f)` reads it out
//! of a closure's state, and `eval` takes it as its second argument, so the
//! library walks environments with `first` and `rest`. `EnvId` is a newtype
//! over that object and nothing more.
//!
//! A name is found by IDENTITY. In the root a name the base did not intern
//! stands for the base's own symbol of the same spelling, so a form read in one
//! base resolves the instruction names of another.

use crate::obj::{EnvId, Obj, NIL};
use crate::objects::Objects;

pub struct Envs {
    /// How many environments have been made -- reporting only.
    made: usize,
}

impl Envs {
    pub fn new() -> Self {
        Envs { made: 0 }
    }

    /// A fresh root environment: an empty tree, no parent.
    pub fn push_root(&mut self, o: &mut Objects) -> EnvId {
        self.make(o, NIL)
    }

    /// A fresh child of `parent`, with no bindings.
    pub fn push(&mut self, o: &mut Objects, parent: EnvId) -> EnvId {
        self.make(o, parent.obj())
    }

    fn make(&mut self, o: &mut Objects, parent: Obj) -> EnvId {
        self.made += 1;
        EnvId::from_obj(o.spair(NIL, parent))
    }

    pub fn parent_of(&self, o: &Objects, id: EnvId) -> Option<EnvId> {
        let p = o.rest(id.obj());
        if p.is_nil() {
            None
        } else {
            Some(EnvId::from_obj(p))
        }
    }

    /// The reference's `x_env_isroot`: a root has no parent.
    pub fn is_root(o: &Objects, e: Obj) -> bool {
        o.rest(e).is_nil()
    }

    /// The reference's `x_env_istree`: a root, or an environment whose spine
    /// the language made rather than the engine.
    pub fn is_tree(o: &Objects, e: Obj) -> bool {
        Self::is_root(o, e) || !o.is_spair(e)
    }

    /// The `(name . value)` entry for `name` in the tree under `node`, or nil.
    /// The reference's `x_alist_bst_lookup`.
    pub fn tree_lookup(o: &Objects, mut node: Obj, name: Obj) -> Obj {
        while !node.is_nil() {
            let entry = o.first(node);
            let key = o.first(entry);
            if key == name {
                return entry;
            }
            let kids = o.rest(node);
            node = if spelling_cmp(o, name, key).is_lt() {
                o.first(kids)
            } else {
                o.rest(kids)
            };
        }
        NIL
    }

    /// Insert `entry` into the tree under `tree`, answering the tree's root.
    /// The reference's `x_alist_bst_insert`: the tree is MUTATED in place so a
    /// closure holding the environment sees a later definition, and a name
    /// already present has its entry replaced.
    pub fn tree_insert(o: &mut Objects, tree: Obj, entry: Obj) -> Obj {
        let name = o.first(entry);
        if tree.is_nil() {
            return tree_node(o, entry);
        }
        let mut at = tree;
        loop {
            if o.first(o.first(at)) == name {
                o.set_data(at, 0, entry.word());
                return tree;
            }
            let kids = o.rest(at);
            let left = spelling_cmp(o, name, o.first(o.first(at))).is_lt();
            let slot = if left { 0 } else { 1 };
            let child = o.data(kids, slot).as_obj();
            if child.is_nil() {
                let n = tree_node(o, entry);
                o.set_data(kids, slot, n.word());
                return tree;
            }
            at = child;
        }
    }

    /// The `(name . value)` entry `name` resolves to from `env` outward, or
    /// nil. The reference's `x_env_lookup`.
    pub fn lookup_entry(&self, o: &Objects, env: EnvId, name: Obj) -> Obj {
        let mut e = env.obj();
        while !e.is_nil() {
            if Self::is_tree(o, e) {
                let entry = Self::tree_lookup(o, o.first(e), name);
                if !entry.is_nil() {
                    return entry;
                }
                if Self::is_root(o, e) {
                    if let Some(own) = o.own_symbol(name) {
                        if own != name {
                            let entry = Self::tree_lookup(o, o.first(e), own);
                            if !entry.is_nil() {
                                return entry;
                            }
                        }
                    }
                }
            } else {
                let mut cell = o.first(e);
                while !cell.is_nil() {
                    let entry = o.first(cell);
                    if o.first(entry) == name {
                        return entry;
                    }
                    cell = o.rest(cell);
                }
            }
            e = o.rest(e);
        }
        NIL
    }

    /// Walk out through the parents. `None` is unbound -- an error, not nil.
    pub fn lookup(&self, o: &Objects, env: EnvId, name: Obj) -> Option<Obj> {
        let entry = self.lookup_entry(o, env, name);
        if entry.is_nil() {
            None
        } else {
            Some(o.rest(entry))
        }
    }

    /// Bind in THIS environment: rebind in place when the name is already
    /// bound here, never touching a parent. The reference's `x_env_bind`.
    pub fn bind(&mut self, o: &mut Objects, env: EnvId, name: Obj, value: Obj) {
        let e = env.obj();
        if Self::is_tree(o, e) {
            let entry = Self::tree_lookup(o, o.first(e), name);
            if !entry.is_nil() {
                o.set_data(entry, 1, value.word());
                return;
            }
            let entry = o.spair(name, value);
            let tree = o.first(e);
            let tree = Self::tree_insert(o, tree, entry);
            o.set_data(e, 0, tree.word());
            return;
        }
        let mut cell = o.first(e);
        while !cell.is_nil() {
            let entry = o.first(cell);
            if o.first(entry) == name {
                o.set_data(entry, 1, value.word());
                return;
            }
            cell = o.rest(cell);
        }
        let entry = o.spair(name, value);
        let head = o.first(e);
        let cell = o.spair(entry, head);
        o.set_data(e, 0, cell.word());
    }

    /// Rebind a name where it is ALREADY bound, answering whether one was found.
    pub fn set_existing(&mut self, o: &mut Objects, env: EnvId, name: Obj, value: Obj) -> bool {
        let entry = self.lookup_entry(o, env, name);
        if entry.is_nil() {
            return false;
        }
        o.set_data(entry, 1, value.word());
        true
    }

    /// Environments made since the engine started -- reporting only.
    pub fn frame_count(&self) -> usize {
        self.made
    }
}

impl Default for Envs {
    fn default() -> Self {
        Envs::new()
    }
}

/// A tree node: the entry, then a kids cell `(left . right)`.
fn tree_node(o: &mut Objects, entry: Obj) -> Obj {
    let kids = o.spair(NIL, NIL);
    o.spair(entry, kids)
}

/// The order of two names by spelling, byte by byte: the reference's
/// `x_lib_strcmp` over the symbols' text.
fn spelling_cmp(o: &Objects, a: Obj, b: Obj) -> std::cmp::Ordering {
    let (mut pa, mut pb) = (o.str_bytes(a), o.str_bytes(b));
    loop {
        let (ca, cb) = (o.heap.byte(pa), o.heap.byte(pb));
        if ca != cb || ca == 0 {
            return ca.cmp(&cb);
        }
        pa = pa.offset(1);
        pb = pb.offset(1);
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

    /// An environment is one pair, (bindings . parent): the library walks it
    /// with first and rest.
    #[test]
    fn an_environment_is_bindings_and_parent() {
        let (mut o, mut envs) = setup();
        let root = envs.push_root(&mut o);
        let child = envs.push(&mut o, root);
        let a = o.sym("a");
        let one = o.int(1);
        envs.bind(&mut o, child, a, one);
        assert_eq!(o.rest(child.obj()), root.obj());
        let entry = o.first(o.first(child.obj()));
        assert_eq!(o.first(entry), a);
        assert_eq!(o.rest(entry), one);
    }

    /// The root keeps a tree, ordered by spelling, and rebinding replaces the
    /// value in place rather than adding a node.
    #[test]
    fn the_root_keeps_a_tree_ordered_by_name() {
        let (mut o, mut envs) = setup();
        let root = envs.push_root(&mut o);
        let (m, a, z) = (o.sym("m"), o.sym("a"), o.sym("z"));
        for (i, s) in [m, a, z].into_iter().enumerate() {
            let v = o.int(i as i64);
            envs.bind(&mut o, root, s, v);
        }
        let tree = o.first(root.obj());
        assert_eq!(o.first(o.first(tree)), m);
        let kids = o.rest(tree);
        assert_eq!(o.first(o.first(o.first(kids))), a);
        assert_eq!(o.first(o.first(o.rest(kids))), z);
        let v = o.int(9);
        envs.bind(&mut o, root, a, v);
        assert_eq!(envs.lookup(&o, root, a), Some(v));
        assert!(o.rest(o.rest(o.first(kids))).is_nil());
    }

    /// A child frame rebinding the same name keeps one entry.
    #[test]
    fn rebinding_updates_in_place() {
        let (mut o, mut envs) = setup();
        let root = envs.push_root(&mut o);
        let child = envs.push(&mut o, root);
        let a = o.sym("a");
        for i in 0..40 {
            let v = o.int(i);
            envs.bind(&mut o, child, a, v);
        }
        let mut n = 0;
        let mut at = o.first(child.obj());
        while !at.is_nil() {
            n += 1;
            at = o.rest(at);
        }
        assert_eq!(n, 1);
    }

    /// A scope the language makes with `pair` keeps a tree, as the root does,
    /// and an engine frame under it still keeps an alist.
    #[test]
    fn a_language_scope_keeps_a_tree() {
        let (mut o, mut envs) = setup();
        let root = envs.push_root(&mut o);
        let scope = EnvId::from_obj(o.pair(NIL, root.obj()));
        let (a, b) = (o.sym("a"), o.sym("b"));
        let (one, two) = (o.int(1), o.int(2));
        envs.bind(&mut o, scope, b, one);
        envs.bind(&mut o, scope, a, two);
        let tree = o.first(scope.obj());
        assert_eq!(o.first(o.first(tree)), b);
        assert_eq!(o.first(o.first(o.first(o.rest(tree)))), a);
        let frame = envs.push(&mut o, scope);
        assert_eq!(envs.lookup(&o, frame, a), Some(two));
        assert_eq!(envs.lookup(&o, root, a), None);
    }
}
