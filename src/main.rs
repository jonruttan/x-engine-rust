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

use std::io::Read;
use x_engine::{diag, engine};

fn main() {
    let mut src = String::new();
    let mut engine = engine::Engine::new();
    if std::io::stdin().read_to_string(&mut src).is_err() {
        report(&engine, &diag::Cond::NoProgram);
    }

    // EVERY argv element, argv[0] included. x-lang's contract says "every argv
    // element is bound as the `args` list", and the reference binds all of them
    // -- its loop runs `for (i = argc - 1; i >= 0; i--)`.
    //
    // Skipping the program name looked tidier and made `args` NIL whenever the
    // engine was invoked without arguments, which is how every harness invokes
    // it. x-lang's compliance suite counts a bare name that evaluates to nil as
    // MISSING, so the engine declared invoke/argv and failed it -- and the
    // library's own `--batch` and `--quiet` scans never noticed, because a
    // program name matches neither.
    let argv: Vec<String> = std::env::args().collect();
    engine.bind_args(&argv);
    engine.set_input(&src);

    // Read a form, evaluate it, read the next — the loop x-lang's wrapper pipes a
    // library into. A raise ends the run and is reported on stderr, the only
    // channel a bare engine has: there is no printer here, because `display` and
    // `write` are x-lang.
    if std::env::var("X_HEAP_STATS").is_ok() {
        // one-off measurement hook
    }
    while let Some(form) = engine.next_form() {
        if let Err(cond) = engine.eval_top(form) {
            report(&engine, &cond);
        }
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
