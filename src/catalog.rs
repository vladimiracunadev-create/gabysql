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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub column_type: ColumnType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableMeta {
    pub name: String,
    pub primary_key: String,
    pub columns: Vec<Column>,
    pub root_page: u32,
}

impl TableMeta {
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(name))
    }

    pub fn serialize(&self) -> DbResult<Vec<u8>> {
        let mut out = Vec::new();
        push_string(&mut out, &self.name)?;
        push_string(&mut out, &self.primary_key)?;
        out.extend_from_slice(&self.root_page.to_le_bytes());
        out.extend_from_slice(&(self.columns.len() as u16).to_le_bytes());
        for column in &self.columns {
            push_string(&mut out, &column.name)?;
            out.push(column.column_type.code());
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
            if offset >= data.len() {
                return Err(DbError::new("meta de tabla corrupta"));
            }
            let column_type = ColumnType::from_code(data[offset])?;
            offset += 1;
            columns.push(Column { name, column_type });
        }
        Ok(Self {
            name,
            primary_key,
            columns,
            root_page,
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
            pk_ok = true;
        }
    }
    if !pk_ok {
        return Err(DbError::new("PRIMARY KEY debe existir en columnas"));
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
