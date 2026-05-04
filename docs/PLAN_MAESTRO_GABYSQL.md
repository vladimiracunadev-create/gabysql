# 🧭 PLAN MAESTRO DE IMPLEMENTACIÓN DE `gabysql`

> **Objetivo**: convertir `gabysql` en un motor embebido serio, paso a paso, sin romper lo que ya funciona y con controles explícitos de error, revisión, despliegue y benchmarking.

> **Base estratégica**: este plan se apoya en [ANALISIS_PROYECCIONES_GABYSQL.md](tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md) y en la dirección general definida en [../ROADMAP.md](../ROADMAP.md).

---

## 🎯 Resultado esperado

Al terminar este plan, `gabysql` debería haber pasado de:
- motor MVP usable

a:
- motor embebido técnicamente defendible
- con formato en disco estable
- recovery más robusto
- constraints y operaciones básicas completas
- índices secundarios
- observabilidad mínima
- tooling de backup, integridad y benchmarks
- comparación objetiva contra otros motores

---

## 🧱 Principios que no se negocian

### 1. No romper lo que ya existe
Cada fase debe preservar:
- apertura de `.db` válidas existentes
- `CREATE`, `INSERT`, `SELECT`, `LIMIT/OFFSET`, `WHERE PK`
- server HTTP y `phpgabyadmin`
- Docker y CI

### 2. Todo cambio de storage debe tener guardrails
Si cambia:
- formato en disco
- WAL
- catálogo
- serialización de filas
- índice persistente

entonces debe venir acompañado de:
- pruebas nuevas
- estrategia de compatibilidad
- herramienta de verificación o migración
- rollback claro de release

### 3. Primero confiabilidad, luego amplitud
El orden correcto es:
1. durability
2. integrity
3. correctness
4. operability
5. performance
6. nuevas features SQL

### 4. Benchmark sin narrativa engañosa
`gabysql` debe compararse contra otros motores, pero con honestidad:
- no fingir equivalencia con PostgreSQL/MySQL como producto total
- comparar cargas y alcances equivalentes
- documentar dónde `gabysql` compite y dónde todavía no

---

## 🗺️ Fases de implementación

## Fase 0 — Línea base y control de daño

### Objetivo
Congelar una base confiable antes de tocar componentes críticos.

### Entregables
- matriz explícita de comportamiento actual
- suite de regresión base
- golden outputs de CLI y HTTP
- baseline de tiempos de ejecución iniciales

### Trabajo
- inventariar comandos y outputs esperados del CLI
- inventariar endpoints y respuestas JSON base
- registrar tamaño de DB, WAL y tiempos sobre cargas simples
- fijar la base canónica de prueba `gabybench`

### Gates de no-ruptura
- `cargo check --tests`
- `cargo clippy --all-targets -- -D warnings`
- `docker build -t gabysql .`
- smoke HTTP y `phpgabyadmin`

### Tiempo estimado
- 3 a 5 días

---

## Fase 1 — Integridad del storage y recovery

> **Estado**: parcialmente entregada (2026-05-03). Ver [CHANGELOG](../CHANGELOG.md) para los hitos. Falta `integrity_check` operacional y crash tests dirigidos.

### Objetivo
Endurecer el corazón del motor sin ampliar demasiado la superficie SQL.

### Alcance
- ~~versionado formal del formato en disco~~ ✅ entregado (`VERSION = 3`, rechazo explícito)
- ~~checksums por página o frame WAL~~ ✅ entregado (CRC32-IEEE en trailer de cada página + verificación en replay)
- ~~B+Tree multinivel real~~ ✅ entregado (LEAF + INTERNAL, root estable)
- ~~hashing del catálogo estable~~ ✅ entregado (FNV-1a-64)
- crash tests (kill -9 entre WAL y file flush) — pendiente
- `integrity_check` (recorrido completo, validación de CRCs y de invariantes del B+Tree) — pendiente
- mejor política de compatibilidad de archivos — pendiente (hoy: rechazo explícito sin migración)

### Trabajo paso a paso
1. Especificar v1 formal del file format
2. añadir checksum en páginas/WAL
3. implementar verificación de integridad
4. crear pruebas de crash-replay
5. documentar compatibilidad y errores esperables

### Riesgo principal
Corromper compatibilidad de archivos o recovery.

### Mitigación
- mantener lector backward-compatible cuando sea posible
- si no lo es, introducir número de versión y rechazo explícito con mensaje claro
- no mezclar esta fase con nuevas features SQL grandes

### Revisión
- revisión obligatoria de storage/WAL
- diff de formato documentado
- prueba de apertura de DB antigua + DB nueva

### Deploy
- release menor controlado
- nota explícita de compatibilidad del formato
- rollback: volver a binario anterior y bloquear apertura de archivos nuevos si cambia versión

