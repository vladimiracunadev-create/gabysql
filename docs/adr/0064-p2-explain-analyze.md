# ADR-0064: P2 — EXPLAIN ANALYZE

**Fecha:** 2026-05-29
**Estado:** Aceptado
**Bloque:** P2 (Fase 3 — Performance / Planeación)
**Antecede:** ADR-0063 (P1 — EXPLAIN)

## Contexto

P1 introdujo `EXPLAIN <stmt>` como dry-run: devuelve un plan estimado
(scan type, joins, filtros, order) sin ejecutar el statement. Útil para
ver qué piensa hacer el engine, pero no responde la pregunta clave
"¿cuánto realmente tardó y cuántas filas tocó?".

P2 cierra ese gap agregando la modalidad `EXPLAIN ANALYZE <stmt>`:
mismo plan que P1, **más** ejecución real del statement, **más**
tiempo wall-clock medido con `std::time::Instant`, **más** conteo
real de filas producidas.

## Decisión

### AST

`Statement::Explain` pasa de variante tupla a struct con flag:

```rust
Explain {
    analyze: bool,
    inner: Box<Statement>,
}
```

### Parser

```rust
if self.match_keyword("EXPLAIN") {
    let analyze = self.match_keyword("ANALYZE");
    let inner = self.parse_statement()?;
    return Ok(Statement::Explain { analyze, inner: Box::new(inner) });
}
```

### Engine — `exec_explain(inner, analyze)`

1. Si `analyze=true`, clona `inner` antes del match plan-walker
   (el walker consume el `inner` original).
2. Corre el walker P1 → llena `steps` con el plan estimado.
3. Si `analyze=true`:
   - `t_start = Instant::now()`
   - `exec_res = self.exec(inner_clone)`
   - `elapsed = t_start.elapsed()`
   - Si `Ok(rs)`: agrega `actual.time` (ms con 3 decimales) y
     `actual.rows` (conteo real, con sufijo de mensaje del inner si lo trae).
   - Si `Err(e)`: agrega `actual.error` con código + ms.
4. `message` del ResultSet diferencia:
   - Sin ANALYZE: `"EXPLAIN: plan estimado (sin ejecutar el statement)"`.
   - Con ANALYZE OK: `"EXPLAIN ANALYZE: plan + ejecución real (X ms, N rows). Cuidado: side-effects PERSISTIDOS."`.
   - Con ANALYZE ERR: `"EXPLAIN ANALYZE: plan + ejecución real (X ms, ERROR)."`.

### Side-effects

**ANALYZE ejecuta el statement real**. Eso significa:
- `EXPLAIN ANALYZE INSERT ...` **persiste** la fila.
- `EXPLAIN ANALYZE UPDATE/DELETE ...` **muta** la tabla.
- `EXPLAIN ANALYZE DDL` (CREATE, DROP, etc.) aplica los cambios.

Si el usuario quiere dry-run, debe usar `EXPLAIN <stmt>` (sin ANALYZE).
El warning en `message` lo deja explícito.

### Error capture

Si el inner falla (PK violation, RLS check, syntax mid-exec, lo que sea),
P2 **no propaga** el error como fallo de EXPLAIN ANALYZE. Lo captura
como step `actual.error` y devuelve `ResultSet` OK con el plan + el
error registrado como dato. Razón: EXPLAIN ANALYZE es una herramienta
de observabilidad; el caller quiere ver "esto falló y tardó X ms",
no recibir un Err que descarta el plan.

## Alternativas consideradas

1. **Hacer ANALYZE transaccional con rollback automático**
   (PostgreSQL ofrece `EXPLAIN (ANALYZE) BEGIN; ... ROLLBACK`).
   Descartado: gabysql aún no expone transacciones explícitas como
   primer-clase. Sería una abstracción nueva. Lo dejo para Fase 4 (TX).
2. **Reportar tiempo por sub-step (per-scan timing)**.
   Descartado para P2: requiere instrumentar cada loop interno
   (exec_select, exec_join, exec_filter, exec_order). Demasiado scope
   para un bloque. Queda para P4 (instrumentación granular).
3. **Hacer ANALYZE devolver dos result sets (plan + filas reales)**.
   Descartado: rompe contrato de `ResultSet` único por statement
   en el resto del engine.

## Tests

Siete tests p2_* + uno renombrado del P1 obsoleto:

- `p1_explain_analyze_now_executes_after_p2` — reemplaza al test que
  esperaba [GBY-4139] y ahora verifica que ANALYZE corre.
- `p2_explain_analyze_select_includes_plan_and_actuals` — plan + actual.time + actual.rows.
- `p2_explain_analyze_select_zero_rows` — actual.rows="0 filas producidas".
- `p2_explain_analyze_insert_persists` — INSERT vía ANALYZE persiste.
- `p2_explain_sin_analyze_no_persiste` — EXPLAIN sin ANALYZE no toca la tabla.
- `p2_explain_analyze_update_modifica` — UPDATE vía ANALYZE muta.
- `p2_explain_analyze_message_warns_persistencia` — message contiene "PERSISTIDOS".
- `p2_explain_analyze_error_capturado` — PK violation → step "actual.error".

## Consecuencias

- (+) Visibilidad real de tiempo y row counts. Útil para detectar regresiones.
- (+) Cero bump de VERSION on-disk — pura feature de runtime.
- (-) ANALYZE de DML/DDL persiste. El warning en `message` lo aclara,
  pero el usuario podría sorprenderse. La doc lo dice también.
- (-) Tiempo medido cubre TODO el `exec()`, incluido overhead de
  EXPLAIN, no solo el statement real. Es una primera aproximación
  honesta; P4 puede refinar a per-step.

## Limitaciones / Trabajo futuro

- **P3 (deferred)**: planner-as-optimizer real, no solo descriptor.
  Hoy `classify_scan` mira las mismas fast-paths que el engine
  ya tiene hardcoded — no hay decisión de planeación.
- **P4 (deferred)**: instrumentación granular per-step (scan time,
  join time, sort time) en lugar de un solo Instant total.
- **TX/Rollback de ANALYZE**: requiere transacciones explícitas
  (Fase 4).
