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

    pub fn code(&self) -> u8 {
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

    pub fn from_code(code: u8) -> DbResult<Self> {
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
/// deleted.
///
/// Bloque L (VERSION 9): extendido con `SetNull` y `SetDefault`. El
/// código binario en disco es estable (0=Restrict, 1=Cascade, 2=SetNull,
/// 3=SetDefault). `NO ACTION` del estándar se acepta como sinónimo de
/// `Restrict` (no hay constraint mode diferido todavía).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDelete {
    /// Refuse the parent DELETE if any child row still references it.
    /// Default behaviour when `ON DELETE` is omitted.
    Restrict,
    /// Delete every child row that references the parent. Cascading
    /// deletes can chain through several tables; the engine guards
    /// against cycles using a visited set on `(table, pk)`.
    Cascade,
    /// Bloque L: poner NULL en la columna FK de cada fila hija. Falla
    /// con `[GBY-3009] FK_SET_NULL_VIOLATES_NOT_NULL` si la columna del
    /// child está declarada `NOT NULL`.
    SetNull,
    /// Bloque L: reasignar la columna FK de cada fila hija a su DEFAULT
    /// declarado. Falla con `[GBY-3010] FK_SET_DEFAULT_MISSING` si no
    /// hay DEFAULT en esa columna.
    SetDefault,
}

impl OnDelete {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::Restrict => 0,
            Self::Cascade => 1,
            Self::SetNull => 2,
            Self::SetDefault => 3,
        }
    }

    pub(crate) fn from_code(code: u8) -> DbResult<Self> {
        match code {
            0 => Ok(Self::Restrict),
            1 => Ok(Self::Cascade),
            2 => Ok(Self::SetNull),
            3 => Ok(Self::SetDefault),
            other => Err(DbError::new(format!(
                "FOREIGN KEY on_delete code desconocido en disco: {} \
                 (esperaba 0=RESTRICT, 1=CASCADE, 2=SET NULL, 3=SET DEFAULT)",
                other
            ))),
        }
    }

    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Restrict => "RESTRICT",
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::SetDefault => "SET DEFAULT",
        }
    }
}

/// Bloque L (VERSION 9): acción a aplicar cuando se actualiza la PK del
/// padre referenciado por una FK. Como gabysql prohíbe UPDATE sobre la
/// PRIMARY KEY (`[GBY-4008] UPDATE_PK_NOT_ALLOWED`), hoy el motor sólo
/// persiste el byte para que el catálogo roundtrippee y un release
/// futuro lo pueda activar sin otro bump de formato. `NoAction` es el
/// default cuando se omite `ON UPDATE`, igual que en ANSI/PostgreSQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnUpdate {
    NoAction,
    Cascade,
    SetNull,
    SetDefault,
    Restrict,
}

impl OnUpdate {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::NoAction => 0,
            Self::Cascade => 1,
            Self::SetNull => 2,
            Self::SetDefault => 3,
            Self::Restrict => 4,
        }
    }

    pub(crate) fn from_code(code: u8) -> DbResult<Self> {
        match code {
            0 => Ok(Self::NoAction),
            1 => Ok(Self::Cascade),
            2 => Ok(Self::SetNull),
            3 => Ok(Self::SetDefault),
            4 => Ok(Self::Restrict),
            other => Err(DbError::new(format!(
                "FOREIGN KEY on_update code desconocido en disco: {} (esperaba 0..=4)",
                other
            ))),
        }
    }

    pub fn as_sql(self) -> &'static str {
        match self {
            Self::NoAction => "NO ACTION",
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::SetDefault => "SET DEFAULT",
            Self::Restrict => "RESTRICT",
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
    /// Bloque L (VERSION 9): acción al actualizar la PK del padre.
    /// `NoAction` para FKs creadas pre-L (default cuando se omite
    /// `ON UPDATE`). Se persiste como byte a continuación del
    /// `on_delete` en el record on-disk.
    pub on_update: OnUpdate,
    /// Residual #2 (VERSION 11, 2026-05-27): nombre explícito de la FK
    /// declarado con `CONSTRAINT <name> FOREIGN KEY (...) REFERENCES ...`.
    /// `None` si la FK vino inline en la columna sin nombre. Se usa
    /// para `ALTER TABLE DROP CONSTRAINT <name>` y para mensajes de
    /// error legibles.
    pub name: Option<String>,
    /// Residual #3 (VERSION 12, 2026-05-27): columnas adicionales del
    /// child (source) cuando la FK es multi-columna. La FK queda
    /// anchored en la primera columna del child (`Column.references`
    /// vive en esa columna); el resto del orden declarado vive acá.
    /// Vacío para FK single-col (caso histórico). Mismo trato all-INT
    /// NOT NULL que K2 — el FK target debe ser la PK compuesta del
    /// padre, que también es all-INT.
    pub extra_source_columns: Vec<String>,
    /// Residual #3 (VERSION 12, 2026-05-27): columnas adicionales del
    /// padre (target) en el mismo orden que `extra_source_columns`.
    /// Para FK single-col queda vacío y el target completo es
    /// `column` (campo histórico). Para FK multi-col,
    /// `[column] + extra_target_columns` es la lista de columnas del
    /// padre, que el motor exige sea exactamente la PK compuesta del
    /// padre (single-col PK queda igual que pre-#3).
    pub extra_target_columns: Vec<String>,
}

