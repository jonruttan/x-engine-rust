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
//!
//! The reader has NO state of its own: it walks a BUFFER — `(val . (read .
//! write))` over a text string — and the buffer it walks is the head of the
//! base's `buffer` row, as the reference reads through
//! `x_base_field_buffer(p_base)`. Reading advances the buffer's read mark, so
//! `io read-char` and a reader macro see exactly what is left after the current
//! form.

use crate::obj::{Obj, NIL};
use crate::objects::Objects;

impl Objects {
    /// The byte at the buffer's read mark, unconsumed.
    pub(crate) fn buf_peek(&mut self, b: Obj) -> Option<u8> {
        self.buf_byte_ahead(b, 0)
    }

    /// The byte `n` ahead of the read mark, for the one-byte lookahead `#\`
    /// needs. An exhausted INTERACTIVE source refills before answering
    /// end-of-input.
    pub(crate) fn buf_byte_ahead(&mut self, b: Obj, n: u64) -> Option<u8> {
        loop {
            let i = self.buf_cursor(b) + n;
            if i < self.buf_write(b) {
                let text = self.buf_text(b);
                return Some(self.heap.byte(self.str_bytes(text).plus(i)));
            }
            if !self.buf_refill(b) {
                return None;
            }
        }
    }

    /// One byte from the engine's input stream, appended at the write mark —
    /// the extension path of the reference's `x_type_buffer_read`, with its
    /// EOF latch: end of input flips the filein head to the fd's bitwise
    /// complement, and every later read fails the latch check without
    /// another syscall. The complement, not a flat -1, so the fd stays
    /// recoverable. Answers whether a byte arrived.
    fn buf_refill(&mut self, b: Obj) -> bool {
        if self.buf_ro(b) {
            return false;
        }
        let base = self.base;
        if base.is_nil() {
            return false;
        }
        let row = crate::base::get(self, base, crate::base::FILEIN);
        if row.is_nil() {
            return false;
        }
        let fd_obj = self.first(row);
        if fd_obj.is_nil() || self.as_int(fd_obj) < 0 {
            return false;
        }
        let Some(input) = self.input.as_mut() else {
            return false;
        };
        let mut byte = [0u8; 1];
        let got = matches!(input.read(&mut byte), Ok(1));
        if !got {
            // EOF, or a read error the stream cannot continue past: latch.
            let fd = self.as_int(fd_obj);
            self.set_data(fd_obj, 0, crate::obj::Word((!fd) as u64));
            return false;
        }
        let mut w = self.buf_write(b);
        if w >= self.input_cap {
            // The region is full: compact the unread remainder to the front,
            // as `buf retain` does, and give up only when a single span
            // fills the whole region.
            let c = self.buf_cursor(b);
            if c == 0 {
                return false;
            }
            let text = self.buf_text(b);
            let at = self.str_bytes(text);
            for i in c..w {
                let v = self.heap.byte(at.plus(i));
                self.heap.set_byte(at.plus(i - c), v);
            }
            self.set_buf_retain(b, 0);
            self.set_buf_cursor(b, 0);
            self.buf_line_shift(b, c);
            w -= c;
            self.set_buf_write(b, w);
        }
        let text = self.buf_text(b);
        self.heap.set_byte(self.str_bytes(text).plus(w), byte[0]);
        self.set_buf_write(b, w + 1);
        true
    }

    /// One byte, consumed. `None` at end of input — which is how `io read-char`
    /// tells exhaustion from a NUL byte it legitimately read.
    pub fn buf_next_byte(&mut self, b: Obj) -> Option<u8> {
        let c = self.buf_peek(b)?;
        self.buf_bump(b);
        Some(c)
    }

    pub(crate) fn buf_bump(&mut self, b: Obj) {
        let i = self.buf_cursor(b);
        self.set_buf_cursor(b, i + 1);
    }

