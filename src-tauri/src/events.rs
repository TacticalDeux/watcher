use crate::state::get_app_state;
use crate::structs::*;
use irelia::ws::types::Event;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tauri::async_runtime::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio::time::timeout;

impl EventProcessor {
    pub fn new(event_rx: mpsc::UnboundedReceiver<Event>, app_handle: AppHandle) -> Self {
        Self {
            event_rx,
            app_handle,
            throttle_map: std::sync::Arc::new(Mutex::new(HashMap::new())),
            app_state: get_app_state(),
            ban_completed_this_phase: std::sync::Arc::new(Mutex::new(false)),
        }
    }

    pub async fn run(&mut self) {
        let mut batch = Vec::with_capacity(20);
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        let mut health_check_interval = tokio::time::interval(Duration::from_secs(5));

        loop {
            tokio::select! {
                event = self.event_rx.recv() => {
                    if let Some(event) = event {
                        batch.push(event);
                        // Process immediately if batch is full
                        if batch.len() >= 10 {
                            self.process_batch(&mut batch).await;
                        }
                    } else {
                        // This will be handled by the connection manager
                        break;
                    }
                }
                // Process remaining events periodically
                _ = interval.tick() => {
                    if !batch.is_empty() {
                        self.process_batch(&mut batch).await;
                    }
                }
                _ = health_check_interval.tick() => {
                    if let Err(e) = self.check_websocket_health().await {
                        eprintln!("WebSocket health check failed: {}", e);
                    }
                }
            }
        }
    }

    async fn process_batch(&self, batch: &mut Vec<Event>) {
        for event in batch.drain(..) {
            if let Err(e) = self.process_event(event).await {
                eprintln!("Error processing event: {}", e);
            }
        }
    }

