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

## Gap 3 — `SELECT (subquery)` sin `FROM` ✓ CERRADO (E5, 2026-05-30)

**Causa raíz** (pre-fix): el parser de SELECT siempre buscaba FROM tras la lista de columnas (`expect_keyword("FROM")` en `parse_select_stmt_inner`).

**Fix aplicado**:
- Parser (`src/sql.rs:~23427`): tras `parse_select_list`, si el siguiente token no es `FROM`, sale por el camino "bare-SELECT" — construye un `SelectStmt` con `table: ""` (sentinel) y solo permite `ORDER BY` / `LIMIT` / `OFFSET` trailing.
- Engine (`exec_bare_select` en `src/sql.rs:~8491`): detecta el sentinel `table.is_empty()`, evalúa cada `SelectItem` contra una fila vacía con `eval_expr_full`, devuelve UNA fila. `*`, `Aggregate` y `Window` rechazados (sin row scope no tienen sentido).

**Tests**: `e5_bare_select_literal`, `e5_bare_select_multi_with_aliases`, `e5_bare_select_scalar_subquery`, `e5_bare_select_with_limit_offset`.

**Prioridad**: ~~P2~~ — entregado.

---

## Gap 4 — `UNIQUE` multi-columna con tipos no-INT ✓ CERRADO (K3, 2026-05-30)

**Causa raíz** (pre-fix): los validadores de UNIQUE multi-col y CREATE INDEX composite en `src/sql.rs` exigían `column_type == Int`. Al inspeccionar el encoder, `encode_column_value` ya manejaba TEXT (y todos los tipos que `stores_as_text` cubre), y la per-column FNV en `encode_composite_key` ya separaba columnas con un sentinel — la limitación era cosmética en la validación upfront, no estructural.

**Fix aplicado**:
- Nuevo helper `validate_composite_member(column, ctx, require_not_null)` en `src/sql.rs:~11901`: rechaza JSON / BLOB / DECIMAL (sin equality fingerprint o sin encoder), acepta INT, FLOAT, BOOL, TEXT, DATE, DATETIME, TIME, UUID. `require_not_null = true` para UNIQUE en CREATE TABLE (preservando comportamiento pre-K3), `false` para CREATE INDEX (preservando el lazy NOT NULL check que el runtime hace en `encode_composite_key`).
- Reemplaza los 3 sitios pre-K3 (UNIQUE table-level, named UNIQUE, CREATE INDEX composite).
- PK compuesta queda intacta — su validador vive en `src/catalog.rs` y exige all-INT NOT NULL. Defer a un bloque futuro si aparece presión.

**Bench**: `parent_cz` ahora declara `CONSTRAINT u_code_region UNIQUE (code, region)` con ambas TEXT — antes era single-col TEXT por workaround.

**Tests**:
- `k3_unique_composite_text_text_works`: UNIQUE (TEXT, TEXT) NOT NULL acepta INSERTs distintos y rebota duplicados con `[GBY-3003]`.
- `k3_unique_composite_text_nullable_rejected`: nullable sigue siendo error.
- `k3_create_index_composite_text_int_works`: CREATE INDEX mixto TEXT+INT funciona.
- `k3_create_index_composite_blob_rejected`: BLOB sigue rechazado.

**Prioridad**: ~~P2~~ — entregado.

---

## Gap 5 — `WITH RECURSIVE` exige `FROM` en el anchor ✓ CERRADO (sub-caso de E5, 2026-05-30)

**Causa raíz** (pre-fix): misma que Gap 3 — el parser no aceptaba bare-SELECT.

**Fix aplicado**: el cambio en `parse_select_stmt_inner` (E5) cubre también el anchor de `WITH RECURSIVE` porque WITH usa el mismo parser de SELECT. Sin código adicional.

**Workaround del bench (todavía vigente, no obligatorio)**: `setup_graph` sigue creando `seed_one (n INT PK, depth INT)` con un row `(1, 0)` — la query del bench podría reescribirse a `WITH RECURSIVE r AS (SELECT 1 AS n, 0 AS depth UNION ALL ...)`. Defer la reescritura del bench a una iteración futura.

**Prioridad**: ~~P2~~ — entregado vía E5.

