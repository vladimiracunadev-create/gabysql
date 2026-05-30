# ADR-0066: Gaps del motor expuestos por el benchmark (2026-05-30)

**Fecha:** 2026-05-30
**Estado:** Aceptado (catálogo de pendientes con prioridad asignada)
**Bloque:** análisis / no-implementación (cada gap defer a su propio bloque)

## Contexto

El `gabybench` extendido a 10 DBs (2026-05-30) corrió queries que tropezaron contra limitaciones REALES del motor. **No son bugs del bench** — son features no implementadas que el bench expuso al intentar usar superficies SQL razonables.

Este ADR documenta cada gap UNA SOLA VEZ con:
- código de error,
- query del bench que lo dispara,
- por qué falla (causa raíz en el motor),
- workaround vigente en el bench,
- bloque/prioridad para fix definitivo.

**Objetivo**: que la próxima vez que el bench arroje uno de estos errores, no haya que volver a investigarlo. Es un catálogo cerrado, agregable cuando aparezcan nuevos.

---

## Gap 1 — Agregados sobre SELECT con JOIN

**Código de error**: `[GBY-4028]`
**Query del bench**: `microblog` → `SELECT u.nombre, COUNT(*) FROM users u JOIN posts p ON p.user_id = u.id WHERE u.id = 7 GROUP BY u.nombre`
**Mensaje**: `agregados (COUNT/SUM/AVG/MIN/MAX) sobre SELECT con JOIN aún no se soportan; reescribir como subquery agregada sobre la tabla base`

**Causa raíz**: `exec_aggregate` solo opera sobre el pipeline single-table. El pipeline JOIN (nested-loop o index-loop) entrega filas joineadas, pero la fase de agregación no está conectada a ese stream — solo a `exec_select_single`.

**Workaround en bench**: `bench_sql_or_skip` → la query queda registrada como SKIP, suite continúa.

**Fix definitivo**: **bloque F2** — extender `exec_aggregate` para consumir el row stream que devuelve `exec_join`. Diseño:
1. Reordenar el pipeline: JOIN → filter → aggregator (hoy: scan → filter → aggregator).
2. El aggregator no necesita cambios; solo el dispatcher de exec_select que detecta agg+JOIN.
3. Tests: F2-1 (COUNT con INNER), F2-2 (SUM(qty*price) con LEFT JOIN), F2-3 (GROUP BY columna de la tabla derecha).

**Prioridad**: P1 (común en cualquier aplicación SQL real).

---

## Gap 2 — `BETWEEN` sin índice ordenado

**Código de error**: `[GBY-4002]`
**Query del bench**: `orders_lines` → `SELECT order_id FROM lines WHERE qty BETWEEN 1 AND 5`
**Mensaje**: `WHERE BETWEEN solo soporta PK (order_id) o columnas INT con índice; 'qty' no califica`

**Causa raíz**: `exec_select_with_where` rebota `BETWEEN` cuando la columna no es PK ni tiene `IndexKind::OrderedInt`. La semántica original asumía que BETWEEN siempre quiere range scan; no contempla el fallback a full-scan + filter post-scan que sí existe para `=`/`<`/`>`.

**Workaround en bench**: SKIP graceful.

**Fix definitivo**: **bloque F3** — agregar fallback full-scan + post-filter para `BETWEEN` cuando no hay índice ordenado. Es ~5 líneas en `classify_scan` / `exec_select_with_where`. Trivial. Solo no se hizo porque no hubo presión hasta este bench.

**Prioridad**: P1.

---

## Gap 3 — `SELECT (subquery)` sin `FROM`

**Código de error**: parser, `se esperaba keyword FROM`
**Query del bench**: `events` → `SELECT (SELECT COUNT(*) FROM events WHERE kind = 'view')`
**Mensaje**: parser de `SELECT` exige `FROM` siempre.

**Causa raíz**: gabysql parser de SELECT siempre busca FROM tras la lista de columnas. PostgreSQL/MySQL aceptan `SELECT 1` o `SELECT (subquery)` sin FROM como queries de retorno-único.

**Workaround en bench**: SKIP graceful.

**Fix definitivo**: **bloque E5** — extender el parser para aceptar bare-SELECT (sin FROM) y el engine para devolver una sola fila virtual. Cambio: ~10 líneas parser + ~5 líneas engine.

**Prioridad**: P2 (rara en apps clásicas, común en ORMs y herramientas de admin).

---

## Gap 4 — `UNIQUE` multi-columna con tipos no-INT

**Código de error**: `[GBY-4067]`
**Query del bench (original, ahora fixeada)**: `constraint_zoo` → `CONSTRAINT u_parent_code UNIQUE (code, region)` con `code TEXT, region TEXT`
**Mensaje**: `CONSTRAINT '...' UNIQUE (code, region): todas las columnas deben ser INT NOT NULL (columna 'code' rompe la regla)`

