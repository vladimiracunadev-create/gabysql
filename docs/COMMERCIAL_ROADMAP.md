# 💼 Hoja de ruta comercial de `gabysql`

> # 🏛️ DOCUMENTO HISTÓRICO — NO ES LA AGENDA OPERATIVA
>
> Este documento fue un **ejercicio mental** de pensar `gabysql` como producto comercial: tres caminos A/B/C, ICPs, comparativas, etc. La conclusión honesta tras escribirlo y operar el proyecto durante varios meses es que **el proyecto no es comercial y no apunta a serlo**.
>
> La agenda real del proyecto vive en **[AGENDA_INVESTIGACION.md](AGENDA_INVESTIGACION.md)**: un marco de aprendizaje + exploración sobre "cómo se vería una DB nativa de la era de los agentes LLM". No hay clientes, no hay validación externa, y eso está bien — el objetivo es entender bases de datos a fondo, no shipear un producto.
>
> Este texto queda en el repo como artefacto histórico: ayuda a entender qué partes valen la pena pulir técnicamente. **No tomes decisiones basadas en este documento.** Si la pregunta es "¿qué hago después?", la respuesta está en `AGENDA_INVESTIGACION.md`.

---

> **Tres caminos posibles para llevar `gabysql` desde "MVP funcional" a "producto comercial defendible". Documento estratégico — para decidir, no para ejecutar paso a paso (eso es el [PLAN_MAESTRO_GABYSQL.md](PLAN_MAESTRO_GABYSQL.md)).**

---

## 🎯 La pregunta que este documento responde

> *"¿Qué tan lejos está `gabysql` de ser una base de datos realmente comercial, y cuál es el camino concreto?"*

La respuesta corta: **"comercial" no es un punto único, son tres niveles de ambición distintos**, con costos, mercados y riesgos muy diferentes. Este documento desarrolla cada uno con el detalle necesario para tomar una decisión informada.

Está basado en los dos documentos estratégicos de [docs/tareas_pendientes/](tareas_pendientes/):
- [`gabysql_roadmap_rdbms.pdf`](tareas_pendientes/gabysql_roadmap_rdbms.pdf) — backlog técnico de 40+ épicas para llegar a un RDBMS completo.
- [`tabla_desafios_bases_datos_priorizada_gabysql.pdf`](tareas_pendientes/tabla_desafios_bases_datos_priorizada_gabysql.pdf) — los 20 desafíos típicos de BDs, scoreados por impacto/frecuencia/dificultad.

Y en el [análisis ejecutivo](tareas_pendientes/ANALISIS_PROYECCIONES_GABYSQL.md) que aterriza esos PDFs en fases reales.

---

## 📊 Resumen ejecutivo de los tres caminos

| Camino | Nombre corto | Mercado | Esfuerzo restante | Equipo | Riesgo |
| :---: | :--- | :--- | :---: | :---: | :---: |
| 🟢 **A** | Embebido nicho comercial | apps de escritorio, edge, IoT, embebidos en otros productos | **6–12 meses** | 1 dev | 🟢 Bajo |
| 🟡 **B** | Cliente-servidor pequeño | dashboards internos, prototipos de empresa, herramientas internas | **+18–30 meses** | 2–3 devs | 🟡 Medio |
| 🔴 **C** | RDBMS comercial competitivo | reemplazar Postgres/MySQL en producción | **+3–5 años** | 4–6 devs senior | 🔴 Alto |

> **Recomendación del producto**: perseguir **A** primero, decidir **B** según tracción real con clientes, **no perseguir C** sin financiamiento explícito o equipo dedicado.

---

## 🟢 Camino A — Embebido nicho comercial

> **El único camino viable para un solo desarrollador**. Compite con SQLite en el nicho de aplicaciones Rust nativas que rechazan FFI a C.

### 🎯 Mercado objetivo

