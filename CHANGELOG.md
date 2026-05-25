# 📝 Changelog

> **Historial de cambios relevantes aplicados al producto y a su base documental.**

---

## 2026-05-25 — Bloque E2: comparadores, `LIKE`, `IS NULL`, `IN literal`

> **Un push a `main`** que cierra el set de operadores básicos del `WHERE`.

### 🆕 Nuevos operadores
- `<`, `<=`, `>`, `>=`, `<>`, `!=` sobre INT / FLOAT / TEXT (lex) / BOOL. NULL en cualquiera de los dos lados → `NULL` (3VL). Tipos incompatibles → `false` (no abortamos la query).
- `[NOT] LIKE 'patron'` sobre TEXT. Wildcards SQL estándar (`%` = cero o más, `_` = exactamente uno) con escape `\%` / `\_`. Backtracking O(|s|·|p|), suficiente para patrones realistas.
- `IS [NOT] NULL` — único predicado que NO propaga NULL (es la forma explícita de testear ausencia).
- `[NOT] IN (lit1, lit2, ...)` con lista literal. Semántica ANSI: si la columna es NULL → NULL; si no hay match y la lista contiene NULL → NULL (especialmente sensible en `NOT IN`).

### 🧬 Tokenizer
- Nuevos símbolos: `<`, `<=`, `>`, `>=`, `<>`, `!=` (con lookahead de 1 char). `!` suelto sigue siendo error (sugerencia explícita en el mensaje).

### 🧠 AST
- `WhereClause` extendido con cuatro variants nuevos: `Compare { op: CompareOp, ... }`, `Like { pattern, negated }`, `IsNull { negated }`, `InList { values, negated }`. Ningún variant tiene fast-path indexada por ahora — todos van por `generic_post_filter` + evaluador 3VL.

### 🚦 Executor
- `generic_post_filter` ahora se activa también cuando el átomo único es E2 (Compare/Like/IsNull/InList). El path por PK/índice queda intacto para `=`, `BETWEEN`, `IN (SELECT)`, `= (SELECT)`, EXISTS y EqColumnRef.
- Tres helpers puros: `eval_compare`, `eval_like`, `eval_in_list`. `like_match` es backtracking recursivo con soporte de escape.

### ⚠️ Limitación residual
- `NOT IN (SELECT ...)` (subquery) explícitamente rechazado por ahora — el desugar a `NOT (col IN (SELECT))` cambia la semántica con NULLs y queda para el bloque H. `NOT IN (lista literal)` sí está.
- `<` / `>` / `<=` / `>=` no aprovechan el índice OrderedInt todavía (range scan optimization queda en backlog; correctitud antes que velocidad).

### 🧪 Validación
- 11 integration tests nuevos en `tests/integration_test.rs` (`e2_*`): comparadores INT, `<>`/`!=` sinónimos, comparación TEXT lex, LIKE básico, NOT LIKE, IS NULL / IS NOT NULL, IN literal, NOT IN con 3VL, combinaciones con AND/OR de E1, LIKE con escape, comparador con JOIN.
- `cargo check --lib --tests` limpio.

### 📚 Documentación
- `docs/SQL_REFERENCE.md` — EBNF del WHERE actualizado, ejemplos de cada operador nuevo, fila E2 en la tabla de soporte.
- `docs/MISSING_COMMANDS.md` — E2 marcado cerrado, hueco #2 del top-5 tachado, comparadores/LIKE/IS NULL/IN literal en ✅.

---

## 2026-05-25 — Bloque E1: `AND` / `OR` / `NOT` + paréntesis en `WHERE`

> **Un push a `main`** que destraba el filtro compuesto en cualquier `SELECT`.

### 🔀 WHERE booleano (bloque E1)
- AST: `WhereClause` (plano) → `WhereExpr = And | Or | Not | Atom(WhereClause)`. Los átomos siguen siendo los seis predicados pre-existentes (`Eq`, `Between`, `In`, `EqSubquery`, `EqColumnRef`, `Exists`) — el bloque no toca su semántica.
- Parser: precedencia estándar SQL `OR` < `AND` < `NOT` < paréntesis / átomo. `NOT EXISTS` mantiene la forma vieja (`Atom(Exists{negated:true})`) para preservar el fast-path correlacionado.
- Executor: cuando el WHERE se reduce a un único átomo, se usan las fast-paths existentes (PK directo, índice secundario, range scan, EXISTS correlacionado post-filter). Cuando hay combinadores se cae a FullScan + evaluador trivaluado (3VL) row-a-row — `defer_window` se activa para que `LIMIT`/`OFFSET` se apliquen DESPUÉS del filtro.
- 3VL para `NULL`: `NULL AND false = false`, `NULL AND true = NULL`, `NULL OR true = true`, `NOT NULL = NULL`. Solo `Some(true)` mantiene la fila.
- Soporte completo en `SELECT` con o sin JOINs. `filter_joined_rows` ahora recibe `&WhereExpr` y aplica el mismo evaluador 3VL sobre filas joined.

### ⚠️ Limitación residual
- `EXISTS` correlacionado y `col = otra.col` (column-ref del outer) **solo se permiten como único átomo del WHERE**. Combinarlos con `AND`/`OR`/`NOT` devuelve `[GBY-4024]`. La generalización queda explícitamente fuera de E1.

### 🧰 Código de error nuevo
- `4024` `WHERE_COMBINATOR_CORRELATED_UNSUPPORTED`

### 🧪 Validación
- 11 integration tests nuevos en `tests/integration_test.rs` (sufijo `e1_*`): AND, OR, NOT, paréntesis, precedencia, BETWEEN + AND combinador, 3VL sobre NULL, NOT anidado, combinador con LIMIT+ORDER, doble NOT, combinador con JOIN, error sintáctico.
- `cargo check --lib --tests` limpio (0 warnings).

### 📚 Documentación
- `docs/SQL_REFERENCE.md` — EBNF del WHERE reescrita con precedencia + 3VL + ejemplos.
- `docs/MISSING_COMMANDS.md` — E1 marcado como cerrado; top-5 actualizado.
- `docs/ERROR_CODES.md` — entry `4024`.

---

## 2026-05-24 — Subqueries completas + roadmap de JOINs cerrado

> **Siete pushes consecutivos a `main`** que cierran dos features grandes del motor SQL.

### 🧩 Subqueries (3 bloques)
- `WHERE col IN (SELECT …)` — no-correlacionada, single-column. Reusa lookup PK/índice.
- `WHERE col = (SELECT …)` — subquery escalar (1 × ≤1). 0 filas o NULL → match vacío (ANSI). >1 fila → `[GBY-4014]`.
- `WHERE [NOT] EXISTS (SELECT …)` — no-correlacionada (pre-ejecuta) y correlacionada single-eq (`inner_col = outer.col`, post-filter per-row con `outer_stack`).

### 🔗 JOINs (4 bloques)
- **A** — `INNER JOIN`, `CROSS JOIN`, comma-syntax (`FROM a, b`), aliases con `[AS]`, multi-tabla en chain (left-deep), self-join. Columnas cualificadas (`tabla.col` o `alias.col`). `SELECT *` expande prefijado.
- **B** — `LEFT [OUTER] JOIN`, `RIGHT [OUTER] JOIN`, `FULL [OUTER] JOIN` con NULL-fill por kind. `OUTER` opcional (ANSI).
- **C** — `JOIN ... USING (col)` (sugar para `ON l.col = r.col`) y `NATURAL JOIN` (auto-derive del USING). `SELECT *` omite la columna fusionada del right.
- **D** — Index-loop join optimization transparente: cuando el `ON` (o el USING/NATURAL derivado) apunta contra PK o columna indexada del right Y el kind es INNER/LEFT, el engine reemplaza el FullScan del right por lookup dirigido. O(N×M) → O(N×log M) por JOIN.

### 🧰 Códigos de error nuevos
- `4011` `SUBQUERY_MUST_RETURN_ONE_COLUMN`
- `4012` `IN_PK_TYPE_MISMATCH`
- `4013` `IN_REQUIRES_PK_OR_INDEX`
- `4014` `SCALAR_SUBQUERY_TOO_MANY_ROWS`
- `4015` `EXISTS_REQUIRES_SUBQUERY`
- `4016` `OUTER_COLUMN_REF_INVALID`
- `4017` `TABLE_ALIAS_DUPLICATED`
- `4018` `COLUMN_AMBIGUOUS`
- `4019` `COLUMN_QUALIFIER_NOT_FOUND`
- `4020` `JOIN_PREDICATE_REQUIRED`
- `4021` `CROSS_JOIN_WITH_ON`
- `4022` `USING_COLUMN_INVALID`
- `4023` `NATURAL_JOIN_NO_COMMON_COLUMN`

### 📚 Documentación
- Doc barrido completo: `README.md`, `docs/SQL_REFERENCE.md`, `docs/STATUS.md`, `docs/ERROR_CODES.md`, `TROUBLESHOOTING.md`, `RUNBOOK.md`, `docs/POSITIONING.md`, `docs/COMPETITIVE_ANALYSIS.md`, `docs/ARCHITECTURE.md`, `docs/API.md`, `docs/TECHNICAL_SPECS.md`, `RECRUITER.md`, `ROADMAP.md`, `web/phpgabyadmin/index.php`.

### 🧪 Validación
- **71/71 tests** integración verdes (16 nuevos entre subqueries y JOINs).
- `cargo fmt --check` ✅ · `cargo clippy --all-targets -- -D warnings` ✅.

---

## 2026-05-18 — Vigesimoséptima intervención: reframe — `gabysql` es un proyecto de aprendizaje, no comercial

> **Solo docs. Cero código.** Reescribe el marco operativo del proyecto.

### ✨ Cambio
- Nuevo documento **[docs/AGENDA_INVESTIGACION.md](docs/AGENDA_INVESTIGACION.md)** (~500 líneas, 10 secciones) que reemplaza como fuente operativa a `COMMERCIAL_ROADMAP.md`/`POSITIONING.md`/`COMPETITIVE_ANALYSIS.md`. Contiene:
  - El reframe explícito: el proyecto **no es comercial y no apunta a serlo**.
  - La tesis: "¿cómo se vería una DB nativa de la era de los agentes LLM?".
  - 7 ejes de investigación con honestidad sobre qué entiendo / qué no / qué cuesta:
    1. Schema semántico (no solo tipado)
    2. Plan-as-data en cada respuesta
    3. Embedded variants de columnas TEXT
    4. Time-travel por default
    5. Audit trail consultable como tabla
    6. Schema migration como conversación
    7. Probes de invariantes
  - 6 Fases de aprendizaje (α–ζ) con **objetivo cognitivo** ("qué quiero entender"), no objetivo de producto.
  - Anti-agenda explícita: lo que NO entra (JOIN/GROUP BY/replicación/optimizer cost-based/etc.).
  - Ritmo realista (1 intervención/semana, no 9/día) y métricas de éxito honestas ("puedo explicar X" en vez de "MAUs").
- **Marcados como históricos** (banner explícito al inicio):
  - `docs/COMMERCIAL_ROADMAP.md`
  - `docs/POSITIONING.md`
  - `docs/COMPETITIVE_ANALYSIS.md`
