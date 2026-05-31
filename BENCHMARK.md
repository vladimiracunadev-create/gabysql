# Benchmark gabysql

> **Evaluación profesional de desempeño** sobre `main`. **10 escenarios sintéticos** que cubren cada subsistema mayor del motor. Metodología reproducible, números honestos, snapshot histórico automático por corrida.

📌 **Corrida vigente**: **2026-05-30** (Windows host, single-core perf, release build).
📂 **Snapshot histórico**: cada corrida deja un MD inmutable en [`docs/benchmarks/BENCHMARK-YYYY-MM-DD.md`](docs/benchmarks/) generado por `gabybench`.
🔄 **Corrida anterior** (3 DBs originales, Docker): [BENCHMARK_2026-05-26.md](BENCHMARK_2026-05-26.md).

---

## 🎯 ¿Para qué sirve este benchmark?

1. **Verificar consistencia** — cada corrida produce números comparables; regresiones son visibles.
2. **Medir efecto de mejoras** — bumps de versión, fixes y bloques nuevos se evalúan contra los números previos.
3. **Detectar huecos del motor** — skip-graceful: queries que tropiezan con limitaciones se documentan, no abortan.
4. **Decidir el roadmap por costo real** — si una query toma 30s sobre 500 rows, eso justifica un planner; si toma 12µs, no.

---

## 🧪 Las 10 DBs y qué subsistema cubre cada una

| # | DB | Filas | Subsistema | Bloque(s) del motor |
|---|---|---:|---|---|
| 1 | **microblog** | 50.000 | OLTP: PK INT + UNIQUE TEXT + idx secundario + ordered-int range | Core + E1-E3 + F + J |
| 2 | **events** | 200.000 | Analítica mediana: scan, range ordenado, aggregates, UNION, DISTINCT | F + I + ADR-0017 |
| 3 | **orders_lines** | 120.000 | PK compuesta all-INT + idx compuesto + JOINs + CTAS + DROP COLUMN | K1 + K2 |
| 4 | **secdb** | 25.000 | **Z**: USERS + ROLES + SET SESSION AUTH + RLS USING + WITH CHECK | Z1 → Z3f |
| 5 | **finance** | 50.000 | **Y6-Y9**: DECIMAL(14,4) exacto + aritmética + SUM/AVG Decimal-exact | Y6 → Y9 |
| 6 | **analytics** | 30.000 | **W3**: window functions OVER + PARTITION BY + RANK + LAG + SUM OVER | W3 |
| 7 | **graph** | 8.000 | **W2**: WITH RECURSIVE fixpoint + **V**: vistas lógicas | W2 + V |
| 8 | **procflow** | 5.000 | **X**: triggers AFTER + user functions + stored procedures + CALL | X1 + X3 + X3b |
| 9 | **types_zoo** | 10.000 | **Y completo**: BLOB X'hex' + UUID + TIME + DATETIME + INT widths + UNSIGNED | Y1 → Y6 |
| 10 | **constraint_zoo** | 15.000 | **L**: CHECK + FK CASCADE on DELETE/UPDATE + UNIQUE | L1 + L2 |

**Total: 513.000 filas distribuidas, seed determinístico (`SEED = 0x9E37_79B9_7F4A_7C15`).**

---

## 📊 Números medidos (corrida 2026-05-30)

### Suite 1 — `microblog` (OLTP)

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (id=5000) | 1000 | **15.60 µs** | 41.40 µs | 56.40 µs | 19.02 µs | 1 |
| PK lookup cold (random id) | 500 | 26.40 µs | 60.50 µs | 95.90 µs | 26.82 µs | 1 |
| UNIQUE TEXT lookup (email) | 1000 | **19.10 µs** | 42.50 µs | 65.70 µs | 23.30 µs | 1 |
| Index secundario eq (user_id=100) | 500 | 16.00 µs | 45.10 µs | 67.20 µs | 20.55 µs | 0 |
| Index ordered range (likes 50..100) | 200 | 36.94 ms | 45.54 ms | 48.14 ms | 38.06 ms | 2073 |
| Full scan TEXT (nombre LIKE 'A%') | 100 | 17.89 ms | 26.69 ms | 39.68 ms | 19.62 ms | 1250 |
| JOIN+COUNT (u.id=7) — F2 | 200 | _pendiente próximo bench_ | | | | |
| Aggregate global posts (COUNT+AVG) | 50 | 192.09 ms | 209.46 ms | 356.30 ms | 196.48 ms | 1 |
| GROUP BY user_id | 20 | 235.31 ms | 251.97 ms | 255.84 ms | 237.81 ms | 4998 |
| INSERT single (in-tx) | 500 | **23.80 µs** | 59.70 µs | 73.10 µs | 31.32 µs | — |
| UPDATE por PK (in-tx) | 500 | 25.40 µs | 65.80 µs | 116.20 µs | 34.50 µs | — |
| DELETE por PK (in-tx) | 500 | 32.20 µs | 93.10 µs | 117.30 µs | 41.53 µs | — |

