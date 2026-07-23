use crate::state::{get_app_state, get_champions_data, get_summoner_spells_data};
use crate::structs::{GameState, Settings};
use crate::ui::update_ui;
use crate::updater;
use tauri::{AppHandle, Emitter};
use tauri_plugin_autostart::ManagerExt;

#[tauri::command]
pub fn get_autostart_state(app: AppHandle) -> bool {
    let enabled = app.autolaunch().is_enabled().unwrap_or(false);
    eprintln!("[autostart] get_autostart_state -> {enabled}");
    enabled
}

#[tauri::command]
pub fn toggle_autostart(enabled: bool, app: AppHandle) -> Result<(), String> {
    eprintln!("[autostart] toggle_autostart({enabled})");
    if enabled {
        app.autolaunch().enable().map_err(|e| format!("Failed to enable autostart: {e}"))
    } else {
        app.autolaunch().disable().map_err(|e| format!("Failed to disable autostart: {e}"))
    }
}

#[tauri::command]
pub async fn frontend_ready(app_handle: AppHandle) {
    updater::check_for_updates(app_handle).await;
}

#[tauri::command]
pub async fn get_champions_and_spells() -> Result<serde_json::Value, String> {
    // `get_champions_data()` now hands back a read-lock guard rather than a
    // &'static ref (the champion list is refreshable from the LCU), so keep the
    // guards alive in locals for the duration of the serialization.
    let champions = match get_champions_data().await {
        Ok(data) => data,
        Err(_) => return Ok(serde_json::json!({"champions": [], "summonerSpells": []})),
    };

    let spells = match get_summoner_spells_data().await {
        Ok(data) => data,
        Err(_) => return Ok(serde_json::json!({"champions": [], "summonerSpells": []})),
    };

    Ok(serde_json::json!({
        "champions": &champions.array,
        "summonerSpells": &spells.array
    }))
}

#[tauri::command]
pub async fn get_current_game_state() -> Result<GameState, String> {
    let game_state = get_app_state().get_game_state().await;
    Ok(GameState {
        is_league_running: game_state.is_league_running,
        connection_status: game_state.connection_status.clone(),
        gameflow_status: game_state.gameflow_status.clone(),
        assigned_role: game_state.assigned_role.clone(),
        game_mode: game_state.game_mode.clone(),
        settings: Settings {
            auto_accept: game_state.settings.auto_accept,
            pick_ban_selection: game_state.settings.pick_ban_selection,
            auto_bravery: game_state.settings.auto_bravery,
            spell_selection: game_state.settings.spell_selection,
            selected_spell1: game_state.settings.selected_spell1.clone(),
            selected_spell2: game_state.settings.selected_spell2.clone(),
            champion_picks: game_state.settings.champion_picks.clone(),
            champion_ban: game_state.settings.champion_ban.clone(),
            close_to_tray: game_state.settings.close_to_tray,
            start_minimized: game_state.settings.start_minimized,
            docker_mode: game_state.settings.docker_mode,
            docker_standalone_when_closed: game_state.settings.docker_standalone_when_closed,
        },
    })
}

#[tauri::command]
pub async fn clear_picks_bans(app_handle: AppHandle) -> Result<(), String> {
    {
        let mut game_state = get_app_state().get_game_state_mut().await;
        game_state.settings.champion_picks.clear();
        game_state.settings.champion_ban = None;
    }
    let game_state = get_app_state().get_game_state().await;
    update_ui(&app_handle, &game_state).await;
    Ok(())
}

#[tauri::command]
pub async fn update_checkbox(
    app_handle: AppHandle,
    id: String,
    checked: bool,
) -> Result<(), String> {
    {
        let mut game_state = get_app_state().get_game_state_mut().await;
        match id.as_str() {
            "auto-accept" => {
                game_state.settings.auto_accept = checked;
            }
            "pick-ban-selection" => {
                game_state.settings.pick_ban_selection = checked;
            }
            "spell-selection" => {
                game_state.settings.spell_selection = checked;
            }
            "auto-bravery" => {
                game_state.settings.auto_bravery = checked;
            }
            "close-to-tray" => {
                game_state.settings.close_to_tray = Some(checked);
            }
            "start-minimized" => {
                game_state.settings.start_minimized = Some(checked);
            }
            "docker-mode" => {
                game_state.settings.docker_mode = Some(checked);
            }
            "docker-standalone-when-closed" => {
                game_state.settings.docker_standalone_when_closed = Some(checked);
            }
            _ => return Err(format!("Unknown checkbox ID: {}", id)),
        }
    }
    // When the user opts into "Start Minimized to Tray", re-write the autostart
    // registry entry so it carries the "--autostart" launch arg. This matters for
    // users who enabled autostart with an older build that didn't pass the arg —
    // otherwise the OS would launch the bare exe and we couldn't detect the
    // startup launch. Only refresh when autostart is currently enabled, so we
    // never silently re-enable it ourselves.
    if id == "start-minimized" && checked {
        let manager = app_handle.autolaunch();
        if manager.is_enabled().unwrap_or(false) {
            let _ = manager.enable();
        }
    }
    // "Dock to League Client" reacts immediately to its toggle: start the
    // Win32 tracking loop on enable, or stop it + restore the standalone
    // window on disable. No-op on non-Windows hosts.
    if id == "docker-mode" {
        if checked {
            crate::docker::enable_docker(&app_handle);
        } else {
            crate::docker::disable_docker(&app_handle);
        }
    }
    let game_state = get_app_state().get_game_state().await;
    update_ui(&app_handle, &game_state).await;
    Ok(())
}

