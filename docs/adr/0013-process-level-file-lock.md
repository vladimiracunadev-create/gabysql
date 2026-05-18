# ADR-0013: Lock exclusivo a nivel de proceso sobre el archivo `.db`

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-18
**Contexto que la motiva**: bloque "locking simple entre procesos" de Fase 2 → cierre del gap de corrupción silenciosa al abrir la misma DB desde dos procesos.

## 🧭 Contexto

Hasta este bloque, `Pager::create` / `Pager::create_force` / `Pager::open` no negociaban ningún tipo de exclusión con otros procesos. La WAL+CRC de `gabysql` asume **un único escritor por archivo**: las páginas dirty del Pager activo se escriben al `.db` desde memoria sin coordinación con el sistema operativo más allá de los `fsync` propios.

Escenarios reales que esto rompía:

- `gabysql-server` corriendo en background + un `gabysql exec demo.db "INSERT …"` invocado en la misma terminal → dos transacciones concurrentes sobre el mismo `.db`, ambas con su propio `Pager`, ambas escribiendo páginas sin coordinarse. Resultado: páginas pisadas, índices secundarios divergentes, posible corrupción del header.
- Reinicios accidentales del server con un proceso huérfano todavía vivo (orquestadores, supervisor mal configurado, doble systemd-start). Mismo problema, sin diagnóstico claro.
- Tests humanos: abrir `phpgabyadmin` apuntando al mismo archivo que un CLI activo.

El motor detectaba la corrupción **a posteriori** (vía CRC, `INTEGRITY CHECK`), pero ya con el daño hecho. No había forma de **prevenirla**.

Restricciones del proyecto:
- ADR-0001 (cero deps externas). Nada de `fs2`, `fd-lock`, ni equivalentes.
- Cross-platform real: Windows, Linux, macOS — los tres en CI.
- No bloquear: si la DB está tomada, fallar rápido con error claro, **nunca** colgarse esperando.
- No tocar el formato en disco. Sin bump de VERSION.

## 💡 Decisión

Adquirir un **lock advisory exclusivo** sobre el archivo `.db` usando la API estable `std::fs::File::try_lock()` (estabilizada en Rust 1.89.0, agosto 2025).

Implementación en [src/storage.rs](../../src/storage.rs):

```rust
fn acquire_db_lock(file: &File, path: &Path) -> DbResult<()> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(TryLockError::WouldBlock) => Err(DbError::new(format!(
            "database is locked by another process: {}. \
             Close the other gabysql process or wait for it to release the lock.",
            path.display()
        ))),
        Err(TryLockError::Error(err)) => Err(DbError::new(format!(
            "failed to acquire DB lock on {}: {}",
            path.display(), err
        ))),
    }
}
```

Puntos de llamada:
- `Pager::create_internal` (cubre `create` y `create_force`) — tras `OpenOptions::open`, antes de escribir el header.
- `Pager::open` — tras `OpenOptions::open`, antes del replay del WAL. Esto garantiza que **solo un proceso puede ejecutar la recovery** sobre un archivo dado.

El lock se libera en `Pager::close` con `file.unlock()` explícito, y también automáticamente cuando el `File` se dropea (red de seguridad).

## 🤔 Alternativas evaluadas

1. **Sentinel file** (`<path>.lock` con PID dentro): trivial, sin deps, pero requiere cleanup manual cuando el proceso muere por `kill -9`. Lock huérfano = DB inutilizable hasta limpieza manual. **Descartada**: la propiedad mínima esperable de "lock de DB" es que se libere automáticamente al morir el proceso.

2. **`fs2` / `fd-lock` crate**: APIs maduras, pero violan ADR-0001.

3. **Lock vía `libc::flock` (Unix) + `LockFileEx` (Windows) a mano**: hace 6 meses era la opción honesta cross-platform sin deps. Hoy `std::fs::File::try_lock` ya cubre exactamente este caso con la API canónica. **Descartada por obsolescencia**.

4. **Lock compartido para lectores + exclusivo para escritores**: gabysql no tiene modo read-only todavía, así que la distinción no aporta valor hoy. Cuando exista `--read-only`, este lock se relaja a `try_lock_shared()` en ese modo. **Diferida**.

5. **Lock por rango (header byte 0 solamente)**: irrelevante con un solo escritor; añade complejidad por nada. Lock sobre archivo completo es lo correcto.

## ✅ Consecuencias

**Positivas**:
- Imposibilita la corrupción por doble apertura, en los tres OS soportados.
- Error claro y accionable: el usuario sabe inmediatamente que otro proceso tiene la DB.
- Cero deps añadidas (ADR-0001 intacto).
- Cero bump de formato (`VERSION = 6` sigue válido).
- Heredado por `gabysql-server` automáticamente: dos servers apuntando a la misma DB ahora compiten por el lock y el segundo falla rápido.

**Negativas / a vigilar**:
- En Linux/macOS el lock es advisory POSIX (`flock(2)`). Procesos que no usen `gabysql` y escriban directo al archivo no son detenidos. Aceptable: el escenario es "dos `gabysql` accidentales", no "un atacante manual".
- Test de integración `cross_process_lock_rejects_second_open` valida la propiedad dentro del mismo proceso (la segunda llamada a `Pager::open` falla mientras la primera vive). Cross-process real se valida en RUNBOOK manualmente.
- En Windows, `LockFileEx` es mandatory a nivel kernel: si un proceso muere sin liberar y el handle queda huérfano, el sistema lo libera. No hay caso de "lock huérfano permanente".

## 🔗 Referencias

- [Rust 1.89.0 release notes — `File::lock` family stabilization](https://blog.rust-lang.org/2025/08/07/Rust-1.89.0.html)
- [src/storage.rs](../../src/storage.rs): implementación + integración.
- [tests/integration_test.rs](../../tests/integration_test.rs): test `cross_process_lock_rejects_second_open`.
- [ADR-0001](0001-rust-zero-deps-core.md): cero deps en el core.
