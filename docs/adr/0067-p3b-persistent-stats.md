# ADR-0067: P3b — stats por tabla persistidas en catálogo (`ObjectKind::TableStats`)

**Fecha:** 2026-06-09
**Estado:** Aceptado
**Bloque:** P3b (Fase 3 — Performance / Planeación)
**Antecede:** ADR-0063 (P1 — EXPLAIN), ADR-0064 (P2 — EXPLAIN ANALYZE), ADR-0065 (P3 — ANALYZE session-scoped)

## Contexto

P3 ([ADR-0065](0065-p3-analyze-stats.md)) entregó `ANALYZE <table>` con stats
en memoria (`Engine.table_stats: HashMap<String, TableStats>`). EXPLAIN
consume esas stats y las muestra como `est.rows=N`. Limitación honesta:
**al cerrar el Engine las stats se perdían** — la siguiente sesión arrancaba
con HashMap vacío y EXPLAIN dejaba de mostrar `est.rows` hasta que el usuario
re-ejecutara `ANALYZE`.

P3b cierra ese gap persistiendo las stats en el catálogo. El roadmap natural
hacia P5 (planner-as-optimizer) requiere stats que sobrevivan a reopens — no
tiene sentido decidir entre `Hash Join` y `Nested Loop` con stats efímeras.

## Decisión

### `ObjectKind::TableStats` (code = 9)

Nuevo discriminador en el enum de catálogo. Mismo patrón que las 8 variantes
existentes (Table, View, Trigger, Procedure, Function, User, Role, Grant,
Policy).

### `StatsMeta`

```rust
pub struct StatsMeta {
    pub name: String,             // tabla a la que pertenecen
    pub row_count: u64,           // exacto al momento del ANALYZE
    pub analyzed_at_nanos: u128,  // wall-clock epoch, para detectar staleness
}
```

Serialización: `push_string(name) || row_count.to_le_bytes() || analyzed_at_nanos.to_le_bytes()`
(8 + 16 = 24 bytes fijos + string len).

### Clave del catálogo: `__stats__:<tabla>`

Pre-pended con `__stats__:` para no colisionar con el record de la tabla
(que usa `hash_name(tabla)`). Sigue el patrón de `GrantMeta::catalog_key_name`
y `PolicyMeta::catalog_key_name`.

### Lifecycle

| Evento | Acción |
|---|---|
| `ANALYZE <table>` | `Catalog::put_table_stats(&meta)` — upsert (sobrescribe stats anteriores) |
| `Engine::new(pager)` | `Catalog::list_table_stats()` → hidratar `HashMap` |
| `DROP TABLE t` | `Catalog::remove_table_stats(t)` además del `remove_table` |
| `TRUNCATE` | (no implementado en P3b — sub-tarea pendiente) |
| Crash / abort | el record solo se commitea si el WAL del ANALYZE commitea |

### Mensaje del ANALYZE

Cambió de:
```
Cache session-scoped — se pierde al cerrar el Engine.
```
a:
```
Persistidas en catálogo — sobreviven a reopen del Engine.
```

El test `p3_analyze_returns_row_count` acepta ambos por compat de tests
históricos.

## Bump VERSION 31 → 32

Política conservadora del motor ([storage.rs:236](../../src/storage.rs)):
**no hay auto-upgrade entre versiones**. Aunque P3b es puramente aditivo
(ningún record existente cambia layout, solo aparece un kind nuevo), las
DBs creadas con V31 son rechazadas con `[GBY-1003]
UNSUPPORTED_FORMAT_VERSION` y el mensaje estándar de "dump + re-create".

Esto evita una clase de bugs sutiles: si un binario viejo abriera una DB
nueva, ignoraría los records `TableStats` y eventualmente podría
corromperlos (p.ej. al re-compactar el B-tree del catálogo).

## Alternativas consideradas

1. **No bumpear VERSION** (zero-bump, como P1/P2/P3).
   - **Descartado**: agregar un nuevo `ObjectKind` sin bump rompe la
     garantía del motor de que binarios viejos pueden abrir DBs viejas.
     Un binario V31 viendo `code=9` en el catálogo crashea con
     `kind de objeto desconocido en catálogo: 9`.

2. **Persistir stats fuera del catálogo** (archivo paralelo `stats.bin`).
   - **Descartado**: rompe el principio "el catálogo es la única fuente
     de verdad del schema". Coordinar dos archivos requiere su propia
     consistencia (qué pasa si una falla y la otra no, qué pasa con WAL).

