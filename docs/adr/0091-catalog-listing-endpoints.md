# ADR-0091: Catalog listing endpoints — server HTTP expone todo el catálogo

**Fecha:** 2026-06-17 → 2026-06-18
**Estado:** Aceptado
**Bloque:** Server HTTP — catalog listing (Pushes 7, 10, 14 de la sesión 2026-06-17/18).
**Refina:** [ADR-0090 M13](0090-m13-cross-request-tx.md) (Sessions HTTP). [ADR-0050 Z1](0050-z1-users-roles.md) (USERS/ROLES). [ADR-0025 Views](0025-views.md). [ADR-0052 Z3 RLS](0052-z3-row-level-security.md).

## Contexto

Hasta esta sesión, el server `gabysql-server` exponía sólo 9 endpoints:

```
GET  /health   /metrics   /dbs   /tables   /schema   /rows
POST /exec     /tx/begin  /tx/commit  /tx/rollback
```

`/tables` listaba tablas + columnas + índices simples + FKs. El
**resto del catálogo** vivía sólo dentro del proceso, accesible vía
las API internas de `Catalog`:

```rust
// Disponibles en código, no en HTTP:
catalog.list_views()
catalog.list_policies()
catalog.list_triggers()
catalog.list_procedures()
catalog.list_functions()
catalog.list_users()
catalog.list_roles()
catalog.list_grants()
```

Eso forzaba a cualquier UI o cliente externo a hacer SQL especulativo
(`SELECT name FROM __views__`, que ni siquiera existe como tabla
exponible) o a parsear `EXPLAIN`/`SHOW` que tampoco listaban estos
objetos. El admin web `phpgabyadmin` y el modelador `gabymodeler` no
podían mostrar policies/triggers/procs/funcs/users/roles/grants —
queda como caja negra.

Decisión post-Push 6: exponer los 8 `list_*` como GET endpoints.

## Decisión

8 endpoints nuevos, todos GET, todos espejando 1:1 la API interna:

```
GET /views?db=<db>
GET /policies?db=<db>[&table=<t>]
GET /triggers?db=<db>[&table=<t>]
GET /procedures?db=<db>
GET /functions?db=<db>
GET /users?db=<db>
GET /roles?db=<db>
GET /grants?db=<db>[&grantee=<g>][&object=<o>]
```

### Shape JSON

Cada respuesta envuelve un array en un objeto con `ok: true`:

```jsonc
GET /views   → { "ok": true, "views":   [{ "name", "source", "columnAliases": [...] | null }] }
GET /roles   → { "ok": true, "roles":   [{ "name" }] }
GET /grants  → { "ok": true, "grants":  [{ "grantee", "object", "privs": ["SELECT","INSERT",...] }] }
```

### Codes → keywords

Los enums internos (timing/event de triggers, action de policies,
KDF scheme de users, bitmask de privs) se **traducen a strings** en
el JSON. El cliente no debería conocer los shifts ni los códigos
binarios — eso es detalle de persistencia.

| Tipo | Interno | JSON |
|---|---|---|
| Trigger timing | `0` / `1` | `"BEFORE"` / `"AFTER"` |
| Trigger event | `0` / `1` / `2` | `"INSERT"` / `"UPDATE"` / `"DELETE"` |
| Policy action | `POLICY_ACTION_SELECT` ... | `"SELECT"` ... `"ALL"` |
| User scheme | `1` / `2` / `3` | `"pbkdf2-sha256"` / `"scrypt"` / `"argon2id"` |
| Grant privs | bitmask u32 | `["SELECT", "INSERT", ...]` |

### Seguridad — `/users`

**Crítico**: `/users` NO serializa `password_hash` ni `salt`. Sólo
expone `name`, `scheme` (string legible) e `iterations`. Material
secreto nunca debe filtrarse vía API HTTP, ni siquiera al cliente
de gestión.

Esto se valida en `tests/server_listing_endpoints.rs::users_endpoint_lists_user_without_secret_material`
con tres asserts negativos:

```rust
assert!(!body.contains("password_hash"));
assert!(!body.contains("\"salt\""));
assert!(!body.contains("hunter2"));        // el password en claro
```

### Filtros

Tres endpoints aceptan filtros opcionales aplicados **post-list**
(no via índice — el catálogo es chico):

- `/policies?table=<t>` — útil para "todas las policies de la tabla X"
  desde la vista Structure de una tabla.
