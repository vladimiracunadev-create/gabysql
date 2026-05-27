# 📋 Estado actual del producto

> **Snapshot técnico — qué funciona hoy, qué está pendiente y por qué subsistema.** Última verificación: 2026-05-25 contra `main` post-bloques E1+E2+E3+F+T+J+J2 (WHERE compuesto + comparadores + UPDATE/DELETE multi-fila + agregados + transacciones explícitas + DML masivo + UPSERT/REPLACE/RETURNING).
>
> 👉 **Para el inventario exhaustivo del SQL no-soportado** (comandos faltantes uno por uno, con prioridades y bloques de implementación): [MISSING_COMMANDS.md](MISSING_COMMANDS.md).

[![Versión](https://img.shields.io/badge/versi%C3%B3n-0.1.x--MVP-7c5cff)](../CHANGELOG.md)
[![Formato en disco](https://img.shields.io/badge/on--disk%20VERSION-7-2d7a66)](TECHNICAL_SPECS.md)
[![Tests integraci%C3%B3n](https://img.shields.io/badge/integration%20tests-283%2F283-brightgreen)](../tests/integration_test.rs)
[![Camino comercial](https://img.shields.io/badge/path-A%20%E2%80%94%20embebido%20nicho-informational)](COMMERCIAL_ROADMAP.md)

---

## 🧱 Madurez por subsistema

Leyenda: 🟢 producción-ready en su scope · 🟡 funcional con limitaciones · 🟠 parcial · 🔴 no implementado

| Subsistema | Estado | Comentario | Archivo |
| :--- | :---: | :--- | :--- |
| Pager (header + caché in-memory) | 🟢 | `PageCache` con cap fija + LRU clean-only (default 1024 páginas ≈ 4 MB; tunable con `set_cache_capacity`). Prefetch pendiente. | [src/storage.rs](../src/storage.rs) |
| WAL after-image + replay | 🟢 | CRC32 verificado por record. Sin checkpoints. | [src/storage.rs](../src/storage.rs) |
| CRC32 por página | 🟢 | IEEE polynomial, table-based; verifica en lectura y replay. | [src/storage.rs](../src/storage.rs) |
| Formato en disco versionado | 🟢 | `VERSION = 7`, rechazo explícito de versiones anteriores. | [TECHNICAL_SPECS.md](TECHNICAL_SPECS.md) |
| `Pager::create` no destructivo | 🟢 | Refuses overwrite; `create_force` explícito. | [src/storage.rs](../src/storage.rs) |
| B+Tree (LEAF + INTERNAL) | 🟡 | Splits OK; falta merge / rebalance al borrar. | [src/bptree.rs](../src/bptree.rs) |
| Catálogo persistente | 🟢 | FNV-1a-64, estable entre versiones de Rust. | [src/catalog.rs](../src/catalog.rs) |
| Tipos de columna | 🟡 | INT/TEXT/BOOL/FLOAT/DATE/DATETIME/JSON. Sin DECIMAL ni BIGINT separado. | [src/catalog.rs](../src/catalog.rs) |
| Constraints declarativas (NOT NULL/UNIQUE/DEFAULT) | 🟢 | Inline en `CREATE TABLE`; `CREATE UNIQUE INDEX`; pre-check sin efectos colaterales. | [src/sql.rs](../src/sql.rs), [src/catalog.rs](../src/catalog.rs) |
| `FOREIGN KEY` declarativas + enforced | 🟢 | Single-column, target = PK del parent, `ON DELETE RESTRICT/CASCADE`, self-ref OK, cycle protection en cascade. | [src/sql.rs](../src/sql.rs), [src/catalog.rs](../src/catalog.rs) |
| Índices secundarios (equality) | 🟢 | Una columna, backfill, mantenimiento INSERT/UPDATE/DELETE. | [src/index.rs](../src/index.rs) |
| Índices compuestos | 🔴 | No incluidos en VERSION 7. Requieren B+Tree byte-keyed o encoder multi-columna; diferidos a un futuro bloque. | — |
| `WHERE col_indexada = val` (no PK) | 🟢 | Plan dispatch: PK vs índice vs error. | [src/sql.rs](../src/sql.rs) |
| `WHERE BETWEEN` (rango por PK) | 🟢 | Solo en SELECT. | [src/sql.rs](../src/sql.rs) |
| Range scan por índice secundario | 🟡 | Solo columnas **INT**: el índice usa el valor como clave del B+Tree (ADR-0017, VERSION 7), `WHERE col_idx BETWEEN a AND b` walk en O(log N + k). TEXT/FLOAT/BOOL/DATE/DATETIME indexados siguen equality-only. | [src/index.rs](../src/index.rs), [src/sql.rs](../src/sql.rs) |
| `ORDER BY` | 🟢 | Cualquier columna, `ASC`/`DESC`, NULLs first. Sort en memoria post-scan. | [src/sql.rs](../src/sql.rs) |
| `JOIN` (INNER/CROSS/LEFT/RIGHT/FULL/USING/NATURAL + index-loop) | 🟢 | Ver entradas separadas más abajo. | [src/sql.rs](../src/sql.rs) |
| `GROUP BY`/`HAVING`/agregados (`COUNT`/`SUM`/`AVG`/`MIN`/`MAX`/`DISTINCT`/`COUNT(DISTINCT)`) | 🟡 | Bloque F (2026-05-25): single-table OK con 3VL ANSI; sobre `SELECT` con `JOIN` aún devuelve `[GBY-4028]`. Window functions / CTE no implementadas. | [src/sql.rs](../src/sql.rs) |
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
| DDL pendiente (PK compuesta, índices compuestos, partial indexes, `ALTER COLUMN TYPE`) | 🔴 | Bloque K2 (planificado aparte; requiere bump de VERSION on-disk 7→8 + ADR). | — |
| Subqueries `ALL`/`ANY`/`SOME` / correlated `=` puro fuera de EXISTS / `LATERAL` / CTE / window functions | 🔴 | P2/P3 y bloque W (CTE). | — |
| `INNER JOIN ... ON l = r`, `CROSS JOIN`, comma-syntax, aliases, multi-tabla, self-join | 🟢 | Nested-loop puro O(N×M×…). WHERE/ORDER BY trabajan sobre filas joineadas. `SELECT *` expande prefijado. | [src/sql.rs](../src/sql.rs) |
| `LEFT [OUTER] JOIN`, `RIGHT [OUTER] JOIN`, `FULL [OUTER] JOIN` con NULL-fill | 🟢 | Implementado vía tracking de matched-rows + NULL-fill por kind. `OUTER` opcional. Combinable en chains. | [src/sql.rs](../src/sql.rs) |
| `JOIN ... USING (col)`, `NATURAL JOIN` | 🟢 | USING soporta 1 columna; NATURAL exige exactamente 1 columna común (>1 → `[GBY-4023]`). `SELECT *` omite la columna fusionada del right (ANSI). | [src/sql.rs](../src/sql.rs) |
| Index-loop join (optimización del nested-loop) | 🟢 | Cuando el `ON` o el USING/NATURAL derivado apunta contra PK o columna indexada del right Y el kind es INNER/LEFT, se hace lookup dirigido en vez de scan completo. O(N1 × log N2) vs O(N1 × N2). RIGHT/FULL siguen nested-loop (no requieren cambios de comportamiento). | [src/sql.rs](../src/sql.rs) |
| Parser SQL | 🟡 | CREATE TABLE (con `NOT NULL`/`UNIQUE`/`DEFAULT`/`REFERENCES` + `IF NOT EXISTS`), CREATE TABLE AS SELECT, DROP TABLE, ALTER TABLE ADD/DROP COLUMN, ALTER TABLE RENAME TO / RENAME COLUMN, RENAME TABLE, INSERT, SELECT, UPDATE, DELETE, CREATE/DROP INDEX, CREATE UNIQUE INDEX, CREATE/DROP DATABASE, SHOW DATABASES, INTEGRITY CHECK. Sin prepared statements. | [src/sql.rs](../src/sql.rs) |
| `CREATE/DROP DATABASE` + `SHOW DATABASES` | 🟢 | Despachados por server (`/exec`) y CLI antes de abrir Pager. En modo single-DB → 405. | [src/server.rs](../src/server.rs), [src/bin/gabysql.rs](../src/bin/gabysql.rs) |
| Engine (executor) | 🟡 | `LeafCursor` lazy para `SELECT … LIMIT N` sin ORDER BY (O(N+offset) IO) + prefetch one-leaf-ahead (ADR-0016) que warm-a la PageCache para la próxima leaf transition. Sin spill-to-disk para sort grande, sin plan lógico/físico explícito. | [src/sql.rs](../src/sql.rs), [src/bptree.rs](../src/bptree.rs) |
| Optimizer cost-based | 🔴 | Camino B/C. | — |
| `EXPLAIN` | 🔴 | Camino A.5+. | — |
| Transacciones explícitas (`BEGIN`/`COMMIT`/`ROLLBACK`) | 🟢 | Bloque T (2026-05-25): batch-local, alias ANSI (`START TRANSACTION`/`END`) y MySQL (`WORK`) aceptados. **Pendiente**: `SAVEPOINT`/`ROLLBACK TO`, isolation levels, read-only y cross-request (HTTP session state). | [src/sql.rs](../src/sql.rs), [src/storage.rs](../src/storage.rs) |
| MVCC | 🔴 | Camino C. | — |
| Manejo de errores | 🟢 | Guía canónica + ~210 mensajes en español con contexto (qué/por qué/cómo). Ver [ERROR_HANDLING.md](ERROR_HANDLING.md). | [src/lib.rs](../src/lib.rs) |
| Concurrencia | 🟡 | Mutex global de proceso para escrituras. | [src/server.rs](../src/server.rs) |
| Lock cross-process sobre `.db` | 🟢 | `File::try_lock()` advisory exclusivo en `Pager::create/open`; falla rápido con error claro si otro proceso tiene la DB. Ver [ADR-0013](adr/0013-process-level-file-lock.md). | [src/storage.rs](../src/storage.rs) |
| `gabysql-server` HTTP/JSON | 🟢 | Token, multi-DB, `/health`, `/metrics`, `/dbs`, `/tables`, `/schema`, `/rows`, `/exec`. | [src/server.rs](../src/server.rs) |
| Observabilidad del server | 🟢 | Endpoint `/metrics` (uptime, counts por status, errors_total, p50/p95 latencia) + flag `-log-json` para logs JSONL por request. Ver [ADR-0014](adr/0014-logs-json-metrics.md). | [src/server.rs](../src/server.rs) |
| Cap de conexiones simultáneas | 🟢 | Default 64, configurable con `-max-connections`. | [src/server.rs](../src/server.rs) |
| TLS nativo en server | 🔴 | Reverse proxy en Camino A; nativo en Camino B. | — |
| Authz por usuario / rol | 🔴 | Solo token compartido. Camino B. | — |
| `phpgabyadmin` | 🟢 | Browse / Structure (con índices CRUD inline) / SQL con snippets. | [web/phpgabyadmin/index.php](../web/phpgabyadmin/index.php) |
| `gabymodeler` (modelador web) | 🟢 | Vanilla HTML+JS, drag&drop entidades, FK Bezier, exporta DDL gabysql. | [web/modeler/index.html](../web/modeler/index.html) |
| Backup / restore con verificación | 🟢 | `gabysql backup/restore [--force] <src> <dst>` + `gabysql verify <db>`. Valida CRC32 por página en lectura **y** re-abre el destino post-escritura. Requiere DB cerrada (lock exclusivo de ADR-0013). Ver [ADR-0015](adr/0015-verified-backup-restore.md). | [src/backup.rs](../src/backup.rs) |
| `INTEGRITY CHECK` operacional | 🟢 | Pages CRC + row decode + index orphans + FK orphans. Devuelve ResultSet con kind/object/detail. | [src/sql.rs](../src/sql.rs) |
| Suite de benchmarks reproducible | 🔴 | `gabybench` especificado pero no implementado. | [GABYBENCH_SPEC.md](GABYBENCH_SPEC.md) |
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

## 🔭 Próximos bloques (post sesión 2026-05-25)

> **Fase 1 cerrada · Fase 2 con superficie SQL relacional clásica completa.** Entregado en esta sesión (7 bloques): E1 (`AND`/`OR`/`NOT` + paréntesis), E2 (comparadores `<`/`>`/`LIKE`/`IS NULL`/`IN literal`), E3 (`UPDATE`/`DELETE` con WHERE completo), F (agregaciones + `GROUP BY`/`HAVING`/`DISTINCT`), T (`BEGIN`/`COMMIT`/`ROLLBACK` explícitos), J (multi-row `INSERT`, `INSERT...SELECT`, `TRUNCATE`), J2 (UPSERT, `REPLACE INTO`, `RETURNING`).
>
> **Pendientes priorizados** (orden recomendado, ver [MISSING_COMMANDS.md](MISSING_COMMANDS.md)):
> - **G** — funciones escalares (`LENGTH`, `UPPER`, `LOWER`, `SUBSTR`, `COALESCE`, `NULLIF`, `CASE WHEN`, `CAST`). **Cerrado 2026-05-26 (G1+G2+G3).**
> - **H** — subqueries restantes: derived tables (`FROM (SELECT ...) t`), `NOT IN (SELECT)`, subquery en SELECT list, multi-predicate correlated. **Cerrado 2026-05-26 (P0+P1).** Sub-pendientes: `ALL/ANY/SOME`, correlated `=` puro, `LATERAL` (P2/P3).
> - **I** — set operations: `UNION`/`UNION ALL`/`INTERSECT`/`EXCEPT`, `VALUES (...)` como tabla virtual. **Cerrado 2026-05-26.**
> - **K** — DDL faltante: PK compuesta, índices compuestos, `CREATE TABLE AS SELECT`, `ALTER TABLE DROP/RENAME COLUMN`, `RENAME TABLE`.
> - **L** — constraints: `CHECK`, `ON DELETE SET NULL/SET DEFAULT`, `ON UPDATE …`, multi-column UNIQUE.
> - **Sub-pendientes de J2**: `EXCLUDED.col` en `DO UPDATE`, `UPDATE ... FROM otra_tabla`.
> - **Sub-pendientes de T**: `SAVEPOINT`/`ROLLBACK TO`, cross-request transactions vía session state HTTP, isolation levels, read-only.
> - **Sub-pendientes de F**: agregados sobre `SELECT` con JOIN (hoy `[GBY-4028]`).
> - **Optimizer**: range scan por índice secundario sobre `TEXT`/`FLOAT`/`DATE`/`DATETIME`, checkpoint del WAL (ADR-0018).

Ver [ROADMAP.md](../ROADMAP.md) para el plan completo de bloques en `main`.
