// Prevenir consola en Windows release build
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;
use tauri::{
    api::process::{Command, CommandChild, CommandEvent},
    CustomMenuItem, Manager, Menu, MenuItem, RunEvent, Submenu, WindowMenuEvent,
};

/// Estado global: handle al child del gabysql-server para poder
/// matarlo en el shutdown.
struct ServerHandle(Mutex<Option<CommandChild>>);

/// Spawn `gabysql-server` como sidecar. La binary se empaqueta en el
/// bundle vía `tauri.conf.json → bundle.externalBin`, así que en
/// runtime queda copada dentro del install dir; Tauri se encarga de
/// resolver el path correcto en cada plataforma.
///
/// Strategy:
/// - `-dir <appDataDir>/databases` — un workspace por usuario, fuera
///   del install dir read-only del .msi.
/// - `-addr 127.0.0.1:18080` — puerto fijo conocido por el frontend.
///   18080 es nuestro convenio (8080 + 10000 para no colisionar con
///   el bin "remoto" de desarrollo); si está ocupado el bind falla y
///   el frontend cae al modo offline (localStorage only).
/// - Sin `-token` — modo local, todo lo que llega al puerto es del
///   usuario que abre la app. CSP en el manifest impide CORS desde
///   otro origen.
fn spawn_server(app: &tauri::AppHandle) -> Result<CommandChild, String> {
    let app_data = app
        .path_resolver()
        .app_data_dir()
        .ok_or("no se pudo resolver app_data_dir")?;
    let db_dir = app_data.join("databases");
    std::fs::create_dir_all(&db_dir)
        .map_err(|e| format!("no se pudo crear db_dir {}: {}", db_dir.display(), e))?;

    let (mut rx, child) = Command::new_sidecar("gabysql-server")
        .map_err(|e| format!("sidecar gabysql-server no encontrado: {}", e))?
        .args([
            "-dir",
            db_dir.to_string_lossy().as_ref(),
            "-addr",
            "127.0.0.1:18080",
        ])
        .spawn()
        .map_err(|e| format!("spawn falló: {}", e))?;

    // Drain stdout/stderr en background — sin esto los pipes se llenan
    // y el server bloquea después de N kilobytes.
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(line) | CommandEvent::Stderr(line) => {
                    // Emit al frontend para debug (ventana DevTools).
                    let _ = app_handle.emit_all("server-log", line);
                }
                CommandEvent::Terminated(payload) => {
                    let _ = app_handle.emit_all("server-terminated", payload.code);
                    break;
                }
                _ => {}
            }
        }
    });

    Ok(child)
}

