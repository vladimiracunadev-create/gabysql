# Análisis del producto tras la sesión P3b → P5e + R1/R4/M2/R8/R6 (2026-06-09 al 2026-06-11)

> **Propósito**: dejar registrado qué cambió, qué quedó frágil, qué características
> chocan entre sí, qué hay que reparar y qué hay que mejorar. Es un diagnóstico
> después de 12 pushes en 48h — el ritmo expuso tensiones que conviene nombrar
> antes de seguir construyendo.
>
> ## Actualización 2026-06-11 (post-acciones del análisis)
>
> Tras escribir el análisis original (justo después de P5e), seguimos trabajando
> sobre las tensiones identificadas. Las acciones aplicadas:
>
> - **Tensión 2.1 (stats stale + P5c)** → ✅ **cerrada por R1** ([ADR-0074](adr/0074-r1-stats-stale-detection.md)).
>   EXPLAIN muestra `stats.age=Xd Yh` (+ `STALE`); P5c bow out automático >7d.
> - **Tensión 2.2 (independencia AND)** → ✅ **mitigada por R6** ([ADR-0077](adr/0077-r6-composite-bucket-size-check.md)).
>   No eliminada — sigue afectando single-col AND y AND más complejo —, pero en el caso composite
>   el post-lookup ya no depende del producto estimado.
> - **Tensión 2.7 (composite bucket grande)** → ✅ **cerrada por R6**.
> - **Tensión 2.8 (HLL no validado en tipos no-INT)** → ✅ **cerrada por R4**
>   (4 tests: TEXT/UUID/DATE/DECIMAL, todos ±25% del NDV real).
> - **Tensión 2.3 / 2.4 (calibración INDEX_BREAKEVEN y threshold P5d)** → parcial:
>   **M2** ([ADR-0075](adr/0075-m2-gabybench-in-ci.md)) puso `gabybench smoke` en CI con artifact JSON
>   por commit. Falta R2/R3 dedicados que analicen N corridas y ajusten constantes.
> - **Asimetría R8 (UPDATE/DELETE no usaban P5b)** → ✅ **cerrada por R8**
>   ([ADR-0076](adr/0076-r8-update-delete-composite-fast-path.md)).
>
> **Marcador rápido al 2026-06-11**: 5 de 9 tensiones del análisis original cerradas; 4 abiertas
> (2 cosméticas: 2.5, 2.6 · 2 dependientes de bench data: 2.3, 2.4 · 1 deuda
> documentada: 2.9). Suite tests 745 → **798**.
>
> ## Actualización 2026-06-15 (sesión maratón de cierre)
>
> Diez pushes en una sesión cerraron 3 tensiones más y la mayoría de las reparaciones
> abiertas:
>
> - **Tensión 2.3 (INDEX_BREAKEVEN sin calibrar)** → ✅ **cerrada por R2**
>   ([ADR-0081](adr/0081-r2-index-breakeven-calibration.md)). Sweep contra
>   gabybench smoke midió C_RANDOM/C_SEQ ≈ 12× (textbook era 5×). Default
>   0.20 → 0.10 + env var override `GABYSQL_INDEX_BREAKEVEN`.
> - **Tensión 2.4 (threshold P5d sin medir)** → ✅ **cerrada con outcome
>   explícito por R3 + R3-cont** ([ADR-0082](adr/0082-r3-p5d-swap-threshold-instrumentation.md) +
>   [ADR-0085](adr/0085-r3-cont-p5d-sweep-results.md)). Sweep 1.5/2.0/3.0/10.0
>   sobre query JOIN nueva — datos inconclusos, default 2.0 stays. Env var
>   `GABYSQL_P5D_SWAP_THRESHOLD` queda para futuros sweeps.
> - **Tensión 2.5 (mensaje EXPLAIN P5c ambiguo)** → ✅ **cerrada por R7**
>   ([ADR-0078](adr/0078-r7-p5c-reanalyze-hint.md)). EXPLAIN del path skip
>   sugiere re-ANALYZE si stats ∈ [24h, 7d).
> - **Residual cobertura SQL #1 (COUNT DISTINCT sobre JOIN, ADR-0066 Gap 1)**
>   → ✅ **cerrada por R9** ([ADR-0079](adr/0079-r9-count-distinct-over-join.md)).
> - **Heurística USING/NATURAL JOIN en EXPLAIN** → ✅ **completada por R10**
>   ([ADR-0080](adr/0080-r10-using-natural-explain.md)). Mitigación parcial de 2.6.
> - **Red de seguridad del optimizer** → ✅ **M3 entregada**
>   ([ADR-0084](adr/0084-m3-proptest-planner.md)). 240 comparaciones por
>   corrida defienden correctness de P5c/P5d/R6 con seed reproducible.
> - **Bonus ANSI fix**: `UPDATE/DELETE WHERE pk = N` con N no presente
>   ya no devuelve `[GBY-3006]`; devuelve 0 filas como PostgreSQL/SQLite
>   ([ADR-0083](adr/0083-ansi-update-delete-no-row-zero.md)). Descubierto
>   por el bug del bench warmup.
>
> **Marcador rápido al 2026-06-15**: **7 de 9 tensiones cerradas**. Quedan
> 2 abiertas: **2.6** (RIGHT/FULL JOIN heurística — documentada en ADR-0073,
> R10 la mitigó parcialmente) y **2.9** (P5d swap puede cambiar orden sin
> ORDER BY — deuda documentada; M3 la sortea ordenando en Rust). Suite tests
> 798 → **828** al cierre del día (15 pushes consecutivos). ADRs 0077 → **0090** (+13 nuevos).
>
> ## Segunda ola 2026-06-15 (5 pushes adicionales) — endurecimiento y features SQL-estándar
>
> Después de cerrar las reparaciones de la primera ola, la sesión siguió con:
>
> - **fix(JOIN)** — bug pre-existente de pre-allocation cartesiano (48 GB en CI 8 GB
>   runner) destapado por R3-cont. `Vec::with_capacity(current.len() * right_rows.len() / 2 + 1)`
>   → `Vec::with_capacity(current.len())`.
> - **Pager proptest** ([ADR-0086](adr/0086-pager-proptest.md)) — segunda capa de
>   la red property-based: 3 invariantes sobre `begin/insert/commit/rollback` +
>   `INTEGRITY CHECK`. Hermana de M3.
> - **M4 fuzz parser** ([ADR-0087](adr/0087-m4-fuzz-parser.md)) — 1 hora limpia
>   sobre 503.8M iters / 139k iters/seg / 0 panics. Evidencia inmutable:
>   [`docs/fuzz/FUZZ-RUN-2026-06-15.md`](fuzz/FUZZ-RUN-2026-06-15.md). Línea de
>   README "X horas de fuzz" finalmente con evidencia citable.
> - **M6** ([ADR-0088](adr/0088-m6-explain-analyze-bias.md)) — `EXPLAIN ANALYZE`
>   anota `actual.bias` con ratio `actual/est` + clasificación GOOD/MILD/HIGH/MATCH
>   para queries scan-only. Cierra el loop con R7 (re-ANALYZE hint).
> - **M12** ([ADR-0089](adr/0089-m12-savepoints.md)) — `SAVEPOINT`/`ROLLBACK TO`/
>   `RELEASE` (ANSI SQL:2003). Antes `[GBY-4029] "savepoints aún no soportados"`.
>   Desbloquea M13.
> - **M13** ([ADR-0090](adr/0090-m13-cross-request-tx.md)) — sessions cross-request
>   en server HTTP. 3 endpoints nuevos + `/exec` con `X-Gabysql-Session` header.
>   Backwards compatible (sin session = auto-commit clásico). Habilita ORMs.