impl ForeignKeyMeta {
    /// Devuelve la lista completa de columnas source en el orden
    /// declarado por el usuario. Single-col → tamaño 1; multi-col →
    /// tamaño ≥ 2.
    ///
    /// Necesita el `anchor_col_name` porque la primera columna source
    /// vive en la `Column` que aloja este `ForeignKeyMeta`, no acá.
    pub fn source_columns<'a>(&'a self, anchor_col_name: &'a str) -> Vec<&'a str> {
        let mut out = Vec::with_capacity(1 + self.extra_source_columns.len());
        out.push(anchor_col_name);
        for c in &self.extra_source_columns {
            out.push(c.as_str());
        }
        out
    }

    /// Devuelve la lista completa de columnas target del padre.
    pub fn target_columns(&self) -> Vec<&str> {
        let mut out = Vec::with_capacity(1 + self.extra_target_columns.len());
        out.push(self.column.as_str());
        for c in &self.extra_target_columns {
            out.push(c.as_str());
        }
        out
    }

    /// `true` si la FK abarca más de una columna.
    pub fn is_composite(&self) -> bool {
        !self.extra_source_columns.is_empty()
    }
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
    /// Primera columna del índice. Pre-K2 era la única; en VERSION 8
    /// puede haber más en `extra_columns`. Se preserva para mantener
    /// el código legacy single-column intacto: cuando `extra_columns`
    /// está vacío, el índice se comporta exactamente como pre-K2.
    pub column: String,
    pub root_page: u32,
    pub unique: bool,
    /// V7+: distinguishes legacy hash-bucket indexes from new
    /// INT-ordered indexes that support range scan. See [`IndexKind`].
    pub kind: IndexKind,
    /// Bloque K2 (VERSION 8): columnas adicionales del índice cuando es
    /// compuesto. Vacío para índices single-column (la mayoría). El
    /// orden importa: el fingerprint FNV-1a-64 se computa en el orden
    /// `[column] + extra_columns`. Cuando no está vacío, `kind` siempre
    /// es `Hash` y todas las columnas deben ser INT NOT NULL.
    pub extra_columns: Vec<String>,
}

impl IndexMeta {
    /// Devuelve la lista completa de columnas del índice en el orden
    /// canónico (`column` primero, después `extra_columns`). Single-column
    /// → Vec de tamaño 1; compuesto → tamaño ≥ 2.
    pub fn all_columns(&self) -> Vec<&str> {
        let mut out = Vec::with_capacity(1 + self.extra_columns.len());
        out.push(self.column.as_str());
        for c in &self.extra_columns {
            out.push(c.as_str());
        }
        out
    }

    /// `true` si el índice cubre más de una columna.
    pub fn is_composite(&self) -> bool {
        !self.extra_columns.is_empty()
    }
}

