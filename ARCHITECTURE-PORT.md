# The architecture

This engine follows the structure of the reference engine,
[x-engine-c](https://github.com/jonruttan/x-engine-c). The reference is
structured around data: one self-describing object tree, and behaviour that
comes from walking it. x-lang's library walks those structures itself, so an
engine that presents the same surface over different structures stops working
where the library reaches past the surface. Each section below names the
reference structure this engine carries, and the last lists where it differs.

## The rules

1. **The base is an argument.** Every primitive receives the base it is
   called in. `(b eval …)` is a call with a different base.
2. **Interpreter state lives on the base.** The catalogue, the types, the
   handlers, the input in flight and the evaluator's saved state are rows of
   the base, reached by route.
3. **Runtime structures are object trees x-lang can walk.** If the library
   can name a structure, it is made of pairs.
4. **Behaviour is data walked by thin code.** What evaluation, application
   and reading mean is held in handlers on types and in slots on the base,
   and each can be replaced from x-lang.

## Objects

Every object lives in one pinned arena of machine words (`src/heap.rs`), and
an object reference is the machine address of its first word. The header is
four words, as `tools/contract/obj-layout.x` gives them:

| word | holds |
|---|---|
| 0 | the heap link: every object is threaded here when allocated |
| 1 | the type object, or nil |
| 2 | the flags |
| 3 | the count of data words |
| 4… | data |

A pair is two data words, `first` and `rest`. The arena is shared by every
base, so an object can be handed from one base to another.

## The base

A base is the execution context. It is an object of two units: a spine and
a slot vector.

**The spine** is a list with one cell per row, and a row's route ends at the
cell whose `first` is its value. `tools/contract/base-paths.x` names the
routes, and the library resolves them by name. The rows:

- the primitive catalogue (`prims`) and the registered types (`type-alist`);
- the last error (`error-str`, `err-line`, `err-file`, `err`) and the active
  guard chain (`error-handler`);
- this base's environment (`env`, `env-root`);
- the input in flight (`filein`, `buffer`, `line`, `files`,
  `file-registry`);
- the evaluator's saved state (`save-stack`, `tco-expr`, `tco-env`,
  `sigint`);
- the collector's hooks and roots (`heap-mark-hooks`, `heap-free-hooks`,
  `heap-mark-roots`) and the allocation ceiling (`alloc-limit`,
  `alloc-count`);
- `true` and `false`.

A type is a spine of pairs too. Its routes are in the same file: a handler
family, among them `name`, `mark`, `make`, `free`, `units`, `length`,
`call`, `eval`, `from`, `to`, `analyse`, `delimit`, `read`, `write`,
`display` and `iter`, is a stack whose head is the handler in force.

**The slot vector** (`src/slots.rs`) holds the engine's routines, each at a
fixed position, and the engine calls a routine through its position.
`tools/contract/base-slots.x` gives the nineteen positions:

| slots | routines |
|---|---|
| 0–5 | `eval`, `eval-list`, `eval-body`, `eval-body-tco`, `eval-tco-trampoline`, `eval-op-body` |
| 6–8 | `callable-call`, `callable-apply`, `obj-prim-call` |
| 9–12 | `env-lookup`, `env-bind`, `env-extend`, `alist-bst-lookup` |
| 13–15 | `token-read`, `token-analyse`, `token-delimit` |
| 16–18 | `heap-mark`, `heap-sweep`, `obj-alloc` |

A slot is a word holding the address of a routine with the engine's one
signature, `(base, run) -> value`. A language reads a slot and calls it
through `ptr call`, stores another routine over it with `obj set!`, and the
routine it stored can call the one it replaced. Every base is made with the
foreign crate's slot doors in its slots: C-ABI entries that run this
engine's own routines.

## Environments

An environment is one pair, `(bindings . parent)` (`src/env.rs`). The root's
bindings are a tree whose nodes are `(entry . (left . right))`, ordered by
the name's spelling, and its parent is nil. Every other environment the
engine makes holds an alist of `(name . value)` entries, and its parent is
the environment it was made in.

The value is the pair: an operative receives it, `eval` takes it as its
second argument, and the library walks it with `first` and `rest`. A name is
found by identity.

