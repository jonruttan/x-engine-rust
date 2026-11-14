//! OS facilities.
//!
//! Mirrors the reference engine's `x-prim/*` sys group. Small, and every row in
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
fn clock(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    Ok(e.objects.int(crate::foreign::cpu_micros()))
}

/// `(sigint-install)` — SIGINT sets `%sigint-flag` instead of ending the run.
///
/// BARE, like `alloc-limit!`, because the REPL arms it and the conformance suite
/// reaches it without a library.
fn sigint_install(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    crate::foreign::interrupt_install();
    let flag = e.sigint_flag;
    e.objects.set_data(flag, 0, crate::obj::Word(0));
    Ok(NIL)
}

/// `(sigint-restore)` — the default disposition, so the process ends as it began.
fn sigint_restore(_e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    crate::foreign::interrupt_restore();
    Ok(NIL)
}

/// A pointer operand as the address a system call receives.
fn real_of(e: &Engine, p: Obj) -> u64 {
    e.objects.as_ptr(p).raw()
}

/// `(sys open path flags)` — the descriptor, or a negative refusal.
fn sys_open(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let path = e.objects.bytes_of(a[0]);
    let flags = e.objects.as_int(a[1]) as i32;
    Ok(e.objects.int(crate::foreign::open_path(&path, flags)))
}

/// `(sys close fd)`
fn sys_close(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let fd = e.objects.as_int(a[0]) as i32;
    Ok(e.objects.int(crate::foreign::close_fd(fd)))
}

/// `(sys read fd p n)` — up to n bytes into the memory at p.
fn sys_read(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let fd = e.objects.as_int(a[0]) as i32;
    let at = real_of(e, a[1]);
    let n = e.objects.as_int(a[2]).max(0) as usize;
    Ok(e.objects.int(crate::foreign::read_into(fd, at, n)))
}

/// `(sys write fd p n)` — n bytes from the memory at p, NULs and all.
fn sys_write(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let fd = e.objects.as_int(a[0]) as i32;
    let at = real_of(e, a[1]);
    let n = e.objects.as_int(a[2]).max(0) as usize;
    Ok(e.objects.int(crate::foreign::write_from(fd, at, n)))
}

/// `(sys read-char fd)` — one byte, or -1.
fn sys_read_char(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let fd = e.objects.as_int(a[0]) as i32;
    Ok(e.objects.int(crate::foreign::read_char(fd)))
}

/// `(sys exit status)` — ends the process.
fn sys_exit(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let status = e.objects.as_int(a[0]) as i32;
    crate::foreign::exit_with(status)
}

crate::uniform_engine!(sys_open_u, sys_open, 2);
crate::uniform_engine!(sys_close_u, sys_close, 1);
crate::uniform_engine!(sys_read_u, sys_read, 3);
crate::uniform_engine!(sys_write_u, sys_write, 3);
crate::uniform_engine!(sys_read_char_u, sys_read_char, 1);
crate::uniform_engine!(sys_exit_u, sys_exit, 1);
crate::uniform_engine!(clock_u, clock, 0);
crate::uniform_engine!(sigint_install_u, sigint_install, 0);
crate::uniform_engine!(sigint_restore_u, sigint_restore, 0);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(None, Some(("sys", "clock")), 0, clock_u),
    PrimDef::row(None, Some(("sys", "read")), 3, sys_read_u),
    PrimDef::row(None, Some(("sys", "write")), 3, sys_write_u),
    PrimDef::row(None, Some(("sys", "open")), 2, sys_open_u),
    PrimDef::row(None, Some(("sys", "close")), 1, sys_close_u),
    PrimDef::row(None, Some(("sys", "read-char")), 1, sys_read_char_u),
    PrimDef::row(None, Some(("sys", "exit")), 1, sys_exit_u),
    PrimDef::row(Some("sigint-install"), None, 0, sigint_install_u),
    PrimDef::row(Some("sigint-restore"), None, 0, sigint_restore_u),
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
