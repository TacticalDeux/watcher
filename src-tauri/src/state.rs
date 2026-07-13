use crate::structs::*;
use irelia::requests::RequestClientType;
use irelia::rest::LcuClient;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::async_runtime::RwLock;
use tokio::sync::OnceCell;

// Centralized application state
pub struct AppState {
    pub game_state: Arc<RwLock<GameState>>,
    pub last_game_state: Arc<RwLock<GameState>>,
    pub champion_cache: Arc<RwLock<ChampionCache>>,
    /// Authoritative champion list. Populated from the LCU via
    /// `refresh_champions_from_lcu()` once the League client connects.
    pub champions_data: Arc<RwLock<ChampionsData>>,
    pub spells_data: Arc<OnceCell<SummonerSpellsData>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            game_state: Arc::new(RwLock::new(GameState::default())),
            last_game_state: Arc::new(RwLock::new(GameState::default())),
            champion_cache: Arc::new(RwLock::new(ChampionCache::new())),
            champions_data: Arc::new(RwLock::new(ChampionsData::default())),
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

    pub async fn get_champions_data(&self) -> Result<tokio::sync::RwLockReadGuard<'_, ChampionsData>, String> {
        Ok(self.champions_data.read().await)
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
    pub cache: HashMap<i32, CacheEntry>,
    pub cleanup_interval: Duration,
    pub last_cleanup: Instant,
    pub ttl: Duration,
}

pub struct CacheEntry {
    pub available: bool,
    pub timestamp: Instant,
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

    pub async fn get_availability(&mut self, champion_id: i32) -> Option<bool> {
        self.cleanup_expired();

        self.cache.get(&champion_id).and_then(|entry| {
            if entry.timestamp.elapsed() < self.ttl {
                Some(entry.available)
            } else {
                None
            }
        })
    }

    pub fn set_availability(&mut self, champion_id: i32, available: bool) {
        self.cache.insert(
            champion_id,
            CacheEntry {
                available,
                timestamp: Instant::now(),
            },
        );
    }

    pub fn cleanup_expired(&mut self) {
        if self.last_cleanup.elapsed() > self.cleanup_interval {
            let now = Instant::now();
            self.cache
                .retain(|_, entry| now.duration_since(entry.timestamp) < self.ttl);
            self.last_cleanup = now;
        }
    }
}

static APP_STATE: OnceLock<AppState> = OnceLock::new();

pub fn get_app_state() -> &'static AppState {
    APP_STATE.get_or_init(|| AppState::new())
}

pub async fn update_game_state<F>(updater: F)
where
    F: FnOnce(&mut GameState),
{
    let mut game_state = get_app_state().get_game_state_mut().await;
    updater(&mut game_state);
}

pub async fn get_champions_data() -> Result<tokio::sync::RwLockReadGuard<'static, ChampionsData>, String> {
    get_app_state().get_champions_data().await
}

pub async fn get_summoner_spells_data() -> Result<&'static SummonerSpellsData, String> {
    get_app_state().get_summoner_spells_data().await
}