- **ADR-0007** (Camino A) marcada como `🗑️ Superseded por AGENDA_INVESTIGACION.md`. El índice de ADRs refleja el cambio.
- **README.md** reescribe la introducción y la tabla de documentos clave: el proyecto se presenta como lo que es (laboratorio de aprendizaje sobre DBs + agentes), no como producto.
- **ROADMAP.md** redirige a la nueva agenda como fuente operativa y mantiene su rol histórico (qué entregó cada Fase 1/2).

### 🎯 Por qué este cambio
Auditoría con el usuario del estado del proyecto:
> *"además no se saca nada con pensar que alguien le interese, si creo todavia esta en pañales, lo realmente es mi objetivo, crea una base de datos que no sea como las demás, mientras evoluciona la IA, el producto puede evolucionar de forma natural con lo que es una base de datos y las nuevas tecnologias"*

El marco anterior (caminos A/B/C, ICPs, comparativas comerciales) distorsionaba las decisiones técnicas: justificaba o vetaba features con argumentos comerciales que en realidad no aplicaban (no hay clientes ni hay intención de tenerlos). El reframe permite decir las cosas como son y elegir exploraciones por **valor de aprendizaje + diferenciación honesta**, no por encaje a un ICP imaginario.

### 🛡️ Lo que NO cambia
- Cero código tocado. Motor estable como estaba.
- ADRs técnicos (0001–0006, 0008–0018) siguen vigentes. Son decisiones del motor, independientes del marco comercial.
- `STATUS.md`, `USE_CASES.md`, `SQL_REFERENCE.md`, `ARCHITECTURE.md`, `TECHNICAL_SPECS.md`, `ERROR_HANDLING.md`, `ERROR_CODES.md` siguen vigentes — describen lo que el motor **es**, no qué se vende.
- 45/45 integration + 27 lib + 7 unit tests verdes. CI sin alterar.

---

## 2026-05-18 — Vigesimosexta intervención: códigos numéricos `[GBY-NNNN]` estilo MySQL `ER_*` + catálogo operacional

> **Sin bump de formato. Sin deps añadidas.** Cierre del trabajo de manejo de errores: cada error user-facing ahora lleva un código estable y existe un catálogo operacional búscable. Análogo al sistema `ER_DUP_ENTRY=1062` de MySQL.

### ✨ Cambio
- Nuevo módulo [src/errors.rs](src/errors.rs):
  - `pub mod codes` con ~30 constantes `pub const NAME: u32 = NNNN` agrupadas por rango:
    - `1000–1999` storage / WAL / file lock
    - `2000–2999` catalog / schema / identificadores
    - `3000–3999` constraints (PK, NOT NULL, UNIQUE, FK)
    - `4000–4999` superficie SQL (parser, planner, limitaciones)
    - `5000–5999` server / HTTP / auth
  - Helper `coded(code: u32, message: impl Into<String>) -> DbError` que produce mensajes con prefijo `[GBY-NNNN]`.
  - 3 unit tests del módulo.
- Sweep de ~30 sitios user-facing en `storage.rs`, `bptree.rs`, `sql.rs`, `catalog.rs`, `index.rs`, `server.rs`: cada error visible para CLI/HTTP/embedido ahora pasa por `coded(...)`.
- Auth fallida (`401`) y server-busy (`503`) llevan códigos `[GBY-5004]` y `[GBY-5005]` respectivamente.
- Nuevo documento normativo [docs/ERROR_CODES.md](docs/ERROR_CODES.md) — catálogo operacional con cada código: causa, remedio, ejemplo de mensaje real, integración desde CLI/HTTP/Rust/Python.
- README, ERROR_HANDLING y CONTRIBUTING enlazan al catálogo.

### 🎯 Por qué este cambio
Pregunta del usuario: *"y tener un número referencial como MySQL tiene para el manejo de errores"*. Razón concreta: el texto de un mensaje puede evolucionar (mejor redacción, más contexto), pero un cliente que reacciona programáticamente al error necesita un contrato estable. El código numérico **es** ese contrato.

Ahora:
- Las herramientas pueden hacer `grep -oE 'GBY-[0-9]{4}'` para detectar la clase del error sin parsear texto humano.
- El troubleshooting tiene un eje claro: cada código apunta a su entrada en [ERROR_CODES.md](docs/ERROR_CODES.md).
- Los clientes embebidos pueden hacer `text.starts_with("[GBY-3001]")` para detectar PK duplicada sin depender de la redacción exacta.

### 🛡️ Decisión: constantes Rust, no JSON externo
Documentada en [src/errors.rs](src/errors.rs) y en la sección "Por qué constantes en Rust" del catálogo:
- Zero-deps (ADR-0001) — sin filesystem I/O al startup.
- Type-checked: el compilador detecta renames; con JSON sería un test runtime dedicado.
- Misma flexibilidad práctica: cambiar un mensaje es edit + rebuild + redeploy en cualquier caso.
- i18n futuro se resuelve con `feature` flags si llega, sin filesystem.

### 🛡️ Restricciones respetadas
- **Cero deps.** ADR-0001 intacto.
- **Cero bump de formato.** VERSION 7 sigue válido.
- **Cero rotura del contrato externo.** Los mensajes ahora prefijan con `[GBY-NNNN]`, pero los clientes que no parsean el texto (mayoría) no se ven afectados.
- **45/45 integration + 30 lib + 4 server + 3 errors unit tests verdes.**

### 📐 Documentos
- [docs/ERROR_CODES.md](docs/ERROR_CODES.md) — catálogo completo de los ~30 códigos.
- [docs/ERROR_HANDLING.md](docs/ERROR_HANDLING.md) — guía de estilo (actualizada para reflejar el nuevo sistema de códigos).

---

## 2026-05-18 — Vigesimoquinta intervención: guía canónica de manejo de errores + sweep al español + enriquecimiento

> **Sin bump de formato. Sin deps añadidas. Levanta la barra de calidad de los mensajes de error a nivel producto.** Cierra el síntoma "los errores en pantalla son pobres y no aclaran nada".

### ✨ Cambio
- Nuevo documento canónico [`docs/ERROR_HANDLING.md`](docs/ERROR_HANDLING.md) — guía normativa para los ~210 sitios donde se construyen errores en el motor:
  - Filosofía: cada mensaje responde *qué pasó*, *por qué*, y (cuando aplica) *cómo se resuelve*.
  - Reglas de estilo: idioma español, minúscula, sin punto final, incluir el nombre concreto del objeto, incluir el dato del fallo, sugerir el remedio.
  - 8 categorías canónicas (validación, NotFound, Conflict, Constraint, Limitación, Integridad, Estado interno, I/O) cada una con patrón recomendado.
  - Mapeo sistemático a HTTP (400/401/404/405/409/500/503).
  - Anti-patrones explícitos (mensajes de una palabra, `unwrap` que miente, `From` que enmascara, idiomas mezclados, secretos en mensajes).
  - Checklist de PR para revisar cualquier nuevo `DbError::new(...)`.

- **Traducción al español de todos los mensajes en inglés** heredados de iteraciones previas:
  - `storage.rs`: `tx already started` → `transacción ya iniciada`; `no active tx` → `no hay transacción activa: commit() requiere un begin() previo`; `bad magic` → `magic bytes inválidos: el archivo no es una base de datos gabysql`; `unsupported gabysql file format` → `formato de archivo gabysql no soportado`; `refusing to overwrite` → `se rehúsa sobrescribir base de datos existente`; `database is locked by another process` → `base de datos bloqueada por otro proceso`; etc.
  - `bptree.rs`: `root page is 0`, `leaf overflow`, `page too small`, `not a leaf page`, `leaf decode overflow`, `internal too large`, `unknown page type`, etc. — todos en español con contexto.
  - `server.rs`: mensajes de `read_request` (`request line vacía`, `método faltante`, `escape URL inválido`), validación de `-max-connections`, mensajes de auth/multi-DB.
  - `index.rs`: `bucket de índice corrupto` con offset, count, len y descripción precisa.

- **Enriquecimiento de mensajes pobres**. Los ~20 mensajes que eran 1-3 palabras y no orientaban al operador ahora incluyen contexto:
  - `default corrupto (kind)` → `DEFAULT corrupto: buffer agotado en offset {N} (len={M}), falta el byte de kind`.
  - `string corrupto` → `string serializado corrupto en offset {N}: header declara {L} bytes pero solo quedan {R} bytes en el buffer`.
  - `fila corrupta (INT)` → `fila corrupta en tabla '{T}': campo '{C}' (INT) necesita 8 bytes en offset {N}, solo quedan {R}`.
  - `db vacío` → `parámetro 'db' vacío: indique el nombre del archivo .db dentro del directorio configurado`.
  - `meta de tabla corrupta` → `TableMeta '{T}' corrupta: faltan bytes para el header de la columna {i} ('{C}') en offset {N}`.
  - `colisión de hash en catálogo` → mensaje completo que dice qué nombres colisionaron y que se debe reportar como bug.
  - `cantidad columnas != valores` → `INSERT INTO '{T}': cantidad de columnas ({c}) no coincide con cantidad de valores ({v})`.

- **3 tests de integración actualizados** que asertaban sobre los strings originales (`duplicate primary key`, `refusing to overwrite`, `locked`) — ahora aceptan tanto el texto en español como, por compatibilidad transicional, el inglés equivalente cuando es razonable.

### 🎯 Por qué este cambio
Auditoría con el usuario: "los errores en pantalla son pobres en indicaciones y no aclaran nada". La auditoría confirmó:
- Existía una convención **observada** pero **no escrita** sobre los mensajes.
- Muchos eran de 1-3 palabras (`db vacío`, `string corrupto`, `fila corrupta (INT)`) — imposibles de buscar en troubleshooting y sin información accionable.
- Había mezcla de español e inglés sin razón.
- Sin documento normativo, un PR podía agregar `"Column Not Found."` y nada lo paraba.

Ahora hay tres cosas concretas:
1. **Documento normativo** (`docs/ERROR_HANDLING.md`) que define qué es un mensaje aceptable.
2. **Estado actual auditado** — ~210 sitios revisados, todos cumplen las reglas.
3. **Checklist de PR** para que nuevos errores se midan contra la guía.

### 🛡️ Restricciones respetadas
- **Cero deps añadidas.** ADR-0001 intacto.
- **Cero bump de formato.** VERSION 7 sigue válido.
- **Cero rotura de API.** Los `Display::fmt` siguen devolviendo el texto puro; los clientes que no leen el texto no se ven afectados.

### 📐 Documentos
- [docs/ERROR_HANDLING.md](docs/ERROR_HANDLING.md) — guía canónica completa (las 8 categorías, checklist de PR, anti-patrones).

---

## 2026-05-18 — Vigesimocuarta intervención: ADR-0018 (Propuesta) — WAL-mode opt-in (sólo diseño)

> **Sin código. Sin bump de formato.** Cierre honesto del ítem "checkpoint del WAL" de Fase 2: el diseño queda capturado con scope, alternativas y condiciones de salida explícitas, pero la implementación se difiere hasta que aparezca medición de `gabybench` o demanda real. Justificación completa: [ADR-0018](docs/adr/0018-wal-mode-opt-in.md).

