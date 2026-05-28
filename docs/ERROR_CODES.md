# 📒 Catálogo de códigos de error de gabysql

> **Cada error user-facing del motor lleva un código estable de 4 dígitos en el prefijo `[GBY-NNNN]`.** Inspirado en los `ER_*` de MySQL: el código es el contrato, el texto humano que viene después puede evolucionar sin romper a los clientes que reaccionan al código.

Las definiciones canónicas viven en [`src/errors.rs`](../src/errors.rs::codes). Este documento es la vista operacional: qué dispara cada código, cómo se resuelve, ejemplo de mensaje real.

Para el **estilo y filosofía** de los mensajes (qué/por qué/cómo), ver [ERROR_HANDLING.md](ERROR_HANDLING.md). Para los **patrones operacionales** con cada error, ver [TROUBLESHOOTING.md](../TROUBLESHOOTING.md) y [RUNBOOK.md](../RUNBOOK.md).

---

## 🧭 Cómo leer un código

Todo mensaje de gabysql con código tiene esta forma:

```
[GBY-2001] tabla no existe: orders
```

| Pieza | Significado |
| :--- | :--- |
| `GBY-` | Prefijo fijo del motor (gabysql) |
| `2001` | Número estable y único de este error. Padded a 4 dígitos con `0` |
| `tabla no existe: orders` | Mensaje humano, en español, con el nombre concreto del objeto |

**Lo que es contrato**: el número.
**Lo que puede cambiar entre versiones**: la frase humana (mejor redacción, más contexto, traducciones futuras). Si tu herramienta parsea el texto, usá una regex sobre el número.

---

## 🔢 Rangos numéricos

| Rango | Subsistema |
| :---: | :--- |
| **1000–1999** | Storage / Pager / WAL / file lock |
| **2000–2999** | Catalog / schema / identificadores |
| **3000–3999** | Constraints (PK, NOT NULL, UNIQUE, FK) |
| **4000–4999** | Superficie SQL (parser, planner, limitaciones) |
| **5000–5999** | Server / HTTP / auth |

Los rangos están reservados (no se asignan códigos cross-range) para que un código sea suficiente para inferir el subsistema sin abrir docs.

---

## 1000–1999 · Storage

