# ADR-0019: PRIMARY KEY compuesta + índices compuestos (VERSION 8)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-26
**Contexto que la motiva**: bloque K2 — habilitar `PRIMARY KEY (a, b, ...)` y `CREATE [UNIQUE] INDEX idx ON t (a, b, ...)` sin reescribir el B+Tree de claves i64.

## 🧭 Contexto

Hasta VERSION 7 el motor asumía PK escalar única y exactamente un índice secundario por columna:

- `TableMeta.primary_key: String` — UNA columna.
- `IndexMeta.column: String` — UNA columna.
- El B+Tree (ADR-0004 / ADR-0017) usa claves i64.
- INSERT/UPDATE extraen la PK directamente del valor INT del row.
- `WHERE col = X` y `WHERE col BETWEEN a AND b` planifican sobre esa única columna.

El backlog enterprise pide PK compuesta (`(curso, alumno)`, `(year, month, sku)`, etc.) y índices multi-columna para acelerar consultas con varios predicados AND. Sin esto, casos triviales como una tabla de asistencias (`PRIMARY KEY (curso, alumno)`) no se pueden expresar — el usuario tiene que falsear una PK sintética + UNIQUE, lo que rompe la equivalencia ANSI.

Las restricciones que pesan:

- **ADR-0001**: cero dependencias externas.
- **ADR-0004**: el B+Tree es i64-keyed; portarlo a byte-keys es una refactor enorme — fuera del scope de K2.
- **ADR-0005 / ADR-0017**: los formatos de bucket actuales (hash vs ordered-int) deben seguir funcionando.
- **VERSION 7 rechaza V6 limpiamente** — el bump VERSION 7 → 8 sigue el mismo patrón.
- **Cada bump = recreación manual** (ningún auto-upgrade): la complejidad de migrar índices secundarios entre layouts es demasiado alta para auto-resolver.

## 💡 Decisión

Tres cambios coordinados bajo VERSION 8, todos restringidos a **claves compuestas all-INT NOT NULL**:

### 1. Modelo de catálogo aditivo

```rust
pub struct TableMeta {
    pub primary_key: String,           // primera columna PK (caso histórico = única)
    pub primary_key_extra: Vec<String>,// K2: vacío para PK single, ≥ 1 para PK compuesta
    // ...
}

pub struct IndexMeta {
    pub column: String,                // primera columna del índice
    pub extra_columns: Vec<String>,    // K2: vacío para single-column
    // ...
}
```

Helpers `pk_columns()`, `has_composite_pk()`, `is_pk_column(name)`, `all_columns()`, `is_composite()`. Las callsites legacy single-column siguen usando `meta.primary_key` y `idx.column` directos; las nuevas pasan por los helpers.

### 2. Fingerprint FNV-1a-64 como clave compuesta

```rust
// src/index.rs
pub fn encode_composite_key(columns: &[&Column], values: &[&Value]) -> DbResult<i64>;
```

- Itera `(col, val)` en orden canónico.
- Por cada par: `encode_column_value(col, val)` (mismo encoder usado en hash-buckets) → bytes → FNV-1a-64.
- Intercala `0xFF` entre columnas para distinguir `('ab', 'c')` de `('a', 'bc')`.
- Resultado: `i64` (cast del `u64`).

La clave i64 entra directo al B+Tree como cualquier otra. Equality lookup es O(log N + bucket_size). **Range scan sobre claves compuestas NO se soporta** — el fingerprint no es order-preserving.

### 3. Formato VERSION 8 en disco

`TableMeta`:

```
[name][pk_count:u8][pk_col_name × pk_count][root_page:u32][col_count:u16][cols...]
[idx_count:u16] × { [name][column][root_page:u32][unique:u8][kind:u8]
                    [extra_cols_count:u8][extra_col_name × extra_cols_count] }
```

Cambios vs V7:

- PK: `[string]` → `[u8:count][string × count]` (count siempre ≥ 1; count = 1 mapea 1:1 al pre-K2).
- Cada `IndexMeta` añade un trailer `[u8:extra_count][string × extra_count]`.

V7 se rechaza al abrir con `[GBY-1003] UNSUPPORTED_FORMAT_VERSION` y un mensaje que sugiere backup + dump + recreate. **No hay auto-upgrade**: la PK compuesta cambia la semántica de la clave del B+Tree para CUALQUIER tabla creada bajo la nueva forma, y propagar eso a una DB V7 in-place sería un punto de fallo silencioso.

### 4. Reglas operativas para claves compuestas

- **All-INT NOT NULL**: cualquier columna PK compuesta o de un índice compuesto debe ser INT NOT NULL. Errores:
  - `[GBY-4064] COMPOSITE_PK_REQUIRES_ALL_INT`
  - `[GBY-4067] COMPOSITE_INDEX_REQUIRES_ALL_INT`
