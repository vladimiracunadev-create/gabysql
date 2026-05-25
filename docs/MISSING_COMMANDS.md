# 🚧 Comandos SQL que faltan en gabysql

> **Inventario exhaustivo de la superficie SQL no soportada hoy.** Sirve como roadmap concreto para cerrar el gap con un motor SQL relacional clásico. Cada feature lleva una **prioridad** (P0 = impacto crítico, P3 = nicho), un **bloque sugerido** (1 bloque = 1 push a `main`) y notas técnicas de implementación.
>
> Última verificación: 2026-05-25 contra `main` post-bloque **E2** (operadores `<`, `>`, `LIKE`, `IS NULL`, `IN literal`).
> Fuentes de verdad complementarias: [SQL_REFERENCE.md](SQL_REFERENCE.md) (lo que SÍ se soporta), [STATUS.md](STATUS.md) (madurez por subsistema), [TECHNICAL_SPECS.md](TECHNICAL_SPECS.md) (formato + subset exacto).

---

## 🎯 Top 5 huecos por impacto práctico

Si vas a ordenar bloques por ROI funcional, este es el orden:

| # | Hueco | Impacto | Bloque sugerido |
|---|---|---|---|
| 1 | ~~**`AND` / `OR` / `NOT` en `WHERE`**~~ ✅ | Cerrado en E1 (2026-05-25) | E1 ✅ |
| 2 | ~~**`<`, `>`, `<=`, `>=`, `<>`, `LIKE`, `IS NULL`**~~ ✅ | Cerrado en E2 (2026-05-25) | E2 ✅ |
| 3 | **`COUNT`, `SUM`, `AVG`, `MIN`, `MAX` + `GROUP BY`** | Sin agregaciones no hay reporting | F |
| 4 | **`UPDATE` / `DELETE` por columna indexada o subquery** | Hoy solo `WHERE pk = N` | E3 |
| 5 | **Transacciones explícitas (`BEGIN`/`COMMIT`/`ROLLBACK`)** | Necesario para apps que componen mutaciones atómicas | T |

Estos 5 bloques cierran >80% de las quejas previsibles de un usuario portando una app SQL clásica.

---

## 📦 Plan de bloques sugerido (1 = 1 push)

Cada bloque deja `main` verde con tests + docs + nuevos códigos de error. Pensado para ejecutar de arriba hacia abajo, pero son razonablemente independientes salvo donde marca **depende de**.

| Bloque | Contenido | Esfuerzo | Depende de |
|---|---|---|---|
| **E1** ✅ | `AND`/`OR`/`NOT` en `WHERE` + paréntesis | Alto (toca AST de WhereClause) | — (**cerrado 2026-05-25**) |
| **E2** ✅ | Operadores `<`, `>`, `<=`, `>=`, `<>`/`!=`, `LIKE`, `IS NULL`/`IS NOT NULL`, `IN (lista, literal)` | Medio | E1 (para combinar) (**cerrado 2026-05-25**) |
| **E3** | `UPDATE`/`DELETE` por columna indexada y por subquery | Medio | E1 |
| **F** | `GROUP BY` + `HAVING` + agregados (`COUNT`, `SUM`, `AVG`, `MIN`, `MAX`, `COUNT(*)`, `DISTINCT`) | Alto (nuevo executor stage) | E1 |
| **G** | Funciones escalares (string + numéricas + fecha) + `CAST` + `CASE WHEN` + `COALESCE`/`NULLIF` | Medio-Alto | E1 |
| **H** | Subqueries restantes: `NOT IN`, derived tables (`FROM (SELECT...) t`), subquery en SELECT list, `ANY`/`ALL` | Medio | F (para subqueries con agg) |
| **I** | Set ops: `UNION`/`UNION ALL`/`INTERSECT`/`EXCEPT`, `VALUES (...)` como tabla virtual | Medio | — |
| **J** | DML masivo: multi-row `INSERT`, `INSERT...SELECT`, `UPSERT`/`ON CONFLICT`, `RETURNING`, `TRUNCATE` | Medio | E3 |
| **K** | DDL faltante: PK compuesta, `CREATE TABLE AS SELECT`, `DROP/RENAME COLUMN`, `RENAME TABLE`, índices compuestos y partial indexes | Alto | — |
| **L** | Constraints: `CHECK`, `ON DELETE SET NULL/SET DEFAULT`, `ON UPDATE ...`, multi-column UNIQUE | Medio | K |
| **T** | Transacciones explícitas: `BEGIN`/`COMMIT`/`ROLLBACK`, `SAVEPOINT`, read-only | Alto (toca server + storage) | — |
| **V** | Vistas: `CREATE VIEW`/`DROP VIEW`, expansion en parser | Medio | F |
| **W** | Window functions + CTE: `WITH ... AS`, `WITH RECURSIVE`, `ROW_NUMBER`/`RANK`/`LAG`/`LEAD`, `SUM() OVER (PARTITION BY ...)` | Muy alto | F |
| **X** | Stored procedures + triggers: `CREATE FUNCTION`, `CREATE TRIGGER`, lenguaje procedural | Muy alto | T, F |
| **Y** | Tipos faltantes: `DECIMAL`/`NUMERIC`, `BLOB`/`BYTEA`, `UUID`, `ARRAY[]`, `INTERVAL`, `ENUM` | Alto (toca formato disco) | — |
| **Z** | Control de acceso: `CREATE USER`/`ROLE`, `GRANT`/`REVOKE`, RLS | Muy alto (auth en server) | T |

