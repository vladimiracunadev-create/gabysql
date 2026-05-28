# ADR-0024: Activación real de `ON UPDATE` — UPDATE sobre PK + cascade

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-27
**Bloque**: Residual #4 del bloque L
**Bump on-disk**: ninguno (el byte `on_update` ya estaba persistido desde L1)

## 🧭 Contexto

L1 ([ADR-0020](0020-fk-referential-actions.md)) parseó `ON UPDATE <action>` y persistió el byte en cada FK record, pero el motor **nunca lo disparaba** porque la PK del padre era inmutable: cualquier `UPDATE` que tocara una columna PK rebotaba con `[GBY-4008] UPDATE_PK_NOT_ALLOWED`. El comentario del ADR-0020 era explícito: "se persiste para que un release futuro lift la restricción sin otro bump".

Este es ese release. Residual #4 lifta `[GBY-4008]` (en UPDATE — no en UPSERT DO UPDATE), implementa el move de la fila al nuevo PK, y dispara la acción declarada en cada FK entrante.

## 💡 Decisión

### 1. `[GBY-4008]` desaparece del path `exec_update`

Antes: cualquier assignment sobre una columna PK rebotaba al armar `expr_assignments`. Ahora la única regla que queda es "el SET no puede tener la misma columna dos veces" + "la columna debe existir" — mismas reglas que para una columna no-PK.

**No tocamos `ON CONFLICT DO UPDATE`**: ahí el `[GBY-4008]` se queda. Cambiar el PK durante un UPSERT rompería la identidad del row que disparó el conflicto, y un usuario que quiere ese efecto debería usar `INSERT ... ON CONFLICT (...) DO NOTHING` + un `UPDATE` separado.

### 2. Detección del PK move en `apply_update_to_pk`

Después de aplicar los overrides y construir la fila nueva, llamamos a `encode_row(meta, &current)` que devuelve `(encoded_pk, row_bytes)`. Si `encoded_pk == old_pk`, seguimos por el camino histórico (`upsert_row`). Si difiere, ramificamos a `move_row_and_cascade_on_update`.

Esto es robusto tanto para PK single como compuesta: `encode_row` ya computaba el fingerprint K2 para PK compuesta antes (era la PK del row); la única diferencia es que antes el resultado siempre coincidía con el PK del WHERE.

### 3. `move_row_and_cascade_on_update`

La función orquesta el move en 4 pasos, en orden estricto para no dejar estado parcial:

1. **Duplicate guard**: `get_row(new_pk)` en este tabla → si existe, `[GBY-3001] DUPLICATE_PRIMARY_KEY`. Como `new_pk != old_pk`, esta lookup no devuelve la propia fila.
2. **ON UPDATE cascade**: snapshot del catálogo, walk cada tabla buscando FKs cuyo `fk.table == meta.name`. Para cada FK:
   - Construye los target values OLD (del `old_row`) y NEW (del `current`).
   - Si `OLD == NEW`, no-op (la PK cambió pero estas columnas target específicas no).
   - Si hay children con OLD target values, aplica `fk.on_update`:
     - **CASCADE**: `cascade_set_fk_tuple(child, child_pk, source_cols, new_target_values)` por cada child PK.
     - **SET NULL**: mismo path con `[Value::Null; N]`. Si alguna source col es NOT NULL → `[GBY-3009]` antes de tocar disco.
     - **SET DEFAULT**: arma el vector de defaults; falla con `[GBY-3010]` si falta DEFAULT en alguna source. DEFAULT NULL + NOT NULL → `[GBY-3002]`.
     - **RESTRICT / NO ACTION**: `[GBY-4073] FK_RESTRICT_BLOCKS_UPDATE`. NO ACTION se trata como RESTRICT (mismo trato que en `ON DELETE`).
