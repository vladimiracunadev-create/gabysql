# 🗺️ ROADMAP

> **Dirección técnica de `gabysql`: qué está estable hoy + qué bloques de cada Fase ya cerraron.**
>
> **🔬 Fuente operativa de qué viene después**: [docs/AGENDA_INVESTIGACION.md](docs/AGENDA_INVESTIGACION.md). Este `ROADMAP.md` documenta lo entregado y lo planificado de Fase 1 y Fase 2; las exploraciones futuras (schema semántico, plan-as-data, embedded variants, time-travel) viven en la agenda. El proyecto **dejó de ser comercial**; ver el reframe en `AGENDA_INVESTIGACION.md §1`.

> **Documentos históricos** (no son agenda operativa): [docs/COMMERCIAL_ROADMAP.md](docs/COMMERCIAL_ROADMAP.md), [docs/POSITIONING.md](docs/POSITIONING.md), [docs/COMPETITIVE_ANALYSIS.md](docs/COMPETITIVE_ANALYSIS.md), [ADR-0007](docs/adr/0007-commercial-path-a.md). Quedan en el repo para entender de dónde viene el proyecto.

---

## 🚦 Estado actual

- Core reescrito en Rust
- Pager con header, páginas fijas y formato en disco **versión `8`** (K2, 2026-05-26: PK e índices admiten múltiples columnas all-INT)
- Cada página persistida lleva trailer CRC32-IEEE (4 bytes); corrupción se detecta al leer y al replay del WAL
- WAL after-image con replay por `COMMIT` y verificación CRC del payload de cada página
- **B+Tree real** con nodos internos sobre PK `INT`; `root_page` permanece estable cruzando splits
- Catálogo de tablas persistente con hashing FNV-1a-64 (estable entre versiones de Rust)
- **Índices secundarios** sobre una columna escalar (no JSON), con backfill automático y mantenimiento en `INSERT`/`UPDATE`/`DELETE`
- SQL estable: `CREATE DATABASE`, `DROP DATABASE`, `SHOW DATABASES`, `CREATE TABLE` (con `NOT NULL` / `DEFAULT` / `UNIQUE` / `REFERENCES ... [ON DELETE RESTRICT|CASCADE]` inline; `PRIMARY KEY (a, b, ...)` table-level all-INT desde K2), `CREATE TABLE [IF NOT EXISTS] [(aliases)] AS SELECT ...` (CTAS, K1), `DROP TABLE [IF EXISTS]`, `ALTER TABLE ADD [COLUMN]`, `ALTER TABLE DROP COLUMN [IF EXISTS]` (K1), `ALTER TABLE RENAME COLUMN` (K1), `ALTER TABLE RENAME TO` / `RENAME TABLE` (K1), `INSERT` (single/multi-row, `INSERT ... SELECT`, `ON CONFLICT`/UPSERT, `REPLACE INTO`, `RETURNING`), `SELECT`/`UPDATE`/`DELETE` con `WHERE` completo (`=`/`<`/`>`/`<=`/`>=`/`<>`/`!=`/`BETWEEN`/`[NOT] LIKE`/`IS [NOT] NULL`/`[NOT] IN (lista | SELECT)`/`= (SELECT)`/`[NOT] EXISTS` con correlated multi-pred, conectados con `AND`/`OR`/`NOT` y paréntesis), expresiones escalares (27 funciones + `CAST` + `CASE` + aritméticos + concat `||` + postfix `Expr`) en SELECT/WHERE/HAVING/UPDATE SET/DELETE WHERE (G1+G2+G3), derived tables `FROM (SELECT ...) AS t` y scalar subquery en SELECT list (H), set ops `UNION`/`UNION ALL`/`INTERSECT [ALL]`/`EXCEPT [ALL]`/`MINUS` + `VALUES` (I), JOINs (INNER/LEFT/RIGHT/FULL/CROSS, USING, NATURAL, index-loop), agregados single-table (`GROUP BY`/`HAVING`/`COUNT`/`SUM`/`AVG`/`MIN`/`MAX`/`DISTINCT`/`COUNT(DISTINCT)`, F), TCL (`BEGIN`/`COMMIT`/`ROLLBACK` batch-local, T), `CREATE [UNIQUE] INDEX` single o compuesto all-INT (K2), `DROP INDEX`, `TRUNCATE`, `LIMIT`/`OFFSET`, `ORDER BY`. (Ver [docs/SQL_REFERENCE.md](docs/SQL_REFERENCE.md) para la gramática completa y [docs/MISSING_COMMANDS.md](docs/MISSING_COMMANDS.md) para lo que falta.)
- Modelador web `gabymodeler` (vanilla HTML/JS) + admin web `phpgabyadmin`, ambos en `web/`
- Server HTTP/JSON para single DB y multi DB con tope de conexiones simultáneas (default 64, configurable con `-max-connections`)
- `Pager::create` rehúsa sobrescribir un archivo existente; `gabysql init --force` para reset intencional
- Admin web `phpgabyadmin` sobre la API HTTP
- CI en Windows, Linux y macOS
- Docker para validación y despliegue reproducible
- Documentación completa de instalación, operación, seguridad, API y troubleshooting

