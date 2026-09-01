//! The foreign door: libc, and calling through a machine address.
//!
//! THE ONLY CRATE THAT MAY WRITE `unsafe`. The engine core FORBIDS it, and
//! `forbid` is the level an inner `#![allow]` cannot override — so the door
//! cannot be a module of the engine, and is a package of its own instead. A
//! `deny` one file opts out of is a convention; a boundary the compiler enforces
//! across crates is not.
//!
//! Everything here reaches outside the process: loading a library, calling
//! through a machine address, entering the kernel, changing a signal
//! disposition. Nothing here knows what an x-lang value is — marshalling stays
//! on the safe side, because moving ordinary code next to unsafe code does not
//! make it safer, it only makes the unsafe harder to review.
//!
//! # Why this is a different kind of pointer
//!
//! Everywhere else in this engine a "pointer" is a BYTE OFFSET into
//! the engine's heap, and that is what lets the rest be safe: nothing outside the
//! engine ever dereferences one, so an offset can never be a wild address.
//! `dlsym` answers a real machine address and breaks that premise, so foreign
//! addresses are kept in their own type — confusing the two is exactly the class
//! of bug the newtypes exist to prevent, and here it would be a segfault rather
//! than a wrong answer.
//!
//! # Handing C a pointer into the heap
//!
//! A string's bytes live in the heap's `Vec<u64>`, NUL-terminated, so a real
//! pointer to them can be handed to C for the duration of one call. That is
//! sound only because the call cannot allocate into this heap — nothing
//! re-enters the engine — so the Vec cannot reallocate under the callee and no
//! foreign code retains the address afterwards. Both conditions are the caller's
//! to keep, which is why marshalling lives here and not at the call sites.

use std::ffi::{c_char, c_int, c_long, c_void};

/// A real machine address, as answered by `dlopen`/`dlsym`.
///
/// Deliberately NOT the engine's `Addr`, which is an offset into its own heap. The two are both machine words and mean entirely different things;
/// one of them is safe to dereference and the other is not this engine's to
/// vouch for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Foreign(pub u64);

impl Foreign {
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

// libm, kept in the process ON PURPOSE. See `anchor_libm`.
#[link(name = "m")]
extern "C" {
    fn ldexp(x: f64, n: c_int) -> f64;
}

extern "C" {
    fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, sym: *const c_char) -> *mut c_void;
    fn clock() -> c_long;
    fn signal(sig: c_int, handler: usize) -> usize;
    fn write(fd: c_int, buf: *const u8, count: usize) -> isize;
    /// VARIADIC, and declared that way deliberately. On aarch64 a variadic
    /// argument goes on the STACK where a fixed one goes in a register, so a
    /// non-variadic declaration would compile, run, and hand the kernel
    /// whatever happened to be in memory.
    fn syscall(number: c_long, ...) -> c_long;
}

/// The raw kernel door: hand it a number and its arguments, answer what it
/// answers. A negative result is `-errno`, which every caller in `lib/x/sys/`
/// folds.
///
/// Six arguments, the kernel maximum on both targets.
pub fn kernel(number: i64, args: &[u64]) -> i64 {
    let mut a = [0u64; 6];
    for (slot, v) in a.iter_mut().zip(args.iter()) {
        *slot = *v;
    }
    // SAFETY: the kernel validates the call and reports refusal as -errno rather
    // than by faulting. A wrong number asks for a syscall that does not exist,
    // which is ENOSYS, and x-lang's own case exercises exactly this by asking
    // `write` to fail on a closed descriptor.
    unsafe { syscall(number as c_long, a[0], a[1], a[2], a[3], a[4], a[5]) as i64 }
}

/// `CLOCKS_PER_SEC` is 1_000_000 on every platform this engine targets, which is
/// what makes `clock()` a microsecond counter directly.
const CLOCKS_PER_SEC: i64 = 1_000_000;

