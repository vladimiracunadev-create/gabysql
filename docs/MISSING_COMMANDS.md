# 🚧 Comandos SQL que faltan en gabysql

> **Inventario exhaustivo de la superficie SQL no soportada hoy.** Sirve como roadmap concreto para cerrar el gap con un motor SQL relacional clásico. Cada feature lleva una **prioridad** (P0 = impacto crítico, P3 = nicho), un **bloque sugerido** (1 bloque = 1 push a `main`) y notas técnicas de implementación.
>
> Última verificación: 2026-05-29 contra `main` post-Fase 3 P3. Stack acumulado de bloques cerrados desde la última auditoría del documento (~K2): G/H/I (subqueries+set-ops+derived), J (multi-row INSERT+UPSERT+RETURNING+TRUNCATE), K1/K2 (CTAS+composite PK), V (VIEW), W1/W2/W3 (CTEs+RECURSIVE+window functions), X1→X4f+X6 (triggers+procedures+functions+IF/CASE/WHILE/FOR/LOOP/DECLARE/RAISE/EXCEPTION/RETURN), Y1→Y9 (tipos extendidos+DECIMAL exacto+BLOB+UUID+UNSIGNED), Z1→Z3f (USERS+ROLES+PBKDF2/scrypt/Blake2b/Argon2id+GRANT/REVOKE+RLS), P1/P2/P3 (EXPLAIN+ANALYZE).
>
> **Cobertura SQL hoy**: >85% de los comandos de un motor SQL clásico están implementados. Los huecos remanentes son específicos (savepoints, prepared statements, bind params, ARRAY/JSONB, cursores, planner real, COPY). Ver tabla "Resumen por prioridad" abajo y la sección "Próximas proyecciones" en [ROADMAP.md](../ROADMAP.md).
>
> 📊 **Gaps específicos identificados por el benchmark 2026-05-30**: catálogo cerrado de 10 gaps en [ADR-0066](adr/0066-bench-exposed-gaps.md) — cada uno con código de error, query del bench que lo dispara, workaround y bloque/prioridad de fix definitivo. El más crítico: `RANK()` y `SUM OVER` cuadráticos (W4).
> Fuentes de verdad complementarias: [SQL_REFERENCE.md](SQL_REFERENCE.md) (lo que SÍ se soporta), [STATUS.md](STATUS.md) (madurez por subsistema), [TECHNICAL_SPECS.md](TECHNICAL_SPECS.md) (formato + subset exacto).

---

## 🎯 Top 5 huecos por impacto práctico (refrescado 2026-05-29)

> **Histórico de huecos cerrados** (movido al pie del archivo): E1, E2, E3, F, T, J, J2, G, H, I, K1, K2, L, V, W, X, Y, Z — todos los "P0 clásicos" del 2026-05-25..27 ya están entregados. La superficie SQL relacional clásica está cubierta. Los huecos remanentes son **de rendimiento, herramientas y completitud avanzada**, no de uso básico.

Si vas a ordenar bloques por ROI funcional **hoy**, este es el orden:

| # | Hueco | Impacto real | Bloque sugerido |
|---|---|---|---|
| 1 | **Planner-as-optimizer cost-based** | Queries analíticas (full scan, agregados sobre 200k rows) cuestan 0.5–4 s en el bench 2026-05-29. Sin reorden de joins ni elección de índice por costo, gabysql es "ejecutor con plan fijo". | **P5** (depende de P4) |
| 2 | **Stats por-columna** (NDV vía HyperLogLog, MCV top-K, histogramas) | P3 ya da `est.rows` global; sin per-column no se puede estimar selectividad de un predicado. Sin esto, P5 no tiene insumos. | **P4** |
| 3 | **`SAVEPOINT` + `ROLLBACK TO SAVEPOINT`** | Hoy `ROLLBACK` descarta TODO el batch. Imposible deshacer una violation de constraint sin perder el resto. | **T1** |
| 4 | **Bind params (`?`, `$1`) + `PREPARE`/`EXECUTE` + plan cache** | Sin esto, hay que concatenar SQL → riesgo de inyección si el caller no escapa. Plan cache habilita reuso del plan parseado. | **N1+N2** |
| 5 | **Agregados sobre `SELECT` con `JOIN`** (`[GBY-4028]`) | El bench 2026-05-29 lo tropezó: queries comunes tipo `SELECT u.name, COUNT(*) FROM users u JOIN posts p ON p.user_id = u.id GROUP BY u.name` rebotan. Hoy hay que rescribirlas como subquery. | **F2** |

Estos 5 bloques son los que más mueven la aguja a partir de hoy. Los 3 primeros (P4+P5+T1) son los que el usuario sentido como "esta DB rinde" o "esta DB no rinde".

---

## 📦 Plan de bloques sugerido (1 = 1 push)

Cada bloque deja `main` verde con tests + docs + nuevos códigos de error. Pensado para ejecutar de arriba hacia abajo, pero son razonablemente independientes salvo donde marca **depende de**.

