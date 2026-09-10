# 09. APIs e integraciones

El contrato HTTP canónico, ejemplos y shapes se mantienen en [docs/API.md](../API.md). No duplique clientes contra esta síntesis sin contrastar aquel documento.

## HTTP/JSON

| Método/ruta | Propósito | Entradas relevantes |
|---|---|---|
| `GET /health` | Salud | Token si se configuró |
| `GET /metrics` | Contadores, estados, latencia y uptime | Token |
| `GET /dbs` | Lista/describe modo | Token |
| `POST /dbs` | Crea DB en modo `-dir` | JSON `db` |
| `GET /tables` | Catálogo de tablas/objetos | Query `db` |
| `GET /schema` | Esquema de tabla | `db`, `table` |
| `GET /rows` | Filas paginadas | `db`, `table`, `limit`, `offset` |
| `POST /exec` | Ejecuta SQL | JSON `db`, `sql`; sesión opcional |
| `POST /tx/begin` | Abre sesión transaccional | DB |
| `POST /tx/commit` | Confirma sesión | Session ID |
| `POST /tx/rollback` | Revierte/cierra sesión | Session ID |

Autenticación: token opcional enviado como `X-Gabysql-Token` o Bearer según cliente/documentación. El servidor impone 100 MiB de body y 64 conexiones por defecto. No se identificaron webhooks ni reintentos internos; el cliente debe implementar backoff ante `SERVER_BUSY` y distinguir códigos `[GBY-*]`.

## MCP

`gabysql-mcp` expone catálogo, descripción, query, execute (omitido en read-only), integrity check, vector search y audit tail. Usa stdio JSON-RPC y delega a HTTP. Recursos: `gabysql://catalog` y `gabysql://schema/{db}`. Protocolo declarado en código: `2024-11-05`.

## Otras integraciones

- Clientes de ejemplo PHP/Python en `examples/`.
- `phpgabyadmin` consume el servidor como proxy web.
- GitHub Actions automatiza CI, seguridad, Pages, release y desktop release.
- Docker Compose conecta servicio y web.
- No se identificaron bases o APIs externas obligatorias.
