# 01. Visión general del sistema

gabysql es un motor de base de datos relacional embebido escrito en Rust. Guarda cada base en un archivo local, ofrece una API de librería, una CLI, un servidor HTTP/JSON, un gateway MCP para agentes y dos interfaces web: `phpgabyadmin` y `gabymodeler`.

## El sistema explicado para una persona no técnica

Es un archivador digital programable. Una aplicación define tablas, guarda filas y consulta o modifica datos mediante SQL. El motor organiza el archivo para encontrar información, valida reglas como claves únicas y relaciones, y usa un registro WAL para recuperarse de una interrupción. Puede usarse dentro de otro programa o como servicio al que se conectan una web o un agente de IA.

## Problema, público y casos de uso

- Persistencia local de una sola pieza para aplicaciones, herramientas y aprendizaje de motores SQL.
- Servicio relacional pequeño controlado por HTTP en una red confiable.
- Laboratorio reproducible de almacenamiento, SQL, índices, recovery y benchmarking.
- Integración con agentes mediante MCP sin abrir directamente el archivo `.db`.

Los actores son el desarrollador embebido, el usuario de CLI, el cliente HTTP, el operador del servidor, el administrador web y el agente MCP.

## Capacidades observadas

El núcleo implementa pager, páginas con CRC32, WAL, B+Tree, catálogo persistente, índices secundarios, restricciones, parser/ejecutor SQL, transacciones y códigos de error estables. Las superficies externas añaden administración, modelado, métricas, logs, backup/restore verificados y benchmarks. La cobertura SQL exacta y sus límites están en [SQL_REFERENCE](../SQL_REFERENCE.md) y [MISSING_COMMANDS](../MISSING_COMMANDS.md).

## Límites

gabysql no debe presentarse como reemplazo directo de PostgreSQL/MySQL. El diseño prioriza embebido, archivo único y single-writer. El servidor integrado no aporta TLS; `phpgabyadmin` debe permanecer local o detrás de un proxy endurecido. La compatibilidad del formato en disco se define en [COMPATIBILITY](../../COMPATIBILITY.md).

## Tecnologías e integraciones

Rust 2021 compone el motor y binarios sin dependencias de crates declaradas; PHP sirve landing/admin; HTML, CSS y JavaScript vanilla implementan el modelador; Docker empaqueta el servicio. Las integraciones externas observadas son clientes HTTP, GitHub Actions, Google Fonts en el modelador y el protocolo MCP. No se identificaron SaaS obligatorios para ejecutar el núcleo.

## Estado observado

La versión canónica es `0.2.0`. El repositorio incluye 23 archivos Rust y 7 workflows. La hoja de ruta mantiene trabajo pendiente de operación de producto y optimización; no debe confundirse con funcionalidad entregada.
