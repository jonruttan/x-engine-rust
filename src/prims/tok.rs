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
use crate::obj::{EnvId, Obj, NIL};
use crate::objects::Objects;
use crate::prim::PrimDef;
use crate::vocabulary::Family;

// --- the buffer --------------------------------------------------------------

/// `(buf make s)` — a buffer viewing a string's bytes, non-owning.
fn make(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    // A made buffer is EMPTY whatever its region holds: the region is capacity,
    // not content, and `str make`'s space fill must never read back as input.
    Ok(a_.buf_writable(a[0], 0, 0))
}

/// `(buf read b)` — the next character, advancing the cursor.
///
/// This is how the tokenizer pulls each character out to feed an analyser:
/// break it and nothing ever reaches a registered type.
fn read(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let b = a[0];
    let text = a_.buf_text(b);
    let at = a_.buf_cursor(b);
    // Bounded by the WRITE mark, not the region: unwritten capacity is not
    // input. x_buffereof is `read >= write`.
    if at >= a_.buf_write(b) {
        return Ok(NIL);
    }
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
    // The CODE, not a character — the reference answers "Integer character
    // code", and the spec asserts 105 for #\i.
    Ok(a_.int(bytes[at as usize - 1] as i64))
}

/// `(buf retain b)` — the retain mark catches up to the cursor.
///
/// This is what makes each token's text its OWN. Without it the next token's
/// span would still start at the beginning of the input, and the second `"43"`
/// in `"42 43 "` would measure five bytes rather than two.
fn retain(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let b = a[0];
    let at = a_.buf_cursor(b);
    if a_.buf_ro(b) {
        // The tokenizer's case: a mark bump, never a copy (#354).
        a_.set_buf_retain(b, at);
        return Ok(b);
    }
    // A WRITABLE buffer compacts: the unread remainder moves to the front of
    // the region, so the tail capacity is writable again. The spec observes the
    // backing string directly — after reading one of "abc", byte 0 is 'b'.
    let text = a_.buf_text(b);
    let base = a_.str_bytes(text);
    let w = a_.buf_write(b);
    let mut i = 0u64;
    while at + i < w {
        let c = a_.heap.byte(base.plus(at + i));
        a_.heap.set_byte(base.plus(i), c);
        i += 1;
    }
    a_.set_buf_retain(b, 0);
    a_.set_buf_cursor(b, 0);
    a_.set_buf_write(b, w - at);
    Ok(b)
}

/// `(buf reset b)` — the cursor goes back to the retain mark.
fn reset(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let at = a_.buf_retain(a[0]);
    a_.set_buf_cursor(a[0], at);
    Ok(a[0])
}

/// `(buf append b s)` — extend the text being read.
/// `(buf append b ch)` — ONE CHARACTER, written at the write mark INTO the
/// region. The region is shared state: writing in place is what makes the byte
/// visible to every view of it. Writes past the region's capacity are clamped.
fn append(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let b = a[0];
    let text = a_.buf_text(b);
    let w = a_.buf_write(b);
    // Clamp at the region's capacity rather than write past it.
    if (w as usize) < a_.byte_len(text) {
        let at = a_.str_bytes(text);
        let ch = a_.as_char(a[1]) as u8;
        a_.heap.set_byte(at.plus(w), ch);
        a_.set_buf_write(b, w + 1);
    }
    Ok(b)
}

/// `(buf read-text b)` — read ONE character; nil at end of input OR on a NUL,
/// which is `x_type_buffer_read` plus the NUL test.
fn read_text(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let b = a[0];
    let got = read(a_, a)?;
    if got.is_nil() {
        return Ok(NIL);
    }
    if a_.as_char(got) == 0 {
        return Ok(NIL);
    }
    Ok(b)
}

// --- the tokenizer -----------------------------------------------------------

