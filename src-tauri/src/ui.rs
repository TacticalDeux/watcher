use crate::state::get_app_state;
use crate::structs::{GameState, Settings, TraySettings};
use tauri::{AppHandle, Emitter, Manager};

pub fn show_app(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
    }
}

#[tauri::command]
pub fn hide_app(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[tauri::command]
pub async fn update_tray_tooltip(
    app_handle: AppHandle,
    _connection_status: &str,
    gameflow_status: &str,
    settings: TraySettings,
) -> Result<(), String> {
    let mut active_settings = Vec::new();
    if settings.auto_accept {
        active_settings.push("Accept");
    }
    if settings.pick_ban_selection {
        active_settings.push("Pick/Ban");
    }
    if settings.spell_selection {
        active_settings.push("Spells");
    }
    let settings_text = if active_settings.is_empty() {
        "None".to_string()
    } else {
        active_settings.join("\n")
    };
    let tooltip = format!("{}\nActive:\n{}", gameflow_status, settings_text);
    if let Some(tray) = app_handle.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(&tooltip));
    }
    Ok(())
}

pub async fn update_ui(app_handle: &AppHandle, current_game_state: &GameState) {
    // Update the main window
    if let Some(window) = app_handle.get_webview_window("main") {
        {
            let mut last_state = get_app_state().get_last_game_state_mut().await;
            let mut changes = serde_json::Map::new();
            if current_game_state.is_league_running != last_state.is_league_running {
                changes.insert(
                    "isLeagueRunning".to_string(),
                    serde_json::to_value(current_game_state.is_league_running).unwrap(),
                );
            }

            if current_game_state.connection_status != last_state.connection_status {
                changes.insert(
                    "connectionStatus".to_string(),
                    serde_json::to_value(&current_game_state.connection_status).unwrap(),
                );
            }

            if current_game_state.gameflow_status != last_state.gameflow_status {
                changes.insert(
                    "gameflowStatus".to_string(),
                    serde_json::to_value(&current_game_state.gameflow_status).unwrap(),
                );
            }

            if current_game_state.assigned_role != last_state.assigned_role {
                changes.insert(
                    "assignedRole".to_string(),
                    serde_json::to_value(&current_game_state.assigned_role).unwrap(),
                );
            }

            if serde_json::to_string(&current_game_state.settings).unwrap()
                != serde_json::to_string(&last_state.settings).unwrap()
            {
                changes.insert(
                    "settings".to_string(),
                    serde_json::to_value(&current_game_state.settings).unwrap(),
                );
            }

            if !changes.is_empty() {
                let _ = window.emit("status-update", changes);
            }
            *last_state = current_game_state.clone();
        }
    }

    update_tray_tooltip_internal(
        app_handle,
        &current_game_state.connection_status,
        &current_game_state.gameflow_status,
        &current_game_state.settings,
    )
    .await;
}

pub async fn update_tray_tooltip_internal(
    app_handle: &AppHandle,
    _connection_status: &str,
    gameflow_status: &str,
    settings: &Settings,
) {
    let tooltip = {
        let tray_settings = TraySettings {
            auto_accept: settings.auto_accept,
            pick_ban_selection: settings.pick_ban_selection,
            spell_selection: settings.spell_selection,
        };

        let mut active_settings = Vec::new();
        if tray_settings.auto_accept {
            active_settings.push("Accept");
        }

        if tray_settings.pick_ban_selection {
            active_settings.push("Pick/Ban");
        }

        if tray_settings.spell_selection {
            active_settings.push("Spells");
        }

        let settings_text = if active_settings.is_empty() {
            "None".to_string()
        } else {
            active_settings.join("\n")
        };

        format!("{}\nActive:\n{}", gameflow_status, settings_text)
    };

    if let Some(tray) = app_handle.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(&tooltip));
    }
}
