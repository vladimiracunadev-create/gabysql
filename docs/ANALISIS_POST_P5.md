# Análisis del producto tras la sesión P3b → P5e (2026-06-09 al 2026-06-11)

> **Propósito**: dejar registrado qué cambió, qué quedó frágil, qué características
> chocan entre sí, qué hay que reparar y qué hay que mejorar. Es un diagnóstico
> después de 8 pushes en 48h — el ritmo expuso tensiones que conviene nombrar
> antes de seguir construyendo.

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

**Tests**: 745 → 781 (+36). Pasaron en 4 runners de CI (ubuntu/macos/windows/docker).

**Líneas tocadas**: ~3 500 LOC en `src/sql.rs`, `src/catalog.rs`, `src/storage.rs`,
`tests/integration_test.rs`, más 6 ADRs nuevos (0067–0073).

---

## 2. Tensiones y choques entre características

### 2.1 Stats stale silenciosas (heredado P3 → amplificado por P5c)

**Problema**: `ANALYZE TABLE` corre una vez, las stats persisten. Pero si la
tabla cambia mucho después, ni el motor ni EXPLAIN lo señalan. Pre-P5c era
informativo nomás. **Post-P5c es load-bearing**: el plan cambia según stats.
Una tabla con 1k filas analizadas hace tiempo, y ahora 1M filas → P5c puede
elegir FullScan creyendo que `est.match` es razonable cuando en realidad
escanea 1M filas para devolver 1.

**Síntoma esperable**: queries que iban rápido se ralentizan tras un crecimiento
de tabla, sin error visible. El usuario no sabe que sus stats son obsoletas.

**Magnitud**: alto — es la principal externalidad de P5c.

### 2.2 Independencia asumida en AND (heredado P5a → propagado a P5c)

`estimate_selectivity(And(a, b)) = sel(a) × sel(b)`. Asume columnas
independientes. En la realidad están correlacionadas (`marca='Toyota' AND
modelo='Corolla'` no es 0.01 × 0.001 — es más cerca de 0.001 porque modelo
implica marca). P5c usa esa estimación para decidir FullScan vs index.

**Síntoma esperable**: en queries con `WHERE a AND b` muy correlacionadas,
P5c sub-estima `est.match` y elige el path equivocado. Falla silenciosa.

**Magnitud**: medio — afecta solo a queries con AND correlacionado.

### 2.3 `INDEX_BREAKEVEN = 0.2` sin calibración empírica

El umbral viene del cost model teórico `C_RANDOM ≈ 5 × C_SEQ`. No está calibrado
contra `gabybench`. Si en la práctica nuestro motor tiene `C_RANDOM ≈ 2 × C_SEQ`
(SSD cache-friendly), el umbral correcto sería ~0.5. Estamos saltando al
FullScan demasiado pronto.

**Magnitud**: medio. Requiere experimentación con gabybench → ajustar la
constante.

### 2.4 Threshold de P5d (2×) sin medición

Mismo problema en menor escala. El swap del build-side se dispara a 2×.
Pudo haber sido 1.5× o 3×. Sin gabybench JOIN-heavy no sabemos.

**Magnitud**: bajo. El cambio es semánticamente seguro; solo el threshold
óptimo es desconocido.

### 2.5 Mensaje en EXPLAIN ambiguo entre P5c y "no hay índice"

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

### 2.7 Composite index NO-UNIQUE con bucket gigantesco

Si el composite index `(qty, precio)` se usa sobre valores muy repetidos
(p.ej. todas las filas con `qty=5, precio=100`), el bucket guarda miles de
PKs. La lookup devuelve esos PKs, hace miles de random reads — exactamente
el caso que P5c quiere evitar. Pero P5c NO se aplica a composite indexes
(solo a single-col). El bench tipo "warehouse con valores skewed" sufriría.

**Magnitud**: medio. No es un bug; es un caso edge que P5c no cubre por
diseño.

### 2.8 HLL bias en columnas con tipos no-INT

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

| # | Item | Severidad | Esfuerzo |
|---|------|-----------|----------|
| R1 | **Detección de stats stale en EXPLAIN + warning si P5c usa stats > X días vieja** | alta | 1 push (~200 LOC) |
| R2 | **Calibrar `INDEX_BREAKEVEN_SELECTIVITY` contra `gabybench` real** (correr el bench, medir, ajustar) | alta | 1 push (~50 LOC + análisis) |
| R3 | **Calibrar threshold P5d (1.5× vs 2× vs 3×)** mismo método | media | 1 push (~50 LOC + análisis) |
| R4 | **HLL test sobre DECIMAL/DATE/UUID/TEXT** — verificar empíricamente que la distribución no esté sesgada | media | 1 push (~250 LOC tests) |
| R5 | **TRUNCATE TABLE limpia stats persistidas** (heredado P3b — falta el statement) | media | parte de un bloque mayor sobre TRUNCATE |
| R6 | **Composite index lookup con bucket gigantesco** — P5c extendido a composite (chequear `est.match` antes del lookup) | media | 1 push (~150 LOC) |
| R7 | **Mensaje EXPLAIN del P5c skip — sugerir re-`ANALYZE` si stats viejas** | baja | 1 push (~30 LOC) |
| R8 | **UPDATE/DELETE con composite-eq aún hacen FullScan** — extender P5b a esos paths | media | 1 push (~200 LOC, reusa helpers) |
| R9 | **`COUNT(DISTINCT col)` sobre JOIN** — heredado de ADR-0066 Gap 1, no cerrado | baja | 1 push (~150 LOC) |
| R10 | **USING/NATURAL JOIN — EXPLAIN heurística conservadora** — completar con resolución de scope dry-run | baja | 1 push (~200 LOC) |

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

## 6. Recomendación de orden

Si tuviera que priorizar el próximo bloque entre todos los anteriores:

1. **M2 — gabybench reproducible en CI** (declarado como P6 en STATUS). Sin
   esto, R2/R3 son adivinaciones.
2. **R1 — detección de stats stale** (analyzed_at_nanos ya persiste — falta
   consumirlo en EXPLAIN + en la decisión de P5c).
3. **R2 — calibrar `INDEX_BREAKEVEN`** una vez M2 esté.
4. **R4 — tests de HLL sobre tipos no-INT** (independiente de M2; previene
   futuros sustos como el hot-fix de P4).
5. **M4 — `cargo fuzz` sobre el parser** (declarado en TAREAS_PENDIENTES,
   sigue ahí; aporta confianza independiente del optimizer).
6. **R5/R8/R9 — cierre de pendientes residuales** del bloque previo.

Cualquier bloque P5f+ debería esperar al menos a (1) y (2).
