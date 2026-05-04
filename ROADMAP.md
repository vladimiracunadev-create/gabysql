# 🗺️ ROADMAP

> **Dirección técnica de `gabysql`: qué está estable hoy y qué falta para acercarlo a una v1 más seria.**

> **Base estratégica adicional**: ver [docs/tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md](docs/tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md) para la lectura ejecutiva de las proyecciones RDBMS, [docs/PLAN_MAESTRO_GABYSQL.md](docs/PLAN_MAESTRO_GABYSQL.md) para la hoja de ruta paso a paso, y [docs/GABYBENCH_SPEC.md](docs/GABYBENCH_SPEC.md) para la base canónica de benchmark y comparación con otros motores.

---

## 🚦 Estado actual

- Core reescrito en Rust
- Pager con header, páginas fijas y formato en disco **versión `4`**
- Cada página persistida lleva trailer CRC32-IEEE (4 bytes); corrupción se detecta al leer y al replay del WAL
- WAL after-image con replay por `COMMIT` y verificación CRC del payload de cada página
- **B+Tree real** con nodos internos sobre PK `INT`; `root_page` permanece estable cruzando splits
- Catálogo de tablas persistente con hashing FNV-1a-64 (estable entre versiones de Rust)
- **Índices secundarios** sobre una columna escalar (no JSON), con backfill automático y mantenimiento en `INSERT`/`UPDATE`/`DELETE`
- SQL estable: `CREATE DATABASE`, `DROP DATABASE`, `SHOW DATABASES`, `CREATE TABLE`, `INSERT`, `SELECT`, `UPDATE`, `DELETE`, `CREATE INDEX`, `DROP INDEX`, `LIMIT/OFFSET`, `WHERE PK =`, `WHERE PK BETWEEN`, `WHERE col_indexada = val`
- Modelador web `gabymodeler` (vanilla HTML/JS) + admin web `phpgabyadmin`, ambos en `web/`
- Server HTTP/JSON para single DB y multi DB con tope de conexiones simultáneas (default 64, configurable con `-max-connections`)
- `Pager::create` rehúsa sobrescribir un archivo existente; `gabysql init --force` para reset intencional
- Admin web `phpgabyadmin` sobre la API HTTP
- CI en Windows, Linux y macOS
- Docker para validación y despliegue reproducible
- Documentación completa de instalación, operación, seguridad, API y troubleshooting

---

## 🎯 Prioridades antes de llamarlo v1 serio

### Fase 1 — Robustez funcional
- ~~`UPDATE` y `DELETE` por PK~~ ✅ entregado
- ~~checksums por página + WAL~~ ✅ entregado (CRC32-IEEE)
- `NOT NULL`, `DEFAULT` y constraints básicas
- mejor validación de tipos en parser y engine
- crash tests dirigidos (kill -9 entre WAL y file flush)
- comando `integrity_check` que recorra y valide CRCs y la estructura del B+Tree
- mejoras de full scan para tablas medianas
- cobertura adicional en parser, storage y server
- política más clara de compatibilidad del formato en disco (changelog explícito por bump de VERSION)

### Fase 2 — Storage y consulta
- ~~índices secundarios (una columna, equality)~~ ✅ entregado
- ~~`WHERE` por columnas no PK (cuando hay índice)~~ ✅ entregado
- índices compuestos
- índices `UNIQUE` declarativos
- range scan por índice secundario (`WHERE indexed_col BETWEEN ...`)
- `ORDER BY`
- checkpoint/compaction del WAL
- locking simple entre procesos
- backup / restore verificado
- logs estructurados y primeras métricas del server

### Fase 3 — Planeación y rendimiento
- planner básico con stats
- `EXPLAIN`
- mejor layout interno del índice
- benchmarks reproducibles con `gabybench`
- profiling y tuning de scans/rangos
- observabilidad del server más madura
- comparación objetiva con SQLite, PostgreSQL, MySQL/MariaDB y DuckDB

### Fase 4 — Operación de producto
- release process más formal
- empaquetado multiplataforma más simple
- política de compatibilidad de formato en disco
- estrategia de backups y restore automatizable
- endurecimiento adicional del admin web
- authz más serio en modo server

---

## 🚫 Lo que no conviene hacer todavía

- venderlo como reemplazo directo de PostgreSQL/MySQL
- prometer SQL amplio sin planner y sin índices secundarios
- exponerlo a Internet sin proxy, TLS y controles externos
- crecer features antes de consolidar recovery, constraints y compatibilidad del storage
- entrar demasiado pronto en replicación, clustering o wire protocol compatible

---

## 🧠 Dirección recomendada

`gabysql` sigue apuntando a una base embebida tipo SQLite:
- storage local
- archivo único
- superficie SQL pequeña pero robusta
- foco en estabilidad, durabilidad y compatibilidad antes que en amplitud funcional
