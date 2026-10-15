use crate::audio::AudioSession;
use crate::config::ConfigStore;
use std::sync::Mutex;

pub struct AppState {
    pub config: ConfigStore,
    pub session: Mutex<AudioSession>,
    pub language: Mutex<String>,
}

impl AppState {
    pub fn new(_app: &tauri::AppHandle) -> Result<Self, String> {
        Ok(Self {
            config: ConfigStore::new(),
            session: Mutex::new(AudioSession::default()),
            language: Mutex::new("system".into()),
        })
    }
}
