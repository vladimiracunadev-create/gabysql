use crate::bptree::{init_leaf_page, KeyValue, Tree};
use crate::catalog::{
    validate_create_table, validate_identifier, Catalog, Column, ColumnType, DefaultLiteral,
    ForeignKeyMeta, IndexKind, IndexMeta, OnDelete, TableMeta,
};
use crate::errors::{coded, codes};
use crate::index::{
    bucket_insert, bucket_lookup, bucket_remove, bucket_unique_conflict, decode_bucket,
    decode_ordered_bucket, encode_bucket, encode_column_value, encode_ordered_bucket, hash_value,
    ordered_bucket_insert, ordered_bucket_remove, ordered_bucket_unique_conflict,
    ordered_int_key_from_value_bytes, validate_indexable,
};
use crate::storage::Pager;
use crate::{DbError, DbResult};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    CreateTable(CreateTableStmt),
    DropTable(DropTableStmt),
    AlterTableAddColumn(AlterAddColumnStmt),
    Insert(InsertStmt),
    Select(SelectStmt),
    Update(UpdateStmt),
    Delete(DeleteStmt),
    CreateIndex(CreateIndexStmt),
    DropIndex(DropIndexStmt),
    /// Database-level statements. They do NOT operate on a single DB file
    /// but on the directory that hosts multiple DBs. The engine returns an
    /// explicit error if it sees them — they are meant to be intercepted
    /// by the caller (gabysql-server / CLI) BEFORE a Pager is opened.
    CreateDatabase(CreateDatabaseStmt),
    DropDatabase(DropDatabaseStmt),
    ShowDatabases,
    /// `INTEGRITY CHECK;` — sweeps the open DB and reports any
    /// detectable corruption: bad page CRCs, secondary-index entries
    /// that point at non-existent rows, FK values that lost their
    /// parent. Returns a result set with one row per finding plus a
    /// summary message.
    IntegrityCheck,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropTableStmt {
    pub name: String,
    pub if_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlterAddColumnStmt {
    pub table: String,
    pub column: ColumnDef,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateDatabaseStmt {
    pub name: String,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropDatabaseStmt {
    pub name: String,
    pub if_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateIndexStmt {
    pub name: String,
    pub table: String,
    pub column: String,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropIndexStmt {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStmt {
    pub table: String,
    pub assignments: Vec<(String, Value)>,
    pub where_column: String,
    pub where_pk: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteStmt {
    pub table: String,
    pub where_column: String,
    pub where_pk: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTableStmt {
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub primary_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub type_name: String,
    pub primary_key: bool,
    pub not_null: bool,
    pub unique: bool,
    pub default: Option<Value>,
    pub references: Option<ForeignKeyDef>,
}

/// Parser-level representation of `REFERENCES <table>(<column>) [ON
/// DELETE RESTRICT|CASCADE]`. Translated into a catalog
/// [`ForeignKeyMeta`] inside the executor (see `value_to_fk`).
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignKeyDef {
    pub table: String,
    pub column: String,
    pub on_delete: OnDelete,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertStmt {
    pub table: String,
    pub columns: Vec<String>,
    pub values: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectStmt {
    /// Base table del FROM. En SELECTs con JOIN sigue siendo "la primera"
    /// tabla declarada (las demás viven en `joins`). Se mantiene como
    /// `String` plano para no romper la API pública preexistente.
    pub table: String,
    /// Alias opcional de la base table (`FROM alumnos a`). Aplica también
    /// cuando hay JOINs — es la forma estándar de des-ambiguar columnas.
    pub table_alias: Option<String>,
    /// JOINs adicionales a la base. Vacío = SELECT single-table (todo el
    /// pipeline single-table sigue intacto). Cada join se aplica en orden
    /// (left-deep tree).
    pub joins: Vec<JoinClause>,
    pub columns: Vec<String>,
    pub where_clause: Option<WhereExpr>,
    pub order_by: Option<OrderClause>,
    pub limit: Option<usize>,
    pub offset: usize,
}

/// Tabla referenciada en el FROM (base o lado derecho de un JOIN).
#[derive(Debug, Clone, PartialEq)]
pub struct TableRef {
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JoinClause {
    pub kind: JoinKind,
    pub right: TableRef,
    /// `None` solo en `CROSS JOIN` (y en la comma-syntax, que se desazucara
    /// a CROSS). `INNER JOIN ... ON` siempre lleva predicado: si falta el
    /// parser devuelve `[GBY-4020]`.
    pub on: Option<JoinPredicate>,
    /// `JOIN ... USING (col)` — el engine deriva `ON l.col = r.col`. En
    /// este release soporta exactamente UNA columna en la lista.
    pub using: Option<Vec<String>>,
    /// `NATURAL JOIN` — el engine deriva un USING usando la columna con
    /// el mismo nombre presente en ambos lados (exactamente una en este
    /// release; 0 o >1 → `[GBY-4023]`).
    pub natural: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    Inner,
    Cross,
    /// `LEFT [OUTER] JOIN` — toda fila del lado izq se conserva, las
    /// columnas del right se llenan con NULL si no hay match.
    Left,
    /// `RIGHT [OUTER] JOIN` — toda fila del lado der se conserva, las
    /// columnas del left se llenan con NULL si no hay match.
    Right,
    /// `FULL [OUTER] JOIN` — unión de LEFT + RIGHT: filas sin match en
    /// cualquiera de los dos lados aparecen con NULLs en el otro.
    Full,
}

/// Predicado simple `t1.col = t2.col` para la cláusula `ON`. En este bloque
/// (foundation) soporta UN solo equi-predicado; `AND`/`OR` y comparadores
/// no-equi quedan para el bloque D.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinPredicate {
    pub left: ColumnRef,
    pub right: ColumnRef,
}

/// Referencia a una columna posiblemente cualificada (`tabla.col` o `col`).
/// El qualifier matchea contra el nombre real de la tabla o su alias
/// (case-insensitive).
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnRef {
    pub qualifier: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderClause {
    pub column: String,
    pub direction: OrderDir,
}

impl OrderClause {
    /// Devuelve el raw del ORDER BY tal cual lo escribió el user (puede
    /// venir cualificado `tabla.col` o bare `col`). El engine de JOIN lo
    /// resuelve contra el `JoinScope`.
    pub fn qualified_input(&self) -> DbResult<String> {
        Ok(self.column.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderDir {
    Asc,
    Desc,
}

/// Árbol booleano que envuelve los predicados atómicos del `WHERE`
/// (Bloque E1). Antes de E1 el `WHERE` era un único `WhereClause`; ahora
/// puede ser una combinación arbitraria con `AND`/`OR`/`NOT` y paréntesis.
///
/// `Atom` lleva el predicado existente sin tocar — eso preserva todas las
/// fast-paths (PK directo, índice secundario, range scan, EXISTS
/// correlacionado) cuando el `WHERE` se reduce a un único átomo.
///
/// Lógica trivaluada (3VL) de NULL:
/// - `NOT NULL = NULL`
/// - `NULL AND false = false`, `NULL AND true = NULL`, `NULL AND NULL = NULL`
/// - `NULL OR true = true`,  `NULL OR false = NULL`, `NULL OR NULL = NULL`
/// - una fila sobrevive el filtro solo si la expresión evalúa a `true`;
///   `NULL` (unknown) y `false` la descartan.
#[derive(Debug, Clone, PartialEq)]
pub enum WhereExpr {
    And(Box<WhereExpr>, Box<WhereExpr>),
    Or(Box<WhereExpr>, Box<WhereExpr>),
    Not(Box<WhereExpr>),
    Atom(WhereClause),
}

impl WhereExpr {
    /// Si la expresión es exactamente un átomo (sin AND/OR/NOT envolventes),
    /// devuelve referencia al `WhereClause` interno. Se usa en `exec_select`
    /// para decidir si aplica una fast-path optimizada (PK directa, índice,
    /// range scan) o si hay que caer al post-filter row-a-row.
    pub fn as_atom(&self) -> Option<&WhereClause> {
        match self {
            WhereExpr::Atom(c) => Some(c),
            _ => None,
        }
    }

    /// Versión owned de [`as_atom`]: si la expresión es un átomo, consume
    /// `self` y devuelve el `WhereClause`; en caso contrario devuelve la
    /// expresión sin modificar dentro de `Err` para que el caller pueda
    /// seguir usándola en el path general.
    pub fn into_atom(self) -> Result<WhereClause, WhereExpr> {
        match self {
            WhereExpr::Atom(c) => Ok(c),
            other => Err(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum WhereClause {
    Eq {
        column: String,
        value: Value,
    },
    Between {
        column: String,
        from: i64,
        to: i64,
    },
    In {
        column: String,
        subquery: Box<SelectStmt>,
    },
    EqSubquery {
        column: String,
        subquery: Box<SelectStmt>,
    },
    /// `WHERE inner_col = outer_table.outer_col` (o sin prefijo), válido SOLO
    /// dentro de subqueries correlacionadas. El engine lo resuelve mirando
    /// el `outer_stack`; fuera de una subquery devuelve `[GBY-4016]`.
    EqColumnRef {
        column: String,
        ref_table: Option<String>,
        ref_column: String,
    },
    /// `WHERE [NOT] EXISTS (SELECT ...)`. La subquery puede contener
    /// `EqColumnRef` (correlacionada) o no (no-correlacionada): el engine
    /// detecta el caso vía `subquery_has_outer_refs` y aplica pre-ejecución
    /// o post-filter per-row.
    Exists {
        subquery: Box<SelectStmt>,
        negated: bool,
    },
    // ───────── Bloque E2: operadores de comparación / nulidad / pertenencia ─────────
    //
    // Ninguno tiene fast-path por índice en este release: todos se evalúan
    // via FullScan + post-filter row-a-row con 3VL. Optimización indexada
    // (range scan para `<`/`>`/`<=`/`>=` sobre OrderedInt, hash lookup para
    // listas pequeñas, etc.) queda explícitamente fuera de E2.
    /// `col <op> literal` con `<op>` en `<, >, <=, >=, <>/!=`.
    /// Compatible con INT/FLOAT (orden numérico), TEXT (lexicográfico) y
    /// BOOL (false < true). NULL en cualquiera de los dos lados → `NULL`
    /// en el resultado (3VL).
    Compare {
        column: String,
        op: CompareOp,
        value: Value,
    },
    /// `col [NOT] LIKE 'patron'`. Wildcards estilo SQL estándar:
    /// `%` = cero o más caracteres, `_` = exactamente uno. Solo TEXT;
    /// otros tipos devuelven NULL (3VL). Escape con `\%` / `\_`.
    Like {
        column: String,
        pattern: String,
        negated: bool,
    },
    /// `col IS [NOT] NULL`. Único predicado que NO propaga NULL: `IS NULL`
    /// sobre NULL devuelve `true` (no `NULL`). Es la forma explícita de
    /// preguntar por ausencia.
    IsNull { column: String, negated: bool },
    /// `col [NOT] IN (lit1, lit2, ...)` con lista literal (no-subquery).
    /// Si la columna es NULL → NULL (3VL). NULLs dentro de la lista se
    /// ignoran (ANSI). `NOT IN` con un NULL en la lista propaga NULL
    /// (semántica ANSI estricta).
    InList {
        column: String,
        values: Vec<Value>,
        negated: bool,
    },
}

/// Operadores de comparación binarios soportados por [`WhereClause::Compare`]
/// (Bloque E2). `Eq` queda fuera porque tiene su propio variant con
/// fast-paths indexadas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
    Ne,
}

impl CompareOp {
    pub fn lexeme(&self) -> &'static str {
        match self {
            CompareOp::Lt => "<",
            CompareOp::Le => "<=",
            CompareOp::Gt => ">",
            CompareOp::Ge => ">=",
            CompareOp::Ne => "<>",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Float(f64),
    Bool(bool),
    String(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub message: Option<String>,
}

pub struct Engine<'a> {
    pager: &'a mut Pager,
    /// Stack de outer-rows activas para resolver `EqColumnRef` dentro de
    /// subqueries correlacionadas. Cada entrada lleva el nombre de la tabla
    /// outer y un mapa de columnas (normalizadas) a valores. Push antes de
    /// ejecutar la subquery correlacionada, pop después — siempre balanceado.
    outer_stack: Vec<OuterRow>,
}

#[derive(Debug, Clone)]
struct OuterRow {
    table: String,
    values: HashMap<String, Value>,
}

impl<'a> Engine<'a> {
    pub fn new(pager: &'a mut Pager) -> Self {
        Self {
            pager,
            outer_stack: Vec::new(),
        }
    }

    pub fn exec(&mut self, statement: Statement) -> DbResult<ResultSet> {
        match statement {
            Statement::CreateTable(stmt) => self.exec_create(stmt),
            Statement::DropTable(stmt) => self.exec_drop_table(stmt),
            Statement::AlterTableAddColumn(stmt) => self.exec_alter_add_column(stmt),
            Statement::Insert(stmt) => self.exec_insert(stmt),
            Statement::Select(stmt) => self.exec_select(stmt),
            Statement::Update(stmt) => self.exec_update(stmt),
            Statement::Delete(stmt) => self.exec_delete(stmt),
            Statement::CreateIndex(stmt) => self.exec_create_index(stmt),
            Statement::DropIndex(stmt) => self.exec_drop_index(stmt),
            Statement::IntegrityCheck => self.exec_integrity_check(),
            Statement::CreateDatabase(_)
            | Statement::DropDatabase(_)
            | Statement::ShowDatabases => Err(DbError::new(
                "CREATE/DROP/SHOW DATABASE no se ejecutan contra una DB; \
                 deben ser interceptados por el caller antes de abrir el Pager",
            )),
        }
    }

    fn exec_create(&mut self, stmt: CreateTableStmt) -> DbResult<ResultSet> {
        let mut columns = Vec::with_capacity(stmt.columns.len());
        let mut primary_key = stmt.primary_key.clone();
        // Remember which inline UNIQUE columns need an auto-created unique
        // index after the table is published. PK column is excluded — the
        // B+Tree already enforces PK uniqueness.
        let mut inline_unique_columns: Vec<String> = Vec::new();
        for column in stmt.columns {
            let column_type = ColumnType::from_sql(&column.type_name)?;
            let is_pk = column.primary_key;
            if is_pk {
                primary_key = column.name.clone();
            }
            // PK is implicitly NOT NULL; surface it in metadata so the
            // engine doesn't have to special-case it later.
            let not_null = column.not_null || is_pk;
            let default = column.default.as_ref().map(value_to_default);
            if column.unique && !is_pk {
                inline_unique_columns.push(column.name.clone());
            }
            let references = column.references.as_ref().map(fk_def_to_meta);
            columns.push(Column {
                name: column.name,
                column_type,
                not_null,
                default: match default {
                    Some(Ok(d)) => Some(d),
                    Some(Err(err)) => return Err(err),
                    None => None,
                },
                references,
            });
        }

        let mut meta = TableMeta {
            name: stmt.name,
            primary_key,
            columns,
            root_page: 0,
            indexes: Vec::new(),
        };
        validate_create_table(&meta)?;
        validate_fk_targets(self.pager, &meta)?;

        {
            let mut catalog = Catalog::open(self.pager);
            if catalog.get_table(&meta.name)?.is_some() {
                return Err(coded(
                    codes::TABLE_ALREADY_EXISTS,
                    format!(
                        "CREATE TABLE rechazado: ya existe una tabla llamada '{}'",
                        meta.name
                    ),
                ));
            }
        }

        let root_page = self.pager.new_page()?;
        let mut leaf_page = vec![0; self.pager.page_size()];
        init_leaf_page(&mut leaf_page);
        self.pager.write_page(root_page, &leaf_page, true)?;
        meta.root_page = root_page;

        // Materialize inline-UNIQUE columns as named unique indexes so the
        // rest of the engine can rely on a single uniqueness path.
        for col_name in &inline_unique_columns {
            let idx_root = self.pager.new_page()?;
            let mut leaf = vec![0; self.pager.page_size()];
            init_leaf_page(&mut leaf);
            self.pager.write_page(idx_root, &leaf, true)?;
            let col_type = meta
                .column(col_name)
                .map(|c| c.column_type)
                .ok_or_else(|| DbError::new("columna UNIQUE inline no presente en meta"))?;
            meta.indexes.push(IndexMeta {
                name: format!(
                    "uq_{}_{}",
                    meta.name.to_ascii_lowercase(),
                    col_name.to_ascii_lowercase()
                ),
                column: col_name.clone(),
                root_page: idx_root,
                unique: true,
                kind: IndexKind::for_column(col_type),
            });
        }

        let mut catalog = Catalog::open(self.pager);
        catalog.put_table(&meta)?;

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("OK".to_string()),
        })
    }

    /// `INTEGRITY CHECK;` — sweep every table, every secondary index and
    /// every FK in the open DB and report anything that doesn't add up.
    /// Each finding becomes one row in the returned ResultSet:
    ///
    ///     columns: ["kind", "object", "detail"]
    ///
    /// Possible `kind` values:
    ///
    ///   * `page_corrupt` — `page_data(no)` failed (bad CRC, etc.)
    ///   * `row_decode` — a row's bytes don't match the column schema
    ///     (rare; only happens if encoder/decoder go out of sync)
    ///   * `orphan_index_entry` — a secondary-index entry points at a
    ///     PK that isn't in the table anymore
    ///   * `fk_target_missing` — a column declares `REFERENCES` against
    ///     a table that no longer exists
    ///   * `fk_orphan` — a non-NULL FK value has no parent row
    ///
    /// The `message` field is `OK · …` when no findings, or
    /// `FAIL · N hallazgos · …` otherwise. Counts in the message are
    /// useful for monitoring even when callers ignore the row payload.
    fn exec_integrity_check(&mut self) -> DbResult<ResultSet> {
        let mut issues: Vec<(String, String, String)> = Vec::new();

        // Snapshot the catalog up front so child-table lookups during the
        // FK sweep don't fight with the table-by-table iteration.
        let tables = {
            let mut catalog = Catalog::open(self.pager);
            catalog.list_tables()?
        };

        // 1. Page-level CRC sweep. Loading every page through the pager
        //    triggers verify_page_checksum on each one — that's the whole
        //    check. We don't try to interpret garbage pages here; their
        //    CRC either matches what was last committed or it doesn't.
        let total_pages = self.pager.header().page_count;
        for page_no in 0..total_pages {
            if let Err(err) = self.pager.page_data(page_no) {
                issues.push((
                    "page_corrupt".into(),
                    format!("page {}", page_no),
                    err.to_string(),
                ));
            }
        }

        let mut rows_scanned = 0usize;
        let mut indexes_scanned = 0usize;
        let mut fks_checked = 0usize;

        for table in &tables {
            // 2. Walk every row. Collect the live PK set (used right
            //    after for the index sweep) and report any decode failure.
            let kvs = {
                let mut catalog = Catalog::open(self.pager);
                catalog.scan_rows(table.root_page, 0, None)?
            };
            let mut live_pks: HashSet<i64> = HashSet::with_capacity(kvs.len());
            for kv in &kvs {
                live_pks.insert(kv.key);
                if let Err(err) = decode_row(table, &kv.value) {
                    issues.push((
                        "row_decode".into(),
                        format!("{}.pk={}", table.name, kv.key),
                        err.to_string(),
                    ));
                }
            }
            rows_scanned += kvs.len();

            // 3. Walk every bucket of every secondary index and verify
            //    each (value_bytes, pk) or pk entry points at a real row.
            //    Hash and OrderedInt buckets have different layouts; the
            //    integrity sweep must decode according to `idx.kind`.
            for idx in &table.indexes {
                indexes_scanned += 1;
                let bucket_kvs = {
                    let mut tree = Tree::new(self.pager);
                    tree.all(idx.root_page)?
                };
                for kv in bucket_kvs {
                    let pks: Vec<i64> = match idx.kind {
                        IndexKind::Hash => decode_bucket(&kv.value)?
                            .into_iter()
                            .map(|(_, pk)| pk)
                            .collect(),
                        IndexKind::OrderedInt => decode_ordered_bucket(&kv.value)?,
                    };
                    for pk in pks {
                        if !live_pks.contains(&pk) {
                            issues.push((
                                "orphan_index_entry".into(),
                                format!("{}.{} pk={}", table.name, idx.name, pk),
                                "PK no existe en la tabla".into(),
                            ));
                        }
                    }
                }
            }

            // 4. FK sweep: for each column with a FK, walk the table's
            //    rows and confirm each non-NULL value still resolves
            //    against the parent.
            for column in &table.columns {
                let Some(fk) = &column.references else {
                    continue;
                };
                let parent = tables
                    .iter()
                    .find(|t| t.name.eq_ignore_ascii_case(&fk.table))
                    .cloned();
                let Some(parent_meta) = parent else {
                    issues.push((
                        "fk_target_missing".into(),
                        format!("{}.{}", table.name, column.name),
                        format!("tabla parent '{}' no existe", fk.table),
                    ));
                    continue;
                };
                for kv in &kvs {
                    let row = decode_row(table, &kv.value)?;
                    let Some(Value::Integer(parent_pk)) = row.get(&normalize_ident(&column.name))
                    else {
                        continue;
                    };
                    fks_checked += 1;
                    let exists = {
                        let mut catalog = Catalog::open(self.pager);
                        catalog
                            .get_row(parent_meta.root_page, *parent_pk)?
                            .is_some()
                    };
                    if !exists {
                        issues.push((
                            "fk_orphan".into(),
                            format!("{}.pk={}.{}={}", table.name, kv.key, column.name, parent_pk),
                            format!("no existe en '{}'", fk.table),
                        ));
                    }
                }
            }
        }

        let issue_count = issues.len();
        let summary = if issue_count == 0 {
            format!(
                "OK · {} tablas · {} filas · {} \u{00ed}ndices · {} FKs · {} p\u{00e1}ginas",
                tables.len(),
                rows_scanned,
                indexes_scanned,
                fks_checked,
                total_pages
            )
        } else {
            format!(
                "FAIL · {} hallazgos · {} tablas · {} filas · {} \u{00ed}ndices · {} FKs · {} p\u{00e1}ginas",
                issue_count,
                tables.len(),
                rows_scanned,
                indexes_scanned,
                fks_checked,
                total_pages
            )
        };

        let rows = issues
            .into_iter()
            .map(|(kind, object, detail)| {
                vec![
                    Value::String(kind),
                    Value::String(object),
                    Value::String(detail),
                ]
            })
            .collect();

        Ok(ResultSet {
            columns: vec!["kind".into(), "object".into(), "detail".into()],
            rows,
            message: Some(summary),
        })
    }

    fn exec_drop_table(&mut self, stmt: DropTableStmt) -> DbResult<ResultSet> {
        let mut catalog = Catalog::open(self.pager);
        let removed = catalog.remove_table(&stmt.name)?;
        if !removed && !stmt.if_exists {
            return Err(DbError::new(format!("tabla no existe: {}", stmt.name)));
        }
        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("OK".to_string()),
        })
    }

    fn exec_alter_add_column(&mut self, stmt: AlterAddColumnStmt) -> DbResult<ResultSet> {
        let mut meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .get_table(&stmt.table)?
                .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?
        };

        // Translate the parser-level ColumnDef into a catalog Column,
        // mirroring the logic in exec_create — but with two extra
        // restrictions specific to ALTER on a populated table:
        //
        //   1. PRIMARY KEY can never be added by ALTER (single PK already
        //      exists; this version doesn't support multi-PK or PK swap).
        //   2. NOT NULL without DEFAULT would leave existing rows
        //      violating the constraint immediately. Reject up front.
        //   3. UNIQUE with a non-NULL DEFAULT applied to more than one
        //      existing row would create a guaranteed duplicate. Reject.
        if stmt.column.primary_key {
            return Err(DbError::new(
                "ALTER TABLE ADD COLUMN no admite PRIMARY KEY (la PK ya existe)",
            ));
        }

        let column_type = ColumnType::from_sql(&stmt.column.type_name)?;
        let default = match stmt.column.default.as_ref().map(value_to_default) {
            Some(Ok(d)) => Some(d),
            Some(Err(err)) => return Err(err),
            None => None,
        };

        if stmt.column.not_null
            && !matches!(&default, Some(d) if !matches!(d, DefaultLiteral::Null))
        {
            return Err(DbError::new(format!(
                "ALTER TABLE ADD COLUMN '{}' NOT NULL requiere un DEFAULT no nulo \
                 (existirían filas previas en NULL)",
                stmt.column.name
            )));
        }

        // Forbid duplicate column name (case-insensitive, like the rest
        // of the engine).
        if meta.column(&stmt.column.name).is_some() {
            return Err(DbError::new(format!(
                "columna '{}' ya existe en la tabla '{}'",
                stmt.column.name, meta.name
            )));
        }

        let new_col = Column {
            name: stmt.column.name.clone(),
            column_type,
            not_null: stmt.column.not_null,
            default: default.clone(),
            references: stmt.column.references.as_ref().map(fk_def_to_meta),
        };
        // Run the standard validation against a *prospective* meta so the
        // same DEFAULT/type compatibility rules used by CREATE TABLE
        // apply here too.
        let mut prospective = meta.clone();
        prospective.columns.push(new_col.clone());
        validate_create_table(&prospective)?;
        validate_fk_targets(self.pager, &prospective)?;

        // Count existing rows once: needed for the UNIQUE/DEFAULT guard
        // and to decide whether the inline-UNIQUE backfill has to do work.
        let row_count = {
            let mut catalog = Catalog::open(self.pager);
            catalog.scan_rows(meta.root_page, 0, None)?.len()
        };

        if stmt.column.unique
            && row_count > 1
            && matches!(&default, Some(d) if !matches!(d, DefaultLiteral::Null))
        {
            return Err(DbError::new(format!(
                "ALTER TABLE ADD COLUMN '{}' UNIQUE con DEFAULT no nulo \
                 produciría duplicados en {} filas existentes",
                stmt.column.name, row_count
            )));
        }

        meta.columns.push(new_col);

        // If the new column is UNIQUE, materialize an index right away
        // and backfill it. Existing rows decode via the EOF-tolerant
        // path in decode_row → they yield the column's DEFAULT (or NULL).
        // The backfill helper already aborts on duplicates, which gives
        // us defense in depth on top of the row_count guard above.
        if stmt.column.unique {
            let idx_name = format!(
                "uq_{}_{}",
                meta.name.to_ascii_lowercase(),
                stmt.column.name.to_ascii_lowercase()
            );
            if meta.index_by_name(&idx_name).is_some() {
                return Err(DbError::new(format!(
                    "no se pudo crear índice UNIQUE auto-nombrado '{}': ya existe",
                    idx_name
                )));
            }
            let idx_root = self.pager.new_page()?;
            let mut leaf = vec![0; self.pager.page_size()];
            init_leaf_page(&mut leaf);
            self.pager.write_page(idx_root, &leaf, true)?;

            let column = meta.columns.last().unwrap().clone();
            let table_root = meta.root_page;
            let rows = {
                let mut catalog = Catalog::open(self.pager);
                catalog.scan_rows(table_root, 0, None)?
            };
            let mut seen_unique: std::collections::HashSet<Vec<u8>> =
                std::collections::HashSet::new();
            for kv in rows {
                let decoded = decode_row(&meta, &kv.value)?;
                let value = decoded
                    .get(&normalize_ident(&column.name))
                    .cloned()
                    .unwrap_or(Value::Null);
                let value_bytes = encode_column_value(&column, &value)?;
                if value_bytes != [0u8] && !seen_unique.insert(value_bytes.clone()) {
                    return Err(DbError::new(format!(
                        "ALTER ADD COLUMN UNIQUE rechazado: backfill de '{}' \
                         encontró duplicados",
                        column.name
                    )));
                }
                index_upsert_pk(
                    self.pager,
                    idx_root,
                    IndexKind::for_column(column_type),
                    &value_bytes,
                    kv.key,
                )?;
            }
            meta.indexes.push(IndexMeta {
                name: idx_name,
                column: stmt.column.name.clone(),
                root_page: idx_root,
                unique: true,
                kind: IndexKind::for_column(column_type),
            });
        }

        let mut catalog = Catalog::open(self.pager);
        catalog.put_table(&meta)?;

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("OK".to_string()),
        })
    }

    fn exec_insert(&mut self, stmt: InsertStmt) -> DbResult<ResultSet> {
        if stmt.columns.len() != stmt.values.len() {
            return Err(coded(
                codes::INSERT_COLS_VS_VALUES_MISMATCH,
                format!(
                    "INSERT INTO '{}': cantidad de columnas ({}) no coincide con cantidad de valores ({})",
                    stmt.table,
                    stmt.columns.len(),
                    stmt.values.len()
                ),
            ));
        }
        let meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_table(&stmt.table)?.ok_or_else(|| {
                coded(
                    codes::TABLE_NOT_FOUND,
                    format!("tabla no existe: {}", stmt.table),
                )
            })?
        };

        let mut seen = HashSet::new();
        let mut values = HashMap::new();
        for (column_name, value) in stmt.columns.into_iter().zip(stmt.values) {
            let normalized = normalize_ident(&column_name);
            if !seen.insert(normalized.clone()) {
                return Err(coded(
                    codes::DUPLICATE_COLUMN_NAME,
                    format!(
                        "INSERT INTO '{}': columna '{}' aparece más de una vez en la lista",
                        stmt.table, column_name
                    ),
                ));
            }
            if meta.column(&normalized).is_none() {
                return Err(coded(
                    codes::COLUMN_NOT_FOUND,
                    format!(
                        "columna '{}' no existe en tabla '{}'",
                        column_name, stmt.table
                    ),
                ));
            }
            values.insert(normalized, value);
        }

        // Apply DEFAULT for columns the user omitted, then enforce NOT NULL
        // on the final view of the row. We do this here (not inside
        // encode_row) so UPDATE keeps its own simpler "merge into existing"
        // semantics without re-running defaults.
        apply_defaults(&meta, &mut values);
        enforce_not_null_on_insert(&meta, &values)?;

        // UNIQUE pre-check: walk every unique secondary index and refuse
        // the INSERT before touching disk if a conflicting (value, pk)
        // already lives in the bucket. This is cheap (one B+Tree get per
        // unique index) and keeps the failure path side-effect free.
        for idx in &meta.indexes {
            if !idx.unique {
                continue;
            }
            let column = meta.column(&idx.column).ok_or_else(|| {
                DbError::new(format!(
                    "índice apunta a columna inexistente: {}",
                    idx.column
                ))
            })?;
            let value = values
                .get(&normalize_ident(&column.name))
                .cloned()
                .unwrap_or(Value::Null);
            let value_bytes = encode_column_value(column, &value)?;
            check_unique_conflict(self.pager, idx, &value_bytes, None)?;
        }

        let (pk, row_bytes) = encode_row(&meta, &values)?;
        // FK pre-check uses the *new* PK so a self-referencing INSERT
        // pointing at itself succeeds (the row will exist after this
        // statement commits).
        enforce_fk_on_insert(self.pager, &meta, &values, pk)?;
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.insert_row(meta.root_page, pk, row_bytes)?;
        }

        // Maintain every secondary index: hash the new column value and
        // upsert (value_bytes, pk) into the index bucket.
        for idx in &meta.indexes {
            let column = meta.column(&idx.column).ok_or_else(|| {
                DbError::new(format!(
                    "índice apunta a columna inexistente: {}",
                    idx.column
                ))
            })?;
            let value = values
                .get(&normalize_ident(&column.name))
                .cloned()
                .unwrap_or(Value::Null);
            let value_bytes = encode_column_value(column, &value)?;
            index_upsert_pk(self.pager, idx.root_page, idx.kind, &value_bytes, pk)?;
        }

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("OK".to_string()),
        })
    }

    /// Look up `ref_table.ref_column` against the current `outer_stack`.
    /// With an explicit `ref_table`, returns the value from the most-recent
    /// frame whose table name matches (case-insensitive). Without one,
    /// returns the value from the top frame. Returns `[GBY-4016]` when no
    /// frame matches (e.g. EqColumnRef used outside a correlated subquery)
    /// or when the frame doesn't carry the requested column.
    fn resolve_outer_ref(&self, ref_table: Option<&str>, ref_column: &str) -> DbResult<Value> {
        let key = normalize_ident(ref_column);
        let frame = self
            .outer_stack
            .iter()
            .rev()
            .find(|frame| match ref_table {
                Some(t) => frame.table.eq_ignore_ascii_case(t),
                None => true,
            })
            .ok_or_else(|| {
                coded(
                    codes::OUTER_COLUMN_REF_INVALID,
                    match ref_table {
                        Some(t) => format!(
                            "outer column '{}.{}' fuera de alcance: o la tabla outer '{}' \
                             no está activa, o la referencia se usó fuera de una subquery \
                             correlacionada",
                            t, ref_column, t
                        ),
                        None => format!(
                            "columna '{}' referenciada sin tabla y sin outer-scope activo; \
                             dentro de un WHERE outer, los RHS deben ser literales — solo \
                             las subqueries correlacionadas pueden referenciar columnas",
                            ref_column
                        ),
                    },
                )
            })?;
        frame.values.get(&key).cloned().ok_or_else(|| {
            coded(
                codes::OUTER_COLUMN_REF_INVALID,
                format!(
                    "outer column '{}' no existe en la tabla outer '{}'",
                    ref_column, frame.table
                ),
            )
        })
    }

    fn exec_select(&mut self, stmt: SelectStmt) -> DbResult<ResultSet> {
        // SELECT con JOINs sigue una ruta distinta (nested-loop, schema
        // combinado, WHERE como post-filter). El single-table path queda
        // exactamente como estaba — sin regresión en performance ni
        // semántica para queries que no usan JOIN.
        if !stmt.joins.is_empty() {
            return self.exec_select_joined(stmt);
        }
        let meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .get_table(&stmt.table)?
                .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?
        };

        let selected_columns = resolve_selected_columns(&meta, &stmt.columns)?;
        let output_columns: Vec<String> = selected_columns
            .iter()
            .map(|(name, _)| name.clone())
            .collect();

        // Si el WHERE es un EXISTS correlacionado, no podemos pre-ejecutar la
        // subquery: hay que evaluarla por cada fila del outer con la fila
        // actual empujada en `outer_stack`. El resto del flujo (plan, scan,
        // ORDER BY, LIMIT/OFFSET) se mantiene; el filtro se aplica entre la
        // materialización de `rows_bytes` y la proyección.
        //
        // Bloque E1: el fast-path solo aplica cuando el WHERE es UN átomo
        // `Exists` correlacionado (sin AND/OR/NOT envolventes). Cuando hay
        // combinadores el filtrado entero se hace por la rama
        // `generic_post_filter` con el evaluador 3VL.
        let exists_postfilter: Option<(Box<SelectStmt>, bool)> = match stmt
            .where_clause
            .as_ref()
            .and_then(|e| e.as_atom())
        {
            Some(WhereClause::Exists { subquery, negated }) if subquery_has_outer_refs(subquery) => {
                Some((subquery.clone(), *negated))
            }
            _ => None,
        };

        // Bloque E1+E2: el path por fast-path indexada solo aplica cuando
        // el WHERE se reduce a un único átomo CON fast-path (los 6 pre-E2:
        // Eq, Between, In subquery, EqSubquery, EqColumnRef, Exists). En
        // cualquier otro caso (combinadores AND/OR/NOT, o átomos E2 que
        // por ahora no tienen optimización indexada) caemos a FullScan +
        // post-filter genérico 3VL.
        let generic_post_filter: Option<WhereExpr> = match &stmt.where_clause {
            Some(expr) => {
                let force = match expr.as_atom() {
                    None => true,
                    Some(atom) => matches!(
                        atom,
                        WhereClause::Compare { .. }
                            | WhereClause::Like { .. }
                            | WhereClause::IsNull { .. }
                            | WhereClause::InList { .. }
                    ),
                };
                if force {
                    Some(expr.clone())
                } else {
                    None
                }
            }
            None => None,
        };

        // First decide what list of PKs we need (or whether we need a full
        // scan), without holding the Catalog borrow. Then we open Catalog
        // again to actually read each row's bytes. This keeps the borrow
        // checker happy when the index path needs &mut self.pager.
        enum Plan {
            FullScan,
            ByPks(Vec<i64>),
            Range { from: i64, to: i64 },
        }

        // Extracto el átomo único del WHERE (si existe) para reusar el
        // dispatch original. Cuando hay combinadores `generic_post_filter`
        // se hace cargo y acá entramos por la rama `None`.
        let where_atom: Option<WhereClause> = stmt
            .where_clause
            .clone()
            .and_then(|e| e.into_atom().ok());

        let plan = if exists_postfilter.is_some() || generic_post_filter.is_some() {
            // El filtrado real ocurre en el post-filter; el scan barre todo.
            Plan::FullScan
        } else {
            match where_atom {
                None => Plan::FullScan,
                Some(WhereClause::Eq { column, value }) => {
                    let normalized = normalize_ident(&column);
                    if normalized == normalize_ident(&meta.primary_key) {
                        let pk = match value {
                            Value::Integer(n) => n,
                            _ => {
                                return Err(DbError::new(format!(
                                    "PRIMARY KEY '{}' es INT; valor incompatible en WHERE",
                                    meta.primary_key
                                )))
                            }
                        };
                        Plan::ByPks(vec![pk])
                    } else if let Some(idx) = meta.index_for_column(&normalized).cloned() {
                        let pks = lookup_pks_via_index(self.pager, &meta, &idx, &value)?;
                        Plan::ByPks(pks)
                    } else {
                        return Err(coded(
                            codes::WHERE_OPERATOR_UNSUPPORTED,
                            format!(
                                "WHERE solo soporta PK ({}) o columnas con índice secundario; \
                             '{}' no está indexada",
                                meta.primary_key, column
                            ),
                        ));
                    }
                }
                Some(WhereClause::Between { column, from, to }) => {
                    let normalized = normalize_ident(&column);
                    if normalized == normalize_ident(&meta.primary_key) {
                        Plan::Range { from, to }
                    } else if let Some(idx) = meta.index_for_column(&normalized).cloned() {
                        // BETWEEN over an indexed column only works when the
                        // index is INT-ordered (ADR-0017). Hash indexes are
                        // equality-only by construction.
                        match idx.kind {
                            IndexKind::OrderedInt => {
                                let pks = lookup_pks_via_index_range(self.pager, &idx, from, to)?;
                                Plan::ByPks(pks)
                            }
                            IndexKind::Hash => {
                                return Err(coded(
                                    codes::BETWEEN_REQUIRES_PK_OR_INT_INDEX,
                                    format!(
                                    "WHERE BETWEEN sobre '{}': el índice secundario es hash-based \
                                     (equality only). Solo columnas INT-indexadas admiten BETWEEN.",
                                    column
                                ),
                                ));
                            }
                        }
                    } else {
                        return Err(coded(
                            codes::BETWEEN_REQUIRES_PK_OR_INT_INDEX,
                            format!(
                                "WHERE BETWEEN solo soporta PK ({}) o columnas INT con índice; \
                             '{}' no califica",
                                meta.primary_key, column
                            ),
                        ));
                    }
                }
                Some(WhereClause::In { column, subquery }) => {
                    // Non-correlated IN: execute the subquery once, materialize
                    // its single-column result, then turn each value into a PK
                    // (direct or via secondary index lookup). The Plan stays
                    // `ByPks`, so the existing row-fetch path handles ORDER BY,
                    // LIMIT and OFFSET without changes.
                    let inner = self.exec_select(*subquery)?;
                    if inner.columns.len() != 1 {
                        return Err(coded(
                            codes::SUBQUERY_MUST_RETURN_ONE_COLUMN,
                            format!(
                                "subquery en IN debe devolver exactamente 1 columna; devolvió {}",
                                inner.columns.len()
                            ),
                        ));
                    }
                    let values: Vec<Value> = inner
                        .rows
                        .into_iter()
                        .filter_map(|mut row| row.pop())
                        .filter(|v| !matches!(v, Value::Null))
                        .collect();
                    if values.is_empty() {
                        Plan::ByPks(Vec::new())
                    } else {
                        let normalized = normalize_ident(&column);
                        if normalized == normalize_ident(&meta.primary_key) {
                            let mut pks = Vec::with_capacity(values.len());
                            for v in values {
                                match v {
                                    Value::Integer(n) => pks.push(n),
                                    _ => {
                                        return Err(coded(
                                            codes::IN_PK_TYPE_MISMATCH,
                                            format!(
                                                "PRIMARY KEY '{}' es INT; valor incompatible en IN",
                                                meta.primary_key
                                            ),
                                        ))
                                    }
                                }
                            }
                            pks.sort_unstable();
                            pks.dedup();
                            Plan::ByPks(pks)
                        } else if let Some(idx) = meta.index_for_column(&normalized).cloned() {
                            let mut pks: Vec<i64> = Vec::new();
                            for v in values {
                                let mut more = lookup_pks_via_index(self.pager, &meta, &idx, &v)?;
                                pks.append(&mut more);
                            }
                            pks.sort_unstable();
                            pks.dedup();
                            Plan::ByPks(pks)
                        } else {
                            return Err(coded(
                                codes::IN_REQUIRES_PK_OR_INDEX,
                                format!(
                                "WHERE IN solo soporta PK ({}) o columnas con índice secundario; \
                                 '{}' no está indexada",
                                meta.primary_key, column
                            ),
                            ));
                        }
                    }
                }
                Some(WhereClause::EqSubquery { column, subquery }) => {
                    // Subquery escalar: la subquery debe devolver 1 columna y a
                    // lo sumo 1 fila. 0 filas o 1 fila NULL → set vacío (semántica
                    // ANSI: comparar contra NULL nunca matchea). 1 fila con valor
                    // se reusa por la rama Eq existente (PK directa o índice).
                    let inner = self.exec_select(*subquery)?;
                    if inner.columns.len() != 1 {
                        return Err(coded(
                            codes::SUBQUERY_MUST_RETURN_ONE_COLUMN,
                            format!(
                                "subquery escalar debe devolver exactamente 1 columna; devolvió {}",
                                inner.columns.len()
                            ),
                        ));
                    }
                    if inner.rows.len() > 1 {
                        return Err(coded(
                        codes::SCALAR_SUBQUERY_TOO_MANY_ROWS,
                        format!(
                            "subquery escalar en WHERE devolvió {} filas; debe devolver a lo sumo 1",
                            inner.rows.len()
                        ),
                    ));
                    }
                    let scalar = inner.rows.into_iter().next().and_then(|mut r| r.pop());
                    match scalar {
                        None | Some(Value::Null) => Plan::ByPks(Vec::new()),
                        Some(value) => {
                            let normalized = normalize_ident(&column);
                            if normalized == normalize_ident(&meta.primary_key) {
                                let pk = match value {
                                    Value::Integer(n) => n,
                                    _ => {
                                        return Err(coded(
                                            codes::IN_PK_TYPE_MISMATCH,
                                            format!(
                                                "PRIMARY KEY '{}' es INT; valor incompatible \
                                             devuelto por la subquery escalar",
                                                meta.primary_key
                                            ),
                                        ))
                                    }
                                };
                                Plan::ByPks(vec![pk])
                            } else if let Some(idx) = meta.index_for_column(&normalized).cloned() {
                                let pks = lookup_pks_via_index(self.pager, &meta, &idx, &value)?;
                                Plan::ByPks(pks)
                            } else {
                                return Err(coded(
                                    codes::IN_REQUIRES_PK_OR_INDEX,
                                    format!(
                                        "WHERE = (SELECT ...) solo soporta PK ({}) o columnas \
                                     con índice secundario; '{}' no está indexada",
                                        meta.primary_key, column
                                    ),
                                ));
                            }
                        }
                    }
                }
                Some(WhereClause::EqColumnRef {
                    column,
                    ref_table,
                    ref_column,
                }) => {
                    // `WHERE inner_col = outer_table.col` resuelto contra el
                    // outer_stack. Si el stack está vacío (uso fuera de una
                    // subquery correlacionada) o la columna outer no existe,
                    // resolve_outer_ref devuelve `[GBY-4016]`. Cuando hay valor
                    // se reusa el dispatch PK/índice del Eq.
                    let value = self.resolve_outer_ref(ref_table.as_deref(), &ref_column)?;
                    let normalized = normalize_ident(&column);
                    if normalized == normalize_ident(&meta.primary_key) {
                        let pk = match value {
                            Value::Integer(n) => n,
                            _ => {
                                return Err(coded(
                                    codes::IN_PK_TYPE_MISMATCH,
                                    format!(
                                        "PRIMARY KEY '{}' es INT; valor incompatible \
                                     devuelto por la outer column referenciada",
                                        meta.primary_key
                                    ),
                                ))
                            }
                        };
                        Plan::ByPks(vec![pk])
                    } else if let Some(idx) = meta.index_for_column(&normalized).cloned() {
                        let pks = lookup_pks_via_index(self.pager, &meta, &idx, &value)?;
                        Plan::ByPks(pks)
                    } else {
                        return Err(coded(
                            codes::IN_REQUIRES_PK_OR_INDEX,
                            format!(
                                "WHERE col = outer.col solo soporta PK ({}) o columnas \
                             con índice secundario; '{}' no está indexada",
                                meta.primary_key, column
                            ),
                        ));
                    }
                }
                Some(WhereClause::Exists { subquery, negated }) => {
                    // Llegamos acá SOLO si `subquery_has_outer_refs(...)` fue
                    // false (las correlacionadas se desvían arriba al
                    // post-filter). Pre-ejecutamos: si hay filas → outer queda
                    // como FullScan; si no → outer queda vacío. `negated`
                    // invierte la decisión.
                    let inner = self.exec_select(*subquery)?;
                    let has_rows = !inner.rows.is_empty();
                    let pass = if negated { !has_rows } else { has_rows };
                    if pass {
                        Plan::FullScan
                    } else {
                        Plan::ByPks(Vec::new())
                    }
                }
                // Átomos E2 (`Compare`, `Like`, `IsNull`, `InList`) nunca
                // alcanzan este match porque `generic_post_filter` los
                // intercepta arriba. Mantenemos un brazo defensivo para
                // que el match siga exhaustivo si alguien agrega una
                // fast-path indexada futura sin tocar este lugar.
                Some(WhereClause::Compare { .. })
                | Some(WhereClause::Like { .. })
                | Some(WhereClause::IsNull { .. })
                | Some(WhereClause::InList { .. }) => Plan::FullScan,
            }
        };

        // ORDER BY validation up front (before any I/O).
        if let Some(ord) = &stmt.order_by {
            if meta.column(&ord.column).is_none() {
                return Err(DbError::new(format!(
                    "ORDER BY: columna '{}' no existe en '{}'",
                    ord.column, meta.name
                )));
            }
        }

        // When ORDER BY is set we must materialize *every* matching row
        // before applying offset/limit — otherwise we'd window over an
        // arbitrary B+Tree-key order and miss the requested ordering.
        // Without ORDER BY we walk the leaves lazily through a
        // `LeafCursor` and let `Iterator::skip().take()` short-circuit
        // the scan as soon as `LIMIT` is satisfied. That turns
        // `SELECT … LIMIT 10` over a million-row table from a full
        // materialization into an O(offset + limit) leaf walk.
        // El post-filter de EXISTS correlacionado se aplica sobre la lista
        // completa de filas, así que también necesita diferir el window —
        // de lo contrario `LIMIT 10` cortaría antes de aplicar EXISTS y
        // devolvería menos filas de las que en realidad matchean.
        let defer_window =
            stmt.order_by.is_some() || exists_postfilter.is_some() || generic_post_filter.is_some();
        let rows_bytes: Vec<KeyValue> = if defer_window {
            let mut catalog = Catalog::open(self.pager);
            match plan {
                Plan::FullScan => catalog.scan_rows(meta.root_page, 0, None)?,
                Plan::Range { from, to } => catalog.range_rows(meta.root_page, from, to)?,
                Plan::ByPks(pks) => {
                    let mut rows = Vec::with_capacity(pks.len());
                    for pk in pks {
                        if let Some(bytes) = catalog.get_row(meta.root_page, pk)? {
                            rows.push(KeyValue {
                                key: pk,
                                value: bytes,
                            });
                        }
                    }
                    rows.sort_by_key(|kv| kv.key);
                    rows
                }
            }
        } else {
            // `take(usize::MAX)` is the closed-form for "no limit"; the
            // standard library short-circuits when the inner iterator
            // returns None, so it never actually reaches usize::MAX.
            let take = stmt.limit.unwrap_or(usize::MAX);
            match plan {
                Plan::FullScan => {
                    let catalog = Catalog::open(self.pager);
                    catalog
                        .scan_cursor(meta.root_page)?
                        .skip(stmt.offset)
                        .take(take)
                        .collect::<DbResult<Vec<_>>>()?
                }
                Plan::Range { from, to } => {
                    let catalog = Catalog::open(self.pager);
                    catalog
                        .range_cursor(meta.root_page, from, to)?
                        .skip(stmt.offset)
                        .take(take)
                        .collect::<DbResult<Vec<_>>>()?
                }
                Plan::ByPks(pks) => {
                    // Bounded by the index lookup that produced `pks`,
                    // so materialization here is O(|pks|), not O(table).
                    let mut catalog = Catalog::open(self.pager);
                    let mut rows = Vec::with_capacity(pks.len());
                    for pk in pks {
                        if let Some(bytes) = catalog.get_row(meta.root_page, pk)? {
                            rows.push(KeyValue {
                                key: pk,
                                value: bytes,
                            });
                        }
                    }
                    rows.sort_by_key(|kv| kv.key);
                    window_rows(rows, stmt.offset, stmt.limit)
                }
            }
        };

        // EXISTS correlacionado: re-evaluamos la subquery con cada fila del
        // outer empujada en `outer_stack`. La fila sólo sobrevive si la
        // condición EXISTS (o NOT EXISTS, según `negated`) se cumple. Si no
        // hay ORDER BY, aplicamos el window acá mismo — el LeafCursor ya no
        // pudo cortar porque forzamos `defer_window` arriba.
        let rows_bytes: Vec<KeyValue> = if let Some((sub_stmt, negated)) = exists_postfilter {
            let mut kept = Vec::with_capacity(rows_bytes.len());
            for kv in rows_bytes {
                let decoded = decode_row(&meta, &kv.value)?;
                self.outer_stack.push(OuterRow {
                    table: meta.name.clone(),
                    values: decoded,
                });
                let inner_res = self.exec_select((*sub_stmt).clone());
                self.outer_stack.pop();
                let inner = inner_res?;
                let has_rows = !inner.rows.is_empty();
                let pass = if negated { !has_rows } else { has_rows };
                if pass {
                    kept.push(kv);
                }
            }
            if stmt.order_by.is_none() {
                window_rows(kept, stmt.offset, stmt.limit)
            } else {
                kept
            }
        } else {
            rows_bytes
        };

        // Bloque E1: post-filter genérico cuando el WHERE no es un único
        // átomo (AND/OR/NOT/paréntesis). Recorremos row-a-row decodificando
        // y evaluando con 3VL. La fila sobrevive solo si la expresión
        // evalúa a `Some(true)` (NULL/unknown descarta, como en ANSI SQL).
        let rows_bytes: Vec<KeyValue> = if let Some(expr) = generic_post_filter {
            let mut kept = Vec::with_capacity(rows_bytes.len());
            for kv in rows_bytes {
                let decoded = decode_row(&meta, &kv.value)?;
                let verdict = self.eval_where_expr_single(&expr, &meta, &decoded)?;
                if matches!(verdict, Some(true)) {
                    kept.push(kv);
                }
            }
            if stmt.order_by.is_none() {
                window_rows(kept, stmt.offset, stmt.limit)
            } else {
                kept
            }
        } else {
            rows_bytes
        };

        let mut rows: Vec<(HashMap<String, Value>, Vec<Value>)> =
            Vec::with_capacity(rows_bytes.len());
        for kv in rows_bytes {
            let decoded = decode_row(&meta, &kv.value)?;
            let projected = project_row(&selected_columns, &decoded)?;
            rows.push((decoded, projected));
        }

        if let Some(ord) = &stmt.order_by {
            let key = normalize_ident(&ord.column);
            rows.sort_by(|a, b| compare_values(a.0.get(&key), b.0.get(&key)));
            if matches!(ord.direction, OrderDir::Desc) {
                rows.reverse();
            }
            // Window is applied after the sort.
            let total = rows.len();
            let start = stmt.offset.min(total);
            let end = match stmt.limit {
                Some(l) => (start + l).min(total),
                None => total,
            };
            rows = rows.into_iter().skip(start).take(end - start).collect();
        }
        let rows: Vec<Vec<Value>> = rows.into_iter().map(|(_, r)| r).collect();

        Ok(ResultSet {
            columns: output_columns,
            rows,
            message: None,
        })
    }

    /// Ejecuta un SELECT con JOINs vía nested-loop sobre filas materializadas.
    ///
    /// Estrategia (deliberadamente simple, sin optimizer):
    ///   1. Cargar el metadata de cada tabla del FROM (base + cada JOIN).
    ///   2. Para cada tabla armar un FullScan a `HashMap<String, Value>` con
    ///      claves cualificadas (`alias.col` o `tabla.col`).
    ///   3. Empezar con las filas de la base y, para cada JOIN, hacer
    ///      cross-product y evaluar el `ON` — left-deep, en el orden en que
    ///      aparecen los JOIN.
    ///   4. Aplicar el `WHERE` como post-filter (Eq, Between, In sobre las
    ///      joined-rows; los predicados con subqueries siguen funcionando
    ///      porque la subquery se ejecuta una vez antes del scan).
    ///   5. Ordenar (`ORDER BY`), proyectar, aplicar `OFFSET`/`LIMIT`.
    ///
    /// Complejidad: O(N1 × N2 × … × Nk) en el peor caso (nested loop puro).
    /// El bloque D del roadmap agregará index-loop join: cuando el `ON`
    /// pega contra una columna indexada del lado derecho, reemplazar el
    /// FullScan del right por un index lookup.
    fn exec_select_joined(&mut self, stmt: SelectStmt) -> DbResult<ResultSet> {
        // --- 1. Construir el scope (lista de tablas con sus aliases) ---
        let mut scope = self.build_join_scope(&stmt)?;

        // --- 2. Materializar la base table como joined-rows ---
        let base = &scope.tables[0];
        let mut current: Vec<HashMap<String, Value>> = self.scan_qualified(base)?;

        // --- 3. Aplicar cada JOIN en orden left-deep ---
        //
        // Para OUTER joins (LEFT/RIGHT/FULL) trackeamos qué filas
        // matchearon de cada lado y luego rellenamos las que quedaron
        // solas con NULL en las columnas del lado vacío. INNER/CROSS no
        // rellenan: las no-matched simplemente se descartan.
        for (i, join) in stmt.joins.iter().enumerate() {
            // Derivar el predicate efectivo: explícito (ON), USING o NATURAL.
            // El derive también puede marcar columnas como `hidden_in_star`
            // — necesario para SELECT * sin duplicar la columna común.
            let derived = derive_join_predicate(&scope, i, join)?;
            for hidden in &derived.hidden_keys {
                scope.hidden_in_star.insert(hidden.clone());
            }
            let effective_on: Option<&JoinPredicate> =
                derived.predicate.as_ref().or(join.on.as_ref());

            // --- Index-loop fast path (bloque D del roadmap) ---
            // Cuando el JOIN es INNER o LEFT y el predicate apunta contra
            // la PK o una columna indexada del right, evitamos materializar
            // todo el right table: por cada left_row hacemos un lookup
            // dirigido. Complejidad: O(N1 × log N2) vs O(N1 × N2).
            //
            // RIGHT/FULL no se optimizan porque necesitarían además saber
            // qué filas del right NO matchearon — eso requiere un scan
            // paralelo y no aporta vs el nested-loop actual.
            if matches!(join.kind, JoinKind::Inner | JoinKind::Left) {
                if let Some(pred) = effective_on {
                    if let Some(plan) = plan_index_loop(&scope, i, pred)? {
                        current = self.run_index_loop_join(current, i, join.kind, &plan, &scope)?;
                        continue;
                    }
                }
            }

            // --- Fallback: nested-loop puro ---
            let right = &scope.tables[i + 1];
            let right_rows = self.scan_qualified(right)?;
            let mut next: Vec<HashMap<String, Value>> =
                Vec::with_capacity(current.len() * right_rows.len() / 2 + 1);
            let mut left_matched = vec![false; current.len()];
            let mut right_matched = vec![false; right_rows.len()];
            for (li, left_row) in current.iter().enumerate() {
                for (ri, right_row) in right_rows.iter().enumerate() {
                    let pass = match effective_on {
                        None => true, // CROSS JOIN o comma-syntax
                        Some(pred) => evaluate_join_predicate(left_row, right_row, pred, &scope)?,
                    };
                    if !pass {
                        continue;
                    }
                    left_matched[li] = true;
                    right_matched[ri] = true;
                    // Merge: las claves nunca chocan porque van prefijadas
                    // con alias/tabla únicos (validados arriba).
                    let mut merged = HashMap::with_capacity(left_row.len() + right_row.len());
                    for (k, v) in left_row {
                        merged.insert(k.clone(), v.clone());
                    }
                    for (k, v) in right_row {
                        merged.insert(k.clone(), v.clone());
                    }
                    next.push(merged);
                }
            }
            let needs_left_fill = matches!(join.kind, JoinKind::Left | JoinKind::Full);
            let needs_right_fill = matches!(join.kind, JoinKind::Right | JoinKind::Full);
            if needs_left_fill {
                let right_null_keys: Vec<String> = scope.tables[i + 1]
                    .meta
                    .columns
                    .iter()
                    .map(|c| {
                        format!(
                            "{}.{}",
                            scope.tables[i + 1].qualifier,
                            normalize_ident(&c.name)
                        )
                    })
                    .collect();
                for (li, left_row) in current.iter().enumerate() {
                    if left_matched[li] {
                        continue;
                    }
                    let mut filled = left_row.clone();
                    for k in &right_null_keys {
                        filled.insert(k.clone(), Value::Null);
                    }
                    next.push(filled);
                }
            }
            if needs_right_fill {
                let left_null_keys: Vec<String> = scope.tables[..=i]
                    .iter()
                    .flat_map(|t| {
                        t.meta
                            .columns
                            .iter()
                            .map(move |c| format!("{}.{}", t.qualifier, normalize_ident(&c.name)))
                    })
                    .collect();
                for (ri, right_row) in right_rows.iter().enumerate() {
                    if right_matched[ri] {
                        continue;
                    }
                    let mut filled: HashMap<String, Value> =
                        HashMap::with_capacity(left_null_keys.len() + right_row.len());
                    for k in &left_null_keys {
                        filled.insert(k.clone(), Value::Null);
                    }
                    for (k, v) in right_row {
                        filled.insert(k.clone(), v.clone());
                    }
                    next.push(filled);
                }
            }
            current = next;
        }

        // --- 4. WHERE como post-filter ---
        if let Some(where_clause) = stmt.where_clause.clone() {
            current = self.filter_joined_rows_expr(current, &where_clause, &scope)?;
        }

        // --- 5. Resolver columnas proyectadas (output_columns = lo que escribió el user) ---
        let (output_columns, projected_keys) = resolve_joined_projection(&scope, &stmt.columns)?;

        // --- 6. ORDER BY sobre la fila joined ---
        if let Some(ord) = &stmt.order_by {
            let key = resolve_joined_column_key(&scope, &ord.qualified_input()?)?;
            current.sort_by(|a, b| compare_values(a.get(&key), b.get(&key)));
            if matches!(ord.direction, OrderDir::Desc) {
                current.reverse();
            }
        }

        // --- 7. Proyectar + OFFSET/LIMIT ---
        let take = stmt.limit.unwrap_or(usize::MAX);
        let rows: Vec<Vec<Value>> = current
            .into_iter()
            .skip(stmt.offset)
            .take(take)
            .map(|row| {
                projected_keys
                    .iter()
                    .map(|k| row.get(k).cloned().unwrap_or(Value::Null))
                    .collect()
            })
            .collect();

        Ok(ResultSet {
            columns: output_columns,
            rows,
            message: None,
        })
    }

    /// FullScan de una tabla devolviendo HashMaps con claves `alias.col`
    /// (lower-case). Se usa como entrada al nested-loop join.
    /// Ejecuta el JOIN con index-loop: por cada fila del current (lado
    /// "left"), saca el valor del predicate y hace lookup directo en el
    /// right (vía PK o índice secundario). Mucho más barato que materializar
    /// el right entero cuando el right es grande.
    fn run_index_loop_join(
        &mut self,
        current: Vec<HashMap<String, Value>>,
        join_idx: usize,
        kind: JoinKind,
        plan: &IndexLoopPlan,
        scope: &JoinScope,
    ) -> DbResult<Vec<HashMap<String, Value>>> {
        let right = &scope.tables[join_idx + 1];
        let needs_left_fill = matches!(kind, JoinKind::Left);
        let right_null_keys: Vec<String> = if needs_left_fill {
            right
                .meta
                .columns
                .iter()
                .map(|c| format!("{}.{}", right.qualifier, normalize_ident(&c.name)))
                .collect()
        } else {
            Vec::new()
        };
        let mut next: Vec<HashMap<String, Value>> = Vec::with_capacity(current.len());
        for left_row in current.iter() {
            let left_value = match left_row.get(&plan.left_key).cloned() {
                Some(v) => v,
                None => Value::Null,
            };
            // NULL del left nunca matchea (semántica SQL standard).
            if matches!(left_value, Value::Null) {
                if needs_left_fill {
                    let mut filled = left_row.clone();
                    for k in &right_null_keys {
                        filled.insert(k.clone(), Value::Null);
                    }
                    next.push(filled);
                }
                continue;
            }
            let matched = self.lookup_right_rows(right, plan, &left_value)?;
            if matched.is_empty() {
                if needs_left_fill {
                    let mut filled = left_row.clone();
                    for k in &right_null_keys {
                        filled.insert(k.clone(), Value::Null);
                    }
                    next.push(filled);
                }
                continue;
            }
            for right_row in matched {
                let mut merged = HashMap::with_capacity(left_row.len() + right_row.len());
                for (k, v) in left_row {
                    merged.insert(k.clone(), v.clone());
                }
                for (k, v) in right_row {
                    merged.insert(k.clone(), v);
                }
                next.push(merged);
            }
        }
        Ok(next)
    }

    /// Hace el lookup en el right según el plan: PK directa
    /// (`Catalog::get_row`) o columna indexada (`lookup_pks_via_index`
    /// + fetch). Devuelve las filas matched ya como HashMap qualified.
    fn lookup_right_rows(
        &mut self,
        right: &JoinTable,
        plan: &IndexLoopPlan,
        left_value: &Value,
    ) -> DbResult<Vec<HashMap<String, Value>>> {
        match &plan.right_strategy {
            RightLookup::Pk => {
                let pk = match left_value {
                    Value::Integer(n) => *n,
                    _ => return Ok(Vec::new()),
                };
                let row_bytes = {
                    let mut catalog = Catalog::open(self.pager);
                    catalog.get_row(right.meta.root_page, pk)?
                };
                match row_bytes {
                    None => Ok(Vec::new()),
                    Some(bytes) => {
                        let decoded = decode_row(&right.meta, &bytes)?;
                        let mut qualified = HashMap::with_capacity(decoded.len());
                        for (col, val) in decoded {
                            qualified.insert(format!("{}.{}", right.qualifier, col), val);
                        }
                        Ok(vec![qualified])
                    }
                }
            }
            RightLookup::Index(idx) => {
                let pks = lookup_pks_via_index(self.pager, &right.meta, idx, left_value)?;
                let mut out = Vec::with_capacity(pks.len());
                let mut catalog = Catalog::open(self.pager);
                for pk in pks {
                    if let Some(bytes) = catalog.get_row(right.meta.root_page, pk)? {
                        let decoded = decode_row(&right.meta, &bytes)?;
                        let mut qualified = HashMap::with_capacity(decoded.len());
                        for (col, val) in decoded {
                            qualified.insert(format!("{}.{}", right.qualifier, col), val);
                        }
                        out.push(qualified);
                    }
                }
                Ok(out)
            }
        }
    }

    fn scan_qualified(&mut self, entry: &JoinTable) -> DbResult<Vec<HashMap<String, Value>>> {
        let raw = {
            let mut catalog = Catalog::open(self.pager);
            catalog.scan_rows(entry.meta.root_page, 0, None)?
        };
        let mut out = Vec::with_capacity(raw.len());
        for kv in raw {
            let decoded = decode_row(&entry.meta, &kv.value)?;
            let mut qualified = HashMap::with_capacity(decoded.len());
            for (col, val) in decoded {
                qualified.insert(format!("{}.{}", entry.qualifier, col), val);
            }
            out.push(qualified);
        }
        Ok(out)
    }

    /// Resuelve `stmt.table` + `stmt.joins` cargando los `TableMeta` y
    /// validando que no haya dos tablas expuestas con el mismo qualifier
    /// (alias preferido; nombre real si no hay alias).
    fn build_join_scope(&mut self, stmt: &SelectStmt) -> DbResult<JoinScope> {
        let mut tables: Vec<JoinTable> = Vec::with_capacity(1 + stmt.joins.len());
        let base = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .get_table(&stmt.table)?
                .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?
        };
        let base_qualifier = stmt
            .table_alias
            .clone()
            .unwrap_or_else(|| stmt.table.clone())
            .to_ascii_lowercase();
        tables.push(JoinTable {
            meta: base,
            qualifier: base_qualifier.clone(),
            raw_name: stmt.table.clone(),
            alias: stmt.table_alias.clone(),
        });
        for join in &stmt.joins {
            let meta = {
                let mut catalog = Catalog::open(self.pager);
                catalog
                    .get_table(&join.right.name)?
                    .ok_or_else(|| DbError::new(format!("tabla no existe: {}", join.right.name)))?
            };
            let qualifier = join
                .right
                .alias
                .clone()
                .unwrap_or_else(|| join.right.name.clone())
                .to_ascii_lowercase();
            if tables.iter().any(|t| t.qualifier == qualifier) {
                return Err(coded(
                    codes::TABLE_ALIAS_DUPLICATED,
                    format!(
                        "alias/nombre de tabla '{}' duplicado en el FROM — \
                         usá `AS otroalias` para des-ambiguar",
                        qualifier
                    ),
                ));
            }
            tables.push(JoinTable {
                meta,
                qualifier,
                raw_name: join.right.name.clone(),
                alias: join.right.alias.clone(),
            });
        }
        Ok(JoinScope {
            tables,
            hidden_in_star: HashSet::new(),
        })
    }

    /// Aplica un `WHERE` sobre el conjunto de filas joineadas. Las formas
    /// soportadas son las que se pueden evaluar fila-a-fila sobre el
    /// schema combinado: `Eq`, `Between`. `In`/`EqSubquery` se evalúan
    /// pre-computando el set una sola vez. `EqColumnRef`/`Exists` con
    /// JOINs no se soportan en este bloque.
    /// Bloque E1: aplica un `WhereExpr` arbitrario (con `AND`/`OR`/`NOT`)
    /// sobre filas joined. Para cada fila evaluamos la expresión con
    /// 3VL: solo sobrevive si devuelve `Some(true)`. Internamente reusa
    /// `filter_joined_rows_atom` para evaluar átomos.
    fn filter_joined_rows_expr(
        &mut self,
        rows: Vec<HashMap<String, Value>>,
        expr: &WhereExpr,
        scope: &JoinScope,
    ) -> DbResult<Vec<HashMap<String, Value>>> {
        // Fast-path: si es un único átomo, dispatchamos al evaluador atómico
        // original (mantiene la semántica/perfomance que ya existía).
        if let WhereExpr::Atom(c) = expr {
            return self.filter_joined_rows_atom(rows, c, scope);
        }
        let mut kept = Vec::with_capacity(rows.len());
        for row in rows {
            let verdict = self.eval_where_expr_joined(expr, &row, scope)?;
            if matches!(verdict, Some(true)) {
                kept.push(row);
            }
        }
        Ok(kept)
    }

    /// Evalúa un `WhereExpr` sobre una única fila joined con lógica
    /// trivaluada (3VL). Devuelve `Some(true)`/`Some(false)`/`None` donde
    /// `None` representa NULL/unknown. AND/OR aplican short-circuit
    /// estándar; NOT propaga NULL.
    fn eval_where_expr_joined(
        &mut self,
        expr: &WhereExpr,
        row: &HashMap<String, Value>,
        scope: &JoinScope,
    ) -> DbResult<Option<bool>> {
        match expr {
            WhereExpr::And(a, b) => {
                let la = self.eval_where_expr_joined(a, row, scope)?;
                if let Some(false) = la {
                    return Ok(Some(false));
                }
                let lb = self.eval_where_expr_joined(b, row, scope)?;
                Ok(match (la, lb) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                })
            }
            WhereExpr::Or(a, b) => {
                let la = self.eval_where_expr_joined(a, row, scope)?;
                if let Some(true) = la {
                    return Ok(Some(true));
                }
                let lb = self.eval_where_expr_joined(b, row, scope)?;
                Ok(match (la, lb) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                })
            }
            WhereExpr::Not(inner) => Ok(match self.eval_where_expr_joined(inner, row, scope)? {
                Some(b) => Some(!b),
                None => None,
            }),
            WhereExpr::Atom(c) => self.eval_atom_joined(c, row, scope),
        }
    }

    /// Evalúa un único `WhereClause` sobre una fila joined, devolviendo 3VL.
    /// `EqColumnRef`/correlated `EXISTS` no se soportan acá (mismo límite
    /// que en pre-E1) — `[GBY-4001]`.
    fn eval_atom_joined(
        &mut self,
        atom: &WhereClause,
        row: &HashMap<String, Value>,
        scope: &JoinScope,
    ) -> DbResult<Option<bool>> {
        match atom {
            WhereClause::Eq { column, value } => {
                let key = resolve_joined_column_key(scope, column)?;
                match row.get(&key) {
                    Some(Value::Null) => Ok(None),
                    Some(v) => match value {
                        Value::Null => Ok(None),
                        other => Ok(Some(values_equal(v, other))),
                    },
                    None => Ok(Some(false)),
                }
            }
            WhereClause::Between { column, from, to } => {
                let key = resolve_joined_column_key(scope, column)?;
                match row.get(&key) {
                    Some(Value::Integer(n)) => Ok(Some(*n >= *from && *n <= *to)),
                    Some(Value::Null) | None => Ok(None),
                    _ => Ok(Some(false)),
                }
            }
            WhereClause::In { column, subquery } => {
                let key = resolve_joined_column_key(scope, column)?;
                let inner = self.exec_select((**subquery).clone())?;
                if inner.columns.len() != 1 {
                    return Err(coded(
                        codes::SUBQUERY_MUST_RETURN_ONE_COLUMN,
                        format!(
                            "subquery en IN debe devolver exactamente 1 columna; devolvió {}",
                            inner.columns.len()
                        ),
                    ));
                }
                let set: Vec<Value> = inner
                    .rows
                    .into_iter()
                    .filter_map(|mut r| r.pop())
                    .filter(|v| !matches!(v, Value::Null))
                    .collect();
                match row.get(&key) {
                    Some(Value::Null) | None => Ok(None),
                    Some(v) => Ok(Some(set.iter().any(|s| values_equal(v, s)))),
                }
            }
            WhereClause::EqSubquery { column, subquery } => {
                let key = resolve_joined_column_key(scope, column)?;
                let inner = self.exec_select((**subquery).clone())?;
                if inner.columns.len() != 1 {
                    return Err(coded(
                        codes::SUBQUERY_MUST_RETURN_ONE_COLUMN,
                        format!(
                            "subquery escalar debe devolver exactamente 1 columna; devolvió {}",
                            inner.columns.len()
                        ),
                    ));
                }
                if inner.rows.len() > 1 {
                    return Err(coded(
                        codes::SCALAR_SUBQUERY_TOO_MANY_ROWS,
                        format!(
                            "subquery escalar en WHERE devolvió {} filas; debe devolver a lo sumo 1",
                            inner.rows.len()
                        ),
                    ));
                }
                let scalar = inner.rows.into_iter().next().and_then(|mut r| r.pop());
                match (scalar, row.get(&key)) {
                    (None, _) | (Some(Value::Null), _) | (_, Some(Value::Null)) | (_, None) => {
                        Ok(None)
                    }
                    (Some(expected), Some(v)) => Ok(Some(values_equal(v, &expected))),
                }
            }
            WhereClause::EqColumnRef { .. } | WhereClause::Exists { .. } => Err(coded(
                codes::WHERE_OPERATOR_UNSUPPORTED,
                "esta forma del WHERE (column-ref / EXISTS) aún no se combina con JOINs \
                 en este release; envolver el JOIN en una subquery o filtrar por valores literales",
            )),
            WhereClause::Compare { column, op, value } => {
                let key = resolve_joined_column_key(scope, column)?;
                Ok(eval_compare(row.get(&key), *op, value))
            }
            WhereClause::Like {
                column,
                pattern,
                negated,
            } => {
                let key = resolve_joined_column_key(scope, column)?;
                Ok(eval_like(row.get(&key), pattern, *negated))
            }
            WhereClause::IsNull { column, negated } => {
                let key = resolve_joined_column_key(scope, column)?;
                let is_null = matches!(row.get(&key), Some(Value::Null) | None);
                Ok(Some(if *negated { !is_null } else { is_null }))
            }
            WhereClause::InList {
                column,
                values,
                negated,
            } => {
                let key = resolve_joined_column_key(scope, column)?;
                Ok(eval_in_list(row.get(&key), values, *negated))
            }
        }
    }

    fn filter_joined_rows_atom(
        &mut self,
        rows: Vec<HashMap<String, Value>>,
        where_clause: &WhereClause,
        scope: &JoinScope,
    ) -> DbResult<Vec<HashMap<String, Value>>> {
        match where_clause {
            WhereClause::Eq { column, value } => {
                let key = resolve_joined_column_key(scope, column)?;
                let expected = value.clone();
                Ok(rows
                    .into_iter()
                    .filter(|r| {
                        r.get(&key)
                            .map(|v| values_equal(v, &expected))
                            .unwrap_or(false)
                    })
                    .collect())
            }
            WhereClause::Between { column, from, to } => {
                let key = resolve_joined_column_key(scope, column)?;
                let lo = *from;
                let hi = *to;
                Ok(rows
                    .into_iter()
                    .filter(|r| match r.get(&key) {
                        Some(Value::Integer(n)) => *n >= lo && *n <= hi,
                        _ => false,
                    })
                    .collect())
            }
            WhereClause::In { column, subquery } => {
                let key = resolve_joined_column_key(scope, column)?;
                let inner = self.exec_select((**subquery).clone())?;
                if inner.columns.len() != 1 {
                    return Err(coded(
                        codes::SUBQUERY_MUST_RETURN_ONE_COLUMN,
                        format!(
                            "subquery en IN debe devolver exactamente 1 columna; devolvió {}",
                            inner.columns.len()
                        ),
                    ));
                }
                let set: Vec<Value> = inner
                    .rows
                    .into_iter()
                    .filter_map(|mut r| r.pop())
                    .filter(|v| !matches!(v, Value::Null))
                    .collect();
                Ok(rows
                    .into_iter()
                    .filter(|r| {
                        r.get(&key)
                            .map(|v| set.iter().any(|s| values_equal(v, s)))
                            .unwrap_or(false)
                    })
                    .collect())
            }
            WhereClause::EqSubquery { column, subquery } => {
                let key = resolve_joined_column_key(scope, column)?;
                let inner = self.exec_select((**subquery).clone())?;
                if inner.columns.len() != 1 {
                    return Err(coded(
                        codes::SUBQUERY_MUST_RETURN_ONE_COLUMN,
                        format!(
                            "subquery escalar debe devolver exactamente 1 columna; devolvió {}",
                            inner.columns.len()
                        ),
                    ));
                }
                if inner.rows.len() > 1 {
                    return Err(coded(
                        codes::SCALAR_SUBQUERY_TOO_MANY_ROWS,
                        format!(
                            "subquery escalar en WHERE devolvió {} filas; debe devolver a lo sumo 1",
                            inner.rows.len()
                        ),
                    ));
                }
                let scalar = inner.rows.into_iter().next().and_then(|mut r| r.pop());
                match scalar {
                    None | Some(Value::Null) => Ok(Vec::new()),
                    Some(expected) => Ok(rows
                        .into_iter()
                        .filter(|r| {
                            r.get(&key)
                                .map(|v| values_equal(v, &expected))
                                .unwrap_or(false)
                        })
                        .collect()),
                }
            }
            WhereClause::EqColumnRef { .. } | WhereClause::Exists { .. } => Err(coded(
                codes::WHERE_OPERATOR_UNSUPPORTED,
                "esta forma del WHERE (column-ref / EXISTS) aún no se combina con JOINs \
                 en este release; envolver el JOIN en una subquery o filtrar por valores literales",
            )),
            // Átomos E2 sobre JOIN: dispatch al evaluador 3VL que filtra
            // fila-a-fila. La fast-path optimizada no aplica acá porque
            // estos operadores ya forzaron generic_post_filter arriba.
            WhereClause::Compare { .. }
            | WhereClause::Like { .. }
            | WhereClause::IsNull { .. }
            | WhereClause::InList { .. } => {
                let mut kept = Vec::with_capacity(rows.len());
                for row in rows {
                    if matches!(
                        self.eval_atom_joined(where_clause, &row, scope)?,
                        Some(true)
                    ) {
                        kept.push(row);
                    }
                }
                Ok(kept)
            }
        }
    }

    /// Bloque E1: evaluador 3VL de `WhereExpr` sobre una fila ya decodificada
    /// de una tabla single (sin JOIN). Devuelve `Some(true)`/`Some(false)`/
    /// `None` (NULL/unknown). Se usa cuando el WHERE contiene
    /// `AND`/`OR`/`NOT` y por lo tanto no podemos usar las fast-paths
    /// indexadas. Las claves del row son nombres normalizados de columnas
    /// (lowercase, sin qualifier).
    fn eval_where_expr_single(
        &mut self,
        expr: &WhereExpr,
        meta: &TableMeta,
        row: &HashMap<String, Value>,
    ) -> DbResult<Option<bool>> {
        match expr {
            WhereExpr::And(a, b) => {
                let la = self.eval_where_expr_single(a, meta, row)?;
                if let Some(false) = la {
                    return Ok(Some(false));
                }
                let lb = self.eval_where_expr_single(b, meta, row)?;
                Ok(match (la, lb) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                })
            }
            WhereExpr::Or(a, b) => {
                let la = self.eval_where_expr_single(a, meta, row)?;
                if let Some(true) = la {
                    return Ok(Some(true));
                }
                let lb = self.eval_where_expr_single(b, meta, row)?;
                Ok(match (la, lb) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                })
            }
            WhereExpr::Not(inner) => Ok(match self.eval_where_expr_single(inner, meta, row)? {
                Some(b) => Some(!b),
                None => None,
            }),
            WhereExpr::Atom(c) => self.eval_atom_single(c, meta, row),
        }
    }

    fn eval_atom_single(
        &mut self,
        atom: &WhereClause,
        meta: &TableMeta,
        row: &HashMap<String, Value>,
    ) -> DbResult<Option<bool>> {
        match atom {
            WhereClause::Eq { column, value } => {
                let key = normalize_ident(column);
                if meta.column(&key).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!("columna '{}' no existe en '{}'", column, meta.name),
                    ));
                }
                match row.get(&key) {
                    Some(Value::Null) | None => Ok(None),
                    Some(v) => match value {
                        Value::Null => Ok(None),
                        other => Ok(Some(values_equal(v, other))),
                    },
                }
            }
            WhereClause::Between { column, from, to } => {
                let key = normalize_ident(column);
                if meta.column(&key).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!("columna '{}' no existe en '{}'", column, meta.name),
                    ));
                }
                match row.get(&key) {
                    Some(Value::Integer(n)) => Ok(Some(*n >= *from && *n <= *to)),
                    Some(Value::Null) | None => Ok(None),
                    _ => Ok(Some(false)),
                }
            }
            WhereClause::In { column, subquery } => {
                let key = normalize_ident(column);
                if meta.column(&key).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!("columna '{}' no existe en '{}'", column, meta.name),
                    ));
                }
                // NOTA E1: la subquery se re-ejecuta por cada fila del
                // outer cuando vive dentro de un AND/OR/NOT. Es correcto
                // pero no óptimo — un caching pre-loop queda para futuros
                // bloques (H optimiza subqueries; este bloque prioriza
                // semántica). La fast-path single-átomo del exec_select
                // sigue sin pagar este costo.
                let inner = self.exec_select((**subquery).clone())?;
                if inner.columns.len() != 1 {
                    return Err(coded(
                        codes::SUBQUERY_MUST_RETURN_ONE_COLUMN,
                        format!(
                            "subquery en IN debe devolver exactamente 1 columna; devolvió {}",
                            inner.columns.len()
                        ),
                    ));
                }
                let set: Vec<Value> = inner
                    .rows
                    .into_iter()
                    .filter_map(|mut r| r.pop())
                    .filter(|v| !matches!(v, Value::Null))
                    .collect();
                match row.get(&key) {
                    Some(Value::Null) | None => Ok(None),
                    Some(v) => Ok(Some(set.iter().any(|s| values_equal(v, s)))),
                }
            }
            WhereClause::EqSubquery { column, subquery } => {
                let key = normalize_ident(column);
                if meta.column(&key).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!("columna '{}' no existe en '{}'", column, meta.name),
                    ));
                }
                let inner = self.exec_select((**subquery).clone())?;
                if inner.columns.len() != 1 {
                    return Err(coded(
                        codes::SUBQUERY_MUST_RETURN_ONE_COLUMN,
                        format!(
                            "subquery escalar debe devolver exactamente 1 columna; devolvió {}",
                            inner.columns.len()
                        ),
                    ));
                }
                if inner.rows.len() > 1 {
                    return Err(coded(
                        codes::SCALAR_SUBQUERY_TOO_MANY_ROWS,
                        format!(
                            "subquery escalar en WHERE devolvió {} filas; debe devolver a lo sumo 1",
                            inner.rows.len()
                        ),
                    ));
                }
                let scalar = inner.rows.into_iter().next().and_then(|mut r| r.pop());
                match (scalar, row.get(&key)) {
                    (None, _) | (Some(Value::Null), _) | (_, Some(Value::Null)) | (_, None) => {
                        Ok(None)
                    }
                    (Some(expected), Some(v)) => Ok(Some(values_equal(v, &expected))),
                }
            }
            WhereClause::Exists { subquery, negated } => {
                // EXISTS dentro de combinadores: solo soportamos
                // no-correlacionado en este release. Correlated EXISTS
                // dentro de AND/OR/NOT requiere un dispatch más complejo
                // (push del outer row antes de cada eval) que queda
                // explícitamente fuera de E1.
                if subquery_has_outer_refs(subquery) {
                    return Err(coded(
                        codes::WHERE_COMBINATOR_CORRELATED_UNSUPPORTED,
                        "EXISTS correlacionado dentro de AND/OR/NOT no se soporta en \
                         este release; usalo como único predicado del WHERE",
                    ));
                }
                let inner = self.exec_select((**subquery).clone())?;
                let has_rows = !inner.rows.is_empty();
                let pass = if *negated { !has_rows } else { has_rows };
                Ok(Some(pass))
            }
            WhereClause::EqColumnRef { .. } => Err(coded(
                codes::WHERE_COMBINATOR_CORRELATED_UNSUPPORTED,
                "referencias a columnas del outer dentro de AND/OR/NOT no se soportan \
                 en este release; el column-ref correlacionado debe ser el único predicado",
            )),
            WhereClause::Compare { column, op, value } => {
                let key = normalize_ident(column);
                if meta.column(&key).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!("columna '{}' no existe en '{}'", column, meta.name),
                    ));
                }
                Ok(eval_compare(row.get(&key), *op, value))
            }
            WhereClause::Like {
                column,
                pattern,
                negated,
            } => {
                let key = normalize_ident(column);
                if meta.column(&key).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!("columna '{}' no existe en '{}'", column, meta.name),
                    ));
                }
                Ok(eval_like(row.get(&key), pattern, *negated))
            }
            WhereClause::IsNull { column, negated } => {
                let key = normalize_ident(column);
                if meta.column(&key).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!("columna '{}' no existe en '{}'", column, meta.name),
                    ));
                }
                let is_null = matches!(row.get(&key), Some(Value::Null) | None);
                Ok(Some(if *negated { !is_null } else { is_null }))
            }
            WhereClause::InList {
                column,
                values,
                negated,
            } => {
                let key = normalize_ident(column);
                if meta.column(&key).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!("columna '{}' no existe en '{}'", column, meta.name),
                    ));
                }
                Ok(eval_in_list(row.get(&key), values, *negated))
            }
        }
    }

    fn exec_update(&mut self, stmt: UpdateStmt) -> DbResult<ResultSet> {
        let meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .get_table(&stmt.table)?
                .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?
        };
        ensure_pk_filter(&meta, &stmt.where_column)?;

        let mut overrides: HashMap<String, Value> = HashMap::new();
        for (column_name, value) in stmt.assignments {
            let normalized = normalize_ident(&column_name);
            if normalized == normalize_ident(&meta.primary_key) {
                return Err(coded(
                    codes::UPDATE_PK_NOT_ALLOWED,
                    format!(
                        "UPDATE sobre '{}': no se permite cambiar la PRIMARY KEY '{}' (esta versión)",
                        meta.name, meta.primary_key
                    ),
                ));
            }
            if meta.column(&normalized).is_none() {
                return Err(coded(
                    codes::COLUMN_NOT_FOUND,
                    format!(
                        "UPDATE sobre '{}': columna '{}' no existe en la tabla",
                        meta.name, column_name
                    ),
                ));
            }
            if overrides.insert(normalized, value).is_some() {
                return Err(coded(
                    codes::DUPLICATE_COLUMN_NAME,
                    format!(
                        "UPDATE sobre '{}': columna '{}' aparece más de una vez en SET",
                        meta.name, column_name
                    ),
                ));
            }
        }

        let existing = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .get_row(meta.root_page, stmt.where_pk)?
                .ok_or_else(|| {
                    coded(
                        codes::ROW_NOT_FOUND_FOR_PK,
                        format!(
                            "UPDATE sobre '{}': fila no existe PK={}",
                            meta.name, stmt.where_pk
                        ),
                    )
                })?
        };
        let old_row = decode_row(&meta, &existing)?;
        let mut current = old_row.clone();
        for (key, value) in &overrides {
            current.insert(key.clone(), value.clone());
        }

        // NOT NULL: any assignment that lands a NULL on a NOT NULL column
        // must be rejected before we touch storage.
        for column in &meta.columns {
            if !column.not_null {
                continue;
            }
            let normalized = normalize_ident(&column.name);
            if !overrides.contains_key(&normalized) {
                continue;
            }
            if matches!(current.get(&normalized), Some(Value::Null) | None) {
                return Err(coded(
                    codes::NOT_NULL_VIOLATED,
                    format!(
                        "UPDATE sobre '{}': columna '{}' es NOT NULL y no puede quedar en NULL",
                        meta.name, column.name
                    ),
                ));
            }
        }

        // UNIQUE pre-check on every changed indexed column (excluding
        // self pk so updating to the same value is a no-op).
        for idx in &meta.indexes {
            if !idx.unique {
                continue;
            }
            let normalized = normalize_ident(&idx.column);
            if !overrides.contains_key(&normalized) {
                continue;
            }
            let column = meta.column(&idx.column).ok_or_else(|| {
                DbError::new(format!(
                    "índice apunta a columna inexistente: {}",
                    idx.column
                ))
            })?;
            let new_value = current.get(&normalized).cloned().unwrap_or(Value::Null);
            let new_bytes = encode_column_value(column, &new_value)?;
            check_unique_conflict(self.pager, idx, &new_bytes, Some(stmt.where_pk))?;
        }

        // FK pre-check on every changed FK column. We pass `where_pk`
        // as the self-ref-allowed pk so updating a self-FK column to
        // point at the same row stays valid.
        enforce_fk_on_update(self.pager, &meta, &old_row, &current, stmt.where_pk)?;

        let (pk, row_bytes) = encode_row(&meta, &current)?;
        if pk != stmt.where_pk {
            return Err(DbError::new(format!(
                "inconsistencia interna en UPDATE sobre '{}': la PK reconstruida del row \
                 es {} pero el WHERE pidió pk={}",
                meta.name, pk, stmt.where_pk
            )));
        }
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.upsert_row(meta.root_page, pk, row_bytes)?;
        }

        // Maintain only the indexes whose column was actually mutated, and
        // only when the new value differs from the old one. That keeps
        // UPDATEs that don't touch indexed columns free of index work.
        for idx in &meta.indexes {
            let normalized = normalize_ident(&idx.column);
            if !overrides.contains_key(&normalized) {
                continue;
            }
            let column = meta.column(&idx.column).ok_or_else(|| {
                DbError::new(format!(
                    "índice apunta a columna inexistente: {}",
                    idx.column
                ))
            })?;
            let old_value = old_row.get(&normalized).cloned().unwrap_or(Value::Null);
            let new_value = current.get(&normalized).cloned().unwrap_or(Value::Null);
            if old_value == new_value {
                continue;
            }
            let old_bytes = encode_column_value(column, &old_value)?;
            let new_bytes = encode_column_value(column, &new_value)?;
            index_remove_pk(self.pager, idx.root_page, idx.kind, &old_bytes, pk)?;
            index_upsert_pk(self.pager, idx.root_page, idx.kind, &new_bytes, pk)?;
        }

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("OK".to_string()),
        })
    }

    fn exec_create_index(&mut self, stmt: CreateIndexStmt) -> DbResult<ResultSet> {
        // 0. Validate the index name shape up front — same rule as table
        //    and column identifiers, since `DROP INDEX` and the catalog
        //    have to be able to address it unambiguously.
        validate_identifier(&stmt.name, "índice")?;
        // 1. Resolve the table.
        let mut meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .get_table(&stmt.table)?
                .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?
        };

        // 2. Validate column + type.
        validate_indexable(&meta, &stmt.column)?;

        // 3. Reject duplicate index name within this table; also reject
        //    duplicate index *over the same column* (one secondary index
        //    per column is enough for this version's equality lookups).
        if meta.index_by_name(&stmt.name).is_some() {
            return Err(coded(
                codes::INDEX_ALREADY_EXISTS,
                format!(
                    "ya existe un índice llamado '{}' en la tabla '{}'",
                    stmt.name, meta.name
                ),
            ));
        }
        if meta.index_for_column(&stmt.column).is_some() {
            return Err(coded(
                codes::INDEX_ALREADY_EXISTS,
                format!(
                    "la columna '{}' ya tiene un índice secundario en '{}' \
                     (esta versión admite un solo índice por columna)",
                    stmt.column, meta.name
                ),
            ));
        }

        // Reject duplicate index name across the whole catalog.
        {
            let mut catalog = Catalog::open(self.pager);
            for other in catalog.list_tables()? {
                if other.name.eq_ignore_ascii_case(&meta.name) {
                    continue;
                }
                if other.index_by_name(&stmt.name).is_some() {
                    return Err(coded(
                        codes::INDEX_ALREADY_EXISTS,
                        format!(
                            "ya existe un índice llamado '{}' en la tabla '{}'",
                            stmt.name, other.name
                        ),
                    ));
                }
            }
        }

        // 4. Allocate a fresh leaf page as the index root.
        let idx_root = self.pager.new_page()?;
        let mut leaf = vec![0; self.pager.page_size()];
        init_leaf_page(&mut leaf);
        self.pager.write_page(idx_root, &leaf, true)?;

        // 5. Backfill: walk every existing row and insert it into the
        //    index. We do this *before* we publish the index in the catalog
        //    so a backfill failure leaves no half-built metadata behind
        //    (the page leak is acceptable; the txn rollback would also
        //    discard everything). For UNIQUE indexes we additionally
        //    track which value bytes we've already seen so we abort with
        //    a clear error when a backfill would violate uniqueness —
        //    otherwise the conflict would only surface on the next INSERT.
        let column = meta
            .column(&stmt.column)
            .ok_or_else(|| DbError::new(format!("columna no existe: {}", stmt.column)))?
            .clone();
        let table_root = meta.root_page;
        let rows = {
            let mut catalog = Catalog::open(self.pager);
            catalog.scan_rows(table_root, 0, None)?
        };
        let mut seen_unique: HashSet<Vec<u8>> = HashSet::new();
        for kv in rows {
            let decoded = decode_row(&meta, &kv.value)?;
            let value = decoded
                .get(&normalize_ident(&column.name))
                .cloned()
                .unwrap_or(Value::Null);
            let value_bytes = encode_column_value(&column, &value)?;
            if stmt.unique && value_bytes != [0u8] && !seen_unique.insert(value_bytes.clone()) {
                return Err(DbError::new(format!(
                    "CREATE UNIQUE INDEX rechazado: columna '{}' tiene valores duplicados existentes",
                    column.name
                )));
            }
            index_upsert_pk(
                self.pager,
                idx_root,
                IndexKind::for_column(column.column_type),
                &value_bytes,
                kv.key,
            )?;
        }

        // 6. Publish the index in the catalog.
        meta.indexes.push(IndexMeta {
            name: stmt.name,
            column: stmt.column,
            root_page: idx_root,
            unique: stmt.unique,
            kind: IndexKind::for_column(column.column_type),
        });
        let mut catalog = Catalog::open(self.pager);
        catalog.put_table(&meta)?;

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("OK".to_string()),
        })
    }

    fn exec_drop_index(&mut self, stmt: DropIndexStmt) -> DbResult<ResultSet> {
        let mut catalog = Catalog::open(self.pager);
        let tables = catalog.list_tables()?;
        let mut hit: Option<TableMeta> = None;
        for table in tables {
            if table.index_by_name(&stmt.name).is_some() {
                hit = Some(table);
                break;
            }
        }
        let mut owner =
            hit.ok_or_else(|| DbError::new(format!("índice no existe: {}", stmt.name)))?;
        owner
            .indexes
            .retain(|idx| !idx.name.eq_ignore_ascii_case(&stmt.name));
        catalog.put_table(&owner)?;
        // We intentionally don't free the index pages: the page allocator
        // has no free-list yet, so dropping the catalog reference is the
        // safest thing to do. A future `vacuum` will reclaim the space.
        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("OK".to_string()),
        })
    }

    fn exec_delete(&mut self, stmt: DeleteStmt) -> DbResult<ResultSet> {
        let meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .get_table(&stmt.table)?
                .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?
        };
        ensure_pk_filter(&meta, &stmt.where_column)?;

        // Refuse the DELETE up front if the target row doesn't exist,
        // matching the pre-FK behaviour. The cascade walker tolerates
        // already-deleted rows (cycles, multi-path), so we have to
        // gate the entry point ourselves.
        let exists = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_row(meta.root_page, stmt.where_pk)?.is_some()
        };
        if !exists {
            return Err(coded(
                codes::ROW_NOT_FOUND_FOR_PK,
                format!(
                    "DELETE FROM '{}': fila no existe PK={}",
                    meta.name, stmt.where_pk
                ),
            ));
        }

        // delete_with_cascade resolves the FK graph, applies RESTRICT
        // by aborting before any write happens, and on CASCADE
        // iteratively removes child rows + their secondary-index entries.
        delete_with_cascade(self.pager, &meta.name, stmt.where_pk)?;

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("OK".to_string()),
        })
    }
}

