# ADR-0054: `WITH CHECK` + `FOR INSERT` policies (Z3b)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z3b (follow-up de Z3 — write-side de RLS para INSERT)
**Bump on-disk**: **VERSION 26 → 27**

## 🧭 Contexto

Z3 (ADR-0052) entregó RLS para SELECT/UPDATE/DELETE vía rewriting del WHERE con el OR de los predicados USING. `INSERT` quedó **explícitamente deferido** porque requiere semántica distinta: el predicado se evalúa contra la **fila a insertar** (no contra filas existentes), y se llama `WITH CHECK` en lugar de `USING`.

Z3b cubre ese hueco: `FOR INSERT` permitido en CREATE POLICY, cláusula `WITH CHECK (expr)` opcional, y enforcement en `exec_insert` antes de persistir. Mantiene compat 100% con DBs Z3.

## 💡 Decisión

### 1. `PolicyMeta` extendido con `with_check_sql: Option<String>`

Layout on-disk: `[..campos Z3..][with_check_flag:u8][with_check_sql opcional]`. Cuando es `None`, sólo se serializa el flag byte `0`. Cuando es `Some`, flag `1` + string length-prefixed.

```rust
pub struct PolicyMeta {
    pub name: String,
    pub table: String,
    pub action: u8,
    pub roles: Vec<String>,
    pub using_sql: String,           // puede ser "" para FOR INSERT
    pub with_check_sql: Option<String>,  // opcional
}
```

Nueva constante: `POLICY_ACTION_INSERT = 2`.

### 2. Sintaxis DDL

```sql
CREATE POLICY name ON table FOR {ALL|SELECT|INSERT|UPDATE|DELETE} [TO role,...]
    [USING (expr)] [WITH CHECK (expr)]
```

Validaciones del parser:
- Al menos una de `USING (...)` o `WITH CHECK (...)` debe estar.
- `FOR INSERT` rechaza `USING (...)` (USING no aplica a INSERT — error explícito).
- Otras combinations libres.

### 3. Semántica de enforcement

| Action al ejecutar | Policies con `action ∈ {INSERT, ALL}` aplicables al user/role | Predicado evaluado |
|---|---|---|
| INSERT | Sí | Si tiene `with_check_sql` → ese. Sino, **policy no aplica** (skip). |
| INSERT | Sí, pero ninguna tiene WITH CHECK | Deny (todas las policies aplicables son skip → no "passed any") |
| INSERT | No (no hay policies para INSERT en la tabla) | **Bypass** (compat — si no hay policies relevantes, RLS no aplica) |

**OR semantics**: si **al menos una** policy aplicable evalúa WITH CHECK como TRUE para la fila, el INSERT pasa. Mismo PERMISSIVE de PG.

**Edge cases importantes**:
- Tabla con policies SELECT pero **sin policies INSERT/ALL** + current_user activo + INSERT → **deny** (`[GBY-4138]`). Tener una policy para SELECT activa RLS en la tabla; INSERT sin policy aplicable se trata como "nadie te autorizó a hacerlo".
- Tabla **completamente sin policies** + current_user activo + INSERT → **bypass** (compat 100% pre-Z3b).
- `current_user is None` (superuser) → **bypass** total.

### 4. UPDATE post-image WITH CHECK — diferido

PostgreSQL chequea WITH CHECK contra la fila *post-update* (asegura que la modificación no saque la fila del scope visible). Z3b **no implementa** este chequeo — la lógica de Z3 USING en exec_update ya filtra qué filas se modifican, lo cual es la primera capa de defensa. El post-image check queda como defer Z3c. La razón: hookear post-image en exec_update requiere intervenir en el bucle per-row interno (post-aplicación de assignments, pre-persist), que toca el flujo principal de UPDATE — mejor en su propio bloque.

### 5. Códigos de error

| Código | Nombre | Caso |
|---|---|---|
| 4138 | `POLICY_CHECK_VIOLATION` | `INSERT` (o `UPDATE` futuro) viola WITH CHECK de todas las policies aplicables |

## 📁 Archivos tocados

- `src/storage.rs`: bump `VERSION 26 → 27`.
- `src/catalog.rs`: `PolicyMeta.with_check_sql: Option<String>`, `POLICY_ACTION_INSERT = 2`, serialize/deserialize extendidos con flag byte.
- `src/errors.rs`: código 4138.
- `src/sql.rs`:
  - `CreatePolicyStmt.with_check_sql: Option<String>`.
  - Parser: `parse_create_policy` reescrito — acepta `FOR INSERT`, USING opcional, WITH CHECK opcional, validaciones. Nuevo helper `capture_paren_balanced`.
  - `exec_create_policy` valida WITH CHECK con `parse_expr`, persiste el campo.
  - Nuevo método `enforce_with_check(table, action_write, new_row)`.
  - Hook en `exec_insert` antes de `apply_insert_row_with_conflict`: builds `check_row` (cols stated + Null para no-stated) y llama `enforce_with_check(table, POLICY_ACTION_INSERT, ...)`.
- `tests/integration_test.rs`: 11 tests `z3b_*`.

## ⛔ Lo que **no** entra en Z3b (defer)

| Ítem | Razón del defer |
|---|---|
| **UPDATE post-image WITH CHECK** | Z3 USING ya gatea qué filas se tocan. Post-image check requiere hookear el bucle interno de UPDATE — defer Z3c. |
| **DEFAULT applied antes del check** | Si el INSERT no setea una columna, hoy es `Value::Null` en el `check_row` (el DEFAULT se aplica después en `apply_insert_row_with_conflict`). Para policies que referencian `col DEFAULT 'x'`, el WITH CHECK no verá `'x'` sino NULL. Limitación documentada. |
| `INSERT ... ON CONFLICT DO UPDATE` con WITH CHECK del UPDATE path | El INSERT path tira WITH CHECK; si cae al UPDATE path por ON CONFLICT, no se hace otro WITH CHECK. Z3c lo cubre cuando llegue el UPDATE post-image. |
| `INSERT ... RETURNING` filtrado por SELECT policies | El RETURNING expone columnas del row recién insertado. Hoy no se filtra contra policies SELECT. Defer. |

## 🧪 Tests

11 tests `z3b_*`:
- Persistencia de `with_check_sql` y `action=2` (INSERT).
- INSERT que pasa WITH CHECK.
- INSERT que falla WITH CHECK → 4138.
- INSERT con policies SELECT pero ninguna INSERT/ALL aplicable → deny.
- INSERT sin policies en la tabla → bypass (compat).
- `FOR ALL` con WITH CHECK usado en INSERT.
- `FOR ALL` sin WITH CHECK no aplica a INSERT → deny.
- `FOR INSERT` rechaza USING en parser.
- `CREATE POLICY` sin USING ni WITH CHECK → error.
- Policy con role filter — bob no en TO alice se bloquea.
- Superuser bypass.

Suite total: **662 passing** (651 → +11 Z3b).

## 🔗 Referencias

- PostgreSQL `CREATE POLICY ... WITH CHECK` (§5.8 RLS).
- ADR-0052 (Z3): foundation que este ADR extiende.
- ADR-0051 (Z2): `current_user` model used by enforcement.
