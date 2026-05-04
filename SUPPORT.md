# 🤝 SUPPORT

> **Cómo pedir ayuda o reportar problemas en `gabysql`.**

---

## 🔎 Antes de reportar

Revisa primero, en este orden:

1. [README.md](README.md) — qué soporta hoy y qué no.
2. [USER_MANUAL.md](USER_MANUAL.md) — uso diario del CLI, server y admin web.
3. [INSTALL.md](INSTALL.md) — build nativo y Docker.
4. [TROUBLESHOOTING.md](TROUBLESHOOTING.md) — errores frecuentes (formato de archivo, checksums, refusing-to-overwrite, server busy, fila no existe).
5. [docs/API.md](docs/API.md) — endpoints HTTP/JSON.
6. [docs/TECHNICAL_SPECS.md](docs/TECHNICAL_SPECS.md) — formato en disco, WAL, B+Tree, índices, gramática SQL.
7. [RUNBOOK.md](RUNBOOK.md) — recovery, backup/restore, smoke checks.

## 📨 Cómo pedir ayuda

| Caso | Canal |
|---|---|
| Bug reproducible | abre un Issue con pasos mínimos |
| Mejora estructural / dirección del producto | abre una Discussion antes de codificar |
| Vulnerabilidad de seguridad | **NO abrir Issue público.** Sigue [SECURITY.md](SECURITY.md) |
| Pregunta de uso o conceptual | Discussion |

## 🧾 Qué incluir en un reporte útil

- commit SHA exacto (idealmente `HEAD` de `main`).
- sistema operativo y versión.
- toolchain Rust (`rustc --version`).
- modo de operación: CLI / `gabysql-server` / Docker.
- comando ejecutado.
- resultado esperado.
- resultado real (con error message exacto y stderr completo).
- si toca el formato en disco: salida de `gabysql info <file.db>` antes y después.
- si toca el server: cabeceras HTTP relevantes y código de respuesta.

## ⚖️ Alcance del soporte

Este repositorio está orientado a:

- aprendizaje aplicado de motores de bases de datos
- evaluación técnica del producto
- producto base estable que evoluciona por fases (ver [ROADMAP.md](ROADMAP.md))
- entorno reproducible vía Docker

**No promete:**

- SLA comercial
- soporte de producción 24/7
- compatibilidad SQL completa con Postgres / MySQL
- migraciones automáticas entre versiones del formato en disco — bumps de `VERSION` rechazan DBs antiguas con error explícito (ver [CHANGELOG.md](CHANGELOG.md))

## 🔁 Tiempos de respuesta orientativos

| Tipo | Acuse | Triage |
|---|---|---|
| Vulnerabilidad de seguridad | 3 días hábiles | 7 días hábiles |
| Bug crítico (corrupción de datos) | 5 días hábiles | 10 días hábiles |
| Bug funcional | best effort | best effort |
| Pregunta de uso | best effort | — |

Los plazos son orientativos: este repo se mantiene como producto base, no como servicio gestionado.
