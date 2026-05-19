# ADR-0017: Índice secundario INT-ordenado para range scan (VERSION 7)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-18
**Contexto que la motiva**: bloque "range scan por índice secundario" de Fase 2 → habilitar `WHERE col_idx BETWEEN a AND b` sin desarmar el resto del motor.

## 🧭 Contexto

ADR-0005 fijó el formato del índice secundario: **B+Tree con clave i64 = `FNV-1a-64(value_bytes)`**, buckets que listan `(value_bytes, pk)` para tolerar colisiones de hash. Esto cierra el caso de equality (`WHERE col = X`) en O(log N + bucket_size), que es lo que el bloque pedía en su momento.

Pero hash → range no se compone. Dos valores cercanos (`5` y `6`) tienen hashes arbitrariamente distintos. Cualquier intento de `WHERE col BETWEEN 5 AND 100` sobre el índice actual requiere visitar todos los buckets — o sea, full scan disfrazado. El ítem "range scan por índice secundario" del roadmap quedó marcado como **no viable con la estructura de ADR-0005**.

La salida natural es: para columnas donde el orden i64 ES el orden semántico, usar el valor directamente como clave del B+Tree en lugar del hash. Con `Tree::cursor_range(idx_root, from, to)` ya implementado (ADR-0008), eso da range scan en O(log N + k) donde k = filas en el rango.

Las columnas que satisfacen "orden i64 = orden semántico" son las **INT**. Las demás (TEXT, FLOAT, BOOL, DATE, DATETIME) requieren un encoder order-preserving distinto, idealmente un B+Tree byte-keyed — fuera de scope para este bloque.

Restricciones del proyecto:
- ADR-0001: cero deps.
- ADR-0005: no romper el formato hash existente — los call sites deben poder convivir.
- ADR-0009: memoria acotada — sin estructuras paralelas extra.
- VERSION 7 rechaza V6 limpiamente (mismo patrón que cada bump anterior).

## 💡 Decisión

Tres cambios en concierto, todos bajo VERSION 7:

### 1. `IndexKind` en `IndexMeta`

```rust
pub enum IndexKind {
    Hash,        // ADR-0005 — equality only
    OrderedInt,  // ADR-0017 — value-as-key, range-capable
}

pub struct IndexMeta {
    pub name: String,
    pub column: String,
    pub root_page: u32,
    pub unique: bool,
    pub kind: IndexKind,  // ← nuevo
}
```

Layout on-disk: un byte extra por índice tras `unique:u8`. V6 files se rechazan con el mensaje estándar "version=6 (expected 7). Re-create the database with the current binary."

`IndexKind::for_column(column_type)` decide automáticamente:
- `ColumnType::Int` → `OrderedInt`
- todo lo demás → `Hash`

### 2. Nuevos buckets ordenados en `src/index.rs`

Layout de un bucket OrderedInt:
```
[count:u16] + count × [pk:i64]
```

No hace falta guardar el valor en cada entry porque la clave del B+Tree **es** el valor. PKs almacenados en orden creciente para que `WHERE pk = …` short-circuits sigan siendo determinísticos.

Helpers:
- `ordered_int_key_from_value_bytes(&[u8]) -> Result<Option<i64>>` — convierte el `value_bytes` canónico a la clave i64; devuelve `None` para NULL (NULL no se indexa).
- `decode_ordered_bucket / encode_ordered_bucket / ordered_bucket_insert / ordered_bucket_remove / ordered_bucket_unique_conflict`.

### 3. Branching por `kind` en sql.rs

Toda función que tocaba un índice (`index_upsert_pk`, `index_remove_pk`, `check_unique_conflict`, `lookup_pks_via_index`, integrity check, FK cascade child lookup) ahora recibe `kind: IndexKind` o lee `idx.kind` y elige el camino correcto.

**Nuevo path: `lookup_pks_via_index_range(pager, idx, from, to)`** — usa `Tree::cursor_range(idx.root_page, from, to)` para iterar las claves en orden, decodifica cada bucket ordenado, devuelve los PKs.

Plan dispatch para `WHERE col BETWEEN a AND b`:
- columna == PK → `Plan::Range` (ya existente, sobre la tabla principal)
- columna con índice OrderedInt → `Plan::ByPks(lookup_pks_via_index_range(...))`
- columna con índice Hash → error claro: *"el índice secundario es hash-based (equality only). Solo columnas INT-indexadas admiten BETWEEN."*
- columna sin índice → error claro

