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

> Bumps de versión: `1` → `2` cambió el hash del catálogo de `DefaultHasher` a FNV-1a-64; `2` → `3` reservó el trailer CRC y agregó verificación en lectura/replay; `3` → `4` extendió `TableMeta` con la lista de índices secundarios; `4` → `5` agregó `NOT NULL` + `DEFAULT` por columna y el flag `unique` por índice; `5` → `6` agregó `FOREIGN KEY` opcional por columna (target table + target column + `ON DELETE` action); `6` → `7` agregó el campo `kind: IndexKind` (`Hash` | `OrderedInt`) a `IndexMeta` para habilitar índices ordenados sobre columnas `INT` con range scan O(log N + k) — ver [ADR-0017](adr/0017-int-ordered-index-version-7.md); `7` → `8` extendió `TableMeta.primary_key` y `IndexMeta.column` a múltiples columnas (PK e índices compuestos) — restringido a all-INT NOT NULL, equality lookup via fingerprint FNV-1a-64, ver [ADR-0019](adr/0019-composite-pk-and-index.md); `8` → `9` (L1) extendió `ForeignKeyMeta` con `on_delete` ampliado (`SET NULL` / `SET DEFAULT` / `NO ACTION`) y nuevo `on_update` con las cinco acciones — ver [ADR-0020](adr/0020-fk-referential-actions.md); `9` → `10` (L2) agregó `CHECK (expr)` column-level y table-level persistidas como texto canónico vía `format_expr` — ver [ADR-0021](adr/0021-check-constraints.md); `10` → `11` (residual #2) agregó nombres opcionales para PK/UNIQUE/FK/CHECK (`pk_name`, `fk_name`, etc.) habilitando `ALTER TABLE DROP CONSTRAINT` — ver [ADR-0022](adr/0022-named-constraints-and-drop.md); `11` → `12` (residual #3) agregó `extra_source_columns` + `extra_target_columns` a `ForeignKeyMeta` para FK multi-col `FOREIGN KEY (a, b) REFERENCES p (x, y)` con lookup O(log n) via fingerprint K2 — ver [ADR-0023](adr/0023-multi-col-foreign-key.md); `12` → `13` (bloque V) agregó un **discriminator byte por record del catalog** (`0x01 = table`, `0x02 = view`) y persiste `ViewMeta { name, source_sql, column_aliases }` para vistas lógicas — ver [ADR-0025](adr/0025-views.md); `13` → `14`/`15` (X3) agregó `ObjectKind::Procedure` con payload reescrito en el bump 15 — ver [ADR-0031](adr/0031-stored-procedures-x3.md); `15` → `16` (X3b) agregó `ObjectKind::Function` — ver [ADR-0032](adr/0032-user-functions-x3b.md); `16` → `17` (Y) tipos `TIME` y `UUID`; `17` → `18` (Y2) `max_length: Option<u32>` para enforcement `VARCHAR(n)`/`CHAR(n)` — ver [ADR-0040](adr/0040-varchar-length-enforcement-y2.md); `18` → `19` (Y3) `int_width: Option<u8>` para enforcement TINY/SMALL/MEDIUM/INT4 — ver [ADR-0043](adr/0043-int-range-enforcement-y3.md); `19` → `20` (Y4) `ColumnType::Blob` con literal `X'hex'` — ver [ADR-0044](adr/0044-blob-bytea-y4.md); `20` → `21` (Y5) high bit `0x80` en `int_width` = `UNSIGNED` — ver [ADR-0045](adr/0045-unsigned-and-uuid-y5.md); `21` → `22` (Y6) `ColumnType::Decimal` exacto (i128 + scale por fila + `(precision, scale)` por columna) — ver [ADR-0046](adr/0046-decimal-exact-y6.md); `22` → `23` (Z1) `ObjectKind::User` + `ObjectKind::Role` — ver [ADR-0050](adr/0050-z1-users-roles.md); `23` → `24` (Z2) `ObjectKind::Grant` (bitmask de privs por `(grantee, object)`) — ver [ADR-0051](adr/0051-z2-grant-revoke.md); `24` → `25` (Z3) `ObjectKind::Policy` con `USING (expr)` para RLS — ver [ADR-0052](adr/0052-z3-row-level-security.md); `25` → `26` (Z1b) reescritura de `UserMeta` con scheme byte + salt + hash + iterations PBKDF2-SHA256 — ver [ADR-0053](adr/0053-z1b-pbkdf2-password-hashing.md); `26` → `27` (Z3b) `PolicyMeta` agrega `with_check_sql` + acepta `FOR INSERT` — ver [ADR-0054](adr/0054-z3b-with-check-rls.md); `27` → `28` (Z1c) scheme=2 scrypt RFC 7914 — ver [ADR-0056](adr/0056-z1c-scrypt-password-hashing.md); `28` → `29` (Z1d) Blake2b RFC 7693 como cimiento — ver [ADR-0058](adr/0058-z1d-blake2b-foundation.md); `29` → `30` (Z1e) Argon2id scheme=3 estructural — ver [ADR-0060](adr/0060-z1e-argon2id-structural.md); `30` → `31` (Z1f) corte semántico tras el partial fix de Argon2id — ver [ADR-0061](adr/0061-z1f-argon2id-partial-fix.md). Las DBs de versiones anteriores son rechazadas explícitamente al abrir con `[GBY-1003]`. Los bloques **P1+P2+P3** (Fase 3, 2026-05-29) son **zero-bump on-disk** — las stats de `ANALYZE` son session-only en `Engine.table_stats: HashMap`.

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
- `COUNT(DISTINCT col)` sobre SELECT con JOIN sigue `[GBY-4028]`. El resto de los agregados sobre JOIN funcionan desde F2 (2026-05-30, ADR-0066 Gap 1+7); single-table desde F.
- `GROUP_CONCAT` / `STRING_AGG` / `JSON_AGG` / `ARRAY_AGG`
- ~~Window functions, CTE (`WITH ... AS`), `WITH RECURSIVE` (bloque W)~~ ✅ entregados: W1 (CTE no-rec) + W2 (recursivas) + W3 (window fns) + W4 (O(n) per partition, 2026-05-30) + E5 (anchor bare-SELECT)
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
