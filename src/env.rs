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
    /// The base this frame belongs to. In the reference the env-alist hangs OFF
    /// the base, so "which base does this environment serve" is structural; a
    /// frame here carries it so the same question is answered from data rather
    /// than from engine state. Children inherit it; only `push_root` sets it,
    /// and the base spine is built AFTER its root frame exists, so the builder
    /// stamps it via `set_base`.
    base: crate::obj::Obj,
    /// Reclaimed by the collector and not yet handed out again.
    ///
    /// An `EnvId` is a bare INDEX, so a frame freed while something still named
    /// it would not fail — it would quietly answer the next activation's
    /// bindings, and the wrong answer would surface somewhere else entirely.
    /// This is the object heap's poison trap, in the one form an index allows.
    dead: bool,
}

pub struct Envs {
    frames: Vec<Frame>,
    /// Slots whose frames were reclaimed, ready for the next activation.
    free: Vec<usize>,
    /// Never hand a reclaimed slot back, so a stale `EnvId` stays dead and the
    /// trap fires instead of being papered over by the next activation. The
    /// same switch as the heap's, for the same reason. See `X_GC_POISON`.
    poison: bool,
}

impl Envs {
    /// No frames. The first `push_root` makes one, and for an engine that is
    /// its own base — there is no privileged "global" environment above the
    /// bases, because a base IS the top of a chain.
    pub fn new() -> Self {
        Envs {
            frames: Vec::new(),
            free: Vec::new(),
            poison: std::env::var("X_GC_POISON").is_ok(),
        }
    }

    /// Every name and value in every frame.
    ///
    /// Frames are not in the heap, so the collector cannot trace into them —
    /// their contents are roots.
    /// Push a frame's names and values onto `out`.
    pub fn bindings_of(&self, id: EnvId, out: &mut Vec<Obj>) {
        match &self.frame(id).vars {
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

    /// How many slots the collector has reclaimed and not yet handed back.
    pub fn free_count(&self) -> usize {
        self.free.len()
    }

    pub fn parent_of(&self, id: EnvId) -> Option<EnvId> {
        self.frame(id).parent
    }

    /// Release every frame not marked reachable, answering the count.
    ///
    /// A frame's SLOT is kept — an EnvId is an index — and its bindings are
    /// dropped, which is where the memory is. The slot goes on a free list for
    /// the next activation, which is safe precisely because nothing reachable
    /// still names it.
    pub fn sweep(&mut self, seen: &[bool]) -> usize {
        let poison = self.poison;
        let mut on_free = vec![false; self.frames.len()];
        for &i in &self.free {
            on_free[i] = true;
        }
        let mut freed = 0;
        for (i, frame) in self.frames.iter_mut().enumerate() {
            if seen.get(i).copied().unwrap_or(false) || on_free[i] {
                continue;
            }
            frame.vars = Bindings::Small(Vec::new());
            frame.parent = None;
            frame.dead = true;
            if !poison {
                self.free.push(i);
            }
            freed += 1;
        }
        freed
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    fn frame(&self, id: EnvId) -> &Frame {
        let f = &self.frames[id.index()];
        assert!(
            !f.dead,
            "environment {} used after it was reclaimed",
            id.index()
        );
        f
    }

    fn frame_mut(&mut self, id: EnvId) -> &mut Frame {
        let f = &mut self.frames[id.index()];
        assert!(
            !f.dead,
            "environment {} used after it was reclaimed",
            id.index()
        );
        f
    }

    /// A fresh frame with NO PARENT.
    ///
    /// This is what makes a base a sandbox rather than a second environment: a
    /// rootless frame cannot see outward, so a name defined in the host is
    /// genuinely unbound inside it. `Envs::push` would have made a child that
    /// inherited everything, which is the opposite of the capability model
    /// `base bind` exists to provide.
    pub fn push_root(&mut self) -> EnvId {
        self.alloc(None, crate::obj::NIL)
    }

    /// A fresh frame whose parent is `parent`, serving the parent's base.
    pub fn push(&mut self, parent: EnvId) -> EnvId {
        let base = self.frames[parent.index()].base;
        self.alloc(Some(parent), base)
    }

    /// Stamp a root frame's base, once the spine it serves exists.
    pub fn set_base(&mut self, id: EnvId, base: crate::obj::Obj) {
        self.frames[id.index()].base = base;
    }

    /// The base `env` serves — read from the frame, not walked for.
    pub fn base_of(&self, id: EnvId) -> crate::obj::Obj {
        self.frame(id).base
    }

    /// Take a frame, REUSING a slot the collector reclaimed when there is one.
    ///
    /// An `EnvId` is an index, so reusing a slot would be a disaster if anything
    /// still named it — which is exactly what the sweep establishes it does not.
    /// Without reuse the frame vector grows for the life of the process even
    /// though its contents are being freed.
    fn alloc(&mut self, parent: Option<EnvId>, base: crate::obj::Obj) -> EnvId {
        let frame = Frame {
            vars: Bindings::Small(Vec::new()),
            parent,
            base,
            dead: false,
        };
        match self.free.pop() {
            Some(i) => {
                self.frames[i] = frame;
                EnvId::new(i)
            }
            None => {
                self.frames.push(frame);
                EnvId::new(self.frames.len() - 1)
            }
        }
    }

    /// Bind in THIS frame, shadowing any outer binding of the same name.
    pub fn bind(&mut self, env: EnvId, name: Obj, value: Obj) {
        self.frame_mut(env).vars.set(name, value);
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
            if self.frame_mut(at).vars.replace(name, value) {
                return true;
            }
            match self.frame(at).parent {
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
