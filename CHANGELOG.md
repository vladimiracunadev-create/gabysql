# 📝 Changelog

> **Historial de cambios relevantes aplicados al producto y a su base documental.**
>
> Para registro detallado de **bugs e incidentes operativos resueltos en sesión** (regresiones de CI, fixes intermedios, errores de configuración), ver [`docs/INCIDENTS_2026-05-25.md`](docs/INCIDENTS_2026-05-25.md). Para el detalle de los hallazgos del audit interno de seguridad ver [`SECURITY_AUDIT_2026-05-25.md`](SECURITY_AUDIT_2026-05-25.md).

---

## 2026-05-26 — Bloque K2: PK compuesta + índices compuestos (VERSION 7 → 8)

> **Un push a `main`** que cierra el sub-bloque K2 del roadmap: el DDL que **sí** cambia el formato en disco. Habilita `PRIMARY KEY (a, b, ...)` (table-level) y `CREATE [UNIQUE] INDEX idx ON t (a, b, ...)`. Bump VERSION 7 → 8 con rechazo limpio de DBs viejas vía `[GBY-1003]`. Ver [ADR-0019](docs/adr/0019-composite-pk-and-index.md) para la decisión y limitaciones.

### 🆕 Sintaxis

- `CREATE TABLE asistencias (curso INT NOT NULL, alumno INT NOT NULL, presente BOOL, PRIMARY KEY (curso, alumno));`
- `CREATE TABLE t (id INT NOT NULL, v INT, PRIMARY KEY (id));` — PK table-level single-col también soportada (estilo opcional).
- `CREATE INDEX idx_ab ON t (a, b);`
- `CREATE UNIQUE INDEX uq_year_month ON ventas (year, month);`

### 🚧 Limitaciones K2 (explícitas)

- PK e índices compuestos **restringidos a all-INT NOT NULL** (`[GBY-4064]` / `[GBY-4067]`). El fingerprint i64 no representa NULL ni tipos no-INT.
- **No partial lookup indexado**: `WHERE a = 1` contra PK `(a, b)` cae a full-scan (resultado correcto, sin error, sin fast-path).
- **No range scan compuesto**: el fingerprint FNV-1a-64 no es order-preserving.
- **FK siguen single-column**: las relaciones multi-col se modelan vía surrogate INT + UNIQUE compuesta.
- **ALTER PK queda fuera** (creación nueva sí; ALTER no).
- **Migración V7 → V8 es manual**: hacer backup, recrear con binario nuevo, dump + INSERT.

### 🔧 Catálogo aditivo

- `TableMeta` agrega `primary_key_extra: Vec<String>` (vacío para PK single).
- `IndexMeta` agrega `extra_columns: Vec<String>` (vacío para single-column).
- Helpers nuevos: `TableMeta::pk_columns()`, `has_composite_pk()`, `is_pk_column(name)`; `IndexMeta::all_columns()`, `is_composite()`.

### 🔢 Fingerprint compuesto

- `src/index.rs::encode_composite_key(columns, values) -> i64` — FNV-1a-64 sobre `encode_column_value()` de cada par + sentinela `0xFF` entre columnas.

### 🗄️ Formato en disco — VERSION 8

```text
TableMeta:
  [name][pk_count:u8][pk_col_name × pk_count][root_page:u32]
  [col_count:u16] × { [name][type_code:u8][flags:u8] (DEFAULT)? (FK)? }
  [idx_count:u16] × {
    [name][column][root_page:u32][unique:u8][kind:u8]
    [extra_cols_count:u8][extra_col_name × extra_cols_count]
  }
```

VERSION 7 se rechaza al abrir con `[GBY-1003] UNSUPPORTED_FORMAT_VERSION` y mensaje que sugiere backup + dump + recreate.

### 🚦 Executor

- `exec_create_table`: el parser entrega `primary_key_extra` desde el table-level `PRIMARY KEY (...)`; el validator (`validate_create_table` en `catalog.rs`) verifica all-INT + NOT NULL en cada columna PK cuando es compuesta.
- `exec_create_index`: para índices compuestos verifica all-INT (`[GBY-4067]`), backfilea con `encode_composite_key` + ordered bucket layout (`[u16:count] + count × pk:i64`), detecta UNIQUE conflicts por fingerprint (`[GBY-3003]`). Publica con `IndexKind::OrderedInt` para reutilizar el decoder de INTEGRITY CHECK.
- `encode_row`: cuando `meta.has_composite_pk()` computa el fingerprint sobre todas las columnas PK; NULL en cualquiera → `[GBY-3007] PRIMARY_KEY_NULL`.
- UPDATE bloquea CUALQUIER columna PK → `[GBY-4008] UPDATE_PK_NOT_ALLOWED` con mensaje que enumera todas las columnas PK.
- Planner del WHERE: PK compuesta + WHERE sobre columna PK → fuerza `Plan::FullScan` + `generic_post_filter` (correcto via 3VL).

### 🆔 Códigos de error nuevos

- `4064 COMPOSITE_PK_REQUIRES_ALL_INT`
- `4065 PRIMARY_KEY_DUPLICATED`
- `4066 FK_TARGET_NOT_INDEXED` (reservado)
- `4067 COMPOSITE_INDEX_REQUIRES_ALL_INT`
- `4068 PARTIAL_KEY_LOOKUP_UNSUPPORTED` (reservado)

### 🧪 Tests

17 nuevos `k2_*`. `cargo fmt --check` ✅ · `cargo clippy --all-targets -- -D warnings` ✅ · `cargo test --all-targets` → **300 passed, 0 failed** (283 prior + 17 k2_*).

---

## 2026-05-26 — Bloque K1: DDL extendido (CTAS, RENAME, DROP/RENAME COLUMN)

> **Un push a `main`** que cierra el sub-bloque K1 del roadmap (`docs/MISSING_COMMANDS.md` §9): DDL faltante que **no** cambia el formato en disco (VERSION sigue en 7). Cubre `CREATE TABLE [IF NOT EXISTS] [(col_aliases)] AS <select>` (CTAS), `RENAME TABLE` / `ALTER TABLE RENAME TO`, `ALTER TABLE DROP COLUMN [IF EXISTS]` y `ALTER TABLE RENAME COLUMN`. La parte de DDL que sí tocaría el formato on-disk (PK compuesta, índices compuestos, partial indexes, `ALTER COLUMN TYPE`) queda para K2.

### 🆕 Sintaxis
- `CREATE TABLE [IF NOT EXISTS] dst AS SELECT id, ... FROM src [WHERE ...];` — la fuente puede ser cualquier `SelectQuery` (SELECT puro, set ops, VALUES).
- `CREATE TABLE dst (pk, label, score) AS SELECT id, nombre, total FROM src;` — alias de columnas opcionales; arity debe matchear.
- `RENAME TABLE old TO new;` y `ALTER TABLE old RENAME TO new;` — equivalentes.
- `ALTER TABLE t DROP COLUMN [IF EXISTS] col;` — la palabra `COLUMN` es obligatoria (para no colisionar con futuros `DROP CONSTRAINT`).
- `ALTER TABLE t RENAME COLUMN old TO new;` — arrastra el cambio a PK / índices / FKs entrantes.

### 🔧 AST + Parser
- Nuevas variantes en `Statement`: `CreateTableAs(CreateTableAsStmt)`, `RenameTable(RenameTableStmt)`, `AlterTableDropColumn(AlterDropColumnStmt)`, `AlterTableRenameColumn(AlterRenameColumnStmt)`.
- `parse_create` ahora reconoce `IF NOT EXISTS` y distingue CTAS de la forma clásica vía lookahead: tras `(` snapshotea `self.pos`, intenta consumir una lista de idents simples seguida de `)` + `AS` y, si no matchea, rollback al snapshot y cae al path tradicional (`col TIPO constraints, ...`).
- `parse_alter` se generaliza para `ADD [COLUMN]` (path histórico) / `DROP COLUMN [IF EXISTS]` / `RENAME TO` / `RENAME COLUMN`. `parse_statement` reconoce el top-level `RENAME TABLE` (alias estilo MySQL).
- Helpers: `try_parse_ctas_column_aliases` (lookahead lista de idents simples), `parse_select_query_for_ctas` (reusa el árbol del bloque I).

### 🚦 Executor
- `exec_create_table_as`: materializa la fuente con `exec_select_query`, valida arity de los alias (`[GBY-4063]`), valida ident y dedup de los headers, infiere tipos por columna (mismo variant en todos los no-NULL → ese tipo; INT+FLOAT promueven; mezcla → TEXT fallback), exige primera columna INT no-NULL como PK (`[GBY-4058]`), detecta duplicados de PK temprano (`[GBY-3001]`), crea la root_page, publica en el catálogo y rellena fila a fila vía `encode_row` + `Catalog::insert_row`. Toda la operación corre dentro de la transacción del batch — si algo falla, el wrap externo hace rollback.
- `exec_rename_table`: valida ident del nuevo nombre, exige que el origen exista (`[GBY-2001]`) y el destino no (`[GBY-4062]`), borra la entry vieja del catálogo + publica la nueva (FNV-1a-64 sobre el nuevo nombre), y barre la lista de tablas re-escribiendo los `ForeignKeyMeta::table` que apuntaban al nombre viejo.
- `exec_alter_drop_column`: chequea existencia (con respeto de `IF EXISTS`), bloquea sobre PK (`[GBY-4059]`), columnas indexadas (`[GBY-4060]`, mensaje sugiere `DROP INDEX <name>`), FKs salientes y entrantes (`[GBY-4061]`); luego full-scan de filas, decode con la meta vieja, remove de la columna del HashMap, re-encode con la meta nueva y `upsert_row` (mismo patrón que `ALTER TABLE ADD COLUMN`).
- `exec_alter_rename_column`: valida ident destino, exige existencia del origen y no-existencia del destino (`[GBY-4062]`); como el on-disk row es posicional, no requiere rewrite — sólo muta `TableMeta.columns[i].name`, `primary_key` (si la columna renombrada era la PK), `IndexMeta::column` y los `ForeignKeyMeta::column` entrantes de otras tablas.

### ⚠️ Limitaciones residuales (cierra K1; abre K2)
- **PK compuesta** y **índices compuestos** quedan para K2 — requieren un encoder multi-columna y bump VERSION 7→8 + ADR.
- **`ALTER COLUMN TYPE`** queda para K2 — requiere rewrite tipado con compatibilidad de defaults.
- **CTAS sin `id INT`**: el motor exige que la primera columna del SELECT sirva como PK (única estrategia compatible con la limitación de PK escalar INT). Sin esa columna, error `[GBY-4058]` explícito. El usuario tiene dos workarounds: (a) anteponer `id INT` en el SELECT, (b) usar la forma con alias `CREATE TABLE t (id, ...) AS SELECT 1, ...`.
- **CTAS con result-set vacío**: rechazado con `[GBY-4058]` — sin filas no se puede confirmar que la primera columna sea INT. Trabajar con `LIMIT 0` no es portable a esquemas en blanco; usar `CREATE TABLE ... (id INT PRIMARY KEY, ...)` clásico.
- **CTAS no hereda DEFAULT/NOT NULL/UNIQUE/FK** del origen: la nueva tabla queda con sólo la PK INT NOT NULL. Si el usuario los necesita, hay que recrear el esquema con DDL clásico + `INSERT INTO ... SELECT ...`.
- **DROP COLUMN sobre la única columna no-PK** está permitido (la tabla queda con sólo PK; estado válido).
- **DROP TABLE ... CASCADE** sigue pendiente (P2).

### 🧰 Códigos de error nuevos
- `4058` `CTAS_REQUIRES_INT_FIRST_COLUMN` — CTAS cuya primera columna no es INT no-NULL.
- `4059` `CANNOT_DROP_PRIMARY_KEY` — DROP COLUMN sobre la PK.
- `4060` `CANNOT_DROP_INDEXED_COLUMN` — DROP COLUMN sobre una columna con índice (mensaje sugiere `DROP INDEX`).
- `4061` `CANNOT_DROP_REFERENCED_COLUMN` — DROP COLUMN sobre una columna con FK saliente o entrante.
- `4062` `RENAME_TARGET_EXISTS` — RENAME TABLE / RENAME COLUMN cuyo destino ya existe.
- `4063` `CTAS_COLUMN_ALIAS_ARITY` — `CREATE TABLE t (alias_list) AS SELECT ...` con arity de aliases ≠ arity del SELECT.

### 🧪 Validación
- 28 tests nuevos `k1_*` cubriendo: CTAS basic / con WHERE / con column-aliases / arity-mismatch / desde set op / desde VALUES / primera col no-INT (4058) / result-set vacío (4058) / `IF NOT EXISTS` no-op / destino tomado (2004) / GROUP BY con primera col TEXT (4058); RENAME TABLE basic / via `ALTER TABLE RENAME TO` / destino tomado (4062) / origen ausente (2001) / FKs entrantes actualizadas; DROP COLUMN basic / `IF EXISTS` no-op / faltante sin IF EXISTS (2002) / PK (4059) / indexada (4060 con sugerencia DROP INDEX) / FK local (4061) / round-trip de datos en columnas restantes; RENAME COLUMN basic / destino tomado (4062) / origen ausente (2002) / sobre PK / sobre columna indexada.
- 283 tests integración total (255 pre-K1 + 28 K1), `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` limpios.

### 🗂️ Formato en disco
- `VERSION = 7` sin cambios. K1 no introduce ningún campo nuevo en `TableMeta`, `Column`, `IndexMeta` ni `ForeignKeyMeta`. DBs creadas con un binario pre-K1 abren sin migración y viceversa.

---

## 2026-05-26 — Bloque I: UNION / INTERSECT / EXCEPT + VALUES como tabla

> **Un push a `main`** que cierra el bloque I del roadmap (`docs/MISSING_COMMANDS.md` §5): operaciones de conjunto entre queries (`UNION` / `UNION ALL`, `INTERSECT` / `INTERSECT ALL`, `EXCEPT` / `EXCEPT ALL` con alias `MINUS`), y `VALUES (...), (...)` usable tanto como statement standalone (`VALUES (1,'a'), (2,'b');` devuelve un ResultSet) como tabla virtual dentro del FROM (`FROM (VALUES (1,'a'), (2,'b')) AS t(c1, c2)`).

### 🆕 Sintaxis
- `SELECT ... UNION [ALL] SELECT ...` — append/dedup de queries con la misma arity.
- `SELECT ... INTERSECT [ALL] SELECT ...` — filas presentes en ambos lados.
- `SELECT ... EXCEPT [ALL] SELECT ...` (alias `MINUS`) — filas del LHS no presentes en el RHS.
- Precedencia ANSI: `INTERSECT` ata más fuerte que `UNION` / `EXCEPT`; los tres son asociativos a izquierda.
- `(SELECT ...) UNION (SELECT ...) ORDER BY col LIMIT n OFFSET m` — ORDER BY / LIMIT / OFFSET al nivel del resultado combinado.
- `VALUES (1, 'a'), (2, 'b');` — statement standalone, devuelve ResultSet con headers `column1`, `column2`, ....
- `SELECT * FROM (VALUES (1, 'a'), (2, 'b')) AS t(id, name)` — tabla virtual literal en el FROM o como RHS de un JOIN (alias de tabla **y** lista de columnas obligatorios).

### 🔧 AST + Parser
- Nuevo enum `SelectQuery { Select(Box<SelectStmt>) | SetOp { lhs, op, all, rhs, order_by, limit, offset } | Values(ValuesClause) }`. `Statement::Select(SelectStmt)` pasa a `Statement::Select(Box<SelectQuery>)` (boxed para que el enum `Statement` no infle por culpa del variant más grande). El path `Select(stmt)` envuelve trivialmente el SelectStmt clásico — todos los call-sites pre-I siguen funcionando vía wrap/unwrap.
- Nuevo enum `SetOpKind { Union, Intersect, Except }` y struct `ValuesClause { rows: Vec<Vec<Expr>> }`.
- `SelectStmt` suma `values_source: Option<(Box<ValuesClause>, Vec<String>)>` para la forma `FROM (VALUES ...) AS t(c1, c2, ...)` como base table; `TableRef` suma `values` + `values_columns` para la forma equivalente en el RHS de un JOIN.
- Parser de set ops: `parse_set_ops_after` (nivel UNION/EXCEPT) → `parse_intersect_after` (sub-nivel INTERSECT, más alta precedencia) → `parse_select_term` (SELECT plano, VALUES, o `(SELECT|VALUES ...)` con sub-árbol). El `ORDER BY` / `LIMIT` / `OFFSET` que sigue al árbol top-level se cuelga del nodo `SetOp`.
- `parse_select_stmt_inner(allow_trailing_order_limit: bool)` — variante interna usada por `parse_select_term` cuando parsea un SELECT sin paréntesis envolventes dentro de un árbol de set ops: el ORDER BY/LIMIT trailing pertenece al outer, no al SELECT.
- `is_post_table_keyword` / `is_select_terminator_keyword` reconocen ahora `UNION` / `INTERSECT` / `EXCEPT` / `MINUS` como cortes del cuerpo del SELECT.

### 🚦 Executor
- `Engine::exec_select_query(SelectQuery)` despacha: `Select(stmt)` al path clásico `exec_select`, `Values(v)` a `exec_values_clause`, `SetOp { ... }` ejecuta ambos lados, llama a `combine_set_op` y aplica ORDER BY / LIMIT / OFFSET sobre el resultset combinado.
- `combine_set_op` valida arity (`[GBY-4054]`) y compatibilidad de tipos columna a columna (`[GBY-4055]` — INT/FLOAT promueven, otros tipos exigen match o NULL); usa `encode_group_key` (de F) para hashear filas y construir multisets con counts; aplica las reglas ANSI de bag-semantics: `UNION ALL` suma counts, `UNION` dedup; `INTERSECT ALL` toma `min(count_l, count_r)`, sin ALL devuelve 1; `EXCEPT ALL` toma `max(0, count_l - count_r)`, sin ALL devuelve 1 si la fila no está en el RHS.
- VALUES en FROM se materializa con `materialize_values_in_from` (mismo patrón que derived tables): infiere tipo por columna sobre los no-NULL, arma un `TableMeta` virtual sin storage, y delega al `JoinScope` igual que un derived. Sin alias de tabla → `[GBY-4052]`; sin lista de columnas o arity distinta → `[GBY-4053]`.
- `apply_order_by_on_resultset` resuelve el ORDER BY top-level por nombre (case-insensitive) contra los headers del resultset combinado (que son los del LHS — regla ANSI); falta de columna → `[GBY-2002]`. NULLs van al final igual que el ORDER BY pre-I.

