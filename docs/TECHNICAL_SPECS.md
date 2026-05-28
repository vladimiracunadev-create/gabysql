# 🧪 TECHNICAL SPECS

> **Formato en disco, WAL, tipos, límites y decisiones técnicas actuales del motor.**

---

## 🧬 Identidad del formato

| Campo | Valor |
|---|---|
| Magic | `GABYSQL1` |
| Versión de formato | `13` |
| Tamaño de página | `4096` bytes (fijo en esta versión) |
| Trailer de checksum por página | `4` bytes (CRC32-IEEE) |
| Hashing del catálogo y de claves de índice | FNV-1a-64 (estable entre versiones de Rust) |
| Tipos de página B+Tree | `LEAF` (1), `INTERNAL` (2) |

> Bumps de versión: `1` → `2` cambió el hash del catálogo de `DefaultHasher` a FNV-1a-64; `2` → `3` reservó el trailer CRC y agregó verificación en lectura/replay; `3` → `4` extendió `TableMeta` con la lista de índices secundarios; `4` → `5` agregó `NOT NULL` + `DEFAULT` por columna y el flag `unique` por índice; `5` → `6` agregó `FOREIGN KEY` opcional por columna (target table + target column + `ON DELETE` action); `6` → `7` agregó el campo `kind: IndexKind` (`Hash` | `OrderedInt`) a `IndexMeta` para habilitar índices ordenados sobre columnas `INT` con range scan O(log N + k) — ver [ADR-0017](adr/0017-int-ordered-index-version-7.md); `7` → `8` extendió `TableMeta.primary_key` y `IndexMeta.column` a múltiples columnas (PK e índices compuestos) — restringido a all-INT NOT NULL, equality lookup via fingerprint FNV-1a-64, ver [ADR-0019](adr/0019-composite-pk-and-index.md); `8` → `9` (L1) extendió `ForeignKeyMeta` con `on_delete` ampliado (`SET NULL` / `SET DEFAULT` / `NO ACTION`) y nuevo `on_update` con las cinco acciones — ver [ADR-0020](adr/0020-fk-referential-actions.md); `9` → `10` (L2) agregó `CHECK (expr)` column-level y table-level persistidas como texto canónico vía `format_expr` — ver [ADR-0021](adr/0021-check-constraints.md); `10` → `11` (residual #2) agregó nombres opcionales para PK/UNIQUE/FK/CHECK (`pk_name`, `fk_name`, etc.) habilitando `ALTER TABLE DROP CONSTRAINT` — ver [ADR-0022](adr/0022-named-constraints.md); `11` → `12` (residual #3) agregó `extra_source_columns` + `extra_target_columns` a `ForeignKeyMeta` para FK multi-col `FOREIGN KEY (a, b) REFERENCES p (x, y)` con lookup O(log n) via fingerprint K2 — ver [ADR-0023](adr/0023-multi-col-fk.md); `12` → `13` (bloque V) agregó un **discriminator byte por record del catalog** (`0x01 = table`, `0x02 = view`) y el persiste `ViewMeta { name, source_sql, column_aliases }` para vistas lógicas — ver [ADR-0025](adr/0025-logical-views.md). Las DBs de versiones anteriores son rechazadas explícitamente al abrir con `[GBY-1003]`.

---

## 📦 Header de la página 0

| Offset | Significado |
|---|---|
| `0..7` | magic |
| `8..11` | versión `u32` little-endian (debe ser `13`) |
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

Layout binario v13 por record del catalog: cada entry comienza con un **discriminator byte** (`0x01 = TableMeta`, `0x02 = ViewMeta`, bloque V/ADR-0025). Para tablas, el cuerpo es el `TableMeta` clásico extendido. Por columna: `[name][type_code:u8][flags:u8]` seguido del payload del default cuando `flags & 0x02`, y del payload del FK cuando `flags & 0x04`. Bits: `0x01 = NOT NULL`, `0x02 = HAS_DEFAULT`, `0x04 = HAS_FK`. Cada `TableMeta` además persiste `[pk_count:u8]` columnas PK (≥1; >1 implica PK compuesta all-INT NOT NULL, K2), un `pk_name: Option<String>` (residual #2/V11) y cada `IndexMeta` persiste `[extra_cols_count:u8]` columnas extra (>0 implica índice compuesto all-INT) + `name: Option<String>`.
- Default: `[kind:u8] + payload` (kinds: 0 Null, 1 Int, 2 Float, 3 Bool, 4 String).
- FK (V12+): `[target_table:string][target_column:string][on_delete:u8][on_update:u8][extra_source_count:u8][extra_source_cols...][extra_target_count:u8][extra_target_cols...][fk_name:opt_string]`. Acciones (V9+): `0 = RESTRICT`, `1 = CASCADE`, `2 = SET NULL`, `3 = SET DEFAULT`, `4 = NO ACTION`. `extra_*_count > 0` indica FK multi-col (residual #3/V12).
- CHECK trailer (V10+): por `TableMeta` se persiste `[check_count:u16] + check_count × ([name:opt_string][source_sql:string])`; el cuerpo se reparsea a `Expr` en cada open via `parse_expr_str` (L2/ADR-0021).
- ViewMeta (V13+): `[discriminator:0x02][name:string][source_sql:string][col_aliases_count:u16][aliases...]`.

El catálogo direcciona por **FNV-1a-64** del nombre normalizado (trim + lowercase). Una colisión de hash devuelve error explícito al abrir.

---

## 🔍 Índices secundarios

Desde VERSION 7 cada `IndexMeta` lleva un campo `kind: IndexKind` (Hash | OrderedInt). Desde VERSION 8 (K2) además admite columnas extra para índices compuestos (`extra_columns: Vec<String>`, vacío para single-column):

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
- Single-column en cualquier tipo escalar; índices compuestos soportados desde K2 (VERSION 8) **restringidos a all-INT NOT NULL**, equality-only via fingerprint FNV-1a-64 (no range scan, no mezcla de tipos).
- `JSON` no es indexable.
- El nombre del índice es único en toda la base de datos.
- Equality (`=`) sobre cualquier columna indexada. **`BETWEEN` solo sobre columnas `INT` con índice `OrderedInt`** (default automático al crear índice single-column sobre `INT`); `BETWEEN` sobre `TEXT`/`FLOAT`/`BOOL`/`DATE`/`DATETIME` indexados, y sobre cualquier índice compuesto, devuelve error claro.

Modo `UNIQUE`:
- `CREATE UNIQUE INDEX` o constraint inline `column UNIQUE` setean `IndexMeta.unique = true`.
- En `INSERT`/`UPDATE` se hace pre-check (`bucket_unique_conflict`) antes de tocar disco; si el valor ya está en la tabla con otra PK, se rechaza con error explícito (no se persiste nada).
- Múltiples `NULL` se permiten (consistente con SQL estándar; `NULL` no es igual a `NULL` para uniqueness).
- En `CREATE UNIQUE INDEX` el backfill aborta apenas detecta el primer duplicado, sin publicar el índice en el catálogo.

---

## 🔗 FOREIGN KEY (VERSION 6+)

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
- la PK puede ser una sola columna `INT` escalar o un grupo compuesto `(a, b, ...)` all-INT NOT NULL declarado table-level (K2, VERSION 8); en cualquier caso es implícitamente `NOT NULL`
- la PK no puede ser `NULL`
- una PK duplicada devuelve error en `INSERT`
- `UPDATE` regular permite mutar la PK desde residual #4 (re-encode + move de la fila + cascade `ON UPDATE` con todas las acciones); `UPSERT DO UPDATE` sigue rechazándolo (`[GBY-4008]`)
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
- `CREATE TABLE` con constraints inline `PRIMARY KEY` / `NOT NULL` / `UNIQUE` / `DEFAULT <literal>` / `CHECK (expr)` / `REFERENCES <tabla>(<col>) [ON DELETE <action>] [ON UPDATE <action>]` con `<action>` ∈ `RESTRICT | CASCADE | SET NULL | SET DEFAULT | NO ACTION` (L1/V9 + residual #4 que activó `ON UPDATE`). Constraints table-level: `PRIMARY KEY (a, b, ...)` (K2, all-INT NOT NULL), `UNIQUE (a, b, ...)`, `CHECK (expr)` (L2), `FOREIGN KEY (a, b) REFERENCES p (x, y) [ON DELETE...] [ON UPDATE...]` (residual #3/V12). Nombres opcionales con `CONSTRAINT <name>` para PK/UNIQUE/FK/CHECK (residual #2/V11).
- `CREATE TABLE [IF NOT EXISTS] [(col_aliases)] AS <select_query>` (CTAS, K1)
- `DROP TABLE [IF EXISTS] <name>` (catalog-only; páginas backing no liberadas)
- `ALTER TABLE <name> ADD [COLUMN] <coldef>` (sin reescritura de filas previas)
- `ALTER TABLE <name> DROP COLUMN [IF EXISTS] <col>` (K1; bloqueado sobre PK / indexada / FK)
- `ALTER TABLE <name> RENAME COLUMN <old> TO <new>` (K1; arrastra PK + índices + FKs entrantes)
- `ALTER TABLE <name> RENAME TO <new>` / `RENAME TABLE <old> TO <new>` (K1)
- `ALTER TABLE <name> ADD [CONSTRAINT <cname>] CHECK (<expr>)` con re-validación O(n) full-scan (L3)
- `ALTER TABLE <name> DROP CONSTRAINT [IF EXISTS] <cname>` (residual #2/V11; resuelve CHECK / UNIQUE / FK por nombre; PK rechazada con `[GBY-4072]`, no encontrado con `[GBY-4071]`)
- `CREATE VIEW [IF NOT EXISTS] <name> [(col_aliases)] AS <select>` y `DROP VIEW [IF EXISTS] <name>` (bloque V/V13; read-only, source debe ser SELECT simple, cycle guard `MAX_VIEW_DEPTH = 32`)
- `INSERT INTO ... VALUES (...)`
- `SELECT ... FROM ... [WHERE ...] [ORDER BY ...] [LIMIT n] [OFFSET n]`
- `WHERE` con la siguiente gramática (idéntica en `SELECT`, `UPDATE`, `DELETE` desde el bloque E3, extendida por G2/G3/H):
  - Operadores atómicos: `=`, `<`, `>`, `<=`, `>=`, `<>`/`!=`, `BETWEEN n AND m`, `IS [NOT] NULL`, `[NOT] LIKE 'patron'` (wildcards `%`/`_` + escape `\`), `[NOT] IN (lit, ...)`, `IN (SELECT ...)`, `NOT IN (SELECT ...)` (H, 3VL ANSI estricta), `= (SELECT ...)`, `[NOT] EXISTS (SELECT ...)` (correlated multi-pred OK desde H).
  - Postfix sobre cualquier `Expr` (G3): `IS [NOT] NULL`, `[NOT] LIKE`, `[NOT] IN`, `[NOT] BETWEEN`.
  - Expresiones escalares (G1+G2+G3) como LHS/RHS: 27 funciones (string, numéricas, fecha/hora), `CAST(x AS TYPE)`, `CASE WHEN ... THEN ... ELSE ... END` (searched + simple), `COALESCE`/`NULLIF`/`IFNULL`/`IF`/`IIF`, aritméticos binarios `+`/`-`/`*`/`/`/`%`, concat `||`.
  - Combinadores: `AND`, `OR`, `NOT`, paréntesis. Precedencia estándar SQL (`OR` < `AND` < `NOT` < átomo).
  - Lógica trivaluada (3VL) ANSI para NULL en todos los operadores (con la única excepción de `IS [NOT] NULL`).
  - Fast-paths indexadas activas solo cuando el WHERE es un único átomo del tipo `=` (PK o índice), `BETWEEN` (PK o `OrderedInt`), `IN (SELECT)` (PK o índice), `= (SELECT)` (PK o índice), `EXISTS`. Cualquier otra forma (combinadores, átomos E2, expresiones escalares, postfix Expr) cae a FullScan + filtro 3VL.
- `ORDER BY <col> [ASC|DESC]` (sort post-scan o por índice OrderedInt)
- `FROM a [AS x] [INNER|LEFT|RIGHT|FULL [OUTER]|CROSS] JOIN b [AS y] (ON l = r | USING (col))` y la comma-syntax
- `FROM a NATURAL [INNER|LEFT|RIGHT|FULL] JOIN b`
- Multi-tabla en cadena left-deep + self-join vía aliases
- Index-loop join optimization (transparente: aplica auto cuando ON pega contra PK/índice del right e INNER/LEFT)
- `LIMIT` / `OFFSET`
- `UPDATE <tabla> SET col = val[, ...] WHERE <where_clause>` (cualquier WHERE válido en SELECT; multi-fila)
- `DELETE FROM <tabla> WHERE <where_clause>` (cualquier WHERE válido en SELECT; cascade FK por fila)
- `CREATE INDEX <nombre> ON <tabla> (<columna>)` (con backfill automático)
- `CREATE INDEX <nombre> ON <tabla> (a, b, ...)` (compuesto, K2; all-INT, equality-only via fingerprint FNV-1a-64)
- `CREATE UNIQUE INDEX <nombre> ON <tabla> (<columna>)` o `(a, b, ...)` (backfill aborta en duplicados)
- `DROP INDEX <nombre>`
- `BEGIN` / `START TRANSACTION` / `COMMIT` / `END` / `ROLLBACK` (batch-local, T)
- Multi-row `INSERT INTO t VALUES (...), (...)`, `INSERT INTO t SELECT ...`, `TRUNCATE [TABLE]` (J)
- `INSERT ... ON CONFLICT [(col)] DO NOTHING | DO UPDATE SET col = literal`, `REPLACE INTO`, `RETURNING *` o `RETURNING col, ...` en `INSERT`/`UPDATE`/`DELETE` (J2)
- `FROM (SELECT ...) AS sub` derived tables (H, alias obligatorio); `SELECT (SELECT MAX(x) FROM t) FROM s` scalar subquery en SELECT list (H, correlated OK)
- Set ops `UNION` / `UNION ALL` / `INTERSECT [ALL]` / `EXCEPT [ALL]` / `MINUS` con precedencia ANSI (INTERSECT > UNION/EXCEPT) y `ORDER BY`/`LIMIT`/`OFFSET` al nivel del resultado combinado (I)
- `VALUES (a, b), (c, d), ...` standalone o `FROM (VALUES ...) AS t(c1, c2, ...)` con alias obligatorio (I)
- `INTEGRITY CHECK` (sweep operacional de páginas + índices + FKs)

### No soportado todavía
- Agregados sobre `SELECT` con `JOIN` (devuelve `[GBY-4028]`). `GROUP BY`/`HAVING`/`COUNT`/`SUM`/`AVG`/`MIN`/`MAX`/`DISTINCT`/`COUNT(DISTINCT)` single-table sí están soportados desde el bloque F
- `GROUP_CONCAT` / `STRING_AGG` / `JSON_AGG` / `ARRAY_AGG`
- Window functions, CTE (`WITH ... AS`), `WITH RECURSIVE` (bloque W)
- `ILIKE`, `REGEXP`, `GLOB`, `IS TRUE`/`IS FALSE`
- `WHERE` por columnas no PK ni indexadas usa FullScan (no es bloqueante — solo perf)
- Optimización indexada para operadores no-`=`/no-`BETWEEN` (`<`, `>`, `LIKE`, `IN literal`) — hoy cae a FullScan aunque la columna tenga índice
- Subqueries `ALL` / `ANY` / `SOME`, correlated `col = outer.col` puro fuera de `EXISTS`, `LATERAL`
- `JOIN` con predicados no-equi en `ON` (`<`, `>`, multi-cond con `AND`), `USING` multi-columna, `NATURAL` con >1 columna común
- PK / índices compuestos con columnas no-INT o nullables (K2 sólo all-INT NOT NULL); range scan sobre claves compuestas; partial indexes; `ALTER COLUMN TYPE`; ALTER PK sobre tabla existente
- `UPDATE ... FROM otra_tabla` (UPDATE con JOIN), `DELETE ... JOIN`, `EXCLUDED.col` en UPSERT (UPSERT DO UPDATE sigue rechazando mutar la PK; el UPDATE regular sí la permite desde residual #4)
- Subqueries dentro de `CHECK (expr)` — rechazadas con `[GBY-4069]`
- Unary `-` / `+` prefix sobre expresiones; `SAVEPOINT` / isolation levels / read-only tx / cross-request tx

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
