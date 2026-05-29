# ADR-0062: DEFAULTs aplicados antes de WITH CHECK en INSERT (Z3f)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z3f (refinamiento WITH CHECK)
**Bump on-disk**: **ninguno** (semantic enforcement)

## 🧭 Contexto

Z3b/Z3c/Z3e cubrieron INSERT/UPDATE/upsert WITH CHECK. Z3d filtró RETURNING contra SELECT policies. Pero quedaba una sutileza documentada como defer: para columnas no-stated con DEFAULT, `enforce_with_check` veía `Null` en lugar del DEFAULT.

Esto producía dos comportamientos sutilmente incorrectos:

1. **Falsos rejects**: una policy `WITH CHECK (col = 'X')` aplicada a un INSERT que no state `col` y donde DEFAULT='X' — la check_row tiene `col=Null`, la policy evalúa `Null = 'X'` → NULL (3VL) → tratado como false → rebota con `[GBY-4138]` aunque el INSERT terminaría persistiendo con `col='X'` correctamente.

2. **Falsos accepts**: una policy `WITH CHECK (col IS NOT NULL)` — la check_row tiene `col=Null` (no se ve el DEFAULT), policy evalúa `Null IS NOT NULL` → false → rebota. Caso opuesto: policy `WITH CHECK (col IS NULL)` — pasaría aunque DEFAULT='X' lo poblaría con un valor.

PG aplica DEFAULTs **antes** del WITH CHECK. Z3f cierra el gap.

## 💡 Decisión

### 1. Llamar `apply_defaults` antes de `enforce_with_check` en `exec_insert`

```rust
if self.current_user.is_some() {
    let mut check_row: HashMap<String, Value> = HashMap::new();
    for (i, col) in normalized_cols.iter().enumerate() {
        check_row.insert(
            col.clone(),
            row_values.get(i).cloned().unwrap_or(Value::Null),
        );
    }
    // Z3f: aplicar DEFAULTs antes del check.
    apply_defaults(&meta, &mut check_row);
    // Inicializar columnas restantes (sin DEFAULT, no-stated) a Null.
    for c in &meta.columns {
        check_row.entry(normalize_ident(&c.name)).or_insert(Value::Null);
    }
    self.enforce_with_check(&stmt.table, POLICY_ACTION_INSERT, &check_row)?;
}
```

`apply_defaults` ya existe y se llama dentro de `apply_insert_row_with_conflict` para construir el row a persistir. Z3f lo invoca **adicionalmente** sobre la `check_row` para que la policy vea los mismos valores que terminarían en el disco.

### 2. Valores stated por el user mantienen precedencia

`apply_defaults` solo llena cols que NO están en el HashMap. Como nosotros insertamos las stated cols primero, sus valores ganan sobre el DEFAULT — comportamiento consistente con `apply_insert_row_with_conflict`.

### 3. Cols sin DEFAULT y no-stated siguen siendo Null

`apply_defaults` solo aplica cuando hay una `DEFAULT` declarada. Cols sin DEFAULT quedan ausentes del HashMap, y el `or_insert(Value::Null)` posterior las llena con Null — comportamiento Z3b/Z3c preservado.

### 4. Sin cambio para tablas sin policies

El bloque entero está dentro de `if self.current_user.is_some()`. Pre-Z3f compat 100% para INSERTs sin user activo o sin policies aplicables.

## 📁 Archivos tocados

- `src/sql.rs`: ~6 LOC añadidas en el hook Z3b de `exec_insert` — invocación de `apply_defaults(&meta, &mut check_row)` entre la fase de stated values y la fase de fallback Null.
- `tests/integration_test.rs`: 5 tests `z3f_*`.

## ⛔ Lo que **no** entra en Z3f

| Ítem | Razón del defer |
|---|---|
| DEFAULTs en el path Z3c/Z3e (UPDATE post-image, upsert UPDATE) | UPDATE no aplica DEFAULTs a cols no-stated (mantienen su valor previo). El hook ya construye `post_row` from `old_row + assignments`, lo cual ES la fila real. No hay gap aquí. |
| Statement-level rollback en RLS violation | Requiere savepoint primitive (Bloque T extension). |
| Column-level filter en RETURNING | Cambio breaking al modelo de policy. |
| Filter contra WITH CHECK además de USING en RETURNING (Z3d) | PG usa solo USING para RETURNING — comportamiento Z3d ya es correcto. |
| Existence leak vía ON CONFLICT DO NOTHING + invisible row | Inherente a SQL semantics; PG tiene la misma issue. |

## 🧪 Tests

5 tests `z3f_*`:
- `z3f_default_aplica_antes_de_with_check_pass` — col con DEFAULT='pending' + policy `WITH CHECK (status = 'pending')` → INSERT pasa.
- `z3f_default_aplica_antes_de_with_check_reject` — col con DEFAULT='pending' + policy `WITH CHECK (status = 'approved')` → rebota con `[GBY-4138]`.
- `z3f_user_stated_value_overrides_default` — user state 'approved' + DEFAULT 'pending' + policy `WITH CHECK (status = 'pending')` → rebota (stated gana).
- `z3f_no_default_col_sin_state_sigue_null` — col sin DEFAULT y no-stated sigue siendo Null → policy `WITH CHECK (col = 'X')` rebota.
- `z3f_compat_sin_policies_sin_change` — sin policies, DEFAULT se aplica como siempre vía path normal.

Suite total: **698 passing + 1 ignored** (693 → +5 Z3f).

## 🔗 Referencias

- PostgreSQL RLS — `WITH CHECK` interaction con `DEFAULT` columns (§5.8).
- ADR-0054 (Z3b): foundation de WITH CHECK que Z3f refina.
- ADR-0057 (Z3d): RETURNING filter complementario.
- ADR-0059 (Z3e): ON CONFLICT path.
