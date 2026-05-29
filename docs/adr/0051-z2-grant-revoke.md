# ADR-0051: `GRANT` / `REVOKE` + `SET SESSION AUTHORIZATION` (Z2)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z2 (segundo sub-bloque del bloque Z)
**Bump on-disk**: **VERSION 23 → 24**

## 🧭 Contexto

Z1 entregó identidad SQL-level (`CREATE USER` / `CREATE ROLE`). Z2 conecta esa identidad al motor de ejecución: GRANT/REVOKE persistidos en el catálogo + chequeo de privilegios en cada DML cuando hay un user activo en la sesión.

Z2 también introduce el primer concepto de **sesión** del motor: `SET SESSION AUTHORIZATION 'name'` activa el modo "actuar como user X"; `DEFAULT` vuelve a superuser (bypass total). Sin SET previo, el motor mantiene el comportamiento pre-Z2 (cualquier statement permitido) — la migración a "deny by default" es opt-in.

## 💡 Decisión

### 1. Catálogo: `ObjectKind::Grant = 7` + `GrantMeta`

```rust
pub struct GrantMeta {
    pub grantee: String,
    pub object: String,
    pub privs: u32,  // bitmask
}
```

Clave compuesta: `__grant__:<grantee_lower>:<object_lower>`. Cada par (grantee, object) tiene su propio slot en el B-tree del catálogo. GRANTs múltiples sobre el mismo par se **mergean por OR** del bitmask antes de persistir; REVOKE limpia con AND-NOT.

El prefijo `__grant__:` no es un nombre legal de tabla/user (los nombres deben matchear `[a-zA-Z_][a-zA-Z0-9_]*`), así que no hay riesgo de colisión con un objeto real.

Para que el chequeo de colisión de hash (`get_object` valida que el record en el bucket corresponda al nombre buscado) funcione con la clave compuesta, `CatalogObject::matches_lookup_name` compara contra la clave compuesta para variants `Grant` y contra `name()` directo para el resto.

### 2. Bitmask de privilegios

```
PRIV_SELECT     = 0x01
PRIV_INSERT     = 0x02
PRIV_UPDATE     = 0x04
PRIV_DELETE     = 0x08
PRIV_REFERENCES = 0x10
PRIV_TRUNCATE   = 0x20
PRIV_ALL        = 0x3F   // OR de los anteriores
```

`GRANT ALL [PRIVILEGES]` expande al mask completo. Privilegios desconocidos → `[GBY-4130]`.

### 3. DDL

```sql
GRANT priv [, priv]* ON [TABLE] obj TO grantee
REVOKE priv [, priv]* ON [TABLE] obj FROM grantee
SET SESSION AUTHORIZATION 'user' | DEFAULT
```

- `TABLE` keyword es opcional (acepta ambos `ON obj` y `ON TABLE obj`).
- `grantee` puede ser un user existente, un role existente, o `PUBLIC` (especial — implícito, no requiere CREATE USER/ROLE).
- REVOKE sobre un par sin GRANT previo es **no-op** (idempotente, no error).
- REVOKE que deja `privs == 0` borra el record entero del catálogo.

### 4. Modelo de sesión

```rust
pub struct Engine<'a> {
    // ...
    current_user: Option<String>,  // None = superuser bypass
}
```

- `None` (default): bypass total de chequeos — comportamiento idéntico pre-Z2. Toda DB existente sigue funcionando.
- `Some(name)`: cada DML llama a `check_priv(object, priv_required)` que busca el mask persistido para `(name, object)` y para `(PUBLIC, object)`, los mergea con OR, y compara.
- `SET SESSION AUTHORIZATION 'name'`: valida que el user exista (sino `[GBY-4125]`) y setea `current_user = Some(name)`.
- `SET SESSION AUTHORIZATION DEFAULT`: `current_user = None`, vuelve a superuser.

### 5. Hooks de enforcement

```
exec_insert    → check_priv(table, PRIV_INSERT)
exec_update    → check_priv(table, PRIV_UPDATE)
exec_delete    → check_priv(table, PRIV_DELETE)
exec_truncate  → check_priv(table, PRIV_TRUNCATE)
exec_select    → check_priv(base + joins, PRIV_SELECT)
                  (derived tables / VALUES no requieren priv — son sintéticas)
```

