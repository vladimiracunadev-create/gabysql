# ADR-0076: R8 — composite-eq fast-path en UPDATE/DELETE (asimetría P5b)

**Fecha:** 2026-06-11
**Estado:** Aceptado
**Bloque:** R8 (reparación post-P5)
**Origen:** [docs/ANALISIS_POST_P5.md §3 R8](../ANALISIS_POST_P5.md) — asimetría P5b
**Reusa:** [ADR-0069](0069-p5b-composite-index-lookup.md) helpers `find_matching_composite_index` + `composite_index_lookup_pks`

## Contexto

P5b (ADR-0069) entregó composite secondary index lookup *solo para
SELECT*. El path `exec_select_with_where` gana el fast-path, pero
`resolve_target_pks` (que usan `exec_update` y `exec_delete`) seguía
con FullScan + 3VL como único path para WHERE compuesto.

Asimetría observable:

```sql
-- Rápido (P5b):
SELECT id FROM lines WHERE qty = 5 AND precio = 100;
-- 8.7 µs sobre 100k filas (bench M2)

-- Lento (pre-R8):
UPDATE lines SET val = 99 WHERE qty = 5 AND precio = 100;
-- ~150 ms sobre 100k filas (FullScan completo)
```

El motor "sabe" lookup-ear el composite index pero no usaba esa
información en mutaciones. R8 cierra la asimetría.

Bonus: el mismo path añade composite-PK fast-path para UPDATE/DELETE
— pre-R8 también caía a FullScan cuando el WHERE era AND-eq sobre
todas las cols de una PK compuesta.

## Decisión

### Dos fast-paths nuevos en `resolve_target_pks`

Insertados entre el PK-single fast-path (existente) y el FullScan
fallback (existente):

```rust
// 1. Composite PK fast-path
if meta.has_composite_pk() {
    if let Some(map) = extract_and_equality_map(where_clause) {
        if map.len() == meta.pk_columns().len() {
            // Compute fingerprint, verify row exists, return single-element Vec.
            ...
        }
    }
}

// 2. Composite secondary index fast-path (reusa helpers P5b)
if let Some(map) = extract_and_equality_map(where_clause) {
    if let Some((idx, fp)) = find_matching_composite_index(meta, &map) {
        let candidate_pks = composite_index_lookup_pks(self.pager, idx.root_page, fp)?;
        // Post-filter via eval_where_expr_single para descartar colisiones FNV
        // + aplicar predicados extra (igual que P5b).
        ...
    }
}
```

### Por qué el post-filter es crítico

El bucket del índice compuesto solo guarda PKs (sin valores). Una
colisión FNV-1a-64 — astronómicamente rara pero posible — devolvería
una PK que no satisface el WHERE. Sin post-filter, UPDATE/DELETE
operaría sobre la fila equivocada.

Caso aún más probable: predicates extra (`WHERE qty=5 AND precio=100
AND sku='A'` — composite cubre qty+precio pero no sku). El post-filter
descarta filas que no matchean sku.

Mismo razonamiento que P5b (ADR-0069) — el post-filter es
**load-bearing para correctness**.

### Materializar antes del eval

`eval_where_expr_single` requiere `&mut self`, pero
`Catalog::open(self.pager)` mantiene el borrow del pager. Resolución:
recolectar todas las `(pk, decoded_row)` tuplas en un `Vec<>` mientras
el catalog vive, después soltarlo y iterar para eval:

```rust
let candidate_rows: Vec<(i64, HashMap<String, Value>)> = {
    let mut catalog = Catalog::open(self.pager);
    let mut rows = Vec::with_capacity(candidate_pks.len());
    for pk in candidate_pks {
        if let Some(bytes) = catalog.get_row(meta.root_page, pk)? {
            rows.push((pk, decode_row(meta, &bytes)?));
        }
    }
    rows
};
// catalog dropped — eval_where_expr_single puede borrowear self
```

Costo: `candidate_pks.len()` clones de HashMap. Aceptable: el composite
index ya redujo el conjunto candidato; típicamente 1-10 filas, no
miles.

### Composite PK con 0 matches

Si el fp computado no existe en el B+tree principal, devolvemos
`Ok((vec![], false))` — NO `was_explicit_single_pk=true`. Razón
semántica: el WHERE original era compuesto (`a=1 AND b=2`), no `pk=N`.
El caller no debe emitir `ROW_NOT_FOUND_FOR_PK` (esa semántica es
para el pre-E3 caso de `pk = literal`).

UPDATE/DELETE con 0 matches devuelven `rows_affected = 0` — consistente
con SQL estándar.

## Alternativas consideradas

