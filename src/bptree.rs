use crate::storage::{Pager, PAGE_CHECKSUM_BYTES};
use crate::{DbError, DbResult};

const PAGE_TYPE_LEAF: u8 = 1;
const PAGE_TYPE_INTERNAL: u8 = 2;

// LEAF layout:  [type:u8][next:u32][count:u16] + count * ([key:i64][vlen:u16][bytes])
const LEAF_HEADER_SIZE: usize = 7;
// INTERNAL layout: [type:u8][reserved:u32][count:u16][first_child:u32]
//                  + count * ([key:i64][child:u32])
// Semantics: children = count+1. For a probe key K, descend into first_child if
// K < entries[0].key; otherwise into entries[i].child where entries[i].key is
// the largest key <= K.
const INTERNAL_HEADER_SIZE: usize = 11;
const INTERNAL_ENTRY_SIZE: usize = 12;
const INTERNAL_MAX_ENTRIES_FLOOR: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct KeyValue {
    pub key: i64,
    pub value: Vec<u8>,
}

struct LeafNode {
    next: u32,
    kvs: Vec<KeyValue>,
}

struct InternalNode {
    first_child: u32,
    entries: Vec<(i64, u32)>,
}

pub struct Tree<'a> {
    pager: &'a mut Pager,
}

impl<'a> Tree<'a> {
    pub fn new(pager: &'a mut Pager) -> Self {
        Self { pager }
    }

    pub fn get(&mut self, root: u32, key: i64) -> DbResult<Option<Vec<u8>>> {
        if root == 0 {
            return Ok(None);
        }
        let leaf_no = self.find_leaf(root, key)?;
        let page = self.pager.page_data(leaf_no)?;
        let leaf = decode_leaf(&page)?;
        match leaf.kvs.binary_search_by_key(&key, |kv| kv.key) {
            Ok(index) => Ok(Some(leaf.kvs[index].value.clone())),
            Err(_) => Ok(None),
        }
    }

    pub fn insert(&mut self, root: u32, key: i64, value: Vec<u8>) -> DbResult<u32> {
        self.put(root, key, value, false)
    }

    pub fn upsert(&mut self, root: u32, key: i64, value: Vec<u8>) -> DbResult<u32> {
        self.put(root, key, value, true)
    }

    pub fn delete(&mut self, root: u32, key: i64) -> DbResult<bool> {
        if root == 0 {
            return Ok(false);
        }
        let leaf_no = self.find_leaf(root, key)?;
        let page = self.pager.page_data(leaf_no)?;
        let mut leaf = decode_leaf(&page)?;
        let index = match leaf.kvs.binary_search_by_key(&key, |kv| kv.key) {
            Ok(idx) => idx,
            Err(_) => return Ok(false),
        };
        leaf.kvs.remove(index);
        let buf = encode_leaf(self.pager.page_size(), &leaf)?;
        self.pager.write_page(leaf_no, &buf, true)?;
        Ok(true)
    }

    pub fn range(&mut self, root: u32, mut from: i64, mut to: i64) -> DbResult<Vec<KeyValue>> {
        if from > to {
            std::mem::swap(&mut from, &mut to);
        }
        if root == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut leaf_no = self.find_leaf(root, from)?;
        while leaf_no != 0 {
            let page = self.pager.page_data(leaf_no)?;
            let leaf = decode_leaf(&page)?;
            for kv in leaf.kvs {
                if kv.key < from {
                    continue;
                }
                if kv.key > to {
                    return Ok(out);
                }
                out.push(kv);
            }
            leaf_no = leaf.next;
        }
        Ok(out)
    }

    pub fn scan(
        &mut self,
        root: u32,
        offset: usize,
        limit: Option<usize>,
    ) -> DbResult<Vec<KeyValue>> {
        if root == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut skipped = 0usize;
        let mut leaf_no = self.leftmost_leaf(root)?;
        while leaf_no != 0 {
            let page = self.pager.page_data(leaf_no)?;
            let leaf = decode_leaf(&page)?;
            for kv in leaf.kvs {
                if skipped < offset {
                    skipped += 1;
                    continue;
                }
                out.push(kv);
                if let Some(max) = limit {
                    if out.len() >= max {
                        return Ok(out);
                    }
                }
            }
            leaf_no = leaf.next;
        }
        Ok(out)
    }

