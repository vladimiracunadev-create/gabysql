# REQUIREMENTS

## Sistemas operativos objetivo
`gabysql` debe funcionar en primera instancia en:
- Windows
- Linux
- macOS

Ese soporte se resuelve en tres niveles:
- código portable en Rust
- CI en los tres sistemas
- binarios `release` generados por la CI

## Requisitos para build nativo
### Comunes
- Rust estable (`cargo`, `rustc`)
- Git

### Windows
- Rust toolchain `stable-x86_64-pc-windows-msvc`
- Visual Studio Build Tools
- MSVC C++ Build Tools
- Windows SDK

### Linux
- toolchain Rust estable
- compilador C base (`build-essential` o equivalente)

### macOS
- toolchain Rust estable
- Xcode Command Line Tools

## Requisitos opcionales
### Para `phpgabyadmin`
- PHP 8.2 validado en este repo
- navegador web

### Para Docker
- Docker Engine o Docker Desktop
- Docker Compose v2

## Requisitos de red y puertos
Por defecto:
- `8080`: `gabysql-server`
- `8000`: `phpgabyadmin` bajo Docker Compose o `php -S`

## Requisitos de almacenamiento
- archivo `.db`
- archivo `.wal` temporal cuando hay transacción activa
- espacio adicional para `target/` si compilas nativo

## Compatibilidad validada hoy
- CI Rust: `ubuntu-latest`, `windows-latest`, `macos-latest`
- Docker build: Linux container sobre Docker Desktop
- PHP lint: `web/index.php` y `web/phpgabyadmin/index.php`

## Recomendación práctica
- desarrollo del motor: build nativo
- validación reproducible: Docker
- validación multiplataforma: GitHub Actions
