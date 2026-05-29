# ADR-0026: CTEs no-recursivas (`WITH ... AS (SELECT ...)`)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: W1 (primer sub-bloque del bloque W del roadmap)
**Bump on-disk**: ninguno (las CTEs viven solo en runtime — no se persisten)

## 🧭 Contexto

El bloque W del roadmap agrupa **CTEs** y **window functions** como una sola entrega "muy alta complejidad". Ese bundle es demasiado grande para un push siguiendo el workflow `1 bloque = 1 push`. Lo partimos en tres sub-bloques independientes:

- **W1** (este ADR): CTEs no-recursivas. P1 según [`docs/MISSING_COMMANDS.md`](../MISSING_COMMANDS.md) §11.
- **W2** (futuro): `WITH RECURSIVE` (fixpoint base+step sobre UNION ALL). P3.
- **W3** (futuro): window functions (`ROW_NUMBER`/`RANK`/`SUM() OVER (PARTITION BY ...)`). P2.

W1 cierra la pieza más común y de mayor valor: dar nombre a una subquery para reusarla, partir queries grandes en piezas legibles, y encadenar CTEs (`WITH a AS (...), b AS (SELECT FROM a) ...`).

## 💡 Decisión

### 1. Inlining en parse-time como derived tables

En lugar de extender el AST con un campo `ctes: Vec<CommonTableExpr>` en `SelectStmt` y propagar un "CTE scope" recursivamente por todo el `Engine` (invasivo, toca subqueries, JOIN scope, materialización), el parser **reescribe el AST**: toda referencia bare a un nombre de CTE en cualquier `FROM` / `JOIN` / subquery se substituye por un `derived_source` clonado del body de la CTE.

Después del parse, el `SelectStmt` resultante es indistinguible de uno donde el usuario hubiera escrito las subqueries inline. El executor no necesita saber que existen CTEs — ya las "ve" como derived tables, infraestructura entregada en el bloque H.

**Trade-off**: si una CTE se referencia N veces, su body se materializa N veces (re-ejecución). Para W1 es aceptable; la optimización (memoización por nombre, similar al fix Issue #1 del [BENCHMARK](../../BENCHMARK.md)) queda como deuda explícita.

### 2. Resolución de nombres y precedencia sobre tablas reales

La CTE **gana** sobre cualquier tabla del catálogo con el mismo nombre. La reescritura del AST sucede ANTES de la expansión de vistas (`expand_view_in_from`) y antes del catalog lookup en el executor, así que el `FROM cte_name` queda transformado en derived_source y nunca se consulta el catálogo. Esto matchea la semántica ANSI y la de PostgreSQL/SQLite.

Las referencias DENTRO del body de una CTE al MISMO nombre **NO se reescriben** (eso sería self-recursión, que requiere `WITH RECURSIVE`). El orden en `inline_cte_into_select` es deliberado: primero recursamos en `derived_source` pre-existente, recién después instalamos el nuevo derived. Una vez instalado, no volvemos a recursar dentro (el body ya fue procesado contra las CTEs declaradas ANTES en la misma cláusula `WITH`).

### 3. Encadenamiento de CTEs

`WITH a AS (...), b AS (SELECT * FROM a)` funciona: cuando parseamos el body de `b`, recorremos las CTEs previamente registradas (`a`) e inlineamos sus referencias en el body de `b` antes de guardar `b`. El orden de declaración es estricto — `b` puede usar `a` pero `a` no puede usar `b` (forward references rechazadas implícitamente porque `b` no está en la lista cuando se parsea `a`).

### 4. Set ops como query principal

`WITH cte AS (...) SELECT ... UNION SELECT ... FROM cte` está soportado: después del `WITH` parseamos la query principal vía `parse_select_stmt` + `parse_set_ops_after`, obteniendo un `SelectQuery` que puede ser un árbol de `SetOp`. El walker `inline_cte_into_query` recorre todas las ramas (`SelectStmt` hojas del árbol) e inlinea las CTEs en cada una. Las dos ramas del UNION ven la misma CTE.

### 5. Pendientes diferidos (rechazados con código explícito)

- **`WITH RECURSIVE`** → `[GBY-4080]`. Requiere fixpoint base+step y detección de convergencia. Bloque W2.
- **Column aliases en la cabecera** (`WITH cte(c1, c2) AS (SELECT x, y FROM t)`) → `[GBY-4081]`. Workaround documentado en el mensaje del error: aliasar dentro del body (`SELECT x AS c1, y AS c2 FROM t`) — semánticamente equivalente.
- **Nombres duplicados** dentro del mismo `WITH` → `[GBY-4079]`. Lookup case-insensitive.

### 6. Sin bump de formato on-disk

Las CTEs viven solo en el AST en runtime. No tocan `Catalog`, no agregan slots a `TableMeta`, no cambian la serialización. La constante `VERSION` se mantiene en 13 (V de vistas). Una BD V13 abierta por un binario sin soporte de W1 simplemente devolvería "tabla no existe" al ver `FROM cte_name` — fail-safe.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4079` | `CTE_DUPLICATE_NAME` | Dos CTEs con el mismo nombre dentro del mismo `WITH` (case-insensitive). |
| `GBY-4080` | `CTE_RECURSIVE_NOT_SUPPORTED` | `WITH RECURSIVE` — diferido a W2. |
| `GBY-4081` | `CTE_COLUMN_ALIASES_NOT_SUPPORTED` | `WITH name(c1, c2) AS (...)` — diferido. Workaround inline. |

## 🧪 Validación

Suite `w1_*` en `tests/integration_test.rs` (10 tests):

- `w1_single_cte_in_from`: caso base.
- `w1_cte_referencing_previous_cte`: encadenamiento.
- `w1_cte_in_join`: CTE como RHS de un INNER JOIN.
- `w1_cte_in_subquery`: CTE referenciada desde un `WHERE col IN (SELECT FROM cte)`.
- `w1_cte_shadows_real_table`: name resolution prioriza la CTE.
- `w1_cte_with_aggregate`: CTE con `GROUP BY` + `SUM`.
- `w1_cte_in_set_op_branch`: CTE visible desde ambas ramas de un `UNION`.
- `w1_cte_duplicate_name_rejected`: `[GBY-4079]`.
- `w1_cte_recursive_rejected`: `[GBY-4080]`.
- `w1_cte_column_aliases_rejected`: `[GBY-4081]`.

Suite total: **387/387 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

- **W2**: `WITH RECURSIVE` con detección de convergencia y guard de profundidad.
- **W3**: window functions.
- **Optimización**: memoización por nombre de CTE para evitar la re-materialización cuando se referencia N>1 veces (mismo patrón que el fix Issue #1 del benchmark para scalar subqueries no correlacionadas).
- **Column aliases en la cabecera**: implementables vía wrapping de cada `SelectItem` del body con `SelectItem::Expression { alias: Some(...) }` cuando la proyección es explícita; con `SELECT *` requiere conocer el schema del body, lo cual recién está disponible post-materialización.