/// Bloque L2 (VERSION 10): un `CHECK (expr)` declarado en una tabla.
///
/// Persistimos el **texto SQL canónico** de la expresión (re-formateado
/// por `format_expr`) en vez del AST. Razones:
///
/// 1. Desacopla el formato on-disk del AST de `Expr`, que evoluciona
///    con cada bloque G/H/I. Cualquier feature de Expr que
///    `format_expr` no sepa serializar falla en el `CREATE TABLE` (un
///    sólo punto), no en cada `INSERT` posterior contra un AST corrupto.
/// 2. Round-trip estable: el lexer/parser ya existen como API pública,
///    así que la conversión texto → AST en cada write es barata
///    relativa al I/O.
/// 3. Catálogo legible — `INTEGRITY CHECK` y futuras vistas
///    `INFORMATION_SCHEMA` ven el SQL literal del usuario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckConstraint {
    /// Nombre del constraint. Si el usuario no declaró
    /// `CONSTRAINT name CHECK (...)`, el parser sintetiza
    /// `<table>_check_<N>` (N empezando en 1) para reportes legibles.
    pub name: String,
    /// Texto SQL canónico de la expresión (`x > 0`, `LENGTH(name) <= 50`,
    /// etc.). Re-parseable por `gabysql::sql::parse_expr_str`.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableMeta {
    pub name: String,
    /// Primera columna de la PRIMARY KEY. Para PK single (el caso
    /// histórico) es la única; para PK compuesta es la primera del
    /// orden declarado en `PRIMARY KEY (a, b, ...)`.
    pub primary_key: String,
    /// Bloque K2 (VERSION 8): columnas adicionales de una PRIMARY KEY
    /// compuesta. Vacío para PK single. Cuando no está vacío, todas las
    /// columnas de la PK (incluyendo `primary_key`) deben ser INT
    /// NOT NULL — la PK compuesta se representa internamente como un
    /// fingerprint FNV-1a-64 i64 (ver ADR-0019).
    pub primary_key_extra: Vec<String>,
    /// Residual #2 (VERSION 11, 2026-05-27): nombre explícito de la PK
    /// declarado con `CONSTRAINT <name> PRIMARY KEY (...)`. `None` si la
    /// PK se declaró inline (`id INT PRIMARY KEY`) o table-level sin
    /// nombre. La PK NO se puede borrar con `DROP CONSTRAINT`; el campo
    /// sirve para mensajes de error y futuro `INFORMATION_SCHEMA`.
    pub primary_key_name: Option<String>,
    pub columns: Vec<Column>,
    pub root_page: u32,
    pub indexes: Vec<IndexMeta>,
    /// Bloque L2 (VERSION 10): constraints `CHECK (expr)` declarados a
    /// nivel de columna o de tabla. Vacío para tablas pre-L2. Se evalúan
    /// en cada INSERT/UPDATE/UPSERT (DO UPDATE); NULL pasa (3VL ANSI),
    /// FALSE rebota con `[GBY-3008]`.
    pub check_constraints: Vec<CheckConstraint>,
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
            .find(|idx| idx.column.eq_ignore_ascii_case(column) && !idx.is_composite())
    }

    pub fn index_by_name(&self, name: &str) -> Option<&IndexMeta> {
        self.indexes
            .iter()
            .find(|idx| idx.name.eq_ignore_ascii_case(name))
    }

    /// Devuelve la lista completa de columnas de la PK en el orden
    /// canónico (`primary_key` primero, después `primary_key_extra`).
    /// PK single → tamaño 1; PK compuesta → tamaño ≥ 2.
    pub fn pk_columns(&self) -> Vec<&str> {
        let mut out = Vec::with_capacity(1 + self.primary_key_extra.len());
        out.push(self.primary_key.as_str());
        for c in &self.primary_key_extra {
            out.push(c.as_str());
        }
        out
    }

    /// `true` si la PRIMARY KEY abarca más de una columna.
    pub fn has_composite_pk(&self) -> bool {
        !self.primary_key_extra.is_empty()
    }

    /// Compara case-insensitive si la columna dada participa en la PK.
    pub fn is_pk_column(&self, column: &str) -> bool {
        self.pk_columns()
            .iter()
            .any(|c| c.eq_ignore_ascii_case(column))
    }

    /// VERSION = 12 on-disk layout for a TableMeta record:
    ///
    ///     [name]
    ///     [pk_count:u8] · pk_count × [pk_col_name]   (pk_count >= 1)
    ///     [pk_name_present:u8] · pk_name_present ? [pk_name] : ∅   ← V11
    ///     [root_page:u32]
    ///     [col_count:u16] · col_count × {
    ///         [name][type_code:u8][flags:u8]
    ///         flags & 0x02 ? DefaultLiteral payload : ∅
    ///         flags & 0x04 ? [target_table][target_column]
    ///                        [on_delete:u8][on_update:u8]
    ///                        [fk_name_present:u8] · fk_name_present ?
    ///                              [fk_name] : ∅
    ///                        [fk_extra_count:u8] · extra ×             ← V12
    ///                              [extra_source_col] · extra ×
    ///                              [extra_target_col]
    ///                        : ∅
    ///     }
    ///     [idx_count:u16] · idx_count × {
    ///         [name][column][root_page:u32][unique:u8][kind:u8]
    ///         [extra_cols_count:u8] · extra × [extra_col_name]   (>= 0)
    ///     }
    ///     [check_count:u16] · check_count × { [name][source] }
    ///
    /// Cambios vs VERSION 11 (Residual #3):
    ///   - Cada FK record añade al final
    ///     `[fk_extra_count:u8]` + N strings source + N strings target,
    ///     en ese orden. FKs single-col escriben count=0 y son
    ///     indistinguibles del caso pre-#3.
    ///   - V11 files se rechazan al abrir con `[GBY-1003]`.
    pub fn serialize(&self) -> DbResult<Vec<u8>> {
        let mut out = Vec::new();
        push_string(&mut out, &self.name)?;
        // PK: [u8:count][string×count]
        let pk_total = 1 + self.primary_key_extra.len();
        if pk_total > u8::MAX as usize {
            return Err(DbError::new(format!(
                "PRIMARY KEY de '{}' tiene {} columnas, máximo soportado es {}",
                self.name,
                pk_total,
                u8::MAX
            )));
        }
        out.push(pk_total as u8);
        push_string(&mut out, &self.primary_key)?;
        for col in &self.primary_key_extra {
            push_string(&mut out, col)?;
        }
        // V11: pk_name optional. Byte 1 = presente seguido de string,
        // byte 0 = ausente. Pre-V11 no había nada acá; la diferencia se
        // detecta por el bump de VERSION (V10 ya está rechazado).
        match &self.primary_key_name {
            Some(name) => {
                out.push(1);
                push_string(&mut out, name)?;
            }
            None => out.push(0),
        }
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
                out.push(fk.on_update.code());
                // V11: fk_name optional. Mismo esquema present-byte.
                match &fk.name {
                    Some(n) => {
                        out.push(1);
                        push_string(&mut out, n)?;
                    }
                    None => out.push(0),
                }
                // V12 (residual #3): columnas extra de la FK
                // multi-col, source + target. Single-col FKs escriben
                // 0 en ambos counts.
                if fk.extra_source_columns.len() != fk.extra_target_columns.len() {
                    return Err(DbError::new(format!(
                        "FOREIGN KEY '{}.{}' tiene arity inconsistente: \
                         {} columnas source extra vs {} columnas target extra",
                        self.name,
                        column.name,
                        fk.extra_source_columns.len(),
                        fk.extra_target_columns.len()
                    )));
                }
                if fk.extra_source_columns.len() > u8::MAX as usize {
                    return Err(DbError::new(format!(
                        "FOREIGN KEY '{}.{}' tiene {} columnas extra, máximo {}",
                        self.name,
                        column.name,
                        fk.extra_source_columns.len(),
                        u8::MAX
                    )));
                }
                out.push(fk.extra_source_columns.len() as u8);
                for c in &fk.extra_source_columns {
                    push_string(&mut out, c)?;
                }
                for c in &fk.extra_target_columns {
                    push_string(&mut out, c)?;
                }
            }
        }
        out.extend_from_slice(&(self.indexes.len() as u16).to_le_bytes());
        for idx in &self.indexes {
            push_string(&mut out, &idx.name)?;
            push_string(&mut out, &idx.column)?;
            out.extend_from_slice(&idx.root_page.to_le_bytes());
            out.push(u8::from(idx.unique));
            out.push(idx.kind.code());
            // K2 trailer: columnas adicionales del índice compuesto.
            if idx.extra_columns.len() > u8::MAX as usize {
                return Err(DbError::new(format!(
                    "índice '{}' tiene {} columnas extra, máximo soportado es {}",
                    idx.name,
                    idx.extra_columns.len(),
                    u8::MAX
                )));
            }
            out.push(idx.extra_columns.len() as u8);
            for col in &idx.extra_columns {
                push_string(&mut out, col)?;
            }
        }
        // Bloque L2 (VERSION 10): trailer de CHECK constraints.
        if self.check_constraints.len() > u16::MAX as usize {
            return Err(DbError::new(format!(
                "tabla '{}' tiene {} CHECK constraints, máximo soportado es {}",
                self.name,
                self.check_constraints.len(),
                u16::MAX
            )));
        }
        out.extend_from_slice(&(self.check_constraints.len() as u16).to_le_bytes());
        for ck in &self.check_constraints {
            push_string(&mut out, &ck.name)?;
            push_string(&mut out, &ck.source)?;
        }
        Ok(out)
    }

    pub fn deserialize(data: &[u8]) -> DbResult<Self> {
        let mut offset = 0usize;
        let name = take_string(data, &mut offset)?;
        // K2 (VERSION 8): la PK es [u8:count][string×count].
        if offset >= data.len() {
            return Err(DbError::new(format!(
                "TableMeta '{}' corrupta: falta el byte pk_count en offset {}",
                name, offset
            )));
        }
        let pk_count = data[offset] as usize;
        offset += 1;
        if pk_count == 0 {
            return Err(DbError::new(format!(
                "TableMeta '{}' corrupta: pk_count=0 (toda tabla debe tener PRIMARY KEY)",
                name
            )));
        }
        let primary_key = take_string(data, &mut offset)?;
        let mut primary_key_extra = Vec::with_capacity(pk_count - 1);
        for _ in 1..pk_count {
            primary_key_extra.push(take_string(data, &mut offset)?);
        }
        // V11: pk_name optional byte.
        if offset >= data.len() {
            return Err(DbError::new(format!(
                "TableMeta '{}' corrupta: falta el byte pk_name_present en offset {}",
                name, offset
            )));
        }
        let pk_name_present = data[offset];
        offset += 1;
        let primary_key_name = if pk_name_present != 0 {
            Some(take_string(data, &mut offset)?)
        } else {
            None
        };
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
                // Bloque L1 (VERSION 9): byte `on_update` a continuación.
                if offset >= data.len() {
                    return Err(DbError::new(format!(
                        "FOREIGN KEY corrupta en '{}.{}': faltan bytes para on_update en offset {}",
                        name, col_name, offset
                    )));
                }
                let on_update = OnUpdate::from_code(data[offset])?;
                offset += 1;
                // V11: fk_name optional byte.
                if offset >= data.len() {
                    return Err(DbError::new(format!(
                        "FOREIGN KEY corrupta en '{}.{}': falta byte fk_name_present en offset {}",
                        name, col_name, offset
                    )));
                }
                let fk_name_present = data[offset];
                offset += 1;
                let fk_name = if fk_name_present != 0 {
                    Some(take_string(data, &mut offset)?)
                } else {
                    None
                };
                // V12 (residual #3): extra columnas multi-col. Source
                // y target tienen el mismo count; el byte se lee una
                // vez y los strings vienen primero source, después target.
                if offset >= data.len() {
                    return Err(DbError::new(format!(
                        "FOREIGN KEY corrupta en '{}.{}': falta byte fk_extra_count en offset {}",
                        name, col_name, offset
                    )));
                }
                let fk_extra_count = data[offset] as usize;
                offset += 1;
                let mut extra_source_columns = Vec::with_capacity(fk_extra_count);
                for _ in 0..fk_extra_count {
                    extra_source_columns.push(take_string(data, &mut offset)?);
                }
                let mut extra_target_columns = Vec::with_capacity(fk_extra_count);
                for _ in 0..fk_extra_count {
                    extra_target_columns.push(take_string(data, &mut offset)?);
                }
                Some(ForeignKeyMeta {
                    table: target_table,
                    column: target_column,
                    on_delete,
                    on_update,
                    name: fk_name,
                    extra_source_columns,
                    extra_target_columns,
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
            // K2 (VERSION 8): trailer con columnas adicionales del índice.
            if offset >= data.len() {
                return Err(DbError::new(format!(
                    "IndexMeta '{}' corrupta en tabla '{}': falta el byte extra_cols_count \
                     en offset {} (len={})",
                    idx_name,
                    name,
                    offset,
                    data.len()
                )));
            }
            let extra_count = data[offset] as usize;
            offset += 1;
            let mut extra_columns = Vec::with_capacity(extra_count);
            for _ in 0..extra_count {
                extra_columns.push(take_string(data, &mut offset)?);
            }
            indexes.push(IndexMeta {
                name: idx_name,
                column,
                root_page,
                unique,
                kind,
                extra_columns,
            });
        }
        // Bloque L2 (VERSION 10): trailer de CHECK constraints. Tablas
        // pre-L2 escriben check_count=0; el `take_string` agotaría buffer
        // sólo si el catálogo está corrupto.
        if offset + 2 > data.len() {
            return Err(DbError::new(format!(
                "TableMeta '{}' corrupta: faltan 2 bytes para check_count en offset {} (len={})",
                name,
                offset,
                data.len()
            )));
        }
        let check_count = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        let mut check_constraints = Vec::with_capacity(check_count);
        for _ in 0..check_count {
            let ck_name = take_string(data, &mut offset)?;
            let source = take_string(data, &mut offset)?;
            check_constraints.push(CheckConstraint {
                name: ck_name,
                source,
            });
        }
        Ok(Self {
            name,
            primary_key,
            primary_key_extra,
            primary_key_name,
            columns,
            root_page,
            indexes,
            check_constraints,
        })
    }
}

