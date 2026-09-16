//! Tauri shell assembly for the shared Web user interface.

/// Runs the desktop shell without registering business commands.
pub fn run() -> Result<(), tauri::Error> {
    tauri::Builder::default().run(tauri::generate_context!())
}