## Evaluation

The evaluator (`src/eval.rs`) is the reference's `x_eval`. A value whose
type word is nil, or one of the two raw markers, is itself. Any other value
is decided by its type's `eval` handler, and a value whose type registers
none is itself.

- **A symbol** evaluates by the SYMBOL type's handler, which looks the name
  up.
- **A list** evaluates by the list type's handler, which evaluates the head
  and applies it through the `call` handler of the operator's type.
- **A callable** is any value whose type registers `call`. Its entry is in
  its slot 0, and one door reads that entry and calls it; procedures,
  operatives and primitives differ in which routine the entry names, not in
  a case of a dispatcher.

Primitives are operatives: a primitive receives its arguments unevaluated
and evaluates the ones it wants. A primitive is a row of data
(`src/prim.rs`): where it is reachable from, its arity, and a function. The
bare name and the catalogue coordinate come from the same row, and the rows
mirror `tools/contract/isa.x`.

A handler on a type may be the engine's or the library's. An engine handler
takes the form unevaluated. A library handler is a closure applied to the
value, with the current environment as a second argument.

Because the handlers are data on the base, a library can change what
evaluation means for a type, in one base, without touching the engine.

## Types in a base

Allocation resolves a value's type through the running base's `type-alist`:
the type is found there by its handle, or made, filed there and answered
(`src/engine.rs`). A child base's values carry the child's types, and the
host's carry the host's.

## Reading

The reader has no state of its own (`src/read.rs`). It walks a buffer,
`(val . (read . write))` over a text string, and the buffer it walks is the
head of the base's `buffer` row. Reading advances the buffer's read mark, so
`io read-char` and a reader macro see what is left after the current form.
`include` keeps its file on the base's `filein` row while it reads.

Where a form may begin, the analysers registered on the base's types are
asked first, and the built-in syntax is the fallback (`src/form.rs`). A
reader macro the library installs therefore changes every later read,
`include` and the session's own loop among them.

## The collector

`heap collect` marks and sweeps (`src/collect.rs`), and nothing else
collects. Sweeping walks the chain through each object's heap link. A freed
object's words go on a free list for an allocation of the same size, and
nothing moves.

The roots are the base, the list the library registers with
`heap mark-root!`, and the values the evaluator is holding in Rust locals,
which it pushes and pops as it goes. Everything else the interpreter holds is
reachable from the base.

The mark hooks run before the mark, and the free hooks before the sweep.

A map on the Rust side keyed by a heap address outlives what it describes:
the address outlives its object, and a chunk handed out again inherits the
dead object's entry. Every such map is purged at each sweep (`base_syms`,
the continuation snapshots), and a new one must join that purge.

## Continuations

`call/cc` makes an escape continuation and snapshots the control records in
flight: a body mid-walk, an argument list mid-evaluation, a binding waiting
on its value. Invoked while its frame is live, the continuation unwinds to
it. Invoked after the frame has returned, it replays the snapshot, inner to
outer. A capture that crosses a frame with no record is refused when it is
invoked, and the refusal can be caught.

## Where it differs from the reference

- **The current environment is an argument.** The reference keeps it on the
  base and saves and restores it around a call; here it is passed beside the
  base. A library `eval` handler therefore receives the environment as its
  second argument where the reference's reads it from the base.
- **The built-in syntax is in Rust.** The reference puts it in handlers on
  the built-in types, so a library can replace it. Here a library can put a
  macro ahead of the built-in reader and cannot replace it.
- **Instances are traced conservatively.** An object made with `obj make`
  holds words the library writes as it likes. The collector follows a word
  of one only when it is in the arena and aligned as an object is. Kinds
  whose layout is known are traced exactly.
- **Continuations replay.** The reference re-enters a continuation by
  copying its C stack back.
- **The declaration is narrower.** `x-engine.xon` has no `full` profile, and
  lacks `instr/cov`, `instr/profile`, `tok/label`, `err/typed-raise` and
  `str/nul-terminated`.

## Measurement

x-lang's conformance suite and spec suite are the judges, and both live in
x-lang. The spec suite has not been run to a whole result on this engine
since the port of x-engine-c's surface through v0.2.31.
