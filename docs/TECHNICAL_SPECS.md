# 🧪 TECHNICAL SPECS

> **Formato en disco, WAL, tipos, límites y decisiones técnicas actuales del motor.**

---

## 🧬 Identidad del formato

| Campo | Valor |
|---|---|
| Magic | `GABYSQL1` |
| Versión de formato | `7` |
| Tamaño de página | `4096` bytes (fijo en esta versión) |
| Trailer de checksum por página | `4` bytes (CRC32-IEEE) |
| Hashing del catálogo y de claves de índice | FNV-1a-64 (estable entre versiones de Rust) |
| Tipos de página B+Tree | `LEAF` (1), `INTERNAL` (2) |

> Bumps de versión: `1` → `2` cambió el hash del catálogo de `DefaultHasher` a FNV-1a-64; `2` → `3` reservó el trailer CRC y agregó verificación en lectura/replay; `3` → `4` extendió `TableMeta` con la lista de índices secundarios; `4` → `5` agregó `NOT NULL` + `DEFAULT` por columna y el flag `unique` por índice; `5` → `6` agregó `FOREIGN KEY` opcional por columna (target table + target column + `ON DELETE` action); `6` → `7` agregó el campo `kind: IndexKind` (`Hash` | `OrderedInt`) a `IndexMeta` para habilitar índices ordenados sobre columnas `INT` con range scan O(log N + k) — ver [ADR-0017](adr/0017-int-ordered-index-version-7.md). Las DBs de versiones anteriores son rechazadas explícitamente al abrir.

---

## 📦 Header de la página 0

| Offset | Significado |
|---|---|
| `0..7` | magic |
| `8..11` | versión `u32` little-endian (debe ser `7`) |
| `12..13` | page size `u16` little-endian (debe ser `4096`) |
| `16..19` | page count `u32` little-endian |
| `20..23` | `catalog_root_page` `u32` little-endian |
| `last 4` | CRC32-IEEE del resto de la página |

---

## 💾 Modelo de persistencia

- El archivo `.db` guarda header, catálogo y páginas del índice principal.
- Cada tabla mantiene una página raíz que es el root de su B+Tree.
- El catálogo guarda `TableMeta` serializado para cada tabla, indexado por `FNV-1a-64(nombre normalizado)`.
- Cada página persistida en disco lleva su CRC32 en los últimos 4 bytes; los encoders del B+Tree y del header reservan ese espacio.

---

## ♻️ WAL

Formato actual:
- record page: `[type=1][pageNo u32][len u32][bytes]`
- commit marker: `[type=2]`

> El payload `bytes` de cada record es la página completa de `len = page_size`, **incluido su trailer CRC**. Por tanto, el CRC de la página dentro del WAL hace de checksum del record: durante `replay_to` la verificación falla si el WAL fue truncado o flipped antes de aplicarse al `.db`.

### Regla de durabilidad
1. el Pager finaliza el CRC trailer de cada página dirty
2. se escriben after-images al WAL
3. se escribe `COMMIT`
4. se sincroniza el WAL
5. se aplican páginas al `.db`
6. se sincroniza el `.db`
7. se elimina el `.wal`

### Recovery
- si existe `.wal` con `COMMIT`, cada record se valida por CRC y se aplica al `.db`
- si algún record falla el CRC, el replay aborta con error explícito (no se reescribe la DB con datos corruptos)
- si no existe `COMMIT`, el WAL se descarta

---

## 🌿 Índice persistente actual

**B+Tree real** con dos tipos de página:

| Tipo | Layout |
|---|---|
| `LEAF` (1) | `[type:u8][next:u32][count:u16] + count × ([key:i64][vlen:u16][bytes])` |
| `INTERNAL` (2) | `[type:u8][reserved:u32][count:u16][first_child:u32] + count × ([key:i64][child:u32])` |