/// Process CPU time in microseconds.
///
/// This is what `lib/x/sys/posix.x` documents `sys clock` to be — "current
/// process CPU time in microseconds (the (Sys time) profiler reads this)" — and
/// what `std::time::Instant` could not give: std has no CPU-time API, so the
/// clock was wall time until this door opened.
pub fn cpu_micros() -> i64 {
    // SAFETY: `clock` takes no arguments, touches no memory of ours, and is
    // defined for every process. Its only documented failure is answering -1,
    // which passes through as a value.
    let ticks = unsafe { clock() } as i64;
    ticks.saturating_mul(1_000_000) / CLOCKS_PER_SEC
}

/// Make sure libm is really in this process, and keep the linker from deciding
/// otherwise.
///
/// x-lang's conformance suite resolves `sqrt` and `pow` from the SELF handle. The
/// reference engine satisfies that by linking `-lm`, so its process has libm
/// whether or not it calls anything in it; a Rust binary that never references a
/// libm symbol does not, and on Linux — where libm is a separate object, unlike
/// macOS where it is part of libSystem — `dlsym` then answers null and the
/// caller calls through address zero.
///
/// A real call rather than a bare `#[link]` attribute because `--as-needed`
/// drops a library no undefined symbol needs. `ldexp(1.0, 1)` is exactly 2.0 in
/// IEEE-754 with no rounding to argue about, so this is also a cheap assertion
/// that the libm we linked is the one we got.
///
/// It runs when the self handle is asked for, which is the moment the guarantee
/// is needed and the only moment it matters.
fn anchor_libm() -> bool {
    // SAFETY: `ldexp` is a pure function of its arguments and touches no memory.
    unsafe { ldexp(1.0, 1) == 2.0 }
}

/// `(ffi dlopen path flags)` — a handle, or null.
///
/// A nil path is the SELF/GLOBAL handle, which is the form `lib/x/sys/socket.x`
/// and `lib/x/sys/posix.x` both use to reach libc without naming a file.
pub fn open(path: Option<&[u8]>, flags: i32) -> Foreign {
    if path.is_none() {
        // The self handle must be able to see the C runtime's maths, as it can
        // in the reference engine.
        let _ = anchor_libm();
    }
    // SAFETY: the path, when given, is a NUL-terminated byte run we own for the
    // duration of the call; dlopen copies what it needs. A null path is the
    // documented self-handle form.
    let h = unsafe {
        match path {
            Some(p) => dlopen(p.as_ptr() as *const c_char, flags as c_int),
            None => dlopen(std::ptr::null(), flags as c_int),
        }
    };
    Foreign(h as u64)
}

/// `(ffi dlsym lib "name")` — an address, or null when unresolvable.
///
/// The failure mode matters as much as the success: x-lang's library branches on
/// a nil handle in several places rather than guarding every lookup.
pub fn sym(handle: Foreign, name: &[u8]) -> Foreign {
    // SAFETY: `name` is a NUL-terminated run we own for the call. A wrong handle
    // is dlsym's to reject, and it does so by answering null.
    let p = unsafe { dlsym(handle.0 as *mut c_void, name.as_ptr() as *const c_char) };
    Foreign(p as u64)
}

/// Call through an address with integer arguments, answering an integer.
///
/// SEVEN arguments, which is the reference engine's limit and therefore x-lang's
/// — `x_prim_ptr_call` declares `long (*)(long × 7)` and documents that "excess
/// arguments are silently ignored". Matching the number matters: a caller
/// written against the reference and passing seven would lose its last argument
/// here, quietly, and the loss would look like the callee misbehaving.
pub fn call_ints(f: Foreign, args: &[u64]) -> u64 {
    type F7 = extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> u64;
    let mut a = [0u64; 7];
    for (slot, v) in a.iter_mut().zip(args.iter()) {
        *slot = *v;
    }
    // SAFETY: the caller has resolved `f` through dlsym, so it is a function
    // this process can enter. The signature is the C integer convention, which
    // passes its first six arguments in registers on both aarch64 and x86-64:
    // handing a function FEWER arguments than it reads is the caller's error to
    // make, exactly as it is in C, and x-lang's contract calls this door
    // UNCHECKED for that reason.
    let g: F7 = unsafe { std::mem::transmute(f.0 as *const c_void) };
    g(a[0], a[1], a[2], a[3], a[4], a[5], a[6])
}

