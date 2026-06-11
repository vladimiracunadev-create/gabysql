# ADR-0071: P5c — cost-based fallback a FullScan cuando selectividad es alta

**Fecha:** 2026-06-11
**Estado:** Aceptado
**Bloque:** P5c (sub-tarea de P5 — planner-as-optimizer)
**Consume:** [ADR-0070](0070-p5a-selectivity-estimation.md) (selectividad P5a)
**Antecede:** P5d (JOIN reorder)

## Contexto

P5a entregó `estimate_selectivity` y EXPLAIN mostraba `est.match=K`
pero el plan seguía inalterado. Caso degradado típico: una tabla de
1k filas con `CREATE INDEX idx_cat ON t (category)` y la query
`WHERE category = 'common'` donde `'common'` cubre 60% de las filas.

Path actual: index lookup devuelve 600 PKs → 600 random reads del
B+tree principal → cada random read cuesta ~5x un read secuencial.
Total ~3000 read-equivalentes secuenciales.

Path FullScan: 1000 reads secuenciales + filtro CPU. Total ~1000.
**3x más rápido** pero el planner lo ignoraba.

P5c implementa el switch automático: si las stats P4 dicen que
`est.match / row_count ≥ INDEX_BREAKEVEN_SELECTIVITY` (~0.2), fuerza
`Plan::FullScan + post-filter`.

## Decisión

### Cost model implícito

```
FullScan = row_count × C_SEQ
Index    = log(row_count) × C_LOG + est.match × C_RANDOM
```

Con `C_RANDOM ≈ 5 × C_SEQ` (SSD; peor en HDD ~10x) y `log` despreciable
a escalas relevantes:

```
Index gana sólo si est.match / row_count < C_SEQ / C_RANDOM ≈ 0.2
```

→ `INDEX_BREAKEVEN_SELECTIVITY = 0.2`.

### Detección

```rust
let p5c_skip_index: bool = !composite_pk_fast_path_active
    && stmt.where_clause.as_ref()
        .and_then(|expr| {
            let stats = self.table_stats.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&meta.name))
                .map(|(_, v)| v)?;
            let sel = estimate_selectivity(stats, expr);
            Some(sel >= INDEX_BREAKEVEN_SELECTIVITY)
        })
        .unwrap_or(false);
```

**Conservador**: sin stats → false (preserva el comportamiento
pre-P5c). El usuario tiene que correr `ANALYZE TABLE` para activar
la lógica.

### Cuándo NO se aplica

1. **Composite PK fast-path** (`composite_pk_fast_path_active`):
   match único por construcción (PK es unique). `est.match = 1`.
   La selectividad siempre baja. P5c lo deja pasar.

2. **Sin WHERE**: no hay nada que estimar — el path siempre es
   FullScan natural.

3. **Sin stats** (ningún `ANALYZE TABLE` corrido): P5c queda OFF.
   Conservador — si el usuario no analizó, el planner no asume.

### Dispatch en `exec_select_with_where`

```rust
let generic_post_filter: Option<WhereExpr> = match &stmt.where_clause {
    Some(_) if composite_pk_fast_path_active => None,
    Some(expr) if p5c_skip_index => Some(expr.clone()),  // ← fuerza post-filter
    Some(expr) => { /* lógica heredada */ }
    None => None,
};

let plan = if let Some(p) = composite_pk_plan {
    p
} else if p5c_skip_index {
    Plan::FullScan  // ← override antes de composite_index/atom dispatch
} else if let Some(p) = composite_index_plan {
    p
} else if exists_postfilter.is_some() || generic_post_filter.is_some() {
    Plan::FullScan
} else { /* atom dispatch */ };
```

El `Plan::FullScan` PLUS el `generic_post_filter` forzado da el
comportamiento correcto: escanear toda la tabla, filtrar row-a-row
con 3VL. **Mismo resultado** que el index lookup — solo más rápido.

### EXPLAIN

`classify_scan` reproduce la misma lógica y anuncia el override:

```
SCAN `t` (P5c: hash-index `idx_cat` disponible
          pero stats prefieren FullScan + post-filter)
         [est.rows=10 cols=3 est.match=6]
```

vs. el caso normal:

```
SCAN `t` → hash-index equality `cat` (bucket lookup, ~O(1))
         [est.rows=10 cols=3 est.match=1]
```

## Alternativas consideradas

1. **Cost model completo con varianzas**.
   - Descartado: complejidad agrega ~600 LOC sin ganancia clara. El
     umbral simple basado en break-even cubre el 95% de los casos
     prácticos. P5d/P5e podrán refinar.

