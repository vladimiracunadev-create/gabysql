# ADR-0049: Cierre del bloque Y — agregados decimal, UUID v7, sci notation (Y9)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Y9 (último sub-bloque de Y; cierre del bloque)
**Bump on-disk**: ninguno

## 🧭 Contexto

Y6/Y7/Y8 introdujeron `DECIMAL` exacto, comparaciones cross-type y la trifecta `* / %`. Faltaba lo que normalmente quiere quien ya invirtió en Decimal: que los **agregados** preserven la exactitud (no caer a `f64` en `SUM`/`AVG`), que existan generadores aleatorios "modernos" (UUID v7, bytes random), y que la entrada de datos acepte **notación científica** (`1.5e3`, `2.5E-4`), tan común en CSV exportados desde herramientas científicas y financieras.

Este ADR cierra el bloque Y. Lo que **no** está en Y queda explícitamente diferido más abajo, con motivación.

## 💡 Decisión

### 1. `SUM(decimal)` exact accumulator multi-modo

Pre-scan no requerido — usamos un acumulador de 3 modos que **promociona** según el tipo de cada fila:

```
mode 0 (int):     acc_int: i128
mode 1 (decimal): acc_dec_value: i128, acc_dec_scale: u8
mode 2 (float):   acc_float: f64
```

Reglas de transición:
- `int → decimal`: rescalar `acc_int * 10^scale` y pasar a modo decimal.
- `decimal → decimal con scale distinto`: `rescale_decimal` al mayor scale.
- `cualquier_cosa → float`: convertir acumulador a `f64` vía `decimal_to_f64` y operar en `f64`.

El tipo de retorno refleja el modo final: `Value::Integer`, `Value::Decimal { value, scale }` o `Value::Float`.

### 2. `AVG(decimal)` exact con política Y8

Pre-scan ligero detecta `has_float` y `has_decimal`. Si **no hay floats** y hay al menos un decimal, llamamos recursivamente a `SUM` para obtener el numerador exacto, y aplicamos la **política Y8 de división**:

```
target_scale = max(sum_scale, 6)
shift = target_scale - sum_scale
scaled = sum_value.checked_mul(10^shift)
result = scaled / count   // truncation hacia cero
→ Value::Decimal { value: result, scale: target_scale }
```

Si **hay floats**, caemos al path `f64` que ya existía, extendido para convertir `Value::Decimal` a `f64`.

### 3. `MIN/MAX(decimal)` — sin trabajo

Ya funcionaban en Y7 gracias a `compare_values` cross-type. Tests `y9_min_max_decimal` confirman.

### 4. `UUID_V7()` — RFC 9562

Aliases: `UUID_V7`, `UUID_GENERATE_V7`, `GEN_UUID_V7`.

Layout RFC 9562:
- bytes `0..=5`: timestamp Unix-ms BE (48 bits)
- nibble alto byte `6`: version `0x7`
- nibble alto byte `8`: variant `0b10xx`
- resto: PRNG xorshift64 sembrado por `(ts_ms ^ magic)`

Salida: string canónica `xxxxxxxx-xxxx-7xxx-Vxxx-xxxxxxxxxxxx` (lower-case).

**Aviso**: PRNG **no criptográfico** (xorshift64). Para tokens de seguridad usar otro mecanismo.

### 5. `GEN_RANDOM_BYTES(n)` — bytes random

Aliases: `GEN_RANDOM_BYTES`, `RANDOM_BYTES`.

Genera `n` bytes a partir de xorshift64 sembrado por `now_nanos ^ magic`. Devuelve `Value::Bytes`. Mismo aviso: **no cripto**.

### 6. Notación científica en literales numéricos

El **lexer** extiende el token `Number` para consumir un sufijo opcional `[eE][+-]?digits` cuando va inmediatamente después del mantissa. `parse_decimal` ya soportaba la conversión:

- Exponente positivo (`1.5e3`): multiplica el valor parseado por `10^exp` al final.
- Exponente negativo (`2.5e-2`): infla `parse_scale = clamp(target_scale + |exp|, 0, 38)` para absorber sin perder precisión, y al final divide por `10^|exp|` (truncando lo que sobra del target).

Float-literals (path `f64`) también ven la notación científica vía `str::parse::<f64>()` que ya la acepta nativo.

## 📁 Archivos tocados

- `src/sql.rs`:
  - `compute_aggregate`: SUM multi-modo + AVG decimal-pure dispatch.
  - `ScalarFunc::GenUuidV7` y `::GenRandomBytes` añadidos al enum, `keyword()`, `from_ident()`, arity check, dispatch.
  - `gen_uuid_v7()` y `gen_random_bytes(n)` helpers.
  - `parse_decimal`: soporte para notación científica con conservación de precisión en `exp` negativos.
  - Lexer `tokenize`: extiende `Number` con `[eE][+-]?digits` opcional.
- `tests/integration_test.rs`: 6 tests `y9_*` (SUM, AVG, MIN/MAX, UUID v7 shape, gen_random_bytes len, sci notation).

## ⛔ Lo que **no** entra en Y9 (diferido explícitamente)

Lo siguiente NO se implementó en Y para mantener el bloque cerrable. Cada uno bumpa on-disk o es un sub-proyecto en sí.

| Ítem | Razón del defer |
|---|---|
| `ARRAY[T]` / `ENUM(...)` / `INTERVAL` / `TIMESTAMPTZ` | Cada uno es nuevo `ColumnType` + variante `Value` + codec on-disk + reglas de comparación/aritmética. Cada uno justifica su propio bloque (Y10+). |
| `UNSIGNED BIGINT` real (`u64`) | El sign-bit truco de Y5 cubre `UINT8/16/32`. `u64` requiere otra ruta de codec porque no entra en `i64`. Defer hasta benchmarks que lo demanden. |
| `DECIMAL`/`BLOB` indexables (B-tree) | Hoy aceptamos los tipos en columnas pero **no** como claves de índice. Cambiar `compare_index_key` para Decimal/Bytes es bloque propio. |
| `CHAR(n)` con padding | Hoy `CHAR(n)` se trata como `VARCHAR(n)` (sin padding a la derecha). El padding cambia comparación/igualdad → defer. |
| Code points reales en `VARCHAR` length-check | Hoy contamos bytes UTF-8, no code points. Cambiar a code points es 1-line pero requiere migración de tests existentes. Defer. |
| `POWER(decimal, n)` exact | Por ahora `POWER` cae a `f64`. Decimal-exact requiere serie taylor o exponenciación rápida con overflow checks → bloque propio. |
| `WHERE col_a = col_b` con tipos heterogéneos relajado | Hoy `compare_values` es estricta entre familias incompatibles (string vs int → error). Relajar a "comparar normalizado" cambia semántica de WHERE → defer. |

## 🧪 Tests

`y9_sum_decimal_exact`, `y9_avg_decimal_exact`, `y9_min_max_decimal`, `y9_uuid_v7_shape`, `y9_gen_random_bytes_len`, `y9_decimal_scientific_notation`. Suite total: **605 passing** (599 → +6 Y9).

## 🔗 Referencias

- ADR-0046 (Y6): `DECIMAL` exact.
- ADR-0047 (Y7): Decimal arith + compare.
- ADR-0048 (Y8): Decimal mul/div/mod.
- RFC 9562 §5.7 UUID v7.
