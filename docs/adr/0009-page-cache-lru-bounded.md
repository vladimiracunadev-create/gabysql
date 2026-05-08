# ADR-0009: `PageCache` con capacidad fija + LRU sobre páginas clean

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-08
**Contexto que la motiva**: auditoría de patrones de diseño para optimización de recursos (bloque 10 del roadmap interno) → commit `6cb958a`.

## 🧭 Contexto

Pre-bloque-10 el cache de páginas del Pager era:

```rust
// storage.rs (pre-bloque-10)
pub struct Pager {
    cache: BTreeMap<u32, CachedPage>,  // ← crece sin freno
    ...
}
fn ensure_page_loaded(&mut self, no: u32) -> DbResult<()> {
    if self.cache.contains_key(&no) { return Ok(()); }
    // ... lee de disco ...
    self.cache.insert(no, CachedPage { data, dirty: false });  // ← nunca evicta
}
```

**Sin política de eviction.** Cada página leída se queda en RAM para siempre. En modo CLI (one-shot) no se nota, pero en `gabysql-server -dir ./dbs` corriendo días o semanas:

- 50 DBs × 200 MB cada una = 10 GB potenciales de páginas.
- Un `INTEGRITY CHECK` periódico (sweep operacional recomendado en RUNBOOK) toca **cada página de cada DB**, las pinea en cache permanentemente.
- Resultado: el server crece a 10 GB de RAM y eventualmente lo mata el OOM killer del kernel. **Sin error, sin warning, sin recovery** — solo `kill` y reiniciar.

Es una fuga de memoria silenciosa, proporcional al working set, no a ningún parámetro configurable.

Restricciones del proyecto:
- Cero dependencias externas (no `lru` crate).
- Mantener correctness de transacciones: las páginas dirty pertenecen a la transacción abierta y **no pueden** perderse antes del WAL flush.
- API pública del Pager debe mantenerse compatible (los call sites internos confían en `page_data`, `write_page`, `commit`, `rollback`).
- Política tuneable: el embedded de un dispositivo IoT necesita 64 páginas; un server multi-DB necesita 4096.

## 💡 Decisión

Reemplazar `BTreeMap<u32, CachedPage>` por un `PageCache` con:

1. **Capacidad fija** (`DEFAULT_CACHE_PAGES = 1024`, configurable via `Pager::set_cache_capacity(n)`).
2. **LRU tracking via contador monótono**: cada `get/get_mut/insert` bumpea `counter: u64` y graba el valor en el slot accedido. La página menos recientemente usada es la del `last_access` más bajo.
3. **Eviction dirty-aware**: cuando `insert()` ocurre con cache lleno, escanea las entradas, filtra solo las **clean**, evicta la del `last_access` mínimo. **Las dirty nunca se evictan.**
4. **Overflow controlado**: si todas las entradas están dirty (edge case mid-tx con muchas writes), se permite que el cache exceda capacidad temporalmente. Drena solo en `commit` cuando `mark_all_clean()` vuelve toda la cache evictable.

Estructura:

```rust
struct PageCache {
    capacity: usize,
    map: HashMap<u32, CacheSlot>,
    counter: u64,
}
struct CacheSlot {
    page: CachedPage,
    last_access: u64,
}
```

API nueva en Pager (todo lo demás compatible):
- `pub fn set_cache_capacity(&mut self, capacity: usize)`
- `pub fn cache_len(&self) -> usize`
- `pub fn cache_capacity(&self) -> usize`

## 🔄 Alternativas consideradas

### Mantener `BTreeMap` sin límite
- **Pro**: cero refactor; cero riesgo de evictar algo necesario.
- **Contra**: la fuga de memoria es la única queja real previsible del modo server. No se puede entregar el primer release server con esto.
- **Veredicto**: rechazada.

### Usar el crate `lru`
- **Pro**: implementación battle-tested, doubly-linked list O(1) en touch + eviction.
- **Contra**: rompe ADR-0001 (cero dependencias externas). El cache es un componente core con contrato de estabilidad alto — cualquier crate externo introduce riesgo de breaking changes en supply chain.
- **Veredicto**: rechazada.

