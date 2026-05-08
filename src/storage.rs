use crate::{DbError, DbResult};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const PAGE_SIZE_DEFAULT: usize = 4096;

/// Maximum number of cached pages per Pager when not configured
/// otherwise. At the default page size of 4 KB this caps the cache
/// at roughly 4 MB per open DB — bounded enough for a long-running
/// server with dozens of DBs, generous enough that small workloads
/// never have to read a hot page twice.
///
/// Why this number: SQLite defaults to 2000 pages (~8 MB at 4 KB);
/// PostgreSQL's `shared_buffers` defaults to 128 MB. We're an embedded
/// engine biased toward many small DBs co-resident, so a smaller per-DB
/// cap is the right knob. Tunable via `Pager::set_cache_capacity`.
pub const DEFAULT_CACHE_PAGES: usize = 1024;
pub const MAGIC: &[u8; 8] = b"GABYSQL1";
// File-format version. Bumped to:
//   2 -> moved catalog hashing from DefaultHasher to FNV-1a-64.
//   3 -> reserved last 4 bytes of every page for a CRC32-IEEE checksum
//        and added a per-record CRC to the WAL.
//   4 -> TableMeta carries a list of secondary indexes; their on-disk
//        B+Tree pages live alongside the table's own root page.
//   5 -> Column carries `not_null` + optional `DEFAULT` literal; IndexMeta
//        carries a `unique` flag (auto-set by inline UNIQUE column
//        constraints and by `CREATE UNIQUE INDEX`). V4 files are rejected
//        on open — no automatic upgrade.
//   6 -> Column carries an optional `FOREIGN KEY` (target table + target
//        column + ON DELETE action). Single-column FKs only; target must
//        be the parent table's PRIMARY KEY. V5 files are rejected on
//        open — no automatic upgrade.
pub const VERSION: u32 = 6;

/// Trailer used inside every page on disk for the CRC32 checksum.
pub const PAGE_CHECKSUM_BYTES: usize = 4;

const WAL_REC_PAGE: u8 = 1;
const WAL_REC_COMMIT: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub page_size: u16,
    pub page_count: u32,
    pub catalog_root_page: u32,
}

impl Default for Header {
    fn default() -> Self {
        Self::new()
    }
}

impl Header {
    pub fn new() -> Self {
        Self {
            version: VERSION,
            page_size: PAGE_SIZE_DEFAULT as u16,
            page_count: 1,
            catalog_root_page: 0,
        }
    }

    pub fn encode_into(&self, dst: &mut [u8]) -> DbResult<()> {
        if dst.len() < PAGE_SIZE_DEFAULT {
            return Err(DbError::new("header page demasiado pequeña"));
        }
        dst.fill(0);
        dst[0..8].copy_from_slice(MAGIC);
        dst[8..12].copy_from_slice(&self.version.to_le_bytes());
        dst[12..14].copy_from_slice(&self.page_size.to_le_bytes());
        dst[16..20].copy_from_slice(&self.page_count.to_le_bytes());
        dst[20..24].copy_from_slice(&self.catalog_root_page.to_le_bytes());
        Ok(())
    }

    pub fn decode(src: &[u8]) -> DbResult<Self> {
        if src.len() < 24 {
            return Err(DbError::new("header demasiado pequeño"));
        }
        if &src[0..8] != MAGIC {
            return Err(DbError::new("bad magic (not gabysql db)"));
        }
        let version = u32::from_le_bytes(src[8..12].try_into().unwrap());
        if version != VERSION {
            return Err(DbError::new(format!(
                "unsupported gabysql file format: version={} (expected {}). \
                 Re-create the database with the current binary.",
                version, VERSION
            )));
        }
        let page_size = u16::from_le_bytes(src[12..14].try_into().unwrap());
        // Format v3 only supports the default page size. The field is kept on
        // disk so a future format revision can lift the constraint without
        // another binary header change.
        if page_size as usize != PAGE_SIZE_DEFAULT {
            return Err(DbError::new(format!(
                "unsupported page_size={}; this build requires {}",
                page_size, PAGE_SIZE_DEFAULT
            )));
        }
        Ok(Self {
            version,
            page_size,
            page_count: u32::from_le_bytes(src[16..20].try_into().unwrap()),
            catalog_root_page: u32::from_le_bytes(src[20..24].try_into().unwrap()),
        })
    }
}

#[derive(Clone)]
struct CachedPage {
    data: Vec<u8>,
    dirty: bool,
}

