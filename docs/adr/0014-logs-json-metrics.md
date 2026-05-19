# ADR-0014: Logs JSON estructurados + endpoint `/metrics` en el server

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-18
**Contexto que la motiva**: bloque "logs estructurados y primeras métricas del server" de Fase 2 → primer paso de observabilidad operacional para `gabysql-server`.

## 🧭 Contexto

Hasta este bloque, `gabysql-server` corría **silencioso**: el único output a stdout/stderr era el banner de arranque (`gabysql-server escuchando en ...`) y los `accept error` esporádicos. Nada por request, nada de contadores agregados.

Operacionalmente esto rompía dos cosas:

1. **Diagnóstico post-mortem**: si un usuario reportaba "el server se puso lento ayer a las 3pm", no había forma de mirar atrás. No había logs, no había histograma de latencia, nada.
2. **Health más allá de `/health`**: el `/health` actual solo dice "estoy vivo y configurado". No responde "¿cómo se está comportando bajo carga?". Para un operador con un dashboard, el binario en producción es opaco.

Restricciones del proyecto:
- ADR-0001: cero deps externas. Nada de `tracing`, `prometheus`, `metrics-rs`.
- ADR-0009: memoria acotada. La estructura de métricas no puede crecer linealmente con el tiempo.
- Opt-in por defecto para los logs. Un binario silencioso es la expectativa razonable; el ruido extra debe pedirse explícitamente.
- Cross-platform: nada de syslog, journald ni hooks específicos de Linux.

## 💡 Decisión

Tres piezas en `src/server.rs`, todas zero-deps:

### 1. `Metrics` in-memory acotado

```rust
pub struct Metrics {
    started_unix: u64,
    requests_by_status: HashMap<u16, u64>,
    errors_total: u64,
    latency_samples: Vec<u32>,  // ring buffer, cap LATENCY_SAMPLE_RING = 1024
    latency_cursor: usize,
    latency_count: u64,
}
```

- Contadores por status HTTP (`requests_by_status[200]`, etc.).
- `errors_total` = suma de status ≥ 500. Un solo número para alertar.
- Ring buffer de 1024 latencias para p50/p95. Memoria O(1) bajo carga sostenida.
- `latency_count` registra el total acumulado (no truncado), para distinguir "1024 samples sobre 1M requests" de "1024 samples sobre 1024 requests".

### 2. Endpoint `GET /metrics`

Devuelve JSON estable:

```json
{
  "ok": true,
  "started_unix": 1747497600,
  "uptime_s": 3600,
  "requests_total": 1234,
  "requests_by_status": {"200": 1180, "400": 30, "500": 24},
  "errors_total": 24,
  "latency_ms": {"p50": 5, "p95": 87, "samples": 1024, "count": 1234}
}
```

Gated por auth como cualquier otro endpoint cuando hay `-token`. Sin cuerpo de request, sin parámetros.

### 3. Logs JSON opt-in

Flag nuevo `-log-json` en `gabysql-server`. Cuando está activo, cada request termina escribiendo una línea JSON a **stdout**:

```json
{"ts_unix":1747497612,"method":"POST","path":"/exec","status":200,"latency_ms":12}
```

stderr sigue siendo el banner de arranque + errores de accept; stdout queda como stream de eventos procesable por `jq`, `tee`, ingest a S3/ELK/Loki, etc.

Por defecto **off**. La UX humana del binario no cambia.

## 🤔 Alternativas evaluadas

1. **`tracing` + `tracing-subscriber`**: el ecosistema canónico. Pero viola ADR-0001 (deps externas) y trae 10-20 transitivas más. No vale por una línea JSON por request.

2. **Logs a archivo con rotación**: agrega complejidad (rotation policy, locks, fsync) sin ganar nada que stdout+pipe-a-archivo no resuelva ya con herramientas del SO (`logrotate`, `multilog`, supervisor).

3. **Histograma exponencial real (HdrHistogram, t-digest)**: más preciso para tails extremos pero requiere implementación manual no-trivial. El ring buffer de 1024 sortable da p50/p95 con error < 0.5% para distribuciones razonables — suficiente para esta fase.

4. **`/metrics` en formato Prometheus text** (`# HELP`, `# TYPE`, `metric_name{label="x"} value`): Prometheus es popular, pero (a) el resto del API ya es JSON, mezclar dos formatos sin razón es sucio; (b) un adapter Prometheus de tercero puede traducir nuestro JSON cuando alguien lo necesite. Mantener un solo content-type es más simple.

5. **Métricas persistentes (a disco entre reinicios)**: agrega filesystem state que no aporta para el caso "operador mira el dashboard ahora mismo". Si alguien quiere histórico, hace scrape periódico al endpoint y guarda fuera.

## ✅ Consecuencias

**Positivas**:
- Observabilidad básica sin tocar deps. ADR-0001 intacto.
- Memoria acotada O(1) (1024 × 4 bytes = 4 KB por server para el ring).
- Opt-in puro: el binario silencioso de hoy sigue siendo el default.
- Logs JSON línea-a-línea son trivialmente procesables (`jq '.latency_ms > 100'`, ingest a ELK con un Logstash filter de 3 líneas).
- `errors_total` es un solo número alertable.

**Negativas / a vigilar**:
- p50/p95 sobre solo 1024 muestras: bajo carga muy alta el ring rota rápido y el snapshot es de los últimos ~segundos de tráfico, no de toda la vida del server. Eso es lo que se quiere para "¿cómo está ahora?", pero no para "¿cuál fue el peor p95 del mes?". Para eso, scrape periódico externo.
- `Mutex` global de `Metrics`: contención mínima (un `record` por request, muy rápido), pero técnicamente un punto de serialización. Si en el futuro hay >10K req/s sostenidos, conviene mover a counters atómicos por status.
- El log JSON no incluye request_id ni body — solo método, path, status, latencia. Suficiente para troubleshooting básico; para auditoría profunda existe ya el audit log del MCP gateway (ADR-0012).

## 🔗 Referencias

- [src/server.rs](../../src/server.rs): `Metrics`, `/metrics` handler, `log_request_json`.
- [src/bin/gabysql-server.rs](../../src/bin/gabysql-server.rs): flag `-log-json`.
- [ADR-0001](0001-rust-zero-deps-core.md): cero deps en el core.
- [ADR-0009](0009-page-cache-lru-bounded.md): memoria acotada del server (mismo principio aplicado aquí).
- [ADR-0012](0012-audit-log-enriquecido.md): audit log en el gateway MCP (no en el motor) — capa complementaria.
