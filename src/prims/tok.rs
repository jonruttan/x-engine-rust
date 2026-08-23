//! The reader protocol.
//!
//! x-lang's reader is extensible: a type registered on a tokenizer base can
//! claim text and turn it into a value, which is how `lib/x/num/bigint.x` makes
//! a long literal read as a BIGINT with no change to the engine. That
//! extensibility is an engine contract, and this is the engine's half of it.
//!
//! An analyser is a STATE MACHINE WHOSE STATES ARE FUNCTIONS. Called with
//! `(buffer score chr)` it answers the analyser for the next character to
//! continue, `()` to decline, or records a length through the score object to
//! accept.
//!
//! A TOKEN MUST BE DELIMITED. The accept branch runs when a character arrives
//! that the current state rejects, so text ending mid-token is never scored:
//! `"42"` produces nothing where `"42 "` produces a token. x-lang's own note on
//! this says it cost an hour of "the protocol does not work" before the trailing
//! space was added, and an engine that accepted at end-of-input would read a
//! token every reader type in the library is written not to expect.
//!
//! The marks are moved by the tokenizer in an order only the tokenizer knows.
//! That does not put them out of reach — it puts them out of reach FROM OUTSIDE,
//! and a `read` handler runs inside that order.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{Obj, NIL};
use crate::objects::Objects;
use crate::prim::PrimDef;
use crate::vocabulary::Family;

// --- the buffer --------------------------------------------------------------

/// `(buf make s)` — a buffer viewing a string's bytes, non-owning.
fn make(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.buf(a[0], 0))
}

/// `(buf read b)` — the next character, advancing the cursor.
///
/// This is how the tokenizer pulls each character out to feed an analyser:
/// break it and nothing ever reaches a registered type.
fn read(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let b = a[0];
    let text = a_.buf_text(b);
    let at = a_.buf_cursor(b);
    let bytes = a_.bytes_of(text);
    if at as usize >= bytes.len() {
        return Ok(NIL);
    }
    a_.set_buf_cursor(b, at + 1);
    Ok(a_.char_new(bytes[at as usize] as u32))
}

/// `(buf tok b)` — the claimed text.
///
/// NOT a separate accounting of the token: its length is exactly the distance
/// between the retain mark and the cursor, which is what an analyser measures
/// when it scores. An engine where the two could disagree would score one length
/// and hand the reader another.
fn tok(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let b = a[0];
    let text = a_.buf_text(b);
    let (from, to) = (a_.buf_retain(b) as usize, a_.buf_cursor(b) as usize);
    let bytes = a_.bytes_of(text);
    let slice = &bytes[from.min(bytes.len())..to.min(bytes.len())];
    Ok(a_.str_from_bytes(slice))
}

/// `(buf last-char b)` — the character most recently read.
fn last_char(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let b = a[0];
    let text = a_.buf_text(b);
    let at = a_.buf_cursor(b);
    let bytes = a_.bytes_of(text);
    if at == 0 || at as usize > bytes.len() {
        return Ok(NIL);
    }
    Ok(a_.char_new(bytes[at as usize - 1] as u32))
}

/// `(buf retain b)` — the retain mark catches up to the cursor.
///
/// This is what makes each token's text its OWN. Without it the next token's
/// span would still start at the beginning of the input, and the second `"43"`
/// in `"42 43 "` would measure five bytes rather than two.
fn retain(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let at = a_.buf_cursor(a[0]);
    a_.set_buf_retain(a[0], at);
    Ok(a[0])
}

/// `(buf reset b)` — the cursor goes back to the retain mark.
fn reset(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let at = a_.buf_retain(a[0]);
    a_.set_buf_cursor(a[0], at);
    Ok(a[0])
}

/// `(buf append b s)` — extend the text being read.
fn append(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let b = a[0];
    let mut bytes = a_.bytes_of(a_.buf_text(b));
    bytes.extend(a_.bytes_of(a[1]));
    let text = a_.str_from_bytes(&bytes);
    a_.set_data(b, 2, text.word());
    Ok(b)
}

/// `(buf read-text b)` — everything from the retain mark to the end.
fn read_text(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let b = a[0];
    let bytes = a_.bytes_of(a_.buf_text(b));
    let from = (a_.buf_retain(b) as usize).min(bytes.len());
    Ok(a_.str_from_bytes(&bytes[from..]))
}

// --- the tokenizer -----------------------------------------------------------

