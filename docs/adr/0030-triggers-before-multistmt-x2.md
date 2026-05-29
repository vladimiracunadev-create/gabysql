# ADR-0030: Triggers BEFORE + body multi-statement (X2)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: X2 (segundo sub-bloque del bloque X del roadmap)
**Bump on-disk**: ninguno (X1 ya tenía el slot de timing en `TriggerMeta`)

## 🧭 Contexto

X1 ([ADR-0029](0029-triggers-after-x1.md)) entregó triggers `AFTER` con body de una sola sentencia. X2 cierra los dos huecos más visibles:

1. **`BEFORE` triggers**: para validaciones que abortan la operación (vía error propagado) y para logs que precedan al write.
2. **Body multi-statement** `BEGIN ... END`: para triggers que necesitan hacer más de una cosa (típico: log + denormalización + counter increment).

Lenguaje procedural completo (variables, IF/THEN, LOOP) y `CREATE FUNCTION` quedan para X3+.

## 💡 Decisión

### 1. BEFORE triggers habilitados

`CREATE TRIGGER ... BEFORE ...` ya no rebota — X1 lo rechazaba al CREATE-time con `[GBY-4093]`. Ahora se persiste y se dispara en los hooks BEFORE de `exec_insert`/`exec_update`/`exec_delete`.

**Cómo se construyen NEW/OLD para BEFORE:**

| Evento | NEW | OLD |
|---|---|---|
| BEFORE INSERT | User-stated cols + `NULL` para columnas no especificadas | — |
| BEFORE UPDATE | OLD con las assignments evaluadas contra OLD (sin tocar disco) | Snapshot del row antes del update |
| BEFORE DELETE | — | Snapshot del row antes del delete |

**Limitación X2 importante**: NEW es **read-only** desde el trigger BEFORE — no se puede mutar la fila desde el body. Esto significa que BEFORE en X2 NO sirve para "rellenar defaults" o "modificar la fila antes de escribir"; sirve para auditar pre-write o abortar (vía error propagado).

**Aborto vía error**: si el body de un BEFORE trigger falla (e.g. INSERT con PK duplicada en otra tabla), el error se propaga y el INSERT/UPDATE/DELETE original aborta — el wrap de transacción del caller hace rollback. Es la mecánica de "validación con BEFORE" hecha por composición.

### 2. Body multi-statement `BEGIN ... END`

Nueva sintaxis:

```sql
CREATE TRIGGER multi AFTER INSERT ON t FOR EACH ROW BEGIN
    INSERT INTO log_a (id) VALUES (NEW.id);
    INSERT INTO log_b (id) VALUES (NEW.id);
END;
```

Cada substatement se separa por `;`. Pueden anidarse `BEGIN ... END` (depth tracking en parser), aunque en X2 no agrega expresividad (no hay scope, no hay control de flujo).

**Cambios estructurales**:

- **`split_statements`** (top-level splitter por `;`) ahora detecta `BEGIN ... END` y mantiene los `;` internos dentro del chunk. Disambigua entre `BEGIN [TRANSACTION];` (transacción, sin block) y `BEGIN <stmt>...` (block-open) por lookahead.
- **Tokenizer** acepta `;` como `Symbol(";")`. Pre-X2 nunca llegaba al tokenizer porque el splitter lo consumía antes; con bodies multi-statement los `;` internos viajan dentro del chunk de tokens.
- **`parse_create_trigger`** detecta `BEGIN` tras `FOR EACH ROW` y consume hasta el `END` matching. El body persistido es el texto entre `BEGIN` y `END` (excluyendo las palabras), conteniendo N statements separados por `;`.
- **`fire_triggers`** llama `parse(substituted_body_sql)` que devuelve `Vec<Statement>` (split_statements vuelve a operar sobre el body). Cada statement se ejecuta en orden. Si alguno falla, el error se propaga (el trigger depth se decrementa correctamente).

### 3. Reglas del body single-statement preservadas (back-compat con X1)

Triggers creados con X1 (body sin `BEGIN/END`) siguen funcionando idénticamente. El parser detecta:

- Si tras `FOR EACH ROW` viene `BEGIN` → path multi-stmt.
- Si viene `INSERT/UPDATE/DELETE/REPLACE` → path single-stmt (X1 path).
- Cualquier otra cosa → `[GBY-4093]`.

## 📐 Códigos de error

Mismos que X1 — no se agregaron códigos nuevos. `[GBY-4093]` ahora cubre también:

- `BEGIN` sin `END` matching.
- `BEGIN END` con body vacío.

## 🧪 Validación

Suite `x2_*` en `tests/integration_test.rs` (7 tests):

- `x2_before_insert_logs_user_stated_new`: BEFORE INSERT con NEW = user cols.
- `x2_before_update_sees_old_and_computed_new`: BEFORE UPDATE con OLD del disco y NEW computado.
- `x2_before_delete_can_log_old`: BEFORE DELETE con OLD.
- `x2_multi_statement_body`: body con 2 INSERTs separados por `;`.
- `x2_before_and_after_both_fire`: BEFORE corre antes que AFTER en el mismo INSERT.
- `x2_before_can_abort_via_uniqueness_violation`: BEFORE que rebota → INSERT principal aborta (transacción rollback).
- `x2_begin_without_end_rejected`: `[GBY-4093]`.

Plus se removió `x1_before_rejected_in_release` (la restricción que el test verificaba fue lifteada).

Suite total: **423/423 pass** (`cargo test --lib --tests`).

## 🔭 Futuro (X3+)

- **NEW mutable en BEFORE**: el trigger puede modificar NEW antes del write (típico use case: timestamp `updated_at = NOW()`). Requiere protocolo de retorno entre el body del trigger y el caller — significativo refactor.
- **`IF expr THEN ... [ELSE ...] END IF`** en el body: control de flujo básico.
- **`LOOP` / `WHILE`** y variables locales (`DECLARE x INT`): lenguaje procedural completo (PL/pgSQL-like).
- **`RAISE EXCEPTION` / `RAISE NOTICE`**: control de errores explícito.
- **`OLD` para UPSERT que terminó en UPDATE**: el path `INSERT ... ON CONFLICT DO UPDATE` sigue firing AFTER UPDATE con OLD=None.
- **`CREATE FUNCTION` con cuerpo SQL** (X3): funciones invocables desde SELECT.
- **`CREATE PROCEDURE` + `CALL`**: side effects sin retornar valor.
- **Triggers sobre vistas (`INSTEAD OF`)**: requiere el rewrite engine de vistas para que las DML sobre vistas tengan algo que rehacer.
