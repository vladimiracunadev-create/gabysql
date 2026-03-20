use crate::{DbError, DbResult};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const PAGE_SIZE_DEFAULT: usize = 4096;
pub const MAGIC: &[u8; 8] = b"GABYSQL1";
pub const VERSION: u32 = 1;

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
            return Err(DbError::new("unsupported version"));
        }
        let page_size = u16::from_le_bytes(src[12..14].try_into().unwrap());
        if page_size == 0 {
            return Err(DbError::new("invalid page size"));
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

pub struct Pager {
    path: PathBuf,
    file: File,
    wal: Option<Wal>,
    header: Header,
    cache: BTreeMap<u32, CachedPage>,
    in_tx: bool,
}

impl Pager {
    pub fn create(path: impl AsRef<Path>) -> DbResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
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
        file.write_all(&page0)?;
        file.sync_all()?;

        Ok(Self {
            path,
            file,
            wal: None,
            header,
            cache: BTreeMap::new(),
            in_tx: false,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> DbResult<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut page0 = vec![0; PAGE_SIZE_DEFAULT];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut page0)?;
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
            header = Header::decode(&refreshed)?;
        }

        Ok(Self {
            path,
            file,
            wal: None,
            header,
            cache: BTreeMap::new(),
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

        let dirty_pages: Vec<(u32, Vec<u8>)> = self
            .cache
            .iter()
            .filter(|(_, page)| page.dirty)
            .map(|(no, page)| (*no, page.data.clone()))
            .collect();

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

        for page in self.cache.values_mut() {
            page.dirty = false;
        }

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
        self.header = Header::decode(&page0)?;
        self.in_tx = false;
        Ok(())
    }

    pub fn page_data(&mut self, no: u32) -> DbResult<Vec<u8>> {
        self.ensure_page_loaded(no)?;
        Ok(self
            .cache
            .get(&no)
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
            .get_mut(&no)
            .ok_or_else(|| DbError::new("page cache inconsistente"))?;
        page.data.copy_from_slice(data);
        page.dirty = dirty;
        Ok(())
    }

    pub fn mark_dirty(&mut self, no: u32) -> DbResult<()> {
        self.ensure_page_loaded(no)?;
        let page = self
            .cache
            .get_mut(&no)
            .ok_or_else(|| DbError::new("page cache inconsistente"))?;
        page.dirty = true;
        Ok(())
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
        let mut header_page = if let Some(existing) = self.cache.get(&0) {
            existing.data.clone()
        } else {
            let mut data = vec![0; self.page_size()];
            self.file.seek(SeekFrom::Start(0))?;
            self.file.read_exact(&mut data)?;
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
        if self.cache.contains_key(&no) {
            return Ok(());
        }
        let mut data = vec![0; self.page_size()];
        self.file.seek(SeekFrom::Start(self.page_offset(no)))?;
        self.file.read_exact(&mut data)?;
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
