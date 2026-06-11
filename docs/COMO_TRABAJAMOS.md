# 🧭 Cómo se actualiza y mejora `gabysql` en el tiempo

> **Para qué sirve este doc**: cuando un commit dice "feat(R6): post-lookup
> bucket size check" o cuando en una conversación aparece "ahora R1 está
> cerrado", entender qué significan **R6** y **R1** sin tener que abrir
> 5 archivos. Este doc explica el sistema completo: cómo nacen los bloques
> de trabajo, cómo se nombran, cómo se entregan, y dónde mirar después
> para no perderse.
>
> **Quién lo lee**: yo cuando vuelva al proyecto después de un tiempo,
> Claude cuando lo invoque en otra sesión, cualquier persona que entre al
> repo y quiera entender qué pasa.

---

## 1. Las letras (P, R, M, F, …) son taquigrafía, no jerga oficial

Cada "bloque de trabajo" tiene una letra + número (ej: **P5b**, **R6**,
**M2**). Esa letra **no es nomenclatura de la industria** de bases de
datos — es shorthand que inventamos en algún momento de una sesión
para poder hablar de cada cosa por nombre corto. Funciona como un
ticket-ID interno.

| Prefijo | Significa | Para qué se usa |
|---|---|---|
| **P** | Planning / **P**lanner | Bloques relacionados al optimizer: `EXPLAIN`, `ANALYZE`, stats por columna, cost-based decisions, JOIN algorithm choice. |
| **R** | **R**eparación | Cosas que un análisis identificó como rotas o frágiles después de una sprint. Origen típico: alguien escribió un doc de "qué quedó débil" tras un push grande y enumeró las reparaciones. |
| **M** | **M**ejora | Cosas que **no están rotas** pero que sumarían. Origen mismo que R, pero con menos urgencia. |
| **F** | **F**eature SQL (cobertura) | `WHERE`, agregados, etc. Los F2/F3 de 2026-05-30 son ejemplos. |
| **E** | **E**xpresiones / dispatch del WHERE | E1 (comparadores), E2 (operadores E3VL), E3 (PK lookup), E5 (bare SELECT). |
| **G** | Funciones escalares (string/num/fecha) | G1, G2, G3. |
| **H** | Subqueries (derived, correlated, IN/EXISTS, scalar) | Bloque H 2026-05-26. |
| **I** | Set ops + VALUES | `UNION`, `INTERSECT`, `EXCEPT`, `VALUES (..)`. |
| **J** | DML masivo + `INSERT ... SELECT` | J + J2. |
| **K** | DDL extendido | PK compuesta, índices compuestos, CTAS, UNIQUE multi-col. |
| **L** | Constraints declarativas | `CHECK`, FK multi-col, named constraints. |
| **N** | Casos chicos sin categoría propia | N5 = `DEFAULT gen_random_uuid()`. |
| **T** | Transactions | TCL básico. |
| **V** | **V**istas lógicas | `CREATE VIEW`. |
| **W** | CTEs + **W**indow functions | W1 (CTE), W2 (recursive), W3 (window), W4 (window O(n)). |
| **X** | Triggers + procedures + PL/pgSQL | X1 → X6. |
| **Y** | Tipos extendidos | DECIMAL, BLOB, UUID, TIME, UNSIGNED. |
| **Z** | Seguridad SQL-level | Users, roles, GRANT/REVOKE, RLS. |

**Si un commit usa una letra que no está en esta tabla**: es nueva, y
quien la inventó debería actualizar este doc.

---

## 2. Cómo nace un bloque

Un "bloque" puede salir de uno de varios lugares. **Esto es importante
para entender por qué algo está en la lista de pendientes**:

| Origen | Ejemplo | Resultado |
|---|---|---|
| Una conversación interactiva donde el usuario pide algo nuevo | "extendé EXPLAIN" → P1, P2, P3 | El bloque nace en la sesión y se entrega como un push. |
| Un gap encontrado por el bench | Gap 10 del ADR-0066 → P5b | El gap queda catalogado en un ADR; el fix viene después como bloque dedicado. |
| Un análisis post-sprint que enumera lo que quedó débil | ANALISIS_POST_P5 enumeró 9 tensiones → R1, R4, R6, R8 cerraron 4 de ellas | El análisis vive en `docs/ANALISIS_POST_P5.md`; las R/M son los items que el análisis nombró. |
| Una recomendación técnica de TAREAS_PENDIENTES | Tarea 3 (fuzz + property tests) | Históricos del proyecto, esperan a que alguien las agarre. |

---

## 3. Cómo se entrega un bloque (regla de oro: 1 bloque = 1 push)

