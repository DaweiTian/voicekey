use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const STORED_SECRET_PLACEHOLDER: &str = "••••••••••••";
pub const HISTORY_RETENTION_DAYS: i64 = 90;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppPreferences {
    pub language: String,
    pub auto_launch: bool,
    /// Whether the always-on HUD capsule is visible. Toggled from the tray
    /// menu ("Show floating capsule"); a hidden capsule stays hidden even when
    /// a session starts. Defaults to visible.
    #[serde(default = "default_true")]
    pub overlay_visible: bool,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            language: "system".into(),
            auto_launch: false,
            overlay_visible: true,
        }
    }
}

/// Mouse-based session triggers. `middle_click_ptt` is Windows-only at the
/// implementation level (`src-tauri/src/mouse.rs`); the flag is stored on all
/// platforms so switching machines never loses the preference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MouseConfig {
    /// Middle mouse button acts as push-to-talk: press starts, release stops.
    /// Off by default because some apps (terminals) use middle-click to paste
    /// and the hook deliberately never swallows the click.
    pub middle_click_ptt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ASRConfig {
    #[serde(default = "default_true")]
    pub low_volume_mode: bool,
    #[serde(default)]
    pub microphone_device_id: String,
    #[serde(default)]
    pub microphone_device_label: String,
    #[serde(default)]
    pub streaming_enabled: bool,
    /// `local` = offline ONNX models; `mimo` = MiMo cloud ASR (mimo-v2.5-asr).
    #[serde(default = "default_asr_engine")]
    pub engine: String,
    /// MiMo ASR endpoint. Empty means fall back to LLM MiMo endpoint / default.
    #[serde(default)]
    pub mimo_endpoint: String,
    /// MiMo ASR API key. Empty means fall back to LLM MiMo API key.
    #[serde(default)]
    pub mimo_api_key: String,
    /// MiMo ASR model id.
    #[serde(default = "default_mimo_asr_model")]
    pub mimo_model: String,
}

fn default_mimo_asr_model() -> String {
    "mimo-v2.5-asr".into()
}

fn default_true() -> bool {
    true
}

fn default_asr_engine() -> String {
    "mimo".into()
}

