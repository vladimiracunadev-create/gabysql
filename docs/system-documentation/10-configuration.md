# 10. Configuración

## Binarios y flags

`gabysql-server` acepta `-db` o `-dir` (mutuamente excluyentes por intención), `-addr`, `-token`, `-max-connections`, `-log-json`, `-log-file` y `-log-level`. `gabysql-mcp` configura URL de server, token, modo read-only, DB y audit log; consulte `--help` del binario compilado.

## Variables de entorno observadas

| Variable | Default/uso | Sensibilidad |
|---|---|---|
| `GABYSQL_LOG_FILE` | Ruta de statement log | Puede contener datos |
| `GABYSQL_LOG_LEVEL` | `none`, `error`, `mod`, `all` | No secreta |
| `GABYSQL_LOG_MAX_BYTES` | 8 MiB | No secreta |
| `GABYSQL_LOG_MAX_FILES` | 3 | No secreta |
| `GABYSQL_AUDIT_LOG` | Audit JSONL del MCP | Puede contener SQL/reason |
| `GABYADMIN_SERVER` | `http://localhost:8080` | No secreta |
| `GABYADMIN_ALLOW_REMOTE` | `0`; `1` habilita remoto | Aumenta superficie |
| `GABYADMIN_TOKEN` | Vacío; token de acceso | Secreto |

Los flags del server pueden prevalecer sobre variables del logger según `from_env_or_flags`. Use valores ficticios en ejemplos y secretos fuera del repositorio.

## Entornos

Desarrollo suele usar Cargo/PHP local y una DB temporal. Docker usa `/data` y el volumen `gabysql-data`. Producción requiere reverse proxy/TLS, permisos restrictivos, rutas persistentes, backup, retención y monitoreo externos; estos componentes no se provisionan desde el crate.

## Configuración incorrecta

Un `-log-file` no escribible puede abortar server; el CLI deshabilita log y avisa. `GABYADMIN_ALLOW_REMOTE=1` permite destinos no locales y exige hardening. Cache demasiado baja degrada lecturas; demasiado alta multiplica memoria por DB. `-token` vacío deja HTTP sin autenticación integrada.
