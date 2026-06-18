# 📋 Estado actual del producto

> **Snapshot técnico — qué funciona hoy, qué está pendiente y por qué subsistema.** Última verificación: 2026-06-15 contra `main` tras la **sesión maratón** (ya 15 pushes consecutivos en el mismo día sobre Fase 3 cerrada). Highlights consolidados: **R2** calibró `INDEX_BREAKEVEN` con bench data real (0.20 → 0.10) · **R3** instrumentó `P5D_SWAP_THRESHOLD` con env var · **R7/R9/R10** pulieron EXPLAIN + COUNT DISTINCT sobre JOIN + USING/NATURAL · **ANSI fix** alineó `UPDATE/DELETE WHERE pk no-existe` con PostgreSQL/SQLite · **M3** + **Pager proptest** = 2 redes property-based defienden planner cost-based y storage layer · **M4 fuzz parser** = 503.8M queries random / 1h limpia / 0 panics (línea README citable) · **M6** `EXPLAIN ANALYZE` clasifica bias del estimator (GOOD/MILD/HIGH) · **M12** `SAVEPOINT` ANSI SQL:2003 · **M13** cross-request tx HTTP — ORMs pueden mantener tx via session header. **VERSION on-disk = 33** (último bump P4 / 2026-06-10; los 15 bloques de 2026-06-15 son **todos zero-bump**).
>
> 👉 **Para el inventario exhaustivo del SQL no-soportado** (comandos faltantes uno por uno, con prioridades y bloques de implementación): [MISSING_COMMANDS.md](MISSING_COMMANDS.md).
>
> 👉 **Para el balance crítico de la última sesión** (tensiones cerradas, abiertas, deuda residual): [ANALISIS_POST_P5.md](ANALISIS_POST_P5.md).
>
> 👉 **Si te perdés con las letras (P5b, R6, M2, …)**: [COMO_TRABAJAMOS.md](COMO_TRABAJAMOS.md) explica de dónde salen los prefijos, cómo nace y se entrega cada bloque, y dónde mirar para qué.

