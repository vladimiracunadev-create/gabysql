# ADR-0023: Multi-column FOREIGN KEY (`FK (a, b) REFERENCES p (x, y)`)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-27
**Bloque**: Residual #3 del bloque L
**Bump on-disk**: VERSION 11 → 12

## 🧭 Contexto

K2 (ADR-0019) entregó PRIMARY KEY compuesta y `CREATE [UNIQUE] INDEX … (a, b, …)` via fingerprint FNV-1a-64 i64. Residual #2 cerró nombres en PK/UNIQUE/FK + `DROP CONSTRAINT`. Pero `FOREIGN KEY` seguía limitada a single-column — un agujero notorio porque:

- Tablas con PK compuesta (K2) no podían ser referenciadas desde otra tabla. El workaround era surrogate INT + UNIQUE compuesta, lo cual duplica almacenamiento e índices.
- Dumps de Postgres con FKs compuestos rebotaban al importar.
- ANSI exige soporte multi-col para FK como nivel core de SQL-92.

## 💡 Decisión

### 1. Persistencia: anchor en `Column.references`, extras en dos Vec paralelos

Mantenemos `Column.references: Option<ForeignKeyMeta>` (la FK queda anchored en la primera columna source) y agregamos dos campos:

```rust
pub struct ForeignKeyMeta {
    pub table: String,                       // target table
    pub column: String,                      // first target column
    pub on_delete: OnDelete,
    pub on_update: OnUpdate,
    pub name: Option<String>,
    pub extra_source_columns: Vec<String>,   // additional child columns
    pub extra_target_columns: Vec<String>,   // additional parent columns
}
```

