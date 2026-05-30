# Benchmark gabysql

> **Evaluación profesional de desempeño** sobre `main`. **10 escenarios sintéticos** que cubren cada subsistema mayor del motor (OLTP, analítica, PK compuesta, RLS, DECIMAL exacto, window functions, recursive CTEs, procedural, tipos extendidos, constraints). Metodología reproducible, números honestos.
>
> 📌 **Corrida vigente**: sesión **2026-05-30** (Windows host + GNU toolchain, single-core perf).
>
> 📂 **Convención**: este archivo es el roll-up vivo + lectura ejecutiva. Cada corrida deja un snapshot inmutable en [`docs/benchmarks/BENCHMARK-YYYY-MM-DD.md`](docs/benchmarks/) generado automáticamente por `gabybench`. Corridas previas vivas: [BENCHMARK_2026-05-26.md](BENCHMARK_2026-05-26.md).

---

## 🎯 ¿Para qué sirve este benchmark?

1. **Verificar consistencia** — cada corrida produce números comparables; regresiones son visibles.
2. **Medir el efecto de las mejoras** — bumps de versión, fixes y nuevos bloques se evalúan contra los números previos.
3. **Detectar huecos del motor** — el bench falla limpio (skip-graceful) cuando una query tropieza con una limitación; el log expone qué falta.
4. **Decidir el roadmap por costo real** — si una query toma 27 segundos sobre 500 rows, vale la pena un planner; si toma 12µs, no.

**Lo que NO es**: comparación contra Postgres/SQLite/MySQL. Esto es **auto-benchmark**, no competitive bench. Para eso ver [docs/COMPETITIVE_ANALYSIS.md](docs/COMPETITIVE_ANALYSIS.md).

---

## 🧪 Las 10 DBs y qué subsistema cubre cada una

| # | DB | Tamaño | Subsistema cubierto | Bloque(s) del motor |
|---|---|---:|---|---|
| 1 | **microblog** | 50k rows (10k users + 40k posts) | OLTP básico: PK INT + UNIQUE TEXT + índice secundario + ordered-int range | Core + E1+E2+E3+F+J |
| 2 | **events** | 200k rows | Analítica mediana: full scan, range scan ordenado, aggregates, GROUP BY, UNION, DISTINCT | F + I + ADR-0017 |
| 3 | **orders_lines** | 120k rows (20k orders + 100k lines) | PK compuesta all-INT (K2) + índice compuesto + JOINs + CTAS + DROP COLUMN | K1 + K2 |
| 4 | **secdb** | 25k rows (5k customers + 20k orders) | Z: USERS + ROLES + `SET SESSION AUTH` + RLS USING + WITH CHECK | Z1 → Z3f |
| 5 | **finance** | 50k transactions | Y: DECIMAL(14,4) exacto + aritmética Decimal Y7/Y8 + SUM/AVG Y9 | Y6 → Y9 |
| 6 | **analytics** | 30k sales | W3: window functions OVER + PARTITION BY + RANK + LAG + SUM OVER | W3 |
| 7 | **graph** | 8k rows (2k nodes + 6k edges) | W2: `WITH RECURSIVE` (fixpoint con dedup) + V: vistas lógicas | W2 + V |
| 8 | **procflow** | 5k rows | X: triggers AFTER UPDATE + user functions + stored procedures + CALL | X1 + X3 + X3b |
| 9 | **types_zoo** | 10k specimens | Y completo: BLOB X'hex' + UUID + TIME + DATETIME + DECIMAL + INT widths + UNSIGNED | Y1 → Y6 |
| 10 | **constraint_zoo** | 15k rows (5k parents + 10k children) | L: CHECK col-level + FK con CASCADE on DELETE/UPDATE + UNIQUE | L1 + L2 |

**Total: 513.000 filas distribuidas** en 10 DBs sintéticas con seed determinístico (`SEED = 0x9E37_79B9_7F4A_7C15`).

---

## 📌 Resumen ejecutivo (2026-05-30)

### Lo que rinde bien (production-grade)

| Operación | Latencia p50 | Observación |
|---|---:|---|
| **PK lookup hot** | **11-12 µs** | B+tree get O(log n) — consistente en TODAS las DBs (microblog/finance/analytics/procflow/graph) |
| **PK lookup cold** | 13-26 µs | Cold cache, primer hit a página nueva |
| **Indexed eq (hash idx)** | **12-17 µs** | secondary index `find_by_eq` O(1) bucket |
| **Indexed eq (ordered-int)** | 14 µs | events `WHERE valor = 12345` con ordered-int idx |
| **UNIQUE TEXT lookup** | 20-22 µs | hash en TEXT secundario |
| **PK compuesta full** (K2) | **16-23 µs** | fingerprint FNV-1a-64 lookup |
| **INSERT in-tx** | 24-31 µs | sin fsync per-statement (in-tx amortizado) |
| **UPDATE por PK in-tx** | 25-32 µs | mismo orden |
| **DELETE por PK in-tx** | 30-39 µs | + cascade check |
| **WITH RECURSIVE 5 hops** | **2.7 ms** | fixpoint con guard 10K + dedup FNV-1a — sorprendentemente rápido |
| **Vista (V) SELECT 100 rows** | 12 ms | re-parsea + ejecuta source SQL |
| **User function call (X3b)** | 773 µs / 200 calls | overhead de eval_expr por fila |

