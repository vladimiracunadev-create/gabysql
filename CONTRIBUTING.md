# 🤝 CONTRIBUTING

> **Cómo contribuir sin romper estabilidad, storage ni coherencia documental.**

---

## 🧭 Principios del repositorio

- No prometer en docs lo que el motor todavía no soporta
- Si cambias storage, parser o semántica SQL, agrega o ajusta pruebas
- Si cambias comportamiento visible, actualiza también la documentación
- Prefiere cambios pequeños, verificables y reversibles

---

## 🔄 Flujo recomendado

1. Entender el comportamiento actual
2. Implementar el cambio en el módulo correspondiente
3. Ejecutar validaciones locales
4. Actualizar documentación afectada
5. Revisar límites, compatibilidad y riesgos

---

## ✅ Validaciones obligatorias

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
php -l web/index.php
php -l web/phpgabyadmin/index.php
```

Si tu entorno Windows todavía no tiene toolchain nativo completo, valida al menos:
```powershell
docker build -t gabysql .
```

### Validaciones de seguridad (recomendadas localmente, obligatorias en CI)

```powershell
cargo install cargo-audit --version 0.22.1 --locked
cargo install cargo-deny  --version 0.19.4 --locked
cargo audit
cargo deny --all-features check
```

CI corre además `detect-secrets`, Trojan Source, `grype` (container scan), `actionlint`, `zizmor` y `pin-check` sobre los workflows. Ver [docs/SECURITY_LAYERS.md](docs/SECURITY_LAYERS.md) para el mapa completo.

---

## 🗺️ Dónde tocar cada cosa

| Área | Archivo principal |
|---|---|
| Pager, header, WAL, CRC y recovery | `src/storage.rs` |
| B+Tree (LEAF + INTERNAL, splits, root estable) | `src/bptree.rs` |
| Catálogo de tablas + `IndexMeta` | `src/catalog.rs` |
| Helpers de índices secundarios (hash, codec de bucket) | `src/index.rs` |
| SQL parser, engine y row codec | `src/sql.rs` |
| API HTTP/JSON | `src/server.rs` |
| Admin web | `web/phpgabyadmin/index.php` |
| Pruebas del core | `tests/integration_test.rs` |
| Workflows CI / seguridad | `.github/workflows/` |
| Política de licencias / advisories | `deny.toml` |

---

## 📚 Contrato documental

Si un cambio afecta comportamiento real, revisa al menos:
- `README.md`
- `CHANGELOG.md`
- `ROADMAP.md`
- `USER_MANUAL.md`
- `RUNBOOK.md`
- la documentación técnica correspondiente en `docs/`

---

## 🧪 Qué cambios requieren prueba nueva

- cambios en formato en disco
- cambios en WAL o recovery
- cambios en parser o gramática SQL
- cambios en constraints y validación de tipos
- cambios en endpoints HTTP
- cambios en semántica de errores

---

## 🚫 Qué evitar

- introducir features “anunciadas” sin pruebas
- ampliar SQL sin revisar impacto en docs
- asumir concurrencia que todavía no existe
- exponer `phpgabyadmin` a hosts remotos sin revisar hardening
