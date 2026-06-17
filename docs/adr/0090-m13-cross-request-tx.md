# ADR-0090: M13 — Cross-request transactions HTTP

**Fecha:** 2026-06-15
**Estado:** Aceptado
**Bloque:** M13 (server HTTP — sesiones tx)
**Origen:** [docs/TAREAS_PENDIENTES.md §6.5](../TAREAS_PENDIENTES.md) — declarado como "depende de M12".
**Refina:** [ADR-0089 M12](0089-m12-savepoints.md) (savepoints — habilita el caso N-puntos), Bloque T (BEGIN/COMMIT/ROLLBACK).

## Contexto

Antes de M13, el servidor HTTP de gabysql era estrictamente
auto-commit por request:

```
POST /exec  { "sql": "BEGIN; INSERT (1); ..." }   ← request 1 (commiteado)
POST /exec  { "sql": "INSERT (2); COMMIT;" }      ← request 2 (NO ve el (1))
```

Cada request abría su propio Pager, hacía `begin/exec/commit/close`.
Imposible que dos requests compartieran tx. Cualquier cliente serio
(ORMs, batch loaders, herramientas de migración) que quiere preparar
una tx larga, validar resultados intermedios, y decidir COMMIT o
ROLLBACK al final — no podía usar gabysql vía HTTP.

M12 (ADR-0089) había agregado savepoints al motor; el server seguía
sin exponerlos porque la tx misma no sobrevivía al request.

## Decisión

Agregar **sesiones cross-request** con un único slot global.

### Endpoints nuevos

```
POST /tx/begin                 → crea sesión, devuelve {"session":"<hex16>"}
POST /tx/commit?session=<id>   → COMMIT + cierra sesión
POST /tx/rollback?session=<id> → ROLLBACK + cierra sesión
```

### `/exec` extendido

Acepta session ID via header `X-Gabysql-Session: <hex16>` o query
param `?session=<id>`. Cuando está presente:

- El Engine usa el Pager de la sesión activa (no abre uno nuevo).
- **NO auto-commit al terminar el request**: la tx sigue viva.
- Error de exec → respuesta 400 con el error, sesión NO se cierra
  automáticamente (mismo comportamiento que PostgreSQL: tx queda en
  estado de error hasta ROLLBACK explícito).
- Cada request actualiza el `last_used` de la sesión.

Cuando NO hay session ID: comportamiento clásico (auto-commit por
request).

### Single-slot global

`SessionStore::current: Option<Session>`. **Máximo UNA sesión activa
a la vez** en todo el servidor.

¿Por qué single-slot?

- El Pager toma file lock (ADR-0013). Múltiples sesiones sobre la
  misma DB requieren repensar single-writer. Diferido a Fase 6 con
  WAL-mode (ADR-0018 propuesta).
- El caso target (ORMs que serializan requests a "una connection") se
  resuelve con single-slot.
- Multi-session sería un breaking change futuro — la API (header
  `X-Gabysql-Session`) sigue válida.

Si llega un `/tx/begin` con sesión ya activa → 409. Si el cliente
quiere reemplazar, debe `/tx/rollback` primero.

### Idle timeout

`SESSION_IDLE_TIMEOUT_SECS = 300` (5 min). El GC es **pasivo**:
cada request a `/tx/*` o `/exec` con session ID checkea
`last_used.elapsed() ≥ 300s` y si sí, hace rollback + drop. Sin
thread sweeper aparte.

Razón de elegir pasivo: el server es single-writer, así que cualquier
operación nueva pasa por el lock → ahí mismo se puede GC. Un sweeper
thread agregaría complejidad de scheduling sin ganancia funcional.

### Session ID generator

`fresh_session_id()` deriva 16 hex chars del clock nanosegundo +
splitmix64 mixer. **No es un token de seguridad** — solo identifica
la sesión vigente. La autenticación al server sigue via
`Authorization: Bearer <token>` o `X-Gabysql-Token` (Sec2).

## Consecuencias

### Positivas

- **ORMs ahora pueden usar el server HTTP**. SQLAlchemy/Diesel/Hibernate
  todos asumen "una connection mantiene tx state hasta COMMIT/ROLLBACK".
- **Loop completo con M12**: el cliente puede hacer `BEGIN`, varios
  `SAVEPOINT/ROLLBACK TO` parciales, y finalmente `COMMIT` — todo
  cruzando varios requests.
- **Backwards compatible**: requests sin session ID se comportan
  exactamente como antes. Cero migración para clientes existentes.
