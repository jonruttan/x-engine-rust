//! The type registry, and iterators.
//!
//! A type object is a SPINE, not a record: x-lang walks it by name, so it needs
//! one cell per committed route reachable by `rest` steps from the object
//! itself.
//!
//! An `impl Objects` block of its own. The objects is one type because objects
//! share a header and an allocator; it is not one FILE, because these kinds
//! have nothing else to do with each other.

use crate::obj::{Flags, Obj, NIL};
use crate::objects::{Objects, FLAG_FOREIGN, FLAG_ITER, FLAG_PRIM};

/// The families a type object carries, grouped as the reference engine groups
/// them.
///
/// A type is a TREE, not a flat spine, and mirroring the reference's shape is
/// deliberate. Decision L1 leaves the STEPS to the engine — only the NAMES are
/// the contract — so a flat layout would have been permitted. It would also have
/// been a fresh set of decisions about a structure whose real ones are already
/// paid for, and this engine has been wrong about a spine before by inventing
/// one. Same steps as the reference, and the whole class of off-by-one goes away.
///
/// Each family owns a STACK: the slot holds a list, and the active handler is
/// its head. That is why every family has two committed routes —
/// `type-X-stack` addressing the list, and `type-X` one `f` deeper addressing
/// the head. The library pushes and pops by writing the PARENT of the stack
/// route, which is why `%reflect-path-parent` exists.
///
/// The groups, in top-level order: name, data, heap, proc, cvt, io, iter, ops.
pub const HEAP_FAMILIES: &[&str] = &["mark", "make", "free", "clone", "units", "length"];
pub const PROC_FAMILIES: &[&str] = &["call", "eval"];
pub const CVT_FAMILIES: &[&str] = &["from", "to"];
pub const IO_FAMILIES: &[&str] = &["analyse", "delimit", "read", "write", "display"];
pub const ITER_FAMILIES: &[&str] = &["iter"];
pub const OPS_FAMILIES: &[&str] = &["ops"];

/// How many top-level cells a type spine has before the engine's own handlers.
const TYPE_GROUPS: usize = 8;

impl Objects {
    /// A group: `n` cells, each holding a STACK that is empty but PRESENT.
    ///
    /// A family's stack is born as a one-cell list holding nil, never as nil
    /// itself. The two look the same through `type-X` — the active handler is
    /// the head either way, and nil means "none installed" — but they are not
    /// the same to a WRITER, and the library writes here constantly.
    ///
    /// `lib/x/type/struct.x` derives every `*-cell` accessor as the PARENT of
    /// the value route, which for `type-from` is the stack list itself; then
    /// `%set-first!` on it installs a handler. With a nil stack that write goes
    /// through nil, and this engine's first attempt corrupted the heap on
    /// `(%type-set-from! (%type-by-atom %int) …)` — after which an unrelated
    /// `def-class` failed, several hundred lines away, naming a symbol that had
    /// nothing to do with it.
    fn group(&mut self, n: usize) -> Obj {
        let mut spine = NIL;
        for _ in 0..n {
            let stack = self.spair(NIL, NIL);
            spine = self.spair(stack, spine);
        }
        spine
    }

    /// A type object, as a TREE.
    ///
    /// Not a two-word record: x-lang walks a type BY NAME, and there are
    /// forty-odd committed names rooted at the type object itself. Every cell
    /// exists from birth even though almost all of them are nil, because a route
    /// that walks off the end answers nil too — and the library cannot tell "no
    /// handler" from "no such route", so it would take the nil either way.
    ///
    /// `handlers` is an ALIST the library hands over — `((call . fn) (write . fn)
    /// …)` — and each entry is installed as the initial handler of its family,
    /// which is the head of that family's stack.
    ///
    /// Fifteen keys, and they are the reference's: it distributes exactly this
    /// set in `x_prim_type_build_struct`. Ignoring the alist and parking it
    /// somewhere private, as this engine first did, means `(make-type "VECTOR"
    /// ((call . fn) …))` builds a type whose call handler is nowhere — and the
    /// failure surfaces later and elsewhere, when something tries to install
    /// into a stack that was never there.
    pub fn type_new(&mut self, name: Obj, handlers: Obj) -> Obj {
        // Each group is a spine with one cell per family; each family's cell
        // holds its STACK, and a stack starts empty.
        let heap = self.group(HEAP_FAMILIES.len());
        let proc = self.group(PROC_FAMILIES.len());
        let cvt = self.group(CVT_FAMILIES.len());
        let io = self.group(IO_FAMILIES.len());
        let iter = self.group(ITER_FAMILIES.len());
        let ops = self.group(OPS_FAMILIES.len());

        // The name is a STACK too, so that `type-name` — its head — is the name
        // itself. The reference does the same, and reflect.x reads the head.
        let name_stack = self.spair(name, NIL);

        let mut spine = NIL;
        let data = self.spair(NIL, NIL);
        for slot in [ops, iter, io, cvt, proc, heap, data, name_stack]
            .into_iter()
            .take(TYPE_GROUPS)
        {
            spine = self.spair(slot, spine);
        }
        // A type TREE carries the tree tag in its own word, which is how the
        // library tells a real tree from any other word it might find: it probes
        // the tag off the first type-alist entry and checks against it before
        // walking.
        let marker = self.spair_marker;
        self.set_type_word(spine, marker);
        self.install_handlers(spine, handlers);
        spine
    }