/// `s0->d` — call a `(const char *, void *) -> double`, answering the bits.
///
/// The shape is `strtod`'s, and `strtod` is what the library calls through it:
/// the second argument is the end pointer, and null says the caller does not
/// want it. A separate spelling rather than a case of [`call_ints`] because the
/// RETURN is a double, and a double comes back in different registers from an
/// integer — reading the wrong ones does not answer a wrong number, it answers
/// whatever was left there.
pub fn call_str_to_double(f: Foreign, s: u64) -> u64 {
    // SAFETY: `s` is a NUL-terminated run the caller owns for the duration, and
    // the null end-pointer is the documented "do not report" form.
    unsafe {
        let g: extern "C" fn(*const c_char, *mut c_void) -> f64 =
            std::mem::transmute(f.0 as *const c_void);
        g(s as *const c_char, std::ptr::null_mut()).to_bits()
    }
}

/// Call a double-taking function, passing IEEE-754 BITS in and out.
///
/// `ptr call` cannot express a double — this engine's fixnum is an integer and
/// there is no float at this level; floats are x-lang's. `ffi call` is the door
/// for conventions that need one, and it solves the representation problem by
/// carrying the bits through integers in both directions, which is what makes it
/// testable from a bare engine.
pub fn call_doubles(f: Foreign, bits: &[u64]) -> u64 {
    // SAFETY: as `call_ints`, with the floating-point convention. The two
    // spellings are separate because the ABI passes doubles in DIFFERENT
    // registers from integers — calling a double function through the integer
    // signature does not answer a wrong number, it reads uninitialised
    // floating-point registers.
    unsafe {
        match bits.len() {
            0 => {
                let g: extern "C" fn() -> f64 = std::mem::transmute(f.0 as *const c_void);
                g().to_bits()
            }
            1 => {
                let g: extern "C" fn(f64) -> f64 = std::mem::transmute(f.0 as *const c_void);
                g(f64::from_bits(bits[0])).to_bits()
            }
            _ => {
                let g: extern "C" fn(f64, f64) -> f64 = std::mem::transmute(f.0 as *const c_void);
                g(f64::from_bits(bits[0]), f64::from_bits(bits[1])).to_bits()
            }
        }
    }
}

// --- interrupts --------------------------------------------------------------

/// Set by the SIGINT handler, read by the evaluator.
///
/// An atomic and nothing else. A signal handler may touch only
/// async-signal-safe state — it interrupts the process at an arbitrary
/// instruction, so anything that could be mid-mutation is off limits, and this
/// engine's heap always could be. Writing the flag OBJECT from the handler would
/// be a data race on the very Vec the interrupted code may be growing.
static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// SIGINT. Two on every platform this engine targets.
const SIGINT: c_int = 2;
/// `SIG_DFL` — the default disposition, which TERMINATES.
const SIG_DFL: usize = 0;