- Lookup desciende por `INTERNAL` → eventualmente cae en `LEAF` (O(log N)).
- Splits en cascada: si una hoja se llena, se divide y promueve la primera key de la mitad derecha al padre. Si el padre se llena, se promueve la mediana, etc.
- **Root estable**: cuando el root necesita splittear, su contenido se copia a una página nueva y el slot original se reescribe como `INTERNAL` con dos hijos. El número de página del root nunca cambia, por lo que el catálogo no necesita actualizarse.
- Full scan: descender al leftmost-leaf y seguir el chain `next`.
- Range scan (`BETWEEN`): descender al leaf que contiene `from`, recorrer hasta `to`.
- `delete` por PK: localizar la hoja, remover la entrada y reescribir la hoja. (Esta versión no rebalancea: las páginas pueden quedar parcialmente vacías; un futuro `vacuum` se hará cargo.)

---

## 🧠 Cache de páginas (Pager)

El Pager mantiene un `PageCache` con capacidad fija (default `DEFAULT_CACHE_PAGES = 1024`, configurable con `Pager::set_cache_capacity`). Política:

- Cada `get/get_mut/insert` bumpea un contador monótono y lo graba en el slot accedido (LRU bookkeeping O(1)).
- En `insert` con cache lleno: scan O(N) sobre `HashMap` para encontrar la página clean menos recientemente usada; se evicta.
- **Las páginas dirty nunca se evictan**: pertenecen a la transacción abierta y deben llegar al WAL antes de poder dropearse. Si el cache está lleno solo de dirty (edge case mid-tx con muchas writes), se permite overflow temporal — drena en commit cuando `mark_all_clean()` vuelve toda la cache evictable.

Implicancia para el server: memoria total acotada por `cache_capacity × #DBs_abiertas × page_size`. Default: 50 DBs × 1024 × 4 KB ≈ 200 MB. Predecible, no swappea, no OOM. Ver [ADR-0009](adr/0009-page-cache-lru-bounded.md).

---

## 🚶 Cursor lazy sobre B+Tree

`bptree::LeafCursor<'a>` implementa `Iterator<Item = DbResult<KeyValue>>`:

- Constructores: `Tree::cursor_full(root)` (full scan en orden de PK) y `Tree::cursor_range(root, from, to)` (range scan inclusive).
- Carga páginas leaf on-demand vía la chain `next` del B+Tree; cada `next()` avanza dentro del buffer actual o salta a la siguiente leaf.
- Combinable con `Iterator::skip(offset).take(limit)` — la stdlib short-circuita cuando el inner iterator agota Some, así que la página leaf N+1 ni se lee del disco.
- Borrow exclusivo: el cursor toma `&mut Pager` por su lifetime. SELECT (read-only) encaja; los call sites read+write (CREATE INDEX backfill, INTEGRITY CHECK, delete cascade) usan los helpers materializadores `Tree::scan / range / all`.

Garantía de complejidad para `SELECT … LIMIT N` sin ORDER BY: **O(N + offset)** páginas leídas, no O(filas_totales). Ver [ADR-0008](adr/0008-leaf-cursor-iterator.md).

Desde [ADR-0016](adr/0016-leafcursor-prefetch.md), `LeafCursor::load_current` hace `page_data` también sobre la siguiente hoja del chain — warm-ea la `PageCache` antes de que el caller la pida, sin allocations adicionales (la página queda en el cache LRU). Helper `Pager::cache_contains(page_no) -> bool` expuesto para introspección/tests.

---

## 🗂️ Catálogo

Cada `TableMeta` contiene:
- nombre de tabla
- nombre de PK
- columnas con `{ name, type, not_null, default?, references? }`
- página raíz de la tabla (root de su B+Tree)
- lista de `IndexMeta { name, column, root_page, unique, kind }` (vacía si la tabla no tiene índices secundarios). `kind: IndexKind` es `Hash` (default; el bucket vive en un B+Tree hash-keyed) u `OrderedInt` (clave física = valor `INT`; habilita `BETWEEN` por índice).

