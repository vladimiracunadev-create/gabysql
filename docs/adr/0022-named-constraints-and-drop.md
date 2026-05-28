# ADR-0022: Nombres explícitos en PK/UNIQUE/FK + `ALTER TABLE DROP CONSTRAINT`

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-27
**Bloque**: Residual #2 del bloque L
**Bump on-disk**: VERSION 10 → 11

## 🧭 Contexto

L2 cerró `CHECK (expr)` con soporte para `CONSTRAINT <name> CHECK (...)` — el único constraint que el parser admitía nombrar. PRIMARY KEY, UNIQUE y FOREIGN KEY se declaraban sin nombre, lo cual:

- Hacía imposible borrar un constraint específico (no había `ALTER TABLE DROP CONSTRAINT <name>`).
- Producía mensajes de error con nombres auto-sintetizados (`uq_t_email`) que no le decían nada al usuario que escribió otro nombre en sus dumps.
- Bloqueaba la compatibilidad con scripts de Postgres/MySQL que casi siempre nombran sus constraints.

Este residual cierra los tres frentes en un solo push:

1. Parser admite `CONSTRAINT <name>` antes de `PRIMARY KEY`, `UNIQUE` y `FOREIGN KEY` table-level.
2. Catálogo persiste el nombre.
3. `ALTER TABLE DROP CONSTRAINT [IF EXISTS] <name>` busca por nombre y descarta la entrada apropiada.

## 💡 Decisión

### 1. Persistencia: dos slots opcionales en TableMeta

Aprovechamos que el formato ya tenía dos huecos naturales:

- **PK name** vive en `TableMeta` como `pub primary_key_name: Option<String>`. Se persiste con un byte presente/ausente seguido del string si presente, justo después de las columnas PK.
- **FK name** vive en `ForeignKeyMeta` como `pub name: Option<String>`. Mismo esquema present-byte tras `on_update`.
- **UNIQUE name** **no requiere campo nuevo** — `IndexMeta.name` ya existe (lo usaba el auto-naming `uq_<table>_<col>` desde V5). Sólo cambia quién lo provee: el usuario en vez del motor.

Bump VERSION 10→11. V10 files rechazados con `[GBY-1003]`.

### 2. Parser: helper unificado `try_match_named_table_constraint_head`

CHECK ya tenía su propio path (`try_match_table_constraint_check_head`) porque su cuerpo es un `Expr` y no una lista de columnas. Para los otros tres, agregamos un helper que:

- Detecta `CONSTRAINT <ident> <kind>` (kind ∈ {PRIMARY KEY, UNIQUE, FOREIGN KEY}).
- Si matchea, consume los 3-4 tokens iniciales y devuelve `Some(NamedConstraintHead { name, kind })`.
- Si después de `CONSTRAINT <ident>` no viene ninguno de los tres, hace rollback de `self.pos` y devuelve `None`.

El caller dispatchea sobre `kind`. Cuerpo:

- `PRIMARY KEY (cols)` → setea `table_level_pk` + `table_level_pk_name`.
- `UNIQUE (cols)` → empuja a `table_level_named_unique` (Vec de `(name, cols)`).
- `FOREIGN KEY (col) REFERENCES t (col) [ON ...]` → empuja a `table_level_named_fks`. **Multi-col FK explicitly rejected** con mensaje que apunta al residual #3.

### 3. Ejecutor: rama dedicada en `exec_create`

`stmt.named_unique_constraints` se materializa igual que `stmt.unique_constraints` (mismo encoder K2), con la única diferencia de que `IndexMeta.name = supplied`. Validación adicional: colisión de nombre contra los índices ya creados (inline UNIQUE + table-level sin nombre) emite `[GBY-2005] INDEX_ALREADY_EXISTS`.

`stmt.named_foreign_keys` se adjunta a la columna correspondiente del Vec — el `Column.references` toma el nombre. Validaciones: columna debe existir, no debe ya tener FK inline, nombre no debe colisionar con otra FK nombrada. Re-corre `validate_fk_targets` después de adjuntar.

### 4. `ALTER TABLE DROP CONSTRAINT [IF EXISTS] <name>`