| Bloque | Contenido | Esfuerzo | Depende de |
|---|---|---|---|
| **E1** ✅ | `AND`/`OR`/`NOT` en `WHERE` + paréntesis | Alto (toca AST de WhereClause) | — (**cerrado 2026-05-25**) |
| **E2** ✅ | Operadores `<`, `>`, `<=`, `>=`, `<>`/`!=`, `LIKE`, `IS NULL`/`IS NOT NULL`, `IN (lista, literal)` | Medio | E1 (para combinar) (**cerrado 2026-05-25**) |
| **E3** ✅ | `UPDATE`/`DELETE` por columna indexada y por subquery | Medio | E1 (**cerrado 2026-05-25**) |
| **F** ✅ | `GROUP BY` + `HAVING` + agregados (`COUNT`, `SUM`, `AVG`, `MIN`, `MAX`, `COUNT(*)`, `DISTINCT`) | Alto (nuevo executor stage) | E1 (**cerrado 2026-05-25**; limitación: sin JOINs aún) |
| **G1+G2+G3** ✅ | Funciones escalares (string + numéricas + fecha, incluyendo P2/P3) + `CAST` + `CASE WHEN` + `COALESCE`/`NULLIF` + operadores aritméticos `+/-/*///%` + concat `\|\|` + postfix `IS NULL`/`LIKE`/`IN`/`BETWEEN` sobre cualquier `Expr`, en SELECT list / WHERE / HAVING / UPDATE SET / DELETE WHERE | Medio-Alto | E1 (**cerrado 2026-05-26**; pendiente sólo `EXCLUDED.col` en UPSERT — sub-pendiente J2-P2) |
| **H** | Subqueries restantes: `NOT IN`, derived tables (`FROM (SELECT...) t`), subquery en SELECT list, `ANY`/`ALL` | Medio | F (para subqueries con agg) |
| **I** ✅ | Set ops: `UNION`/`UNION ALL`/`INTERSECT`/`INTERSECT ALL`/`EXCEPT`/`EXCEPT ALL`/`MINUS`, `VALUES (...)` como query standalone y como tabla virtual en FROM/JOIN (**cerrado 2026-05-26**) | Medio | — |
| **J** ✅ | DML masivo: multi-row `INSERT`, `INSERT...SELECT`, `TRUNCATE`, `UPSERT`/`ON CONFLICT`, `REPLACE INTO`, `RETURNING` | Medio | E3 (**cerrado 2026-05-25**; `EXCLUDED.col` y `UPDATE ... FROM` pendientes) |
| **K1** ✅ | DDL safe sin cambios on-disk: `CREATE TABLE AS SELECT`, `RENAME TABLE`, `ALTER TABLE RENAME TO`, `ALTER TABLE DROP COLUMN [IF EXISTS]`, `ALTER TABLE RENAME COLUMN` (**cerrado 2026-05-26**) | Medio | — |
| **K2** ✅ | DDL con cambios on-disk: PK compuesta `PRIMARY KEY (a, b, ...)` y índices compuestos `CREATE [UNIQUE] INDEX idx ON t (a, b, ...)`. Bump VERSION 7→8 (**cerrado 2026-05-26**, ver ADR-0019). PK compuesta sigue all-INT NOT NULL; UNIQUE/CREATE INDEX compuestos aceptan TEXT/UUID/DATE/etc desde K3 (2026-05-30). | Alto | K1 |
| **K3** ✅ | UNIQUE multi-col + CREATE INDEX composite aceptan INT/FLOAT/BOOL/TEXT/DATE/DATETIME/TIME/UUID (rechazan JSON/BLOB/DECIMAL). **Cerrado 2026-05-30** vía relajación del validador upfront (ver ADR-0066 Gap 4). | Alto | K2 |
| **K4** ✅ | Partial lookup sobre PK compuesta (`WHERE pk1 = X`) usa auto-index `_pk_prefix_<table>` OrderedInt — equivalente al left-most column match de MySQL InnoDB. **Cerrado 2026-05-30** (ver ADR-0066 Gap 9). | Alto | K2 |
| **L1** ✅ | Referential actions: `ON DELETE SET NULL`, `ON DELETE SET DEFAULT`, `ON DELETE NO ACTION`, `ON UPDATE ...` (parsea + persiste; no se dispara hoy), `UNIQUE (a, b, ...)` table-level + parche al composite UNIQUE de K2. Bump VERSION 8→9. **Cerrado 2026-05-27** ([ADR-0020](adr/0020-fk-referential-actions.md)). | Medio | K |
| **L2** ✅ | Constraints: `CHECK (expr)` column-level y table-level (con/sin `CONSTRAINT name`), evaluación en INSERT/UPDATE/UPSERT/DO UPDATE/cascade, 3VL ANSI, subqueries rechazadas en DDL. Bump VERSION 9→10. **Cerrado 2026-05-27** ([ADR-0021](adr/0021-check-constraints.md)). | Medio-Alto | L1, G3 |
| **L3** ✅ | `ALTER TABLE <t> ADD [CONSTRAINT <name>] CHECK (<expr>)` con re-validación O(n) de las filas existentes antes de persistir. Sin estado parcial. Sin bump de formato. **Cerrado 2026-05-27.** | Bajo | L2 |
| **T** ✅ | Transacciones explícitas: `BEGIN`/`COMMIT`/`ROLLBACK` (cerrado 2026-05-25; `SAVEPOINT`, read-only y cross-request quedan pendientes) | Alto | — |
| **V** ✅ | Vistas lógicas: `CREATE VIEW [IF NOT EXISTS] v [(col_aliases)] AS &lt;select_query&gt;`, `DROP VIEW [IF EXISTS] v`. Expansion como derived table en cualquier FROM. Read-only (`[GBY-4075]`). Bump VERSION 12→13 con discriminator byte tabla/vista. **Cerrado 2026-05-27** ([ADR-0025](adr/0025-views.md)). | Medio | F |
| **W** | Window functions + CTE: `WITH ... AS`, `WITH RECURSIVE`, `ROW_NUMBER`/`RANK`/`LAG`/`LEAD`, `SUM() OVER (PARTITION BY ...)` | Muy alto | F |
| **X** | Stored procedures + triggers: `CREATE FUNCTION`, `CREATE TRIGGER`, lenguaje procedural | Muy alto | T, F |
| **Y** ✅ | Tipos extendidos: aliases (BIGINT, VARCHAR(n), DECIMAL(p,s), DOUBLE PRECISION, BOOLEAN, TIMESTAMP, REAL, …) + nuevos `TIME` y `UUID` con código en disco. `DECIMAL` exacto, `BLOB`/`BYTEA`, `ARRAY[]`, `INTERVAL`, `ENUM` diferidos a Y2 | Medio (bump 16→17) | Y2 |
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
| `NOT IN (SELECT ...)` | ✅ (H, 2026-05-26; 3VL ANSI estricta — NULL en subquery propaga NULL) | — |
| `REGEXP` / `~` (regex) | ❌ | P3 |
| `GLOB` (estilo SQLite) | ❌ | P3 |

---

## 🔴 2. Agregaciones — el agujero más grande (Bloque F)

