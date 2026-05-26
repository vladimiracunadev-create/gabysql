//! Numbered error codes for gabysql's user-facing errors.
//!
//! Inspired by MySQL's `ER_*` codes (e.g. `ER_DUP_ENTRY = 1062`): every
//! error that a CLI user, an HTTP client or an embedded caller can act
//! on carries a stable numeric ID rendered as `[GBY-NNNN]` at the head
//! of the message. Tools and runbooks can grep for the ID instead of
//! parsing free text, and the catalog at [`docs/ERROR_CODES.md`]
//! documents every code with cause and remediation.
//!
//! ## Design (and why not JSON)
//!
//! All codes live in this module as `pub const` constants. They compile
//! into the binary, the compiler enforces uniqueness across renames,
//! and there is no runtime I/O to load them. The alternative of an
//! external JSON file was rejected:
//!
//! - It would add a runtime dependency on a filesystem location (where
//!   does the binary look for `error_messages.json`? per-OS path
//!   conventions, packaging headaches).
//! - It would add a failure mode at startup ("error file missing"
//!   becomes a new class of failure that itself has to be reported).
//! - It would violate the zero-deps embedded posture of [ADR-0001].
//! - It would not be more flexible in practice: changing a message
//!   today is a code edit, a re-build and a re-deploy whether the
//!   message lives in `.rs` or in `.json`.
//!
//! [ADR-0001]: ../docs/adr/0001-rust-zero-deps-core.md
//!
//! ## Stability contract
//!
//! - **Codes are stable.** Once published in a release, a code never
//!   changes its meaning and never gets reused after deletion.
//! - **New codes append.** Adding a code is non-breaking; the catalog
//!   in `docs/ERROR_CODES.md` grows.
//! - **Messages can evolve.** The numeric code is the contract; the
//!   human text after the prefix can be rephrased, translated, or
//!   enriched without bumping the catalog.
//!
//! ## How to add a new code
//!
//! 1. Pick the next unused number in the right thousand-range (see the
//!    `codes` module for ranges).
//! 2. Add a `pub const` here with a symbolic SCREAMING_SNAKE name.
//! 3. Add a row to [`docs/ERROR_CODES.md`] in the matching section.
//! 4. Wrap the `DbError::new(...)` call with [`coded`].
//! 5. The PR checklist in [`docs/ERROR_HANDLING.md`] also applies.

use crate::DbError;

/// Numeric ranges for [`codes`]:
///
/// | Range          | Subsystem                                    |
/// | -------------- | -------------------------------------------- |
/// | `1000`–`1999`  | Storage / Pager / WAL / file lock            |
/// | `2000`–`2999`  | Catalog / schema / identifiers               |
/// | `3000`–`3999`  | Constraints (PK, NOT NULL, UNIQUE, FK)       |
/// | `4000`–`4999`  | SQL surface (parser, planner, limitations)   |
/// | `5000`–`5999`  | Server / HTTP / auth                         |
pub mod codes {
    // ---------- Storage (1000s) ----------
    /// `Pager::create` rehúsa sobrescribir una DB existente.
    pub const REFUSE_OVERWRITE_DB: u32 = 1001;
    /// `Pager::create/open` no pudo adquirir el lock exclusivo cross-process.
    pub const DB_LOCKED_BY_PROCESS: u32 = 1002;
    /// El archivo declara una `VERSION` no soportada por este binario.
    pub const UNSUPPORTED_FORMAT_VERSION: u32 = 1003;
    /// Magic bytes ≠ `GABYSQL1`: el archivo no es una DB gabysql.
    pub const BAD_MAGIC_BYTES: u32 = 1004;
    /// `begin()` llamado mientras ya hay una transacción abierta.
    pub const TX_ALREADY_STARTED: u32 = 1005;
    /// `commit()` / `rollback()` llamado sin transacción activa.
    pub const NO_ACTIVE_TX: u32 = 1006;
    /// Página corrupta detectada por el trailer CRC32 al leer del `.db`.
    pub const PAGE_CRC_INVALID: u32 = 1007;
    /// Record del WAL con CRC inválido durante el replay.
    pub const WAL_RECORD_CRC_INVALID: u32 = 1008;
    /// `page_size` declarado en el header no coincide con el de este build.
    pub const UNSUPPORTED_PAGE_SIZE: u32 = 1009;