---

## 1. Resumen de los cambios

| Bloque | Qué entregó | VERSION | Riesgo introducido |
|---|---|---|---|
| **P3b** (2026-06-09) | stats por-tabla persistidas (`ObjectKind::TableStats`) | 31→32 | bajo |
| **P4** (2026-06-10) | stats por-columna (null/HLL/MCV/histograma) | 32→33 | medio (HLL precisó hot-fix splitmix64) |
| **P5a** (2026-06-11) | `estimate_selectivity` + EXPLAIN `est.match` | zero-bump | nulo (annotation only) |
| **P5b** (2026-06-11) | composite secondary index lookup (cierra Gap 10) | zero-bump | bajo |
| **P5c** (2026-06-11) | cost-based skip-index si `sel ≥ 0.2` | zero-bump | medio (primer plan-cambio por stats) |
| **P5d** (2026-06-11) | hash-join build-side swap si `current > right × 2` | zero-bump | bajo (cardinality real) |
| **P5e** (2026-06-11) | EXPLAIN anota algoritmo real de JOIN | zero-bump | nulo (annotation) |
| **R1** (2026-06-11) | detección stats stale + bypass P5c | zero-bump | nulo (mitigación tensión 2.1) |
| **R4** (2026-06-11) | tests HLL sobre TEXT/UUID/DATE/DECIMAL | zero-bump | nulo (tests, mitigación tensión 2.8) |
| **M2** (2026-06-11) | `gabybench smoke` en CI + artifact JSON | zero-bump | nulo (CI) |
| **R8** (2026-06-11) | composite-eq fast-path para UPDATE/DELETE | zero-bump | bajo (reusa helpers P5b) |
| **R6** (2026-06-11) | post-lookup bucket-size check | zero-bump | bajo (mitigación tensiones 2.2/2.7) |

