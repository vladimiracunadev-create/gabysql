# 🧪 TECHNICAL SPECS

> **Formato en disco, WAL, tipos, límites y decisiones técnicas actuales del motor.**

---

## 🧬 Identidad del formato

| Campo | Valor |
|---|---|
| Magic | `GABYSQL1` |
| Versión de formato | `4` |
| Tamaño de página | `4096` bytes (fijo en esta versión) |
| Trailer de checksum por página | `4` bytes (CRC32-IEEE) |
| Hashing del catálogo y de claves de índice | FNV-1a-64 (estable entre versiones de Rust) |
| Tipos de página B+Tree | `LEAF` (1), `INTERNAL` (2) |

> Bumps de versión: `1` → `2` cambió el hash del catálogo de `DefaultHasher` a FNV-1a-64; `2` → `3` reservó el trailer CRC y agregó verificación en lectura/replay; `3` → `4` extendió `TableMeta` con la lista de índices secundarios. Las DBs de versiones anteriores son rechazadas explícitamente al abrir.

---

## 📦 Header de la página 0

| Offset | Significado |
|---|---|
| `0..7` | magic |
| `8..11` | versión `u32` little-endian (debe ser `4`) |
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

## 🗂️ Catálogo

Cada `TableMeta` contiene:
- nombre de tabla
- nombre de PK
- columnas y tipos
- página raíz de la tabla (root de su B+Tree)
- lista de `IndexMeta { name, column, root_page }` (vacía si la tabla no tiene índices secundarios)

El catálogo direcciona por **FNV-1a-64** del nombre normalizado (trim + lowercase). Una colisión de hash devuelve error explícito al abrir.

---

## 🔍 Índices secundarios

Un índice secundario es un B+Tree paralelo cuya **clave** es el FNV-1a-64 del valor de la columna y cuyo **valor** es un *bucket* (lista) de `(value_bytes, pk)`:

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
- Solo equality (`=`); sin range scan ni `BETWEEN` por índice secundario.
- Sin `UNIQUE` declarativo.
- `JSON` no es indexable.
- El nombre del índice es único en toda la base de datos.

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

- la PK debe ser una sola columna `INT` escalar (no se admiten PKs compuestas ni de otros tipos en esta versión)
- la PK no puede ser `NULL`
- una PK duplicada devuelve error en `INSERT`
- `UPDATE` no permite mutar la PK
- `UPDATE` y `DELETE` sobre una PK inexistente retornan error explícito
- columnas no presentes en `INSERT` quedan en `NULL` cuando aplica

---

## 🧠 Gramática SQL soportada

### Soportado
- `CREATE TABLE`
- `INSERT INTO ... VALUES (...)`
- `SELECT ... FROM ...`
- `WHERE <pk> = ...` (en `SELECT`, `UPDATE`, `DELETE`)
- `WHERE <col_indexada> = <valor>` (solo en `SELECT`)
- `WHERE <pk> BETWEEN ... AND ...` (solo en `SELECT`)
- `LIMIT` / `OFFSET`
- `UPDATE <tabla> SET col = val[, ...] WHERE <pk> = N`
- `DELETE FROM <tabla> WHERE <pk> = N`
- `CREATE INDEX <nombre> ON <tabla> (<columna>)` (con backfill automático)
- `DROP INDEX <nombre>`

### No soportado todavía
- `JOIN`
- `ORDER BY`
- `GROUP BY`
- `LIKE`
- `WHERE` por columnas no PK ni indexadas
- `WHERE` sobre columna indexada con operador distinto a `=` (no `BETWEEN`, no `<`/`>`)
- Índices compuestos
- Índices `UNIQUE` declarativos
- `UPDATE` / `DELETE` por columnas no PK ni por rango

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
- no hay índices secundarios
- no hay locking fuerte entre procesos
- no hay migraciones de formato en disco entre versiones mayores
- el cálculo de `total` en `/rows` requiere scan completo

---

## 🧠 Qué significa esto en producto

`gabysql` ya tiene una base sólida para aprender, demostrar y estabilizar storage/SQL básicos, pero todavía no tiene las capas de optimizer, concurrencia y compatibilidad histórica que definen un motor maduro.
