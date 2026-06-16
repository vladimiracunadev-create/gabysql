# ADR-0082: R3 — Instrumentación de P5D_SWAP_THRESHOLD (sin cambio de valor)

**Fecha:** 2026-06-15
**Estado:** Aceptado — entrega parcial (instrumentación; calibración pendiente de data)
**Bloque:** R3 (reparación post-P5)
**Origen:** [docs/ANALISIS_POST_P5.md §3 R3](../ANALISIS_POST_P5.md) — tensión #2.4
**Refina:** [ADR-0072](0072-p5d-hash-join-build-side.md) (P5d) y [ADR-0081](0081-r2-index-breakeven-calibration.md) (mismo patrón aplicado a R2).

## Contexto

P5d (ADR-0072) introdujo swap del build side del hash join:

```rust
let swap_build_side = current.len() > right_rows.len() * 2;
```

El factor **2×** vino de la misma heurística teórica que `INDEX_BREAKEVEN`
en P5c: "swap solo cuando hay ventaja clara, para no flippear orden de
filas en queries sin ORDER BY por desniveles chicos". No fue calibrado
contra workload real.

R2 (ADR-0081) calibró `INDEX_BREAKEVEN` empíricamente porque el smoke
bench tenía las queries necesarias (PK lookup vs FullScan). Para R3, el
**smoke bench actual NO tiene queries que ejerciten asimetría de
cardinality** entre `current` (acumulado de joins previos) y un `right`
chico. Todas las JOIN queries del smoke son single-target (`WHERE id=K`)
que matchean fila única — fuera del rango donde el swap importa.

Sin paired data al menos a 1.5×, 2.0×, 3.0× sobre un dataset común, no
hay base honesta para mover el threshold.

## Decisión

Entrega parcial: solo **instrumentación**, valor sin cambios.

1. **Extraer constante a módulo**: `P5D_SWAP_THRESHOLD: f64 = 2.0`.
2. **Helper con env var override**: `p5d_swap_threshold()` lee
   `GABYSQL_P5D_SWAP_THRESHOLD` (rango aceptado `≥ 1.0`; fail-soft
   fuera de rango o no parseable). Mismo patrón que R2.
3. **Callsite usa el helper**:

```rust
let swap_build_side =
    (current.len() as f64) > (right_rows.len() as f64) * p5d_swap_threshold();
```

4. **Valor default = 2.0**: idéntico a ADR-0072.

### Por qué entrega parcial

R2 cambió el valor con confianza porque tenía data (`bench/results.json`
de un smoke run). Para R3 no tenemos data; cambiar 2× → 1.5× o 3× sin
medición sería el mismo "deriva teórica" que motivó la tensión #2.4
originalmente. Mejor reconocerlo y dejar la instrumentación lista que
inventar un número.

## Trabajo futuro (para cerrar R3 completo)

Para una calibración honesta del threshold:

1. **Agregar bench queries chain-join al smoke** — ejemplo:
   `SELECT count(*) FROM A JOIN B ON ... JOIN C ON ...` donde `A×B`
   acumula 50k-100k filas y `C` tiene 10-100 filas. Eso ejercita el
   path swap.
2. **Sweep manual de 3 corridas** con el env var:
   ```bash
   GABYSQL_P5D_SWAP_THRESHOLD=1.5 cargo run --release --bin gabybench -- smoke
   GABYSQL_P5D_SWAP_THRESHOLD=2.0 cargo run --release --bin gabybench -- smoke
   GABYSQL_P5D_SWAP_THRESHOLD=3.0 cargo run --release --bin gabybench -- smoke
   ```
3. **Comparar latencia mean/p99** entre las 3 sobre las queries chain.
4. **Decidir el nuevo valor** o confirmar 2.0 con data.

Esto es un push separado (R3-cont o R3.2), porque agregar las bench
queries cambia el shape del artifact `bench/results.json` — vale la
pena dejarlo como cambio aislado para no mezclar con la instrumentación.

## Consecuencias

### Positivas

- **El override existe**: usuario puede correr `GABYSQL_P5D_SWAP_THRESHOLD=1.5
  cargo run ...` para experimentar sin recompilar.
- **El callsite ya usa el helper**: cuando la calibración real ocurra,
  cambiar el default es 1 línea.
- **Honesto**: el ADR documenta por qué no se cambió el valor. Cero
  pretensión de calibración sin data.
- Cierre formal de la tensión #2.4 a **modo deferred-with-instrumentation** —
  no queda abierta indefinidamente, queda con un path claro al cierre.

### Negativas / deuda

- Tensión #2.4 sigue abierta en términos de valor empírico. R3 cierra
  la parte instrumental, no la parte calibratoria.
- El smoke bench actual NO tiene queries para esto. Agregarlas es
  trabajo separado documentado arriba.
- Cero env var → cero efecto de R3. Si nadie usa el override, el push
  es invisible. Aceptable porque es para futura calibración.

## Tests

Un test `#[ignore]` (`r3_env_var_override_no_rompe_correctness_de_join`):
verifica que tres valores (0.5, 10.0, garbage) sobre el mismo JOIN
devuelven idénticos resultados — el override NO debe romper correctness,
solo cambiar el path interno. Ignored por la misma razón que R2: env
var es global y rompe a la suite paralela.

Correr serial:

```
cargo test --target x86_64-pc-windows-gnu \
    --tests r3_env_var_override -- --ignored --test-threads=1
```

Suite total: **sin cambios netos en passing count** (test ignored).

## Alternativas consideradas

1. **Cambiar el threshold a 1.5× o 3× sin data.** Rechazado: misma
   crítica que la tensión #2.4 original.
2. **Agregar las bench queries chain-join junto con la instrumentación.**
   Push mucho más grande (~200 LOC nuevas en `gabybench.rs` para setup
   de dataset + queries paired); decisión: dividir en 2 pushes para
   mantener foco.
3. **Punt R3 completo y dejar la tensión abierta.** Rechazado: la
   instrumentación es trivial y útil aún sin calibración inmediata —
   habilita el sweep del día que alguien lo corra.

## Referencias

- [ADR-0072 — P5d hash join build-side selection](0072-p5d-hash-join-build-side.md)
- [ADR-0081 — R2 INDEX_BREAKEVEN calibration](0081-r2-index-breakeven-calibration.md) (mismo patrón aplicado primero)
- [ANALISIS_POST_P5 §2.4](../ANALISIS_POST_P5.md) — tensión que motiva esto
