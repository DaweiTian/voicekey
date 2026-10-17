use crate::audio::AudioSession;
use crate::config::ConfigStore;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Process-wide shared `reqwest::Client` so ASR / LLM / download calls reuse
/// one connection pool instead of building a client (and TLS backend) per call.
/// Intentionally no builder-level timeout: callers keep their own semantics
/// via per-request `.timeout()` (120s ASR, 60s LLM, 300s model download).
pub fn shared_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .build()
            .expect("failed to build shared HTTP client")
    })
}

pub struct AppState {
    pub config: ConfigStore,
    pub session: Mutex<AudioSession>,
    pub language: Mutex<String>,
    /// While the settings hotkey recorder is capturing keystrokes, global
    /// shortcuts must not fire actions (the recorder's combos would otherwise
    /// start a recording / trigger translation). Toggled from the renderer via
    /// `set_hotkey_capture_active`; always reset on recorder exit/unmount.
    pub hotkey_capture_active: AtomicBool,
}

impl AppState {
    /// Construct the shared state. Deliberately takes **no** `AppHandle`, so it
    /// can be registered with `tauri::Builder::manage` (see `lib.rs`) — i.e.
    /// before the config-created windows (main / overlay / session / background)
    /// load their webviews. Those webviews invoke `get_config` /
    /// `get_app_language` the moment they run, which is *before* `setup()`
    /// executes; managing the state in `setup()` lost that first call ("state
    /// not managed") and the overlay HUD silently fell back to its expanded
    /// capsule while its window sat at a persisted half-hidden dock.
    pub fn new() -> Self {
        Self {
            config: ConfigStore::new(),
            session: Mutex::new(AudioSession::default()),
            language: Mutex::new("system".into()),
            hotkey_capture_active: AtomicBool::new(false),
        }
    }

    /// Whether the settings hotkey recorder currently owns the keyboard.
    pub fn hotkey_capture_active(&self) -> bool {
        self.hotkey_capture_active.load(Ordering::Relaxed)
    }

    pub fn set_hotkey_capture_active(&self, active: bool) {
        self.hotkey_capture_active.store(active, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AppState` must stay constructible **without** an `AppHandle` so `lib.rs`
    /// can register it on the builder chain, before the config-created windows
    /// load their webviews. If this ever needs a handle again, the state has to
    /// move back into `setup()` and the startup `get_config` race (which left the
    /// HUD capsule expanded at a half-hidden dock) returns.
    #[test]
    fn state_is_constructible_without_an_app_handle() {
        let state = AppState::new();
        assert_eq!(*state.language.lock().unwrap(), "system");
        assert!(!state.hotkey_capture_active());
    }
}
