# INSTALL

## Objetivo
Esta guía deja `gabysql` funcionando de forma nativa o con Docker en Windows, Linux y macOS.

## Rutas recomendadas
- Si quieres validar rápido el producto completo: usa Docker.
- Si quieres desarrollar o depurar el motor: usa build nativo con Rust.
- Si quieres usar `phpgabyadmin`: necesitas además PHP para servir `web/` o Docker Compose.

## Requisitos mínimos
- Rust estable con `cargo`.
- Git para clonar/versionar.
- PHP 8.2 o compatible para `phpgabyadmin`.
- Docker Desktop o Docker Engine + Compose v2 si usarás contenedores.

Consulta también [docs/REQUIREMENTS.md](./docs/REQUIREMENTS.md).

## Windows
### Build nativo
1. Instala `rustup` y el toolchain estable.
2. Si quieres correr `cargo test` nativo en Windows, instala además:
   - Visual Studio Build Tools
   - MSVC C++ build tools
   - Windows SDK
3. Compila:
```powershell
cargo build --release --bin gabysql --bin gabysql-server
```

### Nota importante
Si no tienes `link.exe` o bibliotecas como `kernel32.lib`, el camino más rápido es usar Docker para validar el proyecto completo mientras instalas el toolchain nativo de Windows.

## Linux
```bash
cargo build --release --bin gabysql --bin gabysql-server
```

Recomendado además:
- `build-essential`
- `pkg-config`
- PHP 8.2 si usarás el admin web fuera de Docker

## macOS
```bash
cargo build --release --bin gabysql --bin gabysql-server
```

Recomendado:
- Xcode Command Line Tools
- PHP o Docker para el admin web

## Build nativo rápido
```powershell
cargo build --release --bin gabysql --bin gabysql-server
cargo run --release --bin gabysql -- init demo.db
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, name TEXT);"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,name) VALUES (1,'Ana');"
cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users;"
```

## Levantar el server HTTP
### Single DB
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
```

### Multi DB
```powershell
mkdir dbs
cargo run --release --bin gabysql-server -- -dir ./dbs -addr :8080
```

## Levantar `phpgabyadmin`
Con el server ya corriendo:
```powershell
php -S localhost:8000 -t web
```

Abrir:
- `http://localhost:8000/phpgabyadmin/`

Variables útiles:
- `GABYADMIN_TOKEN`: exige login para entrar al admin.
- `GABYADMIN_SERVER`: server por defecto al abrir el admin.
- `GABYADMIN_ALLOW_REMOTE=1`: permite apuntar a un server remoto.

## Docker
### Imagen única
```powershell
docker build -t gabysql .
docker run --rm -p 8080:8080 -v ${PWD}\data:/data gabysql
```

### Stack completo
```powershell
docker compose up -d --build
```

Servicios:
- API: `http://localhost:8080`
- Admin web: `http://localhost:8000/phpgabyadmin/`

## Validación post-instalación
```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Si estás en Windows y el toolchain nativo todavía no está listo, valida así:
```powershell
docker build -t gabysql .
```

## Dónde se guardan los datos
- Nativo: donde tú indiques el archivo `.db`.
- Docker `gabysql`: en `/data` dentro del contenedor; monta un volumen o carpeta host.
- Docker Compose: volumen `gabysql-data`.
