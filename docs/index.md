---
title: gabysql
---

# gabysql

> **Base de datos relacional embebida en Rust** — archivo único portable, superficie SQL clásica (DDL + DML + TCL), zero-deps en el core. Versión actual: `v0.1.0` (Fase 2 cerrada el 2026-05-25).

## ⚡ Instalar en Windows en una línea

```powershell
iwr https://raw.githubusercontent.com/vladimiracunadev-create/gabysql/main/scripts/install.ps1 | iex
```

Después: `gabysql init mi.db && gabysql exec mi.db "SELECT 1;"`. Detalle completo en [INSTALL](https://github.com/vladimiracunadev-create/gabysql/blob/main/INSTALL.md).

Para Linux/macOS: descargar el `.tar.gz` correspondiente de [Releases](https://github.com/vladimiracunadev-create/gabysql/releases/latest) o compilar con `cargo build --release`.

## 🔍 ¿Qué hace gabysql?

Una **base de datos relacional** que cabe en **un solo archivo** (`mi.db` + `mi.db.wal`). Diseñada para casos donde SQLite resulta restrictivo en disco/tipos pero PostgreSQL es overkill:

- Apps Rust embebidas que necesitan SQL real en lugar de un KV.
- Workloads OLTP clásicos (PK lookup + índices + JOINs equi-predicado).
- Reporting básico con `GROUP BY` / `HAVING` / agregados.
- Cualquier proyecto que valore _supply chain_ Rust pura (sin C en el core).

**No reemplaza** a PostgreSQL/MySQL para workloads OLAP, MVCC, replicación, window functions o CTE recursivas — esas viven en otros motores.

## 🟢 Superficie SQL soportada hoy

**DDL** — `CREATE / DROP DATABASE`, `SHOW DATABASES`, `CREATE TABLE` (PK, NOT NULL, UNIQUE, DEFAULT, REFERENCES con `ON DELETE RESTRICT/CASCADE`), `DROP TABLE [IF EXISTS]`, `ALTER TABLE ADD COLUMN`, `CREATE [UNIQUE] INDEX`, `DROP INDEX`, `TRUNCATE [TABLE]`.

**DML** — `INSERT` single-row, multi-row `VALUES (..),(..)`, `INSERT INTO t SELECT ...`, `ON CONFLICT [(col)] DO NOTHING / DO UPDATE SET ...`, `REPLACE INTO`, `INSERT/UPDATE/DELETE ... RETURNING * | cols`, `SELECT/UPDATE/DELETE` con `WHERE` completo, `JOIN` (INNER/LEFT/RIGHT/FULL/CROSS/USING/NATURAL + index-loop), `GROUP BY` + `HAVING` + `COUNT/SUM/AVG/MIN/MAX` + `DISTINCT`, subqueries `IN`/`=`/`EXISTS`, `ORDER BY`, `LIMIT`/`OFFSET`.

**TCL + ops** — `BEGIN / START TRANSACTION`, `COMMIT / END`, `ROLLBACK`, `INTEGRITY CHECK`, backup/restore/verify (CLI con CRC end-to-end).

**WHERE** — `=`, `<`, `>`, `<=`, `>=`, `<>`/`!=`, `BETWEEN`, `IS [NOT] NULL`, `[NOT] LIKE` (con `%`/`_`/escape `\`), `[NOT] IN (lista | SELECT)`, `EXISTS (SELECT)`, combinados con `AND`/`OR`/`NOT` y paréntesis — lógica trivaluada ANSI para NULL.

Detalle exhaustivo de cada cláusula: [SQL_REFERENCE](SQL_REFERENCE.md). Catálogo de errores `[GBY-NNNN]`: [ERROR_CODES](ERROR_CODES.md). Lo que aún falta: [MISSING_COMMANDS](MISSING_COMMANDS.md).

## 🚀 Modos de uso

| Modo | Cuándo conviene | Cómo arrancar |
|---|---|---|
| **CLI** (`gabysql.exe`) | Scripts, automatización local, REPL interactivo | `gabysql exec mi.db "SELECT ...;"` / `gabysql repl mi.db` |
| **Server HTTP** (`gabysql-server.exe`) | App que conecta por red, multi-cliente | `gabysql-server -db mi.db -addr :8080` → POST `/exec` |
| **Crate Rust embebido** | App Rust nativa, cero red ni proceso aparte | `gabysql = { … }` + `use gabysql::{sql, storage};` |
| **Docker Compose** | Demo rápida con UI web (phpgabyadmin) | `docker compose up -d` → `localhost:8000` |

Detalle en [USER_MANUAL](https://github.com/vladimiracunadev-create/gabysql/blob/main/USER_MANUAL.md) y [API](API.md).

## 📚 Documentación por perfil

### 👶 Principiante
- [BEGINNERS_GUIDE](BEGINNERS_GUIDE.md) — primeros pasos asumiendo cero contexto.
- [QUICKSTART](https://github.com/vladimiracunadev-create/gabysql/blob/main/QUICKSTART.md) — arranque en 5 minutos.
- [USER_MANUAL](https://github.com/vladimiracunadev-create/gabysql/blob/main/USER_MANUAL.md) — CLI, server, REPL, admin web.

### 🛠️ Operación
- [RUNBOOK](https://github.com/vladimiracunadev-create/gabysql/blob/main/RUNBOOK.md) — backup/restore, métricas, observabilidad.
- [TROUBLESHOOTING](https://github.com/vladimiracunadev-create/gabysql/blob/main/TROUBLESHOOTING.md) — errores comunes con soluciones.
- [SECURITY](https://github.com/vladimiracunadev-create/gabysql/blob/main/SECURITY.md) · [SECURITY_LAYERS](SECURITY_LAYERS.md).

### 📖 Referencia
- [SQL_REFERENCE](SQL_REFERENCE.md) — gramática EBNF + railroad + ejemplos.
- [ERROR_CODES](ERROR_CODES.md) — catálogo `[GBY-NNNN]` con causa y remediación.
- [USE_CASES](USE_CASES.md) — recetas.
- [API](API.md) — endpoints HTTP, request/response JSON.

### 🔧 Técnico
- [ARCHITECTURE](ARCHITECTURE.md) — diseño interno.
- [TECHNICAL_SPECS](TECHNICAL_SPECS.md) — formato en disco, subset SQL exacto.
- [STATUS](STATUS.md) — madurez por subsistema.
- [ADRs](adr/) — decisiones arquitectónicas.

### 📈 Producto
- [POSITIONING](POSITIONING.md) — para qué SÍ / para qué NO.
- [COMPETITIVE_ANALYSIS](COMPETITIVE_ANALYSIS.md) — vs SQLite, DuckDB, Postgres.
- [COMMERCIAL_ROADMAP](COMMERCIAL_ROADMAP.md) — caminos A/B/C.
- [MISSING_COMMANDS](MISSING_COMMANDS.md) — qué falta del SQL clásico.

### 🤝 Contribuir
- [CONTRIBUTING](https://github.com/vladimiracunadev-create/gabysql/blob/main/CONTRIBUTING.md).
- [CHANGELOG](https://github.com/vladimiracunadev-create/gabysql/blob/main/CHANGELOG.md) — historia de cambios por release.
- [ROADMAP](https://github.com/vladimiracunadev-create/gabysql/blob/main/ROADMAP.md).

## 🔗 Recursos externos

- **Repo**: <https://github.com/vladimiracunadev-create/gabysql>
- **Releases (binarios)**: <https://github.com/vladimiracunadev-create/gabysql/releases>
- **Issues**: <https://github.com/vladimiracunadev-create/gabysql/issues>
- **Licencia**: [MIT](https://github.com/vladimiracunadev-create/gabysql/blob/main/LICENSE).

---

<sub>Sitio generado con Jekyll desde el directorio <code>docs/</code> del repo. Auto-deploy en cada push a <code>main</code> vía <code>.github/workflows/pages.yml</code>.</sub>
