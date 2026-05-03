use crate::storage::Pager;
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

    fn put(&mut self, root: u32, key: i64, value: Vec<u8>, allow_replace: bool) -> DbResult<u32> {
        if root == 0 {
            return Err(DbError::new("root page is 0"));
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
                            return Err(DbError::new(format!("duplicate primary key: {}", key)));
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
                    return Err(DbError::new("leaf overflow on single entry"));
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
                let pos = match internal.entries.binary_search_by_key(&split_key, |(k, _)| *k) {
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
                    return Err(DbError::new("internal overflow on single entry"));
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
            other => Err(DbError::new(format!("unknown page type: {}", other))),
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
            return Err(DbError::new("root page is 0"));
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
                other => return Err(DbError::new(format!("unknown page type: {}", other))),
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
                other => return Err(DbError::new(format!("unknown page type: {}", other))),
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
        return Err(DbError::new("page too small"));
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

fn leaf_fits(page_size: usize, kvs: &[KeyValue]) -> bool {
    let payload_size = kvs
        .iter()
        .fold(LEAF_HEADER_SIZE, |size, kv| size + 8 + 2 + kv.value.len());
    payload_size <= page_size
}

fn internal_fits(page_size: usize, entries: &[(i64, u32)]) -> bool {
    INTERNAL_HEADER_SIZE + entries.len() * INTERNAL_ENTRY_SIZE <= page_size
}

fn decode_leaf(page: &[u8]) -> DbResult<LeafNode> {
    if page.len() < LEAF_HEADER_SIZE {
        return Err(DbError::new("page too small"));
    }
    if page[0] != PAGE_TYPE_LEAF {
        return Err(DbError::new("not a leaf page"));
    }
    let next = u32::from_le_bytes(page[1..5].try_into().unwrap());
    let count = u16::from_le_bytes(page[5..7].try_into().unwrap()) as usize;
    let mut pos = LEAF_HEADER_SIZE;
    let mut kvs = Vec::with_capacity(count);
    for _ in 0..count {
        if pos + 10 > page.len() {
            return Err(DbError::new("leaf decode overflow"));
        }
        let key = i64::from_le_bytes(page[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let len = u16::from_le_bytes(page[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        if pos + len > page.len() {
            return Err(DbError::new("leaf decode value overflow"));
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
        return Err(DbError::new("leaf too large"));
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
        return Err(DbError::new("internal page too small"));
    }
    if page[0] != PAGE_TYPE_INTERNAL {
        return Err(DbError::new("not an internal page"));
    }
    let count = u16::from_le_bytes(page[5..7].try_into().unwrap()) as usize;
    let first_child = u32::from_le_bytes(page[7..11].try_into().unwrap());
    let mut pos = INTERNAL_HEADER_SIZE;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        if pos + INTERNAL_ENTRY_SIZE > page.len() {
            return Err(DbError::new("internal decode overflow"));
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
        return Err(DbError::new("internal too large"));
    }
    let max_entries = (page_size - INTERNAL_HEADER_SIZE) / INTERNAL_ENTRY_SIZE;
    if max_entries < INTERNAL_MAX_ENTRIES_FLOOR {
        return Err(DbError::new("page size too small for internal node"));
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
