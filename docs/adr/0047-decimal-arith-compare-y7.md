# ADR-0047: Aritmética y comparación `DECIMAL` exactas (Y7)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Y7 (sexto sub-bloque post-Y, follow-up de Y6)
**Bump on-disk**: ninguno

## 🧭 Contexto

Y6 introdujo `Value::Decimal { value: i128, scale: u8 }` y persistió DECIMAL exacto en disco. Pero la aritmética y las comparaciones seguían promoviendo a `f64` — un `SELECT a + b FROM pagos` perdía precisión silenciosamente, y `WHERE amount > 100.00` podía dar resultados sorprendentes en los bordes por imprecisión binaria.

Y7 cierra esa brecha: **Add/Sub Decimal-puros son exactos en i128, comparaciones alinean scales antes de comparar**. Mul/Div/Mod siguen promoviendo a f64 (decimal-puro mul es más complejo — scale resultante = sum de scales, requiere overflow checks específicos; div necesita una política de truncation/rounding documentada).

## 💡 Decisión

### 1. Add/Sub exacto Decimal-Decimal y Decimal-Int

Al detectar `Decimal + Decimal` o `Decimal ± Int` en `eval_arith`:

1. Extrae `(value, scale)` de cada operando (Int = `(int as i128, 0)`).
2. Computa `target_scale = max(a.scale, b.scale)`.
3. Rescala ambos al `target_scale` via `rescale_decimal` (`checked_mul` con `10^diff`).
4. Aplica `checked_add` / `checked_sub` sobre i128.
5. Resultado: `Value::Decimal { value, scale: target_scale }`.

Overflow en rescale o en la operación dispara `[GBY-4042]` `ARITH_OVERFLOW`.

### 2. Mul/Div/Mod siguen promoviendo a f64

Documentado como limitación. Implementar Decimal-puro mul requiere:

- `value_result = a.value * b.value` (potencial overflow i128 → necesita i256 o rescale prematuro)
- `scale_result = a.scale + b.scale`
- Posiblemente rescale a un `scale` razonable

División necesita política explícita: ¿truncar al scale del divisor? ¿round half-even? ¿round half-up? PG, Oracle y SQL Server difieren. Lo dejo para Y8.

### 3. Cross-type Decimal/Float → promoción f64

Cualquier operación que mezcle `Decimal` con `Float` cae al path f64 estándar (lossy). Documentado. Para preservar precisión, recomendamos no mezclar Decimal con Float — usar Int o convertir el Float a Decimal con `CAST` antes.

### 4. Comparaciones Decimal-Decimal y Decimal-Int exactas

Helper nuevo `compare_decimals(av, asc, bv, bsc) -> Ordering`:

1. Rescala ambos al `target_scale = max(asc, bsc)` via `checked_mul`.
2. Si rescale no overflowea, compara los i128 normalizados.
3. Si overflow, fallback a `decimal_to_f64` (lossy pero no panic).

Actualizado en cuatro lugares:
- `compare_values` (ORDER BY clamp normalizado para Set Operations / agregados)
- `compare_values_nulls_last` (ORDER BY nulls last estándar)
- `eval_compare` (operadores `<`/`<=`/`>`/`>=`/`!=` del WHERE/HAVING)
- `values_equal` (operador `=` y `!=` del WHERE/HAVING)

### 5. Limitación documentada: `WHERE col_a = col_b`

El parser de gabysql rechaza column-to-column equality en WHERE sin contexto de subquery correlacionada (`[GBY-4016]`). No es una limitación de Y7 — Y7 sólo hace que `WHERE col = 100.00` con `col DECIMAL(10,2)` funcione exacto. Para column-to-column, los users hoy escriben `WHERE col_a - col_b = 0` (que sí funciona con Y7 arithmetic).

## 📐 Códigos de error

Reusa:
- `[GBY-4042]` `ARITH_OVERFLOW` para overflow en rescale o checked_add/sub.

## 🧪 Validación

Suite `y7_*` en `tests/integration_test.rs` (9 tests):

- `y7_decimal_add_exact` (0.10 + 0.20 = 0.30 EXACTO)
- `y7_decimal_sub_exact` (1.00 - 0.99 = 0.01)
- `y7_decimal_plus_integer_exact` (99.50 + 1 = 100.50)
- `y7_decimal_diff_scales_align` (DECIMAL(10,2) + DECIMAL(10,4) → scale=4)
- `y7_decimal_equality_with_different_scales` (verifica via `a - b == 0`)
- `y7_decimal_less_than_exact` (WHERE amount < 100.00 borde exacto)
- `y7_decimal_compared_to_integer` (Decimal > Integer cross-type)
- `y7_decimal_order_by_exact` (ORDER BY con scale=4)
- `y7_decimal_negative_arith` (signos negativos)

Suite total: **589/589 pass** (`cargo test --lib --tests`).

## 🔭 Futuro (Y8 y más allá)

- **Mul Decimal-puro** exacto (scale_result = sum, overflow check via i256 o pre-rescale).
- **Div Decimal-puro** con política explícita (truncate to RHS scale, round half-up, half-even).
- **Mod Decimal-puro**.
- **`WHERE col_a = col_b`** sin necesidad de subquery (relax parser).
- **SUM/AVG agregados Decimal-puros** (hoy promueven a f64).
- **DECIMAL indexable** (encoding lex-comparable: sign + complement-2 i128 byte-flip BE).
- Aritmética entre dos columnas de scales muy distintos sin overflow (Y7 confía en i128 — para uso extremo con 38 dígitos hay edge cases).