Layout binario v7 por columna: `[name][type_code:u8][flags:u8]` seguido del payload del default cuando `flags & 0x02`, y del payload del FK cuando `flags & 0x04`. Bits: `0x01 = NOT NULL`, `0x02 = HAS_DEFAULT`, `0x04 = HAS_FK`.
- Default: `[kind:u8] + payload` (kinds: 0 Null, 1 Int, 2 Float, 3 Bool, 4 String).
- FK: `[target_table:string][target_column:string][on_delete:u8]` con `0 = RESTRICT`, `1 = CASCADE`.

El catálogo direcciona por **FNV-1a-64** del nombre normalizado (trim + lowercase). Una colisión de hash devuelve error explícito al abrir.

---

## 🔍 Índices secundarios

Desde VERSION 7, cada `IndexMeta` lleva un campo `kind: IndexKind`:

- **`Hash`** (default; usado para `TEXT`/`FLOAT`/`BOOL`/`DATE`/`DATETIME` y para `INT` salvo override) — equality only.
- **`OrderedInt`** (solo aplica a columnas `INT`) — el B+Tree del índice usa el valor `INT` directamente como clave física, lo que habilita `WHERE col_int_idx BETWEEN a AND b` en O(log N + k). `NULL` no se almacena (consistente con la semántica SQL de `BETWEEN`/`UNIQUE`).

### Buckets (kind = Hash)

Un índice hash es un B+Tree paralelo cuya **clave** es el FNV-1a-64 del valor de la columna y cuyo **valor** es un *bucket* (lista) de `(value_bytes, pk)`:

| Campo | Encoding |
| :--- | :--- |
| Bucket | `[count:u16] + count × ([vlen:u16][value_bytes][pk:i64])` |
| `value_bytes` | Representación canónica del valor (`encode_column_value`): NULL = `[0]`, otros = `[1] + bytes_específicos_del_tipo` |

Operaciones:
- `CREATE INDEX`: aloca un root leaf; recorre todas las filas existentes y hace upsert en el bucket correspondiente; al final publica `IndexMeta` en el `TableMeta`.
- `INSERT`: para cada índice de la tabla, calcula `(value_bytes, pk)` e inserta en el bucket. Idempotente.
- `UPDATE`: si la columna afectada está indexada y el valor cambia, remueve `(old, pk)` del bucket viejo y agrega `(new, pk)` al bucket nuevo. Si la columna no está afectada, no toca el índice.
- `DELETE`: lee la fila antes de borrarla; remueve `(value, pk)` de cada índice. Si el bucket queda vacío, se elimina la entrada del B+Tree.
- `SELECT WHERE col = val` con `col` indexada: hash → bucket → filtra entradas cuyos `value_bytes` matchean exacto → hidrata filas por PK desde la tabla principal.
- `DROP INDEX`: remueve `IndexMeta` del `TableMeta`. **No libera páginas** — el reclaim es trabajo de un futuro `vacuum`.

Restricciones de la versión actual:
- Una columna por índice (no compuestos).
- `JSON` no es indexable.
- El nombre del índice es único en toda la base de datos.
- Equality (`=`) sobre cualquier columna indexada. **`BETWEEN` solo sobre columnas `INT` con índice `OrderedInt`** (default automático al crear índice sobre una columna `INT`); `BETWEEN` sobre `TEXT`/`FLOAT`/`BOOL`/`DATE`/`DATETIME` indexados devuelve error claro.

Modo `UNIQUE`:
- `CREATE UNIQUE INDEX` o constraint inline `column UNIQUE` setean `IndexMeta.unique = true`.
- En `INSERT`/`UPDATE` se hace pre-check (`bucket_unique_conflict`) antes de tocar disco; si el valor ya está en la tabla con otra PK, se rechaza con error explícito (no se persiste nada).
- Múltiples `NULL` se permiten (consistente con SQL estándar; `NULL` no es igual a `NULL` para uniqueness).
- En `CREATE UNIQUE INDEX` el backfill aborta apenas detecta el primer duplicado, sin publicar el índice en el catálogo.

