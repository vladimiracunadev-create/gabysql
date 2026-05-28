# 🧩 Matriz de Compatibilidad

> **Qué entornos están probados, soportados o experimentales para `gabysql` hoy.**

---

## 1. 🖥️ Modos de ejecución

| Modo | Soporte | Notas |
| :--- | :--- | :--- |
| **CLI nativo** (`gabysql`) | 🟢 Primario | `init / info / exec / repl` sobre archivo `.db` local. |
| **API server** (`gabysql-server`) | 🟢 Primario | HTTP/JSON, single DB o multi DB, token opcional, cap de conexiones (default 64). |
| **Docker single-image** | 🟢 Primario | Imagen multi-stage `rust:1.94-bookworm` → `debian:bookworm-slim`. |
| **`docker compose`** (server + phpgabyadmin) | 🟢 Primario | Stack completo levantado por CI. |
| **Embedded (lib `gabysql`)** | 🟡 Soportado | Crate con `[lib]` exportado; sin garantías de API estable hasta `0.2`. |
| **Wire protocol Postgres/MySQL** | 🔴 No soportado | Fuera de alcance hasta fase 4 del ROADMAP. |

## 2. ⚙️ Toolchain Rust

| Versión Rust | Estado |
| :--- | :--- |
| `1.94 stable` | 🟢 Probado en CI (Ubuntu / Windows / macOS) y en imagen Docker. |
| `1.95 stable` | 🟢 Verificado: el repo pasa `cargo fmt`, `clippy --all-targets -- -D warnings` y `cargo test` con esa versión. |
| `nightly` | 🟡 No es target oficial; debería compilar pero no hay garantía. |
| `< 1.94` | 🔴 No soportado (uso de `let-else`, `OnceLock`, expresiones modernas en clippy). |

## 3. 🪟 Sistemas operativos host

| OS | Soporte |
| :--- | :--- |
| **Ubuntu (LTS reciente)** | 🟢 Nativo + CI multi-versión. |
| **Windows 10/11** | 🟢 Probado en CI Windows-latest. Build con `cargo` requiere Visual Studio Build Tools + Windows SDK (ver [INSTALL.md](INSTALL.md)). |
| **macOS (Intel)** | 🟢 Probado en CI macos-latest. |
| **macOS (Apple Silicon)** | 🟢 Probado en CI macos-latest (que ya corre arm64). |
| **WSL2** | 🟢 Esperado funcionar idéntico a Linux nativo. |

## 4. 🐳 Docker / contenedores

| Componente | Versión / nota |
| :--- | :--- |
| Imagen base de build | `rust:1.94-bookworm` |
| Imagen base runtime | `debian:bookworm-slim` |
| Docker Engine | `24.0+` recomendado |
| Docker Compose | `v2+` (sintaxis del archivo: implícita por `docker-compose.yml` sin `version:` deprecada) |
| PHP en `phpgabyadmin` | `php:8.2-apache` |

## 5. 💾 Formato en disco