[![Versión](https://img.shields.io/badge/versi%C3%B3n-0.1.x--MVP-7c5cff)](../CHANGELOG.md)
[![Formato en disco](https://img.shields.io/badge/on--disk%20VERSION-33-2d7a66)](TECHNICAL_SPECS.md)
[![Tests integraci%C3%B3n](https://img.shields.io/badge/tests-828%2F828-brightgreen)](../tests/integration_test.rs)
[![Fuzz parser](https://img.shields.io/badge/fuzz%20parser-503.8M%20iters%20%2F%201h%20%2F%200%20panics-blue)](fuzz/FUZZ-RUN-2026-06-15.md)
[![Camino comercial](https://img.shields.io/badge/path-A%20%E2%80%94%20embebido%20nicho-informational)](COMMERCIAL_ROADMAP.md)

> **2026-05-29 — Día grande: cierre de Fase 2 (seguridad) + arranque y avance fuerte de Fase 3 (planeación).** Lo entregado hoy en orden cronológico:
> - **Z3f** — DEFAULTs aplicados antes de WITH CHECK en INSERT (RLS sin falso-positivo). 🟢
> - **P1** — `EXPLAIN <stmt>` (plan textual, dry-run; clasifica scan type honestamente). 🟢
> - **P2** — `EXPLAIN ANALYZE <stmt>` (ejecuta el inner + `Instant` wall-clock + row count real; side-effects PERSISTEN). 🟢
> - **P3** — `ANALYZE TABLE foo` + EXPLAIN anota `[est.rows=N]` (stats session-scoped, sin persistencia). 🟢
>
> El stack de bloques previos sigue vigente:
> - **CTEs**: `WITH name AS (...)` (W1) y `WITH RECURSIVE` (W2, fixpoint con guard 10K). 🟢
> - **Window functions**: `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD`, `FIRST_VALUE`, `LAST_VALUE`, `SUM/AVG/MIN/MAX/COUNT OVER` (W3). 🟢 *(sin frame explícito, `[GBY-4088]`)*.
> - **Triggers `BEFORE`/`AFTER` con body multi-statement** (X1+X2). 🟢
> - **Stored procedures + `CALL`** (X3). 🟢
> - **User-defined scalar functions `CREATE FUNCTION ... RETURNS ... AS <expr|BEGIN..END>`** (X3b+X4f). 🟢 *(invocables en SELECT/WHERE, RETURN con sentinel)*.
> - **Control de flujo procedural completo**: `IF/THEN/ELSIF/ELSE/END IF`, `DECLARE/SET/WHILE/EXIT [WHEN]`, `RAISE EXCEPTION/NOTICE`, `FOR i IN a..b LOOP`, `BEGIN..EXCEPTION WHEN .. END`, `LOOP..END LOOP`, `CASE..WHEN..END CASE`, `EXCEPTION WHEN <code>` (X4 → X4f). 🟢 *(diferidos: `FOR row IN SELECT`, filtros simbólicos, formato `%` en RAISE)*.
> - **Tipos extendidos**: aliases (`BIGINT`, `VARCHAR(n)`, `DECIMAL(p,s)`, `DOUBLE PRECISION`, `BOOLEAN`, `TIMESTAMP`, …) + nuevos `TIME` y `UUID` (Y). 🟢
> - **Enforcement de `VARCHAR(n)`/`CHAR(n)`** (Y2): `[GBY-4119]` si excede. 🟢 *(diferidos: range SMALLINT/TINYINT, BLOB, DECIMAL exacto, ARRAY, ENUM)*.

---

## 🧱 Madurez por subsistema

Leyenda: 🟢 producción-ready en su scope · 🟡 funcional con limitaciones · 🟠 parcial · 🔴 no implementado

| Subsistema | Estado | Comentario | Archivo |
| :--- | :---: | :--- | :--- |
| Pager (header + caché in-memory) | 🟢 | `PageCache` con cap fija + LRU clean-only (default 1024 páginas ≈ 4 MB; tunable con `set_cache_capacity`). Prefetch pendiente. | [src/storage.rs](../src/storage.rs) |
| WAL after-image + replay | 🟢 | CRC32 verificado por record. Sin checkpoints. | [src/storage.rs](../src/storage.rs) |
| CRC32 por página | 🟢 | IEEE polynomial, table-based; verifica en lectura y replay. | [src/storage.rs](../src/storage.rs) |
| Formato en disco versionado | 🟢 | `VERSION = 33` (P4/2026-06-10: stats por-columna). Bumps acumulados desde V: 14 (X3 procedures), 15 (X3 payload), 16 (X3b functions), 17 (Y TIME/UUID), 18 (Y2 max_length), 19 (Y3 int_width), 20 (Y4 BLOB), 21 (Y5 UNSIGNED), 22 (Y6 DECIMAL exacto), 23 (Z1 User/Role), 24 (Z2 Grant), 25 (Z3 Policy USING), 26 (Z1b PBKDF2 KDF metadata), 27 (Z3b PolicyMeta with_check_sql), 28 (Z1c scrypt scheme=2), 29 (Z1d Blake2b H'), 30 (Z1e Argon2id scheme=3), 31 (Z1f corte), 32 (P3b TableStats), 33 (P4 ColumnStats). Rechazo explícito de versiones anteriores. P1/P2/P3 son zero-bump; P3b/P4 bumpan por nuevo `ObjectKind` / nuevo payload. | [TECHNICAL_SPECS.md](TECHNICAL_SPECS.md) |
| `Pager::create` no destructivo | 🟢 | Refuses overwrite; `create_force` explícito. | [src/storage.rs](../src/storage.rs) |
| B+Tree (LEAF + INTERNAL) | 🟡 | Splits OK; falta merge / rebalance al borrar. | [src/bptree.rs](../src/bptree.rs) |
| Catálogo persistente | 🟢 | FNV-1a-64, estable entre versiones de Rust. | [src/catalog.rs](../src/catalog.rs) |
| Tipos de columna | 🟢 | **INT** + width enforcement TINYINT(1)/SMALLINT(2)/MEDIUMINT(3)/INT4(4)/BIGINT (Y3) + **UNSIGNED** (Y5, high bit en int_width), **TEXT/VARCHAR(n)/CHAR(n)** con max_length enforcement bytes UTF-8 (Y2), **BOOL**, **FLOAT** (alias del antiguo, sin precisión exacta), **DECIMAL(p,s)/NUMERIC(p,s)** EXACTO i128+scale con aritmética Add/Sub/Mul/Div/Mod exact-precision y SUM/AVG exact (Y6+Y7+Y8+Y9), **BLOB/BYTEA/BINARY** con literal `X'hex'` (Y4), **DATE/DATETIME/TIMESTAMP/TIME** ISO-8601, **UUID** 8-4-4-4-12, **JSON** texto no indexable. ARRAY/ENUM/INTERVAL diferidos a Y10. | [src/catalog.rs](../src/catalog.rs), [src/sql.rs](../src/sql.rs) |
| Constraints declarativas (NOT NULL/UNIQUE/DEFAULT) | 🟢 | Inline en `CREATE TABLE`; `CREATE UNIQUE INDEX`; pre-check sin efectos colaterales. | [src/sql.rs](../src/sql.rs), [src/catalog.rs](../src/catalog.rs) |
| `FOREIGN KEY` declarativas + enforced | 🟢 | Single-column y multi-column (`FOREIGN KEY (a, b) REFERENCES p (x, y)`, residual #3/VERSION 12, lookup O(log n) via fingerprint K2). Target = PK del parent. Acciones referenciales completas en `ON DELETE` y `ON UPDATE`: `RESTRICT` / `CASCADE` / `SET NULL` / `SET DEFAULT` / `NO ACTION` (L1/VERSION 9 + residual #4/ON UPDATE activo). Self-ref OK, cycle protection en cascade. | [src/sql.rs](../src/sql.rs), [src/catalog.rs](../src/catalog.rs) |
| `CHECK (expr)` constraints | 🟢 | L2 (VERSION 10) column-level y table-level con eval 3VL ANSI en INSERT/UPDATE/UPSERT/cascade. Persistencia como texto canónico (`format_expr`/`parse_expr_str`). L3 agregó `ALTER TABLE ADD [CONSTRAINT name] CHECK (expr)` con re-validación O(n). Subqueries dentro de CHECK → `[GBY-4069]`. | [src/sql.rs](../src/sql.rs), [src/catalog.rs](../src/catalog.rs), [docs/adr/0021-check-constraints.md](adr/0021-check-constraints.md) |
| Nombres explícitos de constraint + `ALTER TABLE DROP CONSTRAINT` | 🟢 | Residual #2 (VERSION 11). `CONSTRAINT <name>` opcional en PK / UNIQUE / FK / CHECK. `ALTER TABLE <t> DROP CONSTRAINT [IF EXISTS] <name>` resuelve CHECK / UNIQUE / FK (PK rechazada con `[GBY-4072]`). | [src/sql.rs](../src/sql.rs), [docs/adr/0022-named-constraints-and-drop.md](adr/0022-named-constraints-and-drop.md) |
| Vistas lógicas (`CREATE VIEW` / `DROP VIEW`) | 🟢 | Bloque V (VERSION 13). `CREATE VIEW [IF NOT EXISTS] v [(col_aliases)] AS SELECT ...` y `DROP VIEW [IF EXISTS]`. Read-only (`[GBY-4075]`); source debe ser SELECT simple (`[GBY-4078]`); cycle guard `MAX_VIEW_DEPTH=32` (`[GBY-4076]`); namespace compartido tabla/vista (`[GBY-4077]`). Catalog gana discriminator byte por record. | [src/sql.rs](../src/sql.rs), [src/catalog.rs](../src/catalog.rs), [docs/adr/0025-views.md](adr/0025-views.md) |
| Índices secundarios (equality) | 🟢 | Una columna, backfill, mantenimiento INSERT/UPDATE/DELETE. | [src/index.rs](../src/index.rs) |
| Índices compuestos | 🟢 | K2 (2026-05-26, VERSION 8) + **K3 (2026-05-30)**. `CREATE [UNIQUE] INDEX idx ON t (a, b, ...)` — equality lookup vía fingerprint FNV-1a-64. K3 lifteó el restricción de all-INT: hoy acepta INT/FLOAT/BOOL/TEXT/DATE/DATETIME/TIME/UUID NOT NULL (rechaza JSON/BLOB/DECIMAL con `[GBY-4067]`). No range scan (fingerprint no order-preserving). | [src/index.rs](../src/index.rs), [src/sql.rs](../src/sql.rs), [docs/adr/0019-composite-pk-and-index.md](adr/0019-composite-pk-and-index.md), [docs/adr/0066-bench-exposed-gaps.md](adr/0066-bench-exposed-gaps.md) |
| PRIMARY KEY compuesta | 🟢 | K2 (2026-05-26, VERSION 8) + **K4 (2026-05-30)**. `PRIMARY KEY (a, b, ...)` table-level all-INT NOT NULL (`[GBY-4064]`). UPDATE bloqueado sobre cualquier columna PK (`[GBY-4008]`). Partial lookup (`WHERE pk1 = ?`) usa auto-index `_pk_prefix_<table>` OrderedInt desde K4 — equivalente al left-most column match de MySQL InnoDB. | [src/catalog.rs](../src/catalog.rs), [src/sql.rs](../src/sql.rs), [docs/adr/0019-composite-pk-and-index.md](adr/0019-composite-pk-and-index.md), [docs/adr/0066-bench-exposed-gaps.md](adr/0066-bench-exposed-gaps.md) |
| `WHERE col_indexada = val` (no PK) | 🟢 | Plan dispatch: PK vs índice vs error. | [src/sql.rs](../src/sql.rs) |
| `WHERE BETWEEN` | 🟢 | Fast-path por PK simple (Range plan) o índice OrderedInt; cualquier otro caso cae a FullScan + post-filter desde **F3 (2026-05-30)** — mismo path que `=`/`>`/`<`. Antes rebotaba `[GBY-4002]`. | [src/sql.rs](../src/sql.rs), [docs/adr/0066-bench-exposed-gaps.md](adr/0066-bench-exposed-gaps.md) |
| Range scan por índice secundario | 🟡 | Solo columnas **INT**: el índice usa el valor como clave del B+Tree (ADR-0017, VERSION 7), `WHERE col_idx BETWEEN a AND b` walk en O(log N + k). TEXT/FLOAT/BOOL/DATE/DATETIME indexados siguen equality-only. | [src/index.rs](../src/index.rs), [src/sql.rs](../src/sql.rs) |
| `ORDER BY` | 🟢 | Cualquier columna, `ASC`/`DESC`, NULLs first. Sort en memoria post-scan. | [src/sql.rs](../src/sql.rs) |
| `JOIN` (INNER/CROSS/LEFT/RIGHT/FULL/USING/NATURAL + index-loop) | 🟢 | Ver entradas separadas más abajo. | [src/sql.rs](../src/sql.rs) |
| `GROUP BY`/`HAVING`/agregados (`COUNT`/`SUM`/`AVG`/`MIN`/`MAX`/`DISTINCT`/`COUNT(DISTINCT)`) | 🟢 | Bloque F (2026-05-25, single-table) + **F2 (2026-05-30, sobre `SELECT con JOIN` + `COUNT(*) FROM <view>`)** con 3VL ANSI. SUM/AVG sobre `DECIMAL` son Decimal-exact (Y9). Único residual: `COUNT(DISTINCT col)` sobre JOIN sigue `[GBY-4028]`. | [src/sql.rs](../src/sql.rs), [docs/adr/0066-bench-exposed-gaps.md](adr/0066-bench-exposed-gaps.md) |
| CTEs (`WITH` no-recursivo + `WITH RECURSIVE`) | 🟢 | W1 (2026-05-29) + W2 (2026-05-29). `WITH name [(cols)] AS (SELECT ...) [, ...]` y `WITH RECURSIVE name AS (anchor UNION [ALL] step)` con fixpoint guard `MAX_RECURSIVE_ITER=10_000` y dedup FNV-1a-64. Sin bump on-disk. | [src/sql.rs](../src/sql.rs), [docs/adr/0026-cte-non-recursive.md](adr/0026-cte-non-recursive.md), [docs/adr/0027-with-recursive.md](adr/0027-with-recursive.md) |
| Window functions (`OVER (PARTITION BY ... ORDER BY ...)`) | 🟢 | W3 (2026-05-29) + **W4 (2026-05-30, O(n) per partition)**. `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD`, `FIRST_VALUE`, `LAST_VALUE`, `SUM/AVG/MIN/MAX/COUNT OVER`. W4 reescribió la fase per-partition (`fill_window_partition_into`) a un solo walk (prefix sums + adjacent compare); antes era O(n²) — el bench medía 27-50s para 500 filas. **Sin frame explícito** (`[GBY-4088]`) sigue diferido — solo default RANGE UNBOUNDED PRECEDING. Sin bump on-disk. | [src/sql.rs](../src/sql.rs), [docs/adr/0028-window-functions.md](adr/0028-window-functions.md), [docs/adr/0066-bench-exposed-gaps.md](adr/0066-bench-exposed-gaps.md) |
| Triggers `BEFORE`/`AFTER {INSERT\|UPDATE\|DELETE}` con body multi-statement | 🟢 | X1 (AFTER single-stmt) + X2 (BEFORE + body multi-stmt `BEGIN..END`). Guard `MAX_TRIGGER_DEPTH=16`. `NEW.col`/`OLD.col` resueltos via substitución de tokens. VERSION 13→14. | [src/sql.rs](../src/sql.rs), [docs/adr/0029-triggers-after-x1.md](adr/0029-triggers-after-x1.md), [docs/adr/0030-triggers-before-multistmt-x2.md](adr/0030-triggers-before-multistmt-x2.md) |
| Stored procedures + `CALL` | 🟢 | X3 (2026-05-29, VERSION 14→15 → 15→16 payload). `CREATE PROCEDURE name(params) AS BEGIN ... END` con body multi-stmt, `CALL name(args)`, `DROP PROCEDURE`. | [src/sql.rs](../src/sql.rs), [docs/adr/0031-stored-procedures-x3.md](adr/0031-stored-procedures-x3.md) |
| User-defined scalar functions invocables en SELECT/WHERE | 🟢 | X3b (2026-05-29, VERSION 15→16). `CREATE FUNCTION name(params) RETURNS type AS <expr\|BEGIN..END>`, `DROP FUNCTION`. RETURN con sentinel (X4f) en multi-stmt body. | [src/sql.rs](../src/sql.rs), [docs/adr/0032-user-functions-x3b.md](adr/0032-user-functions-x3b.md) |
| PL/pgSQL completo (`IF/CASE/WHILE/FOR/LOOP/DECLARE/SET/RAISE/EXCEPTION/RETURN/BLOCK`) | 🟢 | X4→X4f + X6 (2026-05-29). `IF/THEN/ELSIF/ELSE/END IF`, `CASE..WHEN..END CASE`, `DECLARE`, `SET`, `WHILE LOOP`, `EXIT [WHEN]`, `FOR i IN a..b LOOP`, `FOR row IN (SELECT ...) LOOP` (X6), `RAISE EXCEPTION/NOTICE`, `BEGIN..EXCEPTION WHEN <code>..END`, `LOOP..END LOOP`, `RETURN expr`. Guard `MAX_LOOP_ITERATIONS=100_000`. | [src/sql.rs](../src/sql.rs) |
| Funciones escalares + operadores aritméticos + concat `\|\|` + postfix Expr (`LENGTH`, `UPPER`, `LOWER`, `SUBSTR`, `CONCAT`, `TRIM`/`LTRIM`/`RTRIM`, `REPLACE`, `SPLIT_PART`, `ABS`, `ROUND`, `CEIL`/`FLOOR`, `MOD`, `POWER`/`SQRT`, `NOW`, `CURRENT_DATE`, `CURRENT_TIMESTAMP`, `DATE_ADD`/`DATE_SUB`, `DATEDIFF`, `EXTRACT`, `STRFTIME`, `COALESCE`, `NULLIF`, `IFNULL`, `IF`, `CAST`, `CASE`) | 🟢 | Bloques G1+G2+G3 (2026-05-26): disponibles en SELECT list **y** en `WHERE` / `HAVING` / `UPDATE SET` / `DELETE WHERE` / `ON CONFLICT DO UPDATE SET`. NULL propagation + 3VL en CASE searched, en el WHERE, en aritméticos y en postfix. G3 sumó `+`/`-`/`*`/`/`/`%`, concat `\|\|`, postfix `IS [NOT] NULL`/`[NOT] LIKE`/`[NOT] IN`/`[NOT] BETWEEN` sobre cualquier `Expr`, y las funciones P2/P3 que quedaban abiertas. Errores claros: overflow (`[GBY-4042]`), división por cero (`[GBY-4043]`), tipo aritmético inválido (`[GBY-4044]`), dominio matemático (`[GBY-4045]`), fecha mal formateada (`[GBY-4046]`), campo EXTRACT inválido (`[GBY-4047]`). Sub-pendientes menores: `EXCLUDED.col` en UPSERT (J2-P2) y unary `-` prefix sobre expresión. | [src/sql.rs](../src/sql.rs) |
| Subqueries `WHERE col IN (SELECT …)` no-correlacionadas | 🟢 | Single-column. Outer requiere PK o índice secundario. Subquery se ejecuta una vez y se materializa. | [src/sql.rs](../src/sql.rs) |
| Subqueries escalares `WHERE col = (SELECT …)` no-correlacionadas | 🟢 | 1 columna × ≤1 fila. 0 filas o NULL → match vacío (ANSI). más de 1 fila → `[GBY-4014]`. Reusa lookup PK/índice. | [src/sql.rs](../src/sql.rs) |
| Subqueries `WHERE [NOT] EXISTS (SELECT …)` | 🟢 | No-correlacionada: pre-ejecuta. Correlacionada single-eq: post-filter per-row con `outer_stack`. Desde H (2026-05-26) también dentro de `AND`/`OR`/`NOT`. | [src/sql.rs](../src/sql.rs) |
| `WHERE col NOT IN (SELECT …)` | 🟢 | H (2026-05-26). ANSI 3VL estricta: NULL en la subquery propaga NULL al resultado (`5 NOT IN (1, NULL)` → NULL). | [src/sql.rs](../src/sql.rs) |
| Derived tables `FROM (SELECT …) AS sub` | 🟢 | H (2026-05-26). Alias obligatorio (`[GBY-4048]`). Permitido en FROM o RHS de JOIN. Inferencia de tipo por columna; mezcla → TEXT. Sin índices (full scan); sin UPDATE/DELETE/INSERT sobre derived. | [src/sql.rs](../src/sql.rs) |
| Subquery escalar en SELECT list `SELECT (SELECT MAX(x) FROM t) FROM s` | 🟢 | H (2026-05-26). Correlated OK vía outer_stack. `Expr::ScalarSubquery` evaluada con `Engine::eval_expr_full` (fast-path puro `eval_expr` cuando el árbol no la contiene). | [src/sql.rs](../src/sql.rs) |
| Set operations (`UNION`/`UNION ALL`/`INTERSECT`/`INTERSECT ALL`/`EXCEPT`/`EXCEPT ALL`/`MINUS`) | 🟢 | I (2026-05-26). Precedencia ANSI (INTERSECT > UNION/EXCEPT, asoc-izquierda). ORDER BY / LIMIT / OFFSET al nivel del resultado combinado. Headers heredados del LHS. Validación de arity (`[GBY-4054]`) y tipos compatibles (`[GBY-4055]`, INT/FLOAT promueven). Multisets con counts: ALL preserva, sin ALL dedup. | [src/sql.rs](../src/sql.rs) |
| `VALUES (...), (...)` como query o tabla virtual | 🟢 | I (2026-05-26). Standalone (`VALUES (1,'a'),(2,'b');` → ResultSet con headers `column1`,`column2`,...) y en FROM/JOIN (`FROM (VALUES (...),...) AS t(c1,c2,...)` con alias de tabla y de columnas obligatorios, `[GBY-4052]`/`[GBY-4053]`). Cada fila se evalúa como `Expr` (admite expresiones constantes). | [src/sql.rs](../src/sql.rs) |
| `CREATE TABLE AS SELECT` (CTAS) | 🟢 | K1 (2026-05-26). Fuente = cualquier `SelectQuery` (SELECT, set ops, VALUES). `IF NOT EXISTS` y lista opcional `(col_aliases)`. Primera columna del SELECT debe ser INT no-NULL — se usa como PK de la nueva tabla (`[GBY-4058]` si no lo es). Sin DEFAULT/UNIQUE/FK heredadas del origen. | [src/sql.rs](../src/sql.rs) |
| `RENAME TABLE` / `ALTER TABLE ... RENAME TO` | 🟢 | K1 (2026-05-26). Renombra entry del catálogo (remove + put con la nueva clave hash) y arrastra el cambio a las FKs entrantes (otras tablas que apuntaban al nombre viejo). Destino tomado → `[GBY-4062]`; origen ausente → `[GBY-2001]`. | [src/sql.rs](../src/sql.rs) |
| `ALTER TABLE ... DROP COLUMN [IF EXISTS]` | 🟢 | K1 (2026-05-26). Rewrite in place de cada fila (decode + remove col + re-encode). Bloqueos: PK (`[GBY-4059]`), columna indexada (`[GBY-4060]`, sugiere `DROP INDEX`), FK saliente o entrante (`[GBY-4061]`). `IF EXISTS` → no-op si la columna ya no está. | [src/sql.rs](../src/sql.rs) |
| `ALTER TABLE ... RENAME COLUMN` | 🟢 | K1 (2026-05-26). On-disk row es posicional → no requiere rewrite; sólo muta `TableMeta.columns[i].name` y arrastra el cambio a `primary_key`, índices y FKs entrantes que referencien la columna. Destino tomado → `[GBY-4062]`. | [src/sql.rs](../src/sql.rs) |
| DDL pendiente (partial indexes, `ALTER COLUMN TYPE`, ALTER PK) | 🟡 | K2/2026-05-26 cerró PK compuesta e índices compuestos (VERSION 8). Residual #3/2026-05-27 cerró FK multi-col (VERSION 12). Resto (partial indexes, `ALTER COLUMN TYPE`, ALTER PK) sigue pendiente. | [docs/adr/0019-composite-pk-and-index.md](adr/0019-composite-pk-and-index.md) |
| Subqueries `ALL`/`ANY`/`SOME` / correlated `=` puro fuera de EXISTS / `LATERAL` | 🔴 | Deferred a un bloque "M" futuro (manejo extendido de subqueries). CTE y window functions ya están entregados (ver entradas separadas arriba). | — |
| `INNER JOIN ... ON l = r`, `CROSS JOIN`, comma-syntax, aliases, multi-tabla, self-join | 🟢 | Nested-loop puro O(N×M×…). WHERE/ORDER BY trabajan sobre filas joineadas. `SELECT *` expande prefijado. | [src/sql.rs](../src/sql.rs) |
| `LEFT [OUTER] JOIN`, `RIGHT [OUTER] JOIN`, `FULL [OUTER] JOIN` con NULL-fill | 🟢 | Implementado vía tracking de matched-rows + NULL-fill por kind. `OUTER` opcional. Combinable en chains. | [src/sql.rs](../src/sql.rs) |
| `JOIN ... USING (col)`, `NATURAL JOIN` | 🟢 | USING soporta 1 columna; NATURAL exige exactamente 1 columna común (>1 → `[GBY-4023]`). `SELECT *` omite la columna fusionada del right (ANSI). | [src/sql.rs](../src/sql.rs) |
| Index-loop join (optimización del nested-loop) | 🟢 | Cuando el `ON` o el USING/NATURAL derivado apunta contra PK o columna indexada del right Y el kind es INNER/LEFT, se hace lookup dirigido en vez de scan completo. O(N1 × log N2) vs O(N1 × N2). RIGHT/FULL siguen nested-loop (no requieren cambios de comportamiento). | [src/sql.rs](../src/sql.rs) |
| Parser SQL | 🟡 | DDL: CREATE/DROP TABLE (constraints completos), CTAS K1, ALTER TABLE ADD/DROP COLUMN/CHECK/CONSTRAINT, RENAME TABLE / COLUMN, CREATE/DROP VIEW (V), CREATE/DROP INDEX (single + composite K2), CREATE/DROP DATABASE, SHOW DATABASES, INTEGRITY CHECK, TRUNCATE. DML: INSERT (multi-row + ON CONFLICT + REPLACE + RETURNING), SELECT (set ops, derived, scalar subquery, EXISTS correlated, JOINs ANSI completos), UPDATE, DELETE. **W**: WITH [RECURSIVE], window functions OVER. **X**: CREATE/DROP TRIGGER BEFORE/AFTER + body multi-stmt BEGIN..END, CREATE/DROP PROCEDURE + CALL, CREATE/DROP FUNCTION + RETURN, IF/CASE/WHILE/FOR/LOOP/DECLARE/SET/RAISE/EXCEPTION, BEGIN..EXCEPTION WHEN..END, FOR row IN (SELECT) LOOP. **Y**: tipos extendidos (DECIMAL(p,s), VARCHAR(n), TIME, UUID, BLOB, X'hex' literals, UNSIGNED). **Z**: CREATE/DROP USER/ROLE, ALTER USER SET PASSWORD, GRANT/REVOKE, SET SESSION AUTHORIZATION [WITH PASSWORD], CREATE/DROP POLICY USING+WITH CHECK. **P**: EXPLAIN [ANALYZE] <stmt>, ANALYZE [TABLE] <name>. **TCL**: BEGIN/START TRANSACTION/COMMIT/END/ROLLBACK. Sin prepared statements / bind params / SAVEPOINT. | [src/sql.rs](../src/sql.rs) |
| `CREATE/DROP DATABASE` + `SHOW DATABASES` | 🟢 | Despachados por server (`/exec`) y CLI antes de abrir Pager. En modo single-DB → 405. | [src/server.rs](../src/server.rs), [src/bin/gabysql.rs](../src/bin/gabysql.rs) |
| Engine (executor) | 🟡 | `LeafCursor` lazy para `SELECT … LIMIT N` sin ORDER BY (O(N+offset) IO) + prefetch one-leaf-ahead (ADR-0016) que warm-a la PageCache para la próxima leaf transition. Sin spill-to-disk para sort grande, sin plan lógico/físico explícito. | [src/sql.rs](../src/sql.rs), [src/bptree.rs](../src/bptree.rs) |
| Optimizer cost-based | 🟠 | P3 (2026-05-29) entregó `ANALYZE TABLE foo` con row_count session-scoped y EXPLAIN anota `[est.rows=N]` en cada SCAN. **No hay decisión de plan basada en costo todavía** — el planner sigue fijo. Defer P4 (NDV/MCV/histogramas) y P5 (reorden de joins por costo). | [src/sql.rs](../src/sql.rs), [docs/adr/0065-p3-analyze-stats.md](adr/0065-p3-analyze-stats.md) |
| `EXPLAIN` / `EXPLAIN ANALYZE` / `ANALYZE` | 🟢 | P1 (plan estimado, dry-run; clasifica scan type honestamente — PK lookup / hash-index / ordered-int / full scan + post-filter). P2 (ejecución real + `std::time::Instant` wall-clock + row count real + error captura como step `actual.error`; **side-effects PERSISTEN** en ANALYZE — usar EXPLAIN sin ANALYZE para dry-run). P3 (`ANALYZE [TABLE] <name>` colecta `row_count` session-scoped; EXPLAIN anota `[est.rows=N]` en cada SCAN). Sin bump on-disk en ninguno. | [src/sql.rs](../src/sql.rs), [docs/adr/0063-p1-explain-statement.md](adr/0063-p1-explain-statement.md), [docs/adr/0064-p2-explain-analyze.md](adr/0064-p2-explain-analyze.md), [docs/adr/0065-p3-analyze-stats.md](adr/0065-p3-analyze-stats.md) |
| Transacciones explícitas (`BEGIN`/`COMMIT`/`ROLLBACK`) | 🟢 | Bloque T (2026-05-25): batch-local, alias ANSI (`START TRANSACTION`/`END`) y MySQL (`WORK`) aceptados. **Pendiente**: `SAVEPOINT`/`ROLLBACK TO`, isolation levels, read-only y cross-request (HTTP session state). | [src/sql.rs](../src/sql.rs), [src/storage.rs](../src/storage.rs) |
| MVCC | 🔴 | Camino C. | — |
| Manejo de errores | 🟢 | Guía canónica + ~210 mensajes en español con contexto (qué/por qué/cómo). Ver [ERROR_HANDLING.md](ERROR_HANDLING.md). | [src/lib.rs](../src/lib.rs) |
| Concurrencia | 🟡 | Mutex global de proceso para escrituras. | [src/server.rs](../src/server.rs) |
| Lock cross-process sobre `.db` | 🟢 | `File::try_lock()` advisory exclusivo en `Pager::create/open`; falla rápido con error claro si otro proceso tiene la DB. Ver [ADR-0013](adr/0013-process-level-file-lock.md). | [src/storage.rs](../src/storage.rs) |
| `gabysql-server` HTTP/JSON | 🟢 | Token, multi-DB. Endpoints núcleo: `/health`, `/metrics`, `/dbs`, `/tables`, `/schema`, `/rows`, `/exec`. Sessions cross-request: `/tx/begin`, `/tx/commit`, `/tx/rollback` (M13). Listado del catálogo (sesión 2026-06-17/18, Pushes 7/10/14): `/views`, `/policies`, `/triggers`, `/procedures`, `/functions`, `/users`, `/roles`, `/grants`. `/users` filtra material secreto (password_hash, salt). Filtros `?table=` y `?grantee=/?object=` aplicados post-list. | [src/server.rs](../src/server.rs) |
| Observabilidad del server | 🟢 | Endpoint `/metrics` (uptime, counts por status, errors_total, p50/p95 latencia) + flag `-log-json` para logs JSONL por request. Ver [ADR-0014](adr/0014-logs-json-metrics.md). | [src/server.rs](../src/server.rs) |
| Cap de conexiones simultáneas | 🟢 | Default 64, configurable con `-max-connections`. | [src/server.rs](../src/server.rs) |
| TLS nativo en server | 🔴 | Reverse proxy en Camino A; nativo en Camino B. | — |
| Authz por usuario / rol (SQL-level) | 🟢 | Z1+Z2+Z3 (2026-05-29). USERS persistidos con KDF (scrypt RFC 7914 **default** scheme=2 — verificado vía `/users` E2E 2026-06-18; PBKDF2-SHA256 scheme=1 disponible; Argon2id scheme=3 estructural — ver hilo Z1g pendiente). ROLES. GRANT/REVOKE `{SELECT\|INSERT\|UPDATE\|DELETE\|TRUNCATE}` bitmask. SET SESSION AUTHORIZATION 'name' [WITH PASSWORD '...'] cambia identidad activa. RLS: `CREATE POLICY ... FOR {SELECT\|INSERT\|UPDATE\|DELETE\|ALL} [TO role[,...]] [USING (expr)] [WITH CHECK (expr)]`. Enforcement en exec_select/insert/update/delete/truncate + RETURNING filtrado (Z3d) + DEFAULTs antes de WITH CHECK (Z3f). Token HTTP sigue como capa de transporte. | [src/sql.rs](../src/sql.rs), [docs/adr/0050-z1-users-roles.md](adr/0050-z1-users-roles.md), [docs/adr/0051-z2-grant-revoke.md](adr/0051-z2-grant-revoke.md), [docs/adr/0052-z3-row-level-security.md](adr/0052-z3-row-level-security.md) |
| `phpgabyadmin` v2 | 🟢 | Single-file PHP con paleta GitHub-style, Inter+JetBrains Mono. Tabs: Browse (paginado + export CSV) · Structure (con índices/CHECK inline) · SQL editor con CodeMirror (Ctrl+Enter ejecuta) · Sessions (M13 cross-request tx) · Explain (M6 bias coloreado GOOD/MILD/HIGH) · Stats (KPIs + breakdown + dump `/metrics`) · Policies (CREATE/DROP guiados + listado) · Routines (triggers + procedures + functions) · Security (users + roles + grants con privilegios checkbox). CSRF + HMAC auth cookies preservados. | [web/phpgabyadmin/index.php](../web/phpgabyadmin/index.php) |
| `gabymodeler` v3 | 🟢 | Vanilla HTML+JS, paleta unificada con phpgabyadmin v2. Modela: entidades + FK Bezier + tipos Y1-Y9 (DECIMAL/BLOB/UUID/VARCHAR/INT widths/UNSIGNED) + CHECK inline + composite PK/UNIQUE/INDEX + Views + RLS Policies (USING+CHECK) + Triggers + Procedures + Functions + Users + Roles + Grants. Canvas con zoom-to-cursor (Ctrl+Wheel), pan (middle-click/Alt), fit-all, minimap navegable click-drag, atajos +/−/0/F. SQL emit topológicamente ordenado con todos los CREATE statements. **Tier 1/2 profesional** (2026-06-18, Pushes 26-31): Undo/Redo, Save/Load `.gby`, Export SVG/PNG, multi-selección + lasso + bulk align/distribute, drag-to-create FK, edición inline Tab/Enter, búsqueda global Ctrl+F, auto-layout force-directed, migrations diff schemas → ALTER. | [web/modeler/index.html](../web/modeler/index.html), [web/modeler/README.md](../web/modeler/README.md) |
| `gabymodeler` desktop (Tauri) | 🟢 | Empaquetado Windows `.msi` con `gabysql-server` como sidecar local en `127.0.0.1:18080`. Asociación `.gby`, menú nativo (5 submenús con atajos), workspace de DBs en `%APPDATA%`. Bundle 10-15 MB. Build CI con `git tag desktop-v*`. Tauri 1.6 + WebView2. | [desktop/gabymodeler/](../desktop/gabymodeler/), [docs/adr/0093-desktop-app-tauri.md](adr/0093-desktop-app-tauri.md) |
| Backup / restore con verificación | 🟢 | `gabysql backup/restore [--force] <src> <dst>` + `gabysql verify <db>`. Valida CRC32 por página en lectura **y** re-abre el destino post-escritura. Requiere DB cerrada (lock exclusivo de ADR-0013). Ver [ADR-0015](adr/0015-verified-backup-restore.md). | [src/backup.rs](../src/backup.rs) |
| `INTEGRITY CHECK` operacional | 🟢 | Pages CRC + row decode + index orphans + FK orphans. Devuelve ResultSet con kind/object/detail. | [src/sql.rs](../src/sql.rs) |
| Suite de benchmarks reproducible | 🟡 | `gabybench` (binario `src/bin/gabybench.rs`, separado del producto `gabybench` spec) puebla 3 DBs sintéticas (microblog 50k, events 200k, orders_lines 120k) y mide latencias con p50/p95/p99. Corrida 2026-05-29 vive en [BENCHMARK.md](../BENCHMARK.md). **Defer P6**: tracking de regresiones en CI, exports a JSON parseable, comparaciones cross-commit. | [src/bin/gabybench.rs](../src/bin/gabybench.rs), [BENCHMARK.md](../BENCHMARK.md), [GABYBENCH_SPEC.md](GABYBENCH_SPEC.md) |
| Replicación / HA / clustering | 🔴 | Camino C. | — |
| Wire protocol Postgres/MySQL | 🔴 | Camino C. | — |
| CI multi-OS (Ubuntu/Windows/macOS) | 🟢 | `cargo fmt + clippy + test + build release` por OS. | [.github/workflows/ci.yml](../.github/workflows/ci.yml) |
| `cargo audit` + `cargo deny` | 🟢 | RustSec advisories + licencias + bans + sources. | [.github/workflows/security.yml](../.github/workflows/security.yml) |
| `detect-secrets` (FS + 50 commits) | 🟢 | Baseline en `.secrets.baseline`. | [.github/workflows/security.yml](../.github/workflows/security.yml) |
| Trojan Source / zero-width / patrones peligrosos | 🟢 | grep dirigido sobre `*.rs`/`*.php`. | [.github/workflows/security.yml](../.github/workflows/security.yml) |
| `grype` container scan (only-fixed) | 🟢 | `.grype.yaml` con `only-fixed: true`. | [.grype.yaml](../.grype.yaml) |
| `actionlint` + `zizmor` + `pin-check` | 🟢 | Audita los propios workflows. | [.github/workflows/workflow-security.yml](../.github/workflows/workflow-security.yml) |
| Dependabot semanal (cargo + actions + docker) | 🟢 | — | [.github/dependabot.yml](../.github/dependabot.yml) |

---

## ✅ Verificación local de este snapshot

Cualquiera puede reproducir el estado anterior con:

```bash
# Tests
cargo test --all-targets

# Lint + format
cargo fmt --check
cargo clippy --all-targets -- -D warnings

# Supply chain
cargo install cargo-audit --version 0.22.1 --locked
cargo install cargo-deny  --version 0.19.4 --locked
cargo audit
cargo deny --all-features check

# Container scan
docker build -t gabysql-scan .
grype gabysql-scan -c .grype.yaml

# PHP lint
php -l web/index.php
php -l web/phpgabyadmin/index.php
```

CI corre todo lo anterior automáticamente en cada push a `main` y en cada PR. La rama no avanza si una sola línea falla.

---

## 🔭 Próximos bloques (post sesión 2026-05-29)

> **Fase 1 cerrada · Fase 2 cerrada (SQL relacional completo + constraints + vistas + CTEs + window functions + triggers + procedures + functions + PL/pgSQL + tipos extendidos completos + USERS/ROLES/GRANT/REVOKE + RLS) · Fase 3 arrancada (P1+P2+P3 sobre planeación textual).** Bloques cerrados acumulados:
>
> - **2026-05-25** (7 bloques): E1 / E2 / E3 / F / T / J / J2.
> - **2026-05-26** (7 bloques, VERSION 7→8): G1, G2, G3, H, I, K1, K2.
> - **2026-05-27** (5 pushes, VERSION 8→13): L1, L2, L3, Residual #2, #3, #4, Bloque V.
> - **2026-05-29** sesión 1 (W/X/Y, VERSION 13→22): W1, W2, W3, X1, X2, X3, X3b, X4, X4b, X4c, X4d, X4e, X4f, X6, Y1, Y2, Y3, Y4, Y5, Y6, Y7, Y8, Y9.
> - **2026-05-29** sesión 2 (Z security, VERSION 22→31): Z1, Z2, Z3, Z1b, Z3b, Z3c, Z1c, Z3d, Z1d, Z3e, Z1e, Z1f (Argon2id partial fix — corte semántico VERSION 30→31), Z3f.
> - **2026-05-29** sesión 3 (P planeación, sin bump on-disk): P1 (EXPLAIN), P2 (EXPLAIN ANALYZE), P3 (ANALYZE TABLE + stats en EXPLAIN).
> - **2026-06-09** sesión P3b (VERSION 31→32): stats persistidas en catálogo vía `ObjectKind::TableStats`. Sobreviven a reopen; DROP TABLE las borra. Ver [ADR-0067](adr/0067-p3b-persistent-stats.md).
> - **2026-06-10** sesión P4 (VERSION 32→33): stats por-columna persistidas — `null_count` exacto, NDV vía HyperLogLog (256 reg), MCV top-K=10, histograma equi-depth ~16 buckets. Aún NO consumidas por el planner (eso es P5). Ver [ADR-0068](adr/0068-p4-column-stats.md).
> - **2026-06-11** sesión P5b (zero-bump): composite secondary index lookup. `WHERE c1 = X AND c2 = Y AND ...` con CREATE INDEX (c1, c2, ...) → fingerprint FNV → bucket lookup en vez de FullScan. Cierra el último gap del bench (ADR-0066 Gap 10). Ver [ADR-0069](adr/0069-p5b-composite-index-lookup.md).
> - **2026-06-11** sesión P5a (zero-bump): infraestructura de estimación de selectividad. `estimate_selectivity(stats, expr)` consume MCV / NDV / histograma + reglas AND/OR/NOT/IS NULL. EXPLAIN gana `est.match=K`. Sin cambios al plan (P5a es solo annotation). Ver [ADR-0070](adr/0070-p5a-selectivity-estimation.md).
> - **2026-06-11** sesión P5c (zero-bump): cost-based fallback. Si stats indican `est.match/row_count ≥ 0.2`, fuerza `Plan::FullScan + post-filter` en vez de index lookup (N random reads serían más caros que scan secuencial). Conservador: sin stats, comportamiento inalterado. Primer bloque en que el plan **cambia** según stats. Ver [ADR-0071](adr/0071-p5c-cost-based-fallback.md).
> - **2026-06-11** sesión P5d (zero-bump): hash join build-side selection. Cuando `current` (acumulado de joins previos) supera 2× `right_rows`, swap el build side al lado más chico. Cardinalidad real (no estimada). Threshold conservador para preservar orden de output en queries sin ORDER BY. Ver [ADR-0072](adr/0072-p5d-hash-join-build-side.md).
> - **2026-06-11** sesión P5e (zero-bump): EXPLAIN anota el algoritmo real de JOIN (index-loop / hash / nested-loop) en vez de mentir con "nested-loop" fijo, más cardinality stats de ambos lados. Cierre formal de Fase 3 — EXPLAIN refleja ahora todo el planner. Ver [ADR-0073](adr/0073-p5e-join-algorithm-annotation.md).
> - **2026-06-11** sesión R1 (zero-bump): detección de stats stale. EXPLAIN muestra `stats.age=Xd Yh` (+`STALE` si >7d). P5c bow out cuando stats stale → preserva path indexado. Cierra la tensión #1 del análisis post-P5. Ver [ADR-0074](adr/0074-r1-stats-stale-detection.md).
> - **2026-06-11** sesión R4 (zero-bump, sin ADR): validación empírica de HLL+splitmix64 sobre TEXT (500 distinct), UUID (500), DATE (365) y DECIMAL (300). +4 tests. Todos los tipos dentro de ±25% del NDV real.
> - **2026-06-11** sesión M2 (zero-bump): nuevo modo `gabybench smoke` (microblog + orders_lines, ~1-2 min) y job CI `bench` que sube `bench/results.json` como artifact. Pre-requisito para R2/R3 (calibración de thresholds empíricos). Ver [ADR-0075](adr/0075-m2-gabybench-in-ci.md).
> - **2026-06-11** sesión R8 (zero-bump): composite-eq fast-path para UPDATE/DELETE. Cierra la asimetría P5b (que solo cubría SELECT). Bonus: composite PK fast-path para UPDATE/DELETE también. Suite total 794. Ver [ADR-0076](adr/0076-r8-update-delete-composite-fast-path.md).
> - **2026-06-11** sesión R6 (zero-bump): post-lookup bucket size check. Si el composite devuelve ≥20% de las filas, bail a FullScan + post-filter — usa cardinalidad REAL del bucket, no la estimación con asunción de independencia. Refina P5c en el caso de AND con cols correlacionadas. Suite total 798. Ver [ADR-0077](adr/0077-r6-composite-bucket-size-check.md).
> - **2026-06-15 — sesión maratón (10 pushes, zero-bump excepto VERSION sin cambios)**:
>   - **R7** ([ADR-0078](adr/0078-r7-p5c-reanalyze-hint.md)): EXPLAIN del path P5c skip sugiere re-ANALYZE si stats ∈ [24h, 7d).
>   - **R9** ([ADR-0079](adr/0079-r9-count-distinct-over-join.md)): COUNT(DISTINCT col) sobre JOIN ya soportado — cierra residual ADR-0066 Gap 1.
>   - **R10** ([ADR-0080](adr/0080-r10-using-natural-explain.md)): EXPLAIN reconoce PK/índice en USING/NATURAL JOIN (no solo ON explícito).
>   - **R2** ([ADR-0081](adr/0081-r2-index-breakeven-calibration.md)): primera constante del optimizer calibrada con números reales — `INDEX_BREAKEVEN` 0.20 → 0.10 + env var `GABYSQL_INDEX_BREAKEVEN`.
>   - **R3** ([ADR-0082](adr/0082-r3-p5d-swap-threshold-instrumentation.md)) + **R3-cont** ([ADR-0085](adr/0085-r3-cont-p5d-sweep-results.md)): `P5D_SWAP_THRESHOLD` instrumentado con env var; sweep empírico inconcluso → default 2.0 stays.
>   - **ANSI fix** ([ADR-0083](adr/0083-ansi-update-delete-no-row-zero.md)): `UPDATE/DELETE WHERE pk no-existe` ahora devuelve 0 filas (PostgreSQL/SQLite), no `[GBY-3006]`. Descubierto durante el debug del bench `all`.
>   - **M3** ([ADR-0084](adr/0084-m3-proptest-planner.md)): primera red de seguridad real del optimizer cost-based. Property tests hand-rolled zero-deps: 240 comparaciones automáticas por corrida verifican que P5c/P5d/R6 NUNCA cambian el resultado de un SELECT, solo el path. Habilita futuros bloques optimizer (M5/M6/M7/M9) sobre confianza empírica.
>   - **Bench fix** (commit 3c5d97c): warmup del bench era no-tolerante a errores; ahora best-effort. Suite total 798 → **813** (+15: 14 integration nuevos + 3 proptest nuevos − 2 ajustados por R2 + 2 ajustados por ANSI).
> - **2026-06-15 — segunda ola de la maratón (5 pushes adicionales)**:
>   - **fix(JOIN)** (commit 8f156b4): bug pre-existente destapado por R3-cont sobre CI. `Vec::with_capacity(current.len() * right_rows.len() / 2 + 1)` pre-reservaba 48 GB exactos (100k × 20k × 48 bytes/HashMap) → OOM en runner de 8 GB. Cambio a `Vec::with_capacity(current.len())`; el Vec crece amortizado O(1).
>   - **Pager proptest** ([ADR-0086](adr/0086-pager-proptest.md)): segunda capa de la red de seguridad property-based — 3 invariantes sobre `begin/insert/commit/rollback` (commit visibility, rollback discards, chained tx integrity) sobre secuencias random. ~5100 ops random por corrida.
>   - **M4 — fuzz parser** ([ADR-0087](adr/0087-m4-fuzz-parser.md) + [evidencia](fuzz/FUZZ-RUN-2026-06-15.md)): generador hand-rolled determinístico + `catch_unwind`. **1 hora limpia → 503.8M iters, 139k/s, 0 panics.** Línea de README "X horas de fuzz" satisfecha con evidencia citable.
>   - **M6** ([ADR-0088](adr/0088-m6-explain-analyze-bias.md)): `EXPLAIN ANALYZE` anota step `actual.bias` con ratio actual/est y clasificación GOOD/MILD/HIGH/MATCH. Diagnóstico directo del bias del estimator para queries scan-only.
>   - **M12** ([ADR-0089](adr/0089-m12-savepoints.md)): `SAVEPOINT name` / `ROLLBACK TO [SAVEPOINT]` / `RELEASE [SAVEPOINT]` (ANSI SQL:2003). Pager con full cache snapshot por savepoint. Desbloquea M13. Cierra el `[GBY-4029] "savepoints aún no soportados"`.
>   - **M13** ([ADR-0090](adr/0090-m13-cross-request-tx.md)): cross-request transactions HTTP. 3 endpoints nuevos (`/tx/begin`, `/tx/commit`, `/tx/rollback`) + `/exec` acepta `X-Gabysql-Session` header. ORMs (SQLAlchemy/Hibernate/Diesel) pueden mantener tx state a través de N requests. Backwards compatible: sin session = comportamiento clásico.
>   - **Suite final**: 813 → **828** (+15: 5 M12 + 4 M13 + 3 Pager proptest + 3 M6).
>
> **Pendientes priorizados** (orden recomendado, ver [ROADMAP.md](../ROADMAP.md#-próximas-proyecciones-orden-sugerido) sección "🔭 Próximas proyecciones" para el detalle):
>
> **Cierre inmediato de Fase 3**:
> - ~~**P4** — stats por-columna (NDV vía HyperLogLog, MCV top-K, histogramas equi-depth)~~ ✓ entregado 2026-06-10 (ADR-0068, VERSION 33).
> - ~~**P5b** — composite secondary index lookup~~ ✓ entregado 2026-06-11 (ADR-0069, zero-bump). Cierra ADR-0066 Gap 10.
> - ~~**P5a** — selectivity estimation (consumir stats P4, anotar EXPLAIN)~~ ✓ entregado 2026-06-11 (ADR-0070, zero-bump).
> - ~~**P5c** — cost-based index choice~~ ✓ entregado 2026-06-11 (ADR-0071, zero-bump) + R2 calibró umbral a 0.10 (ADR-0081).
> - ~~**P5d** — JOIN reorder por cardinalidad~~ ✓ entregado 2026-06-11 como hash-join build-side swap (ADR-0072) + R3-cont sweep empírico (ADR-0085). JOIN reorder global (commutative) sigue siendo M9, pendiente.
> - **M8** (ex "P5c-futuro") — prefix matching sobre composite indexes (requiere cambio de layout on-disk). Pendiente, ver TAREAS_PENDIENTES §6.5.
> - ~~**P6** (ahora **M2**) — gabybench con benchmarks reproducibles en CI~~ ✓ entregado 2026-06-11 (ADR-0075). job CI `bench` sube `bench/results.json` como artifact por commit. Falta el comparador entre runs (diff vs baseline) — ver TAREAS_PENDIENTES.
>
> **Hilos cruzados** (no atan a una fase):
> - **Z1g** — Argon2id RFC 9106 §A.3 fix definitivo (hoy estructural, no matchea vector; default sigue scrypt). Ver ADR-0061.
> - ~~**T1** — `SAVEPOINT` + `ROLLBACK TO SAVEPOINT`~~ ✓ entregado 2026-06-15 como **M12** (ADR-0089). ANSI SQL:2003 completo: `SAVEPOINT name` / `ROLLBACK TO [SAVEPOINT] name` / `RELEASE [SAVEPOINT] name`.
> - ~~**T2** — cross-request transactions en el server HTTP~~ ✓ entregado 2026-06-15 como **M13** (ADR-0090). 3 endpoints `/tx/{begin,commit,rollback}` + `/exec` con header `X-Gabysql-Session`. Single-slot global.
> - **N1** — parámetros bind (`?`, `$1`) en API.
> - **N2** — `PREPARE` / `EXECUTE` + plan cache.
> - **N3** — `COPY FROM` / `COPY TO` streaming.
> - **N4** — nested transactions (T1/M12 entregado; N4 propiamente dicho — tx anidadas con BEGIN dentro de BEGIN — sigue pendiente, distinto de savepoints).
> - ~~**F2** — agregados sobre `SELECT` con JOIN~~ ✓ cerrado 2026-05-30 (ADR-0066 Gap 1+7). ~~Limitación residual: `COUNT(DISTINCT col)` sobre JOIN sigue rebotando~~ ✓ cerrado 2026-06-15 por **R9** (ADR-0079).
> - ~~**F3** — full-scan-fallback para `BETWEEN` sin índice~~ ✓ cerrado 2026-05-30 (ADR-0066 Gap 2).
> - ~~**W4** — window functions O(n²) → O(n) por partition~~ ✓ cerrado 2026-05-30 (ADR-0066 Gap 8 crítico).
> - ~~**E5** — bare-SELECT sin FROM~~ ✓ cerrado 2026-05-30 (ADR-0066 Gap 3+5).
> - ~~**K3** — UNIQUE/CREATE INDEX multi-col acepta TEXT/UUID/etc~~ ✓ cerrado 2026-05-30 (ADR-0066 Gap 4).
> - ~~**K4** — auto-index sobre primera col de PK compuesta~~ ✓ cerrado 2026-05-30 (ADR-0066 Gap 9).
> - ~~**N5** — DEFAULT con función pura (UUID, current_timestamp)~~ ✓ cerrado 2026-05-30 (ADR-0066 Gap 6). Limitación residual: SERIAL/AUTOINCREMENT persistido requiere bump VERSION.
> - **Y10** — ARRAY type + JSONB.
> - **X5** — cursores explícitos.

Ver [ROADMAP.md](../ROADMAP.md) para el plan completo de bloques en `main`.
