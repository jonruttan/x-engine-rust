//! State-image primitives: `image save!`, `image write!`, `image rebuild!`.
//!
//! x-lang's docs/state-image-format.md is the contract and x-engine-c's
//! x-prim/image.c the reference. An object record is
//! `[type][flags][n][label word]*n`; a type word is an object index or a role.
//!
//! Three things are this engine's own, because its heap is:
//!
//! * A unit's label comes from the object's KIND, as the collector traces it
//!   (`collect.rs`, `tree_mark`). The writer's mark is that trace
//!   (`heap tree-mark!`), so a word is a reference in the record exactly
//!   where the mark followed it. The reference asks each type's save handler;
//!   this engine's types have none.
//! * The record's flags carry the kind, which the rebuild allocates by.
//! * Engine-held objects -- the kind handles, the handler instructions, the
//!   markers, `#t`, `#f` -- are imaged by ROLE: the rebuild answers the
//!   loader's own object, as it answers the loader's own symbol for a symbol
//!   of the same name. Symbol identity is pointer identity, and the intern
//!   table is the engine's, not the heap's.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{EnvId, Flags, Obj, Word, NIL, WORD};
use crate::objects::{
    kind_load, kind_store, Objects, ATTR_OWN, FLAG_BUF, FLAG_FALSE, FLAG_FN, FLAG_FOREIGN,
    FLAG_HANDLE, FLAG_ITER, FLAG_OP, FLAG_PAIR, FLAG_SLOTS, FLAG_SPAIR, FLAG_STR, FLAG_SYM,
    FLAG_WRAP, STAMPED_KINDS,
};
use crate::prim::PrimDef;
use crate::prims::ptr::{rd_byte, rd_word, wr_byte, wr_word};
use std::collections::HashMap;

/// Unit labels, docs/state-image-format.md section 3.3.
const UNIT_REF: u64 = 0;
const UNIT_WORD: u64 = 1;
const UNIT_BYTES: u64 = 2;
const UNIT_FOREIGN: u64 = 3;

/// Record type words that are not object indices.
const ROLE_SPAIR: i64 = -1;
const ROLE_SATOM: i64 = -2;
const ROLE_NIL: i64 = -3;
/// An engine-held object: unit 0's word is its key in [`engine_statics`].
const ROLE_ENGINE: i64 = -4;

/// The record words before the units, and the words per unit.
const RECORD_UNITS: usize = 3;
const UNIT_WORDS: usize = 2;

/// The flags a rebuilt object keeps from its record: the contract's attribute
/// bits and RO. The rest describe the allocation that made the original.
const FLAGS_KEPT: u64 = 0x0f | 0x40;

/// Key bands of [`engine_statics`].
const KEY_KIND_HANDLE: u64 = 0x100;
const KEY_INT_STATE: u64 = 0x200;
const KEY_EVAL_HANDLER: u64 = 0x300;
const KEY_PRIM: u64 = 0x1_0000;
const KEY_OTHER_HANDLE: u64 = 1 << 32;

/// The label of data word `i` of `o`, as `tree_mark` reads the kind: what it
/// follows is a reference, a string's first word is its bytes, a slot
/// vector's routines are foreign, everything else is a machine word.
fn unit_label(o: &Objects, obj: Obj, kind: Flags, i: u64) -> u64 {
    match kind {
        k if k == FLAG_PAIR || k == FLAG_SPAIR || k == FLAG_ITER => UNIT_REF,
        k if k == FLAG_FN || k == FLAG_OP || k == FLAG_WRAP => {
            if i == 1 {
                UNIT_REF
            } else {
                UNIT_WORD
            }
        }
        k if k == FLAG_FALSE => {
            if i >= 1 {
                UNIT_REF
            } else {
                UNIT_WORD
            }
        }
        k if k == FLAG_BUF => {
            if i == 1 || i == 2 {
                UNIT_REF
            } else {
                UNIT_WORD
            }
        }
        k if k == FLAG_SLOTS => {
            if i == 0 {
                UNIT_REF
            } else {
                UNIT_FOREIGN
            }
        }
        k if k == FLAG_FOREIGN => {
            if i == 0 {
                UNIT_FOREIGN
            } else {
                UNIT_REF
            }
        }
        k if k == FLAG_STR || k == FLAG_SYM || k == FLAG_HANDLE => {
            if i == 0 {
                UNIT_BYTES
            } else {
                UNIT_WORD
            }
        }
        // An instance: x-lang's words, traced conservatively.
        k if k.raw() == 0 => {
            if o.plausible(o.data(obj, i).raw()) {
                UNIT_REF
            } else {
                UNIT_WORD
            }
        }
        _ => UNIT_WORD,
    }
}

