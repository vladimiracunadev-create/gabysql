# ADR-0060: Argon2id RFC 9106 estructural — vector match defer Z1f (Z1e)

**Estado**: ⚠️ Aceptada con limitación documentada
**Fecha**: 2026-05-29
**Bloque**: Z1e (estructura Argon2id; RFC vector match pendiente)
**Bump on-disk**: **VERSION 29 → 30**

## 🧭 Contexto

Z1d (ADR-0058) entregó la primitiva Blake2b validada contra RFC 7693 §A — foundation crypto sobre la cual Argon2id se construye. Z1e arrancó con la ambición de cerrar Argon2id completo: H' variable-length, G compression con BlaMka, memory matrix con indexing híbrido data-dependent/independent, pass loop, parallelism p>1, y match con el test vector oficial RFC 9106 §A.3.

**Honestidad técnica**: implementé la estructura completa (~450 LOC) pero el output **no matchea** el test vector RFC §A.3. El primer byte diverge (`0xa6` vs esperado `0x0a`), indicando un bug que afecta el output desde temprano — probablemente en el column phase del compress, el cálculo de W para el indexing, o el manejo de start_pos en pass > 0. Sin un reference implementation paso-a-paso para contrastar, debug ciego es impráctico.

**Decisión Z1e**: shippear la estructura como código `pub fn argon2id(...)` disponible para experimentación + debug, pero **NO** cambiar el default de password hashing (sigue siendo scrypt Z1c). El vector match queda como Z1f dedicado para debugging con una reference implementation a mano.

Esta partición es honesta:
- Z1d: Blake2b foundation validada (RFC 7693 §A match). ✅
- Z1e (este ADR): Argon2id estructura completa, determinismo verificado, RFC vector pendiente. ⚠️
- Z1f (futuro): debug RFC vector + cambio de default a scheme=3.

## 💡 Decisión

### 1. Implementación estructural completa

`pub fn argon2id(password, salt, secret, ad, m_kib, t, p, dk_len) -> Vec<u8>`:

- **H0 computation** (RFC §3.2.1): Blake2b-64 sobre la concatenación de params + inputs length-prefixed.
- **Initial blocks** B[i][0] y B[i][1] para cada lane via `argon2_h_prime(1024, H0 || LE32(0|1) || LE32(i))`.
- **Memory matrix** `Vec<Vec<[u8; 1024]>>` lane-major (p lanes × q blocks).
- **Pass loop** `for pass in 0..t { for slice in 0..4 { for lane in 0..lanes { ... } } }`.
- **Per-segment processing**: data-independent indexing en slice 0/1 del pass 0 (anti-side-channel), data-dependent en el resto. J1, J2 extraídos del prev block (data-dep) o de un address block generado on-the-fly (data-indep).
- **Reference index calculation** con start_pos / W según pass y slice.
- **Block update**: `B[lane][j] = G(prev, ref)` en pass 0, `XOR= G(prev, ref)` en passes > 0.
- **Final tag** `H'(dk_len, B[0][q-1] XOR B[1][q-1] XOR ... XOR B[p-1][q-1])`.

### 2. Funciones auxiliares

- `argon2_h_prime(t, data)`: RFC §3.2 variable-output Blake2b (recursive chain para t > 64).
- `argon2_gb(v, a, b, c, d)`: BlaMka G operation (Blake2 G + 2*low32(a)*low32(b)).
- `argon2_round(v)`: 8 BlaMka calls (column + diagonal pattern).
- `argon2_compress(x, y)`: G compression para 1024-byte blocks (row phase + column phase).
- `argon2_make_addr_block(...)`: address block generation para data-independent indexing.
- `argon2_process_segment(...)`: per-segment loop.

### 3. `PASSWORD_SCHEME_ARGON2ID = 3` sigue rechazado

`exec_set_session_auth` dispatch retorna `[GBY-4137]` con mensaje informativo:

```
SET SESSION AUTHORIZATION '...': scheme Argon2id (3) structural
implementation disponible en Z1e pero pendiente de matchear RFC 9106
§A.3 test vector — debug en Z1f. Default sigue siendo scrypt (Z1c).
```

