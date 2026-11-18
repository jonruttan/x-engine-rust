//! Leaving machine code at a raise, as x-engine-c's longjmp does.
//!
//! Compiled code reaches the engine through C-ABI doors. On the reference a
//! raise inside one longjmps to the guard and the compiled frames above it
//! never run again. Here a raise is a value: the door holds it on the engine
//! and answers 0, and compiled code that went on would read through that 0 --
//! `(obj-ref v (+ (raises) 1))` is a SIGSEGV.
//!
//! So every entry into machine code is an ESCAPE FRAME. [`enter`] records the
//! callee-saved registers, the stack pointer and the return address, then
//! calls the routine. A door that has just held a raise calls [`escape`],
//! which restores the innermost frame and returns 0 from its [`enter`]. The
//! frames skipped are the compiled code's and the door's own, and none of
//! them owns anything to drop. The engine then raises the held condition from
//! the entry, exactly where it already did.
//!
//! On a target with no escape routine, [`enter`] calls straight through and
//! [`escape`] does nothing: the door answers 0, as before.

use std::sync::atomic::{AtomicPtr, Ordering};

/// One entry's saved machine state, and the entry it nests in.
#[repr(C)]
pub struct Frame {
    regs: [u64; 22],
    prev: *mut Frame,
}

impl Frame {
    fn new() -> Self {
        Frame {
            regs: [0; 22],
            prev: std::ptr::null_mut(),
        }
    }
}

/// The innermost live entry. The engine is single-threaded; atomic only so
/// the static needs no lock on a path every compiled call takes.
static TOP: AtomicPtr<Frame> = AtomicPtr::new(std::ptr::null_mut());

#[cfg(target_vendor = "apple")]
macro_rules! sym {
    ($n:literal) => {
        concat!("_", $n)
    };
}
#[cfg(not(target_vendor = "apple"))]
macro_rules! sym {
    ($n:literal) => {
        $n
    };
}

// x_escape_enter(f, args, frame): save x19-x28, fp, lr, sp, d8-d15 into
// frame, call f with eight integer arguments from args, return its x0.
// x_escape_jump(frame): restore them and return 0 from that x_escape_enter.
#[cfg(target_arch = "aarch64")]
std::arch::global_asm!(
    ".text",
    ".p2align 2",
    concat!(".globl ", sym!("x_escape_enter")),
    concat!(sym!("x_escape_enter"), ":"),
    "stp x19, x20, [x2, #0]",
    "stp x21, x22, [x2, #16]",
    "stp x23, x24, [x2, #32]",
    "stp x25, x26, [x2, #48]",
    "stp x27, x28, [x2, #64]",
    "stp x29, x30, [x2, #80]",
    "mov x9, sp",
    "str x9, [x2, #96]",
    "stp d8, d9, [x2, #104]",
    "stp d10, d11, [x2, #120]",
    "stp d12, d13, [x2, #136]",
    "stp d14, d15, [x2, #152]",
    "stp x29, x30, [sp, #-16]!",
    "mov x29, sp",
    "mov x16, x0",
    "mov x9, x1",
    "ldp x0, x1, [x9, #0]",
    "ldp x2, x3, [x9, #16]",
    "ldp x4, x5, [x9, #32]",
    "ldp x6, x7, [x9, #48]",
    "blr x16",
    "ldp x29, x30, [sp], #16",
    "ret",
    ".p2align 2",
    concat!(".globl ", sym!("x_escape_jump")),
    concat!(sym!("x_escape_jump"), ":"),
    "ldp x19, x20, [x0, #0]",
    "ldp x21, x22, [x0, #16]",
    "ldp x23, x24, [x0, #32]",
    "ldp x25, x26, [x0, #48]",
    "ldp x27, x28, [x0, #64]",
    "ldp x29, x30, [x0, #80]",
    "ldr x9, [x0, #96]",
    "mov sp, x9",
    "ldp d8, d9, [x0, #104]",
    "ldp d10, d11, [x0, #120]",
    "ldp d12, d13, [x0, #136]",
    "ldp d14, d15, [x0, #152]",
    "mov x0, #0",
    "ret",
);

