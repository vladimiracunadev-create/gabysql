# ADR-0043: Enforcement de rango `TINYINT`/`SMALLINT`/`MEDIUMINT`/`INT4` (Y3)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Y3 (segundo sub-bloque post-Y)
**Bump on-disk**: 18 → 19

## 🧭 Contexto

Y dejó pasar `BIGINT`/`SMALLINT`/`TINYINT`/`INT2`/`INT4`/`MEDIUMINT` como **aliases puros de `INT`**: el motor los guardaba como `i64` y aceptaba cualquier número que cupiera en ese rango. Eso ayudó a portar schemas pero falló la promesa semántica: una columna `age TINYINT` aceptaba `INSERT INTO t (age) VALUES (1000)` silenciosamente.

Y3 cierra esa brecha — siguiendo exactamente el mismo patrón de Y2 (`max_length` para VARCHAR(n)/CHAR(n)). Persiste un `int_width: Option<u8>` por columna, agrega un flag bit en el catálogo, y enforce el rango en el encoder.

`BLOB`/`BYTEA` (binario) y `DECIMAL(p,s)` **exacto** siguen diferidos a bloques posteriores — requieren cambios al `Value` enum y a la serialización de filas. Y3 sólo persiste 1 byte por columna y agrega un check de rango — scope mínimo.

## 💡 Decisión

### 1. Persistir `int_width: Option<u8>` en `Column`

```rust
pub struct Column {
    // ... campos anteriores ...
    pub max_length: Option<u32>,  // Y2
    pub int_width: Option<u8>,    // Y3
}
```

El byte codifica el ancho en bytes del rango enforced:

| `int_width` | Tipo declarado | Rango enforced |
|---|---|---|
| `1` | `TINYINT` | i8: `[-128, 127]` |
| `2` | `SMALLINT` / `INT2` | i16: `[-32_768, 32_767]` |
| `3` | `MEDIUMINT` | 24-bit signed: `[-8_388_608, 8_388_607]` |
| `4` | `INT4` | i32: `[-2_147_483_648, 2_147_483_647]` |
| `None` | `INT` / `INTEGER` / `BIGINT` / `INT8` | i64 nativo (sin enforce) |

El motor sigue usando `i64` internamente. Y3 sólo agrega un check al encoder; la representación en disco de un INT no cambia (siguen siendo 8 bytes LE).

### 2. Disk format

Nuevo flag bit:

```rust
const COLUMN_FLAG_HAS_INT_WIDTH: u8 = 0x10;
```

Cuando está prendido, después del bloque `max_length` (Y2) se escribe **1 byte** con el `int_width`. El decoder lo lee condicionalmente al flag.

Columnas pre-Y3 (sin `int_width`) son byte-idénticas a V18 — la adición es additive en wire format.

### 3. Bump 18 → 19

Necesario porque un binario V18 leyendo un V19 con `int_width` no sabría que después de `max_length` puede haber 1 byte extra y leería el `idx_count` desde el offset equivocado. Bump obligatorio. V18 → rechazado con `[GBY-1003]`.

### 4. Helper `extract_int_width`

Función pura en `sql.rs`:

```rust
pub(crate) fn extract_int_width(type_name: &str) -> Option<u8>
```

Toma el `type_name` ya parseado (e.g. `"TINYINT"`, `"SMALLINT"`, `"INT2"`) y devuelve `Some(1..=4)` para los 4 tipos enforced. Strip de `(...)` por si vino con `INT(11)` legacy de MySQL. Cualquier otro tipo (incluyendo `INT`/`BIGINT`/`INT8`) devuelve `None`.

### 5. Enforcement en el encoder

Una sola condición agregada al arm `(ColumnType::Int, Value::Integer)` en `encode_row`:

```rust
if let Some(w) = column.int_width {
    let (min, max) = int_width_range(w);
    if number < min || number > max {
        return Err(coded(codes::INT_RANGE_EXCEEDED, format!(
            "valor {} para columna '{}' fuera de rango {} ({}..={})",
            number, column.name, int_width_label(w), min, max
        )));
    }
}
```

Cubre INSERT, UPDATE, INSERT...SELECT y CTAS — el mismo path único.

### 6. Helpers públicos

```rust
pub(crate) fn int_width_range(width: u8) -> (i64, i64);
pub(crate) fn int_width_label(width: u8) -> &'static str;
```

Ambas tienen `_` fallback al rango i64 completo / label `"INT"` para evitar panics en casos imposibles (codes en disco corruptos, etc.).

## 📐 Código de error nuevo

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4121` | `INT_RANGE_EXCEEDED` | INSERT/UPDATE de entero fuera del rango declarado por `TINYINT`/`SMALLINT`/`INT2`/`MEDIUMINT`/`INT4`. |

## 🧪 Validación

Suite `y3_*` en `tests/integration_test.rs` (14 tests):

- `y3_tinyint_in_range_works` (127, -128, 0 OK)
- `y3_tinyint_over_range_rejected` (200 → 4121)
- `y3_tinyint_under_range_rejected` (-200 → error)
- `y3_smallint_in_range_works` (32767, -32768)
- `y3_smallint_over_range_rejected` (50000)
- `y3_int2_alias_enforced_like_smallint`
- `y3_mediumint_in_range_works` (8388607, -8388608)
- `y3_mediumint_over_range_rejected` (10M)
- `y3_int4_in_range_works` (2147483647)
- `y3_int4_over_range_rejected` (3B)
- `y3_int_bigint_no_enforce` (9 * 10^18 OK en BIGINT)
- `y3_update_also_enforced` (UPDATE dispara 4121)
- `y3_alter_add_smallint_persists_enforcement`
- `y3_int_width_survives_reopen`

Suite total: **544/544 pass** (`cargo test --lib --tests`).

## 🔭 Futuro (Y4+)

> 📝 **Actualización 2026-06-15**: 3 de los 4 ítems ya fueron entregados. Lista actualizada abajo.

Lo que aún queda en la familia "tipos":

- ~~**`BLOB` / `BYTEA` / `BINARY`**~~ ✅ entregado por **Y4** ([ADR-0044](0044-blob-bytea-y4.md), 2026-05-29) — `Value::Bytes` real.
- ~~**`DECIMAL(p,s)` exacto**~~ ✅ entregado por **Y6** ([ADR-0046](0046-decimal-exact-y6.md), 2026-05-29) — `Value::Decimal` con `i128 + scale`. Aritmética en Y7/Y8.
- ~~**`UNSIGNED TINYINT/SMALLINT/INT/BIGINT`**~~ ✅ entregado por **Y5** ([ADR-0045](0045-unsigned-and-uuid-y5.md), 2026-05-29) — bit alto del `int_width`.
- **`CHAR(n)` con padding** a la derecha (estándar SQL). **Sigue pendiente.**
- **Conteo por code points** en `VARCHAR(n)` (vs bytes UTF-8 actual de Y2).
- **`ARRAY[T]`**, **`ENUM(...)`**, **`INTERVAL`**, **`TIME WITH TIME ZONE`**, **`TIMESTAMP WITH TIME ZONE`**.
- **Generación auto de UUID** (`gen_random_uuid()`, `uuid_v4()`).
