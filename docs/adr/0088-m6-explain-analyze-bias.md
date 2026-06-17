# ADR-0088: M6 — EXPLAIN ANALYZE anota bias del estimator (est vs actual)

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Bloque:** M6 (mejora del optimizer / diagnóstico)
**Origen:** [docs/TAREAS_PENDIENTES.md §6.5](../TAREAS_PENDIENTES.md) — "EXPLAIN ANALYZE compara `est.match` vs `actual`. Diagnóstico directo del bias del estimator."
**Refina:** ADR-0070 (P5a — `estimate_selectivity`), ADR-0071 (P5c — cost-based dispatch).

## Contexto

Pre-M6, EXPLAIN te dice:

```
SCAN `t` → hash-index equality `category` (bucket lookup, ~O(1))
          [est.rows=10 cols=1 est.match=6 stats.age=fresh]
```

EXPLAIN ANALYZE corre la query y agrega:

```
actual.time: 0.523 ms wall-clock
actual.rows: 6 filas producidas
```

Para saber si el estimator (P5a) **acertó** tenés que mirar dos
números y hacer la cuenta de cabeza: `est.match=6` vs `actual.rows=6`
→ ratio 1.0 → estimator OK. Para una query lenta donde sospechás que
P5c eligió mal por subestimar/sobreestimar, esto es engorroso.

M6 hace la cuenta automáticamente y la anota como un step extra.

## Decisión

Cuando el inner de un `EXPLAIN ANALYZE` es un SELECT **scan-only**
(sin JOIN/GROUP BY/HAVING/aggregate/LIMIT/OFFSET/DISTINCT/derived/
values/window), agregar un step `actual.bias` que muestra el ratio
y lo clasifica:

```
actual.bias: est.match=6 actual=6 ratio=1.00 BIAS=GOOD (sobre step `1`)
```

### Clasificación

| Banda | Condición | Significado |
|---|---|---|
| `MATCH` | `est=0 && actual=0` | Caso trivial degenerate. |
| `GOOD` | `ratio ∈ [0.5, 2.0]` | Estimador dentro de 2× del real. |
| `MILD` | `ratio ∈ [0.25, 0.5] ∪ [2.0, 4.0]` | 2–4× off. Aceptable pero atento. |
| `HIGH` | resto, incluido `est=0 && actual>0` | >4× off. Sospechar plan equivocado. |

### Por qué scan-only

Solo en SELECT scan-only se cumple que `row_count` final = filas que
sobrevivieron el WHERE = `est.match` esperado. Con JOIN, aggregate,
LIMIT, etc. el `actual.rows` ya no representa lo mismo que `est.match`
del SCAN step — comparar daría un "BIAS=HIGH" falso. Preferimos
omitir el bias en esos casos a engañar al lector.

### Implementación

3 helpers nuevos (`src/sql.rs`):

- `is_scan_only_select(stmt: &Statement) -> bool` — detección del
  subconjunto. Maneja correctamente `SelectQuery::Select` vs
  `SelectQuery::SetOp` / `Values`.
- `extract_est_match(detail: &str) -> Option<u64>` — parser ad-hoc
  del substring `est.match=K` dentro del detail string del SCAN step.
- `classify_bias(est: u64, actual: u64) -> (String, &'static str)` —
  computa ratio + banda.

Integración en `exec_explain` (~12 LOC nuevas en el flujo):

1. Captura `analyze_scan_only = analyze && is_scan_only_select(&inner)`.
2. Si la query ANALYZE arroja Ok y `analyze_scan_only`, busca el
   primer step cuyo detail contiene `est.match=` y emite el step
   `actual.bias` con el ratio.

Cero impacto en queries no-ANALYZE o no-scan-only.

## Consecuencias

### Positivas

- **Diagnóstico directo del estimator**: en una sola lectura, ves si
  P5a/P5c se equivocó y por cuánto. Antes pedía dos queries +
  cálculo manual.