Si el chequeo falla → `[GBY-4129]` PRIVILEGE_DENIED con detalle de mask requerido vs efectivo.

### 6. Códigos de error

| Código | Nombre | Caso |
|---|---|---|
| 4129 | `PRIVILEGE_DENIED` | El user activo no tiene el priv requerido sobre el objeto |
| 4130 | `INVALID_PRIVILEGE` | `GRANT FROBNICATE` — keyword desconocido |
| 4131 | `GRANT_OBJECT_NOT_FOUND` | `GRANT ... ON ghost_table` |
| 4132 | `GRANTEE_NOT_FOUND` | `GRANT ... TO ghost_user` (sin PUBLIC ni USER/ROLE) |

## 📁 Archivos tocados

- `src/storage.rs`: bump `VERSION 23 → 24`.
- `src/catalog.rs`: `ObjectKind::Grant = 7`, `GrantMeta` + helpers, `put_grant`/`get_grant`/`remove_grant`/`list_grants`. Constantes `PRIV_SELECT/INSERT/UPDATE/DELETE/REFERENCES/TRUNCATE/ALL`. `CatalogObject::matches_lookup_name` para soportar claves compuestas en el lookup de colisión.
- `src/errors.rs`: códigos 4129-4132.
- `src/sql.rs`:
  - AST: `Statement::Grant/Revoke/SetSessionAuth` + structs.
  - Parser: `parse_grant`, `parse_revoke`, `parse_privilege_list`, dispatch SESSION AUTHORIZATION en `parse_set_stmt`.
  - Engine: `current_user: Option<String>` en struct; `exec_grant`/`exec_revoke`/`exec_set_session_auth`/`check_priv` métodos.
  - Helpers: `privilege_list_to_mask`, `grantee_is_valid`.
  - Hooks de enforcement en `exec_select`, `exec_insert`, `exec_update`, `exec_delete`, `exec_truncate`.
  - 5 match arms ampliados para cubrir `CatalogObject::Grant` exhaustivamente.
- `tests/integration_test.rs`: 15 tests `z2_*`.

## ⛔ Lo que **no** entra en Z2 (deferido a Z3 o más allá)

| Ítem | Razón del defer |
|---|---|
| **RLS** (`CREATE POLICY ... USING (expr)`) | Bloque Z3 entero — requiere inyección de filtros WHERE por policy match en SELECT/UPDATE/DELETE. |
| `GRANT priv ON COLUMN ... TO ...` | Column-level GRANT. La metadata actual es per-objeto; agregar per-column requiere bitmask vectorial. |
| `WITH GRANT OPTION` | Habilita re-grant transitivo. Requiere chequear el "grantor" en cada GRANT. |
| `ROLE` real con membership (`GRANT role TO user`) | Hoy `CREATE ROLE` persiste pero el role no tiene members. `GRANT role_name TO user_name` sería un GrantMeta con `priv = membership_marker`. |
| `current_user` / `session_user` como funciones SQL | Scalar functions que devuelven el `current_user` activo. Útiles para policies (Z3). |
| GRANTs sobre `PROCEDURE`/`FUNCTION` (`EXECUTE`) | Hoy sólo tabla/vista. Extender `check_priv` para procs/funcs es one-liner pero requiere wire-up en `exec_call` y `eval_user_func`. |
| Default deny en lugar de superuser bypass | Cambiar el default de "sin SET = superuser" a "sin SET = ningún priv" rompería todas las DBs existentes. Migración explícita en futuro Z4. |

## 🧪 Tests

15 tests `z2_*`: persist+merge+revoke de bitmask, GRANT ALL, denied SELECT/INSERT/UPDATE/DELETE/TRUNCATE, PUBLIC visible-a-todos, SET SESSION AUTHORIZATION + DEFAULT, errores 4129/4130/4131/4132 y 4125 al set-session a user inexistente.

Suite total: **629 passing** (614 → +15 Z2).

## 🔗 Referencias

- ANSI SQL:2003 §12 (Privileges).
- PostgreSQL `GRANT` / `REVOKE` / `SET SESSION AUTHORIZATION`.
- ADR-0050 (Z1): foundation de identidad sobre la que se apoya.
