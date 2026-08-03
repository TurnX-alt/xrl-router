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

/// 开机自启（autostart 登录项）携带的参数：命中时启动不显示窗口，静默驻留托盘。
/// 必须与 `tauri_plugin_autostart::init` 的 args 保持一致（见下方 Builder 配置）。
const ARG_MINIMIZED: &str = "--minimized";

// ── Autostart Tauri commands ──
#[tauri::command]
fn get_autostart_status(app: tauri::AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    if enabled {
        app.autolaunch().enable().map_err(|e| e.to_string())
    } else {
        app.autolaunch().disable().map_err(|e| e.to_string())
    }
}

// ── i18n（托盘菜单文本随前端语言切换） ──

/// 托盘菜单项句柄（前端切换语言时 set_locale 更新文本）。
struct TrayMenuState {
    show_item: tauri::menu::MenuItem<tauri::Wry>,
    quit_item: tauri::menu::MenuItem<tauri::Wry>,
}

/// 按 locale 返回托盘菜单文本（zh-CN 默认）。
fn tray_texts(locale: &str) -> (&'static str, &'static str) {
    if locale == "en" {
        ("Show Window", "Quit")
    } else {
        ("显示窗口", "退出")
    }
}

/// 前端切换语言时调用：持久化 locale 并更新托盘菜单文本。
#[tauri::command]
fn set_locale(app: tauri::AppHandle, locale: String) -> Result<(), String> {
    use tauri::Manager;
    let state = app.state::<Arc<AppState>>();
    state
        .database
        .set_setting("locale", &locale)
        .map_err(|e| e.to_string())?;
    let (show_txt, quit_txt) = tray_texts(&locale);
    let menu_state = app.state::<TrayMenuState>();
    let _ = menu_state.show_item.set_text(show_txt);
    let _ = menu_state.quit_item.set_text(quit_txt);
    Ok(())
}

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

    // 开机自启时系统以 `xrl-router --minimized` 拉起进程；setup 里据此隐藏窗口。
    // 手动启动（无该参数）不受影响，正常弹出窗口。
    let silent_start = std::env::args().any(|a| a == ARG_MINIMIZED);
    if silent_start {
        info!("Silent start (--minimized): window will be hidden to tray");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![ARG_MINIMIZED]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            get_autostart_status,
            set_autostart,
            set_locale
        ])
        .setup(move |app| {
            // Resolve data directory using Tauri's path API:
            //   macOS: ~/Library/Application Support/im.xrl.router/
            //   Linux: ~/.config/im.xrl.router/
            //   Windows: C:\Users\<user>\AppData\Roaming\im.xrl.router\
            let data_dir = app.path().app_data_dir()
                .expect("无法获取应用数据目录");
            std::fs::create_dir_all(&data_dir).ok();
            info!("Data directory: {}", data_dir.display());

            // 静默启动（开机自启 --minimized）：窗口隐藏到托盘，网关照常运行。
            // setup 在窗口首次绘制前执行，hide 无闪烁。
            if silent_start {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                    info!("Window hidden (silent start)");
                }
            }

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
            app.manage(app_state.clone());

            // Pass Tauri AppHandle to PluginManager so it can emit events to frontend
            app_state.plugins.set_app_handle(app.handle().clone());

            // System tray: keep the gateway alive when the window is closed.
            // 菜单文本按持久化 locale 初始化（前端切换语言时经 set_locale 更新）。
            let locale = database
                .get_setting("locale")
                .ok()
                .flatten()
                .unwrap_or_else(|| "zh-CN".to_string());
            let (show_txt, quit_txt) = tray_texts(&locale);
            let show_item =
                tauri::menu::MenuItem::with_id(app, "show", show_txt, true, None::<&str>)?;
            let quit_item =
                tauri::menu::MenuItem::with_id(app, "quit", quit_txt, true, None::<&str>)?;
            app.manage(TrayMenuState {
                show_item: show_item.clone(),
                quit_item: quit_item.clone(),
            });
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