/// Every unit of `obj`: its label and its word as stored.
///
/// A word in a reference unit that is not an object's start is a machine
/// word, as the trace passes it by: a spine's cell may hold a raw integer.
fn units_of(o: &Objects, obj: Obj) -> Vec<(u64, u64)> {
    let kind = o.flags(obj);
    (0..o.data_len(obj))
        .map(|i| {
            let w = o.data(obj, i).raw();
            match unit_label(o, obj, kind, i) {
                UNIT_REF if w != 0 && !o.plausible(w) => (UNIT_WORD, w),
                label => (label, w),
            }
        })
        .collect()
}

/// Every primitive object the engine made: the catalogue's and the bare
/// bindings'. One object per instruction, shared by every base.
fn engine_prims(e: &Engine) -> Vec<Obj> {
    let mut out: Vec<Obj> = e.prim_bindings.iter().map(|&(_, p)| p).collect();
    out.extend(e.coord_prims.iter().map(|&(_, _, p)| p));
    out
}

/// The engine-held objects an image names by key rather than copies, keyed
/// by address. A key is stable across processes of one build.
fn engine_statics(e: &Engine) -> HashMap<u64, u64> {
    let o = &e.objects;
    let mut keyed: Vec<(u64, Obj)> = vec![
        (0, o.satom_marker),
        (1, o.pair_type),
        (2, o.int_read),
        (3, o.callable_call_handler),
        (4, o.list_call_handler),
        (5, o.base_label_atom),
        (6, o.true_obj()),
        (7, o.false_obj()),
    ];
    keyed.extend(
        o.kind_handles
            .iter()
            .enumerate()
            .map(|(i, &h)| (KEY_KIND_HANDLE + i as u64, h)),
    );
    keyed.extend(
        o.int_states
            .iter()
            .enumerate()
            .map(|(i, &s)| (KEY_INT_STATE + i as u64, s)),
    );
    keyed.extend(
        o.eval_handlers
            .iter()
            .enumerate()
            .map(|(i, &h)| (KEY_EVAL_HANDLER + i as u64, h)),
    );
    keyed.extend(
        engine_prims(e)
            .into_iter()
            .map(|p| (KEY_PRIM + o.prim_idx(p) as u64, p)),
    );
    keyed.extend(
        o.other_kind_handles
            .iter()
            .map(|(f, &h)| (KEY_OTHER_HANDLE | f.raw(), h)),
    );
    let mut map = HashMap::new();
    for (k, obj) in keyed {
        if !obj.is_nil() {
            map.entry(obj.word().raw()).or_insert(k);
        }
    }
    map
}

/// The addresses of the objects every base shares: the engine statics and
/// the shared symbols. A walk marks them and stops there.
pub(crate) fn shared_objects(e: &Engine) -> std::collections::HashSet<u64> {
    let mut out: std::collections::HashSet<u64> = engine_statics(e).into_keys().collect();
    out.extend(
        e.objects
            .shared_symbols
            .all()
            .iter()
            .map(|s| s.word().raw()),
    );
    out
}

