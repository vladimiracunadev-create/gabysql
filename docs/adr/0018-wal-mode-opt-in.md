# ADR-0018: WAL-mode opt-in con checkpoint explícito

**Estado**: 🟡 Propuesta (diseño aceptado, implementación deferida)
**Fecha**: 2026-05-18
**Contexto que la motiva**: ítem residual de Fase 2 "checkpoint del WAL" → re-evaluado durante intervenciones anteriores como **no aplicable al modelo actual** (WAL-per-transaction). Este ADR documenta el modelo alternativo que sí lo haría aplicable, con la decisión explícita de **no implementarlo todavía**.

> **Resumen ejecutivo**
>
> Este ADR captura el diseño de un modo WAL persistente estilo SQLite-WAL para `gabysql`. **No se implementa en este bloque.** La razón es honesta: el beneficio principal (concurrencia lectores/escritores) no aplica al motor single-writer actual; el beneficio secundario (commits más rápidos) tiene valor real pero no hay un workload medido que lo justifique. El diseño queda escrito para cuando aparezca esa demanda — para `gabybench` mostrando que el fsync del `.db` por commit es el cuello de botella, o para un caso de producción con write-heavy patterns.

## 🧭 Contexto

El WAL actual de `gabysql` ([src/storage.rs](../../src/storage.rs::Pager)) opera en modo **WAL-per-transaction**:

```
begin()  → crea .wal vacío (truncate)
commit() → write pages → COMMIT → fsync WAL → write pages → .db → fsync .db → remove WAL
rollback() → remove WAL, cache limpia
open()   → si WAL existe con COMMIT → replay + remove; sino remove
```

**Cada commit ya es un checkpoint implícito.** Después de commit, el `.db` está al día y el WAL desaparece. El WAL nunca crece a través de transacciones.

Bajo este modelo, *"checkpoint/compaction del WAL"* del roadmap no aplica — no hay nada que compactar porque no hay nada que persista. Esto se documentó en intervenciones anteriores y el ítem se diferió.

Pero la pregunta de fondo sigue abierta: **¿debería `gabysql` tener un WAL persistente al estilo SQLite-WAL?** La respuesta correcta depende de qué se quiere optimizar.

## 💡 Decisión

Este ADR propone — **sin implementar** — un modelo WAL persistente opt-in:

### Modelo propuesto

```
begin()  → si .wal no existe, crea uno; si existe (de commits previos), append-only
commit() → write pages → COMMIT marker → fsync WAL.  NO toca .db. NO fsync .db.
           El WAL crece monotónicamente entre checkpoints.

checkpoint() (explícito, nueva API)
         → para cada page_no con record más reciente en WAL: write a .db
         → fsync .db
         → truncate WAL a 0 bytes
         → ack al caller con stats (pages_flushed, bytes_freed)

open()   → si WAL existe con COMMITs: construye WAL-index in-memory
           {page_no → wal_offset_del_record_más_reciente}.
           NO replay-y-borrar como hoy; replay-y-mantener.
         → en este modo, una llamada implícita a checkpoint() al final del open
           devuelve al estado "como antes" si el caller pasó --checkpoint-on-open

close()  → checkpoint() automático para no dejar WAL pendiente.
```

### Cambio en el camino de lectura

Hoy `page_data(no)` lee directo del `.db` (o del cache). En WAL-mode, antes de ir al `.db`, hay que mirar el WAL-index in-memory:

```rust
fn page_data(&mut self, no: u32) -> DbResult<Vec<u8>> {
    if let Some(cached) = self.cache.get(no) { return Ok(cached.clone()); }
    // ↓ rama nueva en WAL-mode
    if let Some(wal_offset) = self.wal_index.latest_offset(no) {
        let data = self.wal.read_page_at(wal_offset)?;
        verify_page_checksum(&data)?;
        self.cache.insert(no, ...);
        return Ok(data);
    }
    // rama vieja
    let data = self.read_page_from_db(no)?;
    verify_page_checksum(&data)?;
    self.cache.insert(no, ...);
    Ok(data)
}
```

El `wal_index` se construye al abrir (un scan del WAL) y se actualiza incrementalmente en cada `write_page` del commit. Memoria: O(unique_pages_in_WAL) × ~16 bytes por entrada — acotada por el tamaño del WAL.

