# 📘 USER MANUAL

> **Guía de uso diario de `gabysql`, `gabysql-server` y `phpgabyadmin`.**

---

## 🧭 Navegación rápida

| Si quieres... | Abre esta sección |
|---|---|
| crear o consultar una DB local | CLI `gabysql` |
| usar el motor en modo interactivo | REPL |
| exponer la base por HTTP | `gabysql-server` |
| operar desde navegador | `phpgabyadmin` |
| ver ejemplos mínimos | `examples/README.md` |

---

## 1. 🖥️ CLI `gabysql`

El binario principal sirve para inicializar bases, inspeccionar metadatos, ejecutar SQL y abrir un REPL.

### Comandos
```text
gabysql init    [--force] <file.db>
gabysql info               <file.db>
gabysql exec               <file.db> "<SQL...>"
gabysql repl               <file.db>
gabysql backup  [--force] <src.db> <dst.db>
gabysql restore [--force] <src.db> <dst.db>
gabysql verify             <file.db>
```

> `init` rehúsa sobrescribir un archivo existente. Pasa `--force` para reemplazarlo intencionalmente.
> `backup` lee `src.db` validando CRC32 página por página, escribe `dst.db` y lo re-abre al final para confirmar que es legible. `restore` es la operación inversa (mismo motor; el comando explicita la dirección de la operación). `verify` solo recorre el archivo y valida CRCs. Si alguna página falla, los tres abortan con error claro **antes** de tocar el destino. Ver [ADR-0015](docs/adr/0015-verified-backup-restore.md).
> Si otro proceso `gabysql` tiene el archivo abierto, las operaciones que requieren lock (incluido `init`/`exec`/`repl`) fallan con `database is locked by another process` — ver [TROUBLESHOOTING.md](TROUBLESHOOTING.md) y [ADR-0013](docs/adr/0013-process-level-file-lock.md).

### Ejemplos
```powershell
cargo run --release --bin gabysql -- init demo.db
cargo run --release --bin gabysql -- info demo.db
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, email TEXT NOT NULL UNIQUE, name TEXT, active BOOL DEFAULT TRUE);"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,email,name) VALUES (1,'ana@x','Ana');"
cargo run --release --bin gabysql -- exec demo.db "SELECT id,name FROM users WHERE id = 1;"
```

---

## 2. 🧾 SQL soportado

### DDL — `CREATE TABLE` con constraints inline
```sql
CREATE TABLE users (
  id     INT  PRIMARY KEY,
  email  TEXT NOT NULL UNIQUE,
  name   TEXT,
  status TEXT NOT NULL DEFAULT 'pending',
  active BOOL DEFAULT TRUE,
  born   DATE,
  meta   JSON
);

CREATE TABLE orders (
  id      INT   PRIMARY KEY,
  user_id INT   REFERENCES users(id) ON DELETE CASCADE,
  total   FLOAT,
  tries   INT   DEFAULT 0
);
```

Constraints disponibles por columna (gabysql `VERSION 7+`):
- `PRIMARY KEY` — una sola, debe ser `INT`, implícitamente `NOT NULL`.
- `NOT NULL` — rechaza `NULL` literal y omisión sin DEFAULT.
- `UNIQUE` — auto-genera índice unique `uq_<tabla>_<col>`. Múltiples NULL permitidos.
- `DEFAULT <literal>` — INT/FLOAT/BOOL/TEXT/DATE/DATETIME/JSON o `NULL`. Tipo validado al CREATE.
- `REFERENCES <tabla>(<col>) [ON DELETE RESTRICT|CASCADE]` — single-column FK; el target debe ser la PK del parent.

### DDL — `DROP TABLE` y `ALTER TABLE`
```sql
DROP TABLE [IF EXISTS] <name>;
ALTER TABLE <name> ADD [COLUMN] <coldef>;
```
- `DROP TABLE` quita la entrada del catálogo (las páginas backing no se liberan; reclamo futuro vía `vacuum`).
- `ALTER TABLE ADD COLUMN` no reescribe filas previas; éstas se decodifican con el `DEFAULT` de la columna nueva (o `NULL`). El rewrite ocurre naturalmente en el siguiente `UPDATE`. Restricciones: no admite `PRIMARY KEY`; `NOT NULL` requiere `DEFAULT` no nulo; `UNIQUE` con DEFAULT no nulo se rechaza si la tabla tiene > 1 fila.

### DDL de bases de datos (server multi-DB / CLI)
```sql
CREATE DATABASE shop;
CREATE DATABASE IF NOT EXISTS analytics;
SHOW DATABASES;            -- lista las DBs en el directorio
DROP DATABASE IF EXISTS analytics;
```
> Estas sentencias **no se ejecutan contra una `.db` específica**. Se procesan a nivel del directorio configurado (`gabysql-server -dir ./dbs` o el directorio padre del path en CLI). En modo single-DB (`-db`) se rechazan con HTTP 405. No se admite mezclarlas con sentencias de tabla en el mismo `/exec`.

