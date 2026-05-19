# 🧪 Casos de uso de `gabysql`

> **Recetas concretas listas para copiar.** Cada caso responde a un escenario real, con CLI / HTTP / Rust embebido según corresponda. Si tu caso no aparece aquí, abre un Issue.

---

## 🗂️ Índice de casos

| # | Caso | Ruta principal |
| :---: | :--- | :--- |
| 1 | [Catálogo local en una app de escritorio Rust (Tauri/egui)](#1-catalogo-local-en-app-rust) | crate embebido |
| 2 | [Microservicio interno con HTTP/JSON + token](#2-microservicio-http-json) | `gabysql-server` |
| 3 | [Edge / IoT en Raspberry Pi 4](#3-edge-iot) | Docker |
| 4 | [Importar CSV a una tabla nueva](#4-importar-csv) | `phpgabyadmin` o curl |
| 5 | [Mantenimiento en vivo de índices secundarios](#5-mantenimiento-de-indices) | SQL |
| 6 | [Backup / restore manual reproducible](#6-backup-restore-manual) | shell |
| 7 | [Multi-DB con un solo server (`-dir`)](#7-multi-db-con--dir) | `gabysql-server` |
| 8 | [Cliente Python contra la API HTTP](#8-cliente-python) | Python |
| 9 | [Cliente Node.js contra la API HTTP](#9-cliente-nodejs) | Node.js |
| 10 | [Detectar corrupción intencionalmente y verificar el CRC32](#10-corrupcion-y-crc) | shell + xxd |
| 11 | [Modelador web → DDL → ejecutar en phpgabyadmin](#11-modelador-web--ddl--ejecutar-en-phpgabyadmin-zero-sql-para-empezar) | navegador (sin SQL) |
| 12 | [`CREATE/DROP/SHOW DATABASE` desde la API](#12-create--drop--show-database-desde-la-api) | curl |
| 13 | [Stress test rápido (inserts/segundo)](#13-stress-test-rapido-medir-insertssegundo) | python + CLI |
| 14 | [Query exploratoria con phpgabyadmin (zero-SQL)](#14-query-exploratoria-con-phpgabyadmin-zero-sql) | navegador |
| 15 | [Validar schema antes de migrar](#15-validar-schema-antes-de-migrar-smoke-pre-deploy) | shell |
| 16 | [Smoke test de release](#16-smoke-test-de-un-release-recien-buildeado) | shell |
| 17 | [Comparativa side-by-side con SQLite](#17-comparativa-side-by-side-con-sqlite) | shell |
| 18 | [Auditar imagen Docker antes de desplegar](#18-auditar-la-imagen-docker-antes-de-desplegarla) | grype |
| 19 | [Rotar token del server (blue-green)](#19-generar-credenciales-y-rotar-token-del-server) | shell |

---

## 1. Catálogo local en app Rust

**Escenario**: app de escritorio en Tauri/egui que necesita persistir 100k filas, sin libsqlite3 ni FFI a C.

```toml
# Cargo.toml de tu app
[dependencies]
gabysql = { path = "../gabysql" }   # o git, o crates.io cuando se publique
```

```rust
use gabysql::storage::Pager;
use gabysql::sql::{parse, Engine, Value};

fn open_or_create(path: &str) -> gabysql::DbResult<Pager> {
    if std::path::Path::new(path).exists() {
        Pager::open(path)
    } else {
        let mut p = Pager::create(path)?;
        p.close()?;
        let mut p = Pager::open(path)?;
        p.begin()?;
        let mut engine = Engine::new(&mut p);
        for stmt in parse(
            "CREATE TABLE products (
               id INT PRIMARY KEY, sku TEXT, name TEXT, price FLOAT
             );
             CREATE INDEX idx_products_sku ON products (sku);"
        )? { engine.exec(stmt)?; }
        p.commit()?;
        Pager::open(path)
    }
}

fn find_by_sku(pager: &mut Pager, sku: &str) -> gabysql::DbResult<Vec<Vec<Value>>> {
    pager.begin()?;
    let mut engine = Engine::new(pager);
    let stmt = parse(&format!("SELECT id, name, price FROM products WHERE sku = '{}';", sku))?
        .into_iter().next().unwrap();
    let rs = engine.exec(stmt)?;
    pager.commit()?;
    Ok(rs.rows)
}
```

> Por seguridad, cuando construyas SQL con valores externos, valida con un regex `[A-Za-z0-9_-]+` antes de interpolar (ver [`web/phpgabyadmin/index.php`](../web/phpgabyadmin/index.php) para el patrón). Hoy no hay prepared statements; eso queda en [Camino A](COMMERCIAL_ROADMAP.md).

---

## 2. Microservicio HTTP/JSON

**Escenario**: backend interno que expone una BD a varios clientes con auth de token.

```bash
# Levantar (multi-DB en un directorio, con token y techo de conexiones)
gabysql-server -dir ./data -addr :8080 -token "$TOKEN" -max-connections 32 &

# Crear DB
curl -s -X POST http://localhost:8080/dbs \
  -H "X-Gabysql-Token: $TOKEN" -d '{"db":"orders.db"}'

# Schema + índice
curl -s -X POST http://localhost:8080/exec \
  -H "X-Gabysql-Token: $TOKEN" \
  -H 'content-type: application/json' \
  -d @- <<'JSON'
{"db":"orders.db","sql":"
  CREATE TABLE orders (
    id INT PRIMARY KEY,
    customer TEXT,
    status TEXT,
    total FLOAT
  );
  CREATE INDEX idx_orders_status ON orders (status);
"}
JSON

# Bulk insert + consulta por índice
curl -s -X POST http://localhost:8080/exec \
  -H "X-Gabysql-Token: $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"db":"orders.db","sql":"
    INSERT INTO orders (id,customer,status,total) VALUES (1,'\''Ana'\'','\''pending'\'',99.5);
    INSERT INTO orders (id,customer,status,total) VALUES (2,'\''Beto'\'','\''paid'\'',45.0);
    INSERT INTO orders (id,customer,status,total) VALUES (3,'\''Caro'\'','\''pending'\'',12.0);
    SELECT * FROM orders WHERE status = '\''pending'\'';
  "}'
```

Errores típicos (todos retornan JSON con `ok:false`):
- `401 unauthorized` → falta el header `X-Gabysql-Token`.
- `503 server busy` → llegaste al `-max-connections`. Reintenta con backoff.
- `400 fila no existe: PK=N` → `UPDATE`/`DELETE` sobre PK inexistente.

---

## 3. Edge / IoT

**Escenario**: Raspberry Pi 4 con un único proceso lector/escritor.

```bash
# Build de la imagen multi-stage (rust:1.94-bookworm → debian:bookworm-slim)
docker build -t gabysql .

# Run no-root con volumen persistente
docker run -d --name gaby-edge \
  -p 8080:8080 \
  -v /opt/edge/data:/data \
  --restart unless-stopped \
  gabysql

# Verificar
curl -s http://localhost:8080/health | jq .
```

Tamaños orientativos:
- imagen final: ~30 MB (incluye `apt-get upgrade` aplicado en build).
- binario `gabysql-server`: ~3-4 MB.
- footprint en runtime: ~10 MB de RSS para una DB pequeña.

---

## 4. Importar CSV

**Escenario**: tienes un CSV con header y necesitas materializarlo en una tabla.

### Opción rápida — `phpgabyadmin`
```text
1. Selecciona DB → tabla → tab Browse
2. Pulsa "Import" → escoge el .csv
3. La primera fila se interpreta como cabecera de columnas
4. Cada fila se traduce a un INSERT
```

### Opción programática — curl + cliente

```bash
# Suponiendo CSV: id,name,score (sin comillas, sin escapes complejos)
SQL="CREATE TABLE rows_csv (id INT PRIMARY KEY, name TEXT, score INT);"
while IFS=, read -r id name score; do
  [[ "$id" == "id" ]] && continue   # skip header
  SQL+=" INSERT INTO rows_csv (id,name,score) VALUES ($id,'$name',$score);"
done < data.csv

curl -s -X POST http://localhost:8080/exec \
  -H "X-Gabysql-Token: $TOKEN" \
  -H 'content-type: application/json' \
  -d "{\"db\":\"demo.db\",\"sql\":$(jq -Rs <<< "$SQL")}"
```

> Para CSVs con comas dentro de campos, comillas, o `\n` en celdas, hoy hay que pre-procesar el CSV. Un import nativo más robusto está en el [Camino A](COMMERCIAL_ROADMAP.md).

---

## 5. Mantenimiento de índices

**Escenario**: tabla grande, agregaste columna y necesitas un índice nuevo sobre datos existentes.

```sql
-- Ya existen 100k filas. Crear el índice fuerza un backfill automático.
CREATE INDEX idx_users_email ON users (email);

-- A partir de aquí, INSERT/UPDATE/DELETE mantienen el índice en vivo:
INSERT INTO users (id, email) VALUES (101, 'nuevo@example.com');
SELECT * FROM users WHERE email = 'nuevo@example.com';   -- usa el índice

UPDATE users SET email = 'cambiado@example.com' WHERE id = 101;
SELECT * FROM users WHERE email = 'nuevo@example.com';   -- 0 filas
SELECT * FROM users WHERE email = 'cambiado@example.com'; -- 1 fila

-- Si el índice ya no aporta, eliminarlo libera la entrada en TableMeta:
DROP INDEX idx_users_email;
```

Limitaciones actuales del índice secundario:
- una sola columna por índice (compuestos en el [Camino A](COMMERCIAL_ROADMAP.md) backlog).
- equality (`=`) sobre cualquier tipo indexable; `BETWEEN` solo sobre columnas `INT` con índice `OrderedInt` (default automático, ADR-0017). Rango sobre `TEXT`/`FLOAT`/`DATE`/`DATETIME` queda en backlog.
- `UNIQUE` declarativo: ✅ soportado (inline `column UNIQUE` o `CREATE UNIQUE INDEX`).

---

## 6. Backup / restore manual

**Escenario**: snapshot reproducible antes de un experimento o release. Desde [ADR-0015](adr/0015-verified-backup-restore.md), el CLI expone subcomandos dedicados que validan CRC32 página por página en lectura y re-abren el destino para confirmar legibilidad — reemplazan al `cp` informal.

```bash
# 1) Detener cualquier escritor (CLI, server) — el lock cross-process (ADR-0013)
#    ya bloquea backups en caliente; aprovechá el corte para hacer un replay limpio.
sudo systemctl stop gabysql-server   # o kill del proceso CLI

# 2) Si quedó WAL pendiente, forzar replay
gabysql info mydb.db

# 3) Snapshot verificado
gabysql backup mydb.db backup-$(date +%Y%m%d-%H%M).db
# CRC validado página por página al leer + re-abre el destino al final
# Si algo falla, aborta antes de dejar un destino corrupto silencioso

# 4) (Opcional) Sweep CRC standalone, sin escribir nada
gabysql verify backup-*.db
gabysql exec   backup-*.db "SELECT COUNT(*) FROM users;"

# 5) Restore (idéntico motor; comando explicita la dirección)
gabysql restore --force backup-*.db mydb.db
```

> Backup online (sin parar el server) es parte del [Camino A](COMMERCIAL_ROADMAP.md).

---

## 7. Multi-DB con `-dir`

**Escenario**: un solo `gabysql-server` que aloja varias DBs aisladas.

```bash
mkdir -p ./data
gabysql-server -dir ./data -addr :8080 -token secret &

# Cada DB se referencia por nombre en cada request.
curl -s http://localhost:8080/dbs \
  -H "X-Gabysql-Token: secret" | jq .
# {"ok":true,"mode":"multi-db","dbs":[]}

curl -s -X POST http://localhost:8080/dbs \
  -H "X-Gabysql-Token: secret" \
  -d '{"db":"app1.db"}'

curl -s -X POST http://localhost:8080/dbs \
  -H "X-Gabysql-Token: secret" \
  -d '{"db":"app2.db"}'

# Operar contra una en específico:
curl -s -X POST http://localhost:8080/exec \
  -H "X-Gabysql-Token: secret" \
  -H 'content-type: application/json' \
  -d '{"db":"app1.db","sql":"CREATE TABLE t (id INT PRIMARY KEY);"}'
```

El nombre de DB se valida (regex de identificadores SQL); paths absolutos o `..` quedan rechazados.

---

## 8. Cliente Python

**Escenario**: ETL o script de operaciones que usa `requests`.

```python
import requests

BASE = "http://localhost:8080"
HEADERS = {"X-Gabysql-Token": "secret", "Content-Type": "application/json"}

def exec_sql(db: str, sql: str) -> list:
    r = requests.post(f"{BASE}/exec", json={"db": db, "sql": sql}, headers=HEADERS, timeout=10)
    r.raise_for_status()
    payload = r.json()
    if not payload.get("ok"):
        raise RuntimeError(payload.get("error", "unknown error"))
    return payload["results"]

# Crear schema
exec_sql("metrics.db", """
  CREATE TABLE events (id INT PRIMARY KEY, kind TEXT, ts INT);
  CREATE INDEX idx_events_kind ON events (kind);
""")

# Insertar 1000 eventos en una sola transacción HTTP
stmts = ";\n".join(
    f"INSERT INTO events (id,kind,ts) VALUES ({i},'click',{1700000000+i})"
    for i in range(1000)
) + ";"
exec_sql("metrics.db", stmts)

# Consultar por índice
[result] = exec_sql("metrics.db", "SELECT id, ts FROM events WHERE kind = 'click' LIMIT 5;")
print(result["columns"], result["rows"][:5])
```

---

## 9. Cliente Node.js

**Escenario**: API node interna que consume gabysql.

```javascript
import { fetch } from 'undici';

const BASE = 'http://localhost:8080';
const TOKEN = process.env.GABYSQL_TOKEN ?? 'secret';

async function exec(db, sql) {
  const r = await fetch(`${BASE}/exec`, {
    method: 'POST',
    headers: { 'X-Gabysql-Token': TOKEN, 'content-type': 'application/json' },
    body: JSON.stringify({ db, sql }),
  });
  if (!r.ok) throw new Error(`HTTP ${r.status}`);
  const payload = await r.json();
  if (!payload.ok) throw new Error(payload.error);
  return payload.results;
}

await exec('app.db', `
  CREATE TABLE u (id INT PRIMARY KEY, email TEXT);
  CREATE INDEX idx_u_email ON u (email);
  INSERT INTO u (id, email) VALUES (1, 'ana@x.com');
`);

const [byEmail] = await exec('app.db', "SELECT id FROM u WHERE email = 'ana@x.com';");
console.log(byEmail.rows);   // [[1]]
```

---

## 10. Corrupción y CRC

**Escenario**: demostrar que `gabysql` detecta corrupción accidental.

```bash
# 1) DB válida con una fila
gabysql init demo.db
gabysql exec demo.db "
  CREATE TABLE u (id INT PRIMARY KEY, name TEXT);
  INSERT INTO u (id, name) VALUES (1, 'Ana');
"

# 2) Inyectar bit-flip en la página leaf (offset 4096*2 + 50)
python3 - <<'PY'
import os
with open('demo.db', 'r+b') as f:
    f.seek(4096 * 2 + 50)
    b = f.read(1)[0]
    f.seek(4096 * 2 + 50)
    f.write(bytes([b ^ 0xFF]))
PY

# 3) Intentar leer → CRC mismatch detectado
gabysql exec demo.db "SELECT * FROM u WHERE id = 1;"
# error: page 2 corrupt: checksum mismatch (stored=0x..., computed=0x...)
```

Esto es exactamente lo que cubre el test [`page_checksum_detects_corruption`](../tests/integration_test.rs).

Acción recomendada al ver este error en producción: **restaurar desde backup más reciente** (ver [caso 6](#6-backup-restore-manual) y [RUNBOOK.md §Recovery tras caída](../RUNBOOK.md)).

---

## 11. Modelador web → DDL → ejecutar en phpgabyadmin (zero-SQL para empezar)

**Escenario**: arrancar un schema completo sin escribir SQL a mano.

```bash
docker compose up -d --build
# Modeler:        http://localhost:8000/modeler/
# phpgabyadmin:   http://localhost:8000/phpgabyadmin/
```

Pasos en el browser:

1. Abre `gabymodeler` → click en **📦 Cargar ejemplo** para ver `users + orders`.
2. Modifica nombres / tipos / flags `PK`/`idx` a tu gusto.
3. Pulsa **Exportar SQL** → modal con todo el DDL listo (incluye `CREATE DATABASE IF NOT EXISTS shop;`).
4. **📋 Copiar al portapapeles**.
5. Cambia a la pestaña abierta de `phpgabyadmin` → tab **SQL** → pega → **Ejecutar**.
6. Verifica en tab **Browse** que las tablas y los índices aparecen.

> El modelador **no se conecta** al server por sí mismo (zero-coupling con CORS / token). Toda la ejecución pasa por `/exec` mediante phpgabyadmin, dentro de la misma transacción.

---

## 12. CREATE / DROP / SHOW DATABASE desde la API

```bash
# Server multi-DB
gabysql-server -dir ./dbs -addr :8080 -token "$TOKEN" &

# Crear varias DBs en una transacción
curl -s -X POST http://localhost:8080/exec \
  -H "X-Gabysql-Token: $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"sql":"CREATE DATABASE shop; CREATE DATABASE IF NOT EXISTS analytics; SHOW DATABASES;"}'

# {"ok":true,"results":[
#   {"columns":[],"rows":[],"message":"OK","db":"shop"},
#   {"columns":[],"rows":[],"message":"OK","db":"analytics"},
#   {"columns":["database"],"rows":[["analytics"],["shop"]],"message":null}
# ]}

# Drop con guardrail
curl -s -X POST http://localhost:8080/exec \
  -H "X-Gabysql-Token: $TOKEN" \
  -d '{"sql":"DROP DATABASE IF EXISTS analytics;"}'
```

Limitaciones:
- En modo `-db` (single-DB), estos statements responden `405`.
- No se admite mezclar `CREATE DATABASE` con `CREATE TABLE` en el mismo `/exec`: el server rechaza la mezcla con error explícito (no compartirían transacción de todos modos).

---

## 13. Stress test rápido (medir inserts/segundo)

**Escenario**: cuantificar cuántos inserts/segundo aguanta el motor en tu hardware.

```bash
gabysql init bench.db
gabysql exec bench.db "CREATE TABLE big (id INT PRIMARY KEY, payload TEXT);"

# Generar 10000 INSERTs en una sola transacción HTTP
python3 - <<'PY' > /tmp/bulk.sql
print("BEGIN;")
for i in range(10000):
    print(f"INSERT INTO big (id, payload) VALUES ({i}, 'row-{i:08d}');")
print("COMMIT;")
PY

time gabysql exec bench.db "$(cat /tmp/bulk.sql)"

# Verificar el conteo (cuando GROUP BY exista; por ahora basta con scan)
gabysql exec bench.db "SELECT id FROM big LIMIT 5;"
gabysql info bench.db   # mostrará pageCount alto, prueba que hubo splits
```

> Una versión reproducible y con thresholds de regresión vive en [GABYBENCH_SPEC.md](GABYBENCH_SPEC.md). Su implementación es parte del [Camino A](COMMERCIAL_ROADMAP.md).

---

## 14. Query exploratoria con `phpgabyadmin` (zero-SQL)

**Escenario**: alguien no técnico (analista, QA) necesita ver los pedidos pendientes.

```text
1. Abre http://localhost:8000/phpgabyadmin/
2. Sidebar → DB: orders.db → Tabla: orders
3. Pestaña Structure
   • Verifica que existe el índice idx_orders_status sobre status
   • Si no existe, llena el form 'Crear nuevo índice':
       name = idx_orders_status, columna = status → CREATE INDEX
4. Pestaña SQL → click en snippet 'SELECT por columna indexada'
   → se precarga: SELECT * FROM orders WHERE name = 'Ana';
   • Edita 'name' por 'status', el valor por 'pending'
   • Pulsa Ejecutar → tabla con los pedidos pendientes
```

Sin que el usuario tenga que recordar la sintaxis exacta. La sintaxis válida está cubierta por los snippets.

---

## 15. Validar schema antes de migrar (smoke pre-deploy)

**Escenario**: estás por publicar una nueva versión del schema y quieres verificar que la DB existente no quedó inconsistente.

```bash
# Snapshot de tablas
gabysql exec prod.db "SELECT * FROM users LIMIT 0;" 2>&1   # imprime solo columnas
# Si CRC falla aquí, restauras desde backup antes de tocar nada.

# Verificar que el catálogo abre completo
for table in users orders invoices; do
  echo "=== $table ==="
  gabysql exec prod.db "SELECT * FROM $table LIMIT 1;" || echo "FALTA $table"
done

# Verificar que cada índice esperado existe
curl -s "http://localhost:8080/schema?db=prod.db&table=orders" \
  -H "X-Gabysql-Token: $TOKEN" | jq '.table.indexes[] | .name'
# "idx_orders_status"
# "idx_orders_customer"
```

> `gabysql integrity-check <db>` (recorrido completo de B+Trees + revalidación de CRCs) es parte del [Camino A](COMMERCIAL_ROADMAP.md). Mientras tanto, abrir cada tabla con un `SELECT LIMIT 1` cubre el smoke básico.

---

## 16. Smoke test de un release recién buildeado

**Escenario**: tras `cargo build --release`, validar que el binario hace lo prometido antes de subirlo.

```bash
TMP=$(mktemp -d)
cd "$TMP"

# Crear DB nueva (formato actual VERSION 7)
./target/release/gabysql init smoke.db
./target/release/gabysql info smoke.db
# pageSize=4096  pageCount=1  catalogRoot=0

# Setup mínimo
./target/release/gabysql exec smoke.db "
  CREATE TABLE u (id INT PRIMARY KEY, name TEXT);
  INSERT INTO u (id, name) VALUES (1, 'Ana');
  CREATE INDEX idx_u_name ON u (name);
"

# Verificar PK lookup, índice secundario, UPDATE, DELETE, DROP INDEX
./target/release/gabysql exec smoke.db "SELECT * FROM u WHERE id = 1;"
./target/release/gabysql exec smoke.db "SELECT * FROM u WHERE name = 'Ana';"
./target/release/gabysql exec smoke.db "UPDATE u SET name = 'Beto' WHERE id = 1;"
./target/release/gabysql exec smoke.db "SELECT * FROM u WHERE name = 'Beto';"   # 1 fila
./target/release/gabysql exec smoke.db "DELETE FROM u WHERE id = 1;"
./target/release/gabysql exec smoke.db "DROP INDEX idx_u_name;"

# Server smoke
./target/release/gabysql-server -db smoke.db -addr :18080 &
PID=$!
sleep 1
curl -s http://localhost:18080/health | jq .ok   # true
kill $PID

cd / && rm -rf "$TMP"
echo "✅ smoke OK"
```

Este script es exactamente el patrón que `RELEASE.md` define como pre-release manual.

---

## 17. Comparativa side-by-side con SQLite

**Escenario**: medir diferencias y similitudes con el mismo dataset.

```bash
mkdir -p /tmp/compare && cd /tmp/compare

# Mismo schema
SCHEMA="CREATE TABLE k (id INT PRIMARY KEY, name TEXT);"
INSERT="INSERT INTO k (id,name) VALUES (1,'Ana'); INSERT INTO k (id,name) VALUES (2,'Beto');"
QUERY="SELECT * FROM k WHERE id = 2;"

# === gabysql ===
gabysql init g.db
gabysql exec g.db "$SCHEMA"
gabysql exec g.db "$INSERT"
echo "--- gabysql ---"
time gabysql exec g.db "$QUERY"
ls -la g.db

# === SQLite ===
sqlite3 s.db "$SCHEMA"
sqlite3 s.db "$INSERT"
echo "--- sqlite ---"
time sqlite3 s.db "$QUERY"
ls -la s.db
```

Comparativa cualitativa completa en [docs/COMPETITIVE_ANALYSIS.md](COMPETITIVE_ANALYSIS.md).

---

## 18. Auditar la imagen Docker antes de desplegarla

**Escenario**: tu pipeline interno requiere scan de CVEs antes de aprobar un push de imagen.

```bash
docker build -t gabysql:audit .

# Inventario completo (informativo, sin bloquear)
docker run --rm -v /var/run/docker.sock:/var/run/docker.sock \
  anchore/grype:v0.110.0 gabysql:audit -o table | tee grype-full.txt

# Política del proyecto: solo CVEs con fix disponible bloquean (.grype.yaml)
docker run --rm -v /var/run/docker.sock:/var/run/docker.sock \
  -v "$PWD:/work" -w /work \
  anchore/grype:v0.110.0 gabysql:audit -c .grype.yaml
# "No vulnerabilities found" → green-light para desplegar.
```

Esto replica exactamente el job `container_scan` del workflow `security.yml` ([ADR-0006](adr/0006-grype-only-fixed.md)).

---

## 19. Generar credenciales y rotar token del server

**Escenario**: rotar el token compartido sin downtime largo.

```bash
# 1) Generar nuevo token aleatorio
NEW_TOKEN=$(openssl rand -hex 32)

# 2) Levantar segunda instancia con el nuevo token en otro puerto
gabysql-server -db prod.db -addr :8081 -token "$NEW_TOKEN" &
NEW_PID=$!

# 3) Smoke contra la nueva
curl -s http://localhost:8081/health \
  -H "X-Gabysql-Token: $NEW_TOKEN" | jq .ok   # true

# 4) Cambiar el reverse proxy / clientes para apuntar al puerto 8081
#    (cualquier rolling update de tu orquestador funciona)

# 5) Bajar la vieja
kill $(pgrep -f "gabysql-server.*-addr :8080")

# 6) Reasignar el 8080 al nuevo proceso (o ajustar tu config)
```

> Un endpoint `/admin/rotate-token` con auth fina es parte del [Camino B](COMMERCIAL_ROADMAP.md). Mientras tanto, este patrón blue-green simple basta.

---

## 🔭 Más casos en el futuro

Cuando se entreguen los hitos del [Camino A](COMMERCIAL_ROADMAP.md), este doc crecerá con:

- ORDER BY por columna indexada
- Constraints declarativas (UNIQUE / NOT NULL / DEFAULT)
- `gabysql integrity-check` operacional
- Backup / restore con verificación
- Drivers oficiales Python + Node publicados

Si quieres priorizar uno específico, abre un Issue con el caso de uso y los volúmenes esperados.