    // ---------- Catalog / Schema (2000s) ----------
    /// La tabla solicitada no existe en el catálogo.
    pub const TABLE_NOT_FOUND: u32 = 2001;
    /// La columna referenciada no existe en la tabla.
    pub const COLUMN_NOT_FOUND: u32 = 2002;
    /// El índice referenciado no existe en el catálogo.
    pub const INDEX_NOT_FOUND: u32 = 2003;
    /// `CREATE TABLE` sobre un nombre ya existente.
    pub const TABLE_ALREADY_EXISTS: u32 = 2004;
    /// `CREATE INDEX` sobre un nombre ya existente.
    pub const INDEX_ALREADY_EXISTS: u32 = 2005;
    /// Identificador inválido (largo, reservado, caracteres prohibidos).
    pub const INVALID_IDENTIFIER: u32 = 2006;
    /// Más de una columna comparte el mismo nombre en `CREATE TABLE`.
    pub const DUPLICATE_COLUMN_NAME: u32 = 2007;
    /// El tipo del `DEFAULT` declarado es incompatible con el tipo de columna.
    pub const INCOMPATIBLE_DEFAULT_TYPE: u32 = 2008;
    /// `INDEX` solicitado sobre una columna `JSON` (no admitido).
    pub const INDEX_ON_JSON: u32 = 2009;

    // ---------- Constraints (3000s) ----------
    /// `INSERT` con una PK que ya existe en la tabla.
    pub const DUPLICATE_PRIMARY_KEY: u32 = 3001;
    /// `INSERT`/`UPDATE` con `NULL` en una columna `NOT NULL`.
    pub const NOT_NULL_VIOLATED: u32 = 3002;
    /// `INSERT`/`UPDATE` viola un constraint `UNIQUE`.
    pub const UNIQUE_VIOLATED: u32 = 3003;
    /// `INSERT`/`UPDATE` con un valor de FK que no existe en el padre.
    pub const FK_PARENT_MISSING: u32 = 3004;
    /// `DELETE` bloqueado por una FK con `ON DELETE RESTRICT`.
    pub const FK_RESTRICT_BLOCKS_DELETE: u32 = 3005;
    /// `UPDATE`/`DELETE` sobre una PK que no existe.
    pub const ROW_NOT_FOUND_FOR_PK: u32 = 3006;
    /// `PRIMARY KEY` recibió valor `NULL`.
    pub const PRIMARY_KEY_NULL: u32 = 3007;

