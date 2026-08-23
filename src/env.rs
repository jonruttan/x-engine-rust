//! Environments.
//!
//! A frame chain, held OUTSIDE the objects in a plain Vec, and never freed. Objects
//! live in the objects because x-lang reflects over them at committed offsets; an
//! environment is not one of those things at this stage, so keeping it in Rust
//! costs nothing and buys ordinary borrow checking.
//!
//! A frame is referred to by `EnvId`, which is what lets a closure — which IS an
//! objects object — carry one in a data word without it being confusable with an
//! object reference.
//!
//! x-lang's real env model (GH #47) marks frames so that a redefinition really
//! redefines rather than shadowing. That distinction only becomes observable with
//! nested frames and `set!` interacting in ways this engine cannot yet reach;
//! when it does, this is the file that grows a FRAME mark rather than the file
//! that gets replaced.

use crate::obj::{EnvId, Obj};
use std::collections::HashMap;

/// A frame's bindings.
///
/// SMALL BY DEFAULT, and that is a memory decision rather than a speed one.
/// Booting x-core.x creates 364,504 frames; a `HashMap` apiece cost about 80 MB
/// of the engine's 112 MB, and almost every one of them holds a handful of
/// names — an activation binds its parameters and little else.
///
/// The global frame is the exception: it holds every instruction and every
/// library definition, where a linear scan would be the wrong shape. So a frame
/// starts as a vector and PROMOTES to a map once it outgrows one.
enum Bindings {
    Small(Vec<(Obj, Obj)>),
    Large(HashMap<Obj, Obj>),
}

/// Where a frame stops being a list and becomes a table.
///
/// Above a handful of names a scan starts to cost more than a hash; below it the
/// map's allocation dwarfs the data. Sixteen is comfortably past what any
/// activation binds and far below what the global frame holds.
const PROMOTE_AT: usize = 16;

impl Bindings {
    fn get(&self, name: Obj) -> Option<Obj> {
        match self {
            Bindings::Small(v) => v.iter().find(|(k, _)| *k == name).map(|(_, x)| *x),
            Bindings::Large(m) => m.get(&name).copied(),
        }
    }

    fn set(&mut self, name: Obj, value: Obj) {
        match self {
            Bindings::Small(v) => {
                if let Some(slot) = v.iter_mut().find(|(k, _)| *k == name) {
                    slot.1 = value;
                    return;
                }
                v.push((name, value));
                if v.len() > PROMOTE_AT {
                    let m: HashMap<Obj, Obj> = v.drain(..).collect();
                    *self = Bindings::Large(m);
                }
            }
            Bindings::Large(m) => {
                m.insert(name, value);
            }
        }
    }

    /// Rebind an EXISTING name, answering whether it was there.
    fn replace(&mut self, name: Obj, value: Obj) -> bool {
        match self {
            Bindings::Small(v) => match v.iter_mut().find(|(k, _)| *k == name) {
                Some(slot) => {
                    slot.1 = value;
                    true
                }
                None => false,
            },
            Bindings::Large(m) => match m.get_mut(&name) {
                Some(slot) => {
                    *slot = value;
                    true
                }
                None => false,
            },
        }
    }
}

struct Frame {
    vars: Bindings,
    parent: Option<EnvId>,
}

pub struct Envs {
    frames: Vec<Frame>,
}

impl Envs {
    /// No frames. The first `push_root` makes one, and for an engine that is
    /// its own base — there is no privileged "global" environment above the
    /// bases, because a base IS the top of a chain.
    pub fn new() -> Self {
        Envs { frames: Vec::new() }
    }

