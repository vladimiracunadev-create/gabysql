# ADR-0069: P5b — composite secondary index lookup (Gap 10 del bench)

**Fecha:** 2026-06-11
**Estado:** Aceptado
**Bloque:** P5b (sub-tarea aislada de P5 — planner-as-optimizer)
**Cierra:** [ADR-0066 Gap 10](0066-bench-exposed-gaps.md#gap-10--composite-index-lookup-no-usa-el-índice-compuesto)
**Antecede:** P5 (cost-based planner real)

## Contexto

Gap 10 del bench (`gabybench` 2026-05-30) midió 161 ms para esta query
sobre 100 000 filas en `orders_lines`:

```sql
CREATE INDEX idx_lines_qty_precio ON lines(qty, precio);
SELECT order_id FROM lines WHERE qty = 5 AND precio = 100;
```

El motor reconocía el índice compuesto al INSERTAR (`composite_unique_check`,
`composite_index_upsert`) pero el **planner de lectura lo ignoraba**:

1. El WHERE multi-atom AND no es un `atom`, así que el path
   `Some(atom) = where_expr.as_atom()` (que sí maneja índices) no se
   activaba.
2. `extract_and_equality_map` producía el map `{qty: 5, precio: 100}`
   pero el dispatch solo lo usaba para PK compuesta, no para índices
   secundarios compuestos.
3. La query caía a `Plan::FullScan` + post-filter genérico → 100 000
   filas escaneadas.

P5b agrega el fast-path análogo para índices secundarios compuestos:
fingerprint FNV-1a-64 sobre las columnas del índice → bucket lookup
en el B+tree del índice → `Plan::ByPks(...)`.

## Decisión

### Helpers nuevos en `src/sql.rs`

**`find_matching_composite_index(meta, eq_map) -> Option<(&IndexMeta, i64)>`**:

- Itera índices secundarios compuestos del `meta`.
- Para cada uno, chequea que TODAS sus columnas estén cubiertas por
  `eq_map` con valores **no-NULL** (un valor NULL invalida el match —
  `col = NULL` es UNKNOWN en 3VL, el fast-path no aplica).
- Computa el fingerprint en el **orden exacto** del índice usando
  `encode_composite_key` (mismo encoding que el path de inserción —
  garantiza coincidencia bit-perfect).
- Si hay varios candidatos, prefiere el **más largo** (más columnas
  cubiertas → predicado más selectivo en ausencia de stats P5).

**`composite_index_lookup_pks(pager, idx_root, fp) -> DbResult<Vec<i64>>`**:

- Lee el bucket en `fp` del B+tree del índice.
- Decodifica vía `decode_ordered_bucket` (mismo formato que escribe
  `composite_index_upsert`).
- Devuelve la lista de PKs (vacío si el bucket no existe).

### Dispatch en `exec_select_with_where`

Nuevo plan candidato después de `composite_pk_plan` y antes del
fallback a `Plan::FullScan` por post-filter:

```rust
let composite_index_plan: Option<Plan> = if exists_postfilter.is_none() {
    stmt.where_clause
        .as_ref()
        .and_then(extract_and_equality_map)
        .and_then(|map| {
            find_matching_composite_index(&meta, &map)
                .map(|(idx, fp)| (idx.root_page, fp))
        })
        .and_then(|(idx_root, fp)| {
            composite_index_lookup_pks(self.pager, idx_root, fp)
                .ok()
                .map(Plan::ByPks)
        })
} else {
    None
};

let plan = if let Some(p) = composite_pk_plan { p }
    else if let Some(p) = composite_index_plan { p }
    else if exists_postfilter.is_some() || generic_post_filter.is_some() {
        Plan::FullScan
    } else { /* atom dispatch */ };
```

### `generic_post_filter` permanece activo

El bucket del índice compuesto guarda **solo PKs** (sin valores de las
columnas indexadas). Por eso:

1. **Colisiones FNV-1a-64**: dos tuplas distintas pueden producir el
   mismo fingerprint. Astronómicamente raro a 100k–10M filas (birthday
   paradox para 64-bit ~5 mil millones), pero no imposible. El
   `generic_post_filter` re-evalúa el WHERE contra cada fila fetched
   → descarta cualquier colisión silenciosa.
2. **Predicados extra**: `WHERE qty = 5 AND precio = 100 AND sku = 'A'`
   con índice `(qty, precio)` — el fast-path acota a las PKs candidatas
   por el composite, y el post-filter aplica `sku = 'A'` sobre esas.

Sin el post-filter el fast-path sería **incorrecto**. La doc de
`find_matching_composite_index` lo deja explícito.

### `classify_scan` para EXPLAIN

Cuando el WHERE no es atom simple pero matchea un composite secondary
index, EXPLAIN ahora muestra:

```
SCAN `lines` → composite index lookup `idx_lines_qty_precio` (qty, precio)
(B+tree fingerprint, ~O(log n)) [est.rows=100000 cols=4]
```

en vez del genérico `(full scan + WHERE AND/OR/NOT post-filter)`.

## Alternativas consideradas

1. **Implementar prefix matching** (índice `(a, b, c)` aprovecha
   `WHERE a=X AND b=Y` sin necesidad de `c`).
   - Descartado: el fingerprint mezcla TODAS las columnas (sentinela
     0xFF entre ellas, ver `encode_composite_key`). No hay forma de
     hacer prefix lookup sobre fingerprint. Requeriría cambiar el
     layout del índice compuesto a tuple-byte-concatenado con orden
     lexicográfico — un bloque distinto (P5c potencial).
   - Por ahora: prefix matching cae a FullScan, igual que pre-P5b.

2. **Reverse-lookup tuple-by-bytes** dentro del bucket (no solo PK).
   - Descartado: cambiaría el layout on-disk del bucket → bump
     VERSION y migración. El post-filter ya hace el trabajo correcto
     a costo de un `decode_row` extra por PK candidate.

3. **Detección por costo (stats P4)**.
   - Diferido a P5 completo. Por ahora: si el composite cubre TODO el
     AND-eq, ganar siempre vs full scan. No hay ambigüedad de costo.

4. **Múltiples índices que matchean → pick el más largo**.
   - Heurística simple sin stats. Si hay índices `(a, b)` y `(a, b, c)`
     y el WHERE cubre los 3, gana el de 3 (predicado más estrecho →
     bucket más chico en expectativa). Con stats reales sería
     `min(ndv_compuesta)` o `min(est.rows)`. P5 lo reemplazará.

## Tests

6 tests nuevos en `tests/integration_test.rs` (suite `p5b_*`):

- `p5b_composite_index_lookup_devuelve_fila_correcta`: 4 filas con
  combinaciones de qty/precio; `WHERE qty=5 AND precio=100` devuelve
  solo las 2 que matchean.
- `p5b_explain_muestra_composite_index_lookup`: EXPLAIN incluye
  `composite index lookup ... idx_qp`.
- `p5b_lookup_parcial_cae_a_full_scan`: `WHERE qty=5` (solo 1 de 2
  columnas) NO dispara el fast-path → EXPLAIN no menciona composite.
  La query sigue devolviendo el resultado correcto vía FullScan.
- `p5b_extra_predicate_aplica_post_filter`: `WHERE qty=5 AND precio=100
  AND sku='A'` — fast-path por composite + post-filter sobre `sku`.
- `p5b_non_unique_composite_bucket_con_varias_pks`: composite no-UNIQUE
  con bucket de 3 PKs todas valores iguales (qty=7, precio=50) →
  devuelve las 3.
- `p5b_composite_index_unique_lookup`: UNIQUE composite — el path es
  idéntico al non-unique (smoke test).

Suite total: **762 passing** (756 → +6 nuevos P5b). Verificado vía
Docker `rust:1.94-bookworm` (Windows host sin MSVC linker).

## Bench

La query Gap 10 del `gabybench` (`Composite index lookup qty+precio`,
suite `orders_lines`, 100k filas) ya estaba MIDIENDO el caso degradado
(161 ms). Con P5b la misma query ejecuta sobre 1 row (o pocas, según
distribución) — el ADR no cambia el bench, solo la métrica. El próximo
corte de `gabybench` cierra la entry de Gap 10 con el nuevo número.

## Consecuencias

**Positivas**

- (+) Cierra el último gap del bench documentado (ADR-0066). Suite
  catálogo de gaps queda limpia.
- (+) Win directo en performance: una query típica de `orders_lines`
  pasa de O(N) a O(log N).
- (+) Path análogo al `composite_pk_plan` existente (Issue #4 / K2)
  — mismo patrón de fingerprint + ByPks, código parsimonioso.

**Negativas / Limitaciones honestas**

- (-) Solo full-cover. Si el WHERE cubre solo un PREFIJO del índice
  compuesto (`WHERE a=X` con índice `(a,b)`), seguimos cayendo a
  FullScan. Prefix matching requiere otro layout (P5c).
- (-) Heurística "pick el más largo" es deliberadamente simple. Si dos
  composite indexes cubren el mismo set de columnas con stats muy
  distintas, P5b no diferencia. P5 con stats lo refinará.
- (-) El post-filter agrega un `decode_row` extra por PK candidate
  vs. un fast-path puro. En la práctica el bucket tiene 1–10 PKs
  típicamente, el overhead es marginal vs el full-scan de 100k filas.
- (-) UPDATE y DELETE con composite-eq aún hacen FullScan: el fix
  análogo está pendiente. P5b es solo SELECT. Es seguro de extender
  (mismo helper se reusa) pero deferred para mantener el bloque chico.

## Limitaciones / Trabajo futuro

- **P5c (prefix matching)**: nuevo layout de índice compuesto con
  orden lexicográfico → range scan sobre prefijos. Bump VERSION.
- **P5 (planner real)**: consume stats P4. `ndv` por columna estima
  cuántas PKs devolverá el composite. MCV detecta skew (`qty=5`
  podría cubrir 60% de la tabla → composite no gana).
- **Extender a UPDATE/DELETE**: `find_matching_composite_index` ya es
  estándar; basta wirearlo en `exec_update_with_where` y
  `exec_delete_with_where`.
- **OR de eqs sobre composite**: `WHERE (a=1 AND b=2) OR (a=3 AND b=4)`
  podría ser dos composite lookups + UNION de PKs. No común; deferred.