    pub fn all(&mut self, root: u32) -> DbResult<Vec<KeyValue>> {
        self.scan(root, 0, None)
    }

    /// Lazy cursor over every entry in the tree, in key order.
    ///
    /// The cursor borrows the Pager mutably for its lifetime — while
    /// it is alive no other operation can write through the Pager.
    /// SELECT (read-only) is the natural fit; patterns that read
    /// rows AND mutate the same B+Tree (CREATE INDEX backfill,
    /// INTEGRITY CHECK) keep using the materializing helpers
    /// (`scan / range / all`).
    ///
    /// `LeafCursor` implements `Iterator`, so consumers compose with
    /// the standard `skip(offset).take(limit)` to express
    /// LIMIT/OFFSET windowing without ever loading the rows past the
    /// window — that's the whole point of the cursor.
    pub fn cursor_full(mut self, root: u32) -> DbResult<LeafCursor<'a>> {
        if root == 0 {
            let Tree { pager } = self;
            return Ok(LeafCursor::empty(pager));
        }
        let leaf = self.leftmost_leaf(root)?;
        let Tree { pager } = self;
        LeafCursor::open(pager, leaf, None, None)
    }

    /// Lazy cursor over entries with key in `[from, to]` (both bounds
    /// inclusive — same semantics as `WHERE pk BETWEEN from AND to`).
    pub fn cursor_range(
        mut self,
        root: u32,
        mut from: i64,
        mut to: i64,
    ) -> DbResult<LeafCursor<'a>> {
        if from > to {
            std::mem::swap(&mut from, &mut to);
        }
        if root == 0 {
            let Tree { pager } = self;
            return Ok(LeafCursor::empty(pager));
        }
        let leaf = self.find_leaf(root, from)?;
        let Tree { pager } = self;
        LeafCursor::open(pager, leaf, Some(from), Some(to))
    }

    fn put(&mut self, root: u32, key: i64, value: Vec<u8>, allow_replace: bool) -> DbResult<u32> {
        if root == 0 {
            return Err(DbError::new(
                "página raíz del B+Tree es 0: el catálogo no registró un root válido para esta tabla/índice",
            ));
        }
        if let Some((split_key, right_no)) = self.put_recursive(root, key, value, allow_replace)? {
            self.split_root(root, split_key, right_no)?;
        }
        Ok(root)
    }

    fn put_recursive(
        &mut self,
        page_no: u32,
        key: i64,
        value: Vec<u8>,
        allow_replace: bool,
    ) -> DbResult<Option<(i64, u32)>> {
        let page = self.pager.page_data(page_no)?;
        match page_type(&page)? {
            PAGE_TYPE_LEAF => {
                let mut leaf = decode_leaf(&page)?;
                match leaf.kvs.binary_search_by_key(&key, |kv| kv.key) {
                    Ok(idx) => {
                        if !allow_replace {
                            return Err(crate::errors::coded(
                                crate::errors::codes::DUPLICATE_PRIMARY_KEY,
                                format!(
                                    "PRIMARY KEY duplicada: la clave {} ya existe en la tabla",
                                    key
                                ),
                            ));
                        }
                        leaf.kvs[idx].value = value;
                    }
                    Err(idx) => leaf.kvs.insert(idx, KeyValue { key, value }),
                }
                if leaf_fits(self.pager.page_size(), &leaf.kvs) {
                    let buf = encode_leaf(self.pager.page_size(), &leaf)?;
                    self.pager.write_page(page_no, &buf, true)?;
                    return Ok(None);
                }
                let split = leaf.kvs.len() / 2;
                if split == 0 {
                    // Issue #2 (2026-05-27): cuando un único KV excede
                    // una página completa, ocurre típicamente porque
                    // un bucket de índice secundario sobre una columna
                    // de baja cardinalidad acumula miles de row_ids
                    // bajo un mismo `hash(value)`. Hoy no hay overflow
                    // chain — el fix completo requiere reescribir el
                    // bucket layer. Workarounds disponibles:
                    //   - filtrar más antes de indexar (partial index, P2)
                    //   - usar la columna como segunda en un índice
                    //     compuesto sobre una columna de cardinalidad alta
                    //   - aceptar el full-scan post-Issue-#3
                    return Err(DbError::new(format!(
                        "hoja B+Tree (página {}) no admite una sola entrada (key={}): \
                         el valor excede el espacio útil de la página. \
                         Causa típica (Issue #2 del BENCHMARK): índice secundario sobre \
                         una columna de baja cardinalidad con muchos row_ids por valor — \
                         el bucket no entra en una sola página y no hay overflow chain todavía. \
                         Workaround: filtrá la columna o usá un índice compuesto cuya \
                         primera columna sea de alta cardinalidad.",
                        page_no, key
                    )));
                }
                let right_kvs = leaf.kvs.split_off(split);
                let right_first_key = right_kvs[0].key;

                let right_no = self.pager.new_page()?;
                let right_leaf = LeafNode {
                    next: leaf.next,
                    kvs: right_kvs,
                };
                let right_buf = encode_leaf(self.pager.page_size(), &right_leaf)?;
                self.pager.write_page(right_no, &right_buf, true)?;

                let left_leaf = LeafNode {
                    next: right_no,
                    kvs: leaf.kvs,
                };
                let left_buf = encode_leaf(self.pager.page_size(), &left_leaf)?;
                self.pager.write_page(page_no, &left_buf, true)?;
                Ok(Some((right_first_key, right_no)))
            }
            PAGE_TYPE_INTERNAL => {
                let mut internal = decode_internal(&page)?;
                let child_no = pick_child(&internal, key);
                let promoted = self.put_recursive(child_no, key, value, allow_replace)?;
                let Some((split_key, right_no)) = promoted else {
                    return Ok(None);
                };
                let pos = match internal
                    .entries
                    .binary_search_by_key(&split_key, |(k, _)| *k)
                {
                    Ok(_) => {
                        return Err(DbError::new(
                            "split key colisiona con entrada interna existente",
                        ))
                    }
                    Err(idx) => idx,
                };
                internal.entries.insert(pos, (split_key, right_no));
                if internal_fits(self.pager.page_size(), &internal.entries) {
                    let buf = encode_internal(self.pager.page_size(), &internal)?;
                    self.pager.write_page(page_no, &buf, true)?;
                    return Ok(None);
                }

                let mid = internal.entries.len() / 2;
                if mid == 0 {
                    return Err(DbError::new(format!(
                        "nodo interno del B+Tree (página {}) no admite una sola entrada (split_key={}): \
                         el separador excede el espacio útil de la página",
                        page_no, split_key
                    )));
                }
                let right_entries = internal.entries.split_off(mid);
                let (promote_key, promote_child) = right_entries[0];
                let right_first_child = promote_child;
                let right_internal = InternalNode {
                    first_child: right_first_child,
                    entries: right_entries[1..].to_vec(),
                };
                let right_no = self.pager.new_page()?;
                let right_buf = encode_internal(self.pager.page_size(), &right_internal)?;
                self.pager.write_page(right_no, &right_buf, true)?;

                let left_buf = encode_internal(self.pager.page_size(), &internal)?;
                self.pager.write_page(page_no, &left_buf, true)?;
                Ok(Some((promote_key, right_no)))
            }
            other => Err(DbError::new(format!(
                "tipo de página desconocido: {:#04x} (esperaba LEAF={:#04x} o INTERNAL={:#04x})",
                other, PAGE_TYPE_LEAF, PAGE_TYPE_INTERNAL
            ))),
        }
    }

    fn split_root(&mut self, root_no: u32, split_key: i64, right_no: u32) -> DbResult<()> {
        // Copy current root content to a new page so root_no stays stable as new internal.
        let original = self.pager.page_data(root_no)?;
        let left_copy_no = self.pager.new_page()?;
        self.pager.write_page(left_copy_no, &original, true)?;

        let new_root = InternalNode {
            first_child: left_copy_no,
            entries: vec![(split_key, right_no)],
        };
        let buf = encode_internal(self.pager.page_size(), &new_root)?;
        self.pager.write_page(root_no, &buf, true)?;
        Ok(())
    }

    fn find_leaf(&mut self, root: u32, key: i64) -> DbResult<u32> {
        if root == 0 {
            return Err(DbError::new(
                "página raíz del B+Tree es 0: el catálogo no registró un root válido para esta tabla/índice",
            ));
        }
        let mut current = root;
        loop {
            let page = self.pager.page_data(current)?;
            match page_type(&page)? {
                PAGE_TYPE_LEAF => return Ok(current),
                PAGE_TYPE_INTERNAL => {
                    let internal = decode_internal(&page)?;
                    current = pick_child(&internal, key);
                }
                other => {
                    return Err(DbError::new(format!(
                        "tipo de página desconocido: {:#04x} (esperaba LEAF={:#04x} o INTERNAL={:#04x})",
                        other, PAGE_TYPE_LEAF, PAGE_TYPE_INTERNAL
                    )))
                }
            }
        }
    }

    fn leftmost_leaf(&mut self, root: u32) -> DbResult<u32> {
        if root == 0 {
            return Ok(0);
        }
        let mut current = root;
        loop {
            let page = self.pager.page_data(current)?;
            match page_type(&page)? {
                PAGE_TYPE_LEAF => return Ok(current),
                PAGE_TYPE_INTERNAL => {
                    let internal = decode_internal(&page)?;
                    current = internal.first_child;
                }
                other => {
                    return Err(DbError::new(format!(
                        "tipo de página desconocido: {:#04x} (esperaba LEAF={:#04x} o INTERNAL={:#04x})",
                        other, PAGE_TYPE_LEAF, PAGE_TYPE_INTERNAL
                    )))
                }
            }
        }
    }
}

