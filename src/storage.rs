use crate::errors::{coded, codes};
use crate::{DbError, DbResult};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Acquire an exclusive advisory lock on the open `.db` file so no other
/// process (and no other `Pager` instance in the same process) can open
/// the same DB concurrently. Without this, two processes could write
/// dirty pages simultaneously and corrupt the file — the WAL design
/// assumes a single writer.
///
/// The lock is held for the lifetime of the `File` handle owned by the
/// `Pager` and released on `Pager::close` (or on `File` drop). We use
/// `try_lock` (non-blocking) so callers fail fast with a clear error
/// instead of hanging on a busy DB.
fn acquire_db_lock(file: &File, path: &Path) -> DbResult<()> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(TryLockError::WouldBlock) => Err(coded(
            codes::DB_LOCKED_BY_PROCESS,
            format!(
                "base de datos bloqueada por otro proceso: {}. \
                 Cierre el otro proceso gabysql o espere a que libere el lock.",
                path.display()
            ),
        )),
        Err(TryLockError::Error(err)) => Err(coded(
            codes::DB_LOCKED_BY_PROCESS,
            format!(
                "no se pudo adquirir el lock exclusivo sobre {}: {}",
                path.display(),
                err
            ),
        )),
    }
}

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
//   7 -> IndexMeta carries an `IndexKind` byte (Hash | OrderedInt).
//        OrderedInt indexes (created automatically over INT-typed
//        columns) use the value as the B+Tree key, enabling
//        `WHERE col_idx BETWEEN a AND b` range scans. Legacy hash
//        indexes over TEXT/FLOAT/BOOL/DATE/DATETIME stay equality-only.
//        V6 files are rejected on open — no automatic upgrade.
//   8 -> Bloque K2: PRIMARY KEY e índices secundarios admiten varias
//        columnas. `TableMeta` serializa la PK como `[u8:count] +
//        count × string` (count >= 1) y `IndexMeta` lo mismo para sus
//        columnas. La PK compuesta y los índices compuestos están
//        restringidos a multi-INT NOT NULL (ver ADR-0019): se
//        encodean como un fingerprint i64 FNV-1a-64 que vive como
//        clave del B+Tree, lo cual permite equality lookup pero NO
//        range scan sobre claves compuestas. V7 files son rechazados
//        al abrir con `[GBY-1003]` — la migración es manual: hacer
//        backup, recrear con binario V8 y volver a cargar los datos.
//   9 -> Bloque L1: cada FK añade un byte `on_update`, `OnDelete`
//        admite códigos 2=SET NULL y 3=SET DEFAULT. V8 rechazados con
//        `[GBY-1003]`.
//  10 -> Bloque L2: `TableMeta` añade un trailer
//        `check_count:u16 + (name, source)*` con los constraints
//        CHECK declarados a nivel de columna o de tabla. La expresión
//        se serializa como SQL canónico (`format_expr`) y se re-parsea
//        en cada write. V9 files son rechazados al abrir con
//        `[GBY-1003]`.
//  11 -> Residual #2 de L: nombres explícitos en PK y FK
//        (`CONSTRAINT <name> PRIMARY KEY (...)` y
//        `CONSTRAINT <name> FOREIGN KEY (...)`). Cada `TableMeta`
//        añade un byte `pk_name_present:u8` + string opcional tras la
//        lista de columnas PK; cada FK record añade
//        `fk_name_present:u8` + string opcional al final. V10 rechazados
//        con `[GBY-1003]`.
//  12 -> Residual #3 de L: multi-col FOREIGN KEY
//        (`FOREIGN KEY (a, b) REFERENCES p (x, y)`). Cada FK record
//        añade al final `[fk_extra_count:u8]` + N strings de columnas
//        source extra + N strings de columnas target extra. Single-col
//        FKs escriben count=0. El motor exige que las target columns
//        sean exactamente la PK compuesta del padre (single-col PK
//        sigue igual que pre-#3). V11 rechazados con `[GBY-1003]`.
//  13 -> Bloque V: vistas lógicas (`CREATE VIEW v AS SELECT ...`,
//        `DROP VIEW v`). Cada record del catálogo arranca con un byte
//        discriminator `[kind:u8]` (0=Table, 1=View) seguido del
//        payload específico. Las vistas guardan sólo el texto SQL del
//        SELECT que las define; se re-parsean al vuelo y se expanden
//        como subquery en cualquier FROM que las referencia. V12
//        rechazados con `[GBY-1003]` — migración manual: dump SELECT
//        + recreate con binario V13.
//  14 -> Bloque X1 (2026-05-28): triggers (`CREATE TRIGGER name
//        {BEFORE|AFTER} {INSERT|UPDATE|DELETE} ON table FOR EACH ROW
//        <single_dml>`, `DROP TRIGGER`). El discriminator `[kind:u8]`
//        agrega el valor `2=Trigger`. Cada trigger guarda nombre,
//        tabla target, timing y event (1 byte cada uno), y el body
//        como texto SQL (re-parseado en cada fire — mismo patrón que
//        ViewMeta). V13 rechazados con `[GBY-1003]` — migración manual.
//  15 -> Bloque X3 (2026-05-28): stored procedures (`CREATE PROCEDURE
//        name(p1 TYPE, p2 TYPE) AS <body>`, `DROP PROCEDURE`, `CALL`).
//        Discriminator `3=Procedure`. Cada procedure guarda nombre,
//        params (Vec<(String,ColumnType)>) y body como texto SQL.
//        V14 rechazados con `[GBY-1003]` — migración manual.
//  16 -> Bloque X3b (2026-05-28): user-defined scalar functions
//        (`CREATE FUNCTION name(params) RETURNS TYPE AS <expr>`,
//        `DROP FUNCTION`, invocables en SELECT/WHERE/etc.).
//        Discriminator `4=Function`. Payload: [name][return_type:u8]
//        [param_count:u16] × ([pname][ptype:u8]) [body_sql].
//        V15 rechazados con `[GBY-1003]` — migración manual.
//  17 -> Bloque Y (2026-05-29): tipos de columna extendidos. Aliases
//        sintácticos sin cambio en disco (BIGINT/SMALLINT/VARCHAR(n)/
//        DECIMAL(p,s)/TIMESTAMP/BOOLEAN/REAL/DOUBLE/etc. mapean a
//        Int/Text/Float/Bool/DateTime existentes). Dos códigos
//        nuevos en disco: `8=TIME`, `9=UUID` (ambos stores_as_text).
//        V16 rechazados con `[GBY-1003]` porque un schema válido en
//        V17 puede contener columnas TIME/UUID que V16 no sabe leer.
//  18 -> Bloque Y2 (2026-05-29): enforcement de longitud para
//        `VARCHAR(n)` / `CHAR(n)`. Se persiste `max_length: u32`
//        opcional por columna, indicado por el nuevo flag
//        `COLUMN_FLAG_HAS_MAX_LENGTH = 0x08`. V17 rechazados con
//        `[GBY-1003]` — un V18 puede tener bytes extra que V17 no
//        sabe saltar al decodificar el bloque de columnas.
//  19 -> Bloque Y3 (2026-05-29): enforcement de rango para
//        `TINYINT`/`SMALLINT`/`INT2`/`MEDIUMINT`/`INT4`. Se persiste
//        `int_width: u8` opcional por columna (valores 1, 2, 3 o 4)
//        tras `max_length`, indicado por el flag
//        `COLUMN_FLAG_HAS_INT_WIDTH = 0x10`. V18 rechazados con
//        `[GBY-1003]` por la misma razón que V17→V18.
//  20 -> Bloque Y4 (2026-05-29): tipo binario `BLOB` / `BYTEA` /
//        `BINARY` con código en disco `10` y encoding propio (u32
//        LE length + raw bytes) — no es ni text ni int. `Value`
//        gana variante `Bytes(Vec<u8>)`. Literales SQL via `X'hex'`.
//        V19 rechazados con `[GBY-1003]` porque un schema V20 puede
//        tener columnas BLOB que V19 no sabe leer.
//  21 -> Bloque Y5 (2026-05-29): UNSIGNED enforcement para
//        `TINYINT|SMALLINT|MEDIUMINT|INT4|INT|BIGINT` (sintaxis
//        MySQL `<tipo> UNSIGNED`). Reutiliza el byte `int_width`:
//        high bit 0x80 = unsigned, low 4 bits = width (1..=4) o 0
//        para "sin width" (BIGINT UNSIGNED, rango 0..i64::MAX).
//        V20 rechazados con `[GBY-1003]` porque podría haber bytes
//        0x80+ que V20 interpreta como widths inválidos.
pub const VERSION: u32 = 21;

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
            return Err(DbError::new(format!(
                "buffer del header demasiado chico: tiene {} bytes, se requieren al menos {}",
                dst.len(),
                PAGE_SIZE_DEFAULT
            )));
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
            return Err(DbError::new(format!(
                "header demasiado chico: tiene {} bytes, requieren al menos 24 para el header gabysql",
                src.len()
            )));
        }
        if &src[0..8] != MAGIC {
            return Err(coded(
                codes::BAD_MAGIC_BYTES,
                "magic bytes inválidos: el archivo no es una base de datos gabysql (esperaba 'GABYSQL1')",
            ));
        }
        let version = u32::from_le_bytes(src[8..12].try_into().unwrap());
        if version != VERSION {
            return Err(coded(
                codes::UNSUPPORTED_FORMAT_VERSION,
                format!(
                    "formato de archivo gabysql no soportado: version={} (esperaba {}). \
                     Hacé backup del .db, re-creá la base con el binario actual y \
                     migrá los datos manualmente (dump SELECT desde un binario viejo + \
                     CREATE TABLE … AS SELECT / INSERT desde el nuevo). El motor no \
                     intenta auto-upgrade entre versiones incompatibles para evitar \
                     corrupción silenciosa de índices secundarios.",
                    version, VERSION
                ),
            ));
        }
        let page_size = u16::from_le_bytes(src[12..14].try_into().unwrap());
        // Format v3 only supports the default page size. The field is kept on
        // disk so a future format revision can lift the constraint without
        // another binary header change.
        if page_size as usize != PAGE_SIZE_DEFAULT {
            return Err(coded(
                codes::UNSUPPORTED_PAGE_SIZE,
                format!(
                    "page_size no soportado: archivo declara {}, este build requiere {}",
                    page_size, PAGE_SIZE_DEFAULT
                ),
            ));
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
            return Err(coded(
                codes::REFUSE_OVERWRITE_DB,
                format!(
                    "se rehúsa sobrescribir base de datos existente: {}. \
                     Bórrela primero o use create_force() (CLI: 'gabysql init --force').",
                    path.display()
                ),
            ));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        acquire_db_lock(&file, &path)?;

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
        acquire_db_lock(&file, &path)?;
        let mut page0 = vec![0; PAGE_SIZE_DEFAULT];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut page0)?;
        verify_page_checksum(&page0).map_err(|err| {
            DbError::new(format!(
                "página del header corrupta al abrir {}: {}",
                path.display(),
                err
            ))
        })?;
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
                DbError::new(format!(
                    "página del header corrupta después del replay del WAL en {}: {}",
                    path.display(),
                    err
                ))
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
        // Release the advisory lock explicitly. Drop also releases it,
        // but doing it here makes the handoff to another process
        // immediate and deterministic on platforms where Drop ordering
        // is non-obvious.
        let _ = self.file.unlock();
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
            return Err(coded(
                codes::TX_ALREADY_STARTED,
                "transacción ya iniciada: este Pager ya tiene una transacción abierta; \
                 llame a commit() o rollback() antes de begin()",
            ));
        }
        self.wal = Some(Wal::create(wal_path_for(&self.path))?);
        self.in_tx = true;
        Ok(())
    }

    pub fn commit(&mut self) -> DbResult<()> {
        if !self.in_tx {
            return Err(coded(
                codes::NO_ACTIVE_TX,
                "no hay transacción activa: commit() requiere un begin() previo",
            ));
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

        let wal = self.wal.as_mut().ok_or_else(|| {
            DbError::new(
                "estado inconsistente: in_tx=true pero el WAL no fue inicializado en begin()",
            )
        })?;

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
            return Err(coded(
                codes::NO_ACTIVE_TX,
                "no hay transacción activa: rollback() requiere un begin() previo",
            ));
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
            .ok_or_else(|| {
                DbError::new(format!(
                    "estado inconsistente del page cache: la página {} se reportó \
                     como cargada pero no está en el HashMap",
                    no
                ))
            })?
            .data
            .clone())
    }

    pub fn write_page(&mut self, no: u32, data: &[u8], dirty: bool) -> DbResult<()> {
        if data.len() != self.page_size() {
            return Err(DbError::new(format!(
                "tamaño de página inválido al escribir página {}: \
                 recibí {} bytes, el archivo usa páginas de {} bytes",
                no,
                data.len(),
                self.page_size()
            )));
        }
        self.ensure_page_loaded(no)?;
        let page = self.cache.get_mut(no).ok_or_else(|| {
            DbError::new(format!(
                "estado inconsistente del page cache: la página {} se reportó \
                 como cargada pero no está en el HashMap",
                no
            ))
        })?;
        page.data.copy_from_slice(data);
        page.dirty = dirty;
        Ok(())
    }

    pub fn mark_dirty(&mut self, no: u32) -> DbResult<()> {
        self.ensure_page_loaded(no)?;
        let page = self.cache.get_mut(no).ok_or_else(|| {
            DbError::new(format!(
                "estado inconsistente del page cache: la página {} se reportó \
                 como cargada pero no está en el HashMap",
                no
            ))
        })?;
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

    /// Whether a specific page is currently resident in the cache.
    /// Used by tests that need to observe prefetch / read-ahead
    /// behavior (ADR-0016) and by future operational tooling.
    pub fn cache_contains(&self, page_no: u32) -> bool {
        self.cache.contains_key(page_no)
    }

    /// Maximum number of cached pages. Hit by `cache_len()` and
    /// then eviction starts (clean-page LRU first).
    pub fn cache_capacity(&self) -> usize {
        self.cache.capacity()
    }

    pub fn new_page(&mut self) -> DbResult<u32> {
        if !self.in_tx {
            return Err(DbError::new(
                "new_page() requiere una transacción activa: llame a begin() primero",
            ));
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
            return Err(DbError::new(
                "set_catalog_root_page() requiere una transacción activa: llame a begin() primero",
            ));
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
            return Err(DbError::new(format!(
                "página fuera de rango: pedida {}, el archivo tiene {} páginas",
                no, self.header.page_count
            )));
        }
        if self.cache.contains_key(no) {
            return Ok(());
        }
        let mut data = vec![0; self.page_size()];
        self.file.seek(SeekFrom::Start(self.page_offset(no)))?;
        self.file.read_exact(&mut data)?;
        verify_page_checksum(&data).map_err(|err| {
            coded(
                codes::PAGE_CRC_INVALID,
                format!("página {} corrupta: {}", no, err),
            )
        })?;
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
            return Err(DbError::new(format!(
                "Wal::write_page: payload vacío al escribir página {} (esperaba {} bytes)",
                page_no, PAGE_SIZE_DEFAULT
            )));
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
                other => {
                    return Err(DbError::new(format!(
                        "WAL corrupto: record type desconocido {:#04x} (esperaba PAGE={:#04x} o COMMIT={:#04x})",
                        other, WAL_REC_PAGE, WAL_REC_COMMIT
                    )))
                }
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
                        return Err(DbError::new(format!(
                            "WAL inconsistente: record para página {} declara len={}, el .db usa page_size={}",
                            page_no, len, page_size
                        )));
                    }
                    let mut data = vec![0; len];
                    self.file.read_exact(&mut data)?;
                    // Page payload inside the WAL already carries its CRC
                    // trailer. Verify before applying so a torn write is
                    // refused instead of silently corrupting the DB file.
                    verify_page_checksum(&data).map_err(|err| {
                        coded(
                            codes::WAL_RECORD_CRC_INVALID,
                            format!(
                                "WAL record para página {} con CRC32 inválido: {}",
                                page_no, err
                            ),
                        )
                    })?;
                    db.seek(SeekFrom::Start(page_no as u64 * page_size as u64))?;
                    db.write_all(&data)?;
                }
                WAL_REC_COMMIT => {}
                other => {
                    return Err(DbError::new(format!(
                        "WAL corrupto: record type desconocido {:#04x} (esperaba PAGE={:#04x} o COMMIT={:#04x})",
                        other, WAL_REC_PAGE, WAL_REC_COMMIT
                    )))
                }
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
        "página más chica que el trailer del checksum"
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