- **Aplicaciones de escritorio en Rust** (Tauri, egui, Iced) que necesitan persistencia local y prefieren no enlazar libsqlite3.
- **Edge / IoT** con un solo proceso escribiendo, formato auditable, footprint pequeño.
- **Productos integrados** (CMS, ERPs verticales, herramientas administrativas) donde "abrir un Postgres completo" sería sobreingeniería.
- **Material educativo** sobre internals de motores de BD (SO, universidades, bootcamps).

### 💰 Modelo de negocio realista

| Modelo | Viabilidad |
| :--- | :--- |
| Licencia comercial dual MIT + comercial para empresas que requieran indemnification | 🟢 Realista (licencia dual estilo SQLite vs SQLite Encryption Extension) |
| SaaS hosting de gabysql como BD embebida | 🔴 No tiene sentido — el embedded no se hostea |
| Soporte profesional (incident response, custom features) | 🟡 Realista cuando haya 3-5 clientes con dolor real |
| Servicios de capacitación / consultoría sobre internals | 🟢 Realista como producto educativo paralelo |

### 🔬 Diferenciador honesto vs SQLite

| Dimensión | SQLite | gabysql (al final del camino A) |
| :--- | :--- | :--- |
| Lenguaje | C | Rust 100% (audita memory safety) |
| Dependencias | C runtime + amalgamation | **Zero** (solo std de Rust) |
| Formato en disco | versionado, compatible hacia atrás | versionado, **rechazo explícito** entre versiones (deliberado: prefiere fallar a corromper) |
| WAL | journaling + WAL opcional | After-image WAL con CRC32 por record, replay verificado |
| Crash recovery | bien probado, 30+ años | crash tests dirigidos en CI |
| SQL | muy amplio | subset minimalista pero completo en su scope |
| Concurrencia | multi-reader / single-writer | mismo modelo |
| Tooling de seguridad supply-chain | solo del integrador | integrado al repo (cargo audit/deny, grype, zizmor, pin-check) |
| Casos de uso ofensivos | "todo el mundo lo usa" | "lo entiendo byte por byte" |

### 📋 Lo que falta entregar para A

| # | Bloque | Entregable concreto | Esfuerzo (1 dev) |
| :---: | :--- | :--- | :---: |
| 1 | ✅ Índices secundarios | `CREATE INDEX`, `DROP INDEX`, `WHERE col_indexada = val` | **Entregado** |
| 1b | ✅ DDL de DATABASE | `CREATE DATABASE [IF NOT EXISTS]`, `DROP DATABASE [IF EXISTS]`, `SHOW DATABASES` despachados por server/CLI | **Entregado** |
| 1c | ✅ Modelador web `gabymodeler` | Single-page HTML+JS vanilla; ER drag&drop → DDL gabysql → phpgabyadmin | **Entregado** |
| 2 | ✅ Constraints declarativas | `NOT NULL`, `UNIQUE` (single + multi-col), `DEFAULT`, `CHECK` (L2/L3), FK actions completas + ON UPDATE, named constraints + `DROP CONSTRAINT`, FK multi-col, vistas lógicas | **Entregado** (2026-05-27) |
| 3 | `ORDER BY` por columna indexada | descender por el chain `next` del leaf con la dirección correcta | 3–5 sem |
| 4 | `integrity_check` operacional | comando `gabysql integrity-check <db>` que recorre B+Tree, valida CRCs, detecta huérfanos | 2–3 sem |
| 5 | Backup / restore formal | `gabysql backup <db> <out>` con verificación post-restore | 2–4 sem |
| 6 | Suite `gabybench` ejecutable | scripts reproducibles para insert batch, point lookup, range scan, full scan; thresholds en CI | 2–4 sem |
| 7 | Tipos temporales con UTC normalizada | `DATETIME` en epoch UTC + funciones básicas (`NOW`, `DATE_ADD`) | 2–3 sem |
| 8 | Crate Rust embebido publicado en crates.io | API estable, semver, docs.rs | 1–2 sem |
| 9 | Drivers oficiales mínimos | Python, Node.js sobre HTTP (no wire protocol todavía) | 2–4 sem |
| 10 | 1 caso piloto real con feedback | un usuario externo en producción, retro escrita | 4–8 sem |

