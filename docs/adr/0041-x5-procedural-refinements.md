# ADR-0041: Refinamientos del bloque X (X5)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: X5 (cleanup post-X4f de los items menores diferidos)
**Bump on-disk**: ninguno

## 🧭 Contexto

X4f cerró el grueso del bloque X (procedural completo). Pero quedaron 5 items menores listados como "Diferido (post-X4f)" en los CHANGELOG y ADRs:

1. `RAISE WARNING` y `RAISE INFO` (X4c sólo dejó EXCEPTION/NOTICE).
2. Formato `%` en RAISE (`RAISE EXCEPTION 'value % invalid', x`).
3. `STEP n` y `REVERSE` en `FOR i IN a TO b LOOP`.
4. `EXCEPTION WHEN <name>` con filtros simbólicos PG-style (`no_data_found`, `unique_violation`, etc.).

(`FOR row IN SELECT ... LOOP` queda para X6 — requiere composite row scope, que es scope mayor).

X5 los junta en un único push porque cada uno es chico (≤30 líneas de código), no interactúan entre sí, y ninguno toca disco — es 100% AST + parser + engine.

## 💡 Decisión

### 1. `RAISE WARNING` y `RAISE INFO`

```rust
pub enum RaiseLevel {
    Exception, Notice, Warning, Info,  // X5 sumó Warning + Info
}
```

`Warning` e `Info` se comportan exactamente como `Notice`: producen un `ResultSet` vacío con `message: Some("WARNING: ...")` o `"INFO: ..."`. La elección entre los tres es semántica (para el logger del cliente / observador), no de comportamiento del motor.

El parser acepta `RAISE WARNING ...` y `RAISE INFO ...` como hermanos de `NOTICE` antes del default a `EXCEPTION`.

### 2. Formato `%` en RAISE

```sql
RAISE EXCEPTION 'valor % invalido en columna %', v, col;
-- → "[GBY-4111] valor 42 invalido en columna age"
```

- `RaiseStmt` gana `args: Vec<Expr>`.
- Parser: después del literal STRING, si viene `,`, parsea expresiones separadas por coma.
- `format_raise_message(template, args)` substituye cada `%` con el valor textual del arg correspondiente. `%%` escapa un literal `%`. Arity strict: si #% != #args → `[GBY-4120]`.

Compat: sin args, comportamiento idéntico a X4c (el template se pasa tal cual).

### 3. `STEP n` y `REVERSE` en `FOR`

Sintaxis: `FOR var IN [REVERSE] start TO end [STEP n] LOOP ... END LOOP`.

- `ForStmt` gana `step: Option<Expr>` y `reverse: bool`.
- Parser: `REVERSE` opcional antes de `start`, `STEP n` opcional después de `end`.
- Engine: `step_magnitude = |n|` (default 1), `step_effective = -step_magnitude` si reverse else `+step_magnitude`. Loop usa `i.saturating_add(step_effective)` y para según el signo (`i <= end` ascending, `i >= end` descending). `STEP 0` → `[GBY-4120]`.

Compat: sin REVERSE/STEP, comportamiento idéntico a X4c (step=+1).

### 4. `EXCEPTION WHEN <name>` simbólico

```sql
BEGIN
   INSERT INTO t (id) VALUES (1);
EXCEPTION
   WHEN primary_key_violation THEN ...;
   WHEN foreign_key_violation THEN ...;
   WHEN OTHERS THEN ...;
END;
```

- `ExceptionFilter` gana `Name(String)` (además de `Code(u32)` y `Others`).
- Parser: si el token tras WHEN es Ident (y no OTHERS), se guarda como Name.
- Engine: en el matcher de `exec_block`, `Name(n)` se resuelve a código vía `resolve_exception_name(n)` y se compara contra el código del error. Si el nombre no está en la tabla, el filtro nunca matchea (no error — sigue al próximo).

Mapeo inicial (subset PG-style, los más usados):

| Nombre simbólico | Código |
|---|---|
| `no_data_found` / `not_found` | 3006 (ROW_NOT_FOUND_FOR_PK) |
| `unique_violation` | 3003 (UNIQUE_VIOLATED) |
| `not_null_violation` | 3002 (NOT_NULL_VIOLATED) |
| `foreign_key_violation` | 3004 (FK_PARENT_MISSING) |
| `check_violation` | 3008 (CHECK_VIOLATED) |
| `primary_key_violation` | 3001 (DUPLICATE_PRIMARY_KEY) |
| `division_by_zero` | 4043 (DIVISION_BY_ZERO) |
| `numeric_value_out_of_range` | 4042 (ARITH_OVERFLOW) |
| `string_data_right_truncation` / `value_length_exceeded` | 4119 (VALUE_LENGTH_EXCEEDED) |
| `raise_exception` | 4111 (RAISE_EXCEPTION) |

La tabla crece según demanda.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4120` | `RAISE_FORMAT_OR_FOR_STEP_INVALID` | `RAISE 'fmt %', args` con arity mismatch entre `%` y args, **o** `FOR i IN ... STEP 0` (incremento cero ⇒ loop infinito). Slot compartido porque ambos son errores estructurales del modelo procedural sin overlap semántico. |

## 🧪 Validación

Suite `x5_*` en `tests/integration_test.rs` (12 tests):

- `x5_raise_warning_emits_message`, `x5_raise_info_emits_message`
- `x5_raise_format_pct_substitutes_args`, `x5_raise_format_pct_escape_double`, `x5_raise_format_pct_arity_mismatch_errors`
- `x5_for_step_skips_values`, `x5_for_reverse_decrements`, `x5_for_reverse_step_combined`, `x5_for_step_zero_rejected`
- `x5_exception_when_symbolic_unique_falls_through_on_pk_dup` (verifica que `unique_violation` → 3003 NO matchea un `[GBY-3001]` PK dup, cae a OTHERS)
- `x5_exception_when_primary_key_violation_name` (`primary_key_violation` → 3001 sí matchea)
- `x5_exception_when_unknown_name_falls_through` (nombre no mapeado nunca matchea, cae a OTHERS)

Suite total: **522/522 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

Lo que queda del bloque X:

- **`FOR row IN SELECT ... LOOP`** (X6): requiere composite row scope (`row.col` accesible como variables anidadas dentro del body). Es scope mayor — no entra en el cleanup X5.
- Más nombres simbólicos en `resolve_exception_name` según pidan los users reales.

Lo recomendado a continuación es **X6** (FOR row IN SELECT) si se quiere cerrar el bloque X al 100%, o saltar a **Fase 3** (planner + EXPLAIN + benchmarks) que da más palanca producto-wise.