/// Bloque V (VERSION 13, 2026-05-27): vista lógica persistida en el
/// catálogo. Sólo guardamos el texto SQL original del `SELECT` que la
/// define; cada `SELECT FROM v` re-parsea ese texto y lo embebe como
/// subquery del FROM. No hay materialización ni caché — la vista
/// computa al vuelo.
///
/// `column_aliases` corresponde a la sintaxis opcional
/// `CREATE VIEW v (a, b, ...) AS SELECT ...` y permite renombrar las
/// columnas del result-set sin tocar el SELECT subyacente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewMeta {
    pub name: String,
    /// Texto SQL del query que define la vista, tal cual lo escribió
    /// el usuario después de `AS`. El motor lo re-tokeniza/parsea en
    /// cada SELECT que la referencia. Sin canonicalización (a
    /// diferencia de CHECK constraints) porque el SELECT es mucho más
    /// rico que un Expr y un round-trip exact-formatter es out of scope.
    pub source: String,
    /// Aliases opcionales del result-set. `None` si la vista no los
    /// declara — en ese caso el SELECT subyacente provee los nombres.
    pub column_aliases: Option<Vec<String>>,
}

impl ViewMeta {
    /// V13 payload encoding for a `ViewMeta`. Vive aparte del
    /// discriminator byte (que lo escribe el `Catalog::put_view`).
    ///
    ///     [name][source][alias_present:u8] · alias_present ?
    ///         [alias_count:u16] · alias_count × [alias_name]
    ///       : ∅
    pub fn serialize(&self) -> DbResult<Vec<u8>> {
        let mut out = Vec::new();
        push_string(&mut out, &self.name)?;
        push_string(&mut out, &self.source)?;
        match &self.column_aliases {
            Some(aliases) => {
                out.push(1);
                if aliases.len() > u16::MAX as usize {
                    return Err(DbError::new(format!(
                        "vista '{}' tiene {} aliases de columna, máximo {}",
                        self.name,
                        aliases.len(),
                        u16::MAX
                    )));
                }
                out.extend_from_slice(&(aliases.len() as u16).to_le_bytes());
                for a in aliases {
                    push_string(&mut out, a)?;
                }
            }
            None => out.push(0),
        }
        Ok(out)
    }