    // ---------- SQL Surface (4000s) ----------
    /// `WHERE` con operador no soportado (solo `=`, `BETWEEN` e `IN (SELECT ...)`).
    pub const WHERE_OPERATOR_UNSUPPORTED: u32 = 4001;
    /// `BETWEEN` sobre columna que no es PK ni `INT`-indexada.
    pub const BETWEEN_REQUIRES_PK_OR_INT_INDEX: u32 = 4002;
    /// `UPDATE`/`DELETE` sin filtro `WHERE pk = N`.
    pub const UPDATE_DELETE_REQUIRES_PK_FILTER: u32 = 4003;
    /// `LIMIT` negativo.
    pub const LIMIT_NEGATIVE: u32 = 4004;
    /// `OFFSET` negativo.
    pub const OFFSET_NEGATIVE: u32 = 4005;
    /// String literal sin cerrar (comilla simple no balanceada).
    pub const STRING_LITERAL_UNTERMINATED: u32 = 4006;
    /// `INSERT` con cantidad de columnas distinta a cantidad de valores.
    pub const INSERT_COLS_VS_VALUES_MISMATCH: u32 = 4007;
    /// `UPDATE` que intenta mutar la PK.
    pub const UPDATE_PK_NOT_ALLOWED: u32 = 4008;
    /// `LIMIT` aparece más de una vez en la query.
    pub const LIMIT_DUPLICATED: u32 = 4009;
    /// `OFFSET` aparece más de una vez en la query.
    pub const OFFSET_DUPLICATED: u32 = 4010;
    /// Subquery dentro de `IN (...)` proyecta más (o menos) de una columna.
    pub const SUBQUERY_MUST_RETURN_ONE_COLUMN: u32 = 4011;
    /// Valor no-INT propuesto para una PK `INT` dentro de un `IN (SELECT ...)`.
    pub const IN_PK_TYPE_MISMATCH: u32 = 4012;
    /// `WHERE col IN (SELECT ...)` cuando `col` no es PK ni tiene índice secundario.
    pub const IN_REQUIRES_PK_OR_INDEX: u32 = 4013;
    /// Subquery escalar `= (SELECT ...)` que devuelve más de una fila.
    pub const SCALAR_SUBQUERY_TOO_MANY_ROWS: u32 = 4014;
    /// `EXISTS` / `NOT EXISTS` sin `(SELECT ...)` válido a continuación.
    pub const EXISTS_REQUIRES_SUBQUERY: u32 = 4015;
    /// Referencia a una columna del outer (`outer_tbl.col`) usada fuera de una
    /// subquery correlacionada, o apuntando a una tabla/columna que el
    /// outer-stack no provee.
    pub const OUTER_COLUMN_REF_INVALID: u32 = 4016;
    /// Dos tablas del FROM expuestas con el mismo alias (o el mismo nombre
    /// sin alias). Imposible resolver columnas no-cualificadas.
    pub const TABLE_ALIAS_DUPLICATED: u32 = 4017;
    /// Columna sin qualifier que aparece en más de una tabla del FROM.
    /// Hace falta `tabla.col` para des-ambiguar.
    pub const COLUMN_AMBIGUOUS: u32 = 4018;
    /// `tabla.col` donde `tabla` no es ni nombre ni alias de ninguna tabla
    /// del FROM, o `col` que no existe en ninguna tabla del FROM.
    pub const COLUMN_QUALIFIER_NOT_FOUND: u32 = 4019;
    /// `INNER JOIN ...` sin cláusula `ON`. En este bloque toda forma INNER
    /// exige predicado (CROSS JOIN es la forma sin predicado).
    pub const JOIN_PREDICATE_REQUIRED: u32 = 4020;
    /// `CROSS JOIN ... ON ...` — la cartesian product no admite predicado;
    /// usar `INNER JOIN` en su lugar.
    pub const CROSS_JOIN_WITH_ON: u32 = 4021;
    /// `JOIN ... USING (col)` donde `col` no existe en ambas tablas, o el
    /// USING soporta más columnas de las que este release acepta.
    pub const USING_COLUMN_INVALID: u32 = 4022;
    /// `NATURAL JOIN` cuyas tablas no comparten exactamente una columna por
    /// nombre. 0 columnas en común o >1 → error explícito en este release.
    pub const NATURAL_JOIN_NO_COMMON_COLUMN: u32 = 4023;
    /// Predicado correlacionado (`EXISTS (… outer.col …)` o `col = outer.col`)
    /// envuelto en `AND`/`OR`/`NOT` (Bloque E1). El dispatch correlacionado
    /// solo se aplica cuando el predicado correlacionado es el único átomo
    /// del WHERE; combinarlo con boolean operators queda para un bloque
    /// posterior.
    pub const WHERE_COMBINATOR_CORRELATED_UNSUPPORTED: u32 = 4024;
    /// Función agregada usada fuera del SELECT list o HAVING (Bloque F).
    /// `SUM(x) > 10` en `WHERE` o en `ORDER BY` no se acepta; debe ir en
    /// `HAVING` o aliasearse en el SELECT y referirse por alias.
    pub const AGGREGATE_OUTSIDE_HAVING_OR_SELECT: u32 = 4025;
    /// Argumento inválido de función agregada (Bloque F). Casos:
    /// `SUM(*)`, `AVG(DISTINCT x)`, `MIN(*)`, etc. Solo `COUNT(*)` y
    /// `COUNT(DISTINCT col)` son combinaciones especiales aceptadas.
    pub const AGGREGATE_ARG_INVALID: u32 = 4026;
    /// `SELECT` mezcla columnas no-agregadas con agregadas sin que las
    /// no-agregadas figuren en el `GROUP BY` (Bloque F). Cumple la regla
    /// ANSI estricta. Solución: agregar la columna al `GROUP BY`, o
    /// envolverla en una agregada (`MIN(col)` / `MAX(col)`).
    pub const SELECT_COLUMN_NOT_IN_GROUP_BY: u32 = 4027;
    /// `GROUP BY` / agregados sobre `SELECT` con JOIN (Bloque F).
    /// El executor de JOIN aún no implementa el stage de agregación;
    /// reescribir como subquery sobre tabla única o esperar al bloque
    /// posterior que extienda F a multi-tabla.
    pub const AGGREGATE_OVER_JOIN_UNSUPPORTED: u32 = 4028;
    /// `BEGIN` SQL emitido cuando ya hay una transacción explícita
    /// abierta (Bloque T). Savepoints no soportados todavía — la única
    /// forma de salir es `COMMIT` o `ROLLBACK`.
    pub const TX_BEGIN_DOUBLE: u32 = 4029;
    /// `COMMIT` o `ROLLBACK` SQL emitido sin `BEGIN` previo (Bloque T).
    /// Las sentencias fuera de un bloque explícito son auto-commit por
    /// batch — no hace falta cerrarlas manualmente.
    pub const TX_END_WITHOUT_BEGIN: u32 = 4030;
    /// Cláusula `ON CONFLICT` con acción no soportada o malformada
    /// (Bloque J2). Acciones aceptadas: `DO NOTHING`, `DO UPDATE SET ...`.
    /// `REPLACE` solo se obtiene vía `REPLACE INTO ...`.
    pub const ON_CONFLICT_INVALID: u32 = 4031;
    /// `ON CONFLICT (col)` cuyo `col` no es PK ni tiene UNIQUE
    /// (Bloque J2). Sin un constraint indexado el motor no puede
    /// detectar el conflicto.
    pub const ON_CONFLICT_TARGET_NOT_UNIQUE: u32 = 4032;
    /// Sec3 (2026-05-25): expresión SQL con anidamiento mayor al
    /// permitido por el parser. Defensa contra ataques de stack
    /// exhaustion con paréntesis o `NOT` encadenados sin fin
    /// (CWE-674). El límite duro está en `MAX_PARSE_DEPTH` dentro
    /// de `sql.rs`.
    pub const PARSE_DEPTH_EXCEEDED: u32 = 4033;

