//! x-engine-rust — an x-lang engine.
//!
//! The engine owes the wrapper exactly three things (x-lang's contract layer E,
//! `docs/engine-contract.md`):
//!
//!   1. it reads its program from STDIN and evaluates as it reads;
//!   2. it binds every argument-vector element as a list named `args`;
//!   3. it writes diagnostics to STDERR, prefixed `*** ERROR: `.
//!
//! It parses no flags. `--batch` and `--quiet` are x-lang's, read by library
//! code, and an engine that invented opinions about them would be implementing a
//! protocol it has no part in.

use x_engine::{diag, engine};

fn main() {
    let mut engine = engine::Engine::new();
    // Arm two meta units per allocation — source line and file id — as the
    // reference's CLI does. The engine core leaves the policy at zero; the
    // CLI is where source-location tracking is a decision.
    engine.arm_source_meta();
    // The JIT runtime door: emitted machine code resolves nine C-ABI helpers
    // from THIS binary with dlsym. The engine is the host (the safe trait
    // impl); the exported shims live in the foreign crate, and referencing
    // their addresses here keeps the linker from discarding them.
    let host: *mut dyn x_engine_foreign::JitHost = &mut engine;
    x_engine_foreign::jit_install(host);
    std::hint::black_box(x_engine_foreign::jit_exports());

    // EVERY argv element, argv[0] included: x-lang's own library documents
    // `args` as carrying the engine path first and drops it itself —
    // lib/x/tool/contract.x, "`args` minus the engine path and the engine
    // flags x.sh -f prepends", implemented as `(rest args)`.
    let argv: Vec<String> = std::env::args().collect();
    engine.bind_args(&argv);
    engine.set_input_stdin();

    // Read a form, evaluate it, read the next — the loop x-lang's wrapper pipes a
    // library into. A raise ends the run and is reported on stderr, the only
    // channel a bare engine has: there is no printer here, because `display` and
    // `write` are x-lang.
    if std::env::var("X_HEAP_STATS").is_ok() {
        // one-off measurement handler
    }
    while let Some(form) = engine.next_form() {
        if let Err(cond) = engine.eval_top(form) {
            report(&engine, &cond);
        }
        // Between forms the unread remainder compacts to the region's
        // front, which is what bounds a long session to the region.
        engine.compact_input();
    }
    if std::env::var("X_HEAP_STATS").is_ok() {
        eprintln!(
            "heap words={} ({} MB)  objects={}  frames={}",
            engine.objects.heap_words(),
            engine.objects.heap_words() * 8 / 1_048_576,
            engine.objects.alloc_count(),
            engine.envs.frame_count()
        );
    }
}

/// Write a diagnostic and end the run.
///
/// The prefix lives in `diag`, not here. It is contract layer E — x-lang's
/// wrapper reads it — and it was a bare literal written twice in this file.
///
/// A top-level raise ENDS the run, which was checked against x-engine-c rather
/// than assumed: `(error "a") (error "b")` reports the first and never evaluates
/// the second.
fn report(engine: &engine::Engine, cond: &diag::Cond) -> ! {
    eprintln!("{}", cond.diagnostic(&engine.objects));
    std::process::exit(1)
}
