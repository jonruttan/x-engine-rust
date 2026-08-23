//! The reader.
//!
//! A minimal s-expression reader: lists, integers, strings, symbols, and `;`
//! comments. It is deliberately NOT the engine's final reader — x-lang's real
//! one is extensible, and `tok/read-str` drives registered types through an
//! analyser-and-score protocol (x-lang's `tests/x/conformance/core/reader.spec.md`
//! defines it). That protocol needs the type registry, which does not exist yet.
//!
//! What this gives is the observation channel: enough to read `(error "ok")` so
//! the engine can be watched at all. Every conformance case depends on that and
//! nothing else can be checked until it works.

use crate::obj::{Obj, NIL};
use crate::objects::Objects;

/// The reader OWNS its bytes rather than borrowing them, so that the engine can
/// hold one and the `io` instructions can read from the same stream the program
/// arrived on. That is not a convenience: the program comes in on stdin, so
/// whatever `io read-char` should return is by definition what is left in this
/// buffer after the current form. A reader borrowing a slice main had slurped
/// could not be reached from inside a primitive.
pub struct Reader {
    src: Vec<u8>,
    pos: usize,
}

impl Reader {
    pub fn new(src: &str) -> Self {
        Reader {
            src: src.as_bytes().to_vec(),
            pos: 0,
        }
    }