    /// The bytes between two marks, copied out for a name or literal.
    fn buf_slice(&self, b: Obj, start: u64, end: u64) -> Vec<u8> {
        let text = self.buf_text(b);
        let at = self.str_bytes(text);
        (start..end).map(|i| self.heap.byte(at.plus(i))).collect()
    }

    /// Refill an interactive source until its region holds a newline at or
    /// past the read mark, or the stream ends. A no-op for a read-only view.
    pub(crate) fn buf_prefetch_line(&mut self, b: Obj) {
        if self.buf_ro(b) {
            return;
        }
        loop {
            let c = self.buf_cursor(b);
            let w = self.buf_write(b);
            let text = self.buf_text(b);
            let at = self.str_bytes(text);
            let mut seen = false;
            for i in c..w {
                if self.heap.byte(at.plus(i)) == b'\n' {
                    seen = true;
                    break;
                }
            }
            if seen || !self.buf_refill(b) {
                return;
            }
        }
    }

    pub(crate) fn buf_skip_blanks(&mut self, b: Obj) {
        loop {
            match self.buf_peek(b) {
                Some(c) if c.is_ascii_whitespace() => self.buf_bump(b),
                Some(b';') => {
                    while let Some(c) = self.buf_next_byte(b) {
                        if c == b'\n' {
                            break;
                        }
                    }
                }
                _ => return,
            }
        }
    }

    /// One form of BUILT-IN syntax, EXCEPT a list.
    ///
    /// Lists are the form reader's, not this one's, because every element is a
    /// position where a macro may begin — `(def q 'str)` is the ordinary case —
    /// and a list read here would read its elements with no macro in the loop.
    ///
    /// It does NOT skip blanks: the caller has already done that, and may have
    /// offered the position to a macro first.
    /// The non-atom builtin cases; `None` means "a list or an atom starts
    /// here", which the caller reads with its own machinery.
    pub(crate) fn buf_read_one_builtin_except_atom(&mut self, b: Obj) -> Option<Obj> {
        let c = self.buf_peek(b)?;
        match c {
            b'(' => None,
            b')' => {
                self.buf_bump(b);
                Some(NIL)
            }
            b'"' => {
                self.buf_bump(b);
                Some(self.buf_read_string(b))
            }
            b'#' if self.buf_byte_ahead(b, 1) == Some(b'\\') => {
                self.buf_bump(b);
                self.buf_bump(b);
                Some(self.buf_read_char(b))
            }
            _ => None,
        }
    }

    /// The atom between two marks, as `buf_read_atom` builds one.
    pub(crate) fn buf_atom_from(&mut self, b: Obj, start: u64, end: u64) -> Obj {
        let bytes = self.buf_slice(b, start, end);
        let text = String::from_utf8_lossy(&bytes).to_string();
        if text.is_empty() {
            self.buf_bump(b);
            return NIL;
        }
        if let Ok(v) = text.parse::<i64>() {
            self.int(v)
        } else {
            self.sym(&text)
        }
    }

    /// Is the byte at the read mark a lone `.` acting as a tail separator?
    pub(crate) fn buf_at_dot_separator(&mut self, b: Obj) -> bool {
        self.buf_peek(b) == Some(b'.') && self.buf_dot_is_a_separator(b)
    }

