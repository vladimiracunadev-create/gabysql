# ADR-0029: Triggers AFTER (X1)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-28
**Bloque**: X1 (primer sub-bloque del bloque X del roadmap)
**Bump on-disk**: VERSION 13 → 14 (nuevo `ObjectKind::Trigger` en el catálogo)

## 🧭 Contexto

El bloque X del roadmap agrupa **triggers + stored procedures + lenguaje procedural** — una piscina enorme (>5000 líneas de código) que requiere partir en pedazos. X1 entrega el primer sub-bloque: `CREATE TRIGGER` con body single-statement.

Cubre los casos clásicos: auditoría, timestamps de update, denormalización, cascadas manuales. PL/pgSQL completo (BEGIN/END, variables, loops, IF/THEN, exceptions) queda para X2+.

## 💡 Decisión

### 1. Sintaxis canónica

```sql
CREATE TRIGGER name { BEFORE | AFTER } { INSERT | UPDATE | DELETE }
    ON table FOR EACH ROW <single_dml_stmt>;

DROP TRIGGER [IF EXISTS] name;
```

Donde `<single_dml_stmt>` es UNA sentencia `INSERT`, `UPDATE`, `DELETE` o `REPLACE`.

### 2. Sólo AFTER triggers en X1

`BEFORE` rechazado con `[GBY-4093]`. Razón: BEFORE necesita "NEW antes de defaults" (para INSERT) y un mecanismo para abortar la acción — ambas piezas son más invasivas y agregarían riesgo a X1. AFTER es lo más útil en la práctica (audit, log, denormalize). BEFORE llegará en X2.

### 3. Referencias NEW.col y OLD.col vía substitución a nivel de TOKEN

El parser de gabysql tiene una restricción: `INSERT VALUES (...)` sólo acepta literales `Value`, no `Expr`. Eso impide que el AST del body almacene `NEW.col` como `Expr::Column("new.col")` y lo evalúe contra una "row de scope" en runtime.

Solución pragmática: el body se persiste como **texto SQL**, y al fire-time:

1. Se tokeniza el body persistido.
2. Cada token `Ident` cuyo texto empiece con `new.` u `old.` (case-insensitive) se reemplaza por los tokens del literal correspondiente, leyendo el valor del HashMap `new_row` / `old_row` provisto por el dispatcher.
3. Se reconstruye el SQL y se parsea normal.
4. Se ejecuta.

Esta substitución es **agnóstica del contexto** — funciona dentro de `INSERT VALUES`, `UPDATE SET`, `WHERE`, `ON CONFLICT DO UPDATE`, etc., sin necesidad de walker AST por contexto. Trade-off: cost de re-tokenizar + re-parsear por cada fire. Para casos típicos (audit log con pocas filas, denormalización per-row) el overhead es invisible.

### 4. Disparo desde `exec_insert` / `exec_update` / `exec_delete`

Los hooks viven al final del path de éxito de cada DML:

- `exec_insert`: después de cada fila exitosa (`RowOutcome::Inserted`) → `fire_triggers(table, Insert, After, Some(new), None)`. Para `RowOutcome::Updated` (UPSERT que cayó en UPDATE) → `fire_triggers(table, Update, After, Some(new), None)` — OLD no disponible en este path (limitación X1).
- `exec_update`: si hay triggers AFTER UPDATE registrados, se snapshottea OLD ANTES del update y se re-lee la fila DESPUÉS para construir NEW.
- `exec_delete`: si hay triggers AFTER DELETE, se snapshottea OLD antes del `delete_with_cascade`.

`has_after_trigger(table, event)` evita pagar las lecturas extra cuando no hay triggers.

### 5. Guard de recursión: `MAX_TRIGGER_DEPTH = 16`

Cada fire incrementa `Engine::trigger_depth`; al volver, decrementa. Si la cadena pasa 16, `[GBY-4095]`. Cubre el caso clásico de trigger que modifica su propia tabla y se dispara a sí mismo.

