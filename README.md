# 🗄️ gabysql

> **Motor embebido en Rust con archivo único `.db`, WAL simple, API HTTP y admin web liviano.**

[![CI](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/ci.yml/badge.svg)](https://github.com/vladimiracunadev-create/gabysql/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/rust-stable-orange.svg)
![Status](https://img.shields.io/badge/status-base%20estable-2e8b57)
![Target](https://img.shields.io/badge/target-Windows%20%7C%20Linux%20%7C%20macOS-1f6feb)
![Storage](https://img.shields.io/badge/storage-single--file%20db%20%2B%20wal-8a5a2b)

`gabysql` es una base de datos embebida escrita en Rust, pensada como un producto base serio: storage claro, formato en disco entendible, portabilidad real y una ruta de evolución honesta. No pretende todavía reemplazar a PostgreSQL, MySQL o SQLite en amplitud funcional; hoy prioriza estabilidad, durabilidad y claridad arquitectónica.

---

## 🚦 Estado actual del producto

> **Estado**: 🟢 Base estable  
> **Superficie SQL**: `CREATE`, `INSERT`, `SELECT`, `UPDATE`, `DELETE` (todos por PK), `WHERE PK`, `LIMIT/OFFSET`  
> **Persistencia**: `.db` + `.wal` con recovery por `COMMIT`, checksums CRC32 por página  
> **Formato en disco**: `VERSION = 3` (B+Tree real con nodos internos, hash de catálogo FNV-1a-64 estable)  
> **Portabilidad**: Windows, Linux y macOS por CI  
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
| Principiante | [docs/BEGINNERS_GUIDE.md](docs/BEGINNERS_GUIDE.md) | recorrido de 10 minutos |
| Usuario / operador | [USER_MANUAL.md](USER_MANUAL.md) | CLI, server y admin web |
| Operación | [RUNBOOK.md](RUNBOOK.md) | health checks, backup, recovery |
| Técnico / maintainer | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | capas del motor y flujo interno |
| API / integración | [docs/API.md](docs/API.md) | endpoints, auth y payloads |
| Seguridad | [SECURITY.md](SECURITY.md) | postura actual y hardening |

---

## ✨ Capacidades actuales

### Storage y catálogo
- Archivo `.db` con páginas de `4096` bytes; los últimos 4 bytes de cada página son CRC32-IEEE.
- WAL after-image con replay por marcador `COMMIT`; cada página dentro del WAL se valida por CRC antes de aplicarse.
- Catálogo persistente de tablas con hashing FNV-1a-64 (estable entre versiones de Rust).
- Índice por PK como **B+Tree real** con nodos internos: lookup descendente en O(log N).
- `Pager::create` rehúsa sobrescribir un archivo existente (use `gabysql init --force` para reset intencional).

### SQL soportado
- `CREATE TABLE`
- `INSERT`
- `SELECT * FROM tabla`
- `SELECT columnas FROM tabla LIMIT/OFFSET`
- `SELECT ... WHERE <pk> = valor`
- `SELECT ... WHERE <pk> BETWEEN a AND b`
- `UPDATE <tabla> SET col = val[, ...] WHERE <pk> = N` (no permite mutar la PK)
- `DELETE FROM <tabla> WHERE <pk> = N`

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
- Admin web `phpgabyadmin`
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
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, active BOOL);"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,name,active) VALUES (1,'Ana',TRUE);"
cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users;"
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

## 📚 Mapa documental

| Documento | Rol |
|---|---|
| [QUICKSTART.md](QUICKSTART.md) | arranque en 3 pasos |
| [INSTALL.md](INSTALL.md) | instalación y build por sistema operativo |
| [USER_MANUAL.md](USER_MANUAL.md) | uso diario del producto |
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
| [docs/INDEX.md](docs/INDEX.md) | índice técnico completo |

---

## 🏗️ Arquitectura del repositorio

- `src/storage.rs`: header, pager, WAL, checksums CRC32 y recovery.
- `src/bptree.rs`: B+Tree real (hojas + nodos internos) con root estable.
- `src/catalog.rs`: metadatos de tablas y catálogo.
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

- `UPDATE` y `DELETE` solo soportan filtro por PK (no por columna no-PK ni por rango); `UPDATE` no muta la PK.
- No hay `JOIN`, `ORDER BY`, `GROUP BY` ni planner cost-based.
- La PK debe ser una sola columna `INT`.
- No hay índices secundarios.
- No hay MVCC ni locking fuerte entre procesos.
- El servidor cap por defecto a 64 conexiones simultáneas (`-max-connections N` para ajustar).

## 🧠 Dirección técnica

La dirección correcta hoy es tratar a `gabysql` como un motor embebido tipo SQLite en fase temprana:
- storage local
- formato en disco explícito
- WAL simple y verificable
- API pequeña pero estable
- más prioridad a durabilidad y claridad que a amplitud de features
