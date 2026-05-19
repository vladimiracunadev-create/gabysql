use crate::bptree::{init_leaf_page, LeafCursor, Tree};
use crate::errors::{coded, codes};
use crate::storage::Pager;
use crate::{DbError, DbResult};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Int,
    Text,
    Bool,
    Float,
    Date,
    DateTime,
    Json,
}

impl ColumnType {
    pub fn from_sql(value: &str) -> DbResult<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "INT" => Ok(Self::Int),
            "TEXT" => Ok(Self::Text),
            "BOOL" => Ok(Self::Bool),
            "FLOAT" => Ok(Self::Float),
            "DATE" => Ok(Self::Date),
            "DATETIME" => Ok(Self::DateTime),
            "JSON" => Ok(Self::Json),
            other => Err(DbError::new(format!("tipo no soportado: {}", other))),
        }
    }

    pub fn as_sql(&self) -> &'static str {
        match self {
            Self::Int => "INT",
            Self::Text => "TEXT",
            Self::Bool => "BOOL",
            Self::Float => "FLOAT",
            Self::Date => "DATE",
            Self::DateTime => "DATETIME",
            Self::Json => "JSON",
        }
    }

    fn code(&self) -> u8 {
        match self {
            Self::Int => 1,
            Self::Text => 2,
            Self::Bool => 3,
            Self::Float => 4,
            Self::Date => 5,
            Self::DateTime => 6,
            Self::Json => 7,
        }
    }

    fn from_code(code: u8) -> DbResult<Self> {
        match code {
            1 => Ok(Self::Int),
            2 => Ok(Self::Text),
            3 => Ok(Self::Bool),
            4 => Ok(Self::Float),
            5 => Ok(Self::Date),
            6 => Ok(Self::DateTime),
            7 => Ok(Self::Json),
            other => Err(DbError::new(format!(
                "tipo de columna inválido en disco: code={} (esperaba 1=INT, 2=TEXT, 3=BOOL, 4=FLOAT, 5=DATE, 6=DATETIME, 7=JSON)",
                other
            ))),
        }
    }

    pub fn stores_as_text(&self) -> bool {
        matches!(self, Self::Text | Self::Date | Self::DateTime | Self::Json)
    }
}

/// Literal value usable as a column DEFAULT. Mirrors the variants of
/// `sql::Value` but lives in the catalog layer so the on-disk encoding
/// of the catalog is independent of the SQL frontend.
#[derive(Debug, Clone, PartialEq)]
pub enum DefaultLiteral {
    Null,
    Integer(i64),
    Float(f64),
    Bool(bool),
    String(String),
}

impl DefaultLiteral {
    fn kind_code(&self) -> u8 {
        match self {
            Self::Null => 0,
            Self::Integer(_) => 1,
            Self::Float(_) => 2,
            Self::Bool(_) => 3,
            Self::String(_) => 4,
        }
    }

    fn encode_into(&self, out: &mut Vec<u8>) -> DbResult<()> {
        out.push(self.kind_code());
        match self {
            Self::Null => {}
            Self::Integer(n) => out.extend_from_slice(&n.to_le_bytes()),
            Self::Float(n) => out.extend_from_slice(&n.to_le_bytes()),
            Self::Bool(b) => out.push(u8::from(*b)),
            Self::String(s) => push_string(out, s)?,
        }
        Ok(())
    }