**Total estimado: 24–42 semanas (6–10 meses) de un dev senior full-time.**

### ✅ Criterios GO/NO-GO para considerar A "comercial"

- [ ] Una empresa o desarrollador externo lo usa en producción y publica un caso (testimonio o blog post).
- [ ] `gabybench` corre en CI con thresholds que rompen el build ante regresión > 10%.
- [ ] El crate está publicado en crates.io con `0.x` semver y al menos un release menor.
- [ ] Existen drivers oficiales Python + Node.js documentados.
- [ ] No hay ningún `unsafe` no justificado ni dependencia opcional sin ADR.
- [ ] La documentación incluye tutorial de migración desde SQLite (los 5 patterns típicos).

### ⚠️ Riesgos del camino A

| Riesgo | Mitigación |
| :--- | :--- |
| "Nadie escoge un motor desconocido si SQLite funciona" | Posicionar el diferenciador (zero-deps + Rust safety + auditabilidad) y enfocar el ICP, no competir en feature breadth. |
| El mantenedor pierde foco y empieza a perseguir Camino C | Política de "no se acepta feature de Q/T/N sin ADR" definida en [docs/POSITIONING.md](POSITIONING.md). |
| Bumps de formato en disco erosionan la confianza | Cada bump documentado + herramienta `migrate` cuando justifique el costo. |

---

## 🟡 Camino B — Cliente-servidor pequeño

> **Requiere equipo. No es viable para un solo dev en plazos razonables.** Compite con SQLite-as-server o SurrealDB Lite para casos de uso de tooling interno.

### 🎯 Mercado objetivo

- **Tooling interno** de empresas medianas (dashboards, ETL ligero, herramientas de soporte) que no quieren mantener un Postgres.
- **Prototipos de empresa** que necesitan API HTTP + auth + multi-tenancy mínima.
- **Apps multi-cliente** donde el server vive en el mismo edge que el cliente.

### 📋 Suma a A

| # | Bloque | Entregable | Esfuerzo (equipo 2-3) |
| :---: | :--- | :--- | :---: |
| B-1 | Locking por tabla / página | reemplazar el mutex global por lock manager con granularidad | 8–12 sem |
| B-2 | Connection pool real + statement cache | clientes reutilizan parsed AST | 4–6 sem |
| B-3 | Usuarios / roles / permisos por DB y tabla | reemplazar el token único por authz fina | 8–12 sem |
| B-4 | Audit log estructurado | tabla interna `__audit` con eventos de DDL y login | 4–6 sem |
| B-5 | TLS nativo en el server | `rustls` con crate-explícito (única excepción a "zero deps") | 3–5 sem |
| B-6 | Logs estructurados + `/metrics` Prometheus | hand-rolled o crate auditado | 4–8 sem |
| B-7 | Backup online (snapshot + WAL streaming) | sin parar el server | 6–10 sem |
| B-8 | Drivers oficiales en Python, Node, Go, Java, PHP | wrappers HTTP + tests de compat | 6–12 sem |

**Total: +24–40 semanas adicionales sobre A** (1.5–3 años con un equipo de 2-3 devs).

### 💰 Modelo de negocio en B

| Modelo | Viabilidad |
| :--- | :--- |
| **Licencia BSL** (Business Source License) | 🟢 Realista — gratis para uso no-prod, comercial paga |
| **SaaS hosted single-tenant** | 🟡 Realista si el ICP lo pide (gabysql Cloud) |
| **Soporte enterprise** | 🟢 Realista con 5-10 clientes |

### ✅ Criterios GO/NO-GO