/// Lazy iterator over the entries of a B+Tree, anchored at one root.
///
/// **Why this exists.** The materializing helpers (`Tree::scan/range/all`)
/// load every matching entry into a `Vec<KeyValue>` before returning.
/// That works for small tables, but a `SELECT … LIMIT 10` against a
/// million-row table pays for the full million in RAM and CPU just to
/// throw 999.990 away. `LeafCursor` walks leaves on demand: each call
/// to `next()` advances within the current leaf, and only loads the
/// next leaf via the chain pointer when the current one is exhausted.
/// Combine with `Iterator::skip(offset).take(limit)` to express
/// LIMIT/OFFSET windowing that is genuinely O(limit + offset) in I/O,
/// not O(table_size).
///
/// **Borrow semantics.** The cursor holds the Pager mutably for its
/// lifetime, so while a cursor is live no other write through the same
/// Pager can run. SELECT (read-only) is the natural fit. Code paths
/// that read rows AND mutate the same B+Tree (CREATE INDEX backfill,
/// INTEGRITY CHECK) keep using the materializing helpers — they need
/// to drop the read borrow before the write borrow can start.
pub struct LeafCursor<'a> {
    pager: &'a mut Pager,
    /// Page number of the next leaf to load when the current buffer
    /// is drained. `0` means there is no next leaf (we hit the right
    /// edge of the tree).
    next_leaf: u32,
    /// Entries decoded from the current leaf. Drained left-to-right.
    buf: Vec<KeyValue>,
    /// Position within `buf` of the next entry to yield.
    pos: usize,
    /// Lower bound for `cursor_range` — used to skip the prefix of
    /// the starting leaf whose keys fall below `from`. `None` means
    /// no lower bound (full scan).
    lower: Option<i64>,
    /// Upper bound for `cursor_range`, inclusive. The cursor stops
    /// as soon as it encounters a key strictly greater than this.
    upper: Option<i64>,
    /// Sticky flag set on EOF or after a load error so subsequent
    /// `next()` calls return `None` cleanly.
    done: bool,
}

