# 15. Riesgos y deuda técnica

Este inventario es informativo; no modifica comportamiento. “Potencial” no significa vulnerabilidad confirmada.

| Hallazgo | Severidad | Prob. | Evidencia/ubicación | Recomendación | Prioridad |
|---|---|---:|---|---|---:|
| HTTP sin TLS integrado | Alta fuera de localhost | Alta si se expone | `src/server.rs`, `SECURITY.md` | Proxy TLS, firewall, token y red privada | P0 despliegue |
| Formato histórico no universalmente compatible | Alta | Media | `COMPATIBILITY.md`, `storage.rs` | Matriz upgrade, migrador y backups verificados | P0 producto |
| Single-writer limita concurrencia | Media | Alta según carga | lock en `storage.rs` | Mantener posicionamiento embebido; medir antes de rediseñar | P1 |
| SQL/PII en logs | Alta | Media | `dblog.rs`, audit MCP | Permisos, retención, redacción y rotación | P0 operación |
| Server/parser/engine implementados sin crates | Media | Media | `server.rs`, `sql.rs`, ADR-0001 | Fuzzing y revisión continua; preservar límites | P1 |
| `sql.rs` concentra ~27k líneas | Media | Alta | Conteo del commit | Refactor incremental con pruebas, sin mezclar semánticas | P1 |
| Dos harnesses de benchmark | Baja | Media | `gabybench.rs`, `gabysql-bench.rs` | Documentar dueño/objetivo o consolidar salidas | P2 |
| UI web monolítica | Media | Media | `phpgabyadmin/index.php`, `modeler/index.html` | Tests navegador y separación gradual | P2 |
| Sin SLO/alertas/restore drill automatizado | Alta | Media | No identificado en repo | Definir RPO/RTO/SLO y ejecutar simulacros | P1 |
| Dependencia remota Google Fonts | Baja/privacidad | Alta al abrir modelador | `web/modeler/index.html` | Autoalojar o documentar política CSP/privacidad | P2 |
| Worktree con resultados/logs no versionados | Baja | Alta | `git status` analizado | Política de artefactos y `.gitignore` si corresponde | P3 |

La hoja de ruta contiene trabajo futuro adicional; no se repite aquí para evitar convertir planificación histórica en defecto actual. La seguridad requiere contrastar también auditorías fechadas y [SECURITY](../../SECURITY.md).