extern "C" fn on_interrupt(_sig: c_int) {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Install the handler: SIGINT sets a flag instead of killing the process.
///
/// This is the engine's half of ctrl-c. With the default disposition SIGINT
/// terminates; with this installed the evaluator can see it and the library's
/// REPL can turn it into a cancelled expression.
pub fn interrupt_install() {
    INTERRUPTED.store(false, std::sync::atomic::Ordering::SeqCst);
    // SAFETY: `signal` with a valid handler address is defined for SIGINT, and
    // the handler itself touches only an atomic.
    unsafe {
        signal(SIGINT, on_interrupt as extern "C" fn(c_int) as usize);
    }
}

/// Restore the default disposition, so the process ends as it began.
pub fn interrupt_restore() {
    // SAFETY: SIG_DFL is always a valid disposition.
    unsafe {
        signal(SIGINT, SIG_DFL);
    }
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Write bytes to a raw file descriptor — the engine's `io write-str` door
/// when the library has redirected the base's out fd. Answers write(2)'s
/// result.
pub fn write_fd(fd: i32, bytes: &[u8]) -> i64 {
    // SAFETY: write(2) validates the descriptor and reports refusal as
    // -errno rather than by faulting; the buffer is a live Rust slice.
    unsafe { write(fd, bytes.as_ptr(), bytes.len()) as i64 }
}

/// Read one byte of PROCESS memory at a real address — the inverse of the
/// marshal door: a C call answered a pointer, and x-lang reads through it.
/// The caller owns the address's validity, exactly as the reference's
/// process-memory reads do.
pub fn read_byte(at: u64) -> u8 {
    unsafe { *(at as *const u8) }
}

/// Read one machine word of process memory at a real address.
pub fn read_word(at: u64) -> u64 {
    unsafe { std::ptr::read_unaligned(at as *const u64) }
}

/// The NUL-terminated bytes at a real address, copied out. A null pointer
/// answers empty.
pub fn c_str_bytes(at: u64) -> Vec<u8> {
    if at == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut p = at;
    loop {
        let b = read_byte(p);
        if b == 0 {
            break;
        }
        out.push(b);
        p += 1;
    }
    out
}

/// Write one byte of process memory at a real address — the inverse door's
/// writer: x-lang fills a region a C call will read (an exec's argv array).
pub fn write_byte(at: u64, v: u8) {
    unsafe { *(at as *mut u8) = v }
}

/// Write one machine word of process memory at a real address.
pub fn write_word(at: u64, v: u64) {
    unsafe { std::ptr::write_unaligned(at as *mut u64, v) }
}

// --- the JIT runtime door -----------------------------------------------------
//
// Machine code the assembler lane emits calls back into the engine through
// nine C-ABI symbols, resolved with `dlsym(dlopen(NULL))` — so the RUNNING
// BINARY must export them. The engine cannot define them (it forbids unsafe,
// and an exported C function has no engine to hold), so the arrangement is
// inverted: the engine implements the safe [`JitHost`] trait and installs
// itself here once; the exported shims below hold the only unsafe part, one
// raw-pointer dereference each.
//
// REENTRANCY, stated plainly: a shim runs while the engine sits suspended
// inside a `ptr call` into the emitted code, so the `&mut` produced here
// aliases the borrow far up the Rust stack. That is the standard FFI-callback
// compromise; it is confined to this file, the engine is single-threaded, and
// no shim call outlives the `ptr call` that triggered it.

/// What the emitted code needs from the engine. Object references cross as
/// REAL machine addresses (the engine's arena is pinned); nil crosses as 0.
pub trait JitHost {
    fn jit_mkint(&mut self, v: i64) -> u64;
    fn jit_mkpair(&mut self, a: u64, b: u64) -> u64;
    fn jit_firstobj(&mut self, p: u64) -> u64;
    fn jit_restobj(&mut self, p: u64) -> u64;
    fn jit_atomint(&mut self, p: u64) -> i64;
    fn jit_eval_arg(&mut self, expr: u64) -> u64;
    fn jit_score_set(&mut self, score: u64, sign: i64, buffer: u64) -> u64;
    fn jit_buffer_unread(&mut self, buffer: u64) -> u64;
    fn jit_buffer_len(&mut self, buffer: u64) -> i64;
}

struct HostPtr(*mut dyn JitHost);
// SAFETY: the engine is single-threaded; the pointer is only ever used from
// the thread that installed it.
unsafe impl Send for HostPtr {}

static JIT_HOST: std::sync::Mutex<Option<HostPtr>> = std::sync::Mutex::new(None);

/// Install the engine as the JIT host. Call once, before any emitted code can
/// run; the engine must outlive every later `ptr call`.
pub fn jit_install(host: *mut dyn JitHost) {
    *JIT_HOST.lock().unwrap() = Some(HostPtr(host));
}

/// The host, reborrowed for one call. The guard is dropped BEFORE the
/// dereference so a host method that re-enters emitted code cannot deadlock.
fn jit_host() -> &'static mut dyn JitHost {
    let p = JIT_HOST
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| h.0)
        .expect("jit: no host installed");
    // SAFETY: see the module note — single-threaded, installed before use,
    // never outliving the engine.
    unsafe { &mut *p }
}

