# 🔬 Agenda de investigación de gabysql

> **Este documento reemplaza, como fuente operativa, a [COMMERCIAL_ROADMAP.md](COMMERCIAL_ROADMAP.md) y [POSITIONING.md](POSITIONING.md).**
>
> Esos otros documentos quedan en el repo como **artefactos históricos** — el ejercicio mental de pensar el proyecto como producto comercial fue útil para entender qué partes valen la pena pulir, pero no es lo que `gabysql` es. Esto es lo que es.

---

## 1. El reframe (qué dejamos atrás)

Hasta ahora la documentación del proyecto sugería que `gabysql` apuntaba a ser un producto comercial: tres caminos A/B/C, ICPs, comparativas con SQLite/Postgres, "embedded nicho", etc. Ese marco está retirado por una razón simple: **no es verdad**.

- No hay usuarios.
- No hay validación externa de que alguien necesite esta DB específicamente.
- No hay co-maintainer, sponsor, ni cliente piloto.
- La inversión de tiempo no está orientada a vender, está orientada a entender.

Pretender lo contrario distorsiona las decisiones técnicas. Ej.: por mucho tiempo dije "no soportamos JOIN porque el Camino A no lo requiere" — suena a justificación de producto pero la verdad es que "no lo había construido todavía y estaba explorando otras cosas". La formulación honesta resultó ser más útil: en mayo 2026 los implementé (subqueries + JOINs ANSI completos, ver [CHANGELOG.md](../CHANGELOG.md)) porque me interesaba entender el nested-loop join, el problema del schema combinado con qualifiers, y la diferencia operativa entre `EXISTS` correlacionada y `IN (SELECT ...)`. El aprendizaje justificó el trabajo aún sin usuarios.

**Lo que `gabysql` realmente es:**

> Un proyecto de aprendizaje profundo sobre cómo se construye una base de datos, usando la pregunta "¿cómo se vería una DB nativa de la era de los agentes LLM?" como hilo conductor.

El objetivo no es shipear un producto. Es **aprender bases de datos a fondo y, en paralelo, explorar qué cambia cuando el consumidor principal no es un humano escribiendo SQL ni una app, sino un agente que razona sobre datos**.

---

## 2. Qué NO es esta agenda

Igual de importante que decir qué SÍ es.

- **No es un roadmap a v1.0**. No hay v1.0. Hay un motor estable como base + una serie de exploraciones encima.
- **No es un plan para conseguir usuarios**. Si alguien aparece, bien. Si no, también.
- **No es una promesa de fechas**. Las fases tienen orden conceptual, no calendario.
- **No es una lista de features de "DB normal"**. Si la única razón para agregar algo es "todas las DBs lo tienen", probablemente no entra.
- **No es una excusa para no terminar**. Cada exploración cierra con algo concreto: código que corre, un test que pasa, un documento que explica lo que se aprendió.

---

## 3. La tesis

**¿Qué hace a una DB distinta del resto en la era de los agentes LLM?**

Las DBs convencionales (SQLite, Postgres, MySQL, DuckDB) asumen un modelo de uso donde el cliente es **humano** o **aplicación**. Eso define todo:

- **SQL textual** como interfaz primaria. El humano lo escribe, la app lo emite, la DB lo parsea.
- **Schemas tipados pero semánticamente opacos**. La DB sabe que `email` es `TEXT(255)`, no sabe que es un email.
- **Plans privados**. `EXPLAIN` existe pero es algo que pedís aparte; el response normal no carga su propio plan.
- **Tiempo lineal**. El "ahora" es lo único que importa; el pasado vive en backups separados.
- **Audit como log opaco**. Si lo hay, es texto plano para humanos; no es consultable como datos.

Un agente LLM tiene un perfil de uso radicalmente distinto:

- Puede generar SQL pero entiende mejor **intención + esquema rico**.
- Necesita razonar sobre **qué representa** una columna, no sólo su tipo.
- Quiere saber **cómo se ejecutó** una query para evaluar si confiar en el resultado.
- Trabaja con **historia** (¿qué cambió desde la última conversación?, ¿qué hice yo, qué hizo el humano?).
- Le sirve si el motor sabe que ciertas columnas se **mapean a embeddings** y mantiene esa relación.

Ninguna DB existente fue diseñada con ese cliente en mente. La mayoría agrega capas encima (embedding stores, planners externos, gateways MCP) porque el motor de abajo no acomoda esos casos como ciudadanos de primera.