/// Pull the champion list from the running LCU client via
/// `/lol-champ-select/v1/all-grid-champions` — the champ-select grid, keyed
/// on `id`. This is the live, authoritative set (new champions appear without
/// shipping a new data file). Called once the client connects. Retries briefly
/// in case the game-data service isn't ready yet even though auth already is.
pub async fn refresh_champions_from_lcu() -> Result<(), String> {
    // The LCU accepts auth well before its in-process game-data service is
    // ready to serve champion data, so the first call after connect can come
    // back non-JSON (or error). Retry a few times with a short backoff.
    const MAX_RETRIES: u32 = 6;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

    // all-grid-champions is the champ-select grid — exactly what the client
    // shows as pickable, keyed on `id`. (champion-summary.json was tried as an
    // alternate source but only ever returns a bare count in this client build,
    // so it's not useful as a fallback.)
    const ENDPOINT: &str = "/lol-champ-select/v1/all-grid-champions";

    let mut last_err = String::new();
    for attempt in 0..MAX_RETRIES {
        let client = LcuClient::<RequestClientType>::connect()
            .map_err(|e| format!("Failed to connect to LCU: {e}"))?;

        match client.get::<serde_json::Value>(ENDPOINT).await {
            Ok(resp) => {
                log_response_shape(ENDPOINT, &resp);
                if let Some(data) = parse_champion_array(&resp) {
                    eprintln!("refresh_champions: using {ENDPOINT} ({} champions)", data.array.len());
                    return commit_refreshed_champions(data).await;
                }
                last_err = format!("{ENDPOINT} was not a usable champion array");
            }
            Err(e) => {
                eprintln!("refresh_champions: {ENDPOINT} fetch error: {e}");
                last_err = format!("{ENDPOINT} fetch failed: {e}");
            }
        }

        if attempt + 1 < MAX_RETRIES {
            eprintln!("refresh_champions: no usable source yet, retrying in {RETRY_DELAY:?} (attempt {}/{MAX_RETRIES})", attempt + 1);
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }
    Err(format!("Live champion refresh failed: {last_err}"))
}

/// Print the JSON type and a short preview of an LCU endpoint response, so we
/// can see what the client actually returns when parsing fails (e.g. an error
/// object, an HTML redirect, or an unexpected object shape).
fn log_response_shape(endpoint: &str, response: &serde_json::Value) {
    let type_name = match response {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(a) => {
            eprintln!(
                "refresh_champions: {endpoint} -> array(len={}); first = {}",
                a.len(),
                preview_value(a.first(), 400),
            );
            return;
        }
        serde_json::Value::Object(o) => {
            eprintln!(
                "refresh_champions: {endpoint} -> object(keys={:?}); preview = {}",
                o.keys().collect::<Vec<_>>(),
                preview_value(Some(response), 400),
            );
            return;
        }
    };
    eprintln!(
        "refresh_champions: {endpoint} -> {type_name}; preview = {}",
        preview_value(Some(response), 400),
    );
}

fn preview_value(value: Option<&serde_json::Value>, limit: usize) -> String {
    let Some(value) = value else {
        return "<none>".to_string();
    };
    let mut s = value.to_string();
    if s.len() > limit {
        s.truncate(limit);
        s.push_str("...(truncated)");
    }
    s
}

/// Build the indexed championship dataset from an LCU array response. Accepts
/// either `id` (all-grid-champions, champion-summary) or `championId` as the id
/// key, whichever is present per entry. Returns `None` if the response isn't a
/// non-empty array of objects we can read champion ids/names from.
fn parse_champion_array(response: &serde_json::Value) -> Option<ChampionsData> {
    let array = response.as_array()?;
    if array.is_empty() {
        return None;
    }

    let mut champions = Vec::with_capacity(array.len());
    let mut name_index = HashMap::with_capacity(array.len());
    let mut id_index = HashMap::with_capacity(array.len());
    let mut seen_any = false;

    for entry in array.iter() {
        // Both observed LCU sources key the id on `id`; `championId` is kept as
        // a fallback for robustness against other endpoints.
        let id = entry
            .get("id")
            .or_else(|| entry.get("championId"))
            .and_then(|v| v.as_i64());
        let name = entry.get("name").and_then(|v| v.as_str());
        let Some((id, name)) = id.zip(name) else {
            continue;
        };

        // The summary includes pseudo-rows (id 0 / -1 = none/dummy) and the
        // grid endpoint lists a -1 placeholder; none are actually pickable.
        // Skip them — Bravery (-3) is handled as a virtual pick elsewhere.
        if id <= 0 {
            continue;
        }

        seen_any = true;
        let champion = Champion {
            id: id as i32,
            name: name.to_string(),
        };
        name_index.insert(normalize_champion_name(&champion.name), champion.clone());
        id_index.insert(champion.id, champion.clone());
        champions.push(champion);
    }

    if !seen_any {
        return None;
    }

    Some(ChampionsData {
        name_index,
        id_index,
        array: champions,
    })
}

/// Install freshly-fetched champion data into shared state.
async fn commit_refreshed_champions(data: ChampionsData) -> Result<(), String> {
    let mut guard = get_app_state().champions_data.write().await;
    *guard = data;
    Ok(())
}

async fn load_summoner_spells_data() -> Result<SummonerSpellsData, String> {
    let spells_json = tokio::fs::read_to_string("utils/summoner_spells.json")
        .await
        .map_err(|e| format!("Failed to read summoner_spells.json: {}", e))?;

    let spells: Vec<SummonerSpell> = serde_json::from_str(&spells_json)
        .map_err(|e| format!("Failed to parse summoner_spells.json: {}", e))?;

    let mut name_index = HashMap::new();

    for spell in &spells {
        name_index.insert(spell.name.clone(), spell.clone());
    }

    Ok(SummonerSpellsData {
        name_index,
        array: spells,
    })
}

fn normalize_champion_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Path to the persisted settings JSON file.
fn settings_path() -> PathBuf {
    let mut path = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    path.push("watcher");
    let _ = std::fs::create_dir_all(&path);
    path.push("settings.json");
    path
}

/// Persist settings to disk so active options survive restarts.
pub fn persist_settings(settings: &Settings) {
    let path = settings_path();
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(&path, json);
    }
}

/// Load settings from disk. Returns `None` if no saved file exists.
pub fn load_persisted_settings() -> Option<Settings> {
    let path = settings_path();
    if !path.exists() {
        return None;
    }
    let json = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&json).ok()
}