Single-col FK → ambos Vec vacíos (idéntico al record pre-#3). Multi-col `(a,b) REFERENCES p(x,y)` → anchored en `a`/`x`, `extra_source=[b]`, `extra_target=[y]`.

**Por qué anchor en lugar de moverlo a TableMeta**: hubiera roto la ergonomía del código que itera `meta.columns` con `column.references`. Mantenerlo anchored deja todo ese código funcional con el cambio mínimo posible (cada uso simplemente extrae `source_columns(&column.name)` cuando necesita la lista completa).

Bump VERSION 11→12. V11 files rechazados con `[GBY-1003]`.

### 2. Lookup vía fingerprint compuesto (reuso del encoder K2)

El insight clave: el parent persistió su PK compuesta como `encode_composite_key(pk_cols, pk_values)` → i64 fingerprint que ES la clave del B+Tree del parent (K2). Para chequear `(pa, pb) ∈ parent`, computamos `fp = encode_composite_key(parent_pk_cols, [pa, pb])` y hacemos `parent.get_row(parent.root_page, fp)`. Mismo lookup O(log n) que single-col, sólo cambia cómo se construye la clave.

Para que esto funcione, **forzamos que los target_columns de la FK sean exactamente la PK compuesta del parent**, en el mismo orden. El validator DDL rechaza cualquier desviación (subconjunto, reorder, columnas no-PK). Una FK contra `UNIQUE` arbitrario queda fuera de scope — el day la features pague el costo, agregaremos lookup via secondary index.

### 3. Parser: arity validation y anchor split

`try_match_named_table_constraint_head` ya consumía `CONSTRAINT <name> FOREIGN KEY`. El cuerpo en residual #2 leía 1 col + REFERENCES + 1 col. Lo extendí a `(col [, col ...])` en ambos lados con validación de arity. Después de parsear, splittea la primera columna en el anchor y empuja el resto a `extra_source_columns` / `extra_target_columns`.

Column-inline `id INT REFERENCES p(x)` sigue siendo single-col (no hay forma de escribir multi-col inline — ANSI tampoco lo permite).

### 4. Runtime: helpers compartidos entre single y multi-col

Antes residual #3, las funciones FK trabajaban con `target_pk: i64` y `column_name: &str` (single).
Ahora trabajan con tuplas de Values en el orden source-target alineado:

- `fk_lookup_parent_pk(fk, source_values, parent_meta) → Option<i64>`: NULL en cualquier source → `None` (ANSI: FK con NULL pasa). Si el target es la PK compuesta del parent, computa el fingerprint; si es single-col, usa el INT directo.
- `check_fk_value(pager, meta, anchor_col, fk, source_values, self_ref_allowed_pk)`: el bottleneck — para insert/update.
- `collect_fk_source_values(fk, anchor_col, row) → Vec<Value>`: recolecta los valores source en el orden declarado.
- `find_child_pks_with_fk_value(pager, child, fk, anchor_col, target_values)`: para cascade. Single-col mantiene el fast-path con índice secundario; multi-col cae a full-scan comparando tuplas (PostgreSQL hace lo mismo cuando no hay índice por las source cols).
- `cascade_set_fk_tuple(pager, child, child_pk, column_names, new_values)`: mutación atómica de N columnas + mantenimiento de índices afectados + CHECK eval (L2). Reemplaza el `cascade_set_fk_value` single-col, que queda como wrapper de compatibilidad.

### 5. DDL housekeeping

- `validate_fk_targets`: ahora compara `fk.target_columns()` contra `parent.pk_columns()` y exige match exacto en orden. Tipos por par también.
- `ALTER TABLE DROP COLUMN`: rechaza con `[GBY-4061]` si la columna participa en cualquier `extra_source_columns` (saliente) o `extra_target_columns` (entrante) además del anchor/target principal.
- `ALTER TABLE RENAME COLUMN`: arrastra el rename a `extra_source_columns` propios y a `extra_target_columns` de FKs entrantes desde otras tablas. La PK compuesta sigue funcionando porque K2 ya arrastraba `primary_key_extra` y la lista de índices.

### 6. Formato on-disk V12

Trailer nuevo en cada FK record, sólo cuando `flags & HAS_FK`:

```
[fk_extra_count:u8]
[extra_source_col_1] … [extra_source_col_N]    ← N strings
[extra_target_col_1] … [extra_target_col_N]    ← N strings
```

Single-col FK → count=0, payload vacío.

## 🚧 Consecuencias y limitaciones

| Tema | Estado |
|---|---|
| `FOREIGN KEY (a, b) REFERENCES p (x, y)` table-level | ✅ |
| `CONSTRAINT name FOREIGN KEY (...)` con multi-col + nombre | ✅ |
| `ON DELETE CASCADE / SET NULL / SET DEFAULT / RESTRICT` multi-col | ✅ |
| FK multi-col target **debe** ser la PK compuesta del parent | ✅ (otro UNIQUE arbitrario no soportado) |
| Column-inline multi-col FK | ❌ (ANSI tampoco lo permite) |
| Index fast-path para cascade multi-col | ❌ — full scan; mejora futura usando índice UNIQUE compuesto sobre las source cols si existe |
| `ALTER TABLE DROP COLUMN` que participa en FK multi-col → `[GBY-4061]` | ✅ |
| `ALTER TABLE RENAME COLUMN` arrastra rename a FKs multi-col | ✅ |
| Migración V11 → V12 | Manual — dump SELECT + recreate |

## 🔄 Alternativas consideradas

- **Mover FKs de `Column.references` a `TableMeta.foreign_keys: Vec<…>`**: semánticamente más limpio (una FK no "pertenece" a una columna) pero requiere reescribir TODO el código que itera `meta.columns` buscando FKs (varios miles de líneas). Anchor + extras es el cambio mínimo.
- **Hash compuesto sobre los source cols como índice automático**: K2 ya tiene `CREATE INDEX … (a, b)`. Podríamos crear uno transparente al declarar FK multi-col, pero (a) inflaría DDL silenciosamente, (b) el usuario puede crearlo a mano cuando lo necesite. Diferido.
- **FK contra UNIQUE compuesto** (no sólo PK): requiere lookup via índice secundario en lugar del PK del padre. Posible y útil, pero abre un eje de diseño nuevo (cómo manejar `ON DELETE` cuando el padre tiene múltiples filas con la misma combinación UNIQUE pero distintas PKs — no debería ocurrir si UNIQUE está enforcado, pero el motor tendría que asumirlo en runtime). Diferido.

## 📚 Referencias

- [CHANGELOG.md — 2026-05-27 residual #3](../../CHANGELOG.md)
- [MISSING_COMMANDS.md § Constraints](../MISSING_COMMANDS.md)
- [ADR-0019 — Composite PK + Index (K2)](0019-composite-pk-and-index.md)
- [ADR-0022 — Named constraints + DROP CONSTRAINT (residual #2)](0022-named-constraints-and-drop.md)