/// Bounded page cache with clean-only LRU eviction.
///
/// The pre-block-10 implementation was a `BTreeMap<u32, CachedPage>`
/// that grew without bound. In a long-running `gabysql-server` with N
/// DBs, an `INTEGRITY CHECK` (or any large scan) would touch every
/// page of every DB and pin it forever in RAM — a silent memory leak
/// proportional to the working set, not to the configured limit.
///
/// This cache is fixed-capacity. On insert when full, it evicts the
/// least-recently-used **clean** page. Dirty pages are never evicted
/// — they belong to the open transaction and must reach the WAL
/// before they can be dropped. If the cache is full of dirty pages
/// the cache temporarily exceeds capacity rather than risk losing a
/// pending write; the overflow drains naturally on commit when every
/// page transitions to clean.
///
/// LRU is tracked with a monotonic counter (cheap O(1) update); the
/// eviction scan is O(N) but only runs when the cache is at capacity
/// and only over the small fixed cap. For 1024 entries that's a
/// handful of microseconds per eviction — acceptable for an embedded
/// engine and far simpler than a doubly-linked list with manual
/// indices.
struct PageCache {
    capacity: usize,
    map: HashMap<u32, CacheSlot>,
    counter: u64,
}

struct CacheSlot {
    page: CachedPage,
    last_access: u64,
}

impl PageCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            map: HashMap::new(),
            counter: 0,
        }
    }

    fn touch(&mut self, no: u32) {
        if self.map.contains_key(&no) {
            self.counter += 1;
            if let Some(slot) = self.map.get_mut(&no) {
                slot.last_access = self.counter;
            }
        }
    }

    fn get(&mut self, no: u32) -> Option<&CachedPage> {
        self.touch(no);
        self.map.get(&no).map(|s| &s.page)
    }

    fn get_mut(&mut self, no: u32) -> Option<&mut CachedPage> {
        self.touch(no);
        self.map.get_mut(&no).map(|s| &mut s.page)
    }

    fn contains_key(&self, no: u32) -> bool {
        self.map.contains_key(&no)
    }

    /// Insert or replace the cached page for `no`. When the cache is
    /// at capacity and `no` is new, first attempt to evict the LRU
    /// clean page; if every cached page is dirty (mid-transaction
    /// edge case), allow the cache to overflow temporarily — losing
    /// a dirty page would corrupt the database.
    fn insert(&mut self, no: u32, page: CachedPage) {
        if !self.map.contains_key(&no) && self.map.len() >= self.capacity {
            let victim = self
                .map
                .iter()
                .filter(|(_, slot)| !slot.page.dirty)
                .min_by_key(|(_, slot)| slot.last_access)
                .map(|(k, _)| *k);
            if let Some(k) = victim {
                self.map.remove(&k);
            }
        }
        self.counter += 1;
        self.map.insert(
            no,
            CacheSlot {
                page,
                last_access: self.counter,
            },
        );
    }

    fn clear(&mut self) {
        self.map.clear();
    }

    /// Snapshot of (page_no, page) for every dirty entry. Used by
    /// `commit` to pull the WAL payload before fsync-ing.
    fn dirty_snapshot(&self) -> Vec<(u32, CachedPage)> {
        self.map
            .iter()
            .filter(|(_, slot)| slot.page.dirty)
            .map(|(k, slot)| (*k, slot.page.clone()))
            .collect()
    }

    fn mark_all_clean(&mut self) {
        for slot in self.map.values_mut() {
            slot.page.dirty = false;
        }
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn capacity(&self) -> usize {
        self.capacity
    }
}

pub struct Pager {
    path: PathBuf,
    file: File,
    wal: Option<Wal>,
    header: Header,
    cache: PageCache,
    in_tx: bool,
}

impl Pager {
    /// Create a brand-new database file. Refuses to overwrite an existing
    /// file — callers must explicitly remove the old DB first or use
    /// [`Pager::create_force`] when overwrite is the intent.
    pub fn create(path: impl AsRef<Path>) -> DbResult<Self> {
        Self::create_internal(path, false)
    }

    /// Create a database file, removing any existing one at the path. Only
    /// use this when the caller is intentionally discarding the old file.
    pub fn create_force(path: impl AsRef<Path>) -> DbResult<Self> {
        Self::create_internal(path, true)
    }