    fn decode(data: &[u8], offset: &mut usize) -> DbResult<Self> {
        if *offset >= data.len() {
            return Err(DbError::new(format!(
                "DEFAULT corrupto: buffer agotado en offset {} (len={}), \
                 falta el byte de kind",
                *offset,
                data.len()
            )));
        }
        let kind = data[*offset];
        *offset += 1;
        Ok(match kind {
            0 => Self::Null,
            1 => {
                if *offset + 8 > data.len() {
                    return Err(DbError::new(format!(
                        "DEFAULT INT corrupto: necesito 8 bytes en offset {}, \
                         solo quedan {} bytes",
                        *offset,
                        data.len() - *offset
                    )));
                }
                let n = i64::from_le_bytes(data[*offset..*offset + 8].try_into().unwrap());
                *offset += 8;
                Self::Integer(n)
            }
            2 => {
                if *offset + 8 > data.len() {
                    return Err(DbError::new(format!(
                        "DEFAULT FLOAT corrupto: necesito 8 bytes en offset {}, \
                         solo quedan {} bytes",
                        *offset,
                        data.len() - *offset
                    )));
                }
                let n = f64::from_le_bytes(data[*offset..*offset + 8].try_into().unwrap());
                *offset += 8;
                Self::Float(n)
            }
            3 => {
                if *offset >= data.len() {
                    return Err(DbError::new(format!(
                        "DEFAULT BOOL corrupto: necesito 1 byte en offset {} (len={})",
                        *offset,
                        data.len()
                    )));
                }
                let b = data[*offset] != 0;
                *offset += 1;
                Self::Bool(b)
            }
            4 => Self::String(take_string(data, offset)?),
            other => {
                return Err(DbError::new(format!(
                    "DEFAULT corrupto: kind={} desconocido en offset {} \
                     (esperaba 0=Null, 1=Int, 2=Float, 3=Bool, 4=String)",
                    other,
                    *offset - 1
                )))
            }
        })
    }
}

const COLUMN_FLAG_NOT_NULL: u8 = 0x01;
const COLUMN_FLAG_HAS_DEFAULT: u8 = 0x02;
const COLUMN_FLAG_HAS_FK: u8 = 0x04;

/// Action to take when the parent row a `FOREIGN KEY` points at is
/// deleted. SQL standard offers more (SET NULL / SET DEFAULT / NO
/// ACTION); this version supports the two that cover the vast majority
/// of real schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDelete {
    /// Refuse the parent DELETE if any child row still references it.
    /// Default behaviour when `ON DELETE` is omitted.
    Restrict,
    /// Delete every child row that references the parent. Cascading
    /// deletes can chain through several tables; the engine guards
    /// against cycles using a visited set on `(table, pk)`.
    Cascade,
}

impl OnDelete {
    fn code(self) -> u8 {
        match self {
            Self::Restrict => 0,
            Self::Cascade => 1,
        }
    }

    fn from_code(code: u8) -> DbResult<Self> {
        match code {
            0 => Ok(Self::Restrict),
            1 => Ok(Self::Cascade),
            other => Err(DbError::new(format!(
                "FOREIGN KEY on_delete code desconocido en disco: {} (esperaba 0=RESTRICT o 1=CASCADE)",
                other
            ))),
        }
    }

    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Restrict => "RESTRICT",
            Self::Cascade => "CASCADE",
        }
    }
}

/// Single-column foreign key persisted on a `Column`. The target column
/// must be the parent table's PRIMARY KEY in this version — the engine
/// does not yet support REFERENCES against arbitrary UNIQUE columns,
/// which keeps lookup paths simple (parent PK lookup is already O(log n)
/// via the table's own B+Tree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeyMeta {
    pub table: String,
    pub column: String,
    pub on_delete: OnDelete,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    pub column_type: ColumnType,
    pub not_null: bool,
    pub default: Option<DefaultLiteral>,
    pub references: Option<ForeignKeyMeta>,
}

impl Column {
    pub fn plain(name: impl Into<String>, column_type: ColumnType) -> Self {
        Self {
            name: name.into(),
            column_type,
            not_null: false,
            default: None,
            references: None,
        }
    }
}

/// Physical kind of a secondary index, written to disk on VERSION 7+.
///
/// - `Hash`: the legacy bucket-by-FNV1a layout (ADR-0005). Supports
///   equality lookup only. Used for TEXT / FLOAT / BOOL / DATE /
///   DATETIME columns where producing an order-preserving i64 key is
///   not trivial without a byte-keyed B+Tree.
/// - `OrderedInt`: the indexed value (an `INT`) is used **directly as
///   the B+Tree key**, so `cursor_range(from, to)` walks index entries
///   in true value order. Enables `WHERE col_idx BETWEEN a AND b` for
///   INT-typed indexed columns (ADR-0017).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKind {
    Hash,
    OrderedInt,
}

