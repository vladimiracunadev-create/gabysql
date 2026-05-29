# ADR-0052: Row-Level Security — `CREATE POLICY` / `DROP POLICY` (Z3)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z3 (tercer sub-bloque del bloque Z — cierre del trío Z1+Z2+Z3)
**Bump on-disk**: **VERSION 24 → 25**

## 🧭 Contexto

Z1 entregó identidad (users/roles). Z2 conectó esa identidad al motor vía GRANT/REVOKE de privilegios per-objeto. Z3 cierra el círculo con **Row-Level Security**: ahora podemos decir "el user X sólo ve/modifica las filas que cumplan este predicado" — un mecanismo crítico para apps multi-tenant donde un GRANT SELECT sobre la tabla entera ya filtraría demasiado.

Z3 cierra el bloque Z al 100% del scope que tenía en el roadmap. Cierres avanzados (column-level GRANT, WITH CHECK, role membership, KDF real) quedan documentados como defer.

## 💡 Decisión

### 1. Catálogo: `ObjectKind::Policy = 8` + `PolicyMeta`

```rust
pub struct PolicyMeta {
    pub name: String,
    pub table: String,
    pub action: u8,         // 0=ALL, 1=SELECT, 3=UPDATE, 4=DELETE
    pub roles: Vec<String>, // vacío = PUBLIC
    pub using_sql: String,  // texto del predicado, re-parseado en cada fire
}
```

Clave compuesta: `__policy__:<name_lower>:<table_lower>`. Múltiples policies pueden coexistir sobre la misma tabla — sus predicados se combinan con **OR** (semántica PERMISSIVE de PostgreSQL). El sufijo `:table` evita colisiones si dos tablas distintas tienen una policy con el mismo nombre.

`using_sql` se persiste como texto SQL (mismo patrón que `ViewMeta::source` y `TriggerMeta::body_sql`) y se re-tokeniza + re-parsea en cada exec que toca la tabla.

### 2. DDL

```sql
CREATE POLICY name ON table FOR {ALL|SELECT|UPDATE|DELETE} [TO role1, role2, ...] USING (expr)
DROP POLICY [IF EXISTS] name ON table
```

- **Action**: ALL cubre SELECT/UPDATE/DELETE en simultáneo; INSERT no se enforce en Z3 (requeriría `WITH CHECK`, defer Z3b).
- **TO roles**: lista opcional. Vacío = PUBLIC (aplica a todos los users autenticados).
- **USING (expr)**: cualquier expresión booleana válida en gabysql. Puede referenciar columnas de la fila (`owner = 'alice'`), funciones (`CURRENT_TIMESTAMP < expires_at`), aritmética, CASE, etc.
- Sin `INSERT` (no se enforce — defer a WITH CHECK).
- Sin `WITH CHECK` (defer).
- Sin `AS PERMISSIVE | RESTRICTIVE` (Z3 implementa PERMISSIVE = OR de todos los predicados; RESTRICTIVE = AND, defer).

### 3. Estrategia de enforcement: rewriting del WHERE

En lugar de filtrar fila-a-fila post-fetch, Z3 **inyecta** los predicados USING como un AND al `where_clause` del statement:

```
WHERE (orig_where) AND (USING_pred_1 OR USING_pred_2 OR ...)
```

Esto deja que el pipeline existente del engine haga el trabajo — el filter se beneficia automáticamente de índices, range-scans, fast-paths, etc. La construcción se hace con el AST type `WhereExpr` (que ya soporta And/Or/Atom) wrapping cada USING expression en `WhereClause::ExprPredicate { expr: <parsed using> }`.

**Edge cases**:
- Tabla **sin policies** → `build_rls_where` retorna `None`, no rewrite, comportamiento idéntico pre-Z3.
- Tabla con policies pero **ninguna aplicable** al user/action → inject `WHERE false` → deny all rows.
- `current_user is None` (superuser) → bypass total, igual que Z2.

### 4. Hooks

```
exec_select  → build_rls_where(table, POLICY_ACTION_SELECT) → AND con stmt.where_clause
exec_update  → build_rls_where(table, POLICY_ACTION_UPDATE) → AND con stmt.where_clause
exec_delete  → build_rls_where(table, POLICY_ACTION_DELETE) → AND con stmt.where_clause
```

