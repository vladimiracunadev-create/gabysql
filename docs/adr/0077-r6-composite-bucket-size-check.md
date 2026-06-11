# ADR-0077: R6 — post-lookup bucket size check para composite index

**Fecha:** 2026-06-11
**Estado:** Aceptado
**Bloque:** R6 (reparación post-P5)
**Origen:** [docs/ANALISIS_POST_P5.md §3 R6](../ANALISIS_POST_P5.md) — tensión #2.7
**Refina:** [ADR-0071](0071-p5c-cost-based-fallback.md) (P5c — selectividad estimada)

## Contexto

P5c (ADR-0071) usa `estimate_selectivity(stats, WhereExpr)` y, para
`WHERE a=X AND b=Y`, multiplica `sel(a=X) × sel(b=Y)` asumiendo
**independencia** entre columnas. En la realidad las columnas suelen
estar correlacionadas:

- `WHERE marca='Toyota' AND modelo='Corolla'` no es `sel(marca) × sel(modelo)`
  — modelo implica marca, son altamente correlacionadas.
- `WHERE qty=5 AND precio=10` en un catálogo de productos: si la
  mayoría de los productos baratos tienen qty pequeño, ambos cubren
  el mismo subconjunto.

La asunción de independencia puede:

- **Sub-estimar** selectividad real → P5c no skip-ea cuando debería
  → index lookup escanea N random reads cuando FullScan habría sido
  más barato.
- **Sobre-estimar** selectividad real → P5c skip-ea cuando debería
  usar el índice → FullScan innecesario.

El composite index lookup tiene una ventaja: al hacer
`composite_index_lookup_pks(fp)` obtenemos el bucket REAL — la
cardinalidad verdadera del predicado compuesto, sin asumir nada.

R6 usa esa cardinalidad real como check adicional **post-lookup**.

## Decisión

### Helper nuevo

```rust
fn composite_bucket_too_large_for_index_path(
    stats: Option<&TableStats>,
    bucket_size: usize,
) -> bool {
    let Some(stats) = stats else { return false; };
    if stats_are_stale(stats.analyzed_at_nanos) { return false; }
    if stats.row_count == 0 { return false; }
    let ratio = bucket_size as f64 / stats.row_count as f64;
    ratio >= INDEX_BREAKEVEN_SELECTIVITY
}
```

**Conservador en 3 casos**:

1. Sin stats → no bail. Preserva comportamiento pre-R6.
2. Stats stale (R1) → no bail. Si las stats son viejas, el
   `row_count` puede no reflejar la realidad — mejor no decidir.
3. `row_count == 0` → no bail. Evita división por cero.

### Aplicación en SELECT (`exec_select_with_where`)

`composite_index_plan` ahora hace lookup, mide bucket, y devuelve
`None` si es demasiado grande:

```rust
.and_then(|(idx_root, fp)| {
    let pks = composite_index_lookup_pks(self.pager, idx_root, fp).ok()?;
    if composite_bucket_too_large_for_index_path(stats_for_table.as_ref(), pks.len()) {
        return None;  // bail to FullScan via generic_post_filter
    }
    Some(Plan::ByPks(pks))
})
```

Cuando bail, el dispatch cae a `Plan::FullScan + generic_post_filter`
(que ya está forzado por la regla AND-of-eq).

### Aplicación en UPDATE/DELETE (`resolve_target_pks`)

Mismo check en el fast-path composite secundario (introducido por R8,
ADR-0076). Si bail, cae al FullScan + 3VL del final de la función:

```rust
if !composite_bucket_too_large_for_index_path(stats_for_table.as_ref(), candidate_pks.len()) {
    // ... fast-path lookup + post-filter ...
    return Ok((pks, false));
}
// bucket grande → FullScan fallback abajo
```

### Por qué post-lookup y no pre-lookup

`composite_index_lookup_pks` es O(log N) — read del B+tree del índice
+ decode del bucket. Cheap. Hacer el lookup primero y decidir después
es trivial.

Hacerlo **pre**-lookup requeriría algo como "bucket cardinality
estadísticas" — datos extra que el índice no guarda. Out of scope.

### Relación con P5c

P5c sigue activo. Tres escenarios posibles:

| Estimate | Bucket real | Decisión |
|---|---|---|
| baja (P5c no bail) | chico | composite gana (path normal) |
| baja (P5c no bail) | grande | **R6 bail** ← R6 cubre acá |
| alta (P5c bail) | n/a | P5c gana, R6 no se evalúa |

R6 es complementario, no reemplaza P5c. Cubre exactamente el caso
donde la independencia AND falla.

### EXPLAIN no refleja R6

`classify_scan` reporta el plan estático según P5c, pero R6 es una
decisión **runtime** (necesita lookup del bucket para saber). Por
eso EXPLAIN puede mostrar "composite index lookup" cuando R6 en
realidad cae a FullScan. Limitación documentada.