impl IndexKind {
    pub fn code(&self) -> u8 {
        match self {
            IndexKind::Hash => 0,
            IndexKind::OrderedInt => 1,
        }
    }

    pub fn from_code(code: u8) -> DbResult<Self> {
        match code {
            0 => Ok(IndexKind::Hash),
            1 => Ok(IndexKind::OrderedInt),
            other => Err(DbError::new(format!(
                "IndexKind code desconocido en disco: {} (esperaba 0=Hash o 1=OrderedInt)",
                other
            ))),
        }
    }

    /// Pick the right index kind for a column. INT columns get the new
    /// ordered layout (range-scan capable); every other type stays on
    /// the legacy hash-bucket layout (equality only) — extending range
    /// to TEXT/FLOAT would need a byte-keyed B+Tree which is out of
    /// scope for this bump.
    pub fn for_column(column_type: ColumnType) -> Self {
        match column_type {
            ColumnType::Int => IndexKind::OrderedInt,
            _ => IndexKind::Hash,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexMeta {
    pub name: String,
    pub column: String,
    pub root_page: u32,
    pub unique: bool,
    /// V7+: distinguishes legacy hash-bucket indexes from new
    /// INT-ordered indexes that support range scan. See [`IndexKind`].
    pub kind: IndexKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableMeta {
    pub name: String,
    pub primary_key: String,
    pub columns: Vec<Column>,
    pub root_page: u32,
    pub indexes: Vec<IndexMeta>,
}

impl TableMeta {
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(name))
    }

    pub fn index_for_column(&self, column: &str) -> Option<&IndexMeta> {
        self.indexes
            .iter()
            .find(|idx| idx.column.eq_ignore_ascii_case(column))
    }

    pub fn index_by_name(&self, name: &str) -> Option<&IndexMeta> {
        self.indexes
            .iter()
            .find(|idx| idx.name.eq_ignore_ascii_case(name))
    }

    /// VERSION = 7 on-disk layout for a TableMeta record:
    ///
    ///     [name][primary_key][root_page:u32]
    ///     [col_count:u16] · col_count × {
    ///         [name][type_code:u8][flags:u8]
    ///         flags & 0x02 ? DefaultLiteral payload : ∅
    ///         flags & 0x04 ? [target_table][target_column][on_delete:u8] : ∅
    ///     }
    ///     [idx_count:u16] · idx_count × {
    ///         [name][column][root_page:u32][unique:u8][kind:u8]
    ///     }
    pub fn serialize(&self) -> DbResult<Vec<u8>> {
        let mut out = Vec::new();
        push_string(&mut out, &self.name)?;
        push_string(&mut out, &self.primary_key)?;
        out.extend_from_slice(&self.root_page.to_le_bytes());
        out.extend_from_slice(&(self.columns.len() as u16).to_le_bytes());
        for column in &self.columns {
            push_string(&mut out, &column.name)?;
            out.push(column.column_type.code());
            let mut flags = 0u8;
            if column.not_null {
                flags |= COLUMN_FLAG_NOT_NULL;
            }
            if column.default.is_some() {
                flags |= COLUMN_FLAG_HAS_DEFAULT;
            }
            if column.references.is_some() {
                flags |= COLUMN_FLAG_HAS_FK;
            }
            out.push(flags);
            if let Some(default) = &column.default {
                default.encode_into(&mut out)?;
            }
            if let Some(fk) = &column.references {
                push_string(&mut out, &fk.table)?;
                push_string(&mut out, &fk.column)?;
                out.push(fk.on_delete.code());
            }
        }
        out.extend_from_slice(&(self.indexes.len() as u16).to_le_bytes());
        for idx in &self.indexes {
            push_string(&mut out, &idx.name)?;
            push_string(&mut out, &idx.column)?;
            out.extend_from_slice(&idx.root_page.to_le_bytes());
            out.push(u8::from(idx.unique));
            out.push(idx.kind.code());
        }
        Ok(out)
    }

    pub fn deserialize(data: &[u8]) -> DbResult<Self> {
        let mut offset = 0usize;
        let name = take_string(data, &mut offset)?;
        let primary_key = take_string(data, &mut offset)?;
        if offset + 6 > data.len() {
            return Err(DbError::new(format!(
                "TableMeta corrupta para tabla '{}': necesito 6 bytes en offset {} \
                 (root_page+col_count), solo quedan {}",
                name,
                offset,
                data.len() - offset
            )));
        }
        let root_page = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let count = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        let mut columns = Vec::with_capacity(count);
        for i in 0..count {
            let col_name = take_string(data, &mut offset)?;
            if offset + 2 > data.len() {
                return Err(DbError::new(format!(
                    "TableMeta '{}' corrupta: faltan bytes para el header de la columna {} ('{}') en offset {}",
                    name, i, col_name, offset
                )));
            }
            let column_type = ColumnType::from_code(data[offset])?;
            offset += 1;
            let flags = data[offset];
            offset += 1;
            let not_null = flags & COLUMN_FLAG_NOT_NULL != 0;
            let default = if flags & COLUMN_FLAG_HAS_DEFAULT != 0 {
                Some(DefaultLiteral::decode(data, &mut offset)?)
            } else {
                None
            };
            let references = if flags & COLUMN_FLAG_HAS_FK != 0 {
                let target_table = take_string(data, &mut offset)?;
                let target_column = take_string(data, &mut offset)?;
                if offset >= data.len() {
                    return Err(DbError::new(format!(
                        "FOREIGN KEY corrupta en '{}.{}': faltan bytes para on_delete en offset {}",
                        name, col_name, offset
                    )));
                }
                let on_delete = OnDelete::from_code(data[offset])?;
                offset += 1;
                Some(ForeignKeyMeta {
                    table: target_table,
                    column: target_column,
                    on_delete,
                })
            } else {
                None
            };
            columns.push(Column {
                name: col_name,
                column_type,
                not_null,
                default,
                references,
            });
        }
        if offset + 2 > data.len() {
            return Err(DbError::new(format!(
                "TableMeta '{}' corrupta: faltan 2 bytes para idx_count en offset {} (len={})",
                name,
                offset,
                data.len()
            )));
        }
        let idx_count = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        let mut indexes = Vec::with_capacity(idx_count);
        for i in 0..idx_count {
            let idx_name = take_string(data, &mut offset)?;
            let column = take_string(data, &mut offset)?;
            if offset + 6 > data.len() {
                return Err(DbError::new(format!(
                    "IndexMeta corrupta en tabla '{}' (índice {} '{}'): faltan 6 bytes \
                     (root_page+unique+kind) en offset {}",
                    name, i, idx_name, offset
                )));
            }
            let root_page = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            let unique = data[offset] != 0;
            offset += 1;
            let kind = IndexKind::from_code(data[offset])?;
            offset += 1;
            indexes.push(IndexMeta {
                name: idx_name,
                column,
                root_page,
                unique,
                kind,
            });
        }
        Ok(Self {
            name,
            primary_key,
            columns,
            root_page,
            indexes,
        })
    }
}

pub struct Catalog<'a> {
    pager: &'a mut Pager,
}

impl<'a> Catalog<'a> {
    pub fn open(pager: &'a mut Pager) -> Self {
        Self { pager }
    }