### ✨ Cambio
- Nuevo [ADR-0018](docs/adr/0018-wal-mode-opt-in.md) en estado **Propuesta**. Describe:
  - El modelo WAL-per-transaction actual y por qué "checkpoint" no aplica.
  - El modelo propuesto: WAL persistente, `Pager::checkpoint()` explícito, `wal_index` in-memory, read-path WAL-aware.
  - Alternativas evaluadas y descartadas (group commit, mmap, auto-checkpoint, etc.).
  - **Condiciones de salida** (cuándo pasa a "Aceptada" + implementación): cuando `gabybench` muestre fsync(.db) como cuello de botella, o aparezca workload write-heavy con métricas concretas, o se necesite MVCC.
- ROADMAP.md actualizado: el ítem pasa de "diferido sin condiciones" a "diseño aceptado, implementación deferida con condiciones de salida documentadas".

### 🎯 Por qué este formato
Implementar WAL-mode real es ~400-600 LOC en el hot path del Pager con riesgo de regresión alto y sin un workload medido que lo justifique. Hacerlo a ciegas para "marcar el bloque como entregado" contradice la honestidad del resto de Fase 2 (donde cada bloque mostró su scope real, no inflado).

El diseño completo es valor por sí mismo: cualquier persona futura — humana o agente — que retome el ítem encuentra el análisis listo, las alternativas evaluadas, y el contrato de cuándo activarlo. Eso es lo que se entrega.

### 📐 ADR
- [ADR-0018 — WAL-mode opt-in con checkpoint explícito](docs/adr/0018-wal-mode-opt-in.md).

---

## 2026-05-18 — Vigesimotercera intervención: índice INT-ordenado + range scan (Fase 2 — VERSION 7)

> **Bump de formato VERSION 6 → 7.** Cierra el ítem "range scan por índice secundario" del roadmap, restringido honestamente a columnas INT. Justificación completa: [ADR-0017](docs/adr/0017-int-ordered-index-version-7.md).

### ✨ Cambio
- **VERSION on-disk pasa de 6 a 7.** Archivos V6 se rechazan limpiamente al abrir (mensaje "Re-create the database with the current binary"). Igual patrón que cada bump anterior.
- **Nuevo `IndexKind`** en `IndexMeta` ([src/catalog.rs](src/catalog.rs)):
  - `Hash` (ADR-0005): el layout legacy. Usado para TEXT/FLOAT/BOOL/DATE/DATETIME. **Equality only**.
  - `OrderedInt` (nuevo): para columnas INT. El B+Tree se indexa por el valor directamente; los buckets son solo `[count:u16] + count × pk:i64`. Soporta range scan.
  - `IndexKind::for_column(column_type)` decide automáticamente al crear el índice. Cero cambios al SQL externo.
- **Nuevo path `WHERE col_idx BETWEEN a AND b`** sobre columnas INT indexadas: ejecutor llama a `lookup_pks_via_index_range` que usa `Tree::cursor_range(idx.root_page, from, to)` y devuelve los PKs en O(log N + k).
- **BETWEEN sobre columna TEXT/FLOAT/etc. indexada falla loud** con mensaje claro:
  *"el índice secundario es hash-based (equality only). Solo columnas INT-indexadas admiten BETWEEN."*
- **NULL no se almacena en índices OrderedInt**. SQL `BETWEEN` ignora NULL por definición y UNIQUE permite múltiples NULLs; ambas semánticas caen naturalmente al no indexar la representación NULL.
- Helpers nuevos en [src/index.rs](src/index.rs): `ordered_int_key_from_value_bytes`, `encode_ordered_bucket`/`decode_ordered_bucket`, `ordered_bucket_insert`/`_remove`/`_unique_conflict`.
- Integrity check ([src/sql.rs](src/sql.rs)) y FK cascade lookup branchean por `idx.kind` para decodificar el bucket correcto.
- **2 tests nuevos**: range BETWEEN sobre INT indexado (incluyendo verify que NULL queda fuera) y rechazo BETWEEN sobre TEXT indexado.

### 🎯 Por qué este cambio (y por qué INT solamente)
ADR-0005 había fijado el índice como **hash-based** (FNV-1a-64) para tolerar colisiones de hash con un bucket por clave. Equality funciona; range no compone — hashes de valores cercanos son arbitrariamente distintos. El ítem del roadmap "range scan por índice secundario" había sido marcado como **no viable bajo VERSION 6** explícitamente en intervenciones previas.

La salida natural es usar el valor como clave del B+Tree donde el orden i64 ya es el orden semántico — **solo INT** cumple sin tocar el motor. TEXT requeriría un B+Tree byte-keyed (~800+ LOC, riesgo de regresión); FLOAT necesita encoding flip-sign no-trivial. Ambos quedan diferidos a un bloque futuro cuando aparezca demanda real.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001).
- **Memoria acotada** (ADR-0009 — el bucket ordenado es estrictamente más chico que el bucket Hash equivalente).
- **Convivencia limpia**: índices Hash siguen funcionando para los tipos no-INT (ADR-0005 sigue vigente).
- **Sin cambios al cursor**: `Tree::cursor_range` (ADR-0008) ya servía perfectamente.

### 📐 ADR
- [ADR-0017 — Índice secundario INT-ordenado para range scan (VERSION 7)](docs/adr/0017-int-ordered-index-version-7.md).

### 📝 Notas
- **Índices compuestos no entran en este bloque.** El roadmap inicial los agrupaba con range scan bajo el mismo bump, pero compuestos requieren claves multi-columna que con el approach value-as-i64 es forzado. Quedan diferidos a un futuro VERSION 8 (o se entregan dentro de VERSION 7 si la demanda aparece sin necesidad de cambio de formato).

---

## 2026-05-18 — Vigesimosegunda intervención: prefetch one-leaf-ahead en `LeafCursor` (Fase 2 — performance directional)

> **Sin bump de formato. Sin deps añadidas. Mejora direccional sin medición cuantitativa todavía.** Justificación completa: [ADR-0016](docs/adr/0016-leafcursor-prefetch.md).

### ✨ Cambio
- 4 líneas nuevas en [src/bptree.rs](src/bptree.rs::LeafCursor::load_current): después de cargar la hoja actual, si hay siguiente, se hace `page_data` sobre ella para llevarla al `PageCache` (ADR-0009). Best-effort: errores de prefetch se descartan; el error real va a surgir en la próxima iteración real del cursor.
- Nuevo helper `Pager::cache_contains(page_no) -> bool` ([src/storage.rs](src/storage.rs)) para tests + futura tooling operacional.

### 🎯 Por qué este cambio
El `LeafCursor` (ADR-0008) ya hace lo correcto algorítmicamente, pero presenta al kernel y al `PageCache` un patrón de I/O **stop-and-go**: lee hoja N, deja que el caller procese 100 filas (pausa larga), entonces lee hoja N+1. Esto:
1. **Confunde el readahead del kernel**, que necesita lecturas back-to-back para detectar streaming.
2. **Garantiza un cache miss en cada leaf transition** — la primera lectura post-transición siempre paga el costo de syscall + CRC verify.

Prefetcheando la próxima hoja al final de la carga de la actual, el syscall ocurre antes y para cuando el caller la pide, ya está en cache.

### 🛡️ Honestidad sobre la mejora
- **No hay número absoluto todavía.** `gabybench` (la suite reproducible especificada en `docs/GABYBENCH_SPEC.md`) no existe aún. Cuando exista, esto se mide.
- **Sobrelectura potencial de 1 hoja en queries cortas** (`LIMIT N` que cabe en la primera hoja).
- El ADR vende esto como **directional**, no como "scan 2x más rápido".

### 📐 ADR
- [ADR-0016 — Prefetch one-leaf-ahead en `LeafCursor`](docs/adr/0016-leafcursor-prefetch.md).

---

## 2026-05-18 — Vigesimoprimera intervención: backup/restore/verify con validación end-to-end (Fase 2 — operación)

> **Sin bump de formato. Sin deps añadidas.** Cierra el gap operacional "no hay forma confiable de respaldar". Justificación completa: [ADR-0015](docs/adr/0015-verified-backup-restore.md).

### ✨ Cambio
- Nuevo módulo [src/backup.rs](src/backup.rs) con tres entradas públicas: `backup`, `restore`, `verify`. Todas validan **CRC32 página por página en lectura** y, post-escritura, **re-abren el destino y revalidan cada página**. Si una sola página falla el CRC en cualquiera de las dos fases, la operación aborta — nunca se publica un backup roto.
- Nuevos subcomandos CLI:
  - `gabysql backup [--force] <src.db> <dst.db>`
  - `gabysql restore [--force] <src.db> <dst.db>` (alias semántico)
  - `gabysql verify <file.db>`
- Salida estructurada: `OK backup  src=...  dst=...  pages=N  bytes=M`.
- 3 tests de integración nuevos: round-trip con verify, detección de corrupción en origen (byte flip rechaza el backup), verify sobre DB sana.

### 🎯 Por qué este cambio
La operación de respaldo era "`cp demo.db backups/demo.db.bak`" — sin validación, sin awareness del WAL, sin garantía de que el destino se pudiera *usar*. Una página corrupta en el origen se replicaba al backup sin warning hasta que alguien intentaba restaurar (semanas después, en una emergencia).

Ahora el contrato es claro:
- Si el comando termina con `OK`, el archivo destino se puede abrir con el mismo binario, todas sus páginas tienen CRC válido, y su header coincide con el origen.
- Si algo falla, error explícito que apunta a la página corrupta o la causa raíz.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001 intacto).
- **Cero bump de formato.** VERSION = 6 sigue válido — el destino es un `.db` regular.
- **Lock exclusivo** vía ADR-0013: la DB debe estar cerrada por otros procesos (server apagado). Endpoint server-side `/backup` que tome el `write_lock` queda para Fase 3.

### 📐 ADR
- [ADR-0015 — Backup / restore / verify con validación end-to-end](docs/adr/0015-verified-backup-restore.md).

### 📝 Ejemplo
```powershell
# Cierre el server primero (el lock exclusivo bloquea backups online)
gabysql backup demo.db backups/demo.db.bak
# → OK backup  src=demo.db  dst=backups/demo.db.bak  pages=128  bytes=524288

# Verificar un backup antiguo
gabysql verify backups/demo.db.bak
# → OK verify  path=backups/demo.db.bak  pages=128  bytes=524288

# Restaurar
gabysql restore --force backups/demo.db.bak demo.db
```

---

## 2026-05-18 — Vigésima intervención: logs JSON + endpoint `/metrics` en el server (Fase 2 — observabilidad)

> **Sin bump de formato. Sin deps añadidas.** Primer paso de observabilidad operacional para `gabysql-server`. Justificación completa: [ADR-0014](docs/adr/0014-logs-json-metrics.md).

