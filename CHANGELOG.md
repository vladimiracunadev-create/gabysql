# 📝 Changelog

> **Historial de cambios relevantes aplicados al producto y a su base documental.**

---

## 2026-05-07 — Decimosexta intervención: gateway MCP — `gabysql-mcp` (apertura Fase 5 AI-native)

> **Sin bump de formato. Sin cambios al motor.** Esta intervención añade un binario nuevo (`gabysql-mcp`) que es cliente del `gabysql-server` HTTP/JSON existente. No abre el `.db`, no instancia un `Pager`, no toca `storage.rs` / `bptree.rs` / `catalog.rs` / `sql.rs`. El motor queda intacto. Justificación completa: [ADR-0010](docs/adr/0010-mcp-gateway.md).

### ✨ Cambio

- Nuevo binario `src/bin/gabysql-mcp.rs` (~700 líneas, **cero dependencias externas**) que habla el protocolo **MCP (Model Context Protocol)** sobre stdio (JSON-RPC 2.0 delimitado por `\n`).
- Cinco tools expuestas a cualquier cliente MCP-compatible (Claude Desktop, Claude Code, Cursor, etc.):
  - `gabysql_list_databases` → wrap de `GET /dbs`
  - `gabysql_describe_database` → wrap de `GET /tables[?db=…]`
  - `gabysql_query` → wrap de `POST /exec` para `SELECT`/`SHOW`/`DESCRIBE`
  - `gabysql_execute` → wrap de `POST /exec` para `INSERT`/`UPDATE`/`DELETE`/DDL (omitida si se lanza con `--read-only`)
  - `gabysql_integrity_check` → wrap de `POST /exec` con `INTEGRITY CHECK`
- Dos resources MCP:
  - `gabysql://catalog` → lista de bases disponibles
  - `gabysql://schema/{db}` → schema completo de una DB
- Flags: `--server URL` (default `http://127.0.0.1:7878`, también `GABYSQL_SERVER`), `--token T` (también `GABYSQL_TOKEN`), `--read-only`.
- Tests unitarios en el mismo archivo cubren: parser JSON (round-trip + escapes), `initialize`, `tools/list` con y sin `--read-only`, `resources/list`, `ping`, método desconocido, notifications sin id, parsing de URL del server.

### 🎯 Por qué este cambio

El consumidor que más rápido crece en el ecosistema es el agente LLM. Hoy una IA que quiera usar `gabysql` necesita: cliente HTTP a mano + token + el schema de la DB metido en el prompt + reintentos sobre errores SQL sin trazabilidad. Ese pegamento se reescribe en cada integración.

MCP es el estándar emergente que define cómo un servidor expone *tools* y *resources* a clientes-agentes. Si `gabysql` lo habla de fábrica, cualquier agente lo enchufa directo:

```bash
gabysql-server -dir ./dbs -token MI_TOKEN
gabysql-mcp --server http://127.0.0.1:7878 --token MI_TOKEN
# Claude Desktop / Claude Code / Cursor lanzan gabysql-mcp como subprocess
# y descubren las 5 tools + 2 resources sin código de pegamento.
```

### 🛡️ Cómo se respeta el motor

- **No se toca `Cargo.toml`.** El binario se auto-descubre desde `src/bin/`. `Cargo.lock` no añade un solo paquete.
- **No se cambia `[lib]`.** Sigue compilando con cero deps externas. [ADR-0001](docs/adr/0001-rust-zero-deps-core.md) intacto.
- **No se abre el `.db`.** El gateway hace doble salto stdio→HTTP→Pager, así heredas todo lo que ya está endurecido en `server.rs`: `write_lock` global, tope de conexiones, bearer token, CORS preflight, validación de SQL antes de pegar al Pager.
- **No se cambia el formato en disco.** Sin bump de VERSION, sin nuevo tipo de página, sin cambio en el WAL.