    /// Where each handler key lives: (group slot from the type, family index).
    ///
    /// The names are the library's, so they are not this engine's to choose.
    /// `make` and `clone` are absent deliberately — the reference does not let
    /// x-lang set them either.
    fn handler_slot(key: &str) -> Option<(usize, usize)> {
        // Group slots, in the order type_new builds them.
        const DATA: usize = 1;
        const HEAP: usize = 2;
        const PROC: usize = 3;
        const CVT: usize = 4;
        const IO: usize = 5;
        const ITER: usize = 6;
        const OPS: usize = 7;
        let group = |g: usize, fams: &[&str]| fams.iter().position(|f| *f == key).map(|i| (g, i));
        group(HEAP, HEAP_FAMILIES)
            .or_else(|| group(PROC, PROC_FAMILIES))
            .or_else(|| group(CVT, CVT_FAMILIES))
            .or_else(|| group(IO, IO_FAMILIES))
            .or_else(|| group(ITER, ITER_FAMILIES))
            .or_else(|| group(OPS, OPS_FAMILIES))
            .or_else(|| if key == "data" { Some((DATA, 0)) } else { None })
    }

    /// Install each `(key . handler)` as the head of its family's stack.
    ///
    /// An unknown key is SKIPPED rather than refused: which keys exist is
    /// x-lang's vocabulary, and an engine that raised here would be ruling on a
    /// question one layer up.
    fn install_handlers(&mut self, ty: Obj, handlers: Obj) {
        let mut at = handlers;
        while self.is_cell(at) {
            let entry = self.first(at);
            at = self.rest(at);
            if !self.is_cell(entry) {
                continue;
            }
            let key = self.first(entry);
            if !self.is_sym(key) {
                continue;
            }
            let Some((group, family)) = Self::handler_slot(&self.str_val(key)) else {
                continue;
            };
            let handler = self.rest(entry);
            // The group node, then the family's cell, then its stack.
            let mut node = ty;
            for _ in 0..group {
                node = self.rest(node);
            }
            let mut fam = self.first(node);
            for _ in 0..family {
                fam = self.rest(fam);
            }
            let stack = self.first(fam);
            if self.is_cell(stack) {
                self.set_data(stack, 0, handler.word());
            }
        }
    }

    /// A type's installed handler for one family, or nil.
    ///
    /// The ACTIVE handler is the head of the family's stack, which is what the
    /// `type-X` routes address. The reader asks for `analyse` and `read` by the
    /// same door the library uses for `write` and `display` — there is no second
    /// mechanism, and there was one until the handler alist started being
    /// distributed into the tree where it belongs.
    pub fn type_handler(&self, o: Obj, family: &str) -> Obj {
        let Some((group, index)) = Self::handler_slot(family) else {
            return NIL;
        };
        let mut node = o;
        for _ in 0..group {
            node = self.rest(node);
            if node.is_nil() {
                return NIL;
            }
        }
        let mut fam = self.first(node);
        for _ in 0..index {
            fam = self.rest(fam);
            if fam.is_nil() {
                return NIL;
            }
        }
        self.first(self.first(fam))
    }

    /// An instance of a custom type: `n` data words, and a header type word
    /// pointing at the type TREE.
    ///
    /// `t` may arrive as a HANDLE — which is what x-lang passes, since that is
    /// what `type of` and `make-type` answer — so it is resolved here. The word
    /// must hold the TREE: the library dereferences it and checks the tree tag
    /// before walking, and a handle there would fail that check.
    pub fn instance(&mut self, t: Obj, n: usize) -> Obj {
        let tree = self.tree_for(t);
        let o = self.alloc(Flags::new(0), n.max(1));
        self.set_type_word(o, tree);
        o
    }

    /// Resolve a handle to its tree; a tree passes through unchanged.
    pub fn tree_for(&mut self, t: Obj) -> Obj {
        if !self.is_handle(t) {
            return t;
        }
        for &tree in self.builtin_types.values() {
            if self.first(self.first(tree)) == t {
                return tree;
            }
        }
        // A library-made type: the engine keeps no reverse index, and the base's
        // type-alist is the one the library maintains. Callers that own a base
        // resolve through it; here a handle with no builtin behind it stays as it
        // is rather than becoming nil.
        t
    }

