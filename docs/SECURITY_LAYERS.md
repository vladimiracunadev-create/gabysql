# 🛡️ Capas de seguridad de `gabysql`

> **Mapa completo de qué protege qué, dónde está implementado y dónde está documentado.**
> Lectura horizontal: la pieza ofensiva está a la izquierda, la mitigación al centro y los archivos exactos a la derecha.

Este documento es el ancla central. Las políticas (cómo reportar, qué está in-scope) viven en [`SECURITY.md`](../SECURITY.md); los detalles operacionales en [`RUNBOOK.md`](../RUNBOOK.md); los errores de runtime en [`TROUBLESHOOTING.md`](../TROUBLESHOOTING.md).

---

## 1. 💾 Capa de storage / durabilidad

| Amenaza | Mitigación | Implementación | Documentación |
| :--- | :--- | :--- | :--- |
| Corrupción accidental (corte de luz, fallo de disco, escritura torcida) | CRC32-IEEE en los últimos 4 bytes de cada página; verificación en cada lectura del `.db` y al replay del WAL | [`src/storage.rs:finalize_page_checksum / verify_page_checksum`](../src/storage.rs) | [docs/TECHNICAL_SPECS.md](TECHNICAL_SPECS.md) §Identidad del formato y §WAL |
| Replay de WAL truncado | Verificación CRC sobre el payload de cada record antes de aplicar al `.db`; abort explícito | [`src/storage.rs:Wal::replay_to`](../src/storage.rs) | [RUNBOOK.md §Recovery tras caída](../RUNBOOK.md) |
| Apertura de DB con formato incompatible | `Header::decode` rechaza con mensaje explícito y bumpea `VERSION` cada vez que cambia el formato | [`src/storage.rs:Header::decode`](../src/storage.rs) | [CHANGELOG.md](../CHANGELOG.md), [COMPATIBILITY.md §Formato en disco](../COMPATIBILITY.md) |
| Pérdida silenciosa por `gabysql init` sobre archivo existente | `Pager::create` rehúsa overwrite; se requiere `create_force` (CLI: `--force`) | [`src/storage.rs:Pager::create_internal`](../src/storage.rs) | [USER_MANUAL.md §1. CLI](../USER_MANUAL.md), [TROUBLESHOOTING.md §refusing to overwrite](../TROUBLESHOOTING.md) |
| Hash del catálogo dependiente de la versión de Rust (DBs ilegibles tras toolchain upgrade) | FNV-1a-64 fijado en código, independiente de `std` | [`src/catalog.rs:hash_name`](../src/catalog.rs), [`src/index.rs:hash_value`](../src/index.rs) | [docs/TECHNICAL_SPECS.md §Catálogo](TECHNICAL_SPECS.md) |

> Los CRC32 detectan **corrupción accidental**, no manipulación adversarial: un atacante con acceso de escritura al disco puede recomputar el CRC. Esto está explícitamente fuera de scope del modelo de amenaza.

---

## 2. 🌐 Capa de acceso al motor

| Amenaza | Mitigación | Implementación | Documentación |
| :--- | :--- | :--- | :--- |
| Acceso no autenticado al server HTTP | Token compartido opcional vía `X-Gabysql-Token` o `Authorization: Bearer <token>`; `401` ante mismatch | [`src/server.rs:handle_request`](../src/server.rs) | [docs/API.md §Autenticación](API.md) |
| Exhausting de threads / sockets por cliente abusivo | Cap de conexiones simultáneas (default 64, configurable con `-max-connections N`); rechazo `503` sin spawning | [`src/server.rs:run`](../src/server.rs) | [USER_MANUAL.md §Tope de conexiones simultáneas](../USER_MANUAL.md), [TROUBLESHOOTING.md §server busy](../TROUBLESHOOTING.md) |
| Path traversal en modo multi-DB (`-dir`) | Normalización del nombre de DB y rechazo de paths no aceptados | [`src/server.rs:normalize_db_name`](../src/server.rs) | [SECURITY.md](../SECURITY.md) |
| Race entre escrituras concurrentes | Mutex de proceso compartido por todas las rutas que escriben | [`src/server.rs:write_lock`](../src/server.rs) | [docs/ARCHITECTURE.md §Flujo por HTTP](ARCHITECTURE.md) |
| Inyección SQL (gramática soportada) | Parser tipado con AST y `expect_value`/`expect_integer` validados; sin `eval` ni interpolación dinámica | [`src/sql.rs:Parser`](../src/sql.rs) | [docs/TECHNICAL_SPECS.md §Gramática SQL soportada](TECHNICAL_SPECS.md) |

