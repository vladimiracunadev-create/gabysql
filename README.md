# 🗄️ gabysql

> **Motor embebido en Rust con archivo único `.db`, WAL simple, API HTTP y admin web liviano.**

[![CI](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/ci.yml/badge.svg)](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/ci.yml)
[![Security Scan](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/security.yml/badge.svg)](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/security.yml)
[![Workflow security](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/workflow-security.yml/badge.svg)](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/workflow-security.yml)
[![Release](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/release.yml/badge.svg)](https://github.com/vladimiracunadev-create/gabysql/releases/latest)
[![Pages](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/pages.yml/badge.svg)](https://vladimiracunadev-create.github.io/gabysql/)
[![Latest release](https://img.shields.io/github/v/release/vladimiracunadev-create/gabysql?include_prereleases&sort=semver)](https://github.com/vladimiracunadev-create/gabysql/releases/latest)
![Rust](https://img.shields.io/badge/rust-stable-orange.svg)
![Target](https://img.shields.io/badge/target-Windows%20%7C%20Linux%20%7C%20macOS-1f6feb)
![Storage](https://img.shields.io/badge/storage-single--file%20db%20%2B%20wal-8a5a2b)
[![License: MIT](https://img.shields.io/badge/license-MIT-yellow.svg)](LICENSE)

📖 **Documentación online**: <https://vladimiracunadev-create.github.io/gabysql/> (auto-deploy desde `docs/` en cada push a `main`).
📦 **Instalar en Windows**: `iwr https://raw.githubusercontent.com/vladimiracunadev-create/gabysql/main/scripts/install.ps1 | iex` — ver [INSTALL.md](INSTALL.md).

`gabysql` es **un proyecto de aprendizaje + exploración sobre cómo se construye una base de datos**, escrito en Rust desde cero, usando la pregunta *"¿cómo se vería una DB nativa de la era de los agentes LLM?"* como hilo conductor. **No es un producto comercial y no apunta a serlo**: no hay usuarios, no hay clientes, no hay validación externa, y eso está bien. El objetivo es entender bases de datos a fondo y, en paralelo, explorar qué cambia cuando el consumidor principal no es un humano escribiendo SQL ni una app, sino un agente que razona sobre datos.

El motor actual prioriza estabilidad, durabilidad y claridad arquitectónica como **plataforma de exploración** (no como producto). El detalle de hacia dónde va el proyecto vive en **[docs/AGENDA_INVESTIGACION.md](docs/AGENDA_INVESTIGACION.md)**.

---

## 📚 Documentos clave del producto

> **Atajos a los 7 documentos estratégicos.** El resto del repo está enlazado más abajo en *Mapa documental*.

| 📄 | Documento | Para qué sirve |
| :---: | :--- | :--- |
| 📌 | [TAREAS_PENDIENTES](docs/TAREAS_PENDIENTES.md) | **lo próximo a hacer**, ordenado por prioridad real — primer doc al pedir "estado del proyecto" |
| 🔬 | [AGENDA_INVESTIGACION](docs/AGENDA_INVESTIGACION.md) | **agenda real del proyecto**: tesis, ejes de investigación, fases de aprendizaje, anti-agenda |
| 📋 | [STATUS](docs/STATUS.md) | madurez por subsistema (🟢/🟡/🔴 fila por fila) |
| 🧪 | [USE_CASES](docs/USE_CASES.md) | 17 recetas concretas listas para copiar |
| 📐 | [SQL_REFERENCE](docs/SQL_REFERENCE.md) | gramática con railroad diagrams + EBNF + ejemplos |
| 🛡️ | [SECURITY_LAYERS](docs/SECURITY_LAYERS.md) | mapa completo de las 6 capas de seguridad |
| 🚨 | [ERROR_CODES](docs/ERROR_CODES.md) | catálogo numerado de errores `[GBY-NNNN]` (estilo MySQL `ER_*`) |
| 🚧 | [MISSING_COMMANDS](docs/MISSING_COMMANDS.md) | **inventario exhaustivo de lo que falta** del SQL clásico — roadmap concreto para cerrar la línea de comandos |
| 📊 | [BENCHMARK](BENCHMARK.md) | **evaluación profesional vigente** sobre 3 escenarios sintéticos (OLTP / analítica / K2 composite): metodología, P50/P95/P99, top fastest/slowest, issues encontrados con repro + fix sugerido. Se actualiza in-place con cada mejora; corridas históricas se archivan en `docs/benchmarks/` cuando justifican comparación |
| 📜 | [ADRs](docs/adr/) | decisiones arquitectónicas (contexto, alternativas, consecuencias) |
| 🏛️ | Históricos: [POSITIONING](docs/POSITIONING.md) · [COMMERCIAL_ROADMAP](docs/COMMERCIAL_ROADMAP.md) · [COMPETITIVE_ANALYSIS](docs/COMPETITIVE_ANALYSIS.md) | artefactos del intento de pensar `gabysql` como producto. **No son agenda operativa**. |

---

## 🚦 Estado actual del producto

> **Estado**: 🟢 Fase 1 cerrada · Fase 2 cerrada (SQL relacional completo + constraints + vistas + CTEs + window functions + triggers + procedures + user functions + PL/pgSQL completo + tipos extendidos con DECIMAL/BLOB/UUID exactos + USERS/ROLES + GRANT/REVOKE + Row-Level Security) · Fase 3 arrancada con P1+P2+P3 (EXPLAIN + EXPLAIN ANALYZE + ANALYZE TABLE). Secuencia de bloques cerrados: `E1+E2+E3+F+T+J+J2` (2026-05-25) + `G1+G2+G3+H+I+K1+K2` (2026-05-26) + `L1+L2+L3` + residuales L (#2/#3/#4) + `V` (2026-05-27) + `W1+W2+W3` + `X1→X4f+X6` + `Y1→Y9` + `Z1→Z3f` + `P1+P2+P3` (2026-05-29). **VERSION on-disk 31 · 741/741 integration tests verdes (+1 ignored por limitación Argon2id RFC) · CI verde en Ubuntu/macOS/Windows + Docker.**  
> **Superficie SQL** (DDL): `CREATE DATABASE`, `DROP DATABASE`, `SHOW DATABASES`, `CREATE TABLE` (con `PRIMARY KEY` single o compuesta / `NOT NULL` / `UNIQUE (cols)` / `DEFAULT <literal>` / `FOREIGN KEY (a, b) REFERENCES p (x, y)` single o multi-col / `ON DELETE RESTRICT|CASCADE|SET NULL|SET DEFAULT|NO ACTION` / `ON UPDATE ...` / `CHECK (expr)` column-level y table-level / `CONSTRAINT name PRIMARY KEY|UNIQUE|FOREIGN KEY|CHECK`), `CREATE TABLE [IF NOT EXISTS] [(aliases)] AS SELECT …` (CTAS, K1), `DROP TABLE [IF EXISTS]`, `ALTER TABLE ADD [COLUMN]`, `ALTER TABLE ADD [CONSTRAINT name] CHECK (expr)` (L3), `ALTER TABLE DROP COLUMN [IF EXISTS]`, `ALTER TABLE DROP CONSTRAINT [IF EXISTS]` (residual #2), `ALTER TABLE RENAME COLUMN`, `ALTER TABLE RENAME TO` / `RENAME TABLE`, `CREATE [UNIQUE] INDEX idx ON t (a, b, ...)` (K2), `DROP INDEX`, `TRUNCATE [TABLE]`, **`CREATE VIEW [IF NOT EXISTS] v [(col_aliases)] AS SELECT ...`** (V), **`DROP VIEW [IF EXISTS] v`** (V)  
> **Superficie SQL** (DML): `INSERT` (single-row, multi-row `VALUES (..),(..)`, `INSERT INTO t SELECT …`, `ON CONFLICT [(col)] DO NOTHING / DO UPDATE SET …`, `REPLACE INTO`, `RETURNING *`), `SELECT` / `UPDATE` / `DELETE` con `WHERE` completo (todos los comparadores E1+E2, subqueries H, 3VL ANSI), `UPDATE` admite cambio de PK con cascade ON UPDATE (residual #4), `RETURNING`, `JOIN` ANSI completo, agregados con `GROUP BY`/`HAVING`/`DISTINCT`/`ORDER BY`/`LIMIT`/`OFFSET`, derived tables, scalar subqueries, set ops `UNION`/`INTERSECT`/`EXCEPT`/`MINUS` (I), `VALUES (…)` standalone o en FROM (I), expresiones escalares con 27 funciones (string/numéricas/fecha/condicional) + `CAST` + `CASE` + aritméticos + concat + postfix (G1+G2+G3), `SELECT FROM <view>` con expansión transparente (V), `INTEGRITY CHECK`  
> **Superficie SQL** (TCL): `BEGIN` / `START TRANSACTION` / `COMMIT` / `END` / `ROLLBACK` (batch-local; cross-request HTTP y `SAVEPOINT` pendientes)  
> **Superficie SQL** (W/X/Y, 2026-05-29): `WITH [RECURSIVE] cte AS (...)` (W1+W2), window functions `ROW_NUMBER`/`RANK`/`DENSE_RANK`/`LAG`/`LEAD`/`FIRST_VALUE`/`LAST_VALUE`/`SUM|AVG|MIN|MAX|COUNT OVER` (W3), `CREATE TRIGGER [BEFORE|AFTER] {INSERT|UPDATE|DELETE} ON t FOR EACH ROW BEGIN ... END` (X1+X2), `CREATE PROCEDURE name(params) AS BEGIN ... END` + `CALL` (X3), `CREATE FUNCTION name(params) RETURNS type AS <expr|BEGIN..END>` invocable en SELECT/WHERE (X3b+X4f), control de flujo procedural completo `IF/CASE/WHILE/FOR/LOOP/DECLARE/SET/RAISE/EXCEPTION/RETURN` (X4 → X4f), tipos extendidos con aliases (`BIGINT`/`VARCHAR(n)`/`DECIMAL(p,s)`/`DOUBLE PRECISION`/`BOOLEAN`/`TIMESTAMP`/…) + `DECIMAL` exacto i128 + `BLOB` + `TIME` + `UUID` + `UNSIGNED` (Y1→Y9), enforcement `VARCHAR(n)`/`CHAR(n)` en bytes UTF-8 (Y2)
> **Superficie SQL** (Z, 2026-05-29 — seguridad): `CREATE USER name [WITH PASSWORD '...']` con PBKDF2-SHA256 (default), scrypt (scheme=2, Z1c), Blake2b RFC 7693 (Z1d), Argon2id estructural (scheme=3, Z1e/Z1f), `CREATE ROLE`, `DROP USER/ROLE`, `ALTER USER ... SET PASSWORD`, `GRANT/REVOKE {SELECT|INSERT|UPDATE|DELETE|TRUNCATE} ON t TO grantee` (Z2), `SET SESSION AUTHORIZATION 'name' [WITH PASSWORD '...']`, **Row-Level Security**: `CREATE POLICY p ON t FOR {SELECT|INSERT|UPDATE|DELETE|ALL} [TO role[,...]] [USING (expr)] [WITH CHECK (expr)]` (Z3+Z3b+Z3c+Z3e+Z3f), `RETURNING` filtrado contra SELECT policies (Z3d), DEFAULTs aplicados antes de WITH CHECK (Z3f)
> **Superficie SQL** (P, Fase 3, 2026-05-29 — planeación): `EXPLAIN <stmt>` describe el plan como ResultSet textual sin ejecutar (P1), `EXPLAIN ANALYZE <stmt>` ejecuta el inner real + `std::time::Instant` wall-clock + row count real + error captura (P2; **side-effects PERSISTEN** — usar EXPLAIN sin ANALYZE para dry-run), `ANALYZE [TABLE] <name>` colecta `row_count` y lo cachea session-scoped + EXPLAIN consume las stats como `[est.rows=N]` por SCAN (P3)

> **Persistencia**: `.db` + `.wal` con recovery por `COMMIT`, checksums CRC32 por página, crash tests dirigidos  
> **Formato en disco**: `VERSION = 31` — bumps recientes: 23 = `ObjectKind::User/Role` (Z1), 24 = `ObjectKind::Grant` (Z2), 25 = `ObjectKind::Policy` USING (Z3), 26 = UserMeta con scheme byte + salt + iterations PBKDF2 (Z1b), 27 = PolicyMeta con `with_check_sql` + `FOR INSERT` (Z3b), 28 = scrypt scheme=2 (Z1c), 29 = Blake2b H' (Z1d), 30 = Argon2id estructural scheme=3 (Z1e), 31 = Argon2id partial fix (Z1f) — corte semántico, ver ADR-0061. Bumps previos vigentes: Y6 (DECIMAL exacto, i128+scale), Y5 (UNSIGNED en int_width high bit), Y4 (BLOB), Y3 (int_width TINY/SMALL/MEDIUM/INT4), Y2 (max_length VARCHAR/CHAR), Y (TIME+UUID), X3b (`ObjectKind::Function`), X3 (`ObjectKind::Procedure`), V (discriminator tabla/vista), L1 (`ON UPDATE` + SET NULL/DEFAULT), L2 (CHECK trailer), residual #2/#3 (nombres PK/FK opcionales + extras multi-col), K2 (PK + índices compuestos via fingerprint FNV-1a-64). B+Tree real, catálogo FNV-1a-64, índices secundarios `unique` + `IndexKind` Hash/OrderedInt. **Las stats de `ANALYZE` (P3) son session-only — sin bump on-disk; persistencia es P3b.**  
> **Portabilidad**: Windows, Linux y macOS por CI · 741 integration tests verdes · `/metrics` + `-log-json` para observabilidad básica · `gabysql backup/restore/verify` con CRC end-to-end · `WHERE col_int_idx BETWEEN a AND b` con índice ordenado  
> **Performance** (corrida 2026-05-30, 10 DBs sintéticas, 513k filas, ver [BENCHMARK.md](BENCHMARK.md)): PK lookup hot **11-12 µs**, indexed eq **12-17 µs**, PK compuesta (K2) **16-23 µs**, DML in-tx **24-31 µs**, WITH RECURSIVE 5 hops **2.7 ms**, vista expansion **12 ms**. Limitaciones reales identificadas: full scan 200k rows **720 ms**, aggregate full **~700 ms**, RANK/SUM OVER **27-50 s** sobre 500 rows (hallazgo crítico — defer W4).  
> **Runtime opcional**: Docker + `docker compose`

## 🎯 Qué resuelve hoy este repositorio

- **Base local embebida** con archivo único `.db`.
- **Motor SQL mínimo pero útil** para crear tablas, insertar y consultar datos.
- **API HTTP/JSON** para operar una o múltiples bases.
- **Admin web** `phpgabyadmin` para exploración y operación básica.
- **Ruta documental completa** para instalar, operar, extender y endurecer el producto.

---

## 🧭 Rutas recomendadas según perfil

| Perfil | Documento de entrada | Qué mirar primero |
|---|---|---|
| Principiante | [docs/BEGINNERS_GUIDE.md](docs/BEGINNERS_GUIDE.md) + [QUICKSTART.md](QUICKSTART.md) | recorrido de 10 minutos |
| Usuario / operador | [USER_MANUAL.md](USER_MANUAL.md) | CLI, server y admin web |
| Operación | [RUNBOOK.md](RUNBOOK.md) | health checks, backup, recovery |
| Técnico / maintainer | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | capas del motor y flujo interno |
| API / integración | [docs/API.md](docs/API.md) | endpoints, auth y payloads |
| Seguridad | [SECURITY.md](SECURITY.md) + [docs/SECURITY_LAYERS.md](docs/SECURITY_LAYERS.md) | postura, capas y hardening |
| **Para qué existe el proyecto** | [docs/AGENDA_INVESTIGACION.md](docs/AGENDA_INVESTIGACION.md) | tesis, ejes de investigación, fases de aprendizaje |
| **Estado actual** | [docs/STATUS.md](docs/STATUS.md) | madurez por subsistema (qué está 🟢/🟡/🔴) |
| **Decisiones técnicas** | [docs/adr/](docs/adr/) | ADRs numeradas con contexto, alternativas y consecuencias |

---

## ✨ Capacidades actuales

### Storage y catálogo
- Archivo `.db` con páginas de `4096` bytes; los últimos 4 bytes de cada página son CRC32-IEEE.
- WAL after-image con replay por marcador `COMMIT`; cada página dentro del WAL se valida por CRC antes de aplicarse.
- Catálogo persistente de tablas con hashing FNV-1a-64 (estable entre versiones de Rust).
- Índice por PK como **B+Tree real** con nodos internos: lookup descendente en O(log N).
- `Pager::create` rehúsa sobrescribir un archivo existente (use `gabysql init --force` para reset intencional).
- **Lock exclusivo cross-process** sobre el `.db` vía `File::try_lock()` (advisory en Linux/macOS, mandatory en Windows). Dos `gabysql` apuntando al mismo archivo: el segundo falla rápido con mensaje claro, sin corrupción posible. Ver [ADR-0013](docs/adr/0013-process-level-file-lock.md).
- **`PageCache` LRU acotado** (default 1024 páginas ≈ 4 MB por DB; `Pager::set_cache_capacity` runtime). Memoria del server bounded incluso con docenas de DBs activas. Las páginas dirty nunca se evictan — correctness > strict cap. Ver [ADR-0009](docs/adr/0009-page-cache-lru-bounded.md).
- **`LeafCursor` lazy** para `SELECT … LIMIT N`: O(N + offset) páginas leídas, no O(filas_totales). Ver [ADR-0008](docs/adr/0008-leaf-cursor-iterator.md).

### SQL soportado
- `CREATE DATABASE [IF NOT EXISTS] <name>` *(server multi-DB / CLI)*
- `DROP DATABASE [IF EXISTS] <name>`
- `SHOW DATABASES`
- `CREATE TABLE` con constraints inline: `PRIMARY KEY`, `NOT NULL`, `UNIQUE`, `DEFAULT <literal>`, `REFERENCES <tabla>(<col>) [ON DELETE RESTRICT|CASCADE]`
- `CREATE TABLE` con `PRIMARY KEY (a, b, …)` table-level (K2, all-INT NOT NULL)
- `CREATE TABLE [IF NOT EXISTS] [(col_aliases)] AS <select>` (CTAS, K1)
- `DROP TABLE [IF EXISTS] <name>`
- `ALTER TABLE <name> ADD [COLUMN] <coldef>` (sin reescritura de filas previas)
- `ALTER TABLE <name> DROP COLUMN [IF EXISTS] <col>` (K1; bloqueado sobre PK / indexada / FK)
- `ALTER TABLE <name> RENAME COLUMN <old> TO <new>` (K1; arrastra PK + índices + FKs entrantes)
- `ALTER TABLE <name> RENAME TO <new>` / `RENAME TABLE <old> TO <new>` (K1)
- `INSERT` (aplica DEFAULTs, valida NOT NULL, pre-check de UNIQUE y FK)
- `SELECT * FROM tabla`
- `SELECT columnas FROM tabla [ORDER BY <col> [ASC|DESC]] LIMIT/OFFSET`
- `SELECT ... WHERE <pk> = valor`
- `SELECT ... WHERE <pk> BETWEEN a AND b`
- `SELECT ... WHERE <col_indexada> = valor` *(usa índice secundario)*
- `SELECT ... WHERE <col_int_indexada> BETWEEN a AND b` *(usa índice INT-ordenado, ADR-0017)*
- `SELECT ... WHERE <col> IN (SELECT <col> FROM ... [WHERE ...])` *(subquery no-correlacionada, single-column; outer column debe ser PK o tener índice)*
- `SELECT ... WHERE <col> = (SELECT <col> FROM ... [WHERE ...])` *(subquery escalar no-correlacionada; 1 columna × ≤1 fila; 0 filas o NULL → match vacío)*
- `SELECT ... WHERE [NOT] EXISTS (SELECT ... FROM ... [WHERE col = outer_table.col])` *(no-correlacionada O correlacionada single-eq; correlacionada usa post-filter per-row → O(N × subquery))*
- `SELECT ... FROM a [AS x] [INNER] JOIN b [AS y] ON x.col = y.col [JOIN c ON ...]` *(INNER + CROSS + comma-syntax + aliases + multi-tabla + self-join; nested-loop O(N×M))*
- `SELECT ... FROM a LEFT|RIGHT|FULL [OUTER] JOIN b ON x.col = y.col` *(OUTER joins con NULL-fill; `OUTER` opcional)*
- `SELECT ... FROM a JOIN b USING (col)` y `SELECT ... FROM a NATURAL JOIN b` *(sugar/auto-match; `SELECT *` dedupea la columna común)*
- `SELECT ... FROM (SELECT ...) AS sub [JOIN ...]` *(derived tables, H; alias obligatorio)*
- `SELECT (SELECT MAX(x) FROM t) FROM s` *(subquery escalar en SELECT list, H; correlated OK)*
- `<select> UNION [ALL] / INTERSECT [ALL] / EXCEPT [ALL] / MINUS <select>` *(set ops con precedencia ANSI, I)*
- `VALUES (a,b), (c,d), …` standalone o `FROM (VALUES …) AS t(c1,c2,…)` *(I)*
- Expresiones escalares en SELECT list / WHERE / HAVING / UPDATE SET / DELETE WHERE: 27 funciones (string, numéricas, fecha), `CAST`, `CASE WHEN`, aritméticos `+/-/*///%`, concat `||`, postfix `IS [NOT] NULL`/`[NOT] LIKE`/`[NOT] IN`/`[NOT] BETWEEN` sobre cualquier `Expr` *(G1+G2+G3)*
- `UPDATE <tabla> SET col = val[, ...] WHERE <pk> = N` (valida NOT NULL/UNIQUE/FK; mantiene índices)
- `DELETE FROM <tabla> WHERE <pk> = N` (cascade/restrict según FKs entrantes; mantiene índices)
- `CREATE INDEX <nombre> ON <tabla> (<columna>)` (con backfill automático)
- `CREATE INDEX <nombre> ON <tabla> (a, b, …)` (compuesto, K2; all-INT, equality-only via fingerprint FNV-1a-64)
- `CREATE UNIQUE INDEX <nombre> ON <tabla> (<columna>)` (backfill aborta en duplicados)
- `DROP INDEX <nombre>`
- `CREATE [OR REPLACE] VIEW v AS SELECT ...` / `DROP VIEW` (V — vistas lógicas read-only)
- `WITH name AS (SELECT ...)` (W1 — CTE no-recursivo) y `WITH RECURSIVE` (W2 — fixpoint con guard 10K)
- Window functions en SELECT list: `ROW_NUMBER() OVER (PARTITION BY ... ORDER BY ...)`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD`, `FIRST_VALUE`, `LAST_VALUE`, `SUM/AVG/MIN/MAX/COUNT OVER (...)` (W3 — sin frame explícito)
- `CREATE TRIGGER name {BEFORE|AFTER} {INSERT|UPDATE|DELETE} ON t FOR EACH ROW BEGIN ... END` + `DROP TRIGGER` (X1+X2; cascade depth guard 16)
- `CREATE PROCEDURE name(params) AS BEGIN ... END` + `CALL name(args)` + `DROP PROCEDURE` (X3)
- `CREATE FUNCTION name(params) RETURNS type AS <expr | BEGIN ... RETURN expr; END>` invocable en SELECT/WHERE (X3b+X4f)
- Control de flujo procedural completo (X4 → X4f): `IF/THEN/ELSIF/ELSE/END IF`, `CASE WHEN ... END CASE` statement-level, `DECLARE var TYPE [DEFAULT expr]`, `SET var = expr`, `WHILE cond LOOP`, `FOR i IN a..b LOOP`, `LOOP ... END LOOP` standalone, `EXIT [WHEN cond]`, `RAISE [EXCEPTION|NOTICE] 'msg'`, `BEGIN ... EXCEPTION WHEN <code|OTHERS> THEN ... END`, `RETURN expr`. Guard `MAX_LOOP_ITERATIONS=100K`.
- `INTEGRITY CHECK` (sweep operacional: CRCs + filas + índices + FKs)

### Tipos soportados
- `INT` (+ aliases: `BIGINT`, `INTEGER`, `SMALLINT`, `TINYINT`, `MEDIUMINT`, `INT2`, `INT4`, `INT8`)
- `TEXT` (+ aliases: `VARCHAR[(n)]`, `CHAR[(n)]`, `CHARACTER[(n)]`, `CHARACTER VARYING[(n)]`, `NVARCHAR[(n)]`, `NCHAR[(n)]`, `STRING`, `CLOB`). **`(n)` se enforce desde Y2** (bytes UTF-8, `[GBY-4119]`).
- `BOOL` (+ alias `BOOLEAN`)
- `FLOAT` (+ aliases: `REAL`, `DOUBLE`, `DOUBLE PRECISION`, `NUMERIC[(p,s)]`, `DECIMAL[(p,s)]`, `DEC[(p,s)]` — **DECIMAL es alias, no decimal exacto**)
- `DATE`
- `DATETIME` (+ alias `TIMESTAMP`)
- `TIME` (Y, code=8 — HH:MM:SS[.fff], validación lexical en CAST)
- `UUID` (Y, code=9 — 8-4-4-4-12 hex canónico, CAST normaliza a lowercase)
- `JSON`
- `NULL` en columnas no PK

### Runtime y acceso
- CLI `gabysql`
- API `gabysql-server`
- Admin web `phpgabyadmin` (browse / structure / SQL)
- **Modelador web `gabymodeler` v2** (ER → SQL DDL, layout PowerDesigner-style, Check Model con 14 reglas, reverse engineering vía `/tables`) — ver [web/modeler/USER_MANUAL.md](web/modeler/USER_MANUAL.md) (con screenshots) y [web/modeler/README.md](web/modeler/README.md)
- Docker y `docker compose`

---

## ⚡ Inicio rápido

### Opción A — Docker
```powershell
docker build -t gabysql .
docker run --rm -p 8080:8080 -v ${PWD}\data:/data gabysql
```

Stack completo:
```powershell
docker compose up -d --build
```

Entradas principales:
- API: `http://localhost:8080`
- Admin web: `http://localhost:8000/phpgabyadmin/`

### Opción B — Nativo
```powershell
cargo build --release --bin gabysql --bin gabysql-server
cargo run --release --bin gabysql -- init demo.db
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, email TEXT NOT NULL UNIQUE, name TEXT, active BOOL DEFAULT TRUE);"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,email,name) VALUES (1,'ana@x','Ana');"
cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users ORDER BY name ASC;"
cargo run --release --bin gabysql -- exec demo.db "INTEGRITY CHECK;"
```

Levantar API:
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
```

Levantar admin web:
```powershell
php -S localhost:8000 -t web
```

---

## 🧪 Ejemplos de uso

> **15+ recetas concretas listas para copiar** — CLI, HTTP, crate embebido en Rust, clientes Python/Node.js, importar CSV, multi-DB, backup/restore, demostrar la detección de corrupción por CRC, stress test, comparativa con SQLite.
>
> 👉 **[docs/USE_CASES.md](docs/USE_CASES.md)** — todo en un solo documento.

---

## 📚 Mapa documental

| Documento | Rol |
|---|---|
| [QUICKSTART.md](QUICKSTART.md) | arranque en 3 pasos |
| [INSTALL.md](INSTALL.md) | instalación y build por sistema operativo |
| [USER_MANUAL.md](USER_MANUAL.md) | uso diario del producto (CLI + server + admin web) |
| [web/modeler/USER_MANUAL.md](web/modeler/USER_MANUAL.md) | manual de usuario del modelador ER `gabymodeler v2` (con screenshots) |
| [web/modeler/README.md](web/modeler/README.md) | overview técnico de `gabymodeler v2` |
| [RUNBOOK.md](RUNBOOK.md) | operación, backup y recovery |
| [TROUBLESHOOTING.md](TROUBLESHOOTING.md) | resolución de fallos frecuentes |
| [COMPATIBILITY.md](COMPATIBILITY.md) | matriz de compatibilidad (OS, toolchain, Docker, formato) |
| [CONTRIBUTING.md](CONTRIBUTING.md) | reglas de colaboración |
| [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) | normas de comportamiento en la comunidad |
| [SECURITY.md](SECURITY.md) | postura de seguridad y hardening |
| [SUPPORT.md](SUPPORT.md) | cómo pedir ayuda |
| [RELEASE.md](RELEASE.md) | proceso de versionado y entrega |
| [RECRUITER.md](RECRUITER.md) | pitch técnico para evaluación profesional |
| [CHANGELOG.md](CHANGELOG.md) | cambios relevantes aplicados |
| [ROADMAP.md](ROADMAP.md) | dirección técnica y fases futuras |
| [POSITIONING](docs/POSITIONING.md) | qué problema resuelve, ICP y ejemplos de uso |
| [COMMERCIAL_ROADMAP](docs/COMMERCIAL_ROADMAP.md) | tres caminos A/B/C para llegar a producto comercial |
| [COMPETITIVE_ANALYSIS](docs/COMPETITIVE_ANALYSIS.md) | comparativa honesta con SQLite, DuckDB, Postgres, etc. |
| [STATUS](docs/STATUS.md) | snapshot de madurez por subsistema |
| [USE_CASES](docs/USE_CASES.md) | 17 recetas concretas listas para copiar |
| [SQL_REFERENCE](docs/SQL_REFERENCE.md) | esquema de cada comando con railroad diagram + EBNF |
| [ADRs](docs/adr/) | decisiones arquitectónicas con contexto y alternativas |
| [docs/index.md](docs/index.md) | índice técnico completo |

---

## 🏗️ Arquitectura del repositorio

- `src/storage.rs`: header, pager, WAL, checksums CRC32 y recovery.
- `src/bptree.rs`: B+Tree real (hojas + nodos internos) con root estable.
- `src/catalog.rs`: metadatos de tablas y catálogo (incluye `IndexMeta`).
- `src/index.rs`: hashing FNV-1a-64, codec de bucket y helpers de índice secundario.
- `src/sql.rs`: tokenizer, parser, row codec y engine.
- `src/server.rs`: server HTTP/JSON.
- `src/bin/gabysql.rs`: CLI y REPL.
- `src/bin/gabysql-server.rs`: binario del API server.
- `tests/integration_test.rs`: validaciones principales de storage y SQL.
- `web/phpgabyadmin/index.php`: admin web sobre la API.

---

## ✅ Validación actual

El repositorio ya fue validado con:
- `cargo fmt --check`
- `cargo check --tests`
- `cargo clippy --all-targets -- -D warnings`
- `docker build -t gabysql .`
- `docker compose up -d --build`
- `GET /health` sobre `gabysql-server`
- respuesta HTML de `phpgabyadmin`

---

## ⚠️ Limitaciones deliberadas

- `UPDATE` muta la PK con cascade (residual #4); el bloqueo previo se levantó. `UPDATE` y `DELETE` aceptan cualquier `WHERE` válido en `SELECT` (bloque E3) — multi-fila, por columna indexada, por subquery, con combinadores `AND`/`OR`/`NOT`. Fast-path por PK solo cuando el WHERE es exactamente `pk = N` literal; el resto cae a FullScan + filtro 3VL.
- Los índices secundarios soportan single-column en cualquier tipo escalar **e** índices compuestos all-INT NOT NULL (K2, equality-only via fingerprint FNV-1a-64 — no range scan, no mezcla de tipos). `UNIQUE` está soportado (inline o `CREATE UNIQUE INDEX`); `BETWEEN` por índice secundario funciona sobre columnas `INT` single-column (índice `OrderedInt`, ADR-0017). **Pendiente**: range scan sobre `TEXT`/`FLOAT`/`DATE`/`DATETIME` indexados, índices compuestos con columnas no-INT, partial indexes, `ALTER COLUMN TYPE`.
- `FOREIGN KEY` single-column **y multi-column** (`FOREIGN KEY (a, b) REFERENCES p (x, y)`, residual #3). `ON DELETE` y `ON UPDATE` admiten las cinco acciones referenciales `RESTRICT`/`CASCADE`/`SET NULL`/`SET DEFAULT`/`NO ACTION` (L1 + residual #4).
- `ORDER BY` ya está soportado. **`JOIN`** (4 bloques cerrados): A) `INNER`, `CROSS`, comma-syntax, aliases, multi-tabla; B) `LEFT/RIGHT/FULL [OUTER]` con NULL-fill; C) `USING (col)` y `NATURAL JOIN` con dedup en `SELECT *`; D) index-loop optimization (transparente, INNER/LEFT con PK/índice). **Agregados** (bloque F): `GROUP BY`/`HAVING`/`COUNT`/`SUM`/`AVG`/`MIN`/`MAX`/`DISTINCT`/`COUNT(DISTINCT)` single-table; agregados sobre `SELECT` con JOIN devuelven `[GBY-4028]` y quedan para una iteración futura. **Window functions** (W3) y **CTEs** `WITH [RECURSIVE]` (W1/W2) están soportadas desde 2026-05-29 — sin frame explícito en window (`[GBY-4088]`), fixpoint en RECURSIVE con guard de 10K iteraciones.
- Subqueries: `WHERE col IN (SELECT ...)`, `WHERE col NOT IN (SELECT ...)` (H, 3VL ANSI estricta), `WHERE col = (SELECT ...)`, `WHERE [NOT] EXISTS (SELECT ...)` con correlated multi-predicado dentro de `AND`/`OR`/`NOT` (H), derived tables `FROM (SELECT ...) AS t` (H, alias obligatorio), subquery escalar en SELECT list `SELECT (SELECT MAX(x) FROM t) FROM s` (H), CTEs `WITH name AS (...)` no-recursivas y `WITH RECURSIVE` (W1/W2). **Pendiente**: `ALL`/`ANY`/`SOME`, correlated `col = outer.col` puro fuera de `EXISTS`, `LATERAL`.
- **Transacciones explícitas** (bloque T): `BEGIN`/`COMMIT`/`ROLLBACK` batch-local. **Pendiente**: `SAVEPOINT`, cross-request transactions via session state HTTP, isolation levels explícitos, read-only transactions.
- **UPSERT** (bloque J2): `ON CONFLICT [(col)] DO NOTHING | DO UPDATE SET col = literal`. **Pendiente**: `EXCLUDED.col` en RHS de `DO UPDATE` (workaround: precomputar valor en cliente). **`UPDATE ... FROM otra_tabla`** también pendiente.
- Sin planner cost-based; el optimizer es deterministic (PK lookup > index lookup > full scan).
- La PK puede ser una sola columna `INT` o compuesta `(a, b, ...)` all-INT NOT NULL (K2). `ALTER TABLE ADD COLUMN` no admite agregar PK; ALTER PK sobre tabla existente no soportado.
- `JSON` no es indexable (sin semántica de igualdad canónica).
- No hay MVCC. Existe lock advisory cross-process sobre el `.db` (ADR-0013), pero sin MVCC un solo escritor a la vez por archivo.
- El servidor cap por defecto a 64 conexiones simultáneas (`-max-connections N` para ajustar).

## 🧠 Dirección técnica

La dirección correcta hoy es tratar a `gabysql` como un motor embebido tipo SQLite en fase temprana:
- storage local
- formato en disco explícito
- WAL simple y verificable
- API pequeña pero estable
- más prioridad a durabilidad y claridad que a amplitud de features