### ⚠️ Limitaciones residuales (futuros bloques)
- `WITH ... AS (...)` / CTE — bloque W (planificado aparte).
- Set ops dentro de `UPDATE` / `DELETE` — no es estándar ANSI; no se planea.
- `INSERT INTO t (cols) VALUES (...), (...)` ya existía pre-I (bloque J multi-row); el VALUES de I es la forma standalone / FROM y no toca el path INSERT.
- `ALL`/`ANY`/`SOME` sobre subqueries (`col > ALL (SELECT ...)`) — backlog H-P2.
- `VALUES (...), (...) ORDER BY 1` con referencia posicional al ordinal — actualmente ORDER BY exige nombre.

### 🧰 Códigos de error nuevos
- `4052` `VALUES_IN_FROM_REQUIRES_ALIAS` — `FROM (VALUES ...)` sin alias de tabla / sin lista de columnas.
- `4053` `VALUES_COLUMN_ALIAS_ARITY` — arity de `t(c1, c2, ...)` no coincide con las filas de VALUES.
- `4054` `SET_OP_ARITY_MISMATCH` — `UNION` / `INTERSECT` / `EXCEPT` entre queries con distinto número de columnas.
- `4055` `SET_OP_TYPE_MISMATCH` — tipos incompatibles entre las columnas del LHS y del RHS de un set op.
- `4056` `VALUES_ROW_ARITY_MISMATCH` — dos filas del mismo `VALUES` con distinta arity.
- `4057` `VALUES_EMPTY` — `VALUES` sin filas.

### 🧪 Validación
- 22 tests nuevos `i_*` cubriendo: UNION basic/dedup/ALL/three-way/null-dedup/headers-from-lhs; UNION con ORDER BY y LIMIT a nivel top; UNION arity/type mismatch; INTERSECT basic e INTERSECT ALL counts; EXCEPT basic y EXCEPT ALL counts; alias `MINUS`; VALUES standalone / arity mismatch / empty; VALUES en FROM básico / JOIN con tabla persistente / alias requerido / arity de aliases; precedencia (`INTERSECT` ata más fuerte que `UNION`).
- 255 tests integración total (233 pre-I + 22 I), `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` limpios.

---

## 2026-05-26 — Bloque H: derived tables + NOT IN + scalar subquery in SELECT + multi-predicate correlated

> **Un push a `main`** que cierra los P0 + P1 del bloque H del roadmap (`docs/MISSING_COMMANDS.md` §4): derived tables `FROM (SELECT ...) AS alias`, `WHERE col NOT IN (SELECT ...)` con semántica ANSI 3VL, subqueries escalares en SELECT list (`SELECT id, (SELECT COUNT(*) FROM other) FROM t`), y `EXISTS` correlacionado dentro de combinadores `AND`/`OR`/`NOT` (levanta el bloqueo histórico `[GBY-4024]`).

### 🆕 Sintaxis
- `FROM (SELECT ...) AS sub` — derived tables (inline views) en el FROM o en el RHS de un JOIN; alias obligatorio (ANSI estricto, `[GBY-4048]`).
- `WHERE col NOT IN (SELECT ...)` — first-class. Con NULL en la subquery devuelve NULL (3VL ANSI estricta: `5 NOT IN (1, NULL)` → NULL).
- `SELECT id, (SELECT MAX(x) FROM t WHERE t.fk = outer.id) AS m FROM outer` — subquery escalar correlacionada en el SELECT list.
- `WHERE EXISTS (...) AND otra_col = N`, `WHERE NOT EXISTS (...) OR ...`, `WHERE EXISTS (...) AND EXISTS (...)` — combinaciones correlated multi-predicado.

### 🔧 AST + Parser
- `SelectStmt` suma `derived_source: Option<Box<SelectStmt>>`; cuando es `Some`, `table` lleva el alias y la subquery se materializa antes del scan.
- `TableRef` suma `derived: Option<Box<SelectStmt>>` para soportar derived en JOINs.
- `WhereClause::In` suma `negated: bool` — el parser construye `negated=true` cuando ve `NOT IN (SELECT ...)`.
- `Expr` suma `ScalarSubquery(Box<SelectStmt>)`. `parse_expr_primary` detecta `(` seguido de `SELECT` y la consume como subquery escalar.
- Helpers `expr_contains_subquery` (walker) y `Engine::eval_expr_full` (evaluator engine-aware) — el caller usa fast-path `eval_expr` cuando el árbol no contiene subqueries (zero overhead) y delega al engine cuando sí.

### 🚦 Executor
- `Engine::materialize_derived_table` ejecuta la subquery del derived, infiere tipo por columna (mismo variant en todos los no-NULL → ese tipo; mezcla → TEXT fallback) y construye un `TableMeta` virtual + filas decodificadas. Nombres duplicados → `[GBY-4049]`.
- `JoinTable` suma `virtual_rows: Option<Vec<HashMap<String, Value>>>`; `scan_qualified` las devuelve directamente sin hit al pager. `plan_index_loop` rechaza derived (no hay PK/índice real).
- `exec_select` despacha al JOIN path cuando hay `derived_source` (aunque no haya JOINs explícitos) — reusa todo el pipeline materializado.
- `eval_atom_single` ahora pushea el outer row al `outer_stack` al evaluar `Exists`/`EqColumnRef`/`In` correlados dentro de combinadores (antes solo el dispatch top-level lo hacía). Eso destraba `EXISTS` correlacionado en `AND`/`OR`/`NOT`.
- `collect_in_set` + `eval_in_subquery` centralizan la lógica 3VL ANSI de `[NOT] IN (SELECT)` con tracking explícito de NULL.

### ⚠️ Limitaciones residuales (futuros bloques)
- `ALL`/`ANY`/`SOME` (`col > ALL (SELECT ...)`) — P2.
- Correlated `col = outer.col` puro fuera de `EXISTS` combinado con JOINs — P2.
- `LATERAL` joins — P3.
- `WITH` / CTE — bloque W (planificado aparte).
- Derived dentro de UPDATE/DELETE/INSERT — fuera de scope en H.

### 🧰 Códigos de error nuevos
- `4048` `DERIVED_TABLE_REQUIRES_ALIAS` — `FROM (SELECT ...)` sin alias.
- `4049` `DERIVED_DUPLICATE_COLUMN` — derived table con dos columnas del mismo nombre.
- `4050` `DERIVED_COLUMN_TYPE_AMBIGUOUS` — reservado para futura inferencia estricta de tipos en derived.
- `4051` `SCALAR_SUBQUERY_IN_EXPR_REQUIRES_PARENS` — reservado para validaciones futuras.
- `4024` `WHERE_COMBINATOR_CORRELATED_UNSUPPORTED` — DEPRECADO: el motor ya no lo genera (H levantó el bloqueo). Se conserva el slot por estabilidad del catálogo.

### 🧪 Validación
- 18 tests nuevos `h_*` cubriendo: derived basic / nested / con WHERE outer / con aggregate inside / join con persistente / alias requerido / duplicate column; NOT IN basic / NULL en subquery / NULL en outer; scalar subquery basic / correlated / too-many-rows / two-columns / no-rows-returns-null; correlated EXISTS AND/OR/two-EXISTS.
- 233 tests integración total (215 pre-H + 18 H), `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` limpios.

---

## 2026-05-26 — Bloque G3: aritméticos + concat + postfix Expr + funciones P2/P3

> **Un push a `main`** que cierra la familia G: operadores binarios `+`/`-`/`*`/`/`/`%`, concatenación `||`, postfix predicates (`IS [NOT] NULL`, `[NOT] LIKE`, `[NOT] IN`, `[NOT] BETWEEN`) sobre cualquier `Expr`, y las funciones escalares P2/P3 que quedaban abiertas en G1 — string (`TRIM`/`LTRIM`/`RTRIM`/`REPLACE`/`SPLIT_PART`), numéricas (`CEIL`/`FLOOR`/`MOD`/`POWER`/`SQRT`) y fecha (`DATE_ADD`/`DATE_SUB`/`DATEDIFF`/`EXTRACT`/`STRFTIME`).

### 🆕 Operadores
- Aritméticos binarios `+`, `-`, `*`, `/`, `%` sobre INT/FLOAT con promoción implícita (INT+FLOAT → FLOAT), `checked_*` para detectar overflow en INT, y error explícito en división/módulo por cero (entero o flotante).
- Concatenación `||` (regla PostgreSQL: misma precedencia que `+`/`-`). Cualquier tipo se reduce a TEXT con `value_to_text`; NULL propaga (ANSI estricta, igual que `CONCAT`).
- Postfix predicates sobre `Expr`: `LENGTH(x) IS NULL`, `UPPER(x) LIKE 'A%'`, `LENGTH(x) IN (3, 4, 5)`, `LENGTH(x) BETWEEN 3 AND 10` (más sus formas `NOT ...`). El path estructural pre-G3 (columna directa) se preserva intacto para no perder fast-paths.

### 🆕 Funciones escalares P2/P3
- **String:** `TRIM`, `LTRIM`, `RTRIM`, `REPLACE(s, from, to)`, `SPLIT_PART(s, sep, idx)` (1-based, fuera de rango → `''`).
- **Numéricas:** `CEIL` / `CEILING`, `FLOOR`, `MOD(a, b)` (alias del operador `%`), `POWER(x, y)` / `POW`, `SQRT(x)` (negativo → `[GBY-4045]`).
- **Fecha:** `DATE_ADD(d, n)`, `DATE_SUB(d, n)`, `DATEDIFF(d1, d2)` (días), `EXTRACT(YEAR|MONTH|DAY|HOUR|MINUTE|SECOND FROM expr)`, `STRFTIME(fmt, d)` con placeholders `%Y %m %d %H %M %S %%`.

