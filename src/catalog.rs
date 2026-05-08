use crate::bptree::{init_leaf_page, Tree};
use crate::storage::Pager;
use crate::{DbError, DbResult};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
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
            _ => Err(DbError::new("tipo de columna inválido")),
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
            return Err(DbError::new("default corrupto (kind)"));
        }
        let kind = data[*offset];
        *offset += 1;
        Ok(match kind {
            0 => Self::Null,
            1 => {
                if *offset + 8 > data.len() {
                    return Err(DbError::new("default corrupto (int)"));
                }
                let n = i64::from_le_bytes(data[*offset..*offset + 8].try_into().unwrap());
                *offset += 8;
                Self::Integer(n)
            }
            2 => {
                if *offset + 8 > data.len() {
                    return Err(DbError::new("default corrupto (float)"));
                }
                let n = f64::from_le_bytes(data[*offset..*offset + 8].try_into().unwrap());
                *offset += 8;
                Self::Float(n)
            }
            3 => {
                if *offset >= data.len() {
                    return Err(DbError::new("default corrupto (bool)"));
                }
                let b = data[*offset] != 0;
                *offset += 1;
                Self::Bool(b)
            }
            4 => Self::String(take_string(data, offset)?),
            _ => return Err(DbError::new("default corrupto (kind desconocido)")),
        })
    }
}

const COLUMN_FLAG_NOT_NULL: u8 = 0x01;
const COLUMN_FLAG_HAS_DEFAULT: u8 = 0x02;

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    pub column_type: ColumnType,
    pub not_null: bool,
    pub default: Option<DefaultLiteral>,
}

impl Column {
    pub fn plain(name: impl Into<String>, column_type: ColumnType) -> Self {
        Self {
            name: name.into(),
            column_type,
            not_null: false,
            default: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexMeta {
    pub name: String,
    pub column: String,
    pub root_page: u32,
    pub unique: bool,
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

    /// VERSION = 5 on-disk layout for a TableMeta record:
    ///
    ///     [name][primary_key][root_page:u32]
    ///     [col_count:u16] · col_count × {
    ///         [name][type_code:u8][flags:u8]
    ///         flags & 0x02 ? DefaultLiteral payload : ∅
    ///     }
    ///     [idx_count:u16] · idx_count × {
    ///         [name][column][root_page:u32][unique:u8]
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
            out.push(flags);
            if let Some(default) = &column.default {
                default.encode_into(&mut out)?;
            }
        }
        out.extend_from_slice(&(self.indexes.len() as u16).to_le_bytes());
        for idx in &self.indexes {
            push_string(&mut out, &idx.name)?;
            push_string(&mut out, &idx.column)?;
            out.extend_from_slice(&idx.root_page.to_le_bytes());
            out.push(u8::from(idx.unique));
        }
        Ok(out)
    }

    pub fn deserialize(data: &[u8]) -> DbResult<Self> {
        let mut offset = 0usize;
        let name = take_string(data, &mut offset)?;
        let primary_key = take_string(data, &mut offset)?;
        if offset + 6 > data.len() {
            return Err(DbError::new("meta de tabla corrupta"));
        }
        let root_page = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let count = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        let mut columns = Vec::with_capacity(count);
        for _ in 0..count {
            let name = take_string(data, &mut offset)?;
            if offset + 2 > data.len() {
                return Err(DbError::new("meta de tabla corrupta (column header)"));
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
            columns.push(Column {
                name,
                column_type,
                not_null,
                default,
            });
        }
        if offset + 2 > data.len() {
            return Err(DbError::new("meta de tabla corrupta (index count)"));
        }
        let idx_count = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        let mut indexes = Vec::with_capacity(idx_count);
        for _ in 0..idx_count {
            let name = take_string(data, &mut offset)?;
            let column = take_string(data, &mut offset)?;
            if offset + 5 > data.len() {
                return Err(DbError::new("meta de índice corrupta"));
            }
            let root_page = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            let unique = data[offset] != 0;
            offset += 1;
            indexes.push(IndexMeta {
                name,
                column,
                root_page,
                unique,
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
            return Err(DbError::new("colisión de hash en catálogo"));
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
    if meta.name.trim().is_empty() {
        return Err(DbError::new("nombre de tabla vacío"));
    }
    if meta.primary_key.trim().is_empty() {
        return Err(DbError::new(
            "PRIMARY KEY requerida (esta versión solo soporta una PK escalar de tipo INT)",
        ));
    }
    if meta.columns.is_empty() {
        return Err(DbError::new("se requieren columnas"));
    }

    let mut seen = HashSet::new();
    let mut pk_ok = false;
    for column in &meta.columns {
        if column.name.trim().is_empty() {
            return Err(DbError::new("columna sin nombre"));
        }
        let normalized = column.name.to_ascii_lowercase();
        if !seen.insert(normalized) {
            return Err(DbError::new("nombre de columna duplicado"));
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
        return Err(DbError::new("string demasiado largo"));
    }
    out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn take_string(data: &[u8], offset: &mut usize) -> DbResult<String> {
    if *offset + 2 > data.len() {
        return Err(DbError::new("string corrupto"));
    }
    let len = u16::from_le_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize;
    *offset += 2;
    if *offset + len > data.len() {
        return Err(DbError::new("string corrupto"));
    }
    let value = String::from_utf8(data[*offset..*offset + len].to_vec())?;
    *offset += len;
    Ok(value)
}
