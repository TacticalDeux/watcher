use chrono::Utc;
use irelia::requests::RequestClientType;
use irelia::rest::LcuClient;
use irelia::ws::{types::Event, types::EventKind, LcuWebSocket};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use sysinfo::System;
use tauri::async_runtime::{Mutex, RwLock};
use tauri::{image::Image, AppHandle, Emitter, Listener, Manager};
use tokio::fs;
use tokio::sync::{mpsc, OnceCell};
use tokio::time::{sleep, timeout, Duration};

// --- Game State Structures ---
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Champion {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct SummonerSpell {
    pub id: String,
    pub key: u32,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde(rename = "autoAccept")]
    pub auto_accept: bool,
    #[serde(rename = "pickBanSelection")]
    pub pick_ban_selection: bool,
    #[serde(rename = "spellSelection")]
    pub spell_selection: bool,
    #[serde(rename = "selectedSpell1")]
    pub selected_spell1: Option<String>,
    #[serde(rename = "selectedSpell2")]
    pub selected_spell2: Option<String>,
    #[serde(rename = "championPicks")]
    pub champion_picks: Vec<Champion>,
    #[serde(rename = "championBan")]
    pub champion_ban: Option<Champion>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TraySettings {
    #[serde(rename = "autoAccept")]
    pub auto_accept: bool,
    #[serde(rename = "pickBanSelection")]
    pub pick_ban_selection: bool,
    #[serde(rename = "spellSelection")]
    pub spell_selection: bool,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct GameState {
    #[serde(rename = "isLeagueRunning")]
    pub is_league_running: bool,
    #[serde(rename = "connectionStatus")]
    pub connection_status: String,
    #[serde(rename = "gameflowStatus")]
    pub gameflow_status: String,
    #[serde(rename = "assignedRole")]
    pub assigned_role: String,
    pub settings: Settings,
}

// --- Data Maps ---
#[derive(Debug, Clone)]
pub struct ChampionsData {
    pub name_index: HashMap<String, Champion>,
    pub id_index: HashMap<u32, Champion>,
    pub array: Vec<Champion>,
}

#[derive(Debug, Clone)]
pub struct SummonerSpellsData {
    pub name_index: HashMap<String, SummonerSpell>,
    pub array: Vec<SummonerSpell>,
}

// Centralized application state
pub struct AppState {
    game_state: Arc<RwLock<GameState>>,
    last_game_state: Arc<RwLock<GameState>>,
    champion_cache: Arc<RwLock<ChampionCache>>,
    champions_data: Arc<OnceCell<ChampionsData>>,
    spells_data: Arc<OnceCell<SummonerSpellsData>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            game_state: Arc::new(RwLock::new(GameState::default())),
            last_game_state: Arc::new(RwLock::new(GameState::default())),
            champion_cache: Arc::new(RwLock::new(ChampionCache::new())),
            champions_data: Arc::new(OnceCell::new()),
            spells_data: Arc::new(OnceCell::new()),
        }
    }

    pub async fn get_game_state(&self) -> tokio::sync::RwLockReadGuard<'_, GameState> {
        self.game_state.read().await
    }

    pub async fn get_game_state_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, GameState> {
        self.game_state.write().await
    }

    pub async fn get_last_game_state(&self) -> tokio::sync::RwLockReadGuard<'_, GameState> {
        self.last_game_state.read().await
    }

    pub async fn get_last_game_state_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, GameState> {
        self.last_game_state.write().await
    }

    pub async fn get_champion_cache(&self) -> tokio::sync::RwLockWriteGuard<'_, ChampionCache> {
        self.champion_cache.write().await
    }

    pub async fn get_champions_data(&self) -> Result<&ChampionsData, String> {
        match self.champions_data.get() {
            Some(data) => Ok(data),
            None => {
                let data = load_champions_data().await?;
                match self.champions_data.set(data) {
                    Ok(()) => Ok(self.champions_data.get().unwrap()),
                    Err(_) => Ok(self.champions_data.get().unwrap()), // Another thread set it
                }
            }
        }
    }

    pub async fn get_summoner_spells_data(&self) -> Result<&SummonerSpellsData, String> {
        match self.spells_data.get() {
            Some(data) => Ok(data),
            None => {
                let data = load_summoner_spells_data().await?;
                match self.spells_data.set(data) {
                    Ok(()) => Ok(self.spells_data.get().unwrap()),
                    Err(_) => Ok(self.spells_data.get().unwrap()), // Another thread set it
                }
            }
        }
    }
}