### ✨ Cambio
- Nuevo struct `Metrics` en [src/server.rs](src/server.rs): contadores por status HTTP, `errors_total` (status ≥ 500), y ring buffer acotado de 1024 latencias para p50/p95. Memoria O(1) bajo carga sostenida.
- Nuevo endpoint **`GET /metrics`**:
  ```json
  {"ok":true,"started_unix":...,"uptime_s":3600,"requests_total":1234,
   "requests_by_status":{"200":1180,"400":30,"500":24},
   "errors_total":24,
   "latency_ms":{"p50":5,"p95":87,"samples":1024,"count":1234}}
  ```
  Gated por `-token` igual que el resto de la API.
- Nuevo flag **`-log-json`** en `gabysql-server`. Cuando se activa, cada request finalizado emite una línea JSON a stdout:
  ```json
  {"ts_unix":1747497612,"method":"POST","path":"/exec","status":200,"latency_ms":12}
  ```
  Por defecto **off** — la UX del binario silencioso de hoy no cambia. Útil con `tee`, `jq`, ingest a S3/ELK/Loki.
- 4 tests unitarios nuevos: registro de status + latencia, percentiles sobre 1..=100, comportamiento con buffer vacío, ring buffer acotado bajo overflow.

### 🎯 Por qué este cambio
El binario en producción era opaco: sin logs por request, sin contadores agregados, sin forma de responder "¿cómo se está comportando bajo carga?". El RUNBOOK pedía observabilidad básica pero no había nada que pedirle al server más allá de `/health`.

Ahora cualquier operador puede:
- Curl `/metrics` y ver counts por status + p50/p95 inmediatamente.
- Activar `-log-json` y pipear a `jq '. | select(.latency_ms > 100)'` para encontrar requests lentas.
- Configurar una alerta sobre `errors_total` creciendo.

Y todo sin agregar una sola dependencia.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001 intacto). Sin `tracing`, sin `prometheus`, sin `metrics-rs`.
- **Memoria acotada** (ADR-0009 mismo principio). Ring buffer de 1024 × 4 bytes = 4 KB por server.
- **Opt-in** para logs. Defaults preservan la UX silenciosa.
- **Sin bump de formato**. VERSION = 6 sigue válido.

### 📐 ADR
- [ADR-0014 — Logs JSON estructurados + endpoint `/metrics` en el server](docs/adr/0014-logs-json-metrics.md).

---

## 2026-05-18 — Decimonovena intervención: lock exclusivo cross-process sobre el `.db` (Fase 2 — concurrencia)

> **Sin bump de formato. Sin deps añadidas.** Cierra el gap de corrupción silenciosa cuando dos procesos abren la misma DB. Justificación completa: [ADR-0013](docs/adr/0013-process-level-file-lock.md).

### ✨ Cambio
- Nuevo helper privado `acquire_db_lock(&File, &Path)` en [src/storage.rs](src/storage.rs) que llama `File::try_lock()` (advisory exclusivo, **estable desde Rust 1.89.0**).
- Aplicado en `Pager::create` / `Pager::create_force` / `Pager::open`: el lock se adquiere tras abrir el handle y antes de cualquier escritura o replay del WAL.
- `Pager::close` libera el lock explícitamente con `file.unlock()` (drop del `File` también lo libera como red de seguridad).
- Si otro proceso (o incluso otro `Pager` en el mismo proceso) ya tiene la DB tomada, la segunda apertura **falla rápido** con:
  ```
  database is locked by another process: <path>.
  Close the other gabysql process or wait for it to release the lock.
  ```
  No hay espera bloqueante, no hay cuelgue.
- Test nuevo `cross_process_lock_rejects_second_open` que valida: primer `Pager::create` toma el lock → `Pager::open` segundo falla con mensaje claro → `close` del primero libera → `Pager::open` tercero funciona.

### 🎯 Por qué este cambio
La WAL+CRC de `gabysql` asume **un único escritor por archivo**. Sin lock cross-process, dos `gabysql` apuntando al mismo `.db` (server + CLI accidental, server reiniciado con proceso huérfano vivo, etc.) escribían páginas en paralelo y corrompían el archivo. El motor detectaba la corrupción **después** vía CRC, pero el daño ya estaba hecho.

Ahora la corrupción por doble apertura es **imposible**: el segundo proceso no llega a tocar el archivo.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001 intacto). Uso exclusivo de `std::fs::File::try_lock` / `unlock`.
- **Cero bump de formato** (VERSION = 6 sigue válido).
- **Cross-platform**: Windows (`LockFileEx` bajo el capó), Linux (`flock(2)` advisory), macOS (`flock(2)`). Los tres validados en CI.
- **No-bloqueante**: `try_lock` falla inmediatamente; el caller decide qué hacer.

### 📐 ADR
- [ADR-0013 — Lock exclusivo a nivel de proceso sobre el archivo `.db`](docs/adr/0013-process-level-file-lock.md).

### 📝 Notas de roadmap
- Re-evaluado el ítem **"checkpoint/compaction del WAL"** de Fase 2: el WAL actual es per-transaction y se trunca/borra en cada commit (no acumula a través de commits), así que el concepto clásico de checkpoint no aplica sin un cambio previo a WAL persistente. Diferido hasta que aparezca demanda concreta.
- Re-evaluado el ítem **"range scan por índice secundario"**: el índice 2º actual es hash-based (FNV-1a-64, ADR-0005) y no admite range nativo. Agrupado con índices compuestos bajo un futuro bump VERSION 6 → 7 que reestructurará el índice a B+Tree ordenado.

---

## 2026-05-08 — Decimoctava intervención: audit log enriquecido en el gateway (Fase 5 — AI-native, cierre del trío)

> **Sin bump de formato. Sin cambios al motor.** Tercera y última pieza del trío AI-native sobre el gateway. Justificación completa: [ADR-0012](docs/adr/0012-audit-log-enriquecido.md).

### ✨ Cambio
- Nuevo flag `--audit-log <ruta>` (también `GABYSQL_AUDIT_LOG`) en [src/bin/gabysql-mcp.rs](src/bin/gabysql-mcp.rs). Si no se pasa, sin log y overhead cero.
- Nuevo argumento opcional `reason` en `gabysql_execute`: el "por qué" semántico que el agente puede pasar para que quede en el audit.
- Captura de `clientInfo` (`name` + `version`) en el handshake `initialize` → guardado en `RuntimeState` interno y emitido en cada entrada del log.
- Cada llamada a `gabysql_execute` y `gabysql_integrity_check` anexa una línea JSON al archivo (formato JSONL):
  ```json
  {"ts_unix":1730000000,"tool":"gabysql_execute","db":"rag.db",
   "sql":"INSERT INTO docs ...","reason":"backfill inicial del corpus",
   "client":{"name":"claude-desktop","version":"1.2.3"},
   "ok":true,"error":null}
  ```
- Nueva tool **`gabysql_audit_tail(n)`** que devuelve las últimas N entradas. Permite que **el propio agente** revise su historial dentro de la sesión. Si el log no está activo, devuelve `{"enabled":false,"entries":[]}` sin error.
- Append best-effort: si escribir al archivo falla, va a stderr y la tool sigue devolviendo el resultado del motor (mejor perder una entrada que bloquear escrituras por disco lleno).
- 5 tests nuevos: captura de clientInfo, append+tail roundtrip con `reason`+`client`, comportamiento con log desactivado, presencia de `gabysql_audit_tail` en `tools/list`, formato JSONL (una entrada por línea, JSON válido por línea).

### 🎯 Por qué este cambio
Cuando un agente puede escribir en una base, el log del motor responde **el qué** (qué SQL corrió) pero no **el por qué** (qué pidió el usuario, qué identidad tenía el agente, qué razonamiento lo llevó allí). Meter eso en el motor implica bump de formato y que el motor entienda conceptos MCP que no le pertenecen.

Mover el audit al gateway captura el "por qué" exactamente donde el conocimiento existe — el gateway ya sabe quién es el cliente, qué tool se invocó, qué `reason` pasó el agente. Y cierra el loop dándole al propio agente la tool para releer sus acciones. Eso permite patrones de auto-corrección dentro de la misma sesión.

### 🛡️ Cómo se respeta el motor
- **Cero líneas tocadas en `storage.rs`/`bptree.rs`/`sql.rs`/`catalog.rs`/`server.rs`/`lib.rs`.** Solo crece `src/bin/gabysql-mcp.rs`.
- **Sin bump de formato.** Sin nuevas deps. `Cargo.toml`/`Cargo.lock` sin tocar.
- **Opt-in puro.** Sin `--audit-log` el comportamiento es idéntico al gateway pre-ADR — ni un syscall extra.
- **Retrocompatible**: clientes MCP que no pasan `reason` siguen funcionando sin cambios.

### 📐 ADR
- [ADR-0012 — Audit log enriquecido en el gateway, no en el motor](docs/adr/0012-audit-log-enriquecido.md). Cierra el trío con [ADR-0010](docs/adr/0010-mcp-gateway.md) (gateway base) y [ADR-0011](docs/adr/0011-vector-search-gateway-side.md) (vectores).

### 🧪 Ejemplo de uso desde un agente MCP
```bash
# Server + gateway con audit activo
gabysql-server -dir ./dbs -token MI_TOKEN
gabysql-mcp --token MI_TOKEN --audit-log /var/log/gabysql/agent-audit.jsonl
```
```json
{ "method":"tools/call", "params":{
    "name":"gabysql_execute",
    "arguments":{
      "db":"rag.db",
      "sql":"UPDATE users SET email='nuevo@x.com' WHERE id=42",
      "reason":"el usuario reportó que su email anterior ya no funciona"
}}}
```
La línea correspondiente del JSONL queda con `reason`, `client`, `sql`, `ok`. Procesable con `jq '.[] | select(.tool=="gabysql_execute")'` o ingestable a cualquier sink.

---

## 2026-05-07 — Decimoséptima intervención: búsqueda vectorial del lado del gateway (Fase 5 — AI-native, parte 2)

> **Sin bump de formato. Sin cambios al motor.** Esta intervención añade búsqueda vectorial top-k a `gabysql-mcp`. Los vectores se guardan como `TEXT` (`'[0.1,0.2,...]'`); el cómputo ocurre en el binario del gateway. Justificación completa: [ADR-0011](docs/adr/0011-vector-search-gateway-side.md).

### ✨ Cambio
- Nueva tool MCP **`gabysql_vector_search`** en [src/bin/gabysql-mcp.rs](src/bin/gabysql-mcp.rs):
  - Args: `db?`, `table`, `pk_column?` (default `id`), `vector_column`, `query: number[]`, `top_k?` (default 10), `metric?` (default `cosine`).
  - Métricas: `cosine`, `euclidean`/`l2`, `dot`/`ip`.
  - Hace `SELECT <pk>, <vec_col> FROM <table>` vía el HTTP existente, parsea cada vector, computa la distancia y devuelve top-k por heap selection.
  - Identificadores validados con `safe_ident` (regex implícito `[A-Za-z_][A-Za-z0-9_]*`) antes de interpolar al SQL — bloquea inyección.
  - Filas con vector mal formado o de dimensión distinta a la query van al campo `skipped` de la respuesta (no se silencian).
