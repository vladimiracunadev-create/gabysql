# 06. Explicación profunda del código

## Apertura y recuperación

`Pager::open` valida identidad/tamaño/formato, obtiene un lock exclusivo y resuelve el WAL. Un WAL con commit completo puede reproducirse; registros truncados o con CRC inválido no se aplican silenciosamente. La cache LRU limita memoria. Este flujo protege la precondición single-writer de todo el motor.

## Lectura por clave o rango

`Tree::get` desciende nodos internos eligiendo el hijo cuyo separador cubre la clave, decodifica la hoja y devuelve el payload. Los cursores recorren hojas enlazadas, evitando cargar todo el árbol. Un índice secundario traduce un valor a una clave/bucket y finalmente a PKs; el engine recupera y valida las filas.

## Ejecución SQL

1. `parse` tokeniza y construye uno o varios `Statement` con límites de profundidad.
2. `Engine::exec` crea el contexto de logging y despacha por variante.
3. Los `exec_*` resuelven nombres en `Catalog`, comprueban privilegios/RLS y tipos.
4. El planner elige PK, índice o scan según predicado y estadísticas disponibles.
5. Las expresiones usan semántica de valores/NULL; filtros solo aceptan verdadero.
6. DML valida NOT NULL, UNIQUE, FK, CHECK, tipos y policies.
7. Se actualizan árbol primario e índices; errores retornan `DbError` controlado.
8. En transacción explícita, la confirmación o rollback gobierna la persistencia del lote/sesión.

## Escritura y efectos secundarios

Un INSERT materializa valores/defaults antes de checks, codifica la fila, verifica conflictos y mantiene índices. UPDATE construye la imagen posterior antes de `WITH CHECK`; DELETE respeta referencias y acciones. Triggers/procedimientos pueden provocar ejecución anidada, por lo que existen topes de recursión/iteración. Modificar este orden puede crear inconsistencias incluso si el SQL visible parece correcto.

## HTTP y sesiones

El servidor lee request line/headers/body con límite, autentica token si existe, normaliza el nombre de DB y abre/selecciona estado. `/exec` puede usar una sesión transaccional iniciada en `/tx/begin`; la sesión conserva engine/pager entre requests y expira por inactividad con rollback. La respuesta serializa `ResultSet` o error y actualiza métricas.

## MCP

`gabysql-mcp` implementa JSON-RPC/JSON y un cliente HTTP/1.1 sin crates. Lista tools/resources, valida identificadores para búsqueda vectorial y delega query/execute al server. El audit log del gateway es opt-in y distinto del statement log del motor.

## Casos límite que merecen especial cuidado

- Cambios de codecs o discriminantes persistidos requieren compatibilidad/versionado.
- Índices deben mutar atómicamente con la tabla desde la perspectiva de recovery.
- RLS diferencia superusuario (`current_user=None`) de sesión autenticada.
- `EXPLAIN ANALYZE` ejecuta la sentencia y conserva efectos.
- El SQL completo en logs puede contener información sensible.