### Lo que duele (necesita optimización)

| Operación | Latencia | Por qué | Fix sugerido |
|---|---:|---|---|
| **RANK() OVER (PARTITION BY)** | **27-46 s / 500 rows** | algoritmo cuadrático del ranking | W4 (re-implementar ranking O(n log n)) |
| **SUM() OVER (PARTITION BY) cumulativo** | **28-50 s / 500 rows** | mismo problema cuadrático | W4 |
| **Indexed range LARGE 160k rows** | 4.5 s | 28 µs/row decode | leaf cursor batch decode |
| **Full scan eq sin idx (200k rows)** | 720 ms | 3.6 µs/row, dominado por row decode | F3 (BETWEEN fallback) + leaf batch |
| **Aggregate full** (events COUNT, GROUP BY) | 700-820 ms | scan completo + aggregator simple | P4 stats + P5 planner |
| **UNION two ranges (events)** | 970 ms | dedup en memoria post-scan | F4 (streaming union) |
| **Composite index lookup qty+precio** | 220 ms (sin filtro) | full scan sobre `lines` (no usa idx compuesto) | clarificar fast-path en planner |
| **JOIN orders×lines** | 162 ms | nested-loop scan | P5 (planner con stats) |
| **RLS + SELECT** | 45-51 ms (5k rows) | overhead por-row del policy check | enforcement batch (deferido) |
| **SET SESSION AUTHORIZATION + SELECT** | 47 ms | PBKDF2 hash en cada SET | cache de auth session-scope |

### Costo de cada feature de seguridad (overhead aprox sobre baseline)

| Feature | Costo extra |
|---|---|
| Superuser SELECT (no auth) | baseline |
| Con `SET SESSION AUTH` + 1 policy USING | +47 ms por SELECT (autenticación PBKDF2 + filter per-row) |
| Con WITH CHECK en INSERT | +marginal (1 eval extra por row) |
| RLS + PK lookup | +37 ms (vs baseline 12 µs) — overhead dominado por SET AUTH |

### Costo de aritmética DECIMAL exacta (Y7/Y8/Y9)

| Operación | Latencia | vs INT equivalente |
|---|---:|---|
| **SUM(DECIMAL 50k rows)** | 121-159 ms | comparable a SUM(INT) |
| **AVG(DECIMAL 50k rows)** | 131-176 ms | + ~30% por la división final |
| **SELECT a-b LIMIT 1000** | 1.3-1.5 ms | ~1.3 µs/row Decimal sub |
| **SELECT a*1.05 LIMIT 1000** | 1.3-1.4 ms | ~1.3 µs/row Decimal mul scale-extending |
| **GROUP BY + SUM(DECIMAL)** | 205 ms / 1000 groups | acceptable |

**Veredicto**: la aritmética Decimal exacta (i128 + scale) **no introduce overhead significativo** respecto a INT — el costo está dominado por el scan, no por la aritmética. Y9 cumple su promesa.

### Costo de WITH RECURSIVE (W2)

| Profundidad | Latencia | Rows reach |
|---|---:|---:|
| 5 hops desde nodo 1 (graph 2k/6k) | **2.7 ms** | 90 rows alcanzadas |

**Veredicto**: fixpoint con dedup FNV-1a-64 es eficiente para grafos chicos-medianos. Guard 10K iteraciones protege runaway.

---

## 🔎 Hallazgos honestos de esta corrida

### Huecos del motor que el bench expuso (skip-graceful, no abortan)

| Hueco | Código | Query afectada | Bloque deferido |
|---|---|---|---|
| Agregados sobre `SELECT con JOIN` | `[GBY-4028]` | `SELECT COUNT(*) FROM t JOIN u ON ...` | F2 |
| `BETWEEN` sin índice ordenado | `[GBY-4002]` | `WHERE qty BETWEEN 1 AND 5` (lines) | F3 |
| `SELECT (subquery)` sin FROM | parser | scalar subquery bare-SELECT | E5 |

### Bugs del propio bench `gabybench` encontrados y fixeados durante esta sesión

