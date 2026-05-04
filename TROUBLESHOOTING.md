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

## 🔢 `unsupported gabysql file format: version=N (expected 3)`

### Causa
Intentas abrir un archivo con una versión de formato anterior. Las versiones `1` y `2` quedaron explícitamente fuera de la versión actual (`1` usaba `DefaultHasher` no estable; `2` no tenía CRC por página).

### Solución
- Re-crear la base con el binario actual: `gabysql init <file.db>`.
- Si tenías datos en la DB vieja, exportarlos antes con la versión que la creó (no hay aún herramienta automatizada de migración).

---

## 🛡️ `page N corrupt: checksum mismatch` o `WAL record for page N fails checksum`

### Causa
La verificación CRC32 detectó que la página leída del `.db` o del `.wal` no coincide con su checksum almacenado. Causa típica: corte de energía durante una escritura, fallo de disco, o edición manual del archivo.

### Solución
- Restaurar `.db` desde el backup más reciente.
- Borrar el `.wal` solo si el error sale del WAL replay: el `.db` previo aún es válido.
- Si la corrupción es persistente, validar el medio de almacenamiento (smartctl, chkdsk).

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
