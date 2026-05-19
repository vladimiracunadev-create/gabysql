# 📋 Estado actual del producto

> **Snapshot técnico — qué funciona hoy, qué está pendiente y por qué subsistema.** Última verificación: 2026-05-07 contra `main` post-constraints declarativas.

[![Versión](https://img.shields.io/badge/versi%C3%B3n-0.1.x--MVP-7c5cff)](../CHANGELOG.md)
[![Formato en disco](https://img.shields.io/badge/on--disk%20VERSION-7-2d7a66)](TECHNICAL_SPECS.md)
[![Tests integraci%C3%B3n](https://img.shields.io/badge/integration%20tests-45%2F45-brightgreen)](../tests/integration_test.rs)
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
| GROUP BY / JOIN | 🔴 | En [Camino A/B/C](COMMERCIAL_ROADMAP.md) según madurez. | — |
| Subqueries / CTE / window functions | 🔴 | Camino C. | — |
| Parser SQL | 🟡 | CREATE TABLE (con `NOT NULL`/`UNIQUE`/`DEFAULT`/`REFERENCES`), DROP TABLE, ALTER TABLE ADD COLUMN, INSERT, SELECT, UPDATE, DELETE, CREATE/DROP INDEX, CREATE UNIQUE INDEX, CREATE/DROP DATABASE, SHOW DATABASES, INTEGRITY CHECK. Sin prepared statements. | [src/sql.rs](../src/sql.rs) |
| `CREATE/DROP DATABASE` + `SHOW DATABASES` | 🟢 | Despachados por server (`/exec`) y CLI antes de abrir Pager. En modo single-DB → 405. | [src/server.rs](../src/server.rs), [src/bin/gabysql.rs](../src/bin/gabysql.rs) |
| Engine (executor) | 🟡 | `LeafCursor` lazy para `SELECT … LIMIT N` sin ORDER BY (O(N+offset) IO) + prefetch one-leaf-ahead (ADR-0016) que warm-a la PageCache para la próxima leaf transition. Sin spill-to-disk para sort grande, sin plan lógico/físico explícito. | [src/sql.rs](../src/sql.rs), [src/bptree.rs](../src/bptree.rs) |
| Optimizer cost-based | 🔴 | Camino B/C. | — |
| `EXPLAIN` | 🔴 | Camino A.5+. | — |
| Transacciones | 🟡 | Implícita por `exec`; sin savepoints, sin isolation levels explícitos. | [src/storage.rs](../src/storage.rs) |
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

## 🔭 Próximo bloque (Fase 2 avanzada)

> **Fase 1 cerrada · Fase 2 en marcha.** Entregado: `ORDER BY`, `LeafCursor` (Iterator pattern · ADR-0008), `PageCache` LRU acotado (memoria server bounded · ADR-0009). Lo que sigue de Fase 2: índices compuestos, range scan por índice secundario (`WHERE col_indexada BETWEEN ...`), checkpoint del WAL, y eventualmente `Transaction` (Unit of Work) cuando aparezca un workload de INSERT masivo medible.

Ver [ROADMAP.md](../ROADMAP.md) para el plan completo de bloques en `main`.