**`gabysql` se permite preguntar: ¿qué pasa si los agentes son ciudadanos de primera en el diseño?**

Esa pregunta es lo bastante abierta para que sea un proyecto de aprendizaje genuino: ningún libro de DBs da la respuesta, los papers de embedding stores no cubren el resto, y los gateways como MCP son parches sobre motores que no fueron diseñados así.

---

## 4. El motor como laboratorio (lo que ya hay)

A diciembre de 2026 el motor tiene una **base estable suficiente como mesa de trabajo**:

| Pieza | Estado | Comentario |
|---|---|---|
| Storage `.db` + WAL + CRC32 por página | 🟢 | ADR-0003. Recovery + crash tests. |
| B+Tree real (PK + índices secundarios) | 🟢 | ADR-0004, ADR-0005, ADR-0017. |
| SQL surface mínima pero coherente | 🟢 | CREATE/INSERT/SELECT/UPDATE/DELETE/ORDER BY/BETWEEN/LIMIT/OFFSET + índices + FKs |
| Constraints declarativas (NOT NULL, UNIQUE, DEFAULT, FK con todas las acciones, CHECK column/table-level, named) | 🟢 | VERSION 6+ inicio, completado en L1/L2/L3+residuales hasta VERSION 12 |
| File lock cross-process | 🟢 | ADR-0013 |
| Backup/restore/verify con CRC end-to-end | 🟢 | ADR-0015 |
| Observabilidad básica (`/metrics`, `-log-json`) | 🟢 | ADR-0014 |
| Error handling con códigos `[GBY-NNNN]` | 🟢 | ERROR_CODES.md (códigos 1000-4143) |
| Gateway MCP + vector search + audit log | 🟢 | ADR-0010/0011/0012 |
| 724/724 integration tests verdes + 1 ignored (Argon2id RFC vector pendiente), CI multi-OS Ubuntu/macOS/Windows | 🟢 | actualizado al 2026-05-30 |

Este conjunto es lo bastante sólido para ser **plataforma de exploración** sin estar quebrándose todo el tiempo. Las invariantes (formato versionado, single-writer, CRC, file lock) son honestas y consistentes.

**No es un producto. Es un laboratorio.**

---

## 5. Ejes de investigación

Siete ejes que profundizan la pregunta de §3. No están ordenados por prioridad ni por dificultad — están ordenados por cuán claramente entiendo cada uno hoy.

### 5.1. Schema semántico (no solo tipado)

**Idea**: una columna no solo es `TEXT NOT NULL`, también es `SEMANTIC 'email'` o `SEMANTIC 'product_title'` o `SEMANTIC 'embedded_from:body'`. El catálogo persiste esta metadata; los agentes pueden introspectarla y razonar sobre el schema sin que el humano se los explique en el prompt.

**Qué tengo claro**: cómo se persiste (una `Column` gana un campo opcional `semantic_tag: Option<String>`; bump VERSION 8; mismo patrón que cada bump anterior).

**Qué no tengo claro**: el vocabulario. ¿Es un string libre? ¿Un set fijo? ¿Inspirado en schema.org? ¿En Hugging Face dataset features? Eso necesita investigación, no diseño desde primera intuición.

**Costo**: medio. Bump de formato + nueva sintaxis SQL + introspección por gateway. ~500-800 LOC.

### 5.2. Plan-as-data en cada respuesta

**Idea**: hoy `SELECT` devuelve `{ok, columns, rows}`. Que devuelva además `plan: { strategy: "pk_lookup" | "index_scan" | "full_scan", rows_examined: N, rows_returned: M, ... }`. Cuando el cliente es un agente, esa info le permite reportar al humano "esta respuesta vino de un scan completo, podría ser inexacta si la tabla se actualizó" o "esta respuesta usó el índice idx_users_score, confianza alta".

**Qué tengo claro**: el plan ya existe internamente (`Plan::FullScan | ByPks | Range` en `sql.rs`). Sólo hay que exportarlo.

**Qué no tengo claro**: el formato exacto. ¿JSON Schema? ¿Subset que un LLM ingiere bien sin re-aprenderlo? Hay que iterar con un agente real.

**Costo**: bajo-medio. ~200-400 LOC. No requiere bump de formato.

### 5.3. Embedded variants de columnas TEXT

