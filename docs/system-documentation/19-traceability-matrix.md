# 19. Matriz de trazabilidad

| Funcionalidad/regla | Interfaz | Módulo/símbolo | Persistencia | Prueba/evidencia | Documento | Estado |
|---|---|---|---|---|---|---|
| Crear/abrir DB | CLI/server | `Pager::create/open` | Header/páginas/WAL | integration/pager | `TECHNICAL_SPECS` | Verificado |
| DDL y catálogo | SQL/HTTP | `parse`, `Engine`, `Catalog` | Árbol catálogo | integration | `SQL_REFERENCE` | Verificado |
| INSERT/UPDATE/DELETE | Todas | `Engine::exec_*` | Árbol tabla + índices | integration | `USER_MANUAL` | Verificado |
| Constraints | SQL | engine + metadatos | Column/Table/Index/FK | integration | `SQL_REFERENCE` | Verificado |
| SELECT/planner | Todas | select/eval/plan | Árbol/índices/stats | planner + integration | `ANALISIS_POST_P5` | Verificado |
| Transacciones/savepoints | CLI/HTTP | Engine/Pager/session | Estado + WAL | integration + M13 | `API`, ADR-0089/0090 | Verificado |
| Backup/restore/verify | CLI/lib | `backup::*` | Copia de páginas | unit/integration | `RUNBOOK` | Verificado |
| Integridad | CLI/SQL/HTTP | Pager + `INTEGRITY CHECK` | Lee todas las páginas | integration | `RUNBOOK` | Verificado |
| Authz/RLS | SQL/session | User/Role/Grant/Policy + hooks | Catálogo | integration | ADR-0050..0062 | Verificado |
| HTTP API | Clientes/admin/MCP | `server::run`/handlers | DB seleccionada | server tests | `docs/API.md` | Verificado |
| Sesión cross-request | `/tx/*`, `/exec` | session registry | Pager/Engine en memoria | `m13_server.rs` | ADR-0090 | Verificado |
| Métricas/request log | `/metrics`, flag | server metrics/logging | Memoria/stdout | server tests | ADR-0014 | Verificado |
| Statement log | CLI/server/lib | `DbLogger`, `Engine::exec` | JSONL rotado | `dblog_engine.rs` | ADR-0094 | Verificado |
| MCP query/execute | stdio MCP | dispatch/HTTP client | Vía server | tests locales del binario | ADR-0010 | Verificado |
| Vector search MCP | tool MCP | `vector_search` | TEXT/JSON leído por SQL | unitarias MCP | ADR-0011 | Verificado |
| Admin web | Navegador/PHP | `phpgabyadmin/index.php` | Vía HTTP | lint PHP; browser E2E no identificado | README admin | Parcial |
| Modelador | Navegador/Tauri | `modeler/index.html` | Modelo cliente/SQL | pruebas E2E no identificadas | manual modeler | Parcial |
| Release/deploy | Actions/Docker | workflows/Dockerfile | Artefactos/volumen | CI | `RELEASE`, `INSTALL` | Verificado en definición; entorno real requiere validación |

“Verificado” significa que existe implementación y evidencia local; no certifica producción. Las referencias ADR son históricas y deben conservarse.
