# 📐 Architecture Decision Records (ADRs)

> **Decisiones técnicas significativas de `gabysql`** — qué se eligió, por qué, qué se descartó y cuáles son las consecuencias.

Las ADRs se numeran secuencialmente. Una decisión revertida no se borra: se marca como **Superseded by ADR-NNNN** y se mantiene en el repo para preservar trazabilidad.

| # | ADR | Estado | Tema |
| :---: | :--- | :---: | :--- |
| 0001 | [Rust con cero dependencias externas para el core](0001-rust-zero-deps-core.md) | ✅ Aceptada | Lenguaje y supply-chain |
| 0002 | [FNV-1a-64 fijado en código en lugar de `DefaultHasher`](0002-fnv1a-stable-hash.md) | ✅ Aceptada | Estabilidad del formato en disco |
| 0003 | [Trailer CRC32 por página + verificación en lectura y replay](0003-crc32-page-trailer.md) | ✅ Aceptada | Integridad de datos |
| 0004 | [B+Tree con `root_page` estable vía copy-up](0004-bptree-root-stable.md) | ✅ Aceptada | Estructura del índice |
| 0005 | [Bucket layout para índices secundarios + hash collision tolerante](0005-secondary-index-bucket.md) | ✅ Aceptada | Índices secundarios |
| 0006 | [`grype --only-fixed` en lugar de `--fail-on critical`](0006-grype-only-fixed.md) | ✅ Aceptada | Política de container scan |
| 0007 | [Camino A (embebido nicho) antes que B/C](0007-commercial-path-a.md) | ✅ Aceptada | Estrategia de producto |
| 0008 | [`LeafCursor` (Iterator pattern) para SELECT lazy](0008-leaf-cursor-iterator.md) | ✅ Aceptada | Optimización de recursos en lectura |
| 0009 | [`PageCache` con capacidad fija + LRU sobre páginas clean](0009-page-cache-lru-bounded.md) | ✅ Aceptada | Memoria del Pager acotada |
| 0010 | [Gateway MCP como adaptador externo sobre el HTTP/JSON existente](0010-mcp-gateway.md) | ✅ Aceptada | Modo agente / AI-native sin tocar el core |
| 0011 | [Búsqueda vectorial del lado del gateway, no en el motor](0011-vector-search-gateway-side.md) | ✅ Aceptada | Vectores TEXT + top-k en el gateway; cero bump de formato |
| 0012 | [Audit log enriquecido en el gateway, no en el motor](0012-audit-log-enriquecido.md) | ✅ Aceptada | JSONL opt-in con clientInfo + reason semántico |

---

## 📝 Plantilla para una nueva ADR

```markdown
# ADR-NNNN: <decisión en una línea>

**Estado**: 🟡 Propuesta · ✅ Aceptada · ❌ Rechazada · 🗑️ Superseded by ADR-NNNN
**Fecha**: YYYY-MM-DD
**Contexto que la motiva**: link a Issue / commit / sección del CHANGELOG

## 🧭 Contexto

Qué problema motivó esta decisión, qué restricciones había, qué hipótesis se asumieron.

## 💡 Decisión

Qué se decidió hacer, en pocas palabras pero sin ambigüedad.

## 🔄 Alternativas consideradas

Lista de opciones que se evaluaron, con pros y contras.

## 📊 Consecuencias

- **Positivas**: qué se gana.
- **Negativas**: qué se sacrifica.
- **Neutras**: qué cambia sin ser claramente bueno o malo.

## 🔗 Referencias

Issues, RFCs externos, papers, prior art en otros motores.
```

---

## 🧪 Cómo se aplica una ADR en el código

Cada ADR debe poder rastrearse a:
- **el commit** que la implementó (link al SHA en GitHub),
- **el archivo principal** donde vive la implementación,
- **la entrada del CHANGELOG** que la anuncia al usuario,
- **los tests** que la cubren.

Ejemplo: ADR-0003 (CRC32) → commit `f0cb771` → [src/storage.rs](../../src/storage.rs) (`finalize_page_checksum`/`verify_page_checksum`) → [CHANGELOG entry 2026-05-03](../../CHANGELOG.md) → [tests/integration_test.rs](../../tests/integration_test.rs) (`page_checksum_detects_corruption`).

Esa cadena tiene que existir para que la ADR no sea solo aspiracional.
