# Security audit interno · 2026-05-25

> Auditoría con foco de pentest sobre la base v0.1.0 (post sesión E1+E2+E3+F+T+J+J2). 5 hallazgos reales identificados y remediados en el mismo día.
> Auditor: Claude (Anthropic) trabajando en sesión asistida. Validación humana: Vladimir Acuña.

## Resumen ejecutivo

| # | Severidad | Vulnerabilidad | CWE | Estado |
|---|---|---|---|---|
| 1 | 🔴 **CRÍTICO** | Memory DoS en `/exec` por `Content-Length` unbounded | CWE-400 | ✅ Remediado |
| 2 | 🟠 **ALTO** | Timing attack en token de autenticación HTTP | CWE-208 | ✅ Remediado |
| 3 | 🟠 **ALTO** | CSRF en todos los POSTs de `phpgabyadmin` | CWE-352 | ✅ Remediado |
| 4 | 🟠 **ALTO** | `install.ps1` con verificación SHA256 opcional → MITM | CWE-347 | ✅ Remediado |
| 5 | 🟡 **MEDIO** | Stack exhaustion en parser por anidamiento ilimitado | CWE-674 | ✅ Remediado |

Cobertura del audit (sin hallazgos): path traversal en `?db=`, command injection, SQL injection en nombres de DB, XSS en phpgabyadmin (todas las salidas usan `htmlspecialchars`), workflow CI security (SHA pin estricto + `persist-credentials:false`), Dockerfile (no-root user), WAL/backup (no incluye `.wal`, page-level snapshot), `/metrics` (sin valores de columnas).

---

## Detalle por hallazgo

### 1. 🔴 CRÍTICO — Memory DoS por body unbounded en `/exec`

- **Archivo:** `src/server.rs:1047`
- **CWE:** CWE-400 (Uncontrolled Resource Consumption)
- **PoC:**
  ```bash
  curl -X POST http://localhost:8080/exec \
    -H "Content-Length: 999999999999" -d 'x' --max-time 5
  ```
- **Causa:** el while loop crecía `body` hasta el `Content-Length` declarado por el cliente, sin tope.
- **Impacto:** un solo request agotaba la RAM del proceso. Con 64 workers, un atacante con 64 sockets satura el server.
- **Fix:** `MAX_REQUEST_BODY_BYTES = 100 * 1024 * 1024`. Rechaza con `[GBY-5007]` antes de leer si `Content-Length` excede el cap. Defense-in-depth: el while loop también respeta el cap por si el cliente miente sobre el header.

### 2. 🟠 ALTO — Timing attack en token de autenticación

- **Archivo:** `src/server.rs:322, 327`
- **CWE:** CWE-208 (Observable Timing Discrepancy)
- **PoC:** medir latencia de requests con `Authorization: Bearer aaa...a`, `baa...a`, ... — el byte con más latencia es el correcto.
- **Causa:** `value == token` (comparación de `String`) hace short-circuit al primer byte distinto.
- **Impacto:** con ~5 mediciones por byte, ~160 requests recuperan un token de 32 bytes.
- **Fix:** `constant_time_eq(&[u8], &[u8]) -> bool` con XOR + fold sin short-circuit. Aplicado a ambos paths (`Authorization: Bearer ...` y `X-Gabysql-Token`). Implementación interna — no agregamos dependencias.

### 3. 🟠 ALTO — CSRF en phpgabyadmin (todos los POSTs)

- **Archivo:** `web/phpgabyadmin/index.php` — handlers `new_db`, `import_csv`, `create_index`, `run_sql`, `logout` (5 handlers)
- **CWE:** CWE-352 (Cross-Site Request Forgery)
- **PoC:** página atacante con `<form action="http://localhost:8000/" method=post>` + `<input name="run_sql" value=1>` + JS auto-submit ejecuta SQL en el navegador de la víctima logueada.
- **Causa:** ningún form HTML usaba token CSRF. La cookie `gabyadmin_auth` se manda automáticamente en POST cross-origin.
- **Impacto:** ejecución de SQL arbitrario, creación/eliminación de DBs, drop de índices, import de CSV malicioso.
- **Fix:**
  - Token CSRF de 32 bytes (`bin2hex(random_bytes(32))`) generado al inicio de sesión.
  - Helper `csrf_field()` que inserta `<input type=hidden name=csrf_token value=...>`.
  - Helper `require_csrf_token()` que valida con `hash_equals()` y aborta con HTTP 403 si falta o no matchea.
  - Aplicado en los 5 handlers POST y en los 5 forms HTML correspondientes.
  - El form de **login** se excluye a propósito — el atacante no posee la cookie de auth todavía y un CSRF token tampoco serviría.

### 4. 🟠 ALTO — `install.ps1` MITM por SHA256 opcional

