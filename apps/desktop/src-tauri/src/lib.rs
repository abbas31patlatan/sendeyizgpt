use aegis_core::{ApplicationCore, RuntimeStatus};
use tauri::State;

pub struct DesktopState {
    pub core: ApplicationCore,
}

#[tauri::command]
fn runtime_status(state: State<'_, DesktopState>) -> Result<RuntimeStatus, String> {
    state.core.runtime_status().map_err(|error| error.to_string())
}

#[tauri::command]
fn stop_everything(state: State<'_, DesktopState>) -> Result<usize, String> {
    state.core.stop_everything().map_err(|error| error.to_string())
}

pub fn run() {
    let core = match ApplicationCore::new() {
        Ok(core) => core,
        Err(error) => {
            eprintln!("Aegis core failed to initialize: {error}");
            return;
        }
    };

    if let Err(error) = tauri::Builder::default()
        .manage(DesktopState { core })
        .invoke_handler(tauri::generate_handler![runtime_status, stop_everything])
        .run(tauri::generate_context!())
    {
        eprintln!("Aegis desktop application stopped with an error: {error}");
    }
}
