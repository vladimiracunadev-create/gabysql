# ADR-0046: `DECIMAL(p,s)` exacto (Y6)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Y6 (quinto sub-bloque post-Y, el último item grande de tipos)
**Bump on-disk**: 21 → 22

## 🧭 Contexto

Y mapeó `DECIMAL`/`NUMERIC` como **alias puros de FLOAT** — lo que ayudaba a portar schemas sin editar pero **perdía precisión** silenciosamente. Una columna `amount DECIMAL(10,2)` para guardar dinero almacenaba `0.1 + 0.2 = 0.30000000000000004` en lugar de `0.30` exacto. Inaceptable.

Y6 cierra esa brecha de la única forma correcta: nuevo tipo en disco, nueva variante de `Value`, encoding propio que preserva precisión arbitraria hasta 38 dígitos decimales.

## 💡 Decisión

### 1. `Value::Decimal { value: i128, scale: u8 }`

Variante nueva. `value` es la mantissa (entero sin decimales), `scale` indica cuántas posiciones del punto decimal hacia la derecha. Ejemplos:

- `123.45` → `{ value: 12345, scale: 2 }`
- `0.30` → `{ value: 30, scale: 2 }`
- `-1.500` → `{ value: -1500, scale: 3 }`
- `42` → `{ value: 42, scale: 0 }`

i128 da ~38 dígitos significativos — suficiente para cualquier `DECIMAL(p,s)` con `p ≤ 38`.

### 2. `ColumnType::Decimal` (code=11)

`DECIMAL`, `NUMERIC` y `DEC` (con o sin `(p,s)`) ahora mapean a este tipo — **ya no son aliases de FLOAT**. Default cuando no hay sufijo: `(10, 0)` (convención PG/MySQL).

`REAL`, `DOUBLE`, `DOUBLE PRECISION` siguen siendo `Float` — para el caso en que sí querés un f64 nativo.

### 3. Por-columna: `decimal_meta: Option<(u8, u8)>` = `(precision, scale)`

Persiste en el catálogo con un nuevo flag bit `COLUMN_FLAG_HAS_DECIMAL_META = 0x20` + 2 bytes después de `int_width`. Solo para Decimal columns. Precision 1..=38, scale 0..=precision.

### 4. Disk format por-fila

```
[present:u8=1][value:i128 LE = 16 bytes][scale:u8]
```

17 bytes por valor non-NULL (vs 8 para INT/FLOAT). Más caro en storage pero la precisión vale la pena.

### 5. Helpers exportados

```rust
pub fn decimal_to_string(value: i128, scale: u8) -> String;
pub(crate) fn parse_decimal(input: &str, target_scale: u8) -> Result<i128, String>;
pub(crate) fn parse_decimal_with_inferred_scale(input: &str) -> Result<(i128, u8), String>;
pub(crate) fn decimal_fits_precision(value: i128, scale: u8, precision: u8) -> bool;
pub(crate) fn value_to_decimal(v: &Value, target_scale: u8) -> Result<i128, String>;
pub(crate) fn rescale_decimal(value: i128, from: u8, to: u8) -> Result<i128, String>;
pub(crate) fn decimal_to_f64(value: i128, scale: u8) -> f64;
pub(crate) fn extract_decimal_meta(type_name: &str) -> Option<(u8, u8)>;
```

### 6. Comportamiento en INSERT/UPDATE

- **Integer** se rescaliza al `scale` declarado (`42` en `DECIMAL(10,2)` → 4200).
- **Float** se convierte via su repr textual estable de Rust (round-trip vía parser).
- **String** se parsea como decimal canónico (acepta signo `+`/`-`, parte fraccionaria, sin notación científica).
- **Truncación** silenciosa cuando hay más decimales que `scale`: `1.999` en `DECIMAL(10,2)` → `1.99` (no redondea).
- **Padding** automático cuando hay menos: `7.5` en `DECIMAL(10,3)` → `7.500`.
- **Precisión excedida** dispara `[GBY-4123]`: `1000.00` no cabe en `DECIMAL(5,2)` (max = `999.99`).

