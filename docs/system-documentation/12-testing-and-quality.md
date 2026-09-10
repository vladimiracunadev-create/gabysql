# 12. Pruebas y calidad

En el commit analizado se midieron **900 atributos `#[test]`/`#[tokio::test]`** en 23 archivos Rust de `src/` y `tests/`. Es un conteo estático, no una afirmación de cobertura ni de éxito; la fuente real es `cargo test --all-targets`.

## Suites

| Archivo/área | Tipo |
|---|---|
| `tests/integration_test.rs` | Integración extensa de SQL/storage |
| `tests/proptest_pager.rs` | Propiedades del pager |
| `tests/proptest_planner.rs` | Propiedades del planner |
| `tests/fuzz_parser.rs` | Entradas generadas, no panic |
| `tests/m13_server.rs` | Sesiones HTTP/transacciones |
| `tests/server_listing_endpoints.rs` | Catálogo/endpoints |
| `tests/dblog_engine.rs` | Statement logging y anidación |
| módulos `src/*` | Unitarias junto al código |

No se observan crates de property testing: las pruebas generativas usan implementaciones locales/deterministas.

## Validación

```powershell
cargo fmt --check
cargo check --tests
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
php -l web/index.php
php -l web/phpgabyadmin/index.php
```

CI añade seguridad, scans, actionlint/zizmor/pins y benchmarks según los 7 workflows: `ci.yml`, `desktop-release.yml`, `pages.yml`, `release.yml`, `security.yml`, `stale.yml`, `workflow-security.yml`.

## Cobertura observable y faltantes

No se identificó un porcentaje de cobertura generado como fuente de verdad. Prioridades recomendadas: recuperación con fault injection real, matrices de upgrade de formato, tests de navegador del admin/modelador, TLS/proxy de referencia, carga prolongada de sesiones y pruebas de restauración en CI. Estas son propuestas, no fallos confirmados.
