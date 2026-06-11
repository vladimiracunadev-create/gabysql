# ADR-0068: P4 — stats por-columna (NDV / null_count / MCV / histograma)

**Fecha:** 2026-06-10
**Estado:** Aceptado
**Bloque:** P4 (Fase 3 — Performance / Planeación)
**Antecede:** ADR-0067 (P3b — stats por tabla persistidas)
**Bumpea:** VERSION 32 → 33

## Contexto

P3b ([ADR-0067](0067-p3b-persistent-stats.md)) persistió stats agregadas
por tabla — `row_count` y `analyzed_at_nanos` — pero el planner real
(P5) no puede decidir entre planes con solo eso. Necesita:

- **NDV (number of distinct values)** por columna para estimar
  selectividad de `WHERE c = X` (≈ `1/NDV`) y costo de `GROUP BY c`.
- **null_count** para excluir NULLs de cardinalidades estimadas (3VL).
- **MCV (Most Common Values)** para detectar skew: si el 60% de la
  tabla son `c='active'`, ningún índice secundario sobre `c='active'`
  conviene; los demás valores sí.
- **Histograma equi-depth** para rangos: `WHERE c BETWEEN a AND b` —
  cuántos buckets cubre.

P4 entrega esos cuatro componentes persistidos en el mismo record
`ObjectKind::TableStats` de P3b, con un `format_version` interno para
permitir extensiones aditivas sin bump global del VERSION.

**Importante**: P4 solo *recolecta y persiste* las stats. El planner
no las consume todavía — eso es P5 / P5b. EXPLAIN sí muestra que
están presentes (anota `cols=N`) para verificación operativa.

## Decisión

### `ColumnStats` (catalog.rs)

```rust
pub struct ColumnStats {
    pub name: String,
    pub null_count: u64,
    pub ndv: u64,                          // estimado HLL
    pub mcv: Vec<(StatsValue, u64)>,       // top-K=10 por frecuencia desc
    pub histogram: Vec<HistogramBucket>,   // equi-depth ~16 buckets
}

pub struct HistogramBucket {
    pub lower: StatsValue,
    pub upper: StatsValue,
    pub count: u64,
}

pub enum StatsValue {
    Null, Integer(i64), Float(f64), Bool(bool),
    String(String), Decimal { value: i128, scale: u8 },
}
```

`StatsValue` excluye `Value::Bytes` (BLOB sin semántica de orden ni
equality útil para el planner) y vive en `catalog` para mantener la
capa independiente del frontend SQL — mismo patrón que `DefaultLiteral`.

### `StatsMeta` extendido

```rust
pub struct StatsMeta {
    pub name: String,
    pub row_count: u64,
    pub analyzed_at_nanos: u128,
    pub columns: Vec<ColumnStats>,  // ← nuevo en P4
}
```

Layout serializado (apéndice al payload P3b):

```
[ P3b: name (string) | row_count (u64) | analyzed_at_nanos (u128) ]
[ P4: format_version (u8 = 1) | column_count (u16) | per-col: ColumnStats ]
```

### HyperLogLog (m = 256, b = 8 bits del hash para el índice)

- 256 bytes por columna (1 registro de 1 byte cada uno).
- Hash: FNV-1a 64-bit sobre `stats_value_bytes(v)` (encoding canónico
  por tipo). Mismo hash que el resto del motor (ADR-0002).
- Estimación: α₂₅₆ · m² / Σ 2^(-M[j]) con corrección small-range
  (linear counting) cuando E ≤ 2.5m y hay registros en cero.
- Error empírico esperado: ~6.5% típico hasta 100k distinct values;
  validado en test `p4_ndv_aproximado_dentro_del_margen` (200 distinct
  sobre 1k rows, tolerancia ±25%).

### MCV top-K = 10

- `HashMap<bytes(v), (StatsValue, count)>` durante el scan.
- Cap de memoria: 50 000 entries únicas. Al llegar al cap, entradas
  nuevas se descartan pero las existentes siguen contando — sesgo
  hacia valores tempranos frecuentes, aceptable para top-K en
  cardinalidad media-baja.
- Post-scan: sort por count desc, take 10.

### Histograma equi-depth ~16 buckets

- Reservoir sample determinístico: primeras 10 000 filas observadas
  (no aleatorio — evita RNG no-reproducible que rompería tests).
- Solo para tipos ordenables: INT, FLOAT, BOOL, TEXT, DATE, DATETIME,
  TIME, UUID, DECIMAL. JSON / BLOB → histograma vacío.
- Post-scan: sort + slice en N buckets aproximadamente equi-count
  (los primeros `n % buckets` reciben un elemento extra).

### `exec_analyze_table`

Un solo full-scan del B+tree:

```
for kv in scan_rows(root_page):
    let decoded = decode_row(&meta, &kv.value)?;
    for (i, col) in meta.columns.iter().enumerate():
        collectors[i].observe(decoded.get(&normalize_ident(&col.name)));
```

Post-scan: cada `ColumnCollector::finalize()` produce un `ColumnStats`.

Costo: O(rows × cols). Para 100k filas × 5 cols → ~500k observaciones,
en el orden de cientos de milisegundos. Aceptable porque `ANALYZE` es
explícito y manual.

### Bump VERSION 32 → 33

Política conservadora ([storage.rs](../../src/storage.rs)):
**no hay auto-upgrade entre versiones**. Aunque el cambio es aditivo
(no toca records existentes), DBs V32 son rechazadas con
`[GBY-1003] UNSUPPORTED_FORMAT_VERSION` y el mensaje estándar de
"dump + re-create".

