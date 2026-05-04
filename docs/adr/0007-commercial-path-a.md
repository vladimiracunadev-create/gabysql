# ADR-0007: Camino A (embebido nicho) antes que B/C

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-04
**Contexto**: estrategia de producto. Decisión derivada del análisis de los PDFs estratégicos en [docs/tareas_pendientes/](../tareas_pendientes/).

## 🧭 Contexto

`gabysql` puede madurar en tres direcciones distintas (ver [docs/COMMERCIAL_ROADMAP.md](../COMMERCIAL_ROADMAP.md)):

- **A** — Embebido nicho comercial (SQLite-like). Esfuerzo: 6-12 meses, 1 dev.
- **B** — Cliente-servidor pequeño (tooling interno). Esfuerzo: +18-30 meses, 2-3 devs.
- **C** — RDBMS comercial competitivo (vs Postgres). Esfuerzo: +3-5 años, 4-6 devs senior.

El producto está mantenido hoy por **un solo desarrollador**. El backlog técnico (40+ épicas en `gabysql_roadmap_rdbms.pdf`) no es ejecutable por una persona en plazos razonables.

## 💡 Decisión

**Perseguir el Camino A primero, decidir el Camino B según tracción real con clientes, no perseguir el Camino C sin financiamiento explícito o equipo dedicado.**

Esta decisión rige qué features entran al producto: una feature que pertenece al Camino C (joins, MVCC, wire protocol) **no entra** aunque sea "técnicamente interesante", porque desplazaría las features del Camino A que sí completan el producto vendible más cercano.

## 🔄 Alternativas consideradas

- **Perseguir A + B en paralelo**: rechazado — un solo dev no puede hacer ambos sin sacrificar profundidad en cualquiera.
- **Saltar a C directamente**: rechazado — matemáticamente inviable sin equipo y funding (el propio PDF estratégico lo nota).
- **Pivot a otro producto** (analytics, KV, embedded-only sin SQL): rechazado — se descartan diferenciadores ya construidos (B+Tree, parser, server, índices).
- **Dejar `gabysql` como portafolio sin ambición comercial**: rechazado — el repo ya está más allá de eso; las decisiones tomadas (formato versionado, supply-chain, índices) implican intención de producto.

## 📊 Consecuencias

**Positivas**:
- Foco claro: cada feature pasa la prueba "¿esto refuerza el ICP del Camino A?".
- El backlog corto y ejecutable: 10 bloques de 2-8 semanas cada uno (ver [COMMERCIAL_ROADMAP.md §Camino A](../COMMERCIAL_ROADMAP.md#-camino-a--embebido-nicho-comercial)).
- Diferenciador claro frente a SQLite: zero-deps + Rust safety + supply-chain integrada.
- La decisión es revertible: si en 12 meses el Camino A no gana ni un caso piloto, se replantea.

**Negativas**:
- El producto **no** competirá con Postgres / MySQL / TiDB en el corto plazo. Quien busque eso debe ir a otro motor.
- `JOIN`, `ORDER BY` y `GROUP BY` se posponen detrás de constraints declarativas, `integrity_check` y benchmarks.
- El mantenedor tiene que rechazar features que le interesan técnicamente pero no caben en A.

**Neutras**:
- La arquitectura actual (separación pager / bptree / catalog / index / sql / server) ya está preparada para evolucionar a B sin rewrites profundos. Eso es diseño deliberado, no consecuencia de esta ADR.

## 🔗 Referencias

- Documento de estrategia: [docs/COMMERCIAL_ROADMAP.md](../COMMERCIAL_ROADMAP.md).
- Posicionamiento: [docs/POSITIONING.md](../POSITIONING.md).
- Plan operativo paso a paso: [docs/PLAN_MAESTRO_GABYSQL.md](../PLAN_MAESTRO_GABYSQL.md).
- Análisis ejecutivo de los PDFs: [docs/tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md](../tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md).