| Comando | Soportado | Prioridad |
|---|:---:|:---:|
| `GROUP BY <col>` (single) | ✅ (F) | — |
| `GROUP BY <col1>, <col2>` (multi) | ✅ (F) | — |
| `HAVING <cond>` | ✅ (F; acepta agregados directos y por alias) | — |
| `COUNT(*)` | ✅ (F) | — |
| `COUNT(col)` | ✅ (F; ignora NULLs) | — |
| `SUM(col)` | ✅ (F; INT/FLOAT con promoción) | — |
| `AVG(col)` | ✅ (F; FLOAT) | — |
| `MIN(col)` / `MAX(col)` | ✅ (F) | — |
| `SUM/AVG/MIN/MAX/COUNT(expr)` con `Expr` arbitrario — e.g. `SUM(qty * price)`, `AVG(LENGTH(name))` | ✅ (Issue #5, 2026-05-27; via `AggArg::Expr`) | — |
| `DISTINCT` — `SELECT DISTINCT col` | ✅ (F) | — |
| `COUNT(DISTINCT col)` | ✅ (F) | — |
| **Agregados sobre `SELECT` con `JOIN`** | ❌ ([GBY-4028]) | P1 |
| `GROUP_CONCAT` / `STRING_AGG` | ❌ | P2 |
| `JSON_AGG` / `ARRAY_AGG` | ❌ | P3 |

> **Plan de implementación (Bloque F)**: introducir `Aggregate` enum en el AST, nuevo executor stage post-WHERE/pre-ORDER que agrupa por las columnas del GROUP BY y calcula los agregados por bucket. Si no hay GROUP BY pero hay agregados, devuelve UNA sola fila (agregado global).

---

## 🔴 3. Funciones escalares (Bloque G)

Hoy sin ninguna función built-in. Lista mínima útil:

> ✅ **Cerrado en G1 (2026-05-26)** para SELECT list: parser + evaluator + 12 integration tests.
>
> ✅ **Cerrado en G2 (2026-05-26)** la extensión a `WHERE` / `HAVING` / `UPDATE SET` / `DELETE WHERE` / `ON CONFLICT DO UPDATE SET`: nuevo átomo `WhereClause::ExprPredicate` (FullScan + post-filter, sin perder los fast-paths estructurales) + assignments con RHS `Expr` evaluada contra la fila pre-update + helper `eval_expr_as_predicate` con 3VL. 20 integration tests adicionales. **Limitaciones residuales para G3**: operadores postfix sobre Expr (`IS NULL`/`LIKE`/`IN`/`BETWEEN` con LHS expresional → `[GBY-4039]`), operador `||`, aritméticos binarios (`+`/`-`/`*`/`/`), funciones P2/P3 (TRIM/REPLACE/CEIL/FLOOR/MOD/DATE_ADD/SUB/EXTRACT/...), y `EXCLUDED.col` en UPSERT.

### String
| Función | Prioridad |
|---|:---:|
| `LENGTH(s)` | ✅ (G1, 2026-05-26) |
| `UPPER(s)` / `LOWER(s)` | ✅ (G1, 2026-05-26) |
| `SUBSTR(s, from, len)` / `SUBSTRING` | ✅ (G1, 2026-05-26) |
| `TRIM(s)` / `LTRIM` / `RTRIM` | ✅ (G3, 2026-05-26) |
| `CONCAT(a, b, ...)` | ✅ (G1, 2026-05-26) — operador `\|\|` ✅ (G3, 2026-05-26) |
| `REPLACE(s, from, to)` | ✅ (G3, 2026-05-26) |
| `SPLIT_PART(s, sep, idx)` | ✅ (G3, 2026-05-26) |

### Numéricas
| Función | Prioridad |
|---|:---:|
| `ABS(x)` | ✅ (G1, 2026-05-26) |
| `ROUND(x, n)` | ✅ (G1, 2026-05-26) |
| `CEIL(x)` / `FLOOR(x)` | ✅ (G3, 2026-05-26) |
| `MOD(a, b)` u operador `%` | ✅ (G3, 2026-05-26) |
| `POWER(x, y)` / `SQRT(x)` | ✅ (G3, 2026-05-26) |
| Operadores aritméticos `+`/`-`/`*`/`/` | ✅ (G3, 2026-05-26) |

### Fecha / hora
| Función | Prioridad |
|---|:---:|
| `NOW()` / `CURRENT_TIMESTAMP` | ✅ (G1, 2026-05-26) |
| `CURRENT_DATE` | ✅ (G1, 2026-05-26) |
| `DATE_ADD(d, n_days)` / `DATE_SUB` | ✅ (G3, 2026-05-26) |
| `DATEDIFF(d1, d2)` | ✅ (G3, 2026-05-26) |
| `EXTRACT(YEAR FROM d)` | ✅ (G3, 2026-05-26) |
| `STRFTIME(format, d)` | ✅ (G3, 2026-05-26) |

### Conversión y condicional
| Construcción | Prioridad |
|---|:---:|
| `CAST(x AS TYPE)` | ✅ (G1, 2026-05-26) |
| `COALESCE(a, b, ...)` | ✅ (G1, 2026-05-26) |
| `NULLIF(a, b)` | ✅ (G1, 2026-05-26) |
| `IFNULL(a, b)` / `IF(cond, a, b)` | ✅ (G1, 2026-05-26; alias `IIF` aceptado) |
| `CASE WHEN ... THEN ... ELSE ... END` | ✅ (G1, 2026-05-26) |
| `CASE col WHEN x THEN ... END` (simple form) | ✅ (G1, 2026-05-26) |

---

## 🟡 4. Subqueries — variantes faltantes (Bloque H)

| Forma | Soportado | Prioridad |
|---|:---:|:---:|
| `WHERE col IN (SELECT ...)` no-correl | ✅ | — |
| `WHERE col = (SELECT ...)` escalar | ✅ | — |
| `WHERE [NOT] EXISTS (...)` correl single-eq | ✅ | — |
| `WHERE col NOT IN (SELECT ...)` | ✅ (H, 2026-05-26; 3VL ANSI estricta) | — |
| Subquery con `AND`/`OR` multi-predicado correlated | ✅ (H, 2026-05-26; `EXISTS`/`EqColumnRef` en combinadores) | — |
| `FROM (SELECT ...) AS t` (derived tables) | ✅ (H, 2026-05-26; alias obligatorio, derived en JOINs OK) | — |
| `SELECT (SELECT MAX(x) FROM t) FROM s` (subquery en SELECT) | ✅ (H, 2026-05-26; correlated OK) | — |
| `WHERE col > ALL (SELECT ...)` / `ANY` / `SOME` | ❌ | P2 |
| Correlated `col = outer.col` puro fuera de `EXISTS` con JOIN | ❌ | P2 |
| `LATERAL (SELECT ...)` | ❌ | P3 |
| Derived dentro de UPDATE/DELETE/INSERT | ❌ | P3 |

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

## 🟢 6. Set operations (Bloque I) — ✅ cerrado 2026-05-26

| Operación | Soportado | Prioridad |
|---|:---:|:---:|
| `UNION` | ✅ (I, 2026-05-26) | P1 |
| `UNION ALL` | ✅ (I, 2026-05-26) | P1 |
| `INTERSECT` | ✅ (I, 2026-05-26) | P2 |
| `INTERSECT ALL` | ✅ (I, 2026-05-26) | P2 |
| `EXCEPT` / `MINUS` | ✅ (I, 2026-05-26) | P2 |
| `EXCEPT ALL` | ✅ (I, 2026-05-26) | P2 |
| `VALUES (a,b), (c,d)` standalone | ✅ (I, 2026-05-26) | P2 |
| `FROM (VALUES (a,b), (c,d)) AS t(c1, c2)` | ✅ (I, 2026-05-26) | P2 |
| `(SELECT ...) UNION (SELECT ...) ORDER BY x LIMIT n` (top-level) | ✅ (I, 2026-05-26) | P2 |

---

## 🔴 7. DML masivo y avanzado (Bloque E3 + J)

| Comando | Soportado | Prioridad | Bloque |
|---|:---:|:---:|:---:|
| `INSERT ... VALUES (...)` (single row) | ✅ | — | — |
| `INSERT ... VALUES (a,b),(c,d)` multi-row | ✅ (J) | — | — |
| `INSERT INTO t SELECT ...` | ✅ (J) | — | — |
| `INSERT ... ON CONFLICT DO NOTHING / DO UPDATE` | ✅ (J2; sin `EXCLUDED.col`) | — | — |
| `REPLACE INTO` (SQLite-style) | ✅ (J2; desugar a ON CONFLICT DO REPLACE) | — | — |
| `UPDATE ... WHERE col_indexada = val` | ✅ (E3) | — | — |
| `UPDATE ... WHERE col IN (SELECT ...)` | ✅ (E3) | — | — |
| `UPDATE ... WHERE` con AND/OR/NOT, LIKE, IS NULL, etc. | ✅ (E3) | — | — |
| `UPDATE ... FROM otra_tabla` (UPDATE con JOIN) | ❌ | P2 | J |
| `DELETE ... WHERE col_indexada = val` | ✅ (E3) | — | — |
| `DELETE` con subquery en WHERE | ✅ (E3) | — | — |
| `DELETE` con JOIN | ❌ | P1 | (futuro) |
| `TRUNCATE [TABLE]` | ✅ (J; scan + cascade respeta FK ON DELETE) | — | — |
| `RETURNING` clause (`INSERT/UPDATE/DELETE ... RETURNING *` o `... RETURNING col, col`) | ✅ (J2) | — | — |

---

## 🔴 8. Transacciones explícitas (Bloque T)

| Comando | Soportado | Prioridad |
|---|:---:|:---:|
| Auto-commit por `exec` | ✅ | — |
| `BEGIN` / `BEGIN TRANSACTION` / `BEGIN WORK` / `START TRANSACTION` | ✅ (T) | — |
| `COMMIT` / `COMMIT TRANSACTION` / `COMMIT WORK` / `END` | ✅ (T) | — |
| `ROLLBACK` / `ROLLBACK TRANSACTION` / `ROLLBACK WORK` | ✅ (T) | — |
| `SAVEPOINT name` / `ROLLBACK TO SAVEPOINT` | ❌ | P1 |
| `SET TRANSACTION ISOLATION LEVEL ...` | ❌ | P2 |
| Read-only transactions (`BEGIN READ ONLY`) | ❌ | P2 |
| **Cross-request transactions** (mantener tx abierta entre `/exec` HTTP) | ❌ | P1 (requiere session state en el server) |

> **Limitación documentada**: `ROLLBACK` opera sobre el cache del Pager y descarta TODO lo cacheado en el batch, incluidas las sentencias previas al `BEGIN`. Funciona limpio cuando `BEGIN` es la primera sentencia del batch. Para abortar selectivo se necesita `SAVEPOINT` (P1).

---

## 🔴 9. DDL faltante (Bloque K + L)

### Tabla y schema
| Comando | Soportado | Prioridad |
|---|:---:|:---:|
| `CREATE TABLE` con `PRIMARY KEY(a,b)` compuesta | ✅ (K2, 2026-05-26) | all-INT NOT NULL, ver ADR-0019 |
| `CREATE TABLE ... AS SELECT ...` (CTAS) | ✅ (K1, 2026-05-26; primera col del SELECT debe ser INT no-NULL → PK) | — |
| `CREATE TEMPORARY TABLE` | ❌ | P2 |
| `ALTER TABLE ADD COLUMN` | ✅ | — |
| `ALTER TABLE DROP COLUMN` | ✅ (K1, 2026-05-26; con `IF EXISTS`; bloqueado sobre PK / indexada / FK) | — |
| `ALTER TABLE RENAME COLUMN` | ✅ (K1, 2026-05-26; arrastra PK + índices + FKs entrantes) | — |
| `ALTER TABLE RENAME TO` | ✅ (K1, 2026-05-26; alias `RENAME TABLE`; arrastra FKs entrantes) | — |
| `ALTER TABLE ADD [CONSTRAINT name] CHECK (expr)` | ✅ (L3, 2026-05-27; re-valida filas existentes con full-scan, aborta con `[GBY-3008]` sin estado parcial) | — |
| `CREATE TABLE (..., CONSTRAINT name PRIMARY KEY/UNIQUE/FOREIGN KEY ...)` | ✅ (residual #2, 2026-05-27; FK single-col únicamente) | — |
| `ALTER TABLE ADD CONSTRAINT name PRIMARY KEY/UNIQUE/FOREIGN KEY` | ❌ | P2 |
| `ALTER TABLE DROP CONSTRAINT [IF EXISTS] <name>` | ✅ (residual #2, 2026-05-27; lookup CHECK/UNIQUE/FK; PK rechazada con `[GBY-4072]`) | — |
| `ALTER TABLE ALTER COLUMN ... TYPE ...` | ❌ | P2 (K2 — requiere rewrite tipado) |
| `DROP TABLE ... CASCADE` | ❌ | P2 |

### Índices
| Comando | Soportado | Prioridad |
|---|:---:|:---:|
| `CREATE INDEX ... ON t (col)` single | ✅ | — |
| `CREATE UNIQUE INDEX` | ✅ | — |
| `CREATE INDEX ... ON t (a, b, ...)` compuesto | ✅ (K2, 2026-05-26) | all-INT, equality-only via fingerprint FNV-1a-64 |
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
| `FOREIGN KEY ... ON DELETE SET NULL` | ✅ (L1, 2026-05-27; `[GBY-3009]` si la columna del child es NOT NULL) | — |
| `FOREIGN KEY ... ON DELETE SET DEFAULT` | ✅ (L1, 2026-05-27; `[GBY-3010]` si no hay DEFAULT) | — |
| `FOREIGN KEY ... ON DELETE NO ACTION` | ✅ (L1, 2026-05-27; alias de RESTRICT en este release) | — |
| `FOREIGN KEY ... ON UPDATE ...` | ✅ activación real (residual #4, 2026-05-27): UPDATE de PK lifted; CASCADE/SET NULL/SET DEFAULT/RESTRICT/NO ACTION disparados sobre cada FK entrante. UPSERT DO UPDATE sigue restringido | — |
| `FOREIGN KEY (a, b) REFERENCES p (x, y)` multi-col | ✅ (residual #3, 2026-05-27; target = PK compuesta del parent vía fingerprint K2) | — |
| `CHECK (cond)` | ✅ (L2, 2026-05-27) — column-level + table-level, con/sin nombre, 3VL ANSI, sin subqueries (`[GBY-4069]`) | — |
| `EXCLUDE USING ...` (Postgres-style) | ❌ | P3 |
| Constraints diferidas (`DEFERRABLE INITIALLY DEFERRED`) | ❌ | P3 |
| Multi-column `PRIMARY KEY` standalone | ✅ (K2, 2026-05-26; all-INT NOT NULL) | — |
| Multi-column `UNIQUE` standalone table-level | ✅ (L1, 2026-05-27; all-INT NOT NULL, mismo encoder que K2) | — |

---

## 🔴 10. Vistas, sequences, triggers (Bloques V + X)

| Objeto | Soportado | Prioridad |
|---|:---:|:---:|
| `CREATE VIEW` / `DROP VIEW` | ✅ (V, 2026-05-27; read-only, source debe ser SELECT simple — set ops `[GBY-4078]`, INSERT/UPDATE/DELETE rechazados con `[GBY-4075]`) | — |
| `CREATE MATERIALIZED VIEW` | ❌ | P3 |
| `CREATE SEQUENCE` / `nextval` | ❌ | P1 |
| `AUTO_INCREMENT` / `SERIAL` / `IDENTITY` (PK auto) | ❌ | P0 |
| `CREATE SCHEMA` / namespace | ❌ | P3 |
| `CREATE TRIGGER name {BEFORE|AFTER} {INSERT|UPDATE|DELETE} ON t FOR EACH ROW <body>` | ✅ (X1+X2, 2026-05-28; AFTER en X1, BEFORE en X2) | — |
| Trigger body multi-statement (`BEGIN stmt; stmt; END`) | ✅ (X2, 2026-05-28) | — |
| NEW mutable en BEFORE triggers (`NEW.col := ...`) | ❌ (X2: NEW read-only; diferido a X3+) | P2 |
| `IF expr THEN ... [ELSIF ...]* [ELSE ...] END IF` (statement top-level + en bodies) | ✅ (X4, 2026-05-28; anidado OK; NEW/OLD/params via substitución pre-parse) | — |
| Variables locales (`DECLARE name TYPE [DEFAULT expr]`) + asignación (`SET name = expr`) + `WHILE cond LOOP ... END LOOP` + `EXIT [WHEN cond]` | ✅ (X4b, 2026-05-28; scope plano, vars NO usables en INSERT VALUES) | — |
| `FOR i IN a..b LOOP`, `FOR row IN SELECT ... LOOP`, `LOOP ... END LOOP` standalone | ❌ (diferido a X4c+) | P3 |
| Nested scope real (BEGIN..END como block scope) | ❌ (X4b: scope plano) | P3 |
| `RAISE [EXCEPTION\|NOTICE] 'msg'` | ✅ (X4c, 2026-05-28; default EXCEPTION) | — |
| `FOR ident IN start TO end LOOP ... END LOOP` | ✅ (X4c, 2026-05-28; auto-decl var, ascendente step=1) | — |
| `BEGIN <body> [EXCEPTION WHEN OTHERS THEN <handler>] END` (try/catch catch-all) | ✅ (X4d, 2026-05-28; lookahead distingue de BEGIN TRANSACTION) | — |
| `LOOP <body> END LOOP` standalone (sin WHILE/FOR) | ✅ (X4d, 2026-05-28; infinite hasta EXIT o MAX_ITER) | — |
| `EXCEPTION WHEN <code> THEN <handler>` filtros por código numérico + múltiples WHEN encadenados + OTHERS fallback | ✅ (X4e, 2026-05-29; código `[GBY-NNNN]` sin prefijo) | — |
| `CASE WHEN cond THEN <stmts> [ELSE <stmts>] END CASE` statement-level (searched form) | ✅ (X4e, 2026-05-29) | — |
| `EXCEPTION WHEN <name>` filtros simbólicos (`WHEN no_data_found`) | ❌ (X4e solo numéricos; diferido) | P3 |
| `CASE expr WHEN val THEN ...` simple form como statement | ❌ (diferido) | P3 |
| `FOR row IN (SELECT ...) LOOP` (resultset iteration con composite row scope `row.col`) | ✅ (X6, 2026-05-29; SELECT obligatorio entre paréntesis) | — |
| `FOR row IN SELECT ... LOOP` sin paréntesis (estilo PG strict) | ❌ (diferido — X6 requiere paréntesis) | P3 |
| `RETURN expr` en function bodies multi-statement (`AS BEGIN ... RETURN x; END`) | ✅ (X4f, 2026-05-29; sentinel pattern, single-expr body de X3b sigue válido) | — |
| `STEP n` / `REVERSE` en FOR range | ✅ (X5, 2026-05-29; `STEP 0` rechazado con `[GBY-4120]`) | — |
| `RAISE WARNING` / `RAISE INFO` (además de EXCEPTION/NOTICE) | ✅ (X5, 2026-05-29; mismo behavior que NOTICE, distinto prefijo) | — |
| Formato `%` en RAISE (`RAISE EXCEPTION 'val % bad', x`) | ✅ (X5, 2026-05-29; arity strict, `%%` escapa) | — |
| `EXCEPTION WHEN <name>` filtros simbólicos PG-style | ✅ (X5, 2026-05-29; `primary_key_violation`/`unique_violation`/`foreign_key_violation`/etc. via `resolve_exception_name`) | — |
| `RAISE EXCEPTION`/`RAISE NOTICE` desde trigger body | ❌ (workaround: hacer un DML que falle) | P3 |
| Triggers sobre vistas (`INSTEAD OF`) | ❌ | P3 |
| `CREATE PROCEDURE name(...) AS <body>` + `CALL name(args)` | ✅ (X3, 2026-05-28; VERSION 14→15; statement-only, params via token-sub) | — |
| `CREATE FUNCTION name(...) RETURNS TYPE AS <expr>` invocable en SELECT/WHERE | ✅ (X3b, 2026-05-28; VERSION 15→16; body es Expr no SELECT) | — |
| `CREATE FUNCTION ... RETURNS TABLE` (table-valued functions) | ❌ | P3 |
| Body de function como `SELECT` (con FROM) | ❌ (workaround: usar body Expr con built-ins) | P3 |
| Args OUT/INOUT en procedures, `DECLARE` variables, `IF`/`LOOP`/`WHILE` en body | ❌ (diferido a X4 — PL/pgSQL completo) | P3 |
| `CREATE TYPE` (enums, composites) | ❌ | P3 |

---

## 🔴 11. CTE y window functions (Bloque W)

| Feature | Soportado | Prioridad |
|---|:---:|:---:|
| `WITH cte AS (SELECT ...) SELECT ...` | ✅ (W1, 2026-05-28; múltiples + encadenadas, visible en JOIN/subquery/set-ops, shadowing ANSI) | — |
| `WITH cte(c1, c2) AS (...)` (column aliases en cabecera) | ❌ (W1/W2 rechazan con `[GBY-4081]`; workaround inline) | P2 |
| `WITH RECURSIVE name AS (anchor UNION [ALL] step) <body>` | ✅ (W2, 2026-05-28; una sola CTE recursive, body canónico, delta semantics, guards 1000 iter / 100K rows) | — |
| Múltiples CTEs `RECURSIVE` o mezcla recursive + no-recursive en mismo `WITH` | ❌ (W2 rechaza con `[GBY-4082]`) | P3 |
| `ROW_NUMBER() OVER (...)` | ✅ (W3, 2026-05-28) | — |
| `RANK`, `DENSE_RANK`, `NTILE` | ✅ (W3, 2026-05-28) | — |
| `LAG`, `LEAD`, `FIRST_VALUE`, `LAST_VALUE` | ✅ (W3, 2026-05-28; `LAST_VALUE` con full-partition deviation de ANSI) | — |
| `SUM/COUNT/AVG/MIN/MAX OVER (...)` (aggregate windows) | ✅ (W3, 2026-05-28; running con ORDER BY, full-partition sin) | — |
| `OVER (PARTITION BY ... ORDER BY ...)` window spec | ✅ (W3, 2026-05-28; sin frame specs explícitas) | — |
| `ROWS BETWEEN ... AND ...` frame | ❌ (defaults aplican, no se puede customizar) | P3 |
| `WINDOW w AS (...)` named windows | ❌ | P3 |
| `PERCENT_RANK`, `CUME_DIST` | ❌ | P3 |
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
| `TIME` (HH:MM:SS[.fff]) | ✅ (Y, 2026-05-29; VERSION 16→17; code=8; validación lexical en CAST) | — |
| `UUID` (8-4-4-4-12 hex canónico) | ✅ (Y, 2026-05-29; VERSION 16→17; code=9; CAST normaliza a lowercase) | — |
| `BIGINT` / `INTEGER` / `INT8` | ✅ (Y, 2026-05-29; aliases puros de INT — i64 sin enforce) | — |
| `TINYINT` / `SMALLINT` / `INT2` / `MEDIUMINT` / `INT4` | ✅ (Y, 2026-05-29 aliases) + **range enforcement** (Y3, 2026-05-29; VERSION 18→19; `[GBY-4121]` si fuera de rango) | — |
| `VARCHAR(n)` / `CHAR(n)` / `CHARACTER VARYING(n)` / `NVARCHAR(n)` / `STRING` / `CLOB` | ✅ (Y, 2026-05-29; aliases de TEXT) + **enforcement de longitud** (Y2, 2026-05-29; VERSION 17→18; bytes UTF-8, `[GBY-4119]`) | — |
| `REAL` / `DOUBLE` / `DOUBLE PRECISION` | ✅ (Y, 2026-05-29; aliases de FLOAT) | — |
| `NUMERIC[(p,s)]` / `DECIMAL[(p,s)]` / `DEC[(p,s)]` | ✅ (Y6, 2026-05-29; **decimal exacto** con `Value::Decimal { value: i128, scale: u8 }`; VERSION 21→22; `[GBY-4123]`) | — |
| `BOOLEAN` | ✅ (Y, 2026-05-29; alias de BOOL) | — |
| `TIMESTAMP` | ✅ (Y, 2026-05-29; alias de DATETIME) | — |
| `BLOB` / `BYTEA` / `BINARY` / `VARBINARY` (binario crudo) | ✅ (Y4, 2026-05-29; VERSION 19→20; code=10; literal `X'hex'`; CAST AS BLOB; no indexable; no PK/FK/CHECK/DEFAULT) | — |
| `DECIMAL(p,s)` **exacto** (no alias de FLOAT) | ✅ (Y6, 2026-05-29) | — |
| Aritmética `Decimal + Decimal` / `Decimal ± Int` exacta (Add/Sub) | ✅ (Y7, 2026-05-29; align scales + checked i128) | — |
| Aritmética `Decimal * Decimal` / `Decimal / Decimal` / `Decimal % Decimal` exacta | ✅ (Y8, 2026-05-29; Div trunca hacia cero con target_scale = max(a,b,6); Mul scale=a+b con `[GBY-4123]` si > 38) | — |
| Rounding alternativo en Div (half-up, half-even) | ❌ (Y8 trunca; diferido) | P3 |
| `DECIMAL` indexable | ❌ (encoding i128+scale no lex-comparable; diferido) | P3 |
| Enforcement de longitud VARCHAR(n) / CHAR(n) | ✅ (Y2, 2026-05-29; bytes UTF-8) | — |
| Enforcement de rango SMALLINT / TINYINT / MEDIUMINT / INT4 | ✅ (Y3, 2026-05-29) | — |
| `TINYINT UNSIGNED` / `SMALLINT UNSIGNED` / `MEDIUMINT UNSIGNED` / `INT4 UNSIGNED` / `BIGINT UNSIGNED` | ✅ (Y5, 2026-05-29; reusa byte int_width con high bit 0x80; `[GBY-4121]`) | — |
| `UNSIGNED BIGINT` real (u64, no limitado por i64) | ❌ (Y5 sólo enforce >= 0 con upper bound i64::MAX; diferido) | P3 |
| `gen_random_uuid()` / `uuid_v4()` / `uuid_generate_v4()` / `random_uuid()` | ✅ (Y5, 2026-05-29; xorshift PRNG no-crypto, RFC 4122 §4.4) | — |
| `CHAR(n)` con padding a la derecha (estándar SQL) | ❌ (diferido) | P3 |
| Conteo por code points en VARCHAR(n) (vs bytes UTF-8) | ❌ (diferido) | P3 |
| `ARRAY[]` | ❌ (diferido) | P3 |
| `INTERVAL` | ❌ (diferido) | P3 |
| `ENUM` | ❌ (diferido) | P2 |
| `TIME WITH TIME ZONE` / `TIMESTAMP WITH TIME ZONE` | ❌ (diferido) | P3 |
| `GEOMETRY` / `GEOGRAPHY` (PostGIS-like) | ❌ | P3 |
| `INET` / `CIDR` (red) | ❌ | P3 |
| UUID auto-gen v4 random (`gen_random_uuid`/`uuid_v4`/`uuid_generate_v4`/`random_uuid`) | ✅ (Y5, 2026-05-29) | — |
| UUID v7 timestamp-ordered (`uuid_v7`/`uuid_generate_v7`/`gen_uuid_v7`) | ✅ (Y9, 2026-05-29; RFC 9562 §5.7; PRNG no-crypto) | — |
| UUID v1/v6 timestamp-based | ❌ (Y9 sólo v7; diferido) | P3 |
| `gen_random_bytes(n)` (bytes random) | ✅ (Y9, 2026-05-29; xorshift PRNG no-crypto; `Value::Bytes`) | — |
| `SUM(decimal)` / `AVG(decimal)` Decimal-exactos | ✅ (Y9, 2026-05-29; acumulador multi-modo int→decimal→float; AVG aplica política Y8 `target_scale=max(sum_scale,6)`) | — |
| Notación científica en literales numéricos (`1.5e3`, `2.5E-4`) | ✅ (Y9, 2026-05-29; lexer extiende `Number`; `parse_decimal` conserva precisión en exp negativos) | — |
| `POWER(decimal, n)` exact (sin caer a f64) | ❌ (Y9 cae a f64; diferido) | P3 |
| WHERE con comparación heterogénea relajada (string-vs-int normalizado) | ❌ (Y9 estricto; diferido) | P3 |

---

## 🔴 13. Control de acceso (Bloque Z)

Hoy: solo token compartido en el server HTTP. Nada de SQL-level.

| Comando | Soportado | Prioridad |
|---|:---:|:---:|
| `CREATE USER` / `CREATE ROLE` | ✅ (Z1, 2026-05-29; VERSION 22→23; `WITH PASSWORD` o `IDENTIFIED BY`; hash FNV-1a-64 + salt, **NO crypto-grade** — ver ADR-0050) | — |
| `DROP USER [IF EXISTS]` / `DROP ROLE [IF EXISTS]` | ✅ (Z1, 2026-05-29) | — |
| `ALTER USER ... SET PASSWORD ...` / `IDENTIFIED BY` / `WITH PASSWORD` | ✅ (Z1, 2026-05-29; rota salt en cada cambio) | — |
| `GRANT priv [, priv]* ON [TABLE] obj TO user_or_role` (SELECT/INSERT/UPDATE/DELETE/REFERENCES/TRUNCATE/ALL) | ✅ (Z2, 2026-05-29; VERSION 23→24; bitmask u32 persistido; merge OR; PUBLIC implícito; ver ADR-0051) | — |
| `REVOKE priv [, priv]* ON [TABLE] obj FROM user_or_role` | ✅ (Z2, 2026-05-29; AND-NOT; idempotente sin GRANT previo; mask 0 borra el record) | — |
| `SET SESSION AUTHORIZATION 'user' | DEFAULT` | ✅ (Z2, 2026-05-29; activa enforcement en exec\_select/insert/update/delete/truncate; DEFAULT vuelve a superuser) | — |
| Enforcement de privs en DML (PRIVILEGE_DENIED `[GBY-4129]`) | ✅ (Z2, 2026-05-29; check\_priv en hooks; superuser bypass cuando current_user is None) | — |
| `GRANT priv ON COLUMN ... TO ...` (column-level) | ❌ (Z2 sólo per-objeto; diferido) | P3 |
| `WITH GRANT OPTION` (re-grant transitivo) | ❌ (Z2 sin grantor tracking; diferido) | P3 |
| `GRANT role TO user` (role membership) | ❌ (Z1 persiste roles pero sin members; diferido) | P3 |
| Funciones `current_user()` / `session_user()` | ❌ (defer Z3 — útil para policies RLS) | P3 |
| `GRANT EXECUTE ON PROCEDURE/FUNCTION` | ❌ (Z2 sólo tabla/vista; diferido) | P3 |
| Row-level security (RLS) `CREATE POLICY name ON table FOR {ALL|SELECT|UPDATE|DELETE} [TO role,...] USING (expr)` | ✅ (Z3, 2026-05-29; VERSION 24→25; WHERE rewriting + OR semantics; ver ADR-0052) | — |
| `DROP POLICY [IF EXISTS] name ON table` | ✅ (Z3, 2026-05-29) | — |
| `WITH CHECK (expr)` clause + `FOR INSERT` POLICY | ✅ (Z3b, 2026-05-29; VERSION 26→27; `CREATE POLICY ... [USING (expr)] [WITH CHECK (expr)]`; enforcement en INSERT con PERMISSIVE OR; `[GBY-4138]` POLICY_CHECK_VIOLATION; ver ADR-0054) | — |
| UPDATE post-image check con WITH CHECK | ✅ (Z3c, 2026-05-29; hook en exec_update pre-persist; reusa enforce_with_check de Z3b; sin bump on-disk; ver ADR-0055) | — |
| `INSERT ... ON CONFLICT DO UPDATE` con WITH CHECK del UPDATE path | ✅ (Z3e, 2026-05-29; sin bump on-disk; hook en apply_insert_row_with_conflict; reusa enforce_with_check de Z3b; ver ADR-0059) | — |
| `INSERT/UPDATE/DELETE ... RETURNING` filtrado contra policies SELECT | ✅ (Z3d, 2026-05-29; sin bump on-disk; row visible si al menos una SELECT policy USING evalúa TRUE; visibility hiding silencioso; ver ADR-0057) | — |
| `AS PERMISSIVE | RESTRICTIVE` modifier | ❌ (Z3 sólo PERMISSIVE = OR; defer) | P3 |
| `ALTER TABLE ... ENABLE/FORCE ROW LEVEL SECURITY` | ❌ (Z3 activa implícitamente con cualquier policy; defer del flag) | P3 |
| POLICY sobre vistas | ❌ (Z3 sólo tabla base; defer) | P3 |
| `ALTER POLICY` | ❌ (defer; usar DROP + CREATE) | P3 |
| `SET ROLE` / `CURRENT_USER` | ❌ (defer; requiere protocolo extendido server-side) | P3 |
| KDF real para password (PBKDF2-HMAC-SHA256) | ✅ (Z1b, 2026-05-29; VERSION 25→26; Rust puro sin deps; 100K iter OWASP; salt 16B NIST; scheme byte reserva slot para argon2) | — |
| Verificación de password vía `SET SESSION AUTHORIZATION 'name' WITH PASSWORD '...'` | ✅ (Z1b, 2026-05-29; constant-time compare; `[GBY-4137]` AUTH_PASSWORD_INCORRECT si falla) | — |
| scrypt RFC 7914 (memory-hard, resistente a ASIC, ~32 MB/hash) | ✅ (Z1c, 2026-05-29; VERSION 27→28; default; Salsa20/8 + BlockMix + ROMix puro en Rust; ver ADR-0056) | — |
| Blake2b RFC 7693 (foundation crypto) | ✅ (Z1d, 2026-05-29; VERSION 28→29; validado contra RFC 7693 §A; `pub fn blake2b(out_len, data)`; ver ADR-0058) | — |
| `PASSWORD_SCHEME_ARGON2ID = 3` reservado | ✅ (Z1d, 2026-05-29; slot reservado, dispatch con mensaje informativo) | — |
| Argon2id RFC 9106 estructura completa (`pub fn argon2id`) | ⚠️ (Z1e, 2026-05-29; estructura completa ~450 LOC; **NO matchea RFC §A.3 vector todavía**; default sigue scrypt; ver ADR-0060) | — |
| Argon2id RFC 9106 §A.3 test vector match + default scheme=3 | ❌ (Z1f — debug del bug + cambio de default + migración silenciosa) | P3 |
| Migración silenciosa de scheme=1 (PBKDF2) → scheme=2 (scrypt) on next login | ❌ (defer Z1d) | P3 |
| Iteraciones de PBKDF2 configurables (`ALTER SYSTEM ...`) | ❌ (Z1b hardcoded 100K; defer) | P3 |
| Wire-up del verify al servidor HTTP (`Authorization: Bearer user:password`) | ❌ (Z1b sólo expone via SQL; defer) | P2 |
| Quoted identifiers para user/role (`"foo bar"`) | ❌ (Z1 sólo `[a-zA-Z_][a-zA-Z0-9_]*`) | P3 |

---

## 🟡 14. Otros del estándar

| Feature | Soportado | Prioridad |
|---|:---:|:---:|
| `EXPLAIN <statement>` (plan textual sin ejecutar) | ✅ (P1, 2026-05-29; cols step/detail; clasifica scan type honestamente: PK lookup / hash-index / ordered-int / full scan + post-filter; soporta SELECT/INSERT/UPDATE/DELETE/JOIN/ORDER/LIMIT/DISTINCT; ver ADR-0063) | — |
| `EXPLAIN ANALYZE` (timings + row counts reales) | ✅ (P2, 2026-05-29; Instant wall-clock 3 decimales ms + `rs.rows.len()` real + side-effects PERSISTEN + error capturado como step `actual.error`; ver ADR-0064) | — |
| `EXPLAIN` con cost estimates (rows, pages, total cost) | 🟡 (P3, 2026-05-29; `ANALYZE TABLE foo` + EXPLAIN anota `[est.rows=N]` por SCAN. Solo row_count, sin NDV/MCV/histogramas — eso es P4. Stats session-scoped, sin persistencia on-disk — eso es P3b. Ver ADR-0065) | P4 (NDV+MCV) / P5 (optimizer real) |
| `ANALYZE <table>` (colectar stats para optimizador) | ✅ (P3, 2026-05-29; row_count exacto vía scan B+tree, cachea en `Engine.table_stats`, EXPLAIN consume; acepta `ANALYZE TABLE foo` y `ANALYZE foo`; DROP TABLE invalida; session-scoped; ver ADR-0065) | — |
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
| **P0** | ~0 | Cerrado: todos los P0 históricos (AND/OR/NOT, comparaciones, GROUP BY, UPDATE/DELETE indexed, transacciones) entregados durante 2026-05-25..29 |
| **P1** | ~12 | Restantes: bind params (`?`,`$1`), `PREPARE`/`EXECUTE`, `SAVEPOINT`, cross-request tx, `COPY FROM/TO`, `EXPLAIN` cost estimates (P4), planner-as-optimizer (P5), `DELETE ... USING` join, ALL/ANY/SOME, LATERAL |
| **P2** | ~15 | `CREATE SEQUENCE`/`nextval`, cursores explícitos, `MERGE` puro, isolation levels, `BEGIN READ ONLY`, materialized views con REFRESH, FOR UPDATE row-locking, índices funcionales / partial / expression |
| **P3** | ~20 | ARRAY/JSONB, ENUM, time-travel, tablespaces, partitioning, replicación, foreign data wrappers, percentile_disc/cont, JSON path queries, full-text |

---

## 🛣️ Ruta recomendada para "línea de comandos completa"

Si el objetivo es **portar una app SQL clásica sin sorpresas**, esta es la secuencia mínima (todos cerrados hoy):

```
E1 (AND/OR/NOT en WHERE)                                ✅
  ↓
E2 (operadores <, >, LIKE, IS NULL, IN literal)         ✅
  ↓
E3 (UPDATE/DELETE por col indexada y subquery)          ✅
  ↓
F (GROUP BY + HAVING + agregados + DISTINCT)            ✅ (sin JOINs)
  ↓
T (BEGIN / COMMIT / ROLLBACK explícitos)                ✅ (batch-local)
  ↓
J (multi-row INSERT, INSERT...SELECT, TRUNCATE)         ✅
  ↓
J2 (UPSERT, REPLACE INTO, RETURNING)                    ✅ (sin EXCLUDED.col)
  ↓
G1+G2+G3 (funciones escalares + CAST + CASE +
          COALESCE + aritméticos + || + postfix Expr)   ✅
  ↓
H (derived tables, NOT IN (SELECT), subquery en SELECT,
   correlated multi-pred)                               ✅
  ↓
I (UNION, INTERSECT, EXCEPT, MINUS, VALUES)             ✅
  ↓
K1 (CTAS, RENAME TABLE, DROP/RENAME COLUMN)             ✅
  ↓
K2 (PK compuesta + índices compuestos all-INT)          ✅ (VERSION 8)
  ↓
L (CHECK, ON DELETE SET NULL, multi-col UNIQUE)         ⏳
```

Con E1→K2 cerrados, gabysql cubre la **superficie SQL operacional clásica** completa. Lo que sigue (L = constraints adicionales, V = vistas, W = CTE + window, X = triggers + stored procs, Y = tipos exóticos DECIMAL/BLOB/UUID, Z = RLS) es material para casos especializados.

---

## 📚 Referencias cruzadas

- [SQL_REFERENCE.md](SQL_REFERENCE.md) — gramática EBNF + railroad de TODO lo que SÍ se soporta.
- [STATUS.md](STATUS.md) — madurez por subsistema con la línea de demarcación 🟢/🟡/🔴.
- [ERROR_CODES.md](ERROR_CODES.md) — catálogo `[GBY-NNNN]` (nuevos códigos se agregan acá al cerrar cada bloque).
- [TECHNICAL_SPECS.md](TECHNICAL_SPECS.md) — formato en disco + subset SQL exacto.
- [ROADMAP.md](../ROADMAP.md) — dirección técnica general (no entra en este detalle de comandos).
- [AGENDA_INVESTIGACION.md](AGENDA_INVESTIGACION.md) — qué entra y qué no en términos de aprendizaje (algunos bloques de acá pueden ser "anti-agenda" según el momento).
