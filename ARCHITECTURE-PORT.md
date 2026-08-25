# The architecture port

The reference is structured around DATA: one self-describing object tree that
behaviour emerges from by walking. The design's author, verbatim:

> The C version is structured around data. Your version is structured around
> code.

> You've hardcoded too many assumptions at the architectural level, and have
> built structures that deviate too heavily from the working solution built in
> C. It took me years to derive that architecture. You're not going to
> "discover" it in a few hours.

So this document is a TRANSCRIPTION plan, not a design. The design exists, in
`ext/x-engine-c` (via the x-lang checkout) and it is already paid for. Every
increment below names the C structure it copies, lands it whole for one
subsystem, and is judged by the ratchets — 114 conformance checks that pass on
both engines, the spec suite, and this repo's unit tests. No increment merges
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

## Increments, in leverage order

Each is one branch, one commit narrative, gates green before and after.

- **A. Buffer as pair cells.** Smallest, self-contained, and retires the whole
  class of "slots happen to line up" coincidences. `value/tok.rs` +
  `prims/tok.rs`; the spec is `lib/buffer.spec.md` plus the tokenizer suite.
- **B. Engine singletons onto the base.** token-eof, sigint flag, catalog,
  interned-symbol tables addressed as base fields (routes already exist in
  `tools/contract/base-paths.x`). Root set shrinks accordingly; the poison
  stress mode judges it.
- **C. `p_base` as an argument.** Mechanical, wide diff: Engine methods take
  the base; the field and `in_base` bracket retire. `(b eval …)` becomes a
  call with a different argument, as in the C.
- **D. Frames into the tree.** The env model — `Envs`, `EnvId`, the frame
  vector — becomes environment objects on the heap, as the reference's
  env-alist is. Largest and last; the collector's fixpoint machinery
  simplifies to plain marking when it lands.

## What is NOT ported

The reference's known faults, recorded in x-lang's own notes, stay out: the
free-hook allocate-and-escape hazard (its fix — hooks before marking — is
already this engine's behaviour and is now a conformance law), and the
documented-broken free-hook observability. Where the reference and the laws
disagree, the laws win, because both engines pass them.