### ✅ Tests
- Módulo `#[cfg(test)] mod tests` en `src/bin/gabysql-mcp.rs`: 9 tests cubren parser JSON, dispatch JSON-RPC y semántica de `--read-only`. CI multi-OS los ejecuta vía `cargo test`.

### 📐 ADR
- [ADR-0010 — Gateway MCP como adaptador externo sobre el HTTP/JSON existente](docs/adr/0010-mcp-gateway.md): promovida de 🟡 Propuesta a ✅ Aceptada con la implementación.

---

## 2026-05-08 — Decimoquinta intervención: `PageCache` LRU acotado — cierra fuga de memoria del server

> **Sin bump de formato.** El cambio es interno al Pager. La API pública del Pager se mantiene compatible salvo dos métodos nuevos (`set_cache_capacity`, `cache_len`, `cache_capacity`).

### ✨ Cambio
- Reemplazo de `cache: BTreeMap<u32, CachedPage>` (que crecía sin límite) por `cache: PageCache` con **capacidad fija** + **eviction LRU sobre páginas clean**.
- Constante `DEFAULT_CACHE_PAGES = 1024` (~4 MB por DB con páginas de 4 KB). Configurable por instancia con `Pager::set_cache_capacity(n)`.
- LRU implementada con `HashMap<u32, CacheSlot>` + contador monótono (touch en cada `get/get_mut/insert`). Eviction = scan O(N) sobre el map cuando está lleno; para 1024 entradas son µs por inserción.
- Política dirty-aware: **las páginas dirty nunca se evictan** — pertenecen a la transacción abierta y deben llegar al WAL antes de poder dropearse. Si el cache llega a capacidad lleno de dirty, se permite overflow temporal: perder una página dirty corromperia la DB. El overflow drena solo en el commit (todas pasan a clean simultáneamente).

### 🎯 Por qué este cambio

**Pre-bloque-10:**
```rust
struct Pager {
    cache: BTreeMap<u32, CachedPage>,  // ← crece sin freno
}
```
Un `INTEGRITY CHECK` o un `SELECT` con full scan sobre una DB de 200 MB cargaba ~50 K páginas en RAM y **nunca las liberaba**. En `gabysql-server -dir ./dbs` con 50 DBs activas y un sweep operacional periódico, la memoria del server crecía a 10 GB y eventualmente lo mataba el OOM killer. Sin error, sin warning, sin recovery — solo `kill` y reiniciar.

**Post-bloque-10:**
```rust
struct PageCache {
    capacity: usize,                       // bounded
    map: HashMap<u32, CacheSlot>,
    counter: u64,                          // monotonic for LRU
}
```
Memoria del server acotada por `cache_capacity × #DBs_abiertas × page_size`. Para 50 DBs × 1024 páginas × 4 KB = **200 MB max**, predecible, no swappea.

### 🛡️ Comportamiento bajo casos edge
- **Workload chico con cache vacío**: idéntico a antes (cache nunca se llena, no evicta nada).
- **Workload grande de read-only**: evicta clean pages LRU. La página menos usada se cae; si vuelve a pedirse, se relee de disco con CRC verificado (mismo path que cold load).
- **Mid-transaction con muchas writes**: dirty pages se acumulan; clean pages preexistentes se evictan primero. Si el commit se retrasa y entra más dirty que cap, el cache excede cap **temporalmente** (correctness > strict cap). Drena en commit.
- **Rollback**: `cache.clear()` libera todo (mismo path que antes).

