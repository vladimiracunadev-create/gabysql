# 04. Mapa completo del código

## Núcleo y binarios

| Ruta | Responsabilidad | Dependencias/consumidores | Estado |
|---|---|---|---|
| `src/lib.rs` | API común, `DbError`, módulos | Todos los binarios/tests | Activo, crítico |
| `src/storage.rs` | Pager, formato, CRC, WAL, locks, cache | B+Tree, backup, engine | Activo, crítico |
| `src/bptree.rs` | Árbol persistente y cursores | Catálogo, índices, SQL | Activo, crítico |
| `src/catalog.rs` | Tipos y objetos de catálogo serializados | Engine/server | Activo, crítico |
| `src/index.rs` | Índices hash/ordenados, buckets y codecs | Engine | Activo |
| `src/sql.rs` | AST, parser, expresiones, planner y ejecución | CLI/server/tests | Activo, crítico |
| `src/server.rs` | HTTP/JSON, sesiones, métricas y límites | `gabysql-server`, admin, MCP | Activo |
| `src/errors.rs` | Catálogo compilado `[GBY-NNNN]` | Todas las capas | Activo |
| `src/dblog.rs` | Log JSONL de sentencias con rotación | Engine/CLI/server | Activo |
| `src/backup.rs` | Backup, restore y verify con CRC | CLI | Activo |
| `src/bin/gabysql.rs` | CLI y REPL | Librería | Activo |
| `src/bin/gabysql-server.rs` | Flags y arranque HTTP | `server.rs` | Activo |
| `src/bin/gabysql-mcp.rs` | MCP stdio, HTTP client, vector search/audit | Server HTTP | Activo |
| `src/bin/gabybench.rs` | Benchmark integral reproducible | Engine | Activo |
| `src/bin/gabysql-bench.rs` | Harness alternativo de microbench | Engine | Activo; solapamiento requiere contexto |
| `src/bin/demo-dbs.rs` | Generación de bases demo | Engine | Utilidad activa |

## Interfaces y soporte

| Ruta | Responsabilidad | Estado |
|---|---|---|
| `web/index.php` | Landing | Activo |
| `web/phpgabyadmin/index.php` | Admin, auth/cookies/CSRF y proxy | Activo; sensible |
| `web/modeler/index.html` | Modelador ER y generación SQL | Activo |
| `desktop/gabymodeler/src-tauri/` | Wrapper de escritorio | Activo |
| `examples/` | Clientes PHP/Python | Activo |
| `tests/` | Integración, propiedades, fuzz y server | Activo |
| `.github/workflows/` | CI, release, seguridad, Pages y mantenimiento | 7 workflows activos |
| `docs/adr/` | Historia de decisiones | Histórico/normativo; no reescribir |

No se confirmó código muerto mediante análisis dinámico. Los dos binarios de benchmark tienen objetivos distintos documentados, aunque su convivencia aumenta mantenimiento. Los logs `bench-*.log`, `fuzz-1h.log` y `bench/results.json` no son código y aparecen sin seguimiento en el worktree analizado.
