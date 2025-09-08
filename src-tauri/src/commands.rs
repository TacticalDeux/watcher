use crate::state::{get_app_state, get_champions_data, get_summoner_spells_data};
use crate::structs::{GameState, Settings};
use crate::ui::update_ui;
use tauri::AppHandle;

#[tauri::command]
pub async fn get_champions_and_spells() -> Result<serde_json::Value, String> {
    let champions_array = match get_champions_data().await {
        Ok(champions_data) => &champions_data.array,
        Err(_) => return Ok(serde_json::json!({"champions": [], "summonerSpells": []})),
    };

    let spells_array = match get_summoner_spells_data().await {
        Ok(spells_data) => &spells_data.array,
        Err(_) => return Ok(serde_json::json!({"champions": [], "summonerSpells": []})),
    };

    Ok(serde_json::json!({
        "champions": champions_array,
        "summonerSpells": spells_array
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
        settings: Settings {
            auto_accept: game_state.settings.auto_accept,
            pick_ban_selection: game_state.settings.pick_ban_selection,
            spell_selection: game_state.settings.spell_selection,
            selected_spell1: game_state.settings.selected_spell1.clone(),
            selected_spell2: game_state.settings.selected_spell2.clone(),
            champion_picks: game_state.settings.champion_picks.clone(),
            champion_ban: game_state.settings.champion_ban.clone(),
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
            _ => return Err(format!("Unknown checkbox ID: {}", id)),
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
pub async fn remove_champion_pick(app_handle: AppHandle, champion_id: u32) -> Result<(), String> {
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