    pub fn list_tables(&mut self) -> DbResult<Vec<TableMeta>> {
        let header = self.pager.header();
        if header.catalog_root_page == 0 {
            return Ok(Vec::new());
        }
        let mut tree = Tree::new(self.pager);
        let kvs = tree.all(header.catalog_root_page)?;
        kvs.into_iter()
            .map(|kv| TableMeta::deserialize(&kv.value))
            .collect()
    }

    pub fn get_table(&mut self, name: &str) -> DbResult<Option<TableMeta>> {
        let header = self.pager.header();
        if header.catalog_root_page == 0 {
            return Ok(None);
        }
        let key = hash_name(name);
        let mut tree = Tree::new(self.pager);
        if let Some(bytes) = tree.get(header.catalog_root_page, key)? {
            let meta = TableMeta::deserialize(&bytes)?;
            if meta.name.eq_ignore_ascii_case(name) {
                return Ok(Some(meta));
            }
            return Err(DbError::new(format!(
                "colisión de hash FNV-1a-64 en el catálogo: \
                 se buscó '{}' pero el bucket contiene '{}'. Reporte este caso \
                 como bug: los nombres tienen el mismo hash y el motor no \
                 implementa todavía resolución por igualdad de nombre.",
                name, meta.name
            )));
        }
        Ok(None)
    }

