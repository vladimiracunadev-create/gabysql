# Benchmark snapshot — 2026-05-29

> **Snapshot inmutable** generado automáticamente por `gabybench` al final de cada corrida.
> No editar a mano. Vivo en `docs/benchmarks/` para comparación cross-commit.
> El roll-up vivo + lectura ejecutiva están en [BENCHMARK.md](../../BENCHMARK.md).

---

**Resumen**: 10 suites · 71 queries medidas · 0 filas con SKIP

## suite: `analytics`

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (id=15000) | 1000 | 13.20 µs | 39.60 µs | 73.70 µs | 18.90 µs | 1 |
| ROW_NUMBER() OVER (PARTITION BY region ORDER BY revenue DESC) | 5 | 367.07 ms | 382.26 ms | 382.26 ms | 369.96 ms | 500 |
| RANK() OVER (PARTITION BY region ORDER BY revenue DESC) (W4) | 5 | 380.47 ms | 388.01 ms | 388.01 ms | 378.36 ms | 500 |
| SUM OVER (PARTITION BY region) cumulative (W4) | 5 | 233.76 ms | 300.19 ms | 300.19 ms | 247.43 ms | 500 |
| LAG(revenue, 1) OVER (PARTITION BY region ORDER BY id) | 5 | 226.57 ms | 229.28 ms | 229.28 ms | 227.02 ms | 500 |
| GROUP BY region SUM(revenue) (baseline vs OVER) | 20 | 110.69 ms | 117.48 ms | 118.34 ms | 111.47 ms | 5 |

## suite: `constraint_zoo`

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot parent (id=2500) | 500 | 15.00 µs | 31.70 µs | 59.10 µs | 17.59 µs | 1 |
| Lookup por UNIQUE multi-col (code+region) | 200 | 25.60 µs | 97.10 µs | 118.80 µs | 36.62 µs | 1 |
| INSERT con CHECK + FK validation (in-tx) | 200 | 48.90 µs | 116.50 µs | 157.50 µs | 61.29 µs | 0 |
| JOIN parent×child con WHERE region=AR | 20 | 60.08 ms | 65.30 ms | 67.74 ms | 59.96 ms | 200 |

## suite: `events`

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| Full scan eq kind='view' (no idx) | 20 | 652.21 ms | 698.18 ms | 706.82 ms | 654.28 ms | 1 |
| Indexed eq valor=12345 | 500 | 14.70 µs | 34.70 µs | 40.10 µs | 16.94 µs | 0 |
| Indexed range valor 1000..2000 | 200 | 50.62 ms | 56.17 ms | 63.38 ms | 51.42 ms | 2009 |
| Indexed range large 10k..90k | 50 | 4055.24 ms | 4962.87 ms | 5361.54 ms | 4114.41 ms | 160259 |
| Aggregate COUNT(*) full | 30 | 681.40 ms | 778.31 ms | 816.32 ms | 699.20 ms | 1 |
| GROUP BY kind (low-card) | 20 | 786.38 ms | 829.07 ms | 1081.69 ms | 798.38 ms | 7 |
| DISTINCT kind | 20 | 678.78 ms | 796.92 ms | 940.08 ms | 701.28 ms | 7 |
| Subquery escalar COUNT(view) bare-SELECT (E5) | 20 | 838.01 ms | 963.42 ms | 1111.51 ms | 845.28 ms | 1 |
| UNION two valor ranges | 30 | 978.60 ms | 1011.93 ms | 1054.15 ms | 980.86 ms | 2118 |
| SELECT con Expr (UPPER, *2) LIMIT 100 | 200 | 205.30 µs | 705.50 µs | 907.90 µs | 289.48 µs | 100 |

