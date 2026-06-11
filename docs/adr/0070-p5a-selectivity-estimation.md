# ADR-0070: P5a — infraestructura de estimación de selectividad

**Fecha:** 2026-06-11
**Estado:** Aceptado
**Bloque:** P5a (sub-tarea de P5 — planner-as-optimizer)
**Antecede:** P5c (cost-based index choice), P5d (JOIN reorder)
**Consume:** [ADR-0068](0068-p4-column-stats.md) (P4 column stats)

## Contexto

P4 entregó stats por-columna persistidas (`null_count`, `ndv` vía HLL,
MCV top-K, histograma equi-depth), pero el planner sigue ignorándolas
para decidir planes. Antes de wirearlas a decisiones (P5c: choice de
índice, P5d: JOIN reorder), necesitamos:

1. **Una capa que las consuma**: traducir stats + predicado → fracción
   estimada de filas.
2. **Validar empíricamente que funcionan**: la única forma honesta de
   saber si nuestras stats están sesgadas es verlas en acción
   estimando.
3. **Aislar riesgo**: si las estimaciones están mal y *ya* las usamos
   en el plan, regresiones de performance silenciosas. Si las
   estimaciones están mal y solo las *mostramos*, regresión cero.

P5a entrega el (1) y (2) sin tocar el plan. EXPLAIN gana
`est.match=K` que el usuario puede comparar contra los rows reales.
Cuando las estimaciones convencen al ojo humano, P5c las wirea.

## Decisión

### API: `estimate_selectivity`

```rust
fn estimate_selectivity(stats: &TableStats, expr: &WhereExpr) -> f64
```

Recorre el árbol `WhereExpr` (And/Or/Not/Atom) y devuelve fracción
`[0.0, 1.0]` de filas que se espera sobrevivan.

### Reglas por atom

| `WhereClause` | Estimación |
|---|---|
| `Eq{col, val}`, `val ∈ MCV` | `mcv_count(val) / row_count` (exacto) |
| `Eq{col, val}`, `val ∉ MCV` | `(1 − null_frac − mcv_frac) / (ndv − \|MCV\|)` |
| `Eq{col, NULL}` | `0.0` (3VL: `col = NULL` nunca true) |
| `IsNull{col, false}` | `null_count / row_count` (exacto) |
| `IsNull{col, true}` (`IS NOT NULL`) | `1 − null_count / row_count` |
| `Compare{<, ≤, >, ≥}` con histograma | scan + medio bucket parcial |
| `Compare{≠}` | `1 − sel(=)` |
| `Compare` sin histograma | `DEFAULT_RANGE_SELECTIVITY = 1/3` |
| `Between{col, from, to}` con histograma | scan `[from, to]` + half-bucket parcial |
| `Like{col, _}` | `DEFAULT_EQ_SELECTIVITY = 0.1` |
| `InList{col, vals}` | `Σ sel(eq col=v)`, clamped a 1.0 |
| `In{subquery}`, `Exists{}`, `EqSubquery`, `EqColumnRef`, `ExprPredicate` | `DEFAULT_EQ_SELECTIVITY = 0.1` |
| Cualquier predicado negado (`negated: true`) | `1 − sel` |

### Reglas de combinación

| `WhereExpr` | Estimación |
|---|---|
| `And(l, r)` | `sl × sr` (independencia) |
| `Or(l, r)` | `sl + sr − sl × sr` (inclusión-exclusión) |
| `Not(c)` | `1 − sel(c)` |

### Constantes

- `DEFAULT_EQ_SELECTIVITY = 0.1`. PostgreSQL usa el mismo número para
  operadores de igualdad sin stats. Mejor que 0.5 (que infla todo) y
  mejor que 0.01 (que subestima cuando realmente hay pocas filas).
- `DEFAULT_RANGE_SELECTIVITY = 1/3`. Convención clásica de Selinger
  et al. (System R, 1979).

### Histograma — selectivity por bucket

Para `col < v` (Lt), `col ≤ v` (Le), `col > v` (Gt), `col ≥ v` (Ge):

```
matches_all  = bucket queda ENTERAMENTE en el rango cubierto por el predicado
matches_none = bucket queda ENTERAMENTE fuera
otherwise    = solapa parcialmente → suma medio bucket
```

La heurística "medio bucket" (50%) para solapamiento parcial es
estándar en optimizers reales. Asume distribución uniforme dentro del
bucket, lo que el equi-depth busca aproximar pero no garantiza.

Para `Between [from, to]` la misma lógica con dos pivots:
`lower ≥ from && upper ≤ to` → full match; `upper < from || lower > to`
→ full miss; otherwise → medio bucket.

### Anotación EXPLAIN

`stats_annotation` extendida para aceptar `Option<&WhereExpr>`:

```
[est.rows=12 cols=2 est.match=6]  ← MCV exacto sobre code='a' en 6/12
[est.rows=100 cols=2 est.match=1] ← AND multiplicativo: 0.1 × 0.1
[est.rows=12 cols=2 est.match=8]  ← OR unión: 6/12 + 3/12 − producto
[est.rows=5  cols=2 est.match=3]  ← IS NULL exacto
```

El plan **NO se cambia**. EXPLAIN sigue mostrando el path real
(`PK lookup`, `composite index lookup`, `full scan`, ...) — solo
agrega la columna `est.match` cuando hay WHERE + stats.

## Alternativas consideradas

