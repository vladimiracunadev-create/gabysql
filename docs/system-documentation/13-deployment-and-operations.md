# 13. Despliegue y operación

## Artefactos y entornos

Cargo produce CLI, server, MCP, benchmarks y utilidades. Dockerfile/Compose ofrecen el stack de servicio; Tauri empaqueta el modelador de escritorio; GitHub Actions contiene workflows de release y desktop release. Los procedimientos normativos están en [RELEASE](../../RELEASE.md), [INSTALL](../../INSTALL.md) y [RUNBOOK](../../RUNBOOK.md).

## Flujo operativo mínimo

1. Construir en release y validar formato/clippy/tests/PHP.
2. Aprovisionar directorio/volumen persistente con permisos mínimos.
3. Configurar token, address, límites y logs; colocar proxy TLS delante.
4. Arrancar server, ejecutar `/health`, `/metrics` y una consulta controlada.
5. Programar `gabysql backup` y `gabysql verify`; probar restore periódicamente.
6. Recolectar stderr, request log y statement log con retención protegida.

## Observabilidad

`GET /metrics` entrega requests, estados, errores, latencias p50/p95 y uptime. `-log-json` registra requests; `-log-file` registra SQL/resultados a nivel `error|mod|all` con rotación. No se observan Prometheus nativo, tracing distribuido, dashboard o alertas integradas: deben implementarse fuera.

## Recovery y rollback

Tras caída, abrir la DB provoca examen/replay del WAL comprometido. Después valide con `verify` o `INTEGRITY CHECK`. Para rollback de datos, restaure un backup verificado con el proceso detenido. Para rollback de binario/formato, la compatibilidad debe comprobarse antes: no asuma que una versión vieja abrirá una base creada/actualizada por una nueva.

## Mantenimiento

Vigile espacio de DB/WAL/logs, latencia, errores `[GBY-*]`, backups y expiración de sesiones. `ANALYZE` refresca estadísticas cuando el workload cambia. No se identificó automatización de migraciones o mantenimiento online; requiere validación por despliegue.