**Tests**: 745 → **798** (+53). Pasaron en 5 runners de CI (ubuntu/macos/windows/docker + nuevo job `bench`).

**Líneas tocadas**: ~5 000 LOC en `src/sql.rs`, `src/catalog.rs`, `src/storage.rs`,
`src/bin/gabybench.rs`, `tests/integration_test.rs`, `.github/workflows/ci.yml`,
más **11 ADRs nuevos** (0067–0077).

---

## 2. Tensiones y choques entre características

### 2.1 Stats stale silenciosas (heredado P3 → amplificado por P5c) — ✅ CERRADA por R1

**Problema**: `ANALYZE TABLE` corre una vez, las stats persisten. Pero si la
tabla cambia mucho después, ni el motor ni EXPLAIN lo señalan. Pre-P5c era
informativo nomás. **Post-P5c es load-bearing**: el plan cambia según stats.
Una tabla con 1k filas analizadas hace tiempo, y ahora 1M filas → P5c puede
elegir FullScan creyendo que `est.match` es razonable cuando en realidad
escanea 1M filas para devolver 1.

**Síntoma esperable**: queries que iban rápido se ralentizan tras un crecimiento
de tabla, sin error visible. El usuario no sabe que sus stats son obsoletas.

**Magnitud**: alto — es la principal externalidad de P5c.

### 2.2 Independencia asumida en AND (heredado P5a → propagado a P5c) — 🟡 PARCIALMENTE MITIGADA por R6

`estimate_selectivity(And(a, b)) = sel(a) × sel(b)`. Asume columnas
independientes. En la realidad están correlacionadas (`marca='Toyota' AND
modelo='Corolla'` no es 0.01 × 0.001 — es más cerca de 0.001 porque modelo
implica marca). P5c usa esa estimación para decidir FullScan vs index.

**Síntoma esperable**: en queries con `WHERE a AND b` muy correlacionadas,
P5c sub-estima `est.match` y elige el path equivocado. Falla silenciosa.

**Magnitud**: medio — afecta solo a queries con AND correlacionado.

### 2.3 `INDEX_BREAKEVEN = 0.2` sin calibración empírica — ✅ CERRADA por R2

El umbral viene del cost model teórico `C_RANDOM ≈ 5 × C_SEQ`. No está calibrado
contra `gabybench`. Si en la práctica nuestro motor tiene `C_RANDOM ≈ 2 × C_SEQ`
(SSD cache-friendly), el umbral correcto sería ~0.5. Estamos saltando al
FullScan demasiado pronto.

**Magnitud**: medio. Requiere experimentación con gabybench → ajustar la
constante.

### 2.4 Threshold de P5d (2×) sin medición — ✅ CERRADA por R3 + R3-cont

Mismo problema en menor escala. El swap del build-side se dispara a 2×.
Pudo haber sido 1.5× o 3×. Sin gabybench JOIN-heavy no sabemos.

**Magnitud**: bajo. El cambio es semánticamente seguro; solo el threshold
óptimo es desconocido.

### 2.5 Mensaje en EXPLAIN ambiguo entre P5c y "no hay índice" — ✅ MITIGADA por R7

```
SCAN `t` (P5c: hash-index `idx_cat` disponible
          pero stats prefieren FullScan + post-filter)
```

vs

```
SCAN `t` (full scan + WHERE cat=... post-filter; considerá `CREATE INDEX` sobre `cat`)
```

El usuario novato que ve "P5c skip-index" puede asumir que "no debería usar
índice nunca". El mensaje educativo de pre-P5c se silenció. Esto se nota más
cuando un dev no conoce la lógica de break-even.

**Magnitud**: bajo — UX cosmético, no de correctness.