    /// Read one form. `None` at end of input.
    pub fn buf_read_form(&mut self, b: Obj) -> Option<Obj> {
        self.buf_skip_blanks(b);
        let c = self.buf_peek(b)?;
        match c {
            b'(' => {
                self.buf_bump(b);
                Some(self.buf_read_list(b))
            }
            b')' => {
                // A stray close paren: consume it so the loop makes progress
                // rather than spinning, and answer nil.
                self.buf_bump(b);
                Some(NIL)
            }
            b'"' => {
                self.buf_bump(b);
                Some(self.buf_read_string(b))
            }
            b'#' if self.buf_byte_ahead(b, 1) == Some(b'\\') => {
                self.buf_bump(b);
                self.buf_bump(b);
                Some(self.buf_read_char(b))
            }
            _ => Some(self.buf_read_atom(b)),
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
    pub(crate) fn buf_read_list(&mut self, b: Obj) -> Obj {
        // Collect then build right-to-left: a list is a spine of pairs ending in
        // its tail, and building it backwards avoids walking to the end per
        // element.
        let mut items: Vec<Obj> = Vec::new();
        let mut tail = NIL;
        loop {
            self.buf_skip_blanks(b);
            match self.buf_peek(b) {
                None => break,
                Some(b')') => {
                    self.buf_bump(b);
                    break;
                }
                // A lone `.` between forms marks the tail. `.` is only special
                // when it stands alone: `.5` and `foo.bar` are ordinary atoms.
                // With no elements before it, the list IS its tail: `( . x)`
                // reads as the bare form x.
                Some(b'.') if self.buf_dot_is_a_separator(b) => {
                    self.buf_bump(b);
                    self.buf_skip_blanks(b);
                    if let Some(t) = self.buf_read_form(b) {
                        tail = t;
                    }
                    self.buf_skip_blanks(b);
                    if self.buf_peek(b) == Some(b')') {
                        self.buf_bump(b);
                    }
                    break;
                }
                _ => match self.buf_read_form(b) {
                    Some(o) => items.push(o),
                    None => break,
                },
            }
        }
        let mut out = tail;
        for &o in items.iter().rev() {
            out = self.pair(o, out);
        }
        out
    }

    /// Is the `.` at the read mark a tail marker rather than part of an atom?
    fn buf_dot_is_a_separator(&mut self, b: Obj) -> bool {
        match self.buf_byte_ahead(b, 1) {
            None => true,
            Some(c) => c.is_ascii_whitespace() || c == b'(' || c == b')',
        }
    }

    /// A string literal is BYTES; the accumulator must be too — pushing
    /// `u8 as char` onto a `String` is Latin-1 promotion and re-encodes every
    /// byte above 0x7F. Escapes are the reference's set (`\" \\ n t r 0`,
    /// `\xNN` with exactly two hex digits); an UNKNOWN escape keeps the
    /// backslash AND the character.
    pub(crate) fn buf_read_string(&mut self, b: Obj) -> Obj {
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(c) = self.buf_next_byte(b) {
            match c {
                b'"' => break,
                b'\\' => {
                    let Some(e) = self.buf_next_byte(b) else {
                        break;
                    };
                    match e {
                        b'"' => bytes.push(b'"'),
                        b'\\' => bytes.push(b'\\'),
                        b'n' => bytes.push(b'\n'),
                        b't' => bytes.push(b'\t'),
                        b'r' => bytes.push(b'\r'),
                        b'0' => bytes.push(0),
                        b'x' => {
                            let h = self.buf_peek(b).and_then(hex_digit);
                            let l = self.buf_byte_ahead(b, 1).and_then(hex_digit);
                            match (h, l) {
                                (Some(h), Some(l)) => {
                                    bytes.push(h * 16 + l);
                                    self.buf_bump(b);
                                    self.buf_bump(b);
                                }
                                _ => {
                                    bytes.push(b'\\');
                                    bytes.push(b'x');
                                }
                            }
                        }
                        other => {
                            bytes.push(b'\\');
                            bytes.push(other);
                        }
                    }
                }
                other => bytes.push(other),
            }
        }
        self.str_from_bytes(&bytes)
    }

    /// A character literal, with `#\` already consumed.
    ///
    /// ENGINE SYNTAX, not a library reader macro. docs/syntax.md's dialect
    /// matrix puts `#\` chars in the `bare x` column and says the bare column
    /// "is normative for every implementation of the reader" — so an engine that
    /// leaves this to the library is not reading x-lang. Without it `#\A` reads
    /// as a SYMBOL and the failure surfaces far away, as `Unbound SYMBOL '#\A`.
    ///
    /// Three forms, following the reference's reader:
    ///   * a UTF-8 multi-byte character, taken whole;
    ///   * a single byte — ANY byte, including `(` and `\`;
    ///   * a NAME, but only where the first byte is a letter, because a
    ///     non-letter scores immediately. That is what makes `#\(` and `#\;`
    ///     readable at all.
    pub(crate) fn buf_read_char(&mut self, b: Obj) -> Obj {
        let Some(first) = self.buf_peek(b) else {
            // `#\` at end of input: nothing to name a character with.
            return NIL;
        };
        self.buf_bump(b);

        if first >= 0x80 {
            // A multi-byte character: take its continuation bytes too.
            let start = self.buf_cursor(b) - 1;
            while let Some(c) = self.buf_peek(b) {
                if c & 0xc0 != 0x80 {
                    break;
                }
                self.buf_bump(b);
            }
            let bytes = self.buf_slice(b, start, self.buf_cursor(b));
            let text = String::from_utf8_lossy(&bytes).to_string();
            let cp = text.chars().next().map(|c| c as u32).unwrap_or(0);
            return self.char_new(cp);
        }

        if !first.is_ascii_alphabetic() {
            // Scores immediately, so `#\(` is the open paren and not a list.
            return self.char_new(first as u32);
        }

        // A letter: gather the rest of the name. One letter alone is the
        // character itself.
        let start = self.buf_cursor(b) - 1;
        while let Some(c) = self.buf_peek(b) {
            if !c.is_ascii_alphabetic() {
                break;
            }
            self.buf_bump(b);
        }
        let bytes = self.buf_slice(b, start, self.buf_cursor(b));
        let name = String::from_utf8_lossy(&bytes).to_string();
        if name.len() == 1 {
            return self.char_new(first as u32);
        }
        for (n, cp) in crate::vocabulary::CHAR_NAMES {
            if *n == name {
                return self.char_new(*cp);
            }
        }
        // An unknown name is the reference's error. This engine has no channel
        // to raise from inside the reader, so it answers the leading letter and
        // leaves the name — which a caller sees as a wrong character rather than
        // silence.
        self.set_buf_cursor(b, start + 1);
        self.char_new(first as u32)
    }

    pub(crate) fn buf_read_atom(&mut self, b: Obj) -> Obj {
        let start = self.buf_cursor(b);
        while let Some(c) = self.buf_peek(b) {
            if c.is_ascii_whitespace() || c == b'(' || c == b')' || c == b';' {
                break;
            }
            self.buf_bump(b);
        }
        let bytes = self.buf_slice(b, start, self.buf_cursor(b));
        let text = String::from_utf8_lossy(&bytes).to_string();
        if text.is_empty() {
            self.buf_bump(b);
            return NIL;
        }
        // An integer if it parses as one; a symbol otherwise. `-` alone is a
        // symbol, not a malformed number.
        if let Ok(v) = text.parse::<i64>() {
            self.int(v)
        } else {
            self.sym(&text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::Objects;

    fn rbuf(o: &mut Objects, src: &str) -> Obj {
        let t = o.str_new(src);
        o.buf(t, 0)
    }

    fn read_one(src: &str) -> (Objects, Obj) {
        let mut o = Objects::new();
        let b = rbuf(&mut o, src);
        let form = o.buf_read_form(b).expect("a form");
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
        let b = rbuf(&mut o, "(1 2");
        assert!(o.buf_read_form(b).is_some());
        assert!(o.buf_read_form(b).is_none(), "and then stops");
        let mut o = Objects::new();
        let b = rbuf(&mut o, r#""unclosed"#);
        let _ = o.buf_read_form(b);
    }

    /// A stray close paren is consumed so the loop makes progress. Spinning on
    /// it would hang the engine on malformed input.
    #[test]
    fn a_stray_close_paren_does_not_spin() {
        let mut o = Objects::new();
        let b = rbuf(&mut o, ")))42");
        for _ in 0..4 {
            let _ = o.buf_read_form(b);
        }
        assert!(o.buf_read_form(b).is_none());
    }

    #[test]
    fn reading_consumes_forms_in_order() {
        let mut o = Objects::new();
        let b = rbuf(&mut o, "1 2 3");
        let mut seen = Vec::new();
        while let Some(f) = o.buf_read_form(b) {
            seen.push(o.as_int(f));
        }
        assert_eq!(seen, vec![1, 2, 3]);
    }

    /// ENGINE SYNTAX per docs/syntax.md's dialect matrix, where `#\` chars sit
    /// in the `bare x` column and the bare column "is normative for every
    /// implementation of the reader".
    #[test]
    fn a_character_literal_reads_as_a_character_not_a_symbol() {
        let (o, v) = read_one(r"#\A");
        assert!(o.is_char(v), "#\\A must read as a character");
        assert_eq!(o.as_char(v), 65);
    }

    /// The nine names are the reference's, and `lib/` writes them.
    #[test]
    fn the_named_characters_are_the_references_nine() {
        for (src, want) in [
            (r"#\newline", 10u32),
            (r"#\space", 32),
            (r"#\tab", 9),
            (r"#\return", 13),
            (r"#\null", 0),
            (r"#\alarm", 7),
            (r"#\backspace", 8),
            (r"#\delete", 127),
            (r"#\escape", 27),
        ] {
            let (o, v) = read_one(src);
            assert!(o.is_char(v), "{} must read as a character", src);
            assert_eq!(o.as_char(v), want, "for {}", src);
        }
    }

    /// A NON-LETTER scores immediately, which is what makes the delimiters
    /// readable at all: `#\(` is an open paren, not the start of a list.
    #[test]
    fn a_non_letter_scores_immediately() {
        for (src, want) in [(r"#\(", 40u32), (r"#\)", 41), (r"#\;", 59), (r"#\ ", 32)] {
            let (o, v) = read_one(src);
            assert!(o.is_char(v), "{} must read as a character", src);
            assert_eq!(o.as_char(v), want, "for {}", src);
        }
    }

    /// A single letter is the letter, even though letters otherwise gather into
    /// a name.
    #[test]
    fn a_lone_letter_is_itself_and_a_name_needs_more_than_one() {
        let (o, v) = read_one(r"#\n");
        assert_eq!(o.as_char(v), b'n' as u32, "#\\n is the letter, not newline");
    }

    /// Multi-byte characters travel whole rather than as their lead byte.
    #[test]
    fn a_multibyte_character_is_taken_whole() {
        let (o, v) = read_one("#\\\u{e9}");
        assert!(o.is_char(v));
        assert_eq!(o.as_char(v), 0xe9);
    }

    /// And it still DELIMITS: a character in a list does not swallow the rest.
    #[test]
    fn a_character_delimits_inside_a_list() {
        let (o, v) = read_one(r"(#\A 1)");
        let items: Vec<_> = o.list(v).collect();
        assert_eq!(items.len(), 2, "the character must not swallow the list");
        assert_eq!(o.as_char(items[0]), 65);
        assert_eq!(o.int_val(items[1]), 1);
    }
}

#[cfg(test)]
mod string_tests {
    use crate::testkit::eval;

    /// A literal is BYTES. Pushing through `u8 as char` is Latin-1 promotion —
    /// 0xC2 becomes U+00C2 and re-encodes as C3 82 — so every multi-byte
    /// source literal doubled. `"¢"` is the two bytes C2 A2 and must stay them.
    #[test]
    fn a_multibyte_literal_keeps_its_bytes() {
        let (e, v) = eval("\"¢\"");
        assert_eq!(e.objects.bytes_of(v.unwrap()), vec![0xC2, 0xA2]);
    }

    /// The reference's escape set, including the rule that an UNKNOWN escape
    /// keeps the backslash AND the character.
    #[test]
    fn escapes_decode_as_the_reference_decodes_them() {
        let (e, v) = eval(r#""a\x41\r\q""#);
        assert_eq!(e.objects.bytes_of(v.unwrap()), b"aA\r\\q".to_vec());
    }
}

/// A hex digit's value, or None — the reference's hex_digit.
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
