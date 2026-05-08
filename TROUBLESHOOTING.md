# 🩺 TROUBLESHOOTING

> **Fallos frecuentes, causas probables y resolución rápida para `gabysql`.**

---

## 🪟 Windows: `cargo test` falla por `link.exe` o `kernel32.lib`

### Síntoma
El build compila pero no logra linkear tests o binarios release.

### Causa
Falta el toolchain nativo MSVC/Windows SDK.

### Solución
Instala:
- Visual Studio Build Tools
- MSVC C++ build tools
- Windows SDK

Mientras tanto puedes validar el proyecto con Docker:
```powershell
docker build -t gabysql .
```

---

## 🧱 `bad magic (not gabysql db)`

### Causa
El archivo no fue inicializado por `gabysql` o está corrupto.

### Solución
```powershell
cargo run --release --bin gabysql -- init demo.db
```

---

## 🔢 `unsupported gabysql file format: version=N (expected 6)`

### Causa
Intentas abrir un archivo con una versión de formato anterior. La versión actual es `6`. Las versiones `1` a `5` quedaron explícitamente fuera (cada bump persiste cosas nuevas: `2` cambió el hash, `3` agregó CRCs, `4` agregó índices secundarios, `5` agregó `NOT NULL`/`DEFAULT`/`UNIQUE`, `6` agregó `FOREIGN KEY`). Ver [COMPATIBILITY.md §5](COMPATIBILITY.md#5--formato-en-disco).

### Solución
- Re-crear la base con el binario actual: `gabysql init <file.db>`.
- Si tenías datos en la DB vieja, exportarlos antes con la versión que la creó (no hay aún herramienta automatizada de migración).

---

## 🛡️ `columna 'X' es NOT NULL; INSERT no la cubre`

### Causa
Una columna declarada `NOT NULL` quedó ausente del `INSERT` y no tiene `DEFAULT`, o se le pasó `NULL` literal.

### Solución
- Pasar un valor explícito en el `INSERT`, o
- Declarar `DEFAULT <literal>` en el `CREATE TABLE` para que el motor rellene la columna cuando se omita.

---

## 🔁 `violación de UNIQUE en índice 'uq_t_c' (PK existente: N)`

### Causa
El `INSERT`/`UPDATE` intenta colocar un valor que ya existe en otra fila para una columna `UNIQUE` (inline o `CREATE UNIQUE INDEX`). El pre-check del motor lo rechaza **antes de tocar disco**, así que la transacción queda intacta.

### Solución
- Cambiar el valor a uno único, o
- Buscar la PK ofensora (`N`) y decidir si se actualiza o se borra esa fila.

---

## 🔗 `violación de FK: 'X.col' = N no existe en 'Y'`

### Causa
El `INSERT`/`UPDATE` apunta a un parent que no existe en la tabla referenciada. Las FKs no se autocompletan ni se ignoran.

### Solución
- Verificar que el parent exista con `SELECT * FROM <parent> WHERE id = N`.
- Si la columna debe poder estar vacía, declararla nullable y dejar `NULL`.

---

## 🚫 `violación de FK: 'X.col' referencia 'Y' (ON DELETE RESTRICT, M fila(s) afectadas)`

### Causa
Estás intentando borrar un parent que todavía tiene filas hijas, y la FK fue declarada (o tomó el default) `ON DELETE RESTRICT`.

### Solución
- Borrar primero las filas hijas, o
- Re-declarar la FK con `ON DELETE CASCADE` (requiere recrear la tabla; en esta versión no hay `ALTER TABLE ... DROP CONSTRAINT`).

---

## 🩺 `INTEGRITY CHECK` reporta hallazgos

### Causa
El sweep operacional detectó algo: CRC inválido, fila no decodificable, entrada de índice huérfana o FK colgada. Los `kind` posibles están en [docs/SQL_REFERENCE.md §INTEGRITY CHECK](docs/SQL_REFERENCE.md#integrity-check).

### Solución
- `page_corrupt` o `row_decode` → restore desde backup; el archivo está físicamente comprometido.
- `orphan_index_entry` → suele indicar un crash entre escritura del índice y la fila; `DROP INDEX` + `CREATE INDEX` reconstruye limpio.
- `fk_target_missing` o `fk_orphan` → datos inconsistentes; arreglar manualmente con `INSERT` del parent o `DELETE` de las hijas.

---

## 🛡️ `page N corrupt: checksum mismatch` o `WAL record for page N fails checksum`

### Causa
La verificación CRC32 detectó que la página leída del `.db` o del `.wal` no coincide con su checksum almacenado. Causa típica: corte de energía durante una escritura, fallo de disco, o edición manual del archivo.

### Solución
- Restaurar `.db` desde el backup más reciente.
- Borrar el `.wal` solo si el error sale del WAL replay: el `.db` previo aún es válido.
- Si la corrupción es persistente, validar el medio de almacenamiento (smartctl, chkdsk).

---

## 🛢️ `base de datos 'X' ya existe` / `base de datos 'X' no existe`

### Causa
Salidas de `CREATE DATABASE` / `DROP DATABASE` cuando la operación no es idempotente.

### Solución
- Para crear: `CREATE DATABASE IF NOT EXISTS <name>;`
- Para borrar: `DROP DATABASE IF EXISTS <name>;`

---

## 🚦 `CREATE/DROP/SHOW DATABASE requieren modo -dir`

### Causa
El server fue arrancado con `-db <archivo.db>` (single-DB) y se mandó una sentencia que opera sobre el directorio.

### Solución
Reiniciar el server con `-dir <carpeta>`:
```bash
gabysql-server -dir ./dbs -addr :8080
```
o usar `gabysql exec` directamente desde la CLI sobre cualquier path dentro del directorio objetivo.

---

## 🔀 `no se admite mezclar CREATE/DROP/SHOW DATABASE con sentencias de tabla en el mismo /exec`

### Causa
En un `/exec` mandaste un combo como:
```sql
CREATE DATABASE shop;
CREATE TABLE shop_items (id INT PRIMARY KEY);
```
Esos statements no comparten transacción (uno opera sobre el directorio, el otro sobre la `.db` recién creada que aún no existe en el momento del parse).

### Solución
Separar en dos llamadas:
1. `CREATE DATABASE shop;` (sin `db` en el body, server lo despacha al directorio).
2. `CREATE TABLE ...;` (con `"db":"shop.db"` en el body).

---

## 🚫 `refusing to overwrite existing database`

### Causa
`gabysql init <file.db>` se ejecutó sobre un archivo que ya existe. Es deliberado: la versión actual no destruye archivos sin pedirlo.

### Solución
- Si querías iniciar la DB existente, usa `gabysql info <file.db>` o `gabysql exec <file.db> ...` directamente.
- Si querías reset intencional: `gabysql init --force <file.db>`.

---

## 🔁 `duplicate primary key`

### Causa
Insertaste una fila con una PK `INT` ya usada.

### Solución
- usa otra PK
- consulta primero
- si quieres modificar el row, usa `UPDATE <tabla> SET ... WHERE <pk> = N`

---

## ✏️ `fila no existe: PK=N`

### Causa
Un `UPDATE` o `DELETE` apuntó a una PK que no existe en la tabla.

### Solución
- consulta primero con `SELECT ... WHERE pk = N`
- los `UPDATE`/`DELETE` son explícitos: no son no-ops silenciosos como en otros motores

---

## 🚦 `server busy: N active connections (max M)` (HTTP 503)

### Causa
El servidor alcanzó el techo de conexiones simultáneas (default `64`).

### Solución
- el cliente debe reintentar tras un backoff corto
- subir el techo si tu carga lo justifica: `gabysql-server -max-connections 128`

---

## 🔎 `WHERE soporta solo '=' o BETWEEN`

### Causa
La consulta usa operadores no implementados (`LIKE`, `>`, `<`, etc.).

### Solución
Restringe `WHERE` a la PK con `=` o `BETWEEN`. Recordá que `UPDATE` y `DELETE` solo aceptan `=`, no `BETWEEN`.

---

## 🛑 `WHERE solo soporta PK (...) o columnas con índice secundario; '<col>' no está indexada`

### Causa
Hiciste `SELECT ... WHERE col = val` sobre una columna que no es PK y no tiene índice secundario.

### Solución
- crear el índice: `CREATE INDEX idx_<tabla>_<col> ON <tabla> (<col>);`
- o filtrar por la PK
- en `UPDATE` / `DELETE`, el filtro siempre debe ser `WHERE pk = N` (no se admite por columna no-PK aunque tenga índice).

---

## 🔍 `ya existe un índice llamado '<name>' en la tabla '<table>'`

### Causa
Los nombres de índice son únicos en toda la base de datos (no solo por tabla).

### Solución
- usa otro nombre, idealmente con prefijo de tabla: `idx_users_name`, `idx_orders_status`.

---

## 🔍 `la columna '<col>' ya tiene un índice secundario`

### Causa
Esta versión soporta solo un índice secundario por columna.

### Solución
- si quieres reemplazarlo, primero `DROP INDEX <viejo>` y luego `CREATE INDEX`.

---

## 🚫 `no se admiten índices sobre columnas JSON en esta versión`

### Causa
`JSON` no tiene una semántica canónica de igualdad (dos representaciones distintas pueden ser equivalentes).

### Solución
- indexa una columna escalar derivada del JSON (`status TEXT`, `category INT`, etc.) y mantenla sincronizada en cliente.

---

## 🗃️ `tabla no existe`

### Causa
La tabla no fue creada o se consultó con otro nombre.

### Solución
```powershell
Invoke-WebRequest -UseBasicParsing "http://localhost:8080/tables?db=demo.db"
```

---

## 📁 `falta db (modo -dir)`

### Causa
Estás usando `gabysql-server -dir ...` pero no enviaste `db` en la query o payload.

### Solución
Incluye `db` en:
- `GET /tables?db=demo.db`
- `GET /schema?db=demo.db&table=users`
- `GET /rows?db=demo.db&table=users`
- `POST /exec` con `{ "db": "demo.db", "sql": "..." }`

---

## 🔐 `401 unauthorized`

### Causa
El server fue arrancado con `-token` y el cliente no envió token correcto.

### Solución
Envía:
- `X-Gabysql-Token`
- `Authorization: Bearer <token>`

---

## 🧭 `phpgabyadmin` no conecta al server

### Revisa
- que `gabysql-server` esté arriba
- que el host/puerto sean correctos
- que `GABYADMIN_SERVER` apunte al server correcto
- que no esté bloqueado un host remoto por `GABYADMIN_ALLOW_REMOTE`
- que el token HTTP coincida si el server está protegido

---

## 🌍 `Servidor remoto bloqueado` en `phpgabyadmin`

### Causa
Por seguridad, el admin solo acepta loopback por defecto.

### Solución
```powershell
$env:GABYADMIN_ALLOW_REMOTE='1'
php -S localhost:8000 -t web
```

---

## 🐳 Docker: puerto en uso

### Síntoma
`docker compose up` falla porque `8080` o `8000` ya están ocupados.

### Solución
- libera el puerto
- o cambia el mapeo en `docker-compose.yml`

---

## 🔄 Docker: cambios de código no se reflejan

### Solución
```powershell
docker compose up -d --build
```

---

## 🟢 Necesito saber si el producto está sano

### Checklist corto
```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
docker build -t gabysql .
```

Si eso pasa y `/health` responde, la base actual del producto está íntegra para su alcance actual.
