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

---

## 🗺️ Dónde tocar cada cosa

| Área | Archivo principal |
|---|---|
| Pager, header, WAL y recovery | `src/storage.rs` |
| Índice persistente por PK | `src/bptree.rs` |
| Catálogo de tablas | `src/catalog.rs` |
| SQL parser, engine y row codec | `src/sql.rs` |
| API HTTP/JSON | `src/server.rs` |
| Admin web | `web/phpgabyadmin/index.php` |
| Pruebas del core | `tests/integration_test.rs` |

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