### NULL handling

NULL **no se almacena** en índices OrderedInt:
- SQL `BETWEEN` ignora NULL por definición (NULL no es comparable) → coincide.
- UNIQUE permite múltiples NULLs → el bucket OrderedInt nunca ve NULL, así que múltiples NULLs en la columna no disparan conflict.
- `WHERE col = NULL` ya retornaba 0 filas en este motor — sigue igual.

Trade-off: si alguien quisiera `WHERE col IS NULL` acelerado por índice, no es posible con este layout. Aceptable — `IS NULL` no era parte del plan tampoco.

## 🤔 Alternativas evaluadas

1. **B+Tree byte-keyed para TODOS los índices**: el camino limpio que cubre TEXT/FLOAT/DATE además de INT. Pero requiere reescribir `bptree.rs` para clave `Vec<u8>` o introducir un B+Tree paralelo. Estimación: 800+ LOC, riesgo de regresión en hot path. **Diferido a un futuro bloque** cuando se necesite range sobre TEXT.

2. **Encoding order-preserving para FLOAT/DATE en i64**: posible (flip-sign trick para FLOAT, DATE ya es i64 internamente). Habría dado range scan adicional sin nueva estructura. **Diferido**: no resuelve TEXT y agrega complejidad de encoding por tipo. Cuando aparezca demanda concreta, se evalúa.

3. **Mantener todo en hash y agregar un segundo índice ordenado separado** cuando el usuario pide range: dos índices físicos por columna, dos veces el costo de mantenimiento. No.

4. **Índices compuestos en el mismo bump**: el ítem original del roadmap los agrupaba con range scan. Pero compuestos requieren claves multi-columna que con el approach value-as-i64 sería forzado (concatenar dos i64 → un solo i64 pierde información). Compuestos requieren prácticamente el mismo trabajo que un B+Tree byte-keyed. **Separados explícitamente** del bloque actual.

5. **Mantener el hash para INT también, agregar un range scan vía full-bucket sweep**: O(N) sobre el número de claves del índice, igual que un full scan. No aporta nada sobre la tabla.

## ✅ Consecuencias

**Positivas**:
- `WHERE col_idx BETWEEN a AND b` funciona en O(log N + k) para INT-indexed columns. Caso real (tabla de 1M filas, rango de 100 filas) cae de 4 KB×250K reads a ~log_2(250K) reads.
- Layout OrderedInt es estrictamente más pequeño que Hash para el mismo dato (sin value_bytes en cada entry).
- Convivencia limpia con Hash: ADR-0005 sigue válida para los tipos que no califican.
- Cero deps añadidas.
- Sin contaminar otras capas: storage, bptree, catalog (sólo +1 byte por IndexMeta), engine — todo conserva su contrato.
- 45 tests verde (43 previos + 2 nuevos cubriendo el path OrderedInt y el rechazo Hash).

**Negativas / a vigilar**:
- TEXT/FLOAT/BOOL/DATE/DATETIME indexados siguen siendo equality-only. El error message del engine apunta a este ADR para que el usuario sepa por qué.
- NULLs **no entran** al índice OrderedInt — semánticamente correcto pero diferente del Hash (donde NULL tenía bucket propio). Documentado.
- `idx.kind` queda como axis de variación en muchos helpers. Si en el futuro se agrega `OrderedText` (byte-keyed), todo el branching se reescribe — pero esa es la ocasión natural para limpiar la rama Hash si ya no aporta nada.
- V6 → V7 bump rompe compatibilidad. Igual que V5→V6 y V4→V5. Documentado en CHANGELOG, mismo patrón que siempre.

## 🔗 Referencias

- [src/index.rs](../../src/index.rs): nuevos helpers OrderedInt.
- [src/catalog.rs](../../src/catalog.rs): `IndexKind` + serialize/deserialize.
- [src/sql.rs](../../src/sql.rs): branching por kind en mantenimiento + nuevo `lookup_pks_via_index_range` + plan dispatch.
- [src/storage.rs](../../src/storage.rs): `VERSION = 7`.
- [tests/integration_test.rs](../../tests/integration_test.rs): `where_between_on_int_indexed_column_uses_ordered_index`, `where_between_on_text_indexed_column_is_rejected`.
- [ADR-0005](0005-secondary-index-bucket.md): el layout Hash original (sigue vigente para no-INT).
- [ADR-0008](0008-leaf-cursor-iterator.md): el cursor de range que esto explota.