**Idea**: `CREATE COLUMN body_embedded AS EMBEDDING(body) USING 'sentence-transformers/all-MiniLM-L6-v2'`. El motor mantiene la relación: cuando `body` cambia, `body_embedded` se recomputa (o se marca stale). `SELECT … WHERE body_embedded SIMILAR_TO <vector>` usa la columna automáticamente.

**Qué tengo claro**: la idea conceptual. Y que `gabysql-mcp` ya hace búsqueda vectorial (ADR-0011) sobre TEXT con array JSON — esto sería integrarlo al motor como ciudadano de primera.

**Qué no tengo claro**: cómo se ejecuta el embedding. ¿El motor llama al modelo? Eso lo ata a un modelo específico. ¿Lo hace el caller y pasa el vector? Más limpio pero menos automático. ¿Hooks de pre-insert? Probablemente la respuesta correcta.

**Costo**: alto. Cambia el modelo mental del motor: ya no es puramente determinístico, depende de un componente externo (el modelo de embeddings). Requiere repensar transacciones (¿qué pasa si insert falla por timeout del modelo?). ~1000+ LOC + ADR grande.

### 5.4. Time-travel por default

**Idea**: el motor es append-only por debajo. Cada `UPDATE` no muta, agrega una versión. `DELETE` marca como eliminada. `SELECT * FROM users AS OF VERSION 142` reconstruye el estado. Los agentes pueden ver el historial sin que el humano arme nada.

**Qué tengo claro**: las DBs que hacen esto bien (Datomic, XTDB) están documentadas. El concepto general.

**Qué no tengo claro**: implementarlo en un motor existente es casi un rewrite del Pager. El B+Tree actual asume mutación in-place de las hojas. Append-only requiere otra estrategia (copy-on-write, LSM-tree, snapshots).

**Costo**: muy alto. Es de los ejes más ambiciosos. Probable que sea una **rama experimental separada** y no merge a `main` hasta que esté maduro. Aprendizaje enorme — me pone cara a cara con MVCC, GC, log-structured storage, todo lo que en una DB "normal" se da por hecho.

### 5.5. Audit trail consultable como tabla

**Idea**: el log JSONL que `gabysql-mcp` ya escribe (ADR-0012) se ingesta de vuelta a una tabla virtual `_audit` consultable con SQL. `SELECT * FROM _audit WHERE table='users' AND action='UPDATE' ORDER BY ts DESC LIMIT 10` muestra el historial reciente con el `reason` semántico de cada cambio.

**Qué tengo claro**: el log ya existe, el formato es estable, solo hay que exponerlo.

**Qué no tengo claro**: si conviene parsear el archivo JSONL al vuelo (simple pero O(N) por query) o ingestar a una tabla real (más rápido pero duplicación de datos).

**Costo**: bajo. ~200-300 LOC. Es probablemente el quick win más alto del set.

### 5.6. Schema migration como conversación

**Idea**: agente dice "agregá columna `email` derivada del regex en `name`, tipo TEXT, NOT NULL con backfill". El motor parsea la intención, propone un **plan de migración** (qué columna se agrega, cómo se backfillea, qué pasa si el regex falla en alguna fila), y el agente o el humano apruebano. Una vez aprobado, se ejecuta atómicamente.

**Qué tengo claro**: el contrato. `ALTER TABLE PROPOSE ...` devuelve un plan, `ALTER TABLE APPLY <plan_id>` lo ejecuta.

**Qué no tengo claro**: cómo se representa el plan de forma que sobreviva entre llamadas (¿se persiste? ¿es solo para la sesión?), y cómo se asegura que el plan no se vuelva stale entre `PROPOSE` y `APPLY`.

**Costo**: medio-alto. ~500-800 LOC + ADR grande sobre semántica de migración.

### 5.7. Probes de invariantes (extensión de `INTEGRITY CHECK`)

**Idea**: `INTEGRITY CHECK` ya valida CRCs + rows + índices + FKs. Extenderlo a invariantes semánticos: "para cada columna marcada `embedded_from:X`, ¿está el embedding al día con la versión de X?". "Para cada FK, ¿el target aún existe?" (ya lo hace) → ampliar a "¿ese target tiene los semantic types compatibles?".

**Qué tengo claro**: la maquinaria de `INTEGRITY CHECK` es extensible (devuelve `ResultSet` con kind/object/detail).

