# 📌 Tareas pendientes

> **Este es el primer documento que se consulta cuando se pide "estado del proyecto"**. Antes que CHANGELOG, antes que ROADMAP. Lo que está acá es lo próximo a hacer, ordenado por prioridad real (no aspiracional).
>
> Cuando una tarea se cierra, se mueve a CHANGELOG con la entrada formal. No se borra de acá hasta entonces.

---

## 📍 Estado al 2026-06-15 — qué quedó después de la sesión maratón

> Si solo vas a leer una sección de este documento cuando volvés al proyecto, leé esta.

Sesión 2026-06-15 cerró **10 pushes** que terminaron de pagar la deuda de la sesión P5 + endurecieron el motor para iteración futura:

- **R7 / R9 / R10**: pulidos cosméticos del optimizer + cierre del residual ADR-0066 Gap 1 (COUNT DISTINCT sobre JOIN).
- **R2**: primera constante del optimizer (`INDEX_BREAKEVEN`) calibrada con números (0.20 → 0.10) + env var override para sweep futuros.
- **R3 + R3-cont**: instrumentación + sweep empírico de `P5D_SWAP_THRESHOLD`. Outcome explícito (datos inconclusos, 2.0 stays). R3 deja de ser deferred indefinido.
- **bench fix**: warmup del bench era no-tolerante a errores; síntoma del bug del bench, pero también descubrió un bug del motor.
- **ANSI fix**: `UPDATE/DELETE WHERE pk no-existe` ahora devuelve 0 filas (PostgreSQL/SQLite), no `[GBY-3006]`. Compatibilidad con clientes portados desde otros motores.
- **M3 (property tests sobre planner)**: la red de seguridad que faltaba para que futuros bloques de optimizer no introduzcan regresiones silenciosas de correctness. 240 comparaciones automáticas por corrida (con seed reproducible).

**Suite local**: 813 verde + 3 ignored (env-var tests). CI verde en 5 runners. gabybench all 12.9 min, 71 queries, 0 SKIPs.

**Lo que viene cuando vuelvas** (en orden recomendado, todos son pushes independientes):

1. **`cargo fuzz` sobre el parser** (§3 abajo) — 1 hora limpia de fuzz es la línea creíble de README que sigue faltando. Único bloqueo: setup inicial de `cargo-fuzz`. Sin él, no podemos defender "hardened parser".
2. **M6 — EXPLAIN ANALYZE compara `est.match` vs `actual=K`** (§6.5 abajo) — diagnóstico directo del bias del estimator. Hoy ANALYZE re-ejecuta; solo falta agregar la columna comparativa. ~200 LOC. Inmediatamente útil para detectar dónde P5c se equivoca.
3. **M12 — SAVEPOINT + ROLLBACK TO SAVEPOINT** (§6.5) — desbloquea recuperación parcial de batches. Pre-requisito de M13 (cross-request tx en server HTTP).
4. **Demo pública de Fase 5** (§6) — sigue siendo la palanca real para que el repo "sea visto". El motor está sólido; el gateway + vector search + audit con razón semántica son lo distintivo.

**Lo que NO conviene abrir todavía**: M5 (multi-col stats) y M9 (JOIN reorder) — sumar más optimizer sin sostener M3 → más fuzz/coverage es construir sobre arena. M11 (WAL-mode) es Fase 6, no antes.

---

## 🧭 Sobre el contexto operativo (leer antes de las prioridades)

Tres cosas que enmarcan toda esta lista. Si las olvido en una próxima conversación, recordármelas.

1. **`gabysql` es uno de varios proyectos en desarrollo paralelo del autor.** No es la única cosa que compite por tiempo. Las prioridades de este documento aplican **cuando se trabaja en `gabysql`**, no son un mandato calendárico. No hay "deberías estar haciendo X esta semana"; hay "cuando agarres el proyecto, lo siguiente es X".

2. **Las prioridades acá son recomendaciones técnicas, no estratégicas absolutas.** Vienen del análisis del estado del motor + del plan declarado de usarlo en un proyecto comparativo. Pero:
   - El mercado/tecnología/uso de DBs cambia de formas que nadie predice. Un proyecto puede tomar tracción por un tweet, por un caso de uso no anticipado, por estar listo cuando algo más se rompe.
   - Una recomendación técnica correcta no implica que ignorar la recomendación sea error. El criterio del autor sobre cuándo arrancar el proyecto comparativo, cuándo hacer público qué, y qué priorizar de la vida en general es de él, no de la lista.