### DML
```sql
INSERT INTO users (id,email,name) VALUES (1,'ana@x','Ana');
INSERT INTO orders (id,user_id,total) VALUES (10,1,99.5);

SELECT * FROM users;
SELECT id,name FROM users WHERE id = 1;
SELECT id,name FROM users WHERE id BETWEEN 1 AND 10 LIMIT 5 OFFSET 0;

-- ORDER BY (cualquier columna, sort en memoria post-scan)
SELECT id,name FROM users ORDER BY name ASC;
SELECT id,name FROM users ORDER BY score DESC LIMIT 10;

-- SELECT con LIMIT sin ORDER BY usa cursor lazy: solo se leen
-- las páginas leaf necesarias para servir el LIMIT, no la tabla
-- entera (ver ADR-0008). Para tablas grandes la diferencia se nota.
SELECT id FROM big LIMIT 10;
SELECT id FROM big WHERE id BETWEEN 100 AND 200 LIMIT 5;

UPDATE users SET name = 'Ana M', active = FALSE WHERE id = 1;
DELETE FROM users WHERE id = 1;       -- cascade si hay FKs entrantes
```

`INSERT` aplica DEFAULTs para columnas omitidas, valida `NOT NULL`, hace pre-check de `UNIQUE` y `FK` antes de tocar disco. `UPDATE` revalida sólo las constraints cuyas columnas cambiaron. `DELETE` resuelve cascade/restrict según FKs entrantes (worklist con cycle protection).

### Índices secundarios
```sql
CREATE INDEX        idx_users_name ON users (name);
CREATE UNIQUE INDEX uq_users_email ON users (email);   -- aborta si hay duplicados
DROP   INDEX        idx_users_name;
```

Reglas:
- una sola columna por índice (no compuestos).
- soportan equality (`=`) sobre cualquier tipo indexable.
- **`BETWEEN` solo sobre columnas `INT` indexadas** (índice `OrderedInt`, default automático al crear índice sobre `INT`; ADR-0017). `BETWEEN` sobre `TEXT`/`FLOAT`/`BOOL`/`DATE`/`DATETIME` indexados devuelve error claro.
- se mantienen automáticamente al `INSERT`, `UPDATE` (cuando cambia la columna indexada) y `DELETE`.
- **no admiten** columnas `JSON`.
- el nombre del índice debe ser único en toda la base de datos.
- `UNIQUE` permite múltiples `NULL` (consistente con SQL estándar).

### Operacional — `INTEGRITY CHECK`
```sql
INTEGRITY CHECK;
```
Sweep de solo lectura: valida CRCs de cada página, decodifica cada fila, verifica que cada entrada de índice apunte a una PK existente y que cada FK no nula tenga su parent. Devuelve un ResultSet `(kind, object, detail)` con los hallazgos y un `message` resumen `OK · ... | FAIL · ...`. Recomendado tras un crash, restore o como sanity check periódico.

### Tipos soportados
- `INT`
- `TEXT`
- `BOOL`
- `FLOAT`
- `DATE`
- `DATETIME`
- `JSON`
- `NULL` para columnas no PK

