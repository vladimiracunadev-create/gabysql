# ADR-0048: Mul/Div/Mod `DECIMAL` exactos (Y8)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Y8 (séptimo sub-bloque post-Y, follow-up de Y7)
**Bump on-disk**: ninguno

## 🧭 Contexto

Y7 cerró Add/Sub Decimal exactos. Y8 cierra Mul/Div/Mod — la trifecta que completa la aritmética Decimal-pura. Sin esto, multiplicar un precio por una cantidad (`SELECT price * qty FROM line_items`) perdía precisión silenciosamente.

## 💡 Decisión

### 1. Mul: `scale_result = a.scale + b.scale`

```rust
target_scale = a.scale + b.scale       // si > 38 → [GBY-4123]
result.value = a.value.checked_mul(b.value)  // si None → [GBY-4042]
result.scale = target_scale
```

Ejemplo: `1.50 (scale=2) * 2.00 (scale=2) = 3.0000 (scale=4)`.

Si `a.scale + b.scale > 38` → `[GBY-4123]` (DECIMAL_OUT_OF_RANGE) porque el i128 no puede representar más de ~38 dígitos decimales.

Si la multiplicación de las mantissas `a.value * b.value` overflowea i128 → `[GBY-4042]` (ARITH_OVERFLOW).

### 2. Div: `target_scale = max(a.scale, b.scale, 6)`

Política: mínimo 6 decimales en el quotient para preservar precisión razonable (estilo SQL Server). El cliente puede declarar el target scale explícitamente vía `CAST(... AS DECIMAL(p,s))`.

```rust
shift = target_scale - a.scale + b.scale
scaled = a.value.checked_mul(10^shift)   // pre-shift al dividend
result.value = scaled / b.value           // truncation hacia cero
result.scale = target_scale
```

Ejemplo: `1.00 / 3.00 = 0.333333 (scale=6, trunca)`.

División por cero → `[GBY-4043]` (DIVISION_BY_ZERO).

**Truncation policy**: `i128::div` trunca hacia cero, igual que Rust integer div y SQLite. PG y Oracle redondean half-up; SQL Server y MySQL truncan. Elegimos truncation por simplicidad y consistencia con la promesa "DECIMAL es exacto" — redondear introduce sutilezas que merecen un block aparte si hay demanda.

### 3. Mod: `target_scale = max(a.scale, b.scale)`

```rust
a_norm = rescale_decimal(a, target_scale)   // align scales
b_norm = rescale_decimal(b, target_scale)
result.value = a_norm.checked_rem(b_norm)
result.scale = target_scale
```

Ejemplo: `10.00 % 3.00 = 1.00`. Módulo por cero → `[GBY-4043]`.

### 4. Cross-type Decimal/Int sigue exacto

Int se ve como `(int as i128, scale=0)`. La aritmética binaria con Int sigue el mismo path Decimal exact.

### 5. Cross-type Decimal/Float sigue promoviendo a f64

Mezclar Decimal con Float es lossy. Documentado.

## 📐 Códigos de error

Reusa:
- `[GBY-4042]` `ARITH_OVERFLOW` para mul que excede i128 o rescale en mod.
- `[GBY-4043]` `DIVISION_BY_ZERO` para div/mod con divisor cero.
- `[GBY-4123]` `DECIMAL_OUT_OF_RANGE` para scale resultante > 38 en mul.

## 🧪 Validación

Suite `y8_*` en `tests/integration_test.rs` (10 tests):

- `y8_decimal_mul_exact` (1.50 * 2.00 = 3.0000, 0.10 * 0.10 = 0.0100 EXACTO, 123.45 * 100.00 = 12345.0000)
- `y8_decimal_mul_by_integer` (19.99 * 3 = 59.97)
- `y8_decimal_div_exact` (10.00 / 4.00 = 2.500000, 1.00 / 3.00 = 0.333333 trunca)
- `y8_decimal_div_by_integer` (100.00 / 4 = 25.000000)
- `y8_decimal_div_by_zero_errors` (4043)
- `y8_decimal_mod_exact` (10.00 % 3.00 = 1.00, 7.50 % 2.00 = 1.50)
- `y8_decimal_mod_by_zero_errors` (4043)
- `y8_decimal_chain_arith` (price * qty * tax → scale 8 exacto)
- `y8_decimal_mul_scale_overflow_rejected` (scale 20*20=40 > 38 → 4123)
- `y8_decimal_mul_negative` (signos correctos en mul)

Suite total: **599/599 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

- **Rounding alternativo en Div** (half-up, half-even) opt-in via `CAST` o flag de session.
- **`SUM(decimal_col)`/`AVG(decimal_col)` agregados Decimal-puros** (hoy promueven a f64).
- **`WHERE col_a = col_b`** sin necesidad de subquery (limitación general del parser).
- **DECIMAL indexable** (encoding lex-comparable: sign + complement-2 i128 byte-flip BE).
- **`POWER(decimal, decimal)`** exact (necesita política específica, probable f64-only).
- **Auto-rescale para no perder dígitos** en Mul con scale grande (rescale prematuro al límite de i128).
- **Notación científica** en literales DECIMAL (`1.5e3`).

Con Y8, el ciclo Y6→Y7→Y8 cierra la historia DECIMAL para los 4 operadores aritméticos clásicos + 4 comparadores. Las operaciones agregadas (`SUM`, `AVG`, etc.) son el siguiente paso natural.
