# ADR-0039: Tipos de columna extendidos (Y)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Y (primer bloque post-X)
**Bump on-disk**: 16 → 17

## 🧭 Contexto

Hasta el bloque X4f los tipos de columna eran 7: `INT`, `TEXT`, `BOOL`, `FLOAT`, `DATE`, `DATETIME`, `JSON`. Eso cubre la mayor parte de los schemas reales, pero deja afuera dos clases de uso:

1. **Aliases sintácticos** que aparecen en cualquier dump de PostgreSQL/MySQL/SQLServer/Oracle (`BIGINT`, `VARCHAR(255)`, `DECIMAL(10,2)`, `DOUBLE PRECISION`, `TIMESTAMP`, `BOOLEAN`, etc.). Forzar al usuario a editarlos a `INT`/`TEXT`/`FLOAT`/`DATETIME`/`BOOL` antes de poder cargarlos es fricción gratuita.
2. **Tipos comunes que no tenían representación**: `TIME` (hora del día), `UUID` (identificadores universales).

`BLOB`/`BYTEA` (binario crudo), `DECIMAL` con precisión exacta y `ARRAY` quedan diferidos a un bloque Y2 — requieren cambios en `Value` (nueva variante `Bytes` o `Decimal`) y en la serialización de filas, que es scope más grande.

## 💡 Decisión

### 1. Aliases sintácticos sin cambio en disco

Todos los aliases mapean a uno de los 7 tipos existentes. El parser acepta la sintaxis, los parámetros entre paréntesis (`(n)`, `(p,s)`) se consumen pero **no se enforcer** — Y no valida longitud ni precisión.

| Familia | Aliases aceptados | Mapea a |
|---|---|---|
| Enteros | `INT`, `INTEGER`, `INT2`, `INT4`, `INT8`, `BIGINT`, `SMALLINT`, `TINYINT`, `MEDIUMINT` | `Int` |
| Punto flotante | `FLOAT`, `REAL`, `DOUBLE`, `DOUBLE PRECISION`, `NUMERIC[(p,s)]`, `DECIMAL[(p,s)]`, `DEC[(p,s)]` | `Float` |
| Texto | `TEXT`, `VARCHAR[(n)]`, `CHAR[(n)]`, `CHARACTER[(n)]`, `CHARACTER VARYING[(n)]`, `NVARCHAR[(n)]`, `NCHAR[(n)]`, `STRING`, `CLOB` | `Text` |
| Booleano | `BOOL`, `BOOLEAN` | `Bool` |
| Fecha/hora | `DATETIME`, `TIMESTAMP` | `DateTime` |

### 2. Dos tipos nuevos con código en disco

| Tipo | Code | Storage | Validación |
|---|---|---|---|
| `TIME` | 8 | text | lex `HH:MM:SS[.fff]` (en `CAST`) |
| `UUID` | 9 | text | lex `8-4-4-4-12` hex canónico (en `CAST`, normaliza a lowercase) |

Ambos pasan a `stores_as_text()` → son indexables (vía hash bucket, no ordered), aceptan DEFAULT de tipo String, encoder/decoder lo trata como TEXT length-prefixed.

### 3. Bump 16 → 17

Necesario porque un schema legítimo en V17 puede tener columnas `TIME` o `UUID` que un binario V16 no sabría decodificar (`from_code(8)` falla). V16 → V17 es rechazado con `[GBY-1003]` (migración manual via export/import).

Los aliases puros (BIGINT, VARCHAR(n), etc.) **no** habrían justificado el bump — un schema escrito con `BIGINT` se persiste como `Int`, indistinguible en disco de uno escrito como `INT`. El bump es por TIME/UUID.

### 4. `parse_type_name` helper en el parser

Single point of entry para parsear tipos de columna. Soporta:

- Identificador simple: `INT`, `BIGINT`, `UUID`, …
- Identificador compuesto: `DOUBLE PRECISION`, `CHARACTER VARYING`.
- Sufijo paramétrico: `VARCHAR(255)`, `DECIMAL(10, 2)`, `NUMERIC(5)`.

Llamado desde:

