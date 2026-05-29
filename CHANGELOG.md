# 📝 Changelog

> **Historial de cambios relevantes aplicados al producto y a su base documental.**
>
> Para registro detallado de **bugs e incidentes operativos resueltos en sesión** (regresiones de CI, fixes intermedios, errores de configuración), ver [`docs/INCIDENTS_2026-05-25.md`](docs/INCIDENTS_2026-05-25.md). Para el detalle de los hallazgos del audit interno de seguridad ver [`SECURITY_AUDIT_2026-05-25.md`](SECURITY_AUDIT_2026-05-25.md).

---

## 2026-05-29 — Bloque Y3: enforcement de rango `TINYINT`/`SMALLINT`/`MEDIUMINT`/`INT4`

> **Un push a `main`** con **bump on-disk 18 → 19**. Sigue el patrón de Y2 (`max_length` para VARCHAR(n)/CHAR(n)) — ahora un `int_width: Option<u8>` por columna enforce el rango declarado. Detalle en [`docs/adr/0043-int-range-enforcement-y3.md`](docs/adr/0043-int-range-enforcement-y3.md).

### 🆕 Comportamiento habilitado

```sql
CREATE TABLE personas (
    id   INT PRIMARY KEY,
    edad TINYINT,
    año  SMALLINT,
    clic MEDIUMINT,
    salt INT4
);

INSERT INTO personas (id, edad) VALUES (1, 30);   -- OK
INSERT INTO personas (id, edad) VALUES (2, 200);  -- [GBY-4121] INT_RANGE_EXCEEDED
UPDATE  personas SET edad = -200 WHERE id = 1;    -- [GBY-4121] también
```

### 🛠 Implementación

- **`Column`** (catalog): nuevo campo `pub int_width: Option<u8>` con codes 1=TINYINT, 2=SMALLINT/INT2, 3=MEDIUMINT, 4=INT4. `None` para INT/INTEGER/BIGINT/INT8 (i64 nativo sin enforce).
- **Helper `extract_int_width(type_name) -> Option<u8>`** mapea el `type_name` ya parseado al ancho enforced. Strip de `(...)` por si vino con `INT(11)` legacy de MySQL.
- **Disk format (V19)**: nuevo flag bit `COLUMN_FLAG_HAS_INT_WIDTH = 0x10`. Cuando está prendido, después del bloque `max_length` (Y2) se escribe **1 byte** con el width. Columnas sin `int_width` son byte-idénticas a V18.
- **Enforcement en encoder**: condición agregada al arm `(ColumnType::Int, Value::Integer)` en `encode_row`. Cubre INSERT, UPDATE, INSERT...SELECT y CTAS via el mismo path.
- **`int_width_range(w)` y `int_width_label(w)`** como helpers públicos para mensajes legibles.
- **Bump 18 → 19**: V18 no sabría saltar el byte extra y leería el `idx_count` desde un offset inválido. V18 rechazado con `[GBY-1003]`.

### 🆕 Código de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4121` | `INT_RANGE_EXCEEDED` | INSERT/UPDATE de entero fuera del rango `TINYINT`/`SMALLINT`/`INT2`/`MEDIUMINT`/`INT4`. |

### 🚫 Diferido (Y4 y más allá)

- **`BLOB` / `BYTEA` / `BINARY`** (requiere `Value::Bytes` y cambio de serialización).
- **`DECIMAL(p,s)` exacto** (requiere `Value::Decimal`; hoy es alias de FLOAT).
- **`UNSIGNED TINYINT`/`UNSIGNED INT`/etc.** (MySQL-style — Y3 solo enforce signed).
- **`CHAR(n)` con padding** a la derecha.
- **Conteo por code points** en VARCHAR(n) (vs bytes UTF-8 actual).
- **`ARRAY[T]`**, **`ENUM(...)`**, **`INTERVAL`**, **`TIME/TIMESTAMP WITH TIME ZONE`**.
- **`gen_random_uuid()`** y similares.

### 🧪 Validación

- 14 tests `y3_*`: rangos OK + over + under para TINYINT/SMALLINT/MEDIUMINT/INT4, INT2 alias enforced, BIGINT sin enforce (9 * 10^18 OK), UPDATE también dispara, ALTER ADD COLUMN persiste, reopen mantiene la regla.
- Suite total: **544/544 pass** (`cargo test --lib --tests`).

---

## 2026-05-29 — Bloque X6: `FOR row IN (SELECT ...) LOOP` — cierra el bloque X

> **Un push a `main`** sin bump on-disk. Último item del bloque X — itera fila por fila sobre un resultset con composite row scope (`row.col`). Detalle en [`docs/adr/0042-for-row-in-select-x6.md`](docs/adr/0042-for-row-in-select-x6.md).

### 🆕 Sintaxis habilitada

```sql
DECLARE total INT DEFAULT 0;

FOR r IN (SELECT id, val FROM src WHERE active = TRUE ORDER BY id) LOOP
    SET total = total + r.val;
    EXIT WHEN r.id = 999;
END LOOP;

IF total > 1000 THEN RAISE NOTICE 'sobrepasó umbral'; END IF;
```

### 🛠 Implementación

- **AST**: `Statement::ForSelect(Box<ForSelectStmt { var, query: SelectStmt, body }>)` — hermano de `Statement::For` (range loop).
- **Parser**: lookahead `(SELECT` tras `IN`. Si matchea, parsea como ForSelect; si no, cae al path range (X4c/X5). SELECT obligatorio entre paréntesis.
- **Engine `exec_for_select`**: ejecuta el SELECT, computa `var.col` keys (lowercase), snapshot de valores previos, itera filas inyectando los valores en `var_scope`, ejecuta body (EXIT/RETURN propagan por sentinel), restaura al final. Guard `MAX_LOOP_ITERATIONS=100K` heredado.
- **Fast-path en `eval_expr`**: si `Expr::Column(name)` contiene `.`, prueba el nombre completo en lowercase contra el row map ANTES de `normalize_ident` (que tira el qualifier). Permite resolver `r.id` contra la key `"r.id"` inyectada en var_scope.
- **Composite scope sin tipo nuevo**: las columnas viven como variables flat en `var_scope`. No introducimos `Value::Record` — todo escalar.

### 🚫 Sin nuevos códigos de error

Reusa `[GBY-4110]` LOOP_MAX_ITERATIONS_EXCEEDED y `[GBY-2002]` COLUMN_NOT_FOUND para `row.col` referenciado pero ausente del resultset.

### 🧪 Validación

- 8 tests `x6_*`: iteración básica con count, suma de columnas via `r.col`, último valor persiste, EXIT WHEN funciona, resultset vacío no itera, SELECT con WHERE filtra, FOR ... IN (SELECT) dentro de CREATE PROCEDURE, var declarada antes preservada después del loop.
- Suite total: **530/530 pass** (`cargo test --lib --tests`).

### 🎉 Bloque X cerrado al 100%

Con X6 el bloque X (PL/pgSQL) queda completo: triggers BEFORE/AFTER (X1+X2), stored procedures + CALL (X3), user functions con expr o block body (X3b+X4f), IF/CASE statement-level, DECLARE/SET/WHILE/EXIT, RAISE EXCEPTION/NOTICE/WARNING/INFO con formato `%`, FOR range con STEP/REVERSE, FOR row IN SELECT, BEGIN..EXCEPTION..END con filtros por código numérico/simbólico/OTHERS, LOOP standalone, RETURN expr.

Lo único diferido a futuro: `FOR row IN SELECT` sin paréntesis (alinear con PG), `FOREACH SLICE` (requiere ARRAY type → Y3+), `EXECUTE 'dynamic SQL'`.

---

## 2026-05-29 — Bloque X5: refinamientos PL/pgSQL (RAISE WARNING/INFO, formato `%`, FOR STEP/REVERSE, EXCEPTION WHEN simbólico)

> **Un push a `main`** sin bump on-disk. Cleanup de los 4 items menores que quedaron diferidos al cerrar X4f. Detalle en [`docs/adr/0041-x5-procedural-refinements.md`](docs/adr/0041-x5-procedural-refinements.md).

### 🆕 Sintaxis habilitada

```sql
-- RAISE WARNING / INFO + formato %
RAISE WARNING 'cuidado con el valor %', x;
RAISE INFO  'procesando registro % de %', i, total;
RAISE EXCEPTION 'valor % inválido en columna %', v, col;

-- FOR con STEP y REVERSE
FOR i IN 1 TO 100 STEP 2 LOOP ... END LOOP;
FOR i IN REVERSE 10 TO 1 LOOP ... END LOOP;
FOR i IN REVERSE 100 TO 1 STEP 10 LOOP ... END LOOP;

-- EXCEPTION WHEN <name> simbólico estilo PG
BEGIN
   INSERT INTO t (id) VALUES (1);
EXCEPTION
   WHEN primary_key_violation THEN ...;
   WHEN foreign_key_violation THEN ...;
   WHEN check_violation THEN ...;
   WHEN OTHERS THEN ...;
END;
```

### 🛠 Implementación

- **`RaiseLevel`**: nuevos variants `Warning` e `Info`. Mismo comportamiento que `Notice` — distintos prefijos (`WARNING:` / `INFO:` / `NOTICE:`) en el mensaje. La diferencia es semántica (para el cliente / logger), no del motor.
- **`RaiseStmt.args: Vec<Expr>`**: parser acepta args separados por `,` tras el literal STRING. `format_raise_message(template, args)` substituye cada `%` con la representación textual del arg; `%%` escapa un `%` literal; arity strict → `[GBY-4120]` si mismatch.
- **`ForStmt.step: Option<Expr>` + `reverse: bool`**: parser acepta `REVERSE` opcional antes de start y `STEP n` opcional después de end. Engine calcula `step_effective = ±|n|` y usa `saturating_add` con condición de parada según el signo. `STEP 0` → `[GBY-4120]`.
- **`ExceptionFilter::Name(String)`**: parser captura ident después de WHEN si no es OTHERS ni número. Engine: `resolve_exception_name(n) -> Option<u32>` mapea PG-style (`unique_violation` → 3003, `primary_key_violation` → 3001, etc.). Nombres no mapeados nunca matchean (caen al próximo handler).

### 🆕 Código de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4120` | `RAISE_FORMAT_OR_FOR_STEP_INVALID` | Arity mismatch entre `%` y args en RAISE, **o** `FOR ... STEP 0`. |

### 🚫 Diferido (X6 y más allá)

- **`FOR row IN SELECT ... LOOP`** (composite row scope `row.col`) — requiere extender var_scope con nested records.
- Más nombres simbólicos en `resolve_exception_name` según pidan los users reales.

### 🧪 Validación

- 12 tests `x5_*`: RAISE WARNING/INFO, RAISE format con args, RAISE format con `%%` escape, RAISE format arity error, FOR STEP, FOR REVERSE, FOR REVERSE+STEP combinados, FOR STEP 0 rejected, EXCEPTION WHEN unique_violation cae a OTHERS sobre PK dup (mapping correcto), EXCEPTION WHEN primary_key_violation matchea PK dup, EXCEPTION WHEN nombre desconocido cae a OTHERS.
- Suite total: **522/522 pass** (`cargo test --lib --tests`).

---

## 2026-05-29 — Bloque Y2: enforcement de longitud `VARCHAR(n)` / `CHAR(n)`

> **Un push a `main`** con **bump on-disk 17 → 18**. Cierra la deuda más visible de Y: ahora una columna `VARCHAR(254)` realmente rechaza strings de 300 bytes en vez de aceptarlos silenciosamente. Detalle en [`docs/adr/0040-varchar-length-enforcement-y2.md`](docs/adr/0040-varchar-length-enforcement-y2.md).

### 🆕 Comportamiento habilitado

```sql
CREATE TABLE usuarios (
    id   INT PRIMARY KEY,
    nick VARCHAR(20)
);

INSERT INTO usuarios (id, nick) VALUES (1, 'gaby');                        -- OK
INSERT INTO usuarios (id, nick) VALUES (2, 'string-de-mas-de-20-bytes');   -- [GBY-4119] VALUE_LENGTH_EXCEEDED
UPDATE  usuarios SET nick = 'tampoco-cabe-asi' WHERE id = 1;               -- [GBY-4119] también
```

### 🛠 Implementación

- **`Column`** (catalog): nuevo campo `pub max_length: Option<u32>`. Se setea en CREATE TABLE / ALTER TABLE ADD COLUMN cuando el tipo es familia TEXT y trae `(n)`.
- **Helper `extract_length_param(type_name) -> Option<u32>`**: re-parsea el `(n)` desde el string ya capturado por `parse_type_name`. Solo devuelve `Some(_)` para tipos TEXT family (`VARCHAR`, `CHAR`, `CHARACTER`, `CHARACTER VARYING`, `NVARCHAR`, `NCHAR`, `STRING`, `CLOB`). `NUMERIC(10,2)` y `INT(11)` devuelven `None`.
- **Disk format (V18)**: nuevo flag bit `COLUMN_FLAG_HAS_MAX_LENGTH = 0x08`. Cuando está prendido, después del bloque FK opcional se escriben 4 bytes LE con el `u32`. Columnas sin `max_length` son byte-idénticas a V17.
- **Enforcement en encoder**: una línea agregada al arm `stores_as_text` en `encode_row`. INSERT, UPDATE, INSERT...SELECT y CTAS pasan por el mismo path — el check cubre todos sin duplicación.
- **Conteo en bytes UTF-8** (no code points), igual que el length-prefixed encoding global.
- **Bump 17 → 18**: V17 no sabría saltar los 4 bytes extra y leería el `idx_count` desde un offset inválido. V17 rechazado con `[GBY-1003]`.

### 🆕 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4119` | `VALUE_LENGTH_EXCEEDED` | INSERT/UPDATE de string que excede `VARCHAR(n)` / `CHAR(n)`. |

### 🚫 Diferido (Y3 y más allá)

- **Range enforcement** para `SMALLINT` / `TINYINT`.
- **Conteo por code points** (opt-in vía `CHARACTER SET`).
- **`CHAR(n)` con padding** a la derecha (estándar SQL).
- **`BLOB` / `BYTEA`** (requiere `Value::Bytes`).
- **`DECIMAL(p,s)` exacto** (requiere `Value::Decimal`).
- **`ARRAY[T]`**, **`ENUM(...)`**, **`INTERVAL`**.

### 🧪 Validación

- 8 tests `y2_*`: under limit, exact limit, over limit (4119), `CHAR(n)`, TEXT sin `(n)` sin límite, UPDATE también enforce, ALTER ADD COLUMN persiste, reopen mantiene el límite.
- Suite total: **510/510 pass** (`cargo test --lib --tests`).

---

## 2026-05-29 — Bloque Y: tipos de columna extendidos

> **Un push a `main`** con **bump on-disk 16 → 17**. Aliases sintácticos (BIGINT, VARCHAR(n), DECIMAL(p,s), DOUBLE PRECISION, BOOLEAN, TIMESTAMP, …) + dos tipos nuevos en disco: `TIME` y `UUID`. Detalle en [`docs/adr/0039-extended-types-y.md`](docs/adr/0039-extended-types-y.md).

### 🆕 Sintaxis habilitada

```sql
-- Aliases en CREATE TABLE
CREATE TABLE personas (
    id BIGINT PRIMARY KEY,
    nombre VARCHAR(100),
    edad SMALLINT,
    altura DOUBLE PRECISION,
    saldo DECIMAL(12,2),
    activo BOOLEAN,
    creado TIMESTAMP
);

-- Tipos nuevos con código propio
CREATE TABLE jornadas (
    id INT PRIMARY KEY,
    apertura TIME,
    cierre TIME,
    request_id UUID
);

INSERT INTO jornadas VALUES (1, '09:00:00', '18:30:00', '550e8400-e29b-41d4-a716-446655440000');

SELECT CAST('550E8400-E29B-41D4-A716-446655440000' AS UUID);  -- normaliza a lowercase
```

### 🛠 Implementación

- **`ColumnType`** (catalog): variants nuevos `Time` (code=8) y `Uuid` (code=9), ambos `stores_as_text()=true`.
- **`from_sql`** acepta los aliases (INT family / FLOAT family / TEXT family / BOOL family / DATETIME family). Normaliza case + colapsa whitespace + descarta sufijo paramétrico `(n)`/`(p,s)`.
- **Parser**: helper único `parse_type_name` reemplaza el `expect_ident` para tipos en `parse_column_def`, `parse_create_function`, `parse_create_procedure`, `parse_declare_stmt`, `parse_cast_expr`. Soporta multi-word (`DOUBLE PRECISION`, `CHARACTER VARYING`) y sufijo paramétrico con depth tracking.
- **Encoder/decoder**: `Time`/`Uuid` viajan por la rama `stores_as_text` existente (no hay código nuevo de serialización).
- **CAST**: `CAST(x AS TIME)` valida lexical (`HH:MM:SS[.fff]`); `CAST(x AS UUID)` valida lexical (8-4-4-4-12 hex, total 36 chars) y normaliza a lowercase.
- **Storage**: bump VERSION 16→17 — un V17 con columnas `Time`/`Uuid` no es legible por un binario V16. V16 → rechazado con `[GBY-1003]` UNSUPPORTED_FORMAT_VERSION (export/import manual).

### 🚫 Diferido (Y2 y más allá)

- **Length/range enforcement** en VARCHAR(n)/CHAR(n)/SMALLINT/TINYINT.
- **`BLOB`/`BYTEA`/`BINARY`** (requiere `Value::Bytes` y cambio de serialización).
- **`DECIMAL(p,s)` exacto** (requiere `Value::Decimal`; hoy es alias de FLOAT).
- **`ARRAY[T]`**, **`ENUM(...)`**, **`INTERVAL`**.
- **`TIME WITH TIME ZONE`**, **`TIMESTAMP WITH TIME ZONE`**.
- **Generación auto de UUID** (`gen_random_uuid()`, `uuid_v4()`).
- **Validación semántica** de TIME (24:00 hoy se aceptaría).

### 🧪 Validación

- 13 tests `y_*`: aliases INT/FLOAT/TEXT/BOOL/TIMESTAMP families, columnas TIME, columnas UUID, CAST AS TIME/UUID (con normalización lowercase), aliases en function signature, DECLARE con alias, ALTER TABLE ADD COLUMN con alias, tipo inválido (GEOMETRY) sigue erroreando.
- Suite total: **502/502 pass** (`cargo test --lib --tests`).

---

## 2026-05-29 — Bloque X4f: `RETURN expr` en function bodies

> **Un push a `main`** sin bump on-disk. Décimo y último previsto sub-bloque del bloque X. Function bodies multi-statement con `RETURN expr` como sentinel — habilita lógica procedural completa dentro de functions. Detalle en [`docs/adr/0038-return-in-functions-x4f.md`](docs/adr/0038-return-in-functions-x4f.md).

### 🆕 Sintaxis habilitada

```sql
-- Block body con RETURN (X3b single-expr body sigue válido)
CREATE FUNCTION sign(x INT) RETURNS TEXT AS BEGIN
    IF x < 0 THEN
        RETURN 'negative';
    ELSIF x = 0 THEN
        RETURN 'zero';
    ELSE
        RETURN 'positive';
    END IF;
END;

-- Variables locales + loop + RETURN
CREATE FUNCTION sum_to(n INT) RETURNS INT AS BEGIN
    DECLARE i INT DEFAULT 1;
    DECLARE total INT DEFAULT 0;
    WHILE i <= n LOOP
        SET total = total + i;
        SET i = i + 1;
    END LOOP;
    RETURN total;
END;

SELECT sum_to(10);  -- 55
```

### 🛠 Implementación

- **AST**: `Statement::Return(ReturnStmt { value: Expr })`. Engine field nuevo `pending_return_value: Option<Value>`. Constante `RETURN_SIGNAL = "__GABYSQL_RETURN_SIGNAL__"`.
- **Parser**: `parse_create_function` detecta `BEGIN` tras `AS` → body multi-stmt con depth tracking idéntico a trigger/procedure body. `RETURN` keyword en parse_statement crea `Statement::Return`.
- **Engine**:
  - `exec_return`: evalúa expr, guarda en `pending_return_value`, lanza error sentinel.
  - `eval_user_func` (block body): snapshot del `pending_return_value` previo, ejecuta stmts del body, atrapa el sentinel y retorna el valor, restaura el previo. Sin RETURN → NULL.
  - Sentinel burbujea por IF/WHILE/FOR/CASE/BEGIN sin código extra (mismo patrón que EXIT de X4b).
- **Compat**: single-expression body (X3b) sigue funcionando — el parser elige por lookahead.

### 🚫 Diferido (post-X4f)

- `FOR row IN SELECT ... LOOP` (composite row scope).
- `EXCEPTION WHEN <name>` filtros simbólicos.
- `CASE expr WHEN val THEN ...` simple form como statement.
- Formato `%` en RAISE, `STEP n` / `REVERSE` en FOR range, `RAISE WARNING/INFO`.

### 🧪 Validación

- 6 tests `x4f_*`: single-expr body regression, multi-stmt + RETURN, early RETURN en IF, sin RETURN → NULL, WHILE + RETURN (sum_to(10)=55), function compose (quad = dbl(dbl(x))).
- Suite total: **489/489 pass** (`cargo test --lib --tests`).

---

## 2026-05-29 — Bloque X4e: `CASE` statement + `EXCEPTION WHEN <code>`

> **Un push a `main`** sin bump on-disk. Noveno sub-bloque del bloque X. CASE statement-level (vs CASE expression existente) + filtros por código en EXCEPTION handlers, con OTHERS como fallback opcional. Detalle en [`docs/adr/0037-case-exception-filter-x4e.md`](docs/adr/0037-case-exception-filter-x4e.md).

### 🆕 Sintaxis habilitada

```sql
-- CASE statement (searched form)
CASE
    WHEN amount >= 1000 THEN INSERT INTO platinum VALUES (id);
    WHEN amount >= 100  THEN INSERT INTO gold VALUES (id);
    ELSE INSERT INTO bronze VALUES (id);
END CASE;

-- EXCEPTION filtrada por código
BEGIN
    INSERT INTO t (id) VALUES (1);
EXCEPTION
    WHEN 3001 THEN INSERT INTO dup_log VALUES (1);    -- DUPLICATE_PRIMARY_KEY
    WHEN 4111 THEN INSERT INTO raise_log VALUES (1);  -- RAISE EXCEPTION
    WHEN OTHERS THEN INSERT INTO err_log VALUES (1);  -- fallback
END;
```

### 🛠 Implementación

- **AST**: `Statement::Case(Box<CaseStmt { branches, else_branch }>)`. `BlockStmt.exception_handler: Option<Vec<Statement>>` → `BlockStmt.exception_handlers: Vec<(ExceptionFilter, Vec<Statement>)>`. `ExceptionFilter = Code(u32) | Others`.
- **Parser**: parse_case_stmt como IF pero con `END CASE` close. parse_block_stmt extendido — loop de WHEN branches con filtro entero/OTHERS, requiere THEN, body terminado por WHEN/END.
- **Engine**:
  - `exec_case`: idéntico a IF — eval cond, primer TRUE gana, ELSE fallback.
  - `exec_block`: si error, extrae `[GBY-NNNN]` del mensaje via `extract_error_code`; itera handlers en orden; primer filter que matchee (`Code(n)` exacto o `Others`) corre handler. Sin match → propaga.
- **Helper `extract_error_code`**: parsea prefijo `[GBY-NNNN]` y devuelve u32.
- **Splitter + body parsers**: `CASE` keyword también abre depth (cierra con `END CASE`; CASE expression también queda balanceado porque `END` decrementa).

### 🚫 Diferido (a X4f)

- `RETURN expr` en functions (function body de Expr a Vec<Statement>).
- `FOR row IN SELECT ... LOOP` (composite row scope).
- `EXCEPTION WHEN <name>` filtros simbólicos (`WHEN no_data_found`).
- `CASE expr WHEN val THEN ...` simple form como statement.
- Formato `%` en RAISE.

### 🧪 Validación

- 8 tests `x4e_*`: CASE basic chain, ELSE fallback, no-match + no-ELSE (no-op), EXCEPTION WHEN 4111 atrapa RAISE, WHEN código incorrecto propaga, múltiples WHEN con OTHERS fallback, CASE en procedure body, EXCEPTION atrapa runtime error específico (3001).
- Suite total: **483/483 pass** (`cargo test --lib --tests`).

---

## 2026-05-28 — Bloque X4d: `BEGIN..EXCEPTION..END` + `LOOP` standalone

> **Un push a `main`** sin bump on-disk. Octavo sub-bloque del bloque X. Try/catch con `BEGIN..EXCEPTION WHEN OTHERS THEN..END`, loop infinito standalone, y refactor del splitter unificando WHILE/FOR/LOOP. Detalle en [`docs/adr/0036-exception-loop-x4d.md`](docs/adr/0036-exception-loop-x4d.md).

### 🆕 Sintaxis habilitada

```sql
-- Try/catch
BEGIN
    INSERT INTO t (id) VALUES (1);
    INSERT INTO t (id) VALUES (1);  -- PK dup
EXCEPTION WHEN OTHERS THEN
    INSERT INTO log (id) VALUES (99);
END;

-- LOOP standalone
DECLARE i INT DEFAULT 0;
LOOP
    SET i = i + 1;
    EXIT WHEN i = 5;
END LOOP;

-- EXCEPTION dentro de trigger body (no aborta el INSERT principal)
CREATE TRIGGER safe AFTER INSERT ON t FOR EACH ROW BEGIN
    BEGIN
        RAISE EXCEPTION 'inner';
    EXCEPTION WHEN OTHERS THEN
        INSERT INTO caught (id) VALUES (NEW.id);
    END;
END;
```

### 🛠 Implementación

- **AST**: `Statement::Block(Box<BlockStmt { body, exception_handler }>)` + `Statement::Loop(Box<LoopStmt { body }>)`.
- **Parser**: parse_statement detecta BEGIN no-tx → parse_block_stmt. Lookahead post-BEGIN distingue de `BEGIN [TRANSACTION];`. WHEN OTHERS THEN único soporte en X4d.
- **Engine**:
  - `exec_block`: ejecuta body. Si error contiene EXIT_SIGNAL → re-propaga (loop outer debe recibirlo). Si NO hay handler → propaga error. Si HAY handler → captura y ejecuta handler.
  - `exec_loop`: itera infinitamente hasta EXIT sentinel o MAX_LOOP_ITERATIONS guard.
- **Refactor splitter** (también body parsers): el block-open de loops vive en `LOOP` keyword, no en `WHILE`/`FOR`. Unifica los 3 casos en un solo branch. `END LOOP` se distingue via `just_saw_end` flag. WHILE/FOR ya no abren depth — pasan transparente.

### 🚫 Diferido (a X4e — último previsto del bloque X)

- `RETURN expr` en functions (requiere lift de "function body = Expr" de X3b).
- `FOR row IN SELECT ... LOOP` (composite row scope).
- `EXCEPTION WHEN <code> THEN ...` filtros específicos.
- `RAISE` con formato `%`.
- `CASE` statement (vs CASE expression).

### 🧪 Validación

- 8 tests `x4d_*`: handler atrapa RAISE EXCEPTION, handler atrapa PK dup, sin handler propaga, happy path body completo, LOOP + EXIT WHEN, LOOP max-iter guard, BEGIN dentro de WHILE (handler por iteración), BEGIN dentro de trigger body.
- Suite total: **475/475 pass** (`cargo test --lib --tests`).

---

## 2026-05-28 — Bloque X4c: `RAISE` + `FOR LOOP`

> **Un push a `main`** sin bump on-disk. Séptimo sub-bloque del bloque X. `RAISE EXCEPTION|NOTICE` para aborto/info, `FOR i IN start TO end LOOP` con auto-decl de la variable. EXCEPTION handlers + `FOR row IN SELECT` + `RETURN` diferidos a X4d. Detalle en [`docs/adr/0035-raise-for-x4c.md`](docs/adr/0035-raise-for-x4c.md).

### 🆕 Sintaxis habilitada

```sql
-- RAISE
RAISE EXCEPTION 'something broke';   -- → [GBY-4111] con mensaje
RAISE NOTICE 'informational';        -- → OK con message
RAISE 'aborted';                     -- default = EXCEPTION

-- FOR loop
FOR i IN 1 TO 10 LOOP
    INSERT INTO log SELECT i FROM (VALUES (1)) AS x;
END LOOP;

-- FOR + EXIT
FOR i IN 1 TO 100 LOOP
    EXIT WHEN i = 5;
END LOOP;

-- RAISE como validación en trigger
CREATE TRIGGER validate AFTER INSERT ON orders FOR EACH ROW BEGIN
    IF NEW.amount < 0 THEN
        RAISE EXCEPTION 'negative amount not allowed';
    END IF;
END;
```

### 🛠 Implementación

- **AST**: `Statement::Raise(RaiseStmt)` con `level: Exception|Notice` + `message: String`. `Statement::For(Box<ForStmt>)` con `var, start, end, body`.
- **Parser**: `parse_raise_stmt` (acepta EXCEPTION/NOTICE/default-EXCEPTION). `parse_for_stmt` con sintaxis `FOR id IN start TO end LOOP body END LOOP` (no PG `..`, evita ambigüedad con qualifier).
- **Engine**:
  - `exec_raise`: EXCEPTION → `Err(coded(4111, msg))`; NOTICE → `Ok(message)`.
  - `exec_for`: eval start/end (INT-only o `[GBY-4113]`), auto-decl `var` con save+restore previous, iterate inclusivo, propagar EXIT igual que WHILE.
- **Splitter + body parsers** extendidos: `FOR` block-open (salvo `FOR EACH ROW` del trigger header).

### 🚫 Diferido (a X4d)

- `EXCEPTION WHEN ... THEN <body>` handlers (requiere `BEGIN..END` como Statement con handler field).
- `FOR row IN SELECT ... LOOP` (composite row type en var_scope).
- `LOOP ... END LOOP` standalone.
- `RETURN expr` dentro de functions.
- `CASE` statement.
- `STEP n` y dirección descendente (`REVERSE`).
- Formato `%` en RAISE.
- `RAISE WARNING` / `INFO`.

### 🧪 Validación

- 9 tests `x4c_*`: RAISE EXCEPTION/NOTICE/default, RAISE dentro de IF, FOR counts, FOR + EXIT, FOR range vacío, FOR shadow+restore, FOR bounds inválidos.
- Suite total: **467/467 pass** (`cargo test --lib --tests`).

---

## 2026-05-28 — Bloque X4b: variables locales + `WHILE LOOP` + `EXIT`

