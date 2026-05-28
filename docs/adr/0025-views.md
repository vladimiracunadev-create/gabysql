# ADR-0025: Vistas lógicas (`CREATE VIEW` / `DROP VIEW`)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-27
**Bloque**: V (vistas) — siguiente del roadmap tras el cierre del bloque L
**Bump on-disk**: VERSION 12 → 13

## 🧭 Contexto

Con el bloque L completo (constraints + ON UPDATE activo), la superficie SQL de tablas estaba ya cerrada para CRUD relacional clásico. Faltaba el primer mecanismo de **abstracción semántica**: vistas. Casos típicos:

- Encapsular una agregación reutilizable (`CREATE VIEW totales_por_dept AS SELECT dept, SUM(monto) FROM ventas GROUP BY dept`).
- Aislar columnas sensibles (`CREATE VIEW empleados_publicos AS SELECT id, nombre FROM empleados`).
- Renombrar a un lenguaje de dominio (`CREATE VIEW clientes_premium AS SELECT … FROM users WHERE plan = 'gold'`).

Antes de este push, ninguno de esos casos tenía soporte en gabysql — había que repetir el SELECT cada vez.

## 💡 Decisión

### 1. Persistencia: discriminator byte + namespace compartido

El catálogo pre-V era exclusivamente de tablas. Para hospedar también vistas:

- Cada record arranca con un byte discriminator `[kind:u8]` (0=Table, 1=View).
- Tablas y vistas comparten **namespace** (mismo hash key del catálogo). Una vista no puede llamarse igual que una tabla — y viceversa. Rechazo explícito en `CREATE VIEW` con `[GBY-4077]`.

Bump VERSION 12→13. V12 files rechazados con `[GBY-1003]`.

### 2. Persistencia del cuerpo: SQL crudo, no AST

Como con CHECK constraints (L2), guardamos el **texto SQL** del SELECT que define la vista, no su AST. Razones:

- El AST de `SelectQuery` evoluciona con cada bloque (G/H/I/etc.), forzaría un bump por cada cambio.
- El SELECT es mucho más rico que un Expr — serializar el AST exhaustivo es costoso y frágil.
- Re-parsear en cada uso es **barato** comparado al I/O.
- Catálogo legible: futuras vistas `INFORMATION_SCHEMA.views` pueden mostrar el SQL del usuario.

Diferencia con L2: en L2 canonicalizamos vía `format_expr`. En V, **no canonicalizamos** — guardamos el SQL reconstruido tokens-a-tokens (whitespace simplificado pero tokens preservados). Un re-formatter completo de `SelectQuery` queda fuera de scope.

### 3. Expansión en FROM

Cuando una query referencia `FROM v` y `v` es una vista, el motor:

1. Hace `catalog.get_view(name)` en `Engine::expand_view_in_from`.
2. Re-parsea el `source` vía `parse_select_query_str`.
3. Lo embebe como `derived_source` del SelectStmt outer (reusando el camino que H armó para `FROM (SELECT ...) AS d`).
4. El planner downstream lo evalúa como derived table sin necesidad de un nuevo path.

**Limitación**: el source de la vista debe ser un `SelectQuery::Select` simple. Set operations (`UNION` / `INTERSECT` / `EXCEPT`) o `VALUES` como source quedan diferidos (`[GBY-4078]`). El campo `derived_source` del AST es `Option<Box<SelectStmt>>` — soportar set ops requeriría extenderlo a `Option<Box<SelectQuery>>`.

### 4. Protección contra ciclos

`Engine::view_expansion_depth` es un contador local de la sesión. Cada llamada a `expand_view_in_from` lo incrementa antes de re-parsear y lo decrementa después. Límite duro `MAX_VIEW_DEPTH = 32`. Cuando se excede → `[GBY-4076] VIEW_EXPANSION_DEPTH_EXCEEDED`. Cubre tanto el ciclo directo (`v → v`) como el indirecto (`A → B → A`).

### 5. Read-only

Las vistas son **read-only** en este release:

- `INSERT INTO v ...` → `[GBY-4075] VIEW_NOT_WRITABLE`.
- `UPDATE v SET ...` → idem.
- `DELETE FROM v` → idem.

Soporte de "updatable views" (con triggers de rewrite hacia la tabla base) es estándar de SQL pero conlleva una superficie no trivial: cuándo es "automáticamente updatable" (un solo `FROM`, sin agregados, sin DISTINCT, sin GROUP BY), cuándo requiere `INSTEAD OF triggers`. Diferido a un bloque dedicado.

### 6. `CREATE VIEW [IF NOT EXISTS] v [(col_aliases)] AS …`

Sintaxis estándar. `IF NOT EXISTS` es no-op si la vista ya existe; si el nombre choca con una **tabla** rebota incondicionalmente (la tabla no se sobreescribe). Los `column_aliases` se persisten en `ViewMeta.column_aliases` pero su aplicación al re-naming del result-set es **declarativa** en este release — la validación de arity contra el SELECT subyacente está, pero la sustitución efectiva de nombres queda como mejora futura (requiere wrap del SelectItem).

### 7. `DROP VIEW [IF EXISTS] v`

Path simétrico a DROP TABLE pero refusa si el nombre apunta a una tabla (sugerencia explícita de usar DROP TABLE). Sin `IF EXISTS` y vista inexistente → `[GBY-2001]`.

## 🚧 Consecuencias y limitaciones

| Tema | Estado |
|---|---|
| `CREATE VIEW v AS SELECT ...` | ✅ |
| `CREATE VIEW IF NOT EXISTS v AS ...` | ✅ |
| `CREATE VIEW v (a, b) AS SELECT x, y FROM t` | ✅ persistencia; aplicación efectiva del renaming = mejora futura |
| `CREATE VIEW v AS <UNION/INTERSECT/EXCEPT/VALUES>` | ❌ `[GBY-4078]` |
| `DROP VIEW [IF EXISTS] v` | ✅ |
| `SELECT FROM v` (single y multi-nivel) | ✅ con `MAX_VIEW_DEPTH = 32` |
| Vista con agregaciones / JOINs | ✅ (vía expansion como derived table) |
| Vista en JOIN del outer | ✅ (planner trata el alias como tabla virtual) |
| `INSERT` / `UPDATE` / `DELETE` sobre vista | ❌ `[GBY-4075]` |
| Updatable views (INSTEAD OF / auto-rewrite) | ❌ diferido a bloque dedicado |
| Materialized views | ❌ diferido |
| Migración V12 → V13 | Manual — dump SELECT + recreate con binario V13 |

## 🔄 Alternativas consideradas

- **Vistas con namespace separado (`views` distinto de `tables`)**: requeriría dos lookups en cada FROM resolver. El cost > benefit — namespace compartido + discriminator byte es estándar (Postgres, MySQL).
- **Persistir el AST del SELECT serializado**: rechazado por las razones de § 2.
- **Materialized views ahora**: requiere un caché invalidado por triggers de la tabla base. Es un bloque entero propio; diferido.
- **Updatable views automáticas**: implica un re-writer que sólo es trivial para vistas "simples" (single-table, no-agg, etc.). Diferido para no spawear un nuevo set de edge cases.

## 📚 Referencias

- [CHANGELOG.md — 2026-05-27 Bloque V](../../CHANGELOG.md)
- [MISSING_COMMANDS.md § Vistas](../MISSING_COMMANDS.md)
- [ADR-0021 — CHECK constraints (L2)](0021-check-constraints.md) — patrón de "persistir source SQL"
- [Error codes 4075–4078](../ERROR_CODES.md)