    pub fn put_table(&mut self, meta: &TableMeta) -> DbResult<()> {
        let root = self.ensure_root()?;
        let key = hash_name(&meta.name);
        let payload = meta.serialize()?;
        let mut tree = Tree::new(self.pager);
        tree.upsert(root, key, payload)?;
        Ok(())
    }

    /// Remove the catalog entry for the named table. Returns whether an
    /// entry was actually removed. The pages backing the table's data and
    /// secondary indexes are intentionally NOT freed — the page allocator
    /// has no free-list yet, so reclaim is left to a future `vacuum`. This
    /// matches the existing `DROP INDEX` policy.
    pub fn remove_table(&mut self, name: &str) -> DbResult<bool> {
        let header = self.pager.header();
        if header.catalog_root_page == 0 {
            return Ok(false);
        }
        let key = hash_name(name);
        let mut tree = Tree::new(self.pager);
        tree.delete(header.catalog_root_page, key)
    }

    pub fn scan_rows(
        &mut self,
        root_page: u32,
        offset: usize,
        limit: Option<usize>,
    ) -> DbResult<Vec<crate::bptree::KeyValue>> {
        let mut tree = Tree::new(self.pager);
        tree.scan(root_page, offset, limit)
    }

    pub fn range_rows(
        &mut self,
        root_page: u32,
        from: i64,
        to: i64,
    ) -> DbResult<Vec<crate::bptree::KeyValue>> {
        let mut tree = Tree::new(self.pager);
        tree.range(root_page, from, to)
    }

    /// Lazy cursor over every row in a table's B+Tree. Combine with
    /// `Iterator::skip(offset).take(limit)` for cheap LIMIT/OFFSET on
    /// large tables — only the rows actually consumed are loaded.
    /// Caller drops the cursor before opening another Catalog through
    /// the same Pager (the cursor borrows the Pager mutably).
    pub fn scan_cursor(self, root_page: u32) -> DbResult<LeafCursor<'a>> {
        Tree::new(self.pager).cursor_full(root_page)
    }

    /// Lazy cursor over rows with PK in `[from, to]` (both inclusive).
    pub fn range_cursor(self, root_page: u32, from: i64, to: i64) -> DbResult<LeafCursor<'a>> {
        Tree::new(self.pager).cursor_range(root_page, from, to)
    }

    pub fn get_row(&mut self, root_page: u32, key: i64) -> DbResult<Option<Vec<u8>>> {
        let mut tree = Tree::new(self.pager);
        tree.get(root_page, key)
    }

    pub fn insert_row(&mut self, root_page: u32, key: i64, value: Vec<u8>) -> DbResult<()> {
        let mut tree = Tree::new(self.pager);
        tree.insert(root_page, key, value)?;
        Ok(())
    }

    pub fn upsert_row(&mut self, root_page: u32, key: i64, value: Vec<u8>) -> DbResult<()> {
        let mut tree = Tree::new(self.pager);
        tree.upsert(root_page, key, value)?;
        Ok(())
    }

    pub fn delete_row(&mut self, root_page: u32, key: i64) -> DbResult<bool> {
        let mut tree = Tree::new(self.pager);
        tree.delete(root_page, key)
    }

    fn ensure_root(&mut self) -> DbResult<u32> {
        let header = self.pager.header();
        if header.catalog_root_page != 0 {
            return Ok(header.catalog_root_page);
        }
        let page_no = self.pager.new_page()?;
        let mut page = vec![0; self.pager.page_size()];
        init_leaf_page(&mut page);
        self.pager.write_page(page_no, &page, true)?;
        self.pager.set_catalog_root_page(page_no)?;
        Ok(page_no)
    }
}