/// The loader's own object for an engine-static key; `name` is the handle
/// text an other-kind handle is made with when this process has none yet.
fn engine_static(e: &mut Engine, key: u64, name: &str) -> Obj {
    let o = &mut e.objects;
    match key {
        0 => o.satom_marker,
        1 => o.pair_type,
        2 => o.int_read,
        3 => o.callable_call_handler,
        4 => o.list_call_handler,
        5 => o.base_label(),
        6 => o.true_obj(),
        7 => o.false_obj(),
        k if k & KEY_OTHER_HANDLE != 0 => o.kind_handle(Flags::new(k & !KEY_OTHER_HANDLE), name),
        k if k >= KEY_PRIM => {
            let idx = (k - KEY_PRIM) as usize;
            engine_prims(e)
                .into_iter()
                .find(|&p| e.objects.prim_idx(p) == idx)
                .unwrap_or(NIL)
        }
        k if k >= KEY_EVAL_HANDLER => o
            .eval_handlers
            .get((k - KEY_EVAL_HANDLER) as usize)
            .copied()
            .unwrap_or(NIL),
        k if k >= KEY_INT_STATE => o
            .int_states
            .get((k - KEY_INT_STATE) as usize)
            .copied()
            .unwrap_or(NIL),
        k if k >= KEY_KIND_HANDLE => match STAMPED_KINDS.get((k - KEY_KIND_HANDLE) as usize) {
            Some(&(f, text)) => o.kind_handle(f, text),
            None => NIL,
        },
        _ => NIL,
    }
}

/// The NUL-terminated bytes at any address, arena or process memory.
fn c_bytes(o: &Objects, at: u64) -> Vec<u8> {
    let mut out = Vec::new();
    if at == 0 {
        return out;
    }
    let mut p = at;
    loop {
        let b = rd_byte(o, p);
        if b == 0 {
            return out;
        }
        out.push(b);
        p += 1;
    }
}

fn fail(msg: &str) -> Cond {
    Cond::EngineMsg(msg.to_string())
}

// --- image save! ---------------------------------------------------------------

/// `(image save! obj buf)` -- buf receives `[n][label word]*n`, the words as
/// stored; answers n. docs/state-image-format.md section 4.3.
fn save(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let buf = e.objects.as_ptr(a[1]).raw();
    let units = units_of(&e.objects, a[0]);
    let w = WORD as u64;
    wr_word(&mut e.objects, buf, units.len() as u64);
    for (j, &(label, word)) in units.iter().enumerate() {
        let at = buf + w * (1 + UNIT_WORDS * j) as u64;
        wr_word(&mut e.objects, at, label);
        wr_word(&mut e.objects, at + w, word);
    }
    Ok(e.objects.int(units.len() as i64))
}

// --- image write! --------------------------------------------------------------

/// Everything a write pass reads and fills.
struct Writer {
    flag: u64,
    name: Obj,
    env: EnvId,
    statics: HashMap<u64, u64>,
    /// Shared symbol addresses: written with no type word, found by name.
    shared: std::collections::HashSet<u64>,
    /// Object address to object index.
    index: HashMap<u64, i64>,
    /// Word to the external index the naming callable answered.
    externs: HashMap<u64, i64>,
    table: Vec<u64>,
    blob: Vec<u8>,
    sent: i64,
}

impl Writer {
    fn imaged(&self, o: &Objects, obj: Obj) -> bool {
        o.flags_word(obj) & self.flag != 0
    }

    /// The external index of `word`, asking the naming callable once per
    /// distinct word: `(name word label obj)`, an integer or nil.
    fn external(&mut self, e: &mut Engine, word: u64, label: u64, obj: Obj) -> Result<i64, Cond> {
        if let Some(&k) = self.externs.get(&word) {
            return Ok(k);
        }
        let vals = [e.objects.int(word as i64), e.objects.int(label as i64), obj];
        let r = e.apply_values_own(self.name, &vals, self.env)?;
        let k = if r.is_nil() { 0 } else { e.objects.as_int(r) };
        if k == 0 {
            self.sent += 1;
        }
        self.externs.insert(word, k);
        Ok(k)
    }