/// A reader type's handler for one family.
///
/// Read from the TYPE TREE, the same place the library reads `write` and
/// `display` from. The reader's `analyse` and `read` are ordinary families, not
/// a private arrangement, so a type built by `base make-tok` and one built by
/// `type make` carry their handlers identically.
pub(crate) fn handler(e: &mut Engine, ty: Obj, family: Family) -> Obj {
    e.objects.type_handler(ty, family)
}

/// Run one type's analyser from the buffer's current position.
///
/// Answers the length it claims, or `None`. The score object is how acceptance
/// is signalled — the analyser writes a length into it — so it is read after
/// every character rather than inferred from what the analyser returned.
pub(crate) fn score_one(
    e: &mut Engine,
    ty: Obj,
    text: Obj,
    from: u64,
) -> Result<Option<u64>, Cond> {
    let analyse = handler(e, ty, Family::Analyse);
    if analyse.is_nil() {
        return Ok(None);
    }
    // The slot may hold ONE handler or a LIST of them, and the list is how
    // x-lang installs reader macros: lib/x/reader/lit-reader.x pushes
    // `(interp lit quasi unquote <the engine's own symbol analyser>)` onto the
    // symbol type, with the engine's handler captured as the catch-all TAIL.
    // Walking only a lone handler leaves `'x` reading as a symbol named `'x`.
    //
    // Order is the library's and it means something — the catch-all is last on
    // purpose — so the FIRST handler that scores wins rather than the longest.
    for h in handler_list(e, analyse) {
        // The captured tail may be nil — lib/x/reader/lit-reader.x ends its list
        // with the engine's own analyser, and an engine that had none there
        // contributes nothing rather than a call through nil.
        if h.is_nil() {
            continue;
        }
        if let Some(n) = score_with(e, h, text, from)? {
            return Ok(Some(n));
        }
    }
    Ok(None)
}

/// The handlers in a slot: a list walked directly, a lone handler on its own.
///
/// The reference wraps the single case so its walk stays uniform, and says why:
/// it lets the quote and quasiquote readers live on the symbol type beside the
/// symbol reader.
pub(crate) fn handler_list(e: &Engine, slot: Obj) -> Vec<Obj> {
    if e.objects.is_cell(slot) {
        e.objects.list(slot).collect()
    } else {
        vec![slot]
    }
}

/// Run ONE analyser state machine from `from`, answering the length it claims.
fn score_with(e: &mut Engine, analyse: Obj, text: Obj, from: u64) -> Result<Option<u64>, Cond> {
    let buf = e.objects.buf(text, from);
    let score = e.objects.int(0);
    let mut state = analyse;
    let env = e.root_env();

    loop {
        let chr = read(&mut e.objects, &[buf])?;
        if chr.is_nil() {
            // END OF INPUT. No accept branch runs, so nothing is scored — a
            // token must be delimited.
            return Ok(None);
        }
        let next = e.call_with_values(state, &[buf, score, chr], env)?;
        let claimed = e.objects.as_int(score);
        if claimed != 0 {
            // THE LENGTH IS WHAT WAS CONSUMED, and the score only carries its
            // SIGN. The reference computes `(score < 0 ? -1 : 1) * consumed`,
            // and consumed is the buffer's own span — retain to cursor, after
            // any `%buffer-unread` the acceptor performed.
            //
            // Taking the score's MAGNITUDE as the length happens to agree for an
            // acceptor that sets it from `%buffer-len` — `%lit-accept` does — and
            // is wrong for one that sets a bare sign.
            //
            // This is closer to the reference and it is NOT yet enough:
            // `$"a{1}b"` still reads with nil parts, so something else about
            // that analyser's drive is wrong too. Recorded rather than claimed.
            let consumed = e
                .objects
                .buf_cursor(buf)
                .saturating_sub(e.objects.buf_retain(buf));
            return Ok(Some(consumed.max(1)));
        }
        if !e.objects.truthy(next) {
            return Ok(None);
        }
        state = next;
    }
}