1. **Refactorizar `exec_select_with_where` y `resolve_target_pks` a
   compartir un único `compute_target_pks` con todos los fast-paths**.
   - Considerado. Descartado por scope creep — `exec_select_with_where`
     tiene 6+ fast-paths más (composite PK plan, composite index plan,
     P5c skip, Range, exists_postfilter, etc.) que `resolve_target_pks`
     no necesita. La duplicación parcial es controlada (4 fast-paths
     vs 6+); refactor mayor para 2 ramas no compensa.

2. **Aplicar P5c (skip-index si est.match alta) también aquí**.
   - Diferido a R6. Este push trata únicamente la asimetría P5b →
     R8. Mezclar ambas cosas oscurece el commit y los tests.

3. **Solo composite secondary, no composite PK**.
   - Descartado por simetría. Si exec_select_with_where tiene
     composite_pk_plan, exec_update / exec_delete deberían tenerlo
     también. El código adicional es pequeño y reusa el mismo helper
     `encode_composite_key`.

4. **Pasar el `WhereExpr` enviado al `eval_where_expr_single` ya
   resuelto** (post-RLS) en lugar del original.
   - El where_clause que `resolve_target_pks` recibe YA tiene el
     RLS inyectado por `exec_update` / `exec_delete` arriba (ver
     comentarios a `build_rls_where`). No hay nada que pasar — el
     callsite ya hace lo correcto.

## Tests

5 tests nuevos en `tests/integration_test.rs` (suite `r8_*`):

- `r8_update_composite_index_lookup`: UPDATE con WHERE composite-eq
  sobre composite index — solo las filas matched se actualizan.
- `r8_delete_composite_index_lookup`: DELETE análogo.
- `r8_update_composite_pk_lookup`: UPDATE sobre PK compuesta con
  WHERE AND-eq completo — fast-path nuevo.
- `r8_delete_composite_pk_lookup`: DELETE análogo.
- `r8_update_extra_predicate_post_filter`: WHERE composite-eq + sku='A'
  → fast-path acota por composite, post-filter aplica sku — solo
  filas con sku correcto se actualizan.

Suite total: **794 passing** (789 → +5 R8). Verificado vía Docker
`rust:1.94-bookworm`.

Hallazgo del proceso: tests inicialmente usaban `ORDER BY a, b` y
fallaron con `token inesperado: ,` — el motor no soporta multi-col
`ORDER BY` (limitación previa, no documentada en este ADR). Tests
ajustados para ordenar en Rust después del SELECT.

## Consecuencias

**Positivas**

- (+) UPDATE/DELETE con composite-eq pasa de O(N) a O(log N) — la
  misma ganancia ~18 500× que P5b consiguió para SELECT.
- (+) Asimetría P5b cerrada. Las tres operaciones (SELECT/UPDATE/DELETE)
  usan los mismos fast-paths.
- (+) Composite PK fast-path agregado de yapa — UPDATE/DELETE sobre
  PK compuesta también gana ~O(log N).
- (+) Correctness: el post-filter idéntico al de P5b — colisiones FNV
  y predicates extra se manejan igual.

**Negativas / Limitaciones honestas**

- (-) **R6 no aplicado**. Si el composite no-UNIQUE tiene un fp con
  60% de las PKs, UPDATE/DELETE va a iterar todas — más caro que
  FullScan + 3VL. Mismo problema que con SELECT pre-R6.
- (-) Trigger semantics: UPDATE dispara `BEFORE/AFTER UPDATE`
  triggers. La fast-path no cambia ese dispatch — los triggers se
  ejecutan correctamente. Pero NO probé tests específicos de
  trigger en R8; queda como deuda residual a confirmar.
- (-) Sin EXPLAIN para UPDATE/DELETE. El usuario no ve si el
  fast-path se activó. EXPLAIN del SELECT correspondiente sí lo
  muestra (P5b/P5e); el de UPDATE/DELETE no. Diferido.
- (-) RLS interactúa con el fast-path: el WHERE ya tiene el RLS
  inyectado cuando `resolve_target_pks` lo recibe. Si el RLS agrega
  predicates extra, `extract_and_equality_map` puede no extraerlos
  (RLS usa `Expr` complejo, no Eq). El composite fast-path NO se
  activa en presencia de RLS → cae al FullScan. Es comportamiento
  conservador correcto, pero el usuario pierde la ganancia perf
  cuando hay RLS activo.

## Limitaciones / Trabajo futuro

- **R6**: extender P5c a composite indexes — alta selectividad sobre
  composite también debería caer a FullScan.
- **EXPLAIN para UPDATE/DELETE**: anotar la decisión de plan
  (composite_pk / composite_index / FullScan).
- **Single-col index fast-path para UPDATE/DELETE**: hoy `WHERE
  col = X` con índice secundario single-col cae a FullScan en
  UPDATE/DELETE. Extender es trivial (reusa `lookup_pks_via_index`).
- **Trigger interaction tests**: confirmar que BEFORE/AFTER UPDATE
  con el fast-path nuevo se comporta igual que con el FullScan.