impl<'a> LeafCursor<'a> {
    fn empty(pager: &'a mut Pager) -> Self {
        Self {
            pager,
            next_leaf: 0,
            buf: Vec::new(),
            pos: 0,
            lower: None,
            upper: None,
            done: true,
        }
    }

    fn open(
        pager: &'a mut Pager,
        leaf: u32,
        lower: Option<i64>,
        upper: Option<i64>,
    ) -> DbResult<Self> {
        let mut me = Self {
            pager,
            next_leaf: leaf,
            buf: Vec::new(),
            pos: 0,
            lower,
            upper,
            done: leaf == 0,
        };
        if !me.done {
            me.load_current()?;
            // Skip the prefix of the starting leaf that falls below
            // the lower bound — only relevant for cursor_range.
            if let Some(low) = me.lower {
                while me.pos < me.buf.len() && me.buf[me.pos].key < low {
                    me.pos += 1;
                }
            }
        }
        Ok(me)
    }

    fn load_current(&mut self) -> DbResult<()> {
        if self.next_leaf == 0 {
            self.done = true;
            return Ok(());
        }
        let page = self.pager.page_data(self.next_leaf)?;
        let leaf = decode_leaf(&page)?;
        self.buf = leaf.kvs;
        self.pos = 0;
        self.next_leaf = leaf.next;

        // Prefetch one leaf ahead. Synchronous read into the PageCache
        // (ADR-0009) so the next `load_current` becomes a cache hit and
        // skips a disk syscall. We also surface the streaming pattern
        // to the kernel's readahead heuristic earlier — most modern
        // filesystems will then keep the pipeline warm. Best-effort:
        // a CRC failure here is swallowed because the real read happens
        // on the next iteration and will surface the same error then.
        if self.next_leaf != 0 {
            let _ = self.pager.page_data(self.next_leaf);
        }
        Ok(())
    }
}

