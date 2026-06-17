# ADR-0065: P3 — `ANALYZE <table>` + stats session-scoped en EXPLAIN

**Fecha:** 2026-05-29
**Estado:** Aceptado
**Bloque:** P3 (Fase 3 — Performance / Planeación)
**Antecede:** ADR-0063 (P1 — EXPLAIN), ADR-0064 (P2 — EXPLAIN ANALYZE)

## Contexto

P1 entregó `EXPLAIN <stmt>` describiendo el plan. P2 agregó timings y row
counts **reales** vía `EXPLAIN ANALYZE`. Pero EXPLAIN sin ANALYZE no tenía
ninguna **estimación** de cardinalidad — sólo describía el tipo de scan.

P3 cubre ese gap parcialmente: introduce `ANALYZE <table>` (sin EXPLAIN al
frente) como comando standalone que colecta stats básicas — hoy sólo
**row count exacto al momento del scan** — y las cachea en memoria. Después,
EXPLAIN consulta esas stats y anota `[est.rows=N]` en cada SCAN step
sobre la tabla analizada.

Esto es un **paso intermedio honesto** hacia un planner-as-optimizer real
(P5+): hoy no hay decisión de planeación basada en stats, sólo
**visibilidad**. El usuario ve "esta tabla tiene 50k rows, ese SCAN sin
índice va a tocar las 50k" — útil para decidir crear índices, sin pretender
que el planner reorganiza joins por costo.

## Decisión

### AST

```rust
Statement::AnalyzeTable { table: String }
```

Sin variantes adicionales — no hay opciones por ahora.

### Parser

```rust
if self.match_keyword("ANALYZE") {
    let _ = self.match_keyword("TABLE"); // opcional
    let table = self.expect_ident()?;
    return Ok(Statement::AnalyzeTable { table });
}
```

Acepta tanto `ANALYZE TABLE foo` (estilo MySQL/MariaDB) como `ANALYZE foo`
(estilo PostgreSQL). **No hay conflicto** con `EXPLAIN ANALYZE <stmt>`:
ese se consume antes en `parse_statement` con el match `EXPLAIN`. ANALYZE
standalone sólo llega aquí cuando NO viene precedido de EXPLAIN.

### Engine

Nuevo campo en `Engine`:

```rust
table_stats: HashMap<String, TableStats>,

pub struct TableStats {
    pub row_count: u64,
    pub analyzed_at_nanos: u128,
}
```

`exec_analyze_table(table)`:

1. Chequea `PRIV_SELECT` (mismo que un SELECT * — ANALYZE es lectura).
2. Carga `TableMeta` del catálogo. Error `[GBY-2001]` (`TABLE_NOT_FOUND`)
   si no existe. [Fix 2026-06-15: el ADR original cita por error
   `[GBY-4143]`; el código real es `2001` — rango Catalog según
   `ERROR_CODES.md`.]
3. `catalog.scan_rows(meta.root_page, 0, None)?.len()` → row_count exacto.
4. `analyzed_at_nanos = SystemTime::now().duration_since(UNIX_EPOCH)`.
5. `self.table_stats.insert(table, TableStats { row_count, analyzed_at_nanos })`.
6. Devuelve `ResultSet { columns: ["table","row_count"], rows: [...] }`
   con `message` aclarando "Cache session-scoped — se pierde al cerrar".

### EXPLAIN consume las stats

Nuevo helper `Engine::stats_annotation(table)` que devuelve
`" [est.rows=N]"` si hay entry, o `""` si no. `classify_scan` lo concatena
al final de cada string de SCAN — full scan, PK lookup, hash-index,
ordered-int, between, compare, complex.

### Invalidación

- **DROP TABLE**: `exec_drop_table` llama `self.table_stats.remove(&name)`.
  Si después se hace `CREATE TABLE` con el mismo nombre, no quedan stats
  viejas leaking.
- **TRUNCATE**: no invalida explícitamente. Justificación: TRUNCATE no
  cambia el schema y el row count efectivo pasa a 0 — el usuario que
  quiere stats post-truncate debe re-ejecutar ANALYZE. (Decisión:
  consistente con el resto de DDL que tampoco auto-actualiza nada).
