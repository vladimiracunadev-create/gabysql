# API

## Resumen
`gabysql-server` expone una API HTTP/JSON simple para:
- listar bases
- crear bases en modo multi DB
- listar tablas
- ver schema
- leer filas
- ejecutar SQL

## Autenticación
Si el server se arranca con `-token <valor>`, cada request debe incluir uno de estos headers:
- `X-Gabysql-Token: <valor>`
- `Authorization: Bearer <valor>`

Sin token válido, el server responde `401`.

## Modos de operación
### Single DB
```powershell
gabysql-server -db demo.db -addr :8080
```

En este modo, el server trabaja sobre una sola DB y no necesitas enviar `db` en cada request.

### Multi DB
```powershell
gabysql-server -dir ./dbs -addr :8080
```

En este modo sí debes enviar `db` en los endpoints que operan sobre una base específica.

## Endpoints

### `GET /health`
Verifica que el server esté arriba.

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

### `GET /dbs`
Lista bases disponibles.

#### Respuesta en modo single DB
```json
{
  "ok": true,
  "mode": "single-db",
  "dbs": ["demo.db"],
  "single": "demo.db"
}
```

#### Respuesta en modo multi DB
```json
{
  "ok": true,
  "mode": "multi-db",
  "dbs": ["demo.db", "test.db"]
}
```

### `POST /dbs`
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

### `GET /tables?db=demo.db`
Lista tablas registradas.

Ejemplo:
```json
{
  "ok": true,
  "tables": [
    {
      "name": "users",
      "primaryKey": "id",
      "rootPage": 2,
      "columns": [
        { "name": "id", "type": "INT", "pk": true },
        { "name": "name", "type": "TEXT", "pk": false }
      ]
    }
  ]
}
```

### `GET /schema?db=demo.db&table=users`
Retorna el schema de una tabla.

### `GET /rows?db=demo.db&table=users&limit=25&offset=0`
Devuelve filas proyectadas por orden natural del índice principal.

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

### `POST /exec`
Ejecuta una o más sentencias SQL dentro de una transacción.

Request:
```json
{
  "db": "demo.db",
  "sql": "CREATE TABLE users (id INT PRIMARY KEY, name TEXT); INSERT INTO users (id,name) VALUES (1,'Ana'); SELECT * FROM users;"
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

## Errores frecuentes
- `400`: SQL inválido, tabla inexistente, request incompleto.
- `401`: token faltante o incorrecto.
- `404`: endpoint o tabla inexistente.
- `405`: operación no permitida en ese modo.
- `409`: DB ya existe.
- `500`: error interno inesperado.

## Notas operacionales
- El server protege escrituras con un mutex de proceso.
- No existe todavía rate limiting.
- No hay TLS nativo; usa reverse proxy si expones el servicio.
