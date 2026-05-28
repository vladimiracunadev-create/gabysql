# ADR-0020: FK referential actions (`SET NULL` / `SET DEFAULT` / `ON UPDATE`) + UNIQUE multi-col table-level

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-27
**Bloque**: L1 (sub-bloque del bloque L del roadmap)
**Bump on-disk**: VERSION 8 → 9

## 🧭 Contexto

El roadmap [`MISSING_COMMANDS.md`](../MISSING_COMMANDS.md) marca como **P1** tres huecos del bloque L (constraints):

- `FOREIGN KEY ... ON DELETE SET NULL`
- `FOREIGN KEY ... ON DELETE SET DEFAULT`
- `FOREIGN KEY ... ON UPDATE ...`
- Multi-column `UNIQUE (a, b, ...)` table-level

Y como **P1** queda también `CHECK (expr)`. Este último abre una superficie distinta (parser de Expr persistida + evaluador en cada write) y se difiere a un sub-bloque propio **L2**, para mantener cada push focalizado y revisable.

Pre-L1:
- `OnDelete` admitía sólo `RESTRICT` y `CASCADE`. El parser rechazaba el resto con un mensaje genérico.
- `ForeignKeyMeta` no tenía `on_update`. Cualquier `ON UPDATE` en el SQL del usuario era un error de parse.
- `UNIQUE` table-level multi-col se podía expresar sólo vía `CREATE UNIQUE INDEX … ON t (a, b)` (K2). Era imposible declararlo dentro de `CREATE TABLE`.
- K2 entregó composite UNIQUE INDEX pero el **path de INSERT/UPDATE/DELETE chequeaba sólo la primera columna del bucket** — un hueco que no se notaba porque K2 sólo testeó la rama de backfill (`CREATE INDEX` después de cargar datos).

## 💡 Decisión

### 1. Extender `OnDelete` con `SetNull` y `SetDefault`

Códigos binarios estables y aditivos:

```rust
0 → Restrict       (default cuando se omite ON DELETE)
1 → Cascade
2 → SetNull        (L1)
3 → SetDefault     (L1)
```

V8 nunca pudo persistir 2/3 (el parser los rechazaba), así que la extensión es forward-compatible en lectura del byte.

### 2. Nuevo enum `OnUpdate` persistido como byte adicional por FK

```rust
0 → NoAction       (default cuando se omite ON UPDATE; equivale a ANSI/PostgreSQL)
1 → Cascade
2 → SetNull
3 → SetDefault
4 → Restrict
```

**Hoy `ON UPDATE` no se dispara**: gabysql prohíbe `UPDATE` sobre la PK del padre con `[GBY-4008] UPDATE_PK_NOT_ALLOWED`, así que no hay ocasión para que el motor evalúe la acción. Persistirla igual permite que un release futuro lift la restricción sin otro bump de formato.

### 3. Parser: `parse_fk_actions` reemplaza a `parse_on_delete`

Acepta `ON DELETE` y `ON UPDATE` en **cualquier orden**, cada uno una sola vez. Acciones soportadas idénticas en ambos:

```
RESTRICT | CASCADE | SET NULL | SET DEFAULT | NO ACTION
```

`NO ACTION` se acepta como sinónimo de `RESTRICT` (no hay constraint mode diferido todavía).

### 4. Cascade engine extendido

`delete_with_cascade` gana dos ramas:

- `SetNull` → llama a `cascade_set_fk_value(child_meta, child_pk, fk_col, Value::Null)`. Validación pre-write: si `child_col.not_null` ⇒ `[GBY-3009]` antes de tocar disco.
- `SetDefault` → busca el `DEFAULT` del child column. Sin DEFAULT ⇒ `[GBY-3010]`. Con DEFAULT `NULL` y columna NOT NULL ⇒ `[GBY-3002]`.

`cascade_set_fk_value` reusa el patrón del UPDATE normal: lee la fila, mantiene índices secundarios (single y compuestos), re-encodea, escribe.