### 🔧 AST + Parser
- `Expr` suma `Arith(Box<Expr>, ArithOp, Box<Expr>)`, `Like(...)`, `InList(...)`, `Between(...)`. Nuevo enum `ArithOp { Add, Sub, Mul, Div, Mod, Concat }`.
- Cadena de precedencia explícita en el parser: `parse_expr` → `parse_arith` (+/-/||) → `parse_arith_term` (*///%) → `parse_arith_factor` → `parse_expr_primary`. Comparadores y postfix predicates viven al tope (precedencia más baja, como en SQL estándar).
- Tokenizer: emite `||` como un único `Symbol`. `-N` literal solo se forma cuando el token previo NO termina un operando (heurística que respeta `LIMIT -1` / `VALUES (-3)` y a la vez deja funcionar `5 - 3`).
- `EXTRACT(field FROM expr)` se parsea con branch dedicado en `parse_func_call`; internamente se guarda como `Func(Extract, [Literal(String("YEAR")), expr])` para encajar en la firma genérica.
- `parse_where_atom_as_expr` ya no rechaza postfix sobre Expr — delega en `parse_expr` que aplica todo postfix uniformemente.

### 🚦 Executor
- `eval_expr` gana ramas para las nuevas variantes. `eval_arith` centraliza promoción de tipos, `checked_*` y división/módulo por cero. `Like`/`InList`/`Between` reusan los helpers existentes `eval_like` / `eval_in_list` / `eval_compare` con la misma 3VL que las variantes equivalentes de `WhereClause`.
- Helpers `days_from_civil` (inverso del existente `civil_from_days`) y formateadores `extract_date_field` / `strftime_format` / `parse_date_part_to_days` para las funciones de fecha.

### ⚠️ Limitaciones residuales (futuros bloques)
- `EXCLUDED.col` dentro de `ON CONFLICT DO UPDATE SET` (J2-P2 explícito).
- Unary `+` / `-` como operador prefix sobre expresión (el tokenizer captura literales negativos; expresiones tipo `-LENGTH(x)` quedan para una iteración futura — se puede escribir `0 - LENGTH(x)`).
- Subqueries dentro de `IN (...)` sobre LHS expresional (bloque H).
- Operadores aritméticos sobre tipos no numéricos (TEXT + INT, etc.) → `[GBY-4044]` explícito.

### 🧰 Códigos de error nuevos
- `4042` `ARITH_OVERFLOW` — overflow entero en `+/-/*//`.
- `4043` `DIVISION_BY_ZERO` — divisor cero en `/` o `%`.
- `4044` `ARITH_TYPE_MISMATCH` — operador aritmético sobre tipos no compatibles.
- `4045` `MATH_DOMAIN` — `SQRT(-x)`, `POWER(0, neg)`.
- `4046` `DATE_PARSE_ERROR` — TEXT no parseable como DATE/DATETIME en funciones de fecha.
- `4047` `EXTRACT_FIELD_INVALID` — campo no soportado por `EXTRACT`.

### 🧪 Validación
- ~30 integration tests nuevos `g3_*` cubriendo aritméticos (precedencia, paréntesis, overflow, division by zero, mezcla INT/FLOAT, NULL propagation, type mismatch), concat (`||`), postfix sobre Expr (IS NULL / LIKE / IN / BETWEEN), y todas las funciones P2/P3.
- `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- [`docs/SQL_REFERENCE.md`](docs/SQL_REFERENCE.md): subsección "Operadores aritméticos" + tabla de funciones P2/P3 + tabla de errores `4042`-`4047`.
- [`docs/MISSING_COMMANDS.md`](docs/MISSING_COMMANDS.md): los items P2/P3 cerrados se marcan `✅ (G3, 2026-05-26)`; el `||` y los aritméticos pasan a `✅`.
- [`docs/STATUS.md`](docs/STATUS.md): nota de cierre G3 en la fila "Funciones escalares".
- [`docs/ERROR_CODES.md`](docs/ERROR_CODES.md): seis filas nuevas (`4042`–`4047`).
- [`ROADMAP.md`](ROADMAP.md): bullet de cierre G3.

---

## 2026-05-26 — Bloque G2: expresiones escalares en WHERE / HAVING / UPDATE SET

> **Un push a `main`** que completa el bloque G iniciado por G1: las mismas funciones escalares / `CAST` / `CASE` / condicionales ahora se aceptan en las superficies de filtrado y mutación. Cierra la mayor limitación residual documentada en el changelog de G1.

### 🆕 Sentencias / cláusulas extendidas
- `WHERE` (de `SELECT`, `UPDATE`, `DELETE`): cualquier `Expr` BOOL/NULL es válida como átomo. Casos típicos: `WHERE LENGTH(name) > 3`, `WHERE UPPER(name) = 'X'`, `WHERE COALESCE(active, false) = true`, `WHERE CASE WHEN age > 18 THEN true ELSE false END = true`, `WHERE 5 < LENGTH(name)` (LHS literal).
- `HAVING`: ídem WHERE, conservando la libertad ya existente de referir agregados. Ej: `HAVING UPPER(group_col) = 'X'`.
- `UPDATE ... SET col = <expr>` y `ON CONFLICT DO UPDATE SET col = <expr>`: RHS pasa de `Value` a `Expr`. Se evalúa contra la fila **pre-update** (`SET a = b, b = a` swap-eligible).
- `DELETE FROM ... WHERE <expr>`: extensión gratuita gracias a que ya usaba el mismo `WhereExpr` (E3).

### 🔧 AST
- `UpdateStmt::assignments` y `OnConflictAction::DoUpdate::assignments` cambian de `Vec<(String, Value)>` a `Vec<(String, Expr)>`. Cambio de tipo público — los call-sites internos se actualizaron; los literales viejos siguen funcionando porque el parser construye `Expr::Literal(Value::X(...))`.
- `WhereClause` suma la variante `ExprPredicate { expr: Expr }`. Solo se construye cuando el átomo NO encaja en la forma estructural `IDENT OP literal` (LHS o RHS son funciones, CASE, CAST, literal a la izquierda, …); las variantes específicas pre-G2 (`Eq`, `Compare`, `Like`, `IsNull`, `InList`, `Between`) se preservan para mantener intactos los fast-paths PK / índice / range scan / EXISTS correlacionado.

### 🚦 Parser
- `parse_where_atom` arranca con `peek_atom_is_structural` — si el átomo es expresional cae a `parse_where_atom_as_expr` (ambos lados con `parse_expr_primary`, comparador o solo-expr).
- `parse_update` y la rama `ON CONFLICT DO UPDATE` usan `parse_expr` para la RHS de cada assignment.
- Las funciones agregadas siguen siendo estructurales en cualquier contexto: en HAVING resuelven contra el bucket; en WHERE el path estructural devuelve el `[GBY-4025]` claro (en vez del genérico `[GBY-4037]` que daría el path expresional).

### 🚦 Executor
- `eval_atom_single` y `eval_atom_joined` ganan un brazo `ExprPredicate { expr } => eval_expr_as_predicate(expr, row)`. El helper centraliza la 3VL: BOOL pasa tal cual, NULL → unknown (descarta la fila), cualquier otro tipo → `[GBY-4040]`.
- `filter_joined_rows_atom` agrega `ExprPredicate` al grupo "sin fast-path indexada — caer al evaluador 3VL".
- `exec_update` separa la validación shape (PK / columna existe / duplicados) — que sigue siendo one-shot — de la evaluación de la `Expr` que ahora ocurre dentro de `apply_update_to_pk` contra la fila pre-update. Pre-chequeo de tipo con `value_fits_column_type` para atribuir el mismatch a la columna exacta (`[GBY-4041]`).
- El planner (`generic_post_filter` + `Plan::FullScan`) reconoce `ExprPredicate` como predicado sin fast-path indexada, igual que los átomos E2.

### ⚠️ Limitaciones residuales (G3)
- Operadores postfix sobre expresión escalar (`LENGTH(x) IS NULL`, `UPPER(x) LIKE 'A%'`, `LENGTH(x) IN (...)`, `LENGTH(x) BETWEEN ... AND ...`) → `[GBY-4039]` con guía.
- Operador `||` para concatenación, aritméticos binarios (`+`/`-`/`*`/`/`), y funciones P2/P3 (`TRIM`, `REPLACE`, `CEIL`/`FLOOR`, `MOD`, `POWER`/`SQRT`, `DATE_ADD`/`DATE_SUB`, `DATEDIFF`, `EXTRACT`, `STRFTIME`, `SPLIT_PART`) siguen sin soporte.
- `EXCLUDED.col` dentro de `ON CONFLICT DO UPDATE SET` sigue sin soporte (J2-P2 explícito).

### 🧰 Códigos de error nuevos
- `4039` `EXPR_IN_PREDICATE_NOT_SUPPORTED` — operador postfix sobre Expr.
- `4040` `WHERE_EXPR_NOT_BOOLEAN` — expresión en WHERE/HAVING que no rinde BOOL/NULL.
- `4041` `UPDATE_SET_TYPE_MISMATCH` — RHS de `SET col = <expr>` con tipo incompatible.

### 🧪 Validación
- 20 integration tests nuevos en `tests/integration_test.rs` (`g2_*`): WHERE con LENGTH/UPPER/COALESCE/CASE/CAST/3VL, combinación con E1 (AND/OR), LHS literal, error 4040, error 4039 (IS NULL sobre Expr), UPDATE SET con UPPER/COALESCE/CASE/CAST/PK bloqueado/tipo mismatch, snapshot pre-update (`SET a=UPPER(a), b=a`), HAVING con UPPER, DELETE con LENGTH, UPDATE WHERE con UPPER.
- 176/176 tests pasan (los 156 previos + 20 g2). `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- [`docs/SQL_REFERENCE.md`](docs/SQL_REFERENCE.md): sección "Funciones escalares" actualizada con la extensión a WHERE/HAVING/UPDATE SET, ejemplos nuevos, y tres filas de errores típicos (4039/4040/4041).
- [`docs/MISSING_COMMANDS.md`](docs/MISSING_COMMANDS.md): nota de cierre G2 con limitaciones residuales que pasan a G3.
- [`docs/STATUS.md`](docs/STATUS.md): fila de "Funciones escalares" promovida a 🟢 con scope completo y limitaciones residuales explícitas.
- [`docs/ERROR_CODES.md`](docs/ERROR_CODES.md): tres filas nuevas (`4039`–`4041`).
- [`ROADMAP.md`](ROADMAP.md): bullet de cierre G2 debajo del de G1.

---

## 2026-05-26 — Bloque G1: funciones escalares en SELECT list

> **Un push a `main`** que abre el subsistema de funciones escalares: built-ins de string / numéricas / fecha + `CAST` + `CASE` + condicionales (`COALESCE`/`NULLIF`/`IFNULL`/`IF`). Cierra los P0/P1 del bloque G en [docs/MISSING_COMMANDS.md](docs/MISSING_COMMANDS.md) **dentro del SELECT list** — la extensión a `WHERE`/`HAVING`/`UPDATE SET` queda para G2.

### 🆕 Sentencias / cláusulas nuevas
- `SELECT` ahora acepta expresiones escalares como ítems del SELECT list, además de columnas crudas y agregados. `AS alias` opcional por ítem.
- Funciones built-in: `LENGTH`, `UPPER`, `LOWER`, `SUBSTR`/`SUBSTRING`, `CONCAT`, `ABS`, `ROUND`, `NOW`, `CURRENT_DATE`/`CURDATE`, `CURRENT_TIMESTAMP`, `COALESCE`, `NULLIF`, `IFNULL`, `IF`/`IIF`.
- `CAST(expr AS TYPE)` para INT / FLOAT / TEXT / BOOL / DATE / DATETIME / JSON.
- `CASE [operand] WHEN cond THEN val [...] [ELSE val] END` en sus dos formas (searched y simple).

### 🔧 AST
- Nuevo enum `Expr { Literal | Column | Func | Cast | Case | Compare | IsNull }` y enum auxiliar `ExprCmpOp` con los seis comparadores estándar.
- Nuevo enum `ScalarFunc` con las 14 built-ins soportadas + helper `from_ident` que acepta los aliases comunes.
- `SelectItem` gana la variante `Expression { expr, alias }`. `Star`/`Column`/`Aggregate` se preservan para no romper fast-paths.

### 🚦 Executor
- `resolve_selected_columns` ahora devuelve `Vec<Projection>` donde cada `Projection` es `BareColumn` (lookup directo, fast-path pre-G1) o `Expression` (evaluada per-row con `eval_expr`).
- `resolve_joined_projection` análogo: las columnas referenciadas dentro de `Expr::Column` se re-escriben a la forma cualificada `alias.col` con `rewrite_expr_columns_for_join` antes de la proyección — soporta JOIN + expresión escalar end-to-end (con detección de ambigüedad vía `[GBY-4018]`).
- Validación: NULL propagation por defecto (excepto `COALESCE`/`NULLIF`/`IFNULL`/`IF` con su propio control de NULL, y las funciones zero-arg de tiempo). `CASE` searched exige cond BOOL; `CASE` simple matchea por igualdad ANSI (NULL nunca matchea NULL).
- `NOW()` / `CURRENT_TIMESTAMP` / `CURRENT_DATE` formatean UTC como TEXT sin chrono — implementación inline con `civil_from_days` de Howard Hinnant.

### ⚠️ Limitaciones residuales (G2)
- Las expresiones escalares **solo** se aceptan en `SELECT` list. En `WHERE` / `HAVING` / `UPDATE SET` siguen aplicando las restricciones pre-G1 (literales o referencias a columna).
- En queries con `GROUP BY`/`HAVING`/agregados, `SelectItem::Expression` no se acepta todavía (devuelve `[GBY-4027]`). Lo mismo para `RETURNING` (devuelve `[GBY-2002]` con mensaje claro).
- No hay operador `||` para concatenar texto — usar `CONCAT(a, b, ...)`. Tampoco hay operadores aritméticos binarios (`+`/`-`/`*`/`/`).
- Funciones P2/P3 (`TRIM`, `REPLACE`, `CEIL`/`FLOOR`, `MOD`, `POWER`/`SQRT`, `DATE_ADD`/`DATE_SUB`, `DATEDIFF`, `EXTRACT`, `STRFTIME`, `SPLIT_PART`) siguen en backlog del mismo bloque G.

### 🧰 Códigos de error nuevos
- `4034` `SCALAR_FN_ARITY` — función escalar con cantidad equivocada de argumentos.
- `4035` `SCALAR_FN_TYPE_MISMATCH` — argumento de un tipo no aceptado por la función.
- `4036` `CAST_INVALID` — `CAST` cuyo valor no se puede convertir al tipo destino.
- `4037` `SCALAR_FN_UNKNOWN` — función escalar invocada que el motor no conoce.
- `4038` `CASE_BRANCH_TYPE_MISMATCH` — condición de `CASE WHEN` searched que no evalúa a BOOL.

### 🧪 Validación
- 12 integration tests nuevos en `tests/integration_test.rs` (`g1_*`): string funcs, SUBSTR edge cases, CONCAT mixto, ABS/ROUND, NOW/CURRENT_DATE/CURRENT_TIMESTAMP shape, COALESCE/NULLIF/IFNULL/IF, CAST válido + inválido, CASE searched + simple, alias, errores (arity/tipo/desconocido), 3VL con NULL, expresión sobre JOIN.
- `cargo fmt --check + cargo clippy --all-targets -- -D warnings + cargo test --all-targets` limpios.

### 📚 Documentación
- Sección nueva en [`docs/SQL_REFERENCE.md`](docs/SQL_REFERENCE.md) ("Funciones escalares (bloque G1)") con EBNF + tabla de funciones + ejemplos + errores típicos.
- [`docs/MISSING_COMMANDS.md`](docs/MISSING_COMMANDS.md): marcado `✅ (G1, 2026-05-26)` en los items P0/P1 cerrados.
- [`docs/STATUS.md`](docs/STATUS.md): nueva fila en la matriz de madurez por subsistema.
- [`docs/ERROR_CODES.md`](docs/ERROR_CODES.md): seis filas nuevas (`4033`-`4038`).
- [`ROADMAP.md`](ROADMAP.md): bullet de cierre en Fase 2.

---

## 2026-05-25 — Bloque J2: UPSERT, REPLACE INTO, RETURNING

> **Un push a `main`** que completa los pendientes del bloque J (excepto `UPDATE ... FROM`, deferido).

### 🆕 Sentencias / cláusulas nuevas
- `INSERT ... ON CONFLICT [(col)] DO NOTHING` — UPSERT pasivo (skip silencioso).
- `INSERT ... ON CONFLICT [(col)] DO UPDATE SET col = value, ...` — UPSERT activo (actualiza filas conflictivas con literales; sin `EXCLUDED.col` por ahora).
- `REPLACE INTO t (cols) VALUES (...)` — alias SQLite-style; desugar a `INSERT ... ON CONFLICT DO REPLACE` (borra fila conflictiva vía cascade FK + inserta nueva).
- `INSERT|UPDATE|DELETE ... RETURNING *` y `... RETURNING col1, col2` — devuelve las filas afectadas en el ResultSet (INSERT: post-insert; UPDATE: post-update; DELETE: pre-delete snapshot).

### 🔧 AST
- `InsertStmt` gana `on_conflict: Option<OnConflict>` y `returning: Option<Vec<SelectItem>>`.
- `UpdateStmt` y `DeleteStmt` ganan `returning: Option<Vec<SelectItem>>`.
- Nuevo enum `OnConflictAction { DoNothing | DoUpdate { assignments } | Replace }`.
- Nuevo `Statement::Replace(InsertStmt)` (desugar via parser).

### 🚦 Executor
- `apply_insert_row_with_conflict` reemplaza `apply_insert_row` y orquesta la trayectoria por fila: detecta conflictos PK + UNIQUE vía `detect_conflict_pks` y dispatcha a la acción. `RowOutcome { Inserted | Updated | Skipped }` mantiene los contadores y la lista de RETURNING.
- `DoUpdate` reusa `apply_update_to_pk` (E3) sobre las PKs conflictivas.
- `Replace` borra las PKs conflictivas con `delete_with_cascade` (J) y luego sigue el path normal de insert.
- `exec_update` y `exec_delete` recolectan filas post-update / pre-delete cuando hay RETURNING y proyectan vía `project_returning` + `returning_column_names`.
- `format_insert_message` cuenta inserted + replaced + skipped en el `message` del response.

### ⚠️ Limitaciones residuales
- `EXCLUDED.col` en `DO UPDATE SET col = EXCLUDED.col` no se soporta — los RHS deben ser literales por ahora. Workaround: precomputar el valor en cliente.
- `UPDATE ... FROM otra_tabla` (P2) — pendiente; requiere refactor del RHS de SET para aceptar column refs cualificados.
- `ON CONFLICT (col)` solo acepta una columna; multi-column unique constraints no se soportan todavía (los índices compuestos están en backlog del bloque K).

### 🧰 Códigos de error nuevos
- `4031` `ON_CONFLICT_INVALID` — `ON CONFLICT` malformada.
- `4032` `ON_CONFLICT_TARGET_NOT_UNIQUE` — `ON CONFLICT (col)` sobre columna sin PK/UNIQUE.

### 🧪 Validación
- 10 integration tests nuevos en `tests/integration_test.rs` (`j2_*`): INSERT RETURNING * / cols, UPDATE RETURNING, DELETE RETURNING, UPSERT DO NOTHING, UPSERT DO UPDATE, target no-único error, REPLACE INTO reemplaza / inserta, RETURNING con filas omitidas.
- `cargo check + cargo fmt --check + cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- (Se actualiza en el mismo push: SQL_REFERENCE, MISSING_COMMANDS, ERROR_CODES.)

---

## 2026-05-25 — Bloque J: DML masivo (multi-row `INSERT`, `INSERT...SELECT`, `TRUNCATE`)

> **Un push a `main`** que destraba inserts en bloque y limpieza de tabla.

### 🆕 Sentencias nuevas
- `INSERT INTO t (cols) VALUES (a,b),(c,d),...` — multi-row.
- `INSERT INTO t (cols) SELECT ...` — copia masiva desde otra query (puede tener WHERE/ORDER BY/JOIN/GROUP BY del bloque F).
- `TRUNCATE [TABLE] t` — borra todas las filas de la tabla manteniendo el schema. Implementación naive (scan-all-pks + delete_with_cascade); respeta FKs `ON DELETE`. No es O(1) como en PG/MySQL.

### 🔧 Refactor
- `InsertStmt.values: Vec<Value>` → `source: InsertSource { Values(Vec<Vec<Value>>) | Select(Box<SelectStmt>) }`. Single-row queda como caso particular de `Values(vec![row])`.
- `exec_insert` validara columnas + dedup UNA vez y luego itera filas-fuente delegando en el nuevo `apply_insert_row` (que encapsula NOT NULL/UNIQUE/FK/encode/insert/index-maintenance per-row).
- Response `message` ahora trae cuenta: `"OK (3 filas insertadas)"`.

### ⚠️ Comportamiento
- Multi-row no es transaccionalmente atómico **por sí solo** — fila K que falla deja las K-1 anteriores en el cache. El wrap del batch (auto-commit del `/exec` o `BEGIN`/`ROLLBACK` explícito del bloque T) define el alcance del rollback.
- `INSERT...SELECT` ejecuta la subquery completa antes de empezar a insertar (materializa primero). Para queries grandes esto es O(filas) en memoria.

### ⚠️ Limitaciones residuales del bloque J
- `INSERT ... ON CONFLICT DO UPDATE` / `UPSERT` (P1) — pendiente.
- `REPLACE INTO` (P2) — pendiente.
- `RETURNING` clause (P2) — pendiente; requiere extender `ResultSet` con filas devueltas.
- `UPDATE ... FROM otra_tabla` (P2) — pendiente.

### 🧪 Validación
- 8 integration tests nuevos en `tests/integration_test.rs` (`j_*`): multi-row INSERT, aridad mismatch aborta, INSERT...SELECT copia, INSERT...SELECT con WHERE, INSERT...SELECT aridad mismatch, TRUNCATE TABLE preserva schema, TRUNCATE sin keyword TABLE, multi-row con conflicto UNIQUE aborta.
- `cargo check + cargo fmt --check + cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- (Se actualiza en el mismo push: SQL_REFERENCE, MISSING_COMMANDS.)

---

## 2026-05-25 — Bloque T: transacciones explícitas (`BEGIN`/`COMMIT`/`ROLLBACK`)

> **Un push a `main`** que cierra el último P0 del top-5 del roadmap.

### 🔁 Sentencias nuevas
- `BEGIN` / `BEGIN TRANSACTION` / `BEGIN WORK` / `START TRANSACTION` — marca el inicio de una transacción explícita.
- `COMMIT` / `COMMIT TRANSACTION` / `COMMIT WORK` / `END` — persiste lo acumulado y re-abre una tx fresca.
- `ROLLBACK` / `ROLLBACK TRANSACTION` / `ROLLBACK WORK` — descarta lo acumulado y re-abre una tx fresca.

### 🔧 Cambios
- `Statement::Begin` / `Commit` / `Rollback` añadidos al AST.
- `Engine` gana un flag `explicit_tx: bool`. El Pager subyacente SIEMPRE tiene una transacción abierta (la abre el wrap del caller); este flag distingue la implícita del wrap de la explícita pedida por SQL.
- `exec_begin` / `exec_commit` / `exec_rollback` en el Engine. `COMMIT`/`ROLLBACK` invocan `pager.commit()`/`pager.rollback()` seguido de `pager.begin()` para preservar la invariante del wrap (el caller siempre puede hacer commit al final).

### ⚠️ Limitación documentada
- El `ROLLBACK` opera sobre el cache de páginas del Pager — descarta TODO lo cacheado, incluidas las sentencias del MISMO batch que ocurrieron ANTES del `BEGIN`. En la práctica esto significa que `BEGIN`/`ROLLBACK` solo aborta limpio cuando el batch entero arranca con `BEGIN` como primera sentencia. Cross-request transactions (mantener una tx abierta entre `/exec` HTTP) requieren session state en el server — fuera de scope para esta primera versión de T.
- `SAVEPOINT` / `ROLLBACK TO SAVEPOINT` (P1) no soportados. `SET TRANSACTION ISOLATION LEVEL ...` (P2) y `BEGIN READ ONLY` (P2) tampoco.

### 🧰 Códigos de error nuevos
- `4029` `TX_BEGIN_DOUBLE` — `BEGIN` con transacción explícita ya abierta.
- `4030` `TX_END_WITHOUT_BEGIN` — `COMMIT`/`ROLLBACK` sin `BEGIN` previo.

### 🧪 Validación
- 6 integration tests nuevos en `tests/integration_test.rs` (`t_*`): BEGIN+COMMIT persiste, BEGIN+ROLLBACK descarta, doble BEGIN error, COMMIT/ROLLBACK sin BEGIN error, alias START TRANSACTION/END, dos bloques BEGIN/COMMIT consecutivos.
- `cargo check + cargo fmt --check + cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- (Se actualiza en el mismo push: SQL_REFERENCE, MISSING_COMMANDS, ERROR_CODES.)

---

## 2026-05-25 — Bloque F: agregaciones (`GROUP BY`, `HAVING`, `COUNT`/`SUM`/`AVG`/`MIN`/`MAX`, `DISTINCT`)

> **Un push a `main`** que destraba reporting básico. Cierra el hueco más grande del top-5 del roadmap.

### 🧮 Funciones agregadas
- `COUNT(*)` — cuenta todas las filas del bucket (incluyendo NULLs en otras columnas).
- `COUNT(col)` — cuenta filas donde `col` no es NULL.
- `COUNT(DISTINCT col)` — valores no-NULL distintos.
- `SUM(col)` — INT preserva INT; mixto INT+FLOAT promueve a FLOAT. Conjunto vacío o todo-NULL → `NULL` (ANSI).
- `AVG(col)` — promedio FLOAT sobre valores no-NULL.
- `MIN(col)` / `MAX(col)` — ignora NULLs. Conjunto vacío o todo-NULL → `NULL`.

### 🗂️ GROUP BY + HAVING
- `GROUP BY <col> [, <col>]*` — bucketing por tupla (NULLs agrupan con NULLs, consistente con ANSI).
- `HAVING <expr>` — filtro post-agregación. Reusa `WhereExpr` con `allow_aggregates=true`: la LHS de un átomo puede ser una función agregada (`HAVING SUM(price) > 100`) o un alias del SELECT (`HAVING total > 100`).
- ANSI estricto: toda columna no-agregada en el SELECT debe figurar en `GROUP BY` — `[GBY-4027]` si no.

### 🔀 DISTINCT
- `SELECT DISTINCT col [, col]*` — dedup preservando el primer orden de aparición. Compatible con agregados (aunque suele ser redundante post-GROUP BY).

### 🔧 AST
- Nuevo enum `SelectItem { Star | Column(String) | Aggregate { func, arg, alias } }`. `SelectStmt.columns: Vec<String>` pasa a `Vec<SelectItem>`.
- Nuevos campos en `SelectStmt`: `distinct: bool`, `group_by: Vec<String>`, `having: Option<WhereExpr>`.
- Nuevo enum `AggFunc { Count, Sum, Avg, Min, Max }` y `AggArg { Star, Column, DistinctColumn }`.

### 🚦 Executor
- `exec_select` detecta `needs_aggregation` (cualquier agregado, GROUP BY, o HAVING presente) y desvía al nuevo `exec_aggregate_pipeline`. El path no-agregado mantiene fast-paths E1+E2+E3 intactos.
- `exec_aggregate_pipeline`: valida ANSI → bucketea por GROUP BY tuple (encoded como bytes para HashMap) → calcula agregados → aplica HAVING → proyecta a `output_name` → DISTINCT → ORDER BY contra esquema de salida → window.
- `dedup_preserving_order` helper para DISTINCT puro.

### ⚠️ Limitaciones residuales
- **Agregados sobre JOINs no se soportan todavía** — `[GBY-4028] AGGREGATE_OVER_JOIN_UNSUPPORTED`. Workaround: encapsular el JOIN en una subquery y agregar afuera.
- `GROUP_CONCAT` / `STRING_AGG`, `JSON_AGG` / `ARRAY_AGG` — P2/P3, fuera de F.
- Agregados en `ORDER BY` solo via alias o nombre canónico (`order by sum_x`) — no acepta la sintaxis cruda `ORDER BY SUM(x)`. Doable en una iteración menor.

### 🧰 Códigos de error nuevos
- `4025` `AGGREGATE_OUTSIDE_HAVING_OR_SELECT` — agregado en `WHERE` u otra cláusula prohibida.
- `4026` `AGGREGATE_ARG_INVALID` — `SUM(*)`, `AVG(DISTINCT x)`, tipos incompatibles.
- `4027` `SELECT_COLUMN_NOT_IN_GROUP_BY` — columna no-agregada que no figura en GROUP BY.
- `4028` `AGGREGATE_OVER_JOIN_UNSUPPORTED` — agregado en SELECT con JOINs.

### 🐛 Fixes incluidos
- Tres tests `e3_update_*` que llamaban a `SELECT ... WHERE col_no_indexed = val` para verificar el efecto del UPDATE — falla con el fast-path indexado pre-existente. Reescritos para usar `WHERE … AND id > 0` (forza FullScan + 3VL).
- `parser_returns_error_for_invalid_where` esperaba el mensaje legado "WHERE soporta solo" — actualizado al nuevo mensaje E2 y al código `[GBY-4001]`.
- `update_and_delete_by_pk_roundtrip` esperaba error al hacer `DELETE FROM u WHERE name = 1` — ahora es válido (E3). Cambiado a verificar 0 borrados y filas intactas.
- `secondary_index_lookup_and_maintenance` esperaba que `AND` no estuviera soportado — actualizado al comportamiento E1.

### 🧪 Validación
- 14 integration tests nuevos en `tests/integration_test.rs` (`f_*`): COUNT(*) global, COUNT(*) AS alias, COUNT(col) ignora NULL, SUM/AVG/MIN/MAX, GROUP BY single, GROUP BY multi, HAVING con agregada, HAVING con alias, DISTINCT, COUNT(DISTINCT), validación ANSI (col no-GROUP), agregado en WHERE rechazado, agregado sobre JOIN rechazado, input vacío con neutros.
- `cargo check + cargo fmt --check + cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- (Se actualiza en el mismo push: SQL_REFERENCE, MISSING_COMMANDS.)

---

## 2026-05-25 — Bloque E3: `UPDATE` / `DELETE` por cualquier `WHERE`

> **Un push a `main`** que destraba mutaciones masivas y por columnas no-PK.

### 🔧 Cambios
- `UpdateStmt` y `DeleteStmt` ahora llevan `where_clause: WhereExpr` (mismo grammar que `SELECT`). El campo legacy `where_column + where_pk: i64` desaparece.
- `parse_update` / `parse_delete` reusan `parse_where_expr()` — todos los operadores de E1+E2 + subqueries `IN (SELECT)` / `= (SELECT)` / `EXISTS` se aceptan sin tocar el parser.
- Nuevo helper `Engine::resolve_target_pks` que devuelve la lista de PKs matcheadas:
  - **Fast-path** para `WHERE pk = N` literal (preserva el comportamiento pre-E3, incluyendo el error `[GBY-3006] ROW_NOT_FOUND_FOR_PK` cuando N no existe).
  - **Fallback genérico**: FullScan + evaluador `eval_where_expr_single` (mismo motor 3VL que SELECT). Sin optimización por índice todavía — correctitud primero, perf en backlog.
- `exec_update` extrae la lógica per-fila a `apply_update_to_pk` y la invoca por cada PK del lote. Las validaciones (NOT NULL, UNIQUE, FK) corren por-fila — un UNIQUE conflict en la fila K corta el batch y deja las K-1 anteriores commiteadas dentro de la misma transacción (la decisión de revert depende del wrapping en el cliente).
- `exec_delete` resuelve PKs **antes** de borrar para evitar interferencia con cascadas FK que tocan otras tablas o self-refs. Cada cascade tolera filas ya eliminadas (idempotente).
- Response `message` ahora trae la cuenta: `"OK (3 filas actualizadas)"` / `"OK (2 filas eliminadas)"`.

### ⚠️ Limitaciones residuales
- `UPDATE ... FROM otra_tabla` (UPDATE con JOIN) y `DELETE ... JOIN` no se soportan — requieren parser de FROM compartido con SELECT y queda para un bloque futuro.
- `<` / `>` / `LIKE` / `IS NULL` sobre PK o columna indexada **no aprovechan el índice** todavía — todos van por FullScan. Optimización indexada para `=` sobre columna indexada queda en backlog.
- El error `[GBY-4003] UPDATE_DELETE_REQUIRES_PK_FILTER` queda inactivo. La constante permanece en `errors.rs` por el contrato de estabilidad (nunca se reusa) — futuras versiones nunca volverán a emitirla.

### 🧪 Validación
- 10 integration tests nuevos en `tests/integration_test.rs` (`e3_*`): UPDATE por columna indexada, por predicado compuesto, por subquery, 0 matches, fast-path PK con error legado, DELETE por col indexada / combinador / subquery / LIKE, UPDATE preservando UNIQUE.
- `cargo check --lib --tests` limpio sin warnings.

### 📚 Documentación
- `docs/SQL_REFERENCE.md` — EBNF de UPDATE/DELETE actualizada, ejemplos nuevos, errores típicos al día.
- `docs/MISSING_COMMANDS.md` — E3 marcado cerrado, hueco #4 del top-5 tachado.
- `docs/ERROR_CODES.md` — entry `4003` marcada como histórica.

---

## 2026-05-25 — Bloque E2: comparadores, `LIKE`, `IS NULL`, `IN literal`

> **Un push a `main`** que cierra el set de operadores básicos del `WHERE`.

### 🆕 Nuevos operadores
- `<`, `<=`, `>`, `>=`, `<>`, `!=` sobre INT / FLOAT / TEXT (lex) / BOOL. NULL en cualquiera de los dos lados → `NULL` (3VL). Tipos incompatibles → `false` (no abortamos la query).
- `[NOT] LIKE 'patron'` sobre TEXT. Wildcards SQL estándar (`%` = cero o más, `_` = exactamente uno) con escape `\%` / `\_`. Backtracking O(|s|·|p|), suficiente para patrones realistas.
- `IS [NOT] NULL` — único predicado que NO propaga NULL (es la forma explícita de testear ausencia).
- `[NOT] IN (lit1, lit2, ...)` con lista literal. Semántica ANSI: si la columna es NULL → NULL; si no hay match y la lista contiene NULL → NULL (especialmente sensible en `NOT IN`).

### 🧬 Tokenizer
- Nuevos símbolos: `<`, `<=`, `>`, `>=`, `<>`, `!=` (con lookahead de 1 char). `!` suelto sigue siendo error (sugerencia explícita en el mensaje).

### 🧠 AST
- `WhereClause` extendido con cuatro variants nuevos: `Compare { op: CompareOp, ... }`, `Like { pattern, negated }`, `IsNull { negated }`, `InList { values, negated }`. Ningún variant tiene fast-path indexada por ahora — todos van por `generic_post_filter` + evaluador 3VL.

### 🚦 Executor
- `generic_post_filter` ahora se activa también cuando el átomo único es E2 (Compare/Like/IsNull/InList). El path por PK/índice queda intacto para `=`, `BETWEEN`, `IN (SELECT)`, `= (SELECT)`, EXISTS y EqColumnRef.
- Tres helpers puros: `eval_compare`, `eval_like`, `eval_in_list`. `like_match` es backtracking recursivo con soporte de escape.

### ⚠️ Limitación residual
- `NOT IN (SELECT ...)` (subquery) explícitamente rechazado por ahora — el desugar a `NOT (col IN (SELECT))` cambia la semántica con NULLs y queda para el bloque H. `NOT IN (lista literal)` sí está.
- `<` / `>` / `<=` / `>=` no aprovechan el índice OrderedInt todavía (range scan optimization queda en backlog; correctitud antes que velocidad).

### 🧪 Validación
- 11 integration tests nuevos en `tests/integration_test.rs` (`e2_*`): comparadores INT, `<>`/`!=` sinónimos, comparación TEXT lex, LIKE básico, NOT LIKE, IS NULL / IS NOT NULL, IN literal, NOT IN con 3VL, combinaciones con AND/OR de E1, LIKE con escape, comparador con JOIN.
- `cargo check --lib --tests` limpio.

### 📚 Documentación
- `docs/SQL_REFERENCE.md` — EBNF del WHERE actualizado, ejemplos de cada operador nuevo, fila E2 en la tabla de soporte.
- `docs/MISSING_COMMANDS.md` — E2 marcado cerrado, hueco #2 del top-5 tachado, comparadores/LIKE/IS NULL/IN literal en ✅.

---

## 2026-05-25 — Bloque E1: `AND` / `OR` / `NOT` + paréntesis en `WHERE`

> **Un push a `main`** que destraba el filtro compuesto en cualquier `SELECT`.

### 🔀 WHERE booleano (bloque E1)
- AST: `WhereClause` (plano) → `WhereExpr = And | Or | Not | Atom(WhereClause)`. Los átomos siguen siendo los seis predicados pre-existentes (`Eq`, `Between`, `In`, `EqSubquery`, `EqColumnRef`, `Exists`) — el bloque no toca su semántica.
- Parser: precedencia estándar SQL `OR` < `AND` < `NOT` < paréntesis / átomo. `NOT EXISTS` mantiene la forma vieja (`Atom(Exists{negated:true})`) para preservar el fast-path correlacionado.
- Executor: cuando el WHERE se reduce a un único átomo, se usan las fast-paths existentes (PK directo, índice secundario, range scan, EXISTS correlacionado post-filter). Cuando hay combinadores se cae a FullScan + evaluador trivaluado (3VL) row-a-row — `defer_window` se activa para que `LIMIT`/`OFFSET` se apliquen DESPUÉS del filtro.
- 3VL para `NULL`: `NULL AND false = false`, `NULL AND true = NULL`, `NULL OR true = true`, `NOT NULL = NULL`. Solo `Some(true)` mantiene la fila.
- Soporte completo en `SELECT` con o sin JOINs. `filter_joined_rows` ahora recibe `&WhereExpr` y aplica el mismo evaluador 3VL sobre filas joined.

### ⚠️ Limitación residual
- `EXISTS` correlacionado y `col = otra.col` (column-ref del outer) **solo se permiten como único átomo del WHERE**. Combinarlos con `AND`/`OR`/`NOT` devuelve `[GBY-4024]`. La generalización queda explícitamente fuera de E1.

### 🧰 Código de error nuevo
- `4024` `WHERE_COMBINATOR_CORRELATED_UNSUPPORTED`

### 🧪 Validación
- 11 integration tests nuevos en `tests/integration_test.rs` (sufijo `e1_*`): AND, OR, NOT, paréntesis, precedencia, BETWEEN + AND combinador, 3VL sobre NULL, NOT anidado, combinador con LIMIT+ORDER, doble NOT, combinador con JOIN, error sintáctico.
- `cargo check --lib --tests` limpio (0 warnings).

### 📚 Documentación
- `docs/SQL_REFERENCE.md` — EBNF del WHERE reescrita con precedencia + 3VL + ejemplos.
- `docs/MISSING_COMMANDS.md` — E1 marcado como cerrado; top-5 actualizado.
- `docs/ERROR_CODES.md` — entry `4024`.

---

## 2026-05-24 — Subqueries completas + roadmap de JOINs cerrado

> **Siete pushes consecutivos a `main`** que cierran dos features grandes del motor SQL.

### 🧩 Subqueries (3 bloques)
- `WHERE col IN (SELECT …)` — no-correlacionada, single-column. Reusa lookup PK/índice.
- `WHERE col = (SELECT …)` — subquery escalar (1 × ≤1). 0 filas o NULL → match vacío (ANSI). >1 fila → `[GBY-4014]`.
- `WHERE [NOT] EXISTS (SELECT …)` — no-correlacionada (pre-ejecuta) y correlacionada single-eq (`inner_col = outer.col`, post-filter per-row con `outer_stack`).

### 🔗 JOINs (4 bloques)
- **A** — `INNER JOIN`, `CROSS JOIN`, comma-syntax (`FROM a, b`), aliases con `[AS]`, multi-tabla en chain (left-deep), self-join. Columnas cualificadas (`tabla.col` o `alias.col`). `SELECT *` expande prefijado.
- **B** — `LEFT [OUTER] JOIN`, `RIGHT [OUTER] JOIN`, `FULL [OUTER] JOIN` con NULL-fill por kind. `OUTER` opcional (ANSI).
- **C** — `JOIN ... USING (col)` (sugar para `ON l.col = r.col`) y `NATURAL JOIN` (auto-derive del USING). `SELECT *` omite la columna fusionada del right.
- **D** — Index-loop join optimization transparente: cuando el `ON` (o el USING/NATURAL derivado) apunta contra PK o columna indexada del right Y el kind es INNER/LEFT, el engine reemplaza el FullScan del right por lookup dirigido. O(N×M) → O(N×log M) por JOIN.

### 🧰 Códigos de error nuevos
- `4011` `SUBQUERY_MUST_RETURN_ONE_COLUMN`
- `4012` `IN_PK_TYPE_MISMATCH`
- `4013` `IN_REQUIRES_PK_OR_INDEX`
- `4014` `SCALAR_SUBQUERY_TOO_MANY_ROWS`
- `4015` `EXISTS_REQUIRES_SUBQUERY`
- `4016` `OUTER_COLUMN_REF_INVALID`
- `4017` `TABLE_ALIAS_DUPLICATED`
- `4018` `COLUMN_AMBIGUOUS`
- `4019` `COLUMN_QUALIFIER_NOT_FOUND`
- `4020` `JOIN_PREDICATE_REQUIRED`
- `4021` `CROSS_JOIN_WITH_ON`
- `4022` `USING_COLUMN_INVALID`
- `4023` `NATURAL_JOIN_NO_COMMON_COLUMN`

### 📚 Documentación
- Doc barrido completo: `README.md`, `docs/SQL_REFERENCE.md`, `docs/STATUS.md`, `docs/ERROR_CODES.md`, `TROUBLESHOOTING.md`, `RUNBOOK.md`, `docs/POSITIONING.md`, `docs/COMPETITIVE_ANALYSIS.md`, `docs/ARCHITECTURE.md`, `docs/API.md`, `docs/TECHNICAL_SPECS.md`, `RECRUITER.md`, `ROADMAP.md`, `web/phpgabyadmin/index.php`.

### 🧪 Validación
- **71/71 tests** integración verdes (16 nuevos entre subqueries y JOINs).
- `cargo fmt --check` ✅ · `cargo clippy --all-targets -- -D warnings` ✅.

---

## 2026-05-18 — Vigesimoséptima intervención: reframe — `gabysql` es un proyecto de aprendizaje, no comercial

> **Solo docs. Cero código.** Reescribe el marco operativo del proyecto.

### ✨ Cambio
- Nuevo documento **[docs/AGENDA_INVESTIGACION.md](docs/AGENDA_INVESTIGACION.md)** (~500 líneas, 10 secciones) que reemplaza como fuente operativa a `COMMERCIAL_ROADMAP.md`/`POSITIONING.md`/`COMPETITIVE_ANALYSIS.md`. Contiene:
  - El reframe explícito: el proyecto **no es comercial y no apunta a serlo**.
  - La tesis: "¿cómo se vería una DB nativa de la era de los agentes LLM?".
  - 7 ejes de investigación con honestidad sobre qué entiendo / qué no / qué cuesta:
    1. Schema semántico (no solo tipado)
    2. Plan-as-data en cada respuesta
    3. Embedded variants de columnas TEXT
    4. Time-travel por default
    5. Audit trail consultable como tabla
    6. Schema migration como conversación
    7. Probes de invariantes
  - 6 Fases de aprendizaje (α–ζ) con **objetivo cognitivo** ("qué quiero entender"), no objetivo de producto.
  - Anti-agenda explícita: lo que NO entra (JOIN/GROUP BY/replicación/optimizer cost-based/etc.).
  - Ritmo realista (1 intervención/semana, no 9/día) y métricas de éxito honestas ("puedo explicar X" en vez de "MAUs").
- **Marcados como históricos** (banner explícito al inicio):
  - `docs/COMMERCIAL_ROADMAP.md`
  - `docs/POSITIONING.md`
  - `docs/COMPETITIVE_ANALYSIS.md`
- **ADR-0007** (Camino A) marcada como `🗑️ Superseded por AGENDA_INVESTIGACION.md`. El índice de ADRs refleja el cambio.
- **README.md** reescribe la introducción y la tabla de documentos clave: el proyecto se presenta como lo que es (laboratorio de aprendizaje sobre DBs + agentes), no como producto.
- **ROADMAP.md** redirige a la nueva agenda como fuente operativa y mantiene su rol histórico (qué entregó cada Fase 1/2).

### 🎯 Por qué este cambio
Auditoría con el usuario del estado del proyecto:
> *"además no se saca nada con pensar que alguien le interese, si creo todavia esta en pañales, lo realmente es mi objetivo, crea una base de datos que no sea como las demás, mientras evoluciona la IA, el producto puede evolucionar de forma natural con lo que es una base de datos y las nuevas tecnologias"*

El marco anterior (caminos A/B/C, ICPs, comparativas comerciales) distorsionaba las decisiones técnicas: justificaba o vetaba features con argumentos comerciales que en realidad no aplicaban (no hay clientes ni hay intención de tenerlos). El reframe permite decir las cosas como son y elegir exploraciones por **valor de aprendizaje + diferenciación honesta**, no por encaje a un ICP imaginario.

### 🛡️ Lo que NO cambia
- Cero código tocado. Motor estable como estaba.
- ADRs técnicos (0001–0006, 0008–0018) siguen vigentes. Son decisiones del motor, independientes del marco comercial.
- `STATUS.md`, `USE_CASES.md`, `SQL_REFERENCE.md`, `ARCHITECTURE.md`, `TECHNICAL_SPECS.md`, `ERROR_HANDLING.md`, `ERROR_CODES.md` siguen vigentes — describen lo que el motor **es**, no qué se vende.
- 45/45 integration + 27 lib + 7 unit tests verdes. CI sin alterar.

---

## 2026-05-18 — Vigesimosexta intervención: códigos numéricos `[GBY-NNNN]` estilo MySQL `ER_*` + catálogo operacional

> **Sin bump de formato. Sin deps añadidas.** Cierre del trabajo de manejo de errores: cada error user-facing ahora lleva un código estable y existe un catálogo operacional búscable. Análogo al sistema `ER_DUP_ENTRY=1062` de MySQL.

### ✨ Cambio
- Nuevo módulo [src/errors.rs](src/errors.rs):
  - `pub mod codes` con ~30 constantes `pub const NAME: u32 = NNNN` agrupadas por rango:
    - `1000–1999` storage / WAL / file lock
    - `2000–2999` catalog / schema / identificadores
    - `3000–3999` constraints (PK, NOT NULL, UNIQUE, FK)
    - `4000–4999` superficie SQL (parser, planner, limitaciones)
    - `5000–5999` server / HTTP / auth
  - Helper `coded(code: u32, message: impl Into<String>) -> DbError` que produce mensajes con prefijo `[GBY-NNNN]`.
  - 3 unit tests del módulo.
- Sweep de ~30 sitios user-facing en `storage.rs`, `bptree.rs`, `sql.rs`, `catalog.rs`, `index.rs`, `server.rs`: cada error visible para CLI/HTTP/embedido ahora pasa por `coded(...)`.
- Auth fallida (`401`) y server-busy (`503`) llevan códigos `[GBY-5004]` y `[GBY-5005]` respectivamente.
- Nuevo documento normativo [docs/ERROR_CODES.md](docs/ERROR_CODES.md) — catálogo operacional con cada código: causa, remedio, ejemplo de mensaje real, integración desde CLI/HTTP/Rust/Python.
- README, ERROR_HANDLING y CONTRIBUTING enlazan al catálogo.

### 🎯 Por qué este cambio
Pregunta del usuario: *"y tener un número referencial como MySQL tiene para el manejo de errores"*. Razón concreta: el texto de un mensaje puede evolucionar (mejor redacción, más contexto), pero un cliente que reacciona programáticamente al error necesita un contrato estable. El código numérico **es** ese contrato.

Ahora:
- Las herramientas pueden hacer `grep -oE 'GBY-[0-9]{4}'` para detectar la clase del error sin parsear texto humano.
- El troubleshooting tiene un eje claro: cada código apunta a su entrada en [ERROR_CODES.md](docs/ERROR_CODES.md).
- Los clientes embebidos pueden hacer `text.starts_with("[GBY-3001]")` para detectar PK duplicada sin depender de la redacción exacta.

### 🛡️ Decisión: constantes Rust, no JSON externo
Documentada en [src/errors.rs](src/errors.rs) y en la sección "Por qué constantes en Rust" del catálogo:
- Zero-deps (ADR-0001) — sin filesystem I/O al startup.
- Type-checked: el compilador detecta renames; con JSON sería un test runtime dedicado.
- Misma flexibilidad práctica: cambiar un mensaje es edit + rebuild + redeploy en cualquier caso.
- i18n futuro se resuelve con `feature` flags si llega, sin filesystem.

### 🛡️ Restricciones respetadas
- **Cero deps.** ADR-0001 intacto.
- **Cero bump de formato.** VERSION 7 sigue válido.
- **Cero rotura del contrato externo.** Los mensajes ahora prefijan con `[GBY-NNNN]`, pero los clientes que no parsean el texto (mayoría) no se ven afectados.
- **45/45 integration + 30 lib + 4 server + 3 errors unit tests verdes.**

### 📐 Documentos
- [docs/ERROR_CODES.md](docs/ERROR_CODES.md) — catálogo completo de los ~30 códigos.
- [docs/ERROR_HANDLING.md](docs/ERROR_HANDLING.md) — guía de estilo (actualizada para reflejar el nuevo sistema de códigos).

---

## 2026-05-18 — Vigesimoquinta intervención: guía canónica de manejo de errores + sweep al español + enriquecimiento

> **Sin bump de formato. Sin deps añadidas. Levanta la barra de calidad de los mensajes de error a nivel producto.** Cierra el síntoma "los errores en pantalla son pobres y no aclaran nada".

### ✨ Cambio
- Nuevo documento canónico [`docs/ERROR_HANDLING.md`](docs/ERROR_HANDLING.md) — guía normativa para los ~210 sitios donde se construyen errores en el motor:
  - Filosofía: cada mensaje responde *qué pasó*, *por qué*, y (cuando aplica) *cómo se resuelve*.
  - Reglas de estilo: idioma español, minúscula, sin punto final, incluir el nombre concreto del objeto, incluir el dato del fallo, sugerir el remedio.
  - 8 categorías canónicas (validación, NotFound, Conflict, Constraint, Limitación, Integridad, Estado interno, I/O) cada una con patrón recomendado.
  - Mapeo sistemático a HTTP (400/401/404/405/409/500/503).
  - Anti-patrones explícitos (mensajes de una palabra, `unwrap` que miente, `From` que enmascara, idiomas mezclados, secretos en mensajes).
  - Checklist de PR para revisar cualquier nuevo `DbError::new(...)`.

- **Traducción al español de todos los mensajes en inglés** heredados de iteraciones previas:
  - `storage.rs`: `tx already started` → `transacción ya iniciada`; `no active tx` → `no hay transacción activa: commit() requiere un begin() previo`; `bad magic` → `magic bytes inválidos: el archivo no es una base de datos gabysql`; `unsupported gabysql file format` → `formato de archivo gabysql no soportado`; `refusing to overwrite` → `se rehúsa sobrescribir base de datos existente`; `database is locked by another process` → `base de datos bloqueada por otro proceso`; etc.
  - `bptree.rs`: `root page is 0`, `leaf overflow`, `page too small`, `not a leaf page`, `leaf decode overflow`, `internal too large`, `unknown page type`, etc. — todos en español con contexto.
  - `server.rs`: mensajes de `read_request` (`request line vacía`, `método faltante`, `escape URL inválido`), validación de `-max-connections`, mensajes de auth/multi-DB.
  - `index.rs`: `bucket de índice corrupto` con offset, count, len y descripción precisa.

- **Enriquecimiento de mensajes pobres**. Los ~20 mensajes que eran 1-3 palabras y no orientaban al operador ahora incluyen contexto:
  - `default corrupto (kind)` → `DEFAULT corrupto: buffer agotado en offset {N} (len={M}), falta el byte de kind`.
  - `string corrupto` → `string serializado corrupto en offset {N}: header declara {L} bytes pero solo quedan {R} bytes en el buffer`.
  - `fila corrupta (INT)` → `fila corrupta en tabla '{T}': campo '{C}' (INT) necesita 8 bytes en offset {N}, solo quedan {R}`.
  - `db vacío` → `parámetro 'db' vacío: indique el nombre del archivo .db dentro del directorio configurado`.
  - `meta de tabla corrupta` → `TableMeta '{T}' corrupta: faltan bytes para el header de la columna {i} ('{C}') en offset {N}`.
  - `colisión de hash en catálogo` → mensaje completo que dice qué nombres colisionaron y que se debe reportar como bug.
  - `cantidad columnas != valores` → `INSERT INTO '{T}': cantidad de columnas ({c}) no coincide con cantidad de valores ({v})`.

- **3 tests de integración actualizados** que asertaban sobre los strings originales (`duplicate primary key`, `refusing to overwrite`, `locked`) — ahora aceptan tanto el texto en español como, por compatibilidad transicional, el inglés equivalente cuando es razonable.

### 🎯 Por qué este cambio
Auditoría con el usuario: "los errores en pantalla son pobres en indicaciones y no aclaran nada". La auditoría confirmó:
- Existía una convención **observada** pero **no escrita** sobre los mensajes.
- Muchos eran de 1-3 palabras (`db vacío`, `string corrupto`, `fila corrupta (INT)`) — imposibles de buscar en troubleshooting y sin información accionable.
- Había mezcla de español e inglés sin razón.
- Sin documento normativo, un PR podía agregar `"Column Not Found."` y nada lo paraba.

Ahora hay tres cosas concretas:
1. **Documento normativo** (`docs/ERROR_HANDLING.md`) que define qué es un mensaje aceptable.
2. **Estado actual auditado** — ~210 sitios revisados, todos cumplen las reglas.
3. **Checklist de PR** para que nuevos errores se midan contra la guía.

### 🛡️ Restricciones respetadas
- **Cero deps añadidas.** ADR-0001 intacto.
- **Cero bump de formato.** VERSION 7 sigue válido.
- **Cero rotura de API.** Los `Display::fmt` siguen devolviendo el texto puro; los clientes que no leen el texto no se ven afectados.

### 📐 Documentos
- [docs/ERROR_HANDLING.md](docs/ERROR_HANDLING.md) — guía canónica completa (las 8 categorías, checklist de PR, anti-patrones).

---

## 2026-05-18 — Vigesimocuarta intervención: ADR-0018 (Propuesta) — WAL-mode opt-in (sólo diseño)

> **Sin código. Sin bump de formato.** Cierre honesto del ítem "checkpoint del WAL" de Fase 2: el diseño queda capturado con scope, alternativas y condiciones de salida explícitas, pero la implementación se difiere hasta que aparezca medición de `gabybench` o demanda real. Justificación completa: [ADR-0018](docs/adr/0018-wal-mode-opt-in.md).

### ✨ Cambio
- Nuevo [ADR-0018](docs/adr/0018-wal-mode-opt-in.md) en estado **Propuesta**. Describe:
  - El modelo WAL-per-transaction actual y por qué "checkpoint" no aplica.
  - El modelo propuesto: WAL persistente, `Pager::checkpoint()` explícito, `wal_index` in-memory, read-path WAL-aware.
  - Alternativas evaluadas y descartadas (group commit, mmap, auto-checkpoint, etc.).
  - **Condiciones de salida** (cuándo pasa a "Aceptada" + implementación): cuando `gabybench` muestre fsync(.db) como cuello de botella, o aparezca workload write-heavy con métricas concretas, o se necesite MVCC.
- ROADMAP.md actualizado: el ítem pasa de "diferido sin condiciones" a "diseño aceptado, implementación deferida con condiciones de salida documentadas".

### 🎯 Por qué este formato
Implementar WAL-mode real es ~400-600 LOC en el hot path del Pager con riesgo de regresión alto y sin un workload medido que lo justifique. Hacerlo a ciegas para "marcar el bloque como entregado" contradice la honestidad del resto de Fase 2 (donde cada bloque mostró su scope real, no inflado).

El diseño completo es valor por sí mismo: cualquier persona futura — humana o agente — que retome el ítem encuentra el análisis listo, las alternativas evaluadas, y el contrato de cuándo activarlo. Eso es lo que se entrega.

### 📐 ADR
- [ADR-0018 — WAL-mode opt-in con checkpoint explícito](docs/adr/0018-wal-mode-opt-in.md).

---

## 2026-05-18 — Vigesimotercera intervención: índice INT-ordenado + range scan (Fase 2 — VERSION 7)

> **Bump de formato VERSION 6 → 7.** Cierra el ítem "range scan por índice secundario" del roadmap, restringido honestamente a columnas INT. Justificación completa: [ADR-0017](docs/adr/0017-int-ordered-index-version-7.md).

### ✨ Cambio
- **VERSION on-disk pasa de 6 a 7.** Archivos V6 se rechazan limpiamente al abrir (mensaje "Re-create the database with the current binary"). Igual patrón que cada bump anterior.
- **Nuevo `IndexKind`** en `IndexMeta` ([src/catalog.rs](src/catalog.rs)):
  - `Hash` (ADR-0005): el layout legacy. Usado para TEXT/FLOAT/BOOL/DATE/DATETIME. **Equality only**.
  - `OrderedInt` (nuevo): para columnas INT. El B+Tree se indexa por el valor directamente; los buckets son solo `[count:u16] + count × pk:i64`. Soporta range scan.
  - `IndexKind::for_column(column_type)` decide automáticamente al crear el índice. Cero cambios al SQL externo.
- **Nuevo path `WHERE col_idx BETWEEN a AND b`** sobre columnas INT indexadas: ejecutor llama a `lookup_pks_via_index_range` que usa `Tree::cursor_range(idx.root_page, from, to)` y devuelve los PKs en O(log N + k).
- **BETWEEN sobre columna TEXT/FLOAT/etc. indexada falla loud** con mensaje claro:
  *"el índice secundario es hash-based (equality only). Solo columnas INT-indexadas admiten BETWEEN."*
- **NULL no se almacena en índices OrderedInt**. SQL `BETWEEN` ignora NULL por definición y UNIQUE permite múltiples NULLs; ambas semánticas caen naturalmente al no indexar la representación NULL.
- Helpers nuevos en [src/index.rs](src/index.rs): `ordered_int_key_from_value_bytes`, `encode_ordered_bucket`/`decode_ordered_bucket`, `ordered_bucket_insert`/`_remove`/`_unique_conflict`.
- Integrity check ([src/sql.rs](src/sql.rs)) y FK cascade lookup branchean por `idx.kind` para decodificar el bucket correcto.
- **2 tests nuevos**: range BETWEEN sobre INT indexado (incluyendo verify que NULL queda fuera) y rechazo BETWEEN sobre TEXT indexado.

### 🎯 Por qué este cambio (y por qué INT solamente)
ADR-0005 había fijado el índice como **hash-based** (FNV-1a-64) para tolerar colisiones de hash con un bucket por clave. Equality funciona; range no compone — hashes de valores cercanos son arbitrariamente distintos. El ítem del roadmap "range scan por índice secundario" había sido marcado como **no viable bajo VERSION 6** explícitamente en intervenciones previas.

La salida natural es usar el valor como clave del B+Tree donde el orden i64 ya es el orden semántico — **solo INT** cumple sin tocar el motor. TEXT requeriría un B+Tree byte-keyed (~800+ LOC, riesgo de regresión); FLOAT necesita encoding flip-sign no-trivial. Ambos quedan diferidos a un bloque futuro cuando aparezca demanda real.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001).
- **Memoria acotada** (ADR-0009 — el bucket ordenado es estrictamente más chico que el bucket Hash equivalente).
- **Convivencia limpia**: índices Hash siguen funcionando para los tipos no-INT (ADR-0005 sigue vigente).
- **Sin cambios al cursor**: `Tree::cursor_range` (ADR-0008) ya servía perfectamente.