- `parse_column_def` (CREATE TABLE / ALTER TABLE ADD COLUMN).
- `parse_create_function` (params + RETURNS).
- `parse_create_procedure` (params).
- `parse_declare_stmt` (X4b: `DECLARE name TYPE [DEFAULT expr]`).
- `parse_cast_expr` (`CAST(expr AS TYPE)`).

El string devuelto se pasa tal cual a `ColumnType::from_sql`, que normaliza case, colapsa espacios, descarta el sufijo paramétrico y resuelve el alias.

### 5. Sin códigos de error nuevos

Y reusa los códigos existentes:

- `[GBY-3007]` (familia) para mismatches `value vs column`.
- `[GBY-NNNN] CAST_INVALID` para `CAST('xx' AS TIME)` mal formado.
- Mensaje genérico `tipo no soportado: <X>` para tipos no reconocidos (e.g. `GEOMETRY`, `ARRAY`).

## 📐 Limitaciones explícitas

- **`VARCHAR(n)` y `CHAR(n)` no enforcer longitud**. Una columna `VARCHAR(10)` acepta strings de cualquier tamaño hasta 65535 bytes (límite global de TEXT). Validación de longitud queda para Y2.
- **`DECIMAL(p,s)` no es decimal exacto**. Se almacena como `f64`. Si necesitás precisión exacta (sumas monetarias), Y no la da — esperar a un bloque futuro con `Value::Decimal`.
- **`TIME` y `UUID` solo validan forma léxica**. `TIME '25:99:99'` se aceptaría como string (24:00 no rechaza). El UUID no chequea version/variant.
- **`BLOB`/`BYTEA`/`BYTES`/`BINARY`** no soportados — requieren `Value::Bytes` que cambia toda la serialización.
- **`ARRAY[T]`, `RANGE[T]`, `ENUM(...)`, `INTERVAL`, `TIME WITH TIME ZONE`, `TIMESTAMP WITH TIME ZONE`** no soportados.

## 🧪 Validación

Suite `y_*` en `tests/integration_test.rs` (13 tests):

- `y_int_family_aliases_work`: BIGINT, SMALLINT, TINYINT, INTEGER, MEDIUMINT, INT8.
- `y_float_family_aliases_work`: REAL, DOUBLE, DOUBLE PRECISION, NUMERIC(p,s), DECIMAL(p,s).
- `y_text_family_aliases_work`: VARCHAR(n), CHAR(n), STRING, CHARACTER VARYING(n), NVARCHAR(n).
- `y_bool_and_timestamp_aliases`: BOOLEAN, TIMESTAMP.
- `y_time_column_stores_and_queries`: TIME con `HH:MM:SS` y `HH:MM:SS.fff`.
- `y_uuid_column_stores_and_queries`: UUID canónico.
- `y_cast_to_uuid_normalizes_lowercase`: CAST AS UUID lowercase'a.
- `y_cast_to_time_validates_format`: CAST AS TIME valida + rechaza basura.
- `y_cast_to_uuid_rejects_invalid`: CAST AS UUID rechaza string sin forma.
- `y_aliases_in_function_signature`: BIGINT y NUMERIC(p,s) y DOUBLE PRECISION en CREATE FUNCTION.
- `y_declare_var_with_alias_type`: DECLARE m VARCHAR(50) dentro de procedure.
- `y_alter_table_add_column_with_alias`: ALTER TABLE ADD COLUMN VARCHAR(n) / DECIMAL(p,s) / TIME.
- `y_unsupported_type_still_errors`: GEOMETRY sigue tirando error (no es alias).

## 🔭 Futuro (Y2 y más allá)

- **Length enforcement** en VARCHAR(n)/CHAR(n).
- **Range enforcement** en SMALLINT/TINYINT.
- **`BLOB`/`BYTEA`** con `Value::Bytes` real.
- **`DECIMAL(p,s)` exacto** con `Value::Decimal` (string o int128 + scale).
- **`UUID` con generación auto** (`gen_random_uuid()`).
- **`TIME WITH TIME ZONE`**, **`TIMESTAMP WITH TIME ZONE`**.
- **`ARRAY[T]`**, **`ENUM(...)`**, **`INTERVAL`**.