### 2.6 `RIGHT/FULL JOIN` en EXPLAIN — heurística imprecisa

P5e reporta hash join para todos los `RIGHT/FULL JOIN`, pero el dispatcher
real puede usar nested-loop. Es imprecisión documentada (mejor que mentir
con "nested-loop" siempre), pero deja una ventana donde EXPLAIN no coincide
con la ejecución real.

**Magnitud**: bajo. RIGHT/FULL son raros en queries típicas.

### 2.7 Composite index NO-UNIQUE con bucket gigantesco — ✅ CERRADA por R6

Si el composite index `(qty, precio)` se usa sobre valores muy repetidos
(p.ej. todas las filas con `qty=5, precio=100`), el bucket guarda miles de
PKs. La lookup devuelve esos PKs, hace miles de random reads — exactamente
el caso que P5c quiere evitar. Pero P5c NO se aplica a composite indexes
(solo a single-col). El bench tipo "warehouse con valores skewed" sufriría.

**Magnitud**: medio. No es un bug; es un caso edge que P5c no cubre por
diseño.

### 2.8 HLL bias en columnas con tipos no-INT — ✅ CERRADA por R4

El hot-fix de P4 (splitmix64 finalizer) se validó con INT secuenciales.
Para DECIMAL, DATE, UUID, TEXT — no validado empíricamente. Si HLL está
sesgado en algún tipo (ej. DECIMAL: el encoding incluye `value + scale` →
distribución no uniforme), P5c puede tomar decisiones malas en queries
sobre esos tipos.

**Magnitud**: medio. Específico a workloads con esos tipos.

### 2.9 P5d swap puede cambiar orden de resultado sin ORDER BY

Tests existentes (no en P5d) pueden asumir orden determinístico. P5d swap
cambia el iterador exterior → orden ligeramente distinto. Threshold 2×
minimiza casos, pero NO los elimina. Si un test futuro depende de orden
en un JOIN sin ORDER BY, puede romperse al cruzar el 2× con datos nuevos.

**Magnitud**: bajo. Tests actuales pasan, pero es deuda potencial.

---

## 3. Lista de reparaciones (lo que se rompió o quedó débil)

| # | Item | Severidad | Estado |
|---|------|-----------|--------|
| R1 | **Detección de stats stale en EXPLAIN + warning si P5c usa stats > X días vieja** | alta | ✅ entregada ([ADR-0074](adr/0074-r1-stats-stale-detection.md)) |
| R2 | **Calibrar `INDEX_BREAKEVEN_SELECTIVITY` contra `gabybench` real** | alta | ✅ entregada ([ADR-0081](adr/0081-r2-index-breakeven-calibration.md)): 0.20 → 0.10 |
| R3 | **Calibrar threshold P5d (1.5× vs 2× vs 3×)** | media | ✅ cerrada con outcome — instrumentación ([ADR-0082](adr/0082-r3-p5d-swap-threshold-instrumentation.md)) + sweep empírico inconcluso ([ADR-0085](adr/0085-r3-cont-p5d-sweep-results.md)): default 2.0 stays |
| R4 | **HLL test sobre DECIMAL/DATE/UUID/TEXT** | media | ✅ entregada (sin ADR; ver `r4_*` tests) |
| R5 | **TRUNCATE TABLE limpia stats persistidas** | media | 🔴 abierto (bloqueado por implementar TRUNCATE como statement) |
| R6 | **Composite index lookup con bucket gigantesco** — refinamiento de P5c | media | ✅ entregada ([ADR-0077](adr/0077-r6-composite-bucket-size-check.md)) |
| R7 | **Mensaje EXPLAIN del P5c skip — sugerir re-`ANALYZE` si stats viejas** | baja | ✅ entregada ([ADR-0078](adr/0078-r7-p5c-reanalyze-hint.md)) |
| R8 | **UPDATE/DELETE con composite-eq aún hacen FullScan** | media | ✅ entregada ([ADR-0076](adr/0076-r8-update-delete-composite-fast-path.md)) |
| R9 | **`COUNT(DISTINCT col)` sobre JOIN** — ADR-0066 Gap 1 residual | baja | ✅ entregada ([ADR-0079](adr/0079-r9-count-distinct-over-join.md)) |
| R10 | **USING/NATURAL JOIN — EXPLAIN heurística conservadora** | baja | ✅ entregada ([ADR-0080](adr/0080-r10-using-natural-explain.md)) |

