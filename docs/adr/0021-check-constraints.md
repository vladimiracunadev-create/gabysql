# ADR-0021: `CHECK (expr)` constraints — persistencia por texto canónico

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-27
**Bloque**: L2 (sub-bloque del bloque L del roadmap; cierra L)
**Bump on-disk**: VERSION 9 → 10

## 🧭 Contexto

L1 cerró las acciones referenciales y `UNIQUE` multi-col, pero dejó pendiente la mitad de "constraints" del bloque L: `CHECK (expr)` column-level y table-level. Es el último constraint declarativo del SQL clásico que faltaba en gabysql.

Pre-L2:
- El motor podía evaluar `Expr` complejas en `WHERE`, `HAVING`, `SELECT list`, `UPDATE SET`, etc. (bloques E1-E3, F, G1-G3, H, I).
- No había modo de **declarar** un predicado a nivel de tabla y que el motor lo validara automáticamente en cada write.
- El catálogo no tenía mecanismo para persistir una expresión arbitraria.

## 💡 Decisión

### 1. Persistir el `source` SQL canónico — no el AST

Tres opciones evaluadas:

| Opción | Pros | Contras |
|---|---|---|
| **Serializar `Expr` AST en binario** | Lectura instantánea | Acopla el formato on-disk al AST. Cada bloque G/H/I que agrega variantes a `Expr` fuerza un bump de VERSION. |
| **Texto SQL canónico** (elegida) | Forward-compatible: el AST puede evolucionar sin tocar el catálogo. Catálogo legible (`INTEGRITY CHECK` muestra el CHECK literal). | Pequeño overhead de re-parse en cada write (lex+parse de la expresión). |
| **Texto SQL **original** del usuario** | Cero conversión al guardar | Si la canonicalización cambia (e.g. comportamiento del lexer), el catálogo persistido se vuelve no-re-parseable silenciosamente. |

Elegimos **texto canónico** porque (a) el costo de re-parse es despreciable comparado al I/O, (b) desacopla la evolución del AST del formato on-disk, y (c) el round-trip queda determinístico vía `format_expr` (no depende del whitespace del usuario).

### 2. `format_expr(&Expr) -> DbResult<String>`

Serializador que envuelve binarios (`Compare`, `Arith`, `Like`, `InList`, `Between`, `IsNull`) en paréntesis para que la salida sea precedencia-neutra. Strings con `'...'` y escape doble (`''`); floats con punto decimal explícito para que el lexer no los confunda con ints; `EXTRACT(field FROM date)` con su sintaxis especial. `ScalarSubquery` se rechaza con `[GBY-4069]`.

### 3. `parse_expr_str(source) -> DbResult<Expr>`

Contraparte pública. Tokeniza + parsea un `Expr` standalone, exigiendo EOF al final. Usado en (a) el validator DDL (`validate_check_constraints`) y (b) el enforcement runtime (`enforce_check_constraints`).

### 4. Evaluación en cada write

`enforce_check_constraints(meta, values)` itera los CHECKs, re-parsea cada `source`, evalúa con `eval_expr_as_predicate` (3VL ANSI):

- `Ok(Some(true))` → pasa
- `Ok(None)` → pasa (NULL via 3VL, regla ANSI)
- `Ok(Some(false))` → `[GBY-3008] CHECK_VIOLATED`
- `Err(_)` → re-emite con contexto del CHECK afectado

Hooks de invocación:
- `exec_insert` → después de NOT NULL, antes de UNIQUE/FK.
- `exec_update` (path compartido por UPSERT DO UPDATE) → después del merge de overrides, antes de UNIQUE/FK.
- `cascade_set_fk_value` → después de mutar el FK col del child, antes de tocar disco. Sin esto, un `ON DELETE SET NULL` podría violar un CHECK del child silenciosamente.

### 5. Validación en DDL

`validate_check_constraints(meta)` corre justo después de `validate_fk_targets`:
- Re-parsea cada `source` (smoke check del round-trip).
- Rechaza `ScalarSubquery` con `[GBY-4069]`.
- Recorre el AST y exige que cada `Expr::Column(name)` apunte a una columna existente (`[GBY-2002]`). Acepta qualifier `t.col` si `t` matchea el nombre de la tabla; refs cross-table rebotan.

No hay type-checking estricto del predicado — la regla "debe evaluar a BOOL o NULL" se delega al evaluador en runtime, que ya emite errores claros. Reservamos `[GBY-4070] CHECK_EXPR_NOT_BOOLEAN` por si añadimos type-check estático más adelante.

### 6. Nombres

Si el usuario declara `CONSTRAINT name CHECK (...)`, el `name` persiste tal cual y aparece en los mensajes de `[GBY-3008]`. Sin nombre, sintetizamos `<table>_check_<N>` con N monotónico empezando en 1. Esto da diagnósticos legibles tanto cuando el SQL es escrito a mano como cuando viene de un ORM.

### 7. Formato on-disk

Trailer nuevo al final del record de `TableMeta`:

```
[check_count:u16] · check × { [name:string][source:string] }
```

Las tablas pre-L2 escriben `check_count = 0` y son indistinguibles del caso "sin CHECK". V9 files rechazados con `[GBY-1003]` al abrir (sin migración automática — la regla del repo).

## 🚧 Consecuencias y limitaciones

| Tema | Estado L2 |
|---|---|
| `CHECK (expr)` column-level | ✅ |
| `CHECK (expr)` table-level | ✅ |
| `CONSTRAINT name CHECK (...)` con nombre | ✅ |
| 3VL ANSI (NULL pasa) | ✅ |
| CHECK con escalares (G1-G3) | ✅ |
| Subqueries en CHECK | ❌ rechazado en DDL con `[GBY-4069]` (ANSI lo prohíbe; gabysql alinea) |
| `ALTER TABLE ADD CHECK ...` | ❌ requiere re-validar filas existentes; diferido |
| `ALTER TABLE ADD COLUMN ... CHECK (...)` | ❌ misma razón |
| `CONSTRAINT name PRIMARY KEY / UNIQUE / FOREIGN KEY` (nombres en otros constraints) | ❌ sólo CHECK lleva nombre por ahora |
| Migración V9 → V10 | Manual — dump SELECT + recreate |

## 🔄 Alternativas consideradas

- **Type-check estricto del predicado en DDL** (e.g. exigir que evalúe a BOOL): requiere un type-inferencer para `Expr` que hoy no tenemos. El runtime ya rebota con error claro si el CHECK no es booleano (`eval_expr_as_predicate` retorna error). Diferido a un release con type-system propio.
- **Permitir subqueries**: ANSI las prohíbe; la mayoría de motores también (Postgres las acepta pero las marca como "non-standard" y desaconseja). Sin subqueries, el catálogo es self-contained y el eval es O(1) por CHECK.
- **Compilar el AST a un closure en CREATE TABLE y cachearlo**: el ahorro es marginal porque el costo dominante es el I/O, no el parse. Reservado para una optimización futura si el profiling lo justifica.

## 📚 Referencias

- [CHANGELOG.md — 2026-05-27 L2](../../CHANGELOG.md)
- [MISSING_COMMANDS.md § Constraints](../MISSING_COMMANDS.md)
- [ADR-0020 — FK referential actions (L1)](0020-fk-referential-actions.md)
- [Error codes 3008, 4069, 4070](../ERROR_CODES.md)
