# ADR-0037: `CASE` statement + `EXCEPTION WHEN <code>` (X4e)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: X4e (noveno sub-bloque del bloque X del roadmap)
**Bump on-disk**: ninguno

## 🧭 Contexto

X4d cerró el control de flujo procedural básico con `BEGIN..EXCEPTION..END` (catch-all `WHEN OTHERS`) y `LOOP` standalone. X4e suma dos refinamientos:

1. **`CASE WHEN ... THEN ... END CASE`**: statement-level (vs CASE expression que ya vive en `Expr::Case`).
2. **`EXCEPTION WHEN <code> THEN ...`**: filtros por código específico, además del `WHEN OTHERS` catch-all.

`RETURN expr` en functions y `FOR row IN SELECT` quedan para X4f (último previsto del bloque X).

## 💡 Decisión

### 1. `CASE` statement-level

```sql
CASE
    WHEN cond1 THEN stmts;
    WHEN cond2 THEN stmts;
    [ELSE stmts;]
END CASE;
```

- **Searched form solo** — sin operando inicial. La simple form (`CASE x WHEN v THEN ...`) se escribe como `CASE WHEN x = v THEN ...`.
- **Semánticamente idéntico a IF/ELSIF/ELSE/END IF** — el motor literalmente reusa el mismo algoritmo, solo cambia la sintaxis.
- `CASE` (statement) cierra con `END CASE`; `CASE` expression cierra con `END` solo. El parser distingue por contexto (parse_statement vs parse_expr).

### 2. `EXCEPTION WHEN <code> THEN ...`

```sql
BEGIN
    ...
EXCEPTION
    WHEN 4111 THEN <handler1>;      -- atrapa RAISE EXCEPTION user-triggered
    WHEN 3001 THEN <handler2>;      -- atrapa DUPLICATE_PRIMARY_KEY
    WHEN OTHERS THEN <fallback>;    -- catch-all (opcional, último)
END;
```

- **Múltiples WHEN encadenados** — se prueban en orden; primer filter que matchee corre su handler.
- **Filtro = literal entero** (`4111`, `3001`, etc.) — el código `[GBY-NNNN]` sin el prefijo. PG usa nombres simbólicos (`no_data_found`); X4e usa códigos numéricos por simplicidad. Lookup symbólico podría agregarse en el futuro.
- **`OTHERS`** atrapa cualquier error no atrapado por filtros previos — debe ir al final.
- Si ningún WHEN matchea (y no hay OTHERS), el error re-propaga.

**Cambio AST**: `BlockStmt.exception_handler: Option<Vec<Statement>>` → `BlockStmt.exception_handlers: Vec<(ExceptionFilter, Vec<Statement>)>`. Vacío = sin handlers = propaga. X4d siempre creaba 1 entry con `Others`; X4e admite N entries.

### 3. Helper `extract_error_code`

```rust
fn extract_error_code(msg: &str) -> Option<u32> { ... }
```

Parsea el prefijo `[GBY-NNNN]` de un mensaje de error y extrae el código. Si el mensaje no lleva el prefijo standard, retorna `None` y los filtros `Code(n)` no matchean (solo `Others` puede atrapar).

### 4. Splitter: `CASE` keyword también abre block

`split_statements` ya trackea `BEGIN`/`IF`/`LOOP`. X4e suma `CASE`:

- `CASE` abre depth +1 (salvo just_saw_end).
- `END CASE` cierra (END -1, CASE post-END es close-keyword, no abre).
- **CASE expression** también pasa por el mismo branch — abre +1 con CASE, cierra -1 con END (CASE expression no tiene `END CASE`, solo `END`). Sigue balanceado.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4116` | `CASE_STATEMENT_MALFORMED` | Falta WHEN/THEN/END CASE. |
| `GBY-4117` | `EXCEPTION_FILTER_INVALID` | Filtro WHEN no es OTHERS ni literal entero. |

## 🧪 Validación

Suite `x4e_*` en `tests/integration_test.rs` (8 tests):

- `x4e_case_statement_basic`: chain con 3 branches.
- `x4e_case_statement_else_falls_through`: ELSE ejecuta cuando ningún WHEN matchea.
- `x4e_case_statement_no_match_no_else`: no-op silencioso.
- `x4e_exception_when_specific_code`: handler filtra por código 4111.
- `x4e_exception_when_wrong_code_propagates`: filtro por código incorrecto deja propagar.
- `x4e_exception_multiple_when_others_fallback`: chain de WHEN con OTHERS al final.
- `x4e_case_in_procedure_body`: CASE dentro de procedure.
- `x4e_exception_handler_runtime_error_specific`: handler atrapa `[GBY-3001]` DUPLICATE_PRIMARY_KEY.

Suite total: **483/483 pass** (`cargo test --lib --tests`).

## 🔭 Futuro (X4f — último previsto del bloque X)

- **`RETURN expr` en functions**: requiere extender function body de Expr a Vec<Statement> con RETURN como sentinel.
- **`FOR row IN SELECT ... LOOP`**: composite row scope (`row.col`).
- **`EXCEPTION WHEN <name>`**: filtros simbólicos (`WHEN no_data_found`) además de los numéricos.
- **`CASE expr WHEN val THEN ...`**: simple form como statement (X4e solo searched).
- **`RAISE` con formato `%`**: `RAISE EXCEPTION 'value % invalid', x`.

Con X4e, el control de flujo procedural de gabysql cubre los patrones más usados de PL/pgSQL. Lo que queda (X4f y más allá) son features útiles pero menos demandados.
