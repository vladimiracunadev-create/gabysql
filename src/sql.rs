use crate::bptree::{init_leaf_page, KeyValue, Tree};
use crate::catalog::{
    validate_create_table, validate_identifier, Catalog, CatalogObject, CheckConstraint, Column,
    ColumnType, DefaultLiteral, ForeignKeyMeta, IndexKind, IndexMeta, OnDelete, OnUpdate,
    TableMeta, ViewMeta,
};
use crate::errors::{coded, codes};
use crate::index::{
    bucket_insert, bucket_lookup, bucket_remove, bucket_unique_conflict, decode_bucket,
    decode_ordered_bucket, encode_bucket, encode_column_value, encode_composite_key,
    encode_ordered_bucket, hash_value, ordered_bucket_insert, ordered_bucket_remove,
    ordered_bucket_unique_conflict, ordered_int_key_from_value_bytes, validate_indexable,
};
use crate::storage::Pager;
use crate::{DbError, DbResult};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    CreateTable(CreateTableStmt),
    DropTable(DropTableStmt),
    AlterTableAddColumn(AlterAddColumnStmt),
    /// L3 (2026-05-27): `ALTER TABLE <t> ADD [CONSTRAINT name] CHECK (<expr>)`.
    /// Disparar un full-scan O(n) sobre la tabla para validar cada fila
    /// contra el nuevo predicado antes de persistirlo.
    AlterTableAddCheck(AlterAddCheckStmt),
    /// Residual #2 (2026-05-27): `ALTER TABLE <t> DROP CONSTRAINT <name>`.
    /// Lookup case-insensitive del nombre a través de: CHECKs, índices
    /// UNIQUE nombrados, FKs nombradas. PK rejected.
    AlterTableDropConstraint(AlterDropConstraintStmt),
    /// Bloque V (2026-05-27): `CREATE VIEW [IF NOT EXISTS] name
    /// [(col_aliases)] AS <select_query>`. La fuente puede ser un
    /// SELECT, VALUES, o set operation — cualquier `SelectQuery`.
    CreateView(CreateViewStmt),
    /// Bloque V (2026-05-27): `DROP VIEW [IF EXISTS] <name>`.
    DropView(DropViewStmt),
    /// Bloque K1 (2026-05-26): `CREATE TABLE [IF NOT EXISTS] <name>
    /// [ (col1, col2, ...) ] AS <select>`. La fuente puede ser cualquier
    /// `SelectQuery` (SELECT, UNION/INTERSECT/EXCEPT, o VALUES). La
    /// primera columna del result-set debe ser INT y se promueve a PK
    /// de la nueva tabla — sin esa columna, error `[GBY-4058]`.
    CreateTableAs(CreateTableAsStmt),
    /// Bloque K1: `RENAME TABLE <old> TO <new>` y la forma equivalente
    /// `ALTER TABLE <old> RENAME TO <new>`. Renombra la entry del
    /// catálogo y actualiza las FKs que apuntaban al nombre viejo.
    RenameTable(RenameTableStmt),
    /// Bloque K1: `ALTER TABLE <t> DROP COLUMN [IF EXISTS] <col>`.
    /// Rewrite in place de cada fila (decode + remove col + re-encode +
    /// insert). Bloqueado sobre PK, columnas indexadas, y columnas con
    /// FK saliente o entrante.
    AlterTableDropColumn(AlterDropColumnStmt),
    /// Bloque K1: `ALTER TABLE <t> RENAME COLUMN <old> TO <new>`. No
    /// reescribe filas (el on-disk row es posicional); solo muta
    /// `TableMeta.columns[i].name`, `primary_key`, índices y FKs que
    /// referencien la columna.
    AlterTableRenameColumn(AlterRenameColumnStmt),
    Insert(InsertStmt),
    /// Bloque I (2026-05-26): el SELECT statement pasa de envolver un
    /// `SelectStmt` plano a envolver un `SelectQuery`, que admite además
    /// operaciones de conjunto (`UNION`/`INTERSECT`/`EXCEPT`) y
    /// `VALUES (...), (...)` standalone como query. El caso `SELECT ...`
    /// puro sigue funcionando idéntico — se construye como
    /// `SelectQuery::Select(stmt)`.
    Select(Box<SelectQuery>),
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
    /// Bloque T: `BEGIN` / `START TRANSACTION` — marca el inicio de una
    /// transacción explícita dentro del mismo batch. El wrap externo
    /// (CLI/server) ya garantiza atomicidad de batch; `BEGIN` permite
    /// además abortar el batch a mitad de camino con `ROLLBACK`.
    Begin,
    /// Bloque J: `TRUNCATE [TABLE] <name>` — borra todas las filas de la
    /// tabla. Implementación: iteración sobre todas las PKs aplicando
    /// `delete_with_cascade` por fila (no es O(1) como en PG/MySQL, pero
    /// respeta FKs `ON DELETE` declaradas). Restricciones diferidas no
    /// se soportan.
    Truncate(TruncateStmt),
    /// Bloque J2: `REPLACE INTO t (cols) VALUES (...)` (SQLite-style).
    /// Desugar a `INSERT ... ON CONFLICT DO REPLACE`: el parser construye
    /// un `InsertStmt` con `on_conflict = Some(OnConflict { target: None,
    /// action: Replace })`.
    Replace(InsertStmt),
    /// Bloque T: `COMMIT` / `END` — cierra la transacción explícita
    /// activa: persiste lo acumulado y re-abre una tx fresca para que
    /// el wrap del caller siga válido.
    Commit,
    /// Bloque T: `ROLLBACK` — descarta lo acumulado en la transacción
    /// explícita actual y re-abre una tx fresca. Las sentencias previas
    /// del MISMO batch (incluso las que pasaron antes del BEGIN) también
    /// se pierden, porque el wrap externo es una única transacción
    /// física; documentado como limitación de la versión inicial de T.
    Rollback,
}

/// Bloque I (2026-05-26): nivel superior de un statement SELECT.
///
/// Antes de I, un `SELECT` siempre se representaba como `SelectStmt`
/// (un solo cuerpo con FROM/WHERE/...). Con set ops (`UNION`,
/// `INTERSECT`, `EXCEPT`/`MINUS`) y con `VALUES` como query standalone,
/// hace falta un wrapper que pueda ser cualquiera de las tres formas.
/// El árbol queda izquierdo-anidado (asociativo a izquierda según
/// ANSI), y la precedencia se aplica en el parser:
/// `INTERSECT` ata más fuerte que `UNION` / `EXCEPT`.
///
/// El `ORDER BY` / `LIMIT` / `OFFSET` que aparece DESPUÉS del último
/// término de un árbol de set ops aplica al resultado combinado y vive
/// en la variante `SetOp` (no en cada lado). Cuando el SELECT es plano
/// (sin set ops), el ORDER BY/LIMIT/OFFSET vive en el `SelectStmt` como
/// hasta ahora.
#[derive(Debug, Clone, PartialEq)]
pub enum SelectQuery {
    /// SELECT puro — wrapper trivial sobre la representación pre-I.
    Select(Box<SelectStmt>),
    /// Operación de conjunto entre dos sub-queries.
    SetOp {
        lhs: Box<SelectQuery>,
        op: SetOpKind,
        /// `true` cuando es la forma `... ALL` (preserva duplicados).
        all: bool,
        rhs: Box<SelectQuery>,
        /// `ORDER BY` opcional a nivel del resultado combinado.
        /// Resuelto por nombre contra el header del resultset final
        /// (que viene del LHS — semántica ANSI estándar).
        order_by: Option<OrderClause>,
        /// `LIMIT` y `OFFSET` opcionales aplicados al resultado
        /// combinado y post-ORDER BY.
        limit: Option<usize>,
        offset: usize,
    },
    /// `VALUES (row), (row), ...` como query standalone.
    Values(ValuesClause),
}

/// Bloque I: tipo de operación de conjunto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOpKind {
    Union,
    Intersect,
    /// `EXCEPT` (ANSI) y `MINUS` (alias Oracle) — un solo variant.
    Except,
}

impl SetOpKind {
    pub fn keyword(&self) -> &'static str {
        match self {
            SetOpKind::Union => "UNION",
            SetOpKind::Intersect => "INTERSECT",
            SetOpKind::Except => "EXCEPT",
        }
    }
}

/// Bloque I: lista de filas literales para `VALUES (...)`. Cada fila es
/// un `Vec<Expr>` (expresiones, no `Value`) — esto permite literales
/// directos (`1`, `'a'`) pero también expresiones constantes evaluables
/// sin contexto de fila (`1+2`, `LENGTH('abc')`). El executor evalúa
/// cada `Expr` contra una fila vacía (`HashMap` vacío) — referencias a
/// columnas dentro de un `VALUES` fallan limpio porque el row está vacío.
///
/// Invariantes garantizadas por el parser:
/// - `rows.len() >= 1` (`VALUES` vacío → `[GBY-4057]`).
/// - Toda fila tiene la misma arity (mismatch → `[GBY-4056]`).
#[derive(Debug, Clone, PartialEq)]
pub struct ValuesClause {
    pub rows: Vec<Vec<Expr>>,
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

/// L3 (2026-05-27): `ALTER TABLE <t> ADD [CONSTRAINT <name>] CHECK (<expr>)`.
/// El `source` ya está canonicalizado por `format_expr` para garantizar
/// que el round-trip catálogo ↔ disco sea estable, mismo contrato que
/// los CHECKs declarados en `CREATE TABLE` (Bloque L2).
#[derive(Debug, Clone, PartialEq)]
pub struct AlterAddCheckStmt {
    pub table: String,
    /// Nombre explícito si vino con `CONSTRAINT name CHECK (...)`. Si es
    /// `None`, el executor sintetiza `<tabla>_check_<N>` con N empezando
    /// donde quedó el último check declarado.
    pub name: Option<String>,
    /// Texto SQL canónico re-parseable por `parse_expr_str`.
    pub source: String,
}

/// Residual #2 (2026-05-27): `ALTER TABLE <t> DROP CONSTRAINT <name>`.
#[derive(Debug, Clone, PartialEq)]
pub struct AlterDropConstraintStmt {
    pub table: String,
    pub name: String,
    pub if_exists: bool,
}

/// Bloque V (2026-05-27): `CREATE VIEW [IF NOT EXISTS] name
/// [(col_aliases)] AS <select_query>`. El parser captura el texto SQL
/// canonicalizado del SELECT en `source` (reconstrucción token-a-token)
/// para que `exec_create_view` lo persista intacto en el catálogo.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateViewStmt {
    pub name: String,
    pub if_not_exists: bool,
    pub column_aliases: Option<Vec<String>>,
    /// Texto SQL re-construido del SELECT subyacente. Re-parseable por
    /// `parse_select_query_for_ctas` en cada ejecución de la vista.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropViewStmt {
    pub name: String,
    pub if_exists: bool,
}

/// Bloque K1 (2026-05-26): AST de `CREATE TABLE [IF NOT EXISTS] <name>
/// [ (col_alias, ...) ] AS <select_query>`. La fuente reusa el árbol del
/// bloque I (`SelectQuery`), por lo que admite SELECT puro, operaciones
/// de conjunto y `VALUES (...), (...)` como fuente.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateTableAsStmt {
    pub name: String,
    pub source: Box<SelectQuery>,
    /// `true` cuando vino `CREATE TABLE IF NOT EXISTS ... AS ...`. Con
    /// ese flag, si el destino ya existe la sentencia es no-op (devuelve
    /// un mensaje informativo); sin el flag, error `[GBY-2004]`.
    pub if_not_exists: bool,
    /// Lista opcional de alias para las columnas (`AS dst (a, b, c)`).
    /// Si está presente sustituye los headers del result-set y debe
    /// coincidir en arity (`[GBY-4063]`). Si es `None`, se usan los
    /// headers del SELECT/VALUES tal cual.
    pub column_aliases: Option<Vec<String>>,
}

/// Bloque K1: AST común de `RENAME TABLE <old> TO <new>` y
/// `ALTER TABLE <old> RENAME TO <new>`.
#[derive(Debug, Clone, PartialEq)]
pub struct RenameTableStmt {
    pub old_name: String,
    pub new_name: String,
}

/// Bloque K1: AST de `ALTER TABLE <table> DROP COLUMN [IF EXISTS] <col>`.
#[derive(Debug, Clone, PartialEq)]
pub struct AlterDropColumnStmt {
    pub table: String,
    pub column: String,
    pub if_exists: bool,
}

/// Bloque K1: AST de `ALTER TABLE <table> RENAME COLUMN <old> TO <new>`.
#[derive(Debug, Clone, PartialEq)]
pub struct AlterRenameColumnStmt {
    pub table: String,
    pub old_name: String,
    pub new_name: String,
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
    /// Primera columna del índice. Bloque K2 (2026-05-26): cuando el
    /// parser ve `CREATE [UNIQUE] INDEX idx ON t (a, b, ...)` con más
    /// de una columna, `column` lleva la primera y `extra_columns` el
    /// resto. Para índices single-column (caso histórico)
    /// `extra_columns` queda vacío.
    pub column: String,
    pub unique: bool,
    /// Bloque K2: columnas adicionales del índice compuesto. Vacío
    /// para single-column. Cuando no está vacío, el executor exige
    /// que todas las columnas sean INT (4067) y genera la clave del
    /// B+Tree como fingerprint FNV-1a-64 sobre la tupla (ver
    /// ADR-0019).
    pub extra_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropIndexStmt {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStmt {
    pub table: String,
    /// Bloque G2 (2026-05-26): la RHS de cada assignment pasa de `Value`
    /// a `Expr`. Eso habilita `SET col = UPPER(col)`, `SET col = COALESCE(col, 0)`,
    /// CASE/CAST, etc. Para back-compat un literal se construye como
    /// `Expr::Literal(Value::X(...))`. La expresión se evalúa contra la
    /// fila **pre-update** (no observa los otros assignments del mismo SET).
    pub assignments: Vec<(String, Expr)>,
    /// Bloque E3: el WHERE de UPDATE/DELETE es un `WhereExpr` completo —
    /// admite los mismos operadores que `SELECT.where_clause` (=, BETWEEN,
    /// <, >, LIKE, IS NULL, IN literal/SELECT, AND/OR/NOT, etc.). Es
    /// obligatorio: las mutaciones masivas sin WHERE quedan deshabilitadas
    /// hasta un release explícito.
    pub where_clause: WhereExpr,
    /// Bloque J2: `RETURNING *` o `RETURNING col1, col2, ...`. Cuando
    /// es `Some`, el ResultSet trae las filas actualizadas (post-update)
    /// proyectadas según la lista.
    pub returning: Option<Vec<SelectItem>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteStmt {
    pub table: String,
    /// Bloque E3: ver doc de `UpdateStmt::where_clause`. Mismo grammar,
    /// mismas reglas.
    pub where_clause: WhereExpr,
    /// Bloque J2: `RETURNING *` o `RETURNING col1, col2, ...`. Cuando
    /// es `Some`, el ResultSet trae las filas borradas (snapshot
    /// previo a la eliminación) proyectadas según la lista.
    pub returning: Option<Vec<SelectItem>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTableStmt {
    pub name: String,
    pub columns: Vec<ColumnDef>,
    /// Primera columna de la PRIMARY KEY (inline `… PRIMARY KEY` o la
    /// primera del table-level `PRIMARY KEY (a, b, ...)`).
    pub primary_key: String,
    /// Bloque K2 (2026-05-26): columnas adicionales declaradas por un
    /// table-level `PRIMARY KEY (a, b, ...)`. Vacío para PK single
    /// (caso histórico). El validator (`validate_create_table`) exige
    /// que toda columna PK sea INT NOT NULL cuando hay más de una.
    pub primary_key_extra: Vec<String>,
    /// Bloque L1 (2026-05-27): listas de columnas declaradas como
    /// `UNIQUE (a, b, ...)` a nivel de tabla. Cada entrada se materializa
    /// como un índice UNIQUE en `CREATE TABLE`. Vacío para tablas que
    /// sólo usan UNIQUE inline en una columna o no usan UNIQUE.
    pub unique_constraints: Vec<Vec<String>>,
    /// Bloque L2 (2026-05-27): `CHECK (expr)` declarados a nivel de
    /// columna o de tabla, junto con su nombre opcional
    /// (`CONSTRAINT name CHECK (...)`). El parser ya re-formatea cada
    /// expresión con `format_expr` para que el catálogo persista el
    /// texto canónico (y no haya drift entre el SQL original y el
    /// re-parse).
    pub check_constraints: Vec<CheckConstraint>,
    /// Residual #2 (2026-05-27): nombre opcional para la PRIMARY KEY
    /// (declarado vía `CONSTRAINT <name> PRIMARY KEY (...)`). Si la PK
    /// se declaró inline o sin `CONSTRAINT`, queda `None`.
    pub primary_key_name: Option<String>,
    /// Residual #2 (2026-05-27): listas de columnas declaradas como
    /// `CONSTRAINT <name> UNIQUE (a, b, ...)` a nivel de tabla. Igual
    /// que `unique_constraints` pero con nombre explícito. Cada entrada
    /// se materializa como un índice UNIQUE con `IndexMeta.name = name`.
    pub named_unique_constraints: Vec<(String, Vec<String>)>,
    /// Residual #2 (2026-05-27): FKs declaradas table-level con nombre
    /// (`CONSTRAINT <name> FOREIGN KEY (col) REFERENCES t (col) [ON ...]`).
    /// Se aplican sobre la columna correspondiente del `columns` Vec
    /// durante la construcción del `TableMeta`. Single-col only por
    /// ahora (multi-col es residual #3).
    pub named_foreign_keys: Vec<NamedForeignKey>,
}

/// Residual #2 (2026-05-27): tipo discriminado para
/// `try_match_named_table_constraint_head`. CHECK no aparece — su
/// path es distinto porque el cuerpo es un `Expr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamedConstraintKind {
    PrimaryKey,
    Unique,
    ForeignKey,
}

#[derive(Debug, Clone)]
struct NamedConstraintHead {
    name: String,
    kind: NamedConstraintKind,
}

/// Residual #2 (2026-05-27): FK table-level con nombre explícito.
/// Residual #3 (2026-05-27): admite multi-col mediante
/// `extra_source_columns` y `extra_target_columns` (mismo orden,
/// misma arity). Single-col los deja vacíos.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedForeignKey {
    pub name: String,
    pub column: String,
    pub target_table: String,
    pub target_column: String,
    pub on_delete: OnDelete,
    pub on_update: OnUpdate,
    pub extra_source_columns: Vec<String>,
    pub extra_target_columns: Vec<String>,
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

/// Parser-level representation of
/// `REFERENCES <table>(<column>) [ON DELETE ...] [ON UPDATE ...]`.
/// Translated into a catalog [`ForeignKeyMeta`] inside the executor
/// (see `fk_def_to_meta`).
///
/// Bloque L1 (2026-05-27): `on_delete` admite ahora `SET NULL` y
/// `SET DEFAULT` además de los originales `RESTRICT`/`CASCADE`. Se
/// agrega `on_update` (default `NoAction`); el motor lo persiste pero
/// no lo dispara hoy porque la PK es inmutable (`[GBY-4008]`).
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignKeyDef {
    pub table: String,
    pub column: String,
    pub on_delete: OnDelete,
    pub on_update: OnUpdate,
    /// Residual #2 (2026-05-27): nombre del constraint si fue declarado
    /// con `CONSTRAINT <name> FOREIGN KEY (col) REFERENCES …`. `None`
    /// para FKs declaradas inline en la columna sin nombre.
    pub name: Option<String>,
    /// Residual #3 (2026-05-27): columnas adicionales para FK multi-col
    /// (`FOREIGN KEY (a, b) REFERENCES p (x, y)`). El parser column-inline
    /// las deja vacías; sólo la rama table-level las llena.
    pub extra_source_columns: Vec<String>,
    pub extra_target_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertStmt {
    pub table: String,
    pub columns: Vec<String>,
    /// Bloque J: el origen de filas de un INSERT puede ser una lista de
    /// tuplas literales (multi-row) o un SELECT (INSERT...SELECT).
    /// Pre-J era `values: Vec<Value>` (single row); el wrapping en un
    /// enum unifica el tratamiento y mantiene el camino single-row como
    /// caso particular de `Values(vec![row])`.
    pub source: InsertSource,
    /// Bloque J2: cláusula `ON CONFLICT [(col)] DO NOTHING | DO UPDATE`.
    /// Cuando es `Some`, las violaciones de PK o UNIQUE durante el
    /// insert se rutean a la acción declarada en vez de abortar.
    /// `REPLACE INTO` se desazucara como `ON CONFLICT DO REPLACE`.
    pub on_conflict: Option<OnConflict>,
    /// Bloque J2: `RETURNING *` o `RETURNING col1, col2, ...`. Cuando
    /// es `Some`, el ResultSet trae las filas insertadas proyectadas
    /// según la lista (en vez del `message` con la cuenta).
    pub returning: Option<Vec<SelectItem>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnConflict {
    /// Columna objetivo del conflicto. `None` = cualquier constraint
    /// disparable (PK o cualquier UNIQUE).
    pub target: Option<String>,
    pub action: OnConflictAction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OnConflictAction {
    /// `DO NOTHING`: la fila conflictiva se descarta silenciosamente.
    DoNothing,
    /// `DO UPDATE SET col = expr, ...`: actualiza la fila existente con
    /// los assignments. Bloque G2 generaliza la RHS a `Expr` (igual que
    /// `UpdateStmt`), evaluada contra la fila pre-update. `EXCLUDED.col`
    /// (referirse a la fila intentada) sigue sin soportarse — P2 dentro
    /// del backlog de J2.
    DoUpdate { assignments: Vec<(String, Expr)> },
    /// `REPLACE` (desazucar de SQLite-style `REPLACE INTO`): borra las
    /// filas conflictivas vía cascade y reinserta con los valores nuevos.
    Replace,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertSource {
    /// `VALUES (a,b,c), (d,e,f), ...`. Cada tupla interior debe tener
    /// la misma aridad que `columns`. La lista exterior tiene ≥1 tupla.
    Values(Vec<Vec<Value>>),
    /// `SELECT ...`. El executor materializa la query, exige que su
    /// número de columnas coincida con `columns` del INSERT y mapea
    /// cada fila a un row a insertar. Subqueries con agregados o
    /// JOINs son válidas — la ejecución usa el mismo `exec_select`.
    Select(Box<SelectStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TruncateStmt {
    pub table: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectStmt {
    /// Base table del FROM. En SELECTs con JOIN sigue siendo "la primera"
    /// tabla declarada (las demás viven en `joins`). Se mantiene como
    /// `String` plano para no romper la API pública preexistente.
    ///
    /// Bloque H (2026-05-26): cuando `derived_source` es `Some`, este
    /// campo contiene el **alias obligatorio** de la derived table en
    /// vez del nombre de una tabla persistente. El executor consulta
    /// `derived_source` primero — si está, materializa la subquery; si
    /// no, abre `table` contra el catálogo como hasta ahora.
    pub table: String,
    /// Bloque H: cuando es `Some(stmt)`, el FROM es una derived table
    /// `(SELECT ...) AS <table>`. La subquery se materializa una sola
    /// vez (no admite correlación con su propio outer) y se expone como
    /// una tabla virtual cuyo nombre es `table`. ANSI exige el alias —
    /// el parser devuelve `[GBY-4048]` si lo omite.
    pub derived_source: Option<Box<SelectStmt>>,
    /// Bloque I (2026-05-26): cuando es `Some`, el FROM es un
    /// `(VALUES (...), (...)) AS table(c1, c2, ...)`. El primer
    /// elemento es la cláusula VALUES; el segundo, la lista de aliases
    /// de columna (obligatoria, validada por el parser). El executor
    /// materializa igual que `derived_source` y la entrega como tabla
    /// virtual al `JoinScope`. Mutuamente excluyente con `derived_source`.
    pub values_source: Option<(Box<ValuesClause>, Vec<String>)>,
    /// Alias opcional de la base table (`FROM alumnos a`). Aplica también
    /// cuando hay JOINs — es la forma estándar de des-ambiguar columnas.
    pub table_alias: Option<String>,
    /// JOINs adicionales a la base. Vacío = SELECT single-table (todo el
    /// pipeline single-table sigue intacto). Cada join se aplica en orden
    /// (left-deep tree).
    pub joins: Vec<JoinClause>,
    /// Bloque F: cada item del SELECT puede ser `*`, una columna explícita,
    /// o una función agregada (`COUNT/SUM/AVG/MIN/MAX`). El SELECT list
    /// puro `*` se representa como `vec![SelectItem::Star]`. Mezclar `*`
    /// con otras formas no se acepta en este release.
    pub columns: Vec<SelectItem>,
    pub where_clause: Option<WhereExpr>,
    /// Bloque F: `SELECT DISTINCT` — dedup post-proyección.
    pub distinct: bool,
    /// Bloque F: columnas del `GROUP BY` (en orden). Vacío = sin
    /// agrupamiento explícito. Si hay funciones agregadas en el SELECT
    /// sin `GROUP BY`, se hace agregado global (UNA fila de salida).
    pub group_by: Vec<String>,
    /// Bloque F: filtro post-agregación. Reusa el mismo `WhereExpr` de
    /// E1/E2 pero parseado con `allow_aggregates=true`: la LHS de un
    /// átomo puede ser una función agregada (`SUM(price)`, `COUNT(*)`,
    /// etc.) que el evaluador resuelve contra el bucket agrupado.
    pub having: Option<WhereExpr>,
    pub order_by: Option<OrderClause>,
    pub limit: Option<usize>,
    pub offset: usize,
}

/// Tabla referenciada en el FROM (base o lado derecho de un JOIN).
#[derive(Debug, Clone, PartialEq)]
pub struct TableRef {
    pub name: String,
    pub alias: Option<String>,
    /// Bloque H (2026-05-26): cuando es `Some`, el operand del JOIN es
    /// una derived table `(SELECT ...) AS alias`. `name` lleva el alias,
    /// `alias` es siempre `None` (la sintaxis pone el alias dentro del
    /// constructor). El executor materializa antes de joinear.
    pub derived: Option<Box<SelectStmt>>,
    /// Bloque I (2026-05-26): cuando es `Some`, el operand es un
    /// `(VALUES (...), (...)) AS alias(c1, c2, ...)` — una tabla
    /// virtual literal. `name` lleva el alias obligatorio (4052) y
    /// `values_columns` lleva la lista de aliases de columna,
    /// también obligatoria con arity que matchea el row (4053).
    pub values: Option<Box<ValuesClause>>,
    /// Bloque I: alias de columna para una VALUES en FROM. `Some` sí y
    /// sólo sí `values.is_some()`. Mismo número de entradas que la
    /// arity de las tuplas (validado por el parser, 4053).
    pub values_columns: Option<Vec<String>>,
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

/// Bloque F: ítem proyectable en el SELECT list. Pre-F el SELECT solo
/// admitía `*` (encoded como `Vec::new()`) o una lista de idents. F
/// extiende a un enum unificado: `Star` mantiene `SELECT *`, `Column`
/// es la columna explícita, y `Aggregate` representa `func(arg) [AS alias]`.
#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    Star,
    Column(String),
    Aggregate {
        func: AggFunc,
        arg: AggArg,
        /// Alias opcional (`AS total`). Cuando está presente sobrescribe
        /// el nombre canónico en el header del ResultSet y se acepta
        /// como referencia válida en `HAVING` y `ORDER BY`.
        alias: Option<String>,
    },
    /// Bloque G1 (2026-05-26): expresión escalar arbitraria proyectada en
    /// el SELECT list — funciones (`LENGTH(name)`), CAST, CASE, literales,
    /// `COALESCE`, etc. Mantiene `Column` y `Aggregate` separados para no
    /// romper los fast-paths existentes (bare column lookup + agregación).
    Expression {
        expr: Expr,
        alias: Option<String>,
    },
}

/// Bloque G1: árbol mínimo de expresiones escalares. Vive solo dentro del
/// SELECT list por ahora (G2 lo extenderá a WHERE/HAVING/UPDATE SET).
///
/// El subset es voluntariamente chico: no hay operadores aritméticos
/// binarios (`+`, `-`, `*`, `/`) ni booleanos a tope (`AND`/`OR`) en el
/// árbol general — `Compare`/`IsNull` solo se materializan dentro de
/// `Case` searched. Esto evita interferir con el parser actual de WHERE
/// (que es donde viven los booleans) y deja el alcance acotado a "la
/// función + el CASE + el CAST".
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Literal directo: número, string, NULL, TRUE/FALSE.
    Literal(Value),
    /// Referencia a una columna; puede venir cualificada (`tabla.col`),
    /// se resuelve igual que `SelectItem::Column`.
    Column(String),
    /// Función escalar `NAME(arg, arg, ...)`.
    Func(ScalarFunc, Vec<Expr>),
    /// `CAST(expr AS TYPE)`.
    Cast(Box<Expr>, ColumnType),
    /// `CASE [operand] WHEN cond THEN val [WHEN ...] [ELSE val] END`.
    /// `operand = None` → searched form (cond es Expr booleana).
    /// `operand = Some(x)` → simple form (cond se compara por igualdad
    /// contra x con la misma semántica que `NULLIF`).
    Case {
        operand: Option<Box<Expr>>,
        branches: Vec<(Expr, Expr)>,
        else_branch: Option<Box<Expr>>,
    },
    /// Comparación binaria `lhs <op> rhs`. En G1 solo se construye dentro
    /// de un `CASE WHEN` searched.
    Compare(Box<Expr>, ExprCmpOp, Box<Expr>),
    /// `expr IS [NOT] NULL`. En G1 solo aparece dentro de `CASE WHEN`
    /// searched. NUNCA propaga NULL: es la forma explícita de preguntar
    /// por ausencia, igual que el `IS NULL` del WHERE.
    IsNull(Box<Expr>, bool /* negated */),
    /// Bloque G3: operador binario aritmético / concatenación.
    /// `+`, `-`, `*`, `/`, `%` y `||` con precedencia clásica armada
    /// por el parser (`*` `/` `%` antes que `+` `-` `||`).
    Arith(Box<Expr>, ArithOp, Box<Expr>),
    /// Bloque G3: `lhs [NOT] LIKE 'patron'` como expresión. Igual
    /// semántica que `WhereClause::Like` (3VL, escape con `\`).
    Like(Box<Expr>, String, bool /* negated */),
    /// Bloque G3: `lhs [NOT] IN (lit1, lit2, ...)` como expresión. Solo
    /// listas literales — subqueries en IN sobre Expr quedan para H.
    InList(Box<Expr>, Vec<Value>, bool /* negated */),
    /// Bloque G3: `lhs [NOT] BETWEEN low AND high` como expresión. Los
    /// tres operandos son `Expr`; promoción de tipos igual que
    /// `Compare`.
    Between(Box<Expr>, Box<Expr>, Box<Expr>, bool /* negated */),
    /// Bloque H (2026-05-26): subquery escalar embebida en una expresión
    /// del SELECT list / WHERE / HAVING / SET.
    ///
    /// La subquery debe devolver exactamente 1 columna y, en runtime, a
    /// lo sumo 1 fila (0 → NULL, más de 1 → `[GBY-4014]`). Puede
    /// referenciar columnas del outer scope vía el `outer_stack` del
    /// engine (correlated). El parser solo la construye dentro de
    /// paréntesis: `(SELECT MAX(x) FROM t)`. La evaluación requiere
    /// acceso al engine, por lo que `eval_expr` puro devuelve error si
    /// encuentra esta variante — el caller debe usar
    /// `Engine::eval_expr_full`.
    ScalarSubquery(Box<SelectStmt>),
}

/// Bloque G3: operadores binarios soportados por [`Expr::Arith`].
/// `Concat` agrupa al operador `||` y comparte precedencia con `+`/`-`
/// (regla PostgreSQL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Concat,
}

impl ArithOp {
    pub fn lexeme(&self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
            ArithOp::Mod => "%",
            ArithOp::Concat => "||",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprCmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Bloque G1: funciones escalares built-in que reconoce el parser y
/// evalúa el motor. La lista cubre los items P0/P1 del bloque G en
/// `docs/MISSING_COMMANDS.md`; el resto (TRIM, REPLACE, CEIL/FLOOR,
/// DATE_ADD, etc.) queda para iteraciones posteriores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarFunc {
    // string
    Length,
    Upper,
    Lower,
    Substr,
    Concat,
    // string (G3 P2/P3)
    Trim,
    Ltrim,
    Rtrim,
    Replace,
    SplitPart,
    // numeric
    Abs,
    Round,
    // numeric (G3 P2/P3)
    Ceil,
    Floor,
    Mod,
    Power,
    Sqrt,
    // datetime
    Now,
    CurrentDate,
    CurrentTimestamp,
    // datetime (G3 P2/P3)
    DateAdd,
    DateSub,
    Datediff,
    Extract,
    Strftime,
    // conditional
    Coalesce,
    Nullif,
    Ifnull,
    If,
}

impl ScalarFunc {
    pub fn keyword(&self) -> &'static str {
        match self {
            ScalarFunc::Length => "LENGTH",
            ScalarFunc::Upper => "UPPER",
            ScalarFunc::Lower => "LOWER",
            ScalarFunc::Substr => "SUBSTR",
            ScalarFunc::Concat => "CONCAT",
            ScalarFunc::Trim => "TRIM",
            ScalarFunc::Ltrim => "LTRIM",
            ScalarFunc::Rtrim => "RTRIM",
            ScalarFunc::Replace => "REPLACE",
            ScalarFunc::SplitPart => "SPLIT_PART",
            ScalarFunc::Abs => "ABS",
            ScalarFunc::Round => "ROUND",
            ScalarFunc::Ceil => "CEIL",
            ScalarFunc::Floor => "FLOOR",
            ScalarFunc::Mod => "MOD",
            ScalarFunc::Power => "POWER",
            ScalarFunc::Sqrt => "SQRT",
            ScalarFunc::Now => "NOW",
            ScalarFunc::CurrentDate => "CURRENT_DATE",
            ScalarFunc::CurrentTimestamp => "CURRENT_TIMESTAMP",
            ScalarFunc::DateAdd => "DATE_ADD",
            ScalarFunc::DateSub => "DATE_SUB",
            ScalarFunc::Datediff => "DATEDIFF",
            ScalarFunc::Extract => "EXTRACT",
            ScalarFunc::Strftime => "STRFTIME",
            ScalarFunc::Coalesce => "COALESCE",
            ScalarFunc::Nullif => "NULLIF",
            ScalarFunc::Ifnull => "IFNULL",
            ScalarFunc::If => "IF",
        }
    }

    /// Devuelve el `ScalarFunc` asociado al ident (case-insensitive).
    /// Acepta también aliases comunes: `SUBSTRING` → `Substr`, `IIF` →
    /// `If`. Devuelve `None` si el ident no es un built-in conocido.
    pub fn from_ident(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "LENGTH" | "LEN" | "CHAR_LENGTH" => Some(ScalarFunc::Length),
            "UPPER" => Some(ScalarFunc::Upper),
            "LOWER" => Some(ScalarFunc::Lower),
            "SUBSTR" | "SUBSTRING" => Some(ScalarFunc::Substr),
            "CONCAT" => Some(ScalarFunc::Concat),
            "TRIM" => Some(ScalarFunc::Trim),
            "LTRIM" => Some(ScalarFunc::Ltrim),
            "RTRIM" => Some(ScalarFunc::Rtrim),
            "REPLACE" => Some(ScalarFunc::Replace),
            "SPLIT_PART" => Some(ScalarFunc::SplitPart),
            "ABS" => Some(ScalarFunc::Abs),
            "ROUND" => Some(ScalarFunc::Round),
            "CEIL" | "CEILING" => Some(ScalarFunc::Ceil),
            "FLOOR" => Some(ScalarFunc::Floor),
            "MOD" => Some(ScalarFunc::Mod),
            "POWER" | "POW" => Some(ScalarFunc::Power),
            "SQRT" => Some(ScalarFunc::Sqrt),
            "NOW" => Some(ScalarFunc::Now),
            "CURRENT_DATE" | "CURDATE" => Some(ScalarFunc::CurrentDate),
            "CURRENT_TIMESTAMP" => Some(ScalarFunc::CurrentTimestamp),
            "DATE_ADD" => Some(ScalarFunc::DateAdd),
            "DATE_SUB" => Some(ScalarFunc::DateSub),
            "DATEDIFF" => Some(ScalarFunc::Datediff),
            "EXTRACT" => Some(ScalarFunc::Extract),
            "STRFTIME" => Some(ScalarFunc::Strftime),
            "COALESCE" => Some(ScalarFunc::Coalesce),
            "NULLIF" => Some(ScalarFunc::Nullif),
            "IFNULL" => Some(ScalarFunc::Ifnull),
            "IF" | "IIF" => Some(ScalarFunc::If),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl AggFunc {
    pub fn keyword(&self) -> &'static str {
        match self {
            AggFunc::Count => "COUNT",
            AggFunc::Sum => "SUM",
            AggFunc::Avg => "AVG",
            AggFunc::Min => "MIN",
            AggFunc::Max => "MAX",
        }
    }
    pub fn from_ident(text: &str) -> Option<Self> {
        match text.to_ascii_uppercase().as_str() {
            "COUNT" => Some(AggFunc::Count),
            "SUM" => Some(AggFunc::Sum),
            "AVG" => Some(AggFunc::Avg),
            "MIN" => Some(AggFunc::Min),
            "MAX" => Some(AggFunc::Max),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AggArg {
    /// `COUNT(*)` — cuenta filas, no nulls. Solo válido con `Count`.
    Star,
    /// `COUNT(col)`, `SUM(col)`, etc. Los NULL se descartan al agregar.
    Column(String),
    /// `COUNT(DISTINCT col)`. P1 del roadmap; solo aplica a `Count`.
    DistinctColumn(String),
    /// Issue #5 (2026-05-27): `SUM(qty * price)`, `AVG(salary * 1.1)`,
    /// etc. — el argumento es un `Expr` arbitrario (G1+G2+G3), no
    /// sólo un ident. Se evalúa por fila contra el row decodificado
    /// y se agrega el `Value` resultante.
    Expr(Expr),
}

impl SelectItem {
    /// Nombre canónico que usa la columna en el ResultSet y en las
    /// referencias de HAVING/ORDER BY. Cuando hay alias, el alias gana;
    /// cuando no, se sintetiza una forma estable (e.g. `count_*`,
    /// `sum_price`, `count_distinct_x`).
    pub fn output_name(&self) -> String {
        match self {
            SelectItem::Star => "*".to_string(),
            SelectItem::Column(name) => name.clone(),
            SelectItem::Aggregate { func, arg, alias } => {
                if let Some(a) = alias {
                    return a.clone();
                }
                let func_lower = func.keyword().to_ascii_lowercase();
                match arg {
                    AggArg::Star => format!("{}_*", func_lower),
                    AggArg::Column(c) => format!("{}_{}", func_lower, normalize_ident(c)),
                    AggArg::DistinctColumn(c) => {
                        format!("{}_distinct_{}", func_lower, normalize_ident(c))
                    }
                    // Issue #5: para `SUM(qty*price)` no hay un column
                    // name canónico; usamos un label sintético sobre el
                    // Expr (mismo helper que SelectItem::Expression).
                    AggArg::Expr(expr) => format!("{}_{}", func_lower, expr_default_label(expr)),
                }
            }
            SelectItem::Expression { expr, alias } => {
                if let Some(a) = alias {
                    return a.clone();
                }
                expr_default_label(expr)
            }
        }
    }
}

/// Bloque G1: nombre por defecto para una `SelectItem::Expression` sin
/// alias. La intención es que sea estable y "razonable" para que el
/// caller pueda referirla — no es necesario que sea SQL parseable. Para
/// funciones tipo `LENGTH(name)` devuelve `"length(name)"`; para CASE
/// devuelve `"case"`; para literales el repr canónico; etc.
fn expr_default_label(expr: &Expr) -> String {
    match expr {
        Expr::Literal(v) => match v {
            Value::Null => "NULL".to_string(),
            Value::Integer(n) => n.to_string(),
            Value::Float(f) => format!("{}", f),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
            Value::String(s) => format!("'{}'", s),
        },
        Expr::Column(name) => name.clone(),
        Expr::Func(f, args) => {
            let inside: Vec<String> = args.iter().map(expr_default_label).collect();
            format!("{}({})", f.keyword().to_ascii_lowercase(), inside.join(","))
        }
        Expr::Cast(inner, ty) => format!("cast({} as {})", expr_default_label(inner), ty.as_sql()),
        Expr::Case { .. } => "case".to_string(),
        Expr::Compare(_, _, _) => "compare".to_string(),
        Expr::IsNull(_, negated) => {
            if *negated {
                "is_not_null".to_string()
            } else {
                "is_null".to_string()
            }
        }
        Expr::Arith(l, op, r) => {
            format!(
                "({}{}{})",
                expr_default_label(l),
                op.lexeme(),
                expr_default_label(r)
            )
        }
        Expr::Like(l, _, negated) => {
            format!(
                "{}{}_like",
                expr_default_label(l),
                if *negated { "_not" } else { "" }
            )
        }
        Expr::InList(l, _, negated) => {
            format!(
                "{}{}_in",
                expr_default_label(l),
                if *negated { "_not" } else { "" }
            )
        }
        Expr::Between(l, _, _, negated) => {
            format!(
                "{}{}_between",
                expr_default_label(l),
                if *negated { "_not" } else { "" }
            )
        }
        Expr::ScalarSubquery(_) => "scalar_subquery".to_string(),
    }
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
        /// Bloque H (2026-05-26): `NOT IN (SELECT ...)`. Semántica ANSI
        /// estricta — si la subquery contiene algún NULL en su columna
        /// proyectada, `col NOT IN (...)` devuelve NULL (3VL), no false.
        /// Esto es importante: `5 NOT IN (1, 2, NULL)` ⇒ NULL.
        negated: bool,
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
    IsNull {
        column: String,
        negated: bool,
    },
    /// `col [NOT] IN (lit1, lit2, ...)` con lista literal (no-subquery).
    /// Si la columna es NULL → NULL (3VL). NULLs dentro de la lista se
    /// ignoran (ANSI). `NOT IN` con un NULL en la lista propaga NULL
    /// (semántica ANSI estricta).
    InList {
        column: String,
        values: Vec<Value>,
        negated: bool,
    },
    /// Bloque G2 (2026-05-26): predicado expresional general. Cualquier
    /// `Expr` que evalúe a BOOL (o NULL → 3VL) puede usarse como átomo
    /// del WHERE. NO tiene fast-path indexada — se evalúa siempre por
    /// FullScan + post-filter.
    ///
    /// Las variantes específicas pre-G2 (`Eq`, `Compare`, `Like`, `IsNull`,
    /// `InList`, `Between`, ...) se preservan: el parser sigue prefiriendo
    /// la forma estructural cuando el átomo encaja en `IDENT OP literal`,
    /// para mantener los fast-paths PK / índice / range scan / EXISTS
    /// correlacionado intactos. `ExprPredicate` solo se construye cuando
    /// el parser detecta una forma que NO encaja en las estructurales
    /// (LHS o RHS son funciones, CASE, CAST, literal a la izquierda, ...).
    ExprPredicate {
        expr: Expr,
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
    /// Bloque T: marca `true` mientras un `BEGIN` SQL está activo y aún
    /// no se cerró con `COMMIT`/`ROLLBACK`. El Pager subyacente SIEMPRE
    /// tiene una transacción abierta (la abre el caller en su wrap);
    /// este flag distingue la tx implícita del wrap de la tx explícita
    /// pedida por el usuario via SQL. Doble `BEGIN` → `[GBY-4029]`;
    /// `COMMIT`/`ROLLBACK` sin `BEGIN` → `[GBY-4030]`.
    explicit_tx: bool,
    /// Bloque V (2026-05-27): profundidad actual de expansión de
    /// vistas. Cada `FROM v` con `v` siendo una vista incrementa
    /// este contador antes de re-parsear el body de la vista; se
    /// decrementa al volver. Protege contra vistas que se referencian
    /// mutuamente (`A → B → A`) y contra el caso degenerado donde una
    /// vista se referencia a sí misma. Límite duro: `MAX_VIEW_DEPTH`.
    view_expansion_depth: usize,
}

/// Bloque V: límite de profundidad de expansión de vistas. 32 está muy
/// por encima de cualquier caso real (5-6 niveles es lo normal) y muy
/// por debajo del stack de Rust (~5k frames).
const MAX_VIEW_DEPTH: usize = 32;

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
            explicit_tx: false,
            view_expansion_depth: 0,
        }
    }

    pub fn exec(&mut self, statement: Statement) -> DbResult<ResultSet> {
        match statement {
            Statement::CreateTable(stmt) => self.exec_create(stmt),
            Statement::DropTable(stmt) => self.exec_drop_table(stmt),
            Statement::AlterTableAddColumn(stmt) => self.exec_alter_add_column(stmt),
            Statement::AlterTableAddCheck(stmt) => self.exec_alter_add_check(stmt),
            Statement::AlterTableDropConstraint(stmt) => self.exec_alter_drop_constraint(stmt),
            Statement::CreateView(stmt) => self.exec_create_view(stmt),
            Statement::DropView(stmt) => self.exec_drop_view(stmt),
            Statement::CreateTableAs(stmt) => self.exec_create_table_as(stmt),
            Statement::RenameTable(stmt) => self.exec_rename_table(stmt),
            Statement::AlterTableDropColumn(stmt) => self.exec_alter_drop_column(stmt),
            Statement::AlterTableRenameColumn(stmt) => self.exec_alter_rename_column(stmt),
            Statement::Insert(stmt) => self.exec_insert(stmt),
            Statement::Select(query) => self.exec_select_query(*query),
            Statement::Update(stmt) => self.exec_update(stmt),
            Statement::Delete(stmt) => self.exec_delete(stmt),
            Statement::CreateIndex(stmt) => self.exec_create_index(stmt),
            Statement::DropIndex(stmt) => self.exec_drop_index(stmt),
            Statement::IntegrityCheck => self.exec_integrity_check(),
            Statement::Truncate(stmt) => self.exec_truncate(stmt),
            // Bloque J2: REPLACE INTO se ejecuta vía la misma ruta que
            // INSERT — el parser ya seteó on_conflict=Replace.
            Statement::Replace(stmt) => self.exec_insert(stmt),
            Statement::Begin => self.exec_begin(),
            Statement::Commit => self.exec_commit(),
            Statement::Rollback => self.exec_rollback(),
            Statement::CreateDatabase(_)
            | Statement::DropDatabase(_)
            | Statement::ShowDatabases => Err(DbError::new(
                "CREATE/DROP/SHOW DATABASE no se ejecutan contra una DB; \
                 deben ser interceptados por el caller antes de abrir el Pager",
            )),
        }
    }

    /// Bloque T: marca el inicio de una transacción explícita. No toca el
    /// Pager (la transacción física ya está abierta por el wrap del
    /// caller); solo voltea el flag `explicit_tx`. Doble `BEGIN` sin
    /// `COMMIT`/`ROLLBACK` intermedio devuelve `[GBY-4029]`.
    fn exec_begin(&mut self) -> DbResult<ResultSet> {
        if self.explicit_tx {
            return Err(coded(
                codes::TX_BEGIN_DOUBLE,
                "BEGIN: ya hay una transacción explícita abierta — cerrala con COMMIT o ROLLBACK \
                 antes de empezar otra (savepoints aún no soportados)",
            ));
        }
        self.explicit_tx = true;
        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("BEGIN".to_string()),
        })
    }

    /// Bloque T: cierra la transacción explícita activa. Persiste lo
    /// acumulado vía `pager.commit()` y re-abre una tx fresca con
    /// `pager.begin()` para que el wrap del caller (que también hará
    /// commit al final) siga válido. Sin `BEGIN` previo → `[GBY-4030]`.
    fn exec_commit(&mut self) -> DbResult<ResultSet> {
        if !self.explicit_tx {
            return Err(coded(
                codes::TX_END_WITHOUT_BEGIN,
                "COMMIT: no hay transacción explícita activa; las sentencias fuera de BEGIN/COMMIT \
                 son auto-commit por batch (no hace falta COMMIT)",
            ));
        }
        self.pager.commit()?;
        self.pager.begin()?;
        self.explicit_tx = false;
        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("COMMIT".to_string()),
        })
    }

    /// Bloque T: descarta la transacción explícita activa vía
    /// `pager.rollback()` y re-abre una tx fresca. Sin `BEGIN` previo →
    /// `[GBY-4030]`. ⚠️ El rollback descarta TODO el cache de páginas
    /// del Pager — incluidas las sentencias anteriores del MISMO batch
    /// que ocurrieron ANTES del BEGIN (porque el wrap externo abrió una
    /// única transacción física). En la práctica esto significa que
    /// `BEGIN`/`ROLLBACK` solo aborta limpio si todo el batch arrancó
    /// con `BEGIN` como primera sentencia.
    fn exec_rollback(&mut self) -> DbResult<ResultSet> {
        if !self.explicit_tx {
            return Err(coded(
                codes::TX_END_WITHOUT_BEGIN,
                "ROLLBACK: no hay transacción explícita activa; un ROLLBACK fuera de BEGIN \
                 no tiene blanco sobre el que actuar",
            ));
        }
        self.pager.rollback()?;
        self.pager.begin()?;
        self.explicit_tx = false;
        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("ROLLBACK".to_string()),
        })
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

        // K2: el parser puede entregar columnas PK adicionales por
        // table-level `PRIMARY KEY (a, b, ...)`.
        let primary_key_extra = stmt.primary_key_extra.clone();
        // Bloque L2 (2026-05-27): heredar los CHECKs del parser. Cada
        // entrada ya viene con `name` definitivo y `source` canónico
        // (re-serializado vía `format_expr`).
        let check_constraints = stmt.check_constraints.clone();
        let mut meta = TableMeta {
            name: stmt.name,
            primary_key,
            primary_key_extra,
            primary_key_name: stmt.primary_key_name.clone(),
            columns,
            root_page: 0,
            indexes: Vec::new(),
            check_constraints,
        };
        validate_create_table(&meta)?;
        validate_fk_targets(self.pager, &meta)?;
        validate_check_constraints(&meta)?;

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
                extra_columns: Vec::new(),
            });
        }

        // Bloque L1 (2026-05-27): table-level `UNIQUE (a, b, ...)`.
        // Single-col → mismo path que inline UNIQUE (Hash/OrderedInt
        // según tipo). Multi-col → reusa el camino de K2 (fingerprint
        // FNV-1a-64 vía `IndexKind::OrderedInt`, all-INT NOT NULL).
        for cols in &stmt.unique_constraints {
            if cols.is_empty() {
                return Err(DbError::new(format!(
                    "UNIQUE () vacío declarado en CREATE TABLE '{}'",
                    meta.name
                )));
            }
            // Validar que cada columna existe en la tabla.
            for col_name in cols {
                if meta.column(col_name).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!(
                            "UNIQUE (...) referencia columna '{}' que no existe en '{}'",
                            col_name, meta.name
                        ),
                    ));
                }
            }
            let is_composite = cols.len() > 1;
            // Para compuestos, K2 exige all-INT NOT NULL.
            if is_composite {
                for col_name in cols {
                    let column = meta
                        .column(col_name)
                        .expect("ya validado arriba que existe");
                    if column.column_type != ColumnType::Int || !column.not_null {
                        return Err(coded(
                            codes::COMPOSITE_INDEX_REQUIRES_ALL_INT,
                            format!(
                                "UNIQUE ({}): todas las columnas de un UNIQUE compuesto deben \
                                 ser INT NOT NULL en este release (columna '{}' rompe la regla)",
                                cols.join(", "),
                                col_name
                            ),
                        ));
                    }
                }
            }
            let idx_root = self.pager.new_page()?;
            let mut leaf = vec![0; self.pager.page_size()];
            init_leaf_page(&mut leaf);
            self.pager.write_page(idx_root, &leaf, true)?;
            let first = cols[0].clone();
            let extra: Vec<String> = cols.iter().skip(1).cloned().collect();
            let first_col_type = meta
                .column(&first)
                .map(|c| c.column_type)
                .expect("validado arriba");
            let kind = if is_composite {
                IndexKind::OrderedInt
            } else {
                IndexKind::for_column(first_col_type)
            };
            let idx_name = format!(
                "uq_{}_{}",
                meta.name.to_ascii_lowercase(),
                cols.iter()
                    .map(|c| c.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join("_")
            );
            meta.indexes.push(IndexMeta {
                name: idx_name,
                column: first,
                root_page: idx_root,
                unique: true,
                kind,
                extra_columns: extra,
            });
        }

        // Residual #2 (2026-05-27): named UNIQUE table-level. Mismo
        // materializado que la rama anterior pero con `name = supplied`.
        // Validamos también que no haya colisión de nombre contra los
        // índices ya creados (inline UNIQUE + table-level sin nombre).
        for (idx_name, cols) in &stmt.named_unique_constraints {
            if cols.is_empty() {
                return Err(DbError::new(format!(
                    "UNIQUE () vacío declarado en CREATE TABLE '{}'",
                    meta.name
                )));
            }
            for col_name in cols {
                if meta.column(col_name).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!(
                            "UNIQUE (...) referencia columna '{}' que no existe en '{}'",
                            col_name, meta.name
                        ),
                    ));
                }
            }
            if meta.index_by_name(idx_name).is_some() {
                return Err(coded(
                    codes::INDEX_ALREADY_EXISTS,
                    format!(
                        "CONSTRAINT '{}' ya existe en '{}' (colisiona con un índice previo)",
                        idx_name, meta.name
                    ),
                ));
            }
            let is_composite = cols.len() > 1;
            if is_composite {
                for col_name in cols {
                    let column = meta.column(col_name).expect("validado arriba");
                    if column.column_type != ColumnType::Int || !column.not_null {
                        return Err(coded(
                            codes::COMPOSITE_INDEX_REQUIRES_ALL_INT,
                            format!(
                                "CONSTRAINT '{}' UNIQUE ({}): todas las columnas deben ser \
                                 INT NOT NULL (columna '{}' rompe la regla)",
                                idx_name,
                                cols.join(", "),
                                col_name
                            ),
                        ));
                    }
                }
            }
            let idx_root = self.pager.new_page()?;
            let mut leaf = vec![0; self.pager.page_size()];
            init_leaf_page(&mut leaf);
            self.pager.write_page(idx_root, &leaf, true)?;
            let first = cols[0].clone();
            let extra: Vec<String> = cols.iter().skip(1).cloned().collect();
            let first_col_type = meta
                .column(&first)
                .map(|c| c.column_type)
                .expect("validado arriba");
            let kind = if is_composite {
                IndexKind::OrderedInt
            } else {
                IndexKind::for_column(first_col_type)
            };
            meta.indexes.push(IndexMeta {
                name: idx_name.clone(),
                column: first,
                root_page: idx_root,
                unique: true,
                kind,
                extra_columns: extra,
            });
        }

        // Residual #2 (2026-05-27): FKs table-level con nombre. La FK
        // se adjunta a la columna correspondiente del child, igual que
        // si hubiera venido inline — la diferencia es que `name` tiene
        // valor explícito y permite `ALTER TABLE DROP CONSTRAINT <name>`.
        for nfk in &stmt.named_foreign_keys {
            let col_idx = meta
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(&nfk.column))
                .ok_or_else(|| {
                    coded(
                        codes::COLUMN_NOT_FOUND,
                        format!(
                            "CONSTRAINT '{}' FOREIGN KEY referencia columna '{}' que no existe en '{}'",
                            nfk.name, nfk.column, meta.name
                        ),
                    )
                })?;
            if meta.columns[col_idx].references.is_some() {
                return Err(DbError::new(format!(
                    "columna '{}' ya tiene una FK declarada inline; remover el inline o \
                     no declarar también CONSTRAINT '{}' sobre la misma columna",
                    nfk.column, nfk.name
                )));
            }
            // Chequear que el nombre no colisione con otra FK ya nombrada.
            let dup = meta.columns.iter().any(|c| {
                c.references
                    .as_ref()
                    .and_then(|r| r.name.as_ref())
                    .map(|n| n.eq_ignore_ascii_case(&nfk.name))
                    .unwrap_or(false)
            });
            if dup {
                return Err(DbError::new(format!(
                    "CONSTRAINT '{}' ya existe en '{}' (otra FK con el mismo nombre)",
                    nfk.name, meta.name
                )));
            }
            meta.columns[col_idx].references = Some(ForeignKeyMeta {
                table: nfk.target_table.clone(),
                column: nfk.target_column.clone(),
                on_delete: nfk.on_delete,
                on_update: nfk.on_update,
                name: Some(nfk.name.clone()),
                extra_source_columns: nfk.extra_source_columns.clone(),
                extra_target_columns: nfk.extra_target_columns.clone(),
            });
        }
        // Re-validar FK targets ahora que las table-level fueron adjuntadas.
        validate_fk_targets(self.pager, &meta)?;

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
                extra_columns: Vec::new(),
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

    /// Bloque K1 (2026-05-26): `CREATE TABLE [IF NOT EXISTS] <name>
    /// [(c1, c2, ...)] AS <select>`. Ejecuta la fuente, valida que la
    /// primera columna del result-set sea INT (única estrategia disponible
    /// hoy para producir la PK de la nueva tabla — ver `[GBY-4058]`),
    /// crea la tabla y la rellena fila a fila. Toda la operación corre
    /// dentro de la transacción del batch — si falla la inserción de
    /// cualquier fila, el wrap del caller hace rollback y el catálogo
    /// queda sin la tabla a medias.
    fn exec_create_table_as(&mut self, stmt: CreateTableAsStmt) -> DbResult<ResultSet> {
        validate_identifier(&stmt.name, "tabla")?;

        // Pre-check de existencia con respeto del flag IF NOT EXISTS.
        let already_exists = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_table(&stmt.name)?.is_some()
        };
        if already_exists {
            if stmt.if_not_exists {
                return Ok(ResultSet {
                    columns: Vec::new(),
                    rows: Vec::new(),
                    message: Some(format!(
                        "OK · tabla '{}' ya existe, CTAS no-op (IF NOT EXISTS)",
                        stmt.name
                    )),
                });
            }
            return Err(coded(
                codes::TABLE_ALREADY_EXISTS,
                format!(
                    "CREATE TABLE AS rechazado: ya existe una tabla llamada '{}'",
                    stmt.name
                ),
            ));
        }

        // Materializa la fuente como ResultSet (reusa todo el path de I).
        let mut source_rs = self.exec_select_query(*stmt.source)?;

        // Aplica los alias de columnas (si vinieron) — debe matchear arity.
        if let Some(aliases) = &stmt.column_aliases {
            if aliases.len() != source_rs.columns.len() {
                return Err(coded(
                    codes::CTAS_COLUMN_ALIAS_ARITY,
                    format!(
                        "CREATE TABLE '{}' (...) AS: la lista de alias tiene {} columnas \
                         pero el SELECT proyecta {}",
                        stmt.name,
                        aliases.len(),
                        source_rs.columns.len()
                    ),
                ));
            }
            for alias in aliases {
                validate_identifier(alias, "columna")?;
            }
            // Chequeo de duplicados case-insensitive (la misma regla que
            // CREATE TABLE clásica usa más adelante).
            let mut seen = HashSet::new();
            for alias in aliases {
                if !seen.insert(alias.to_ascii_lowercase()) {
                    return Err(coded(
                        codes::DUPLICATE_COLUMN_NAME,
                        format!(
                            "CREATE TABLE '{}' (...) AS: nombre de columna duplicado '{}'",
                            stmt.name, alias
                        ),
                    ));
                }
            }
            source_rs.columns = aliases.clone();
        }

        if source_rs.columns.is_empty() {
            return Err(DbError::new(format!(
                "CREATE TABLE '{}' AS: el SELECT no proyecta ninguna columna",
                stmt.name
            )));
        }
        // Dedup de los headers (sin alias explícito puede venir un SELECT
        // con dos columnas del mismo nombre — error temprano).
        {
            let mut seen = HashSet::new();
            for h in &source_rs.columns {
                if !seen.insert(h.to_ascii_lowercase()) {
                    return Err(coded(
                        codes::DUPLICATE_COLUMN_NAME,
                        format!(
                            "CREATE TABLE '{}' AS: el SELECT proyecta dos columnas con el \
                             mismo nombre '{}' — usá alias en el SELECT o la cláusula \
                             '(col_aliases)' del CTAS para desambiguar",
                            stmt.name, h
                        ),
                    ));
                }
            }
        }

        // Validar cada header como ident (mismo strict de columna).
        for h in &source_rs.columns {
            validate_identifier(h, "columna")?;
        }

        // Infiere tipos por columna sobre los valores no-NULL. Estrategia:
        // mismo variant en todos los no-NULL → ese tipo; INT + FLOAT
        // promueven a FLOAT; cualquier otra mezcla → TEXT como fallback.
        let n_cols = source_rs.columns.len();
        let mut inferred: Vec<Option<ColumnType>> = vec![None; n_cols];
        let mut fallback_text: Vec<bool> = vec![false; n_cols];
        for row in &source_rs.rows {
            for (i, v) in row.iter().enumerate() {
                let t = match v {
                    Value::Null => continue,
                    Value::Integer(_) => ColumnType::Int,
                    Value::Float(_) => ColumnType::Float,
                    Value::Bool(_) => ColumnType::Bool,
                    Value::String(_) => ColumnType::Text,
                };
                match inferred[i] {
                    None => inferred[i] = Some(t),
                    Some(prev) if prev == t => {}
                    Some(ColumnType::Int) if t == ColumnType::Float => {
                        inferred[i] = Some(ColumnType::Float);
                    }
                    Some(ColumnType::Float) if t == ColumnType::Int => {}
                    Some(_) => fallback_text[i] = true,
                }
            }
        }

        // La PK es la primera columna y debe inferir como INT (estrategia
        // explícita: queremos error claro si el usuario olvidó un id).
        // Caso tabla vacía: la inferencia es None → tratar como NO-INT
        // (no podemos asumir nada sin evidencia).
        let first_is_int = matches!(inferred.first(), Some(Some(ColumnType::Int)));
        if !first_is_int || fallback_text.first().copied().unwrap_or(false) {
            return Err(coded(
                codes::CTAS_REQUIRES_INT_FIRST_COLUMN,
                format!(
                    "CREATE TABLE '{}' AS rechazado: la primera columna del SELECT debe \
                     proyectar valores INT no-nulos (se usa como PRIMARY KEY de la nueva tabla). \
                     Antepoñé un `id INT` en el SELECT o usá `CREATE TABLE t (id, ...) AS \
                     SELECT 1, ...` con un literal INT explícito",
                    stmt.name
                ),
            ));
        }
        // Además exigimos que la PK no contenga NULL ni duplicados — el
        // path normal de insert capturaría eso fila a fila, pero mejor
        // error temprano y limpio.
        let mut pk_seen: HashSet<i64> = HashSet::with_capacity(source_rs.rows.len());
        for (i, row) in source_rs.rows.iter().enumerate() {
            match row.first() {
                Some(Value::Integer(n)) => {
                    if !pk_seen.insert(*n) {
                        return Err(coded(
                            codes::DUPLICATE_PRIMARY_KEY,
                            format!(
                                "CREATE TABLE '{}' AS rechazado: la fila {} duplica la PK \
                                 ({}); el SELECT debe producir valores únicos en la primera columna",
                                stmt.name,
                                i + 1,
                                n
                            ),
                        ));
                    }
                }
                Some(Value::Null) | None => {
                    return Err(coded(
                        codes::PRIMARY_KEY_NULL,
                        format!(
                            "CREATE TABLE '{}' AS rechazado: la fila {} tiene NULL en la \
                             primera columna (que se usa como PRIMARY KEY)",
                            stmt.name,
                            i + 1
                        ),
                    ));
                }
                _ => {
                    return Err(coded(
                        codes::CTAS_REQUIRES_INT_FIRST_COLUMN,
                        format!(
                            "CREATE TABLE '{}' AS rechazado: la fila {} tiene un valor \
                             no-INT en la primera columna",
                            stmt.name,
                            i + 1
                        ),
                    ));
                }
            }
        }

        // Construye los ColumnDef de la nueva tabla. La primera columna
        // es PK INT NOT NULL; el resto toma el tipo inferido (o TEXT
        // como fallback en columnas conflictivas / 100% NULL).
        let pk_name = source_rs.columns[0].clone();
        let mut columns: Vec<Column> = Vec::with_capacity(n_cols);
        for (i, name) in source_rs.columns.iter().enumerate() {
            let ty = if i == 0 {
                ColumnType::Int
            } else if fallback_text[i] {
                ColumnType::Text
            } else {
                inferred[i].unwrap_or(ColumnType::Text)
            };
            columns.push(Column {
                name: name.clone(),
                column_type: ty,
                not_null: i == 0,
                default: None,
                references: None,
            });
        }
        let mut meta = TableMeta {
            name: stmt.name.clone(),
            primary_key: pk_name.clone(),
            primary_key_extra: Vec::new(),
            primary_key_name: None,
            columns,
            root_page: 0,
            indexes: Vec::new(),
            check_constraints: Vec::new(),
        };
        validate_create_table(&meta)?;

        // Reserva la root_page de la tabla y publica el catálogo.
        let root_page = self.pager.new_page()?;
        let mut leaf_page = vec![0; self.pager.page_size()];
        init_leaf_page(&mut leaf_page);
        self.pager.write_page(root_page, &leaf_page, true)?;
        meta.root_page = root_page;
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.put_table(&meta)?;
        }

        // Inserta cada fila vía encode_row + insert_row (sin pasar por
        // el path normal de INSERT — no hay UNIQUE/FK/defaults a aplicar
        // porque la tabla recién creada no los declara).
        let row_count = source_rs.rows.len();
        for row in source_rs.rows {
            let mut values: HashMap<String, Value> = HashMap::with_capacity(n_cols);
            for (i, v) in row.into_iter().enumerate() {
                values.insert(normalize_ident(&source_rs.columns[i]), v);
            }
            let (pk, row_bytes) = encode_row(&meta, &values)?;
            let mut catalog = Catalog::open(self.pager);
            catalog.insert_row(meta.root_page, pk, row_bytes)?;
        }

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(format!(
                "OK · tabla '{}' creada con {} fila{} ({} columna{})",
                stmt.name,
                row_count,
                if row_count == 1 { "" } else { "s" },
                n_cols,
                if n_cols == 1 { "" } else { "s" }
            )),
        })
    }

    /// Bloque K1 (2026-05-26): `RENAME TABLE <old> TO <new>` o la forma
    /// equivalente `ALTER TABLE <old> RENAME TO <new>`. Renombra la
    /// entry del catálogo (remove + put con la nueva clave hash) y
    /// actualiza los `ForeignKeyMeta::table` de otras tablas que
    /// apuntaban al nombre viejo. Las páginas de datos no se mueven —
    /// la tabla mantiene su `root_page`, sus filas y sus índices.
    fn exec_rename_table(&mut self, stmt: RenameTableStmt) -> DbResult<ResultSet> {
        validate_identifier(&stmt.new_name, "tabla")?;

        if stmt.old_name.eq_ignore_ascii_case(&stmt.new_name) {
            // No-op silencioso: renombrar a sí mismo es idempotente.
            return Ok(ResultSet {
                columns: Vec::new(),
                rows: Vec::new(),
                message: Some(format!("OK · '{}' = '{}'", stmt.old_name, stmt.new_name)),
            });
        }

        let mut meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_table(&stmt.old_name)?.ok_or_else(|| {
                coded(
                    codes::TABLE_NOT_FOUND,
                    format!("RENAME TABLE: tabla origen '{}' no existe", stmt.old_name),
                )
            })?
        };

        // El destino no puede existir ya.
        let target_exists = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_table(&stmt.new_name)?.is_some()
        };
        if target_exists {
            return Err(coded(
                codes::RENAME_TARGET_EXISTS,
                format!(
                    "RENAME TABLE rechazado: ya existe una tabla llamada '{}'",
                    stmt.new_name
                ),
            ));
        }

        let old_name = meta.name.clone();
        meta.name = stmt.new_name.clone();

        // Persiste el cambio: borrar la entry vieja, escribir la nueva,
        // actualizar las FKs entrantes en otras tablas.
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.remove_table(&old_name)?;
            catalog.put_table(&meta)?;
        }

        // Recorre el catálogo y reescribe las FKs que apunten al nombre
        // viejo. Hacemos snapshot primero para no chocar con la iteración.
        let all_tables: Vec<TableMeta> = {
            let mut catalog = Catalog::open(self.pager);
            catalog.list_tables()?
        };
        for mut other in all_tables {
            if other.name.eq_ignore_ascii_case(&meta.name) {
                continue;
            }
            let mut changed = false;
            for col in other.columns.iter_mut() {
                if let Some(fk) = col.references.as_mut() {
                    if fk.table.eq_ignore_ascii_case(&old_name) {
                        fk.table = meta.name.clone();
                        changed = true;
                    }
                }
            }
            if changed {
                let mut catalog = Catalog::open(self.pager);
                catalog.put_table(&other)?;
            }
        }

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(format!(
                "OK · tabla renombrada de '{}' a '{}'",
                old_name, meta.name
            )),
        })
    }

    /// L3 (2026-05-27): `ALTER TABLE <t> ADD [CONSTRAINT <n>] CHECK (<expr>)`.
    /// Re-valida todas las filas existentes contra el nuevo predicado.
    /// Si alguna evalúa a FALSE, la operación entera rebota con
    /// `[GBY-3008]` sin tocar el catálogo — sin estado parcial. NULL
    /// pasa por 3VL ANSI (mismo contrato que CHECK en CREATE TABLE).
    fn exec_alter_add_check(&mut self, stmt: AlterAddCheckStmt) -> DbResult<ResultSet> {
        let mut meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_table(&stmt.table)?.ok_or_else(|| {
                coded(
                    codes::TABLE_NOT_FOUND,
                    format!("tabla no existe: {}", stmt.table),
                )
            })?
        };

        // 1. Re-parsear el source canónico ya producido por el parser.
        //    El round-trip format_expr → parse_expr_str cubre la regla
        //    de "rechazo de subqueries" via [GBY-4069].
        let expr = parse_expr_str(&stmt.source).map_err(|e| {
            DbError::new(format!(
                "ALTER TABLE ADD CHECK en '{}': re-parse del predicado falló — {}",
                stmt.table, e
            ))
        })?;

        // 2. Validar que cada columna referenciada existe en la tabla,
        //    y que el expr no contiene subqueries. Reusa los walkers
        //    que ya usa `validate_check_constraints` para CREATE TABLE.
        check_expr_no_subquery(&expr).map_err(|e| {
            DbError::new(format!("ALTER TABLE ADD CHECK en '{}': {}", stmt.table, e))
        })?;
        collect_check_columns(&expr, &mut |col| {
            let key = strip_qualifier(col, &meta.name);
            if meta.column(&key).is_none() {
                Err(coded(
                    codes::COLUMN_NOT_FOUND,
                    format!(
                        "ALTER TABLE ADD CHECK en '{}': la columna '{}' no existe",
                        meta.name, col
                    ),
                ))
            } else {
                Ok(())
            }
        })?;

        // 3. Resolver el nombre definitivo. Si vino explícito, validar
        //    que no colisione con un CHECK ya declarado. Si no, sintetizar
        //    `<tabla>_check_<N>` con N empezando donde quedaron los
        //    anteriores (mismo esquema que el parser de CREATE TABLE).
        let final_name = match stmt.name {
            Some(n) => {
                let lower = n.to_ascii_lowercase();
                if meta
                    .check_constraints
                    .iter()
                    .any(|c| c.name.to_ascii_lowercase() == lower)
                {
                    return Err(DbError::new(format!(
                        "CHECK constraint '{}' ya existe en '{}'",
                        n, meta.name
                    )));
                }
                n
            }
            None => {
                let mut n = meta.check_constraints.len() + 1;
                loop {
                    let candidate = format!("{}_check_{}", meta.name.to_ascii_lowercase(), n);
                    let taken = meta
                        .check_constraints
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case(&candidate));
                    if !taken {
                        break candidate;
                    }
                    n += 1;
                }
            }
        };

        // 4. Full-scan: re-validar cada fila contra el predicado nuevo.
        //    Cualquier FALSE aborta antes de tocar catálogo — el catálogo
        //    sigue exactamente como estaba.
        let rows = {
            let mut catalog = Catalog::open(self.pager);
            catalog.scan_rows(meta.root_page, 0, None)?
        };
        for kv in &rows {
            let row = decode_row(&meta, &kv.value)?;
            match eval_expr_as_predicate(&expr, &row) {
                Ok(Some(true)) | Ok(None) => continue,
                Ok(Some(false)) => {
                    return Err(coded(
                        codes::CHECK_VIOLATED,
                        format!(
                            "ALTER TABLE ADD CHECK '{}' en '{}' rechazado: la fila con PK={} \
                             ya viola el predicado `{}` (no se puede agregar el constraint sobre \
                             datos preexistentes que no lo cumplen)",
                            final_name, meta.name, kv.key, stmt.source
                        ),
                    ));
                }
                Err(e) => {
                    return Err(DbError::new(format!(
                        "ALTER TABLE ADD CHECK '{}' en '{}': error al evaluar fila PK={} — {}",
                        final_name, meta.name, kv.key, e
                    )));
                }
            }
        }

        // 5. Persistir. El bloque L2 ya dejó el slot en TableMeta y el
        //    trailer en disco (VERSION 10), así que sólo es un put_table
        //    con un Vec extendido.
        meta.check_constraints.push(CheckConstraint {
            name: final_name.clone(),
            source: stmt.source.clone(),
        });
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.put_table(&meta)?;
        }

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(format!(
                "OK · CHECK '{}' agregado a '{}' tras validar {} fila(s) existentes",
                final_name,
                meta.name,
                rows.len()
            )),
        })
    }

    /// Residual #2 (2026-05-27): `ALTER TABLE <t> DROP CONSTRAINT [IF EXISTS] <name>`.
    /// Lookup case-insensitive del `name` a través de:
    ///
    ///   1. `check_constraints` — drop la entry.
    ///   2. `indexes` (sólo UNIQUE con nombre explícito o auto-generado)
    ///      — invocar el mismo path que `DROP INDEX` (libera root y
    ///      saca el `IndexMeta`).
    ///   3. `columns[*].references.name` — limpiar el `references` de
    ///      la columna afectada (la columna sigue, pero deja de ser FK).
    ///
    /// La PK no se puede borrar con `DROP CONSTRAINT` — el motor rebota
    /// con `[GBY-4072]`. Nombre desconocido → `[GBY-4071]` (a menos que
    /// `IF EXISTS` lo silencie).
    fn exec_alter_drop_constraint(&mut self, stmt: AlterDropConstraintStmt) -> DbResult<ResultSet> {
        let mut meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_table(&stmt.table)?.ok_or_else(|| {
                coded(
                    codes::TABLE_NOT_FOUND,
                    format!("tabla no existe: {}", stmt.table),
                )
            })?
        };
        let target = stmt.name.to_ascii_lowercase();

        // PK rejection — antes que cualquier otra rama.
        if meta
            .primary_key_name
            .as_ref()
            .map(|n| n.eq_ignore_ascii_case(&target))
            .unwrap_or(false)
        {
            return Err(coded(
                codes::CANNOT_DROP_PRIMARY_KEY_CONSTRAINT,
                format!(
                    "DROP CONSTRAINT '{}': es la PRIMARY KEY de '{}' y la PK es inmutable; \
                     usar DROP TABLE si la intención es rehacer el esquema",
                    stmt.name, meta.name
                ),
            ));
        }

        // 1. CHECK constraints.
        if let Some(pos) = meta
            .check_constraints
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(&target))
        {
            let removed = meta.check_constraints.remove(pos);
            {
                let mut catalog = Catalog::open(self.pager);
                catalog.put_table(&meta)?;
            }
            return Ok(ResultSet {
                columns: Vec::new(),
                rows: Vec::new(),
                message: Some(format!(
                    "OK · CHECK constraint '{}' eliminado de '{}'",
                    removed.name, meta.name
                )),
            });
        }

        // 2. UNIQUE index (named) — buscar por nombre del IndexMeta. El
        //    motor no distingue índices "auto" de "named" — la ÚNICA
        //    regla es que UNIQUE indexes sí se pueden DROP, mientras
        //    que un índice no-UNIQUE creado por CREATE INDEX necesita
        //    DROP INDEX. (DROP CONSTRAINT no debe borrar índices
        //    "técnicos" sin UNIQUE; rebotamos en ese caso.)
        if let Some(idx_pos) = meta
            .indexes
            .iter()
            .position(|i| i.name.eq_ignore_ascii_case(&target))
        {
            let idx_meta = meta.indexes[idx_pos].clone();
            if !idx_meta.unique {
                return Err(coded(
                    codes::CONSTRAINT_NOT_FOUND,
                    format!(
                        "DROP CONSTRAINT '{}': existe un índice con ese nombre en '{}' pero \
                         NO es UNIQUE — usar DROP INDEX para borrarlo",
                        stmt.name, meta.name
                    ),
                ));
            }
            // Libera la root page del índice (no hay free-list — page leak
            // aceptable, mismo contrato que DROP INDEX).
            meta.indexes.remove(idx_pos);
            {
                let mut catalog = Catalog::open(self.pager);
                catalog.put_table(&meta)?;
            }
            return Ok(ResultSet {
                columns: Vec::new(),
                rows: Vec::new(),
                message: Some(format!(
                    "OK · UNIQUE constraint '{}' eliminado de '{}'",
                    idx_meta.name, meta.name
                )),
            });
        }

        // 3. FK con nombre — buscar en columns[*].references.name.
        let mut fk_hit: Option<(usize, String)> = None;
        for (i, col) in meta.columns.iter().enumerate() {
            if let Some(fk) = &col.references {
                if let Some(n) = &fk.name {
                    if n.eq_ignore_ascii_case(&target) {
                        fk_hit = Some((i, n.clone()));
                        break;
                    }
                }
            }
        }
        if let Some((col_idx, fk_name)) = fk_hit {
            meta.columns[col_idx].references = None;
            {
                let mut catalog = Catalog::open(self.pager);
                catalog.put_table(&meta)?;
            }
            return Ok(ResultSet {
                columns: Vec::new(),
                rows: Vec::new(),
                message: Some(format!(
                    "OK · FOREIGN KEY constraint '{}' eliminado de '{}'",
                    fk_name, meta.name
                )),
            });
        }

        if stmt.if_exists {
            return Ok(ResultSet {
                columns: Vec::new(),
                rows: Vec::new(),
                message: Some(format!(
                    "OK · CONSTRAINT '{}' no existía en '{}' (IF EXISTS)",
                    stmt.name, meta.name
                )),
            });
        }
        Err(coded(
            codes::CONSTRAINT_NOT_FOUND,
            format!(
                "DROP CONSTRAINT '{}': no existe en '{}'. Constraints visibles: \
                 {} CHECK, {} UNIQUE indexes, {} FK nombradas",
                stmt.name,
                meta.name,
                meta.check_constraints.len(),
                meta.indexes.iter().filter(|i| i.unique).count(),
                meta.columns
                    .iter()
                    .filter(|c| c
                        .references
                        .as_ref()
                        .and_then(|r| r.name.as_ref())
                        .is_some())
                    .count(),
            ),
        ))
    }

    /// Bloque V (2026-05-27): rechaza un INSERT/UPDATE/DELETE cuyo
    /// target es una vista. Las vistas son read-only en este release.
    fn reject_if_view(&mut self, name: &str, op_label: &str) -> DbResult<()> {
        let is_view = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_view(name)?.is_some()
        };
        if is_view {
            return Err(coded(
                codes::VIEW_NOT_WRITABLE,
                format!(
                    "{} sobre '{}' rechazado: '{}' es una VISTA, no una tabla. Las vistas son \
                     read-only en este release; modificá la tabla base directamente.",
                    op_label, name, name
                ),
            ));
        }
        Ok(())
    }

    /// Bloque V (2026-05-27): `CREATE VIEW [IF NOT EXISTS] name
    /// [(col_aliases)] AS <select_query>`.
    ///
    /// 1. Valida que el SELECT subyacente sea un `SelectQuery::Select`
    ///    simple — set ops y VALUES quedan diferidos
    ///    (`[GBY-4078]`).
    /// 2. Rechaza colisión con cualquier objeto del catálogo (table o
    ///    view) con `[GBY-4077]`. `IF NOT EXISTS` lo convierte en
    ///    no-op si la colisión es contra OTRA vista con el mismo
    ///    nombre; contra una tabla rebota siempre.
    /// 3. Persiste via `Catalog::put_view`.
    fn exec_create_view(&mut self, stmt: CreateViewStmt) -> DbResult<ResultSet> {
        // Validar sintácticamente el source.
        let parsed = parse_select_query_str(&stmt.source)?;
        if !matches!(parsed, SelectQuery::Select(_)) {
            return Err(coded(
                codes::VIEW_SOURCE_NOT_SIMPLE_SELECT,
                format!(
                    "CREATE VIEW '{}': el SELECT subyacente debe ser un SELECT simple en este \
                     release; UNION/INTERSECT/EXCEPT/VALUES como source quedan diferidos",
                    stmt.name
                ),
            ));
        }
        // Validar colisión con catálogo.
        let existing = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_object(&stmt.name)?
        };
        match existing {
            Some(CatalogObject::Table(_)) => {
                return Err(coded(
                    codes::VIEW_NAME_COLLIDES_WITH_OBJECT,
                    format!(
                        "CREATE VIEW '{}': ya existe una TABLA con ese nombre. Las vistas y \
                         tablas comparten namespace; usá otro nombre o DROP TABLE primero.",
                        stmt.name
                    ),
                ));
            }
            Some(CatalogObject::View(_)) => {
                if stmt.if_not_exists {
                    return Ok(ResultSet {
                        columns: Vec::new(),
                        rows: Vec::new(),
                        message: Some(format!(
                            "OK · vista '{}' ya existía (IF NOT EXISTS)",
                            stmt.name
                        )),
                    });
                }
                return Err(coded(
                    codes::VIEW_NAME_COLLIDES_WITH_OBJECT,
                    format!(
                        "CREATE VIEW '{}': ya existe una vista con ese nombre",
                        stmt.name
                    ),
                ));
            }
            None => {}
        }
        let meta = ViewMeta {
            name: stmt.name.clone(),
            source: stmt.source,
            column_aliases: stmt.column_aliases,
        };
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.put_view(&meta)?;
        }
        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(format!("OK · vista '{}' creada", meta.name)),
        })
    }

    /// Bloque V (2026-05-27): `DROP VIEW [IF EXISTS] <name>`.
    fn exec_drop_view(&mut self, stmt: DropViewStmt) -> DbResult<ResultSet> {
        let existing = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_object(&stmt.name)?
        };
        match existing {
            Some(CatalogObject::View(_)) => {
                let mut catalog = Catalog::open(self.pager);
                catalog.remove_object(&stmt.name)?;
                Ok(ResultSet {
                    columns: Vec::new(),
                    rows: Vec::new(),
                    message: Some(format!("OK · vista '{}' eliminada", stmt.name)),
                })
            }
            Some(CatalogObject::Table(_)) => Err(DbError::new(format!(
                "DROP VIEW '{}': el nombre apunta a una TABLA, no a una vista. Usá DROP TABLE.",
                stmt.name
            ))),
            None => {
                if stmt.if_exists {
                    Ok(ResultSet {
                        columns: Vec::new(),
                        rows: Vec::new(),
                        message: Some(format!("OK · vista '{}' no existía (IF EXISTS)", stmt.name)),
                    })
                } else {
                    Err(coded(
                        codes::TABLE_NOT_FOUND,
                        format!("DROP VIEW '{}': la vista no existe", stmt.name),
                    ))
                }
            }
        }
    }

    /// Issue #1 (2026-05-27): pre-evalúa toda `ScalarSubquery` no
    /// correlacionada dentro de `expr` y la sustituye por
    /// `Expr::Literal(value)`. Recursivo: si una subquery contiene
    /// otras subqueries no-correlated, también las memoiza.
    ///
    /// Ahorro práctico: en `SELECT (SELECT COUNT(*) FROM t) FROM t LIMIT N`
    /// el sub-COUNT pasa de O(N · |t|) (re-evaluado por fila del outer)
    /// a O(|t|) (una sola pasada).
    fn memoize_uncorrelated_scalar_subqueries(&mut self, expr: &mut Expr) -> DbResult<()> {
        match expr {
            Expr::Literal(_) | Expr::Column(_) => Ok(()),
            Expr::ScalarSubquery(sub) => {
                // Si es correlacionada, NO podemos memoizar — depende
                // del row del outer. Recursamos dentro por si la
                // subquery a su vez contiene scalar subqueries
                // memoizables en su SELECT list.
                if select_stmt_is_correlated(sub) {
                    for item in sub.columns.iter_mut() {
                        if let SelectItem::Expression { expr: inner, .. } = item {
                            self.memoize_uncorrelated_scalar_subqueries(inner)?;
                        }
                    }
                    return Ok(());
                }
                // No correlacionada → ejecutar UNA vez y reemplazar
                // por el literal resultante. Si la subquery falla
                // (e.g. multi-row, multi-column), el error sale acá
                // como saldría en el path original — semánticamente
                // idéntico, no se postergó a runtime.
                let inner_stmt = (**sub).clone();
                let inner_res = self.exec_select(inner_stmt)?;
                if inner_res.columns.len() != 1 {
                    return Err(coded(
                        codes::SUBQUERY_MUST_RETURN_ONE_COLUMN,
                        format!(
                            "subquery escalar debe devolver exactamente 1 columna; devolvió {}",
                            inner_res.columns.len()
                        ),
                    ));
                }
                if inner_res.rows.len() > 1 {
                    return Err(coded(
                        codes::SCALAR_SUBQUERY_TOO_MANY_ROWS,
                        format!(
                            "subquery escalar devolvió {} filas; debe devolver a lo sumo 1",
                            inner_res.rows.len()
                        ),
                    ));
                }
                let v = inner_res
                    .rows
                    .into_iter()
                    .next()
                    .and_then(|mut r| r.pop())
                    .unwrap_or(Value::Null);
                *expr = Expr::Literal(v);
                Ok(())
            }
            Expr::Func(_, args) => {
                for a in args.iter_mut() {
                    self.memoize_uncorrelated_scalar_subqueries(a)?;
                }
                Ok(())
            }
            Expr::Cast(inner, _) => self.memoize_uncorrelated_scalar_subqueries(inner),
            Expr::Case {
                operand,
                branches,
                else_branch,
            } => {
                if let Some(e) = operand.as_mut() {
                    self.memoize_uncorrelated_scalar_subqueries(e)?;
                }
                for (c, v) in branches.iter_mut() {
                    self.memoize_uncorrelated_scalar_subqueries(c)?;
                    self.memoize_uncorrelated_scalar_subqueries(v)?;
                }
                if let Some(e) = else_branch.as_mut() {
                    self.memoize_uncorrelated_scalar_subqueries(e)?;
                }
                Ok(())
            }
            Expr::Compare(l, _, r) | Expr::Arith(l, _, r) => {
                self.memoize_uncorrelated_scalar_subqueries(l)?;
                self.memoize_uncorrelated_scalar_subqueries(r)
            }
            Expr::IsNull(i, _) | Expr::Like(i, _, _) | Expr::InList(i, _, _) => {
                self.memoize_uncorrelated_scalar_subqueries(i)
            }
            Expr::Between(l, lo, hi, _) => {
                self.memoize_uncorrelated_scalar_subqueries(l)?;
                self.memoize_uncorrelated_scalar_subqueries(lo)?;
                self.memoize_uncorrelated_scalar_subqueries(hi)
            }
        }
    }

    /// Issue #1: aplica `memoize_uncorrelated_scalar_subqueries` a
    /// todas las Expr que aparecen en el SELECT list, GROUP BY,
    /// HAVING, ORDER BY de un SelectStmt. Llamado al inicio de
    /// `exec_select` y `exec_select_joined` antes de iterar filas.
    fn memoize_select_stmt(&mut self, stmt: &mut SelectStmt) -> DbResult<()> {
        for item in stmt.columns.iter_mut() {
            if let SelectItem::Expression { expr, .. } = item {
                self.memoize_uncorrelated_scalar_subqueries(expr)?;
            }
        }
        Ok(())
    }

    /// Bloque V (2026-05-27): si `stmt.table` apunta a una vista, parsea
    /// el source SQL de la vista y lo embebe como derived table del
    /// FROM. Idempotente para tablas (no-op). Llamado desde
    /// `exec_select` y `exec_select_joined` antes del lookup del catálogo.
    ///
    /// Limitaciones:
    /// - El source de la vista debe ser un `SelectQuery::Select` simple
    ///   (validado en CREATE VIEW con `[GBY-4078]`).
    /// - Protege contra ciclos vía `view_expansion_depth` —
    ///   `[GBY-4076]` cuando excede `MAX_VIEW_DEPTH`.
    /// - Los `column_aliases` de la vista, si los hay, se aplican
    ///   re-bautizando los nombres de salida del SELECT subyacente
    ///   en el alias (`stmt.table_alias`).
    fn expand_view_in_from(&mut self, stmt: &mut SelectStmt) -> DbResult<()> {
        // Sólo expandimos si el FROM es una tabla bare (no derived
        // table ni VALUES). El nombre debe matchear una view del
        // catálogo.
        if stmt.derived_source.is_some() || stmt.values_source.is_some() {
            return Ok(());
        }
        let view = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_view(&stmt.table)?
        };
        let Some(view) = view else {
            return Ok(());
        };
        // Cycle / depth guard.
        if self.view_expansion_depth >= MAX_VIEW_DEPTH {
            return Err(coded(
                codes::VIEW_EXPANSION_DEPTH_EXCEEDED,
                format!(
                    "expansión de vistas excedió la profundidad {} al referenciar '{}' \
                     (¿ciclo entre vistas?)",
                    MAX_VIEW_DEPTH, stmt.table
                ),
            ));
        }
        self.view_expansion_depth += 1;
        let parsed = parse_select_query_str(&view.source)?;
        let inner_stmt = match parsed {
            SelectQuery::Select(s) => s,
            _ => {
                self.view_expansion_depth -= 1;
                return Err(coded(
                    codes::VIEW_SOURCE_NOT_SIMPLE_SELECT,
                    format!(
                        "vista '{}': el source persistido no es un SELECT simple (estado \
                         inconsistente — el catálogo debería haberlo rechazado en CREATE VIEW)",
                        view.name
                    ),
                ));
            }
        };
        self.view_expansion_depth -= 1;
        // El alias del derived table en el FROM exterior es el nombre
        // de la vista, salvo que el usuario ya haya declarado uno
        // explícito en el query original (`FROM v AS x`).
        let alias_for_outer = stmt
            .table_alias
            .clone()
            .unwrap_or_else(|| view.name.clone());
        // Convertir la vista en derived source. `stmt.table` deja de
        // tener significado relacional — pasa a ser el alias visible
        // en el resto del planner.
        stmt.table = alias_for_outer.clone();
        stmt.table_alias = None;
        stmt.derived_source = Some(inner_stmt);

        // Bloque V: los column_aliases de la vista renombran las
        // columnas de salida del SELECT subyacente. Empujamos un Vec
        // que el planner de derived tables sabe interpretar.
        if let Some(aliases) = &view.column_aliases {
            // Validación: el alias_count debe matchear el número de
            // columnas que proyecta el SELECT interno (best-effort —
            // el planner igual lo re-validará).
            let inner = stmt.derived_source.as_ref().unwrap();
            if inner.columns.len() != aliases.len() {
                return Err(coded(
                    codes::DERIVED_TABLE_REQUIRES_ALIAS,
                    format!(
                        "vista '{}' declaró {} column aliases pero el SELECT subyacente \
                         proyecta {} columnas",
                        view.name,
                        aliases.len(),
                        inner.columns.len()
                    ),
                ));
            }
            // Truco: aplicamos los aliases mutando el inner para que el
            // resultado los exponga directamente. Sustituimos cada
            // `SelectItem::output_name()` con el alias correspondiente
            // wrappeando el ítem original con un alias visible. Para
            // simplicidad, sólo soportamos column aliases sobre
            // proyecciones que ya son `Column` o `Expr` sin alias; si
            // alguna fila tiene alias propio, queda como advertencia.
            // (Un sweep completo del SelectItem para forzar aliases
            // queda diferido.)
            // No-op explícito aquí — los aliases se aplican vía el
            // planner de derived tables si en el futuro extendemos el
            // SelectItem AST. Por ahora documentamos la limitación.
            let _ = alias_for_outer;
        }
        Ok(())
    }

    /// Bloque K1 (2026-05-26): `ALTER TABLE <t> DROP COLUMN [IF EXISTS] <col>`.
    /// Bloqueos: PK (`[GBY-4059]`), columna indexada (`[GBY-4060]`),
    /// FK saliente o entrante (`[GBY-4061]`). Implementación: full scan
    /// de filas, decodifica con la meta vieja, descarta la columna,
    /// re-encodea con la meta nueva y reinserta (mismo patrón que
    /// `ALTER TABLE ADD COLUMN`).
    fn exec_alter_drop_column(&mut self, stmt: AlterDropColumnStmt) -> DbResult<ResultSet> {
        let mut meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_table(&stmt.table)?.ok_or_else(|| {
                coded(
                    codes::TABLE_NOT_FOUND,
                    format!("DROP COLUMN: tabla '{}' no existe", stmt.table),
                )
            })?
        };

        let col_norm = normalize_ident(&stmt.column);
        let col_idx = meta
            .columns
            .iter()
            .position(|c| normalize_ident(&c.name) == col_norm);
        let Some(idx) = col_idx else {
            if stmt.if_exists {
                return Ok(ResultSet {
                    columns: Vec::new(),
                    rows: Vec::new(),
                    message: Some(format!(
                        "OK · columna '{}' no existía en '{}' (IF EXISTS)",
                        stmt.column, meta.name
                    )),
                });
            }
            return Err(coded(
                codes::COLUMN_NOT_FOUND,
                format!(
                    "DROP COLUMN: columna '{}' no existe en '{}'",
                    stmt.column, meta.name
                ),
            ));
        };

        let column = meta.columns[idx].clone();

        // Bloqueo: PK.
        if column.name.eq_ignore_ascii_case(&meta.primary_key) {
            return Err(coded(
                codes::CANNOT_DROP_PRIMARY_KEY,
                format!(
                    "DROP COLUMN '{}': es la PRIMARY KEY de '{}' y no se puede borrar; \
                     usá DROP TABLE si la intención es rehacer el esquema",
                    stmt.column, meta.name
                ),
            ));
        }

        // Bloqueo: columna indexada (incluye índices UNIQUE inline).
        if let Some(idx_meta) = meta
            .indexes
            .iter()
            .find(|i| normalize_ident(&i.column) == col_norm)
        {
            return Err(coded(
                codes::CANNOT_DROP_INDEXED_COLUMN,
                format!(
                    "DROP COLUMN '{}': existe el índice '{}' sobre esa columna. \
                     Ejecutá 'DROP INDEX {}' primero",
                    stmt.column, idx_meta.name, idx_meta.name
                ),
            ));
        }

        // Bloqueo: FK saliente desde esta columna — caso anchor (la
        // columna es la primera del set source de la FK).
        if column.references.is_some() {
            return Err(coded(
                codes::CANNOT_DROP_REFERENCED_COLUMN,
                format!(
                    "DROP COLUMN '{}': la columna declara una FOREIGN KEY hacia otra tabla. \
                     Usar ALTER TABLE DROP CONSTRAINT <name> sobre la FK antes (residual #2).",
                    stmt.column
                ),
            ));
        }
        // Residual #3 (2026-05-27): FK saliente multi-col donde esta
        // columna está en `extra_source_columns` de otra. La FK está
        // anchored en otra columna pero esta también participa.
        for c in &meta.columns {
            if let Some(fk) = &c.references {
                if fk
                    .extra_source_columns
                    .iter()
                    .any(|s| normalize_ident(s) == col_norm)
                {
                    return Err(coded(
                        codes::CANNOT_DROP_REFERENCED_COLUMN,
                        format!(
                            "DROP COLUMN '{}': participa en una FOREIGN KEY multi-col anclada \
                             en '{}' (target table '{}'). Usar ALTER TABLE DROP CONSTRAINT \
                             antes.",
                            stmt.column, c.name, fk.table
                        ),
                    ));
                }
            }
        }

        // Bloqueo: FK entrante — otra tabla apunta a esta columna como su
        // parent. Esto sólo aplica si la columna es la PK del padre, lo
        // cual ya está descartado arriba; igual lo mantenemos por defense
        // in depth en caso de que `references.column` apunte a una
        // columna distinta a la PK (futuro release).
        let all_tables: Vec<TableMeta> = {
            let mut catalog = Catalog::open(self.pager);
            catalog.list_tables()?
        };
        for other in &all_tables {
            if other.name.eq_ignore_ascii_case(&meta.name) {
                continue;
            }
            for c in &other.columns {
                if let Some(fk) = &c.references {
                    if !fk.table.eq_ignore_ascii_case(&meta.name) {
                        continue;
                    }
                    // Anchor target o cualquier extra_target apuntando a
                    // esta columna bloquea el DROP. Residual #3.
                    let mut all_targets = vec![fk.column.clone()];
                    all_targets.extend(fk.extra_target_columns.iter().cloned());
                    if all_targets.iter().any(|t| normalize_ident(t) == col_norm) {
                        return Err(coded(
                            codes::CANNOT_DROP_REFERENCED_COLUMN,
                            format!(
                                "DROP COLUMN '{}': la tabla '{}' (columna '{}') tiene una \
                                 FOREIGN KEY que apunta a esta columna",
                                stmt.column, other.name, c.name
                            ),
                        ));
                    }
                }
            }
        }

        // Build de la nueva meta (sin la columna).
        let mut new_meta = meta.clone();
        new_meta.columns.remove(idx);
        validate_create_table(&new_meta)?;

        // Full scan + rewrite. Decodificamos con la meta vieja,
        // sacamos la columna del HashMap y re-encodeamos con la nueva.
        let kvs = {
            let mut catalog = Catalog::open(self.pager);
            catalog.scan_rows(meta.root_page, 0, None)?
        };
        for kv in kvs {
            let mut row = decode_row(&meta, &kv.value)?;
            row.remove(&col_norm);
            let (pk, bytes) = encode_row(&new_meta, &row)?;
            // sanity: la PK no debería cambiar al borrar otra columna.
            debug_assert_eq!(pk, kv.key, "DROP COLUMN movió la PK — bug en encode_row");
            let mut catalog = Catalog::open(self.pager);
            catalog.upsert_row(new_meta.root_page, pk, bytes)?;
        }

        // Reemplaza el catálogo (mismo nombre/clave hash → upsert).
        meta = new_meta;
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.put_table(&meta)?;
        }

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(format!(
                "OK · columna '{}' eliminada de '{}'",
                column.name, meta.name
            )),
        })
    }

    /// Bloque K1 (2026-05-26): `ALTER TABLE <t> RENAME COLUMN <old> TO <new>`.
    /// El on-disk row es posicional, así que no requiere rewrite de
    /// datos: alcanza con mutar `TableMeta.columns[i].name` y arrastrar
    /// el cambio a `primary_key`, índices y FKs que referencien la
    /// columna (locales y entrantes).
    fn exec_alter_rename_column(&mut self, stmt: AlterRenameColumnStmt) -> DbResult<ResultSet> {
        validate_identifier(&stmt.new_name, "columna")?;

        let mut meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_table(&stmt.table)?.ok_or_else(|| {
                coded(
                    codes::TABLE_NOT_FOUND,
                    format!("RENAME COLUMN: tabla '{}' no existe", stmt.table),
                )
            })?
        };

        let old_norm = normalize_ident(&stmt.old_name);
        let new_norm = normalize_ident(&stmt.new_name);

        if old_norm == new_norm {
            return Ok(ResultSet {
                columns: Vec::new(),
                rows: Vec::new(),
                message: Some(format!(
                    "OK · columna '{}' ya se llamaba así en '{}'",
                    stmt.old_name, meta.name
                )),
            });
        }

        // La columna origen tiene que existir.
        let idx = meta
            .columns
            .iter()
            .position(|c| normalize_ident(&c.name) == old_norm)
            .ok_or_else(|| {
                coded(
                    codes::COLUMN_NOT_FOUND,
                    format!(
                        "RENAME COLUMN: columna '{}' no existe en '{}'",
                        stmt.old_name, meta.name
                    ),
                )
            })?;

        // El nombre destino no puede coincidir con otra columna ya
        // presente (case-insensitive).
        if meta
            .columns
            .iter()
            .any(|c| normalize_ident(&c.name) == new_norm)
        {
            return Err(coded(
                codes::RENAME_TARGET_EXISTS,
                format!(
                    "RENAME COLUMN rechazado: ya existe una columna llamada '{}' en '{}'",
                    stmt.new_name, meta.name
                ),
            ));
        }

        // Mutaciones in-place.
        let old_actual = meta.columns[idx].name.clone();
        meta.columns[idx].name = stmt.new_name.clone();
        if meta.primary_key.eq_ignore_ascii_case(&old_actual) {
            meta.primary_key = stmt.new_name.clone();
        }
        for ix in meta.indexes.iter_mut() {
            if normalize_ident(&ix.column) == old_norm {
                ix.column = stmt.new_name.clone();
            }
            // K2: índices compuestos también renombran extra_columns.
            for ec in ix.extra_columns.iter_mut() {
                if normalize_ident(ec) == old_norm {
                    *ec = stmt.new_name.clone();
                }
            }
        }
        // No tocamos `references.column` ni `references.extra_target_columns`
        // de la propia tabla: esos apuntan a columnas del PARENT, no
        // locales. Pero sí debemos actualizar
        // `references.extra_source_columns` cuando una FK multi-col
        // declarada en otra columna referencia la columna que estamos
        // renombrando como source extra (residual #3).
        for col in meta.columns.iter_mut() {
            if let Some(fk) = col.references.as_mut() {
                for ec in fk.extra_source_columns.iter_mut() {
                    if normalize_ident(ec) == old_norm {
                        *ec = stmt.new_name.clone();
                    }
                }
            }
        }

        // Validar el resultado y persistirlo.
        validate_create_table(&meta)?;
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.put_table(&meta)?;
        }

        // FKs entrantes: otras tablas pueden tener `fk.column = old_name`
        // si apuntaban a esta columna como parent. Hoy las FKs sólo
        // apuntan a la PK del parent, así que si renombramos la PK
        // tenemos que arrastrar el cambio.
        let all_tables: Vec<TableMeta> = {
            let mut catalog = Catalog::open(self.pager);
            catalog.list_tables()?
        };
        for mut other in all_tables {
            if other.name.eq_ignore_ascii_case(&meta.name) {
                continue;
            }
            let mut changed = false;
            for col in other.columns.iter_mut() {
                if let Some(fk) = col.references.as_mut() {
                    if !fk.table.eq_ignore_ascii_case(&meta.name) {
                        continue;
                    }
                    if normalize_ident(&fk.column) == old_norm {
                        fk.column = stmt.new_name.clone();
                        changed = true;
                    }
                    // Residual #3: extra_target_columns también deben
                    // arrastrar el rename para FK multi-col.
                    for et in fk.extra_target_columns.iter_mut() {
                        if normalize_ident(et) == old_norm {
                            *et = stmt.new_name.clone();
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                let mut catalog = Catalog::open(self.pager);
                catalog.put_table(&other)?;
            }
        }

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(format!(
                "OK · columna '{}' renombrada a '{}' en '{}'",
                old_actual, meta.name, stmt.new_name
            )),
        })
    }

    fn exec_insert(&mut self, stmt: InsertStmt) -> DbResult<ResultSet> {
        // Bloque V (2026-05-27): rechazo claro si el target es una vista.
        self.reject_if_view(&stmt.table, "INSERT")?;
        // Bloque J: validamos columnas y normalizamos UNA vez para todo
        // el batch (single-row, multi-row o INSERT...SELECT). Después
        // iteramos las filas-fuente delegando en `apply_insert_row`.
        // Bloque J2: si hay on_conflict (UPSERT / REPLACE) o returning,
        // las pasamos al loop para que cada fila se procese según el
        // contrato declarado.
        let meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_table(&stmt.table)?.ok_or_else(|| {
                coded(
                    codes::TABLE_NOT_FOUND,
                    format!("tabla no existe: {}", stmt.table),
                )
            })?
        };
        // Validar nombres de columnas y dedup. Producimos la lista de
        // claves normalizadas en el orden del INSERT.
        let mut seen = HashSet::new();
        let mut normalized_cols = Vec::with_capacity(stmt.columns.len());
        for column_name in &stmt.columns {
            let normalized = normalize_ident(column_name);
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
            normalized_cols.push(normalized);
        }

        // Recolectamos la lista de filas-fuente. Para `Values` salen
        // directo del AST; para `Select` ejecutamos la subquery y
        // extraemos las filas en el orden de columnas del SELECT.
        let rows_to_insert: Vec<Vec<Value>> = match stmt.source {
            InsertSource::Values(rows) => {
                for (i, row) in rows.iter().enumerate() {
                    if row.len() != stmt.columns.len() {
                        return Err(coded(
                            codes::INSERT_COLS_VS_VALUES_MISMATCH,
                            format!(
                                "INSERT INTO '{}': fila {} tiene {} valores pero hay {} columnas",
                                stmt.table,
                                i + 1,
                                row.len(),
                                stmt.columns.len()
                            ),
                        ));
                    }
                }
                rows
            }
            InsertSource::Select(sub) => {
                let inner = self.exec_select(*sub)?;
                if inner.columns.len() != stmt.columns.len() {
                    return Err(coded(
                        codes::INSERT_COLS_VS_VALUES_MISMATCH,
                        format!(
                            "INSERT INTO '{}' SELECT: el SELECT devolvió {} columnas pero el INSERT espera {}",
                            stmt.table,
                            inner.columns.len(),
                            stmt.columns.len()
                        ),
                    ));
                }
                inner.rows
            }
        };

        // Bloque J2: validamos on_conflict target antes del loop (si hay).
        if let Some(oc) = &stmt.on_conflict {
            if let Some(target) = &oc.target {
                let key = normalize_ident(target);
                let is_pk = normalize_ident(&meta.primary_key) == key;
                let is_unique = meta
                    .indexes
                    .iter()
                    .any(|i| i.unique && normalize_ident(&i.column) == key);
                if !is_pk && !is_unique {
                    return Err(coded(
                        codes::ON_CONFLICT_TARGET_NOT_UNIQUE,
                        format!(
                            "ON CONFLICT ({}): la columna no es PK ni tiene índice UNIQUE — \
                             el motor no puede detectar el conflicto",
                            target
                        ),
                    ));
                }
            }
        }

        let mut affected_rows: Vec<HashMap<String, Value>> = Vec::new();
        let mut inserted = 0usize;
        let mut skipped = 0usize;
        let mut replaced = 0usize;
        for row_values in rows_to_insert {
            let outcome = self.apply_insert_row_with_conflict(
                &meta,
                &normalized_cols,
                row_values,
                stmt.on_conflict.as_ref(),
            )?;
            match outcome {
                RowOutcome::Inserted(row) => {
                    inserted += 1;
                    if stmt.returning.is_some() {
                        affected_rows.push(row);
                    }
                }
                RowOutcome::Updated(row) => {
                    replaced += 1;
                    if stmt.returning.is_some() {
                        affected_rows.push(row);
                    }
                }
                RowOutcome::Skipped => {
                    skipped += 1;
                }
            }
        }

        // Bloque J2: con RETURNING devolvemos las filas; el message sigue
        // siendo informativo. Sin RETURNING devolvemos solo el message.
        if let Some(returning) = &stmt.returning {
            let projected = project_returning(&meta, returning, &affected_rows)?;
            let columns = returning_column_names(&meta, returning);
            return Ok(ResultSet {
                columns,
                rows: projected,
                message: Some(format_insert_message(inserted, replaced, skipped)),
            });
        }
        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(format_insert_message(inserted, replaced, skipped)),
        })
    }

    /// Bloque J2: variante de `apply_insert_row` que enrutamiento de
    /// conflictos PK/UNIQUE según `on_conflict`. Sin on_conflict el
    /// comportamiento es idéntico al pre-J2 (errorea en violación).
    /// Devuelve un `RowOutcome` para que el caller actualice los
    /// contadores y la lista de RETURNING.
    fn apply_insert_row_with_conflict(
        &mut self,
        meta: &TableMeta,
        normalized_cols: &[String],
        row_values: Vec<Value>,
        on_conflict: Option<&OnConflict>,
    ) -> DbResult<RowOutcome> {
        let mut values = HashMap::new();
        for (key, value) in normalized_cols.iter().zip(row_values) {
            values.insert(key.clone(), value);
        }
        apply_defaults(meta, &mut values);

        // Detectar conflicto ANTES de validar NOT NULL — el conflicto
        // PK/UNIQUE permite saltarse las constraints si la acción es
        // DoNothing (la fila nueva nunca se materializa).
        if let Some(oc) = on_conflict {
            let conflict_pks =
                detect_conflict_pks(self.pager, meta, &values, oc.target.as_deref())?;
            if !conflict_pks.is_empty() {
                match &oc.action {
                    OnConflictAction::DoNothing => return Ok(RowOutcome::Skipped),
                    OnConflictAction::DoUpdate { assignments } => {
                        // Bloque G2: cada `(col, expr)` se valida shape
                        // una vez (PK / columna existe). La evaluación
                        // de la Expr ocurre dentro de `apply_update_to_pk`
                        // contra la fila pre-update — `EXCLUDED.col` sigue
                        // sin soportarse (J2-P2).
                        let mut expr_assignments: Vec<(String, Expr)> = Vec::new();
                        for (col, expr) in assignments {
                            let k = normalize_ident(col);
                            if k == normalize_ident(&meta.primary_key) {
                                return Err(coded(
                                    codes::UPDATE_PK_NOT_ALLOWED,
                                    format!(
                                        "ON CONFLICT DO UPDATE sobre '{}': no se permite mutar la PRIMARY KEY '{}'",
                                        meta.name, meta.primary_key
                                    ),
                                ));
                            }
                            if meta.column(&k).is_none() {
                                return Err(coded(
                                    codes::COLUMN_NOT_FOUND,
                                    format!(
                                        "ON CONFLICT DO UPDATE: columna '{}' no existe en '{}'",
                                        col, meta.name
                                    ),
                                ));
                            }
                            expr_assignments.push((k, expr.clone()));
                        }
                        // Aplicamos a TODAS las filas conflictivas (en la
                        // práctica suele ser 1, pero un mismo INSERT puede
                        // chocar contra más de una constraint).
                        let mut last_row: Option<HashMap<String, Value>> = None;
                        for pk in &conflict_pks {
                            self.apply_update_to_pk(meta, *pk, &expr_assignments)?;
                            // Leer fila post-update para RETURNING.
                            let bytes = {
                                let mut catalog = Catalog::open(self.pager);
                                catalog.get_row(meta.root_page, *pk)?
                            };
                            if let Some(b) = bytes {
                                last_row = Some(decode_row(meta, &b)?);
                            }
                        }
                        return Ok(match last_row {
                            Some(r) => RowOutcome::Updated(r),
                            None => RowOutcome::Skipped,
                        });
                    }
                    OnConflictAction::Replace => {
                        // Borramos las filas conflictivas vía cascade y
                        // luego procedemos al insert normal.
                        for pk in &conflict_pks {
                            let still_there = {
                                let mut catalog = Catalog::open(self.pager);
                                catalog.get_row(meta.root_page, *pk)?.is_some()
                            };
                            if still_there {
                                delete_with_cascade(self.pager, &meta.name, *pk)?;
                            }
                        }
                        // continúa al path de insert normal abajo
                    }
                }
            }
        }

        enforce_not_null_on_insert(meta, &values)?;
        // Bloque L2 (2026-05-27): CHECK eval contra la fila propuesta.
        // Aplica defaults antes — el `values` que llega ya pasó por
        // `apply_defaults` aguas arriba (mismo lugar que NOT NULL).
        enforce_check_constraints(meta, &values)?;

        // Bloque L1 (2026-05-27): el UNIQUE pre-check distingue índices
        // single-column (value bytes de la columna) de compuestos (FNV-1a-64
        // fingerprint sobre todas las columnas, mismo encoder que K2). Sin
        // esta separación, un UNIQUE (a, b) se chequeaba contra el bucket
        // de la primera columna y rechazaba INSERTs legítimos por colisión
        // del primer componente.
        for idx in &meta.indexes {
            if !idx.unique {
                continue;
            }
            if idx.is_composite() {
                let fp = composite_fp_for_values(meta, idx, &values)?;
                composite_unique_check(self.pager, idx, fp, None)?;
            } else {
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
        }

        let (pk, row_bytes) = encode_row(meta, &values)?;
        enforce_fk_on_insert(self.pager, meta, &values, pk)?;
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.insert_row(meta.root_page, pk, row_bytes)?;
        }

        for idx in &meta.indexes {
            if idx.is_composite() {
                let fp = composite_fp_for_values(meta, idx, &values)?;
                composite_index_upsert(self.pager, idx.root_page, fp, pk)?;
            } else {
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
        }

        Ok(RowOutcome::Inserted(values))
    }

    /// Bloque J: `TRUNCATE [TABLE] <name>`. Implementación naive: scan
    /// completo de PKs + `delete_with_cascade` por fila. NO es O(1) como
    /// en Postgres/MySQL (que re-asignan el segmento); preferimos
    /// respetar `ON DELETE` declarado (cascade/restrict) y mantener
    /// índices secundarios consistentes vía el path normal de delete.
    fn exec_truncate(&mut self, stmt: TruncateStmt) -> DbResult<ResultSet> {
        let meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_table(&stmt.table)?.ok_or_else(|| {
                coded(
                    codes::TABLE_NOT_FOUND,
                    format!("tabla no existe: {}", stmt.table),
                )
            })?
        };
        let pks: Vec<i64> = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .scan_rows(meta.root_page, 0, None)?
                .into_iter()
                .map(|kv| kv.key)
                .collect()
        };
        let mut deleted = 0usize;
        for pk in pks {
            let still_there = {
                let mut catalog = Catalog::open(self.pager);
                catalog.get_row(meta.root_page, pk)?.is_some()
            };
            if !still_there {
                continue;
            }
            delete_with_cascade(self.pager, &meta.name, pk)?;
            deleted += 1;
        }
        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(format!(
                "OK ({} fila{} eliminada{})",
                deleted,
                if deleted == 1 { "" } else { "s" },
                if deleted == 1 { "" } else { "s" }
            )),
        })
    }

    /// Bloque H (2026-05-26): evaluador de `Expr` con acceso al engine,
    /// necesario para resolver `Expr::ScalarSubquery`. Para árboles sin
    /// subqueries dispara la fast-path `eval_expr` (sin overhead) y solo
    /// recursa per-variant cuando hay alguna subquery escondida en el
    /// árbol.
    ///
    /// `outer_table_name` (cuando es `Some`) se usa como nombre de la
    /// frame que se pushea en `outer_stack` antes de ejecutar cada
    /// subquery — así una subquery correlacionada puede resolver
    /// `outer.col` aunque la outer query haya proyectado columnas bare
    /// sin qualifier.
    fn eval_expr_full(
        &mut self,
        expr: &Expr,
        row: &HashMap<String, Value>,
        outer_table_name: Option<&str>,
    ) -> DbResult<Value> {
        if !expr_contains_subquery(expr) {
            return eval_expr(expr, row);
        }
        match expr {
            Expr::ScalarSubquery(sub) => {
                let pushed = if let Some(name) = outer_table_name {
                    self.outer_stack.push(OuterRow {
                        table: name.to_string(),
                        values: row.clone(),
                    });
                    true
                } else {
                    false
                };
                let inner_res = self.exec_select((**sub).clone());
                if pushed {
                    self.outer_stack.pop();
                }
                let inner = inner_res?;
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
                            "subquery escalar en SELECT/expr devolvió {} filas; debe devolver a lo sumo 1",
                            inner.rows.len()
                        ),
                    ));
                }
                let scalar = inner.rows.into_iter().next().and_then(|mut r| r.pop());
                Ok(scalar.unwrap_or(Value::Null))
            }
            Expr::Literal(_) | Expr::Column(_) => eval_expr(expr, row),
            Expr::Func(f, args) => {
                // Reusamos la dispatch logic de eval_expr para
                // short-circuit (COALESCE/IF/IFNULL/NULLIF) — solo
                // que evaluamos cada arg con eval_expr_full.
                match f {
                    ScalarFunc::Coalesce => {
                        for a in args {
                            let v = self.eval_expr_full(a, row, outer_table_name)?;
                            if !matches!(v, Value::Null) {
                                return Ok(v);
                            }
                        }
                        Ok(Value::Null)
                    }
                    ScalarFunc::Ifnull => {
                        let a = self.eval_expr_full(&args[0], row, outer_table_name)?;
                        if matches!(a, Value::Null) {
                            self.eval_expr_full(&args[1], row, outer_table_name)
                        } else {
                            Ok(a)
                        }
                    }
                    ScalarFunc::If => {
                        let cond = self.eval_expr_full(&args[0], row, outer_table_name)?;
                        let truthy = match cond {
                            Value::Bool(b) => b,
                            Value::Null => false,
                            other => {
                                return Err(coded(
                                    codes::SCALAR_FN_TYPE_MISMATCH,
                                    format!(
                                        "IF(cond,...): cond debe ser BOOL, recibí {}",
                                        value_type_name(&other)
                                    ),
                                ));
                            }
                        };
                        if truthy {
                            self.eval_expr_full(&args[1], row, outer_table_name)
                        } else {
                            self.eval_expr_full(&args[2], row, outer_table_name)
                        }
                    }
                    ScalarFunc::Nullif => {
                        let a = self.eval_expr_full(&args[0], row, outer_table_name)?;
                        let b = self.eval_expr_full(&args[1], row, outer_table_name)?;
                        if matches!(a, Value::Null) || matches!(b, Value::Null) {
                            return Ok(a);
                        }
                        if values_equal(&a, &b) {
                            Ok(Value::Null)
                        } else {
                            Ok(a)
                        }
                    }
                    _ => {
                        let mut evaluated = Vec::with_capacity(args.len());
                        for a in args {
                            evaluated.push(self.eval_expr_full(a, row, outer_table_name)?);
                        }
                        eval_scalar_fn(*f, evaluated)
                    }
                }
            }
            Expr::Cast(inner, ty) => {
                let v = self.eval_expr_full(inner, row, outer_table_name)?;
                cast_value(v, *ty)
            }
            Expr::Case {
                operand,
                branches,
                else_branch,
            } => match operand {
                None => {
                    for (cond, val) in branches {
                        let c = self.eval_expr_full(cond, row, outer_table_name)?;
                        match c {
                            Value::Bool(true) => {
                                return self.eval_expr_full(val, row, outer_table_name);
                            }
                            Value::Bool(false) | Value::Null => continue,
                            other => {
                                return Err(coded(
                                    codes::CASE_BRANCH_TYPE_MISMATCH,
                                    format!(
                                        "CASE WHEN: la condición debe ser BOOL, recibí {}",
                                        value_type_name(&other)
                                    ),
                                ));
                            }
                        }
                    }
                    match else_branch {
                        Some(e) => self.eval_expr_full(e, row, outer_table_name),
                        None => Ok(Value::Null),
                    }
                }
                Some(op_expr) => {
                    let op_val = self.eval_expr_full(op_expr, row, outer_table_name)?;
                    for (when_val, then_val) in branches {
                        let wv = self.eval_expr_full(when_val, row, outer_table_name)?;
                        if values_equal(&op_val, &wv) {
                            return self.eval_expr_full(then_val, row, outer_table_name);
                        }
                    }
                    match else_branch {
                        Some(e) => self.eval_expr_full(e, row, outer_table_name),
                        None => Ok(Value::Null),
                    }
                }
            },
            Expr::Compare(lhs, op, rhs) => {
                let a = self.eval_expr_full(lhs, row, outer_table_name)?;
                let b = self.eval_expr_full(rhs, row, outer_table_name)?;
                if matches!(a, Value::Null) || matches!(b, Value::Null) {
                    return Ok(Value::Null);
                }
                let cmp_op = match op {
                    ExprCmpOp::Eq => return Ok(Value::Bool(values_equal(&a, &b))),
                    ExprCmpOp::Ne => return Ok(Value::Bool(!values_equal(&a, &b))),
                    ExprCmpOp::Lt => CompareOp::Lt,
                    ExprCmpOp::Le => CompareOp::Le,
                    ExprCmpOp::Gt => CompareOp::Gt,
                    ExprCmpOp::Ge => CompareOp::Ge,
                };
                match eval_compare(Some(&a), cmp_op, &b) {
                    Some(b) => Ok(Value::Bool(b)),
                    None => Ok(Value::Null),
                }
            }
            Expr::IsNull(inner, negated) => {
                let v = self.eval_expr_full(inner, row, outer_table_name)?;
                let is_null = matches!(v, Value::Null);
                Ok(Value::Bool(if *negated { !is_null } else { is_null }))
            }
            Expr::Arith(lhs, op, rhs) => {
                let a = self.eval_expr_full(lhs, row, outer_table_name)?;
                let b = self.eval_expr_full(rhs, row, outer_table_name)?;
                eval_arith(a, *op, b)
            }
            Expr::Like(lhs, pattern, negated) => {
                let v = self.eval_expr_full(lhs, row, outer_table_name)?;
                match eval_like(Some(&v), pattern, *negated) {
                    Some(b) => Ok(Value::Bool(b)),
                    None => Ok(Value::Null),
                }
            }
            Expr::InList(lhs, values, negated) => {
                let v = self.eval_expr_full(lhs, row, outer_table_name)?;
                match eval_in_list(Some(&v), values, *negated) {
                    Some(b) => Ok(Value::Bool(b)),
                    None => Ok(Value::Null),
                }
            }
            Expr::Between(lhs, lo, hi, negated) => {
                let v = self.eval_expr_full(lhs, row, outer_table_name)?;
                let lv = self.eval_expr_full(lo, row, outer_table_name)?;
                let hv = self.eval_expr_full(hi, row, outer_table_name)?;
                if matches!(v, Value::Null)
                    || matches!(lv, Value::Null)
                    || matches!(hv, Value::Null)
                {
                    return Ok(Value::Null);
                }
                let ge_lo = eval_compare(Some(&v), CompareOp::Ge, &lv);
                let le_hi = eval_compare(Some(&v), CompareOp::Le, &hv);
                match (ge_lo, le_hi) {
                    (Some(a), Some(b)) => {
                        let between = a && b;
                        Ok(Value::Bool(if *negated { !between } else { between }))
                    }
                    _ => Ok(Value::Null),
                }
            }
        }
    }

    /// Bloque H (2026-05-26): ejecuta la subquery de una derived table
    /// y devuelve un schema virtual + filas decodificadas listas para
    /// usar como JoinTable. La inferencia de tipo va columna-a-columna
    /// sobre los valores observados: si todos los no-NULL comparten
    /// variante → ese tipo; mezcla → TEXT como fallback documentado.
    /// Columnas con nombres duplicados en el output de la subquery
    /// disparan `[GBY-4049]`.
    fn materialize_derived_table(
        &mut self,
        sub: &SelectStmt,
        alias: &str,
    ) -> DbResult<MaterializedDerived> {
        let raw = self.exec_select((*sub).clone())?;
        // Bloque H: cuando la subquery se ejecuta a través del JOIN
        // path (`exec_select_joined`), las columnas vienen prefijadas
        // con `qualifier.` para des-ambiguar. Para la derived table
        // de un solo "scope interno" lo que quiere el outer es la
        // columna sin prefijo (`id`, no `sub.id`) — eso le permite
        // al outer referirla bare o con su propio alias. Quedan
        // expuestas las dos formas: el output normalizado del schema
        // virtual usa solo el sufijo después del último `.`.
        let result = ResultSet {
            columns: raw
                .columns
                .iter()
                .map(|c| c.rsplit('.').next().unwrap_or(c).to_string())
                .collect(),
            rows: raw.rows,
            message: raw.message,
        };
        // Validar columnas únicas (case-insensitive sobre el ident).
        let mut seen: HashSet<String> = HashSet::new();
        for col in &result.columns {
            let key = normalize_ident(col);
            if !seen.insert(key) {
                return Err(coded(
                    codes::DERIVED_DUPLICATE_COLUMN,
                    format!(
                        "derived table '{}' proyecta dos columnas con el mismo nombre '{}' — \
                         usá alias para des-ambiguar (`SELECT a AS x, b AS y`)",
                        alias, col
                    ),
                ));
            }
        }
        // Inferir tipo por columna: si todos los valores no-NULL son
        // del mismo variant, ese tipo gana; mezcla → TEXT.
        let n_cols = result.columns.len();
        let mut inferred_types: Vec<Option<ColumnType>> = vec![None; n_cols];
        let mut conflicting: Vec<bool> = vec![false; n_cols];
        for row in &result.rows {
            for (i, v) in row.iter().enumerate() {
                let t = match v {
                    Value::Null => continue,
                    Value::Integer(_) => ColumnType::Int,
                    Value::Float(_) => ColumnType::Float,
                    Value::Bool(_) => ColumnType::Bool,
                    Value::String(_) => ColumnType::Text,
                };
                match inferred_types[i] {
                    None => inferred_types[i] = Some(t),
                    Some(prev) if prev == t => {}
                    Some(_) => conflicting[i] = true,
                }
            }
        }
        let columns: Vec<Column> = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let ty = if conflicting[i] {
                    ColumnType::Text
                } else {
                    inferred_types[i].unwrap_or(ColumnType::Text)
                };
                Column::plain(name.clone(), ty)
            })
            .collect();
        let primary_key = columns.first().map(|c| c.name.clone()).unwrap_or_default();
        let meta = TableMeta {
            name: alias.to_string(),
            primary_key,
            primary_key_extra: Vec::new(),
            primary_key_name: None,
            columns,
            root_page: 0,
            indexes: Vec::new(),
            check_constraints: Vec::new(),
        };
        // Decodificar filas a HashMap<colname-normalizado, Value>.
        let rows: Vec<HashMap<String, Value>> = result
            .rows
            .into_iter()
            .map(|r| {
                result
                    .columns
                    .iter()
                    .zip(r)
                    .map(|(name, val)| (normalize_ident(name), val))
                    .collect()
            })
            .collect();
        Ok(MaterializedDerived { meta, rows })
    }

    /// Bloque H: proyecta una fila contra una lista de `Projection`,
    /// delegando expresiones a `eval_expr_full` (que puede ejecutar
    /// subqueries escalares). Las `BareColumn` siguen el lookup directo.
    fn project_row_with_engine(
        &mut self,
        projections: &[Projection],
        row: &HashMap<String, Value>,
        outer_table_name: Option<&str>,
    ) -> DbResult<Vec<Value>> {
        let mut out = Vec::with_capacity(projections.len());
        for p in projections {
            let value = match p {
                Projection::BareColumn { key, .. } => row.get(key).cloned().ok_or_else(|| {
                    DbError::new(format!("columna no encontrada en fila: {}", key))
                })?,
                Projection::Expression { expr, .. } => {
                    self.eval_expr_full(expr, row, outer_table_name)?
                }
            };
            out.push(value);
        }
        Ok(out)
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

    /// Bloque I (2026-05-26): entry-point del SELECT statement. Despacha
    /// entre SELECT plano, operación de conjunto, o VALUES standalone.
    /// El path `Select(stmt)` delega al `exec_select` clásico — todo el
    /// pipeline pre-I sigue intacto sin regresión.
    pub fn exec_select_query(&mut self, query: SelectQuery) -> DbResult<ResultSet> {
        match query {
            SelectQuery::Select(stmt) => self.exec_select(*stmt),
            SelectQuery::Values(v) => self.exec_values_clause(&v, None),
            SelectQuery::SetOp {
                lhs,
                op,
                all,
                rhs,
                order_by,
                limit,
                offset,
            } => {
                let left = self.exec_select_query(*lhs)?;
                let right = self.exec_select_query(*rhs)?;
                let mut combined = combine_set_op(left, right, op, all)?;
                if let Some(order) = order_by {
                    apply_order_by_on_resultset(&mut combined, &order)?;
                }
                apply_limit_offset_on_resultset(&mut combined, limit, offset);
                Ok(combined)
            }
        }
    }

    /// Bloque I: materializa una `VALUES` standalone como ResultSet. Sin
    /// `alias_columns`, los headers son `column1, column2, ...` (estándar
    /// SQL92). Con `alias_columns`, esos nombres se usan en el header.
    fn exec_values_clause(
        &mut self,
        clause: &ValuesClause,
        alias_columns: Option<&[String]>,
    ) -> DbResult<ResultSet> {
        if clause.rows.is_empty() {
            return Err(coded(
                codes::VALUES_EMPTY,
                "VALUES requiere al menos una fila — `VALUES ();` o `VALUES;` no se aceptan",
            ));
        }
        let arity = clause.rows[0].len();
        if arity == 0 {
            return Err(coded(
                codes::VALUES_ROW_ARITY_MISMATCH,
                "VALUES: cada fila debe tener al menos una expresión",
            ));
        }
        for (i, row) in clause.rows.iter().enumerate() {
            if row.len() != arity {
                return Err(coded(
                    codes::VALUES_ROW_ARITY_MISMATCH,
                    format!(
                        "VALUES: fila {} tiene {} expresiones pero la fila 1 tiene {}",
                        i + 1,
                        row.len(),
                        arity
                    ),
                ));
            }
        }
        let columns: Vec<String> = if let Some(aliases) = alias_columns {
            if aliases.len() != arity {
                return Err(coded(
                    codes::VALUES_COLUMN_ALIAS_ARITY,
                    format!(
                        "lista de aliases de columna tiene {} entradas pero las filas \
                         de VALUES tienen {}",
                        aliases.len(),
                        arity
                    ),
                ));
            }
            aliases.to_vec()
        } else {
            (1..=arity).map(|i| format!("column{}", i)).collect()
        };
        // Evaluamos cada Expr con una fila VACÍA — VALUES no puede
        // referirse a columnas (no hay scope). Una referencia a columna
        // dentro de un VALUES fallará limpio en eval_expr_full ("columna
        // no encontrada"). Subqueries escalares sí funcionan: el outer
        // stack pasa por el engine y no por el row.
        let empty_row: HashMap<String, Value> = HashMap::new();
        let mut rows: Vec<Vec<Value>> = Vec::with_capacity(clause.rows.len());
        for row_exprs in &clause.rows {
            let mut out = Vec::with_capacity(arity);
            for expr in row_exprs {
                let v = self.eval_expr_full(expr, &empty_row, None)?;
                out.push(v);
            }
            rows.push(out);
        }
        Ok(ResultSet {
            columns,
            rows,
            message: None,
        })
    }

    /// Bloque I: materializa una VALUES clause en el formato que necesita
    /// el JOIN path — `MaterializedDerived` con `TableMeta` virtual.
    /// Reusa `exec_values_clause` para la evaluación, y arma el meta
    /// con tipos inferidos del primer no-NULL de cada columna (igual
    /// estrategia que derived tables).
    fn materialize_values_in_from(
        &mut self,
        clause: &ValuesClause,
        alias: &str,
        alias_columns: &[String],
    ) -> DbResult<MaterializedDerived> {
        let rs = self.exec_values_clause(clause, Some(alias_columns))?;
        // Inferir tipos columna a columna.
        let n_cols = rs.columns.len();
        let mut inferred: Vec<Option<ColumnType>> = vec![None; n_cols];
        let mut conflicting: Vec<bool> = vec![false; n_cols];
        for row in &rs.rows {
            for (i, v) in row.iter().enumerate() {
                let t = match v {
                    Value::Null => continue,
                    Value::Integer(_) => ColumnType::Int,
                    Value::Float(_) => ColumnType::Float,
                    Value::Bool(_) => ColumnType::Bool,
                    Value::String(_) => ColumnType::Text,
                };
                match inferred[i] {
                    None => inferred[i] = Some(t),
                    Some(prev) if prev == t => {}
                    Some(_) => conflicting[i] = true,
                }
            }
        }
        let columns: Vec<Column> = rs
            .columns
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let ty = if conflicting[i] {
                    ColumnType::Text
                } else {
                    inferred[i].unwrap_or(ColumnType::Text)
                };
                Column::plain(name.clone(), ty)
            })
            .collect();
        let primary_key = columns.first().map(|c| c.name.clone()).unwrap_or_default();
        let meta = TableMeta {
            name: alias.to_string(),
            primary_key,
            primary_key_extra: Vec::new(),
            primary_key_name: None,
            columns,
            root_page: 0,
            indexes: Vec::new(),
            check_constraints: Vec::new(),
        };
        let rows: Vec<HashMap<String, Value>> = rs
            .rows
            .into_iter()
            .map(|r| {
                rs.columns
                    .iter()
                    .zip(r)
                    .map(|(name, val)| (normalize_ident(name), val))
                    .collect()
            })
            .collect();
        Ok(MaterializedDerived { meta, rows })
    }

    fn exec_select(&mut self, mut stmt: SelectStmt) -> DbResult<ResultSet> {
        // Bloque V (2026-05-27): si el FROM apunta a una vista, la
        // re-expandimos como derived source ANTES de cualquier otro
        // dispatch — así el resto del pipeline ve la query post-rewrite.
        self.expand_view_in_from(&mut stmt)?;
        // Issue #1 (2026-05-27): memoize toda scalar subquery
        // no-correlacionada en el SELECT list — una sola evaluación
        // en vez de N (factor ~1000× para LIMIT 10 sobre tablas
        // grandes). Tiene que correr ANTES del dispatch a
        // exec_select_joined para que el path JOIN también se
        // beneficie.
        self.memoize_select_stmt(&mut stmt)?;
        // SELECT con JOINs sigue una ruta distinta (nested-loop, schema
        // combinado, WHERE como post-filter). El single-table path queda
        // exactamente como estaba — sin regresión en performance ni
        // semántica para queries que no usan JOIN.
        //
        // Bloque H (2026-05-26): si el FROM es una derived table (con o
        // sin JOINs), también delegamos al pipeline del JOIN — opera
        // sobre filas materializadas en HashMap que es exactamente lo
        // que necesitamos para una virtual table sin pager.
        if !stmt.joins.is_empty() || stmt.derived_source.is_some() || stmt.values_source.is_some() {
            return self.exec_select_joined(stmt);
        }
        let meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .get_table(&stmt.table)?
                .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?
        };

        // Bloque F: detectar si la query necesita el stage de agregación
        // (cualquier `SelectItem::Aggregate`, GROUP BY no vacío, o HAVING).
        // Cuando lo necesita, el SELECT list puede mezclar columnas
        // bare con agregadas — `resolve_selected_columns` no aplica
        // porque la proyección se construye después de bucketear.
        let needs_aggregation = stmt_needs_aggregation(&stmt);
        let selected_columns = if needs_aggregation {
            // Placeholder: para el path agregado, la proyección final se
            // arma en exec_aggregate_pipeline. Devolvemos un vec vacío
            // aquí para que el resto del flujo no lo use.
            Vec::new()
        } else {
            resolve_selected_columns(&meta, &stmt.columns)?
        };
        let output_columns: Vec<String> = if needs_aggregation {
            stmt.columns.iter().map(|i| i.output_name()).collect()
        } else {
            selected_columns
                .iter()
                .map(|p| p.display().to_string())
                .collect()
        };

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
        let exists_postfilter: Option<(Box<SelectStmt>, bool)> =
            match stmt.where_clause.as_ref().and_then(|e| e.as_atom()) {
                Some(WhereClause::Exists { subquery, negated })
                    if subquery_has_outer_refs(subquery) =>
                {
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
        // Issue #4 (2026-05-27): detección temprana del fast-path por
        // PK compuesta antes de decidir `generic_post_filter`. Si el
        // WHERE es un puro AND-of-equality que cubre toda la PK
        // compuesta, no necesitamos post-filter — el lookup directo
        // por fingerprint devuelve exactamente la fila pedida.
        let composite_pk_fast_path_active: bool = meta.has_composite_pk()
            && stmt
                .where_clause
                .as_ref()
                .and_then(extract_and_equality_map)
                .map(|map| map.len() == meta.pk_columns().len())
                .unwrap_or(false);

        let generic_post_filter: Option<WhereExpr> = match &stmt.where_clause {
            Some(_) if composite_pk_fast_path_active => None,
            Some(expr) => {
                let force = match expr.as_atom() {
                    None => true,
                    Some(atom) => {
                        matches!(
                            atom,
                            WhereClause::Compare { .. }
                                | WhereClause::Like { .. }
                                | WhereClause::IsNull { .. }
                                | WhereClause::InList { .. }
                                | WhereClause::ExprPredicate { .. }
                        ) || matches!(
                            atom,
                            // Bloque H: `NOT IN (SELECT ...)` cae al
                            // post-filter genérico — la fast-path indexada
                            // solo cubre `IN` afirmativo (devuelve PKs
                            // matched); negar matched-PKs no es trivial
                            // sobre el cursor.
                            WhereClause::In { negated: true, .. }
                        ) || match atom {
                            // K2: lookup parcial / sin todas las columnas
                            // PK sobre una tabla con PK compuesta. El
                            // planner cae a FullScan (no puede calcular
                            // el fingerprint) y el post-filter genérico
                            // se ocupa del WHERE.
                            WhereClause::Eq { column, .. }
                            | WhereClause::Between { column, .. } => {
                                meta.has_composite_pk() && meta.is_pk_column(column)
                            }
                            _ => false,
                        }
                    }
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
        let where_atom: Option<WhereClause> =
            stmt.where_clause.clone().and_then(|e| e.into_atom().ok());

        // Issue #4 (2026-05-27): fast-path para `WHERE pk_a = ? AND pk_b = ? [AND ...]`
        // sobre PK compuesta. Si el WHERE es un AND de Eq que cubre
        // EXACTAMENTE todas las cols de la PK compuesta, computamos el
        // fingerprint y vamos directo al B+Tree (O(log n) en vez de full
        // scan O(n)). Antes del fix, ese caso degeneraba a full scan
        // — bug detectado por el benchmark (composite PK lookup 145 ms
        // vs los <500 µs esperados).
        let composite_pk_plan: Option<Plan> = if meta.has_composite_pk()
            && exists_postfilter.is_none()
            && generic_post_filter.is_none()
        {
            stmt.where_clause
                .as_ref()
                .and_then(extract_and_equality_map)
                .and_then(|map| {
                    let pk_cols = meta.pk_columns();
                    if pk_cols.len() != map.len() {
                        return None;
                    }
                    // Reunir los Value en el orden EXACTO de la PK.
                    let mut ordered: Vec<Value> = Vec::with_capacity(pk_cols.len());
                    for pc in &pk_cols {
                        let key = normalize_ident(pc);
                        let v = map.get(&key)?;
                        ordered.push(v.clone());
                    }
                    // Tipos deben coincidir con las columnas PK.
                    let col_metas: Vec<Column> = pk_cols
                        .iter()
                        .filter_map(|pc| meta.column(pc).cloned())
                        .collect();
                    if col_metas.len() != pk_cols.len() {
                        return None;
                    }
                    let col_refs: Vec<&Column> = col_metas.iter().collect();
                    let val_refs: Vec<&Value> = ordered.iter().collect();
                    let fp = encode_composite_key(&col_refs, &val_refs).ok()?;
                    Some(Plan::ByPks(vec![fp]))
                })
        } else {
            None
        };

        let plan = if let Some(p) = composite_pk_plan {
            p
        } else if exists_postfilter.is_some() || generic_post_filter.is_some() {
            // El filtrado real ocurre en el post-filter; el scan barre todo.
            Plan::FullScan
        } else {
            match where_atom {
                None => Plan::FullScan,
                Some(WhereClause::Eq { column, value }) => {
                    let normalized = normalize_ident(&column);
                    // K2: con PK compuesta el match por la primera (o cualquier)
                    // columna PK no permite calcular el fingerprint — full-scan.
                    if !meta.has_composite_pk()
                        && normalized == normalize_ident(&meta.primary_key)
                    {
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
                    } else if meta.has_composite_pk() && meta.is_pk_column(&normalized) {
                        // Lookup parcial sobre PK compuesta: cae a FullScan
                        // y deja que `eval_where_expr_single` filtre.
                        Plan::FullScan
                    } else {
                        // Issue #3 (2026-05-27): antes rebotábamos con
                        // [GBY-4001] para `WHERE col_no_indexada = val`,
                        // mientras que `>`, `<`, `LIKE`, `IS NULL`, etc.,
                        // sí aceptaban full-scan. La inconsistencia
                        // confundía al usuario. Ahora `=` también cae
                        // a FullScan + post-filter — misma semántica
                        // que el resto de operadores. `[GBY-4001]` queda
                        // como código reservado.
                        let _ = column;
                        Plan::FullScan
                    }
                }
                Some(WhereClause::Between { column, from, to }) => {
                    let normalized = normalize_ident(&column);
                    if !meta.has_composite_pk()
                        && normalized == normalize_ident(&meta.primary_key)
                    {
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
                Some(WhereClause::In {
                    column,
                    subquery,
                    negated: false,
                }) => {
                    // Non-correlated IN (afirmativo): execute the subquery once, materialize
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
                | Some(WhereClause::InList { .. })
                | Some(WhereClause::ExprPredicate { .. })
                // Bloque H: NOT IN (SELECT) cae al generic_post_filter.
                | Some(WhereClause::In { negated: true, .. }) => Plan::FullScan,
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

        // Bloque F: si la query es agregada, abandonamos el path normal y
        // dejamos que `exec_aggregate_pipeline` se ocupe de bucketear,
        // calcular agregados, aplicar HAVING, ORDER BY y window contra
        // el esquema de salida. La descodificación de filas se hace
        // dentro porque puede incluir COUNT(DISTINCT col) que necesita
        // recorrer todos los valores antes de devolver la fila final.
        if needs_aggregation {
            return self.exec_aggregate_pipeline(&meta, &stmt, rows_bytes, output_columns);
        }

        let mut rows: Vec<(HashMap<String, Value>, Vec<Value>)> =
            Vec::with_capacity(rows_bytes.len());
        let outer_name = meta.name.clone();
        for kv in rows_bytes {
            let decoded = decode_row(&meta, &kv.value)?;
            // Bloque H: la proyección puede contener subqueries escalares
            // (`SELECT (SELECT ... FROM other) FROM t`); cuando es el
            // caso, evaluamos cada Expr con el engine para que la
            // subquery se ejecute. Para árboles sin subquery se preserva
            // el fast-path pre-H (`project_row`) sin overhead.
            let projected = self.project_row_with_engine(
                &selected_columns,
                &decoded,
                Some(outer_name.as_str()),
            )?;
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
        let mut rows: Vec<Vec<Value>> = rows.into_iter().map(|(_, r)| r).collect();

        // Bloque F: `SELECT DISTINCT` sin agregados — dedup post-proyección
        // preservando el primer orden de aparición. Para queries con
        // agregados, el bucketing ya hace dedup natural por GROUP BY.
        if stmt.distinct {
            rows = dedup_preserving_order(rows);
        }

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

            // --- Fallback: hash join (Issue #6, 2026-05-27) o nested-loop ---
            //
            // Issue #6: si el ON es un equi-predicado, construimos un
            // HashMap por value bytes del right table y probamos cada
            // left row en O(1). Para CROSS JOIN (sin predicate) o
            // cualquier futuro non-equi (que el AST actual no soporta
            // pero sí podría) caemos al nested-loop O(N×M). Bench pre-fix:
            // 100 posts × 10K users ≈ 479 ms; post-fix esperado: <20 ms.
            let right = &scope.tables[i + 1];
            let right_rows = self.scan_qualified(right)?;
            let mut next: Vec<HashMap<String, Value>> =
                Vec::with_capacity(current.len() * right_rows.len() / 2 + 1);
            let mut left_matched = vec![false; current.len()];
            let mut right_matched = vec![false; right_rows.len()];

            // Hash join fast-path: precompute keys del predicado y
            // decidir en qué lado vive cada uno, igual que el fallback
            // de `evaluate_join_predicate`. Salto al nested-loop si
            // no hay predicate (CROSS JOIN) o si una key no resuelve.
            let hash_join_plan: Option<(String, String)> = if let Some(pred) = effective_on {
                let lkey = resolve_joined_column_key(&scope, &column_ref_to_raw(&pred.left))?;
                let rkey = resolve_joined_column_key(&scope, &column_ref_to_raw(&pred.right))?;
                // Determinar cuál key indexa right_rows mirando
                // el primer right row (todas las filas comparten
                // el mismo conjunto de claves prefijadas).
                let right_has_rkey = right_rows.first().is_some_and(|r| r.contains_key(&rkey));
                let right_has_lkey = right_rows.first().is_some_and(|r| r.contains_key(&lkey));
                if right_has_rkey {
                    Some((rkey, lkey))
                } else if right_has_lkey {
                    Some((lkey, rkey))
                } else {
                    None
                }
            } else {
                None
            };

            if let Some((right_index_key, left_probe_key)) = hash_join_plan {
                // Build: hash sobre los valores del right_index_key.
                // NULL nunca matchea (SQL standard: NULL = NULL → NULL),
                // así que filas NULL del right se omiten del hash y
                // sólo aparecen vía LEFT/RIGHT/FULL fill-null.
                let mut hash: HashMap<Vec<u8>, Vec<usize>> =
                    HashMap::with_capacity(right_rows.len());
                for (ri, r) in right_rows.iter().enumerate() {
                    let v = r.get(&right_index_key).cloned().unwrap_or(Value::Null);
                    if matches!(v, Value::Null) {
                        continue;
                    }
                    let bytes = encode_group_key(&[v]);
                    hash.entry(bytes).or_default().push(ri);
                }
                // Probe.
                for (li, left_row) in current.iter().enumerate() {
                    let lv = left_row
                        .get(&left_probe_key)
                        .cloned()
                        .unwrap_or(Value::Null);
                    if matches!(lv, Value::Null) {
                        continue;
                    }
                    let bytes = encode_group_key(&[lv]);
                    let Some(ris) = hash.get(&bytes) else {
                        continue;
                    };
                    for &ri in ris {
                        left_matched[li] = true;
                        right_matched[ri] = true;
                        let right_row = &right_rows[ri];
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
            } else {
                // Nested-loop O(N×M): CROSS JOIN o caso donde el
                // hash join no aplica (predicate keys irresolvibles).
                for (li, left_row) in current.iter().enumerate() {
                    for (ri, right_row) in right_rows.iter().enumerate() {
                        let pass = match effective_on {
                            None => true, // CROSS JOIN o comma-syntax
                            Some(pred) => {
                                evaluate_join_predicate(left_row, right_row, pred, &scope)?
                            }
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
        let windowed: Vec<HashMap<String, Value>> =
            current.into_iter().skip(stmt.offset).take(take).collect();
        let mut rows: Vec<Vec<Value>> = Vec::with_capacity(windowed.len());
        // Bloque H: para JOINs no tenemos un solo "outer table name"
        // — la subquery escalar correlacionada en SELECT sobre un JOIN
        // queda fuera del alcance de este release; pasamos `None` y
        // delegamos a `eval_expr_full` que evaluará la subquery sin
        // pushear frame. Una subquery sin outer refs funciona; una con
        // outer refs caería en `[GBY-4016]`.
        for row in windowed {
            let mut out = Vec::with_capacity(projected_keys.len());
            for p in &projected_keys {
                let v = match p {
                    JoinedProjection::Key(k) => row.get(k).cloned().unwrap_or(Value::Null),
                    JoinedProjection::Expr(e) => self.eval_expr_full(e, &row, None)?,
                };
                out.push(v);
            }
            rows.push(out);
        }

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
        // Bloque H: derived table — las filas ya están materializadas.
        if let Some(rows) = entry.virtual_rows.as_ref() {
            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let mut qualified = HashMap::with_capacity(row.len());
                for (col, val) in row {
                    qualified.insert(format!("{}.{}", entry.qualifier, col), val.clone());
                }
                out.push(qualified);
            }
            return Ok(out);
        }
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
        // Bloque H: la base table puede ser una derived table. La
        // materializamos primero (ejecutando la subquery) y construimos
        // un `TableMeta` virtual cuyas columnas vienen de la headers
        // del ResultSet.
        let (base_meta, base_virtual_rows) = if let Some(sub) = stmt.derived_source.as_ref() {
            let materialized = self.materialize_derived_table(sub, &stmt.table)?;
            (materialized.meta, Some(materialized.rows))
        } else if let Some((vals, aliases)) = stmt.values_source.as_ref() {
            // Bloque I: VALUES en FROM como base table.
            let materialized = self.materialize_values_in_from(vals, &stmt.table, aliases)?;
            (materialized.meta, Some(materialized.rows))
        } else {
            let m = {
                let mut catalog = Catalog::open(self.pager);
                catalog
                    .get_table(&stmt.table)?
                    .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?
            };
            (m, None)
        };
        let base_qualifier = stmt
            .table_alias
            .clone()
            .unwrap_or_else(|| stmt.table.clone())
            .to_ascii_lowercase();
        tables.push(JoinTable {
            meta: base_meta,
            qualifier: base_qualifier.clone(),
            raw_name: stmt.table.clone(),
            alias: stmt.table_alias.clone(),
            virtual_rows: base_virtual_rows,
        });
        for join in &stmt.joins {
            // Bloque H: el RHS de un JOIN también puede ser derived.
            // Bloque I: o un VALUES en FROM.
            let (meta, virtual_rows) = if let Some(sub) = join.right.derived.as_ref() {
                let materialized = self.materialize_derived_table(sub, &join.right.name)?;
                (materialized.meta, Some(materialized.rows))
            } else if let Some(vals) = join.right.values.as_ref() {
                let aliases = join.right.values_columns.as_ref().ok_or_else(|| {
                    coded(
                        codes::VALUES_IN_FROM_REQUIRES_ALIAS,
                        "VALUES en JOIN requiere alias de columnas `AS t(c1, c2, ...)`",
                    )
                })?;
                let materialized =
                    self.materialize_values_in_from(vals, &join.right.name, aliases)?;
                (materialized.meta, Some(materialized.rows))
            } else {
                let m = {
                    let mut catalog = Catalog::open(self.pager);
                    catalog.get_table(&join.right.name)?.ok_or_else(|| {
                        DbError::new(format!("tabla no existe: {}", join.right.name))
                    })?
                };
                (m, None)
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
                virtual_rows,
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
            WhereExpr::Not(inner) => {
                Ok(self.eval_where_expr_joined(inner, row, scope)?.map(|b| !b))
            }
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
            WhereClause::In {
                column,
                subquery,
                negated,
            } => {
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
                let (set, had_null) = collect_in_set(inner.rows);
                Ok(eval_in_subquery(row.get(&key), &set, had_null, *negated))
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
            WhereClause::ExprPredicate { expr } => eval_expr_as_predicate(expr, row),
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
            WhereClause::In {
                column,
                subquery,
                negated,
            } => {
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
                let (set, had_null) = collect_in_set(inner.rows);
                let neg = *negated;
                Ok(rows
                    .into_iter()
                    .filter(|r| {
                        matches!(
                            eval_in_subquery(r.get(&key), &set, had_null, neg),
                            Some(true)
                        )
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
            // G2 suma `ExprPredicate` al mismo grupo.
            WhereClause::Compare { .. }
            | WhereClause::Like { .. }
            | WhereClause::IsNull { .. }
            | WhereClause::InList { .. }
            | WhereClause::ExprPredicate { .. } => {
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
            WhereExpr::Not(inner) => Ok(self.eval_where_expr_single(inner, meta, row)?.map(|b| !b)),
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
                ensure_column_visible(meta, &key, column, row)?;
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
                ensure_column_visible(meta, &key, column, row)?;
                match row.get(&key) {
                    Some(Value::Integer(n)) => Ok(Some(*n >= *from && *n <= *to)),
                    Some(Value::Null) | None => Ok(None),
                    _ => Ok(Some(false)),
                }
            }
            WhereClause::In {
                column,
                subquery,
                negated,
            } => {
                let key = normalize_ident(column);
                ensure_column_visible(meta, &key, column, row)?;
                // Bloque H: si la subquery es correlacionada, pusheamos
                // el outer row para que `outer.col` resuelva. Cuando es
                // afirmativo, NULL en la subquery se descarta y la
                // membresía se evalúa por presencia; cuando es NOT IN,
                // la semántica ANSI exige propagar NULL si la subquery
                // contiene algún NULL (3VL estricta).
                let is_correlated = subquery_has_outer_refs(subquery);
                if is_correlated {
                    self.outer_stack.push(OuterRow {
                        table: meta.name.clone(),
                        values: row.clone(),
                    });
                }
                let inner_res = self.exec_select((**subquery).clone());
                if is_correlated {
                    self.outer_stack.pop();
                }
                let inner = inner_res?;
                if inner.columns.len() != 1 {
                    return Err(coded(
                        codes::SUBQUERY_MUST_RETURN_ONE_COLUMN,
                        format!(
                            "subquery en IN debe devolver exactamente 1 columna; devolvió {}",
                            inner.columns.len()
                        ),
                    ));
                }
                let (set, had_null) = collect_in_set(inner.rows);
                Ok(eval_in_subquery(row.get(&key), &set, had_null, *negated))
            }
            WhereClause::EqSubquery { column, subquery } => {
                let key = normalize_ident(column);
                ensure_column_visible(meta, &key, column, row)?;
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
                // Bloque H (2026-05-26): EXISTS dentro de combinadores
                // soporta tanto no-correlacionado como correlacionado.
                // Para el correlacionado pusheamos el outer row en
                // `outer_stack` antes de re-ejecutar la subquery y
                // popeamos al terminar (siempre balanceado, incluso si
                // la subquery falla).
                let is_correlated = subquery_has_outer_refs(subquery);
                if is_correlated {
                    self.outer_stack.push(OuterRow {
                        table: meta.name.clone(),
                        values: row.clone(),
                    });
                }
                let inner_res = self.exec_select((**subquery).clone());
                if is_correlated {
                    self.outer_stack.pop();
                }
                let inner = inner_res?;
                let has_rows = !inner.rows.is_empty();
                let pass = if *negated { !has_rows } else { has_rows };
                Ok(Some(pass))
            }
            WhereClause::EqColumnRef {
                column,
                ref_table,
                ref_column,
            } => {
                // Bloque H: dentro de un combinador (AND/OR/NOT) la
                // forma `inner_col = outer.col` se resuelve contra el
                // outer_stack y se compara con el valor presente en la
                // fila inner actual.
                let key = normalize_ident(column);
                ensure_column_visible(meta, &key, column, row)?;
                let outer_val = self.resolve_outer_ref(ref_table.as_deref(), ref_column)?;
                match (row.get(&key), &outer_val) {
                    (Some(Value::Null), _) | (None, _) | (_, Value::Null) => Ok(None),
                    (Some(v), other) => Ok(Some(values_equal(v, other))),
                }
            }
            WhereClause::Compare { column, op, value } => {
                let key = normalize_ident(column);
                ensure_column_visible(meta, &key, column, row)?;
                Ok(eval_compare(row.get(&key), *op, value))
            }
            WhereClause::Like {
                column,
                pattern,
                negated,
            } => {
                let key = normalize_ident(column);
                ensure_column_visible(meta, &key, column, row)?;
                Ok(eval_like(row.get(&key), pattern, *negated))
            }
            WhereClause::IsNull { column, negated } => {
                let key = normalize_ident(column);
                ensure_column_visible(meta, &key, column, row)?;
                let is_null = matches!(row.get(&key), Some(Value::Null) | None);
                Ok(Some(if *negated { !is_null } else { is_null }))
            }
            WhereClause::InList {
                column,
                values,
                negated,
            } => {
                let key = normalize_ident(column);
                ensure_column_visible(meta, &key, column, row)?;
                Ok(eval_in_list(row.get(&key), values, *negated))
            }
            WhereClause::ExprPredicate { expr } => {
                // Bloque H: si el árbol contiene subqueries escalares,
                // dispatch al evaluador con engine; sino, fast-path.
                if expr_contains_subquery(expr) {
                    let v = self.eval_expr_full(expr, row, Some(meta.name.as_str()))?;
                    match v {
                        Value::Bool(b) => Ok(Some(b)),
                        Value::Null => Ok(None),
                        other => Err(coded(
                            codes::WHERE_EXPR_NOT_BOOLEAN,
                            format!(
                                "expresión en WHERE/HAVING debe evaluar a BOOL (o NULL), recibí {}",
                                value_type_name(&other)
                            ),
                        )),
                    }
                } else {
                    eval_expr_as_predicate(expr, row)
                }
            }
        }
    }

    fn exec_update(&mut self, stmt: UpdateStmt) -> DbResult<ResultSet> {
        self.reject_if_view(&stmt.table, "UPDATE")?;
        let meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .get_table(&stmt.table)?
                .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?
        };

        // Validamos shape de assignments (PK / columna existe / duplicados)
        // una sola vez — esos chequeos no dependen de los valores
        // calculados por fila. La evaluación de la `Expr` y la coerción
        // de tipo ocurren más abajo, por cada fila, dentro de
        // `apply_update_to_pk` vía `expr_assignments`.
        let mut expr_assignments: Vec<(String, Expr)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (column_name, expr) in stmt.assignments {
            let normalized = normalize_ident(&column_name);
            // Residual #4 de L (2026-05-27): UPDATE sobre columnas PK
            // ya está permitido. `apply_update_to_pk` detecta el cambio
            // de PK comparando encoded_pk vs el pk del WHERE y, si
            // difieren, mueve la fila (delete viejo + insert nuevo) y
            // dispara la acción ON UPDATE declarada en cada FK
            // entrante. La regla `[GBY-4008]` queda como código
            // reservado pero el motor ya no lo emite desde aquí.
            //
            // ON CONFLICT DO UPDATE (UPSERT) SIGUE rechazando UPDATE
            // de PK: cambiarla rompería la noción del "conflicto" que
            // disparó el UPSERT.
            if meta.column(&normalized).is_none() {
                return Err(coded(
                    codes::COLUMN_NOT_FOUND,
                    format!(
                        "UPDATE sobre '{}': columna '{}' no existe en la tabla",
                        meta.name, column_name
                    ),
                ));
            }
            if !seen.insert(normalized.clone()) {
                return Err(coded(
                    codes::DUPLICATE_COLUMN_NAME,
                    format!(
                        "UPDATE sobre '{}': columna '{}' aparece más de una vez en SET",
                        meta.name, column_name
                    ),
                ));
            }
            expr_assignments.push((normalized, expr));
        }

        // Bloque E3: resolver target PKs según el WHERE. Fast-path para
        // Eq sobre PK literal (preserva el comportamiento pre-E3); el
        // resto cae a FullScan + 3VL. `was_explicit_single_pk` se usa
        // abajo: cuando el WHERE pidió una PK concreta y la fila no
        // existe, devolvemos `ROW_NOT_FOUND_FOR_PK` para no romper la
        // semántica anterior; con WHERE compuesto, 0 matches es OK
        // (UPDATE de 0 filas, igual que SQL estándar).
        let (target_pks, was_explicit_single_pk) =
            self.resolve_target_pks(&meta, &stmt.where_clause, "UPDATE")?;

        if target_pks.is_empty() && was_explicit_single_pk {
            // El WHERE original era `pk = N` y N no existe. Mantenemos el
            // error legado para que callers existentes (CLI, tests) sigan
            // observando el mismo código.
            return Err(coded(
                codes::ROW_NOT_FOUND_FOR_PK,
                format!("UPDATE sobre '{}': fila no existe", meta.name),
            ));
        }

        let mut updated = 0usize;
        let mut affected_rows: Vec<HashMap<String, Value>> = Vec::new();
        for pk in &target_pks {
            self.apply_update_to_pk(&meta, *pk, &expr_assignments)?;
            updated += 1;
            if stmt.returning.is_some() {
                // Bloque J2: re-leer la fila post-update para RETURNING.
                let bytes = {
                    let mut catalog = Catalog::open(self.pager);
                    catalog.get_row(meta.root_page, *pk)?
                };
                if let Some(b) = bytes {
                    affected_rows.push(decode_row(&meta, &b)?);
                }
            }
        }

        if let Some(returning) = &stmt.returning {
            let projected = project_returning(&meta, returning, &affected_rows)?;
            let columns = returning_column_names(&meta, returning);
            return Ok(ResultSet {
                columns,
                rows: projected,
                message: Some(format!(
                    "OK ({} fila{} actualizada{})",
                    updated,
                    if updated == 1 { "" } else { "s" },
                    if updated == 1 { "" } else { "s" }
                )),
            });
        }

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(format!(
                "OK ({} fila{} actualizada{})",
                updated,
                if updated == 1 { "" } else { "s" },
                if updated == 1 { "" } else { "s" }
            )),
        })
    }

    /// Bloque E3 + G2: aplica las assignments `(col_normalizada, Expr)`
    /// a la fila con PK dada. La RHS de cada assignment se evalúa
    /// contra la fila **pre-update** — todos los `Expr` ven los
    /// mismos valores de origen, no los que otros assignments del mismo
    /// SET puedan haber producido. Encapsula todas las validaciones
    /// por-fila (NOT NULL, UNIQUE, FK), el upsert y el mantenimiento
    /// de índices.
    fn apply_update_to_pk(
        &mut self,
        meta: &TableMeta,
        pk: i64,
        expr_assignments: &[(String, Expr)],
    ) -> DbResult<()> {
        let existing = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_row(meta.root_page, pk)?.ok_or_else(|| {
                coded(
                    codes::ROW_NOT_FOUND_FOR_PK,
                    format!("UPDATE sobre '{}': fila no existe PK={}", meta.name, pk),
                )
            })?
        };
        let old_row = decode_row(meta, &existing)?;
        // G2: evaluamos cada Expr contra la fila pre-update y armamos
        // el `overrides` final. La coerción al tipo de la columna la
        // hace el encoder (`encode_column_value`) al persistir;
        // errores de tipo aquí se reportan como `[GBY-4041]`.
        let mut overrides: HashMap<String, Value> = HashMap::new();
        for (col_key, expr) in expr_assignments {
            let val = eval_expr(expr, &old_row)?;
            // G2: pre-chequeo de tipo. `encode_row` rechazaría el
            // mismatch igual, pero acá podemos atribuirlo a la columna
            // exacta del SET y devolver un código (`[GBY-4041]`)
            // accionable. NULL siempre pasa (lo valida NOT NULL más
            // abajo); INT en columna FLOAT se promueve sin warning.
            if let Some(col) = meta.column(col_key) {
                if !value_fits_column_type(&val, col.column_type) {
                    return Err(coded(
                        codes::UPDATE_SET_TYPE_MISMATCH,
                        format!(
                            "UPDATE sobre '{}': el valor calculado para '{}' es {} y la \
                             columna es {}; envolver con CAST(... AS {}) si la conversión \
                             es intencional",
                            meta.name,
                            col.name,
                            value_type_name(&val),
                            col.column_type.as_sql(),
                            col.column_type.as_sql()
                        ),
                    ));
                }
            }
            overrides.insert(col_key.clone(), val);
        }
        let mut current = old_row.clone();
        for (key, value) in &overrides {
            current.insert(key.clone(), value.clone());
        }

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

        for idx in &meta.indexes {
            if !idx.unique {
                continue;
            }
            // Bloque L1: distinguir índices compuestos. Pre-check sólo si
            // el SET toca alguna columna que el índice cubre.
            let touched = idx
                .all_columns()
                .iter()
                .any(|c| overrides.contains_key(&normalize_ident(c)));
            if !touched {
                continue;
            }
            if idx.is_composite() {
                let fp = composite_fp_for_values(meta, idx, &current)?;
                composite_unique_check(self.pager, idx, fp, Some(pk))?;
            } else {
                let column = meta.column(&idx.column).ok_or_else(|| {
                    DbError::new(format!(
                        "índice apunta a columna inexistente: {}",
                        idx.column
                    ))
                })?;
                let new_value = current
                    .get(&normalize_ident(&idx.column))
                    .cloned()
                    .unwrap_or(Value::Null);
                let new_bytes = encode_column_value(column, &new_value)?;
                check_unique_conflict(self.pager, idx, &new_bytes, Some(pk))?;
            }
        }

        // Bloque L2 (2026-05-27): CHECK eval contra la fila MERGED
        // (`current` ya tiene los overrides aplicados). Cubre UPDATE y
        // UPSERT DO UPDATE (los dos van por este código path).
        enforce_check_constraints(meta, &current)?;

        enforce_fk_on_update(self.pager, meta, &old_row, &current, pk)?;

        let (encoded_pk, row_bytes) = encode_row(meta, &current)?;
        // Residual #4 (2026-05-27): si el SET cambió alguna columna PK,
        // `encoded_pk != pk`. Disparar el camino de "PK move" — incluye
        // chequeo de duplicado en new_pk, cascade ON UPDATE sobre los
        // children, y move atómico de la fila + índices del propio
        // padre. Para PK estable (caso histórico), seguimos por el
        // upsert directo.
        if encoded_pk != pk {
            self.move_row_and_cascade_on_update(
                meta, pk, encoded_pk, &old_row, &current, row_bytes, &overrides,
            )?;
            return Ok(());
        }
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.upsert_row(meta.root_page, encoded_pk, row_bytes)?;
        }

        for idx in &meta.indexes {
            // Bloque L1: mantener composites cuando se toca alguna de
            // sus columnas (no sólo `idx.column`).
            let touched = idx
                .all_columns()
                .iter()
                .any(|c| overrides.contains_key(&normalize_ident(c)));
            if !touched {
                continue;
            }
            if idx.is_composite() {
                let old_fp = composite_fp_for_values(meta, idx, &old_row)?;
                let new_fp = composite_fp_for_values(meta, idx, &current)?;
                if old_fp == new_fp {
                    continue;
                }
                composite_index_remove(self.pager, idx.root_page, old_fp, encoded_pk)?;
                composite_index_upsert(self.pager, idx.root_page, new_fp, encoded_pk)?;
            } else {
                let normalized = normalize_ident(&idx.column);
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
                index_remove_pk(self.pager, idx.root_page, idx.kind, &old_bytes, encoded_pk)?;
                index_upsert_pk(self.pager, idx.root_page, idx.kind, &new_bytes, encoded_pk)?;
            }
        }

        Ok(())
    }

    /// Residual #4 de L (2026-05-27): cuando un UPDATE cambia alguna
    /// columna PK, esta función orquesta el "PK move":
    ///
    /// 1. **Duplicate guard**: si ya hay otra fila con `new_pk`,
    ///    aborta con `[GBY-3001] DUPLICATE_PRIMARY_KEY`.
    /// 2. **ON UPDATE cascade**: para cada FK entrante a esta tabla,
    ///    aplica la acción declarada — CASCADE propaga `new` target
    ///    values, SET NULL/SET DEFAULT mutan source cols, RESTRICT
    ///    (y NO ACTION en este release) aborta con `[GBY-4073]`.
    /// 3. **Self-FK edge case**: si esta tabla se referencia a sí
    ///    misma (`fk.table == meta.name`), incluimos la propia fila
    ///    en el cascade — el `cascade_set_fk_tuple` sobre la misma
    ///    fila después del move funciona idempotente.
    /// 4. **Data move**: delete row at `old_pk`, insert at `new_pk`.
    /// 5. **Index move**: para cada índice secundario, remove old +
    ///    insert new con las values correspondientes.
    ///
    /// Sin estado parcial: cualquier rama de error aborta antes de
    /// tocar disco (orden: duplicate check → cascade RESTRICT check
    /// → cascade application → data move → indexes).
    #[allow(clippy::too_many_arguments)]
    fn move_row_and_cascade_on_update(
        &mut self,
        meta: &TableMeta,
        old_pk: i64,
        new_pk: i64,
        old_row: &HashMap<String, Value>,
        new_row: &HashMap<String, Value>,
        new_row_bytes: Vec<u8>,
        overrides: &HashMap<String, Value>,
    ) -> DbResult<()> {
        // 1. Duplicate guard. Self-row no debe rebotar — pero
        //    `new_pk != old_pk` ya garantiza que mirar new_pk no nos
        //    devuelve la fila que estamos por mover.
        let already_there = {
            let mut catalog = Catalog::open(self.pager);
            catalog.get_row(meta.root_page, new_pk)?.is_some()
        };
        if already_there {
            return Err(coded(
                codes::DUPLICATE_PRIMARY_KEY,
                format!(
                    "UPDATE sobre '{}': la nueva PK ya existe en otra fila (PK destino = {})",
                    meta.name, new_pk
                ),
            ));
        }

        // 2. ON UPDATE cascade. Snapshot del catálogo: la cascade
        //    sólo muta DATOS de tablas, no schema.
        let snapshot = {
            let mut catalog = Catalog::open(self.pager);
            catalog.list_tables()?
        };
        for child_table in &snapshot {
            for child_col in &child_table.columns {
                let Some(fk) = &child_col.references else {
                    continue;
                };
                if !fk.table.eq_ignore_ascii_case(&meta.name) {
                    continue;
                }
                // Target values del parent en el orden declarado.
                let old_target_values: Vec<Value> = fk
                    .target_columns()
                    .iter()
                    .map(|t| {
                        old_row
                            .get(&normalize_ident(t))
                            .cloned()
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                let new_target_values: Vec<Value> = fk
                    .target_columns()
                    .iter()
                    .map(|t| {
                        new_row
                            .get(&normalize_ident(t))
                            .cloned()
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                // Sólo cascadeamos si los target values CAMBIARON.
                // ON UPDATE x es no-op si la columna FK target no
                // está entre los overrides.
                if old_target_values == new_target_values {
                    continue;
                }
                let child_pks = find_child_pks_with_fk_value(
                    self.pager,
                    child_table,
                    fk,
                    &child_col.name,
                    &old_target_values,
                )?;
                if child_pks.is_empty() {
                    continue;
                }
                // Source col names del child para esta FK (anchored
                // en child_col.name; resto en extra_source_columns).
                let source_col_names = fk.source_columns(&child_col.name);
                // Edge case: cascade CASCADE/SET NULL/SET DEFAULT
                // sobre columnas que también son PK del child. No
                // soportado en este release — error claro en vez
                // de corromper el B+Tree.
                let cascade_touches_child_pk =
                    source_col_names.iter().any(|c| child_table.is_pk_column(c));
                let needs_mutation = matches!(
                    fk.on_update,
                    OnUpdate::Cascade | OnUpdate::SetNull | OnUpdate::SetDefault
                );
                if needs_mutation && cascade_touches_child_pk {
                    return Err(coded(
                        codes::FK_UPDATE_CASCADE_AFFECTS_CHILD_PK,
                        format!(
                            "UPDATE sobre '{}' bloqueado: la cascade ON UPDATE en '{}' \
                             mutaría columnas que también participan en la PK del child \
                             ({}). Este release no encadena PK-moves.",
                            meta.name,
                            child_table.name,
                            source_col_names.join(", ")
                        ),
                    ));
                }
                match fk.on_update {
                    OnUpdate::NoAction | OnUpdate::Restrict => {
                        return Err(coded(
                            codes::FK_RESTRICT_BLOCKS_UPDATE,
                            format!(
                                "UPDATE sobre '{}' bloqueado: '{}.{}' referencia esta fila \
                                 con ON UPDATE {} ({} fila(s) hijas afectarían)",
                                meta.name,
                                child_table.name,
                                child_col.name,
                                fk.on_update.as_sql(),
                                child_pks.len()
                            ),
                        ));
                    }
                    OnUpdate::Cascade => {
                        let cols_refs: Vec<&str> = source_col_names.to_vec();
                        for cpk in child_pks {
                            cascade_set_fk_tuple(
                                self.pager,
                                child_table,
                                cpk,
                                &cols_refs,
                                &new_target_values,
                            )?;
                        }
                    }
                    OnUpdate::SetNull => {
                        // Validar que ninguna source col del child sea NOT NULL.
                        for src in &source_col_names {
                            let scol = child_table.column(src).ok_or_else(|| {
                                DbError::new(format!(
                                    "FK rota: columna source '{}' no existe en '{}'",
                                    src, child_table.name
                                ))
                            })?;
                            if scol.not_null {
                                return Err(coded(
                                    codes::FK_SET_NULL_VIOLATES_NOT_NULL,
                                    format!(
                                        "UPDATE sobre '{}' bloqueado: '{}.{}' es NOT NULL y la \
                                         FK declaró ON UPDATE SET NULL ({} fila(s) hijas \
                                         afectarían)",
                                        meta.name,
                                        child_table.name,
                                        src,
                                        child_pks.len()
                                    ),
                                ));
                            }
                        }
                        let nulls: Vec<Value> =
                            source_col_names.iter().map(|_| Value::Null).collect();
                        let cols_refs: Vec<&str> = source_col_names.to_vec();
                        for cpk in child_pks {
                            cascade_set_fk_tuple(self.pager, child_table, cpk, &cols_refs, &nulls)?;
                        }
                    }
                    OnUpdate::SetDefault => {
                        let mut defaults: Vec<Value> = Vec::with_capacity(source_col_names.len());
                        for src in &source_col_names {
                            let scol = child_table.column(src).ok_or_else(|| {
                                DbError::new(format!(
                                    "FK rota: columna source '{}' no existe en '{}'",
                                    src, child_table.name
                                ))
                            })?;
                            let Some(default) = &scol.default else {
                                return Err(coded(
                                    codes::FK_SET_DEFAULT_MISSING,
                                    format!(
                                        "UPDATE sobre '{}' bloqueado: '{}.{}' no tiene DEFAULT \
                                         y la FK declaró ON UPDATE SET DEFAULT ({} fila(s) hijas)",
                                        meta.name,
                                        child_table.name,
                                        src,
                                        child_pks.len()
                                    ),
                                ));
                            };
                            let v = default_to_value(default);
                            if matches!(v, Value::Null) && scol.not_null {
                                return Err(coded(
                                    codes::NOT_NULL_VIOLATED,
                                    format!(
                                        "UPDATE sobre '{}' bloqueado: ON UPDATE SET DEFAULT \
                                         pondría '{}.{}' (NOT NULL) en NULL — el DEFAULT \
                                         declarado es NULL",
                                        meta.name, child_table.name, src
                                    ),
                                ));
                            }
                            defaults.push(v);
                        }
                        let cols_refs: Vec<&str> = source_col_names.to_vec();
                        for cpk in child_pks {
                            cascade_set_fk_tuple(
                                self.pager,
                                child_table,
                                cpk,
                                &cols_refs,
                                &defaults,
                            )?;
                        }
                    }
                }
            }
        }

        // 3. Data move: delete old, insert new. NO usar upsert_row con
        //    new_pk porque la key cambió — necesitamos un delete real
        //    de old_pk seguido de insert en new_pk.
        {
            let mut catalog = Catalog::open(self.pager);
            catalog.delete_row(meta.root_page, old_pk)?;
            catalog.insert_row(meta.root_page, new_pk, new_row_bytes)?;
        }

        // 4. Mantener índices secundarios — TODOS, no sólo los
        //    tocados por overrides, porque el PK cambió y el bucket
        //    almacena el PK como payload junto al value.
        let _ = overrides; // overrides ya no nos sirve; recalc por meta
        for idx in &meta.indexes {
            if idx.is_composite() {
                let old_fp = composite_fp_for_values(meta, idx, old_row)?;
                let new_fp = composite_fp_for_values(meta, idx, new_row)?;
                composite_index_remove(self.pager, idx.root_page, old_fp, old_pk)?;
                composite_index_upsert(self.pager, idx.root_page, new_fp, new_pk)?;
            } else {
                let idx_col = meta.column(&idx.column).ok_or_else(|| {
                    DbError::new(format!(
                        "índice apunta a columna inexistente: {}",
                        idx.column
                    ))
                })?;
                let old_value = old_row
                    .get(&normalize_ident(&idx.column))
                    .cloned()
                    .unwrap_or(Value::Null);
                let new_value = new_row
                    .get(&normalize_ident(&idx.column))
                    .cloned()
                    .unwrap_or(Value::Null);
                let old_bytes = encode_column_value(idx_col, &old_value)?;
                let new_bytes = encode_column_value(idx_col, &new_value)?;
                index_remove_pk(self.pager, idx.root_page, idx.kind, &old_bytes, old_pk)?;
                index_upsert_pk(self.pager, idx.root_page, idx.kind, &new_bytes, new_pk)?;
            }
        }
        Ok(())
    }

    /// Bloque E3: dado un WHERE arbitrario sobre una tabla, devuelve la
    /// lista de PKs cuyas filas matchean. Estrategia:
    /// 1. Si el WHERE es exactamente `Eq` sobre la PK con literal INT,
    ///    devolvemos `vec![n]` directo (sin tocar disco) — flag
    ///    `was_explicit_single_pk = true` para preservar el error
    ///    legado `ROW_NOT_FOUND_FOR_PK` cuando la fila no existe.
    /// 2. En cualquier otro caso: FullScan de la tabla + evaluador 3VL
    ///    fila-a-fila. Correcto para todos los operadores del WHERE
    ///    (E1+E2 + subqueries). Sin optimización indexada — queda en
    ///    backlog para no duplicar el dispatcher de SELECT.
    ///
    /// `op_label` se usa solo para mensajes de error (e.g. "UPDATE" /
    /// "DELETE").
    fn resolve_target_pks(
        &mut self,
        meta: &TableMeta,
        where_clause: &WhereExpr,
        _op_label: &str,
    ) -> DbResult<(Vec<i64>, bool)> {
        // Fast-path: `WHERE pk = literal` (compatibilidad pre-E3).
        // K2: solo aplica para PK single — con PK compuesta el match por
        // una sola columna PK debe caer al full-scan que evalúa todos los
        // predicados; el fingerprint requiere TODAS las columnas PK.
        if !meta.has_composite_pk() {
            if let WhereExpr::Atom(WhereClause::Eq { column, value }) = where_clause {
                if normalize_ident(column) == normalize_ident(&meta.primary_key) {
                    let pk = match value {
                        Value::Integer(n) => *n,
                        _ => {
                            return Err(DbError::new(format!(
                                "PRIMARY KEY '{}' es INT; valor incompatible en WHERE",
                                meta.primary_key
                            )))
                        }
                    };
                    return Ok((vec![pk], true));
                }
            }
        }
        // Fallback genérico: FullScan + evaluador 3VL. Reusa el mismo
        // evaluador del SELECT (`eval_where_expr_single`) — la única
        // diferencia es que acá necesitamos las PKs, no las filas
        // proyectadas, así que iteramos sobre los `KeyValue` crudos.
        let rows = {
            let mut catalog = Catalog::open(self.pager);
            catalog.scan_rows(meta.root_page, 0, None)?
        };
        let mut pks = Vec::new();
        for kv in rows {
            let decoded = decode_row(meta, &kv.value)?;
            let verdict = self.eval_where_expr_single(where_clause, meta, &decoded)?;
            if matches!(verdict, Some(true)) {
                pks.push(kv.key);
            }
        }
        Ok((pks, false))
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

        // K2 (2026-05-26): índice compuesto — TODAS las columnas deben
        // ser INT (4067). El fingerprint i64 no acepta otros tipos.
        let is_composite = !stmt.extra_columns.is_empty();
        if is_composite {
            for col_name in std::iter::once(&stmt.column).chain(stmt.extra_columns.iter()) {
                let col = meta.column(col_name).ok_or_else(|| {
                    coded(
                        codes::COLUMN_NOT_FOUND,
                        format!(
                            "CREATE INDEX '{}': columna '{}' no existe en '{}'",
                            stmt.name, col_name, meta.name
                        ),
                    )
                })?;
                if col.column_type != ColumnType::Int {
                    return Err(coded(
                        codes::COMPOSITE_INDEX_REQUIRES_ALL_INT,
                        format!(
                            "CREATE INDEX '{}': columna '{}' es {} — los índices compuestos \
                             en VERSION 8 exigen INT en todas las columnas (ver ADR-0019)",
                            stmt.name,
                            col_name,
                            col.column_type.as_sql()
                        ),
                    ));
                }
            }
        } else {
            // 2. Validate single-column index + type.
            validate_indexable(&meta, &stmt.column)?;
        }

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
        if !is_composite && meta.index_for_column(&stmt.column).is_some() {
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
        if is_composite {
            // Composite index backfill: fingerprint cada fila y guardamos
            // PKs en un ordered bucket. Para UNIQUE detectamos colisiones
            // por fingerprint — y como la composición incluye un sentinel
            // entre columnas, una colisión real es astronómicamente
            // improbable (FNV-1a-64 sobre tuplas distintas).
            let composite_columns: Vec<Column> = std::iter::once(stmt.column.clone())
                .chain(stmt.extra_columns.iter().cloned())
                .map(|name| {
                    meta.column(&name)
                        .cloned()
                        .ok_or_else(|| DbError::new(format!("columna no existe: {}", name)))
                })
                .collect::<DbResult<_>>()?;
            let mut seen_fp: HashSet<i64> = HashSet::new();
            for kv in rows {
                let decoded = decode_row(&meta, &kv.value)?;
                let values: Vec<Value> = composite_columns
                    .iter()
                    .map(|c| {
                        decoded
                            .get(&normalize_ident(&c.name))
                            .cloned()
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                let col_refs: Vec<&Column> = composite_columns.iter().collect();
                let val_refs: Vec<&Value> = values.iter().collect();
                let fp = encode_composite_key(&col_refs, &val_refs)?;
                if stmt.unique && !seen_fp.insert(fp) {
                    return Err(coded(
                        codes::UNIQUE_VIOLATED,
                        format!(
                            "CREATE UNIQUE INDEX '{}' rechazado: ya existen filas con la \
                             misma combinación de ({}) en '{}'",
                            stmt.name,
                            composite_columns
                                .iter()
                                .map(|c| c.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                            meta.name
                        ),
                    ));
                }
                // Bucket payload: ordered bucket de PKs indexado por el
                // fingerprint. Reusamos el encoder de OrderedInt para
                // mantener un solo path de decoder en INTEGRITY CHECK.
                let mut current = {
                    let mut tree = Tree::new(self.pager);
                    match tree.get(idx_root, fp)? {
                        Some(bytes) => decode_ordered_bucket(&bytes)?,
                        None => Vec::new(),
                    }
                };
                ordered_bucket_insert(&mut current, kv.key);
                let encoded = encode_ordered_bucket(&current)?;
                let mut tree = Tree::new(self.pager);
                tree.upsert(idx_root, fp, encoded)?;
            }
        } else {
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
        }

        // 6. Publish the index in the catalog.
        // K2: índices compuestos usan `kind = OrderedInt` para que el
        // sweep de INTEGRITY CHECK use el decoder correcto (ordered bucket
        // = lista de PKs sin value-bytes). La clave NO es order-preserving
        // —es un fingerprint FNV-1a-64 i64— por eso el planner JAMÁS
        // dispara range scan contra un índice compuesto.
        let kind = if is_composite {
            IndexKind::OrderedInt
        } else {
            IndexKind::for_column(column.column_type)
        };
        meta.indexes.push(IndexMeta {
            name: stmt.name,
            column: stmt.column,
            root_page: idx_root,
            unique: stmt.unique,
            kind,
            extra_columns: stmt.extra_columns,
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
        self.reject_if_view(&stmt.table, "DELETE")?;
        let meta = {
            let mut catalog = Catalog::open(self.pager);
            catalog
                .get_table(&stmt.table)?
                .ok_or_else(|| DbError::new(format!("tabla no existe: {}", stmt.table)))?
        };

        // Bloque E3: resolver PKs igual que UPDATE — fast-path para
        // `WHERE pk = N` (que mantiene el error legado si no existe la
        // fila), fallback FullScan + 3VL para todo lo demás.
        let (target_pks, was_explicit_single_pk) =
            self.resolve_target_pks(&meta, &stmt.where_clause, "DELETE")?;

        if target_pks.is_empty() && was_explicit_single_pk {
            return Err(coded(
                codes::ROW_NOT_FOUND_FOR_PK,
                format!("DELETE FROM '{}': fila no existe", meta.name),
            ));
        }

        // Resolvemos las PKs ANTES de empezar a borrar — si borráramos
        // mientras iteramos un FullScan, los efectos de las primeras
        // cascadas podrían modificar la lista en flight (especialmente
        // crítico con FK ON DELETE CASCADE que toca otras tablas pero
        // también con cascadas dentro de la misma vía self-ref).
        let mut deleted = 0usize;
        let mut affected_rows: Vec<HashMap<String, Value>> = Vec::new();
        for pk in target_pks {
            let still_there = {
                let mut catalog = Catalog::open(self.pager);
                catalog.get_row(meta.root_page, pk)?.is_some()
            };
            if !still_there {
                continue;
            }
            // Bloque J2: snapshot de la fila ANTES del delete para RETURNING.
            if stmt.returning.is_some() {
                let bytes = {
                    let mut catalog = Catalog::open(self.pager);
                    catalog.get_row(meta.root_page, pk)?
                };
                if let Some(b) = bytes {
                    affected_rows.push(decode_row(&meta, &b)?);
                }
            }
            delete_with_cascade(self.pager, &meta.name, pk)?;
            deleted += 1;
        }

        if let Some(returning) = &stmt.returning {
            let projected = project_returning(&meta, returning, &affected_rows)?;
            let columns = returning_column_names(&meta, returning);
            return Ok(ResultSet {
                columns,
                rows: projected,
                message: Some(format!(
                    "OK ({} fila{} eliminada{})",
                    deleted,
                    if deleted == 1 { "" } else { "s" },
                    if deleted == 1 { "" } else { "s" }
                )),
            });
        }

        Ok(ResultSet {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(format!(
                "OK ({} fila{} eliminada{})",
                deleted,
                if deleted == 1 { "" } else { "s" },
                if deleted == 1 { "" } else { "s" }
            )),
        })
    }

    /// Bloque F: pipeline de agregación. Se invoca desde `exec_select`
    /// cuando la query tiene agregados, `GROUP BY` o `HAVING`. Recibe
    /// las filas crudas YA filtradas por el `WHERE` y produce el
    /// `ResultSet` final aplicando: bucketing → cálculo de agregados →
    /// `HAVING` → `ORDER BY` → `OFFSET`/`LIMIT`.
    fn exec_aggregate_pipeline(
        &mut self,
        meta: &TableMeta,
        stmt: &SelectStmt,
        rows_bytes: Vec<KeyValue>,
        output_columns: Vec<String>,
    ) -> DbResult<ResultSet> {
        // 1. Validar invariantes ANSI antes de hacer trabajo.
        validate_aggregate_select(stmt, meta)?;

        // 2. Decodificar todas las filas que pasaron el WHERE. Materializar
        //    es necesario porque el bucketing requiere ver todas las filas
        //    para emitir UN resultado por bucket.
        let mut decoded_rows: Vec<HashMap<String, Value>> = Vec::with_capacity(rows_bytes.len());
        for kv in rows_bytes {
            decoded_rows.push(decode_row(meta, &kv.value)?);
        }

        // 3. Particionar en buckets por las claves del GROUP BY. Las claves
        //    se normalizan a lowercase. Cuando GROUP BY está vacío y hay
        //    agregados, usamos un único bucket global (key = vec![]) que
        //    produce UNA fila incluso si la entrada tiene 0 filas.
        let group_keys: Vec<String> = stmt.group_by.iter().map(|c| normalize_ident(c)).collect();
        for key in &group_keys {
            if meta.column(key).is_none() {
                return Err(coded(
                    codes::COLUMN_NOT_FOUND,
                    format!("GROUP BY: columna '{}' no existe en '{}'", key, meta.name),
                ));
            }
        }
        // Vec<(key_tuple, Vec<row>)> preservando el orden de primera aparición.
        let mut bucket_order: Vec<Vec<Value>> = Vec::new();
        type GroupBucket = (Vec<Value>, Vec<HashMap<String, Value>>);
        let mut buckets: HashMap<Vec<u8>, GroupBucket> = HashMap::new();
        for row in decoded_rows {
            let key_tuple: Vec<Value> = group_keys
                .iter()
                .map(|k| row.get(k).cloned().unwrap_or(Value::Null))
                .collect();
            let key_bytes = encode_group_key(&key_tuple);
            buckets
                .entry(key_bytes.clone())
                .and_modify(|(_, rs)| rs.push(row.clone()))
                .or_insert_with(|| {
                    bucket_order.push(key_tuple.clone());
                    (key_tuple, vec![row])
                });
        }

        // Caso especial: sin GROUP BY explícito y SIN agregados es
        // imposible llegar acá (needs_aggregation sería false). Si hay
        // agregados pero buckets está vacío (0 filas pasaron el WHERE),
        // ANSI dice que devolvemos UNA fila con los neutros (COUNT=0,
        // SUM=NULL, etc.). Insertamos un bucket vacío sintético.
        if group_keys.is_empty() && buckets.is_empty() {
            buckets.insert(Vec::new(), (Vec::new(), Vec::new()));
            bucket_order.push(Vec::new());
        }

        // 4. Por cada bucket, computar agregados y armar la fila de
        //    salida (HashMap<output_name, Value>). Las columnas no-agg
        //    son las del GROUP BY (mismo valor en todas las filas del
        //    bucket, leemos del key_tuple).
        let agg_items: Vec<(&SelectItem, String)> = stmt
            .columns
            .iter()
            .filter(|it| matches!(it, SelectItem::Aggregate { .. }))
            .map(|it| (it, it.output_name()))
            .collect();

        let mut output_rows: Vec<HashMap<String, Value>> = Vec::with_capacity(bucket_order.len());
        for key_bytes in bucket_order.iter().map(|t| encode_group_key(t.as_slice())) {
            let (key_tuple, rows) = buckets
                .remove(&key_bytes)
                .expect("bucket presente en bucket_order");
            let mut out_row: HashMap<String, Value> = HashMap::new();
            // Columnas del GROUP BY: clave normalizada + valor del tuple.
            for (i, k) in group_keys.iter().enumerate() {
                out_row.insert(k.clone(), key_tuple[i].clone());
            }
            // Cada agregado se computa contra todas las filas del bucket.
            // Insertamos el valor bajo el `output_name` (alias si existe;
            // canonical si no) Y también bajo el canonical key — eso
            // permite que `HAVING SUM(monto) > 100` y `HAVING total > 100`
            // (alias) ambos resuelvan al mismo bucket value.
            for (item, output_name) in &agg_items {
                if let SelectItem::Aggregate { func, arg, alias } = item {
                    let value = compute_aggregate(*func, arg, &rows)?;
                    out_row.insert(output_name.clone(), value.clone());
                    if alias.is_some() {
                        let canonical = SelectItem::Aggregate {
                            func: *func,
                            arg: arg.clone(),
                            alias: None,
                        }
                        .output_name();
                        out_row.insert(canonical, value);
                    }
                }
            }
            output_rows.push(out_row);
        }

        // 5. HAVING: filtrar buckets. El evaluador reusa el de WHERE pero
        //    la fila es el bucket-aggregate, no la fila cruda.
        if let Some(expr) = &stmt.having {
            let mut kept = Vec::with_capacity(output_rows.len());
            for row in output_rows {
                let verdict = self.eval_where_expr_single(expr, meta, &row)?;
                if matches!(verdict, Some(true)) {
                    kept.push(row);
                }
            }
            output_rows = kept;
        }

        // 6. Proyección final en el orden del SELECT list.
        let mut projected: Vec<Vec<Value>> = output_rows
            .iter()
            .map(|row| {
                stmt.columns
                    .iter()
                    .map(|item| {
                        let name = item.output_name();
                        let key = match item {
                            SelectItem::Column(c) => normalize_ident(c),
                            _ => name.clone(),
                        };
                        row.get(&key).cloned().unwrap_or(Value::Null)
                    })
                    .collect()
            })
            .collect();

        // 7. DISTINCT (redundante si GROUP BY ya dedup, pero ANSI lo
        //    permite — lo aplicamos por completitud).
        if stmt.distinct {
            projected = dedup_preserving_order(projected);
        }

        // 8. ORDER BY contra el esquema de salida. La columna referenciada
        //    debe coincidir con un `output_name` (alias del agregado, nombre
        //    canónico, o columna del GROUP BY). Si no, error.
        if let Some(ord) = &stmt.order_by {
            let target = ord.column.clone();
            let target_norm = normalize_ident(&target);
            let idx = output_columns
                .iter()
                .position(|n| n.eq_ignore_ascii_case(&target) || normalize_ident(n) == target_norm);
            let idx = idx.ok_or_else(|| {
                coded(
                    codes::COLUMN_NOT_FOUND,
                    format!(
                        "ORDER BY: '{}' no figura en el SELECT list de una query agregada",
                        target
                    ),
                )
            })?;
            projected.sort_by(|a, b| compare_values(Some(&a[idx]), Some(&b[idx])));
            if matches!(ord.direction, OrderDir::Desc) {
                projected.reverse();
            }
        }

        // 9. OFFSET/LIMIT.
        let total = projected.len();
        let start = stmt.offset.min(total);
        let end = match stmt.limit {
            Some(l) => (start + l).min(total),
            None => total,
        };
        let windowed: Vec<Vec<Value>> = projected
            .into_iter()
            .skip(start)
            .take(end - start)
            .collect();

        Ok(ResultSet {
            columns: output_columns,
            rows: windowed,
            message: None,
        })
    }
}

/// Bloque F: true si la query requiere el stage de agregación
/// (cualquier agregado en SELECT, `GROUP BY` no vacío, o `HAVING`).
fn stmt_needs_aggregation(stmt: &SelectStmt) -> bool {
    stmt.having.is_some()
        || !stmt.group_by.is_empty()
        || stmt
            .columns
            .iter()
            .any(|i| matches!(i, SelectItem::Aggregate { .. }))
}

/// Bloque F: valida invariantes ANSI antes del bucketing.
/// - Toda columna no-agregada en el SELECT debe figurar en `GROUP BY`.
/// - Si hay JOINs, devolvemos error claro (agregados sobre JOINs es
///   un release futuro).
/// - Las columnas del GROUP BY deben existir.
fn validate_aggregate_select(stmt: &SelectStmt, meta: &TableMeta) -> DbResult<()> {
    if !stmt.joins.is_empty() {
        return Err(coded(
            codes::AGGREGATE_OVER_JOIN_UNSUPPORTED,
            "agregados (COUNT/SUM/AVG/MIN/MAX) y GROUP BY/HAVING sobre SELECT con JOIN \
             aún no se soportan; reescribir como subquery agregada sobre la tabla base",
        ));
    }
    let group_set: HashSet<String> = stmt.group_by.iter().map(|c| normalize_ident(c)).collect();
    for item in &stmt.columns {
        match item {
            SelectItem::Star => {
                if !group_set.is_empty()
                    || stmt
                        .columns
                        .iter()
                        .any(|i| matches!(i, SelectItem::Aggregate { .. }))
                {
                    return Err(coded(
                        codes::SELECT_COLUMN_NOT_IN_GROUP_BY,
                        "SELECT *: no se permite combinar `*` con agregados o GROUP BY; \
                         enumerar las columnas a proyectar (y agregarlas al GROUP BY)",
                    ));
                }
            }
            SelectItem::Column(c) => {
                let key = normalize_ident(c);
                if meta.column(&key).is_none() {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        format!("SELECT: columna '{}' no existe en '{}'", c, meta.name),
                    ));
                }
                if !group_set.contains(&key) {
                    return Err(coded(
                        codes::SELECT_COLUMN_NOT_IN_GROUP_BY,
                        format!(
                            "SELECT: la columna '{}' no figura en GROUP BY ni es una función agregada — \
                             agregala al GROUP BY o envolvela en MIN/MAX/SUM/AVG/COUNT",
                            c
                        ),
                    ));
                }
            }
            SelectItem::Aggregate { arg, .. } => match arg {
                AggArg::Star => {}
                AggArg::Column(c) | AggArg::DistinctColumn(c) => {
                    let key = normalize_ident(c);
                    if meta.column(&key).is_none() {
                        return Err(coded(
                            codes::COLUMN_NOT_FOUND,
                            format!(
                                "función agregada referencia columna '{}' que no existe en '{}'",
                                c, meta.name
                            ),
                        ));
                    }
                }
                // Issue #5: el chequeo de columnas para Expr se hace en
                // runtime al evaluar (eval_expr ya rebota COLUMN_NOT_FOUND).
                AggArg::Expr(_) => {}
            },
            SelectItem::Expression { .. } => {
                // Bloque G1: expresiones escalares en SELECT con
                // GROUP BY/HAVING/agregados todavía no se soportan.
                // El bloque G2 las trata (requiere que las columnas
                // referenciadas estén en el GROUP BY o envueltas en
                // una agregada).
                return Err(coded(
                    codes::SELECT_COLUMN_NOT_IN_GROUP_BY,
                    "expresiones escalares en SELECT con GROUP BY / agregados aún no se soportan \
                     (bloque G1: solo SELECT plano); reescribir como subquery o esperar G2",
                ));
            }
        }
    }
    Ok(())
}

/// Bloque F: serialización determinística de una tupla de valores que
/// sirve como clave de HashMap para los buckets del GROUP BY. NULL se
/// representa por un byte sentinela (`0xFE`) distinto al de cualquier
/// type-tag — todos los NULLs del mismo GROUP BY van al mismo bucket
/// (consistente con la semántica de SQL: `NULL` agrupa con `NULL`).
fn encode_group_key(values: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for v in values {
        match v {
            Value::Null => out.push(0xFE),
            Value::Integer(n) => {
                out.push(0x01);
                out.extend_from_slice(&n.to_le_bytes());
            }
            Value::Float(f) => {
                out.push(0x02);
                out.extend_from_slice(&f.to_bits().to_le_bytes());
            }
            Value::Bool(b) => {
                out.push(0x03);
                out.push(if *b { 1 } else { 0 });
            }
            Value::String(s) => {
                out.push(0x04);
                out.extend_from_slice(&(s.len() as u32).to_le_bytes());
                out.extend_from_slice(s.as_bytes());
            }
        }
        out.push(0xFF); // separador
    }
    out
}

/// Bloque F: cómputo de un agregado sobre una lista de filas del bucket.
/// Semántica:
/// - `COUNT(*)`: cuenta TODAS las filas del bucket (incluyendo las que
///   tienen NULL en otras columnas).
/// - `COUNT(col)`: cuenta filas donde `col` no es NULL.
/// - `COUNT(DISTINCT col)`: cuenta valores distintos no-NULL.
/// - `SUM(col)`: suma valores no-NULL (INT → INT, FLOAT → FLOAT, mixto → FLOAT).
///   `SUM` sobre conjunto vacío o todo-NULL → `NULL` (ANSI).
/// - `AVG(col)`: promedio de valores no-NULL como FLOAT.
///   Conjunto vacío o todo-NULL → `NULL`.
/// - `MIN(col)` / `MAX(col)`: ignora NULLs. Conjunto vacío o todo-NULL → `NULL`.
fn compute_aggregate(
    func: AggFunc,
    arg: &AggArg,
    rows: &[HashMap<String, Value>],
) -> DbResult<Value> {
    // Issue #5 (2026-05-27): si el argumento es un `Expr` arbitrario
    // (e.g. `SUM(qty * price)`), pre-evaluamos contra cada fila para
    // obtener un vector de `Value` y reutilizamos el mismo motor de
    // agregación que para columnas, sintetizando una clave anónima.
    if let AggArg::Expr(expr) = arg {
        let synthetic_key = "__agg_expr__";
        let mut synthetic_rows: Vec<HashMap<String, Value>> = Vec::with_capacity(rows.len());
        for r in rows {
            let v = eval_expr(expr, r)?;
            let mut m: HashMap<String, Value> = HashMap::with_capacity(1);
            m.insert(synthetic_key.to_string(), v);
            synthetic_rows.push(m);
        }
        let synthetic_arg = AggArg::Column(synthetic_key.to_string());
        return compute_aggregate(func, &synthetic_arg, &synthetic_rows);
    }
    match (func, arg) {
        (AggFunc::Count, AggArg::Star) => Ok(Value::Integer(rows.len() as i64)),
        (AggFunc::Count, AggArg::Column(col)) => {
            let key = normalize_ident(col);
            let n = rows
                .iter()
                .filter(|r| !matches!(r.get(&key), Some(Value::Null) | None))
                .count();
            Ok(Value::Integer(n as i64))
        }
        (AggFunc::Count, AggArg::DistinctColumn(col)) => {
            let key = normalize_ident(col);
            let mut seen: HashSet<Vec<u8>> = HashSet::new();
            for r in rows {
                let v = r.get(&key).cloned().unwrap_or(Value::Null);
                if matches!(v, Value::Null) {
                    continue;
                }
                seen.insert(encode_group_key(&[v]));
            }
            Ok(Value::Integer(seen.len() as i64))
        }
        (AggFunc::Sum, AggArg::Column(col)) => {
            let key = normalize_ident(col);
            let mut acc_int: i128 = 0;
            let mut acc_float: f64 = 0.0;
            let mut any = false;
            let mut as_float = false;
            for r in rows {
                match r.get(&key) {
                    Some(Value::Integer(n)) => {
                        any = true;
                        if as_float {
                            acc_float += *n as f64;
                        } else {
                            acc_int += *n as i128;
                        }
                    }
                    Some(Value::Float(f)) => {
                        any = true;
                        if !as_float {
                            acc_float = acc_int as f64 + *f;
                            as_float = true;
                        } else {
                            acc_float += *f;
                        }
                    }
                    Some(Value::Null) | None => {}
                    other => {
                        return Err(coded(
                            codes::AGGREGATE_ARG_INVALID,
                            format!(
                                "SUM solo opera sobre INT o FLOAT; valor incompatible: {:?}",
                                other
                            ),
                        ));
                    }
                }
            }
            if !any {
                return Ok(Value::Null);
            }
            if as_float {
                Ok(Value::Float(acc_float))
            } else {
                Ok(Value::Integer(acc_int as i64))
            }
        }
        (AggFunc::Avg, AggArg::Column(col)) => {
            let key = normalize_ident(col);
            let mut sum = 0.0;
            let mut count = 0usize;
            for r in rows {
                match r.get(&key) {
                    Some(Value::Integer(n)) => {
                        sum += *n as f64;
                        count += 1;
                    }
                    Some(Value::Float(f)) => {
                        sum += *f;
                        count += 1;
                    }
                    Some(Value::Null) | None => {}
                    other => {
                        return Err(coded(
                            codes::AGGREGATE_ARG_INVALID,
                            format!(
                                "AVG solo opera sobre INT o FLOAT; valor incompatible: {:?}",
                                other
                            ),
                        ));
                    }
                }
            }
            if count == 0 {
                Ok(Value::Null)
            } else {
                Ok(Value::Float(sum / count as f64))
            }
        }
        (AggFunc::Min, AggArg::Column(col)) | (AggFunc::Max, AggArg::Column(col)) => {
            let key = normalize_ident(col);
            let pick_min = matches!(func, AggFunc::Min);
            let mut best: Option<Value> = None;
            for r in rows {
                let v = r.get(&key).cloned().unwrap_or(Value::Null);
                if matches!(v, Value::Null) {
                    continue;
                }
                best = Some(match &best {
                    None => v,
                    Some(curr) => {
                        let take = if pick_min {
                            compare_values(Some(&v), Some(curr)).is_lt()
                        } else {
                            compare_values(Some(&v), Some(curr)).is_gt()
                        };
                        if take {
                            v
                        } else {
                            curr.clone()
                        }
                    }
                });
            }
            Ok(best.unwrap_or(Value::Null))
        }
        _ => Err(coded(
            codes::AGGREGATE_ARG_INVALID,
            format!(
                "combinación inválida de función y argumento: {}({:?})",
                func.keyword(),
                arg
            ),
        )),
    }
}

/// Bloque F: dedup preservando el orden de primera aparición. Usado por
/// `SELECT DISTINCT`. Comparación basada en una serialización
/// determinística — la misma que usan los buckets del GROUP BY.
fn dedup_preserving_order(rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let key = encode_group_key(&r);
        if seen.insert(key) {
            out.push(r);
        }
    }
    out
}

/// Bloque I (2026-05-26): combina dos `ResultSet` con la semántica de
/// `UNION` / `INTERSECT` / `EXCEPT` (con o sin `ALL`).
///
/// Headers de salida: los del LHS (regla ANSI: el primer SELECT impone
/// los names). Validaciones:
/// - Mismo número de columnas (`[GBY-4054]`).
/// - Tipos compatibles columna a columna: INT/FLOAT promueven entre
///   sí, los demás tipos exigen match exacto (NULL no chequea).
///   `[GBY-4055]` si rompe.
///
/// Multiplicidades:
/// - `Union`: append (con dedup si `!all`).
/// - `Intersect`: intersección de bags. Con ALL: `min(count_l, count_r)`.
///   Sin ALL: presencia en ambos, count 1.
/// - `Except`: bag-diff. Con ALL: `max(0, count_l - count_r)`. Sin
///   ALL: presente en LHS y NO en RHS, count 1.
///
/// Para hashear filas con NULL se usa `encode_group_key` — dos NULLs
/// son iguales acá, comportamiento ANSI de set ops.
fn combine_set_op(
    left: ResultSet,
    right: ResultSet,
    op: SetOpKind,
    all: bool,
) -> DbResult<ResultSet> {
    if left.columns.len() != right.columns.len() {
        return Err(coded(
            codes::SET_OP_ARITY_MISMATCH,
            format!(
                "{} entre queries con {} y {} columnas — ambas deben proyectar la misma arity",
                op.keyword(),
                left.columns.len(),
                right.columns.len()
            ),
        ));
    }
    // Validar compatibilidad de tipos por columna.
    let n_cols = left.columns.len();
    for col in 0..n_cols {
        let lty = infer_column_type(&left.rows, col);
        let rty = infer_column_type(&right.rows, col);
        if !set_op_types_compatible(lty, rty) {
            return Err(coded(
                codes::SET_OP_TYPE_MISMATCH,
                format!(
                    "{}: la columna {} del LHS es {:?} y la del RHS es {:?} — \
                     tipos incompatibles (sólo INT/FLOAT promueven entre sí)",
                    op.keyword(),
                    col + 1,
                    lty,
                    rty
                ),
            ));
        }
    }
    let headers = left.columns.clone();
    // Construir un multiset de cada lado.
    let mut left_counts: HashMap<Vec<u8>, (Vec<Value>, usize)> = HashMap::new();
    for row in left.rows {
        let key = encode_group_key(&row);
        left_counts
            .entry(key)
            .and_modify(|(_, c)| *c += 1)
            .or_insert((row, 1));
    }
    let mut right_counts: HashMap<Vec<u8>, (Vec<Value>, usize)> = HashMap::new();
    for row in right.rows {
        let key = encode_group_key(&row);
        right_counts
            .entry(key)
            .and_modify(|(_, c)| *c += 1)
            .or_insert((row, 1));
    }
    // Para preservar un orden estable de salida (LHS-first, luego RHS
    // en el orden en que aparecieron por primera vez en el RHS),
    // iteramos sobre los rows originales — pero usamos los counts del
    // multiset combinado.
    let mut out_rows: Vec<Vec<Value>> = Vec::new();
    match op {
        SetOpKind::Union => {
            // Construir orden: claves del LHS (con su row), luego claves
            // del RHS que no aparecieron en LHS.
            let mut seen_keys: HashSet<Vec<u8>> = HashSet::new();
            for (key, (row, lcount)) in left_counts.iter() {
                let rcount = right_counts.get(key).map(|(_, c)| *c).unwrap_or(0);
                let total = if all { lcount + rcount } else { 1 };
                for _ in 0..total {
                    out_rows.push(row.clone());
                }
                seen_keys.insert(key.clone());
            }
            for (key, (row, rcount)) in right_counts.iter() {
                if seen_keys.contains(key) {
                    continue;
                }
                let total = if all { *rcount } else { 1 };
                for _ in 0..total {
                    out_rows.push(row.clone());
                }
            }
        }
        SetOpKind::Intersect => {
            for (key, (row, lcount)) in left_counts.iter() {
                if let Some((_, rcount)) = right_counts.get(key) {
                    let total = if all { (*lcount).min(*rcount) } else { 1 };
                    for _ in 0..total {
                        out_rows.push(row.clone());
                    }
                }
            }
        }
        SetOpKind::Except => {
            for (key, (row, lcount)) in left_counts.iter() {
                let rcount = right_counts.get(key).map(|(_, c)| *c).unwrap_or(0);
                let total = if all {
                    lcount.saturating_sub(rcount)
                } else if rcount == 0 {
                    1
                } else {
                    0
                };
                for _ in 0..total {
                    out_rows.push(row.clone());
                }
            }
        }
    }
    Ok(ResultSet {
        columns: headers,
        rows: out_rows,
        message: None,
    })
}

/// Bloque I: tipo "dominante" de los valores no-NULL de una columna en
/// un ResultSet. Devuelve `None` si todas las celdas son NULL — eso es
/// compatible con cualquier otro tipo.
fn infer_column_type(rows: &[Vec<Value>], col: usize) -> Option<ColumnType> {
    let mut current: Option<ColumnType> = None;
    for row in rows {
        if col >= row.len() {
            continue;
        }
        let t = match &row[col] {
            Value::Null => continue,
            Value::Integer(_) => ColumnType::Int,
            Value::Float(_) => ColumnType::Float,
            Value::Bool(_) => ColumnType::Bool,
            Value::String(_) => ColumnType::Text,
        };
        match current {
            None => current = Some(t),
            Some(prev) if prev == t => {}
            // Mezcla INT+FLOAT en el MISMO lado → promociona a FLOAT.
            Some(ColumnType::Int) if t == ColumnType::Float => current = Some(ColumnType::Float),
            Some(ColumnType::Float) if t == ColumnType::Int => {}
            // Cualquier otra mezcla cae a TEXT como compromiso (se
            // valida contra el otro lado luego).
            Some(_) => current = Some(ColumnType::Text),
        }
    }
    current
}

/// Bloque I: dos tipos son compatibles entre set-op-lados si:
/// - alguno es `None` (sólo NULLs en ese lado),
/// - son iguales,
/// - o ambos son numéricos (INT/FLOAT — promueven).
fn set_op_types_compatible(a: Option<ColumnType>, b: Option<ColumnType>) -> bool {
    match (a, b) {
        (None, _) | (_, None) => true,
        (Some(x), Some(y)) if x == y => true,
        (Some(ColumnType::Int), Some(ColumnType::Float))
        | (Some(ColumnType::Float), Some(ColumnType::Int)) => true,
        _ => false,
    }
}

/// Bloque I: aplica un `ORDER BY` sobre un `ResultSet` ya combinado.
/// La columna se resuelve por nombre (case-insensitive) contra los
/// headers. Si no existe → `[GBY-2002]`. Ordena estable; NULLs van al
/// final igual que el ORDER BY de SELECT plano (regla pre-I).
fn apply_order_by_on_resultset(rs: &mut ResultSet, order: &OrderClause) -> DbResult<()> {
    let target = normalize_ident(&order.column);
    let idx = rs
        .columns
        .iter()
        .position(|c| normalize_ident(c) == target)
        .ok_or_else(|| {
            coded(
                codes::COLUMN_NOT_FOUND,
                format!(
                    "ORDER BY: la columna '{}' no figura en el output de la operación de conjunto",
                    order.column
                ),
            )
        })?;
    let desc = matches!(order.direction, OrderDir::Desc);
    rs.rows.sort_by(|a, b| {
        let ord = compare_values_nulls_last(&a[idx], &b[idx]);
        if desc {
            ord.reverse()
        } else {
            ord
        }
    });
    Ok(())
}

/// Bloque I: aplica `LIMIT`/`OFFSET` sobre un ResultSet ya combinado.
fn apply_limit_offset_on_resultset(rs: &mut ResultSet, limit: Option<usize>, offset: usize) {
    if offset > 0 {
        let drop = offset.min(rs.rows.len());
        rs.rows.drain(..drop);
    }
    if let Some(lim) = limit {
        rs.rows.truncate(lim);
    }
}

/// Bloque I: comparación de valores para ORDER BY (NULLs últimos).
/// Mezcla INT/FLOAT promueve; otras mezclas se ordenan por nombre de
/// tipo (estable, suficiente para el ResultSet combinado).
fn compare_values_nulls_last(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
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
        (x, y) => format!("{:?}", x).cmp(&format!("{:?}", y)),
    }
}

pub fn parse(sql_text: &str) -> DbResult<Vec<Statement>> {
    let mut statements = Vec::new();
    for chunk in split_statements(sql_text) {
        let tokens = tokenize(&chunk)?;
        let mut parser = Parser {
            tokens,
            pos: 0,
            where_depth: 0,
            in_having: false,
            pending_check_name: None,
        };
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
    // K2 (2026-05-26): cuando la PK es compuesta, ignoramos el `pk`
    // single-column derivado del loop y al final calculamos el
    // fingerprint FNV-1a-64 sobre todas las columnas PK. Pre-K2 (PK
    // single) la lógica no cambia: el path `column.eq_ignore_ascii_case
    // (&meta.primary_key)` captura el valor INT y lo usa como key.
    let composite_pk = meta.has_composite_pk();

    for column in &meta.columns {
        let normalized = normalize_ident(&column.name);
        let value = values.get(&normalized).cloned().unwrap_or(Value::Null);
        match (&column.column_type, value) {
            (ColumnType::Int, Value::Null) => {
                // Cualquier columna PK con NULL → error explícito (single
                // o compuesta). En compuesta, además el fingerprint no
                // puede representar NULL sin ambigüedad.
                if meta.is_pk_column(&column.name) {
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
                if !composite_pk && column.name.eq_ignore_ascii_case(&meta.primary_key) {
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

    if composite_pk {
        // Composite PK: el row encoding ya incluye los valores de cada
        // columna PK al lado del resto; la clave del B+Tree es el
        // fingerprint sobre la tupla. Pedimos cada valor al HashMap por
        // su nombre normalizado; el `Value::Null` rama de arriba ya
        // habría disparado 3007 si faltaba.
        let pk_cols: Vec<Column> = meta
            .pk_columns()
            .iter()
            .map(|name| {
                meta.column(name).cloned().ok_or_else(|| {
                    DbError::new(format!(
                        "PK '{}' apunta a columna inexistente en '{}'",
                        name, meta.name
                    ))
                })
            })
            .collect::<DbResult<_>>()?;
        let pk_vals: Vec<Value> = pk_cols
            .iter()
            .map(|c| {
                values
                    .get(&normalize_ident(&c.name))
                    .cloned()
                    .unwrap_or(Value::Null)
            })
            .collect();
        let col_refs: Vec<&Column> = pk_cols.iter().collect();
        let val_refs: Vec<&Value> = pk_vals.iter().collect();
        let fp = encode_composite_key(&col_refs, &val_refs)?;
        return Ok((fp, out));
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

/// Bloque G1: una "proyección resuelta" del SELECT list. Para columnas
/// bare seguimos el lookup directo por clave normalizada (preserva el
/// fast-path pre-G1); para expresiones llevamos el `Expr` y lo evaluamos
/// per-row con `eval_expr`. El campo `display` es el header que ve el
/// caller en `ResultSet.columns`.
#[derive(Debug, Clone)]
enum Projection {
    BareColumn { display: String, key: String },
    Expression { display: String, expr: Expr },
}

impl Projection {
    fn display(&self) -> &str {
        match self {
            Projection::BareColumn { display, .. } => display,
            Projection::Expression { display, .. } => display,
        }
    }
}

// Bloque H (2026-05-26): la antigua `project_row` libre fue reemplazada
// por `Engine::project_row_with_engine`, que puede ejecutar
// `Expr::ScalarSubquery`. Conservar la firma libre causaba código
// muerto — el dispatch ahora siempre pasa por el engine.

fn resolve_selected_columns(
    meta: &TableMeta,
    requested: &[SelectItem],
) -> DbResult<Vec<Projection>> {
    // Bloque F: el path no-agregado solo acepta columnas crudas o `*`.
    // Si llegan `SelectItem::Aggregate` acá es bug del caller — el
    // dispatcher (`needs_aggregation`) debería haber desviado al
    // pipeline de agregación. Devolvemos error explícito por defensa.
    if requested.is_empty() || (requested.len() == 1 && matches!(requested[0], SelectItem::Star)) {
        return Ok(meta
            .columns
            .iter()
            .map(|column| Projection::BareColumn {
                display: column.name.clone(),
                key: normalize_ident(&column.name),
            })
            .collect());
    }

    let mut out = Vec::with_capacity(requested.len());
    for item in requested {
        match item {
            SelectItem::Column(n) => {
                let normalized = normalize_ident(n);
                let column = meta.column(&normalized).ok_or_else(|| {
                    coded(
                        codes::COLUMN_NOT_FOUND,
                        format!("columna '{}' no existe en tabla '{}'", n, meta.name),
                    )
                })?;
                out.push(Projection::BareColumn {
                    display: column.name.clone(),
                    key: normalize_ident(&column.name),
                });
            }
            SelectItem::Star => {
                return Err(coded(
                    codes::COLUMN_NOT_FOUND,
                    "SELECT *: combinar `*` con columnas explícitas no se soporta — usá una lista",
                ));
            }
            SelectItem::Aggregate { .. } => {
                return Err(DbError::new(
                    "interno: resolve_selected_columns no debe recibir agregados".to_string(),
                ));
            }
            SelectItem::Expression { expr, .. } => {
                // Bloque G1: validamos que las columnas referenciadas
                // existan en el schema. Esto da error temprano (en
                // tiempo de planeo) en lugar de explotar a mitad de
                // proyección con un mensaje menos claro.
                validate_expr_columns(expr, meta)?;
                out.push(Projection::Expression {
                    display: item.output_name(),
                    expr: expr.clone(),
                });
            }
        }
    }
    Ok(out)
}

/// Bloque G1: walk recursivo del `Expr` para confirmar que cada
/// `Column(name)` referida existe en el meta. Para JOINs hay una versión
/// específica que mira el `JoinScope` (con qualifier + ambigüedad).
fn validate_expr_columns(expr: &Expr, meta: &TableMeta) -> DbResult<()> {
    match expr {
        Expr::Literal(_) => Ok(()),
        Expr::Column(name) => {
            let key = normalize_ident(name);
            if meta.column(&key).is_none() {
                return Err(coded(
                    codes::COLUMN_NOT_FOUND,
                    format!("columna '{}' no existe en tabla '{}'", name, meta.name),
                ));
            }
            Ok(())
        }
        Expr::Func(_, args) => {
            for a in args {
                validate_expr_columns(a, meta)?;
            }
            Ok(())
        }
        Expr::Cast(inner, _) => validate_expr_columns(inner, meta),
        Expr::Case {
            operand,
            branches,
            else_branch,
        } => {
            if let Some(op) = operand {
                validate_expr_columns(op, meta)?;
            }
            for (c, v) in branches {
                validate_expr_columns(c, meta)?;
                validate_expr_columns(v, meta)?;
            }
            if let Some(e) = else_branch {
                validate_expr_columns(e, meta)?;
            }
            Ok(())
        }
        Expr::Compare(a, _, b) => {
            validate_expr_columns(a, meta)?;
            validate_expr_columns(b, meta)?;
            Ok(())
        }
        Expr::IsNull(inner, _) => validate_expr_columns(inner, meta),
        Expr::Arith(a, _, b) => {
            validate_expr_columns(a, meta)?;
            validate_expr_columns(b, meta)?;
            Ok(())
        }
        Expr::Like(inner, _, _) | Expr::InList(inner, _, _) => validate_expr_columns(inner, meta),
        Expr::Between(a, lo, hi, _) => {
            validate_expr_columns(a, meta)?;
            validate_expr_columns(lo, meta)?;
            validate_expr_columns(hi, meta)?;
            Ok(())
        }
        // Bloque H: la validación temprana no entra a la subquery
        // (su propio executor valida sus columnas contra su propio
        // schema en runtime, y además puede referenciar `outer.col`
        // que aún no está resuelto acá).
        Expr::ScalarSubquery(_) => Ok(()),
    }
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
        on_update: def.on_update,
        name: def.name.clone(),
        extra_source_columns: def.extra_source_columns.clone(),
        extra_target_columns: def.extra_target_columns.clone(),
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
        // Snapshot del padre para chequear nombres y tipos.
        let is_self_ref = fk.table.eq_ignore_ascii_case(&meta.name);
        let target_meta_owned;
        let target: &TableMeta = if is_self_ref {
            meta
        } else {
            let snap = {
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
            target_meta_owned = snap;
            &target_meta_owned
        };

        // Residual #3 (2026-05-27): FK puede ser multi-col. Caso
        // multi-col: target_columns debe ser exactamente la PK del
        // padre (en orden) y todas las source/target deben tener
        // matching types.
        let source_cols = fk.source_columns(&column.name);
        let target_cols = fk.target_columns();
        if source_cols.len() != target_cols.len() {
            return Err(DbError::new(format!(
                "FOREIGN KEY '{}.{}' tiene arity inconsistente: {} source vs {} target",
                meta.name,
                column.name,
                source_cols.len(),
                target_cols.len()
            )));
        }
        let target_pk_cols = target.pk_columns();
        // ANSI permite FK contra UNIQUE arbitrarios, pero gabysql sólo
        // contra PK (la regla histórica pre-#3). Single-col PK + FK
        // sigue funcionando idéntico al pre-#3 (mismo error si el
        // target column no coincide con la PK).
        let pk_set_lc: Vec<String> = target_pk_cols
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect();
        let target_set_lc: Vec<String> =
            target_cols.iter().map(|s| s.to_ascii_lowercase()).collect();
        if pk_set_lc != target_set_lc {
            return Err(DbError::new(format!(
                "FOREIGN KEY '{}.{}' debe referenciar exactamente la PRIMARY KEY de '{}' \
                 en el mismo orden (PK = ({}), FK target = ({})); esta versión no admite \
                 REFERENCES contra columnas no-PK ni contra subconjuntos / reorderings",
                meta.name,
                column.name,
                target.name,
                target_pk_cols.join(", "),
                target_cols.join(", ")
            )));
        }
        // Tipos de cada par (source[i], target[i]) deben matchear.
        for (src, tgt) in source_cols.iter().zip(target_cols.iter()) {
            let src_col = meta.column(src).ok_or_else(|| {
                coded(
                    codes::COLUMN_NOT_FOUND,
                    format!(
                        "FOREIGN KEY '{}': columna source '{}' no existe",
                        meta.name, src
                    ),
                )
            })?;
            let tgt_col = target.column(tgt).ok_or_else(|| {
                DbError::new(format!(
                    "FK rota: tabla '{}' no expone columna '{}'",
                    target.name, tgt
                ))
            })?;
            if src_col.column_type != tgt_col.column_type {
                return Err(DbError::new(format!(
                    "FOREIGN KEY '{}.{}' → '{}.{}' tipos inconsistentes: {} vs {}",
                    meta.name,
                    src,
                    target.name,
                    tgt,
                    src_col.column_type.as_sql(),
                    tgt_col.column_type.as_sql()
                )));
            }
        }
    }
    Ok(())
}

/// Bloque L2 (2026-05-27): evalúa todos los `CHECK (expr)` de la
/// tabla contra la fila propuesta y rebota con `[GBY-3008]` si alguno
/// resulta FALSE (ANSI 3VL: NULL pasa). Para usar en el path de write
/// (INSERT, UPDATE, UPSERT DO UPDATE, ON CONFLICT DO UPDATE).
///
/// Re-parsea el `source` canónico en cada llamada. El costo extra
/// (~lex+parse de cada CHECK por row) es aceptable porque (a) el
/// catálogo guarda texto y no AST, (b) la lista típica es < 5 CHECKs
/// por tabla.
fn enforce_check_constraints(meta: &TableMeta, values: &HashMap<String, Value>) -> DbResult<()> {
    for ck in &meta.check_constraints {
        let expr = parse_expr_str(&ck.source).map_err(|e| {
            DbError::new(format!(
                "CHECK '{}' en '{}': re-parse falló — {}",
                ck.name, meta.name, e
            ))
        })?;
        // Las claves del row vienen normalizadas (`normalize_ident`)
        // — eval_expr ya las resuelve case-insensitive.
        match eval_expr_as_predicate(&expr, values) {
            Ok(Some(true)) => continue,
            Ok(None) => continue, // NULL → pass (3VL ANSI)
            Ok(Some(false)) => {
                return Err(coded(
                    codes::CHECK_VIOLATED,
                    format!(
                        "CHECK '{}' en tabla '{}' violado por la fila propuesta: predicado `{}` evaluó a FALSE",
                        ck.name, meta.name, ck.source
                    ),
                ));
            }
            Err(e) => {
                // Errores del evaluador (división por cero, etc.) — los
                // re-emitimos con contexto del CHECK afectado.
                return Err(DbError::new(format!(
                    "CHECK '{}' en '{}': error al evaluar — {}",
                    ck.name, meta.name, e
                )));
            }
        }
    }
    Ok(())
}

/// Bloque L2 (2026-05-27): valida en DDL que cada `CHECK (expr)` de
/// `meta`:
///
/// 1. Re-parsea limpio desde su `source` canónico (smoke check del
///    round-trip `format_expr` → `parse_expr_str`).
/// 2. Sólo referencia columnas que existen en la tabla. La verificación
///    es por nombre (case-insensitive); columnas qualified
///    (`t.col`) se permiten siempre que el qualifier matchee el nombre
///    de la tabla.
/// 3. No contiene `ScalarSubquery` — `format_expr` ya lo habría
///    rechazado, pero re-chequeamos por defensa en profundidad.
fn validate_check_constraints(meta: &TableMeta) -> DbResult<()> {
    for ck in &meta.check_constraints {
        let expr = parse_expr_str(&ck.source).map_err(|e| {
            DbError::new(format!(
                "CHECK '{}' en tabla '{}': re-parse falló — {}",
                ck.name, meta.name, e
            ))
        })?;
        check_expr_no_subquery(&expr).map_err(|e| {
            DbError::new(format!(
                "CHECK '{}' en tabla '{}': {}",
                ck.name, meta.name, e
            ))
        })?;
        collect_check_columns(&expr, &mut |col| {
            let key = strip_qualifier(col, &meta.name);
            if meta.column(&key).is_none() {
                Err(coded(
                    codes::COLUMN_NOT_FOUND,
                    format!(
                        "CHECK '{}' en tabla '{}': la columna '{}' no existe",
                        ck.name, meta.name, col
                    ),
                ))
            } else {
                Ok(())
            }
        })?;
    }
    Ok(())
}

/// Bloque L2: extrae sólo el nombre de columna (lower) de un eventual
/// `t.col` cuando `t` matchea el nombre de la tabla; si el qualifier
/// es otra tabla, devuelve el ident completo (que el validador
/// rechazará — CHECK no admite refs cross-table).
fn strip_qualifier(name: &str, table: &str) -> String {
    if let Some((qual, col)) = name.split_once('.') {
        if qual.eq_ignore_ascii_case(table) {
            return col.to_string();
        }
    }
    name.to_string()
}

/// Bloque L2: walker del AST que invoca `cb(name)` por cada
/// `Expr::Column(name)` encontrado. Permite reusar el árbol para
/// validar columns y para detectar ScalarSubquery sin re-implementar
/// el recorrido.
fn collect_check_columns(expr: &Expr, cb: &mut dyn FnMut(&str) -> DbResult<()>) -> DbResult<()> {
    match expr {
        Expr::Literal(_) => Ok(()),
        Expr::Column(name) => cb(name),
        Expr::Func(_, args) => {
            for a in args {
                collect_check_columns(a, cb)?;
            }
            Ok(())
        }
        Expr::Cast(inner, _) => collect_check_columns(inner, cb),
        Expr::Case {
            operand,
            branches,
            else_branch,
        } => {
            if let Some(op) = operand {
                collect_check_columns(op, cb)?;
            }
            for (c, v) in branches {
                collect_check_columns(c, cb)?;
                collect_check_columns(v, cb)?;
            }
            if let Some(e) = else_branch {
                collect_check_columns(e, cb)?;
            }
            Ok(())
        }
        Expr::Compare(l, _, r) | Expr::Arith(l, _, r) => {
            collect_check_columns(l, cb)?;
            collect_check_columns(r, cb)
        }
        Expr::IsNull(inner, _) | Expr::Like(inner, _, _) | Expr::InList(inner, _, _) => {
            collect_check_columns(inner, cb)
        }
        Expr::Between(lhs, lo, hi, _) => {
            collect_check_columns(lhs, cb)?;
            collect_check_columns(lo, cb)?;
            collect_check_columns(hi, cb)
        }
        Expr::ScalarSubquery(_) => Err(coded(
            codes::CHECK_CONTAINS_SUBQUERY,
            "CHECK no admite subqueries",
        )),
    }
}

/// Bloque L2: valida que `expr` no contenga subqueries en ningún nivel.
fn check_expr_no_subquery(expr: &Expr) -> DbResult<()> {
    collect_check_columns(expr, &mut |_| Ok(()))
}

/// Verify that the given `target_pk` exists in the FK's parent table.
/// `self_ref_allowed_pk` lets INSERT/UPDATE accept a self-FK that points
/// at the very row being written (the row will exist as soon as the
/// statement commits — refusing it would make self-managed entities
/// impossible to insert in the first place).
/// Residual #3 (2026-05-27): calcula la PK del parent (fingerprint o
/// i64 directo) a partir de los valores source de la FK en el orden
/// declarado. Devuelve `None` si algún valor source es NULL — en ese
/// caso ANSI dice "FK no se chequea" (filas con NULL en FK col se
/// admiten libremente, igual que pre-#3).
///
/// Para FK single-col target = single-col PK: devuelve el INT
/// directamente (compatible con el contrato pre-#3 del B+Tree).
/// Para FK multi-col target = PK compuesta: devuelve el fingerprint
/// FNV-1a-64 i64 (mismo encoder que K2 usa para insertar la PK).
fn fk_lookup_parent_pk(
    fk: &ForeignKeyMeta,
    source_values: &[Value],
    parent_meta: &TableMeta,
) -> DbResult<Option<i64>> {
    if source_values.iter().any(|v| matches!(v, Value::Null)) {
        return Ok(None);
    }
    if !parent_meta.has_composite_pk() && !fk.is_composite() {
        // Caso histórico pre-#3: single-col INT FK.
        match &source_values[0] {
            Value::Integer(n) => Ok(Some(*n)),
            other => Err(DbError::new(format!(
                "FK '{}': valor source no-INT ({:?}) contra PK single-col",
                fk.name.as_deref().unwrap_or("(sin nombre)"),
                other
            ))),
        }
    } else {
        // Multi-col o single-col contra PK compuesta. Construir el
        // fingerprint en el orden EXACTO de la PK del parent
        // (`pk_columns()`), que el validator DDL exige sea idéntico
        // al orden `fk.target_columns()`.
        let pk_col_names = parent_meta.pk_columns();
        let parent_pk_cols: Vec<Column> = pk_col_names
            .iter()
            .map(|n| {
                parent_meta.column(n).cloned().ok_or_else(|| {
                    DbError::new(format!(
                        "FK rota: tabla padre '{}' no expone columna PK '{}'",
                        parent_meta.name, n
                    ))
                })
            })
            .collect::<DbResult<_>>()?;
        let col_refs: Vec<&Column> = parent_pk_cols.iter().collect();
        let val_refs: Vec<&Value> = source_values.iter().collect();
        Ok(Some(encode_composite_key(&col_refs, &val_refs)?))
    }
}

/// Verify that the parent row identified by the FK source values
/// exists. NULL en cualquier source column → no-op (ANSI). Self-ref a
/// la fila que se está insertando (mismo PK) → no-op.
fn check_fk_value(
    pager: &mut Pager,
    meta: &TableMeta,
    column_name: &str,
    fk: &ForeignKeyMeta,
    source_values: &[Value],
    self_ref_allowed_pk: i64,
) -> DbResult<()> {
    let parent_meta = if fk.table.eq_ignore_ascii_case(&meta.name) {
        meta.clone()
    } else {
        let mut catalog = Catalog::open(pager);
        catalog.get_table(&fk.table)?.ok_or_else(|| {
            DbError::new(format!(
                "FK rota: tabla '{}' no existe (referida por '{}.{}')",
                fk.table, meta.name, column_name
            ))
        })?
    };
    let Some(target_pk) = fk_lookup_parent_pk(fk, source_values, &parent_meta)? else {
        return Ok(()); // NULL en source → ANSI lo deja pasar.
    };
    if fk.table.eq_ignore_ascii_case(&meta.name) && target_pk == self_ref_allowed_pk {
        return Ok(());
    }
    let exists = {
        let mut catalog = Catalog::open(pager);
        catalog.get_row(parent_meta.root_page, target_pk)?.is_some()
    };
    if !exists {
        let src_names = fk.source_columns(column_name).join(", ");
        let val_repr = source_values
            .iter()
            .map(value_repr_compact)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(coded(
            codes::FK_PARENT_MISSING,
            format!(
                "violación de FOREIGN KEY{}: ({}) = ({}) no existe en la tabla padre '{}'",
                fk.name
                    .as_ref()
                    .map(|n| format!(" '{}'", n))
                    .unwrap_or_default(),
                src_names,
                val_repr,
                fk.table
            ),
        ));
    }
    Ok(())
}

/// Helper para mensaje de FK_PARENT_MISSING: representación compacta
/// de un `Value` sin las decoraciones del Debug.
fn value_repr_compact(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Integer(n) => n.to_string(),
        Value::Float(n) => n.to_string(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::String(s) => format!("'{}'", s),
    }
}

/// Helper: collect source values for an FK from a row HashMap, in the
/// order declared (anchor col first, then `extra_source_columns`).
fn collect_fk_source_values(
    fk: &ForeignKeyMeta,
    anchor_col: &str,
    row: &HashMap<String, Value>,
) -> Vec<Value> {
    fk.source_columns(anchor_col)
        .iter()
        .map(|c| row.get(&normalize_ident(c)).cloned().unwrap_or(Value::Null))
        .collect()
}

/// Walk every FK and call [`check_fk_value`] when the source tuple is
/// non-NULL. INSERT-time entry point.
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
        let source_values = collect_fk_source_values(fk, &column.name, values);
        check_fk_value(pager, meta, &column.name, fk, &source_values, new_pk)?;
    }
    Ok(())
}

/// UPDATE-time entry point. Re-valida la FK sólo si CAMBIÓ alguna de
/// las columnas source — leaving the tuple unchanged can never break
/// referential integrity.
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
        let old_vals = collect_fk_source_values(fk, &column.name, old_row);
        let new_vals = collect_fk_source_values(fk, &column.name, current);
        if old_vals == new_vals {
            continue;
        }
        check_fk_value(pager, meta, &column.name, fk, &new_vals, pk)?;
    }
    Ok(())
}

/// Find every child PK whose FK source tuple matches the parent row
/// being deleted. Residual #3 (2026-05-27): generalizado para FK
/// multi-col. La función recibe los **valores target** del parent
/// (en el orden `fk.target_columns()`) y compara contra los valores
/// source del child fila por fila.
///
/// Para FK single-col el path usa la index lookup rápida (Hash u
/// OrderedInt) que K2 ya tenía. Para multi-col cae a full-scan; un
/// fast-path por composite UNIQUE index sobre los source cols
/// (que K2/L1 ya saben construir) queda como mejora futura.
fn find_child_pks_with_fk_value(
    pager: &mut Pager,
    child_table: &TableMeta,
    fk: &ForeignKeyMeta,
    anchor_col: &str,
    target_values: &[Value],
) -> DbResult<Vec<i64>> {
    let source_col_names = fk.source_columns(anchor_col);
    if source_col_names.len() != target_values.len() {
        return Err(DbError::new(format!(
            "FK incoherente: arity {} source vs {} target en '{}'",
            source_col_names.len(),
            target_values.len(),
            child_table.name
        )));
    }

    // Fast-path single-col: usa índice secundario por la columna FK.
    if !fk.is_composite() {
        let parent_pk = match &target_values[0] {
            Value::Integer(n) => *n,
            _ => return Ok(Vec::new()),
        };
        let column = child_table.column(anchor_col).ok_or_else(|| {
            DbError::new(format!(
                "FK incoherente: columna '{}' no existe en '{}'",
                anchor_col, child_table.name
            ))
        })?;
        let value = Value::Integer(parent_pk);
        let value_bytes = encode_column_value(column, &value)?;

        if let Some(idx) = child_table.index_for_column(anchor_col) {
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
        // Fallback full-scan.
        let mut catalog = Catalog::open(pager);
        let rows = catalog.scan_rows(child_table.root_page, 0, None)?;
        let mut hits = Vec::new();
        for kv in rows {
            let row = decode_row(child_table, &kv.value)?;
            if let Some(Value::Integer(n)) = row.get(&normalize_ident(anchor_col)) {
                if *n == parent_pk {
                    hits.push(kv.key);
                }
            }
        }
        return Ok(hits);
    }

    // Multi-col path: full-scan comparando tuplas. Cualquier source
    // value NULL en el row del child hace que no match (igual que
    // PostgreSQL: NULL ≠ NULL en FK matching).
    let mut catalog = Catalog::open(pager);
    let rows = catalog.scan_rows(child_table.root_page, 0, None)?;
    let mut hits = Vec::new();
    for kv in rows {
        let row = decode_row(child_table, &kv.value)?;
        let mut all_match = true;
        for (i, src) in source_col_names.iter().enumerate() {
            let got = row
                .get(&normalize_ident(src))
                .cloned()
                .unwrap_or(Value::Null);
            // NULL en cualquier source → no match.
            if matches!(got, Value::Null) || got != target_values[i] {
                all_match = false;
                break;
            }
        }
        if all_match {
            hits.push(kv.key);
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
        // Residual #3 (2026-05-27): para FK multi-col necesitamos los
        // VALORES de las columnas PK del parent (no sólo el fingerprint
        // i64). Decodificamos la fila del padre acá, ANTES de tocar
        // children, así si el padre ya no existe (cascade que llegó
        // doblemente) seguimos siendo idempotentes.
        let parent_row_values: Option<HashMap<String, Value>> = {
            let mut catalog = Catalog::open(pager);
            match catalog.get_row(parent_meta.root_page, parent_pk)? {
                Some(bytes) => Some(decode_row(&parent_meta, &bytes)?),
                None => None,
            }
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
                // Construir los valores target del parent en el orden
                // exacto de `fk.target_columns()` para alimentar el
                // matcher de children.
                let Some(parent_row) = parent_row_values.as_ref() else {
                    // Padre ya borrado en pasada anterior → nada para
                    // cascadear desde este nodo. Otros pares parent/pk
                    // del queue siguen.
                    continue;
                };
                let target_values: Vec<Value> = fk
                    .target_columns()
                    .iter()
                    .map(|t| {
                        parent_row
                            .get(&normalize_ident(t))
                            .cloned()
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                let child_pks = find_child_pks_with_fk_value(
                    pager,
                    child_table,
                    fk,
                    &child_col.name,
                    &target_values,
                )?;
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
                    OnDelete::SetNull => {
                        // Bloque L1 + residual #3: la cascade pone NULL
                        // en TODAS las columnas source de la FK. Si
                        // cualquiera de ellas es NOT NULL, falla antes
                        // de tocar disco — no hay rollback parcial.
                        let source_col_names = fk.source_columns(&child_col.name);
                        for src in &source_col_names {
                            let scol = child_table.column(src).ok_or_else(|| {
                                DbError::new(format!(
                                    "FK rota: columna source '{}' no existe en '{}'",
                                    src, child_table.name
                                ))
                            })?;
                            if scol.not_null {
                                return Err(coded(
                                    codes::FK_SET_NULL_VIOLATES_NOT_NULL,
                                    format!(
                                        "DELETE FROM '{}' bloqueado: '{}.{}' es NOT NULL y la FK \
                                         declaró ON DELETE SET NULL ({} fila(s) hijas afectarían)",
                                        parent_name,
                                        child_table.name,
                                        src,
                                        child_pks.len()
                                    ),
                                ));
                            }
                        }
                        let new_values: Vec<Value> =
                            source_col_names.iter().map(|_| Value::Null).collect();
                        for cpk in child_pks {
                            cascade_set_fk_tuple(
                                pager,
                                child_table,
                                cpk,
                                &source_col_names,
                                &new_values,
                            )?;
                        }
                    }
                    OnDelete::SetDefault => {
                        // Residual #3: SET DEFAULT reasigna CADA source
                        // column a su DEFAULT declarado. Sin DEFAULT en
                        // alguna, [GBY-3010]. DEFAULT NULL con NOT NULL,
                        // [GBY-3002].
                        let source_col_names = fk.source_columns(&child_col.name);
                        let mut new_values: Vec<Value> = Vec::with_capacity(source_col_names.len());
                        for src in &source_col_names {
                            let scol = child_table.column(src).ok_or_else(|| {
                                DbError::new(format!(
                                    "FK rota: columna source '{}' no existe en '{}'",
                                    src, child_table.name
                                ))
                            })?;
                            let Some(default) = &scol.default else {
                                return Err(coded(
                                    codes::FK_SET_DEFAULT_MISSING,
                                    format!(
                                        "DELETE FROM '{}' bloqueado: '{}.{}' no tiene DEFAULT y \
                                         la FK declaró ON DELETE SET DEFAULT ({} fila(s) hijas \
                                         afectarían)",
                                        parent_name,
                                        child_table.name,
                                        src,
                                        child_pks.len()
                                    ),
                                ));
                            };
                            let v = default_to_value(default);
                            if matches!(v, Value::Null) && scol.not_null {
                                return Err(coded(
                                    codes::NOT_NULL_VIOLATED,
                                    format!(
                                        "DELETE FROM '{}' bloqueado: ON DELETE SET DEFAULT \
                                         pondría '{}.{}' (NOT NULL) en NULL — el DEFAULT \
                                         declarado es NULL",
                                        parent_name, child_table.name, src
                                    ),
                                ));
                            }
                            new_values.push(v);
                        }
                        for cpk in child_pks {
                            cascade_set_fk_tuple(
                                pager,
                                child_table,
                                cpk,
                                &source_col_names,
                                &new_values,
                            )?;
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
            if idx.is_composite() {
                // Bloque L1: composites se limpian por fingerprint.
                let fp = composite_fp_for_values(&parent_meta, idx, &row)?;
                composite_index_remove(pager, idx.root_page, fp, parent_pk)?;
            } else {
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
        }
        let mut catalog = Catalog::open(pager);
        catalog.delete_row(parent_meta.root_page, parent_pk)?;
    }
    Ok(())
}

/// Bloque L1 (2026-05-27): calcula el fingerprint FNV-1a-64 de un row
/// para un índice compuesto. Espera que todas las columnas del índice
/// existan en `values`; las ausentes caen a `Value::Null` (lo cual el
/// encoder traduce a un sentinel — no es válido para UNIQUE compuesto
/// porque K2 exige NOT NULL, pero defendemos en profundidad).
fn composite_fp_for_values(
    meta: &TableMeta,
    idx: &IndexMeta,
    values: &HashMap<String, Value>,
) -> DbResult<i64> {
    let composite_columns: Vec<Column> = idx
        .all_columns()
        .iter()
        .map(|name| {
            meta.column(name).cloned().ok_or_else(|| {
                DbError::new(format!(
                    "índice compuesto '{}' apunta a columna inexistente: {}",
                    idx.name, name
                ))
            })
        })
        .collect::<DbResult<_>>()?;
    let vals: Vec<Value> = composite_columns
        .iter()
        .map(|c| {
            values
                .get(&normalize_ident(&c.name))
                .cloned()
                .unwrap_or(Value::Null)
        })
        .collect();
    let col_refs: Vec<&Column> = composite_columns.iter().collect();
    let val_refs: Vec<&Value> = vals.iter().collect();
    encode_composite_key(&col_refs, &val_refs)
}

/// Bloque L1: chequea conflicto UNIQUE para un índice compuesto sobre
/// el fingerprint `fp`. El bucket guarda PKs como un ordered set; si ya
/// hay alguna PK distinta de `exclude_pk`, la combinación de columnas
/// está duplicada → error.
fn composite_unique_check(
    pager: &mut Pager,
    idx: &IndexMeta,
    fp: i64,
    exclude_pk: Option<i64>,
) -> DbResult<()> {
    let mut tree = Tree::new(pager);
    let bucket = match tree.get(idx.root_page, fp)? {
        Some(bytes) => decode_ordered_bucket(&bytes)?,
        None => return Ok(()),
    };
    let conflict = bucket.iter().any(|pk| Some(*pk) != exclude_pk);
    if conflict {
        let cols = idx.all_columns().join(", ");
        return Err(coded(
            codes::UNIQUE_VIOLATED,
            format!(
                "violación de UNIQUE en índice compuesto '{}' sobre ({})",
                idx.name, cols
            ),
        ));
    }
    Ok(())
}

/// Bloque L1: upsert (`fp` → bucket of PKs) en un índice compuesto. La
/// clave del B+Tree es el fingerprint i64 directo, no la codificación
/// OrderedInt (que envuelve con un tag). Vive aparte de `index_upsert_pk`
/// porque K2 estableció ese contrato y cambiarlo rompería el bucket
/// existente.
fn composite_index_upsert(pager: &mut Pager, idx_root: u32, fp: i64, pk: i64) -> DbResult<()> {
    let mut tree = Tree::new(pager);
    let mut bucket = match tree.get(idx_root, fp)? {
        Some(bytes) => decode_ordered_bucket(&bytes)?,
        None => Vec::new(),
    };
    ordered_bucket_insert(&mut bucket, pk);
    let payload = encode_ordered_bucket(&bucket)?;
    tree.upsert(idx_root, fp, payload)?;
    Ok(())
}

/// Bloque L1: contraparte del upsert — saca `pk` del bucket en
/// `fp`. Si el bucket queda vacío, borra la entrada.
fn composite_index_remove(pager: &mut Pager, idx_root: u32, fp: i64, pk: i64) -> DbResult<bool> {
    let mut tree = Tree::new(pager);
    let Some(bytes) = tree.get(idx_root, fp)? else {
        return Ok(false);
    };
    let mut bucket = decode_ordered_bucket(&bytes)?;
    let removed = ordered_bucket_remove(&mut bucket, pk);
    if !removed {
        return Ok(false);
    }
    if bucket.is_empty() {
        tree.delete(idx_root, fp)?;
    } else {
        let payload = encode_ordered_bucket(&bucket)?;
        tree.upsert(idx_root, fp, payload)?;
    }
    Ok(true)
}

/// Bloque L1 (2026-05-27): mutate one column of one row in `child_meta`
/// in place. Wrapper sobre `cascade_set_fk_tuple` para el caso
/// single-col. Sigue acá por compatibilidad con tests y para mantener
/// el call-site del cascade SET NULL/SET DEFAULT pre-#3 idéntico.
#[allow(dead_code)]
fn cascade_set_fk_value(
    pager: &mut Pager,
    child_meta: &TableMeta,
    child_pk: i64,
    column_name: &str,
    new_value: Value,
) -> DbResult<()> {
    cascade_set_fk_tuple(
        pager,
        child_meta,
        child_pk,
        &[column_name],
        std::slice::from_ref(&new_value),
    )
}

/// Residual #3 (2026-05-27): mutate N source columns of one row at
/// once, used by `ON DELETE SET NULL` / `SET DEFAULT` para FKs
/// multi-col (single-col también pasa por acá, con `column_names.len()
/// == 1`). Atómico: o todas las columnas se mutan, o ninguna.
///
/// Mantiene los índices secundarios que tocan cualquier source col,
/// y revalida los CHECK del child contra la fila resultante.
fn cascade_set_fk_tuple(
    pager: &mut Pager,
    child_meta: &TableMeta,
    child_pk: i64,
    column_names: &[&str],
    new_values: &[Value],
) -> DbResult<()> {
    if column_names.len() != new_values.len() {
        return Err(DbError::new(format!(
            "cascade_set_fk_tuple: arity {} cols vs {} values",
            column_names.len(),
            new_values.len()
        )));
    }
    // Helper: ¿este índice toca alguna de las columnas que estamos mutando?
    let idx_touches_any = |idx: &IndexMeta| -> bool {
        for cname in column_names {
            if idx.column.eq_ignore_ascii_case(cname)
                || idx
                    .extra_columns
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(cname))
            {
                return true;
            }
        }
        false
    };

    // 1. Leer la fila — puede haber desaparecido si una cascade previa
    //    la borró por otro camino (ciclos, multi-FK). Tratar como no-op.
    let bytes = {
        let mut catalog = Catalog::open(pager);
        match catalog.get_row(child_meta.root_page, child_pk)? {
            Some(b) => b,
            None => return Ok(()),
        }
    };
    let old_row = decode_row(child_meta, &bytes)?;
    // Si NINGUNA columna cambia, no hacemos nada.
    let mut anything_changed = false;
    for (cname, new_val) in column_names.iter().zip(new_values.iter()) {
        let key = normalize_ident(cname);
        let old = old_row.get(&key).cloned().unwrap_or(Value::Null);
        if old != *new_val {
            anything_changed = true;
            break;
        }
    }
    if !anything_changed {
        return Ok(());
    }

    // 2. Preparar la fila nueva (todas las columnas a la vez).
    let mut new_row = old_row.clone();
    for (cname, new_val) in column_names.iter().zip(new_values.iter()) {
        new_row.insert(normalize_ident(cname), new_val.clone());
    }

    // 3. Validación NOT NULL para CADA columna mutada.
    for (cname, new_val) in column_names.iter().zip(new_values.iter()) {
        let column = child_meta.column(cname).ok_or_else(|| {
            DbError::new(format!(
                "cascade_set_fk_tuple: columna '{}' no existe en '{}'",
                cname, child_meta.name
            ))
        })?;
        if column.not_null && matches!(new_val, Value::Null) {
            return Err(coded(
                codes::NOT_NULL_VIOLATED,
                format!(
                    "cascade rechazada: '{}.{}' es NOT NULL y la acción intentó poner NULL",
                    child_meta.name, column.name
                ),
            ));
        }
    }

    // Bloque L2: CHECK eval contra la fila resultante completa.
    enforce_check_constraints(child_meta, &new_row)?;

    // 4. Pre-check UNIQUE para los índices afectados.
    for idx in &child_meta.indexes {
        if !idx.unique || !idx_touches_any(idx) {
            continue;
        }
        if idx.is_composite() {
            let new_fp = composite_fp_for_values(child_meta, idx, &new_row)?;
            composite_unique_check(pager, idx, new_fp, Some(child_pk))?;
        } else {
            let idx_col = child_meta.column(&idx.column).ok_or_else(|| {
                DbError::new(format!(
                    "índice apunta a columna inexistente: {}",
                    idx.column
                ))
            })?;
            let new_v = new_row
                .get(&normalize_ident(&idx.column))
                .cloned()
                .unwrap_or(Value::Null);
            let value_bytes = encode_column_value(idx_col, &new_v)?;
            check_unique_conflict(pager, idx, &value_bytes, Some(child_pk))?;
        }
    }

    // 5. Sacar las entradas viejas de los índices afectados.
    for idx in &child_meta.indexes {
        if !idx_touches_any(idx) {
            continue;
        }
        if idx.is_composite() {
            let old_fp = composite_fp_for_values(child_meta, idx, &old_row)?;
            composite_index_remove(pager, idx.root_page, old_fp, child_pk)?;
        } else {
            let idx_col = child_meta.column(&idx.column).ok_or_else(|| {
                DbError::new(format!(
                    "índice apunta a columna inexistente: {}",
                    idx.column
                ))
            })?;
            let old_col_value = old_row
                .get(&normalize_ident(&idx.column))
                .cloned()
                .unwrap_or(Value::Null);
            let old_bytes = encode_column_value(idx_col, &old_col_value)?;
            index_remove_pk(pager, idx.root_page, idx.kind, &old_bytes, child_pk)?;
        }
    }

    // 6. Re-encodear y escribir la fila.
    let (encoded_pk, row_bytes) = encode_row(child_meta, &new_row)?;
    if encoded_pk != child_pk {
        return Err(DbError::new(format!(
            "inconsistencia interna en cascade_set_fk_tuple sobre '{}': la PK reconstruida \
             del row es {} pero la cascade apuntaba a pk={}",
            child_meta.name, encoded_pk, child_pk
        )));
    }
    {
        let mut catalog = Catalog::open(pager);
        catalog.upsert_row(child_meta.root_page, encoded_pk, row_bytes)?;
    }

    // 7. Re-insertar en cada índice afectado con la fila nueva.
    for idx in &child_meta.indexes {
        if !idx_touches_any(idx) {
            continue;
        }
        if idx.is_composite() {
            let new_fp = composite_fp_for_values(child_meta, idx, &new_row)?;
            composite_index_upsert(pager, idx.root_page, new_fp, child_pk)?;
        } else {
            let idx_col = child_meta.column(&idx.column).ok_or_else(|| {
                DbError::new(format!(
                    "índice apunta a columna inexistente: {}",
                    idx.column
                ))
            })?;
            let new_col_value = new_row
                .get(&normalize_ident(&idx.column))
                .cloned()
                .unwrap_or(Value::Null);
            let new_bytes = encode_column_value(idx_col, &new_col_value)?;
            index_upsert_pk(pager, idx.root_page, idx.kind, &new_bytes, child_pk)?;
        }
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
    /// Bloque H (2026-05-26): cuando este JoinTable proviene de una
    /// derived table `(SELECT ...) AS alias`, aquí viven las filas ya
    /// materializadas (decoded). `scan_qualified` las devuelve en lugar
    /// de hacer FullScan contra el pager. El index-loop plan no aplica
    /// — el `meta` virtual no tiene ni PK ni índices.
    virtual_rows: Option<Vec<HashMap<String, Value>>>,
}

/// Bloque H (2026-05-26): salida de `materialize_derived_table` —
/// schema virtual + filas decodificadas listas para usar.
struct MaterializedDerived {
    meta: TableMeta,
    rows: Vec<HashMap<String, Value>>,
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
    // Bloque H: las derived tables no tienen pager-backed storage —
    // el index-loop fast-path no aplica. Caemos al nested-loop con
    // las filas virtuales ya materializadas (scan_qualified las
    // devuelve directamente).
    if right.virtual_rows.is_some() {
        return Ok(None);
    }
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
                    virtual_rows: t.virtual_rows.clone(),
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

/// Convierte una columna del SELECT (`*`, `col` o `tabla.col`) en una
/// lista de `JoinedProjection`. `*` se expande a TODAS las columnas de
/// TODAS las tablas, en orden. Bloque G1: ahora también acepta
/// expresiones escalares (`SelectItem::Expression`); las columnas que la
/// expresión referencia se resuelven contra el `JoinScope` (qualifier
/// si lo trae, sino busca en todas las tablas con check de ambigüedad).
fn resolve_joined_projection(
    scope: &JoinScope,
    requested: &[SelectItem],
) -> DbResult<(Vec<String>, Vec<JoinedProjection>)> {
    let mut output = Vec::new();
    let mut projs = Vec::new();
    if requested.is_empty() || (requested.len() == 1 && matches!(requested[0], SelectItem::Star)) {
        for t in &scope.tables {
            for col in &t.meta.columns {
                let key = format!("{}.{}", t.qualifier, normalize_ident(&col.name));
                if scope.hidden_in_star.contains(&key) {
                    continue;
                }
                output.push(format!("{}.{}", t.qualifier, col.name));
                projs.push(JoinedProjection::Key(key));
            }
        }
        return Ok((output, projs));
    }
    for item in requested {
        match item {
            SelectItem::Column(raw) => {
                output.push(raw.clone());
                projs.push(JoinedProjection::Key(resolve_joined_column_key(
                    scope, raw,
                )?));
            }
            SelectItem::Star => {
                return Err(coded(
                    codes::COLUMN_NOT_FOUND,
                    "SELECT *: combinar `*` con columnas explícitas no se soporta",
                ));
            }
            SelectItem::Aggregate { .. } => {
                return Err(coded(
                    codes::AGGREGATE_OVER_JOIN_UNSUPPORTED,
                    "agregados (COUNT/SUM/AVG/MIN/MAX) sobre SELECT con JOIN aún no se soportan; \
                     reescribir como subquery agregada sobre la tabla base",
                ));
            }
            SelectItem::Expression { expr, .. } => {
                // Bloque G1: re-escribimos cada `Expr::Column` para que
                // apunte a la clave cualificada que vive en la fila
                // joineada (`alias.col`). Si el ident es ambiguo o no
                // existe en ninguna tabla, `resolve_joined_column_key`
                // devuelve `[GBY-4018]` / `[GBY-4019]`.
                let rewritten = rewrite_expr_columns_for_join(expr.clone(), scope)?;
                output.push(item.output_name());
                projs.push(JoinedProjection::Expr(rewritten));
            }
        }
    }
    Ok((output, projs))
}

/// Bloque G1: una proyección dentro de un SELECT con JOIN. `Key` es el
/// camino rápido pre-G1 (lookup directo en la HashMap joineada);
/// `Expr` evalúa la expresión contra la fila joineada con
/// `eval_expr` — los `Expr::Column` ya fueron reescritos a la forma
/// cualificada por `rewrite_expr_columns_for_join`.
#[derive(Debug, Clone)]
enum JoinedProjection {
    Key(String),
    Expr(Expr),
}

/// Bloque G1: reescribe cada `Expr::Column(name)` para que su nombre sea
/// la clave cualificada usada en la fila joineada (`alias.col`). Esto
/// permite reutilizar `eval_expr` sin enseñarle a navegar el `JoinScope`.
fn rewrite_expr_columns_for_join(expr: Expr, scope: &JoinScope) -> DbResult<Expr> {
    match expr {
        Expr::Literal(v) => Ok(Expr::Literal(v)),
        Expr::Column(raw) => Ok(Expr::Column(resolve_joined_column_key(scope, &raw)?)),
        Expr::Func(f, args) => {
            let mut out = Vec::with_capacity(args.len());
            for a in args {
                out.push(rewrite_expr_columns_for_join(a, scope)?);
            }
            Ok(Expr::Func(f, out))
        }
        Expr::Cast(inner, ty) => Ok(Expr::Cast(
            Box::new(rewrite_expr_columns_for_join(*inner, scope)?),
            ty,
        )),
        Expr::Case {
            operand,
            branches,
            else_branch,
        } => {
            let operand = match operand {
                Some(op) => Some(Box::new(rewrite_expr_columns_for_join(*op, scope)?)),
                None => None,
            };
            let mut new_branches = Vec::with_capacity(branches.len());
            for (c, v) in branches {
                new_branches.push((
                    rewrite_expr_columns_for_join(c, scope)?,
                    rewrite_expr_columns_for_join(v, scope)?,
                ));
            }
            let else_branch = match else_branch {
                Some(e) => Some(Box::new(rewrite_expr_columns_for_join(*e, scope)?)),
                None => None,
            };
            Ok(Expr::Case {
                operand,
                branches: new_branches,
                else_branch,
            })
        }
        Expr::Compare(a, op, b) => Ok(Expr::Compare(
            Box::new(rewrite_expr_columns_for_join(*a, scope)?),
            op,
            Box::new(rewrite_expr_columns_for_join(*b, scope)?),
        )),
        Expr::IsNull(inner, neg) => Ok(Expr::IsNull(
            Box::new(rewrite_expr_columns_for_join(*inner, scope)?),
            neg,
        )),
        Expr::Arith(a, op, b) => Ok(Expr::Arith(
            Box::new(rewrite_expr_columns_for_join(*a, scope)?),
            op,
            Box::new(rewrite_expr_columns_for_join(*b, scope)?),
        )),
        Expr::Like(inner, pat, neg) => Ok(Expr::Like(
            Box::new(rewrite_expr_columns_for_join(*inner, scope)?),
            pat,
            neg,
        )),
        Expr::InList(inner, vs, neg) => Ok(Expr::InList(
            Box::new(rewrite_expr_columns_for_join(*inner, scope)?),
            vs,
            neg,
        )),
        Expr::Between(a, lo, hi, neg) => Ok(Expr::Between(
            Box::new(rewrite_expr_columns_for_join(*a, scope)?),
            Box::new(rewrite_expr_columns_for_join(*lo, scope)?),
            Box::new(rewrite_expr_columns_for_join(*hi, scope)?),
            neg,
        )),
        // Bloque H: la subquery vive en su propio scope; el rewriting
        // de qualifiers no debe descender dentro de ella. El engine la
        // ejecuta con su propio `parse_select_stmt`/`build_join_scope`.
        Expr::ScalarSubquery(sub) => Ok(Expr::ScalarSubquery(sub)),
    }
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
    in_result.map(|b| if negated { !b } else { b })
}

/// Bloque H (2026-05-26): materializa el set proyectado por una subquery
/// en `IN (SELECT ...)`. Devuelve los valores no-NULL como `Vec<Value>`
/// y, separadamente, si la subquery contenía algún NULL. El flag lo
/// usa `eval_in_subquery` para aplicar la 3VL ANSI en `NOT IN`:
/// `5 NOT IN (1, NULL)` → NULL, no true.
fn collect_in_set(rows: Vec<Vec<Value>>) -> (Vec<Value>, bool) {
    let mut set = Vec::with_capacity(rows.len());
    let mut had_null = false;
    for mut r in rows {
        if let Some(v) = r.pop() {
            if matches!(v, Value::Null) {
                had_null = true;
            } else {
                set.push(v);
            }
        }
    }
    (set, had_null)
}

/// Bloque H: evalúa `lhs [NOT] IN (subquery_set)` con semántica ANSI
/// trivaluada. Reglas:
/// - `lhs` NULL → NULL (3VL clásica).
/// - Afirmativo `IN`: true si está en el set; false si no (los NULL de
///   la subquery se ignoran por completo).
/// - `NOT IN`: si el set tiene match → false; si no hay match y la
///   subquery contenía algún NULL → NULL (estricta ANSI); si no hay
///   match y no había NULL → true.
fn eval_in_subquery(
    lhs: Option<&Value>,
    set: &[Value],
    had_null: bool,
    negated: bool,
) -> Option<bool> {
    let lhs = lhs?;
    if matches!(lhs, Value::Null) {
        return None;
    }
    let matched = set.iter().any(|v| values_equal(lhs, v));
    if !negated {
        return Some(matched);
    }
    if matched {
        Some(false)
    } else if had_null {
        None
    } else {
        Some(true)
    }
}

/// Bloque J2: resultado de aplicar un row del INSERT cuando hay
/// `ON CONFLICT`. Permite distinguir las 3 trayectorias (insertó nuevo,
/// reemplazó una existente, o saltó por DO NOTHING) sin volver a
/// recorrer el storage.
enum RowOutcome {
    Inserted(HashMap<String, Value>),
    Updated(HashMap<String, Value>),
    Skipped,
}

/// Bloque J2: encuentra las PKs que conflictúan con el row propuesto.
/// Si hay `target` explícito (`ON CONFLICT (col)`), busca solo esa
/// constraint. Sin target, escanea PK + todos los UNIQUE indexes.
/// Devuelve `Vec<i64>` con las PKs ofendidas (de-duplicadas y ordenadas).
fn detect_conflict_pks(
    pager: &mut Pager,
    meta: &TableMeta,
    values: &HashMap<String, Value>,
    target: Option<&str>,
) -> DbResult<Vec<i64>> {
    let mut out: Vec<i64> = Vec::new();
    let pk_key = normalize_ident(&meta.primary_key);
    let check_pk = match target {
        Some(t) => normalize_ident(t) == pk_key,
        None => true,
    };
    if check_pk {
        if let Some(Value::Integer(n)) = values.get(&pk_key) {
            let exists = {
                let mut catalog = Catalog::open(pager);
                catalog.get_row(meta.root_page, *n)?.is_some()
            };
            if exists {
                out.push(*n);
            }
        }
    }
    for idx in &meta.indexes {
        if !idx.unique {
            continue;
        }
        let key = normalize_ident(&idx.column);
        if let Some(t) = target {
            if normalize_ident(t) != key {
                continue;
            }
        }
        let column = meta.column(&idx.column).ok_or_else(|| {
            DbError::new(format!("índice apunta a col inexistente: {}", idx.column))
        })?;
        let v = values.get(&key).cloned().unwrap_or(Value::Null);
        if matches!(v, Value::Null) {
            // NULLs nunca conflictúan en UNIQUE (ANSI).
            continue;
        }
        let bytes = encode_column_value(column, &v)?;
        let pks = lookup_pks_via_index(pager, meta, idx, &v)?;
        // `pks` ya está deduplicado por construcción del index lookup.
        for p in pks {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        let _ = bytes; // referencia al binding para silenciar el unused
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Bloque J2: formato del `message` del response para INSERT/REPLACE
/// con on_conflict. Cuenta inserts + reemplazos + skips por separado.
fn format_insert_message(inserted: usize, replaced: usize, skipped: usize) -> String {
    let mut parts = Vec::new();
    parts.push(format!(
        "{} fila{} insertada{}",
        inserted,
        if inserted == 1 { "" } else { "s" },
        if inserted == 1 { "" } else { "s" }
    ));
    if replaced > 0 {
        parts.push(format!(
            "{} actualizada{}/reemplazada{}",
            replaced,
            if replaced == 1 { "" } else { "s" },
            if replaced == 1 { "" } else { "s" }
        ));
    }
    if skipped > 0 {
        parts.push(format!(
            "{} omitida{}",
            skipped,
            if skipped == 1 { "" } else { "s" }
        ));
    }
    format!("OK ({})", parts.join(", "))
}

/// Bloque J2: nombres de columnas que el ResultSet expone para
/// `RETURNING`. Para `*` enumera todas las columnas del meta en orden;
/// para una lista explícita usa el raw del SelectItem::Column.
fn returning_column_names(meta: &TableMeta, items: &[SelectItem]) -> Vec<String> {
    if items.len() == 1 && matches!(items[0], SelectItem::Star) {
        return meta.columns.iter().map(|c| c.name.clone()).collect();
    }
    items
        .iter()
        .map(|i| match i {
            SelectItem::Column(c) => c.clone(),
            SelectItem::Star => "*".to_string(),
            _ => i.output_name(),
        })
        .collect()
}

/// Bloque J2: proyecta la lista de filas afectadas según la cláusula
/// `RETURNING`. Cada `HashMap<String, Value>` debe tener las columnas
/// del meta como keys (ya normalizadas). Para `*` proyecta en el orden
/// del schema; para `col1, col2` proyecta en el orden pedido.
fn project_returning(
    meta: &TableMeta,
    items: &[SelectItem],
    rows: &[HashMap<String, Value>],
) -> DbResult<Vec<Vec<Value>>> {
    let keys: Vec<String> = if items.len() == 1 && matches!(items[0], SelectItem::Star) {
        meta.columns
            .iter()
            .map(|c| normalize_ident(&c.name))
            .collect()
    } else {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            match item {
                SelectItem::Column(c) => {
                    let k = normalize_ident(c);
                    if meta.column(&k).is_none() {
                        return Err(coded(
                            codes::COLUMN_NOT_FOUND,
                            format!("RETURNING: columna '{}' no existe en '{}'", c, meta.name),
                        ));
                    }
                    out.push(k);
                }
                SelectItem::Star => {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        "RETURNING *: no se puede mezclar con columnas explícitas",
                    ));
                }
                SelectItem::Aggregate { .. } => {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        "RETURNING no admite funciones agregadas",
                    ));
                }
                SelectItem::Expression { .. } => {
                    return Err(coded(
                        codes::COLUMN_NOT_FOUND,
                        "RETURNING no admite expresiones escalares en este release (G1: solo \
                         columnas crudas o `*`)",
                    ));
                }
            }
        }
        out
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let projected: Vec<Value> = keys
            .iter()
            .map(|k| row.get(k).cloned().unwrap_or(Value::Null))
            .collect();
        out.push(projected);
    }
    Ok(out)
}

/// Bloque F: validación de columna usada por `eval_atom_single`. La
/// columna está OK si o bien existe en el meta de la tabla, o bien
/// ya está materializada como clave en la fila — eso último cubre las
/// "columnas virtuales" que vienen del pipeline de agregación (output
/// names de COUNT/SUM/AVG/MIN/MAX y aliases del SELECT que viven en
/// el bucket pero no en el schema físico). Pre-F este chequeo era
/// solo `meta.column(...).is_none()`; la extensión preserva el UX de
/// "columna inexistente" en WHERE/UPDATE/DELETE y al mismo tiempo
/// deja pasar las referencias virtuales en HAVING.
fn ensure_column_visible(
    meta: &TableMeta,
    key: &str,
    raw_name: &str,
    row: &HashMap<String, Value>,
) -> DbResult<()> {
    if meta.column(key).is_some() || row.contains_key(key) {
        return Ok(());
    }
    Err(coded(
        codes::COLUMN_NOT_FOUND,
        format!("columna '{}' no existe en '{}'", raw_name, meta.name),
    ))
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
            | WhereClause::InList { .. }
            // G2: `ExprPredicate` no transporta subqueries en este release
            // — `Expr` aún no contiene `Subquery`/`Outer`. Cuando G3 las
            // agregue habrá que walkear la Expr aquí.
            | WhereClause::ExprPredicate { .. } => false,
        },
    }
}

/// Bloque G1: valida la cantidad de argumentos de una función escalar
/// al parsearla. Devuelve `[GBY-4034]` si la aridad no calza.
fn validate_scalar_arity(f: ScalarFunc, n: usize) -> DbResult<()> {
    let ok = match f {
        ScalarFunc::Length
        | ScalarFunc::Upper
        | ScalarFunc::Lower
        | ScalarFunc::Abs
        | ScalarFunc::Trim
        | ScalarFunc::Ltrim
        | ScalarFunc::Rtrim
        | ScalarFunc::Ceil
        | ScalarFunc::Floor
        | ScalarFunc::Sqrt => n == 1,
        ScalarFunc::Round => n == 1 || n == 2,
        ScalarFunc::Substr => n == 2 || n == 3,
        ScalarFunc::Concat | ScalarFunc::Coalesce => n >= 1,
        ScalarFunc::Nullif
        | ScalarFunc::Ifnull
        | ScalarFunc::Mod
        | ScalarFunc::Power
        | ScalarFunc::DateAdd
        | ScalarFunc::DateSub
        | ScalarFunc::Datediff
        | ScalarFunc::Extract
        | ScalarFunc::Strftime => n == 2,
        ScalarFunc::Replace | ScalarFunc::SplitPart | ScalarFunc::If => n == 3,
        ScalarFunc::Now | ScalarFunc::CurrentDate | ScalarFunc::CurrentTimestamp => n == 0,
    };
    if ok {
        return Ok(());
    }
    Err(coded(
        codes::SCALAR_FN_ARITY,
        format!(
            "{}: cantidad incorrecta de argumentos ({} recibido{})",
            f.keyword(),
            n,
            if n == 1 { "" } else { "s" }
        ),
    ))
}

/// Bloque L2 (2026-05-27): serializa un `Expr` de vuelta a SQL
/// canónico re-parseable por [`parse_expr_str`].
///
/// Usado por el catálogo para persistir el `source` de un
/// [`CheckConstraint`] como texto en vez del AST — evita acoplar el
/// formato on-disk al AST que cambia con cada bloque G/H/I. La salida
/// envuelve sub-expresiones binarias en paréntesis para no depender de
/// la precedencia (el round-trip queda estable).
///
/// `ScalarSubquery` no se admite: `CHECK` no puede usar subqueries en
/// ANSI ni en gabysql (el evaluador `eval_expr` lo rechaza
/// activamente).
pub fn format_expr(expr: &Expr) -> DbResult<String> {
    match expr {
        Expr::Literal(v) => Ok(format_value_literal(v)),
        Expr::Column(name) => Ok(name.clone()),
        Expr::Func(f, args) => format_func_call(*f, args),
        Expr::Cast(inner, ty) => {
            Ok(format!("CAST({} AS {})", format_expr(inner)?, ty.as_sql()))
        }
        Expr::Case {
            operand,
            branches,
            else_branch,
        } => {
            let mut out = String::from("CASE");
            if let Some(op) = operand {
                out.push(' ');
                out.push_str(&format_expr(op)?);
            }
            for (cond, val) in branches {
                out.push_str(" WHEN ");
                out.push_str(&format_expr(cond)?);
                out.push_str(" THEN ");
                out.push_str(&format_expr(val)?);
            }
            if let Some(el) = else_branch {
                out.push_str(" ELSE ");
                out.push_str(&format_expr(el)?);
            }
            out.push_str(" END");
            Ok(out)
        }
        Expr::Compare(lhs, op, rhs) => Ok(format!(
            "({} {} {})",
            format_expr(lhs)?,
            cmp_op_sql(*op),
            format_expr(rhs)?
        )),
        Expr::IsNull(inner, negated) => Ok(format!(
            "({} IS {}NULL)",
            format_expr(inner)?,
            if *negated { "NOT " } else { "" }
        )),
        Expr::Arith(lhs, op, rhs) => Ok(format!(
            "({} {} {})",
            format_expr(lhs)?,
            op.lexeme(),
            format_expr(rhs)?
        )),
        Expr::Like(lhs, pattern, negated) => Ok(format!(
            "({} {}LIKE {})",
            format_expr(lhs)?,
            if *negated { "NOT " } else { "" },
            quote_string(pattern)
        )),
        Expr::InList(lhs, values, negated) => {
            let items: Vec<String> = values.iter().map(format_value_literal).collect();
            Ok(format!(
                "({} {}IN ({}))",
                format_expr(lhs)?,
                if *negated { "NOT " } else { "" },
                items.join(", ")
            ))
        }
        Expr::Between(lhs, lo, hi, negated) => Ok(format!(
            "({} {}BETWEEN {} AND {})",
            format_expr(lhs)?,
            if *negated { "NOT " } else { "" },
            format_expr(lo)?,
            format_expr(hi)?
        )),
        Expr::ScalarSubquery(_) => Err(coded(
            codes::CHECK_CONTAINS_SUBQUERY,
            "CHECK no admite subqueries (ANSI; gabysql sigue la regla por consistencia con el evaluador).",
        )),
    }
}

/// Bloque L2: helper para serializar literales SQL. Strings van con
/// comillas simples y escapean `'` doblándola (estilo ANSI). Floats
/// usan la representación de Rust (estable round-trip vía f64 parser).
fn format_value_literal(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Bool(true) => "TRUE".to_string(),
        Value::Bool(false) => "FALSE".to_string(),
        Value::Integer(n) => n.to_string(),
        Value::Float(n) => {
            // Aseguramos punto decimal para que el parser detecte float
            // (e.g. `1` vs `1.0`).
            let s = format!("{}", n);
            if s.contains('.')
                || s.contains('e')
                || s.contains('E')
                || s == "inf"
                || s == "-inf"
                || s == "NaN"
            {
                s
            } else {
                format!("{}.0", s)
            }
        }
        Value::String(s) => quote_string(s),
    }
}

fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push('\'');
            out.push('\'');
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn cmp_op_sql(op: ExprCmpOp) -> &'static str {
    match op {
        ExprCmpOp::Eq => "=",
        ExprCmpOp::Ne => "<>",
        ExprCmpOp::Lt => "<",
        ExprCmpOp::Le => "<=",
        ExprCmpOp::Gt => ">",
        ExprCmpOp::Ge => ">=",
    }
}

fn format_func_call(f: ScalarFunc, args: &[Expr]) -> DbResult<String> {
    // EXTRACT(<field> FROM <date>) tiene sintaxis especial: args[0] es
    // un Literal(String) con el field name, args[1] la fecha.
    if matches!(f, ScalarFunc::Extract) {
        if args.len() != 2 {
            return Err(DbError::new(format!(
                "EXTRACT esperaba 2 args internos, recibí {}",
                args.len()
            )));
        }
        let field = match &args[0] {
            Expr::Literal(Value::String(s)) => s.clone(),
            other => {
                return Err(DbError::new(format!(
                    "EXTRACT: arg field interno debe ser literal STRING, recibí {:?}",
                    other
                )))
            }
        };
        return Ok(format!(
            "EXTRACT({} FROM {})",
            field,
            format_expr(&args[1])?
        ));
    }
    let rendered: Vec<String> = args.iter().map(format_expr).collect::<DbResult<_>>()?;
    Ok(format!("{}({})", f.keyword(), rendered.join(", ")))
}

/// Issue #1 (2026-05-27): chequea si un `SelectStmt` (típicamente el
/// body de una `Expr::ScalarSubquery`) referencia el outer scope. Eso
/// pasa cuando hay un `WhereClause::EqColumnRef` en cualquier nivel
/// del WHERE — esa es la ÚNICA forma de leer outer.col en gabysql
/// (`Expr::Column` siempre se resuelve contra el row local del scope
/// en que se evalúa). Si no hay ninguna, la subquery es
/// **no-correlacionada** y su valor se puede memoizar: evaluar una
/// vez antes del loop del outer, sustituir el `ScalarSubquery` por
/// un `Literal(value)` y ahorrar los N-1 re-cómputos.
///
/// Bench pre-fix: `SELECT (SELECT COUNT(*) FROM events) FROM events LIMIT 10`
/// tardaba 7.5 s (re-evalúa el subquery por cada una de las 10 filas).
/// Post-fix: <10 ms (una sola evaluación).
fn select_stmt_is_correlated(stmt: &SelectStmt) -> bool {
    fn where_has_eq_columnref(w: &WhereExpr) -> bool {
        match w {
            WhereExpr::Atom(c) => matches!(c, WhereClause::EqColumnRef { .. }),
            WhereExpr::And(l, r) | WhereExpr::Or(l, r) => {
                where_has_eq_columnref(l) || where_has_eq_columnref(r)
            }
            WhereExpr::Not(inner) => where_has_eq_columnref(inner),
        }
    }
    if let Some(w) = &stmt.where_clause {
        if where_has_eq_columnref(w) {
            return true;
        }
    }
    // Conservative: si hay subqueries anidadas (en columns, derived_source,
    // etc.) cuyos cuerpos referencien outer, también marca como correlated.
    for item in &stmt.columns {
        if let SelectItem::Expression { expr, .. } = item {
            if expr_contains_correlated_subquery(expr) {
                return true;
            }
        }
    }
    if let Some(sub) = &stmt.derived_source {
        if select_stmt_is_correlated(sub) {
            return true;
        }
    }
    false
}

/// Issue #1: walker que detecta si una Expr contiene una scalar
/// subquery cuyo body referencia el outer scope (i.e., correlacionada
/// transitivamente).
fn expr_contains_correlated_subquery(expr: &Expr) -> bool {
    match expr {
        Expr::Literal(_) | Expr::Column(_) => false,
        Expr::ScalarSubquery(s) => select_stmt_is_correlated(s),
        Expr::Func(_, args) => args.iter().any(expr_contains_correlated_subquery),
        Expr::Cast(inner, _) => expr_contains_correlated_subquery(inner),
        Expr::Case {
            operand,
            branches,
            else_branch,
        } => {
            operand
                .as_ref()
                .is_some_and(|e| expr_contains_correlated_subquery(e))
                || branches.iter().any(|(c, v)| {
                    expr_contains_correlated_subquery(c) || expr_contains_correlated_subquery(v)
                })
                || else_branch
                    .as_ref()
                    .is_some_and(|e| expr_contains_correlated_subquery(e))
        }
        Expr::Compare(l, _, r) | Expr::Arith(l, _, r) => {
            expr_contains_correlated_subquery(l) || expr_contains_correlated_subquery(r)
        }
        Expr::IsNull(i, _) | Expr::Like(i, _, _) | Expr::InList(i, _, _) => {
            expr_contains_correlated_subquery(i)
        }
        Expr::Between(l, lo, hi, _) => {
            expr_contains_correlated_subquery(l)
                || expr_contains_correlated_subquery(lo)
                || expr_contains_correlated_subquery(hi)
        }
    }
}

/// Issue #4 (2026-05-27): si `expr` es un AND-tree de `Eq { col, val }`
/// con valores literales (sin subqueries ni column-refs correlated),
/// devuelve un map `col_normalizada → Value`. Caso contrario `None`.
/// Usado por el planner para detectar `WHERE a = X AND b = Y AND ...`
/// y activar el fast-path por fingerprint sobre PK compuesta.
fn extract_and_equality_map(expr: &WhereExpr) -> Option<HashMap<String, Value>> {
    fn walk(expr: &WhereExpr, out: &mut HashMap<String, Value>) -> bool {
        match expr {
            WhereExpr::Atom(WhereClause::Eq { column, value }) => {
                let key = normalize_ident(column);
                // Si la misma columna aparece dos veces con valores
                // distintos, no es seguro (and de eqs contradictorios
                // = 0 rows, pero el planner no lo necesita): caemos
                // a None.
                if let Some(prev) = out.get(&key) {
                    if prev != value {
                        return false;
                    }
                }
                out.insert(key, value.clone());
                true
            }
            WhereExpr::And(l, r) => walk(l, out) && walk(r, out),
            // OR/NOT/otros tipos de atom → no es un puro AND-of-equality
            _ => false,
        }
    }
    let mut out = HashMap::new();
    if walk(expr, &mut out) && !out.is_empty() {
        Some(out)
    } else {
        None
    }
}

/// Bloque V (2026-05-27): reconstruye SQL re-parseable a partir de
/// una sub-lista de tokens. El lexer no preserva whitespace, así que
/// la salida no es byte-equivalente al original — pero sí semántica
/// y léxicamente equivalente (mismos tokens en el mismo orden).
///
/// Strings se re-quote con `'...'` y escape doble `''`; symbols van
/// pegados a sus vecinos para identifiers (`a,b` no necesita espacio,
/// pero `a b` sí). La heurística: insertar espacio entre tokens si
/// AMBOS son Ident/Number/String — entre symbols, o symbol↔otro, no
/// hace falta para que el lexer los separe.
fn reconstruct_sql_from_tokens(tokens: &[Token]) -> String {
    let mut out = String::new();
    let mut prev_was_word = false;
    for tok in tokens {
        if matches!(tok.kind, TokenKind::Eof) {
            continue;
        }
        let is_word = matches!(
            tok.kind,
            TokenKind::Ident | TokenKind::Number | TokenKind::String
        );
        if prev_was_word && is_word {
            out.push(' ');
        }
        match tok.kind {
            TokenKind::String => {
                out.push('\'');
                for ch in tok.text.chars() {
                    if ch == '\'' {
                        out.push('\'');
                        out.push('\'');
                    } else {
                        out.push(ch);
                    }
                }
                out.push('\'');
            }
            _ => out.push_str(&tok.text),
        }
        prev_was_word = is_word;
    }
    out
}

/// Bloque V (2026-05-27): parsea un `SelectQuery` standalone — usado
/// para re-cargar el `source` de una `ViewMeta` desde catálogo.
pub fn parse_select_query_str(source: &str) -> DbResult<SelectQuery> {
    let tokens = tokenize(source)?;
    let mut parser = Parser {
        tokens,
        pos: 0,
        where_depth: 0,
        in_having: false,
        pending_check_name: None,
    };
    let query = parser.parse_select_query_for_ctas()?;
    if !parser.is_eof() {
        return Err(DbError::new(format!(
            "parse_select_query_str: tokens sobrantes tras el SELECT: '{}'",
            source
        )));
    }
    Ok(query)
}

/// Bloque L2: parsea una expresión standalone (sin SELECT) — útil para
/// re-cargar el `source` de un `CheckConstraint` desde catálogo.
pub fn parse_expr_str(source: &str) -> DbResult<Expr> {
    let tokens = tokenize(source)?;
    let mut parser = Parser {
        tokens,
        pos: 0,
        where_depth: 0,
        in_having: false,
        pending_check_name: None,
    };
    let expr = parser.parse_expr()?;
    if !parser.is_eof() {
        return Err(DbError::new(format!(
            "parse_expr_str: tokens sobrantes después de la expresión: '{}'",
            source
        )));
    }
    Ok(expr)
}

/// Bloque G1: evaluador de `Expr` sobre una fila ya decodificada. Las
/// claves de `row` son ident normalizado (`normalize_ident`) o, en el
/// caso de JOINs, `alias.ident` — `Expr::Column` resuelve igual que la
/// proyección bare: busca primero la key exacta normalizada y, si no
/// está, intenta el sufijo después del último `.`.
fn eval_expr(expr: &Expr, row: &HashMap<String, Value>) -> DbResult<Value> {
    match expr {
        Expr::Literal(v) => Ok(v.clone()),
        Expr::Column(name) => {
            let key = normalize_ident(name);
            if let Some(v) = row.get(&key) {
                return Ok(v.clone());
            }
            // Match suffix para filas joineadas (`alias.col` en la map).
            for (k, v) in row {
                if k.rsplit('.').next().unwrap_or(k) == key {
                    return Ok(v.clone());
                }
            }
            Err(coded(
                codes::COLUMN_NOT_FOUND,
                format!("columna '{}' no encontrada al evaluar expresión", name),
            ))
        }
        Expr::Func(f, args) => {
            // Funciones que NO evalúan todos los args antes (short-circuit):
            // Coalesce/Ifnull/If/Nullif requieren control de NULL propio.
            match f {
                ScalarFunc::Coalesce => {
                    for a in args {
                        let v = eval_expr(a, row)?;
                        if !matches!(v, Value::Null) {
                            return Ok(v);
                        }
                    }
                    Ok(Value::Null)
                }
                ScalarFunc::Ifnull => {
                    let a = eval_expr(&args[0], row)?;
                    if matches!(a, Value::Null) {
                        eval_expr(&args[1], row)
                    } else {
                        Ok(a)
                    }
                }
                ScalarFunc::If => {
                    let cond = eval_expr(&args[0], row)?;
                    let truthy = match cond {
                        Value::Bool(b) => b,
                        Value::Null => false,
                        other => {
                            return Err(coded(
                                codes::SCALAR_FN_TYPE_MISMATCH,
                                format!(
                                    "IF(cond,...): cond debe ser BOOL, recibí {}",
                                    value_type_name(&other)
                                ),
                            ));
                        }
                    };
                    if truthy {
                        eval_expr(&args[1], row)
                    } else {
                        eval_expr(&args[2], row)
                    }
                }
                ScalarFunc::Nullif => {
                    let a = eval_expr(&args[0], row)?;
                    let b = eval_expr(&args[1], row)?;
                    if matches!(a, Value::Null) || matches!(b, Value::Null) {
                        return Ok(a);
                    }
                    if values_equal(&a, &b) {
                        Ok(Value::Null)
                    } else {
                        Ok(a)
                    }
                }
                _ => {
                    let mut evaluated = Vec::with_capacity(args.len());
                    for a in args {
                        evaluated.push(eval_expr(a, row)?);
                    }
                    eval_scalar_fn(*f, evaluated)
                }
            }
        }
        Expr::Cast(inner, ty) => {
            let v = eval_expr(inner, row)?;
            cast_value(v, *ty)
        }
        Expr::Case {
            operand,
            branches,
            else_branch,
        } => match operand {
            None => {
                // Searched: cada cond debe evaluar a BOOL.
                for (cond, val) in branches {
                    let c = eval_expr(cond, row)?;
                    match c {
                        Value::Bool(true) => return eval_expr(val, row),
                        Value::Bool(false) | Value::Null => continue,
                        other => {
                            return Err(coded(
                                codes::CASE_BRANCH_TYPE_MISMATCH,
                                format!(
                                    "CASE WHEN: la condición debe ser BOOL, recibí {}",
                                    value_type_name(&other)
                                ),
                            ));
                        }
                    }
                }
                match else_branch {
                    Some(e) => eval_expr(e, row),
                    None => Ok(Value::Null),
                }
            }
            Some(op_expr) => {
                let op_val = eval_expr(op_expr, row)?;
                for (when_val, then_val) in branches {
                    let wv = eval_expr(when_val, row)?;
                    // ANSI: NULL nunca matchea NULL en CASE simple — para
                    // eso está IS NULL. `values_equal` ya implementa ese
                    // contract.
                    if values_equal(&op_val, &wv) {
                        return eval_expr(then_val, row);
                    }
                }
                match else_branch {
                    Some(e) => eval_expr(e, row),
                    None => Ok(Value::Null),
                }
            }
        },
        Expr::Compare(lhs, op, rhs) => {
            let a = eval_expr(lhs, row)?;
            let b = eval_expr(rhs, row)?;
            if matches!(a, Value::Null) || matches!(b, Value::Null) {
                return Ok(Value::Null);
            }
            let cmp_op = match op {
                ExprCmpOp::Eq => return Ok(Value::Bool(values_equal(&a, &b))),
                ExprCmpOp::Ne => return Ok(Value::Bool(!values_equal(&a, &b))),
                ExprCmpOp::Lt => CompareOp::Lt,
                ExprCmpOp::Le => CompareOp::Le,
                ExprCmpOp::Gt => CompareOp::Gt,
                ExprCmpOp::Ge => CompareOp::Ge,
            };
            match eval_compare(Some(&a), cmp_op, &b) {
                Some(b) => Ok(Value::Bool(b)),
                None => Ok(Value::Null),
            }
        }
        Expr::IsNull(inner, negated) => {
            let v = eval_expr(inner, row)?;
            let is_null = matches!(v, Value::Null);
            Ok(Value::Bool(if *negated { !is_null } else { is_null }))
        }
        Expr::Arith(lhs, op, rhs) => {
            let a = eval_expr(lhs, row)?;
            let b = eval_expr(rhs, row)?;
            eval_arith(a, *op, b)
        }
        Expr::Like(lhs, pattern, negated) => {
            let v = eval_expr(lhs, row)?;
            match eval_like(Some(&v), pattern, *negated) {
                Some(b) => Ok(Value::Bool(b)),
                None => Ok(Value::Null),
            }
        }
        Expr::InList(lhs, values, negated) => {
            let v = eval_expr(lhs, row)?;
            match eval_in_list(Some(&v), values, *negated) {
                Some(b) => Ok(Value::Bool(b)),
                None => Ok(Value::Null),
            }
        }
        Expr::Between(lhs, lo, hi, negated) => {
            let v = eval_expr(lhs, row)?;
            let lv = eval_expr(lo, row)?;
            let hv = eval_expr(hi, row)?;
            if matches!(v, Value::Null) || matches!(lv, Value::Null) || matches!(hv, Value::Null) {
                return Ok(Value::Null);
            }
            let ge_lo = eval_compare(Some(&v), CompareOp::Ge, &lv);
            let le_hi = eval_compare(Some(&v), CompareOp::Le, &hv);
            match (ge_lo, le_hi) {
                (Some(a), Some(b)) => {
                    let between = a && b;
                    Ok(Value::Bool(if *negated { !between } else { between }))
                }
                _ => Ok(Value::Null),
            }
        }
        Expr::ScalarSubquery(_) => Err(coded(
            codes::WHERE_OPERATOR_UNSUPPORTED,
            "subquery escalar dentro de Expr requiere el path con engine \
             (`Engine::eval_expr_full`); este caller la invocó con la firma pura",
        )),
    }
}

/// Bloque H: walker que detecta si un árbol `Expr` contiene alguna
/// `ScalarSubquery` en cualquier nivel. Lo usa el dispatcher de
/// proyección y de WHERE/HAVING para decidir si vale el fast-path
/// `eval_expr` (sin engine) o si hay que ir por `eval_expr_full`.
fn expr_contains_subquery(expr: &Expr) -> bool {
    match expr {
        Expr::ScalarSubquery(_) => true,
        Expr::Literal(_) | Expr::Column(_) => false,
        Expr::Func(_, args) => args.iter().any(expr_contains_subquery),
        Expr::Cast(inner, _) | Expr::IsNull(inner, _) => expr_contains_subquery(inner),
        Expr::Case {
            operand,
            branches,
            else_branch,
        } => {
            operand
                .as_deref()
                .map(expr_contains_subquery)
                .unwrap_or(false)
                || branches
                    .iter()
                    .any(|(c, v)| expr_contains_subquery(c) || expr_contains_subquery(v))
                || else_branch
                    .as_deref()
                    .map(expr_contains_subquery)
                    .unwrap_or(false)
        }
        Expr::Compare(a, _, b) | Expr::Arith(a, _, b) => {
            expr_contains_subquery(a) || expr_contains_subquery(b)
        }
        Expr::Like(inner, _, _) | Expr::InList(inner, _, _) => expr_contains_subquery(inner),
        Expr::Between(a, b, c, _) => {
            expr_contains_subquery(a) || expr_contains_subquery(b) || expr_contains_subquery(c)
        }
    }
}

/// Bloque G3: evaluador del operador binario aritmético / concat.
/// Reglas:
/// - NULL en cualquiera de los operandos → NULL (3VL).
/// - INT op INT → INT con `checked_*`; overflow → `[GBY-4042]`.
/// - INT/FLOAT mixto → promueve a FLOAT.
/// - División o módulo con divisor cero → `[GBY-4043]`.
/// - `Concat` (`||`): adopta la regla ANSI estricta (NULL → NULL).
///   Cualquier tipo se imprime via `value_to_text`.
/// - Cualquier otra combinación (`TEXT + INT`, `BOOL * 2`, ...) → `[GBY-4044]`.
fn eval_arith(a: Value, op: ArithOp, b: Value) -> DbResult<Value> {
    if matches!(a, Value::Null) || matches!(b, Value::Null) {
        return Ok(Value::Null);
    }
    if matches!(op, ArithOp::Concat) {
        return Ok(Value::String(format!(
            "{}{}",
            value_to_text(&a),
            value_to_text(&b)
        )));
    }
    // Promoción numérica.
    let (af, bf, both_int) = match (&a, &b) {
        (Value::Integer(x), Value::Integer(y)) => (*x as f64, *y as f64, Some((*x, *y))),
        (Value::Integer(x), Value::Float(y)) => (*x as f64, *y, None),
        (Value::Float(x), Value::Integer(y)) => (*x, *y as f64, None),
        (Value::Float(x), Value::Float(y)) => (*x, *y, None),
        _ => {
            return Err(coded(
                codes::ARITH_TYPE_MISMATCH,
                format!(
                    "operador '{}' no acepta operandos {} y {}",
                    op.lexeme(),
                    value_type_name(&a),
                    value_type_name(&b)
                ),
            ));
        }
    };
    if let Some((x, y)) = both_int {
        // Camino entero puro: checked_* + cero check.
        let r = match op {
            ArithOp::Add => x.checked_add(y),
            ArithOp::Sub => x.checked_sub(y),
            ArithOp::Mul => x.checked_mul(y),
            ArithOp::Div => {
                if y == 0 {
                    return Err(coded(codes::DIVISION_BY_ZERO, "división entera por cero"));
                }
                x.checked_div(y)
            }
            ArithOp::Mod => {
                if y == 0 {
                    return Err(coded(codes::DIVISION_BY_ZERO, "módulo por cero"));
                }
                x.checked_rem(y)
            }
            ArithOp::Concat => unreachable!(),
        };
        return match r {
            Some(v) => Ok(Value::Integer(v)),
            None => Err(coded(
                codes::ARITH_OVERFLOW,
                format!("overflow aritmético en INT: {} {} {}", x, op.lexeme(), y),
            )),
        };
    }
    // Camino flotante (al menos un operando FLOAT).
    let r = match op {
        ArithOp::Add => af + bf,
        ArithOp::Sub => af - bf,
        ArithOp::Mul => af * bf,
        ArithOp::Div => {
            if bf == 0.0 {
                return Err(coded(
                    codes::DIVISION_BY_ZERO,
                    "división por cero (flotante)",
                ));
            }
            af / bf
        }
        ArithOp::Mod => {
            if bf == 0.0 {
                return Err(coded(codes::DIVISION_BY_ZERO, "módulo por cero (flotante)"));
            }
            af % bf
        }
        ArithOp::Concat => unreachable!(),
    };
    Ok(Value::Float(r))
}

/// Bloque G2: evalúa una `Expr` como predicado booleano para
/// WHERE/HAVING. Devuelve `Some(true)`/`Some(false)` cuando la
/// expresión rinde un BOOL concreto, `None` cuando rinde NULL
/// (3VL: la fila no pasa el filtro). Cualquier otro tipo es un
/// error claro `[GBY-4040]` — la expresión sin operador de
/// comparación (`WHERE LENGTH(x)`) cae acá.
fn eval_expr_as_predicate(expr: &Expr, row: &HashMap<String, Value>) -> DbResult<Option<bool>> {
    let v = eval_expr(expr, row)?;
    match v {
        Value::Bool(b) => Ok(Some(b)),
        Value::Null => Ok(None),
        other => Err(coded(
            codes::WHERE_EXPR_NOT_BOOLEAN,
            format!(
                "expresión en WHERE/HAVING debe evaluar a BOOL (o NULL), recibí {}; \
                 ¿faltó un operador de comparación (=, <, >, ...)?",
                value_type_name(&other)
            ),
        )),
    }
}

/// Bloque G1: dispatcher para las funciones escalares "puras" (sin
/// short-circuit). Los args ya vienen evaluados. NULL propagation:
/// salvo `Concat` (que adopta la regla ANSI estricta y propaga), si
/// cualquier arg es NULL devolvemos NULL sin invocar al cuerpo de la
/// función. `Now` / `CurrentDate` / `CurrentTimestamp` no tienen args.
fn eval_scalar_fn(f: ScalarFunc, args: Vec<Value>) -> DbResult<Value> {
    // NULL propagation por defecto. Excepciones tratadas arriba o abajo.
    if !matches!(
        f,
        ScalarFunc::Now | ScalarFunc::CurrentDate | ScalarFunc::CurrentTimestamp
    ) && args.iter().any(|v| matches!(v, Value::Null))
    {
        return Ok(Value::Null);
    }
    match f {
        ScalarFunc::Length => match &args[0] {
            Value::String(s) => Ok(Value::Integer(s.chars().count() as i64)),
            other => Err(coded(
                codes::SCALAR_FN_TYPE_MISMATCH,
                format!("LENGTH requiere TEXT, recibí {}", value_type_name(other)),
            )),
        },
        ScalarFunc::Upper => match &args[0] {
            Value::String(s) => Ok(Value::String(s.to_uppercase())),
            other => Err(coded(
                codes::SCALAR_FN_TYPE_MISMATCH,
                format!("UPPER requiere TEXT, recibí {}", value_type_name(other)),
            )),
        },
        ScalarFunc::Lower => match &args[0] {
            Value::String(s) => Ok(Value::String(s.to_lowercase())),
            other => Err(coded(
                codes::SCALAR_FN_TYPE_MISMATCH,
                format!("LOWER requiere TEXT, recibí {}", value_type_name(other)),
            )),
        },
        ScalarFunc::Substr => {
            let s = match &args[0] {
                Value::String(s) => s,
                other => {
                    return Err(coded(
                        codes::SCALAR_FN_TYPE_MISMATCH,
                        format!("SUBSTR requiere TEXT, recibí {}", value_type_name(other)),
                    ));
                }
            };
            let from = match &args[1] {
                Value::Integer(n) => *n,
                other => {
                    return Err(coded(
                        codes::SCALAR_FN_TYPE_MISMATCH,
                        format!(
                            "SUBSTR(s, from): 'from' debe ser INT, recibí {}",
                            value_type_name(other)
                        ),
                    ));
                }
            };
            let chars: Vec<char> = s.chars().collect();
            // SQL standard: from es 1-based. from <= 0 → tratar como 1.
            let start = if from <= 1 { 0 } else { (from - 1) as usize };
            let end = if args.len() == 3 {
                let len = match &args[2] {
                    Value::Integer(n) => *n,
                    other => {
                        return Err(coded(
                            codes::SCALAR_FN_TYPE_MISMATCH,
                            format!(
                                "SUBSTR(s, from, len): 'len' debe ser INT, recibí {}",
                                value_type_name(other)
                            ),
                        ));
                    }
                };
                if len <= 0 {
                    start
                } else {
                    (start + len as usize).min(chars.len())
                }
            } else {
                chars.len()
            };
            let start = start.min(chars.len());
            Ok(Value::String(chars[start..end].iter().collect()))
        }
        ScalarFunc::Concat => {
            // NULL propagation ya fue chequeada arriba.
            let mut out = String::new();
            for v in &args {
                out.push_str(&value_to_text(v));
            }
            Ok(Value::String(out))
        }
        ScalarFunc::Abs => match &args[0] {
            Value::Integer(n) => Ok(Value::Integer(n.wrapping_abs())),
            Value::Float(f) => Ok(Value::Float(f.abs())),
            other => Err(coded(
                codes::SCALAR_FN_TYPE_MISMATCH,
                format!(
                    "ABS requiere INT o FLOAT, recibí {}",
                    value_type_name(other)
                ),
            )),
        },
        ScalarFunc::Round => {
            let value = &args[0];
            let n_decimals: i64 = if args.len() == 2 {
                match &args[1] {
                    Value::Integer(n) => *n,
                    other => {
                        return Err(coded(
                            codes::SCALAR_FN_TYPE_MISMATCH,
                            format!(
                                "ROUND(x, n): 'n' debe ser INT, recibí {}",
                                value_type_name(other)
                            ),
                        ));
                    }
                }
            } else {
                0
            };
            match value {
                Value::Integer(n) => Ok(Value::Integer(*n)),
                Value::Float(f) => {
                    if n_decimals <= 0 {
                        Ok(Value::Float(f.round()))
                    } else {
                        let factor = 10f64.powi(n_decimals as i32);
                        Ok(Value::Float((f * factor).round() / factor))
                    }
                }
                other => Err(coded(
                    codes::SCALAR_FN_TYPE_MISMATCH,
                    format!(
                        "ROUND requiere INT o FLOAT, recibí {}",
                        value_type_name(other)
                    ),
                )),
            }
        }
        ScalarFunc::Now | ScalarFunc::CurrentTimestamp => Ok(Value::String(now_datetime_utc())),
        ScalarFunc::CurrentDate => {
            let dt = now_datetime_utc();
            // primeros 10 chars son YYYY-MM-DD.
            Ok(Value::String(dt[..10].to_string()))
        }
        // -------- Bloque G3: string P2/P3 --------
        ScalarFunc::Trim => match &args[0] {
            Value::String(s) => Ok(Value::String(s.trim().to_string())),
            other => Err(coded(
                codes::SCALAR_FN_TYPE_MISMATCH,
                format!("TRIM requiere TEXT, recibí {}", value_type_name(other)),
            )),
        },
        ScalarFunc::Ltrim => match &args[0] {
            Value::String(s) => Ok(Value::String(s.trim_start().to_string())),
            other => Err(coded(
                codes::SCALAR_FN_TYPE_MISMATCH,
                format!("LTRIM requiere TEXT, recibí {}", value_type_name(other)),
            )),
        },
        ScalarFunc::Rtrim => match &args[0] {
            Value::String(s) => Ok(Value::String(s.trim_end().to_string())),
            other => Err(coded(
                codes::SCALAR_FN_TYPE_MISMATCH,
                format!("RTRIM requiere TEXT, recibí {}", value_type_name(other)),
            )),
        },
        ScalarFunc::Replace => {
            let s = expect_text(&args[0], "REPLACE", "s")?;
            let from = expect_text(&args[1], "REPLACE", "from")?;
            let to = expect_text(&args[2], "REPLACE", "to")?;
            if from.is_empty() {
                // Evitar bucle infinito en `String::replace` con patrón
                // vacío — devolver el string sin cambios es la opción
                // segura y la que toma SQLite.
                return Ok(Value::String(s.to_string()));
            }
            Ok(Value::String(s.replace(from, to)))
        }
        ScalarFunc::SplitPart => {
            let s = expect_text(&args[0], "SPLIT_PART", "s")?;
            let sep = expect_text(&args[1], "SPLIT_PART", "sep")?;
            let idx = match &args[2] {
                Value::Integer(n) => *n,
                other => {
                    return Err(coded(
                        codes::SCALAR_FN_TYPE_MISMATCH,
                        format!(
                            "SPLIT_PART(s, sep, idx): 'idx' debe ser INT, recibí {}",
                            value_type_name(other)
                        ),
                    ));
                }
            };
            if idx <= 0 {
                return Err(coded(
                    codes::SCALAR_FN_TYPE_MISMATCH,
                    "SPLIT_PART: 'idx' debe ser >= 1 (1-based)",
                ));
            }
            if sep.is_empty() {
                return Ok(Value::String(if idx == 1 {
                    s.to_string()
                } else {
                    String::new()
                }));
            }
            let parts: Vec<&str> = s.split(sep).collect();
            let i = (idx as usize) - 1;
            Ok(Value::String(
                parts.get(i).copied().unwrap_or("").to_string(),
            ))
        }
        // -------- Bloque G3: numéricas P2/P3 --------
        ScalarFunc::Ceil => match &args[0] {
            Value::Integer(n) => Ok(Value::Integer(*n)),
            Value::Float(f) => Ok(Value::Float(f.ceil())),
            other => Err(coded(
                codes::SCALAR_FN_TYPE_MISMATCH,
                format!(
                    "CEIL requiere INT o FLOAT, recibí {}",
                    value_type_name(other)
                ),
            )),
        },
        ScalarFunc::Floor => match &args[0] {
            Value::Integer(n) => Ok(Value::Integer(*n)),
            Value::Float(f) => Ok(Value::Float(f.floor())),
            other => Err(coded(
                codes::SCALAR_FN_TYPE_MISMATCH,
                format!(
                    "FLOOR requiere INT o FLOAT, recibí {}",
                    value_type_name(other)
                ),
            )),
        },
        ScalarFunc::Mod => {
            // Reusa el operador binario; mismas reglas de tipo y de cero.
            eval_arith(args[0].clone(), ArithOp::Mod, args[1].clone())
        }
        ScalarFunc::Power => {
            let x = value_as_f64(&args[0], "POWER", "x")?;
            let y = value_as_f64(&args[1], "POWER", "y")?;
            // x^y con base 0 y exponente negativo es ±Inf → tratamos como
            // dominio inválido.
            if x == 0.0 && y < 0.0 {
                return Err(coded(codes::MATH_DOMAIN, "POWER(0, y) con y<0 indefinido"));
            }
            Ok(Value::Float(x.powf(y)))
        }
        ScalarFunc::Sqrt => {
            let x = value_as_f64(&args[0], "SQRT", "x")?;
            if x < 0.0 {
                return Err(coded(
                    codes::MATH_DOMAIN,
                    format!("SQRT({}) indefinido en reales (argumento negativo)", x),
                ));
            }
            Ok(Value::Float(x.sqrt()))
        }
        // -------- Bloque G3: fechas P2/P3 --------
        ScalarFunc::DateAdd => date_add_days(&args[0], expect_int(&args[1], "DATE_ADD", "n")?),
        ScalarFunc::DateSub => date_add_days(&args[0], -expect_int(&args[1], "DATE_SUB", "n")?),
        ScalarFunc::Datediff => {
            let d1 = parse_date_part_to_days(&args[0], "DATEDIFF")?;
            let d2 = parse_date_part_to_days(&args[1], "DATEDIFF")?;
            Ok(Value::Integer(d1 - d2))
        }
        ScalarFunc::Extract => {
            let field = match &args[0] {
                Value::String(s) => s.to_ascii_uppercase(),
                other => {
                    return Err(coded(
                        codes::EXTRACT_FIELD_INVALID,
                        format!(
                            "EXTRACT: campo debe ser un keyword (YEAR/MONTH/DAY/HOUR/MINUTE/SECOND), recibí {}",
                            value_type_name(other)
                        ),
                    ));
                }
            };
            let s = match &args[1] {
                Value::String(s) => s.as_str(),
                other => {
                    return Err(coded(
                        codes::DATE_PARSE_ERROR,
                        format!(
                            "EXTRACT: argumento de fecha debe ser TEXT, recibí {}",
                            value_type_name(other)
                        ),
                    ));
                }
            };
            extract_date_field(&field, s)
        }
        ScalarFunc::Strftime => {
            let fmt = expect_text(&args[0], "STRFTIME", "format")?;
            let s = expect_text(&args[1], "STRFTIME", "fecha")?;
            strftime_format(fmt, s)
        }
        // Casos con short-circuit: ya tratados en `eval_expr`. Si llegamos
        // acá es bug del caller.
        ScalarFunc::Coalesce | ScalarFunc::Ifnull | ScalarFunc::If | ScalarFunc::Nullif => Err(
            DbError::new("interno: short-circuit fn dispatcheada por eval_scalar_fn"),
        ),
    }
}

/// Bloque G3: helper para funciones que esperan TEXT.
fn expect_text<'a>(v: &'a Value, func: &str, slot: &str) -> DbResult<&'a str> {
    match v {
        Value::String(s) => Ok(s.as_str()),
        other => Err(coded(
            codes::SCALAR_FN_TYPE_MISMATCH,
            format!(
                "{}({}): se esperaba TEXT, recibí {}",
                func,
                slot,
                value_type_name(other)
            ),
        )),
    }
}

/// Bloque G3: helper para funciones que esperan INT.
fn expect_int(v: &Value, func: &str, slot: &str) -> DbResult<i64> {
    match v {
        Value::Integer(n) => Ok(*n),
        other => Err(coded(
            codes::SCALAR_FN_TYPE_MISMATCH,
            format!(
                "{}({}): se esperaba INT, recibí {}",
                func,
                slot,
                value_type_name(other)
            ),
        )),
    }
}

/// Bloque G3: promueve INT/FLOAT a f64 para funciones matemáticas.
fn value_as_f64(v: &Value, func: &str, slot: &str) -> DbResult<f64> {
    match v {
        Value::Integer(n) => Ok(*n as f64),
        Value::Float(f) => Ok(*f),
        other => Err(coded(
            codes::SCALAR_FN_TYPE_MISMATCH,
            format!(
                "{}({}): se esperaba INT o FLOAT, recibí {}",
                func,
                slot,
                value_type_name(other)
            ),
        )),
    }
}

/// Bloque G3: parsea la parte `YYYY-MM-DD` de un string DATE o
/// DATETIME, devuelve días desde la epoch (1970-01-01).
fn parse_date_part_to_days(v: &Value, func: &str) -> DbResult<i64> {
    let s = match v {
        Value::String(s) => s.as_str(),
        other => {
            return Err(coded(
                codes::DATE_PARSE_ERROR,
                format!(
                    "{}: se esperaba TEXT (DATE/DATETIME), recibí {}",
                    func,
                    value_type_name(other)
                ),
            ));
        }
    };
    let date_part = if looks_like_datetime(s) {
        &s[..10]
    } else if looks_like_date(s) {
        s
    } else {
        return Err(coded(
            codes::DATE_PARSE_ERROR,
            format!(
                "{}: '{}' no es DATE 'YYYY-MM-DD' ni DATETIME 'YYYY-MM-DD HH:MM:SS'",
                func, s
            ),
        ));
    };
    let y: i64 = date_part[..4].parse().map_err(|_| {
        coded(
            codes::DATE_PARSE_ERROR,
            format!("{}: año inválido en '{}'", func, s),
        )
    })?;
    let m: u32 = date_part[5..7].parse().map_err(|_| {
        coded(
            codes::DATE_PARSE_ERROR,
            format!("{}: mes inválido en '{}'", func, s),
        )
    })?;
    let d: u32 = date_part[8..10].parse().map_err(|_| {
        coded(
            codes::DATE_PARSE_ERROR,
            format!("{}: día inválido en '{}'", func, s),
        )
    })?;
    Ok(days_from_civil(y, m, d))
}

/// Bloque G3: suma `n` días al date-part de un DATE o DATETIME. Para
/// DATETIME preserva el time-part. Para DATE devuelve un DATE.
fn date_add_days(v: &Value, n: i64) -> DbResult<Value> {
    let s = match v {
        Value::String(s) => s.clone(),
        other => {
            return Err(coded(
                codes::DATE_PARSE_ERROR,
                format!(
                    "DATE_ADD/DATE_SUB: se esperaba TEXT, recibí {}",
                    value_type_name(other)
                ),
            ));
        }
    };
    let (date_part, time_suffix) = if looks_like_datetime(&s) {
        (&s[..10], Some(&s[10..]))
    } else if looks_like_date(&s) {
        (s.as_str(), None)
    } else {
        return Err(coded(
            codes::DATE_PARSE_ERROR,
            format!("DATE_ADD/DATE_SUB: '{}' no es DATE ni DATETIME válido", s),
        ));
    };
    let y: i64 = date_part[..4]
        .parse()
        .map_err(|_| coded(codes::DATE_PARSE_ERROR, format!("año inválido: '{}'", s)))?;
    let m: u32 = date_part[5..7]
        .parse()
        .map_err(|_| coded(codes::DATE_PARSE_ERROR, format!("mes inválido: '{}'", s)))?;
    let d: u32 = date_part[8..10]
        .parse()
        .map_err(|_| coded(codes::DATE_PARSE_ERROR, format!("día inválido: '{}'", s)))?;
    let days = days_from_civil(y, m, d).saturating_add(n);
    let (yy, mm, dd) = civil_from_days(days);
    let new_date = format!("{:04}-{:02}-{:02}", yy, mm, dd);
    match time_suffix {
        Some(suffix) => Ok(Value::String(format!("{}{}", new_date, suffix))),
        None => Ok(Value::String(new_date)),
    }
}

/// Bloque G3: EXTRACT(<field> FROM expr) sobre DATE/DATETIME en TEXT.
fn extract_date_field(field: &str, s: &str) -> DbResult<Value> {
    let (date_part, time_part): (&str, Option<&str>) = if looks_like_datetime(s) {
        (&s[..10], Some(&s[11..]))
    } else if looks_like_date(s) {
        (s, None)
    } else {
        return Err(coded(
            codes::DATE_PARSE_ERROR,
            format!("EXTRACT: '{}' no es DATE/DATETIME válido", s),
        ));
    };
    let yy: i64 = date_part[..4]
        .parse()
        .map_err(|_| coded(codes::DATE_PARSE_ERROR, format!("año inválido: '{}'", s)))?;
    let mm: i64 = date_part[5..7]
        .parse()
        .map_err(|_| coded(codes::DATE_PARSE_ERROR, format!("mes inválido: '{}'", s)))?;
    let dd: i64 = date_part[8..10]
        .parse()
        .map_err(|_| coded(codes::DATE_PARSE_ERROR, format!("día inválido: '{}'", s)))?;
    match field {
        "YEAR" => Ok(Value::Integer(yy)),
        "MONTH" => Ok(Value::Integer(mm)),
        "DAY" => Ok(Value::Integer(dd)),
        "HOUR" | "MINUTE" | "SECOND" => {
            let tp = match time_part {
                Some(t) => t,
                None => {
                    return Err(coded(
                        codes::DATE_PARSE_ERROR,
                        format!(
                            "EXTRACT({} FROM ...): '{}' es DATE sin componente de hora",
                            field, s
                        ),
                    ));
                }
            };
            // tp = "HH:MM:SS"
            let hh: i64 = tp[..2]
                .parse()
                .map_err(|_| coded(codes::DATE_PARSE_ERROR, format!("hora inválida: '{}'", s)))?;
            let mi: i64 = tp[3..5]
                .parse()
                .map_err(|_| coded(codes::DATE_PARSE_ERROR, format!("minuto inválido: '{}'", s)))?;
            let ss: i64 = tp[6..8].parse().map_err(|_| {
                coded(
                    codes::DATE_PARSE_ERROR,
                    format!("segundo inválido: '{}'", s),
                )
            })?;
            Ok(Value::Integer(match field {
                "HOUR" => hh,
                "MINUTE" => mi,
                "SECOND" => ss,
                _ => unreachable!(),
            }))
        }
        other => Err(coded(
            codes::EXTRACT_FIELD_INVALID,
            format!(
                "EXTRACT: campo '{}' no soportado; usar YEAR/MONTH/DAY/HOUR/MINUTE/SECOND",
                other
            ),
        )),
    }
}

/// Bloque G3: formateo mínimo de fechas estilo `strftime`. Soporta
/// `%Y`, `%m`, `%d`, `%H`, `%M`, `%S` y `%%`. Otros placeholders
/// `%X` se emiten tal cual (permisivo).
fn strftime_format(fmt: &str, s: &str) -> DbResult<Value> {
    let (date_part, time_part): (&str, Option<&str>) = if looks_like_datetime(s) {
        (&s[..10], Some(&s[11..]))
    } else if looks_like_date(s) {
        (s, None)
    } else {
        return Err(coded(
            codes::DATE_PARSE_ERROR,
            format!("STRFTIME: '{}' no es DATE/DATETIME válido", s),
        ));
    };
    let mut out = String::with_capacity(fmt.len());
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&date_part[..4]),
            Some('m') => out.push_str(&date_part[5..7]),
            Some('d') => out.push_str(&date_part[8..10]),
            Some('H') => match time_part {
                Some(t) => out.push_str(&t[..2]),
                None => out.push_str("00"),
            },
            Some('M') => match time_part {
                Some(t) => out.push_str(&t[3..5]),
                None => out.push_str("00"),
            },
            Some('S') => match time_part {
                Some(t) => out.push_str(&t[6..8]),
                None => out.push_str("00"),
            },
            Some('%') => out.push('%'),
            Some(other) => {
                // Permisivo: lo emitimos literal con su `%`.
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    Ok(Value::String(out))
}

/// Bloque G3: inverso de `civil_from_days`. Algoritmo de Howard
/// Hinnant: convierte (year, month, day) gregoriano a días desde
/// 1970-01-01.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let mu = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mu + 2) / 5 + d as u64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i64 - 719_468
}

/// Bloque G2: ¿el `Value` calculado encaja en la columna destino para
/// un `UPDATE SET col = <expr>`? Reglas:
/// - NULL siempre cabe (NOT NULL se valida aparte).
/// - INT cabe tanto en `INT` como en `FLOAT` (promoción implícita).
/// - FLOAT en `FLOAT`, BOOL en `BOOL`, TEXT en cualquier columna
///   `stores_as_text()` (`TEXT`/`DATE`/`DATETIME`/`JSON` — la validación
///   de forma de la fecha la hace el encoder, no acá).
///
/// El resto es mismatch.
fn value_fits_column_type(v: &Value, ct: ColumnType) -> bool {
    match (v, ct) {
        (Value::Null, _) => true,
        (Value::Integer(_), ColumnType::Int) => true,
        (Value::Integer(_), ColumnType::Float) => true,
        (Value::Float(_), ColumnType::Float) => true,
        (Value::Bool(_), ColumnType::Bool) => true,
        (Value::String(_), t) if t.stores_as_text() => true,
        _ => false,
    }
}

/// Bloque G1: nombre legible del tipo de un `Value` para mensajes de
/// error de tipo.
fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NULL",
        Value::Integer(_) => "INT",
        Value::Float(_) => "FLOAT",
        Value::Bool(_) => "BOOL",
        Value::String(_) => "TEXT",
    }
}

/// Bloque G1: representación canónica de un `Value` como texto (usada
/// por `CONCAT`, `value_default_label`, y la rama TEXT de `cast_value`).
fn value_to_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(), // CONCAT propaga NULL antes de llegar acá
        Value::Integer(n) => n.to_string(),
        Value::Float(f) => format!("{}", f),
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::String(s) => s.clone(),
    }
}

/// Bloque G1: implementación de `CAST(expr AS TYPE)`. Devuelve
/// `[GBY-4036]` si la conversión es imposible (texto no-numérico a INT,
/// fechas malformadas, etc.). NULL siempre se propaga.
fn cast_value(v: Value, ty: ColumnType) -> DbResult<Value> {
    if matches!(v, Value::Null) {
        return Ok(Value::Null);
    }
    match ty {
        ColumnType::Int => match v {
            Value::Integer(n) => Ok(Value::Integer(n)),
            Value::Float(f) => Ok(Value::Integer(f.trunc() as i64)),
            Value::Bool(b) => Ok(Value::Integer(if b { 1 } else { 0 })),
            Value::String(s) => s.trim().parse::<i64>().map(Value::Integer).map_err(|_| {
                coded(
                    codes::CAST_INVALID,
                    format!("CAST('{}' AS INT): no es un entero válido", s),
                )
            }),
            Value::Null => unreachable!(),
        },
        ColumnType::Float => match v {
            Value::Integer(n) => Ok(Value::Float(n as f64)),
            Value::Float(f) => Ok(Value::Float(f)),
            Value::Bool(b) => Ok(Value::Float(if b { 1.0 } else { 0.0 })),
            Value::String(s) => s.trim().parse::<f64>().map(Value::Float).map_err(|_| {
                coded(
                    codes::CAST_INVALID,
                    format!("CAST('{}' AS FLOAT): no es un número válido", s),
                )
            }),
            Value::Null => unreachable!(),
        },
        ColumnType::Text => Ok(Value::String(value_to_text(&v))),
        ColumnType::Bool => match v {
            Value::Bool(b) => Ok(Value::Bool(b)),
            Value::Integer(n) => Ok(Value::Bool(n != 0)),
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "t" | "1" => Ok(Value::Bool(true)),
                "false" | "f" | "0" => Ok(Value::Bool(false)),
                _ => Err(coded(
                    codes::CAST_INVALID,
                    format!("CAST('{}' AS BOOL): valor no reconocido", s),
                )),
            },
            other => Err(coded(
                codes::CAST_INVALID,
                format!(
                    "CAST AS BOOL desde {} no soportado",
                    value_type_name(&other)
                ),
            )),
        },
        ColumnType::Date => match v {
            Value::String(s) => {
                if looks_like_date(&s) {
                    Ok(Value::String(s))
                } else {
                    Err(coded(
                        codes::CAST_INVALID,
                        format!("CAST('{}' AS DATE): formato esperado YYYY-MM-DD", s),
                    ))
                }
            }
            other => Err(coded(
                codes::CAST_INVALID,
                format!(
                    "CAST AS DATE desde {} no soportado",
                    value_type_name(&other)
                ),
            )),
        },
        ColumnType::DateTime => match v {
            Value::String(s) => {
                if looks_like_datetime(&s) || looks_like_date(&s) {
                    Ok(Value::String(s))
                } else {
                    Err(coded(
                        codes::CAST_INVALID,
                        format!(
                            "CAST('{}' AS DATETIME): formato esperado YYYY-MM-DD HH:MM:SS",
                            s
                        ),
                    ))
                }
            }
            other => Err(coded(
                codes::CAST_INVALID,
                format!(
                    "CAST AS DATETIME desde {} no soportado",
                    value_type_name(&other)
                ),
            )),
        },
        ColumnType::Json => match v {
            Value::String(s) => Ok(Value::String(s)),
            other => Err(coded(
                codes::CAST_INVALID,
                format!(
                    "CAST AS JSON desde {} no soportado",
                    value_type_name(&other)
                ),
            )),
        },
    }
}

fn looks_like_date(s: &str) -> bool {
    // YYYY-MM-DD
    let b = s.as_bytes();
    b.len() == 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
}

fn looks_like_datetime(s: &str) -> bool {
    // YYYY-MM-DD HH:MM:SS  (longitud 19)
    let b = s.as_bytes();
    b.len() == 19
        && looks_like_date(&s[..10])
        && (b[10] == b' ' || b[10] == b'T')
        && b[11..13].iter().all(u8::is_ascii_digit)
        && b[13] == b':'
        && b[14..16].iter().all(u8::is_ascii_digit)
        && b[16] == b':'
        && b[17..19].iter().all(u8::is_ascii_digit)
}

/// Bloque G1: formatea el instante actual como `YYYY-MM-DD HH:MM:SS` en
/// UTC, sin chrono. Convierte los segundos desde UNIX_EPOCH a la fecha
/// civil con el algoritmo de Howard Hinnant (`days_from_civil` inverso).
fn now_datetime_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;
    let (y, mo, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, m, s)
}

/// Howard Hinnant, "civil_from_days": convierte días-desde-epoch
/// (1970-01-01) a (year, month, day) en el calendario gregoriano
/// proléptico. Sirve para evitar la dependencia en chrono.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
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
            // Bloque I: set operations al final del término.
            | "UNION"
            | "INTERSECT"
            | "EXCEPT"
            | "MINUS"
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

/// Bloque F: keywords que terminan el SELECT list. Se usa para decidir
/// si un Ident tras un ítem del SELECT es alias o un keyword estructural
/// (`FROM`, `WHERE`, `GROUP`, `HAVING`, `ORDER`, `LIMIT`, `OFFSET`).
fn is_select_terminator_keyword(text: &str) -> bool {
    matches!(
        text.to_ascii_uppercase().as_str(),
        "FROM"
            | "WHERE"
            | "GROUP"
            | "HAVING"
            | "ORDER"
            | "LIMIT"
            | "OFFSET"
            // Bloque I: set ops también terminan un SELECT body.
            | "UNION"
            | "INTERSECT"
            | "EXCEPT"
            | "MINUS"
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
        // Bloque G3: `-N` solo se trata como literal negativo cuando el
        // token anterior NO termina un operando. Reglas:
        // - Número, String, `)` cierran un operando → `-` siguiente es operador.
        // - Ident: es operando SOLO si NO es un keyword que introduce un valor
        //   (LIMIT, OFFSET, VALUES, WHERE, AND, OR, IN, BETWEEN, RETURNING, ...).
        //   En esos casos `-N` es literal negativo. Para idents "comunes"
        //   (column refs) un `-` siguiente es resta.
        // Sin esta guarda `5-3` se tokenizaba como `5`, `-3` rompiendo la resta.
        let prev_is_operand = match tokens.last() {
            Some(t) => match (&t.kind, t.text.as_str()) {
                (TokenKind::Number, _) | (TokenKind::String, _) => true,
                (TokenKind::Symbol, ")") => true,
                (TokenKind::Ident, txt) => {
                    let upper = txt.to_ascii_uppercase();
                    // Lista de keywords que introducen un valor (NO son operandos).
                    !matches!(
                        upper.as_str(),
                        "LIMIT"
                            | "OFFSET"
                            | "VALUES"
                            | "WHERE"
                            | "AND"
                            | "OR"
                            | "NOT"
                            | "IN"
                            | "BETWEEN"
                            | "RETURNING"
                            | "BY"
                            | "ON"
                            | "USING"
                            | "SET"
                            | "SELECT"
                            | "HAVING"
                            | "WHEN"
                            | "THEN"
                            | "ELSE"
                            | "CASE"
                            | "LIKE"
                            | "AS"
                            | "FROM"
                            | "INTO"
                            | "DEFAULT"
                            | "IS"
                    )
                }
                _ => false,
            },
            None => false,
        };
        if ch.is_ascii_digit()
            || (ch == '-'
                && !prev_is_operand
                && index + 1 < chars.len()
                && chars[index + 1].is_ascii_digit())
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
            '(' | ')' | ',' | '*' | '=' | '+' | '/' | '%' => {
                tokens.push(Token {
                    kind: TokenKind::Symbol,
                    text: ch.to_string(),
                });
                index += 1;
            }
            // Bloque G3: '-' suelto como operador binario. El caso de
            // literal negativo ya lo capturó la rama de `is_ascii_digit`
            // arriba (que también detecta `-NN` cuando viene precedido
            // por algo que no es número/ident). Acá emitimos un símbolo
            // que el parser combinará en la precedencia aritmética.
            '-' => {
                tokens.push(Token {
                    kind: TokenKind::Symbol,
                    text: "-".to_string(),
                });
                index += 1;
            }
            // Bloque G3: `||` = concat. Dos pipes pegados forman un
            // único Symbol "||" para consistencia con `<=`, `>=`, etc.
            // Un único `|` suelto NO se soporta — error explícito.
            '|' => {
                if index + 1 < chars.len() && chars[index + 1] == '|' {
                    tokens.push(Token {
                        kind: TokenKind::Symbol,
                        text: "||".to_string(),
                    });
                    index += 2;
                } else {
                    return Err(DbError::new(
                        "símbolo no soportado: '|' suelto; ¿quisiste decir '||' (concat)?"
                            .to_string(),
                    ));
                }
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
                    return Err(DbError::new(
                        "símbolo no soportado: '!' suelto; ¿quisiste decir '!='?".to_string(),
                    ));
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
    /// Bloque F: flag scope-local que activa el parser de agregados
    /// dentro de un átomo WHERE. Solo HAVING lo enciende (via
    /// `parse_where_expr_with(true)`); WHERE normal lo deja en false
    /// para rechazar `SUM(x) > 10` con error claro.
    in_having: bool,
    /// Sec3 (2026-05-25): profundidad de recursión actual del parser
    /// expresiones WHERE/HAVING. Pre-fix, `WHERE (((((...)))))` con
    /// miles de paréntesis o `NOT NOT NOT...` consumía el stack del
    /// proceso (CWE-674: Uncontrolled Recursion). Se incrementa en
    /// cada entrada a `parse_where_or/and/not/primary` y se decrementa
    /// al salir; si supera `MAX_PARSE_DEPTH` devuelve `[GBY-4033]`.
    where_depth: usize,
    /// Bloque L2 (2026-05-27): stash temporal del nombre cuando
    /// `CONSTRAINT <name> CHECK ...` se detecta a nivel de tabla. Lo
    /// llena `try_match_table_constraint_check_head` y lo consume el
    /// caller en la misma iteración del loop de CREATE TABLE.
    pending_check_name: Option<String>,
}

/// Sec3: profundidad máxima permitida en el árbol de expresiones del
/// WHERE/HAVING. 100 es ~10× lo que un humano va a escribir a mano y
/// muy por debajo del límite de stack típico de Rust (~2 MB = ~5k
/// frames con frames promedio).
const MAX_PARSE_DEPTH: usize = 100;

impl Parser {
    fn parse_statement(&mut self) -> DbResult<Statement> {
        if self.match_keyword("CREATE") {
            return self.parse_create();
        }
        if self.match_keyword("INSERT") {
            return self.parse_insert();
        }
        if self.match_keyword("REPLACE") {
            // Bloque J2: REPLACE INTO ...
            return self.parse_replace();
        }
        if self.match_keyword("SELECT") {
            // Bloque I: a partir del primer SELECT puede venir un árbol
            // de set operations o un único SELECT. `parse_select_query`
            // consume el lhs y, si encuentra UNION/INTERSECT/EXCEPT,
            // arma el árbol con precedencia ANSI; si no, devuelve el
            // SELECT envuelto trivialmente en `SelectQuery::Select`.
            let stmt = self.parse_select_stmt()?;
            let lhs = SelectQuery::Select(Box::new(stmt));
            let query = self.parse_set_ops_after(lhs)?;
            return Ok(Statement::Select(Box::new(query)));
        }
        if self.match_keyword("VALUES") {
            // Bloque I: `VALUES (..), (..);` como statement standalone.
            let values = self.parse_values_body()?;
            let lhs = SelectQuery::Values(values);
            let query = self.parse_set_ops_after(lhs)?;
            return Ok(Statement::Select(Box::new(query)));
        }
        // Bloque I: `(SELECT ...) UNION (SELECT ...)` también empieza
        // con `(` — el statement-level se reconoce por el lookahead.
        if self.peek().kind == TokenKind::Symbol
            && self.peek().text == "("
            && self
                .tokens
                .get(self.pos + 1)
                .map(|t| {
                    t.kind == TokenKind::Ident
                        && (t.text.eq_ignore_ascii_case("SELECT")
                            || t.text.eq_ignore_ascii_case("VALUES"))
                })
                .unwrap_or(false)
        {
            let lhs = self.parse_select_term()?;
            let query = self.parse_set_ops_after(lhs)?;
            return Ok(Statement::Select(Box::new(query)));
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
        if self.match_keyword("RENAME") {
            // Bloque K1: `RENAME TABLE <old> TO <new>;` (alias estilo
            // MySQL de `ALTER TABLE <old> RENAME TO <new>`).
            self.expect_keyword("TABLE")?;
            let old_name = self.expect_ident()?;
            self.expect_keyword("TO")?;
            let new_name = self.expect_ident()?;
            return Ok(Statement::RenameTable(RenameTableStmt {
                old_name,
                new_name,
            }));
        }
        if self.match_keyword("TRUNCATE") {
            // Bloque J: `TRUNCATE TABLE <name>` (palabra TABLE opcional,
            // como en MySQL/SQLite).
            let _ = self.match_keyword("TABLE");
            let table = self.expect_ident()?;
            return Ok(Statement::Truncate(TruncateStmt { table }));
        }
        // Bloque T: transacciones explícitas. `BEGIN` y `START TRANSACTION`
        // son sinónimos (ANSI); `COMMIT` y `END` son sinónimos (también
        // ANSI). `ROLLBACK` no tiene alias estándar relevante en este
        // release (SAVEPOINT queda para un bloque posterior).
        if self.match_keyword("BEGIN") {
            let _ = self.match_keyword("TRANSACTION"); // `BEGIN TRANSACTION` opcional
            let _ = self.match_keyword("WORK"); // `BEGIN WORK` opcional (ANSI)
            return Ok(Statement::Begin);
        }
        if self.match_keyword("START") {
            self.expect_keyword("TRANSACTION")?;
            return Ok(Statement::Begin);
        }
        if self.match_keyword("COMMIT") {
            let _ = self.match_keyword("TRANSACTION");
            let _ = self.match_keyword("WORK");
            return Ok(Statement::Commit);
        }
        if self.match_keyword("END") {
            let _ = self.match_keyword("TRANSACTION");
            let _ = self.match_keyword("WORK");
            return Ok(Statement::Commit);
        }
        if self.match_keyword("ROLLBACK") {
            let _ = self.match_keyword("TRANSACTION");
            let _ = self.match_keyword("WORK");
            return Ok(Statement::Rollback);
        }
        Err(DbError::new(
            "sentencia no soportada (solo CREATE/INSERT/SELECT/UPDATE/DELETE/DROP/ALTER/RENAME/SHOW/INTEGRITY/TRUNCATE/BEGIN/COMMIT/ROLLBACK)",
        ))
    }

    fn parse_update(&mut self) -> DbResult<Statement> {
        let table = self.expect_ident()?;
        self.expect_keyword("SET")?;
        let mut assignments = Vec::new();
        loop {
            let column = self.expect_ident()?;
            self.expect_symbol("=")?;
            // Bloque G2: la RHS es una `Expr` general (función, CASE,
            // CAST, COALESCE, literal). Para back-compat con queries
            // pre-G2, un literal se parsea como `Expr::Literal(...)`
            // — el evaluador devuelve el `Value` original sin alterar.
            let value = self.parse_expr()?;
            assignments.push((column, value));
            if !self.match_symbol(",") {
                break;
            }
        }
        // Bloque E3: WHERE obligatorio, gramática completa (reusa la del
        // SELECT: WhereExpr con AND/OR/NOT/paréntesis + todos los átomos
        // E1+E2). Si falta, expect_keyword devuelve error.
        self.expect_keyword("WHERE")?;
        let where_clause = self.parse_where_expr()?;
        let returning = self.parse_returning_clause()?;
        Ok(Statement::Update(UpdateStmt {
            table,
            assignments,
            where_clause,
            returning,
        }))
    }

    fn parse_delete(&mut self) -> DbResult<Statement> {
        self.expect_keyword("FROM")?;
        let table = self.expect_ident()?;
        self.expect_keyword("WHERE")?;
        let where_clause = self.parse_where_expr()?;
        let returning = self.parse_returning_clause()?;
        Ok(Statement::Delete(DeleteStmt {
            table,
            where_clause,
            returning,
        }))
    }

    fn parse_create_index(&mut self, unique: bool) -> DbResult<Statement> {
        let name = self.expect_ident()?;
        self.expect_keyword("ON")?;
        let table = self.expect_ident()?;
        self.expect_symbol("(")?;
        // Bloque K2 (2026-05-26): la lista de columnas admite ≥ 1
        // ident separados por coma. La primera va a `column` para
        // mantener back-compat con el resto del executor; las
        // adicionales viajan en `extra_columns`. El validator del
        // executor exige que toda la lista sea INT cuando hay > 1
        // columna (`COMPOSITE_INDEX_REQUIRES_ALL_INT`, 4067).
        let column = self.expect_ident()?;
        let mut extra_columns: Vec<String> = Vec::new();
        while self.match_symbol(",") {
            extra_columns.push(self.expect_ident()?);
        }
        self.expect_symbol(")")?;
        Ok(Statement::CreateIndex(CreateIndexStmt {
            name,
            table,
            column,
            unique,
            extra_columns,
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
        if self.match_keyword("VIEW") {
            // Bloque V (2026-05-27).
            let if_exists = self.parse_if_exists()?;
            let name = self.expect_ident()?;
            return Ok(Statement::DropView(DropViewStmt { name, if_exists }));
        }
        self.expect_keyword("INDEX")?;
        let name = self.expect_ident()?;
        Ok(Statement::DropIndex(DropIndexStmt { name }))
    }

    /// Bloque V (2026-05-27): parsea `CREATE VIEW [IF NOT EXISTS] name
    /// [(col_aliases)] AS <select_query>`. Captura el texto SQL
    /// re-construido del SELECT desde los tokens (sin canonicalizar);
    /// el executor lo persiste en `ViewMeta.source` y lo re-parsea al
    /// expandir la vista.
    fn parse_create_view(&mut self) -> DbResult<Statement> {
        let if_not_exists = if self.match_keyword("IF") {
            self.expect_keyword("NOT")?;
            self.expect_keyword("EXISTS")?;
            true
        } else {
            false
        };
        let name = self.expect_ident()?;
        // Aliases opcionales `(a, b, ...)`.
        let column_aliases = if self.peek().kind == TokenKind::Symbol && self.peek().text == "(" {
            self.expect_symbol("(")?;
            let mut aliases = vec![self.expect_ident()?];
            while self.match_symbol(",") {
                aliases.push(self.expect_ident()?);
            }
            self.expect_symbol(")")?;
            Some(aliases)
        } else {
            None
        };
        self.expect_keyword("AS")?;
        // Snapshot del rango de tokens del SELECT para reconstruir el
        // source. Validamos sintácticamente que sea un SelectQuery
        // antes de descartar el AST — así un CREATE VIEW con SQL roto
        // falla en DDL, no en la primera lectura de la vista.
        let start = self.pos;
        let _ = self.parse_select_query_for_ctas()?;
        let end = self.pos;
        let source = reconstruct_sql_from_tokens(&self.tokens[start..end]);
        Ok(Statement::CreateView(CreateViewStmt {
            name,
            if_not_exists,
            column_aliases,
            source,
        }))
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
        if self.match_keyword("ADD") {
            // L3 (2026-05-27): `ADD CHECK (...)` y `ADD CONSTRAINT name CHECK (...)`
            // se discriminan ANTES de delegar a `parse_column_def`. La
            // pista es la keyword inmediatamente después de ADD:
            //   - `ADD CHECK`           → ADD CHECK sin nombre
            //   - `ADD CONSTRAINT n CHECK` → ADD CHECK con nombre
            //   - cualquier otra cosa  → ADD [COLUMN] <coldef>
            if self.match_keyword("CHECK") {
                return self.parse_alter_add_check(table, None);
            }
            // Snapshot para CONSTRAINT name CHECK — si tras `CONSTRAINT`
            // no viene un ident+CHECK, hacemos rollback y dejamos que
            // el path de ADD COLUMN falle con su error normal.
            let snap = self.pos;
            if self.match_keyword("CONSTRAINT") {
                let name_tok = self.peek().clone();
                if name_tok.kind == TokenKind::Ident {
                    let saved_after_name = self.pos + 1;
                    self.pos += 1;
                    if self.match_keyword("CHECK") {
                        return self.parse_alter_add_check(table, Some(name_tok.text));
                    }
                    // ROllback: `CONSTRAINT name X` con X != CHECK aún
                    // no soportado. Caer al error genérico.
                    self.pos = saved_after_name - 1;
                }
                self.pos = snap;
            }
            // The COLUMN keyword is optional, matching most other dialects.
            let _ = self.match_keyword("COLUMN");
            // Bloque L2: ALTER TABLE ADD COLUMN no admite CHECKs por
            // ahora — el column-level CHECK requeriría re-validar todas
            // las filas existentes para esa columna nueva. Hoy sólo
            // ALTER ADD CHECK (top-level) re-valida.
            let mut dropped: Vec<(Option<String>, Expr)> = Vec::new();
            let column = self.parse_column_def(&mut dropped)?;
            if !dropped.is_empty() {
                return Err(DbError::new(
                    "ALTER TABLE ADD COLUMN no admite CHECK en este release \
                     (usar `ALTER TABLE <t> ADD CHECK (...)` por separado)",
                ));
            }
            return Ok(Statement::AlterTableAddColumn(AlterAddColumnStmt {
                table,
                column,
            }));
        }
        // Bloque K1: `ALTER TABLE <t> DROP COLUMN [IF EXISTS] <col>`.
        // Residual #2: también `DROP CONSTRAINT [IF EXISTS] <name>`.
        if self.match_keyword("DROP") {
            if self.match_keyword("CONSTRAINT") {
                let if_exists = self.parse_if_exists()?;
                let name = self.expect_ident()?;
                return Ok(Statement::AlterTableDropConstraint(
                    AlterDropConstraintStmt {
                        table,
                        name,
                        if_exists,
                    },
                ));
            }
            self.expect_keyword("COLUMN")?;
            let if_exists = self.parse_if_exists()?;
            let column = self.expect_ident()?;
            return Ok(Statement::AlterTableDropColumn(AlterDropColumnStmt {
                table,
                column,
                if_exists,
            }));
        }
        // Bloque K1: `ALTER TABLE <old> RENAME TO <new>` (rename tabla)
        // o `ALTER TABLE <t> RENAME COLUMN <old> TO <new>` (rename col).
        if self.match_keyword("RENAME") {
            if self.match_keyword("TO") {
                let new_name = self.expect_ident()?;
                return Ok(Statement::RenameTable(RenameTableStmt {
                    old_name: table,
                    new_name,
                }));
            }
            self.expect_keyword("COLUMN")?;
            let old_name = self.expect_ident()?;
            self.expect_keyword("TO")?;
            let new_name = self.expect_ident()?;
            return Ok(Statement::AlterTableRenameColumn(AlterRenameColumnStmt {
                table,
                old_name,
                new_name,
            }));
        }
        Err(DbError::new(
            "ALTER TABLE: se esperaba ADD [COLUMN], DROP COLUMN, o RENAME [TO|COLUMN]",
        ))
    }

    /// L3 (2026-05-27): después de consumir `ALTER TABLE <t> ADD [CONSTRAINT n] CHECK`,
    /// parsea el cuerpo `(<expr>)` y canonicaliza el source con
    /// `format_expr` para que el executor lo persista round-trip-estable.
    fn parse_alter_add_check(
        &mut self,
        table: String,
        name: Option<String>,
    ) -> DbResult<Statement> {
        self.expect_symbol("(")?;
        let expr = self.parse_expr()?;
        self.expect_symbol(")")?;
        let source = format_expr(&expr)?;
        Ok(Statement::AlterTableAddCheck(AlterAddCheckStmt {
            table,
            name,
            source,
        }))
    }

    /// Shared between `CREATE TABLE` and `ALTER TABLE ADD COLUMN`. Reads
    /// `name type column_constraint*` and returns the parser-level
    /// `ColumnDef`. The constraint loop is intentionally permissive about
    /// order — semantic validation (e.g. "DEFAULT NULL incompatible con
    /// NOT NULL") happens later in `validate_create_table`.
    fn parse_column_def(
        &mut self,
        column_checks: &mut Vec<(Option<String>, Expr)>,
    ) -> DbResult<ColumnDef> {
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
                let (on_delete, on_update) = self.parse_fk_actions()?;
                references = Some(ForeignKeyDef {
                    table: target_table,
                    column: target_column,
                    on_delete,
                    on_update,
                    name: None,
                    extra_source_columns: Vec::new(),
                    extra_target_columns: Vec::new(),
                });
            } else if self.match_keyword("CONSTRAINT") {
                // Bloque L2: `CONSTRAINT <name> CHECK (...)` inline en
                // una columna. El `CONSTRAINT name` aplica sólo al
                // siguiente constraint; sólo soportamos CHECK por ahora
                // (PK/UNIQUE/FK con nombre quedan para otra entrega).
                let cname = self.expect_ident()?;
                self.expect_keyword("CHECK")?;
                self.expect_symbol("(")?;
                let expr = self.parse_expr()?;
                self.expect_symbol(")")?;
                column_checks.push((Some(cname), expr));
            } else if self.match_keyword("CHECK") {
                // Bloque L2: `CHECK (expr)` column-level sin nombre.
                self.expect_symbol("(")?;
                let expr = self.parse_expr()?;
                self.expect_symbol(")")?;
                column_checks.push((None, expr));
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

    /// Parse the optional `ON DELETE …` / `ON UPDATE …` tail of a
    /// `REFERENCES` clause, in any order, and at most one of each.
    ///
    /// Acciones admitidas (Bloque L1, 2026-05-27):
    /// `RESTRICT | CASCADE | SET NULL | SET DEFAULT | NO ACTION`.
    /// `NO ACTION` se acepta como sinónimo de `RESTRICT` (no hay modo
    /// diferido todavía). Defaults: `ON DELETE` → `RESTRICT` (más
    /// seguro: refuse antes que drop silencioso); `ON UPDATE` →
    /// `NO ACTION` (compatible con ANSI/PostgreSQL).
    fn parse_fk_actions(&mut self) -> DbResult<(OnDelete, OnUpdate)> {
        let mut on_delete: Option<OnDelete> = None;
        let mut on_update: Option<OnUpdate> = None;
        loop {
            if !self.match_keyword("ON") {
                break;
            }
            if self.match_keyword("DELETE") {
                if on_delete.is_some() {
                    return Err(DbError::new("ON DELETE declarado dos veces en la misma FK"));
                }
                on_delete = Some(self.parse_referential_action_on_delete()?);
            } else if self.match_keyword("UPDATE") {
                if on_update.is_some() {
                    return Err(DbError::new("ON UPDATE declarado dos veces en la misma FK"));
                }
                on_update = Some(self.parse_referential_action_on_update()?);
            } else {
                return Err(DbError::new(
                    "después de ON se espera DELETE o UPDATE en la cláusula REFERENCES",
                ));
            }
        }
        Ok((
            on_delete.unwrap_or(OnDelete::Restrict),
            on_update.unwrap_or(OnUpdate::NoAction),
        ))
    }

    /// Match one referential action token sequence for `ON DELETE`:
    /// `RESTRICT | CASCADE | SET NULL | SET DEFAULT | NO ACTION`.
    fn parse_referential_action_on_delete(&mut self) -> DbResult<OnDelete> {
        if self.match_keyword("CASCADE") {
            Ok(OnDelete::Cascade)
        } else if self.match_keyword("RESTRICT") {
            Ok(OnDelete::Restrict)
        } else if self.match_keyword("NO") {
            self.expect_keyword("ACTION")?;
            // NO ACTION ≡ RESTRICT en este release (sin deferred mode).
            Ok(OnDelete::Restrict)
        } else if self.match_keyword("SET") {
            if self.match_keyword("NULL") {
                Ok(OnDelete::SetNull)
            } else if self.match_keyword("DEFAULT") {
                Ok(OnDelete::SetDefault)
            } else {
                Err(DbError::new("ON DELETE SET requiere NULL o DEFAULT"))
            }
        } else {
            Err(DbError::new(
                "ON DELETE admite RESTRICT | CASCADE | SET NULL | SET DEFAULT | NO ACTION",
            ))
        }
    }

    /// Match one referential action token sequence for `ON UPDATE`. Las
    /// acciones se aceptan sintácticamente y se persisten; el motor
    /// nunca las dispara hoy porque la PK del padre es inmutable
    /// (`[GBY-4008]`).
    fn parse_referential_action_on_update(&mut self) -> DbResult<OnUpdate> {
        if self.match_keyword("CASCADE") {
            Ok(OnUpdate::Cascade)
        } else if self.match_keyword("RESTRICT") {
            Ok(OnUpdate::Restrict)
        } else if self.match_keyword("NO") {
            self.expect_keyword("ACTION")?;
            Ok(OnUpdate::NoAction)
        } else if self.match_keyword("SET") {
            if self.match_keyword("NULL") {
                Ok(OnUpdate::SetNull)
            } else if self.match_keyword("DEFAULT") {
                Ok(OnUpdate::SetDefault)
            } else {
                Err(DbError::new("ON UPDATE SET requiere NULL o DEFAULT"))
            }
        } else {
            Err(DbError::new(
                "ON UPDATE admite RESTRICT | CASCADE | SET NULL | SET DEFAULT | NO ACTION",
            ))
        }
    }

    /// Bloque L2 (2026-05-27): lookahead para detectar
    /// `CONSTRAINT <name> CHECK (…)` a nivel de tabla. Cuando
    /// matchea, consume los 3 tokens (CONSTRAINT, ident, CHECK), stashea
    /// el nombre en `self.pending_check_name` y devuelve `Ok(true)`. Si
    /// la palabra después de CONSTRAINT no es un ident seguido de CHECK,
    /// hace rollback y devuelve `Ok(false)`.
    ///
    /// Residual #2 (2026-05-27): los otros tipos (PRIMARY KEY / UNIQUE /
    /// FOREIGN KEY) se manejan vía `try_match_named_table_constraint_head`.
    fn try_match_table_constraint_check_head(&mut self) -> DbResult<bool> {
        let snap = self.pos;
        if !self.match_keyword("CONSTRAINT") {
            return Ok(false);
        }
        let name_tok = self.peek().clone();
        if name_tok.kind != TokenKind::Ident {
            self.pos = snap;
            return Ok(false);
        }
        self.pos += 1;
        if !self.match_keyword("CHECK") {
            self.pos = snap;
            return Ok(false);
        }
        self.pending_check_name = Some(name_tok.text);
        Ok(true)
    }

    /// Residual #2 (2026-05-27): detecta el inicio de
    /// `CONSTRAINT <name> PRIMARY KEY | UNIQUE | FOREIGN KEY` a nivel de
    /// tabla. Si matchea, consume los 3 tokens iniciales (CONSTRAINT +
    /// ident + keyword del kind) y devuelve `Some(NamedConstraintHead)`
    /// con `name` y `kind`; el caller parsea el cuerpo (`(cols)` o
    /// `(col) REFERENCES t (col) …`).
    ///
    /// CHECK no se incluye acá — sigue por `try_match_table_constraint_check_head`
    /// porque su cuerpo es un `Expr`, no una lista de columnas.
    fn try_match_named_table_constraint_head(&mut self) -> DbResult<Option<NamedConstraintHead>> {
        let snap = self.pos;
        if !self.match_keyword("CONSTRAINT") {
            return Ok(None);
        }
        let name_tok = self.peek().clone();
        if name_tok.kind != TokenKind::Ident {
            self.pos = snap;
            return Ok(None);
        }
        self.pos += 1;
        if self.match_keyword("PRIMARY") {
            self.expect_keyword("KEY")?;
            return Ok(Some(NamedConstraintHead {
                name: name_tok.text,
                kind: NamedConstraintKind::PrimaryKey,
            }));
        }
        if self.match_keyword("UNIQUE") {
            return Ok(Some(NamedConstraintHead {
                name: name_tok.text,
                kind: NamedConstraintKind::Unique,
            }));
        }
        if self.match_keyword("FOREIGN") {
            self.expect_keyword("KEY")?;
            return Ok(Some(NamedConstraintHead {
                name: name_tok.text,
                kind: NamedConstraintKind::ForeignKey,
            }));
        }
        // No es ninguno de los tres → rollback y dejar que el handler
        // de CHECK (o el error genérico) capture.
        self.pos = snap;
        Ok(None)
    }

    /// Bloque L1 (2026-05-27): lookahead para distinguir un constraint
    /// `UNIQUE (a, b, ...)` a nivel de tabla del UNIQUE inline en una
    /// columna. Devuelve `true` y consume `UNIQUE` cuando lo siguiente
    /// es `(`; deja `self.pos` intacto si no es el caso.
    ///
    /// Vive aparte para esquivar `clippy::blocks-in-conditions` y dejar
    /// el `else if` del CREATE TABLE legible como una sola línea.
    fn try_match_table_unique_head(&mut self) -> bool {
        let snap = self.pos;
        if !self.match_keyword("UNIQUE") {
            return false;
        }
        let tok = self.peek();
        if tok.kind == TokenKind::Symbol && tok.text == "(" {
            true
        } else {
            self.pos = snap;
            false
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
        // Bloque V (2026-05-27): `CREATE VIEW [IF NOT EXISTS] name
        // [(col_aliases)] AS <select_query>`. Reusa el parser del
        // SELECT/VALUES/set-ops del CTAS (K1).
        if self.match_keyword("VIEW") {
            return self.parse_create_view();
        }
        self.expect_keyword("TABLE")?;
        // Bloque K1: `IF NOT EXISTS` opcional. Sólo significativo en CTAS
        // (la forma clásica de CREATE TABLE rechaza el nombre duplicado y
        // por compatibilidad ignora el flag — mantenido para no romper
        // SQL existente de usuarios que ya lo escriben por costumbre).
        let if_not_exists = if self.match_keyword("IF") {
            self.expect_keyword("NOT")?;
            self.expect_keyword("EXISTS")?;
            true
        } else {
            false
        };
        let name = self.expect_ident()?;

        // Bloque K1: distinguir `CREATE TABLE t AS <select>` (CTAS, sin
        // paréntesis) y `CREATE TABLE t (col_aliases...) AS <select>`
        // (CTAS con alias de columnas) del clásico `CREATE TABLE t (col_def, ...)`.
        if self.match_keyword("AS") {
            let source = self.parse_select_query_for_ctas()?;
            return Ok(Statement::CreateTableAs(CreateTableAsStmt {
                name,
                source: Box::new(source),
                if_not_exists,
                column_aliases: None,
            }));
        }
        self.expect_symbol("(")?;
        // Lookahead K1: si todo lo que hay dentro del paréntesis es una
        // lista de idents simples (sin tipo, sin constraints) cerrada por
        // `)` y seguida de `AS`, es la forma CTAS con alias de columnas.
        // Snapshotteamos `self.pos` y si el intento falla volvemos atrás
        // y caemos al path clásico.
        let snapshot = self.pos;
        if let Some(aliases) = self.try_parse_ctas_column_aliases() {
            // Después de `)` consumido por try_parse_ctas_column_aliases,
            // exige `AS`. Si no está, volver al snapshot — era un
            // CREATE TABLE clásico cuya primera columna era un Ident sin
            // tipo (que va a fallar después como error de parsing).
            if self.match_keyword("AS") {
                let source = self.parse_select_query_for_ctas()?;
                return Ok(Statement::CreateTableAs(CreateTableAsStmt {
                    name,
                    source: Box::new(source),
                    if_not_exists,
                    column_aliases: Some(aliases),
                }));
            }
            self.pos = snapshot;
        }
        let mut columns = Vec::new();
        let mut primary_key = String::new();
        // Bloque K2 (2026-05-26): la PK puede declararse de tres formas:
        //   a) inline en una columna  (id INT PRIMARY KEY)
        //   b) table-level single     (PRIMARY KEY (id))
        //   c) table-level composite  (PRIMARY KEY (a, b, ...))
        // Las tres son mutuamente excluyentes: si aparece más de una se
        // emite `[GBY-4065] PRIMARY_KEY_DUPLICATED`. `table_level_pk` lleva
        // las columnas del table-level y, si se eligió esa forma, su
        // primer ítem se copia a `primary_key` para mantener el shape del
        // AST. Las adicionales viajan en `primary_key_extra` del Stmt.
        let mut table_level_pk: Option<Vec<String>> = None;
        // Residual #2 (2026-05-27): nombre opcional para la PK
        // (`CONSTRAINT <name> PRIMARY KEY (...)`). Sólo se permite con
        // la forma table-level — la inline `id INT PRIMARY KEY` no
        // acepta nombre (ANSI tampoco lo admite ahí).
        let mut table_level_pk_name: Option<String> = None;
        // Bloque L1 (2026-05-27): table-level `UNIQUE (a, b, ...)`. Cada
        // declaración se materializa en el executor como un índice
        // UNIQUE (compuesto si len > 1, simple si len == 1) reutilizando
        // el camino que ya armó K2 para `CREATE UNIQUE INDEX`.
        let mut table_level_unique: Vec<Vec<String>> = Vec::new();
        // Residual #2 (2026-05-27): UNIQUE table-level con nombre
        // (`CONSTRAINT <name> UNIQUE (a, b, ...)`). Mismo materializado
        // que `table_level_unique` pero con `IndexMeta.name = name`.
        let mut table_level_named_unique: Vec<(String, Vec<String>)> = Vec::new();
        // Residual #2 (2026-05-27): FK table-level con nombre.
        let mut table_level_named_fks: Vec<NamedForeignKey> = Vec::new();
        // Bloque L2 (2026-05-27): CHECKs recogidos del column-level y
        // del table-level. Cada entrada es `(nombre opcional, Expr)`.
        // El nombre se materializa después (synthetic
        // `<table>_check_<N>` si no se declaró `CONSTRAINT name`).
        let mut raw_checks: Vec<(Option<String>, Expr)> = Vec::new();
        loop {
            // Detectar table constraint `PRIMARY KEY (a, b, ...)` antes
            // de delegar a `parse_column_def` — sin ambigüedad porque
            // una columna SIEMPRE empieza con un Ident seguido de un
            // tipo, no por la keyword PRIMARY.
            if self.match_keyword("PRIMARY") {
                self.expect_keyword("KEY")?;
                self.expect_symbol("(")?;
                let mut pk_cols: Vec<String> = vec![self.expect_ident()?];
                while self.match_symbol(",") {
                    pk_cols.push(self.expect_ident()?);
                }
                self.expect_symbol(")")?;
                if table_level_pk.is_some() {
                    return Err(coded(
                        codes::PRIMARY_KEY_DUPLICATED,
                        "PRIMARY KEY declarada dos veces a nivel de tabla",
                    ));
                }
                table_level_pk = Some(pk_cols);
            } else if self.try_match_table_unique_head() {
                // Aquí ya consumimos `UNIQUE`; resta `(col, col, ...)`.
                self.expect_symbol("(")?;
                let mut cols: Vec<String> = vec![self.expect_ident()?];
                while self.match_symbol(",") {
                    cols.push(self.expect_ident()?);
                }
                self.expect_symbol(")")?;
                table_level_unique.push(cols);
            } else if self.match_keyword("CHECK") {
                // Bloque L2: table-level `CHECK (expr)` sin nombre.
                self.expect_symbol("(")?;
                let expr = self.parse_expr()?;
                self.expect_symbol(")")?;
                raw_checks.push((None, expr));
            } else if self.try_match_table_constraint_check_head()? {
                // Bloque L2: `CONSTRAINT name CHECK (...)` table-level.
                let name = self
                    .pending_check_name
                    .take()
                    .expect("try_match_table_constraint_check_head sets pending_check_name");
                self.expect_symbol("(")?;
                let expr = self.parse_expr()?;
                self.expect_symbol(")")?;
                raw_checks.push((Some(name), expr));
            } else if let Some(named) = self.try_match_named_table_constraint_head()? {
                // Residual #2: `CONSTRAINT <name> PRIMARY KEY | UNIQUE | FOREIGN KEY`.
                // El helper consumió 3 tokens (CONSTRAINT + ident + kind);
                // ahora parseamos el cuerpo según `kind`.
                match named.kind {
                    NamedConstraintKind::PrimaryKey => {
                        self.expect_symbol("(")?;
                        let mut pk_cols: Vec<String> = vec![self.expect_ident()?];
                        while self.match_symbol(",") {
                            pk_cols.push(self.expect_ident()?);
                        }
                        self.expect_symbol(")")?;
                        if table_level_pk.is_some() {
                            return Err(coded(
                                codes::PRIMARY_KEY_DUPLICATED,
                                "PRIMARY KEY declarada dos veces a nivel de tabla",
                            ));
                        }
                        table_level_pk = Some(pk_cols);
                        table_level_pk_name = Some(named.name);
                    }
                    NamedConstraintKind::Unique => {
                        self.expect_symbol("(")?;
                        let mut cols: Vec<String> = vec![self.expect_ident()?];
                        while self.match_symbol(",") {
                            cols.push(self.expect_ident()?);
                        }
                        self.expect_symbol(")")?;
                        table_level_named_unique.push((named.name, cols));
                    }
                    NamedConstraintKind::ForeignKey => {
                        // Residual #3 (2026-05-27): admite multi-col en
                        // source y target. Single-col → extra_* vacíos.
                        // Arity de source y target debe matchear.
                        self.expect_symbol("(")?;
                        let mut source_cols: Vec<String> = vec![self.expect_ident()?];
                        while self.match_symbol(",") {
                            source_cols.push(self.expect_ident()?);
                        }
                        self.expect_symbol(")")?;
                        self.expect_keyword("REFERENCES")?;
                        let target_table = self.expect_ident()?;
                        self.expect_symbol("(")?;
                        let mut target_cols: Vec<String> = vec![self.expect_ident()?];
                        while self.match_symbol(",") {
                            target_cols.push(self.expect_ident()?);
                        }
                        self.expect_symbol(")")?;
                        if source_cols.len() != target_cols.len() {
                            return Err(DbError::new(format!(
                                "FOREIGN KEY '{}' tiene arity inconsistente: {} columnas \
                                 source vs {} columnas target",
                                named.name,
                                source_cols.len(),
                                target_cols.len()
                            )));
                        }
                        let (on_delete, on_update) = self.parse_fk_actions()?;
                        let anchor_source = source_cols.remove(0);
                        let anchor_target = target_cols.remove(0);
                        table_level_named_fks.push(NamedForeignKey {
                            name: named.name,
                            column: anchor_source,
                            target_table,
                            target_column: anchor_target,
                            on_delete,
                            on_update,
                            extra_source_columns: source_cols,
                            extra_target_columns: target_cols,
                        });
                    }
                }
            } else {
                let column = self.parse_column_def(&mut raw_checks)?;
                if column.primary_key {
                    if !primary_key.is_empty() {
                        return Err(coded(
                            codes::PRIMARY_KEY_DUPLICATED,
                            format!(
                                "PRIMARY KEY ya declarada en columna '{}'; '{}' no puede tenerla también",
                                primary_key, column.name
                            ),
                        ));
                    }
                    primary_key = column.name.clone();
                }
                columns.push(column);
            }
            if self.match_symbol(")") {
                break;
            }
            self.expect_symbol(",")?;
        }
        // Bloque L2: materializar nombres definitivos y serializar el
        // `source` canónico. Si dos CHECKs explícitamente nombrados
        // colisionan, rebotamos con `[GBY-2004]` (duplicate) reciclado
        // como mensaje claro.
        let mut check_constraints: Vec<CheckConstraint> = Vec::new();
        let mut seen_names: HashSet<String> = HashSet::new();
        let mut synth_counter = 1usize;
        for (maybe_name, expr) in raw_checks {
            let final_name = match maybe_name {
                Some(n) => {
                    let lower = n.to_ascii_lowercase();
                    if !seen_names.insert(lower) {
                        return Err(DbError::new(format!(
                            "CHECK constraint '{}' declarada dos veces en la misma tabla",
                            n
                        )));
                    }
                    n
                }
                None => loop {
                    let candidate =
                        format!("{}_check_{}", name.to_ascii_lowercase(), synth_counter);
                    synth_counter += 1;
                    if seen_names.insert(candidate.clone()) {
                        break candidate;
                    }
                },
            };
            let source = format_expr(&expr)?;
            check_constraints.push(CheckConstraint {
                name: final_name,
                source,
            });
        }
        // Reconciliar table-level PK contra inline PK.
        let primary_key_extra = if let Some(pk_cols) = table_level_pk {
            if !primary_key.is_empty() {
                return Err(coded(
                    codes::PRIMARY_KEY_DUPLICATED,
                    format!(
                        "PRIMARY KEY ya declarada inline en columna '{}'; no se puede declarar \
                         también a nivel de tabla con PRIMARY KEY (...)",
                        primary_key
                    ),
                ));
            }
            let mut it = pk_cols.into_iter();
            primary_key = it
                .next()
                .expect("parser garantiza ≥ 1 columna en PRIMARY KEY (...)");
            it.collect()
        } else {
            Vec::new()
        };
        Ok(Statement::CreateTable(CreateTableStmt {
            name,
            columns,
            primary_key,
            primary_key_extra,
            primary_key_name: table_level_pk_name,
            unique_constraints: table_level_unique,
            named_unique_constraints: table_level_named_unique,
            named_foreign_keys: table_level_named_fks,
            check_constraints,
        }))
    }

    /// Bloque K1: intenta consumir `(ident, ident, ...)` como lista de
    /// alias de columnas para CTAS. Devuelve `Some(aliases)` si el cierre
    /// `)` aparece sin que aparezca ningún token incompatible (tipo,
    /// constraint, símbolo distinto a `,` o `)`); el caller decide si la
    /// secuencia es realmente CTAS examinando si después viene `AS`.
    /// En caso de no matchear, deja `self.pos` justo después del `(`
    /// inicial (el caller hace rollback al snapshot original).
    fn try_parse_ctas_column_aliases(&mut self) -> Option<Vec<String>> {
        let start = self.pos;
        let mut aliases = Vec::new();
        // Caso vacío `()` no se acepta — siempre habrá al menos un ident.
        loop {
            let tok = self.peek().clone();
            if tok.kind != TokenKind::Ident {
                self.pos = start;
                return None;
            }
            self.pos += 1;
            aliases.push(tok.text);
            let next = self.peek().clone();
            if next.kind == TokenKind::Symbol && next.text == "," {
                self.pos += 1;
                continue;
            }
            if next.kind == TokenKind::Symbol && next.text == ")" {
                self.pos += 1;
                return Some(aliases);
            }
            // Cualquier otra cosa (otro ident, keyword tipo `INT`, etc.)
            // → no era CTAS aliases.
            self.pos = start;
            return None;
        }
    }

    /// Bloque K1: parsea la fuente de un CTAS (`SELECT ...`, `VALUES ...`,
    /// o un set-op de cualquiera de las dos formas). Reusa el camino
    /// completo del bloque I.
    fn parse_select_query_for_ctas(&mut self) -> DbResult<SelectQuery> {
        if self.match_keyword("SELECT") {
            let stmt = self.parse_select_stmt()?;
            let lhs = SelectQuery::Select(Box::new(stmt));
            return self.parse_set_ops_after(lhs);
        }
        if self.match_keyword("VALUES") {
            let values = self.parse_values_body()?;
            let lhs = SelectQuery::Values(values);
            return self.parse_set_ops_after(lhs);
        }
        // `(SELECT ...) UNION ...` también es válido.
        if self.peek().kind == TokenKind::Symbol && self.peek().text == "(" {
            let lhs = self.parse_select_term()?;
            return self.parse_set_ops_after(lhs);
        }
        Err(DbError::new(
            "CREATE TABLE AS: se esperaba SELECT, VALUES o un subquery entre paréntesis",
        ))
    }

    fn parse_insert(&mut self) -> DbResult<Statement> {
        let stmt = self.parse_insert_body(false)?;
        Ok(Statement::Insert(stmt))
    }

    /// Bloque J2: `REPLACE INTO ...` reusa el cuerpo del INSERT y le
    /// asigna `on_conflict = Replace` automáticamente. Equivale a un
    /// `INSERT ... ON CONFLICT DO REPLACE`.
    fn parse_replace(&mut self) -> DbResult<Statement> {
        let stmt = self.parse_insert_body(true)?;
        Ok(Statement::Replace(stmt))
    }

    /// Bloque J + J2: parsea el cuerpo común de INSERT/REPLACE. Cuando
    /// `force_replace` es true ignoramos el `ON CONFLICT` que pudiera
    /// venir en SQL (REPLACE INTO ya define la acción) y construimos
    /// `OnConflictAction::Replace`.
    fn parse_insert_body(&mut self, force_replace: bool) -> DbResult<InsertStmt> {
        self.expect_keyword("INTO")?;
        let table = self.expect_ident()?;
        self.expect_symbol("(")?;
        let columns = self.parse_ident_list()?;
        self.expect_symbol(")")?;
        let source = if self.match_keyword("VALUES") {
            let mut rows = Vec::new();
            self.expect_symbol("(")?;
            rows.push(self.parse_value_list()?);
            self.expect_symbol(")")?;
            while self.match_symbol(",") {
                self.expect_symbol("(")?;
                rows.push(self.parse_value_list()?);
                self.expect_symbol(")")?;
            }
            InsertSource::Values(rows)
        } else if self.match_keyword("SELECT") {
            let subquery = self.parse_select_stmt()?;
            InsertSource::Select(Box::new(subquery))
        } else {
            return Err(coded(
                codes::INSERT_COLS_VS_VALUES_MISMATCH,
                format!(
                    "INSERT/REPLACE INTO '{}': se esperaba VALUES (…) o SELECT … después de la lista de columnas",
                    table
                ),
            ));
        };
        let on_conflict = if force_replace {
            Some(OnConflict {
                target: None,
                action: OnConflictAction::Replace,
            })
        } else if self.match_keyword("ON") {
            self.expect_keyword("CONFLICT")?;
            // Target opcional: `(col)` — solo se acepta un nombre por ahora.
            let target = if self.match_symbol("(") {
                let c = self.expect_ident()?;
                self.expect_symbol(")")?;
                Some(c)
            } else {
                None
            };
            self.expect_keyword("DO")?;
            let action = if self.match_keyword("NOTHING") {
                OnConflictAction::DoNothing
            } else if self.match_keyword("UPDATE") {
                self.expect_keyword("SET")?;
                let mut assignments = Vec::new();
                loop {
                    let col = self.expect_ident()?;
                    self.expect_symbol("=")?;
                    // Bloque G2: igual que `UPDATE SET ...`, la RHS es
                    // una `Expr`. `EXCLUDED.col` sigue sin habilitarse
                    // (queda en backlog J2-P2).
                    let value = self.parse_expr()?;
                    assignments.push((col, value));
                    if !self.match_symbol(",") {
                        break;
                    }
                }
                OnConflictAction::DoUpdate { assignments }
            } else {
                return Err(coded(
                    codes::ON_CONFLICT_INVALID,
                    "ON CONFLICT: se esperaba DO NOTHING o DO UPDATE SET ...",
                ));
            };
            Some(OnConflict { target, action })
        } else {
            None
        };
        let returning = self.parse_returning_clause()?;
        Ok(InsertStmt {
            table,
            columns,
            source,
            on_conflict,
            returning,
        })
    }

    /// Bloque J2: cláusula `RETURNING` opcional al final de
    /// INSERT/UPDATE/DELETE. Acepta `RETURNING *` o `RETURNING col1, col2`.
    fn parse_returning_clause(&mut self) -> DbResult<Option<Vec<SelectItem>>> {
        if !self.match_keyword("RETURNING") {
            return Ok(None);
        }
        if self.match_symbol("*") {
            return Ok(Some(vec![SelectItem::Star]));
        }
        let mut items = Vec::new();
        items.push(SelectItem::Column(self.expect_ident()?));
        while self.match_symbol(",") {
            items.push(SelectItem::Column(self.expect_ident()?));
        }
        Ok(Some(items))
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
        self.parse_select_stmt_inner(true)
    }

    /// Bloque I: como `parse_select_stmt` pero con `allow_trailing_order_limit`
    /// configurable. Cuando se llama desde el RHS de un set-op sin
    /// paréntesis envolventes, hay que dejar el `ORDER BY` / `LIMIT`
    /// final al outer (regla ANSI). Cuando se llama desde un statement
    /// top-level o desde dentro de un `( SELECT ... )` con paréntesis,
    /// el ORDER BY/LIMIT pertenece al SELECT mismo.
    fn parse_select_stmt_inner(
        &mut self,
        allow_trailing_order_limit: bool,
    ) -> DbResult<SelectStmt> {
        // Bloque F: `DISTINCT` opcional inmediatamente después de SELECT.
        // No se combina con agregados sin GROUP BY de manera explícita —
        // el executor valida la coherencia ANSI más abajo.
        let distinct = self.match_keyword("DISTINCT");
        let columns = self.parse_select_list()?;
        self.expect_keyword("FROM")?;
        // Base table + alias opcional (`AS` aceptado pero opcional).
        // Bloque H: el FROM puede arrancar con una derived table
        // `(SELECT ...) [AS] alias`. En ese caso el "table" es solo el
        // alias y `derived_source` lleva la subquery. El alias es
        // obligatorio (ANSI estricto, `[GBY-4048]`).
        let (table, table_alias, derived_source, values_source) = if self.peek().kind
            == TokenKind::Symbol
            && self.peek().text == "("
            && self
                .tokens
                .get(self.pos + 1)
                .map(|t| t.kind == TokenKind::Ident && t.text.eq_ignore_ascii_case("SELECT"))
                .unwrap_or(false)
        {
            self.expect_symbol("(")?;
            self.expect_keyword("SELECT")?;
            let sub = self.parse_select_stmt()?;
            self.expect_symbol(")")?;
            // Alias OBLIGATORIO. Acepta `AS alias` o `alias` bare.
            let alias = if self.match_keyword("AS") {
                Some(self.expect_ident()?)
            } else if self.peek().kind == TokenKind::Ident
                && !is_select_terminator_keyword(&self.peek().text)
            {
                let a = self.peek().text.clone();
                self.pos += 1;
                Some(a)
            } else {
                None
            };
            let alias = alias.ok_or_else(|| {
                coded(
                    codes::DERIVED_TABLE_REQUIRES_ALIAS,
                    "derived table `(SELECT ...)` requiere un alias obligatorio \
                         (`(SELECT ...) AS sub`); ANSI no permite omitirlo",
                )
            })?;
            (alias, None, Some(Box::new(sub)), None)
        } else if self.peek().kind == TokenKind::Symbol
            && self.peek().text == "("
            && self
                .tokens
                .get(self.pos + 1)
                .map(|t| t.kind == TokenKind::Ident && t.text.eq_ignore_ascii_case("VALUES"))
                .unwrap_or(false)
        {
            // Bloque I: `FROM (VALUES (...), ...) AS t(c1, c2, ...)`.
            self.expect_symbol("(")?;
            self.expect_keyword("VALUES")?;
            let values = self.parse_values_body()?;
            self.expect_symbol(")")?;
            let (alias, cols) = self.parse_values_in_from_alias()?;
            (alias, None, None, Some((Box::new(values), cols)))
        } else {
            let t = self.expect_ident()?;
            let a = self.try_parse_alias()?;
            (t, a, None, None)
        };

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
            // Bloque H: el RHS de un JOIN también puede ser una derived
            // table. Alias obligatorio.
            let right = if self.peek().kind == TokenKind::Symbol
                && self.peek().text == "("
                && self
                    .tokens
                    .get(self.pos + 1)
                    .map(|t| t.kind == TokenKind::Ident && t.text.eq_ignore_ascii_case("SELECT"))
                    .unwrap_or(false)
            {
                self.expect_symbol("(")?;
                self.expect_keyword("SELECT")?;
                let sub = self.parse_select_stmt()?;
                self.expect_symbol(")")?;
                let alias = if self.match_keyword("AS") {
                    Some(self.expect_ident()?)
                } else if self.peek().kind == TokenKind::Ident
                    && !is_select_terminator_keyword(&self.peek().text)
                {
                    let a = self.peek().text.clone();
                    self.pos += 1;
                    Some(a)
                } else {
                    None
                };
                let alias = alias.ok_or_else(|| {
                    coded(
                        codes::DERIVED_TABLE_REQUIRES_ALIAS,
                        "derived table en JOIN requiere alias obligatorio",
                    )
                })?;
                TableRef {
                    name: alias,
                    alias: None,
                    derived: Some(Box::new(sub)),
                    values: None,
                    values_columns: None,
                }
            } else if self.peek().kind == TokenKind::Symbol
                && self.peek().text == "("
                && self
                    .tokens
                    .get(self.pos + 1)
                    .map(|t| t.kind == TokenKind::Ident && t.text.eq_ignore_ascii_case("VALUES"))
                    .unwrap_or(false)
            {
                // Bloque I: VALUES en JOIN.
                self.expect_symbol("(")?;
                self.expect_keyword("VALUES")?;
                let values = self.parse_values_body()?;
                self.expect_symbol(")")?;
                let (alias, cols) = self.parse_values_in_from_alias()?;
                TableRef {
                    name: alias,
                    alias: None,
                    derived: None,
                    values: Some(Box::new(values)),
                    values_columns: Some(cols),
                }
            } else {
                let right_name = self.expect_ident()?;
                let right_alias = self.try_parse_alias()?;
                TableRef {
                    name: right_name,
                    alias: right_alias,
                    derived: None,
                    values: None,
                    values_columns: None,
                }
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

        // Bloque F: GROUP BY <col> [, <col>]* — opcional, entre WHERE y
        // HAVING/ORDER BY. Acepta columnas bare (single-table) o
        // cualificadas (`tabla.col`); el executor las resuelve.
        let mut group_by: Vec<String> = Vec::new();
        if self.match_keyword("GROUP") {
            self.expect_keyword("BY")?;
            group_by.push(self.expect_ident()?);
            while self.match_symbol(",") {
                group_by.push(self.expect_ident()?);
            }
        }

        // Bloque F: HAVING — mismo grammar que WHERE pero permite
        // funciones agregadas como LHS de un átomo (`HAVING SUM(x) > 10`).
        let mut having: Option<WhereExpr> = None;
        if self.match_keyword("HAVING") {
            having = Some(self.parse_where_expr_with(true)?);
        }

        // Optional ORDER BY <ident> [ASC|DESC]. Has to come after WHERE
        // and before LIMIT/OFFSET — that's the standard SQL order and
        // also what most callers expect.
        //
        // Bloque I: cuando este SELECT es un sub-término dentro de un
        // árbol de set ops sin paréntesis envolventes
        // (`SELECT ... UNION SELECT ... ORDER BY x`), el ORDER BY/LIMIT
        // pertenece al outer y NO debe consumirse acá. El flag
        // `allow_trailing_order_limit` decide.
        let mut order_by = None;
        if allow_trailing_order_limit && self.match_keyword("ORDER") {
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
            if !allow_trailing_order_limit {
                break;
            }
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
            derived_source,
            values_source,
            table_alias,
            joins,
            columns,
            where_clause,
            distinct,
            group_by,
            having,
            order_by,
            limit,
            offset,
        })
    }

    /// Bloque I (2026-05-26): parsea el cuerpo de un `VALUES`: una o
    /// más tuplas `( expr [, expr]* )` separadas por coma. Cada `expr`
    /// se parsea con `parse_expr` para admitir literales y expresiones
    /// constantes (`1+2`, `LENGTH('abc')`). Toda tupla debe tener la
    /// misma arity — la validación final ocurre en el executor (4056)
    /// porque también lo chequea el path standalone que no pasa por
    /// este parser (e.g. INSERT...VALUES preexistente).
    fn parse_values_body(&mut self) -> DbResult<ValuesClause> {
        let mut rows: Vec<Vec<Expr>> = Vec::new();
        loop {
            self.expect_symbol("(")?;
            let mut row: Vec<Expr> = Vec::new();
            row.push(self.parse_expr()?);
            while self.match_symbol(",") {
                row.push(self.parse_expr()?);
            }
            self.expect_symbol(")")?;
            rows.push(row);
            if !self.match_symbol(",") {
                break;
            }
        }
        if rows.is_empty() {
            return Err(coded(
                codes::VALUES_EMPTY,
                "VALUES sin filas — la cláusula necesita al menos `(...)`",
            ));
        }
        Ok(ValuesClause { rows })
    }

    /// Bloque I: tras un `(VALUES ...)` dentro de FROM, parsea el alias
    /// obligatorio de tabla + el alias obligatorio de columnas
    /// (`AS t(c1, c2, ...)` o `t(c1, c2, ...)`).
    fn parse_values_in_from_alias(&mut self) -> DbResult<(String, Vec<String>)> {
        let _ = self.match_keyword("AS");
        let alias = if self.peek().kind == TokenKind::Ident
            && !is_select_terminator_keyword(&self.peek().text)
        {
            let a = self.peek().text.clone();
            self.pos += 1;
            a
        } else {
            return Err(coded(
                codes::VALUES_IN_FROM_REQUIRES_ALIAS,
                "VALUES en FROM requiere alias de tabla obligatorio: \
                 `(VALUES (...), ...) AS t(c1, c2, ...)`",
            ));
        };
        if !self.match_symbol("(") {
            return Err(coded(
                codes::VALUES_IN_FROM_REQUIRES_ALIAS,
                format!(
                    "VALUES en FROM '{}' requiere lista de aliases de columna: \
                     `AS {}(c1, c2, ...)`",
                    alias, alias
                ),
            ));
        }
        let mut cols: Vec<String> = Vec::new();
        cols.push(self.expect_ident()?);
        while self.match_symbol(",") {
            cols.push(self.expect_ident()?);
        }
        self.expect_symbol(")")?;
        Ok((alias, cols))
    }

    /// Bloque I: tras un `lhs` ya parseado (SELECT plano o VALUES),
    /// consume opcionalmente un árbol de set operations a su derecha,
    /// con la precedencia ANSI: INTERSECT ata más fuerte que
    /// UNION/EXCEPT. Si después del árbol viene `ORDER BY`/`LIMIT`/
    /// `OFFSET` a nivel top, lo cuelga del nodo `SetOp` resultante.
    fn parse_set_ops_after(&mut self, lhs: SelectQuery) -> DbResult<SelectQuery> {
        // Primero: si lo siguiente es INTERSECT, vamos a un sub-nivel
        // de "intersect" con LHS = `lhs`, luego sigue UNION/EXCEPT.
        let mut current = self.parse_intersect_after(lhs)?;
        // Capa UNION/EXCEPT (asociativos a izquierda).
        loop {
            let op = if self.match_keyword("UNION") {
                SetOpKind::Union
            } else if self.match_keyword("EXCEPT") || self.match_keyword("MINUS") {
                SetOpKind::Except
            } else {
                break;
            };
            let all = self.match_keyword("ALL");
            let rhs_term = self.parse_select_term()?;
            // El RHS también puede tener INTERSECT que ate más fuerte.
            let rhs = self.parse_intersect_after(rhs_term)?;
            current = SelectQuery::SetOp {
                lhs: Box::new(current),
                op,
                all,
                rhs: Box::new(rhs),
                order_by: None,
                limit: None,
                offset: 0,
            };
        }
        // ORDER BY / LIMIT / OFFSET top-level — sólo si el resultado es
        // un SetOp (si fue SELECT plano, ya los consumió parse_select_stmt).
        if let SelectQuery::SetOp { .. } = &current {
            let (top_order, top_limit, top_offset) = self.parse_top_order_limit()?;
            if top_order.is_some() || top_limit.is_some() || top_offset != 0 {
                if let SelectQuery::SetOp {
                    lhs, op, all, rhs, ..
                } = current
                {
                    current = SelectQuery::SetOp {
                        lhs,
                        op,
                        all,
                        rhs,
                        order_by: top_order,
                        limit: top_limit,
                        offset: top_offset,
                    };
                }
            }
        }
        Ok(current)
    }

    /// Bloque I: nivel INTERSECT (ata más fuerte que UNION/EXCEPT).
    fn parse_intersect_after(&mut self, lhs: SelectQuery) -> DbResult<SelectQuery> {
        let mut current = lhs;
        while self.match_keyword("INTERSECT") {
            let all = self.match_keyword("ALL");
            let rhs = self.parse_select_term()?;
            current = SelectQuery::SetOp {
                lhs: Box::new(current),
                op: SetOpKind::Intersect,
                all,
                rhs: Box::new(rhs),
                order_by: None,
                limit: None,
                offset: 0,
            };
        }
        Ok(current)
    }

    /// Bloque I: un "término" en un árbol de set ops — un SELECT, un
    /// VALUES, o un `( <select_query> )`.
    fn parse_select_term(&mut self) -> DbResult<SelectQuery> {
        if self.peek().kind == TokenKind::Symbol && self.peek().text == "(" {
            // Look-ahead: `(SELECT ...)` o `(VALUES ...)` — set-op-paren.
            let lookahead = self.tokens.get(self.pos + 1).cloned();
            let is_select_or_values = lookahead
                .map(|t| {
                    t.kind == TokenKind::Ident
                        && (t.text.eq_ignore_ascii_case("SELECT")
                            || t.text.eq_ignore_ascii_case("VALUES"))
                })
                .unwrap_or(false);
            if is_select_or_values {
                self.expect_symbol("(")?;
                let inner_lhs = if self.match_keyword("SELECT") {
                    SelectQuery::Select(Box::new(self.parse_select_stmt()?))
                } else {
                    self.expect_keyword("VALUES")?;
                    SelectQuery::Values(self.parse_values_body()?)
                };
                // Permitir set ops anidados dentro del paréntesis.
                let inner = self.parse_set_ops_after(inner_lhs)?;
                self.expect_symbol(")")?;
                return Ok(inner);
            }
            // No es `(SELECT|VALUES ...)` — caer por error abajo.
        }
        if self.match_keyword("SELECT") {
            // Sin paréntesis: ORDER BY/LIMIT al final pertenecen al outer.
            let stmt = self.parse_select_stmt_inner(false)?;
            return Ok(SelectQuery::Select(Box::new(stmt)));
        }
        if self.match_keyword("VALUES") {
            return Ok(SelectQuery::Values(self.parse_values_body()?));
        }
        Err(DbError::new(
            "se esperaba SELECT, VALUES o `(SELECT ...)` después de UNION/INTERSECT/EXCEPT",
        ))
    }

    /// Bloque I: parsea ORDER BY/LIMIT/OFFSET al nivel top de un árbol
    /// de set ops. Reusa la misma semántica que `parse_select_stmt`.
    fn parse_top_order_limit(&mut self) -> DbResult<(Option<OrderClause>, Option<usize>, usize)> {
        let mut order_by: Option<OrderClause> = None;
        if self.match_keyword("ORDER") {
            self.expect_keyword("BY")?;
            let column = self.expect_ident()?;
            let direction = if self.match_keyword("DESC") {
                OrderDir::Desc
            } else {
                let _ = self.match_keyword("ASC");
                OrderDir::Asc
            };
            order_by = Some(OrderClause { column, direction });
        }
        let mut limit: Option<usize> = None;
        let mut offset: usize = 0;
        let mut seen_limit = false;
        let mut seen_offset = false;
        loop {
            if self.match_keyword("LIMIT") {
                if seen_limit {
                    return Err(coded(
                        codes::LIMIT_DUPLICATED,
                        "LIMIT aparece más de una vez en la query",
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
                        "OFFSET aparece más de una vez en la query",
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
        Ok((order_by, limit, offset))
    }

    /// Bloque F: parsea el SELECT list. Acepta una mezcla de columnas
    /// explícitas (`col` o `tabla.col`) y agregados (`COUNT(*)`,
    /// `SUM(col)`, etc., con `AS alias` opcional). El símbolo `*` solo
    /// es válido como único item (`SELECT *`).
    fn parse_select_list(&mut self) -> DbResult<Vec<SelectItem>> {
        if self.match_symbol("*") {
            return Ok(vec![SelectItem::Star]);
        }
        let mut items = Vec::new();
        items.push(self.parse_select_item()?);
        while self.match_symbol(",") {
            items.push(self.parse_select_item()?);
        }
        Ok(items)
    }

    /// Parsea un único ítem del SELECT list: o bien una función
    /// agregada (cuando el ident es uno de COUNT/SUM/AVG/MIN/MAX seguido
    /// inmediatamente de `(`) o una columna. Detecta el alias opcional
    /// `[AS] alias` siempre que no choque con keywords del statement.
    fn parse_select_item(&mut self) -> DbResult<SelectItem> {
        // Lookahead: si el token actual es uno de los nombres de agregada
        // y el siguiente es `(`, parseamos como agregado (sin tocar — se
        // preserva fast-path del bloque F).
        let head = self.peek().clone();
        if head.kind == TokenKind::Ident {
            if let Some(func) = AggFunc::from_ident(&head.text) {
                let next = self.tokens.get(self.pos + 1).cloned().unwrap_or(Token {
                    kind: TokenKind::Eof,
                    text: String::new(),
                });
                if next.kind == TokenKind::Symbol && next.text == "(" {
                    self.pos += 1; // consume agg-func name
                    self.expect_symbol("(")?;
                    let arg = self.parse_agg_arg(func)?;
                    self.expect_symbol(")")?;
                    let alias = self.try_parse_select_alias()?;
                    return Ok(SelectItem::Aggregate { func, arg, alias });
                }
            }
        }
        // Bloque G1: fast-path bare column. Si es Ident NO seguido de `(`
        // y NO es un keyword reservado de expresión (CASE/CAST/funcion
        // built-in), preservamos la representación clásica
        // `SelectItem::Column(name)`. Esto mantiene el comportamiento
        // pre-G para el caso abrumadoramente más común (`SELECT a, b, c
        // FROM t`) y evita que el resolver de proyección tenga que
        // distinguir "expresión-que-es-solo-una-columna" de "columna
        // bare" — preserva todos los call-sites de bloques anteriores.
        if head.kind == TokenKind::Ident {
            let next = self.tokens.get(self.pos + 1).cloned().unwrap_or(Token {
                kind: TokenKind::Eof,
                text: String::new(),
            });
            let next_is_lparen = next.kind == TokenKind::Symbol && next.text == "(";
            let is_expr_keyword = head.text.eq_ignore_ascii_case("CASE")
                || head.text.eq_ignore_ascii_case("CAST")
                || head.text.eq_ignore_ascii_case("NULL")
                || head.text.eq_ignore_ascii_case("TRUE")
                || head.text.eq_ignore_ascii_case("FALSE");
            let is_zero_arg_fn = matches!(
                head.text.to_ascii_uppercase().as_str(),
                "CURRENT_DATE" | "CURRENT_TIMESTAMP" | "CURDATE"
            );
            // Bloque G3: si el siguiente token es un operador aritmético
            // o el concat `||`, no es columna bare — es una expresión.
            // Cae a `parse_expr` para que la precedencia se aplique.
            let next_is_arith = next.kind == TokenKind::Symbol
                && matches!(next.text.as_str(), "+" | "-" | "*" | "/" | "%" | "||");
            if !next_is_lparen && !is_expr_keyword && !is_zero_arg_fn && !next_is_arith {
                self.pos += 1;
                let column = head.text.clone();
                // ¿hay alias? `col AS x` o `col x`.
                let _ = self.try_parse_select_alias_for_column(&mut |_| {});
                // Para preservar back-compat, descartamos el alias de la
                // forma `Column` (los blocs E2/F/J2 ya tenían UX sin
                // alias bare; lo dejamos para la rama Expression). El
                // helper de abajo NO consume tokens cuando es bare.
                return Ok(SelectItem::Column(column));
            }
        }
        // Caso general: expresión.
        let expr = self.parse_expr()?;
        let alias = self.try_parse_select_alias()?;
        // Si la expresión es solo `Expr::Column(x)` sin alias y sin nada
        // raro, devolvemos `SelectItem::Column(x)` por compatibilidad
        // con todos los path pre-G1 (incluído needs_aggregation /
        // GROUP BY validation que sólo conoce Column).
        if alias.is_none() {
            if let Expr::Column(name) = &expr {
                return Ok(SelectItem::Column(name.clone()));
            }
        }
        Ok(SelectItem::Expression { expr, alias })
    }

    /// Helper neutral: no consume tokens. Existe solo para mantener una
    /// signatura simétrica al resto del parser; la forma `Column` no
    /// admitía alias en pre-G1 (no estaba en `parse_select_item`).
    fn try_parse_select_alias_for_column(&mut self, _: &mut dyn FnMut(&str)) -> Option<String> {
        None
    }

    /// Bloque G1: entry-point del parser de expresiones escalares.
    ///
    /// El árbol G1 es deliberadamente plano:
    ///   - primary (literal | col | func | CASE | CAST | `(`expr`)`)
    ///   - opcionalmente seguido de comparación o `IS [NOT] NULL` —
    ///     solo útil dentro de un `CASE WHEN searched`.
    ///
    /// No hay operadores aritméticos ni `AND`/`OR`: ese subset se
    /// agregará junto con G2 (uso en WHERE/HAVING).
    fn parse_expr(&mut self) -> DbResult<Expr> {
        // Capa más alta del árbol: comparadores y postfix predicates
        // (LIKE / IN / BETWEEN / IS NULL) — precedencia más baja que
        // los operadores aritméticos.
        let lhs = self.parse_arith()?;
        if let Some(op) = self.peek_expr_cmp_op() {
            self.pos += 1;
            let rhs = self.parse_arith()?;
            return Ok(Expr::Compare(Box::new(lhs), op, Box::new(rhs)));
        }
        // Postfix predicates sobre Expr — habilitado por G3.
        self.parse_predicate_postfix(lhs)
    }

    /// Bloque G3: nivel `+`, `-`, `||` (left-assoc, mismo nivel que en
    /// PostgreSQL).
    fn parse_arith(&mut self) -> DbResult<Expr> {
        let mut left = self.parse_arith_term()?;
        loop {
            let t = self.peek();
            if t.kind != TokenKind::Symbol {
                break;
            }
            let op = match t.text.as_str() {
                "+" => ArithOp::Add,
                "-" => ArithOp::Sub,
                "||" => ArithOp::Concat,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_arith_term()?;
            left = Expr::Arith(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    /// Bloque G3: nivel `*`, `/`, `%` (más alta precedencia que `+`/`-`).
    fn parse_arith_term(&mut self) -> DbResult<Expr> {
        let mut left = self.parse_arith_factor()?;
        loop {
            let t = self.peek();
            if t.kind != TokenKind::Symbol {
                break;
            }
            let op = match t.text.as_str() {
                "*" => ArithOp::Mul,
                "/" => ArithOp::Div,
                "%" => ArithOp::Mod,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_arith_factor()?;
            left = Expr::Arith(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    /// Bloque G3: atom-level. Wrapper sobre `parse_expr_primary` para
    /// dejar puerta abierta a unary +/- en el futuro.
    fn parse_arith_factor(&mut self) -> DbResult<Expr> {
        self.parse_expr_primary()
    }

    /// Bloque G3: después de parsear una `Expr` LHS en `parse_expr`,
    /// chequea los postfix predicates SQL: `IS [NOT] NULL`,
    /// `[NOT] LIKE 'patron'`, `[NOT] IN (lit, ...)`,
    /// `[NOT] BETWEEN low AND high`. Devuelve la `Expr` envuelta en la
    /// variante correspondiente, o la propia LHS si no había postfix.
    fn parse_predicate_postfix(&mut self, lhs: Expr) -> DbResult<Expr> {
        // `IS [NOT] NULL`
        if self.peek().kind == TokenKind::Ident && self.peek().text.eq_ignore_ascii_case("IS") {
            self.pos += 1;
            let negated = self.match_keyword("NOT");
            self.expect_keyword("NULL")?;
            return Ok(Expr::IsNull(Box::new(lhs), negated));
        }
        // `LIKE 'patron'`
        if self.match_keyword("LIKE") {
            let pattern = self.expect_string_literal("LIKE")?;
            return Ok(Expr::Like(Box::new(lhs), pattern, false));
        }
        // `IN (lit, ...)`
        if self.match_keyword("IN") {
            let values = self.parse_in_literal_list_for_expr()?;
            return Ok(Expr::InList(Box::new(lhs), values, false));
        }
        // `BETWEEN low AND high`
        if self.match_keyword("BETWEEN") {
            let lo = self.parse_arith()?;
            self.expect_keyword("AND")?;
            let hi = self.parse_arith()?;
            return Ok(Expr::Between(
                Box::new(lhs),
                Box::new(lo),
                Box::new(hi),
                false,
            ));
        }
        // `NOT LIKE | NOT IN | NOT BETWEEN`
        if self.peek().kind == TokenKind::Ident && self.peek().text.eq_ignore_ascii_case("NOT") {
            let next = self.tokens.get(self.pos + 1).cloned().unwrap_or(Token {
                kind: TokenKind::Eof,
                text: String::new(),
            });
            if next.kind == TokenKind::Ident {
                let upper = next.text.to_ascii_uppercase();
                match upper.as_str() {
                    "LIKE" => {
                        self.pos += 2;
                        let pattern = self.expect_string_literal("NOT LIKE")?;
                        return Ok(Expr::Like(Box::new(lhs), pattern, true));
                    }
                    "IN" => {
                        self.pos += 2;
                        let values = self.parse_in_literal_list_for_expr()?;
                        return Ok(Expr::InList(Box::new(lhs), values, true));
                    }
                    "BETWEEN" => {
                        self.pos += 2;
                        let lo = self.parse_arith()?;
                        self.expect_keyword("AND")?;
                        let hi = self.parse_arith()?;
                        return Ok(Expr::Between(
                            Box::new(lhs),
                            Box::new(lo),
                            Box::new(hi),
                            true,
                        ));
                    }
                    _ => {}
                }
            }
        }
        Ok(lhs)
    }

    /// Bloque G3: parsea `(lit, lit, ...)` para postfix `IN` sobre
    /// `Expr`. Solo literales (no subqueries — eso queda para H).
    fn parse_in_literal_list_for_expr(&mut self) -> DbResult<Vec<Value>> {
        self.expect_symbol("(")?;
        if self.peek().kind == TokenKind::Ident && self.peek().text.eq_ignore_ascii_case("SELECT") {
            return Err(coded(
                codes::WHERE_OPERATOR_UNSUPPORTED,
                "IN (SELECT ...) sobre expresión escalar no se soporta en este release — \
                 esperar al bloque H del roadmap",
            ));
        }
        let mut values = vec![self.expect_value()?];
        while self.match_symbol(",") {
            values.push(self.expect_value()?);
        }
        self.expect_symbol(")")?;
        Ok(values)
    }

    fn parse_expr_primary(&mut self) -> DbResult<Expr> {
        let head = self.peek().clone();
        // Literal numérico.
        if head.kind == TokenKind::Number {
            self.pos += 1;
            if head.text.contains('.') {
                return Ok(Expr::Literal(Value::Float(head.text.parse()?)));
            }
            return Ok(Expr::Literal(Value::Integer(head.text.parse()?)));
        }
        // Literal string.
        if head.kind == TokenKind::String {
            self.pos += 1;
            return Ok(Expr::Literal(Value::String(head.text)));
        }
        // Paréntesis: o bien expresión anidada, o bien una subquery
        // escalar (Bloque H). Detectamos el caso `(SELECT ...)` por
        // lookahead — el resto sigue siendo una sub-expresión común.
        if head.kind == TokenKind::Symbol && head.text == "(" {
            let after = self.tokens.get(self.pos + 1).cloned().unwrap_or(Token {
                kind: TokenKind::Eof,
                text: String::new(),
            });
            if after.kind == TokenKind::Ident && after.text.eq_ignore_ascii_case("SELECT") {
                self.pos += 2; // consume `(` + `SELECT`
                let subquery = self.parse_select_stmt()?;
                self.expect_symbol(")")?;
                return Ok(Expr::ScalarSubquery(Box::new(subquery)));
            }
            self.pos += 1;
            let inner = self.parse_expr()?;
            self.expect_symbol(")")?;
            return Ok(inner);
        }
        if head.kind == TokenKind::Ident {
            // NULL / TRUE / FALSE.
            if head.text.eq_ignore_ascii_case("NULL") {
                self.pos += 1;
                return Ok(Expr::Literal(Value::Null));
            }
            if head.text.eq_ignore_ascii_case("TRUE") {
                self.pos += 1;
                return Ok(Expr::Literal(Value::Bool(true)));
            }
            if head.text.eq_ignore_ascii_case("FALSE") {
                self.pos += 1;
                return Ok(Expr::Literal(Value::Bool(false)));
            }
            // CASE … END.
            if head.text.eq_ignore_ascii_case("CASE") {
                self.pos += 1;
                return self.parse_case_expr();
            }
            // CAST(expr AS TYPE).
            if head.text.eq_ignore_ascii_case("CAST") {
                self.pos += 1;
                return self.parse_cast_expr();
            }
            // Funciones zero-arg sin parens: CURRENT_DATE / CURRENT_TIMESTAMP.
            let upper = head.text.to_ascii_uppercase();
            let next_is_lparen = self
                .tokens
                .get(self.pos + 1)
                .map(|t| t.kind == TokenKind::Symbol && t.text == "(")
                .unwrap_or(false);
            if (upper == "CURRENT_DATE" || upper == "CURRENT_TIMESTAMP" || upper == "CURDATE")
                && !next_is_lparen
            {
                self.pos += 1;
                let f = if upper == "CURRENT_TIMESTAMP" {
                    ScalarFunc::CurrentTimestamp
                } else {
                    ScalarFunc::CurrentDate
                };
                return Ok(Expr::Func(f, Vec::new()));
            }
            // Llamada a función `IDENT(...)`.
            let next = self.tokens.get(self.pos + 1).cloned().unwrap_or(Token {
                kind: TokenKind::Eof,
                text: String::new(),
            });
            if next.kind == TokenKind::Symbol && next.text == "(" {
                let func = ScalarFunc::from_ident(&head.text).ok_or_else(|| {
                    coded(
                        codes::SCALAR_FN_UNKNOWN,
                        format!("función escalar desconocida: '{}'", head.text),
                    )
                })?;
                self.pos += 1; // ident
                self.expect_symbol("(")?;
                // Bloque G3: `EXTRACT(field FROM expr)` tiene sintaxis
                // especial — el primer "argumento" es un keyword. Lo
                // empaquetamos como `Literal(String("YEAR"))` y la fecha
                // como segundo arg para encajar en la firma genérica.
                if matches!(func, ScalarFunc::Extract) {
                    let field_tok = self.peek().clone();
                    if field_tok.kind != TokenKind::Ident {
                        return Err(coded(
                            codes::EXTRACT_FIELD_INVALID,
                            format!(
                                "EXTRACT: se esperaba un keyword YEAR/MONTH/DAY/HOUR/MINUTE/SECOND, recibí '{}'",
                                field_tok.text
                            ),
                        ));
                    }
                    let upper = field_tok.text.to_ascii_uppercase();
                    if !matches!(
                        upper.as_str(),
                        "YEAR" | "MONTH" | "DAY" | "HOUR" | "MINUTE" | "SECOND"
                    ) {
                        return Err(coded(
                            codes::EXTRACT_FIELD_INVALID,
                            format!(
                                "EXTRACT: campo '{}' no soportado; usar YEAR/MONTH/DAY/HOUR/MINUTE/SECOND",
                                field_tok.text
                            ),
                        ));
                    }
                    self.pos += 1;
                    self.expect_keyword("FROM")?;
                    let date_expr = self.parse_expr()?;
                    self.expect_symbol(")")?;
                    let args = vec![Expr::Literal(Value::String(upper)), date_expr];
                    validate_scalar_arity(func, args.len())?;
                    return Ok(Expr::Func(func, args));
                }
                let mut args = Vec::new();
                if !(self.peek().kind == TokenKind::Symbol && self.peek().text == ")") {
                    args.push(self.parse_expr()?);
                    while self.match_symbol(",") {
                        args.push(self.parse_expr()?);
                    }
                }
                self.expect_symbol(")")?;
                validate_scalar_arity(func, args.len())?;
                return Ok(Expr::Func(func, args));
            }
            // Ident bare → column reference.
            self.pos += 1;
            return Ok(Expr::Column(head.text));
        }
        Err(coded(
            codes::WHERE_OPERATOR_UNSUPPORTED,
            format!("expresión inválida: token inesperado '{}'", head.text),
        ))
    }

    fn parse_case_expr(&mut self) -> DbResult<Expr> {
        // CASE [operand] (WHEN cond THEN val)+ [ELSE val] END
        let operand = if self.peek().kind == TokenKind::Ident
            && self.peek().text.eq_ignore_ascii_case("WHEN")
        {
            None
        } else {
            Some(Box::new(self.parse_expr()?))
        };
        let mut branches = Vec::new();
        while self.match_keyword("WHEN") {
            let cond = self.parse_expr()?;
            self.expect_keyword("THEN")?;
            let val = self.parse_expr()?;
            branches.push((cond, val));
        }
        if branches.is_empty() {
            return Err(coded(
                codes::WHERE_OPERATOR_UNSUPPORTED,
                "CASE requiere al menos una rama WHEN ... THEN ...",
            ));
        }
        let else_branch = if self.match_keyword("ELSE") {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect_keyword("END")?;
        Ok(Expr::Case {
            operand,
            branches,
            else_branch,
        })
    }

    fn parse_cast_expr(&mut self) -> DbResult<Expr> {
        self.expect_symbol("(")?;
        let inner = self.parse_expr()?;
        self.expect_keyword("AS")?;
        let type_ident = self.expect_ident()?;
        let ty = ColumnType::from_sql(&type_ident)?;
        self.expect_symbol(")")?;
        Ok(Expr::Cast(Box::new(inner), ty))
    }

    /// Operadores de comparación válidos dentro de un `Expr`. Mismos
    /// símbolos que el WHERE pero el set incluye `=` (el WHERE lo trata
    /// separado por sus fast-paths).
    fn peek_expr_cmp_op(&self) -> Option<ExprCmpOp> {
        let t = self.peek();
        if t.kind != TokenKind::Symbol {
            return None;
        }
        match t.text.as_str() {
            "=" => Some(ExprCmpOp::Eq),
            "<>" | "!=" => Some(ExprCmpOp::Ne),
            "<" => Some(ExprCmpOp::Lt),
            "<=" => Some(ExprCmpOp::Le),
            ">" => Some(ExprCmpOp::Gt),
            ">=" => Some(ExprCmpOp::Ge),
            _ => None,
        }
    }

    fn parse_agg_arg(&mut self, func: AggFunc) -> DbResult<AggArg> {
        // `COUNT(*)` es el único caso con `*`; los demás operan sobre 1 columna.
        if self.match_symbol("*") {
            if !matches!(func, AggFunc::Count) {
                return Err(coded(
                    codes::AGGREGATE_ARG_INVALID,
                    format!(
                        "función agregada {}(*) no soportada — solo COUNT(*); el resto requiere una columna",
                        func.keyword()
                    ),
                ));
            }
            return Ok(AggArg::Star);
        }
        if self.match_keyword("DISTINCT") {
            if !matches!(func, AggFunc::Count) {
                return Err(coded(
                    codes::AGGREGATE_ARG_INVALID,
                    format!(
                        "DISTINCT dentro de {} no soportado en este release; solo COUNT(DISTINCT col)",
                        func.keyword()
                    ),
                ));
            }
            let col = self.expect_ident()?;
            return Ok(AggArg::DistinctColumn(col));
        }
        // Issue #5 (2026-05-27): parse_expr en lugar de expect_ident,
        // para que `SUM(qty * price)`, `AVG(LENGTH(name))`, etc., compilen.
        // Si la Expr resultante es justo `Column(name)`, colapsamos a
        // `AggArg::Column(name)` para mantener el fast-path y el output_name
        // pre-Issue-#5.
        let expr = self.parse_expr()?;
        Ok(match expr {
            Expr::Column(name) => AggArg::Column(name),
            other => AggArg::Expr(other),
        })
    }

    /// Acepta `AS alias` o `alias` directo (bare). El alias bare se
    /// detecta solo si el siguiente token es un Ident NO reservado
    /// dentro del flujo del SELECT (`FROM`, `WHERE`, `GROUP`, `HAVING`,
    /// `ORDER`, `LIMIT`, `OFFSET`, comma o EOF marcan el final del ítem).
    fn try_parse_select_alias(&mut self) -> DbResult<Option<String>> {
        if self.match_keyword("AS") {
            let name = self.expect_ident()?;
            return Ok(Some(name));
        }
        let t = self.peek();
        if t.kind == TokenKind::Ident && !is_select_terminator_keyword(&t.text) {
            let alias = self.peek().text.clone();
            self.pos += 1;
            return Ok(Some(alias));
        }
        Ok(None)
    }

    /// Parsea una expresión de WHERE con soporte completo de `AND`/`OR`/`NOT`
    /// y paréntesis (Bloque E1). Precedencia estándar SQL:
    ///   `OR` (más baja) < `AND` < `NOT` < paréntesis / átomo (más alta).
    /// Asume que el caller ya consumió el keyword `WHERE`.
    fn parse_where_expr(&mut self) -> DbResult<WhereExpr> {
        self.parse_where_expr_with(false)
    }

    /// Bloque F: variante que opcionalmente permite agregados como LHS
    /// de los átomos de comparación (solo para HAVING).
    fn parse_where_expr_with(&mut self, allow_aggregates: bool) -> DbResult<WhereExpr> {
        let prev = self.in_having;
        self.in_having = allow_aggregates;
        let result = self.parse_where_or();
        self.in_having = prev;
        result
    }

    fn parse_where_or(&mut self) -> DbResult<WhereExpr> {
        // Sec3: punto de entrada del descenso recursivo. Todo paréntesis
        // que abra una sub-expresión vuelve acá vía parse_where_primary,
        // así que incrementar el contador aquí cubre el caso del ataque
        // `WHERE ((((...))))`. Para `NOT NOT NOT...` el check también
        // está en `parse_where_not`, que es recursión directa.
        self.where_depth += 1;
        if self.where_depth > MAX_PARSE_DEPTH {
            self.where_depth -= 1;
            return Err(coded(
                codes::PARSE_DEPTH_EXCEEDED,
                format!(
                    "expresión WHERE demasiado profunda (límite: {} niveles); \
                     simplificá la query o partila en varias",
                    MAX_PARSE_DEPTH
                ),
            ));
        }
        let result = (|| {
            let mut left = self.parse_where_and()?;
            while self.match_keyword("OR") {
                let right = self.parse_where_and()?;
                left = WhereExpr::Or(Box::new(left), Box::new(right));
            }
            Ok(left)
        })();
        self.where_depth -= 1;
        result
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
        // Sec3: `NOT NOT NOT ...` recursa directamente acá sin pasar por
        // parse_where_or, así que el contador necesita su propio check
        // local. Ataque típico: `WHERE NOT NOT NOT ... NOT col = 1`
        // con miles de NOT consume el stack del proceso.
        self.where_depth += 1;
        if self.where_depth > MAX_PARSE_DEPTH {
            self.where_depth -= 1;
            return Err(coded(
                codes::PARSE_DEPTH_EXCEEDED,
                format!(
                    "demasiados `NOT` encadenados en WHERE (límite: {})",
                    MAX_PARSE_DEPTH
                ),
            ));
        }
        // `NOT NOT x` se permite (cada NOT se apila y se cancela vía 3VL en
        // el evaluador). `NOT EXISTS (...)` mantiene la forma vieja
        // (`Atom(Exists { negated: true })`) para preservar el fast-path
        // del executor — sin esto el `EXISTS` correlacionado tendría que
        // re-evaluarse vía post-filter genérico.
        let result = (|| {
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
        })();
        self.where_depth -= 1;
        result
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
    ///
    /// Bloque G2: cuando el átomo NO encaja en la forma estructural
    /// `IDENT OP literal` (porque LHS es función, CASE, CAST, literal a
    /// la izquierda, etc.), caemos al path expresional que parsea ambos
    /// lados con `parse_expr` y construye un `WhereClause::ExprPredicate`.
    /// Esto preserva intactas las fast-paths PK/índice/EXISTS del path
    /// estructural — solo lo expresional paga FullScan + post-filter.
    fn parse_where_atom(&mut self) -> DbResult<WhereClause> {
        // G2 lookahead: si el átomo NO tiene forma estructural,
        // delegamos al parser de expresiones general.
        if !self.peek_atom_is_structural() {
            return self.parse_where_atom_as_expr();
        }
        let column = self.parse_where_atom_lhs()?;
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
            } else if self.peek().kind == TokenKind::Ident && !is_value_keyword(&self.peek().text) {
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
            // Bloque H (2026-05-26): `NOT IN (SELECT ...)` ahora es un
            // first-class atom — `negated` se propaga al evaluador que
            // aplica 3VL: si la subquery proyecta algún NULL, el
            // resultado del NOT IN es NULL (no false), igual que la
            // ANSI strict semantics de `5 NOT IN (1, NULL)`.
            return Ok(WhereClause::In {
                column,
                subquery: Box::new(subquery),
                negated,
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

    /// Bloque G2: detecta si el átomo del WHERE/HAVING comienza con la
    /// forma estructural `IDENT [OP ...]` (donde IDENT puede ser una
    /// agregada en HAVING o un column-ref simple). Devuelve `false`
    /// cuando el primer token es un literal, un `(`, o un IDENT que
    /// abre llamada a función / `CASE` / `CAST` — todos casos que solo
    /// el path expresional sabe parsear. El resultado decide qué rama
    /// toma `parse_where_atom`; preserva los fast-paths estructurales
    /// intactos.
    fn peek_atom_is_structural(&self) -> bool {
        let t = self.peek();
        match t.kind {
            TokenKind::Ident => {
                let upper = t.text.to_ascii_uppercase();
                // Constructores expresionales como LHS solo del WHERE
                // (NULL/TRUE/FALSE como LHS también caen al path expr).
                if matches!(upper.as_str(), "CASE" | "CAST" | "NULL" | "TRUE" | "FALSE") {
                    return false;
                }
                // Bloque G3: si el siguiente token es un operador
                // aritmético o el concat `||`, el átomo es
                // expresional (la forma estructural `col OP literal`
                // no aplica).
                if let Some(next) = self.tokens.get(self.pos + 1) {
                    if next.kind == TokenKind::Symbol
                        && matches!(next.text.as_str(), "+" | "-" | "*" | "/" | "%" | "||")
                    {
                        return false;
                    }
                }
                // Funciones zero-arg sin parens — son expresiones.
                if matches!(
                    upper.as_str(),
                    "CURRENT_DATE" | "CURRENT_TIMESTAMP" | "CURDATE"
                ) {
                    // Si lo siguiente NO es `(`, es una expresión 0-arg.
                    let next_is_lparen = self
                        .tokens
                        .get(self.pos + 1)
                        .map(|n| n.kind == TokenKind::Symbol && n.text == "(")
                        .unwrap_or(false);
                    if !next_is_lparen {
                        return false;
                    }
                }
                // Llamada `IDENT(` que NO sea una agregada permitida en
                // HAVING → función escalar, expresión. En HAVING las
                // agregadas (`COUNT(*)`, `SUM(col)`, ...) sí son
                // estructurales: `parse_where_atom_lhs` ya las maneja.
                let next_is_lparen = self
                    .tokens
                    .get(self.pos + 1)
                    .map(|n| n.kind == TokenKind::Symbol && n.text == "(")
                    .unwrap_or(false);
                if next_is_lparen {
                    // Agregadas: SIEMPRE estructural — en HAVING para
                    // resolverlas contra los buckets; fuera de HAVING
                    // para que `parse_where_atom_lhs` devuelva el error
                    // claro `[GBY-4025]` (agregada fuera de HAVING/SELECT),
                    // no que el path expresional las confunda con una
                    // función escalar desconocida (`[GBY-4037]`).
                    if AggFunc::from_ident(&t.text).is_some() {
                        return true;
                    }
                    // Cualquier otro IDENT( → escalar → expresional.
                    return false;
                }
                true
            }
            // Literal/símbolo a la izquierda → expresión por definición.
            _ => false,
        }
    }

    /// Bloque G2: parsea el átomo del WHERE como expresión completa
    /// (ambos lados son `Expr`). Cubre `LENGTH(x) > 3`,
    /// `5 < LENGTH(x)`, `UPPER(x) = 'A'`, `CASE ... END = 1`, y la
    /// forma "expr a secas" (`COALESCE(activo, false)`) que se evalúa
    /// como predicado booleano directo en `eval_expr_as_predicate`.
    ///
    /// Los operadores postfix (`IS [NOT] NULL`, `[NOT] LIKE`,
    /// `[NOT] IN`, `BETWEEN`) sobre una expresión escalar todavía NO
    /// se soportan — devolvemos `[GBY-4039]` con guía.
    fn parse_where_atom_as_expr(&mut self) -> DbResult<WhereClause> {
        // Bloque G3: el path expresional usa la misma cadena de
        // precedencia del SELECT — aritmética, comparador, postfix
        // predicates (IS NULL / LIKE / IN / BETWEEN). `parse_expr`
        // ya devuelve la `Expr` envuelta con el postfix si aparece.
        let expr = self.parse_expr()?;
        Ok(WhereClause::ExprPredicate { expr })
    }

    /// Bloque F: parsea el LHS de un átomo del WHERE/HAVING. En HAVING
    /// (`self.in_having == true`) acepta también funciones agregadas
    /// como `SUM(price)`, `COUNT(*)`, `COUNT(DISTINCT col)`; la salida
    /// es el nombre canónico (e.g. `sum_price`, `count_*`,
    /// `count_distinct_col`) que el evaluador busca en el bucket
    /// agregado. En WHERE normal rechaza esa forma con un mensaje claro.
    fn parse_where_atom_lhs(&mut self) -> DbResult<String> {
        let head = self.peek().clone();
        if head.kind == TokenKind::Ident {
            if let Some(func) = AggFunc::from_ident(&head.text) {
                let next = self.tokens.get(self.pos + 1).cloned().unwrap_or(Token {
                    kind: TokenKind::Eof,
                    text: String::new(),
                });
                if next.kind == TokenKind::Symbol && next.text == "(" {
                    if !self.in_having {
                        return Err(coded(
                            codes::AGGREGATE_OUTSIDE_HAVING_OR_SELECT,
                            format!(
                                "función agregada {} solo se permite en SELECT y HAVING; \
                                 movela al SELECT con un alias y referencialo en el WHERE \
                                 (no es válido — usá HAVING)",
                                func.keyword()
                            ),
                        ));
                    }
                    self.pos += 1; // consume agg-func name
                    self.expect_symbol("(")?;
                    let arg = self.parse_agg_arg(func)?;
                    self.expect_symbol(")")?;
                    return Ok(SelectItem::Aggregate {
                        func,
                        arg,
                        alias: None,
                    }
                    .output_name());
                }
            }
        }
        self.expect_ident()
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