1. **No estimar selectividad ahora — saltar directo a P5c**.
   - Descartado: P5c necesita la API. Hacerla y testearla aparte da
     una superficie reviewable más chica y un check empírico (EXPLAIN
     muestra los números antes de wirearlos a decisiones).

2. **Implementar Selinger completo (correlación inter-columna,
   join cardinality estimation, ...)** desde el primer push.
   - Descartado: scope creep. Esto es la **base**; AND/OR/NOT
     bastan para WHERE sobre tabla única. JOIN cardinality se
     resuelve en P5d con sus propias reglas (multiplica las tablas y
     aplica selectividades de equi-joins).

3. **Usar muestras de Monte Carlo en lugar de fórmulas cerradas**.
   - Descartado: zero-deps + costo en tiempo. Sample-based estimation
     da mejores números pero requiere re-scan parcial en cada
     EXPLAIN. Las fórmulas cerradas con MCV+NDV+histograma cubren
     90% del valor a 0% del costo.

4. **Bias adjustment para HLL** (ej. usar offset empírico para corregir
   NDV cuando es chico).
   - Descartado por ahora: el `splitmix64` finalizer agregado en el
     hot-fix de P4 ya resolvió el sesgo más severo (sequential ints).
     Si P5c muestra que el remaining bias rompe decisiones, agregamos
     bias correction como hot-fix.

5. **`avg_per_bucket` en vez de half-bucket** para solapamiento parcial.
   - Considerado, no aplicado: equi-depth busca que cada bucket tenga
     `count` similar, así que `count/2` ≈ `avg/2`. La diferencia es
     marginal y agrega un parámetro de tuning.

## Tests

7 tests nuevos en `tests/integration_test.rs` (suite `p5a_*`):

- `p5a_explain_sin_where_no_muestra_est_match`: regression — sin WHERE,
  no aparece `est.match` (solo `est.rows cols=M`).
- `p5a_selectivity_eq_mcv_exacto`: `code='a'` con MCV count=6/12 →
  `est.match` ~ 6 (acepta 3..=8 por el AND extra con `id > 0`).
- `p5a_selectivity_is_null_exacto`: 3 NULLs reales → `est.match = 3`
  exacto.
- `p5a_selectivity_and_es_producto`: dos `=` independientes con
  sel=0.10 → AND=0.01 → `est.match` ≤ 3 (real = 1).
- `p5a_selectivity_or_union`: `code='a' OR code='b'` (6+3 de 12) →
  inclusión-exclusión ~ 0.625 → `est.match` en `[7, 10]`.
- `p5a_sin_stats_usa_default`: tabla sin ANALYZE → `est.match` no
  aparece (no hay stats, no annotation).
- `p5a_selectivity_between_usa_histograma`: BETWEEN 1..25 sobre 100
  valores uniformes → `est.match` en `[12, 40]` (real = 25, tolerancia
  ±50% por equi-depth de 16 buckets).

Suite total: **769 passing** (762 → +7 P5a). Verificado vía Docker
`rust:1.94-bookworm`.

## Consecuencias

**Positivas**

- (+) Las stats P4 dejan de ser "datos almacenados que nadie usa".
  EXPLAIN ahora muestra qué tan bien (o mal) estiman.
- (+) Riesgo de regresión de plan = **cero**. No cambia ningún path.
- (+) P5c (cost-based index choice) puede consumir `estimate_selectivity`
  inmediatamente — pull en lugar de push.
- (+) El usuario / desarrollador puede comparar `est.match` vs filas
  reales y reportar sesgos antes de que P5c los amplifique en
  decisiones de plan.

**Negativas / Limitaciones honestas**

- (-) Asunción de independencia entre columnas (`sl × sr`). En la
  realidad las columnas están correlacionadas — `marca = 'Toyota' AND
  modelo = 'Corolla'` no es independiente. Selinger reconoció esto en
  1979; el remedio (multi-column stats) está fuera del scope de P5a.
- (-) Histograma half-bucket es una aproximación grosera. Para
  predicados que caen en el medio de un bucket grande, el error puede
  ser ±50%.
- (-) `Compare` sobre TEXT con histograma: si bien el código lo soporta
  vía `cmp_stats_values`, no hemos validado el comportamiento sobre
  strings con histograma. Tests futuros.
- (-) Subqueries (`EqSubquery`, `Exists`, `In(subquery)`) usan el
  default fijo. Estimar cardinalidad de una subquery requiere recursión
  del estimador — diferido.
- (-) `Like 'X%'` no usa el histograma (podría: rango `[X, X+inf]`).
  Diferido — `Like` es relativamente raro en queries de catálogo.

## Limitaciones / Trabajo futuro

- **P5c**: cost-based index choice. Cuando hay múltiples paths
  posibles (FullScan, single-col index, composite index), elegir el
  de menor costo estimado = `est.match × cost_per_row(path)`.
- **P5d**: JOIN reorder. Estimar cardinalidad post-join con
  `est_join_rows = est.match(left) × est.match(right) × (1/ndv_join_col)`.
  Reorder commutative JOINs para minimizar tamaño intermedio.
- **Multi-column stats** (correlación): histograma 2D o
  funciones-de-dependencia (PostgreSQL `CREATE STATISTICS`).
- **EXPLAIN ANALYZE**: comparar `est.match` vs `actual=K`. Hoy ANALYZE
  ya re-ejecuta la query — agregar la comparación lado-a-lado.
- **Calibración**: si tras P5c se ve que la heurística half-bucket
  sesga sistemáticamente, pasar a interpolación dentro del bucket
  usando `lower/upper`.
