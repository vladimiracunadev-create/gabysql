//! Secondary index machinery.
//!
//! Two physical layouts coexist, distinguished by [`IndexKind`] in
//! `IndexMeta`:
//!
//! - **Hash buckets** (`IndexKind::Hash`, ADR-0005): the B+Tree key is a
//!   64-bit FNV-1a hash of the indexed column value, and the value is a
//!   *bucket* listing every `(serialized_value, primary_key)` pair that
//!   hashes to that key. Storing the serialized value alongside the PK
//!   lets the engine distinguish hash collisions from real matches at
//!   lookup time, and it lets duplicate values share a bucket without
//!   losing any of them. Used for TEXT / FLOAT / BOOL / DATE / DATETIME
//!   columns and supports equality lookup only.
//!   Bucket layout:
//!   `[count:u16] + count × ([vlen:u16][value_bytes][pk:i64])`
//!
//! - **INT-ordered** (`IndexKind::OrderedInt`, ADR-0017, VERSION 7+): the
//!   B+Tree key **is** the indexed `INT` value itself, so
//!   `Tree::cursor_range(from, to)` walks index entries in true value
//!   order and we can satisfy `WHERE col_idx BETWEEN a AND b`. The bucket
//!   payload only needs the PKs because the value is recoverable from the
//!   key. NULL is **not** indexed here (SQL `BETWEEN` ignores NULLs and
//!   UNIQUE allows multiple NULLs, so skipping them keeps both semantics
//!   intact).
//!   Bucket layout:
//!   `[count:u16] + count × [pk:i64]`

use crate::catalog::{Column, ColumnType, TableMeta};
use crate::errors::{coded, codes};
use crate::sql::Value;
use crate::{DbError, DbResult};

/// Stable 64-bit hash for index keys. Same algorithm and seed as
/// `catalog::hash_name`, kept as a copy here to make the on-disk contract
/// of indexes explicit and independent.
pub fn hash_value(bytes: &[u8]) -> i64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash as i64
}

/// Canonical byte representation of a single column value, used both as
/// hash input and as bucket discriminator at lookup time.
///
/// Two values that share this representation are treated as equal by the
/// index. NULL is encoded as a zero-byte payload distinct from any valid
/// value (an empty TEXT is encoded as a 1-byte tag plus zero data, so it
/// does not collide with NULL).
pub fn encode_column_value(column: &Column, value: &Value) -> DbResult<Vec<u8>> {
    let mut out = Vec::new();
    match (&column.column_type, value) {
        (_, Value::Null) => {
            out.push(0); // NULL marker
        }
        (ColumnType::Int, Value::Integer(n)) => {
            out.push(1);
            out.extend_from_slice(&n.to_le_bytes());
        }
        (ColumnType::Float, Value::Float(n)) => {
            out.push(1);
            out.extend_from_slice(&n.to_le_bytes());
        }
        (ColumnType::Float, Value::Integer(n)) => {
            out.push(1);
            out.extend_from_slice(&(*n as f64).to_le_bytes());
        }
        (ColumnType::Bool, Value::Bool(b)) => {
            out.push(1);
            out.push(u8::from(*b));
        }
        (ct, Value::String(s)) if ct.stores_as_text() => {
            out.push(1);
            out.extend_from_slice(s.as_bytes());
        }
        (ct, _) => {
            return Err(DbError::new(format!(
                "valor incompatible con columna de tipo {}",
                ct.as_sql()
            )));
        }
    }
    Ok(out)
}

/// Decode a bucket. Returns `Vec<(value_bytes, pk)>` in insertion order.
pub fn decode_bucket(data: &[u8]) -> DbResult<Vec<(Vec<u8>, i64)>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if data.len() < 2 {
        return Err(DbError::new(format!(
            "bucket de índice (Hash) corrupto: necesito 2 bytes para el count, hay {}",
            data.len()
        )));
    }
    let count = u16::from_le_bytes(data[0..2].try_into().unwrap()) as usize;
    let mut pos = 2usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        if pos + 2 > data.len() {
            return Err(DbError::new(format!(
                "bucket de índice (Hash) corrupto: faltan 2 bytes para el vlen \
                 de la entrada {} de {} en offset {}",
                i, count, pos
            )));
        }
        let vlen = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        if pos + vlen + 8 > data.len() {
            return Err(DbError::new(format!(
                "bucket de índice (Hash) corrupto: entrada {} declara vlen={} \
                 + pk(8) bytes en offset {}, solo quedan {}",
                i,
                vlen,
                pos,
                data.len() - pos
            )));
        }
        let value = data[pos..pos + vlen].to_vec();
        pos += vlen;
        let pk = i64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
        pos += 8;
        out.push((value, pk));
    }
    Ok(out)
}