> **Un push a `main`** sin bump on-disk. Sexto sub-bloque del bloque X. `DECLARE`/`SET` para variables locales, `WHILE LOOP` con guard de runaway, `EXIT [WHEN]` para break. Detalle en [`docs/adr/0034-vars-loops-x4b.md`](docs/adr/0034-vars-loops-x4b.md).

### 🆕 Sintaxis habilitada

```sql
-- Variables locales
DECLARE counter INT DEFAULT 0;
DECLARE label TEXT;

-- Asignación
SET counter = counter + 1;

-- Loop con guard
WHILE counter < 100 LOOP
    SET counter = counter + 1;
    IF counter >= 50 THEN EXIT; END IF;
END LOOP;

-- Loop con EXIT WHEN
WHILE TRUE LOOP
    SET counter = counter + 1;
    EXIT WHEN counter = 42;
END LOOP;
```

### 🛠 Implementación

- **AST**: Statement::Declare/Set/While(Box)/Exit + structs.
- **Parser**: detección al top de parse_statement; parse_while_stmt usa parse_loop_body que termina en `END`.
- **Engine**: nuevo field `var_scope: HashMap<String, Value>` (scope plano por instancia de Engine — limitación X4b documentada).
  - `exec_declare`: agrega var, error si redeclare ([GBY-4108]).
  - `exec_set`: actualiza var, error si no declarada ([GBY-4107]).
  - `exec_while`: itera con guard `MAX_LOOP_ITERATIONS = 100_000` ([GBY-4109]). EXIT viaja como `DbError` sentinel string que el matcher atrapa.
  - `exec_exit`: emite el sentinel `Err` (Optional WHEN cond pre-eval).
- **eval_expr_full extendido**: cuando `var_scope` no-vacío, hace merge `vars + row` (row gana) y delega a `eval_expr`. Permite que `Expr::Column("counter")` resuelva a la variable cuando no hay columna real homónima.
- **Splitter + body parsers** extendidos para trackear `WHILE ... END LOOP` (junto con BEGIN/END y IF/END IF).

### ⚠️ Limitación conocida — variables en `INSERT VALUES`

`INSERT INTO t VALUES (counter)` **NO** funciona — el parser de VALUES exige Value literal (no Expr), y `counter` aparece como Ident no reconocido. **Workarounds**:
- `INSERT INTO t SELECT counter FROM (VALUES (1)) AS x` (SELECT subquery acepta Expr).
- Usar `UPDATE` (SET acepta Expr).
- En procedures: usar params (`p_n`) en vez de variables locales (los params SÍ se substituyen a literal en CALL).

Lift de esta restricción está fuera de scope de X4b.

### 🚫 Diferido (a X4c+)

- `RAISE EXCEPTION` / `RAISE NOTICE`.
- `EXCEPTION WHEN ... THEN` handlers.
- `FOR i IN a..b LOOP`, `FOR row IN SELECT ... LOOP`.
- `LOOP ... END LOOP` standalone (sin WHILE).
- `RETURN expr` dentro de functions.
- `CASE` statement (vs CASE expression).
- Nested scope real (BEGIN..END como block scope).
- Type checking estricto en DECLARE/SET.

### 🧪 Validación

- 8 tests `x4b_*`: DECLARE+SET+IF, WHILE counter, EXIT WHEN, EXIT unconditional, SET sin DECLARE rechazado, redeclare rechazado, max-iter guard, DECLARE+WHILE dentro de procedure.
- Suite total: **458/458 pass** (`cargo test --lib --tests`).

---

## 2026-05-28 — Bloque X4: control de flujo `IF/THEN/ELSIF/ELSE/END IF`

> **Un push a `main`** sin bump on-disk. Quinto sub-bloque del bloque X. Control de flujo `IF` como statement top-level — utilizable directamente en batches SQL y dentro de bodies de trigger/procedure. Variables locales, `LOOP`, `EXCEPTION` diferidos a X4b+. Detalle en [`docs/adr/0033-if-then-else-x4.md`](docs/adr/0033-if-then-else-x4.md).

### 🆕 Sintaxis habilitada

```sql
-- Statement top-level
IF total >= 100 THEN
    INSERT INTO big_log VALUES (id, total);
ELSIF total >= 10 THEN
    INSERT INTO med_log VALUES (id, total);
ELSE
    INSERT INTO small_log VALUES (id, total);
END IF;

-- Dentro de trigger body
CREATE TRIGGER classify AFTER INSERT ON t FOR EACH ROW BEGIN
    IF NEW.v >= 100 THEN
        INSERT INTO big_log (id) VALUES (NEW.id);
    ELSE
        INSERT INTO small_log (id) VALUES (NEW.id);
    END IF;
END;

-- Dentro de procedure body
CREATE PROCEDURE classify(p_id INT, p_v INT) AS BEGIN
    IF p_v >= 100 THEN INSERT INTO log VALUES (p_id, 'big');
    ELSE INSERT INTO log VALUES (p_id, 'small');
    END IF;
END;

-- Anidado
IF cond1 THEN
    IF cond2 THEN ... END IF;
END IF;
```

### 🛠 Implementación

- **AST**: nuevo `Statement::If(Box<IfStmt>)` con `branches: Vec<(Expr, Vec<Statement>)>` (IF + ELSIF chain) y `else_branch: Option<Vec<Statement>>`.
- **Parser**: `parse_if_stmt` recursivo (IF anidado funciona naturalmente porque `parse_if_body` llama `parse_statement` que vuelve a entrar). `IF` se intercepta en `parse_statement` antes de INSERT — el `IF` de `DROP TABLE IF EXISTS` se consume DENTRO de `parse_drop` y nunca llega.
- **Engine `exec_if`**: evalúa cada condition contra row vacío (NEW/OLD/params ya substituidos por el caller antes del parse del body). Primer TRUE gana; NULL → FALSE (3VL); no-bool → `[GBY-4105]`.
- **`split_statements` extendido** (X2 BEGIN/END tracking → X4 también IF/END IF). Distingue:
  - `IF expr THEN ...` → bloque (depth+1).
  - `END IF` → close-keyword (depth-1; el IF posterior no abre — flag `just_saw_end`).
  - `IF(...)` → función escalar `IF(cond, a, b)` — no abre.
  - `IF [NOT] EXISTS` → DDL conditional — no abre.
- **Body parsers de trigger/procedure** replican el mismo tracking IF/END IF (para capturar el body completo al CREATE-time, no parar antes en el END del IF interno).
- **Tokenizer**: `IF`/`ELSIF`/`THEN` agregados a la lista de keywords que introducen un valor — habilita `IF -5 > 0 THEN` (literal negativo después de `IF`).

### 🚫 Diferido (a X4b+)

- **Variables locales** (`DECLARE x INT DEFAULT expr`).
- **Asignación** (`SET x = expr` / `x := expr`).
- **`WHILE`/`LOOP`/`FOR`** + **`EXIT [WHEN]`** / **`CONTINUE`**.
- **`RAISE EXCEPTION` / `RAISE NOTICE`**.
- **`EXCEPTION WHEN ... THEN`** handlers.
- **`RETURN expr`** dentro de functions.

### 🧪 Validación

- 9 tests `x4_*`: IF simple, IF/ELSE, IF/ELSIF/ELSE chain, IF en trigger/procedure body, IF anidado, condición no-bool rechazada, IF sin END rechazado, IF con NEW dentro de trigger (incluye valores negativos post-subst).
- Suite total: **450/450 pass** (`cargo test --lib --tests`).

---

## 2026-05-28 — Bloque X3b: user-defined scalar functions [VERSION 15→16]

> **Un push a `main`** con bump on-disk **VERSION 15 → 16**. Cuarto sub-bloque del bloque X. `CREATE FUNCTION RETURNS scalar` invocable desde cualquier expresión (SELECT/WHERE/HAVING). Cierra el cuarteto de routines server-side: triggers + procedures + functions. Detalle en [`docs/adr/0032-user-functions-x3b.md`](docs/adr/0032-user-functions-x3b.md).

### 🆕 Sintaxis habilitada

```sql
CREATE FUNCTION dbl(p_x INT) RETURNS INT AS p_x * 2;
CREATE FUNCTION greet(p_name TEXT) RETURNS TEXT AS CONCAT('Hi ', p_name);
CREATE FUNCTION big(p_x INT) RETURNS BOOL AS p_x >= 100;

-- Invocable en SELECT
SELECT id, dbl(v) AS doubled FROM t;
-- Invocable en WHERE
SELECT * FROM t WHERE big(v);
-- Composición
CREATE FUNCTION quad(p_x INT) RETURNS INT AS dbl(dbl(p_x));

DROP FUNCTION [IF EXISTS] dbl;
```

**Body es UNA expresión** (no un SELECT — desviación práctica de ANSI porque gabysql requiere FROM). Para funciones complejas que necesitan consultar tablas, queda para futuro (RETURNS TABLE / body SQL completo).

### 🛠 Implementación

- **Nuevo `Expr::UserFunc { name, args }` variant**: el parser lo emite cuando `IDENT(args)` no matchea ningún `ScalarFunc` built-in. Eval via `eval_expr_full` → `eval_user_func` (catalog lookup + arity + token-sub de params + parse del body como Expr + recursive eval).
- **17 walkers de Expr actualizados** con arm para UserFunc (format, validate, inline_cte, substitute_new_old, rewrite_columns_for_join, memoize, etc.).
- **Persistencia**: nuevo `ObjectKind::Function` (discriminator `4`). `FunctionMeta { name, params, return_type, body_sql }`. Bump VERSION 15→16.
- **Composición trivial**: el body parseado puede contener `Expr::UserFunc` que dispara otro `eval_user_func` recursivamente.
- **CHECK constraints rechazan UserFunc** para preservar pureza (las built-ins puras siguen permitidas).

### 🚫 Diferido (a futuro)

- Body como SELECT (ANSI-puro `AS $$ SELECT ... $$`) — requiere `SELECT` sin FROM o convención de invocación.
- `RETURNS TABLE` (table-valued functions usables en FROM).
- Type checking estricto en arg/return.
- Recursion guard (`MAX_FUNCTION_DEPTH`).
- `IMMUTABLE`/`STABLE`/`VOLATILE` hints para el planner.
- Body PL/pgSQL (variables, IF, LOOP) — X4.

### 🧪 Validación

- 9 tests `x3b_*` (simple SELECT, WHERE, builtin dentro del body, arity, not_found, DROP, persistencia, name collision, composición).
- Test pre-existente `g1_errors_arity_type_unknown` actualizado: `FOO(1)` ahora retorna `[GBY-4103]` (no `[GBY-4037]`) porque el parser optimistamente lo trata como user-defined.
- Suite total: **441/441 pass** (`cargo test --lib --tests`).

### 🎉 Routines server-side completas

Con X3b cierran las 4 routines server-side clásicas:

| Routine | Statement | Resultado | Bloque |
|---|---|---|---|
| Trigger AFTER | (auto-fire) | Side effect post-write | X1 |
| Trigger BEFORE + multi-stmt | (auto-fire) | Side effect pre-write + multi-DML | X2 |
| Procedure | `CALL name(args)` | Side effects parametrizados | X3 |
| Function | `name(args)` en Expr | Valor escalar | X3b |

---

## 2026-05-28 — Bloque X3: stored procedures (`CREATE PROCEDURE` + `CALL`) [VERSION 14→15]

> **Un push a `main`** con bump on-disk **VERSION 14 → 15**. Tercer sub-bloque del bloque X. Stored procedures con `CALL`, persistidas en el catálogo (`ObjectKind::Procedure`). Funciones invocables desde SELECT diferidas a X3b. Detalle en [`docs/adr/0031-stored-procedures-x3.md`](docs/adr/0031-stored-procedures-x3.md).

### 🆕 Sintaxis habilitada

```sql
CREATE PROCEDURE log_msg(p_id INT, p_msg TEXT) AS
    INSERT INTO log (id, msg) VALUES (p_id, p_msg);

CREATE PROCEDURE log_both(p_id INT) AS BEGIN
    INSERT INTO log_a (id) VALUES (p_id);
    INSERT INTO log_b (id) VALUES (p_id);
END;

CALL log_msg(42, 'hello');
CALL log_both(10 + 5);  -- args son expresiones

DROP PROCEDURE [IF EXISTS] log_msg;
```

### 🛠 Implementación

- **Persistencia**: nuevo `ObjectKind::Procedure` (discriminator `3`). `ProcedureMeta { name, params: Vec<(String, ColumnType)>, body_sql }`. Body persistido como texto SQL.
- **Bump VERSION 14 → 15**. V14 abierto por binario X3+ rebota con `[GBY-1003]`.
- **Substitución de parámetros via token-sub bare-ident**: `substitute_params_in_sql_text` busca tokens `Ident` cuyo texto matchee param name (case-insensitive) y los reemplaza por los tokens del literal del arg evaluado.
- **CALL statement standalone**: no es Expr. El executor: lookup → arity check → evaluar args con `eval_expr_full` contra fila vacía → bind → token-sub → parse + exec cada stmt.
- **Body grammar** idéntico a triggers: DML simple o `BEGIN ... END` multi-stmt. Reusa el splitter de X2 (que ya distingue `BEGIN TRANSACTION` vs `BEGIN <body>`).

### 🚫 Limitación conocida (documentada)

Si una columna real tiene el mismo nombre que un parámetro, el ident de la columna también se substituye y la query rompe. **Workaround**: prefijar param names (`p_id`, `arg_name`) — convención estándar PG.

### 🚫 Diferido (a X3b+)

- **`CREATE FUNCTION ... RETURNS scalar`** invocable desde SELECT/WHERE — requiere extender el AST de `Expr` (~160 match arms).
- **Type checking estricto** en CALL (hoy confía en que el DML downstream rebote).
- **Args OUT/INOUT**, **`DECLARE` variables locales**, **control de flujo** (`IF`/`LOOP`/`WHILE`) — área de lenguaje procedural, X4+.
- **Recursion guard** para procedures (`MAX_PROCEDURE_DEPTH`).

### 🧪 Validación

- 9 tests `x3_*`: CALL simple, multi-stmt body, args como expresiones, arity mismatch, procedure desconocida, DROP + IF EXISTS, name collision, persistencia tras reopen.
- Suite total: **432/432 pass** (`cargo test --lib --tests`).

---

## 2026-05-28 — Bloque X2: triggers BEFORE + body multi-statement

> **Un push a `main`** sin bump on-disk (los slots ya estaban en `TriggerMeta` desde X1). Cierra los dos huecos más visibles que dejó X1: BEFORE triggers y body `BEGIN ... END` con múltiples sentencias. Detalle en [`docs/adr/0030-triggers-before-multistmt-x2.md`](docs/adr/0030-triggers-before-multistmt-x2.md).

### 🆕 Sintaxis habilitada

```sql
-- BEFORE triggers (X1 los rechazaba con [GBY-4093])
CREATE TRIGGER log_pre BEFORE UPDATE ON products
    FOR EACH ROW INSERT INTO change_log (id, op, oldv, newv)
                 VALUES (NEW.id, 'before', OLD.price, NEW.price);

-- Body multi-statement con BEGIN ... END
CREATE TRIGGER multi AFTER INSERT ON t FOR EACH ROW BEGIN
    INSERT INTO log_a (id) VALUES (NEW.id);
    INSERT INTO log_b (id) VALUES (NEW.id);
END;
```

### 🛠 Implementación

- **BEFORE triggers**: lift del rechazo X1. Hooks BEFORE en `exec_insert/update/delete` antes del actual write.
  - BEFORE INSERT: NEW = user-stated cols + NULL para no especificadas.
  - BEFORE UPDATE: snapshot OLD del disco, NEW = OLD con assignments evaluados contra OLD (sin tocar disco).
  - BEFORE DELETE: snapshot OLD del disco.
  - NEW es **read-only** en X2 — para rellenar defaults o mutar NEW desde el trigger (`updated_at = NOW()`) hace falta X3.
  - Aborto: si el body del BEFORE rebota, propaga el error y el DML principal aborta (transacción rollback del wrap caller).
- **Body multi-statement**:
  - `split_statements` ahora distingue `BEGIN [TRANSACTION];` (transaction, no abre block) de `BEGIN <dml>` (block-open, mantiene `;` internos juntos en el chunk). Lookahead post-`BEGIN`: si lo que sigue es `;`/EOF/`TRANSACTION` → tx; sino → block.
  - Tokenizer acepta `;` como `Symbol(";")` (pre-X2 nunca llegaba a tokenize).
  - Parser de `CREATE TRIGGER`: detecta `BEGIN` tras `FOR EACH ROW`, consume hasta `END` matching (con depth para potencial nesting), captura body como texto entre `BEGIN` y `END`.
  - `fire_triggers`: tras la substitución NEW/OLD, parsea el body — `parse()` re-splittea por `;` y devuelve `Vec<Statement>`. Ejecuta cada uno en orden.
- **Helper `has_trigger(table, event, timing)`**: generalización de `has_after_trigger` para evitar snapshots OLD/NEW innecesarios cuando no hay triggers BEFORE/AFTER registrados.

### 🚫 Diferido (a X3+)

- **NEW mutable en BEFORE** (típico use case: `NEW.updated_at = NOW()`).
- **Control de flujo**: `IF`/`LOOP`/`WHILE`, variables locales (`DECLARE`).
- **`RAISE EXCEPTION` / `RAISE NOTICE`**: aborto explícito con mensaje.
- **`CREATE FUNCTION` / `CREATE PROCEDURE`**.
- **OLD para UPSERT que terminó en UPDATE**.
- **Triggers sobre vistas (`INSTEAD OF`)**.

### 🧪 Validación

- 7 nuevos tests `x2_*`: BEFORE INSERT/UPDATE/DELETE, multi-stmt body, BEFORE+AFTER en mismo INSERT, BEFORE que aborta via PK violation, `BEGIN` sin `END`.
- Removido `x1_before_rejected_in_release` (la restricción ya no aplica).
- Suite total: **423/423 pass** (`cargo test --lib --tests`).

---

## 2026-05-28 — Bloque X1: triggers AFTER (`CREATE TRIGGER`)

> **Un push a `main`** con bump on-disk **VERSION 13 → 14**. Entrega el primer sub-bloque del bloque X del roadmap: triggers AFTER con body single-statement, persistidos en el catálogo. Detalle en [`docs/adr/0029-triggers-after-x1.md`](docs/adr/0029-triggers-after-x1.md).

### 🆕 Sintaxis habilitada

```sql
CREATE TRIGGER audit_user_insert AFTER INSERT ON users
    FOR EACH ROW INSERT INTO audit (id, action, who) VALUES (NEW.id, 'inserted', NEW.id);

CREATE TRIGGER log_price_change AFTER UPDATE ON products
    FOR EACH ROW INSERT INTO price_log (id, old_price, new_price)
                 VALUES (NEW.id, OLD.price, NEW.price);

CREATE TRIGGER tomb AFTER DELETE ON items
    FOR EACH ROW INSERT INTO removed (id, name) VALUES (OLD.id, OLD.name);

DROP TRIGGER [IF EXISTS] audit_user_insert;
```

### 🛠 Implementación

- **Persistencia**: nuevo `ObjectKind::Trigger` en el catálogo (discriminator `2`). `TriggerMeta { name, table, timing, event, body_sql }`. Body persistido como texto SQL — mismo enfoque que `ViewMeta.source`.
- **Bump VERSION 13 → 14**. V13 abierto por un binario X1+ rebota con `[GBY-1003]` (migración manual: dump + recreate).
- **NEW/OLD vía substitución a nivel de TOKEN**: el parser de INSERT VALUES solo acepta literales, no Expr — así que en lugar de un walker AST, tokenizamos el body persistido, reemplazamos cada token `Ident("NEW.x"|"OLD.x")` por los tokens del literal correspondiente del row, reconstruimos SQL y parseamos. Funciona en cualquier contexto.
- **Hooks en `exec_insert/update/delete`**: AFTER fires después del write exitoso. `has_after_trigger(table, event)` evita el overhead de snapshot OLD/NEW cuando no hay triggers.
- **Guard de recursión `MAX_TRIGGER_DEPTH = 16`** vía `Engine::trigger_depth`. Cascada infinita rebota `[GBY-4095]`.

### 🚫 Diferido (con código de error explícito)

- **BEFORE triggers** → `[GBY-4093]`. Diferido a X2 (necesita semántica clara de NEW antes-de-defaults y abort).
- **Body multi-statement (`BEGIN ... END`)** → X2.
- **Lenguaje procedural** (variables, IF/THEN, LOOP, EXCEPTION) → X3+.
- **`CREATE FUNCTION` / `CREATE PROCEDURE`** → X3+.
- **Triggers sobre vistas (INSTEAD OF)** → futuro.

### 🧪 Validación

- 10 tests `x1_*`: audit INSERT, UPDATE con NEW+OLD, DELETE con OLD, persistencia tras reopen, DROP + DROP IF EXISTS, BEFORE rechazado, colisión de nombres, recursion guard, body no-DML rechazado.
- Suite total: **417/417 pass** (`cargo test --lib --tests`).

---

## 2026-05-28 — Bloque W3: window functions (cierra bloque W)

> **Un push a `main`.** Última pieza del bloque W del roadmap. 13 funciones soportadas (ranking, aggregate, value) con `OVER ( [PARTITION BY ...] [ORDER BY ...] )`. Sin bump de formato — puro motor de proyección. Detalle en [`docs/adr/0028-window-functions.md`](docs/adr/0028-window-functions.md).

### 🆕 Sintaxis habilitada

- **Ranking**: `ROW_NUMBER()`, `RANK()`, `DENSE_RANK()`, `NTILE(n)`.
- **Aggregate windows**: `COUNT(*)`, `COUNT(expr)`, `SUM`, `AVG`, `MIN`, `MAX` con `OVER (...)`.
- **Value**: `LAG(expr [, offset [, default]])`, `LEAD(...)`, `FIRST_VALUE(expr)`, `LAST_VALUE(expr)`.
- **WindowSpec**: `PARTITION BY` y `ORDER BY` ambos opcionales (sujeto a restricciones por función).

Ejemplos:

```sql
-- numerar dentro de cada region por salary descendente
SELECT id, region, ROW_NUMBER() OVER (PARTITION BY region ORDER BY salary DESC) AS rk
FROM employees;

-- total corrido por orden de fecha
SELECT date, amount, SUM(amount) OVER (ORDER BY date) AS running
FROM transactions;

-- mirar fila anterior
SELECT date, price, LAG(price) OVER (ORDER BY date) AS prev_price
FROM ticks;

-- partir en quartiles
SELECT id, score, NTILE(4) OVER (ORDER BY score DESC) AS quartile
FROM students;
```

### 🛠 Implementación

- **Pipeline dedicado** `exec_window_select`: detecta windows en `stmt.columns` y deriva.
  1. Materializa todas las filas source ejecutando una copia con `SELECT * FROM ... WHERE ...`.
  2. Por cada window item: particiona, ordena, y computa per-row el valor.
  3. Proyecta cada fila combinando Column / Expression / Window precomputado.
  4. Aplica el ORDER BY / LIMIT / OFFSET originales sobre el resultado.
- **Defaults de frame** según familia (sin frame specs explícitas en este release):
  - Ranking: per-row.
  - Aggregate con `ORDER BY` → running (RANGE UNBOUNDED PRECEDING AND CURRENT ROW).
  - Aggregate sin `ORDER BY` → full partition.
  - `LAST_VALUE` → full partition (**desviación de ANSI**, documentada).
- **Compute per función**: ranking via comparaciones de tie por order_by; NTILE via distribución balanceada (las primeras `N % buckets` particiones reciben 1 fila más); LAG/LEAD con offset+default.

### 🚫 Diferido

- Frame specs explícitas (`ROWS BETWEEN N PRECEDING AND CURRENT ROW`, etc.).
- `WINDOW w AS (...)` named windows.
- `PERCENT_RANK`, `CUME_DIST`.
- Mezcla con `GROUP BY` / `HAVING` / agregados clásicos en el mismo SELECT — `[GBY-4090]`. Workaround: derived table.

### 🧪 Validación

- 12 tests `w3_*` cubriendo cada función + edge cases (NULL en LAG, FIRST/LAST_VALUE con full-partition, NTILE distribución desigual, RANK vs DENSE_RANK con ties).
- Suite total: **407/407 pass** (`cargo test --lib --tests`).

### 🎉 Bloque W completo

Con W3 cierra el bloque W del roadmap (W1 + W2 + W3 en tres pushes consecutivos: 2026-05-28). Próximos candidatos en el roadmap: Fase 3 (planner + `EXPLAIN` + comparativa con SQLite/PG/MySQL/DuckDB), bloque X (triggers + stored procs), bloque Y (tipos faltantes DECIMAL/BLOB/UUID), bloque Z (RLS).

---

## 2026-05-28 — Bloque W2: `WITH RECURSIVE` (fixpoint base+step)

> **Un push a `main`.** Entrega la mitad recursive del bloque W del roadmap: `WITH RECURSIVE name AS (anchor UNION [ALL] step) <body>`. Sin bump de formato — la materialización vive solo en runtime. Detalle en [`docs/adr/0027-with-recursive.md`](docs/adr/0027-with-recursive.md).

### 🆕 Sintaxis habilitada

- Generador de números clásico:
  ```sql
  WITH RECURSIVE nums AS (
      SELECT 1 AS n
      UNION ALL
      SELECT n + 1 FROM nums WHERE n < 100
  )
  SELECT n FROM nums;
  ```
- Traversal de jerarquías (descendientes de un nodo):
  ```sql
  WITH RECURSIVE descendants AS (
      SELECT id FROM tree WHERE id = :root_id
      UNION ALL
      SELECT t.id FROM tree t INNER JOIN descendants d ON t.parent = d.id
  )
  SELECT id FROM descendants;
  ```
- La CTE materializada es JOINeable desde el body con tablas persistentes y participa en cualquier expresión de SELECT.

### 🛠 Implementación

- **Algoritmo de fixpoint con delta semantics ANSI** (no cumulative): cada iteración procesa solo las filas nuevas de la iteración anterior — terminación natural cuando `delta = ∅`.
- **Bridge a través del inlining de W1**: el `accum` final se convierte a un `SelectStmt` con `values_source` (cada `Value` envuelto en `Expr::Literal`) vía `rows_to_values_select`, y se inyecta al body reusando `inline_cte_into_query` del bloque W1.
- **Mismo bridge en cada iteración del fixpoint**: el step se clona, se inlinea con el delta como virtual table, y se ejecuta. El executor no necesita saber que hay recursión.
- **Dedup vía `format!("{:?}", row)`** porque `Value` no implementa `Hash` (la variant `Float` no es totalmente ordenable). Suficiente para `UNION` (vs `UNION ALL`).
- **Guards de runaway**: 1000 iteraciones máximas (`[GBY-4083]`), 100K filas acumuladas máximas (`[GBY-4084]`).

### 🚫 Diferido (con código de error explícito)

- Múltiples CTEs recursive en el mismo `WITH` → `[GBY-4082]`. Workaround: anidar.
- Body que no es `anchor UNION [ALL] step` canónico → `[GBY-4086]`.
- Schema mismatch entre anchor y step (arity distinta) → `[GBY-4085]`.
- Column aliases en la cabecera (`WITH RECURSIVE name(c1, c2) AS ...`) → `[GBY-4081]` (mismo workaround que W1).

### 🧪 Validación

- 8 tests `w2_*` en `tests/integration_test.rs`: generador de números, dedup natural de UNION, max-iter guard, body no-UNION rechazado, multi-recursive rechazado, schema mismatch, JOIN desde el body, traversal de árbol.
- El test `w1_cte_recursive_rejected` se removió (la sintaxis pasó de rechazada a soportada); el código `4080` queda **retirado** pero reservado.
- Suite total: **395/395 pass** (`cargo test --lib --tests`).

---

## 2026-05-28 — Bloque W1: CTEs no-recursivas + fix residual Issue #3

> **Un push a `main`** con dos cambios coherentes y chicos: el bloque W1 (CTEs) y un fix del residual del Issue #3 del benchmark (`WHERE col = val` sobre col no-PK / no-indexada caía a FullScan sin post-filter y devolvía TODAS las filas — bug detectado mientras se escribían los tests de W1). Sin bump de formato.
>
> Detalle de la decisión de W1 y trade-offs en [`docs/adr/0026-cte-non-recursive.md`](docs/adr/0026-cte-non-recursive.md).

### 🆕 Sintaxis habilitada

- CTE simple: `WITH seniors AS (SELECT id FROM emp WHERE salario >= 100) SELECT id FROM seniors;`
- Múltiples CTEs encadenadas (CTE2 puede usar CTE1):
  - `WITH a AS (SELECT ...), b AS (SELECT FROM a WHERE ...) SELECT FROM b;`
- CTE en `JOIN`, en subqueries (`IN (SELECT FROM cte)`, `EXISTS (SELECT FROM cte)`, scalar subquery), y visible desde ambas ramas de un `UNION` / `INTERSECT` / `EXCEPT`.
- Shadowing ANSI: el nombre de la CTE prevalece sobre cualquier tabla real homónima del catálogo.

### 🛠 Implementación

- **Inlining post-parse como derived tables** (sin cambios en el executor): cada `FROM cte_name` se reescribe a `(SELECT ... FROM ...) AS cte_name` en el AST. Reusa `materialize_derived_table` del bloque H. Documentado en [ADR-0026 §1](docs/adr/0026-cte-non-recursive.md).
- Helpers libres `inline_cte_into_query` / `inline_cte_into_select` / `inline_cte_into_where` / `inline_cte_into_clause` / `inline_cte_into_expr` recorren el AST y substituyen referencias.
- Orden de recursión deliberado para evitar self-loops: primero descendemos en `derived_source` pre-existente, después instalamos el nuevo derived (que ya no se vuelve a procesar).

### 🚫 Diferido (con código de error explícito)

- `WITH RECURSIVE` → `[GBY-4080]` — bloque W2 (fixpoint base+step).
- `WITH cte(c1, c2) AS (...)` (column aliases en la cabecera) → `[GBY-4081]` — workaround: aliasar en el body (`SELECT x AS c1, y AS c2`).
- Nombres duplicados en el mismo `WITH` → `[GBY-4079]`.

### 🐞 Fix residual Issue #3 (`WHERE col = val` sin filtro)

El lifteo de `[GBY-4001]` (commit `49204ee`) hizo que `WHERE col = val` sobre columna no-PK no-indexada cayera a `Plan::FullScan` — pero el `generic_post_filter` no incluía `WhereClause::Eq` en su lista de "force", así que el scan retornaba TODAS las filas sin aplicar el filtro. Detectado escribiendo los tests de W1 (la CTE `WITH x AS (SELECT id FROM t WHERE v = 1)` devolvía 2 filas en vez de 1). Fix: extender el matcher de `generic_post_filter` para activar el post-filter cuando `Eq` cae a FullScan (col que no es PK simple y no está indexada). El test `secondary_index_lookup_and_maintenance` se actualizó para verificar el filtrado correcto (40 Anas exactas en lugar de "id=1 debe aparecer", que dependía del bug).

