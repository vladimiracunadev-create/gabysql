# ADR-0016: Prefetch one-leaf-ahead en `LeafCursor`

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-18
**Contexto que la motiva**: ítem residual "mejoras de full scan para tablas medianas" de Fase 1 → primer paso hacia un cursor que coopere con la jerarquía de caches (kernel + `PageCache`).

## 🧭 Contexto

`LeafCursor` (ADR-0008) ya hace el trabajo correcto desde el punto de vista algorítmico: `SELECT … LIMIT N` deja de materializar la tabla completa y consume O(N + offset) páginas en disco en vez de O(filas_totales).

Pero el patrón de acceso del cursor le presenta al kernel — y a nuestro propio `PageCache` (ADR-0009) — un perfil de I/O **stop-and-go**:

```
read leaf 1 (disk syscall) → decode 100 rows → yield 100 rows
... pausa larga mientras el caller procesa ...
read leaf 2 (disk syscall) → decode → yield ...
```

Dos consecuencias:
1. **El readahead del kernel no se "engancha"**: las heurísticas sequential-readahead de Linux/Windows necesitan ver lecturas back-to-back para detectar un patrón streaming y prefetchear. Con una pausa larga entre leaf N y leaf N+1, la heurística puede no dispararse y cada `page_data` se paga como I/O frío.
2. **Nuestro `PageCache` arranca vacío para cada leaf transition**: la primera lectura post-transición es siempre miss → syscall → CRC verify → cache populate.

Para un scan grande la latencia acumulada de "miss en cada leaf transition" se nota. La mejora barata es **adelantar la lectura de la próxima hoja** al final de la carga de la actual: el syscall ocurre cuando el cursor "no lo necesita todavía", y para cuando el caller termina de procesar la hoja actual, la siguiente ya está en `PageCache`.

## 💡 Decisión

Modificación de 4 líneas en [src/bptree.rs](../../src/bptree.rs):

```rust
fn load_current(&mut self) -> DbResult<()> {
    if self.next_leaf == 0 { self.done = true; return Ok(()); }
    let page = self.pager.page_data(self.next_leaf)?;
    let leaf = decode_leaf(&page)?;
    self.buf = leaf.kvs;
    self.pos = 0;
    self.next_leaf = leaf.next;

    // Prefetch one leaf ahead — synchronous read into PageCache.
    if self.next_leaf != 0 {
        let _ = self.pager.page_data(self.next_leaf);
    }
    Ok(())
}
```

Properties:

- **One leaf ahead, no más**: si el caller corta el iterador después de la primera hoja (`LIMIT 5` que cabe en una hoja), desperdiciamos exactamente 1 lectura. Trade-off aceptable.
- **Best-effort**: el resultado del prefetch se descarta (`let _ = ...`). Si una hoja prefetched tiene CRC roto, el error se materializa en la próxima iteración real, no acá. No duplicamos surface de error.
- **Sin nueva API pública**: ningún caller del cursor cambia. La mejora es invisible para `Engine` y para los tests existentes.
- **Helper `Pager::cache_contains(page_no)`** agregado público (para tests y futura tooling operacional). Era todo lo que faltaba para hacer asserts sobre el cache.

## 🤔 Alternativas evaluadas

1. **Lookahead de N>1 hojas** (N=4): más agresivo, mejor para scans largos. Pero (a) si el `LIMIT` es pequeño, desperdicia más; (b) no hay benchmarks que justifiquen N>1 todavía. Decisión: arrancar con N=1, sintonizar con `gabybench`.

2. **Bulk read syscall** (`pread` de 16 páginas contiguas en un único `read`): el win real para scans largos, porque elimina N-1 syscalls y deja el filesystem firmware pipelinear. Pero requiere extender `Pager` con una API de bulk-read y manejar el caso de páginas no contiguas (los leaves no están garantizadamente adyacentes en el archivo). **Diferida** hasta que `gabybench` muestre que es el cuello de botella.

3. **Async I/O** (`io_uring` en Linux, `IOCP` en Windows): saltar la simulación sincrónica del prefetch y hacer overlap real con compute. Imposible sin un runtime async (tokio, etc.) o sin wrappers manuales no-portables. Viola ADR-0001.

4. **No hacer nada y esperar gabybench**: argumentable. El contraargumento: la mejora es de 4 líneas, no toca formato, no rompe ningún test, y al menos calienta la cache antes del próximo acceso — incluso si el efecto medible es pequeño, la dirección es correcta y no cuesta nada.

## ✅ Consecuencias

**Positivas**:
- Cero cambios a la API pública del cursor.
- Cero deps. Cero bump de formato.
- Forward-only scans presentan ahora un patrón sequential al kernel; en Linux esto suele activar `posix_fadvise(SEQUENTIAL)` implícito.
- `PageCache` queda warm para la próxima leaf transition → `page_data` hit en lugar de syscall.
- Establece la primitiva sobre la que un futuro bulk-read puede crecer.

**Negativas / a vigilar**:
- **Mejora no medida**. Sin `gabybench` no hay número absoluto. La justificación es directional, no empírica. CHANGELOG y este ADR son explícitos al respecto — no se vende esto como "scan 2x más rápido".
- **Sobrelectura de 1 leaf en queries muy chicas** (`LIMIT N` que cabe en la primera hoja). Cuando `gabybench` lo mida, si el costo es real, se condicionará el prefetch a "ya cargamos al menos 2 hojas" o se lo gating-eará por una hint del executor.
- **No aplica a `cursor_range` cuando `to` cae dentro de la hoja actual**: el cursor latcha `done = true` y no llama a `load_current` para el siguiente, así que el prefetch no se ejecuta. Esto es correcto — no queremos prefetchear hojas que sabemos que no leeremos.
- Helper `Pager::cache_contains` ahora es público: pequeño aumento de superficie. Bajo riesgo (read-only de estado existente, sin invariantes nuevas).

## 🔗 Referencias

- [src/bptree.rs](../../src/bptree.rs): `LeafCursor::load_current`.
- [src/storage.rs](../../src/storage.rs): `Pager::cache_contains`.
- [ADR-0008](0008-leaf-cursor-iterator.md): el cursor que esto extiende.
- [ADR-0009](0009-page-cache-lru-bounded.md): el PageCache que esto explota.
- [GABYBENCH_SPEC.md](../GABYBENCH_SPEC.md): la suite que medirá esto cuando exista.