pub fn parse(sql_text: &str) -> DbResult<Vec<Statement>> {
    let mut statements = Vec::new();
    for chunk in split_statements(sql_text) {
        let tokens = tokenize(&chunk)?;
        let mut parser = Parser { tokens, pos: 0 };
        let statement = parser.parse_statement()?;
        if !parser.is_eof() {
            return Err(DbError::new(format!(
                "token inesperado: {}",
                parser.peek().text
            )));
        }
        statements.push(statement);
    }
    Ok(statements)
}

pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let bytes = sql.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let ch = bytes[index] as char;
        if ch == '\'' {
            if in_string && index + 1 < bytes.len() && bytes[index + 1] as char == '\'' {
                current.push_str("''");
                index += 2;
                continue;
            }
            in_string = !in_string;
            current.push(ch);
            index += 1;
            continue;
        }
        if ch == ';' && !in_string {
            let stmt = current.trim();
            if !stmt.is_empty() {
                out.push(stmt.to_string());
            }
            current.clear();
            index += 1;
            continue;
        }
        current.push(ch);
        index += 1;
    }
    let tail = current.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

pub fn encode_row(meta: &TableMeta, values: &HashMap<String, Value>) -> DbResult<(i64, Vec<u8>)> {
    let mut out = Vec::new();
    let mut pk = None;

    for column in &meta.columns {
        let normalized = normalize_ident(&column.name);
        let value = values.get(&normalized).cloned().unwrap_or(Value::Null);
        match (&column.column_type, value) {
            (ColumnType::Int, Value::Null) => {
                if column.name.eq_ignore_ascii_case(&meta.primary_key) {
                    return Err(coded(
                        codes::PRIMARY_KEY_NULL,
                        format!(
                            "PRIMARY KEY '{}' no puede ser NULL en INSERT/UPDATE de tabla '{}'",
                            column.name, meta.name
                        ),
                    ));
                }
                out.push(0);
            }
            (ColumnType::Int, Value::Integer(number)) => {
                out.push(1);
                out.extend_from_slice(&number.to_le_bytes());
                if column.name.eq_ignore_ascii_case(&meta.primary_key) {
                    pk = Some(number);
                }
            }
            (ColumnType::Float, Value::Null)
            | (ColumnType::Bool, Value::Null)
            | (ColumnType::Text, Value::Null)
            | (ColumnType::Date, Value::Null)
            | (ColumnType::DateTime, Value::Null)
            | (ColumnType::Json, Value::Null) => out.push(0),
            (ColumnType::Float, Value::Float(number)) => {
                out.push(1);
                out.extend_from_slice(&number.to_le_bytes());
            }
            (ColumnType::Float, Value::Integer(number)) => {
                out.push(1);
                out.extend_from_slice(&(number as f64).to_le_bytes());
            }
            (ColumnType::Bool, Value::Bool(flag)) => {
                out.push(1);
                out.push(u8::from(flag));
            }
            (column_type, Value::String(text)) if column_type.stores_as_text() => {
                let bytes = text.as_bytes();
                if bytes.len() > u16::MAX as usize {
                    return Err(DbError::new(format!("{} demasiado largo", column.name)));
                }
                out.push(1);
                out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
                out.extend_from_slice(bytes);
            }
            (ColumnType::Int, _) => {
                return Err(DbError::new(format!("{} debe ser INT", column.name)))
            }
            (ColumnType::Float, _) => {
                return Err(DbError::new(format!("{} debe ser FLOAT", column.name)))
            }
            (ColumnType::Bool, _) => {
                return Err(DbError::new(format!("{} debe ser BOOL", column.name)))
            }
            (_, _) => {
                return Err(DbError::new(format!(
                    "{} debe ser TEXT-compatible",
                    column.name
                )))
            }
        }
    }

    Ok((
        pk.ok_or_else(|| {
            DbError::new(format!(
                "INSERT/UPDATE de tabla '{}' sin valor para la PRIMARY KEY '{}'",
                meta.name, meta.primary_key
            ))
        })?,
        out,
    ))
}

