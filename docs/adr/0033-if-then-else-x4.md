# ADR-0033: Control de flujo `IF/THEN/ELSIF/ELSE/END IF` (X4)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: X4 (quinto sub-bloque del bloque X del roadmap)
**Bump on-disk**: ninguno (puro runtime; los bodies de trigger/procedure ya se persistían como texto SQL desde X1/X3)

## 🧭 Contexto

X1+X2 (triggers), X3 (procedures), X3b (functions) entregaron las 4 routines server-side básicas. Los bodies eran flat sequences of DMLs — sin control de flujo. X4 agrega lo más útil del lenguaje procedural: **condicionales**.

PL/pgSQL completo (variables, LOOP, EXCEPTION, FOR, FOREACH, CONTINUE) queda para X4b+.

## 💡 Decisión

### 1. Sintaxis canónica

```sql
IF condition THEN
    <stmts>;
[ELSIF condition THEN
    <stmts>;]*
[ELSE
    <stmts>;]
END IF;
```

Donde:

- `condition` es una `Expr` que debe evaluar a BOOL (NULL → FALSE, 3VL).
- `<stmts>` es una lista de statements separados por `;` (último `;` opcional). Cada stmt puede ser DML, otro `IF` (anidado), `CALL`, etc.
- `IF` es un **statement top-level** — funciona dentro de bodies de trigger/procedure y también en batches SQL planos.

### 2. AST: `Statement::If(Box<IfStmt>)`

```rust
pub struct IfStmt {
    pub branches: Vec<(Expr, Vec<Statement>)>,  // IF + ELSIF chain
    pub else_branch: Option<Vec<Statement>>,
}
```

El primer ítem de `branches` es el IF inicial; los subsiguientes son ELSIF. La evaluación toma el primer TRUE; si ninguno, ejecuta `else_branch` si existe.

### 3. Splitter de statements distingue `IF` block-open de `IF` función

`split_statements` ya trackea `BEGIN ... END` desde X2. X4 agrega `IF`:

- `IF` keyword en posición top-level → abre bloque (depth+=1).
- `IF` después de `END` (es decir, `END IF` keyword close) → no abre (consumido como parte del close).
- `IF` seguido de `(` (función escalar `IF(cond, a, b)`) → no abre.
- `IF NOT EXISTS` / `IF EXISTS` (DDL conditionals tipo `DROP TABLE IF EXISTS`) → no abre.

Se mantiene un flag `just_saw_end` para distinguir `END IF` (close) de un nuevo IF separado.

Los mismos cambios se replican en los **body parsers** de `CREATE TRIGGER` y `CREATE PROCEDURE` (que también necesitan trackear el block depth para saber dónde termina el body al CREATE-time).

### 4. Tokenizer: `IF`, `ELSIF`, `THEN` agregados a la lista de keywords no-operand

Pre-X4, el tokenizer trataba `IF`, `ELSIF`, `THEN` como idents "comunes" → un `-N` después se tokenizaba como `-` (operador) `N` (número), forzando aritmética en lugar de literal negativo. Eso rompía `IF -5 > 0 THEN` (`-5` no es operando válido tras `>`).

Fix: agregar las 3 keywords a la lista de "keywords que introducen un valor" (junto con `WHERE`/`AND`/`OR`/etc.). Ahora `IF -5 > 0` tokeniza como `IF, -5, >, 0` (literal negativo).

### 5. Engine: `exec_if`

```rust
fn exec_if(&mut self, stmt: IfStmt) -> DbResult<ResultSet> {
    let empty = HashMap::new();
    let chosen = stmt.branches.iter().find_map(|(cond, body)| {
        let v = self.eval_expr_full(cond, &empty, None)?;
        let truthy = match v { Bool(b) => b, Null => false, _ => err(4105) };
        if truthy { Some(body.clone()) } else { None }
    }).or(stmt.else_branch);
    if let Some(body) = chosen {
        for s in body { self.exec(s)?; }
    }
    Ok(no-op)
}
```

La condición se evalúa contra **row vacío** porque NEW/OLD/params ya fueron substituidos a nivel de TOKEN antes del parse del body. El engine no necesita scope.

### 6. Composición con triggers/procedures: sin cambios adicionales

`fire_triggers` y `exec_call` ya hacen substitución NEW/OLD/params + `parse(substituted_text)` → `Vec<Statement>` + iterate exec. Con X4, algunos de esos Statements son `Statement::If` — el dispatcher los maneja vía `exec_if`. Cero invasión del flujo X1/X3.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4105` | `IF_CONDITION_NOT_BOOLEAN` | Condición evalúa a algo que no es BOOL/NULL. |
| `GBY-4106` | `IF_BLOCK_MALFORMED` | Falta THEN, falta END IF, ELSIF/ELSE fuera de lugar, EOF prematuro. |

## 🧪 Validación

Suite `x4_*` en `tests/integration_test.rs` (9 tests):

- `x4_if_then_simple`: IF top-level con THEN branch que ejecuta.
- `x4_if_then_else`: IF FALSE → ELSE branch.
- `x4_if_elsif_else_chain`: chain con 3 ELSIF + 1 ELSE; matchea la tercera.
- `x4_if_in_trigger_body`: IF dentro de un trigger AFTER INSERT que classifica filas.
- `x4_if_in_procedure_body`: IF dentro de procedure body.
- `x4_nested_if`: IF anidado.
- `x4_if_condition_not_bool_rejected`: `[GBY-4105]`.
- `x4_if_without_end_rejected`: `[GBY-4106]`.
- `x4_if_with_new_in_trigger`: condición usa `NEW.v > 0` (post-subst funciona con valores negativos).

Suite total: **450/450 pass** (`cargo test --lib --tests`).

## 🔭 Futuro (X4b+)

- **Variables locales** (`DECLARE x INT [DEFAULT expr]`): el primer paso al PL/pgSQL completo. Requiere scope local en el engine.
- **Asignación** (`SET x = expr` o `x := expr`).
- **`WHILE cond LOOP ... END LOOP`** + **`LOOP ... EXIT WHEN cond ... END LOOP`**.
- **`FOR i IN a..b LOOP`** / **`FOR row IN SELECT ...`**.
- **`RAISE EXCEPTION` / `RAISE NOTICE`**: lanzar errores explícitos con mensaje.
- **`EXCEPTION WHEN ... THEN ...`**: handlers de errores.
- **`RETURN expr`** dentro de functions: alternativa al `AS <expr>` único.
- **CASE statement** (vs CASE expression que ya existe): `CASE expr WHEN ... THEN ... END CASE`.

Con X4, el bloque X cubre el 80% de los patrones server-side comunes — los casos restantes (variables, loops) son nicho dentro de SQL puro y suelen resolverse mejor con código aplicación.