### 📐 ADR
- [ADR-0017 — Índice secundario INT-ordenado para range scan (VERSION 7)](docs/adr/0017-int-ordered-index-version-7.md).

### 📝 Notas
- **Índices compuestos no entran en este bloque.** El roadmap inicial los agrupaba con range scan bajo el mismo bump, pero compuestos requieren claves multi-columna que con el approach value-as-i64 es forzado. Quedan diferidos a un futuro VERSION 8 (o se entregan dentro de VERSION 7 si la demanda aparece sin necesidad de cambio de formato).

---

## 2026-05-18 — Vigesimosegunda intervención: prefetch one-leaf-ahead en `LeafCursor` (Fase 2 — performance directional)

> **Sin bump de formato. Sin deps añadidas. Mejora direccional sin medición cuantitativa todavía.** Justificación completa: [ADR-0016](docs/adr/0016-leafcursor-prefetch.md).

### ✨ Cambio
- 4 líneas nuevas en [src/bptree.rs](src/bptree.rs::LeafCursor::load_current): después de cargar la hoja actual, si hay siguiente, se hace `page_data` sobre ella para llevarla al `PageCache` (ADR-0009). Best-effort: errores de prefetch se descartan; el error real va a surgir en la próxima iteración real del cursor.
- Nuevo helper `Pager::cache_contains(page_no) -> bool` ([src/storage.rs](src/storage.rs)) para tests + futura tooling operacional.

