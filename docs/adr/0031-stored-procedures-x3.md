# ADR-0031: Stored procedures (`CREATE PROCEDURE` + `CALL`) (X3)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: X3 (tercer sub-bloque del bloque X del roadmap)
**Bump on-disk**: VERSION 14 → 15 (nuevo `ObjectKind::Procedure` en el catálogo)

## 🧭 Contexto

X1 ([ADR-0029](0029-triggers-after-x1.md)) y X2 ([ADR-0030](0030-triggers-before-multistmt-x2.md)) entregaron triggers AFTER/BEFORE con body single-stmt y multi-stmt. X3 entrega la pieza siguiente del bloque X: **stored procedures con `CALL`**.

Funciones escalares invocables desde SELECT (`CREATE FUNCTION RETURNS scalar`) quedan diferidas a X3b — requieren extender el AST de `Expr` con un variant nuevo y tocar ~160 match arms. Procedures como statement-level routines son una pieza mucho más acotada que paga sola.

> 📝 **Actualización 2026-06-15**: X3b entregada por [ADR-0032](0032-user-functions-x3b.md) el 2026-05-28. Funciones escalares ya son invocables desde SELECT/WHERE/CHECK/HAVING/UPDATE SET.

## 💡 Decisión

### 1. Sintaxis canónica

```sql
CREATE PROCEDURE name(p1 TYPE [, p2 TYPE]*) AS <body>;

DROP PROCEDURE [IF EXISTS] name;

CALL name(arg1, arg2, ...);
```

Donde:

- `<body>` es **DML simple** (INSERT/UPDATE/DELETE/REPLACE) o `BEGIN stmt; stmt; ... END` — mismo grammar que un trigger body (reutilizamos el splitter de X2).
- Cada `arg` en `CALL` es una **expresión** (`Expr`) evaluada contra una fila vacía (igual que VALUES). Permite literales (`10`), aritmética (`10 + 5`), funciones (`LENGTH('abc')`), CASE, etc.
- Los parámetros se sustituyen en el body por sus valores literales en cada CALL (token-level — mismo enfoque que NEW/OLD en triggers).

### 2. Persistencia: `ObjectKind::Procedure` (VERSION 15)

El catálogo, que desde V14 tenía `[kind:u8]` con `0=Table`, `1=View`, `2=Trigger`, suma `3=Procedure`. Payload:

```
[name][param_count:u16] · param_count × ([param_name][type_code:u8]) · [body_sql]
```

VERSION bump 14 → 15 — V14 abierto por un binario X3+ rebota con `[GBY-1003]`. Migración manual: dump + recreate.

### 3. Substitución de parámetros: token-level con nombres bare

A diferencia de los triggers (donde `NEW.x` / `OLD.x` están cualificados y son inconfundibles), los parámetros de procedure son idents bare. Eso introduce una **limitación conocida**: si una columna real tiene el mismo nombre que un parámetro, el ident de la columna también se substituye y la query rompe.

**Workaround documentado** (mismo que PostgreSQL recomienda para evitar conflictos): usar prefijos en los nombres de parámetro:

```sql
-- ❌ choque: `id` aparece como param Y como columna de la tabla.
CREATE PROCEDURE add_log(id INT) AS INSERT INTO log (id) VALUES (id);
-- En el body, ambos `id` se substituirían — el del INSERT INTO log (id) se vuelve
-- INSERT INTO log (10) que falla como sintaxis.

-- ✅ prefijo
CREATE PROCEDURE add_log(p_id INT) AS INSERT INTO log (id) VALUES (p_id);
```

### 4. CALL es un statement standalone

`CALL` no se puede usar como Expr ni dentro de SELECT — es solo un statement top-level. Eso es coherente con la naturaleza side-effect-only de las procedures (no retornan valor). Para "función" invocable en SELECT hay que esperar X3b.

El executor:

1. Lookup de la procedure por nombre → `[GBY-4099]` si no existe.
2. Validar arity → `[GBY-4100]` si difiere.
3. Evaluar cada `arg` con `eval_expr_full` contra fila vacía + outer scope nulo.
4. Bind param_name (lowercased) → valor.
5. Token-substitution sobre `body_sql`.
6. `parse(substituted)` → `Vec<Statement>` (split por `;` interno).
7. Exec cada statement en orden. Falla a mitad → propaga el error → wrap de transacción del caller hace rollback.

### 5. Type checking en CALL: best-effort

Hoy NO se valida que el tipo del arg matchee con el tipo declarado del param (e.g. `CALL foo('text')` cuando `foo(p INT)`). El motor confía en que el INSERT/UPDATE downstream rebote en runtime con un type mismatch. Validación estricta queda diferida.

### 6. No hay recursion guard (por ahora)

Una procedure que se llama a sí misma vía CALL desde su body **NO** está bloqueada por un depth counter — el wrap de transacción del caller eventualmente rebota por stack overflow (Rust recursion limit) o exhaustion de recursos. Si esto se vuelve problema, agregar un `proc_depth` similar al `trigger_depth` es trivial.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4097` | `PROCEDURE_NAME_COLLIDES` | Nombre colisiona con tabla / vista / trigger / procedure existente. |
| `GBY-4098` | `PROCEDURE_BODY_INVALID` | Body no es DML/BEGIN/END, body vacío, param duplicado, BEGIN sin END matching. |
| `GBY-4099` | `PROCEDURE_NOT_FOUND` | `CALL` o `DROP PROCEDURE` sobre nombre inexistente (sin `IF EXISTS`). |
| `GBY-4100` | `PROCEDURE_ARITY_MISMATCH` | `CALL` recibió N args; procedure declara M. |

## 🧪 Validación

Suite `x3_*` en `tests/integration_test.rs` (9 tests):

- `x3_simple_call_inserts`: CALL básico con 2 params.
- `x3_call_with_multi_stmt_body`: body `BEGIN ... END` con 2 INSERTs.
- `x3_call_arg_can_be_expression`: `CALL p(10 + 5)` evalúa la expresión.
- `x3_call_arity_mismatch_rejected`: `[GBY-4100]`.
- `x3_call_unknown_procedure_rejected`: `[GBY-4099]`.
- `x3_drop_procedure_works` + `x3_drop_procedure_if_exists_noop`: lifecycle.
- `x3_procedure_name_collides_with_table`: `[GBY-4097]`.
- `x3_procedure_persists_across_reopen`: el TriggerMeta sobrevive close del pager.

Suite total: **432/432 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

- **`CREATE FUNCTION ... RETURNS scalar`** (X3b): invocable desde SELECT/WHERE/etc. Requiere extender el AST de `Expr` con `UserFunc { name, args }` y tocar ~160 match arms.
- **`CREATE PROCEDURE ... RETURNS TABLE`**: procedures que devuelven un resultset. Cambia el dispatch de CALL.
- **Argumentos OUT / INOUT**: PG/SQL Server-style. Requiere bindings de vuelta.
- **Type checking estricto** en CALL.
- **Recursion guard** (`MAX_PROCEDURE_DEPTH`).
- **`DECLARE` variables locales** dentro de `BEGIN ... END`: ya es PL/pgSQL.
- **Control de flujo** (`IF`/`LOOP`/`WHILE`): X4+.

Con X3, el bloque X queda con cobertura razonable para los 3 casos de uso más comunes de "lógica del lado servidor":

- **Triggers** (X1+X2): reaccionar a DMLs.
- **Procedures** (X3): encapsular side effects parametrizados.

Falta solo **funciones** (X3b) para tener el quartet operativo clásico. Después de eso, X4 es lenguaje procedural completo — área que probablemente requiere su propia spec de proyecto.