    fn create_internal(path: impl AsRef<Path>, force: bool) -> DbResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        if !force && path.exists() {
            return Err(DbError::new(format!(
                "refusing to overwrite existing database: {}. \
                 Delete it first or use create_force().",
                path.display()
            )));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&path)?;

        let header = Header::new();
        let mut page0 = vec![0; PAGE_SIZE_DEFAULT];
        header.encode_into(&mut page0)?;
        finalize_page_checksum(&mut page0);
        file.write_all(&page0)?;
        file.sync_all()?;

        Ok(Self {
            path,
            file,
            wal: None,
            header,
            cache: PageCache::new(DEFAULT_CACHE_PAGES),
            in_tx: false,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> DbResult<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut page0 = vec![0; PAGE_SIZE_DEFAULT];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut page0)?;
        verify_page_checksum(&page0)
            .map_err(|err| DbError::new(format!("header page corrupt: {}", err)))?;
        let mut header = Header::decode(&page0)?;

        let wal_path = wal_path_for(&path);
        if wal_path.exists() {
            let mut wal = Wal::open(&wal_path)?;
            if wal.has_commit() {
                wal.replay_to(&mut file, header.page_size as usize)?;
                file.sync_all()?;
            }
            drop(wal);
            let _ = fs::remove_file(&wal_path);

            let mut refreshed = vec![0; PAGE_SIZE_DEFAULT];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut refreshed)?;
            verify_page_checksum(&refreshed).map_err(|err| {
                DbError::new(format!("header page corrupt after WAL replay: {}", err))
            })?;
            header = Header::decode(&refreshed)?;
        }