### 7. Comportamiento en lectura

El decoder restaura `value: i128` + `scale: u8` exactos. `Value::Decimal { value: 12345, scale: 2 }` se imprime como `"123.45"` en text contexts y se serializa como `"123.45"` (string en JSON, para no perder precisión en clientes que usan f64).

### 8. Aritmética con otros tipos

`Decimal + Int`, `Decimal + Float`, etc.: **promueven a f64** (lossy). Documentado. Aritmética puramente Decimal-Decimal sería el siguiente paso (overflow check + rescale) pero queda diferida — el use case dominante es storage exact.

### 9. CAST AS DECIMAL

`CAST(x AS DECIMAL)` sin sufijo paramétrico infiere el `scale` del input (cuenta dígitos tras el `.`). Para precisión declarativa, recomendamos declarar la columna `DECIMAL(p,s)` y dejar que el encoder maneje el rescale.

### 10. Bump 21 → 22

Necesario por el code 11 en disco y el nuevo flag bit. V21 rechazado con `[GBY-1003]` — un V22 puede tener columnas DECIMAL que V21 no sabe decodificar.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4123` | `DECIMAL_OUT_OF_RANGE` | Parte entera de un valor `DECIMAL(p,s)` excede `10^(p-s)`, o overflow de i128. |

## 🚫 Limitaciones documentadas

- **Aritmética mixta promueve a f64** (lossy). Aritmética Decimal-pura aún no implementada.
- **CAST AS DECIMAL no admite sufijo `(p,s)`** — usar columna tipada para precisión declarativa.
- **`DEFAULT DECIMAL` se persiste como string** y se re-parsea al insertar (no compromete precisión, pero requiere round-trip).
- **No indexable como ordered** (la representación i128+scale no es lex-comparable directo).
- **NaN/Inf no representables** — son inválidos para DECIMAL.
- **Notación científica `1.5e3`** no soportada en literales DECIMAL.

## 🔄 Cambios incompatibles

Tests `y_*` que asumían `DECIMAL`/`NUMERIC` → `Value::Float` fueron actualizados:

- `y_float_family_aliases_work`: ahora aserta `Value::Decimal { value: 450, scale: 2 }` para `NUMERIC(10,2) = 4.5`.
- `y_alter_table_add_column_with_alias`: ahora aserta `Value::Decimal { value: 9950, scale: 2 }` para `DECIMAL(8,2) = 99.5`.

## 🧪 Validación

Suite `y6_*` en `tests/integration_test.rs` (13 tests):

- `y6_decimal_roundtrip_exact`
- `y6_decimal_preserves_money_precision` (0.1+0.2 = 0.30 exacto)
- `y6_decimal_truncates_extra_decimals` (1.999 → 1.99 con scale=2)
- `y6_decimal_pads_fractional_part` (7 → 7.000 con scale=3)
- `y6_decimal_negative_works`
- `y6_decimal_precision_exceeded_rejected` (1000.00 no cabe en DECIMAL(5,2))
- `y6_decimal_at_precision_boundary_ok` (999.99 sí cabe)
- `y6_numeric_alias_works`
- `y6_decimal_default_precision_when_no_params` (DECIMAL sin (p,s) = (10, 0))
- `y6_cast_text_to_decimal`
- `y6_decimal_null_works`
- `y6_decimal_survives_reopen` (12 dígitos enteros + 4 fraccionarios)
- `y6_dec_alias_works`

Suite total: **580/580 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

- **Aritmética Decimal-Decimal exacta** (sin promoción a f64).
- **`DECIMAL` indexable** (encoding lex-comparable: sign + complement-2 i128 with byte-flip).
- **`UNSIGNED BIGINT` real (u64)**.
- **`CHAR(n)` con padding** ANSI strict.
- **Conteo por code points** en VARCHAR(n).
- **`ARRAY[T]`**, **`ENUM(...)`**, **`INTERVAL`**, **TZ types**.
- **BLOB indexable** (overflow chain).
- **UUID v1/v6/v7** timestamp-based.
