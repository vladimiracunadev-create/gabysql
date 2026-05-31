# 👔 RECRUITER

> **Audiencia**: reclutadores, hiring managers, líderes técnicos.  
> **Executive summary**: este repositorio es un motor de base de datos embebido construido desde cero en Rust puro (sin dependencias externas). Combina ingeniería de sistemas (storage, B+Tree real, WAL con CRC, índices secundarios) con disciplina de producto (CI multi-OS, supply-chain hardening, documentación profesional, formato en disco versionado con rechazo explícito de versiones anteriores).

---

## 💼 Qué demuestra este repo a nivel profesional

`gabysql` no es un wrapper de una librería existente. Cada capa está implementada y entendida:

| Capa | Implementación |
| :--- | :--- |
| **Pager / WAL** | Páginas de 4096 B con CRC32-IEEE en trailer, after-image WAL con replay validado por checksum. |
| **B+Tree** | Hojas + nodos internos, root-stable splits con técnica copy-up. Lookup O(log N). |
| **Catálogo** | Persistente, hashing FNV-1a-64 fijado en código (estable entre versiones de Rust). |
| **SQL** | Parser, AST y engine para `CREATE TABLE` (con `PK`/`NOT NULL`/`UNIQUE`/`DEFAULT`/`REFERENCES … ON DELETE RESTRICT\|CASCADE` inline), `DROP TABLE`, `ALTER TABLE ADD COLUMN`, `INSERT`/`SELECT` (con `ORDER BY [ASC\|DESC]`)/`UPDATE`/`DELETE` (con cascade), `CREATE [UNIQUE] INDEX`/`DROP INDEX`, `CREATE/DROP DATABASE`, `SHOW DATABASES`, `INTEGRITY CHECK`. **Todas las constraints son enforced por el engine** — no son meros decoradores. |
| **Modelador web** | `gabymodeler v2`: single-page HTML+JS vanilla (sin npm) layout **PowerDesigner-style** con Object Browser + Canvas + Result List + Status bar. Check Model continuo con 14 reglas (espejo del validador del engine), SQL Preview en vivo, **reverse-engineering** vía `GET /tables`. Manual con screenshots: [web/modeler/USER_MANUAL.md](web/modeler/USER_MANUAL.md). |
| **FOREIGN KEY enforced** | Single-column FK con validación al DDL (target = PK del parent), pre-check en `INSERT`/`UPDATE`, cascade/restrict en `DELETE` con worklist + cycle protection. |
| **Índices secundarios** | Equality lookup `WHERE col = val` resuelto por bucket hash → filtro exacto → hidratación por PK. `UNIQUE` con pre-check sin efectos colaterales. |
| **Crash recovery validado** | 3 tests sintéticos cubren kill-9 entre WAL flush y file flush (replay correcto, WAL sin COMMIT ignorado, replay idempotente). |
| **Server HTTP/JSON** | Hand-rolled (zero deps), token auth opcional, cap de conexiones, mutex de escritura. |
| **CI / Supply chain** | `cargo fmt + clippy + test` multi-OS, `cargo audit + cargo deny`, `detect-secrets` (FS + historial), Trojan Source detection, `grype` container scan, `actionlint + zizmor + pin-check` para los workflows mismos. |

## 📡 Evidencia visible

| Área | Dónde mirarlo |
| :--- | :--- |
| Storage durable | [src/storage.rs](src/storage.rs) — Pager, WAL, CRC32, recovery. |
| B+Tree real | [src/bptree.rs](src/bptree.rs) — splits + root estable. |
| Índices secundarios | [src/index.rs](src/index.rs) + tests en [tests/integration_test.rs](tests/integration_test.rs). |
| SQL completo | [src/sql.rs](src/sql.rs) — parser, AST, engine. |
| Server seguro | [src/server.rs](src/server.rs) — cap conexiones, token, mutex. |
| Decisión de no-overwrite | [src/storage.rs:Pager::create](src/storage.rs) — refuses to destroy existing files silently. |
| Hardening CI | [.github/workflows/](.github/workflows) (4 workflows) + [deny.toml](deny.toml) + [.secrets.baseline](.secrets.baseline). |
| Honestidad técnica | [CHANGELOG.md](CHANGELOG.md), [docs/PLAN_MAESTRO_GABYSQL.md](docs/PLAN_MAESTRO_GABYSQL.md), [docs/tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md](docs/tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md). |

## ⚡ Qué mirar en 5 minutos

1. [README.md](README.md) — qué hace, qué no, y por qué.
2. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — capas y responsabilidades del motor.
3. [docs/TECHNICAL_SPECS.md](docs/TECHNICAL_SPECS.md) — formato en disco, B+Tree, WAL, CRC32, gramática SQL.
4. [docs/PLAN_MAESTRO_GABYSQL.md](docs/PLAN_MAESTRO_GABYSQL.md) — plan por fases para evolucionar sin romper foco.
5. [SECURITY.md](SECURITY.md) — disclosure, scope in/out, mitigaciones.

