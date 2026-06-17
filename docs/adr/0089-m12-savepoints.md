# ADR-0089: M12 — SAVEPOINT / ROLLBACK TO SAVEPOINT / RELEASE

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Bloque:** M12 (TCL — recuperación parcial dentro de transacción)
**Origen:** [docs/TAREAS_PENDIENTES.md §6.5](../TAREAS_PENDIENTES.md) — declarado como pre-requisito de M13 (cross-request tx en server HTTP).
**Refina:** Bloque T (BEGIN/COMMIT/ROLLBACK).

## Contexto

Antes de M12, gabysql tenía solo el track básico: `BEGIN`/`COMMIT`/`ROLLBACK`.
Si hacías 50 INSERTs y el #30 fallaba, `ROLLBACK` descartaba los 50.

`SAVEPOINT` es estándar SQL (ANSI SQL:2003, soportado por PostgreSQL,
SQLite, MySQL, Oracle). Permite marcar checkpoints dentro de una
transacción y revertir parcialmente. Cualquier cliente serio (ORMs,
herramientas de migración, frameworks de testing) lo asume disponible.

Hasta hoy, intentar `SAVEPOINT` fallaba con `[GBY-4029]
TX_BEGIN_DOUBLE` (el mensaje incluso decía "savepoints aún no
soportados"). Esta entrega lo soporta.

## Decisión

3 statements SQL nuevos + soporte en el Pager + dispatch en el Engine.

### SQL aceptado

```sql
SAVEPOINT my_point                              -- marca checkpoint
ROLLBACK TO [SAVEPOINT] my_point                -- revierte hasta el checkpoint
RELEASE [SAVEPOINT] my_point                    -- libera el checkpoint
```

`SAVEPOINT` es token obligatorio en `SAVEPOINT name`. En `ROLLBACK TO`
y `RELEASE` la palabra `SAVEPOINT` es opcional (compatibilidad con
PostgreSQL).

### Semántica

- **`SAVEPOINT name`**: pushea un snapshot completo del cache de
  páginas + header. Cero efecto sobre la transacción — solo
  bookkeeping interno. `[GBY-4143]` si no hay `BEGIN` activo.
- **`ROLLBACK TO SAVEPOINT name`**: busca el savepoint más reciente con
  ese nombre, descarta cualquier savepoint declarado DESPUÉS, y
  restaura el cache + header desde el snapshot. El savepoint **sigue
  en la stack** (semántica ANSI: ROLLBACK TO no libera). `[GBY-4144]`
  si el nombre no existe, `[GBY-4143]` si no hay tx.
- **`RELEASE SAVEPOINT name`**: pop del savepoint + todos los
  posteriores. NO revierte cambios — los inserts entre el savepoint y
  el RELEASE permanecen. `[GBY-4144]` / `[GBY-4143]` igual que arriba.
- **`COMMIT`** y **`ROLLBACK`** (full): limpian la stack de
  savepoints automáticamente.

### Implementación

**Pager (`src/storage.rs`)**:

```rust
struct Savepoint {
    name: String,
    header: Header,
    cache_snapshot: HashMap<u32, CachedPage>,
}

pub struct Pager {
    // ... campos existentes ...
    savepoints: Vec<Savepoint>,
}

impl Pager {
    pub fn savepoint(&mut self, name: String) -> DbResult<()> { ... }
    pub fn rollback_to_savepoint(&mut self, name: &str) -> DbResult<()> { ... }
    pub fn release_savepoint(&mut self, name: &str) -> DbResult<()> { ... }
}
```

**PageCache (`src/storage.rs`)**: nuevos `full_snapshot()` (clone de
todas las páginas, dirty y clean, con su flag) y `restore_snapshot()`
(reemplaza contenido del cache).

**Engine (`src/sql.rs`)**: 3 nuevas variantes en `enum Statement`
(`Savepoint(String)`, `RollbackToSavepoint(String)`,
`ReleaseSavepoint(String)`) + dispatch + 3 `exec_*` methods que
verifican `explicit_tx` y forwardean al Pager.

**Parser**: 5 lookaheads adicionales:

- `SAVEPOINT name` → `Statement::Savepoint`.
- `RELEASE [SAVEPOINT] name` → `Statement::ReleaseSavepoint`.
- `ROLLBACK TO [SAVEPOINT] name` → `Statement::RollbackToSavepoint`
  (insertado antes del path `ROLLBACK [TRANSACTION/WORK]` existente).

**Error codes nuevos** (`src/errors.rs`):

- `SAVEPOINT_OUTSIDE_TX = 4143`.
- `SAVEPOINT_NOT_FOUND = 4144`.

### Costo de memoria

`full_snapshot()` clona TODO el cache. Con cache default de 1024
páginas × 4 KB = **~4 MB por savepoint**. Para un workload típico
(2–5 savepoints simultáneos), ~10–20 MB extra. Aceptable.

Optimización futura: undo log por-página en vez de snapshot completo.
Cuesta sustancialmente más código; diferible hasta que la memoria se
vuelva un cuello.

## Consecuencias

### Positivas

- **Recuperación parcial dentro de tx**: cualquier flujo "intenta N
  ops, si una falla revertí solo eso" ahora funciona. ORMs, runners
  de tests, scripts de migración pueden usar `SAVEPOINT`.
- **Desbloquea M13** (cross-request tx en server HTTP). M13 requiere
  que el servidor pueda dar checkpoints dentro de una tx larga; M12
  es el bloque que faltaba.
- **Standard-compliant**: misma sintaxis que PostgreSQL/SQLite/MySQL.
  Sin sorpresas al portar SQL externo.
- **Loop natural con ANSI fix** (ADR-0083): `UPDATE/DELETE WHERE pk
  no-existe` devuelve 0 filas en vez de error, y si quisieras
  cancelar selectivamente operaciones que sí erran (e.g. PK
  duplicada), `SAVEPOINT + try + ROLLBACK TO` ahora es la
  herramienta canónica.

### Negativas / deuda

- **Snapshot full por savepoint**: ~4 MB cada uno. Workloads con
  cientos de savepoints simultáneos sufrirán. La doc lo explicita.
- **No persiste en WAL**: si el proceso crashea mid-tx, los
  savepoints se pierden (igual que el resto de la tx). Aceptable —
  son in-memory por definición.
- **Single-writer no cambia**: gabysql sigue siendo single-writer
  global. Savepoints viven dentro de UNA tx; no abren la puerta a
  concurrencia.

## Alternativas consideradas

1. **Undo log por-página**. Más eficiente en memoria pero substancialmente
   más complejo de implementar correctamente (apply/un-apply ordering,
   manejo de allocations). Diferible.
2. **Savepoints solo por nombre stack (FIFO/LIFO)** sin permitir
   nombres custom. SQLite por defecto los permite por nombre y los
   clientes esperan eso. Rechazado.
3. **Hacer `RELEASE` también revertir** (semántica no-ANSI). Rechazado:
   PostgreSQL/SQLite ya tienen la semántica correcta y los clientes
   asumen eso.

## Tests añadidos

Cinco tests `m12_*` en `tests/integration_test.rs`:

- `m12_rollback_to_savepoint_preserves_pre_changes`: INSERT(1);
  SAVEPOINT sp1; INSERT(2); INSERT(3); ROLLBACK TO sp1 → solo (1)
  queda al COMMIT.
- `m12_release_savepoint_keeps_changes`: INSERT(1); SAVEPOINT;
  INSERT(2); RELEASE → ambos (1, 2) quedan. RELEASE no es rollback.
- `m12_nested_savepoints_rollback_outer_invalidates_inner`:
  SAVEPOINT outer; INSERT(2); SAVEPOINT inner; INSERT(3); ROLLBACK TO
  outer → la stack pierde `inner`; `ROLLBACK TO inner` ahora rebota
  `[GBY-4144]`.
- `m12_savepoint_outside_tx_errors`: `SAVEPOINT sp` sin BEGIN →
  `[GBY-4143]`.
- `m12_rollback_to_unknown_savepoint_errors`: `BEGIN; ROLLBACK TO
  nope` → `[GBY-4144]`.

**Suite total**: 819 → **824** (+5 tests; el Pager proptest tampoco se
rompe porque el modelo `BTreeMap<id, v>` no toca savepoints).

## Referencias

- [Bloque T (BEGIN/COMMIT/ROLLBACK)](../../src/sql.rs) — base sobre la
  que M12 se asienta.
- [ADR-0083 — ANSI fix UPDATE/DELETE](0083-ansi-update-delete-no-row-zero.md) — el otro paso del alineamiento ANSI de esta sesión.
- [TAREAS_PENDIENTES.md §6.5](../TAREAS_PENDIENTES.md) — declaraba M12 como pre-requisito de M13.

## Trabajo futuro

- **M13**: cross-request transactions en el servidor HTTP. Ahora
  posible — M12 da el primitivo de checkpoint que el servidor
  necesita para abortar request individualmente sin tirar la tx.
- **Undo log por-página** si el snapshot full se vuelve problemático
  en workloads reales.
- **SAVEPOINT + crash recovery**: hoy el WAL solo registra commits
  finales. Para que SAVEPOINTs sobrevivan a crash mid-tx habría que
  serializarlos al WAL. Fuera de scope.