// Champion availability cache
pub struct ChampionCache {
    cache: HashMap<u32, CacheEntry>,
    cleanup_interval: Duration,
    last_cleanup: Instant,
    ttl: Duration,
}

struct CacheEntry {
    available: bool,
    timestamp: Instant,
}

impl ChampionCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            cleanup_interval: Duration::from_secs(60),
            last_cleanup: Instant::now(),
            ttl: Duration::from_millis(30000),
        }
    }

    pub async fn get_availability(&mut self, champion_id: u32) -> Option<bool> {
        self.cleanup_expired();

        self.cache.get(&champion_id).and_then(|entry| {
            if entry.timestamp.elapsed() < self.ttl {
                Some(entry.available)
            } else {
                None
            }
        })
    }

    pub fn set_availability(&mut self, champion_id: u32, available: bool) {
        self.cache.insert(
            champion_id,
            CacheEntry {
                available,
                timestamp: Instant::now(),
            },
        );
    }

    fn cleanup_expired(&mut self) {
        if self.last_cleanup.elapsed() > self.cleanup_interval {
            let now = Instant::now();
            self.cache
                .retain(|_, entry| now.duration_since(entry.timestamp) < self.ttl);
            self.last_cleanup = now;
        }
    }
}

static APP_STATE: OnceLock<AppState> = OnceLock::new();

fn get_app_state() -> &'static AppState {
    APP_STATE.get_or_init(|| AppState::new())
}

async fn update_game_state<F>(updater: F) -> GameState
where
    F: FnOnce(&mut GameState),
{
    let mut game_state = get_app_state().get_game_state_mut().await;
    updater(&mut game_state);
    game_state.clone()
}

pub async fn get_champions_data() -> Result<&'static ChampionsData, String> {
    get_app_state().get_champions_data().await
}

pub async fn get_summoner_spells_data() -> Result<&'static SummonerSpellsData, String> {
    get_app_state().get_summoner_spells_data().await
}

async fn load_champions_data() -> Result<ChampionsData, String> {
    let path = std::env::current_dir()
        .map_err(|e| format!("Failed to get current directory: {}", e))?
        .join("utils")
        .join("champions.json");

    let data = fs::read_to_string(&path)
        .await
        .map_err(|e| format!("Failed to read champions.json: {}", e))?;

    let champions_array: Vec<Champion> = serde_json::from_str(&data)
        .map_err(|e| format!("Failed to parse champions data: {}", e))?;

    // Pre-compute indices for faster lookups
    let mut name_index = HashMap::with_capacity(champions_array.len() * 2);
    let mut id_index = HashMap::with_capacity(champions_array.len());

    for champ in &champions_array {
        let normalized_name = normalize_champion_name(&champ.name);
        name_index.insert(normalized_name, champ.clone());
        name_index.insert(champ.name.to_lowercase(), champ.clone());
        id_index.insert(champ.id, champ.clone());
    }

    Ok(ChampionsData {
        name_index,
        id_index,
        array: champions_array,
    })
}

async fn load_summoner_spells_data() -> Result<SummonerSpellsData, String> {
    let path = std::env::current_dir()
        .map_err(|e| format!("Failed to get current directory: {}", e))?
        .join("utils")
        .join("summoner_spells.json");

    let data = fs::read_to_string(&path)
        .await
        .map_err(|e| format!("Failed to read summoner_spells.json: {}", e))?;

    let spells_array: Vec<SummonerSpell> = serde_json::from_str(&data)
        .map_err(|e| format!("Failed to parse summoner spells data: {}", e))?;

    // Pre-compute name index for faster lookups
    let mut name_index = HashMap::with_capacity(spells_array.len());

    for spell in &spells_array {
        name_index.insert(spell.name.clone(), spell.clone());
    }

    Ok(SummonerSpellsData {
        name_index,
        array: spells_array,
    })
}

fn normalize_champion_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

// --- Connection Manager ---
// Connection states
const STATE_DISCONNECTED: u8 = 0;
const STATE_CONNECTING: u8 = 1;
const STATE_CONNECTED: u8 = 2;

// Connection events for internal communication
#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    LeagueProcessDetected,
    LeagueProcessLost,
    ConnectionEstablished,
    ConnectionLost,
    HealthCheckFailed,
    ReconnectRequested,
}

// Exponential backoff for connection attempts
#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    current_delay: Duration,
    max_delay: Duration,
    multiplier: f64,
    base_delay: Duration,
}

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