## ✅ Señales profesionales

| Señal | Dónde se ve |
| :--- | :--- |
| Pensamiento sistémico | El producto se entiende como capas con contratos, no como features sueltas. |
| Disciplina de release | Cada bump de formato en disco: `VERSION` explícita + rechazo + nota en `CHANGELOG`. |
| Honestidad técnica | El plan diferencia claramente "embebido nicho", "cliente-servidor pequeño" y "RDBMS comercial competitivo" — y reconoce qué corresponde al esfuerzo de un solo desarrollador y qué requiere equipo. |
| Disciplina de seguridad | CI con cargo-audit, cargo-deny, detect-secrets, zizmor, pin-check, grype. Acciones third-party pinneadas a SHA, no a tag movible. |
| Documentación viva | Cada bump del producto trae barrido de `README/CHANGELOG/USER_MANUAL/RUNBOOK/TROUBLESHOOTING/SECURITY/ARCHITECTURE/TECHNICAL_SPECS`. |

## 🧠 Lo que este repositorio sí es hoy

- un motor embebido **MVP funcional con storage durable, índices primarios y secundarios, y SQL básico estable**.
- una base con **disciplina de releases y supply-chain de nivel proyecto profesional**.
- una pieza de portafolio que demuestra **ingeniería de sistemas sin atajos** (zero crates externos).

## 🚫 Lo que todavía no intenta vender

- compatibilidad SQL completa con Postgres / MySQL.
- MVCC, replicación, clustering, sharding, wire protocol.
- planner cost-based real, `SAVEPOINT`, transacciones cross-request en el server HTTP, bind params (`?`, `$1`), `PREPARE`/`EXECUTE`, `COPY FROM`/`COPY TO`, ARRAY/JSONB, cursores explícitos, `SERIAL`/`AUTOINCREMENT` (DEFAULT con `gen_random_uuid()`/`current_timestamp()` sí — N5/2026-05-30; counter persistido aún no). (Lo que **sí** está cerrado al 2026-05-30: `GROUP BY`/`HAVING`/agregados single-table — F, `BEGIN`/`COMMIT`/`ROLLBACK` explícitos batch-local — T, funciones escalares + `CAST` + `CASE` + aritméticos + `||` — G1+G2+G3, set ops `UNION/INTERSECT/EXCEPT/MINUS` + `VALUES` — I, derived tables + scalar subquery + correlated multi-pred — H, **bare-SELECT** (`SELECT 1`, `SELECT (subq)`) — E5, CTAS + `RENAME TABLE` + `ALTER TABLE DROP/RENAME COLUMN` — K1, PK all-INT + índices compuestos K2 + **UNIQUE/INDEX multi-col TEXT/UUID/DATE** — K3 + **auto-index sobre primera col de PK compuesta** — K4, constraints `CHECK`/`FK multi-col`/named — L1+L2+L3+#2+#3+#4, vistas — V, **CTEs no-rec + `WITH RECURSIVE`** — W1+W2, **window functions** (`ROW_NUMBER`/`RANK`/`LAG`/`LEAD`/`SUM OVER`/etc., sin frame explícito) — W3, **window functions O(n) por partition** — W4, **triggers `BEFORE`/`AFTER`** con body multi-stmt — X1+X2, **stored procedures + `CALL`** — X3, **user functions** — X3b, PL/pgSQL completo `IF/CASE/WHILE/FOR/LOOP/DECLARE/RAISE/EXCEPTION/RETURN` — X4→X4f+X6, tipos extendidos `DECIMAL` exacto + `BLOB` + `TIME` + `UUID` + `UNSIGNED` — Y1→Y9, **identidad SQL-level**: USERS+ROLES con PBKDF2/scrypt/Blake2b/Argon2id, GRANT/REVOKE, **Row-Level Security** con USING/WITH CHECK — Z1→Z3f, **EXPLAIN + EXPLAIN ANALYZE + ANALYZE TABLE** con stats session-scoped — P1+P2+P3, **agregados sobre SELECT con JOIN** + **`COUNT(*) FROM <view>`** — F2, **BETWEEN sin índice ordenado** — F3, **DEFAULT con función pura** — N5. 9/10 gaps del ADR-0066 cerrados; pendiente Gap 10/P5b dentro del bloque P5 — planner real.)
- soporte de producción comercial (no hay SLA).

## 🌐 Contacto

Las personas reclutadoras pueden contactar al maintainer directamente vía GitHub o por los canales descritos en [SECURITY.md](SECURITY.md).
