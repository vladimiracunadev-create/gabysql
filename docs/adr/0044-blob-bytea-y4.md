# ADR-0044: Tipo binario `BLOB` / `BYTEA` / `BINARY` (Y4)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Y4 (tercer sub-bloque post-Y)
**Bump on-disk**: 19 → 20

## 🧭 Contexto

Y2/Y3 enforcaron longitud y rango sobre los tipos ya existentes. Y4 abre una **nueva familia**: bytes crudos. Esto es invasivo porque implica:

- Nueva variante `Value::Bytes(Vec<u8>)` — todos los `match Value` exhaustivos del crate hay que ampliarlos.
- Nuevo `ColumnType::Blob` (code=10) — todos los matches sobre ColumnType.
- Encoding propio en disco (u32 LE length + raw bytes — distinto del u16 que usa la familia text).
- Nueva sintaxis de literal: `X'hex'` (estándar SQL).
- Nuevo TokenKind para que el parser no confunda `X'cafe'` con un identifier `X` seguido de un string `'cafe'`.

`DECIMAL` exacto, `UNSIGNED *`, `CHAR(n)` con padding y los `WITH TIME ZONE` siguen diferidos.

## 💡 Decisión

### 1. `Value::Bytes(Vec<u8>)`

Nueva variante del enum. Cobra sentido como tipo escalar normal — viaja por todos los flujos del engine como cualquier otro valor. Las operaciones no-binarias (aritmética, comparaciones SQL ordenadas, agregados numéricos) la rechazan con type mismatch.

### 2. `ColumnType::Blob` code=10

```rust
"BLOB" | "BYTEA" | "BINARY" | "VARBINARY" => Ok(Self::Blob),
```

Cuatro aliases sintácticos para portar schemas de cualquier dialecto. NO `stores_as_text()` — usa un path de encoding propio.

### 3. Disk format

Bytes encoding:

```
[present:u8=1][len:u32 LE][bytes...]
```

`NULL` se codifica con el `present=0` heredado. La diferencia con TEXT es el `len:u32` (vs `u16` para text-family) — los BLOBs pueden ser más grandes y consideramos que 65 535 bytes era una limitación más restrictiva de lo razonable. El upper bound práctico queda en el tamaño de página del B+Tree (~4 KB hoy), pero el encoding no impone u16.

### 4. Sintaxis `X'hex'` (estándar SQL)

Tokenizer nuevo:

```rust
if (ch == 'X' || ch == 'x') && index + 1 < chars.len() && chars[index + 1] == '\'' {
    // ... extrae hasta el siguiente '\''
    tokens.push(Token { kind: TokenKind::Blob, text: hex });
}
```

- `X''` → bytes vacíos (válido).
- `X'deadbeef'` → 4 bytes.
- Largo impar → `[GBY-4122]`.
- Char no-hex → `[GBY-4122]`.

`parse_hex_to_bytes(s)` decodifica el texto del token. Acepta también prefijo `0x` para uso en `CAST('0xdeadbeef' AS BLOB)`.

### 5. `CAST(text AS BLOB)`

Acepta string hex (con o sin `0x`), parsea con `parse_hex_to_bytes`. Sin restricción sobre tipo de origen para `Bytes` → `Bytes` (no-op). Otros tipos → `[GBY-4036]`.

### 6. Display

- `Value::Bytes(b)` → `bytes_to_hex_display(b)` (con prefijo `0x`, lowercase).
- En JSON server output: `"0xdeadbeef"` (string en JSON).
- En `format_value_literal` (catálogo / CHECK constraints): `X'deadbeef'` (literal SQL canónico).

### 7. Bump 19 → 20

Necesario por el code de columna 10 en disco. V19 rechazado con `[GBY-1003]`.

## 📐 Códigos de error

| Código | Nombre | Cuándo |
|---|---|---|
| `GBY-4122` | `BLOB_LITERAL_INVALID` | `X'hex'` con largo impar, char no-hex, o `CAST('s' AS BLOB)` con `s` no hex. |

## 🚫 Limitaciones explícitas

- **BLOB no indexable** — no se puede hacer `CREATE INDEX ... ON t (blob_col)`. La comparación bytewise está disponible vía `=` pero no hay clave estable para el secondary B+Tree.
- **BLOB no comparable con `<`/`>`/`BETWEEN`** — solo equality (`=`/`<>`).
- **DEFAULT BLOB no soportado** — `value_to_default` rechaza Bytes (el catálogo `DefaultLiteral` no tiene variante binaria).
- **BLOB en FK/PK no soportado** — la PK debe ser INT, las FK apuntan a PK INT.
- **BLOB en CHECK no soportado** (limitación del evaluador de CHECK).
- Tamaño práctico limitado por la página del B+Tree (~4 KB hoy). Para datos más grandes esperar a un overflow chain explícito.

## 🧪 Validación

Suite `y4_*` en `tests/integration_test.rs` (13 tests):

- `y4_blob_insert_and_select_roundtrip` (deadbeef + 00ff7f80)
- `y4_blob_empty_literal_works` (`X''`)
- `y4_blob_lowercase_x_literal` (`x'cafe'`)
- `y4_bytea_alias_works`, `y4_binary_alias_works`
- `y4_blob_odd_hex_length_rejected` (4122)
- `y4_blob_non_hex_char_rejected` (4122)
- `y4_cast_text_to_blob` (`CAST('0xdeadbeef' AS BLOB)`)
- `y4_cast_to_blob_rejects_bad_hex`
- `y4_blob_null_value_works`
- `y4_blob_large_payload` (256 bytes, 0..255)
- `y4_blob_survives_reopen`
- `y4_unsupported_type_geometry_still_errors`

Suite total: **557/557 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

- **`DECIMAL(p,s)` exacto** — `Value::Decimal` con i128+scale o string interno.
- **`UNSIGNED TINYINT/SMALLINT/INT/BIGINT`** (MySQL-style).
- **`CHAR(n)` con padding** a la derecha (ANSI strict).
- **Conteo por code points** en VARCHAR(n).
- **`ARRAY[T]`**, **`ENUM(...)`**, **`INTERVAL`**, **`TIME/TIMESTAMP WITH TIME ZONE`**.
- **`gen_random_uuid()`** y similares.
- **BLOB indexable** (overflow chain + key bytewise).
- **Conversión BLOB → TEXT con encoding explícito** (`CONVERT(blob USING utf8)`).
