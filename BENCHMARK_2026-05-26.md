# Benchmark gabysql

## Metadata

- **Fecha**: 2026-05-27 19:03:43 -04:00
- **Commit**: `87583a8` (branch `main`)
- **Toolchain**: rustc 1.95.0 (59807616e 2026-04-14) — x86_64-pc-windows-gnu
- **OS**: Microsoft Windows 11 Home Single Language 10.0.26200
- **CPU**: Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz (8 threads lógicos)
- **RAM**: 15,9 GB
- **Build profile**: release (LTO según Cargo.toml)
- **Warmup descartado**: 5 iters por operación


## Escenario: microblog

- DB: `bench-output\microblog.db`
- iters: 100 (warmup descartado: 5)
- phase: all

### Carga

| Operación | Filas | Tiempo (s) | Throughput (rows/s) |
|---|---:|---:|---:|
| INSERT users | 10000 | 1.123 | 8903 |
| INSERT posts | 40000 | 1.834 | 21810 |

### Queries

| Operación | N | P50 (µs) | P95 (µs) | P99 (µs) | Min (µs) | Max (µs) | Filas | Notas |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| Q1 PK lookup (users.id) | 95 | 24.80 | 44.00 | 61.30 | 9.60 | 67.20 | 1 | 1000 ids dispersos |
| Q2a Indexed eq posts.user_id (sin idx) | 0 | — | — | — | — | — | 0 | pre CREATE INDEX — ERROR: [GBY-4001] WHERE solo soporta PK (id) o columnas con índice secundario; 'user_id' no está indexada |
| Q2b Indexed eq posts.user_id (con idx) | 95 | 52.30 | 160.70 | 258.60 | 14.60 | 270.10 | 4 | idx_posts_user activo |
| Q3 Range scan PK (posts.id BETWEEN) | 95 | 263.70 | 305.60 | 312.90 | 176.40 | 330.00 | 100 | rango 100 |
| Q4 JOIN posts×users (BETWEEN 1..100) | 95 | 478698.2 | 716661.0 | 1048288 | 426749.2 | 1096748 | 100 | join via idx PK |
| Q5 Aggregate COUNT(*) WHERE likes>50 | 45 | 202775.3 | 240268.7 | 290408.0 | 182540.0 | 290408.0 | 1 | full scan posts |
| Q6 UPDATE posts.likes (auto-commit) | 95 | 133.10 | 300.60 | 448.30 | 57.60 | 488.40 | 1 | tx por iter (incluye fsync) |

**Queries con problema (N/A o ERROR):**

- `Q2a Indexed eq posts.user_id (sin idx)` → pre CREATE INDEX — ERROR: [GBY-4001] WHERE solo soporta PK (id) o columnas con índice secundario; 'user_id' no está indexada


## Escenario: events

- DB: `bench-output\events.db`
- iters: 100 (warmup descartado: 5)
- phase: all

### Carga

| Operación | Filas | Tiempo (s) | Throughput (rows/s) |
|---|---:|---:|---:|
| INSERT events | 200000 | 8.681 | 23040 |

### Queries

