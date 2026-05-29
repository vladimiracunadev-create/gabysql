# ADR-0061: Argon2id `same_slice_extra` inversion fix (Z1f, parcial)

**Estado**: ⚠️ Aceptada con limitación documentada
**Fecha**: 2026-05-29
**Bloque**: Z1f (continuación del debug Argon2id)
**Bump on-disk**: **VERSION 30 → 31**

## 🧭 Contexto

Z1e (ADR-0060) shippeó la estructura completa de Argon2id pero el output no matcheaba RFC 9106 §A.3 (primer byte `0xa6` vs esperado `0x0a`). Z1f es el primer round de debug.

Tras re-leer RFC 9106 §3.4.1.2 vs libargon2 reference impl (`src/ref.c::index_alpha`), identifiqué un bug claro: la convención del **edge case adjustment** para cross-lane referencing.

## 💡 Bug identificado y corregido

### Convención libargon2 (correcta)

Per `index_alpha` en libargon2:
```c
if (same_lane) {
    reference_area_size = lane_length - segment_length + position->index - 1;
} else {
    reference_area_size = lane_length - segment_length +
                          ((position->index == 0) ? -1 : 0);
}
```

Para cross-lane reference:
- `position->index == 0` (j == slice_start): W = X − 1
- `position->index > 0`  (j > slice_start):  W = X

Donde X = `slice * sl` para pass 0, `3 * sl` para pass > 0.

### Bug Z1e (invertido)

Mi código tenía:
```rust
let same_slice_extra: u64 = if j == slice_start { 0 } else { 1 };
```

Que da:
- `j == slice_start`: edge_sub = 0 → W = X
- `j > slice_start`:  edge_sub = 1 → W = X - 1

**Exactamente lo opuesto** de la convención correcta.

### Fix Z1f

```rust
let edge_sub: u64 = if j == slice_start { 1 } else { 0 };
```

## 📊 Efecto del fix en el RFC vector

| Estado | Output (primer byte) | Output (32 bytes hex) |
|---|---|---|
| Esperado RFC §A.3 | `0a` | `0aa4c4248e30e06eff5ee38e71b1ffc7c789e87ea4336fafcda4a34dcb894da5` |
| Z1e (pre-fix) | `a6` | `a6d06b41464b6e79c1a2454cfb08644d922670fefe160615d69c5c6787ad0de8` |
| Z1f (post-fix) | `0d` | `0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659` |

El fix **cambia el output significativamente**, confirmando que afecta la indexación. El primer byte pasa de `0xa6` a `0x0d` (más cerca del esperado `0x0a` pero todavía distinto). **Bug(s) adicional(es) restantes**.

## ⛔ Lo que queda pendiente (Z1g)

Bugs candidatos remaining (no he logrado aislar cuál):
- Posible off-by-one en la inicialización del address block para pass 0 slice 0 (`starting_index = 2`). Para el test vector (sl=2), pass 0 slice 0 está vacío, así que no afecta este caso, pero podría afectar params más grandes.
- Posible discrepancia en el column phase del compress.
- Posible byte-ordering subtle.
- Posible bug en la propagación del XOR para pass > 0.

Z1g debe aislar el bug restante, probablemente añadiendo trazas paso-a-paso y comparando contra una reference impl Rust externa.

## 📁 Archivos tocados

- `src/storage.rs`: bump `VERSION 30 → 31`.
- `src/sql.rs`: inversión de la condición `same_slice_extra` → `edge_sub` en `argon2_process_segment` (~5 LOC de cambio, con comentario que documenta la convención libargon2).

## 🧪 Tests

- Suite: **693 passing + 1 ignored** (sin nuevos tests). El test `z1e_argon2id_rfc9106_test_vector_pending_z1f` sigue marcado `#[ignore]` hasta full RFC match.
- `cargo fmt --check` + `cargo clippy --lib --tests -- -D warnings` limpio.

## 🔗 Referencias

- libargon2 reference impl: github.com/P-H-C/phc-winner-argon2 (`src/ref.c::index_alpha`).
- ADR-0060 (Z1e): estructura inicial con el bug.
- RFC 9106 §3.4.1.2 — Indexing.