### Opt-in via flag, no default

```
pub struct PagerOptions {
    pub wal_mode: WalMode,  // Default = TransactionPerWal (actual), Persistent = nuevo
}
```

CLI: `gabysql init --wal-persistent demo.db`. Server: flag `-wal-persistent`.

**Default sigue siendo el modelo actual.** El opt-in se gana adopción cuando haya métricas que lo justifiquen.

## 🤔 Alternativas evaluadas

1. **Implementar ahora**: posible, pero costoso. Estimación: 400-600 LOC en el hot path de `storage.rs` + reorganización de los tests de crash recovery (3 escenarios actuales asumen WAL-per-transaction). Riesgo de regresión alto sin un workload de validación. **No vale antes de `gabybench`**.

2. **No hacer nada, marcar el ítem como inviable permanentemente**: borrar el ítem del roadmap. **Desperdicia el análisis hecho**. Mejor dejar este ADR como punto de aterrizaje cuando alguien venga con un caso real.

3. **WAL-mode como default**: ahorraría una fsync por commit (medible, ~2-5ms en SSD). Pero rompe la propiedad "un commit exitoso = .db al día" en la que descansan ADR-0013 (lock file lock), ADR-0015 (backup verifica .db) y el patrón mental del usuario. **No vale como default** sin medición previa.

4. **WAL-mode con auto-checkpoint cada N commits o cada T segundos**: hace falta un thread o un counter por commit. Agrega complejidad sin justificación. **Diferido al opt-in primero**, auto-checkpoint encima.

5. **Group commit sin WAL-mode**: agrupar N commits en una sola fsync del .db. Más simple que WAL-mode pero asume múltiples writers — `gabysql` es single-writer (Mutex global). **No aplica**.

6. **mmap del .db en lugar de WAL-mode**: enfoque distinto pero comparable en complejidad y con sus propios trade-offs (mmap fsync semantics es notoriamente sutil cross-platform). **Fuera de scope, sería otro ADR**.

## ✅ Consecuencias de NO implementar ahora

**Positivas (de no implementar)**:
- Cero cambios al hot path → cero riesgo de regresión.
- El motor actual ya es correcto y entendible. Cada commit deja el `.db` consistente en disco.
- ADR-0013 (file lock), ADR-0015 (backup verifies .db) y los crash tests existentes siguen aplicando sin necesidad de adaptarse.
- El ítem del roadmap pasa de "no aplica" a "diseño aceptado, implementación pendiente" — estado más honesto.

**Negativas (de no implementar)**:
- Cada commit paga 2 fsyncs (WAL + .db). Para workloads con muchos commits pequeños (e.g., INSERT en loop), esto es el cuello de botella. Hoy ese workload no está siendo perfilado.
- Sin WAL persistente, no hay forma trivial de implementar "readers see snapshot" — pero esto solo importa si el motor evoluciona a múltiples writers/readers concurrentes. Hoy es single-writer.

## 🎯 Condiciones de salida (cuándo retomar)

Este ADR pasa a "Aceptada" + implementación cuando se cumpla alguna de:

1. **`gabybench` muestre que `fsync(.db)` por commit domina la latencia** en el workload típico medido.
2. Aparezca un **caso de uso productivo write-heavy** (e.g., ingest de logs estructurados) con métricas concretas.
3. Se necesite **MVCC o readers concurrentes** — WAL-mode es prerequisito para snapshot isolation.
4. **Un PR externo** lo proponga con benchmarks; el diseño está acá listo para revisar.

## 🔗 Referencias

- [src/storage.rs](../../src/storage.rs): WAL actual (WAL-per-transaction).
- [SQLite WAL-mode documentation](https://www.sqlite.org/wal.html): el modelo de referencia.
- [ADR-0013](0013-process-level-file-lock.md): el lock que ya asume single-writer.
- [ADR-0015](0015-verified-backup-restore.md): el backup que asume `.db` consistente post-commit.
- [docs/GABYBENCH_SPEC.md](../GABYBENCH_SPEC.md): la suite de benchmark que justificaría esta implementación.
- [ROADMAP.md](../../ROADMAP.md): ítem "checkpoint del WAL" — ahora apunta acá.
