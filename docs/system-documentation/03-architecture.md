# 03. Arquitectura

gabysql usa una arquitectura por capas dentro de un crate: interfaces -> parser/engine -> catálogo e índices -> B+Tree -> pager/WAL -> archivo. El servidor añade concurrencia de conexiones, pero el acceso de escritura conserva el modelo single-writer.

```mermaid
flowchart TD
  U[CLI / app embebida / HTTP / MCP / admin] --> E[Parser y Engine SQL]
  E --> C[Catálogo y reglas]
  E --> I[Índices secundarios]
  C --> B[B+Tree]
  I --> B
  B --> P[Pager + cache + CRC32]
  P --> W[WAL y recovery]
  P --> D[(archivo .db)]
  W --> D
```

`src/sql.rs` concentra AST, parser, evaluación y ejecución. `src/catalog.rs` serializa metadatos. `src/index.rs` codifica buckets hash e índices INT ordenados. `src/bptree.rs` implementa hojas, nodos internos y cursores. `src/storage.rs` controla páginas, cache, lock, WAL y recovery.

## Flujos de interfaz

```mermaid
sequenceDiagram
  participant Client
  participant Server
  participant Engine
  participant Pager
  Client->>Server: POST /exec {db, sql}
  Server->>Server: auth, límites y selección de DB
  Server->>Engine: parse + exec
  Engine->>Pager: leer/modificar páginas
  Pager->>Pager: WAL + CRC + flush
  Engine-->>Server: ResultSet o DbError
  Server-->>Client: JSON + estado HTTP
```

El CLI omite la capa HTTP. El gateway MCP es un adaptador HTTP/JSON y no abre la base. `phpgabyadmin` actúa como proxy/controlador PHP; `gabymodeler` produce SQL en el navegador.

## Estado, errores y procesos

`Engine` conserva catálogo/estadísticas/sesión y puede recibir un `DbLogger`. El servidor mantiene métricas, locks y sesiones transaccionales con expiración pasiva. Los errores públicos usan `DbError` con prefijos `[GBY-NNNN]`. No se identificaron colas ni workers persistentes; la concurrencia HTTP usa threads.

## Despliegue

```mermaid
flowchart LR
  B[Browser] -->|HTTP| PHP[Landing + phpgabyadmin]
  PHP -->|HTTP/JSON| S[gabysql-server]
  A[Agente] -->|stdio MCP| M[gabysql-mcp]
  M -->|HTTP/JSON| S
  S --> F[(uno o varios .db + .wal)]
```

La descripción detallada y decisiones están en [ARCHITECTURE](../ARCHITECTURE.md) y [ADR](../adr/README.md).
