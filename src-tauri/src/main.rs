#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use mc_proxy::{start, StartConfig};
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_vec_pretty};
use std::{
    env::current_dir,
    fs::{create_dir_all, read_to_string, write},
    path::PathBuf,
};
use tauri::api::path::app_dir;

#[derive(Serialize, Deserialize, Default)]
struct AppState {
    username: Option<String>,
    password: Option<String>,
    api_key: Option<String>,
    access_token: Option<String>,
    file_path: Option<PathBuf>,
}

#[tauri::command]
async fn begin(
    state: tauri::State<'_, AppState>,
    username: Option<String>,
    password: Option<String>,
    api_key: Option<String>,
) -> Result<(), String> {
    if let Some(api_key) = state.api_key.as_ref().cloned().or(api_key) {
        if let Some(access_token) = state.access_token.as_ref().cloned() {
            write(
                state.file_path.as_ref().unwrap(),
                to_vec_pretty(&AppState {
                    access_token: Some(access_token.clone()),
                    api_key: Some(api_key.clone()),
                    ..Default::default()
                })
                .unwrap(),
            )
            .map_err(|err| format!("{:?}", err))?;
            start(StartConfig::Microsoft { access_token }, api_key)
                .await
                .map_err(|err| format!("{:?}", err))
        } else {
            if let Some(username) = state.username.as_ref().cloned().or(username) {
                if let Some(password) = state.password.as_ref().cloned().or(password) {
                    write(
                        state.file_path.as_ref().unwrap(),
                        to_vec_pretty(&AppState {
                            username: Some(username.clone()),
                            password: Some(password.clone()),
                            api_key: Some(api_key.clone()),
                            ..Default::default()
                        })
                        .unwrap(),
                    )
                    .map_err(|err| format!("{:?}", err))?;
                    start(StartConfig::Yggdrasil { username, password }, api_key)
                        .await
                        .map_err(|err| format!("{:?}", err))
                } else {
                    Err("no password".into())
                }
            } else {
                Err("no username".into())
            }
        }
    } else {
        Err("no API key".into())
    }
}

fn main() {
    let ctx = tauri::generate_context!();
    let mut conf_file = app_dir(ctx.config()).unwrap_or(current_dir().unwrap());
    create_dir_all(&conf_file).unwrap();
    conf_file.push("conf.json");
    let mut state: AppState = read_to_string(&conf_file)
        .as_deref()
        .map_or(Ok(Default::default()), from_str)
        .unwrap_or(Default::default());
    state.file_path = Some(conf_file);
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![begin])
        .run(ctx)
        .expect("error while running tauri application");
}
