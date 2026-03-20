# gabysql

`gabysql` es una base de datos embebida escrita en Rust, con archivo único `.db`, WAL simple y una superficie SQL pequeña pero estable para uso local, laboratorios, demos y un producto base que pueda crecer con orden.

No intenta competir todavía con PostgreSQL, MySQL o SQLite en amplitud funcional. El foco actual es otro: storage claro, formato en disco entendible, portabilidad real en Windows/Linux/macOS y una ruta de endurecimiento honesta.

## Estado del producto
- Base funcional y estable para `CREATE`, `INSERT` y `SELECT`.
- Server HTTP/JSON listo para uso local o laboratorio.
- Admin web `phpgabyadmin` operando sobre la API HTTP.
- Validación nativa en Windows, Linux y macOS vía CI.
- Validación reproducible vía Docker y `docker compose`.

## Qué hace hoy
- Archivo `.db` con páginas de `4096` bytes.
- WAL after-image con recovery por marcador `COMMIT`.
- Índice persistente de hojas enlazadas por PK `INT`.
- Catálogo de tablas persistente.
- SQL soportado:
  - `CREATE TABLE`
  - `INSERT`
  - `SELECT * FROM tabla`
  - `SELECT columnas FROM tabla LIMIT/OFFSET`
  - `SELECT ... WHERE <pk> = valor`
  - `SELECT ... WHERE <pk> BETWEEN a AND b`
- Tipos soportados:
  - `INT`
  - `TEXT`
  - `BOOL`
  - `FLOAT`
  - `DATE`
  - `DATETIME`
  - `JSON`
  - `NULL` en columnas no PK

## Compatibilidad
- Nativo: Windows, Linux y macOS mediante binarios Rust.
- Contenedores: Docker para validar y desplegar `gabysql-server` y `phpgabyadmin`.
- CI: GitHub Actions valida `cargo fmt`, `cargo clippy`, `cargo test` y build `release` en `ubuntu-latest`, `windows-latest` y `macos-latest`.
- Artefactos: la CI genera binarios `release` por sistema operativo validado.

## Inicio rápido
### Opción 1: Docker
```powershell
docker build -t gabysql .
docker run --rm -p 8080:8080 -v ${PWD}\data:/data gabysql
```

Stack completo:
```powershell
docker compose up -d --build
```

Luego:
- API: `http://localhost:8080`
- Admin web: `http://localhost:8000/phpgabyadmin/`

### Opción 2: nativo
```powershell
cargo build --release --bin gabysql --bin gabysql-server
cargo run --release --bin gabysql -- init demo.db
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, active BOOL);"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,name,active) VALUES (1,'Ana',TRUE);"
cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users;"
```

Server HTTP:
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
```

Admin web:
```powershell
php -S localhost:8000 -t web
```

## Documentación
- [INSTALL.md](./INSTALL.md): instalación y build en Windows, Linux, macOS y Docker.
- [USER_MANUAL.md](./USER_MANUAL.md): uso de CLI, server HTTP y `phpgabyadmin`.
- [RUNBOOK.md](./RUNBOOK.md): operación diaria, backup, recovery y smoke checks.
- [TROUBLESHOOTING.md](./TROUBLESHOOTING.md): fallos frecuentes y cómo resolverlos.
- [CONTRIBUTING.md](./CONTRIBUTING.md): flujo de trabajo y reglas para cambios.
- [SECURITY.md](./SECURITY.md): postura de seguridad y hardening recomendado.
- [CHANGELOG.md](./CHANGELOG.md): cambios ya realizados.
- [ROADMAP.md](./ROADMAP.md): próximos focos del producto.
- [docs/INDEX.md](./docs/INDEX.md): mapa completo de documentación técnica.

## Arquitectura del repo
- `src/storage.rs`: header, pager, WAL y recovery.
- `src/bptree.rs`: índice persistente de hojas enlazadas.
- `src/catalog.rs`: metadatos de tablas y catálogo.
- `src/sql.rs`: tokenizer, parser, row codec y engine.
- `src/server.rs`: server HTTP/JSON.
- `src/bin/gabysql.rs`: CLI y REPL.
- `src/bin/gabysql-server.rs`: binario del API server.
- `tests/integration_test.rs`: validaciones principales de storage y SQL.
- `web/phpgabyadmin/index.php`: admin web sobre la API.

## Validación actual
El repositorio ya fue validado con:
- `cargo fmt --check`
- `cargo check --tests`
- `cargo clippy --all-targets -- -D warnings`
- `docker build -t gabysql .`
- `docker compose up -d --build`
- `GET /health` sobre `gabysql-server`
- respuesta HTML de `phpgabyadmin`

## Limitaciones actuales
- No hay `UPDATE` ni `DELETE` todavía.
- No hay `JOIN`, `ORDER BY`, `GROUP BY` ni planner cost-based.
- La PK actual debe ser `INT`.
- No hay índices secundarios.
- No hay MVCC ni locking fuerte entre procesos.
- La estructura persistente actual no es todavía un B+Tree multinivel completo.

## Dirección técnica
La dirección correcta hoy es tratar a `gabysql` como un motor embebido tipo SQLite en fase temprana:
- storage local
- formato en disco explícito
- WAL simple y verificable
- API pequeña pero estable
- más prioridad a durabilidad y claridad que a amplitud de features