        Ok(Self {
            path,
            file,
            wal: None,
            header,
            cache: PageCache::new(DEFAULT_CACHE_PAGES),
            in_tx: false,
        })
    }

    pub fn close(&mut self) -> DbResult<()> {
        if self.in_tx {
            self.rollback()?;
        }
        self.file.sync_all()?;
        Ok(())
    }

    pub fn header(&self) -> Header {
        self.header.clone()
    }

    pub fn page_size(&self) -> usize {
        self.header.page_size as usize
    }

    pub fn begin(&mut self) -> DbResult<()> {
        if self.in_tx {
            return Err(DbError::new("tx already started"));
        }
        self.wal = Some(Wal::create(wal_path_for(&self.path))?);
        self.in_tx = true;
        Ok(())
    }

    pub fn commit(&mut self) -> DbResult<()> {
        if !self.in_tx {
            return Err(DbError::new("no active tx"));
        }

        // Materialize dirty pages and finalize their checksum trailer
        // once, before they hit either WAL or main file. The cache
        // returns a Vec snapshot so we don't hold a borrow across
        // the WAL writes below.
        let mut dirty_pages: Vec<(u32, Vec<u8>)> = self
            .cache
            .dirty_snapshot()
            .into_iter()
            .map(|(no, page)| (no, page.data))
            .collect();
        for (_, data) in dirty_pages.iter_mut() {
            finalize_page_checksum(data);
        }

        let wal = self
            .wal
            .as_mut()
            .ok_or_else(|| DbError::new("wal no inicializado"))?;

        for (no, data) in &dirty_pages {
            wal.write_page(*no, data)?;
        }
        wal.write_commit()?;
        wal.sync()?;

        for (no, data) in &dirty_pages {
            self.file.seek(SeekFrom::Start(self.page_offset(*no)))?;
            self.file.write_all(data)?;
        }
        self.file.sync_all()?;

        // After commit, every cached page is in sync with disk and
        // becomes evictable by the LRU.
        self.cache.mark_all_clean();

        self.wal = None;
        let _ = fs::remove_file(wal_path_for(&self.path));
        self.in_tx = false;
        Ok(())
    }

    pub fn rollback(&mut self) -> DbResult<()> {
        if !self.in_tx {
            return Err(DbError::new("no active tx"));
        }
        self.cache.clear();
        self.wal = None;
        let _ = fs::remove_file(wal_path_for(&self.path));

        let mut page0 = vec![0; PAGE_SIZE_DEFAULT];
        self.file.seek(SeekFrom::Start(0))?;
        self.file.read_exact(&mut page0)?;
        verify_page_checksum(&page0)?;
        self.header = Header::decode(&page0)?;
        self.in_tx = false;
        Ok(())
    }

    pub fn page_data(&mut self, no: u32) -> DbResult<Vec<u8>> {
        self.ensure_page_loaded(no)?;
        Ok(self
            .cache
            .get(no)
            .ok_or_else(|| DbError::new("page cache inconsistente"))?
            .data
            .clone())
    }

    pub fn write_page(&mut self, no: u32, data: &[u8], dirty: bool) -> DbResult<()> {
        if data.len() != self.page_size() {
            return Err(DbError::new("tamaño de página inválido"));
        }
        self.ensure_page_loaded(no)?;
        let page = self
            .cache
            .get_mut(no)
            .ok_or_else(|| DbError::new("page cache inconsistente"))?;
        page.data.copy_from_slice(data);
        page.dirty = dirty;
        Ok(())
    }

    pub fn mark_dirty(&mut self, no: u32) -> DbResult<()> {
        self.ensure_page_loaded(no)?;
        let page = self
            .cache
            .get_mut(no)
            .ok_or_else(|| DbError::new("page cache inconsistente"))?;
        page.dirty = true;
        Ok(())
    }

    /// Reconfigure the LRU cache capacity (in pages). Default is
    /// [`DEFAULT_CACHE_PAGES`]. Callers can shrink for memory-tight
    /// embeddings or grow for warm working sets. Eviction kicks in
    /// the next time `insert` runs against a full cache.
    pub fn set_cache_capacity(&mut self, capacity: usize) {
        self.cache.capacity = capacity.max(1);
    }

    /// Number of cached pages right now. Mostly useful for tests and
    /// `INTEGRITY CHECK`-style introspection.
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// Maximum number of cached pages. Hit by `cache_len()` and
    /// then eviction starts (clean-page LRU first).
    pub fn cache_capacity(&self) -> usize {
        self.cache.capacity()
    }

    pub fn new_page(&mut self) -> DbResult<u32> {
        if !self.in_tx {
            return Err(DbError::new("NewPage requires tx"));
        }
        let no = self.header.page_count;
        self.header.page_count += 1;
        self.refresh_header_page()?;
        self.cache.insert(
            no,
            CachedPage {
                data: vec![0; self.page_size()],
                dirty: true,
            },
        );
        Ok(no)
    }

    pub fn set_catalog_root_page(&mut self, page_no: u32) -> DbResult<()> {
        if !self.in_tx {
            return Err(DbError::new("SetCatalogRootPage requires tx"));
        }
        self.header.catalog_root_page = page_no;
        self.refresh_header_page()?;
        Ok(())
    }

    fn refresh_header_page(&mut self) -> DbResult<()> {
        let mut header_page = if let Some(existing) = self.cache.get(0) {
            existing.data.clone()
        } else {
            let mut data = vec![0; self.page_size()];
            self.file.seek(SeekFrom::Start(0))?;
            self.file.read_exact(&mut data)?;
            verify_page_checksum(&data)?;
            data
        };
        self.header.encode_into(&mut header_page)?;
        self.cache.insert(
            0,
            CachedPage {
                data: header_page,
                dirty: true,
            },
        );
        Ok(())
    }

    fn ensure_page_loaded(&mut self, no: u32) -> DbResult<()> {
        if no >= self.header.page_count {
            return Err(DbError::new(format!("page out of range: {}", no)));
        }
        if self.cache.contains_key(no) {
            return Ok(());
        }
        let mut data = vec![0; self.page_size()];
        self.file.seek(SeekFrom::Start(self.page_offset(no)))?;
        self.file.read_exact(&mut data)?;
        verify_page_checksum(&data)
            .map_err(|err| DbError::new(format!("page {} corrupt: {}", no, err)))?;
        self.cache.insert(no, CachedPage { data, dirty: false });
        Ok(())
    }

    fn page_offset(&self, no: u32) -> u64 {
        no as u64 * self.page_size() as u64
    }
}

impl Drop for Pager {
    fn drop(&mut self) {
        if self.in_tx {
            let _ = self.rollback();
        }
    }
}

struct Wal {
    file: File,
    has_commit: bool,
}

