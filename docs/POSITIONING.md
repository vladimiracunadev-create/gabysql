# 🎯 Posicionamiento de `gabysql`

> # 🏛️ DOCUMENTO HISTÓRICO — NO ES LA AGENDA OPERATIVA
>
> Este documento fue el intento de posicionar `gabysql` como producto comercial (ICPs, casos de uso, "a quién le sirve"). La conclusión tras operar el proyecto es que **`gabysql` no es un producto; es un proyecto de aprendizaje + exploración**. No tiene ICP porque no tiene clientes y no apunta a tenerlos.
>
> La declaración real de qué es el proyecto vive en **[AGENDA_INVESTIGACION.md](AGENDA_INVESTIGACION.md)**.
>
> Este texto queda como artefacto histórico para entender de dónde viene el proyecto. **No lo uses para tomar decisiones técnicas o de scope.**

---

> **Por qué existe este motor, qué problema resuelve y a quién le sirve.** Documento ancla: cuando dudes si una feature pertenece al producto, vuelve aquí.

---

## 💡 Idea central

`gabysql` es **un motor de base de datos embebido escrito 100% en Rust con cero dependencias externas**, diseñado como un producto base auditable: cada capa (storage, WAL, B+Tree, índices, SQL, server HTTP) está implementada y entendida por el mantenedor, no compuesta a partir de crates de terceros.

Esto no es una decisión estética. Es la **propuesta de valor** que lo separa de SQLite (C, no audita Rust safety), de DuckDB (C++, columnar, otro caso de uso), y de cualquier wrapper de un motor existente.

---

## 🧭 Línea profesional que representa

`gabysql` se alinea con una historia técnica más amplia centrada en:

- ingeniería de sistemas sin atajos: storage durable, formato en disco versionado con rechazo explícito, recovery por WAL con CRC32.
- disciplina de releases: cada bump de formato es bisagra documentada en `CHANGELOG`, `TECHNICAL_SPECS` y `COMPATIBILITY`.
- supply-chain seria desde el día uno: `cargo audit`, `cargo deny`, `detect-secrets`, `grype`, `actionlint`, `zizmor`, `pin-check`.
- honestidad de madurez: el producto se vende como lo que es hoy (motor embebido MVP serio), no como lo que aspira a ser en 3 años.

Ver el contexto extenso en [RECRUITER.md](../RECRUITER.md) y la postura completa en [docs/SECURITY_LAYERS.md](SECURITY_LAYERS.md).

---

## 🔥 Qué problema resuelve hoy

| Problema real | Cómo lo aborda `gabysql` |
| :--- | :--- |
| **"Necesito una BD embebida en Rust nativa, sin enlazar libsqlite ni un FFI a C"** | El binario es 100% Rust safe + el subset de `unsafe` que la stdlib ya verifica. Sin C, sin libsqlite3, sin FFI. |
| **"Quiero un motor cuyo formato en disco entienda completo, byte por byte"** | El formato está documentado en [TECHNICAL_SPECS.md](TECHNICAL_SPECS.md): header de 24 B + páginas de 4096 B con trailer CRC32, leaf y internal pages con layout explícito. Cero magia. |
| **"Necesito poder demostrar disciplina de seguridad en un repositorio reproducible"** | CI multi-OS + 4 workflows (`ci`, `security`, `workflow-security`, `stale`) cubren cargo-audit, cargo-deny, detect-secrets, Trojan Source, grype, actionlint, zizmor, pin-check. Toda acción third-party pinneada a SHA. |
| **"Quiero una API HTTP/JSON simple sobre una DB embebida, sin desplegar un Postgres"** | `gabysql-server` es un binario de 736 líneas hand-rolled, con token, cap de conexiones, multi-DB. Sin frameworks. |
| **"Necesito una base canónica para enseñar internals de DB engines"** | Cada módulo (~200-800 líneas) cubre un concepto: pager, WAL, B+Tree, catalog, índice secundario, SQL parser. Útil como material didáctico real. |

---

## 🚫 Qué problemas NO resuelve hoy (y posiblemente nunca)

Antes de invertir tiempo o de adoptarlo:

- ❌ **Reemplazo directo de PostgreSQL/MySQL** — sin MVCC, sin planner cost-based, sin replicación, sin window functions ni CTE recursivas. Ver [docs/COMMERCIAL_ROADMAP.md](COMMERCIAL_ROADMAP.md) para la diferencia entre los caminos A/B/C.
- ❌ **Workloads analíticos OLAP** — el motor es row-oriented, sin compresión columnar. Para eso está DuckDB.
- ❌ **Concurrencia masiva** — un solo mutex de proceso para escrituras; lecturas concurrentes sin MVCC.
- ❌ **Window functions, CTE recursivas, vistas materializadas** — no implementadas. Reporting básico SÍ funciona desde el bloque F (`GROUP BY`/`HAVING`/`COUNT`/`SUM`/`AVG`/`MIN`/`MAX`/`DISTINCT` single-table; sobre `JOIN` aún devuelve `[GBY-4028]`).
- ❌ **Compatibilidad wire-protocol con Postgres/MySQL** — los clientes tienen que hablar HTTP/JSON o usar el crate embebido.

> **Lo que ya entrega hoy en SQL relacional clásico:** `JOIN` completo (INNER, CROSS, LEFT/RIGHT/FULL [OUTER], USING, NATURAL, multi-tabla, self-join), `WHERE col IN/= (SELECT ...)`, `WHERE [NOT] EXISTS (SELECT ...)` correlacionada, `ORDER BY ASC/DESC`, `LIMIT/OFFSET`, índices secundarios (hash + INT-ordered con `BETWEEN`), `FOREIGN KEY` con `ON DELETE`, constraints declarativas. Ver [docs/SQL_REFERENCE.md](SQL_REFERENCE.md) para la gramática completa.

---

## 👥 ICP — perfiles para los que `gabysql` es razonable hoy

| Perfil | Por qué aplica |
| :--- | :--- |
| **Aplicación de escritorio en Rust** que necesita persistencia local sin enlazar libsqlite3 | Cero deps + binario único + crate embebible |
| **Edge / IoT** con un solo proceso escribiendo, varios leyendo (read-mostly) | Footprint pequeño, formato auditable, CRC32 anti-corrupción |
| **Backend interno** donde "abrir una DB completa" sería sobreingeniería | API HTTP/JSON + admin web en un `docker compose up` |
| **Material didáctico de internals de DB** | Código pequeño, módulos ortogonales, formato en disco documentado, tests integrales reales |
| **Portafolio técnico** que quiere demostrar ingeniería de sistemas sin atajos | El propio repo es la evidencia |

## 👥 Para quién NO sirve hoy

| Perfil | Razón |
| :--- | :--- |
| Equipos buscando reemplazar Postgres en producción | Faltan MVCC, joins, planner, replicación — años de trabajo |
| Workloads analíticos | Sin columnar, sin vectorización, sin spill-to-disk |
| Apps con concurrencia alta de escritura | Mutex global por proceso |
| Apps que dependen de drivers oficiales para 5 lenguajes | Solo HTTP/JSON + crate Rust |
| Producción crítica sin tolerancia a "won't fix" CVEs en la base Debian | Migrar a `gcr.io/distroless/cc` está en el roadmap, no entregado |

---

## 🧱 Cómo se decide qué entra y qué no

Una feature entra al producto si pasa estas tres pruebas:

1. **¿Refuerza el ICP definido arriba?** Si no — al ROADMAP, no al sprint actual.
2. **¿Mantiene la regla de cero dependencias externas?** Si rompe esa regla, requiere un ADR explícito en [docs/adr/](adr/).
3. **¿Se entrega con tests de integración + documentación + posible bump del formato en disco?** Si no, queda en draft.

Esa tercera pregunta es la que mata la mayoría de las "buenas ideas". Es deliberado.

---

## 🔭 Tres caminos posibles, una decisión por hacer

`gabysql` puede madurar en tres direcciones distintas, cada una con costo y mercado distintos. Esto está desarrollado en profundidad en [docs/COMMERCIAL_ROADMAP.md](COMMERCIAL_ROADMAP.md):

- **Camino A — Embebido nicho comercial** (el realista para 1 dev): SQLite-like + zero-deps + Rust-safety. Esfuerzo aprox. 6–12 meses.
- **Camino B — Cliente-servidor pequeño**: requiere equipo. Esfuerzo aprox. 18–30 meses adicionales.
- **Camino C — RDBMS comercial competitivo**: requiere financiamiento o equipo dedicado. 3–5 años.

La recomendación actual del producto es perseguir **A primero**, decidir **B según tracción**, y **no perseguir C** sin financiamiento explícito.

---

## 🧪 Ejemplos de uso reales

### Ejemplo 1 — App de escritorio Rust con catálogo local

