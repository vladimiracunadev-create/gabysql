# ADR-0094: Log de sentencias y errores en el motor, no en cada frontend

**Fecha:** 2026-08-13
**Estado:** Aceptado
**Bloque:** L — log de sentencias del motor.
**Complementa:** [ADR-0012](0012-audit-log-enriquecido.md) (audit log del gateway MCP). [ADR-0014](0014-logs-json-metrics.md) (logs JSON + `/metrics` del server).
**Respeta:** [ADR-0001](0001-rust-zero-deps-core.md) (cero deps runtime).

## Contexto

Todo motor de base de datos serio persiste dos cosas que gabysql no
persistía: **qué se ejecutó** y **qué falló**. Lo que había era esto:

| Capa | Qué captura | Dónde | Cobertura |
| :--- | :--- | :--- | :--- |
| `gabysql-mcp --audit-log` (ADR-0012) | SQL + `reason` + `clientInfo` + ok/error | archivo JSONL | **sólo tráfico MCP** |
| `gabysql-server -log-json` (ADR-0014) | method, path, status, latency | stdout | **sin SQL ni código de error** |
| `gabysql` CLI / REPL | — | — | nada |
| Uso embebido (lib) | — | — | nada |
| `.msi` de escritorio | — | — | nada |

El agujero concreto: `DbError` ([src/lib.rs](../../src/lib.rs)) es un
`struct` con un solo `String`. Se propaga hacia arriba y **muere en
quien lo llame** — el CLI lo imprime a stderr, el server lo serializa a
JSON y responde 400. Nadie lo escribe a ningún lado. Un `POST /exec` que
revienta con `[GBY-3001]` aparece en el log JSON del server como
`{"status":400}`, sin decir qué sentencia fue ni qué código dio.

Traducido al vocabulario de PostgreSQL: existía el log del **transporte**
y faltaba el equivalente a `log_statement` / `log_min_error_statement`,
que es el log de **la base**.

Restricciones:

- ADR-0001: cero deps. Nada de `tracing`, `log`, `slog`.
- El default debe seguir siendo un binario silencioso.
- El motor tiene 5 frontends (CLI, REPL, server HTTP, gateway MCP,
  embebido) y crecerá: la solución no puede ser "instrumentar cada uno".

## Decisión

Módulo nuevo [`src/dblog.rs`](../../src/dblog.rs) — sink JSONL
append-only con rotación por tamaño — enganchado en **un solo punto**:
`Engine::exec`.

### 1. Por qué `Engine::exec` y no los frontends

`Engine::exec` es un `match` de dispatch único por el que pasa **toda**
sentencia, venga de donde venga. Instrumentarlo cubre de una sola vez el
CLI, el REPL, el server, el gateway y cualquier embebido futuro, sin que
ninguno de ellos tenga que enterarse.

El costo de esa decisión es un detalle no obvio: `exec` se llama
**recursivamente a sí mismo** desde 12 sitios de `sql.rs` (bodies de
trigger, procedure, function, `WHILE`/`LOOP`/`FOR`, handlers de
`EXCEPTION`). Sin guard, un `CALL` con un loop de 1000 iteraciones
escribiría 1000 líneas por **una** sentencia del usuario. Por eso el
`Engine` lleva un `exec_depth` y sólo se loguea el nivel 0. Hay dos tests
E2E dedicados a esto (`nested_exec_from_a_procedure_loop_...` y
`nested_exec_from_a_trigger_...`).

### 2. El texto SQL lo aporta el caller

`Engine::exec` recibe un `Statement` **ya parseado**: para cuando llega,
el texto original se perdió. El caller que sí lo tiene lo declara con
`Engine::set_log_source(&sql)`, y cada entrada queda con el texto del
batch completo más un `stmt_index` que desambigua cuál de las sentencias
del batch la generó.

Consecuencia directa: **los errores del parser no pasan por el hook**.
Un `SELEKT oops` falla en `parse()`, antes de que exista un `Statement`.
Se loguean explícitamente desde el frontend con `kind: "PARSE"` — en
[src/server.rs](../../src/server.rs) (`log_parse_error`) y en
[src/bin/gabysql.rs](../../src/bin/gabysql.rs). Es asimétrico y feo, pero
la alternativa (que `parse` devuelva spans por sentencia) es una cirugía
al parser que no se justifica por esto.

### 3. Niveles

Espejan `log_statement` de PostgreSQL, con `error` agregado abajo:

| Nivel | Qué entra |
| :--- | :--- |
| `none` | nada |
| `error` (**default**) | sólo sentencias que fallaron |
| `mod` | errores + sentencias que cambian estado |
| `all` | todo, incluidos los `SELECT` |