### Tiempo estimado
- 2 a 4 semanas

---

## Fase 2 — Correctitud funcional básica

> **Estado**: arrancada (2026-05-03). `UPDATE` y `DELETE` por PK ya están en main; faltan constraints declarativas.

### Objetivo
Completar las operaciones esenciales del motor y dejar reglas de datos más serias.

### Alcance
- ~~`UPDATE` por PK~~ ✅ entregado
- ~~`DELETE` por PK~~ ✅ entregado
- `NOT NULL` — pendiente
- `DEFAULT` — pendiente
- `UNIQUE` — pendiente
- casts y semántica de `NULL` más claras — pendiente

### Trabajo paso a paso
1. implementar constraints en catálogo
2. extender parser y engine con `UPDATE`/`DELETE`
3. validar reglas de `NULL` y defaults
4. ampliar tests de errores y edge cases
5. ajustar API y admin web si hace falta

### Gates de no-regresión
- todos los tests anteriores
- pruebas nuevas de constraints
- pruebas HTTP para errores y éxito
- smoke manual de `phpgabyadmin`

### Revisión
- foco en regresiones semánticas
- mensajes de error claros y consistentes
- revisar que rollback siga funcionando

### Deploy
- release menor
- changelog explícito de nuevas semánticas

### Tiempo estimado
- 3 a 5 semanas

---

## Fase 3 — Índices y consultas usables

> **Estado**: arrancada (2026-05-04). Índices secundarios simples + `WHERE` por columna no-PK ya están en main; quedan los compuestos y `ORDER BY`.

### Objetivo
Quitar a la PK la carga de ser la única vía eficiente de consulta.

### Alcance
- ~~índices secundarios simples~~ ✅ entregado (una columna, equality, backfill, mantenimiento en INSERT/UPDATE/DELETE)
- ~~`WHERE` por columnas no PK~~ ✅ entregado (cuando la columna tiene índice)
- índices compuestos
- `UNIQUE` declarativo (índice + constraint)
- range scan por índice secundario (`WHERE col_indexada BETWEEN ...`)
- `ORDER BY` básico

### Trabajo paso a paso
1. definir metadata de índices en catálogo
2. implementar create/drop de índices
3. mantener índices en `INSERT`/`UPDATE`/`DELETE`
4. usar índice secundario en filtros simples
5. añadir `ORDER BY` inicial donde aplique

### Riesgo principal
Desalineación entre índice primario, secundarios y filas reales.

### Mitigación
- verificador de invariantes
- tests de consistency check tras operaciones mixtas
- benchmarks antes/después

### Revisión
- revisión de mantenimiento incremental de índices
- pruebas de consistency e invariantes

### Deploy
- feature release controlado
- documentación clara de qué filtros usan índice y cuáles no

### Tiempo estimado
- 4 a 8 semanas

---

## Fase 4 — Operación seria y tooling

### Objetivo
Volver a `gabysql` operable y diagnosticable fuera del laboratorio.

### Alcance
- backup/restore formal
- `integrity_check`
- `vacuum` o compaction manual inicial
- logs estructurados
- métricas mínimas del server
- runbooks operativos más ricos

### Trabajo paso a paso
1. comando de backup offline verificado
2. comando de restore y validación
3. log estructurado en `gabysql-server`
4. métricas básicas: latencia, errores, counts
5. documentación de operación y recuperación

### Gates
- restore exitoso de DB canónica
- benchmark y smoke luego de restore
- verificación de integridad posterior

### Tiempo estimado
- 3 a 5 semanas

---

## Fase 5 — Rendimiento, explain y planificación básica

### Objetivo
Empezar a explicar y optimizar consultas en vez de solo ejecutarlas.

### Alcance
- benchmarks reproducibles
- `EXPLAIN`
- estadísticas básicas
- planner por reglas
- profiling de hot paths

### Trabajo paso a paso
1. cerrar suite `gabybench`
2. medir point lookup, range scan, insert batch, full scan, sort simple
3. añadir `EXPLAIN` textual/JSON
4. introducir stats básicas por tabla/índice
5. planner por reglas antes de CBO

### Revisión
- separar cambios de optimizer de cambios de storage
- exigir resultados reproducibles
- comparar plan y latencia antes/después

### Tiempo estimado
- 4 a 8 semanas

---

## Fase 6 — Server más fuerte, sin sobredimensionarlo

### Objetivo
Hacer que el modo server sea más serio sin convertirlo todavía en un Postgres alternativo.

### Alcance
- límites de conexiones
- authz más serio
- auditoría básica
- defaults seguros de operación
- mejores perfiles `dev` / `prod`

### Tiempo estimado
- 3 a 6 semanas

---

## 🚫 Fuera de foco temprano

No deben entrar antes de cerrar bien las fases 1 a 5:
- MVCC completo
- wire protocol compatible con PostgreSQL/MySQL
- replicación
- clustering / HA
- sharding
- multi-tenancy seria

