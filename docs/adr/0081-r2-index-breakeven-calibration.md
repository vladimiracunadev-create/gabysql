# ADR-0081: R2 — INDEX_BREAKEVEN_SELECTIVITY calibrado contra gabybench smoke

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Bloque:** R2 (reparación post-P5 — primer cambio de constante con evidencia empírica)
**Origen:** [docs/ANALISIS_POST_P5.md §3 R2](../ANALISIS_POST_P5.md) — tensión #2.3
**Refina:** [ADR-0071](0071-p5c-cost-based-fallback.md) (P5c) y [ADR-0075](0075-m2-gabybench-in-ci.md) (M2 bench infra).

## Contexto

P5c (ADR-0071) introdujo el primer plan-cambio basado en stats: si
`est.match / row_count ≥ INDEX_BREAKEVEN_SELECTIVITY`, fuerza `FullScan +
post-filter` en vez del lookup indexado. El valor inicial **0.20** vino
de un cost model teórico:

```
FullScan cost = row_count × C_SEQ
Index cost    = log(N) × C_LOG + est.match × C_RANDOM
break-even sel = C_SEQ / C_RANDOM  con C_RANDOM ≈ 5 × C_SEQ
              ≈ 0.20
```

`C_RANDOM ≈ 5 × C_SEQ` es la heurística clásica para SSDs. M2
(ADR-0075) puso `gabybench smoke` en CI con artifact JSON por commit
precisamente para poder calibrar esto con datos reales.

## Medición

Una corrida del smoke (2026-06-15, Windows + GNU toolchain, release
build) sobre microblog (10k users + 40k posts) + orders_lines (20k
orders + 100k lines) dio los siguientes números (mean ns):

| Path | N filas | Mean | Por-fila | Tipo |
|---|---|---|---|---|
| PK lookup hot (id=5000) | 1 | 15.87 µs | 15.87 µs/lookup | C_RANDOM (caliente) |
| PK lookup cold (random) | 1 | 26.91 µs | 26.91 µs/lookup | C_RANDOM (frío) |
| UNIQUE TEXT lookup (email) | 1 | 23.75 µs | 23.75 µs/lookup | C_RANDOM |
| Index secundario eq (bucket=0) | 0 | 17.95 µs | n/a | overhead |
| Index ordered range (2073 rows) | 2073 | 34.89 ms | 16.83 µs/row | C_RANDOM (warm) |
| Full scan TEXT LIKE A% (10k) | 1250 | 15.90 ms | 1.59 µs/row | **C_SEQ** |
| BETWEEN qty no-idx (100k) | 24933 | 209.89 ms | 2.10 µs/row | **C_SEQ** |

### Cálculo del ratio

```
C_SEQ    ≈ 1.6 – 2.1 µs/row  (sequential leaf-cursor)
C_RANDOM ≈ 17  – 27  µs/lookup (B+tree walk + page decode + row lookup)
```

| Pair | C_RANDOM/C_SEQ | break-even sel |
|---|---|---|
| PK cold ÷ TEXT scan | 26.91 ÷ 1.59 = 16.9× | 0.059 |
| range walk ÷ BETWEEN scan | 16.83 ÷ 2.10 = 8.0× | 0.125 |
| PK cold ÷ BETWEEN scan | 26.91 ÷ 2.10 = 12.8× | 0.078 |

**Geo mean ratio ≈ 12×**, break-even ∈ [0.06, 0.13].

### Por qué C_RANDOM resulta más alto que la heurística teórica

En textbook SSDs, "random read" se modela como un seek físico (~50 µs
en HDD, ~10 µs en SSD). En gabysql, un "random read" del index path es:

1. Walk del B+tree desde root hasta leaf — típicamente 2-3 pages.
2. Decode de cada page (header + slot directory + cells).
3. Lookup de la fila por offset dentro del leaf.

Es decir, 3+ page reads + decoding, no un solo seek. La heurística
clásica subestimaba.

## Decisión

Cambiar `INDEX_BREAKEVEN_SELECTIVITY` de **0.20 → 0.10**.

**0.10** es conservador dentro del rango empírico [0.06, 0.13] —
favorece el índice sobre la duda. El cambio implica que:

- Queries con `est.match ≥ 10%` ahora caen a FullScan (antes: ≥ 20%).
- Queries con `est.match < 10%` mantienen el lookup indexado.

### Override en runtime: `GABYSQL_INDEX_BREAKEVEN`

