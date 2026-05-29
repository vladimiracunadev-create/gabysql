# ADR-0028: Window functions — `OVER (PARTITION BY ... ORDER BY ...)`

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: W3 (tercer y último sub-bloque del bloque W del roadmap)
**Bump on-disk**: ninguno (puro motor de proyección)

## 🧭 Contexto

Con W1 (CTEs no recursivas) y W2 (`WITH RECURSIVE`) entregadas, queda cerrar la pieza grande del bloque W: **window functions**. SQL las introduce con el operador `OVER (PARTITION BY ... ORDER BY ...)` aplicado a una función — el clásico `ROW_NUMBER() OVER (PARTITION BY region ORDER BY salary DESC)` para enumerar dentro de un grupo, `SUM(amount) OVER (ORDER BY date)` para totales corridos, `LAG/LEAD` para mirar fila anterior/siguiente, etc.

Es la primera vez que el motor proyecta valores que dependen de OTRAS filas (no sólo de la fila actual). Eso fuerza un pipeline distinto al del SELECT clásico.

## 💡 Decisión

### 1. Catálogo de funciones soportadas

Tres familias, todas en un solo push:

| Familia | Funciones | Args | ORDER BY |
|---|---|---|---|
| Ranking | `ROW_NUMBER`, `RANK`, `DENSE_RANK` | 0 | opcional |
| Bucket | `NTILE(n)` | 1 | **requerido** |
| Aggregate | `COUNT(*)`, `COUNT(expr)`, `SUM`, `AVG`, `MIN`, `MAX` | 0 o 1 | opcional (cambia semántica del frame) |
| Value | `LAG(expr[, offset[, default]])`, `LEAD(...)`, `FIRST_VALUE(expr)`, `LAST_VALUE(expr)` | 1..3 | `LAG`/`LEAD` requeridos |

### 2. WindowSpec mínimo

```
OVER ( [PARTITION BY expr {, expr}*]
       [ORDER BY    expr [ASC|DESC] {, expr [ASC|DESC]}*] )
```

**No** soportamos frame specs explícitas (`ROWS BETWEEN ... AND ...`, `RANGE`, `GROUPS`) ni window naming (`WINDOW w AS (...)`). El default de frame se aplica según familia (ver §3).

### 3. Defaults de frame (sin spec explícita)

- **Ranking** (`ROW_NUMBER`/`RANK`/`DENSE_RANK`/`NTILE`): no aplica frame, el valor es per-row dentro de la partition ordenada.
- **Aggregate** con `ORDER BY`: running aggregate (RANGE UNBOUNDED PRECEDING AND CURRENT ROW). `SUM(x) OVER (ORDER BY d)` da el acumulado.
- **Aggregate** sin `ORDER BY`: full partition. `SUM(x) OVER (PARTITION BY region)` da el total de la región.
- **`LAG`/`LEAD`/`FIRST_VALUE`**: per-row según offset / posición inicial.
- **`LAST_VALUE`**: full partition (**desviación de ANSI**, que usaría CURRENT ROW por defecto — contraintuitivo). Documentado y testeado.

### 4. Arquitectura del pipeline

El executor detecta windows en `stmt.columns` y deriva a `exec_window_select`. Pipeline:

```text
1. Validar: no GROUP BY/HAVING/aggregate clásico mezclado ([GBY-4090])
2. Validar arity y `ORDER BY` obligatorio por función
3. Clonar stmt con columns=[Star] + sin ORDER BY/LIMIT/OFFSET
4. Ejecutar via exec_select_query → ResultSet con TODAS las filas source
5. Convertir a Vec<HashMap<String, Value>> (con keys cualificadas + suffix)
6. Por cada window item:
     - Particionar índices por partition_by exprs (clave string-encoded)
     - Ordenar cada partition por order_by exprs
     - Computar window value per row → Vec<Value>
7. Proyectar fila a fila: Column lookup / Expression eval / Window lookup precomputado
8. Aplicar el ORDER BY/LIMIT/OFFSET original sobre la proyección
```

La materialización completa es el costo: O(N) memoria en el peor caso. Para SELECTs grandes esto va a doler — futura optimización por streaming queda fuera del scope inicial.

