# CLAUDE.md

> Guía operacional para asistentes (Claude Code y otros) trabajando
> en este repositorio. Lectura obligatoria antes del primer cambio.

## Qué es este repo

**gabysql** — motor de base de datos relacional escrito desde cero en
Rust, single-file portable (`.db` + `.wal`), CRC32 por página, B+Tree
real, WAL after-image, sin C en el core, sin runtime obligatorio.

Estado: VERSION on-disk 33, fase 2 cerrada (SQL relacional completo +
constraints + vistas + CTEs + window functions + triggers + procs +
funcs + tipos extendidos + USERS/ROLES/GRANT/REVOKE + RLS), fase 3
cerrada (planner cost-based P1-P5e + hardening + sesión maratón
M3/M4/M6/M12/M13) + bloque L (log de sentencias del motor, ADR-0094).
850 tests en `tests/` verdes en CI Ubuntu/macOS/Windows +
Docker + bench job. Ver [docs/STATUS.md](docs/STATUS.md) para el
detalle.

Stack de productos:
1. **`gabysql` CLI** — REPL + scripts.
2. **`gabysql-server` HTTP/JSON** — 19 endpoints (core + Tx M13 + catalog listing).
3. **`web/phpgabyadmin/`** (PHP single-file) + **`web/modeler/`** (HTML
   single-file) — productos de gestión visual. Ambos vanilla, sin
   build system, deploy-zero.
4. **`desktop/gabymodeler/`** — el modelador empaquetado con Tauri 1.6
   como `.msi` Windows. Incluye `gabysql-server` como sidecar local.
   Build CI con `git tag desktop-v*`.

## Reglas duras

Reglas guardadas en memoria que tienen historia detrás. No las violes
sin razón concreta:

### Workflow operacional
- **1 bloque del roadmap = 1 push directo a `main`** (no PRs, no
  branches). Autorizado por el dueño. Cuando termines un bloque y
  el CI esté verde, pusheás.
- Antes de push: `cargo fmt --check` + `cargo clippy --all-targets --
  -D warnings` + `cargo test --lib --tests` (locales) deben pasar
  limpio. CI valida lo mismo en Linux/macOS/Windows + Docker. CI con
  warnings de clippy = CI rojo.
- Nunca declarar "X OK" sin pegar el output del comando que lo prueba.
  El usuario tiene permiso explícito de pedir "mostrame el comando".

### Rust toolchain
- `cargo`/`rustc`/`clippy` **NO** están en PATH global. Viven en
  `~/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/`. Exportar
  PATH antes de cualquier `cargo`:
  ```bash
  export PATH="$HOME/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin:$PATH"
  ```
- En Windows hay colisión `/usr/bin/link.exe` (Git Bash Unix linker)
  vs MSVC linker. `cargo test` local puede fallar con
  `link: missing operand after '\377\376'`. Eso es ambiente, no
  código. `cargo clippy --all-targets` valida tipos sin linkear.
  CI corre tests en Linux donde no hay colisión.

### PowerShell vs Bash
- PowerShell por default escribe UTF-16 LE BOM. Si redirigís output
  con `| Out-File`, leerlo con `cat` en bash da caracteres chinos.
  Usar `-Encoding utf8` o la Bash tool directa.

### Procesos largos
- Antes de lanzar bench/fuzz: matar zombies con
  `Get-Process gabybench | Stop-Process -Force`. Wrappers PowerShell
  y procesos bash `until` quedan vivos por horas si no.

### Bench
- Validar SQL nuevo contra el motor antes de agregarlo al bench.
  Catálogo de gaps en `docs/adr/0066-bench-exposed-gaps.md`.

### Docs invariants
- Antes de declarar "docs en regla", correr los 4 greps de la
  checklist. STATUS.md tiende a quedarse atrás (test count, VERSION,
  features "todavía no" cuando ya están). Lecciones reales en
  2026-05-29/30 y 2026-06-17/18 (el `scheme` default era stale 13
  pushes seguidos).

## Estructura