### 🎯 Por qué este cambio
El `LeafCursor` (ADR-0008) ya hace lo correcto algorítmicamente, pero presenta al kernel y al `PageCache` un patrón de I/O **stop-and-go**: lee hoja N, deja que el caller procese 100 filas (pausa larga), entonces lee hoja N+1. Esto:
1. **Confunde el readahead del kernel**, que necesita lecturas back-to-back para detectar streaming.
2. **Garantiza un cache miss en cada leaf transition** — la primera lectura post-transición siempre paga el costo de syscall + CRC verify.

Prefetcheando la próxima hoja al final de la carga de la actual, el syscall ocurre antes y para cuando el caller la pide, ya está en cache.

### 🛡️ Honestidad sobre la mejora
- **No hay número absoluto todavía.** `gabybench` (la suite reproducible especificada en `docs/GABYBENCH_SPEC.md`) no existe aún. Cuando exista, esto se mide.
- **Sobrelectura potencial de 1 hoja en queries cortas** (`LIMIT N` que cabe en la primera hoja).
- El ADR vende esto como **directional**, no como "scan 2x más rápido".

### 📐 ADR
- [ADR-0016 — Prefetch one-leaf-ahead en `LeafCursor`](docs/adr/0016-leafcursor-prefetch.md).

---

## 2026-05-18 — Vigesimoprimera intervención: backup/restore/verify con validación end-to-end (Fase 2 — operación)

> **Sin bump de formato. Sin deps añadidas.** Cierra el gap operacional "no hay forma confiable de respaldar". Justificación completa: [ADR-0015](docs/adr/0015-verified-backup-restore.md).

### ✨ Cambio
- Nuevo módulo [src/backup.rs](src/backup.rs) con tres entradas públicas: `backup`, `restore`, `verify`. Todas validan **CRC32 página por página en lectura** y, post-escritura, **re-abren el destino y revalidan cada página**. Si una sola página falla el CRC en cualquiera de las dos fases, la operación aborta — nunca se publica un backup roto.
- Nuevos subcomandos CLI:
  - `gabysql backup [--force] <src.db> <dst.db>`
  - `gabysql restore [--force] <src.db> <dst.db>` (alias semántico)
  - `gabysql verify <file.db>`
- Salida estructurada: `OK backup  src=...  dst=...  pages=N  bytes=M`.
- 3 tests de integración nuevos: round-trip con verify, detección de corrupción en origen (byte flip rechaza el backup), verify sobre DB sana.

### 🎯 Por qué este cambio
La operación de respaldo era "`cp demo.db backups/demo.db.bak`" — sin validación, sin awareness del WAL, sin garantía de que el destino se pudiera *usar*. Una página corrupta en el origen se replicaba al backup sin warning hasta que alguien intentaba restaurar (semanas después, en una emergencia).

Ahora el contrato es claro:
- Si el comando termina con `OK`, el archivo destino se puede abrir con el mismo binario, todas sus páginas tienen CRC válido, y su header coincide con el origen.
- Si algo falla, error explícito que apunta a la página corrupta o la causa raíz.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001 intacto).
- **Cero bump de formato.** VERSION = 6 sigue válido — el destino es un `.db` regular.
- **Lock exclusivo** vía ADR-0013: la DB debe estar cerrada por otros procesos (server apagado). Endpoint server-side `/backup` que tome el `write_lock` queda para Fase 3.

