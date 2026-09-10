# Documentación integral de gabysql

> Análisis del repositorio en el commit `ed4135c`, versión `0.2.0`, realizado el 10 de septiembre de 2026.

Esta colección conecta la visión de producto, el código, la persistencia, las interfaces y la operación de gabysql. Está dirigida a personas nuevas en el proyecto, mantenedores, auditores, operadores y agentes de IA. Los documentos canónicos existentes siguen siendo la fuente normativa; esta carpeta funciona como mapa transversal y declara sus inferencias.

## Cómo leer esta documentación

| Documento | Audiencia principal | Estado |
|---|---|---|
| [01. Visión del sistema](01-system-overview.md) | General | Verificado contra repo |
| [02. Instalación y ejecución](02-installation-and-execution.md) | Desarrollo/operación | Verificado contra comandos y manifiestos |
| [03. Arquitectura](03-architecture.md) | Técnica | Verificado contra módulos |
| [04. Mapa de código](04-code-map.md) | Desarrollo | Inventario del commit analizado |
| [05. Referencia técnica](05-technical-reference.md) | Desarrollo | Catálogo de símbolos principales |
| [06. Explicación profunda](06-deep-code-explanation.md) | Desarrollo avanzado | Flujos críticos |
| [07. Persistencia y modelo de datos](07-database.md) | Datos/auditoría | Verificado contra storage y catálogo |
| [08. Flujo de datos](08-data-flow.md) | Técnica/auditoría | Verificado contra interfaces |
| [09. APIs e integraciones](09-apis-and-integrations.md) | Integración | Remite al contrato API canónico |
| [10. Configuración](10-configuration.md) | Operación | Variables y flags actuales |
| [11. Seguridad](11-security.md) | Seguridad/auditoría | Controles y límites observables |
| [12. Pruebas y calidad](12-testing-and-quality.md) | Calidad | Conteos medidos |
| [13. Despliegue y operación](13-deployment-and-operations.md) | Operación | Procedimientos reproducibles |
| [14. Resolución de problemas](14-troubleshooting.md) | Soporte | Diagnósticos prácticos |
| [15. Riesgos y deuda](15-risks-and-technical-debt.md) | Decisión/auditoría | Informativo; no corrige hallazgos |
| [16. Glosario](16-glossary.md) | Todas | Términos del dominio |
| [17. Resumen ejecutivo](17-executive-summary.md) | Dirección | Síntesis no técnica |
| [18. Guía de incorporación](18-new-developer-guide.md) | Nuevos contribuidores | Itinerario progresivo |
| [19. Matriz de trazabilidad](19-traceability-matrix.md) | Auditoría/desarrollo | Función a código, datos y prueba |

## Convenciones y límites

- **Comprobado** significa visible en código, manifiesto, prueba o documento canónico enlazado.
- **Inferencia basada en el código** identifica una conclusión razonable que no constituye contrato.
- **Requiere validación** señala decisiones operativas o de producto que el repositorio no puede probar.
- Los nombres de archivos y símbolos se escriben en `monoespaciado`.
- Los PDF se generan desde estos Markdown con `scripts/generate_system_docs_pdf.py`; Markdown es la fuente principal.

## Pendientes de validación

- No se identificó un SLO, entorno productivo gestionado ni procedimiento automatizado de rollback de binarios.
- No se documenta una política organizacional de retención para bases, backups, audit logs o statement logs.
- La postura de red/TLS depende de infraestructura externa: el servidor integrado ofrece HTTP, no terminación TLS.
- Las cifras de pruebas son una fotografía del commit; deben regenerarse al cambiar la suite.

Los PDF equivalentes están en [`pdf/`](pdf/). Los diagramas fuente están en Mermaid dentro de los Markdown.
