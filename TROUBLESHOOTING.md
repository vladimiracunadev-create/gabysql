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

## 🔢 `unsupported version`

### Causa
Intentas abrir un archivo con una versión de formato no soportada por este build.

### Solución
Usa la misma versión del motor que creó el archivo o migra el formato cuando exista soporte oficial de upgrade.

---

## 🔁 `duplicate primary key`

### Causa
Insertaste una fila con una PK `INT` ya usada.

### Solución
- usa otra PK
- consulta primero
- no asumas comportamiento tipo upsert: hoy no existe

---

## 🔎 `WHERE soporta solo '=' o BETWEEN`

### Causa
La consulta usa operadores no implementados (`LIKE`, `>`, `<`, etc.).

### Solución
Restringe `WHERE` a la PK con `=` o `BETWEEN`.

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