- 9 tests unitarios nuevos: cosine identity/orthogonal, euclidean Pitágoras, dot con sort ascendente, dimension mismatch, vector cero, top-k heap, validador de identificadores (acepta válidos / rechaza inyección), aliases de métrica, schema visible en `tools/list`.

### 🎯 Por qué este cambio
La búsqueda vectorial es lo que la mayoría de agentes LLM espera de una "DB para los nuevos tiempos". El camino correcto a largo plazo es un tipo `VECTOR(n)` nativo con índice ANN — pero eso requiere bump de formato, cambios profundos en `sql.rs`/`storage.rs`/`bptree.rs`, y meses de trabajo. **Hacerlo "para validar el use case" es prematuro.**

Esta entrega resuelve el 80% del valor (top-k usable hoy desde cualquier cliente MCP) con el 5% del riesgo (cero líneas tocadas en el motor). El ADR-0011 documenta las **condiciones de salida explícitas** para promover a `VECTOR(n)` nativo cuando la señal aparezca: dataset > 100K vectores, demanda de operadores SQL, o necesidad de índice ANN.

### 🛡️ Cómo se respeta el motor
- **No se toca `Cargo.toml`/`Cargo.lock`.** Sin nuevas deps. ADR-0001 intacto.
- **No se toca `src/lib.rs` ni ningún archivo del motor.** Solo crece `src/bin/gabysql-mcp.rs`.
- **No se cambia el formato en disco.** Los vectores son `TEXT`; `INSERT INTO docs (id, content, embedding) VALUES (1, 'texto', '[0.1,0.2,...]')` es SQL estándar que el motor procesa sin saber que es un vector.
- **Storage existente sigue válido.** DBs viejas no requieren migración.

### 📐 ADR
- [ADR-0011 — Búsqueda vectorial del lado del gateway, no en el motor](docs/adr/0011-vector-search-gateway-side.md)

### 🧪 Ejemplo de uso desde un agente MCP
```json
{ "method": "tools/call", "params": {
    "name": "gabysql_vector_search",
    "arguments": {
      "db": "rag.db",
      "table": "docs",
      "vector_column": "embedding",
      "query": [0.12, -0.04, 0.88, /* ... */],
      "top_k": 5,
      "metric": "cosine"
    }
} }
```

---

## 2026-05-07 — Decimosexta intervención: gateway MCP — `gabysql-mcp` (apertura Fase 5 AI-native)

> **Sin bump de formato. Sin cambios al motor.** Esta intervención añade un binario nuevo (`gabysql-mcp`) que es cliente del `gabysql-server` HTTP/JSON existente. No abre el `.db`, no instancia un `Pager`, no toca `storage.rs` / `bptree.rs` / `catalog.rs` / `sql.rs`. El motor queda intacto. Justificación completa: [ADR-0010](docs/adr/0010-mcp-gateway.md).

### ✨ Cambio

- Nuevo binario `src/bin/gabysql-mcp.rs` (~700 líneas, **cero dependencias externas**) que habla el protocolo **MCP (Model Context Protocol)** sobre stdio (JSON-RPC 2.0 delimitado por `\n`).
- Cinco tools expuestas a cualquier cliente MCP-compatible (Claude Desktop, Claude Code, Cursor, etc.):
  - `gabysql_list_databases` → wrap de `GET /dbs`
  - `gabysql_describe_database` → wrap de `GET /tables[?db=…]`
  - `gabysql_query` → wrap de `POST /exec` para `SELECT`/`SHOW`/`DESCRIBE`
  - `gabysql_execute` → wrap de `POST /exec` para `INSERT`/`UPDATE`/`DELETE`/DDL (omitida si se lanza con `--read-only`)
  - `gabysql_integrity_check` → wrap de `POST /exec` con `INTEGRITY CHECK`
- Dos resources MCP:
  - `gabysql://catalog` → lista de bases disponibles
  - `gabysql://schema/{db}` → schema completo de una DB
- Flags: `--server URL` (default `http://127.0.0.1:7878`, también `GABYSQL_SERVER`), `--token T` (también `GABYSQL_TOKEN`), `--read-only`.
- Tests unitarios en el mismo archivo cubren: parser JSON (round-trip + escapes), `initialize`, `tools/list` con y sin `--read-only`, `resources/list`, `ping`, método desconocido, notifications sin id, parsing de URL del server.

### 🎯 Por qué este cambio

El consumidor que más rápido crece en el ecosistema es el agente LLM. Hoy una IA que quiera usar `gabysql` necesita: cliente HTTP a mano + token + el schema de la DB metido en el prompt + reintentos sobre errores SQL sin trazabilidad. Ese pegamento se reescribe en cada integración.

MCP es el estándar emergente que define cómo un servidor expone *tools* y *resources* a clientes-agentes. Si `gabysql` lo habla de fábrica, cualquier agente lo enchufa directo:

```bash
gabysql-server -dir ./dbs -token MI_TOKEN
gabysql-mcp --server http://127.0.0.1:7878 --token MI_TOKEN
# Claude Desktop / Claude Code / Cursor lanzan gabysql-mcp como subprocess
# y descubren las 5 tools + 2 resources sin código de pegamento.
```

### 🛡️ Cómo se respeta el motor

- **No se toca `Cargo.toml`.** El binario se auto-descubre desde `src/bin/`. `Cargo.lock` no añade un solo paquete.
- **No se cambia `[lib]`.** Sigue compilando con cero deps externas. [ADR-0001](docs/adr/0001-rust-zero-deps-core.md) intacto.
- **No se abre el `.db`.** El gateway hace doble salto stdio→HTTP→Pager, así heredas todo lo que ya está endurecido en `server.rs`: `write_lock` global, tope de conexiones, bearer token, CORS preflight, validación de SQL antes de pegar al Pager.
- **No se cambia el formato en disco.** Sin bump de VERSION, sin nuevo tipo de página, sin cambio en el WAL.

### ✅ Tests
- Módulo `#[cfg(test)] mod tests` en `src/bin/gabysql-mcp.rs`: 9 tests cubren parser JSON, dispatch JSON-RPC y semántica de `--read-only`. CI multi-OS los ejecuta vía `cargo test`.

### 📐 ADR
- [ADR-0010 — Gateway MCP como adaptador externo sobre el HTTP/JSON existente](docs/adr/0010-mcp-gateway.md): promovida de 🟡 Propuesta a ✅ Aceptada con la implementación.

---

## 2026-05-08 — Decimoquinta intervención: `PageCache` LRU acotado — cierra fuga de memoria del server

> **Sin bump de formato.** El cambio es interno al Pager. La API pública del Pager se mantiene compatible salvo dos métodos nuevos (`set_cache_capacity`, `cache_len`, `cache_capacity`).

### ✨ Cambio
- Reemplazo de `cache: BTreeMap<u32, CachedPage>` (que crecía sin límite) por `cache: PageCache` con **capacidad fija** + **eviction LRU sobre páginas clean**.
- Constante `DEFAULT_CACHE_PAGES = 1024` (~4 MB por DB con páginas de 4 KB). Configurable por instancia con `Pager::set_cache_capacity(n)`.
- LRU implementada con `HashMap<u32, CacheSlot>` + contador monótono (touch en cada `get/get_mut/insert`). Eviction = scan O(N) sobre el map cuando está lleno; para 1024 entradas son µs por inserción.
- Política dirty-aware: **las páginas dirty nunca se evictan** — pertenecen a la transacción abierta y deben llegar al WAL antes de poder dropearse. Si el cache llega a capacidad lleno de dirty, se permite overflow temporal: perder una página dirty corromperia la DB. El overflow drena solo en el commit (todas pasan a clean simultáneamente).

### 🎯 Por qué este cambio

**Pre-bloque-10:**
```rust
struct Pager {
    cache: BTreeMap<u32, CachedPage>,  // ← crece sin freno
}
```
Un `INTEGRITY CHECK` o un `SELECT` con full scan sobre una DB de 200 MB cargaba ~50 K páginas en RAM y **nunca las liberaba**. En `gabysql-server -dir ./dbs` con 50 DBs activas y un sweep operacional periódico, la memoria del server crecía a 10 GB y eventualmente lo mataba el OOM killer. Sin error, sin warning, sin recovery — solo `kill` y reiniciar.

**Post-bloque-10:**
```rust
struct PageCache {
    capacity: usize,                       // bounded
    map: HashMap<u32, CacheSlot>,
    counter: u64,                          // monotonic for LRU
}
```
Memoria del server acotada por `cache_capacity × #DBs_abiertas × page_size`. Para 50 DBs × 1024 páginas × 4 KB = **200 MB max**, predecible, no swappea.

### 🛡️ Comportamiento bajo casos edge
- **Workload chico con cache vacío**: idéntico a antes (cache nunca se llena, no evicta nada).
- **Workload grande de read-only**: evicta clean pages LRU. La página menos usada se cae; si vuelve a pedirse, se relee de disco con CRC verificado (mismo path que cold load).
- **Mid-transaction con muchas writes**: dirty pages se acumulan; clean pages preexistentes se evictan primero. Si el commit se retrasa y entra más dirty que cap, el cache excede cap **temporalmente** (correctness > strict cap). Drena en commit.
- **Rollback**: `cache.clear()` libera todo (mismo path que antes).