```rust
// Cargo.toml
// [dependencies]
// gabysql = { path = "../gabysql" }   # crate embebido, zero deps externas

use gabysql::storage::Pager;
use gabysql::sql::{parse, Engine};

fn main() -> gabysql::DbResult<()> {
    let mut pager = Pager::open("catalog.db")?;
    pager.begin()?;
    let stmts = parse(
        "CREATE TABLE products (
           id INT PRIMARY KEY,
           name TEXT,
           price FLOAT,
           in_stock BOOL
         );
         CREATE INDEX idx_products_name ON products (name);"
    )?;
    let mut engine = Engine::new(&mut pager);
    for stmt in stmts { engine.exec(stmt)?; }
    pager.commit()
}
```

### Ejemplo 2 — Microservicio interno con HTTP/JSON

```bash
# Levantar el server contra una carpeta de DBs
gabysql-server -dir ./dbs -addr :8080 -token "$TOKEN" -max-connections 32 &

# Crear DB y tabla desde curl (auth via header)
curl -s -X POST http://localhost:8080/dbs \
  -H "X-Gabysql-Token: $TOKEN" \
  -d '{"db":"orders.db"}'

curl -s -X POST http://localhost:8080/exec \
  -H "X-Gabysql-Token: $TOKEN" \
  -H 'content-type: application/json' \
  -d '{
    "db":"orders.db",
    "sql":"CREATE TABLE orders (id INT PRIMARY KEY, customer TEXT, status TEXT);
           CREATE INDEX idx_orders_status ON orders (status);"
  }'

# Insertar y consultar por columna indexada (no PK)
curl -s -X POST http://localhost:8080/exec \
  -H "X-Gabysql-Token: $TOKEN" \
  -H 'content-type: application/json' \
  -d '{
    "db":"orders.db",
    "sql":"INSERT INTO orders (id,customer,status) VALUES (1,'\''Ana'\'','\''pending'\'');
           SELECT * FROM orders WHERE status = '\''pending'\'';"
  }'
```

### Ejemplo 3 — Catálogo de productos en edge (Raspberry Pi / IoT)

```bash
# Imagen multi-stage final ~30 MB; un solo binario, sin deps de runtime.
docker build -t gabysql .
docker run --rm -p 8080:8080 -v ./data:/data gabysql

# El dispositivo edge solo necesita:
#  - el binario gabysql-server
#  - un volumen persistente para el .db
#  - cualquier cliente HTTP (incluso curl en BusyBox)
```

### Ejemplo 4 — Material didáctico: ver el formato en disco

```bash
gabysql init demo.db
gabysql exec demo.db "CREATE TABLE u (id INT PRIMARY KEY, name TEXT);"

# Inspeccionar los primeros bytes del header.
xxd -l 32 demo.db
# 00000000: 4741 4259 5351 4c31 0700 0000 0010 0000  GABYSQL1........
#                                ^^^^^^^^^ versión = 7
#                                          ^^^^ page_size = 4096
```

Cada uno de esos 32 bytes está documentado en [TECHNICAL_SPECS.md §Header](TECHNICAL_SPECS.md). Útil para enseñar cómo se diseña un formato en disco real.

### Ejemplo 5 — Modelador web (`gabymodeler`) → SQL → phpgabyadmin

```bash
docker compose up -d --build
# Modelador:   http://localhost:8000/modeler/
# Admin web:   http://localhost:8000/phpgabyadmin/
```

En el modelador: drag&drop entidades + columnas + flags (PK / idx). `Exportar SQL` produce `CREATE DATABASE`, `CREATE TABLE` y `CREATE INDEX` listos para pegar en `phpgabyadmin → tab SQL`. Single-page HTML+JS vanilla, cero deps, persistencia en `localStorage`.

### Ejemplo 6 — phpgabyadmin como banco de pruebas visual

```bash
docker compose up -d --build
# Abrir http://localhost:8000/phpgabyadmin/
#  - Pestaña Browse: paginación, export/import CSV.
#  - Pestaña Structure: lista columnas, marca cuáles están indexadas,
#    lista índices secundarios con botón DROP, formulario CREATE INDEX.
#  - Pestaña SQL: snippets con un click para SELECT/UPDATE/DELETE/
#    CREATE INDEX/DROP INDEX precargados sobre la tabla seleccionada.
```

Ver detalle en [USER_MANUAL.md §5](../USER_MANUAL.md).

---

## ⚖️ Comparativa con otros motores

Ver [docs/COMPETITIVE_ANALYSIS.md](COMPETITIVE_ANALYSIS.md) para el análisis dimensión por dimensión vs SQLite, DuckDB, LMDB, RocksDB, Postgres y MySQL.