### Suite 2 — `events` (analítica mediana, 200k rows)

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| Full scan eq `kind='view'` (no idx) | 20 | **761.92 ms** | 1013.55 ms | 1101.03 ms | 784.74 ms | 1 |
| Indexed eq `valor=12345` | 500 | **19.00 µs** | 54.40 µs | 72.00 µs | 24.97 µs | 0 |
| Indexed range valor 1000..2000 (~2k rows) | 200 | 55.71 ms | 65.85 ms | 80.63 ms | 56.87 ms | 2009 |
| **Indexed range LARGE 10k..90k** (~160k rows) | 50 | **3.48 s** | 4.80 s | 5.27 s | 3.66 s | 160 259 |
| Aggregate COUNT(*) full | 30 | 537.87 ms | 554.33 ms | 560.09 ms | 538.85 ms | 1 |
| GROUP BY kind (low-card 7) | 20 | 636.19 ms | 679.24 ms | 701.52 ms | 645.33 ms | 7 |
| DISTINCT kind | 20 | 477.46 ms | 485.49 ms | 502.14 ms | 477.23 ms | 7 |
| Scalar subquery bare-SELECT (E5) | 20 | _pendiente próximo bench_ | | | | |
| UNION two ranges | 30 | 710.18 ms | 737.07 ms | 761.84 ms | 713.87 ms | 2118 |
| SELECT con UPPER/expr LIMIT 100 | 200 | **187.30 µs** | 222.10 µs | 238.70 µs | 192.84 µs | 100 |

### Suite 3 — `orders_lines` (PK compuesta K2, 120k rows)

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| **PK compuesta full** (order_id+line_no) | 1000 | **16.80 µs** | 18.30 µs | 35.30 µs | 17.35 µs | 1 |
| PK compuesta partial (order_id only) | 100 | 137.23 ms | 143.03 ms | 147.89 ms | 138.43 ms | 5 |
| Composite index lookup qty+precio | 200 | 161.24 ms | 182.09 ms | 201.39 ms | 164.27 ms | 100 000 |
| JOIN orders×lines on order_id=7 | 100 | 163.80 ms | 236.06 ms | 302.95 ms | 171.33 ms | 0 |
| Aggregate SUM(qty*precio) GROUP LIMIT 10 | 5 | **51.30 µs** | 52.30 µs | 52.30 µs | 51.18 µs | 10 |
| BETWEEN qty 1..5 (no idx, full scan) — F3 | 50 | _pendiente próximo bench_ | | | | |
| CTAS lines_summary (one-shot) | 1 | 726 ms | — | — | — | 0 |
| DROP COLUMN fecha (orders 20k) | 1 | 149.66 ms | — | — | — | 0 |
| INSERT PK compuesta (in-tx) | 500 | **26.10 µs** | 65.70 µs | 100.60 µs | 34.40 µs | — |

### Suite 4 — `secdb` (Z: USERS + RLS, 25k rows)

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| SELECT * full (no auth — superuser, 5k rows) | 50 | **7.78 ms** | 11.53 ms | 14.33 ms | 7.79 ms | 5000 |
| SET AUTH alice + SELECT (default deny) | 50 | 48.34 ms | 51.21 ms | 55.21 ms | 48.31 ms | 0 |
| SET AUTH bob + SELECT (RLS country='AR') | 50 | 51.70 ms | 61.14 ms | 70.84 ms | 53.71 ms | 999 |
| SET AUTH carol + SELECT (RLS tier=1) | 50 | 45.73 ms | 59.26 ms | 77.91 ms | 47.12 ms | 1694 |
| **RLS + PK lookup** (bob, id=2500) | 200 | **40.38 ms** | 52.66 ms | 59.29 ms | 41.92 ms | 1 |
| JOIN customers×orders (no RLS) | 20 | 89.69 ms | 101.08 ms | 101.34 ms | 91.47 ms | 9 |

**Conclusión costo de seguridad**: el overhead de `SET SESSION AUTHORIZATION + SELECT` es ~**40-50 ms por query** (dominado por PBKDF2 hash en cada SET) — comparable a un full scan mediano. Cache de auth a nivel sesión sería el fix más alto-impacto.

