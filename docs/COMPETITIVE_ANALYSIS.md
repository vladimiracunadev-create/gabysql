# 🥊 Análisis competitivo de `gabysql`

> # 🏛️ DOCUMENTO HISTÓRICO — NO ES LA AGENDA OPERATIVA
>
> Comparativa contra SQLite/DuckDB/Postgres/MySQL hecha bajo el marco "esto es un producto comercial". El proyecto **no** es un producto y no compite con esos motores en sus ejes. Ver [AGENDA_INVESTIGACION.md](AGENDA_INVESTIGACION.md) para qué es realmente.
>
> Este texto sigue siendo **útil como mapa del mercado de DBs** (entender qué hace cada uno y por qué). Pero **no** es donde `gabysql` aspira a ganar.

---

> **Comparación honesta frente a otros motores de base de datos**: dónde `gabysql` ya gana, dónde pierde, y dónde puede ganar al final de cada camino comercial.
>
> **Regla del documento**: nada de hype. Si `gabysql` no gana en una dimensión hoy, se dice. Si gana solo en un nicho específico, se acota.

---

## 🎯 Para qué sirve este análisis

- **Para decidir si adoptar `gabysql`** — comparar contra el motor que ya usas o estás considerando.
- **Para reclutadores y stakeholders** — entender por qué este producto existe a pesar de SQLite.
- **Para el propio mantenedor** — mantener honesto el roadmap: si en 12 meses el camino A no gana ni siquiera en el nicho identificado, hay que repensar el producto.

---

## 🏟️ Universo de competidores

| Categoría | Motor | Por qué entra al ring |
| :--- | :--- | :--- |
| **Embebido OLTP** | **SQLite** | El competidor directo, el más maduro, lo que todo el mundo asume. |
| Embebido OLAP | **DuckDB** | Para validar que `gabysql` no compite ahí (y no quiere). |
| KV embebido | **LMDB** | El otro extremo: máxima velocidad en KV puro. |
| KV embebido | **RocksDB / sled** | Storage engines puros, sin SQL. |
| Server OLTP | **PostgreSQL** | El estándar para "BD seria". |
| Server OLTP | **MySQL / MariaDB** | El estándar para "BD seria con licencia más amigable". |
| Distribuida | **TiDB / YugabyteDB** | El espacio del Camino C. |
| Multi-modelo | **SurrealDB** | Producto Rust nuevo, comparable en stack. |

Si quieres que agregue un competidor específico a esta tabla (DuckLake, libSQL, Turso, Hermes, etc.), abre un Issue.

---

## 📊 Tabla maestra: dimensiones por motor (estado HOY de `gabysql` v0.1.x)

Leyenda: 🟢 = gana / 🟡 = empate o aceptable / 🔴 = pierde / ⚪ = no aplica