// x_escape_enter(f, args, frame): save rbx, rbp, r12-r15, the caller's stack
// pointer and the return address into frame; call f with eight integer
// arguments from args, the seventh and eighth on the stack; return its rax.
// x_escape_jump(frame): restore them and return 0 from that x_escape_enter.
#[cfg(target_arch = "x86_64")]
std::arch::global_asm!(
    ".text",
    concat!(".globl ", sym!("x_escape_enter")),
    concat!(sym!("x_escape_enter"), ":"),
    "mov [rdx + 0], rbx",
    "mov [rdx + 8], rbp",
    "mov [rdx + 16], r12",
    "mov [rdx + 24], r13",
    "mov [rdx + 32], r14",
    "mov [rdx + 40], r15",
    "lea rcx, [rsp + 8]",
    "mov [rdx + 48], rcx",
    "mov rcx, [rsp]",
    "mov [rdx + 56], rcx",
    "push rbp",
    "mov rbp, rsp",
    "mov r10, rdi",
    "mov r11, rsi",
    "sub rsp, 16",
    "mov rax, [r11 + 48]",
    "mov [rsp], rax",
    "mov rax, [r11 + 56]",
    "mov [rsp + 8], rax",
    "mov rdi, [r11 + 0]",
    "mov rsi, [r11 + 8]",
    "mov rdx, [r11 + 16]",
    "mov rcx, [r11 + 24]",
    "mov r8, [r11 + 32]",
    "mov r9, [r11 + 40]",
    "call r10",
    "leave",
    "ret",
    concat!(".globl ", sym!("x_escape_jump")),
    concat!(sym!("x_escape_jump"), ":"),
    "mov rbx, [rdi + 0]",
    "mov rbp, [rdi + 8]",
    "mov r12, [rdi + 16]",
    "mov r13, [rdi + 24]",
    "mov r14, [rdi + 32]",
    "mov r15, [rdi + 40]",
    "mov rsp, [rdi + 48]",
    "xor eax, eax",
    "jmp qword ptr [rdi + 56]",
);

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
extern "C" {
    fn x_escape_enter(f: u64, args: *const u64, frame: *mut Frame) -> u64;
    fn x_escape_jump(frame: *mut Frame) -> !;
}

/// Call `f` with eight integer arguments as an escape frame: a door's
/// [`escape`] while it runs returns 0 from here.
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
pub fn enter(f: u64, args: &[u64; 8]) -> u64 {
    let mut frame = Frame::new();
    frame.prev = TOP.load(Ordering::Relaxed);
    TOP.store(&mut frame, Ordering::Relaxed);
    // SAFETY: `f` is an address the caller vouches is a function taking up
    // to eight integer arguments, as `call_ints` always has. The frame lives
    // on this stack for the whole call, and is unlinked below on both ways
    // back: the routine's return and an escape, which lands here returning 0.
    let out = unsafe { x_escape_enter(f, args.as_ptr(), &mut frame) };
    TOP.store(frame.prev, Ordering::Relaxed);
    out
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub fn enter(f: u64, args: &[u64; 8]) -> u64 {
    type F8 = extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> u64;
    // SAFETY: as `call_ints`: the caller vouches for `f`.
    let g: F8 = unsafe { std::mem::transmute(f as *const std::ffi::c_void) };
    g(
        args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7],
    )
}

/// Leave the machine code a door was entered from: return 0 from the
/// innermost [`enter`]. Does nothing when no entry is live (the engine called
/// the door itself) or on a target with no escape routine.
pub fn escape() {
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    {
        let top = TOP.load(Ordering::Relaxed);
        if !top.is_null() {
            // SAFETY: `top` is the frame of a live `enter` further down this
            // stack: entries unlink their frame before returning. Every frame
            // between here and there is a door's or compiled code's, and none
            // owns a value to drop.
            unsafe { x_escape_jump(top) }
        }
    }
}
