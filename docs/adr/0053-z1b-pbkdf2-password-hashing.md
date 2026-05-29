# ADR-0053: PBKDF2-HMAC-SHA256 para password hashing (Z1b)

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-29
**Bloque**: Z1b (follow-up de Z1 — promoción del hash de password a crypto-grade)
**Bump on-disk**: **VERSION 25 → 26**

## 🧭 Contexto

Z1 (ADR-0050) entregó `CREATE USER ... WITH PASSWORD '...'` con un hash FNV-1a-64 + salt aleatorio. El propósito declarado era "bookkeeping SQL-level alineado con el estándar"; FNV no es crypto-grade y no resiste ataques de diccionario / GPU. El defer fue explícito: "para producción real necesitamos un KDF dedicado — Z1b".

Z1b cumple esa promesa: reemplaza FNV con **PBKDF2-HMAC-SHA256** implementado en Rust puro (sin deps externas), y además habilita la verificación real de password vía `SET SESSION AUTHORIZATION 'name' WITH PASSWORD '...'`.

## 💡 Decisión

### 1. PBKDF2-HMAC-SHA256 puro en Rust

Implementación in-tree de:
- **SHA-256** (RFC 6234 / FIPS 180-4): ~80 LOC, in-memory, suficiente para passwords < 1 KB.
- **HMAC-SHA256** (RFC 2104): ~25 LOC.
- **PBKDF2** (RFC 8018) con `dkLen = hLen = 32` → sólo T_1 (un bloque).

Parámetros estándar (constantes en `sql.rs`):

```rust
pub const PBKDF2_ITERATIONS: u32 = 100_000;   // OWASP 2023 PBKDF2-SHA256
pub const PASSWORD_SALT_LEN: usize = 16;      // NIST 800-132
pub const PASSWORD_HASH_LEN: usize = 32;      // = SHA-256 output
```

100K iteraciones tardan ~50-100ms en un CPU moderno — suficiente para inhibir ataques de diccionario online sin insultar al usuario en login.

### 2. UserMeta on-disk con `scheme` byte

```rust
pub struct UserMeta {
    pub name: String,
    pub scheme: u8,           // 1 = PBKDF2-SHA256 (Z1b en adelante)
    pub salt: Vec<u8>,        // longitud variable, length-prefixed u16
    pub password_hash: Vec<u8>,
    pub iterations: u32,
}
```

Layout: `[name][scheme:u8][salt_len:u16][salt][hash_len:u16][hash][iterations:u32]`.

El byte `scheme` reserva espacio para schemes futuros (argon2id, bcrypt) **sin bumpear VERSION**. Cuando Z1c llegue con argon2, sólo agrega `scheme = 2` y la verificación dispatchea.

### 3. Salt aleatorio reusando `gen_random_bytes` (Y9)

```rust
fn gen_password_salt_bytes() -> Vec<u8> {
    gen_random_bytes(PASSWORD_SALT_LEN)
}
```

El salt sigue siendo no-crypto-grade (xorshift64 seeded por nanos). Eso está bien — el salt sólo necesita ser **único** para que dos usuarios con la misma password no compartan hash. La resistencia del esquema viene de PBKDF2 + iteraciones, no de la imprevisibilidad del salt.

### 4. Verificación de password — `SET SESSION AUTHORIZATION ... WITH PASSWORD '...'`

Extensión del parser y exec de Z2:

```sql
SET SESSION AUTHORIZATION 'alice' WITH PASSWORD 'correct-horse-battery-staple';
-- Engine: hashea 'correct-horse-...' con el salt persistido de alice,
-- compara contra alice.password_hash con constant_time_eq.
-- Si matchea → current_user = Some("alice").
-- Si no → [GBY-4137] AUTH_PASSWORD_INCORRECT.
```

Sin `WITH PASSWORD '...'` el modo es **trust** — compat con Z2 (el server interno ya autenticó en otra capa, e.g. `-token` HTTP). Esto es importante: no rompemos ningún caller pre-Z1b.

**Constant-time compare**: `constant_time_eq` no cortocircuita en el primer mismatch, evitando timing attacks. (Aunque con 100K iter el timing es casi todo en PBKDF2; el constant-time compare es defensa en profundidad.)

### 5. Códigos de error

| Código | Nombre | Caso |
|---|---|---|
| 4137 | `AUTH_PASSWORD_INCORRECT` | `SET SESSION ... WITH PASSWORD '...'` falla la verificación PBKDF2 |