### 5. Mezcla con GROUP BY: explícitamente rechazada

```sql
-- ❌ [GBY-4090]
SELECT region, SUM(amount), ROW_NUMBER() OVER (ORDER BY region) FROM sales GROUP BY region;
```

Workaround: envolver el GROUP BY en una derived table y aplicar la window sobre el resultado.

```sql
-- ✅
SELECT region, total, ROW_NUMBER() OVER (ORDER BY total DESC) AS rk
FROM (SELECT region, SUM(amount) AS total FROM sales GROUP BY region) AS agg;
```

### 6. Window functions en otros contextos: no permitidas

Solo el SELECT list del SELECT top-level acepta windows. WHERE / HAVING / ORDER BY-expr-del-mismo-select / body de CTE recursive / CHECK constraints todos rechazan con `[GBY-4091]` (el parser las acepta y el executor las rebota cuando intenta evaluarlas fuera del windowing path).

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4087` | `WINDOW_FUNCTION_UNKNOWN` | (Reservado para nombres no reconocidos — hoy el parser no se equivoca; futuro para extensibilidad.) |
| `GBY-4088` | `WINDOW_REQUIRES_ORDER_BY` | `LAG`/`LEAD`/`NTILE` sin `ORDER BY` dentro del OVER. También: nombre de función always-window sin `OVER`. |
| `GBY-4089` | `WINDOW_ARG_MISMATCH` | Arity incorrecta (`ROW_NUMBER(x)`, `LAG(a,b,c,d)`, etc.). `DISTINCT` dentro de window aggregate. |
| `GBY-4090` | `WINDOW_NOT_ALLOWED_WITH_GROUP_BY` | Mezcla con `GROUP BY` / `HAVING` / agregados clásicos. |
| `GBY-4091` | `WINDOW_NOT_ALLOWED_HERE` | Window en `RETURNING`, etc. |

## 🧪 Validación

Suite `w3_*` en `tests/integration_test.rs` (12 tests):

- `w3_row_number_no_partition`: numeración global por orden de `v`.
- `w3_row_number_partitioned`: numeración dentro de cada `region`.
- `w3_rank_vs_dense_rank_with_ties`: `RANK` salta, `DENSE_RANK` no.
- `w3_running_sum`: total corrido con `SUM(...) OVER (ORDER BY ...)`.
- `w3_full_partition_sum_no_order`: total de partition sin ORDER BY.
- `w3_lag_and_lead_default`: fila anterior/siguiente con NULL en bordes.
- `w3_first_and_last_value`: extremos de la partition ordenada (LAST_VALUE = full-partition).
- `w3_ntile_distributes_evenly`: 7 filas en 3 buckets = 3+2+2.
- `w3_count_star_running`: COUNT(*) corrido.
- `w3_window_with_group_by_rejected`: mezcla → `[GBY-4090]`.
- `w3_lag_without_order_by_rejected`: → `[GBY-4088]`.
- `w3_avg_running`: AVG corrido.

Suite total: **407/407 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

- **Frame specs explícitas** (`ROWS BETWEEN N PRECEDING AND M FOLLOWING`, `RANGE`, `GROUPS`).
- **`WINDOW w AS (...)`**: dar nombre a una window spec y reusarla — útil cuando varias columnas comparten la misma cláusula.
- **`PERCENT_RANK`, `CUME_DIST`**: funciones de distribución.
- **Optimización de memoria**: hoy materializamos todas las filas source. Para queries con `LIMIT N` post-window, podríamos cortar antes — requiere coordinar con el ORDER BY del outer.
- **Mezcla con GROUP BY** en el mismo SELECT: requiere un planner que sepa que las windows se computan POST-GROUP-BY sobre el resultset agregado.
- **`LAST_VALUE` con frame ANSI** (CURRENT ROW por default con ORDER BY): cuando lleguen frames explícitos, queda automáticamente.

Con W3 cierra el **bloque W completo** — CTEs (W1+W2) y window functions (W3). Próximas piezas grandes son **Fase 3** (planner + EXPLAIN + comparativa con SQLite/PG/MySQL/DuckDB) y los bloques **X** (triggers + stored procs), **Y** (tipos faltantes: DECIMAL/BLOB/UUID), **Z** (control de acceso).