pub fn decode_row(meta: &TableMeta, data: &[u8]) -> DbResult<HashMap<String, Value>> {
    let mut offset = 0usize;
    let mut out = HashMap::new();

    for column in &meta.columns {
        // EOF before this column means the row was written under an older
        // schema (before ALTER TABLE ADD COLUMN added the trailing
        // column). Fall back to the column's DEFAULT, or NULL when it has
        // none. The on-disk row stays untouched until the next UPDATE
        // rewrites it with the full column count.
        if offset >= data.len() {
            let key = normalize_ident(&column.name);
            let value = match &column.default {
                Some(default) => default_to_value(default),
                None => Value::Null,
            };
            out.insert(key, value);
            continue;
        }
        let present = data[offset];
        offset += 1;
        let key = normalize_ident(&column.name);
        if present == 0 {
            out.insert(key, Value::Null);
            continue;
        }

        let value = match column.column_type {
            ColumnType::Int => {
                if offset + 8 > data.len() {
                    return Err(DbError::new(format!(
                        "fila corrupta en tabla '{}': campo '{}' (INT) necesita 8 bytes \
                         en offset {}, solo quedan {}",
                        meta.name,
                        column.name,
                        offset,
                        data.len() - offset
                    )));
                }
                let number = i64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                offset += 8;
                Value::Integer(number)
            }
            ColumnType::Float => {
                if offset + 8 > data.len() {
                    return Err(DbError::new(format!(
                        "fila corrupta en tabla '{}': campo '{}' (FLOAT) necesita 8 bytes \
                         en offset {}, solo quedan {}",
                        meta.name,
                        column.name,
                        offset,
                        data.len() - offset
                    )));
                }
                let number = f64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                offset += 8;
                Value::Float(number)
            }
            ColumnType::Bool => {
                if offset >= data.len() {
                    return Err(DbError::new(format!(
                        "fila corrupta en tabla '{}': campo '{}' (BOOL) necesita 1 byte \
                         en offset {} (data_len={})",
                        meta.name,
                        column.name,
                        offset,
                        data.len()
                    )));
                }
                let flag = data[offset] != 0;
                offset += 1;
                Value::Bool(flag)
            }
            ColumnType::Text | ColumnType::Date | ColumnType::DateTime | ColumnType::Json => {
                if offset + 2 > data.len() {
                    return Err(DbError::new(format!(
                        "fila corrupta en tabla '{}': campo '{}' ({}) necesita 2 bytes \
                         para len en offset {}, solo quedan {}",
                        meta.name,
                        column.name,
                        column.column_type.as_sql(),
                        offset,
                        data.len() - offset
                    )));
                }
                let len = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
                offset += 2;
                if offset + len > data.len() {
                    return Err(DbError::new(format!(
                        "fila corrupta en tabla '{}': campo '{}' ({}) declara len={} \
                         en offset {}, solo quedan {} bytes",
                        meta.name,
                        column.name,
                        column.column_type.as_sql(),
                        len,
                        offset,
                        data.len() - offset
                    )));
                }
                let text = String::from_utf8(data[offset..offset + len].to_vec())?;
                offset += len;
                Value::String(text)
            }
        };
        out.insert(key, value);
    }

    Ok(out)
}

