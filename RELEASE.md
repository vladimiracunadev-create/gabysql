# 📦 Estrategia de Versionado y Entrega

> **Cómo se empaca, versiona y publica una nueva versión de `gabysql`.**

---

## 1. 🔢 Patrón de versionado

`gabysql` sigue **[Semantic Versioning 2.0.0](https://semver.org/)** (`MAJOR.MINOR.PATCH`):

- **MAJOR (1.x.x)** — cambios estructurales o de API que rompen contratos públicos del crate / del CLI / del servidor HTTP.
- **MINOR (x.1.x)** — features nuevas (índices, sentencias SQL, endpoints) compatibles hacia atrás a nivel de API.
- **PATCH (x.x.1)** — bugfixes, mejoras de performance, hardening de seguridad sin cambios de comportamiento observable.

> **Importante**: el formato en disco (`storage::VERSION`) tiene su propio versionado, **independiente de SemVer**. Un bump del formato puede ocurrir en cualquier MAJOR o MINOR siempre que:
> - se documente en [CHANGELOG.md](CHANGELOG.md),
> - el binario rechace explícitamente versiones anteriores con mensaje claro al abrir,
> - se actualice [docs/TECHNICAL_SPECS.md](docs/TECHNICAL_SPECS.md) y [COMPATIBILITY.md](COMPATIBILITY.md).

## 2. 🔄 Flujo de release

### 2.1. Preparación local

```bash
# árbol limpio en main
git checkout main && git pull --ff-only

# verificación obligatoria
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets

# verificación de seguridad (corre lo mismo que CI)
cargo install cargo-audit --version 0.22.1 --locked
cargo install cargo-deny  --version 0.19.4 --locked
cargo audit
cargo deny --all-features check

# Docker debe seguir construyendo
docker build -t gabysql .
```

### 2.2. Actualización documental

Antes de tagear:

- [ ] [CHANGELOG.md](CHANGELOG.md) tiene una nueva entrada con fecha y resumen ordenado por categoría.
- [ ] Si cambia el formato en disco: nota explícita de migración + bump de `VERSION` en `src/storage.rs`.
- [ ] Si cambia comportamiento del CLI o HTTP: actualizar [USER_MANUAL.md](USER_MANUAL.md) y [docs/API.md](docs/API.md).
- [ ] [ROADMAP.md](ROADMAP.md) refleja qué hitos quedaron entregados.

### 2.3. Tag y push

```bash
git add CHANGELOG.md src/ docs/
git commit -m "chore: prepare release v0.2.0"
git tag -a v0.2.0 -m "Release v0.2.0: índices secundarios"
git push origin main --tags
```

### 2.4. CI / artefactos

Tras el push, los workflows de [.github/workflows/](.github/workflows) ejecutan:

- `ci.yml` — `cargo fmt + clippy + test` en Ubuntu / Windows / macOS y construye binarios `release` por OS, subidos como artefactos.
- `security.yml` — `cargo-audit + cargo-deny`, `detect-secrets` (FS + historial), Trojan Source, container scan con `grype --fail-on critical`.
- `workflow-security.yml` — `actionlint + zizmor + pin-check` sobre los propios workflows.

El tag dispara los binarios firmados que se publican en la página de Releases. Si algún job falla, **el release no avanza**.

## 3. ✅ Checklist pre-release obligatorio

- [ ] `main` está limpio, sin cambios sin commitear.
- [ ] CI verde en último commit (todos los workflows).
- [ ] Si bumpea formato en disco: probado el flujo `init → INSERT → reabrir` en Linux y Windows.
- [ ] Probado que una DB de la versión anterior es **rechazada con mensaje explícito** (no abierta silenciosamente).
- [ ] [USER_MANUAL.md](USER_MANUAL.md) y [docs/API.md](docs/API.md) reflejan los cambios visibles al usuario.
- [ ] `phpgabyadmin` smoke en local (`docker compose up -d --build` + `http://localhost:8000/phpgabyadmin/`).
- [ ] `gabymodeler` smoke: `http://localhost:8000/modeler/` → `📦 Cargar ejemplo` → `Exportar SQL` produce un DDL no vacío.
- [ ] Ningún token / credencial / archivo `.env` ha entrado al repo (CI lo verifica con `detect-secrets`, pero se confirma manualmente igual).
- [ ] Tag firmado o al menos atribuido al maintainer correcto.

## 4. 🛡️ Política de revocación

Si tras publicar se detecta una regresión grave:

1. crear un Issue público describiendo el alcance (sin payloads reproducibles destructivos),
2. publicar `vX.Y.Z+1` con el fix,
3. añadir entrada de "**Hotfix** — vX.Y.Z" al CHANGELOG,
4. si el bug afecta corrupción de datos, marcar `vX.Y.Z` como yanked en el tag y dejar nota visible en el README.

No se borran tags ni Releases publicados — Semver y la trazabilidad pesan más que el "ocultar el error".