**Qué no tengo claro**: cuáles son los invariantes "interesantes" más allá de los obvios. Hay que dejar que aparezcan al implementar otros ejes.

**Costo**: bajo, incremental. Cada nuevo eje agrega un probe propio.

---

## 6. Fases de aprendizaje

Cada fase tiene un **objetivo cognitivo** (qué quiero entender), no un objetivo de producto. La fase termina cuando puedo **explicar** lo que aprendí + mostrar **algo concreto** corriendo.

### Fase α — Endurecer la base (antes que cualquier exploración)

**Objetivo cognitivo**: entender qué falla cuando el motor se somete a entrada adversarial. Construir el aparato de medición que las exploraciones siguientes van a necesitar.

**Entregables**:

1. **`gabybench` mínimo**: 3 workloads (insert-heavy, range scan grande, mixed UPDATE/SELECT). Reporta a stdout en JSON. CI lo corre en cada push y publica el delta.
2. **Fuzzer del parser SQL** (`cargo fuzz` sobre `parse(...)`). Objetivo: 1 hora sin panic ni `unwrap` fallido.
3. **Property tests del Pager** (`proptest`): "para cualquier secuencia de begin/insert/commit/rollback, el `.db` final pasa `INTEGRITY CHECK`".
4. **Crash tests sintéticos extendidos**: hoy hay 3; llevar a 10+ cubriendo kill -9 en cada punto crítico (WAL escrito, .db escrito a medias, header corrupto, etc.).

**Métrica de éxito**: el motor sobrevive 1 hora de fuzz + property tests + crash tests sin corrupción ni panic. Si encuentra bugs (probable), se documentan + arreglan antes de avanzar.

**Por qué primero**: las exploraciones siguientes van a meter complejidad (schema rico, plan-as-data, embeddings, time-travel). Si la base no es sólida, no voy a saber distinguir un bug nuevo de un bug viejo. **Es trabajo no glamoroso pero load-bearing**.

### Fase β — Plan-as-data + audit trail consultable

**Objetivo cognitivo**: empezar a tratar al motor como **interlocutor**, no solo como ejecutor. Una respuesta no es "acá están las filas"; es "acá están las filas + cómo las obtuve + qué pasó antes en esta tabla".

**Entregables**:

1. Toda respuesta de `SELECT` incluye un campo `plan` con strategy/rows_examined/rows_returned/index_used.
2. Tabla virtual `_audit` consultable con SQL. Lee del JSONL del gateway (o reingesta a una tabla real, se decide al implementar).
3. ADR-0019 con el formato del plan, decidido tras iterar con un agente MCP real.

**Métrica de éxito**: un agente puede pedir "explicame por qué confiar en este resultado" y, sólo con la respuesta del motor (plan + _audit), construir una respuesta coherente.

**Por qué segundo**: pequeño, autocontenido, alto valor. Es el primer eje que cambia genuinamente el contrato del motor.

### Fase γ — Schema semántico

**Objetivo cognitivo**: ¿qué pasa cuando el motor entiende el **significado** (mínimo) de una columna, no solo su tipo? ¿El planner cambia? ¿La introspección por el gateway cambia? ¿Aparecen reglas nuevas (ej: "una columna marcada `email` no puede tener whitespace")?

**Entregables**:

1. Bump VERSION 8 → 9: `Column` gana `semantic_tag: Option<String>`.
2. Sintaxis SQL: `CREATE TABLE users (id INT PRIMARY KEY, email TEXT SEMANTIC 'email' NOT NULL);`.
3. Gateway MCP expone `gabysql_describe_schema` que incluye los semantic_tags.
4. Documento de vocabulario: qué tags reconoce el motor y por qué (deliberadamente corto al principio: `email`, `url`, `embedded_from:<col>`, `raw_text`, `id_external`).

**Métrica de éxito**: un agente puede listar tablas y, sin prompt extra, decir "esta tabla parece ser un catálogo de usuarios; tiene un campo de email y un id externo". Sin que el humano se lo cuente.

### Fase δ — Embedded variants de columnas TEXT

**Objetivo cognitivo**: ¿qué se rompe cuando una columna del motor depende de un componente externo (modelo de embeddings)? ¿Cómo cambian las transacciones, las migraciones, la semántica de UPDATE?

**Entregables**:

1. Sintaxis: `CREATE COLUMN body_emb AS EMBEDDING(body) USING 'modelo_id'`.
2. Hooks pre-insert/post-update que llaman al gateway MCP (o a un endpoint externo configurado) para computar el embedding.
3. `SELECT … ORDER BY EMBEDDING_SIMILARITY(body_emb, <vector>) LIMIT 5` como sintaxis de primer nivel.
4. Probe en `INTEGRITY CHECK`: detecta embeddings stale (texto cambió, embedding no recomputado).
5. ADR grande sobre cómo manejar el fallo del modelo de embeddings dentro de una transacción.

**Métrica de éxito**: `gabysql` deja de ser una DB clásica con un store vectorial al lado. Pasa a ser una DB donde búsqueda semántica es una operación SQL natural. **Esto es el corazón de la tesis** — si esta fase funciona, el proyecto tiene una razón de existir genuina.

### Fase ε — Schema migration como conversación

**Objetivo cognitivo**: ¿se puede tratar la migración de schema como un objeto de primer orden — un plan persistible que un agente propone, un humano revisa, y el motor ejecuta atómicamente?

**Entregables**:

1. `ALTER TABLE … PROPOSE` devuelve un plan JSON.
2. `ALTER TABLE … APPLY <plan_id>` lo ejecuta.
3. Plans persistidos en una tabla `_migrations` (también consultable).

**Métrica de éxito**: dos agentes pueden colaborar en una migración: uno propone, otro revisa el plan, el primero aplica. El humano puede entrar en cualquier punto.

### Fase ζ — Time-travel (rama experimental)

**Objetivo cognitivo**: aprender cómo funcionan los motores append-only / MVCC desde adentro, implementándolo. Esta fase **no merge a `main`** hasta que esté maduro — vive como rama `experiment/time-travel`.

**Entregables**: sin compromiso. La rama se considera exitosa si **enseña** suficiente sobre el problema como para poder explicar bien por qué Datomic, XTDB, Postgres-con-MVCC tomaron las decisiones que tomaron. Puede o no llegar a algo merge-able.

**Por qué última**: es la más ambiciosa. Tiene sentido hacerla cuando las otras fases hayan endurecido la disciplina del proyecto.

---

## 7. Anti-agenda (lo que NO entra)

Lista explícita para no perder foco:

- ~~**`JOIN`, subconsultas.**~~ — entregados en mayo 2026 porque el ejercicio de implementarlos enseñaba algo concreto (modelo de ejecución nested-loop, outer-stack para correlated, derivar predicates de USING/NATURAL, index-loop para optimizar). Ver [CHANGELOG.md](../CHANGELOG.md) y [docs/SQL_REFERENCE.md](SQL_REFERENCE.md).
- **`GROUP BY`, vistas, triggers, CTE, window functions.** Son features de "DB normal" que replican lo que ya existe. Si aparece una pregunta de investigación que las requiera (ej. "¿cómo se le explica un plan de window function a un agente?"), entran; mientras tanto, no.
- **Replicación, HA, clustering, sharding.** Fuera de scope. La pregunta central no es "cómo se distribuye una DB"; es "cómo se diseña una DB para agentes". Distribución es un problema ortogonal y enorme.
- **Optimizer cost-based.** Sin workload real ni benchmarks maduros (fase α apenas arranca eso), un cost-based optimizer es teología. El planner actual (deterministic dispatch) es suficiente hasta que algo lo demande con datos.
- **TLS nativo, multi-tenant, autenticación fuerte.** Si el proyecto algún día se expone a producción, esto entra. Hoy no.
- **Soporte de tipos exóticos** (DECIMAL, BIGINT separado, GEOMETRY, etc.). Los 7 tipos actuales son suficientes para todas las exploraciones de §6.
- **GUI propietaria, dashboard integrado, marketing site.** `phpgabyadmin` + `gabymodeler` ya cubren la necesidad de inspección manual; no necesitan crecer más.
- **Buscar usuarios externos / pitch decks / comparativas con otras DBs.** Los documentos comerciales históricos cubrieron esa exploración mental. No vuelven a actualizarse.

Si una idea no aparece arriba y no aparece en §5–§6, probablemente no entra. Pero la lista no es definitiva; si en el curso de explorar Fase β aparece una pregunta que merece su propia fase, se agrega acá.

---

## 8. Ritmo y métricas honestas