/// `(tok read-str TB text)` — drive every registered type over the text, score
/// them against each other, and answer the LIST of tokens produced.
fn read_str(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let text = a[1];
    let len = e.objects.bytes_of(text).len() as u64;
    // THE FIRST ARGUMENT IS A BASE, not a token base. x-lang calls this as
    // `(tok read-str (%base) text)` — lib/x/reader/lit-reader.x's `chunk` does,
    // to re-read an interpolation's literal piece through the ordinary string
    // reader — so the types to drive are the BASE'S TYPE-ALIST, the same ones
    // the reader consults.
    //
    // Reading them from a token base found nothing at all: every chunk of a
    // `$"…"` came back nil, so the literal built a `(Str8 str …)` whose pieces
    // were all nil and evaluated to nothing. The banner said `helium()`.
    //
    // A TOKBASE drives the scorer over the types registered in it. That is the
    // protocol x-lang's conformance suite exercises, delimiting included: an
    // undelimited `"42"` must yield no tokens at all.
    if !e.objects.is_tokbase(a[0]) {
        // A BASE reads the text as the engine reads any other source.
        //
        // It cannot go through the scorer here, and the reason is this engine's
        // deviation rather than the caller's mistake: the reference expresses its
        // BUILT-IN syntax as analyse/read handlers on the builtin types, so
        // scoring plain text finds them. This engine keeps that syntax in Rust
        // (see crate::form), so the scorer would find nothing registered and
        // answer no tokens -- which is exactly what happened to every chunk of a
        // `$"…"` literal.
        let text = e.objects.str_val(text);
        let mut r = crate::read::Reader::new(&text);
        let mut forms: Vec<Obj> = Vec::new();
        while let Some(f) = e.read_form_from(&mut r)? {
            forms.push(f);
        }
        let mut list = NIL;
        for &f in forms.iter().rev() {
            list = e.objects.pair(f, list);
        }
        return Ok(list);
    }
    let types: Vec<Obj> = e.objects.list(e.objects.tokbase_types(a[0])).collect();
    let env = e.root_env();

    let mut tokens: Vec<Obj> = Vec::new();
    let mut at = 0u64;
    while at < len {
        // EVERY type is tried at this position and the longest claim wins.
        // Taking the first that matches would make registration order decide
        // the language, which is the distinction between a scorer and a search.
        let mut best: Option<(u64, Obj)> = None;
        for &ty in &types {
            if let Some(n) = score_one(e, ty, text, at)? {
                // `map_or`, not `is_none_or`: the latter is stable since 1.82 and
                // this crate's declared MSRV is 1.78. Raising the floor for one
                // call would be the tail wagging the dog.
                if n > 0 && best.map_or(true, |(b, _)| n > b) {
                    best = Some((n, ty));
                }
            }
        }
        let Some((n, ty)) = best else { break };

        // The winner's `read` runs against a buffer positioned on exactly the
        // span it claimed: retain at the start, cursor at the end.
        let buf = e.objects.buf(text, at);
        e.objects.set_buf_cursor(buf, at + n);
        let reader = handler(e, ty, Family::Read);
        // As with the analysers, the slot may be a LIST. A reader DECLINES by
        // answering nil without consuming, so the next one sees the same buffer
        // — which is why each attempt gets a buffer positioned identically
        // rather than one carried over from a reader that already looked.
        let mut token = NIL;
        if reader.is_nil() {
            token = tok(&mut e.objects, &[buf])?;
        } else {
            for r in handler_list(e, reader) {
                let fresh = e.objects.buf(text, at);
                e.objects.set_buf_cursor(fresh, at + n);
                let got = e.call_with_values(r, &[fresh], env)?;
                if !got.is_nil() {
                    token = got;
                    break;
                }
            }
        }
        tokens.push(token);
        at += n;
    }

    let mut list = NIL;
    for &t in tokens.iter().rev() {
        list = e.objects.pair(t, list);
    }
    Ok(list)
}

/// `(tok read TB b)` — the same drive, over a buffer already positioned.
/// `(tok read buffer)` — ONE FORM from the buffer, leaving its cursor after what
/// was read.
///
/// ONE argument, the buffer. It was declared as taking two, `(tok read TB
/// buffer)`, and read the second — so `lib/x/reader/lit-reader.x`'s
/// `(%token-read buffer)` handed it a buffer it ignored and a nil it used, and
/// `'x` came out as `(lit ())`.
///
/// This is what a reader macro calls to read the form it prefixes: `%lit-read`
/// answers `(lit X)` by reading X through here. It used to re-tokenize the
/// buffer's whole text and answer a LIST of every token in it, which is a
/// different instruction entirely.
fn read_tok(e: &mut Engine, a: &[Obj]) -> EvalResult {
    e.read_form_at(a[0])
}

// --- registration ------------------------------------------------------------

/// `(base make-tok)` — a base with NO types registered.
fn make_tok(a_: &mut Objects, _a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.tokbase())
}

/// `(base make-type TB "NAME" handlers)` — register a reader type.
fn make_type(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let ty = a_.type_new(a[1], a[2]);
    a_.tokbase_add(a[0], ty);
    Ok(ty)
}