    /// One byte, consumed. `None` at end of input — which is how `io read-char`
    /// tells exhaustion from a NUL byte it legitimately read.
    pub fn next_byte(&mut self) -> Option<u8> {
        let c = self.src.get(self.pos).copied()?;
        self.pos += 1;
        Some(c)
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_blanks(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => self.pos += 1,
                Some(b';') => {
                    while let Some(c) = self.peek() {
                        self.pos += 1;
                        if c == b'\n' {
                            break;
                        }
                    }
                }
                _ => return,
            }
        }
    }

    /// Read one form. `None` at end of input.
    pub fn read(&mut self, a: &mut Objects) -> Option<Obj> {
        self.skip_blanks();
        let c = self.peek()?;
        match c {
            b'(' => {
                self.pos += 1;
                Some(self.read_list(a))
            }
            b')' => {
                // A stray close paren: consume it so the loop makes progress
                // rather than spinning, and answer nil.
                self.pos += 1;
                Some(NIL)
            }
            b'"' => {
                self.pos += 1;
                Some(self.read_string(a))
            }
            _ => Some(self.read_atom(a)),
        }
    }

    /// A list, which may be IMPROPER: `(a . b)` puts `b` in the tail rather
    /// than making a third element.
    ///
    /// Without this a dotted parameter list reads as three names — `_`, `.` and
    /// `args` — and a rest parameter silently binds nothing while the dot binds
    /// the argument. Every `read` handler in x-lang's reader protocol is written
    /// `(fn (_ . args) ...)`, so the whole protocol fails on a reader that
    /// treats the dot as an atom, and it fails by producing nil rather than by
    /// complaining.
    fn read_list(&mut self, a: &mut Objects) -> Obj {
        // Collect then build right-to-left: a list is a spine of pairs ending in
        // its tail, and building it backwards avoids walking to the end per
        // element.
        let mut items: Vec<Obj> = Vec::new();
        let mut tail = NIL;
        loop {
            self.skip_blanks();
            match self.peek() {
                None => break,
                Some(b')') => {
                    self.pos += 1;
                    break;
                }
                // A lone `.` between forms marks the tail. `.` is only special
                // when it stands alone: `.5` and `foo.bar` are ordinary atoms.
                Some(b'.') if self.dot_is_a_separator() && !items.is_empty() => {
                    self.pos += 1;
                    self.skip_blanks();
                    if let Some(t) = self.read(a) {
                        tail = t;
                    }
                    self.skip_blanks();
                    if self.peek() == Some(b')') {
                        self.pos += 1;
                    }
                    break;
                }
                _ => match self.read(a) {
                    Some(o) => items.push(o),
                    None => break,
                },
            }
        }
        let mut out = tail;
        for &o in items.iter().rev() {
            out = a.pair(o, out);
        }
        out
    }

    /// Is the `.` at the cursor a tail marker rather than part of an atom?
    fn dot_is_a_separator(&self) -> bool {
        match self.src.get(self.pos + 1) {
            None => true,
            Some(c) => c.is_ascii_whitespace() || *c == b'(' || *c == b')',
        }
    }

    fn read_string(&mut self, a: &mut Objects) -> Obj {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    if let Some(e) = self.peek() {
                        self.pos += 1;
                        s.push(match e {
                            b'n' => '\n',
                            b't' => '\t',
                            other => other as char,
                        });
                    }
                }
                other => s.push(other as char),
            }
        }
        a.str_new(&s)
    }

    fn read_atom(&mut self, a: &mut Objects) -> Obj {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || c == b'(' || c == b')' || c == b';' {
                break;
            }
            self.pos += 1;
        }
        let text = String::from_utf8_lossy(&self.src[start..self.pos]).to_string();
        if text.is_empty() {
            self.pos += 1;
            return NIL;
        }
        // An integer if it parses as one; a symbol otherwise. `-` alone is a
        // symbol, not a malformed number.
        if let Ok(v) = text.parse::<i64>() {
            a.int(v)
        } else {
            a.sym(&text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::Objects;

    fn read_one(src: &str) -> (Objects, Obj) {
        let mut o = Objects::new();
        let mut r = Reader::new(src);
        let form = Reader::read(&mut r, &mut o).expect("a form");
        (o, form)
    }

    #[test]
    fn an_integer_reads_as_an_integer_and_a_name_as_a_symbol() {
        let (o, v) = read_one("42");
        assert_eq!(o.as_int(v), 42);
        let (o, v) = read_one("-7");
        assert_eq!(o.as_int(v), -7);
        let (o, v) = read_one("alpha");
        assert!(o.is_sym(v));
    }

    /// `-` alone is a NAME, not a malformed number. x-lang binds it as an
    /// instruction, so a reader that tried to parse it as an integer would make
    /// subtraction unspellable.
    #[test]
    fn a_lone_minus_is_a_symbol() {
        let (o, v) = read_one("-");
        assert!(o.is_sym(v));
        assert_eq!(o.sym_name(v), "-");
    }

    #[test]
    fn a_list_reads_as_a_spine() {
        let (o, v) = read_one("(1 2 3)");
        let items: Vec<Obj> = o.list(v).collect();
        assert_eq!(items.len(), 3);
        assert_eq!(o.as_int(items[2]), 3);
    }

    #[test]
    fn the_empty_list_is_nil() {
        let (_, v) = read_one("()");
        assert!(v.is_nil(), "() is nil is NULL -- one value");
    }

    /// THE DOTTED PAIR. `(fn (_ . args) ...)` is how every `read` handler in
    /// x-lang's reader protocol is written, and a reader without this makes it
    /// three names — `_`, `.` and `args` — so the dot binds the argument and the
    /// rest parameter binds nothing. It fails by producing nil, not by
    /// complaining, which is how it went unnoticed.
    #[test]
    fn a_dotted_tail_is_the_rest_not_an_element() {
        let (o, v) = read_one("(a . b)");
        assert!(o.is_sym(o.first(v)), "the head is a symbol");
        assert!(
            o.is_sym(o.rest(v)),
            "and the TAIL is the symbol, not a pair"
        );
        assert_eq!(o.sym_name(o.rest(v)), "b");
    }

    #[test]
    fn a_dotted_tail_after_several_elements() {
        let (o, v) = read_one("(a b . c)");
        let tail = o.rest(o.rest(v));
        assert!(o.is_sym(tail));
        assert_eq!(o.sym_name(tail), "c");
    }

    /// `.` is special only when it STANDS ALONE. A reader that treated every
    /// leading dot as a tail marker would break decimals and dotted names.
    #[test]
    fn a_dot_inside_an_atom_is_not_a_separator() {
        let (o, v) = read_one("(a .5 b)");
        let items: Vec<Obj> = o.list(v).collect();
        assert_eq!(items.len(), 3, ".5 is an atom, not a tail marker");
        let (o, v) = read_one("foo.bar");
        assert_eq!(o.sym_name(v), "foo.bar");
    }

    #[test]
    fn strings_read_with_their_escapes() {
        let (o, v) = read_one(r#""a\nb""#);
        assert_eq!(o.str_val(v), "a\nb");
        let (o, v) = read_one(r#""""#);
        assert_eq!(o.str_val(v), "");
    }

    #[test]
    fn comments_run_to_end_of_line() {
        let (o, v) = read_one("; ignored\n42");
        assert_eq!(o.as_int(v), 42);
        let (o, v) = read_one("(1 ; two\n 3)");
        let items: Vec<Obj> = o.list(v).collect();
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn nesting_reads_to_the_right_depth() {
        let (o, v) = read_one("(1 (2 (3)))");
        let inner = o.first(o.rest(v));
        let innermost = o.first(o.rest(inner));
        assert_eq!(o.as_int(o.first(innermost)), 3);
    }

    /// Unterminated input ENDS rather than looping or panicking: the loop that
    /// drives this reads until `None`, so a reader that never returned would
    /// hang the engine on a truncated file.
    #[test]
    fn unterminated_input_terminates() {
        let mut o = Objects::new();
        let mut r = Reader::new("(1 2");
        assert!(Reader::read(&mut r, &mut o).is_some());
        assert!(Reader::read(&mut r, &mut o).is_none(), "and then stops");
        let mut o = Objects::new();
        let mut r = Reader::new(r#""unclosed"#);
        let _ = Reader::read(&mut r, &mut o);
    }

    /// A stray close paren is consumed so the loop makes progress. Spinning on
    /// it would hang the engine on malformed input.
    #[test]
    fn a_stray_close_paren_does_not_spin() {
        let mut o = Objects::new();
        let mut r = Reader::new(")))42");
        for _ in 0..4 {
            let _ = Reader::read(&mut r, &mut o);
        }
        assert!(Reader::read(&mut r, &mut o).is_none());
    }

    #[test]
    fn reading_consumes_forms_in_order() {
        let mut o = Objects::new();
        let mut r = Reader::new("1 2 3");
        let mut seen = Vec::new();
        while let Some(f) = Reader::read(&mut r, &mut o) {
            seen.push(o.as_int(f));
        }
        assert_eq!(seen, vec![1, 2, 3]);
    }
}
