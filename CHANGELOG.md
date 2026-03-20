# Changelog

## 2026-03-19 - Segunda intervención: migración completa a Rust y estabilización base

### Estado actual del sistema
- Motor embebido en Rust con archivo único `.db`.
- CLI `gabysql` para `init`, `info`, `exec` y `repl`.
- Server HTTP `gabysql-server` para operar una base única o un directorio de bases.
- `phpgabyadmin` consumiendo la API HTTP como consola web liviana.
- Docker y `docker compose` para levantar server y admin web en un entorno reproducible.

### Cambios estructurales
- Se eliminó la implementación anterior en Go y se reemplazó por un proyecto Rust con `Cargo`.
- Se separó el core en módulos de storage, catálogo, SQL, servidor y estructura persistente por clave primaria.
- Se unificó la documentación para reflejar solo las capacidades reales del motor actual.

### Mejoras funcionales
- Soporte de `CREATE TABLE`, `INSERT` y `SELECT` con full scan, `LIMIT/OFFSET`, `WHERE <pk> = ...` y `BETWEEN`.
- Soporte de tipos `INT`, `TEXT`, `BOOL`, `FLOAT`, `DATE`, `DATETIME`, `JSON` y `NULL` en columnas no PK.
- Rechazo explícito de claves primarias duplicadas en vez de sobrescritura silenciosa.
- Recovery WAL por marcador `COMMIT` para rehidratar páginas confirmadas tras reinicio.

### Estabilidad y seguridad
- El parser SQL ahora devuelve errores controlados en escenarios inválidos en lugar de derribar el proceso.
- Se corrigió el manejo de comillas escapadas dentro de strings SQL para soportar textos complejos en inserciones multi-sentencia.
- `phpgabyadmin` quedó endurecido con cookie firmada y bloqueo de servidores remotos salvo habilitación explícita.
- La UI web y el README quedaron alineados con el comportamiento real del motor.

### Documentación del producto
- Se creó un set documental completo alineado con el estándar usado en otros repos del perfil.
- Se añadieron guías de instalación, uso, operación, seguridad, troubleshooting y contribución.
- Se añadió documentación técnica de arquitectura, requisitos, API y especificaciones del motor.
- Se dejó trazabilidad de cambios, límites actuales y dirección de roadmap sin inflar capacidades.

### Validación y entrega continua
- Se agregaron pruebas de integración para roundtrip básico, PK duplicada, paginación con `LIMIT/OFFSET`, `NULL`, parser inválido y recovery WAL.
- Se agregó CI en GitHub Actions con `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` y lint de PHP.
- La matriz de CI cubre `ubuntu-latest`, `windows-latest` y `macos-latest`, más build Docker en Linux.
- La CI publica artefactos `release` por sistema operativo para facilitar distribución nativa multiplataforma.
- El `Dockerfile` valida `cargo test --all-targets` antes de construir binarios release.
- `docker compose` permite probar juntos `gabysql-server` y `phpgabyadmin`.

### Validación realizada en esta intervención
- `cargo fmt --check`: OK
- `cargo check --tests`: OK
- `cargo clippy --all-targets -- -D warnings`: OK
- `docker build -t gabysql .`: OK
- `docker compose up -d --build`: OK
- `GET http://localhost:8080/health`: OK
- `GET http://localhost:8000`: OK

### Límites actuales conocidos
- El índice persistente sigue siendo una estructura de hojas enlazadas por PK `INT`; no es todavía un B+Tree multinivel completo.
- No hay optimizer cost-based ni estadísticas de consulta.
- No hay concurrencia avanzada, MVCC ni transacciones complejas.
- Sigue siendo un producto base estable para evolucionar, no un reemplazo directo de motores maduros como PostgreSQL o MySQL.