- `/triggers?table=<t>` — idem.
- `/grants?grantee=<g>` / `?object=<o>` — útil para "qué privilegios
  tiene Bob" y "quién tiene SELECT sobre orders".

Decisión: filtros post-list en vez de WHERE en el storage es
tolerable porque el catálogo típico tiene <100 entradas total.
Cuando un repo crezca a 10k policies tendrá que migrarse a un
secondary index, pero esa es deuda futura, no de hoy.

## Tests E2E

Push 15 agregó `tests/server_listing_endpoints.rs` con 10 tests:

```
views_endpoint_lists_declared_view
policies_endpoint_lists_declared_policies
policies_endpoint_filters_by_table
triggers_endpoint_lists_declared_trigger
procedures_endpoint_lists_declared_procedure
functions_endpoint_lists_declared_function
users_endpoint_lists_user_without_secret_material    ← seguridad
roles_endpoint_lists_declared_role
grants_endpoint_lists_privileges_as_keyword_array
grants_endpoint_filters_by_grantee
```

Total de tests del repo pasa de 828 a 838. CI verde en Ubuntu/macOS/
Windows + Docker.

## Alternativas descartadas

- **Endpoint único `/catalog?type=view|policy|...`**: rechazado por
  romper REST y dificultar caché por endpoint. Hoy cada uno se puede
  cachear independientemente.
- **Endpoint `/objects` unificado**: existe `catalog.list_objects()`
  internamente; lo dejamos para una futura iteración si emerge un
  caso de uso de "muéstrame TODO el catálogo de un vistazo". Hoy
  los clientes que conocemos (phpgabyadmin tabs, gabymodeler import)
  ya saben qué tipo necesitan.
- **GraphQL**: rechazado — ADR-0001 dice "cero deps externas". gRPC
  same.

## Consecuencias

### Positivas
- `phpgabyadmin` puede listar todo (Pushes 8, 11, 17).
- `gabymodeler` puede hacer reverse engineering completo (Push 21).
- Material secreto blindado por el `/users` con assertions.
- Forward-compat: futuros `list_*` (e.g. `list_indexes` global) calzan
  en el mismo patrón.

### Negativas / tradeoffs
- 8 endpoints más para mantener si el shape del catalog cambia.
  Mitigado por los tests E2E que detectan drift al primer commit.
- Sin paginación: si un DB llega a tener 50k policies, una sola
  respuesta JSON pesa. Aceptable por el rango operacional típico.

## Hotfix relacionado

El test del endpoint `/users` falló en CI inicial (Push 16 → CI rojo)
porque mi mapeo de `scheme` en `user_meta_json` decía `1 => "argon2id"`
basado en una lectura errónea de `docs/STATUS.md`. El motor real
devuelve `scheme=2` (scrypt) como default. Pushes 16.fix1 + 15.fix2
corrigieron:
- mapeo en server.rs: 1=pbkdf2-sha256, 2=scrypt, 3=argon2id.
- test agnóstico al default exacto (acepta cualquier scheme conocido).
- ADR-0091 documenta el orden correcto.
- STATUS.md fila 102 corregida en Push 20 con "scrypt default
  verificado vía E2E".

Lección lesson_docs_invariants_check re-aplicada: las afirmaciones
sobre defaults del motor ahora se cross-checkean vs test E2E.

## Referencias

- Commits: [d7f7f0a](https://github.com/vladimiracunadev-create/gabysql/commit/d7f7f0a) (Push 7), [0b4cceb](https://github.com/vladimiracunadev-create/gabysql/commit/0b4cceb) (Push 10), [a0e1f71](https://github.com/vladimiracunadev-create/gabysql/commit/a0e1f71) (Push 14), [ef7a8a9](https://github.com/vladimiracunadev-create/gabysql/commit/ef7a8a9) (Push 15 tests).
- Hotfixes: [5934a8c](https://github.com/vladimiracunadev-create/gabysql/commit/5934a8c), [cf89d0f](https://github.com/vladimiracunadev-create/gabysql/commit/cf89d0f), [b13505d](https://github.com/vladimiracunadev-create/gabysql/commit/b13505d).
- Implementación: [src/server.rs](../../src/server.rs).
- Tests: [tests/server_listing_endpoints.rs](../../tests/server_listing_endpoints.rs).
- Catálogo interno: [src/catalog.rs](../../src/catalog.rs).
