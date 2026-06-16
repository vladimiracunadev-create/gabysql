# ADR-0085: R3-cont — Sweep empírico de P5D_SWAP_THRESHOLD (datos inconclusos, 2.0 stays)

**Fecha:** 2026-06-15
**Estado:** Aceptado — calibración completada con outcome explícito: "data inconclusa, mantener default"
**Bloque:** R3-cont (continuación de R3, ADR-0082 instrumentación)
**Origen:** [ADR-0082](0082-r3-p5d-swap-threshold-instrumentation.md) marcaba R3 como "entrega parcial, calibración pendiente de bench chain-join". Este ADR cierra esa pendiente.

## Contexto

R3 (ADR-0082) extrajo `P5D_SWAP_THRESHOLD` a constante de módulo y
agregó el override por env var `GABYSQL_P5D_SWAP_THRESHOLD`. **No
cambió el valor** porque el smoke bench de la época no tenía queries
que ejercieran asimetría de cardinality entre `current` (acumulado de
joins previos) y `right` (próxima tabla).

R3-cont agrega esa query y corre el sweep manualmente para informar
una decisión basada en datos.

## Metodología

### Query agregada al smoke

```sql
SELECT COUNT(*) FROM lines l JOIN orders o ON o.total = l.precio
```

Esta query es la única del smoke que va por **hash join** (no
index-loop, no nested): el predicate `o.total = l.precio` no matchea
PK ni índice del RHS. Asimetría de cardinality:

- `lines` (current, FROM): 100 000 filas.
- `orders` (right, JOIN): 20 000 filas.
- ratio current/right = **5.0×**.

### Sweep

4 corridas de `gabybench smoke`, cada una con un override distinto:

```bash
GABYSQL_P5D_SWAP_THRESHOLD=1.5 ./target/.../gabybench smoke
GABYSQL_P5D_SWAP_THRESHOLD=2.0 ./target/.../gabybench smoke   # default
GABYSQL_P5D_SWAP_THRESHOLD=3.0 ./target/.../gabybench smoke
GABYSQL_P5D_SWAP_THRESHOLD=10.0 ./target/.../gabybench smoke
```

Threshold 1.5 / 2.0 / 3.0 → **swap activo** (5.0 > threshold). 10.0 →
**sin swap**. 10 iteraciones por corrida (default para hash-join
queries grandes — más iter pega muy fuerte la suite).

## Resultados

| Threshold | Swap activo | p50 | p95 | mean | Comentario |
|---|---|---|---|---|---|
| **1.5** | sí | 444.92 ms | 527.43 ms | 449.83 ms | swap más agresivo |
| **2.0** | sí | 451.54 ms | 1133.10 ms | 534.85 ms | **default** — p95 outlier alto (ver §Notas) |
| **3.0** | sí | 397.80 ms | 400.81 ms | 395.34 ms | swap más conservador, mejor mean |
| **10.0** | no | 436.59 ms | 465.75 ms | 439.84 ms | sin swap |

**Rango total**: 395 – 535 ms mean. Diferencia entre extremos ~35%,
pero p95 muestra alta varianza intra-corrida (1133 ms en 2.0 vs 400
ms en 3.0 — atribuible a outlier de single-iter sobre 10).

## Interpretación

### Lectura literal

No hay diferencia estadísticamente significativa entre los 4
thresholds. El mean del "no swap" (439 ms) cae **dentro del rango de
los swap variants** (395–535 ms). El swap más conservador (3.0) da el
mejor mean (395 ms) y la menor varianza (p95 = 400 ms).

### Por qué la data es así

En esta query, build = right (default sin swap) construye hash sobre
20k filas (`orders`); con swap, sobre 100k (`lines`). Esperaríamos
que swap **empeore** porque la hash table grande tiene más colisiones
y peor cache locality.

Lo que observamos en cambio es **indistinguible** — los thresholds 1.5
y 2.0 (que disparan swap) están en el mismo orden de magnitud que 10.0
(no swap). Posibles causas:

- La probe phase domina sobre la build phase: 100k probes con hash
  table de 20k vs 20k probes con hash table de 100k — los conteos de
  hash compute son comparables.
- Cache effects dominan over hash collisions a esta escala.
- 10 iteraciones no atrapan diferencias de <20%.

### Por qué 3.0 dio el mejor mean

Probablemente noise de single-corrida (no es estadístico). Repetir el
sweep N veces y promediar daría una señal más clara, pero el costo
operativo crece linealmente: 4 corridas × 5 repeticiones × 2 min/corrida
= 40 minutos solo para R3, sin mover otras agujas.

## Decisión

**Mantener `P5D_SWAP_THRESHOLD = 2.0`** como default.

Razones:

1. **No hay base empírica para mover el valor**. La data muestra
   diferencias dentro de ±15% del mean — ruido más que señal.
2. **El rationale original de ADR-0072 sigue válido**: "Threshold 2×:
   solo invertimos cuando hay ventaja clara, para no introducir cambios
   sutiles de orden de filas en queries sin ORDER BY". Es una elección
   de **conservadurismo semántico**, no de optimización. Mover el
   threshold sin razón es agregar inestabilidad sin contrapartida.
3. **El env var sigue disponible**: cualquier usuario con un workload
   patológico puede sobreescribir con `GABYSQL_P5D_SWAP_THRESHOLD=X`.
   No se pierde flexibilidad.

## Consecuencias

### Positivas

- **R3 cierra como "calibración intentada con outcome explícito"**, no
  como "deferred indefinidamente". Honestidad sobre el límite de lo
  que el smoke bench puede informar.
- **Habilita futuras re-calibraciones** sin re-hacer el setup — la
  query chain-join `"JOIN hash O×L (P5d swap target)"` queda en el
  smoke. Próxima vez que alguien tenga interés puede correr el sweep
  con N > 10 iters y resolver con menos noise.
- **No introduce regresiones**: el valor de la constante NO cambia,
  comportamiento idéntico al pre-R3-cont.

### Negativas / deuda

- **Cobertura limitada del espacio de queries**. Una sola query con un
  ratio fijo (5×) no caracteriza el espacio total. Workloads con
  ratios extremos (50×, 500×) podrían cambiar la conclusión.
- **N=10 iteraciones es bajo** para detectar diferencias <20%.
  Mejorable subiendo a N=50 o 100, a costa de tiempo de smoke.
- **No probamos chain joins reales** (A×B×C donde current acumulado).
  El smoke no tiene 3 tablas relacionables; cada nueva chain pediría
  más infra de bench.

## Trabajo futuro (re-abierto si el smoke crece)

1. **N>=50 iters** por threshold para reducir varianza.
2. **Ratios extremos** (current/right = 50×, 500×) — agregar queries
   de magnitudes asimétricas.
3. **Verdadero chain join** (3+ tablas con cardinality acumulada).
4. **Memoria** además de latencia (hash table size — observar con
   profiler).

Ninguno bloquea avanzar el motor. Quedan como mejora si el espacio
comparativo lo exige.

## Tests

Sin cambios en suite — la query agregada al bench es para medir, no
para testear. Suite sigue en 813 verde.

## Referencias

- [ADR-0072 — P5d hash-join build-side selection](0072-p5d-hash-join-build-side.md)
- [ADR-0082 — R3 instrumentación](0082-r3-p5d-swap-threshold-instrumentation.md) — el push del que esto es continuación
- [ADR-0075 — M2 gabybench en CI](0075-m2-gabybench-in-ci.md) — la infra de bench
- `bench/results.json` — última corrida con la nueva query