pub fn validate_create_table(meta: &TableMeta) -> DbResult<()> {
    validate_identifier(&meta.name, "tabla")?;
    if meta.primary_key.trim().is_empty() {
        return Err(DbError::new(
            "PRIMARY KEY requerida (esta versión solo soporta una PK escalar de tipo INT)",
        ));
    }
    if meta.columns.is_empty() {
        return Err(DbError::new(format!(
            "CREATE TABLE '{}' rechazado: debe declarar al menos una columna",
            meta.name
        )));
    }

    let mut seen = HashSet::new();
    let mut pk_ok = false;
    for column in &meta.columns {
        validate_identifier(&column.name, "columna")?;
        let normalized = column.name.to_ascii_lowercase();
        if !seen.insert(normalized) {
            return Err(coded(
                codes::DUPLICATE_COLUMN_NAME,
                format!(
                    "CREATE TABLE '{}' rechazado: nombre de columna duplicado '{}'",
                    meta.name, column.name
                ),
            ));
        }
        if column.name.eq_ignore_ascii_case(&meta.primary_key) {
            if column.column_type != ColumnType::Int {
                return Err(DbError::new(format!(
                    "PRIMARY KEY '{}' debe ser INT (esta versión sólo admite PK INT escalar; ver USER_MANUAL)",
                    column.name
                )));
            }
            if column.default.is_some() {
                return Err(DbError::new(format!(
                    "PRIMARY KEY '{}' no admite DEFAULT en esta versión",
                    column.name
                )));
            }
            pk_ok = true;
        }

        // NOT NULL + DEFAULT NULL is contradictory.
        if column.not_null && matches!(column.default, Some(DefaultLiteral::Null)) {
            return Err(DbError::new(format!(
                "columna '{}': NOT NULL incompatible con DEFAULT NULL",
                column.name
            )));
        }

        // The DEFAULT literal must be compatible with the declared type.
        if let Some(default) = &column.default {
            validate_default_against_type(&column.name, &column.column_type, default)?;
        }
    }
    if !pk_ok {
        return Err(DbError::new("PRIMARY KEY debe existir en columnas"));
    }
    Ok(())
}

/// Maximum identifier length, matching PostgreSQL's `NAMEDATALEN - 1` and
/// giving plenty of room for prefixes like `idx_` / `uq_` without
/// overflowing.
pub const MAX_IDENT_LEN: usize = 64;

/// Reserved words that the parser already gives meaning to. Banning them
/// as identifiers prevents ambiguous statements like `CREATE TABLE select
/// (id INT PRIMARY KEY)` and keeps the gramatical grammar future-proof
/// when new keywords land. Comparison is case-insensitive.
pub const RESERVED_WORDS: &[&str] = &[
    // DDL / DML verbs
    "create",
    "drop",
    "alter",
    "add",
    "column",
    "table",
    "index",
    "unique",
    "database",
    "databases",
    "show",
    "if",
    "exists",
    "not",
    "insert",
    "into",
    "select",
    "update",
    "delete",
    "from",
    "set",
    "values",
    // Predicates / clauses
    "where",
    "between",
    "and",
    "limit",
    "offset",
    "order",
    "by",
    "asc",
    "desc",
    "on",
    "primary",
    "key",
    "default",
    "null",
    "true",
    "false",
    // FOREIGN KEY clause (VERSION 6+)
    "foreign",
    "references",
    "cascade",
    "restrict",
    // Operational sentences (VERSION 6+)
    "integrity",
    "check",
    // Built-in column types
    "int",
    "text",
    "bool",
    "float",
    "date",
    "datetime",
    "json",
];

