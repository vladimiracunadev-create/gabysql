# 🗺️ ROADMAP

> **Dirección técnica de `gabysql`: qué está estable hoy y qué falta para acercarlo a una v1 más seria.**

> **Base estratégica adicional**: ver [docs/tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md](docs/tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md) para la lectura ejecutiva de las proyecciones RDBMS y su priorización aplicada a `gabysql`.

---

## 🚦 Estado actual

- Core reescrito en Rust
- Pager con header, páginas fijas y formato en disco versión `1`
- WAL after-image con replay por `COMMIT`
- Índice persistente de hojas enlazadas por PK `INT`
- Catálogo de tablas persistente
- SQL mínimo estable: `CREATE`, `INSERT`, `SELECT`, `LIMIT/OFFSET`, `WHERE PK =`, `WHERE PK BETWEEN`
- Server HTTP/JSON para single DB y multi DB
- Admin web `phpgabyadmin` sobre la API HTTP
- CI en Windows, Linux y macOS
- Docker para validación y despliegue reproducible
- Documentación completa de instalación, operación, seguridad, API y troubleshooting

---

## 🎯 Prioridades antes de llamarlo v1 serio

### Fase 1 — Robustez funcional
- `UPDATE` y `DELETE` por PK
- `NOT NULL`, `DEFAULT` y constraints básicas
- mejor validación de tipos en parser y engine
- mejoras de full scan para tablas medianas
- cobertura adicional en parser, storage y server
- checksums + crash tests + `integrity_check`
- política más clara de compatibilidad del formato en disco

### Fase 2 — Storage y consulta
- índices secundarios
- `WHERE` por columnas no PK
- `ORDER BY`
- checkpoint/compaction del WAL
- locking simple entre procesos
- backup / restore verificado
- logs estructurados y primeras métricas del server

### Fase 3 — Planeación y rendimiento
- planner básico con stats
- `EXPLAIN`
- mejor layout interno del índice
- benchmarks reproducibles
- profiling y tuning de scans/rangos
- observabilidad del server más madura

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
