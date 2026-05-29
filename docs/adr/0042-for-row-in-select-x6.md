# ADR-0042: `FOR row IN (SELECT ...) LOOP` (X6)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: X6 (cierre del bloque X — último item diferido)
**Bump on-disk**: ninguno

## 🧭 Contexto

X4f cerró el grueso del control de flujo procedural. X5 limpió 4 items menores. Quedaba uno: **iterar sobre un resultset**, el patrón clásico de PL/pgSQL `FOR row IN SELECT ... LOOP`. Con esto el bloque X queda 100% cerrado.

La forma anterior de iterar valores SQL desde código procedural era forzada — `FOR i IN 1 TO N LOOP` solo da números secuenciales, y dentro del loop no podías leer una fila de una tabla sin escribir una subquery escalar por columna. X6 cierra esa brecha.

## 💡 Decisión

### Sintaxis

```sql
FOR row IN (SELECT col1, col2, ... FROM t [WHERE ...] [ORDER BY ...] [LIMIT ...]) LOOP
    -- dentro del body, row.col1 y row.col2 son variables
    ...
    EXIT WHEN row.col1 = 999;
END LOOP;
```

- El SELECT va **siempre entre paréntesis** — distinto a PG (que acepta sin paréntesis). Esto simplifica el parser: el lookahead `(SELECT` después de `IN` decide si es ForSelect o range loop (X4c/X5).
- El nombre del record (`row` arriba) es libre — cualquier ident. Sus columnas se exponen como `<name>.<col>` dentro del body.
- Cualquier SELECT válido sirve como source: con `WHERE`, `ORDER BY`, `LIMIT`, derived tables, etc. No hay restricción especial.

### AST

```rust
pub struct ForSelectStmt {
    pub var: String,
    pub query: SelectStmt,
    pub body: Vec<Statement>,
}
Statement::ForSelect(Box<ForSelectStmt>)
```

Hermano de `Statement::For(ForStmt)` (range loop) — no se reusa la misma struct porque las dos formas tienen shape totalmente distinta.

### Parser

`parse_for_stmt` ya parsea range loops. X6 sólo agrega un lookahead temprano:

```rust
if peek == "(" && peek+1 ~= "SELECT" {
    pos += 1;                         // consume "("
    expect_keyword("SELECT")?;        // parse_select_stmt asume SELECT ya consumido
    let query = self.parse_select_stmt()?;
    expect_symbol(")")?;
    expect_keyword("LOOP")?;
    let body = parse_loop_body()?;
    expect "END" "LOOP";
    return Ok(Statement::ForSelect(...));
}
// caer al range loop (X4c/X5)
```

### Engine

```rust
fn exec_for_select(&mut self, stmt: ForSelectStmt) -> DbResult<ResultSet> {
    let rs = self.exec_select(stmt.query)?;
    let prefix = format!("{}.", stmt.var.to_ascii_lowercase());
    let keys: Vec<String> = rs.columns.iter()
        .map(|c| format!("{}{}", prefix, c.to_ascii_lowercase()))
        .collect();
    let saved: Vec<(String, Option<Value>)> = keys.iter()
        .map(|k| (k.clone(), self.var_scope.remove(k)))
        .collect();
    for row in rs.rows {
        // MAX_LOOP_ITERATIONS guard
        for (k, v) in keys.iter().zip(row) {
            self.var_scope.insert(k.clone(), v);
        }
        // run body, handle EXIT sentinel
    }
    // restore saved
}
```

- **Composite scope = HashMap flat con claves qualified**. `row.id` y `row.name` viven en `var_scope` como `"row.id"` y `"row.name"`. No introducimos un `Record` value type — todo se mantiene escalar.
- **MAX_LOOP_ITERATIONS guard** heredado del resto de loops (100K). Defensa contra resultsets gigantes que escaparon al usuario.
- **Shadowing + restore** per record key. Si una variable `row.id` ya existía antes del FOR (raro, pero posible), se guarda y restaura.
- **EXIT y RETURN** propagan por sentinel, igual que en el resto de loops.

### `eval_expr` fast-path para Column qualified

`normalize_ident("row.id")` devuelve `"id"` (tira el qualifier). Eso rompería la resolución para `var_scope` flat con claves qualified. Fix mínimo en `eval_expr` del arm `Expr::Column`:

```rust
if name.contains('.') {
    let full = name.to_ascii_lowercase();
    if let Some(v) = row.get(&full) { return Ok(v.clone()); }
}
// ... resto del lookup normal ...
```

Es un fast-path puro. Si el nombre tiene `.`, se prueba el lookup completo antes de normalizar — sin colisiones con el código existente porque las claves de columnas reales no llevan `.` (las joineadas sí, y matchearían igual).

## 📐 Códigos de error

Ninguno nuevo. Reusa:
- `[GBY-4110]` `LOOP_MAX_ITERATIONS_EXCEEDED` (si el resultset excede 100K filas).
- `[GBY-2002]` `COLUMN_NOT_FOUND` si se referencia `row.col` con un nombre que no está en el resultset.
- Errores propios del SELECT interno (FK, type mismatch, etc.).

## 🧪 Validación

Suite `x6_*` en `tests/integration_test.rs` (8 tests):

- `x6_for_select_basic_iteration_counts`: 3 filas, cnt=3.
- `x6_for_select_reads_row_col_in_expr`: suma `r.val` (10+20+30=60).
- `x6_for_select_last_row_value_persists`: último `r.id` visto = 30.
- `x6_for_select_exit_when_works`: `EXIT WHEN r.id = 3` corta tras 3 iteraciones.
- `x6_for_select_empty_result_noop`: tabla vacía, body no se ejecuta.
- `x6_for_select_with_where_in_subquery`: SELECT con WHERE filtra correctamente.
- `x6_for_select_inside_procedure`: FOR ... IN (SELECT) dentro de `CREATE PROCEDURE`.
- `x6_for_select_row_scope_restored_after`: var declarada antes (`last_seen`) preservada después del loop.

Suite total: **530/530 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

Con X6, el bloque X queda 100% cerrado. Lo que sigue (Y3 = BLOB/DECIMAL exacto/SMALLINT range, Fase 3 = planner+EXPLAIN+benchmarks, Z = RLS/GRANT/REVOKE) ya no es X.

Refinamientos opcionales sobre X6 que podrían entrar después:

- `FOR row IN SELECT ... LOOP` **sin paréntesis** (alinear con PG).
- `FOREACH r SLICE n IN ARRAY ... LOOP` (requiere ARRAY type, espera Y3+).
- `FOR rec IN EXECUTE 'SELECT ...' LOOP` (dynamic SQL — requiere literal-string SELECT).
- Exponer `row` como un único valor JSON serializable (alternativa a las claves qualified).