### 5. UNIQUE table-level en CREATE TABLE

El parser reconoce `UNIQUE (col1, col2, ...)` como una claúsula al mismo nivel que `PRIMARY KEY (...)`:

```sql
CREATE TABLE t (
    id INT PRIMARY KEY,
    a INT NOT NULL,
    b INT NOT NULL,
    UNIQUE (a, b)
);
```

Y materializa el índice idéntico al de `CREATE UNIQUE INDEX uq_t_a_b ON t (a, b)` — reusa el mismo encoder de K2 (fingerprint FNV-1a-64 i64). Para multi-col se aplican las restricciones de K2: **all-INT NOT NULL** (`[GBY-4067]` si no se cumple).

Single-col `UNIQUE (col)` también se admite y es equivalente a UNIQUE inline.

### 6. Parche al composite UNIQUE de K2

L1 cierra un hueco que K2 dejó sin notar: el path de INSERT/UPDATE/DELETE iteraba `meta.indexes` usando sólo `idx.column`, omitiendo `extra_columns`. Resultado: un `UNIQUE (a, b)` se chequeaba contra el bucket de `a` solo, y rechazaba INSERTs legítimos por colisión del primer componente.

L1 introduce cuatro helpers que **separan claramente el camino composite** del single-column:

- `composite_fp_for_values(meta, idx, values) -> i64` — fingerprint para el row entero.
- `composite_unique_check(pager, idx, fp, exclude_pk)` — pre-check con exclusión propia (UPDATE).
- `composite_index_upsert(pager, root, fp, pk)` — bucket de PKs keyed por fp i64 directo (sin tag OrderedInt).
- `composite_index_remove(pager, root, fp, pk)` — counterpart del upsert.

El bucket key para composites es el **fingerprint i64 crudo** en el B+Tree — no la codificación OrderedInt con tag (`0x01 + i64`) que usa `index_upsert_pk`. Mantenemos ambos paths separados para no romper el contrato on-disk de los índices existentes.

## 🚧 Consecuencias y limitaciones

| Tema | Estado L1 |
|---|---|
| `ON DELETE SET NULL` / `SET DEFAULT` | ✅ Operativo |
| `ON DELETE NO ACTION` | ✅ Alias de `RESTRICT` |
| `ON UPDATE <action>` | ✅ Parsea y persiste; **no se dispara** (PK inmutable, `[GBY-4008]`) |
| `UNIQUE (a, b, ...)` table-level | ✅ Operativo (mismo encoder que K2) |
| Multi-col `FOREIGN KEY` | ❌ Sigue limitado a single-col (gap K2 sin cerrar) |
| `CHECK (expr)` | ❌ Diferido al sub-bloque **L2** |
| Migración V8 → V9 | Manual — dump SELECT + recreate |

## 🔄 Alternativas consideradas

- **Hacer ON UPDATE realmente activo**: requería permitir UPDATE sobre la PK, lo cual rompe el invariante de `[GBY-4008]` y abre un camino de mantenimiento de FK entrantes mucho más amplio (cada update del padre puede disparar cascades en N child tables). Se difiere a un release futuro con su propio ADR.
- **Almacenar CHECK como AST serializado en vez de texto SQL**: difícil de mantener round-trip-stable a medida que `Expr` evoluciona con cada bloque G/H/I. Y de todos modos L2 es un push aparte: la decisión queda allí.
- **Permitir CHECK en L1 con shortcut "sólo literal vs literal"**: tentador pero el 80% del valor de CHECK está en `CHECK (precio > 0)`, `CHECK (estado IN ('A','B','C'))`. Sin el evaluador completo de Expr, no vale la pena.

## 📚 Referencias

- [CHANGELOG.md — 2026-05-27 L1](../../CHANGELOG.md)
- [MISSING_COMMANDS.md § Constraints](../MISSING_COMMANDS.md)
- [ADR-0019 — Composite PK + Index (K2)](0019-composite-pk-and-index.md)
- [Error codes 3008/3009/3010](../ERROR_CODES.md)