**6 de 10 cerradas.** Las 4 abiertas restantes están detalladas en
[TAREAS_PENDIENTES.md §4](TAREAS_PENDIENTES.md) con esfuerzo estimado.

---

## 4. Lista de mejoras (lo que aún no se rompió pero falta)

### 4.1 Fundacionales (la base que falta antes de seguir)

| # | Item | Por qué importa |
|---|------|---|
| M1 | **Auto-ANALYZE** (scheduler que dispare cuando la tabla cambió > X%) | Hoy las stats son responsabilidad manual del usuario. Sin esto, R1 (stale detection) es un workaround. |
| M2 | **gabybench reproducible en CI con tracking de regresiones** (el P6 declarado en el plan) | Sin esto no podemos calibrar R2/R3 ni detectar regresiones de P5c/P5d en futuros bloques. |
| M3 | **Property tests para JOIN reorder y P5c** (`proptest` sobre planes A vs B sobre los mismos datos) | Validar que cambios futuros del optimizer no introducen regresiones de correctness silenciosas. |
| M4 | **Fuzz testing del parser** (`cargo fuzz`) — declarado en TAREAS_PENDIENTES como prioridad alta | Una hora limpia de fuzz es línea creíble en README; cero no lo es. |

### 4.2 Planner y stats (continuación natural)

| # | Item | Por qué importa |
|---|------|---|
| M5 | **Multi-column stats** (correlación inter-columna) — `CREATE STATISTICS (col_a, col_b)` | Resolvería 2.2 (independencia AND). PostgreSQL lo tiene desde 10. Aquí: ~600 LOC nuevos + bump VERSION. |
| M6 | **EXPLAIN ANALYZE compara `est.match` vs `actual=K`** | Diagnóstico directo de sesgos del estimator sin tener que correr queries separadas. |
| M7 | **Hints SQL** (`/*+ INDEX(t, idx_cat) */`, `/*+ HASH_JOIN(a, b) */`) | Override per-query del cost model cuando los stats fallan. Estándar en Oracle/MySQL/SQL Server. |
| M8 | **Prefix matching sobre composite index** (P5b-futuro / P5c-futuro) | `WHERE a=X` con `INDEX (a, b)` — requiere cambio de layout on-disk a lexicographic tuple-bytes. Bump VERSION. |
| M9 | **Base table reorder para INNER chains** (extensión natural de P5d) | Hoy P5d swap solo el step current. Reorder global requeriría refactor de `build_join_scope`. |

### 4.3 Resiliencia y operación

| # | Item | Por qué importa |
|---|------|---|
| M10 | **Detectar stats stale con `analyzed_at_nanos` ya persistido** | Ya tenemos el dato; falta consumirlo. EXPLAIN podría mostrar `[stats hace 7d 5h]` y P5c bajarse cuando son muy viejas. |
| M11 | **WAL-mode opt-in** (ADR-0018 pendiente, archivado por riesgo) | Habilitaría lectores concurrentes, abriría espacio comparativo serio vs SQLite. |
| M12 | **SAVEPOINT + ROLLBACK TO SAVEPOINT** (T1 declarado en STATUS pendientes) | Hoy `ROLLBACK` descarta todo el batch — no se puede recuperar parcialmente de errores. |
| M13 | **Cross-request transactions en server HTTP** (T2) | El server actual recrea transacción por request. Sin esto, no se puede escribir un cliente que haga BEGIN/INSERT/INSERT/COMMIT. |

### 4.4 Calidad de código y DX

| # | Item | Por qué importa |
|---|------|---|
| M14 | **Refactor de `exec_select_with_where`** (~2 000 LOC en una función) | Se está volviendo difícil de razonar. Extraer plan-selection a un struct/módulo aparte. |
| M15 | **`#[doc(hidden)]` o módulo privado para helpers de P5\*** | `estimate_selectivity`, `find_matching_composite_index`, etc. están en el top-level de `sql.rs`. Conviene encapsularlos. |
| M16 | **Tests de regresión empíricos** — guardar `est.match` esperado vs el que devuelve el motor para 20+ queries representativas, fallar si la diferencia supera X% | Detecta regresiones de calibración sin tener que correr el bench entero. |

---

## 5. Análisis de riesgo de la sesión completa

### Qué salió bien

- **0 regresiones funcionales en CI** a través de 6 pushes consecutivos sobre el motor.
- Cada bloque entregó tests propios (+36 tests en suite, suite total 781 verde).
- ADRs documentan honestamente las limitaciones (no se escondió la asunción
  de independencia AND, ni que `INDEX_BREAKEVEN` no está calibrado).
