//! The engine: the context, and how one is built.
//!
//! Mirrors the reference engine's `x-prim.c` — the instruction table is walked
//! once here, each row binding its bare name and filing its coordinate from the
//! SAME primitive object, which is what makes the two incapable of disagreeing.
//!
//! Construction lives apart from evaluation for the reason the C separates them:
//! registering instructions and running them are different jobs, and one of them
//! happens once.

use crate::env::Envs;
use crate::obj::{EnvId, Obj, NIL};
use crate::objects::Objects;
use crate::prim::PrimDef;
use crate::read::Reader;
use crate::symbols::Symbols;
use std::collections::HashMap;

pub struct Engine {
    pub objects: Objects,
    pub envs: Envs,
    /// Every registered instruction, indexed by the number a primitive object
    /// carries in its data word.
    /// `pub(crate)` because the evaluator lives next door in `eval.rs`: the
    /// table is built here and read there, and those are different jobs in the
    /// same crate.
    pub(crate) prims: Vec<PrimDef>,
    /// THIS engine's own base — the one `%base` answers and the one top-level
    /// forms evaluate in. Other bases made with `base make` are ordinary values.
    pub base: Obj,
    /// The primitive catalog, shared by every base. The instructions are the
    /// same objects whichever base reaches them; only BINDINGS are per-base.
    pub(crate) catalog: Obj,
    /// Name-to-primitive for every registered instruction, kept so a new base
    /// can be given the instruction set. A fresh base must evaluate `(+ 2 3)`,
    /// so it is born knowing the machine — what it does NOT get is the host's
    /// definitions.
    pub(crate) prim_bindings: Vec<(Obj, Obj)>,
    /// The input stream. The engine owns it because the PROGRAM arrives on it:
    /// what `io read-char` should answer is whatever is left after the form being
    /// evaluated, which a reader living in main could not be asked.
    pub reader: Reader,
    /// Sources being LOADED, innermost last.
    ///
    /// `io read` and `io read-char` must answer from the source currently being
    /// read, not from the process's stdin. x-lang's reader handlers depend on
    /// it: `lib/x/type/vector.x` reads a `#(…)` literal's elements by calling
    /// `(io read)` from inside the reader, and while an `include` is running the
    /// thing being read is the FILE.
    ///
    /// Without this the vector handler reached past the file and ate a form off
    /// stdin — so the first form after `(include "lib/x-core.x")` silently
    /// vanished, and the REPL launcher was the form that vanished.
    pub(crate) loading: Vec<Reader>,
    /// Values the EVALUATOR is holding that nothing else points at.
    ///
    /// A form being evaluated came from the reader and lives in a Rust local; a
    /// collection triggered underneath it would free the code that is running.
    /// See `Engine::root_set`.
    pub(crate) roots: Vec<Obj>,
    /// Frames the EVALUATOR is holding, for the same reason and with the same
    /// discipline: an activation frame is named by a Rust local from the moment
    /// it is pushed until its body starts running.
    pub(crate) env_roots: Vec<EnvId>,
    /// Already collecting: a hook's own evaluation must not recurse on hooks.
    pub(crate) in_gc: bool,
    /// Bases displaced by `in_base`, still live while their children run.
    pub(crate) base_stack: Vec<Obj>,
    /// How many `guard` bodies are on the stack.
    ///
    /// The interrupt flag only becomes a STOP while a handler can catch it —
    /// `x_eval_error` with no handler would take down the whole run, so the
    /// reference tests `x_eval_field_error_handler` before raising. That field
    /// is a base slot there; `guard` is Rust-side here, so the depth is the
    /// same question asked of this stack.
    pub(crate) guard_depth: u32,
    /// Collect every N evaluation steps. Zero — the default — never collects on
    /// its own, which is what `gc/explicit-only` promises.
    pub(crate) gc_stress: u32,
    pub(crate) stress_countdown: u32,
    /// An escape in flight: which continuation is unwinding, and with what.
    ///
    /// A raise carries a value; this says the unwind is an ESCAPE rather than a
    /// condition, which is the difference `guard` must respect. A handler that
    /// caught an escaping continuation would strand it at the wrong depth and
    /// turn a non-local exit into a handled error.
    pub(crate) escaping: Option<(u64, Obj)>,
    pub(crate) next_cont: u64,
    /// Each base's own symbol table, parked while another base is running.
    ///
    /// A base is an interpreter context and interns for itself; this is where
    /// the context that is NOT currently executing keeps its table.
    pub(crate) base_syms: HashMap<Obj, Symbols>,
    /// The allocation ceiling, once armed.
    ///
    /// Checked per FORM rather than per allocation: the guard exists to stop a
    /// runaway before it takes the machine down, and a check in `alloc` would
    /// mean threading a Result through every constructor to catch something
    /// that is not a value error.
    pub(crate) alloc_limit: Option<usize>,
    /// A form parked for the evaluator's own loop — the reference engine's
    /// `tco-expr` and `tco-env`, which is what makes tail calls not grow the
    /// stack.
    pub(crate) tail: Option<(Obj, EnvId)>,
    /// How many evaluations are stacked. See [`Engine::nothing_pending`].
    pub(crate) eval_depth: usize,
    /// The one end-of-input sentinel, bound to every base as `%token-eof`.
    ///
    /// SHARED across bases, like the reference's static — asked and confirmed:
    /// a child base's `%token-eof` is `(obj same?)` to the host's. It has to be.
    /// The REPL compares what `io repl-read` answered against the name it can
    /// see, and if a child's sentinel were a different object the comparison
    /// would silently never match, turning end-of-input into an infinite loop.
    pub(crate) token_eof: Obj,
    /// The object `%sigint-flag` names.
    ///
    /// A signal handler cannot write it — it may touch only async-signal-safe
    /// state — so the handler sets an atomic and the eval loop publishes it
    /// here, between forms. That is soon enough: x-lang's own case reads the
    /// flag in the form AFTER the one that raises.
    pub(crate) sigint_flag: Obj,
}

