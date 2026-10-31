//! `maix/1.1-paged`: the cells of a large store as checked 4 KB pages.
//!
//! A store with many cells would otherwise travel as one huge JSON document that has to be parsed
//! whole. Above [`MIN_CELLS`] the package keeps the store's structure (sheets, columns, formats,
//! dimensions, history) in `data/maix.json` with the profile `maix/1.1-paged`, and the cells in
//! `data/maix.pages`, stored uncompressed in the zip:
//!
//! ```text
//! page 0        header   "MXPG", version, page size, sheet count, page count, cell count,
//!                        and per sheet: id, cells, first leaf, leaf count, first index page
//! leaf pages    cells in (row, col, dimension) order, chained through `next`
//! index pages   per leaf: its first key and page number (so one cell is found without
//!               reading the whole sheet)
//! overflow      the body of a cell too large for a leaf, chained through `next`
//! ```
//!
//! Every page starts with a CRC-32C of the rest of the page, its kind, a record count and the
//! next page of its chain. Reading checks every page it touches and never trusts a count, length
//! or pointer: chains cannot loop, lengths cannot run past their page, keys must be in order.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::maix::{Cell, Maix, Meta, Value};

pub const PAGE: usize = 4096;
pub const PROFILE_PAGED: &str = "maix/1.1-paged";
/// Stores with at least this many cells are written paged.
pub const MIN_CELLS: usize = 200_000;
/// Most sheets a paged store can describe (the directory lives in the header page).
pub const MAX_SHEETS: usize = 100;
const MAGIC: &[u8; 4] = b"MXPG";
const VERSION: u16 = 1;
const NONE: u32 = u32::MAX;
const HEAD: usize = 12;
const BODY: usize = PAGE - HEAD;
/// Bodies up to this size sit inside the leaf; larger ones go to overflow pages.
const MAX_INLINE: usize = 1024;
const MAX_CELLS: u64 = 100_000_000;
const KIND_HEADER: u8 = 0;
const KIND_LEAF: u8 = 1;
const KIND_INDEX: u8 = 2;
const KIND_OVERFLOW: u8 = 3;
const INDEX_ENTRY: usize = 18;

type Key = (u32, u16, u64);

fn bad(message: impl Into<String>) -> Error {
    Error::Paged(message.into())
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 { (c >> 1) ^ 0x82F6_3B78 } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32C (Castagnoli).
pub fn crc32c(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        let index = ((c ^ u32::from(b)) & 0xFF) as usize;
        c = CRC_TABLE.get(index).copied().unwrap_or(0) ^ (c >> 8);
    }
    !c
}

// ---- reading primitives ---------------------------------------------------------------------

struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    fn new(b: &'a [u8]) -> Self {
        Rd { b, at: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or_else(|| bad("length overflow"))?;
        let out = self.b.get(self.at..end).ok_or_else(|| bad("a record runs past its page"))?;
        self.at = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    fn text(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| bad("text that is not UTF-8"))
    }

    fn key(&mut self) -> Result<Key> {
        Ok((self.u32()?, self.u16()?, self.u64()?))
    }

    fn done(&self) -> bool {
        self.at == self.b.len()
    }
}

struct Page<'a> {
    kind: u8,
    count: u16,
    next: u32,
    payload: &'a [u8],
}

fn page(bytes: &[u8], no: u32) -> Result<Page<'_>> {
    let start = (no as usize).checked_mul(PAGE).ok_or_else(|| bad("page number overflow"))?;
    let end = start.checked_add(PAGE).ok_or_else(|| bad("page number overflow"))?;
    let raw = bytes.get(start..end).ok_or_else(|| bad(format!("page {no} is beyond the end")))?;
    let mut r = Rd::new(raw);
    let stored = r.u32()?;
    if crc32c(raw.get(4..).unwrap_or(&[])) != stored {
        return Err(bad(format!("page {no} fails its checksum")));
    }
    let kind = r.u8()?;
    r.u8()?;
    let count = r.u16()?;
    let next = r.u32()?;
    Ok(Page { kind, count, next, payload: raw.get(HEAD..).unwrap_or(&[]) })
}

// ---- writing --------------------------------------------------------------------------------

struct PageBuf {
    kind: u8,
    count: u16,
    next: u32,
    payload: Vec<u8>,
}