---

## 🔴 1. Predicados y operadores en WHERE (Bloque E1 + E2)

### 1.1 Boolean combinators

| Operador | Soportado | Prioridad |
|---|:---:|:---:|
| `=` | ✅ | — |
| `BETWEEN` (PK + INT indexada) | ✅ | — |
| `AND` | ✅ (E1, 2026-05-25) | — |
| `OR` | ✅ (E1, 2026-05-25) | — |
| `NOT` (fuera de `NOT EXISTS`) | ✅ (E1, 2026-05-25) | — |
| Paréntesis para agrupar | ✅ (E1, 2026-05-25) | — |

> ✅ **Cerrado en E1**: el AST cambió a `WhereExpr = And | Or | Not | Atom(WhereClause)` con precedencia estándar (`OR` < `AND` < `NOT` < átomo) y lógica trivaluada para NULL. Limitación residual: `EXISTS` correlacionado y `col = otra.col` solo se aceptan como único átomo (combinarlos con AND/OR/NOT devuelve `[GBY-4024]`). Detalles en [SQL_REFERENCE.md](SQL_REFERENCE.md#-ebnf).

### 1.2 Comparadores

| Operador | Soportado | Prioridad |
|---|:---:|:---:|
| `=` | ✅ | — |
| `<`, `>`, `<=`, `>=` | ✅ (E2) | — |
| `<>` / `!=` | ✅ (E2) | — |
| `LIKE` / `NOT LIKE` | ✅ (E2; `%`/`_` + escape `\`) | — |
| `ILIKE` (case-insensitive) | ❌ | P2 |
| `IS NULL` / `IS NOT NULL` | ✅ (E2) | — |
| `IS TRUE` / `IS FALSE` | ❌ | P3 |
| `IN (lista_literales)` — `id IN (1,2,3)` | ✅ (E2) | — |
| `NOT IN (lista_literales)` | ✅ (E2; 3VL ANSI con NULLs en la lista) | — |
| `NOT IN (SELECT ...)` | ❌ (esperar bloque H) | P1 |
| `REGEXP` / `~` (regex) | ❌ | P3 |
| `GLOB` (estilo SQLite) | ❌ | P3 |

---

## 🔴 2. Agregaciones — el agujero más grande (Bloque F)

| Comando | Soportado | Prioridad |
|---|:---:|:---:|
| `GROUP BY <col>` (single) | ❌ | P0 |
| `GROUP BY <col1>, <col2>` (multi) | ❌ | P0 |
| `HAVING <cond>` | ❌ | P0 |
| `COUNT(*)` | ❌ | P0 |
| `COUNT(col)` | ❌ | P0 |
| `SUM(col)` | ❌ | P0 |
| `AVG(col)` | ❌ | P0 |
| `MIN(col)` / `MAX(col)` | ❌ | P0 |
| `DISTINCT` — `SELECT DISTINCT col` | ❌ | P0 |
| `COUNT(DISTINCT col)` | ❌ | P1 |
| `GROUP_CONCAT` / `STRING_AGG` | ❌ | P2 |
| `JSON_AGG` / `ARRAY_AGG` | ❌ | P3 |

> **Plan de implementación (Bloque F)**: introducir `Aggregate` enum en el AST, nuevo executor stage post-WHERE/pre-ORDER que agrupa por las columnas del GROUP BY y calcula los agregados por bucket. Si no hay GROUP BY pero hay agregados, devuelve UNA sola fila (agregado global).

---

## 🔴 3. Funciones escalares (Bloque G)

Hoy sin ninguna función built-in. Lista mínima útil:

### String
| Función | Prioridad |
|---|:---:|
| `LENGTH(s)` | P1 |
| `UPPER(s)` / `LOWER(s)` | P1 |
| `SUBSTR(s, from, len)` / `SUBSTRING` | P1 |
| `TRIM(s)` / `LTRIM` / `RTRIM` | P2 |
| `CONCAT(a, b, ...)` y operador `||` | P1 |
| `REPLACE(s, from, to)` | P2 |
| `SPLIT_PART(s, sep, idx)` | P3 |

### Numéricas
| Función | Prioridad |
|---|:---:|
| `ABS(x)` | P1 |
| `ROUND(x, n)` | P1 |
| `CEIL(x)` / `FLOOR(x)` | P2 |
| `MOD(a, b)` u operador `%` | P2 |
| `POWER(x, y)` / `SQRT(x)` | P3 |

### Fecha / hora
| Función | Prioridad |
|---|:---:|
| `NOW()` / `CURRENT_TIMESTAMP` | P1 |
| `CURRENT_DATE` | P1 |
| `DATE_ADD(d, n_days)` / `DATE_SUB` | P2 |
| `DATEDIFF(d1, d2)` | P2 |
| `EXTRACT(YEAR FROM d)` | P2 |
| `STRFTIME(format, d)` | P3 |

### Conversión y condicional
| Construcción | Prioridad |
|---|:---:|
| `CAST(x AS TYPE)` | P0 |
| `COALESCE(a, b, ...)` | P0 |
| `NULLIF(a, b)` | P1 |
| `IFNULL(a, b)` / `IF(cond, a, b)` | P1 |
| `CASE WHEN ... THEN ... ELSE ... END` | P0 |
| `CASE col WHEN x THEN ... END` (simple form) | P1 |

---

## 🟡 4. Subqueries — variantes faltantes (Bloque H)

| Forma | Soportado | Prioridad |
|---|:---:|:---:|
| `WHERE col IN (SELECT ...)` no-correl | ✅ | — |
| `WHERE col = (SELECT ...)` escalar | ✅ | — |
| `WHERE [NOT] EXISTS (...)` correl single-eq | ✅ | — |
| `WHERE col NOT IN (SELECT ...)` | ❌ | P1 |
| Subquery con `AND`/`OR` multi-predicado correlated | ❌ | P1 (depende de E1) |
| `FROM (SELECT ...) AS t` (derived tables) | ❌ | P0 |
| `SELECT (SELECT MAX(x) FROM t) FROM s` (subquery en SELECT) | ❌ | P1 (depende de F) |
| `WHERE col > ALL (SELECT ...)` / `ANY` / `SOME` | ❌ | P2 |
| Correlated en `=` o `IN` (no solo `EXISTS`) | ❌ | P2 |
| `LATERAL (SELECT ...)` | ❌ | P3 |

---

## 🟡 5. JOINs — variantes faltantes

Hoy soportadas: INNER, CROSS, LEFT/RIGHT/FULL [OUTER], USING (1 col), NATURAL (1 col común), multi-tabla chain, self-join, index-loop optimization. **Faltan:**

| Feature | Prioridad |
|---|:---:|
| `ON` con `AND`/`OR` multi-predicado | P1 (depende de E1) |
| `ON` con operadores no-equi (`<`, `>`, `BETWEEN`) | P1 (depende de E2) |
| `USING (col1, col2, ...)` multi-columna | P2 |
| `NATURAL JOIN` con >1 columna común | P2 |
| `LATERAL JOIN` | P3 |
| Hash join / merge join (estrategias alternativas) | P3 (hoy nested + index-loop) |
| Reorderable join planner cost-based | P3 |

---

## 🔴 6. Set operations (Bloque I)

| Operación | Soportado | Prioridad |
|---|:---:|:---:|
| `UNION` | ❌ | P1 |
| `UNION ALL` | ❌ | P1 |
| `INTERSECT` | ❌ | P2 |
| `EXCEPT` / `MINUS` | ❌ | P2 |
| `VALUES (a,b), (c,d)` como tabla virtual | ❌ | P2 |

---

## 🔴 7. DML masivo y avanzado (Bloque E3 + J)

| Comando | Soportado | Prioridad | Bloque |
|---|:---:|:---:|:---:|
| `INSERT ... VALUES (...)` (single row) | ✅ | — | — |
| `INSERT ... VALUES (a,b),(c,d)` multi-row | ❌ | P0 | J |
| `INSERT INTO t SELECT ...` | ❌ | P0 | J |
| `INSERT ... ON CONFLICT DO UPDATE` / `UPSERT` | ❌ | P1 | J |
| `REPLACE INTO` (SQLite-style) | ❌ | P2 | J |
| `UPDATE ... WHERE col_indexada = val` | ❌ | P0 | E3 |
| `UPDATE ... WHERE col IN (SELECT ...)` | ❌ | P1 | E3 |
| `UPDATE ... FROM otra_tabla` (UPDATE con JOIN) | ❌ | P2 | J |
| `DELETE ... WHERE col_indexada = val` | ❌ | P0 | E3 |
| `DELETE` con JOIN o subquery en WHERE | ❌ | P1 | E3 |
| `TRUNCATE TABLE` | ❌ | P2 | J |
| `RETURNING` clause (`UPDATE ... RETURNING id`) | ❌ | P2 | J |

---

## 🔴 8. Transacciones explícitas (Bloque T)

| Comando | Soportado | Prioridad |
|---|:---:|:---:|
| Auto-commit por `exec` | ✅ | — |
| `BEGIN` / `START TRANSACTION` | ❌ | P0 |
| `COMMIT` / `END` explícito | ❌ | P0 |
| `ROLLBACK` | ❌ | P0 |
| `SAVEPOINT name` / `ROLLBACK TO SAVEPOINT` | ❌ | P1 |
| `SET TRANSACTION ISOLATION LEVEL ...` | ❌ | P2 |
| Read-only transactions (`BEGIN READ ONLY`) | ❌ | P2 |

> **Impacto operacional**: hoy cada `/exec` es una transacción atómica. No podés agrupar varias requests del cliente en una sola transacción ni hacer commit/rollback condicional desde el cliente.

---

## 🔴 9. DDL faltante (Bloque K + L)

### Tabla y schema
| Comando | Soportado | Prioridad |
|---|:---:|:---:|
| `CREATE TABLE` con `PRIMARY KEY(a,b)` compuesta | ❌ | P1 |
| `CREATE TABLE ... AS SELECT ...` (CTAS) | ❌ | P1 |
| `CREATE TEMPORARY TABLE` | ❌ | P2 |
| `ALTER TABLE ADD COLUMN` | ✅ | — |
| `ALTER TABLE DROP COLUMN` | ❌ | P1 |
| `ALTER TABLE RENAME COLUMN` | ❌ | P1 |
| `ALTER TABLE RENAME TO` | ❌ | P1 |
| `ALTER TABLE ADD CONSTRAINT` | ❌ | P2 |
| `ALTER TABLE ALTER COLUMN ... TYPE ...` | ❌ | P2 |
| `DROP TABLE ... CASCADE` | ❌ | P2 |

### Índices
| Comando | Soportado | Prioridad |
|---|:---:|:---:|
| `CREATE INDEX ... ON t (col)` single | ✅ | — |
| `CREATE UNIQUE INDEX` | ✅ | — |
| `CREATE INDEX ... ON t (a, b, ...)` compuesto | ❌ | P0 |
| `CREATE INDEX ... ON t (col) WHERE cond` (partial) | ❌ | P2 |
| `CREATE INDEX ... INCLUDE (...)` (covering) | ❌ | P3 |
| `REINDEX` | ❌ | P3 |

### Constraints
| Constraint | Soportado | Prioridad |
|---|:---:|:---:|
| `PRIMARY KEY` (single INT) | ✅ | — |
| `NOT NULL` | ✅ | — |
| `UNIQUE` (inline + standalone) | ✅ | — |
| `DEFAULT <literal>` | ✅ | — |
| `FOREIGN KEY ... ON DELETE RESTRICT|CASCADE` | ✅ | — |
| `FOREIGN KEY ... ON DELETE SET NULL` | ❌ | P1 |
| `FOREIGN KEY ... ON DELETE SET DEFAULT` | ❌ | P2 |
| `FOREIGN KEY ... ON UPDATE ...` | ❌ | P2 |
| `CHECK (cond)` | ❌ | P1 |
| `EXCLUDE USING ...` (Postgres-style) | ❌ | P3 |
| Constraints diferidas (`DEFERRABLE INITIALLY DEFERRED`) | ❌ | P3 |
| Multi-column `PRIMARY KEY` o `UNIQUE` standalone | ❌ | P1 |

---

## 🔴 10. Vistas, sequences, triggers (Bloques V + X)

| Objeto | Soportado | Prioridad |
|---|:---:|:---:|
| `CREATE VIEW` / `DROP VIEW` | ❌ | P1 |
| `CREATE MATERIALIZED VIEW` | ❌ | P3 |
| `CREATE SEQUENCE` / `nextval` | ❌ | P1 |
| `AUTO_INCREMENT` / `SERIAL` / `IDENTITY` (PK auto) | ❌ | P0 |
| `CREATE SCHEMA` / namespace | ❌ | P3 |
| `CREATE TRIGGER` | ❌ | P2 |
| `CREATE FUNCTION` / stored procedures | ❌ | P3 |
| `CREATE TYPE` (enums, composites) | ❌ | P3 |

---

## 🔴 11. CTE y window functions (Bloque W)

| Feature | Soportado | Prioridad |
|---|:---:|:---:|
| `WITH cte AS (SELECT ...) SELECT ...` | ❌ | P1 |
| `WITH RECURSIVE` | ❌ | P3 |
| `ROW_NUMBER() OVER (...)` | ❌ | P2 |
| `RANK`, `DENSE_RANK`, `NTILE` | ❌ | P2 |
| `LAG`, `LEAD`, `FIRST_VALUE`, `LAST_VALUE` | ❌ | P2 |
| `SUM(x) OVER (PARTITION BY ...)` | ❌ | P2 |
| `OVER (PARTITION BY ... ORDER BY ...)` window spec completa | ❌ | P3 |
| `ROWS BETWEEN ... AND ...` frame | ❌ | P3 |

---

## 🟡 12. Tipos faltantes (Bloque Y)

| Tipo | Soportado | Prioridad |
|---|:---:|:---:|
| `INT`, `TEXT`, `BOOL`, `FLOAT`, `DATE`, `DATETIME`, `JSON` | ✅ | — |
| `DECIMAL(p, s)` / `NUMERIC` | ❌ | P1 |
| `BIGINT` / `SMALLINT` / `TINYINT` separados | ❌ | P3 (hoy todo es i64) |
| `BLOB` / `BYTEA` (binario) | ❌ | P1 |
| `UUID` | ❌ | P2 (workaround: TEXT) |
| `ARRAY[]` | ❌ | P3 |
| `INTERVAL` | ❌ | P3 |
| `ENUM` | ❌ | P2 |
| `GEOMETRY` / `GEOGRAPHY` (PostGIS-like) | ❌ | P3 |
| `INET` / `CIDR` (red) | ❌ | P3 |

---

## 🔴 13. Control de acceso (Bloque Z)

Hoy: solo token compartido en el server HTTP. Nada de SQL-level.

| Comando | Soportado | Prioridad |
|---|:---:|:---:|
| `CREATE USER` / `CREATE ROLE` | ❌ | P2 |
| `GRANT` / `REVOKE` | ❌ | P2 |
| `SET ROLE` | ❌ | P3 |
| Row-level security (RLS) | ❌ | P3 |
| `ALTER USER ... SET PASSWORD ...` | ❌ | P2 |

---

## 🟡 14. Otros del estándar

| Feature | Soportado | Prioridad |
|---|:---:|:---:|
| `EXPLAIN` / `EXPLAIN ANALYZE` | ❌ | P1 |
| `PREPARE` / `EXECUTE` (prepared statements) | ❌ | P2 |
| Parámetros bind (`?`, `$1`) en API | ❌ | P1 |
| `COPY FROM` / `COPY TO` (bulk load) | ❌ | P2 |
| `IMPORT` / `EXPORT` declarativos | ❌ | P3 (hoy: `gabysql backup`) |
| Comentarios en SQL (`-- ...`, `/* ... */`) | ❌ | P0 |
| `PRAGMA` (SQLite-style runtime config) | ❌ | P3 |
| `SET search_path` (Postgres-style) | ❌ | P3 |
| Multi-statement con stop-on-error vs continue | ❌ | P2 |

---

## 🧮 Resumen por prioridad

| Prioridad | Cantidad | Significado |
|:---:|:---:|---|
| **P0** | ~25 | Bloquea uso real; cualquier app SQL clásica los necesita |
| **P1** | ~30 | Comunes en cualquier base, esperables |
| **P2** | ~25 | Nice-to-have, no críticos |
| **P3** | ~20 | Nicho / avanzado / opcional |

---

## 🛣️ Ruta recomendada para "línea de comandos completa"

Si el objetivo es **portar una app SQL clásica sin sorpresas**, esta es la secuencia mínima:

```
E1 (AND/OR/NOT en WHERE)
  ↓
E2 (operadores <, >, LIKE, IS NULL, IN literal)
  ↓
E3 (UPDATE/DELETE por col indexada y subquery)
  ↓
F (GROUP BY + HAVING + agregados + DISTINCT)
  ↓
G (funciones escalares + CAST + CASE + COALESCE)
  ↓
T (BEGIN / COMMIT / ROLLBACK explícitos)
  ↓
J (multi-row INSERT, INSERT...SELECT, RETURNING, UPSERT)
  ↓
H (derived tables, NOT IN, subquery en SELECT)
  ↓
I (UNION, INTERSECT, EXCEPT, VALUES)
  ↓
K (PK compuesta, ALTER DROP/RENAME, índices compuestos)
  ↓
L (CHECK, ON DELETE SET NULL, multi-col UNIQUE)
```

Con E1+E2+E3+F+G+T+J cerrados, gabysql cubre la **superficie SQL operacional clásica** completa. Lo demás (vistas, CTE, window, triggers, tipos exóticos, RLS) es ya material para casos especializados.

---

## 📚 Referencias cruzadas

- [SQL_REFERENCE.md](SQL_REFERENCE.md) — gramática EBNF + railroad de TODO lo que SÍ se soporta.
- [STATUS.md](STATUS.md) — madurez por subsistema con la línea de demarcación 🟢/🟡/🔴.
- [ERROR_CODES.md](ERROR_CODES.md) — catálogo `[GBY-NNNN]` (nuevos códigos se agregan acá al cerrar cada bloque).
- [TECHNICAL_SPECS.md](TECHNICAL_SPECS.md) — formato en disco + subset SQL exacto.
- [ROADMAP.md](../ROADMAP.md) — dirección técnica general (no entra en este detalle de comandos).
- [AGENDA_INVESTIGACION.md](AGENDA_INVESTIGACION.md) — qué entra y qué no en términos de aprendizaje (algunos bloques de acá pueden ser "anti-agenda" según el momento).
