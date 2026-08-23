//! OS facilities.
//!
//! Mirrors the reference engine's `x-prim/*` sys family. Small, and every row in
//! it runs into the same wall from a different side: an operating system is not
//! reachable from safe Rust's standard library.

use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{Obj, NIL};
use crate::prim::PrimDef;

/// `(sys clock)` — a monotonic microsecond counter.
///
/// PROCESS CPU TIME, which is what `lib/x/sys/posix.x` documents the instruction
/// to be: "current process CPU time in microseconds (the (Sys time) profiler
/// reads this)".
///
/// It was WALL time until the foreign door opened, because Rust's standard
/// library has no CPU-time API — `Instant` was all that was reachable without a
/// libc call. The conformance contract asks only that the number not go
/// backwards, so the deviation cost nothing a test could see and everything a
/// profile would: it attributed waiting to work.
fn clock(e: &mut Engine, _a: &[Obj]) -> EvalResult {
    Ok(e.objects.int(crate::foreign::cpu_micros()))
}

/// `(sigint-install)` — SIGINT sets `%sigint-flag` instead of ending the run.
///
/// BARE, like `alloc-limit!`, because the REPL arms it and the conformance suite
/// reaches it without a library.
fn sigint_install(e: &mut Engine, _a: &[Obj]) -> EvalResult {
    crate::foreign::interrupt_install();
    let flag = e.sigint_flag;
    e.objects.set_data(flag, 0, crate::obj::Word(0));
    Ok(NIL)
}

/// `(sigint-restore)` — the default disposition, so the process ends as it began.
fn sigint_restore(_e: &mut Engine, _a: &[Obj]) -> EvalResult {
    crate::foreign::interrupt_restore();
    Ok(NIL)
}

pub const TABLE: &[PrimDef] = &[
    PrimDef::filed_full("sys", "clock", 0, clock),
    PrimDef::bare_full("sigint-install", 0, sigint_install),
    PrimDef::bare_full("sigint-restore", 0, sigint_restore),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{truthy, with_coords};

    const SYS: &[(&str, &str, &str)] = &[("%clock", "sys", "clock")];

    /// The whole of what the contract asks: it must not go backwards.
    #[test]
    fn the_clock_does_not_go_backwards() {
        assert!(truthy(&with_coords(
            SYS,
            "(def t0 (%clock))
             (def burn (fn (self n) (match ((= n 0) 1) (#t (%seq (pair n n) (self (- n 1)))))))
             (burn 2000)
             (match ((< (%clock) t0) ()) (#t 1))"
        )));
    }

    /// And it advances, or a constant would satisfy the test above.
    #[test]
    fn the_clock_advances() {
        assert!(truthy(&with_coords(
            SYS,
            "(def t0 (%clock))
             (def burn (fn (self n) (match ((= n 0) 1) (#t (%seq (pair n n) (self (- n 1)))))))
             (burn 20000)
             (< t0 (%clock))"
        )));
    }
}
