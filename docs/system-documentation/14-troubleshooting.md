# 14. Resolución de problemas

| Síntoma | Causa probable | Diagnóstico y solución | Riesgo |
|---|---|---|---|
| `link.exe`/`kernel32.lib` ausente | MSVC incompleto | Instalar Build Tools + Windows SDK o usar release/Docker | Ninguno sobre datos |
| `[GBY-1002] database locked` | Otro proceso/Pager abrió la DB | Identificar y detener dueño; no borrar locks a ciegas | Forzar acceso puede corromper |
| `bad magic` | Archivo no gabysql | Confirmar ruta; restaurar copia correcta | No sobrescribir el original |
| `unsupported ... version` | Formato incompatible | Consultar `COMPATIBILITY.md`; convertir/restaurar | Apertura forzada no soportada |
| CRC/WAL checksum | Corrupción/truncado | Detener escrituras, preservar evidencia, `verify`, restaurar | Puede perder cambios posteriores al backup |
| `401 unauthorized` | Token ausente/incorrecto | Revisar headers y configuración sin imprimir secreto | Rotar token si se expuso |
| `server busy` | Tope de conexiones | Revisar métricas; backoff; ajustar `-max-connections` con pruebas | Más threads/recursos |
| DB no válida en `-dir` | Nombre rechazado | Usar nombre simple y endpoint correcto | Evita traversal |
| Sesión transaccional desaparece | Idle > 300 s o cierre | Revisar flujo/session ID; repetir transacción | Operación pudo hacer rollback |
| Admin no conecta | URL/allowlist/token | Revisar `GABYADMIN_SERVER`, remoto y health | No habilitar remoto sin proxy |
| Log no abre | Ruta/permisos | Probar directorio y espacio; corregir `-log-file` | El SQL del log es sensible |
| Consulta lenta | Scan/stats stale | `EXPLAIN`, `ANALYZE`, índice apropiado, gabybench | Índices cuestan escritura/espacio |

Comandos seguros de diagnóstico:

```powershell
gabysql info demo.db
gabysql verify demo.db
cargo test --all-targets
Invoke-WebRequest -UseBasicParsing http://localhost:8080/health
Invoke-WebRequest -UseBasicParsing http://localhost:8080/metrics
```

Antes de reparar, copie los artefactos de incidente de forma consistente y consulte [RUNBOOK](../../RUNBOOK.md) y [INCIDENTS](../INCIDENTS_2026-05-25.md). No ejecute `init --force`, restore `--force` ni elimine WAL sin comprender qué sobrescribirá.