| Dimensión | gabysql HOY | SQLite | DuckDB | LMDB | Postgres | MySQL | SurrealDB |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: |
| **Lenguaje del core** | Rust (safe) 🟢 | C 🔴 | C++ 🔴 | C 🔴 | C 🔴 | C/C++ 🔴 | Rust 🟢 |
| **Dependencias externas** | **Zero** 🟢 | C runtime 🟡 | múltiples libs 🔴 | Zero 🟢 | grande 🔴 | grande 🔴 | varias 🟡 |
| **Tamaño del binario release** | ~3-4 MB 🟢 | ~1.5 MB 🟢 | ~50 MB 🔴 | <1 MB 🟢 | ~50 MB 🔴 | ~250 MB 🔴 | ~30 MB 🟡 |
| **Formato en disco documentado byte por byte** | 🟢 | 🟢 | 🟡 | 🟢 | 🟡 | 🟡 | 🔴 |
| **CRC por página** | 🟢 (CRC32-IEEE) | 🟢 (opcional) | 🟢 | 🔴 (mmap puro) | 🟢 | 🟢 | 🟡 |
| **WAL con verificación de integridad en replay** | 🟢 | 🟢 | ⚪ | ⚪ | 🟢 | 🟢 | 🟡 |
| **Embebido (lib in-process)** | 🟢 | 🟢 | 🟢 | 🟢 | 🔴 | 🔴 | 🟡 |
| **Server HTTP/JSON nativo** | 🟢 | 🔴 | 🔴 | 🔴 | 🔴 | 🔴 | 🟢 |
| **Modelador ER web included (sin npm)** | 🟢 | 🔴 | 🔴 | 🔴 | 🔴 | 🔴 | 🟡 |
| **`CREATE/DROP/SHOW DATABASE`** | 🟢 | 🔴 (un archivo = una DB) | 🟢 | 🔴 | 🟢 | 🟢 | 🟢 |
| **Wire protocol (Postgres/MySQL)** | 🔴 | 🔴 | 🟡 | ⚪ | 🟢 | 🟢 | 🟡 |
| **CRUD básico (INSERT/UPDATE/DELETE)** | 🟢 | 🟢 | 🟢 | ⚪ | 🟢 | 🟢 | 🟢 |
| **Índices secundarios (equality)** | 🟢 | 🟢 | 🟢 | ⚪ | 🟢 | 🟢 | 🟢 |
| **Índices compuestos / UNIQUE** | 🔴 | 🟢 | 🟢 | ⚪ | 🟢 | 🟢 | 🟢 |
| **JOIN** (INNER/CROSS/LEFT/RIGHT/FULL/USING/NATURAL + index-loop) | 🟢 | 🟢 | 🟢 | ⚪ | 🟢 | 🟢 | 🟢 |
| **Subqueries** (`IN`/`=`/`EXISTS` correlacionado) | 🟢 | 🟢 | 🟢 | ⚪ | 🟢 | 🟢 | 🟢 |
| **ORDER BY** | 🟢 | 🟢 | 🟢 | ⚪ | 🟢 | 🟢 | 🟢 |
| **GROUP BY / HAVING / agregados (COUNT/SUM/AVG/MIN/MAX/DISTINCT)** | 🟢 (single-table) | 🟢 | 🟢 | ⚪ | 🟢 | 🟢 | 🟢 |
| **Window functions** | 🔴 | 🟢 | 🟢 | ⚪ | 🟢 | 🟢 | 🟢 |
| **Optimizer cost-based** | 🔴 | 🟡 | 🟢 | ⚪ | 🟢 | 🟢 | 🟡 |
| **MVCC** | 🔴 | 🔴 | 🟢 | 🟢 | 🟢 | 🟢 | 🟢 |
| **Concurrencia (multi-writer)** | 🔴 (mutex) | 🔴 (single writer) | 🟢 | 🟡 | 🟢 | 🟢 | 🟢 |
| **Replicación** | 🔴 | 🔴 (fuera del core) | 🔴 | 🔴 | 🟢 | 🟢 | 🟢 |
| **HA / clustering** | 🔴 | 🔴 | 🔴 | 🔴 | 🟡 | 🟡 | 🟢 |
| **Supply-chain CI builtin del repo** | 🟢 | 🟡 (del integrador) | 🟡 | 🟡 | ⚪ | ⚪ | 🟡 |
| **Tooling de seguridad pinned a SHA + zizmor + grype + cargo-audit** | 🟢 | ⚪ | ⚪ | ⚪ | ⚪ | ⚪ | 🟡 |
| **Licencia permisiva sin contaminación viral** | 🟢 (MIT) | 🟢 (Public Domain) | 🟢 (MIT) | 🟢 (OpenLDAP) | 🟢 (PostgreSQL) | 🟡 (GPL/dual) | 🟡 (BSL) |
| **Madurez** | 🔴 (MVP) | 🟢 (30 años) | 🟡 (4 años) | 🟢 (15+ años) | 🟢 (35+ años) | 🟢 (30 años) | 🟡 (3 años) |

---

## 🎯 Comparativa pareada — el caso que más importa

### vs SQLite — el competidor real

**Cuándo `gabysql` HOY es razonable elegir sobre SQLite:**

- Tu app está escrita en Rust y rechazas FFI a C por **memory safety auditable**.
- Necesitas un **server HTTP/JSON nativo** sobre la BD embebida sin introducir un Postgres.
- Tu equipo valora **supply-chain hardening** integrado al repo del motor (no del integrador).
- Estás haciendo **material educativo** sobre internals de motores.

