# 💻 INSTALL

> **Guía de instalación y arranque de `gabysql` en Windows, Linux, macOS y Docker.**

> **Uso recomendado**: 📍 Empieza aquí si quieres levantar el producto por primera vez.

---

## 🧭 Elige tu ruta

| Ruta | Ideal para | Resultado |
|---|---|---|
| Nativo con Rust | desarrollo, depuración, cambios al motor | binarios locales + tests |
| Docker | validación reproducible, demo rápida, server + admin | stack listo con API y web |
| PHP local | uso de `phpgabyadmin` y `gabymodeler` fuera de Docker | interfaces web sobre API y modelador ER |

---

## 📋 Requisitos mínimos

- Rust estable con `cargo`
- Git
- PHP 8.2 o compatible para `phpgabyadmin`
- Docker Desktop o Docker Engine + Compose v2 si usarás contenedores

> [!TIP]
> Si tu objetivo es validar rápido el producto completo, Docker es la ruta más corta.

Consulta también [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md).

---

## 🪟 Windows

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
Si te faltan `link.exe` o bibliotecas como `kernel32.lib`, usa Docker mientras terminas de instalar el toolchain nativo.

---

## 🐧 Linux

```bash
cargo build --release --bin gabysql --bin gabysql-server
```

Recomendado además:
- `build-essential`
- `pkg-config`
- PHP 8.2 si usarás el admin web fuera de Docker

---

## 🍎 macOS

```bash
cargo build --release --bin gabysql --bin gabysql-server
```

Recomendado:
- Xcode Command Line Tools
- PHP o Docker para el admin web

---

## ⚡ Build nativo rápido

```powershell
cargo build --release --bin gabysql --bin gabysql-server
cargo run --release --bin gabysql -- init demo.db
cargo run --release --bin gabysql -- exec demo.db "CREATE TABLE users (id INT PRIMARY KEY, name TEXT);"
cargo run --release --bin gabysql -- exec demo.db "INSERT INTO users (id,name) VALUES (1,'Ana');"
cargo run --release --bin gabysql -- exec demo.db "SELECT * FROM users;"
```

---

## 🌐 Levantar el server HTTP

### Single DB
```powershell
cargo run --release --bin gabysql-server -- -db demo.db -addr :8080
```

### Multi DB
```powershell
mkdir dbs
cargo run --release --bin gabysql-server -- -dir ./dbs -addr :8080
```

---

## 🧪 Levantar las interfaces web (`phpgabyadmin` y `gabymodeler`)

Con el server ya corriendo:
```powershell
php -S localhost:8000 -t web
```

Abrir:
- Landing: `http://localhost:8000/`
- Admin web: `http://localhost:8000/phpgabyadmin/`
- Modelador ER: `http://localhost:8000/modeler/`

> El **modelador** es HTML estático puro (sin PHP necesario); cualquier servidor de archivos sirve, p.ej. `python3 -m http.server 8000 --directory web`. El **admin** sí necesita PHP para validar token y proxy a la API.

Variables útiles:
- `GABYADMIN_TOKEN`: exige login para entrar al admin
- `GABYADMIN_SERVER`: server por defecto al abrir el admin
- `GABYADMIN_ALLOW_REMOTE=1`: permite apuntar a un server remoto

---

## 🐳 Docker

### Imagen única
```powershell
docker build -t gabysql .
docker run --rm -p 8080:8080 -v ${PWD}\data:/data gabysql
```

### Stack completo
```powershell
docker compose up -d --build
```

Entradas principales:
- API: `http://localhost:8080`
- Landing: `http://localhost:8000/`
- Admin web: `http://localhost:8000/phpgabyadmin/`
- Modelador ER: `http://localhost:8000/modeler/`

---

## ✅ Validación post-instalación

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Si estás en Windows y el toolchain nativo todavía no está listo:
```powershell
docker build -t gabysql .
```

---

## 🗂️ Dónde se guardan los datos

| Modo | Ubicación |
|---|---|
| Nativo | donde tú indiques el archivo `.db` |
| Docker `gabysql` | `/data` dentro del contenedor |
| Docker Compose | volumen `gabysql-data` |
