// Prevenir consola en Windows release build
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{
    CustomMenuItem, Manager, Menu, MenuItem, Submenu, WindowMenuEvent,
};

/// Construye el menú nativo de la app. La asociación con acciones se
/// hace por id y reenviada al frontend via `window.emit("menu", id)`.
/// El frontend escucha ese evento y dispara la función JS correspondiente
/// (los IDs coinciden con los atajos ya existentes — Ctrl+S, Ctrl+Z, etc).
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

    let edit_undo      = CustomMenuItem::new("edit:undo",      "Deshacer").accelerator("CmdOrCtrl+Z");
    let edit_redo      = CustomMenuItem::new("edit:redo",      "Rehacer").accelerator("CmdOrCtrl+Y");
    let edit_select_all = CustomMenuItem::new("edit:select-all","Seleccionar todo").accelerator("CmdOrCtrl+A");
    let edit_duplicate = CustomMenuItem::new("edit:duplicate", "Duplicar selección").accelerator("CmdOrCtrl+D");
    let edit_search    = CustomMenuItem::new("edit:search",    "Buscar…").accelerator("CmdOrCtrl+F");
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

    let view_zoom_in  = CustomMenuItem::new("view:zoom-in",  "Zoom +").accelerator("CmdOrCtrl+Plus");
    let view_zoom_out = CustomMenuItem::new("view:zoom-out", "Zoom −").accelerator("CmdOrCtrl+-");
    let view_zoom_reset = CustomMenuItem::new("view:zoom-reset", "Zoom 100%").accelerator("CmdOrCtrl+0");
    let view_fit_all  = CustomMenuItem::new("view:fit-all",  "Encuadrar todo").accelerator("F");
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

    let help_docs   = CustomMenuItem::new("help:docs",   "Documentación gabysql");
    let help_about  = CustomMenuItem::new("help:about",  "Acerca de gabymodeler…");
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

fn main() {
    tauri::Builder::default()
        .menu(build_menu())
        .on_menu_event(on_menu_event)
        .setup(|app| {
            // Argumento de línea de comando = abrir archivo .gby al iniciar.
            // Esto es lo que Windows nos manda cuando se asocia el .gby.
            let args: Vec<String> = std::env::args().skip(1).collect();
            if let Some(path) = args.into_iter().find(|a| a.to_lowercase().ends_with(".gby")) {
                if let Some(win) = app.get_window("main") {
                    // El frontend escucha 'open-file' con el path completo.
                    let _ = win.emit("open-file", path);
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error mientras corría la app gabymodeler");
}
