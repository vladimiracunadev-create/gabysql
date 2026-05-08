# 🗺️ ROADMAP

> **Dirección técnica de `gabysql`: qué está estable hoy y qué falta para acercarlo a una v1 más seria.**

> **Base estratégica adicional**: ver [docs/tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md](docs/tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md) para la lectura ejecutiva de las proyecciones RDBMS, [docs/PLAN_MAESTRO_GABYSQL.md](docs/PLAN_MAESTRO_GABYSQL.md) para la hoja de ruta paso a paso, y [docs/GABYBENCH_SPEC.md](docs/GABYBENCH_SPEC.md) para la base canónica de benchmark y comparación con otros motores.

---

## 🚦 Estado actual

- Core reescrito en Rust
- Pager con header, páginas fijas y formato en disco **versión `6`**
- Cada página persistida lleva trailer CRC32-IEEE (4 bytes); corrupción se detecta al leer y al replay del WAL
- WAL after-image con replay por `COMMIT` y verificación CRC del payload de cada página
- **B+Tree real** con nodos internos sobre PK `INT`; `root_page` permanece estable cruzando splits
- Catálogo de tablas persistente con hashing FNV-1a-64 (estable entre versiones de Rust)
- **Índices secundarios** sobre una columna escalar (no JSON), con backfill automático y mantenimiento en `INSERT`/`UPDATE`/`DELETE`
- SQL estable: `CREATE DATABASE`, `DROP DATABASE`, `SHOW DATABASES`, `CREATE TABLE` (con `NOT NULL` / `DEFAULT` / `UNIQUE` / `REFERENCES ... [ON DELETE RESTRICT|CASCADE]` inline), `DROP TABLE [IF EXISTS]`, `ALTER TABLE ADD [COLUMN] <coldef>`, `INSERT`, `SELECT`, `UPDATE`, `DELETE` (con cascade), `CREATE INDEX`, `CREATE UNIQUE INDEX`, `DROP INDEX`, `LIMIT/OFFSET`, `WHERE PK =`, `WHERE PK BETWEEN`, `WHERE col_indexada = val`
- Modelador web `gabymodeler` (vanilla HTML/JS) + admin web `phpgabyadmin`, ambos en `web/`
- Server HTTP/JSON para single DB y multi DB con tope de conexiones simultáneas (default 64, configurable con `-max-connections`)
- `Pager::create` rehúsa sobrescribir un archivo existente; `gabysql init --force` para reset intencional
- Admin web `phpgabyadmin` sobre la API HTTP
- CI en Windows, Linux y macOS
- Docker para validación y despliegue reproducible
- Documentación completa de instalación, operación, seguridad, API y troubleshooting

---

## 🎯 Prioridades antes de llamarlo v1 serio

### Fase 1 — Robustez funcional ✅ ENTREGADA
- ~~`UPDATE` y `DELETE` por PK~~ ✅ entregado
- ~~checksums por página + WAL~~ ✅ entregado (CRC32-IEEE)
- ~~`NOT NULL`, `DEFAULT` y `UNIQUE` declarativos~~ ✅ entregado (VERSION 5)
- ~~`FOREIGN KEY` declarativas + enforced~~ ✅ entregado (VERSION 6)
- ~~mejor validación de tipos en parser y engine~~ ✅ entregado (identificadores duros + reserved words + DEFAULT/FK type checks)
- ~~comando `INTEGRITY CHECK` que recorra y valide CRCs y la estructura del B+Tree~~ ✅ entregado
- ~~política más clara de compatibilidad del formato en disco (changelog explícito por bump de VERSION)~~ ✅ entregado (1 bloque = 1 push a `main`, CHANGELOG entry por intervención)
- ~~crash tests dirigidos (kill -9 entre WAL y file flush)~~ ✅ entregado (3 escenarios sintéticos en `tests/integration_test.rs`)
- mejoras de full scan para tablas medianas — pendiente

### Fase 2 — Storage y consulta
- ~~índices secundarios (una columna, equality)~~ ✅ entregado
- ~~`WHERE` por columnas no PK (cuando hay índice)~~ ✅ entregado
- ~~índices `UNIQUE` declarativos~~ ✅ entregado (VERSION 5)
- ~~`FOREIGN KEY` declarativas + enforced~~ ✅ entregado (VERSION 6)
- ~~`ORDER BY <col> [ASC|DESC]`~~ ✅ entregado
- ~~`LeafCursor` lazy para `SELECT … LIMIT N` (O(N+offset) en vez de O(table))~~ ✅ entregado (ADR-0008)
- ~~`PageCache` LRU acotado (memoria del server bounded)~~ ✅ entregado (ADR-0009)
- índices compuestos
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

### Fase 5 — AI-native (apertura)

> **Tesis**: el consumidor que crece más rápido en el ecosistema es el agente LLM. Hoy cualquier integración con `gabysql` desde un agente requiere pegamento manual (cliente HTTP + token + schema metido en el prompt). Esta fase elimina ese pegamento sin tocar el motor. Justificación y diseño detallados en [ADR-0010](docs/adr/0010-mcp-gateway.md).

- ~~**Gateway MCP** (`gabysql-mcp`) como **binario adaptador separado** sobre el HTTP/JSON existente~~ ✅ entregado (ADR-0010)
  - tools: `gabysql_list_databases`, `gabysql_describe_database`, `gabysql_query`, `gabysql_execute` (omitido en `--read-only`), `gabysql_integrity_check`
  - resources: `gabysql://catalog`, `gabysql://schema/{db}`
  - **cero deps externas** (decisión final superó el objetivo del ADR — JSON parser, JSON-RPC y cliente HTTP/1.1 implementados a mano en el binario; ADR-0001 intacto)
  - reusa authz (bearer token), `write_lock` y rate-limit del server tal cual
- ~~**Búsqueda semántica** (top-k vectorial sobre columnas TEXT con array JSON)~~ ✅ entregada via gateway (ADR-0011)
  - tool `gabysql_vector_search` con métricas cosine/euclidean/dot, top-k configurable
  - **cero bump de formato** — los vectores son `TEXT`, el cómputo ocurre en `gabysql-mcp` en Rust
  - condiciones de salida hacia un `VECTOR(n)` nativo documentadas en el ADR (>100K vectores, demanda de operadores SQL, índice ANN)
- ~~**Audit log enriquecido** (cada escritura registra el agente, el "por qué" semántico y el clientInfo además del SQL)~~ ✅ entregado en el gateway (ADR-0012)
  - flag `--audit-log <path>` (también `GABYSQL_AUDIT_LOG`); opt-in, sin overhead si no se activa
  - captura `clientInfo` de `initialize` + argumento `reason` opcional en `gabysql_execute`
  - tool `gabysql_audit_tail(n)` para que el propio agente revise sus acciones
  - JSONL append-only, procesable con `jq`/`tail`/ingest a S3/ELK
  - **cero impacto en el motor** — sin bump de formato, sin tocar `storage.rs`/`bptree.rs`/`sql.rs`/`catalog.rs`/`server.rs`

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
