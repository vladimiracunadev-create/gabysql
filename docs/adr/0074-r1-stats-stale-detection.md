# ADR-0074: R1 — detección de stats stale + bypass de P5c

**Fecha:** 2026-06-11
**Estado:** Aceptado
**Bloque:** R1 (reparación post-P5)
**Origen:** [docs/ANALISIS_POST_P5.md §3 R1](../ANALISIS_POST_P5.md) — tensión #2.1
**Consume:** ADR-0067 (P3b — `analyzed_at_nanos` ya persiste)

## Contexto

P5c (ADR-0071) introdujo la primera decisión de plan que depende de
stats. Si las stats están obsoletas, el plan se equivoca silenciosamente.
Esto fue identificado como la **tensión #1** en el análisis post-P5
([docs/ANALISIS_POST_P5.md](../ANALISIS_POST_P5.md)).

El dato `analyzed_at_nanos` ya estaba persistido en cada `StatsMeta`
(P3b/2026-06-09) pero nadie lo consumía. Esta brecha — "tener el dato
y no usarlo" — es exactamente lo que el análisis identificó como
prioridad alta.

R1 conecta el cable.

## Decisión

### Threshold: 7 días

```rust
const STATS_STALE_THRESHOLD_SECS: u64 = 7 * 24 * 60 * 60;
```

Por qué 7 días:

- Para workloads con cambios graduales (catálogos de productos, logs
  acumulativos), 7 días es razonable antes de que las distribuciones
  se desvíen.
- Para usuarios que actualizan la tabla a diario, el threshold sirve
  como recordatorio: las stats *deberían* refrescarse al menos
  semanalmente.
- Deliberadamente generoso — no queremos alarmar al usuario casual
  con `STALE` después de 1 hora.

Futuro: ajustable per-tabla con `ALTER TABLE ... SET STATS_TTL=Xd`
(no en este push).

### Helpers nuevos

```rust
fn stats_age_secs(analyzed_at_nanos: u128) -> u64;
fn stats_are_stale(analyzed_at_nanos: u128) -> bool;
fn format_stats_age(age_secs: u64) -> String;
```

`stats_age_secs` devuelve 0 si el reloj está atrasado (sesgo
conservador, no marca falsos positivos de stale por skew de reloj).

`format_stats_age` renderiza con resolución útil:

- `< 60s` → `"fresh"` (no se muestra en EXPLAIN)
- `< 1h` → `"45m"`
- `< 1d` → `"3h 27m"`
- `≥ 1d` → `"5d 12h"`

### EXPLAIN annotation

`stats_annotation` extendida — cuando hay stats y `age ≥ 60s`,
agrega `stats.age=Xd Yh` y opcionalmente ` STALE`:

```
[est.rows=10 cols=3 est.match=6 stats.age=3d 5h]      ← fresca aún
[est.rows=10 cols=3 est.match=6 stats.age=10d 0h STALE] ← stale
[est.rows=3 est.match=1]                              ← <60s, no se muestra
```

### Bypass de P5c

```rust
let p5c_skip_index = ...
    .and_then(|expr| {
        let stats = ...;
        if stats_are_stale(stats.analyzed_at_nanos) {
            return Some(false);  // bow out
        }
        let sel = estimate_selectivity(stats, expr);
        Some(sel >= INDEX_BREAKEVEN_SELECTIVITY)
    });
```

Cuando las stats son stale, P5c **no se aplica**. El plan vuelve al
comportamiento conservador pre-P5c (siempre preferir índice si existe).
Argumento: mejor "lento por usar índice cuando no convenía" que
"lento por usar FullScan basándose en datos viejos que sub-estiman
selectividad". El primer caso es contenido; el segundo puede escalar.

Aplica tanto en `exec_select_with_where` como en `classify_scan`
(EXPLAIN) — ambos consultan el mismo helper, decisión consistente.

## Alternativas consideradas

1. **Threshold configurable per-instancia** vía variable de entorno
   `GABYSQL_STATS_TTL_DAYS`.
   - Diferido. La constante en código es buena para ahora; cuando
     haya señales de que 7 días no encaja para algún workload, lo
     hacemos config.

2. **Bypass parcial**: degradar progresivamente la confianza con el
   tiempo (mezcla lineal entre `sel` real y `DEFAULT_EQ_SELECTIVITY`
   según `age / threshold`).
   - Descartado por complejidad. La regla binaria "stale o no"
     es legible. Si se ve que es muy abrupto, refinamos.

