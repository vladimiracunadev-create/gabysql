# ADR-0075: M2 — `gabybench smoke` en CI con artifact JSON

**Fecha:** 2026-06-11
**Estado:** Aceptado
**Bloque:** M2 (mejora post-P5)
**Origen:** [docs/ANALISIS_POST_P5.md §4 M2](../ANALISIS_POST_P5.md) — declarado
históricamente como "P6"
**Antecede:** calibración empírica de `INDEX_BREAKEVEN` (R2),
threshold de P5d (R3), tracking de regresiones

## Contexto

El análisis post-P5 (2026-06-11) identificó que múltiples bloques
recién entregados (P5c, P5d, R1) dependen de constantes empíricas
(`INDEX_BREAKEVEN = 0.2`, `swap_threshold = 2×`,
`STATS_STALE_THRESHOLD = 7d`, etc.) sin medición real contra el bench.

`gabybench` existe (`src/bin/gabybench.rs`) y produce
`bench/results.json` con timings, pero corre solo a mano. Eso
significa:

- Una regresión de plan introducida por P5c→P5e podría tardar
  semanas en notarse.
- Los thresholds no se pueden calibrar sin baseline.
- El primer aviso de un cambio que rompe perf vendría del usuario, no
  del CI.

M2 conecta el bench al CI.

## Decisión

### Nuevo modo `smoke` del binario

```
$ gabybench smoke
== smoke setup microblog ==
   ok en 2.10s
== smoke setup orders_lines ==
   ok en 4.30s
== microblog (10 queries) ==
   ...
== orders_lines (PK compuesta, 8 queries) ==
   ...
== smoke OK — resultados en bench/results.json ==
```

Corre solo dos suites:

1. **microblog** — la más liviana (10k users, 40k posts).
2. **orders_lines** — clave para validar P5b/P5c (composite index
   lookup + skip-index por alta selectividad).

Tiempo esperado: **1-2 min** vs los 10-15 del modo `all`.

Suficiente para detectar regresiones obvias sin saturar el CI.

### Nuevo job `bench` en `.github/workflows/ci.yml`

```yaml
bench:
  runs-on: ubuntu-latest
  permissions:
    contents: read
  steps:
    - uses: actions/checkout@... persist-credentials: false
    - uses: dtolnay/rust-toolchain@... stable
    - name: Build gabybench (release)
      run: cargo build --release --bin gabybench
    - name: Run gabybench smoke
      run: ./target/release/gabybench smoke
    - name: Upload bench results
      uses: actions/upload-artifact@... gabybench-smoke-results
      path: bench/results.json
```

`upload-artifact` guarda `bench/results.json` por commit — accesible
desde GitHub Actions UI. Cualquier dev puede comparar runs entre
commits para diagnosticar regresiones manualmente.

### Por qué upload-artifact y no commit-back

Considerado: commitear `bench/baseline.json` y comparar. Descartado
por:

- Loop de commits del bot complica auditoría.
- El comparador no existe todavía (eso es M2-futuro).
- Los artifacts de GHA dan suficiente para tracking manual ahora.

### Por qué smoke y no all

`all` corre 10 suites con datasets de 10k–200k filas → 10-15 min
por job. Multiplicado por cada push, costo prohibitivo. `smoke`
cubre los paths principales (single-table WHERE, composite PK, JOIN
hash, JOIN index-loop, composite index lookup) en 1-2 min — la
mayoría de regresiones obvias caerían acá.

`all` queda disponible vía CLI manual y para corridas planificadas
(workflow_dispatch o cron — fuera del scope de este push).

## Alternativas consideradas

1. **Bench como step en el job `rust`** (no job separado).
   - Descartado: el job `rust` corre 3 OS (ubuntu/macos/windows).
     No tiene sentido benchear 3 veces. `bench` solo corre Linux
     (resultado más reproducible que Windows/macOS).

2. **Bench con flag `--quick` que reduce N filas por 10×**.
   - Considerado pero más invasivo (toca constantes hardcodeadas en
     todos los `setup_*`). El modo `smoke` que solo elige suites
     es más cirujano y reusa código existente.

3. **Comparar contra baseline guardado en el repo**.
   - Diferido. Comparar requiere definir tolerancia per-query y
     manejar variabilidad de runner (GitHub Actions tiene ruido
     conocido). Construible incremental: este push sube artifact;
     después agregamos comparador.

4. **Solo correr en `main`/`tags`, no en PRs**.
   - Descartado: queremos detectar regresiones ANTES del merge, no
     después. 1-2 min por PR es aceptable.

5. **Workflow separado `bench.yml`**.
   - Considerado. Lo hicimos parte de `ci.yml` para que GitHub UI
     muestre el estado del bench junto con fmt/clippy/test/docker.
     Un sólo "CI ✓" en el badge es UX más simple.

## Tests

No hay tests integration para M2 — el cambio es de pipeline + un nuevo
modo CLI del binario. Verificación:

1. **Local Docker**: `docker run --rm -v "${PWD}:/app" -w /app
   rust:1.94-bookworm sh -c "cargo build --release --bin gabybench &&
   ./target/release/gabybench smoke"` corre limpio y produce
   `bench/results.json`.
2. **CI**: el job `bench` aparece en runs futuros del workflow CI;
   el artifact `gabybench-smoke-results` queda accesible.

## Consecuencias

**Positivas**

- (+) Primera señal automática de regresión de performance. Los
  thresholds de P5c/P5d/R1 dejan de estar "en el aire" — ahora hay
  data point por commit.
- (+) Pre-requisito para R2 (calibrar `INDEX_BREAKEVEN`) y R3
  (calibrar swap_threshold de P5d). Sin baseline no se puede calibrar.
- (+) Visibilidad pública en GitHub Actions UI — un dev externo
  puede ver `bench/results.json` de cada commit.
- (+) Setup minimalista, ~1-2 min, sin saturar CI.

**Negativas / Limitaciones honestas**

- (-) **Sin comparador automático**. CI no falla si una query se
  pone 10× más lenta — solo sube el JSON. Detección de regresión es
  manual hasta que se agregue el comparador.
- (-) **Cobertura limitada a 2 suites de 10**. Regresiones en
  workloads específicos (vector search, RLS, window functions) no
  se detectan acá.
- (-) **Ruido del runner GHA**. Los timings de un runner compartido
  son ruidosos — diff de ±20% es normal sin regresión real.
  Comparador futuro tendrá que filtrar este ruido.
- (-) **Sin histórico persistido**. Artifacts de GHA expiran (90 días
  default). Para análisis de tendencia mes-a-mes hay que mover a
  external storage (S3, Postgres, etc.).
- (-) **No corre en macOS/Windows**. Regresiones específicas de
  Windows path-handling o macOS cache behavior no se detectan acá.

## Limitaciones / Trabajo futuro

- **Comparador**: parser de `results.json` que diffea contra baseline
  + reporta regresiones >20% como warning, >50% como fail.
- **Baseline persistido**: commitear `bench/baseline.json` cuando
  el maintainer lo aprueba; el comparador del PR muestra delta vs
  baseline.
- **`gabybench all` planificado**: workflow_dispatch + cron semanal
  para correr todas las suites; resultados archivados.
- **Métricas extra**: capturar también memoria peak por suite,
  page-cache hit rate del Pager, número de pages allocated.
- **Histórico off-CI**: mover artifacts a S3/Postgres para trend
  analysis multi-mes.
- **Calibración con bench data** (R2/R3): una vez M2 esté operando,
  ajustar `INDEX_BREAKEVEN` y `swap_threshold` empíricamente.