- **Cero deps nuevas**: TcpStream + std::sync::Mutex bastan. ID
  generator hand-rolled (alinea ADR-0001).

### Negativas / deuda

- **Single-slot global**: solo un cliente a la vez puede tener tx
  abierta. Multi-session real es ADR-0018 (WAL-mode).
- **Idle timeout pasivo**: si nadie hace requests, una sesión
  expirada queda en memoria hasta el próximo request. Aceptable —
  el lock no se libera al SO hasta el drop, pero memoria ≈ size del
  cache + savepoints (~4-20 MB).
- **No persiste a través de crash del server**: el state vive en
  memoria. Si el server crashea, la sesión y todos sus cambios se
  pierden. PostgreSQL hace lo mismo — es comportamiento esperado.
- **Sin pooling**: cada `/tx/begin` abre un Pager nuevo. Para
  workloads con muchas tx cortas hay overhead de file-open por tx.
  Optimización futura (pool de Pagers reutilizables).

## Tests añadidos

Cuatro tests E2E en nuevo binario `tests/m13_server.rs`. Cada uno
arranca el server real en un thread con puerto efímero
(`TcpListener::bind("127.0.0.1:0")`) y hace requests reales via
`TcpStream`:

- `m13_cross_request_tx_persists_on_commit`: BEGIN; INSERT en
  request 2; INSERT en request 3; COMMIT en request 4; verificar
  `SELECT COUNT(*)` = 2 en request 5 (sin session).
- `m13_cross_request_tx_rollback_discards`: BEGIN; INSERT;
  ROLLBACK; verificar count = 0.
- `m13_double_begin_rejected_409`: dos `/tx/begin` consecutivos → el
  segundo recibe 409.
- `m13_invalid_session_id_404`: `/exec` con session ID que no existe
  → 404.

Cero deps externas para los tests (TcpStream + parsing JSON ad-hoc).

**Suite total**: 824 → **828** (+4).

## Ejemplo de uso (curl)

```bash
# 1) Iniciar sesión.
SESSION=$(curl -sX POST http://127.0.0.1:8080/tx/begin -d '{}' \
    | jq -r '.session')

# 2) Operar varios requests dentro de la misma tx.
curl -sX POST http://127.0.0.1:8080/exec \
    -H "X-Gabysql-Session: $SESSION" \
    -d '{"sql":"INSERT INTO users (id, name) VALUES (1, \"Ana\")"}'

curl -sX POST http://127.0.0.1:8080/exec \
    -H "X-Gabysql-Session: $SESSION" \
    -d '{"sql":"INSERT INTO users (id, name) VALUES (2, \"Beto\")"}'

# 3) Decidir commit o rollback al final.
curl -sX POST "http://127.0.0.1:8080/tx/commit?session=$SESSION"
# o
# curl -sX POST "http://127.0.0.1:8080/tx/rollback?session=$SESSION"
```

## Alternativas consideradas

1. **Multi-session con file lock por DB**. Requiere repensar el lock
   del Pager (ADR-0013 lo asume exclusive). Diferido a Fase 6.
2. **Auto-creación de sesión** cuando `/exec` empieza con `BEGIN` sin
   session header. Más mágico pero menos predecible — un typo del
   cliente podía crear sesiones huérfanas. Rechazado.
3. **Sweeper thread** para idle timeout. Más correcto bajo carga
   baja, pero agrega complejidad de scheduling. Pasivo basta para
   v1 — re-evaluar si se ve memoria acumulada en producción.
4. **REST estricto**: `POST /sessions`, `DELETE /sessions/<id>`.
   Más RESTful pero menos descubrible vía curl. Elegimos
   `/tx/{begin,commit,rollback}` por simetría con BEGIN/COMMIT/ROLLBACK
   SQL.

## Próximo trabajo

- **Multi-session real** (Fase 6 con WAL-mode).
- **Connection pool** de Pagers reutilizables (perf optimization).
- **Streaming results** para SELECTs grandes dentro de sesión.
- **Idle timeout configurable** via flag CLI.

## Referencias

- [ADR-0089 — M12 SAVEPOINTs](0089-m12-savepoints.md) — habilita el
  uso interesante de sesiones.
- [ADR-0013 — File lock cross-process](0013-process-level-file-lock.md)
  — por qué single-slot por ahora.
- [Bloque T](../STATUS.md) — BEGIN/COMMIT/ROLLBACK base.
- [TAREAS_PENDIENTES.md §6.5](../TAREAS_PENDIENTES.md) — declaraba M13 como pre-requisito para "cliente que haga BEGIN/INSERT/INSERT/COMMIT".