---

## 🔗 FOREIGN KEY (VERSION 7+)

Cada columna puede declarar como mucho una FK single-column. Se persiste en `Column.references = Some(ForeignKeyMeta { table, column, on_delete })`.

Reglas de validación al DDL (CREATE TABLE / ALTER ADD COLUMN):
- Target table debe existir (o ser self-ref a la tabla siendo creada).
- Target column debe ser la PK del target table (no se admite REFERENCES contra UNIQUE no-PK en esta versión).
- Tipo de la columna FK debe matchear el tipo de la PK del target (hoy ambos son siempre INT).

Enforcement en runtime:
- `INSERT`: para cada FK no nula, lookup parent.get_row(value); error si no existe. Self-FK que apunta a su propia PK siendo insertada se acepta.
- `UPDATE`: solo se revalidan FKs cuyo valor cambió.
- `DELETE`:
  - `RESTRICT` (default): aborta el DELETE antes de cualquier write si existe alguna fila hija.
  - `CASCADE`: worklist iterativo. Por cada `(tabla, pk)` enqueada se enumeran las hijas, se aplican RESTRICT/CASCADE recursivamente, se borra la fila del B+Tree y se evictan sus entradas en cada índice secundario. `visited: HashSet<(table, pk)>` corta ciclos.
- Lookup de hijas: si la columna FK del hijo tiene índice secundario, lookup O(log n) por bucket; si no, full scan filtrando por valor. Recomendación: indexar siempre las columnas FK.

---

## 🧾 Tipos de columna

- `INT`
- `TEXT`
- `BOOL`
- `FLOAT`
- `DATE`
- `DATETIME`
- `JSON`

---

## 🧱 Reglas de fila

- todos los identificadores (tabla, columna, índice) cumplen `[A-Za-z_][A-Za-z0-9_]*`, longitud ≤ `MAX_IDENT_LEN = 64`, no reservados — definido y enforzado en [`catalog::validate_identifier`](../src/catalog.rs)
- la PK debe ser una sola columna `INT` escalar (no se admiten PKs compuestas ni de otros tipos en esta versión); es implícitamente `NOT NULL`
- la PK no puede ser `NULL`
- una PK duplicada devuelve error en `INSERT`
- `UPDATE` no permite mutar la PK
- `UPDATE` y `DELETE` sobre una PK inexistente retornan error explícito
- columnas no presentes en `INSERT` toman su `DEFAULT` si lo tienen; si no, quedan en `NULL`
- filas previas a un `ALTER TABLE ADD COLUMN` se decodifican con el `DEFAULT` de la columna nueva (o `NULL` si no tiene); se materializan en disco en el próximo `UPDATE`
- `NOT NULL`: rechazo en `INSERT` (columna ausente sin DEFAULT, o `NULL` literal) y en `UPDATE` (asignación a `NULL`)
- `DEFAULT NULL` y `NOT NULL` en la misma columna se rechazan en `CREATE TABLE`
- el literal de `DEFAULT` debe coincidir con el tipo de la columna (validado en `CREATE TABLE`)

---

## 🧠 Gramática SQL soportada