3. **El orden de la lista puede invalidarse rápido**. Disparadores que justifican reordenar sin culpa:
   - Aparece interés externo concreto (alguien probó algo, pidió algo).
   - El proyecto comparativo se acerca o se aleja en el calendario.
   - Una decisión técnica de otro proyecto del repositorio toca cosas que `gabysql` también necesita.
   - Sale algo nuevo en el ecosistema (Rust stdlib, MCP spec, modelos LLM accesibles localmente) que cambia el costo de una de las fases.
   - Aparece pereza/aburrimiento con una tarea — válido, se salta o se reordena.

---

## 🔥 Prioridad alta — recomendaciones técnicas

> Vienen del balance honesto del estado actual + del análisis post-Fase 3 ([ANALISIS_POST_P5.md](ANALISIS_POST_P5.md)). Son las cosas que, **si las hacés cuando trabajes en este proyecto**, mueven más el dial.

### 1. ~~Construir `gabybench` mínimo~~ — ✅ entregada

**Estado al 2026-06-11**: cerrada. Existe como binario `src/bin/gabybench.rs`, soporta modo `all` (10 DBs, ~10 min) y modo `smoke` (microblog+orders_lines, ~1-2 min). CI corre el smoke en cada push y sube `bench/results.json` como artifact (job `bench`). Ver [ADR-0075](adr/0075-m2-gabybench-in-ci.md).

**Lo que falta de esta línea** (al 2026-06-15):
- ~~**R2**~~ — ✅ entregada ([ADR-0081](adr/0081-r2-index-breakeven-calibration.md)): 0.20 → 0.10 + env var override.
- ~~**R3**~~ — ✅ cerrada con outcome explícito ([ADR-0082](adr/0082-r3-p5d-swap-threshold-instrumentation.md) + [ADR-0085](adr/0085-r3-cont-p5d-sweep-results.md)): sweep inconcluso, default 2.0 stays.
- **Comparador entre runs**: el CI sube artifacts pero no hay diff automático contra baseline. Sin esto, una regresión de 50% en una query no falla el push. ÚNICA línea pendiente del gabybench original.

---

### 2. Cobertura SQL para comparaciones realistas — ✅ entregada

**Estado al 2026-05-26**: cerrada. JOINs ANSI completos (INNER/LEFT/RIGHT/FULL/CROSS/USING/NATURAL + index-loop optimization) entregados antes de la sesión 2026-05-25; `WHERE` completo (E1+E2+E3), agregados single-table, DML masivo (J), UPSERT/RETURNING (J2), funciones escalares + aritméticos (G1+G2+G3), subqueries (H), set ops + VALUES (I), DDL extendido (K1+K2) en sesiones 2026-05-25 y 2026-05-26. La superficie SQL operacional clásica está completa.

**Pendiente residual** (al 2026-06-11):
- `UPDATE ... FROM` — no implementado.
- `EXCLUDED.col` en UPSERT — no implementado.
- ~~`COUNT(DISTINCT col)` sobre JOIN~~ — ✅ cerrada 2026-06-15 por R9 ([ADR-0079](adr/0079-r9-count-distinct-over-join.md)).
- **`ORDER BY a, b` (multi-col)** — descubierto en R8 (2026-06-11): el parser solo acepta single-col.
- Ninguno bloquea el proyecto comparativo.

---

### 3. Fuzz testing + property tests — **parcial, mayoría abierta**

**Qué**:
- ~~`cargo fuzz` sobre `parse(...)` — 1 hora mínima sin panic ni `unwrap` fallido~~ — ✅ entregada 2026-06-15 ([ADR-0087](adr/0087-m4-fuzz-parser.md)). Hand-rolled (libFuzzer/AFL choca con Windows+GNU+ADR-0001); 1h limpia = **503.8M iters, 0 panics**. Evidencia: [`docs/fuzz/FUZZ-RUN-2026-06-15.md`](fuzz/FUZZ-RUN-2026-06-15.md). Próxima mejora: `cargo fuzz` real en CI Linux + fuzz sobre `exec()`.
- ~~`proptest` sobre el Pager~~ — ✅ entregada 2026-06-15 ([ADR-0086](adr/0086-pager-proptest.md)). 3 invariantes (commit visibility, rollback discards, chained tx integrity), ~5100 ops random por corrida.
- ~~**`proptest` sobre planner**~~ — ✅ entregada 2026-06-15 ([ADR-0084](adr/0084-m3-proptest-planner.md)). Hand-rolled zero-deps, 240 comparaciones por corrida sobre P5c/P5d/R6.
- Extender los 3 crash tests sintéticos actuales a 10+ escenarios. Pendiente.

**Por qué importa**: SQLite tiene millones de horas de fuzz acumuladas. **Una hora limpia de fuzz** es una línea creíble en el README; cero horas no lo es. Con M3 ya defendido sobre el planner cost-based, falta la fuzz cobertura del parser/storage para hablar de "hardened".