### 🧪 Validación
- 39/39 tests de integración (1 nuevo: `page_cache_is_bounded_and_evicts_clean_pages` siembra 200 filas, abre con `set_cache_capacity(4)`, recorre cada página de la DB y asserta que `cache_len() <= 4`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 🔭 `Transaction` (Unit of Work) — pospuesto a bloque futuro
La recomendación original de bloque 10 incluía un objeto `Transaction` que reemplazara las 40+ aperturas de `Catalog::open(self.pager)` por una unit-of-work compartida con cache de `TableMeta`. Después de medir el impacto real:
- La fuga de memoria del cache es **inmediata** (problema agudo del server).
- La memoización de `TableMeta` es **marginal** (lookup hash + decode = µs; el ahorro existe pero no aparece en profiles de workloads reales).
- El refactor de 40 sitios cuesta ~1500 líneas y rompe muchos diffs en revisiones.

Decisión: **se entrega solo el `PageCache` LRU en este bloque**. El `Transaction` queda como propuesta independiente con su propio análisis cuando aparezca un workload que lo justifique (ej. INSERT masivo medido).

---

## 2026-05-08 — Decimocuarta intervención: `LeafCursor` (Iterator pattern) — Fase 2 paso 2

> **Sin bump de formato.** El cambio es estructural: cómo se leen los rows del B+Tree.

### ✨ Cambio
- Nuevo `bptree::LeafCursor<'a>` que implementa `Iterator<Item = DbResult<KeyValue>>` y carga páginas leaf **on-demand** vía la chain `next` del B+Tree.
- Constructores en `Tree`: `cursor_full(root)` (full scan en orden de PK) y `cursor_range(root, from, to)` (range scan inclusive en ambos extremos).
- Wrappers en `Catalog`: `scan_cursor(root)` y `range_cursor(root, from, to)` para el caller del SQL layer.
- `exec_select` reescrito: cuando NO hay `ORDER BY`, los planes `FullScan` y `Range` consumen el cursor con `.skip(offset).take(limit)` en vez de materializar todo el B+Tree. Cuando hay `ORDER BY`, sigue materializando (necesita ordenar antes de window).

### 🎯 Impacto medible en recursos
- `SELECT … LIMIT N` sobre tabla de N filas pasa de O(filas_totales) memoria + IO a **O(N + offset)** memoria + IO. Verificable: el test `cursor_limit_returns_only_requested_rows` sobre 1.000 filas valida que `LIMIT 5` devuelve solo 5 PKs en orden, sin intermediarios.
- `SELECT … WHERE pk BETWEEN a AND b LIMIT N` corta el walk apenas la PK supera `b`, sin tocar páginas ulteriores.
- `Plan::ByPks` (path de índice secundario) sigue materializando — está acotado por la cardinalidad del lookup, no por el tamaño de la tabla.

### 🛡️ Borrow semantics (intencionales)
El cursor toma `&mut Pager` por su lifetime. Mientras está vivo, ninguna otra escritura puede pasar por el mismo Pager. Eso es lo correcto para SELECT (read-only) y por eso solo lo usa `exec_select`. Los call sites que necesitan leer Y mutar el mismo B+Tree (`CREATE INDEX` backfill, `INTEGRITY CHECK`, `delete_with_cascade`) siguen usando los helpers materializadores (`scan / range / all`); ahí la materialización es correcta porque la lectura tiene que terminar antes que la escritura empiece.

### 🧪 Validación
- 38/38 tests de integración (1 nuevo: `cursor_limit_returns_only_requested_rows` ejercita LIMIT/OFFSET y BETWEEN sobre 1.000 filas).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Decimotercera intervención: crash tests dirigidos (Fase 1 reabierta y cerrada del todo)

> **Sin bump de formato.** Solo nuevos tests de integración que ejercitan el path WAL→file con escenarios de crash sintéticos.

### 🧪 Crash recovery scenarios cubiertos
Los tests no matan procesos — sintetizan en disco el estado que un `kill -9` dejaría en cada momento crítico del flujo de `Pager::commit`:

1. **`crash_recovery_partial_file_restored_from_wal`** — kill después del WAL flush + COMMIT marker pero antes de tocar el data file. Trunca el data file al header y verifica que el reopen replica las páginas del WAL y el `SELECT` devuelve los datos completos.
2. **`crash_recovery_wal_without_commit_is_ignored`** — kill antes del COMMIT marker (transacción no durable). Forja un WAL con páginas pero sin marker; verifica que el reopen NO replica nada y los datos previos quedan intactos.
3. **`crash_recovery_replay_is_idempotent`** — kill durante los writes al data file con WAL ya flusheado. Re-planta el mismo WAL después de un replay exitoso y verifica que un segundo replay converge al mismo estado (no double-counting, no corrupción).

### 🎯 Cierre definitivo de Fase 1
Esto cubre el ítem "crash tests dirigidos (kill -9 entre WAL y file flush)" que quedaba pendiente en el [ROADMAP](../ROADMAP.md). Fase 1 (Robustez funcional) queda 100% entregada y demostrada con tests reproducibles.

### 🧪 Validación
- 37/37 tests de integración (3 nuevos).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Duodécima intervención: `ORDER BY` (Fase 2 paso 1)

> **Sin bump de formato.** Todo el ordering ocurre en memoria sobre el resultado del scan/range/index path.

### ✨ Funcionalidad SQL
- **`SELECT ... ORDER BY <col> [ASC|DESC]`**. ASC es el default cuando se omite la dirección. Va entre `WHERE` y `LIMIT/OFFSET`.
- Funciona sobre **cualquier columna** del schema (no requiere índice). Reusa el scan/range/index path existente y ordena el resultado en memoria.
- **NULLs sortean primero** bajo ASC (consistente con SQLite). En DESC quedan al final por reverse.
- Comparación tipada: INT/INT, FLOAT/FLOAT, mixto INT↔FLOAT (promueve a f64), BOOL (false<true), TEXT/DATE/DATETIME/JSON por byte order.

### 🧱 Cambios estructurales
- `SelectStmt.order_by: Option<OrderClause>` con `OrderClause { column, direction: OrderDir }`.
- Cuando `order_by` está set, el executor difiere `LIMIT/OFFSET` hasta después del sort para no truncar prematuramente.
- Nuevo helper `compare_values(Option<&Value>, Option<&Value>) -> Ordering` con NULL-first semantics.
- Validación pre-I/O: `ORDER BY` sobre columna inexistente devuelve error explícito.
- Reserved words extendidas: `order`, `by`, `asc`, `desc`.

### 🧪 Validación
- 34/34 tests de integración (4 nuevos: `order_by_int_asc_desc`, `order_by_text_with_limit_offset_window`, `order_by_nulls_sort_first_under_asc`, `order_by_unknown_column_rejected`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Undécima intervención: gabymodeler v2 (PowerDesigner-style) + CORS

> **Sin bump de formato.** El motor no cambia; el modeler reescrito y el server gana headers CORS para que el modeler pueda hablarle directo.

### 🌐 gabymodeler v2 (`web/modeler/`)
Reescritura completa del modelador, espejo del motor `VERSION 6`:
- **Layout PowerDesigner-style**: header de toolbar + Object Browser izquierdo (árbol DB > Tables > columnas con badges PK/NN/UN/FK + sección Indexes) + Canvas central + Result List inferior colapsable + Status bar.
- **Schema editor**: cada columna lleva flags inline `PK / NN / UN / FK` y un input `default` editable. PK fuerza INT + NOT NULL automáticamente. FK abre un mini-modal para elegir tabla, columna PK del target y `ON DELETE RESTRICT|CASCADE`.
- **Check Model** continuo (14 reglas): PK ausente / duplicada / no INT, columna duplicada, identificador inválido o reservado (espejo de `catalog::RESERVED_WORDS`), `NOT NULL + DEFAULT NULL`, `DEFAULT` sobre PK, UNIQUE sobre JSON, FK rota / con type mismatch / target no-PK, etc. Cada hallazgo es clickeable y selecciona la entidad/columna en canvas + browser.
- **SQL Preview en vivo** (sin abrir modal). El emit ordena tablas topológicamente (parents antes que children) y emite todas las constraints inline (`PRIMARY KEY`, `NOT NULL`, `UNIQUE`, `DEFAULT <literal>`, `REFERENCES ... ON DELETE ...`) — DDL fiel al motor `VERSION 6`.
- **↘ Importar de gabysql**: dialog que pide URL del server, token opcional y nombre de DB; consume `GET /tables?db=<db>` y reconstruye entidades + columnas + constraints + FKs desde la respuesta enriquecida del bloque 3. Reverse engineering one-shot.
- **Migración v1 → v2 automática**: si encuentra `gabymodeler.v1` en localStorage, lo lee y produce un `gabymodeler.v2` con las constraints en blanco (los flags se editan a mano).
- **FK lines**: SVG Bezier con marker arrow; `CASCADE` se dibuja sólida, `RESTRICT` punteada.

### 🔓 CORS en `gabysql-server`
- Toda respuesta lleva `Access-Control-Allow-Origin: *`, `Access-Control-Allow-Methods: GET, POST, OPTIONS`, `Access-Control-Allow-Headers: Authorization, Content-Type, X-Gabysql-Token` y `Access-Control-Max-Age: 600`.
- El método `OPTIONS` se contesta con `204 No Content` antes de cualquier auth — los preflights del navegador no llevan credenciales y rechazarlos rompería el modeler en cross-origin.
- También se agregaron `204 No Content` y `503 Service Unavailable` al mapa de status text del response writer.

### 🧪 Validación
- 30/30 tests de integración siguen verdes (no se agregaron tests de modeler — es UI vanilla).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 📋 web/modeler/README.md
Reescrito para el layout v2 y el flujo con reverse engineering.

---

## 2026-05-07 — Décima intervención: `INTEGRITY CHECK` (cierre de Fase 1)

> **Sin bump de formato.** El comando es de solo lectura — no toca el catálogo ni los datos.

### ✨ Funcionalidad SQL
- **`INTEGRITY CHECK;`** — barre la DB abierta y devuelve un ResultSet con una fila por hallazgo. Columnas: `kind`, `object`, `detail`. El campo `message` resume con `OK · N tablas · M filas · K índices · F FKs · P páginas` o `FAIL · ...` según el caso.

### 🔍 Qué chequea
1. **CRC de cada página**: itera de `0..page_count` haciendo `Pager::page_data`. Cualquier falla del CRC se reporta como `kind=page_corrupt`.
2. **Decodificación de cada fila**: `decode_row` corre sobre cada fila de cada tabla. Falla → `kind=row_decode`.
3. **Índices secundarios**: walks every bucket de cada índice y verifica que cada `(value_bytes, pk)` apunte a una PK que efectivamente existe en la tabla. Si no → `kind=orphan_index_entry`.
4. **FOREIGN KEYs**: para cada columna con `references`, verifica que el parent table exista (sino `fk_target_missing`) y que cada valor no nulo de la columna tenga su parent row (sino `fk_orphan`).

### 🧱 Cambios estructurales
- Nuevo `Statement::IntegrityCheck` y método `Engine::exec_integrity_check`.
- Reserved words extendidas: `integrity`, `check`.
- Sin cambios al on-disk format ni al catálogo.

### 🧪 Validación
- 30/30 tests de integración (2 nuevos: `integrity_check_clean_db_returns_ok`, `integrity_check_reports_corrupted_page`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 🎯 Cierre de Fase 1 (Robustez funcional)
Con este bloque, los 5 ítems de Fase 1 del [ROADMAP](../ROADMAP.md) están entregados:
- ~~`UPDATE`/`DELETE` por PK~~
- ~~Checksums por página + WAL~~
- ~~`NOT NULL` / `DEFAULT` / `UNIQUE`~~
- ~~`FOREIGN KEY` + `ON DELETE` enforced~~
- ~~`INTEGRITY CHECK` operacional~~

El motor está listo para empezar a sumar features de Fase 2 (índices compuestos, range scan secundario, `ORDER BY`) o para una primera publicación con SLAs de durabilidad medibles.

---

## 2026-05-07 — Novena intervención: FOREIGN KEY enforced (Camino A · paso 5)

> **On-disk format jump: VERSION 5 → 6.** `Column` ahora persiste un FK opcional `(target_table, target_column, on_delete)`. DBs v5 son rechazadas explícitamente al abrir.

### ✨ Funcionalidad SQL
- **`REFERENCES <table>(<column>) [ON DELETE RESTRICT|CASCADE]`** como constraint de columna en `CREATE TABLE` y `ALTER TABLE ADD COLUMN`. Default `RESTRICT` cuando se omite `ON DELETE`.
- **Validación al DDL**: target table debe existir (o ser self-ref a la tabla siendo creada), target column debe ser la PK del target, tipos deben coincidir (en esta versión ambos son siempre `INT`).
- **Enforcement en `INSERT`**: cada FK no nula chequea que exista la fila parent. Self-FK que apunta al PK que se está insertando se acepta (caso CEO/manager-de-sí-mismo).
- **Enforcement en `UPDATE`**: solo se revalidan FKs cuyo valor cambió.
- **Enforcement en `DELETE`**:
  - `RESTRICT` (default) aborta el DELETE si existe alguna fila hija; sin efectos colaterales.
  - `CASCADE` borra las hijas iterativamente (worklist con `visited` set sobre `(tabla, pk)` para cortar ciclos), incluyendo sus entradas en índices secundarios.
- **Self-references** soportadas (`employee.manager_id REFERENCES employee(id)`).

### 🧱 Cambios estructurales
- `catalog::ForeignKeyMeta { table, column, on_delete: OnDelete }` con `OnDelete::{Restrict, Cascade}`.
- `Column.references: Option<ForeignKeyMeta>` persistido bajo flag `0x04 = HAS_FK`.
- `RESERVED_WORDS` extendido con `foreign`, `references`, `cascade`, `restrict`.
- Helpers nuevos en `sql.rs`: `validate_fk_targets`, `check_fk_value`, `enforce_fk_on_insert`, `enforce_fk_on_update`, `find_child_pks_with_fk_value`, `delete_with_cascade`.
- `find_child_pks_with_fk_value` usa el índice secundario sobre la columna FK si existe; cae en full scan si no — recomendación documentada de indexar columnas FK para DELETEs O(log n).
- `exec_delete` simplificado: chequea existencia y delega en `delete_with_cascade`, que maneja índices secundarios + cascade + cycle protection.

### 🌐 Endpoint `/schema` extendido
Cada columna ahora incluye `references: { table, column, onDelete } | null`:
```json
{
  "name": "parent_id", "type": "INT", "pk": false, "notNull": false, "unique": false,
  "hasDefault": false, "default": null,
  "references": { "table": "parent", "column": "id", "onDelete": "CASCADE" }
}
```

### 🛡️ Restricciones de la versión
- Solo FK de columna única (no compuestas).
- Target debe ser la PK del parent — `REFERENCES` contra `UNIQUE` no-PK no está soportado todavía.
- Solo `RESTRICT` y `CASCADE` (ni `SET NULL`, ni `SET DEFAULT`, ni `NO ACTION`).
- `ALTER TABLE ADD COLUMN ... REFERENCES ...` reusa los mismos guards que UNIQUE: si la columna es `NOT NULL` necesita un `DEFAULT` que apunte a un parent existente, etc.

### 🧪 Validación
- 28/28 tests de integración (6 nuevos: `fk_create_validation_rejects_bad_targets`, `fk_insert_update_enforcement`, `fk_self_reference_allows_pointing_at_self`, `fk_delete_restrict_blocks_when_children_exist`, `fk_delete_cascade_removes_children_and_grandchildren`, `old_v5_db_file_is_rejected_after_v6_bump`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

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
