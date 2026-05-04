# ADR-0002: FNV-1a-64 fijado en código en lugar de `DefaultHasher`

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-03
**Contexto**: hallazgo crítico #2 del MVP. Bump VERSION 1 → 2.

## 🧭 Contexto

El catálogo de tablas direcciona cada `TableMeta` por una clave `i64` derivada del nombre normalizado de la tabla. Esa clave queda **persistida** en el archivo `.db`. La implementación original usaba `std::collections::hash_map::DefaultHasher`, cuya documentación oficial declara explícitamente:

> *"Hashes computed by DefaultHasher are not portable across processes (...). The hash algorithm of DefaultHasher is not specified and is subject to change at any time."*

Es decir: un upgrade del toolchain de Rust podía cambiar el algoritmo, dejando todas las DBs existentes con claves recalculadas distintas a las almacenadas — silenciosamente "perdiendo" tablas.

## 💡 Decisión

Reemplazar `DefaultHasher` por **FNV-1a-64 implementado inline en el repo** (`src/catalog.rs::hash_name` y `src/index.rs::hash_value`). El algoritmo está pinneado byte por byte en código fuente, con sus constantes (`FNV_OFFSET_BASIS`, `FNV_PRIME`) fijas. Cualquier cambio futuro requeriría una nueva ADR y un bump de `VERSION`.

## 🔄 Alternativas consideradas

- **SipHash con seed fijo**: opción razonable, pero `SipHasher` está deprecated en `std`; usar uno externo viola [ADR-0001](0001-rust-zero-deps-core.md).
- **Mantener DefaultHasher pero documentar el riesgo**: rechazado — el riesgo es silencioso, no detectable por el usuario hasta que ya perdió datos.
- **xxHash**: más rápido pero requiere implementación propia más compleja; FNV-1a ofrece el balance correcto entre simplicidad, velocidad aceptable y determinismo.
- **CityHash / Murmur3**: similar — más complejos, sin ganancia clara sobre FNV-1a para este caso de uso (claves cortas, no hot path).

## 📊 Consecuencias

**Positivas**:
- Las claves del catálogo y los buckets de índices secundarios ahora son **completamente deterministas** entre versiones de Rust, plataformas y arquitecturas.
- El algoritmo cabe en ~10 líneas legibles, sin tabla precomputada compleja.
- Mismo algoritmo se reusa en [ADR-0005](0005-secondary-index-bucket.md) para hashing de valores indexados.

**Negativas**:
- Un bump del formato (VERSION 1 → 2) que **rechaza explícitamente** las DBs anteriores. No hay migración automática.
- FNV-1a no es un hash criptográfico ni resistente a colisiones adversariales; pero el caso de uso (catálogo interno) no lo requiere.

**Neutras**:
- El catálogo sigue rechazando explícitamente colisiones de hash (verifica `meta.name == requested_name` tras decode).

## 🔗 Referencias

- Commit: `7c32171`.
- Implementación: [src/catalog.rs:hash_name](../../src/catalog.rs).
- Test: implícito — todos los tests fallarían si el hash cambiara entre runs.
- CHANGELOG: entrada 2026-05-03.