pub fn encode_bucket(entries: &[(Vec<u8>, i64)]) -> DbResult<Vec<u8>> {
    if entries.len() > u16::MAX as usize {
        return Err(DbError::new(format!(
            "bucket de índice (Hash) excede u16::MAX entradas: tiene {} (límite {}). \
             Esto indica una colisión patológica de FNV-1a-64; reporte el caso.",
            entries.len(),
            u16::MAX
        )));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (value, pk) in entries {
        if value.len() > u16::MAX as usize {
            return Err(DbError::new(format!(
                "valor indexado excede u16::MAX bytes: tiene {} bytes (límite {}, pk={})",
                value.len(),
                u16::MAX,
                pk
            )));
        }
        out.extend_from_slice(&(value.len() as u16).to_le_bytes());
        out.extend_from_slice(value);
        out.extend_from_slice(&pk.to_le_bytes());
    }
    Ok(out)
}

/// Insert `(value, pk)` into a bucket. If an entry with the same `(value,
/// pk)` already exists the bucket is left unchanged (idempotent).
pub fn bucket_insert(entries: &mut Vec<(Vec<u8>, i64)>, value: Vec<u8>, pk: i64) {
    if entries.iter().any(|(v, p)| *p == pk && v == &value) {
        return;
    }
    entries.push((value, pk));
}

/// Remove the first entry that matches `(value, pk)`. Returns whether a
/// match was found.
pub fn bucket_remove(entries: &mut Vec<(Vec<u8>, i64)>, value: &[u8], pk: i64) -> bool {
    let pos = entries
        .iter()
        .position(|(v, p)| *p == pk && v.as_slice() == value);
    match pos {
        Some(i) => {
            entries.remove(i);
            true
        }
        None => false,
    }
}

/// Return every PK whose stored value bytes match `value`.
pub fn bucket_lookup(entries: &[(Vec<u8>, i64)], value: &[u8]) -> Vec<i64> {
    entries
        .iter()
        .filter_map(|(v, pk)| (v.as_slice() == value).then_some(*pk))
        .collect()
}

/// Returns the PK that already owns `value` in a UNIQUE bucket, if any —
/// excluding `exclude_pk` so callers can skip themselves during UPDATE.
/// `None` means the value is free and the caller may proceed to insert.
///
/// NULL is a special case: SQL UNIQUE allows multiple NULLs, so a bucket
/// hit on the NULL marker is *not* treated as a conflict here. Callers
/// pass the canonical encoding produced by `encode_column_value`; the
/// NULL marker is the single byte `0x00`.
pub fn bucket_unique_conflict(
    entries: &[(Vec<u8>, i64)],
    value: &[u8],
    exclude_pk: Option<i64>,
) -> Option<i64> {
    if value == [0u8] {
        return None;
    }
    entries
        .iter()
        .find(|(v, pk)| v.as_slice() == value && Some(*pk) != exclude_pk)
        .map(|(_, pk)| *pk)
}

// ---------- OrderedInt bucket helpers (VERSION 7) ----------

/// Decode the i64 key that an INT column's `value_bytes` (as produced by
/// [`encode_column_value`]) represents. Returns `None` for the NULL
/// marker since NULL is not stored in OrderedInt indexes.
pub fn ordered_int_key_from_value_bytes(value_bytes: &[u8]) -> DbResult<Option<i64>> {
    if value_bytes == [0u8] {
        return Ok(None);
    }
    if value_bytes.len() != 9 || value_bytes[0] != 1 {
        return Err(DbError::new(format!(
            "value_bytes inválidos para índice OrderedInt: tiene {} bytes, esperaba 9 \
             (tag 0x01 + i64 LE); tag={:#04x}",
            value_bytes.len(),
            value_bytes.first().copied().unwrap_or(0)
        )));
    }
    Ok(Some(i64::from_le_bytes(
        value_bytes[1..9].try_into().unwrap(),
    )))
}

/// Decode an OrderedInt bucket: `[count:u16] + count × pk:i64`.
pub fn decode_ordered_bucket(data: &[u8]) -> DbResult<Vec<i64>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if data.len() < 2 {
        return Err(DbError::new(format!(
            "bucket de índice (OrderedInt) corrupto: necesito 2 bytes para el count, hay {}",
            data.len()
        )));
    }
    let count = u16::from_le_bytes(data[0..2].try_into().unwrap()) as usize;
    let expected = 2 + count * 8;
    if data.len() != expected {
        return Err(DbError::new(format!(
            "bucket de índice (OrderedInt) corrupto: count={} implica {} bytes, \
             el bucket tiene {}",
            count,
            expected,
            data.len()
        )));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = 2 + i * 8;
        out.push(i64::from_le_bytes(data[off..off + 8].try_into().unwrap()));
    }
    Ok(out)
}