impl Default for ASRConfig {
    fn default() -> Self {
        Self {
            low_volume_mode: true,
            microphone_device_id: String::new(),
            microphone_device_label: String::new(),
            streaming_enabled: false,
            engine: default_asr_engine(),
            mimo_endpoint: String::new(),
            mimo_api_key: String::new(),
            mimo_model: default_mimo_asr_model(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderKeyConfig {
    pub api_key: String,
    pub model: String,
    pub endpoint: String,
}

impl ProviderKeyConfig {
    pub fn with_model(model: &str) -> Self {
        Self {
            model: model.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LLMRefineConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_true")]
    pub translate_output: bool,
    #[serde(default)]
    pub openai: ProviderKeyConfig,
    #[serde(default)]
    pub deepseek: ProviderKeyConfig,
    #[serde(default)]
    pub openrouter: ProviderKeyConfig,
    #[serde(default)]
    pub tokendance: ProviderKeyConfig,
    #[serde(default)]
    pub mimo: ProviderKeyConfig,
    #[serde(default)]
    pub custom: ProviderKeyConfig,
}

impl Default for LLMRefineConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "mimo".into(),
            endpoint: "https://api.xiaomimimo.com/v1".into(),
            model: "mimo-v2.5".into(),
            api_key: String::new(),
            translate_output: false,
            openai: ProviderKeyConfig::with_model("gpt-4o-mini"),
            deepseek: ProviderKeyConfig::with_model("deepseek-chat"),
            openrouter: ProviderKeyConfig::with_model("openai/gpt-4o-mini"),
            tokendance: ProviderKeyConfig::with_model("deepseek-v4.1-flash"),
            mimo: ProviderKeyConfig {
                model: "mimo-v2.5".into(),
                endpoint: "https://api.xiaomimimo.com/v1".into(),
                ..Default::default()
            },
            custom: ProviderKeyConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyConfig {
    pub ptt_key: String,
    pub toggle_settings: String,
    pub translate_key: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            ptt_key: "Control+Shift+Space".into(),
            toggle_settings: "Control+Shift+,".into(),
            translate_key: "Control+Shift+T".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TranslationConfig {
    pub enabled: bool,
    pub target_language: String,
}

impl Default for TranslationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            target_language: "English".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub app: AppPreferences,
    pub asr: ASRConfig,
    #[serde(rename = "llmRefine")]
    pub llm_refine: LLMRefineConfig,
    pub hotkey: HotkeyConfig,
    pub translation: TranslationConfig,
    #[serde(default)]
    pub mouse: MouseConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            app: AppPreferences::default(),
            asr: ASRConfig::default(),
            llm_refine: LLMRefineConfig::default(),
            hotkey: HotkeyConfig::default(),
            translation: TranslationConfig::default(),
            mouse: MouseConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSecretRequest {
    pub scope: String,
    pub provider: String,
}

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("voice-key");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn config_file() -> PathBuf {
    config_dir().join("voice-key-config.json")
}

pub fn history_file() -> PathBuf {
    config_dir().join("voice-key-history.json")
}

/// Lifetime stats store, independent from history so history deletion or the
/// 90-day retention prune never resets cumulative figures.
pub fn stats_file() -> PathBuf {
    config_dir().join("voice-key-stats.json")
}

pub fn log_dir() -> PathBuf {
    let dir = config_dir().join("logs");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn model_storage_dir() -> PathBuf {
    let dir = config_dir().join("local-asr");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new() -> Self {
        Self {
            path: config_file(),
        }
    }

    pub fn load(&self) -> AppConfig {
        match fs::read_to_string(&self.path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => AppConfig::default(),
        }
    }

    pub fn save(&self, config: &AppConfig) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
        fs::write(&self.path, json).map_err(|e| e.to_string())
    }

    /// Config sent to the renderer never includes plaintext API keys.
    pub fn load_masked(&self) -> AppConfig {
        let mut config = self.load();
        let mask = |value: &str| {
            if value.is_empty() {
                String::new()
            } else {
                STORED_SECRET_PLACEHOLDER.to_string()
            }
        };
        config.asr.mimo_api_key = mask(&config.asr.mimo_api_key);
        config.llm_refine.api_key = mask(&config.llm_refine.api_key);
        config.llm_refine.openai.api_key = mask(&config.llm_refine.openai.api_key);
        config.llm_refine.deepseek.api_key = mask(&config.llm_refine.deepseek.api_key);
        config.llm_refine.openrouter.api_key = mask(&config.llm_refine.openrouter.api_key);
        config.llm_refine.tokendance.api_key = mask(&config.llm_refine.tokendance.api_key);
        config.llm_refine.mimo.api_key = mask(&config.llm_refine.mimo.api_key);
        config.llm_refine.custom.api_key = mask(&config.llm_refine.custom.api_key);
        config
    }

    pub fn get_secret(&self, request: &ConfigSecretRequest) -> Result<String, String> {
        let config = self.load();
        match (request.scope.as_str(), request.provider.as_str()) {
            ("llm-refine", "openai") => Ok(config.llm_refine.openai.api_key),
            ("llm-refine", "deepseek") => Ok(config.llm_refine.deepseek.api_key),
            ("llm-refine", "openrouter") => Ok(config.llm_refine.openrouter.api_key),
            ("llm-refine", "tokendance") => Ok(config.llm_refine.tokendance.api_key),
            ("llm-refine", "mimo") => Ok(config.llm_refine.mimo.api_key),
            ("llm-refine", "custom-compatible") => Ok(config.llm_refine.custom.api_key),
            ("asr", "mimo") => Ok(config.asr.mimo_api_key),
            _ => Err("Invalid saved API key request".into()),
        }
    }

    /// Merge partial config, preserving stored secrets when placeholder is echoed back.
    pub fn merge(&self, partial: &serde_json::Value) -> Result<AppConfig, String> {
        let stored = self.load();
        let mut merged = stored.clone();

        if let Some(app) = partial.get("app") {
            // Merge field-by-field (same rationale as `asr` below): patching
            // `language` alone must never reset `overlayVisible`, which is owned
            // by the tray menu and may legitimately differ from the renderer's
            // snapshot. An explicit value always wins; absent keys keep stored.
            if let Some(obj) = app.as_object() {
                if let Some(v) = obj.get("language").and_then(|v| v.as_str()) {
                    merged.app.language = v.to_string();
                }
                if let Some(v) = obj.get("autoLaunch").and_then(|v| v.as_bool()) {
                    merged.app.auto_launch = v;
                }
                if let Some(v) = obj.get("overlayVisible").and_then(|v| v.as_bool()) {
                    merged.app.overlay_visible = v;
                }
            }
        }

        if let Some(asr) = partial.get("asr") {
            // Merge field-by-field: fields absent from the patch keep their
            // stored value. Deserializing the whole struct here would silently
            // reset omitted fields (mimoApiKey / mimoEndpoint / ...) to their
            // serde defaults. An explicit empty string still clears a field.
            if let Some(obj) = asr.as_object() {
                if let Some(v) = obj.get("lowVolumeMode").and_then(|v| v.as_bool()) {
                    merged.asr.low_volume_mode = v;
                }
                if let Some(v) = obj.get("microphoneDeviceId").and_then(|v| v.as_str()) {
                    merged.asr.microphone_device_id = v.chars().take(256).collect();
                }
                if let Some(v) = obj.get("microphoneDeviceLabel").and_then(|v| v.as_str()) {
                    merged.asr.microphone_device_label = v.chars().take(128).collect();
                }
                if let Some(v) = obj.get("streamingEnabled").and_then(|v| v.as_bool()) {
                    merged.asr.streaming_enabled = v;
                }
                if let Some(v) = obj.get("engine").and_then(|v| v.as_str()) {
                    let engine = v.to_string();
                    if engine == "local" || engine == "mimo" {
                        merged.asr.engine = engine;
                    }
                }
                if let Some(v) = obj.get("mimoEndpoint").and_then(|v| v.as_str()) {
                    merged.asr.mimo_endpoint = v.chars().take(512).collect();
                }
                if let Some(v) = obj.get("mimoApiKey").and_then(|v| v.as_str()) {
                    if v == STORED_SECRET_PLACEHOLDER {
                        // keep stored value
                    } else {
                        merged.asr.mimo_api_key = v.to_string();
                    }
                }
                if let Some(v) = obj.get("mimoModel").and_then(|v| v.as_str()) {
                    merged.asr.mimo_model = if v.trim().is_empty() {
                        default_mimo_asr_model()
                    } else {
                        v.chars().take(128).collect()
                    };
                }
            }
        }

        if let Some(llm) = partial.get("llmRefine") {
            let mut next = merged.llm_refine.clone();
            if let Some(v) = llm.get("enabled").and_then(|v| v.as_bool()) {
                next.enabled = v;
            }
            if let Some(v) = llm.get("provider").and_then(|v| v.as_str()) {
                next.provider = v.to_string();
            }
            if let Some(v) = llm.get("endpoint").and_then(|v| v.as_str()) {
                next.endpoint = v.to_string();
            }
            if let Some(v) = llm.get("model").and_then(|v| v.as_str()) {
                next.model = v.to_string();
            }
            if let Some(v) = llm.get("translateOutput").and_then(|v| v.as_bool()) {
                next.translate_output = v;
            }
            if let Some(v) = llm.get("apiKey").and_then(|v| v.as_str()) {
                if v == STORED_SECRET_PLACEHOLDER {
                    next.api_key = merged.llm_refine.api_key.clone();
                } else {
                    next.api_key = v.to_string();
                }
            }

            for (key, slot) in [
                ("openai", &mut next.openai),
                ("deepseek", &mut next.deepseek),
                ("openrouter", &mut next.openrouter),
                ("tokendance", &mut next.tokendance),
                ("mimo", &mut next.mimo),
                ("custom", &mut next.custom),
            ] {
                if let Some(provider) = llm.get(key) {
                    if let Some(v) = provider.get("model").and_then(|v| v.as_str()) {
                        slot.model = v.to_string();
                    }
                    if let Some(v) = provider.get("endpoint").and_then(|v| v.as_str()) {
                        slot.endpoint = v.to_string();
                    }
                    if let Some(v) = provider.get("apiKey").and_then(|v| v.as_str()) {
                        if v == STORED_SECRET_PLACEHOLDER {
                            slot.api_key = match key {
                                "openai" => stored.llm_refine.openai.api_key.clone(),
                                "deepseek" => stored.llm_refine.deepseek.api_key.clone(),
                                "openrouter" => stored.llm_refine.openrouter.api_key.clone(),
                                "tokendance" => stored.llm_refine.tokendance.api_key.clone(),
                                "mimo" => stored.llm_refine.mimo.api_key.clone(),
                                _ => stored.llm_refine.custom.api_key.clone(),
                            };
                        } else {
                            slot.api_key = v.to_string();
                        }
                    }
                }
            }

            // Keep top-level model/endpoint in sync for the active custom-compatible provider.
            if next.provider == "custom-compatible" {
                if next.custom.model.trim().is_empty() {
                    next.custom.model = next.model.clone();
                }
                if next.custom.endpoint.trim().is_empty() {
                    next.custom.endpoint = next.endpoint.clone();
                }
                next.model = next.custom.model.clone();
                next.endpoint = next.custom.endpoint.clone();
                next.api_key = next.custom.api_key.clone();
            }

            merged.llm_refine = next;
        }

        if let Some(hotkey) = partial.get("hotkey") {
            if let Some(v) = hotkey.get("pttKey").and_then(|v| v.as_str()) {
                merged.hotkey.ptt_key = v.to_string();
            }
            if let Some(v) = hotkey.get("toggleSettings").and_then(|v| v.as_str()) {
                merged.hotkey.toggle_settings = v.to_string();
            }
            if let Some(v) = hotkey.get("translateKey").and_then(|v| v.as_str()) {
                merged.hotkey.translate_key = v.to_string();
            }
        }

        if let Some(translation) = partial.get("translation") {
            if let Some(v) = translation.get("enabled").and_then(|v| v.as_bool()) {
                merged.translation.enabled = v;
            }
            if let Some(v) = translation.get("targetLanguage").and_then(|v| v.as_str()) {
                merged.translation.target_language = v.to_string();
            }
        }

        if let Some(mouse) = partial.get("mouse") {
            if let Some(v) = mouse.get("middleClickPtt").and_then(|v| v.as_bool()) {
                merged.mouse.middle_click_ptt = v;
            }
        }

        self.save(&merged)?;
        Ok(merged)
    }
}