impl<'a> Iterator for LeafCursor<'a> {
    type Item = DbResult<KeyValue>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        loop {
            if self.pos < self.buf.len() {
                let kv = self.buf[self.pos].clone();
                self.pos += 1;
                if let Some(up) = self.upper {
                    if kv.key > up {
                        // We've crossed the right bound; latch done so
                        // any further `next()` is a clean None.
                        self.done = true;
                        return None;
                    }
                }
                return Some(Ok(kv));
            }
            if self.next_leaf == 0 {
                self.done = true;
                return None;
            }
            if let Err(err) = self.load_current() {
                // Surface the load error once and latch done; the
                // cursor doesn't try to recover from CRC failures.
                self.done = true;
                return Some(Err(err));
            }
        }
    }
}

pub fn init_leaf_page(page: &mut [u8]) {
    page.fill(0);
    page[0] = PAGE_TYPE_LEAF;
    page[1..5].copy_from_slice(&0u32.to_le_bytes());
    page[5..7].copy_from_slice(&0u16.to_le_bytes());
}

fn page_type(page: &[u8]) -> DbResult<u8> {
    if page.is_empty() {
        return Err(DbError::new(
            "página vacía: no se puede leer el byte de page_type",
        ));
    }
    Ok(page[0])
}

fn pick_child(internal: &InternalNode, key: i64) -> u32 {
    if internal.entries.is_empty() {
        return internal.first_child;
    }
    if key < internal.entries[0].0 {
        return internal.first_child;
    }
    let mut chosen = internal.entries[0].1;
    for (k, child) in internal.entries.iter().skip(1) {
        if key < *k {
            return chosen;
        }
        chosen = *child;
    }
    chosen
}

fn usable_page_size(page_size: usize) -> usize {
    page_size - PAGE_CHECKSUM_BYTES
}

fn leaf_fits(page_size: usize, kvs: &[KeyValue]) -> bool {
    let payload_size = kvs
        .iter()
        .fold(LEAF_HEADER_SIZE, |size, kv| size + 8 + 2 + kv.value.len());
    payload_size <= usable_page_size(page_size)
}

fn internal_fits(page_size: usize, entries: &[(i64, u32)]) -> bool {
    INTERNAL_HEADER_SIZE + entries.len() * INTERNAL_ENTRY_SIZE <= usable_page_size(page_size)
}