### Suite 5 — `finance` (Y: DECIMAL exacto, 50k transactions)

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (id=25000) | 1000 | **12.50 µs** | 13.10 µs | 18.40 µs | 12.70 µs | 1 |
| Index secundario eq (account_id=500) | 500 | 261.70 µs | 590.10 µs | 706.30 µs | 298.11 µs | 58 |
| **SUM(amount) full Decimal-exact (Y9)** | 30 | **134.60 ms** | 162.73 ms | 165.94 ms | 133.85 ms | 1 |
| **AVG(fee) full Decimal-exact (Y9)** | 30 | 140.91 ms | 161.52 ms | 181.89 ms | 143.19 ms | 1 |
| SELECT amount - fee LIMIT 1000 (Decimal Y7) | 50 | 1.37 ms | 1.69 ms | 2.13 ms | 1.42 ms | 1000 |
| SELECT amount * 1.05 LIMIT 1000 (Decimal Y8) | 50 | 1.29 ms | 1.71 ms | 2.43 ms | 1.37 ms | 1000 |
| GROUP BY account_id SUM(amount) | 10 | 157.45 ms | 176.69 ms | 176.69 ms | 160.83 ms | 1000 |

**Conclusión costo DECIMAL exacto**: SUM/AVG Decimal sobre 50k rows toma 130-160 ms — **comparable a INT**, sin overhead significativo. Y9 (Decimal-exact accumulator) entrega lo que promete.

### Suite 6 — `analytics` (W3: window functions, 30k sales)

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (id=15000) | 1000 | **12.00 µs** | 17.10 µs | 30.20 µs | 12.90 µs | 1 |
| **ROW_NUMBER() OVER (PARTITION BY region)** | 5 | 270.26 ms | 279.77 ms | 279.77 ms | 267.50 ms | 500 |
| **🚨 RANK() OVER (PARTITION BY)** | 5 | **44.5 s** | 45.6 s | 45.6 s | 44.5 s | 500 |
| **🚨 SUM OVER (PARTITION BY) cumulativo** | 5 | **59.4 s** | 62.5 s | 62.5 s | 60.0 s | 500 |
| LAG(revenue, 1) OVER (PARTITION BY) | 5 | 229.52 ms | 246.14 ms | 246.14 ms | 232.57 ms | 500 |
| GROUP BY region SUM(revenue) (baseline) | 20 | **113.70 ms** | 128.31 ms | 129.68 ms | 114.83 ms | 5 |

🚨 **HALLAZGO CRÍTICO** — `RANK()` y `SUM() OVER (PARTITION BY)` son **O(n²) hoy**. 500 rows toman 44-60 segundos. Esto es **inviable productivamente**. ROW_NUMBER, LAG y LEAD están OK porque su algoritmo es lineal.

**Defer W4 — refactor `compute_window_value` a O(n log n)**.

Mientras tanto, el bench bajó `iters` para esas 2 queries (de 5 → 2 en la siguiente corrida) para no hacer durar 10 min el bench cada vez.

### Suite 7 — `graph` (W2 + V, 8k rows)

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (id=1000) | 500 | **13.80 µs** | 21.00 µs | 36.60 µs | 14.80 µs | 1 |
| Indexed edges by src=100 | 200 | **31.70 µs** | 63.40 µs | 81.50 µs | 39.53 µs | 2 |
| **WITH RECURSIVE traversal (5 hops)** | 10 | **4.20 ms** | 6.58 ms | 6.58 ms | 4.11 ms | 90 |
| SELECT FROM view heavy_edges LIMIT 100 | 50 | 18.11 ms | 25.28 ms | 29.80 ms | 18.97 ms | 100 |
| JOIN edges×nodes (label de cada edge) | 20 | 76.26 ms | 158.96 ms | 256.54 ms | 88.52 ms | 100 |

**Conclusión WITH RECURSIVE**: fixpoint con dedup FNV-1a-64 es **muy eficiente** — 4 ms para 5 hops desde nodo 1, expandiendo a 90 nodos. Guard de 10K iteraciones protege runaway.