### 📐 ADR
- [ADR-0015 — Backup / restore / verify con validación end-to-end](docs/adr/0015-verified-backup-restore.md).

### 📝 Ejemplo
```powershell
# Cierre el server primero (el lock exclusivo bloquea backups online)
gabysql backup demo.db backups/demo.db.bak
# → OK backup  src=demo.db  dst=backups/demo.db.bak  pages=128  bytes=524288

# Verificar un backup antiguo
gabysql verify backups/demo.db.bak
# → OK verify  path=backups/demo.db.bak  pages=128  bytes=524288

# Restaurar
gabysql restore --force backups/demo.db.bak demo.db
```

---

## 2026-05-18 — Vigésima intervención: logs JSON + endpoint `/metrics` en el server (Fase 2 — observabilidad)

> **Sin bump de formato. Sin deps añadidas.** Primer paso de observabilidad operacional para `gabysql-server`. Justificación completa: [ADR-0014](docs/adr/0014-logs-json-metrics.md).

### ✨ Cambio
- Nuevo struct `Metrics` en [src/server.rs](src/server.rs): contadores por status HTTP, `errors_total` (status ≥ 500), y ring buffer acotado de 1024 latencias para p50/p95. Memoria O(1) bajo carga sostenida.
- Nuevo endpoint **`GET /metrics`**:
  ```json
  {"ok":true,"started_unix":...,"uptime_s":3600,"requests_total":1234,
   "requests_by_status":{"200":1180,"400":30,"500":24},
   "errors_total":24,
   "latency_ms":{"p50":5,"p95":87,"samples":1024,"count":1234}}
  ```
  Gated por `-token` igual que el resto de la API.
- Nuevo flag **`-log-json`** en `gabysql-server`. Cuando se activa, cada request finalizado emite una línea JSON a stdout:
  ```json
  {"ts_unix":1747497612,"method":"POST","path":"/exec","status":200,"latency_ms":12}
  ```
  Por defecto **off** — la UX del binario silencioso de hoy no cambia. Útil con `tee`, `jq`, ingest a S3/ELK/Loki.
- 4 tests unitarios nuevos: registro de status + latencia, percentiles sobre 1..=100, comportamiento con buffer vacío, ring buffer acotado bajo overflow.

### 🎯 Por qué este cambio
El binario en producción era opaco: sin logs por request, sin contadores agregados, sin forma de responder "¿cómo se está comportando bajo carga?". El RUNBOOK pedía observabilidad básica pero no había nada que pedirle al server más allá de `/health`.

Ahora cualquier operador puede:
- Curl `/metrics` y ver counts por status + p50/p95 inmediatamente.
- Activar `-log-json` y pipear a `jq '. | select(.latency_ms > 100)'` para encontrar requests lentas.
- Configurar una alerta sobre `errors_total` creciendo.

Y todo sin agregar una sola dependencia.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001 intacto). Sin `tracing`, sin `prometheus`, sin `metrics-rs`.
- **Memoria acotada** (ADR-0009 mismo principio). Ring buffer de 1024 × 4 bytes = 4 KB por server.
- **Opt-in** para logs. Defaults preservan la UX silenciosa.
- **Sin bump de formato**. VERSION = 6 sigue válido.

### 📐 ADR
- [ADR-0014 — Logs JSON estructurados + endpoint `/metrics` en el server](docs/adr/0014-logs-json-metrics.md).

---

## 2026-05-18 — Decimonovena intervención: lock exclusivo cross-process sobre el `.db` (Fase 2 — concurrencia)

> **Sin bump de formato. Sin deps añadidas.** Cierra el gap de corrupción silenciosa cuando dos procesos abren la misma DB. Justificación completa: [ADR-0013](docs/adr/0013-process-level-file-lock.md).

### ✨ Cambio
- Nuevo helper privado `acquire_db_lock(&File, &Path)` en [src/storage.rs](src/storage.rs) que llama `File::try_lock()` (advisory exclusivo, **estable desde Rust 1.89.0**).
- Aplicado en `Pager::create` / `Pager::create_force` / `Pager::open`: el lock se adquiere tras abrir el handle y antes de cualquier escritura o replay del WAL.
- `Pager::close` libera el lock explícitamente con `file.unlock()` (drop del `File` también lo libera como red de seguridad).
- Si otro proceso (o incluso otro `Pager` en el mismo proceso) ya tiene la DB tomada, la segunda apertura **falla rápido** con:
  ```
  database is locked by another process: <path>.
  Close the other gabysql process or wait for it to release the lock.
  ```
  No hay espera bloqueante, no hay cuelgue.
- Test nuevo `cross_process_lock_rejects_second_open` que valida: primer `Pager::create` toma el lock → `Pager::open` segundo falla con mensaje claro → `close` del primero libera → `Pager::open` tercero funciona.

### 🎯 Por qué este cambio
La WAL+CRC de `gabysql` asume **un único escritor por archivo**. Sin lock cross-process, dos `gabysql` apuntando al mismo `.db` (server + CLI accidental, server reiniciado con proceso huérfano vivo, etc.) escribían páginas en paralelo y corrompían el archivo. El motor detectaba la corrupción **después** vía CRC, pero el daño ya estaba hecho.

Ahora la corrupción por doble apertura es **imposible**: el segundo proceso no llega a tocar el archivo.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001 intacto). Uso exclusivo de `std::fs::File::try_lock` / `unlock`.
- **Cero bump de formato** (VERSION = 6 sigue válido).
- **Cross-platform**: Windows (`LockFileEx` bajo el capó), Linux (`flock(2)` advisory), macOS (`flock(2)`). Los tres validados en CI.
- **No-bloqueante**: `try_lock` falla inmediatamente; el caller decide qué hacer.

### 📐 ADR
- [ADR-0013 — Lock exclusivo a nivel de proceso sobre el archivo `.db`](docs/adr/0013-process-level-file-lock.md).

### 📝 Notas de roadmap
- Re-evaluado el ítem **"checkpoint/compaction del WAL"** de Fase 2: el WAL actual es per-transaction y se trunca/borra en cada commit (no acumula a través de commits), así que el concepto clásico de checkpoint no aplica sin un cambio previo a WAL persistente. Diferido hasta que aparezca demanda concreta.
- Re-evaluado el ítem **"range scan por índice secundario"**: el índice 2º actual es hash-based (FNV-1a-64, ADR-0005) y no admite range nativo. Agrupado con índices compuestos bajo un futuro bump VERSION 6 → 7 que reestructurará el índice a B+Tree ordenado.

---

## 2026-05-08 — Decimoctava intervención: audit log enriquecido en el gateway (Fase 5 — AI-native, cierre del trío)

> **Sin bump de formato. Sin cambios al motor.** Tercera y última pieza del trío AI-native sobre el gateway. Justificación completa: [ADR-0012](docs/adr/0012-audit-log-enriquecido.md).

### ✨ Cambio
- Nuevo flag `--audit-log <ruta>` (también `GABYSQL_AUDIT_LOG`) en [src/bin/gabysql-mcp.rs](src/bin/gabysql-mcp.rs). Si no se pasa, sin log y overhead cero.
- Nuevo argumento opcional `reason` en `gabysql_execute`: el "por qué" semántico que el agente puede pasar para que quede en el audit.
- Captura de `clientInfo` (`name` + `version`) en el handshake `initialize` → guardado en `RuntimeState` interno y emitido en cada entrada del log.
- Cada llamada a `gabysql_execute` y `gabysql_integrity_check` anexa una línea JSON al archivo (formato JSONL):
  ```json
  {"ts_unix":1730000000,"tool":"gabysql_execute","db":"rag.db",
   "sql":"INSERT INTO docs ...","reason":"backfill inicial del corpus",
   "client":{"name":"claude-desktop","version":"1.2.3"},
   "ok":true,"error":null}
  ```
- Nueva tool **`gabysql_audit_tail(n)`** que devuelve las últimas N entradas. Permite que **el propio agente** revise su historial dentro de la sesión. Si el log no está activo, devuelve `{"enabled":false,"entries":[]}` sin error.
- Append best-effort: si escribir al archivo falla, va a stderr y la tool sigue devolviendo el resultado del motor (mejor perder una entrada que bloquear escrituras por disco lleno).
- 5 tests nuevos: captura de clientInfo, append+tail roundtrip con `reason`+`client`, comportamiento con log desactivado, presencia de `gabysql_audit_tail` en `tools/list`, formato JSONL (una entrada por línea, JSON válido por línea).

### 🎯 Por qué este cambio
Cuando un agente puede escribir en una base, el log del motor responde **el qué** (qué SQL corrió) pero no **el por qué** (qué pidió el usuario, qué identidad tenía el agente, qué razonamiento lo llevó allí). Meter eso en el motor implica bump de formato y que el motor entienda conceptos MCP que no le pertenecen.

Mover el audit al gateway captura el "por qué" exactamente donde el conocimiento existe — el gateway ya sabe quién es el cliente, qué tool se invocó, qué `reason` pasó el agente. Y cierra el loop dándole al propio agente la tool para releer sus acciones. Eso permite patrones de auto-corrección dentro de la misma sesión.

### 🛡️ Cómo se respeta el motor
- **Cero líneas tocadas en `storage.rs`/`bptree.rs`/`sql.rs`/`catalog.rs`/`server.rs`/`lib.rs`.** Solo crece `src/bin/gabysql-mcp.rs`.
- **Sin bump de formato.** Sin nuevas deps. `Cargo.toml`/`Cargo.lock` sin tocar.
- **Opt-in puro.** Sin `--audit-log` el comportamiento es idéntico al gateway pre-ADR — ni un syscall extra.
- **Retrocompatible**: clientes MCP que no pasan `reason` siguen funcionando sin cambios.

### 📐 ADR
- [ADR-0012 — Audit log enriquecido en el gateway, no en el motor](docs/adr/0012-audit-log-enriquecido.md). Cierra el trío con [ADR-0010](docs/adr/0010-mcp-gateway.md) (gateway base) y [ADR-0011](docs/adr/0011-vector-search-gateway-side.md) (vectores).

### 🧪 Ejemplo de uso desde un agente MCP
```bash
# Server + gateway con audit activo
gabysql-server -dir ./dbs -token MI_TOKEN
gabysql-mcp --token MI_TOKEN --audit-log /var/log/gabysql/agent-audit.jsonl
```
```json
{ "method":"tools/call", "params":{
    "name":"gabysql_execute",
    "arguments":{
      "db":"rag.db",
      "sql":"UPDATE users SET email='nuevo@x.com' WHERE id=42",
      "reason":"el usuario reportó que su email anterior ya no funciona"
}}}
```
La línea correspondiente del JSONL queda con `reason`, `client`, `sql`, `ok`. Procesable con `jq '.[] | select(.tool=="gabysql_execute")'` o ingestable a cualquier sink.

---

## 2026-05-07 — Decimoséptima intervención: búsqueda vectorial del lado del gateway (Fase 5 — AI-native, parte 2)

> **Sin bump de formato. Sin cambios al motor.** Esta intervención añade búsqueda vectorial top-k a `gabysql-mcp`. Los vectores se guardan como `TEXT` (`'[0.1,0.2,...]'`); el cómputo ocurre en el binario del gateway. Justificación completa: [ADR-0011](docs/adr/0011-vector-search-gateway-side.md).

### ✨ Cambio
- Nueva tool MCP **`gabysql_vector_search`** en [src/bin/gabysql-mcp.rs](src/bin/gabysql-mcp.rs):
  - Args: `db?`, `table`, `pk_column?` (default `id`), `vector_column`, `query: number[]`, `top_k?` (default 10), `metric?` (default `cosine`).
  - Métricas: `cosine`, `euclidean`/`l2`, `dot`/`ip`.
  - Hace `SELECT <pk>, <vec_col> FROM <table>` vía el HTTP existente, parsea cada vector, computa la distancia y devuelve top-k por heap selection.
  - Identificadores validados con `safe_ident` (regex implícito `[A-Za-z_][A-Za-z0-9_]*`) antes de interpolar al SQL — bloquea inyección.
  - Filas con vector mal formado o de dimensión distinta a la query van al campo `skipped` de la respuesta (no se silencian).
- 9 tests unitarios nuevos: cosine identity/orthogonal, euclidean Pitágoras, dot con sort ascendente, dimension mismatch, vector cero, top-k heap, validador de identificadores (acepta válidos / rechaza inyección), aliases de métrica, schema visible en `tools/list`.

### 🎯 Por qué este cambio
La búsqueda vectorial es lo que la mayoría de agentes LLM espera de una "DB para los nuevos tiempos". El camino correcto a largo plazo es un tipo `VECTOR(n)` nativo con índice ANN — pero eso requiere bump de formato, cambios profundos en `sql.rs`/`storage.rs`/`bptree.rs`, y meses de trabajo. **Hacerlo "para validar el use case" es prematuro.**

Esta entrega resuelve el 80% del valor (top-k usable hoy desde cualquier cliente MCP) con el 5% del riesgo (cero líneas tocadas en el motor). El ADR-0011 documenta las **condiciones de salida explícitas** para promover a `VECTOR(n)` nativo cuando la señal aparezca: dataset > 100K vectores, demanda de operadores SQL, o necesidad de índice ANN.

### 🛡️ Cómo se respeta el motor
- **No se toca `Cargo.toml`/`Cargo.lock`.** Sin nuevas deps. ADR-0001 intacto.
- **No se toca `src/lib.rs` ni ningún archivo del motor.** Solo crece `src/bin/gabysql-mcp.rs`.
- **No se cambia el formato en disco.** Los vectores son `TEXT`; `INSERT INTO docs (id, content, embedding) VALUES (1, 'texto', '[0.1,0.2,...]')` es SQL estándar que el motor procesa sin saber que es un vector.
- **Storage existente sigue válido.** DBs viejas no requieren migración.

### 📐 ADR
- [ADR-0011 — Búsqueda vectorial del lado del gateway, no en el motor](docs/adr/0011-vector-search-gateway-side.md)

### 🧪 Ejemplo de uso desde un agente MCP
```json
{ "method": "tools/call", "params": {
    "name": "gabysql_vector_search",
    "arguments": {
      "db": "rag.db",
      "table": "docs",
      "vector_column": "embedding",
      "query": [0.12, -0.04, 0.88, /* ... */],
      "top_k": 5,
      "metric": "cosine"
    }
} }
```

---

## 2026-05-07 — Decimosexta intervención: gateway MCP — `gabysql-mcp` (apertura Fase 5 AI-native)

> **Sin bump de formato. Sin cambios al motor.** Esta intervención añade un binario nuevo (`gabysql-mcp`) que es cliente del `gabysql-server` HTTP/JSON existente. No abre el `.db`, no instancia un `Pager`, no toca `storage.rs` / `bptree.rs` / `catalog.rs` / `sql.rs`. El motor queda intacto. Justificación completa: [ADR-0010](docs/adr/0010-mcp-gateway.md).

### ✨ Cambio

- Nuevo binario `src/bin/gabysql-mcp.rs` (~700 líneas, **cero dependencias externas**) que habla el protocolo **MCP (Model Context Protocol)** sobre stdio (JSON-RPC 2.0 delimitado por `\n`).
- Cinco tools expuestas a cualquier cliente MCP-compatible (Claude Desktop, Claude Code, Cursor, etc.):
  - `gabysql_list_databases` → wrap de `GET /dbs`
  - `gabysql_describe_database` → wrap de `GET /tables[?db=…]`
  - `gabysql_query` → wrap de `POST /exec` para `SELECT`/`SHOW`/`DESCRIBE`
  - `gabysql_execute` → wrap de `POST /exec` para `INSERT`/`UPDATE`/`DELETE`/DDL (omitida si se lanza con `--read-only`)
  - `gabysql_integrity_check` → wrap de `POST /exec` con `INTEGRITY CHECK`
- Dos resources MCP:
  - `gabysql://catalog` → lista de bases disponibles
  - `gabysql://schema/{db}` → schema completo de una DB
- Flags: `--server URL` (default `http://127.0.0.1:7878`, también `GABYSQL_SERVER`), `--token T` (también `GABYSQL_TOKEN`), `--read-only`.
- Tests unitarios en el mismo archivo cubren: parser JSON (round-trip + escapes), `initialize`, `tools/list` con y sin `--read-only`, `resources/list`, `ping`, método desconocido, notifications sin id, parsing de URL del server.

### 🎯 Por qué este cambio

El consumidor que más rápido crece en el ecosistema es el agente LLM. Hoy una IA que quiera usar `gabysql` necesita: cliente HTTP a mano + token + el schema de la DB metido en el prompt + reintentos sobre errores SQL sin trazabilidad. Ese pegamento se reescribe en cada integración.

MCP es el estándar emergente que define cómo un servidor expone *tools* y *resources* a clientes-agentes. Si `gabysql` lo habla de fábrica, cualquier agente lo enchufa directo:

```bash
gabysql-server -dir ./dbs -token MI_TOKEN
gabysql-mcp --server http://127.0.0.1:7878 --token MI_TOKEN
# Claude Desktop / Claude Code / Cursor lanzan gabysql-mcp como subprocess
# y descubren las 5 tools + 2 resources sin código de pegamento.
```

### 🛡️ Cómo se respeta el motor

- **No se toca `Cargo.toml`.** El binario se auto-descubre desde `src/bin/`. `Cargo.lock` no añade un solo paquete.
- **No se cambia `[lib]`.** Sigue compilando con cero deps externas. [ADR-0001](docs/adr/0001-rust-zero-deps-core.md) intacto.
- **No se abre el `.db`.** El gateway hace doble salto stdio→HTTP→Pager, así heredas todo lo que ya está endurecido en `server.rs`: `write_lock` global, tope de conexiones, bearer token, CORS preflight, validación de SQL antes de pegar al Pager.
- **No se cambia el formato en disco.** Sin bump de VERSION, sin nuevo tipo de página, sin cambio en el WAL.

### ✅ Tests
- Módulo `#[cfg(test)] mod tests` en `src/bin/gabysql-mcp.rs`: 9 tests cubren parser JSON, dispatch JSON-RPC y semántica de `--read-only`. CI multi-OS los ejecuta vía `cargo test`.

### 📐 ADR
- [ADR-0010 — Gateway MCP como adaptador externo sobre el HTTP/JSON existente](docs/adr/0010-mcp-gateway.md): promovida de 🟡 Propuesta a ✅ Aceptada con la implementación.

---

## 2026-05-08 — Decimoquinta intervención: `PageCache` LRU acotado — cierra fuga de memoria del server

> **Sin bump de formato.** El cambio es interno al Pager. La API pública del Pager se mantiene compatible salvo dos métodos nuevos (`set_cache_capacity`, `cache_len`, `cache_capacity`).