- VERSION bump solo donde fue necesario (P4); 5 de los 7 bloques zero-bump.

### Qué salió frágil

- **No hay bench que valide perf**. P5c y P5d cambian el plan; no medimos el
  impacto real. Confiamos en el cost model teórico.
- **Test coverage de los CASOS QUE P5c/P5d DEBERÍAN ATACAR es indirecto**.
  Verificamos que el plan elegido es el esperado en cierto setup, no que
  la query es más rápida. Sin bench → ciego al beneficio real.
- **Acumulación de heurísticas no calibradas**: `INDEX_BREAKEVEN = 0.2`,
  `threshold P5d = 2×`, `DEFAULT_EQ_SELECTIVITY = 0.1`, `K=10` para MCV,
  `buckets=16` para histograma, `SAMPLE_CAP=10000`. Cada una razonable
  individualmente; juntas, decisiones del optimizer son sensibles a la
  combinación.

### Qué pasaría si se aplica más optimizer sin las reparaciones

Sumar P5f/P5g (reorder global de joins, estimación cross-JOIN) **sobre las
limitaciones actuales** amplifica las decisiones malas. Si P5c ya puede
equivocarse por stats stale o correlación, P5f equivocaría JOINs enteros.

**Conclusión operativa**: antes de seguir agregando optimizer, hacer
**R1, R2, M2 (gabybench en CI)** en algún orden. Esto da la red de
seguridad que faltó durante esta sesión.

---

## 6. Recomendación de orden — actualizada 2026-06-15

Original (antes de hacer nada): M2 → R1 → R2 → R4 → M4 → R5/R8/R9.

**Lo que efectivamente se hizo en la sesión 2026-06-11**: R1 → R4 → M2 → R8 → R6.

**Lo que se hizo en la sesión maratón 2026-06-15** (10 pushes en orden):
R7 → R9 → R10 → R2 → R3 → bench-fix → ANSI-fix → M3 → R3-cont → docs sweep.

Diferencias con la recomendación previa:
- R2 calibró con UN sweep de smoke (no 5-10 cross-commit). Suficiente porque
  el ratio C_RANDOM/C_SEQ resultó tan distinto al textbook que la decisión
  fue obvia.
- R3 se entregó como "calibración con outcome inconcluso, default stays"
  en vez de "data fija el threshold". Honesto.
- M4 (`cargo fuzz`) sigue abierto — único bloqueo: setup inicial. Pasa a
  ser el item más importante de la próxima sesión.
- ANSI fix apareció como bonus mientras se debugueaba el bench all.

**Próximo orden recomendado desde 2026-06-15** (cuando vuelvas):

1. **M4 — `cargo fuzz` sobre el parser**. 1 hora limpia es la línea de
   README que sigue faltando. Único bloqueo: setup inicial de `cargo-fuzz`.
2. **M6 — EXPLAIN ANALYZE compara `est.match` vs `actual=K`**. Diagnóstico
   directo del bias del estimator. Hoy ANALYZE re-ejecuta; solo falta agregar
   la columna comparativa. ~200 LOC.
3. **Proptest sobre Pager** (no sobre planner, eso es M3 ya cerrada).
   Misma técnica zero-deps que M3 — defiende correctness de
   begin/insert/commit/rollback bajo input adversarial.
4. **M12 — SAVEPOINT + ROLLBACK TO SAVEPOINT**. Desbloquea M13 (cross-request
   tx en server HTTP).
5. **Demo pública de Fase 5** (MCP gateway + vector search + audit). La
   palanca real para que el repo "sea visto".

**Lo que NO conviene hacer antes**: M5 (multi-col stats) y M9 (JOIN reorder).
Sumar más optimizer sin sostener M4 (fuzz) → construir sobre arena.
M11 (WAL-mode) es Fase 6.

**Otra opción razonable**: **parar acá**. Las 7 de 9 tensiones cerradas, la
suite es 828/828 verde, gabybench all 71/71 queries sin SKIPs, 3 redes property-based defienden
el optimizer contra regresiones futuras, ANSI fix elimina la asimetría más
visible con clientes portados. Es un punto de descanso natural con el motor
en mejor estado que nunca antes. El siguiente eje (más optimizer / más DDL /
más ergonomía / demo pública) puede esperar a contexto externo (tweet,
proyecto comparativo activo, etc).