/// Encode an OrderedInt bucket. Errors out past `u16::MAX` entries on a
/// single key — a pathological case we don't expect under normal usage
/// but check anyway so a bug never silently corrupts the page.
pub fn encode_ordered_bucket(pks: &[i64]) -> DbResult<Vec<u8>> {
    if pks.len() > u16::MAX as usize {
        return Err(DbError::new(format!(
            "bucket de índice (OrderedInt) excede u16::MAX entradas: {} PKs \
             para una misma clave (límite {})",
            pks.len(),
            u16::MAX
        )));
    }
    let mut out = Vec::with_capacity(2 + pks.len() * 8);
    out.extend_from_slice(&(pks.len() as u16).to_le_bytes());
    for pk in pks {
        out.extend_from_slice(&pk.to_le_bytes());
    }
    Ok(out)
}

/// Idempotent insert into an ordered bucket. Keeps the PK list sorted so
/// equality lookups + `WHERE pk = …` short-circuits remain deterministic.
pub fn ordered_bucket_insert(pks: &mut Vec<i64>, pk: i64) {
    if let Err(pos) = pks.binary_search(&pk) {
        pks.insert(pos, pk);
    }
}

/// Remove `pk` from an ordered bucket. Returns whether the bucket changed.
pub fn ordered_bucket_remove(pks: &mut Vec<i64>, pk: i64) -> bool {
    match pks.binary_search(&pk) {
        Ok(pos) => {
            pks.remove(pos);
            true
        }
        Err(_) => false,
    }
}

/// UNIQUE conflict for an OrderedInt bucket: any PK other than
/// `exclude_pk` in the bucket means the value is already taken.
pub fn ordered_bucket_unique_conflict(pks: &[i64], exclude_pk: Option<i64>) -> Option<i64> {
    pks.iter().copied().find(|pk| Some(*pk) != exclude_pk)
}

/// Bloque K2 (VERSION 8): fingerprint determinístico de una clave
/// compuesta (PK o índice compuesto). Reusa la codificación canónica
/// `encode_column_value` por columna y hashea las bytes con FNV-1a-64,
/// intercalando un sentinela `0xFF` entre columnas para minimizar
/// colisiones triviales (`(1, 23) ≠ (12, 3)`).
///
/// El resultado se castea a `i64` y se usa como clave del B+Tree —
/// equality-only por construcción (no es order-preserving). Range scan
/// sobre claves compuestas no se soporta en este release; el usuario
/// que necesita range scan usa PK single-column como antes (ADR-0019).
///
/// Requiere `columns.len() == values.len()` y rechaza NULL en cualquier
/// posición (la PK/UNIQUE compuesta no admite NULLs por construcción;
/// el chequeo aquí es defensa en profundidad).
pub fn encode_composite_key(columns: &[&Column], values: &[&Value]) -> DbResult<i64> {
    if columns.len() != values.len() {
        return Err(DbError::new(format!(
            "encode_composite_key: arity mismatch — {} columnas vs {} valores",
            columns.len(),
            values.len()
        )));
    }
    if columns.is_empty() {
        return Err(DbError::new(
            "encode_composite_key: lista vacía (se necesitan ≥ 1 columnas)",
        ));
    }
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut fnv: u64 = FNV_OFFSET_BASIS;
    for (col, val) in columns.iter().zip(values.iter()) {
        if matches!(val, Value::Null) {
            return Err(coded(
                codes::PRIMARY_KEY_NULL,
                format!(
                    "columna '{}' es parte de una clave compuesta y no admite NULL",
                    col.name
                ),
            ));
        }
        let bytes = encode_column_value(col, val)?;
        for b in bytes {
            fnv ^= b as u64;
            fnv = fnv.wrapping_mul(FNV_PRIME);
        }
        // Sentinela inter-columna: distingue tuplas con bytes concatenados
        // equivalentes (e.g. ('a', 'bc') vs ('ab', 'c')) sin necesidad de
        // escapado costoso.
        fnv ^= 0xFF;
        fnv = fnv.wrapping_mul(FNV_PRIME);
    }
    Ok(fnv as i64)
}

/// Validate that the column type can be used as an index key in this
/// version. (Today: any of the supported scalar types except JSON, which
/// has no canonical equality semantics.)
pub fn validate_indexable(meta: &TableMeta, column: &str) -> DbResult<()> {
    let col = meta.column(column).ok_or_else(|| {
        coded(
            codes::COLUMN_NOT_FOUND,
            format!("columna '{}' no existe en tabla '{}'", column, meta.name),
        )
    })?;
    if matches!(col.column_type, ColumnType::Json) {
        return Err(coded(
            codes::INDEX_ON_JSON,
            format!(
                "no se admiten índices sobre columnas JSON en esta versión \
                 (columna '{}' es JSON)",
                column
            ),
        ));
    }
    Ok(())
}
