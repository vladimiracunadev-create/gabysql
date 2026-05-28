# Benchmark gabysql

> **Evaluación profesional de desempeño** sobre `main`. Tres escenarios sintéticos representativos (OLTP-like, analítica mediana, K2 PK compuesta) corridos en una sola máquina, con metodología reproducible y caveats honestos.
>
> 📌 **Corrida vigente**: sesión **2026-05-27**, commit `87583a8` (post bloques G/H/I/K1/K2; previa a L1+L2+L3 + residuales + V de la misma fecha que llevaron VERSION 8 → 13).
>
> 📂 **Convención**: este archivo se actualiza in-place a medida que el motor mejora. Corridas previas se archivan en `docs/benchmarks/BENCHMARK-YYYY-MM-DD.md` solo cuando hay un cambio de fondo que justifica mantener el snapshot histórico para comparar (e.g. fix de un issue importante, bump de VERSION en disco, cambio de hardware del runner). Mientras tanto, este es el único reporte.

---

## 📌 Resumen ejecutivo

**Lo que anda bien**: el motor entrega lookups por PK en **~25 µs**, indexed equality en **~50 µs**, y UPDATEs auto-commit (con fsync por iter) en **~130 µs** — números consistentes con un B+Tree real sobre PK e índice secundario. La carga masiva sostiene **~20K rows/s** en INSERT con FK enforcement y mantenimiento de índices.

**Lo que duele**: una subquery escalar no-correlacionada en el SELECT list (`SELECT (SELECT COUNT(*) FROM events) FROM events LIMIT 10`) tardó **7.5 segundos** porque se re-evalúa por cada fila del outer. Factor **~1000× de optimización disponible** con memoización trivial. JOIN nested-loop sin alternativa hash; `CREATE INDEX` colapsa con cardinalidad muy baja sobre datasets grandes; composite PK lookup tarda **145 ms** vs los ~200 µs esperados de un B+Tree de 100K rows.

**Veredicto operativo**: el motor está listo para workloads OLTP de tamaño chico-medio con queries por PK / índice. Inadecuado todavía para analítica pura sin trabajo de planner.

---

## 🔬 Metodología

| Parámetro | Valor |
|---|---|
| **Fecha** | 2026-05-27 19:03 –04:00 |
| **Commit** | `87583a8` · branch `main` |
| **Toolchain** | rustc 1.95.0 — `x86_64-pc-windows-gnu` |
| **Profile** | `release` (LTO según `Cargo.toml`) |
| **OS** | Windows 11 Home Single Language · build 10.0.26200 |
| **CPU** | Intel i7-8550U @ 1.80 GHz · 8 threads lógicos |
| **RAM** | 15.9 GB |
| **Disco** | SSD local |
| **Iteraciones por op** | 100 (5 warmup descartados) |
| **Métrica de latencia** | P50, P95, P99, min, max en µs sobre runs individuales |
| **Métrica de throughput** | rows/s sobre la fase de carga completa |
| **Page cache** | LRU bounded default (1024 páginas ≈ 4 MB) |
| **WAL** | After-image, sin checkpoint |
| **Concurrencia** | Single-thread (Mutex global, sin contención) |
| **Harness** | `src/bin/gabysql-bench.rs` (~600 LoC, zero-deps, embebe la librería directo) |

### Caveats explícitos

1. **Una sola máquina, una sola corrida** — no hay intervalos de confianza estadísticos. P95/P99 sobre N=95 son indicativos, no rigurosos.
2. **Datos sintéticos** — PRNG xorshift64. Distribuciones aproximan zipf con `event_type` (top tipo ~50%) pero no reflejan un workload real.
3. **Sin OS cache warm-up** dirigido — los primeros 5 iters se descartan, pero la página puede o no estar caliente.
4. **Sin comparativa contra SQLite/PostgreSQL/DuckDB** — el objetivo es medir *expectativa relativa entre operaciones del propio motor*, no posicionamiento competitivo. Esa comparativa requiere un setup separado (mismo dataset, mismo hardware, mismas queries) y queda fuera.
5. **El reporte mide latencia desde `Engine::exec` hasta retorno** — no incluye round-trip de cliente HTTP / JSON / autenticación. Es un floor del motor, no del producto end-to-end.

