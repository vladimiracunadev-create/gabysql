# gabymodeler — Desktop app (Tauri)

> Empaquetado del modelador `gabymodeler v3` como app de escritorio
> Windows / macOS / Linux usando **Tauri 1.6**.
> Bundle final esperado: **5-10 MB** (vs 100+ MB en Electron).

---

## ¿Por qué Tauri y no Electron?

| | Tauri | Electron |
|---|---|---|
| **Bundle** | 5-10 MB | 100-150 MB |
| **WebView** | Nativo del SO (WebView2 en Win, WKWebView en macOS) | Chromium embebido |
| **Backend** | Rust (coherente con el motor gabysql) | Node.js |
| **Coldstart** | <1 s | 2-4 s |
| **RAM** | ~40 MB idle | ~150 MB idle |

Para un tool que el usuario tiene abierto **horas** mientras modela
un schema, esa diferencia se siente.

---

## Estructura

```
desktop/gabymodeler/
├── README.md              (este archivo)
└── src-tauri/
    ├── Cargo.toml         crate Rust (tauri 1.6 + plugins)
    ├── build.rs           tauri_build::build()
    ├── tauri.conf.json    manifest: identifier, bundles, allowlist, CSP, fileAssociations
    ├── icons/             .gitkeep — generar con `cargo tauri icon`
    └── src/
        └── main.rs        menú nativo + bridge frontend
```

El frontend (HTML + JS) lo provee el archivo `web/modeler/index.html`
del repo principal — Tauri lo carga directo via `distDir: "../../../web/modeler"`.
No hay copia ni build step del frontend.

---

## Prerequisitos

### Una vez por máquina

```powershell
# 1. Rust toolchain
rustup install stable

# 2. Tauri CLI
cargo install tauri-cli --version "^1.6"

# 3. (Solo Windows) WebView2 Runtime — viene con Edge moderno, casi siempre ya está.
#    Si la primera build se queja: instalá desde
#    https://developer.microsoft.com/microsoft-edge/webview2/

# 4. (Solo Windows + .msi) WiX Toolset
#    Tauri lo descarga automáticamente la primera vez que corras --bundles msi.

# 5. (Solo macOS) Xcode Command Line Tools
xcode-select --install
```

---

## Iconos

El bundle final necesita iconos. Si tenés un PNG cuadrado de 1024x1024:

```bash
cd desktop/gabymodeler
cargo tauri icon path/to/icon-source.png
```

Esto genera todos los formatos requeridos en `src-tauri/icons/`:
`32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns` (macOS),
`icon.ico` (Windows), `Square*Logo*.png` (Microsoft Store).

Mientras no hayas hecho esto, `cargo tauri dev` igual funciona con
un icono Tauri default — sólo `cargo tauri build` requiere los reales.

---

## Desarrollo

```bash
cd desktop/gabymodeler/src-tauri
cargo tauri dev
```

Abre la ventana directo apuntando a `web/modeler/index.html`. Hot
reload no aplica (no hay bundler — el archivo es estático), pero
F5 dentro de la ventana refresca.

---

## Build final

```bash
# Windows: produce .msi + .nsis en target/release/bundle/
cargo tauri build --bundles msi nsis

# macOS: produce .dmg + .app
cargo tauri build --bundles dmg

# Linux: produce .deb + .AppImage
cargo tauri build --bundles deb appimage
```

Salida típica:
```
target/release/bundle/msi/gabymodeler_0.1.0_x64_en-US.msi
target/release/bundle/nsis/gabymodeler_0.1.0_x64-setup.exe
```

El instalador `.msi` o `.exe` (NSIS) ya:
- Registra la **asociación `.gby`** (doble-click abre la app).
- Crea acceso directo en Start Menu.
- Agrega entrada de Programs & Features para desinstalar limpio.

---

## Code signing (decisión comercial)

Sin firmar, Windows muestra **SmartScreen** la primera vez que
ejecutás el `.exe`. Para evitarlo necesitás un certificado EV:

| Proveedor | Precio aproximado/año |
|---|---|
| Sectigo EV CodeSigning | USD 270 |
| DigiCert EV | USD 474 |
| SSL.com EV | USD 199 |

Una vez tengas el `.pfx`, agregás al `tauri.conf.json`:

```jsonc
"windows": {
  "certificateThumbprint": "ABCDEF1234567890...",
  "digestAlgorithm": "sha256",
  "timestampUrl": "http://timestamp.sectigo.com"
}
```

macOS necesita un Apple Developer Program ($99/año) + notarización.

---

## Auto-update

El manifest tiene `updater: { active: false }` por default. Para
activarlo:

1. Generá keypair: `cargo tauri signer generate -w ~/.tauri/gby.key`
2. Pegá la pubkey en `tauri.conf.json → updater.pubkey`.
3. Activá `updater.active = true`.
4. Configurá el endpoint que sirva el `latest.json` con la firma.

Sin esto, los usuarios tienen que descargar manualmente cada release.
Para v1 está bien — para v2 es prioritario.

---

## Menú nativo y atajos

`src/main.rs` define 5 submenús con sus atajos estándar. Cada item
emite `window.emit("menu", "<id>")` al frontend. El frontend
(en `web/modeler/index.html`) tiene un bridge `if (window.__TAURI__)`
que escucha y dispara la función JS correspondiente.

| Menú | Items |
|---|---|
| **Archivo** | Abrir / Guardar / Guardar como / Export SVG / Export PNG / Salir |
| **Edición** | Undo / Redo / Select All / Duplicate / Search |
| **Vista** | Zoom +/-/0 / Fit All / Auto-layout |
| **Herramientas** | Generar migración |
| **Ayuda** | Docs / Acerca de |

Los mismos atajos que existen en el web (Ctrl+Z, Ctrl+F, etc.)
siguen funcionando dentro de la app — los registra el frontend.
Los del menú nativo se suman como duplicados pero no entran en
conflicto (el mismo accelerator resuelve el mismo handler).

---

## File association

`tauri.conf.json → bundle.fileAssociations` registra `.gby` como
"gabymodeler model". El installer hace el registry write en Windows
(o `Info.plist` en macOS / `*.desktop` en Linux). Doble-click un
`.gby` abre la app con el path como argumento; `main.rs` lo detecta
en `setup()` y emite `open-file` al frontend, que lo lee y lo carga.

---

## TODO antes de release público

- [ ] Icono real (1024x1024 source).
- [ ] Probar build completo en Windows + macOS + Linux.
- [ ] Code signing en Windows.
- [ ] Apple Developer Program + notarización para macOS.
- [ ] Auto-update setup.
- [ ] LICENSE (apuntada por wix.license) — hoy apunta a `../../../LICENSE`,
      verificar que exista y sea texto plano.
- [ ] Testear file association: doble-click un `.gby` desde Explorer
      con la app cerrada.
- [ ] Decidir versionado independiente vs sincronizado con el motor.