**Costo**: medio. `cargo fuzz` requiere config inicial + corpus + tiempo de ejecución. Choca con ADR-0001 (zero deps) si se quiere el crate `proptest` — para el Pager se puede hacer hand-rolled mismo enfoque que M3.

**Esfuerzo**: 1 intervención para fuzz setup + 1ª hora; 1 separada para Pager proptest.

---

### 4. Reparaciones del análisis post-P5 — abiertas

**De [ANALISIS_POST_P5.md](ANALISIS_POST_P5.md), las que quedan abiertas tras la sesión 2026-06-11**:

| # | Item | Esfuerzo |
|---|------|----------|
| ~~**R2**~~ | ~~Calibrar `INDEX_BREAKEVEN` con bench data~~ — ✅ entregada 2026-06-15 ([ADR-0081](adr/0081-r2-index-breakeven-calibration.md)): 0.20 → 0.10 + env var override | — |
| ~~**R3**~~ | ~~Calibrar threshold P5d~~ — ✅ cerrada 2026-06-15 con outcome explícito: instrumentación ([ADR-0082](adr/0082-r3-p5d-swap-threshold-instrumentation.md)) + sweep empírico inconcluso ([ADR-0085](adr/0085-r3-cont-p5d-sweep-results.md)). Default 2.0 stays. | — |
| **R5** | TRUNCATE TABLE limpia stats — **bloqueado por implementar TRUNCATE como statement** | parte de bloque mayor |
| ~~**R7**~~ | ~~Mensaje EXPLAIN P5c sugiere re-ANALYZE~~ — ✅ entregada 2026-06-15 ([ADR-0078](adr/0078-r7-p5c-reanalyze-hint.md)) | — |
| ~~**R9**~~ | ~~`COUNT(DISTINCT col)` sobre JOIN~~ — ✅ entregada 2026-06-15 ([ADR-0079](adr/0079-r9-count-distinct-over-join.md)) | — |
| ~~**R10**~~ | ~~USING/NATURAL JOIN — EXPLAIN heurística completa~~ — ✅ entregada 2026-06-15 ([ADR-0080](adr/0080-r10-using-natural-explain.md)) | — |

**Tensiones abiertas tras la sesión 2026-06-15** (de las 9 originales del análisis post-P5):
- **2.6** — RIGHT/FULL JOIN heurística imprecisa en EXPLAIN (documentado en ADR-0073; mitigado parcialmente por R10 sobre USING/NATURAL, pero RIGHT/FULL siguen aproximando).
- **2.9** — P5d swap puede cambiar orden de filas sin ORDER BY (deuda documentada; M3 defiende usando sort en Rust para el test, no resuelve la deuda original).

Las **7 restantes (2.1–2.5, 2.7, 2.8)** quedaron cerradas en esta sesión y la anterior.

---

## 🟡 Prioridad media — cosas que mueven la aguja del proyecto

### 5. Re-evaluar el modelo single-writer

**Qué**: hoy el motor es estrictamente single-writer (Mutex global en server.rs + file lock cross-process). Eso es defendible mientras el proyecto sea de aprendizaje, pero **el proyecto comparativo va a sufrir** porque cualquier benchmark concurrente da números peores que SQLite (que soporta lectores concurrentes vía WAL-mode).

**Opciones**:
- **A**: aceptar la limitación y elegir benchmarks single-writer. Honesto pero limita la comparativa.
- **B**: implementar lectores concurrentes con snapshot isolation simple. Requiere repensar el Pager. Es esencialmente parte del WAL-mode de ADR-0018 (Propuesta).
- **C**: implementar MVCC. Demasiado grande para este momento del proyecto.

**Por qué medio y no alto**: porque depende del scope que tome el proyecto comparativo. Si vas a comparar OLTP serio, esto es alto. Si vas a comparar features distintivas (vector search, audit, schema rico), esto importa menos.

**Esfuerzo**: opción A: 0 (decisión). Opción B: 1 ADR + ~600 LOC + tests. Opción C: rewrite parcial.

---

### 6. Demo pública de Fase 5 (MCP gateway + vector search + audit)

**Qué**: un repo o gist con un agente Claude/Cursor/etc. usando `gabysql-mcp` para hacer algo concreto — RAG sobre PDFs, audit log consultable, query asistida — y un README que muestre que funciona.

**Por qué importa**: para "llamar la atención" eventualmente (la razón por la que el repo es público), la diferenciación real está en el MCP gateway. No en el motor. El motor es decente; el gateway con vector search integrada y audit log con `reason` semántico es lo que no tiene ninguna otra DB del nicho. Pero hoy nadie ve eso porque no hay una demo simple.

**Esfuerzo**: 1 intervención + setup de un agente real para probarlo. Probablemente un repo separado en GitHub que linkee de vuelta.

---

### 6.5 Mejoras del optimizer/stats — del análisis post-P5

