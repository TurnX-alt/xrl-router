// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(deprecated)]

use crate::config::Config;
use crate::db::Database;
use std::sync::Arc;
use tauri::Manager;
use tracing::{info, error};

mod config;
mod crypto;
mod db;
mod error;
mod gateway;
mod http;

mod api;
mod keys;
mod middleware;
mod models;
mod plugin;
mod providers;
mod search;
mod types;

use gateway::server::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .json()
        .init();

    // Load configuration (env vars only — actual paths resolved in setup())
    let config = Config::from_env();
    info!("xrl-router starting...");
    info!("Server port: {}", config.port);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(move |app| {
            // Resolve data directory using Tauri's path API:
            //   macOS: ~/Library/Application Support/im.xrl.router/
            //   Linux: ~/.config/im.xrl.router/
            //   Windows: C:\Users\<user>\AppData\Roaming\im.xrl.router\
            let data_dir = app.path().app_data_dir()
                .expect("无法获取应用数据目录");
            std::fs::create_dir_all(&data_dir).ok();
            info!("Data directory: {}", data_dir.display());

            let db_path = data_dir.join("xrl-router.db");
            let master_key_path = data_dir.join("master.key");

            info!("Database path: {}", db_path.display());

            // Load or create master key (encrypts Provider API keys at rest)
            let master_key = crypto::load_or_create_master_key(&master_key_path)
                .map_err(|e| {
                    error!("Failed to initialize master key: {}", e);
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            // Initialize database
            let database = Database::new(&db_path)
                .map_err(|e| {
                    error!("Failed to open database: {}", e);
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            // Run migrations
            database.migrate().map_err(|e| {
                error!("Failed to run database migrations: {}", e);
                Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
            })?;

            // Create shared application state with all registries
            let app_state = Arc::new(AppState::new(config.clone(), database.clone(), master_key));

            // Pass Tauri AppHandle to PluginManager so it can emit events to frontend
            app_state.plugins.set_app_handle(app.handle().clone());

            // System tray: keep the gateway alive when the window is closed.
            let show_item =
                tauri::menu::MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let quit_item =
                tauri::menu::MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show_item, &quit_item])?;
            let _tray = tauri::tray::TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().cloned().unwrap())
                .tooltip("xrl-router")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            // Start gateway server in Tauri's async runtime
            let state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = gateway::server::start_gateway(state).await {
                    error!("Gateway server failed: {}", e);
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // Hide to tray on close instead of quitting, so the gateway keeps running.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