| Operación | N | P50 (µs) | P95 (µs) | P99 (µs) | Min (µs) | Max (µs) | Filas | Notas |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| Q1 Full scan COUNT WHERE latency>1000 | 5 | 1333512 | 1882056 | 1882056 | 1208055 | 1882056 | 1 | scan 200K (latency_ms no idx) |
| Q2 GROUP BY event_type | 5 | 1149541 | 1204097 | 1204097 | 1138746 | 1204097 | 7 | agg + hash group |
| Q3 Indexed lookup type='login' LIMIT 100 | 0 | — | — | — | — | — | 0 | tipo frecuente ~50% — ERROR: [GBY-4001] WHERE solo soporta PK (id) o columnas con índice secundario; 'event_type' no está indexada |
| Q4 Indexed lookup type='admin_action' LIMIT 100 | 0 | — | — | — | — | — | 0 | tipo raro ~1% — ERROR: [GBY-4001] WHERE solo soporta PK (id) o columnas con índice secundario; 'event_type' no está indexada |
| Q5 Scalar func WHERE LENGTH(payload)>100 | 5 | 906060.0 | 929737.3 | 929737.3 | 893639.9 | 929737.3 | 1 | G2 scalar in WHERE |
| Q6 INTERSECT (latency>500 ∩ user_id<100) | 5 | 1591849 | 2445207 | 2445207 | 1469718 | 2445207 | 7 | I set op |
| Q7 Scalar subquery in SELECT LIMIT 10 | 15 | 7546652 | 8604318 | 8801558 | 7119853 | 8801558 | 10 | H scalar subquery |
| Q8 Derived table GROUP BY filter cnt>1000 | 5 | 897995.3 | 928209.0 | 928209.0 | 861113.9 | 928209.0 | 7 | H derived table |

**Queries con problema (N/A o ERROR):**

- `Q3 Indexed lookup type='login' LIMIT 100` → tipo frecuente ~50% — ERROR: [GBY-4001] WHERE solo soporta PK (id) o columnas con índice secundario; 'event_type' no está indexada
- `Q4 Indexed lookup type='admin_action' LIMIT 100` → tipo raro ~1% — ERROR: [GBY-4001] WHERE solo soporta PK (id) o columnas con índice secundario; 'event_type' no está indexada


## Escenario: catalog

- DB: `bench-output\catalog.db`
- iters: 100 (warmup descartado: 5)
- phase: all

### Carga

| Operación | Filas | Tiempo (s) | Throughput (rows/s) |
|---|---:|---:|---:|
| INSERT orders | 10000 | 0.472 | 21171 |
| INSERT order_lines (PK compuesta) | 100000 | 12.053 | 8297 |

### Queries

| Operación | N | P50 (µs) | P95 (µs) | P99 (µs) | Min (µs) | Max (µs) | Filas | Notas |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| Q1 Composite PK full (order_id AND line_no) | 95 | 144857.0 | 160329.2 | 172678.2 | 137190.2 | 183651.3 | 1 | lookup exacto |
| Q2 Composite PK partial (order_id only) | 15 | 162455.2 | 173573.1 | 174666.8 | 139792.3 | 174666.8 | 10 | fallback a scan |
| Q3 JOIN orders×order_lines (id BETWEEN 1..10) | 95 | 64139.7 | 71177.6 | 78835.0 | 61684.5 | 88348.0 | 0 | join + composite |
| Q4 Aggregate SUM(qty*price) GROUP LIMIT 100 | 0 | — | — | — | — | — | 0 | agg 100K rows — ERROR: se esperaba sÃ­mbolo ) |

**Queries con problema (N/A o ERROR):**

- `Q4 Aggregate SUM(qty*price) GROUP LIMIT 100` → agg 100K rows — ERROR: se esperaba sÃ­mbolo )


## Observaciones y recomendaciones

### Hallazgos por escenario

**microblog (40K posts / 10K users):**
- `Q1 PK lookup` p50=24.8µs — el B+Tree de PK responde como debe.
- `Q2b con idx` p50=52.3µs vs `Q2a sin idx` que falla con GBY-4001: el motor **rechaza** WHERE sobre columna no-PK no-indexada en vez de full-scan. Esto es defensivo y conservador, pero rompe la portabilidad de SQL "ingenuo". Sugerencia: degradar a full-scan con warning en vez de error duro, controlado por flag.
- `Q4 JOIN posts×users` p50=479ms para 100 filas matched — claramente nested-loop sin hash/merge. Es el outlier más grande del set.
- `Q5 COUNT WHERE likes>50` (full scan 40K rows) p50=203ms. WHERE sobre columna no indexada **sí** se permite cuando es comparación numérica (`>`); aparentemente el rechazo 4001 aplica solo a igualdad. Inconsistencia que merece documentarse.
- `Q6 UPDATE` p50=133µs incluyendo fsync por iter — razonable.