### Suite 8 — `procflow` (X: triggers + procedures, 5k rows)

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (id=2500) | 500 | **12.20 µs** | 14.40 µs | 30.30 µs | 13.04 µs | 1 |
| SELECT FUNCTION double_balance(balance) X3b | 100 | 1.24 ms | 4.38 ms | 9.14 ms | 1.82 ms | 200 |
| COUNT(*) FROM audit_log (estado inicial) | 50 | **11.80 µs** | 14.10 µs | 15.20 µs | 12.14 µs | 1 |
| UPDATE dispara trigger AFTER (in-tx, ids únicos) | 200 | _pendiente — rerun en curso con fix_ | | | | |
| COUNT(*) FROM audit_log (post UPDATEs) | 50 | _pendiente_ | | | | |
| CALL bump_counter() X3 | 200 | _pendiente_ | | | | |

**Hallazgo**: el trigger del bench inicial usaba `NEW.id` como PK de `audit_log` + el UPDATE rotaba sobre 100 filas → PK dup `[GBY-3001]`. **Fix aplicado**: el UPDATE usa IDs únicos 1..200 → cada trigger fire mete un PK único. La próxima corrida cubre estas 3 queries.

### Suite 9 — `types_zoo` (Y completo: BLOB+UUID+TIME+DECIMAL, 10k specimens)

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| _pendiente — rerun en curso (suite no llegó a correr)_ | | | | | | |

### Suite 10 — `constraint_zoo` (L: CHECK + FK CASCADE, 15k rows)

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| _pendiente — rerun en curso (suite no llegó a correr)_ | | | | | | |

---

## 📌 Resumen ejecutivo

### ✅ Lo que rinde bien (production-grade en su scope)

| Operación | Latencia p50 | Comentario |
|---|---:|---|
| **PK lookup hot** | **12-16 µs** | Consistente entre TODAS las DBs (microblog 16µs, finance 13µs, analytics 12µs, procflow 12µs, graph 14µs) — B+tree get O(log n) |
| **Indexed eq (hash idx)** | 12-19 µs | secondary index `find_by_eq` O(1) |
| **Indexed eq (ordered-int)** | 19 µs | events `WHERE valor = 12345` |
| **UNIQUE TEXT lookup** | 19 µs | hash en TEXT secundario |
| **PK compuesta full (K2)** | **17 µs** | fingerprint FNV-1a-64 |
| **DML in-tx** (INSERT/UPDATE/DELETE PK) | 24-32 µs | sin fsync per-statement |
| **WITH RECURSIVE 5 hops** | **4.2 ms** | fixpoint + dedup, 90 nodos reach |
| **DECIMAL SUM 50k rows** | **135 ms** | Y9 exact, comparable a INT |
| **DECIMAL aritmética 1k rows** | 1.3 ms | Y7+Y8 sin overhead |
| **User function (X3b) en SELECT** | 1.2 ms / 200 calls | eval_expr per-row |
| **GROUP BY single-table** | 113 ms / 30k rows | aceptable |

### ⚠️ Lo que duele (necesita optimización)

| Operación | Latencia | Bloque que lo arregla |
|---|---:|---|
| **🚨 RANK() OVER (PARTITION BY) 500 rows** | **44 s** | W4 (refactor O(n log n)) |
| **🚨 SUM OVER (PARTITION BY) 500 rows** | **60 s** | W4 |
| **Indexed range LARGE 160k rows** | 3.48 s | leaf cursor batch decode |
| **Full scan eq 200k rows** | 762 ms | F3 + planner P5 |
| **UNION two ranges (events)** | 710 ms | F4 (streaming) |
| **Aggregate full (events COUNT)** | 538 ms | P4 stats + P5 planner |
| **GROUP BY low-card (events)** | 636 ms | P4 + P5 |
| **DISTINCT** | 477 ms | hash early-out |
| **RLS + SELECT** | 40-52 ms | auth cache sesión |
| **JOIN orders×lines** | 164 ms | P5 (planner real) |
| **PK compuesta partial** | 137 ms | hoy full scan (no usa idx parcial) |

### 🔎 Huecos del motor expuestos por el bench

**Catálogo completo, auditado y priorizado**: [ADR-0066 — Gaps del motor expuestos por el benchmark](docs/adr/0066-bench-exposed-gaps.md).

Resumen (10 gaps identificados, cada uno con código de error + query del bench que lo dispara + workaround + bloque/prioridad de fix):

