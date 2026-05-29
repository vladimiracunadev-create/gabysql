# ADR-0036: `BEGIN..EXCEPTION..END` + `LOOP` standalone (X4d)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: X4d (octavo sub-bloque del bloque X del roadmap)
**Bump on-disk**: ninguno

## 🧭 Contexto

Con X4/X4b/X4c entregados, gabysql tiene `IF/ELSIF/ELSE`, variables locales con `DECLARE/SET`, `WHILE` y `FOR` loops, `EXIT [WHEN]`, y `RAISE EXCEPTION/NOTICE`. X4d cierra dos huecos de PL/pgSQL:

1. **`BEGIN ... EXCEPTION WHEN OTHERS THEN ... END`** — try/catch.
2. **`LOOP ... END LOOP`** standalone — infinite loop terminado por EXIT.

`RETURN expr` en functions y `FOR row IN SELECT ... LOOP` quedan para X4e (último previsto del bloque X).

## 💡 Decisión

### 1. `BEGIN ... [EXCEPTION WHEN OTHERS THEN ...] END` como Statement

Nuevo `Statement::Block(Box<BlockStmt>)` con `body: Vec<Statement>` y `exception_handler: Option<Vec<Statement>>`. El parser detecta:

- `BEGIN` seguido de `TRANSACTION`/`WORK`/`;`/EOF → `Statement::Begin` (transacción, existing).
- `BEGIN` seguido de cualquier otra cosa → `Statement::Block` (nuevo).

Lookahead simple, sin ambigüedad. Triggers/procedures que usan `BEGIN <stmts> END` siguen funcionando porque el body parser de CREATE TRIGGER/PROCEDURE ya envuelve sus bodies dentro de su propio handling — el `Statement::Block` solo aparece cuando el parser llega a un `BEGIN` no-tx en `parse_statement`.

**X4d solo soporta `WHEN OTHERS`** (catch-all). Filtros por código específico (`WHEN no_data_found THEN ...`) quedan diferidos.

### 2. Semántica del handler

`exec_block`:

1. Ejecuta cada stmt del body en orden.
2. Si alguno rebota:
   - Si el error contiene `EXIT_SIGNAL` (X4b sentinel) → re-propagar inmediatamente. El EXIT debe llegar al WHILE/FOR/LOOP outer; el handler de EXCEPTION no es un sink.
   - Si NO hay handler → propagar el error.
   - Si HAY handler → ejecutar el handler en su lugar, retornar OK con "EXCEPTION caught: <orig>" como mensaje.
3. Si el body termina OK, retornar OK.

**Atrapa todo tipo de error**: `RAISE EXCEPTION` user-triggered, PK duplicate, type mismatch, división por cero, etc. Como `WHEN OTHERS` es catch-all. Si se necesita re-raise condicional, el handler puede usar `RAISE EXCEPTION 'new msg'`.

### 3. `LOOP ... END LOOP` standalone

Nuevo `Statement::Loop(Box<LoopStmt>)` con `body: Vec<Statement>`. Itera infinitamente hasta que un `EXIT` (sentinel) sale, o hasta hit `MAX_LOOP_ITERATIONS = 100_000`. Mismo guard que WHILE/FOR.

### 4. Refactor del splitter: el block-open de loops vive en `LOOP`, no en `WHILE`/`FOR`

Pre-X4d, `WHILE` y `FOR` abrían depth (block-open), y `LOOP` era no-op. Esto funcionaba pero requería código duplicado en split_statements y body parsers para cada keyword.

X4d unifica: `LOOP` siempre abre depth (salvo si just-saw-`END`, en cuyo caso es parte de `END LOOP`). `WHILE`/`FOR` ya no abren depth. Resultado: `WHILE cond LOOP body END LOOP`, `FOR i IN ... LOOP body END LOOP`, y `LOOP body END LOOP` standalone se manejan todos por el mismo branch.

Esto también beneficia el splitter/body parser que ahora tiene menos casos (solo `BEGIN`, `IF`, `LOOP` abren depth).

### 5. EXIT sigue burbujeando a través de `Statement::Block`

Punto sutil pero crítico: si un `EXIT` aparece dentro de un `BEGIN..END` que vive dentro de un `WHILE`, el EXIT debe escapar del Block para llegar al WHILE outer. `exec_block` chequea `if e.contains(EXIT_SIGNAL) return Err(e)` ANTES de aplicar el handler — el sentinel pasa transparentemente.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4114` | `EXCEPTION_HANDLER_MALFORMED` | Falta WHEN/OTHERS/THEN, EXCEPTION sin BEGIN, etc. |
| `GBY-4115` | `LOOP_BLOCK_MALFORMED` | Falta END LOOP. |

## 🧪 Validación

Suite `x4d_*` en `tests/integration_test.rs` (8 tests):

- `x4d_exception_catches_raise`: handler atrapa RAISE EXCEPTION.
- `x4d_exception_catches_runtime_error`: handler atrapa PK duplicate.
- `x4d_no_exception_propagates`: BEGIN sin handler propaga el error.
- `x4d_block_without_error_runs_body`: happy path — body completo, handler ignorado.
- `x4d_loop_standalone_with_exit`: LOOP infinite + EXIT WHEN termina.
- `x4d_loop_max_iter_guard`: LOOP sin EXIT → `[GBY-4109]`.
- `x4d_exception_inside_loop`: handler dentro de WHILE atrapa cada iteración sin parar el loop.
- `x4d_exception_in_trigger_body`: BEGIN..EXCEPTION..END dentro de trigger body — el trigger no falla aunque el body interno rebote.

Suite total: **475/475 pass** (`cargo test --lib --tests`).

## 🔭 Futuro (X4e — último previsto del bloque X)

- **`RETURN expr`** en functions: requiere extender function body de Expr a Vec<Statement> con RETURN como sentinel. Lift de la restricción "function body = Expr" de X3b.
- **`FOR row IN SELECT ... LOOP`**: iteración sobre resultset. Requiere composite row scope (`row.col` accessible) o registro intermedio (`DECLARE row RECORD`).
- **`EXCEPTION WHEN <code> THEN ...`**: handlers filtrados por código de error específico (`WHEN no_data_found THEN`, etc.).
- **`RAISE` con formato `%`**: `RAISE EXCEPTION 'value % invalid', x`.
- **`CASE` statement** (vs CASE expression).
- **`SAVEPOINT`** + `ROLLBACK TO`: ya parte de TCL más que de PL/pgSQL.

Con X4d, gabysql cubre la inmensa mayoría de patrones PL/pgSQL del lado servidor. Lo que queda (X4e) son features útiles pero menos demandados; paridad completa con PL/pgSQL es un proyecto independiente.
