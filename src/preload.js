(async () => {
  // Wait for __TAURI__ to be available
  while (!window.__TAURI__) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }

  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  const { open } = window.__TAURI__.shell;

  window.tauriAPI = {
    getChampionsAndSpells: () => invoke("get_champions_and_spells"),
    getCurrentGameState: () => invoke("get_current_game_state"),

    onStatusUpdate: (callback) => {
      listen("status-update", (event) => callback(event.payload));
    },

    clearPicksBans: () => {
      invoke("clear_picks_bans");
    },

    updateCheckbox: (id, checked) => {
      invoke("update_checkbox", { id, checked });
    },

    updatePickBanText: (type, text) => {
      invoke("update_pick_ban_text", { type, text });
    },

    updateSelectedSpell: (spellSlot, spellName) => {
      invoke("update_selected_spell", { spellSlot, spellName });
    },

    updateTrayTooltip: (connectionStatus, gameflowStatus, settings) => {
      invoke("update_tray_tooltip", {
        connectionStatus: connectionStatus,
        gameflowStatus: gameflowStatus,
        settings: settings,
      });
    },

    onAppWillQuit: (callback) => {
      listen("app-will-quit", callback);
    },

    // Backward compatibility with existing code
    on: (channel, callback) => {
      listen(channel, (event) => callback(event.payload));
    },

    send: (channel, data) => {
      invoke(channel, data);
    },

    openExternal: (url) => {
      return open(url);
    },

    ready: true,
  };
})();