| Código | Símbolo | Causa | Remedio |
| :---: | :--- | :--- | :--- |
| `1001` | `REFUSE_OVERWRITE_DB` | `gabysql init` o `Pager::create` sobre un archivo `.db` ya existente. | Use `gabysql init --force <file.db>` si la intención es resetear. |
| `1002` | `DB_LOCKED_BY_PROCESS` | Otro proceso `gabysql` tiene la DB abierta (file lock cross-process, ADR-0013). | Detener el otro proceso (`gabysql-server`, CLI, etc.) o esperar a que cierre. Ver [TROUBLESHOOTING.md §database is locked](../TROUBLESHOOTING.md#-database-is-locked-by-another-process). |
| `1003` | `UNSUPPORTED_FORMAT_VERSION` | El `.db` declara una `VERSION` distinta a la del binario actual. | Re-crear la base con el binario actual: `gabysql init <file.db>`. No hay migración automática entre versiones del formato. |
| `1004` | `BAD_MAGIC_BYTES` | El archivo no empieza con `GABYSQL1` — no es una DB gabysql. | Apuntar al archivo correcto, o crear uno nuevo con `gabysql init`. |
| `1005` | `TX_ALREADY_STARTED` | `Pager::begin()` con una transacción ya abierta (uso embebido). | Llamar a `commit()` o `rollback()` antes del nuevo `begin()`. |
| `1006` | `NO_ACTIVE_TX` | `Pager::commit()` o `rollback()` sin un `begin()` previo. | Asegurar que `begin()` corra antes de cualquier mutación. |
| `1007` | `PAGE_CRC_INVALID` | El trailer CRC32 de una página no coincide con el contenido. Corrupción de disco o WAL parcialmente aplicado. | Restaurar desde el último backup. Considerar `gabysql verify <db>` para localizar la página. |
| `1008` | `WAL_RECORD_CRC_INVALID` | Un record del WAL falla CRC durante el replay. | El WAL está corrupto — descartar `.wal`, restaurar `.db` desde backup. Ver [RUNBOOK.md §Recovery tras caída](../RUNBOOK.md). |
| `1009` | `UNSUPPORTED_PAGE_SIZE` | El archivo declara un `page_size` distinto al fijo del build (`4096`). | Re-crear la DB con el binario actual. |

---

## 2000–2999 · Catalog / Schema

| Código | Símbolo | Causa | Remedio |
| :---: | :--- | :--- | :--- |
| `2001` | `TABLE_NOT_FOUND` | Operación sobre una tabla que no existe en la DB. | Crear la tabla con `CREATE TABLE`, o corregir el nombre. `SHOW DATABASES` y `phpgabyadmin` listan lo disponible. |
| `2002` | `COLUMN_NOT_FOUND` | Columna referenciada (en `INSERT`, `UPDATE`, `WHERE`, `SELECT`) no existe en la tabla. | Verificar el schema con `GET /schema?table=...` o `phpgabyadmin → Structure`. |
| `2003` | `INDEX_NOT_FOUND` | `DROP INDEX` sobre un índice inexistente. | `SHOW DATABASES` / `phpgabyadmin → Structure` muestran los índices definidos. |
| `2004` | `TABLE_ALREADY_EXISTS` | `CREATE TABLE` sobre un nombre ya tomado. | Usar otro nombre o `DROP TABLE` previo. |
| `2005` | `INDEX_ALREADY_EXISTS` | `CREATE INDEX` con un nombre que ya existe en la DB, o sobre una columna que ya tiene índice. | Usar otro nombre. Esta versión admite **un solo índice por columna**. |
| `2006` | `INVALID_IDENTIFIER` | Nombre de tabla/columna/índice vacío, demasiado largo (>64), con caracteres prohibidos o palabra reservada del motor. | Identificadores válidos: `[A-Za-z_][A-Za-z0-9_]{0,63}`, no reservados. |
| `2007` | `DUPLICATE_COLUMN_NAME` | `CREATE TABLE` con dos columnas que comparten nombre (case-insensitive), o `INSERT`/`UPDATE` con la misma columna repetida en la lista. | Eliminar la duplicada. |
| `2008` | `INCOMPATIBLE_DEFAULT_TYPE` | El literal de `DEFAULT` no es compatible con el tipo declarado de la columna (ej. `DEFAULT 'foo'` en `INT`). | Ajustar el literal al tipo de la columna. |
| `2009` | `INDEX_ON_JSON` | `CREATE INDEX` sobre una columna `JSON`. | No soportado. Cambiar el tipo o usar otro motor para queries vectoriales (`gabysql-mcp` con búsqueda semántica, ADR-0011). |

---

## 3000–3999 · Constraints

| Código | Símbolo | Causa | Remedio |
| :---: | :--- | :--- | :--- |
| `3001` | `DUPLICATE_PRIMARY_KEY` | `INSERT` con una PK que ya existe. | Usar otro valor de PK o `UPDATE` la fila existente. |
| `3002` | `NOT_NULL_VIOLATED` | `INSERT`/`UPDATE` deja `NULL` en una columna declarada `NOT NULL`. | Pasar un valor explícito o agregar un `DEFAULT` no nulo a la columna. |
| `3003` | `UNIQUE_VIOLATED` | `INSERT`/`UPDATE` viola un índice `UNIQUE`. El mensaje incluye la PK existente que ya tiene ese valor. | Usar otro valor o modificar la fila conflictiva. |
| `3004` | `FK_PARENT_MISSING` | `INSERT`/`UPDATE` con un valor de FK que no existe en la tabla padre. | Crear primero la fila padre o usar un valor de FK válido. |
| `3005` | `FK_RESTRICT_BLOCKS_DELETE` | `DELETE` sobre una fila que tiene hijos referenciándola con `ON DELETE RESTRICT`. El mensaje incluye cuántos hijos hay. | Borrar primero los hijos o redefinir la FK con `ON DELETE CASCADE`. |
| `3006` | `ROW_NOT_FOUND_FOR_PK` | `UPDATE` o `DELETE` sobre una PK que no existe. | Verificar la PK con un `SELECT` previo. |
| `3007` | `PRIMARY_KEY_NULL` | `INSERT`/`UPDATE` pasa `NULL` para la PK. | La PK no puede ser `NULL` por definición — pasar un entero. |
| `3008` | `CHECK_VIOLATED` | (Reservado para L2.) `INSERT`/`UPDATE`/`UPSERT` viola un constraint `CHECK (expr)`. NULL pasa según 3VL ANSI. | Revisar el predicado declarado o pasar un valor que lo satisfaga. |
| `3009` | `FK_SET_NULL_VIOLATES_NOT_NULL` | `ON DELETE SET NULL` intentó poner `NULL` en una columna FK declarada `NOT NULL`. La cascade aborta sin rollback parcial. | Quitar `NOT NULL` de la columna del child o redefinir la FK con `CASCADE`/`SET DEFAULT`. |
| `3010` | `FK_SET_DEFAULT_MISSING` | `ON DELETE SET DEFAULT` no encontró un `DEFAULT` declarado para la columna FK del child. | Declarar un `DEFAULT <valor>` en la columna o cambiar la acción a `CASCADE`/`SET NULL`. |

---

## 4000–4999 · Superficie SQL

| Código | Símbolo | Causa | Remedio |
| :---: | :--- | :--- | :--- |
| `4001` | `WHERE_OPERATOR_UNSUPPORTED` | `WHERE` con un operador fuera de la gramática actual, o `=`/`BETWEEN` sobre columna no-PK sin índice secundario (fast-path indexado). | Gramática soportada (E1+E2): `=`, `<`, `>`, `<=`, `>=`, `<>`/`!=`, `BETWEEN`, `IS [NOT] NULL`, `[NOT] LIKE`, `[NOT] IN (lista \| SELECT)`, `EXISTS`, combinados con `AND`/`OR`/`NOT` + paréntesis. Ver [SQL_REFERENCE.md](SQL_REFERENCE.md). Para `=`/`BETWEEN` sin índice: crear `CREATE INDEX` o reescribir el WHERE para que caiga al post-filter (e.g. envolver en `(...) AND TRUE`). |
| `4002` | `BETWEEN_REQUIRES_PK_OR_INT_INDEX` | `WHERE col BETWEEN a AND b` sobre una columna que no es PK ni tiene índice `OrderedInt` (solo INT con índice). | Usar `=` en su lugar, indexar la columna si es `INT`, o filtrar por PK. Ver [ADR-0017](adr/0017-int-ordered-index-version-7.md). |
| `4003` | `UPDATE_DELETE_REQUIRES_PK_FILTER` | (Histórico) `UPDATE` o `DELETE` con `WHERE` sobre columna no-PK. **Inactivo desde el bloque E3 (2026-05-25)**: `UPDATE`/`DELETE` ahora aceptan cualquier `WHERE`. La constante se conserva por estabilidad — nunca se reemite. | — |
| `4004` | `LIMIT_NEGATIVE` | `LIMIT n` con `n < 0`. | `LIMIT` admite valores `>= 0`. |
| `4005` | `OFFSET_NEGATIVE` | `OFFSET n` con `n < 0`. | `OFFSET` admite valores `>= 0`. |
| `4006` | `STRING_LITERAL_UNTERMINATED` | Literal `'...'` sin la comilla de cierre. | Cerrar el literal o escapar la comilla interna duplicándola (`''`). |
| `4007` | `INSERT_COLS_VS_VALUES_MISMATCH` | `INSERT INTO t (a,b,c) VALUES (1,2)` — la cantidad de columnas no coincide con la cantidad de valores. | Igualar las listas. |
| `4008` | `UPDATE_PK_NOT_ALLOWED` | `UPDATE t SET pk = ...` — esta versión no admite cambiar la PK. | Hacer `INSERT` con la nueva PK y `DELETE` de la vieja. |
| `4009` | `LIMIT_DUPLICATED` | `LIMIT n LIMIT m` — `LIMIT` aparece más de una vez. | Usar uno solo. |
| `4010` | `OFFSET_DUPLICATED` | `OFFSET n OFFSET m` — `OFFSET` aparece más de una vez. | Usar uno solo. |
| `4011` | `SUBQUERY_MUST_RETURN_ONE_COLUMN` | `WHERE col IN (SELECT a, b ...)` — la subquery proyecta más (o menos) de una columna. | Reescribir la subquery para que devuelva una sola columna. |
| `4012` | `IN_PK_TYPE_MISMATCH` | `WHERE pk_int IN (SELECT t FROM ...)` donde la subquery devuelve valores no-`INT`. | Ajustar la subquery para que devuelva `INT`, o filtrar por una columna no-PK. |
| `4013` | `IN_REQUIRES_PK_OR_INDEX` | `WHERE col IN (SELECT ...)` (o `= (SELECT ...)`) cuando `col` no es PK ni tiene índice secundario. | Crear índice (`CREATE INDEX idx_t_col ON t (col);`) o filtrar por la PK. |
| `4014` | `SCALAR_SUBQUERY_TOO_MANY_ROWS` | `WHERE col = (SELECT ...)` cuya subquery devolvió más de 1 fila. | Restringir la subquery con un `WHERE`/`LIMIT 1`, o usar `IN (SELECT ...)` en lugar de `=`. |
| `4015` | `EXISTS_REQUIRES_SUBQUERY` | `EXISTS`/`NOT EXISTS` no seguido por `(SELECT ...)`. | Escribir `EXISTS (SELECT ... FROM ... [WHERE ...])`. |
| `4016` | `OUTER_COLUMN_REF_INVALID` | `col = outer_table.col` usado fuera de una subquery correlacionada, o la tabla outer / columna outer no están en el alcance. | Mover la referencia dentro de un `EXISTS (SELECT ... WHERE inner_col = outer_table.outer_col)`, o usar un literal/subquery escalar. |
| `4017` | `TABLE_ALIAS_DUPLICATED` | Dos tablas del `FROM` expuestas con el mismo qualifier (alias o nombre). | Asignar alias distintos: `FROM t AS a JOIN otra AS b`. |
| `4018` | `COLUMN_AMBIGUOUS` | Columna sin qualifier que existe en más de una tabla del `FROM`. | Cualificar con `tabla.col` o `alias.col`. |
| `4019` | `COLUMN_QUALIFIER_NOT_FOUND` | `tabla.col` donde `tabla` no es nombre ni alias del FROM, o `col` no existe en ninguna tabla. | Verificar el nombre y los alias declarados. |
| `4020` | `JOIN_PREDICATE_REQUIRED` | `INNER JOIN ...` sin `ON l = r`. | Agregar `ON tabla1.col = tabla2.col` (o usar `CROSS JOIN` si querés cartesiano). |
| `4021` | `CROSS_JOIN_WITH_ON` | `CROSS JOIN ... ON ...` — el cartesian product no admite predicado. | Cambiar a `INNER JOIN ... ON ...`. |
| `4022` | `USING_COLUMN_INVALID` | `JOIN ... USING (col)` con col que no existe en ambas tablas, o USING con cantidad de columnas no soportada (este release: exactamente 1). | Verificar que `col` exista en ambos lados; reescribir con `ON` para multi-columna. |
| `4023` | `NATURAL_JOIN_NO_COMMON_COLUMN` | `NATURAL JOIN` cuyas tablas no comparten exactamente 1 columna por nombre (0 o >1). | Usar `JOIN ... ON` o `USING` explícito. |
| `4024` | `WHERE_COMBINATOR_CORRELATED_UNSUPPORTED` | **DEPRECADO (Bloque H, 2026-05-26):** el motor ya no lo emite — `EXISTS`/`EqColumnRef` correlacionados dentro de `AND`/`OR`/`NOT` están soportados. Slot reservado por estabilidad del catálogo. | — |
| `4025` | `AGGREGATE_OUTSIDE_HAVING_OR_SELECT` | Función agregada (`COUNT`, `SUM`, `AVG`, `MIN`, `MAX`) usada fuera del SELECT list o HAVING — típicamente en `WHERE`. | Moverla a `HAVING`, o aliasearla en el `SELECT` y referirse por alias. |
| `4026` | `AGGREGATE_ARG_INVALID` | Argumento inválido de función agregada: `SUM(*)`, `AVG(DISTINCT x)`, `MIN(*)`, o tipos no-numéricos en `SUM`/`AVG`. | Solo `COUNT(*)` y `COUNT(DISTINCT col)` son combinaciones especiales aceptadas. Para `SUM`/`AVG` usar columnas INT o FLOAT. |
| `4027` | `SELECT_COLUMN_NOT_IN_GROUP_BY` | `SELECT` mezcla columnas no-agregadas que no figuran en `GROUP BY`. Cumple la regla ANSI estricta. | Agregar la columna al `GROUP BY` o envolverla en una función agregada (`MIN`/`MAX`). |
| `4028` | `AGGREGATE_OVER_JOIN_UNSUPPORTED` | Agregados (`COUNT/SUM/AVG/MIN/MAX`) o `GROUP BY`/`HAVING` sobre un `SELECT` con `JOIN`. El executor de JOIN aún no implementa el stage de agregación. | Reescribir como subquery agregada sobre la tabla base (e.g. `SELECT COUNT(*) FROM (SELECT ...)` — pero los derived tables también están en backlog; por ahora separar la query). |
| `4029` | `TX_BEGIN_DOUBLE` | `BEGIN` SQL emitido con una transacción explícita ya abierta. `SAVEPOINT` no soportado todavía — la única forma de salir es `COMMIT` o `ROLLBACK`. | Cerrar la transacción anterior antes de abrir una nueva. |
| `4030` | `TX_END_WITHOUT_BEGIN` | `COMMIT` o `ROLLBACK` SQL emitido sin `BEGIN` previo. Las sentencias fuera de un bloque explícito son auto-commit por batch — no hace falta cerrarlas manualmente. | Eliminar el `COMMIT`/`ROLLBACK` redundante, o agregar el `BEGIN` faltante al inicio del bloque. |
| `4031` | `ON_CONFLICT_INVALID` | `ON CONFLICT` con acción no soportada o malformada (acciones aceptadas: `DO NOTHING`, `DO UPDATE SET ...`). `REPLACE` solo se obtiene vía `REPLACE INTO ...`. | Reescribir la cláusula con una acción soportada o usar `REPLACE INTO`. |
| `4032` | `ON_CONFLICT_TARGET_NOT_UNIQUE` | `ON CONFLICT (col)` cuyo `col` no es PK ni tiene índice UNIQUE — sin un constraint indexado no se puede detectar el conflicto. | Crear `CREATE UNIQUE INDEX` sobre la columna, usar la PK como target, u omitir `(col)` para que la cláusula aplique a cualquier constraint. |
| `4033` | `PARSE_DEPTH_EXCEEDED` | Expresión SQL con anidamiento mayor al permitido por el parser (defensa contra stack exhaustion via paréntesis o `NOT` encadenados). | Simplificar la expresión o partirla en varias consultas. |
| `4034` | `SCALAR_FN_ARITY` | Función escalar invocada con la cantidad equivocada de argumentos (e.g. `LENGTH()` o `SUBSTR(s)`). | Pasar la cantidad correcta de argumentos según la signatura de la función. |
| `4035` | `SCALAR_FN_TYPE_MISMATCH` | Argumento de una función escalar con un tipo no aceptado (e.g. `LENGTH(123)` o `ABS('x')`). | Usar el tipo correcto, o envolver con `CAST(... AS TYPE)`. |
| `4036` | `CAST_INVALID` | `CAST(x AS TYPE)` cuyo valor no se puede convertir al tipo destino (e.g. `CAST('abc' AS INT)`). | Pre-validar el valor; usar `COALESCE`/`CASE` para descartar valores inválidos antes del `CAST`. |
| `4037` | `SCALAR_FN_UNKNOWN` | Invocación a una función escalar que el motor no reconoce (e.g. `FOO(1)`). | Ver la lista de funciones soportadas en `SQL_REFERENCE.md` (sección "Funciones escalares"); algunas todavía no están implementadas. |
| `4038` | `CASE_BRANCH_TYPE_MISMATCH` | Condición de un `CASE WHEN` searched que no evalúa a BOOL. | Reescribir la condición como una comparación (`x > 10`, `x IS NULL`, etc.). |
| `4039` | `EXPR_IN_PREDICATE_NOT_SUPPORTED` | G2 (cerrado por G3): operador postfix (`IS NULL`/`LIKE`/`IN`/`BETWEEN`) con LHS expresional. Desde G3 la query funciona; el código queda reservado y sin emisión activa para preservar el contrato de estabilidad. | — |
| `4040` | `WHERE_EXPR_NOT_BOOLEAN` | G2: expresión usada como predicado completo del WHERE/HAVING que no rinde BOOL/NULL, e.g. `WHERE LENGTH(x)` sin comparador. | Agregar el operador de comparación faltante (`= 0`, `> 3`, etc.). |
| `4041` | `UPDATE_SET_TYPE_MISMATCH` | G2: la RHS de `UPDATE ... SET col = <expr>` rinde un tipo incompatible con la columna (e.g. TEXT en INT). | Envolver con `CAST(... AS TIPO)` explícito si la conversión es intencional. |
| `4042` | `ARITH_OVERFLOW` | G3: operación entera con overflow (e.g. `i64::MAX + 1`). | Promover a FLOAT con `CAST(... AS FLOAT)` antes de la operación si el rango es necesario. |
| `4043` | `DIVISION_BY_ZERO` | G3: divisor cero en `/` o `%` (entero o flotante). | Pre-filtrar con `WHERE divisor <> 0` o usar `NULLIF(divisor, 0)` para devolver NULL. |
| `4044` | `ARITH_TYPE_MISMATCH` | G3: operador aritmético sobre tipos incompatibles (e.g. `'abc' + 1`). | Reescribir con `CAST` explícito o usar `\|\|` si la intención era concatenar. |
| `4045` | `MATH_DOMAIN` | G3: función matemática fuera del dominio real (e.g. `SQRT(-1)`, `POWER(0, -1)`). | Pre-filtrar el dominio del argumento o devolver NULL con `CASE WHEN ... THEN ... ELSE NULL END`. |
| `4046` | `DATE_PARSE_ERROR` | G3: TEXT no parseable como DATE/DATETIME en `DATE_ADD`/`DATEDIFF`/`EXTRACT`/`STRFTIME`. | Asegurar el formato `YYYY-MM-DD` o `YYYY-MM-DD HH:MM:SS`. |
| `4047` | `EXTRACT_FIELD_INVALID` | G3: `EXTRACT(<campo> FROM ...)` con un campo desconocido. | Usar uno de `YEAR`, `MONTH`, `DAY`, `HOUR`, `MINUTE`, `SECOND`. |
| `4048` | `DERIVED_TABLE_REQUIRES_ALIAS` | H (2026-05-26): `FROM (SELECT ...)` sin alias — ANSI exige nombre obligatorio para poder referenciar las columnas del derived. | Agregar `AS sub` (o un bare `sub`) después del `)`. |
| `4049` | `DERIVED_DUPLICATE_COLUMN` | H: la subquery de un derived table proyecta dos columnas con el mismo nombre. | Usar alias en la subquery (`SELECT a AS x, b AS y`). |
| `4050` | `DERIVED_COLUMN_TYPE_AMBIGUOUS` | H: reservado para validación estricta futura de tipos mixtos en derived. Por ahora el motor cae a TEXT como fallback documentado. | — |
| `4051` | `SCALAR_SUBQUERY_IN_EXPR_REQUIRES_PARENS` | H: reservado para subquery escalar en Expr sin paréntesis envolventes. Por ahora el parser solo acepta `(SELECT ...)`. | — |
| `4052` | `VALUES_IN_FROM_REQUIRES_ALIAS` | I (2026-05-26): `FROM (VALUES (...), ...)` sin alias de tabla, o sin lista de columnas (`AS t(c1, c2, ...)`). VALUES no provee nombres por sí mismo. | Agregar `AS t(c1, c2, ...)` después del `)`. |
| `4053` | `VALUES_COLUMN_ALIAS_ARITY` | I: la lista `t(c1, c2, ...)` tiene una arity distinta a las tuplas de `VALUES`. | Igualar el número de aliases al número de expresiones por fila. |
| `4054` | `SET_OP_ARITY_MISMATCH` | I: `UNION` / `INTERSECT` / `EXCEPT` entre dos queries con distinto número de columnas. | Igualar la arity proyectada por ambos SELECT. |
| `4055` | `SET_OP_TYPE_MISMATCH` | I: tipos incompatibles entre la columna `N` del LHS y la del RHS de un set op. INT/FLOAT promueven entre sí; cualquier otra mezcla rompe. | Aplicar `CAST` para uniformar, o reordenar columnas. |
| `4056` | `VALUES_ROW_ARITY_MISMATCH` | I: dos filas del mismo `VALUES` con distinta arity. | Igualar el número de expresiones en cada fila. |
| `4057` | `VALUES_EMPTY` | I: `VALUES` sin ninguna fila — sintaxis inválida. | Agregar al menos una tupla `(...)`. |
| `4058` | `CTAS_REQUIRES_INT_FIRST_COLUMN` | K1 (2026-05-26): `CREATE TABLE t AS SELECT ...` cuya primera columna del result-set no es INT no-NULL. La primera columna se usa como PK INT de la nueva tabla. | Antepoñer un `id INT` en el SELECT, o usar la forma `CREATE TABLE t (id, ...) AS SELECT 1, ...`. |
| `4059` | `CANNOT_DROP_PRIMARY_KEY` | K1: `ALTER TABLE t DROP COLUMN <pk>` — la PRIMARY KEY no se puede borrar. | Usar `DROP TABLE` si la intención es rehacer el esquema. |
| `4060` | `CANNOT_DROP_INDEXED_COLUMN` | K1: la columna a borrar tiene un índice asociado (`CREATE INDEX` o `UNIQUE` inline). | Ejecutar `DROP INDEX <name>` antes del `DROP COLUMN`. |
| `4061` | `CANNOT_DROP_REFERENCED_COLUMN` | K1: la columna a borrar participa en una FOREIGN KEY — saliente (la columna referencia otra tabla) o entrante (otra tabla la referencia). | Recrear la tabla sin esa FK o esperar al soporte de `ALTER ... DROP CONSTRAINT`. |
| `4062` | `RENAME_TARGET_EXISTS` | K1: `RENAME TABLE old TO new` (o `RENAME COLUMN old TO new`) cuyo destino ya está tomado por otra tabla/columna. | Elegir un nombre libre. |
| `4063` | `CTAS_COLUMN_ALIAS_ARITY` | K1: `CREATE TABLE t (a, b) AS SELECT x, y, z FROM ...` — la lista de aliases no matchea la arity del SELECT. | Igualar el número de aliases al número de columnas que proyecta el SELECT. |
| `4064` | `COMPOSITE_PK_REQUIRES_ALL_INT` | K2 (2026-05-26): `PRIMARY KEY (a, b, ...)` con alguna columna no-INT o nullable. El fingerprint i64 que sostiene la PK compuesta exige all-INT NOT NULL (ver ADR-0019). | Declarar todas las columnas PK como `INT NOT NULL`, o modelar con surrogate `id INT PRIMARY KEY` + `UNIQUE (a, b, ...)`. |
| `4065` | `PRIMARY_KEY_DUPLICATED` | K2: `PRIMARY KEY` declarada dos veces — inline en una columna + table-level, o dos columnas con `PRIMARY KEY` inline. | Elegir una única forma de declarar la PK. |
| `4066` | `FK_TARGET_NOT_INDEXED` | K2 (reservado): una FOREIGN KEY apunta a columna del padre que no es ni PK ni UNIQUE. Hoy se reusa `3004` para los casos prácticos. | Hacer la columna padre PK o UNIQUE. |
| `4067` | `COMPOSITE_INDEX_REQUIRES_ALL_INT` | K2: `CREATE INDEX idx ON t (a, b, ...)` con alguna columna no-INT. Mismo motivo que 4064. | Indexar solo columnas INT, o crear índices single-column individuales. |
| `4068` | `PARTIAL_KEY_LOOKUP_UNSUPPORTED` | K2 (reservado): `WHERE a = 1` contra PK compuesta `(a, b)` — el motor cae a full-scan correctamente, sin emitir error. Reservado para un futuro warning explícito. | (no usado hoy). |

---

## 5000–5999 · Server / HTTP

| Código | Símbolo | Causa | Remedio |
| :---: | :--- | :--- | :--- |
| `5001` | `MISSING_DB_PARAM` | Endpoint multi-DB invocado sin `?db=...`. | Pasar `?db=<nombre>.db` en la query string. |
| `5002` | `MISSING_TABLE_PARAM` | `GET /schema` o `GET /rows` sin `?table=...`. | Pasar `?table=<nombre>`. |
| `5003` | `INVALID_DB_NAME` | `?db=...` con `/`, `\` u otros caracteres prohibidos. | Solo se admiten nombres relativos dentro del directorio configurado con `-dir`. |
| `5004` | `UNAUTHORIZED` | Falta header `Authorization: Bearer <token>` o el token es incorrecto. | Pasar el token con el que arrancó el server. |
| `5005` | `SERVER_BUSY` | El cap de conexiones simultáneas está al máximo (default 64). | Esperar y reintentar, o subir `-max-connections N`. |
| `5006` | `SERVER_NOT_MULTI_DB` | Endpoint multi-DB (`?db=...`) sobre un server arrancado con `-db` (modo single-DB). | Arrancar el server con `-dir <carpeta>` para habilitar multi-DB. |

---

## 🛠️ Cómo usar los códigos desde un cliente

### CLI / shell

```bash
# Capturar el código exacto de un fallo
err=$(gabysql exec demo.db "DROP TABLE inexistente;" 2>&1)
echo "$err"
# → error: [GBY-2001] tabla no existe: inexistente

# Extraer solo el código
echo "$err" | grep -oE 'GBY-[0-9]{4}'
# → GBY-2001
```

### Cliente HTTP

```bash
curl -s http://localhost:8080/exec \
  -H 'Authorization: Bearer secret' \
  -d '{"sql":"DROP TABLE inexistente;"}' | jq -r .error
# → "[GBY-2001] tabla no existe: inexistente"
```

```python
import re

resp = requests.post("http://localhost:8080/exec", json={"sql": "..."}, headers={"Authorization": "Bearer ..."})
data = resp.json()
if not data["ok"]:
    match = re.match(r"\[GBY-(\d{4})\]", data["error"])
    code = int(match.group(1)) if match else None
    if code == 3001:  # DUPLICATE_PRIMARY_KEY
        # tomar acción específica
        ...
```

### Embedido en Rust

```rust
use gabysql::errors::codes;

if let Err(err) = pager.commit() {
    let text = err.to_string();
    if text.starts_with(&format!("[GBY-{:04}]", codes::NO_ACTIVE_TX)) {
        // recover from the no-tx case
    }
}
```

> **Nota**: hoy `DbError` solo expone el texto. Cuando exista demanda real para pattern-match programático, se introducirá `enum DbErrorKind` y un método `code()` directo. El contrato vía prefijo string es estable y suficiente para la mayoría de los casos.

---

## 📦 Por qué constantes en Rust, no JSON externo

> Pregunta razonable cuando uno ve este catálogo: *"¿no sería más flexible tener un `errors.json` cargado al arranque?"*. La respuesta es no, y vale escribirla acá para no repetirla:

1. **gabysql es zero-deps embebido** (ADR-0001). Un JSON externo agrega filesystem I/O al startup y una clase nueva de fallo ("no encuentro `errors.json`" — ¿con qué error reportás eso?).
2. **Los códigos son del motor, no de configuración**. Renombrar `TABLE_NOT_FOUND` rompe el código que lo usa; con constantes el compilador detecta el error, con JSON solo lo detecta una run de tests dedicada.
3. **No hay ganancia real**. Cambiar un mensaje hoy es editar `.rs`, rebuild, redeploy. Con `errors.json` sería editar `.json`, redeploy y rezar que el formato no rompió el parser. Mismo trabajo, más superficie de fallo.
4. **i18n no es el caso de uso hoy**. Si en el futuro hace falta, un build feature `i18n_es` / `i18n_en` con constantes diferentes resuelve el caso sin filesystem.

---

## 📋 Estabilidad

- **Códigos son contrato.** Una vez publicados en un release, **no cambian de significado y no se reusan** tras eliminación.
- **Nuevos códigos siempre se agregan al final** del rango correspondiente.
- **El texto humano puede evolucionar** (mejor redacción, más contexto). Si tu código depende del texto, eso es bug del cliente — usá el número.
- **Cambios al catálogo** se anuncian en el `CHANGELOG.md`.

---

## 🔗 Referencias

- [src/errors.rs](../src/errors.rs) — definiciones canónicas + helper `coded()`.
- [ERROR_HANDLING.md](ERROR_HANDLING.md) — filosofía y reglas de estilo de los mensajes.
- [TROUBLESHOOTING.md](../TROUBLESHOOTING.md) — operación: ¿qué hago cuando veo este código?
- [RUNBOOK.md](../RUNBOOK.md) — procedimientos formales (backup, recovery) referidos desde los códigos `1007`/`1008`.
- [ADR-0013](adr/0013-process-level-file-lock.md) — origen del `GBY-1002`.
- [ADR-0015](adr/0015-verified-backup-restore.md) — origen de los códigos `1007`/`1008` (verify/restore).
- [ADR-0017](adr/0017-int-ordered-index-version-7.md) — origen del `GBY-4002` (BETWEEN sobre índice INT-ordenado).