---

## Gap 6 — Trigger `BEFORE`/`AFTER` con `NEW.id` como PK del audit table ✓ CERRADO parcialmente (N5, 2026-05-30)

**Causa raíz**: era principalmente un bug del bench (audit PK colisionando con id source). El gap del motor era la falta de DEFAULT evaluado por función — UUID / now() no se podían usar en `DEFAULT` aunque ambos existían como funciones escalares.

**Fix aplicado (N5)**:
- Parser (`parse_default_expr` en `src/sql.rs:~25329`): tras `DEFAULT`, si el siguiente token es `ident(`, lo trata como llamada a función pura. Whitelist actual: `gen_random_uuid` / `uuid_v4` / `uuid_generate_v4` / `random_uuid`, `uuid_v7` / `uuid_generate_v7` / `gen_uuid_v7`, `current_timestamp` / `now`. Codifica como `Value::String("__GBY_DEFAULT_FN__<canonical>__")` — sin bump de `VERSION` on-disk: piggyback sobre el variant String del `DefaultLiteral` existente.
- Eval (`evaluate_default_string` en `src/sql.rs:~13735`): `default_to_value` ahora detecta el prefijo reservado y dispatch a `gen_uuid_v4()` / `gen_uuid_v7()` / `now_datetime_utc()`. Strings que no matchean el prefijo siguen como literales TEXT.

**Limitación residual**: `SERIAL` / `AUTOINCREMENT` (contador persistido) NO se entrega en N5 — requiere bump de `VERSION` para persistir el counter por columna. La solución actual cubre el caso de audit log via UUID, que es el caso del ADR original.

**Tests**:
- `n5_default_gen_random_uuid`: tres INSERTs generan tres UUIDs distintos de 36 chars.
- `n5_default_current_timestamp`: produce un string con shape `YYYY-MM-DD...`.
- `n5_default_function_unknown_rejected`: función fuera del whitelist → error.
- `n5_default_literal_string_still_works`: regresión — literales TEXT siguen intactos.

**Convención reservada**: el prefijo `__GBY_DEFAULT_FN__` queda reservado en el namespace de strings de gabysql; un usuario que persista un literal con ese prefijo se topará con la re-evaluación.

**Prioridad**: ~~P2~~ — entregado (vía N5).

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

## Gap 9 — `PRIMARY KEY` compuesta partial scan no usa índice ✓ CERRADO (K4, 2026-05-30)

**Causa raíz** (pre-fix): el fingerprint FNV-1a-64 exige TODAS las columnas de la PK para el lookup O(log n). Lookup parcial caía a full-scan (137 ms para 5 rows sobre 100k).

**Fix aplicado**: en vez de replantear el on-disk format del fingerprint, se auto-crea un índice single-col `OrderedInt` sobre la primera columna de la PK cuando hay PK compuesta. Estrategia equivalente al "left-most column match" de MySQL InnoDB. El planner existente (`exec_select_with_where` line ~8757) ya usa secondary indexes; el cambio es solo materializar el índice en `CREATE TABLE`.

Detalles:
- `src/sql.rs:~2739`: tras la materialización de UNIQUE indexes, si `meta.has_composite_pk()` y no hay un single-col idx pre-existente sobre `meta.primary_key`, se crea `_pk_prefix_<table>` (nombre reservado con prefijo `_`) como `IndexKind::OrderedInt`. PK compuesta sigue siendo all-INT NOT NULL — el tipo está garantizado.
- El INSERT/UPDATE/DELETE no necesita cambios: ya iteran sobre `meta.indexes` para mantener todos los secondary indexes.
- Skip si el usuario ya declaró un single-col idx sobre pk1 (UNIQUE inline, table-level, o CREATE INDEX manual).

**Latencia**: lookup parcial sobre 50k rows pasa de full-scan a O(log n) — test `k4_composite_pk_partial_lookup_uses_index` corre en <500ms (debug) cuando antes hubiera tomado ~70ms (release).

**Tests**:
- `k4_composite_pk_partial_lookup_uses_index`: lookup parcial sobre 50k filas en perf budget.
- `k4_composite_pk_auto_index_visible_in_meta`: nombre reservado discoverable.
- `k4_no_auto_index_for_simple_pk`: PK simple NO genera auto-index (innecesario).