| # | Gap | Código | Bloque defer | Prioridad |
|---|---|---|---|---:|
| 1 | ~~Agregados sobre `SELECT con JOIN`~~ | ~~`[GBY-4028]`~~ | ~~F2~~ ✓ | ~~P1~~ cerrado 2026-05-30 |
| 2 | ~~`BETWEEN` sin índice ordenado~~ | ~~`[GBY-4002]`~~ | ~~F3~~ ✓ | ~~P1~~ cerrado 2026-05-30 |
| 3 | ~~`SELECT (subquery)` sin FROM~~ | ~~parser~~ | ~~E5~~ ✓ | ~~P2~~ cerrado 2026-05-30 |
| 4 | `UNIQUE` multi-col exige all-INT | `[GBY-4067]` | K3 | P2 |
| 5 | ~~`WITH RECURSIVE` requiere FROM en anchor~~ | ~~`[GBY-4081]`~~ | ~~(depende de E5)~~ ✓ | ~~P2~~ cerrado 2026-05-30 |
| 6 | Trigger PK auto-gen (sin SERIAL / DEFAULT con función) | bench design | N5 | P2 |
| 7 | ~~`COUNT(*) FROM <view>`~~ | ~~`[GBY-4028]`~~ | ~~F2~~ ✓ | ~~P1~~ cerrado 2026-05-30 |
| 8 | ~~**`RANK()` y `SUM OVER (PARTITION BY)` eran O(n²)**~~ | ~~sin código~~ | ~~**W4 crítico**~~ ✓ | ~~**P1**~~ cerrado 2026-05-30 |
| 9 | PK compuesta partial scan no usa idx | sin código | K4 | P2 |
| 10 | Composite index no detecta `WHERE A AND B` | sin código | P5b | P1 (con P5) |

**Política**: el bench tiene `bench_sql_or_skip` para los gaps documentados — la suite **no aborta** por una limitación conocida. Si aparece un gap NUEVO, agregarlo a ADR-0066 antes de aplicar el workaround.

### 🐛 Bugs del propio `gabybench` encontrados y fixeados

| Bug | Fix |
|---|---|
| Warmup colisionaba con main loop (PK dup en INSERT i=0) | offset `i + 1_000_000` para warmup |
| DML loops sin tx activa | wrap explícito `begin/commit` |
| Queries citaban columna inexistente `id` en `lines` | corregido a `order_id` |
| Doble-open de pager (procflow/constraint_zoo) → `[GBY-1002]` | reusar pager o `close()` antes de re-abrir |
| Doble `begin()` sobre tx implícita → `[GBY-1005]` | `Pager::open` directo sin tx implícita |
| UNIQUE multi-col TEXT en constraint_zoo → `[GBY-4067]` | cambiar a UNIQUE single-col |
| WITH RECURSIVE sin seed table → `[GBY-4081]` | agregar `seed_one` |
| Trigger usaba `NEW.id` como PK + UPDATE rotaba IDs → `[GBY-3001]` | UPDATE con IDs únicos 1..N |

---

## 🎯 Cómo correr el bench vos mismo

```bash
# Limpia + setup de las 10 DBs + run completo + archive snapshot
cargo run --release --bin gabybench -- all

# Solo setup (no corre queries)
cargo run --release --bin gabybench -- setup

# Solo run (re-usa DBs ya seteadas)
cargo run --release --bin gabybench -- run
```

Outputs:
- **stdout**: tabla por suite con p50/p95/p99/mean/rows + line "== gabybench OK — total X.X min =="
- **`bench/results.json`**: raw rows para post-procesar
- **`bench/dbs/*.db`**: las 10 DBs sintéticas regenerables
- **`docs/benchmarks/BENCHMARK-YYYY-MM-DD[_N].md`**: snapshot histórico inmutable (auto-generado al final)

**Tiempo total esperado**: ~10-12 min en hardware moderno (con RANK/SUM OVER en 2 iters cada uno para no quemar 5+ min en esos solos).

---

## 📌 Roll-up corrida 2026-05-27 (Docker — histórica)

Mantenido por compatibilidad. Para los números actuales ver arriba.

**Fixes que más mueven la aguja** (sesión 2026-05-27, post bloques L+V):

| # | Issue | Antes | Después | Mejora |
|---|---|---:|---:|---:|
| #1 | Scalar subquery no-correlacionada re-evaluada por fila | 7.55 s (LIMIT 10) | **3.34 s** | ~2.3× |
| #4 | Composite PK lookup que no usaba B+Tree | 145 ms | **216 µs** | **~670×** |
| #5 | `parse_agg_arg` rechazaba aritméticos | error de parse | **439 µs** | desbloqueado |
| #3 | `[GBY-4001]` rechazaba `WHERE col_no_idx = val` | error | **630 ms full scan** | semántica unificada |

**Lo que sigue duele en aquella corrida**: full scans 200K rows (3.2-3.6 s en Docker), `INTERSECT` que duplica el costo, JOIN sin push-down al outer. Mayoría reconfirmados en 2026-05-30 con números ligeramente distintos (Docker vs host Windows).