- **Archivo:** `scripts/install.ps1`
- **CWE:** CWE-347 (Improper Verification of Cryptographic Signature)
- **PoC:** un MITM activo intercepta tanto el `.zip` (devuelve binario malicioso) como el `.sha256` (devuelve 404). El script captura la `WebException` y continúa sin verificación.
- **Causa:** `catch [System.Net.WebException]` silencioso reducía la verificación de integridad a un warning.
- **Impacto:** RCE en máquina del usuario que ejecuta el `iwr ... | iex`.
- **Fix:** removido el catch silencioso. Cualquier falla en descargar el `.sha256`, o cualquier hash no-coincidente, es **fatal** — se aborta sin escribir nada al destino.

### 5. 🟡 MEDIO — Stack exhaustion en parser

- **Archivo:** `src/sql.rs` — `parse_where_or`, `parse_where_not`
- **CWE:** CWE-674 (Uncontrolled Recursion)
- **PoC:** `SELECT * FROM t WHERE ((((...10000...))))` o `WHERE NOT NOT NOT ... col=1` con miles de tokens crashea el proceso por stack overflow.
- **Causa:** parser recursivo sin contador de profundidad.
- **Impacto:** DoS (proceso crashea, transacciones en flight pierden datos no commiteados). Sin corrupción de disco — el WAL recovery limpia.
- **Fix:** field `where_depth: usize` en `Parser`. Constante `MAX_PARSE_DEPTH = 100`. Check al entrar/salir de `parse_where_or` y `parse_where_not`. Devuelve `[GBY-4033] PARSE_DEPTH_EXCEEDED` con mensaje claro.

---

## Cobertura sin hallazgos (verificada)

| Categoría | Resultado | Detalle |
|---|---|---|
| Path traversal en `?db=` | ✅ OK | `normalize_db_name()` rechaza `/`, `\`, `..` |
| Command injection | ✅ OK | sin `Command::new` con input del usuario |
| SQL injection en nombre de DB | ✅ OK | normalización + validación dual (Rust + PHP) |
| XSS en phpgabyadmin | ✅ OK | `htmlspecialchars()` en todas las salidas |
| `/metrics` sin auth | ✅ OK | solo agregados (p50/p95/count), no valores de columnas |
| Workflows CI | ✅ OK | SHA pin estricto, `persist-credentials: false`, permisos `read` por default |
| Dockerfile | ✅ OK | usuario no-root `gabysql`, debian-slim, apt-upgrade en build |
| WAL en backup | ✅ OK | no se incluye en `gabysql backup`; replay se hace al abrir el destino |
| Cookie de admin firmada | ✅ OK | `hash_hmac('sha256', ...)` + `hash_equals` constant-time |

---

## Códigos de error nuevos

| Código | Nombre | Severidad | Uso |
|---|---|---|---|
| `4033` | `PARSE_DEPTH_EXCEEDED` | client error | Anidamiento mayor a `MAX_PARSE_DEPTH` en WHERE/HAVING |
| `5007` | `REQUEST_BODY_TOO_LARGE` | client error | HTTP request con `Content-Length` > `MAX_REQUEST_BODY_BYTES` |

---

## Tests de regresión

- `sec_parser_rejects_deep_paren_nesting` — 200 paréntesis → `[GBY-4033]`, no crash.
- `sec_parser_rejects_deep_not_chain` — 200 NOT encadenados → `[GBY-4033]`, no crash.
- `sec_parser_accepts_reasonable_depth` — 20 paréntesis pasan sin problema (sanidad del límite).

`cargo check + cargo fmt --check + cargo clippy --all-targets -- -D warnings` limpios.

---

## Pendientes / out of scope

- **Rate limiting** por IP en el server HTTP. Hoy hay cap de conexiones simultáneas (`-max-connections`), pero no rate por origen. Útil para mitigar brute-force al token aunque el constant-time compare ya elimina el timing leak. Pendiente.
- **TLS / HTTPS termination** en el server. Hoy `gabysql-server` solo habla HTTP plano — se asume un reverse proxy delante. Documentado en RUNBOOK pero no enforced.
- **Audit de Cargo.lock** vs CVEs recientes. `cargo-audit` ya corre en CI weekly. Sin findings actuales (próxima ejecución sched: domingo).

---

## Reproducir el audit

```bash
# Resumen rápido: contás los hallazgos con git log filtrando por commit Sec*:
git log --oneline --grep='^fix(sec|security|csrf|sec[0-9])'

# O por archivo cambiado:
git log --all --oneline -- SECURITY_AUDIT_2026-05-25.md
```

Próximo audit programado: cuando se cierre el siguiente bloque grande del roadmap (G o K — funciones escalares o DDL faltante), o ante report externo en `security@`.
