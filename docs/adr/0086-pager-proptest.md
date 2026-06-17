# ADR-0086: Property tests sobre el Pager / capa de transacciones

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Bloque:** Segundo nivel de la red de seguridad de hardening (después de M3 sobre planner)
**Origen:** [docs/TAREAS_PENDIENTES.md §3](../TAREAS_PENDIENTES.md) — "proptest sobre Pager" como item independiente de M3.
**Refina:** ADR-0084 (M3 — misma técnica zero-deps aplicada al planner).

## Contexto

M3 (ADR-0084) defendió la correctness del **planner cost-based**: la
invariante "el resultado de un SELECT no debe cambiar según si ANALYZE
corrió o no". Eso es la capa de arriba.

La capa de abajo — el Pager, que decide qué páginas leer/escribir y
maneja transacciones — también necesita una red de seguridad. Hoy
había 3 crash tests sintéticos (puntos elegidos por el dev) y eso es
todo. Si una secuencia rara de `begin/insert/commit/rollback` rompe
algo, no nos enteramos hasta que un usuario lo dispara.

Este push agrega property tests al nivel del Pager — mismo enfoque
hand-rolled zero-deps que M3.

## Decisión

Crear `tests/proptest_pager.rs` con 3 property tests sobre secuencias
random de operaciones DML + tx control. Cada uno verifica una
invariante distinta.

### Infraestructura compartida con M3

- LCG determinístico con mismas constantes que `gabybench` /
  `proptest_planner` / `fuzz_parser`.
- Cleanup explícito de `.db` + `.wal` por iteración (no leak de archivos
  en `temp_dir`).
- Cada falla imprime seed reproducible.

### Generador de ops

```rust
enum Op {
    Insert(i64, i64),  // (id, v)
    UpdateById(i64),   // SET v=v+1 WHERE id=...
    DeleteById(i64),   // DELETE WHERE id=...
}
```

IDs del pool `0..30` para garantizar overlap: updates y deletes deben
tener chance real de matchear inserts previos. Distribución: 60%
inserts, 20% updates, 20% deletes (ajustable).

### Modelo de referencia

`BTreeMap<i64, i64>` en Rust que replica la semántica del engine post-ANSI
fix (PK dup ignorada en insert, UPDATE/DELETE sobre fila inexistente
devuelve 0 filas). Se compara los IDs del engine vs los del modelo
después de cada commit.

### Tests

1. **`pager_commit_visibility_invariant`** (40 iters × 50 ops):
   - Aplica ops, commitea, reabre la DB, lee `SELECT id FROM t ORDER BY id`.
   - Debe matchear los IDs del modelo Rust.
   - Verifica `INTEGRITY CHECK` clean al final.
2. **`pager_rollback_discards_invariant`** (30 iters × (20 + 30 ops)):
   - Fase 1: commit con 20 ops → snapshot.
   - Fase 2: tx con 30 ops adicionales → ROLLBACK.
   - Reabre y verifica que el estado es EXACTAMENTE el snapshot pre-tx.
   - Verifica `INTEGRITY CHECK` clean al final.
3. **`pager_chained_tx_integrity_invariant`** (20 iters × 8 tx × 10 ops):
   - Chain de 8 transacciones random; 70% commit, 30% rollback (rng-decidido).
   - Modelo Rust solo aplica los ops de tx commiteadas.
   - Verificación intermedia después de cada tx (no solo al final).
   - `INTEGRITY CHECK` final.

Total: 40×50 + 30×(20+30) + 20×8×10 = 2000 + 1500 + 1600 = **5100 ops
random** ejercitados por corrida.

## Consecuencias

### Positivas

- **Capa de storage defendida automáticamente** contra secuencias raras.
  Antes el dev tenía que pensar "qué caso puede romper esto"; ahora
  miles de combinaciones se prueban cada CI run.
- **Reproducibilidad 1:1**: cada falla imprime seed → re-correr con
  `Lcg::new(seed)` da exactamente el mismo dataset y ops. Sin
  debugging-por-aproximación.
- **Verificación intermedia en chained**: detecta corrupción que solo
  emerge tras N ciclos commit/rollback intercalados, no solo al final.
- **Reusa la fix ANSI 2026-06-15** (UPDATE/DELETE sobre PK no-existe →
  0 filas): el modelo Rust replica esa semántica, sin la cual el match
  fallaría falsa-positivamente en cada UPDATE/DELETE de ID random.
- **Hermana de M3**: las dos capas (planner + storage) tienen el mismo
  shape de test. Fácil para futuros devs agregar más invariantes.

### Negativas / deuda

- **Sin crash recovery**: el test verifica commit/rollback *clean*. No
  ejercita `kill -9` mid-tx ni archivos `.wal` truncados. Los 3 crash
  tests sintéticos existentes siguen siendo las únicas defensas ahí.
- **Schema fijo** (`t(id PK, v)`): no varía estructura. Bugs que solo
  aparecen con tablas con muchas columnas / FKs / índices secundarios
  no se detectan.
- **No prueba concurrencia**: gabysql es single-writer (Mutex global +
  file lock); concurrencia inter-proceso se evalúa con otros tests
  específicos.
- **Costo de tiempo**: ~3-4 segundos en suite. Aceptable mientras no
  crezca; si se duplica, considerar mover el chained test (el más
  pesado) a `#[ignore]` + run nightly.

## Alternativas consideradas

1. **Usar `proptest` crate**. Mejor shrinking, mejor reporting, ergonomía
   más pulida. Choca con ADR-0001 (zero-deps core). Misma decisión que
   en M3.
2. **Incluir CREATE TABLE / DROP TABLE en el generador**. Espacio de
   ops más rico pero requiere modelo más sofisticado. Diferible:
   focusing first on DML over fixed schema.
3. **Tests separados por tipo de op** en vez de mezclados. Rechazado:
   bugs interesantes emergen de combinaciones (e.g. INSERT seguido de
   UPDATE seguido de DELETE sobre el mismo PK en la misma tx).

## Tests añadidos

- `pager_commit_visibility_invariant`
- `pager_rollback_discards_invariant`
- `pager_chained_tx_integrity_invariant`

**Suite total**: 813 → **816** (+3 tests, ~5100 ops random adicionales
por corrida).

## Referencias

- [ADR-0084 — M3 property tests sobre planner](0084-m3-proptest-planner.md) — hermana de este push.
- [ADR-0083 — ANSI fix UPDATE/DELETE](0083-ansi-update-delete-no-row-zero.md) — el modelo Rust replica esa semántica.
- [ADR-0001 — Zero deps core](0001-rust-zero-deps-core.md) — define la restricción del hand-roll.
- [TAREAS_PENDIENTES §3](../TAREAS_PENDIENTES.md) — declaraba este item como abierto post-M3.
