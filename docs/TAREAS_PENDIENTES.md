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

## 🔥 Prioridad alta — recomendaciones técnicas para el proyecto comparativo

> Vienen del análisis honesto del estado actual + del plan de usar `gabysql` al lado de SQLite/DuckDB/etc. Son las cosas que, **si las hacés cuando trabajes en este proyecto**, mueven más el dial. Si en el ínterin cambia el contexto (ver §🧭) y otra cosa pasa al frente, se reordenan sin drama.

### 1. Construir `gabybench` mínimo

**Qué**: 3 workloads (insert-heavy, range-scan, mixed UPDATE/SELECT) corriendo en CI, reportando números en JSON + tabla humana. Línea base honesta de performance del motor.

**Por qué importa**: sin esto cada decisión de performance es opinión. ADR-0016 (prefetch LeafCursor), ADR-0009 (PageCache LRU), ADR-0017 (índice INT-ordenado) están como "directional sin medir" porque no hay con qué medirlos. Y el proyecto comparativo necesita números reproducibles para arrancar.

**Diseño ya planteado**: zero-deps (sin `criterion`), `src/bin/gabybench.rs`, ~300 LOC, output dual texto+JSON, integración CI con artifacts. Detalle completo en la conversación que generó este archivo (resumen: xorshift64 + percentile a mano + 3 workloads cerrados + 1 warmup + 5 measured runs + reporte de mediana).

**Esfuerzo**: 1 intervención, 4–8 horas. Es la siguiente cosa razonable a hacer.

**Cuándo se considera cerrado**: 
- Binario corre local con `cargo run --release --bin gabybench -- --workload all`.
- CI lo corre en cada push con `--rows 2000 --runs 3`, sube `bench.json` como artifact.
- README tiene sección "Performance baseline" con números del último corte.

---

### 2. Cobertura SQL para comparaciones realistas — ✅ entregada

**Estado al 2026-05-26**: cerrada. JOINs ANSI completos (INNER/LEFT/RIGHT/FULL/CROSS/USING/NATURAL + index-loop optimization) entregados antes de la sesión 2026-05-25; `WHERE` completo (E1+E2+E3), agregados single-table (`COUNT`/`SUM`/`AVG`/`MIN`/`MAX` + `GROUP BY`/`HAVING`/`DISTINCT` — bloque F), DML masivo (J), UPSERT/RETURNING (J2), funciones escalares + aritméticos en cualquier cláusula (G1+G2+G3), subqueries restantes (H), set ops + VALUES (I), DDL extendido (K1+K2 con PK compuesta + índices compuestos all-INT) en sesiones 2026-05-25 y 2026-05-26. La superficie SQL operacional clásica está completa.

**Pendiente residual** (al 2026-05-30): `UPDATE ... FROM`, `EXCLUDED.col` en UPSERT. Lo que estaba pendiente y cerró 2026-05-30: agregados sobre `SELECT con JOIN` (F2, ADR-0066 Gap 1+7 — `COUNT(DISTINCT col)` sobre JOIN sigue residual), CTE recursivas con anchor bare-SELECT (E5/W2). Window functions (W3+W4) y CTE no-rec (W1) ya estaban cerrados antes. Ninguno bloquea el proyecto comparativo.

---

### 3. Fuzz testing + property tests

**Qué**: 
- `cargo fuzz` sobre `parse(...)` — 1 hora mínima sin panic ni `unwrap` fallido.
- `proptest` sobre el Pager: "para cualquier secuencia válida de begin/insert/commit/rollback, el `.db` final pasa `INTEGRITY CHECK`".
- Extender los 3 crash tests sintéticos actuales a 10+ escenarios (kill -9 en cada punto crítico).

**Por qué importa**: SQLite tiene millones de horas de fuzz acumuladas. No tenemos que igualar eso, pero **una hora limpia de fuzz** es una línea creíble que se puede mencionar en el README; cero horas no lo es. Y para un comparativo serio, una DB que se rompe bajo entrada adversarial es vergonzosa.

**Costo**: medio. `cargo fuzz` requiere config inicial + corpus + tiempo de ejecución. `proptest` se integra a `cargo test` con un crate (esto rompe ADR-0001 si lo metemos al motor — pero como `dev-dependency` está OK, sólo el binario final mantiene cero deps).

**Esfuerzo**: 1–2 intervenciones (parser fuzz + property test del Pager pueden ir juntos; crash tests adicionales aparte).

---

## 🟡 Prioridad media — cosas que mueven la aguja del proyecto

### 4. Re-evaluar el modelo single-writer

**Qué**: hoy el motor es estrictamente single-writer (Mutex global en server.rs + file lock cross-process). Eso es defendible mientras el proyecto sea de aprendizaje, pero **el proyecto comparativo va a sufrir** porque cualquier benchmark concurrente da números peores que SQLite (que soporta lectores concurrentes vía WAL-mode).

**Opciones**:
- **A**: aceptar la limitación y elegir benchmarks single-writer. Honesto pero limita la comparativa.
- **B**: implementar lectores concurrentes con snapshot isolation simple. Requiere repensar el Pager. Es esencialmente parte del WAL-mode de ADR-0018 (Propuesta).
- **C**: implementar MVCC. Demasiado grande para este momento del proyecto.

**Por qué medio y no alto**: porque depende del scope que tome el proyecto comparativo. Si vas a comparar OLTP serio, esto es alto. Si vas a comparar features distintivas (vector search, audit, schema rico), esto importa menos.

**Esfuerzo**: opción A: 0 (decisión). Opción B: 1 ADR + ~600 LOC + tests. Opción C: rewrite parcial.

---

### 5. Demo pública de Fase 5 (MCP gateway + vector search + audit)

**Qué**: un repo o gist con un agente Claude/Cursor/etc. usando `gabysql-mcp` para hacer algo concreto — RAG sobre PDFs, audit log consultable, query asistida — y un README que muestre que funciona.

**Por qué importa**: para "llamar la atención" eventualmente (la razón por la que el repo es público), la diferenciación real está en el MCP gateway. No en el motor. El motor es decente; el gateway con vector search integrada y audit log con `reason` semántico es lo que no tiene ninguna otra DB del nicho. Pero hoy nadie ve eso porque no hay una demo simple.

**Esfuerzo**: 1 intervención + setup de un agente real para probarlo. Probablemente un repo separado en GitHub que linkee de vuelta.

---

## 🔵 Prioridad baja — diferido con razón

### 6. Las fases β–ζ de AGENDA_INVESTIGACION.md

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