| `VERSION` del header | Estado | Notas |
| :--- | :--- | :--- |
| `13` | 🟢 Actual | Bloque V (2026-05-27). Catalog gana un **discriminator byte por record** (table vs view) y persiste `ViewMeta { name, source_sql, column_aliases }`. Habilita `CREATE VIEW [IF NOT EXISTS] v [(col_aliases)] AS SELECT ...` / `DROP VIEW [IF EXISTS] v` (ADR-0025). Rechaza VERSION 12 con `[GBY-1003]`. |
| `12` | 🔴 Rechazado | Residual #3 (2026-05-27). `ForeignKeyMeta` gana `extra_source_columns` + `extra_target_columns` para FK multi-col `FOREIGN KEY (a, b) REFERENCES p (x, y)`; lookup O(log n) via fingerprint K2 (ADR-0023). Migrar via backup + dump + recreate. |
| `11` | 🔴 Rechazado | Residual #2 (2026-05-27). Persiste nombres opcionales para PK/UNIQUE/FK/CHECK (`pk_name`, `fk_name`, etc.) y habilita `ALTER TABLE DROP CONSTRAINT [IF EXISTS] <name>` (ADR-0022). Migrar via backup + dump + recreate. |
| `10` | 🔴 Rechazado | Bloque L2 (2026-05-27). Agregaba `CHECK (expr)` column-level y table-level, persistido como texto canónico vía `format_expr` (ADR-0021). Migrar via backup + dump + recreate. |
| `9` | 🔴 Rechazado | Bloque L1 (2026-05-27). Extendía `ForeignKeyMeta` con `on_delete` ampliado (`SET NULL` / `SET DEFAULT` / `NO ACTION`) y nuevo campo `on_update` con las cinco acciones referenciales (ADR-0020). Migrar via backup + dump + recreate. |
| `8` | 🔴 Rechazado | Extendía `TableMeta.primary_key` y `IndexMeta.column` a múltiples columnas: `PRIMARY KEY (a, b, ...)` table-level y `CREATE [UNIQUE] INDEX idx ON t (a, b, ...)` (K2, all-INT NOT NULL, equality-only via fingerprint FNV-1a-64, ADR-0019). |
| `7` | 🔴 Rechazado | Agregaba `kind: IndexKind` (`Hash` \| `OrderedInt`) a `IndexMeta`: índices sobre columnas `INT` ordenados con `BETWEEN` por índice (ADR-0017). Migrar via backup + dump + recreate. |
| `6` | 🔴 Rechazado | Agregaba `FOREIGN KEY` por columna (target table + column + ON DELETE RESTRICT/CASCADE). Recrear DB con binario actual. |
| `5` | 🔴 Rechazado | Agregaba `NOT NULL` + `DEFAULT` por columna y `unique` por índice. Recrear DB con binario actual. |
| `4` | 🔴 Rechazado | Sin constraints declarativas. Recrear DB. |
| `3` | 🔴 Rechazado | Sin índices secundarios. Recrear DB. |
| `2` | 🔴 Rechazado | Sin CRC. Recrear DB. |
| `1` | 🔴 Rechazado | Hash `DefaultHasher` no estable. Recrear DB. |

> Cada bump de `VERSION` se publica con changelog explícito. No hay migración automática en esta etapa.

## 6. 🌐 Navegadores para `phpgabyadmin` y `gabymodeler`

Ambas UIs son HTML + CSS + JS vanilla (sin frameworks ni npm). Soportado:

- Chrome / Edge (Chromium) `100+`
- Firefox `100+`
- Safari `15+`

No se prueba contra IE11 ni navegadores legacy.

| Cliente | Necesita PHP | Persistencia | Ejecuta SQL contra el motor |
| :--- | :---: | :--- | :---: |
| `phpgabyadmin` | sí (8.2+) | en el server gabysql | ✅ vía `/exec` |
| `gabymodeler` | no (HTML estático) | `localStorage` del browser | ❌ produce DDL para pegar en phpgabyadmin |

## 7. 📡 Drivers / clientes

| Cliente | Soporte |
| :--- | :--- |
| HTTP/JSON (cualquier lenguaje con `curl`/`fetch`) | 🟢 Documentado en [docs/API.md](docs/API.md). |
| Ejemplos PHP / Python | 🟢 En [examples/](examples). |
| Driver oficial Go / Java / Node / Rust como crate | 🔴 No publicado todavía. |

## 8. 🧠 Cache de páginas (Pager)

| Parámetro | Default | Cómo se ajusta | Notas |
| :--- | :--- | :--- | :--- |
| `cache_capacity` (páginas) | `1024` (`DEFAULT_CACHE_PAGES`) | `Pager::set_cache_capacity(n)` runtime | A 4 KB/página = ~4 MB por DB. Política: LRU sobre páginas clean; las dirty nunca se evictan. Ver [ADR-0009](docs/adr/0009-page-cache-lru-bounded.md). |
| `cache_len()` | — | introspección | Tamaño actual; nunca debería superar `cache_capacity` salvo overflow temporal mid-tx con muchas writes pendientes. |

> Para servers long-running con N DBs activas, la memoria del cache total es aproximadamente `N × cache_capacity × page_size`. Con defaults: 50 DBs × 1024 páginas × 4 KB ≈ **200 MB**. Predecible y acotado, vs el comportamiento pre-ADR-0009 que crecía sin freno.

---

## 9. ⚠️ Restricciones conocidas

- El servidor no expone TLS nativo. Para producción se requiere un reverse proxy con TLS.
- `cargo audit` y `cargo deny` corren en CI (workflow `security.yml`); el grafo de dependencias hoy es vacío, pero la barrera está activa para el día que se introduzcan crates.
- Las DBs creadas con versiones anteriores del formato no son legibles — ver [TROUBLESHOOTING.md](TROUBLESHOOTING.md#-unsupported-gabysql-file-format-versionn-expected-8) (sección reescrita en cada bump).