fn project_row(
    selected_columns: &[(String, String)],
    row: &HashMap<String, Value>,
) -> DbResult<Vec<Value>> {
    let mut out = Vec::with_capacity(selected_columns.len());
    for (_, normalized) in selected_columns {
        let value = row.get(normalized).cloned().ok_or_else(|| {
            DbError::new(format!("columna no encontrada en fila: {}", normalized))
        })?;
        out.push(value);
    }
    Ok(out)
}

fn resolve_selected_columns(
    meta: &TableMeta,
    requested: &[String],
) -> DbResult<Vec<(String, String)>> {
    if requested.is_empty() {
        return Ok(meta
            .columns
            .iter()
            .map(|column| (column.name.clone(), normalize_ident(&column.name)))
            .collect());
    }

    let mut out = Vec::with_capacity(requested.len());
    for name in requested {
        let normalized = normalize_ident(name);
        let column = meta.column(&normalized).ok_or_else(|| {
            coded(
                codes::COLUMN_NOT_FOUND,
                format!("columna '{}' no existe en tabla '{}'", name, meta.name),
            )
        })?;
        out.push((column.name.clone(), normalize_ident(&column.name)));
    }
    Ok(out)
}

fn ensure_pk_filter(meta: &TableMeta, column: &str) -> DbResult<()> {
    if meta.primary_key.eq_ignore_ascii_case(column) {
        return Ok(());
    }
    Err(coded(
        codes::UPDATE_DELETE_REQUIRES_PK_FILTER,
        format!(
            "UPDATE/DELETE solo soportan WHERE sobre la PRIMARY KEY ({}); \
             '{}' no es la PK de '{}'",
            meta.primary_key, column, meta.name
        ),
    ))
}

