# ADR-0034: Variables + `WHILE LOOP` + `EXIT [WHEN]` (X4b)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: X4b (sexto sub-bloque del bloque X del roadmap)
**Bump on-disk**: ninguno (puro runtime)

## 🧭 Contexto

X4 ([ADR-0033](0033-if-then-else-x4.md)) entregó el control de flujo básico (`IF/THEN/ELSIF/ELSE/END IF`). X4b cierra el otro pilar del lenguaje procedural: **variables locales + loops**.

`RAISE EXCEPTION`, exception handlers, `FOR` loops, `CASE` statement quedan para X4c.

## 💡 Decisión

### 1. Sintaxis canónica

```sql
DECLARE name TYPE [DEFAULT expr];
SET name = expr;

WHILE cond LOOP
    <stmts>;
END LOOP;

EXIT [WHEN cond];
```

Todos son statements top-level — funcionan en batches SQL directos y dentro de bodies de trigger/procedure.

### 2. Scope plano (no anidado en X4b)

`Engine` gana un campo `var_scope: HashMap<String, Value>` — un único scope plano compartido entre todos los frames de la sesión. **Limitación documentada**: variables declaradas en una procedure son visibles desde otra procedure invocada después. PG/PL/pgSQL tiene scope anidado por BEGIN..END block; X4b no — es nuestra simplificación.

Workaround para evitar contaminación: usar nombres prefijados (`my_proc_i`).

`DECLARE` redeclara → `[GBY-4108]`. `SET` sin DECLARE previo → `[GBY-4107]`.

### 3. Variables visibles en `Expr` (NO en INSERT VALUES)

Cuando `eval_expr_full` se llama con `var_scope` no-vacío, hace un merge: las claves del `row` ganan, las del `var_scope` aparecen como fallback. `Expr::Column("i")` lookup → primero row, luego var_scope.

**Limitación importante**: `INSERT INTO t VALUES (i)` NO funciona — el parser de VALUES exige Value literal, no Expr. La variable `i` aparece como Ident que el `expect_value` rechaza.

**Workaround**:
- Usar `INSERT INTO t SELECT i FROM (VALUES (1)) AS x` (SELECT subquery acepta Expr).
- O usar `UPDATE` (cuyo SET acepta Expr).
- O usar el `param` de procedure que SÍ se substituye a literal en CALL.

Este límite refleja la asimetría real de gabysql donde `INSERT VALUES` es más restrictivo que `INSERT SELECT`. Lift de esa restricción está fuera de scope de X4b.

### 4. `WHILE cond LOOP <body> END LOOP`

Itera mientras `cond` evalúa a TRUE (NULL → FALSE, 3VL). Guard duro `MAX_LOOP_ITERATIONS = 100_000`. Si supera → `[GBY-4109]`. Causa típica: SET que no muta la variable de control.

El body se ejecuta como sequence de Statements. `EXIT` viaja como sentinel `DbError` que `exec_while` atrapa silenciosamente y termina el loop.

### 5. `EXIT [WHEN cond]`

`EXIT` sin WHEN sale incondicionalmente. `EXIT WHEN cond` evalúa la cond y sale sólo si TRUE.

Implementación via "sentinel error": `exec_exit` retorna `Err(DbError::new(EXIT_SIGNAL))` donde `EXIT_SIGNAL` es una constante de string. `exec_while` matchea esa string y termina. Si el EXIT escapa de cualquier WHILE (porque no hay loop activo), el error sigue propagándose pero idealmente debería convertirse a `[GBY-4110] EXIT_OUTSIDE_LOOP` — en X4b queda como string interno (cosmético).

### 6. Splitter y body parsers extendidos

`split_statements` ya trackea `BEGIN ... END` (X2) y `IF ... END IF` (X4). X4b agrega:

- `WHILE ... END LOOP`: WHILE incrementa depth (salvo si just_saw_end). LOOP keyword reset just_saw_end. El END decrementa, el siguiente LOOP queda como ident normal.
- `END LOOP` se distingue de `END IF` solo por el keyword post-END.

Los body parsers de CREATE TRIGGER y CREATE PROCEDURE también reconocen WHILE como block-open (igual que IF).

### 7. Type coercion en DECLARE/SET: best-effort

- `DECLARE x FLOAT DEFAULT 5`: el `5` (Integer) se promueve a `Value::Float(5.0)`.
- Otros mismatches (e.g. `DECLARE x INT DEFAULT 'text'`): se guardan como vienen, type info perdida. Document.

Type checking estricto queda para futuro.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4107` | `VARIABLE_NOT_DECLARED` | `SET` sobre variable sin DECLARE previo. |
| `GBY-4108` | `VARIABLE_REDECLARED` | `DECLARE` sobre nombre ya declarado en el scope. |
| `GBY-4109` | `LOOP_MAX_ITERATIONS_EXCEEDED` | WHILE > 100K iteraciones. |
| `GBY-4110` | `EXIT_OUTSIDE_LOOP` | Reservado — actualmente EXIT fuera de loop propaga el sentinel string. |

## 🧪 Validación

Suite `x4b_*` en `tests/integration_test.rs` (8 tests):

- `x4b_declare_and_set`: DECLARE + SET + IF que lee la variable.
- `x4b_while_loop_counter`: counter loop con SET y check intra-loop.
- `x4b_exit_when`: EXIT WHEN cond.
- `x4b_set_undeclared_var_rejected`: `[GBY-4107]`.
- `x4b_redeclare_rejected`: `[GBY-4108]`.
- `x4b_while_max_iter_guard`: `[GBY-4109]`.
- `x4b_declare_in_procedure_body`: DECLARE + WHILE dentro de procedure (documenta workaround de INSERT VALUES + param).
- `x4b_exit_unconditional`: EXIT sin WHEN.

Suite total: **458/458 pass** (`cargo test --lib --tests`).

## 🔭 Futuro (X4c+)

- **`RAISE EXCEPTION ...` / `RAISE NOTICE ...`**: aborto explícito con mensaje.
- **`EXCEPTION WHEN ... THEN <body>`**: catch handlers.
- **`FOR i IN a..b LOOP`**: foreach con auto-increment.
- **`FOR row IN SELECT ... LOOP`**: iteración sobre resultset.
- **`LOOP ... END LOOP`** standalone (sin WHILE), terminado por EXIT.
- **`RETURN expr`** dentro de functions.
- **`CASE` statement** (vs CASE expression).
- **Nested scope** real (BEGIN..END como block scope, no solo split boundary).
- **Type checking estricto** en DECLARE/SET.
- **Variables en INSERT VALUES** — requiere lift de la restricción de VALUES → ValueExpr o pre-eval de Expr literal en parse_value.

Con X4b, gabysql cubre los patrones procedurales más comunes del lado servidor. Lo que queda (X4c) son features útiles pero menos demandados — alcanzar PL/pgSQL completo es un objetivo de proyecto separado.
