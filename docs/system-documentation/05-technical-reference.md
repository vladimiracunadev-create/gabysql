# 05. Referencia técnica

Esta es una guía de entrada; los detalles completos viven en el Rustdoc y documentos enlazados.

## Tipos y símbolos centrales

| Símbolo | Archivo | Contrato, efectos y riesgo |
|---|---|---|
| `DbResult<T>`, `DbError` | `src/lib.rs` | Resultado uniforme; el texto puede incluir código estable. Cambiarlo afecta todas las APIs. |
| `Pager` | `src/storage.rs` | Abre, bloquea, cachea, lee/escribe y recupera el archivo. Efectos de I/O y compatibilidad crítica. |
| `Tree`, `LeafCursor`, `KeyValue` | `src/bptree.rs` | Lookup, insert, delete y rangos persistentes. Riesgo de corrupción/orden. |
| `Catalog`, `CatalogObject`, `TableMeta`, `Column` | `src/catalog.rs` | Persistencia de esquema y objetos. Cambios de codec requieren estrategia de formato. |
| `IndexKind` y helpers de bucket | `src/index.rs` | Igualdad hash o rango INT; mutan árboles auxiliares. |
| `Statement`, `SelectQuery`, `Expr`, `Value` | `src/sql.rs` | AST y valores públicos del lenguaje. |
| `parse(&str)` | `src/sql.rs` | SQL -> sentencias o `DbError`; limita profundidad para evitar abuso. |
| `Engine` | `src/sql.rs` | Ejecuta sentencias, constraints, RLS, índices y transacciones; muta pager/catálogo. |
| `ResultSet` | `src/sql.rs` | Columnas, filas y mensaje que consumen CLI/server. |
| `ServerConfig`, `run` | `src/server.rs` | Configura y sirve HTTP; I/O de red, threads y archivos. |
| `DbLogger`, `LogLevel`, `LogRecord` | `src/dblog.rs` | JSONL y rotación; puede contener SQL/datos sensibles. |
| `backup`, `restore`, `verify` | `src/backup.rs` | I/O validado; rehúsa sobrescritura salvo `force`. |

## Constantes/formatos relevantes

`PAGE_SIZE_DEFAULT=4096`, cache por defecto 1024 páginas, body HTTP máximo 100 MiB, conexiones por defecto 64, idle de sesión 300 s, statement log 8 MiB/3 archivos. Verifique estos valores en código antes de automatizar alertas; son estado actual, no promesa perpetua.

## Comandos, rutas y errores

La CLI expone `init`, `info`, `exec`, `repl`, `backup`, `restore` y `verify`, además de flujos multi-base descritos en [USER_MANUAL](../../USER_MANUAL.md). Los endpoints están en [09](09-apis-and-integrations.md). Los errores se catalogan en [ERROR_CODES](../ERROR_CODES.md): 1000 storage, 2000 catálogo, 3000 constraints, 4000 SQL, 5000 server y 6000 observabilidad.

## Llamadas principales

`CLI/server -> parse -> Engine::exec -> exec_* -> Catalog/Tree/Pager`. La escritura actualiza tabla e índices bajo las validaciones aplicables; el pager registra páginas sucias y las persiste mediante WAL. Para firmas exhaustivas use `cargo doc --no-deps --open`.
