# 📋 Estado actual del producto

> **Snapshot técnico — qué funciona hoy, qué está pendiente y por qué subsistema.** Última verificación: 2026-05-07 contra `main` post-constraints declarativas.

[![Versión](https://img.shields.io/badge/versi%C3%B3n-0.1.x--MVP-7c5cff)](../CHANGELOG.md)
[![Formato en disco](https://img.shields.io/badge/on--disk%20VERSION-6-2d7a66)](TECHNICAL_SPECS.md)
[![Tests integraci%C3%B3n](https://img.shields.io/badge/integration%20tests-30%2F30-brightgreen)](../tests/integration_test.rs)
[![Camino comercial](https://img.shields.io/badge/path-A%20%E2%80%94%20embebido%20nicho-informational)](COMMERCIAL_ROADMAP.md)

---

## 🧱 Madurez por subsistema

Leyenda: 🟢 producción-ready en su scope · 🟡 funcional con limitaciones · 🟠 parcial · 🔴 no implementado

| Subsistema | Estado | Comentario | Archivo |
| :--- | :---: | :--- | :--- |
| Pager (header + caché in-memory) | 🟢 | LRU pendiente, prefetch pendiente. | [src/storage.rs](../src/storage.rs) |
| WAL after-image + replay | 🟢 | CRC32 verificado por record. Sin checkpoints. | [src/storage.rs](../src/storage.rs) |
| CRC32 por página | 🟢 | IEEE polynomial, table-based; verifica en lectura y replay. | [src/storage.rs](../src/storage.rs) |
| Formato en disco versionado | 🟢 | `VERSION = 6`, rechazo explícito de versiones anteriores. | [TECHNICAL_SPECS.md](TECHNICAL_SPECS.md) |
| `Pager::create` no destructivo | 🟢 | Refuses overwrite; `create_force` explícito. | [src/storage.rs](../src/storage.rs) |
| B+Tree (LEAF + INTERNAL) | 🟡 | Splits OK; falta merge / rebalance al borrar. | [src/bptree.rs](../src/bptree.rs) |
| Catálogo persistente | 🟢 | FNV-1a-64, estable entre versiones de Rust. | [src/catalog.rs](../src/catalog.rs) |
| Tipos de columna | 🟡 | INT/TEXT/BOOL/FLOAT/DATE/DATETIME/JSON. Sin DECIMAL ni BIGINT separado. | [src/catalog.rs](../src/catalog.rs) |
| Constraints declarativas (NOT NULL/UNIQUE/DEFAULT) | 🟢 | Inline en `CREATE TABLE`; `CREATE UNIQUE INDEX`; pre-check sin efectos colaterales. | [src/sql.rs](../src/sql.rs), [src/catalog.rs](../src/catalog.rs) |
| `FOREIGN KEY` declarativas + enforced | 🟢 | Single-column, target = PK del parent, `ON DELETE RESTRICT/CASCADE`, self-ref OK, cycle protection en cascade. | [src/sql.rs](../src/sql.rs), [src/catalog.rs](../src/catalog.rs) |
| Índices secundarios (equality) | 🟢 | Una columna, backfill, mantenimiento INSERT/UPDATE/DELETE. | [src/index.rs](../src/index.rs) |
| Índices compuestos | 🔴 | En el [Camino A](COMMERCIAL_ROADMAP.md). | — |
| `WHERE col_indexada = val` (no PK) | 🟢 | Plan dispatch: PK vs índice vs error. | [src/sql.rs](../src/sql.rs) |
| `WHERE BETWEEN` (rango por PK) | 🟢 | Solo en SELECT. | [src/sql.rs](../src/sql.rs) |
| Range scan por índice secundario | 🔴 | En [Camino A](COMMERCIAL_ROADMAP.md). | — |
| ORDER BY / GROUP BY / JOIN | 🔴 | En [Camino A/B/C](COMMERCIAL_ROADMAP.md) según madurez. | — |
| Subqueries / CTE / window functions | 🔴 | Camino C. | — |
| Parser SQL | 🟡 | CREATE TABLE (con `NOT NULL`/`UNIQUE`/`DEFAULT`/`REFERENCES`), DROP TABLE, ALTER TABLE ADD COLUMN, INSERT, SELECT, UPDATE, DELETE, CREATE/DROP INDEX, CREATE UNIQUE INDEX, CREATE/DROP DATABASE, SHOW DATABASES, INTEGRITY CHECK. Sin prepared statements. | [src/sql.rs](../src/sql.rs) |
| `CREATE/DROP DATABASE` + `SHOW DATABASES` | 🟢 | Despachados por server (`/exec`) y CLI antes de abrir Pager. En modo single-DB → 405. | [src/server.rs](../src/server.rs), [src/bin/gabysql.rs](../src/bin/gabysql.rs) |
| Engine (executor) | 🟡 | Sin iterator pattern, sin spill-to-disk, sin plan lógico/físico. | [src/sql.rs](../src/sql.rs) |
| Optimizer cost-based | 🔴 | Camino B/C. | — |
| `EXPLAIN` | 🔴 | Camino A.5+. | — |
| Transacciones | 🟡 | Implícita por `exec`; sin savepoints, sin isolation levels explícitos. | [src/storage.rs](../src/storage.rs) |
| MVCC | 🔴 | Camino C. | — |
| Concurrencia | 🟡 | Mutex global de proceso para escrituras. | [src/server.rs](../src/server.rs) |
| `gabysql-server` HTTP/JSON | 🟢 | Token, multi-DB, `/health`, `/dbs`, `/tables`, `/schema`, `/rows`, `/exec`. | [src/server.rs](../src/server.rs) |
| Cap de conexiones simultáneas | 🟢 | Default 64, configurable con `-max-connections`. | [src/server.rs](../src/server.rs) |
| TLS nativo en server | 🔴 | Reverse proxy en Camino A; nativo en Camino B. | — |
| Authz por usuario / rol | 🔴 | Solo token compartido. Camino B. | — |
| `phpgabyadmin` | 🟢 | Browse / Structure (con índices CRUD inline) / SQL con snippets. | [web/phpgabyadmin/index.php](../web/phpgabyadmin/index.php) |
| `gabymodeler` (modelador web) | 🟢 | Vanilla HTML+JS, drag&drop entidades, FK Bezier, exporta DDL gabysql. | [web/modeler/index.html](../web/modeler/index.html) |
| Backup / restore con verificación | 🔴 | Solo `cp` informal hoy. Camino A. | — |
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

## 🔭 Próximo bloque (Fase 2 — Storage y consulta)

> **Fase 1 cerrada.** Los 5 ítems de robustez funcional están entregados. El siguiente paso natural es Fase 2: índices compuestos, range scan por índice secundario (`WHERE col_indexada BETWEEN ...`), `ORDER BY`, y checkpoint del WAL. También entra en juego el **relayout PowerDesigner-style de gabymodeler** ahora que el motor expone toda la información (constraints, FKs, introspección completa) que la UI necesita.

Ver [ROADMAP.md](../ROADMAP.md) para el plan completo de bloques en `main`.
