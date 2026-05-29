# ADR-0050: Identidad SQL-level — `CREATE USER` / `CREATE ROLE` (Z1)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z1 (primer sub-bloque del bloque Z — control de acceso SQL)
**Bump on-disk**: **VERSION 22 → 23**

## 🧭 Contexto

Hasta Y9 cerrado, el control de acceso de gabysql era **exclusivamente** un token compartido en el server HTTP (`-token` flag). A nivel SQL no existían `CREATE USER`, `GRANT`, `REVOKE` ni RLS — un agujero grande para casos donde una app multi-tenant necesita modelar identidad.

Z1 abre el bloque Z entregando la **fundación** que GRANT/REVOKE (Z2) y RLS (Z3) van a apoyarse encima: un namespace persistente de users y roles, con DDL completo y un hash de password no-cripto-pero-aceptable.

## 💡 Decisión

### 1. Catálogo: dos nuevos `ObjectKind`

```rust
ObjectKind::User = 5
ObjectKind::Role = 6
```

Reusa el `B-tree` del catálogo (mismo path que tablas/vistas/triggers/etc). Namespace **flat**: un name no puede ser simultáneamente tabla, vista, user, role, etc. — colisión rebotada con el código correspondiente.

### 2. `UserMeta` + `RoleMeta`

```rust
pub struct UserMeta {
    pub name: String,
    pub password_hash: u64,  // FNV-1a-64(salt || password)
    pub salt: u64,           // xorshift64(SystemTime::nanos ^ magic)
}

pub struct RoleMeta {
    pub name: String,
}
```

`UserMeta` serializa como `[len:u16][name][hash:u64 LE][salt:u64 LE]`. `RoleMeta` sólo el nombre.

### 3. DDL completo

```sql
CREATE USER name [WITH PASSWORD '...' | IDENTIFIED BY '...']
DROP USER [IF EXISTS] name
CREATE ROLE name
DROP ROLE [IF EXISTS] name
ALTER USER name SET PASSWORD '...'
ALTER USER name IDENTIFIED BY '...'
ALTER USER name WITH PASSWORD '...'
```

`CREATE USER` sin cláusula de password persiste el user con hash de string vacío. Es legítimo (caso "creo el user ahora, le seteo password después con ALTER").

### 4. Hash de password: FNV-1a-64 + salt — **NO crypto-grade**

```rust
salt: xorshift64(SystemTime::nanos ^ magic)
hash: FNV-1a-64(salt.to_le_bytes() || password.as_bytes())
```

**Aviso explícito**: esto **no es un KDF**. No es PBKDF2, no es bcrypt, no es argon2. No resiste:
- Ataques de diccionario con GPU.
- Tablas precomputadas (mitigado parcialmente por el salt aleatorio por user).
- Cualquier adversario con acceso al archivo `.db`.

El propósito de Z1 es **bookkeeping SQL-level** alineado con el estándar — para que apps puedan modelar `who-is-who` y, eventualmente, conectar GRANT/REVOKE/RLS. La autenticación real en el servidor HTTP sigue siendo el token compartido. Si en el futuro se necesita auth de password de verdad, Z1b: integrar argon2 o PBKDF2 con iteraciones configurables — defer.

### 5. Validación de nombres

`[a-zA-Z_][a-zA-Z0-9_]*`, no vacío, ≤ 64 bytes. Sin quoted identifiers (`"foo bar"`) — defer. Diferente a `validate_identifier` existente porque queremos límites más estrictos sobre identidad.

### 6. Códigos de error

| Código | Nombre | Caso |
|---|---|---|
| 4124 | `USER_ALREADY_EXISTS` | `CREATE USER` con nombre ocupado |
| 4125 | `USER_NOT_FOUND` | `DROP USER` / `ALTER USER` sobre nombre inexistente |
| 4126 | `ROLE_ALREADY_EXISTS` | `CREATE ROLE` con nombre ocupado |
| 4127 | `ROLE_NOT_FOUND` | `DROP ROLE` sobre nombre inexistente |
| 4128 | `INVALID_USER_NAME` | Nombre vacío, char inválido, > 64 bytes |

## 📁 Archivos tocados

- `src/storage.rs`: bump `VERSION 22 → 23`.
- `src/catalog.rs`: `ObjectKind::User/Role` con codes 5/6, `UserMeta` + `RoleMeta` (serialize/deserialize), `put_user`/`put_role`/`get_user`/`get_role`/`list_users`/`list_roles`, extensiones en `decode_catalog_object` y `CatalogObject::name()`.
- `src/errors.rs`: códigos 4124-4128.
- `src/sql.rs`:
  - AST: `Statement::CreateUser/DropUser/CreateRole/DropRole/AlterUserPassword` + structs correspondientes.
  - Parser: `parse_create_user` / `parse_create_role` / `parse_alter_user` + dispatch en `parse_create`, `parse_drop`, `parse_alter`.
  - Engine: `exec_create_user` / `exec_drop_user` / `exec_create_role` / `exec_drop_role` / `exec_alter_user_password`.
  - Helpers: `validate_user_name`, `catalog_object_kind_name`, `gen_password_salt`, `hash_password`.
  - Match arms exhaustivos: 6 sitios actualizados para incluir `User`/`Role`.
- `tests/integration_test.rs`: 9 tests `z1_*`.

## ⛔ Lo que **no** entra en Z1 (split del bloque Z)

| Sub-bloque | Scope |
|---|---|
| **Z2** | `GRANT priv ON object TO user|role` y `REVOKE` con bitmask de privs persistente por (grantee, object), enforcement en exec_select/insert/update/delete. |
| **Z3** | RLS: `CREATE POLICY name ON table FOR action USING (expr)` + inyección de filtros WHERE por policy match en SELECT/UPDATE/DELETE. |
| Z1b (futuro) | KDF real (argon2/bcrypt) reemplazando FNV-1a, con iteraciones configurables y migración del field `password_hash`. |
| Quoted identifiers | `"foo bar"` como nombre de user/role. |
| `SET ROLE` / `CURRENT_USER` | Identidad de la sesión activa. Requiere protocolo extendido server-side. |

## 🧪 Tests

`z1_create_user_persists_and_drops`, `z1_create_user_duplicate_errors`, `z1_drop_user_not_found_errors`, `z1_create_role_persists_and_drops`, `z1_create_role_duplicate_errors`, `z1_alter_user_password_changes_hash`, `z1_invalid_user_name_errors`, `z1_user_role_name_collision`, `z1_identified_by_syntax_works`.

Suite total: **614 passing** (605 → +9 Z1).

## 🔗 Referencias

- ANSI SQL:2003 §4.34 (Privileges, users, roles).
- PostgreSQL `CREATE ROLE` / `CREATE USER` (alias).
- MySQL `CREATE USER ... IDENTIFIED BY '...'`.