    // ---------- Server / HTTP (5000s) ----------
    /// Falta el parámetro `?db=...` en una request multi-DB.
    pub const MISSING_DB_PARAM: u32 = 5001;
    /// Falta el parámetro `?table=...` en `/schema` o `/rows`.
    pub const MISSING_TABLE_PARAM: u32 = 5002;
    /// El nombre de DB contiene separadores de path u otros caracteres prohibidos.
    pub const INVALID_DB_NAME: u32 = 5003;
    /// Token de autenticación inválido o ausente.
    pub const UNAUTHORIZED: u32 = 5004;
    /// Server saturado: el cap de conexiones concurrentes está al máximo.
    pub const SERVER_BUSY: u32 = 5005;
    /// El server arrancó en modo single-DB; la operación requería `-dir`.
    pub const SERVER_NOT_MULTI_DB: u32 = 5006;
    /// Sec1 (2026-05-25): HTTP request con `Content-Length` mayor al
    /// permitido (`MAX_REQUEST_BODY_BYTES`). Defensa contra DoS por
    /// memory exhaustion (CWE-400).
    pub const REQUEST_BODY_TOO_LARGE: u32 = 5007;
}

/// Build a `DbError` with the `[GBY-NNNN]` prefix. See module docs for
/// the stability contract and the catalog at `docs/ERROR_CODES.md`.
///
/// ```ignore
/// return Err(coded(codes::TABLE_NOT_FOUND, format!(
///     "tabla no existe: '{}'", name
/// )));
/// // → "[GBY-2001] tabla no existe: 'orders'"
/// ```
pub fn coded(code: u32, message: impl Into<String>) -> DbError {
    DbError::new(format!("[GBY-{:04}] {}", code, message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coded_prepends_id_with_four_digit_width() {
        let err = coded(codes::TABLE_NOT_FOUND, "tabla no existe: 'orders'");
        assert_eq!(err.to_string(), "[GBY-2001] tabla no existe: 'orders'");
    }

    #[test]
    fn low_codes_are_zero_padded() {
        // Hypothetical low code — uses the same formatting.
        let err = coded(42, "ejemplo");
        assert_eq!(err.to_string(), "[GBY-0042] ejemplo");
    }

    #[test]
    fn codes_are_in_documented_ranges() {
        // Storage 1000s
        assert!((1000..2000).contains(&codes::REFUSE_OVERWRITE_DB));
        assert!((1000..2000).contains(&codes::DB_LOCKED_BY_PROCESS));
        // Catalog 2000s
        assert!((2000..3000).contains(&codes::TABLE_NOT_FOUND));
        assert!((2000..3000).contains(&codes::INDEX_ON_JSON));
        // Constraints 3000s
        assert!((3000..4000).contains(&codes::DUPLICATE_PRIMARY_KEY));
        assert!((3000..4000).contains(&codes::PRIMARY_KEY_NULL));
        // SQL 4000s
        assert!((4000..5000).contains(&codes::WHERE_OPERATOR_UNSUPPORTED));
        assert!((4000..5000).contains(&codes::OFFSET_DUPLICATED));
        // Server 5000s
        assert!((5000..6000).contains(&codes::MISSING_DB_PARAM));
        assert!((5000..6000).contains(&codes::SERVER_NOT_MULTI_DB));
    }
}
