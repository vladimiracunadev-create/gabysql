# ADR-0040: Enforcement de longitud `VARCHAR(n)` / `CHAR(n)` (Y2)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Y2 (primer sub-bloque post-Y)
**Bump on-disk**: 17 → 18

## 🧭 Contexto

Y dejó pasar `VARCHAR(n)` y `CHAR(n)` como **aliases puros de TEXT**: el `(n)` se aceptaba sintácticamente pero no se persistía ni se enforcaba. Eso es suficiente para portar schemas sin editar, pero no para validar la calidad de los datos: una columna `email VARCHAR(254)` no protegía de inserts mucho más grandes.

Y2 cierra esa brecha. Es el sub-bloque más liviano de los que quedaron diferidos en Y porque no requiere cambios al `Value` enum ni a la serialización de filas — solo persiste un `Option<u32>` por columna en el catálogo y agrega un check en el encoder.

`BLOB`/`BYTEA`, `DECIMAL` exacto, `ARRAY`, range enforcement para `SMALLINT`/`TINYINT`, formato `%` en RAISE, etc. siguen diferidos a Y3+.

## 💡 Decisión

### 1. Persistir `max_length: Option<u32>` en `Column`

```rust
pub struct Column {
    pub name: String,
    pub column_type: ColumnType,
    pub not_null: bool,
    pub default: Option<DefaultLiteral>,
    pub references: Option<ForeignKeyMeta>,
    pub max_length: Option<u32>,  // Y2
}
```

- Sólo se setea cuando el tipo declarado es familia TEXT (`VARCHAR`, `CHAR`, `CHARACTER`, `CHARACTER VARYING`, `NVARCHAR`, `NCHAR`, `STRING`, `CLOB`) y trae `(n)`.
- `NUMERIC(p,s)`/`DECIMAL(p,s)` también traen paréntesis pero `extract_length_param` los devuelve `None` (sólo TEXT family).
- `(n,m)` con coma se ignora (no aplica a TEXT).

### 2. Disk format

Nuevo flag bit:

```rust
const COLUMN_FLAG_HAS_MAX_LENGTH: u8 = 0x08;
```

Cuando está prendido, después del bloque opcional de FK (que en sí mismo es variable-length) se escriben **4 bytes LE** con el `u32`. El decoder lo lee condicionalmente al flag.

Columnas escritas con el flag apagado (= sin `max_length`, lo normal) son byte-idénticas a V17. La adición es 100% additive en wire format.

### 3. Bump 17 → 18

Necesario aunque la adición sea additive: un binario V17 leyendo un schema V18 con `max_length` no sabría que después del FK puede haber 4 bytes extra y leería el `idx_count` desde el offset equivocado. Bump obligatorio. V17 → rechazado con `[GBY-1003]` (export/import manual).

### 4. Helper `extract_length_param`

Función pura en `sql.rs`:

```rust
pub(crate) fn extract_length_param(type_name: &str) -> Option<u32>
```

Toma el `type_name` ya parseado (e.g. `"VARCHAR(255)"`, `"CHAR(10)"`) y devuelve `Some(n)` si:

- Hay un `(`.
- El identificador base (todo lo que va antes del `(`, normalizado a uppercase + whitespace colapsado) pertenece a la familia TEXT.
- El contenido entre paréntesis es un `u32` válido sin comas.

Casos en que devuelve `None`:

- `VARCHAR` sin `(...)` — sin límite.
- `NUMERIC(10,2)` — tiene coma; además no es text family.
- `DECIMAL(8,4)` — no es text family.
- `INT(11)` — base no es text family (acá `(11)` es la display width legacy de MySQL, que ignoramos).
- `VARCHAR(abc)` — no parsea como u32.

### 5. Enforcement en el encoder

Una sola línea agregada al arm de `stores_as_text` en `encode_row`:

```rust
if let Some(max) = column.max_length {
    if bytes.len() > max as usize {
        return Err(coded(
            codes::VALUE_LENGTH_EXCEEDED,
            format!("valor para columna '{}' excede {} bytes declarados ({} bytes recibidos)", ...),
        ));
    }
}
```

Como INSERT y UPDATE (incluyendo `INSERT...SELECT` y CTAS) terminan llamando al mismo encoder, el check cubre todos los paths sin duplicación.

### 6. Semántica: bytes UTF-8, no code points

`max_length` se mide en **bytes UTF-8**, igual que el length-prefixed encoding global (`u16` para el largo). Esto difiere de PostgreSQL (`character varying` cuenta caracteres) pero coincide con MySQL `VARCHAR ... CHARACTER SET utf8mb4` en bytes. La decisión se documenta para que un futuro modo de conteo por code points pueda agregarse como `Option<LengthUnit>` sin romper el wire format.

## 📐 Código de error nuevo

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4119` | `VALUE_LENGTH_EXCEEDED` | INSERT/UPDATE de string que excede `VARCHAR(n)`/`CHAR(n)`. |

## 🧪 Validación

Suite `y2_*` en `tests/integration_test.rs` (8 tests):

- `y2_varchar_under_limit_works`: caso típico.
- `y2_varchar_exact_limit_works`: borde inferior (`n` exacto).
- `y2_varchar_over_limit_rejected`: borde superior — error `[GBY-4119]`.
- `y2_char_n_enforces_too`: misma regla aplica a `CHAR(n)`.
- `y2_text_without_n_has_no_limit`: regression — `TEXT` sin paréntesis sigue sin límite Y2 (queda el global 65535).
- `y2_update_also_enforced`: UPDATE también dispara el check.
- `y2_alter_add_varchar_n_persists_limit`: `ALTER TABLE ADD COLUMN ... VARCHAR(n)` también persiste el límite.
- `y2_limit_survives_reopen`: el `max_length` viaja en disco — DB cerrada y reabierta sigue enforzando.

Suite total: **510/510 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

> 📝 **Actualización 2026-06-15**: la mayoría de estos pendientes ya fueron entregados en bloques posteriores. Lista anotada abajo.

Pendientes que aún no entran:

- **Conteo por code points** en `CHAR(n)`/`VARCHAR(n)` (opt-in vía cláusula `CHARACTER SET`). **Sigue pendiente.**
- ~~**Range enforcement** para `SMALLINT` y `TINYINT`~~ ✅ entregado por **Y3** ([ADR-0043](0043-int-range-enforcement-y3.md), 2026-05-29). Hoy también `MEDIUMINT`, `INT4` y `UNSIGNED` (Y5/[ADR-0045](0045-unsigned-and-uuid-y5.md)).
- ~~**`BLOB`/`BYTEA`** con `Value::Bytes` real~~ ✅ entregado por **Y4** ([ADR-0044](0044-blob-bytea-y4.md), 2026-05-29).
- ~~**`DECIMAL(p,s)` exacto** con `Value::Decimal`~~ ✅ entregado por **Y6** ([ADR-0046](0046-decimal-exact-y6.md), 2026-05-29) + aritmética Y7/Y8 ([ADR-0047](0047-decimal-arith-compare-y7.md)/[ADR-0048](0048-decimal-mul-div-mod-y8.md)).
- **`CHAR(n)` con padding** (estándar SQL exige padding a `n` espacios — hoy se guarda como vino). **Sigue pendiente.**