### Reproducir

```powershell
$env:Path = "$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin;" + $env:Path
cargo build --release --bin gabysql-bench
mkdir bench-output

.\target\release\gabysql-bench.exe --scenario microblog --phase all `
    --db bench-output\microblog.db --out BENCHMARK_2026-05-27.md --iters 100
.\target\release\gabysql-bench.exe --scenario events --phase all `
    --db bench-output\events.db --out BENCHMARK_2026-05-27.md --iters 100
.\target\release\gabysql-bench.exe --scenario catalog --phase all `
    --db bench-output\catalog.db --out BENCHMARK_2026-05-27.md --iters 100
```

---

## 🗂 Escenario 1 — `microblog` (OLTP-like)

### Schema

```sql
CREATE TABLE users (
    id         INT PRIMARY KEY,
    nombre     TEXT NOT NULL,
    email      TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL
);

CREATE TABLE posts (
    id         INT PRIMARY KEY,
    user_id    INT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    titulo     TEXT NOT NULL,
    body       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    likes      INT NOT NULL DEFAULT 0
);

CREATE INDEX idx_posts_user ON posts(user_id);
```

- Filas: 10,000 users + 40,000 posts ≈ 4 posts/user promedio (distribución no uniforme).
- DB size on disk: **9.8 MB**.

### Carga

| Operación | Filas | Tiempo | Throughput |
|---|---:|---:|---:|
| INSERT users (con UNIQUE check) | 10,000 | 1.123 s | **8,903 rows/s** |
| INSERT posts (con FK enforce + idx maintenance) | 40,000 | 1.834 s | **21,810 rows/s** |

> Posts es más rápido porque el INSERT no tiene UNIQUE pre-check; las dos FK lookups (parent users + insert idx) suman menos que la verificación de unicidad de email.

### Queries

| Op | P50 (µs) | P95 (µs) | P99 (µs) | Max (µs) | Filas | Notas |
|---|---:|---:|---:|---:|---:|---|
| **Q1** PK lookup `WHERE id=N` | **24.8** | 44.0 | 61.3 | 67.2 | 1 | 1000 ids dispersos |
| **Q2a** Indexed eq sin idx | — | — | — | — | — | ❌ `[GBY-4001]` (ver Issue #3) |
| **Q2b** Indexed eq con idx | **52.3** | 160.7 | 258.6 | 270.1 | 4 | idx activo |
| **Q3** Range scan PK `BETWEEN a AND a+99` | 263.7 | 305.6 | 312.9 | 330.0 | 100 | walk leaf |
| **Q4** JOIN `posts × users` (100 filas) | **478,698** | 716,661 | 1,048,288 | 1,096,748 | 100 | nested-loop (ver Issue #6) |
| **Q5** Aggregate `COUNT(*) WHERE likes>50` | 202,775 | 240,269 | 290,408 | 290,408 | 1 | full scan 40K rows |
| **Q6** UPDATE auto-commit (incluye fsync) | 133.1 | 300.6 | 448.3 | 488.4 | 1 | tx por iter |

---

## 🗂 Escenario 2 — `events` (analítica mediana)

### Schema

```sql
CREATE TABLE events (
    id         INT PRIMARY KEY,
    event_type TEXT NOT NULL,
    user_id    INT NOT NULL,
    payload    TEXT NOT NULL,
    ts         TEXT NOT NULL,
    latency_ms INT NOT NULL
);

