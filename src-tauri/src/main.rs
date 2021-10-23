#![cfg_attr(
  all(not(debug_assertions), target_os = "windows"),
  windows_subsystem = "windows"
)]

use mc_proxy::start;
use std::env::var;

#[tauri::command]
async fn begin() {
  dbg!("INVOKED");
  start(var("UN").unwrap(), var("PW").unwrap(), var("KEY").unwrap()).await.unwrap();
}

fn main() {
  tauri::Builder::default()
    .invoke_handler(tauri::generate_handler![begin])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
