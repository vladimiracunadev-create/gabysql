# 🗄️ gabysql

> **Motor embebido en Rust con archivo único `.db`, WAL simple, API HTTP y admin web liviano.**

[![CI](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/ci.yml/badge.svg)](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/rust-stable-orange.svg)
![Status](https://img.shields.io/badge/status-base%20estable-2e8b57)
![Target](https://img.shields.io/badge/target-Windows%20%7C%20Linux%20%7C%20macOS-1f6feb)
![Storage](https://img.shields.io/badge/storage-single--file%20db%20%2B%20wal-8a5a2b)

`gabysql` es una base de datos embebida escrita en Rust, pensada como un producto base serio: storage claro, formato en disco entendible, portabilidad real y una ruta de evolución honesta. No pretende todavía reemplazar a PostgreSQL, MySQL o SQLite en amplitud funcional; hoy prioriza estabilidad, durabilidad y claridad arquitectónica.

---

## 📚 Documentos clave del producto

> **Atajos a los 7 documentos estratégicos.** El resto del repo está enlazado más abajo en *Mapa documental*.

| 📄 | Documento | Para qué sirve |
| :---: | :--- | :--- |
| 🎯 | [POSITIONING](docs/POSITIONING.md) | qué problema resuelve, ICP, ejemplos de uso reales |
| 💼 | [COMMERCIAL_ROADMAP](docs/COMMERCIAL_ROADMAP.md) | los 3 caminos A/B/C para llegar a producto comercial |
| 🥊 | [COMPETITIVE_ANALYSIS](docs/COMPETITIVE_ANALYSIS.md) | comparativa honesta vs SQLite/DuckDB/Postgres/MySQL/etc. |
| 📋 | [STATUS](docs/STATUS.md) | madurez por subsistema (🟢/🟡/🔴 fila por fila) |
| 🧪 | [USE_CASES](docs/USE_CASES.md) | 17 recetas concretas listas para copiar |
| 📐 | [SQL_REFERENCE](docs/SQL_REFERENCE.md) | gramática con railroad diagrams + EBNF + ejemplos |
| 🛡️ | [SECURITY_LAYERS](docs/SECURITY_LAYERS.md) | mapa completo de las 6 capas de seguridad |
| 🚨 | [ERROR_CODES](docs/ERROR_CODES.md) | catálogo numerado de errores `[GBY-NNNN]` (estilo MySQL `ER_*`) |
| 📜 | [ADRs](docs/adr/) | decisiones arquitectónicas (contexto, alternativas, consecuencias) |

---

## 🚦 Estado actual del producto

> **Estado**: 🟢 Fase 1 (Robustez funcional) cerrada · Fase 2 arrancada  
> **Superficie SQL**: `CREATE DATABASE`, `DROP DATABASE`, `SHOW DATABASES`, `CREATE TABLE` (con `PRIMARY KEY` / `NOT NULL` / `UNIQUE` / `DEFAULT <literal>` / `REFERENCES … ON DELETE RESTRICT|CASCADE`), `DROP TABLE [IF EXISTS]`, `ALTER TABLE ADD [COLUMN] <coldef>`, `INSERT`, `SELECT … [WHERE …] [ORDER BY <col> [ASC|DESC]] [LIMIT n] [OFFSET n]`, `UPDATE`, `DELETE` (con cascade), `CREATE INDEX`, `CREATE UNIQUE INDEX`, `DROP INDEX`, `INTEGRITY CHECK`  
> **Persistencia**: `.db` + `.wal` con recovery por `COMMIT`, checksums CRC32 por página, crash tests dirigidos  
> **Formato en disco**: `VERSION = 7` (B+Tree real, hash de catálogo FNV-1a-64, índices secundarios + `unique` flag + `IndexKind` Hash/OrderedInt, columnas con `not_null` + `default`, `FOREIGN KEY` con `on_delete`)  
> **Portabilidad**: Windows, Linux y macOS por CI · 45/45 tests de integración verdes · `/metrics` + `-log-json` para observabilidad básica · `gabysql backup/restore/verify` con CRC end-to-end · `WHERE col_int_idx BETWEEN a AND b` con índice ordenado  
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
| **Producto / negocio** | [docs/POSITIONING.md](docs/POSITIONING.md) + [docs/COMMERCIAL_ROADMAP.md](docs/COMMERCIAL_ROADMAP.md) | qué problema resuelve y caminos comerciales A/B/C |
| **Comparativa** | [docs/COMPETITIVE_ANALYSIS.md](docs/COMPETITIVE_ANALYSIS.md) | dónde gana / pierde vs SQLite/Postgres/DuckDB/etc. |
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
- `DROP TABLE [IF EXISTS] <name>`
- `ALTER TABLE <name> ADD [COLUMN] <coldef>` (sin reescritura de filas previas)
- `INSERT` (aplica DEFAULTs, valida NOT NULL, pre-check de UNIQUE y FK)
- `SELECT * FROM tabla`
- `SELECT columnas FROM tabla [ORDER BY <col> [ASC|DESC]] LIMIT/OFFSET`
- `SELECT ... WHERE <pk> = valor`
- `SELECT ... WHERE <pk> BETWEEN a AND b`
- `SELECT ... WHERE <col_indexada> = valor` *(usa índice secundario)*
- `SELECT ... WHERE <col_int_indexada> BETWEEN a AND b` *(usa índice INT-ordenado, ADR-0017)*
- `UPDATE <tabla> SET col = val[, ...] WHERE <pk> = N` (valida NOT NULL/UNIQUE/FK; mantiene índices)
- `DELETE FROM <tabla> WHERE <pk> = N` (cascade/restrict según FKs entrantes; mantiene índices)
- `CREATE INDEX <nombre> ON <tabla> (<columna>)` (con backfill automático)
- `CREATE UNIQUE INDEX <nombre> ON <tabla> (<columna>)` (backfill aborta en duplicados)
- `DROP INDEX <nombre>`
- `INTEGRITY CHECK` (sweep operacional: CRCs + filas + índices + FKs)

### Tipos soportados
- `INT`
- `TEXT`
- `BOOL`
- `FLOAT`
- `DATE`
- `DATETIME`
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
| [docs/INDEX.md](docs/INDEX.md) | índice técnico completo |

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

- `UPDATE` y `DELETE` solo aceptan filtro `WHERE <pk> = N` (no por columna no-PK ni por rango); `UPDATE` no muta la PK.
- Los índices secundarios soportan **una sola columna por índice**. `UNIQUE` ya está soportado (inline o `CREATE UNIQUE INDEX`); `BETWEEN` por índice secundario funciona sobre columnas `INT` (índice `OrderedInt`, ADR-0017). Índices compuestos y range scan sobre `TEXT`/`FLOAT`/`DATE`/`DATETIME` indexados quedan para Fase 2.
- `FOREIGN KEY` solo single-column; el target debe ser la PK del parent. `ON DELETE` admite `RESTRICT` y `CASCADE` (no `SET NULL`/`SET DEFAULT`).
- `ORDER BY` ya está soportado; **`JOIN` y `GROUP BY` no** (Fase 2/3).
- Sin planner cost-based; el optimizer es deterministic (PK lookup > index lookup > full scan).
- La PK debe ser una sola columna `INT`. `ALTER TABLE ADD COLUMN` no admite agregar PK.
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
