//! Syntactic forms.
//!
//! Mirrors the reference engine's `src/x-syntax/`: binding, closure, control and
//! quotation, one file each. These are the forms whose whole purpose is to decide
//! what gets evaluated, which is why they are operatives and why they sit apart
//! from the instruction namespaces in `crate::prims`.
//!
//! The boundary is x-engine-c's. It was one 538-line file here until an engine
//! that had already made this split was read instead of guessed at.

pub mod binding;
pub mod closure;
pub mod control;
pub mod quote;
