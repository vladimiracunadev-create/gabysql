# RUNBOOK

## Objetivo
Este runbook describe la operación base de `gabysql` en modo local y server HTTP.

## 1. Arranque estándar
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

## 2. Smoke checks mínimos
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

## 3. Backup recomendado
### Recomendación principal
Haz backup offline:
1. Detén el proceso que escribe.
2. Verifica que no quede `.wal` pendiente.
3. Copia el `.db`.

### Qué evitar
- No hagas backup de un `.db` activo suponiendo consistencia total.
- No copies solo el `.db` si hubo una caída y todavía existe `.wal` sin replay.

## 4. Recovery tras caída
Si quedó un archivo `.wal` junto al `.db`:
1. Abre la base con `gabysql info <db>` o arranca el server apuntando a esa DB.
2. `Pager::open` reintentará replay si el WAL tiene marcador `COMMIT`.
3. Tras el replay correcto, el `.wal` se elimina.

## 5. Operación de `phpgabyadmin`
- Mantén el admin expuesto solo en red local o detrás de un reverse proxy controlado.
- Usa `GABYADMIN_TOKEN` si no es un laboratorio desechable.
- No habilites `GABYADMIN_ALLOW_REMOTE=1` salvo que realmente necesites apuntar a otro host.

## 6. Operación con token HTTP
Si el server usa `-token secret`, verifica con:
```powershell
Invoke-WebRequest -UseBasicParsing -Headers @{ 'X-Gabysql-Token' = 'secret' } http://localhost:8080/health
```

## 7. Comandos útiles
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

## 8. Incidentes comunes
- `bad magic (not gabysql db)`: el archivo no es una DB válida de `gabysql`.
- `duplicate primary key`: se intentó insertar una PK ya existente.
- `WHERE soporta solo '=' o BETWEEN`: la consulta usa un filtro no soportado todavía.
- `401 unauthorized`: falta token o es incorrecto.
- `db inválida`: el nombre de DB trae ruta o caracteres no permitidos en modo `-dir`.

## 9. Observabilidad actual
Hoy el server escribe errores a stderr y expone solo `/health` como endpoint de salud.
No hay todavía métricas Prometheus, tracing distribuido ni dashboard integrado.