### Identificadores
- Forma léxica: `[A-Za-z_][A-Za-z0-9_]*`
- Longitud máxima: **64**
- No pueden ser palabras reservadas del parser (lista completa en [docs/SQL_REFERENCE.md](docs/SQL_REFERENCE.md#-identificadores)).

### Reglas importantes
- La PK debe ser **una sola columna `INT`**. Esta versión no soporta PKs compuestas ni de otros tipos.
- `PRIMARY KEY` no puede ser `NULL` y es implícitamente `NOT NULL`.
- PK duplicada se rechaza al `INSERT`.
- `WHERE col = val` funciona sobre la PK siempre, y sobre cualquier otra columna **solo si tiene un índice secundario**.
- `WHERE col BETWEEN a AND b` funciona sobre la PK y sobre cualquier columna `INT` con índice secundario (índice `OrderedInt`, ADR-0017). Para `TEXT`/`FLOAT`/`BOOL`/`DATE`/`DATETIME` queda en backlog.
- `UPDATE` y `DELETE` solo aceptan `WHERE pk = N`, no por columna no-PK.
- `UPDATE` no permite cambiar la PK; intentarlo devuelve error explícito.
- `UPDATE` y `DELETE` sobre una PK inexistente retornan error (no son no-ops silenciosos).
- `DELETE` en una tabla con FKs entrantes aplica cascade/restrict según `ON DELETE` declarado.

---

## 3. ⌨️ REPL

```powershell
cargo run --release --bin gabysql -- repl demo.db
```

Cada sentencia debe terminar con `;`.

---

## 4. 🌐 `gabysql-server`

Expone la base por HTTP/JSON.

### Single DB
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
```

### Multi DB
```powershell
cargo run --release --bin gabysql-server -- -dir ./dbs -addr :8080
```

### Token opcional
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -token secret
```

El cliente debe enviar uno de estos headers:
- `X-Gabysql-Token: secret`
- `Authorization: Bearer secret`

### Tope de conexiones simultáneas
El servidor por defecto limita a `64` conexiones activas. Las conexiones por encima del techo reciben `503` y se cierran sin generar threads. Para ajustarlo:
```powershell
cargo run --release --bin gabysql-server -- -dir ./dbs -max-connections 32
```

### Logs JSON estructurados (`-log-json`)
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -log-json
```
Con `-log-json`, cada request finalizado emite una sola línea JSON a stdout: `{ts_unix, method, path, status, latency_ms}`. Pensado para ingestión por Loki, Vector, journald, etc. Sin el flag, el binario solo escribe el banner de arranque a stderr (default silencioso por request). Ver [ADR-0014](docs/adr/0014-logs-json-metrics.md).

### Endpoint `/metrics`
```powershell
Invoke-WebRequest -UseBasicParsing http://localhost:8080/metrics
```
Devuelve un JSON con `started_unix`, `uptime_s`, `requests_total`, `requests_by_status` (mapa `status_code → count`), `errors_total` y `latency_ms` (`p50`, `p95`, `samples`, `count`). Útil para scraping periódico o un dashboard mínimo sin desplegar Prometheus. Ver [docs/API.md §GET /metrics](docs/API.md#get-metrics).

> [!TIP]
> Para detalles de endpoints, payloads y errores, ve a [docs/API.md](docs/API.md).

---

## 5. 📐 `gabymodeler v2` — modelador web (PowerDesigner-style)

`gabymodeler v2` es un single-page HTML+JS vanilla (sin npm, sin frameworks, sin backend acoplado) con layout PowerDesigner-style — Object Browser + Canvas + Result List + Status bar — espejo del motor `gabysql VERSION 7`.

### Levantarlo
```bash
docker compose up -d --build
# Modeler:        http://localhost:8000/modeler/
# phpgabyadmin:   http://localhost:8000/phpgabyadmin/
```
o con `php -S`:
```bash
php -S localhost:8000 -t web
```

### Capacidades
- **Constraints inline por columna**: `PK`, `NOT NULL` (NN), `UNIQUE` (UN), `DEFAULT <literal>`, `FOREIGN KEY` (FK con `ON DELETE RESTRICT|CASCADE`).
- **Check Model continuo** con 14 reglas (PK ausente, identificador inválido o reservado, NOT NULL+DEFAULT NULL, FK rota, type mismatch, etc.). Cada hallazgo es clickeable y selecciona la entidad/columna.
- **SQL Preview en vivo** (panel inferior) + modal `Ver SQL` con copiar/descargar. El DDL emitido respeta orden topológico (parents antes que children) e incluye todas las constraints inline.
- **↘ Importar de gabysql**: dado URL del server + DB, hace `GET /tables?db=<db>` y reconstruye entidades, columnas, constraints y FKs. CORS habilitado en el server desde VERSION 6+.
- **Persistencia local** (`localStorage` con clave `gabymodeler.v2`; migración automática desde la v1 vieja).

### Manual completo con screenshots
👉 **[web/modeler/USER_MANUAL.md](web/modeler/USER_MANUAL.md)** — walkthrough end-to-end de cada surface con capturas reales.

---

## 6. 🧭 `phpgabyadmin`

Es un cliente PHP del API HTTP, no un motor separado.

### Levantarlo localmente
```powershell
php -S localhost:8000 -t web
```

### Flujo recomendado
1. Arranca `gabysql-server`
2. Arranca `phpgabyadmin`
3. Entra a `http://localhost:8000/phpgabyadmin/`
4. Selecciona DB, tabla o ejecuta SQL

### Pestañas disponibles
- **Browse** — paginación de filas, export CSV, import CSV.
- **Structure** — columnas + tipos + PK + columna marcada como indexada; **lista de índices secundarios** con botón `DROP` por índice y formulario inline `CREATE INDEX` que filtra automáticamente PK y JSON.
- **SQL** — editor con snippets de un click para `SELECT` / `SELECT por PK` / `SELECT por columna indexada` / `INSERT` / `UPDATE` / `DELETE` / `CREATE INDEX` / `DROP INDEX`. Cada ejecución es Begin → Exec → Commit; múltiples sentencias separadas por `;` entran en la misma transacción.

### Seguridad del admin
- Si defines `GABYADMIN_TOKEN`, el admin pedirá login
- La cookie de login está firmada
- Por defecto solo permite `localhost` y loopback
- Para apuntar a un server remoto debes definir `GABYADMIN_ALLOW_REMOTE=1`

---

## 7. 🧪 Ejemplos incluidos

Consulta [examples/README.md](examples/README.md) para ejemplos de clientes Python y PHP contra CLI y HTTP.

---

## 8. ✅ Buenas prácticas de uso

- Usa `gabysql` para operaciones locales y pruebas rápidas
- Usa `gabysql-server` cuando necesites UI web o integración HTTP
- Para snapshots usá `gabysql backup <src.db> <dst.db>` en vez de `cp` — valida CRC end-to-end y re-abre el destino para confirmar legibilidad.
- Si necesitas reproducibilidad entre equipos, usa Docker Compose