```
src/
  storage.rs      Pager + WAL + page cache + CRC32 + cross-process lock
  bptree.rs       B+Tree con leaf cursor lazy + prefetch one-ahead
  index.rs        Secondary indexes (hash UNIQUE + composite K2/K3)
  sql.rs          Parser + Engine + planner cost-based (~25k LOC)
  catalog.rs      Catálogo persistido — tablas, índices, views,
                  policies, triggers, procs, funcs, users, roles, grants
  server.rs       HTTP server + sessions M13 + 17 endpoints
  dblog.rs        Log de sentencias/errores del motor — JSONL append-only
                  con rotación por tamaño. Enganchado en Engine::exec
  errors.rs       ~210 mensajes en español [GBY-NNNN]
  bin/
    gabybench.rs       Bench de carga
    demo-dbs.rs        Genera DBs demo
    gabysql-bench.rs   Bench secundario

tests/
  integration_test.rs              818 tests integración
  m13_server.rs                    4 E2E tx cross-request
  server_listing_endpoints.rs      10 E2E catalog endpoints (Push 15)
  dblog_engine.rs                  9 E2E del log del motor (bloque L)
  proptest_planner.rs              3 property tests (M3)
  proptest_pager.rs                3 property tests
  fuzz_parser.rs                   M4, 503.8M iters/h limpia

web/
  phpgabyadmin/index.php   Admin v2 — 9 tabs, CodeMirror, CSRF
  modeler/index.html       gabymodeler v3 — ER + 9 colecciones top-level
  index.html               Landing page

docs/
  STATUS.md           Estado vigente con tabla de features
  ERROR_HANDLING.md   Guía canónica de [GBY-NNNN]
  adr/0001..0092      Decisiones arquitectónicas
  benchmarks/         Resultados por commit
```

## Entrypoints clave

| Tarea | Archivo |
|---|---|
| Agregar SQL statement nuevo | `src/sql.rs` (parser + Statement enum + exec_*) |
| Cambiar shape de página | `src/storage.rs` + bumpear VERSION en magic header |
| Nuevo endpoint HTTP | `src/server.rs` (match en handle_request + handler fn + JSON serializer) |
| Nuevo tipo de objeto persistible | `src/catalog.rs` (struct + serialize/deserialize + put_X/get_X/list_X) |
| Nuevo error code | `src/errors.rs` (constante en el rango del bloque correspondiente) + fila en `docs/ERROR_CODES.md` |
| Nuevo campo en el log de sentencias | `src/dblog.rs` (`LogRecord` + `render_entry`) — bumpear `LOG_SCHEMA_VERSION` si cambia el shape |
| Clasificar un `Statement` nuevo para el log | `src/sql.rs` (`statement_kind` — el `match` es exhaustivo, el compilador te avisa) |
| Tab nuevo en phpgabyadmin | `web/phpgabyadmin/index.php` ($tabLinks array + elseif tab block) |
| Feature nueva en modeler | `web/modeler/index.html` (state schema + modal + browser tree + generateSQL + checkModel + status counts) |
| Release desktop .msi | `git tag desktop-v0.1.X && git push --tags` → workflow `desktop-release.yml` |
| Config sidecar / menú nativo | `desktop/gabymodeler/src-tauri/{tauri.conf.json, src/main.rs}` |
| Regenerar iconos | `cd desktop/gabymodeler && python generate_icons.py` |
| ADR nuevo | `docs/adr/NNNN-titulo-corto.md` (formato: fecha, estado, bloque, refina, contexto, decisión, alternativas, consecuencias, referencias) |

## Comandos comunes

```bash
# Toolchain Rust
export PATH="$HOME/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin:$PATH"

# Pre-push gate
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --lib --tests          # Linux/macOS/CI. Windows local puede fallar por link.exe

# Server local
cargo run --release --bin gabysql-server -- -dir ./data -addr :8080

# Productos web (server PHP local)
php -S localhost:8000 -t web

# Docker compose (modeler + admin + server)
docker compose up -d --build

# Bench smoke
cargo run --release --bin gabybench -- smoke

# Ver CI último push
gh run list --limit 3 --branch main
gh run view --log-failed <run-id>
```

## Cosas que NO hacer

- No agregar deps de crates externos (ADR-0001 — cero deps runtime).
- No usar PowerShell para multi-line strings que vayan a archivos
  leídos por bash (UTF-16 BOM).
- No declarar "tests OK" sin pegar el output del comando.
- No hacer push con clippy warnings — CI los trata como error.
- No tocar el formato on-disk sin bumpear VERSION + ADR + test de
  migración.
- No exponer material secreto (`password_hash`, `salt`) en respuestas
  HTTP. `/users` tiene asserts negativos en tests para esto.
- No usar `git rebase -i` (interactive). No usar `git add -A` sin
  revisar (puede levantar logs/binarios). No `--no-verify`.
- No crear archivos `.md` "para el asistente" salvo este CLAUDE.md.
  Si el repo necesita docs, van en `docs/`.
