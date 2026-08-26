# The architecture port

The reference is structured around DATA: one self-describing object tree that
behaviour emerges from by walking. The design's author, verbatim:

> The C version is structured around data. Your version is structured around
> code.

> You've hardcoded too many assumptions at the architectural level, and have
> built structures that deviate too heavily from the working solution built in
> C. It took me years to derive that architecture. You're not going to
> "discover" it in a few hours.

So this document is a TRANSCRIPTION plan, not a design — and the plan is
EXECUTED: all four increments are on `main`. The design exists, in
`ext/x-engine-c` (via the x-lang checkout) and it is already paid for. Every
increment below names the C structure it copies, landed it whole for one
subsystem, and was judged by the ratchets — 122 conformance checks that pass on
both engines, the spec suite, and this repo's unit tests. No increment merged
red, and no increment "improves" on the reference: deviations are recorded in
the commit that makes them, or not made.

## Why transcription, not derivation

The reference has STRUCTURES x-lang itself can walk; a Rust record with prims
in front presents the same surface until the library reaches where the record
didn't anticipate — `intrinsics.x` poking buffer cells, logo pruning a
type-alist, `reflect.x` replacing prims. The collector's root set states the
same point numerically: the reference enumerates THREE roots because the
interpreter state IS the base tree, and every root a hand-kept list omits is a
use-after-free waiting for stress.

## Target invariants (all from the C; none invented here)

1. **The current base is an argument.** `p_base` threads through every call in
   the reference, so "which base is running" is data flowing through the
   program, and `(b eval …)` needs no bracket, no swap, no compensating root
   stack. The `in_base` bracket and the `base` field are approximations to
   retire.
2. **Interpreter state lives on the base tree.** Catalog, handlers, roots, the
   sigint flag, the token-eof sentinel, the type-alist — base fields, reached
   by route. The collector's root set becomes: the base, the root chain, the
   registered roots. Frames follow last (the env model is the largest piece).
3. **Runtime structures are object trees x-lang can walk.** A buffer IS
   `(val . (read . write))` — cells, so `%cell-int`/`%buffer-unread` in
   `lib/x/reader/intrinsics.x` work by construction rather than by the
   accident of slot order. A token base IS a base. A type IS a spine of pairs
   the library walks. If the library can name it, it is made of pairs.
4. **Behaviour is data walked by thin code.** The analyse slots, the ops
   alists, the from-relations already moved; the remaining Rust dispatch
   special cases shrink to walkers as the structures above land.

## Increments, in leverage order — ALL FOUR COMPLETE

Each was one branch, one commit narrative, gates green before and after. All
four are on `main`; D landed via PR #5.

- **A. Buffer as pair cells.** COMPLETE. A buffer is the reference's
  `(val . (read . write))` in `value/tok.rs` + `prims/tok.rs`, judged by
  `lib/buffer.spec.md` plus the tokenizer suite.
- **B. Engine singletons onto the base.** COMPLETE. token-eof, sigint flag,
  catalog, interned-symbol tables are base fields (routes in
  `tools/contract/base-paths.x`); the root set shrank accordingly, judged by
  the poison stress mode.
- **C. `p_base` as an argument.** COMPLETE. Every applicative takes the base —
  DYNAMIC, flowing through the call, never derived from the environment's
  frame. `(b eval …)` is a call with a different argument, as in the C.
- **D. Frames into the tree.** COMPLETE. An environment is a holder object —
  chain head, parent holder, base — whose chain is a spine of ordinary spair
  cells; `EnvId` is a newtype over the holder. The frame vector, the
  collector's fixpoint machinery, frame sweeping, and the dead-frame traps
  are deleted: one plain mark traces holders and cells like any other
  objects. Landing it surfaced a rule the plan now records below.

## E. Evaluation is a type handler — ALL FOUR ABOVE WERE PRELUDE

The reference's machine is `x_eval`: nil answers nil, an untyped value
answers itself, and EVERYTHING ELSE is one line — call the value's type's
EVAL handler with the argument frame. Symbol lookup is the SYMBOL
type's registered eval (`x_type_symbol_eval`); application is the LIST
type's, which evaluates the head and dispatches through the OPERATOR'S
type's CALL handler; a callable is any value whose type registers call. The
author's statement of what this buys, verbatim:

> The design of the C engine allows for the interpreter to change to
> interpret any syntax. Using x-lang the engine can be altered to be a
> Javascript interpreter, or a C compiler, or even a CPU.

The uniformity that makes it work is `x_callable_call`: every callable
stores its entry in SLOT 0 and the dispatch is "read slot 0, call it with
`(callable . args)`" — self rides in the args, and procedure/operative/
primitive are which function sits in the slot, not kinds of a dispatcher.

