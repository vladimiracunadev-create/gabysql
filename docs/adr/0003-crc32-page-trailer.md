# ADR-0003: Trailer CRC32 por página + verificación en lectura y replay

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-03
**Contexto**: hallazgo crítico #3 del MVP. Bump VERSION 2 → 3.

## 🧭 Contexto

Hasta esta decisión, ninguna página persistida en disco llevaba checksum. Una corrupción accidental (corte de luz, fallo de disco, escritura torcida) producía resultados silenciosos: el motor abría la DB, los decoders confiaban en los bytes y entregaban valores adulterados al engine SQL, que los devolvía al usuario como si fueran correctos.

Para cualquier producto que se venda como "BD seria con datos de usuarios", esta postura es insostenible.

## 💡 Decisión

Reservar los **últimos 4 bytes** de cada página persistida en disco como trailer **CRC32-IEEE** del resto de la página. El `Pager`:

- **Finaliza el CRC** justo antes de cada flush (a `.db` y a `.wal`), una sola vez por página dirty.
- **Verifica el CRC** en cada lectura del `.db` y en cada `replay_to` del WAL.
- **Aborta con error explícito** ante mismatch — no escribe sobre el `.db` con datos corruptos.

La estructura de leaf/internal pages reserva el espacio (`PAGE_CHECKSUM_BYTES = 4`) en sus cálculos `_fits`.

## 🔄 Alternativas consideradas

- **xxHash o SipHash en lugar de CRC32**: rechazado — CRC32 es estándar industrial para detección de corrupción accidental, simple, table-based, y la velocidad es irrelevante en el hot path comparado con I/O.
- **Checksum en cabecera de página en lugar de trailer**: rechazado — el trailer permite calcularlo sobre `page[..-4]` sin tener que distinguir tipos de página.
- **CRC solo en WAL records**: rechazado — el `.db` puede corromperse fuera de un WAL replay (bit rot del disco).
- **No tener checksums y delegar al filesystem (ZFS/btrfs)**: rechazado — el motor no controla el FS sobre el que corre.

## 📊 Consecuencias

**Positivas**:
- Toda corrupción accidental se detecta en la primera lectura post-fallo.
- El replay del WAL valida cada record antes de aplicarlo al `.db`, evitando que un WAL truncado corrompa la DB.
- Implementación pinneada: tabla CRC32 IEEE construida una sola vez con `OnceLock`.

**Negativas**:
- Bump de formato (VERSION 2 → 3) con rechazo explícito de DBs anteriores.
- Las páginas pierden 4 bytes útiles cada una (~0.1% del payload de una página de 4096).
- Cada flush calcula CRC32 sobre 4092 bytes — coste O(n) en bytes de página dirty, pero amortizado por el I/O posterior.

**Neutras**:
- El CRC32 detecta **corrupción accidental**, no manipulación adversarial. Un atacante con acceso de escritura al `.db` puede recomputar el CRC. Esto está documentado en [docs/SECURITY_LAYERS.md §1](../SECURITY_LAYERS.md).

## 🔗 Referencias

- Commit: `f0cb771`.
- Implementación: [src/storage.rs:finalize_page_checksum / verify_page_checksum / crc32_ieee](../../src/storage.rs).
- Test: [tests/integration_test.rs::page_checksum_detects_corruption](../../tests/integration_test.rs).
- CHANGELOG: entrada 2026-05-03.