    pub fn deserialize(data: &[u8]) -> DbResult<Self> {
        let mut offset = 0usize;
        let name = take_string(data, &mut offset)?;
        let source = take_string(data, &mut offset)?;
        if offset >= data.len() {
            return Err(DbError::new(format!(
                "ViewMeta '{}' corrupta: falta byte alias_present",
                name
            )));
        }
        let alias_present = data[offset];
        offset += 1;
        let column_aliases = if alias_present != 0 {
            if offset + 2 > data.len() {
                return Err(DbError::new(format!(
                    "ViewMeta '{}' corrupta: faltan 2 bytes para alias_count",
                    name
                )));
            }
            let n = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
            offset += 2;
            let mut aliases = Vec::with_capacity(n);
            for _ in 0..n {
                aliases.push(take_string(data, &mut offset)?);
            }
            Some(aliases)
        } else {
            None
        };
        Ok(Self {
            name,
            source,
            column_aliases,
        })
    }
}

/// Bloque V (VERSION 13): discriminator byte que arranca cada record
/// del catálogo. Permite tener tablas, vistas y triggers conviviendo
/// en el mismo B+Tree del catálogo sin colisiones de schema.
///
/// Bloque X1 (VERSION 14, 2026-05-28): agregado `Trigger`.
/// Bloque X3 (VERSION 15, 2026-05-28): agregado `Procedure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Table,
    View,
    Trigger,
    Procedure,
}

