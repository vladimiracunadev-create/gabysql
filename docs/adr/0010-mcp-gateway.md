# ADR-0010: Gateway MCP como adaptador externo sobre el HTTP/JSON existente

**Estado**: 🟡 Propuesta
**Fecha**: 2026-05-07
**Contexto que la motiva**: apertura de Fase 5 (AI-native) en [ROADMAP.md](../../ROADMAP.md). Sin commit todavía; este ADR precede a la implementación para fijar el contrato antes de escribir código.

## 🧭 Contexto

`gabysql` se posiciona hoy como base embebida tipo SQLite (ver [ADR-0007](0007-commercial-path-a.md)). El producto se consume por dos rutas:

1. **CLI single-process** (`gabysql.rs`) → SQL directo contra un archivo `.db`.
2. **Server HTTP/JSON** (`gabysql-server.rs`) → POST `/sql` con `{ "sql": "..." }`, autorización opcional vía bearer token, tope de conexiones simultáneas configurable.

Ambos hablan SQL. Ambos asumen que el cliente sabe qué tablas existen, qué columnas tiene cada una, qué índices son útiles, y cómo se forma un `WHERE` válido para el subset SQL que `gabysql` soporta hoy.

El consumidor que está creciendo más rápido en el ecosistema **no cumple ninguno de esos supuestos**: agentes LLM (Claude, Cursor, Copilot, asistentes propios) que necesitan descubrir el schema, generar SQL, ejecutarlo y leer resultados sin un humano traduciendo entre lenguaje natural y SQL en cada vuelta. Hoy ese consumidor tiene que:

- Abrir un cliente HTTP con un token,
- Memorizar la forma del payload (`{"sql": "..."}` vs `{"sql": "...", "db": "..."}`),
- Mantener fuera del modelo el catálogo de tablas (porque el LLM no lo conoce salvo que se lo pegues en el prompt),
- Reintentar errores SQL sin trazabilidad estructurada.

Cada agente reimplementa este pegamento. El protocolo **MCP (Model Context Protocol)** es la respuesta estándar emergente a ese problema: define cómo un servidor expone *tools* (funciones invocables), *resources* (datos legibles) y *prompts* a cualquier cliente compatible (Claude Desktop, Claude Code, Cursor, etc.) sobre stdio o HTTP. Si `gabysql` habla MCP de fábrica, cualquier agente lo enchufa directo sin código de pegamento.

Restricciones que esta decisión debe respetar:

- **No tocar el core.** [ADR-0001](0001-rust-zero-deps-core.md) fija cero dependencias externas en el motor; MCP requiere dependencias (al menos JSON-RPC, schema validation, y probablemente un crate `mcp-sdk` o equivalente). Estas dependencias **no pueden infiltrarse** en `storage.rs`, `bptree.rs`, `catalog.rs` ni `sql.rs`.
- **Compatibilidad del formato en disco intacta.** No hay bump de VERSION; no hay nuevo tipo de página; no hay cambio en el WAL.
- **Modelo transaccional intacto.** El gateway no abre conexiones nuevas al Pager — reusa el server HTTP que ya tiene `write_lock` global y tope de conexiones (ver [src/server.rs:46](../../src/server.rs)).
- **Authz sigue siendo del server.** El bearer token configurable en `gabysql-server -token <T>` se reutiliza tal cual; el gateway lo propaga.

## 💡 Decisión

Implementar **`gabysql-mcp`** como un **binario adaptador separado** que:

1. Vive en `src/bin/gabysql-mcp.rs`, junto a `gabysql.rs` y `gabysql-server.rs`.
2. Habla el protocolo MCP por **stdio** (transporte primario, el que usan Claude Desktop y Claude Code) y opcionalmente HTTP (transporte secundario para integraciones server-to-server).
3. Internamente es un **cliente del HTTP/JSON existente** (`POST /sql` contra `gabysql-server`). No abre el `.db` directamente, no toca el Pager, no instancia un Engine.
4. Expone un set acotado y estable de **tools MCP**:
   - `gabysql_list_databases` → wrap de `SHOW DATABASES`.
   - `gabysql_describe_database(db)` → wrap de `SHOW TABLES` + `DESCRIBE` por tabla, devuelto como un único bundle JSON estructurado (no SQL crudo).
   - `gabysql_query(db, sql)` → wrap de `POST /sql` para `SELECT`/`SHOW`/`DESCRIBE`.
   - `gabysql_execute(db, sql)` → wrap de `POST /sql` para `INSERT`/`UPDATE`/`DELETE`/DDL, separado por seguridad (un cliente puede tener un `gabysql-mcp` configurado en modo read-only y el binario rechaza la tool de escritura antes de tocar la red).
   - `gabysql_integrity_check(db)` → wrap de `INTEGRITY CHECK`.
5. Expone como **resources MCP** (legibles vía URI, sin invocar tool):
   - `gabysql://schema/<db>` → schema completo en JSON, cacheado en el gateway con invalidación por TTL corto (default 30s).
   - `gabysql://catalog` → lista de DBs disponibles.
6. Las dependencias externas (JSON-RPC, MCP SDK, etc.) viven **solo** en el target binario `gabysql-mcp` — `Cargo.toml` las declara como `[[bin]] required-features` o detrás de un feature flag opcional `mcp`. El crate library (`src/lib.rs`) sigue con cero dependencias externas.

Diagrama:

```
┌───────────────────┐   stdio MCP   ┌──────────────┐  HTTP/JSON   ┌─────────────────┐  Pager  ┌──────┐
│ Claude / Cursor / │ ────────────► │ gabysql-mcp  │ ───────────► │ gabysql-server  │ ──────► │ .db  │
│ agente cualquiera │               │ (adaptador)  │              │ (sin cambios)   │         │      │
└───────────────────┘               └──────────────┘              └─────────────────┘         └──────┘
                                          ▲
                                    cero dependencias
                                    en el core; el SDK
                                    MCP vive aquí
```

## 🔄 Alternativas consideradas

### Embeber MCP dentro de `gabysql-server`
- **Pro**: un solo proceso; el agente se conecta directo sin doble salto.
- **Contra**: rompe [ADR-0001](0001-rust-zero-deps-core.md). Las dependencias del SDK MCP (serde derivado del JSON-RPC, posiblemente tokio si se usa transport async) quedarían linkeadas al mismo binario que abre el `.db`. Cualquier CVE en esas deps obliga a republicar el motor de la DB. Inaceptable para un producto cuyo eje comercial es "supply-chain mínima" (ver [ADR-0007](0007-commercial-path-a.md)).
- **Veredicto**: rechazada.

### Implementar MCP a mano sin SDK, dentro del server
- **Pro**: cero dependencias, embebido, un proceso.
- **Contra**: MCP es un protocolo vivo (la spec evoluciona en el repo `modelcontextprotocol/specification`). Mantener un parser JSON-RPC + state machine + capability negotiation a mano es 2-3 KLOC de código de protocolo que no aporta al diferencial de gabysql. Es exactamente el tipo de superficie que conviene delegar al SDK del estándar.
- **Veredicto**: rechazada.

### Adaptador en otro lenguaje (Python/Node)
- **Pro**: SDKs MCP en Python y TypeScript son los más maduros; menos código a escribir.
- **Contra**: añade un runtime extra al deploy del usuario (Python o Node). Hoy `gabysql` se entrega como binario único; meter "instala Python 3.11+" para usar el modo agente lo desperfila completamente. Además, `gabysql-mcp` querrá compartir el formato de errores y el parsing de respuestas con el resto del proyecto — más fácil en Rust.
- **Veredicto**: rechazada.

### **Binario Rust separado, cliente del HTTP existente** (decisión)
- **Pro**: cero impacto en el core. Las dependencias del SDK MCP están aisladas en su propio target. Reutiliza el authz (bearer token), el rate limiting (max-connections), el `write_lock` y el journal/WAL del server sin tocarlos. Si MCP cambia o muere, se borra el binario y el resto del producto sigue intacto. Fácil de versionar independientemente.
- **Contra**: doble salto stdio→HTTP→Pager (latencia adicional de ~100µs–1ms por llamada loopback). Para workloads de agente — donde el cuello de botella es el LLM, no la DB — es invisible. El día que importe se evalúa un transport directo.
- **Veredicto**: **propuesta**.

## 📊 Consecuencias

### Positivas
- **gabysql se vuelve enchufable a cualquier agente MCP-compatible sin escribir código intermedio.** Diferencial real frente a SQLite, DuckDB y Postgres, que requieren un wrapper MCP escrito por el integrador.
- **Cero riesgo para el core.** El motor sigue auditable y libre de dependencias externas; el ADR-0001 se mantiene íntegro.
- **Authz, rate-limit y durabilidad son del server, no del gateway.** El gateway no puede corromper datos porque no toca el Pager.
- **Path de adopción incremental**: usuarios actuales del HTTP/JSON o CLI no cambian nada. Quienes quieran modo agente lanzan `gabysql-mcp` adicionalmente.
- **Fundación para Fase 5 completa**: el mismo gateway puede sumar herramientas semánticas más adelante (búsqueda vectorial, audit log enriquecido) sin volver a tocar el motor.

### Negativas
- **Latencia extra por el doble salto** stdio→HTTP→Pager. Esperable: cientos de µs a 1 ms por llamada en loopback. Aceptable para el use case (agente con LLM en el loop).
- **Operacionalmente son dos procesos**: el server HTTP y el gateway MCP. El RUNBOOK tiene que documentar el modelo de despliegue (un gateway por agente, server compartido).
- **Dependencias externas entran al árbol del workspace**, aunque aisladas en el binario. Hay que vigilarlas con `cargo deny` y el flujo de [ADR-0006](0006-grype-only-fixed.md) — y mantenerlas mínimas (objetivo: <10 deps directas en el gateway).
- **El gateway introduce una superficie de ataque nueva** (proceso que habla a Internet vía MCP, autentica con el server). El threat model se documenta antes del primer release del gateway.

### Neutras
- El binario nuevo aparece en CI multi-OS pero no afecta los tests del core (los suyos van separados).
- `Cargo.toml` gana una sección `[features]` y un `[[bin]]` adicional; cambio puramente aditivo.
- El gateway puede correr en una máquina distinta del server (cualquier transport HTTP), abriendo un patrón de "agente local, DB remota" sin cambios en el motor.

## 🔗 Referencias

- Spec MCP: <https://modelcontextprotocol.io/specification>
- Repo del estándar: <https://github.com/modelcontextprotocol/specification>
- ADRs encadenadas: [ADR-0001](0001-rust-zero-deps-core.md) (cero deps en core, este ADR la respeta vía aislamiento por binario), [ADR-0007](0007-commercial-path-a.md) (camino A — el modo agente refuerza el nicho embebido, no lo abandona).
- Implementación pendiente: `src/bin/gabysql-mcp.rs` + sección `[features.mcp]` en `Cargo.toml`. Bloque siguiente del roadmap.
- Prior art: el patrón "MCP server como wrapper de un servicio existente" es el dominante hoy (filesystem, GitHub, Slack, Postgres mcp servers todos siguen este shape).
