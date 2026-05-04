# 🧰 RUNBOOK

> **Guía de operación diaria, smoke checks, backup y recovery de `gabysql`.**

> **Audiencia**: Operadores, maintainers, soporte técnico.

---

## 🚀 Arranque estándar

### Single DB
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
```

### Multi DB
```powershell
mkdir dbs
cargo run --release --bin gabysql-server -- -dir ./dbs -addr :8080
```

### Docker Compose
```powershell
docker compose up -d --build
```

---

## ✅ Smoke checks mínimos

### Health
```powershell
Invoke-WebRequest -UseBasicParsing http://localhost:8080/health
```

### Crear DB en modo `-dir`
```powershell
Invoke-WebRequest -UseBasicParsing -Method Post -ContentType 'application/json' -Body '{"db":"demo"}' http://localhost:8080/dbs
```

### Ejecutar SQL
```powershell
Invoke-WebRequest -UseBasicParsing -Method Post -ContentType 'application/json' -Body '{"db":"demo.db","sql":"CREATE TABLE users (id INT PRIMARY KEY, name TEXT);"}' http://localhost:8080/exec
```

### Leer filas
```powershell
Invoke-WebRequest -UseBasicParsing "http://localhost:8080/rows?db=demo.db&table=users&limit=25&offset=0"
```

---

## 💾 Backup recomendado

### Estrategia principal
1. Detén el proceso que escribe
2. Verifica que no quede `.wal` pendiente
3. Copia el `.db`

### Qué evitar
- no hacer backup de un `.db` activo suponiendo consistencia total
- no copiar solo el `.db` si hubo una caída y todavía existe `.wal` sin replay

---

## ♻️ Recovery tras caída

Si quedó un archivo `.wal` junto al `.db`:
1. Abre la base con `gabysql info <db>` o arranca el server apuntando a esa DB
2. `Pager::open` reintentará replay si el WAL tiene marcador `COMMIT`
3. Cada página dentro del WAL se valida por CRC32 antes de aplicarse al `.db`
4. Tras el replay correcto, el `.wal` se elimina

> [!IMPORTANT]
> El recovery actual depende de la existencia de `COMMIT` en el WAL. Si el archivo quedó truncado o sin commit marker, el WAL se descarta. Si una página del WAL falla la verificación CRC, el replay aborta con error explícito en vez de corromper el `.db`.

---

## 🌐 Operación de `phpgabyadmin`

- Mantén el admin expuesto solo en red local o detrás de un reverse proxy controlado
- Usa `GABYADMIN_TOKEN` si no es un laboratorio desechable
- No habilites `GABYADMIN_ALLOW_REMOTE=1` salvo que realmente necesites apuntar a otro host

---

## 🔐 Operación con token HTTP

Si el server usa `-token secret`, verifica con:
```powershell
Invoke-WebRequest -UseBasicParsing -Headers @{ 'X-Gabysql-Token' = 'secret' } http://localhost:8080/health
```

---

## 🧪 Comandos útiles

### Validación nativa
```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

### Validación con Docker
```powershell
docker build -t gabysql .
docker compose up -d --build
```

### Apagar stack Docker
```powershell
docker compose down
```

---

## 🚨 Incidentes comunes

| Incidente | Significado |
|---|---|
| `base de datos 'X' ya existe` / `no existe` | `CREATE/DROP DATABASE` sin `IF [NOT] EXISTS` cuando aplicaba |
| `CREATE/DROP/SHOW DATABASE requieren modo -dir` | server arrancado con `-db`; volver a arrancar con `-dir` |
| `no se admite mezclar CREATE/DROP/SHOW DATABASE con sentencias de tabla` | separar en dos `/exec` distintos |
| `bad magic (not gabysql db)` | el archivo no es una DB válida |
| `unsupported gabysql file format: version=N` | la DB fue creada con una versión anterior del formato; recrearla |
| `page N corrupt: checksum mismatch` | corrupción detectada al leer; restaurar desde backup |
| `WAL record for page N fails checksum` | WAL corrupto antes del replay; restaurar `.db` desde backup y descartar `.wal` |
| `refusing to overwrite existing database` | `gabysql init` detectó un archivo previo; usar `init --force` si la intención es reset |
| `duplicate primary key` | intento de insertar PK repetida |
| `fila no existe: PK=N` | `UPDATE` o `DELETE` sobre una PK que no existe |
| `no se permite cambiar la PRIMARY KEY en UPDATE` | un `UPDATE ... SET pk = ...` fue rechazado |
| `WHERE soporta solo '=' o BETWEEN` | consulta fuera del subconjunto SQL actual |
| `WHERE solo soporta PK` | un `UPDATE`/`DELETE`/`SELECT` filtró por columna no PK |
| `server busy: N active connections (max M)` | el servidor alcanzó `-max-connections`; el cliente debe reintentar |
| `401 unauthorized` | token faltante o incorrecto |
| `db inválida` | nombre de DB no aceptado en modo `-dir` |

---

## 📈 Observabilidad actual

Hoy el server:
- escribe errores a stderr
- al arrancar imprime `single`, `dir` y el `max_connections` efectivo
- expone `/health` como endpoint de salud
- no tiene todavía métricas Prometheus, tracing distribuido ni dashboard integrado