impl ObjectKind {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::Table => 0,
            Self::View => 1,
            Self::Trigger => 2,
            Self::Procedure => 3,
        }
    }

    pub(crate) fn from_code(code: u8) -> DbResult<Self> {
        match code {
            0 => Ok(Self::Table),
            1 => Ok(Self::View),
            2 => Ok(Self::Trigger),
            3 => Ok(Self::Procedure),
            other => Err(DbError::new(format!(
                "kind de objeto desconocido en catálogo: {} (esperaba 0=Table, 1=View, 2=Trigger, 3=Procedure)",
                other
            ))),
        }
    }
}

/// Bloque X1 (VERSION 14): metadata de un trigger. El body se persiste
/// como texto SQL — re-tokenizado y re-parseado en cada fire (mismo
/// patrón que `ViewMeta::source`). Layout on-disk:
///
///     [name][table][timing:u8][event:u8][body_sql]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerMeta {
    pub name: String,
    pub table: String,
    pub timing_code: u8,
    pub event_code: u8,
    pub body_sql: String,
}

impl TriggerMeta {
    pub fn serialize(&self) -> DbResult<Vec<u8>> {
        let mut out = Vec::new();
        push_string(&mut out, &self.name)?;
        push_string(&mut out, &self.table)?;
        out.push(self.timing_code);
        out.push(self.event_code);
        push_string(&mut out, &self.body_sql)?;
        Ok(out)
    }
    pub fn deserialize(data: &[u8]) -> DbResult<Self> {
        let mut offset = 0usize;
        let name = take_string(data, &mut offset)?;
        let table = take_string(data, &mut offset)?;
        if offset + 2 > data.len() {
            return Err(DbError::new(format!(
                "TriggerMeta '{}' corrupta: faltan bytes timing/event",
                name
            )));
        }
        let timing_code = data[offset];
        offset += 1;
        let event_code = data[offset];
        offset += 1;
        let body_sql = take_string(data, &mut offset)?;
        Ok(Self {
            name,
            table,
            timing_code,
            event_code,
            body_sql,
        })
    }
}

/// Bloque X3 (VERSION 15, 2026-05-28): metadata de un stored procedure.
/// Layout on-disk:
///
///     [name][param_count:u16] · param_count × ([param_name][type_code:u8]) · [body_sql]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureMeta {
    pub name: String,
    /// Lista ordenada de parámetros formales con su tipo declarado.
    /// El tipo se valida en CALL (best-effort: el motor hace coerce
    /// implícito INT↔FLOAT; mismatch de TEXT vs INT rebota).
    pub params: Vec<(String, ColumnType)>,
    /// Body persistido como texto SQL (mismo enfoque que TriggerMeta).
    /// Puede ser una sola sentencia DML o un bloque `BEGIN ... END`.
    pub body_sql: String,
}

impl ProcedureMeta {
    pub fn serialize(&self) -> DbResult<Vec<u8>> {
        let mut out = Vec::new();
        push_string(&mut out, &self.name)?;
        if self.params.len() > u16::MAX as usize {
            return Err(DbError::new(format!(
                "procedure '{}' tiene {} params, máximo {}",
                self.name,
                self.params.len(),
                u16::MAX
            )));
        }
        out.extend_from_slice(&(self.params.len() as u16).to_le_bytes());
        for (n, t) in &self.params {
            push_string(&mut out, n)?;
            out.push(t.code());
        }
        push_string(&mut out, &self.body_sql)?;
        Ok(out)
    }
    pub fn deserialize(data: &[u8]) -> DbResult<Self> {
        let mut offset = 0usize;
        let name = take_string(data, &mut offset)?;
        if offset + 2 > data.len() {
            return Err(DbError::new(format!(
                "ProcedureMeta '{}' corrupta: faltan 2 bytes param_count",
                name
            )));
        }
        let pcount = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        let mut params = Vec::with_capacity(pcount);
        for _ in 0..pcount {
            let pname = take_string(data, &mut offset)?;
            if offset >= data.len() {
                return Err(DbError::new(format!(
                    "ProcedureMeta '{}': falta type_code de param '{}'",
                    name, pname
                )));
            }
            let tcode = data[offset];
            offset += 1;
            let ptype = ColumnType::from_code(tcode)?;
            params.push((pname, ptype));
        }
        let body_sql = take_string(data, &mut offset)?;
        Ok(Self {
            name,
            params,
            body_sql,
        })
    }
}

/// Wrapper de lectura para un record del catálogo — tabla, vista, trigger o procedure.
#[derive(Debug, Clone)]
pub enum CatalogObject {
    Table(TableMeta),
    View(ViewMeta),
    Trigger(TriggerMeta),
    Procedure(ProcedureMeta),
}

impl CatalogObject {
    pub fn name(&self) -> &str {
        match self {
            Self::Table(t) => &t.name,
            Self::View(v) => &v.name,
            Self::Trigger(t) => &t.name,
            Self::Procedure(p) => &p.name,
        }
    }
}

