// Connection states
pub const STATE_DISCONNECTED: u8 = 0;
pub const STATE_CONNECTING: u8 = 1;
pub const STATE_CONNECTED: u8 = 2;

// Status string constants
pub const STATUS_LOOKING_FOR_MATCH: &str = "Looking for match...";
pub const STATUS_IN_LOBBY: &str = "In Lobby";
pub const STATUS_MATCH_FOUND: &str = "Match Found!";
pub const STATUS_CHAMPION_SELECT: &str = "Champion Select";
pub const STATUS_IN_GAME: &str = "In Game";
pub const STATUS_POST_GAME: &str = "Post-Game";
pub const STATUS_GAME_COMPLETE: &str = "Game Complete";
pub const STATUS_IDLING: &str = "Idling...";
pub const STATUS_LEAGUE_DETECTED: &str = "League Client detected";
pub const STATUS_CONNECTED: &str = "Connected";
pub const STATUS_CONNECTION_LOST: &str = "Connection lost";
pub const STATUS_LEAGUE_CLOSED: &str = "League Client closed";
pub const ROLE_EMPTY: &str = "";

// Champion Select virtual champion IDs.
// The LCU ships a few "champions" that never exist in champion-summary.json:
//   -1 = none/dummy (empty slot, "skip")
// Bravery is a special Arena-only pick that the client accepts but never
// surfaces in any grid or pickable list, so it has to be hardcoded - just
// like Riot's own RCP client does internally.
pub const NONE_CHAMPION_ID: i32 = -1;
pub const BRAVERY_CHAMPION_ID: i32 = -3;

// Internal game-mode name for Arena (斗魂竞技场). Used to gate Bravery.
pub const CHERRY_GAME_MODE: &str = "CHERRY";

// Display name for the Bravery pick, used by the frontend and in pick/ban UI.
pub const BRAVERY_NAME: &str = "Bravery";