**Prioridad**: ~~P2~~ — entregado.

---

## Gap 10 — `Composite index lookup` no usa el índice compuesto ✓ CERRADO (P5b, 2026-06-11)

**Código de error original**: ninguno — funcionaba, pero hacía full-scan.
**Query del bench**: `orders_lines` → `SELECT order_id FROM lines WHERE qty = 5 AND precio = 100` con `CREATE INDEX idx_lines_qty_precio ON lines (qty, precio)`.
**Latencia medida pre-fix**: **161 ms** para 100k rows.

**Causa raíz** (pre-fix): el planner identificaba el composite index al insertar pero el path de READ no lo usaba — el WHERE multi-atom AND caía a `FullScan + post-filter`. `extract_and_equality_map` producía el map correcto pero el dispatch solo lo usaba para PK compuesta, no para índices secundarios compuestos.

**Fix aplicado** (commit del bloque P5b, ver [ADR-0069](0069-p5b-composite-index-lookup.md)):

1. Nuevo `find_matching_composite_index(meta, eq_map) -> Option<(&IndexMeta, i64)>` (`src/sql.rs:~15001`): si hay índice compuesto cuyas TODAS las columnas están cubiertas por el AND-eq, devuelve el índice y el fingerprint FNV-1a-64. Multiple candidates → pick el más largo (más selectivo en ausencia de stats).
2. Nuevo `composite_index_lookup_pks(pager, idx_root, fp)`: lee el bucket en `fp`, devuelve `Vec<i64>` de PKs.
3. Dispatch en `exec_select_with_where` (`src/sql.rs:~9220`): nuevo `composite_index_plan` después de `composite_pk_plan` y antes del fallback a `Plan::FullScan` por post-filter. `generic_post_filter` permanece activo como red de seguridad (collisions + extra predicates).
4. `classify_scan` (EXPLAIN): nueva rama que detecta el caso y emite `composite index lookup ‘<idx_name>’ (col1, col2, ...) (B+tree fingerprint, ~O(log n))`.

**Limitación residual**: solo full-cover (todas las columnas del índice cubiertas). Prefix matching (`WHERE a=X` con índice `(a, b)`) sigue cayendo a FullScan — requiere cambiar el layout on-disk del índice a tuple-byte-concatenado con orden lexicográfico (potencial P5c).

**Tests**: 6 nuevos en `tests/integration_test.rs` (suite p5b_*). Suite total 762 passing.

**Prioridad**: ~~P1~~ — entregado.

---

## Catálogo de prioridades

| Gap | Código | Bloque | Prioridad |
|---|---|---|---:|
| 1 | ~~`[GBY-4028]`~~ | ~~F2~~ ✓ | ~~P1~~ cerrado 2026-05-30 |
| 2 | ~~`[GBY-4002]`~~ | ~~F3~~ ✓ | ~~P1~~ cerrado 2026-05-30 |
| 3 | ~~parser~~ | ~~**E5**~~ ✓ | ~~P2~~ cerrado 2026-05-30 |
| 4 | ~~`[GBY-4067]`~~ | ~~**K3**~~ ✓ | ~~P2~~ cerrado 2026-05-30 |
| 5 | ~~`[GBY-4081]`~~ | ~~dependencia de E5~~ ✓ | ~~P2~~ cerrado 2026-05-30 |
| 6 | ~~`[GBY-3001]` (bench)~~ | ~~**N5** (DEFAULT con función)~~ ✓ | ~~P2~~ cerrado 2026-05-30 |
| 7 | ~~`[GBY-4028]`~~ (vista) | ~~F2~~ ✓ (sub-caso de Gap 1) | ~~P1~~ cerrado 2026-05-30 |
| 8 | ~~sin código~~ | ~~**W4**~~ ✓ | ~~**P1 crítico**~~ cerrado 2026-05-30 |
| 9 | ~~sin código~~ | ~~**K4**~~ ✓ | ~~P2~~ cerrado 2026-05-30 |
| 10 | ~~sin código~~ | ~~**P5b**~~ ✓ | ~~P1~~ cerrado 2026-06-11 |

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