#[no_mangle]
pub extern "C" fn jit_mkint(_base: u64, v: i64) -> u64 {
    jit_host().jit_mkint(v)
}

#[no_mangle]
pub extern "C" fn jit_mkpair(_base: u64, a: u64, b: u64) -> u64 {
    jit_host().jit_mkpair(a, b)
}

#[no_mangle]
pub extern "C" fn jit_firstobj(p: u64) -> u64 {
    jit_host().jit_firstobj(p)
}

#[no_mangle]
pub extern "C" fn jit_restobj(p: u64) -> u64 {
    jit_host().jit_restobj(p)
}

#[no_mangle]
pub extern "C" fn jit_atomint(p: u64) -> i64 {
    jit_host().jit_atomint(p)
}

#[no_mangle]
pub extern "C" fn jit_eval_arg(_base: u64, expr: u64) -> u64 {
    jit_host().jit_eval_arg(expr)
}

#[no_mangle]
pub extern "C" fn jit_score_set(score: u64, sign: i64, buffer: u64) -> u64 {
    jit_host().jit_score_set(score, sign, buffer)
}

#[no_mangle]
pub extern "C" fn jit_buffer_unread(buffer: u64) -> u64 {
    jit_host().jit_buffer_unread(buffer)
}

#[no_mangle]
pub extern "C" fn jit_buffer_len(buffer: u64) -> i64 {
    jit_host().jit_buffer_len(buffer)
}

