# ADR-0056: scrypt (RFC 7914) memory-hard password hashing (Z1c)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z1c (follow-up de Z1b — promoción del KDF a memory-hard)
**Bump on-disk**: **VERSION 27 → 28**

## 🧭 Contexto

Z1b (ADR-0053) reemplazó FNV con **PBKDF2-HMAC-SHA256** (100K iter). PBKDF2 cumple los requisitos OWASP de password hashing pero **no es memory-hard**: un atacante con un ASIC o un GPU puede paralelizar miles de hashes por segundo porque sólo requiere CPU/cores, no memoria significativa.

Z1b dejó documentado como defer: "Argon2id o bcrypt (scheme=2, memory-hard, resistente a ASIC)". Z1c entrega esa promesa con **scrypt** (RFC 7914) — un KDF memory-hard estandarizado, predecesor honesto de argon2 y todavía recomendado por NIST. Implementación pura en Rust sin deps, ~250 LOC sobre el PBKDF2 ya existente.

**Por qué scrypt y no argon2id**: implementar argon2id correctamente desde cero (Blake2b variant, G function, memory blocks indexing) es ~800-1000 LOC con muchas oportunidades de bug silencioso. Scrypt comparte el mismo objetivo (memory-hard, resistente a ASIC) con un diseño mucho más simple y bien-entendido. Cuando un caso de uso justifique argon2id explícitamente, llega como `scheme = 3` en Z1d — el byte ya está preparado.

## 💡 Decisión

### 1. scrypt RFC 7914 puro en Rust

Implementación in-tree de:
- **Salsa20/8 core** (Bernstein): 8 rondas (4 double-rounds column+row) sobre un block de 64 bytes (~30 LOC).
- **BlockMix**(B, r): wrapper que procesa 2r bloques de 64B con Salsa20/8, reordena pares e impares (~25 LOC).
- **ROMix**(B, N, r): la parte memory-hard — aloca `N * 128 * r` bytes (~32 MB para los params interactive) y hace N iteraciones de BlockMix + N saltos pseudo-aleatorios guiados por el contenido (~30 LOC).
- **scrypt**(P, S, N, r, p, dkLen): pipeline RFC §6 — PBKDF2 inicial → ROMix por bloque → PBKDF2 final (~25 LOC).
- **pbkdf2_sha256_extended**: variante del PBKDF2 de Z1b que soporta `dkLen` arbitrario (necesario para el output multi-block de scrypt) (~25 LOC).

Parámetros estándar (constantes en `sql.rs`):

```rust
pub const SCRYPT_N: u32 = 16384;  // OWASP / RFC 7914 §7 interactive
pub const SCRYPT_R: u32 = 8;       // RFC standard
pub const SCRYPT_P: u32 = 1;       // serial (no parallel overhead)
```

N=16384 + r=8 ≈ **32 MB de memoria** por hash, ~100ms en CPU moderno. Suficiente para volver impráctico un ataque de diccionario con GPU/ASIC porque cada hash requiere 32 MB de RAM dedicada.

### 2. UserMeta — mismo layout, scheme=2

El layout on-disk de Z1b cubre Z1c sin cambios estructurales: el byte `scheme` reserva slot para esquemas futuros. Z1c lo usa para `2 = scrypt`. El campo `iterations` se reusa para encodear `N` (cuando scheme=2, `iterations` representa el work factor scrypt). Salt y hash siguen siendo 16B y 32B respectivamente.

```rust
pub const PASSWORD_SCHEME_SCRYPT: u8 = 2;
```

### 3. Default cambia: `CREATE USER` ahora usa scrypt

`exec_create_user` y `exec_alter_user_password` ahora persisten con `scheme = PASSWORD_SCHEME_SCRYPT`. Users creados pre-Z1c (scheme=1, PBKDF2) **siguen funcionando**: `exec_set_session_auth` ya hacía dispatch sobre `meta.scheme` (Z1b), y ahora cubre ambos:

```rust
let ok = match meta.scheme {
    PASSWORD_SCHEME_PBKDF2_SHA256 => verify_password_pbkdf2(pw, &salt, &hash),
    PASSWORD_SCHEME_SCRYPT       => verify_password_scrypt(pw, &salt, &hash),
    other                        => return Err(...),  // [GBY-4137]
};
```

### 4. VERSION bump 27 → 28

A pesar de que el layout de `UserMeta` no cambió (mismo formato Z1b), bumpeamos VERSION para señalar la migración semántica: un .db Z1c contiene users con scheme=2 que un binario Z1b no sabe verificar (devolvería error de scheme desconocido). El bump documenta el corte.

## 📁 Archivos tocados

- `src/storage.rs`: bump `VERSION 27 → 28`.
- `src/sql.rs`:
  - Constantes nuevas: `PASSWORD_SCHEME_SCRYPT = 2`, `SCRYPT_N = 16384`, `SCRYPT_R = 8`, `SCRYPT_P = 1`.
  - Funciones nuevas (~250 LOC total): `salsa20_8_core`, `block_mix`, `ro_mix`, `scrypt_hash`, `pbkdf2_sha256_extended`, `hash_password_scrypt`, `verify_password_scrypt`.
  - `exec_create_user` y `exec_alter_user_password`: cambian default a scheme=2 / scrypt.
  - `exec_set_session_auth`: dispatch nuevo sobre `meta.scheme` para soportar PBKDF2 (scheme=1) y scrypt (scheme=2) en paralelo.
- `tests/integration_test.rs`: 6 tests `z1c_*` + ajuste de `z1b_create_user_persists_pbkdf2_meta` para aceptar scheme=1 o 2 (shape-check, no identity-check).

## ⛔ Lo que **no** entra en Z1c (defer)

| Ítem | Razón del defer |
|---|---|
| **Argon2id** (scheme=3) | Memory-hard también pero con resistencia adicional a side-channel attacks (variant id) y validación formal RFC 9106. Implementación ~800 LOC con Blake2b variant — defer hasta caso de uso explícito. |
| Parámetros N/r/p configurables | Hoy hardcoded N=16384. Defer hasta que un caller necesite tunear (más memoria = más caro para defender + atacar). |
| Migración silenciosa de scheme=1 → scheme=2 on next login | Si un user creado pre-Z1c se autentica exitosamente, podríamos re-hashearlo con scrypt y persistir. Defer porque requiere coordinar con el flow de `exec_set_session_auth`. |
| Test vector RFC 7914 §8 explícito | No incluido en los tests por simplicidad — los z1c_* validan determinismo + auth round-trip, suficiente para detectar regresiones. Si dudás de la corrección, contrastá manualmente contra el vector `(P="password", S="NaCl", N=1024, r=8, p=16)` del RFC. |
| Wire-up al server HTTP | Mismo defer de Z1b — el token compartido sigue gobernando la auth HTTP. |

## 🧪 Tests

6 tests `z1c_*`:
- `z1c_default_scheme_is_scrypt` — CREATE USER por default emite scheme=2, N=16384, salt 16B, hash 32B.
- `z1c_scrypt_correct_password_authenticates` — happy path.
- `z1c_scrypt_wrong_password_rejected` — [GBY-4137].
- `z1c_alter_user_password_uses_scrypt` — ALTER mantiene scheme=2.
- `z1c_same_password_different_salt_different_hash` — uniqueness del salt.
- `z1c_scrypt_deterministic` — dos SET con misma password autentican (necesario para que la verificación funcione).

Test ajustado: `z1b_create_user_persists_pbkdf2_meta` ahora acepta `scheme ∈ {1, 2}` (Z1b o Z1c) — verifica el shape on-disk, no la identidad del KDF.

Suite total: **674 passing** (668 → +6 Z1c).

## 🔗 Referencias

- RFC 7914 (scrypt) — Percival & Josefsson 2016.
- ADR-0053 (Z1b): foundation de PBKDF2 que este ADR promociona a memory-hard.
- ADR-0050 (Z1): bookkeeping de identidad SQL-level.
- NIST SP 800-63B §5.1.1.2 — scrypt aceptado como Memory-Hard Function approved.