**Cuándo SQLite sigue siendo la mejor opción:**

- Necesitas SQL amplio (joins, CTE, window functions, FTS, JSON1).
- Necesitas drivers oficiales en 20 lenguajes ya hechos.
- Necesitas confianza de "30 años en producción mundial".
- Tu equipo no es de Rust o no le importa el lenguaje del core.

**Veredicto honesto**: SQLite gana en feature breadth, ecosistema y madurez. `gabysql` gana en zero-deps, Rust safety, y supply-chain integrada.

```rust
// gabysql (Rust nativo, sin FFI)
let mut pager = gabysql::storage::Pager::open("app.db")?;
let mut engine = gabysql::sql::Engine::new(&mut pager);

// SQLite (FFI a libsqlite3)
let conn = rusqlite::Connection::open("app.db")?;  // depends on libsqlite3-sys
```

### vs DuckDB — comparten poco mercado

DuckDB es columnar/analítico/OLAP. `gabysql` es row-oriented/transaccional/OLTP. **No compiten directamente**. Si tu carga es analítica (agregaciones grandes, scans con filtros, joins anchos), DuckDB. Si es OLTP simple (point lookups, CRUD por PK), `gabysql` o SQLite.

### vs LMDB / RocksDB — comparten storage layer, no SQL

LMDB y RocksDB son **storage engines KV**, no BDs SQL. `gabysql` entrega **SQL completo + WAL + CRC + índices secundarios + HTTP/JSON** encima de un B+Tree propio. Si necesitas máxima velocidad KV puro y vas a construir tu propia capa, LMDB. Si quieres SQL out-of-the-box, `gabysql`.

### vs Postgres / MySQL — escalas distintas

