# 📌 Tareas pendientes

> **Este es el primer documento que se consulta cuando se pide "estado del proyecto"**. Antes que CHANGELOG, antes que ROADMAP. Lo que está acá es lo próximo a hacer, ordenado por prioridad real (no aspiracional).
>
> Cuando una tarea se cierra, se mueve a CHANGELOG con la entrada formal. No se borra de acá hasta entonces.

---

## 🔥 Prioridad alta — prerequisitos para que el motor sirva al proyecto comparativo

> El proyecto comparativo (separado, que va a usar `gabysql` al lado de SQLite/DuckDB/etc.) requiere que el motor sea más robusto que hoy. Estas tareas son **prerequisito** para que la comparación no sea vergonzosa.

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

### 2. Cobertura SQL para comparaciones realistas

**Qué**: agregar `JOIN` (INNER al menos), `WHERE` sobre columnas no-PK no-indexadas (full scan filtrado), y `COUNT(*)` / `SUM` / `AVG` mínimos.

**Por qué importa**: cualquier workload comparativo realista usa varias tablas. Sin `JOIN`, la mitad de las queries de TPC-H, YCSB o cualquier benchmark estándar no corren. La comparativa con SQLite/DuckDB sería sobre el subset trivial donde `gabysql` opera; deshonesto.

**Costo**: mediano-alto. `JOIN` toca planner + executor + probablemente requiere reordenar `Plan` enum. Agregados (`COUNT`, `SUM`, `AVG`) son más chicos.

**Tensión**: contradice parte de la "anti-agenda" del AGENDA_INVESTIGACION.md ("no JOIN porque no aprendo nada nuevo"). La resolución: bajo el marco "aprendizaje puro" eso era cierto; bajo el marco "el proyecto comparativo necesita esto", deja de serlo. La agenda debe actualizarse explicitando el cambio cuando esto se haga.

**Esfuerzo**: 2–3 intervenciones por separado (JOIN una, agregados otra, planner cleanup otra).

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

- **Cómo se agrega una tarea**: si surge algo durante una intervención que no se puede cerrar en el mismo bloque, va acá antes de cerrar el commit. La idea es que este documento sea el único lugar donde "lo que falta" vive — no en TODOs comentados en el código, no en mensajes de WhatsApp, no en la cabeza.
- **Cómo se cierra una tarea**: cuando la entrega vive en `main`, se mueve a CHANGELOG.md con la entrada formal de la intervención y se borra de acá.
- **Cómo se reordenan prioridades**: cualquier momento. La prioridad alta de hoy puede ser media mañana si el contexto cambia (ej: si el proyecto comparativo se pospone, gabybench baja de prioridad real aunque siga siendo lo más útil técnicamente).

---

## 🔗 Referencias

- [AGENDA_INVESTIGACION.md](AGENDA_INVESTIGACION.md) — el norte conceptual (qué clase de DB queremos entender).
- [ROADMAP.md](../ROADMAP.md) — historial de lo entregado en Fase 1 y Fase 2.
- [CHANGELOG.md](../CHANGELOG.md) — entradas formales de cada intervención cerrada.