impl PageBuf {
    fn new(kind: u8) -> Self {
        PageBuf { kind, count: 0, next: NONE, payload: Vec::new() }
    }

    fn write(&self, out: &mut Vec<u8>) -> Result<()> {
        if self.payload.len() > BODY {
            return Err(bad("a page overflowed"));
        }
        let mut raw = vec![0u8; PAGE];
        raw[4] = self.kind;
        raw[6..8].copy_from_slice(&self.count.to_le_bytes());
        raw[8..12].copy_from_slice(&self.next.to_le_bytes());
        raw[HEAD..HEAD + self.payload.len()].copy_from_slice(&self.payload);
        let crc = crc32c(&raw[4..]);
        raw[..4].copy_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&raw);
        Ok(())
    }
}

fn push(pages: &mut Vec<PageBuf>, p: PageBuf) -> Result<u32> {
    let no = u32::try_from(pages.len()).map_err(|_| bad("too many pages"))?;
    if no >= NONE - 1 {
        return Err(bad("too many pages"));
    }
    pages.push(p);
    Ok(no)
}

fn put_text(out: &mut Vec<u8>, text: &str) -> Result<()> {
    out.extend_from_slice(&u32::try_from(text.len()).map_err(|_| bad("text too long"))?.to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

fn encode_body(cell: &Cell) -> Result<Vec<u8>> {
    let mut b = Vec::new();
    b.extend_from_slice(&cell.version.to_le_bytes());
    b.extend_from_slice(&cell.updated_at.to_le_bytes());
    match &cell.value {
        Value::Null => b.push(0),
        Value::Int(i) => {
            b.push(1);
            b.extend_from_slice(&i.to_le_bytes());
        }
        Value::Float(f) => {
            b.push(2);
            b.extend_from_slice(&f.to_bits().to_le_bytes());
        }
        Value::Text(t) => {
            b.push(3);
            put_text(&mut b, t)?;
        }
        Value::Bool(v) => {
            b.push(4);
            b.push(u8::from(*v));
        }
        Value::Date(d) => {
            b.push(5);
            b.extend_from_slice(&d.to_le_bytes());
        }
    }
    match &cell.formula {
        Some(f) => {
            b.push(1);
            put_text(&mut b, f)?;
        }
        None => b.push(0),
    }
    if cell.meta.is_empty() {
        b.push(0);
    } else {
        b.push(1);
        let json = serde_json::to_string(&cell.meta)?;
        put_text(&mut b, &json)?;
    }
    Ok(b)
}

fn decode_body(body: &[u8]) -> Result<Cell> {
    let mut r = Rd::new(body);
    let version = r.u32()?;
    let updated_at = r.u64()?;
    let value = match r.u8()? {
        0 => Value::Null,
        1 => Value::Int(r.i64()?),
        2 => Value::Float(f64::from_bits(r.u64()?)),
        3 => Value::Text(r.text()?),
        4 => Value::Bool(r.u8()? != 0),
        5 => Value::Date(r.i64()?),
        other => return Err(bad(format!("unknown value type {other}"))),
    };
    let formula = if r.u8()? == 0 { None } else { Some(r.text()?) };
    let meta: BTreeMap<String, Meta> = if r.u8()? == 0 { BTreeMap::new() } else { serde_json::from_str(&r.text()?)? };
    if !r.done() {
        return Err(bad("a cell has bytes left over"));
    }
    Ok(Cell { value, formula, version, updated_at, meta })
}

/// One leaf record: the key and either the cell body or a pointer to its overflow pages.
fn record(key: &Key, cell: &Cell, pages: &mut Vec<PageBuf>) -> Result<Vec<u8>> {
    let body = encode_body(cell)?;
    let mut rec = Vec::with_capacity(body.len().min(MAX_INLINE) + 24);
    rec.extend_from_slice(&key.0.to_le_bytes());
    rec.extend_from_slice(&key.1.to_le_bytes());
    rec.extend_from_slice(&key.2.to_le_bytes());
    if body.len() <= MAX_INLINE {
        rec.push(0);
        rec.extend_from_slice(&(body.len() as u16).to_le_bytes());
        rec.extend_from_slice(&body);
    } else {
        let first = pages.len();
        let chunks: Vec<&[u8]> = body.chunks(BODY).collect();
        for (i, chunk) in chunks.iter().enumerate() {
            let mut p = PageBuf::new(KIND_OVERFLOW);
            p.count = chunk.len() as u16;
            p.next = if i + 1 < chunks.len() { (first + i + 1) as u32 } else { NONE };
            p.payload = chunk.to_vec();
            push(pages, p)?;
        }
        rec.push(1);
        rec.extend_from_slice(&(first as u32).to_le_bytes());
        rec.extend_from_slice(&u32::try_from(body.len()).map_err(|_| bad("a cell is too large"))?.to_le_bytes());
    }
    Ok(rec)
}

struct DirEntry {
    id: u16,
    cells: u64,
    first_leaf: u32,
    leaves: u32,
    first_index: u32,
}

/// Writes every sheet's cells as pages. The store's structure travels in `data/maix.json`.
pub fn encode(maix: &Maix) -> Result<Vec<u8>> {
    if maix.sheets.len() > MAX_SHEETS {
        return Err(bad("too many sheets for paged storage"));
    }
    let mut pages = vec![PageBuf::new(KIND_HEADER)];
    let mut dir = vec![];
    let mut total = 0u64;
    for sheet in &maix.sheets {
        let mut leaves: Vec<(u32, Key)> = vec![];
        let mut cur: Option<(PageBuf, Key)> = None;
        for (key, cell) in &sheet.cells {
            let rec = record(key, cell, &mut pages)?;
            if cur.as_ref().is_some_and(|(p, _)| p.payload.len() + rec.len() > BODY)
                && let Some((full, first)) = cur.take()
            {
                let no = push(&mut pages, full)?;
                leaves.push((no, first));
            }
            let (p, _) = cur.get_or_insert_with(|| (PageBuf::new(KIND_LEAF), *key));
            p.payload.extend_from_slice(&rec);
            p.count += 1;
        }
        if let Some((last, first)) = cur.take() {
            let no = push(&mut pages, last)?;
            leaves.push((no, first));
        }
        for (i, (no, _)) in leaves.iter().enumerate() {
            let next = leaves.get(i + 1).map_or(NONE, |l| l.0);
            if let Some(p) = pages.get_mut(*no as usize) {
                p.next = next;
            }
        }
        let first_index = pages.len() as u32;
        let per = BODY / INDEX_ENTRY;
        let chunks: Vec<&[(u32, Key)]> = leaves.chunks(per).collect();
        for (j, chunk) in chunks.iter().enumerate() {
            let mut p = PageBuf::new(KIND_INDEX);
            p.count = chunk.len() as u16;
            p.next = if j + 1 < chunks.len() { first_index + j as u32 + 1 } else { NONE };
            for (no, key) in *chunk {
                p.payload.extend_from_slice(&key.0.to_le_bytes());
                p.payload.extend_from_slice(&key.1.to_le_bytes());
                p.payload.extend_from_slice(&key.2.to_le_bytes());
                p.payload.extend_from_slice(&no.to_le_bytes());
            }
            push(&mut pages, p)?;
        }
        let cells = sheet.cells.len() as u64;
        total += cells;
        dir.push(DirEntry { id: sheet.id, cells, first_leaf: leaves.first().map_or(NONE, |l| l.0), leaves: leaves.len() as u32, first_index: if chunks.is_empty() { NONE } else { first_index } });
    }
    let mut head = PageBuf::new(KIND_HEADER);
    head.count = dir.len() as u16;
    head.payload.extend_from_slice(MAGIC);
    head.payload.extend_from_slice(&VERSION.to_le_bytes());
    head.payload.extend_from_slice(&(PAGE as u32).to_le_bytes());
    head.payload.extend_from_slice(&(pages.len() as u32).to_le_bytes());
    head.payload.extend_from_slice(&total.to_le_bytes());
    for e in &dir {
        head.payload.extend_from_slice(&e.id.to_le_bytes());
        head.payload.extend_from_slice(&e.cells.to_le_bytes());
        head.payload.extend_from_slice(&e.first_leaf.to_le_bytes());
        head.payload.extend_from_slice(&e.leaves.to_le_bytes());
        head.payload.extend_from_slice(&e.first_index.to_le_bytes());
    }
    if let Some(slot) = pages.first_mut() {
        *slot = head;
    }
    let mut out = Vec::with_capacity(pages.len() * PAGE);
    for p in &pages {
        p.write(&mut out)?;
    }
    Ok(out)
}

// ---- reading --------------------------------------------------------------------------------

/// Reads the cells back from pages, one sheet at a time or one cell at a time.
pub struct PagedStore<'a> {
    bytes: &'a [u8],
    dir: Vec<DirEntry>,
    n_pages: usize,
    pub total_cells: u64,
}

impl<'a> PagedStore<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self> {
        if bytes.is_empty() || bytes.len() % PAGE != 0 {
            return Err(bad("the data is not a whole number of pages"));
        }
        let n_pages = bytes.len() / PAGE;
        let head = page(bytes, 0)?;
        if head.kind != KIND_HEADER {
            return Err(bad("the first page is not a header"));
        }
        let mut r = Rd::new(head.payload);
        if &r.array::<4>()? != MAGIC {
            return Err(bad("wrong magic"));
        }
        if r.u16()? != VERSION {
            return Err(bad("a version this program does not read"));
        }
        if r.u32()? as usize != PAGE {
            return Err(bad("an unexpected page size"));
        }
        if r.u32()? as usize != n_pages {
            return Err(bad("the page count does not match the data"));
        }
        let total_cells = r.u64()?;
        if total_cells > MAX_CELLS {
            return Err(bad("too many cells"));
        }
        if head.count as usize > MAX_SHEETS {
            return Err(bad("too many sheets"));
        }
        let mut dir = vec![];
        for _ in 0..head.count {
            let e = DirEntry { id: r.u16()?, cells: r.u64()?, first_leaf: r.u32()?, leaves: r.u32()?, first_index: r.u32()? };
            if e.leaves as usize > n_pages || e.cells > total_cells {
                return Err(bad("a sheet entry is out of range"));
            }
            dir.push(e);
        }
        if dir.iter().map(|e| e.cells).sum::<u64>() != total_cells {
            return Err(bad("the sheet cell counts do not add up"));
        }
        Ok(PagedStore { bytes, dir, n_pages, total_cells })
    }

    pub fn sheet_ids(&self) -> Vec<u16> {
        self.dir.iter().map(|e| e.id).collect()
    }

    fn entry(&self, id: u16) -> Result<&DirEntry> {
        self.dir.iter().find(|e| e.id == id).ok_or_else(|| bad(format!("no pages for sheet {id}")))
    }

    fn overflow(&self, first: u32, total: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(total.min(1 << 20));
        let mut no = first;
        let mut hops = 0usize;
        while no != NONE {
            hops += 1;
            if hops > self.n_pages {
                return Err(bad("an overflow chain loops"));
            }
            let p = page(self.bytes, no)?;
            if p.kind != KIND_OVERFLOW {
                return Err(bad("an overflow pointer leads to the wrong kind of page"));
            }
            out.extend_from_slice(p.payload.get(..p.count as usize).ok_or_else(|| bad("an overflow page claims too much"))?);
            if out.len() > total {
                return Err(bad("an overflow chain is longer than declared"));
            }
            no = p.next;
        }
        if out.len() != total {
            return Err(bad("an overflow chain is shorter than declared"));
        }
        Ok(out)
    }

    fn read_record(&self, r: &mut Rd<'_>) -> Result<(Key, Cell)> {
        let key = r.key()?;
        let cell = match r.u8()? {
            0 => {
                let n = r.u16()? as usize;
                decode_body(r.take(n)?)?
            }
            1 => {
                let first = r.u32()?;
                let total = r.u32()? as usize;
                decode_body(&self.overflow(first, total)?)?
            }
            other => return Err(bad(format!("unknown record mode {other}"))),
        };
        Ok((key, cell))
    }

    fn leaf_records(&self, no: u32) -> Result<Vec<(Key, Cell)>> {
        let p = page(self.bytes, no)?;
        if p.kind != KIND_LEAF {
            return Err(bad("a leaf pointer leads to the wrong kind of page"));
        }
        let mut r = Rd::new(p.payload);
        let mut out = Vec::with_capacity(p.count as usize);
        for _ in 0..p.count {
            out.push(self.read_record(&mut r)?);
        }
        Ok(out)
    }

    /// Checks the checksum and kind of every page, including the ones a plain read never visits.
    pub fn verify(&self) -> Result<()> {
        for no in 0..self.n_pages {
            let no = u32::try_from(no).map_err(|_| bad("too many pages"))?;
            let p = page(self.bytes, no)?;
            if p.kind > KIND_OVERFLOW {
                return Err(bad(format!("page {no} has an unknown kind")));
            }
        }
        Ok(())
    }

    /// Every cell of a sheet.
    pub fn read_sheet(&self, id: u16) -> Result<BTreeMap<Key, Cell>> {
        let e = self.entry(id)?;
        let mut out = BTreeMap::new();
        let mut no = e.first_leaf;
        let mut seen = 0u32;
        let mut last: Option<Key> = None;
        while no != NONE {
            if seen >= e.leaves {
                return Err(bad("a leaf chain is longer than declared"));
            }
            seen += 1;
            let p = page(self.bytes, no)?;
            for (key, cell) in self.leaf_records(no)? {
                if last.is_some_and(|l| l >= key) {
                    return Err(bad("cells out of order"));
                }
                last = Some(key);
                out.insert(key, cell);
                if out.len() as u64 > e.cells {
                    return Err(bad("more cells than declared"));
                }
            }
            no = p.next;
        }
        if seen != e.leaves || out.len() as u64 != e.cells {
            return Err(bad("a sheet has fewer pages or cells than declared"));
        }
        Ok(out)
    }

    /// The index of a sheet: the first key of each leaf and the leaf's page.
    pub fn index(&self, id: u16) -> Result<Vec<(Key, u32)>> {
        let e = self.entry(id)?;
        let mut out = vec![];
        let mut no = e.first_index;
        let mut hops = 0usize;
        while no != NONE {
            hops += 1;
            if hops > self.n_pages {
                return Err(bad("an index chain loops"));
            }
            let p = page(self.bytes, no)?;
            if p.kind != KIND_INDEX {
                return Err(bad("an index pointer leads to the wrong kind of page"));
            }
            let mut r = Rd::new(p.payload);
            for _ in 0..p.count {
                let key = r.key()?;
                out.push((key, r.u32()?));
            }
            no = p.next;
        }
        if out.len() != e.leaves as usize {
            return Err(bad("the index does not list every leaf"));
        }
        Ok(out)
    }

    /// One cell, found through the index by reading a single leaf.
    pub fn find(&self, id: u16, key: Key) -> Result<Option<Cell>> {
        let index = self.index(id)?;
        let at = index.partition_point(|(first, _)| *first <= key);
        let Some((_, leaf)) = at.checked_sub(1).and_then(|i| index.get(i)) else {
            return Ok(None);
        };
        Ok(self.leaf_records(*leaf)?.into_iter().find(|(k, _)| *k == key).map(|(_, c)| c))
    }
}

