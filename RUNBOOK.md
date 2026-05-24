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
Invoke-WebRequest -UseBasicParsing -Method Post -ContentType 'application/json' -Body '{"db":"demo.db","sql":"CREATE TABLE users (id INT PRIMARY KEY, email TEXT NOT NULL UNIQUE, name TEXT);"}' http://localhost:8080/exec
```

### Leer filas
```powershell
Invoke-WebRequest -UseBasicParsing "http://localhost:8080/rows?db=demo.db&table=users&limit=25&offset=0"
```

### Inspeccionar schema completo (constraints + FKs + índices)
```powershell
Invoke-WebRequest -UseBasicParsing "http://localhost:8080/tables?db=demo.db"
Invoke-WebRequest -UseBasicParsing "http://localhost:8080/schema?db=demo.db&table=users"
```

### Sweep de integridad (post-restore o periódico)
```powershell
Invoke-WebRequest -UseBasicParsing -Method Post -ContentType 'application/json' -Body '{"db":"demo.db","sql":"INTEGRITY CHECK;"}' http://localhost:8080/exec
```
Devuelve un ResultSet con filas `(kind, object, detail)` por hallazgo y `message` resumen `OK · ... | FAIL · ...`. Si hay `page_corrupt` o `row_decode`, restore desde backup.

> **Nota sobre memoria.** `INTEGRITY CHECK` toca cada página de la DB. Desde [ADR-0009](docs/adr/0009-page-cache-lru-bounded.md) el `PageCache` está acotado por defecto a 1024 páginas (~4 MB por DB), así que el sweep no fuga RAM aunque la DB sea grande — las páginas viejas se evictan LRU a medida que entran nuevas. Pre-ADR-0009 este endpoint era un disparador clásico de OOM en server long-running.

### Tuning del cache de páginas
Default: 1024 páginas (~4 MB) por Pager. Para embebidos con poca RAM, bajar; para servers con working set grande, subir. Solo accesible via API embebida (no hay flag CLI):

```rust
let mut pager = Pager::open("demo.db")?;
pager.set_cache_capacity(64);   // ~256 KB para IoT
// ... o
pager.set_cache_capacity(8192); // ~32 MB para hot path de queries grandes
```

Inspección runtime:
```rust
println!("cache: {}/{} páginas", pager.cache_len(), pager.cache_capacity());
```

---

## 💾 Backup / restore / verify (operación canónica)

Desde [ADR-0015](docs/adr/0015-verified-backup-restore.md), el CLI expone tres subcomandos dedicados que reemplazan al `cp` informal. Todos validan **CRC32 página por página en lectura** y los de escritura **re-abren el destino al final** para confirmar legibilidad — si algo falla, abortan sin dejar un archivo destino corrupto silencioso.

### Backup
```powershell
# 1) Detén el proceso que escribe (importante: file lock cross-process desde ADR-0013)
# 2) Replay del WAL si quedó pendiente (gabysql info <db> lo dispara)
gabysql info mydb.db

# 3) Snapshot reproducible
gabysql backup mydb.db backup-2026-05-18.db
# --force si querés sobreescribir el destino
```

### Restore
```powershell
gabysql restore backup-2026-05-18.db mydb.db --force
```

### Verify (sweep CRC sin escribir)
```powershell
gabysql verify mydb.db
```

### Qué evitar
- no hacer backup de un `.db` activo desde otro proceso: el lock cross-process del ADR-0013 lo bloquea explícitamente (fail-fast con `database is locked by another process`).
- no copiar solo el `.db` con `cp` si hubo una caída y todavía existe `.wal` sin replay. Si tenés que usar `cp`, primero abrí la DB para forzar el replay.
- `gabysql backup`/`restore`/`verify` son las operaciones soportadas; `cp` queda como fallback de emergencia.

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
| `database is locked by another process` | otro proceso `gabysql` ya tiene la `.db` abierta (file lock exclusivo, ADR-0013); detener el otro proceso y reintentar |
| `duplicate primary key` | intento de insertar PK repetida |
| `fila no existe: PK=N` | `UPDATE` o `DELETE` sobre una PK que no existe |
| `no se permite cambiar la PRIMARY KEY en UPDATE` | un `UPDATE ... SET pk = ...` fue rechazado |
| `WHERE soporta solo '=', BETWEEN o IN (SELECT ...)` | consulta fuera del subconjunto SQL actual |
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
- expone `GET /metrics` (JSON) con `requests_total`, `requests_by_status`, `errors_total`, `latency_ms` (p50/p95/samples/count) y `uptime_s`. Ideal para scraping periódico o dashboard mínimo. Ver [ADR-0014](docs/adr/0014-logs-json-metrics.md) y [docs/API.md §GET /metrics](docs/API.md#get-metrics).
- soporta `-log-json`: con el flag, cada request termina emitiendo una línea JSON a stdout: `{ts_unix, method, path, status, latency_ms}`. Sin el flag, no hay log por request — solo el banner de arranque a stderr.
- no tiene todavía métricas Prometheus textfile, tracing distribuido ni dashboard integrado

### Smoke check de métricas
```powershell
Invoke-WebRequest -UseBasicParsing http://localhost:8080/metrics | Select-Object -ExpandProperty Content
```
