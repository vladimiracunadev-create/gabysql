# ADR-0063: `EXPLAIN <statement>` — descripción del plan de ejecución (P1)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: P1 (primer sub-bloque de **Fase 3** — Planeación y rendimiento)
**Bump on-disk**: **ninguno** (read-only sobre catálogo + AST)

## 🧭 Contexto

Cerramos la Fase 2 (W/X/Y/Z) con el subsistema de seguridad maduro. La **Fase 3** del roadmap pide planner básico, `EXPLAIN`, benchmarks reproducibles, profiling y comparativos contra SQLite/PostgreSQL/MySQL/DuckDB.

`EXPLAIN` es el sub-bloque natural para arrancar porque:
1. Es discreto y testeable.
2. Es prerrequisito del planner-as-optimizer (P2/P3) — no podés validar mejoras del plan sin poder leerlo.
3. Da valor inmediato al usuario: ahora puede confirmar **cuál fast-path va a usar el engine** (PK lookup vs full scan vs index range) sin leer el código.

`EXPLAIN ANALYZE` (timings + row counts reales) se difere a **P2** porque requiere instrumentación del exec loop.

Bloque nombrado **P1** para cortar limpiamente con la cadena Z. P = Performance/Planeación.

## 💡 Decisión

### 1. AST + Parser

```rust
Statement::Explain(Box<Statement>)
```

Parser: `EXPLAIN <stmt>` recursivo — wrappea cualquier statement válido. `EXPLAIN ANALYZE` rechazado con `[GBY-4139]` UNSUPPORTED_SYNTAX y mensaje informativo que apunta a P2.

### 2. Engine: walk AST + clasificación de scan

`exec_explain(inner)` despacha por tipo de statement y emite una `Vec<(step, detail)>`:

```
SELECT: explain_select_query
  └─ classify_scan(table, where)
      ├─ Sin WHERE → "full scan"
      ├─ WHERE col=val con col=PK → "PK lookup (B+tree get, ~O(log n))"
      ├─ WHERE col=val con índice hash → "hash-index equality (bucket lookup, ~O(1))"
      ├─ WHERE col=val con índice ordered → "ordered-int index equality"
      ├─ WHERE col BETWEEN a AND b con índice ordered → "ordered-int index BETWEEN range"
      ├─ WHERE comparison → "full scan + WHERE post-filter"
      └─ AND/OR/NOT compound → "full scan + WHERE post-filter (predicate complejo)"
  ├─ Joins (kind, target, predicate type) — siempre nested-loop hoy
  ├─ GROUP BY (hash-group)
  ├─ ORDER BY (in-memory sort)
  ├─ LIMIT/OFFSET
  └─ DISTINCT (hash-dedup)
```

Para INSERT/UPDATE/DELETE, agrega target + tipo de scan + RETURNING info si aplica.

ResultSet output:
```
columns = ["step", "detail"]
rows = [
    ["1", "SCAN `t` → PK lookup `id` (B+tree get, ~O(log n))"],
    ["2", "ORDER BY `n` Asc (in-memory sort)"],
    ["3", "LIMIT Some(10) OFFSET 5"],
]
message = "EXPLAIN: plan estimado (sin ejecutar el statement)"
```

### 3. Clasificación de scan es **honesta sobre lo que el engine hace hoy**

La lógica de `classify_scan` espeja exactamente las fast-path checks que hacen `exec_select`, `exec_update`, `exec_delete`. No inventa optimizaciones que no existen. Si el output dice "PK lookup", el engine va a usar PK lookup. Si dice "full scan + WHERE post-filter", eso es lo que va a ejecutar.

Esto es el contrato implícito de EXPLAIN: si miente, no sirve.

### 4. EXPLAIN no ejecuta el statement subyacente

Garantía testeada: `p1_explain_does_not_execute_statement` — `EXPLAIN INSERT INTO t ...` no persiste nada en `t`.

## 📁 Archivos tocados

- `src/sql.rs`:
  - `Statement::Explain(Box<Statement>)` nuevo variant.
  - Parser: dispatch `EXPLAIN` al inicio de `parse_statement` (rechaza `ANALYZE` con `[GBY-4139]`).
  - Engine: `exec_explain`, `explain_select_query`, `explain_select`, `classify_scan`, `find_index_kind`, `explain_dml_target` (~250 LOC).
- `src/errors.rs`: código `UNSUPPORTED_SYNTAX = 4139`.
- `tests/integration_test.rs`: 10 tests `p1_*`.

## ⛔ Lo que **no** entra en P1 (defer P2/P3)

| Ítem | Razón del defer |
|---|---|
| **`EXPLAIN ANALYZE`** con timings + row counts reales | Requiere instrumentación del exec loop con timers + counters. P2 dedicado. |
| **Cost estimates** (row count, page reads, total cost) | Requiere stats del catálogo (histograms, cardinalities). Necesita planner-as-optimizer — P3. |
| **Plan choice** (planner que elige entre alternativas) | Hoy el engine tiene fast-paths estáticos. P3 = planner que reordena joins, elige índices, etc. |
| **Plan tree** (formato jerárquico estilo PG `->`) | Output actual es lista plana con step IDs. Si los planes se vuelven más complejos, P2 introduce tree printer. |
| **Output formats** (`FORMAT JSON`, `FORMAT TEXT`) | Hoy texto plano en una `ResultSet`. JSON parseable es útil para tooling — defer. |
| **`EXPLAIN` sobre statements procedurales** (IF, WHILE, BEGIN..END) | Hoy se imprimen como "statement DDL/control — no es plan-able". Para esos, EXPLAIN tendría que recursar por el body. |
| Sub-plan recursión completa para SELECTs anidados en INSERT/UPDATE/etc. | INSERT...SELECT muestra "ver sub-plan" + base_step=100. Funciona pero no es perfecto para casos profundamente anidados. |

## 🧪 Tests

10 tests `p1_*`:
- Full scan sin WHERE.
- PK lookup con `WHERE pk = val`.
- Hash/ordered index lookup con `WHERE indexed_col = val`.
- JOIN + ORDER BY + WHERE compuesto.
- INSERT VALUES (multi-row).
- UPDATE con PK lookup en WHERE.
- DELETE con PK lookup.
- Dry-run: `EXPLAIN INSERT` no persiste.
- `EXPLAIN ANALYZE` rebota limpio con `[GBY-4139]`.
- DISTINCT + ORDER BY + LIMIT + OFFSET acumulados.

Suite total: **708 passing + 1 ignored** (698 → +10 P1).

## 🔗 Referencias

- PostgreSQL `EXPLAIN` docs §14.1 — similar shape de output, sin costs.
- SQLite `EXPLAIN QUERY PLAN` — más parecido al estilo P1 (text descriptions, sin tree).
- Roadmap Fase 3.