// Connection health tracker
#[derive(Debug, Clone)]
pub struct ConnectionHealth {
    last_successful_request: Arc<AtomicU64>,
    consecutive_failures: Arc<AtomicU64>,
    is_healthy: Arc<AtomicBool>,
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

// Process monitor for League client
#[derive(Debug, Clone)]
pub struct ProcessMonitor {
    last_check: Arc<AtomicU64>,
    last_result: Arc<AtomicBool>,
    check_interval: Duration,
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

#[derive(Debug, Clone)]
pub struct LeagueClientReadiness {
    process_start_time: Arc<AtomicU64>,
    is_ready: Arc<AtomicBool>,
    readiness_check_interval: Duration,
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

// Event processor for WebSocket events
pub struct EventProcessor {
    event_rx: mpsc::UnboundedReceiver<Event>,
    app_handle: AppHandle,
    throttle_map: Arc<Mutex<HashMap<String, u64>>>,
    app_state: &'static AppState,
    ban_completed_this_phase: Arc<Mutex<bool>>,
}

impl EventProcessor {
    pub fn new(event_rx: mpsc::UnboundedReceiver<Event>, app_handle: AppHandle) -> Self {
        Self {
            event_rx,
            app_handle,
            throttle_map: Arc::new(Mutex::new(HashMap::new())),
            app_state: get_app_state(),
            ban_completed_this_phase: Arc::new(Mutex::new(false)),
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
                        self.handle_gameflow_phase(event_data).await?;
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
                    _ => {
                        // Log unhandled events for debugging if needed
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_gameflow_phase(
        &self,
        event_data: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        if let Some(phase) = event_data.get("data").and_then(|v| v.as_str()) {
            // Reset ban flag when entering champion select
            if phase == "ChampSelect" {
                let mut ban_flag = self.ban_completed_this_phase.lock().await;
                *ban_flag = false;
            }

            let game_state_clone = update_game_state(|state| match phase {
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

            update_ui(&self.app_handle, &game_state_clone).await;
        }
        Ok(())
    }

    async fn handle_gameflow_session(
        &self,
        event_data: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        if let Some(data_obj) = event_data.get("data").and_then(|v| v.as_object()) {
            // Handle phase changes
            let phase = data_obj.get("phase").and_then(|v| v.as_str()).or_else(|| {
                data_obj
                    .get("gameData")
                    .and_then(|v| v.as_object())
                    .and_then(|game_data| game_data.get("phase"))
                    .and_then(|v| v.as_str())
            });

            if let Some(phase_str) = phase {
                // Reset ban flag when entering champion select
                if phase_str == "ChampSelect" {
                    let mut ban_flag = self.ban_completed_this_phase.lock().await;
                    *ban_flag = false;
                }

                let game_state_clone = update_game_state(|state| match phase_str {
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
                        state.gameflow_status = phase_str.to_string();
                    }
                })
                .await;

                update_ui(&self.app_handle, &game_state_clone).await;
            }

            self.handle_role_assignment(data_obj).await?;
        }
        Ok(())
    }

    async fn handle_role_assignment(
        &self,
        data_obj: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        if let Some(my_team) = data_obj.get("myTeam").and_then(|v| v.as_array()) {
            if let Some(local_player_cell_id) =
                data_obj.get("localPlayerCellId").and_then(|v| v.as_u64())
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
                        let game_state_clone = update_game_state(|state| {
                            state.assigned_role = assigned_position.to_string();
                        })
                        .await;
                        update_ui(&self.app_handle, &game_state_clone).await;
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_ready_check(
        &self,
        event_data: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        if let Some(data_obj) = event_data.get("data").and_then(|v| v.as_object()) {
            if let Some(state) = data_obj.get("state").and_then(|v| v.as_str()) {
                if state == "InProgress" {
                    let game_state = self.app_state.get_game_state().await;
                    let should_auto_accept = game_state.settings.auto_accept;
                    drop(game_state);

                    if should_auto_accept {
                        let _ = self.auto_accept_match().await;
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_champion_select(
        &self,
        event_data: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        if let Some(data_obj) = event_data.get("data").and_then(|v| v.as_object()) {
            let (
                should_pick_ban,
                should_select_spells,
                champion_picks,
                champion_ban,
                selected_spell1,
                selected_spell2,
            ) = {
                let game_state = self.app_state.get_game_state().await;
                (
                    game_state.settings.pick_ban_selection,
                    game_state.settings.spell_selection,
                    game_state.settings.champion_picks.clone(),
                    game_state.settings.champion_ban.clone(),
                    game_state.settings.selected_spell1.clone(),
                    game_state.settings.selected_spell2.clone(),
                )
            };

            if should_pick_ban {
                let current_phase = data_obj
                    .get("timer")
                    .and_then(|t| t.as_object())
                    .and_then(|timer| timer.get("phase"))
                    .and_then(|p| p.as_str());

                if current_phase == Some("PLANNING") {
                    // Skip actions during planning phase (first ~15 seconds before banning phase)
                    return Ok(());
                }

                if let Some(actions) = data_obj.get("actions").and_then(|v| v.as_array()) {
                    self.process_champion_select_actions(
                        actions,
                        data_obj,
                        &champion_picks,
                        &champion_ban,
                    )
                    .await?;
                }
            }

            if should_select_spells && selected_spell1.is_some() && selected_spell2.is_some() {
                let _ = self.handle_spell_selection().await;
            }
        }
        Ok(())
    }

    async fn process_champion_select_actions(
        &self,
        actions: &Vec<serde_json::Value>,
        data_obj: &serde_json::Map<String, serde_json::Value>,
        champion_picks: &Vec<Champion>,
        champion_ban: &Option<Champion>,
    ) -> Result<(), String> {
        let local_player_cell_id = data_obj
            .get("localPlayerCellId")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Flatten the actions array (it's nested)
        for action_group in actions {
            if let Some(action_array) = action_group.as_array() {
                for action in action_array {
                    if let Some(action_obj) = action.as_object() {
                        let actor_cell_id = action_obj
                            .get("actorCellId")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);

                        // Only process actions for the current user
                        if actor_cell_id != local_player_cell_id {
                            continue;
                        }

                        let is_in_progress = action_obj
                            .get("isInProgress")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        let is_completed = action_obj
                            .get("completed")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);

                        let action_type = action_obj
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        if !is_in_progress || is_completed {
                            continue;
                        }

                        match action_type {
                            "ban" => {
                                if let Some(champion) = champion_ban.as_ref() {
                                    // Check if we haven't already banned this phase
                                    let can_ban = {
                                        let ban_flag = self.ban_completed_this_phase.lock().await;
                                        !*ban_flag
                                    };

                                    if can_ban {
                                        let _ = self
                                            .handle_ban(
                                                serde_json::to_value(action_obj).unwrap(),
                                                champion.clone(),
                                            )
                                            .await;
                                    }
                                }
                            }
                            "pick" => {
                                // Try primary pick first, then fallback picks
                                for champion in champion_picks {
                                    let is_available =
                                        match self.is_champion_available(champion.id).await {
                                            Ok(available) => available,
                                            Err(_) => continue, // Skip if we can't check availability
                                        };

                                    if is_available {
                                        let _ = self
                                            .handle_pick(
                                                serde_json::to_value(action_obj).unwrap(),
                                                champion.clone(),
                                            )
                                            .await;
                                        break;
                                    }
                                }
                            }
                            _ => {} // Ignore other action types
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn check_websocket_health(&self) -> Result<(), String> {
        let client = self
            .get_lcu_client()
            .await
            .map_err(|e| format!("Failed to get LCU client: {}", e))?;

        // Try a simple request to check if the connection is still alive
        timeout(Duration::from_secs(2), async {
            client
                .get::<serde_json::Value>("/lol-summoner/v1/current-summoner")
                .await
        })
        .await
        .map_err(|_| "Health check timeout".to_string())?
        .map_err(|e| format!("Health check failed: {}", e))?;

        Ok(())
    }

    async fn auto_accept_match(&self) -> Result<(), String> {
        let client = self.get_lcu_client().await?;
        match client
            .post::<_, serde_json::Value>(
                "/lol-matchmaking/v1/ready-check/accept",
                &serde_json::json!({}),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("Failed to accept match: {}", e)),
        }
    }

    async fn handle_pick(
        &self,
        pick_action: serde_json::Value,
        champion_pick: Champion,
    ) -> Result<(), String> {
        let client = self.get_lcu_client().await?;

        let is_available = self.is_champion_available(champion_pick.id).await?;
        if !is_available {
            return Err(format!(
                "Champion {} is not available for pick.",
                champion_pick.name
            ));
        }

        let action_id = pick_action
            .get("id")
            .and_then(|v| v.as_u64())
            .ok_or("Pick action ID not found")?;

        let actor_cell_id = pick_action
            .get("actorCellId")
            .and_then(|v| v.as_u64())
            .ok_or("Actor cell ID not found")?;

        let is_in_progress = pick_action
            .get("isInProgress")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let is_completed = pick_action
            .get("completed")
            .and_then(|v| v.as_bool())
            .unwrap_or(true); // Default to true to be safe

        if !is_in_progress || is_completed {
            return Err("Pick action is not available for completion".to_string());
        }

        let body = serde_json::json!({
            "actorCellId": actor_cell_id,
            "championId": champion_pick.id,
            "completed": true,
            "id": action_id,
            "isAllyAction": true,
            "type": "pick"
        });

        match client
            .patch::<_, serde_json::Value>(
                &format!("/lol-champ-select/v1/session/actions/{}", action_id),
                &body,
            )
            .await
        {
            Ok(_) => {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                Ok(())
            }
            Err(e) => Err(format!("Pick failed for {}: {}", champion_pick.name, e)),
        }
    }

    async fn handle_ban(
        &self,
        ban_action: serde_json::Value,
        champion_ban: Champion,
    ) -> Result<(), String> {
        let client = self.get_lcu_client().await?;

        let is_available = self.is_champion_available(champion_ban.id).await?;
        if !is_available {
            return Err(format!(
                "Champion {} is not available for ban.",
                champion_ban.name
            ));
        }

        let action_id = ban_action
            .get("id")
            .and_then(|v| v.as_u64())
            .ok_or("Ban action ID not found")?;

        let actor_cell_id = ban_action
            .get("actorCellId")
            .and_then(|v| v.as_u64())
            .ok_or("Actor cell ID not found")?;

        let is_in_progress = ban_action
            .get("isInProgress")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let is_completed = ban_action
            .get("completed")
            .and_then(|v| v.as_bool())
            .unwrap_or(true); // Default to true to be safe

        if !is_in_progress || is_completed {
            return Err("Ban action is not available for completion".to_string());
        }

        let body = serde_json::json!({
            "actorCellId": actor_cell_id,
            "championId": champion_ban.id,
            "completed": true,
            "id": action_id,
            "isAllyAction": true,
            "type": "ban"
        });

        match client
            .patch::<_, serde_json::Value>(
                &format!("/lol-champ-select/v1/session/actions/{}", action_id),
                &body,
            )
            .await
        {
            Ok(_) => {
                // Mark ban as completed for this phase
                {
                    let mut ban_flag = self.ban_completed_this_phase.lock().await;
                    *ban_flag = true;
                }
                Ok(())
            }
            Err(e) => Err(format!("Ban failed for {}: {}", champion_ban.name, e)),
        }
    }

    async fn handle_spell_selection(&self) -> Result<(), String> {
        let (selected_spell1_name, selected_spell2_name) = {
            let game_state_guard = self.app_state.get_game_state().await;
            (
                game_state_guard.settings.selected_spell1.clone(),
                game_state_guard.settings.selected_spell2.clone(),
            )
        };

        let spells_data = get_summoner_spells_data().await?;

        let spell1 =
            selected_spell1_name.and_then(|name| spells_data.name_index.get(&name).cloned());
        let spell2 =
            selected_spell2_name.and_then(|name| spells_data.name_index.get(&name).cloned());

        if let (Some(spell1), Some(spell2)) = (spell1, spell2) {
            let client = self.get_lcu_client().await?;
            let body = serde_json::json!({
                "spell1Id": spell1.key,
                "spell2Id": spell2.key,
            });

            match client
                .patch::<_, serde_json::Value>("/lol-champ-select/v1/session/my-selection", &body)
                .await
            {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("Spell selection failed: {}", e)),
            }
        } else {
            Err("One or both selected spells not found.".to_string())
        }
    }

    async fn is_champion_available(&self, champion_id: u32) -> Result<bool, String> {
        let mut cache = self.app_state.get_champion_cache().await;

        // Check cache first
        if let Some(available) = cache.get_availability(champion_id).await {
            return Ok(available);
        }

        let client = self.get_lcu_client().await?;
        match client
            .get::<serde_json::Value>(&format!(
                "/lol-champ-select/v1/grid-champions/{}",
                champion_id
            ))
            .await
        {
            Ok(response) => {
                let is_available = response
                    .get("selectionStatus")
                    .and_then(|s| s.get("pickedByOtherOrBanned"))
                    .and_then(|v| v.as_bool())
                    .map_or(true, |b| !b);

                cache.set_availability(champion_id, is_available);

                Ok(is_available)
            }
            Err(e) => Err(format!("Failed to check champion availability: {}", e)),
        }
    }

    async fn get_lcu_client(&self) -> Result<LcuClient<RequestClientType>, String> {
        let manager = self.app_handle.state::<ConnectionManager>();
        manager
            .get_lcu_client()
            .await
            .ok_or("LCU Client not available".to_string())
    }

    async fn should_process_event(&self, uri: &str) -> bool {
        let now = Utc::now().timestamp_millis() as u64;
        let key = uri.to_string();

        // Read lock first
        {
            let throttle_map = self.throttle_map.lock().await;
            if let Some(&last_processed) = throttle_map.get(&key) {
                if now - last_processed < 300 {
                    return false;
                }
            }
        }

        // Write lock to update
        let mut throttle_map = self.throttle_map.lock().await;
        throttle_map.insert(key, now);

        // Cleanup old entries periodically
        if throttle_map.len() > 100 {
            let cutoff = now - 60000;
            throttle_map.retain(|_, &mut timestamp| timestamp > cutoff);
        }

        true
    }
}

pub struct ConnectionManager {
    // Connection state
    connection_state: Arc<AtomicU8>,

    // LCU client and WebSocket
    lcu_client: Arc<RwLock<Option<LcuClient<RequestClientType>>>>,
    lcu_websocket: Arc<RwLock<Option<(LcuWebSocket, Arc<AtomicBool>)>>>,

    // Event communication
    internal_event_tx: mpsc::UnboundedSender<ConnectionEvent>,
    internal_event_rx: Arc<Mutex<mpsc::UnboundedReceiver<ConnectionEvent>>>,

    // Health monitoring
    connection_health: ConnectionHealth,
    process_monitor: ProcessMonitor,

    // Task handles
    connection_task: Arc<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,
    event_task: Arc<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,
    monitor_task: Arc<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,

    // Rate limiting for connection attempts
    last_connection_attempt: Arc<AtomicU64>,
    connection_attempt_cooldown: Duration,

    // League Client readiness for connection attempts
    client_readiness: LeagueClientReadiness,
}

impl ConnectionManager {
    pub fn new() -> Self {
        let (internal_event_tx, internal_event_rx) = mpsc::unbounded_channel();

        Self {
            connection_state: Arc::new(AtomicU8::new(STATE_DISCONNECTED)),
            lcu_client: Arc::new(RwLock::new(None)),
            lcu_websocket: Arc::new(RwLock::new(None)),
            internal_event_tx,
            internal_event_rx: Arc::new(Mutex::new(internal_event_rx)),
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
        self.start_connection_loop(app_handle.clone()).await;
        self.start_event_loop(app_handle.clone()).await;
        self.start_monitoring_loop(app_handle).await;
    }

    async fn start_connection_loop(&self, app_handle: AppHandle) {
        let connection_state = self.connection_state.clone();
        let process_monitor = ProcessMonitor::new();
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
                    let game_state_clone = update_game_state(|state| {
                        state.is_league_running = is_league_running;
                        if !is_league_running {
                            state.connection_status = "League Client not running".to_string();
                            state.gameflow_status = "Waiting for League Client...".to_string();
                            state.assigned_role = "".to_string();
                        }
                    })
                    .await;

                    update_ui(&app_handle_clone, &game_state_clone).await;
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
                                let game_state_clone = update_game_state(|state| {
                                    state.connection_status = "Waiting to connect...".to_string();
                                })
                                .await;

                                update_ui(&app_handle_clone, &game_state_clone).await;
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
                                let game_state_clone = update_game_state(|state| {
                                    state.connection_status = "Connected".to_string();
                                })
                                .await;

                                update_ui(&app_handle_clone, &game_state_clone).await;
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
                        let game_state_clone = update_game_state(|state| {
                            state.is_league_running = true;
                            state.connection_status = "League Client detected".to_string();
                        })
                        .await;
                        update_ui(&app_handle_clone, &game_state_clone).await;
                    }
                    ConnectionEvent::LeagueProcessLost => {
                        let game_state_clone = update_game_state(|state| {
                            state.is_league_running = false;
                            state.connection_status = "League Client not running".to_string();
                            state.gameflow_status = "Waiting for League Client...".to_string();
                            state.assigned_role = "".to_string();
                        })
                        .await;
                        update_ui(&app_handle_clone, &game_state_clone).await;

                        // Connection cleanup will be handled by the main connection manager
                    }
                    ConnectionEvent::ConnectionEstablished => {
                        let game_state_clone = update_game_state(|state| {
                            state.connection_status = "Connected".to_string();
                        })
                        .await;
                        update_ui(&app_handle_clone, &game_state_clone).await;
                    }
                    ConnectionEvent::ConnectionLost => {
                        let game_state_clone = update_game_state(|state| {
                            state.connection_status = "Connection lost - retrying".to_string();
                            state.gameflow_status = "Waiting for League Client...".to_string();
                            state.assigned_role = "".to_string();
                        })
                        .await;
                        update_ui(&app_handle_clone, &game_state_clone).await;

                        // Connection cleanup will be handled by the main connection manager
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
        let mut event_processor = EventProcessor::new(ws_event_rx, app_handle.clone());
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

    /// Cleans up all connection resources including WebSocket and HTTP clients
    ///
    /// This method properly terminates WebSocket connections using the .abort() method
    /// which ensures immediate cleanup of underlying resources and event handlers.
    async fn cleanup_connection_resources(&self) {
        // Clean up WebSocket connection using abort() for immediate termination
        {
            let mut ws_guard = self.lcu_websocket.write().await;
            if let Some((ws, is_active)) = ws_guard.take() {
                // Stop the WebSocket event processing
                is_active.store(false, Ordering::Relaxed);
                // Use abort() to immediately terminate the WebSocket connection
                // This cleans up internal event loops and closes the connection gracefully
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
                        let game_state_clone = update_game_state(|state| match phase {
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
                        update_ui(&app_handle, &game_state_clone).await;
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
                                        let game_state_clone = update_game_state(|state| {
                                            state.assigned_role = assigned_position.to_string();
                                        })
                                        .await;
                                        update_ui(&app_handle, &game_state_clone).await;
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

    /// Gracefully shuts down the ConnectionManager
    ///
    /// Cancels all background tasks and properly terminates WebSocket connections
    /// using the abort() method for immediate resource cleanup.
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

    // Add a method to check if League is running (for external calls)
    pub async fn check_league_process(&self) -> bool {
        self.process_monitor.is_league_running().await
    }
}

impl Drop for ConnectionManager {
    /// Ensures cleanup even if shutdown() wasn't called explicitly
    ///
    /// Uses try_lock variants to avoid blocking in the destructor context
    /// and properly aborts WebSocket connections for resource cleanup.
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

// --- Tauri Command handlers ---
#[tauri::command]
async fn get_champions_and_spells() -> Result<serde_json::Value, String> {
    let champions_array = match get_champions_data().await {
        Ok(champions_data) => champions_data.array.clone(),
        Err(_) => Vec::new(),
    };

    let spells_array = match get_summoner_spells_data().await {
        Ok(spells_data) => spells_data.array.clone(),
        Err(_) => Vec::new(),
    };

    Ok(serde_json::json!({
        "champions": champions_array,
        "summonerSpells": spells_array
    }))
}

#[tauri::command]
async fn get_current_game_state() -> Result<GameState, String> {
    let game_state = get_app_state().get_game_state().await.clone();
    Ok(game_state)
}

#[tauri::command]
async fn clear_picks_bans(app_handle: AppHandle) -> Result<(), String> {
    let game_state_clone = {
        let mut game_state = get_app_state().get_game_state_mut().await;
        game_state.settings.champion_picks.clear();
        game_state.settings.champion_ban = None;
        game_state.clone()
    };
    update_ui(&app_handle, &game_state_clone).await;
    Ok(())
}

#[tauri::command]
async fn update_checkbox(app_handle: AppHandle, id: String, checked: bool) -> Result<(), String> {
    let game_state_clone = {
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
        game_state.clone()
    };
    update_ui(&app_handle, &game_state_clone).await;
    Ok(())
}

#[tauri::command]
async fn update_pick_ban_text(
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
        let game_state_clone = {
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
            game_state.clone()
        };
        update_ui(&app_handle, &game_state_clone).await;
    }
    Ok(())
}

#[tauri::command]
async fn update_selected_spell(
    app_handle: AppHandle,
    spell_slot: u32,
    spell_name: String,
) -> Result<(), String> {
    if spell_name.is_empty() || (spell_slot != 1 && spell_slot != 2) {
        return Ok(());
    }
    let spells = get_summoner_spells_data().await?;
    if !spells.name_index.contains_key(&spell_name) {
        return Ok(());
    }
    let game_state_clone = {
        let mut game_state = get_app_state().get_game_state_mut().await;
        if spell_slot == 1 {
            game_state.settings.selected_spell1 = Some(spell_name);
        } else {
            game_state.settings.selected_spell2 = Some(spell_name);
        }
        game_state.clone()
    };
    update_ui(&app_handle, &game_state_clone).await;
    Ok(())
}

#[tauri::command]
async fn remove_champion_pick(app_handle: AppHandle, index: usize) -> Result<(), String> {
    let game_state_clone = {
        let mut game_state = get_app_state().get_game_state_mut().await;
        if index < game_state.settings.champion_picks.len() {
            game_state.settings.champion_picks.remove(index);
            game_state.clone()
        } else {
            return Err(format!("Invalid index for remove_champion_pick: {}", index));
        }
    };
    update_ui(&app_handle, &game_state_clone).await;
    Ok(())
}

#[tauri::command]
async fn reorder_champion_picks(
    app_handle: AppHandle,
    new_order: Vec<usize>,
) -> Result<(), String> {
    let game_state_clone = {
        let mut game_state = get_app_state().get_game_state_mut().await;
        let current_picks_len = game_state.settings.champion_picks.len();
        if new_order.len() != current_picks_len
            || new_order.iter().any(|&idx| idx >= current_picks_len)
        {
            return Err(format!(
                "Invalid new_order for reorder_champion_picks: {:?}",
                new_order
            ));
        }
        let mut reordered_picks: Vec<Champion> = Vec::with_capacity(current_picks_len);
        let original_picks = game_state.settings.champion_picks.clone();
        for &index in &new_order {
            reordered_picks.push(original_picks[index].clone());
        }
        game_state.settings.champion_picks = reordered_picks;
        game_state.clone()
    };
    update_ui(&app_handle, &game_state_clone).await;
    Ok(())
}

#[tauri::command]
fn show_app(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
    }
}

#[tauri::command]
fn hide_app(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[tauri::command]
async fn update_tray_tooltip(
    app_handle: AppHandle,
    _connection_status: String,
    gameflow_status: String,
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
                    let champions_result = get_champions_data().await;
                    let spells_result = get_summoner_spells_data().await;
                    let champions_array = match get_champions_data().await {
                        Ok(champions_data) => champions_data.array.clone(),
                        Err(_) => Vec::new(),
                    };
                    let spells_array = match get_summoner_spells_data().await {
                        Ok(spells_data) => spells_data.array.clone(),
                        Err(_) => Vec::new(),
                    };
                });
            });

            #[cfg(debug_assertions)]
            {
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
            }

            let connection_manager = ConnectionManager::new();
            app.manage(connection_manager);

            let app_handle = app.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                // Wait a bit for the UI to be ready
                sleep(Duration::from_millis(1000)).await;

                let manager = app_handle.state::<ConnectionManager>();
                let is_league_running = manager.check_league_process().await;

                // Set initial game state
                {
                    let initial_state = {
                        let mut game_state = get_app_state().get_game_state_mut().await;
                        game_state.is_league_running = is_league_running;
                        if is_league_running {
                            game_state.connection_status = "League Client detected".to_string();
                            game_state.gameflow_status =
                                "Connecting to League Client...".to_string();
                        } else {
                            game_state.connection_status =
                                "Waiting for League Client...".to_string();
                            game_state.gameflow_status = "Waiting for League Client...".to_string();
                        }
                        game_state.clone()
                    };
                    update_ui(&app_handle, &initial_state).await;
                }

                manager.start(app_handle.clone()).await;

                let app_handle_tray = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        sleep(Duration::from_secs(5)).await;
                        let current_game_state = get_app_state().get_game_state().await.clone();
                        let tray_settings = TraySettings {
                            auto_accept: current_game_state.settings.auto_accept,
                            pick_ban_selection: current_game_state.settings.pick_ban_selection,
                            spell_selection: current_game_state.settings.spell_selection,
                        };
                        let _ = update_tray_tooltip(
                            app_handle_tray.clone(),
                            current_game_state.connection_status.clone(),
                            current_game_state.gameflow_status.clone(),
                            tray_settings,
                        )
                        .await;
                    }
                });

                let app_handle_clone = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        sleep(Duration::from_secs(30)).await;
                        let mut champion_cache = get_app_state().get_champion_cache().await;
                        champion_cache.cleanup_expired();
                        if champion_cache.cache.len() > 100 {
                            champion_cache.cache.clear();
                        }
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
            show_app,
            hide_app,
            update_tray_tooltip,
            get_champions_and_spells,
            get_current_game_state
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
