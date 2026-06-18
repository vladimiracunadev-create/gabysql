# ADR-0093: gabymodeler como app de escritorio (Tauri + sidecar gabysql-server)

**Fecha:** 2026-06-18
**Estado:** Aceptado
**Bloque:** Productos — desktop edition (Pushes 32, 33).
**Refina:** [ADR-0092](0092-products-refresh-modeler-v3-admin-v2.md) (gabymodeler v3).

## Contexto

`gabymodeler v3` cubre 100% del motor `gabysql VERSION 33` y trae
Tier 1+2 de profesionalización (Undo/Redo, Save/Load .gby, Export
SVG/PNG, multi-selección, lasso, drag-FK, Ctrl+F, auto-layout,
migrations diff → ALTER). Pero hasta esta sesión todo eso vivía en
un archivo HTML servido por `php -S` o Docker.

Eso era suficiente para devs internos, pero un usuario que quiere
"abrir el modelador y empezar a trabajar" necesita:

1. Una **app instalable** (doble-click el `.msi`, accesos directos,
   asociación de archivos `.gby`).
2. **Cero setup**: sin Docker, sin PHP, sin Python.
3. **Funcionar offline**: sin necesidad de un `gabysql-server` ya
   levantado en alguna parte.
4. **Ventana nativa**: menús, atajos, dialog de archivos.

## Decisión

### Empaquetar con **Tauri 1.6**

| | Tauri | Electron |
|---|---|---|
| Bundle | 5-10 MB | 100-150 MB |
| WebView | Nativo del SO (WebView2 / WKWebView) | Chromium embebido |
| Backend | Rust (coherente con motor) | Node.js |
| Coldstart | <1 s | 2-4 s |
| RAM idle | ~40 MB | ~150 MB |

Tauri 1.6 (no 2.x) porque la API 1.6 es estable, los plugins de
sidecar funcionan bien y la documentación es densa. Migrar a 2.x
queda como deuda futura sin urgencia.

### Spawn `gabysql-server` como **sidecar**

Esta es la decisión clave. El alternativo era exigir que el usuario
instale `gabysql` aparte (CLI o server). Eso rompe el flujo
"doble-click → trabajando".

```rust
fn spawn_server(app: &tauri::AppHandle) -> Result<CommandChild, String> {
    let db_dir = app
        .path_resolver().app_data_dir()
        .map(|p| p.join("databases"))?;
    std::fs::create_dir_all(&db_dir)?;
    let (mut rx, child) = Command::new_sidecar("gabysql-server")?
        .args(["-dir", &db_dir.to_string_lossy(), "-addr", "127.0.0.1:18080"])
        .spawn()?;
    // drain stdout/stderr en task tokio …
    Ok(child)
}
```

Puerto fijo `127.0.0.1:18080` — convención que el frontend conoce
via `#[tauri::command] server_addr()`. Si el bind falla (puerto
ocupado) la app arranca igual, en modo offline (`localStorage` solo).
El frontend cae limpio.

DB workspace en `%APPDATA%/dev.gabysql.gabymodeler/databases/` (por
usuario, persistente entre upgrades, fuera del install dir read-only
del `.msi`).

Cleanup: `RunEvent::Exit` mata el child antes de cerrar la ventana —
sin esto el server queda zombie.

### Iconos generados programáticamente

`desktop/gabymodeler/generate_icons.py` con **Pillow puro** produce
los 16 outputs requeridos por Tauri 1.6 (32x32, 128x128, 128x128@2x,
10 Square*Logo para Microsoft Store, `icon.ico` multi-res, placeholder
`icon.icns`). Diseño: gradient diagonal `#1f6feb → #58a6ff`, brand
mark grid 2x2 blanco — coherente con la paleta del modeler v3.

Idempotente: re-correr el script regenera todo. Bajar barrera de
"diseñador con Illustrator" a "cualquiera que sepa Python".

### Build en CI, no local

