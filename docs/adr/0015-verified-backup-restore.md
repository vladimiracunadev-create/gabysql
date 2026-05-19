# ADR-0015: Backup / restore / verify con validación end-to-end

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-18
**Contexto que la motiva**: bloque "backup / restore verificado" de Fase 2 → cierra el último gap operacional crítico antes de pensar en producción.

## 🧭 Contexto

Hasta este bloque, "backup" en `gabysql` significaba **`cp demo.db backups/demo.db.bak`**. El RUNBOOK lo decía explícitamente como solución informal. Los problemas concretos:

1. **`cp` no entiende el WAL**: si el `.db` se copia mientras hay un `.wal` con un `COMMIT` aplicado a medias, el backup queda en un estado inconsistente que ningún sweep posterior detecta hasta que un crash recovery falla.
2. **Sin validación de CRC**: `cp` copia bytes literalmente. Una página corrupta en el origen se replica al backup sin warning. La primera vez que alguien restaure ese backup, será para descubrir que era inservible.
3. **Sin garantía end-to-end**: nada confirma que el archivo destino se puede *abrir* con la misma versión del binario. `cp` puede dejar truncado, con permisos rotos, en disco con bad block, etc.
4. **Restore manual**: si alguien hace `cp backup.db demo.db` con un server vivo apuntando al mismo path, ahora ADR-0013 lo previene — pero la coordinación queda en manos del operador.

Esto era aceptable en Fase 1 (producto en validación interna). Para acercarse a una operación seria — el ítem del roadmap antes de Fase 3 — hace falta una operación de backup que **o produzca un archivo verificado-bueno o falle ruidosamente**.

Restricciones del proyecto:
- ADR-0001: cero deps.
- ADR-0013: la operación adquiere el lock exclusivo del `.db` mientras corre. **Implica server apagado** durante el backup. Aceptable para Fase 2; un endpoint server-side `/backup` que tome el `write_lock` queda para Fase 3.
- Reproducible cross-platform: misma semántica en Linux/macOS/Windows.

## 💡 Decisión

Nuevo módulo [src/backup.rs](../../src/backup.rs) con tres entradas públicas y subcomandos CLI espejos:

### Fase 1: copia con validación de lectura

```rust
let mut src_pager = Pager::open(src)?;          // ← exclusive lock + WAL replay
let header = src_pager.header();
let mut dst_file = OpenOptions::new().create(true).truncate(true)
    .write(true).read(true).open(dst)?;

for page_no in 0..header.page_count {
    let data = src_pager.page_data(page_no)?;   // ← verifica CRC32 en cada read
    dst_file.write_all(&data)?;
}
dst_file.sync_all()?;
src_pager.close()?;
```

Si **una sola página** del origen tiene CRC inválido, `page_data` falla y el backup aborta antes de publicar el archivo destino. Ningún backup roto se entrega.

### Fase 2: verificación end-to-end del destino

```rust
let mut dst_pager = Pager::open(dst)?;          // ← lock + header decode + version check
if dst_pager.header() != header {
    return Err("destination header does not match source");
}
for page_no in 0..page_count {
    let _ = dst_pager.page_data(page_no)?;      // ← walk + CRC check todas las páginas
}
dst_pager.close()?;
```

Después de escribir el destino, lo reabrimos con el mismo `Pager::open` que usa la app real. Esto valida:
- Header (magic, VERSION, page_size).
- CRC32 de cada página post-fsync.
- Ausencia de WAL huérfano en el destino.

Si algo falla acá, el backup se reporta como inválido y el destino queda en disco para forense — el usuario decide qué hacer.

### API pública

```rust
pub fn backup(src: &Path, dst: &Path, force: bool)  -> DbResult<BackupReport>
pub fn restore(src: &Path, dst: &Path, force: bool) -> DbResult<BackupReport>
pub fn verify(path: &Path)                          -> DbResult<VerifyReport>
```

Subcomandos CLI:
- `gabysql backup [--force] <src.db> <dst.db>`
- `gabysql restore [--force] <src.db> <dst.db>` (alias semántico)
- `gabysql verify <file.db>`

## 🤔 Alternativas evaluadas

1. **`fs::copy` + post-validación**: una sola llamada a `std::fs::copy` es más rápida porque usa zero-copy en kernels modernos. Pero **no valida CRC durante la copia** — solo después. Si el destino se llena de un disco con bad blocks, te enteras en la fase 2 con bytes ya escritos. La página-a-página deja el control del fallo en el momento de lectura, no después. Trade-off: lentitud aceptable (un backup de 1 GB son ~250K páginas a un read/write/CRC cada una; en SSD son ~2s).

2. **Backup incremental (page-level diff)**: requiere persistir un "página → versión" en algún lado y comparar contra el último backup. Útil para DBs grandes pero **fuera de scope** para Fase 2. Un full backup verificable resuelve el caso del 95% (DB de < 10 GB).

3. **Backup online (server-side, sin tomar el lock exclusivo)**: requeriría snapshot del WAL + copy-on-write del `.db`. Complejidad alta. Diferido a Fase 3 cuando exista el endpoint HTTP `/backup` que tome el `write_lock` del server.

4. **Backup a un único archivo tar con metadata**: complica el contrato del archivo destino. Hoy el destino es un `.db` válido, abrible por el mismo binario sin pasos extra. Eso es la propiedad correcta.

5. **Comprimir el backup (gzip)**: separable del problema central. El usuario puede `gabysql backup ... | gzip > backups/demo.db.gz` cuando exista un modo stdout — fuera de scope para este bloque.

## ✅ Consecuencias

**Positivas**:
- Cierra el gap operacional "no hay forma confiable de respaldar". El RUNBOOK pasa de "`cp` informal" a un comando con contrato claro.
- Misma operación verifica que el destino se puede *usar*, no solo *que existe*.
- Detección de corrupción upstream: si la DB origen tiene una página corrupta, el operador se entera **al intentar respaldarla**, no semanas después al restaurar.
- `verify` independiente cubre el caso "tengo un `.db` de hace 6 meses y quiero saber si todavía es válido" sin necesidad de hacer copia.
- Cero deps añadidas (ADR-0001 intacto). Sin bump de formato.

**Negativas / a vigilar**:
- Requiere acceso exclusivo al `.db` durante el backup (ADR-0013). Servidor debe estar apagado. Documentado en CHANGELOG + RUNBOOK; endpoint server-side queda para Fase 3.
- Coste lineal con el tamaño de la DB (todas las páginas se leen, escriben y revalidan). Para una DB de 100 GB esto son minutos. Aceptable para el caso de uso embebido.
- No hay rotation policy. El operador es responsable de borrar backups viejos. Deliberado: rotation policy depende del entorno (cron + `find -mtime`, S3 lifecycle, etc.).

## 🔗 Referencias

- [src/backup.rs](../../src/backup.rs): implementación.
- [src/bin/gabysql.rs](../../src/bin/gabysql.rs): subcomandos CLI.
- [tests/integration_test.rs](../../tests/integration_test.rs): `backup_roundtrip_verifies_end_to_end`, `backup_detects_corrupted_source`, `verify_walks_every_page`.
- [ADR-0003](0003-crc32-page-trailer.md): el CRC32 por página que esto explota.
- [ADR-0013](0013-process-level-file-lock.md): el lock exclusivo que protege la operación.
