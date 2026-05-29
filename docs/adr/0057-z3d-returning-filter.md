# ADR-0057: RETURNING filtrado contra SELECT policies (Z3d)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z3d (follow-up de Z3c — cierre del RLS para RETURNING)
**Bump on-disk**: **ninguno** (semantic enforcement)

## 🧭 Contexto

Z3/Z3b/Z3c entregaron RLS para SELECT (read-side), INSERT WITH CHECK, UPDATE post-image. Quedaba un caso de leak: el RETURNING. Un user con INSERT permission y una policy WITH CHECK permisiva podía hacer `INSERT INTO t VALUES (...) RETURNING *` y leer columnas que su policy SELECT no le permitiría — porque el RETURNING devuelve el row recién mutado sin pasar por el filtro SELECT.

Z3d cierra ese hueco: el `affected_rows` de INSERT/UPDATE/DELETE pasa por un filtro adicional contra policies SELECT antes de proyectarlo via `project_returning`.

## 💡 Decisión

### 1. Nuevo helper `filter_rows_by_select_policies`

```rust
fn filter_rows_by_select_policies(
    &mut self,
    table: &str,
    rows: Vec<HashMap<String, Value>>,
) -> DbResult<Vec<HashMap<String, Value>>>
```

Reglas:
- `current_user is None` (superuser) → bypass, devuelve `rows` sin filtrar.
- Tabla sin policies → bypass (compat 100%).
- Filtra policies aplicables a `(POLICY_ACTION_SELECT|ALL, role match)`.
- Si no hay aplicables → devuelve `Vec::new()` (deny). El user verá un RETURNING vacío aunque la mutación haya pasado.
- PERMISSIVE OR: una fila aparece si **al menos una** policy aplicable evalúa USING como TRUE.

### 2. Hook en 3 sitios

```
exec_insert  → affected_rows → filter_rows_by_select_policies → project_returning
exec_update  → affected_rows → filter_rows_by_select_policies → project_returning
exec_delete  → affected_rows → filter_rows_by_select_policies → project_returning
```

El filtrado es **post-mutación, pre-proyección**. Importante: la mutación ya sucedió (INSERT persistió, UPDATE persistió, DELETE persistió). Sólo se filtra qué *se devuelve al caller*. Es information-hiding, no atomicidad.

### 3. Semántica de "row invisible"

Cuando un row se filtra del RETURNING, **el caller no se entera** — no hay error, sólo recibe menos filas que las que se mutaron. Esto es consistente con PostgreSQL RLS: la visibility filtering es silenciosa para evitar leaks por canales laterales (e.g. "esa fila existe pero no te la muestro" leakearía la existencia).

### 4. No requiere bump on-disk

Z3d es enforcement **semántico** sobre el formato existente. Reusa `PolicyMeta` y `list_policies_for_table`. No bump VERSION.

## 📁 Archivos tocados

- `src/sql.rs`:
  - Helper nuevo: `filter_rows_by_select_policies` (~60 LOC).
  - 3 sitios donde `project_returning` se llama: `exec_insert` (~6759), `exec_update` (~9818), `exec_delete` (~10708). Antes de la projection, se inserta el filter call.
- `tests/integration_test.rs`: 7 tests `z3d_*`.

## ⛔ Lo que **no** entra en Z3d (defer)

| Ítem | Razón del defer |
|---|---|
| `INSERT ... ON CONFLICT DO UPDATE` con RETURNING filtrado por SELECT del UPDATE path | El path interno de upsert tiene su propia rama de `apply_insert_row_with_conflict`. Ya filtra en el path normal de INSERT vía Z3d; el UPDATE path interno también heredó el filter por compartir `affected_rows`. Verificación end-to-end con upsert + policies queda en Z3e. |
| Filter contra policies WITH CHECK además de USING | Hoy sólo SELECT USING aplica al filtro. Si una policy tiene WITH CHECK distinto de USING, el RETURNING sigue siendo gobernado por USING (lo que el user "ve"). Comportamiento consistente con PG. |
| Statement-level rollback cuando RETURNING queda vacío por filter | Hoy la mutación pasa aunque el RETURNING quede vacío. PG hace lo mismo — RLS no aborta el statement, sólo filtra el output. |
| Column-level filter en RETURNING (e.g. expone solo algunas cols) | Hoy es all-or-nothing por fila. Column-level necesita policy más rica. Defer indefinido. |

## 🧪 Tests

7 tests `z3d_*`:
- `z3d_insert_returning_visible_row` — RETURNING devuelve la fila si la policy SELECT la cubre.
- `z3d_insert_returning_invisible_row_filtered` — WITH CHECK permite, SELECT filtra → RETURNING vacío.
- `z3d_update_returning_filters_invisible_rows` — UPDATE RETURNING respeta la policy SELECT.
- `z3d_delete_returning_filters` — DELETE RETURNING respeta la policy SELECT.
- `z3d_returning_no_policies_compat` — sin policies en la tabla, RETURNING devuelve todo (compat).
- `z3d_returning_superuser_bypass` — sin SET SESSION AUTHORIZATION, RETURNING devuelve todo.
- `z3d_returning_filtered_to_empty_when_no_select_policy_match` — INSERT pasa (WITH CHECK TRUE), pero la SELECT policy es TO bob; alice ve RETURNING vacío.

Suite total: **681 passing** (674 → +7 Z3d).

## 🔗 Referencias

- PostgreSQL RLS — RETURNING + RLS interaction (§5.8).
- ADR-0054 (Z3b): WITH CHECK que motiva el leak que Z3d cierra.
- ADR-0055 (Z3c): post-image enforcement que es paralelo conceptualmente a este filter.
- ADR-0052 (Z3): foundation del WHERE rewriting que Z3d aplica row-a-row sobre affected_rows.
