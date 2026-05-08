# ADR-0008: `LeafCursor` (Iterator pattern) para SELECT lazy

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-08
**Contexto que la motiva**: auditoría de patrones de diseño para optimización de recursos (bloque 9 del roadmap interno) → commit `c1316ea`.

## 🧭 Contexto

Pre-bloque-9 todos los walks del B+Tree retornaban `Vec<KeyValue>` materializado:

```rust
// catalog.rs (pre-bloque-9)
pub fn scan_rows(&mut self, root: u32, offset: usize, limit: Option<usize>)
    -> DbResult<Vec<KeyValue>>
pub fn range_rows(&mut self, root: u32, from: i64, to: i64)
    -> DbResult<Vec<KeyValue>>
```

Esto es correcto para tablas pequeñas (los tests usan ~600 filas y nunca se nota), pero el costo crece linealmente con el tamaño de la tabla, **independientemente del LIMIT pedido**:

- `SELECT id FROM tabla LIMIT 10` sobre **1.000.000 filas**:
  - lee las 1M páginas leaf del B+Tree desde disco,
  - decodifica las 1M filas en memoria,
  - construye un `Vec<KeyValue>` de ~50 MB,
  - aplica LIMIT 10 sobre el Vec,
  - tira las 999.990 restantes.

El bloque 7 (`ORDER BY`) agudizó el problema: cuando hay `ORDER BY` se setea `defer_window = true`, lo que **forza** materializar todo aunque haya LIMIT chico, porque el sort necesita el conjunto completo. Sin un mecanismo de iteración lazy, ninguna optimización de window puede convivir con ORDER BY.

Restricciones del proyecto:
- Cero dependencias externas (no se puede usar `iterator-extras` ni similar).
- Compatibilidad de formato en disco — el cambio no debe bumpear VERSION.
- API pública del Pager y Catalog debe mantenerse (otros call sites — backfill, INTEGRITY CHECK, delete cascade — siguen necesitando materialización).
- Borrow checker: el cursor toma `&mut Pager` por su lifetime; no debe romper los call sites read+write existentes.

## 💡 Decisión

Implementar **`bptree::LeafCursor<'a>`** que:

1. Implementa `Iterator<Item = DbResult<KeyValue>>`.
2. Anclado a un root del B+Tree, descenso al leaf inicial (leftmost para full scan, `find_leaf(from)` para range).
3. Carga la página leaf actual en `buf: Vec<KeyValue>`, drena con `pos: usize`, salta a la siguiente leaf vía la chain `next` cuando se vacía.
4. Mantiene `upper: Option<i64>` para corte temprano en range scans inclusive.
5. Sticky `done: bool` para EOF idempotente.

Constructores públicos en `Tree`:
- `Tree::cursor_full(root) -> LeafCursor<'a>`
- `Tree::cursor_range(root, from, to) -> LeafCursor<'a>`

Wrappers en `Catalog` (consumen `self` para liberar el descenso del borrow del Pager antes de construir el cursor):
- `Catalog::scan_cursor(root) -> LeafCursor<'a>`
- `Catalog::range_cursor(root, from, to) -> LeafCursor<'a>`

`exec_select` reescrito: cuando NO hay `ORDER BY`, los planes `FullScan` y `Range` consumen el cursor con `.skip(stmt.offset).take(stmt.limit.unwrap_or(usize::MAX)).collect::<DbResult<Vec<_>>>()?`. La promesa de `Iterator::take` (no avanza el inner iterator más allá del N-ésimo `Some`) es lo que vuelve la operación O(N + offset) en disco, no O(table_size).

## 🔄 Alternativas consideradas

### Mantener `Vec<KeyValue>` y aceptar el costo
- **Pro**: cero refactor; el motor está pensado para tablas chicas.
- **Contra**: hace inviable cualquier feature que escale a tablas medianas/grandes. ORDER BY ya cargó el costo; el próximo (range scan secundario) lo agudizará.
- **Veredicto**: rechazada — invertir ahora cuesta poco; postergar cuesta exponencialmente más.