---

## 3. 🔍 Capa SDLC / supply chain del repositorio

Implementadas en CI (GitHub Actions) bajo [`.github/workflows/security.yml`](../.github/workflows/security.yml).

| Amenaza | Mitigación | Workflow / herramienta | Configuración |
| :--- | :--- | :--- | :--- |
| Vulnerabilidad en dependencias Cargo (RustSec advisories) | `cargo-audit` 0.22.1 con enforcement progresivo (soft en PR, hard en push a `main` y schedule semanal) | `security.yml :: cargo_audit` | n/a (DB pública RustSec) |
| Licencias incompatibles, crates banneados, registries desconocidos | `cargo-deny` 0.19.4 con `check advisories + bans + licenses + sources` | `security.yml :: cargo_deny` | [`deny.toml`](../deny.toml) |
| Secretos commiteados (claves API, tokens, etc.) | `detect-secrets` 1.5.0 sobre filesystem **y** los últimos 50 commits | `security.yml :: secrets` | [`.secrets.baseline`](../.secrets.baseline) |
| Trojan Source (CVE-2021-42574 — caracteres bidi Unicode) | grep contra rangos `‪-‮`, `⁦-⁩`, `‏`, `؜` | `security.yml :: supply_chain` | n/a |
| Caracteres zero-width / homoglyphs en código fuente | grep contra `​`, `‌`, `‍`, `﻿` | `security.yml :: supply_chain` | n/a |
| Patrones peligrosos Rust (`Command::new` con `format!`, `mem::transmute` en src) | grep dirigido sobre `*.rs` con allowlist de tests/.github | `security.yml :: supply_chain` | n/a |
| Patrones peligrosos PHP (`eval`, `system`, `exec` con interpolación) | grep dirigido sobre `*.php` | `security.yml :: supply_chain` | n/a |
| URLs de webhook/exfil hardcodeadas | grep contra Discord, Slack, webhook.site, ngrok, requestbin, etc. | `security.yml :: supply_chain` | n/a |

---

## 4. 🛠️ Capa de los propios workflows (meta-seguridad)

Implementada en [`.github/workflows/workflow-security.yml`](../.github/workflows/workflow-security.yml). Ataque típico: alguien fusiona un workflow malicioso o repunteable.

| Amenaza | Mitigación | Herramienta |
| :--- | :--- | :--- |
| Acción third-party con tag movible (`@v1`, `@main`, `@master`) | `pin-check`: parser YAML que rechaza cualquier `uses:` no pinneado a SHA de 40 hex; allowlist vacía por defecto | `workflow-security.yml :: pin-check` |
| Script injection vía expresiones `${{ github.event.* }}` interpoladas en `run:` | `zizmor 1.5.2` con `--persona=auditor` | `workflow-security.yml :: zizmor` |
| `pull_request_target` mal usado / permisos excesivos / `inputs` no sanitizados | `actionlint 1.7.7` con shellcheck integrado, checksum verificado | `workflow-security.yml :: actionlint` |

Y en cada workflow:

- `permissions: contents: read` a nivel workflow (default deny). Cada job sube permisos solo donde los necesita.
- `persist-credentials: false` en cada `actions/checkout` para que un step posterior no pueda re-usar el `GITHUB_TOKEN`.
- `concurrency` block para cancelar runs superados.

---

## 5. 🐳 Capa de la imagen Docker

| Amenaza | Mitigación | Implementación |
| :--- | :--- | :--- |
| CVEs **fixeables** en la base `debian:bookworm-slim` o en paquetes del runtime | `grype 0.110.0` con política `only-fixed: true` + `fail-on-severity: critical` sobre la imagen final, ejecutado en cada PR y push a `main` | `security.yml :: container_scan` + [`.grype.yaml`](../.grype.yaml) |
| CVEs aplicables que ya tienen parche en repos Debian | `apt-get upgrade` en el stage runtime del [`Dockerfile`](../Dockerfile) trae los fixes publicados al momento del build | [`Dockerfile`](../Dockerfile) |
| Ejecución como root dentro del container | `Dockerfile` crea usuario `gabysql` y hace `USER gabysql` antes de `CMD` | [`Dockerfile`](../Dockerfile) |
| Datos persistidos sin volumen explícito | `VOLUME ["/data"]` declarado en el Dockerfile; `docker-compose.yml` usa volume nombrado | [`Dockerfile`](../Dockerfile), [`docker-compose.yml`](../docker-compose.yml) |

