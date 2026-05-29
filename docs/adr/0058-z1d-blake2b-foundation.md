# ADR-0058: Blake2b primitive foundation + scheme=3 reservado para Argon2id (Z1d)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z1d (foundation para Argon2id futuro)
**Bump on-disk**: **VERSION 28 → 29**

## 🧭 Contexto

Z1c (ADR-0056) entregó scrypt como KDF memory-hard (~32 MB/hash). El defer documentaba: "Argon2id (scheme=3) — memory-hard también pero con resistencia adicional a side-channel attacks (variant id) y validación formal RFC 9106".

Z1d arrancó con la ambición de implementar Argon2id completo en Rust puro. **Tras un análisis honesto de complejidad** (~600-1000 LOC de código crypto delicado: G compression con BlaMka, memory matrix indexing híbrido data-dependent + data-independent, lane parallelism), decidimos **partir el entregable**:

- **Z1d (este ADR)**: entrega la **primitiva Blake2b RFC 7693** — foundation crypto sobre la que Argon2id se construye. Validada contra el test vector oficial RFC 7693 §A. ~200 LOC, low-risk.
- **Z1e (defer)**: entrega Argon2id completo construido encima de la Blake2b de Z1d. Validado contra RFC 9106 §A.3 test vector.

Esta partición es honesta: en lugar de shippear un Argon2id experimental con potenciales bugs crypto sutiles, shippeamos foundation sólida + reserva clara del slot.

## 💡 Decisión

### 1. Blake2b RFC 7693 puro en Rust

Implementación in-tree de:
- **IV constants** (idéntico a SHA-512 IV).
- **Sigma permutation table** (12 rondas, las últimas 2 son rotación de las primeras 2).
- **G mixing function** (RFC 7693 §3.1).
- **Compression F** (RFC 7693 §3.2) con manejo de counter `t` (u128 — Blake2b admite mensajes hasta 2^128 bytes).
- **API streaming**: init con out_len, update incremental, finalize con last-block flag.
- **Función pública `blake2b(out_len, data) -> Vec<u8>`** para uso desde tests y futuros bloques.

### 2. Test vector validation

```rust
#[test]
fn z1d_blake2b_rfc7693_abc_test_vector() {
    let expected: [u8; 64] = [
        0xBA, 0x80, 0xA5, 0x3F, 0x98, 0x1C, 0x4D, 0x0D, ...
    ];
    assert_eq!(blake2b(64, b"abc"), expected);
}
```

Match byte-for-byte con el test vector oficial RFC 7693 §A — la implementación es **correcta**.

### 3. `PASSWORD_SCHEME_ARGON2ID = 3` reservado

Constante nueva en el dispatch del scheme byte:

```rust
pub const PASSWORD_SCHEME_ARGON2ID: u8 = 3;
```

El dispatch en `exec_set_session_auth` recibe el caso scheme=3 con un mensaje informativo:

```
[GBY-4137] SET SESSION AUTHORIZATION '...': scheme Argon2id (3)
reservado pero no implementado en Z1d (foundation Blake2b ya
disponible; Argon2id full en Z1e)
```

Esto previene confusión si alguien intenta hacer una migración manual del scheme byte en el on-disk format.

### 4. Bump VERSION 28 → 29

Aunque Z1d no cambia el layout (UserMeta sigue idéntico a Z1b/Z1c), bumpeamos VERSION para señalizar el corte: el binario Z1d sabe de Blake2b y del slot scheme=3 reservado; binarios pre-Z1d no.

## 📁 Archivos tocados

- `src/storage.rs`: bump `VERSION 28 → 29`.
- `src/sql.rs`:
  - Constante nueva: `PASSWORD_SCHEME_ARGON2ID = 3`.
  - Bloque Blake2b (~150 LOC): `BLAKE2B_IV`, `BLAKE2B_SIGMA`, `blake2b_g`, `blake2b_compress`, `pub fn blake2b(out_len, data)`.
  - `exec_set_session_auth` dispatch extendido con arm explícito para `PASSWORD_SCHEME_ARGON2ID` que rebota con mensaje informativo.
- `tests/integration_test.rs`: 4 tests `z1d_*`:
  - `z1d_blake2b_rfc7693_abc_test_vector` — match exacto con RFC 7693 §A.
  - `z1d_blake2b_empty_input` — match con vector conocido para BLAKE2b-512("").
  - `z1d_blake2b_variable_output_length` — out_len gobierna el truncado.
  - `z1d_scheme_3_argon2id_reserved_returns_clear_error` — constant exposed = 3, default scheme sigue siendo 2 (scrypt).

## ⛔ Lo que **no** entra en Z1d (defer Z1e)

| Ítem | Razón del defer |
|---|---|
| **G compression** (BlaMka — Argon2 §3.5) | 8 rounds de 4-operación con multiplicación 32-bit. ~30 LOC. |
| **Memory matrix B[lane][block]** | Estructura de N\*128\*r KiB. Para m=64 MiB son 64K bloques de 1 KiB. Indexing crítico. |
| **Indexing híbrido (data-dependent + data-independent)** | Para Argon2id slice 0/1 del pass 0 usan indexing data-independent (anti-side-channel); el resto data-dependent. La elección + el J1/J2 cálculo son el corazón del algoritmo. |
| **H' variable output** (RFC 9106 §3.2) | Wrapper de Blake2b para output de tamaño arbitrario via cadena de Blake2b-64. ~30 LOC. |
| **Pass loop completo** (passes × slices × lanes × blocks) | El bucle principal de Argon2id. ~100 LOC. |
| **Parallelism p>1 lanes** | Lanes independientes. Para password hashing p=1 es típico (single-thread, single-hash). |
| **RFC 9106 §A.3 test vector validation** | Verificación end-to-end contra el output canonical de la RFC. |

## 🧪 Tests

4 tests `z1d_*` cubren la corrección de Blake2b y el estado reservado de scheme=3. Suite total: **685 passing** (681 → +4 Z1d).

## 🔗 Referencias

- RFC 7693 (Blake2) — Saarinen & Aumasson 2015.
- RFC 9106 (Argon2) — Biryukov, Dinu, Khovratovich, Josefsson 2021.
- ADR-0056 (Z1c): scrypt como KDF default actual; sigue activo en Z1d.
- ADR-0053 (Z1b): PBKDF2 como legacy support.
- ADR-0050 (Z1): foundation de identidad SQL-level sobre la que se construyen los KDFs.
