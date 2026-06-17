# 🌐 API

> **Referencia HTTP/JSON de `gabysql-server`: endpoints, autenticación, payloads y respuestas.**

---

## 🧭 Resumen

`gabysql-server` expone una API simple para:
- listar bases
- crear bases en modo multi DB
- listar tablas
- ver schema
- leer filas
- ejecutar SQL

---

## 🔐 Autenticación

Si el server se arranca con `-token <valor>`, cada request debe incluir uno de estos headers:
- `X-Gabysql-Token: <valor>`
- `Authorization: Bearer <valor>`

Sin token válido, el server responde `401`.

Para **sesiones cross-request** (M13) hay un header adicional sin relación con autenticación:
- `X-Gabysql-Session: <hex16>` — ID de sesión devuelto por `/tx/begin`. Equivalente al query param `?session=<hex16>` en cualquier endpoint que lo acepte. Ver [`POST /tx/begin`](#post-txbegin--post-txcommit--post-txrollback-m13) abajo.

---

## 🗂️ Modos de operación

| Modo | Comando | Implicación |
|---|---|---|
| Single DB | `gabysql-server -db demo.db -addr :8080` | no necesitas enviar `db` en cada request |
| Multi DB | `gabysql-server -dir ./dbs -addr :8080` | debes enviar `db` en los endpoints que operan sobre una base |

---

## 🚏 Endpoints

| Método | Ruta | Qué hace |
|---|---|---|
| `GET` | `/health` | health check del server |
| `GET` | `/metrics` | métricas operacionales (JSON: contadores + latencias p50/p95) |
| `GET` | `/dbs` | lista bases disponibles |
| `POST` | `/dbs` | crea una DB en modo `-dir` |
| `GET` | `/tables` | lista tablas de una DB |
| `GET` | `/schema` | devuelve schema de una tabla |
| `GET` | `/rows` | devuelve filas con paginación |
| `POST` | `/exec` | ejecuta una o más sentencias SQL (auto-commit por request, o dentro de sesión con `X-Gabysql-Session`) |
| `POST` | `/tx/begin` | **M13 (2026-06-15)** — abre una sesión cross-request con tx activa, devuelve `session_id` |
| `POST` | `/tx/commit?session=<id>` | **M13** — commit + cierra la sesión |
| `POST` | `/tx/rollback?session=<id>` | **M13** — rollback + cierra la sesión |

---

## `GET /health`

Ejemplo de respuesta:
```json
{
  "ok": true,
  "name": "gabysql-server",
  "single": false,
  "dir": "/data",
  "timeUnix": 1773970519
}
```

---

## `GET /metrics`

Devuelve un snapshot de las métricas operacionales acumuladas desde el arranque del server. Pensado para scraping periódico (cron + curl, Vector, Telegraf) o un dashboard mínimo sin desplegar Prometheus. Ver [ADR-0014](adr/0014-logs-json-metrics.md).

Ejemplo de respuesta:
```json
{
  "ok": true,
  "started_unix": 1773970519,
  "uptime_s": 4827,
  "requests_total": 1284,
  "requests_by_status": {
    "200": 1199,
    "400": 42,
    "401": 11,
    "404": 18,
    "500": 14
  },
  "errors_total": 85,
  "latency_ms": {
    "p50": 2,
    "p95": 38,
    "samples": 1024,
    "count": 1284
  }
}
```

Notas:
- `errors_total` cuenta status `>= 500` (errores del server, no del cliente).
- `latency_ms.samples` es el tamaño del ring buffer en memoria (cap fijo `LATENCY_SAMPLE_RING = 1024`); `count` es el total observado desde el arranque. `p50`/`p95` se calculan sobre el sample.
- Si el server arranca con `-log-json`, cada request termina emitiendo una línea JSON a stdout (`{ts_unix, method, path, status, latency_ms}`).
- No requiere autenticación distinta a la del server (si arrancaste con `-token`, el header se exige también acá).

---

## `GET /dbs`

### Respuesta en modo single DB
```json
{
  "ok": true,
  "mode": "single-db",
  "dbs": ["demo.db"],
  "single": "demo.db"
}
```

### Respuesta en modo multi DB
```json
{
  "ok": true,
  "mode": "multi-db",
  "dbs": ["demo.db", "test.db"]
}
```

---

## `POST /dbs`

Crea una base en modo `-dir`.

Request:
```json
{ "db": "demo" }
```

El server normaliza el nombre y agrega `.db` si falta.

Posibles respuestas:
- `201` creada
- `409` ya existe
- `400` nombre inválido

---

## `GET /tables?db=demo.db`

Lista todas las tablas con su schema completo (mismo shape que `/schema`, pero embebido en un array). Útil para reverse-engineering one-shot.

```json
{
  "ok": true,
  "tables": [
    {
      "name": "users",
      "primaryKey": "id",
      "rootPage": 2,
      "columns": [
        { "name": "id",    "type": "INT",  "pk": true,  "notNull": true,  "unique": false, "hasDefault": false, "default": null,      "references": null },
        { "name": "email", "type": "TEXT", "pk": false, "notNull": true,  "unique": true,  "hasDefault": false, "default": null,      "references": null },
        { "name": "status","type": "TEXT", "pk": false, "notNull": true,  "unique": false, "hasDefault": true,  "default": "pending", "references": null }
      ],
      "indexes": [
        { "name": "uq_users_email", "column": "email", "rootPage": 4, "unique": true }
      ]
    }
  ]
}
```

---

## `GET /schema?db=demo.db&table=users`

Retorna el schema completo de **una** tabla, con la información necesaria para reconstruir el `CREATE TABLE` original (modo reverse-engineering del modeler).

Ejemplo:
```json
{
  "ok": true,
  "table": {
    "name": "users",
    "primaryKey": "id",
    "rootPage": 2,
    "columns": [
      { "name": "id",    "type": "INT",  "pk": true,  "notNull": true,  "unique": false, "hasDefault": false, "default": null },
      { "name": "email", "type": "TEXT", "pk": false, "notNull": true,  "unique": true,  "hasDefault": false, "default": null },
      { "name": "status","type": "TEXT", "pk": false, "notNull": true,  "unique": false, "hasDefault": true,  "default": "pending" },
      { "name": "score", "type": "FLOAT","pk": false, "notNull": false, "unique": false, "hasDefault": true,  "default": 0.0,  "references": null },
      { "name": "active","type": "BOOL", "pk": false, "notNull": false, "unique": false, "hasDefault": true,  "default": true, "references": null },
      { "name": "manager_id", "type": "INT", "pk": false, "notNull": false, "unique": false, "hasDefault": false, "default": null,
        "references": { "table": "users", "column": "id", "onDelete": "RESTRICT" } }
    ],
    "indexes": [
      { "name": "uq_users_email", "column": "email", "rootPage": 4, "unique": true }
    ]
  }
}
```

Reglas del campo `default`:
- `"hasDefault": false` ⇒ `"default": null` significa **no hay** DEFAULT declarado.
- `"hasDefault": true` y `"default": null` significa **`DEFAULT NULL` explícito**.
- En cualquier otro caso `default` lleva el literal con su tipo nativo (`number`, `string`, `boolean`).

Campo `references`:
- `null` cuando la columna no tiene FK.
- Objeto `{ table, column, onDelete }` con la FK declarada. `onDelete` es `"RESTRICT"` o `"CASCADE"` (default `"RESTRICT"` cuando se omite en el SQL).

Status posibles: `200` con `ok: true`, `404` con `ok: false, error: "tabla no existe"`.

---

## `GET /rows?db=demo.db&table=users&limit=25&offset=0`

Reglas:
- `limit` por defecto: `25`
- `offset` por defecto: `0`
- `limit` máximo: `1000`

Ejemplo:
```json
{
  "ok": true,
  "db": "demo.db",
  "table": "users",
  "total": 2,
  "limit": 25,
  "offset": 0,
  "columns": ["id", "name"],
  "rows": [[1, "Ana"], [2, "Beto"]]
}
```

---

## `POST /exec`

Ejecuta una o más sentencias SQL dentro de una transacción. Acepta:
- `CREATE DATABASE [IF NOT EXISTS] <name>` *(server multi-DB)*
- `DROP DATABASE [IF EXISTS] <name>`
- `SHOW DATABASES`
- `CREATE TABLE`, `DROP TABLE`, `ALTER TABLE ADD COLUMN`, `INSERT`, `UPDATE`, `DELETE`
- `SELECT` / `UPDATE` / `DELETE` con:
  - `WHERE` completo: atomos `=`, `<`, `>`, `<=`, `>=`, `<>`/`!=`, `BETWEEN`, `IS [NOT] NULL`, `[NOT] LIKE`, `[NOT] IN (lista | SELECT)`, `= (SELECT)`, `[NOT] EXISTS (SELECT)`. Combinadores `AND`, `OR`, `NOT`, paréntesis. Lógica trivaluada ANSI para NULL. Gramática detallada en [SQL_REFERENCE.md](SQL_REFERENCE.md).
  - `ORDER BY <col> [ASC|DESC]`, `LIMIT n`, `OFFSET n` (solo en `SELECT`)
  - `FROM` con `[INNER|LEFT|RIGHT|FULL [OUTER]|CROSS] JOIN ... (ON l = r | USING (col))` y `NATURAL JOIN` (multi-tabla, aliases, self-join) — solo en `SELECT`
- `CREATE INDEX <nombre> ON <tabla> (<columna>)` (con backfill) y `CREATE UNIQUE INDEX`
- `DROP INDEX <nombre>`
- `INTEGRITY CHECK`

`UPDATE` y `DELETE` son single-table (sin `JOIN` ni `UPDATE ... FROM`) pero aceptan el WHERE completo y operan multi-fila (response `message` trae la cuenta). `SELECT` con JOINs admite `WHERE` cualificado (`tabla.col = val`) como post-filter.

> Las sentencias **DATABASE-level** (`CREATE/DROP/SHOW DATABASE`) **no** abren un `Pager` — el server las despacha contra el directorio configurado con `-dir`. **No se admite mezclarlas con sentencias de tabla en el mismo `/exec`**: el server retorna `400` si lo intentas. En modo single-DB (`-db`) responden `405`.

Request:
```json
{
  "db": "demo.db",
  "sql": "CREATE TABLE users (id INT PRIMARY KEY, name TEXT); INSERT INTO users (id,name) VALUES (1,'Ana'); UPDATE users SET name = 'Ana M' WHERE id = 1; SELECT * FROM users;"
}
```

Ejemplo de respuesta:
```json
{
  "ok": true,
  "results": [
    { "columns": [], "rows": [], "message": "OK" },
    { "columns": [], "rows": [], "message": "OK" },
    { "columns": ["id", "name"], "rows": [[1, "Ana"]] }
  ]
}
```

---

## `POST /tx/begin` · `POST /tx/commit` · `POST /tx/rollback` (M13)

**Cross-request transactions** vía sesión single-slot global. Habilitado por
[ADR-0090](adr/0090-m13-cross-request-tx.md). Permite que un cliente externo
(ORM, batch loader, script) abra una transacción en un request, ejecute N
sentencias en requests subsiguientes, y commitee/rolbackee al final.

### Flujo

1. **Abrir sesión** — `POST /tx/begin` (opcionalmente `{"db":"<name>"}` en modo `-dir`).
   - 200 → `{"ok":true,"session":"<hex16>","db":"<name>"}`. Guardar el `session` para usarlo en cada `/exec` y para cerrar la tx.
   - 409 → ya hay una sesión activa. El server es single-slot: el cliente debe esperar a que la sesión existente cierre, o forzar cierre con `/tx/rollback` si conoce el ID.
2. **Ejecutar SQL en la sesión** — `POST /exec` con header `X-Gabysql-Session: <hex16>` o query param `?session=<hex16>`. El server NO auto-commit; el `Pager` de la sesión persiste sus dirty pages entre requests.
   - 200 → `{"ok":true,"session":"<hex16>","results":[...]}`.
   - 404 → el `session` no existe o expiró (idle timeout 300s).
3. **Cerrar sesión** — `POST /tx/commit?session=<hex16>` o `POST /tx/rollback?session=<hex16>`. Devuelve `{"ok":true,"message":"COMMIT","db":"<name>"}` (o `ROLLBACK`).

### Ejemplo `curl`

```bash
# 1) Abrir sesión.
SESSION=$(curl -sX POST http://127.0.0.1:8080/tx/begin -d '{}' | jq -r '.session')

# 2) Operar varios requests dentro de la misma tx.
curl -sX POST http://127.0.0.1:8080/exec \
    -H "X-Gabysql-Session: $SESSION" \
    -d '{"sql":"INSERT INTO users (id, name) VALUES (1, \"Ana\")"}'

curl -sX POST http://127.0.0.1:8080/exec \
    -H "X-Gabysql-Session: $SESSION" \
    -d '{"sql":"INSERT INTO users (id, name) VALUES (2, \"Beto\")"}'

# 3) Decidir commit o rollback al final.
curl -sX POST "http://127.0.0.1:8080/tx/commit?session=$SESSION"
```

### Headers

- **`X-Gabysql-Session: <hex16>`** — alternativa al query param. Para SDKs que prefieren mantener el ID fuera de la URL.

### Errores específicos

- `400` — `/tx/commit` o `/tx/rollback` sin `?session=<id>`.
- `404` — session ID no corresponde a la sesión activa (ya cerrada o nunca existió).
- `409` — segundo `/tx/begin` con sesión activa (single-slot).

### Notas

- **Single-slot global**: solo una sesión cross-request activa a la vez en todo el server. Multi-session real depende de WAL-mode (ADR-0018, Fase 6).
- **Idle timeout**: 300s. Cualquier request a `/tx/*` o `/exec` con session ID checkea el last_used; si pasó el threshold, hace rollback + cierre silencioso.
- **Backwards compatible**: requests sin session ID se comportan exactamente como antes de M13 (auto-commit por request).
- **Cómo combina con SAVEPOINT (M12)**: el cliente puede hacer `BEGIN` (implícito por `/tx/begin`), múltiples `SAVEPOINT` / `ROLLBACK TO` parciales en requests separados, y decidir `/tx/commit` o `/tx/rollback` al final.

---

## 🚨 Errores frecuentes

| Código | Motivo típico |
|---|---|
| `400` | SQL inválido, tabla inexistente, request incompleto |
| `401` | token faltante o incorrecto |
| `404` | endpoint o tabla inexistente |
| `405` | operación no permitida en ese modo |
| `409` | DB ya existe |
| `500` | error interno inesperado |
| `503` | techo de conexiones simultáneas alcanzado (default `64`) |

---

## 🧠 Notas operacionales

- El server protege escrituras con un mutex de proceso.
- El server limita conexiones concurrentes (default `64`, ajustable con `gabysql-server -max-connections N`). Conexiones por encima del techo reciben `503`.
- No existe todavía rate limiting por IP/cliente.
- No hay TLS nativo; usa reverse proxy si expones el servicio.