### Política frente a CVEs `(won't fix)`

`debian:bookworm-slim` reporta decenas de CVEs (incluyendo Critical y High) en `libc6`, `libpam`, `ncurses`, `util-linux`, `gpgv`, etc. que Debian marca **`(won't fix)` para esta major release**: no hay parche aguas arriba.

Decisión de proyecto: **el merge se bloquea solo cuando hay un Critical con fix disponible** (política estándar Anchore/Snyk/Trivy). Las CVEs no-fixable:

- siguen apareciendo en el reporte completo del job (`grype ... -o table | tee` + `$GITHUB_STEP_SUMMARY`),
- son auditables en cada run sin necesidad de re-correr nada localmente,
- son el motivo principal por el que el ROADMAP contempla migrar a `gcr.io/distroless/cc-debian12` (superficie radicalmente menor) cuando el binario y `phpgabyadmin` lo permitan.

Esta postura es deliberada y se prefiere a:

- **Ignorar lista hardcodeada de CVEs**: produce drift silencioso cada vez que una nueva CVE no-fixable se publica.
- **`--fail-on high`**: produce falsos positivos permanentes que el equipo aprende a ignorar (security fatigue).
- **Cambiar a `alpine`**: no menos CVEs, solo distintas — y rompe glibc compat con el binario `cargo build --release` actual.

---

## 6. 📅 Capa de operación / disclosure

| Tema | Documento |
| :--- | :--- |
| Política de disclosure responsable, scope in/out, SLA de respuesta | [SECURITY.md](../SECURITY.md) |
| Versiones soportadas y formato en disco vigente | [SECURITY.md §Versiones soportadas](../SECURITY.md), [COMPATIBILITY.md](../COMPATIBILITY.md) |
| Actualizaciones automáticas de dependencias | [`.github/dependabot.yml`](../.github/dependabot.yml) — cargo + github-actions + docker, semanal |
| Checklist pre-release (incl. comprobación de detect-secrets) | [RELEASE.md](../RELEASE.md) |
| Cómo pedir ayuda sin filtrar payloads sensibles | [SUPPORT.md](../SUPPORT.md) |
| Estándares de comportamiento en la comunidad | [CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md) |

---

## 🧪 Cómo verificar que todas las capas pasan

```bash
# Capa 1-2: motor
cargo test --all-targets

# Capa 3: SDLC
cargo install cargo-audit --version 0.22.1 --locked
cargo install cargo-deny  --version 0.19.4 --locked
cargo audit
cargo deny --all-features check

# Capa 4: workflows (requiere Python + zizmor)
pip install 'zizmor==1.5.2' pyyaml==6.0.2
zizmor --persona=auditor --format=plain .github/workflows/

# Capa 5: container
docker build -t gabysql-scan .
grype gabysql-scan --fail-on critical
```

CI corre todo lo anterior automáticamente en cada push a `main` y en cada PR. Una falla en cualquier capa **bloquea el merge**.

---

## 📌 Lo que estas capas NO cubren

Para no inflar la postura de seguridad más allá de lo entregado:

- **TLS nativo**: el server publica HTTP plano. Producción → reverse proxy con TLS.
- **Cifrado en reposo**: el `.db` no está cifrado; el OS / disco completo es responsable.
- **MVCC / aislamiento avanzado**: una sola transacción global por proceso.
- **Authz fina**: solo token compartido; no hay usuarios/roles, no hay auditoría granular.
- **Rate limiting por IP/cliente**: el cap de conexiones es DoS-mitigation mínimo, no un WAF.
- **Manipulación adversarial del `.db`**: los CRC32 detectan errores accidentales, no tampering.

Estas brechas están explícitamente registradas en [SECURITY.md §Riesgos conocidos](../SECURITY.md) y en el [ROADMAP](../ROADMAP.md) como capas futuras.
