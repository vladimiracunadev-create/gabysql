# ADR-0059: ON CONFLICT DO UPDATE con WITH CHECK del UPDATE path (Z3e)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z3e (follow-up de Z3c/Z3d — cierre del WITH CHECK en el upsert path)
**Bump on-disk**: **ninguno** (semantic enforcement)

## 🧭 Contexto

Z3b habilitó INSERT WITH CHECK; Z3c agregó UPDATE post-image WITH CHECK; Z3d filtró RETURNING contra SELECT policies. Quedaba un caso de leak: `INSERT ... ON CONFLICT (col) DO UPDATE SET ...` — el upsert path cae internamente al UPDATE pero no llama `exec_update`. Lo escribe el método `apply_insert_row_with_conflict` que llama `apply_update_to_pk` directamente. Sin Z3e, este path no aplicaba WITH CHECK del UPDATE — un user podía hacer:

```sql
-- Policy: UPDATE ON t WITH CHECK (owner = 'alice')
SET SESSION AUTHORIZATION 'alice';
INSERT INTO t (id, owner) VALUES (1, 'alice')
  ON CONFLICT (id) DO UPDATE SET owner = 'bob';
-- Antes Z3e: pasaba (sin chequear WITH CHECK del UPDATE)
-- Z3e: rebota con [GBY-4138]
```

## 💡 Decisión

### 1. Hook nuevo en `apply_insert_row_with_conflict`, branch `OnConflictAction::DoUpdate`

Por cada `pk` en `conflict_pks`, **antes** de `apply_update_to_pk`:

1. Si `current_user.is_some()`, recargar `old_row` del catálogo via `get_row` + `decode_row`.
2. Construir `new_map = old_row.clone()` y aplicar cada `(col, expr)` de `expr_assignments` evaluando `expr` contra `old_row` con `eval_expr_full`.
3. Llamar `self.enforce_with_check(meta.name, POLICY_ACTION_UPDATE, &new_map)`.
4. Si rebota → propagar `[GBY-4138]` y el INSERT entero falla.

### 2. Sin statement-level rollback

Mismo contrato que Z3c: si el INSERT está en un batch multi-row y la fila K viola WITH CHECK, las K-1 anteriores ya fueron persistidas. Para atomicidad fuerte, el caller envuelve en `BEGIN ... COMMIT`.

### 3. Reusa `enforce_with_check` y la semántica de Z3b/Z3c

Misma OR PERMISSIVE + fallback USING-as-WITH-CHECK para policies sin WITH CHECK explícito. Sin policies en tabla → bypass. Superuser → bypass.

### 4. Sin bump on-disk

Z3e es enforcement semántico sobre el formato existente. Reusa `PolicyMeta` tal cual.

## 📁 Archivos tocados

- `src/sql.rs`: ~25 LOC añadidas en el bucle `for pk in &conflict_pks` dentro de `apply_insert_row_with_conflict`, branch `OnConflictAction::DoUpdate`.
- `tests/integration_test.rs`: 5 tests `z3e_*`.

## ⛔ Lo que **no** entra en Z3e

| Ítem | Razón del defer |
|---|---|
| **Statement-level rollback en RLS violation** | Requiere savepoint primitive (Bloque T). Defer Z3f. |
| **Column-level filter en RETURNING** | Hoy es all-or-nothing por fila. Column-level necesita policy más rica (defer indefinido). |
| `INSERT ... ON CONFLICT DO NOTHING` con check de SELECT policy sobre la fila existente | Hoy DoNothing no chequea nada; un user podría detectar la existencia de filas que no podría SELECTear. Defer Z3f. |

## 🧪 Tests

5 tests `z3e_*`:
- `z3e_upsert_passing_with_check` — upsert que satisface WITH CHECK pasa.
- `z3e_upsert_violating_with_check_rejected` — el UPDATE path del upsert que viola WITH CHECK rebota con 4138.
- `z3e_upsert_with_check_fallback_to_using` — policy UPDATE sin WITH CHECK explícito reusa USING (PG semantics).
- `z3e_upsert_superuser_bypass` — superuser bypasea el check en el upsert path.
- `z3e_upsert_no_policies_compat` — sin policies → upsert pasa sin chequear (compat 100%).

Suite total: **690 passing** (685 → +5 Z3e).

## 🔗 Referencias

- PostgreSQL RLS — `INSERT ... ON CONFLICT DO UPDATE` interaction con policies.
- ADR-0054 (Z3b): WITH CHECK foundation.
- ADR-0055 (Z3c): UPDATE post-image que este ADR extiende al upsert path.
- ADR-0057 (Z3d): RETURNING filter complementario.