/// The nine exported addresses, for the binary to anchor so the linker keeps
/// them reachable by `dlsym`.
pub fn jit_exports() -> [usize; 9] {
    [
        jit_mkint as extern "C" fn(u64, i64) -> u64 as usize,
        jit_mkpair as extern "C" fn(u64, u64, u64) -> u64 as usize,
        jit_firstobj as extern "C" fn(u64) -> u64 as usize,
        jit_restobj as extern "C" fn(u64) -> u64 as usize,
        jit_atomint as extern "C" fn(u64) -> i64 as usize,
        jit_eval_arg as extern "C" fn(u64, u64) -> u64 as usize,
        jit_score_set as extern "C" fn(u64, i64, u64) -> u64 as usize,
        jit_buffer_unread as extern "C" fn(u64) -> u64 as usize,
        jit_buffer_len as extern "C" fn(u64) -> i64 as usize,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve or fail LOUDLY. A null answered here used to be called anyway,
    /// and a test that segfaults reports nothing at all — which is how a Linux
    /// runner turned a missing libm into "signal: 11" with no name attached.
    fn must(lib: Foreign, name: &[u8]) -> Foreign {
        let f = sym(lib, name);
        assert!(
            !f.is_null(),
            "could not resolve {}",
            String::from_utf8_lossy(name)
        );
        f
    }

    /// libm the way `lib/x/num/float.x` finds it: by name first, self handle
    /// last. On Linux libm is a separate object and the self handle only sees it
    /// because `anchor_libm` put it there, so this also checks that anchor.
    fn libm() -> Foreign {
        for name in [b"libm.so.6\0".as_ref(), b"libm.dylib\0".as_ref()] {
            let h = open(Some(name), 1);
            if !h.is_null() {
                return h;
            }
        }
        open(None, 1)
    }

    /// The self-handle form the library uses, and a symbol every process has.
    #[test]
    fn the_process_can_open_and_resolve_its_own_symbols() {
        let lib = open(None, 1);
        assert!(!lib.is_null(), "the self handle must resolve");
        assert!(!must(lib, b"strlen\0").is_null());
    }

    /// The failure mode matters as much as the success.
    #[test]
    fn an_unresolvable_symbol_is_null() {
        let lib = open(None, 1);
        assert!(sym(lib, b"no_such_symbol_anywhere_at_all\0").is_null());
    }

    /// The reference passes SEVEN, and a caller that passes seven must not lose
    /// the last one. `snprintf` is variadic, so this reaches past the fixed
    /// arguments and proves the tail arrives.
    #[test]
    fn the_seventh_argument_is_not_dropped() {
        let lib = open(None, 1);
        let f = must(lib, b"strtol\0");
        // (nptr, endptr, base) -- three used, four ignored, and the call must
        // still be well formed.
        let s = b"101\0";
        assert_eq!(call_ints(f, &[s.as_ptr() as u64, 0, 2, 0, 0, 0, 0]), 5);
    }

    /// A double RETURN through the string convention: strtod, which is what the
    /// library calls through this door.
    #[test]
    fn a_string_converts_to_a_double_through_its_own_convention() {
        let lib = open(None, 1);
        let f = must(lib, b"strtod\0");
        let s = b"2.5\0";
        assert_eq!(
            f64::from_bits(call_str_to_double(f, s.as_ptr() as u64)),
            2.5
        );
    }

    /// `strlen` is pure, cannot fail, and its answer is checkable without
    /// trusting anything else.
    #[test]
    fn a_resolved_symbol_can_be_called() {
        let lib = open(None, 1);
        let f = must(lib, b"strlen\0");
        let s = b"hello\0";
        assert_eq!(call_ints(f, &[s.as_ptr() as u64]), 5);
    }

    /// The self handle must see libm, because x-lang's conformance suite asks it
    /// to. On Linux that is true only because `anchor_libm` linked it.
    #[test]
    fn the_self_handle_sees_the_c_runtimes_maths() {
        let lib = open(None, 1);
        assert!(
            !sym(lib, b"sqrt\0").is_null(),
            "the self handle must reach libm -- x-lang's spec resolves sqrt through it"
        );
    }

    /// Doubles travel as bits, and the values are the ones x-lang's own case
    /// uses: sqrt(4.0) = 2.0, pow(3.0, 2.0) = 9.0.
    #[test]
    fn doubles_travel_as_bit_patterns() {
        let lib = libm();
        let sqrt = must(lib, b"sqrt\0");
        assert_eq!(
            call_doubles(sqrt, &[4616189618054758400]),
            4611686018427387904
        );
        let pow = must(lib, b"pow\0");
        assert_eq!(
            call_doubles(pow, &[4613937818241073152, 4611686018427387904]),
            4621256167635550208
        );
    }

    /// The handler sets the flag rather than ending the process. Raising at
    /// ourselves is the only way to observe the difference, and it is safe here
    /// because the disposition is restored before the test returns.
    #[test]
    fn an_installed_handler_sets_a_flag_instead_of_terminating() {
        interrupt_install();
        assert!(!interrupted(), "armed clean");
        let lib = open(None, 1);
        let raise = must(lib, b"raise\0");
        call_ints(raise, &[2]);
        assert!(interrupted(), "the handler ran instead of the default");
        interrupt_restore();
    }

    /// CPU time, which is what the instruction is documented to answer.
    #[test]
    fn the_cpu_clock_does_not_go_backwards() {
        let t0 = cpu_micros();
        let mut acc = 0u64;
        for i in 0..200_000u64 {
            acc = acc.wrapping_add(i);
        }
        assert!(acc > 0);
        assert!(cpu_micros() >= t0);
    }
}
