# ADR-0084: M3 — Property tests sobre el planner

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Bloque:** M3 (fundacional de Fase 4 — red de seguridad del optimizer)
**Origen:** [docs/TAREAS_PENDIENTES.md §3](../TAREAS_PENDIENTES.md) y
[docs/ANALISIS_POST_P5.md §4.1 M3](../ANALISIS_POST_P5.md) — mencionado como "deuda más cara"
después de la sesión P5.
**Refina:** ADR-0071 (P5c), ADR-0072 (P5d), ADR-0077 (R6), ADR-0081 (R2).

## Contexto

Tras la sesión P5 (P5c + P5d + R6), el planner de gabysql es **cost-based**:
las stats persistidas determinan QUÉ path corre.

- P5c: si `sel >= 0.10` (R2 calibrado), salta el índice y va a FullScan.
- P5d: si `current.len() > right_rows.len() × 2.0`, swap del build side
  del hash join.
- R6: post-lookup bucket-size check sobre composite index.

Los 3 cambios son **plan-cambios**: el motor toma decisiones distintas
según las stats. La correctness invariante crítica es:

> **El resultado de un SELECT NUNCA debe cambiar según si ANALYZE corrió
> o no, ni cómo está calibrado el threshold.**

Si esa invariante se rompe, el motor mintió: devolvió un set distinto
porque eligió un path distinto. Un test unitario tradicional solo
verifica casos específicos elegidos por el dev — fácil dejar pasar el
caso adversarial. Property tests cubren la combinatoria.

## Decisión

Crear `tests/proptest_planner.rs` (test binario aparte para no agrandar
el monolito de `integration_test.rs`) con 3 property tests
hand-rolled — **zero deps externas** (alinea con ADR-0001).

### Infraestructura

- **LCG determinístico** con las mismas constantes que `gabybench`
  (Numerical Recipes seed). Determinismo crítico: cada fallo imprime el
  seed que lo causó, reproducible 1:1.
- **Generadores de fixtures**:
  - `populate(db, seed, n)`: tabla `t(id PK, a INT, b TEXT, c INT)` con
    índices sobre `a` y `b`. Distribución de `a` skewed (70% en 0..5,
    30% en 5..20) — ejerce explícitamente alta vs baja selectividad,
    que es exactamente el espacio que P5c discrimina.
  - `populate_join(db, seed)`: `u(id, label)` 50 rows + `o(id,
    user_id, val)` 300 rows con índice en `o.user_id`. Cardinality
    asimétrica para ejercer P5d swap.
- **`random_where(rng)`**: genera 10 shapes distintos — Eq sobre PK,
  Eq sobre col indexada, Compare, Between, AND, OR, IN list.

### Tests

1. **`m3_select_results_invariant_with_vs_without_analyze`** (50 iters ×
   3 queries = **150 comparaciones**): para cada `(seed, WHERE)`, popula
   2 DBs idénticas, corre ANALYZE en una sola, ejecuta `SELECT id FROM
   t WHERE <generado> ORDER BY id` en ambas. Los `Vec<Vec<Value>>` deben
   ser iguales.
2. **`m3_count_invariant_with_vs_without_analyze`** (30 iters): mismo
   patrón pero con `COUNT(*) WHERE <generado>`. Cardinalidad invariante
   sin necesidad de ordenar.
3. **`m3_inner_join_invariant_with_vs_without_analyze`** (20 iters × 3
   queries fijas): JOIN inner sobre `u × o`. Ordenamos los resultados
   en Rust (`sort_by` sobre el `format!("{:?}", row)`) para defenderse
   del orden no determinístico que P5d swap puede producir sin
   `ORDER BY` robusto. La invariante real es el **set**, no el orden.

### Notas de implementación

- **Sin proptest crate**: cumple zero-deps. El shrinking automático no
  está disponible, pero el seed determinista permite re-correr y
  bisectar a mano si hace falta.
- **Determinismo cross-platform**: el LCG usa `u64` con wrapping arith;
  mismo resultado en x86-64, ARM, Windows/Linux. La distribución skewed
  de `a` no depende de `f64` (que sí varía).
- **DB temporales aisladas por seed** (`std::env::temp_dir() /
  gby_proptest_{label}_{seed:x}.db`). Cleanup explícito en cada
  iteración — no leak de archivos.

## Consecuencias

### Positivas

- **Primera red de seguridad real para el optimizer**. Antes de M3,
  cualquier bloque P5* podía introducir regresiones de correctness que
  un test unitario no cubría — porque los unitarios prueban *un caso
  elegido por el dev*, no el espacio adversarial.
- **150 + 30 + 60 = 240 comparaciones** automáticas por corrida. Cada
  CI run las atraviesa.
- **Habilita futuras Fase 4** (más optimizer: M5 multi-col stats, M7
  hints SQL, M9 base table reorder) sobre una base de confianza
  empírica. Cada nuevo plan-cambio debe seguir pasando este pack.
- **Seed reproducible**: si CI falla con seed `0xabc...`, el dev re-corre
  localmente con el mismo seed y reproduce 1:1.

### Negativas / deuda

- **Sin shrinking automático**: si un test falla, el seed reproduce el
  fallo pero no minimiza el caso. Mitigación: bisección manual del
  rango de queries / mod de la distribución. Para shrinking serio
  hay que agregar `proptest` crate — choca con ADR-0001.
- **Cobertura limitada del WHERE grammar**: el generador cubre 10
  shapes. SQL real soporta cientos. Cada shape nuevo del motor (M7
  hints, window functions parametrizadas) probablemente requiere
  agregar un caso al generador.
- **JOIN test ordena en Rust** para evitar la deuda documentada de
  ADR-0072 (P5d swap puede cambiar orden sin ORDER BY robusto). Si en
  el futuro se garantiza orden estable, este sort se puede quitar.
- **Costo de tiempo**: el test JOIN tarda ~12s en suite (popula 2 DBs ×
  20 iters × 350 rows). Mientras quede bajo 30s no es un problema; si
  crece, mover a `#[ignore]` + correr nightly.

## Tests añadidos

- `m3_select_results_invariant_with_vs_without_analyze`
- `m3_count_invariant_with_vs_without_analyze`
- `m3_inner_join_invariant_with_vs_without_analyze`

**Suite total**: 810 integration + **3 proptest** = **813 verde**, 3
ignored. Sin regresiones.

## Alternativas consideradas

1. **Usar `proptest` crate**. Mejor shrinking, mejor reporting, ergonomía
   más pulida. Choca con ADR-0001 (zero-deps core). Rechazado por
   defecto; reconsiderable si el LCG-rolled hand-test demuestra ser
   limitante.
2. **Agregar tests al archivo `integration_test.rs` existente**. Ya
   tiene ~19k líneas. Property tests son una categoría conceptual
   distinta (regenerativa, no-determinista por shape, con seed seed
   logging diferente). Aparte se navega mejor.
3. **Solo correrlos en CI, no en local**. Innecesario — el costo es
   bajo (15s en local) y la confianza per-push vale más.

## Referencias

- [ADR-0071 — P5c cost-based fallback](0071-p5c-cost-based-fallback.md)
- [ADR-0072 — P5d hash join build-side](0072-p5d-hash-join-build-side.md)
- [ADR-0077 — R6 composite bucket-size check](0077-r6-composite-bucket-size-check.md)
- [ADR-0081 — R2 INDEX_BREAKEVEN calibration](0081-r2-index-breakeven-calibration.md)
- [ADR-0001 — Zero deps core](0001-rust-zero-deps-core.md) — define la restricción que justifica el hand-roll
