mod commands;
mod connection;
mod constants;
mod events;
mod state;
mod structs;
mod ui;

pub use commands::*;
pub use constants::*;
pub use state::{get_app_state, get_champions_data, get_summoner_spells_data};
pub use structs::*;
pub use ui::*;

use tauri::{image::Image, Listener, Manager};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

#[allow(dead_code, unused_variables)]
pub fn run() {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let hide = MenuItemBuilder::with_id("hide", "Hide App").build(app)?;
            let show = MenuItemBuilder::with_id("show", "Show App").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&show)
                .item(&hide)
                .separator()
                .item(&quit)
                .build()?;
            let tray_icon = Image::from_path("icons/icon.png")?;
            let _tray = TrayIconBuilder::with_id("main")
                .icon(tray_icon.clone())
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    let app_handle = app.app_handle().clone();
                    match event.id().as_ref() {
                        "quit" => {
                            std::process::exit(0);
                        }
                        "hide" => {
                            let _ = app_handle.get_webview_window("main").map(|w| w.hide());
                        }
                        "show" => {
                            let _ = app_handle.get_webview_window("main").map(|w| w.show());
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|app, event| {
                    match event {
                        // Handle left click to show/hide window
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } => {
                            let app_handle = app.app_handle().clone();
                            if let Some(window) = app_handle.get_webview_window("main") {
                                if window.is_visible().unwrap_or(false) {
                                    let _ = window.hide();
                                } else {
                                    let _ = window.show();
                                }
                            }
                        }
                        // Right click will automatically show the context menu
                        TrayIconEvent::Click {
                            button: MouseButton::Right,
                            button_state: MouseButtonState::Up,
                            ..
                        } => {
                            // Menu will show automatically - no need to handle this explicitly
                        }
                        // Ignore all other events
                        _ => {}
                    }
                })
                .build(app)?;

            let window = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Watcher")
            .icon(tray_icon.clone())?
            .inner_size(370.0, 600.0)
            .min_inner_size(370.0, 600.0)
            .build()?;

            let app_handle_clone = app.app_handle().clone();
            window.once("tauri://created", move |_| {
                let app_handle = app_handle_clone.clone();
                tauri::async_runtime::spawn(async move {
                    // Re-emit initial data
                    let _champions_result = get_champions_data().await;
                    let _spells_result = get_summoner_spells_data().await;
                    let _champions_array = match get_champions_data().await {
                        Ok(champions_data) => &champions_data.array,
                        Err(_) => return,
                    };
                    let _spells_array = match get_summoner_spells_data().await {
                        Ok(spells_data) => &spells_data.array,
                        Err(_) => return,
                    };
                });
            });

            #[cfg(debug_assertions)]
            {
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
            }

            let (event_tx, _event_rx) = mpsc::channel::<ConnectionEvent>(32);
            let connection_manager = ConnectionManager::new(event_tx);
            app.manage(connection_manager);

            let app_handle = app.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                // Wait a bit for the UI to be ready
                sleep(Duration::from_millis(1000)).await;

                let manager = app_handle.state::<ConnectionManager>();
                let is_league_running = manager.check_league_process().await;

                // Set initial game state
                {
                    let mut game_state = get_app_state().get_game_state_mut().await;
                    game_state.is_league_running = is_league_running;
                    if is_league_running {
                        game_state.connection_status = "League Client detected".to_string();
                        game_state.gameflow_status = "Connecting to League Client...".to_string();
                    } else {
                        game_state.connection_status = "Waiting for League Client...".to_string();
                        game_state.gameflow_status = "Waiting for League Client...".to_string();
                    }
                }
                let game_state = get_app_state().get_game_state().await;
                update_ui(&app_handle, &game_state).await;

                manager.start(app_handle.clone()).await;

                let app_handle_tray = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        sleep(Duration::from_secs(5)).await;
                        let current_game_state = get_app_state().get_game_state().await;
                        let tray_settings = TraySettings {
                            auto_accept: current_game_state.settings.auto_accept,
                            pick_ban_selection: current_game_state.settings.pick_ban_selection,
                            spell_selection: current_game_state.settings.spell_selection,
                        };
                        let _ = update_tray_tooltip(
                            app_handle_tray.clone(),
                            &current_game_state.connection_status,
                            &current_game_state.gameflow_status,
                            tray_settings,
                        )
                        .await;
                    }
                });

                let _app_handle_clone = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        sleep(Duration::from_secs(30)).await;
                        let mut champion_cache = get_app_state().get_champion_cache().await;
                        champion_cache.cleanup_expired();
                        // Champion cache cleanup is handled internally
                        drop(champion_cache);
                    }
                });
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            clear_picks_bans,
            update_checkbox,
            update_pick_ban_text,
            update_selected_spell,
            remove_champion_pick,
            reorder_champion_picks,
            hide_app,
            update_tray_tooltip,
            get_champions_and_spells,
            get_current_game_state
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