- [ ] 3+ clientes pagantes con renewal year-1 ≥ 80%.
- [ ] Locking por tabla pasa benchmark con 100 conexiones concurrentes.
- [ ] Backup online verificado en una DB de 10 GB+.
- [ ] Drivers oficiales en al menos 4 lenguajes con CI propia.

### ⚠️ Riesgos del camino B

| Riesgo | Mitigación |
| :--- | :--- |
| Competencia con Postgres baja: ¿por qué no usar Postgres? | El ICP es claro: empresas que necesitan algo mucho más simple, embebible, single-binary. |
| El equipo no escala: 2-3 devs es exactamente el momento más frágil | Definir bloques B-N independientes antes de contratar. |

---

## 🔴 Camino C — RDBMS comercial competitivo

> **Realista solo con financiamiento o un equipo dedicado de 4-6 ingenieros senior por 3-5 años.** Compite con Postgres, MySQL, TiDB, YugabyteDB, SurrealDB.

### 🎯 Mercado objetivo

- Reemplazar Postgres / MySQL en producción.
- BDs distribuidas con replicación, HA, sharding.
- Apps con SQL amplio (joins, subqueries, CTEs, window functions).

### 📋 Suma a B

| # | Bloque | Entregable | Esfuerzo (equipo 4-6) |
| :---: | :--- | :--- | :---: |
| C-1 | Optimizer cost-based con stats | analyze, selectivity, join order | 10–18 sem |
| C-2 | Joins (nested loop + hash + merge) | con tests de TPC-H subset | 10–18 sem |
| C-3 | Subqueries, CTEs, window functions | parser + executor avanzado | 10–18 sem |
| C-4 | MVCC | snapshot isolation real, vacuum | 14–26 sem |
| C-5 | Replicación basada en WAL streaming | follower + failover manual | 12–24 sem |
| C-6 | Wire protocol Postgres / MySQL | que clientes existentes funcionen sin cambios | 12–24 sem |
| C-7 | TPC-C-lite + TPC-H-lite con targets | benchmarking continuo en CI | 8–16 sem |
| C-8 | HA con leader election, quorum, split-brain handling | runbook + chaos tests | 16–30 sem |

**Total: +120–200 semanas adicionales** (≥ 3 años con 4-6 devs senior).

### 💰 Modelo de negocio en C

| Modelo | Viabilidad |
| :--- | :--- |
| **Open core + cloud SaaS** | 🟢 Es el modelo de Yugabyte, TiDB, ClickHouse |
| **Licenciamiento perpetuo** | 🔴 Murió como modelo en BDs |
| **Marketplace integrations** | 🟢 con AWS/GCP/Azure si el producto es maduro |

### ✅ Criterios GO/NO-GO para C

- [ ] Financiamiento Serie A o equivalente (≥ USD 5M para 18 meses).
- [ ] Equipo de **al menos** 4 devs senior con experiencia en motores de BDs.
- [ ] Customer development con 10+ candidatos a cliente design partner antes de comprometerse.
- [ ] El producto en estado B tiene tracción comercial real.

### ⚠️ Riesgos del camino C

| Riesgo | Mitigación |
| :--- | :--- |
| Postgres es free, open-source, maduro y tiene 30 años de inversión | gabysql necesita un diferenciador claro distinto de "Postgres pero más nuevo". |
| El esfuerzo es matemáticamente incompatible con un solo dev | No iniciar sin el equipo y el funding. |
| El mercado de BDs distribuidas está saturado | Validar antes de codear. |

---

## 🧪 Cómo se ve cada camino en código (ejemplos concretos)

### Lo que `gabysql` ya hace hoy (estado actual del MVP)

