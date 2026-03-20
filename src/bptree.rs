use crate::storage::Pager;
use crate::{DbError, DbResult};

const PAGE_TYPE_LEAF: u8 = 1;
const LEAF_HEADER_SIZE: usize = 7;

#[derive(Debug, Clone, PartialEq)]
pub struct KeyValue {
    pub key: i64,
    pub value: Vec<u8>,
}

pub struct Tree<'a> {
    pager: &'a mut Pager,
}

impl<'a> Tree<'a> {
    pub fn new(pager: &'a mut Pager) -> Self {
        Self { pager }
    }

    pub fn get(&mut self, root: u32, key: i64) -> DbResult<Option<Vec<u8>>> {
        let mut leaf_no = self.find_leaf(root, key)?;
        while leaf_no != 0 {
            let page = self.pager.page_data(leaf_no)?;
            let kvs = decode_leaf(&page)?;
            match kvs.binary_search_by_key(&key, |kv| kv.key) {
                Ok(index) => return Ok(Some(kvs[index].value.clone())),
                Err(_) => {
                    leaf_no = leaf_next(&page);
                }
            }
        }
        Ok(None)
    }

    pub fn insert(&mut self, root: u32, key: i64, value: Vec<u8>) -> DbResult<u32> {
        self.put(root, key, value, false)
    }

    pub fn upsert(&mut self, root: u32, key: i64, value: Vec<u8>) -> DbResult<u32> {
        self.put(root, key, value, true)
    }

    pub fn range(&mut self, root: u32, mut from: i64, mut to: i64) -> DbResult<Vec<KeyValue>> {
        if from > to {
            std::mem::swap(&mut from, &mut to);
        }
        let mut out = Vec::new();
        let mut leaf_no = self.find_leaf(root, from)?;
        while leaf_no != 0 {
            let page = self.pager.page_data(leaf_no)?;
            let kvs = decode_leaf(&page)?;
            for kv in kvs {
                if kv.key < from {
                    continue;
                }
                if kv.key > to {
                    return Ok(out);
                }
                out.push(kv);
            }
            leaf_no = leaf_next(&page);
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
        let mut leaf_no = root;
        while leaf_no != 0 {
            let page = self.pager.page_data(leaf_no)?;
            let kvs = decode_leaf(&page)?;
            for kv in kvs {
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
            leaf_no = leaf_next(&page);
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
        let leaf_no = self.find_leaf(root, key)?;
        let current_page = self.pager.page_data(leaf_no)?;
        let mut kvs = decode_leaf(&current_page)?;

        match kvs.binary_search_by_key(&key, |kv| kv.key) {
            Ok(index) => {
                if !allow_replace {
                    return Err(DbError::new(format!("duplicate primary key: {}", key)));
                }
                kvs[index].value = value;
            }
            Err(index) => kvs.insert(index, KeyValue { key, value }),
        }

        if fits_leaf(self.pager.page_size(), &kvs) {
            let next = leaf_next(&current_page);
            let page = encode_leaf(self.pager.page_size(), &kvs, next)?;
            self.pager.write_page(leaf_no, &page, true)?;
            return Ok(root);
        }

        let mid = kvs.len() / 2;
        let left = kvs[..mid].to_vec();
        let right = kvs[mid..].to_vec();
        let new_page_no = self.pager.new_page()?;
        let current_next = leaf_next(&current_page);

        let right_page = encode_leaf(self.pager.page_size(), &right, current_next)?;
        self.pager.write_page(new_page_no, &right_page, true)?;

        let left_page = encode_leaf(self.pager.page_size(), &left, new_page_no)?;
        self.pager.write_page(leaf_no, &left_page, true)?;

        Ok(root)
    }

    fn find_leaf(&mut self, root: u32, key: i64) -> DbResult<u32> {
        if root == 0 {
            return Err(DbError::new("root page is 0"));
        }
        let mut leaf_no = root;
        loop {
            let page = self.pager.page_data(leaf_no)?;
            let kvs = decode_leaf(&page)?;
            if kvs.is_empty() {
                return Ok(leaf_no);
            }
            let last = kvs.last().map(|kv| kv.key).unwrap_or(key);
            let next = leaf_next(&page);
            if key <= last || next == 0 {
                return Ok(leaf_no);
            }
            leaf_no = next;
        }
    }
}

pub fn init_leaf_page(page: &mut [u8]) {
    page.fill(0);
    page[0] = PAGE_TYPE_LEAF;
    page[1..5].copy_from_slice(&0u32.to_le_bytes());
    page[5..7].copy_from_slice(&0u16.to_le_bytes());
}

fn fits_leaf(page_size: usize, kvs: &[KeyValue]) -> bool {
    let payload_size = kvs
        .iter()
        .fold(LEAF_HEADER_SIZE, |size, kv| size + 8 + 2 + kv.value.len());
    payload_size <= page_size
}

fn leaf_next(page: &[u8]) -> u32 {
    u32::from_le_bytes(page[1..5].try_into().unwrap())
}

fn leaf_count(page: &[u8]) -> u16 {
    u16::from_le_bytes(page[5..7].try_into().unwrap())
}

fn decode_leaf(page: &[u8]) -> DbResult<Vec<KeyValue>> {
    if page.len() < LEAF_HEADER_SIZE {
        return Err(DbError::new("page too small"));
    }
    if page[0] != PAGE_TYPE_LEAF {
        return Err(DbError::new("not a leaf page"));
    }
    let count = leaf_count(page) as usize;
    let mut pos = LEAF_HEADER_SIZE;
    let mut out = Vec::with_capacity(count);
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
        out.push(KeyValue {
            key,
            value: page[pos..pos + len].to_vec(),
        });
        pos += len;
    }
    Ok(out)
}

fn encode_leaf(page_size: usize, kvs: &[KeyValue], next: u32) -> DbResult<Vec<u8>> {
    if !fits_leaf(page_size, kvs) {
        return Err(DbError::new("leaf too large"));
    }
    let mut page = vec![0; page_size];
    init_leaf_page(&mut page);
    page[1..5].copy_from_slice(&next.to_le_bytes());
    page[5..7].copy_from_slice(&(kvs.len() as u16).to_le_bytes());
    let mut pos = LEAF_HEADER_SIZE;
    for kv in kvs {
        page[pos..pos + 8].copy_from_slice(&kv.key.to_le_bytes());
        pos += 8;
        page[pos..pos + 2].copy_from_slice(&(kv.value.len() as u16).to_le_bytes());
        pos += 2;
        page[pos..pos + kv.value.len()].copy_from_slice(&kv.value);
        pos += kv.value.len();
    }
    Ok(page)
}