3. **Diferir P3b a parte de P5** (junto con el planner-as-optimizer).
   - **Descartado**: P5 es grande (reorder de joins, choice de índice,
     hash vs nested-loop). Cortarlo en P3b independiente da un push pequeño,
     validable y reversible. Además desbloquea P4 (stats por-columna), que
     también necesitará persistencia.

4. **`PUBLISH` / `COMMIT` explícito de stats** (en vez de auto-persistir).
   - **Descartado**: `ANALYZE` ya es un statement explícito del usuario. Si
     lo ejecutó es porque quiere las stats; cargar el peso ergonómico de
     un segundo paso (`COMMIT STATS`) es overkill.

## Tests

4 tests nuevos en `tests/integration_test.rs` (suite p3b_*):

- `p3b_stats_sobreviven_reopen`: ANALYZE en sesión 1, EXPLAIN en sesión 2
  muestra `est.rows`.
- `p3b_drop_table_borra_stats_del_catalogo`: DROP en sesión 1, re-CREATE
  en sesión 2 NO ve stats viejas.
- `p3b_analyze_sobrescribe_stats_persistidas`: dos ANALYZE consecutivos
  con row_count distintos — el segundo gana.
- `p3b_db_sin_analyze_arranca_sin_stats`: regression — DB que nunca corrió
  ANALYZE abre limpio (HashMap vacío, EXPLAIN sin `est.rows`).

Test viejo `p3_stats_son_session_scoped_se_pierden_en_reapertura` invertido
a `p3_stats_persisten_a_traves_de_reapertura_p3b` — documenta el cambio
de contrato P3 → P3b y previene regresión.

Suite total: **749 passing** (745 → +4 P3b nuevos). El test invertido no
cuenta como suma, era ya parte del conteo.

## Consecuencias

**Positivas**

- (+) EXPLAIN muestra `est.rows` consistentemente después del primer
  ANALYZE — no hay "ventana ciega" entre sesiones.
- (+) Pre-requisito limpio para P4 (stats por-columna) y P5 (planner real).
  Ambos heredan el slot `ObjectKind::TableStats` y solo extienden `StatsMeta`.
- (+) DROP TABLE no deja huérfanos en el catálogo.

**Negativas / Limitaciones honestas**

- (-) `analyzed_at_nanos` no se usa todavía en ninguna decisión — EXPLAIN
  no muestra "stats stale (analizadas hace X minutos)". Queda para P4/P5.
- (-) `TRUNCATE TABLE` no resetea las stats persistidas. Quien trunque y
  no re-ejecute `ANALYZE` verá `est.rows` viejos en EXPLAIN. **Actualización
  2026-06-15**: TRUNCATE como statement existe desde el bloque J
  (`Statement::Truncate` / `exec_truncate`). Lo que sigue pendiente es
  que `TRUNCATE` borre el record `ObjectKind::TableStats` análogamente
  a DROP — ver R5 en [TAREAS_PENDIENTES.md §4](../TAREAS_PENDIENTES.md).
- (-) Auto-ANALYZE (autovacuum-style) sigue pendiente. P3b solo persiste
  lo que el usuario disparó explícitamente. Sin un scheduler, las stats
  pueden volverse stale silenciosamente si la tabla cambia mucho.
- (-) Bump VERSION fuerza re-creación de DBs V31. No hay path de upgrade
  in-place — política deliberada del motor para evitar corrupción.

## Limitaciones / Trabajo futuro

- **P4**: stats por-columna (NDV vía HyperLogLog, top-K MCV, histogramas
  equi-depth). `StatsMeta` se extiende con un `Vec<ColumnStats>`; el slot
  on-disk ya existe.
- **P5**: planner-as-optimizer real que consuma las stats. Sub-tarea P5b
  cierra el último gap del bench documentado en
  [ADR-0066 Gap 10](0066-bench-exposed-gaps.md): composite index lookup
  con `WHERE c1=X AND c2=Y`.
- **EXPLAIN con staleness**: `est.rows=N (stats hace 3d 5h)` para que el
  usuario vea si conviene re-ejecutar `ANALYZE`.
- **Auto-ANALYZE**: scheduler que re-analiza cuando la tabla cambia más
  de un umbral (p.ej. 10% de rows insertadas desde el último analyze).
  Requiere infraestructura de jobs.
- **`ANALYZE` global** sin argumento: iterar todas las tablas del catálogo.
