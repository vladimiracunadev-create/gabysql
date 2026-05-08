# ADR-0012: Audit log enriquecido en el gateway, no en el motor

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-08
**Contexto que la motiva**: cierre del trío AI-native (Fase 5) en [ROADMAP.md](../../ROADMAP.md). Implementación entregada en `src/bin/gabysql-mcp.rs`.

## 🧭 Contexto

Cuando un agente LLM puede escribir en una base, aparece un problema operacional nuevo: **¿qué hizo, cuándo, y por qué?**

El log de errores del server (stderr) y el WAL del motor responden **el qué** (qué SQL se ejecutó) pero no **el por qué** (qué pidió el usuario al agente, qué identidad tenía el agente, qué razonamiento llevó a esa escritura). Para auditar workflows con agentes hace falta más metadata, y meter eso en el motor implica:

- Un nuevo tipo de página o tabla de sistema para el log → bump de formato.
- Cambios en `Engine::exec` para capturar y persistir la metadata → ondas en `sql.rs`.
- API HTTP nueva para que el cliente le pase la metadata al server.
- Y, sobre todo, el motor termina sabiendo qué es un "agente", "una razón semántica", "un clientInfo" — conceptos que pertenecen al protocolo MCP, no a SQL.

Es exactamente la misma trampa que [ADR-0010](0010-mcp-gateway.md) y [ADR-0011](0011-vector-search-gateway-side.md) ya esquivaron: meter en el motor cosas que pertenecen al gateway.

Restricciones del proyecto:
- Cero deps externas en el core ([ADR-0001](0001-rust-zero-deps-core.md)).
- Sin bump de formato en disco si no es estrictamente necesario.
- Cualquier feature AI-native debe seguir la línea del gateway: el motor **no se toca**.

## 💡 Decisión

Implementar audit log enriquecido **en el gateway MCP**, no en el motor. Concretamente:

1. Nuevo flag `--audit-log <ruta>` (también `GABYSQL_AUDIT_LOG`). Si no se pasa, **no hay log** — overhead cero, semántica idéntica al gateway pre-ADR.
2. Cuando el log está activo, cada llamada a una tool **mutadora** (`gabysql_execute`, `gabysql_integrity_check`) anexa una línea JSON al archivo (formato JSONL: una entrada por línea, append-only).
3. La entrada captura:
   - `ts_unix`: epoch seconds.
   - `tool`: nombre de la tool MCP.
   - `db`: archivo `.db` afectado (o `null` en single-db).
   - `sql`: la sentencia SQL ejecutada.
   - `reason`: **el "por qué" semántico** que el agente pasa como argumento opcional de la tool. Es el campo central de esta ADR — convierte el log en algo distinto a un log de SQL.
   - `client`: `clientInfo` (`name` + `version`) capturado en `initialize`. Identifica qué agente hizo qué.
   - `ok`: si la llamada al motor tuvo éxito.
   - `error`: mensaje de error si `ok=false`.
4. Nueva tool `gabysql_audit_tail(n)` que devuelve las últimas N entradas. Permite que **el propio agente revise sus acciones** ("¿qué he escrito en esta DB en las últimas horas?"). Si el log no está activo, la tool devuelve `{"enabled":false,"entries":[]}` sin error.
5. Las lecturas (`gabysql_query`, `gabysql_describe_database`, `gabysql_list_databases`, `gabysql_vector_search`) **no se loguean por defecto**. El criterio: el audit log existe para responder "¿qué se escribió y por qué?", no "¿qué se leyó?". Logguear lecturas multiplicaría el volumen sin proporción al valor.
6. El append es **best-effort**: si escribir al archivo falla (permisos, disco lleno), se loguea a `stderr` y la tool continúa devolviendo lo que el motor le dio. La alternativa (rechazar la tool si no se puede auditar) es más estricta pero implica que un disco lleno bloquea todas las escrituras, lo cual es peor para la operación.

Resultado: una traza completa de la actividad de los agentes sin que el motor cambie en una sola línea.

## 🔄 Alternativas consideradas

### Tabla de sistema en el motor (ej. `__gabysql_audit`)
- **Pro**: el log vive en la misma DB, transaccionalmente coherente con la escritura que registra. Si la transacción rollback-ea, el log también.
- **Contra**: bump de formato. Cambios en `Engine::exec`. El motor empieza a entender conceptos MCP. Y la coherencia transaccional, en la práctica, no es tan valiosa: los agentes no usan transacciones explícitas y el `reason` es metadata externa al motor.
- **Veredicto**: rechazada — mismo razonamiento que en ADR-0010 y ADR-0011.

### Logging structurado en el `gabysql-server` (no en el gateway)
- **Pro**: captura todas las escrituras, vengan o no por MCP. Útil si en el futuro hay otros clientes.
- **Contra**: el server no sabe nada de `clientInfo` ni `reason` — esos conceptos viajan en el protocolo MCP, no en HTTP/JSON. Para que los reciba, habría que extender el payload de `/exec` con campos opcionales. Eso es razonable pero contamina el endpoint que ya existe y ya está estable. Y deja al gateway sin manera de loguear sus tools internas (vector_search, audit_tail).
- **Veredicto**: rechazada por ahora. Si en el futuro aparecen otros clientes mutadores no-MCP, esta ADR queda complementada (no superseded) por una ADR-X de logging server-side.

### Sidecar (proceso aparte) escuchando los `tools/call`
- **Pro**: aísla aún más.
- **Contra**: tres procesos en lugar de dos. Sobre-ingeniería para algo que cabe en ~150 líneas dentro del binario que ya existe.
- **Veredicto**: rechazada.

