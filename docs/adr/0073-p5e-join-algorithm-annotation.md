# ADR-0073: P5e — EXPLAIN anota el algoritmo real de JOIN

**Fecha:** 2026-06-11
**Estado:** Aceptado
**Bloque:** P5e (sub-tarea de P5 — planner-as-optimizer)
**Cierra:** Fase 3 (Performance / Planeación)
**Relaciona:** Issue #6 (hash join), Bloque D (index-loop)

## Contexto

Pre-P5e, `explain_select` emitía siempre `"(nested-loop)"` como
descripción del algoritmo de JOIN. **Mentira**: el dispatcher de
`exec_select_joined` decide entre tres paths:

1. **Index-loop** (`run_index_loop_join`): cuando INNER/LEFT JOIN y
   el predicate apunta a la PK o columna indexada del RHS. O(N × log M).
2. **Hash join** (Issue #6): cuando hay equi-predicate y las keys
   resuelven en ambos lados. O(N + M).
3. **Nested-loop**: CROSS JOIN o predicates no-equi. O(N × M).

El usuario consultando EXPLAIN no podía saber cuál de los tres se
iba a usar. Eso anula medio valor de EXPLAIN — el otro medio
(detectar SCAN paths) ya funcionaba.

P5e cierra esto: EXPLAIN refleja la elección real. Además anota
las cardinalidades reales de cada lado del JOIN cuando hay stats.

## Decisión

### `classify_join_algorithm(join, base_table) -> String`

Heurística estática (sin construir scope ni ejecutar subqueries) que
mira:

1. **CROSS JOIN** o ningún predicate → `"nested-loop, ~O(N×M)"`.
2. **RHS derived/VALUES** → `"hash join, ~O(N+M)"` (no hay índice
   posible sobre rows materializadas en memoria).
3. **JoinKind ∈ {RIGHT, FULL}** → `"hash join"` (el dispatcher real
   no entra al index-loop para esos casos — ver comentario al
   código existente sobre por qué).
4. **JoinKind ∈ {INNER, LEFT}** con ON explícito:
   - Identifica el lado del predicate que apunta al RHS via qualifier.
   - Si la columna RHS es PK → `"index-loop on `qual`.`col` (PK)"`.
   - Si tiene índice secundario → `"index-loop on `qual`.`col`
     (index `idx_name`)"`.
   - Si no → `"hash join"`.
5. **USING/NATURAL**: reportamos `"hash join"` conservadoramente
   (sin scope completo no podemos resolver el nombre derivado).

### `join_side_stats(table) -> String`

Cuando hay stats para la tabla, anota ` [<table>.rows=N]`. Llamado
una vez por lado del JOIN.

### Ejemplo de salida

Pre-P5e:
```
1.join.1: INNER JOIN `big` ON (...) (nested-loop)
```

Post-P5e (con stats):
```
1.join.1: INNER JOIN `big` ON (...) (index-loop on `big`.`id` (PK), ~O(N × log M)) [small.rows=2] [big.rows=8]
```

```
1.join.1: CROSS JOIN `b` (nested-loop, ~O(N×M))
```

## Por qué heurística estática y no scope completo

Construir el `JoinScope` requiere ejecutar `materialize_derived_table`
para fuentes derivadas — eso ejecutaría subqueries dentro de EXPLAIN.
Cambio de comportamiento no-trivial.

La heurística estática captura:

- ✅ INNER/LEFT con `ON` explícito sobre PK o índice — el caso más
  común.
- ✅ CROSS y predicate-less.
- ✅ Distinción RHS real vs derived/VALUES.

Pierde:

- ❌ USING/NATURAL: el dispatcher real puede ir a index-loop si el
  nombre derivado coincide con la PK del RHS. Reportamos hash; el
  engine puede sorprender con index-loop. Es una imprecisión
  conservadora.
- ❌ Predicates complejos con `AND` (el dispatcher actual tampoco
  los soporta para index-loop — solo single equi).

## Tests

4 tests nuevos en `tests/integration_test.rs` (suite `p5e_*`):

- `p5e_explain_join_pk_anota_index_loop`: `a.b_id = b.id` con `b.id`
  PK → EXPLAIN debe contener `"index-loop"` y `"PK"`.
- `p5e_explain_join_index_secundario`: `a.ref_b = b.code` con
  `CREATE INDEX idx_b_code ON b(code)` → EXPLAIN contiene
  `"index-loop"` y el nombre del índice.
- `p5e_explain_cross_join_anota_nested_loop`: `CROSS JOIN` →
  `"nested-loop"` y `"O(N×M)"`.
- `p5e_explain_join_anota_stats_de_ambos_lados`: ANALYZE en ambas
  tablas → EXPLAIN incluye `small.rows=2` y `big.rows=8`.

Suite total: **781 passing** (777 → +4 P5e). Verificado vía Docker
`rust:1.94-bookworm`.

## Alternativas consideradas

1. **Construir scope completo en EXPLAIN**.
   - Descartado por side-effect: materializar derived tables
     ejecuta subqueries. EXPLAIN debe ser puramente descriptivo
     (no `ANALYZE`). Romper esta promesa cambiaría tests existentes
     que llaman EXPLAIN sobre derived/CTE.

2. **Anotar también build/probe del hash join**.
   - Diferido: requiere predecir `current.len()` post-JOINs previos,
     que sin ejecución es imposible. P5d hace la decisión a runtime;
     EXPLAIN no la puede simular sin correr.

3. **Estimar `est.rows` del JOIN result usando estimate_selectivity
   cruzado entre las dos tablas**.
   - Diferido a un eventual P5f. Para JOIN equi sobre cols indexadas,
     `est.rows ≈ |left| × |right| × (1/max(ndv_left, ndv_right))` es
     el shape clásico (Selinger). Aplicable pero scope creep
     respecto al objetivo de "anotar algoritmo".

## Consecuencias

**Positivas**

- (+) EXPLAIN deja de mentir sobre algoritmos. Útil para diagnosticar
  por qué un JOIN es lento (¿hash o nested? ¿hay índice?).
- (+) Stats integradas en la salida del JOIN (no solo del SCAN).
- (+) Cierre formal de Fase 3 — EXPLAIN ahora refleja TODO el planner
  (SCAN paths, composite index lookup, P5c skip, JOIN algorithm).

**Negativas / Limitaciones honestas**

- (-) USING/NATURAL → hash conservador. Si el dispatcher real va a
  index-loop, EXPLAIN no lo dice. Mejor que mentir con `nested-loop`,
  pero no perfecto.
- (-) Predicates no-equi compuestos (`a.x = b.y AND b.z > 5`) — el
  dispatcher actual ni siquiera intenta index-loop sobre esos; ambos
  caen a hash o nested. EXPLAIN refleja correctamente pero la causa
  raíz es la limitación del dispatcher, no de P5e.
- (-) No simula el swap de P5d (build-side selection). EXPLAIN no
  muestra qué lado se eligió como build.
- (-) `RIGHT JOIN` y `FULL JOIN` se reportan como hash en EXPLAIN.
  El dispatcher real puede usar hash o nested-loop dependiendo del
  predicate. La heurística no distingue — siempre hash. Imprecisión
  documentada.

## Limitaciones / Trabajo futuro

- **EXPLAIN con scope real** (P5f / Fase 4): refactor para construir
  el scope sin side-effects (versión "dry-run" de
  `materialize_derived_table`). Resolvería USING/NATURAL correctamente.
- **Estimación de cardinality post-JOIN**: combinar
  `estimate_selectivity` cruzado entre tablas para anotar `est.rows=K`
  del JOIN result.
- **Anotar P5d** (build-side swap): EXPLAIN debería mostrar
  `"hash join (build=current size=10, probe=right size=100)"` cuando
  el swap se va a activar. Requiere predecir runtime cardinality —
  difícil sin scope.
- **Hints SQL** (`/*+ HASH_JOIN(a, b) */`): override per-query del
  algorithm.