3. **Data move**: `delete_row(old_pk)` + `insert_row(new_pk, new_bytes)`. NO usamos `upsert` porque la key cambia; necesitamos un delete real seguido de insert para que el B+Tree mueva la entrada.
4. **Index maintenance**: para cada índice secundario (composite o single), remove old (con old row values y old_pk) + insert new (con new row values y new_pk). A diferencia del UPDATE estable, acá actualizamos TODOS los índices, no solo los tocados por overrides — porque el PK cambió y el bucket guarda el PK junto al value.

### 4. Caso degenerado: cascade afecta a la PK del child

Si el child usa la columna source de la FK también como su propia PK (e.g. `CREATE TABLE child (id INT PRIMARY KEY REFERENCES parent (id) ON UPDATE CASCADE)`), una cascade CASCADE/SET NULL/SET DEFAULT mutaría la PK del child. Eso forzaría una cascada de PK moves encadenadas, que este release no soporta.

Detección: durante el walk de cascade, si `fk.source_columns(...)` incluye alguna PK col del child Y la acción requiere mutación, rebotamos con `[GBY-4074] FK_UPDATE_CASCADE_AFFECTS_CHILD_PK` antes de tocar nada.

### 5. ON UPDATE es no-op si la columna target no cambió

Si el `UPDATE parent SET label = ...` no toca columnas que son target de alguna FK, no disparamos cascade — ni siquiera RESTRICT rebota. La regla queda alineada con SQL estándar: `ON UPDATE RESTRICT` sólo aplica cuando el VALOR target cambia.

## 🚧 Consecuencias y limitaciones

| Tema | Estado |
|---|---|
| `UPDATE t SET pk_col = x WHERE ...` single-col PK | ✅ |
| `UPDATE t SET a = ..., b = ... WHERE ...` con PK compuesta `(a, b)` | ✅ |
| `ON UPDATE CASCADE` single-col y multi-col | ✅ |
| `ON UPDATE SET NULL` / `SET DEFAULT` | ✅ con [GBY-3009] / [GBY-3010] / [GBY-3002] |
| `ON UPDATE RESTRICT` / `NO ACTION` (default) | ✅ → [GBY-4073] |
| ON UPDATE no-op cuando la columna target no cambia | ✅ |
| Cascade donde source col también es PK del child | ❌ rebota con [GBY-4074] |
| `INSERT ... ON CONFLICT DO UPDATE SET pk_col = ...` | ❌ sigue rebotando con [GBY-4008] (intencional) |
| UPDATE de PK que colisiona con otra fila | ❌ [GBY-3001] DUPLICATE_PRIMARY_KEY |

## 🔄 Alternativas consideradas

- **Cascadear PK moves cuando la cascade afecta al PK del child**: en teoría es lo que ANSI exige (`ON UPDATE CASCADE` debería ser transitivo). En la práctica abre un grafo de moves donde cada child puede tener sus propios FK entrantes que requieren otra cascade. Diferido a un release posterior con su propio ADR — por ahora, error claro con [GBY-4074].
- **Permitir UPDATE de PK en UPSERT DO UPDATE**: descartado por consistencia semántica. El `(id)` después de `ON CONFLICT` identifica el row que disparó el conflicto; cambiar ese row's PK durante el handler haría inconsistente el reporte (`OK 1 fila`: cuál?).
- **Move via `upsert(new_pk)` + `delete(old_pk)` después**: el orden importa cuando otra operación lee la tabla concurrentemente, y aunque el motor es single-writer hoy, evitamos depender de ese contrato — `delete(old)` + `insert(new)` es el orden natural.

## 📚 Referencias

- [CHANGELOG.md — 2026-05-27 residual #4](../../CHANGELOG.md)
- [ADR-0020 — FK referential actions (L1)](0020-fk-referential-actions.md) (donde nació el byte `on_update`)
- [ADR-0023 — Multi-col FOREIGN KEY (residual #3)](0023-multi-col-foreign-key.md)
- [Error codes 4008 (legacy), 4073, 4074](../ERROR_CODES.md)