- **Loop natural con R7**: R7 sugiere re-ANALYZE en el path P5c skip
  si stats >24h. M6 te dice si re-ANALIZAR sirvió o si el problema
  es fundamental del estimator (e.g. correlación AND no detectada).
- **Base de evidencia para futuras calibraciones**: cada query que
  vos corras con `EXPLAIN ANALYZE` ahora deja un dato bias/ratio.
  Acumular esos datos informa decisiones como "¿conviene cambiar
  R2 INDEX_BREAKEVEN de 0.10 a 0.05?" — con data en vez de teoría.
- **Cero cost cuando no se usa**: `is_scan_only_select` retorna en
  microsegundos; los helpers solo se llaman en el path EXPLAIN
  ANALYZE.

### Negativas / deuda

- **Solo scan-only**. Para queries con JOIN / GROUP BY / aggregate
  (donde más se equivoca P5d build-side, R6 composite bucket), no
  hay bias. Resolverlo correctamente requiere instrumentar
  conteos por-step durante la ejecución — fuera de scope para 1
  push.
- **Parser ad-hoc del string**. `extract_est_match` hace
  `detail.find("est.match=")` y parsea. Si el formato del SCAN step
  cambia (e.g. `est.match: 6` con dos puntos), se rompe silenciosamente.
  Aceptable porque el formato es interno de gabysql; agregar test
  de regresión que falle si cambia.
- **Bandas absolutas**. `GOOD = [0.5, 2.0]` es heurística sin
  calibración empírica. Es probable que para gabysql sea apropiado;
  re-evaluar si los benchmarks lo indican.
- **No persiste**. El bias se imprime una vez; no se guarda. Si
  quisieras "stats sobre el estimator a lo largo del tiempo" tendrías
  que parsear los logs.

## Alternativas consideradas

1. **Mostrar `actual=K` en el mismo SCAN step** (mutando su detail
   string en vez de un step aparte). Más compacto pero hace el SCAN
   step asimétrico entre EXPLAIN y EXPLAIN ANALYZE — el lector tiene
   que aprender dos formas. El step separado es más simple.
2. **Calcular bias también para JOIN/aggregate** comparando contra
   alguna estimación derivada del plan. Cualquier estimación
   intermedia sería propensa a error sistemático; mejor honestidad
   que un "BIAS=GOOD" optimista falso.
3. **Tracking acumulativo del bias** (e.g. tabla `__est_bias_log__`).
   Útil pero requiere bump VERSION + infra de gc. Diferible.

## Tests

Tres tests nuevos (`m6_*` en `tests/integration_test.rs`):

- `m6_explain_analyze_bias_good_cuando_estimador_da_en_el_clavo` —
  25 filas con MCV exacto sobre `'a'` (6 entries). `est.match=6`,
  `actual=6` → `BIAS=GOOD`.
- `m6_explain_analyze_sin_bias_en_query_con_join` — query con JOIN
  no debe emitir `actual.bias`. Regresión guard.
- `m6_explain_analyze_bias_high_cuando_estimador_sobreestima` — WHERE
  sobre valor no presente en MCV → P5a usa `DEFAULT_EQ_SELECTIVITY`
  y estima ≥1; actual=0 → ratio=0 → `BIAS=HIGH`. Exactamente el caso
  de uso que motiva M6.

**Suite total**: 816 → **819** (+3 tests integration).

## Referencias

- [ADR-0070 — P5a estimate_selectivity](0070-p5a-selectivity-estimation.md) — la estimación que M6 mide.
- [ADR-0071 — P5c cost-based fallback](0071-p5c-cost-based-fallback.md) — el consumidor del estimator.
- [ADR-0078 — R7 P5c re-ANALYZE hint](0078-r7-p5c-reanalyze-hint.md) — loop natural con M6.
- [TAREAS_PENDIENTES.md §6.5](../TAREAS_PENDIENTES.md) — declaraba este item como ~200 LOC.
