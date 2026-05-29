# ADR-0032: User-defined scalar functions (`CREATE FUNCTION RETURNS scalar`) (X3b)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: X3b (cuarto sub-bloque del bloque X del roadmap)
**Bump on-disk**: VERSION 15 → 16 (nuevo `ObjectKind::Function` en el catálogo)

## 🧭 Contexto

X3 ([ADR-0031](0031-stored-procedures-x3.md)) entregó `CREATE PROCEDURE` + `CALL` — encapsulación de side effects parametrizados como statement standalone. La pieza siguiente es **funciones escalares user-defined invocables desde expresiones** — `SELECT my_dbl(x) FROM t WHERE my_big(y)`.

Esto requiere extender el AST de `Expr` (un variant nuevo) y tocar todos los walkers/validators/serializers que pattern-matchean sobre `Expr`. ~17 lugares afectados.

## 💡 Decisión

### 1. Sintaxis canónica

```sql
CREATE FUNCTION name(p1 TYPE [, p2 TYPE]*) RETURNS TYPE AS <expr>;
DROP FUNCTION [IF EXISTS] name;

-- Invocable en cualquier expresión:
SELECT name(arg1, arg2) FROM t;
SELECT * FROM t WHERE name(col) >= 10;
SELECT inner(outer(x)) FROM t;
```

El body es **UNA expresión** (`<expr>`), no un SELECT. Esto es una **desviación práctica de ANSI** (PostgreSQL usa `RETURNS ... AS $$ SELECT ... $$ LANGUAGE SQL`). Razón: gabysql exige `FROM` en SELECTs, y forzar a los usuarios a escribir `SELECT 1+2 FROM dummy` para funciones triviales es ergonómicamente penoso. La forma `AS <expr>` es más natural y se evalúa contra row vacío.

### 2. AST: `Expr::UserFunc { name, args }`

Nuevo variant del enum `Expr`. El parser lo emite cuando `IDENT(args)` no matchea ningún `ScalarFunc` built-in (fallback). El executor lo resuelve via `eval_expr_full` → `eval_user_func` (catalog lookup + arity check + token-sub de params + parse del body + recursive eval).

**17 walkers de `Expr` actualizados** para manejar la nueva variant:

