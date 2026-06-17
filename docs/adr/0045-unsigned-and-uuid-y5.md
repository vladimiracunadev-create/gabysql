# ADR-0045: `UNSIGNED` enforcement + `gen_random_uuid()` (Y5)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Y5 (cuarto sub-bloque post-Y)
**Bump on-disk**: 20 → 21

## 🧭 Contexto

Y3 enforce el rango signed para `TINYINT/SMALLINT/MEDIUMINT/INT4`. Pero la sintaxis MySQL `<tipo> UNSIGNED` es ultra-común en schemas reales (ID auto-incrementales, contadores, flags) y portar esos schemas a gabysql requería reescribir todo a tipos signed con rangos más amplios.

Y5 cierra esa brecha sin agregar bytes nuevos en disco — reutiliza el byte `int_width` de Y3 prendiendo el high bit como flag de signedness.

Además agrega `gen_random_uuid()` (UUID v4 random), la función más pedida después de tipo UUID — sin ella, un schema con `id UUID DEFAULT gen_random_uuid()` no se puede portar.

## 💡 Decisión

### 1. `UNSIGNED` reusa el byte `int_width`

```
int_width byte layout (Y5):
┌──┬──┬──┬──┬──┬──┬──┬──┐
│ S│ 0│ 0│ 0│ W3│W2│W1│W0│
└──┴──┴──┴──┴──┴──┴──┴──┘
 │              └────────── width (1=TINYINT, 2=SMALLINT, 3=MEDIUMINT, 4=INT4, 0=BIGINT default)
 └────────────────────────── sign: 0=signed, 1=unsigned
```

Ejemplos:
- `0x01` → TINYINT signed (Y3)
- `0x81` → TINYINT UNSIGNED (Y5)
- `0x02` → SMALLINT signed
- `0x82` → SMALLINT UNSIGNED
- `0x04` → INT4 signed
- `0x84` → INT4 UNSIGNED
- `0x80` → BIGINT UNSIGNED (no width restriction, solo positivo)

V20 reading V21 con bytes `0x8X` los interpretaría como widths inválidos → bump 20→21 obligatorio.

### 2. Parser: `UNSIGNED` opcional tras el tipo

`parse_column_def` agrega un `match_keyword("UNSIGNED")` después de `parse_type_name`. Si el tipo no es INT, el flag se ignora silentemente — no es error, sólo no aplica.

### 3. Rangos con `int_width_range`

Extendido para chequear el high bit:

```rust
match (unsigned, width) {
    (false, 1) => i8 range,
    (true, 1) => (0, u8::MAX),
    (false, 2) => i16 range,
    (true, 2) => (0, u16::MAX),
    (false, 3) => 24-bit signed,
    (true, 3) => (0, 16_777_215),
    (false, 4) => i32 range,
    (true, 4) => (0, u32::MAX),
    (false, _) => (i64::MIN, i64::MAX),
    (true, _) => (0, i64::MAX),  // BIGINT UNSIGNED — limitado por i64 interno
}
```

### 4. `BIGINT UNSIGNED` no llega a u64

El motor sigue usando `i64` internamente. `BIGINT UNSIGNED` enforce sólo `>= 0` — el upper bound queda en `i64::MAX` (9_223_372_036_854_775_807), no `u64::MAX`. Documentado como limitación.

### 5. `gen_random_uuid()` / `uuid_v4()`

UUID v4 (random), formato canónico `8-4-4-4-12` lowercase:

```sql
SELECT GEN_RANDOM_UUID();   -- '5f8b1c2d-4e7a-4b8c-9d0e-1f2a3b4c5d6e'
SELECT UUID_V4();           -- alias
SELECT UUID_GENERATE_V4();  -- alias (PG)
SELECT RANDOM_UUID();       -- alias
```

PRNG interno: xorshift64 seeded por `SystemTime::nanos` XOR un magic constant. **No es criptográficamente seguro** — alcanza para test data, surrogate keys, IDs no-secretos. Sigue RFC 4122 §4.4 para los nibbles de version (`4`) y variant (`10xx`).

Cero deps externas — no agregamos `uuid` crate. La calidad estadística es suficiente para descubrir colisiones en suites de test (~10^18 valores posibles).

## 📐 Códigos de error

Reutiliza `[GBY-4121]` `INT_RANGE_EXCEEDED` para violaciones de UNSIGNED — el mensaje ahora identifica el tipo como `TINYINT UNSIGNED` etc.

## 🧪 Validación

Suite `y5_*` en `tests/integration_test.rs` (10 tests):

- `y5_tinyint_unsigned_in_range_works` (0, 255, 128 OK)
- `y5_tinyint_unsigned_negative_rejected` (-1 → 4121)
- `y5_tinyint_unsigned_over_range_rejected` (300)
- `y5_smallint_unsigned_range` (65535 OK, 70000 error)
- `y5_int4_unsigned_range` (u32::MAX OK, 5e9 error)
- `y5_bigint_unsigned_rejects_negative`
- `y5_unsigned_persists_across_reopen`
- `y5_signed_still_works_unchanged` (regression Y3 — SMALLINT sigue siendo signed)
- `y5_gen_random_uuid_returns_valid_uuid` (formato + version + variant nibbles)
- `y5_uuid_v4_alias_works`

Suite total: **567/567 pass** (`cargo test --lib --tests`).

## 🔭 Futuro

> 📝 **Actualización 2026-06-15**: el "item grande" (DECIMAL exacto) y UUID v7 ya entregados. Lista anotada abajo.

Lo que queda en familia tipos:

- ~~**`DECIMAL(p,s)` exacto** (`Value::Decimal` con i128+scale)~~ ✅ entregado por **Y6** ([ADR-0046](0046-decimal-exact-y6.md), 2026-05-29) + aritmética Y7/Y8 + cierre Y9.
- **`UNSIGNED BIGINT` real** (`u64`) — requiere ampliar `Value::Integer` a un wrapper signed/unsigned. **Sigue pendiente** (UNSIGNED INT/SMALLINT/TINYINT sí enforcen rango desde Y5; el caso BIGINT u64 real no).
- **`CHAR(n)` con padding** ANSI strict a la derecha. **Sigue pendiente.**
- **Conteo por code points** en VARCHAR(n) (vs bytes UTF-8). **Sigue pendiente.**
- **`ARRAY[T]`**, **`ENUM(...)`**, **`INTERVAL`**, **TZ types**. **Siguen pendientes.**
- **BLOB indexable** (overflow chain + bytewise key). **Sigue pendiente** (BLOB existe pero no es indexable).
- **`CONVERT(blob USING utf8)`**. **Sigue pendiente.**
- **UUID v1/v6/v7** (timestamp-based) además de v4 — **v7 entregado por Y9** ([ADR-0049](0049-y9-closure-y-block.md), `UUID_V7()`). v1/v6 siguen pendientes.
- **`gen_random_bytes(n)`** scalar function. **Sigue pendiente.**