### 🧪 Validación

- 10 tests `w1_*` en `tests/integration_test.rs` cubriendo cada caso de uso + cada error code.
- 1 test de regresión `regression_eq_non_indexed_col_filters` para el residual Issue #3.
- Suite total: **388/388 pass** (`cargo test --lib --tests`).

---

## 2026-05-27 — Performance fixes: 5 issues del BENCHMARK resueltos

> **Un push a `main`** que resuelve 5 de los 6 issues identificados en la corrida pre-L+V del benchmark (Issue #2 queda diferido con error claro). Sin bump de formato. Reporte completo en [`BENCHMARK.md`](BENCHMARK.md).

### 🐞 Issues resueltos

| # | Sev | Issue | Antes | Después |
|---|---|---|---:|---:|
| **#1** | 🔴 | Scalar subquery no-correlacionada re-evaluada por fila | 7.55 s (LIMIT 10) | **3.34 s** (1 eval cached) |
| **#3** | 🟡 | `[GBY-4001]` rechazaba `WHERE col_no_idx = val` | error | full scan + post-filter |
| **#4** | 🟡 | Composite PK lookup degeneraba a full scan | 145 ms | **216 µs** (~670×) |
| **#5** | 🟢 | `parse_agg_arg` rechazaba aritméticos: `SUM(qty*price)` | parse error | **302 µs** sobre 100K rows |
| **#6** | 🟢 | JOIN sin hash-join (sólo nested-loop) | nested-loop O(N×M) | hash join O(N+M) cuando aplica |

### 🛠 Detalle de los fixes

- **#1 (memoización)**: `Engine::memoize_select_stmt` + `select_stmt_is_correlated` walker pre-evalúan toda `Expr::ScalarSubquery` no-correlated UNA vez y sustituyen el árbol con `Expr::Literal(value)`. Correlación se detecta vía `WhereClause::EqColumnRef` recursivo.
- **#3 (política WHERE)**: el branch `else` del planner `WhereClause::Eq` cae a `Plan::FullScan` igual que `>`, `<`, `LIKE`, `IS NULL`. `[GBY-4001]` queda como código reservado.
- **#4 (composite PK fast-path)**: `extract_and_equality_map` walker reconoce AND-of-equality que cubre toda la PK compuesta; activa `composite_pk_fast_path_active` antes del `generic_post_filter`, computa el fingerprint K2 y va directo al B+Tree.
- **#5 (AggArg::Expr)**: nueva variante `AggArg::Expr(Expr)`. `parse_agg_arg` delega a `parse_expr()` y colapsa a `AggArg::Column` cuando el resultado es un único `Column`. `compute_aggregate` pre-evalúa la Expr por row contra una key sintética y reusa el motor de agregación existente.
- **#6 (hash join)**: `exec_select_joined` ahora construye un `HashMap<Vec<u8>, Vec<usize>>` sobre la columna del lado right antes del loop, y probea cada left row en O(1). Sólo aplica a equi-joins; el bench actual no lo exhibe porque todos sus JOINs pegan al fast-path *index-loop* preexistente.

### 🐞 Issue diferido

- **#2 (CREATE INDEX bucket overflow)**: error de bucket-too-big ahora trae un mensaje claro indicando la causa (cardinalidad baja sobre datasets grandes) y workarounds. El fix real (overflow chain entre páginas del bucket) es un bloque propio. La corrida actual del bench muestra que el caso de 200K rows × 8 valores únicos × payload chico SÍ crea el índice — el bug original era condicional a payload por row más grande.

### 🆕 Sintaxis habilitada (Issue #5)

- `SUM(expr * expr)` y similar:
  - `SELECT order_id, SUM(qty * price) AS total FROM order_lines GROUP BY order_id`
  - `SELECT AVG(salary * 1.1) FROM employees`
  - Cualquier `Expr` (G1+G2+G3) como argumento de `SUM`/`AVG`/`MIN`/`MAX`/`COUNT`.

### 🧪 Tests

Tests sin cambio en cantidad: **377/377 verdes**. Las 6 modificaciones tocan rutas internas; la cobertura existente las ejercita transitivamente (composite PK lookups, aggregations, JOINs, scalar subqueries, WHERE sobre col no-indexada). `cargo fmt --check` + `clippy --all-targets -D warnings` limpios.

---

## 2026-05-27 — Bloque V: vistas lógicas (`CREATE VIEW` / `DROP VIEW`) (VERSION 12 → 13)

> **Un push a `main`** que abre el bloque V del roadmap (vistas) — primer mecanismo de abstracción semántica del motor. Bump VERSION 12→13 con discriminator byte para que tablas y vistas convivan en el catálogo. Ver [ADR-0025](docs/adr/0025-views.md).

### 🆕 Sintaxis

- `CREATE VIEW v AS SELECT ...`
- `CREATE VIEW IF NOT EXISTS v AS SELECT ...`
- `CREATE VIEW v (a, b) AS SELECT x, y FROM t` — aliases de columna (persistidos; aplicación efectiva queda como mejora futura).
- `DROP VIEW v`
- `DROP VIEW IF EXISTS v`

### 🛡️ Semántica

- **Vistas no son materializadas**: cada `SELECT FROM v` re-evalúa el SELECT subyacente contra el estado actual de la tabla base. Lo que ven los queries refleja inmediatamente cualquier INSERT/UPDATE/DELETE en la tabla base.
- **Read-only**: `INSERT`/`UPDATE`/`DELETE` sobre una vista rebota con `[GBY-4075] VIEW_NOT_WRITABLE`. Las vistas updatable (con `INSTEAD OF` triggers o auto-rewrite) quedan diferidas a un bloque dedicado.
- **Expansion via derived table**: en cualquier `FROM v` donde `v` sea una vista, el motor parsea el source SQL y lo embebe como derived source del FROM (reusando el path de H, `FROM (SELECT ...) AS d`).
- **Cycle guard**: `MAX_VIEW_DEPTH = 32`. Vistas mutuamente referenciadas (`A → B → A`) rebotan con `[GBY-4076]`.
- **Namespace compartido** con tablas: una vista no puede llamarse igual que una tabla (y viceversa) — `[GBY-4077] VIEW_NAME_COLLIDES_WITH_OBJECT`.

### 🚧 Limitaciones

- **Source del SELECT debe ser un SELECT simple**. Set operations (`UNION`/`INTERSECT`/`EXCEPT`) o `VALUES` como source rebotan con `[GBY-4078] VIEW_SOURCE_NOT_SIMPLE_SELECT`. Limitación del AST `derived_source: Option<Box<SelectStmt>>` — soportar set ops requiere extender el shape.
- **Vistas read-only**. Auto-updatable y `INSTEAD OF` triggers difieridos.
- **Materialized views**: no soportadas.
- **Aliases de columna**: la persistencia y validación de arity están; la sustitución efectiva de nombres queda como mejora futura.
- Migración V12 → V13: manual (dump SELECT + recreate).

### 🔧 Catálogo

- Cada record del catálogo arranca con `[kind:u8]` discriminator (`0=Table`, `1=View`).
- Nuevo `ViewMeta { name, source, column_aliases }`.
- API nueva: `Catalog::get_view`, `Catalog::put_view`, `Catalog::list_views`, `Catalog::list_objects`, `Catalog::get_object`, `Catalog::remove_object`. `list_tables` ahora filtra Views automáticamente.
- `parse_select_query_str(s) -> DbResult<SelectQuery>` expuesto en `gabysql::sql` para clientes que necesiten re-parsear el source de una vista.

### 🆕 Errores

- `[GBY-4075] VIEW_NOT_WRITABLE`
- `[GBY-4076] VIEW_EXPANSION_DEPTH_EXCEEDED`
- `[GBY-4077] VIEW_NAME_COLLIDES_WITH_OBJECT`
- `[GBY-4078] VIEW_SOURCE_NOT_SIMPLE_SELECT`

### 🗄️ Formato en disco — VERSION 13

```
Cada record del catálogo:
[kind:u8] · {
  kind == 0 (Table) → TableMeta serialize (V12 layout)
  kind == 1 (View)  → [name][source][alias_present:u8] · alias_present ?
                          [alias_count:u16] · alias × [alias_name] : ∅
}
```

### 🧪 Tests nuevos (14)

`v_create_view_and_select`, `v_view_reflects_base_table_changes`, `v_drop_view`, `v_drop_view_if_exists_noop`, `v_view_name_collides_with_table`, `v_view_if_not_exists_idempotent`, `v_insert_on_view_rejected`, `v_update_on_view_rejected`, `v_delete_on_view_rejected`, `v_view_with_aggregation`, `v_view_persists_across_reopen`, `v_view_referencing_view`, `v_create_view_with_set_op_source_rejected`, `v_v12_db_rejected_with_unsupported_version`.

Total integration tests: 377 (363 pre-V + 14 nuevos), todos verdes.

---

## 2026-05-27 — Residual #4 de L: activación real de `ON UPDATE` + UPDATE sobre PK

> **Un push a `main`** que cierra el residual #4 — el último listado tras #2/#3. Con este push, el **bloque L completo** (constraints) queda 100% entregado: CHECK, referential actions, naming, multi-col FK y ahora ON UPDATE activo. Sin bump de formato: el byte `on_update` ya estaba persistido desde L1.

### 🆕 Operaciones habilitadas

- `UPDATE t SET pk_col = <expr> WHERE ...` ahora **funciona**. Antes rebotaba con `[GBY-4008] UPDATE_PK_NOT_ALLOWED`; ahora el motor:
  - Computa el nuevo PK (single-col INT o compuesto fingerprint K2).
  - Verifica que no haya otra fila con ese PK (`[GBY-3001]` si la hay).
  - Dispara la acción `ON UPDATE` declarada en cada FK entrante.
  - Mueve la fila (`delete(old_pk)` + `insert(new_pk)`) y mantiene los índices secundarios.
- `INSERT ... ON CONFLICT DO UPDATE SET pk_col = ...` **sigue rebotando** con `[GBY-4008]` (intencional — el UPSERT identifica el row por la PK conflictiva, cambiarla rompe la semántica).

### 🛡️ Acciones `ON UPDATE` ahora activas

| Acción | Comportamiento |
|---|---|
| `CASCADE` | propaga los nuevos target values a todas las source cols de cada child que matcheaba el OLD value |
| `SET NULL` | mismo path; valida que ninguna source col del child sea NOT NULL (`[GBY-3009]` si lo es) |
| `SET DEFAULT` | reasigna cada source col al DEFAULT declarado; `[GBY-3010]` si falta DEFAULT, `[GBY-3002]` si DEFAULT NULL + NOT NULL |
| `RESTRICT` | rebota con `[GBY-4073]` antes de tocar disco |
| `NO ACTION` (default si se omite) | alias de RESTRICT en este release |

ON UPDATE es **no-op** si la columna target específica no cambió. Una `UPDATE parent SET label = ...` que no toca `id` (target de la FK) no dispara cascade.

### 🚧 Limitaciones

- **Cascade que afectaría la PK del child**: si una source col de la FK también participa en la PK del child (caso degenerado tipo `CREATE TABLE c (id INT PRIMARY KEY REFERENCES p (id) ON UPDATE CASCADE)`), el cascade quedaría encadenando moves indefinidamente. Rebota con `[GBY-4074] FK_UPDATE_CASCADE_AFFECTS_CHILD_PK`. Diferido a futuro.
- `INSERT ... ON CONFLICT DO UPDATE SET pk_col = ...` no soportado por las razones de arriba.
- Sin bump de formato — V12 sigue.

### 🆕 Errores

- `[GBY-4073] FK_RESTRICT_BLOCKS_UPDATE`
- `[GBY-4074] FK_UPDATE_CASCADE_AFFECTS_CHILD_PK`

### 🧪 Tests

3 tests legacy (`update_and_delete_by_pk_roundtrip`, `g2_update_set_pk_blocked`, `k2_pk_composite_update_pk_col_blocked`) que esperaban `[GBY-4008]` se reescribieron al nuevo contrato (`r4_*` los reemplaza con cobertura más amplia).

Nuevos (12): `r4_update_pk_single_moves_row`, `r4_update_pk_to_existing_value_rejected`, `r4_on_update_cascade_single_col`, `r4_on_update_set_null_single_col`, `r4_on_update_set_default_single_col`, `r4_on_update_restrict_blocks`, `r4_on_update_default_no_action_blocks_like_restrict`, `r4_on_update_cascade_multi_col`, `r4_on_update_no_op_when_target_unchanged`, `r4_cascade_affects_child_pk_rejected`, `r4_update_pk_maintains_secondary_index`, `r4_update_pk_in_upsert_still_blocked`.

Total integration tests: 363 (351 pre-#4 + 12 nuevos), todos verdes.

Decisiones y limitaciones en [ADR-0024](docs/adr/0024-on-update-activation.md).

---

## 2026-05-27 — Residual #3 de L: multi-column FOREIGN KEY (VERSION 11 → 12)

> **Un push a `main`** que cierra el residual #3 listado tras #2: `FOREIGN KEY (a, b) REFERENCES p (x, y)` con todos los `ON DELETE` (CASCADE, SET NULL, SET DEFAULT, RESTRICT, NO ACTION) y `ON UPDATE` persistido. Bump VERSION 11→12 con rechazo limpio de V11 vía `[GBY-1003]`. Ver [ADR-0023](docs/adr/0023-multi-col-foreign-key.md).

### 🆕 Sintaxis

- `CREATE TABLE child (..., CONSTRAINT fk_multi FOREIGN KEY (pa, pb) REFERENCES parent (a, b) ON DELETE CASCADE);`
- También sin nombre: `CREATE TABLE child (..., FOREIGN KEY (pa, pb) REFERENCES parent (a, b));`
- Mismo soporte `ON DELETE` / `ON UPDATE` del bloque L1 (RESTRICT, CASCADE, SET NULL, SET DEFAULT, NO ACTION).
- Multi-col FK column-inline **no** se soporta (ANSI tampoco lo permite — usar table-level).

### 🛡️ Semántica

- **Target = PK compuesta del parent**: las `target_columns` declaradas en `REFERENCES p (x, y)` deben matchear exactamente la PK del parent en el mismo orden. Otro UNIQUE arbitrario rebota al validar (limitación documentada).
- **Lookup O(log n) via fingerprint**: el motor computa `fp = encode_composite_key(parent_pk_cols, source_values)` (mismo encoder de K2) y hace `parent.get_row(fp)`. NULL en cualquier source → ANSI 3VL pasa sin chequear.
- **Cascade SET NULL / SET DEFAULT mutan todas las source cols atómicamente**: si cualquier source es NOT NULL bajo SET NULL → `[GBY-3009]`; si falta DEFAULT en alguna → `[GBY-3010]`. Sin rollback parcial.
- **Búsqueda de hijos en cascade**: single-col mantiene el fast-path por índice secundario (Hash/OrderedInt). Multi-col cae a full-scan comparando tuplas (igual que PostgreSQL cuando no hay índice por las source cols).

### 🚧 Limitaciones

- FK multi-col target **debe** ser la PK del parent (no UNIQUE arbitrario). Único caso práctico hoy.
- Sin fast-path indexado para cascade multi-col — `CREATE INDEX child (pa, pb)` no se usa automáticamente para acelerar el find del cascade. Mejora futura.
- `ALTER TABLE ADD FOREIGN KEY ... (a, b) REFERENCES ...` no implementado (igual que L3 con CHECK, requiere re-validar filas existentes). Diferido.
- Activación real de `ON UPDATE` sigue diferida al residual #4.
- Migración V11 → V12 manual: dump SELECT + recreate.

### 🔧 Catálogo

- `ForeignKeyMeta` añade `pub extra_source_columns: Vec<String>` y `pub extra_target_columns: Vec<String>`. Single-col → ambos vacíos.
- Helpers nuevos: `ForeignKeyMeta::source_columns(anchor)`, `::target_columns()`, `::is_composite()`.
- Helpers runtime: `fk_lookup_parent_pk`, `collect_fk_source_values`, `cascade_set_fk_tuple` (reemplaza el single-col `cascade_set_fk_value` como wrapper).

### 🗄️ Formato en disco — VERSION 12

```
[col_count:u16] · col × {
    [name][type:u8][flags:u8]
    flags & 0x02 ? DefaultLiteral : ∅
    flags & 0x04 ? [target_table][target_column]
                   [on_delete:u8][on_update:u8]
                   [fk_name_present:u8] · fk_name_present ? [fk_name] : ∅
                   [fk_extra_count:u8] · extra ×              ← NUEVO
                         [extra_source_col] · extra ×
                         [extra_target_col]
                   : ∅
}
```

### 🧪 Tests nuevos (12)

`r3_multi_col_fk_happy_path`, `r3_multi_col_fk_parent_missing_rejected`, `r3_multi_col_fk_delete_cascade`, `r3_multi_col_fk_delete_set_null`, `r3_multi_col_fk_set_null_rejected_when_any_col_not_null`, `r3_multi_col_fk_arity_mismatch_at_ddl`, `r3_multi_col_fk_target_must_be_pk`, `r3_multi_col_fk_null_source_passes_via_3vl`, `r3_multi_col_fk_drop_via_drop_constraint`, `r3_multi_col_fk_persists_across_reopen`, `r3_drop_column_blocked_by_multi_col_fk`, `r3_v11_db_rejected_with_unsupported_version`.

Total integration tests: 351 (339 pre-residual-#3 + 12 nuevos), todos verdes. El test `r2_multi_col_fk_rejected_with_clear_message` se actualizó a `r2_multi_col_fk_now_supported_post_r3` como smoke check.

---

## 2026-05-27 — Residual #2 de L: nombres en PK/UNIQUE/FK + `ALTER TABLE DROP CONSTRAINT` (VERSION 10 → 11)

> **Un push a `main`** que cierra el residual #2 listado tras L3: poder nombrar PK/UNIQUE/FK con `CONSTRAINT <name>` y borrarlos por nombre con `ALTER TABLE DROP CONSTRAINT`. Bump VERSION 10→11 con rechazo limpio de V10 vía `[GBY-1003]`. Ver [ADR-0022](docs/adr/0022-named-constraints-and-drop.md).

### 🆕 Sintaxis

- `CREATE TABLE t (..., CONSTRAINT pk_t PRIMARY KEY (id));`
- `CREATE TABLE t (..., CONSTRAINT uq_email UNIQUE (email));`
- `CREATE TABLE t (..., CONSTRAINT fk_t_parent FOREIGN KEY (parent_id) REFERENCES parent (id) ON DELETE CASCADE);` — single-col, multi-col rebota apuntando al residual #3.
- `ALTER TABLE t DROP CONSTRAINT <name>;`
- `ALTER TABLE t DROP CONSTRAINT IF EXISTS <name>;` — no-op silencioso si no existe.

### 🛡️ Semántica del DROP CONSTRAINT

Lookup case-insensitive en este orden:

1. **PK** → rechazo con `[GBY-4072] CANNOT_DROP_PRIMARY_KEY_CONSTRAINT` (PK inmutable).
2. **CHECK** → drop la entry del Vec.
3. **UNIQUE index** → si el índice existe pero NO es UNIQUE, rechazo con sugerencia de `DROP INDEX`.
4. **FK con nombre** → limpia `column.references` (la columna queda; deja de ser FK).
5. Sin match → `[GBY-4071] CONSTRAINT_NOT_FOUND` con breakdown de constraints visibles. Con `IF EXISTS`, no-op.

### 🚧 Limitaciones

- `CONSTRAINT <name> FOREIGN KEY (a, b) REFERENCES p (x, y)` rechazado con mensaje claro → diferido al residual #3.
- Nombrado inline en columna (`email TEXT CONSTRAINT uq_email UNIQUE`) no soportado todavía — sólo `CONSTRAINT name CHECK` inline desde L2.
- Migración V10 → V11 manual: dump SELECT + recreate.

### 🔧 Catálogo

- `TableMeta` añade `pub primary_key_name: Option<String>`.
- `ForeignKeyMeta` añade `pub name: Option<String>`.
- `IndexMeta.name` ya existía (V5+); ahora puede venir del usuario o del auto-naming (`uq_<table>_<cols>`).

### 🆕 Errores

- `[GBY-4071] CONSTRAINT_NOT_FOUND` — DROP CONSTRAINT con nombre desconocido.
- `[GBY-4072] CANNOT_DROP_PRIMARY_KEY_CONSTRAINT` — DROP CONSTRAINT sobre la PK.

### 🗄️ Formato en disco — VERSION 11

```
[name][pk_count:u8] · pk_count × [pk_col]
[pk_name_present:u8] · pk_name_present ? [pk_name] : ∅            ← NUEVO
[root_page:u32]
[col_count:u16] · col × {
    [name][type:u8][flags:u8]
    flags & 0x02 ? DefaultLiteral : ∅
    flags & 0x04 ? [target_table][target_column]
                   [on_delete:u8][on_update:u8]
                   [fk_name_present:u8] · fk_name_present ?
                         [fk_name] : ∅                            ← NUEVO
                   : ∅
}
[idx_count:u16] · idx × { … }
[check_count:u16] · check × { [name][source] }
```

### 🧪 Tests nuevos (10)

`r2_constraint_name_primary_key`, `r2_constraint_name_unique_and_drop`, `r2_constraint_name_foreign_key_and_drop`, `r2_drop_constraint_check`, `r2_drop_constraint_unknown_name_rejected`, `r2_drop_constraint_if_exists_no_op`, `r2_drop_constraint_non_unique_index_rejected`, `r2_named_constraints_persist_across_reopen`, `r2_multi_col_fk_rejected_with_clear_message`, `r2_v10_db_rejected_with_unsupported_version`.

Total integration tests: 339 (329 pre-residual-#2 + 10 nuevos), todos verdes.

---

## 2026-05-27 — L3 (residual de L): `ALTER TABLE ADD CHECK` con re-validación de filas

> **Un push a `main`** que cierra el sub-pendiente #1 listado tras L2: agregar un `CHECK (expr)` a una tabla **ya cargada con datos**. Sin bump de formato (V10 ya tiene el slot). Pequeño y autocontenido: ~280 líneas en `src/sql.rs` + 9 tests + docs.

### 🆕 Sintaxis

- `ALTER TABLE t ADD CHECK (qty > 0);`
- `ALTER TABLE t ADD CONSTRAINT qty_positiva CHECK (qty > 0);`
- El nombre, si se omite, se sintetiza como `<tabla>_check_<N>` con N empezando donde quedó el último CHECK declarado.

### 🛡️ Semántica

- **Re-valida todas las filas existentes** con un full-scan O(n) antes de tocar el catálogo. Cualquier fila que evalúe a `FALSE` aborta el ALTER entero con `[GBY-3008]` y la PK ofensiva en el mensaje — sin rollback parcial.
- 3VL ANSI: filas con NULL en alguna columna del predicado pasan (mismo contrato que CHECK en CREATE TABLE).
- Subqueries dentro del predicado se rechazan con `[GBY-4069]`.
- Columnas referenciadas inexistentes se rechazan con `[GBY-2002]`.
- Nombres duplicados (otro CHECK ya declarado con ese nombre) se rechazan al validar, antes del full-scan.

### 🚧 Limitaciones residuales

- `ALTER TABLE ADD COLUMN ... CHECK (...)` sigue rechazado (el CHECK inline en una columna nueva tendría que re-validar todas las filas para esa columna — `ALTER TABLE t ADD COLUMN x INT; ALTER TABLE t ADD CHECK (x > 0);` es el path soportado).
- `ALTER TABLE DROP CONSTRAINT <name>` no implementado todavía (queda en el residual #2 junto con nombres en PK/UNIQUE/FK).

### 🧪 Tests nuevos (9)

`l3_alter_add_check_validates_existing_rows_and_persists`, `l3_alter_add_check_rejects_when_existing_row_violates`, `l3_alter_add_constraint_name_check_persists_name`, `l3_alter_add_check_null_passes_via_3vl`, `l3_alter_add_check_rejects_unknown_column`, `l3_alter_add_check_rejects_subquery`, `l3_alter_add_check_duplicate_name_rejected`, `l3_alter_add_check_persists_across_reopen`, `l3_alter_add_column_with_inline_check_rejected_with_clear_message`.

Total integration tests: 329 (320 pre-L3 + 9 nuevos), todos verdes.

---

## 2026-05-27 — Bloque L2: CHECK (expr) constraints (VERSION 9 → 10)

> **Un push a `main`** que cierra el sub-bloque L2 del roadmap: constraints `CHECK (expr)` column-level y table-level con evaluación real en cada `INSERT`/`UPDATE`/`UPSERT DO UPDATE`. Bump VERSION 9 → 10 con rechazo limpio de DBs V9 vía `[GBY-1003]`. Ver [ADR-0021](docs/adr/0021-check-constraints.md). Con L2 cierra el bloque **L** completo del roadmap.

### 🆕 Sintaxis

- Column-level: `CREATE TABLE t (id INT PRIMARY KEY, age INT CHECK (age >= 0));`
- Table-level: `CREATE TABLE r (id INT PRIMARY KEY, lo INT, hi INT, CHECK (lo <= hi));`
- Con nombre: `CONSTRAINT edad_positiva CHECK (edad > 0)` — el nombre aparece en `[GBY-3008]` para diagnóstico claro.
- CHECKs sin nombre se materializan como `<tabla>_check_<N>` (N empieza en 1, monotónico).
- Soporta cualquier `Expr` re-parseable: comparadores, `AND`/`OR`/`NOT`, `BETWEEN`, `IN (...)`, `LIKE`, `IS NULL`, escalares (`LENGTH`, `UPPER`, fecha, aritméticos, `CAST`, `CASE WHEN`).

### 🛡️ Semántica

- **3VL ANSI**: si el predicado evalúa a `NULL`, la fila pasa. Sólo `FALSE` rebota con `[GBY-3008]`. Mismo comportamiento que PostgreSQL y SQLite.
- Se evalúa **en cada write**: `INSERT`, `UPDATE`, `UPSERT DO UPDATE`, `ON CONFLICT DO UPDATE`, y también dentro de `cascade_set_fk_value` (SET NULL/SET DEFAULT pueden no satisfacer un CHECK del child).
- Sin rollback parcial: si el CHECK falla, la operación entera rebota antes de tocar disco.

### 🚧 Limitaciones L2 (explícitas)

- **Subqueries prohibidas** dentro de CHECK (`(SELECT ...)`). Falla en DDL con `[GBY-4069]`. Es la regla ANSI y simplifica la cosecha de stats.
- **Sin `ALTER TABLE ADD CHECK`**: agregar un CHECK a una tabla existente requeriría re-validar todas las filas. Se difiere.
- **Sin column-level CHECK en `ALTER TABLE ADD COLUMN`**: misma razón (el `parse_column_def` lo rechaza explícitamente).
- Sólo CHECKs con nombre en table-level y column-level. **PK/UNIQUE/FK con nombre** (`CONSTRAINT name PRIMARY KEY (...)`) no soportado todavía — el parser sólo entiende `CONSTRAINT name CHECK (...)`.
- Migración V9 → V10 es manual: dump SELECT + recreate con binario L2.

### 🔧 Catálogo

- Nuevo `CheckConstraint { name: String, source: String }` con el SQL canónico del predicado (re-formateado por `format_expr`).
- `TableMeta` añade `pub check_constraints: Vec<CheckConstraint>`.
- Decisión de diseño: persistimos **texto canónico** (no AST). Razones en ADR-0021 § Decisión.

### 🔤 Round-trip Expr ↔ texto

- `gabysql::sql::format_expr(&Expr) -> DbResult<String>`: serializa el AST a SQL canónico, envuelve binarios en paréntesis para precedencia neutra, rechaza `ScalarSubquery`.
- `gabysql::sql::parse_expr_str(&str) -> DbResult<Expr>`: contraparte — re-construye el AST desde catálogo.
- DDL pre-validación: cada CHECK roundtrip-tea (parse → format → parse) y se rechazan refs a columnas inexistentes con `[GBY-2002]`.

### 🆕 Errores

- `[GBY-3008] CHECK_VIOLATED` (estaba reservado en L1; ahora live).
- `[GBY-4069] CHECK_CONTAINS_SUBQUERY`
- `[GBY-4070] CHECK_EXPR_NOT_BOOLEAN` (reservado para el caso `CHECK (LENGTH(x))` sin comparar; hoy el evaluador rebota con el código genérico del eval, pero el código del catálogo está disponible para futuras validaciones DDL).

### 🗄️ Formato en disco — VERSION 10

```
[name][pk_count:u8] · pk_count × [pk_col]
[root_page:u32]
[col_count:u16] · col × { … }
[idx_count:u16] · idx × { … }
[check_count:u16] · check × { [name][source] }       ← L2 añade trailer
```

### 🧪 Tests nuevos (10)

`l2_check_column_level_rejects_violation_on_insert`, `l2_check_column_level_allows_null_via_3vl`, `l2_check_table_level_multi_col`, `l2_check_with_scalar_function`, `l2_check_violated_on_update`, `l2_named_check_constraint_roundtrips`, `l2_check_rejects_unknown_column_at_ddl`, `l2_check_rejects_subquery_at_ddl`, `l2_check_persists_across_reopen`, `l2_v9_db_rejected_with_unsupported_version`.

Total integration tests: 320 (310 pre-L2 + 10 nuevos), todos verdes.

---

## 2026-05-27 — Bloque L1: FK referential actions + UNIQUE multi-col table-level (VERSION 8 → 9)

> **Un push a `main`** que cierra el sub-bloque L1 del roadmap: extender las acciones referenciales de `FOREIGN KEY` y exponer `UNIQUE (a, b, ...)` table-level. Bump VERSION 8 → 9 con rechazo limpio de DBs V8 vía `[GBY-1003]`. Ver [ADR-0020](docs/adr/0020-fk-referential-actions.md) para la decisión y limitaciones. El sub-bloque L2 (`CHECK (expr)`) queda diferido a una entrega aparte.

### 🆕 Sintaxis

- `FOREIGN KEY` extendido:
  - `REFERENCES t(c) ON DELETE SET NULL` — pone NULL en el child (`[GBY-3009]` si la columna es NOT NULL).
  - `REFERENCES t(c) ON DELETE SET DEFAULT` — reasigna al DEFAULT declarado (`[GBY-3010]` si no hay DEFAULT).
  - `REFERENCES t(c) ON DELETE NO ACTION` — alias de `RESTRICT` en este release.
  - `REFERENCES t(c) ON UPDATE <action>` — acepta `RESTRICT | CASCADE | SET NULL | SET DEFAULT | NO ACTION`; **se persiste pero no se dispara hoy** (la PK del padre es inmutable por `[GBY-4008]`).
  - `ON DELETE` y `ON UPDATE` en cualquier orden (`ON UPDATE … ON DELETE …` tan válido como el revés).
- `UNIQUE (a, b, ...)` declarada a nivel de tabla en `CREATE TABLE`:
  - `CREATE TABLE t (id INT PRIMARY KEY, a INT NOT NULL, b INT NOT NULL, UNIQUE (a, b));`
  - Materializa el mismo índice UNIQUE compuesto que `CREATE UNIQUE INDEX … ON t (a, b)` (K2). Single-col `UNIQUE (col)` también admitido como alias de UNIQUE inline.

### 🛡️ Enforcement composite UNIQUE (parche K2)

K2 entregó composite UNIQUE INDEX pero el INSERT/UPDATE/DELETE chequeaba sólo el bucket de la primera columna. L1 cierra el hueco: el path de write usa el fingerprint FNV-1a-64 completo. Helpers nuevos en `sql.rs`:

- `composite_fp_for_values(meta, idx, values) -> i64`
- `composite_unique_check(pager, idx, fp, exclude_pk)`
- `composite_index_upsert(pager, root, fp, pk)`
- `composite_index_remove(pager, root, fp, pk)`

### 🚧 Limitaciones L1 (explícitas)

- `ON UPDATE` se persiste pero **nunca dispara** — `UPDATE` sobre la PK del padre sigue rebotando con `[GBY-4008]`. El byte queda en disco para que un release futuro lo active sin otro bump.
- `SET NULL` requiere que la columna FK del child admita NULL. Con `NOT NULL` se aborta con `[GBY-3009]` (sin rollback parcial: la validación es pre-write).
- `SET DEFAULT` exige DEFAULT declarado. Sin él, `[GBY-3010]`. Con DEFAULT NULL y columna NOT NULL, `[GBY-3002]`.
- `CHECK (expr)` queda diferido al **sub-bloque L2** (CHECK column-level y table-level, eval en INSERT/UPDATE/UPSERT).
- Multi-column FK sigue fuera de scope (K2 limitation).
- Migración V8 → V9 es manual: dump SELECT + recreate con binario L1.

### 🔧 Catálogo

- `OnDelete` extiende su rango de códigos: `0=Restrict, 1=Cascade, 2=SetNull, 3=SetDefault`.
- `OnUpdate` nuevo enum: `0=NoAction, 1=Cascade, 2=SetNull, 3=SetDefault, 4=Restrict`.
- `ForeignKeyMeta` añade `pub on_update: OnUpdate`.
- En disco: cada FK record gana un byte `[on_update:u8]` a continuación de `[on_delete:u8]`.

### 🆕 Errores

- `[GBY-3009] FK_SET_NULL_VIOLATES_NOT_NULL`
- `[GBY-3010] FK_SET_DEFAULT_MISSING`
- (Reservado para L2) `[GBY-3008] CHECK_VIOLATED` — el código entra al catálogo en L1 para no romper la numeración cuando L2 cierre.

### 🗄️ Formato en disco — VERSION 9

```
[name][pk_count:u8] · pk_count × [pk_col]
[root_page:u32]
[col_count:u16] · col × {
    [name][type:u8][flags:u8]
    flags & 0x02 ? DefaultLiteral : ∅
    flags & 0x04 ? [target_table][target_column]
                   [on_delete:u8][on_update:u8] : ∅       ← L1 añade on_update
}
[idx_count:u16] · idx × { … }
```

### 🧪 Tests nuevos (10)

`l1_fk_on_delete_set_null_sets_child_to_null`, `l1_fk_on_delete_set_null_rejects_when_child_col_not_null`, `l1_fk_on_delete_set_default_uses_declared_default`, `l1_fk_on_delete_set_default_rejects_when_no_default`, `l1_fk_no_action_is_alias_of_restrict`, `l1_fk_on_update_parsed_and_roundtrips`, `l1_fk_on_update_after_on_delete_in_any_order`, `l1_unique_multi_column_table_level_rejects_duplicate_combo`, `l1_unique_single_column_table_level_works`, `l1_v8_db_rejected_with_unsupported_version`.

---

## 2026-05-26 — Bloque K2: PK compuesta + índices compuestos (VERSION 7 → 8)

> **Un push a `main`** que cierra el sub-bloque K2 del roadmap: el DDL que **sí** cambia el formato en disco. Habilita `PRIMARY KEY (a, b, ...)` (table-level) y `CREATE [UNIQUE] INDEX idx ON t (a, b, ...)`. Bump VERSION 7 → 8 con rechazo limpio de DBs viejas vía `[GBY-1003]`. Ver [ADR-0019](docs/adr/0019-composite-pk-and-index.md) para la decisión y limitaciones.

### 🆕 Sintaxis

- `CREATE TABLE asistencias (curso INT NOT NULL, alumno INT NOT NULL, presente BOOL, PRIMARY KEY (curso, alumno));`
- `CREATE TABLE t (id INT NOT NULL, v INT, PRIMARY KEY (id));` — PK table-level single-col también soportada (estilo opcional).
- `CREATE INDEX idx_ab ON t (a, b);`
- `CREATE UNIQUE INDEX uq_year_month ON ventas (year, month);`

### 🚧 Limitaciones K2 (explícitas)

- PK e índices compuestos **restringidos a all-INT NOT NULL** (`[GBY-4064]` / `[GBY-4067]`). El fingerprint i64 no representa NULL ni tipos no-INT.
- **No partial lookup indexado**: `WHERE a = 1` contra PK `(a, b)` cae a full-scan (resultado correcto, sin error, sin fast-path).
- **No range scan compuesto**: el fingerprint FNV-1a-64 no es order-preserving.
- **FK siguen single-column**: las relaciones multi-col se modelan vía surrogate INT + UNIQUE compuesta.
- **ALTER PK queda fuera** (creación nueva sí; ALTER no).
- **Migración V7 → V8 es manual**: hacer backup, recrear con binario nuevo, dump + INSERT.

### 🔧 Catálogo aditivo

- `TableMeta` agrega `primary_key_extra: Vec<String>` (vacío para PK single).
- `IndexMeta` agrega `extra_columns: Vec<String>` (vacío para single-column).
- Helpers nuevos: `TableMeta::pk_columns()`, `has_composite_pk()`, `is_pk_column(name)`; `IndexMeta::all_columns()`, `is_composite()`.

### 🔢 Fingerprint compuesto

- `src/index.rs::encode_composite_key(columns, values) -> i64` — FNV-1a-64 sobre `encode_column_value()` de cada par + sentinela `0xFF` entre columnas.

### 🗄️ Formato en disco — VERSION 8

```text
TableMeta:
  [name][pk_count:u8][pk_col_name × pk_count][root_page:u32]
  [col_count:u16] × { [name][type_code:u8][flags:u8] (DEFAULT)? (FK)? }
  [idx_count:u16] × {
    [name][column][root_page:u32][unique:u8][kind:u8]
    [extra_cols_count:u8][extra_col_name × extra_cols_count]
  }
```

VERSION 7 se rechaza al abrir con `[GBY-1003] UNSUPPORTED_FORMAT_VERSION` y mensaje que sugiere backup + dump + recreate.

### 🚦 Executor

- `exec_create_table`: el parser entrega `primary_key_extra` desde el table-level `PRIMARY KEY (...)`; el validator (`validate_create_table` en `catalog.rs`) verifica all-INT + NOT NULL en cada columna PK cuando es compuesta.
- `exec_create_index`: para índices compuestos verifica all-INT (`[GBY-4067]`), backfilea con `encode_composite_key` + ordered bucket layout (`[u16:count] + count × pk:i64`), detecta UNIQUE conflicts por fingerprint (`[GBY-3003]`). Publica con `IndexKind::OrderedInt` para reutilizar el decoder de INTEGRITY CHECK.
- `encode_row`: cuando `meta.has_composite_pk()` computa el fingerprint sobre todas las columnas PK; NULL en cualquiera → `[GBY-3007] PRIMARY_KEY_NULL`.
- UPDATE bloquea CUALQUIER columna PK → `[GBY-4008] UPDATE_PK_NOT_ALLOWED` con mensaje que enumera todas las columnas PK.
- Planner del WHERE: PK compuesta + WHERE sobre columna PK → fuerza `Plan::FullScan` + `generic_post_filter` (correcto via 3VL).

### 🆔 Códigos de error nuevos

- `4064 COMPOSITE_PK_REQUIRES_ALL_INT`
- `4065 PRIMARY_KEY_DUPLICATED`
- `4066 FK_TARGET_NOT_INDEXED` (reservado)
- `4067 COMPOSITE_INDEX_REQUIRES_ALL_INT`
- `4068 PARTIAL_KEY_LOOKUP_UNSUPPORTED` (reservado)

### 🧪 Tests

17 nuevos `k2_*`. `cargo fmt --check` ✅ · `cargo clippy --all-targets -- -D warnings` ✅ · `cargo test --all-targets` → **300 passed, 0 failed** (283 prior + 17 k2_*).

---

## 2026-05-26 — Bloque K1: DDL extendido (CTAS, RENAME, DROP/RENAME COLUMN)

> **Un push a `main`** que cierra el sub-bloque K1 del roadmap (`docs/MISSING_COMMANDS.md` §9): DDL faltante que **no** cambia el formato en disco (VERSION sigue en 7). Cubre `CREATE TABLE [IF NOT EXISTS] [(col_aliases)] AS <select>` (CTAS), `RENAME TABLE` / `ALTER TABLE RENAME TO`, `ALTER TABLE DROP COLUMN [IF EXISTS]` y `ALTER TABLE RENAME COLUMN`. La parte de DDL que sí tocaría el formato on-disk (PK compuesta, índices compuestos, partial indexes, `ALTER COLUMN TYPE`) queda para K2.

### 🆕 Sintaxis
- `CREATE TABLE [IF NOT EXISTS] dst AS SELECT id, ... FROM src [WHERE ...];` — la fuente puede ser cualquier `SelectQuery` (SELECT puro, set ops, VALUES).
- `CREATE TABLE dst (pk, label, score) AS SELECT id, nombre, total FROM src;` — alias de columnas opcionales; arity debe matchear.
- `RENAME TABLE old TO new;` y `ALTER TABLE old RENAME TO new;` — equivalentes.
- `ALTER TABLE t DROP COLUMN [IF EXISTS] col;` — la palabra `COLUMN` es obligatoria (para no colisionar con futuros `DROP CONSTRAINT`).
- `ALTER TABLE t RENAME COLUMN old TO new;` — arrastra el cambio a PK / índices / FKs entrantes.

### 🔧 AST + Parser
- Nuevas variantes en `Statement`: `CreateTableAs(CreateTableAsStmt)`, `RenameTable(RenameTableStmt)`, `AlterTableDropColumn(AlterDropColumnStmt)`, `AlterTableRenameColumn(AlterRenameColumnStmt)`.
- `parse_create` ahora reconoce `IF NOT EXISTS` y distingue CTAS de la forma clásica vía lookahead: tras `(` snapshotea `self.pos`, intenta consumir una lista de idents simples seguida de `)` + `AS` y, si no matchea, rollback al snapshot y cae al path tradicional (`col TIPO constraints, ...`).
- `parse_alter` se generaliza para `ADD [COLUMN]` (path histórico) / `DROP COLUMN [IF EXISTS]` / `RENAME TO` / `RENAME COLUMN`. `parse_statement` reconoce el top-level `RENAME TABLE` (alias estilo MySQL).
- Helpers: `try_parse_ctas_column_aliases` (lookahead lista de idents simples), `parse_select_query_for_ctas` (reusa el árbol del bloque I).

### 🚦 Executor
- `exec_create_table_as`: materializa la fuente con `exec_select_query`, valida arity de los alias (`[GBY-4063]`), valida ident y dedup de los headers, infiere tipos por columna (mismo variant en todos los no-NULL → ese tipo; INT+FLOAT promueven; mezcla → TEXT fallback), exige primera columna INT no-NULL como PK (`[GBY-4058]`), detecta duplicados de PK temprano (`[GBY-3001]`), crea la root_page, publica en el catálogo y rellena fila a fila vía `encode_row` + `Catalog::insert_row`. Toda la operación corre dentro de la transacción del batch — si algo falla, el wrap externo hace rollback.
- `exec_rename_table`: valida ident del nuevo nombre, exige que el origen exista (`[GBY-2001]`) y el destino no (`[GBY-4062]`), borra la entry vieja del catálogo + publica la nueva (FNV-1a-64 sobre el nuevo nombre), y barre la lista de tablas re-escribiendo los `ForeignKeyMeta::table` que apuntaban al nombre viejo.
- `exec_alter_drop_column`: chequea existencia (con respeto de `IF EXISTS`), bloquea sobre PK (`[GBY-4059]`), columnas indexadas (`[GBY-4060]`, mensaje sugiere `DROP INDEX <name>`), FKs salientes y entrantes (`[GBY-4061]`); luego full-scan de filas, decode con la meta vieja, remove de la columna del HashMap, re-encode con la meta nueva y `upsert_row` (mismo patrón que `ALTER TABLE ADD COLUMN`).
- `exec_alter_rename_column`: valida ident destino, exige existencia del origen y no-existencia del destino (`[GBY-4062]`); como el on-disk row es posicional, no requiere rewrite — sólo muta `TableMeta.columns[i].name`, `primary_key` (si la columna renombrada era la PK), `IndexMeta::column` y los `ForeignKeyMeta::column` entrantes de otras tablas.

### ⚠️ Limitaciones residuales (cierra K1; abre K2)
- **PK compuesta** y **índices compuestos** quedan para K2 — requieren un encoder multi-columna y bump VERSION 7→8 + ADR.
- **`ALTER COLUMN TYPE`** queda para K2 — requiere rewrite tipado con compatibilidad de defaults.
- **CTAS sin `id INT`**: el motor exige que la primera columna del SELECT sirva como PK (única estrategia compatible con la limitación de PK escalar INT). Sin esa columna, error `[GBY-4058]` explícito. El usuario tiene dos workarounds: (a) anteponer `id INT` en el SELECT, (b) usar la forma con alias `CREATE TABLE t (id, ...) AS SELECT 1, ...`.
- **CTAS con result-set vacío**: rechazado con `[GBY-4058]` — sin filas no se puede confirmar que la primera columna sea INT. Trabajar con `LIMIT 0` no es portable a esquemas en blanco; usar `CREATE TABLE ... (id INT PRIMARY KEY, ...)` clásico.
- **CTAS no hereda DEFAULT/NOT NULL/UNIQUE/FK** del origen: la nueva tabla queda con sólo la PK INT NOT NULL. Si el usuario los necesita, hay que recrear el esquema con DDL clásico + `INSERT INTO ... SELECT ...`.
- **DROP COLUMN sobre la única columna no-PK** está permitido (la tabla queda con sólo PK; estado válido).
- **DROP TABLE ... CASCADE** sigue pendiente (P2).

### 🧰 Códigos de error nuevos
- `4058` `CTAS_REQUIRES_INT_FIRST_COLUMN` — CTAS cuya primera columna no es INT no-NULL.
- `4059` `CANNOT_DROP_PRIMARY_KEY` — DROP COLUMN sobre la PK.
- `4060` `CANNOT_DROP_INDEXED_COLUMN` — DROP COLUMN sobre una columna con índice (mensaje sugiere `DROP INDEX`).
- `4061` `CANNOT_DROP_REFERENCED_COLUMN` — DROP COLUMN sobre una columna con FK saliente o entrante.
- `4062` `RENAME_TARGET_EXISTS` — RENAME TABLE / RENAME COLUMN cuyo destino ya existe.
- `4063` `CTAS_COLUMN_ALIAS_ARITY` — `CREATE TABLE t (alias_list) AS SELECT ...` con arity de aliases ≠ arity del SELECT.

### 🧪 Validación
- 28 tests nuevos `k1_*` cubriendo: CTAS basic / con WHERE / con column-aliases / arity-mismatch / desde set op / desde VALUES / primera col no-INT (4058) / result-set vacío (4058) / `IF NOT EXISTS` no-op / destino tomado (2004) / GROUP BY con primera col TEXT (4058); RENAME TABLE basic / via `ALTER TABLE RENAME TO` / destino tomado (4062) / origen ausente (2001) / FKs entrantes actualizadas; DROP COLUMN basic / `IF EXISTS` no-op / faltante sin IF EXISTS (2002) / PK (4059) / indexada (4060 con sugerencia DROP INDEX) / FK local (4061) / round-trip de datos en columnas restantes; RENAME COLUMN basic / destino tomado (4062) / origen ausente (2002) / sobre PK / sobre columna indexada.
- 283 tests integración total (255 pre-K1 + 28 K1), `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` limpios.

### 🗂️ Formato en disco
- `VERSION = 7` sin cambios. K1 no introduce ningún campo nuevo en `TableMeta`, `Column`, `IndexMeta` ni `ForeignKeyMeta`. DBs creadas con un binario pre-K1 abren sin migración y viceversa.

---

## 2026-05-26 — Bloque I: UNION / INTERSECT / EXCEPT + VALUES como tabla

> **Un push a `main`** que cierra el bloque I del roadmap (`docs/MISSING_COMMANDS.md` §5): operaciones de conjunto entre queries (`UNION` / `UNION ALL`, `INTERSECT` / `INTERSECT ALL`, `EXCEPT` / `EXCEPT ALL` con alias `MINUS`), y `VALUES (...), (...)` usable tanto como statement standalone (`VALUES (1,'a'), (2,'b');` devuelve un ResultSet) como tabla virtual dentro del FROM (`FROM (VALUES (1,'a'), (2,'b')) AS t(c1, c2)`).

### 🆕 Sintaxis
- `SELECT ... UNION [ALL] SELECT ...` — append/dedup de queries con la misma arity.
- `SELECT ... INTERSECT [ALL] SELECT ...` — filas presentes en ambos lados.
- `SELECT ... EXCEPT [ALL] SELECT ...` (alias `MINUS`) — filas del LHS no presentes en el RHS.
- Precedencia ANSI: `INTERSECT` ata más fuerte que `UNION` / `EXCEPT`; los tres son asociativos a izquierda.
- `(SELECT ...) UNION (SELECT ...) ORDER BY col LIMIT n OFFSET m` — ORDER BY / LIMIT / OFFSET al nivel del resultado combinado.
- `VALUES (1, 'a'), (2, 'b');` — statement standalone, devuelve ResultSet con headers `column1`, `column2`, ....
- `SELECT * FROM (VALUES (1, 'a'), (2, 'b')) AS t(id, name)` — tabla virtual literal en el FROM o como RHS de un JOIN (alias de tabla **y** lista de columnas obligatorios).

### 🔧 AST + Parser
- Nuevo enum `SelectQuery { Select(Box<SelectStmt>) | SetOp { lhs, op, all, rhs, order_by, limit, offset } | Values(ValuesClause) }`. `Statement::Select(SelectStmt)` pasa a `Statement::Select(Box<SelectQuery>)` (boxed para que el enum `Statement` no infle por culpa del variant más grande). El path `Select(stmt)` envuelve trivialmente el SelectStmt clásico — todos los call-sites pre-I siguen funcionando vía wrap/unwrap.
- Nuevo enum `SetOpKind { Union, Intersect, Except }` y struct `ValuesClause { rows: Vec<Vec<Expr>> }`.
- `SelectStmt` suma `values_source: Option<(Box<ValuesClause>, Vec<String>)>` para la forma `FROM (VALUES ...) AS t(c1, c2, ...)` como base table; `TableRef` suma `values` + `values_columns` para la forma equivalente en el RHS de un JOIN.
- Parser de set ops: `parse_set_ops_after` (nivel UNION/EXCEPT) → `parse_intersect_after` (sub-nivel INTERSECT, más alta precedencia) → `parse_select_term` (SELECT plano, VALUES, o `(SELECT|VALUES ...)` con sub-árbol). El `ORDER BY` / `LIMIT` / `OFFSET` que sigue al árbol top-level se cuelga del nodo `SetOp`.
- `parse_select_stmt_inner(allow_trailing_order_limit: bool)` — variante interna usada por `parse_select_term` cuando parsea un SELECT sin paréntesis envolventes dentro de un árbol de set ops: el ORDER BY/LIMIT trailing pertenece al outer, no al SELECT.
- `is_post_table_keyword` / `is_select_terminator_keyword` reconocen ahora `UNION` / `INTERSECT` / `EXCEPT` / `MINUS` como cortes del cuerpo del SELECT.

### 🚦 Executor
- `Engine::exec_select_query(SelectQuery)` despacha: `Select(stmt)` al path clásico `exec_select`, `Values(v)` a `exec_values_clause`, `SetOp { ... }` ejecuta ambos lados, llama a `combine_set_op` y aplica ORDER BY / LIMIT / OFFSET sobre el resultset combinado.
- `combine_set_op` valida arity (`[GBY-4054]`) y compatibilidad de tipos columna a columna (`[GBY-4055]` — INT/FLOAT promueven, otros tipos exigen match o NULL); usa `encode_group_key` (de F) para hashear filas y construir multisets con counts; aplica las reglas ANSI de bag-semantics: `UNION ALL` suma counts, `UNION` dedup; `INTERSECT ALL` toma `min(count_l, count_r)`, sin ALL devuelve 1; `EXCEPT ALL` toma `max(0, count_l - count_r)`, sin ALL devuelve 1 si la fila no está en el RHS.
- VALUES en FROM se materializa con `materialize_values_in_from` (mismo patrón que derived tables): infiere tipo por columna sobre los no-NULL, arma un `TableMeta` virtual sin storage, y delega al `JoinScope` igual que un derived. Sin alias de tabla → `[GBY-4052]`; sin lista de columnas o arity distinta → `[GBY-4053]`.
- `apply_order_by_on_resultset` resuelve el ORDER BY top-level por nombre (case-insensitive) contra los headers del resultset combinado (que son los del LHS — regla ANSI); falta de columna → `[GBY-2002]`. NULLs van al final igual que el ORDER BY pre-I.

### ⚠️ Limitaciones residuales (futuros bloques)
- `WITH ... AS (...)` / CTE — bloque W (planificado aparte).
- Set ops dentro de `UPDATE` / `DELETE` — no es estándar ANSI; no se planea.
- `INSERT INTO t (cols) VALUES (...), (...)` ya existía pre-I (bloque J multi-row); el VALUES de I es la forma standalone / FROM y no toca el path INSERT.
- `ALL`/`ANY`/`SOME` sobre subqueries (`col > ALL (SELECT ...)`) — backlog H-P2.
- `VALUES (...), (...) ORDER BY 1` con referencia posicional al ordinal — actualmente ORDER BY exige nombre.

### 🧰 Códigos de error nuevos
- `4052` `VALUES_IN_FROM_REQUIRES_ALIAS` — `FROM (VALUES ...)` sin alias de tabla / sin lista de columnas.
- `4053` `VALUES_COLUMN_ALIAS_ARITY` — arity de `t(c1, c2, ...)` no coincide con las filas de VALUES.
- `4054` `SET_OP_ARITY_MISMATCH` — `UNION` / `INTERSECT` / `EXCEPT` entre queries con distinto número de columnas.
- `4055` `SET_OP_TYPE_MISMATCH` — tipos incompatibles entre las columnas del LHS y del RHS de un set op.
- `4056` `VALUES_ROW_ARITY_MISMATCH` — dos filas del mismo `VALUES` con distinta arity.
- `4057` `VALUES_EMPTY` — `VALUES` sin filas.

### 🧪 Validación
- 22 tests nuevos `i_*` cubriendo: UNION basic/dedup/ALL/three-way/null-dedup/headers-from-lhs; UNION con ORDER BY y LIMIT a nivel top; UNION arity/type mismatch; INTERSECT basic e INTERSECT ALL counts; EXCEPT basic y EXCEPT ALL counts; alias `MINUS`; VALUES standalone / arity mismatch / empty; VALUES en FROM básico / JOIN con tabla persistente / alias requerido / arity de aliases; precedencia (`INTERSECT` ata más fuerte que `UNION`).
- 255 tests integración total (233 pre-I + 22 I), `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` limpios.

---

## 2026-05-26 — Bloque H: derived tables + NOT IN + scalar subquery in SELECT + multi-predicate correlated

> **Un push a `main`** que cierra los P0 + P1 del bloque H del roadmap (`docs/MISSING_COMMANDS.md` §4): derived tables `FROM (SELECT ...) AS alias`, `WHERE col NOT IN (SELECT ...)` con semántica ANSI 3VL, subqueries escalares en SELECT list (`SELECT id, (SELECT COUNT(*) FROM other) FROM t`), y `EXISTS` correlacionado dentro de combinadores `AND`/`OR`/`NOT` (levanta el bloqueo histórico `[GBY-4024]`).

### 🆕 Sintaxis
- `FROM (SELECT ...) AS sub` — derived tables (inline views) en el FROM o en el RHS de un JOIN; alias obligatorio (ANSI estricto, `[GBY-4048]`).
- `WHERE col NOT IN (SELECT ...)` — first-class. Con NULL en la subquery devuelve NULL (3VL ANSI estricta: `5 NOT IN (1, NULL)` → NULL).
- `SELECT id, (SELECT MAX(x) FROM t WHERE t.fk = outer.id) AS m FROM outer` — subquery escalar correlacionada en el SELECT list.
- `WHERE EXISTS (...) AND otra_col = N`, `WHERE NOT EXISTS (...) OR ...`, `WHERE EXISTS (...) AND EXISTS (...)` — combinaciones correlated multi-predicado.

### 🔧 AST + Parser
- `SelectStmt` suma `derived_source: Option<Box<SelectStmt>>`; cuando es `Some`, `table` lleva el alias y la subquery se materializa antes del scan.
- `TableRef` suma `derived: Option<Box<SelectStmt>>` para soportar derived en JOINs.
- `WhereClause::In` suma `negated: bool` — el parser construye `negated=true` cuando ve `NOT IN (SELECT ...)`.
- `Expr` suma `ScalarSubquery(Box<SelectStmt>)`. `parse_expr_primary` detecta `(` seguido de `SELECT` y la consume como subquery escalar.
- Helpers `expr_contains_subquery` (walker) y `Engine::eval_expr_full` (evaluator engine-aware) — el caller usa fast-path `eval_expr` cuando el árbol no contiene subqueries (zero overhead) y delega al engine cuando sí.

### 🚦 Executor
- `Engine::materialize_derived_table` ejecuta la subquery del derived, infiere tipo por columna (mismo variant en todos los no-NULL → ese tipo; mezcla → TEXT fallback) y construye un `TableMeta` virtual + filas decodificadas. Nombres duplicados → `[GBY-4049]`.
- `JoinTable` suma `virtual_rows: Option<Vec<HashMap<String, Value>>>`; `scan_qualified` las devuelve directamente sin hit al pager. `plan_index_loop` rechaza derived (no hay PK/índice real).
- `exec_select` despacha al JOIN path cuando hay `derived_source` (aunque no haya JOINs explícitos) — reusa todo el pipeline materializado.
- `eval_atom_single` ahora pushea el outer row al `outer_stack` al evaluar `Exists`/`EqColumnRef`/`In` correlados dentro de combinadores (antes solo el dispatch top-level lo hacía). Eso destraba `EXISTS` correlacionado en `AND`/`OR`/`NOT`.
- `collect_in_set` + `eval_in_subquery` centralizan la lógica 3VL ANSI de `[NOT] IN (SELECT)` con tracking explícito de NULL.

### ⚠️ Limitaciones residuales (futuros bloques)
- `ALL`/`ANY`/`SOME` (`col > ALL (SELECT ...)`) — P2.
- Correlated `col = outer.col` puro fuera de `EXISTS` combinado con JOINs — P2.
- `LATERAL` joins — P3.
- `WITH` / CTE — bloque W (planificado aparte).
- Derived dentro de UPDATE/DELETE/INSERT — fuera de scope en H.

### 🧰 Códigos de error nuevos
- `4048` `DERIVED_TABLE_REQUIRES_ALIAS` — `FROM (SELECT ...)` sin alias.
- `4049` `DERIVED_DUPLICATE_COLUMN` — derived table con dos columnas del mismo nombre.
- `4050` `DERIVED_COLUMN_TYPE_AMBIGUOUS` — reservado para futura inferencia estricta de tipos en derived.
- `4051` `SCALAR_SUBQUERY_IN_EXPR_REQUIRES_PARENS` — reservado para validaciones futuras.
- `4024` `WHERE_COMBINATOR_CORRELATED_UNSUPPORTED` — DEPRECADO: el motor ya no lo genera (H levantó el bloqueo). Se conserva el slot por estabilidad del catálogo.

### 🧪 Validación
- 18 tests nuevos `h_*` cubriendo: derived basic / nested / con WHERE outer / con aggregate inside / join con persistente / alias requerido / duplicate column; NOT IN basic / NULL en subquery / NULL en outer; scalar subquery basic / correlated / too-many-rows / two-columns / no-rows-returns-null; correlated EXISTS AND/OR/two-EXISTS.
- 233 tests integración total (215 pre-H + 18 H), `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` limpios.

---

## 2026-05-26 — Bloque G3: aritméticos + concat + postfix Expr + funciones P2/P3

> **Un push a `main`** que cierra la familia G: operadores binarios `+`/`-`/`*`/`/`/`%`, concatenación `||`, postfix predicates (`IS [NOT] NULL`, `[NOT] LIKE`, `[NOT] IN`, `[NOT] BETWEEN`) sobre cualquier `Expr`, y las funciones escalares P2/P3 que quedaban abiertas en G1 — string (`TRIM`/`LTRIM`/`RTRIM`/`REPLACE`/`SPLIT_PART`), numéricas (`CEIL`/`FLOOR`/`MOD`/`POWER`/`SQRT`) y fecha (`DATE_ADD`/`DATE_SUB`/`DATEDIFF`/`EXTRACT`/`STRFTIME`).

### 🆕 Operadores
- Aritméticos binarios `+`, `-`, `*`, `/`, `%` sobre INT/FLOAT con promoción implícita (INT+FLOAT → FLOAT), `checked_*` para detectar overflow en INT, y error explícito en división/módulo por cero (entero o flotante).
- Concatenación `||` (regla PostgreSQL: misma precedencia que `+`/`-`). Cualquier tipo se reduce a TEXT con `value_to_text`; NULL propaga (ANSI estricta, igual que `CONCAT`).
- Postfix predicates sobre `Expr`: `LENGTH(x) IS NULL`, `UPPER(x) LIKE 'A%'`, `LENGTH(x) IN (3, 4, 5)`, `LENGTH(x) BETWEEN 3 AND 10` (más sus formas `NOT ...`). El path estructural pre-G3 (columna directa) se preserva intacto para no perder fast-paths.

### 🆕 Funciones escalares P2/P3
- **String:** `TRIM`, `LTRIM`, `RTRIM`, `REPLACE(s, from, to)`, `SPLIT_PART(s, sep, idx)` (1-based, fuera de rango → `''`).
- **Numéricas:** `CEIL` / `CEILING`, `FLOOR`, `MOD(a, b)` (alias del operador `%`), `POWER(x, y)` / `POW`, `SQRT(x)` (negativo → `[GBY-4045]`).
- **Fecha:** `DATE_ADD(d, n)`, `DATE_SUB(d, n)`, `DATEDIFF(d1, d2)` (días), `EXTRACT(YEAR|MONTH|DAY|HOUR|MINUTE|SECOND FROM expr)`, `STRFTIME(fmt, d)` con placeholders `%Y %m %d %H %M %S %%`.

### 🔧 AST + Parser
- `Expr` suma `Arith(Box<Expr>, ArithOp, Box<Expr>)`, `Like(...)`, `InList(...)`, `Between(...)`. Nuevo enum `ArithOp { Add, Sub, Mul, Div, Mod, Concat }`.
- Cadena de precedencia explícita en el parser: `parse_expr` → `parse_arith` (+/-/||) → `parse_arith_term` (*///%) → `parse_arith_factor` → `parse_expr_primary`. Comparadores y postfix predicates viven al tope (precedencia más baja, como en SQL estándar).
- Tokenizer: emite `||` como un único `Symbol`. `-N` literal solo se forma cuando el token previo NO termina un operando (heurística que respeta `LIMIT -1` / `VALUES (-3)` y a la vez deja funcionar `5 - 3`).
- `EXTRACT(field FROM expr)` se parsea con branch dedicado en `parse_func_call`; internamente se guarda como `Func(Extract, [Literal(String("YEAR")), expr])` para encajar en la firma genérica.
- `parse_where_atom_as_expr` ya no rechaza postfix sobre Expr — delega en `parse_expr` que aplica todo postfix uniformemente.

### 🚦 Executor
- `eval_expr` gana ramas para las nuevas variantes. `eval_arith` centraliza promoción de tipos, `checked_*` y división/módulo por cero. `Like`/`InList`/`Between` reusan los helpers existentes `eval_like` / `eval_in_list` / `eval_compare` con la misma 3VL que las variantes equivalentes de `WhereClause`.
- Helpers `days_from_civil` (inverso del existente `civil_from_days`) y formateadores `extract_date_field` / `strftime_format` / `parse_date_part_to_days` para las funciones de fecha.

### ⚠️ Limitaciones residuales (futuros bloques)
- `EXCLUDED.col` dentro de `ON CONFLICT DO UPDATE SET` (J2-P2 explícito).
- Unary `+` / `-` como operador prefix sobre expresión (el tokenizer captura literales negativos; expresiones tipo `-LENGTH(x)` quedan para una iteración futura — se puede escribir `0 - LENGTH(x)`).
- Subqueries dentro de `IN (...)` sobre LHS expresional (bloque H).
- Operadores aritméticos sobre tipos no numéricos (TEXT + INT, etc.) → `[GBY-4044]` explícito.

### 🧰 Códigos de error nuevos
- `4042` `ARITH_OVERFLOW` — overflow entero en `+/-/*//`.
- `4043` `DIVISION_BY_ZERO` — divisor cero en `/` o `%`.
- `4044` `ARITH_TYPE_MISMATCH` — operador aritmético sobre tipos no compatibles.
- `4045` `MATH_DOMAIN` — `SQRT(-x)`, `POWER(0, neg)`.
- `4046` `DATE_PARSE_ERROR` — TEXT no parseable como DATE/DATETIME en funciones de fecha.
- `4047` `EXTRACT_FIELD_INVALID` — campo no soportado por `EXTRACT`.

### 🧪 Validación
- ~30 integration tests nuevos `g3_*` cubriendo aritméticos (precedencia, paréntesis, overflow, division by zero, mezcla INT/FLOAT, NULL propagation, type mismatch), concat (`||`), postfix sobre Expr (IS NULL / LIKE / IN / BETWEEN), y todas las funciones P2/P3.
- `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- [`docs/SQL_REFERENCE.md`](docs/SQL_REFERENCE.md): subsección "Operadores aritméticos" + tabla de funciones P2/P3 + tabla de errores `4042`-`4047`.
- [`docs/MISSING_COMMANDS.md`](docs/MISSING_COMMANDS.md): los items P2/P3 cerrados se marcan `✅ (G3, 2026-05-26)`; el `||` y los aritméticos pasan a `✅`.
- [`docs/STATUS.md`](docs/STATUS.md): nota de cierre G3 en la fila "Funciones escalares".
- [`docs/ERROR_CODES.md`](docs/ERROR_CODES.md): seis filas nuevas (`4042`–`4047`).
- [`ROADMAP.md`](ROADMAP.md): bullet de cierre G3.

---

## 2026-05-26 — Bloque G2: expresiones escalares en WHERE / HAVING / UPDATE SET

> **Un push a `main`** que completa el bloque G iniciado por G1: las mismas funciones escalares / `CAST` / `CASE` / condicionales ahora se aceptan en las superficies de filtrado y mutación. Cierra la mayor limitación residual documentada en el changelog de G1.

### 🆕 Sentencias / cláusulas extendidas
- `WHERE` (de `SELECT`, `UPDATE`, `DELETE`): cualquier `Expr` BOOL/NULL es válida como átomo. Casos típicos: `WHERE LENGTH(name) > 3`, `WHERE UPPER(name) = 'X'`, `WHERE COALESCE(active, false) = true`, `WHERE CASE WHEN age > 18 THEN true ELSE false END = true`, `WHERE 5 < LENGTH(name)` (LHS literal).
- `HAVING`: ídem WHERE, conservando la libertad ya existente de referir agregados. Ej: `HAVING UPPER(group_col) = 'X'`.
- `UPDATE ... SET col = <expr>` y `ON CONFLICT DO UPDATE SET col = <expr>`: RHS pasa de `Value` a `Expr`. Se evalúa contra la fila **pre-update** (`SET a = b, b = a` swap-eligible).
- `DELETE FROM ... WHERE <expr>`: extensión gratuita gracias a que ya usaba el mismo `WhereExpr` (E3).

### 🔧 AST
- `UpdateStmt::assignments` y `OnConflictAction::DoUpdate::assignments` cambian de `Vec<(String, Value)>` a `Vec<(String, Expr)>`. Cambio de tipo público — los call-sites internos se actualizaron; los literales viejos siguen funcionando porque el parser construye `Expr::Literal(Value::X(...))`.
- `WhereClause` suma la variante `ExprPredicate { expr: Expr }`. Solo se construye cuando el átomo NO encaja en la forma estructural `IDENT OP literal` (LHS o RHS son funciones, CASE, CAST, literal a la izquierda, …); las variantes específicas pre-G2 (`Eq`, `Compare`, `Like`, `IsNull`, `InList`, `Between`) se preservan para mantener intactos los fast-paths PK / índice / range scan / EXISTS correlacionado.

### 🚦 Parser
- `parse_where_atom` arranca con `peek_atom_is_structural` — si el átomo es expresional cae a `parse_where_atom_as_expr` (ambos lados con `parse_expr_primary`, comparador o solo-expr).
- `parse_update` y la rama `ON CONFLICT DO UPDATE` usan `parse_expr` para la RHS de cada assignment.
- Las funciones agregadas siguen siendo estructurales en cualquier contexto: en HAVING resuelven contra el bucket; en WHERE el path estructural devuelve el `[GBY-4025]` claro (en vez del genérico `[GBY-4037]` que daría el path expresional).

### 🚦 Executor
- `eval_atom_single` y `eval_atom_joined` ganan un brazo `ExprPredicate { expr } => eval_expr_as_predicate(expr, row)`. El helper centraliza la 3VL: BOOL pasa tal cual, NULL → unknown (descarta la fila), cualquier otro tipo → `[GBY-4040]`.
- `filter_joined_rows_atom` agrega `ExprPredicate` al grupo "sin fast-path indexada — caer al evaluador 3VL".
- `exec_update` separa la validación shape (PK / columna existe / duplicados) — que sigue siendo one-shot — de la evaluación de la `Expr` que ahora ocurre dentro de `apply_update_to_pk` contra la fila pre-update. Pre-chequeo de tipo con `value_fits_column_type` para atribuir el mismatch a la columna exacta (`[GBY-4041]`).
- El planner (`generic_post_filter` + `Plan::FullScan`) reconoce `ExprPredicate` como predicado sin fast-path indexada, igual que los átomos E2.

### ⚠️ Limitaciones residuales (G3)
- Operadores postfix sobre expresión escalar (`LENGTH(x) IS NULL`, `UPPER(x) LIKE 'A%'`, `LENGTH(x) IN (...)`, `LENGTH(x) BETWEEN ... AND ...`) → `[GBY-4039]` con guía.
- Operador `||` para concatenación, aritméticos binarios (`+`/`-`/`*`/`/`), y funciones P2/P3 (`TRIM`, `REPLACE`, `CEIL`/`FLOOR`, `MOD`, `POWER`/`SQRT`, `DATE_ADD`/`DATE_SUB`, `DATEDIFF`, `EXTRACT`, `STRFTIME`, `SPLIT_PART`) siguen sin soporte.
- `EXCLUDED.col` dentro de `ON CONFLICT DO UPDATE SET` sigue sin soporte (J2-P2 explícito).

### 🧰 Códigos de error nuevos
- `4039` `EXPR_IN_PREDICATE_NOT_SUPPORTED` — operador postfix sobre Expr.
- `4040` `WHERE_EXPR_NOT_BOOLEAN` — expresión en WHERE/HAVING que no rinde BOOL/NULL.
- `4041` `UPDATE_SET_TYPE_MISMATCH` — RHS de `SET col = <expr>` con tipo incompatible.

### 🧪 Validación
- 20 integration tests nuevos en `tests/integration_test.rs` (`g2_*`): WHERE con LENGTH/UPPER/COALESCE/CASE/CAST/3VL, combinación con E1 (AND/OR), LHS literal, error 4040, error 4039 (IS NULL sobre Expr), UPDATE SET con UPPER/COALESCE/CASE/CAST/PK bloqueado/tipo mismatch, snapshot pre-update (`SET a=UPPER(a), b=a`), HAVING con UPPER, DELETE con LENGTH, UPDATE WHERE con UPPER.
- 176/176 tests pasan (los 156 previos + 20 g2). `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- [`docs/SQL_REFERENCE.md`](docs/SQL_REFERENCE.md): sección "Funciones escalares" actualizada con la extensión a WHERE/HAVING/UPDATE SET, ejemplos nuevos, y tres filas de errores típicos (4039/4040/4041).
- [`docs/MISSING_COMMANDS.md`](docs/MISSING_COMMANDS.md): nota de cierre G2 con limitaciones residuales que pasan a G3.
- [`docs/STATUS.md`](docs/STATUS.md): fila de "Funciones escalares" promovida a 🟢 con scope completo y limitaciones residuales explícitas.
- [`docs/ERROR_CODES.md`](docs/ERROR_CODES.md): tres filas nuevas (`4039`–`4041`).
- [`ROADMAP.md`](ROADMAP.md): bullet de cierre G2 debajo del de G1.

---

## 2026-05-26 — Bloque G1: funciones escalares en SELECT list

> **Un push a `main`** que abre el subsistema de funciones escalares: built-ins de string / numéricas / fecha + `CAST` + `CASE` + condicionales (`COALESCE`/`NULLIF`/`IFNULL`/`IF`). Cierra los P0/P1 del bloque G en [docs/MISSING_COMMANDS.md](docs/MISSING_COMMANDS.md) **dentro del SELECT list** — la extensión a `WHERE`/`HAVING`/`UPDATE SET` queda para G2.

### 🆕 Sentencias / cláusulas nuevas
- `SELECT` ahora acepta expresiones escalares como ítems del SELECT list, además de columnas crudas y agregados. `AS alias` opcional por ítem.
- Funciones built-in: `LENGTH`, `UPPER`, `LOWER`, `SUBSTR`/`SUBSTRING`, `CONCAT`, `ABS`, `ROUND`, `NOW`, `CURRENT_DATE`/`CURDATE`, `CURRENT_TIMESTAMP`, `COALESCE`, `NULLIF`, `IFNULL`, `IF`/`IIF`.
- `CAST(expr AS TYPE)` para INT / FLOAT / TEXT / BOOL / DATE / DATETIME / JSON.
- `CASE [operand] WHEN cond THEN val [...] [ELSE val] END` en sus dos formas (searched y simple).

### 🔧 AST
- Nuevo enum `Expr { Literal | Column | Func | Cast | Case | Compare | IsNull }` y enum auxiliar `ExprCmpOp` con los seis comparadores estándar.
- Nuevo enum `ScalarFunc` con las 14 built-ins soportadas + helper `from_ident` que acepta los aliases comunes.
- `SelectItem` gana la variante `Expression { expr, alias }`. `Star`/`Column`/`Aggregate` se preservan para no romper fast-paths.

### 🚦 Executor
- `resolve_selected_columns` ahora devuelve `Vec<Projection>` donde cada `Projection` es `BareColumn` (lookup directo, fast-path pre-G1) o `Expression` (evaluada per-row con `eval_expr`).
- `resolve_joined_projection` análogo: las columnas referenciadas dentro de `Expr::Column` se re-escriben a la forma cualificada `alias.col` con `rewrite_expr_columns_for_join` antes de la proyección — soporta JOIN + expresión escalar end-to-end (con detección de ambigüedad vía `[GBY-4018]`).
- Validación: NULL propagation por defecto (excepto `COALESCE`/`NULLIF`/`IFNULL`/`IF` con su propio control de NULL, y las funciones zero-arg de tiempo). `CASE` searched exige cond BOOL; `CASE` simple matchea por igualdad ANSI (NULL nunca matchea NULL).
- `NOW()` / `CURRENT_TIMESTAMP` / `CURRENT_DATE` formatean UTC como TEXT sin chrono — implementación inline con `civil_from_days` de Howard Hinnant.

### ⚠️ Limitaciones residuales (G2)
- Las expresiones escalares **solo** se aceptan en `SELECT` list. En `WHERE` / `HAVING` / `UPDATE SET` siguen aplicando las restricciones pre-G1 (literales o referencias a columna).
- En queries con `GROUP BY`/`HAVING`/agregados, `SelectItem::Expression` no se acepta todavía (devuelve `[GBY-4027]`). Lo mismo para `RETURNING` (devuelve `[GBY-2002]` con mensaje claro).
- No hay operador `||` para concatenar texto — usar `CONCAT(a, b, ...)`. Tampoco hay operadores aritméticos binarios (`+`/`-`/`*`/`/`).
- Funciones P2/P3 (`TRIM`, `REPLACE`, `CEIL`/`FLOOR`, `MOD`, `POWER`/`SQRT`, `DATE_ADD`/`DATE_SUB`, `DATEDIFF`, `EXTRACT`, `STRFTIME`, `SPLIT_PART`) siguen en backlog del mismo bloque G.

### 🧰 Códigos de error nuevos
- `4034` `SCALAR_FN_ARITY` — función escalar con cantidad equivocada de argumentos.
- `4035` `SCALAR_FN_TYPE_MISMATCH` — argumento de un tipo no aceptado por la función.
- `4036` `CAST_INVALID` — `CAST` cuyo valor no se puede convertir al tipo destino.
- `4037` `SCALAR_FN_UNKNOWN` — función escalar invocada que el motor no conoce.
- `4038` `CASE_BRANCH_TYPE_MISMATCH` — condición de `CASE WHEN` searched que no evalúa a BOOL.

### 🧪 Validación
- 12 integration tests nuevos en `tests/integration_test.rs` (`g1_*`): string funcs, SUBSTR edge cases, CONCAT mixto, ABS/ROUND, NOW/CURRENT_DATE/CURRENT_TIMESTAMP shape, COALESCE/NULLIF/IFNULL/IF, CAST válido + inválido, CASE searched + simple, alias, errores (arity/tipo/desconocido), 3VL con NULL, expresión sobre JOIN.
- `cargo fmt --check + cargo clippy --all-targets -- -D warnings + cargo test --all-targets` limpios.

### 📚 Documentación
- Sección nueva en [`docs/SQL_REFERENCE.md`](docs/SQL_REFERENCE.md) ("Funciones escalares (bloque G1)") con EBNF + tabla de funciones + ejemplos + errores típicos.
- [`docs/MISSING_COMMANDS.md`](docs/MISSING_COMMANDS.md): marcado `✅ (G1, 2026-05-26)` en los items P0/P1 cerrados.
- [`docs/STATUS.md`](docs/STATUS.md): nueva fila en la matriz de madurez por subsistema.
- [`docs/ERROR_CODES.md`](docs/ERROR_CODES.md): seis filas nuevas (`4033`-`4038`).
- [`ROADMAP.md`](ROADMAP.md): bullet de cierre en Fase 2.

---

## 2026-05-25 — Bloque J2: UPSERT, REPLACE INTO, RETURNING

> **Un push a `main`** que completa los pendientes del bloque J (excepto `UPDATE ... FROM`, deferido).

### 🆕 Sentencias / cláusulas nuevas
- `INSERT ... ON CONFLICT [(col)] DO NOTHING` — UPSERT pasivo (skip silencioso).
- `INSERT ... ON CONFLICT [(col)] DO UPDATE SET col = value, ...` — UPSERT activo (actualiza filas conflictivas con literales; sin `EXCLUDED.col` por ahora).
- `REPLACE INTO t (cols) VALUES (...)` — alias SQLite-style; desugar a `INSERT ... ON CONFLICT DO REPLACE` (borra fila conflictiva vía cascade FK + inserta nueva).
- `INSERT|UPDATE|DELETE ... RETURNING *` y `... RETURNING col1, col2` — devuelve las filas afectadas en el ResultSet (INSERT: post-insert; UPDATE: post-update; DELETE: pre-delete snapshot).

### 🔧 AST
- `InsertStmt` gana `on_conflict: Option<OnConflict>` y `returning: Option<Vec<SelectItem>>`.
- `UpdateStmt` y `DeleteStmt` ganan `returning: Option<Vec<SelectItem>>`.
- Nuevo enum `OnConflictAction { DoNothing | DoUpdate { assignments } | Replace }`.
- Nuevo `Statement::Replace(InsertStmt)` (desugar via parser).

### 🚦 Executor
- `apply_insert_row_with_conflict` reemplaza `apply_insert_row` y orquesta la trayectoria por fila: detecta conflictos PK + UNIQUE vía `detect_conflict_pks` y dispatcha a la acción. `RowOutcome { Inserted | Updated | Skipped }` mantiene los contadores y la lista de RETURNING.
- `DoUpdate` reusa `apply_update_to_pk` (E3) sobre las PKs conflictivas.
- `Replace` borra las PKs conflictivas con `delete_with_cascade` (J) y luego sigue el path normal de insert.
- `exec_update` y `exec_delete` recolectan filas post-update / pre-delete cuando hay RETURNING y proyectan vía `project_returning` + `returning_column_names`.
- `format_insert_message` cuenta inserted + replaced + skipped en el `message` del response.

### ⚠️ Limitaciones residuales
- `EXCLUDED.col` en `DO UPDATE SET col = EXCLUDED.col` no se soporta — los RHS deben ser literales por ahora. Workaround: precomputar el valor en cliente.
- `UPDATE ... FROM otra_tabla` (P2) — pendiente; requiere refactor del RHS de SET para aceptar column refs cualificados.
- `ON CONFLICT (col)` solo acepta una columna; multi-column unique constraints no se soportan todavía (los índices compuestos están en backlog del bloque K).

### 🧰 Códigos de error nuevos
- `4031` `ON_CONFLICT_INVALID` — `ON CONFLICT` malformada.
- `4032` `ON_CONFLICT_TARGET_NOT_UNIQUE` — `ON CONFLICT (col)` sobre columna sin PK/UNIQUE.

### 🧪 Validación
- 10 integration tests nuevos en `tests/integration_test.rs` (`j2_*`): INSERT RETURNING * / cols, UPDATE RETURNING, DELETE RETURNING, UPSERT DO NOTHING, UPSERT DO UPDATE, target no-único error, REPLACE INTO reemplaza / inserta, RETURNING con filas omitidas.
- `cargo check + cargo fmt --check + cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- (Se actualiza en el mismo push: SQL_REFERENCE, MISSING_COMMANDS, ERROR_CODES.)

---

## 2026-05-25 — Bloque J: DML masivo (multi-row `INSERT`, `INSERT...SELECT`, `TRUNCATE`)

> **Un push a `main`** que destraba inserts en bloque y limpieza de tabla.

### 🆕 Sentencias nuevas
- `INSERT INTO t (cols) VALUES (a,b),(c,d),...` — multi-row.
- `INSERT INTO t (cols) SELECT ...` — copia masiva desde otra query (puede tener WHERE/ORDER BY/JOIN/GROUP BY del bloque F).
- `TRUNCATE [TABLE] t` — borra todas las filas de la tabla manteniendo el schema. Implementación naive (scan-all-pks + delete_with_cascade); respeta FKs `ON DELETE`. No es O(1) como en PG/MySQL.

### 🔧 Refactor
- `InsertStmt.values: Vec<Value>` → `source: InsertSource { Values(Vec<Vec<Value>>) | Select(Box<SelectStmt>) }`. Single-row queda como caso particular de `Values(vec![row])`.
- `exec_insert` validara columnas + dedup UNA vez y luego itera filas-fuente delegando en el nuevo `apply_insert_row` (que encapsula NOT NULL/UNIQUE/FK/encode/insert/index-maintenance per-row).
- Response `message` ahora trae cuenta: `"OK (3 filas insertadas)"`.

### ⚠️ Comportamiento
- Multi-row no es transaccionalmente atómico **por sí solo** — fila K que falla deja las K-1 anteriores en el cache. El wrap del batch (auto-commit del `/exec` o `BEGIN`/`ROLLBACK` explícito del bloque T) define el alcance del rollback.
- `INSERT...SELECT` ejecuta la subquery completa antes de empezar a insertar (materializa primero). Para queries grandes esto es O(filas) en memoria.

### ⚠️ Limitaciones residuales del bloque J
- `INSERT ... ON CONFLICT DO UPDATE` / `UPSERT` (P1) — pendiente.
- `REPLACE INTO` (P2) — pendiente.
- `RETURNING` clause (P2) — pendiente; requiere extender `ResultSet` con filas devueltas.
- `UPDATE ... FROM otra_tabla` (P2) — pendiente.

### 🧪 Validación
- 8 integration tests nuevos en `tests/integration_test.rs` (`j_*`): multi-row INSERT, aridad mismatch aborta, INSERT...SELECT copia, INSERT...SELECT con WHERE, INSERT...SELECT aridad mismatch, TRUNCATE TABLE preserva schema, TRUNCATE sin keyword TABLE, multi-row con conflicto UNIQUE aborta.
- `cargo check + cargo fmt --check + cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- (Se actualiza en el mismo push: SQL_REFERENCE, MISSING_COMMANDS.)

---

## 2026-05-25 — Bloque T: transacciones explícitas (`BEGIN`/`COMMIT`/`ROLLBACK`)

> **Un push a `main`** que cierra el último P0 del top-5 del roadmap.

### 🔁 Sentencias nuevas
- `BEGIN` / `BEGIN TRANSACTION` / `BEGIN WORK` / `START TRANSACTION` — marca el inicio de una transacción explícita.
- `COMMIT` / `COMMIT TRANSACTION` / `COMMIT WORK` / `END` — persiste lo acumulado y re-abre una tx fresca.
- `ROLLBACK` / `ROLLBACK TRANSACTION` / `ROLLBACK WORK` — descarta lo acumulado y re-abre una tx fresca.

### 🔧 Cambios
- `Statement::Begin` / `Commit` / `Rollback` añadidos al AST.
- `Engine` gana un flag `explicit_tx: bool`. El Pager subyacente SIEMPRE tiene una transacción abierta (la abre el wrap del caller); este flag distingue la implícita del wrap de la explícita pedida por SQL.
- `exec_begin` / `exec_commit` / `exec_rollback` en el Engine. `COMMIT`/`ROLLBACK` invocan `pager.commit()`/`pager.rollback()` seguido de `pager.begin()` para preservar la invariante del wrap (el caller siempre puede hacer commit al final).

### ⚠️ Limitación documentada
- El `ROLLBACK` opera sobre el cache de páginas del Pager — descarta TODO lo cacheado, incluidas las sentencias del MISMO batch que ocurrieron ANTES del `BEGIN`. En la práctica esto significa que `BEGIN`/`ROLLBACK` solo aborta limpio cuando el batch entero arranca con `BEGIN` como primera sentencia. Cross-request transactions (mantener una tx abierta entre `/exec` HTTP) requieren session state en el server — fuera de scope para esta primera versión de T.
- `SAVEPOINT` / `ROLLBACK TO SAVEPOINT` (P1) no soportados. `SET TRANSACTION ISOLATION LEVEL ...` (P2) y `BEGIN READ ONLY` (P2) tampoco.

### 🧰 Códigos de error nuevos
- `4029` `TX_BEGIN_DOUBLE` — `BEGIN` con transacción explícita ya abierta.
- `4030` `TX_END_WITHOUT_BEGIN` — `COMMIT`/`ROLLBACK` sin `BEGIN` previo.

### 🧪 Validación
- 6 integration tests nuevos en `tests/integration_test.rs` (`t_*`): BEGIN+COMMIT persiste, BEGIN+ROLLBACK descarta, doble BEGIN error, COMMIT/ROLLBACK sin BEGIN error, alias START TRANSACTION/END, dos bloques BEGIN/COMMIT consecutivos.
- `cargo check + cargo fmt --check + cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- (Se actualiza en el mismo push: SQL_REFERENCE, MISSING_COMMANDS, ERROR_CODES.)

---

## 2026-05-25 — Bloque F: agregaciones (`GROUP BY`, `HAVING`, `COUNT`/`SUM`/`AVG`/`MIN`/`MAX`, `DISTINCT`)

> **Un push a `main`** que destraba reporting básico. Cierra el hueco más grande del top-5 del roadmap.

### 🧮 Funciones agregadas
- `COUNT(*)` — cuenta todas las filas del bucket (incluyendo NULLs en otras columnas).
- `COUNT(col)` — cuenta filas donde `col` no es NULL.
- `COUNT(DISTINCT col)` — valores no-NULL distintos.
- `SUM(col)` — INT preserva INT; mixto INT+FLOAT promueve a FLOAT. Conjunto vacío o todo-NULL → `NULL` (ANSI).
- `AVG(col)` — promedio FLOAT sobre valores no-NULL.
- `MIN(col)` / `MAX(col)` — ignora NULLs. Conjunto vacío o todo-NULL → `NULL`.

### 🗂️ GROUP BY + HAVING
- `GROUP BY <col> [, <col>]*` — bucketing por tupla (NULLs agrupan con NULLs, consistente con ANSI).
- `HAVING <expr>` — filtro post-agregación. Reusa `WhereExpr` con `allow_aggregates=true`: la LHS de un átomo puede ser una función agregada (`HAVING SUM(price) > 100`) o un alias del SELECT (`HAVING total > 100`).
- ANSI estricto: toda columna no-agregada en el SELECT debe figurar en `GROUP BY` — `[GBY-4027]` si no.

### 🔀 DISTINCT
- `SELECT DISTINCT col [, col]*` — dedup preservando el primer orden de aparición. Compatible con agregados (aunque suele ser redundante post-GROUP BY).

### 🔧 AST
- Nuevo enum `SelectItem { Star | Column(String) | Aggregate { func, arg, alias } }`. `SelectStmt.columns: Vec<String>` pasa a `Vec<SelectItem>`.
- Nuevos campos en `SelectStmt`: `distinct: bool`, `group_by: Vec<String>`, `having: Option<WhereExpr>`.
- Nuevo enum `AggFunc { Count, Sum, Avg, Min, Max }` y `AggArg { Star, Column, DistinctColumn }`.

### 🚦 Executor
- `exec_select` detecta `needs_aggregation` (cualquier agregado, GROUP BY, o HAVING presente) y desvía al nuevo `exec_aggregate_pipeline`. El path no-agregado mantiene fast-paths E1+E2+E3 intactos.
- `exec_aggregate_pipeline`: valida ANSI → bucketea por GROUP BY tuple (encoded como bytes para HashMap) → calcula agregados → aplica HAVING → proyecta a `output_name` → DISTINCT → ORDER BY contra esquema de salida → window.
- `dedup_preserving_order` helper para DISTINCT puro.

### ⚠️ Limitaciones residuales
- **Agregados sobre JOINs no se soportan todavía** — `[GBY-4028] AGGREGATE_OVER_JOIN_UNSUPPORTED`. Workaround: encapsular el JOIN en una subquery y agregar afuera.
- `GROUP_CONCAT` / `STRING_AGG`, `JSON_AGG` / `ARRAY_AGG` — P2/P3, fuera de F.
- Agregados en `ORDER BY` solo via alias o nombre canónico (`order by sum_x`) — no acepta la sintaxis cruda `ORDER BY SUM(x)`. Doable en una iteración menor.

### 🧰 Códigos de error nuevos
- `4025` `AGGREGATE_OUTSIDE_HAVING_OR_SELECT` — agregado en `WHERE` u otra cláusula prohibida.
- `4026` `AGGREGATE_ARG_INVALID` — `SUM(*)`, `AVG(DISTINCT x)`, tipos incompatibles.
- `4027` `SELECT_COLUMN_NOT_IN_GROUP_BY` — columna no-agregada que no figura en GROUP BY.
- `4028` `AGGREGATE_OVER_JOIN_UNSUPPORTED` — agregado en SELECT con JOINs.

### 🐛 Fixes incluidos
- Tres tests `e3_update_*` que llamaban a `SELECT ... WHERE col_no_indexed = val` para verificar el efecto del UPDATE — falla con el fast-path indexado pre-existente. Reescritos para usar `WHERE … AND id > 0` (forza FullScan + 3VL).
- `parser_returns_error_for_invalid_where` esperaba el mensaje legado "WHERE soporta solo" — actualizado al nuevo mensaje E2 y al código `[GBY-4001]`.
- `update_and_delete_by_pk_roundtrip` esperaba error al hacer `DELETE FROM u WHERE name = 1` — ahora es válido (E3). Cambiado a verificar 0 borrados y filas intactas.
- `secondary_index_lookup_and_maintenance` esperaba que `AND` no estuviera soportado — actualizado al comportamiento E1.

### 🧪 Validación
- 14 integration tests nuevos en `tests/integration_test.rs` (`f_*`): COUNT(*) global, COUNT(*) AS alias, COUNT(col) ignora NULL, SUM/AVG/MIN/MAX, GROUP BY single, GROUP BY multi, HAVING con agregada, HAVING con alias, DISTINCT, COUNT(DISTINCT), validación ANSI (col no-GROUP), agregado en WHERE rechazado, agregado sobre JOIN rechazado, input vacío con neutros.
- `cargo check + cargo fmt --check + cargo clippy --all-targets -- -D warnings` limpios.

### 📚 Documentación
- (Se actualiza en el mismo push: SQL_REFERENCE, MISSING_COMMANDS.)

---

## 2026-05-25 — Bloque E3: `UPDATE` / `DELETE` por cualquier `WHERE`

> **Un push a `main`** que destraba mutaciones masivas y por columnas no-PK.

### 🔧 Cambios
- `UpdateStmt` y `DeleteStmt` ahora llevan `where_clause: WhereExpr` (mismo grammar que `SELECT`). El campo legacy `where_column + where_pk: i64` desaparece.
- `parse_update` / `parse_delete` reusan `parse_where_expr()` — todos los operadores de E1+E2 + subqueries `IN (SELECT)` / `= (SELECT)` / `EXISTS` se aceptan sin tocar el parser.
- Nuevo helper `Engine::resolve_target_pks` que devuelve la lista de PKs matcheadas:
  - **Fast-path** para `WHERE pk = N` literal (preserva el comportamiento pre-E3, incluyendo el error `[GBY-3006] ROW_NOT_FOUND_FOR_PK` cuando N no existe).
  - **Fallback genérico**: FullScan + evaluador `eval_where_expr_single` (mismo motor 3VL que SELECT). Sin optimización por índice todavía — correctitud primero, perf en backlog.
- `exec_update` extrae la lógica per-fila a `apply_update_to_pk` y la invoca por cada PK del lote. Las validaciones (NOT NULL, UNIQUE, FK) corren por-fila — un UNIQUE conflict en la fila K corta el batch y deja las K-1 anteriores commiteadas dentro de la misma transacción (la decisión de revert depende del wrapping en el cliente).
- `exec_delete` resuelve PKs **antes** de borrar para evitar interferencia con cascadas FK que tocan otras tablas o self-refs. Cada cascade tolera filas ya eliminadas (idempotente).
- Response `message` ahora trae la cuenta: `"OK (3 filas actualizadas)"` / `"OK (2 filas eliminadas)"`.

### ⚠️ Limitaciones residuales
- `UPDATE ... FROM otra_tabla` (UPDATE con JOIN) y `DELETE ... JOIN` no se soportan — requieren parser de FROM compartido con SELECT y queda para un bloque futuro.
- `<` / `>` / `LIKE` / `IS NULL` sobre PK o columna indexada **no aprovechan el índice** todavía — todos van por FullScan. Optimización indexada para `=` sobre columna indexada queda en backlog.
- El error `[GBY-4003] UPDATE_DELETE_REQUIRES_PK_FILTER` queda inactivo. La constante permanece en `errors.rs` por el contrato de estabilidad (nunca se reusa) — futuras versiones nunca volverán a emitirla.

### 🧪 Validación
- 10 integration tests nuevos en `tests/integration_test.rs` (`e3_*`): UPDATE por columna indexada, por predicado compuesto, por subquery, 0 matches, fast-path PK con error legado, DELETE por col indexada / combinador / subquery / LIKE, UPDATE preservando UNIQUE.
- `cargo check --lib --tests` limpio sin warnings.

### 📚 Documentación
- `docs/SQL_REFERENCE.md` — EBNF de UPDATE/DELETE actualizada, ejemplos nuevos, errores típicos al día.
- `docs/MISSING_COMMANDS.md` — E3 marcado cerrado, hueco #4 del top-5 tachado.
- `docs/ERROR_CODES.md` — entry `4003` marcada como histórica.

---

## 2026-05-25 — Bloque E2: comparadores, `LIKE`, `IS NULL`, `IN literal`

> **Un push a `main`** que cierra el set de operadores básicos del `WHERE`.

### 🆕 Nuevos operadores
- `<`, `<=`, `>`, `>=`, `<>`, `!=` sobre INT / FLOAT / TEXT (lex) / BOOL. NULL en cualquiera de los dos lados → `NULL` (3VL). Tipos incompatibles → `false` (no abortamos la query).
- `[NOT] LIKE 'patron'` sobre TEXT. Wildcards SQL estándar (`%` = cero o más, `_` = exactamente uno) con escape `\%` / `\_`. Backtracking O(|s|·|p|), suficiente para patrones realistas.
- `IS [NOT] NULL` — único predicado que NO propaga NULL (es la forma explícita de testear ausencia).
- `[NOT] IN (lit1, lit2, ...)` con lista literal. Semántica ANSI: si la columna es NULL → NULL; si no hay match y la lista contiene NULL → NULL (especialmente sensible en `NOT IN`).

### 🧬 Tokenizer
- Nuevos símbolos: `<`, `<=`, `>`, `>=`, `<>`, `!=` (con lookahead de 1 char). `!` suelto sigue siendo error (sugerencia explícita en el mensaje).

### 🧠 AST
- `WhereClause` extendido con cuatro variants nuevos: `Compare { op: CompareOp, ... }`, `Like { pattern, negated }`, `IsNull { negated }`, `InList { values, negated }`. Ningún variant tiene fast-path indexada por ahora — todos van por `generic_post_filter` + evaluador 3VL.

### 🚦 Executor
- `generic_post_filter` ahora se activa también cuando el átomo único es E2 (Compare/Like/IsNull/InList). El path por PK/índice queda intacto para `=`, `BETWEEN`, `IN (SELECT)`, `= (SELECT)`, EXISTS y EqColumnRef.
- Tres helpers puros: `eval_compare`, `eval_like`, `eval_in_list`. `like_match` es backtracking recursivo con soporte de escape.

### ⚠️ Limitación residual
- `NOT IN (SELECT ...)` (subquery) explícitamente rechazado por ahora — el desugar a `NOT (col IN (SELECT))` cambia la semántica con NULLs y queda para el bloque H. `NOT IN (lista literal)` sí está.
- `<` / `>` / `<=` / `>=` no aprovechan el índice OrderedInt todavía (range scan optimization queda en backlog; correctitud antes que velocidad).

### 🧪 Validación
- 11 integration tests nuevos en `tests/integration_test.rs` (`e2_*`): comparadores INT, `<>`/`!=` sinónimos, comparación TEXT lex, LIKE básico, NOT LIKE, IS NULL / IS NOT NULL, IN literal, NOT IN con 3VL, combinaciones con AND/OR de E1, LIKE con escape, comparador con JOIN.
- `cargo check --lib --tests` limpio.

### 📚 Documentación
- `docs/SQL_REFERENCE.md` — EBNF del WHERE actualizado, ejemplos de cada operador nuevo, fila E2 en la tabla de soporte.
- `docs/MISSING_COMMANDS.md` — E2 marcado cerrado, hueco #2 del top-5 tachado, comparadores/LIKE/IS NULL/IN literal en ✅.

---

## 2026-05-25 — Bloque E1: `AND` / `OR` / `NOT` + paréntesis en `WHERE`

> **Un push a `main`** que destraba el filtro compuesto en cualquier `SELECT`.

### 🔀 WHERE booleano (bloque E1)
- AST: `WhereClause` (plano) → `WhereExpr = And | Or | Not | Atom(WhereClause)`. Los átomos siguen siendo los seis predicados pre-existentes (`Eq`, `Between`, `In`, `EqSubquery`, `EqColumnRef`, `Exists`) — el bloque no toca su semántica.
- Parser: precedencia estándar SQL `OR` < `AND` < `NOT` < paréntesis / átomo. `NOT EXISTS` mantiene la forma vieja (`Atom(Exists{negated:true})`) para preservar el fast-path correlacionado.
- Executor: cuando el WHERE se reduce a un único átomo, se usan las fast-paths existentes (PK directo, índice secundario, range scan, EXISTS correlacionado post-filter). Cuando hay combinadores se cae a FullScan + evaluador trivaluado (3VL) row-a-row — `defer_window` se activa para que `LIMIT`/`OFFSET` se apliquen DESPUÉS del filtro.
- 3VL para `NULL`: `NULL AND false = false`, `NULL AND true = NULL`, `NULL OR true = true`, `NOT NULL = NULL`. Solo `Some(true)` mantiene la fila.
- Soporte completo en `SELECT` con o sin JOINs. `filter_joined_rows` ahora recibe `&WhereExpr` y aplica el mismo evaluador 3VL sobre filas joined.

### ⚠️ Limitación residual
- `EXISTS` correlacionado y `col = otra.col` (column-ref del outer) **solo se permiten como único átomo del WHERE**. Combinarlos con `AND`/`OR`/`NOT` devuelve `[GBY-4024]`. La generalización queda explícitamente fuera de E1.

### 🧰 Código de error nuevo
- `4024` `WHERE_COMBINATOR_CORRELATED_UNSUPPORTED`

### 🧪 Validación
- 11 integration tests nuevos en `tests/integration_test.rs` (sufijo `e1_*`): AND, OR, NOT, paréntesis, precedencia, BETWEEN + AND combinador, 3VL sobre NULL, NOT anidado, combinador con LIMIT+ORDER, doble NOT, combinador con JOIN, error sintáctico.
- `cargo check --lib --tests` limpio (0 warnings).

### 📚 Documentación
- `docs/SQL_REFERENCE.md` — EBNF del WHERE reescrita con precedencia + 3VL + ejemplos.
- `docs/MISSING_COMMANDS.md` — E1 marcado como cerrado; top-5 actualizado.
- `docs/ERROR_CODES.md` — entry `4024`.

---

## 2026-05-24 — Subqueries completas + roadmap de JOINs cerrado

> **Siete pushes consecutivos a `main`** que cierran dos features grandes del motor SQL.

### 🧩 Subqueries (3 bloques)
- `WHERE col IN (SELECT …)` — no-correlacionada, single-column. Reusa lookup PK/índice.
- `WHERE col = (SELECT …)` — subquery escalar (1 × ≤1). 0 filas o NULL → match vacío (ANSI). >1 fila → `[GBY-4014]`.
- `WHERE [NOT] EXISTS (SELECT …)` — no-correlacionada (pre-ejecuta) y correlacionada single-eq (`inner_col = outer.col`, post-filter per-row con `outer_stack`).

### 🔗 JOINs (4 bloques)
- **A** — `INNER JOIN`, `CROSS JOIN`, comma-syntax (`FROM a, b`), aliases con `[AS]`, multi-tabla en chain (left-deep), self-join. Columnas cualificadas (`tabla.col` o `alias.col`). `SELECT *` expande prefijado.
- **B** — `LEFT [OUTER] JOIN`, `RIGHT [OUTER] JOIN`, `FULL [OUTER] JOIN` con NULL-fill por kind. `OUTER` opcional (ANSI).
- **C** — `JOIN ... USING (col)` (sugar para `ON l.col = r.col`) y `NATURAL JOIN` (auto-derive del USING). `SELECT *` omite la columna fusionada del right.
- **D** — Index-loop join optimization transparente: cuando el `ON` (o el USING/NATURAL derivado) apunta contra PK o columna indexada del right Y el kind es INNER/LEFT, el engine reemplaza el FullScan del right por lookup dirigido. O(N×M) → O(N×log M) por JOIN.

### 🧰 Códigos de error nuevos
- `4011` `SUBQUERY_MUST_RETURN_ONE_COLUMN`
- `4012` `IN_PK_TYPE_MISMATCH`
- `4013` `IN_REQUIRES_PK_OR_INDEX`
- `4014` `SCALAR_SUBQUERY_TOO_MANY_ROWS`
- `4015` `EXISTS_REQUIRES_SUBQUERY`
- `4016` `OUTER_COLUMN_REF_INVALID`
- `4017` `TABLE_ALIAS_DUPLICATED`
- `4018` `COLUMN_AMBIGUOUS`
- `4019` `COLUMN_QUALIFIER_NOT_FOUND`
- `4020` `JOIN_PREDICATE_REQUIRED`
- `4021` `CROSS_JOIN_WITH_ON`
- `4022` `USING_COLUMN_INVALID`
- `4023` `NATURAL_JOIN_NO_COMMON_COLUMN`

### 📚 Documentación
- Doc barrido completo: `README.md`, `docs/SQL_REFERENCE.md`, `docs/STATUS.md`, `docs/ERROR_CODES.md`, `TROUBLESHOOTING.md`, `RUNBOOK.md`, `docs/POSITIONING.md`, `docs/COMPETITIVE_ANALYSIS.md`, `docs/ARCHITECTURE.md`, `docs/API.md`, `docs/TECHNICAL_SPECS.md`, `RECRUITER.md`, `ROADMAP.md`, `web/phpgabyadmin/index.php`.

### 🧪 Validación
- **71/71 tests** integración verdes (16 nuevos entre subqueries y JOINs).
- `cargo fmt --check` ✅ · `cargo clippy --all-targets -- -D warnings` ✅.

---

## 2026-05-18 — Vigesimoséptima intervención: reframe — `gabysql` es un proyecto de aprendizaje, no comercial

> **Solo docs. Cero código.** Reescribe el marco operativo del proyecto.

### ✨ Cambio
- Nuevo documento **[docs/AGENDA_INVESTIGACION.md](docs/AGENDA_INVESTIGACION.md)** (~500 líneas, 10 secciones) que reemplaza como fuente operativa a `COMMERCIAL_ROADMAP.md`/`POSITIONING.md`/`COMPETITIVE_ANALYSIS.md`. Contiene:
  - El reframe explícito: el proyecto **no es comercial y no apunta a serlo**.
  - La tesis: "¿cómo se vería una DB nativa de la era de los agentes LLM?".
  - 7 ejes de investigación con honestidad sobre qué entiendo / qué no / qué cuesta:
    1. Schema semántico (no solo tipado)
    2. Plan-as-data en cada respuesta
    3. Embedded variants de columnas TEXT
    4. Time-travel por default
    5. Audit trail consultable como tabla
    6. Schema migration como conversación
    7. Probes de invariantes
  - 6 Fases de aprendizaje (α–ζ) con **objetivo cognitivo** ("qué quiero entender"), no objetivo de producto.
  - Anti-agenda explícita: lo que NO entra (JOIN/GROUP BY/replicación/optimizer cost-based/etc.).
  - Ritmo realista (1 intervención/semana, no 9/día) y métricas de éxito honestas ("puedo explicar X" en vez de "MAUs").
- **Marcados como históricos** (banner explícito al inicio):
  - `docs/COMMERCIAL_ROADMAP.md`
  - `docs/POSITIONING.md`
  - `docs/COMPETITIVE_ANALYSIS.md`
- **ADR-0007** (Camino A) marcada como `🗑️ Superseded por AGENDA_INVESTIGACION.md`. El índice de ADRs refleja el cambio.
- **README.md** reescribe la introducción y la tabla de documentos clave: el proyecto se presenta como lo que es (laboratorio de aprendizaje sobre DBs + agentes), no como producto.
- **ROADMAP.md** redirige a la nueva agenda como fuente operativa y mantiene su rol histórico (qué entregó cada Fase 1/2).

### 🎯 Por qué este cambio
Auditoría con el usuario del estado del proyecto:
> *"además no se saca nada con pensar que alguien le interese, si creo todavia esta en pañales, lo realmente es mi objetivo, crea una base de datos que no sea como las demás, mientras evoluciona la IA, el producto puede evolucionar de forma natural con lo que es una base de datos y las nuevas tecnologias"*

El marco anterior (caminos A/B/C, ICPs, comparativas comerciales) distorsionaba las decisiones técnicas: justificaba o vetaba features con argumentos comerciales que en realidad no aplicaban (no hay clientes ni hay intención de tenerlos). El reframe permite decir las cosas como son y elegir exploraciones por **valor de aprendizaje + diferenciación honesta**, no por encaje a un ICP imaginario.

### 🛡️ Lo que NO cambia
- Cero código tocado. Motor estable como estaba.
- ADRs técnicos (0001–0006, 0008–0018) siguen vigentes. Son decisiones del motor, independientes del marco comercial.
- `STATUS.md`, `USE_CASES.md`, `SQL_REFERENCE.md`, `ARCHITECTURE.md`, `TECHNICAL_SPECS.md`, `ERROR_HANDLING.md`, `ERROR_CODES.md` siguen vigentes — describen lo que el motor **es**, no qué se vende.
- 45/45 integration + 27 lib + 7 unit tests verdes. CI sin alterar.

---

## 2026-05-18 — Vigesimosexta intervención: códigos numéricos `[GBY-NNNN]` estilo MySQL `ER_*` + catálogo operacional

> **Sin bump de formato. Sin deps añadidas.** Cierre del trabajo de manejo de errores: cada error user-facing ahora lleva un código estable y existe un catálogo operacional búscable. Análogo al sistema `ER_DUP_ENTRY=1062` de MySQL.

### ✨ Cambio
- Nuevo módulo [src/errors.rs](src/errors.rs):
  - `pub mod codes` con ~30 constantes `pub const NAME: u32 = NNNN` agrupadas por rango:
    - `1000–1999` storage / WAL / file lock
    - `2000–2999` catalog / schema / identificadores
    - `3000–3999` constraints (PK, NOT NULL, UNIQUE, FK)
    - `4000–4999` superficie SQL (parser, planner, limitaciones)
    - `5000–5999` server / HTTP / auth
  - Helper `coded(code: u32, message: impl Into<String>) -> DbError` que produce mensajes con prefijo `[GBY-NNNN]`.
  - 3 unit tests del módulo.
- Sweep de ~30 sitios user-facing en `storage.rs`, `bptree.rs`, `sql.rs`, `catalog.rs`, `index.rs`, `server.rs`: cada error visible para CLI/HTTP/embedido ahora pasa por `coded(...)`.
- Auth fallida (`401`) y server-busy (`503`) llevan códigos `[GBY-5004]` y `[GBY-5005]` respectivamente.
- Nuevo documento normativo [docs/ERROR_CODES.md](docs/ERROR_CODES.md) — catálogo operacional con cada código: causa, remedio, ejemplo de mensaje real, integración desde CLI/HTTP/Rust/Python.
- README, ERROR_HANDLING y CONTRIBUTING enlazan al catálogo.

### 🎯 Por qué este cambio
Pregunta del usuario: *"y tener un número referencial como MySQL tiene para el manejo de errores"*. Razón concreta: el texto de un mensaje puede evolucionar (mejor redacción, más contexto), pero un cliente que reacciona programáticamente al error necesita un contrato estable. El código numérico **es** ese contrato.

Ahora:
- Las herramientas pueden hacer `grep -oE 'GBY-[0-9]{4}'` para detectar la clase del error sin parsear texto humano.
- El troubleshooting tiene un eje claro: cada código apunta a su entrada en [ERROR_CODES.md](docs/ERROR_CODES.md).
- Los clientes embebidos pueden hacer `text.starts_with("[GBY-3001]")` para detectar PK duplicada sin depender de la redacción exacta.

### 🛡️ Decisión: constantes Rust, no JSON externo
Documentada en [src/errors.rs](src/errors.rs) y en la sección "Por qué constantes en Rust" del catálogo:
- Zero-deps (ADR-0001) — sin filesystem I/O al startup.
- Type-checked: el compilador detecta renames; con JSON sería un test runtime dedicado.
- Misma flexibilidad práctica: cambiar un mensaje es edit + rebuild + redeploy en cualquier caso.
- i18n futuro se resuelve con `feature` flags si llega, sin filesystem.

### 🛡️ Restricciones respetadas
- **Cero deps.** ADR-0001 intacto.
- **Cero bump de formato.** VERSION 7 sigue válido.
- **Cero rotura del contrato externo.** Los mensajes ahora prefijan con `[GBY-NNNN]`, pero los clientes que no parsean el texto (mayoría) no se ven afectados.
- **45/45 integration + 30 lib + 4 server + 3 errors unit tests verdes.**

### 📐 Documentos
- [docs/ERROR_CODES.md](docs/ERROR_CODES.md) — catálogo completo de los ~30 códigos.
- [docs/ERROR_HANDLING.md](docs/ERROR_HANDLING.md) — guía de estilo (actualizada para reflejar el nuevo sistema de códigos).

---

## 2026-05-18 — Vigesimoquinta intervención: guía canónica de manejo de errores + sweep al español + enriquecimiento

> **Sin bump de formato. Sin deps añadidas. Levanta la barra de calidad de los mensajes de error a nivel producto.** Cierra el síntoma "los errores en pantalla son pobres y no aclaran nada".

### ✨ Cambio
- Nuevo documento canónico [`docs/ERROR_HANDLING.md`](docs/ERROR_HANDLING.md) — guía normativa para los ~210 sitios donde se construyen errores en el motor:
  - Filosofía: cada mensaje responde *qué pasó*, *por qué*, y (cuando aplica) *cómo se resuelve*.
  - Reglas de estilo: idioma español, minúscula, sin punto final, incluir el nombre concreto del objeto, incluir el dato del fallo, sugerir el remedio.
  - 8 categorías canónicas (validación, NotFound, Conflict, Constraint, Limitación, Integridad, Estado interno, I/O) cada una con patrón recomendado.
  - Mapeo sistemático a HTTP (400/401/404/405/409/500/503).
  - Anti-patrones explícitos (mensajes de una palabra, `unwrap` que miente, `From` que enmascara, idiomas mezclados, secretos en mensajes).
  - Checklist de PR para revisar cualquier nuevo `DbError::new(...)`.

- **Traducción al español de todos los mensajes en inglés** heredados de iteraciones previas:
  - `storage.rs`: `tx already started` → `transacción ya iniciada`; `no active tx` → `no hay transacción activa: commit() requiere un begin() previo`; `bad magic` → `magic bytes inválidos: el archivo no es una base de datos gabysql`; `unsupported gabysql file format` → `formato de archivo gabysql no soportado`; `refusing to overwrite` → `se rehúsa sobrescribir base de datos existente`; `database is locked by another process` → `base de datos bloqueada por otro proceso`; etc.
  - `bptree.rs`: `root page is 0`, `leaf overflow`, `page too small`, `not a leaf page`, `leaf decode overflow`, `internal too large`, `unknown page type`, etc. — todos en español con contexto.
  - `server.rs`: mensajes de `read_request` (`request line vacía`, `método faltante`, `escape URL inválido`), validación de `-max-connections`, mensajes de auth/multi-DB.
  - `index.rs`: `bucket de índice corrupto` con offset, count, len y descripción precisa.

- **Enriquecimiento de mensajes pobres**. Los ~20 mensajes que eran 1-3 palabras y no orientaban al operador ahora incluyen contexto:
  - `default corrupto (kind)` → `DEFAULT corrupto: buffer agotado en offset {N} (len={M}), falta el byte de kind`.
  - `string corrupto` → `string serializado corrupto en offset {N}: header declara {L} bytes pero solo quedan {R} bytes en el buffer`.
  - `fila corrupta (INT)` → `fila corrupta en tabla '{T}': campo '{C}' (INT) necesita 8 bytes en offset {N}, solo quedan {R}`.
  - `db vacío` → `parámetro 'db' vacío: indique el nombre del archivo .db dentro del directorio configurado`.
  - `meta de tabla corrupta` → `TableMeta '{T}' corrupta: faltan bytes para el header de la columna {i} ('{C}') en offset {N}`.
  - `colisión de hash en catálogo` → mensaje completo que dice qué nombres colisionaron y que se debe reportar como bug.
  - `cantidad columnas != valores` → `INSERT INTO '{T}': cantidad de columnas ({c}) no coincide con cantidad de valores ({v})`.

- **3 tests de integración actualizados** que asertaban sobre los strings originales (`duplicate primary key`, `refusing to overwrite`, `locked`) — ahora aceptan tanto el texto en español como, por compatibilidad transicional, el inglés equivalente cuando es razonable.

### 🎯 Por qué este cambio
Auditoría con el usuario: "los errores en pantalla son pobres en indicaciones y no aclaran nada". La auditoría confirmó:
- Existía una convención **observada** pero **no escrita** sobre los mensajes.
- Muchos eran de 1-3 palabras (`db vacío`, `string corrupto`, `fila corrupta (INT)`) — imposibles de buscar en troubleshooting y sin información accionable.
- Había mezcla de español e inglés sin razón.
- Sin documento normativo, un PR podía agregar `"Column Not Found."` y nada lo paraba.

Ahora hay tres cosas concretas:
1. **Documento normativo** (`docs/ERROR_HANDLING.md`) que define qué es un mensaje aceptable.
2. **Estado actual auditado** — ~210 sitios revisados, todos cumplen las reglas.
3. **Checklist de PR** para que nuevos errores se midan contra la guía.

### 🛡️ Restricciones respetadas
- **Cero deps añadidas.** ADR-0001 intacto.
- **Cero bump de formato.** VERSION 7 sigue válido.
- **Cero rotura de API.** Los `Display::fmt` siguen devolviendo el texto puro; los clientes que no leen el texto no se ven afectados.

### 📐 Documentos
- [docs/ERROR_HANDLING.md](docs/ERROR_HANDLING.md) — guía canónica completa (las 8 categorías, checklist de PR, anti-patrones).

---

## 2026-05-18 — Vigesimocuarta intervención: ADR-0018 (Propuesta) — WAL-mode opt-in (sólo diseño)

> **Sin código. Sin bump de formato.** Cierre honesto del ítem "checkpoint del WAL" de Fase 2: el diseño queda capturado con scope, alternativas y condiciones de salida explícitas, pero la implementación se difiere hasta que aparezca medición de `gabybench` o demanda real. Justificación completa: [ADR-0018](docs/adr/0018-wal-mode-opt-in.md).

### ✨ Cambio
- Nuevo [ADR-0018](docs/adr/0018-wal-mode-opt-in.md) en estado **Propuesta**. Describe:
  - El modelo WAL-per-transaction actual y por qué "checkpoint" no aplica.
  - El modelo propuesto: WAL persistente, `Pager::checkpoint()` explícito, `wal_index` in-memory, read-path WAL-aware.
  - Alternativas evaluadas y descartadas (group commit, mmap, auto-checkpoint, etc.).
  - **Condiciones de salida** (cuándo pasa a "Aceptada" + implementación): cuando `gabybench` muestre fsync(.db) como cuello de botella, o aparezca workload write-heavy con métricas concretas, o se necesite MVCC.
- ROADMAP.md actualizado: el ítem pasa de "diferido sin condiciones" a "diseño aceptado, implementación deferida con condiciones de salida documentadas".

### 🎯 Por qué este formato
Implementar WAL-mode real es ~400-600 LOC en el hot path del Pager con riesgo de regresión alto y sin un workload medido que lo justifique. Hacerlo a ciegas para "marcar el bloque como entregado" contradice la honestidad del resto de Fase 2 (donde cada bloque mostró su scope real, no inflado).

El diseño completo es valor por sí mismo: cualquier persona futura — humana o agente — que retome el ítem encuentra el análisis listo, las alternativas evaluadas, y el contrato de cuándo activarlo. Eso es lo que se entrega.

### 📐 ADR
- [ADR-0018 — WAL-mode opt-in con checkpoint explícito](docs/adr/0018-wal-mode-opt-in.md).

---

## 2026-05-18 — Vigesimotercera intervención: índice INT-ordenado + range scan (Fase 2 — VERSION 7)

> **Bump de formato VERSION 6 → 7.** Cierra el ítem "range scan por índice secundario" del roadmap, restringido honestamente a columnas INT. Justificación completa: [ADR-0017](docs/adr/0017-int-ordered-index-version-7.md).

### ✨ Cambio
- **VERSION on-disk pasa de 6 a 7.** Archivos V6 se rechazan limpiamente al abrir (mensaje "Re-create the database with the current binary"). Igual patrón que cada bump anterior.
- **Nuevo `IndexKind`** en `IndexMeta` ([src/catalog.rs](src/catalog.rs)):
  - `Hash` (ADR-0005): el layout legacy. Usado para TEXT/FLOAT/BOOL/DATE/DATETIME. **Equality only**.
  - `OrderedInt` (nuevo): para columnas INT. El B+Tree se indexa por el valor directamente; los buckets son solo `[count:u16] + count × pk:i64`. Soporta range scan.
  - `IndexKind::for_column(column_type)` decide automáticamente al crear el índice. Cero cambios al SQL externo.
- **Nuevo path `WHERE col_idx BETWEEN a AND b`** sobre columnas INT indexadas: ejecutor llama a `lookup_pks_via_index_range` que usa `Tree::cursor_range(idx.root_page, from, to)` y devuelve los PKs en O(log N + k).
- **BETWEEN sobre columna TEXT/FLOAT/etc. indexada falla loud** con mensaje claro:
  *"el índice secundario es hash-based (equality only). Solo columnas INT-indexadas admiten BETWEEN."*
- **NULL no se almacena en índices OrderedInt**. SQL `BETWEEN` ignora NULL por definición y UNIQUE permite múltiples NULLs; ambas semánticas caen naturalmente al no indexar la representación NULL.
- Helpers nuevos en [src/index.rs](src/index.rs): `ordered_int_key_from_value_bytes`, `encode_ordered_bucket`/`decode_ordered_bucket`, `ordered_bucket_insert`/`_remove`/`_unique_conflict`.
- Integrity check ([src/sql.rs](src/sql.rs)) y FK cascade lookup branchean por `idx.kind` para decodificar el bucket correcto.
- **2 tests nuevos**: range BETWEEN sobre INT indexado (incluyendo verify que NULL queda fuera) y rechazo BETWEEN sobre TEXT indexado.

### 🎯 Por qué este cambio (y por qué INT solamente)
ADR-0005 había fijado el índice como **hash-based** (FNV-1a-64) para tolerar colisiones de hash con un bucket por clave. Equality funciona; range no compone — hashes de valores cercanos son arbitrariamente distintos. El ítem del roadmap "range scan por índice secundario" había sido marcado como **no viable bajo VERSION 6** explícitamente en intervenciones previas.

La salida natural es usar el valor como clave del B+Tree donde el orden i64 ya es el orden semántico — **solo INT** cumple sin tocar el motor. TEXT requeriría un B+Tree byte-keyed (~800+ LOC, riesgo de regresión); FLOAT necesita encoding flip-sign no-trivial. Ambos quedan diferidos a un bloque futuro cuando aparezca demanda real.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001).
- **Memoria acotada** (ADR-0009 — el bucket ordenado es estrictamente más chico que el bucket Hash equivalente).
- **Convivencia limpia**: índices Hash siguen funcionando para los tipos no-INT (ADR-0005 sigue vigente).
- **Sin cambios al cursor**: `Tree::cursor_range` (ADR-0008) ya servía perfectamente.

### 📐 ADR
- [ADR-0017 — Índice secundario INT-ordenado para range scan (VERSION 7)](docs/adr/0017-int-ordered-index-version-7.md).

### 📝 Notas
- **Índices compuestos no entran en este bloque.** El roadmap inicial los agrupaba con range scan bajo el mismo bump, pero compuestos requieren claves multi-columna que con el approach value-as-i64 es forzado. Quedan diferidos a un futuro VERSION 8 (o se entregan dentro de VERSION 7 si la demanda aparece sin necesidad de cambio de formato).

---

## 2026-05-18 — Vigesimosegunda intervención: prefetch one-leaf-ahead en `LeafCursor` (Fase 2 — performance directional)

> **Sin bump de formato. Sin deps añadidas. Mejora direccional sin medición cuantitativa todavía.** Justificación completa: [ADR-0016](docs/adr/0016-leafcursor-prefetch.md).

### ✨ Cambio
- 4 líneas nuevas en [src/bptree.rs](src/bptree.rs::LeafCursor::load_current): después de cargar la hoja actual, si hay siguiente, se hace `page_data` sobre ella para llevarla al `PageCache` (ADR-0009). Best-effort: errores de prefetch se descartan; el error real va a surgir en la próxima iteración real del cursor.
- Nuevo helper `Pager::cache_contains(page_no) -> bool` ([src/storage.rs](src/storage.rs)) para tests + futura tooling operacional.

### 🎯 Por qué este cambio
El `LeafCursor` (ADR-0008) ya hace lo correcto algorítmicamente, pero presenta al kernel y al `PageCache` un patrón de I/O **stop-and-go**: lee hoja N, deja que el caller procese 100 filas (pausa larga), entonces lee hoja N+1. Esto:
1. **Confunde el readahead del kernel**, que necesita lecturas back-to-back para detectar streaming.
2. **Garantiza un cache miss en cada leaf transition** — la primera lectura post-transición siempre paga el costo de syscall + CRC verify.

Prefetcheando la próxima hoja al final de la carga de la actual, el syscall ocurre antes y para cuando el caller la pide, ya está en cache.

### 🛡️ Honestidad sobre la mejora
- **No hay número absoluto todavía.** `gabybench` (la suite reproducible especificada en `docs/GABYBENCH_SPEC.md`) no existe aún. Cuando exista, esto se mide.
- **Sobrelectura potencial de 1 hoja en queries cortas** (`LIMIT N` que cabe en la primera hoja).
- El ADR vende esto como **directional**, no como "scan 2x más rápido".

### 📐 ADR
- [ADR-0016 — Prefetch one-leaf-ahead en `LeafCursor`](docs/adr/0016-leafcursor-prefetch.md).

---

## 2026-05-18 — Vigesimoprimera intervención: backup/restore/verify con validación end-to-end (Fase 2 — operación)

> **Sin bump de formato. Sin deps añadidas.** Cierra el gap operacional "no hay forma confiable de respaldar". Justificación completa: [ADR-0015](docs/adr/0015-verified-backup-restore.md).

### ✨ Cambio
- Nuevo módulo [src/backup.rs](src/backup.rs) con tres entradas públicas: `backup`, `restore`, `verify`. Todas validan **CRC32 página por página en lectura** y, post-escritura, **re-abren el destino y revalidan cada página**. Si una sola página falla el CRC en cualquiera de las dos fases, la operación aborta — nunca se publica un backup roto.
- Nuevos subcomandos CLI:
  - `gabysql backup [--force] <src.db> <dst.db>`
  - `gabysql restore [--force] <src.db> <dst.db>` (alias semántico)
  - `gabysql verify <file.db>`
- Salida estructurada: `OK backup  src=...  dst=...  pages=N  bytes=M`.
- 3 tests de integración nuevos: round-trip con verify, detección de corrupción en origen (byte flip rechaza el backup), verify sobre DB sana.

### 🎯 Por qué este cambio
La operación de respaldo era "`cp demo.db backups/demo.db.bak`" — sin validación, sin awareness del WAL, sin garantía de que el destino se pudiera *usar*. Una página corrupta en el origen se replicaba al backup sin warning hasta que alguien intentaba restaurar (semanas después, en una emergencia).

Ahora el contrato es claro:
- Si el comando termina con `OK`, el archivo destino se puede abrir con el mismo binario, todas sus páginas tienen CRC válido, y su header coincide con el origen.
- Si algo falla, error explícito que apunta a la página corrupta o la causa raíz.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001 intacto).
- **Cero bump de formato.** VERSION = 6 sigue válido — el destino es un `.db` regular.
- **Lock exclusivo** vía ADR-0013: la DB debe estar cerrada por otros procesos (server apagado). Endpoint server-side `/backup` que tome el `write_lock` queda para Fase 3.

### 📐 ADR
- [ADR-0015 — Backup / restore / verify con validación end-to-end](docs/adr/0015-verified-backup-restore.md).

### 📝 Ejemplo
```powershell
# Cierre el server primero (el lock exclusivo bloquea backups online)
gabysql backup demo.db backups/demo.db.bak
# → OK backup  src=demo.db  dst=backups/demo.db.bak  pages=128  bytes=524288

# Verificar un backup antiguo
gabysql verify backups/demo.db.bak
# → OK verify  path=backups/demo.db.bak  pages=128  bytes=524288

# Restaurar
gabysql restore --force backups/demo.db.bak demo.db
```

---

## 2026-05-18 — Vigésima intervención: logs JSON + endpoint `/metrics` en el server (Fase 2 — observabilidad)

> **Sin bump de formato. Sin deps añadidas.** Primer paso de observabilidad operacional para `gabysql-server`. Justificación completa: [ADR-0014](docs/adr/0014-logs-json-metrics.md).

### ✨ Cambio
- Nuevo struct `Metrics` en [src/server.rs](src/server.rs): contadores por status HTTP, `errors_total` (status ≥ 500), y ring buffer acotado de 1024 latencias para p50/p95. Memoria O(1) bajo carga sostenida.
- Nuevo endpoint **`GET /metrics`**:
  ```json
  {"ok":true,"started_unix":...,"uptime_s":3600,"requests_total":1234,
   "requests_by_status":{"200":1180,"400":30,"500":24},
   "errors_total":24,
   "latency_ms":{"p50":5,"p95":87,"samples":1024,"count":1234}}
  ```
  Gated por `-token` igual que el resto de la API.
- Nuevo flag **`-log-json`** en `gabysql-server`. Cuando se activa, cada request finalizado emite una línea JSON a stdout:
  ```json
  {"ts_unix":1747497612,"method":"POST","path":"/exec","status":200,"latency_ms":12}
  ```
  Por defecto **off** — la UX del binario silencioso de hoy no cambia. Útil con `tee`, `jq`, ingest a S3/ELK/Loki.
- 4 tests unitarios nuevos: registro de status + latencia, percentiles sobre 1..=100, comportamiento con buffer vacío, ring buffer acotado bajo overflow.

### 🎯 Por qué este cambio
El binario en producción era opaco: sin logs por request, sin contadores agregados, sin forma de responder "¿cómo se está comportando bajo carga?". El RUNBOOK pedía observabilidad básica pero no había nada que pedirle al server más allá de `/health`.

Ahora cualquier operador puede:
- Curl `/metrics` y ver counts por status + p50/p95 inmediatamente.
- Activar `-log-json` y pipear a `jq '. | select(.latency_ms > 100)'` para encontrar requests lentas.
- Configurar una alerta sobre `errors_total` creciendo.

Y todo sin agregar una sola dependencia.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001 intacto). Sin `tracing`, sin `prometheus`, sin `metrics-rs`.
- **Memoria acotada** (ADR-0009 mismo principio). Ring buffer de 1024 × 4 bytes = 4 KB por server.
- **Opt-in** para logs. Defaults preservan la UX silenciosa.
- **Sin bump de formato**. VERSION = 6 sigue válido.

### 📐 ADR
- [ADR-0014 — Logs JSON estructurados + endpoint `/metrics` en el server](docs/adr/0014-logs-json-metrics.md).

---

## 2026-05-18 — Decimonovena intervención: lock exclusivo cross-process sobre el `.db` (Fase 2 — concurrencia)

> **Sin bump de formato. Sin deps añadidas.** Cierra el gap de corrupción silenciosa cuando dos procesos abren la misma DB. Justificación completa: [ADR-0013](docs/adr/0013-process-level-file-lock.md).

### ✨ Cambio
- Nuevo helper privado `acquire_db_lock(&File, &Path)` en [src/storage.rs](src/storage.rs) que llama `File::try_lock()` (advisory exclusivo, **estable desde Rust 1.89.0**).
- Aplicado en `Pager::create` / `Pager::create_force` / `Pager::open`: el lock se adquiere tras abrir el handle y antes de cualquier escritura o replay del WAL.
- `Pager::close` libera el lock explícitamente con `file.unlock()` (drop del `File` también lo libera como red de seguridad).
- Si otro proceso (o incluso otro `Pager` en el mismo proceso) ya tiene la DB tomada, la segunda apertura **falla rápido** con:
  ```
  database is locked by another process: <path>.
  Close the other gabysql process or wait for it to release the lock.
  ```
  No hay espera bloqueante, no hay cuelgue.
- Test nuevo `cross_process_lock_rejects_second_open` que valida: primer `Pager::create` toma el lock → `Pager::open` segundo falla con mensaje claro → `close` del primero libera → `Pager::open` tercero funciona.

### 🎯 Por qué este cambio
La WAL+CRC de `gabysql` asume **un único escritor por archivo**. Sin lock cross-process, dos `gabysql` apuntando al mismo `.db` (server + CLI accidental, server reiniciado con proceso huérfano vivo, etc.) escribían páginas en paralelo y corrompían el archivo. El motor detectaba la corrupción **después** vía CRC, pero el daño ya estaba hecho.

Ahora la corrupción por doble apertura es **imposible**: el segundo proceso no llega a tocar el archivo.

### 🛡️ Restricciones respetadas
- **Cero deps** (ADR-0001 intacto). Uso exclusivo de `std::fs::File::try_lock` / `unlock`.
- **Cero bump de formato** (VERSION = 6 sigue válido).
- **Cross-platform**: Windows (`LockFileEx` bajo el capó), Linux (`flock(2)` advisory), macOS (`flock(2)`). Los tres validados en CI.
- **No-bloqueante**: `try_lock` falla inmediatamente; el caller decide qué hacer.

### 📐 ADR
- [ADR-0013 — Lock exclusivo a nivel de proceso sobre el archivo `.db`](docs/adr/0013-process-level-file-lock.md).

### 📝 Notas de roadmap
- Re-evaluado el ítem **"checkpoint/compaction del WAL"** de Fase 2: el WAL actual es per-transaction y se trunca/borra en cada commit (no acumula a través de commits), así que el concepto clásico de checkpoint no aplica sin un cambio previo a WAL persistente. Diferido hasta que aparezca demanda concreta.
- Re-evaluado el ítem **"range scan por índice secundario"**: el índice 2º actual es hash-based (FNV-1a-64, ADR-0005) y no admite range nativo. Agrupado con índices compuestos bajo un futuro bump VERSION 6 → 7 que reestructurará el índice a B+Tree ordenado.

---

## 2026-05-08 — Decimoctava intervención: audit log enriquecido en el gateway (Fase 5 — AI-native, cierre del trío)

> **Sin bump de formato. Sin cambios al motor.** Tercera y última pieza del trío AI-native sobre el gateway. Justificación completa: [ADR-0012](docs/adr/0012-audit-log-enriquecido.md).

### ✨ Cambio
- Nuevo flag `--audit-log <ruta>` (también `GABYSQL_AUDIT_LOG`) en [src/bin/gabysql-mcp.rs](src/bin/gabysql-mcp.rs). Si no se pasa, sin log y overhead cero.
- Nuevo argumento opcional `reason` en `gabysql_execute`: el "por qué" semántico que el agente puede pasar para que quede en el audit.
- Captura de `clientInfo` (`name` + `version`) en el handshake `initialize` → guardado en `RuntimeState` interno y emitido en cada entrada del log.
- Cada llamada a `gabysql_execute` y `gabysql_integrity_check` anexa una línea JSON al archivo (formato JSONL):
  ```json
  {"ts_unix":1730000000,"tool":"gabysql_execute","db":"rag.db",
   "sql":"INSERT INTO docs ...","reason":"backfill inicial del corpus",
   "client":{"name":"claude-desktop","version":"1.2.3"},
   "ok":true,"error":null}
  ```
- Nueva tool **`gabysql_audit_tail(n)`** que devuelve las últimas N entradas. Permite que **el propio agente** revise su historial dentro de la sesión. Si el log no está activo, devuelve `{"enabled":false,"entries":[]}` sin error.
- Append best-effort: si escribir al archivo falla, va a stderr y la tool sigue devolviendo el resultado del motor (mejor perder una entrada que bloquear escrituras por disco lleno).
- 5 tests nuevos: captura de clientInfo, append+tail roundtrip con `reason`+`client`, comportamiento con log desactivado, presencia de `gabysql_audit_tail` en `tools/list`, formato JSONL (una entrada por línea, JSON válido por línea).

### 🎯 Por qué este cambio
Cuando un agente puede escribir en una base, el log del motor responde **el qué** (qué SQL corrió) pero no **el por qué** (qué pidió el usuario, qué identidad tenía el agente, qué razonamiento lo llevó allí). Meter eso en el motor implica bump de formato y que el motor entienda conceptos MCP que no le pertenecen.

Mover el audit al gateway captura el "por qué" exactamente donde el conocimiento existe — el gateway ya sabe quién es el cliente, qué tool se invocó, qué `reason` pasó el agente. Y cierra el loop dándole al propio agente la tool para releer sus acciones. Eso permite patrones de auto-corrección dentro de la misma sesión.

### 🛡️ Cómo se respeta el motor
- **Cero líneas tocadas en `storage.rs`/`bptree.rs`/`sql.rs`/`catalog.rs`/`server.rs`/`lib.rs`.** Solo crece `src/bin/gabysql-mcp.rs`.
- **Sin bump de formato.** Sin nuevas deps. `Cargo.toml`/`Cargo.lock` sin tocar.
- **Opt-in puro.** Sin `--audit-log` el comportamiento es idéntico al gateway pre-ADR — ni un syscall extra.
- **Retrocompatible**: clientes MCP que no pasan `reason` siguen funcionando sin cambios.

### 📐 ADR
- [ADR-0012 — Audit log enriquecido en el gateway, no en el motor](docs/adr/0012-audit-log-enriquecido.md). Cierra el trío con [ADR-0010](docs/adr/0010-mcp-gateway.md) (gateway base) y [ADR-0011](docs/adr/0011-vector-search-gateway-side.md) (vectores).

### 🧪 Ejemplo de uso desde un agente MCP
```bash
# Server + gateway con audit activo
gabysql-server -dir ./dbs -token MI_TOKEN
gabysql-mcp --token MI_TOKEN --audit-log /var/log/gabysql/agent-audit.jsonl
```
```json
{ "method":"tools/call", "params":{
    "name":"gabysql_execute",
    "arguments":{
      "db":"rag.db",
      "sql":"UPDATE users SET email='nuevo@x.com' WHERE id=42",
      "reason":"el usuario reportó que su email anterior ya no funciona"
}}}
```
La línea correspondiente del JSONL queda con `reason`, `client`, `sql`, `ok`. Procesable con `jq '.[] | select(.tool=="gabysql_execute")'` o ingestable a cualquier sink.

---

## 2026-05-07 — Decimoséptima intervención: búsqueda vectorial del lado del gateway (Fase 5 — AI-native, parte 2)

> **Sin bump de formato. Sin cambios al motor.** Esta intervención añade búsqueda vectorial top-k a `gabysql-mcp`. Los vectores se guardan como `TEXT` (`'[0.1,0.2,...]'`); el cómputo ocurre en el binario del gateway. Justificación completa: [ADR-0011](docs/adr/0011-vector-search-gateway-side.md).

### ✨ Cambio
- Nueva tool MCP **`gabysql_vector_search`** en [src/bin/gabysql-mcp.rs](src/bin/gabysql-mcp.rs):
  - Args: `db?`, `table`, `pk_column?` (default `id`), `vector_column`, `query: number[]`, `top_k?` (default 10), `metric?` (default `cosine`).
  - Métricas: `cosine`, `euclidean`/`l2`, `dot`/`ip`.
  - Hace `SELECT <pk>, <vec_col> FROM <table>` vía el HTTP existente, parsea cada vector, computa la distancia y devuelve top-k por heap selection.
  - Identificadores validados con `safe_ident` (regex implícito `[A-Za-z_][A-Za-z0-9_]*`) antes de interpolar al SQL — bloquea inyección.
  - Filas con vector mal formado o de dimensión distinta a la query van al campo `skipped` de la respuesta (no se silencian).
- 9 tests unitarios nuevos: cosine identity/orthogonal, euclidean Pitágoras, dot con sort ascendente, dimension mismatch, vector cero, top-k heap, validador de identificadores (acepta válidos / rechaza inyección), aliases de métrica, schema visible en `tools/list`.

### 🎯 Por qué este cambio
La búsqueda vectorial es lo que la mayoría de agentes LLM espera de una "DB para los nuevos tiempos". El camino correcto a largo plazo es un tipo `VECTOR(n)` nativo con índice ANN — pero eso requiere bump de formato, cambios profundos en `sql.rs`/`storage.rs`/`bptree.rs`, y meses de trabajo. **Hacerlo "para validar el use case" es prematuro.**

Esta entrega resuelve el 80% del valor (top-k usable hoy desde cualquier cliente MCP) con el 5% del riesgo (cero líneas tocadas en el motor). El ADR-0011 documenta las **condiciones de salida explícitas** para promover a `VECTOR(n)` nativo cuando la señal aparezca: dataset > 100K vectores, demanda de operadores SQL, o necesidad de índice ANN.

### 🛡️ Cómo se respeta el motor
- **No se toca `Cargo.toml`/`Cargo.lock`.** Sin nuevas deps. ADR-0001 intacto.
- **No se toca `src/lib.rs` ni ningún archivo del motor.** Solo crece `src/bin/gabysql-mcp.rs`.
- **No se cambia el formato en disco.** Los vectores son `TEXT`; `INSERT INTO docs (id, content, embedding) VALUES (1, 'texto', '[0.1,0.2,...]')` es SQL estándar que el motor procesa sin saber que es un vector.
- **Storage existente sigue válido.** DBs viejas no requieren migración.

### 📐 ADR
- [ADR-0011 — Búsqueda vectorial del lado del gateway, no en el motor](docs/adr/0011-vector-search-gateway-side.md)

### 🧪 Ejemplo de uso desde un agente MCP
```json
{ "method": "tools/call", "params": {
    "name": "gabysql_vector_search",
    "arguments": {
      "db": "rag.db",
      "table": "docs",
      "vector_column": "embedding",
      "query": [0.12, -0.04, 0.88, /* ... */],
      "top_k": 5,
      "metric": "cosine"
    }
} }
```

---

## 2026-05-07 — Decimosexta intervención: gateway MCP — `gabysql-mcp` (apertura Fase 5 AI-native)

> **Sin bump de formato. Sin cambios al motor.** Esta intervención añade un binario nuevo (`gabysql-mcp`) que es cliente del `gabysql-server` HTTP/JSON existente. No abre el `.db`, no instancia un `Pager`, no toca `storage.rs` / `bptree.rs` / `catalog.rs` / `sql.rs`. El motor queda intacto. Justificación completa: [ADR-0010](docs/adr/0010-mcp-gateway.md).

### ✨ Cambio

- Nuevo binario `src/bin/gabysql-mcp.rs` (~700 líneas, **cero dependencias externas**) que habla el protocolo **MCP (Model Context Protocol)** sobre stdio (JSON-RPC 2.0 delimitado por `\n`).
- Cinco tools expuestas a cualquier cliente MCP-compatible (Claude Desktop, Claude Code, Cursor, etc.):
  - `gabysql_list_databases` → wrap de `GET /dbs`
  - `gabysql_describe_database` → wrap de `GET /tables[?db=…]`
  - `gabysql_query` → wrap de `POST /exec` para `SELECT`/`SHOW`/`DESCRIBE`
  - `gabysql_execute` → wrap de `POST /exec` para `INSERT`/`UPDATE`/`DELETE`/DDL (omitida si se lanza con `--read-only`)
  - `gabysql_integrity_check` → wrap de `POST /exec` con `INTEGRITY CHECK`
- Dos resources MCP:
  - `gabysql://catalog` → lista de bases disponibles
  - `gabysql://schema/{db}` → schema completo de una DB
- Flags: `--server URL` (default `http://127.0.0.1:7878`, también `GABYSQL_SERVER`), `--token T` (también `GABYSQL_TOKEN`), `--read-only`.
- Tests unitarios en el mismo archivo cubren: parser JSON (round-trip + escapes), `initialize`, `tools/list` con y sin `--read-only`, `resources/list`, `ping`, método desconocido, notifications sin id, parsing de URL del server.

### 🎯 Por qué este cambio

El consumidor que más rápido crece en el ecosistema es el agente LLM. Hoy una IA que quiera usar `gabysql` necesita: cliente HTTP a mano + token + el schema de la DB metido en el prompt + reintentos sobre errores SQL sin trazabilidad. Ese pegamento se reescribe en cada integración.

MCP es el estándar emergente que define cómo un servidor expone *tools* y *resources* a clientes-agentes. Si `gabysql` lo habla de fábrica, cualquier agente lo enchufa directo:

```bash
gabysql-server -dir ./dbs -token MI_TOKEN
gabysql-mcp --server http://127.0.0.1:7878 --token MI_TOKEN
# Claude Desktop / Claude Code / Cursor lanzan gabysql-mcp como subprocess
# y descubren las 5 tools + 2 resources sin código de pegamento.
```

### 🛡️ Cómo se respeta el motor

- **No se toca `Cargo.toml`.** El binario se auto-descubre desde `src/bin/`. `Cargo.lock` no añade un solo paquete.
- **No se cambia `[lib]`.** Sigue compilando con cero deps externas. [ADR-0001](docs/adr/0001-rust-zero-deps-core.md) intacto.
- **No se abre el `.db`.** El gateway hace doble salto stdio→HTTP→Pager, así heredas todo lo que ya está endurecido en `server.rs`: `write_lock` global, tope de conexiones, bearer token, CORS preflight, validación de SQL antes de pegar al Pager.
- **No se cambia el formato en disco.** Sin bump de VERSION, sin nuevo tipo de página, sin cambio en el WAL.

### ✅ Tests
- Módulo `#[cfg(test)] mod tests` en `src/bin/gabysql-mcp.rs`: 9 tests cubren parser JSON, dispatch JSON-RPC y semántica de `--read-only`. CI multi-OS los ejecuta vía `cargo test`.

### 📐 ADR
- [ADR-0010 — Gateway MCP como adaptador externo sobre el HTTP/JSON existente](docs/adr/0010-mcp-gateway.md): promovida de 🟡 Propuesta a ✅ Aceptada con la implementación.

---

## 2026-05-08 — Decimoquinta intervención: `PageCache` LRU acotado — cierra fuga de memoria del server

> **Sin bump de formato.** El cambio es interno al Pager. La API pública del Pager se mantiene compatible salvo dos métodos nuevos (`set_cache_capacity`, `cache_len`, `cache_capacity`).

### ✨ Cambio
- Reemplazo de `cache: BTreeMap<u32, CachedPage>` (que crecía sin límite) por `cache: PageCache` con **capacidad fija** + **eviction LRU sobre páginas clean**.
- Constante `DEFAULT_CACHE_PAGES = 1024` (~4 MB por DB con páginas de 4 KB). Configurable por instancia con `Pager::set_cache_capacity(n)`.
- LRU implementada con `HashMap<u32, CacheSlot>` + contador monótono (touch en cada `get/get_mut/insert`). Eviction = scan O(N) sobre el map cuando está lleno; para 1024 entradas son µs por inserción.
- Política dirty-aware: **las páginas dirty nunca se evictan** — pertenecen a la transacción abierta y deben llegar al WAL antes de poder dropearse. Si el cache llega a capacidad lleno de dirty, se permite overflow temporal: perder una página dirty corromperia la DB. El overflow drena solo en el commit (todas pasan a clean simultáneamente).

### 🎯 Por qué este cambio

**Pre-bloque-10:**
```rust
struct Pager {
    cache: BTreeMap<u32, CachedPage>,  // ← crece sin freno
}
```
Un `INTEGRITY CHECK` o un `SELECT` con full scan sobre una DB de 200 MB cargaba ~50 K páginas en RAM y **nunca las liberaba**. En `gabysql-server -dir ./dbs` con 50 DBs activas y un sweep operacional periódico, la memoria del server crecía a 10 GB y eventualmente lo mataba el OOM killer. Sin error, sin warning, sin recovery — solo `kill` y reiniciar.

**Post-bloque-10:**
```rust
struct PageCache {
    capacity: usize,                       // bounded
    map: HashMap<u32, CacheSlot>,
    counter: u64,                          // monotonic for LRU
}
```
Memoria del server acotada por `cache_capacity × #DBs_abiertas × page_size`. Para 50 DBs × 1024 páginas × 4 KB = **200 MB max**, predecible, no swappea.

### 🛡️ Comportamiento bajo casos edge
- **Workload chico con cache vacío**: idéntico a antes (cache nunca se llena, no evicta nada).
- **Workload grande de read-only**: evicta clean pages LRU. La página menos usada se cae; si vuelve a pedirse, se relee de disco con CRC verificado (mismo path que cold load).
- **Mid-transaction con muchas writes**: dirty pages se acumulan; clean pages preexistentes se evictan primero. Si el commit se retrasa y entra más dirty que cap, el cache excede cap **temporalmente** (correctness > strict cap). Drena en commit.
- **Rollback**: `cache.clear()` libera todo (mismo path que antes).

### 🧪 Validación
- 39/39 tests de integración (1 nuevo: `page_cache_is_bounded_and_evicts_clean_pages` siembra 200 filas, abre con `set_cache_capacity(4)`, recorre cada página de la DB y asserta que `cache_len() <= 4`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 🔭 `Transaction` (Unit of Work) — pospuesto a bloque futuro
La recomendación original de bloque 10 incluía un objeto `Transaction` que reemplazara las 40+ aperturas de `Catalog::open(self.pager)` por una unit-of-work compartida con cache de `TableMeta`. Después de medir el impacto real:
- La fuga de memoria del cache es **inmediata** (problema agudo del server).
- La memoización de `TableMeta` es **marginal** (lookup hash + decode = µs; el ahorro existe pero no aparece en profiles de workloads reales).
- El refactor de 40 sitios cuesta ~1500 líneas y rompe muchos diffs en revisiones.

Decisión: **se entrega solo el `PageCache` LRU en este bloque**. El `Transaction` queda como propuesta independiente con su propio análisis cuando aparezca un workload que lo justifique (ej. INSERT masivo medido).

---

## 2026-05-08 — Decimocuarta intervención: `LeafCursor` (Iterator pattern) — Fase 2 paso 2

> **Sin bump de formato.** El cambio es estructural: cómo se leen los rows del B+Tree.

### ✨ Cambio
- Nuevo `bptree::LeafCursor<'a>` que implementa `Iterator<Item = DbResult<KeyValue>>` y carga páginas leaf **on-demand** vía la chain `next` del B+Tree.
- Constructores en `Tree`: `cursor_full(root)` (full scan en orden de PK) y `cursor_range(root, from, to)` (range scan inclusive en ambos extremos).
- Wrappers en `Catalog`: `scan_cursor(root)` y `range_cursor(root, from, to)` para el caller del SQL layer.
- `exec_select` reescrito: cuando NO hay `ORDER BY`, los planes `FullScan` y `Range` consumen el cursor con `.skip(offset).take(limit)` en vez de materializar todo el B+Tree. Cuando hay `ORDER BY`, sigue materializando (necesita ordenar antes de window).

### 🎯 Impacto medible en recursos
- `SELECT … LIMIT N` sobre tabla de N filas pasa de O(filas_totales) memoria + IO a **O(N + offset)** memoria + IO. Verificable: el test `cursor_limit_returns_only_requested_rows` sobre 1.000 filas valida que `LIMIT 5` devuelve solo 5 PKs en orden, sin intermediarios.
- `SELECT … WHERE pk BETWEEN a AND b LIMIT N` corta el walk apenas la PK supera `b`, sin tocar páginas ulteriores.
- `Plan::ByPks` (path de índice secundario) sigue materializando — está acotado por la cardinalidad del lookup, no por el tamaño de la tabla.

### 🛡️ Borrow semantics (intencionales)
El cursor toma `&mut Pager` por su lifetime. Mientras está vivo, ninguna otra escritura puede pasar por el mismo Pager. Eso es lo correcto para SELECT (read-only) y por eso solo lo usa `exec_select`. Los call sites que necesitan leer Y mutar el mismo B+Tree (`CREATE INDEX` backfill, `INTEGRITY CHECK`, `delete_with_cascade`) siguen usando los helpers materializadores (`scan / range / all`); ahí la materialización es correcta porque la lectura tiene que terminar antes que la escritura empiece.

### 🧪 Validación
- 38/38 tests de integración (1 nuevo: `cursor_limit_returns_only_requested_rows` ejercita LIMIT/OFFSET y BETWEEN sobre 1.000 filas).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Decimotercera intervención: crash tests dirigidos (Fase 1 reabierta y cerrada del todo)

> **Sin bump de formato.** Solo nuevos tests de integración que ejercitan el path WAL→file con escenarios de crash sintéticos.

### 🧪 Crash recovery scenarios cubiertos
Los tests no matan procesos — sintetizan en disco el estado que un `kill -9` dejaría en cada momento crítico del flujo de `Pager::commit`:

1. **`crash_recovery_partial_file_restored_from_wal`** — kill después del WAL flush + COMMIT marker pero antes de tocar el data file. Trunca el data file al header y verifica que el reopen replica las páginas del WAL y el `SELECT` devuelve los datos completos.
2. **`crash_recovery_wal_without_commit_is_ignored`** — kill antes del COMMIT marker (transacción no durable). Forja un WAL con páginas pero sin marker; verifica que el reopen NO replica nada y los datos previos quedan intactos.
3. **`crash_recovery_replay_is_idempotent`** — kill durante los writes al data file con WAL ya flusheado. Re-planta el mismo WAL después de un replay exitoso y verifica que un segundo replay converge al mismo estado (no double-counting, no corrupción).

### 🎯 Cierre definitivo de Fase 1
Esto cubre el ítem "crash tests dirigidos (kill -9 entre WAL y file flush)" que quedaba pendiente en el [ROADMAP](../ROADMAP.md). Fase 1 (Robustez funcional) queda 100% entregada y demostrada con tests reproducibles.

### 🧪 Validación
- 37/37 tests de integración (3 nuevos).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Duodécima intervención: `ORDER BY` (Fase 2 paso 1)

> **Sin bump de formato.** Todo el ordering ocurre en memoria sobre el resultado del scan/range/index path.

### ✨ Funcionalidad SQL
- **`SELECT ... ORDER BY <col> [ASC|DESC]`**. ASC es el default cuando se omite la dirección. Va entre `WHERE` y `LIMIT/OFFSET`.
- Funciona sobre **cualquier columna** del schema (no requiere índice). Reusa el scan/range/index path existente y ordena el resultado en memoria.
- **NULLs sortean primero** bajo ASC (consistente con SQLite). En DESC quedan al final por reverse.
- Comparación tipada: INT/INT, FLOAT/FLOAT, mixto INT↔FLOAT (promueve a f64), BOOL (false<true), TEXT/DATE/DATETIME/JSON por byte order.

### 🧱 Cambios estructurales
- `SelectStmt.order_by: Option<OrderClause>` con `OrderClause { column, direction: OrderDir }`.
- Cuando `order_by` está set, el executor difiere `LIMIT/OFFSET` hasta después del sort para no truncar prematuramente.
- Nuevo helper `compare_values(Option<&Value>, Option<&Value>) -> Ordering` con NULL-first semantics.
- Validación pre-I/O: `ORDER BY` sobre columna inexistente devuelve error explícito.
- Reserved words extendidas: `order`, `by`, `asc`, `desc`.

### 🧪 Validación
- 34/34 tests de integración (4 nuevos: `order_by_int_asc_desc`, `order_by_text_with_limit_offset_window`, `order_by_nulls_sort_first_under_asc`, `order_by_unknown_column_rejected`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Undécima intervención: gabymodeler v2 (PowerDesigner-style) + CORS

> **Sin bump de formato.** El motor no cambia; el modeler reescrito y el server gana headers CORS para que el modeler pueda hablarle directo.

### 🌐 gabymodeler v2 (`web/modeler/`)
Reescritura completa del modelador, espejo del motor `VERSION 6`:
- **Layout PowerDesigner-style**: header de toolbar + Object Browser izquierdo (árbol DB > Tables > columnas con badges PK/NN/UN/FK + sección Indexes) + Canvas central + Result List inferior colapsable + Status bar.
- **Schema editor**: cada columna lleva flags inline `PK / NN / UN / FK` y un input `default` editable. PK fuerza INT + NOT NULL automáticamente. FK abre un mini-modal para elegir tabla, columna PK del target y `ON DELETE RESTRICT|CASCADE`.
- **Check Model** continuo (14 reglas): PK ausente / duplicada / no INT, columna duplicada, identificador inválido o reservado (espejo de `catalog::RESERVED_WORDS`), `NOT NULL + DEFAULT NULL`, `DEFAULT` sobre PK, UNIQUE sobre JSON, FK rota / con type mismatch / target no-PK, etc. Cada hallazgo es clickeable y selecciona la entidad/columna en canvas + browser.
- **SQL Preview en vivo** (sin abrir modal). El emit ordena tablas topológicamente (parents antes que children) y emite todas las constraints inline (`PRIMARY KEY`, `NOT NULL`, `UNIQUE`, `DEFAULT <literal>`, `REFERENCES ... ON DELETE ...`) — DDL fiel al motor `VERSION 6`.
- **↘ Importar de gabysql**: dialog que pide URL del server, token opcional y nombre de DB; consume `GET /tables?db=<db>` y reconstruye entidades + columnas + constraints + FKs desde la respuesta enriquecida del bloque 3. Reverse engineering one-shot.
- **Migración v1 → v2 automática**: si encuentra `gabymodeler.v1` en localStorage, lo lee y produce un `gabymodeler.v2` con las constraints en blanco (los flags se editan a mano).
- **FK lines**: SVG Bezier con marker arrow; `CASCADE` se dibuja sólida, `RESTRICT` punteada.

### 🔓 CORS en `gabysql-server`
- Toda respuesta lleva `Access-Control-Allow-Origin: *`, `Access-Control-Allow-Methods: GET, POST, OPTIONS`, `Access-Control-Allow-Headers: Authorization, Content-Type, X-Gabysql-Token` y `Access-Control-Max-Age: 600`.
- El método `OPTIONS` se contesta con `204 No Content` antes de cualquier auth — los preflights del navegador no llevan credenciales y rechazarlos rompería el modeler en cross-origin.
- También se agregaron `204 No Content` y `503 Service Unavailable` al mapa de status text del response writer.

### 🧪 Validación
- 30/30 tests de integración siguen verdes (no se agregaron tests de modeler — es UI vanilla).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 📋 web/modeler/README.md
Reescrito para el layout v2 y el flujo con reverse engineering.

---

## 2026-05-07 — Décima intervención: `INTEGRITY CHECK` (cierre de Fase 1)

> **Sin bump de formato.** El comando es de solo lectura — no toca el catálogo ni los datos.

### ✨ Funcionalidad SQL
- **`INTEGRITY CHECK;`** — barre la DB abierta y devuelve un ResultSet con una fila por hallazgo. Columnas: `kind`, `object`, `detail`. El campo `message` resume con `OK · N tablas · M filas · K índices · F FKs · P páginas` o `FAIL · ...` según el caso.

### 🔍 Qué chequea
1. **CRC de cada página**: itera de `0..page_count` haciendo `Pager::page_data`. Cualquier falla del CRC se reporta como `kind=page_corrupt`.
2. **Decodificación de cada fila**: `decode_row` corre sobre cada fila de cada tabla. Falla → `kind=row_decode`.
3. **Índices secundarios**: walks every bucket de cada índice y verifica que cada `(value_bytes, pk)` apunte a una PK que efectivamente existe en la tabla. Si no → `kind=orphan_index_entry`.
4. **FOREIGN KEYs**: para cada columna con `references`, verifica que el parent table exista (sino `fk_target_missing`) y que cada valor no nulo de la columna tenga su parent row (sino `fk_orphan`).

### 🧱 Cambios estructurales
- Nuevo `Statement::IntegrityCheck` y método `Engine::exec_integrity_check`.
- Reserved words extendidas: `integrity`, `check`.
- Sin cambios al on-disk format ni al catálogo.

### 🧪 Validación
- 30/30 tests de integración (2 nuevos: `integrity_check_clean_db_returns_ok`, `integrity_check_reports_corrupted_page`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

### 🎯 Cierre de Fase 1 (Robustez funcional)
Con este bloque, los 5 ítems de Fase 1 del [ROADMAP](../ROADMAP.md) están entregados:
- ~~`UPDATE`/`DELETE` por PK~~
- ~~Checksums por página + WAL~~
- ~~`NOT NULL` / `DEFAULT` / `UNIQUE`~~
- ~~`FOREIGN KEY` + `ON DELETE` enforced~~
- ~~`INTEGRITY CHECK` operacional~~

El motor está listo para empezar a sumar features de Fase 2 (índices compuestos, range scan secundario, `ORDER BY`) o para una primera publicación con SLAs de durabilidad medibles.

---

## 2026-05-07 — Novena intervención: FOREIGN KEY enforced (Camino A · paso 5)

> **On-disk format jump: VERSION 5 → 6.** `Column` ahora persiste un FK opcional `(target_table, target_column, on_delete)`. DBs v5 son rechazadas explícitamente al abrir.

### ✨ Funcionalidad SQL
- **`REFERENCES <table>(<column>) [ON DELETE RESTRICT|CASCADE]`** como constraint de columna en `CREATE TABLE` y `ALTER TABLE ADD COLUMN`. Default `RESTRICT` cuando se omite `ON DELETE`.
- **Validación al DDL**: target table debe existir (o ser self-ref a la tabla siendo creada), target column debe ser la PK del target, tipos deben coincidir (en esta versión ambos son siempre `INT`).
- **Enforcement en `INSERT`**: cada FK no nula chequea que exista la fila parent. Self-FK que apunta al PK que se está insertando se acepta (caso CEO/manager-de-sí-mismo).
- **Enforcement en `UPDATE`**: solo se revalidan FKs cuyo valor cambió.
- **Enforcement en `DELETE`**:
  - `RESTRICT` (default) aborta el DELETE si existe alguna fila hija; sin efectos colaterales.
  - `CASCADE` borra las hijas iterativamente (worklist con `visited` set sobre `(tabla, pk)` para cortar ciclos), incluyendo sus entradas en índices secundarios.
- **Self-references** soportadas (`employee.manager_id REFERENCES employee(id)`).

### 🧱 Cambios estructurales
- `catalog::ForeignKeyMeta { table, column, on_delete: OnDelete }` con `OnDelete::{Restrict, Cascade}`.
- `Column.references: Option<ForeignKeyMeta>` persistido bajo flag `0x04 = HAS_FK`.
- `RESERVED_WORDS` extendido con `foreign`, `references`, `cascade`, `restrict`.
- Helpers nuevos en `sql.rs`: `validate_fk_targets`, `check_fk_value`, `enforce_fk_on_insert`, `enforce_fk_on_update`, `find_child_pks_with_fk_value`, `delete_with_cascade`.
- `find_child_pks_with_fk_value` usa el índice secundario sobre la columna FK si existe; cae en full scan si no — recomendación documentada de indexar columnas FK para DELETEs O(log n).
- `exec_delete` simplificado: chequea existencia y delega en `delete_with_cascade`, que maneja índices secundarios + cascade + cycle protection.

### 🌐 Endpoint `/schema` extendido
Cada columna ahora incluye `references: { table, column, onDelete } | null`:
```json
{
  "name": "parent_id", "type": "INT", "pk": false, "notNull": false, "unique": false,
  "hasDefault": false, "default": null,
  "references": { "table": "parent", "column": "id", "onDelete": "CASCADE" }
}
```

### 🛡️ Restricciones de la versión
- Solo FK de columna única (no compuestas).
- Target debe ser la PK del parent — `REFERENCES` contra `UNIQUE` no-PK no está soportado todavía.
- Solo `RESTRICT` y `CASCADE` (ni `SET NULL`, ni `SET DEFAULT`, ni `NO ACTION`).
- `ALTER TABLE ADD COLUMN ... REFERENCES ...` reusa los mismos guards que UNIQUE: si la columna es `NOT NULL` necesita un `DEFAULT` que apunte a un parent existente, etc.

### 🧪 Validación
- 28/28 tests de integración (6 nuevos: `fk_create_validation_rejects_bad_targets`, `fk_insert_update_enforcement`, `fk_self_reference_allows_pointing_at_self`, `fk_delete_restrict_blocks_when_children_exist`, `fk_delete_cascade_removes_children_and_grandchildren`, `old_v5_db_file_is_rejected_after_v6_bump`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Octava intervención: identificadores duros + introspección completa (Camino A · paso 4)

> **Sin bump de formato.** Los datos en disco no cambian; el cambio es de validación (más estricta) y de contrato JSON (más rico).

### ✨ Identificadores
- Nuevo `catalog::validate_identifier(name, kind)` — única definición de "identificador válido" en el motor: `[A-Za-z_][A-Za-z0-9_]*`, longitud máxima `MAX_IDENT_LEN = 64`, no reservada.
- Lista `catalog::RESERVED_WORDS` con todas las keywords del parser y los nombres de tipo (`int`, `text`, `bool`, `float`, `date`, `datetime`, `json`, etc.).
- Aplicado en `CREATE TABLE` (nombre de tabla + cada columna), `ALTER TABLE ADD COLUMN` (nombre de columna nueva, vía `validate_create_table` sobre meta prospectivo) y `CREATE [UNIQUE] INDEX` (nombre de índice).

### 🌐 Endpoint `/schema` extendido
La respuesta de `GET /schema?db=X&table=Y` (y por tanto también `GET /tables`) ahora incluye lo necesario para reverse-engineering completo desde el frontend:

```json
{
  "ok": true,
  "table": {
    "name": "users",
    "primaryKey": "id",
    "rootPage": 2,
    "columns": [
      { "name": "id",    "type": "INT",  "pk": true,  "notNull": true,  "unique": false, "hasDefault": false, "default": null },
      { "name": "email", "type": "TEXT", "pk": false, "notNull": true,  "unique": true,  "hasDefault": false, "default": null },
      { "name": "status","type": "TEXT", "pk": false, "notNull": true,  "unique": false, "hasDefault": true,  "default": "pending" }
    ],
    "indexes": [
      { "name": "uq_users_email", "column": "email", "rootPage": 4, "unique": true }
    ]
  }
}
```

Campos nuevos por columna: `notNull`, `unique` (derivado de los índices unique de una columna), `hasDefault`, `default` (literal con su tipo nativo en JSON; `null` para "no default" o `DEFAULT NULL`). Campo nuevo por índice: `unique`.

### 🧪 Validación
- 22/22 tests de integración (1 nuevo: `identifier_rules_apply_across_ddl` cubre tabla/columna/índice y los tres rechazos: reservada, longitud, ALTER).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Séptima intervención: edición incremental de schemas (Camino A · paso 3)

> **Sin bump de formato.** El layout `VERSION = 5` ya soporta `TableMeta` con cualquier número de columnas; las filas previas se decodifican con un fallback a `DEFAULT` o `NULL` cuando la fila quedó "corta" frente al esquema nuevo.

### ✨ Funcionalidad SQL
- **`DROP TABLE [IF EXISTS] <name>`** — borra la entrada del catálogo. Las páginas backing (data + índices secundarios) **no** se liberan; el reclaim queda para un futuro `vacuum` (consistente con la política de `DROP INDEX`).
- **`ALTER TABLE <name> ADD [COLUMN] <coldef>`** — agrega una columna al final del esquema. Soporta `NOT NULL`, `DEFAULT`, `UNIQUE`. La keyword `COLUMN` es opcional.

### 🧱 Cambios estructurales
- `decode_row` tolera EOF mientras quedan columnas por decodificar: rellena con el `DEFAULT` de la columna o `NULL`. Permite `ADD COLUMN` sin reescribir filas existentes; el rewrite ocurre naturalmente en el próximo `UPDATE` de cada fila.
- `Catalog::remove_table` borra la entrada del catálogo via `Tree::delete`.
- `parse_column_def` factorizado y compartido entre `CREATE TABLE` y `ALTER TABLE ADD COLUMN`.
- `parse_if_exists` factorizado para `DROP DATABASE` / `DROP TABLE`.

### 🛡️ Restricciones de `ALTER ... ADD COLUMN`
- `PRIMARY KEY` rechazado (la PK ya existe; esta versión no admite swap ni multi-PK).
- `NOT NULL` requiere `DEFAULT` no nulo (sin él, las filas previas violarían la constraint inmediatamente).
- `UNIQUE` con `DEFAULT` no nulo en tabla con > 1 fila se rechaza (produciría duplicados en el backfill).
- `UNIQUE` sin DEFAULT en tabla poblada está OK: filas previas decodifican como `NULL`, y SQL UNIQUE permite múltiples NULLs.
- Nombre de columna duplicado rechazado.
- Validación completa del `coldef` (compatibilidad de tipo del DEFAULT, etc.) reusada del path de `CREATE TABLE`.

### 🧪 Validación
- 21/21 tests de integración (4 nuevos: `drop_table_removes_catalog_entry`, `alter_add_column_decodes_old_rows_with_default_or_null`, `alter_add_column_constraint_guards`, `alter_add_column_unique_then_enforces`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-07 — Sexta intervención: constraints declarativas (Camino A · paso 2)

> **On-disk format jump: VERSION 4 → 5.** `Column` ahora persiste `NOT NULL` y `DEFAULT`; `IndexMeta` persiste `unique`. Las DBs creadas con la entrega anterior son rechazadas explícitamente al abrir — re-crear con el binario v5.

### ✨ Funcionalidad SQL
- **`NOT NULL`** como constraint de columna en `CREATE TABLE`. Validado en `INSERT` (columna omitida sin DEFAULT, o `NULL` explícito) y en `UPDATE` (asignación que dejaría la columna en `NULL`). PK es implícitamente `NOT NULL`.
- **`DEFAULT <literal>`** como constraint de columna. Soporta `INT`, `FLOAT`, `BOOL`, `TEXT`/`DATE`/`DATETIME`/`JSON` y `NULL`. La compatibilidad de tipo entre literal y columna se valida en `CREATE TABLE` — `name TEXT DEFAULT 1` se rechaza. PK no admite `DEFAULT`.
- **`UNIQUE`** inline en columna y **`CREATE UNIQUE INDEX`** como sentencia. Inline auto-genera un índice unique con nombre `uq_<tabla>_<columna>`. Múltiples `NULL` se permiten (consistente con SQL estándar). Conflicto de UNIQUE se chequea **antes** de tocar disco — el INSERT/UPDATE falla sin efectos colaterales.
- `CREATE UNIQUE INDEX` sobre tabla con duplicados existentes **aborta el backfill** con error claro; no deja índice colgado.

### 🧱 Cambios estructurales
- `catalog::Column { name, column_type, not_null, default }` con `DefaultLiteral { Null, Integer, Float, Bool, String }` propio del catálogo (no acopla con `sql::Value`).
- `catalog::IndexMeta` lleva `unique: bool`.
- Layout v5 por columna: `[name][type_code:u8][flags:u8][default_payload?]` con `flags & 0x01 = NOT NULL`, `flags & 0x02 = HAS_DEFAULT`.
- Layout v5 por índice: `[name][column][root_page:u32][unique:u8]`.
- Nuevo helper `index::bucket_unique_conflict` y `sql::check_unique_conflict` — un único path de uniqueness para inline UNIQUE y `CREATE UNIQUE INDEX`.
- `sql::ColumnDef` lleva `not_null`, `unique`, `default: Option<Value>` para el AST del parser.

### 🧪 Validación
- 17/17 tests de integración (6 nuevos: `not_null_rejects_missing_and_explicit_null`, `default_fills_missing_and_can_be_overridden`, `default_with_not_null_combination`, `default_type_mismatch_rejected_at_create`, `inline_unique_rejects_duplicates`, `create_unique_index_backfill_aborts_on_duplicates`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.

---

## 2026-05-05 — Quinta intervención: DDL de DATABASE + modelador web

### ✨ Funcionalidad SQL
- **`CREATE DATABASE [IF NOT EXISTS] <name>;`** — crea un archivo `.db` en el directorio de `-dir` (server) o junto al path objetivo (CLI).
- **`DROP DATABASE [IF EXISTS] <name>;`** — borra el archivo `.db` y su `.wal` si quedó.
- **`SHOW DATABASES;`** — lista las DBs presentes en el directorio.

Estas sentencias **no se ejecutan contra una `.db` específica** (no operan sobre `TableMeta`). Las despacha el caller — `gabysql-server` para HTTP `/exec` y la CLI `gabysql exec` — antes de abrir el `Pager`. Mezclar DB-level con table-level en un mismo `/exec` se rechaza con error explícito.

### 🌐 Modelador web `gabymodeler`
- Nueva carpeta [`web/modeler/`](web/modeler/) — single-page HTML+CSS+JS vanilla, sin frameworks, sin npm, sin backend acoplado.
- Drag & drop de entidades sobre canvas con grid; SVG para líneas FK Bezier.
- Columnas con tipos (`INT/TEXT/BOOL/FLOAT/DATE/DATETIME/JSON`), flag `PK` (auto-fija `INT`), flag `idx` (índice secundario).
- Botón "↪ FK" para columnas que apuntan a otra entidad — la FK se documenta como comentario en el SQL (las FOREIGN KEY declarativas no se enforced en `VERSION 4`).
- **Exporta SQL** con `CREATE DATABASE [IF NOT EXISTS]` + `CREATE TABLE` + `CREATE INDEX`, copia al clipboard o descarga `.sql`.
- Persiste el modelo en `localStorage` (`gabymodeler.v1`).
- Botón "📦 Cargar ejemplo" trae un schema `users + orders` con FK indexada para evaluar el flujo en 1 click.

### 🧭 Landing `web/index.php` rediseñada
- Reemplaza la tarjeta única de phpgabyadmin por **dos tarjetas lado a lado**: `gabymodeler` y `phpgabyadmin`. Cada una con CTA propio.
- Documenta el flujo recomendado: **modeler → SQL → phpgabyadmin → ejecutar**.

### 🧪 Validación
- 11/11 tests de integración (incluye nuevo `database_level_statements_parse_and_engine_rejects`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`: clean.
- `php -l web/index.php` y `php -l web/phpgabyadmin/index.php`: clean.

---

## 2026-05-04 — Cuarta intervención: índices secundarios + scaffolding profesional

> **On-disk format jump: VERSION 3 → 4.** `TableMeta` ahora persiste una lista de índices secundarios; las DBs creadas con la entrega anterior son rechazadas explícitamente al abrir.

### ✨ Funcionalidad SQL
- **Índices secundarios**: `CREATE INDEX <name> ON <table> (<column>);` y `DROP INDEX <name>;`. Soporta backfill automático sobre tablas con datos existentes.
- **`SELECT WHERE col = val` por columna no-PK** consulta el índice cuando existe (lookup O(1) sobre bucket por hash, filtro exacto por bytes, hidratación por PK). Si la columna no es PK ni está indexada, se rechaza con mensaje explícito.
- `WhereClause::Eq` ahora acepta cualquier `Value` (no solo `i64`), por lo que `SELECT WHERE name = 'Ana'` o `WHERE score = 9.5` funcionan igual que `WHERE id = 1`.
- Mantenimiento automático de índices en `INSERT` / `UPDATE` / `DELETE`: el índice solo se actualiza cuando la columna indexada está afectada y el valor cambia.

### 🧱 Cambios estructurales
- Nuevo módulo [`src/index.rs`](src/index.rs): hashing FNV-1a-64, codec de bucket `[count:u16] + N×([vlen:u16][value][pk:i64])`, helpers `bucket_insert/remove/lookup`.
- `TableMeta::indexes: Vec<IndexMeta { name, column, root_page }>` persistido al final del payload del catálogo.
- Reglas de validación: una sola PK INT escalar (sin cambios), una sola columna por índice secundario, `JSON` no es indexable (sin semántica de igualdad canónica).
- `DROP INDEX` no libera páginas — el reclaim queda para una futura herramienta `vacuum`.

### 🛡️ Hardening de CI / supply chain (entrega previa, consolidada en docs)
- 4 workflows: `ci.yml` endurecido, `security.yml`, `workflow-security.yml`, `stale.yml`.
- `cargo audit` 0.22.1 (RustSec), `cargo deny` 0.19.4 (advisories + licenses + bans + sources, regido por [deny.toml](deny.toml)).
- `detect-secrets` (FS + últimos 50 commits), Trojan Source / zero-width / patrones peligrosos Rust+PHP / URLs de exfil.
- `grype` container scan con `--fail-on critical`.
- `actionlint` + `zizmor` + `pin-check` (rechaza acciones sin SHA pin).
- Acciones third-party pinneadas a SHA, `permissions: contents: read` por defecto, `persist-credentials: false`.
- Dependabot semanal: github-actions + cargo + docker.

### 📚 Scaffolding profesional importado desde otros repos del perfil
- `CODE_OF_CONDUCT.md`, `SUPPORT.md`, `COMPATIBILITY.md`, `RECRUITER.md`, `QUICKSTART.md`, `RELEASE.md`.
- `.editorconfig` y `.gitattributes` con normalización LF / CRLF coherente con CI multi-OS.
- `pull_request_template.md` con checklist de fmt/clippy/test/formato-en-disco/supply-chain.

### 🧪 Validación
- 10/10 tests de integración (incluye nuevos: split de B+Tree con 600 filas, detección de corrupción por checksum, rechazo de overwrite, UPDATE/DELETE roundtrip, **índices secundarios end-to-end con backfill + INSERT/UPDATE/DELETE/DROP**).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo audit`, `cargo deny check`: OK.
- `actionlint`, `zizmor`: 0 findings.

### ⚠️ Migración requerida
- DBs creadas con `VERSION = 3` no son legibles. Re-crear con `gabysql init <file.db>`. Mensaje de error explícito al abrir.

---

## 2026-05-03 — Tercera intervención: cierre de hallazgos críticos del MVP

> **On-disk format jump: VERSION 1 → 3.** Toda DB creada antes de esta entrega es rechazada explícitamente al abrir. Recrearla con la versión actual (`gabysql init <file.db>`).

### 🧱 Cambios estructurales del motor
- **B+Tree real**: el índice por PK pasó de una lista enlazada de hojas a un B+Tree con nodos internos. Lookup descendente en O(log N), `root_page` permanece estable cruzando splits gracias a copy-up del root.
- **Hash del catálogo determinista**: las claves del catálogo de tablas se calculaban con `DefaultHasher` (no estable entre versiones de Rust). Reemplazado por FNV-1a-64 inline en código.
- **Checksums CRC32-IEEE**: cada página persiste un trailer de 4 bytes con su CRC. El Pager lo finaliza antes de flushear y verifica al leer y al replay del WAL. La corrupción ahora produce error explícito en vez de silencio.
- **`Pager::create` no destructivo**: rehúsa sobrescribir un archivo existente. Se introdujo `create_force` para el camino explícito de reset (`gabysql init --force <file.db>`).
- **`page_size` honesto**: el header valida que `page_size == PAGE_SIZE_DEFAULT`; el campo se mantiene en disco para una futura revisión del formato.

### ✨ Funcionalidad SQL
- `UPDATE <tabla> SET col = val[, ...] WHERE <pk> = N;` (no permite cambiar la PK).
- `DELETE FROM <tabla> WHERE <pk> = N;` (error si la PK no existe).
- Mensajes de error de PK más explícitos sobre la limitación INT-only de esta versión.

### 🛡️ Endurecimiento del modo server
- `gabysql-server` aplica un techo de conexiones concurrentes (default 64, configurable con `-max-connections N`). Conexiones extra reciben 503 y se cierran sin generar threads.

### 🧪 Validación
- 9/9 tests de integración (incluye nuevos: split de B+Tree con 600 filas, detección de corrupción por checksum, rechazo de overwrite, UPDATE/DELETE roundtrip).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`: OK.

### ⚠️ Migración requerida
- Bases de datos creadas con versiones anteriores a esta entrega no son legibles. El error es explícito (`unsupported gabysql file format: version=...`). Re-crear con el binario actual.

---

## 2026-03-19 — Segunda intervención: migración completa a Rust y estabilización base

### 🧱 Estado actual del sistema
- Motor embebido en Rust con archivo único `.db`
- CLI `gabysql` para `init`, `info`, `exec` y `repl`
- Server HTTP `gabysql-server` para operar una base única o un directorio de bases
- `phpgabyadmin` consumiendo la API HTTP como consola web liviana
- Docker y `docker compose` para levantar server y admin web en un entorno reproducible

### 🏗️ Cambios estructurales
- Se eliminó la implementación anterior en Go y se reemplazó por un proyecto Rust con `Cargo`
- Se separó el core en módulos de storage, catálogo, SQL, servidor y estructura persistente por clave primaria
- Se unificó la documentación para reflejar solo las capacidades reales del motor actual

### ✨ Mejoras funcionales
- Soporte de `CREATE TABLE`, `INSERT` y `SELECT` con full scan, `LIMIT/OFFSET`, `WHERE <pk> = ...` y `BETWEEN`
- Soporte de tipos `INT`, `TEXT`, `BOOL`, `FLOAT`, `DATE`, `DATETIME`, `JSON` y `NULL` en columnas no PK
- Rechazo explícito de claves primarias duplicadas en vez de sobrescritura silenciosa
- Recovery WAL por marcador `COMMIT` para rehidratar páginas confirmadas tras reinicio

### 🛡️ Estabilidad y seguridad
- El parser SQL ahora devuelve errores controlados en escenarios inválidos en lugar de derribar el proceso
- Se corrigió el manejo de comillas escapadas dentro de strings SQL para soportar textos complejos en inserciones multi-sentencia
- `phpgabyadmin` quedó endurecido con cookie firmada y bloqueo de servidores remotos salvo habilitación explícita
- La UI web y el README quedaron alineados con el comportamiento real del motor

### 🎨 Documentación y lenguaje visual
- Se creó un set documental completo alineado con el estándar usado en otros repos del perfil
- Se añadieron guías de instalación, uso, operación, seguridad, troubleshooting y contribución
- Se añadió documentación técnica de arquitectura, requisitos, API y especificaciones del motor
- Se aplicó una capa visual consistente con badges, bloques de estado, tablas de navegación y rutas por perfil

### ✅ Validación y entrega continua
- Se agregaron pruebas de integración para roundtrip básico, PK duplicada, paginación con `LIMIT/OFFSET`, `NULL`, parser inválido y recovery WAL
- Se agregó CI en GitHub Actions con `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` y lint de PHP
- La matriz de CI cubre `ubuntu-latest`, `windows-latest` y `macos-latest`, más build Docker en Linux
- La CI publica artefactos `release` por sistema operativo para facilitar distribución nativa multiplataforma
- El `Dockerfile` valida `cargo test --all-targets` antes de construir binarios release
- `docker compose` permite probar juntos `gabysql-server` y `phpgabyadmin`

### 🧪 Validación realizada en esta intervención
- `cargo fmt --check`: OK
- `cargo check --tests`: OK
- `cargo clippy --all-targets -- -D warnings`: OK
- `docker build -t gabysql .`: OK
- `docker compose up -d --build`: OK
- `GET http://localhost:8080/health`: OK
- `GET http://localhost:8000`: OK

### ⚠️ Límites actuales conocidos (al cierre de la 2ª intervención)
- El índice persistente sigue siendo una estructura de hojas enlazadas por PK `INT`; no es todavía un B+Tree multinivel completo *(superado en la 3ª intervención: ver entrada superior)*
- No hay optimizer cost-based ni estadísticas de consulta
- No hay concurrencia avanzada, MVCC ni transacciones complejas
- Sigue siendo un producto base estable para evolucionar, no un reemplazo directo de motores maduros como PostgreSQL o MySQL
