# 📌 Tareas pendientes

> **Este es el primer documento que se consulta cuando se pide "estado del proyecto"**. Antes que CHANGELOG, antes que ROADMAP. Lo que está acá es lo próximo a hacer, ordenado por prioridad real (no aspiracional).
>
> Cuando una tarea se cierra, se mueve a CHANGELOG con la entrada formal. No se borra de acá hasta entonces.

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

**Lo que falta de esta línea**: 
- **R2**: calibrar `INDEX_BREAKEVEN_SELECTIVITY = 0.2` (constante de P5c) contra datos reales del smoke bench. Hoy es heurística teórica (`C_RANDOM ≈ 5 × C_SEQ`). 
- **R3**: calibrar `swap_threshold = 2×` de P5d (hash-join build-side) — mismo método.
- **Comparador entre runs**: el CI sube artifacts pero no hay diff automático contra baseline. Sin esto, una regresión de 50% en una query no falla el push.

---

### 2. Cobertura SQL para comparaciones realistas — ✅ entregada

**Estado al 2026-05-26**: cerrada. JOINs ANSI completos (INNER/LEFT/RIGHT/FULL/CROSS/USING/NATURAL + index-loop optimization) entregados antes de la sesión 2026-05-25; `WHERE` completo (E1+E2+E3), agregados single-table, DML masivo (J), UPSERT/RETURNING (J2), funciones escalares + aritméticos (G1+G2+G3), subqueries (H), set ops + VALUES (I), DDL extendido (K1+K2) en sesiones 2026-05-25 y 2026-05-26. La superficie SQL operacional clásica está completa.

**Pendiente residual** (al 2026-06-11):
- `UPDATE ... FROM` — no implementado.
- `EXCLUDED.col` en UPSERT — no implementado.
- `COUNT(DISTINCT col)` sobre JOIN — F2 cerró el caso general (2026-05-30) pero esta variante sigue rebotando con `[GBY-4028]`.
- **`ORDER BY a, b` (multi-col)** — descubierto en R8 (2026-06-11): el parser solo acepta single-col.
- Ninguno bloquea el proyecto comparativo.

---

### 3. Fuzz testing + property tests — **alta, sigue abierta**

**Qué**:
- `cargo fuzz` sobre `parse(...)` — 1 hora mínima sin panic ni `unwrap` fallido.
- `proptest` sobre el Pager: "para cualquier secuencia válida de begin/insert/commit/rollback, el `.db` final pasa `INTEGRITY CHECK`".
- **`proptest` sobre planner** (NUEVO, post P5c/P5d/R6): "para el mismo dato y WHERE, los resultados con `ANALYZE` corrido y sin correr son idénticos" — defiende correctness de las decisiones cost-based.
- Extender los 3 crash tests sintéticos actuales a 10+ escenarios.

**Por qué importa**: SQLite tiene millones de horas de fuzz acumuladas. **Una hora limpia de fuzz** es una línea creíble en el README; cero horas no lo es. Y con el optimizer P5c/P5d/R6 que cambia planes según stats, las property tests son la red de seguridad real contra regresiones de correctness silenciosas.

**Costo**: medio. `cargo fuzz` requiere config inicial + corpus + tiempo de ejecución. `proptest` se integra como `dev-dependency` sin tocar ADR-0001.

**Esfuerzo**: 1–2 intervenciones.

---

### 4. Reparaciones del análisis post-P5 — abiertas

**De [ANALISIS_POST_P5.md](ANALISIS_POST_P5.md), las que quedan abiertas tras la sesión 2026-06-11**:

| # | Item | Esfuerzo |
|---|------|----------|
| **R2** | Calibrar `INDEX_BREAKEVEN` con bench data | 1 push (~50 LOC + análisis manual) |
| **R3** | Calibrar threshold P5d | 1 push (~50 LOC + análisis) |
| **R5** | TRUNCATE TABLE limpia stats — **bloqueado por implementar TRUNCATE como statement** | parte de bloque mayor |
| ~~**R7**~~ | ~~Mensaje EXPLAIN P5c sugiere re-ANALYZE~~ — ✅ entregada 2026-06-15 ([ADR-0078](adr/0078-r7-p5c-reanalyze-hint.md)) | — |
| **R9** | `COUNT(DISTINCT col)` sobre JOIN | 1 push (~150 LOC) |
| **R10** | USING/NATURAL JOIN — EXPLAIN heurística completa | 1 push (~200 LOC) |

**Tensiones abiertas (no necesariamente reparables sin trabajo nuevo)**:
- ~~**2.5**~~ — Mensaje EXPLAIN P5c ambiguo — ✅ cerrada por R7 (2026-06-15).
- **2.6** — RIGHT/FULL JOIN heurística imprecisa en EXPLAIN (documentado).
- **2.9** — P5d swap puede cambiar orden de filas sin ORDER BY (deuda documentada).

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
| **M3** | Property tests para planner (`proptest` sobre P5c/P5d/R6) | Ya mencionado arriba en tarea 3. Crítico antes de cualquier P5f+. | 1 push (~300 LOC) |
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
