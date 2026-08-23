//! The object kinds.
//!
//! Each file is one kind and its own `impl Objects` block. `Objects` stays a single
//! type — objects share a header, an allocator and a heap, so splitting the
//! TYPE would mean threading three references through every constructor — but it
//! stopped being a single file, because pairs, strings, continuations and
//! tokenizer buffers have nothing to do with one another.
//!
//! It had grown to 834 lines and 81 methods, which is how it got here: the objects
//! was split once into storage and interning, and then every new object kind was
//! appended to whatever remained.

pub mod callable;
pub mod num;
pub mod pair;
pub mod ptr;
pub mod text;
pub mod tok;
pub mod typed;
