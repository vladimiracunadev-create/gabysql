# ADR-0078: R7 — EXPLAIN del path P5c skip sugiere re-ANALYZE

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Bloque:** R7 (reparación post-P5)
**Origen:** [docs/ANALISIS_POST_P5.md §3 R7](../ANALISIS_POST_P5.md) — tensión #2.5
**Refina:** [ADR-0071](0071-p5c-cost-based-fallback.md) (P5c) y [ADR-0074](0074-r1-stats-stale-detection.md) (R1)

## Contexto

Hoy EXPLAIN tiene tres niveles de visibilidad sobre el uso de stats por P5c:

1. **Stats frescas (<7d)** → P5c puede decidir skip-index. EXPLAIN dice:
   `(P5c: hash-index 'idx_cat' disponible pero stats prefieren FullScan + post-filter)`.
2. **Stats stale (≥7d, R1)** → P5c bow out automáticamente. EXPLAIN dice:
   `hash-index equality 'cat' [... STALE]`.
3. **Sin stats** → camino indexado siempre. EXPLAIN no anota nada de P5c.

La banda intermedia — stats entre 1 día y 7 días — es un punto ciego. P5c
toma la decisión con stats que **podrían** estar bien o **podrían** haber
quedado desfasadas por inserts/updates posteriores. El usuario no sabe si
la decisión P5c es de confiar o si conviene re-`ANALYZE`.

Tensión #2.5 del análisis post-P5 lo nombra como cosmético, pero tiene
costo real: el dev que ve "P5c skip-index" sin contexto puede asumir que
"no debería usar índice nunca" cuando la realidad es "no debería usar
índice **según stats que ya tienen N días**".

## Decisión

Cuando el path **P5c skip** se activa en `classify_scan`, anotar un
sufijo `"; sugerencia: re-ANALYZE (stats Xd Yh)"` al mensaje SCAN si la
edad de las stats está en `[24h, 7d)`. Fuera de esa ventana:

- **<24h**: el dev acaba de correr ANALYZE. No molestar.
- **≥7d**: R1 ya bypassea P5c (no entra a esta rama, no llega el hint).

### Constante nueva

```rust
const STATS_REANALYZE_HINT_SECS: u64 = 24 * 60 * 60;
```

24h porque es el punto donde típicamente ya cambiaron suficientes filas
como para que la heurística pueda haber quedado desfasada. Es menor que
los 7d de `STATS_STALE_THRESHOLD_SECS` (R1) — la idea es que el hint
aparece **antes** de que P5c se bypassee, no después.

### Helper nuevo

```rust
fn p5c_reanalyze_hint(analyzed_at_nanos: u128) -> String {
    let age = stats_age_secs(analyzed_at_nanos);
    if !(STATS_REANALYZE_HINT_SECS..STATS_STALE_THRESHOLD_SECS).contains(&age) {
        return String::new();
    }
    format!("; sugerencia: re-ANALYZE (stats {})", format_stats_age(age))
}
```

### Integración

En `classify_scan`, el hint se calcula una vez al detectar `p5c_skip_index`
y se anexa al texto del path P5c en las 5 ramas que existen hoy:

- Composite index AND-eq (P5b path con P5c skip).
- Hash-index single-col Eq.
- Ordered-int single-col Eq.
- Ordered-int single-col BETWEEN.
- (la quinta rama hash/ordered Eq comparte el mismo helper)

Mensaje resultante con stats 2d:

```
SCAN `t` (P5c: hash-index `idx_cat` disponible pero stats prefieren
          FullScan + post-filter; sugerencia: re-ANALYZE (stats 2d 0h))
          [est.rows=10 cols=1 est.match=6 stats.age=2d 0h]
```

## Consecuencias

### Positivas

- **El dev novato** que ve "P5c skip-index" ahora tiene una pista
  accionable cuando las stats son no triviales.
- **El dev experto** que acaba de correr ANALYZE no recibe el hint —
  no genera ruido.
- **R1 + R7 forman un loop completo**: stats >24h → hint; stats >7d →
  bypass + STALE.
- Mensaje del path indexado normal no cambia — el hint es solo cuando
  la decisión P5c es de tipo skip.

### Negativas / deuda

- Sufijo agrega ~50 chars al mensaje. Ya era largo. Aceptable por ahora;
  si el output de EXPLAIN se vuelve unwieldy, considerar truncar en una
  segunda línea.
- El umbral 24h es heurístico (mismo problema que `INDEX_BREAKEVEN=0.2`
  o `STATS_STALE_THRESHOLD=7d`). No está calibrado contra workloads
  reales. Aceptable porque es un mensaje cosmético, no una decisión de
  plan.
- No se aplica al path P5b composite UPDATE/DELETE (R8): R8 reusa el
  fast-path sin pasar por `classify_scan`. Si la simetría importa, es
  trabajo para otro push.

## Alternativas consideradas

1. **Hint permanente cuando P5c skip dispara** (sin umbral de edad).
   Rechazado: el dev que acaba de correr ANALYZE no necesita el hint.
2. **Hint según delta de row_count vs estimado** (en lugar de edad).
   Más preciso conceptualmente, pero requiere infraestructura nueva
   (contadores de mod por tabla); fuera de scope para R7.
3. **Hint solo si P5c skip ≠ R6 skip** (composite vs single-col).
   Demasiado sutil — el problema es el mismo en ambos paths.

## Tests

Tres tests nuevos (`r7_*` en `tests/integration_test.rs`):

- `r7_p5c_skip_sin_hint_con_stats_frescas` — stats <24h, P5c activo,
  el hint NO aparece.
- `r7_p5c_skip_sugiere_reanalyze_con_stats_de_2d` — stats reescritas a
  2 días, P5c activo, hint con texto `"sugerencia: re-ANALYZE"` y
  edad legible.
- `r7_p5c_no_aplica_sin_hint_aunque_stats_viejas` — baja selectividad,
  P5c NO se activa, hint NO aparece aunque stats sean viejas.

Suite total: 798 → **801** (+3). Resto sin cambios.

## Referencias

- [ADR-0071 — P5c cost-based fallback](0071-p5c-cost-based-fallback.md)
- [ADR-0074 — R1 stats stale detection](0074-r1-stats-stale-detection.md)
- [ANALISIS_POST_P5 §2.5](../ANALISIS_POST_P5.md) — tensión cosmética que motiva esto.