3. **Re-`ANALYZE` automático cuando se detecta stale** (auto-ANALYZE).
   - Diferido a M1 (auto-ANALYZE, declarado en docs/ANALISIS_POST_P5.md).
     Requiere scheduler que aquí no tenemos.

4. **Warning explícito en el `message` de la query** ("⚠ stats stale
   sobre tabla `t`; considerá `ANALYZE TABLE t`").
   - Considerado y descartado para este push: agregar warnings en
     paths normales complica el ResultSet shape. EXPLAIN ya lo dice
     — el usuario que mira EXPLAIN ve `STALE`.

5. **Stats stale → no anotar `est.match` (no estimar)**.
   - Considerado: si decimos stale, también el `est.match` está
     equivocado. Pero seguir mostrándolo da una pista útil ("esta
     era la última estimación conocida"). Lo dejamos.

## Tests

4 tests nuevos en `tests/integration_test.rs` (suite `r1_*`):

- `r1_explain_muestra_stats_age_para_stats_frescas`: stats recién
  creadas (<60s) → no aparece `stats.age` en EXPLAIN.
- `r1_explain_muestra_stats_age_para_stats_viejas`: stats con 3d →
  EXPLAIN muestra `stats.age=3d X` sin `STALE`.
- `r1_explain_marca_stale_si_supera_threshold`: stats con 10d →
  EXPLAIN muestra `stats.age=10d ... STALE`.
- `r1_p5c_bow_out_si_stats_stale`: setup que dispararía P5c con
  stats frescas. Tras forzar stats a 14d, EXPLAIN muestra
  `hash-index` (P5c bow out) en vez de `P5c skip-index`.

Helper de test `r1_overwrite_stats_timestamp(db, table, age_secs)`
abre el catálogo directamente y reescribe `StatsMeta.analyzed_at_nanos`
para simular el paso del tiempo determinísticamente. Sin esto, el test
necesitaría `sleep(7 días)`.

Suite total: **785 passing** (781 → +4 R1). Verificado vía Docker
`rust:1.94-bookworm`.

## Consecuencias

**Positivas**

- (+) Cierra la tensión #1 identificada en el análisis post-P5: P5c
  ya no toma decisiones load-bearing con datos viejos.
- (+) Usuario tiene feedback visible (`STALE` en EXPLAIN) cuando
  conviene re-`ANALYZE`.
- (+) `analyzed_at_nanos` deja de ser "dato persistido pero no usado".
- (+) Path conservador es seguro — si nos equivocamos al detectar
  stale (reloj saltó, etc.) el peor caso es "usamos el índice cuando
  el FullScan era mejor" — pérdida acotada.

**Negativas / Limitaciones honestas**

- (-) Threshold 7d es arbitrario. Workloads con cambios horarios
  (logs en producción) verán stale 7 días después — demasiado tarde.
- (-) No hay re-`ANALYZE` automático. El usuario tiene que ver
  `STALE` y actuar. Si nunca abre EXPLAIN, no se entera.
- (-) `STALE` no se propaga a `est.match`: seguimos mostrando el
  número aunque sea poco confiable. Es una decisión consciente
  (mantener el contexto histórico) pero puede confundir.
- (-) El reloj del sistema es la única referencia. Si la DB se monta
  en una máquina con reloj atrasado, `stats_age_secs` devuelve 0 y
  nada se marca stale. Sin sincronización confiable, no hay solución.
- (-) Sin auto-ANALYZE (M1) y sin gabybench en CI (M2), no podemos
  validar empíricamente que 7 días es el threshold correcto.

## Limitaciones / Trabajo futuro

- **M1 — Auto-ANALYZE**: cuando una tabla cambia >X% desde el último
  ANALYZE, re-disparar automáticamente. Requiere bookkeeping de
  row_count delta y un scheduler.
- **Threshold per-tabla**: `ALTER TABLE t SET STATS_TTL='1d'` para
  workloads con cambios rápidos.
- **Detección de invalidación grosera**: si `row_count` actual difiere
  >50% del `row_count` persistido, marcar como stale aunque el tiempo
  no haya pasado. Más sensible que el threshold de tiempo.
- **Warning explícito en `message`**: opcional, opt-in vía SET.