### Cambiar API a `for_each(callback)` en lugar de Iterator
```rust
fn scan_with<F: FnMut(KeyValue) -> ControlFlow<()>>(&mut self, root: u32, f: F)
    -> DbResult<()>;
```
- **Pro**: evita el problema de borrow checker (callback no captura `&mut Pager`).
- **Contra**: rompe la composición con la stdlib (`take/skip/filter/map`). Cada call site que quiera ORDER BY o WHERE adicional tiene que reimplementar el callback.
- **Veredicto**: rechazada — perdés la potencia ergonómica del trait `Iterator`.

### Iterator que internamente clona el Pager
- **Pro**: no toma `&mut Pager`, así que múltiples cursors simultáneos posibles.
- **Contra**: `Pager` no es clonable (tiene `File`, WAL, cache). Forzaría `Arc<Mutex<…>>`, complicando el sync model y atando el motor a runtime overhead permanente.
- **Veredicto**: rechazada — el caso de uso "múltiples cursors a la vez" no existe (un thread por request, una transacción a la vez).

### Iterator pattern como aquí + `&mut Pager` exclusivo
- **Pro**: composición idiomática con stdlib, cero overhead, borrow checker enforza el invariante "no escrituras concurrentes" gratis.
- **Contra**: los call sites read+write (backfill, INTEGRITY CHECK, delete cascade) no pueden usarlo. Hay que coexistir con los helpers materializadores.
- **Veredicto**: **aceptada**. La coexistencia es un feature, no un bug — los path read+write necesitan terminar la lectura antes de la escritura, y la materialización lo hace explícito.

## 📊 Consecuencias

### Positivas
- `SELECT … LIMIT N` sobre tablas grandes pasa de O(filas_totales) a **O(N + offset)** en RAM y IO. Verificable con el test `cursor_limit_returns_only_requested_rows` (1.000 filas, LIMIT 5 / LIMIT 3 OFFSET 7 / BETWEEN+LIMIT).
- Habilita Fase 3 (range scan secundario, JOIN, `ORDER BY` con merge sort externo) sin reescribir API de Catalog/Tree.
- Borrow checker enforza estáticamente que no haya escrituras concurrentes sobre el mismo Pager mientras un cursor está vivo — invariante de seguridad gratis.
- Composabilidad con stdlib (`skip/take/filter/map/collect`) deja el código de SELECT más declarativo.

### Negativas
- Los call sites read+write (CREATE INDEX backfill, INTEGRITY CHECK, delete_with_cascade) **no pueden** usar el cursor. Tienen que seguir con `scan_rows / range_rows / all`. Es una bifurcación de API que hay que documentar y respetar (el comentario en `LeafCursor` lo hace).
- Si en el futuro se intenta usar el cursor en uno de esos paths sin pensarlo, el borrow checker lo rechaza con un error verboso. Es seguro pero potencialmente confuso.

### Neutras
- El cursor clona `KeyValue` por `next()` (porque la implementación de `decode_leaf` ya copia los bytes). Una variante zero-copy con borrow del page buffer ahorraría una alloc por entrada pero complicaría la API. Postergada.

## 🔗 Referencias

- Implementación: [src/bptree.rs](../../src/bptree.rs) (`LeafCursor`, `Tree::cursor_full`, `Tree::cursor_range`).
- Adapter en catálogo: [src/catalog.rs](../../src/catalog.rs) (`Catalog::scan_cursor`, `Catalog::range_cursor`).
- Uso en SELECT: [src/sql.rs](../../src/sql.rs) (`Engine::exec_select`, rama `defer_window = false`).
- Test: [tests/integration_test.rs](../../tests/integration_test.rs) (`cursor_limit_returns_only_requested_rows`).
- CHANGELOG: entry "Decimocuarta intervención: `LeafCursor`" (2026-05-08).
- Inspiración: SQLite cursor API (`sqlite3BtreeNext`), Iterator trait de Rust (`std::iter::Iterator`).