Items que NO son reparaciones (no hay nada roto), pero amplían lo entregado en la sesión 2026-06-10/11:

| # | Item | Por qué importa | Esfuerzo |
|---|------|---|---|
| **M1** | Auto-ANALYZE (scheduler que dispare cuando la tabla cambia >X%) | Hoy las stats son manuales. R1 detecta stale, pero el usuario tiene que mirar EXPLAIN y actuar. Auto-ANALYZE cierra el loop. | 1 push grande (~600 LOC + jobs infra) |
| ~~**M3**~~ | ~~Property tests para planner~~ — ✅ entregada 2026-06-15 ([ADR-0084](adr/0084-m3-proptest-planner.md)). Hand-rolled zero-deps, 240 comparaciones por corrida. | — |
| **M5** | Multi-column stats (correlación) | Resuelve la asunción de independencia de P5c. PostgreSQL `CREATE STATISTICS`. Reemplazaría R6 conceptualmente. | 1 push grande (~600 LOC + bump VERSION) |
| **M6** | EXPLAIN ANALYZE compara `est.match` vs `actual` | Diagnóstico directo del bias del estimator. Hoy ANALYZE re-ejecuta — solo falta agregar la columna comparativa. | 1 push (~200 LOC) |
| **M7** | Hints SQL (`/*+ INDEX(t, idx) */`) | Override per-query del cost model. Estándar en Oracle/MySQL/SQL Server. | 1 push (~400 LOC, requiere parser changes) |
| **M8** | Prefix matching sobre composite indexes (`WHERE a=X` con índice `(a,b)`) | Hoy cae a FullScan. Requiere cambio de layout on-disk a lexicographic tuple-bytes. | 1 push grande (~800 LOC + bump VERSION) |
| **M9** | Base table reorder para INNER JOIN chains (P5d extendido) | Hoy P5d solo swap el step current. Reorder global requiere refactor de `build_join_scope`. | 1 push (~600 LOC, riesgo medio) |
| **M11** | WAL-mode opt-in (ADR-0018) | Habilita lectores concurrentes. Espacio comparativo serio vs SQLite. | 1 push grande |
| **M12** | SAVEPOINT + ROLLBACK TO SAVEPOINT (T1) | Hoy ROLLBACK descarta todo el batch. | 1 push (~400 LOC) |
| **M13** | Cross-request transactions en server HTTP (T2) | Sin esto no se puede escribir un cliente que haga BEGIN/INSERT/INSERT/COMMIT. | 1 push (~500 LOC, depende de M12) |

---

## 🔵 Prioridad baja — diferido con razón

### 7. Las fases β–ζ de AGENDA_INVESTIGACION.md

Plan-as-data, schema semántico, embedded variants, time-travel, migration como conversación — todo eso sigue siendo el norte conceptual del proyecto, pero **viene después de Fase α y de las prioridades altas de este documento**. Sin benchmarks y sin cobertura SQL real, esas fases serían castillos sobre arena.

Mantener en AGENDA_INVESTIGACION.md como north star. No mover acá hasta que las prioridades altas cierren.

---

## 🏛️ Notas de proceso

- **Cómo se agrega una tarea**: si surge algo durante una intervención que no se puede cerrar en el mismo bloque, va acá antes de cerrar el commit. La idea es que este documento sea el único lugar donde "lo que falta" vive — no en TODOs comentados en el código, no en mensajes sueltos, no en la cabeza.
- **Cómo se cierra una tarea**: cuando la entrega vive en `main`, se mueve a CHANGELOG.md con la entrada formal de la intervención y se borra de acá.
- **Cómo se reordenan prioridades**: cualquier momento, sin justificar. La prioridad alta de hoy puede ser media mañana si el contexto cambia. Los disparadores típicos están en §🧭.
- **Cómo se vetan recomendaciones**: si una tarea acá deja de tener sentido (ej: "ya no voy a hacer el proyecto comparativo, gabybench pierde el motivo principal"), se mueve a una sección "💤 archivadas" con una línea de por qué. No se borra silencioso — el por-qué del veto es información útil después.
- **Cuándo el asistente debe callarse**: cuando el aporte de seguir empujando una recomendación es marginal respecto al ruido. Si una conversación entra en loop sobre la misma prioridad, se anota acá como "punto de fricción no resuelto" y se pasa a otra cosa.

---

## 🔗 Referencias

- [AGENDA_INVESTIGACION.md](AGENDA_INVESTIGACION.md) — el norte conceptual (qué clase de DB queremos entender).
- [ROADMAP.md](../ROADMAP.md) — historial de lo entregado en Fase 1 y Fase 2.
- [CHANGELOG.md](../CHANGELOG.md) — entradas formales de cada intervención cerrada.
