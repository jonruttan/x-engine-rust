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
subsystem, and was judged by the ratchets — 114 conformance checks that pass on
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
2. **Interpreter state lives on the base tree.** Catalog, hooks, roots, the
   sigint flag, the token-eof sentinel, the type-alist — base fields, reached
   by route. The collector's root set becomes: the base, the root chain, the
   registered roots. Frames follow last (the env model is the largest piece).
3. **Runtime structures are object trees x-lang can walk.** A buffer IS
   `(val . (read . write))` — cells, so `%cell-int`/`%buffer-unread` in
   `lib/x/reader/intrinsics.x` work by construction rather than by the
   accident of slot order. A token base IS a base. A type IS its descriptor
   spine. If the library can name it, it is made of pairs.
4. **Behaviour is data walked by thin code.** The analyse slots, the ops
   alists, the from-relations already moved; the remaining Rust `Family`/
   dispatch special cases shrink to walkers as the structures above land.

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
walkers as their structures move. The known open is the token base — `base
make-type` answers the tree where the reference answers the name atom after
filing, because the token base is not yet a base with a type-alist. The
full-suite gap to the reference (181 of 2549, distribution: logo, math
functions, Buf construction, numeric guards, posix tail) is tracked in
x-lang's suite, not here.

## What is NOT ported

The reference's known faults, recorded in x-lang's own notes, stay out: the
free-hook allocate-and-escape hazard (its fix — hooks before marking — is
already this engine's behaviour and is now a conformance law), and the
documented-broken free-hook observability. Where the reference and the laws
disagree, the laws win, because both engines pass them.