Para no requerir recompilación cuando alguien quiera correr sweep de
calibración (probar 0.05, 0.10, 0.20, 0.30 contra el mismo dataset),
se agrega una env var:

```bash
GABYSQL_INDEX_BREAKEVEN=0.30 cargo run --bin gabybench -- smoke
```

Reglas:

- Parse falla o valor fuera de `[0.0, 1.0]` → fail-soft, vuelve al
  default 0.10 sin error.
- Lectura por-llamada (no cache) — ~50 ns por `std::env::var`,
  irrelevante frente al cost del scan.
- No expuesto vía API pública del crate — es testing/operations only.

## Consecuencias

### Positivas

- **Primera constante del optimizer respaldada por números** (no
  derivación teórica sin medir). El proceso queda documentado y es
  reproducible vía CI artifact + esta ADR.
- Cierra parcialmente tensión #2.3 del análisis post-P5 ("INDEX_BREAKEVEN
  sin calibración empírica").
- Env var habilita sweep de calibración futuro sin recompilación. M2
  (gabybench en CI) + R2 (env var) componen la pipeline mínima para
  iteraciones de calibración en próximos pushes.

### Negativas / deuda

- **Una sola corrida** del bench informa la decisión. Idealmente 5-10
  corridas distintas (en máquinas/SSDs distintos) darían más
  confianza. La constante puede revisarse cuando llegue más data.
- El cambio NO es zero-impact: queries con sel ∈ [0.10, 0.20] ahora
  cambian de plan. **No hay regression test que mida latencia
  end-to-end** — confiamos en que el cost model es correcto. Si
  algún workload puntual sufre, M6 (EXPLAIN ANALYZE compara `est.match`
  vs actual) ayudaría a diagnosticarlo.
- Hot vs cold C_RANDOM difieren 2× (15.87 vs 26.91 µs). Caches frías
  empujan el break-even más bajo (~0.06); calientes más alto (~0.10).
  0.10 da margen para ambos.

### Tests afectados

Dos tests pre-existentes usaban sel=0.10 como "baja selectividad" — con
el nuevo umbral 0.10 ese valor cae en el borde. Ajustados:

- `p5c_baja_selectividad_sigue_usando_indice`: dataset 10→20 filas
  para que `'d'` quede en sel=0.05 (claramente < 0.10).
- `r7_p5c_no_aplica_sin_hint_aunque_stats_viejas`: dataset 10→20
  filas, todas distintas para sel=0.05.

Nuevos tests (`r2_*`):

- `r2_index_breakeven_default_010_dispara_sobre_sel_012` — verifica
  que con el nuevo default, sel=0.12 dispara P5c (con 0.20 NO
  hubiera disparado). Test sencillo, sin env vars.
- `r2_env_var_override_baja_y_sube_umbral` — `#[ignore]` por race
  condition con `std::env::set_var` en suite paralela. Cubre override
  válido (0.30) y inválido (fail-soft). Correr con:
  `cargo test -- --ignored --test-threads=1 r2_env_var_override`.

## Alternativas consideradas

1. **Mantener 0.20 hasta tener N corridas comparativas.** Honesto
   pero no progresa — M2 ya da data desde hace 4 commits sin que
   nadie use los números. Acción débil.
2. **Bajar más agresivamente a 0.06** (el extremo del rango).
   Sub-óptimo: si la varianza del bench es 20%, 0.06 puede flippear
   decisiones de plan entre commits. 0.10 es defensable.
3. **Cost-model-based dispatch en runtime** (no constante, sino
   `C_RANDOM` y `C_SEQ` medidos cada N queries). Mucho más complejo;
   apropiado para Fase 6 si los problemas de calibración persisten.

## Tests

- Default 0.10: 809 → **810** verde (+1 test default).
- Env var path: 1 test `#[ignore]` + comando documentado para
  ejecutarlo serialmente.
- 2 tests existentes ajustados (no agregan ni quitan al total).

## Referencias

- [ADR-0071 — P5c cost-based fallback](0071-p5c-cost-based-fallback.md)
- [ADR-0075 — M2 gabybench en CI](0075-m2-gabybench-in-ci.md) — la infra que habilita este push
- [ANALISIS_POST_P5 §2.3](../ANALISIS_POST_P5.md) — tensión que motiva la calibración
- `bench/results.json` — corrida 2026-06-15 que informa los números arriba