| Bug | Fix |
|---|---|
| Warmup colisionaba con main loop en suites DML | offset `i + 1_000_000` para warmup |
| DML loops corrían sin tx activa | wrap `pager.begin()/commit()` alrededor |
| Queries citaban columna inexistente `id` en `lines` | corregido a `order_id` |
| `pager2 = open_for_bench(path)` con `pager` ya abierto → `[GBY-1002]` lock | reusar pager o `close()` antes de re-abrir |
| Doble `begin()` sobre pager con tx implícita → `[GBY-1005]` | usar `Pager::open` directo (sin tx implícita) |
| Constraint zoo `UNIQUE (code, region)` multi-col TEXT → `[GBY-4067]` | cambiar a UNIQUE single-col (K2 only all-INT) |
| `WITH RECURSIVE` sin FROM en anchor → `[GBY-4081]` | agregar tabla `seed_one` para el anchor |
| `COUNT(*) FROM view` → `[GBY-4028]` | cambiar a `SELECT * LIMIT 100` |

### Hallazgo crítico: window functions `RANK`/`SUM OVER` son cuadráticas

- **ROW_NUMBER()** OVER (PARTITION BY) → 247-347 ms / 500 rows ✅ aceptable
- **LAG()** OVER (PARTITION BY) → 158 ms / 500 rows ✅ aceptable
- **RANK()** OVER (PARTITION BY ORDER BY) → **27-46 SEGUNDOS / 500 rows** ⚠️ inviable productivamente
- **SUM() OVER (PARTITION BY)** cumulativo → **27-50 segundos / 500 rows** ⚠️ inviable

Este es el **hallazgo #1 del bench**. Hay un algoritmo cuadrático en `compute_window_value` para RANK y SUM-OVER. Defer **W4 — refactor window functions a O(n log n)**.

---

## 🎯 Cómo correr el bench vos mismo

```bash
# Limpia + setup de las 10 DBs + run completo
cargo run --release --bin gabybench -- all

# Solo setup (no corre queries)
cargo run --release --bin gabybench -- setup

# Solo run (re-usa DBs ya seteadas)
cargo run --release --bin gabybench -- run
```

Outputs:
- **stdout**: tabla por suite con p50/p95/p99/mean/rows
- **`bench/results.json`**: raw rows para post-procesar
- **`bench/dbs/*.db`**: las 10 DBs sintéticas regenerables
- **`docs/benchmarks/BENCHMARK-YYYY-MM-DD[_N].md`**: snapshot histórico inmutable (auto-generado al final)

---

## 📌 Resumen ejecutivo (corrida 2026-05-27 — Docker)

> Mantenido por compatibilidad histórica. Para los números actuales ver arriba.

**Fixes que más mueven la aguja** (sesión 2026-05-27, post bloques L+V):

| # | Issue | Antes | Después | Mejora |
|---|---|---:|---:|---:|
| #1 | Scalar subquery no-correlacionada re-evaluada por fila | 7.55 s (LIMIT 10) | **3.34 s** | ~2.3× — N→1 eval; el residual es el costo de la única evaluación, no del loop |
| #4 | Composite PK lookup que no usaba B+Tree | 145 ms | **216 µs** | **~670×** — fingerprint fast-path activado |
| #5 | `parse_agg_arg` rechazaba aritméticos | error de parse | **439 µs** | desbloqueado: `SUM(qty*price)` compila |
| #3 | `[GBY-4001]` rechazaba `WHERE col_no_idx = val` | error | **630 ms full scan** | semántica unificada con `>`, `<`, etc. |
| #6 | JOIN nested-loop puro sin hash | aplicado vía index-loop existente (no medible en bench actual) | — | implementado; bench no lo exhibe porque todos los JOINs del bench pegan al fast-path indexado |
| #2 | `CREATE INDEX` colapso con baja cardinalidad | unable-to-create | sigue limitado: mensaje de error claro + workarounds documentados | deferred (requiere overflow chain en bucket; ADR aparte) |

**Lo que anda bien post-fix**: PK lookup en **~240 µs**, indexed equality en **~350 µs**, UPDATE auto-commit (fsync por iter) en **~130 µs**, composite PK lookup en **~220 µs** (vs 145 ms pre-fix). El loop sobre N rows con un subquery no-correlacionado costaba O(N · scan); ahora es O(scan + N).

**Lo que sigue duele**: full scans sobre 200K rows (3.2–3.6 s en Docker), `INTERSECT` que duplica el costo de un full scan, JOIN sin push-down de WHERE al outer (el bench mide JOIN+BETWEEN en 1 s porque el BETWEEN se aplica POST-JOIN). El bucket overflow para índices secundarios de baja cardinalidad sigue pendiente.

**Veredicto operativo**: el motor está listo para OLTP de tamaño chico-medio + analítica liviana sobre tablas de hasta ~100K rows con PK + índice. Inadecuado todavía para queries con full-scan repetitivos sin planner que push-downee WHERE.
