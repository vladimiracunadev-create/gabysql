# ADR-0027: `WITH RECURSIVE` — fixpoint base+step con delta semantics

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: W2 (segundo sub-bloque del bloque W del roadmap)
**Bump on-disk**: ninguno (la materialización vive solo en runtime)

## 🧭 Contexto

W1 ([ADR-0026](0026-cte-non-recursive.md)) entregó las CTEs no-recursivas como inlining en parse-time — cada `FROM cte` se substituía por una derived table clonada del body. Ese truco no funciona para `WITH RECURSIVE`: el body de una CTE recursive se refiere a sí mismo, así que un inlining ingenuo entra en loop infinito.

W2 implementa el algoritmo de fixpoint estándar (base + step iterativo) y reusa el mismo bridge de W1 para inyectar el resultado final en el body principal.

## 💡 Decisión

### 1. Sintaxis canónica restringida

Sólo aceptamos la forma:

```sql
WITH RECURSIVE name AS (
    <anchor_select>     -- proyecta el schema base
    UNION [ALL]
    <step_select>       -- referencia `name` en algún FROM
)
<body_select>
```

- **Una sola CTE recursive por statement.** Multi (`WITH RECURSIVE a AS (...), b AS (...)`) rechazado con `[GBY-4082]`. La mezcla con CTEs no-recursive en el mismo `WITH` también queda diferida.
- **Body canónico = `UNION [ALL]` de dos SELECTs.** Cualquier otra forma (un único SELECT, set ops anidadas, INTERSECT/EXCEPT) rebota con `[GBY-4086]`.
- **Column aliases en la cabecera** (`WITH RECURSIVE name(c1, c2) AS (...)`) rechazados con `[GBY-4081]` — mismo workaround que W1: aliasar dentro del anchor.

### 2. Algoritmo de fixpoint con **delta semantics**

```text
accum := exec(anchor)            // set inicial
delta := accum                   // "filas nuevas" de la última iteración
seen  := { anchor_rows }         // sólo si UNION (no ALL)

loop:
    if delta vacío → terminar
    if iter ≥ MAX_ITER → [GBY-4083]
    if |accum| ≥ MAX_ROWS → [GBY-4084]

    step_iter := step.clone() con `FROM name` reescrito a VALUES(delta)
    new_rows  := exec(step_iter)
    if arity(new_rows) ≠ arity(anchor) → [GBY-4085]

    new_delta := new_rows
        filtradas por `seen.insert(row_key)` si UNION (no ALL)
    accum.extend(new_delta)
    delta := new_delta
```

Donde `MAX_ITER = 1000` y `MAX_ROWS = 100_000`. Los guards son cinturones de seguridad — recursión bien terminada no debería acercarse.

**Por qué delta y no cumulative**: ANSI requiere delta. PostgreSQL lo implementa así. SQLite también. Es la opción correcta para recursión lineal (cada iteración procesa solo las filas más nuevas) y termina naturalmente para queries bien formadas.

### 3. Bridge a través del inlining de W1

Después del fixpoint, el `accum` final se convierte a un `SelectStmt` con `values_source = Some((ValuesClause de Vec<Vec<Expr::Literal>>, anchor.columns))` vía `rows_to_values_select`. Ese SelectStmt sintético se inyecta al `body` reusando `inline_cte_into_query` de W1.

El executor downstream ve una derived table con `VALUES` — infraestructura entregada en el bloque I. No necesita saber que hubo recursión.

**Caso degenerado: `rows.is_empty()`.** ANSI `VALUES` exige ≥1 fila, así que generamos un wrapper `(SELECT * FROM (VALUES (NULL, NULL, ...)) AS t(c1, c2) LIMIT 0)` que preserva el schema con cero filas.

### 4. Dedup vía `format!("{:?}", row)`

`Value` no implementa `Hash`/`Eq` por la variant `Float` (NaN rompe reflexividad). Para `UNION` (no `ALL`), usamos `HashSet<String>` con clave `format!("{:?}", row)` — estable dentro de un proceso, suficiente para dedup. La opción más correcta (`OrderedFloat` o key bytes) queda diferida.

### 5. Sin bump de formato on-disk

La materialización vive en memoria de la transacción. Ninguna estructura nueva en el `Catalog`, ningún byte nuevo en `TableMeta`. `VERSION` se mantiene en 13.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4082` | `RECURSIVE_CTE_MULTIPLE_NOT_SUPPORTED` | Más de una CTE recursive en el mismo `WITH`. |
| `GBY-4083` | `RECURSIVE_CTE_MAX_ITERATIONS_EXCEEDED` | Fixpoint pasó las 1000 iteraciones. Falta condición de corte. |
| `GBY-4084` | `RECURSIVE_CTE_MAX_ROWS_EXCEEDED` | Fixpoint acumuló 100K filas. Mismo diagnóstico. |
| `GBY-4085` | `RECURSIVE_CTE_SCHEMA_MISMATCH` | Step proyecta arity distinta al anchor. |
| `GBY-4086` | `RECURSIVE_CTE_BODY_NOT_UNION` | Body de la CTE no es la forma canónica `anchor UNION [ALL] step`. |

El código `4080` (`CTE_RECURSIVE_NOT_SUPPORTED` de W1) queda **retirado** — el slot se mantiene reservado para no reciclarlo.

## 🧪 Validación

Suite `w2_*` en `tests/integration_test.rs` (8 tests):

- `w2_number_generator_union_all`: caso canónico `1..N` con `UNION ALL` y corte `WHERE n < 5`.
- `w2_union_dedups_naturally`: step produce constante; `UNION` dedup ⇒ termina natural en 1 iteración.
- `w2_max_iterations_guard`: recursión sin corte rebota con `[GBY-4083]`.
- `w2_body_not_union_rejected`: CTE recursive con body no-UNION → `[GBY-4086]`.
- `w2_multiple_recursive_rejected`: multi → `[GBY-4082]`.
- `w2_schema_mismatch_rejected`: anchor 1 col, step 2 col → `[GBY-4085]`.
- `w2_recursive_visible_in_body_joins`: la CTE materializada es JOINeable desde el body con una tabla persistente.
- `w2_hierarchy_traversal`: clásico descendientes de un árbol vía recursión.

Suite total: **395/395 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

- **Múltiples CTEs recursive** y mezcla con no-recursive en el mismo `WITH`: requiere un scope ordenado y un walker que distinga referencias.
- **Cumulative semantics como opción**: para queries no-lineales (joins de la CTE consigo misma sobre el acumulado), PostgreSQL permite leer del accum. Diferido — la mayoría de los casos prácticos funcionan con delta.
- **Optimización**: hoy cada iteración recompila el step y materializa una VALUES. Para CTEs grandes esto es O(n²). Una representación más eficiente (cursor o iterator) está en el radar.
- **`UNION` con `ORDER BY`/`LIMIT` dentro del step**: hoy soportado vía passthrough del executor, sin validación específica.