## suite: `finance`

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (id=25000) | 1000 | 13.80 µs | 47.10 µs | 75.50 µs | 19.45 µs | 1 |
| Index secundario eq (account_id=500) | 500 | 273.10 µs | 698.80 µs | 969.40 µs | 341.65 µs | 58 |
| SUM(amount) full (Decimal-exact Y9) | 30 | 167.98 ms | 178.14 ms | 187.96 ms | 169.32 ms | 1 |
| AVG(fee) full (Decimal-exact Y9) | 30 | 185.69 ms | 240.35 ms | 246.75 ms | 198.05 ms | 1 |
| SELECT amount - fee LIMIT 1000 (Decimal sub Y7) | 50 | 1902.30 µs | 4956.10 µs | 5480.70 µs | 2523.17 µs | 1000 |
| SELECT amount * 1.05 LIMIT 1000 (Decimal mul Y8) | 50 | 2540.50 µs | 5700.30 µs | 6244.50 µs | 3198.07 µs | 1000 |
| GROUP BY account_id SUM(amount) | 10 | 216.34 ms | 250.00 ms | 250.00 ms | 220.55 ms | 1000 |

## suite: `graph`

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (id=1000) | 500 | 12.70 µs | 34.50 µs | 49.90 µs | 16.16 µs | 1 |
| Indexed edges by src=100 | 200 | 36.90 µs | 78.50 µs | 101.60 µs | 42.45 µs | 2 |
| WITH RECURSIVE traversal (5 hops desde 1) | 10 | 3194.30 µs | 5645.50 µs | 5645.50 µs | 3666.59 µs | 90 |
| SELECT FROM view heavy_edges LIMIT 100 (V expansión) | 50 | 16.83 ms | 18.68 ms | 22.40 ms | 17.05 ms | 100 |
| COUNT(*) FROM heavy_edges (view + F2) | 20 | 17.96 ms | 19.56 ms | 20.33 ms | 18.17 ms | 1 |
| JOIN edges×nodes (label de cada edge) | 20 | 74.41 ms | 78.95 ms | 82.14 ms | 74.50 ms | 100 |

## suite: `microblog`

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (id=5000) | 1000 | 13.10 µs | 53.80 µs | 70.70 µs | 18.17 µs | 1 |
| PK lookup cold (random id) | 500 | 19.30 µs | 59.40 µs | 83.40 µs | 25.59 µs | 1 |
| UNIQUE TEXT lookup (email) | 1000 | 22.70 µs | 78.40 µs | 108.70 µs | 34.19 µs | 1 |
| Index secundario eq (user_id=100) | 500 | 12.10 µs | 13.50 µs | 30.10 µs | 12.73 µs | 0 |
| Index ordered range (likes 50..100) | 200 | 29.69 ms | 35.09 ms | 38.96 ms | 30.34 ms | 2073 |
| Full scan TEXT (nombre LIKE A%) | 100 | 12.71 ms | 13.34 ms | 14.42 ms | 12.82 ms | 1250 |
| JOIN+COUNT (u.id=7) (F2) | 200 | 973.38 ms | 1139.36 ms | 1218.71 ms | 978.01 ms | 1 |
| Aggregate global posts | 50 | 171.05 ms | 208.50 ms | 213.11 ms | 173.22 ms | 1 |
| GROUP BY user_id | 20 | 190.70 ms | 253.67 ms | 286.86 ms | 202.20 ms | 4998 |
| INSERT single (in-tx) | 500 | 26.10 µs | 70.10 µs | 98.60 µs | 34.86 µs | 0 |
| UPDATE por PK (in-tx) | 500 | 36.70 µs | 89.70 µs | 120.50 µs | 44.25 µs | 0 |
| DELETE por PK (in-tx) | 500 | 38.40 µs | 135.10 µs | 196.10 µs | 54.69 µs | 0 |

