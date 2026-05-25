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
| `POST` | `/exec` | ejecuta una o más sentencias SQL |

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
