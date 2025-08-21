use crate::constants::*;
use crate::state::{get_app_state, update_game_state};
use crate::structs::*;
use crate::ui::update_ui;
use chrono::Utc;
use irelia::requests::RequestClientType;
use irelia::rest::LcuClient;
use irelia::ws::{types::Event, types::EventKind, LcuWebSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use sysinfo::System;
use tauri::async_runtime::{Mutex, RwLock};
use tauri::AppHandle;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};

impl ExponentialBackoff {
    pub fn new() -> Self {
        Self {
            current_delay: Duration::from_millis(1000),
            max_delay: Duration::from_millis(30000),
            multiplier: 1.5,
            base_delay: Duration::from_millis(1000),
        }
    }

    pub fn next_delay(&mut self) -> Duration {
        let delay = self.current_delay;
        self.current_delay = Duration::from_millis(
            ((self.current_delay.as_millis() as f64) * self.multiplier)
                .min(self.max_delay.as_millis() as f64) as u64,
        );
        delay
    }

    pub fn reset(&mut self) {
        self.current_delay = self.base_delay;
    }
}

impl ConnectionHealth {
    pub fn new() -> Self {
        Self {
            last_successful_request: Arc::new(AtomicU64::new(0)),
            consecutive_failures: Arc::new(AtomicU64::new(0)),
            is_healthy: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn mark_success(&self) {
        let now = Utc::now().timestamp_millis() as u64;
        self.last_successful_request.store(now, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.is_healthy.store(true, Ordering::Relaxed);
    }

    pub fn mark_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        if self.consecutive_failures.load(Ordering::Relaxed) > 3 {
            self.is_healthy.store(false, Ordering::Relaxed);
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.is_healthy.load(Ordering::Relaxed)
    }

    pub fn should_health_check(&self) -> bool {
        let now = Utc::now().timestamp_millis() as u64;
        let last_check = self.last_successful_request.load(Ordering::Relaxed);
        now - last_check > 30000 // 30 seconds
    }
}

impl ProcessMonitor {
    pub fn new() -> Self {
        Self {
            last_check: Arc::new(AtomicU64::new(0)),
            last_result: Arc::new(AtomicBool::new(false)),
            check_interval: Duration::from_secs(2),
        }
    }

    pub async fn is_league_running(&self) -> bool {
        let now = Utc::now().timestamp_millis() as u64;
        let last_check = self.last_check.load(Ordering::Relaxed);

        if now - last_check < self.check_interval.as_millis() as u64 {
            return self.last_result.load(Ordering::Relaxed);
        }

        let is_running = tauri::async_runtime::spawn_blocking(|| {
            let mut system = System::new_all();
            system.refresh_processes();
            let mut processes = system.processes_by_name("LeagueClientUx.exe");
            let next_process = processes.next();
            next_process.is_some()
        })
        .await
        .unwrap_or(false);

        self.last_check.store(now, Ordering::Relaxed);
        self.last_result.store(is_running, Ordering::Relaxed);
        is_running
    }
}

impl LeagueClientReadiness {
    pub fn new() -> Self {
        Self {
            process_start_time: Arc::new(AtomicU64::new(0)),
            is_ready: Arc::new(AtomicBool::new(false)),
            readiness_check_interval: Duration::from_millis(1000),
        }
    }

    pub fn mark_process_started(&self) {
        let now = Utc::now().timestamp_millis() as u64;
        self.process_start_time.store(now, Ordering::Relaxed);
        self.is_ready.store(false, Ordering::Relaxed);
    }

    pub async fn check_readiness(&self) -> bool {
        if self.is_ready.load(Ordering::Relaxed) {
            return true;
        }

        let start_time = self.process_start_time.load(Ordering::Relaxed);
        if start_time == 0 {
            return false;
        }

        let now = Utc::now().timestamp_millis() as u64;
        if now - start_time < self.readiness_check_interval.as_millis() as u64 {
            return false;
        }

        match LcuClient::connect() {
            Ok(client) => {
                // Try a simple request to verify the client is fully operational
                match timeout(Duration::from_secs(2), async {
                    client
                        .get::<serde_json::Value>("/lol-summoner/v1/current-summoner")
                        .await
                })
                .await
                {
                    Ok(Ok(_)) => {
                        self.is_ready.store(true, Ordering::Relaxed);
                        true
                    }
                    _ => false,
                }
            }
            Err(_) => false,
        }
    }

    pub fn reset(&self) {
        self.process_start_time.store(0, Ordering::Relaxed);
        self.is_ready.store(false, Ordering::Relaxed);
    }
}

impl ConnectionManager {
    pub fn new(_event_tx: mpsc::Sender<ConnectionEvent>) -> Self {
        let (internal_tx, internal_rx) = mpsc::unbounded_channel::<ConnectionEvent>();

        Self {
            connection_state: Arc::new(AtomicU8::new(STATE_DISCONNECTED)),
            lcu_client: Arc::new(RwLock::new(None)),
            lcu_websocket: Arc::new(RwLock::new(None)),
            internal_event_tx: internal_tx,
            internal_event_rx: Arc::new(Mutex::new(internal_rx)),
            connection_health: ConnectionHealth::new(),
            process_monitor: ProcessMonitor::new(),
            connection_task: Arc::new(Mutex::new(None)),
            event_task: Arc::new(Mutex::new(None)),
            monitor_task: Arc::new(Mutex::new(None)),
            last_connection_attempt: Arc::new(AtomicU64::new(0)),
            connection_attempt_cooldown: Duration::from_secs(5),
            client_readiness: LeagueClientReadiness::new(),
        }
    }

    pub async fn start(&self, app_handle: AppHandle) {
        // Start connection management loops
        self.start_connection_loop(app_handle.clone()).await;
        self.start_event_loop(app_handle.clone()).await;
        self.start_monitoring_loop(app_handle).await;
    }

    async fn start_connection_loop(&self, app_handle: AppHandle) {
        let connection_state = self.connection_state.clone();
        let process_monitor = self.process_monitor.clone();
        let connection_health = self.connection_health.clone();
        let lcu_client = self.lcu_client.clone();
        let lcu_websocket = self.lcu_websocket.clone();
        let internal_event_tx = self.internal_event_tx.clone();
        let last_connection_attempt = self.last_connection_attempt.clone();
        let connection_attempt_cooldown = self.connection_attempt_cooldown;
        let app_handle_clone = app_handle.clone();
        let client_readiness = self.client_readiness.clone();

        let task = tauri::async_runtime::spawn(async move {
            let mut backoff = ExponentialBackoff::new();
            let mut last_league_state = false;

            loop {
                let current_state = connection_state.load(Ordering::Relaxed);
                let is_league_running = process_monitor.is_league_running().await;

                // Update global game state with League process status
                {
                    update_game_state(|state| {
                        state.is_league_running = is_league_running;
                        if !is_league_running {
                            state.connection_status = "League Client not running".to_string();
                            state.gameflow_status = "Waiting for League Client...".to_string();
                            state.assigned_role = "".to_string();
                        }
                    })
                    .await;

                    let game_state = get_app_state().get_game_state().await;
                    update_ui(&app_handle_clone, &game_state).await;
                }

                // Check if League process state changed
                if is_league_running != last_league_state {
                    if is_league_running {
                        // League process started - mark the start time but don't connect yet
                        client_readiness.mark_process_started();
                        let _ = internal_event_tx.send(ConnectionEvent::LeagueProcessDetected);
                    } else {
                        // League process ended - force cleanup
                        let _ = internal_event_tx.send(ConnectionEvent::LeagueProcessLost);
                        connection_state.store(STATE_DISCONNECTED, Ordering::Relaxed);
                        client_readiness.reset();

                        // Clean up connection resources
                        {
                            let mut client_guard = lcu_client.write().await;
                            *client_guard = None;
                        }
                        {
                            let mut ws_guard = lcu_websocket.write().await;
                            if let Some((_, is_active)) = ws_guard.take() {
                                is_active.store(false, Ordering::Relaxed);
                            }
                        }
                    }
                    last_league_state = is_league_running;
                }

                match (current_state, is_league_running) {
                    // League not running - wait and check again
                    (_, false) => {
                        if current_state != STATE_DISCONNECTED {
                            connection_state.store(STATE_DISCONNECTED, Ordering::Relaxed);
                        }
                        sleep(Duration::from_secs(3)).await;
                        continue;
                    }
                    // League running but not connected - check if ready before attempting connection
                    (STATE_DISCONNECTED, true) => {
                        if !client_readiness.check_readiness().await {
                            // Update UI to show we're waiting for client to be ready
                            {
                                update_game_state(|state| {
                                    state.connection_status = "Waiting to connect...".to_string();
                                })
                                .await;

                                let game_state = get_app_state().get_game_state().await;
                                update_ui(&app_handle_clone, &game_state).await;
                            }
                            sleep(Duration::from_millis(500)).await;
                            continue;
                        }

                        // Rate limit connection attempts
                        let now = Utc::now().timestamp_millis() as u64;
                        let last_attempt = last_connection_attempt.load(Ordering::Relaxed);
                        if now - last_attempt < connection_attempt_cooldown.as_millis() as u64 {
                            sleep(Duration::from_millis(1000)).await;
                            continue;
                        }

                        last_connection_attempt.store(now, Ordering::Relaxed);
                        connection_state.store(STATE_CONNECTING, Ordering::Relaxed);

                        match Self::attempt_single_connection(
                            lcu_client.clone(),
                            lcu_websocket.clone(),
                            internal_event_tx.clone(),
                            app_handle_clone.clone(),
                        )
                        .await
                        {
                            Ok(_) => {
                                connection_state.store(STATE_CONNECTED, Ordering::Relaxed);
                                connection_health.mark_success();
                                backoff.reset();
                                let _ =
                                    internal_event_tx.send(ConnectionEvent::ConnectionEstablished);
                            }
                            Err(e) => {
                                eprintln!("Connection attempt failed: {}", e);
                                connection_state.store(STATE_DISCONNECTED, Ordering::Relaxed);
                                connection_health.mark_failure();

                                // If connection failed, reset readiness to force re-check
                                client_readiness.reset();

                                let delay = backoff.next_delay();
                                eprintln!("Waiting {:?} before retrying", delay);
                                sleep(delay).await;
                            }
                        }
                    }
                    (STATE_CONNECTING, true) => {
                        // Wait for connection attempt to complete
                        sleep(Duration::from_millis(1000)).await;
                    }
                    (STATE_CONNECTED, true) => {
                        // Perform periodic health check
                        if connection_health.should_health_check() {
                            if let Err(e) = Self::health_check(lcu_client.clone()).await {
                                eprintln!("Health check failed: {}", e);
                                connection_health.mark_failure();
                                if !connection_health.is_healthy() {
                                    connection_state.store(STATE_DISCONNECTED, Ordering::Relaxed);
                                    // Reset readiness to force re-check
                                    client_readiness.reset();
                                    let _ = internal_event_tx.send(ConnectionEvent::ConnectionLost);
                                }
                            } else {
                                connection_health.mark_success();
                            }
                        }
                        sleep(Duration::from_secs(10)).await;
                    }
                    (3_u8..=u8::MAX, true) => unimplemented!(),
                }
            }
        });

        *self.connection_task.lock().await = Some(task);
    }

    async fn start_event_loop(&self, app_handle: AppHandle) {
        let app_handle_clone = app_handle.clone();
        let event_rx = self.internal_event_rx.clone();

        let task = tauri::async_runtime::spawn(async move {
            let mut event_rx = event_rx.lock().await;
            while let Some(event) = event_rx.recv().await {
                match event {
                    ConnectionEvent::LeagueProcessDetected => {
                        update_game_state(|state| {
                            state.is_league_running = true;
                            state.connection_status = STATUS_LEAGUE_DETECTED.to_string();
                        })
                        .await;
                        let game_state = get_app_state().get_game_state().await;
                        update_ui(&app_handle_clone, &game_state).await;
                    }
                    ConnectionEvent::LeagueProcessLost => {
                        update_game_state(|state| {
                            state.is_league_running = false;
                            state.connection_status = STATUS_LEAGUE_CLOSED.to_string();
                            state.gameflow_status = ROLE_EMPTY.to_string();
                            state.assigned_role = ROLE_EMPTY.to_string();
                        })
                        .await;
                        let game_state = get_app_state().get_game_state().await;
                        update_ui(&app_handle_clone, &game_state).await;
                    }
                    ConnectionEvent::ConnectionEstablished => {
                        update_game_state(|state| {
                            state.connection_status = STATUS_CONNECTED.to_string();
                        })
                        .await;
                        let game_state = get_app_state().get_game_state().await;
                        update_ui(&app_handle_clone, &game_state).await;
                    }
                    ConnectionEvent::ConnectionLost => {
                        update_game_state(|state| {
                            state.connection_status = STATUS_CONNECTION_LOST.to_string();
                            state.gameflow_status = ROLE_EMPTY.to_string();
                            state.assigned_role = ROLE_EMPTY.to_string();
                        })
                        .await;
                        let game_state = get_app_state().get_game_state().await;
                        update_ui(&app_handle_clone, &game_state).await;
                    }
                    ConnectionEvent::HealthCheckFailed => {
                        // TODO: Handle health check failure
                    }
                    ConnectionEvent::ReconnectRequested => {
                        // TODO: Handle reconnect request
                    }
                }
            }
        });

        *self.event_task.lock().await = Some(task);
    }

    async fn start_monitoring_loop(&self, app_handle: AppHandle) {
        let process_monitor = self.process_monitor.clone();
        let internal_event_tx = self.internal_event_tx.clone();
        let _app_handle_clone = app_handle.clone();

        let task = tauri::async_runtime::spawn(async move {
            let mut last_league_state = false;
            loop {
                let is_league_running = process_monitor.is_league_running().await;

                if is_league_running != last_league_state {
                    let event = if is_league_running {
                        ConnectionEvent::LeagueProcessDetected
                    } else {
                        ConnectionEvent::LeagueProcessLost
                    };
                    let _ = internal_event_tx.send(event);
                    last_league_state = is_league_running;
                }
                sleep(Duration::from_secs(3)).await;
            }
        });

        *self.monitor_task.lock().await = Some(task);
    }

    async fn attempt_single_connection(
        lcu_client: Arc<RwLock<Option<LcuClient<RequestClientType>>>>,
        lcu_websocket: Arc<RwLock<Option<(LcuWebSocket, Arc<AtomicBool>)>>>,
        _event_tx: mpsc::UnboundedSender<ConnectionEvent>,
        app_handle: AppHandle,
    ) -> Result<(), String> {
        // Clean up any existing connections first
        {
            let mut ws_guard = lcu_websocket.write().await;
            if let Some((ws, is_active)) = ws_guard.take() {
                // Stop the WebSocket event processing and abort connection
                is_active.store(false, Ordering::Relaxed);
                let _ = ws.abort();
            }
        }
        {
            let mut client_guard = lcu_client.write().await;
            *client_guard = None;
        }

        // Attempt to connect to LCU with retries
        let client = match timeout(Duration::from_secs(5), async {
            // Try to connect with a small delay to ensure the client is ready
            sleep(Duration::from_millis(500)).await;
            LcuClient::connect()
        })
        .await
        {
            Ok(Ok(client)) => client,
            Ok(Err(e)) => return Err(format!("Failed to connect to LCU: {}", e)),
            Err(_) => return Err("Connection timeout".to_string()),
        };

        // Set up WebSocket with a delay to ensure the client is ready
        let mut ws = LcuWebSocket::new();
        let (ws_event_tx, ws_event_rx) = mpsc::unbounded_channel();
        let is_active = Arc::new(AtomicBool::new(true));
        let is_active_clone = is_active.clone();
        sleep(Duration::from_millis(1000)).await;

        // Subscribe to all LCU JSON API events
        let subscription_result =
            ws.subscribe(EventKind::json_api_event(), move |event: &Event| {
                if is_active_clone.load(Ordering::Relaxed) {
                    let _ = ws_event_tx.send(event.clone());
                }
            });

        if subscription_result.is_none() {
            return Err("Failed to subscribe to WebSocket events".to_string());
        }

        // Start event processor
        let mut event_processor = crate::EventProcessor::new(ws_event_rx, app_handle.clone());
        tauri::async_runtime::spawn(async move {
            event_processor.run().await;
        });

        // Store the successful connections
        {
            let mut client_guard = lcu_client.write().await;
            *client_guard = Some(client);
        }
        {
            let mut ws_guard = lcu_websocket.write().await;
            *ws_guard = Some((ws, is_active));
        }

        sleep(Duration::from_millis(500)).await;

        // Get initial gameflow state with retries
        let retry_count = 5;
        for i in 0..retry_count {
            if let Err(e) =
                Self::get_initial_gameflow_state(lcu_client.clone(), app_handle.clone()).await
            {
                eprintln!(
                    "Attempt {} failed to get initial gameflow state: {}",
                    i + 1,
                    e
                );
                if i < retry_count - 1 {
                    // Exponential backoff for retries
                    let delay = Duration::from_millis(500 * (i + 1));
                    sleep(delay).await;
                } else {
                    return Err(format!(
                        "Failed to get initial gameflow state after {} attempts: {}",
                        retry_count, e
                    ));
                }
            } else {
                break;
            }
        }

        Ok(())
    }

    async fn health_check(
        lcu_client: Arc<RwLock<Option<LcuClient<RequestClientType>>>>,
    ) -> Result<(), String> {
        let client = {
            let client_guard = lcu_client.read().await;
            client_guard
                .as_ref()
                .ok_or("No LCU client available")?
                .clone()
        };

        timeout(Duration::from_secs(3), async {
            client
                .get::<serde_json::Value>("/lol-summoner/v1/current-summoner")
                .await
        })
        .await
        .map_err(|_| "Health check timeout".to_string())?
        .map_err(|e| format!("Health check failed: {}", e))?;

        Ok(())
    }

    async fn cleanup_connection_resources(&self) {
        // Clean up WebSocket connection using abort() for immediate termination
        {
            let mut ws_guard = self.lcu_websocket.write().await;
            if let Some((ws, is_active)) = ws_guard.take() {
                // Stop the WebSocket event processing
                is_active.store(false, Ordering::Relaxed);
                // Use abort() to immediately terminate the WebSocket connection
                let _ = ws.abort();
            }
        }

        // Clean up HTTP client
        {
            let mut client_guard = self.lcu_client.write().await;
            *client_guard = None;
        }

        // Update the global game state to reflect disconnection
        {
            let mut game_state = get_app_state().get_game_state_mut().await;
            game_state.is_league_running = false;
            game_state.connection_status = "League Client not running".to_string();
            game_state.gameflow_status = "Waiting for League Client...".to_string();
            game_state.assigned_role = "".to_string();
        }

        // Clear any cached data that might be stale
        let mut champion_cache = get_app_state().get_champion_cache().await;
        champion_cache.cache.clear();
        drop(champion_cache);
    }

    async fn get_initial_gameflow_state(
        lcu_client: Arc<RwLock<Option<LcuClient<RequestClientType>>>>,
        app_handle: AppHandle,
    ) -> Result<(), String> {
        let client = {
            let client_guard = lcu_client.read().await;
            client_guard
                .as_ref()
                .ok_or("No LCU client available")?
                .clone()
        };

        // Get the current gameflow phase with a delay to let league fully init
        match timeout(Duration::from_secs(3), async {
            client
                .get::<serde_json::Value>("/lol-gameflow/v1/gameflow-phase")
                .await
        })
        .await
        {
            Ok(Ok(response)) => {
                if let Some(phase) = response.as_str() {
                    {
                        update_game_state(|state| match phase {
                            "Matchmaking" => {
                                state.gameflow_status = "Looking for match...".to_string();
                                state.assigned_role = "".to_string();
                            }
                            "Lobby" => {
                                state.gameflow_status = "In Lobby".to_string();
                                state.assigned_role = "".to_string();
                            }
                            "ReadyCheck" => {
                                state.gameflow_status = "Match Found!".to_string();
                            }
                            "ChampSelect" => {
                                state.gameflow_status = "Champion Select".to_string();
                            }
                            "InProgress" => {
                                state.gameflow_status = "In Game".to_string();
                            }
                            "WaitingForStats" => {
                                state.gameflow_status = "Post-Game".to_string();
                            }
                            "EndOfGame" => {
                                state.gameflow_status = "Game Complete".to_string();
                                state.assigned_role = "".to_string();
                            }
                            "None" => {
                                state.gameflow_status = "Idling...".to_string();
                            }
                            _ => {
                                state.gameflow_status = phase.to_string();
                            }
                        })
                        .await;
                        let game_state = get_app_state().get_game_state().await;
                        update_ui(&app_handle, &game_state).await;
                    }
                }
            }
            Ok(Err(e)) => {
                return Err(format!("Failed to get initial gameflow state: {}", e));
            }
            Err(_) => {
                return Err("Timeout getting initial gameflow state".to_string());
            }
        }

        match timeout(Duration::from_secs(3), async {
            client
                .get::<serde_json::Value>("/lol-gameflow/v1/session")
                .await
        })
        .await
        {
            Ok(Ok(response)) => {
                if let Some(session_obj) = response.as_object() {
                    // Update role information if available
                    if let Some(my_team) = session_obj.get("myTeam").and_then(|v| v.as_array()) {
                        if let Some(local_player_cell_id) = session_obj
                            .get("localPlayerCellId")
                            .and_then(|v| v.as_u64())
                        {
                            if let Some(player_data) = my_team.iter().find(|player| {
                                player
                                    .get("cellId")
                                    .and_then(|v| v.as_u64())
                                    .map_or(false, |id| id == local_player_cell_id)
                            }) {
                                if let Some(assigned_position) =
                                    player_data.get("assignedPosition").and_then(|v| v.as_str())
                                {
                                    {
                                        update_game_state(|state| {
                                            state.assigned_role = assigned_position.to_string();
                                        })
                                        .await;
                                        let game_state = get_app_state().get_game_state().await;
                                        update_ui(&app_handle, &game_state).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                // It's okay if this fails, we'll try to get role info later
                eprintln!("Failed to get initial session data: {}", e);
            }
            Err(_) => {
                // Timeout is acceptable here
                eprintln!("Timeout getting initial session data");
            }
        }

        Ok(())
    }

    // Public API methods
    pub fn is_connected(&self) -> bool {
        self.connection_state.load(Ordering::Relaxed) == STATE_CONNECTED
    }

    pub fn is_connecting(&self) -> bool {
        self.connection_state.load(Ordering::Relaxed) == STATE_CONNECTING
    }

    pub async fn force_reconnect(&self) {
        let _ = self
            .internal_event_tx
            .send(ConnectionEvent::ReconnectRequested);
    }

    pub async fn get_lcu_client(&self) -> Option<LcuClient<RequestClientType>> {
        let client_guard = self.lcu_client.read().await;
        client_guard.as_ref().cloned()
    }

    pub async fn shutdown(&self) {
        // Cancel all background tasks
        if let Some(task) = self.connection_task.lock().await.take() {
            task.abort();
        }
        if let Some(task) = self.event_task.lock().await.take() {
            task.abort();
        }
        if let Some(task) = self.monitor_task.lock().await.take() {
            task.abort();
        }

        // Clean up WebSocket connections using abort() for proper termination
        {
            let mut ws_guard = self.lcu_websocket.write().await;
            if let Some((ws, is_active)) = ws_guard.take() {
                // Stop the WebSocket event processing
                is_active.store(false, Ordering::Relaxed);
                // LcuWebSocket.abort() ensures immediate termination and cleanup
                let _ = ws.abort();
            }
        }
        {
            let mut client_guard = self.lcu_client.write().await;
            *client_guard = None;
        }

        // Clean up application state
        self.cleanup_connection_resources().await;
    }

    pub async fn check_league_process(&self) -> bool {
        self.process_monitor.is_league_running().await
    }
}

impl Drop for ConnectionManager {
    fn drop(&mut self) {
        // Abort all background tasks
        if let Ok(mut task_guard) = self.connection_task.try_lock() {
            if let Some(task) = task_guard.take() {
                task.abort();
            }
        }
        if let Ok(mut task_guard) = self.event_task.try_lock() {
            if let Some(task) = task_guard.take() {
                task.abort();
            }
        }
        if let Ok(mut task_guard) = self.monitor_task.try_lock() {
            if let Some(task) = task_guard.take() {
                task.abort();
            }
        }

        // Clean up WebSocket connection using abort() for proper resource cleanup
        if let Ok(mut ws_guard) = self.lcu_websocket.try_write() {
            if let Some((ws, is_active)) = ws_guard.take() {
                is_active.store(false, Ordering::Relaxed);
                // Abort WebSocket to ensure immediate cleanup even during shutdown
                let _ = ws.abort();
            }
        }

        // Clean up HTTP client
        if let Ok(mut client_guard) = self.lcu_client.try_write() {
            *client_guard = None;
        }
    }
}