/// Construye el menú nativo de la app. La asociación con acciones se
/// hace por id y reenviada al frontend via `window.emit("menu", id)`.
fn build_menu() -> Menu {
    let file_open    = CustomMenuItem::new("file:open",      "Abrir .gby...").accelerator("CmdOrCtrl+O");
    let file_save    = CustomMenuItem::new("file:save",      "Guardar").accelerator("CmdOrCtrl+S");
    let file_save_as = CustomMenuItem::new("file:save-as",   "Guardar como .gby...").accelerator("CmdOrCtrl+Shift+S");
    let file_export_svg = CustomMenuItem::new("file:export-svg", "Exportar como SVG");
    let file_export_png = CustomMenuItem::new("file:export-png", "Exportar como PNG");
    let file_quit    = CustomMenuItem::new("file:quit",      "Salir").accelerator("CmdOrCtrl+Q");

    let file_menu = Submenu::new(
        "Archivo",
        Menu::new()
            .add_item(file_open)
            .add_item(file_save)
            .add_item(file_save_as)
            .add_native_item(MenuItem::Separator)
            .add_item(file_export_svg)
            .add_item(file_export_png)
            .add_native_item(MenuItem::Separator)
            .add_item(file_quit),
    );

    let edit_undo       = CustomMenuItem::new("edit:undo",       "Deshacer").accelerator("CmdOrCtrl+Z");
    let edit_redo       = CustomMenuItem::new("edit:redo",       "Rehacer").accelerator("CmdOrCtrl+Y");
    let edit_select_all = CustomMenuItem::new("edit:select-all", "Seleccionar todo").accelerator("CmdOrCtrl+A");
    let edit_duplicate  = CustomMenuItem::new("edit:duplicate",  "Duplicar selección").accelerator("CmdOrCtrl+D");
    let edit_search     = CustomMenuItem::new("edit:search",     "Buscar…").accelerator("CmdOrCtrl+F");
    let edit_menu = Submenu::new(
        "Edición",
        Menu::new()
            .add_item(edit_undo)
            .add_item(edit_redo)
            .add_native_item(MenuItem::Separator)
            .add_item(edit_select_all)
            .add_item(edit_duplicate)
            .add_native_item(MenuItem::Separator)
            .add_item(edit_search),
    );

    let view_zoom_in    = CustomMenuItem::new("view:zoom-in",    "Zoom +").accelerator("CmdOrCtrl+Plus");
    let view_zoom_out   = CustomMenuItem::new("view:zoom-out",   "Zoom −").accelerator("CmdOrCtrl+-");
    let view_zoom_reset = CustomMenuItem::new("view:zoom-reset", "Zoom 100%").accelerator("CmdOrCtrl+0");
    let view_fit_all    = CustomMenuItem::new("view:fit-all",    "Encuadrar todo").accelerator("F");
    let view_autolayout = CustomMenuItem::new("view:autolayout", "Auto-layout");
    let view_menu = Submenu::new(
        "Vista",
        Menu::new()
            .add_item(view_zoom_in)
            .add_item(view_zoom_out)
            .add_item(view_zoom_reset)
            .add_native_item(MenuItem::Separator)
            .add_item(view_fit_all)
            .add_item(view_autolayout),
    );

    let tools_migrate = CustomMenuItem::new("tools:migrate", "Generar migración…");
    let tools_menu = Submenu::new(
        "Herramientas",
        Menu::new().add_item(tools_migrate),
    );

    let help_docs  = CustomMenuItem::new("help:docs",  "Documentación gabysql");
    let help_about = CustomMenuItem::new("help:about", "Acerca de gabymodeler…");
    let help_menu = Submenu::new(
        "Ayuda",
        Menu::new().add_item(help_docs).add_item(help_about),
    );

    Menu::new()
        .add_submenu(file_menu)
        .add_submenu(edit_menu)
        .add_submenu(view_menu)
        .add_submenu(tools_menu)
        .add_submenu(help_menu)
}

/// Bridge menu→frontend. El JS escucha con
///     window.__TAURI__.event.listen('menu', e => router(e.payload))
fn on_menu_event(event: WindowMenuEvent) {
    let id = event.menu_item_id().to_string();
    let _ = event.window().emit("menu", id);
}

#[tauri::command]
fn server_addr() -> &'static str {
    "http://127.0.0.1:18080"
}

fn main() {
    let app = tauri::Builder::default()
        .menu(build_menu())
        .on_menu_event(on_menu_event)
        .invoke_handler(tauri::generate_handler![server_addr])
        .manage(ServerHandle(Mutex::new(None)))
        .setup(|app| {
            // 1) Spawn gabysql-server como sidecar. Si falla, la app
            //    arranca igual — el frontend cae a modo offline (sin
            //    importación reverse-engineering, pero el modelado
            //    local con localStorage sigue funcionando).
            match spawn_server(&app.handle()) {
                Ok(child) => {
                    let state = app.state::<ServerHandle>();
                    *state.0.lock().unwrap() = Some(child);
                    eprintln!("gabysql-server arrancado en 127.0.0.1:18080");
                }
                Err(e) => eprintln!("gabysql-server NO arrancó: {}", e),
            }

            // 2) Argumento de línea de comando = abrir archivo .gby al iniciar.
            //    Esto es lo que Windows nos manda cuando se asocia el .gby.
            let args: Vec<String> = std::env::args().skip(1).collect();
            if let Some(path) = args.into_iter().find(|a| a.to_lowercase().ends_with(".gby")) {
                if let Some(win) = app.get_window("main") {
                    let _ = win.emit("open-file", path);
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error mientras se construía la app gabymodeler");

    // Run con handler de exit que kill al server child antes de cerrar.
    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
            // Scopear el MutexGuard al bloque interior — sin esto
            // el guard del `state.0.lock()` vive hasta el final del
            // `if let Some(child)` y rustc rechaza (E0597) porque el
            // `state` local se dropea antes que el guard temporal.
            let child = {
                let state = app_handle.state::<ServerHandle>();
                let mut slot = state.0.lock().unwrap();
                slot.take()
            };
            if let Some(child) = child {
                let _ = child.kill();
                eprintln!("gabysql-server detenido");
            }
        }
    });
}
