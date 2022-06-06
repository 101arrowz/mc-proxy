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
    net::TcpListener,
    io::{Read, Write}
};
use tauri::{api::path::app_dir, Manager};
use percent_encoding::percent_decode_str;

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
    access_token: Option<String>,
    username: Option<String>,
    password: Option<String>,
    api_key: Option<String>,
) -> Result<(), String> {
    if let Some(api_key) = api_key.or(state.api_key.as_ref().cloned()) {
        if let Some(access_token) = access_token.or(state.access_token.as_ref().cloned()) {
            write(
                state.file_path.as_ref().unwrap(),
                to_vec_pretty(&AppState {
                    access_token: Some(access_token.clone()),
                    api_key: Some(api_key.clone()),
                    ..Default::default()
                })
                .unwrap(),
            )
            .map_err(|err| err.to_string())?;
            start(StartConfig::Microsoft { access_token }, api_key)
                .await
                .map_err(|err| err.to_string())
        } else {
            if let Some(username) = username.or(state.username.as_ref().cloned()) {
                if let Some(password) = password.or(state.password.as_ref().cloned()) {
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
                    .map_err(|err| err.to_string())?;
                    start(StartConfig::Yggdrasil { username, password }, api_key)
                        .await
                        .map_err(|err| err.to_string())
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

#[tauri::command]
async fn ms_flow(state: tauri::State<'_, AppState>, window: tauri::Window, api_key: Option<String>) -> Result<(), String> {
    if let Some(api_key) = api_key.or(state.api_key.as_ref().cloned()) {
        let target = window.get_window("ms-oauth2").unwrap();
        target.show().map_err(|e| e.to_string())?;
        target.eval(r#"window.location.replace("https://login.live.com/oauth20_authorize.srf?client_id=128cac2a-5362-4fa5-ade3-267ed3c12503&response_type=token&redirect_uri=http%3A%2F%2Flocalhost%3A31260%2F&scope=XboxLive.signin")"#)
            .map_err(|e| e.to_string())?;
        let listener = TcpListener::bind("127.0.0.1:31260").map_err(|e| e.to_string())?;
        let mut stream = listener.accept().map_err(|e| e.to_string())?.0;
        let content = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>a</title><script>location.href=location.href.replace('#','?').replace(':31260', ':31261')</script></head></html>"#;
        stream.write_all(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", content.len(), content).as_bytes()
        ).map_err(|e| e.to_string())?;
        let listener = TcpListener::bind("127.0.0.1:31261").map_err(|e| e.to_string())?;
        let mut stream = listener.accept().map_err(|e| e.to_string())?.0;
        let mut buf = [0; 4096];
        stream.read(&mut buf).map_err(|e| e.to_string())?;
        let access_token = buf.iter().position(|&c| c == b'&').and_then(|p| {
            std::str::from_utf8(&buf[..p]).ok()
        }).and_then(|req| req.strip_prefix("GET /?access_token=")).and_then(|encoded| {
            percent_decode_str(encoded).decode_utf8().ok()
        }).ok_or("failed to extract access token")?.into_owned();
        target.hide().map_err(|err| err.to_string())?;
        begin(state, Some(access_token), None, None, Some(api_key)).await
    } else {
        Err("no API key".to_string())
    }
}

fn main() {
    let ctx = tauri::generate_context!();
    let mut conf_file = app_dir(ctx.config()).unwrap_or(current_dir().unwrap());
    create_dir_all(&conf_file).unwrap();
    conf_file.push("conf.json");
    let mut state: AppState = read_to_string(&conf_file).as_deref()
        .map_or(Ok(Default::default()), from_str)
        .unwrap_or(Default::default());
    state.file_path = Some(conf_file);
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![begin, ms_flow])
        .run(ctx)
        .expect("error while running tauri application");
}