Razón: un binario V32 viendo el `format_version` byte después de
`analyzed_at_nanos` lo trataría como basura (intentaría seguir
parseando o crashearía al re-compactar el catálogo).

## Alternativas consideradas

1. **Particionar en P4a/b/c** (NDV, luego MCV, luego histograma).
   - Descartado: tres bumps de VERSION (32→33→34→35) cuestan tres
     reset de DBs cada uno. Un solo bump con `format_version`
     interno futuro-proof cubre las tres features de un saque.

2. **No bumpear VERSION** (zero-bump como P1/P2/P3).
   - Descartado: el `format_version` byte aparece en una posición
     fija del payload existente; binarios viejos lo confundirían.

3. **Reservoir sampling aleatorio** (Vitter Algorithm R).
   - Descartado: RNG en `exec_analyze_table` rompería tests
     determinísticos. La toma de los primeros 10k es sesgada al
     comienzo del scan pero el scan ya itera en orden de PK — para
     un histograma equi-depth post-sort el sesgo es despreciable.

4. **Sketch HLL++ con bias correction de Google (2007)**.
   - Descartado: complejidad agrega ~200 LOC y el error de HLL
     básico ya es suficiente para guiar el planner. Reevaluar si
     P5 mide regresiones por bias.

5. **Top-K vía Misra-Gries / Space-Saving** (streaming exact).
   - Descartado: el HashMap con cap simple es más barato a la
     cardinalidad de columnas reales (< 50k distinct típicos). Si
     una columna excede el cap, perdemos accuracy en el long tail,
     pero el top-K queda correcto porque las frecuentes se ven
     temprano.

## Tests

7 tests nuevos en `tests/integration_test.rs` (suite p4_*):

- `p4_analyze_persiste_column_stats`: 3 columnas → 3 ColumnStats con
  nombres correctos, `row_count = 5`.
- `p4_ndv_aproximado_dentro_del_margen`: 1k rows con 200 distinct →
  HLL devuelve 200 ± 25%.
- `p4_null_count_exacto`: 3 NULLs en `maybe`, 0 en `id` (PK).
- `p4_mcv_top_k_ordenado_por_frecuencia`: 'a'×6 > 'b'×3 > 'c'×2 >
  'd'×1 → mcv[0] = ('a', 6), orden descendente.
- `p4_histograma_buckets_aproximadamente_equi_depth`: 100 filas
  distintas → suma de buckets = 100, ratio max/min ≤ 2x.
- `p4_column_stats_sobreviven_reopen`: regression de P3b extendido;
  EXPLAIN en sesión nueva muestra `est.rows=4 cols=2`.
- `p4_drop_table_borra_column_stats`: DROP TABLE limpia el record
  completo (igual que P3b — el column_stats es payload del mismo
  record).

## Consecuencias

**Positivas**

- (+) P5 (planner-as-optimizer) tiene materia prima real para
  decidir entre `Hash Join`, `Nested Loop`, choice de índice, etc.
- (+) EXPLAIN gana visibilidad: `[est.rows=N cols=M]` indica que
  hay stats columnares disponibles, no solo agregadas.
- (+) Auditoría: `ColumnStats::null_count` exacto sirve para
  detectar columnas mal modeladas (90% NULL → considerar NULL flag
  o separar tabla).

**Negativas / Limitaciones honestas**

- (-) Las stats **no se consumen todavía** en el planner. P4 es
  data collection. La selectividad estimada para `WHERE c=X`
  podría usarse en `classify_scan` pero el código actual ignora
  `ndv/mcv/histogram`.
- (-) Reservoir sample de 10k no es uniforme — sesgo al comienzo
  del scan. Para tablas con tendencia temporal en PK (ej. logs por
  timestamp) el histograma sub-representa filas recientes.
- (-) Cap de MCV en 50k entries: columnas con >50k distinct values
  pierden accuracy en el long tail. Top-K queda correcto si los
  valores frecuentes aparecen antes del cap.
- (-) Sin auto-ANALYZE: stats por columna también pueden volverse
  stale silenciosamente (heredado de P3/P3b). EXPLAIN no muestra
  staleness — pendiente.
- (-) BLOB y JSON: `null_count` y `ndv` se calculan; MCV e
  histograma quedan vacíos. El planner no podrá usar esas columnas
  para predicate selectivity.
- (-) Bump VERSION fuerza re-creación de DBs V32 (igual que P3b
  forzó re-creación de V31).

## Limitaciones / Trabajo futuro

- **P5**: consumir `ndv` para estimar selectividad de `=`/`!=`,
  `mcv` para skew detection, `histogram` para rangos `BETWEEN`.
- **P5b** (Gap 10 del bench): composite index lookup
  `WHERE c1 = X AND c2 = Y` — usar `ndv` combinado de (c1, c2)
  para decidir entre full-scan + filter vs. composite index seek.
- **Auto-ANALYZE**: scheduler que dispare cuando una tabla cambia
  >10% de rows. Requiere infraestructura de jobs.
- **EXPLAIN con staleness**: `est.rows=N (stats hace 3d 5h)`.
- **HLL++**: si el error empírico de HLL básico molesta al
  planner, agregar bias correction.
- **`ANALYZE` global** sin argumento: iterar todas las tablas.
- **TRUNCATE TABLE**: cuando llegue, debe borrar el record completo
  (heredado de P3b — sigue pendiente).
