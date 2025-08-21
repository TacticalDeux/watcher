use crate::structs::*;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::async_runtime::RwLock;
use tokio::sync::OnceCell;

// Centralized application state
pub struct AppState {
    pub game_state: Arc<RwLock<GameState>>,
    pub last_game_state: Arc<RwLock<GameState>>,
    pub champion_cache: Arc<RwLock<ChampionCache>>,
    pub champions_data: Arc<OnceCell<ChampionsData>>,
    pub spells_data: Arc<OnceCell<SummonerSpellsData>>,
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
    pub cache: HashMap<u32, CacheEntry>,
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

pub async fn get_champions_data() -> Result<&'static ChampionsData, String> {
    get_app_state().get_champions_data().await
}

pub async fn get_summoner_spells_data() -> Result<&'static SummonerSpellsData, String> {
    get_app_state().get_summoner_spells_data().await
}

// Data loading functions
async fn load_champions_data() -> Result<ChampionsData, String> {
    let champions_json = tokio::fs::read_to_string("utils/champions.json")
        .await
        .map_err(|e| format!("Failed to read champions.json: {}", e))?;

    let champions: Vec<Champion> = serde_json::from_str(&champions_json)
        .map_err(|e| format!("Failed to parse champions.json: {}", e))?;

    let mut name_index = HashMap::new();
    let mut id_index = HashMap::new();

    for champion in &champions {
        name_index.insert(normalize_champion_name(&champion.name), champion.clone());
        id_index.insert(champion.id, champion.clone());
    }

    Ok(ChampionsData {
        name_index,
        id_index,
        array: champions,
    })
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