- **No partial lookup**: `WHERE a = 1` contra PK `(a, b)` cae a full-scan (correcto, sin error). El planner registra el caso pero por ahora lo silencia.
- **UPDATE bloquea TODA columna PK** (single o compuesta) → `[GBY-4008] UPDATE_PK_NOT_ALLOWED`.
- **DELETE**: funciona vía full-scan + filtro 3VL cuando el WHERE es AND-de-equalities sobre todas las columnas PK (no hay fast-path por fingerprint — el costo de implementarlo no justifica el ahorro hasta que aparezcan benchmarks reales que lo motiven).
- **FK siguen single-column**: la única columna padre permitida es la PK del padre cuando ésta es single, o una UNIQUE explícita. K2 no extiende FK a multi-col.
- **PK duplicada**: inline + table-level → `[GBY-4065] PRIMARY_KEY_DUPLICATED`.

### 5. Índices compuestos: bucket layout

El índice compuesto vive como `IndexKind::OrderedInt` (no por su orden semántico — un fingerprint no tiene orden útil — sino por reutilizar el decoder existente de `decode_ordered_bucket`). Bucket = `[count:u16] + count × [pk:i64]`. La clave del B+Tree es el fingerprint i64 calculado por `encode_composite_key`. INTEGRITY CHECK recorre los buckets con el mismo decoder.

UNIQUE compuesto: el backfill detecta colisiones por fingerprint y emite `[GBY-3003] UNIQUE_VIOLATED`. Una colisión real de FNV-1a-64 (≠ duplicación) es astronómicamente improbable sobre tuplas de INT.

## 🔄 Alternativas consideradas

- **B+Tree byte-keyed real**: rechazado. Reescribir el indexing + cursors + range scan para soportar claves arbitrarias de bytes es un proyecto multi-bloque; rompe ADR-0004 y obliga a revisar todos los call sites del Tree. K2 explícitamente lo deja fuera.
- **Encoder order-preserving multi-column** (concatenación con prefix-length escaping): viable pero requiere también claves byte-string en el B+Tree. Mismo problema que el anterior. Para K2 el equality lookup es suficiente.
- **PK compuesta mixta (INT + TEXT, etc.)**: rechazado para K2. El fingerprint funciona sobre cualquier tipo, pero abrir la puerta a TEXT/FLOAT/etc. obliga a documentar y testear casos de borde (NULL en TEXT vs INT, encoding consistente cross-platform, etc.). All-INT mantiene la superficie chica.
- **Auto-migración V7 → V8 al abrir**: rechazado. Reescribir todos los índices secundarios con el nuevo layout en un solo paso, atómico, con CRC + WAL, sin tener garantías de que la PK compuesta no introdujo colisiones nuevas en los buckets, es un punto de fallo silencioso inaceptable. Migración manual (`dump + recreate`) es explícita y verificable.

## ⚖️ Consecuencias

**Positivas**

- PK e índices compuestos cubren el caso enterprise común (joins por clave múltiple, dedup multi-key, surrogate-keys más fieles a las relaciones del dominio).
- El layout aditivo mantiene 100% de back-compat con código y tests pre-K2: 283 tests prior pasan sin tocarse.
- Cero cambios al B+Tree, al WAL ni al page cache. La complejidad K2 vive en `catalog.rs`, `index.rs` y el planner del WHERE.
- El bump de VERSION rompe limpiamente las DBs viejas — no hay corrupción silenciosa posible.

**Negativas / deuda**

- Range scan sobre claves compuestas no soportado. Casos como `WHERE (year, month) BETWEEN (2025, 1) AND (2026, 12)` requieren full-scan o reescribir el predicado en INT-puro.
- Partial lookup sobre PK compuesta (`WHERE a = ?`) cae a full-scan silenciosamente. Sin warning.
- FK siguen single-column. Las relaciones multi-col necesitan ser modeladas como ID surrogate + UNIQUE compuesta.
- ALTER de PK compuesta sobre tabla existente queda fuera (creación nueva sí, ALTER no — bloque futuro).
- Migración V7 → V8 es manual.

## 📎 Referencias

- ADR-0001 (cero deps)
- ADR-0004 (B+Tree i64-keyed)
- ADR-0005 (índice hash-bucket)
- ADR-0017 (índice INT-ordenado)
- `src/catalog.rs::TableMeta` (modelo aditivo)
- `src/index.rs::encode_composite_key` (fingerprint)
- `src/storage.rs::VERSION` (bump 7 → 8)
- Tests: `tests/integration_test.rs::k2_*` (17 tests)
