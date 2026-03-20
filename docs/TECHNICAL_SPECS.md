# TECHNICAL SPECS

## Identidad del formato
- Magic: `GABYSQL1`
- Versión de formato: `1`
- Tamaño de página por defecto: `4096` bytes

## Header de la página 0
Offsets principales:
- `0..7`: magic
- `8..11`: versión `u32` little-endian
- `12..13`: page size `u16` little-endian
- `16..19`: page count `u32` little-endian
- `20..23`: `catalog_root_page` `u32` little-endian

## Modelo de persistencia
- El archivo `.db` guarda header, catálogo y páginas del índice principal.
- Cada tabla mantiene una página raíz apuntando a una cadena de hojas persistentes.
- El catálogo guarda `TableMeta` serializado para cada tabla.

## WAL
Formato actual:
- record page: `[type=1][pageNo u32][len u32][bytes]`
- commit marker: `[type=2]`

Regla de durabilidad:
1. se escriben after-images al WAL
2. se escribe `COMMIT`
3. se sincroniza el WAL
4. se aplican páginas al `.db`
5. se sincroniza el `.db`
6. se elimina el `.wal`

Recovery:
- si existe `.wal` con `COMMIT`, se reaplican páginas al `.db`
- si no existe `COMMIT`, el WAL se descarta

## Índice persistente actual
No es todavía un B+Tree multinivel completo.

Hoy el motor usa una estructura de hojas enlazadas:
- clave: `i64`
- valor: bytes serializados de fila
- split al llenarse una hoja
- scan por cadena enlazada
- búsqueda puntual por PK
- rango por `BETWEEN`

## Catálogo
Cada `TableMeta` contiene:
- nombre de tabla
- nombre de PK
- columnas y tipos
- página raíz de la tabla

El catálogo usa hash del nombre de tabla para direccionamiento y detecta colisiones al leer.

## Tipos de columna
- `INT`
- `TEXT`
- `BOOL`
- `FLOAT`
- `DATE`
- `DATETIME`
- `JSON`

## Reglas de fila
- la PK debe ser `INT`
- la PK no puede ser `NULL`
- una PK duplicada devuelve error
- columnas no presentes en `INSERT` quedan en `NULL` cuando aplica

## Gramática SQL soportada
- `CREATE TABLE`
- `INSERT INTO ... VALUES (...)`
- `SELECT ... FROM ...`
- `WHERE <pk> = ...`
- `WHERE <pk> BETWEEN ... AND ...`
- `LIMIT`
- `OFFSET`

No soportado todavía:
- `UPDATE`
- `DELETE`
- `JOIN`
- `ORDER BY`
- `GROUP BY`
- `LIKE`
- `WHERE` sobre columnas no PK

## Semántica HTTP
- mutex de proceso para escrituras
- modo single DB o multi DB
- token opcional por header
- `limit` máximo de `1000` en `/rows`

## Limitaciones técnicas actuales
- no hay MVCC
- no hay índices secundarios
- no hay locking fuerte entre procesos
- no hay migraciones de formato en disco entre versiones mayores
- el cálculo de `total` en `/rows` requiere scan completo

## Qué significa esto en producto
`gabysql` ya tiene una base sólida para aprender, demostrar y estabilizar storage/SQL básicos, pero todavía no tiene las capas de optimizer, concurrencia y compatibilidad histórica que definen un motor maduro.