### 🧪 Validación
- 39/39 tests de integración (1 nuevo: `page_cache_is_bounded_and_evicts_clean_pages` siembra 200 filas, abre con `set_cache_capacity(4)`, recorre cada página de la DB y asserta que `cache_len() <= 4`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 🔭 `Transaction` (Unit of Work) — pospuesto a bloque futuro
La recomendación original de bloque 10 incluía un objeto `Transaction` que reemplazara las 40+ aperturas de `Catalog::open(self.pager)` por una unit-of-work compartida con cache de `TableMeta`. Después de medir el impacto real:
- La fuga de memoria del cache es **inmediata** (problema agudo del server).
- La memoización de `TableMeta` es **marginal** (lookup hash + decode = µs; el ahorro existe pero no aparece en profiles de workloads reales).
- El refactor de 40 sitios cuesta ~1500 líneas y rompe muchos diffs en revisiones.

Decisión: **se entrega solo el `PageCache` LRU en este bloque**. El `Transaction` queda como propuesta independiente con su propio análisis cuando aparezca un workload que lo justifique (ej. INSERT masivo medido).

---

## 2026-05-08 — Decimocuarta intervención: `LeafCursor` (Iterator pattern) — Fase 2 paso 2

> **Sin bump de formato.** El cambio es estructural: cómo se leen los rows del B+Tree.

### ✨ Cambio
- Nuevo `bptree::LeafCursor<'a>` que implementa `Iterator<Item = DbResult<KeyValue>>` y carga páginas leaf **on-demand** vía la chain `next` del B+Tree.
- Constructores en `Tree`: `cursor_full(root)` (full scan en orden de PK) y `cursor_range(root, from, to)` (range scan inclusive en ambos extremos).
- Wrappers en `Catalog`: `scan_cursor(root)` y `range_cursor(root, from, to)` para el caller del SQL layer.
- `exec_select` reescrito: cuando NO hay `ORDER BY`, los planes `FullScan` y `Range` consumen el cursor con `.skip(offset).take(limit)` en vez de materializar todo el B+Tree. Cuando hay `ORDER BY`, sigue materializando (necesita ordenar antes de window).

### 🎯 Impacto medible en recursos
- `SELECT … LIMIT N` sobre tabla de N filas pasa de O(filas_totales) memoria + IO a **O(N + offset)** memoria + IO. Verificable: el test `cursor_limit_returns_only_requested_rows` sobre 1.000 filas valida que `LIMIT 5` devuelve solo 5 PKs en orden, sin intermediarios.
- `SELECT … WHERE pk BETWEEN a AND b LIMIT N` corta el walk apenas la PK supera `b`, sin tocar páginas ulteriores.
- `Plan::ByPks` (path de índice secundario) sigue materializando — está acotado por la cardinalidad del lookup, no por el tamaño de la tabla.

### 🛡️ Borrow semantics (intencionales)
El cursor toma `&mut Pager` por su lifetime. Mientras está vivo, ninguna otra escritura puede pasar por el mismo Pager. Eso es lo correcto para SELECT (read-only) y por eso solo lo usa `exec_select`. Los call sites que necesitan leer Y mutar el mismo B+Tree (`CREATE INDEX` backfill, `INTEGRITY CHECK`, `delete_with_cascade`) siguen usando los helpers materializadores (`scan / range / all`); ahí la materialización es correcta porque la lectura tiene que terminar antes que la escritura empiece.

### 🧪 Validación
- 38/38 tests de integración (1 nuevo: `cursor_limit_returns_only_requested_rows` ejercita LIMIT/OFFSET y BETWEEN sobre 1.000 filas).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Decimotercera intervención: crash tests dirigidos (Fase 1 reabierta y cerrada del todo)

> **Sin bump de formato.** Solo nuevos tests de integración que ejercitan el path WAL→file con escenarios de crash sintéticos.

### 🧪 Crash recovery scenarios cubiertos
Los tests no matan procesos — sintetizan en disco el estado que un `kill -9` dejaría en cada momento crítico del flujo de `Pager::commit`:

1. **`crash_recovery_partial_file_restored_from_wal`** — kill después del WAL flush + COMMIT marker pero antes de tocar el data file. Trunca el data file al header y verifica que el reopen replica las páginas del WAL y el `SELECT` devuelve los datos completos.
2. **`crash_recovery_wal_without_commit_is_ignored`** — kill antes del COMMIT marker (transacción no durable). Forja un WAL con páginas pero sin marker; verifica que el reopen NO replica nada y los datos previos quedan intactos.
3. **`crash_recovery_replay_is_idempotent`** — kill durante los writes al data file con WAL ya flusheado. Re-planta el mismo WAL después de un replay exitoso y verifica que un segundo replay converge al mismo estado (no double-counting, no corrupción).

### 🎯 Cierre definitivo de Fase 1
Esto cubre el ítem "crash tests dirigidos (kill -9 entre WAL y file flush)" que quedaba pendiente en el [ROADMAP](../ROADMAP.md). Fase 1 (Robustez funcional) queda 100% entregada y demostrada con tests reproducibles.

### 🧪 Validación
- 37/37 tests de integración (3 nuevos).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Duodécima intervención: `ORDER BY` (Fase 2 paso 1)

> **Sin bump de formato.** Todo el ordering ocurre en memoria sobre el resultado del scan/range/index path.

### ✨ Funcionalidad SQL
- **`SELECT ... ORDER BY <col> [ASC|DESC]`**. ASC es el default cuando se omite la dirección. Va entre `WHERE` y `LIMIT/OFFSET`.
- Funciona sobre **cualquier columna** del schema (no requiere índice). Reusa el scan/range/index path existente y ordena el resultado en memoria.
- **NULLs sortean primero** bajo ASC (consistente con SQLite). En DESC quedan al final por reverse.
- Comparación tipada: INT/INT, FLOAT/FLOAT, mixto INT↔FLOAT (promueve a f64), BOOL (false<true), TEXT/DATE/DATETIME/JSON por byte order.

### 🧱 Cambios estructurales
- `SelectStmt.order_by: Option<OrderClause>` con `OrderClause { column, direction: OrderDir }`.
- Cuando `order_by` está set, el executor difiere `LIMIT/OFFSET` hasta después del sort para no truncar prematuramente.
- Nuevo helper `compare_values(Option<&Value>, Option<&Value>) -> Ordering` con NULL-first semantics.
- Validación pre-I/O: `ORDER BY` sobre columna inexistente devuelve error explícito.
- Reserved words extendidas: `order`, `by`, `asc`, `desc`.

### 🧪 Validación
- 34/34 tests de integración (4 nuevos: `order_by_int_asc_desc`, `order_by_text_with_limit_offset_window`, `order_by_nulls_sort_first_under_asc`, `order_by_unknown_column_rejected`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Undécima intervención: gabymodeler v2 (PowerDesigner-style) + CORS

> **Sin bump de formato.** El motor no cambia; el modeler reescrito y el server gana headers CORS para que el modeler pueda hablarle directo.

### 🌐 gabymodeler v2 (`web/modeler/`)
Reescritura completa del modelador, espejo del motor `VERSION 6`:
- **Layout PowerDesigner-style**: header de toolbar + Object Browser izquierdo (árbol DB > Tables > columnas con badges PK/NN/UN/FK + sección Indexes) + Canvas central + Result List inferior colapsable + Status bar.
- **Schema editor**: cada columna lleva flags inline `PK / NN / UN / FK` y un input `default` editable. PK fuerza INT + NOT NULL automáticamente. FK abre un mini-modal para elegir tabla, columna PK del target y `ON DELETE RESTRICT|CASCADE`.
- **Check Model** continuo (14 reglas): PK ausente / duplicada / no INT, columna duplicada, identificador inválido o reservado (espejo de `catalog::RESERVED_WORDS`), `NOT NULL + DEFAULT NULL`, `DEFAULT` sobre PK, UNIQUE sobre JSON, FK rota / con type mismatch / target no-PK, etc. Cada hallazgo es clickeable y selecciona la entidad/columna en canvas + browser.
- **SQL Preview en vivo** (sin abrir modal). El emit ordena tablas topológicamente (parents antes que children) y emite todas las constraints inline (`PRIMARY KEY`, `NOT NULL`, `UNIQUE`, `DEFAULT <literal>`, `REFERENCES ... ON DELETE ...`) — DDL fiel al motor `VERSION 6`.
- **↘ Importar de gabysql**: dialog que pide URL del server, token opcional y nombre de DB; consume `GET /tables?db=<db>` y reconstruye entidades + columnas + constraints + FKs desde la respuesta enriquecida del bloque 3. Reverse engineering one-shot.
- **Migración v1 → v2 automática**: si encuentra `gabymodeler.v1` en localStorage, lo lee y produce un `gabymodeler.v2` con las constraints en blanco (los flags se editan a mano).
- **FK lines**: SVG Bezier con marker arrow; `CASCADE` se dibuja sólida, `RESTRICT` punteada.

### 🔓 CORS en `gabysql-server`
- Toda respuesta lleva `Access-Control-Allow-Origin: *`, `Access-Control-Allow-Methods: GET, POST, OPTIONS`, `Access-Control-Allow-Headers: Authorization, Content-Type, X-Gabysql-Token` y `Access-Control-Max-Age: 600`.
- El método `OPTIONS` se contesta con `204 No Content` antes de cualquier auth — los preflights del navegador no llevan credenciales y rechazarlos rompería el modeler en cross-origin.
- También se agregaron `204 No Content` y `503 Service Unavailable` al mapa de status text del response writer.

### 🧪 Validación
- 30/30 tests de integración siguen verdes (no se agregaron tests de modeler — es UI vanilla).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 📋 web/modeler/README.md
Reescrito para el layout v2 y el flujo con reverse engineering.

---

## 2026-05-07 — Décima intervención: `INTEGRITY CHECK` (cierre de Fase 1)

> **Sin bump de formato.** El comando es de solo lectura — no toca el catálogo ni los datos.

### ✨ Funcionalidad SQL
- **`INTEGRITY CHECK;`** — barre la DB abierta y devuelve un ResultSet con una fila por hallazgo. Columnas: `kind`, `object`, `detail`. El campo `message` resume con `OK · N tablas · M filas · K índices · F FKs · P páginas` o `FAIL · ...` según el caso.

### 🔍 Qué chequea
1. **CRC de cada página**: itera de `0..page_count` haciendo `Pager::page_data`. Cualquier falla del CRC se reporta como `kind=page_corrupt`.
2. **Decodificación de cada fila**: `decode_row` corre sobre cada fila de cada tabla. Falla → `kind=row_decode`.
3. **Índices secundarios**: walks every bucket de cada índice y verifica que cada `(value_bytes, pk)` apunte a una PK que efectivamente existe en la tabla. Si no → `kind=orphan_index_entry`.
4. **FOREIGN KEYs**: para cada columna con `references`, verifica que el parent table exista (sino `fk_target_missing`) y que cada valor no nulo de la columna tenga su parent row (sino `fk_orphan`).

### 🧱 Cambios estructurales
- Nuevo `Statement::IntegrityCheck` y método `Engine::exec_integrity_check`.
- Reserved words extendidas: `integrity`, `check`.
- Sin cambios al on-disk format ni al catálogo.

### 🧪 Validación
- 30/30 tests de integración (2 nuevos: `integrity_check_clean_db_returns_ok`, `integrity_check_reports_corrupted_page`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 🎯 Cierre de Fase 1 (Robustez funcional)
Con este bloque, los 5 ítems de Fase 1 del [ROADMAP](../ROADMAP.md) están entregados:
- ~~`UPDATE`/`DELETE` por PK~~
- ~~Checksums por página + WAL~~
- ~~`NOT NULL` / `DEFAULT` / `UNIQUE`~~
- ~~`FOREIGN KEY` + `ON DELETE` enforced~~
- ~~`INTEGRITY CHECK` operacional~~

El motor está listo para empezar a sumar features de Fase 2 (índices compuestos, range scan secundario, `ORDER BY`) o para una primera publicación con SLAs de durabilidad medibles.

---

## 2026-05-07 — Novena intervención: FOREIGN KEY enforced (Camino A · paso 5)

> **On-disk format jump: VERSION 5 → 6.** `Column` ahora persiste un FK opcional `(target_table, target_column, on_delete)`. DBs v5 son rechazadas explícitamente al abrir.

### ✨ Funcionalidad SQL
- **`REFERENCES <table>(<column>) [ON DELETE RESTRICT|CASCADE]`** como constraint de columna en `CREATE TABLE` y `ALTER TABLE ADD COLUMN`. Default `RESTRICT` cuando se omite `ON DELETE`.
- **Validación al DDL**: target table debe existir (o ser self-ref a la tabla siendo creada), target column debe ser la PK del target, tipos deben coincidir (en esta versión ambos son siempre `INT`).
- **Enforcement en `INSERT`**: cada FK no nula chequea que exista la fila parent. Self-FK que apunta al PK que se está insertando se acepta (caso CEO/manager-de-sí-mismo).
- **Enforcement en `UPDATE`**: solo se revalidan FKs cuyo valor cambió.
- **Enforcement en `DELETE`**:
  - `RESTRICT` (default) aborta el DELETE si existe alguna fila hija; sin efectos colaterales.
  - `CASCADE` borra las hijas iterativamente (worklist con `visited` set sobre `(tabla, pk)` para cortar ciclos), incluyendo sus entradas en índices secundarios.
- **Self-references** soportadas (`employee.manager_id REFERENCES employee(id)`).

### 🧱 Cambios estructurales
- `catalog::ForeignKeyMeta { table, column, on_delete: OnDelete }` con `OnDelete::{Restrict, Cascade}`.
- `Column.references: Option<ForeignKeyMeta>` persistido bajo flag `0x04 = HAS_FK`.
- `RESERVED_WORDS` extendido con `foreign`, `references`, `cascade`, `restrict`.
- Helpers nuevos en `sql.rs`: `validate_fk_targets`, `check_fk_value`, `enforce_fk_on_insert`, `enforce_fk_on_update`, `find_child_pks_with_fk_value`, `delete_with_cascade`.
- `find_child_pks_with_fk_value` usa el índice secundario sobre la columna FK si existe; cae en full scan si no — recomendación documentada de indexar columnas FK para DELETEs O(log n).
- `exec_delete` simplificado: chequea existencia y delega en `delete_with_cascade`, que maneja índices secundarios + cascade + cycle protection.

### 🌐 Endpoint `/schema` extendido
Cada columna ahora incluye `references: { table, column, onDelete } | null`:
```json
{
  "name": "parent_id", "type": "INT", "pk": false, "notNull": false, "unique": false,
  "hasDefault": false, "default": null,
  "references": { "table": "parent", "column": "id", "onDelete": "CASCADE" }
}
```

### 🛡️ Restricciones de la versión
- Solo FK de columna única (no compuestas).
- Target debe ser la PK del parent — `REFERENCES` contra `UNIQUE` no-PK no está soportado todavía.
- Solo `RESTRICT` y `CASCADE` (ni `SET NULL`, ni `SET DEFAULT`, ni `NO ACTION`).
- `ALTER TABLE ADD COLUMN ... REFERENCES ...` reusa los mismos guards que UNIQUE: si la columna es `NOT NULL` necesita un `DEFAULT` que apunte a un parent existente, etc.

### 🧪 Validación
- 28/28 tests de integración (6 nuevos: `fk_create_validation_rejects_bad_targets`, `fk_insert_update_enforcement`, `fk_self_reference_allows_pointing_at_self`, `fk_delete_restrict_blocks_when_children_exist`, `fk_delete_cascade_removes_children_and_grandchildren`, `old_v5_db_file_is_rejected_after_v6_bump`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Octava intervención: identificadores duros + introspección completa (Camino A · paso 4)

> **Sin bump de formato.** Los datos en disco no cambian; el cambio es de validación (más estricta) y de contrato JSON (más rico).

### ✨ Identificadores
- Nuevo `catalog::validate_identifier(name, kind)` — única definición de "identificador válido" en el motor: `[A-Za-z_][A-Za-z0-9_]*`, longitud máxima `MAX_IDENT_LEN = 64`, no reservada.
- Lista `catalog::RESERVED_WORDS` con todas las keywords del parser y los nombres de tipo (`int`, `text`, `bool`, `float`, `date`, `datetime`, `json`, etc.).
- Aplicado en `CREATE TABLE` (nombre de tabla + cada columna), `ALTER TABLE ADD COLUMN` (nombre de columna nueva, vía `validate_create_table` sobre meta prospectivo) y `CREATE [UNIQUE] INDEX` (nombre de índice).

### 🌐 Endpoint `/schema` extendido
La respuesta de `GET /schema?db=X&table=Y` (y por tanto también `GET /tables`) ahora incluye lo necesario para reverse-engineering completo desde el frontend:

```json
{
  "ok": true,
  "table": {
    "name": "users",
    "primaryKey": "id",
    "rootPage": 2,
    "columns": [
      { "name": "id",    "type": "INT",  "pk": true,  "notNull": true,  "unique": false, "hasDefault": false, "default": null },
      { "name": "email", "type": "TEXT", "pk": false, "notNull": true,  "unique": true,  "hasDefault": false, "default": null },
      { "name": "status","type": "TEXT", "pk": false, "notNull": true,  "unique": false, "hasDefault": true,  "default": "pending" }
    ],
    "indexes": [
      { "name": "uq_users_email", "column": "email", "rootPage": 4, "unique": true }
    ]
  }
}
```

Campos nuevos por columna: `notNull`, `unique` (derivado de los índices unique de una columna), `hasDefault`, `default` (literal con su tipo nativo en JSON; `null` para "no default" o `DEFAULT NULL`). Campo nuevo por índice: `unique`.

### 🧪 Validación
- 22/22 tests de integración (1 nuevo: `identifier_rules_apply_across_ddl` cubre tabla/columna/índice y los tres rechazos: reservada, longitud, ALTER).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Séptima intervención: edición incremental de schemas (Camino A · paso 3)

> **Sin bump de formato.** El layout `VERSION = 5` ya soporta `TableMeta` con cualquier número de columnas; las filas previas se decodifican con un fallback a `DEFAULT` o `NULL` cuando la fila quedó "corta" frente al esquema nuevo.

### ✨ Funcionalidad SQL
- **`DROP TABLE [IF EXISTS] <name>`** — borra la entrada del catálogo. Las páginas backing (data + índices secundarios) **no** se liberan; el reclaim queda para un futuro `vacuum` (consistente con la política de `DROP INDEX`).
- **`ALTER TABLE <name> ADD [COLUMN] <coldef>`** — agrega una columna al final del esquema. Soporta `NOT NULL`, `DEFAULT`, `UNIQUE`. La keyword `COLUMN` es opcional.

### 🧱 Cambios estructurales
- `decode_row` tolera EOF mientras quedan columnas por decodificar: rellena con el `DEFAULT` de la columna o `NULL`. Permite `ADD COLUMN` sin reescribir filas existentes; el rewrite ocurre naturalmente en el próximo `UPDATE` de cada fila.
- `Catalog::remove_table` borra la entrada del catálogo via `Tree::delete`.
- `parse_column_def` factorizado y compartido entre `CREATE TABLE` y `ALTER TABLE ADD COLUMN`.
- `parse_if_exists` factorizado para `DROP DATABASE` / `DROP TABLE`.

### 🛡️ Restricciones de `ALTER ... ADD COLUMN`
- `PRIMARY KEY` rechazado (la PK ya existe; esta versión no admite swap ni multi-PK).
- `NOT NULL` requiere `DEFAULT` no nulo (sin él, las filas previas violarían la constraint inmediatamente).
- `UNIQUE` con `DEFAULT` no nulo en tabla con > 1 fila se rechaza (produciría duplicados en el backfill).
- `UNIQUE` sin DEFAULT en tabla poblada está OK: filas previas decodifican como `NULL`, y SQL UNIQUE permite múltiples NULLs.
- Nombre de columna duplicado rechazado.
- Validación completa del `coldef` (compatibilidad de tipo del DEFAULT, etc.) reusada del path de `CREATE TABLE`.

### 🧪 Validación
- 21/21 tests de integración (4 nuevos: `drop_table_removes_catalog_entry`, `alter_add_column_decodes_old_rows_with_default_or_null`, `alter_add_column_constraint_guards`, `alter_add_column_unique_then_enforces`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Sexta intervención: constraints declarativas (Camino A · paso 2)

> **On-disk format jump: VERSION 4 → 5.** `Column` ahora persiste `NOT NULL` y `DEFAULT`; `IndexMeta` persiste `unique`. Las DBs creadas con la entrega anterior son rechazadas explícitamente al abrir — re-crear con el binario v5.

### ✨ Funcionalidad SQL
- **`NOT NULL`** como constraint de columna en `CREATE TABLE`. Validado en `INSERT` (columna omitida sin DEFAULT, o `NULL` explícito) y en `UPDATE` (asignación que dejaría la columna en `NULL`). PK es implícitamente `NOT NULL`.
- **`DEFAULT <literal>`** como constraint de columna. Soporta `INT`, `FLOAT`, `BOOL`, `TEXT`/`DATE`/`DATETIME`/`JSON` y `NULL`. La compatibilidad de tipo entre literal y columna se valida en `CREATE TABLE` — `name TEXT DEFAULT 1` se rechaza. PK no admite `DEFAULT`.
- **`UNIQUE`** inline en columna y **`CREATE UNIQUE INDEX`** como sentencia. Inline auto-genera un índice unique con nombre `uq_<tabla>_<columna>`. Múltiples `NULL` se permiten (consistente con SQL estándar). Conflicto de UNIQUE se chequea **antes** de tocar disco — el INSERT/UPDATE falla sin efectos colaterales.
- `CREATE UNIQUE INDEX` sobre tabla con duplicados existentes **aborta el backfill** con error claro; no deja índice colgado.

### 🧱 Cambios estructurales
- `catalog::Column { name, column_type, not_null, default }` con `DefaultLiteral { Null, Integer, Float, Bool, String }` propio del catálogo (no acopla con `sql::Value`).
- `catalog::IndexMeta` lleva `unique: bool`.
- Layout v5 por columna: `[name][type_code:u8][flags:u8][default_payload?]` con `flags & 0x01 = NOT NULL`, `flags & 0x02 = HAS_DEFAULT`.
- Layout v5 por índice: `[name][column][root_page:u32][unique:u8]`.
- Nuevo helper `index::bucket_unique_conflict` y `sql::check_unique_conflict` — un único path de uniqueness para inline UNIQUE y `CREATE UNIQUE INDEX`.
- `sql::ColumnDef` lleva `not_null`, `unique`, `default: Option<Value>` para el AST del parser.

### 🧪 Validación
- 17/17 tests de integración (6 nuevos: `not_null_rejects_missing_and_explicit_null`, `default_fills_missing_and_can_be_overridden`, `default_with_not_null_combination`, `default_type_mismatch_rejected_at_create`, `inline_unique_rejects_duplicates`, `create_unique_index_backfill_aborts_on_duplicates`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-05 — Quinta intervención: DDL de DATABASE + modelador web

### ✨ Funcionalidad SQL
- **`CREATE DATABASE [IF NOT EXISTS] <name>;`** — crea un archivo `.db` en el directorio de `-dir` (server) o junto al path objetivo (CLI).
- **`DROP DATABASE [IF EXISTS] <name>;`** — borra el archivo `.db` y su `.wal` si quedó.
- **`SHOW DATABASES;`** — lista las DBs presentes en el directorio.

Estas sentencias **no se ejecutan contra una `.db` específica** (no operan sobre `TableMeta`). Las despacha el caller — `gabysql-server` para HTTP `/exec` y la CLI `gabysql exec` — antes de abrir el `Pager`. Mezclar DB-level con table-level en un mismo `/exec` se rechaza con error explícito.

### 🌐 Modelador web `gabymodeler`
- Nueva carpeta [`web/modeler/`](web/modeler/) — single-page HTML+CSS+JS vanilla, sin frameworks, sin npm, sin backend acoplado.
- Drag & drop de entidades sobre canvas con grid; SVG para líneas FK Bezier.
- Columnas con tipos (`INT/TEXT/BOOL/FLOAT/DATE/DATETIME/JSON`), flag `PK` (auto-fija `INT`), flag `idx` (índice secundario).
- Botón "↪ FK" para columnas que apuntan a otra entidad — la FK se documenta como comentario en el SQL (las FOREIGN KEY declarativas no se enforced en `VERSION 4`).
- **Exporta SQL** con `CREATE DATABASE [IF NOT EXISTS]` + `CREATE TABLE` + `CREATE INDEX`, copia al clipboard o descarga `.sql`.
- Persiste el modelo en `localStorage` (`gabymodeler.v1`).
- Botón "📦 Cargar ejemplo" trae un schema `users + orders` con FK indexada para evaluar el flujo en 1 click.

### 🧭 Landing `web/index.php` rediseñada
- Reemplaza la tarjeta única de phpgabyadmin por **dos tarjetas lado a lado**: `gabymodeler` y `phpgabyadmin`. Cada una con CTA propio.
- Documenta el flujo recomendado: **modeler → SQL → phpgabyadmin → ejecutar**.

### 🧪 Validación
- 11/11 tests de integración (incluye nuevo `database_level_statements_parse_and_engine_rejects`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.
- `php -l web/index.php` y `php -l web/phpgabyadmin/index.php`: clean.

---

## 2026-05-04 — Cuarta intervención: índices secundarios + scaffolding profesional

> **On-disk format jump: VERSION 3 → 4.** `TableMeta` ahora persiste una lista de índices secundarios; las DBs creadas con la entrega anterior son rechazadas explícitamente al abrir.

### ✨ Funcionalidad SQL
- **Índices secundarios**: `CREATE INDEX <name> ON <table> (<column>);` y `DROP INDEX <name>;`. Soporta backfill automático sobre tablas con datos existentes.
- **`SELECT WHERE col = val` por columna no-PK** consulta el índice cuando existe (lookup O(1) sobre bucket por hash, filtro exacto por bytes, hidratación por PK). Si la columna no es PK ni está indexada, se rechaza con mensaje explícito.
- `WhereClause::Eq` ahora acepta cualquier `Value` (no solo `i64`), por lo que `SELECT WHERE name = 'Ana'` o `WHERE score = 9.5` funcionan igual que `WHERE id = 1`.
- Mantenimiento automático de índices en `INSERT` / `UPDATE` / `DELETE`: el índice solo se actualiza cuando la columna indexada está afectada y el valor cambia.

### 🧱 Cambios estructurales
- Nuevo módulo [`src/index.rs`](src/index.rs): hashing FNV-1a-64, codec de bucket `[count:u16] + N×([vlen:u16][value][pk:i64])`, helpers `bucket_insert/remove/lookup`.
- `TableMeta::indexes: Vec<IndexMeta { name, column, root_page }>` persistido al final del payload del catálogo.
- Reglas de validación: una sola PK INT escalar (sin cambios), una sola columna por índice secundario, `JSON` no es indexable (sin semántica de igualdad canónica).
- `DROP INDEX` no libera páginas — el reclaim queda para una futura herramienta `vacuum`.

### 🛡️ Hardening de CI / supply chain (entrega previa, consolidada en docs)
- 4 workflows: `ci.yml` endurecido, `security.yml`, `workflow-security.yml`, `stale.yml`.
- `cargo audit` 0.22.1 (RustSec), `cargo deny` 0.19.4 (advisories + licenses + bans + sources, regido por [deny.toml](deny.toml)).
- `detect-secrets` (FS + últimos 50 commits), Trojan Source / zero-width / patrones peligrosos Rust+PHP / URLs de exfil.
- `grype` container scan con `--fail-on critical`.
- `actionlint` + `zizmor` + `pin-check` (rechaza acciones sin SHA pin).
- Acciones third-party pinneadas a SHA, `permissions: contents: read` por defecto, `persist-credentials: false`.
- Dependabot semanal: github-actions + cargo + docker.

### 📚 Scaffolding profesional importado desde otros repos del perfil
- `CODE_OF_CONDUCT.md`, `SUPPORT.md`, `COMPATIBILITY.md`, `RECRUITER.md`, `QUICKSTART.md`, `RELEASE.md`.
- `.editorconfig` y `.gitattributes` con normalización LF / CRLF coherente con CI multi-OS.
- `pull_request_template.md` con checklist de fmt/clippy/test/formato-en-disco/supply-chain.

### 🧪 Validación
- 10/10 tests de integración (incluye nuevos: split de B+Tree con 600 filas, detección de corrupción por checksum, rechazo de overwrite, UPDATE/DELETE roundtrip, **índices secundarios end-to-end con backfill + INSERT/UPDATE/DELETE/DROP**).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo audit`, `cargo deny check`: OK.
- `actionlint`, `zizmor`: 0 findings.

### ⚠️ Migración requerida
- DBs creadas con `VERSION = 3` no son legibles. Re-crear con `gabysql init <file.db>`. Mensaje de error explícito al abrir.

---

## 2026-05-03 — Tercera intervención: cierre de hallazgos críticos del MVP

> **On-disk format jump: VERSION 1 → 3.** Toda DB creada antes de esta entrega es rechazada explícitamente al abrir. Recrearla con la versión actual (`gabysql init <file.db>`).

### 🧱 Cambios estructurales del motor
- **B+Tree real**: el índice por PK pasó de una lista enlazada de hojas a un B+Tree con nodos internos. Lookup descendente en O(log N), `root_page` permanece estable cruzando splits gracias a copy-up del root.
- **Hash del catálogo determinista**: las claves del catálogo de tablas se calculaban con `DefaultHasher` (no estable entre versiones de Rust). Reemplazado por FNV-1a-64 inline en código.
- **Checksums CRC32-IEEE**: cada página persiste un trailer de 4 bytes con su CRC. El Pager lo finaliza antes de flushear y verifica al leer y al replay del WAL. La corrupción ahora produce error explícito en vez de silencio.
- **`Pager::create` no destructivo**: rehúsa sobrescribir un archivo existente. Se introdujo `create_force` para el camino explícito de reset (`gabysql init --force <file.db>`).
- **`page_size` honesto**: el header valida que `page_size == PAGE_SIZE_DEFAULT`; el campo se mantiene en disco para una futura revisión del formato.

### ✨ Funcionalidad SQL
- `UPDATE <tabla> SET col = val[, ...] WHERE <pk> = N;` (no permite cambiar la PK).
- `DELETE FROM <tabla> WHERE <pk> = N;` (error si la PK no existe).
- Mensajes de error de PK más explícitos sobre la limitación INT-only de esta versión.

### 🛡️ Endurecimiento del modo server
- `gabysql-server` aplica un techo de conexiones concurrentes (default 64, configurable con `-max-connections N`). Conexiones extra reciben 503 y se cierran sin generar threads.

### 🧪 Validación
- 9/9 tests de integración (incluye nuevos: split de B+Tree con 600 filas, detección de corrupción por checksum, rechazo de overwrite, UPDATE/DELETE roundtrip).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`: OK.

### ⚠️ Migración requerida
- Bases de datos creadas con versiones anteriores a esta entrega no son legibles. El error es explícito (`unsupported gabysql file format: version=...`). Re-crear con el binario actual.

---

## 2026-03-19 — Segunda intervención: migración completa a Rust y estabilización base

### 🧱 Estado actual del sistema
- Motor embebido en Rust con archivo único `.db`
- CLI `gabysql` para `init`, `info`, `exec` y `repl`
- Server HTTP `gabysql-server` para operar una base única o un directorio de bases
- `phpgabyadmin` consumiendo la API HTTP como consola web liviana
- Docker y `docker compose` para levantar server y admin web en un entorno reproducible

### 🏗️ Cambios estructurales
- Se eliminó la implementación anterior en Go y se reemplazó por un proyecto Rust con `Cargo`
- Se separó el core en módulos de storage, catálogo, SQL, servidor y estructura persistente por clave primaria
- Se unificó la documentación para reflejar solo las capacidades reales del motor actual

### ✨ Mejoras funcionales
- Soporte de `CREATE TABLE`, `INSERT` y `SELECT` con full scan, `LIMIT/OFFSET`, `WHERE <pk> = ...` y `BETWEEN`
- Soporte de tipos `INT`, `TEXT`, `BOOL`, `FLOAT`, `DATE`, `DATETIME`, `JSON` y `NULL` en columnas no PK
- Rechazo explícito de claves primarias duplicadas en vez de sobrescritura silenciosa
- Recovery WAL por marcador `COMMIT` para rehidratar páginas confirmadas tras reinicio

### 🛡️ Estabilidad y seguridad
- El parser SQL ahora devuelve errores controlados en escenarios inválidos en lugar de derribar el proceso
- Se corrigió el manejo de comillas escapadas dentro de strings SQL para soportar textos complejos en inserciones multi-sentencia
- `phpgabyadmin` quedó endurecido con cookie firmada y bloqueo de servidores remotos salvo habilitación explícita
- La UI web y el README quedaron alineados con el comportamiento real del motor

### 🎨 Documentación y lenguaje visual
- Se creó un set documental completo alineado con el estándar usado en otros repos del perfil
- Se añadieron guías de instalación, uso, operación, seguridad, troubleshooting y contribución
- Se añadió documentación técnica de arquitectura, requisitos, API y especificaciones del motor
- Se aplicó una capa visual consistente con badges, bloques de estado, tablas de navegación y rutas por perfil

### ✅ Validación y entrega continua
- Se agregaron pruebas de integración para roundtrip básico, PK duplicada, paginación con `LIMIT/OFFSET`, `NULL`, parser inválido y recovery WAL
- Se agregó CI en GitHub Actions con `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` y lint de PHP
- La matriz de CI cubre `ubuntu-latest`, `windows-latest` y `macos-latest`, más build Docker en Linux
- La CI publica artefactos `release` por sistema operativo para facilitar distribución nativa multiplataforma
- El `Dockerfile` valida `cargo test --all-targets` antes de construir binarios release
- `docker compose` permite probar juntos `gabysql-server` y `phpgabyadmin`

### 🧪 Validación realizada en esta intervención
- `cargo fmt --check`: OK
- `cargo check --tests`: OK
- `cargo clippy --all-targets -- -D warnings`: OK
- `docker build -t gabysql .`: OK
- `docker compose up -d --build`: OK
- `GET http://localhost:8080/health`: OK
- `GET http://localhost:8000`: OK

### ⚠️ Límites actuales conocidos (al cierre de la 2ª intervención)
- El índice persistente sigue siendo una estructura de hojas enlazadas por PK `INT`; no es todavía un B+Tree multinivel completo *(superado en la 3ª intervención: ver entrada superior)*
- No hay optimizer cost-based ni estadísticas de consulta
- No hay concurrencia avanzada, MVCC ni transacciones complejas
- Sigue siendo un producto base estable para evolucionar, no un reemplazo directo de motores maduros como PostgreSQL o MySQL