    /// A bytes unit: the blob offset of `[len][bytes NUL]`.
    fn bytes(&mut self, o: &Objects, word: u64) -> u64 {
        let off = self.blob.len() as u64;
        let bytes = c_bytes(o, word);
        self.blob
            .extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        self.blob.extend_from_slice(&bytes);
        self.blob.push(0);
        off
    }

    fn type_word(&self, o: &Objects, obj: Obj) -> Result<i64, Cond> {
        if self.shared.contains(&obj.word().raw()) {
            return Ok(ROLE_NIL);
        }
        if self.statics.contains_key(&obj.word().raw()) {
            return Ok(ROLE_ENGINE);
        }
        let t = o.type_of_word(obj);
        if t.is_nil() {
            Ok(ROLE_NIL)
        } else if t == o.spair_marker {
            Ok(ROLE_SPAIR)
        } else if t == o.satom_marker {
            Ok(ROLE_SATOM)
        } else {
            self.index
                .get(&t.word().raw())
                .copied()
                .ok_or_else(|| fail(crate::vocabulary::MSG_IMAGE_TYPE_NOT_IMAGED))
        }
    }

    fn emit(&mut self, e: &mut Engine, obj: Obj) -> Result<(), Cond> {
        let tw = self.type_word(&e.objects, obj)?;
        let units = if tw == ROLE_ENGINE {
            let key = self.statics[&obj.word().raw()];
            let mut u = vec![(UNIT_WORD, key)];
            if e.objects.is(obj, FLAG_HANDLE) {
                u.push((UNIT_BYTES, e.objects.data(obj, 0).raw()));
            }
            u
        } else {
            units_of(&e.objects, obj)
        };
        let flags = e.objects.flags_word(obj);
        self.table.push(tw as u64);
        self.table
            .push(kind_store(kind_load(flags)) | (flags & FLAGS_KEPT));
        self.table.push(units.len() as u64);
        for (label, word) in units {
            let out = match label {
                UNIT_REF if word == 0 => 0,
                UNIT_REF => match self.index.get(&word) {
                    Some(&i) => i as u64,
                    None => {
                        let k = self.external(e, word, UNIT_REF, obj)?;
                        (-k) as u64
                    }
                },
                UNIT_BYTES => self.bytes(&e.objects, word),
                UNIT_FOREIGN if word == 0 => 0,
                UNIT_FOREIGN => self.external(e, word, UNIT_FOREIGN, obj)? as u64,
                _ => word,
            };
            self.table.push(label);
            self.table.push(out);
        }
        Ok(())
    }
}