/// The EXPRESSION LAYER's version, which `x-version` reports.
///
/// Not this crate's version, and the distinction is the reference engine's:
/// x-version is "ext/x-expr's X_VERSION, '0.1.0' and rightly stable", while
/// which release of x-lang an engine belongs to is `x-release`. This engine has
/// no separate expression crate, so the number it reports is the one the layer
/// has always reported rather than a second thing to keep in step.
pub const X_EXPR_VERSION: &str = "0.1.0";

impl Engine {
    pub fn new() -> Self {
        let mut e = Engine {
            objects: Objects::new(),
            envs: Envs::new(),
            prims: Vec::new(),
            base: NIL,
            catalog: NIL,
            prim_bindings: Vec::new(),
            reader: Reader::new(""),
            loading: Vec::new(),
            roots: Vec::new(),
            env_roots: Vec::new(),
            in_gc: false,
            base_stack: Vec::new(),
            guard_depth: 0,
            gc_stress: std::env::var("X_GC_STRESS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            stress_countdown: std::env::var("X_GC_STRESS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            escaping: None,
            next_cont: 1,
            base_syms: HashMap::new(),
            alloc_limit: None,
            tail: None,
            eval_depth: 0,
            token_eof: NIL,
            sigint_flag: NIL,
        };

        // One pass over the whole instruction set. Each row contributes its bare
        // name and its coordinate from THE SAME primitive object, which is what
        // makes a coordinate and its bare binding incapable of disagreeing.
        let mut coords: Vec<(&'static str, &'static str, Obj)> = Vec::new();
        for def in crate::prims::all() {
            let idx = e.prims.len();
            e.prims.push(def);
            let obj = e.objects.prim(idx);
            if let Some(name) = def.bare {
                let sym = e.objects.sym_shared(name);
                e.prim_bindings.push((sym, obj));
            }
            if let Some((ns, m)) = def.coord {
                // Namespace and method names too: the catalog is walked with
                // symbols read in whichever base is asking.
                let _ = e.objects.sym_shared(ns);
                let _ = e.objects.sym_shared(m);
                coords.push((ns, m, obj));
            }
        }
        e.catalog = e.file_catalog(&coords);

        // The `%isa-values` objects, made BEFORE the first base, because every
        // base binds them and the root base is made the same way as any other.
        e.token_eof = e.objects.token_eof();
        e.sigint_flag = e.objects.int(0);

        // The engine's own base is made the same way as any other. It differs
        // only in being the one the read-eval loop uses.
        e.base = e.make_base();
        e.register_builtin_types();
        e
    }

    /// File every builtin type in the base's `type-alist`, keyed by the handle
    /// `type of` answers.
    ///
    /// The library reaches a type's tree ONLY through this table:
    /// `lib/x/boot/printer.x` looks a handle up with
    /// `(%registry-assoc-rest handle (first %reflect-type-alist-cell))` and then
    /// pushes its render handler into the tree it gets back.
    ///
    /// An empty table does not fail politely. The lookup answers nil,
    /// `%reflect-step` nil-propagates as designed, and `%set-first!` then writes
    /// through NIL — so booting printer.x corrupted the heap and `#f` itself came
    /// out truthy, which sent every later branch the wrong way with nothing
    /// raised. The table has to be populated before any library loads.
    ///
    /// Handle and tree are the SAME object here, because `type of` answers the
    /// type object itself; the reference keys by a separate sentinel. The alist
    /// shape is what matters to the library, not which of the two it holds.
    fn register_builtin_types(&mut self) {
        // One representative value per kind, and EVERY kind the library can
        // name — not just the ones it files render handlers for. A missing entry
        // does not surface where it is missing: `lib/x/type/convert.x` looks the
        // PTR type up, gets nil, and writes a conversion alist through it, after
        // which an unrelated `def-class` fails hundreds of lines later naming a
        // symbol that has nothing to do with it.
        let one = self.objects.int(1);
        let samples = [
            self.objects.int(0),
            self.objects.str_new(""),
            // A SYMBOL, and `#t` is one — the sample only has to have the type.
            self.objects.true_obj(),
            self.objects.char_new(65),
            self.objects.false_obj(),
            self.objects.pair(one, NIL),
            self.objects.ptr(crate::obj::Addr::new(0)),
            self.objects.foreign(0),
            self.objects.env_obj(EnvId::new(0)),
        ];
        for v in samples {
            let _ = self.objects.type_tree_of(v);
        }
        // The callables and the reader's own kinds too. A type made only when
        // something first asks for it leaves every object allocated BEFORE that
        // ask carrying a nil type word, and the library reads that word
        // directly — so the ask has to happen here, before any of them exist.
        for (flags, text) in crate::objects::STAMPED_KINDS {
            if !self.objects.builtin_types.contains_key(flags) {
                let name = self.objects.handle(text);
                let t = self.objects.type_new(name, NIL);
                self.objects.builtin_types.insert(*flags, t);
                self.objects.unfiled_types.push(t);
            }
        }
        // Drained here and, from now on, by the `type of` instruction itself.
        for t in self.objects.take_unfiled_types() {
            self.file_type(t);
        }
    }

    /// The TREE a handle names, resolved through the base's type-alist.
    ///
    /// The alist is the library's index and the only complete one: a type
    /// `make-type` built lives there and nowhere else. A value that is already a
    /// tree passes through, so callers need not know which they hold.
    pub(crate) fn resolve_tree(&mut self, t: Obj) -> Obj {
        if !self.objects.is_handle(t) {
            return t;
        }
        let alist = crate::base::get(&self.objects, self.base, crate::base::TYPE_ALIST);
        let mut at = alist;
        while self.objects.is_cell(at) {
            let entry = self.objects.first(at);
            if self.objects.is_cell(entry) && self.objects.first(entry) == t {
                return self.objects.rest(entry);
            }
            at = self.objects.rest(at);
        }
        // Unknown handle: hand it back rather than nil, so a caller storing it
        // keeps what it was given instead of silently losing the type.
        t
    }

    /// File a type in the base's `type-alist`, where the library looks it up.
    ///
    /// EVERY type goes here, the ones `type make` builds at runtime as much as
    /// the builtins. `(type by-atom …)` in lib/x/type/struct.x walks this table
    /// and answers nil for anything absent — and its callers do not check:
    /// `lib/x/type/promise.x` pushes a call handler straight into what it gets
    /// back, so an unfiled type turned into a write through nil.
    ///
    /// Handle and tree are the same object here; the reference keys by a
    /// separate sentinel. The library only cares about the alist's shape.
    pub(crate) fn file_type(&mut self, t: Obj) {
        let base = self.base;
        // Keyed by the HANDLE, valued by the TREE — the shape x-lang walks:
        // `type by-atom` is handed what `type of` answered and expects the tree
        // back.
        let handle = self.objects.type_handle_of_tree(t);
        let entry = self.objects.spair(handle, t);
        let head = crate::base::get(&self.objects, base, crate::base::TYPE_ALIST);
        let cell = self.objects.spair(entry, head);
        crate::base::set(&mut self.objects, base, crate::base::TYPE_ALIST, cell);
    }

    /// Build the catalog: `((ns . ((method . prim) ...)) ...)`, the shape x-lang
    /// walks the base to find. Namespace and method symbols are interned, so the
    /// lookups the prelude performs are pointer comparisons.
    fn file_catalog(&mut self, rows: &[(&str, &str, Obj)]) -> Obj {
        let mut namespaces: Vec<(&str, Vec<(&str, Obj)>)> = Vec::new();
        for (ns, m, o) in rows {
            match namespaces.iter_mut().find(|(n, _)| n == ns) {
                Some((_, ms)) => ms.push((m, *o)),
                None => namespaces.push((ns, vec![(m, *o)])),
            }
        }
        let mut cat = NIL;
        for (ns, ms) in namespaces.iter().rev() {
            let mut methods = NIL;
            for (m, o) in ms.iter().rev() {
                let msym = self.objects.sym(m);
                let entry = self.objects.spair(msym, *o);
                methods = self.objects.spair(entry, methods);
            }
            let nsym = self.objects.sym(ns);
            let nsentry = self.objects.spair(nsym, methods);
            cat = self.objects.spair(nsentry, cat);
        }
        cat
    }

    /// A fresh base: a rootless environment carrying the instruction set, and a
    /// base spine naming it.
    ///
    /// ROOTLESS is the whole of the sandbox. The new frame has no parent, so a
    /// name defined in the host is genuinely unbound inside it — that is
    /// x-lang's isolation story, and `base bind` is how a host hands in exactly
    /// what it chooses.
    ///
    /// The instruction set IS given, because a fresh base must evaluate
    /// `(+ 2 3)`. A sandbox withholds the host's definitions, not the machine.
    pub fn make_base(&mut self) -> Obj {
        let env = self.envs.push_root();

        // `#t` and `#f` are instruction-level too: a form read in the host and
        // evaluated in a child must find them. They are `%isa-values` rows in
        // their own right, which is why they are declared and not merely bound.
        let t = self.objects.sym_shared(crate::vocabulary::TRUE);
        self.envs.bind(env, t, t);
        let f = self.objects.sym_shared(crate::vocabulary::FALSE);
        let fo = self.objects.false_obj();
        self.envs.bind(env, f, fo);

        // --- the identity values -------------------------------------------
        // x-lang's `meta/identity` capability, and it is CORE: an engine that
        // cannot say which release it is cannot be pinned against, and a pinned
        // amalgam from one release booting on another is the segfault (#435)
        // that put these here.
        //
        // `x-version` is the EXPRESSION LAYER's version and rightly stable;
        // `x-release` is which release of x-lang this engine is, stamped from
        // the environment at build time. They are deliberately different
        // numbers: before the reference engine separated them, two releases
        // whose sources never changed reported identically.
        for (name, text) in [
            (crate::vocabulary::X_MACHINE, env!("X_MACHINE")),
            (crate::vocabulary::X_VERSION, X_EXPR_VERSION),
            (crate::vocabulary::X_RELEASE, env!("X_RELEASE")),
        ] {
            let sym = self.objects.sym_shared(name);
            let v = self.objects.str_new(text);
            self.envs.bind(env, sym, v);
        }
        // The `%isa-values` rows: names an engine binds to OBJECTS rather than
        // to callables. They belong here, with the rest of the instruction set,
        // because the reference binds them into every base — asked directly, and
        // a child base sees both. Binding them only to the root left a sandbox
        // able to run the machine but not to see a signal or an end of input.
        //
        // The objects themselves are SHARED, not copied per base, which is also
        // what the reference does: a child's `%sigint-flag` is `(obj same?)` to
        // the host's, so a signal is visible from wherever it is observed.
        let eof = self.objects.sym_shared(crate::vocabulary::TOKEN_EOF);
        let (t, f) = (self.token_eof, self.sigint_flag);
        self.envs.bind(env, eof, t);
        let flag = self.objects.sym_shared(crate::vocabulary::SIGINT_FLAG);
        self.envs.bind(env, flag, f);

        for (sym, obj) in self.prim_bindings.clone() {
            self.envs.bind(env, sym, obj);
        }

        let base = crate::base::build(&mut self.objects, self.catalog, env);
        // The root frame serves the spine just built — stamped after, because
        // the spine cannot exist before its env does.
        self.envs.set_base(env, base);
        // A fresh base interns for itself, from empty. NOT a snapshot of the
        // parent's table: x-engine-c was asked, and a symbol the host interned
        // before the child existed is still a different object inside it.
        self.base_syms.insert(base, Symbols::new());
        base
    }

    /// Run `f` with `base`'s symbol table installed, restoring afterwards.
    ///
    /// Bracketed rather than assigned, because a base can evaluate into another
    /// base: the table that was running has to come back, not the engine's own.
    /// ALSO THE CURRENT BASE, not just the table. The reference has no
    /// equivalent bracket because it needs none: `p_base` is an ARGUMENT,
    /// threaded through every call, so "which base is current" is data flowing
    /// through the program. This engine keeps it in a field — and the bracket
    /// used to swap the symbol table while leaving the field alone, so under
    /// `(b eval …)` every prim that consults the base (`resolve_tree`,
    /// `file_type`, `make-instance`) read the HOST's registry while running the
    /// child's code. A type filed in a child base could never be found by the
    /// code that filed it.
    ///
    /// The displaced base rides `base_stack`, which the collector roots — the
    /// host base may have no other reference while a child runs.
    pub fn in_base<T>(&mut self, base: Obj, f: impl FnOnce(&mut Self) -> T) -> T {
        let table = self.base_syms.remove(&base).unwrap_or_default();
        let outer = self.objects.swap_symbols(table);
        self.base_stack.push(self.base);
        let prev = std::mem::replace(&mut self.base, base);
        let result = f(self);
        self.base = prev;
        self.base_stack.pop();
        let inner = self.objects.swap_symbols(outer);
        self.base_syms.insert(base, inner);
        result
    }

    /// The environment a base names, through the base's own committed route.
    pub fn base_env(&self, base: Obj) -> EnvId {
        crate::base::env_of(&self.objects, base)
    }

    /// The environment top-level forms evaluate in: this engine's own base.
    ///
    /// NOT a privileged "global" frame. Since `base make` exists, the engine's
    /// own context is simply the first base, and treating it as a special outer
    /// scope would make the host a parent of every sandbox.
    pub fn root_env(&self) -> EnvId {
        self.base_env(self.base)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base;
    use crate::prims;

    /// EVERY row in the instruction set must be reachable after registration.
    /// A row that registered nothing would sit in the table consuming an index
    /// while being uncallable, and nothing else would notice.
    #[test]
    fn every_registered_row_is_reachable() {
        let mut e = Engine::new();
        let env = e.root_env();
        for def in prims::all() {
            if let Some(name) = def.bare {
                let sym = e.objects.sym_shared(name);
                assert!(
                    e.envs.lookup(env, sym).is_some(),
                    "`{}` registered but not bound",
                    name
                );
            }
        }
    }

    /// The bare binding and the catalog entry are ONE object, because they come
    /// from one row. Two separately-made primitives would behave alike and fail
    /// this, which is the failure x-lang's suite looks for.
    #[test]
    fn a_bare_name_and_its_coordinate_are_the_same_object() {
        let mut e = Engine::new();
        let env = e.root_env();
        let catalog = base::catalog_of(&e.objects, e.base);
        for def in prims::all() {
            let (Some(name), Some((ns, m))) = (def.bare, def.coord) else {
                continue;
            };
            let bare = e.envs.lookup(env, e.objects.sym(name)).expect("bound");
            let filed = lookup_coord(&mut e, catalog, ns, m).expect("filed");
            assert_eq!(bare, filed, "`{}` and ({} {}) differ", name, ns, m);
        }
    }

    fn lookup_coord(e: &mut Engine, catalog: Obj, ns: &str, m: &str) -> Option<Obj> {
        let (nsym, msym) = (e.objects.sym(ns), e.objects.sym(m));
        let methods = e
            .objects
            .list(catalog)
            .find(|&entry| e.objects.first(entry) == nsym)
            .map(|entry| e.objects.rest(entry))?;
        e.objects
            .list(methods)
            .find(|&entry| e.objects.first(entry) == msym)
            .map(|entry| e.objects.rest(entry))
    }

    /// A fresh engine's base is a base like any other: `base make` and the
    /// engine's own context are built by the same function, so a route missing
    /// from one is missing from both.
    #[test]
    fn the_engines_own_base_carries_every_route() {
        let mut e = Engine::new();
        for b in [e.base, e.make_base()] {
            for (n, name) in base::ROUTES.iter().enumerate() {
                let mut at = b;
                for _ in 0..n {
                    at = e.objects.rest(at);
                }
                assert!(!at.is_nil(), "route `{}` missing from a base", name);
            }
        }
    }

    /// `#t` and `#f` are bound in every base, or a fresh one could not run a
    /// `match`.
    #[test]
    fn a_fresh_base_knows_true_and_false() {
        let mut e = Engine::new();
        let b = e.make_base();
        let env = e.base_env(b);
        for name in ["#t", "#f"] {
            let sym = e.objects.sym(name);
            assert!(e.envs.lookup(env, sym).is_some(), "`{}` unbound", name);
        }
    }
}
