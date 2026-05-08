# 📝 Changelog

> **Historial de cambios relevantes aplicados al producto y a su base documental.**

---

## 2026-05-07 — Octava intervención: identificadores duros + introspección completa (Camino A · paso 4)

> **Sin bump de formato.** Los datos en disco no cambian; el cambio es de validación (más estricta) y de contrato JSON (más rico).

### ✨ Identificadores
- Nuevo `catalog::validate_identifier(name, kind)` — única definición de "identificador válido" en el motor: `[A-Za-z_][A-Za-z0-9_]*`, longitud máxima `MAX_IDENT_LEN = 64`, no reservada.
- Lista `catalog::RESERVED_WORDS` con todas las keywords del parser y los nombres de tipo (`int`, `text`, `bool`, `float`, `date`, `datetime`, `json`, etc.).
- Aplicado en `CREATE TABLE` (nombre de tabla + cada columna), `ALTER TABLE ADD COLUMN` (nombre de columna nueva, vía `validate_create_table` sobre meta prospectivo) y `CREATE [UNIQUE] INDEX` (nombre de índice).

### 🌐 Endpoint `/schema` extendido
La respuesta de `GET /schema?db=X&table=Y` (y por tanto también `GET /tables`) ahora incluye lo necesario para reverse-engineering completo desde el frontend:

```json
{
  "ok": true,
  "table": {
    "name": "users",
    "primaryKey": "id",
    "rootPage": 2,
    "columns": [
      { "name": "id",    "type": "INT",  "pk": true,  "notNull": true,  "unique": false, "hasDefault": false, "default": null },
      { "name": "email", "type": "TEXT", "pk": false, "notNull": true,  "unique": true,  "hasDefault": false, "default": null },
      { "name": "status","type": "TEXT", "pk": false, "notNull": true,  "unique": false, "hasDefault": true,  "default": "pending" }
    ],
    "indexes": [
      { "name": "uq_users_email", "column": "email", "rootPage": 4, "unique": true }
    ]
  }
}
```

Campos nuevos por columna: `notNull`, `unique` (derivado de los índices unique de una columna), `hasDefault`, `default` (literal con su tipo nativo en JSON; `null` para "no default" o `DEFAULT NULL`). Campo nuevo por índice: `unique`.

### 🧪 Validación
- 22/22 tests de integración (1 nuevo: `identifier_rules_apply_across_ddl` cubre tabla/columna/índice y los tres rechazos: reservada, longitud, ALTER).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Séptima intervención: edición incremental de schemas (Camino A · paso 3)

> **Sin bump de formato.** El layout `VERSION = 5` ya soporta `TableMeta` con cualquier número de columnas; las filas previas se decodifican con un fallback a `DEFAULT` o `NULL` cuando la fila quedó "corta" frente al esquema nuevo.

### ✨ Funcionalidad SQL
- **`DROP TABLE [IF EXISTS] <name>`** — borra la entrada del catálogo. Las páginas backing (data + índices secundarios) **no** se liberan; el reclaim queda para un futuro `vacuum` (consistente con la política de `DROP INDEX`).
- **`ALTER TABLE <name> ADD [COLUMN] <coldef>`** — agrega una columna al final del esquema. Soporta `NOT NULL`, `DEFAULT`, `UNIQUE`. La keyword `COLUMN` es opcional.

### 🧱 Cambios estructurales
- `decode_row` tolera EOF mientras quedan columnas por decodificar: rellena con el `DEFAULT` de la columna o `NULL`. Permite `ADD COLUMN` sin reescribir filas existentes; el rewrite ocurre naturalmente en el próximo `UPDATE` de cada fila.
- `Catalog::remove_table` borra la entrada del catálogo via `Tree::delete`.
- `parse_column_def` factorizado y compartido entre `CREATE TABLE` y `ALTER TABLE ADD COLUMN`.
- `parse_if_exists` factorizado para `DROP DATABASE` / `DROP TABLE`.