/// `(image write! cursor flag table blob name roots result)` -- the object
/// table and the blob of every object from `cursor` down the allocation
/// chain that carries `flag`. docs/state-image-format.md section 4.
///
/// `result` holds the table's capacity in words and the blob's in bytes on
/// the way in; on the way out the object count, table words, blob bytes and
/// sentinel count, then the object index of each of `roots` (0 for one
/// outside the image). Answers the object count.
fn write(e: &mut Engine, c: Obj, args: Obj, env: EnvId) -> EvalResult {
    let a = e.eargs_for(c, args, env, 7)?;
    let result = e.objects.as_ptr(a[6]).raw();
    let w = WORD as u64;
    let table_cap = rd_word(&e.objects, result) as usize;
    let blob_cap = rd_word(&e.objects, result + w) as usize;
    let mut wr = Writer {
        flag: e.objects.as_int(a[1]) as u64,
        name: a[4],
        env,
        statics: engine_statics(e),
        shared: e
            .objects
            .shared_symbols
            .all()
            .iter()
            .map(|s| s.word().raw())
            .collect(),
        index: HashMap::new(),
        externs: HashMap::new(),
        table: Vec::new(),
        blob: Vec::new(),
        sent: 0,
    };

    // Pass 1: indices in chain order, newest first, as the loader rebuilds them.
    let mut chain = Vec::new();
    let mut at = a[0];
    while !at.is_nil() {
        if wr.imaged(&e.objects, at) {
            chain.push(at);
            wr.index.insert(at.word().raw(), chain.len() as i64);
        }
        at = e.objects.chain_next(at);
    }

    // Pass 2: each object's record.
    for &obj in &chain {
        wr.emit(e, obj)?;
    }
    if wr.table.len() > table_cap {
        return Err(fail(crate::vocabulary::MSG_IMAGE_TABLE_FULL));
    }
    if wr.blob.len() > blob_cap {
        return Err(fail(crate::vocabulary::MSG_IMAGE_BLOB_FULL));
    }

    let table = e.objects.as_ptr(a[2]).raw();
    for (i, &word) in wr.table.iter().enumerate() {
        wr_word(&mut e.objects, table + w * i as u64, word);
    }
    let blob = e.objects.as_ptr(a[3]).raw();
    for (i, &b) in wr.blob.iter().enumerate() {
        wr_byte(&mut e.objects, blob + i as u64, b);
    }

    let counts = [
        chain.len() as u64,
        wr.table.len() as u64,
        wr.blob.len() as u64,
        wr.sent as u64,
    ];
    for (i, &v) in counts.iter().enumerate() {
        wr_word(&mut e.objects, result + w * i as u64, v);
    }
    let mut slot = counts.len() as u64;
    let mut roots = a[5];
    while !roots.is_nil() {
        let r = e.objects.first(roots);
        let i = if r.is_nil() {
            0
        } else {
            wr.index.get(&r.word().raw()).copied().unwrap_or(0)
        };
        wr_word(&mut e.objects, result + w * slot, i as u64);
        slot += 1;
        roots = e.objects.rest(roots);
    }
    Ok(e.objects.int(chain.len() as i64))
}

// --- image rebuild! ------------------------------------------------------------

/// One record as read: its type word, flags and units.
struct Record {
    tw: i64,
    flags: u64,
    units: Vec<(u64, u64)>,
}

/// Everything the rebuild passes read.
struct Image {
    blob: u64,
    ext: Obj,
    nextern: i64,
    ix: Vec<Obj>,
}

impl Image {
    /// The bytes of a blob entry, `[len][bytes NUL]` at offset `off`.
    fn bytes(&self, o: &Objects, off: u64) -> Vec<u8> {
        let at = self.blob + off;
        let len = rd_word(o, at);
        (0..len).map(|i| rd_byte(o, at + WORD as u64 + i)).collect()
    }

    fn external(&self, o: &Objects, k: i64) -> Obj {
        if k <= 0 || k >= self.nextern {
            return NIL;
        }
        o.data(self.ext, k as u64).as_obj()
    }

    /// A ref unit: an object index, or the negated external index.
    fn reference(&self, o: &Objects, v: i64) -> Obj {
        if v > 0 && (v as usize) < self.ix.len() {
            return self.ix[v as usize];
        }
        self.external(o, -v)
    }
}

