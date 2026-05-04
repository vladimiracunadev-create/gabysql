//! Secondary index machinery.
//!
//! A secondary index is a B+Tree whose key is a 64-bit FNV-1a hash of the
//! indexed column value, and whose value is a *bucket* listing every
//! `(serialized_value, primary_key)` pair that hashes to that key. Storing
//! the serialized value alongside the PK lets the engine distinguish hash
//! collisions from real matches at lookup time, and it lets duplicate
//! values share a bucket without losing any of them.
//!
//! Bucket layout on disk:
//!     [count:u16] + count × ([vlen:u16][value_bytes][pk:i64])
//!
//! Today only equality lookups (`WHERE col = N`) hit the index; range and
//! ordered scans over secondary indexes are out of scope for this hito.

use crate::catalog::{Column, ColumnType, TableMeta};
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
        return Err(DbError::new("bucket de índice corrupto"));
    }
    let count = u16::from_le_bytes(data[0..2].try_into().unwrap()) as usize;
    let mut pos = 2usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if pos + 2 > data.len() {
            return Err(DbError::new("bucket de índice corrupto (vlen)"));
        }
        let vlen = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        if pos + vlen + 8 > data.len() {
            return Err(DbError::new("bucket de índice corrupto (value+pk)"));
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
        return Err(DbError::new(
            "bucket de índice excede u16::MAX entradas (colisión patológica)",
        ));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (value, pk) in entries {
        if value.len() > u16::MAX as usize {
            return Err(DbError::new("valor indexado excede u16::MAX bytes"));
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

/// Validate that the column type can be used as an index key in this
/// version. (Today: any of the supported scalar types except JSON, which
/// has no canonical equality semantics.)
pub fn validate_indexable(meta: &TableMeta, column: &str) -> DbResult<()> {
    let col = meta
        .column(column)
        .ok_or_else(|| DbError::new(format!("columna no existe: {}", column)))?;
    if matches!(col.column_type, ColumnType::Json) {
        return Err(DbError::new(
            "no se admiten índices sobre columnas JSON en esta versión",
        ));
    }
    Ok(())
}