/// Bloque V (VERSION 13): decodifica un payload del catálogo
/// dispatcheando por el byte discriminator inicial.
fn decode_catalog_object(data: &[u8]) -> DbResult<CatalogObject> {
    if data.is_empty() {
        return Err(DbError::new(
            "record del catálogo vacío: falta el byte discriminator de ObjectKind",
        ));
    }
    let kind = ObjectKind::from_code(data[0])?;
    let rest = &data[1..];
    Ok(match kind {
        ObjectKind::Table => CatalogObject::Table(TableMeta::deserialize(rest)?),
        ObjectKind::View => CatalogObject::View(ViewMeta::deserialize(rest)?),
        ObjectKind::Trigger => CatalogObject::Trigger(TriggerMeta::deserialize(rest)?),
        ObjectKind::Procedure => CatalogObject::Procedure(ProcedureMeta::deserialize(rest)?),
    })
}

pub struct Catalog<'a> {
    pager: &'a mut Pager,
}

impl<'a> Catalog<'a> {
    pub fn open(pager: &'a mut Pager) -> Self {
        Self { pager }
    }

    /// Bloque V (VERSION 13): los records del catálogo arrancan con un
    /// byte discriminator (`ObjectKind`). `list_objects` devuelve todos,
    /// `list_tables` filtra Tables, `list_views` filtra Views.
    pub fn list_objects(&mut self) -> DbResult<Vec<CatalogObject>> {
        let header = self.pager.header();
        if header.catalog_root_page == 0 {
            return Ok(Vec::new());
        }
        let mut tree = Tree::new(self.pager);
        let kvs = tree.all(header.catalog_root_page)?;
        kvs.into_iter()
            .map(|kv| decode_catalog_object(&kv.value))
            .collect()
    }

    pub fn list_tables(&mut self) -> DbResult<Vec<TableMeta>> {
        Ok(self
            .list_objects()?
            .into_iter()
            .filter_map(|o| match o {
                CatalogObject::Table(t) => Some(t),
                _ => None,
            })
            .collect())
    }

    pub fn list_views(&mut self) -> DbResult<Vec<ViewMeta>> {
        Ok(self
            .list_objects()?
            .into_iter()
            .filter_map(|o| match o {
                CatalogObject::View(v) => Some(v),
                _ => None,
            })
            .collect())
    }

    /// Bloque X1: lista todos los triggers del catálogo. El executor
    /// los filtra in-memory por tabla/event/timing antes de fire.
    pub fn list_triggers(&mut self) -> DbResult<Vec<TriggerMeta>> {
        Ok(self
            .list_objects()?
            .into_iter()
            .filter_map(|o| match o {
                CatalogObject::Trigger(t) => Some(t),
                _ => None,
            })
            .collect())
    }

    /// Bloque X3: lista todas las procedures del catálogo.
    pub fn list_procedures(&mut self) -> DbResult<Vec<ProcedureMeta>> {
        Ok(self
            .list_objects()?
            .into_iter()
            .filter_map(|o| match o {
                CatalogObject::Procedure(p) => Some(p),
                _ => None,
            })
            .collect())
    }

    /// Lookup case-insensitive por nombre. Devuelve el objeto sea
    /// table o view; se usa cuando el caller no sabe (e.g. resolver
    /// de FROM, ALTER TABLE rechazando colisión con vista).
    pub fn get_object(&mut self, name: &str) -> DbResult<Option<CatalogObject>> {
        let header = self.pager.header();
        if header.catalog_root_page == 0 {
            return Ok(None);
        }
        let key = hash_name(name);
        let mut tree = Tree::new(self.pager);
        if let Some(bytes) = tree.get(header.catalog_root_page, key)? {
            let obj = decode_catalog_object(&bytes)?;
            if obj.name().eq_ignore_ascii_case(name) {
                return Ok(Some(obj));
            }
            return Err(DbError::new(format!(
                "colisión de hash FNV-1a-64 en el catálogo: \
                 se buscó '{}' pero el bucket contiene '{}'. Reporte este caso \
                 como bug.",
                name,
                obj.name()
            )));
        }
        Ok(None)
    }

    pub fn get_table(&mut self, name: &str) -> DbResult<Option<TableMeta>> {
        match self.get_object(name)? {
            Some(CatalogObject::Table(t)) => Ok(Some(t)),
            _ => Ok(None),
        }
    }

    pub fn get_view(&mut self, name: &str) -> DbResult<Option<ViewMeta>> {
        match self.get_object(name)? {
            Some(CatalogObject::View(v)) => Ok(Some(v)),
            _ => Ok(None),
        }
    }

    pub fn get_trigger(&mut self, name: &str) -> DbResult<Option<TriggerMeta>> {
        match self.get_object(name)? {
            Some(CatalogObject::Trigger(t)) => Ok(Some(t)),
            _ => Ok(None),
        }
    }

    pub fn get_procedure(&mut self, name: &str) -> DbResult<Option<ProcedureMeta>> {
        match self.get_object(name)? {
            Some(CatalogObject::Procedure(p)) => Ok(Some(p)),
            _ => Ok(None),
        }
    }

