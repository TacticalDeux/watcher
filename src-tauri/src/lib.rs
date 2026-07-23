mod commands;
mod connection;
mod constants;
mod docker;
mod events;
mod state;
mod structs;
mod ui;
mod updater;

pub use commands::*;
pub use constants::*;
pub use state::{get_app_state, get_champions_data, get_summoner_spells_data, load_persisted_settings};
pub use structs::*;
pub use ui::*;
pub use updater::*;

use tauri::{image::Image, Emitter, Listener, Manager};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

#[allow(dead_code, unused_variables)]
pub fn run() {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            // The OS passes "--autostart" back to us when it auto-launches the
            // app at system startup, so we can tell that launch apart from a
            // manual one (and honor "Start Minimized to Tray").
            Some(vec!["--autostart"]),
        ))
        .setup(|app| {
            tauri::async_runtime::spawn(async {
                let temp_dir = std::env::temp_dir();
                if let Ok(mut entries) = tokio::fs::read_dir(temp_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        if let Some(file_name) = entry.file_name().to_str() {
                            if (file_name.starts_with("watcher")
                                && file_name.ends_with("_x64-setup.exe"))
                                || file_name == "watcher-update.exe"
                            {
                                let _ = tokio::fs::remove_file(entry.path()).await;
                            }
                        }
                    }
                }
            });

            let check_for_updates =
                MenuItemBuilder::with_id("check_for_updates", "Check for Updates").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let hide = MenuItemBuilder::with_id("hide", "Hide App").build(app)?;
            let show = MenuItemBuilder::with_id("show", "Show App").build(app)?;
            let toggle_dock = MenuItemBuilder::with_id("toggle_dock", "Dock to League").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&show)
                .item(&hide)
                .separator()
                .item(&toggle_dock)
                .separator()
                .item(&check_for_updates)
                .separator()
                .item(&quit)
                .build()?;
            let resource_dir = app.path().resource_dir()?;
            let icon_path = resource_dir.join("icons/icon.png");
            let tray_icon = Image::from_path(&icon_path)?;
            let _tray = TrayIconBuilder::with_id("main")
                .icon(tray_icon.clone())
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    let app_handle = app.app_handle().clone();
                    match event.id().as_ref() {
                        "quit" => {
                            app_handle.exit(0);
                        }
                        "hide" => {
                            let _ = app_handle.get_webview_window("main").map(|w| w.hide());
                        }
                        "show" => {
                            let _ = app_handle.get_webview_window("main").map(|w| w.show());
                        }
                        "check_for_updates" => {
                            let app_handle_clone = app_handle.clone();
                            tauri::async_runtime::spawn(async move {
                                app_handle_clone.emit("checking-for-updates", ()).unwrap();
                                if let Err(e) =
                                    updater::perform_update_check(&app_handle_clone).await
                                {
                                    eprintln!("Update check failed: {}", e);
                                }
                            });
                        }
                        "toggle_dock" => {
                            let app_handle = app_handle.clone();
                            tauri::async_runtime::spawn(async move {
                                let docked = {
                                    get_app_state()
                                        .get_game_state()
                                        .await
                                        .settings
                                        .docker_mode
                                        == Some(true)
                                };
                                {
                                    let mut gs = get_app_state()
                                        .get_game_state_mut()
                                        .await;
                                    gs.settings.docker_mode = Some(!docked);
                                }
                                if docked {
                                    crate::docker::disable_docker(&app_handle);
                                } else {
                                    crate::docker::enable_docker(&app_handle);
                                }
                                let gs = get_app_state().get_game_state().await;
                                update_ui(&app_handle, &gs).await;
                            });
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
                                    let _ = window.set_focus();
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

            // When the OS auto-launches us at startup it passes "--autostart". In
            // that case — and only if the user also opted into "Start Minimized to
            // Tray" — skip showing the window and stay in the tray instead.
            let launched_at_startup = std::env::args().any(|a| a == "--autostart");
            let start_minimized = launched_at_startup
                && load_persisted_settings()
                    .map(|s| {
                        eprintln!(
                            "[start-minimized] loaded: {:?}, evaluating to {}",
                            s.start_minimized,
                            s.start_minimized == Some(true),
                        );
                        s.start_minimized == Some(true)
                    })
                    .unwrap_or_else(|| {
                        eprintln!("[start-minimized] no persisted settings found");
                        false
                    });
            eprintln!(
                "[start-minimized] launched_at_startup={launched_at_startup}, start_minimized={start_minimized}",
            );

            let window = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Watcher")
            .icon(tray_icon.clone())?
            .inner_size(370.0, 600.0)
            .min_inner_size(370.0, 600.0)
            .visible(!start_minimized)
            .build()?;

            // Intercept the window close button so we can ask the user whether
            // to actually quit or minimize to tray instead.
            let close_handle = app.app_handle().clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if let Some(win) = close_handle.get_webview_window("main") {
                        let _ = win.emit("close-requested", ());
                    }
                }
            });

            let app_handle_clone = app.app_handle().clone();
            window.once("tauri://created", move |_| {
                let app_handle = app_handle_clone.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = get_summoner_spells_data().await;
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
            app.manage(docker::DockerState::default());

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

                // Restore persisted settings (close-to-tray, toggles, picks/bans).
                if let Some(saved) = load_persisted_settings() {
                    let want_docker = saved.docker_mode == Some(true);
                    let mut game_state = get_app_state().get_game_state_mut().await;
                    game_state.settings = saved;
                    drop(game_state);
                    // "Dock to League Client" restored on: spawn the tracker so it
                    // docks as soon as the League client window appears. Harmless
                    // when the client isn't running yet (the tracker stays hidden).
                    if want_docker {
                        docker::enable_docker(&app_handle);
                    }
                }

                let game_state = get_app_state().get_game_state().await;
                update_ui(&app_handle, &game_state).await;
                // Tell the renderer we have fully loaded state (including persisted
                // settings) so it can safely call get_game_state().
                let _ = app_handle.emit("state-ready", ());

                manager.start(app_handle.clone()).await;

                let app_handle_tray = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        sleep(Duration::from_secs(5)).await;
                        let current_game_state = get_app_state().get_game_state().await;
                        let tray_settings = TraySettings {
                            auto_accept: current_game_state.settings.auto_accept,
                            pick_ban_selection: current_game_state.settings.pick_ban_selection,
                            auto_bravery: current_game_state.settings.auto_bravery,
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
            remove_champion_ban,
            reorder_champion_picks,
            hide_app,
            close_app,
            update_tray_tooltip,
            get_champions_and_spells,
            get_current_game_state,
            run_updater,
            frontend_ready,
            get_autostart_state,
            toggle_autostart,
            #[cfg(debug_assertions)]
            test_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
