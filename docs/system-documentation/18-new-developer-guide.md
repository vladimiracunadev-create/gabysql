# 18. Guía para un nuevo desarrollador

## Itinerario sugerido

1. Lea [README](../../README.md), [esta visión](01-system-overview.md) y [ARCHITECTURE](../ARCHITECTURE.md).
2. Ejecute quickstart y pruebas; cree una DB descartable fuera del repo si es posible.
3. Siga `gabysql exec`: `src/bin/gabysql.rs` -> `parse`/`Engine` -> catálogo/B+Tree/pager.
4. Lea `TECHNICAL_SPECS` y ADR-0001/0003/0004/0018 antes de tocar storage.
5. Lea `SQL_REFERENCE`, `ERROR_HANDLING` y tests de integración antes de parser/engine.
6. Recorra `server.rs`, `docs/API.md`, clientes de ejemplo y admin para el flujo HTTP.
7. Use `GABYBENCH_SPEC` y gabybench antes de afirmar mejoras de rendimiento.

## Preparación y convenciones

```powershell
cargo build
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

No prometa features no implementadas. Todo error visible nuevo obtiene `[GBY-NNNN]`. Todo cambio de storage declara compatibilidad/migración/rechazo. Cambios de comportamiento actualizan pruebas, manuales, roadmap/changelog cuando corresponda y ADR si hay decisión duradera.

## Dónde añadir cambios

Pager/WAL en `storage.rs`; árbol en `bptree.rs`; objetos/codecs en `catalog.rs`; índices en `index.rs`; SQL en `sql.rs`; HTTP en `server.rs`; errores en `errors.rs`; interfaces en `web/`. Consulte [CONTRIBUTING](../../CONTRIBUTING.md).

## Tareas iniciales apropiadas

- Mejorar ejemplos o enlaces documentales comprobables.
- Añadir un test de regresión para un error ya entendido.
- Ampliar troubleshooting desde un incidente reproducible.
- Instrumentar/analizar un benchmark sin cambiar semántica.

Evite como primera tarea cambios de formato, WAL/recovery, codecs de filas, transacciones o RLS. Esas áreas requieren revisión de invariantes y compatibilidad.