`error` es el default porque el caso de uso dominante ("quiero ver qué
falló") no debería costar el volumen de `all`.

**Qué cuenta como "cambia estado"** para el corte de `mod` — la función
`statement_kind` en `sql.rs` es la fuente de verdad, y clasifica tres
cosas, no sólo escrituras de datos:

1. **Estado durable**: DDL, DML, y `ANALYZE` (persiste un record
   `TableStats` en el catálogo desde P3b).
2. **Estado transaccional**: `BEGIN`/`COMMIT`/`ROLLBACK`/savepoints.
   PostgreSQL los excluye de `log_statement=mod`; acá se **incluyen a
   propósito**, porque sin el boundary de commit el log no permite
   reconstruir *qué quedó aplicado* — que es exactamente lo que se le
   pide a un log de auditoría.
3. **Contexto de seguridad**: `SET SESSION AUTHORIZATION` no toca un byte
   del disco, pero cambia bajo qué identidad se evalúan los privilegios
   de todo lo que sigue.

Las sentencias de control de flujo (`CALL`, `BEGIN…END`, `IF`, loops,
`CASE`) se marcan como cambio de estado de forma **conservadora**: su
body puede contener DML y desde afuera no se sabe. Un falso positivo
cuesta una línea de log de más; un falso negativo pierde una escritura
del log de auditoría.

`EXPLAIN` pelado es de sólo lectura, pero `EXPLAIN ANALYZE` **ejecuta**
el statement interno y por lo tanto hereda su clasificación.

### 4. Rotación por tamaño

ADR-0014 descartó la rotación con el argumento "stdout + `logrotate`
resuelve". Ese argumento no aplica acá y es la diferencia operativa
central de esta ADR: en el uso **embebido** y en el `.msi` de escritorio
no hay supervisor, no hay `logrotate` y no hay nadie rotando nada — el
archivo crecería sin techo en la máquina de un usuario final.

Default: 8 MiB por archivo, 3 rotados (`.log.1`, `.log.2`, `.log.3`) →
techo de ~32 MiB. `max_bytes = 0` desactiva la rotación.

El handle se cierra **antes** de renombrar: Windows rechaza el rename de
un archivo con handles abiertos. El tamaño se siembra desde el
`metadata()` real al abrir, para que un reinicio no reinicie el conteo.

### 5. Formato

Una línea JSON por sentencia, con `\n` final:

```json
{"v":1,"ts_unix":1786655967,"kind":"INSERT","mutating":true,"stmt_index":0,"ok":false,"rows":0,"duration_us":136,"sql":"INSERT INTO users (id,name) VALUES (1,'Duplicada');","code":3001,"error":"[GBY-3001] PRIMARY KEY duplicada: la clave 1 ya existe en la tabla"}
```

El campo `v` va **desde el día uno**. ADR-0012 anotó como consecuencia
negativa el no haber versionado el shape del audit log del gateway; esa
deuda no se repite acá.

`code` se extrae del prefijo `[GBY-NNNN]` y se omite cuando el error no
lo trae — todavía quedan errores construidos con `DbError::new` pelado
(el `PARSE` del smoke test de abajo es uno).

### 6. Configuración

| | `gabysql-server` | `gabysql` CLI |
| :--- | :--- | :--- |
| Archivo | `-log-file P` / `GABYSQL_LOG_FILE` | `GABYSQL_LOG_FILE` |
| Nivel | `-log-level L` / `GABYSQL_LOG_LEVEL` | `GABYSQL_LOG_LEVEL` |
| Rotación | `GABYSQL_LOG_MAX_BYTES`, `GABYSQL_LOG_MAX_FILES` | idem |

El CLI usa argumentos posicionales (`gabysql exec <db> <sql...>`), así que
flags ahí obligarían a distinguirlas del SQL. Por env queda limpio, y
además es la vía natural para el `.msi` y para wrappers que lanzan el
binario.

Sin configuración, el `Engine` no tiene logger y **no paga ni un
`Instant::now()`** — el camino sin log es una rama antes de cualquier
trabajo.

## Verificación

Smoke E2E con el binario real, `GABYSQL_LOG_LEVEL=mod`:

```
--- intento de PK duplicada (debe fallar) ---
error: [GBY-3001] PRIMARY KEY duplicada: la clave 1 ya existe en la tabla
--- SELECT (no debe loguearse en level=mod) ---
--- SQL que no parsea ---
error: sentencia no soportada (solo CREATE/INSERT/SELECT/...)
===== contenido de gabysql.log =====
{"v":1,...,"kind":"CREATE TABLE","mutating":true,"stmt_index":0,"ok":true,"rows":0,"duration_us":101,"sql":"CREATE TABLE users (id INT PRIMARY KEY, name TEXT);"}
{"v":1,...,"kind":"INSERT","mutating":true,"stmt_index":0,"ok":true,"rows":0,"duration_us":126,"sql":"INSERT INTO users (id,name) VALUES (1,'Ana');"}
{"v":1,...,"kind":"INSERT","mutating":true,"stmt_index":0,"ok":false,"rows":0,"duration_us":136,"sql":"INSERT INTO users (id,name) VALUES (1,'Duplicada');","code":3001,"error":"[GBY-3001] PRIMARY KEY duplicada: la clave 1 ya existe en la tabla"}
{"v":1,...,"kind":"PARSE","mutating":false,"stmt_index":0,"ok":false,"rows":0,"duration_us":0,"sql":"SELEKT oops;","error":"sentencia no soportada (solo CREATE/INSERT/SELECT/...)"}
```

El `SELECT` no aparece (correcto para `mod`), el fallo trae `code` y SQL
completo, y el error de parseo quedó capturado con `kind: "PARSE"`.

## Tests

- **12 unit** en [`src/dblog.rs`](../../src/dblog.rs): matriz de niveles,
  JSONL válido línea a línea, escape de comillas/newlines dentro del SQL,
  rotación con techo y descarte del más viejo, siembra de tamaño al
  reabrir, creación de directorios padre, `extract_code`.
- **9 E2E** en [`tests/dblog_engine.rs`](../../tests/dblog_engine.rs):
  motor sin logger no crea archivo; `error` captura `[GBY-3001]` y saltea
  los éxitos; `mod` loguea DDL/DML pero no `SELECT`; `all` loguea
  `SELECT` con `rows`/`duration_us`; `stmt_index` dentro de un batch;
  **guard de anidamiento** con procedure+loop y con trigger; control
  transaccional en `mod`; `EXPLAIN` pelado no es mutación.

## Alternativas descartadas

**Tabla de sistema `__gabysql_log` dentro de la DB.** Coherencia
transaccional con la escritura que registra. Pero: bump de formato
on-disk, y el problema fatal — si la transacción hace rollback, el log
del error se va con ella. Justo el caso que más importa es el que se
pierde.

**Extender `-log-json` de ADR-0014 con el SQL.** Cubre sólo el server.
Deja fuera CLI, embebido y desktop, que es donde más falta hace.

**Sink a stdout en vez de archivo.** Es lo que ya hace ADR-0014 y
funciona para el server bajo supervisor. No funciona para el `.msi` ni
para el uso embebido, donde no hay nadie escuchando stdout.

**Crate `tracing`.** El ecosistema canónico, y 10-20 deps transitivas.
Viola ADR-0001 por una línea JSON por sentencia.

## Consecuencias

**Positivas**

- Un solo enganche cubre los 5 frontends, presentes y futuros.
- El código `[GBY-NNNN]` queda persistido y es filtrable
  (`jq 'select(.code == 3001)'`).
- Rotación incorporada → el desktop y el embebido no acumulan sin techo.
- Opt-in con overhead cero cuando está apagado.
- Formato versionado (`v`) desde el inicio.
- `server.rs` deja de mantener su propia copia de `json_string`: delega
  en `dblog::json_escape`.

**Negativas / a vigilar**

- **El SQL completo queda en texto plano en disco.** Los valores de un
  `INSERT`/`UPDATE` — datos personales, hashes, tokens — van al archivo
  tal cual. Es una decisión explícita (el valor de diagnóstico lo
  justifica), pero el archivo hereda la sensibilidad de la base y debe
  tratarse con los mismos permisos. Si en el futuro hace falta, la salida
  natural es un nivel `shape` que loguee `INSERT ON users` sin valores.
- **Los errores del parser se loguean desde el frontend, no desde el
  motor.** Un embebido que llame a `parse()` por su cuenta no los va a
  registrar. Documentado arriba.
- **La rotación no es segura cross-process.** El append de una línea sí
  lo es (O_APPEND), pero dos procesos rotando el mismo archivo compiten.
  Servers concurrentes deben apuntar a archivos distintos.
- **`Mutex` global del sink.** Un `lock` por sentencia logueada. Con
  `level=error` es despreciable; con `level=all` bajo carga alta es un
  punto de serialización a medir.
- **`ANALYZE` clasificado como mutación** puede sorprender a quien espere
  la semántica de PostgreSQL. Es correcto para gabysql desde P3b, donde
  `ANALYZE` persiste stats en el catálogo.

## Referencias

- [src/dblog.rs](../../src/dblog.rs) — `DbLogger`, `LogLevel`, `LogRecord`, rotación, `json_escape`.
- [src/sql.rs](../../src/sql.rs) — `statement_kind`, `Engine::attach_logger`, `Engine::set_log_source`, wrapper de `Engine::exec` + `exec_dispatch`.
- [src/server.rs](../../src/server.rs) — `ServerConfig::logger`, `log_parse_error`.
- [src/bin/gabysql-server.rs](../../src/bin/gabysql-server.rs) — flags `-log-file` / `-log-level`.
- [src/bin/gabysql.rs](../../src/bin/gabysql.rs) — logger por env, `OnceLock` por proceso.
- [docs/ERROR_CODES.md](../ERROR_CODES.md) — rango `6000–6999`.
- Prior art: `log_statement` / `log_min_error_statement` / `log_rotation_size` de PostgreSQL; `general_log` + `slow_query_log` de MySQL.
