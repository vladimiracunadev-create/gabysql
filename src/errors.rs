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
    /// Bloque L (2026-05-27): `INSERT`/`UPDATE`/`UPSERT` viola un
    /// constraint `CHECK (expr)`. La expresión evaluó a FALSE
    /// (NULL pasa según 3VL ANSI, igual que en PostgreSQL/SQLite).
    pub const CHECK_VIOLATED: u32 = 3008;
    /// Bloque L: `ON DELETE SET NULL` intentó poner NULL en una FK
    /// child cuya columna está declarada `NOT NULL`. La cascade aborta
    /// la operación entera (no hay rollback parcial).
    pub const FK_SET_NULL_VIOLATES_NOT_NULL: u32 = 3009;
    /// Bloque L: `ON DELETE SET DEFAULT` no encontró un DEFAULT
    /// declarado para la columna FK del child. Sin DEFAULT no hay valor
    /// para reasignar; la cascade aborta.
    pub const FK_SET_DEFAULT_MISSING: u32 = 3010;

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
    /// Reservado histórico. Hasta el bloque H (2026-05-26) este código
    /// se emitía cuando un predicado correlacionado (`EXISTS (… outer.col …)`
    /// o `col = outer.col`) aparecía envuelto en `AND`/`OR`/`NOT`. H1
    /// habilita `EXISTS` correlacionado dentro de combinadores; el
    /// código se conserva por estabilidad del catálogo (ADR de la sec. ##
    /// Stability contract) pero el motor ya no lo genera.
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
    /// Bloque G1 (2026-05-26): función escalar invocada con la cantidad
    /// equivocada de argumentos (e.g. `LENGTH()` o `SUBSTR(s)`).
    pub const SCALAR_FN_ARITY: u32 = 4034;
    /// Bloque G1: argumento de una función escalar con un tipo que la
    /// función no acepta (e.g. `LENGTH(123)` o `ABS('x')`).
    pub const SCALAR_FN_TYPE_MISMATCH: u32 = 4035;
    /// Bloque G1: `CAST(x AS TYPE)` cuyo valor no se puede convertir al
    /// tipo destino (e.g. `CAST('abc' AS INT)`).
    pub const CAST_INVALID: u32 = 4036;
    /// Bloque G1: invocación a una función escalar que el motor no
    /// reconoce (e.g. `FOO(1)`).
    pub const SCALAR_FN_UNKNOWN: u32 = 4037;
    /// Bloque G1: condición de un `CASE` searched (`CASE WHEN cond …`)
    /// que no evalúa a BOOL.
    pub const CASE_BRANCH_TYPE_MISMATCH: u32 = 4038;
    /// Bloque G2 (2026-05-26): los operadores postfix `IS [NOT] NULL`,
    /// `[NOT] LIKE`, `[NOT] IN (...)` y `BETWEEN` aún no aceptan una
    /// expresión escalar como LHS — solo columnas simples. La forma
    /// expresional (`LENGTH(x) IS NULL`, `UPPER(x) LIKE 'A%'`, etc.)
    /// queda para un bloque posterior.
    pub const EXPR_IN_PREDICATE_NOT_SUPPORTED: u32 = 4039;
    /// Bloque G2: una expresión escalar usada como predicado completo del
    /// WHERE/HAVING no evaluó a BOOL (ni NULL en 3VL). Caso típico:
    /// `WHERE LENGTH(name)` sin comparar contra nada — falta el operador.
    pub const WHERE_EXPR_NOT_BOOLEAN: u32 = 4040;
    /// Bloque G2: el valor calculado para un `UPDATE ... SET col = <expr>`
    /// no encaja en el tipo declarado de la columna (e.g. asignar TEXT a
    /// una columna INT). Lo dispara el encoder al rechazar el cast
    /// implícito; con `SET col = CAST(... AS T)` se evita.
    pub const UPDATE_SET_TYPE_MISMATCH: u32 = 4041;
    /// Bloque G3 (2026-05-26): aritmético entero produjo overflow en
    /// `checked_add` / `_sub` / `_mul` / `_div` (e.g. `i64::MAX + 1`).
    /// Antes de G3 no había operadores binarios; ahora `+`, `-`, `*`,
    /// `/`, `%` sobre INT pueden disparar este código.
    pub const ARITH_OVERFLOW: u32 = 4042;
    /// Bloque G3: división o módulo cuyo divisor evaluó a cero
    /// (entero o flotante). gabysql elige error explícito en vez de
    /// devolver `±Inf`/`NaN` para no contaminar resultados aguas abajo.
    pub const DIVISION_BY_ZERO: u32 = 4043;
    /// Bloque G3: operador aritmético o `||` aplicado a tipos que no
    /// admiten esa combinación (e.g. `'abc' + 1`, `true * 2`,
    /// `BOOL || INT`). NULL en cualquiera de los lados NO dispara esto
    /// — propaga NULL via 3VL.
    pub const ARITH_TYPE_MISMATCH: u32 = 4044;
    /// Bloque G3: función matemática llamada con un argumento fuera
    /// del dominio (e.g. `SQRT(-1)`, `POWER(0, -1)`).
    pub const MATH_DOMAIN: u32 = 4045;
    /// Bloque G3: función de fecha (`DATE_ADD`, `DATEDIFF`, `EXTRACT`,
    /// `STRFTIME`, ...) recibió un string que no parsea como
    /// `YYYY-MM-DD` ni `YYYY-MM-DD HH:MM:SS`.
    pub const DATE_PARSE_ERROR: u32 = 4046;
    /// Bloque G3: `EXTRACT(<field> FROM ...)` con un campo que no es
    /// `YEAR`/`MONTH`/`DAY`/`HOUR`/`MINUTE`/`SECOND`.
    pub const EXTRACT_FIELD_INVALID: u32 = 4047;
    /// Bloque H (2026-05-26): `FROM (SELECT ...)` sin alias. ANSI exige
    /// que toda derived table tenga un nombre para poder referenciar sus
    /// columnas; gabysql lo aplica estrictamente para que el resolver de
    /// columnas no tenga ambigüedades.
    pub const DERIVED_TABLE_REQUIRES_ALIAS: u32 = 4048;
    /// Bloque H: `FROM (SELECT a, a FROM t) AS d` — la subquery de una
    /// derived table proyecta dos columnas con el mismo nombre/alias.
    /// Sin nombres únicos, el outer no puede referenciar las columnas
    /// sin colisión.
    pub const DERIVED_DUPLICATE_COLUMN: u32 = 4049;
    /// Bloque H: una columna de una derived table mezcla valores de
    /// tipos incompatibles (e.g. INT + TEXT en un UNION simulado). El
    /// schema virtual cae a TEXT como compromiso pero deja este código
    /// reservado para validaciones futuras más estrictas.
    pub const DERIVED_COLUMN_TYPE_AMBIGUOUS: u32 = 4050;
    /// Bloque H: subquery escalar en el SELECT list / Expr sin
    /// paréntesis envolventes. `SELECT SELECT x FROM t` no se acepta;
    /// debe ir `(SELECT x FROM t)`.
    pub const SCALAR_SUBQUERY_IN_EXPR_REQUIRES_PARENS: u32 = 4051;
    /// Bloque I (2026-05-26): `FROM (VALUES (...), ...)` sin alias. La
    /// cláusula VALUES no provee nombres por sí misma, por lo que ANSI
    /// exige un alias de tabla (`AS t`). gabysql además exige el alias
    /// de columnas (`AS t(c1, c2, ...)`) para que el outer pueda
    /// referenciarlas — sin esa lista, el resolver no tiene names.
    pub const VALUES_IN_FROM_REQUIRES_ALIAS: u32 = 4052;
    /// Bloque I: la lista de aliases de columna de una `VALUES` en FROM
    /// (`AS t(c1, c2, ...)`) tiene una arity distinta a las tuplas, o
    /// dos tuplas de un mismo `VALUES` tienen arity distinta entre sí.
    /// Ambos casos rompen el shape rectangular de la tabla virtual.
    pub const VALUES_COLUMN_ALIAS_ARITY: u32 = 4053;
    /// Bloque I: una operación de conjunto (`UNION` / `INTERSECT` /
    /// `EXCEPT`) entre dos queries cuyo número de columnas no coincide.
    /// ANSI exige `arity(lhs) == arity(rhs)`.
    pub const SET_OP_ARITY_MISMATCH: u32 = 4054;
    /// Bloque I: una operación de conjunto entre dos queries cuyas
    /// columnas, en alguna posición, tienen tipos no compatibles. INT
    /// y FLOAT promueven; el resto exige match exacto (o NULL).
    pub const SET_OP_TYPE_MISMATCH: u32 = 4055;
    /// Bloque I: dos filas de un mismo `VALUES (..), (..)` tienen
    /// distinto número de expresiones. Toda fila debe tener la misma
    /// arity (la del primer row).
    pub const VALUES_ROW_ARITY_MISMATCH: u32 = 4056;
    /// Bloque I: `VALUES` sin ninguna fila — sintaxis inválida.
    pub const VALUES_EMPTY: u32 = 4057;
    /// Bloque K1 (2026-05-26): `CREATE TABLE <t> AS SELECT ...` cuya
    /// primera columna del result-set no es INT (o admite NULL). En este
    /// release gabysql exige que la primera columna proyectada del
    /// SELECT sirva como PRIMARY KEY de la tabla destino (única estrategia
    /// disponible: PK escalar INT). Soluciones: anteponer un `id INT` en
    /// el SELECT, o reescribir la fuente con un `ROW_NUMBER()` materializado
    /// como columna INT — pendiente para un release posterior.
    pub const CTAS_REQUIRES_INT_FIRST_COLUMN: u32 = 4058;
    /// Bloque K1: `ALTER TABLE <t> DROP COLUMN <c>` rechazado porque `c`
    /// es la PRIMARY KEY de la tabla. La PK no se puede borrar; usar
    /// `DROP TABLE` si la intención es rehacer el esquema.
    pub const CANNOT_DROP_PRIMARY_KEY: u32 = 4059;
    /// Bloque K1: `ALTER TABLE <t> DROP COLUMN <c>` rechazado porque `c`
    /// tiene un índice asociado (`CREATE INDEX` o `UNIQUE` inline). Hay
    /// que ejecutar `DROP INDEX <name>` antes para desreferenciar la
    /// columna.
    pub const CANNOT_DROP_INDEXED_COLUMN: u32 = 4060;
    /// Bloque K1: `ALTER TABLE <t> DROP COLUMN <c>` rechazado porque `c`
    /// participa en una FOREIGN KEY — saliente (la columna es FK hacia
    /// otra tabla) o entrante (otra tabla apunta a esta columna como su
    /// parent). Hay que recrear/eliminar la FK antes.
    pub const CANNOT_DROP_REFERENCED_COLUMN: u32 = 4061;
    /// Bloque K1: `ALTER TABLE <t> RENAME COLUMN <a> TO <b>` o
    /// `RENAME TABLE <a> TO <b>` cuando `b` ya existe (otra columna /
    /// otra tabla). El motor no auto-sobrescribe: hay que elegir un
    /// nombre libre.
    pub const RENAME_TARGET_EXISTS: u32 = 4062;
    /// Bloque K1: `CREATE TABLE t (c1, c2, ...) AS SELECT ...` cuya
    /// lista de alias de columnas tiene una arity distinta a las
    /// columnas que el SELECT proyecta.
    pub const CTAS_COLUMN_ALIAS_ARITY: u32 = 4063;
    /// Bloque K2 (2026-05-26): `PRIMARY KEY (a, b, ...)` con alguna
    /// columna que no es INT o admite NULL. En VERSION 8 la PK compuesta
    /// está restringida a multi-INT NOT NULL: la implementación usa un
    /// fingerprint FNV-1a-64 que vive como i64 en el B+Tree y por eso
    /// no admite NULL ni tipos no-INT (ver ADR-0019).
    pub const COMPOSITE_PK_REQUIRES_ALL_INT: u32 = 4064;
    /// Bloque K2: `PRIMARY KEY` declarado dos veces en el mismo
    /// `CREATE TABLE` — inline en una columna + table-level
    /// `PRIMARY KEY (...)`, o dos columnas con PRIMARY KEY inline.
    pub const PRIMARY_KEY_DUPLICATED: u32 = 4065;
    /// Bloque K2 (reservado): una FOREIGN KEY apunta a una columna del
    /// padre que no es ni PK ni tiene índice UNIQUE. Pre-K2 sólo se
    /// admitía apuntar a la PK; con PK compuesta single-col-FK debe
    /// apuntar a una de las columnas de esa PK o a una UNIQUE. Hoy se
    /// reusa `FK_PARENT_MISSING (3004)` para los casos prácticos; el
    /// código se reserva para una futura validación específica.
    pub const FK_TARGET_NOT_INDEXED: u32 = 4066;
    /// Bloque K2: `CREATE INDEX idx ON t (a, b, ...)` cuya lista de
    /// columnas mezcla tipos no-INT. Mismo motivo que 4064 — el bucket
    /// usa fingerprint i64 y por eso exige INT en todas las columnas.
    pub const COMPOSITE_INDEX_REQUIRES_ALL_INT: u32 = 4067;
    /// Bloque K2 (reservado): `WHERE a = 1` contra una PK compuesta
    /// `(a, b)` cuando el usuario esperaba lookup parcial por la primera
    /// columna. El motor cae a full-scan en ese caso (no es un error);
    /// el código queda reservado para un release futuro que ofrezca
    /// `PARTIAL_KEY_LOOKUP_UNSUPPORTED` como warning explícito.
    pub const PARTIAL_KEY_LOOKUP_UNSUPPORTED: u32 = 4068;
    /// Bloque L2 (2026-05-27): `CHECK (expr)` contiene una subquery
    /// (`(SELECT …)` dentro del predicado). ANSI prohíbe esto y el
    /// evaluador `eval_expr` lo rechaza activamente. Reescribir como
    /// constraint relacional o validarlo desde el cliente.
    pub const CHECK_CONTAINS_SUBQUERY: u32 = 4069;
    /// Bloque L2: el predicado declarado en `CHECK (expr)` no evalúa a
    /// BOOL (ni NULL en 3VL). Caso típico: `CHECK (LENGTH(name))` sin
    /// comparación. Igual que `WHERE_EXPR_NOT_BOOLEAN` pero localizado
    /// para mensajes de error claros al usuario de CREATE TABLE.
    pub const CHECK_EXPR_NOT_BOOLEAN: u32 = 4070;
    /// Residual #2 de L (2026-05-27): `ALTER TABLE DROP CONSTRAINT <name>`
    /// no encontró ningún constraint con ese nombre (CHECK, UNIQUE
    /// nombrado, o FK nombrada). El mensaje incluye las opciones
    /// disponibles cuando hay constraints en la tabla.
    pub const CONSTRAINT_NOT_FOUND: u32 = 4071;
    /// Residual #2 de L: intento de borrar la PRIMARY KEY con
    /// `DROP CONSTRAINT <name>`. La PK es inmutable durante la vida
    /// de la tabla; rehacer el esquema con `DROP TABLE` + recreate si
    /// la intención es cambiarla.
    pub const CANNOT_DROP_PRIMARY_KEY_CONSTRAINT: u32 = 4072;
    /// Residual #4 de L (2026-05-27): `UPDATE` cambió un PK que tiene
    /// children con `ON UPDATE RESTRICT` (o `NO ACTION`, que en este
    /// release es alias). El motor aborta el UPDATE entero sin estado
    /// parcial. Solución: borrar/actualizar primero los children, o
    /// declarar la FK con `ON UPDATE CASCADE` / `SET NULL` /
    /// `SET DEFAULT`.
    pub const FK_RESTRICT_BLOCKS_UPDATE: u32 = 4073;
    /// Residual #4 de L: `UPDATE` con `ON UPDATE CASCADE` haría que la
    /// PK del child cambie (porque una columna source de la FK también
    /// participa en la PK del child). Este release no soporta cascadas
    /// de PK encadenadas. Solución: rediseñar la FK o reescribir el
    /// UPDATE como DELETE + INSERT con una transacción explícita.
    pub const FK_UPDATE_CASCADE_AFFECTS_CHILD_PK: u32 = 4074;
    /// Bloque V (2026-05-27): `INSERT`/`UPDATE`/`DELETE` apuntando a un
    /// nombre que existe en el catálogo pero es una **vista**. Las
    /// vistas son read-only en este release — no hay rewrites de
    /// modificación a la tabla base. El INSERT/UPDATE/DELETE debe
    /// dirigirse a la tabla base directamente.
    pub const VIEW_NOT_WRITABLE: u32 = 4075;
    /// Bloque V: la expansión de vistas excedió `MAX_VIEW_DEPTH`
    /// (vistas mutuamente referenciadas, ciclo directo, o anidamiento
    /// patológico). Solución: simplificar el grafo de vistas o
    /// materializar en una tabla.
    pub const VIEW_EXPANSION_DEPTH_EXCEEDED: u32 = 4076;
    /// Bloque V: `CREATE VIEW` cuyo nombre colisiona con una tabla o
    /// vista ya existente. Mismo namespace para los tres.
    pub const VIEW_NAME_COLLIDES_WITH_OBJECT: u32 = 4077;
    /// Bloque V: el SELECT subyacente de la vista resultó en una set
    /// operation (UNION/INTERSECT/EXCEPT) o VALUES. Este release sólo
    /// expande vistas cuyo source es un `SELECT` simple — set ops
    /// como source quedan diferidos.
    pub const VIEW_SOURCE_NOT_SIMPLE_SELECT: u32 = 4078;
    /// Bloque W1 (2026-05-28): dos `CTE` dentro del mismo `WITH`
    /// declararon el mismo nombre (case-insensitive). Cada nombre debe
    /// ser único dentro de la cláusula `WITH`; el lookup posterior es
    /// case-insensitive.
    pub const CTE_DUPLICATE_NAME: u32 = 4079;
    /// Bloque W1: reservado para `WITH RECURSIVE`. **Retirado en W2**
    /// (2026-05-28): la sintaxis recursiva está soportada — los errores
    /// específicos del fixpoint viven en 4082..=4086. El código se
    /// mantiene reservado para no reciclar el slot.
    pub const CTE_RECURSIVE_NOT_SUPPORTED: u32 = 4080;
    /// Bloque W1: `WITH name(c1, c2, ...) AS (...)` — column aliases en
    /// la cabecera de la CTE diferidos. Workaround: aliasar dentro del
    /// SELECT del body (`SELECT x AS c1, y AS c2 FROM ...`), que es
    /// equivalente en semántica.
    pub const CTE_COLUMN_ALIASES_NOT_SUPPORTED: u32 = 4081;
    /// Bloque W2 (2026-05-28): `WITH RECURSIVE` con más de una CTE
    /// declarada — soportamos exactamente una CTE recursive por `WITH`
    /// en este release. Workaround: anidar `WITH RECURSIVE`s en
    /// subqueries separadas o pre-materializar.
    pub const RECURSIVE_CTE_MULTIPLE_NOT_SUPPORTED: u32 = 4082;
    /// Bloque W2: la materialización del fixpoint excedió el límite
    /// duro de iteraciones (`MAX_RECURSIVE_ITERATIONS`). Recursión sin
    /// terminación natural — agregar una condición de corte al step
    /// (`WHERE n < N`) o usar `UNION` en vez de `UNION ALL` para que
    /// el dedup converja.
    pub const RECURSIVE_CTE_MAX_ITERATIONS_EXCEEDED: u32 = 4083;
    /// Bloque W2: la materialización del fixpoint excedió el límite
    /// duro de filas totales acumuladas (`MAX_RECURSIVE_ROWS`). Mismo
    /// diagnóstico que `MAX_ITERATIONS_EXCEEDED`.
    pub const RECURSIVE_CTE_MAX_ROWS_EXCEEDED: u32 = 4084;
    /// Bloque W2: el SELECT del step proyecta una arity / orden de
    /// columnas incompatible con el anchor. ANSI exige columnas
    /// posicionalmente compatibles para que UNION ALL pueda anidar.
    pub const RECURSIVE_CTE_SCHEMA_MISMATCH: u32 = 4085;
    /// Bloque W2: el body de una CTE marcada `RECURSIVE` debe ser una
    /// `UNION` o `UNION ALL` de dos SELECTs (anchor + step). Otra
    /// forma (un único SELECT, o INTERSECT/EXCEPT, o tres ramas) no
    /// se acepta — apenas la forma canónica. Workaround: si la CTE
    /// no necesita ser recursive, quitar la palabra `RECURSIVE`.
    pub const RECURSIVE_CTE_BODY_NOT_UNION: u32 = 4086;
    /// Bloque W3 (2026-05-28): nombre de función seguido de `OVER (...)`
    /// que no es una window function reconocida. Soportadas: ranking
    /// (`ROW_NUMBER`/`RANK`/`DENSE_RANK`), agregados (`COUNT`/`SUM`/
    /// `AVG`/`MIN`/`MAX`), value (`LAG`/`LEAD`/`FIRST_VALUE`/
    /// `LAST_VALUE`/`NTILE`).
    pub const WINDOW_FUNCTION_UNKNOWN: u32 = 4087;
    /// Bloque W3: la función window requiere `ORDER BY` dentro del
    /// `OVER (...)`. Aplica a `LAG`, `LEAD`, `NTILE`, y a las funciones
    /// de ranking sin orden definido (ROW_NUMBER permite no-ORDER
    /// pero el resultado es no-determinístico — los avisos van vía doc).
    pub const WINDOW_REQUIRES_ORDER_BY: u32 = 4088;
    /// Bloque W3: la función window recibió un número incorrecto de
    /// argumentos. Las ranking no toman args; `NTILE(n)` toma 1; agg
    /// toma 1 (excepto `COUNT(*)`); LAG/LEAD toman 1..=3
    /// (`expr [, offset [, default]]`).
    pub const WINDOW_ARG_MISMATCH: u32 = 4089;
    /// Bloque W3: SELECT que mezcla window functions con `GROUP BY` /
    /// `HAVING` / agregados clásicos no-window. Diferido — para usar
    /// ambos hay que envolver el GROUP BY en una derived table y
    /// aplicar el window sobre el resultado.
    pub const WINDOW_NOT_ALLOWED_WITH_GROUP_BY: u32 = 4090;
    /// Bloque W3: window function aparece en un contexto donde no se
    /// permite (WHERE, HAVING, ORDER BY del propio SELECT, body de
    /// una CTE recursive, CHECK constraint, etc.). Sólo el SELECT
    /// list del SELECT top-level acepta windows.
    pub const WINDOW_NOT_ALLOWED_HERE: u32 = 4091;
    /// Bloque X1 (2026-05-28): `CREATE TRIGGER name ...` cuyo nombre
    /// colisiona con otra trigger / tabla / vista en el catálogo. Los
    /// nombres viven en un namespace global (mismo trato que vistas).
    pub const TRIGGER_NAME_COLLIDES: u32 = 4092;
    /// Bloque X1: el body de `CREATE TRIGGER ... FOR EACH ROW <stmt>`
    /// debe ser una sentencia DML simple (INSERT / UPDATE / DELETE).
    /// SELECTs, transacciones, DDL u otros statements rechazados.
    pub const TRIGGER_BODY_INVALID: u32 = 4093;
    /// Bloque X1: referencia a `NEW.col` en un trigger `DELETE` (donde
    /// NEW no existe), a `OLD.col` en un `INSERT`, o a una columna
    /// inexistente en NEW/OLD.
    pub const TRIGGER_NEW_OLD_OUT_OF_SCOPE: u32 = 4094;
    /// Bloque X1: la cascada de triggers excedió `MAX_TRIGGER_DEPTH`
    /// (un trigger disparó otro DML que disparó otro trigger, etc.).
    /// Causa típica: trigger que modifica la misma tabla. Mismo
    /// fail-safe que VIEW_EXPANSION_DEPTH_EXCEEDED.
    pub const TRIGGER_RECURSION_DEPTH_EXCEEDED: u32 = 4095;
    /// Bloque X1: `DROP TRIGGER name` sobre un nombre que no existe.
    /// `DROP TRIGGER IF EXISTS` no rebota — devuelve OK silencioso.
    pub const TRIGGER_NOT_FOUND: u32 = 4096;
    /// Bloque X3 (2026-05-28): `CREATE PROCEDURE name ...` cuyo nombre
    /// colisiona con tabla / vista / trigger / procedure existente.
    /// Mismo namespace global que el resto del catálogo.
    pub const PROCEDURE_NAME_COLLIDES: u32 = 4097;
    /// Bloque X3: el body de `CREATE PROCEDURE ... AS <body>` debe ser
    /// una sentencia DML simple (INSERT/UPDATE/DELETE/REPLACE) o un
    /// bloque `BEGIN stmt; stmt; END` con varias DMLs. SELECT y otros
    /// rechazados.
    pub const PROCEDURE_BODY_INVALID: u32 = 4098;
    /// Bloque X3: `CALL name(args)` sobre un nombre que no existe.
    /// También usado por `DROP PROCEDURE name` sin `IF EXISTS`.
    pub const PROCEDURE_NOT_FOUND: u32 = 4099;
    /// Bloque X3: `CALL name(args)` recibió un número de argumentos
    /// distinto al declarado en la signatura de la procedure.
    pub const PROCEDURE_ARITY_MISMATCH: u32 = 4100;
    /// Bloque X3b (2026-05-28): `CREATE FUNCTION name ...` cuyo nombre
    /// colisiona con tabla / vista / trigger / procedure / function
    /// existente. Mismo namespace global.
    pub const FUNCTION_NAME_COLLIDES: u32 = 4101;
    /// Bloque X3b: el body de `CREATE FUNCTION ... AS <expr>` no es una
    /// expresión válida, o falta el `AS` / `RETURNS`. También cuando
    /// el body usa subqueries (diferido).
    pub const FUNCTION_BODY_INVALID: u32 = 4102;
    /// Bloque X3b: `name(args)` en una expresión sobre un nombre que no
    /// matchea ningún ScalarFunc built-in NI una function user-defined
    /// en el catálogo. También: `DROP FUNCTION` sin `IF EXISTS` sobre
    /// nombre inexistente.
    pub const FUNCTION_NOT_FOUND: u32 = 4103;
    /// Bloque X3b: `name(args)` recibió un número de argumentos distinto
    /// al declarado en la signatura de la function.
    pub const FUNCTION_ARITY_MISMATCH: u32 = 4104;
    /// Bloque X4 (2026-05-28): la condición de un `IF ... THEN` (en el
    /// body de un trigger o procedure) no evaluó a BOOL. NULL se trata
    /// como FALSE (3VL → la branch THEN no corre).
    pub const IF_CONDITION_NOT_BOOLEAN: u32 = 4105;
    /// Bloque X4: bloque `IF` malformado — falta `THEN`, falta `END IF`,
    /// `ELSIF`/`ELSE` fuera de lugar, etc.
    pub const IF_BLOCK_MALFORMED: u32 = 4106;

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