---

## 🎯 Prioridades antes de llamarlo v1 serio

### Fase 1 — Robustez funcional ✅ ENTREGADA
- ~~`UPDATE` y `DELETE` por PK~~ ✅ entregado
- ~~checksums por página + WAL~~ ✅ entregado (CRC32-IEEE)
- ~~`NOT NULL`, `DEFAULT` y `UNIQUE` declarativos~~ ✅ entregado (VERSION 5)
- ~~`FOREIGN KEY` declarativas + enforced~~ ✅ entregado (VERSION 6)
- ~~mejor validación de tipos en parser y engine~~ ✅ entregado (identificadores duros + reserved words + DEFAULT/FK type checks)
- ~~comando `INTEGRITY CHECK` que recorra y valide CRCs y la estructura del B+Tree~~ ✅ entregado
- ~~política más clara de compatibilidad del formato en disco (changelog explícito por bump de VERSION)~~ ✅ entregado (1 bloque = 1 push a `main`, CHANGELOG entry por intervención)
- ~~crash tests dirigidos (kill -9 entre WAL y file flush)~~ ✅ entregado (3 escenarios sintéticos en `tests/integration_test.rs`)
- ~~mejoras de full scan para tablas medianas~~ ✅ entregado (ADR-0016, prefetch one-leaf-ahead en `LeafCursor`; medición cuantitativa pendiente hasta `gabybench`)

