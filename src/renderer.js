document.addEventListener("DOMContentLoaded", () => {
  const elements = Object.freeze({
    fileMenuButton: document.getElementById("file-menu-button"),
    fileDropdownContent: document.getElementById("file-dropdown-content"),
    hideAppButton: document.getElementById("hide-app-button"),
    dockButton: document.getElementById("dock-button"),
    clearPicksBansButton: document.getElementById("clear-picks-bans"),
    settingsSection: document.getElementById("settings-section"),
    pickTextInput: document.getElementById("pick-text-input"),
    banTextInput: document.getElementById("ban-text-input"),
    pickBanStatus: document.getElementById("pick-ban-status"),
    connectionStatus: document.getElementById("connection-status"),
    gameflowStatus: document.getElementById("gameflow-status"),
    assignedRole: document.getElementById("assigned-role"),
    pickNotFoundLabel: document.getElementById("pick-not-found-label"),
    banNotFoundLabel: document.getElementById("ban-not-found-label"),
    spellWarningLabel: document.getElementById("spell-warning-label"),
    themeToggleButton: document.getElementById("theme-toggle"),
    themeIcon: document.getElementById("theme-icon"),
    updateButton: document.getElementById("update-button"),
    testUpdateButton: document.getElementById("test-update-button"),
    updateStatus: document.getElementById("update-status"),
    pickBanSection: document.getElementById("pick-ban-section"),
    pickSuggestions: document.getElementById("pick-suggestions"),
    banSuggestions: document.getElementById("ban-suggestions"),
    currentPicks: document.getElementById("current-picks"),
    currentBans: document.getElementById("current-bans"),
    spell1Dropdown: document.getElementById("spell1-dropdown"),
    spell2Dropdown: document.getElementById("spell2-dropdown"),
    spell1Image: document.getElementById("spell1-image"),
    spell2Image: document.getElementById("spell2-image"),
    autoBraveryCheckbox: document.getElementById("auto-bravery-checkbox"),
    autoAcceptCheckbox: document.getElementById("auto-accept-checkbox"),
    pickBanSelectionCheckbox: document.getElementById(
      "pick-ban-selection-checkbox",
    ),
    spellSelectionCheckbox: document.getElementById("spell-selection-checkbox"),
    closeToTrayCheckbox: document.getElementById("close-to-tray-checkbox"),
  });

  elements.updateButton.classList.add("hidden");
  elements.testUpdateButton.onclick = () => {
    window.tauriAPI.send("test_update");
  };

  let champions = [];
  let summonerSpells = [];
  let selectedSpell1 = null;
  let selectedSpell2 = null;
  let championPicks = [];
  let banPick = null;
  let currentPickSuggestions = new Array(8);
  let currentBanSuggestions = new Array(8);
  let pickSuggestionsCount = 0;
  let banSuggestionsCount = 0;
  let pickHighlightedIndex = -1;
  let banHighlightedIndex = -1;
  let lastIsLeagueRunning = false;
  let currentConnectionStatus = "Starting...";
  let currentGameflowStatus = "Waiting for League...";
  let currentGameMode = "";
  let isLcuConnected = false;
  let normalizedChampionCache = new Map();
  // Set to true once the backend emits "state-ready" after loading persisted
  // settings. Until then, get_game_state() might race with the initial disk load.
  let stateReady = false;

  function debounce(func, delay) {
    let timeoutId;
    let lastArgs;
    let lastThis;

    const debounced = function (...args) {
      lastArgs = args;
      lastThis = this;

      if (timeoutId !== undefined) {
        clearTimeout(timeoutId);
      }

      timeoutId = setTimeout(() => {
        timeoutId = undefined;
        func.apply(lastThis, lastArgs);
      }, delay);
    };

    debounced.cancel = () => {
      if (timeoutId !== undefined) {
        clearTimeout(timeoutId);
        timeoutId = undefined;
      }
    };

    return debounced;
  }

  function throttle(func, limit) {
    let inThrottle;
    return function (...args) {
      if (!inThrottle) {
        func.apply(this, args);
        inThrottle = true;
        setTimeout(() => (inThrottle = false), limit);
      }
    };
  }

  function isArena() {
    return currentGameMode === "CHERRY";
  }

  // Show/hide the pick/ban config panel via the CSS `.expanded` transition
  // rather than display:block/none, so the max-height animation runs and the
  // collapsed styling (max-height:0; opacity:0) keeps it hidden by default.
  function setPickBanExpanded(expanded) {
    if (elements.pickBanSection) {
      elements.pickBanSection.classList.toggle("expanded", !!expanded);
    }
  }

  const settingsChangeHandler = (event) => {
    const target = event.target;
    if (target.type === "checkbox" && target.dataset.setting) {
      // Batch DOM updates
      requestAnimationFrame(() => {
        window.tauriAPI.send("update_checkbox", {
          id: target.dataset.setting,
          checked: target.checked,
        });

        if (target.dataset.setting === "spell-selection") {
          updateSpellWarning();
        }
        if (target.dataset.setting === "pick-ban-selection") {
          setPickBanExpanded(target.checked);
        }

        const currentSettings = {
          autoAccept: document.getElementById("auto-accept-checkbox").checked,
          autoBravery: document.getElementById("auto-bravery-checkbox").checked,
          pickBanSelection: document.getElementById(
            "pick-ban-selection-checkbox",
          ).checked,
          spellSelection: document.getElementById("spell-selection-checkbox")
            .checked,
        };

        window.tauriAPI.updateTrayTooltip(
          currentConnectionStatus,
          currentGameflowStatus,
          currentSettings,
        );
      });
    }
  };

  elements.settingsSection.addEventListener("change", settingsChangeHandler, {
    passive: true,
  });

  const debouncedPickInput = debounce((value) => {
    window.tauriAPI.send("update_pick_ban_text", { type: "pick", text: value });
  }, 300);

  const debouncedBanInput = debounce((value) => {
    window.tauriAPI.send("update_pick_ban_text", { type: "ban", text: value });
  }, 300);

  const debouncedShowPickSuggestions = debounce(showPickSuggestions, 150);
  const debouncedShowBanSuggestions = debounce(showBanSuggestions, 150);

  const pickInputHandler = (event) => {
    const value = event.target.value;
    debouncedPickInput(value);
    debouncedShowPickSuggestions(value);
  };

  const banInputHandler = (event) => {
    const value = event.target.value;
    debouncedBanInput(value);
    debouncedShowBanSuggestions(value);
  };

  elements.pickTextInput.addEventListener("input", pickInputHandler, {
    passive: true,
  });
  elements.banTextInput.addEventListener("input", banInputHandler, {
    passive: true,
  });

  function buildNormalizedChampionCache() {
    normalizedChampionCache.clear();
    for (let i = 0; i < champions.length; i++) {
      const champion = champions[i];
      const normalized = champion.name.toLowerCase().replace(/[ ']/g, "");
      normalizedChampionCache.set(champion.id, normalized);
    }
  }

  function setupCollapsibleSections() {
    const sectionHeaders = document.querySelectorAll(".section-header");
    const clickHandler = (event) => {
      const header = event.currentTarget;
      const section = header.parentElement;
      const sectionId = section.id;

      section.classList.toggle("collapsed");
      const isCollapsed = section.classList.contains("collapsed");
      localStorage.setItem(`${sectionId}-collapsed`, isCollapsed);
    };

    for (let i = 0; i < sectionHeaders.length; i++) {
      const header = sectionHeaders[i];
      const section = header.parentElement;
      const sectionId = section.id;

      if (localStorage.getItem(`${sectionId}-collapsed`) === "true") {
        section.classList.add("collapsed");
      }

      header.addEventListener("click", clickHandler, { passive: true });
    }
  }

  // Apply the theme: dark is the :root default (no body class); light adds
  // the `.light` class to <body>, which style.css keys token overrides off.
  // The icon is CSS-only FA, so swapping the class list just changes the glyph
  // — no SVG cloning (that was the old FA-JS bug).
  function applyTheme(isLight) {
    document.body.classList.toggle("light", isLight);
    localStorage.setItem("theme", isLight ? "light" : "dark");
    // Light mode shows a moon (switch back to dark); dark shows a sun.
    elements.themeIcon.classList.toggle("fa-moon", isLight);
    elements.themeIcon.classList.toggle("fa-sun", !isLight);
  }

  function setupThemeToggle() {
    // Migrate legacy keys: "light-theme"/"light" -> light; anything else
    // (including "dark-theme", "dark", or absent) -> dark (the :root default).
    const stored = localStorage.getItem("theme");
    const isLight = stored === "light-theme" || stored === "light";
    applyTheme(isLight);

    elements.themeToggleButton.addEventListener(
      "click",
      () => {
        requestAnimationFrame(() =>
          applyTheme(!document.body.classList.contains("light")),
        );
      },
      { passive: true },
    );
  }

  async function setupIPCListeners() {
    // Wait for tauriAPI to be available and ready
    while (!window.tauriAPI || !window.tauriAPI.ready) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }

    window.tauriAPI.on("update-available", (data) => {
      // Fill and show the update modal with version + release notes.
      // The footer also indicates the update so it's visible without opening the modal.
      elements.updateStatus.textContent = `Update available: v${data.version}`;
      elements.updateButton.classList.remove("hidden");
      elements.updateButton.onclick = () => showUpdateModal();

      const versionText = document.getElementById("update-version-text");
      if (versionText) {
        versionText.innerHTML = `A new version <strong>v${data.version}</strong> is available.`;
      }

      const notesEl = document.getElementById("update-notes-text");
      if (notesEl) {
        notesEl.textContent = data.notes || "No release notes provided.";
      }

      const nowBtn = document.getElementById("update-now-btn");
      if (nowBtn) {
        nowBtn.dataset.updateUrl = data.url || "";
      }
      showUpdateModal();
    });

    window.tauriAPI.on("state-ready", () => {
    if (!stateReady) {
      console.log("[state] state-ready received, re-fetching game state");
      stateReady = true;
    }
  });

    window.tauriAPI.on("checking-for-updates", () => {
      elements.updateStatus.textContent = "Checking for updates...";
    });

    // Fired after every update check (whether or not an update was found).
    // For the "already up to date" path, "update-available" is never received,
    // so this is the only chance to restore the footer text.
    window.tauriAPI.on("check-updates-complete", () => {
      const current = elements.updateStatus.textContent;
      // Only reset if the check resolved in the "already up to date" path —
      // i.e. the status never progressed beyond "Checking for updates...".
      if (current === "Checking for updates...") {
        elements.updateStatus.textContent = "Program is up to date.";
      }
    });

    window.tauriAPI.on("close-requested", () => {
      // Read the current close-to-tray preference from the checkbox's
      // data-initialized attribute. If the user has never set this preference
      // (backend value is null), show a one-time dialog.
      const cb = document.getElementById("close-to-tray-checkbox");
      if (cb.dataset.initialized !== "true") {
        document.getElementById("close-pref-dialog").classList.add("show");
        return;
      }
      if (cb.checked) {
        window.tauriAPI.send("hide_app");
      } else {
        window.tauriAPI.send("close_app");
      }
    });

    // One-time close preference dialog buttons also save the choice.
    document.getElementById("close-pref-quit").addEventListener("click", () => {
      document.getElementById("close-pref-dialog").classList.remove("show");
      window.tauriAPI.send("update_checkbox", { id: "close-to-tray", checked: false });
      window.tauriAPI.send("close_app");
    });
    document.getElementById("close-pref-tray").addEventListener("click", () => {
      document.getElementById("close-pref-dialog").classList.remove("show");
      window.tauriAPI.send("update_checkbox", { id: "close-to-tray", checked: true });
      window.tauriAPI.send("hide_app");
    });

    window.tauriAPI.onChampionsUpdated(async () => {
      // A live LCU refresh just succeeded; re-fetch champion data so autocomplete
      // stays in sync — especially important when new champions are released.
      try {
        const data = await window.tauriAPI.getChampionsAndSpells();
        champions = data.champions || [];
        buildNormalizedChampionCache();
      } catch (e) {
        console.error("Failed to refresh champions:", e);
      }
    });

    window.tauriAPI.on("status-update", (data) => {
      // Batch DOM updates
      requestAnimationFrame(() => {
        updateConnectionStatus(data);
        updateControlStates();
        if (data.gameflowStatus !== undefined) {
          currentGameflowStatus = data.gameflowStatus;
          elements.gameflowStatus.textContent = data.gameflowStatus;
        }
        if (data.assignedRole !== undefined) {
          updateAssignedRole(data.assignedRole);
        }
        if (data.gameMode !== undefined && data.gameMode !== currentGameMode) {
          currentGameMode = data.gameMode;
          updateControlStates();
        }

        // Update checkbox states based on main process settings
        if (data.settings) {
          document.getElementById("auto-accept-checkbox").checked =
            data.settings.autoAccept;
          document.getElementById("auto-bravery-checkbox").checked =
            data.settings.autoBravery;
          document.getElementById("pick-ban-selection-checkbox").checked =
            data.settings.pickBanSelection;
          document.getElementById("spell-selection-checkbox").checked =
            data.settings.spellSelection;
          const ctt = document.getElementById("close-to-tray-checkbox");
            if (data.settings.closeToTray != null) {
              ctt.checked = data.settings.closeToTray;
              ctt.dataset.initialized = "true";
            }
          const dcm = document.getElementById("docker-mode-checkbox");
          if (data.settings.dockerMode != null) dcm.checked = data.settings.dockerMode;
          document.body.classList.toggle("docker-mode", dcm.checked);
          updateDockMenuButton(dcm.checked);

          // Ensure pick/ban section visibility stays in sync with the setting.
          setPickBanExpanded(data.settings.pickBanSelection);
          updateSpellWarning();
        }
      });
    });
  }

  function updateDockMenuButton(docked) {
    const dockBtn = document.getElementById("dock-button");
    const hideBtn = document.getElementById("hide-app-button");
    if (docked) {
      dockBtn.style.display = "none";
      hideBtn.innerHTML = '<i class="fa-solid fa-link-slash"></i> Undock';
    } else {
      dockBtn.style.display = "";
      hideBtn.innerHTML = '<i class="fa-solid fa-eye-slash"></i> Hide App';
    }
  }

  async function fetchAndInitializeData() {
    // Wait for the backend to finish loading persisted state from disk.
    // This prevents get_game_state() from returning default values when it races
    // ahead of the async settings load in lib.rs.
    if (!stateReady) {
      console.log("[state] waiting for state-ready...");
      await new Promise((resolve) => {
        const timeout = setTimeout(resolve, 5000); // 5s fallback
        window.tauriAPI.once("state-ready", () => {
          clearTimeout(timeout);
          stateReady = true;
          resolve();
        });
      });
      console.log("[state] state-ready confirmed, fetching game state");
    }

    try {
      const initialData = await window.tauriAPI.getChampionsAndSpells();
      console.log("Fetched initial data:", initialData);
      champions = initialData.champions || [];
      summonerSpells = initialData.summonerSpells || [];
      console.log("Champions loaded:", champions.length);
      console.log("Spells loaded:", summonerSpells.length);
      buildNormalizedChampionCache();
      populateSpellSelection();

      const gameState = await window.tauriAPI.getCurrentGameState();
      console.log("Fetched game state:", gameState);
      updateConnectionStatus(gameState);
      if (gameState.gameflowStatus !== undefined) {
        currentGameflowStatus = gameState.gameflowStatus;
        elements.gameflowStatus.textContent = gameState.gameflowStatus;
      }
      if (gameState.assignedRole !== undefined) {
        updateAssignedRole(gameState.assignedRole);
      }
      if (gameState.gameMode !== undefined) {
        currentGameMode = gameState.gameMode;
      }
      if (gameState.settings) {
        document.getElementById("auto-accept-checkbox").checked =
          gameState.settings.autoAccept;
        document.getElementById("auto-bravery-checkbox").checked =
          gameState.settings.autoBravery;
        document.getElementById("pick-ban-selection-checkbox").checked =
          gameState.settings.pickBanSelection;
        document.getElementById("spell-selection-checkbox").checked =
          gameState.settings.spellSelection;
        const ctt = document.getElementById("close-to-tray-checkbox");
        if (gameState.settings.closeToTray != null) {
          ctt.checked = gameState.settings.closeToTray;
          ctt.dataset.initialized = "true";
        }
        const dcm = document.getElementById("docker-mode-checkbox");
        if (gameState.settings.dockerMode != null) dcm.checked = gameState.settings.dockerMode;
        document.body.classList.toggle("docker-mode", dcm.checked);
        updateDockMenuButton(dcm.checked);

        // Initialize autostart checkbox state from registry
        try {
          const autostartEnabled = await window.tauriAPI.send("get_autostart_state");
          console.log("[autostart] init state from registry:", autostartEnabled);
          document.getElementById("open-on-start-checkbox").checked = autostartEnabled;
          // "Start Minimized to Tray" is a sub-option of Open on System Start,
          // so it follows the autostart toggle's state.
          updateStartMinimizedVisibility(autostartEnabled);
          const minCb = document.getElementById("start-minimized-checkbox");
          console.log(
            "[autostart] startMinimized from settings:",
            gameState.settings.startMinimized,
          );
          if (minCb && gameState.settings.startMinimized != null) {
            console.log("[autostart] initializing checkbox to:", gameState.settings.startMinimized);
            minCb.checked = gameState.settings.startMinimized;
          } else {
            console.log("[autostart] startMinimized is null/undefined — checkbox left at default");
          }
        } catch (e) {
          console.error("[autostart] init failed:", e);
          updateStartMinimizedVisibility(false);
        }

        restoreSpellDropdowns(gameState.settings);
        setPickBanExpanded(gameState.settings.pickBanSelection);
        updateSpellWarning();
      }
      updateControlStates();

      if (
        gameState.connectionStatus &&
        gameState.gameflowStatus &&
        gameState.settings
      ) {
        window.tauriAPI.updateTrayTooltip(
          gameState.connectionStatus,
          gameState.gameflowStatus,
          gameState.settings,
        );
      }
    } catch (error) {
      console.error("Failed to fetch initial data or game state:", error);
    }
  }

  function updateConnectionStatus(data) {
    if (data.isLeagueRunning !== undefined) {
      lastIsLeagueRunning = data.isLeagueRunning;
    }

    if (data.connectionStatus !== undefined) {
      currentConnectionStatus = data.connectionStatus;
      isLcuConnected = currentConnectionStatus === "Connected";
      const statusText = lastIsLeagueRunning
        ? `${currentConnectionStatus || "League Running"}`
        : `${currentConnectionStatus || "League not running"}`;
      elements.connectionStatus.textContent = statusText;
    }

    elements.connectionStatus.className = lastIsLeagueRunning
      ? "status-connected"
      : "status-disconnected";
  }

  function updateControlStates() {
    const controls = [
      elements.autoAcceptCheckbox,
      elements.pickBanSelectionCheckbox,
      elements.spellSelectionCheckbox,
      elements.clearPicksBansButton,
      elements.pickTextInput,
      elements.banTextInput,
      elements.spell1Dropdown,
      elements.spell2Dropdown,
    ];
    for (const el of controls) {
      if (el) el.disabled = !isLcuConnected;
    }

    // Bravery is additionally gated on Arena mode.
    if (elements.autoBraveryCheckbox) {
      elements.autoBraveryCheckbox.disabled = !(isLcuConnected && isArena());
    }
  }

  function updateAssignedRole(role) {
    const card = elements.assignedRole.closest(".status-card");
    if (role) {
      elements.assignedRole.textContent = `Role: ${role}`;
      if (card) card.style.display = "";
    } else {
      if (card) card.style.display = "none";
    }
  }

  function setupButtonHandlers() {
    const clearHandler = async () => {
      // Batch all updates
      championPicks.length = 0;
      banPick = null;
      elements.pickTextInput.value = "";
      elements.banTextInput.value = "";

      requestAnimationFrame(async () => {
        updatePickBanDisplay();
        await window.tauriAPI.send("clear_picks_bans");
        showTemporaryLabel(
          elements.pickBanStatus,
          "Picks and bans cleared.",
          3000,
        );
      });
    };

    elements.clearPicksBansButton.addEventListener("click", clearHandler, {
      passive: true,
    });

    elements.fileMenuButton.addEventListener(
      "click",
      (event) => {
        event.stopPropagation();
        elements.fileDropdownContent.classList.toggle("show");
      },
      { passive: true },
    );

    elements.hideAppButton.addEventListener(
      "click",
      () => {
        const docked = document.getElementById("docker-mode-checkbox").checked;
        if (docked) {
          document.getElementById("docker-mode-checkbox").checked = false;
          window.tauriAPI.send("update_checkbox", {
            id: "docker-mode",
            checked: false,
          });
        } else {
          window.tauriAPI.send("hide_app");
        }
        elements.fileDropdownContent.classList.remove("show");
      },
      { passive: true },
    );

    elements.dockButton.addEventListener(
      "click",
      () => {
        document.getElementById("docker-mode-checkbox").checked = true;
        window.tauriAPI.send("update_checkbox", {
          id: "docker-mode",
          checked: true,
        });
        elements.fileDropdownContent.classList.remove("show");
      },
      { passive: true },
    );

    document.getElementById("settings-button").addEventListener(
      "click",
      () => {
        elements.fileDropdownContent.classList.remove("show");
        showSettingsModal();
      },
      { passive: true },
    );

    document.addEventListener(
      "click",
      (event) => {
        if (!event.target.closest(".dropdown")) {
          elements.fileDropdownContent.classList.remove("show");
        }
      },
      { passive: true },
    );

    const aboutButton = document.getElementById("about-button");
    aboutButton?.addEventListener("click", showAboutModal, { passive: true });
  }

  function setupInputEventListeners() {
    const pickFocusHandler = (event) => {
      const value = event.target.value.trim();
      if (value) {
        debouncedShowPickSuggestions(value);
      }
    };

    const banFocusHandler = (event) => {
      const value = event.target.value.trim();
      if (value) {
        debouncedShowBanSuggestions(value);
      }
    };

    const pickBlurHandler = () => {
      setTimeout(() => {
        hidePickSuggestions();
        debouncedPickInput.cancel();
        debouncedShowPickSuggestions.cancel();
      }, 150);
    };

    const banBlurHandler = () => {
      setTimeout(() => {
        hideBanSuggestions();
        debouncedBanInput.cancel();
        debouncedShowBanSuggestions.cancel();
      }, 150);
    };

    elements.pickTextInput.addEventListener("focus", pickFocusHandler, {
      passive: true,
    });
    elements.pickTextInput.addEventListener("blur", pickBlurHandler, {
      passive: true,
    });
    elements.pickTextInput.addEventListener("keydown", handlePickKeydown);

    elements.banTextInput.addEventListener("focus", banFocusHandler, {
      passive: true,
    });
    elements.banTextInput.addEventListener("blur", banBlurHandler, {
      passive: true,
    });
    elements.banTextInput.addEventListener("keydown", handleBanKeydown);
  }

  function handlePickKeydown(event) {
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        debouncedShowPickSuggestions.cancel();
        navigatePickSuggestions(1);
        break;
      case "ArrowUp":
        event.preventDefault();
        debouncedShowPickSuggestions.cancel();
        navigatePickSuggestions(-1);
        break;
      case "ArrowRight":
        if (
          pickSuggestionsCount > 0 &&
          event.target.selectionStart === event.target.value.length
        ) {
          event.preventDefault();
          const suggestionToFill =
            pickHighlightedIndex >= 0
              ? currentPickSuggestions[pickHighlightedIndex]
              : currentPickSuggestions[0];
          event.target.value = suggestionToFill.name;
          hidePickSuggestions();
          requestAnimationFrame(() => {
            event.target.setSelectionRange(
              event.target.value.length,
              event.target.value.length,
            );
          });
        }
        break;
      case "Enter":
        event.preventDefault();
        handlePickEnter(event.target);
        break;
      case "Escape":
        hidePickSuggestions();
        break;
    }
  }

  function handleBanKeydown(event) {
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        debouncedShowBanSuggestions.cancel();
        navigateBanSuggestions(1);
        break;
      case "ArrowUp":
        event.preventDefault();
        debouncedShowBanSuggestions.cancel();
        navigateBanSuggestions(-1);
        break;
      case "ArrowRight":
        if (
          banSuggestionsCount > 0 &&
          event.target.selectionStart === event.target.value.length
        ) {
          event.preventDefault();
          const suggestionToFill =
            banHighlightedIndex >= 0
              ? currentBanSuggestions[banHighlightedIndex]
              : currentBanSuggestions[0];
          event.target.value = suggestionToFill.name;
          hideBanSuggestions();
          requestAnimationFrame(() => {
            event.target.setSelectionRange(
              event.target.value.length,
              event.target.value.length,
            );
          });
        }
        break;
      case "Enter":
        event.preventDefault();
        handleBanEnter(event.target);
        break;
      case "Escape":
        hideBanSuggestions();
        break;
    }
  }

  function handlePickEnter(input) {
    if (pickHighlightedIndex >= 0 && pickSuggestionsCount > 0) {
      selectPickChampion(currentPickSuggestions[pickHighlightedIndex]);
    } else if (pickSuggestionsCount > 0) {
      // Select first suggestion if suggestions are visible but none highlighted
      selectPickChampion(currentPickSuggestions[0]);
    } else {
      const text = input.value.trim().toLowerCase().replace(/[ ']/g, "");
      if (text === "") {
        if (championPicks.length < 2) {
          championPicks.push({ id: 0, name: "" });
          updatePickBanDisplay();
        }
      } else {
        const matchingChampion = findChampionByNormalizedName(text);
        if (matchingChampion) {
          selectPickChampion(matchingChampion);
        } else {
          showTemporaryLabel(
            elements.pickNotFoundLabel,
            "No champion found.",
            1500,
          );
        }
      }
    }
  }

  function handleBanEnter(input) {
    if (banHighlightedIndex >= 0 && banSuggestionsCount > 0) {
      selectBanChampion(currentBanSuggestions[banHighlightedIndex]);
    } else if (banSuggestionsCount > 0) {
      // Select first suggestion if suggestions are visible but none highlighted
      selectBanChampion(currentBanSuggestions[0]);
    } else {
      const text = input.value.trim().toLowerCase().replace(/[ ']/g, "");
      if (text === "") {
        banPick = { id: 0, name: "" };
        updatePickBanDisplay();
      } else {
        const matchingChampion = findChampionByNormalizedName(text);
        if (matchingChampion) {
          selectBanChampion(matchingChampion);
        } else {
          showTemporaryLabel(
            elements.banNotFoundLabel,
            "No champion found.",
            1500,
          );
        }
      }
    }
  }

  function findChampionByNormalizedName(normalizedText) {
    for (let i = 0; i < champions.length; i++) {
      const champion = champions[i];
      if (normalizedChampionCache.get(champion.id) === normalizedText) {
        return champion;
      }
    }
    return null;
  }

  function showPickSuggestions(query) {
    showSuggestions(
      elements.pickSuggestions,
      query,
      champions,
      currentPickSuggestions,
      selectPickChampion,
      (count) => {
        pickSuggestionsCount = count;
      },
    );
    pickHighlightedIndex = -1;
  }

  function hidePickSuggestions() {
    hideSuggestions(elements.pickSuggestions);
    pickSuggestionsCount = 0;
    pickHighlightedIndex = -1;
  }

  function navigatePickSuggestions(direction) {
    pickHighlightedIndex = navigateSuggestions(
      elements.pickSuggestions,
      currentPickSuggestions,
      pickHighlightedIndex,
      direction,
      pickSuggestionsCount,
    );
  }

  function showBanSuggestions(query) {
    showSuggestions(
      elements.banSuggestions,
      query,
      champions,
      currentBanSuggestions,
      selectBanChampion,
      (count) => {
        banSuggestionsCount = count;
      },
    );
    banHighlightedIndex = -1;
  }

  function hideBanSuggestions() {
    hideSuggestions(elements.banSuggestions);
    banSuggestionsCount = 0;
    banHighlightedIndex = -1;
  }

  function navigateBanSuggestions(direction) {
    banHighlightedIndex = navigateSuggestions(
      elements.banSuggestions,
      currentBanSuggestions,
      banHighlightedIndex,
      direction,
      banSuggestionsCount,
    );
  }

  function showSuggestions(
    dropdown,
    query,
    source,
    suggestions,
    selectHandler,
    setCount,
  ) {
    if (!query.trim()) {
      hideSuggestions(dropdown);
      setCount(0);
      return;
    }

    const normalizedQuery = query.toLowerCase().replace(/[ ']/g, "");
    let count = 0;

    // Optimized search with early termination
    for (let i = 0; i < source.length && count < 8; i++) {
      const champion = source[i];
      const normalizedName = normalizedChampionCache.get(champion.id);
      if (normalizedName && normalizedName.includes(normalizedQuery)) {
        suggestions[count] = champion;
        count++;
      }
    }

    setCount(count);

    if (count === 0) {
      hideSuggestions(dropdown);
      return;
    }

    const fragment = document.createDocumentFragment();
    for (let i = 0; i < count; i++) {
      const champion = suggestions[i];
      const item = document.createElement("div");
      item.className = "suggestion-item";
      item.innerHTML = `<span class="champion-name">${champion.name}</span><span class="champion-id">ID: ${champion.id}</span>`;

      // Use closure to capture champion reference
      item.addEventListener(
        "click",
        (
          (c) => () =>
            selectHandler(c)
        )(champion),
        { passive: true },
      );
      fragment.appendChild(item);
    }

    // Batch DOM update
    requestAnimationFrame(() => {
      dropdown.innerHTML = "";
      dropdown.appendChild(fragment);
      dropdown.classList.add("show");
    });
  }

  function hideSuggestions(dropdown) {
    dropdown.classList.remove("show");
  }

  function navigateSuggestions(
    dropdown,
    suggestions,
    highlightedIndex,
    direction,
    count,
  ) {
    if (count === 0) return highlightedIndex;

    const items = dropdown.children;

    // Remove current highlighting
    if (highlightedIndex >= 0 && highlightedIndex < items.length) {
      items[highlightedIndex].classList.remove("highlighted");
    }

    // Calculate new index
    highlightedIndex += direction;
    if (highlightedIndex >= count) {
      highlightedIndex = 0;
    } else if (highlightedIndex < 0) {
      highlightedIndex = count - 1;
    }

    // Add highlighting to new item
    if (highlightedIndex >= 0 && highlightedIndex < items.length) {
      items[highlightedIndex].classList.add("highlighted");
      items[highlightedIndex].scrollIntoView({
        block: "nearest",
        behavior: "smooth",
      });
    }

    return highlightedIndex;
  }

  function selectPickChampion(champion) {
    if (championPicks.length >= 2) {
      showTemporaryLabel(
        elements.pickNotFoundLabel,
        "Maximum picks reached (2).",
        1500,
      );
      return;
    }

    for (let i = 0; i < championPicks.length; i++) {
      if (championPicks[i].id === champion.id) {
        showTemporaryLabel(
          elements.pickNotFoundLabel,
          "Champion already selected.",
          1500,
        );
        return;
      }
    }

    championPicks.push(champion);
    elements.pickTextInput.value = "";
    hidePickSuggestions();
    updatePickBanDisplay();

    // Send the full champion name to the backend
    window.tauriAPI.send("update_pick_ban_text", {
      type: "pick",
      text: champion.name,
    });
  }

  function selectBanChampion(champion) {
    // Check if champion is already picked
    for (let i = 0; i < championPicks.length; i++) {
      if (championPicks[i].id === champion.id) {
        showTemporaryLabel(
          elements.banNotFoundLabel,
          "Champion already selected as pick.",
          1500,
        );
        return;
      }
    }

    banPick = champion;
    elements.banTextInput.value = "";
    hideBanSuggestions();
    updatePickBanDisplay();

    window.tauriAPI.send("update_pick_ban_text", {
      type: "ban",
      text: champion.name,
    });
  }

  function removePickChampion(championId) {
    const index = championPicks.findIndex((p) => p.id === championId);
    if (index > -1) {
      championPicks.splice(index, 1);
      updatePickBanDisplay();
      window.tauriAPI.send("remove_champion_pick", { championId });
    }
  }

  function removeBanChampion() {
    banPick = null;
    updatePickBanDisplay();
    window.tauriAPI.send("remove_champion_ban");
  }

  function updatePickBanDisplay() {
    // Batch DOM updates
    requestAnimationFrame(() => {
      updateDisplay(
        elements.currentPicks,
        "Picks:",
        championPicks,
        removePickChampion,
      );
      updateDisplay(
        elements.currentBans,
        "Ban:",
        banPick ? [banPick] : [],
        removeBanChampion,
      );
    });
  }

  function updateDisplay(container, headerText, items, removeHandler) {
    // Use DocumentFragment for better performance
    const fragment = document.createDocumentFragment();

    if (items.length > 0) {
      const header = document.createElement("strong");
      header.textContent = headerText;
      fragment.appendChild(header);

      for (let i = 0; i < items.length; i++) {
        const itemElement = createChampionItem(items[i], i, removeHandler);
        fragment.appendChild(itemElement);
      }
    }

    container.innerHTML = "";
    container.appendChild(fragment);
  }

  function createChampionItem(champion, index, removeHandler) {
    const championItem = document.createElement("div");
    championItem.className = "champion-item";

    const championInfo = document.createElement("span");
    championInfo.className = "champion-info";
    championInfo.textContent = champion.name
      ? `${champion.name} (ID: ${champion.id})`
      : "None (Skip)";

    const removeBtn = document.createElement("button");
    removeBtn.className = "remove-btn";
    removeBtn.innerHTML = "×";
    removeBtn.title = "Remove champion";
    removeBtn.addEventListener("click", () => removeHandler(champion.id), {
      passive: true,
    });

    championItem.appendChild(championInfo);
    championItem.appendChild(removeBtn);
    return championItem;
  }

  function populateSpellSelection() {
    const spellDropdowns = [elements.spell1Dropdown, elements.spell2Dropdown];

    // Create options fragment once and clone
    const fragment = document.createDocumentFragment();
    const defaultOption = document.createElement("option");
    defaultOption.value = "";
    defaultOption.textContent = "None";
    fragment.appendChild(defaultOption);

    for (let i = 0; i < summonerSpells.length; i++) {
      const spell = summonerSpells[i];
      const option = document.createElement("option");
      option.value = spell.name;
      option.textContent = spell.name;
      fragment.appendChild(option);
    }

    // Populate both dropdowns
    spellDropdowns.forEach((dropdown) => {
      dropdown.innerHTML = "";
      dropdown.appendChild(fragment.cloneNode(true));
    });

    const spell1ChangeHandler = (event) => {
      selectedSpell1 = event.target.value;
      const imageSrc = selectedSpell1
        ? `./images/${selectedSpell1.toLowerCase()}.webp`
        : "./images/no_icon.webp";
      elements.spell1Image.src = imageSrc;
      window.tauriAPI.send("update_selected_spell", {
        spellSlot: 1,
        spellName: selectedSpell1,
      });
      updateSpellWarning();
    };

    const spell2ChangeHandler = (event) => {
      selectedSpell2 = event.target.value;
      const imageSrc = selectedSpell2
        ? `./images/${selectedSpell2.toLowerCase()}.webp`
        : "./images/no_icon.webp";
      elements.spell2Image.src = imageSrc;
      window.tauriAPI.send("update_selected_spell", {
        spellSlot: 2,
        spellName: selectedSpell2,
      });
      updateSpellWarning();
    };

    elements.spell1Dropdown.addEventListener("change", spell1ChangeHandler, {
      passive: true,
    });
    elements.spell2Dropdown.addEventListener("change", spell2ChangeHandler, {
      passive: true,
    });

    // Set default images
    elements.spell1Image.src = "./images/no_icon.webp";
    elements.spell2Image.src = "./images/no_icon.webp";
  }

  function updateSpellWarning() {
    const checkbox = document.getElementById("spell-selection-checkbox");
    const isSpellSelectionOn = checkbox && checkbox.checked;
    const shouldShow =
      isSpellSelectionOn && (!selectedSpell1 || !selectedSpell2);
    elements.spellWarningLabel.style.display = shouldShow ? "block" : "none";
  }

  // On launch the backend hands back the persisted summoner spells as spell
  // IDs (e.g. "SummonerFlash"), but the dropdowns are keyed by spell name and
  // the icons are named off the lowercased name ("<name>.webp"). Map each
  // stored spell back to its name, select it in the dropdown, and show the
  // matching icon so a previously-chosen spell pair survives a restart.
  function restoreSpellDropdowns(settings) {
    if (!settings || !summonerSpells || summonerSpells.length === 0) return;

    const restoreSlot = (id, dropdown, image, setter) => {
      if (!id) return;
      const spell = summonerSpells.find((s) => s.id === id);
      if (!spell) return;
      if (dropdown) dropdown.value = spell.name;
      if (image) image.src = `./images/${spell.name.toLowerCase()}.webp`;
      setter(spell.name);
    };

    restoreSlot(
      settings.selectedSpell1,
      elements.spell1Dropdown,
      elements.spell1Image,
      (v) => { selectedSpell1 = v; },
    );
    restoreSlot(
      settings.selectedSpell2,
      elements.spell2Dropdown,
      elements.spell2Image,
      (v) => { selectedSpell2 = v; },
    );

    updateSpellWarning();
  }

  const activeLabelTimeouts = new Set();

  function showTemporaryLabel(element, message, duration) {
    // Clear any existing timeout for this element
    activeLabelTimeouts.forEach((timeoutId) => {
      if (element.dataset.timeoutId === String(timeoutId)) {
        clearTimeout(timeoutId);
        activeLabelTimeouts.delete(timeoutId);
      }
    });

    element.textContent = message;
    element.style.display = "block";

    const timeoutId = setTimeout(() => {
      element.style.display = "none";
      element.textContent = "";
      element.dataset.timeoutId = "";
      activeLabelTimeouts.delete(timeoutId);
    }, duration);

    element.dataset.timeoutId = String(timeoutId);
    activeLabelTimeouts.add(timeoutId);
  }

  function setInitialDisplay() {
    // Batch initial display setup
    const elementsToHide = [
      elements.pickNotFoundLabel,
      elements.banNotFoundLabel,
      elements.spellWarningLabel,
    ];

    elementsToHide.forEach((element) => {
      element.style.display = "none";
    });

    const globalClickHandler = (event) => {
      if (!event.target.closest(".autocomplete-container")) {
        hidePickSuggestions();
        hideBanSuggestions();
      }
    };

    document.addEventListener("click", globalClickHandler, {
      passive: true,
      capture: true,
    });
  }

  function cleanup() {
    debouncedPickInput.cancel();
    debouncedBanInput.cancel();
    debouncedShowPickSuggestions.cancel();
    debouncedShowBanSuggestions.cancel();

    activeLabelTimeouts.forEach((timeoutId) => {
      clearTimeout(timeoutId);
    });
    activeLabelTimeouts.clear();

    normalizedChampionCache.clear();
  }

  // Listen for page unload to cleanup
  window.addEventListener("beforeunload", cleanup, { passive: true });

  function showAboutModal() {
    const modal = document.getElementById("about-modal");
    if (modal) {
      modal.classList.add("show");
      // Prevent body scroll when modal is open
      document.body.style.overflow = "hidden";
    }
  }

  function hideAboutModal() {
    const modal = document.getElementById("about-modal");
    if (modal) {
      modal.classList.remove("show");
      // Restore body scroll
      document.body.style.overflow = "";
    }
  }

  function setupAboutModal() {
    const modal = document.getElementById("about-modal");
    const closeButton = document.getElementById("close-about");

    closeButton?.addEventListener("click", hideAboutModal);

    // Close modal when clicking outside of it
    modal?.addEventListener("click", (event) => {
      if (event.target === modal) {
        hideAboutModal();
      }
    });

    // Handle external links - open in default browser
    const aboutLinks = modal?.querySelectorAll('a[href^="http"]');
    aboutLinks?.forEach((link) => {
      link.addEventListener("click", async (event) => {
        event.preventDefault();
        const url = link.getAttribute("href");
        if (url) {
          try {
            // Use Tauri shell API to open URL in default browser
            await window.tauriAPI.openExternal(url);
            console.log("Successfully opened external link:", url);
          } catch (error) {
            console.error("Failed to open external link:", error);
            // Fallback: copy to clipboard if available
            if (navigator.clipboard) {
              try {
                await navigator.clipboard.writeText(url);
                console.log("URL copied to clipboard as fallback:", url);
                // Show temporary notification to user
                const linkText = link.textContent;
                alert(
                  `Could not open ${linkText}. URL copied to clipboard: ${url}`,
                );
              } catch (clipboardError) {
                console.error("Failed to copy to clipboard:", clipboardError);
                alert(`Could not open link: ${url}`);
              }
            } else {
              alert(`Could not open link: ${url}`);
            }
          }
        }
      });
    });
  }

  function setupUpdateModal() {
    const closeBtn = document.getElementById("close-update");
    const laterBtn = document.getElementById("update-later-btn");
    const nowBtn = document.getElementById("update-now-btn");
    const modal = document.getElementById("update-modal");

    closeBtn?.addEventListener("click", hideUpdateModal);
    laterBtn?.addEventListener("click", hideUpdateModal);

    modal?.addEventListener("click", (event) => {
      if (event.target === modal) {
        hideUpdateModal();
      }
    });

    // "Update Now" — download and install the update.
    nowBtn?.addEventListener("click", () => {
      hideUpdateModal();
      // Stored by the update-available handler below when it populates the modal.
      const pendingUrl = nowBtn.dataset.updateUrl;
      if (pendingUrl) {
        elements.updateStatus.textContent = "Updating...";
        window.tauriAPI.send("run_updater", { url: pendingUrl });
      }
    });
  }

  function showSettingsModal() {
    const modal = document.getElementById("settings-modal");
    if (modal) {
      modal.classList.add("show");
      document.body.style.overflow = "hidden";
    }
  }

  function hideSettingsModal() {
    const modal = document.getElementById("settings-modal");
    if (modal) {
      modal.classList.remove("show");
      document.body.style.overflow = "";
    }
  }

  function showUpdateModal() {
    const modal = document.getElementById("update-modal");
    if (modal) {
      modal.classList.add("show");
      document.body.style.overflow = "hidden";
    }
  }

  function hideUpdateModal() {
    const modal = document.getElementById("update-modal");
    if (modal) {
      modal.classList.remove("show");
      document.body.style.overflow = "";
    }
  }

  // "Start Minimized to Tray" is a sub-option of "Open on System Start" and is
  // only shown while autostart is enabled. Toggles the row's visibility.
  function updateStartMinimizedVisibility(autostartEnabled) {
    const group = document.getElementById("start-minimized-group");
    if (group) {
      group.classList.toggle("hidden", !autostartEnabled);
    }
  }

  function setupSettingsModal() {
    const modal = document.getElementById("settings-modal");
    const closeButton = document.getElementById("close-settings");

    closeButton?.addEventListener("click", hideSettingsModal);

    modal?.addEventListener("click", (event) => {
      if (event.target === modal) {
        hideSettingsModal();
      }
    });

    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && modal?.classList.contains("show")) {
        hideSettingsModal();
      }
    });

    // Close to tray toggle — persists via the backend.
    document.getElementById("close-to-tray-checkbox").addEventListener("change", (event) => {
      event.target.dataset.initialized = "true";
      window.tauriAPI.send("update_checkbox", {
        id: "close-to-tray",
        checked: event.target.checked,
      });
    });

    // Dock to League Client toggle — persists via the backend.
    document.getElementById("docker-mode-checkbox").addEventListener("change", (event) => {
      window.tauriAPI.send("update_checkbox", {
        id: "docker-mode",
        checked: event.target.checked,
      });
    });

    // Open on system start toggle
    document.getElementById("open-on-start-checkbox").addEventListener("change", async (event) => {
      const enabled = event.target.checked;
      console.log("[autostart] checkbox changed to:", enabled);
      try {
        await window.tauriAPI.send("toggle_autostart", { enabled });
        console.log("[autostart] toggle succeeded");
        // The "Start Minimized to Tray" sub-option only applies while autostart
        // is on; toggle its row, and reset the setting when turning autostart off.
        updateStartMinimizedVisibility(enabled);
        if (!enabled) {
          const minCb = document.getElementById("start-minimized-checkbox");
          if (minCb && minCb.checked) {
            minCb.checked = false;
            window.tauriAPI.send("update_checkbox", { id: "start-minimized", checked: false });
          }
        }
      } catch (e) {
        console.error("[autostart] toggle failed:", e);
        event.target.checked = !enabled;
        console.log("[autostart] reverted checkbox to:", event.target.checked);
        updateStartMinimizedVisibility(event.target.checked);
      }
    });

    // "Start Minimized to Tray" sub-option — persists via the backend.
    document.getElementById("start-minimized-checkbox").addEventListener("change", (event) => {
      window.tauriAPI.send("update_checkbox", {
        id: "start-minimized",
        checked: event.target.checked,
      });
    });
  }

  async function init() {
    setupCollapsibleSections();
    setupThemeToggle();
    await setupIPCListeners();
    setupButtonHandlers();
    setupInputEventListeners();
    setupAboutModal();
    setupUpdateModal();
    setupSettingsModal();
    fetchAndInitializeData();
    setInitialDisplay();
    window.tauriAPI.send("frontend_ready");
  }

  (async () => {
    await init();
  })();
});