El motor `gabysql` compila local sin problemas. Pero `tauri-cli`
requiere MSVC linker moderno (VS Build Tools 2022, 3-5 GB). Los
runners GitHub Actions `windows-latest` ya tienen todo eso pre-
instalado, así que el workflow `.github/workflows/desktop-release.yml`
hace:

1. `cargo build --release --bin gabysql-server`
2. Copia el `.exe` con sufijo de target triple a `binaries/`.
3. `cd src-tauri && cargo tauri build --bundles msi nsis`
4. Sube artifact + crea GitHub Release si el ref es un tag
   `desktop-v*`.

El usuario hace `git tag desktop-v0.1.0 && git push --tags` y a los
~20 min tiene el `.msi` en Releases. Sin servidor de build propio.

## Alternativas descartadas

- **Electron**: 100+ MB bundle. Cierra el caso de uso "tool que
  abrís y dejás abierto horas" en una sola decisión técnica.
- **Microsoft Store + WinUI**: lockean al ecosistema Windows
  Modern. El producto está en transición Web/Desktop, no quiero
  forzar la mano.
- **Servir solo el `web/modeler/index.html` directo** con un
  `start chrome.exe --app=...` script: feo, no genera asociación
  de archivos, sin menú nativo, sin sidecar del server.
- **Server externo obligatorio**: el usuario tiene que instalar
  gabysql separately. Rompe el flujo zero-setup.
- **Tauri 2.x**: API más reciente pero migración no trivial. Sin
  blocker concreto, queda como deuda futura.

## Consecuencias

### Positivas
- `.msi` ~10-15 MB que incluye server + modeler + asociación `.gby`.
- Sin code signing el SmartScreen pide confirmación una vez por
  máquina; con cert EV ($200/año) desaparece.
- Workflow CI corre en ~15-25 min, gratis (público).
- Server local en `127.0.0.1:18080` permite que la importación
  reverse-engineering (Push 21) funcione offline con cualquier DB
  que el usuario cree localmente.
- Cualquier feature del modelador v3 (zoom/pan/minimap/Ctrl+F/
  migrations) está disponible idéntica en la desktop edition.

### Negativas / tradeoffs
- WebView2 runtime: en Windows 11 viene, en 10 puede faltar.
  Tauri tiene `webviewInstallMode: "embedBootstrapper"` que
  agrega ~120 KB y descarga si falta. Aceptable.
- Sin auto-update: cada release el usuario descarga manualmente.
  Tauri tiene plugin de updater listo, queda como deuda.
- macOS bundle no se buildea (ICNS es placeholder; macOS real
  requiere `iconutil` que no está en runner Windows). Decisión:
  Windows primero, macOS cuando haya demanda.
- Linux .deb/.AppImage no se buildea: Tauri en Ubuntu requiere
  libwebkit2gtk-4.0-dev + libssl-dev + libgtk-3-dev. Otro job
  cuando haga falta.

## Métricas esperadas

- Bundle .msi: 10-15 MB (modeler HTML + Tauri runtime + gabysql-server
  release binary stripped).
- Tiempo coldstart: <1 s (WebView nativo + server spawn async).
- RAM idle: ~50 MB (40 WebView + 10 server con DB vacía).

## Referencias

- Commits: [376a191](https://github.com/vladimiracunadev-create/gabysql/commit/376a191) (Push 32 scaffold), [763dbe7](https://github.com/vladimiracunadev-create/gabysql/commit/763dbe7) (Push 33 sidecar + CI).
- Tag: `desktop-v0.1.0` → Release con .msi + .exe (NSIS).
- Workflow: [.github/workflows/desktop-release.yml](../../.github/workflows/desktop-release.yml).
- README de la desktop edition: [desktop/gabymodeler/README.md](../../desktop/gabymodeler/README.md).
- Sidecar config: [desktop/gabymodeler/src-tauri/tauri.conf.json](../../desktop/gabymodeler/src-tauri/tauri.conf.json) (`bundle.externalBin`).
- Modeler v3 (frontend reusado tal cual): [web/modeler/index.html](../../web/modeler/index.html) (bridge `if (window.__TAURI__)`).