### Soportado
- `CREATE DATABASE [IF NOT EXISTS] <name>` *(server multi-DB / CLI; intercept antes de abrir Pager)*
- `DROP DATABASE [IF EXISTS] <name>`
- `SHOW DATABASES`
- `CREATE TABLE` con constraints inline `PRIMARY KEY` / `NOT NULL` / `UNIQUE` / `DEFAULT <literal>` / `REFERENCES <tabla>(<col>) [ON DELETE RESTRICT|CASCADE]`
- `DROP TABLE [IF EXISTS] <name>` (catalog-only; páginas backing no liberadas)
- `ALTER TABLE <name> ADD [COLUMN] <coldef>` (sin reescritura de filas previas)
- `INSERT INTO ... VALUES (...)`
- `SELECT ... FROM ... [WHERE ...] [ORDER BY ...] [LIMIT n] [OFFSET n]`
- `WHERE <pk> = ...` (en `SELECT`, `UPDATE`, `DELETE`)
- `WHERE <col_indexada> = <valor>` (solo en `SELECT`)
- `WHERE <pk> BETWEEN ... AND ...` (solo en `SELECT`)
- `WHERE <col_int_indexada> BETWEEN ... AND ...` (solo en `SELECT`; requiere índice `OrderedInt`, default para INT)
- `WHERE <col> IN (SELECT ...)` — subquery no-correlacionada single-column
- `WHERE <col> = (SELECT ...)` — subquery escalar no-correlacionada (1×≤1)
- `WHERE [NOT] EXISTS (SELECT ...)` — no-correlacionada y correlacionada single-eq (`inner = outer.col`)
- `ORDER BY <col> [ASC|DESC]` (sort post-scan o por índice OrderedInt)
- `FROM a [AS x] [INNER|LEFT|RIGHT|FULL [OUTER]|CROSS] JOIN b [AS y] (ON l = r | USING (col))` y la comma-syntax
- `FROM a NATURAL [INNER|LEFT|RIGHT|FULL] JOIN b`
- Multi-tabla en cadena left-deep + self-join vía aliases
- Index-loop join optimization (transparente: aplica auto cuando ON pega contra PK/índice del right e INNER/LEFT)
- `LIMIT` / `OFFSET`
- `UPDATE <tabla> SET col = val[, ...] WHERE <pk> = N`
- `DELETE FROM <tabla> WHERE <pk> = N`
- `CREATE INDEX <nombre> ON <tabla> (<columna>)` (con backfill automático)
- `CREATE UNIQUE INDEX <nombre> ON <tabla> (<columna>)` (backfill aborta en duplicados)
- `DROP INDEX <nombre>`
- `INTEGRITY CHECK` (sweep operacional de páginas + índices + FKs)

### No soportado todavía
- `GROUP BY`, agregaciones (`SUM`, `AVG`, `COUNT`, `MIN`, `MAX`)
- `LIKE` y operadores no-`=` (`<`, `>`, `<=`, `>=`, `<>`)
- `WHERE` con `AND` / `OR` / `NOT` en el outer (limitación de un solo predicado por WHERE)
- `WHERE` por columnas no PK ni indexadas (en SELECT single-table)
- `WHERE` sobre columna no-INT indexada con operador distinto a `=` (no `BETWEEN`, no `<`/`>`)
- Subqueries correlacionadas con múltiples predicados; derived tables (`FROM (SELECT ...) t`); CTE; window functions
- `JOIN` con predicados no-equi en `ON` (`<`, `>`, multi-cond con `AND`), `USING` multi-columna, `NATURAL` con >1 columna común
- Índices compuestos (multi-columna)
- `UPDATE` / `DELETE` por columnas no PK ni por rango ni con JOINs

---

## 🌐 Semántica HTTP

- mutex de proceso para escrituras
- modo single DB o multi DB
- token opcional por header
- techo de conexiones simultáneas (default `64`, configurable con `-max-connections`); las conexiones extra reciben `503 Service Unavailable`
- `limit` máximo de `1000` en `/rows`

---

## ⚠️ Limitaciones técnicas actuales

- no hay MVCC
- el locking cross-process es advisory (`File::try_lock`); previene apertura concurrente pero no es un sustituto de MVCC ni de un protocolo de replicación — ver [ADR-0013](adr/0013-process-level-file-lock.md)
- no hay migraciones de formato en disco entre versiones mayores
- el cálculo de `total` en `/rows` requiere scan completo

---

## 🧠 Qué significa esto en producto

`gabysql` ya tiene una base sólida para aprender, demostrar y estabilizar storage/SQL básicos, pero todavía no tiene las capas de optimizer, concurrencia y compatibilidad histórica que definen un motor maduro.
