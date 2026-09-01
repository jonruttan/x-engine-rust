//! An x-lang engine, in Rust.
//!
//! The core of this crate `forbid`s unsafe code — not "safe Rust" loosely, but
//! the level an inner `allow` cannot override. That is why the foreign door is
//! the separate [`foreign`] crate rather than a module here.
//!
//! [x-lang](https://github.com/jonruttan/x-lang) is a language, not an
//! implementation. `x-engine-c` is one engine; this is another, written to the
//! published contract rather than by reading the C and copying it.
//!
//! # Embedding
//!
//! ```
//! use x_engine::engine::Engine;
//!
//! let mut e = Engine::new();
//! let v = e.eval_str("(+ 2 3)").unwrap();
//! assert_eq!(e.objects.as_int(v), 5);
//! ```
//!
//! The engine is a library first and a binary second. `src/main.rs` is a thin
//! wrapper that reads a program from stdin — contract layer E — and everything
//! it does is available to a caller that would rather drive the engine directly.
//!
//! # Sandboxing
//!
//! `base make` answers another interpreter context, not another environment. A
//! child base is born with the instruction set and nothing of the host's, and a
//! host hands capabilities in one name at a time.
//!
//! ```
//! use x_engine::engine::Engine;
//!
//! let mut e = Engine::new();
//! let child = e.make_base();
//! let env = e.base_env(child);
//!
//! // The machine is there.
//! let v = e.eval_str("(+ 2 3)").unwrap();
//! assert_eq!(e.objects.as_int(v), 5);
//!
//! // The host's definitions are not.
//! e.eval_str("(def host-only 99)").unwrap();
//! let name = e.objects.sym("host-only");
//! assert!(e.envs.lookup(&e.objects, env, name).is_none());
//!
//! // Until one is handed over.
//! let answer = e.objects.sym("answer");
//! let v = e.objects.int(42);
//! e.envs.bind(&mut e.objects, env, answer, v);
//! assert_eq!(e.envs.lookup(&e.objects, env, answer), Some(v));
//! ```
//!
//! # The layers
//!
//! An engine is a MACHINE. It reads the word at a slot and applies an operator.
//! It does not type-check, it does not count arguments, and it has no opinion
//! about dividing by zero — those are x-lang's, one layer up.
//!
//! ```
//! use x_engine::engine::Engine;
//!
//! let mut e = Engine::new();
//! // A symbol operand is READ, not refused. x-engine-c does the same.
//! assert!(e.eval_str("(+ 1 (lit a))").is_ok());
//! // Dividing by zero answers zero rather than trapping.
//! let v = e.eval_str("(/ 1 0)").unwrap();
//! assert_eq!(e.objects.as_int(v), 0);
//! ```
//!
//! The modules follow that same layering:
//!
//! | module | what it knows |
//! |---|---|
//! | [`foreign`] | the door out: dlopen, syscalls, signals — the ONLY unsafe code, and its own crate |
//! | [`dbl`] | doubles as bit patterns — this engine's number is an integer |
//! | [`heap`] | words and bytes — the storage x-lang's `heap/*` instructions name |
//! | [`objects`] | the object model over the heap — mirrors the C engine's `x-obj.c` |
//! | [`symbols`] | name to object, and nothing else |
//! | [`base`] | the execution context, reachable by reflection |
//! | [`env`] | frame chains, and rootless frames for sandboxes |
//! | [`prim`] | an instruction as data, graded by what it may reach |
//! | [`eval`] | the evaluator |
//! | [`diag`] | conditions, and how they are shown |

/// The foreign door, re-exported under the name the engine calls it by.
///
/// A SEPARATE CRATE, because this one forbids unsafe code and `forbid` is the
/// level an inner `allow` cannot override. Everything that reaches outside the
/// process lives there; everything here is safe Rust the compiler enforces as
/// such.
pub use x_engine_foreign as foreign;

pub mod base;
pub mod collect;
pub mod dbl;
pub mod diag;
pub mod engine;
pub mod env;
pub mod eval;
pub mod form;
pub mod heap;
pub mod jit;
pub mod obj;
pub mod objects;
pub mod prim;
pub mod prims;
pub mod read;
pub mod session;
pub mod symbols;
pub mod syntax;
pub mod value;
pub mod vocabulary;

#[cfg(test)]
mod testkit;