    pub fn iter(&mut self, step: Obj, state: Obj) -> Obj {
        let o = self.alloc(FLAG_ITER, 2);
        self.set_data(o, 0, step.word());
        self.set_data(o, 1, state.word());
        o
    }

    pub fn iter_step(&self, o: Obj) -> Obj {
        self.data(o, 0).as_obj()
    }

    pub fn iter_state(&self, o: Obj) -> Obj {
        self.data(o, 1).as_obj()
    }

    /// The type object of a value. A custom instance carries one in its header;
    /// everything else is keyed by flags, so repeated asks answer the same
    /// object — `(same? (type of 1) (type of 2))` must hold.
    /// The TYPE HANDLE of a value — what `type of` answers.
    ///
    /// A handle, not the tree. x-lang's `Type of` is documented as returning
    /// "the type's handle atom", the type-alist is keyed by it, and
    /// `%reflect-satom-tw` is probed off `(type of 0)` to learn what tag a
    /// HANDLE carries. Answering the tree instead made that probe find the
    /// TREE's tag, so handle-tag and tree-tag became the same value and this
    /// engine's own base read as a type handle.
    pub fn type_of(&mut self, o: Obj) -> Obj {
        let tree = self.type_tree_of(o);
        if tree.is_nil() {
            return NIL;
        }
        self.type_handle_of_tree(tree)
    }

    /// The handle stored in a tree's name slot.
    pub fn type_handle_of_tree(&self, tree: Obj) -> Obj {
        if tree.is_nil() {
            return NIL;
        }
        self.first(self.first(tree))
    }

    /// The type TREE of a value: what the engine dispatches on, and what a
    /// value's type word points at.
    pub fn type_tree_of(&mut self, o: Obj) -> Obj {
        if o.is_nil() {
            return NIL;
        }
        let carried = self.type_of_word(o);
        if !carried.is_nil() && carried != self.spair_marker && carried != self.satom_marker {
            return carried;
        }
        let flags = self.reported_flags(o);
        if let Some(&t) = self.builtin_types.get(&flags) {
            return t;
        }
        // The REFERENCE's name for this kind. It is read: with the type word
        // stamped, `%reflect-type-name` dereferences the tree and answers what
        // it finds there.
        let name = self.handle(crate::objects::kind_name(flags));
        let t = self.type_new(name, NIL);
        self.builtin_types.insert(flags, t);
        // A type nobody filed is a type the library cannot reach: `type by-atom`
        // walks the base's alist and answers nil, and its callers write into
        // what they get back rather than checking. Record it here and let the
        // `type of` instruction file it — this layer has no base to file into.
        self.unfiled_types.push(t);
        t
    }

    /// Types created since the last drain, for the caller that owns a base.
    pub fn take_unfiled_types(&mut self) -> Vec<Obj> {
        std::mem::take(&mut self.unfiled_types)
    }

    /// The flags a value's TYPE is keyed by, which are not always the flags it
    /// carries.
    ///
    /// A foreign callable is flagged apart from a primitive INTERNALLY, because
    /// their data words mean different things — a primitive's is an index into
    /// the instruction table, a foreign callable's is a real machine address, and
    /// dispatching on the wrong one segfaults instead of answering wrongly. That
    /// is a representation concern and it stops at the engine's edge.
    ///
    /// x-lang sees one type. `obj make-callable` is the JIT's door: it takes the
    /// address of code just emitted and hands back "something the evaluator will
    /// call", and the conformance case pins exactly that — the result's type is
    /// the type of a primitive. Reporting a second callable type would make the
    /// library's type dispatch miss every compiled function.
    fn reported_flags(&self, o: Obj) -> Flags {
        let f = self.flags(o);
        if f == FLAG_FOREIGN {
            FLAG_PRIM
        } else {
            f
        }
    }