CREATE INDEX idx_events_type ON events(event_type);  -- ⚠️ FALLA (ver Issue #2)
```

- Filas: 200,000 events con ~8 valores únicos de `event_type` (distribución zipf-ish: login ~50%, view ~20%, long tail).
- DB size on disk: **56 MB**.

### Carga

| Operación | Filas | Tiempo | Throughput |
|---|---:|---:|---:|
| INSERT events | 200,000 | 8.681 s | **23,040 rows/s** |

### Queries

| Op | P50 (µs) | P95 (µs) | P99 (µs) | Max (µs) | Filas | Notas |
|---|---:|---:|---:|---:|---:|---|
| **Q1** `COUNT WHERE latency_ms>1000` (full scan) | 1,333,512 | 1,882,056 | 1,882,056 | 1,882,056 | 1 | scan 200K |
| **Q2** `GROUP BY event_type` agg | 1,149,541 | 1,204,097 | 1,204,097 | 1,204,097 | 7 | hash group |
| **Q3** Indexed lookup `type='login'` | — | — | — | — | — | ❌ idx no creado (Issue #2) |
| **Q4** Indexed lookup `type='admin_action'` | — | — | — | — | — | ❌ idx no creado (Issue #2) |
| **Q5** `WHERE LENGTH(payload)>100` (G2) | 906,060 | 929,737 | 929,737 | 929,737 | 1 | scalar fn en WHERE |
| **Q6** `INTERSECT` entre dos predicados (I) | 1,591,849 | 2,445,207 | 2,445,207 | 2,445,207 | 7 | set op |
| **Q7** Scalar subquery en SELECT list (H) | **7,546,652** | 8,604,318 | 8,801,558 | 8,801,558 | 10 | ⚠️ re-evaluación por fila (Issue #1) |
| **Q8** Derived table + GROUP BY (H) | 897,995 | 928,209 | 928,209 | 928,209 | 7 | scan + agg materializado |

---

## 🗂 Escenario 3 — `catalog` (K2 PK + índice compuestos)

### Schema

```sql
CREATE TABLE orders (
    id          INT PRIMARY KEY,
    customer_id INT NOT NULL,
    total       INT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE TABLE order_lines (
    order_id INT NOT NULL,
    line_no  INT NOT NULL,
    sku      TEXT NOT NULL,
    qty      INT NOT NULL,
    price    INT NOT NULL,
    PRIMARY KEY (order_id, line_no)        -- K2: composite PK
);

CREATE INDEX idx_lines_order_sku
    ON order_lines (order_id, line_no);    -- K2: composite index
```

- Filas: 10,000 orders + 100,000 order_lines (≈10 lines/order).
- DB size on disk: **13 MB**.

### Carga

| Operación | Filas | Tiempo | Throughput |
|---|---:|---:|---:|
| INSERT orders | 10,000 | 0.472 s | 21,171 rows/s |
| INSERT order_lines (PK compuesta + idx compuesto) | 100,000 | 12.053 s | **8,297 rows/s** |

> El INSERT en `order_lines` es 2.5× más lento que en `orders` porque cada fila calcula el fingerprint FNV-1a-64 de la PK compuesta, consulta el B+Tree por duplicado, y mantiene el índice compuesto (que también calcula su propio fingerprint).

### Queries

| Op | P50 (µs) | P95 (µs) | P99 (µs) | Max (µs) | Filas | Notas |
|---|---:|---:|---:|---:|---:|---|
| **Q1** Composite PK full `WHERE order_id=X AND line_no=Y` | **144,857** | 160,329 | 172,678 | 183,651 | 1 | ⚠️ esperaba <200µs (Issue #4) |
| **Q2** Composite PK partial `WHERE order_id=X` | 162,455 | 173,573 | 174,667 | 174,667 | 10 | fallback a FullScan (esperado) |
| **Q3** JOIN orders × order_lines | 64,140 | 71,178 | 78,835 | 88,348 | 0 | join nested-loop |
| **Q4** `SUM(qty*price) GROUP BY order_id` | — | — | — | — | — | ❌ parse error (Issue #5) |

> **Composite PK full lookup (Q1) tarda casi lo mismo que el partial (Q2)** — fuerte señal de que el lookup compuesto NO está usando el B+Tree por el fingerprint y degenera a scan. Bug a auditar.

---

## 🏆 Rankings de toda la sesión

### Top-3 más rápidas (P50)

| # | Operación | Escenario | P50 |
|---|---|---|---:|
| 🥇 | PK lookup `users.id` | microblog | **24.8 µs** |
| 🥈 | Indexed eq `posts.user_id` con idx | microblog | **52.3 µs** |
| 🥉 | UPDATE auto-commit `posts.likes` | microblog | **133 µs** |

### Top-3 más lentas (P50)

| # | Operación | Escenario | P50 |
|---|---|---|---:|
| 1 | Scalar subquery en SELECT list | events | **7.55 s** ⚠️ |
| 2 | `INTERSECT` entre dos predicados | events | 1.59 s |
| 3 | Full scan `COUNT WHERE latency>1000` | events | 1.33 s |

---

## 🐞 Issues encontrados

> Cada issue es candidato a fix en sesiones futuras. Severidad: 🔴 crítico · 🟡 importante · 🟢 menor.

### Issue #1 — 🔴 Scalar subquery no-correlacionada se re-evalúa por fila

- **Escenario**: events Q7.
- **Query**: `SELECT id, (SELECT COUNT(*) FROM events) AS total FROM events LIMIT 10`.
- **Observado**: P50 **7,546,652 µs (7.5 s)** para LIMIT 10 — es decir, cada una de las 10 filas dispara un scan completo de 200K rows.
- **Esperado**: <10 ms total. La subquery es no-correlacionada (no referencia el outer) y debería evaluarse exactamente UNA vez y memoizarse.
- **Causa probable**: el path de `Expr::ScalarSubquery` (bloque H) no distingue correlated vs no-correlated y siempre re-ejecuta.
- **Fix sugerido**: walker `expr_subquery_is_correlated(&Expr, outer_scope) -> bool`. Si `false`, ejecutar una sola vez antes del loop y reusar el `Value` cacheado.
- **Impacto estimado**: factor **~1000×** para queries con scalar subqueries no-correlacionadas.

### Issue #2 — 🔴 `CREATE INDEX` colapsa con cardinalidad muy baja

- **Escenario**: events Q3 / Q4.
- **Setup**: 200,000 rows con 8 valores únicos de `event_type` → cada bucket tiene ~25K row_ids.
- **Observado**: `CREATE INDEX idx_events_type ON events(event_type)` falla silenciosamente con error interno tipo "hoja B+Tree no admite una sola entrada". Sin idx, los WHERE por igualdad sobre `event_type` devuelven `[GBY-4001]`.
- **Causa probable**: el formato del bucket del índice secundario (ADR-0005) empaca todos los row_ids de un value contiguos en una página. Cuando una key tiene más row_ids de los que caben en una página, el bucket no se puede escribir.
- **Fix sugerido**: overflow chain para buckets grandes, o cambio a "un B+Tree por value" para keys de muy baja cardinalidad.
- **Impacto**: bloquea cualquier indexado sobre columnas categóricas con pocos valores y datasets grandes (justamente el caso de uso típico de un índice).

### Issue #3 — 🟡 `[GBY-4001]` inconsistente: rechaza `=` pero acepta `>`

- **Escenarios**: microblog Q2a, Q5 / events Q1.
- **Observado**:
  - `WHERE user_id = 42` (no indexada, igualdad) → `[GBY-4001]` (rechazado).
  - `WHERE likes > 50` (no indexada, comparación) → permitido como full scan.
- **Causa**: el planner trata `=` como obligatoriamente fast-path (PK o índice) mientras que `<`, `>`, `LIKE`, etc., caen siempre a full scan + post-filter.
- **Discusión**: la política conservadora con `=` evita scans accidentales sobre tablas grandes, pero la inconsistencia confunde al usuario y bloquea queries simples. Dos opciones razonables:
  - **(A)** Permitir full scan con `=` también (alinear con el resto).
  - **(B)** Aplicar la misma restricción a TODOS los operadores (más estricto pero coherente).
- **Recomendación**: opción A + flag `--strict-where` para opción B.

### Issue #4 — 🟡 Composite PK lookup en 145 ms es sospechoso

- **Escenario**: catalog Q1 vs Q2.
- **Observado**: `WHERE order_id=X AND line_no=Y` (PK compuesta exacta) → **145 ms**. `WHERE order_id=X` (PK parcial, fallback a scan) → **162 ms**. Diferencia de solo 12% — el lookup exacto NO está aprovechando el B+Tree.
- **Esperado**: lookup exacto en O(log 100,000) ≈ **<500 µs**. Partial debería ser >100× más lento.
- **Causa probable**: el código del planner que reconoce "AND-equality sobre todas las cols de la PK compuesta" no se está disparando, o el cálculo del fingerprint FNV-1a-64 no se usa para indexar el B+Tree (que sigue keyed por `i64`).
- **Fix sugerido**: trace explícito del path de planificación para composite PK; asegurar que el fast-path por fingerprint se ejecuta y que el B+Tree de la tabla está keyed por el fingerprint (no por `Value::Integer` de una columna ausente).

### Issue #5 — 🟢 Parser de agregados no usa `Expr` completo de G3

- **Escenario**: catalog Q4.
- **Query**: `SELECT order_id, SUM(qty * price) AS total FROM order_lines GROUP BY order_id LIMIT 100`.
- **Observado**: error de parser `se esperaba símbolo )` después de `qty`.
- **Causa**: `parse_agg_arg` parsea un único `ident` como argumento. No baja a `parse_expr` ni reconoce aritméticos.
- **Fix sugerido**: cambiar `AggArg::Column(String)` por `AggArg::Expr(Expr)` y reusar el evaluador de G1/G2/G3. Cambio mediano pero contenido.

### Issue #6 — 🟢 JOIN nested-loop sin alternativa

- **Escenario**: microblog Q4.
- **Observado**: JOIN de 100 posts × users (table size ≈ 10K) tardó **479 ms**, es decir ~4.8 ms por fila matched.
- **Causa**: nested-loop O(N×M) sin opción de hash join. El index-loop existe pero solo cuando el `ON` apunta a PK o índice del *right* y el kind es INNER/LEFT.
- **Fix sugerido**: hash join cuando el inner table cabe en `page_cache` (4 MB default ≈ 100K rows con filas chicas).
- **Impacto**: factor 10×–100× según el tamaño del inner.

---

## 🎯 Recomendaciones priorizadas

1. **(🔴 alta)** Fix Issue #1 — memoización de subquery escalar no-correlacionada. **Factor ~1000×**. Cambio chico.
2. **(🔴 alta)** Fix Issue #2 — overflow chain en bucket de índice secundario, o B+Tree por value. Desbloquea indexado sobre columnas categóricas con datasets reales.
3. **(🟡 media)** Auditar Issue #4 — composite PK lookup que no usa B+Tree. Si se confirma, el fix es el fast-path real prometido por K2.
4. **(🟡 media)** Resolver Issue #3 — política `[GBY-4001]` consistente (recomendación: permisiva con warning + flag estricto).
5. **(🟢 baja)** Fix Issue #5 — `AggArg::Expr` en lugar de `AggArg::Column`. Habilita `SUM(qty*price)`, `AVG(salary*1.1)`, etc.
6. **(🟢 baja)** Implementar hash join cuando el inner cabe en cache (Issue #6).

---

## 📁 Artefactos

| Archivo | Tamaño | Descripción |
|---|---:|---|
| `src/bin/gabysql-bench.rs` | ~600 LoC | Harness zero-deps, embebe la librería |
| `BENCHMARK_2026-05-27.md` | este archivo | Reporte profesional con metodología, números, issues, recos |
| `bench-output/microblog.db` | 9.8 MB | OLTP (10K users + 40K posts + idx) |
| `bench-output/events.db` | 56 MB | Analítica (200K events) |
| `bench-output/catalog.db` | 13 MB | K2 (10K orders + 100K order_lines con PK compuesta) |

---

## 🔁 Cómo extender este benchmark

- **Más iteraciones**: subir `--iters 1000` para reducir varianza en queries cortas.
- **Comparar contra SQLite/PG**: setup separado, mismo dataset (generar con un script común que escupe SQL portable), mismas queries (subset que ambos soporten).
- **Concurrencia**: hoy single-thread por el Mutex global. Cuando exista write-through MVCC, repetir con N threads concurrentes.
- **Workload skewed**: agregar escenarios con write/read ratios distintos (90/10, 50/50, 10/90) y medir degradación del WAL.
- **Cold cache**: instrumentar `Pager::reset_cache()` antes de cada query para medir el peor caso desde disco.