2. **Tuning de `INDEX_BREAKEVEN` por tipo de índice** (Hash vs
   OrderedInt).
   - Descartado: ambos tipos pagan random fetch al B+tree principal.
     El cost del lookup en el ÍNDICE difiere (Hash O(1), OrderedInt
     O(log) sin range), pero es despreciable vs los random fetches.

3. **Activar P5c sin stats usando heurística por row_count**.
   - Descartado: sin stats, no sabemos `est.match`. Asumir un default
     conservador (0.1) haría que P5c casi nunca dispare, lo que
     anularía su valor. Mejor exigir ANALYZE.

4. **Pre-computar selectividad en `exec_analyze_table`** y cachearla
   por WHERE.
   - Descartado: WHERE es dinámico — diferentes queries tienen
     selectividades distintas. La estimación es barata
     (~µs por evaluación) — no vale cachear.

5. **Override per-query con hint `/*+ INDEX(t) */`**.
   - Descartado por ahora: no tenemos parser de hints. Si P5c se
     equivoca, el remedio es re-ANALIZAR (refrescar stats) o
     re-evaluar el umbral globalmente. Hints es Roadmap futuro.

## Tests

5 tests nuevos en `tests/integration_test.rs` (suite `p5c_*`):

- `p5c_alta_selectividad_prefiere_fullscan_sobre_hash_idx`:
  `category='a'` cubre 60% (6/10) → EXPLAIN muestra `P5c: hash-index
  disponible pero stats prefieren FullScan`. El SELECT real devuelve
  las 6 filas correctas (regression de correctness).
- `p5c_baja_selectividad_sigue_usando_indice`: `category='d'` cubre
  10% (1/10) → mantiene `hash-index equality`.
- `p5c_sin_stats_conserva_path_indexado`: sin `ANALYZE` → P5c off,
  EXPLAIN muestra el path indexado.
- `p5c_correctness_fullscan_devuelve_mismo_resultado`: SELECT con
  `cat='x'` (7/10 = 70%) → P5c activa, resultado bit-a-bit idéntico
  al index path.
- `p5c_composite_pk_lookup_no_es_overridden`: PK compuesta + WHERE
  exact match → composite_pk_plan SIEMPRE gana, P5c no toca.

Suite total: **774 passing** (769 → +5 P5c). Verificado vía Docker
`rust:1.94-bookworm`.

## Consecuencias

**Positivas**

- (+) Performance: queries con WHERE de alta selectividad **realmente**
  se ejecutan más rápido cuando ANALYZE corrió.
- (+) Las stats P4 dejan de ser informativas pasivas. Por primera
  vez el plan **cambia** en función de las stats.
- (+) Correctness preservada: el post-filter se ejecuta en todos los
  casos (probado en `p5c_correctness_fullscan_devuelve_mismo_resultado`).
- (+) Conservador: sin `ANALYZE`, comportamiento idéntico al pre-P5c.
  Cero regresión para usuarios que no analicen.

**Negativas / Limitaciones honestas**

- (-) Si las stats están sesgadas (ej. HLL sub-estima NDV, MCV
  cap-hit), P5c puede tomar la decisión equivocada en cualquier
  dirección. Hot-fix vía re-ANALYZE o ajuste del umbral global.
- (-) `INDEX_BREAKEVEN = 0.2` es derivado teóricamente (C_SEQ vs
  C_RANDOM ratio típico SSD) pero no calibrado empíricamente sobre
  el gabybench. P5d/P5e o un bloque de calibración podrían refinar.
- (-) No hay hint per-query (`/*+ INDEX(t) */`). El override es
  global; si el usuario quiere forzar siempre índice, debe no
  correr ANALYZE (workaround grosero).
- (-) Stats stale: si la tabla cambia mucho después de ANALYZE, el
  est.match queda viejo y P5c decide con datos obsoletos. EXPLAIN no
  muestra cuán viejas son las stats (heredado de P3b — pendiente).
- (-) El cost model NO considera tamaño de fila, ni profundidad del
  B+tree, ni page cache hit-rate. Funciona en la mayoría de los casos
  pero edge cases (filas muy grandes con páginas múltiples por fila,
  o tablas que ya están enteras en page cache) podrían sesgar.

## Limitaciones / Trabajo futuro

- **P5d**: JOIN reorder usando `est.match` por tabla.
- **P5e**: choice de algoritmo JOIN (nested vs hash vs index-loop) por
  costo.
- **Calibración de `INDEX_BREAKEVEN`** contra el `gabybench` real.
- **Hints SQL** (`/*+ USE_INDEX(t, idx_cat) */`) para override per-query.
- **Detección de staleness**: si `ANALYZE` corrió hace mucho o la
  tabla cambió >X%, marcar las stats como dudosas y bajar a default.
- **Cost model multidimensional**: considerar row_width, page cache
  hit-rate, B+tree depth. Roadmap maduro de DBs reales.