    pub fn set_iter_state(&mut self, o: Obj, s: Obj) {
        self.set_data(o, 1, s.word())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obj::NIL;

    /// The JIT's door: a made callable must be of the SAME type as a primitive,
    /// or the library's dispatch misses every function it compiled.
    #[test]
    fn a_foreign_callable_reports_the_type_of_a_primitive() {
        let mut o = Objects::new();
        let f = o.foreign(0x1000);
        let p = o.prim(0);
        assert_eq!(o.type_of(f), o.type_of(p));
    }

    /// And they stay distinct INSIDE, because the data words mean different
    /// things — an index into the instruction table versus a machine address.
    #[test]
    fn but_they_remain_distinguishable_to_the_engine() {
        let mut o = Objects::new();
        let f = o.foreign(0x1000);
        assert!(o.is_foreign(f));
        assert!(!o.is_prim(f));
    }

    /// EVERY route a type declares must resolve on a type this engine builds.
    ///
    /// The same check as the base's, for the same reason: a spine shorter than
    /// its route list walks off the end and answers nil, which reads as "no
    /// value" rather than "no such route" — and the library would take the nil.
    /// Every committed type route is REACHABLE, walked from this engine's own
    /// base-paths.x rather than from a list retyped here.
    ///
    /// "Reachable" is not "non-nil". `%reflect-step` nil-propagates on purpose —
    /// lib/x/boot/registry.x says why: paths address OPTIONAL slots, and an
    /// empty handler stack is absent rather than broken. The reference engine
    /// would fail a test that demanded a value at every route.
    ///
    /// What must hold is that every STACK route has a parent: the cell whose
    /// `first` is the stack. That is where the library writes to push a handler,
    /// and a spine too short to reach it fails silently — the walk answers nil,
    /// which reads as "no handler" rather than "no such route".
    ///
    /// The VALUE routes are excluded deliberately, and the exclusion is the
    /// point rather than a convenience: `type-X` is one `f` past `type-X-stack`,
    /// so its parent IS the stack, and an empty stack is nil. Demanding a parent
    /// there would demand a handler be installed at birth, which the reference
    /// does not do either.
    #[test]
    fn every_declared_stack_route_has_a_parent_to_write_to() {
        let mut o = Objects::new();
        let name = o.str_new("T");
        let ty = o.type_new(name, NIL);

        let mut checked = 0;
        for (route, steps) in declared_type_routes() {
            if !route.ends_with("-stack") {
                continue;
            }
            // The parent: every step but the last.
            let mut at = ty;
            for (n, step) in steps[..steps.len() - 1].iter().enumerate() {
                assert!(
                    !at.is_nil(),
                    "route `{}` has no parent to write to: nil at step {}",
                    route,
                    n
                );
                at = match step.as_str() {
                    "f" => o.first(at),
                    _ => o.rest(at),
                };
            }
            assert!(!at.is_nil(), "route `{}` has no parent to write to", route);
            checked += 1;
        }
        assert!(checked > 10, "only {} stack routes checked", checked);
    }

    /// A family's stack is PRESENT but empty: the route resolves to a real cell
    /// whose head is nil.
    ///
    /// The distinction has no effect on reading — `type-X` answers nil either
    /// way, meaning no handler — and decides everything about WRITING. Every
    /// `*-cell` accessor in lib/x/type/struct.x is the parent of a value route,
    /// which IS the stack, and the library installs handlers with `%set-first!`
    /// on it. A nil stack sends that write through nil.
    #[test]
    fn every_family_stack_is_present_though_empty() {
        let mut o = Objects::new();
        let name = o.str_new("T");
        let ty = o.type_new(name, NIL);

        let mut checked = 0;
        for (route, steps) in declared_type_routes() {
            if !route.ends_with("-stack") {
                continue;
            }
            let mut at = ty;
            for step in &steps {
                assert!(!at.is_nil(), "route `{}` walks off the end", route);
                at = if step == "f" { o.first(at) } else { o.rest(at) };
            }
            assert!(
                !at.is_nil(),
                "stack `{}` is nil; a handler install would write through it",
                route
            );
            checked += 1;
        }
        assert!(checked > 10, "only {} stacks checked", checked);
    }

    /// The name is readable straight away: reflect.x reads `type-name` as the
    /// head of the name stack, so the stack cannot start empty.
    #[test]
    fn the_name_is_the_head_of_the_name_stack() {
        let mut o = Objects::new();
        let name = o.str_new("T");
        let ty = o.type_new(name, NIL);
        for (route, steps) in declared_type_routes() {
            if route != "type-name" {
                continue;
            }
            let mut at = ty;
            for step in &steps {
                at = if step == "f" { o.first(at) } else { o.rest(at) };
            }
            assert_eq!(at, name, "type-name must resolve to the name itself");
            return;
        }
        panic!("no type-name route declared");
    }

    /// The type routes this engine commits to, parsed from its own contract.
    fn declared_type_routes() -> Vec<(String, Vec<String>)> {
        let paths = std::fs::read_to_string("tools/contract/base-paths.x")
            .expect("the engine's own committed paths");
        let mut out = Vec::new();
        for line in paths.lines() {
            let Some(body) = line.trim().strip_prefix('(') else {
                continue;
            };
            let body = body.split(';').next().unwrap_or("");
            let body = body.trim().trim_end_matches(')').trim();
            let mut words = body.split_whitespace();
            let (Some(route), Some("type")) = (words.next(), words.next()) else {
                continue;
            };
            out.push((
                route.to_string(),
                words.map(str::to_string).collect::<Vec<_>>(),
            ));
        }
        out
    }
}