fn window_rows<T: Clone>(rows: Vec<T>, offset: usize, limit: Option<usize>) -> Vec<T> {
    if offset >= rows.len() {
        return Vec::new();
    }
    let end = match limit {
        Some(limit) => (offset + limit).min(rows.len()),
        None => rows.len(),
    };
    rows[offset..end].to_vec()
}

/// Translate `WHERE col = value` into the list of PKs that match, by
/// going through the secondary index `idx`. Returns an empty Vec when no
/// row matches. Branches on `idx.kind`: hash buckets for legacy indexes,
/// direct value-keyed lookup for OrderedInt indexes (ADR-0017).
fn lookup_pks_via_index(
    pager: &mut Pager,
    meta: &TableMeta,
    idx: &IndexMeta,
    value: &Value,
) -> DbResult<Vec<i64>> {
    let column = meta.column(&idx.column).ok_or_else(|| {
        DbError::new(format!(
            "índice apunta a columna inexistente: {}",
            idx.column
        ))
    })?;
    let value_bytes = encode_column_value(column, value)?;

    let mut tree = Tree::new(pager);
    match idx.kind {
        IndexKind::Hash => {
            let key = hash_value(&value_bytes);
            let bucket_bytes = match tree.get(idx.root_page, key)? {
                Some(b) => b,
                None => return Ok(Vec::new()),
            };
            let bucket = decode_bucket(&bucket_bytes)?;
            Ok(bucket_lookup(&bucket, &value_bytes))
        }
        IndexKind::OrderedInt => {
            // NULL never lives in an OrderedInt index, so a NULL lookup
            // is always empty (matches SQL semantics for `WHERE col = NULL`).
            let Some(key) = ordered_int_key_from_value_bytes(&value_bytes)? else {
                return Ok(Vec::new());
            };
            let bucket_bytes = match tree.get(idx.root_page, key)? {
                Some(b) => b,
                None => return Ok(Vec::new()),
            };
            decode_ordered_bucket(&bucket_bytes)
        }
    }
}

/// Walk an OrderedInt index from key `from` to key `to` (inclusive)
/// and return every PK in the range, sorted by indexed value first
/// and PK second (stable order inside duplicates). The caller is
/// responsible for guarding that `idx.kind == OrderedInt`.
fn lookup_pks_via_index_range(
    pager: &mut Pager,
    idx: &IndexMeta,
    from: i64,
    to: i64,
) -> DbResult<Vec<i64>> {
    debug_assert!(matches!(idx.kind, IndexKind::OrderedInt));
    let mut pks = Vec::new();
    let tree = Tree::new(pager);
    let cursor = tree.cursor_range(idx.root_page, from, to)?;
    for entry in cursor {
        let kv = entry?;
        let bucket = decode_ordered_bucket(&kv.value)?;
        pks.extend(bucket);
    }
    Ok(pks)
}

/// Add `(value_bytes, pk)` to the secondary index B+Tree rooted at
/// `idx_root`. Idempotent — re-inserting the same `(value, pk)` is a
/// no-op. Branches on `kind`.
pub(crate) fn index_upsert_pk(
    pager: &mut Pager,
    idx_root: u32,
    kind: IndexKind,
    value_bytes: &[u8],
    pk: i64,
) -> DbResult<()> {
    let mut tree = Tree::new(pager);
    match kind {
        IndexKind::Hash => {
            let key = hash_value(value_bytes);
            let mut bucket = match tree.get(idx_root, key)? {
                Some(bytes) => decode_bucket(&bytes)?,
                None => Vec::new(),
            };
            bucket_insert(&mut bucket, value_bytes.to_vec(), pk);
            let payload = encode_bucket(&bucket)?;
            tree.upsert(idx_root, key, payload)?;
        }
        IndexKind::OrderedInt => {
            // NULLs are intentionally not stored in OrderedInt indexes
            // (see module docs). UNIQUE with multiple NULLs and BETWEEN
            // ignoring NULL both fall out of this naturally.
            let Some(key) = ordered_int_key_from_value_bytes(value_bytes)? else {
                return Ok(());
            };
            let mut bucket = match tree.get(idx_root, key)? {
                Some(bytes) => decode_ordered_bucket(&bytes)?,
                None => Vec::new(),
            };
            ordered_bucket_insert(&mut bucket, pk);
            let payload = encode_ordered_bucket(&bucket)?;
            tree.upsert(idx_root, key, payload)?;
        }
    }
    Ok(())
}

/// Remove `(value_bytes, pk)` from the index. If the bucket becomes
/// empty after the removal we delete the leaf entry outright; otherwise
/// we re-write the smaller bucket. Returns whether an entry was
/// actually removed.
pub(crate) fn index_remove_pk(
    pager: &mut Pager,
    idx_root: u32,
    kind: IndexKind,
    value_bytes: &[u8],
    pk: i64,
) -> DbResult<bool> {
    let mut tree = Tree::new(pager);
    match kind {
        IndexKind::Hash => {
            let key = hash_value(value_bytes);
            let Some(bytes) = tree.get(idx_root, key)? else {
                return Ok(false);
            };
            let mut bucket = decode_bucket(&bytes)?;
            let removed = bucket_remove(&mut bucket, value_bytes, pk);
            if !removed {
                return Ok(false);
            }
            if bucket.is_empty() {
                tree.delete(idx_root, key)?;
            } else {
                let payload = encode_bucket(&bucket)?;
                tree.upsert(idx_root, key, payload)?;
            }
            Ok(true)
        }
        IndexKind::OrderedInt => {
            let Some(key) = ordered_int_key_from_value_bytes(value_bytes)? else {
                return Ok(false);
            };
            let Some(bytes) = tree.get(idx_root, key)? else {
                return Ok(false);
            };
            let mut bucket = decode_ordered_bucket(&bytes)?;
            let removed = ordered_bucket_remove(&mut bucket, pk);
            if !removed {
                return Ok(false);
            }
            if bucket.is_empty() {
                tree.delete(idx_root, key)?;
            } else {
                let payload = encode_ordered_bucket(&bucket)?;
                tree.upsert(idx_root, key, payload)?;
            }
            Ok(true)
        }
    }
}

/// Convert a parser-time `Value` literal into a catalog `DefaultLiteral`.
/// Type compatibility against the column type is enforced later by
/// `validate_create_table` — here we only translate the shape.
fn value_to_default(value: &Value) -> DbResult<DefaultLiteral> {
    Ok(match value {
        Value::Null => DefaultLiteral::Null,
        Value::Integer(n) => DefaultLiteral::Integer(*n),
        Value::Float(n) => DefaultLiteral::Float(*n),
        Value::Bool(b) => DefaultLiteral::Bool(*b),
        Value::String(s) => DefaultLiteral::String(s.clone()),
    })
}

fn default_to_value(default: &DefaultLiteral) -> Value {
    match default {
        DefaultLiteral::Null => Value::Null,
        DefaultLiteral::Integer(n) => Value::Integer(*n),
        DefaultLiteral::Float(n) => Value::Float(*n),
        DefaultLiteral::Bool(b) => Value::Bool(*b),
        DefaultLiteral::String(s) => Value::String(s.clone()),
    }
}

/// For every column the user did not list in INSERT, apply its DEFAULT (if
/// any). Columns without a default are left absent — `encode_row` then
/// stores them as NULL, which `enforce_not_null_on_insert` will catch
/// downstream when the column is NOT NULL.
fn apply_defaults(meta: &TableMeta, values: &mut HashMap<String, Value>) {
    for column in &meta.columns {
        let normalized = normalize_ident(&column.name);
        if values.contains_key(&normalized) {
            continue;
        }
        if let Some(default) = &column.default {
            values.insert(normalized, default_to_value(default));
        }
    }
}

/// After defaults are applied, fail if any NOT NULL column still resolves
/// to NULL (either explicit `NULL` literal or absent altogether).
fn enforce_not_null_on_insert(meta: &TableMeta, values: &HashMap<String, Value>) -> DbResult<()> {
    for column in &meta.columns {
        if !column.not_null {
            continue;
        }
        let normalized = normalize_ident(&column.name);
        let is_null = matches!(values.get(&normalized), None | Some(Value::Null));
        if is_null {
            return Err(coded(
                codes::NOT_NULL_VIOLATED,
                format!(
                    "INSERT INTO '{}': columna '{}' es NOT NULL y no fue cubierta por VALUES \
                     (ni tiene DEFAULT no nulo)",
                    meta.name, column.name
                ),
            ));
        }
    }
    Ok(())
}

/// Translate the parser's `ForeignKeyDef` into the catalog-layer
/// `ForeignKeyMeta`. Lives here (not in catalog) so the SQL frontend
/// stays the only place that knows about the parser AST.
fn fk_def_to_meta(def: &ForeignKeyDef) -> ForeignKeyMeta {
    ForeignKeyMeta {
        table: def.table.clone(),
        column: def.column.clone(),
        on_delete: def.on_delete,
    }
}

