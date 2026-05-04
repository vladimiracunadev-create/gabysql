# ADR-0004: B+Tree con `root_page` estable vía copy-up

**Estado**: ✅ Aceptada
**Fecha**: 2026-05-03
**Contexto**: hallazgo crítico #1 del MVP. Reemplazo de la "lista enlazada de hojas" por un B+Tree real con nodos internos.

## 🧭 Contexto

La implementación original llamada `bptree.rs` no era un B+Tree: era una lista enlazada de páginas LEAF, donde `find_leaf` recorría linealmente buscando la hoja con la PK correspondiente. Eso es **O(N páginas) por lookup**, lo que invalida el motor para tablas medianas.

Al introducir nodos internos surge un problema: la `root_page` de cada tabla está persistida en `TableMeta` dentro del catálogo. Si el root necesita splittear (porque se llenó), un B+Tree clásico promueve el root a un nivel superior — y el número de página del root **cambia**. Eso obligaría a reescribir el catálogo en cada split del root, propagando un costo en cascada.

## 💡 Decisión

Mantener el **número de página del root estable**, sin importar cuántos splits ocurran, mediante la técnica **copy-up**:

1. Cuando el root necesita splittear, su contenido **se copia a una página nueva** allocada por el `Pager`.
2. La página original (con su `root_page` original) se **reescribe como `INTERNAL`** con dos hijos: el copy-up + la mitad derecha del split.
3. El catálogo nunca aprende de los splits — la `root_page` que tenía sigue siendo válida.

Esto se aplica recursivamente cuando un internal padre se llena.

## 🔄 Alternativas consideradas

- **Cambiar el `root_page` y reescribir el catálogo**: rechazado — cada split del root forzaría un write transaccional en el catálogo, encadenando posibles splits en su propio B+Tree.
- **B-Link tree (concurrente)**: rechazado — overkill para el modelo actual de un solo escritor.
- **Mantener leaf-only**: rechazado — derrota el objetivo del refactor.

## 📊 Consecuencias

**Positivas**:
- Lookups en O(log N) reales.
- El catálogo se mantiene simple y no necesita actualizarse en splits estructurales.
- El mismo patrón de root estable se reutiliza para los índices secundarios (cada índice es un B+Tree propio con su `root_page` fijo).

**Negativas**:
- Una página extra allocada por root-split (la copia del root previo). Aceptable: los splits del root son raros en cargas reales.
- El código del `bptree.rs` es más complejo que el de la lista enlazada: hay que distinguir LEAF e INTERNAL en cada operación.

**Neutras**:
- La estructura no implementa merge / rebalance al borrar — las páginas pueden quedar parcialmente vacías. Aceptable hoy; reclamación de espacio queda para una futura herramienta `vacuum`.

## 🔗 Referencias

- Commit: `e97d2cc`.
- Implementación: [src/bptree.rs](../../src/bptree.rs) — funciones `put_recursive`, `split_root`, `find_leaf`, `leftmost_leaf`.
- Test: [tests/integration_test.rs::btree_splits_leaves_and_promotes_internal_root](../../tests/integration_test.rs) — 600 filas que fuerzan splits + lookup en pk lejano.
- CHANGELOG: entrada 2026-05-03.
