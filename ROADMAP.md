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

> Para el **inventario exhaustivo de comandos SQL que faltan** (con prioridades P0–P3 y la secuencia de bloques cerrados `E1 → E2 → E3 → F → T → J → J2 → G1 → G2 → G3 → H → I → K1 → K2` y pendientes `L → V → W → X → Y → Z`), ver [docs/MISSING_COMMANDS.md](docs/MISSING_COMMANDS.md).