JOINs en SELECT: el rewriting **sólo aplica a la base table** (`stmt.table`). Tablas joineadas no reciben RLS en este release — el documento de defer lo explicita.

### 5. Códigos de error

| Código | Nombre | Caso |
|---|---|---|
| 4133 | `POLICY_ALREADY_EXISTS` | `CREATE POLICY` con (name, table) ya ocupado |
| 4134 | `POLICY_NOT_FOUND` | `DROP POLICY` sobre (name, table) inexistente |
| 4135 | `POLICY_TARGET_INVALID` | Target no es tabla (vista no soportada en Z3) |
| 4136 | `POLICY_PREDICATE_FAILED` | USING expr no parsea / falla al evaluar |

## 📁 Archivos tocados

- `src/storage.rs`: bump `VERSION 24 → 25`.
- `src/catalog.rs`: `ObjectKind::Policy = 8`, `PolicyMeta` + helpers, constantes `POLICY_ACTION_*`, `put_policy`/`get_policy`/`remove_policy`/`list_policies`/`list_policies_for_table`. Extensión de `CatalogObject::matches_lookup_name` para Policy (key compuesta).
- `src/errors.rs`: códigos 4133-4136.
- `src/sql.rs`:
  - AST: `Statement::CreatePolicy/DropPolicy` + structs.
  - Parser: `parse_create_policy` (captura USING como texto vía `reconstruct_sql_from_tokens`), dispatch `DROP POLICY` en `parse_drop`, dispatch `CREATE POLICY` en `parse_create`.
  - Engine: `exec_create_policy`/`exec_drop_policy`; helpers `build_rls_where(table, action) -> Option<WhereExpr>` y `merge_where_with_rls(orig, rls)`.
  - Hooks: rewriting del `where_clause` al inicio de `exec_select`, `exec_update`, `exec_delete`.
  - 5 match arms ampliados para cubrir `CatalogObject::Policy` exhaustivamente.
- `tests/integration_test.rs`: 14 tests `z3_*`.

## ⛔ Lo que **no** entra en Z3 (defer explícito)

| Ítem | Razón del defer |
|---|---|
| `WITH CHECK (expr)` clause | Para `INSERT` y `UPDATE` write-side. Diferente semántica que `USING` (which gates reads). Defer Z3b. |
| Policy sobre `INSERT` | Implicaría WITH CHECK. Defer Z3b. |
| `AS RESTRICTIVE` (AND semantics) | Z3 implementa sólo PERMISSIVE (OR de todos los predicados aplicables). RESTRICTIVE requiere un bit en `PolicyMeta` + lógica de combinación. Defer. |
| Policies sobre **vistas** | Hoy `POLICY_TARGET_INVALID` si target es vista. Defer porque vistas heredan policies de su base table, no las tienen propias en PG. |
| RLS sobre **JOINs / tablas secundarias** | Z3 sólo rewrite-ea el WHERE de la base table del SELECT. Para tablas joineadas requeriría rewrite recursivo de cada `JoinClause`. Defer. |
| `ALTER TABLE ... ENABLE ROW LEVEL SECURITY` flag | Z3 activa RLS implícitamente "any-policy = RLS-on". PG requiere ENABLE explícito. Diferencia documentada; defer del flag por simplicidad. |
| `FORCE ROW LEVEL SECURITY` (table owner bypass) | PG permite que el owner bypasee policies. Z3 no diferencia owner — el bypass es solo via `SET SESSION AUTHORIZATION DEFAULT`. Defer. |
| `ALTER POLICY` (cambiar predicado in-place) | Hoy hay que DROP + CREATE. Defer. |
| Función `current_user()` invocable desde dentro del USING expr | Útil para `USING (owner = current_user())`. Hoy hay que usar `'alice'` literal. Defer. |

## 🧪 Tests

14 tests `z3_*`: persist + drop, select/update/delete filter, action ALL, multiple-policies OR semantics, no-policies = compat, role list restricts, superuser bypass, error codes 4133/4134/4135.

Suite total: **643 passing** (629 → +14 Z3).

## 🔗 Referencias

- PostgreSQL `CREATE POLICY` (§5.8 RLS).
- ADR-0050 (Z1) y ADR-0051 (Z2): cadena de dependencia.
- RFC 9562 §5 (no aplicable directamente; pero las policies pueden usar UUID v7 ordenado de Y9).