impl Wal {
    fn create(path: PathBuf) -> DbResult<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        Ok(Self {
            file,
            has_commit: false,
        })
    }

    fn open(path: &Path) -> DbResult<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut wal = Self {
            file,
            has_commit: false,
        };
        wal.scan_commit()?;
        Ok(wal)
    }

    fn has_commit(&self) -> bool {
        self.has_commit
    }

    fn write_page(&mut self, page_no: u32, data: &[u8]) -> DbResult<()> {
        if data.is_empty() {
            return Err(DbError::new("empty page data"));
        }
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&[WAL_REC_PAGE])?;
        self.file.write_all(&page_no.to_le_bytes())?;
        self.file.write_all(&(data.len() as u32).to_le_bytes())?;
        self.file.write_all(data)?;
        Ok(())
    }

    fn write_commit(&mut self) -> DbResult<()> {
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&[WAL_REC_COMMIT])?;
        self.has_commit = true;
        Ok(())
    }

    fn sync(&mut self) -> DbResult<()> {
        self.file.sync_all()?;
        Ok(())
    }

    fn scan_commit(&mut self) -> DbResult<()> {
        self.file.seek(SeekFrom::Start(0))?;
        loop {
            let mut rec_type = [0u8; 1];
            match self.file.read_exact(&mut rec_type) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(err) => return Err(err.into()),
            }
            match rec_type[0] {
                WAL_REC_PAGE => {
                    let mut hdr = [0u8; 8];
                    self.file.read_exact(&mut hdr)?;
                    let len = u32::from_le_bytes(hdr[4..8].try_into().unwrap()) as usize;
                    self.file.seek(SeekFrom::Current(len as i64))?;
                }
                WAL_REC_COMMIT => self.has_commit = true,
                _ => return Err(DbError::new("unknown wal record")),
            }
        }
    }

    fn replay_to(&mut self, db: &mut File, page_size: usize) -> DbResult<()> {
        if !self.has_commit {
            return Ok(());
        }
        self.file.seek(SeekFrom::Start(0))?;
        loop {
            let mut rec_type = [0u8; 1];
            match self.file.read_exact(&mut rec_type) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(err) => return Err(err.into()),
            }
            match rec_type[0] {
                WAL_REC_PAGE => {
                    let mut hdr = [0u8; 8];
                    self.file.read_exact(&mut hdr)?;
                    let page_no = u32::from_le_bytes(hdr[0..4].try_into().unwrap());
                    let len = u32::from_le_bytes(hdr[4..8].try_into().unwrap()) as usize;
                    if len != page_size {
                        return Err(DbError::new("wal con tamaño de página inconsistente"));
                    }
                    let mut data = vec![0; len];
                    self.file.read_exact(&mut data)?;
                    // Page payload inside the WAL already carries its CRC
                    // trailer. Verify before applying so a torn write is
                    // refused instead of silently corrupting the DB file.
                    verify_page_checksum(&data).map_err(|err| {
                        DbError::new(format!(
                            "WAL record for page {} fails checksum: {}",
                            page_no, err
                        ))
                    })?;
                    db.seek(SeekFrom::Start(page_no as u64 * page_size as u64))?;
                    db.write_all(&data)?;
                }
                WAL_REC_COMMIT => {}
                _ => return Err(DbError::new("unknown wal record")),
            }
        }
    }
}

fn wal_path_for(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".wal");
    PathBuf::from(value)
}

/// Write the CRC32-IEEE of `page[..len-4]` into the trailing 4 bytes.
pub fn finalize_page_checksum(page: &mut [u8]) {
    let n = page.len();
    assert!(
        n >= PAGE_CHECKSUM_BYTES,
        "page smaller than checksum trailer"
    );
    let crc = crc32_ieee(&page[..n - PAGE_CHECKSUM_BYTES]);
    page[n - PAGE_CHECKSUM_BYTES..].copy_from_slice(&crc.to_le_bytes());
}

/// Recompute the CRC32-IEEE over the page payload and compare against the
/// trailing 4 bytes. Returns an error if they don't match.
pub fn verify_page_checksum(page: &[u8]) -> DbResult<()> {
    if page.len() < PAGE_CHECKSUM_BYTES {
        return Err(DbError::new("page smaller than checksum trailer"));
    }
    let n = page.len();
    let stored = u32::from_le_bytes(page[n - PAGE_CHECKSUM_BYTES..].try_into().unwrap());
    let calculated = crc32_ieee(&page[..n - PAGE_CHECKSUM_BYTES]);
    if stored != calculated {
        return Err(DbError::new(format!(
            "checksum mismatch (stored=0x{:08x}, computed=0x{:08x})",
            stored, calculated
        )));
    }
    Ok(())
}

fn crc32_table() -> &'static [u32; 256] {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xEDB88320
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        table
    })
}

pub fn crc32_ieee(data: &[u8]) -> u32 {
    let table = crc32_table();
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in data {
        let idx = ((crc ^ *byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ table[idx];
    }
    !crc
}