## suite: `orders_lines`

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK compuesta full (order_id+line_no) | 1000 | 17.60 µs | 51.90 µs | 86.50 µs | 22.91 µs | 1 |
| PK compuesta partial (order_id only) | 100 | 54.30 µs | 138.80 µs | 175.90 µs | 67.18 µs | 5 |
| Composite index lookup qty+precio | 200 | 16.50 µs | 39.80 µs | 60.70 µs | 20.50 µs | 0 |
| JOIN orders×lines on order_id=7 | 100 | 165.26 ms | 187.60 ms | 214.99 ms | 169.02 ms | 0 |
| Aggregate SUM(qty*precio) GROUP | 5 | 54.80 µs | 54.90 µs | 54.90 µs | 54.46 µs | 10 |
| BETWEEN qty 1..5 (no idx, full scan) | 50 | 230.69 ms | 241.21 ms | 255.40 ms | 232.30 ms | 24933 |
| CTAS lines_summary (one-shot) | 1 | 780.87 ms | 780.87 ms | 780.87 ms | 780.87 ms | 0 |
| DROP COLUMN fecha (orders 20k) | 1 | 154.61 ms | 154.61 ms | 154.61 ms | 154.61 ms | 0 |
| INSERT PK compuesta (in-tx) | 500 | 50.00 µs | 121.90 µs | 183.60 µs | 61.87 µs | 0 |

## suite: `procflow`

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (id=2500) | 500 | 12.90 µs | 34.60 µs | 52.90 µs | 16.79 µs | 1 |
| SELECT con FUNCTION double_balance(balance) X3b | 100 | 928.70 µs | 1828.70 µs | 2117.00 µs | 1077.62 µs | 200 |
| COUNT(*) FROM audit_log (estado inicial) | 50 | 8900 ns | 26.70 µs | 32.70 µs | 11.58 µs | 1 |
| UPDATE dispara trigger AFTER (in-tx, ids únicos) | 200 | 83.00 µs | 240.00 µs | 297.30 µs | 114.50 µs | 0 |
| CALL bump_counter() X3 (in-tx) | 200 | 23.00 µs | 64.90 µs | 77.10 µs | 31.26 µs | 0 |

## suite: `secdb`

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| SELECT * full (no auth — superuser) | 50 | 7861.60 µs | 10.44 ms | 11.10 ms | 7881.82 µs | 5000 |
| SET AUTH alice + SELECT (default deny) | 50 | 49.87 ms | 50.84 ms | 54.43 ms | 50.04 ms | 0 |
| SET AUTH bob + SELECT (RLS country=AR) | 50 | 52.95 ms | 57.97 ms | 70.62 ms | 53.75 ms | 999 |
| SET AUTH carol + SELECT (RLS tier=1) | 50 | 53.07 ms | 56.60 ms | 59.50 ms | 53.50 ms | 1694 |
| RLS + PK lookup (bob, id=2500) | 200 | 50.08 ms | 56.26 ms | 66.76 ms | 51.17 ms | 1 |
| JOIN customers×orders (no RLS, superuser) | 20 | 115.54 ms | 117.21 ms | 117.78 ms | 115.27 ms | 9 |

## suite: `types_zoo`

| Query | N | p50 | p95 | p99 | mean | rows |
|---|---:|---:|---:|---:|---:|---:|
| PK lookup hot (full row con BLOB+UUID+TIME) | 500 | 33.90 µs | 72.60 µs | 91.00 µs | 35.32 µs | 1 |
| SELECT solo INT widths (proyección) | 100 | 3147.90 µs | 5771.90 µs | 6345.00 µs | 3759.49 µs | 1000 |
| SELECT solo DECIMAL price + arithmetic | 100 | 3343.00 µs | 5867.10 µs | 6233.90 µs | 3879.57 µs | 1000 |
| SELECT solo TEXT/UUID/TIME (todos textuales) | 100 | 3391.90 µs | 6162.00 µs | 6541.90 µs | 3941.33 µs | 1000 |
| SELECT solo BLOB (overhead u32 + raw bytes) | 100 | 3083.10 µs | 5843.70 µs | 6381.80 µs | 3654.45 µs | 1000 |
| WHERE price > 5000 (Decimal compare Y7) | 20 | 31.54 ms | 32.42 ms | 32.62 ms | 31.36 ms | 1 |

