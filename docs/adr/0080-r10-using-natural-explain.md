# ADR-0080: R10 — EXPLAIN extiende heurística P5e a USING/NATURAL JOIN

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Bloque:** R10 (reparación post-P5)
**Origen:** [docs/ANALISIS_POST_P5.md §3 R10](../ANALISIS_POST_P5.md) — tensión #2.6 relacionada.
**Refina:** [ADR-0073](0073-p5e-join-algorithm-annotation.md) (P5e).

## Contexto

P5e (ADR-0073) anota el algoritmo real del JOIN en EXPLAIN: `index-loop`,
`hash join`, o `nested-loop` según las reglas del dispatcher.

Para `ON l.col = r.col` el análisis ya era completo — si `r.col` es PK o
tiene un índice secundario, se anota `index-loop`. Para `USING(col)` y
`NATURAL JOIN`, el comentario in-code decía:

```rust
// USING/NATURAL: si el nombre coincide con PK o índice del right,
// también irían a index-loop. Pero no resolvemos los nombres acá
// sin scope completo — reportamos hash conservadoramente.
"hash join, ~O(N+M)".to_string()
```

El fallback "hash" era seguro pero impreciso: el dispatcher real
desazucara USING/NATURAL a un equi-predicate con la misma columna en
ambos lados, así que el path de ejecución es **idéntico** al del ON
explícito. La heurística estática estaba más conservadora que el
runtime.

## Decisión

Extender `classify_join_algorithm` para resolver los nombres de columnas
de USING y NATURAL y aplicar el mismo check de PK/índice del right que
ya hace el path ON.

### Caso USING

```rust
let usn_keys: Vec<String> = if let Some(cols) = &join.using {
    cols.clone()
} else if join.natural { ... } else { Vec::new() };
```

`join.using` ya tiene la lista de nombres explícitos del usuario —
se usa tal cual.

### Caso NATURAL

NATURAL JOIN no lleva nombres explícitos; el dispatcher real calcula
intersección de columnas en runtime con el scope completo. Para la
heurística estática agrego un helper:

```rust
fn natural_join_keys(&mut self, left_table: &str, right_table: &str) -> Vec<String> {
    // intersección por nombre (case-insensitive) entre
    // left_meta.columns y right_meta.columns
}
```

Recibe `base_table` (que ahora deja de ser `_base_table`) como
aproximación del lado izquierdo del JOIN.

### Check shared con ON

Una vez resueltas las keys candidatas, el loop es idéntico al del path
ON: para cada `col`, si normalizado matchea `right_meta.primary_key` →
index-loop PK; si está en `right_meta.index_for_column` → index-loop
con nombre del índice; sino → fallback hash.

## Consecuencias

### Positivas

- **EXPLAIN ya no miente "hash join"** cuando el JOIN va a correr como
  index-loop. Tensión #2.6 mitigada (ese ítem nombra RIGHT/FULL JOIN
  específicamente — sigue abierto para esos casos pero R10 cubre el
  USING/NATURAL transversal).
- Mismo formato de mensaje que ON — el usuario no aprende dos lecturas.
- Heurística estática, cero costo de runtime.

### Negativas / deuda

- **Aproximación NATURAL JOIN en chain**: `natural_join_keys` mira
  solo `base_table`, no el scope acumulado por joins previos. Si la
  columna NATURAL viene de una tabla agregada por un join anterior
  (`a JOIN b NATURAL JOIN c` donde la columna común está entre `b` y
  `c`), la heurística no la encuentra y cae a "hash" — el dispatcher
  real sí lo resolverá. Sub-estimación silenciosa; deuda documentada
  acá. Para corregirla correctamente hay que reconstruir el scope
  acumulado, que es trabajo separado.
- **No considera tipo del índice**: si la columna tiene un índice
  ordered-int + un hash sobre la misma col, devuelve el primero que
  encuentre `index_for_column`. No es problema funcional — el
  dispatcher elige el path real igual.
- **USING multi-col no soportado por el parser hoy** (ADR del JOIN
  declara "exactamente UNA columna"). R10 itera el `Vec<String>` por
  si en el futuro se generaliza — el primero que matchee gana.

## Alternativas consideradas

1. **Resolver USING/NATURAL en runtime, no en EXPLAIN.** Más preciso
   pero requiere ejecutar el planner. Rechazado: EXPLAIN debe ser
   instantáneo.
2. **Mantener "hash join" para NATURAL en chain joins.** Más honesto
   que sub-estimar, pero pierde precisión en el 90% del uso real
   (single join). Rechazado: la deuda está documentada.
3. **No hacer nada (mantener el comportamiento P5e original).** Falla
   silenciosa que confunde al usuario que ve "hash join" cuando el
   tiempo real es de index-loop. Rechazado.

## Tests

Cuatro tests nuevos (`r10_*` en `tests/integration_test.rs`):

- `r10_explain_using_sobre_pk_anota_index_loop` — `USING (id)` con
  `id` como PK común → `index-loop on b.id (PK)`.
- `r10_explain_using_sobre_indice_secundario` — `USING (code)` con
  índice secundario `idx_b_code` → `index-loop on b.code (index idx_b_code)`.
- `r10_explain_natural_sobre_pk_anota_index_loop` — `NATURAL JOIN`
  donde la columna común `id` es PK del right → `index-loop ... (PK)`.
- `r10_explain_using_sobre_columna_sin_indice_cae_a_hash` — `USING (k)`
  sin índice ni PK en `k` → `hash join`. Regression test del fallback.

Suite total: 804 → **808** (+4). Sin regresiones.

## Referencias

- [ADR-0073 — P5e join algorithm annotation](0073-p5e-join-algorithm-annotation.md)
- [ANALISIS_POST_P5 §2.6](../ANALISIS_POST_P5.md) — tensión relacionada
  (RIGHT/FULL JOIN heurística).
- [TAREAS_PENDIENTES.md §4 R10](../TAREAS_PENDIENTES.md)