**events (200K rows):**
- `CREATE INDEX idx_events_type` falla silenciosamente durante load (best-effort): "hoja B+Tree no admite una sola entrada". Con 200K eventos y solo 8 valores distintos de `event_type` (login=50%, view=20%…), el índice colapsa en pocas keys con listas gigantes de row_ids que no caben en una página. **Bug pre-existente**: el formato del idx secundario no maneja keys de alta cardinalidad invertida. Q3/Q4 (lookups por `event_type`) caen entonces a N/A.
- `Q7 Scalar subquery in SELECT LIMIT 10` p50=**7.5 segundos** — la subquery `(SELECT COUNT(*) FROM events)` se evalúa **por cada fila** del outer (no cacheada). Optimización obvia: detectar subqueries no correlacionadas y memoizar.
- Full scans de 200K rows: 0.9–1.9s. Throughput de ~100K rows/s.

**catalog (10K orders / 100K líneas con PK compuesta):**
- `Q1 Composite PK full` (order_id AND line_no) p50=145ms — sorprendentemente lento para un lookup exacto. El fingerprint FNV-1a-64 introducido en K2 puede estar haciendo un scan en vez de B+Tree lookup. Vale auditar.
- `Q4 SUM(qty*price)` falla en parser: `qty * price` dentro de `SUM(...)` no se acepta. Parser de funciones agregadas no usa el Expr completo de G3.

### Top 3 fastest ops

| # | Operación | P50 |
|---|---|---:|
| 1 | microblog Q1 PK lookup users.id | 24.8 µs |
| 2 | microblog Q2b indexed eq con idx | 52.3 µs |
| 3 | microblog Q6 UPDATE auto-commit | 133.1 µs |

### Top 3 slowest ops

| # | Operación | P50 |
|---|---|---:|
| 1 | events Q7 Scalar subquery in SELECT | 7.55 s |
| 2 | events Q6 INTERSECT | 1.59 s |
| 3 | events Q1 Full scan latency>1000 | 1.33 s |

### Recomendaciones priorizadas

1. **(alta) Subquery no correlacionada en SELECT list**: cachear el resultado (Q7 = 7.5s → debería ser ~1ms + 1 scan). Probable factor 1000x.
2. **(alta) Índice secundario con baja cardinalidad**: rediseñar el formato del idx para usar listas externas o un B+Tree por value en vez de empacar todos los row_ids en una entrada. Bloquea bench de `event_type`.
3. **(media) Inconsistencia GBY-4001**: o se rechazan TODAS las queries no-indexadas (incluyendo `>`, `LIKE`) o ninguna. La política mixta confunde.
4. **(media) JOIN nested-loop**: Q4 microblog 479ms para 100 filas es ~4ms/fila. Hash join cuando el inner table cabe en memoria reduciría a < 50ms.
5. **(baja) `SUM(expr)` con aritmética**: extender el parser de agregados para usar `Expr` completo. Pequeño.
6. **(baja) Composite PK lookup en 145ms**: investigar si el lookup es realmente O(log n) o degeneró a scan. Para 100K rows con B+Tree esperaría < 200µs.

### Setup reproducible

```powershell
$env:Path = "$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin;" + $env:Path
cargo build --release --bin gabysql-bench
.\target\release\gabysql-bench.exe --scenario microblog --phase all --db bench-output\microblog.db --out BENCHMARK_2026-05-26.md --iters 100
.\target\release\gabysql-bench.exe --scenario events    --phase all --db bench-output\events.db    --out BENCHMARK_2026-05-26.md --iters 100
.\target\release\gabysql-bench.exe --scenario catalog   --phase all --db bench-output\catalog.db   --out BENCHMARK_2026-05-26.md --iters 100
```