---

## 🧪 Control de errores y no-regresión

## Pirámide de validación

### Nivel 1 — Correctitud local
- unit tests
- integration tests
- parser error cases
- WAL/crash tests
- integrity checks

### Nivel 2 — Compatibilidad del producto
- golden tests de CLI
- golden tests de JSON HTTP
- apertura de DBs previas compatibles
- backup/restore de DB canónica

### Nivel 3 — Operación real
- Docker build
- Docker compose smoke
- health checks
- pruebas del admin web

### Nivel 4 — Performance y regresión
- baseline de tiempos
- thresholds por comando crítico
- comparación contra motores de referencia

---

## 👀 Proceso de revisión por cambio

Todo cambio significativo debe pasar por esta lista:

1. ¿rompe formato en disco o recovery?
2. ¿rompe SQL existente?
3. ¿cambia errores visibles al usuario?
4. ¿cambia respuesta de API o admin web?
5. ¿hay prueba nueva que cubra el cambio?
6. ¿hay actualización documental?
7. ¿hay benchmark si toca hot paths?

---

## 🚀 Estrategia de despliegue

## Tipos de release

| Tipo | Cuándo usarlo | Qué exige |
|---|---|---|
| patch | bug fix sin cambio de formato | tests + smoke |
| minor | nuevas features compatibles | tests + docs + benchmarks focales |
| minor con formato | cambio de file format controlado | compat policy + migración o rechazo explícito |
| experimental | features grandes aún inmaduras | bandera o rama separada |

## Política recomendada
- no mezclar cambios de storage profundos con features SQL grandes en el mismo release
- si cambia formato, documentarlo en portada, changelog y release notes
- mantener Docker como validación obligatoria del release

---

## ⏱️ Medición de tiempos de comandos

## Objetivo
No basta con “funciona”; hay que medir cuánto tarda y detectar regresiones.

## Windows / PowerShell
Usar `Measure-Command`:

```powershell
Measure-Command { cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users;" }
```

Para extraer milisegundos:

```powershell
$time = Measure-Command { cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users;" }
$time.TotalMilliseconds
```

## Linux / macOS
Usar `/usr/bin/time`:

```bash
/usr/bin/time -f "%E real, %M KB" cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users;"
```

## Qué medir siempre
- `init`
- `info`
- batch `INSERT`
- point lookup por PK
- range scan
- full scan paginado
- `backup` / `restore` cuando existan
- `integrity_check` cuando exista
- endpoints HTTP equivalentes

---

## 🗃️ Base de datos canónica de prueba: `gabybench`

> **Objetivo**: tener una DB de referencia fija para validar funcionalidad, regresión y comparación con otros motores.

La especificación detallada vive en [GABYBENCH_SPEC.md](GABYBENCH_SPEC.md).

### Debe incluir
- tablas relacionales simples
- mezcla de lookup por PK, rango, filtro por columna secundaria y scans
- timestamps/fechas
- texto variable
- volumen pequeño, mediano y grande

### Escalas recomendadas
- `S`: ~10k filas principales
- `M`: ~100k filas principales
- `L`: ~1M filas principales cuando el motor ya soporte bien esa escala

---

## 🥊 Comparación con otros motores

## Motores de referencia recomendados

| Motor | Rol en la comparación |
|---|---|
| SQLite | baseline principal de motor embebido |
| PostgreSQL | referencia OLTP server madura |
| MySQL/MariaDB | referencia server tradicional |
| DuckDB | referencia analítica / scans |

## Reglas de comparación
- comparar el mismo schema lógico cuando sea posible
- comparar el mismo dataset `gabybench`
- comparar las mismas consultas y operaciones
- documentar diferencias de capacidad, no solo tiempos
- separar benchmarks OLTP de benchmarks más analíticos

## Qué debe mostrar la comparación
- dónde `gabysql` ya es razonable
- dónde todavía queda lejos
- qué mejora produjo cada fase del roadmap

---

## 📅 Orden de ejecución recomendado

1. Fase 0 — baseline, golden tests y `gabybench`
2. Fase 1 — formato, WAL, checksums, crash tests
3. Fase 2 — `UPDATE`, `DELETE`, constraints
4. Fase 3 — índices secundarios y consultas más útiles
5. Fase 4 — backup/restore, logs, métricas, tooling
6. Fase 5 — benchmarks, `EXPLAIN`, planner básico
7. Fase 6 — endurecimiento del modo server

---

## ✅ Criterio de éxito por etapa

Una fase solo se considera terminada si cumple las cuatro cosas:
- **funciona**
- **no rompe lo previo**
- **queda medida**
- **queda documentada**

Ese es el criterio correcto para que `gabysql` crezca con seriedad y no solo con features.
