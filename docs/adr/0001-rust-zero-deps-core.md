# ADR-0001: Rust con cero dependencias externas para el core

**Estado**: ✅ Aceptada
**Fecha**: 2026-03-19 (decisión original) · revisada 2026-05-04
**Contexto**: rewrite del producto desde Go a Rust.

## 🧭 Contexto

El motor anterior estaba en Go y tenía la superficie típica de un proyecto Go: docenas de dependencias transitivas, varios `replace` en `go.mod`, y una dificultad real para auditar qué entraba al binario final. Para un producto que pretende ser una BD seria (datos de usuarios) y al mismo tiempo crecer comercialmente, ese nivel de opacidad de supply-chain era inviable.

## 💡 Decisión

El core de `gabysql` se escribe en **Rust** y `Cargo.toml` declara **cero dependencias externas**. Todo se construye con `std`. La excepción es la imagen Docker base (Debian) y el toolchain (Rust). Cualquier dependencia futura requiere una ADR explícita justificándola.

## 🔄 Alternativas consideradas

- **Mantener Go** con disciplina de auditoría: rechazado, el ecosistema Go normaliza la inflación de deps.
- **Zig**: rechazado por madurez del ecosistema y curva de aprendizaje del equipo.
- **C++**: rechazado por memory safety; el competidor (SQLite) ya está en C, sin diferenciador.
- **Rust con dependencias permitidas**: rechazado, perderíamos el diferenciador de "auditable byte por byte".

## 📊 Consecuencias

**Positivas**:
- Diferenciador frente a SQLite, libSQL, DuckDB y todas las BDs que enlazan C.
- `cargo audit` y `cargo deny` empiezan con grafo vacío — la primera dep que se introduzca llamará la atención inmediatamente.
- Binario release ~3-4 MB sin trade-offs de tamaño.
- Hot paths del motor (CRC32, FNV-1a, B+Tree, parser SQL) son legibles y debuggeables sin ir a un crate externo.

**Negativas**:
- Cualquier feature que tenga un crate maduro (TLS, JSON parsing, cron parsing) requerirá implementación propia o un ADR de excepción.
- Curva de mantenimiento mayor: cuando hay un bug en CRC32 IEEE, no se actualiza una crate, se corrige aquí.
- Zero deps fuerza decisiones explícitas sobre qué entra al producto y qué se queda fuera (consistente con [docs/POSITIONING.md](../POSITIONING.md)).

**Neutras**:
- TLS nativo en `gabysql-server` queda fuera del MVP; se delega a un reverse proxy (`nginx`/`caddy`) hasta que el [Camino B](../COMMERCIAL_ROADMAP.md) justifique introducir `rustls` con su propio ADR.

## 🔗 Referencias

- Implementación: todo el repo, pero el header del [Cargo.toml](../../Cargo.toml) lo hace explícito.
- Verificación: workflow `security.yml :: cargo_deny` corre `check sources` con allowlist solo de `crates.io-index`.
- Postura: [docs/SECURITY_LAYERS.md §3](../SECURITY_LAYERS.md).