    async fn process_event(&self, event: Event) -> Result<(), String> {
        let event_json = serde_json::to_value(&event)
            .map_err(|e| format!("Failed to serialize event: {}", e))?;

        if let Some(event_data) = event_json
            .as_array()
            .and_then(|arr| arr.get(2))
            .and_then(|obj| obj.as_object())
        {
            if let Some(uri) = event_data.get("uri").and_then(|v| v.as_str()) {
                // Throttle events by URI
                if !self.should_process_event(uri).await {
                    return Ok(());
                }

                match uri {
                    uri if uri.contains("/lol-gameflow/v1/gameflow-phase") => {
                        // Process immediately and emit directly to UI
                        if let Some(data) = event_data.get("data").and_then(|v| v.as_str()) {
                            {
                                let mut game_state = get_app_state().get_game_state_mut().await;
                                game_state.gameflow_status = data.to_string();
                            }

                            // Direct UI emission bypassing full update cycle
                            if let Some(window) = self.app_handle.get_webview_window("main") {
                                let mut changes = serde_json::Map::new();
                                changes.insert(
                                    "gameflowStatus".to_string(),
                                    serde_json::Value::String(data.to_string()),
                                );
                                let _ = window.emit("status-update", changes);
                            }
                        }
                        return Ok(());
                    }
                    uri if uri.contains("/lol-gameflow/v1/session") => {
                        self.handle_gameflow_session(event_data).await?;
                    }
                    uri if uri.contains("/lol-matchmaking/v1/ready-check") => {
                        self.handle_ready_check(event_data).await?;
                    }
                    uri if uri.contains("/lol-champ-select/v1/session") => {
                        self.handle_champion_select(event_data).await?;
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    // async fn handle_gameflow_phase(
    //     &self,
    //     event_data: &serde_json::Map<String, Value>,
    // ) -> Result<(), String> {
    //     if let Some(data) = event_data.get("data").and_then(|v| v.as_str()) {
    //         let mut game_state = get_app_state().get_game_state_mut().await;
    //         let previous_phase = game_state.gameflow_status.clone();
    //         game_state.gameflow_status = data.to_string();

    //         // Handle phase transitions
    //         match data {
    //             "ChampSelect" => {
    //                 if previous_phase != "ChampSelect" {
    //                     *self.ban_completed_this_phase.lock().await = false;
    //                 }
    //             }
    //             "InProgress" => {
    //                 // Currently unimplemented
    //             }
    //             "WaitingForStats" => {
    //                 // Currently unimplemented
    //             }
    //             "PreEndOfGame" => {
    //                 // Currently unimplemented
    //             }
    //             _ => {}
    //         }

    //         drop(game_state);
    //         self.update_ui().await?;
    //     }
    //     Ok(())
    // }

    async fn handle_gameflow_session(
        &self,
        event_data: &serde_json::Map<String, Value>,
    ) -> Result<(), String> {
        if let Some(data) = event_data.get("data").and_then(|v| v.as_object()) {
            if let Some(phase) = data.get("phase").and_then(|v| v.as_str()) {
                let mut game_state = get_app_state().get_game_state_mut().await;
                let previous_phase = game_state.gameflow_status.clone();
                game_state.gameflow_status = phase.to_string();

                match phase {
                    "ChampSelect" => {
                        if previous_phase != "ChampSelect" {
                            *self.ban_completed_this_phase.lock().await = false;
                        }
                    }
                    _ => {}
                }

                drop(game_state);
                self.update_ui().await?;
            }
        }
        Ok(())
    }

    async fn handle_ready_check(
        &self,
        event_data: &serde_json::Map<String, Value>,
    ) -> Result<(), String> {
        if let Some(data) = event_data.get("data").and_then(|v| v.as_object()) {
            if let Some(state) = data.get("state").and_then(|v| v.as_str()) {
                if state == "InProgress" {
                    let game_state = get_app_state().get_game_state().await;
                    if game_state.settings.auto_accept {
                        drop(game_state);
                        self.auto_accept_match().await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_champion_select(
        &self,
        event_data: &serde_json::Map<String, Value>,
    ) -> Result<(), String> {
        if let Some(data) = event_data.get("data").and_then(|v| v.as_object()) {
            if let Some(actions) = data.get("actions").and_then(|v| v.as_array()) {
                self.process_champion_select_actions(actions).await?;
            }

            // Handle role assignment
            if let Some(my_team) = data.get("myTeam").and_then(|v| v.as_array()) {
                self.handle_role_assignment(my_team).await?;
            }
        }
        Ok(())
    }

    async fn process_champion_select_actions(&self, actions: &Vec<Value>) -> Result<(), String> {
        for action_group in actions {
            if let Some(action_array) = action_group.as_array() {
                for action in action_array {
                    if let Some(action_obj) = action.as_object() {
                        if let (Some(_actor_cell_id), Some(is_in_progress), Some(action_type)) = (
                            action_obj.get("actorCellId").and_then(|v| v.as_i64()),
                            action_obj.get("isInProgress").and_then(|v| v.as_bool()),
                            action_obj.get("type").and_then(|v| v.as_str()),
                        ) {
                            if is_in_progress {
                                match action_type {
                                    "pick" => {
                                        self.handle_pick(action_obj).await?;
                                    }
                                    "ban" => {
                                        self.handle_ban(action_obj).await?;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_role_assignment(&self, my_team: &Vec<Value>) -> Result<(), String> {
        // Find our summoner in the team
        for member in my_team {
            if let Some(member_obj) = member.as_object() {
                if let Some(assigned_position) =
                    member_obj.get("assignedPosition").and_then(|v| v.as_str())
                {
                    let mut game_state = get_app_state().get_game_state_mut().await;
                    game_state.assigned_role = assigned_position.to_string();
                    drop(game_state);
                    break;
                }
            }
        }
        Ok(())
    }

    async fn handle_pick(&self, action: &serde_json::Map<String, Value>) -> Result<(), String> {
        let game_state = get_app_state().get_game_state().await;
        if !game_state.settings.pick_ban_selection {
            return Ok(());
        }

        let champion_picks = &game_state.settings.champion_picks;
        if champion_picks.is_empty() {
            return Ok(());
        }

        // Try to pick the first available champion from the list
        for champion in champion_picks {
            if self.is_champion_available(champion.id).await? {
                if let Some(client) = self.get_lcu_client().await {
                    let pick_data = serde_json::json!({
                        "actorCellId": action.get("actorCellId"),
                        "championId": champion.id,
                        "completed": true,
                        "id": action.get("id"),
                        "isAllyAction": action.get("isAllyAction"),
                        "type": "pick"
                    });

                    if let Some(action_id) = action.get("id").and_then(|v| v.as_i64()) {
                        let endpoint =
                            format!("/lol-champ-select/v1/session/actions/{}", action_id);
                        let _: Result<serde_json::Value, _> =
                            client.patch(&endpoint, Some(pick_data)).await;
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_ban(&self, action: &serde_json::Map<String, Value>) -> Result<(), String> {
        let ban_completed = *self.ban_completed_this_phase.lock().await;
        if ban_completed {
            return Ok(());
        }

        let game_state = get_app_state().get_game_state().await;
        if !game_state.settings.pick_ban_selection {
            return Ok(());
        }

        if let Some(ban_champion) = &game_state.settings.champion_ban {
            if let Some(client) = self.get_lcu_client().await {
                let ban_data = serde_json::json!({
                    "actorCellId": action.get("actorCellId"),
                    "championId": ban_champion.id,
                    "completed": true,
                    "id": action.get("id"),
                    "isAllyAction": action.get("isAllyAction"),
                    "type": "ban"
                });

                if let Some(action_id) = action.get("id").and_then(|v| v.as_i64()) {
                    let endpoint = format!("/lol-champ-select/v1/session/actions/{}", action_id);
                    let result: Result<serde_json::Value, _> =
                        client.patch(&endpoint, Some(ban_data)).await;
                    if result.is_ok() {
                        *self.ban_completed_this_phase.lock().await = true;
                    }
                }
            }
        }

        Ok(())
    }

    async fn auto_accept_match(&self) -> Result<(), String> {
        if let Some(client) = self.get_lcu_client().await {
            let accept_data = serde_json::json!({});
            let _: Result<serde_json::Value, _> = client
                .post("/lol-matchmaking/v1/ready-check/accept", Some(accept_data))
                .await;
        }
        Ok(())
    }

    async fn is_champion_available(&self, champion_id: u32) -> Result<bool, String> {
        if let Some(client) = self.get_lcu_client().await {
            match timeout(Duration::from_secs(2), async {
                client
                    .get::<Value>("/lol-champ-select/v1/pickable-champion-ids")
                    .await
            })
            .await
            {
                Ok(Ok(response)) => {
                    if let Some(pickable_ids) = response.as_array() {
                        return Ok(pickable_ids
                            .iter()
                            .any(|id| id.as_u64().map(|id| id as u32) == Some(champion_id)));
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }

    async fn get_lcu_client(
        &self,
    ) -> Option<irelia::rest::LcuClient<irelia::requests::RequestClientType>> {
        match irelia::rest::LcuClient::connect() {
            Ok(client) => Some(client),
            Err(_) => None,
        }
    }

    async fn check_websocket_health(&self) -> Result<(), String> {
        // Health check implementation
        Ok(())
    }

    async fn should_process_event(&self, uri: &str) -> bool {
        if uri.contains("/lol-gameflow/v1/gameflow-phase") {
            return true;
        }

        let now = chrono::Utc::now().timestamp_millis() as u64;
        let mut throttle_map = self.throttle_map.lock().await;

        if let Some(&last_time) = throttle_map.get(uri) {
            if now - last_time < 100 {
                // 100ms throttle
                return false;
            }
        }

        throttle_map.insert(uri.to_string(), now);
        true
    }

    async fn update_ui(&self) -> Result<(), String> {
        let game_state = get_app_state().get_game_state().await;
        let _ = self.app_handle.emit("game-state-update", &*game_state);
        Ok(())
    }
}
