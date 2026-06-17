# ADR-0035: `RAISE` + `FOR LOOP` (X4c)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: X4c (séptimo sub-bloque del bloque X del roadmap)
**Bump on-disk**: ninguno

## 🧭 Contexto

X4 + X4b dejaron el lenguaje procedural con IF, variables, y WHILE. X4c agrega dos piezas más:

1. **`RAISE EXCEPTION|NOTICE 'msg'`**: aborto explícito con mensaje (EXCEPTION) o info logging (NOTICE).
2. **`FOR i IN start TO end LOOP ... END LOOP`**: range loop con auto-declaración de la variable de iteración.

`EXCEPTION WHEN ... THEN` handlers, `FOR row IN SELECT ... LOOP`, `LOOP ... END LOOP` standalone, y `RETURN` quedan para X4d.

## 💡 Decisión

### 1. `RAISE [EXCEPTION|NOTICE] 'msg'`

```sql
RAISE EXCEPTION 'something broke';      -- → DbError [GBY-4111]
RAISE NOTICE 'informational';           -- → OK con message en ResultSet
RAISE 'aborted';                        -- = RAISE EXCEPTION (default)
```

- **Default level es EXCEPTION** (consistente con PG).
- **Mensaje es un literal STRING** — formato `%` estilo PG diferido a X4d (workaround: `CONCAT` antes). **Actualización 2026-06-15**: el formato `%` con arity validation entró en X5 ([ADR-0041](0041-x5-procedural-refinements.md), `[GBY-4120]` si los `%` no matchean los args). X4d ([ADR-0036](0036-exception-loop-x4d.md)) entregó los handlers `EXCEPTION WHEN`.
- **EXCEPTION**: levanta `DbError::new("[GBY-4111] <msg>")`. Propaga normalmente — el wrap del caller hace rollback de la transacción. No hay handler en X4c (X4d).
- **NOTICE**: retorna ResultSet vacío con `message = "NOTICE: <msg>"`. No interrumpe el flujo.

### 2. `FOR ident IN start TO end LOOP <body> END LOOP`

```sql
FOR i IN 1 TO 10 LOOP
    -- body, puede usar i en expr
END LOOP;
```

- **Sintaxis non-PG**: PG usa `FOR i IN 1..10 LOOP`. gabysql usa `start TO end` para evitar conflicto del `..` con qualificadores ident. Documentado.
- **Inclusiva**: i toma valores `start, start+1, ..., end`.
- **Auto-declaración**: la variable `i` se agrega a `var_scope` al inicio. Si existía con el mismo nombre, se shadowa temporalmente y se **restaura** al final (PG convention).
- **`start > end` → no itera, sin error** (PG behavior).
- **STEP fijo en 1**, ascendente. `STEP n` y descendente diferidos.
- **Guard `MAX_LOOP_ITERATIONS = 100K`** compartido con WHILE.
- **`EXIT [WHEN]`** funciona dentro de FOR (mismo sentinel).
- **start/end deben ser INT** — float/text/null → `[GBY-4113]`.

### 3. Splitter + body parsers: `FOR` como block-open

`FOR` se trata como block-open igual que `WHILE` (cierra con `END LOOP`). **Excepción importante**: `FOR EACH ROW` dentro de `CREATE TRIGGER` NO abre bloque — el splitter mira el lookahead post-FOR; si la siguiente palabra es `EACH`, no incrementa depth. Los body parsers de trigger/procedure ya están DENTRO del body post-FOR-EACH-ROW, así que no enfrentan ese caso.

### 4. RAISE en cualquier contexto procedural

`RAISE` funciona como cualquier otro statement — top-level, dentro de IF/WHILE/FOR body, dentro de trigger/procedure body. Útil para validaciones:

```sql
CREATE TRIGGER validate AFTER INSERT ON orders FOR EACH ROW BEGIN
    IF NEW.amount < 0 THEN
        RAISE EXCEPTION 'negative amount not allowed';
    END IF;
END;
```

El `RAISE EXCEPTION` aborta el `INSERT` (la transacción rollback).

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4111` | `RAISE_EXCEPTION` | User-triggered RAISE EXCEPTION. El mensaje del DbError = mensaje del user. |
| `GBY-4112` | `RAISE_MESSAGE_INVALID` | `RAISE` con argumento no-STRING (ident, literal numérico, etc.). |
| `GBY-4113` | `FOR_RANGE_INVALID` | start o end no-INT. |

## 🧪 Validación

Suite `x4c_*` en `tests/integration_test.rs` (9 tests):

- `x4c_raise_exception`: RAISE EXCEPTION propaga `[GBY-4111]` con mensaje.
- `x4c_raise_notice`: RAISE NOTICE retorna OK con message.
- `x4c_raise_default_is_exception`: RAISE sin level = EXCEPTION.
- `x4c_raise_inside_if`: RAISE dentro de IF.
- `x4c_for_loop_counts`: FOR básico.
- `x4c_for_loop_with_exit`: FOR + EXIT WHEN.
- `x4c_for_loop_empty_range`: start > end no itera.
- `x4c_for_loop_var_shadow_restore`: variable previa restaurada post-FOR.
- `x4c_for_loop_bad_range`: bounds non-INT → `[GBY-4113]`.

Suite total: **467/467 pass** (`cargo test --lib --tests`).

## 🔭 Futuro (X4d+)

- **`EXCEPTION WHEN ... THEN <body>`** handlers en `BEGIN ... EXCEPTION ... END` blocks. Requiere convertir `BEGIN ... END` en una Statement con campo opcional de handler.
- **`FOR row IN SELECT ... LOOP`**: iteración sobre resultset (row se auto-declara como composite type — necesita representación de Row en var_scope).
- **`LOOP ... END LOOP`** standalone (sin condición, terminado por EXIT WHEN).
- **`RETURN expr`** dentro de functions: alternativa al `AS <expr>` único.
- **`CASE` statement** (vs CASE expression).
- **`STEP n`** en FOR y dirección descendente (`REVERSE`).
- **Formato `%`** en RAISE (`RAISE EXCEPTION 'value % invalid', x`).
- **`RAISE WARNING` / `RAISE INFO`**: niveles intermedios.
- **`RAISE USING ERRCODE = ...`**: emit con código específico.

Con X4c, el bloque X tiene cobertura razonable de PL/pgSQL básico. Lo que queda (X4d) son features menos demandados; alcanzar paridad total con PL/pgSQL es un proyecto independiente.