**Causa raíz**: K2 (composite PK/index, ADR-0019) usa fingerprint FNV-1a-64 sobre 8 bytes por columna i64. Solo todo-INT NOT NULL. UNIQUE multi-col TEXT requeriría un fingerprint sobre bytes UTF-8 variables.

**Workaround en bench**: cambié a `UNIQUE` single-col TEXT (que sí funciona). Documentado en el código del bench como TODO.

**Fix definitivo**: **bloque K3** — extender fingerprint composite a soportar TEXT (hash FNV sobre UTF-8 bytes con length-prefix). Cambio: ~30 líneas en `Catalog::compute_fingerprint` + tests.

**Prioridad**: P2.

---

## Gap 5 — `WITH RECURSIVE` exige `FROM` en el anchor

**Código de error**: `[GBY-4081]`
**Query del bench (original)**: `WITH RECURSIVE reach AS (SELECT 1 AS n, 0 AS depth UNION ALL SELECT e.dst, r.depth+1 FROM reach r JOIN edges e ON ...) SELECT * FROM reach`
**Mensaje**: el parser no acepta `SELECT 1 AS n, 0 AS depth` como anchor (le pide FROM).

**Causa raíz**: misma que Gap 3 (parser no acepta bare-SELECT). En WITH RECURSIVE el anchor suele ser un SELECT chico de valores literales; en gabysql hay que materializar una tabla seed.

**Workaround en bench**: setup_graph crea tabla `seed_one (n INT PK, depth INT)` con un row `(1, 0)` y el anchor hace `SELECT n, depth FROM seed_one`.

**Fix definitivo**: depende de **E5** (bare-SELECT). Una vez E5 esté, este gap desaparece sin código adicional.

**Prioridad**: P2 (workaround viable; common en literatura SQL pero no bloqueante).

---

## Gap 6 — Trigger `BEFORE`/`AFTER` con `NEW.id` como PK del audit table

**Código de error**: `[GBY-3001]` PRIMARY KEY duplicada
**Query del bench (original)**: trigger `AFTER UPDATE ON accounts FOR EACH ROW BEGIN INSERT INTO audit_log (id, ...) VALUES (NEW.id, ...) END` + UPDATE iterando sobre `id = (i % 100) + 1`
**Mensaje**: `PRIMARY KEY duplicada: la clave 1 ya existe en la tabla`

**Causa raíz**: **no es un gap del motor — es un bug del bench**. Pero lo dejo documentado porque es la trampa más fácil de caer cuando se diseñan triggers reales: si el audit log usa una columna source como PK, queda obligado a unicidad. La solución real-world es usar una secuencia / autoincrement / UUID.

**Workaround en bench**: UPDATE con IDs únicos `(i + 1) as i64` rango 1..200 → cada trigger fire genera un audit_log con PK único.

**Fix futuro**: **bloque N5** — agregar `SERIAL` / `AUTOINCREMENT` o `gen_random_uuid()` como DEFAULT viable en columnas PK. Hoy hay `gen_random_uuid()` (Y5) pero el motor no lo evalúa como DEFAULT — solo como expresión SQL escalar.

**Prioridad**: P2.

---

## Gap 7 — `COUNT(*) FROM <view>`

**Código de error**: `[GBY-4028]` (mismo que Gap 1)
**Query del bench (original)**: `SELECT COUNT(*) FROM heavy_edges` donde `heavy_edges` es VIEW.
**Mensaje**: el motor expande la vista a su source SQL inline, que es un SELECT con WHERE; al envolver con COUNT(*) cae al case de "agg sobre SELECT no-base".

**Causa raíz**: la expansión de vista (V) genera un derived-table-like wrapper; los agregados sobre ese wrapper caen al mismo case que JOIN+agg.

**Workaround en bench**: cambié `COUNT(*) FROM heavy_edges` por `SELECT id, src, dst, weight FROM heavy_edges LIMIT 100` — sin agregar, sí funciona.

**Fix definitivo**: arreglar como sub-caso de **F2** (Gap 1). Cuando F2 esté, esto se arregla solo porque el wrapper view se convertirá en un node más del pipeline.

**Prioridad**: P1 (junto con F2).

---

## Gap 8 — `RANK()` y `SUM() OVER (PARTITION BY)` cuadráticos

**Código de error**: ninguno — la query funciona, pero **toma 44-60 segundos para 500 filas**.
**Query del bench**: `analytics` → `SELECT region, revenue, RANK() OVER (PARTITION BY region ORDER BY revenue DESC) FROM sales LIMIT 500` y la análoga con `SUM OVER`.