- `expr_default_label`, `format_expr` → serialize back to `name(arg, ...)`.
- `memoize_uncorrelated_scalar_subqueries`, `inline_cte_into_expr`, `substitute_new_old_in_expr`, `rewrite_expr_columns_for_join` → recurse into args.
- `validate_expr_columns` → validate args' columns against schema.
- `collect_check_columns` → reject (CHECK constraints can't use user functions; preserva pureza).
- `expr_contains_subquery` → returns `true` (forza el path engine-aware).
- `expr_contains_correlated_subquery` → recursivo en args.
- `eval_expr` (free) → error (requires engine).
- `eval_expr_full` (engine) → dispatch a `self.eval_user_func`.

### 3. Persistencia: `ObjectKind::Function` (VERSION 16)

Discriminator `4=Function`. Payload:

```
[name][return_type:u8][param_count:u16] · param_count × ([pname][ptype:u8]) · [body_sql]
```

Bump VERSION 15 → 16. V15 abierto por binario X3b+ rebota con `[GBY-1003]`.

### 4. Eval: substitución de params + re-parse del body como `Expr`

`eval_user_func`:

1. Lookup `FunctionMeta` por nombre → `[GBY-4103]` si no existe.
2. Validar arity → `[GBY-4104]` si difiere.
3. Evaluar cada `arg` con `eval_expr_full` contra el row actual + outer scope (permite que el arg sea una correlated subquery).
4. Bind `pname.to_ascii_lowercase() → Value`.
5. `substitute_params_in_sql_text` sobre `body_sql` (mismo helper que procedures — token-sub bare-ident).
6. `parse_expr_str(substituted)` → `Expr`.
7. `eval_expr_full` del body contra row vacío y outer_stack=None.
8. Return value.

**Composición**: dado que el body puede invocar a otras user functions (vía Expr::UserFunc tras parse), la composición funciona naturalmente — `eval_user_func` se llama recursivamente. Sin guard de recursión (futuro `MAX_FUNCTION_DEPTH` si se vuelve necesario).

### 5. Type checking: best-effort

Hoy NO se valida que el tipo del arg matchee con el tipo declarado del param, ni que el resultado del body matchee con `RETURNS`. El motor confía en que las operaciones downstream rebotan ante type mismatch. Es coherente con el approach de procedures.

### 6. Limitación heredada de procedures: choque param/columna

El token-sub de params es bare-ident. Workaround: prefijar (`p_x` en lugar de `x`). Documentado.

### 7. CHECK constraints rechazan user functions

`collect_check_columns` levanta error si el predicado de un `CHECK` contiene `Expr::UserFunc`. Razón: el body de la function podría hacer cualquier cosa y rompería la pureza del CHECK (re-validable al ALTER, etc.).

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4101` | `FUNCTION_NAME_COLLIDES` | Nombre colisiona con tabla / vista / trigger / procedure / function. |
| `GBY-4102` | `FUNCTION_BODY_INVALID` | Body vacío, params duplicados, falta `AS` / `RETURNS`. |
| `GBY-4103` | `FUNCTION_NOT_FOUND` | Invocación a function inexistente (también `DROP FUNCTION` sin `IF EXISTS`). |
| `GBY-4104` | `FUNCTION_ARITY_MISMATCH` | Args ≠ params declarados. |

## 🧪 Validación

Suite `x3b_*` en `tests/integration_test.rs` (9 tests):

- `x3b_simple_function_in_select`: `SELECT dbl(v) FROM t`.
- `x3b_function_in_where`: `WHERE big(v)`.
- `x3b_function_uses_builtin`: body usa `CONCAT('Hi ', p_name)`.
- `x3b_function_arity_mismatch`: `[GBY-4104]`.
- `x3b_function_not_found`: `[GBY-4103]`.
- `x3b_function_drop_works`: lifecycle.
- `x3b_function_persists`: sobrevive close del pager.
- `x3b_function_name_collision`: `[GBY-4101]`.
- `x3b_function_calling_function`: composición `quad(x) = dbl(dbl(x))`.

Plus actualizado `g1_errors_arity_type_unknown` para el nuevo error code (`FOO(1)` post-X3b emite `[GBY-4103]` en lugar de `[GBY-4037]`).

Suite total: **441/441 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

- **Body como SELECT** (ANSI-puro: `AS $$ SELECT expr $$`): aceptaría también `SELECT` con FROM/WHERE/etc., devuelve rows[0][0]. Útil si la function necesita consultar una tabla. Diferido hasta que tengamos `SELECT` sin FROM o un patrón claro de invocación.
- **`CREATE FUNCTION ... RETURNS TABLE`**: function que devuelve un resultset (table-valued function). Usable en `FROM`.
- **Type checking estricto** en arg/return.
- **Recursion guard** (`MAX_FUNCTION_DEPTH`).
- **`IMMUTABLE` / `STABLE` / `VOLATILE`** hints para que el planner cache resultados.
- **Body PL/pgSQL completo** (X4) — variables locales, IF/THEN, LOOP, EXCEPTION.

Con X3b, el cuarteto clásico de routines server-side queda completo:

| Routine | Statement | Resultado |
|---|---|---|
| Trigger | (auto-fire) | Side effects en respuesta a DMLs |
| Procedure | `CALL name(args)` | Side effects parametrizados |
| Function | `name(args)` en expr | Valor escalar, sin side effects |

Falta X4 (lenguaje procedural completo) para tener PL/pgSQL-like — área grande que probablemente requiere su propia spec.