Postgres/MySQL son productos server-first con MVCC, replicación, HA, optimizer maduro. `gabysql` no compite con ellos hoy y posiblemente nunca (ver [Camino C](COMMERCIAL_ROADMAP.md#-camino-c--rdbms-comercial-competitivo)). El ICP de `gabysql` empieza donde Postgres es **sobreingeniería**.

### vs SurrealDB — el otro Rust

SurrealDB es multi-modelo (document + graph + relational), distribuido por diseño, con BSL. `gabysql` es estrictamente relacional, single-node, MIT, embebido-first. Comparten lenguaje pero apuntan a mercados muy distintos: SurrealDB compite con MongoDB/Neo4j; `gabysql` compite con SQLite/LMDB.

---

## 📈 Cómo cambia la tabla al final del **Camino A** (≈ 12 meses)

Solo cambian las dimensiones que el camino A entrega:

| Dimensión | HOY | Al final de A |
| :--- | :---: | :---: |
| Índices compuestos / UNIQUE | 🟡 (UNIQUE 🟢, compuestos 🔴) | 🟢 |
| ORDER BY (al menos por índice) | 🟢 | 🟢 |
| Constraints declarativas (NOT NULL/DEFAULT/UNIQUE) | 🔴 | 🟢 |
| `integrity_check` + backup/restore con verificación | 🟢 | 🟢 |
| Suite de benchmarks reproducible (`gabybench`) | 🔴 | 🟢 |
| Drivers oficiales (Python + Node.js) | 🔴 | 🟢 |
| Madurez | 🔴 | 🟡 (`0.5+`, ≥ 1 caso piloto) |

**Después de A**, frente a SQLite, `gabysql` cierra la mayor parte del gap funcional para el ICP definido (apps Rust nativas + edge + tooling interno).

---

## 📈 Cómo cambia al final del **Camino B** (≈ 30 meses adicionales)

| Dimensión | Al final de A | Al final de B |
| :--- | :---: | :---: |
| Concurrencia (multi-writer) | 🔴 | 🟢 (locking por tabla/página) |
| Authz fina por usuario/rol | 🔴 | 🟢 |
| TLS nativo en server | 🔴 | 🟢 |
| Backup online (sin parar el server) | 🔴 | 🟢 |
| Drivers oficiales en 4+ lenguajes | 🟢 (2) | 🟢 (Python, Node, Go, Java, PHP) |
| Madurez | 🟡 | 🟢 (`1.x`, 5+ clientes pagantes) |

**Después de B**, `gabysql` empieza a competir directamente con SQLite-as-server, libSQL/Turso, y el segmento más bajo de Postgres-en-Docker.

---

## 📈 Cómo cambia al final del **Camino C** (≈ 5 años, equipo dedicado)

Todas las celdas que hoy son 🔴 frente a Postgres pasan a 🟢 o 🟡. Pero el competidor también se mueve: Postgres no se queda quieto, y para el momento que `gabysql` tenga MVCC + joins + replicación, Postgres tendrá AIO + columnar + zheap. **No se recomienda planificar este camino sin equipo y financiamiento explícitos**.

---

## 🧪 Ejemplos concretos por escenario

### Escenario 1 — App de escritorio Rust con catálogo local de 100k filas

```rust
// gabysql
let mut pager = gabysql::storage::Pager::open("catalog.db")?;
// 4 MB binario extra al ejecutable, sin libsqlite3-sys.

// SQLite
let conn = rusqlite::Connection::open("catalog.db")?;
// + libsqlite3-sys (C, ~1.5 MB) + bindings (~150 KB)
```

**Veredicto**: empate técnico hoy. `gabysql` gana si auditas el binario completo en Rust importa para tu compliance.

### Escenario 2 — Tooling interno con 10 usuarios concurrentes

```bash
# gabysql HOY
gabysql-server -dir ./dbs -addr :8080 -max-connections 64
# Mutex global de proceso para escrituras → no escala más allá de ~50 req/s en write

# Postgres
docker run -d -e POSTGRES_PASSWORD=... -p 5432:5432 postgres:16
# MVCC + connection pool → miles de req/s
```

**Veredicto HOY**: Postgres gana claramente. **Después del Camino B**, `gabysql` cierra el gap para este caso.

### Escenario 3 — Microservicio edge en Raspberry Pi 4

```bash
# gabysql
docker pull gabysql:latest        # ~30 MB
docker run --rm -p 8080:8080 -v ./data:/data gabysql

# SQLite (con su HTTP wrapper hand-rolled)
# necesitas escribirlo tú o adoptar libSQL/Turso
```

**Veredicto**: `gabysql` gana hoy si el edge necesita HTTP/JSON listo. SQLite sin HTTP queda fuera.

### Escenario 4 — Workload analítico (SUM/AVG/JOIN sobre 50M filas)

```sql
-- DuckDB
SELECT region, AVG(price)
FROM sales
JOIN regions ON sales.region_id = regions.id
GROUP BY region;
-- 100ms con vectorización + columnar
```

**Veredicto**: DuckDB siempre gana. `gabysql` no compite ni competirá en OLAP.

---

## 🧠 Resumen ejecutivo: ¿cuándo elegir `gabysql`?

> Hoy, sin caminar A todavía, `gabysql` es razonable si **(1) tu app es Rust nativa**, **(2) rechazas C en el core por compliance o gusto técnico**, **(3) tu workload es OLTP relacional clásico** (lookups por PK/índice, JOINs equi-predicado, subqueries `IN`/`EXISTS`, reporting básico con `GROUP BY`/`HAVING`/agregados single-table desde el bloque F), **(4) valoras supply-chain integrada** y **(5) podés vivir sin window functions / CTE recursivas / agregados sobre JOIN todavía**.

Si rompes alguno de esos cinco puntos, hay un competidor mejor: SQLite, DuckDB, Postgres o SurrealDB según el caso.

Eso no es un defecto del producto — es la consecuencia honesta de que `gabysql` es un **MVP de un solo desarrollador con disciplina, no una alternativa enterprise**. La pregunta correcta no es "¿es mejor que Postgres?" — la pregunta correcta es "¿en qué nicho específico vale más este producto que sus alternativas, y cómo lo expandimos?". El [Camino A](COMMERCIAL_ROADMAP.md) responde a esa segunda pregunta.
