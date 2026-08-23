# x-engine-rust

A second engine for [x-lang](https://github.com/jonruttan/x-lang), in Rust.

The engine core `forbid`s unsafe code. It is not "safe Rust" outright, and the
distinction is the design: `forbid` cannot be overridden by an inner `allow`,
which is why the foreign door — dlopen, calling through a machine address,
syscalls, signal dispositions — is a SEPARATE crate (`foreign/`) rather than a
module. That crate is the only place the compiler permits `unsafe`.

x-lang is a language, not an implementation. `x-engine-c` is one engine; this is
an attempt at another, written to the published contract rather than by reading
the C and copying it.

## Where it is

```
$ X_ENGINE_DIR=../x-engine-rust sh tools/check/engine-contract.sh   # in x-lang
engine-contract:
  satisfaction: x-engine-rust provides everything requires.x needs.
```

Nineteen capabilities, and the `core`, `gc` and `posix` profiles. 102 of 102
conformance checks; 18 declared compliance rows, none failing.

**What that does NOT mean is that x-lang runs on it.** Every suite above is
BARE — the conformance and compliance runners load no library, on purpose — and
x-lang's library has never booted here: piping `lib/x-core.x` in dies at once,
because the first thing it wants is `display` and its boot needs `./` includes
resolved against the INCLUDING file, which this engine resolves against the
working directory. A satisfied capability set is a claim about instruction
coverage, not a claim about booting, and the distance between the two is
currently unmeasured.

The refusal this line used to print was the specification, and it is *derived*
from what the library actually reaches, so it never drifted the way a
hand-written task list would.

## Checked, not guessed

Where x-lang's documents do not rule, x-engine-c is asked and the answer written
down. Three so far:

- A top-level raise **ends the run**; the second form of
  `(error "a") (error "b")` never evaluates.
- The unbound-symbol text is `Unbound SYMBOL 'name`.
- `eq?` compares **numbers by value and strings by identity**: `(eq? 1 1)` holds,
  `(eq? "a" "a")` does not.

The first version of this engine got the third one wrong in the direction that
looks right — pure pointer identity — and the conformance harness said so.

## The target

`docs/engine-contract.md` in x-lang describes four profiles. The one to aim at is
**`core`** — 108 instructions — and the interesting thing about it is what it
leaves out: no foreign door, no syscalls, and **no garbage collector**. The
smallest thing that can boot x-lang allocates and never frees.

That shapes everything here:

| group | count | notes |
|---|---|---|
| `isa/raw-op` | 32 | machine arithmetic; trivial |
| `isa/alloc` | 12 | constructors |
| `isa/raw-mem` + `reflect/ptr-casts` | 16 | the heap |
| `isa/tok` | 9 | the reader's scoring protocol |
| `isa/hot` | 7 | derivable — may be written in x rather than here |
| `isa/io` | 5 | byte I/O |
| `isa/types` | 2 | |
| `isa/spine` | 25 | the evaluator — the hard part |

`call/cc` sits in the spine, but x-lang's library never calls it (only
`doc-prims.x` documents it), so escape-only continuations suffice for everything
the language actually does.

## The heap, and why the core can forbid unsafe

Decision L1 in x-lang's contract requires a word-addressable object model:
`obj->ptr`, `ptr->int` round-tripping, and `ptr ref-word` reading offsets that
**x-lang computes itself** from the layout descriptors.

That normally forces raw pointers. It does not here, because the `core` profile
has **no foreign door** — no `dlopen`, no `ptr call` — so a pointer never escapes
to C and nothing outside this engine ever dereferences one. So:

- objects live in a flat heap of machine words;
- a "pointer" is a **byte offset into that heap**, not a machine address;
- every reflective read x-lang performs lands back inside the heap by
  construction.

`int/ptr-same-width` holds because an offset and a fixnum are both a machine word.

## This engine's layout is not the C engine's

`tools/contract/obj-layout.x` reserves **no heap-link word**: with no collector
there is no chain to thread, so the header is two words where the C engine's is
three, and every later slot shifts down.

That is a CONSEQUENCE, not a goal. The header is short because a feature is
missing, and nothing was chosen here.

An earlier version of this section claimed the difference was the point — that
x-lang booting on both engines would show decision L1 (the layout travels with
the engine) doing its job. That was wrong three times over, and worth recording
rather than quietly deleting:

- **x-lang has never booted on this engine.** Not once. Piping `lib/x-core.x`
  in dies immediately; the first thing the library wants is `display`, and its
  boot needs `./` includes resolved against the INCLUDING file, which this
  engine resolves against the working directory. The suites this engine passes
  are all BARE — the conformance runner and the compliance runner both load no
  library, on purpose. Passing them says nothing about booting.
- **Nothing would report the verdict.** The `layout` digest in `x-engine.xon`
  is a staleness check and only that: `tools/check/compliance.sh` asks whether
  the declaration still matches this engine's own descriptor files. No part of
  the apparatus compares this layout against x-lang's expectation, so there is
  no mechanism by which "boots on both" becomes a judgement about L1.
- **An experiment nobody designed is not evidence.** The difference arrived by
  omission; calling it a probe afterwards dressed a gap as a result.

What the layout difference actually is: an open question about whether the
library tolerates a header of a different width, which stays open until this
engine boots one. That is a fact about this engine's maturity, not a finding
about x-lang's contract.

## Order of work

Following x-lang's own advice for a second implementation:

1. Ship the four contract files describing this engine's own layout.
   (`obj-layout.x` done; `base-paths.x` and `base-layout.x` come with the base.)
2. Write `claims.x` for the guarantees actually made. Claim less rather than more.
3. Generate `x-engine.xon` with x-lang's generator, run against this directory.
4. Implement toward `core`, checking against x-lang's conformance suite:
   `make conformance X_BIN=.../x-engine X_ENGINE_DIR=.../x-engine-rust`
5. Add `gc`, then `posix`, if and when the library tiers that need them matter.

Two guarantees are already claimed and both are free rather than earned:
`gc/explicit-only` and `gc/non-moving` hold because there is no collector at all.
If one is ever added they stop being free and must be re-earned.

## Building and checking

```
cargo build --release           # produces target/release/x-engine
sh check.sh                     # fmt, clippy (warnings are errors), unit tests
sh check.sh /path/to/x-lang     # ...and the conformance suite
```

The unit tests and the conformance suite answer different questions. The tests
ask whether a primitive does what this engine intends; the suite asks whether
that intent is x-lang's. Neither substitutes for the other, and for a while this
repo had only the second — which is how it accumulated a 615-line dispatcher and
no way to test a primitive in isolation.

The binary is named `x-engine`, not `x-bin`; `claims.x` says so in a `(binary …)`
row, and x-lang's wrapper reads it rather than assuming a filename.