    pub fn put_table(&mut self, meta: &TableMeta) -> DbResult<()> {
        let root = self.ensure_root()?;
        let key = hash_name(&meta.name);
        let mut payload = Vec::with_capacity(1 + 64);
        payload.push(ObjectKind::Table.code());
        payload.extend_from_slice(&meta.serialize()?);
        let mut tree = Tree::new(self.pager);
        tree.upsert(root, key, payload)?;
        Ok(())
    }

    pub fn put_view(&mut self, meta: &ViewMeta) -> DbResult<()> {
        let root = self.ensure_root()?;
        let key = hash_name(&meta.name);
        let mut payload = Vec::with_capacity(1 + 32);
        payload.push(ObjectKind::View.code());
        payload.extend_from_slice(&meta.serialize()?);
        let mut tree = Tree::new(self.pager);
        tree.upsert(root, key, payload)?;
        Ok(())
    }

    pub fn put_trigger(&mut self, meta: &TriggerMeta) -> DbResult<()> {
        let root = self.ensure_root()?;
        let key = hash_name(&meta.name);
        let mut payload = Vec::with_capacity(1 + 32);
        payload.push(ObjectKind::Trigger.code());
        payload.extend_from_slice(&meta.serialize()?);
        let mut tree = Tree::new(self.pager);
        tree.upsert(root, key, payload)?;
        Ok(())
    }

    pub fn put_procedure(&mut self, meta: &ProcedureMeta) -> DbResult<()> {
        let root = self.ensure_root()?;
        let key = hash_name(&meta.name);
        let mut payload = Vec::with_capacity(1 + 32);
        payload.push(ObjectKind::Procedure.code());
        payload.extend_from_slice(&meta.serialize()?);
        let mut tree = Tree::new(self.pager);
        tree.upsert(root, key, payload)?;
        Ok(())
    }

    /// Remove the catalog entry for the named object (table or view).
    /// Las páginas que respaldan la tabla y sus índices NO se liberan
    /// (mismo contrato pre-V de `remove_table`); para vistas no hay
    /// páginas asociadas — sólo se borra la entrada del catálogo.
    pub fn remove_object(&mut self, name: &str) -> DbResult<bool> {
        let header = self.pager.header();
        if header.catalog_root_page == 0 {
            return Ok(false);
        }
        let key = hash_name(name);
        let mut tree = Tree::new(self.pager);
        tree.delete(header.catalog_root_page, key)
    }

    /// Alias histórico de `remove_object`. Pre-V era el único path de
    /// borrado del catálogo.
    pub fn remove_table(&mut self, name: &str) -> DbResult<bool> {
        self.remove_object(name)
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
            "PRIMARY KEY requerida (esta versión admite PK escalar INT o PK compuesta multi-INT NOT NULL)",
        ));
    }
    if meta.columns.is_empty() {
        return Err(DbError::new(format!(
            "CREATE TABLE '{}' rechazado: debe declarar al menos una columna",
            meta.name
        )));
    }

    let mut seen = HashSet::new();
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

    // Validar columnas PK — una a una. Para la PK single (caso histórico)
    // exigimos INT. Para la PK compuesta exigimos además NOT NULL en toda
    // columna (4064): el fingerprint i64 no puede representar NULL sin
    // ambigüedad y SQL UNIQUE/PK no admite NULLs en columnas de PK.
    let is_composite = meta.has_composite_pk();
    // Dedup case-insensitive sobre las columnas PK declaradas.
    let mut pk_seen = HashSet::new();
    for pk_col_name in meta.pk_columns() {
        let lower = pk_col_name.to_ascii_lowercase();
        if !pk_seen.insert(lower) {
            return Err(coded(
                codes::PRIMARY_KEY_DUPLICATED,
                format!(
                    "CREATE TABLE '{}' rechazado: la columna '{}' aparece dos veces en PRIMARY KEY",
                    meta.name, pk_col_name
                ),
            ));
        }
        let col = meta.column(pk_col_name).ok_or_else(|| {
            DbError::new(format!(
                "PRIMARY KEY '{}' no existe en columnas de '{}'",
                pk_col_name, meta.name
            ))
        })?;
        if col.column_type != ColumnType::Int {
            if is_composite {
                return Err(coded(
                    codes::COMPOSITE_PK_REQUIRES_ALL_INT,
                    format!(
                        "PRIMARY KEY compuesta de '{}' rechazada: columna '{}' es {} (debe ser INT). \
                         La PK compuesta en VERSION 8 está restringida a multi-INT NOT NULL — \
                         ver ADR-0019.",
                        meta.name,
                        col.name,
                        col.column_type.as_sql()
                    ),
                ));
            }
            return Err(DbError::new(format!(
                "PRIMARY KEY '{}' debe ser INT (esta versión sólo admite PK INT escalar; ver USER_MANUAL)",
                col.name
            )));
        }
        if col.default.is_some() {
            return Err(DbError::new(format!(
                "PRIMARY KEY '{}' no admite DEFAULT en esta versión",
                col.name
            )));
        }
        if is_composite && !col.not_null {
            return Err(coded(
                codes::COMPOSITE_PK_REQUIRES_ALL_INT,
                format!(
                    "PRIMARY KEY compuesta de '{}' rechazada: columna '{}' debe ser NOT NULL \
                     (todas las columnas de una PK compuesta deben serlo)",
                    meta.name, col.name
                ),
            ));
        }
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