### Doubly-linked list manual con índices en `Vec<Slot>`
- **Pro**: O(1) touch + eviction como `lru` crate, sin dependencia.
- **Contra**: ~150 líneas extra de unsafe-adjacent code (free list, pointer juggling con `Option<usize>`). Para cap = 1024 el O(N) actual es ~µs por eviction, no aparece en profile.
- **Veredicto**: rechazada por overengineering preventivo. Si en el futuro `cap > 10K` y eviction aparece como hot path, se reconsidera.

### Eviction agresiva (también de dirty pages, vía writeback inmediato)
- **Pro**: cap estricto siempre respetado.
- **Contra**: rompe el modelo transaccional (writes parciales fuera del WAL → corrupción si crashea entre el writeback y el commit). Requiere cambiar el WAL a logging de operaciones, no after-image.
- **Veredicto**: rechazada — fuera del scope.

### **Capacity bounded + LRU clean-only** (decisión)
- **Pro**: cap estricto en estado estable (post-commit). Mantiene el invariante crítico "ninguna dirty se pierde". Implementación simple. Configurable.
- **Contra**: el cap puede excederse temporalmente mid-tx con muchas writes pendientes. Es el único trade-off aceptable porque la alternativa es corromper la DB.
- **Veredicto**: **aceptada**.

## 📊 Consecuencias

### Positivas
- Memoria del server **acotada y predecible**: `cache_capacity × #DBs_abiertas × page_size`. Default = 1024 × 4 KB = ~4 MB por DB.
- Cierra la única fuga de memoria conocida del modo server long-running.
- Sigue sin dependencias externas; el algoritmo es ~120 líneas auditables.
- API pública del Pager 100% compatible con call sites existentes — adoptado sin tocar `bptree.rs`, `catalog.rs`, ni `sql.rs`.
- Tres métodos nuevos (`set_cache_capacity`, `cache_len`, `cache_capacity`) habilitan tuning runtime y observabilidad para `INTEGRITY CHECK`-style introspection.

### Negativas
- Workloads con working set > capacity sufren **cache misses** que antes no ocurrían (la primera lectura pega a disco, las siguientes pueden o no pegarle según LRU). Mitigación: `set_cache_capacity(N)` para subir.
- Eviction es O(N) en el cap, no O(1). Para cap = 1024 es invisible; para cap = 100K aparecería en profile y habría que migrar a doubly-linked list.
- Mid-tx con muchas writes consecutivas puede exceder el cap temporalmente. Aceptable porque drena en commit, pero el cap no es "duro" — es "en estado estable".

### Neutras
- El orden de iteración de `HashMap` es no determinista; eso cambia el orden en que las páginas dirty se escriben al WAL respecto al `BTreeMap` previo (que ordenaba por page_no). El WAL replay aplica todas igual y el resultado es el mismo (cada página es write-through completo), así que es invisible funcionalmente.
- Se reemplazó `BTreeMap` por `HashMap` en el cache → ya no se mantiene orden por page_no en iteración. Si en el futuro se necesita orden (ej. para checkpoint optimizado), habría que cambiar la estructura.

## 🔗 Referencias

- Implementación: [src/storage.rs](../../src/storage.rs) (`PageCache`, `CacheSlot`, `DEFAULT_CACHE_PAGES`).
- Test: [tests/integration_test.rs](../../tests/integration_test.rs) (`page_cache_is_bounded_and_evicts_clean_pages`).
- CHANGELOG: entry "Decimoquinta intervención: `PageCache` LRU acotado" (2026-05-08).
- Inspiración: SQLite pager (`sqlite3PcacheCreate`, default `PAGER_DEFAULT_CACHE_SIZE = 2000`); PostgreSQL `shared_buffers` (default 128 MB, mucho más grande porque target es server dedicado).
- Pattern: clásico **LRU cache** (Tanenbaum, "Modern Operating Systems"); variante con **dirty-aware eviction** común en motores de DB (SQLite, MySQL InnoDB).