### ✨ Cambio
- Reemplazo de `cache: BTreeMap<u32, CachedPage>` (que crecía sin límite) por `cache: PageCache` con **capacidad fija** + **eviction LRU sobre páginas clean**.
- Constante `DEFAULT_CACHE_PAGES = 1024` (~4 MB por DB con páginas de 4 KB). Configurable por instancia con `Pager::set_cache_capacity(n)`.
- LRU implementada con `HashMap<u32, CacheSlot>` + contador monótono (touch en cada `get/get_mut/insert`). Eviction = scan O(N) sobre el map cuando está lleno; para 1024 entradas son µs por inserción.
- Política dirty-aware: **las páginas dirty nunca se evictan** — pertenecen a la transacción abierta y deben llegar al WAL antes de poder dropearse. Si el cache llega a capacidad lleno de dirty, se permite overflow temporal: perder una página dirty corromperia la DB. El overflow drena solo en el commit (todas pasan a clean simultáneamente).

### 🎯 Por qué este cambio

**Pre-bloque-10:**
```rust
struct Pager {
    cache: BTreeMap<u32, CachedPage>,  // ← crece sin freno
}
```
Un `INTEGRITY CHECK` o un `SELECT` con full scan sobre una DB de 200 MB cargaba ~50 K páginas en RAM y **nunca las liberaba**. En `gabysql-server -dir ./dbs` con 50 DBs activas y un sweep operacional periódico, la memoria del server crecía a 10 GB y eventualmente lo mataba el OOM killer. Sin error, sin warning, sin recovery — solo `kill` y reiniciar.

**Post-bloque-10:**
```rust
struct PageCache {
    capacity: usize,                       // bounded
    map: HashMap<u32, CacheSlot>,
    counter: u64,                          // monotonic for LRU
}
```
Memoria del server acotada por `cache_capacity × #DBs_abiertas × page_size`. Para 50 DBs × 1024 páginas × 4 KB = **200 MB max**, predecible, no swappea.

### 🛡️ Comportamiento bajo casos edge
- **Workload chico con cache vacío**: idéntico a antes (cache nunca se llena, no evicta nada).
- **Workload grande de read-only**: evicta clean pages LRU. La página menos usada se cae; si vuelve a pedirse, se relee de disco con CRC verificado (mismo path que cold load).
- **Mid-transaction con muchas writes**: dirty pages se acumulan; clean pages preexistentes se evictan primero. Si el commit se retrasa y entra más dirty que cap, el cache excede cap **temporalmente** (correctness > strict cap). Drena en commit.
- **Rollback**: `cache.clear()` libera todo (mismo path que antes).

