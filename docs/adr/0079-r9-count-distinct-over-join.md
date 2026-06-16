# ADR-0079: R9 — COUNT(DISTINCT col) sobre SELECT con JOIN

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Bloque:** R9 (reparación post-P5 — residual ADR-0066 Gap 1)
**Origen:** [docs/ANALISIS_POST_P5.md §3 R9](../ANALISIS_POST_P5.md) y
[docs/adr/0066-bench-exposed-gaps.md](0066-bench-exposed-gaps.md) Gap 1 residual.
**Relacionado:** F2 (count_distinct base implementation).

## Contexto

`COUNT(DISTINCT col)` sobre una tabla simple funciona desde F2
(`compute_aggregate` rama `(Count, DistinctColumn)` — encode_group_key
sobre cada valor no-NULL, deduplicar, contar).

Sobre `SELECT ... JOIN ... GROUP BY ...` rebotaba con error
`[GBY-4028] AGGREGATE_OVER_JOIN_UNSUPPORTED`:

```
COUNT(DISTINCT col) sobre SELECT con JOIN aún no se soporta;
usar subquery agregada sobre la tabla base
```

Causa: en el path single-table las filas se indexan por column key plano
(`"prod"`); en el path joined se indexan por column key cualificado
(`"o.prod"`). `compute_aggregate` (rama `DistinctColumn`) hace
`normalize_ident(col)` que devuelve el último segmento después del `.`
en minúsculas — `"o.prod"` → `"prod"`. El lookup `row.get("prod")`
sobre la fila joined falla porque la fila contiene `"o.prod"`. Resultado:
todos los valores resueltos a `Value::Null` y siempre devolvía 0,
pero antes de eso el path se cortaba con `[GBY-4028]` para evitar la
falla silenciosa.

## Decisión

Tratar `AggArg::DistinctColumn` en `exec_aggregate_joined` como un
agregado especial, evaluado inline sin pasar por `compute_aggregate`.

### Cambio local en `exec_aggregate_joined`

Se introduce un enum **interno** al método (no pub, no exportado) que
distingue dos paths para el `prepared_args`:

```rust
enum JoinedAggPrep {
    Standard(AggArg),     // dispatch normal a compute_aggregate
    DistinctExpr(Expr),   // count-distinct inline
}
```

El rewrite de `AggArg::DistinctColumn(c)` resuelve la columna a su
forma cualificada con `resolve_joined_column_key(scope, c)` —el mismo
helper que ya usaba `AggArg::Column(c)`— y guarda
`JoinedAggPrep::DistinctExpr(Expr::Column(qualified_key))`.

En el bucket loop, cuando el prepared es `DistinctExpr(expr)`:

```rust
if !matches!(func, AggFunc::Count) {
    return Err(coded(codes::AGGREGATE_OVER_JOIN_UNSUPPORTED,
        format!("DISTINCT solo es válido en COUNT, no en {:?}", func)));
}
let mut seen: HashSet<Vec<u8>> = HashSet::new();
for row in &bucket_rows {
    let v = eval_expr(expr, row)?;
    if matches!(v, Value::Null) { continue; }
    seen.insert(encode_group_key(&[v]));
}
Value::Integer(seen.len() as i64)
```

- `eval_expr` ya sabe resolver `Expr::Column("o.prod")` sobre la fila
  joined (fast-path por nombre completo, ver `eval_expr` rama
  `Expr::Column`).
- `encode_group_key(&[v])` reusa la misma función que el path
  single-table `(Count, DistinctColumn)` — semántica idéntica de
  deduplicación.
- NULL se ignora — coincide con el comportamiento single-table y con
  SQL ANSI.

### Cierres

- El error `AGGREGATE_OVER_JOIN_UNSUPPORTED` deja de dispararse para
  el caso COUNT(DISTINCT col). Sigue disponible para otros casos
  futuros (e.g. DISTINCT en SUM/AVG, no soportado por el parser hoy
  pero descartado defensivamente en el match).
- F2 sigue siendo el dispatch base para single-table — sin cambios.

## Consecuencias

### Positivas

- Cierra el residual de **ADR-0066 Gap 1** que quedaba abierto post-F2.
- El usuario puede escribir `COUNT(DISTINCT u.email)` sobre JOIN sin
  workarounds (la sugerencia previa "usar subquery agregada sobre la
  tabla base" deja de ser necesaria).
- Soporta `GROUP BY` también — el distinct se calcula por bucket.
- Cero cambios on-disk, zero-bump.

### Negativas / deuda

- `eval_expr` se llama una vez por fila del bucket — costo lineal,
  mismo orden que el path single-table. Sin index ni hash table
  global; cada bucket inicia con su propio `HashSet`.
- El enum `JoinedAggPrep` está scope-limited a `exec_aggregate_joined`
  — si el patrón se repite en otra función, conviene promoverlo a
  module-level (no hoy, evitar overdesign).
- El error mensaje cuando el parser emita `DistinctColumn` con un
  `func` distinto de `Count` (caso hipotético) ahora dice "DISTINCT
  solo es válido en COUNT, no en …" — el parser actual no lo emite,
  pero el match defensivo está por si cambia.

## Alternativas consideradas

1. **Agregar `AggArg::DistinctExpr(Expr)` a la enum pública.** Más
   reutilizable, pero requiere actualizar 4+ exhaustive matches
   (`output_name`, validación pre-bucket, window functions, etc.) por
   un caso que sólo aparece en joined-context. Overkill para R9.
2. **Re-keyar las filas joined a forma plana** antes de pasar al
   path single-table. Costo de memoria + bug surface si dos tablas
   tienen la misma columna. Rechazado.
3. **Llamar a `compute_aggregate` con el qualified key pero
   modificando `compute_aggregate` para no normalizar cuando ve un
   '.'.** Cambia comportamiento del dispatch base por un caso
   particular — fragiliza el helper. Rechazado.

## Tests

Tres tests nuevos (`r9_*` en `tests/integration_test.rs`):

- `r9_count_distinct_sobre_inner_join_sin_group_by` — INNER JOIN, sin
  GROUP BY. Resultado escalar (3 productos distintos).
- `r9_count_distinct_sobre_inner_join_con_group_by` — INNER JOIN +
  GROUP BY u.name. Distinct por bucket — `a→2, b→1, c→1`.
- `r9_count_distinct_ignora_nulls_sobre_left_join` — LEFT JOIN donde
  filas unmatched introducen NULL en `o.prod`. Solo 2 valores no-NULL
  contados.

Suite total: 801 → **804** (+3). Sin regresiones.

## Referencias

- [ADR-0066 — bench-exposed gaps](0066-bench-exposed-gaps.md) Gap 1 residual.
- [F2 — count_distinct base implementation](../../CHANGELOG.md) — entrada 2026-05-30.
- [TAREAS_PENDIENTES.md §4 R9](../TAREAS_PENDIENTES.md)
