# 16. Glosario

| Término | Definición |
|---|---|
| AST | Representación estructurada de una sentencia después del parseo. |
| B+Tree | Árbol ordenado persistente usado para claves, catálogo e índices. |
| Catálogo | Metadatos que describen tablas, columnas y otros objetos. |
| CRC32 | Checksum para detectar corrupción accidental de páginas/registros. |
| DDL/DML | SQL que define estructura / lee o modifica filas. |
| Embebido | Motor integrado en la misma aplicación/proceso, sin servidor obligatorio. |
| Engine | Componente que ejecuta el AST y aplica reglas SQL. |
| FK | Foreign key; relación que exige una fila padre válida. |
| MCP | Protocolo para que agentes descubran tools/resources y las invoquen. |
| MCV/NDV | Valores más comunes / cantidad estimada de valores distintos. |
| Pager | Abstracción que administra páginas, cache, I/O y WAL. |
| PK | Primary key; identificador único de fila. |
| RLS | Row-Level Security; policies que filtran/validan filas por identidad. |
| ResultSet | Columnas, filas y mensaje devueltos por una ejecución. |
| Single-writer | Solo un escritor/instancia abre la base a la vez. |
| Statement log | JSONL del motor con SQL, resultado y código de error. |
| WAL | Write-Ahead Log; registro previo usado para durabilidad/recovery. |
| `gabysql` | CLI y nombre del crate/motor. |
| `gabysql-server` | Adaptador HTTP/JSON del motor. |
| `gabysql-mcp` | Adaptador MCP que consume el servidor HTTP. |
| `phpgabyadmin` | Interfaz administrativa PHP. |
| `gabymodeler` | Modelador entidad-relación web/escritorio. |
| `gabybench` | Workload reproducible para medir capacidades y latencias. |
