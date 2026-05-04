<!-- Resumen breve del cambio. ¿Qué se hizo y por qué? -->

## Cambios

- 

## Verificación local

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --all-targets`
- [ ] `docker build -t gabysql .` (si toca runtime, server o `Dockerfile`)

## Riesgo y compatibilidad

- [ ] No cambia el formato en disco (página, header, WAL, índice).
- [ ] Cambia el formato en disco — incluyo bump de `VERSION` y nota explícita en [CHANGELOG.md](../CHANGELOG.md).
- [ ] Cambia errores visibles al usuario o respuestas HTTP — actualicé [docs/API.md](../docs/API.md) y [TROUBLESHOOTING.md](../TROUBLESHOOTING.md).

## Seguridad

- [ ] No introduce nuevas dependencias Cargo.
- [ ] Si introduce dependencias, su licencia está en el allowlist de [deny.toml](../deny.toml).
- [ ] No agrega `unsafe`, `transmute`, `Command::new` con interpolación, `eval/system/exec` en PHP, ni URLs hardcodeadas a webhooks externos.
- [ ] No requiere bypass de hooks ni de `cargo fmt`/`clippy`.

## Contexto adicional

<!-- Issues relacionados, decisiones, capturas, links. -->
