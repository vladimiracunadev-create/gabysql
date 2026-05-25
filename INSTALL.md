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

Hay **tres rutas** según qué quieras: instalar listo para usar, descargar manualmente, o compilar desde fuentes.

### Ruta A — Instalador one-liner (recomendado para usuarios)

Desde PowerShell, una sola línea:

```powershell
iwr https://raw.githubusercontent.com/vladimiracunadev-create/gabysql/main/scripts/install.ps1 | iex
```

Esto:
1. Consulta el último release publicado en GitHub.
2. Descarga `gabysql-<tag>-windows-x86_64.zip` y verifica el SHA256.
3. Extrae los binarios (`gabysql.exe`, `gabysql-server.exe`) a `%LOCALAPPDATA%\Programs\gabysql\`.
4. Agrega ese directorio al `PATH` del usuario (no toca el PATH del sistema, no requiere admin).

Después abrí **una nueva terminal** (las terminales abiertas no ven el PATH refrescado) y verificá:

```powershell
gabysql --version
gabysql init mi-base.db
gabysql exec mi-base.db "SELECT 1;"
```

Variantes:

```powershell
# Instalar una versión específica
& ([scriptblock]::Create((iwr https://raw.githubusercontent.com/vladimiracunadev-create/gabysql/main/scripts/install.ps1).Content)) -Version v0.2.0

# Sin modificar el PATH (uso con ruta absoluta)
& ([scriptblock]::Create((iwr https://raw.githubusercontent.com/vladimiracunadev-create/gabysql/main/scripts/install.ps1).Content)) -NoPath

# Directorio destino custom
& ([scriptblock]::Create((iwr https://raw.githubusercontent.com/vladimiracunadev-create/gabysql/main/scripts/install.ps1).Content)) -InstallDir 'D:\tools\gabysql'
```

> Si PowerShell rechaza el script con un error de política, corré una vez (solo para tu usuario):
> ```powershell
> Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned
> ```

**Para desinstalar**: borrar `%LOCALAPPDATA%\Programs\gabysql` y sacar esa entrada del PATH del usuario (Configuración → Sistema → Acerca de → Configuración avanzada → Variables de entorno).

### Ruta B — Descarga manual del release

Si no querés ejecutar el script:

1. Ir a [Releases](https://github.com/vladimiracunadev-create/gabysql/releases).
2. Descargar `gabysql-<tag>-windows-x86_64.zip`.
3. Descomprimir donde quieras (ej: `C:\gabysql\`).
4. Opcional: agregá esa carpeta al `PATH` para tener `gabysql` accesible desde cualquier `cmd`/`pwsh`.

### Ruta C — Build nativo desde fuentes

Si querés compilar (porque vas a modificar el motor, o porque querés validar la cadena de build):

1. Instalá [`rustup`](https://rustup.rs) y el toolchain estable.
2. Para que `cargo test` y `cargo build` funcionen en Windows, instalá:
   - Visual Studio Build Tools
   - MSVC C++ build tools
   - Windows SDK
3. Compilá:
   ```powershell
   git clone https://github.com/vladimiracunadev-create/gabysql.git
   cd gabysql
   cargo build --release --bin gabysql --bin gabysql-server
   ```
4. Los binarios quedan en `target\release\gabysql.exe` y `target\release\gabysql-server.exe`. Copiá donde quieras y agregá al `PATH`.

### Si fallan `link.exe` o `kernel32.lib`

Falta el toolchain MSVC. Dos opciones: instalá Visual Studio Build Tools (la solución correcta para la Ruta C) o usá la **Ruta A / Ruta B** que no requieren toolchain.

### Primer uso (cualquier ruta)

```powershell
gabysql init demo.db
gabysql exec demo.db "CREATE TABLE notas (id INT PRIMARY KEY, texto TEXT);"
gabysql exec demo.db "INSERT INTO notas (id, texto) VALUES (1, 'hola');"
gabysql exec demo.db "SELECT * FROM notas;"
gabysql repl demo.db   # REPL interactivo
```

La base de datos es **un archivo único** (`demo.db` + su `demo.db.wal`). Para backup: `gabysql backup demo.db backup.gby`. Para diagnóstico: `gabysql info demo.db`.

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