/// Validate every `FOREIGN KEY` declared on `meta` at DDL time:
///
/// * The target table must exist (or be the table being created — that
///   handles self-referencing FKs in `CREATE TABLE`).
/// * The target column must be the parent table's `PRIMARY KEY`. This
///   version doesn't accept `REFERENCES` against arbitrary `UNIQUE`
///   columns yet — it keeps the lookup path simple (parent PK is always
///   indexed by the table's own B+Tree) and matches the most common
///   real-world FK shape.
/// * The FK column's type must match the parent's PK type (today both
///   are necessarily `INT`).
fn validate_fk_targets(pager: &mut Pager, meta: &TableMeta) -> DbResult<()> {
    for column in &meta.columns {
        let Some(fk) = &column.references else {
            continue;
        };
        let is_self_ref = fk.table.eq_ignore_ascii_case(&meta.name);
        let (target_pk_name, target_pk_type, target_name) = if is_self_ref {
            let pk = meta.column(&meta.primary_key).ok_or_else(|| {
                DbError::new("FK self-ref: tabla sin PK definida (estado inconsistente)")
            })?;
            (meta.primary_key.clone(), pk.column_type, meta.name.clone())
        } else {
            let target = {
                let mut catalog = Catalog::open(pager);
                catalog.get_table(&fk.table)?.ok_or_else(|| {
                    coded(
                        codes::TABLE_NOT_FOUND,
                        format!(
                            "FOREIGN KEY '{}.{}' referencia tabla inexistente '{}'",
                            meta.name, column.name, fk.table
                        ),
                    )
                })?
            };
            let pk = target.column(&target.primary_key).ok_or_else(|| {
                DbError::new(format!(
                    "FK rota: tabla '{}' no expone su PK '{}'",
                    target.name, target.primary_key
                ))
            })?;
            (
                target.primary_key.clone(),
                pk.column_type,
                target.name.clone(),
            )
        };
        if !target_pk_name.eq_ignore_ascii_case(&fk.column) {
            return Err(DbError::new(format!(
                "FOREIGN KEY '{}.{}' debe referenciar la PK de '{}' (es '{}'); \
                 esta versión no admite REFERENCES contra columnas no-PK",
                meta.name, column.name, target_name, target_pk_name
            )));
        }
        if column.column_type != target_pk_type {
            return Err(DbError::new(format!(
                "FOREIGN KEY '{}.{}' debe ser {} para coincidir con la PK de '{}'",
                meta.name,
                column.name,
                target_pk_type.as_sql(),
                target_name
            )));
        }
    }
    Ok(())
}

/// Verify that the given `target_pk` exists in the FK's parent table.
/// `self_ref_allowed_pk` lets INSERT/UPDATE accept a self-FK that points
/// at the very row being written (the row will exist as soon as the
/// statement commits — refusing it would make self-managed entities
/// impossible to insert in the first place).
fn check_fk_value(
    pager: &mut Pager,
    meta: &TableMeta,
    column_name: &str,
    fk: &ForeignKeyMeta,
    target_pk: i64,
    self_ref_allowed_pk: i64,
) -> DbResult<()> {
    if fk.table.eq_ignore_ascii_case(&meta.name) && target_pk == self_ref_allowed_pk {
        return Ok(());
    }
    let parent_meta = {
        let mut catalog = Catalog::open(pager);
        catalog.get_table(&fk.table)?.ok_or_else(|| {
            DbError::new(format!(
                "FK rota: tabla '{}' no existe (referida por '{}.{}')",
                fk.table, meta.name, column_name
            ))
        })?
    };
    let exists = {
        let mut catalog = Catalog::open(pager);
        catalog.get_row(parent_meta.root_page, target_pk)?.is_some()
    };
    if !exists {
        return Err(coded(
            codes::FK_PARENT_MISSING,
            format!(
                "violación de FOREIGN KEY: '{}.{}' = {} no existe en la tabla padre '{}'",
                meta.name, column_name, target_pk, fk.table
            ),
        ));
    }
    Ok(())
}

/// Walk every column with a FK and call [`check_fk_value`] when the
/// final value (after defaults) is non-NULL. INSERT-time entry point.
fn enforce_fk_on_insert(
    pager: &mut Pager,
    meta: &TableMeta,
    values: &HashMap<String, Value>,
    new_pk: i64,
) -> DbResult<()> {
    for column in &meta.columns {
        let Some(fk) = &column.references else {
            continue;
        };
        let value = values
            .get(&normalize_ident(&column.name))
            .cloned()
            .unwrap_or(Value::Null);
        let Value::Integer(target_pk) = value else {
            continue;
        };
        check_fk_value(pager, meta, &column.name, fk, target_pk, new_pk)?;
    }
    Ok(())
}

/// UPDATE-time entry point. Same as INSERT but only revalidates FK
/// columns whose value actually changed — leaving an FK column
/// untouched can never break referential integrity.
fn enforce_fk_on_update(
    pager: &mut Pager,
    meta: &TableMeta,
    old_row: &HashMap<String, Value>,
    current: &HashMap<String, Value>,
    pk: i64,
) -> DbResult<()> {
    for column in &meta.columns {
        let Some(fk) = &column.references else {
            continue;
        };
        let normalized = normalize_ident(&column.name);
        let old_val = old_row.get(&normalized).cloned().unwrap_or(Value::Null);
        let new_val = current.get(&normalized).cloned().unwrap_or(Value::Null);
        if old_val == new_val {
            continue;
        }
        let Value::Integer(target_pk) = new_val else {
            continue;
        };
        check_fk_value(pager, meta, &column.name, fk, target_pk, pk)?;
    }
    Ok(())
}

/// Find every child PK whose FK column equals `parent_pk`. Uses the
/// secondary index on the child column when it exists; falls back to a
/// full scan otherwise (with the documented O(n) cost — see
/// `docs/SQL_REFERENCE.md`).
fn find_child_pks_with_fk_value(
    pager: &mut Pager,
    child_table: &TableMeta,
    fk_column: &str,
    parent_pk: i64,
) -> DbResult<Vec<i64>> {
    let column = child_table.column(fk_column).ok_or_else(|| {
        DbError::new(format!(
            "FK incoherente: columna '{}' no existe en '{}'",
            fk_column, child_table.name
        ))
    })?;
    let value = Value::Integer(parent_pk);
    let value_bytes = encode_column_value(column, &value)?;

    if let Some(idx) = child_table.index_for_column(fk_column) {
        let mut tree = Tree::new(pager);
        match idx.kind {
            IndexKind::Hash => {
                let key = hash_value(&value_bytes);
                let bucket_bytes = match tree.get(idx.root_page, key)? {
                    Some(b) => b,
                    None => return Ok(Vec::new()),
                };
                let bucket = decode_bucket(&bucket_bytes)?;
                return Ok(bucket_lookup(&bucket, &value_bytes));
            }
            IndexKind::OrderedInt => {
                let Some(key) = ordered_int_key_from_value_bytes(&value_bytes)? else {
                    return Ok(Vec::new());
                };
                let bucket_bytes = match tree.get(idx.root_page, key)? {
                    Some(b) => b,
                    None => return Ok(Vec::new()),
                };
                return decode_ordered_bucket(&bucket_bytes);
            }
        }
    }

    let mut catalog = Catalog::open(pager);
    let rows = catalog.scan_rows(child_table.root_page, 0, None)?;
    let mut hits = Vec::new();
    for kv in rows {
        let row = decode_row(child_table, &kv.value)?;
        if let Some(Value::Integer(n)) = row.get(&normalize_ident(fk_column)) {
            if *n == parent_pk {
                hits.push(kv.key);
            }
        }
    }
    Ok(hits)
}

/// DELETE one row from `root_table` and propagate to children according
/// to each child FK's `ON DELETE` action. Iterative worklist plus a
/// `visited` set on `(table, pk)` to short-circuit cycles in CASCADE
/// graphs (e.g. two tables that mutually reference each other).
///
/// All secondary-index maintenance is performed inline so the caller
/// (the executor) doesn't need to know which rows ended up disappearing.
fn delete_with_cascade(pager: &mut Pager, root_table: &str, root_pk: i64) -> DbResult<()> {
    use std::collections::VecDeque;

    // Snapshot the catalog once: cascading deletes mutate row data only,
    // never schema, so the snapshot stays valid for the whole walk.
    let snapshot = {
        let mut catalog = Catalog::open(pager);
        catalog.list_tables()?
    };
    let lookup_meta = |name: &str| -> Option<TableMeta> {
        snapshot
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .cloned()
    };

    let mut visited: HashSet<(String, i64)> = HashSet::new();
    let mut queue: VecDeque<(String, i64)> = VecDeque::new();
    visited.insert((root_table.to_ascii_lowercase(), root_pk));
    queue.push_back((root_table.to_string(), root_pk));

    while let Some((parent_name, parent_pk)) = queue.pop_front() {
        let parent_meta = match lookup_meta(&parent_name) {
            Some(m) => m,
            None => continue,
        };

        // 1. Resolve children before touching the parent row, so we can
        //    refuse the whole DELETE on RESTRICT without partial state.
        for child_table in &snapshot {
            for child_col in &child_table.columns {
                let Some(fk) = &child_col.references else {
                    continue;
                };
                if !fk.table.eq_ignore_ascii_case(&parent_name) {
                    continue;
                }
                let child_pks =
                    find_child_pks_with_fk_value(pager, child_table, &child_col.name, parent_pk)?;
                if child_pks.is_empty() {
                    continue;
                }
                match fk.on_delete {
                    OnDelete::Restrict => {
                        return Err(coded(
                            codes::FK_RESTRICT_BLOCKS_DELETE,
                            format!(
                                "DELETE FROM '{}' bloqueado: '{}.{}' referencia esta fila \
                                 con ON DELETE RESTRICT ({} fila(s) hijas afectarían)",
                                parent_name,
                                child_table.name,
                                child_col.name,
                                child_pks.len()
                            ),
                        ));
                    }
                    OnDelete::Cascade => {
                        for cpk in child_pks {
                            let key = (child_table.name.to_ascii_lowercase(), cpk);
                            if visited.insert(key) {
                                queue.push_back((child_table.name.clone(), cpk));
                            }
                        }
                    }
                }
            }
        }

        // 2. Delete the parent row from disk and from every secondary
        //    index it participated in. The row may have already vanished
        //    if a previous cascade step removed it (cycles, multi-path);
        //    treat that as a no-op.
        let row_bytes = {
            let mut catalog = Catalog::open(pager);
            catalog.get_row(parent_meta.root_page, parent_pk)?
        };
        let Some(bytes) = row_bytes else {
            continue;
        };
        let row = decode_row(&parent_meta, &bytes)?;
        for idx in &parent_meta.indexes {
            let column = parent_meta.column(&idx.column).ok_or_else(|| {
                DbError::new(format!(
                    "índice apunta a columna inexistente: {}",
                    idx.column
                ))
            })?;
            let value = row
                .get(&normalize_ident(&column.name))
                .cloned()
                .unwrap_or(Value::Null);
            let value_bytes = encode_column_value(column, &value)?;
            index_remove_pk(pager, idx.root_page, idx.kind, &value_bytes, parent_pk)?;
        }
        let mut catalog = Catalog::open(pager);
        catalog.delete_row(parent_meta.root_page, parent_pk)?;
    }
    Ok(())
}

/// Look the value up in a UNIQUE index bucket and translate any conflict
/// into a user-facing error. Caller passes `exclude_pk = Some(self_pk)`
/// during UPDATE so the row's pre-existing entry doesn't false-trigger.
pub(crate) fn check_unique_conflict(
    pager: &mut Pager,
    idx: &IndexMeta,
    value_bytes: &[u8],
    exclude_pk: Option<i64>,
) -> DbResult<()> {
    let mut tree = Tree::new(pager);
    let conflict_pk: Option<i64> = match idx.kind {
        IndexKind::Hash => {
            let key = hash_value(value_bytes);
            let bucket = match tree.get(idx.root_page, key)? {
                Some(bytes) => decode_bucket(&bytes)?,
                None => return Ok(()),
            };
            bucket_unique_conflict(&bucket, value_bytes, exclude_pk)
        }
        IndexKind::OrderedInt => {
            // NULLs are not tracked in OrderedInt indexes, so a NULL
            // pre-check is always a free pass (SQL allows many NULLs
            // even under UNIQUE).
            let Some(key) = ordered_int_key_from_value_bytes(value_bytes)? else {
                return Ok(());
            };
            let bucket = match tree.get(idx.root_page, key)? {
                Some(bytes) => decode_ordered_bucket(&bytes)?,
                None => return Ok(()),
            };
            ordered_bucket_unique_conflict(&bucket, exclude_pk)
        }
    };
    if let Some(other_pk) = conflict_pk {
        return Err(coded(
            codes::UNIQUE_VIOLATED,
            format!(
                "violación de UNIQUE en índice '{}' (PK existente: {})",
                idx.name, other_pk
            ),
        ));
    }
    Ok(())
}