Workaround: el usuario puede comparar `est.match` de EXPLAIN con
filas reales devueltas. Si `est.match` está sub-estimando, R6
probablemente está activando.

## Alternativas consideradas

1. **Estimación combinada por correlación** (multi-column stats).
   - Diferido (M5 del análisis post-P5). PostgreSQL `CREATE STATISTICS
     (col_a, col_b)`. ~600 LOC + bump VERSION. Reemplazaría tanto P5c
     como R6 — pero es trabajo grande para un push.

2. **Bucket cardinality cacheada en el índice**.
   - Diferido. Cambiaría layout on-disk del índice. R6 con post-lookup
     no necesita esto y funciona bien.

3. **Threshold separado para R6** (ej. 0.3 en vez de 0.2).
   - Considerado. Descartado por simplicidad. Misma intuición de
     break-even — random reads vs sequential. Cambio futuro si los
     datos del bench muestran que conviene.

4. **R6 también para single-col index**.
   - No tiene sentido: el single-col index lookup ya da PKs y P5c
     ya estima sobre el átomo simple `col = X`. La asunción de
     independencia no aplica a un solo predicate. R6 es específico
     para AND-of-eq.

5. **Anunciar R6 en EXPLAIN haciendo el lookup**.
   - Descartado. EXPLAIN puramente descriptivo (no `ANALYZE`).
     Hacer el lookup ahí cambiaría el contrato.

## Tests

4 tests nuevos en `tests/integration_test.rs` (suite `r6_*`):

- `r6_composite_bucket_grande_cae_a_fullscan_select`: 10 filas,
  bucket `(qty=5, precio=10)` cubre 8 → ratio 0.8 ≥ 0.2 → R6 bail
  para SELECT. Resultado correcto (8 filas matched).
- `r6_composite_bucket_chico_sigue_usando_indice`: 100 filas con
  100 buckets distintos, query selecciona 1 → ratio 0.01 → mantiene
  composite path (EXPLAIN dice "composite index lookup").
- `r6_composite_bucket_grande_correctness_update`: UPDATE con mismo
  setup que el SELECT → 8 filas modificadas correctamente vía R8 +
  R6 fast-path → FullScan.
- `r6_sin_stats_no_bail`: sin ANALYZE → composite path sigue
  activo (sesgo conservador).

Suite total: **798 passing** (794 → +4 R6). Verificado vía Docker
`rust:1.94-bookworm`.

## Consecuencias

**Positivas**

- (+) Cierra la tensión #2.7 del análisis post-P5: composite
  no-UNIQUE con bucket grande ya no causa N random reads.
- (+) Refina la asunción de independencia de P5c — la única que el
  análisis post-P5 marcó como "magnitud alta" (tensión #2.2).
- (+) Aplica simétricamente a SELECT y UPDATE/DELETE (continúa el
  patrón de R8).
- (+) Conservador en presencia de stats stale (heredado R1) — la
  decisión runtime es robusta a datos viejos.

**Negativas / Limitaciones honestas**

- (-) **EXPLAIN miente cuando R6 bail-ea**. El plan estático dice
  "composite index lookup", el runtime hace FullScan. Mejor que P5e
  (que mentía con "nested-loop" siempre), pero todavía imperfecto.
- (-) Hace el lookup aunque después abandone el resultado. Costo
  O(log N) — despreciable vs FullScan que vamos a correr — pero
  desperdicio teórico. Cacheable si pega.
- (-) Sin auto-ANALYZE (M1), el `row_count` usado en el ratio
  puede ser viejo. R6 bow-out por stale lo mitiga, pero workloads
  que crecen rápido pueden ver R6 decidir mal entre ANALYZEs.
- (-) Threshold compartido con P5c (0.2). Si la calibración (R2)
  ajusta INDEX_BREAKEVEN, ambos paths se mueven juntos. Para la
  mayoría de los casos esto es correcto (mismo break-even
  random-vs-seq), pero podría haber casos donde queremos thresholds
  distintos.

## Limitaciones / Trabajo futuro

- **M5 — multi-column stats** (correlación): reemplazaría la
  asunción de independencia con datos reales. R6 quedaría como
  fallback útil de todos modos.
- **EXPLAIN con runtime annotation**: opcional con flag
  (`EXPLAIN /*+ TRACE_RUNTIME */`) que ejecute el primer lookup
  para anotar R6.
- **Cache de bucket sizes**: agregar al índice un counter por
  fingerprint. Permitiría decidir pre-lookup. Cambio de layout
  on-disk, bump VERSION.
- **Threshold separado** (R6_BREAKEVEN distinto de
  INDEX_BREAKEVEN): si bench data muestra que conviene.