/// Enforce the canonical identifier shape used everywhere in the engine:
/// `[A-Za-z_][A-Za-z0-9_]*`, length at most [`MAX_IDENT_LEN`], and not in
/// the reserved-words list. `kind` shows up in the error (e.g. "tabla",
/// "columna", "índice") so users can tell which slot rejected the name.
///
/// Centralising the rule here means parser frontends, the executor and
/// any future migration code share the same definition — there is no
/// "good" identifier in one layer and a "bad" identifier in another.
pub fn validate_identifier(name: &str, kind: &str) -> DbResult<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(coded(
            codes::INVALID_IDENTIFIER,
            format!("nombre de {} vacío", kind),
        ));
    }
    if trimmed.len() > MAX_IDENT_LEN {
        return Err(coded(
            codes::INVALID_IDENTIFIER,
            format!(
                "nombre de {} '{}' excede el máximo de {} caracteres (tiene {})",
                kind,
                trimmed,
                MAX_IDENT_LEN,
                trimmed.len()
            ),
        ));
    }
    let mut chars = trimmed.chars();
    let first = chars.next().expect("non-empty checked above");
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(coded(
            codes::INVALID_IDENTIFIER,
            format!(
                "nombre de {} '{}' inválido: debe empezar con letra o '_'",
                kind, trimmed
            ),
        ));
    }
    for ch in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '_') {
            return Err(coded(
                codes::INVALID_IDENTIFIER,
                format!(
                    "nombre de {} '{}' inválido: solo se admiten [A-Za-z0-9_]",
                    kind, trimmed
                ),
            ));
        }
    }
    let lower = trimmed.to_ascii_lowercase();
    if RESERVED_WORDS.iter().any(|w| *w == lower) {
        return Err(coded(
            codes::INVALID_IDENTIFIER,
            format!(
                "nombre de {} '{}' es palabra reservada del motor",
                kind, trimmed
            ),
        ));
    }
    Ok(())
}

fn validate_default_against_type(
    column: &str,
    column_type: &ColumnType,
    default: &DefaultLiteral,
) -> DbResult<()> {
    let ok = match (column_type, default) {
        (_, DefaultLiteral::Null) => true,
        (ColumnType::Int, DefaultLiteral::Integer(_)) => true,
        (ColumnType::Float, DefaultLiteral::Float(_) | DefaultLiteral::Integer(_)) => true,
        (ColumnType::Bool, DefaultLiteral::Bool(_)) => true,
        (ct, DefaultLiteral::String(_)) if ct.stores_as_text() => true,
        _ => false,
    };
    if !ok {
        return Err(DbError::new(format!(
            "columna '{}': DEFAULT incompatible con tipo {}",
            column,
            column_type.as_sql()
        )));
    }
    Ok(())
}

/// Stable on-disk hash for catalog keys. FNV-1a 64-bit. We must not depend on
/// `std::collections::hash_map::DefaultHasher` here: the standard library
/// explicitly does not guarantee its output is stable across Rust versions,
/// and the result of this function is persisted inside DB files.
fn hash_name(name: &str) -> i64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let normalized = name.trim().to_ascii_lowercase();
    let mut hash = FNV_OFFSET_BASIS;
    for byte in normalized.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash as i64
}

fn push_string(out: &mut Vec<u8>, value: &str) -> DbResult<()> {
    let bytes = value.as_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err(DbError::new(format!(
            "string demasiado largo para serializar: {} bytes, máximo soportado es {} (u16::MAX)",
            bytes.len(),
            u16::MAX
        )));
    }
    out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn take_string(data: &[u8], offset: &mut usize) -> DbResult<String> {
    if *offset + 2 > data.len() {
        return Err(DbError::new(format!(
            "string serializado corrupto: necesito 2 bytes para el header de longitud \
             en offset {}, solo quedan {} bytes",
            *offset,
            data.len().saturating_sub(*offset)
        )));
    }
    let len = u16::from_le_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize;
    *offset += 2;
    if *offset + len > data.len() {
        return Err(DbError::new(format!(
            "string serializado corrupto en offset {}: header declara {} bytes \
             pero solo quedan {} bytes en el buffer",
            *offset - 2,
            len,
            data.len() - *offset
        )));
    }
    let value = String::from_utf8(data[*offset..*offset + len].to_vec())?;
    *offset += len;
    Ok(value)
}
