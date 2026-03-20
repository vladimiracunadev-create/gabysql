# CONTRIBUTING

## Objetivo
Las contribuciones a `gabysql` deben priorizar estabilidad, legibilidad del formato en disco y coherencia entre código, pruebas y documentación.

## Principios del repositorio
- No prometer en docs lo que el motor todavía no soporta.
- Si cambias storage, parser o semántica SQL, agrega o ajusta pruebas.
- Si cambias comportamiento visible, actualiza también la documentación.
- Prefiere cambios pequeños, verificables y reversibles.

## Flujo recomendado
1. Entender el comportamiento actual.
2. Implementar el cambio en el módulo correspondiente.
3. Ejecutar validaciones locales.
4. Actualizar documentación afectada.
5. Revisar límites, compatibilidad y riesgos.

## Validaciones obligatorias
```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
php -l web/index.php
php -l web/phpgabyadmin/index.php
```

Si tu entorno Windows todavía no tiene toolchain nativo completo, valida al menos:
```powershell
docker build -t gabysql .
```

## Dónde tocar cada cosa
- `src/storage.rs`: pager, header, WAL y recovery.
- `src/bptree.rs`: índice persistente por PK.
- `src/catalog.rs`: catálogo de tablas.
- `src/sql.rs`: SQL parser, engine y serialización de filas.
- `src/server.rs`: API HTTP/JSON.
- `web/phpgabyadmin/index.php`: admin web.
- `tests/integration_test.rs`: pruebas de integración del core.

## Contrato documental
Si un cambio afecta comportamiento real, revisa al menos:
- `README.md`
- `CHANGELOG.md`
- `ROADMAP.md`
- `USER_MANUAL.md`
- `RUNBOOK.md`
- la documentación técnica correspondiente en `docs/`

## Qué cambios requieren prueba nueva
- cambios en formato en disco
- cambios en WAL o recovery
- cambios en parser o gramática SQL
- cambios en constraints y validación de tipos
- cambios en endpoints HTTP
- cambios en semántica de errores

## Qué evitar
- introducir features “anunciadas” sin pruebas
- ampliar SQL sin revisar impacto en docs
- asumir concurrencia que todavía no existe
- exponer `phpgabyadmin` a hosts remotos sin revisar hardening
