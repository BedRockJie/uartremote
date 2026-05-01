use serde::Serialize;
use uart_remote_core::{
    list_serial_ports as core_list_serial_ports, SerialConfig, SerialPortInfo, TokenAuth,
};

#[derive(Debug, Serialize)]
struct AppStatus {
    app_name: &'static str,
    version: &'static str,
    core_ready: bool,
    server_embedded: bool,
}

#[tauri::command]
fn app_status() -> AppStatus {
    AppStatus {
        app_name: "UartRemote",
        version: env!("CARGO_PKG_VERSION"),
        core_ready: true,
        server_embedded: false,
    }
}

#[tauri::command]
fn list_serial_ports() -> Result<Vec<SerialPortInfo>, String> {
    core_list_serial_ports().map_err(|error| error.to_string())
}

#[tauri::command]
fn default_serial_config() -> SerialConfig {
    SerialConfig::default()
}

#[tauri::command]
fn verify_token(expected: String, provided: String) -> bool {
    TokenAuth::new(expected).verify(&provided).is_ok()
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            app_status,
            default_serial_config,
            verify_token,
            list_serial_ports
        ])
        .run(tauri::generate_context!())
        .expect("failed to run UartRemote desktop app");
}
