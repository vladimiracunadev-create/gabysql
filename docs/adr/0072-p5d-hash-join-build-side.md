# ADR-0072: P5d — hash join build-side selection por cardinalidad

**Fecha:** 2026-06-11
**Estado:** Aceptado
**Bloque:** P5d (sub-tarea de P5 — planner-as-optimizer)
**Antecede:** P5e (algorithm choice JOIN)
**Relaciona:** Issue #6 (2026-05-27) — hash join inicial

## Contexto

`exec_select_joined` ejecuta los JOINs left-deep: `current` (acumulado
de joins previos) JOIN `right_rows` (próxima tabla). Para INNER JOIN
con equi-predicado, Issue #6 (2026-05-27) introdujo hash join
construyendo el HashMap **siempre sobre `right_rows`**.

Esto sub-óptimo cuando `current` es mucho más chico que `right_rows`.
Ejemplo: tras un JOIN previo y un WHERE muy selectivo, `current.len()
= 10` filas. La próxima tabla `big` tiene 100k. Build hash sobre 100k
→ memoria + tiempo gastados en hashear filas que ningún row de
`current` va a probear.

Path óptimo: build hash sobre las 10 de `current`, probear con las
100k. Mismo resultado, build trivial.

P5d implementa el swap automático cuando hay ventaja clara.

## Decisión

### Threshold: 2×

```rust
let swap_build_side = current.len() > right_rows.len() * 2;
```

Solo invertimos cuando `current` es **más del doble** que
`right_rows`. Por qué 2×:

- Hash join cost ≈ `|build| + |probe| × probes_per_key`. Build es
  esencialmente lineal en `|build|`. La memoria es proporcional a
  `|build|`.
- A 2× la ganancia ya supera el overhead de elegir y el riesgo de
  introducir cambios sutiles en el orden de output.
- A 1× la diferencia es marginal — no vale el riesgo.

Sin stats, sin estimación. Las cardinalidades `current.len()` y
`right_rows.len()` son **conocidas exactamente** al momento del JOIN
(ya fueron materializadas). No hay estimación: es cost-based real.

### Mecánica del swap

Cuando `swap_build_side`:
- Hash se construye desde `current` indexado por `left_probe_key`.
- Probe es `right_rows` indexado por `right_index_key`.
- Las funciones `left_matched[li]` y `right_matched[ri]` se siguen
  marcando correctamente — solo cambia el orden en que se iteran.

Cuando NO `swap_build_side`: comportamiento idéntico a Issue #6.

### Sin impacto en LEFT/RIGHT/FULL JOIN semánticamente

El swap NO cambia qué rows aparecen ni con qué NULLs. Las arrays
`left_matched` y `right_matched` siguen siendo la fuente de verdad
para el post-JOIN null-fill. El resultado set es **idéntico** módulo
orden — y SQL sin ORDER BY no garantiza orden de todos modos.

Verificado en `p5d_left_join_swap_preserva_null_fill`: LEFT JOIN con
swap activo, las filas sin match siguen apareciendo con NULL.

### NO afecta

- **CROSS JOIN**: no entra al hash-join path (no hay predicate).
  Cae al nested-loop.
- **Index-loop fast-path**: corre antes del hash-join. Si match,
  evita ambos paths.
- **Nested-loop fallback**: cuando el hash plan no resuelve keys.
  Sin swap.

## Alternativas consideradas

1. **Reorder de toda la chain de JOINs** (smallest-base-first).
   - Considerado pero descartado por scope: requiere reescribir
     `build_join_scope`, re-resolver predicados, y solo es seguro
     para INNER JOIN. Riesgo correctness alto vs ganancia
     incremental. Roadmap para P5d+1.

2. **Threshold dinámico basado en memoria disponible**.
   - Descartado: agrega tuning runtime. 2× es robusto y
     no-paramétrico. Si en producción se ve que 2× no es óptimo,
     ajustamos la constante.

3. **Swap en cada hash join sin threshold** (siempre lado menor).
   - Descartado por ordering: tests sin ORDER BY asumen estabilidad
     del orden actual. Cambiar el iterador exterior cambia el orden.
     Threshold 2× minimiza casos donde el swap NO ayuda pero sí
     cambia orden.

4. **Anotar la decisión en EXPLAIN**.
   - Diferido: requiere refactor de `explain_select_joined` para
     simular el swap. No hay un path EXPLAIN-natural hoy. Lo
     dejamos para P5e que ya planea anotar choice de algoritmo.

## Tests

3 tests nuevos en `tests/integration_test.rs` (suite `p5d_*`):

- `p5d_inner_join_correctness_small_left_grande_right`: small=2 filas,
  big=6 filas → no dispara swap (`2 > 6×2` falso). El resultado
  ordenado por `b.id` es el esperado (6 filas joined).
- `p5d_inner_join_correctness_left_grande_swap_dispara`: big=10 filas,
  tiny=2 filas → swap activo (`10 > 2×2` true). Las 10 filas se
  preservan, el JOIN es correcto.
- `p5d_left_join_swap_preserva_null_fill`: LEFT JOIN con `ref_id=99`
  sin match en `tiny` (label=NULL) — regression de que el swap NO
  rompe la semántica de OUTER JOIN.

Suite total: **777 passing** (774 → +3 P5d). Verificado vía Docker
`rust:1.94-bookworm`.

## Consecuencias

**Positivas**

- (+) Memoria del hash join acotada por la cardinalidad MÍNIMA de
  los dos lados (cuando hay diferencia clara).
- (+) Conservador: threshold 2× evita cambiar comportamiento en
  casos limítrofes. Tests existentes pasan sin tocar.
- (+) Cero estimación — usa cardinalidades reales. Sin riesgo de
  decisiones malas por stats sesgadas.
- (+) Aplica a INNER, LEFT, RIGHT, FULL — el swap es semánticamente
  correcto en todos.

**Negativas / Limitaciones honestas**

- (-) Solo afecta el step CURRENT del left-deep chain. Si `current`
  se inflara por un join previo subóptimo, P5d corrige aguas abajo
  pero no upstream. Reorder global queda para futuro.
- (-) Threshold 2× es heurístico — no calibrado vs el bench. Si se
  observa que 1.5× o 3× rinde mejor, ajustar la constante.
- (-) Si `current.len() == right_rows.len()` (exactamente), no swap.
  Es el caso esperado — sin ventaja.
- (-) No anota en EXPLAIN. El usuario no ve si el swap se activó.
  Diferido a P5e.

## Limitaciones / Trabajo futuro

- **Base table reorder**: cuando hay 3+ tablas INNER JOIN'd, elegir
  la más chica como base. Requiere refactor de
  `build_join_scope` para re-resolver predicados según el nuevo orden.
- **Reorder de joins por costo**: para INNER chains de N tablas,
  exhaustive search (N ≤ 6) o DP-Selinger.
- **Calibración**: medir 1.5× vs 2× vs 3× en `gabybench` con
  workloads JOIN-heavy.
- **EXPLAIN annotation**: mostrar `hash join (build=current size=10,
  probe=right size=100)` cuando el swap se activó.