- **INSERT/UPDATE/DELETE**: **no invalidan**. Las stats quedan **stale**
  silenciosamente. Esto es honesto: re-ejecutar full scan en cada DML
  sería un costo enorme para una feature de observabilidad. PostgreSQL
  hace lo mismo (autovacuum corre periódicamente, no per-statement).
- **Cierre del Engine**: las stats viven en `HashMap` en memoria.
  Re-abrir la DB = stats vacías. **Persistencia es P3b**.

## Alternativas consideradas

1. **Persistir stats en el catálogo (`ObjectKind::TableStats`)** desde
   P3. Descartado para el primer cut: requiere bump VERSION 31→32,
   migración, serialize/deserialize, lifecycle vs DROP. Es trabajo
   real que conviene aislar en P3b.
2. **Auto-ANALYZE en INSERT/UPDATE/DELETE** para mantener stats frescas.
   Descartado: cada DML pagaría un full scan. Costo prohibitivo. Una
   alternativa válida sería incrementar/decrementar contadores en
   cada DML, pero eso requiere refactor mayor y entra en el scope de
   P3b/P4.
3. **Stats por-columna (NDV, MCV, histogramas)**. Descartado para P3 —
   es el alcance de P4. Hoy con row_count global ya hay valor para
   el usuario.
4. **Renombrar a `VACUUM ANALYZE` estilo Postgres**. Descartado: VACUUM
   implica reclamar espacio (cosa que gabysql tampoco implementa hoy).
   ANALYZE solo, sin VACUUM, es más honesto.

## Tests

Ocho tests `p3_*`:

- `p3_analyze_returns_row_count` — INSERT 3 rows → ANALYZE → row_count=3.
- `p3_analyze_sin_keyword_table_funciona` — `ANALYZE u` (sin TABLE) parsea.
- `p3_analyze_table_inexistente_falla` — `[GBY-2001]` o "no existe".
- `p3_explain_sin_analyze_previo_no_muestra_est_rows` — EXPLAIN cold no anota.
- `p3_explain_post_analyze_muestra_est_rows` — `[est.rows=5]` en SCAN.
- `p3_explain_con_where_pk_lookup_muestra_est_rows` — PK lookup también
  trae stats anexadas.
- `p3_stats_son_session_scoped_se_pierden_en_reapertura` — re-abrir DB =
  EXPLAIN sin est.rows.
- `p3_drop_table_invalida_stats` — DROP + CREATE + INSERT = no leak.

## Consecuencias

- (+) Visibilidad real de cardinalidad en EXPLAIN. Decisión de "crear
  índice o no" se hace con datos concretos.
- (+) Sin bump on-disk: zero migration risk. Tests existentes (708+8 P2)
  no cambian.
- (+) Lecciones aprendidas se conservan: stats viejas explícitamente
  marcadas como session-scoped + warning en message del ANALYZE.
- (-) Stats stale entre DML y siguiente ANALYZE. El usuario debe
  re-ejecutar manualmente. PostgreSQL tiene el mismo trade-off
  (autovacuum no es online).
- (-) No persiste — abrir/cerrar pierde el trabajo. Esperable para P3;
  P3b lo levanta a catálogo.
- (-) Sólo row_count. No hay NDV, MCV ni histogramas. EXPLAIN no puede
  estimar selectividad de un predicado específico — sólo decir "esta
  tabla tiene N rows".

## Limitaciones / Trabajo futuro

- **P3b**: persistir stats vía `ObjectKind::TableStats` + bump VERSION.
  Lifecycle: stats viven mientras viva la tabla, se borran con DROP TABLE.
- **P4**: stats por-columna (NDV vía HyperLogLog, top-K MCV,
  histogramas equi-depth). Habilita estimación de selectividad real.
- **P5**: planner-as-optimizer real — usa stats para reorden de joins,
  choice de índice, decisión de hash vs nested-loop.
- **Auto-ANALYZE / autovacuum**: agendamiento periódico, threshold-based.
  Requiere infraestructura de jobs.
- **`ANALYZE` global** (sin argumento) que itere todas las tablas. Hoy
  hay que llamarlo una por una.