`exec_create_user` y `exec_alter_user_password` siguen persistiendo con `scheme = PASSWORD_SCHEME_SCRYPT = 2`. **Sin regresión de seguridad**.

### 4. Bump VERSION 29 → 30

Aunque el on-disk format no cambia, bumpeamos para señalizar el corte: el binario Z1e tiene la struct Argon2id disponible (aunque no usada por default). Binarios pre-Z1e no la tienen.

## 📁 Archivos tocados

- `src/storage.rs`: bump `VERSION 29 → 30`.
- `src/sql.rs`:
  - Constantes nuevas: `ARGON2_M_KIB = 65536`, `ARGON2_T = 2`, `ARGON2_P = 1`.
  - ~450 LOC: `argon2_h_prime`, `argon2_gb`, `argon2_round`, `argon2_compress`, `argon2_make_addr_block`, `argon2_process_segment`, `pub fn argon2id`, `hash_password_argon2id`, `verify_password_argon2id`.
  - `hash_password_*` y `verify_password_*` marcados `#[allow(dead_code)]` (no se llaman desde el dispatch default).
- `tests/integration_test.rs`: 4 tests `z1e_*`:
  - `z1e_argon2id_rfc9106_test_vector_pending_z1f` — `#[ignore]` hasta que el vector matchee.
  - `z1e_argon2id_function_exists_and_is_deterministic` — verifica que `argon2id()` es deterministic + salt uniqueness.
  - `z1e_default_scheme_remains_scrypt` — confirma que el default NO cambió.
  - `z1e_argon2id_scheme_3_rejected_with_clear_message` — constant `PASSWORD_SCHEME_ARGON2ID = 3` expuesto.

## ⛔ Lo que **no** entra en Z1e (defer Z1f)

| Ítem | Razón del defer |
|---|---|
| **RFC 9106 §A.3 vector match** | Bug en algún sitio del pipeline (compress / indexing / start_pos / W). Requiere debug paso-a-paso contra una reference implementation. |
| Cambio de default a scheme=3 | Bloqueado por el vector match — no podemos shippear "Argon2id" sin matchear el RFC. |
| Argon2d / Argon2i variants | Sólo Argon2id en scope; los otros dos variants tienen indexing distinto. |
| Migración silenciosa scheme=2 → scheme=3 | Sin Argon2id production-ready no hay a dónde migrar. |
| Wire-up al server HTTP | Mismo defer histórico. |

## 🧪 Validación

- Suite: **693 passing + 1 ignored** (690 → +3 z1e_* + 1 ignored). `z1e_argon2id_function_exists_and_is_deterministic` verifica determinismo (h(p,s) = h(p,s)) y unicidad (h(p,s1) ≠ h(p,s2)). `z1e_default_scheme_remains_scrypt` confirma no regresión. `z1e_argon2id_rfc9106_test_vector_pending_z1f` está `#[ignore]` — se ejecuta con `cargo test -- --ignored` para validar Z1f cuando llegue.
- `cargo fmt --check` + `cargo clippy --lib --tests -- -D warnings` limpio.

## 🔗 Referencias

- RFC 9106 (Argon2) — Biryukov, Dinu, Khovratovich, Josefsson 2021.
- ADR-0058 (Z1d): Blake2b foundation.
- ADR-0056 (Z1c): scrypt como default actual.

## 📜 Reflexión

Z1e es un caso interesante de honestidad técnica vs ambición de scope. La opción "shippear Argon2id-inspired sin RFC match" sería deshonesta — usuarios esperarían interoperabilidad con otras implementaciones. La opción "no shippear nada" desperdicia el trabajo ya hecho. La opción elegida — **shippear estructura + flagear claramente la limitación + defer del vector match a Z1f** — preserva el progreso, mantiene la integridad del label "Argon2id", y deja una pista clara de qué falta.

El default de scrypt sigue cubriendo el objetivo memory-hard, así que la seguridad efectiva no se degrada.
