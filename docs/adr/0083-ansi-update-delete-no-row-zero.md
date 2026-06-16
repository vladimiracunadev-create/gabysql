# ADR-0083: UPDATE/DELETE sobre PK inexistente devuelve 0 filas (ANSI fix)

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Origen:** Bug expuesto por gabybench all 2026-06-15 (warmup procflow `UPDATE accounts WHERE id = 1_000_001` tiraba `[GBY-3006]`).
**Refina:** E3 (resolve_target_pks) — preserva el flag interno, cambia la semántica de los callers.

## Contexto

Pre-fix, `UPDATE/DELETE WHERE pk = N` con `N` no presente devolvía
`[GBY-3006] ROW_NOT_FOUND_FOR_PK`. El comentario in-code lo justificaba
como "compat pre-E3" — el motor histórico siempre erraba ahí y el flag
`was_explicit_single_pk` en `resolve_target_pks` existía para preservar
esa semántica.

PostgreSQL, SQLite y ANSI SQL devuelven en cambio `UPDATE 0` / `DELETE 0`:
WHERE que no matchea es un caso normal de cero filas afectadas, no un
error. Cualquier cliente portado de otro motor a gabysql se encontraba
con un error code que no esperaba.

El bug del bench fue solo el síntoma: cualquier closure que arma una
clave dinámica con `i` (e.g. `WHERE id = i+1`) corre el riesgo de pedir
una PK que no existe (race, batch off-by-one, warmup con offset
gigante) y voltear la operación entera.

## Decisión

Alinear con ANSI. Cambios mínimos:

### `exec_update` (sql.rs:11647)

Eliminado el early-return `[GBY-3006]` cuando `target_pks.is_empty() &&
was_explicit_single_pk`. Reemplazado con flujo normal: si la lista está
vacía, `updated` queda en 0 y el `ResultSet` final reporta `"OK (0 filas
actualizadas)"`.

Dentro del loop sobre `target_pks`, agregado `still_there` check al tope:
si la PK literal viene en la lista pero la fila no existe en disco
(porque `resolve_target_pks` no la verifica en el fast-path), `continue`
silencioso. Mismo patrón que `exec_delete` ya tenía.

### `exec_delete` (sql.rs:12671)

Eliminado el early-return análogo. El `still_there` check ya existía
desde antes — solo hizo falta sacar el error explícito.

### Flag `was_explicit_single_pk`

Permanece en la firma de `resolve_target_pks` (segundo elemento del
tuple). Ningún caller lo consume hoy; queda como hook por si más
adelante tracing/EXPLAIN quiere distinguir "WHERE pk literal" de
"WHERE pk derivado de FullScan". El doc-comment de la función refleja
la deprecación de su uso original.

### Excepción de PL/pgSQL

El error code `ROW_NOT_FOUND_FOR_PK` (codes.rs:112) **no se elimina** —
lo sigue usando el exception handler `no_data_found` en bloques
PL/pgSQL (ver `sql.rs:706`). Cambia solo quién lo emite.

## Consecuencias

### Positivas

- **Compatibilidad con clientes ANSI**: portar SQL desde PostgreSQL/SQLite
  sin sorpresas. Un `UPDATE WHERE id = $1` que antes podía tirar
  `[GBY-3006]` por dato faltante ahora devuelve 0 filas igual que
  cualquier otro motor.
- **Bench warmup robusto sin parche**: el fix de `gabybench.rs` (commit
  3c5d97c, swallow errores en warmup) sigue siendo correcto pero ya no
  enmascara este caso particular — si re-corres el bench all, el warmup
  procflow no rebotaría.
- **Eliminada una asimetría conceptual**: el flag `was_explicit_single_pk`
  era un *artifact* de la implementación, no de la semántica SQL.
  Mantenerlo separado del flujo limpia el modelo mental.

### Negativas / deuda

- **Posible silencio en errores reales**: una app que confiaba en
  `[GBY-3006]` para detectar "esta key debería existir" ya no lo
  recibirá. Mitigación: el `ResultSet.message` dice claramente "0 filas
  actualizadas/eliminadas"; los callers que necesitan asegurarse pueden
  inspeccionar el número o usar `SELECT 1 FROM t WHERE pk = N` antes.
- **El flag `was_explicit_single_pk` queda como ruido en la firma**.
  Removerlo es 1 push trivial cuando se confirme que nadie más lo
  necesita. No urgente.

## Alternativas consideradas

1. **Solo arreglar el bench (mantener el error legacy)**. Rechazado:
   trataría el síntoma, no la causa. Cualquier cliente real seguiría
   chocando con el código `[GBY-3006]` no-ANSI.
2. **Cambiar `apply_update_to_pk` para devolver `DbResult<bool>`**.
   Toca 2+ call sites (UPSERT incluido), riesgo de regresiones.
   Rechazado en favor del `still_there` check al tope del loop —
   localizado, mismo patrón que ya usa DELETE.
3. **Eliminar el flag `was_explicit_single_pk`** del return de
   `resolve_target_pks`. Diferido: nadie lo consume hoy pero la firma
   ya está cambiada; modificarla obliga a tocar las 3 callsites del
   helper sin ganancia inmediata. Sigue como deuda chica.

## Tests

Dos tests existentes ajustados (antes asertaban el error):

- `update_and_delete_by_pk_roundtrip` (línea ~322): assert message
  contiene `"0 fila"` en vez de error.
- `e3_update_by_pk_returns_zero_rows_when_not_found` (renombrado desde
  `_still_errors_when_not_found`): igual.

Un test nuevo (`ansi_delete_by_pk_returns_zero_rows_when_not_found`):
verifica que `DELETE FROM t WHERE id = 999` sobre tabla sin esa PK
devuelve "0 filas eliminadas" sin error.

Suite total: 809 → **810** (+1).

## Referencias

- [E3 (resolve_target_pks)](../../src/sql.rs)
- [PL/pgSQL exception handler no_data_found](../../src/sql.rs) (sql.rs:706)
- Bench bug expuesto por gabybench all 2026-06-15 — ver commit 3c5d97c.