This engine's evaluator WAS the inverse: a Rust match over object kinds,
with an eight-variant `Body` enum behind the primitive arm — every
semantic it hardcoded was a semantic no base could replace. The
increments, ALL THREE COMPLETE (PRs #7-#12):

- **E1. Eval dispatches through the type.** COMPLETE. The eval core is the
  reference's: read the type word; a type with an eval handler decides, a
  value without one is itself. The current symbol and pair arms become
  ENGINE EVAL HANDLERS registered on every base's SYMBOL and PAIR types —
  replaceable per base from x-lang, like every other handler now is.
- **E2. Application dispatches through the type.** COMPLETE. The list
  handler resolves
  its operator and applies through the operator's type's CALL handler;
  PROCEDURE, OPERATIVE and PRIMITIVE types register theirs, and the
  evaluator's kind-match dissolves. Class value-call joins natively.
- **E3. One calling convention.** COMPLETE, spec-first: every
  callable carries its ENTRY in slot 0 (a table index — the engine's
  spelling of the reference's function pointer) and its state in slot 1;
  a closure's state is the reference's `(params body env . bst)` spine,
  which `lib/x/tool/cov.x` reads and
  `tests/x/conformance/core/handlers.spec.md` now states as law; the four
  per-kind call handlers collapsed into ONE door that reads slot 0 and never
  consults the callee's kind — a foreign address misses the table and
  declines, keeping its invocation with the undeclared jit lane; and
  `Body` is DELETED — every row is one function shape that evaluates its
  own arguments (fast paths survive INSIDE a uniform row, as
  `x_prim_arith_binop` keeps its `use_ops` flag inside one signature —
  never as dispatcher kinds). The environment convention settled below.

THE ENVIRONMENT CONVENTION, SETTLED: the current environment is an
ARGUMENT, as the base is. The reference keeps it on the base's
`env-alist` field because C threads one context pointer — and its whole
compound-save/restore machinery exists to repair that mutation; passing
the environment beside the base gives the same dynamic value without the
dance, and is the same philosophy as invariant 1. The library-visible
door follows: where the reference's handlers read the current environment
off the base's routes, this engine's library eval handlers RECEIVE it — the
value first, the environment second, and a one-parameter handler never sees
the extra argument. The doors differ per engine as base routes do under
decision L1; the LAW (handlers govern, and can resolve names in the scope
they run in) is the same.

PER-BASE STAMPING, COMPLETE: allocation resolves the kind's type
through the current base's type-alist — found by the kind's handle, or
built, filed there, and answered, as `x_type_struct_get` does — so a
child base's values carry the child's types and the host's carry the
host's. The walk runs per typed allocation, the reference's own cost
model. Builtin types carry NO write/display handler (the reference's
constructors leave those stacks nil-headed and the library's printer
falls back by name); the engine-render-on-every-type deviation is gone
with the singleton stamp table.

## The off-heap rule (paid for landing D)

A Rust-side map keyed by a heap ADDRESS outlives what it describes: the
address outlives its object, and a recycled chunk inherits the dead object's
entry. The collector purges such maps (`Envs::index`, `base_syms`) at every
sweep, and any new address-keyed map must join that purge. The diagnostic
signature when one is missed: the fault is reuse-dependent, poison mode hides
it without a single read-trap, and a freed-but-reachable check stays clean —
because the stale reference is in a map no mark walks. Grep `HashMap<Obj`.

## What remains

Invariant 4's tail: the remaining Rust dispatch special cases shrink to
walkers as their structures move. (The token base open recorded here
earlier closed with the tokenizer arc — it is a base with a type-alist
now.) EVAL STATE ON THE BASE, COMPLETE for the tail, the saves, and
the guard: `save-stack`, `tco-expr`, `tco-env`, `sigint`, and
`error-handler` are base rows the evaluator reads and writes — the
save stack is the `def` question, the tco pair is the parked tail, the
handler row is the active guard chain, and the current base's row
nodes are cached across the `in_base` bracket. THE READER TOO: it has
no state of its own — reading walks the buffer at the head of the
base's `buffer` row, `include` pushes and pops the `filein`, `buffer`,
and `line` rows as the reference's include does, and a reader macro's
nested read continues on the same stream because there is only one.
The Engine struct now holds Rust-side resources (open files, roots,
the instruction table), not evaluator state. STDIN REFILLS: the
program's own input arrives one byte at a time through the interactive
source buffer — the reference's read channel, with its EOF latch (end
of input flips the filein head to the fd's bitwise complement, sticky
and recoverable). An interactive source prefetches to a line boundary
before each form, so the analyser contest's bounded view holds every
byte a token on the line can claim; a claimed token typed across a
newline still scores short, which files never see. The full-suite gap to the
reference is 93 of 2549 from clean state, with the pin spec passing
whole — the chronic pin failures were a lock its own spec stranded
across runs, healed on the x-lang side. The gap's families: list
call, float, proc and the posix tail, apply and improper call forms,
error-atom printing, BOOL. Tracked in x-lang's suite, not here.

## What is NOT ported

The reference's known faults, recorded in x-lang's own notes, stay out: the
free-hook allocate-and-escape hazard (its fix — hooks before marking — is
already this engine's behaviour and is now a conformance law), and the
documented-broken free-hook observability. Where the reference and the laws
disagree, the laws win, because both engines pass them.