```sql
-- Setup
CREATE TABLE users (id INT PRIMARY KEY, name TEXT, score FLOAT);
INSERT INTO users (id, name, score) VALUES (1, 'Ana', 9.5);
INSERT INTO users (id, name, score) VALUES (2, 'Beto', 7.0);

-- Lookup por PK (existe)
SELECT * FROM users WHERE id = 1;

-- Range por PK (existe)
SELECT * FROM users WHERE id BETWEEN 1 AND 100 LIMIT 25;

-- DML completo (existe)
UPDATE users SET name = 'Ana M' WHERE id = 1;
DELETE FROM users WHERE id = 2;

-- Índices secundarios (existe)
CREATE INDEX idx_users_name ON users (name);
SELECT * FROM users WHERE name = 'Ana M';   -- usa el índice
DROP INDEX idx_users_name;
```

### Lo que `gabysql` debe hacer al final del Camino A

```sql
-- Constraints declarativas
CREATE TABLE orders (
  id INT PRIMARY KEY,
  customer_id INT NOT NULL,
  email TEXT UNIQUE,
  status TEXT NOT NULL DEFAULT 'pending',
  created_at DATETIME DEFAULT NOW()
);

-- ORDER BY por índice secundario
CREATE INDEX idx_orders_status ON orders (status);
SELECT id, customer_id FROM orders
  WHERE status = 'pending'
  ORDER BY created_at DESC
  LIMIT 50;

-- integrity_check operacional
$ gabysql integrity-check orders.db
ok pages=128 leaves=42 internals=3 indexes=2 crcs=verified

-- Backup verificado
$ gabysql backup orders.db backup-2026-05-04.gabybak
$ gabysql restore backup-2026-05-04.gabybak verified.db
ok rows-restored=12450
```

### Lo que `gabysql` debe hacer al final del Camino B

```bash
# Locking por tabla — concurrencia real
$ ab -n 10000 -c 100 -p insert.json -T application/json \
     -H 'X-Gabysql-Token: secret' \
     http://localhost:8080/exec
# requests/sec mucho mayor que con mutex global

# Authz fina
$ gabysql-cli grant SELECT ON orders TO analyst@local
$ gabysql-cli grant ALL    ON orders TO admin@local

# /metrics Prometheus
$ curl http://localhost:8080/metrics | grep gabysql
gabysql_requests_total{op="select",status="200"} 12450
gabysql_index_lookups_total{table="orders",index="idx_orders_status"} 8201
gabysql_wal_replay_seconds_count 3
```

### Lo que `gabysql` debe hacer al final del Camino C

```sql
-- Joins reales
SELECT u.name, COUNT(o.id) AS pedidos
FROM users u
LEFT JOIN orders o ON o.customer_id = u.id
WHERE o.created_at >= '2026-01-01'
GROUP BY u.name
ORDER BY pedidos DESC
LIMIT 10;

-- EXPLAIN de un plan real
EXPLAIN SELECT * FROM orders WHERE status = 'pending' ORDER BY created_at;
-- Index Scan using idx_orders_status (cost=0.4..125.6 rows=84)
--   Sort: created_at desc

-- Wire protocol Postgres
$ psql "host=localhost port=8432 dbname=orders user=admin"
psql (16.x, server gabysql 1.0)
orders=> SELECT * FROM users LIMIT 1;
```

---

## 🧭 Decisión recomendada hoy

> **Camino A, paso a paso, sin distracciones.**

Es el único que:
- es ejecutable por un dev senior solo,
- compite en un nicho real (no en el océano rojo de "el próximo Postgres"),
- mantiene el diferenciador que ya está en el código (zero-deps, Rust-safety, auditabilidad),
- y permite reevaluar B con datos reales en 12 meses.

El paso 1 concreto del Camino A ya quedó entregado: **índices secundarios + `WHERE col_indexada = val`** (ver [CHANGELOG.md](../CHANGELOG.md) entrada 2026-05-04).

El paso 2 sugerido: **constraints declarativas (`NOT NULL`, `UNIQUE`, `DEFAULT`)**. Sin esto, ningún caso de uso serio puede confiar el schema al motor.
