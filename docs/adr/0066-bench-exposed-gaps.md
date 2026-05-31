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

## Gap 1 — Agregados sobre SELECT con JOIN ✓ CERRADO (F2, 2026-05-30)

**Código de error original**: `[GBY-4028]` (lifteado para el path JOIN — ahora se emite solo desde `COUNT(DISTINCT col)` sobre JOIN, deferred).
**Query del bench**: `microblog` → `SELECT u.nombre, COUNT(*) FROM users u JOIN posts p ON p.user_id = u.id WHERE u.id = 7 GROUP BY u.nombre`

**Causa raíz** (pre-fix): `exec_aggregate_pipeline` solo operaba sobre el pipeline single-table. El pipeline JOIN (nested-loop / hash-join / index-loop) entregaba filas joineadas, pero la fase de agregación no estaba conectada a ese stream — solo a `exec_select` single-table.

**Fix aplicado**:
1. Nuevo `exec_aggregate_joined` (`src/sql.rs:~11580`): bucketea las filas que produce el JOIN por claves cualificadas (`alias.col`), reusa `compute_aggregate` con `AggArg::Expr` rewriting (pre-resolución de columnas vía `rewrite_expr_columns_for_join`), HAVING (3VL con `eval_where_expr_single` sobre el `having_row` que tiene aggregates indexados por output_name + canonical), DISTINCT, ORDER BY, LIMIT/OFFSET. ~250 líneas.
2. Dispatch: `exec_select_joined` (`src/sql.rs:~9450`) chequea `stmt_needs_aggregation` después del WHERE y delega.
3. `resolve_joined_projection` ya no rechaza `SelectItem::Aggregate` — el dispatch lo desvía antes.

**Limitación conocida**: `COUNT(DISTINCT col)` sobre JOIN sigue rebotando con `[GBY-4028]` — el path DISTINCT en `compute_aggregate` usa `normalize_ident(col)` que tira el qualifier. Defer al bloque que generalice DISTINCT.

**Tests**: `f2_aggregate_over_join_count_inner`, `f2_aggregate_over_join_sum_expr_left_join`, `f2_aggregate_over_join_group_by_right_table`, `f2_count_star_over_view` (todos en `tests/integration_test.rs`).

**Prioridad**: ~~P1~~ — entregado.

---

## Gap 2 — `BETWEEN` sin índice ordenado ✓ CERRADO (F3, 2026-05-30)

**Código de error original**: `[GBY-4002]` (lifteado — ahora reservado).
**Query del bench**: `orders_lines` → `SELECT order_id FROM lines WHERE qty BETWEEN 1 AND 5`

**Causa raíz** (pre-fix): `exec_select_with_where` rebotaba `BETWEEN` cuando la columna no era PK ni tenía `IndexKind::OrderedInt`. La semántica original asumía range scan exclusivo; no contemplaba el fallback a full-scan + filter post-scan que sí existía para `=`/`<`/`>`.

**Fix aplicado (commit del bloque F3)**:
1. Planner (`src/sql.rs:8777`): rama `else` (col sin índice) deja de devolver `Err(BETWEEN_REQUIRES_PK_OR_INT_INDEX)` y cae a `Plan::FullScan` — mismo path que `=`. Hash-idx también cae a FullScan (antes erroreaba; hoy generic_post_filter lo cubre).
2. Force-rule en `generic_post_filter` (`src/sql.rs:8657`): BETWEEN exige post-filter salvo cuando hay fast-path (simple-PK Range plan o índice OrderedInt). El evaluador `eval_where_expr_single` ya soportaba BETWEEN; no se tocó.
3. Bench: la query `BETWEEN qty 1..5` mide en vez de hacer SKIP.
4. Tests: `where_between_on_non_indexed_column_full_scans` (nuevo) y `where_between_on_text_indexed_column_full_scans` (renombrado de `_is_rejected`, asserts actualizados a la nueva semántica).

**Resultado**: BETWEEN ahora se comporta como `=`, `>`, `<`, `LIKE` — fast-path indexado cuando aplica, FullScan + post-filter en cualquier otro caso. Sin pérdida de performance en el path indexado; type mismatch (INT BETWEEN sobre TEXT col) devuelve 0 filas en vez de error, consistente con la 3VL del resto del WHERE.

**Prioridad**: ~~P1~~ — entregado.

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

## Gap 7 — `COUNT(*) FROM <view>` ✓ CERRADO (sub-caso de F2, 2026-05-30)

**Código de error original**: `[GBY-4028]` (mismo que Gap 1).
**Query del bench**: `SELECT COUNT(*) FROM heavy_edges` donde `heavy_edges` es VIEW.

**Causa raíz** (pre-fix): la expansión de vista (V) genera un derived source; los agregados sobre derived caían en el mismo path que JOIN+agg.

**Fix aplicado**: la expansión de vista convierte el FROM a `derived_source`. Eso dispatch a `exec_select_joined` (línea 8544) — ahora ese path tiene aggregator gracias a F2. **Sin código adicional**.

**Test**: `f2_count_star_over_view`.

**Prioridad**: ~~P1~~ — entregado junto con Gap 1.

---

## Gap 8 — `RANK()` y `SUM() OVER (PARTITION BY)` cuadráticos ✓ CERRADO (W4, 2026-05-30)

**Causa raíz** (pre-fix): `compute_window_value` iteraba, por cada fila del partition, recorriendo TODO el partition (0..pos) para calcular rank/sum acumulado — O(n²) por partition.

**Fix aplicado**: nueva función `fill_window_partition_into` (`src/sql.rs:~20115`) que rellena el resultado de toda una partition en O(n):
- RANK / DENSE_RANK: walk lineal con `order_by_equal` adjacente (1 sola comparación por fila vs N).
- SUM / AVG OVER con ORDER BY: prefix sum + count running. Sin ORDER BY: una pasada y todas las filas reciben el mismo agregado.
- COUNT(expr) OVER: pre-eval por fila + prefix count de no-nulls.
- MIN / MAX OVER: running prefix con comparación incremental. Sin ORDER BY: una pasada.
- RowNumber / Ntile / Lag / Lead / FirstValue / LastValue: ya eran O(1) per row, delegados al evaluador per-row clásico.

**Tests**:
- `w4_rank_sum_over_2k_rows_is_linear`: 2000 filas con RANK+SUM OVER terminan en <0.5s (antes serían minutos).
- `w4_rank_dense_rank_matches_expected_values`: verificación de corrección con ties.
- `w4_sum_over_running_prefix`: SUM OVER con prefix progresivo (10, 30, 60, …).

**Bench**: iters de RANK/SUM OVER vuelve de 2 → 5 (antes bajado por O(n²)).

**Prioridad**: ~~**P1 crítico**~~ — entregado.

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
| 1 | ~~`[GBY-4028]`~~ | ~~F2~~ ✓ | ~~P1~~ cerrado 2026-05-30 |
| 2 | ~~`[GBY-4002]`~~ | ~~F3~~ ✓ | ~~P1~~ cerrado 2026-05-30 |
| 3 | parser | **E5** | P2 |
| 4 | `[GBY-4067]` | **K3** | P2 |
| 5 | `[GBY-4081]` | dependencia de E5 | P2 |
| 6 | `[GBY-3001]` (bench) | **N5** (DEFAULT con función) | P2 |
| 7 | ~~`[GBY-4028]`~~ (vista) | ~~F2~~ ✓ (sub-caso de Gap 1) | ~~P1~~ cerrado 2026-05-30 |
| 8 | ~~sin código~~ | ~~**W4**~~ ✓ | ~~**P1 crítico**~~ cerrado 2026-05-30 |
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