### 6. Persistencia: `ObjectKind::Trigger` en el catálogo (VERSION 14)

El catálogo, que desde V13 ya tenía discriminator byte `[kind:u8]` (`0=Table`, `1=View`), suma `2=Trigger`. El payload del trigger es:

```
[name][table][timing:u8][event:u8][body_sql]
```

donde `body_sql` es el texto SQL del body — mismo enfoque que `ViewMeta.source`. Bump `VERSION 13 → 14`; V13 abierto por un binario X1+ rebota con `[GBY-1003]`.

### 7. Namespace global con tablas y vistas

`CREATE TRIGGER name ...` colisiona con cualquier nombre de tabla/vista/trigger ya existente (`[GBY-4092]`). Mismo trato que vistas — el catálogo usa un B+Tree único keyed por hash del nombre.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4092` | `TRIGGER_NAME_COLLIDES` | El nombre colisiona con una tabla, vista u otro trigger. |
| `GBY-4093` | `TRIGGER_BODY_INVALID` | Body no es DML, body vacío, o BEFORE pedido (diferido a X2). **Actualización 2026-06-15**: X2 ([ADR-0030](0030-triggers-before-multistmt-x2.md), 2026-05-28) entregó BEFORE triggers + body multi-statement. Hoy `[GBY-4093]` se emite solo si el body es inválido por otra razón. |
| `GBY-4094` | `TRIGGER_NEW_OLD_OUT_OF_SCOPE` | `NEW.x` en DELETE, `OLD.x` en INSERT, o columna inexistente. |
| `GBY-4095` | `TRIGGER_RECURSION_DEPTH_EXCEEDED` | Cascada de triggers > 16 niveles. |
| `GBY-4096` | `TRIGGER_NOT_FOUND` | `DROP TRIGGER` sobre un nombre que no existe (sin `IF EXISTS`). |

## 🧪 Validación

Suite `x1_*` en `tests/integration_test.rs` (10 tests):

- `x1_after_insert_audit`: auditoría clásica con `NEW.col` en INSERT VALUES.
- `x1_after_update_uses_new_and_old`: log de cambios con ambos NEW y OLD.
- `x1_after_delete_uses_old`: tombstone table.
- `x1_trigger_persists_across_reopen`: el trigger sobrevive al close del pager.
- `x1_drop_trigger_works` + `x1_drop_trigger_if_exists_noop`: lifecycle.
- `x1_before_rejected_in_release`: `[GBY-4093]` para BEFORE.
- `x1_trigger_name_collides_with_table`: `[GBY-4092]`.
- `x1_trigger_recursion_guard`: cascada UPDATE→trigger→UPDATE rebota `[GBY-4095]` a 16.
- `x1_trigger_body_must_be_dml`: SELECT como body → `[GBY-4093]`.

Suite total: **417/417 pass** (`cargo test --lib --tests`).

## 🔭 Futuro (X2, X3...)

- **BEFORE triggers**: ejecutar antes del write; con habilidad de abortar (e.g. trigger que lanza error → rollback). Requiere semántica clara de NEW antes-de-defaults.
- **Body multi-statement con `BEGIN ... END`**: necesario para triggers complejos que hacen más de una cosa.
- **Lenguaje procedural** (variables, IF/THEN, LOOP, EXCEPTION) — PL/pgSQL básico.
- **Triggers sobre vistas** con INSTEAD OF.
- **`CREATE FUNCTION` con cuerpo SQL** (P1) y con cuerpo PL/pgSQL (P3).
- **`CREATE PROCEDURE` + `CALL`**.
- **`INSERT ... VALUES (expr, ...)`** acepta Expr en posición de literal — sacaría la restricción que motivó la substitución a nivel de token.
- **OLD para UPSERT que terminó en UPDATE** — capturar OLD dentro de `apply_insert_row_with_conflict`.