Nueva statement `AlterDropConstraintStmt { table, name, if_exists }`. El executor lookupea case-insensitive en este orden:

1. **PK**: si `meta.primary_key_name` matchea → rechazo con `[GBY-4072] CANNOT_DROP_PRIMARY_KEY_CONSTRAINT`. Antes de cualquier otra rama porque la PK podría sintácticamente colisionar con otros nombres y queremos un error específico.
2. **CHECK constraints**: `meta.check_constraints.remove(idx)`. Único cambio en TableMeta + persist.
3. **UNIQUE index**: busca en `meta.indexes` por nombre. Si encuentra y `unique=true`, lo remueve (la página queda leaked — mismo contrato que `DROP INDEX`). Si encuentra pero NO es UNIQUE, rebota con `[GBY-4071]` y mensaje sugiriendo `DROP INDEX`.
4. **FK con nombre**: itera `meta.columns[*].references` buscando `references.name`. Si matchea, setea `column.references = None`.

Sin match en ninguno → `[GBY-4071] CONSTRAINT_NOT_FOUND` con un breakdown de cuántos constraints existen (CHECK, UNIQUE, FK con nombre) para ayudar al diagnóstico. Con `IF EXISTS` se silencia y se devuelve OK no-op.

## 🚧 Consecuencias y limitaciones

| Tema | Estado |
|---|---|
| `CONSTRAINT <name> PRIMARY KEY` table-level | ✅ |
| `CONSTRAINT <name> UNIQUE` table-level (single o multi-col) | ✅ |
| `CONSTRAINT <name> FOREIGN KEY` table-level (single-col) | ✅ |
| `CONSTRAINT <name> FOREIGN KEY` multi-col | ❌ rechazado con mensaje → residual #3 |
| `CONSTRAINT <name>` inline en columna (e.g. `email TEXT CONSTRAINT uq_email UNIQUE`) | ❌ sólo column-level CHECK con nombre (L2); UNIQUE/FK inline con nombre queda para otra entrega |
| `ALTER TABLE DROP CONSTRAINT <name>` para CHECK | ✅ |
| `ALTER TABLE DROP CONSTRAINT <name>` para UNIQUE | ✅ (cualquier UNIQUE INDEX, no sólo los declarados con `CONSTRAINT`) |
| `ALTER TABLE DROP CONSTRAINT <name>` para FK | ✅ sólo si fue nombrada |
| `ALTER TABLE DROP CONSTRAINT <name>` para PK | ❌ rechazo con `[GBY-4072]` (PK inmutable) |
| `ALTER TABLE DROP CONSTRAINT IF EXISTS <name>` | ✅ no-op silencioso si no existe |
| Auto-rename de FK al `ALTER TABLE RENAME COLUMN` | ✅ inherited (el rename mantiene `references.name`) |
| Migración V10 → V11 | Manual — dump SELECT + recreate |

## 🔄 Alternativas consideradas

- **Tabla separada `constraints` en el catálogo**: hubiera unificado el lookup de DROP CONSTRAINT en un solo punto, pero rompe la localidad de las refs (cada constraint es lógicamente parte del schema de su tabla) y agrega un round-trip por DDL. Se descartó.
- **Bump conjunto V10→V11 que también arregle residuales #3 y #4**: tentador pero los tres residuales tocan zonas muy distintas (catálogo, executor, evaluación de PK mutation). Mantener cada residual en su push deja diffs revisables.
- **`DROP CONSTRAINT` con cascade**: si la UNIQUE que se droppea es referenciada por una FK como parent column, podríamos cascadear. Como las FKs apuntan exclusivamente a PK en este release, el caso es nulo en práctica. Reservado para cuando aterricen FKs apuntando a UNIQUE arbitrarias.

## 📚 Referencias

- [CHANGELOG.md — 2026-05-27 residual #2](../../CHANGELOG.md)
- [MISSING_COMMANDS.md § Constraints](../MISSING_COMMANDS.md)
- [ADR-0021 — CHECK constraints (L2)](0021-check-constraints.md)
- [Error codes 4071, 4072](../ERROR_CODES.md)