    /// Every name and value in every frame.
    ///
    /// Frames are not in the heap, so the collector cannot trace into them —
    /// their contents are roots.
    pub fn all_bindings(&self) -> Vec<Obj> {
        let mut out = Vec::new();
        for f in &self.frames {
            match &f.vars {
                Bindings::Small(v) => {
                    for (k, x) in v {
                        out.push(*k);
                        out.push(*x);
                    }
                }
                Bindings::Large(m) => {
                    for (k, x) in m {
                        out.push(*k);
                        out.push(*x);
                    }
                }
            }
        }
        out
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    fn frame(&self, id: EnvId) -> &Frame {
        &self.frames[id.index()]
    }

    /// A fresh frame with NO PARENT.
    ///
    /// This is what makes a base a sandbox rather than a second environment: a
    /// rootless frame cannot see outward, so a name defined in the host is
    /// genuinely unbound inside it. `Envs::push` would have made a child that
    /// inherited everything, which is the opposite of the capability model
    /// `base bind` exists to provide.
    pub fn push_root(&mut self) -> EnvId {
        self.frames.push(Frame {
            vars: Bindings::Small(Vec::new()),
            parent: None,
        });
        EnvId::new(self.frames.len() - 1)
    }

    /// A fresh frame whose parent is `parent`.
    pub fn push(&mut self, parent: EnvId) -> EnvId {
        self.frames.push(Frame {
            vars: Bindings::Small(Vec::new()),
            parent: Some(parent),
        });
        EnvId::new(self.frames.len() - 1)
    }

    /// Bind in THIS frame, shadowing any outer binding of the same name.
    pub fn bind(&mut self, env: EnvId, name: Obj, value: Obj) {
        self.frames[env.index()].vars.set(name, value);
    }

    /// Walk out through the chain. `None` is unbound, which the caller turns into
    /// a raise — an unbound name is an error, not nil.
    pub fn lookup(&self, env: EnvId, name: Obj) -> Option<Obj> {
        let mut at = env;
        loop {
            if let Some(v) = self.frame(at).vars.get(name) {
                return Some(v);
            }
            at = self.frame(at).parent?;
        }
    }

    /// Rebind a name where it is ALREADY bound, answering whether one was found.
    ///
    /// This is `set!`, and it is a different operation from `bind`: binding in
    /// the current frame would shadow the outer name rather than change it, and a
    /// caller that then read the outer frame would see the old value.
    pub fn set_existing(&mut self, env: EnvId, name: Obj, value: Obj) -> bool {
        let mut at = env;
        loop {
            // One pass: `replace` answers whether the name was there, so a hit
            // never costs a second lookup and a miss never writes.
            if self.frames[at.index()].vars.replace(name, value) {
                return true;
            }
            match self.frames[at.index()].parent {
                Some(p) => at = p,
                None => return false,
            }
        }
    }
}

impl Default for Envs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::Objects;

    /// A root frame, the way an engine starts one.
    fn setup() -> (Objects, Envs, EnvId, Obj) {
        let mut a = Objects::new();
        let mut envs = Envs::new();
        let root = envs.push_root();
        let x = a.sym("x");
        (a, envs, root, x)
    }

    #[test]
    fn a_child_frame_sees_its_parents_bindings() {
        let (mut a, mut envs, root, x) = setup();
        let v = a.int(1);
        envs.bind(root, x, v);
        let child = envs.push(root);
        assert_eq!(envs.lookup(child, x), Some(v));
    }

    #[test]
    fn binding_in_a_child_shadows_rather_than_replaces() {
        let (mut a, mut envs, root, x) = setup();
        let outer = a.int(1);
        let inner = a.int(2);
        envs.bind(root, x, outer);
        let child = envs.push(root);
        envs.bind(child, x, inner);
        assert_eq!(envs.lookup(child, x), Some(inner));
        assert_eq!(
            envs.lookup(root, x),
            Some(outer),
            "the outer binding must be untouched"
        );
    }

    /// The difference between `set!` and `def`: set! reaches the frame the name
    /// actually lives in. Shadowing here would leave the outer value stale and
    /// the caller reading it would never know.
    #[test]
    fn set_existing_reaches_the_owning_frame() {
        let (mut a, mut envs, root, x) = setup();
        let old = a.int(1);
        let new = a.int(2);
        envs.bind(root, x, old);
        let child = envs.push(root);
        assert!(envs.set_existing(child, x, new));
        assert_eq!(envs.lookup(root, x), Some(new));
    }

    #[test]
    fn set_existing_refuses_an_unbound_name() {
        let (mut a, mut envs, root, _) = setup();
        let never = a.sym("never-bound");
        let v = a.int(1);
        assert!(!envs.set_existing(root, never, v));
    }

    /// The sandbox property: a root frame does not see the frame it was made
    /// from. Using `push` here instead would inherit every host binding and the
    /// isolation would be silently absent.
    #[test]
    fn a_root_frame_cannot_see_outward() {
        let (mut a, mut envs, host, x) = setup();
        let v = a.int(1);
        envs.bind(host, x, v);
        let sandbox = envs.push_root();
        assert_eq!(envs.lookup(sandbox, x), None);
        assert_eq!(envs.lookup(host, x), Some(v));
    }

    /// And two roots do not see each other -- what makes `base bind` a
    /// capability handed to ONE base rather than a global name.
    #[test]
    fn two_roots_are_independent() {
        let (mut a, mut envs, _root, x) = setup();
        let v = a.int(1);
        let r1 = envs.push_root();
        let r2 = envs.push_root();
        envs.bind(r1, x, v);
        assert_eq!(envs.lookup(r1, x), Some(v));
        assert_eq!(envs.lookup(r2, x), None);
    }

    #[test]
    fn an_unbound_name_is_absent_not_nil() {
        let (mut a, envs, root, _) = setup();
        let missing = a.sym("missing");
        assert_eq!(envs.lookup(root, missing), None);
    }
}
