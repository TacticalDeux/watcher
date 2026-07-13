use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::Duration;
use tauri::async_runtime::RwLock;
use tauri::AppHandle;
use tokio::sync::mpsc;

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Champion {
    // i32 so virtual picks like Bravery (-3) can sit alongside real champion IDs.
    pub id: i32,
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
    /// Arena-only "Auto Pick Bravery" toggle.
    #[serde(rename = "autoBravery")]
    pub auto_bravery: bool,
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
    /// Remembered close preference: true = minimize to tray, false = quit.
    /// `None` means the user hasn't chosen yet (shows a one-time dialog).
    /// User sets this via the "Close to Tray" checkbox in Settings.
    #[serde(rename = "closeToTray", default, skip_serializing_if = "Option::is_none")]
    pub close_to_tray: Option<bool>,
    /// Whether the app should start hidden in the tray when the OS auto-launches
    /// it at system startup. Only meaningful while "Open on System Start" is on,
    /// so the UI shows it as an indented sub-option of that toggle. `None` means
    /// the user hasn't chosen yet.
    #[serde(rename = "startMinimized", default, skip_serializing_if = "Option::is_none")]
    pub start_minimized: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TraySettings {
    #[serde(rename = "autoAccept")]
    pub auto_accept: bool,
    #[serde(rename = "pickBanSelection")]
    pub pick_ban_selection: bool,
    /// Arena-only "Auto Pick Bravery" toggle.
    #[serde(rename = "autoBravery")]
    pub auto_bravery: bool,
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
    /// Internal LCU game-mode of the active session (e.g. "CHERRY" = Arena).
    /// Drives mode-specific behaviour like the Bravery auto-pick.
    #[serde(rename = "gameMode")]
    pub game_mode: String,
    pub settings: Settings,
}

#[derive(Debug, Clone, Default)]
pub struct ChampionsData {
    pub name_index: HashMap<String, Champion>,
    pub id_index: HashMap<i32, Champion>,
    pub array: Vec<Champion>,
}

#[derive(Debug, Clone)]
pub struct SummonerSpellsData {
    pub name_index: HashMap<String, SummonerSpell>,
    pub array: Vec<SummonerSpell>,
}

#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    LeagueProcessDetected,
    LeagueProcessLost,
    ConnectionEstablished,
    ConnectionLost,
    HealthCheckFailed,
    ReconnectRequested,
}

#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    pub current_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
    pub base_delay: Duration,
}

#[derive(Debug, Clone)]
pub struct ConnectionHealth {
    pub last_successful_request: Arc<AtomicU64>,
    pub consecutive_failures: Arc<AtomicU64>,
    pub is_healthy: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub struct ProcessMonitor {
    pub last_check: Arc<AtomicU64>,
    pub last_result: Arc<AtomicBool>,
    pub check_interval: Duration,
}

#[derive(Debug, Clone)]
pub struct LeagueClientReadiness {
    pub process_start_time: Arc<AtomicU64>,
    pub is_ready: Arc<AtomicBool>,
    pub readiness_check_interval: Duration,
}

pub struct EventProcessor {
    pub event_rx: mpsc::UnboundedReceiver<irelia::ws::types::Event>,
    pub app_handle: AppHandle,
    pub throttle_map: std::sync::Arc<tauri::async_runtime::Mutex<HashMap<String, u64>>>,
    pub app_state: &'static crate::state::AppState,
    pub ban_completed_this_phase: std::sync::Arc<tauri::async_runtime::Mutex<bool>>,
}

pub struct ConnectionManager {
    pub connection_state: std::sync::Arc<std::sync::atomic::AtomicU8>,
    pub lcu_client: std::sync::Arc<
        RwLock<Option<irelia::rest::LcuClient<irelia::requests::RequestClientType>>>,
    >,
    pub lcu_websocket: std::sync::Arc<
        RwLock<
            Option<(
                irelia::ws::LcuWebSocket,
                std::sync::Arc<std::sync::atomic::AtomicBool>,
            )>,
        >,
    >,
    pub internal_event_tx: mpsc::UnboundedSender<ConnectionEvent>,
    pub internal_event_rx:
        std::sync::Arc<tauri::async_runtime::Mutex<mpsc::UnboundedReceiver<ConnectionEvent>>>,
    pub connection_health: ConnectionHealth,
    pub process_monitor: ProcessMonitor,
    pub connection_task:
        std::sync::Arc<tauri::async_runtime::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,
    pub event_task:
        std::sync::Arc<tauri::async_runtime::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,
    pub monitor_task:
        std::sync::Arc<tauri::async_runtime::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,
    pub last_connection_attempt: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub connection_attempt_cooldown: Duration,
    pub client_readiness: LeagueClientReadiness,
}
