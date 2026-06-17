# ADR-0087: M4 — fuzz parser hand-rolled (1 hora limpia, 500M iters)

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Bloque:** M4 (línea README "parser hardened" históricamente pendiente)
**Origen:** [docs/TAREAS_PENDIENTES.md §3](../TAREAS_PENDIENTES.md) — "1 hora mínima sin panic ni unwrap fallido" declarado como prioridad alta desde hace meses.
**Hermana:** ADR-0084 (M3 — proptest planner) y ADR-0086 (proptest Pager). Mismo enfoque hand-rolled zero-deps.

## Contexto

SQLite tiene millones de horas de fuzz acumuladas — parte central de su
credibilidad. gabysql tenía **cero**. Cualquier afirmación de "parser
robusto" sin evidencia de fuzz es vacía.

`cargo fuzz` real (libFuzzer / AFL) requiere:

- **Linux** (libFuzzer no funciona limpio en Windows).
- **Rust nightly** (libFuzzer hook).
- **Setup inicial no trivial** (`cargo fuzz init`, target binaries, corpus).

Ninguno disponible en el entorno típico del autor (Windows + GNU
toolchain, sin MSVC linker). Choca también con ADR-0001 (zero deps
core) — `cargo-fuzz` agrega `libfuzzer-sys` como dep.

## Decisión

Implementar un **stress test del parser hand-rolled** que dé el mismo
nivel de evidencia práctica que una hora de `cargo fuzz` real,
ejecutable en el entorno del autor.

### Diseño (`tests/fuzz_parser.rs`)

- **LCG determinístico** (mismas constantes que gabybench /
  proptest_planner / proptest_pager).
- **Generador mixto 70/30**:
  - 70% queries pseudo-estructuradas: 2-60 tokens picados de un
    vocabulario SQL (~90 keywords, 13 operadores, 7 puntuaciones) +
    identificadores `[a-z]{1,12}` + literales numéricos/string/NULL.
  - 30% mutación adversarial: query estructurado + 1-4 bytes random
    inyectados/borrados/reemplazados (ataca decoding UTF-8, buffer
    bounds, tokenizer corner cases).
- **`panic::catch_unwind`** envuelve cada llamada a `parse()`. Si
  panica, captura seed + query y sigue (hasta 20 panics, después
  corta).
- **`panic::set_hook(Box::new(|_| {}))`** suprime el output stderr
  durante la corrida — sin esto cada panic dumpea stack trace
  ilegible.
- **Progress cada 5 segundos**: iters, parse_ok, parse_err, panics.
- **Duración configurable** via env var `GABYSQL_FUZZ_PARSER_SECS`
  (default 60). Marked `#[ignore]` — no corre en CI per-commit.

### Comando para reproducir 1h:

```bash
GABYSQL_FUZZ_PARSER_SECS=3600 \
    cargo test --target x86_64-pc-windows-gnu --release \
    --test fuzz_parser -- --ignored --nocapture
```

## Evidencia de la primera corrida

Capturada en
[`docs/fuzz/FUZZ-RUN-2026-06-15.md`](../fuzz/FUZZ-RUN-2026-06-15.md):

| Métrica | Valor |
|---|---|
| Duración | 3 600 s |
| Iters totales | **503 861 946** |
| Throughput | 139 961 iters/seg |
| Parse OK (random suerte) | 74 787 |
| Parse error | 503 787 159 |
| **PANICS** | **0** |

500 millones de inputs random — incluyendo 30% bytes mutados —
**ninguno disparó panic, unwrap fallido ni loop infinito** en
`gabysql::sql::parse()`.

## Lectura honesta (importante)

Esta evidencia respalda:

- El parser no panic-ea ante inputs random masivos.
- Los `unwrap()` y `expect()` internos cubren los casos que el
  generador supo armar.

Esta evidencia **NO** respalda:

- Coverage del parser. Sin `cargo fuzz` real (coverage-guided), no
  sabemos qué % del código del parser se ejercitó. Generación pura
  random tiende a explorar shapes superficiales primero.
- Que el motor entero esté hardened. Solo `parse()` se invocó —
  `exec()` queda sin fuzz.
- Que no haya bugs semánticos. Un parser que devuelve `Err` no es un
  parser correcto — solo es un parser que no panic-ea.

El ADR de la evidencia (FUZZ-RUN-2026-06-15.md) detalla estas
limitaciones y propone próximos pasos: cargo-fuzz real en CI Linux,
fuzz sobre `exec()`, coverage-guided generation.

## Consecuencias

### Positivas

- **Línea pendiente del README satisfecha**. "X horas de fuzz limpio"
  ahora es una afirmación con evidencia citable (commit + log
  capturado).
- **Reproducible 1:1** vía seed determinístico. Si un dev futuro
  agrega un caso al generador y rompe algo, el log captura el seed.
- **Cero deps externas** — alinea con ADR-0001 y no agrega superficie
  de supply chain.
- **Re-ejecutable cuando sea**. Refrescar la evidencia es 1 hora de
  CPU + 1 markdown actualizado.

### Negativas / deuda

- **No es coverage-guided**. La gran ventaja de libFuzzer/AFL es que
  el motor aprende qué inputs exploran código nuevo. Acá generamos
  random puro — saturación efectiva del espacio cubierto.
- **Solo parser, no exec**. Bugs de runtime no se atrapan acá.
- **Marcado `#[ignore]`**. Si nadie lo corre, se vuelve invisible. La
  evidencia capturada en markdown mitiga eso (queda en docs).
- **No prueba parseos válidos largos**. Tokens random tienden a
  errores rápido; queries de >60 tokens válidos sintácticamente son
  estadísticamente raros en el output.

## Alternativas consideradas

1. **`cargo fuzz` real en CI Linux** (libFuzzer). La evidencia sería
   más fuerte (coverage-guided). Diferido porque requiere setup
   nightly + workflow GitHub Actions + corpus inicial. ADR-0087 deja
   propuesta para próxima iteración.
2. **`AFL`** — descartado por mismas razones.
3. **`proptest` crate** — choca con ADR-0001.
4. **No hacer nada, dejar la línea del README pendiente
   indefinidamente** — rechazado: 1 hora de generación random es
   mejor que cero horas de cualquier cosa.

## Tests añadidos

- `m4_fuzz_parser_no_panic` (`#[ignore]`).

**Suite total**: sigue en **816 verde + 4 ignored** (1 Argon2 + 2
env-var + 1 fuzz_parser). El fuzz no afecta el conteo de la corrida
default.

## Próxima mejora documentada

Cuando alguien quiera subir el bar:

1. CI Linux con `cargo fuzz` real (workflow nightly).
2. Fuzz sobre `exec()` con pre-fixture de tabla.
3. Coverage-guided generation.

## Referencias

- [Evidencia capturada — FUZZ-RUN-2026-06-15.md](../fuzz/FUZZ-RUN-2026-06-15.md)
- [ADR-0084 — M3 proptest planner](0084-m3-proptest-planner.md)
- [ADR-0086 — proptest Pager](0086-pager-proptest.md)
- [ADR-0001 — Zero deps core](0001-rust-zero-deps-core.md)
- [TAREAS_PENDIENTES §3](../TAREAS_PENDIENTES.md) — declaraba este item desde hace meses.