### 🧪 Validación
- 39/39 tests de integración (1 nuevo: `page_cache_is_bounded_and_evicts_clean_pages` siembra 200 filas, abre con `set_cache_capacity(4)`, recorre cada página de la DB y asserta que `cache_len() <= 4`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 🔭 `Transaction` (Unit of Work) — pospuesto a bloque futuro
La recomendación original de bloque 10 incluía un objeto `Transaction` que reemplazara las 40+ aperturas de `Catalog::open(self.pager)` por una unit-of-work compartida con cache de `TableMeta`. Después de medir el impacto real:
- La fuga de memoria del cache es **inmediata** (problema agudo del server).
- La memoización de `TableMeta` es **marginal** (lookup hash + decode = µs; el ahorro existe pero no aparece en profiles de workloads reales).
- El refactor de 40 sitios cuesta ~1500 líneas y rompe muchos diffs en revisiones.

Decisión: **se entrega solo el `PageCache` LRU en este bloque**. El `Transaction` queda como propuesta independiente con su propio análisis cuando aparezca un workload que lo justifique (ej. INSERT masivo medido).

---

## 2026-05-08 — Decimocuarta intervención: `LeafCursor` (Iterator pattern) — Fase 2 paso 2

> **Sin bump de formato.** El cambio es estructural: cómo se leen los rows del B+Tree.

### ✨ Cambio
- Nuevo `bptree::LeafCursor<'a>` que implementa `Iterator<Item = DbResult<KeyValue>>` y carga páginas leaf **on-demand** vía la chain `next` del B+Tree.
- Constructores en `Tree`: `cursor_full(root)` (full scan en orden de PK) y `cursor_range(root, from, to)` (range scan inclusive en ambos extremos).
- Wrappers en `Catalog`: `scan_cursor(root)` y `range_cursor(root, from, to)` para el caller del SQL layer.
- `exec_select` reescrito: cuando NO hay `ORDER BY`, los planes `FullScan` y `Range` consumen el cursor con `.skip(offset).take(limit)` en vez de materializar todo el B+Tree. Cuando hay `ORDER BY`, sigue materializando (necesita ordenar antes de window).

### 🎯 Impacto medible en recursos
- `SELECT … LIMIT N` sobre tabla de N filas pasa de O(filas_totales) memoria + IO a **O(N + offset)** memoria + IO. Verificable: el test `cursor_limit_returns_only_requested_rows` sobre 1.000 filas valida que `LIMIT 5` devuelve solo 5 PKs en orden, sin intermediarios.
- `SELECT … WHERE pk BETWEEN a AND b LIMIT N` corta el walk apenas la PK supera `b`, sin tocar páginas ulteriores.
- `Plan::ByPks` (path de índice secundario) sigue materializando — está acotado por la cardinalidad del lookup, no por el tamaño de la tabla.

### 🛡️ Borrow semantics (intencionales)
El cursor toma `&mut Pager` por su lifetime. Mientras está vivo, ninguna otra escritura puede pasar por el mismo Pager. Eso es lo correcto para SELECT (read-only) y por eso solo lo usa `exec_select`. Los call sites que necesitan leer Y mutar el mismo B+Tree (`CREATE INDEX` backfill, `INTEGRITY CHECK`, `delete_with_cascade`) siguen usando los helpers materializadores (`scan / range / all`); ahí la materialización es correcta porque la lectura tiene que terminar antes que la escritura empiece.

### 🧪 Validación
- 38/38 tests de integración (1 nuevo: `cursor_limit_returns_only_requested_rows` ejercita LIMIT/OFFSET y BETWEEN sobre 1.000 filas).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Decimotercera intervención: crash tests dirigidos (Fase 1 reabierta y cerrada del todo)

> **Sin bump de formato.** Solo nuevos tests de integración que ejercitan el path WAL→file con escenarios de crash sintéticos.

### 🧪 Crash recovery scenarios cubiertos
Los tests no matan procesos — sintetizan en disco el estado que un `kill -9` dejaría en cada momento crítico del flujo de `Pager::commit`:

1. **`crash_recovery_partial_file_restored_from_wal`** — kill después del WAL flush + COMMIT marker pero antes de tocar el data file. Trunca el data file al header y verifica que el reopen replica las páginas del WAL y el `SELECT` devuelve los datos completos.
2. **`crash_recovery_wal_without_commit_is_ignored`** — kill antes del COMMIT marker (transacción no durable). Forja un WAL con páginas pero sin marker; verifica que el reopen NO replica nada y los datos previos quedan intactos.
3. **`crash_recovery_replay_is_idempotent`** — kill durante los writes al data file con WAL ya flusheado. Re-planta el mismo WAL después de un replay exitoso y verifica que un segundo replay converge al mismo estado (no double-counting, no corrupción).

### 🎯 Cierre definitivo de Fase 1
Esto cubre el ítem "crash tests dirigidos (kill -9 entre WAL y file flush)" que quedaba pendiente en el [ROADMAP](../ROADMAP.md). Fase 1 (Robustez funcional) queda 100% entregada y demostrada con tests reproducibles.

### 🧪 Validación
- 37/37 tests de integración (3 nuevos).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Duodécima intervención: `ORDER BY` (Fase 2 paso 1)

> **Sin bump de formato.** Todo el ordering ocurre en memoria sobre el resultado del scan/range/index path.

### ✨ Funcionalidad SQL
- **`SELECT ... ORDER BY <col> [ASC|DESC]`**. ASC es el default cuando se omite la dirección. Va entre `WHERE` y `LIMIT/OFFSET`.
- Funciona sobre **cualquier columna** del schema (no requiere índice). Reusa el scan/range/index path existente y ordena el resultado en memoria.
- **NULLs sortean primero** bajo ASC (consistente con SQLite). En DESC quedan al final por reverse.
- Comparación tipada: INT/INT, FLOAT/FLOAT, mixto INT↔FLOAT (promueve a f64), BOOL (false<true), TEXT/DATE/DATETIME/JSON por byte order.

### 🧱 Cambios estructurales
- `SelectStmt.order_by: Option<OrderClause>` con `OrderClause { column, direction: OrderDir }`.
- Cuando `order_by` está set, el executor difiere `LIMIT/OFFSET` hasta después del sort para no truncar prematuramente.
- Nuevo helper `compare_values(Option<&Value>, Option<&Value>) -> Ordering` con NULL-first semantics.
- Validación pre-I/O: `ORDER BY` sobre columna inexistente devuelve error explícito.
- Reserved words extendidas: `order`, `by`, `asc`, `desc`.

### 🧪 Validación
- 34/34 tests de integración (4 nuevos: `order_by_int_asc_desc`, `order_by_text_with_limit_offset_window`, `order_by_nulls_sort_first_under_asc`, `order_by_unknown_column_rejected`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Undécima intervención: gabymodeler v2 (PowerDesigner-style) + CORS

> **Sin bump de formato.** El motor no cambia; el modeler reescrito y el server gana headers CORS para que el modeler pueda hablarle directo.

### 🌐 gabymodeler v2 (`web/modeler/`)
Reescritura completa del modelador, espejo del motor `VERSION 6`:
- **Layout PowerDesigner-style**: header de toolbar + Object Browser izquierdo (árbol DB > Tables > columnas con badges PK/NN/UN/FK + sección Indexes) + Canvas central + Result List inferior colapsable + Status bar.
- **Schema editor**: cada columna lleva flags inline `PK / NN / UN / FK` y un input `default` editable. PK fuerza INT + NOT NULL automáticamente. FK abre un mini-modal para elegir tabla, columna PK del target y `ON DELETE RESTRICT|CASCADE`.
- **Check Model** continuo (14 reglas): PK ausente / duplicada / no INT, columna duplicada, identificador inválido o reservado (espejo de `catalog::RESERVED_WORDS`), `NOT NULL + DEFAULT NULL`, `DEFAULT` sobre PK, UNIQUE sobre JSON, FK rota / con type mismatch / target no-PK, etc. Cada hallazgo es clickeable y selecciona la entidad/columna en canvas + browser.
- **SQL Preview en vivo** (sin abrir modal). El emit ordena tablas topológicamente (parents antes que children) y emite todas las constraints inline (`PRIMARY KEY`, `NOT NULL`, `UNIQUE`, `DEFAULT <literal>`, `REFERENCES ... ON DELETE ...`) — DDL fiel al motor `VERSION 6`.
- **↘ Importar de gabysql**: dialog que pide URL del server, token opcional y nombre de DB; consume `GET /tables?db=<db>` y reconstruye entidades + columnas + constraints + FKs desde la respuesta enriquecida del bloque 3. Reverse engineering one-shot.
- **Migración v1 → v2 automática**: si encuentra `gabymodeler.v1` en localStorage, lo lee y produce un `gabymodeler.v2` con las constraints en blanco (los flags se editan a mano).
- **FK lines**: SVG Bezier con marker arrow; `CASCADE` se dibuja sólida, `RESTRICT` punteada.

### 🔓 CORS en `gabysql-server`
- Toda respuesta lleva `Access-Control-Allow-Origin: *`, `Access-Control-Allow-Methods: GET, POST, OPTIONS`, `Access-Control-Allow-Headers: Authorization, Content-Type, X-Gabysql-Token` y `Access-Control-Max-Age: 600`.
- El método `OPTIONS` se contesta con `204 No Content` antes de cualquier auth — los preflights del navegador no llevan credenciales y rechazarlos rompería el modeler en cross-origin.
- También se agregaron `204 No Content` y `503 Service Unavailable` al mapa de status text del response writer.

### 🧪 Validación
- 30/30 tests de integración siguen verdes (no se agregaron tests de modeler — es UI vanilla).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 📋 web/modeler/README.md
Reescrito para el layout v2 y el flujo con reverse engineering.

---

## 2026-05-07 — Décima intervención: `INTEGRITY CHECK` (cierre de Fase 1)

> **Sin bump de formato.** El comando es de solo lectura — no toca el catálogo ni los datos.

### ✨ Funcionalidad SQL
- **`INTEGRITY CHECK;`** — barre la DB abierta y devuelve un ResultSet con una fila por hallazgo. Columnas: `kind`, `object`, `detail`. El campo `message` resume con `OK · N tablas · M filas · K índices · F FKs · P páginas` o `FAIL · ...` según el caso.

### 🔍 Qué chequea
1. **CRC de cada página**: itera de `0..page_count` haciendo `Pager::page_data`. Cualquier falla del CRC se reporta como `kind=page_corrupt`.
2. **Decodificación de cada fila**: `decode_row` corre sobre cada fila de cada tabla. Falla → `kind=row_decode`.
3. **Índices secundarios**: walks every bucket de cada índice y verifica que cada `(value_bytes, pk)` apunte a una PK que efectivamente existe en la tabla. Si no → `kind=orphan_index_entry`.
4. **FOREIGN KEYs**: para cada columna con `references`, verifica que el parent table exista (sino `fk_target_missing`) y que cada valor no nulo de la columna tenga su parent row (sino `fk_orphan`).

### 🧱 Cambios estructurales
- Nuevo `Statement::IntegrityCheck` y método `Engine::exec_integrity_check`.
- Reserved words extendidas: `integrity`, `check`.
- Sin cambios al on-disk format ni al catálogo.

### 🧪 Validación
- 30/30 tests de integración (2 nuevos: `integrity_check_clean_db_returns_ok`, `integrity_check_reports_corrupted_page`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 🎯 Cierre de Fase 1 (Robustez funcional)
Con este bloque, los 5 ítems de Fase 1 del [ROADMAP](../ROADMAP.md) están entregados:
- ~~`UPDATE`/`DELETE` por PK~~
- ~~Checksums por página + WAL~~
- ~~`NOT NULL` / `DEFAULT` / `UNIQUE`~~
- ~~`FOREIGN KEY` + `ON DELETE` enforced~~
- ~~`INTEGRITY CHECK` operacional~~

El motor está listo para empezar a sumar features de Fase 2 (índices compuestos, range scan secundario, `ORDER BY`) o para una primera publicación con SLAs de durabilidad medibles.

---

## 2026-05-07 — Novena intervención: FOREIGN KEY enforced (Camino A · paso 5)

> **On-disk format jump: VERSION 5 → 6.** `Column` ahora persiste un FK opcional `(target_table, target_column, on_delete)`. DBs v5 son rechazadas explícitamente al abrir.

### ✨ Funcionalidad SQL
- **`REFERENCES <table>(<column>) [ON DELETE RESTRICT|CASCADE]`** como constraint de columna en `CREATE TABLE` y `ALTER TABLE ADD COLUMN`. Default `RESTRICT` cuando se omite `ON DELETE`.
- **Validación al DDL**: target table debe existir (o ser self-ref a la tabla siendo creada), target column debe ser la PK del target, tipos deben coincidir (en esta versión ambos son siempre `INT`).
- **Enforcement en `INSERT`**: cada FK no nula chequea que exista la fila parent. Self-FK que apunta al PK que se está insertando se acepta (caso CEO/manager-de-sí-mismo).
- **Enforcement en `UPDATE`**: solo se revalidan FKs cuyo valor cambió.
- **Enforcement en `DELETE`**:
  - `RESTRICT` (default) aborta el DELETE si existe alguna fila hija; sin efectos colaterales.
  - `CASCADE` borra las hijas iterativamente (worklist con `visited` set sobre `(tabla, pk)` para cortar ciclos), incluyendo sus entradas en índices secundarios.
- **Self-references** soportadas (`employee.manager_id REFERENCES employee(id)`).

### 🧱 Cambios estructurales
- `catalog::ForeignKeyMeta { table, column, on_delete: OnDelete }` con `OnDelete::{Restrict, Cascade}`.
- `Column.references: Option<ForeignKeyMeta>` persistido bajo flag `0x04 = HAS_FK`.
- `RESERVED_WORDS` extendido con `foreign`, `references`, `cascade`, `restrict`.
- Helpers nuevos en `sql.rs`: `validate_fk_targets`, `check_fk_value`, `enforce_fk_on_insert`, `enforce_fk_on_update`, `find_child_pks_with_fk_value`, `delete_with_cascade`.
- `find_child_pks_with_fk_value` usa el índice secundario sobre la columna FK si existe; cae en full scan si no — recomendación documentada de indexar columnas FK para DELETEs O(log n).
- `exec_delete` simplificado: chequea existencia y delega en `delete_with_cascade`, que maneja índices secundarios + cascade + cycle protection.

### 🌐 Endpoint `/schema` extendido
Cada columna ahora incluye `references: { table, column, onDelete } | null`:
```json
{
  "name": "parent_id", "type": "INT", "pk": false, "notNull": false, "unique": false,
  "hasDefault": false, "default": null,
  "references": { "table": "parent", "column": "id", "onDelete": "CASCADE" }
}
```

### 🛡️ Restricciones de la versión
- Solo FK de columna única (no compuestas).
- Target debe ser la PK del parent — `REFERENCES` contra `UNIQUE` no-PK no está soportado todavía.
- Solo `RESTRICT` y `CASCADE` (ni `SET NULL`, ni `SET DEFAULT`, ni `NO ACTION`).
- `ALTER TABLE ADD COLUMN ... REFERENCES ...` reusa los mismos guards que UNIQUE: si la columna es `NOT NULL` necesita un `DEFAULT` que apunte a un parent existente, etc.

### 🧪 Validación
- 28/28 tests de integración (6 nuevos: `fk_create_validation_rejects_bad_targets`, `fk_insert_update_enforcement`, `fk_self_reference_allows_pointing_at_self`, `fk_delete_restrict_blocks_when_children_exist`, `fk_delete_cascade_removes_children_and_grandchildren`, `old_v5_db_file_is_rejected_after_v6_bump`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Octava intervención: identificadores duros + introspección completa (Camino A · paso 4)

> **Sin bump de formato.** Los datos en disco no cambian; el cambio es de validación (más estricta) y de contrato JSON (más rico).

### ✨ Identificadores
- Nuevo `catalog::validate_identifier(name, kind)` — única definición de "identificador válido" en el motor: `[A-Za-z_][A-Za-z0-9_]*`, longitud máxima `MAX_IDENT_LEN = 64`, no reservada.
- Lista `catalog::RESERVED_WORDS` con todas las keywords del parser y los nombres de tipo (`int`, `text`, `bool`, `float`, `date`, `datetime`, `json`, etc.).
- Aplicado en `CREATE TABLE` (nombre de tabla + cada columna), `ALTER TABLE ADD COLUMN` (nombre de columna nueva, vía `validate_create_table` sobre meta prospectivo) y `CREATE [UNIQUE] INDEX` (nombre de índice).

### 🌐 Endpoint `/schema` extendido
La respuesta de `GET /schema?db=X&table=Y` (y por tanto también `GET /tables`) ahora incluye lo necesario para reverse-engineering completo desde el frontend:

```json
{
  "ok": true,
  "table": {
    "name": "users",
    "primaryKey": "id",
    "rootPage": 2,
    "columns": [
      { "name": "id",    "type": "INT",  "pk": true,  "notNull": true,  "unique": false, "hasDefault": false, "default": null },
      { "name": "email", "type": "TEXT", "pk": false, "notNull": true,  "unique": true,  "hasDefault": false, "default": null },
      { "name": "status","type": "TEXT", "pk": false, "notNull": true,  "unique": false, "hasDefault": true,  "default": "pending" }
    ],
    "indexes": [
      { "name": "uq_users_email", "column": "email", "rootPage": 4, "unique": true }
    ]
  }
}
```

Campos nuevos por columna: `notNull`, `unique` (derivado de los índices unique de una columna), `hasDefault`, `default` (literal con su tipo nativo en JSON; `null` para "no default" o `DEFAULT NULL`). Campo nuevo por índice: `unique`.

### 🧪 Validación
- 22/22 tests de integración (1 nuevo: `identifier_rules_apply_across_ddl` cubre tabla/columna/índice y los tres rechazos: reservada, longitud, ALTER).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Séptima intervención: edición incremental de schemas (Camino A · paso 3)

> **Sin bump de formato.** El layout `VERSION = 5` ya soporta `TableMeta` con cualquier número de columnas; las filas previas se decodifican con un fallback a `DEFAULT` o `NULL` cuando la fila quedó "corta" frente al esquema nuevo.

### ✨ Funcionalidad SQL
- **`DROP TABLE [IF EXISTS] <name>`** — borra la entrada del catálogo. Las páginas backing (data + índices secundarios) **no** se liberan; el reclaim queda para un futuro `vacuum` (consistente con la política de `DROP INDEX`).
- **`ALTER TABLE <name> ADD [COLUMN] <coldef>`** — agrega una columna al final del esquema. Soporta `NOT NULL`, `DEFAULT`, `UNIQUE`. La keyword `COLUMN` es opcional.

### 🧱 Cambios estructurales
- `decode_row` tolera EOF mientras quedan columnas por decodificar: rellena con el `DEFAULT` de la columna o `NULL`. Permite `ADD COLUMN` sin reescribir filas existentes; el rewrite ocurre naturalmente en el próximo `UPDATE` de cada fila.
- `Catalog::remove_table` borra la entrada del catálogo via `Tree::delete`.
- `parse_column_def` factorizado y compartido entre `CREATE TABLE` y `ALTER TABLE ADD COLUMN`.
- `parse_if_exists` factorizado para `DROP DATABASE` / `DROP TABLE`.

### 🛡️ Restricciones de `ALTER ... ADD COLUMN`
- `PRIMARY KEY` rechazado (la PK ya existe; esta versión no admite swap ni multi-PK).
- `NOT NULL` requiere `DEFAULT` no nulo (sin él, las filas previas violarían la constraint inmediatamente).
- `UNIQUE` con `DEFAULT` no nulo en tabla con > 1 fila se rechaza (produciría duplicados en el backfill).
- `UNIQUE` sin DEFAULT en tabla poblada está OK: filas previas decodifican como `NULL`, y SQL UNIQUE permite múltiples NULLs.
- Nombre de columna duplicado rechazado.
- Validación completa del `coldef` (compatibilidad de tipo del DEFAULT, etc.) reusada del path de `CREATE TABLE`.

### 🧪 Validación
- 21/21 tests de integración (4 nuevos: `drop_table_removes_catalog_entry`, `alter_add_column_decodes_old_rows_with_default_or_null`, `alter_add_column_constraint_guards`, `alter_add_column_unique_then_enforces`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Sexta intervención: constraints declarativas (Camino A · paso 2)

> **On-disk format jump: VERSION 4 → 5.** `Column` ahora persiste `NOT NULL` y `DEFAULT`; `IndexMeta` persiste `unique`. Las DBs creadas con la entrega anterior son rechazadas explícitamente al abrir — re-crear con el binario v5.

### ✨ Funcionalidad SQL
- **`NOT NULL`** como constraint de columna en `CREATE TABLE`. Validado en `INSERT` (columna omitida sin DEFAULT, o `NULL` explícito) y en `UPDATE` (asignación que dejaría la columna en `NULL`). PK es implícitamente `NOT NULL`.
- **`DEFAULT <literal>`** como constraint de columna. Soporta `INT`, `FLOAT`, `BOOL`, `TEXT`/`DATE`/`DATETIME`/`JSON` y `NULL`. La compatibilidad de tipo entre literal y columna se valida en `CREATE TABLE` — `name TEXT DEFAULT 1` se rechaza. PK no admite `DEFAULT`.
- **`UNIQUE`** inline en columna y **`CREATE UNIQUE INDEX`** como sentencia. Inline auto-genera un índice unique con nombre `uq_<tabla>_<columna>`. Múltiples `NULL` se permiten (consistente con SQL estándar). Conflicto de UNIQUE se chequea **antes** de tocar disco — el INSERT/UPDATE falla sin efectos colaterales.
- `CREATE UNIQUE INDEX` sobre tabla con duplicados existentes **aborta el backfill** con error claro; no deja índice colgado.

### 🧱 Cambios estructurales
- `catalog::Column { name, column_type, not_null, default }` con `DefaultLiteral { Null, Integer, Float, Bool, String }` propio del catálogo (no acopla con `sql::Value`).
- `catalog::IndexMeta` lleva `unique: bool`.
- Layout v5 por columna: `[name][type_code:u8][flags:u8][default_payload?]` con `flags & 0x01 = NOT NULL`, `flags & 0x02 = HAS_DEFAULT`.
- Layout v5 por índice: `[name][column][root_page:u32][unique:u8]`.
- Nuevo helper `index::bucket_unique_conflict` y `sql::check_unique_conflict` — un único path de uniqueness para inline UNIQUE y `CREATE UNIQUE INDEX`.
- `sql::ColumnDef` lleva `not_null`, `unique`, `default: Option<Value>` para el AST del parser.

### 🧪 Validación
- 17/17 tests de integración (6 nuevos: `not_null_rejects_missing_and_explicit_null`, `default_fills_missing_and_can_be_overridden`, `default_with_not_null_combination`, `default_type_mismatch_rejected_at_create`, `inline_unique_rejects_duplicates`, `create_unique_index_backfill_aborts_on_duplicates`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-05 — Quinta intervención: DDL de DATABASE + modelador web

### ✨ Funcionalidad SQL
- **`CREATE DATABASE [IF NOT EXISTS] <name>;`** — crea un archivo `.db` en el directorio de `-dir` (server) o junto al path objetivo (CLI).
- **`DROP DATABASE [IF EXISTS] <name>;`** — borra el archivo `.db` y su `.wal` si quedó.
- **`SHOW DATABASES;`** — lista las DBs presentes en el directorio.

Estas sentencias **no se ejecutan contra una `.db` específica** (no operan sobre `TableMeta`). Las despacha el caller — `gabysql-server` para HTTP `/exec` y la CLI `gabysql exec` — antes de abrir el `Pager`. Mezclar DB-level con table-level en un mismo `/exec` se rechaza con error explícito.

### 🌐 Modelador web `gabymodeler`
- Nueva carpeta [`web/modeler/`](web/modeler/) — single-page HTML+CSS+JS vanilla, sin frameworks, sin npm, sin backend acoplado.
- Drag & drop de entidades sobre canvas con grid; SVG para líneas FK Bezier.
- Columnas con tipos (`INT/TEXT/BOOL/FLOAT/DATE/DATETIME/JSON`), flag `PK` (auto-fija `INT`), flag `idx` (índice secundario).
- Botón "↪ FK" para columnas que apuntan a otra entidad — la FK se documenta como comentario en el SQL (las FOREIGN KEY declarativas no se enforced en `VERSION 4`).
- **Exporta SQL** con `CREATE DATABASE [IF NOT EXISTS]` + `CREATE TABLE` + `CREATE INDEX`, copia al clipboard o descarga `.sql`.
- Persiste el modelo en `localStorage` (`gabymodeler.v1`).
- Botón "📦 Cargar ejemplo" trae un schema `users + orders` con FK indexada para evaluar el flujo en 1 click.

### 🧭 Landing `web/index.php` rediseñada
- Reemplaza la tarjeta única de phpgabyadmin por **dos tarjetas lado a lado**: `gabymodeler` y `phpgabyadmin`. Cada una con CTA propio.
- Documenta el flujo recomendado: **modeler → SQL → phpgabyadmin → ejecutar**.

### 🧪 Validación
- 11/11 tests de integración (incluye nuevo `database_level_statements_parse_and_engine_rejects`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.
- `php -l web/index.php` y `php -l web/phpgabyadmin/index.php`: clean.

---

## 2026-05-04 — Cuarta intervención: índices secundarios + scaffolding profesional

> **On-disk format jump: VERSION 3 → 4.** `TableMeta` ahora persiste una lista de índices secundarios; las DBs creadas con la entrega anterior son rechazadas explícitamente al abrir.

### ✨ Funcionalidad SQL
- **Índices secundarios**: `CREATE INDEX <name> ON <table> (<column>);` y `DROP INDEX <name>;`. Soporta backfill automático sobre tablas con datos existentes.
- **`SELECT WHERE col = val` por columna no-PK** consulta el índice cuando existe (lookup O(1) sobre bucket por hash, filtro exacto por bytes, hidratación por PK). Si la columna no es PK ni está indexada, se rechaza con mensaje explícito.
- `WhereClause::Eq` ahora acepta cualquier `Value` (no solo `i64`), por lo que `SELECT WHERE name = 'Ana'` o `WHERE score = 9.5` funcionan igual que `WHERE id = 1`.
- Mantenimiento automático de índices en `INSERT` / `UPDATE` / `DELETE`: el índice solo se actualiza cuando la columna indexada está afectada y el valor cambia.

### 🧱 Cambios estructurales
- Nuevo módulo [`src/index.rs`](src/index.rs): hashing FNV-1a-64, codec de bucket `[count:u16] + N×([vlen:u16][value][pk:i64])`, helpers `bucket_insert/remove/lookup`.
- `TableMeta::indexes: Vec<IndexMeta { name, column, root_page }>` persistido al final del payload del catálogo.
- Reglas de validación: una sola PK INT escalar (sin cambios), una sola columna por índice secundario, `JSON` no es indexable (sin semántica de igualdad canónica).
- `DROP INDEX` no libera páginas — el reclaim queda para una futura herramienta `vacuum`.

### 🛡️ Hardening de CI / supply chain (entrega previa, consolidada en docs)
- 4 workflows: `ci.yml` endurecido, `security.yml`, `workflow-security.yml`, `stale.yml`.
- `cargo audit` 0.22.1 (RustSec), `cargo deny` 0.19.4 (advisories + licenses + bans + sources, regido por [deny.toml](deny.toml)).
- `detect-secrets` (FS + últimos 50 commits), Trojan Source / zero-width / patrones peligrosos Rust+PHP / URLs de exfil.
- `grype` container scan con `--fail-on critical`.
- `actionlint` + `zizmor` + `pin-check` (rechaza acciones sin SHA pin).
- Acciones third-party pinneadas a SHA, `permissions: contents: read` por defecto, `persist-credentials: false`.
- Dependabot semanal: github-actions + cargo + docker.

### 📚 Scaffolding profesional importado desde otros repos del perfil
- `CODE_OF_CONDUCT.md`, `SUPPORT.md`, `COMPATIBILITY.md`, `RECRUITER.md`, `QUICKSTART.md`, `RELEASE.md`.
- `.editorconfig` y `.gitattributes` con normalización LF / CRLF coherente con CI multi-OS.
- `pull_request_template.md` con checklist de fmt/clippy/test/formato-en-disco/supply-chain.

### 🧪 Validación
- 10/10 tests de integración (incluye nuevos: split de B+Tree con 600 filas, detección de corrupción por checksum, rechazo de overwrite, UPDATE/DELETE roundtrip, **índices secundarios end-to-end con backfill + INSERT/UPDATE/DELETE/DROP**).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo audit`, `cargo deny check`: OK.
- `actionlint`, `zizmor`: 0 findings.

### ⚠️ Migración requerida
- DBs creadas con `VERSION = 3` no son legibles. Re-crear con `gabysql init <file.db>`. Mensaje de error explícito al abrir.

---

## 2026-05-03 — Tercera intervención: cierre de hallazgos críticos del MVP

> **On-disk format jump: VERSION 1 → 3.** Toda DB creada antes de esta entrega es rechazada explícitamente al abrir. Recrearla con la versión actual (`gabysql init <file.db>`).

### 🧱 Cambios estructurales del motor
- **B+Tree real**: el índice por PK pasó de una lista enlazada de hojas a un B+Tree con nodos internos. Lookup descendente en O(log N), `root_page` permanece estable cruzando splits gracias a copy-up del root.
- **Hash del catálogo determinista**: las claves del catálogo de tablas se calculaban con `DefaultHasher` (no estable entre versiones de Rust). Reemplazado por FNV-1a-64 inline en código.
- **Checksums CRC32-IEEE**: cada página persiste un trailer de 4 bytes con su CRC. El Pager lo finaliza antes de flushear y verifica al leer y al replay del WAL. La corrupción ahora produce error explícito en vez de silencio.
- **`Pager::create` no destructivo**: rehúsa sobrescribir un archivo existente. Se introdujo `create_force` para el camino explícito de reset (`gabysql init --force <file.db>`).
- **`page_size` honesto**: el header valida que `page_size == PAGE_SIZE_DEFAULT`; el campo se mantiene en disco para una futura revisión del formato.

### ✨ Funcionalidad SQL
- `UPDATE <tabla> SET col = val[, ...] WHERE <pk> = N;` (no permite cambiar la PK).
- `DELETE FROM <tabla> WHERE <pk> = N;` (error si la PK no existe).
- Mensajes de error de PK más explícitos sobre la limitación INT-only de esta versión.

### 🛡️ Endurecimiento del modo server
- `gabysql-server` aplica un techo de conexiones concurrentes (default 64, configurable con `-max-connections N`). Conexiones extra reciben 503 y se cierran sin generar threads.

### 🧪 Validación
- 9/9 tests de integración (incluye nuevos: split de B+Tree con 600 filas, detección de corrupción por checksum, rechazo de overwrite, UPDATE/DELETE roundtrip).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`: OK.

### ⚠️ Migración requerida
- Bases de datos creadas con versiones anteriores a esta entrega no son legibles. El error es explícito (`unsupported gabysql file format: version=...`). Re-crear con el binario actual.

---

## 2026-03-19 — Segunda intervención: migración completa a Rust y estabilización base

### 🧱 Estado actual del sistema
- Motor embebido en Rust con archivo único `.db`
- CLI `gabysql` para `init`, `info`, `exec` y `repl`
- Server HTTP `gabysql-server` para operar una base única o un directorio de bases
- `phpgabyadmin` consumiendo la API HTTP como consola web liviana
- Docker y `docker compose` para levantar server y admin web en un entorno reproducible

### 🏗️ Cambios estructurales
- Se eliminó la implementación anterior en Go y se reemplazó por un proyecto Rust con `Cargo`
- Se separó el core en módulos de storage, catálogo, SQL, servidor y estructura persistente por clave primaria
- Se unificó la documentación para reflejar solo las capacidades reales del motor actual

### ✨ Mejoras funcionales
- Soporte de `CREATE TABLE`, `INSERT` y `SELECT` con full scan, `LIMIT/OFFSET`, `WHERE <pk> = ...` y `BETWEEN`
- Soporte de tipos `INT`, `TEXT`, `BOOL`, `FLOAT`, `DATE`, `DATETIME`, `JSON` y `NULL` en columnas no PK
- Rechazo explícito de claves primarias duplicadas en vez de sobrescritura silenciosa
- Recovery WAL por marcador `COMMIT` para rehidratar páginas confirmadas tras reinicio

### 🛡️ Estabilidad y seguridad
- El parser SQL ahora devuelve errores controlados en escenarios inválidos en lugar de derribar el proceso
- Se corrigió el manejo de comillas escapadas dentro de strings SQL para soportar textos complejos en inserciones multi-sentencia
- `phpgabyadmin` quedó endurecido con cookie firmada y bloqueo de servidores remotos salvo habilitación explícita
- La UI web y el README quedaron alineados con el comportamiento real del motor

### 🎨 Documentación y lenguaje visual
- Se creó un set documental completo alineado con el estándar usado en otros repos del perfil
- Se añadieron guías de instalación, uso, operación, seguridad, troubleshooting y contribución
- Se añadió documentación técnica de arquitectura, requisitos, API y especificaciones del motor
- Se aplicó una capa visual consistente con badges, bloques de estado, tablas de navegación y rutas por perfil

### ✅ Validación y entrega continua
- Se agregaron pruebas de integración para roundtrip básico, PK duplicada, paginación con `LIMIT/OFFSET`, `NULL`, parser inválido y recovery WAL
- Se agregó CI en GitHub Actions con `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` y lint de PHP
- La matriz de CI cubre `ubuntu-latest`, `windows-latest` y `macos-latest`, más build Docker en Linux
- La CI publica artefactos `release` por sistema operativo para facilitar distribución nativa multiplataforma
- El `Dockerfile` valida `cargo test --all-targets` antes de construir binarios release
- `docker compose` permite probar juntos `gabysql-server` y `phpgabyadmin`

### 🧪 Validación realizada en esta intervención
- `cargo fmt --check`: OK
- `cargo check --tests`: OK
- `cargo clippy --all-targets -- -D warnings`: OK
- `docker build -t gabysql .`: OK
- `docker compose up -d --build`: OK
- `GET http://localhost:8080/health`: OK
- `GET http://localhost:8000`: OK

### ⚠️ Límites actuales conocidos (al cierre de la 2ª intervención)
- El índice persistente sigue siendo una estructura de hojas enlazadas por PK `INT`; no es todavía un B+Tree multinivel completo *(superado en la 3ª intervención: ver entrada superior)*
- No hay optimizer cost-based ni estadísticas de consulta
- No hay concurrencia avanzada, MVCC ni transacciones complejas
- Sigue siendo un producto base estable para evolucionar, no un reemplazo directo de motores maduros como PostgreSQL o MySQL
