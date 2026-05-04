# ADR-0005: Bucket layout para índices secundarios + tolerancia a colisiones

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-04
**Contexto**: implementación de índices secundarios. Bump VERSION 3 → 4.

## 🧭 Contexto

Un índice secundario sobre una columna no-PK necesita mapear el **valor de la columna → la lista de PKs** que lo contienen. Hay dos restricciones simultáneas:

1. La clave del B+Tree tiene que ser `i64` (firma actual del módulo `bptree`).
2. Los valores indexables son escalares de varios tipos: INT, TEXT, BOOL, FLOAT, DATE, DATETIME.

La opción "natural" sería usar el valor serializado como clave directa, pero eso requeriría que el B+Tree tuviera claves de longitud variable, lo cual implica un refactor mayor.

## 💡 Decisión

El índice secundario es un **B+Tree paralelo** cuya:

- **Clave** es el `FNV-1a-64(value_bytes)` — un `i64` derivado del valor canonical-encoded.
- **Valor** es un *bucket*: lista de tuplas `(value_bytes, pk)`.

Layout del bucket:

```
[count:u16] + count × ([vlen:u16][value_bytes][pk:i64])
```

El bucket tolera dos casos:

1. **Colisión de hash**: dos valores distintos que hashean igual. El bucket distingue por `value_bytes` exactos en lookup.
2. **Valores duplicados**: dos filas con el mismo valor en la columna indexada. El bucket guarda ambas tuplas con sus PKs distintos.

Operaciones:
- `bucket_insert`: idempotente — re-insertar el mismo `(value, pk)` no agrega duplicado.
- `bucket_remove`: O(N) sobre el bucket; si queda vacío, la entrada del B+Tree se elimina.
- `bucket_lookup`: filtra `(v, _)` cuyos `v == query_value_bytes`.

## 🔄 Alternativas consideradas

- **Refactorizar `bptree` para soportar claves de longitud variable**: rechazado por costo y riesgo (rompe ADR-0004).
- **Crear un B+Tree distinto por tipo de columna**: rechazado por inflación de código.
- **Usar el hash sin tolerancia a colisiones (un solo `(value, pk)` por entrada)**: rechazado — frágil ante colisiones FNV-1a, que existen aunque sean raras en strings cortos.
- **Separate index para duplicados (overflow page)**: rechazado por complejidad; el bucket inline cubre el caso común.

## 📊 Consecuencias

**Positivas**:
- Reusa el `bptree` existente sin refactor.
- Lookup `WHERE col = val` es O(log N) descenso al bucket + O(B) filtro por bytes (donde B es la cardinalidad del bucket — típicamente pequeño).
- El mismo bucket sirve para guardar duplicados (índice no-`UNIQUE`) y para tolerar colisiones de hash.
- Backfill de un índice nuevo es O(N) sobre la tabla — un solo scan.

**Negativas**:
- Si una columna tiene **millones de filas con el mismo valor**, el bucket explota a un solo leaf grande. Caso patológico documentado; el límite de 4096 bytes por página y la restricción `count ≤ u16::MAX` lo hacen quebrarse de forma controlada.
- El índice no soporta **range scan** (`WHERE col BETWEEN ...`) porque las claves del B+Tree son hashes, no los valores ordenados. Esto está reconocido como limitación y queda para un futuro índice ordenado en el [Camino A](../COMMERCIAL_ROADMAP.md).
- `JSON` no es indexable porque no hay semántica canónica de igualdad (dos representaciones distintas pueden ser equivalentes).

**Neutras**:
- Una sola columna por índice (no compuestos) es restricción independiente, no consecuencia de esta decisión.

## 🔗 Referencias

- Commit: `a2e925f` (groundwork) + `e51e4a0` + `a6c5998` + `ac40473`.
- Implementación: [src/index.rs](../../src/index.rs), [src/sql.rs:lookup_pks_via_index](../../src/sql.rs).
- Test: [tests/integration_test.rs::secondary_index_lookup_and_maintenance](../../tests/integration_test.rs) — backfill de 200 filas, INSERT/UPDATE/DELETE con maintenance, DROP INDEX.
- CHANGELOG: entrada 2026-05-04.
