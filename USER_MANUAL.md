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
gabysql init [--force] <file.db>
gabysql info <file.db>
gabysql exec <file.db> "<SQL...>"
gabysql repl <file.db>
```

> `init` rehúsa sobrescribir un archivo existente. Pasa `--force` para reemplazarlo intencionalmente.

### Ejemplos
```powershell
cargo run --release --bin gabysql -- init demo.db
cargo run --release --bin gabysql -- info demo.db
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, active BOOL);"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,name,active) VALUES (1,'Ana',TRUE);"
cargo run --release --bin gabysql -- exec demo.db "SELECT id,name FROM users WHERE id = 1;"
```

---

## 2. 🧾 SQL soportado

### DDL
```sql
CREATE TABLE users (
  id INT PRIMARY KEY,
  name TEXT,
  active BOOL,
  score FLOAT,
  born DATE,
  meta JSON
);
```

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
INSERT INTO users (id,name,active) VALUES (1,'Ana',TRUE);
SELECT * FROM users;
SELECT id,name FROM users WHERE id = 1;
SELECT id,name FROM users WHERE id BETWEEN 1 AND 10 LIMIT 5 OFFSET 0;
UPDATE users SET name = 'Ana M', active = FALSE WHERE id = 1;
DELETE FROM users WHERE id = 1;
```

### Índices secundarios
```sql
-- Crear un índice secundario sobre una columna no-PK.
-- Si la tabla ya tiene filas, el índice se backfillea automáticamente.
CREATE INDEX idx_users_name ON users (name);

-- Una vez creado, las búsquedas por igualdad sobre la columna usan el índice.
SELECT * FROM users WHERE name = 'Ana';

-- Eliminar el índice. Vuelve a fallar el SELECT por esa columna.
DROP INDEX idx_users_name;
```

Reglas de los índices secundarios:
- una sola columna por índice (no compuestos).
- soportan equality (`=`), no rangos ni `BETWEEN`.
- se mantienen automáticamente al `INSERT`, `UPDATE` (cuando cambia la columna indexada) y `DELETE`.
- **no admiten** columnas `JSON` (sin semántica canónica de igualdad).
- el nombre del índice debe ser único en toda la base de datos.

### Tipos soportados
- `INT`
- `TEXT`
- `BOOL`
- `FLOAT`
- `DATE`
- `DATETIME`
- `JSON`
- `NULL` para columnas no PK

### Reglas importantes
- La PK debe ser **una sola columna `INT`**. Esta versión no soporta PKs compuestas ni de otros tipos.
- `PRIMARY KEY` no puede ser `NULL`.
- PK duplicada se rechaza al `INSERT`.
- `WHERE col = val` funciona sobre la PK siempre, y sobre cualquier otra columna **solo si tiene un índice secundario**.
- `WHERE col BETWEEN a AND b` solo funciona sobre la PK (sin range scan en índices secundarios todavía).
- `UPDATE` y `DELETE` solo aceptan `WHERE pk = N`, no por columna no-PK.
- `UPDATE` no permite cambiar la PK; intentarlo devuelve error explícito.
- `UPDATE` y `DELETE` sobre una PK inexistente retornan error (no son no-ops silenciosos).

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

> [!TIP]
> Para detalles de endpoints, payloads y errores, ve a [docs/API.md](docs/API.md).

---

## 5. 📐 `gabymodeler` — modelador web

`gabymodeler` es un single-page HTML+JS vanilla (sin npm, sin frameworks, sin backend acoplado) para diseñar esquemas y exportarlos como SQL DDL listo para `gabysql`.

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

### Flujo
1. Click en `＋ Nueva entidad` para cada tabla.
2. Define columnas con su tipo, marca `PK` (que se fija a `INT` automáticamente) y `idx` para indexar.
3. (Opcional) Botón `↪ FK` agrega una columna que apunta a `tabla.columna` de otra entidad — se dibuja una línea Bezier y se documenta como comentario en el SQL (FK declarativas no son enforced en `VERSION 4`).
4. `Exportar SQL` → modal con `CREATE DATABASE [IF NOT EXISTS]` + `CREATE TABLE` + `CREATE INDEX`.
5. Copia o descarga el `.sql` y pégalo en `phpgabyadmin → tab SQL` para ejecutarlo.

El modelo se persiste en `localStorage` (clave `gabymodeler.v1`); botón `📦 Cargar ejemplo` carga un schema `users + orders` con FK indexada para evaluar el flujo en 1 click.

Detalle completo en [web/modeler/README.md](web/modeler/README.md).

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
- Haz backup offline del `.db` antes de cambios manuales o pruebas destructivas
- Si necesitas reproducibilidad entre equipos, usa Docker Compose
