//! "Dock to League Client" — attaches Watcher to the left edge of the League
//! client window so the two read as one combined `[ Watcher | League ]` window.
//!
//! Watcher's *own* window is driven entirely through Tauri's cross-platform API;
//! the only Win32 here is *observing* the League client's window — finding its
//! HWND by process ID (the same `LeagueClientUx.exe` name the connection loop in
//! `connection.rs` already monitors) and reading its rect + minimized state. A
//! short polling loop (cheap user32 calls on the hot path; a `sysinfo` PID lookup
//! only when the cached HWND goes stale, throttled to ~2 s) keeps Watcher docked
//! to the client's left edge + top, matching the client's height.
//!
//! While docked we drop the taskbar icon (`skip_taskbar`), go always-on-top,
//! remove the title bar (`set_decorations(false)`) and lock resizing, and
//! hide/show with the client's lifecycle (hide when the client is minimized or
//! closed; reappear when it reopens). On disable we restore the standalone
//! window look + the geometry snapshotted before docking.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Docked panel width, kept in sync with the build-time `inner_size` in
/// `lib.rs`. The panel does not itself scale — it keeps this width and tracks
/// the client's left edge + height.
const PANEL_W: i32 = 220;

#[cfg(windows)]
mod ffi {
    use super::PANEL_W;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use tauri::{Manager, PhysicalPosition, PhysicalSize};
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic,
        IsWindow, IsWindowVisible,
    };

    /// A snapshot of the League client's window: its HWND (as a Send-safe
    /// `usize`) plus the screen-coordinate rect and whether it's minimized.
    pub(super) struct LeagueWindow {
        pub hwnd: usize,
        pub left: i32,
        pub top: i32,
        pub bottom: i32,
        pub minimized: bool,
    }

    /// Read the rect/state of a cached HWND. Returns `None` if the window is no
    /// longer alive or no longer visible. `EnumWindows`-fresh HWNDs go through
    /// here too, so a dead cached handle naturally falls through to a re-lookup.
    pub(super) fn read_window(hwnd_val: usize) -> Option<LeagueWindow> {
        let hwnd = hwnd_val as HWND;
        // Safety: `hwnd` is a real window handle value (from EnumWindows or our
        // own cache). All four calls are read-only Win32 window queries.
        unsafe {
            if IsWindow(hwnd) == 0 {
                return None;
            }
            if IsWindowVisible(hwnd) == 0 {
                return None;
            }
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetWindowRect(hwnd, &mut rect) == 0 {
                return None;
            }
            Some(LeagueWindow {
                hwnd: hwnd_val,
                left: rect.left,
                top: rect.top,
                bottom: rect.bottom,
                minimized: IsIconic(hwnd) != 0,
            })
        }
    }

    /// Search context threaded through `EnumWindows`' LPARAM.
    struct EnumCtx {
        pid: u32,
        best_hwnd: usize,
        best_area: i64,
    }

    /// `EnumWindows` callback: keep tracking the **largest visible top-level
    /// window** owned by our target PID. We key purely on PID ownership + visible
    /// area (not window title/class), since the League client's window strings
    /// have varied across releases — the main client window is reliably the
    /// biggest visible top-level window of its process.
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // Safety: `lparam` points to our stack `EnumCtx`; we only read/append to it.
        let ctx = unsafe { &mut *(lparam as *mut EnumCtx) };
        // Safety: `hwnd` is a live top-level window handed to us by EnumWindows.
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1; // skip invisible; keep enumerating
            }
            let mut win_pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut win_pid);
            if win_pid != ctx.pid {
                return 1;
            }
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetWindowRect(hwnd, &mut rect) == 0 {
                return 1;
            }
            // Use the outer-frame area to rank; the client's main shell dwarfs its
            // helper/popover windows, so this picks the right HWND reliably.
            let width = (rect.right - rect.left).max(0) as i64;
            let height = (rect.bottom - rect.top).max(0) as i64;
            let area = width * height;
            if area > ctx.best_area {
                ctx.best_area = area;
                ctx.best_hwnd = hwnd as usize;
            }
        }
        1 // keep enumerating
    }

    /// Find the largest visible top-level window owned by `pid`. Returns `None`
    /// when no such window exists yet (the client process may be up before its
    /// main window is created).
    fn find_top_window_for_pid(pid: u32) -> Option<usize> {
        let mut ctx = EnumCtx {
            pid,
            best_hwnd: 0,
            best_area: 0,
        };
        let lparam = &mut ctx as *mut EnumCtx as LPARAM;
        // Safety: `enum_proc` only touches the `EnumCtx` via `lparam` and calls
        // read-only user32 functions; the context lives on this stack for the
        // duration of the synchronous EnumWindows call.
        unsafe { EnumWindows(Some(enum_proc), lparam) };
        if ctx.best_hwnd != 0 {
            Some(ctx.best_hwnd)
        } else {
            None
        }
    }

    /// Look up the `LeagueClientUx.exe` PID via the same process name the
    /// connection loop already monitors. Returns the first such PID.
    /// This rebuilds a `sysinfo::System` (expensive) so callers throttle it.
    fn find_league_pid() -> Option<u32> {
        let mut system = sysinfo::System::new_all();
        system.refresh_processes();
        let pid = system
            .processes_by_name("LeagueClientUx.exe")
            .next()
            .map(|p| p.pid().as_u32());
        pid
    }

    /// Resolve the current League client window. Reuses the cached HWND when it's
    /// still alive (cheap `IsWindow`/`GetWindowRect` only) and only falls back to
    /// the heavy `sysinfo` PID lookup when the cache is stale. `allow_lookup`
    /// gates the heavy path so a not-yet-created client window doesn't rebuild
    /// `System` on every 250 ms tick.
    pub(super) fn resolve_league_window(cached: Option<usize>, allow_lookup: bool) -> Option<LeagueWindow> {
        if let Some(h) = cached {
            if let Some(lw) = read_window(h) {
                return Some(lw);
            }
        }
        if !allow_lookup {
            return None;
        }
        let pid = find_league_pid()?;
        let hwnd = find_top_window_for_pid(pid)?;
        read_window(hwnd)
    }

    // --- docked / standalone window appearance (Tauri cross-platform API) ---

    fn apply_docked_props(window: &tauri::WebviewWindow) {
        let _ = window.set_decorations(false);
        let _ = window.set_resizable(false);
        let _ = window.set_always_on_top(true);
        let _ = window.set_skip_taskbar(true);
        // Relax the build-time min height (600) so the docked panel can shrink to
        // follow a short client; keep the width fixed.
        let _ = window.set_min_size(Some(PhysicalSize::new(PANEL_W as u32, 1)));
    }

    fn restore_standalone_props(window: &tauri::WebviewWindow) {
        let _ = window.set_decorations(true);
        let _ = window.set_resizable(true);
        let _ = window.set_always_on_top(false);
        let _ = window.set_skip_taskbar(false);
        let _ = window.set_min_size(Some(PhysicalSize::new(PANEL_W as u32, 600)));
    }

    pub fn enable_docker(app_handle: &tauri::AppHandle) {
        let Some(window) = app_handle.get_webview_window("main") else {
            return;
        };

        // Snapshot the user's current normal geometry so disable_docker can
        // put the window back where it was before we docked (before we move it).
        let saved = {
            let (mut x, mut y) = (0i32, 0i32);
            let (mut w, mut h) = (PANEL_W as u32, 600u32);
            if let Ok(p) = window.outer_position() {
                x = p.x;
                y = p.y;
            }
            if let Ok(s) = window.outer_size() {
                w = s.width;
                h = s.height;
            }
            (x, y, w, h)
        };

        apply_docked_props(&window);

        // Stop any prior tracker, then arm this one's stop flag.
        let stop = Arc::new(AtomicBool::new(false));
        let state = app_handle.state::<super::DockerState>();
        {
            let mut guard = state.stop.lock().expect("docker stop mutex poisoned");
            if let Some(prev) = guard.take() {
                prev.store(true, Ordering::SeqCst);
            }
            let mut rguard = state.saved_rect.lock().expect("docker saved_rect poisoned");
            if rguard.is_none() {
                *rguard = Some(saved);
            }
            *guard = Some(stop.clone());
        }

        // Get our own HWND so the focus check can tell whether the user is
        // actively interacting with the docked panel.
        let watcher_hwnd = window.hwnd().ok().map(|h| h.0 as usize);

        tauri::async_runtime::spawn(async move {
            let mut cached_hwnd: Option<usize> = None;
            // PID lookup throttle: time-based (~2 s) so it's independent of the
            // variable tick rate.
            let pid_lookup_cooldown = Duration::from_secs(2);
            let mut last_pid_lookup = Instant::now();
            // Adaptive polling: fast during drag, slow when stationary.
            const FAST_TICK_MS: u64 = 10;
            const SLOW_TICK_MS: u64 = 100;
            const STABLE_SWITCH_TICKS: u32 = 15; // 15 × 10ms = 150ms of reached → slow
            let mut tick_ms = FAST_TICK_MS;
            let mut stable_ticks: u32 = 0;
            let mut last_rect: (i32, i32, i32) = (0, 0, 0);

            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(tick_ms)).await;

                // Read state under a short-lived guard; drop it before blocking.
                let (docker_on, league_running) = {
                    let gs = crate::get_app_state().get_game_state().await;
                    (gs.settings.docker_mode == Some(true), gs.is_league_running)
                };
                if !docker_on {
                    return;
                }
                if !league_running {
                    let _ = window.hide();
                    cached_hwnd = None;
                    stable_ticks = 0;
                    tick_ms = FAST_TICK_MS;
                    continue;
                }

                // Time-based throttle for the heavy PID lookup.
                let allow_lookup = last_pid_lookup.elapsed() >= pid_lookup_cooldown;
                if allow_lookup {
                    last_pid_lookup = Instant::now();
                }

                let cached = cached_hwnd;
                let resolved = tauri::async_runtime::spawn_blocking(move || {
                    resolve_league_window(cached, allow_lookup)
                })
                .await
                .ok()
                .flatten();

                let Some(lw) = resolved else {
                    continue;
                };
                cached_hwnd = Some(lw.hwnd);

                if lw.minimized {
                    let _ = window.hide();
                    stable_ticks = 0;
                    tick_ms = FAST_TICK_MS;
                    continue;
                }

                // Hide only when neither League nor the docked panel itself is
                // the foreground window.
                let fg = unsafe { GetForegroundWindow() } as usize;
                let has_focus = fg == lw.hwnd || watcher_hwnd == Some(fg);
                if !has_focus {
                    let _ = window.hide();
                    continue;
                }

                // Detect movement. If the rect changed, we're being dragged →
                // fast polling. If stable for N fast ticks, drop to slow.
                let cur = (lw.left, lw.top, lw.bottom);
                if cur != last_rect {
                    last_rect = cur;
                    stable_ticks = 0;
                    tick_ms = FAST_TICK_MS;
                } else if tick_ms == FAST_TICK_MS {
                    stable_ticks += 1;
                    if stable_ticks >= STABLE_SWITCH_TICKS {
                        tick_ms = SLOW_TICK_MS;
                    }
                }

                // Dock tightly to the LEFT of the client, matching its top + height.
                let height = lw.bottom - lw.top;
                if height <= 0 {
                    continue;
                }
                let _ = window.set_size(PhysicalSize::new(PANEL_W as u32, height as u32));
                let _ = window.set_position(PhysicalPosition::new(lw.left - PANEL_W, lw.top));
                if !window.is_visible().unwrap_or(false) {
                    let _ = window.show();
                }
            }
        });
    }

    pub fn disable_docker(app_handle: &tauri::AppHandle) {
        let Some(window) = app_handle.get_webview_window("main") else {
            return;
        };
        let state = app_handle.state::<super::DockerState>();

        // Stop any running tracker.
        if let Some(stop) = state
            .stop
            .lock()
            .expect("docker stop mutex poisoned")
            .take()
        {
            stop.store(true, Ordering::SeqCst);
        }

        restore_standalone_props(&window);

        // Restore the pre-dock geometry if we snapshotted it; else re-center.
        let saved = state
            .saved_rect
            .lock()
            .expect("docker saved_rect poisoned")
            .take();
        match saved {
            Some((x, y, w, h)) => {
                let _ = window.set_size(PhysicalSize::new(w, h));
                let _ = window.set_position(PhysicalPosition::new(x, y));
            }
            None => {
                let _ = window.center();
            }
        }
        let _ = window.show();
    }
}

// On non-Windows hosts (e.g. CI/`cargo build`) the docker setting still
// persists and the toggle still flows through the UI, but the dock itself is a
// no-op — the app is Windows-only in practice (`windows_subsystem = "windows"`).
#[cfg(windows)]
pub use ffi::{disable_docker, enable_docker};

#[cfg(not(windows))]
pub fn enable_docker(_app_handle: &tauri::AppHandle) {}
#[cfg(not(windows))]
pub fn disable_docker(_app_handle: &tauri::AppHandle) {}

/// Managed state holding the live tracker's stop flag and the snapshotted
/// pre-dock window geometry. Registered once in `lib.rs` via `app.manage`.
#[derive(Default)]
pub struct DockerState {
    stop: std::sync::Mutex<Option<Arc<AtomicBool>>>,
    saved_rect: std::sync::Mutex<Option<(i32, i32, u32, u32)>>,
}
