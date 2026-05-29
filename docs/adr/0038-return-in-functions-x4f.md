# ADR-0038: `RETURN expr` en function bodies (X4f)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: X4f (décimo y último previsto sub-bloque del bloque X)
**Bump on-disk**: ninguno

## 🧭 Contexto

X3b habilitó `CREATE FUNCTION name(...) RETURNS TYPE AS <expr>` con body **expresión única** (`AS x + 1`, `AS CASE WHEN ... END`). Esa forma alcanza para fórmulas puras, pero queda corta para lógica que necesita variables locales, branches procedurales o early-exit.

X4b/c/d/e ya armaron toda la maquinaria procedural (`DECLARE/SET/WHILE/EXIT`, `RAISE`, `BEGIN..EXCEPTION..END`, `LOOP`, `CASE`, `IF`). X4f cierra el círculo permitiendo function bodies multi-statement con `RETURN expr` como mecanismo de salida.

## 💡 Decisión

### 1. Sintaxis dual: expression body **o** block body

```sql
-- Forma X3b (sigue funcionando)
CREATE FUNCTION dbl(x INT) RETURNS INT AS x * 2;

-- Forma X4f (nueva)
CREATE FUNCTION sign(x INT) RETURNS TEXT AS BEGIN
    IF x < 0 THEN
        RETURN 'negative';
    ELSIF x = 0 THEN
        RETURN 'zero';
    ELSE
        RETURN 'positive';
    END IF;
END;
```

El parser detecta `BEGIN` tras `AS` y conmuta a modo block (depth tracking idéntico a procedure/trigger body). Cualquier otro token arranca expression body (compat X3b).

### 2. `RETURN expr` como sentinel

- Nueva `Statement::Return(ReturnStmt { value: Expr })`.
- `exec_return` evalúa la expresión, la guarda en `Engine.pending_return_value: Option<Value>` y lanza un error con mensaje sentinel `__GABYSQL_RETURN_SIGNAL__`.
- Mismo patrón que `EXIT_SIGNAL` de X4b — burbujea a través de IF/WHILE/FOR/CASE/BEGIN sin ceremonia adicional.
- `eval_user_func` (en modo block body) atrapa el sentinel, lee `pending_return_value`, restaura el valor previo y retorna `Value` al caller. Sin RETURN → devuelve `Value::Null`.
- `pending_return_value` se snapshot-tea por invocación (`prev_pending = take()` antes / `= prev_pending` al final) para soportar funciones que llaman funciones sin pisarse.

### 3. `RETURN` solo válido dentro de function body

Fuera de `eval_user_func`, el sentinel burbujea hasta `exec` top-level y se convierte en `[GBY-4118] RETURN fuera de function body`. Procedures no deberían usar `RETURN expr` (no tienen tipo de retorno).

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4118` | `RETURN_OUTSIDE_FUNCTION` | `RETURN expr` ejecutado fuera de un function body multi-statement. |

## 🧪 Validación

Suite `x4f_*` en `tests/integration_test.rs` (6 tests):

- `x4f_function_single_expr_body_still_works`: regression — body expression sigue funcionando (X3b).
- `x4f_function_multistmt_body_with_return`: `BEGIN ... RETURN x*2; END` retorna value.
- `x4f_function_early_return_in_if`: branches IF con RETURN early-exit.
- `x4f_function_without_return_returns_null`: function que cae al final sin RETURN → NULL.
- `x4f_function_with_loop_and_return`: `sum_to(10)` = 55 vía WHILE + RETURN al final.
- `x4f_function_calling_function_with_return`: `quad(x) = dbl(dbl(x))` — verifica restore de `pending_return_value`.

Suite total: **489/489 pass** (`cargo test --lib --tests`).

## 🔭 Futuro (post-X4f)

Con X4f cierra el bloque X (procedural completo: triggers + procedures + functions + IF/CASE/WHILE/FOR/LOOP + DECLARE/SET + RAISE + EXCEPTION + RETURN). Items menores que quedan diferidos:

- **`FOR row IN SELECT ... LOOP`**: composite row scope (`row.col`) — requiere extender var_scope con nested records.
- **`EXCEPTION WHEN <name>`**: filtros simbólicos (`WHEN no_data_found`) — tabla de mapeo nombre → código.
- **`CASE expr WHEN val THEN ...`** simple form como statement.
- **`RAISE` con formato `%`**: `RAISE EXCEPTION 'value % invalid', x`.
- **`STEP n` / `REVERSE`** en FOR range loops.
- **`RAISE WARNING/INFO`** (X4c solo cubre EXCEPTION/NOTICE).

Lo recomendado a partir de aquí es saltar a **Fase 3** (planner + EXPLAIN + benchmarks vs SQLite/PG/MySQL/DuckDB) o al bloque **Y** (tipos DECIMAL/BLOB/UUID), que dan más palanca producto-wise que pulir más PL/pgSQL.
