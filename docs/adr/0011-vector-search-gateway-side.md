# ADR-0011: Búsqueda vectorial del lado del gateway, no en el motor

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-07
**Contexto que la motiva**: continuación de Fase 5 (AI-native) en [ROADMAP.md](../../ROADMAP.md). Implementación entregada como tool nueva en `src/bin/gabysql-mcp.rs`.

## 🧭 Contexto

El uso de bases por agentes LLM trae con él la expectativa de **búsqueda semántica**: dada una consulta en lenguaje natural, embeberla con un modelo y traer las filas cuyo vector está más cerca. Hoy el patrón estándar para resolverlo es:

1. Tener un tipo `VECTOR(n)` nativo en el motor (ej. pgvector en Postgres).
2. Indexarlo con un algoritmo aproximado (HNSW, IVF) para no escanear toda la tabla.
3. Exponer operadores de distancia (`<->`, `<=>`, `<#>` en pgvector).

`gabysql` no tiene nada de eso, y meterlo bien implica:
- Un nuevo tipo de dato y `Value` variant ([src/sql.rs](../../src/sql.rs)) → bump del formato en disco.
- Un nuevo layout de página o estrategia de índice ANN ([src/storage.rs](../../src/storage.rs), [src/bptree.rs](../../src/bptree.rs)) → más bump.
- Cambios en parser, engine y `RESULT_SET` → ondas en todo el motor.

Eso es **un proyecto grande, riesgoso y con superficie de error alta**, especialmente para un motor cuyo eje es estabilidad y compatibilidad del formato (ver [ADR-0007](0007-commercial-path-a.md)). Hacerlo "para validar el use case" es exactamente la trampa que [ROADMAP.md](../../ROADMAP.md) advierte: "crecer features antes de consolidar recovery, constraints y compatibilidad del storage".

Restricciones del proyecto:
- Cero dependencias externas en el core ([ADR-0001](0001-rust-zero-deps-core.md)).
- Sin bump del formato en disco si no es estrictamente necesario.
- Cualquier feature AI-native debe seguir la línea trazada por [ADR-0010](0010-mcp-gateway.md): **el motor no se toca**.

Pregunta práctica: **¿se puede entregar búsqueda vectorial usable hoy sin tocar el motor?**

## 💡 Decisión

Implementar búsqueda vectorial **en el gateway MCP** (`gabysql-mcp`), no en el motor. Concretamente:

1. Los vectores se almacenan como **columna `TEXT`** que contiene un array JSON de floats: `"[0.1, 0.2, ..., 0.768]"`. El motor no sabe que es un vector — solo ve texto.
2. El gateway expone una nueva tool MCP **`gabysql_vector_search`** con argumentos:
   - `db` (opcional)
   - `table` (requerido)
   - `pk_column` (default `"id"`)
   - `vector_column` (requerido)
   - `query` (requerido — array JSON de floats)
   - `top_k` (default `10`)
   - `metric` (default `"cosine"`; acepta también `"euclidean"`/`"l2"` y `"dot"`/`"ip"`)
3. La tool ejecuta `SELECT <pk>, <vec_col> FROM <table>` vía el HTTP existente (tras validar que los identificadores son `[A-Za-z0-9_]+` para evitar inyección), parsea cada fila del lado del gateway, computa la distancia en Rust contra el vector de consulta y devuelve top-k por heap selection.
4. Los identificadores se validan con `safe_ident`; cualquier carácter fuera de `[A-Za-z0-9_]` o que empiece con dígito hace fallar la tool antes de pegar al server.
5. Filas con vector mal formado o de dimensión distinta a la query se cuentan en el campo `skipped` de la respuesta (el agente sabe que las saltó, no se silencia el problema).

Resultado: el agente puede pedir "tráeme las 5 filas más parecidas a este vector" sin que el motor sepa nada de vectores ni distancias.

## 🔄 Alternativas consideradas

### Tipo `VECTOR(n)` nativo con bump de formato
- **Pro**: el camino "correcto" a largo plazo. Permite índices ANN, operadores SQL nativos (`ORDER BY vec <-> query LIMIT k`), enforcement de dimensión, almacenamiento eficiente (16 bytes/dim sin overhead JSON).
- **Contra**: bump de VERSION, cambios en `sql.rs`/`storage.rs`/`catalog.rs`/`bptree.rs`. Muchos meses de trabajo bien hecho. **Y aún no tenemos un solo usuario pidiendo esto** — es prematuro invertir.
- **Veredicto**: rechazada **por ahora**. Esta ADR se considera *stepping stone* para validar el use case. Si la tool del gateway resulta usada y aparecen workloads >100K vectores, se abre una ADR nueva para promover a `VECTOR(n)` nativo y esta queda **superseded** sin haber roto nada.

### Vectores como `TEXT`, distancias en SQL puro (UDFs)
- **Pro**: el cómputo se queda en el motor; no hay scan + serializar + parsear en el cliente.
- **Contra**: `gabysql` no tiene UDFs ni soporta funciones agregadas custom. Implementarlo es comparable a meter el tipo nativo. Mismo problema que la alternativa anterior.
- **Veredicto**: rechazada.