fn decode_leaf(page: &[u8]) -> DbResult<LeafNode> {
    if page.len() < LEAF_HEADER_SIZE {
        return Err(DbError::new(format!(
            "página de hoja corrupta: tiene {} bytes, se requieren al menos {} para el header",
            page.len(),
            LEAF_HEADER_SIZE
        )));
    }
    if page[0] != PAGE_TYPE_LEAF {
        return Err(DbError::new(format!(
            "página corrupta: page_type={:#04x} (esperaba LEAF={:#04x})",
            page[0], PAGE_TYPE_LEAF
        )));
    }
    let next = u32::from_le_bytes(page[1..5].try_into().unwrap());
    let count = u16::from_le_bytes(page[5..7].try_into().unwrap()) as usize;
    let mut pos = LEAF_HEADER_SIZE;
    let mut kvs = Vec::with_capacity(count);
    for i in 0..count {
        if pos + 10 > page.len() {
            return Err(DbError::new(format!(
                "hoja corrupta: overflow decodificando entrada {} de {} en offset {} \
                 (page_len={})",
                i,
                count,
                pos,
                page.len()
            )));
        }
        let key = i64::from_le_bytes(page[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let len = u16::from_le_bytes(page[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        if pos + len > page.len() {
            return Err(DbError::new(format!(
                "hoja corrupta: valor de la entrada {} (key={}) declara len={} \
                 pero solo quedan {} bytes en la página",
                i,
                key,
                len,
                page.len() - pos
            )));
        }
        kvs.push(KeyValue {
            key,
            value: page[pos..pos + len].to_vec(),
        });
        pos += len;
    }
    Ok(LeafNode { next, kvs })
}

fn encode_leaf(page_size: usize, leaf: &LeafNode) -> DbResult<Vec<u8>> {
    if !leaf_fits(page_size, &leaf.kvs) {
        return Err(DbError::new(format!(
            "hoja B+Tree no entra en una página: tiene {} entradas, page_size={}; \
             el caller debió splittear antes de llamar a encode_leaf()",
            leaf.kvs.len(),
            page_size
        )));
    }
    let mut page = vec![0; page_size];
    page[0] = PAGE_TYPE_LEAF;
    page[1..5].copy_from_slice(&leaf.next.to_le_bytes());
    page[5..7].copy_from_slice(&(leaf.kvs.len() as u16).to_le_bytes());
    let mut pos = LEAF_HEADER_SIZE;
    for kv in &leaf.kvs {
        page[pos..pos + 8].copy_from_slice(&kv.key.to_le_bytes());
        pos += 8;
        page[pos..pos + 2].copy_from_slice(&(kv.value.len() as u16).to_le_bytes());
        pos += 2;
        page[pos..pos + kv.value.len()].copy_from_slice(&kv.value);
        pos += kv.value.len();
    }
    Ok(page)
}

fn decode_internal(page: &[u8]) -> DbResult<InternalNode> {
    if page.len() < INTERNAL_HEADER_SIZE {
        return Err(DbError::new(format!(
            "página interna corrupta: tiene {} bytes, se requieren al menos {} para el header",
            page.len(),
            INTERNAL_HEADER_SIZE
        )));
    }
    if page[0] != PAGE_TYPE_INTERNAL {
        return Err(DbError::new(format!(
            "página corrupta: page_type={:#04x} (esperaba INTERNAL={:#04x})",
            page[0], PAGE_TYPE_INTERNAL
        )));
    }
    let count = u16::from_le_bytes(page[5..7].try_into().unwrap()) as usize;
    let first_child = u32::from_le_bytes(page[7..11].try_into().unwrap());
    let mut pos = INTERNAL_HEADER_SIZE;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        if pos + INTERNAL_ENTRY_SIZE > page.len() {
            return Err(DbError::new(format!(
                "página interna corrupta: overflow decodificando entrada {} de {} \
                 en offset {} (page_len={})",
                i,
                count,
                pos,
                page.len()
            )));
        }
        let key = i64::from_le_bytes(page[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let child = u32::from_le_bytes(page[pos..pos + 4].try_into().unwrap());
        pos += 4;
        entries.push((key, child));
    }
    Ok(InternalNode {
        first_child,
        entries,
    })
}

fn encode_internal(page_size: usize, internal: &InternalNode) -> DbResult<Vec<u8>> {
    if !internal_fits(page_size, &internal.entries) {
        return Err(DbError::new(format!(
            "nodo interno del B+Tree no entra en una página: tiene {} entradas, page_size={}; \
             el caller debió splittear antes de llamar a encode_internal()",
            internal.entries.len(),
            page_size
        )));
    }
    let max_entries = (usable_page_size(page_size) - INTERNAL_HEADER_SIZE) / INTERNAL_ENTRY_SIZE;
    if max_entries < INTERNAL_MAX_ENTRIES_FLOOR {
        return Err(DbError::new(format!(
            "page_size demasiado chico para nodos internos: caben {} entradas, \
             el motor requiere al menos {} (page_size={})",
            max_entries, INTERNAL_MAX_ENTRIES_FLOOR, page_size
        )));
    }
    let mut page = vec![0; page_size];
    page[0] = PAGE_TYPE_INTERNAL;
    page[1..5].copy_from_slice(&0u32.to_le_bytes());
    page[5..7].copy_from_slice(&(internal.entries.len() as u16).to_le_bytes());
    page[7..11].copy_from_slice(&internal.first_child.to_le_bytes());
    let mut pos = INTERNAL_HEADER_SIZE;
    for (key, child) in &internal.entries {
        page[pos..pos + 8].copy_from_slice(&key.to_le_bytes());
        pos += 8;
        page[pos..pos + 4].copy_from_slice(&child.to_le_bytes());
        pos += 4;
    }
    Ok(page)
}
