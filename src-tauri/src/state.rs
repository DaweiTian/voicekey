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
    pub fn new(_app: &tauri::AppHandle) -> Result<Self, String> {
        Ok(Self {
            config: ConfigStore::new(),
            session: Mutex::new(AudioSession::default()),
            language: Mutex::new("system".into()),
            hotkey_capture_active: AtomicBool::new(false),
        })
    }

    /// Whether the settings hotkey recorder currently owns the keyboard.
    pub fn hotkey_capture_active(&self) -> bool {
        self.hotkey_capture_active.load(Ordering::Relaxed)
    }

    pub fn set_hotkey_capture_active(&self, active: bool) {
        self.hotkey_capture_active.store(active, Ordering::Relaxed);
    }
}