### Sidecar service en otro proceso
- **Pro**: aísla aún más el cómputo vectorial.
- **Contra**: tres procesos en lugar de dos. La tool del gateway resuelve esto en un proceso adicional sin necesitar un sidecar dedicado. Sobre-ingeniería.
- **Veredicto**: rechazada.

### **Vectores como `TEXT`, distancias en el gateway** (decisión)
- **Pro**: cero impacto en el motor. Cero bump de formato. Una tool nueva en el binario que ya existe ([ADR-0010](0010-mcp-gateway.md)). Funciona desde el primer día. El usuario inserta vectores con `INSERT INTO docs (id, content, embedding) VALUES (1, 'texto', '[0.1,0.2,...]')` — SQL estándar, sin sintaxis especial.
- **Contra**: scan O(n·d) por query. Para 10K filas × 768 dims es ~30 ms; para 1M filas se vuelve segundos. Hay overhead de serializar/parsear JSON en cada query. Y los vectores ocupan ~5× más en disco que un float32 packing nativo (≈8 bytes ASCII por dim vs 4 bytes binarios).
- **Veredicto**: **aceptada**, con condiciones de salida explícitas (ver consecuencias).

## 📊 Consecuencias

### Positivas
- **Búsqueda vectorial usable hoy.** Cualquier agente con acceso al gateway MCP puede hacer top-k semántico sin que nadie haya tocado el motor.
- **Cero riesgo para el formato en disco.** No hay bump de VERSION; las DBs existentes siguen siendo válidas; las nuevas pueden mezclar columnas vectoriales y normales sin problemas.
- **Permite validar el use case antes de invertir en el camino caro.** Si la tool no se usa, no perdimos nada. Si se usa mucho con datasets grandes, ya tenemos señal para abrir el ADR de `VECTOR(n)` nativo con datos.
- **Tres métricas estándar de fábrica** (cosine, euclidean, dot) cubren ~95% de los embeddings comerciales (OpenAI, Cohere, Voyage, modelos locales) sin requerir configuración.
- **Sin nuevas deps**, sigue cumpliendo [ADR-0001](0001-rust-zero-deps-core.md).

### Negativas
- **Escala mal**. Para >100K vectores el scan O(n) en cada query empieza a doler. La tool no oculta esto: la respuesta lleva `scanned` y `skipped` para que el agente vea el costo. El RUNBOOK menciona explícitamente que la tool es para datasets chicos.
- **Sin enforcement de dimensión**. Si una fila tiene un vector de dimensión distinta a la query, se descarta y se cuenta en `skipped`. No hay constraint del motor que lo evite — porque para el motor es solo `TEXT`.
- **Storage ineficiente**: ~5× más espacio que packing binario.
- **El cómputo viaja por la red dos veces** (server → gateway → cliente). Para deploys remotos del server, el ancho de banda puede importar.

### Neutras
- La superficie de la API MCP crece en una tool. Los clientes que no la usen no pagan nada.
- El binario `gabysql-mcp` crece ~250 líneas (distancias + heap + parser de vectores + validador de identificadores), sin nuevas deps.
- `safe_ident` queda como helper general: cualquier futura tool del gateway que interpole identificadores SQL puede reusarlo.

## 🚪 Condiciones de salida (cuándo promover a `VECTOR(n)` nativo)

Esta ADR queda **superseded por una ADR futura** cuando se cumpla **al menos uno** de estos criterios:

- Un usuario reporta dataset > 100K vectores y latencia inaceptable.
- Aparece demanda de operadores SQL nativos (`ORDER BY emb <-> q LIMIT k`).
- Se necesita enforcement de dimensión a nivel de `CREATE TABLE`.
- Se necesita índice ANN (HNSW/IVF) — caso en que el scan O(n) ya no es opción.

Hasta entonces, esta solución es la entrega correcta.

## 🔗 Referencias

- Implementación: [src/bin/gabysql-mcp.rs](../../src/bin/gabysql-mcp.rs) (`vector_search`, `distance`, `parse_metric`, `push_top_k`, `safe_ident`).
- Tool MCP: `gabysql_vector_search` (visible en `tools/list`).
- Tests: módulo `#[cfg(test)] mod tests` del binario — 9 tests nuevos cubren las 3 distancias, dimension mismatch, vector cero, top-k heap, validador de identificadores, alias de métrica, y schema de la tool.
- ADRs encadenadas: [ADR-0010](0010-mcp-gateway.md) (gateway MCP — esta ADR es su primera extensión funcional), [ADR-0001](0001-rust-zero-deps-core.md) (cero deps, intacta), [ADR-0007](0007-commercial-path-a.md) (camino A — vectores entran sin tocar el nicho embebido).
- Prior art:
  - **pgvector** (`<->`, `<=>`, `<#>`): el camino "correcto" pero requiere ser parte del motor.
  - **DuckDB VSS extension**: similar — requiere ser una extensión cargada en el motor.
  - **SQLite-VSS**: módulo cargable. Tampoco aplica porque `gabysql` no tiene mecanismo de extensiones.
  - **LanceDB / Chroma / Qdrant**: bases dedicadas a vectores. Demasiado para el use case que estamos validando.
  - El patrón "vectores como JSON en TEXT y cómputo en el cliente" es el que usan muchas integraciones tempranas con SQLite antes de migrar a sqlite-vss. Mismo razonamiento aquí: validar antes de invertir.