**Ritmo**:
- **Una intervención por semana, no por día.** Las 9 intervenciones de hoy fueron un sprint puntual; no son el modo de operar normal. Un motor de DB debe envejecer entre cambios, no estar siempre en lava reciente.
- **Cada bloque cierra con un cierre escrito**: una entrada en `CHANGELOG.md` que dice **qué aprendí**, no solo qué se cambió. Si no puedo articular el aprendizaje en 3 líneas, la fase no está cerrada.

**Métricas de éxito (no de producto)**:

| Métrica tradicional ❌ | Métrica honesta ✅ |
|---|---|
| MAUs / DAUs | "Puedo explicar conceptualmente X" |
| Customers | "El motor sobrevive Y bajo fuzz" |
| Revenue | "Aprendí algo no-obvio que no estaba en libros" |
| Feature velocity | "El cierre de cada fase tiene un párrafo de aprendizaje real" |
| GitHub stars | (Indiferente.) |

**Definición de "fase cerrada"**:

1. El código compila, los tests pasan, CI verde.
2. Hay al menos un test nuevo que captura la propiedad introducida.
3. Hay una ADR (si hubo una decisión arquitectural) y/o una entrada de CHANGELOG con el aprendizaje.
4. Si la fase introdujo conceptos nuevos, hay un párrafo en este documento explicándolos (o un link a su ADR).

---

## 9. Cómo se relacionan los otros documentos

Esta agenda **reemplaza, como fuente operativa**, a los siguientes documentos. No los borro: quedan como artefactos históricos para entender de dónde venía el proyecto.

| Documento | Estado tras este reframe |
|---|---|
| [COMMERCIAL_ROADMAP.md](COMMERCIAL_ROADMAP.md) | **🏛️ Histórico**. Ejercicio mental, no agenda. |
| [POSITIONING.md](POSITIONING.md) | **🏛️ Histórico**. ICPs, casos de uso comerciales — no es lo que el proyecto es. |
| [COMPETITIVE_ANALYSIS.md](COMPETITIVE_ANALYSIS.md) | **🏛️ Histórico**. Útil como mapa del mercado, no como ranking donde gabysql aspira. |
| [ADR-0007](adr/0007-commercial-path-a.md) ("Camino A") | **🗑️ Superseded** por este documento. |
| [ROADMAP.md](../ROADMAP.md) | **Vigente, reorientado**. Apunta acá como fuente operativa; sigue manteniendo el historial de qué se entregó. |
| [STATUS.md](STATUS.md) | **Vigente**. Describe lo que el motor **es**, no qué se vende. |
| [USE_CASES.md](USE_CASES.md) | **Vigente**. Recetas técnicas, no pitch comercial. |
| [ADRs 0001–0006, 0008–0018](adr/) | **Vigentes**. Decisiones técnicas, independientes del marco comercial. |
| [ERROR_HANDLING.md](ERROR_HANDLING.md) + [ERROR_CODES.md](ERROR_CODES.md) | **Vigentes**. |
| [ARCHITECTURE.md](ARCHITECTURE.md) + [TECHNICAL_SPECS.md](TECHNICAL_SPECS.md) + [SQL_REFERENCE.md](SQL_REFERENCE.md) | **Vigentes**. |

---

## 10. Para futuro-yo (o quien lea esto)

Si en tres meses estás dudando de si seguir el proyecto, leé otra vez §1 y §3. No necesita usuarios. No necesita validar nada. **Su única razón de existir es que vos entiendas profundamente cómo se construye una DB y cómo se vería una pensada para agentes**.

Si un día aparece alguien interesado en usar `gabysql` para algo real, ese día este documento cambia. Mientras tanto, no.

Si alguna fase deja de ser interesante, **pará esa fase**. No la termines por terminarla. El costo de un proyecto de aprendizaje no es ingeniería desperdiciada — es energía gastada en algo que dejó de enseñar. Mejor saltar a otra fase, o tomarse un break.

Si al cabo de seis meses ninguna fase de §6 avanzó pero aprendiste lo que querías sobre DBs por otro camino (papers, otro proyecto, un trabajo distinto), también es una salida válida. `gabysql` no te debe nada.

---

## 🔗 Referencias

- [README.md](../README.md) — entrada al proyecto.
- [STATUS.md](STATUS.md) — qué hay hoy.
- [adr/](adr/) — historial de decisiones técnicas, fuente de verdad de lo construido.
- [CHANGELOG.md](../CHANGELOG.md) — historial cronológico, donde cada fase futura registrará su cierre con aprendizaje.