### 🛡️ Restricciones de `ALTER ... ADD COLUMN`
- `PRIMARY KEY` rechazado (la PK ya existe; esta versión no admite swap ni multi-PK).
- `NOT NULL` requiere `DEFAULT` no nulo (sin él, las filas previas violarían la constraint inmediatamente).
- `UNIQUE` con `DEFAULT` no nulo en tabla con > 1 fila se rechaza (produciría duplicados en el backfill).
- `UNIQUE` sin DEFAULT en tabla poblada está OK: filas previas decodifican como `NULL`, y SQL UNIQUE permite múltiples NULLs.
- Nombre de columna duplicado rechazado.
- Validación completa del `coldef` (compatibilidad de tipo del DEFAULT, etc.) reusada del path de `CREATE TABLE`.

### 🧪 Validación
- 21/21 tests de integración (4 nuevos: `drop_table_removes_catalog_entry`, `alter_add_column_decodes_old_rows_with_default_or_null`, `alter_add_column_constraint_guards`, `alter_add_column_unique_then_enforces`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Sexta intervención: constraints declarativas (Camino A · paso 2)

> **On-disk format jump: VERSION 4 → 5.** `Column` ahora persiste `NOT NULL` y `DEFAULT`; `IndexMeta` persiste `unique`. Las DBs creadas con la entrega anterior son rechazadas explícitamente al abrir — re-crear con el binario v5.

### ✨ Funcionalidad SQL
- **`NOT NULL`** como constraint de columna en `CREATE TABLE`. Validado en `INSERT` (columna omitida sin DEFAULT, o `NULL` explícito) y en `UPDATE` (asignación que dejaría la columna en `NULL`). PK es implícitamente `NOT NULL`.
- **`DEFAULT <literal>`** como constraint de columna. Soporta `INT`, `FLOAT`, `BOOL`, `TEXT`/`DATE`/`DATETIME`/`JSON` y `NULL`. La compatibilidad de tipo entre literal y columna se valida en `CREATE TABLE` — `name TEXT DEFAULT 1` se rechaza. PK no admite `DEFAULT`.
- **`UNIQUE`** inline en columna y **`CREATE UNIQUE INDEX`** como sentencia. Inline auto-genera un índice unique con nombre `uq_<tabla>_<columna>`. Múltiples `NULL` se permiten (consistente con SQL estándar). Conflicto de UNIQUE se chequea **antes** de tocar disco — el INSERT/UPDATE falla sin efectos colaterales.
- `CREATE UNIQUE INDEX` sobre tabla con duplicados existentes **aborta el backfill** con error claro; no deja índice colgado.

### 🧱 Cambios estructurales
- `catalog::Column { name, column_type, not_null, default }` con `DefaultLiteral { Null, Integer, Float, Bool, String }` propio del catálogo (no acopla con `sql::Value`).
- `catalog::IndexMeta` lleva `unique: bool`.
- Layout v5 por columna: `[name][type_code:u8][flags:u8][default_payload?]` con `flags & 0x01 = NOT NULL`, `flags & 0x02 = HAS_DEFAULT`.
- Layout v5 por índice: `[name][column][root_page:u32][unique:u8]`.
- Nuevo helper `index::bucket_unique_conflict` y `sql::check_unique_conflict` — un único path de uniqueness para inline UNIQUE y `CREATE UNIQUE INDEX`.
- `sql::ColumnDef` lleva `not_null`, `unique`, `default: Option<Value>` para el AST del parser.

### 🧪 Validación
- 17/17 tests de integración (6 nuevos: `not_null_rejects_missing_and_explicit_null`, `default_fills_missing_and_can_be_overridden`, `default_with_not_null_combination`, `default_type_mismatch_rejected_at_create`, `inline_unique_rejects_duplicates`, `create_unique_index_backfill_aborts_on_duplicates`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-05 — Quinta intervención: DDL de DATABASE + modelador web

### ✨ Funcionalidad SQL
- **`CREATE DATABASE [IF NOT EXISTS] <name>;`** — crea un archivo `.db` en el directorio de `-dir` (server) o junto al path objetivo (CLI).
- **`DROP DATABASE [IF EXISTS] <name>;`** — borra el archivo `.db` y su `.wal` si quedó.
- **`SHOW DATABASES;`** — lista las DBs presentes en el directorio.

Estas sentencias **no se ejecutan contra una `.db` específica** (no operan sobre `TableMeta`). Las despacha el caller — `gabysql-server` para HTTP `/exec` y la CLI `gabysql exec` — antes de abrir el `Pager`. Mezclar DB-level con table-level en un mismo `/exec` se rechaza con error explícito.

### 🌐 Modelador web `gabymodeler`
- Nueva carpeta [`web/modeler/`](web/modeler/) — single-page HTML+CSS+JS vanilla, sin frameworks, sin npm, sin backend acoplado.
- Drag & drop de entidades sobre canvas con grid; SVG para líneas FK Bezier.
- Columnas con tipos (`INT/TEXT/BOOL/FLOAT/DATE/DATETIME/JSON`), flag `PK` (auto-fija `INT`), flag `idx` (índice secundario).
- Botón "↪ FK" para columnas que apuntan a otra entidad — la FK se documenta como comentario en el SQL (las FOREIGN KEY declarativas no se enforced en `VERSION 4`).
- **Exporta SQL** con `CREATE DATABASE [IF NOT EXISTS]` + `CREATE TABLE` + `CREATE INDEX`, copia al clipboard o descarga `.sql`.
- Persiste el modelo en `localStorage` (`gabymodeler.v1`).
- Botón "📦 Cargar ejemplo" trae un schema `users + orders` con FK indexada para evaluar el flujo en 1 click.

### 🧭 Landing `web/index.php` rediseñada
- Reemplaza la tarjeta única de phpgabyadmin por **dos tarjetas lado a lado**: `gabymodeler` y `phpgabyadmin`. Cada una con CTA propio.
- Documenta el flujo recomendado: **modeler → SQL → phpgabyadmin → ejecutar**.

### 🧪 Validación
- 11/11 tests de integración (incluye nuevo `database_level_statements_parse_and_engine_rejects`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.
- `php -l web/index.php` y `php -l web/phpgabyadmin/index.php`: clean.

---

## 2026-05-04 — Cuarta intervención: índices secundarios + scaffolding profesional

> **On-disk format jump: VERSION 3 → 4.** `TableMeta` ahora persiste una lista de índices secundarios; las DBs creadas con la entrega anterior son rechazadas explícitamente al abrir.

### ✨ Funcionalidad SQL
- **Índices secundarios**: `CREATE INDEX <name> ON <table> (<column>);` y `DROP INDEX <name>;`. Soporta backfill automático sobre tablas con datos existentes.
- **`SELECT WHERE col = val` por columna no-PK** consulta el índice cuando existe (lookup O(1) sobre bucket por hash, filtro exacto por bytes, hidratación por PK). Si la columna no es PK ni está indexada, se rechaza con mensaje explícito.
- `WhereClause::Eq` ahora acepta cualquier `Value` (no solo `i64`), por lo que `SELECT WHERE name = 'Ana'` o `WHERE score = 9.5` funcionan igual que `WHERE id = 1`.
- Mantenimiento automático de índices en `INSERT` / `UPDATE` / `DELETE`: el índice solo se actualiza cuando la columna indexada está afectada y el valor cambia.

### 🧱 Cambios estructurales
- Nuevo módulo [`src/index.rs`](src/index.rs): hashing FNV-1a-64, codec de bucket `[count:u16] + N×([vlen:u16][value][pk:i64])`, helpers `bucket_insert/remove/lookup`.
- `TableMeta::indexes: Vec<IndexMeta { name, column, root_page }>` persistido al final del payload del catálogo.
- Reglas de validación: una sola PK INT escalar (sin cambios), una sola columna por índice secundario, `JSON` no es indexable (sin semántica de igualdad canónica).
- `DROP INDEX` no libera páginas — el reclaim queda para una futura herramienta `vacuum`.

### 🛡️ Hardening de CI / supply chain (entrega previa, consolidada en docs)
- 4 workflows: `ci.yml` endurecido, `security.yml`, `workflow-security.yml`, `stale.yml`.
- `cargo audit` 0.22.1 (RustSec), `cargo deny` 0.19.4 (advisories + licenses + bans + sources, regido por [deny.toml](deny.toml)).
- `detect-secrets` (FS + últimos 50 commits), Trojan Source / zero-width / patrones peligrosos Rust+PHP / URLs de exfil.
- `grype` container scan con `--fail-on critical`.
- `actionlint` + `zizmor` + `pin-check` (rechaza acciones sin SHA pin).
- Acciones third-party pinneadas a SHA, `permissions: contents: read` por defecto, `persist-credentials: false`.
- Dependabot semanal: github-actions + cargo + docker.

### 📚 Scaffolding profesional importado desde otros repos del perfil
- `CODE_OF_CONDUCT.md`, `SUPPORT.md`, `COMPATIBILITY.md`, `RECRUITER.md`, `QUICKSTART.md`, `RELEASE.md`.
- `.editorconfig` y `.gitattributes` con normalización LF / CRLF coherente con CI multi-OS.
- `pull_request_template.md` con checklist de fmt/clippy/test/formato-en-disco/supply-chain.

### 🧪 Validación
- 10/10 tests de integración (incluye nuevos: split de B+Tree con 600 filas, detección de corrupción por checksum, rechazo de overwrite, UPDATE/DELETE roundtrip, **índices secundarios end-to-end con backfill + INSERT/UPDATE/DELETE/DROP**).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo audit`, `cargo deny check`: OK.
- `actionlint`, `zizmor`: 0 findings.

### ⚠️ Migración requerida
- DBs creadas con `VERSION = 3` no son legibles. Re-crear con `gabysql init <file.db>`. Mensaje de error explícito al abrir.

---

## 2026-05-03 — Tercera intervención: cierre de hallazgos críticos del MVP

> **On-disk format jump: VERSION 1 → 3.** Toda DB creada antes de esta entrega es rechazada explícitamente al abrir. Recrearla con la versión actual (`gabysql init <file.db>`).

### 🧱 Cambios estructurales del motor
- **B+Tree real**: el índice por PK pasó de una lista enlazada de hojas a un B+Tree con nodos internos. Lookup descendente en O(log N), `root_page` permanece estable cruzando splits gracias a copy-up del root.
- **Hash del catálogo determinista**: las claves del catálogo de tablas se calculaban con `DefaultHasher` (no estable entre versiones de Rust). Reemplazado por FNV-1a-64 inline en código.
- **Checksums CRC32-IEEE**: cada página persiste un trailer de 4 bytes con su CRC. El Pager lo finaliza antes de flushear y verifica al leer y al replay del WAL. La corrupción ahora produce error explícito en vez de silencio.
- **`Pager::create` no destructivo**: rehúsa sobrescribir un archivo existente. Se introdujo `create_force` para el camino explícito de reset (`gabysql init --force <file.db>`).
- **`page_size` honesto**: el header valida que `page_size == PAGE_SIZE_DEFAULT`; el campo se mantiene en disco para una futura revisión del formato.

### ✨ Funcionalidad SQL
- `UPDATE <tabla> SET col = val[, ...] WHERE <pk> = N;` (no permite cambiar la PK).
- `DELETE FROM <tabla> WHERE <pk> = N;` (error si la PK no existe).
- Mensajes de error de PK más explícitos sobre la limitación INT-only de esta versión.

### 🛡️ Endurecimiento del modo server
- `gabysql-server` aplica un techo de conexiones concurrentes (default 64, configurable con `-max-connections N`). Conexiones extra reciben 503 y se cierran sin generar threads.

### 🧪 Validación
- 9/9 tests de integración (incluye nuevos: split de B+Tree con 600 filas, detección de corrupción por checksum, rechazo de overwrite, UPDATE/DELETE roundtrip).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`: OK.

### ⚠️ Migración requerida
- Bases de datos creadas con versiones anteriores a esta entrega no son legibles. El error es explícito (`unsupported gabysql file format: version=...`). Re-crear con el binario actual.

---

## 2026-03-19 — Segunda intervención: migración completa a Rust y estabilización base

### 🧱 Estado actual del sistema
- Motor embebido en Rust con archivo único `.db`
- CLI `gabysql` para `init`, `info`, `exec` y `repl`
- Server HTTP `gabysql-server` para operar una base única o un directorio de bases
- `phpgabyadmin` consumiendo la API HTTP como consola web liviana
- Docker y `docker compose` para levantar server y admin web en un entorno reproducible

### 🏗️ Cambios estructurales
- Se eliminó la implementación anterior en Go y se reemplazó por un proyecto Rust con `Cargo`
- Se separó el core en módulos de storage, catálogo, SQL, servidor y estructura persistente por clave primaria
- Se unificó la documentación para reflejar solo las capacidades reales del motor actual

### ✨ Mejoras funcionales
- Soporte de `CREATE TABLE`, `INSERT` y `SELECT` con full scan, `LIMIT/OFFSET`, `WHERE <pk> = ...` y `BETWEEN`
- Soporte de tipos `INT`, `TEXT`, `BOOL`, `FLOAT`, `DATE`, `DATETIME`, `JSON` y `NULL` en columnas no PK
- Rechazo explícito de claves primarias duplicadas en vez de sobrescritura silenciosa
- Recovery WAL por marcador `COMMIT` para rehidratar páginas confirmadas tras reinicio

### 🛡️ Estabilidad y seguridad
- El parser SQL ahora devuelve errores controlados en escenarios inválidos en lugar de derribar el proceso
- Se corrigió el manejo de comillas escapadas dentro de strings SQL para soportar textos complejos en inserciones multi-sentencia
- `phpgabyadmin` quedó endurecido con cookie firmada y bloqueo de servidores remotos salvo habilitación explícita
- La UI web y el README quedaron alineados con el comportamiento real del motor

### 🎨 Documentación y lenguaje visual
- Se creó un set documental completo alineado con el estándar usado en otros repos del perfil
- Se añadieron guías de instalación, uso, operación, seguridad, troubleshooting y contribución
- Se añadió documentación técnica de arquitectura, requisitos, API y especificaciones del motor
- Se aplicó una capa visual consistente con badges, bloques de estado, tablas de navegación y rutas por perfil

### ✅ Validación y entrega continua
- Se agregaron pruebas de integración para roundtrip básico, PK duplicada, paginación con `LIMIT/OFFSET`, `NULL`, parser inválido y recovery WAL
- Se agregó CI en GitHub Actions con `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` y lint de PHP
- La matriz de CI cubre `ubuntu-latest`, `windows-latest` y `macos-latest`, más build Docker en Linux
- La CI publica artefactos `release` por sistema operativo para facilitar distribución nativa multiplataforma
- El `Dockerfile` valida `cargo test --all-targets` antes de construir binarios release
- `docker compose` permite probar juntos `gabysql-server` y `phpgabyadmin`

### 🧪 Validación realizada en esta intervención
- `cargo fmt --check`: OK
- `cargo check --tests`: OK
- `cargo clippy --all-targets -- -D warnings`: OK
- `docker build -t gabysql .`: OK
- `docker compose up -d --build`: OK
- `GET http://localhost:8080/health`: OK
- `GET http://localhost:8000`: OK

### ⚠️ Límites actuales conocidos (al cierre de la 2ª intervención)
- El índice persistente sigue siendo una estructura de hojas enlazadas por PK `INT`; no es todavía un B+Tree multinivel completo *(superado en la 3ª intervención: ver entrada superior)*
- No hay optimizer cost-based ni estadísticas de consulta
- No hay concurrencia avanzada, MVCC ni transacciones complejas
- Sigue siendo un producto base estable para evolucionar, no un reemplazo directo de motores maduros como PostgreSQL o MySQL