/// `(image rebuild! buf ostart nobj externals nextern blob index)` -- the
/// object graph of an image, docs/state-image-format.md section 5. Two
/// passes, allocate and patch, so a type exists before an instance names it.
/// A symbol is the loader's own when it has one of that name, and an
/// engine-held object is always the loader's own. `index` is filled so that
/// entry i is object i; answers it.
fn rebuild(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let w = WORD as u64;
    let buf = e.objects.as_ptr(a[0]).raw();
    let ostart = e.objects.as_int(a[1]) as u64;
    let n = e.objects.as_int(a[2]).max(0) as usize;
    let index = e.objects.as_ptr(a[6]).raw();
    let mut img = Image {
        blob: e.objects.as_ptr(a[5]).raw(),
        ext: a[3],
        nextern: e.objects.as_int(a[4]),
        ix: vec![NIL; n + 1],
    };

    let mut records = Vec::with_capacity(n);
    let mut pos = buf + w * ostart;
    for _ in 0..n {
        let tw = rd_word(&e.objects, pos) as i64;
        let flags = rd_word(&e.objects, pos + w);
        let count = rd_word(&e.objects, pos + 2 * w) as usize;
        let units = (0..count)
            .map(|j| {
                let u = pos + w * (RECORD_UNITS + UNIT_WORDS * j) as u64;
                (rd_word(&e.objects, u), rd_word(&e.objects, u + w))
            })
            .collect();
        records.push(Record { tw, flags, units });
        pos += w * (RECORD_UNITS + UNIT_WORDS * count) as u64;
    }

    // Pass 1: allocate, or answer the loader's own.
    let mut own = vec![false; n + 1];
    for (k, r) in records.iter().enumerate() {
        let i = k + 1;
        let kind = kind_load(r.flags);
        if r.tw == ROLE_ENGINE {
            let key = r.units.first().map_or(u64::MAX, |u| u.1);
            let name = match r.units.get(1) {
                Some(&(UNIT_BYTES, off)) => {
                    String::from_utf8_lossy(&img.bytes(&e.objects, off)).into_owned()
                }
                _ => String::new(),
            };
            img.ix[i] = engine_static(e, key, &name);
            own[i] = true;
            continue;
        }
        if kind == FLAG_SYM {
            if let Some(&(UNIT_BYTES, off)) = r.units.first() {
                let name = String::from_utf8_lossy(&img.bytes(&e.objects, off)).into_owned();
                let found = e
                    .objects
                    .shared_symbols
                    .get(&name)
                    .or_else(|| e.objects.symbols.get(&name));
                if let Some(s) = found {
                    img.ix[i] = s;
                    own[i] = true;
                    continue;
                }
            }
        }
        let obj = e.objects.alloc(kind, r.units.len().max(1));
        e.objects.set_attr(obj, r.flags & FLAGS_KEPT);
        img.ix[i] = obj;
    }

    // Pass 2: type each object and fill its units.
    for (k, r) in records.iter().enumerate() {
        let i = k + 1;
        if own[i] {
            continue;
        }
        let obj = img.ix[i];
        let t = match r.tw {
            ROLE_SPAIR => e.objects.spair_marker,
            ROLE_SATOM => e.objects.satom_marker,
            ROLE_NIL => NIL,
            t => img.reference(&e.objects, t),
        };
        e.objects.set_type_word(obj, t);
        for (j, &(label, v)) in r.units.iter().enumerate() {
            let word = match label {
                UNIT_REF => img.reference(&e.objects, v as i64).word(),
                UNIT_BYTES => {
                    let bytes = img.bytes(&e.objects, v);
                    e.objects.set_attr(obj, ATTR_OWN);
                    Word(e.objects.heap.store_bytes(&bytes).raw())
                }
                UNIT_FOREIGN => {
                    let x = img.external(&e.objects, v as i64);
                    Word(if x.is_nil() {
                        0
                    } else {
                        e.objects.as_int(x) as u64
                    })
                }
                _ => Word(v),
            };
            e.objects.set_data(obj, j as u64, word);
        }
        if kind_load(r.flags) == FLAG_SYM {
            let name = e.objects.sym_name(obj);
            e.objects.symbols.put(&name, obj);
        }
    }

    for (i, &obj) in img.ix.iter().enumerate() {
        wr_word(&mut e.objects, index + w * i as u64, obj.word().raw());
    }
    Ok(a[6])
}

crate::uniform_engine!(save_u, save, 2);
crate::uniform_engine!(rebuild_u, rebuild, 7);

pub const TABLE: &[PrimDef] = &[
    PrimDef::row(None, Some(("image", "save!")), 2, save_u),
    PrimDef::row(None, Some(("image", "write!")), 7, write),
    PrimDef::row(None, Some(("image", "rebuild!")), 7, rebuild_u),
];
