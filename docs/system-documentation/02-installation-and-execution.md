# 02. Instalación y ejecución

La guía normativa es [INSTALL.md](../../INSTALL.md); esta página ofrece el recorrido mínimo verificado.

## Requisitos

- Rust estable con Cargo; edición del crate: 2021.
- En Windows, MSVC Build Tools y Windows SDK para compilar nativamente.
- PHP 8.2 cuando se usa `phpgabyadmin` fuera de Docker.
- Docker y Docker Compose solo para el flujo en contenedor.
- No hay dependencias Rust externas declaradas en `Cargo.toml`.

## Desarrollo

```powershell
cargo build --release --bin gabysql --bin gabysql-server
cargo run --release --bin gabysql -- init demo.db
cargo run --release --bin gabysql -- exec demo.db "SELECT 1;"
```

Servidor e interfaces:

```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
php -S localhost:8000 -t web
```

En modo multi-base se sustituye `-db demo.db` por `-dir ./dbs`. El modelador es estático; el admin requiere PHP.

## Docker

```powershell
docker build -t gabysql .
docker compose up -d --build
```

API: `http://localhost:8080`; landing/admin/modelador: `http://localhost:8000`.

## Pruebas

```powershell
cargo fmt --check
cargo check --tests
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
php -l web/index.php
php -l web/phpgabyadmin/index.php
```

## Datos iniciales, backup y restauración

`gabysql init archivo.db` crea una base. `gabysql backup origen.db copia.db`, `restore` y `verify` son las operaciones soportadas y validan CRC. No copie un `.db` activo ni omita su WAL pendiente. Consulte [RUNBOOK](../../RUNBOOK.md).

## Fallos frecuentes

`link.exe` o `kernel32.lib` ausentes indican toolchain MSVC incompleto. `database is locked` indica otro `Pager`/proceso activo. `unsupported format version` requiere una base compatible o conversión explícita; no fuerce la apertura.