**Causa raíz**: `compute_window_value` en W3 itera, por cada fila del partition, recorriendo TODO el partition para calcular el rank/sum acumulado. Es O(n²) por partition.

**Lo que SÍ funciona OK (lineal)**:
- `ROW_NUMBER() OVER` → 270 ms / 500 rows
- `LAG()` / `LEAD()` → 229 ms / 500 rows
- `FIRST_VALUE` / `LAST_VALUE` (no medido pero misma estructura)

**Workaround en bench**: bajé `iters` de 5 → 2 para RANK y SUM OVER (las 2 cuadráticas) para que la suite no tarde 4+ min en esas dos solas. La queries siguen midiendo correctamente p50.

**Fix definitivo**: **bloque W4** — refactor `compute_window_value` a O(n log n):
- Sort por partition key + order key una sola vez.
- RANK/DENSE_RANK: walk lineal con comparación a la fila anterior.
- SUM OVER cumulativo: prefix sum O(n) por partition.

Cambio sustantivo (~150 líneas + tests). Bloque deferred.

**Prioridad**: P1 (es un hallazgo crítico — hace inviable usar window functions en producción).

---

## Gap 9 — `PRIMARY KEY` compuesta partial scan no usa índice

**Código de error**: ninguno — funciona, pero hace full-scan.
**Query del bench**: `orders_lines` → `SELECT * FROM lines WHERE order_id = X` (sin line_no).
**Latencia medida**: **137 ms** para encontrar 5 rows sobre 100k.

**Causa raíz**: K2 fingerprint exige TODAS las columnas de la PK para el lookup O(log n). Lookup parcial cae a full-scan.

**Fix definitivo**: **bloque K4** — implementar range scan sobre prefix de PK compuesta. Equivalente a "left-most column index match" de MySQL. Requiere replantear el fingerprint para que sea prefix-friendly (e.g. concatenar cada columna como [u32 width][bytes]).

**Prioridad**: P2.

---

## Gap 10 — `Composite index lookup` no usa el índice compuesto

**Código de error**: ninguno — funciona, pero hace full-scan.
**Query del bench**: `orders_lines` → `SELECT order_id FROM lines WHERE qty = 5 AND precio = 100` con `CREATE INDEX idx_lines_qty_precio ON lines (qty, precio)`.
**Latencia medida**: **161 ms** para 100k rows (devuelve TODAS las rows — el predicado no filtra realmente, pero el planner debería usar el índice).

**Causa raíz**: el planner identifica el composite index pero `WHERE col1 = X AND col2 = Y` no se detecta como "AND-composable" — el dispatcher no compone los dos predicados en una fingerprint lookup.

**Fix definitivo**: **bloque P5b** — planner con detección de `WHERE composable AND` → composite index lookup. Sub-tarea del bloque P5 (planner real).

**Prioridad**: P1 (cuando llegue P5).

---

## Catálogo de prioridades

| Gap | Código | Bloque | Prioridad |
|---|---|---|---:|
| 1 | `[GBY-4028]` | **F2** | P1 |
| 2 | `[GBY-4002]` | **F3** | P1 |
| 3 | parser | **E5** | P2 |
| 4 | `[GBY-4067]` | **K3** | P2 |
| 5 | `[GBY-4081]` | dependencia de E5 | P2 |
| 6 | `[GBY-3001]` (bench) | **N5** (DEFAULT con función) | P2 |
| 7 | `[GBY-4028]` (vista) | F2 (resuelve al cerrar Gap 1) | P1 |
| 8 | sin código | **W4** | **P1 crítico** |
| 9 | sin código | **K4** | P2 |
| 10 | sin código | **P5b** | P1 (cuando llegue P5) |

---

## Próxima vez que el bench falle

1. **No reinventar diagnóstico**: revisar primero si el error está en esta tabla.
2. **Si está**: el SKIP/workaround ya está en el bench. No hay que hacer nada — la suite avanza.
3. **Si no está**: investigar, agregar entrada nueva acá, aplicar workaround en el bench, mantener la suite verde.

El bench tiene `bench_sql_or_skip` para los gaps documentados — no debe abortar nunca por una limitación conocida.

---

## Limitación honesta

- Estos 10 gaps NO son exhaustivos del motor. Son los que el bench tropezó al cubrir las 10 DBs sintéticas.
- Otros gaps reales del motor (sin querer escribirlos todos) están en [docs/MISSING_COMMANDS.md](../MISSING_COMMANDS.md): `SAVEPOINT`, `PREPARE`/`EXECUTE`, bind params, `COPY FROM/TO`, ARRAY/JSONB, cursores explícitos, etc.
- Si en una futura iteración del bench se agregan queries nuevas que toquen gaps NUEVOS, hay que agregarlos a este catálogo antes de aplicar workaround.