#[tauri::command]
pub async fn update_pick_ban_text(
    app_handle: AppHandle,
    r#type: String,
    text: String,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let champions = get_champions_data().await?;
    let normalized_input = text
        .trim()
        .to_lowercase()
        .replace(|c: char| !c.is_ascii_alphanumeric(), "");
    let champion = champions
        .name_index
        .get(&normalized_input)
        .or_else(|| champions.name_index.get(&text.trim().to_lowercase()))
        .cloned();
    if let Some(champion) = champion {
        {
            let mut game_state = get_app_state().get_game_state_mut().await;
            if r#type == "pick" {
                let already_exists = game_state
                    .settings
                    .champion_picks
                    .iter()
                    .any(|p| p.id == champion.id);
                let is_banned = game_state
                    .settings
                    .champion_ban
                    .as_ref()
                    .map_or(false, |b| b.id == champion.id);
                if !already_exists && !is_banned && game_state.settings.champion_picks.len() < 5 {
                    game_state.settings.champion_picks.push(champion);
                }
            } else if r#type == "ban" {
                let already_picked = game_state
                    .settings
                    .champion_picks
                    .iter()
                    .any(|p| p.id == champion.id);
                if !already_picked {
                    game_state.settings.champion_ban = Some(champion);
                }
            }
        }
    }
    let game_state = get_app_state().get_game_state().await;
    update_ui(&app_handle, &game_state).await;
    Ok(())
}

#[tauri::command]
pub async fn update_selected_spell(
    app_handle: AppHandle,
    spell_slot: u32,
    spell_name: String,
) -> Result<(), String> {
    if spell_name.is_empty() || (spell_slot != 1 && spell_slot != 2) {
        return Ok(());
    }
    let spells = get_summoner_spells_data().await?;
    let spell = spells.name_index.get(&spell_name.to_lowercase()).cloned();

    if let Some(spell) = spell {
        {
            let mut game_state = get_app_state().get_game_state_mut().await;
            if spell_slot == 1 {
                game_state.settings.selected_spell1 = Some(spell.id);
            } else {
                game_state.settings.selected_spell2 = Some(spell.id);
            }
        }
        let game_state = get_app_state().get_game_state().await;
        update_ui(&app_handle, &game_state).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_champion_pick(app_handle: AppHandle, champion_id: i32) -> Result<(), String> {
    {
        let mut game_state = get_app_state().get_game_state_mut().await;
        game_state
            .settings
            .champion_picks
            .retain(|c| c.id != champion_id);
    }
    let game_state = get_app_state().get_game_state().await;
    update_ui(&app_handle, &game_state).await;
    Ok(())
}

#[tauri::command]
pub async fn reorder_champion_picks(
    app_handle: AppHandle,
    from_index: usize,
    to_index: usize,
) -> Result<(), String> {
    {
        let mut game_state = get_app_state().get_game_state_mut().await;
        let picks = &mut game_state.settings.champion_picks;

        if from_index < picks.len() && to_index < picks.len() {
            let item = picks.remove(from_index);
            picks.insert(to_index, item);
        }
    }
    let game_state = get_app_state().get_game_state().await;
    update_ui(&app_handle, &game_state).await;
    Ok(())
}

#[tauri::command]
pub async fn remove_champion_ban(app_handle: AppHandle) -> Result<(), String> {
    {
        let mut game_state = get_app_state().get_game_state_mut().await;
        game_state.settings.champion_ban = None;
    }
    let game_state = get_app_state().get_game_state().await;
    update_ui(&app_handle, &game_state).await;
    Ok(())
}

#[cfg(debug_assertions)]
#[tauri::command]
pub fn test_update(app_handle: AppHandle) -> Result<(), String> {
    let _ = app_handle.emit(
        "update-available",
        serde_json::json!({
            "version": "9.9.9",
            "notes": "Test update.\n- Mock change 1\n- Mock change 2",
            "url": "https://example.com/test-update.exe"
        }),
    );
    Ok(())
}