/// Fills the sheets of `skeleton` (a store read from `data/maix.json`) with the cells in `bytes`.
pub fn decode_into(skeleton: &mut Maix, bytes: &[u8]) -> Result<()> {
    let store = PagedStore::open(bytes)?;
    store.verify()?;
    let known: Vec<u16> = skeleton.sheets.iter().map(|s| s.id).collect();
    if store.sheet_ids().iter().any(|id| !known.contains(id)) {
        return Err(bad("pages for a sheet the store does not have"));
    }
    for sheet in &mut skeleton.sheets {
        sheet.cells = store.read_sheet(sheet.id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maix::Sheet;

    fn cell(value: Value, n: u32) -> Cell {
        Cell { value, formula: None, version: n, updated_at: 1_000 + u64::from(n), meta: BTreeMap::new() }
    }

    fn store_with(cells: usize) -> Maix {
        let mut m = Maix::default();
        let a = m.add_sheet("Big");
        m.add_sheet("Empty");
        let b = m.add_sheet("Small");
        let sheet = m.sheets.iter_mut().find(|s| s.id == a).unwrap();
        for i in 0..cells as u32 {
            sheet.cells.insert((i / 7, (i % 7) as u16, u64::from(i % 3)), cell(Value::Int(i64::from(i) * 3), i));
        }
        let small: &mut Sheet = m.sheets.iter_mut().find(|s| s.id == b).unwrap();
        small.cells.insert((0, 0, 0), cell(Value::Text("héllo".into()), 1));
        small.cells.insert((0, 1, 0), Cell { formula: Some("=A1*2".into()), ..cell(Value::Float(2.5), 2) });
        small.cells.insert((1, 0, 9), cell(Value::Bool(true), 3));
        small.cells.insert((2, 0, 0), cell(Value::Date(86_400_000), 4));
        small.cells.insert((3, 0, 0), cell(Value::Null, 5));
        let mut meta = Cell { meta: BTreeMap::new(), ..cell(Value::Int(1), 6) };
        meta.meta.insert("pos".into(), [("tax".to_string(), serde_json::json!(0.18))].into_iter().collect());
        small.cells.insert((4, 0, 0), meta);
        // Too big for a leaf: it goes to overflow pages.
        small.cells.insert((5, 0, 0), cell(Value::Text("long ".repeat(3_000)), 7));
        m
    }

    fn skeleton_of(m: &Maix) -> Maix {
        let mut s = m.skeleton();
        s.profile = crate::maix::PROFILE.into();
        s
    }

    #[test]
    fn the_checksum_is_crc32c() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0);
    }

    #[test]
    fn pages_round_trip_every_kind_of_cell() {
        let m = store_with(5_000);
        let bytes = encode(&m).unwrap();
        assert_eq!(bytes.len() % PAGE, 0);
        let mut back = skeleton_of(&m);
        assert!(back.sheets.iter().all(|s| s.cell_count() == 0));
        decode_into(&mut back, &bytes).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn a_cell_is_found_through_the_index() {
        let m = store_with(20_000);
        let bytes = encode(&m).unwrap();
        let store = PagedStore::open(&bytes).unwrap();
        assert_eq!(store.total_cells, m.cell_total() as u64);
        let big = m.sheets.iter().find(|s| s.name == "Big").unwrap();
        for key in [(0u32, 0u16, 0u64), (1, 3, 0), (1000, 2, 1), (2857, 5, 2)] {
            let want = big.cells.get(&key).cloned();
            assert_eq!(store.find(big.id, key).unwrap(), want, "{key:?}");
        }
        assert_eq!(store.find(big.id, (999_999, 0, 0)).unwrap(), None, "past the last cell");
        let empty = m.sheets.iter().find(|s| s.name == "Empty").unwrap();
        assert_eq!(store.find(empty.id, (0, 0, 0)).unwrap(), None);
        assert!(store.read_sheet(empty.id).unwrap().is_empty());
    }

    #[test]
    fn a_damaged_page_is_refused() {
        let m = store_with(3_000);
        let bytes = encode(&m).unwrap();
        for at in [PAGE + 100, PAGE * 2 + 5, bytes.len() - 7] {
            let mut broken = bytes.clone();
            broken[at] ^= 0x40;
            let mut back = skeleton_of(&m);
            assert!(decode_into(&mut back, &broken).is_err(), "byte {at}");
        }
        assert!(PagedStore::open(&bytes[..bytes.len() - 1]).is_err(), "a cut file");
        assert!(PagedStore::open(&[]).is_err());
    }

    #[test]
    fn pointers_that_lie_are_caught_not_followed() {
        let m = store_with(3_000);
        let mut bytes = encode(&m).unwrap();
        // Make the first leaf point back at itself, then fix its checksum so only the loop is wrong.
        let first_leaf = (1..bytes.len() / PAGE).find(|i| bytes[i * PAGE + 4] == KIND_LEAF).unwrap();
        let start = first_leaf * PAGE;
        bytes[start + 8..start + 12].copy_from_slice(&(first_leaf as u32).to_le_bytes());
        let crc = crc32c(&bytes[start + 4..start + PAGE]);
        bytes[start..start + 4].copy_from_slice(&crc.to_le_bytes());
        let mut back = skeleton_of(&m);
        assert!(decode_into(&mut back, &bytes).is_err());
    }

    #[test]
    fn an_empty_store_has_just_a_header() {
        let m = Maix::default();
        let bytes = encode(&m).unwrap();
        assert_eq!(bytes.len(), PAGE);
        let mut back = skeleton_of(&m);
        decode_into(&mut back, &bytes).unwrap();
        assert_eq!(back, m);
    }
}
