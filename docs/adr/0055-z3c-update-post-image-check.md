# ADR-0055: UPDATE post-image WITH CHECK enforcement (Z3c)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z3c (follow-up de Z3b — cierre del WITH CHECK para UPDATE)
**Bump on-disk**: **ninguno** (semantic enforcement sólo)

## 🧭 Contexto

Z3 (ADR-0052) entregó RLS read-side para UPDATE — el `where_clause` se rewrite-ea con `OR de USING` para gatear qué filas se tocan. Z3b (ADR-0054) cubrió INSERT con `WITH CHECK`. Faltaba el tercer caso documentado de Postgres RLS: el **post-image check de UPDATE** — verificar que la fila resultante (post-assignments) siga dentro del scope de las policies aplicables.

Sin Z3c, un user con UPDATE permission podría hacer `UPDATE t SET owner = 'bob' WHERE owner = 'alice'` y "regalarse" filas a otro user, escapando del scope inicial. Z3c cierra ese hueco.

## 💡 Decisión

### 1. Hook en `exec_update` antes de `apply_update_to_pk`

Para cada fila a actualizar, si `current_user.is_some()`:
1. Tomar el `old_row` (ya hidratado si `needs_old_snapshot`, o recargarlo del catálogo).
2. Construir el `post_row` = `old_row` con `assignments` aplicados.
3. Llamar `enforce_with_check(table, POLICY_ACTION_UPDATE, &post_row)`.
4. Si rebota → `[GBY-4138]` y NO se persiste el UPDATE para esa fila.

### 2. Semántica del predicado evaluado (idéntico a Z3b INSERT)

`enforce_with_check` filtra policies aplicables (action ∈ {UPDATE, ALL}, role match) y para cada una usa:
- `with_check_sql` si está definido (PG explicit form);
- sino, `using_sql` como fallback (PG default semantics — UPDATE policies sin WITH CHECK reusan USING).

PERMISSIVE OR: al menos una policy aplicable debe evaluar TRUE para que el UPDATE pase.

### 3. Sin statement-level rollback

Si el UPDATE itera N filas y la fila K viola WITH CHECK, las K-1 anteriores ya fueron persistidas. **No hay rollback parcial dentro del statement** — esto es consistente con el comportamiento existente del engine en otros casos (e.g. constraint violation en multi-row INSERT). Para atomicidad fuerte, el caller envuelve en `BEGIN ... COMMIT/ROLLBACK`.

### 4. Sin cambio on-disk

Z3c es enforcement **semántico** sobre el formato Z3b: reusa `PolicyMeta` tal cual. No bump de VERSION.

## 📁 Archivos tocados

- `src/sql.rs`: nuevo bloque dentro de `exec_update` (~40 LOC) que construye el `post_row` y llama `enforce_with_check`.
- `tests/integration_test.rs`: 6 tests `z3c_*`.

## ⛔ Lo que **no** entra en Z3c

| Ítem | Razón del defer |
|---|---|
| `INSERT ... ON CONFLICT DO UPDATE` hookear el WITH CHECK del UPDATE path | El `apply_insert_row_with_conflict` que entra al path de UPDATE no llama `exec_update` — duplica lógica. Hookear el WITH CHECK ahí requeriría refactor o duplicación. Defer. |
| `INSERT/UPDATE ... RETURNING` filtrado contra policies SELECT | Tras un INSERT/UPDATE válido bajo WITH CHECK, el RETURNING expone la fila resultante. Aplicarle policies SELECT post-hoc requiere un segundo filtro sobre `affected_rows`. Defer. |
| Statement-level rollback en RLS violation | Hoy las filas pre-violación quedan persistidas. Defer hasta que el bloque T (transactions) tenga un savepoint primitive. |
| DEFAULTs aplicados antes del check | Mismo defer de Z3b: cols no-stated ven `Null` en el `post_row` antes del DEFAULT que aplica `apply_update_to_pk`. Para una policy que referencia `DEFAULT col`, no se ve el DEFAULT. |

## 🧪 Tests

6 tests `z3c_*`:
- UPDATE con post-image dentro del scope → pasa.
- UPDATE que saca la fila del scope (cambio de owner) → 4138.
- UPDATE policy sin `WITH CHECK` explícito reusa `USING` (PG semantics).
- UPDATE policy con `WITH CHECK` distinto de `USING` (e.g. `n >= 0`).
- Superuser bypass.
- Sin policies en la tabla → no enforcement (compat).

Suite total: **668 passing** (662 → +6 Z3c).

## 🔗 Referencias

- PostgreSQL `CREATE POLICY ... WITH CHECK` — UPDATE post-image semantics.
- ADR-0054 (Z3b): foundation de WITH CHECK que este ADR extiende a UPDATE.
- ADR-0052 (Z3): USING filtering en exec_update (read-side, complementario).
