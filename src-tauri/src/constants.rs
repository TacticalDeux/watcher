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