### Fase 2 — Storage y consulta
- ~~índices secundarios (una columna, equality)~~ ✅ entregado
- ~~`WHERE` por columnas no PK (cuando hay índice)~~ ✅ entregado
- ~~índices `UNIQUE` declarativos~~ ✅ entregado (VERSION 5)
- ~~`FOREIGN KEY` declarativas + enforced~~ ✅ entregado (VERSION 6)
- ~~`ORDER BY <col> [ASC|DESC]`~~ ✅ entregado
- ~~`LeafCursor` lazy para `SELECT … LIMIT N` (O(N+offset) en vez de O(table))~~ ✅ entregado (ADR-0008)
- ~~`PageCache` LRU acotado (memoria del server bounded)~~ ✅ entregado (ADR-0009)
- ~~índices compuestos~~ ✅ entregado en VERSION 8 (sub-bloque **K2**, 2026-05-26) — restringido a all-INT NOT NULL, equality-only via fingerprint FNV-1a-64 (ADR-0019). Range scan sobre claves compuestas y mezcla de tipos siguen pendientes.
- ~~range scan por índice secundario (`WHERE indexed_col BETWEEN ...`)~~ ✅ entregado para columnas **INT** (ADR-0017, VERSION 7). TEXT/FLOAT/BOOL/DATE/DATETIME indexados siguen siendo equality-only — diferido.
- checkpoint/compaction del WAL — **diseño aceptado, implementación deferida** ([ADR-0018](docs/adr/0018-wal-mode-opt-in.md)). El WAL actual es per-transaction (cada commit ya checkpoint-ea); habilitar checkpoint requiere primero un WAL persistente opt-in. Condiciones de salida documentadas en el ADR.
- ~~locking simple entre procesos~~ ✅ entregado (ADR-0013, `File::try_lock` advisory exclusivo en `Pager::create/open`)
- ~~backup / restore verificado~~ ✅ entregado (ADR-0015, `gabysql backup/restore/verify` con CRC end-to-end)
- ~~logs estructurados y primeras métricas del server~~ ✅ entregado (ADR-0014, endpoint `/metrics` + flag `-log-json`)
- ~~funciones escalares en SELECT list (`LENGTH`, `UPPER`, `LOWER`, `SUBSTR`, `CONCAT`, `ABS`, `ROUND`, `NOW`, `CURRENT_DATE`, `CURRENT_TIMESTAMP`, `COALESCE`, `NULLIF`, `IFNULL`, `IF`, `CAST`, `CASE`)~~ ✅ entregado (bloque **G1**, 2026-05-26). Extensión a `WHERE`/`HAVING`/`UPDATE SET` queda para G2.
- ~~funciones escalares también en `WHERE` / `HAVING` / `UPDATE SET` / `DELETE WHERE` / `ON CONFLICT DO UPDATE SET`~~ ✅ entregado (bloque **G2**, 2026-05-26).
- ~~aritméticos binarios (`+`/`-`/`*`/`/`/`%`), operador `||`, postfix `IS [NOT] NULL`/`[NOT] LIKE`/`[NOT] IN`/`[NOT] BETWEEN` sobre cualquier `Expr`, y funciones escalares P2/P3 (`TRIM`/`LTRIM`/`RTRIM`, `REPLACE`, `SPLIT_PART`, `CEIL`/`FLOOR`, `MOD`, `POWER`/`SQRT`, `DATE_ADD`/`DATE_SUB`, `DATEDIFF`, `EXTRACT`, `STRFTIME`)~~ ✅ entregado (bloque **G3**, 2026-05-26). Pendientes residuales menores: `EXCLUDED.col` en UPSERT (sub-pendiente J2-P2) y unary `-` prefix sobre expresión.
- ~~subqueries restantes (P0+P1): derived tables `FROM (SELECT ...) AS sub`, `WHERE col NOT IN (SELECT ...)` con 3VL ANSI, subquery escalar en SELECT list (con correlated), y multi-predicate correlated EXISTS dentro de `AND`/`OR`/`NOT`~~ ✅ entregado (bloque **H**, 2026-05-26). Sub-pendientes para futuros bloques: `ALL`/`ANY`/`SOME` (P2), correlated `col = outer.col` puro fuera de `EXISTS` (P2), `LATERAL` (P3), `WITH` / CTE (bloque W aparte).
- ~~set operations (`UNION`/`UNION ALL`/`INTERSECT`/`INTERSECT ALL`/`EXCEPT`/`EXCEPT ALL`/`MINUS`) con precedencia ANSI + ORDER BY/LIMIT al nivel del resultado combinado, y `VALUES (...), (...)` como query standalone y como tabla virtual en FROM/JOIN (`FROM (VALUES ...) AS t(c1, c2, ...)`)~~ ✅ entregado (bloque **I**, 2026-05-26). Sub-pendientes para futuros bloques: `WITH ... AS (...)` (CTE, bloque W aparte), `ORDER BY 1` posicional sobre set ops, set ops dentro de DML (no es ANSI estándar).
- ~~DDL extendido sin cambio de formato on-disk: `CREATE TABLE [IF NOT EXISTS] [(col_aliases)] AS <select>` (CTAS), `RENAME TABLE` / `ALTER TABLE RENAME TO`, `ALTER TABLE DROP COLUMN [IF EXISTS]` (con bloqueos sobre PK/indexada/FK), `ALTER TABLE RENAME COLUMN` (arrastra PK + índices + FKs entrantes)~~ ✅ entregado (sub-bloque **K1**, 2026-05-26).
- ~~DDL con cambio de formato on-disk: `PRIMARY KEY (a, b, ...)` table-level + `CREATE [UNIQUE] INDEX idx ON t (a, b, ...)`. Bump VERSION 7→8, fingerprint FNV-1a-64 i64 como clave del B+Tree, all-INT NOT NULL exigido. V7 rechazado con `[GBY-1003]` y guía de migración manual~~ ✅ entregado (sub-bloque **K2**, 2026-05-26, [ADR-0019](docs/adr/0019-composite-pk-and-index.md)). Sub-pendientes residuales: partial indexes, `ALTER COLUMN TYPE`, ALTER PK sobre tabla existente, FK multi-col, range scan sobre claves compuestas — todos diferidos.
- ~~Constraints — referential actions y multi-col UNIQUE table-level: `ON DELETE SET NULL / SET DEFAULT`, `ON DELETE NO ACTION` como alias de RESTRICT, parser de `ON UPDATE ...` (persistido pero no disparado porque la PK sigue inmutable), `UNIQUE (a, b, ...)` declarada dentro de `CREATE TABLE` reusando el encoder compuesto de K2. Bump VERSION 8→9 con rechazo limpio de V8 vía `[GBY-1003]`. También parchea un hueco que K2 dejó: el path INSERT/UPDATE/DELETE ahora chequea correctamente UNIQUE compuestos por fingerprint completo (no sólo por la primera columna)~~ ✅ entregado (sub-bloque **L1**, 2026-05-27, [ADR-0020](docs/adr/0020-fk-referential-actions.md)). Sub-pendientes: multi-col `FOREIGN KEY`; activación real de `ON UPDATE` (requiere lift de `[GBY-4008]`).
- ~~Constraints — `CHECK (expr)`: column-level (`age INT CHECK (age >= 0)`) y table-level (`CHECK (lo <= hi)`), con o sin nombre (`CONSTRAINT name CHECK (...)`). Persistencia por texto canónico vía `format_expr` + re-parse vía `parse_expr_str`; evaluación en INSERT/UPDATE/UPSERT/DO UPDATE y dentro de cascade SET NULL/SET DEFAULT. 3VL ANSI: NULL pasa, FALSE rebota con `[GBY-3008]`. Subqueries dentro de CHECK rechazadas en DDL con `[GBY-4069]`. Bump VERSION 9→10~~ ✅ entregado (sub-bloque **L2**, 2026-05-27, [ADR-0021](docs/adr/0021-check-constraints.md)). Con L2 cierra el bloque **L** completo.
- ~~`ALTER TABLE <t> ADD [CONSTRAINT <name>] CHECK (<expr>)` con re-validación O(n) de todas las filas existentes antes de persistir. Sin estado parcial: cualquier fila que viole el predicado nuevo aborta el ALTER con `[GBY-3008]` y la PK ofensiva. Sin bump de formato (el slot ya estaba en V10)~~ ✅ entregado (sub-bloque **L3** / residual #1 de L, 2026-05-27).
- ~~Nombres explícitos en PK/UNIQUE/FK declarados con `CONSTRAINT <name> PRIMARY KEY (...)`, `CONSTRAINT <name> UNIQUE (...)`, `CONSTRAINT <name> FOREIGN KEY (col) REFERENCES t (col) [ON ...]` table-level (single-col FK). Habilita `ALTER TABLE <t> DROP CONSTRAINT [IF EXISTS] <name>` con lookup case-insensitive sobre CHECK/UNIQUE/FK y rechazo `[GBY-4072]` sobre la PK. Bump VERSION 10→11 con rechazo limpio de V10 vía `[GBY-1003]`~~ ✅ entregado (residual #2 de L, 2026-05-27, [ADR-0022](docs/adr/0022-named-constraints-and-drop.md)).
- ~~Multi-column FOREIGN KEY (`FOREIGN KEY (a, b) REFERENCES parent (x, y)`) con todos los `ON DELETE` del bloque L1 (CASCADE/SET NULL/SET DEFAULT/RESTRICT/NO ACTION). Target debe matchear la PK compuesta del parent — lookup O(log n) via fingerprint FNV-1a-64 (mismo encoder de K2). Cascade busca children por full-scan tuple-match (mejora indexada futura). `ALTER TABLE DROP COLUMN` y `RENAME COLUMN` arrastran extras. Bump VERSION 11→12~~ ✅ entregado (residual #3 de L, 2026-05-27, [ADR-0023](docs/adr/0023-multi-col-foreign-key.md)).
- ~~Activación real de `ON UPDATE` — lift de `[GBY-4008] UPDATE_PK_NOT_ALLOWED` para regular UPDATE (UPSERT DO UPDATE sigue restringido). El motor recomputa PK al SET, valida duplicate, dispara la acción declarada en cada FK entrante (CASCADE/SET NULL/SET DEFAULT/RESTRICT/NO ACTION), y mueve la fila (delete + insert) manteniendo índices secundarios. ON UPDATE es no-op si la columna target específica no cambió. Sin bump de formato — el byte `on_update` ya vivía persistido desde L1. Edge case del child cuyo PK depende del cascade → `[GBY-4074]` claro. Con este push **cierra el bloque L 100% — todos los residuales entregados**~~ ✅ entregado (residual #4 de L, 2026-05-27, [ADR-0024](docs/adr/0024-on-update-activation.md)).
- ~~Performance fixes post-BENCHMARK pre-L+V: memoización de scalar subquery no-correlacionada (Issue #1, ~factor del LIMIT en re-evals ahorradas), política `[GBY-4001]` consistente (Issue #3, `WHERE col_no_idx = val` ya no rebota), fast-path por fingerprint para composite PK lookup (Issue #4, 145 ms → 216 µs ~670×), `AggArg::Expr` para `SUM(qty*price)` (Issue #5, era parse error), hash join O(N+M) para equi-joins fuera del index-loop (Issue #6). Issue #2 (bucket overflow en índice secundario) queda diferido con error claro + workarounds — fix real requiere overflow chain en el bucket layer. Reporte cuantitativo en [BENCHMARK.md](BENCHMARK.md)~~ ✅ entregado (post-L+V tuning, 2026-05-27).
- ~~CTEs no-recursivas (`WITH name AS (SELECT ...) [, name2 AS (...)]* SELECT ...`). Múltiples CTEs encadenables (CTE2 puede referenciar CTE1), visible desde `FROM`/`JOIN`/subqueries (`IN`/`EXISTS`/scalar) y ambos lados de un set op (`UNION`/`INTERSECT`/`EXCEPT`). Resolución de nombres prioriza la CTE sobre tablas reales (shadowing ANSI). Column aliases en la cabecera rechazados con `[GBY-4081]` (workaround inline). Implementación por inlining post-parse como derived tables (sin tocar executor, sin bump de formato). Re-ejecución si la CTE se referencia N veces — optimización futura por memoización~~ ✅ entregado (bloque **W1**, 2026-05-28, [ADR-0026](docs/adr/0026-cte-non-recursive.md)).
- ~~`WITH RECURSIVE name AS (anchor UNION [ALL] step) <body>` — fixpoint base+step con delta semantics ANSI. Una sola CTE recursive por statement (multi → `[GBY-4082]`), body canónico (no-UNION → `[GBY-4086]`), guards de runaway: 1000 iteraciones (`[GBY-4083]`) y 100K filas (`[GBY-4084]`). El accum final se inyecta al body via el bridge `rows_to_values_select` + `inline_cte_into_query` (reusa el inlining de W1). Sin bump de formato — la materialización vive solo en runtime~~ ✅ entregado (bloque **W2**, 2026-05-28, [ADR-0027](docs/adr/0027-with-recursive.md)).
- ~~Window functions con `OVER ( [PARTITION BY ...] [ORDER BY ... [ASC|DESC]] )`: ranking (`ROW_NUMBER`, `RANK`, `DENSE_RANK`, `NTILE`), aggregate (`COUNT(*)`/`COUNT(expr)`/`SUM`/`AVG`/`MIN`/`MAX` con running cuando hay ORDER BY o full-partition sin), value (`LAG`/`LEAD`/`FIRST_VALUE`/`LAST_VALUE`). 13 funciones cubiertas. Pipeline: materializa todas las filas source post-WHERE, particiona y ordena, computa per-row. Mezcla con `GROUP BY` rechazada (`[GBY-4090]`), workaround vía derived table. Frame specs explícitos (`ROWS BETWEEN ...`) y `WINDOW w AS (...)` named windows diferidos~~ ✅ entregado (bloque **W3**, 2026-05-28, [ADR-0028](docs/adr/0028-window-functions.md)). Con este push **cierra el bloque W completo**.

- ~~Triggers AFTER `{INSERT|UPDATE|DELETE}` con body single-statement (`CREATE TRIGGER name AFTER <event> ON table FOR EACH ROW <single_dml>`, `DROP TRIGGER [IF EXISTS] name`). Persistencia en catálogo (nuevo `ObjectKind::Trigger`, VERSION 13→14). Referencias `NEW.col` / `OLD.col` resueltas vía substitución a nivel de TOKEN antes de cada fire — funciona dentro de `INSERT VALUES`, `UPDATE SET`, `WHERE`, etc. Guard de recursión `MAX_TRIGGER_DEPTH=16` (`[GBY-4095]`). BEFORE diferido a X2; body multi-statement / lenguaje procedural / `CREATE FUNCTION` / `CREATE PROCEDURE` diferidos a sub-bloques posteriores~~ ✅ entregado (bloque **X1**, 2026-05-28, [ADR-0029](docs/adr/0029-triggers-after-x1.md)).
- ~~Triggers BEFORE + body multi-statement (`BEGIN stmt; stmt; ... END`). BEFORE construye NEW best-effort: en INSERT son los cols user-stated (resto NULL); en UPDATE se evalúan las assignments contra OLD (sin tocar disco); en DELETE solo OLD. NEW es read-only en X2 (mutar NEW para "rellenar defaults" llega en X3+). Errores del trigger BEFORE propagan y abortan el DML principal (transacción rollback) — esa es la mecánica de validación. `split_statements` extendida para distinguir `BEGIN [TRANSACTION];` (tx, sin block) de `BEGIN <dml>` (block-open, mantiene `;` internos). Tokenizer admite `;` como Symbol~~ ✅ entregado (bloque **X2**, 2026-05-28, [ADR-0030](docs/adr/0030-triggers-before-multistmt-x2.md)).

- ~~Stored procedures (`CREATE PROCEDURE name(p1 TYPE, ...) AS <body>`, `DROP PROCEDURE [IF EXISTS]`, `CALL name(args)`). Persistencia en catálogo: nuevo `ObjectKind::Procedure` (VERSION 14→15). Body single-stmt o `BEGIN ... END` multi-stmt (mismo grammar que triggers). Args en CALL son expresiones evaluadas contra fila vacía. Substitución de parámetros via token-sub bare-ident — limitación: choque con columnas del mismo nombre (workaround documentado: prefijar `p_`). CALL es statement standalone (no Expr); funciones escalares invocables en SELECT quedan para X3b~~ ✅ entregado (bloque **X3**, 2026-05-28, [ADR-0031](docs/adr/0031-stored-procedures-x3.md)).

- ~~User-defined scalar functions (`CREATE FUNCTION name(p1 TYPE, ...) RETURNS TYPE AS <expr>`, `DROP FUNCTION [IF EXISTS]`). Invocables desde cualquier expresión (SELECT/WHERE/HAVING). Body es UNA `Expr` (desviación práctica de ANSI — sin SELECT/FROM). Persistencia: nuevo `ObjectKind::Function` (VERSION 15→16). AST: nuevo `Expr::UserFunc { name, args }` + arm en 17 walkers de Expr. Composición trivial (functions invocan otras functions). CHECK constraints rechazan user functions para preservar pureza~~ ✅ entregado (bloque **X3b**, 2026-05-28, [ADR-0032](docs/adr/0032-user-functions-x3b.md)). Con X3b cierran las **4 routines server-side clásicas**: triggers (X1+X2), procedures (X3), functions (X3b).

- ~~Control de flujo `IF expr THEN <stmts> [ELSIF expr THEN <stmts>]* [ELSE <stmts>] END IF` como statement top-level. Útil sobre todo en bodies de trigger/procedure pero también funciona en batches SQL planos. Condición evalúa a BOOL (NULL→FALSE, 3VL). IF anidado soportado. NEW/OLD/params se substituyen por valores antes del parse, así que la condición ve literales y el engine la evalúa contra row vacío. Splitter de statements + body parsers extendidos para trackear `IF ... END IF` igual que `BEGIN ... END`. Sin bump on-disk. Variables/LOOP/EXCEPTION diferidos a X4b+~~ ✅ entregado (bloque **X4**, 2026-05-28, [ADR-0033](docs/adr/0033-if-then-else-x4.md)).

- ~~Variables locales (`DECLARE name TYPE [DEFAULT expr]`), asignación (`SET name = expr`), `WHILE cond LOOP ... END LOOP` con guard `MAX_LOOP_ITERATIONS=100K`, `EXIT [WHEN cond]` con sentinel propagation. Engine field `var_scope: HashMap<String, Value>` plano (no anidado — limitación X4b). Variables visibles en `Expr` (cond de IF/WHILE, RHS de SET, WHERE, etc.) via merge en `eval_expr_full`. Variables NO visibles dentro de `INSERT VALUES` (parser exige Value literal — workaround: usar `INSERT ... SELECT` o procedure params). Sin bump on-disk~~ ✅ entregado (bloque **X4b**, 2026-05-28, [ADR-0034](docs/adr/0034-vars-loops-x4b.md)).

- ~~`RAISE [EXCEPTION|NOTICE] 'msg'` (default EXCEPTION) — aborto explícito con mensaje (`[GBY-4111]`) o info logging. Funciona en cualquier contexto procedural (top-level, dentro de IF/WHILE/FOR, trigger/procedure body). `FOR ident IN start TO end LOOP <body> END LOOP` — range loop con auto-declaración de la variable de iteración (shadowing con restore), inclusivo, ascendente con step=1. `start > end` no itera (sin error). EXIT y guard MAX_LOOP_ITERATIONS heredados de X4b. EXCEPTION handlers + FOR row IN SELECT + LOOP standalone + RETURN diferidos a X4d~~ ✅ entregado (bloque **X4c**, 2026-05-28, [ADR-0035](docs/adr/0035-raise-for-x4c.md)).

### Fase 3 — Planeación y rendimiento
- planner básico con stats
- `EXPLAIN`
- mejor layout interno del índice
- benchmarks reproducibles con `gabybench`
- profiling y tuning de scans/rangos
- observabilidad del server más madura
- comparación objetiva con SQLite, PostgreSQL, MySQL/MariaDB y DuckDB

### Fase 4 — Operación de producto
- release process más formal
- empaquetado multiplataforma más simple
- política de compatibilidad de formato en disco
- estrategia de backups y restore automatizable
- endurecimiento adicional del admin web
- authz más serio en modo server

### Fase 5 — AI-native (apertura)

> **Tesis**: el consumidor que crece más rápido en el ecosistema es el agente LLM. Hoy cualquier integración con `gabysql` desde un agente requiere pegamento manual (cliente HTTP + token + schema metido en el prompt). Esta fase elimina ese pegamento sin tocar el motor. Justificación y diseño detallados en [ADR-0010](docs/adr/0010-mcp-gateway.md).

- ~~**Gateway MCP** (`gabysql-mcp`) como **binario adaptador separado** sobre el HTTP/JSON existente~~ ✅ entregado (ADR-0010)
  - tools: `gabysql_list_databases`, `gabysql_describe_database`, `gabysql_query`, `gabysql_execute` (omitido en `--read-only`), `gabysql_integrity_check`
  - resources: `gabysql://catalog`, `gabysql://schema/{db}`
  - **cero deps externas** (decisión final superó el objetivo del ADR — JSON parser, JSON-RPC y cliente HTTP/1.1 implementados a mano en el binario; ADR-0001 intacto)
  - reusa authz (bearer token), `write_lock` y rate-limit del server tal cual
- ~~**Búsqueda semántica** (top-k vectorial sobre columnas TEXT con array JSON)~~ ✅ entregada via gateway (ADR-0011)
  - tool `gabysql_vector_search` con métricas cosine/euclidean/dot, top-k configurable
  - **cero bump de formato** — los vectores son `TEXT`, el cómputo ocurre en `gabysql-mcp` en Rust
  - condiciones de salida hacia un `VECTOR(n)` nativo documentadas en el ADR (>100K vectores, demanda de operadores SQL, índice ANN)
- ~~**Audit log enriquecido** (cada escritura registra el agente, el "por qué" semántico y el clientInfo además del SQL)~~ ✅ entregado en el gateway (ADR-0012)
  - flag `--audit-log <path>` (también `GABYSQL_AUDIT_LOG`); opt-in, sin overhead si no se activa
  - captura `clientInfo` de `initialize` + argumento `reason` opcional en `gabysql_execute`
  - tool `gabysql_audit_tail(n)` para que el propio agente revise sus acciones
  - JSONL append-only, procesable con `jq`/`tail`/ingest a S3/ELK
  - **cero impacto en el motor** — sin bump de formato, sin tocar `storage.rs`/`bptree.rs`/`sql.rs`/`catalog.rs`/`server.rs`

---

## 🚫 Lo que no conviene hacer todavía

- venderlo como reemplazo directo de PostgreSQL/MySQL
- prometer SQL amplio sin planner y sin índices secundarios
- exponerlo a Internet sin proxy, TLS y controles externos
- crecer features antes de consolidar recovery, constraints y compatibilidad del storage
- entrar demasiado pronto en replicación, clustering o wire protocol compatible

---

## 🧠 Dirección recomendada

`gabysql` sigue apuntando a una base embebida tipo SQLite:
- storage local
- archivo único
- **superficie SQL relacional clásica completa** (post-sesión 2026-05-25): DDL (incluyendo `TRUNCATE`), DML completo (`INSERT` single/multi-row/`SELECT`, `UPSERT` con `ON CONFLICT`, `REPLACE INTO`, `RETURNING`), índices secundarios + UNIQUE + range scan sobre INT, FK con cascade/restrict, `ORDER BY`, `WHERE` con todos los operadores E1+E2 (`=`/`<`/`>`/`<=`/`>=`/`<>`/`!=`/`BETWEEN`/`LIKE`/`IS NULL`/`IN literal`/`IN (SELECT)`/`= (SELECT)`/`[NOT] EXISTS`) combinados con `AND`/`OR`/`NOT` y paréntesis (3VL ANSI), todos los JOINs ANSI (INNER, CROSS, LEFT/RIGHT/FULL, USING, NATURAL, multi-tabla, self-join, index-loop optimization), `UPDATE`/`DELETE` con `WHERE` completo (E3), agregaciones single-table (`GROUP BY`/`HAVING`/`COUNT`/`SUM`/`AVG`/`MIN`/`MAX`/`DISTINCT`/`COUNT(DISTINCT)` — bloque F), **transacciones explícitas** `BEGIN`/`COMMIT`/`ROLLBACK` batch-local (bloque T)
- foco en estabilidad, durabilidad y compatibilidad antes que en amplitud OLAP (sin window functions / CTE recursivas, sin agregados sobre JOIN aún, sin `EXCLUDED.col` en UPSERT, sin `UPDATE ... FROM`, sin `SAVEPOINT`, sin partial indexes / `ALTER COLUMN TYPE` / ALTER PK / FK multi-col, sin range scan sobre claves compuestas)

> Para el **inventario exhaustivo de comandos SQL que faltan** (con prioridades P0–P3 y la secuencia de bloques cerrados `E1 → E2 → E3 → F → T → J → J2 → G1 → G2 → G3 → H → I → K1 → K2 → L (completo) → V` y pendientes `W → X → Y → Z`), ver [docs/MISSING_COMMANDS.md](docs/MISSING_COMMANDS.md).