/// Total ordering used by `ORDER BY`. NULLs sort first under ASC
/// (matching SQLite, opposite of PostgreSQL's NULLS LAST default — we
/// pick the simpler "low end" semantics so DESC mirrors as NULLs last
/// without a separate `NULLS LAST` clause). Mixed types shouldn't
/// happen in practice (a column has one declared type) but we still
/// return Equal rather than panicking when they do.
fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let av = a.unwrap_or(&Value::Null);
    let bv = b.unwrap_or(&Value::Null);
    match (av, bv) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        (Value::Integer(x), Value::Integer(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Integer(x), Value::Float(y)) => {
            (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (Value::Float(x), Value::Integer(y)) => {
            x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
        }
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

/// Scope de un SELECT con JOINs: la lista de tablas en orden left-deep
/// con su qualifier ya resuelto (alias si existe, nombre real si no).
/// Construido una sola vez por `build_join_scope`; consumido por la
/// resolución de columnas, el WHERE post-filter y el ORDER BY.
struct JoinScope {
    tables: Vec<JoinTable>,
    /// Claves `qualifier.col` que NO deben aparecer en `SELECT *` porque
    /// fueron "fusionadas" via USING/NATURAL — la columna del lado
    /// izquierdo ya cubre la del derecho (semántica ANSI: la columna
    /// común aparece una sola vez).
    hidden_in_star: HashSet<String>,
}

struct JoinTable {
    meta: TableMeta,
    /// Lower-case del alias (preferido) o nombre real. Es el prefix con
    /// el que viven las columnas en la HashMap de joined-rows.
    qualifier: String,
    /// El nombre real de la tabla. Se acepta como qualifier alternativo
    /// solo si la tabla NO tiene alias declarado (regla SQL standard).
    raw_name: String,
    alias: Option<String>,
}

/// Plan de ejecución index-loop para un JOIN específico. Solo se construye
/// cuando el predicate efectivo apunta a la PK o a una columna indexada
/// del right (es decir, podemos hacer lookup dirigido en vez de scan).
struct IndexLoopPlan {
    /// Clave canónica `qualifier.col` en la fila joineada actual (left)
    /// de donde sale el valor para hacer el lookup.
    left_key: String,
    /// Cómo buscar en el right: por PK directa o por índice secundario.
    right_strategy: RightLookup,
}

enum RightLookup {
    Pk,
    Index(IndexMeta),
}

/// Decide si el predicate puede ejecutarse como index-loop sobre el right.
/// Devuelve `Some(plan)` cuando una de las dos columnas del predicate
/// apunta a la PK o a una columna indexada del right (y la otra reside en
/// alguna tabla previa del scope). Devuelve `None` si no califica — el
/// caller cae al nested-loop.
fn plan_index_loop(
    scope: &JoinScope,
    join_idx: usize,
    pred: &JoinPredicate,
) -> DbResult<Option<IndexLoopPlan>> {
    let right = &scope.tables[join_idx + 1];
    let left_candidate = column_ref_to_raw(&pred.left);
    let right_candidate = column_ref_to_raw(&pred.right);
    // Tratamos las dos orientaciones del predicate: `left.col = right.col`
    // y `right.col = left.col`. Para que califique, una de las columnas
    // debe vivir en el right table y la otra en alguna tabla previa.
    for (right_side, left_side) in [
        (&right_candidate, &left_candidate),
        (&left_candidate, &right_candidate),
    ] {
        let (right_qual, right_col) = split_qualified_ident(right_side);
        let right_normalized = normalize_ident(&right_col);
        let right_qualifies = right_qual
            .as_deref()
            .map(|q| {
                q.eq_ignore_ascii_case(&right.qualifier)
                    || (right.alias.is_none() && q.eq_ignore_ascii_case(&right.raw_name))
            })
            .unwrap_or(false);
        if !right_qualifies {
            continue;
        }
        if right.meta.column(&right_normalized).is_none() {
            continue;
        }
        // El "left" del predicate debe poder resolverse en cualquier
        // tabla previa al join_idx (osea no en el right). Reusamos
        // resolve_joined_column_key sobre un sub-scope sin el right.
        let sub_scope = JoinScope {
            tables: scope
                .tables
                .iter()
                .enumerate()
                .filter(|(i, _)| *i <= join_idx)
                .map(|(_, t)| JoinTable {
                    meta: t.meta.clone(),
                    qualifier: t.qualifier.clone(),
                    raw_name: t.raw_name.clone(),
                    alias: t.alias.clone(),
                })
                .collect(),
            hidden_in_star: HashSet::new(),
        };
        let left_key = match resolve_joined_column_key(&sub_scope, left_side) {
            Ok(k) => k,
            Err(_) => continue,
        };
        // Strategy: PK directa si el target es la PK del right; índice
        // secundario si existe; ninguna si la columna no es indexable.
        let strategy = if right
            .meta
            .primary_key
            .eq_ignore_ascii_case(&right_normalized)
        {
            RightLookup::Pk
        } else if let Some(idx) = right.meta.index_for_column(&right_normalized).cloned() {
            RightLookup::Index(idx)
        } else {
            continue;
        };
        return Ok(Some(IndexLoopPlan {
            left_key,
            right_strategy: strategy,
        }));
    }
    Ok(None)
}

/// Resultado de derivar el predicate efectivo de un JOIN. `predicate` es
/// `Some` cuando proviene de USING/NATURAL (el ON explícito se evalúa por
/// el path normal). `hidden_keys` son las claves canónicas del lado right
/// que `SELECT *` debe omitir para no duplicar la columna fusionada.
struct DerivedJoin {
    predicate: Option<JoinPredicate>,
    hidden_keys: Vec<String>,
}

fn derive_join_predicate(
    scope: &JoinScope,
    join_idx: usize,
    join: &JoinClause,
) -> DbResult<DerivedJoin> {
    let right = &scope.tables[join_idx + 1];
    if join.natural {
        // Buscar columnas comunes entre el right y cualquier tabla previa
        // (en chains multi-tabla `a NATURAL JOIN b NATURAL JOIN c`, la
        // columna común puede estar en `a` o en `b` para el segundo NATURAL).
        let right_cols: Vec<&str> = right.meta.columns.iter().map(|c| c.name.as_str()).collect();
        let mut commons: Vec<(String, String)> = Vec::new(); // (left_qualifier, col_name)
        for prev in &scope.tables[..=join_idx] {
            for prev_col in &prev.meta.columns {
                let matches_right = right_cols
                    .iter()
                    .any(|r| r.eq_ignore_ascii_case(&prev_col.name));
                let already_picked = commons
                    .iter()
                    .any(|(_, c)| c.eq_ignore_ascii_case(&prev_col.name));
                if matches_right && !already_picked {
                    commons.push((prev.qualifier.clone(), prev_col.name.clone()));
                }
            }
        }
        if commons.len() != 1 {
            return Err(coded(
                codes::NATURAL_JOIN_NO_COMMON_COLUMN,
                format!(
                    "NATURAL JOIN sobre '{}' espera exactamente 1 columna común; \
                     se detectaron {} (este release soporta single-column NATURAL)",
                    right.raw_name,
                    commons.len()
                ),
            ));
        }
        let (left_qual, col) = &commons[0];
        let pred = JoinPredicate {
            left: ColumnRef {
                qualifier: Some(left_qual.clone()),
                name: col.clone(),
            },
            right: ColumnRef {
                qualifier: Some(right.qualifier.clone()),
                name: col.clone(),
            },
        };
        let hidden = vec![format!("{}.{}", right.qualifier, normalize_ident(col))];
        return Ok(DerivedJoin {
            predicate: Some(pred),
            hidden_keys: hidden,
        });
    }
    if let Some(using_cols) = &join.using {
        if using_cols.len() != 1 {
            return Err(coded(
                codes::USING_COLUMN_INVALID,
                format!(
                    "USING con {} columnas; este release acepta exactamente 1 columna en USING",
                    using_cols.len()
                ),
            ));
        }
        let col = &using_cols[0];
        let normalized = normalize_ident(col);
        let right_has = right.meta.column(&normalized).is_some();
        let left_match = scope.tables[..=join_idx]
            .iter()
            .find(|t| t.meta.column(&normalized).is_some());
        if !right_has || left_match.is_none() {
            return Err(coded(
                codes::USING_COLUMN_INVALID,
                format!(
                    "USING ({}) requiere que '{}' exista en ambos lados del JOIN",
                    col, col
                ),
            ));
        }
        let left_qual = left_match.unwrap().qualifier.clone();
        let pred = JoinPredicate {
            left: ColumnRef {
                qualifier: Some(left_qual),
                name: col.clone(),
            },
            right: ColumnRef {
                qualifier: Some(right.qualifier.clone()),
                name: col.clone(),
            },
        };
        let hidden = vec![format!("{}.{}", right.qualifier, normalized)];
        return Ok(DerivedJoin {
            predicate: Some(pred),
            hidden_keys: hidden,
        });
    }
    Ok(DerivedJoin {
        predicate: None,
        hidden_keys: Vec::new(),
    })
}

/// Convierte una columna del SELECT (`*`, `col` o `tabla.col`) en el par
/// `(output_label, lookup_key)` que necesitamos para proyectar. `*` se
/// expande a TODAS las columnas de TODAS las tablas, en orden.
fn resolve_joined_projection(
    scope: &JoinScope,
    requested: &[String],
) -> DbResult<(Vec<String>, Vec<String>)> {
    let mut output = Vec::new();
    let mut keys = Vec::new();
    if requested.is_empty() {
        // `SELECT *` → todas las columnas de todas las tablas, prefijadas
        // por qualifier. Omite las que quedaron "hidden" por USING/NATURAL
        // (ANSI: la columna común aparece una sola vez).
        for t in &scope.tables {
            for col in &t.meta.columns {
                let key = format!("{}.{}", t.qualifier, normalize_ident(&col.name));
                if scope.hidden_in_star.contains(&key) {
                    continue;
                }
                output.push(format!("{}.{}", t.qualifier, col.name));
                keys.push(key);
            }
        }
        return Ok((output, keys));
    }
    for raw in requested {
        output.push(raw.clone());
        keys.push(resolve_joined_column_key(scope, raw)?);
    }
    Ok((output, keys))
}

/// Toma una referencia de columna como string raw (`col` o `tabla.col`)
/// y devuelve la clave canónica en `qualifier.normalized_col` para buscar
/// en la HashMap de joined-rows.
fn resolve_joined_column_key(scope: &JoinScope, raw: &str) -> DbResult<String> {
    let (qualifier, name) = split_qualified_ident(raw);
    let normalized = normalize_ident(&name);
    if let Some(q) = qualifier {
        let q_lc = q.to_ascii_lowercase();
        // Match contra alias preferido; nombre real solo si la tabla
        // NO tiene alias (SQL standard: alias hides original name).
        let table = scope.tables.iter().find(|t| {
            t.qualifier == q_lc || (t.alias.is_none() && t.raw_name.eq_ignore_ascii_case(&q))
        });
        let table = table.ok_or_else(|| {
            coded(
                codes::COLUMN_QUALIFIER_NOT_FOUND,
                format!(
                    "qualifier '{}' no coincide con ninguna tabla/alias del FROM",
                    q
                ),
            )
        })?;
        if table.meta.column(&normalized).is_none() {
            return Err(coded(
                codes::COLUMN_QUALIFIER_NOT_FOUND,
                format!(
                    "columna '{}' no existe en la tabla '{}'",
                    name, table.raw_name
                ),
            ));
        }
        Ok(format!("{}.{}", table.qualifier, normalized))
    } else {
        // Sin qualifier: buscar en todas las tablas. Si está en más de
        // una → ambiguous (error 4018).
        let matches: Vec<&JoinTable> = scope
            .tables
            .iter()
            .filter(|t| t.meta.column(&normalized).is_some())
            .collect();
        if matches.is_empty() {
            return Err(coded(
                codes::COLUMN_QUALIFIER_NOT_FOUND,
                format!("columna '{}' no existe en ninguna tabla del FROM", name),
            ));
        }
        if matches.len() > 1 {
            let tables: Vec<&str> = matches.iter().map(|t| t.qualifier.as_str()).collect();
            return Err(coded(
                codes::COLUMN_AMBIGUOUS,
                format!(
                    "columna '{}' es ambigua: existe en {} — usá tabla.col para des-ambiguar",
                    name,
                    tables.join(", ")
                ),
            ));
        }
        Ok(format!("{}.{}", matches[0].qualifier, normalized))
    }
}

/// Evalúa un equi-predicado `l.col = r.col` sobre dos sub-rows. Las
/// columnas se resuelven contra el scope completo (las dos pueden vivir
/// en cualquier tabla ya joineada o en el nuevo right).
fn evaluate_join_predicate(
    left_row: &HashMap<String, Value>,
    right_row: &HashMap<String, Value>,
    pred: &JoinPredicate,
    scope: &JoinScope,
) -> DbResult<bool> {
    let lkey = resolve_joined_column_key(scope, &column_ref_to_raw(&pred.left))?;
    let rkey = resolve_joined_column_key(scope, &column_ref_to_raw(&pred.right))?;
    let lv = left_row.get(&lkey).or_else(|| right_row.get(&lkey));
    let rv = right_row.get(&rkey).or_else(|| left_row.get(&rkey));
    match (lv, rv) {
        (Some(a), Some(b)) => Ok(values_equal(a, b)),
        _ => Ok(false),
    }
}

fn column_ref_to_raw(cref: &ColumnRef) -> String {
    match &cref.qualifier {
        Some(q) => format!("{}.{}", q, cref.name),
        None => cref.name.clone(),
    }
}

/// Igualdad estricta para WHERE post-filter en JOINs. NULL != NULL (SQL
/// standard). Mismo tipo a tipo. INT vs FLOAT promueve a FLOAT.
/// Bloque E2: evalúa `lhs <op> rhs` con 3VL. NULL en cualquiera de los dos
/// lados → `None` (unknown). Tipos compatibles: INT↔INT, FLOAT↔FLOAT,
/// INT↔FLOAT (promoción), TEXT↔TEXT, BOOL↔BOOL. Cualquier otra combinación
/// devuelve `Some(false)` (no son comparables → no matchean).
fn eval_compare(lhs: Option<&Value>, op: CompareOp, rhs: &Value) -> Option<bool> {
    let lhs = lhs?;
    if matches!(lhs, Value::Null) || matches!(rhs, Value::Null) {
        return None;
    }
    use std::cmp::Ordering;
    let ord: Option<Ordering> = match (lhs, rhs) {
        (Value::Integer(a), Value::Integer(b)) => Some(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (Value::Integer(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
        (Value::Float(a), Value::Integer(b)) => a.partial_cmp(&(*b as f64)),
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        _ => None,
    };
    let ord = match ord {
        Some(o) => o,
        // Tipos incompatibles (ej. TEXT vs INT) no son comparables; ANSI
        // strictamente devolvería error de tipo, pero gabysql elige
        // `false` (la fila no matchea) para no abortar consultas mixtas.
        None => return Some(false),
    };
    Some(match op {
        CompareOp::Lt => ord == Ordering::Less,
        CompareOp::Le => ord != Ordering::Greater,
        CompareOp::Gt => ord == Ordering::Greater,
        CompareOp::Ge => ord != Ordering::Less,
        CompareOp::Ne => ord != Ordering::Equal,
    })
}

/// Bloque E2: evalúa `lhs [NOT] LIKE patron`. Solo aplica sobre TEXT;
/// cualquier otro tipo (incluido NULL) → `None`. Wildcards SQL estándar:
/// `%` = cero o más chars, `_` = exactamente uno. Escape con `\%` / `\_`
/// (los demás `\X` se interpretan literales). Implementación recursiva
/// con memoization implícita por backtracking acotado al patrón.
fn eval_like(lhs: Option<&Value>, pattern: &str, negated: bool) -> Option<bool> {
    let s = match lhs? {
        Value::String(s) => s.as_str(),
        Value::Null => return None,
        _ => return Some(false),
    };
    let m = like_match(s, pattern);
    Some(if negated { !m } else { m })
}

/// Backtracking simple `s` vs `pattern` con wildcards `%` / `_`. La
/// recursión es O(|s|·|pattern|) en el peor caso; alcanza para patrones
/// realistas. Para patrones gigantes con muchos `%` un NFA sería mejor —
/// queda para optimización futura.
fn like_match(s: &str, pattern: &str) -> bool {
    let s_chars: Vec<char> = s.chars().collect();
    let p_chars: Vec<char> = pattern.chars().collect();
    fn go(s: &[char], p: &[char]) -> bool {
        if p.is_empty() {
            return s.is_empty();
        }
        match p[0] {
            '%' => {
                // Match cero o más caracteres. Probamos sin consumir nada,
                // y si falla consumimos uno y seguimos.
                if go(s, &p[1..]) {
                    return true;
                }
                if !s.is_empty() && go(&s[1..], p) {
                    return true;
                }
                false
            }
            '_' => !s.is_empty() && go(&s[1..], &p[1..]),
            '\\' if p.len() >= 2 => {
                // Escape: el siguiente char se matchea literal (sin tratarlo
                // como wildcard). Útil para buscar `%` o `_` literales.
                !s.is_empty() && s[0] == p[1] && go(&s[1..], &p[2..])
            }
            c => !s.is_empty() && s[0] == c && go(&s[1..], &p[1..]),
        }
    }
    go(&s_chars, &p_chars)
}

/// Bloque E2: evalúa `lhs [NOT] IN (v1, v2, ...)` con semántica ANSI 3VL.
///
/// - `lhs IS NULL` → `NULL` (unknown), independiente de la lista.
/// - `lhs IN (lista)` → `true` si algún `v_i` matchea por igualdad;
///   `NULL` si no hubo match y la lista contiene algún NULL;
///   `false` si no hubo match y la lista no tiene NULLs.
/// - `lhs NOT IN (lista)` = `NOT (lhs IN (lista))` con la misma 3VL: si
///   la lista contiene NULL y no hubo match, el resultado es NULL.
fn eval_in_list(lhs: Option<&Value>, values: &[Value], negated: bool) -> Option<bool> {
    let lhs = lhs?;
    if matches!(lhs, Value::Null) {
        return None;
    }
    let mut had_null = false;
    let mut matched = false;
    for v in values {
        if matches!(v, Value::Null) {
            had_null = true;
            continue;
        }
        if values_equal(lhs, v) {
            matched = true;
            break;
        }
    }
    let in_result = if matched {
        Some(true)
    } else if had_null {
        None
    } else {
        Some(false)
    };
    match in_result {
        Some(b) => Some(if negated { !b } else { b }),
        None => None,
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Integer(x), Value::Integer(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Integer(x), Value::Float(y)) | (Value::Float(y), Value::Integer(x)) => {
            (*x as f64) == *y
        }
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        _ => false,
    }
}

/// Walk the WHERE tree looking for `EqColumnRef` — the marker that this
/// subquery references the outer scope and therefore must be re-executed
/// per outer row. We descend through nested subqueries (IN, =, EXISTS) so
/// a column ref two levels deep still flags the parent as correlated.
fn subquery_has_outer_refs(stmt: &SelectStmt) -> bool {
    match &stmt.where_clause {
        Some(expr) => where_expr_has_outer_refs(expr),
        None => false,
    }
}

/// Walk de la expresión booleana del WHERE buscando referencias outer
/// (`EqColumnRef`) en cualquier nivel, incluyendo dentro de subqueries
/// anidadas. Análogo de `subquery_has_outer_refs` para el árbol `WhereExpr`
/// — la presencia de un solo `EqColumnRef` en cualquier hoja marca toda la
/// subquery como correlacionada.
fn where_expr_has_outer_refs(expr: &WhereExpr) -> bool {
    match expr {
        WhereExpr::And(a, b) | WhereExpr::Or(a, b) => {
            where_expr_has_outer_refs(a) || where_expr_has_outer_refs(b)
        }
        WhereExpr::Not(inner) => where_expr_has_outer_refs(inner),
        WhereExpr::Atom(c) => match c {
            WhereClause::EqColumnRef { .. } => true,
            WhereClause::Exists { subquery, .. }
            | WhereClause::In { subquery, .. }
            | WhereClause::EqSubquery { subquery, .. } => subquery_has_outer_refs(subquery),
            // Átomos E2 no llevan subqueries ni referencias outer.
            WhereClause::Eq { .. }
            | WhereClause::Between { .. }
            | WhereClause::Compare { .. }
            | WhereClause::Like { .. }
            | WhereClause::IsNull { .. }
            | WhereClause::InList { .. } => false,
        },
    }
}

fn normalize_ident(value: &str) -> String {
    value
        .rsplit('.')
        .next()
        .unwrap_or(value)
        .trim()
        .to_ascii_lowercase()
}

/// Keywords that pueden aparecer inmediatamente después del nombre (o
/// alias) de una tabla. Si vemos uno de estos, NO es un alias — es la
/// continuación natural del SELECT. Esto evita que `FROM t WHERE ...`
/// engulla `WHERE` como alias de `t`.
fn is_post_table_keyword(text: &str) -> bool {
    matches!(
        text.to_ascii_uppercase().as_str(),
        "WHERE"
            | "ORDER"
            | "LIMIT"
            | "OFFSET"
            | "INNER"
            | "CROSS"
            | "JOIN"
            | "LEFT"
            | "RIGHT"
            | "FULL"
            | "NATURAL"
            | "OUTER"
            | "ON"
            | "USING"
            | "GROUP"
            | "HAVING"
            | "AS"
    )
}

/// Returns `true` when the identifier is actually one of the value-keywords
/// that `expect_value` resolves (`TRUE`, `FALSE`, `NULL`). Used by the
/// WHERE parser to decide if `col = <ident>` is a column reference or a
/// boolean/null literal.
fn is_value_keyword(text: &str) -> bool {
    matches!(
        text.to_ascii_uppercase().as_str(),
        "TRUE" | "FALSE" | "NULL"
    )
}

/// Splits a possibly-qualified identifier like `outer.col` into
/// `(Some("outer"), "col")`. Bare identifiers like `col` become
/// `(None, "col")`. Multi-dot identifiers keep only the LAST segment as
/// the column name and the segment before it as the table.
fn split_qualified_ident(raw: &str) -> (Option<String>, String) {
    match raw.rsplit_once('.') {
        Some((prefix, name)) => {
            let table = prefix.rsplit('.').next().unwrap_or(prefix).trim();
            (Some(table.to_string()), name.trim().to_string())
        }
        None => (None, raw.trim().to_string()),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Ident,
    Number,
    String,
    Symbol,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
struct Token {
    kind: TokenKind,
    text: String,
}

fn tokenize(input: &str) -> DbResult<Vec<Token>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        if is_ident_start(ch) {
            let start = index;
            index += 1;
            while index < chars.len() && is_ident_part(chars[index]) {
                index += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Ident,
                text: chars[start..index].iter().collect(),
            });
            continue;
        }
        if ch.is_ascii_digit()
            || (ch == '-' && index + 1 < chars.len() && chars[index + 1].is_ascii_digit())
        {
            let start = index;
            index += 1;
            while index < chars.len() && chars[index].is_ascii_digit() {
                index += 1;
            }
            if index < chars.len() && chars[index] == '.' {
                let dot = index;
                index += 1;
                let decimals_start = index;
                while index < chars.len() && chars[index].is_ascii_digit() {
                    index += 1;
                }
                if decimals_start == index {
                    index = dot;
                }
            }
            tokens.push(Token {
                kind: TokenKind::Number,
                text: chars[start..index].iter().collect(),
            });
            continue;
        }
        if ch == '\'' {
            index += 1;
            let mut value = String::new();
            while index < chars.len() {
                if chars[index] == '\'' {
                    if index + 1 < chars.len() && chars[index + 1] == '\'' {
                        value.push('\'');
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                value.push(chars[index]);
                index += 1;
            }
            if index > chars.len() {
                return Err(coded(
                    codes::STRING_LITERAL_UNTERMINATED,
                    format!(
                        "literal string sin cerrar: comillas simples no balanceadas (texto inicial: '{}')",
                        value.chars().take(40).collect::<String>()
                    ),
                ));
            }
            tokens.push(Token {
                kind: TokenKind::String,
                text: value,
            });
            continue;
        }
        match ch {
            '(' | ')' | ',' | '*' | '=' => {
                tokens.push(Token {
                    kind: TokenKind::Symbol,
                    text: ch.to_string(),
                });
                index += 1;
            }
            // Bloque E2: operadores de comparación. Reconocemos primero los
            // bi-carácter (`<=`, `>=`, `<>`, `!=`) y luego los mono (`<`, `>`).
            // `!` solo es válido como prefijo de `!=`; suelto es un error
            // explícito para que el usuario no confunda con NOT.
            '<' => {
                if index + 1 < chars.len() && chars[index + 1] == '=' {
                    tokens.push(Token {
                        kind: TokenKind::Symbol,
                        text: "<=".to_string(),
                    });
                    index += 2;
                } else if index + 1 < chars.len() && chars[index + 1] == '>' {
                    tokens.push(Token {
                        kind: TokenKind::Symbol,
                        text: "<>".to_string(),
                    });
                    index += 2;
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Symbol,
                        text: "<".to_string(),
                    });
                    index += 1;
                }
            }
            '>' => {
                if index + 1 < chars.len() && chars[index + 1] == '=' {
                    tokens.push(Token {
                        kind: TokenKind::Symbol,
                        text: ">=".to_string(),
                    });
                    index += 2;
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Symbol,
                        text: ">".to_string(),
                    });
                    index += 1;
                }
            }
            '!' => {
                if index + 1 < chars.len() && chars[index + 1] == '=' {
                    tokens.push(Token {
                        kind: TokenKind::Symbol,
                        text: "!=".to_string(),
                    });
                    index += 2;
                } else {
                    return Err(DbError::new(format!(
                        "símbolo no soportado: '!' suelto; ¿quisiste decir '!='?"
                    )));
                }
            }
            _ => return Err(DbError::new(format!("carÃ¡cter no soportado: {}", ch))),
        }
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        text: String::new(),
    });
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn parse_statement(&mut self) -> DbResult<Statement> {
        if self.match_keyword("CREATE") {
            return self.parse_create();
        }
        if self.match_keyword("INSERT") {
            return self.parse_insert();
        }
        if self.match_keyword("SELECT") {
            let stmt = self.parse_select_stmt()?;
            return Ok(Statement::Select(stmt));
        }
        if self.match_keyword("UPDATE") {
            return self.parse_update();
        }
        if self.match_keyword("DELETE") {
            return self.parse_delete();
        }
        if self.match_keyword("DROP") {
            return self.parse_drop();
        }
        if self.match_keyword("ALTER") {
            return self.parse_alter();
        }
        if self.match_keyword("SHOW") {
            return self.parse_show();
        }
        if self.match_keyword("INTEGRITY") {
            self.expect_keyword("CHECK")?;
            return Ok(Statement::IntegrityCheck);
        }
        Err(DbError::new(
            "sentencia no soportada (solo CREATE/INSERT/SELECT/UPDATE/DELETE/DROP/ALTER/SHOW/INTEGRITY)",
        ))
    }

    fn parse_update(&mut self) -> DbResult<Statement> {
        let table = self.expect_ident()?;
        self.expect_keyword("SET")?;
        let mut assignments = Vec::new();
        loop {
            let column = self.expect_ident()?;
            self.expect_symbol("=")?;
            let value = self.expect_value()?;
            assignments.push((column, value));
            if !self.match_symbol(",") {
                break;
            }
        }
        self.expect_keyword("WHERE")?;
        let where_column = self.expect_ident()?;
        self.expect_symbol("=")?;
        let where_pk = self.expect_integer()?;
        Ok(Statement::Update(UpdateStmt {
            table,
            assignments,
            where_column,
            where_pk,
        }))
    }

    fn parse_delete(&mut self) -> DbResult<Statement> {
        self.expect_keyword("FROM")?;
        let table = self.expect_ident()?;
        self.expect_keyword("WHERE")?;
        let where_column = self.expect_ident()?;
        self.expect_symbol("=")?;
        let where_pk = self.expect_integer()?;
        Ok(Statement::Delete(DeleteStmt {
            table,
            where_column,
            where_pk,
        }))
    }

    fn parse_create_index(&mut self, unique: bool) -> DbResult<Statement> {
        let name = self.expect_ident()?;
        self.expect_keyword("ON")?;
        let table = self.expect_ident()?;
        self.expect_symbol("(")?;
        let column = self.expect_ident()?;
        self.expect_symbol(")")?;
        Ok(Statement::CreateIndex(CreateIndexStmt {
            name,
            table,
            column,
            unique,
        }))
    }

    fn parse_drop(&mut self) -> DbResult<Statement> {
        if self.match_keyword("DATABASE") {
            let if_exists = self.parse_if_exists()?;
            let name = self.expect_ident()?;
            return Ok(Statement::DropDatabase(DropDatabaseStmt {
                name,
                if_exists,
            }));
        }
        if self.match_keyword("TABLE") {
            let if_exists = self.parse_if_exists()?;
            let name = self.expect_ident()?;
            return Ok(Statement::DropTable(DropTableStmt { name, if_exists }));
        }
        self.expect_keyword("INDEX")?;
        let name = self.expect_ident()?;
        Ok(Statement::DropIndex(DropIndexStmt { name }))
    }

    fn parse_if_exists(&mut self) -> DbResult<bool> {
        if self.match_keyword("IF") {
            self.expect_keyword("EXISTS")?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn parse_alter(&mut self) -> DbResult<Statement> {
        self.expect_keyword("TABLE")?;
        let table = self.expect_ident()?;
        self.expect_keyword("ADD")?;
        // The COLUMN keyword is optional, matching most other dialects.
        let _ = self.match_keyword("COLUMN");
        let column = self.parse_column_def()?;
        Ok(Statement::AlterTableAddColumn(AlterAddColumnStmt {
            table,
            column,
        }))
    }

    /// Shared between `CREATE TABLE` and `ALTER TABLE ADD COLUMN`. Reads
    /// `name type column_constraint*` and returns the parser-level
    /// `ColumnDef`. The constraint loop is intentionally permissive about
    /// order — semantic validation (e.g. "DEFAULT NULL incompatible con
    /// NOT NULL") happens later in `validate_create_table`.
    fn parse_column_def(&mut self) -> DbResult<ColumnDef> {
        let name = self.expect_ident()?;
        let type_name = self.expect_ident()?;
        let mut primary_key = false;
        let mut not_null = false;
        let mut unique = false;
        let mut default: Option<Value> = None;
        let mut references: Option<ForeignKeyDef> = None;
        loop {
            if self.match_keyword("PRIMARY") {
                self.expect_keyword("KEY")?;
                primary_key = true;
            } else if self.match_keyword("NOT") {
                self.expect_keyword("NULL")?;
                not_null = true;
            } else if self.match_keyword("UNIQUE") {
                unique = true;
            } else if self.match_keyword("DEFAULT") {
                if default.is_some() {
                    return Err(DbError::new(format!(
                        "DEFAULT duplicado en columna '{}'",
                        name
                    )));
                }
                default = Some(self.expect_value()?);
            } else if self.match_keyword("REFERENCES") {
                if references.is_some() {
                    return Err(DbError::new(format!(
                        "REFERENCES duplicado en columna '{}'",
                        name
                    )));
                }
                let target_table = self.expect_ident()?;
                self.expect_symbol("(")?;
                let target_column = self.expect_ident()?;
                self.expect_symbol(")")?;
                let on_delete = self.parse_on_delete()?;
                references = Some(ForeignKeyDef {
                    table: target_table,
                    column: target_column,
                    on_delete,
                });
            } else {
                break;
            }
        }
        Ok(ColumnDef {
            name,
            type_name,
            primary_key,
            not_null,
            unique,
            default,
            references,
        })
    }

    /// Optional `ON DELETE RESTRICT|CASCADE` tail of a `REFERENCES`
    /// clause. Defaults to `RESTRICT` when omitted — that matches
    /// PostgreSQL's implicit behaviour and is the safest choice (refuses
    /// the parent DELETE rather than silently dropping children).
    fn parse_on_delete(&mut self) -> DbResult<OnDelete> {
        if !self.match_keyword("ON") {
            return Ok(OnDelete::Restrict);
        }
        self.expect_keyword("DELETE")?;
        if self.match_keyword("CASCADE") {
            Ok(OnDelete::Cascade)
        } else if self.match_keyword("RESTRICT") {
            Ok(OnDelete::Restrict)
        } else {
            Err(DbError::new(
                "ON DELETE solo admite RESTRICT o CASCADE en esta versión",
            ))
        }
    }

    fn parse_create_database(&mut self) -> DbResult<Statement> {
        let if_not_exists = if self.match_keyword("IF") {
            self.expect_keyword("NOT")?;
            self.expect_keyword("EXISTS")?;
            true
        } else {
            false
        };
        let name = self.expect_ident()?;
        Ok(Statement::CreateDatabase(CreateDatabaseStmt {
            name,
            if_not_exists,
        }))
    }

    fn parse_show(&mut self) -> DbResult<Statement> {
        self.expect_keyword("DATABASES")?;
        Ok(Statement::ShowDatabases)
    }

    fn parse_create(&mut self) -> DbResult<Statement> {
        if self.match_keyword("UNIQUE") {
            self.expect_keyword("INDEX")?;
            return self.parse_create_index(true);
        }
        if self.match_keyword("INDEX") {
            return self.parse_create_index(false);
        }
        if self.match_keyword("DATABASE") {
            return self.parse_create_database();
        }
        self.expect_keyword("TABLE")?;
        let name = self.expect_ident()?;
        self.expect_symbol("(")?;
        let mut columns = Vec::new();
        let mut primary_key = String::new();
        loop {
            let column = self.parse_column_def()?;
            if column.primary_key {
                primary_key = column.name.clone();
            }
            columns.push(column);
            if self.match_symbol(")") {
                break;
            }
            self.expect_symbol(",")?;
        }
        Ok(Statement::CreateTable(CreateTableStmt {
            name,
            columns,
            primary_key,
        }))
    }

    fn parse_insert(&mut self) -> DbResult<Statement> {
        self.expect_keyword("INTO")?;
        let table = self.expect_ident()?;
        self.expect_symbol("(")?;
        let columns = self.parse_ident_list()?;
        self.expect_symbol(")")?;
        self.expect_keyword("VALUES")?;
        self.expect_symbol("(")?;
        let values = self.parse_value_list()?;
        self.expect_symbol(")")?;
        Ok(Statement::Insert(InsertStmt {
            table,
            columns,
            values,
        }))
    }

    /// Parsea `[AS] <ident>` como alias opcional. Devuelve `None` si el
    /// próximo token NO es un alias válido (ej. otra keyword reservada de
    /// la sentencia o final del FROM).
    fn try_parse_alias(&mut self) -> DbResult<Option<String>> {
        // `AS` es opcional. Si está, después tiene que venir un ident.
        if self.match_keyword("AS") {
            let alias = self.expect_ident()?;
            return Ok(Some(alias));
        }
        // Sin `AS`: el alias es opcional. Para no engullir keywords de la
        // continuación del SELECT, sólo lo agarramos si el peek es un
        // Ident y no coincide con una keyword conocida en este punto.
        if self.peek().kind == TokenKind::Ident && !is_post_table_keyword(&self.peek().text) {
            let alias = self.expect_ident()?;
            return Ok(Some(alias));
        }
        Ok(None)
    }

    fn parse_join_predicate(&mut self) -> DbResult<JoinPredicate> {
        let left = self.parse_column_ref()?;
        self.expect_symbol("=")?;
        let right = self.parse_column_ref()?;
        Ok(JoinPredicate { left, right })
    }

    fn parse_column_ref(&mut self) -> DbResult<ColumnRef> {
        let raw = self.expect_ident()?;
        let (qualifier, name) = split_qualified_ident(&raw);
        Ok(ColumnRef { qualifier, name })
    }

    fn parse_select_stmt(&mut self) -> DbResult<SelectStmt> {
        let columns = if self.match_symbol("*") {
            Vec::new()
        } else {
            self.parse_ident_list()?
        };
        self.expect_keyword("FROM")?;
        // Base table + alias opcional (`AS` aceptado pero opcional).
        let table = self.expect_ident()?;
        let table_alias = self.try_parse_alias()?;

        // Cero o más JOINs en cadena. Aceptamos tres formas:
        //   - `, b` (comma-syntax) → CROSS JOIN sin ON
        //   - `CROSS JOIN b`        → CROSS JOIN sin ON (error si lleva ON)
        //   - `[INNER] JOIN b ON l = r` → INNER JOIN equi-predicado
        let mut joins: Vec<JoinClause> = Vec::new();
        loop {
            // `NATURAL` puede preceder a INNER/LEFT/RIGHT/FULL/JOIN (no a CROSS).
            let natural = self.match_keyword("NATURAL");
            let (kind, parsed) = if !natural && self.match_symbol(",") {
                (JoinKind::Cross, true)
            } else if !natural && self.match_keyword("CROSS") {
                self.expect_keyword("JOIN")?;
                (JoinKind::Cross, true)
            } else if self.match_keyword("INNER") {
                self.expect_keyword("JOIN")?;
                (JoinKind::Inner, true)
            } else if self.match_keyword("LEFT") {
                let _ = self.match_keyword("OUTER");
                self.expect_keyword("JOIN")?;
                (JoinKind::Left, true)
            } else if self.match_keyword("RIGHT") {
                let _ = self.match_keyword("OUTER");
                self.expect_keyword("JOIN")?;
                (JoinKind::Right, true)
            } else if self.match_keyword("FULL") {
                let _ = self.match_keyword("OUTER");
                self.expect_keyword("JOIN")?;
                (JoinKind::Full, true)
            } else if self.match_keyword("JOIN") {
                (JoinKind::Inner, true)
            } else {
                if natural {
                    return Err(coded(
                        codes::JOIN_PREDICATE_REQUIRED,
                        "NATURAL debe ir seguido por JOIN (o LEFT/RIGHT/FULL/INNER JOIN)",
                    ));
                }
                (JoinKind::Inner, false)
            };
            if !parsed {
                break;
            }
            let right_name = self.expect_ident()?;
            let right_alias = self.try_parse_alias()?;
            let right = TableRef {
                name: right_name,
                alias: right_alias,
            };
            // Resolución de la cláusula del JOIN — pueden venir ON, USING
            // o nada (CROSS o NATURAL). Mutuamente excluyentes.
            let mut on: Option<JoinPredicate> = None;
            let mut using: Option<Vec<String>> = None;
            if self.match_keyword("ON") {
                if matches!(kind, JoinKind::Cross) {
                    return Err(coded(
                        codes::CROSS_JOIN_WITH_ON,
                        "CROSS JOIN no admite ON; usá INNER JOIN si necesitás predicado",
                    ));
                }
                if natural {
                    return Err(coded(
                        codes::CROSS_JOIN_WITH_ON,
                        "NATURAL JOIN ya implica el predicado — no se puede combinar con ON",
                    ));
                }
                on = Some(self.parse_join_predicate()?);
            } else if self.match_keyword("USING") {
                if matches!(kind, JoinKind::Cross) {
                    return Err(coded(
                        codes::CROSS_JOIN_WITH_ON,
                        "CROSS JOIN no admite USING; usá INNER JOIN si necesitás predicado",
                    ));
                }
                if natural {
                    return Err(coded(
                        codes::CROSS_JOIN_WITH_ON,
                        "NATURAL JOIN ya implica el predicado — no se puede combinar con USING",
                    ));
                }
                self.expect_symbol("(")?;
                let mut cols = vec![self.expect_ident()?];
                while self.match_symbol(",") {
                    cols.push(self.expect_ident()?);
                }
                self.expect_symbol(")")?;
                using = Some(cols);
            } else {
                // Sin ON ni USING: válido solo si CROSS o NATURAL.
                if !matches!(kind, JoinKind::Cross) && !natural {
                    return Err(coded(
                        codes::JOIN_PREDICATE_REQUIRED,
                        format!(
                            "JOIN sobre '{}' requiere cláusula ON l = r, USING (col) o NATURAL \
                             (CROSS JOIN es la única forma sin predicado)",
                            right.name
                        ),
                    ));
                }
            }
            joins.push(JoinClause {
                kind,
                right,
                on,
                using,
                natural,
            });
        }

        let mut where_clause: Option<WhereExpr> = None;
        if self.match_keyword("WHERE") {
            where_clause = Some(self.parse_where_expr()?);
        }

        // Optional ORDER BY <ident> [ASC|DESC]. Has to come after WHERE
        // and before LIMIT/OFFSET — that's the standard SQL order and
        // also what most callers expect.
        let mut order_by = None;
        if self.match_keyword("ORDER") {
            self.expect_keyword("BY")?;
            let column = self.expect_ident()?;
            // ASC is the default. We still consume the literal ASC
            // token if present so it doesn't leak into the LIMIT/OFFSET
            // parser below.
            let direction = if self.match_keyword("DESC") {
                OrderDir::Desc
            } else {
                let _ = self.match_keyword("ASC");
                OrderDir::Asc
            };
            order_by = Some(OrderClause { column, direction });
        }

        let mut limit = None;
        let mut offset = 0usize;
        let mut seen_limit = false;
        let mut seen_offset = false;
        loop {
            if self.match_keyword("LIMIT") {
                if seen_limit {
                    return Err(coded(
                        codes::LIMIT_DUPLICATED,
                        "LIMIT aparece más de una vez en la query: solo se admite uno por SELECT",
                    ));
                }
                let raw = self.expect_integer()?;
                if raw < 0 {
                    return Err(coded(
                        codes::LIMIT_NEGATIVE,
                        format!("LIMIT debe ser >= 0; recibí {}", raw),
                    ));
                }
                limit = Some(raw as usize);
                seen_limit = true;
                continue;
            }
            if self.match_keyword("OFFSET") {
                if seen_offset {
                    return Err(coded(
                        codes::OFFSET_DUPLICATED,
                        "OFFSET aparece más de una vez en la query: solo se admite uno por SELECT",
                    ));
                }
                let raw = self.expect_integer()?;
                if raw < 0 {
                    return Err(coded(
                        codes::OFFSET_NEGATIVE,
                        format!("OFFSET debe ser >= 0; recibí {}", raw),
                    ));
                }
                offset = raw as usize;
                seen_offset = true;
                continue;
            }
            break;
        }

        Ok(SelectStmt {
            table,
            table_alias,
            joins,
            columns,
            where_clause,
            order_by,
            limit,
            offset,
        })
    }

    /// Parsea una expresión de WHERE con soporte completo de `AND`/`OR`/`NOT`
    /// y paréntesis (Bloque E1). Precedencia estándar SQL:
    ///   `OR` (más baja) < `AND` < `NOT` < paréntesis / átomo (más alta).
    /// Asume que el caller ya consumió el keyword `WHERE`.
    fn parse_where_expr(&mut self) -> DbResult<WhereExpr> {
        self.parse_where_or()
    }

    fn parse_where_or(&mut self) -> DbResult<WhereExpr> {
        let mut left = self.parse_where_and()?;
        while self.match_keyword("OR") {
            let right = self.parse_where_and()?;
            left = WhereExpr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_where_and(&mut self) -> DbResult<WhereExpr> {
        let mut left = self.parse_where_not()?;
        while self.match_keyword("AND") {
            let right = self.parse_where_not()?;
            left = WhereExpr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_where_not(&mut self) -> DbResult<WhereExpr> {
        // `NOT NOT x` se permite (cada NOT se apila y se cancela vía 3VL en
        // el evaluador). `NOT EXISTS (...)` mantiene la forma vieja
        // (`Atom(Exists { negated: true })`) para preservar el fast-path
        // del executor — sin esto el `EXISTS` correlacionado tendría que
        // re-evaluarse vía post-filter genérico.
        if self.match_keyword("NOT") {
            if self.peek().kind == TokenKind::Ident
                && self.peek().text.eq_ignore_ascii_case("EXISTS")
            {
                self.pos += 1; // consume EXISTS
                let subquery = self.parse_exists_subquery_body()?;
                return Ok(WhereExpr::Atom(WhereClause::Exists {
                    subquery: Box::new(subquery),
                    negated: true,
                }));
            }
            let inner = self.parse_where_not()?;
            return Ok(WhereExpr::Not(Box::new(inner)));
        }
        self.parse_where_primary()
    }

    fn parse_where_primary(&mut self) -> DbResult<WhereExpr> {
        if self.peek().kind == TokenKind::Symbol && self.peek().text == "(" {
            // Distinguir `(` de paréntesis de expresión booleana vs `(SELECT ...)`
            // de un átomo EXISTS/IN/=. Acá venimos del nivel primary: el único
            // caso donde un `(` arranca un sub-statement es `EXISTS (SELECT)`
            // — y ése ya lo consumió `parse_where_not`. Cualquier `(` que
            // llegue acá agrupa una expresión booleana.
            self.expect_symbol("(")?;
            let expr = self.parse_where_expr()?;
            self.expect_symbol(")")?;
            return Ok(expr);
        }
        // EXISTS sin NOT delante — válido como átomo dentro de cualquier
        // posición (top-level, dentro de paréntesis, lado derecho de AND/OR).
        if self.match_keyword("EXISTS") {
            let subquery = self.parse_exists_subquery_body()?;
            return Ok(WhereExpr::Atom(WhereClause::Exists {
                subquery: Box::new(subquery),
                negated: false,
            }));
        }
        let atom = self.parse_where_atom()?;
        Ok(WhereExpr::Atom(atom))
    }

    fn parse_exists_subquery_body(&mut self) -> DbResult<SelectStmt> {
        if !(self.peek().kind == TokenKind::Symbol && self.peek().text == "(") {
            return Err(coded(
                codes::EXISTS_REQUIRES_SUBQUERY,
                "EXISTS requiere '(SELECT ...)' a continuación",
            ));
        }
        self.expect_symbol("(")?;
        self.expect_keyword("SELECT")?;
        let subquery = self.parse_select_stmt()?;
        self.expect_symbol(")")?;
        Ok(subquery)
    }

    /// Parsea un único predicado (átomo) del WHERE. Las combinaciones
    /// booleanas las arma `parse_where_*` por encima.
    ///
    /// Operadores soportados (después del nombre de columna):
    /// - `=` literal / `(SELECT ...)` / `otra.col` (column-ref correlacionado)
    /// - `<`, `>`, `<=`, `>=`, `<>`, `!=` (bloque E2)
    /// - `BETWEEN n AND m`
    /// - `[NOT] IN (...)` — lista literal o `(SELECT ...)` (bloque E2 añade lista)
    /// - `[NOT] LIKE 'patron'` (bloque E2)
    /// - `IS [NOT] NULL` (bloque E2)
    fn parse_where_atom(&mut self) -> DbResult<WhereClause> {
        let column = self.expect_ident()?;
        // `=` exacto: misma lógica que pre-E2 (literal | subquery | outer-ref).
        if self.match_symbol("=") {
            if self.peek().kind == TokenKind::Symbol && self.peek().text == "(" {
                self.expect_symbol("(")?;
                self.expect_keyword("SELECT")?;
                let subquery = self.parse_select_stmt()?;
                self.expect_symbol(")")?;
                return Ok(WhereClause::EqSubquery {
                    column,
                    subquery: Box::new(subquery),
                });
            } else if self.peek().kind == TokenKind::Ident
                && !is_value_keyword(&self.peek().text)
            {
                let raw = self.expect_ident()?;
                let (ref_table, ref_column) = split_qualified_ident(&raw);
                return Ok(WhereClause::EqColumnRef {
                    column,
                    ref_table,
                    ref_column,
                });
            }
            let value = self.expect_value()?;
            return Ok(WhereClause::Eq { column, value });
        }
        // Comparadores E2: `<`, `<=`, `<>`, `>`, `>=`, `!=`.
        if let Some(op) = self.peek_compare_op() {
            self.pos += 1;
            let value = self.expect_value()?;
            return Ok(WhereClause::Compare { column, op, value });
        }
        // `IS [NOT] NULL`. El keyword `IS` aún no aparece en otra parte del
        // grammar, así que su consumo acá no choca con nada.
        if self.match_keyword("IS") {
            let negated = self.match_keyword("NOT");
            if !self.match_keyword("NULL") {
                return Err(coded(
                    codes::WHERE_OPERATOR_UNSUPPORTED,
                    format!(
                        "se esperaba NULL después de IS{} sobre '{}'",
                        if negated { " NOT" } else { "" },
                        column
                    ),
                ));
            }
            return Ok(WhereClause::IsNull { column, negated });
        }
        if self.match_keyword("LIKE") {
            let pattern = self.expect_string_literal("LIKE")?;
            return Ok(WhereClause::Like {
                column,
                pattern,
                negated: false,
            });
        }
        if self.match_keyword("BETWEEN") {
            let from = self.expect_integer()?;
            self.expect_keyword("AND")?;
            let to = self.expect_integer()?;
            return Ok(WhereClause::Between { column, from, to });
        }
        if self.match_keyword("IN") {
            return self.parse_in_body(column, false);
        }
        // Forma postfix con `NOT`: `NOT LIKE`, `NOT IN`. El `NOT` del
        // combinador booleano se consume antes (en `parse_where_not`);
        // si llegamos acá es porque el `NOT` apareció justo después de
        // la columna, así que pertenece a un operador postfix.
        if self.match_keyword("NOT") {
            if self.match_keyword("LIKE") {
                let pattern = self.expect_string_literal("NOT LIKE")?;
                return Ok(WhereClause::Like {
                    column,
                    pattern,
                    negated: true,
                });
            }
            if self.match_keyword("IN") {
                return self.parse_in_body(column, true);
            }
            return Err(coded(
                codes::WHERE_OPERATOR_UNSUPPORTED,
                format!(
                    "después de NOT se esperaba LIKE o IN sobre la columna '{}'",
                    column
                ),
            ));
        }
        Err(coded(
            codes::WHERE_OPERATOR_UNSUPPORTED,
            format!(
                "WHERE: no se reconoció el operador después de la columna '{}'. \
                 Operadores soportados: =, <, >, <=, >=, <>, !=, BETWEEN ... AND, \
                 IS [NOT] NULL, [NOT] LIKE, [NOT] IN (lista | SELECT)",
                column
            ),
        ))
    }

    /// Después de consumir `IN` (o `NOT IN`), parsea el cuerpo:
    /// `(SELECT ...)` (subquery) o `(lit, lit, ...)` (lista literal).
    /// `negated` true sólo viene del path `NOT IN`.
    fn parse_in_body(&mut self, column: String, negated: bool) -> DbResult<WhereClause> {
        self.expect_symbol("(")?;
        if self.peek().kind == TokenKind::Ident && self.peek().text.eq_ignore_ascii_case("SELECT") {
            self.pos += 1;
            let subquery = self.parse_select_stmt()?;
            self.expect_symbol(")")?;
            if negated {
                // `NOT IN (SELECT ...)` aún no se desugara a un átomo
                // dedicado en este release — el bloque H del roadmap lo
                // generaliza junto con NOT IN correlacionado. Acá lo
                // rechazamos explícitamente para no devolver semántica
                // silenciosamente incorrecta.
                return Err(coded(
                    codes::WHERE_OPERATOR_UNSUPPORTED,
                    "NOT IN (SELECT ...) no se soporta en este release; usar IN (SELECT ...) \
                     dentro de NOT (...) tiene semántica distinta (3VL con NULLs) — esperar \
                     al bloque H del roadmap",
                ));
            }
            return Ok(WhereClause::In {
                column,
                subquery: Box::new(subquery),
            });
        }
        // Lista literal: por lo menos un valor.
        let mut values = vec![self.expect_value()?];
        while self.match_symbol(",") {
            values.push(self.expect_value()?);
        }
        self.expect_symbol(")")?;
        Ok(WhereClause::InList {
            column,
            values,
            negated,
        })
    }

    /// Si el token actual es uno de los símbolos de comparación E2,
    /// devuelve el `CompareOp` correspondiente sin avanzar el cursor.
    /// El caller hace `self.pos += 1` al consumirlo.
    fn peek_compare_op(&self) -> Option<CompareOp> {
        let t = self.peek();
        if t.kind != TokenKind::Symbol {
            return None;
        }
        match t.text.as_str() {
            "<" => Some(CompareOp::Lt),
            "<=" => Some(CompareOp::Le),
            ">" => Some(CompareOp::Gt),
            ">=" => Some(CompareOp::Ge),
            "<>" | "!=" => Some(CompareOp::Ne),
            _ => None,
        }
    }

    /// Consume un literal string, rechazando cualquier otro tipo. Útil
    /// para operadores cuyo RHS debe ser texto (LIKE).
    fn expect_string_literal(&mut self, context: &str) -> DbResult<String> {
        let t = self.peek().clone();
        if t.kind != TokenKind::String {
            return Err(coded(
                codes::WHERE_OPERATOR_UNSUPPORTED,
                format!(
                    "{} requiere un literal string como patrón; llegó '{}'",
                    context, t.text
                ),
            ));
        }
        self.pos += 1;
        Ok(t.text)
    }

    fn parse_ident_list(&mut self) -> DbResult<Vec<String>> {
        let mut out = vec![self.expect_ident()?];
        while self.match_symbol(",") {
            out.push(self.expect_ident()?);
        }
        Ok(out)
    }

    fn parse_value_list(&mut self) -> DbResult<Vec<Value>> {
        let mut out = vec![self.expect_value()?];
        while self.match_symbol(",") {
            out.push(self.expect_value()?);
        }
        Ok(out)
    }

    fn expect_value(&mut self) -> DbResult<Value> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Number => {
                self.pos += 1;
                if token.text.contains('.') {
                    Ok(Value::Float(token.text.parse()?))
                } else {
                    Ok(Value::Integer(token.text.parse()?))
                }
            }
            TokenKind::String => {
                self.pos += 1;
                Ok(Value::String(token.text))
            }
            TokenKind::Ident => {
                if token.text.eq_ignore_ascii_case("TRUE") {
                    self.pos += 1;
                    Ok(Value::Bool(true))
                } else if token.text.eq_ignore_ascii_case("FALSE") {
                    self.pos += 1;
                    Ok(Value::Bool(false))
                } else if token.text.eq_ignore_ascii_case("NULL") {
                    self.pos += 1;
                    Ok(Value::Null)
                } else {
                    Err(DbError::new(format!("valor invÃ¡lido: {}", token.text)))
                }
            }
            _ => Err(DbError::new(format!("valor invÃ¡lido: {}", token.text))),
        }
    }

    fn expect_integer(&mut self) -> DbResult<i64> {
        let token = self.peek().clone();
        if token.kind != TokenKind::Number {
            return Err(DbError::new(format!(
                "se esperaba nÃºmero, llegÃ³: {}",
                token.text
            )));
        }
        if token.text.contains('.') {
            return Err(DbError::new(format!(
                "se esperaba entero, llegÃ³: {}",
                token.text
            )));
        }
        self.pos += 1;
        Ok(token.text.parse()?)
    }

    fn expect_ident(&mut self) -> DbResult<String> {
        let token = self.peek().clone();
        if token.kind != TokenKind::Ident {
            return Err(DbError::new(format!(
                "se esperaba identificador, llegÃ³: {}",
                token.text
            )));
        }
        self.pos += 1;
        Ok(token.text)
    }

    fn expect_keyword(&mut self, expected: &str) -> DbResult<()> {
        if self.match_keyword(expected) {
            return Ok(());
        }
        Err(DbError::new(format!("se esperaba keyword {}", expected)))
    }

    fn match_keyword(&mut self, expected: &str) -> bool {
        let token = self.peek();
        if token.kind == TokenKind::Ident && token.text.eq_ignore_ascii_case(expected) {
            self.pos += 1;
            return true;
        }
        false
    }

    fn expect_symbol(&mut self, expected: &str) -> DbResult<()> {
        if self.match_symbol(expected) {
            return Ok(());
        }
        Err(DbError::new(format!("se esperaba sÃ­mbolo {}", expected)))
    }

    fn match_symbol(&mut self, expected: &str) -> bool {
        let token = self.peek();
        if token.kind == TokenKind::Symbol && token.text == expected {
            self.pos += 1;
            return true;
        }
        false
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn is_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_part(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'
}