```
idea
  ↓
plan corto (acá charlamos antes de codear; identifica scope + riesgos)
  ↓
código (src/) + tests (tests/integration_test.rs) en una sola rama
  ↓
verificación local:
  - cargo fmt --check
  - cargo clippy --all-targets -- -D warnings
  - cargo test --all-targets   (vía Docker si no hay MSVC local)
  ↓
ADR si bumpea VERSION o cambia un plan (docs/adr/NNNN-*.md)
  ↓
update de docs vivos: STATUS.md, CHANGELOG.md, otros si pega
  ↓
commit con mensaje descriptivo (incluye qué hace, por qué, y lo
honestamente abierto)
  ↓
push a main
  ↓
CI verde en 5 jobs (rust×3 OS + docker + bench + php)
  ↓
si CI rojo: hot-fix en commit separado, no amend
```

**Por qué 1 bloque = 1 push**: cada push está bien aislado, fácil de
revertir, fácil de buscar en `git log` por qué se hizo algo. Bachear
cambios en commits gigantes es un anti-patrón conocido.

---

## 4. Dónde mirar para no perderse

Estos son los **5 archivos que cubren el 90% de las preguntas**:

| Pregunta | Mirar acá | Por qué |
|---|---|---|
| "¿Qué pasó en el último push?" | [CHANGELOG.md](../CHANGELOG.md) | Entradas formales, una por bloque entregado. La entrada más reciente arriba. |
| "¿Qué funciona hoy?" | [STATUS.md](STATUS.md) | Tabla de madurez por subsistema (🟢 / 🟡 / 🔴). Snapshot al día de la última sesión. |
| "¿Por qué se decidió X?" | [docs/adr/](adr/) | Cada decisión arquitectónica tiene su ADR con contexto + alternativas + consecuencias. Numerados 0001 → N. |
| "¿Qué falta?" | [TAREAS_PENDIENTES.md](TAREAS_PENDIENTES.md) | Lista priorizada de lo próximo a hacer. **Es el primer doc que se abre al pedir "estado del proyecto"**. |
| "¿Qué quedó débil tras la última sprint grande?" | [ANALISIS_POST_P5.md](ANALISIS_POST_P5.md) | Diagnóstico crítico después de la sesión 2026-06-10/11. Vive ahí porque puede actualizarse después de cada sprint mayor con otro análisis paralelo. |

**Cuando aparezca otro análisis grande**, vivirá como `ANALISIS_POST_NN.md`
en `docs/`. La idea es no tirar el viejo — sirve para entender qué se
estaba pensando en ese momento.

---

## 5. Cómo leer un commit / ADR / referencia en una conversación

Ejemplo: el commit dice `feat(R6): post-lookup bucket size check refina P5c sobre composite`.

Descomposición:

- **`feat(R6)`**: es un feature nuevo, label interno **R6**.
- **R6** → busca en `ANALISIS_POST_P5.md` o `TAREAS_PENDIENTES.md`,
  está enumerado ahí.
- **`refina P5c`**: depende del bloque P5c (lookup el ADR de P5c en
  `docs/adr/` o el CHANGELOG para entender qué era P5c).
- **`composite`**: contexto del motor — composite secondary index.

Si en una conversación aparece "ahora R1 está cerrado por ADR-0074":

- **R1** → el item 1 de la lista de reparaciones del análisis.
- **ADR-0074** → el archivo `docs/adr/0074-r1-stats-stale-detection.md`,
  con el detalle completo de qué se hizo y por qué.

---

## 6. Cuando algo deja de tener sentido

Si una recomendación que está en `TAREAS_PENDIENTES.md` deja de tener
sentido (cambió el contexto, otra cosa la reemplazó, el usuario decidió
otra cosa), **no se borra silencioso**. Se mueve a una sección
"💤 archivadas" con una línea de por qué.

El por-qué del veto es información útil después — "no, esto no se
hizo porque…" evita que la siguiente sesión vuelva a proponerlo.

---

## 7. Convenciones específicas de `gabysql`

Algunas reglas heredadas que aplican a este repo (vienen de la memoria
de sesiones anteriores):

- **1 bloque = 1 push a `main`**. No batch.
- **Cuando hay autorización standing**, el push va sin pedir confirmación.
- **Antes de declarar "OK"** un cambio: hay que pegar el output del
  comando que lo prueba. Decir "tests verdes" sin mostrar el `test
  result: ok. N passed; 0 failed` es desconfiable.
- **Validar queries del bench contra el motor ANTES de medir**. No
  asumir que el SQL del bench es compatible.
- **Matar zombies antes de procesos largos** (`Get-Process gabybench |
  Stop-Process -Force` al inicio de cada sesión).
- **Chequear invariantes de docs antes de commit** (VERSION, test count,
  claims de "X está pendiente" que ya cerró).

---

## 8. Si te perdés leyendo un texto largo

Tres preguntas que siempre tienen respuesta rápida:

| Pregunta | Respuesta |
|---|---|
| "¿Qué prefijo es ese?" | Tabla del §1 de este doc. |
| "¿En qué estado está ese item?" | `TAREAS_PENDIENTES.md` para abiertos, `CHANGELOG.md` para cerrados. |
| "¿Por qué se decidió así?" | `docs/adr/` con el número que aparezca en el commit o conversación. |

Y si **nada de esto aclara**: pegame un grito y reescribo en castellano
sin códigos. Ese es el contrato.