### **JSONL append-only en el gateway, opt-in por flag** (decisión)
- **Pro**: cero impacto en el motor. Cero deps. Activación opt-in (sin flag = comportamiento idéntico al gateway pre-ADR). El formato JSONL es trivial de procesar con `jq`, `grep`, `tail`, o ingestar en cualquier herramienta de log analytics. La tool `gabysql_audit_tail` cierra el loop dándole al agente acceso a su propia historia.
- **Contra**: el log no es transaccional con la escritura — si el motor commitea y luego falla el append, hay un escrita sin entrada. La probabilidad real es bajísima (escribir una línea a un archivo local) y la consecuencia es "perdiste una entrada del log", no "corrompiste la DB". Aceptable.
- **Veredicto**: **aceptada**.

## 📊 Consecuencias

### Positivas
- **El "por qué" semántico queda capturado.** Es la diferencia central entre un log de SQL y un audit log de agentes.
- **Identidad del cliente queda en cada entrada.** Saber que la escritura vino de `claude-desktop@1.2.3` vs `cursor@0.42` cambia el análisis post-hoc.
- **Cero impacto en el motor.** Sin bump de formato, sin cambios en `storage.rs`/`bptree.rs`/`sql.rs`/`catalog.rs`/`server.rs`.
- **Opt-in sin overhead por defecto.** Quien no use `--audit-log` no paga nada (ni un syscall extra).
- **JSONL es procesable por todo el ecosistema** (`jq`, `tail -f`, ingesta en S3/BigQuery/ELK). No inventamos formato.
- **`gabysql_audit_tail` es novedoso**: la mayoría de audit logs son consumidos por humanos *después*. Que el propio agente pueda releer su historial dentro de una sesión abre patrones de auto-corrección y verificación.

### Negativas
- **No es transaccional con el motor.** Si el motor commitea y el filesystem falla después, queda una escritura sin entry. Probabilidad real: muy baja. Mitigación: errores de append van a stderr para que la operación los note.
- **Solo captura tráfico que pasa por MCP.** Una escritura hecha con `curl` directo al `gabysql-server` no aparece en el log. Esto es por diseño — el gateway no es el único punto de entrada al motor — pero hay que documentarlo en RUNBOOK.
- **Formato no versionado.** Las entradas son JSON con campos fijos pero sin campo `version`. Si en el futuro cambiamos el shape, los consumidores rompen. Mitigación: añadir `schema_version` la primera vez que se cambie un campo, no antes (YAGNI).
- **El log puede crecer sin límite.** No hay rotación incorporada. El operador resuelve con `logrotate` o equivalente. Documentar en RUNBOOK.

### Neutras
- El binario `gabysql-mcp` crece ~200 líneas (AuditEntry, append, tail, runtime state, capture en initialize). Sin nuevas deps.
- `dispatch` y `handle_tools_call` ahora reciben `&Mutex<RuntimeState>`. Cambio interno; el contrato MCP visible al cliente no cambia (las tools nuevas son aditivas, las viejas son compatibles).
- El campo `reason` en `gabysql_execute` es opcional y retrocompatible: clientes viejos que no lo pasan siguen funcionando.

## 🚪 Condiciones de salida

Esta ADR queda **complementada** (no superseded) por una ADR futura cuando ocurra alguno de:

- Volumen de tráfico mutador justifica almacenamiento estructurado (rotación, compactación, índice por fecha) — entonces se evalúa SQLite/Parquet/Loki como sink alternativo del gateway.
- Aparecen clientes mutadores no-MCP que necesitan ser auditados — se abre una ADR de logging server-side, complementaria a esta.
- Necesidad de coherencia transaccional con el commit del motor — se reabre la opción de tabla de sistema interna.

Hasta entonces, JSONL append-only en el gateway es el balance correcto.

## 🔗 Referencias

- Implementación: [src/bin/gabysql-mcp.rs](../../src/bin/gabysql-mcp.rs) (`AuditEntry`, `audit_append`, `audit_tail`, `RuntimeState`, `ClientInfo`, captura en `handle_initialize`).
- Tools/flags nuevos: `--audit-log`, env `GABYSQL_AUDIT_LOG`, argumento `reason` en `gabysql_execute`, tool `gabysql_audit_tail`.
- Tests: módulo `#[cfg(test)] mod tests` del binario — 5 tests nuevos cubren captura de clientInfo, append+tail roundtrip, comportamiento sin log activo, presencia de la tool en `tools/list`, formato JSONL (una entrada por línea, JSON válido por línea).
- ADRs encadenadas: cierra el trío AI-native — [ADR-0010](0010-mcp-gateway.md) (gateway base), [ADR-0011](0011-vector-search-gateway-side.md) (vectores), **ADR-0012** (audit). Las tres siguen [ADR-0001](0001-rust-zero-deps-core.md) (cero deps en core) y [ADR-0007](0007-commercial-path-a.md) (camino A — embebido nicho con extensiones AI sobre el gateway).
- Prior art:
  - **PostgreSQL `pgaudit`**: extensión que loguea actividad en el motor — el camino "correcto" pero requiere ser parte del motor.
  - **AWS CloudTrail**: log de "quién hizo qué" sobre APIs administrativas — captura `clientInfo` equivalente. Mismo patrón aplicado a una superficie distinta.
  - **OpenTelemetry semantic conventions for AI agents**: el campo `reason` aquí es un primer paso hacia ese estándar, simplificado al contexto MCP/SQL.