pub const TABLE: &[PrimDef] = &[
    PrimDef::filed("buf", "make", 1, make),
    PrimDef::filed("buf", "read", 1, read),
    PrimDef::filed("buf", "tok", 1, tok),
    PrimDef::filed("buf", "last-char", 1, last_char),
    PrimDef::filed("buf", "retain", 1, retain),
    PrimDef::filed("buf", "reset", 1, reset),
    PrimDef::filed("buf", "append", 2, append),
    PrimDef::filed("buf", "read-text", 1, read_text),
    PrimDef::both_full("token-read-string", "tok", "read-str", 2, read_str),
    PrimDef::filed_full("tok", "read", 1, read_tok),
    PrimDef::filed("base", "make-tok", 0, make_tok),
    PrimDef::filed("base", "make-type", 3, make_type),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::Objects;

    /// The marks the language reads reflectively and the ones Rust reads must be
    /// the same two numbers. They were not: this is the check that says so
    /// directly, without going through the reader protocol to find out.
    #[test]
    fn the_buffer_marks_are_where_the_contract_says() {
        let mut a = Objects::new();
        let text = a.str_new("42 ");
        let b = a.buf(text, 0);
        assert_eq!(a.buf_retain(b), 0);
        assert_eq!(a.buf_cursor(b), 0);
        a.set_buf_cursor(b, 2);
        assert_eq!(a.buf_cursor(b), 2, "the cursor cell must round-trip");
        assert_eq!(a.buf_retain(b), 0, "and must not disturb the retain mark");
    }

    #[test]
    fn tok_is_the_span_between_the_marks() {
        let mut a = Objects::new();
        let text = a.str_new("42 ");
        let b = a.buf(text, 0);
        a.set_buf_cursor(b, 2);
        let t = tok(&mut a, &[b]).expect("tok");
        assert_eq!(a.str_val(t), "42");
        assert_eq!(a.byte_len(t), 2);
    }

    /// THE DELIMITER RULE, which is the part of the protocol most easily got
    /// wrong: accepting at end-of-input reads a token where the language expects
    /// none, and every reader type in the library is written to the behaviour
    /// asserted here.
    #[test]
    fn a_token_must_be_delimited() {
        // The data offset is DERIVED, not written in. It is
        // `%obj-meta-len * word-size` and the engine's own contract says what
        // that is; a literal 16 here was right only while the header was two
        // words, and adding the collector's chain link made it silently read
        // the flags word instead.
        let off = crate::objects::META_LEN * 8;
        let p = P.replace("{OFF}", &off.to_string());
        const P: &str = r#"
            (def %digit? (fn (_ c) (match ((< c 48) ()) ((< 57 c) ()) (#t 1))))
            (def %o2p (%coord (lit obj) (lit ->ptr)))
            (def %refw (%coord (lit ptr) (lit ref-word)))
            (def %setw (%coord (lit ptr) (lit set-word!)))
            (def %cellint (fn (_ x) (%refw (%o2p x) {OFF})))
            (def %setcell (fn (_ p v) (%setw (%o2p p) {OFF} v) p))
            (def %buflen (fn (_ b) (- (%cellint (rest b)) (%cellint b))))
            (def %unread (fn (_ b) (%setcell (rest b) (- (%cellint (rest b)) 1))))
            (def %scoreset (fn (_ s b) (%setcell s (%buflen b))))
            (def %digits ())
            (set! %digits (fn (_ b s c)
              (match ((%digit? c) %digits) (#t (%seq (%unread b) (%scoreset s b))))))
            (def %an (fn (_ b s c) (match ((%digit? c) %digits) (#t ()))))
            (def tb ((%coord (lit base) (lit make-tok))))
            ((%coord (lit base) (lit make-type)) tb "N"
              (pair (pair (lit analyse) %an)
                    (pair (pair (lit read) (fn (_ . args) 7)) ())))
            (def %rs (%coord (lit tok) (lit read-str)))
        "#;
        assert_eq!(
            crate::testkit::int_of(&format!("{} (first (%rs tb \"42 \"))", p)),
            7,
            "a delimited token is read"
        );
        assert!(
            crate::testkit::truthy(&format!("{} (eq? (%rs tb \"42\") ())", p)),
            "an undelimited one is not"
        );
    }

    #[test]
    fn last_char_is_the_one_just_before_the_cursor() {
        let mut a = Objects::new();
        let text = a.str_new("42 ");
        let b = a.buf(text, 0);
        a.set_buf_cursor(b, 2);
        let c = last_char(&mut a, &[b]).expect("last-char");
        assert_eq!(a.as_char(c), b'2' as u32);
    }
}