## 📁 Archivos tocados

- `src/storage.rs`: bump `VERSION 25 → 26`.
- `src/catalog.rs`: `UserMeta` rediseñado con `scheme`/`salt`/`password_hash`/`iterations`. Serialize/deserialize nuevos. Doc comments actualizados.
- `src/errors.rs`: código 4137.
- `src/sql.rs`:
  - Helpers nuevos: `sha256()`, `hmac_sha256()`, `pbkdf2_sha256()`, `hash_password_pbkdf2()`, `verify_password_pbkdf2()`, `constant_time_eq()`, `gen_password_salt_bytes()`.
  - Constantes: `PASSWORD_SCHEME_PBKDF2_SHA256 = 1`, `PBKDF2_ITERATIONS = 100_000`, `PASSWORD_SALT_LEN = 16`, `PASSWORD_HASH_LEN = 32`.
  - `exec_create_user` y `exec_alter_user_password` ahora usan `hash_password_pbkdf2` + `gen_password_salt_bytes`.
  - `SetSessionAuthStmt` extendido con `password: Option<String>`.
  - Parser: `WITH PASSWORD '...'` opcional tras user name.
  - `exec_set_session_auth` verifica PBKDF2 si el caller pasó password.
- `tests/integration_test.rs`: 8 tests `z1b_*`. Actualización de `z1_create_user_persists_and_drops` para comparar contra `Vec<u8>` en vez de `u64`.

## ⛔ Lo que **no** entra en Z1b (defer)

| Ítem | Razón del defer |
|---|---|
| **Argon2id** o **bcrypt** | Más resistentes a ASIC que PBKDF2 (argon2 es memory-hard). Implementación ~1000+ LOC pura. Defer a Z1c, ya con el `scheme` byte preparado. |
| Iteraciones configurables (`ALTER SYSTEM SET pbkdf2_iterations = ...`) | Hardcoded 100K. Defer hasta que aparezca un caso de uso real. |
| Migración automática de usuarios scheme=0 (FNV legacy) | Z1b bumpa VERSION, así que un .db V23/V24/V25 no se abre con código Z1b. No hay migración silenciosa. Si necesitamos retro-compat, agregar lectura de UserMeta v1 + re-hash on next login. |
| Verificación de password contra hash desde el servidor HTTP | Hoy el token compartido sigue gobernando el HTTP. Conectar `Authorization: Bearer <user>:<password>` con `verify_password_pbkdf2` queda como bloque server-side futuro. |
| `pg_authid`-style catálogo introspectable | Una tabla virtual `gabysql_users` con `(name, has_password, iterations, scheme_name)`. Útil para debugging; defer. |

## 🧪 Tests

8 tests `z1b_*`:
- `z1b_create_user_persists_pbkdf2_meta` — scheme=1, salt=16B, hash=32B, iterations=100K.
- `z1b_same_password_different_salt` — uniqueness del salt.
- `z1b_set_session_auth_correct_password` — happy path.
- `z1b_set_session_auth_wrong_password_errors` — 4137.
- `z1b_set_session_auth_without_password_still_works` — modo trust (compat Z2).
- `z1b_alter_user_password_then_auth_with_new` — old password queda inválida tras ALTER.
- `z1b_pbkdf2_deterministic_same_salt` — verificación determinística (dos SET con misma password pasan).
- `z1b_empty_password_creates_user_but_blocks_auth` — `CREATE USER alice` sin password produce un hash que no matchea ninguna password explícita.

Suite total: **651 passing** (643 → +8 Z1b).

Tiempos: el suite ahora tarda ~52s (vs 18s pre-Z1b) por los ~20 hashes PBKDF2 que disparan los tests Z1/Z1b combinados (cada uno ~50ms). Compromiso aceptable.

## 🔗 Referencias

- RFC 6234 (SHA-256), RFC 2104 (HMAC), RFC 8018 (PBKDF2).
- OWASP Password Storage Cheat Sheet 2023 — PBKDF2-SHA256 con ≥ 100K iter.
- NIST SP 800-132 — Salt de 128 bits.
- ADR-0050 (Z1): foundation que este ADR promociona a crypto-grade.
- ADR-0051 (Z2): el `SET SESSION AUTHORIZATION` se extiende acá con `WITH PASSWORD`.