/// A reader type's handler for one family.
///
/// Read from the TYPE TYPE, the same place the library reads `write` and
/// `display` from. The reader's `analyse` and `read` are ordinary families, not
/// a private arrangement, so a type built by `base make-tok` and one built by
/// `type make` carry their handlers identically.
pub(crate) fn handler(e: &mut Engine, ty: Obj, family: Family) -> Obj {
    e.objects.type_handler(ty, family)
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

/// Does `score` beat `best`, by the reference's comparison?
///
/// `x_token_analyse` writes it as
/// `score >= i_best || (i_best < 1 && score <= i_best)`, with `i_best` starting
/// at zero. Two things fall out of that shape and both matter: `>=` means a
/// LATER type takes a tie, which is how the library's ordering settles
/// equal-length claims; and while nothing positive has matched, a MORE negative
/// score wins — the symbol fallback scores negative so any positive match beats
/// it (sexp/symbol.c).
pub(crate) fn better(score: i64, best: Option<i64>) -> bool {
    match best {
        None => true,
        Some(b) => score >= b || (b < 1 && score <= b),
    }
}

/// THE ANALYSE CONTEST: which registered type claims the text at `at`, and how
/// much of it — `x_token_analyse`, shared by both drives.
///
/// ONE BUFFER for the whole contest, rewound to the token start between
/// attempts, so an analyser's side effects — `%buffer-unread` backing off a
/// delimiter it peeked at, the retain mark, `last-char` — accumulate the way
/// the library expects.
///
/// A handler's attempt ends one of two ways, measured differently:
///
///   * it RETURNS THE SCORE OBJECT — an accept; the claim is the score's own
///     value, which `%score-set` filled from the buffer span. Setting the
///     score is NOT an accept: `%float-first-frac` sets it on the first
///     fractional digit and returns the next state.
///   * it runs out of input with a score already set — the EOF auto-score,
///     `sign(score) * consumed`, which claims an undelimited token at the end
///     of a source.
///
/// A handler answering nil rewinds first, so it claims nothing. `better`
/// decides the winner; `>=` means a later type takes a tie, so registration
/// order settles equal-length claims while LENGTH settles unequal ones.
/// `env` is the CALLER's environment, captured before any base swap: it only
/// serves `call_with_values`' argument quoting, whose `(lit v)` heads must
/// resolve — a bare token base binds no instruction names at all.
pub(crate) fn analyse(
    e: &mut Engine,
    types: &[Obj],
    buf: Obj,
    env: EnvId,
) -> Result<Option<(Obj, i64)>, Cond> {
    let mark = e.root_mark();
    e.root_push(buf);
    let score = e.objects.int(0);
    e.root_push(score);
    let state_slot = e.root_mark();
    e.root_push(NIL);

    let rewind = |e: &mut Engine| {
        let start = e.objects.buf_retain(buf);
        e.objects.set_buf_cursor(buf, start);
    };

    let mut best: Option<i64> = None;
    let mut winner = NIL;

    for &ty in types {
        if ty.is_nil() {
            continue;
        }
        let slot = handler(e, ty, Family::Analyse);
        if slot.is_nil() {
            continue;
        }
        for h in handler_list(e, slot) {
            // The captured tail may be nil — lib/x/reader/lit-reader.x ends its
            // list with the engine's own analyser.
            if h.is_nil() {
                continue;
            }
            e.objects.set_data(score, 0, crate::obj::Word::from_i64(0));
            let mut state = h;
            e.roots[state_slot] = state;

            let accepted = loop {
                let chr = match read(&mut e.objects, &[buf]) {
                    Ok(v) => v,
                    Err(c) => {
                        e.root_truncate(mark);
                        return Err(c);
                    }
                };
                // End of input: break WITHOUT rewinding, so the auto-score below
                // still sees the span.
                if chr.is_nil() {
                    break None;
                }
                let next = match e.call_with_values(state, &[buf, score, chr], env) {
                    Ok(v) => v,
                    Err(c) => {
                        e.root_truncate(mark);
                        return Err(c);
                    }
                };
                if next.is_nil() {
                    rewind(e);
                    break None;
                }
                if next == buf {
                    continue;
                }
                if next == score {
                    break Some(e.objects.as_int(score));
                }
                state = next;
                e.roots[state_slot] = state;
            };

            let claim = match accepted {
                Some(n) if n != 0 => Some(n),
                Some(_) => None,
                None => {
                    let consumed = e
                        .objects
                        .buf_cursor(buf)
                        .saturating_sub(e.objects.buf_retain(buf))
                        as i64;
                    let scored = e.objects.as_int(score);
                    if consumed > 0 && scored != 0 {
                        Some(if scored < 0 { -consumed } else { consumed })
                    } else {
                        None
                    }
                }
            };
            if let Some(n) = claim {
                if better(n, best) {
                    best = Some(n);
                    winner = ty;
                }
            }
            rewind(e);
        }
    }

    e.root_truncate(mark);
    Ok(best.map(|n| (winner, n)))
}

/// `(tok read-str TB text)` — drive every registered type over the text, score
/// them against each other, and answer the LIST of tokens produced.
fn read_str(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let text = a[1];
    let gmark = e.root_mark();
    e.root_push(text);
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
    // THE FIRST ARGUMENT'S TYPE-ALIST IS THE TYPE LIST. For a token base that
    // is its own list; for a REAL base it is the base's type-alist — the same
    // registry `make-instance` and `type ?` read, which is what lets an app
    // register a language on a child base and have every consumer agree
    // (apps/logo). The whole drive runs IN that base, so a read handler's
    // `%make-instance` resolves its handle where the type was filed.
    //
    // Types with no analyse handler cost one nil check, exactly as in the form
    // reader. When NO type claims a position, a real base falls back to the
    // engine's own reader for ONE form — this engine keeps its built-in syntax
    // in Rust where the reference expresses it as handlers on the builtin
    // types, so the fallback is the same implicit tail the reference gets by
    // construction. That keeps `$"…"` interpolation working (a plain host base
    // claims nothing and reads forms) while a registered language wins wherever
    // its analysers claim.
    // A token base and an interpreter base differ by CONTENTS, not kind. The
    // engine-reader fallback below stands in for the built-in types the
    // reference files on every full base's alist, so it applies exactly where
    // those types would be: a base with a catalog. A `base make-tok` base has
    // none, and an input nothing claims yields no tokens there.
    let falls_back = !crate::base::catalog_of(&e.objects, a[0]).is_nil();
    let alist = crate::base::get(&e.objects, a[0], crate::base::TYPE_ALIST);
    let entries: Vec<Obj> = e.objects.list(alist).collect();
    let types: Vec<Obj> = entries.iter().map(|&entry| e.objects.rest(entry)).collect();

    // The registered types are held in a Rust Vec for the whole drive, and the
    // handlers they carry are x-lang code that can collect.
    for t in &types {
        e.root_push(*t);
    }
    let target = a[0];
    let env = e.root_env();
    let mut tokens: Vec<Obj> = Vec::new();
    // ONE buffer for the whole drive, as `x_prim_token_read_string` builds one:
    // a read handler may consume PAST its own span — a block reader reads its
    // nested tokens recursively — and the drive must continue from wherever the
    // reads left the cursor. A per-token buffer clipped to the claimed span cut
    // logo's `[` reader off from its block's contents.
    let buf = e.objects.buf(text, 0);
    e.root_push(buf);
    loop {
        let at = e.objects.buf_retain(buf);
        if at >= len {
            break;
        }
        // The same contest the form reader runs. See `analyse`. The contest
        // rewinds the cursor to the retain mark between handlers, so the
        // buffer comes back positioned where it started.
        let claim = e.in_base(target, |e| analyse(e, &types, buf, env))?;
        let Some((ty, claim)) = claim else {
            if falls_back {
                // No registered type claims here: the engine's own reader takes
                // one form, or the input is done.
                let scratch = e.objects.buf(text, at);
                e.root_push(scratch);
                let form = e.in_base(target, |e| e.read_form_in(scratch))?;
                let Some(form) = form else { break };
                // A nil token has always STOPPED read-str — the drop-
                // unterminated-tail contract's other half.
                if form.is_nil() {
                    break;
                }
                let pos = e.objects.buf_cursor(scratch);
                // An atom running to the very end of the input has no
                // delimiter to finish it: it is TRUNCATED, and read-str's
                // contract drops it silently, as the reference's analyse
                // protocol does when end of input arrives mid-token.
                if pos >= len {
                    let last = e.objects.buf_text(scratch);
                    let tail = e.objects.heap.byte(e.objects.str_bytes(last).plus(pos - 1));
                    let delimited = tail.is_ascii_whitespace() || tail == b')' || tail == b';';
                    if !delimited {
                        break;
                    }
                }
                e.root_push(form);
                tokens.push(form);
                e.objects.set_buf_retain(buf, pos);
                e.objects.set_buf_cursor(buf, pos);
                continue;
            }
            break;
        };
        // The sign ordered the contest; the span is the magnitude.
        let n = claim.unsigned_abs();
        if n == 0 {
            break;
        }

        // The winner's `read` runs with the cursor at the end of the claimed
        // span and the retain mark at its start — and may keep reading.
        e.objects.set_buf_cursor(buf, at + n);
        let rmark = e.root_mark();
        e.root_push(ty);
        let reader = handler(e, ty, Family::Read);
        e.root_push(reader);
        // A type with NO read handler DISCARDS its span — whitespace, comments —
        // and the drive fetches another token, as `x_token_read` does.
        if reader.is_nil() {
            e.root_truncate(rmark);
            e.objects.set_buf_retain(buf, at + n);
            continue;
        }
        // The slot may be a LIST. A reader DECLINES by answering nil without
        // consuming, so each attempt starts from the same position.
        let mut token = NIL;
        for r in handler_list(e, reader) {
            e.objects.set_buf_cursor(buf, at + n);
            let fmark = e.root_mark();
            e.root_push(r);
            let got = e.in_base(target, |e| e.call_with_values(r, &[buf], env))?;
            e.root_truncate(fmark);
            if !got.is_nil() {
                token = got;
                break;
            }
        }
        e.root_truncate(rmark);
        // Every reader declined: the drive stops, dropping the unterminated
        // tail — the contract `x_prim_token_read_string` states and keeps.
        if token.is_nil() {
            break;
        }
        e.root_push(token);
        tokens.push(token);
        // Everything the read consumed is done with: retain to the cursor, as
        // `x_token_read` retains after each delivered token.
        let end = e.objects.buf_cursor(buf);
        e.objects.set_buf_retain(buf, end.max(at + n));
        // A read that walked the cursor BACKWARD cannot be allowed to wedge
        // the drive; the claimed span is the floor.
        if e.objects.buf_retain(buf) <= at {
            break;
        }
    }

    let mut list = NIL;
    for &t in tokens.iter().rev() {
        list = e.objects.pair(t, list);
        e.root_push(list);
    }
    e.root_truncate(gmark);
    Ok(list)
}

/// `(tok read buffer)` — ONE FORM from the buffer, leaving its cursor after
/// what was read. ONE argument, the buffer.
///
/// This is what a reader macro calls to read the form it prefixes: `%lit-read`
/// answers `(lit X)` by reading X through here.
fn read_tok(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    e.read_form_at(a[0])
}

// --- the engine's integer token type -----------------------------------------
// TRANSCRIBED from the reference's `x-token/sexp/int.c`: five analyser states
// (sign, prefix, base, digits, xdigits) and a reader, installed on every
// base's INTEGER type. The states answer SELF while consuming; on the first
// non-member character they unread it and accept with a POSITIVE score of the
// span — deterministic, as the sexp analysers are — or decline when nothing
// was consumed. State indexes into `Objects::int_states`.

pub(crate) const ST_SIGN: usize = 0;
const ST_PREFIX: usize = 1;
const ST_BASE: usize = 2;
const ST_DIGITS: usize = 3;
const ST_XDIGITS: usize = 4;

fn buf_span(a_: &Objects, b: Obj) -> u64 {
    a_.buf_cursor(b).saturating_sub(a_.buf_retain(b))
}

fn unread(a_: &mut Objects, b: Obj) {
    let c = a_.buf_cursor(b);
    a_.set_buf_cursor(b, c.saturating_sub(1));
}

/// Accept: unread the delimiter, score the span, answer the SCORE object —
/// or decline when the span is empty.
fn int_accept(a_: &mut Objects, a: &[Obj]) -> Obj {
    let (b, score) = (a[0], a[1]);
    unread(a_, b);
    let n = buf_span(a_, b);
    if n < 1 {
        return NIL;
    }
    a_.set_data(score, 0, crate::obj::Word::from_i64(n as i64));
    score
}

fn chr_of(a_: &Objects, a: &[Obj]) -> u32 {
    a_.as_char(a[2])
}

fn int_digits(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    if chr_of(a_, a).is_ascii_digit_u32() {
        return Ok(a_.int_states[ST_DIGITS]);
    }
    Ok(int_accept(a_, a))
}

fn int_xdigits(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let c = chr_of(a_, a);
    if c.is_ascii_digit_u32() || (0x61..=0x66).contains(&c) || (0x41..=0x46).contains(&c) {
        return Ok(a_.int_states[ST_XDIGITS]);
    }
    Ok(int_accept(a_, a))
}

fn int_base(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let c = chr_of(a_, a);
    if c == b'x' as u32 || c == b'X' as u32 {
        return Ok(a_.int_states[ST_XDIGITS]);
    }
    int_digits(a_, a)
}

fn int_prefix(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let c = chr_of(a_, a);
    if c == b'0' as u32 {
        return Ok(a_.int_states[ST_BASE]);
    }
    if !c.is_ascii_digit_u32() {
        unread(a_, a[0]);
        return Ok(NIL);
    }
    Ok(a_.int_states[ST_DIGITS])
}

fn int_sign(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let c = chr_of(a_, a);
    if c == b'+' as u32 || c == b'-' as u32 {
        return Ok(a_.int_states[ST_PREFIX]);
    }
    int_prefix(a_, a)
}

/// Leading zero reads DECIMAL (019 = 19); only an explicit 0x/0X prefix is
/// hex — x-lang #45 R5b, as `x_sexp_int_read` keeps it.
///
/// The VALUE parse runs on the raw text past the token span, stopping at the
/// first invalid character, as the reference's `strtoint(bufferval, NULL, base)`
/// does: a capped analyser can award `0xFF` a one-byte span, and the value is
/// still 255 while `xFF` follows as its own token.
fn int_read(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let b = a[0];
    let (start, end) = (a_.buf_retain(b), a_.buf_cursor(b));
    if end <= start {
        return Ok(NIL);
    }
    let text = a_.buf_text(b);
    let bytes = a_.bytes_of(text);
    // Greedy over BUFFERED text only: the region past the write mark is
    // capacity, and under stdin refill it holds stale bytes from earlier fills.
    let stop = (a_.buf_write(b) as usize).min(bytes.len());
    let raw = &bytes[(start as usize).min(stop)..stop];
    let (sign, body) = match raw.first() {
        Some(b'-') => (-1i64, &raw[1..]),
        Some(b'+') => (1, &raw[1..]),
        _ => (1, raw),
    };
    let n = if body.len() > 1 && body[0] == b'0' && (body[1] == b'x' || body[1] == b'X') {
        body[2..]
            .iter()
            .map_while(|c| (*c as char).to_digit(16))
            .fold(0i64, |acc, d| acc.wrapping_mul(16).wrapping_add(d as i64))
    } else {
        body.iter()
            .map_while(|c| (*c as char).to_digit(10))
            .fold(0i64, |acc, d| acc.wrapping_mul(10).wrapping_add(d as i64))
    };
    Ok(a_.int(sign * n))
}

/// The state table, in `Objects::int_states` order.
#[rustfmt::skip]
pub(crate) const INT_STATES: &[PrimDef] = &[
    PrimDef::row(Some("%int-tok-sign"), None, 3, int_sign_u),
    PrimDef::row(Some("%int-tok-prefix"), None, 3, int_prefix_u),
    PrimDef::row(Some("%int-tok-base"), None, 3, int_base_u),
    PrimDef::row(Some("%int-tok-digits"), None, 3, int_digits_u),
    PrimDef::row(Some("%int-tok-xdigits"), None, 3, int_xdigits_u),
];

#[rustfmt::skip]
pub(crate) const INT_READ: PrimDef = PrimDef::row(Some("%int-tok-read"), None, 1, int_read_u);

trait AsciiDigitU32 {
    fn is_ascii_digit_u32(&self) -> bool;
}
impl AsciiDigitU32 for u32 {
    fn is_ascii_digit_u32(&self) -> bool {
        (0x30..=0x39).contains(self)
    }
}

// --- registration ------------------------------------------------------------

/// `(base make-tok)` — a REAL base with an empty type-alist.
///
/// The reference's `x_prim_make_token_base` allocates an ordinary base with
/// nothing registered, inheriting only the boolean singletons. A token base
/// differs from an interpreter base by its CONTENTS — no catalog, no types —
/// not by its kind: the tokenizer drives whatever type-alist the base it is
/// handed carries, and `base make-type` files into the same alist either way.
fn make_tok(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    let env = e.envs.push_root(&mut e.objects);
    let t = e.objects.sym_shared(crate::vocabulary::TRUE);
    e.envs.bind(&mut e.objects, env, t, t);
    let f = e.objects.sym_shared(crate::vocabulary::FALSE);
    let fo = e.objects.false_obj();
    e.envs.bind(&mut e.objects, env, f, fo);
    let base = crate::base::build(&mut e.objects, NIL, env);
    e.envs.set_base(&mut e.objects, env, base);
    e.base_syms.insert(base, crate::symbols::Symbols::new());
    Ok(base)
}

/// `(base make-type TARGET "NAME" handlers)` — register a type, ANSWERING ITS
/// HANDLE.
///
/// A real base files `(handle . type)` in ITS type-alist — the one list
/// `make-instance` resolves a handle through, `type ?` reads a name from, and
/// the tokenizer contest iterates; apps depend on that identity (apps/logo
/// registers a language on a child base and dispatches every token with
/// `%type?`). A token base keeps its own list for the bare-protocol checks.
///
/// The HANDLE comes back, not the type, because the handle is what everything
/// downstream compares — as `x_prim_base_make_type` answers its name atom.
fn make_type(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let text = e.objects.str_val(a[1]);
    let name = e.objects.handle(&text);
    let ty = e.objects.type_new(name, a[2]);
    let entry = e.objects.spair(name, ty);
    let head = crate::base::get(&e.objects, a[0], crate::base::TYPE_ALIST);
    let cell = e.objects.spair(entry, head);
    crate::base::set(&mut e.objects, a[0], crate::base::TYPE_ALIST, cell);
    Ok(name)
}

crate::uniform_value!(int_sign_u, int_sign, 3);
crate::uniform_value!(int_prefix_u, int_prefix, 3);
crate::uniform_value!(int_base_u, int_base, 3);
crate::uniform_value!(int_digits_u, int_digits, 3);
crate::uniform_value!(int_xdigits_u, int_xdigits, 3);
crate::uniform_value!(int_read_u, int_read, 1);
crate::uniform_value!(make_u, make, 1);
crate::uniform_value!(read_u, read, 1);
crate::uniform_value!(tok_u, tok, 1);
crate::uniform_value!(last_char_u, last_char, 1);
crate::uniform_value!(retain_u, retain, 1);
crate::uniform_value!(reset_u, reset, 1);
crate::uniform_value!(append_u, append, 2);
crate::uniform_value!(read_text_u, read_text, 1);
crate::uniform_engine!(read_str_u, read_str, 2);
crate::uniform_engine!(read_tok_u, read_tok, 1);
crate::uniform_engine!(make_tok_u, make_tok, 0);
crate::uniform_engine!(make_type_u, make_type, 3);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(None, Some(("buf", "make")), 1, make_u),
    PrimDef::row(None, Some(("buf", "read")), 1, read_u),
    PrimDef::row(None, Some(("buf", "tok")), 1, tok_u),
    PrimDef::row(None, Some(("buf", "last-char")), 1, last_char_u),
    PrimDef::row(None, Some(("buf", "retain")), 1, retain_u),
    PrimDef::row(None, Some(("buf", "reset")), 1, reset_u),
    PrimDef::row(None, Some(("buf", "append")), 2, append_u),
    PrimDef::row(None, Some(("buf", "read-text")), 1, read_text_u),
    PrimDef::row(Some("token-read-string"), Some(("tok", "read-str")), 2, read_str_u),
    PrimDef::row(None, Some(("tok", "read")), 1, read_tok_u),
    PrimDef::row(None, Some(("base", "make-tok")), 0, make_tok_u),
    PrimDef::row(None, Some(("base", "make-type")), 3, make_type_u),
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
