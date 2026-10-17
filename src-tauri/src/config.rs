//! App configuration store (`voice-key-config.json`).
//!
//! Thread safety ([`CONFIG_LOCK`]): every read-modify-write of the config file
//! runs under one process-wide mutex, the same pattern as `stats.rs::STATS_LOCK`.
//! Concurrent writers exist: `commands::sync_overlay_visibility_flag` runs on
//! the overlay-rebuild worker thread, on tray-event threads and on the main
//! thread; `set_overlay_position` runs on renderer IPC threads. Interleaved
//! `load -> mutate -> save` cycles would silently lose each other's updates.
//!
//! Write entry points (enumerated so any new one joins the same lock):
//! * [`ConfigStore::merge`] - the only functional writer. Callers in
//!   `commands.rs`: `set_config` (and its hotkey rollback),
//!   `test_refine_connection`, `set_app_language`,
//!   `sync_overlay_visibility_flag` (overlay worker / tray / main thread) and
//!   `set_overlay_position` (both the persist and the reset branch).
//! Writes reach the file via the private [`ConfigStore::save_unlocked`]
//! primitive, always while holding the lock.
//! No other module writes `voice-key-config.json` (`fs::write` in history.rs /
//! stats.rs / asr.rs targets different files).
//!
//! Lock discipline: the mutex is only ever held around file I/O and pure JSON
//! merging - no session / stats / tray lock is taken while it is held, so no
//! lock-ordering deadlock is possible (exactly one lock, never nested).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

/// Process-wide guard for read-modify-write cycles on the config file (see the
/// module header for the enumerated write entry points). A poisoned mutex is
/// recovered instead of panicking: the authoritative data is on disk, not in
/// the guard.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

pub const STORED_SECRET_PLACEHOLDER: &str = "••••••••••••";
pub const HISTORY_RETENTION_DAYS: i64 = 90;

/// Persisted HUD capsule position (physical desktop pixels, as captured from
/// the overlay window's `outerPosition()` after a drag). `None` means "no
/// custom position": the capsule uses the bottom-center default. Cleared by
/// `set_overlay_position` with a `null` coordinate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OverlayPosition {
    pub x: f64,
    pub y: f64,
}

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
    /// Last drag-and-drop position of the HUD capsule (physical pixels).
    /// `None` = bottom-center default. Written by `set_overlay_position`.
    #[serde(default)]
    pub overlay_position: Option<OverlayPosition>,
    /// Work-area edge the capsule last **docked** to via the magnetic snap
    /// (`"left"` / `"right"` / `"top"` / `"bottom"`); `None` = not docked
    /// (also written on the reset branch). Persisted together with
    /// `overlay_position` by `set_overlay_position` so a restart restores not
    /// only *where* the capsule sits but *which shape* the renderer shows
    /// there: a docked standby capsule renders compact (icon only).
    #[serde(default)]
    pub overlay_docked: Option<String>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            language: "system".into(),
            auto_launch: false,
            overlay_visible: true,
            overlay_position: None,
            overlay_docked: None,
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

impl Default for ASRConfig {
    fn default() -> Self {
        Self {
            low_volume_mode: true,
            microphone_device_id: String::new(),
            microphone_device_label: String::new(),
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
    /// 用户词表（人名/术语）。非敏感，与 API key 同侧持久化，但不参与脱敏。
    #[serde(default)]
    pub user_glossary: Vec<String>,
    /// 口水词清理档位："off" | "rules" | "llm"。非法/缺失值按 "off"。
    #[serde(default)]
    pub filler_cleanup: String,
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
            user_glossary: Vec::new(),
            filler_cleanup: "off".into(),
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

/// invoke 边界校验用户词表：形状非法/超限报错（渲染层数据不可信），
/// 空串与重复静默清洗。上限 100 条 × 64 字符。
pub fn normalize_user_glossary(raw: Option<&serde_json::Value>) -> Result<Vec<String>, String> {
    let Some(value) = raw else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| "llmRefine.userGlossary must be an array of strings".to_string())?;
    let mut terms: Vec<String> = Vec::new();
    for item in items {
        let s = item
            .as_str()
            .ok_or_else(|| "llmRefine.userGlossary must contain only strings".to_string())?;
        let trimmed = s.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > 64 {
            return Err("llmRefine.userGlossary entries must be at most 64 characters".into());
        }
        if !terms.iter().any(|t| t == trimmed) {
            terms.push(trimmed.to_string());
        }
    }
    if terms.len() > 100 {
        return Err("llmRefine.userGlossary supports at most 100 entries".into());
    }
    Ok(terms)
}

/// 仅接受三档字面量；其余返回 `None`（调用方保留旧值）。
pub fn normalize_filler_cleanup(raw: &str) -> Option<String> {
    match raw {
        "off" | "rules" | "llm" => Some(raw.to_string()),
        _ => None,
    }
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

    /// Read the config file. Serialized against writers by [`CONFIG_LOCK`] so
    /// a concurrent `fs::write` can never be observed half-written (a torn
    /// read would deserialize to `AppConfig::default()` and could, via
    /// `merge`, wipe stored settings).
    pub fn load(&self) -> AppConfig {
        let _guard = CONFIG_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        self.load_unlocked()
    }

    /// [`Self::load`] without locking. Only call while holding [`CONFIG_LOCK`].
    fn load_unlocked(&self) -> AppConfig {
        match fs::read_to_string(&self.path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => AppConfig::default(),
        }
    }

    /// The config-file write primitive. Not thread-safe on its own: callers
    /// must hold [`CONFIG_LOCK`] (see the module header: every write path
    /// goes through it).
    fn save_unlocked(&self, config: &AppConfig) -> Result<(), String> {
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
    ///
    /// The whole load -> mutate -> save sequence runs under [`CONFIG_LOCK`]
    /// (same pattern as `stats.rs::STATS_LOCK`): concurrent callers
    /// (`sync_overlay_visibility_flag` on the overlay-rebuild worker thread,
    /// tray / main-thread paths, `set_overlay_position`, `set_config`) can
    /// never interleave and lose each other's updates. Nothing else is locked
    /// inside, so holding it cannot deadlock.
    pub fn merge(&self, partial: &serde_json::Value) -> Result<AppConfig, String> {
        let _guard = CONFIG_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let stored = self.load_unlocked();
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
                // `overlayPosition`: explicit `null` clears the persisted HUD
                // position (capsule falls back to the bottom-center default);
                // a `{ x, y }` object stores it; anything else is ignored so a
                // malformed patch can never corrupt the stored position.
                if let Some(v) = obj.get("overlayPosition") {
                    if v.is_null() {
                        merged.app.overlay_position = None;
                    } else if let Some(pos) = v.as_object().and_then(|o| {
                        let x = o.get("x").and_then(|n| n.as_f64())?;
                        let y = o.get("y").and_then(|n| n.as_f64())?;
                        (x.is_finite() && y.is_finite()).then_some(OverlayPosition { x, y })
                    }) {
                        merged.app.overlay_position = Some(pos);
                    }
                }
                // `overlayDocked`: `null` clears the dock (capsule renders the
                // expanded standby shape); one of the four known work-area
                // edges stores it; anything else is ignored so a malformed
                // patch can never corrupt the stored dock state.
                if let Some(v) = obj.get("overlayDocked") {
                    if v.is_null() {
                        merged.app.overlay_docked = None;
                    } else if let Some(edge) = v.as_str() {
                        if matches!(edge, "left" | "right" | "top" | "bottom") {
                            merged.app.overlay_docked = Some(edge.to_string());
                        }
                    }
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

            if let Some(obj) = llm.as_object() {
                if let Some(v) = obj.get("fillerCleanup").and_then(|v| v.as_str()) {
                    if let Some(valid) = normalize_filler_cleanup(v) {
                        next.filler_cleanup = valid;
                    }
                }
                if let Some(v) = obj.get("userGlossary") {
                    if !v.is_null() {
                        next.user_glossary = normalize_user_glossary(Some(v))?;
                    }
                }
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

        self.save_unlocked(&merged)?;
        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_user_glossary_trims_dedupes_and_skips_empty() {
        let raw = serde_json::json!([" 张伟 ", "张伟", "", "芷若"]);
        assert_eq!(
            normalize_user_glossary(Some(&raw)).unwrap(),
            vec!["张伟".to_string(), "芷若".to_string()]
        );
        assert_eq!(normalize_user_glossary(None).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn normalize_user_glossary_rejects_malformed_payloads() {
        // 非字符串条目 / 非数组 / 单条超 64 字符 / 超 100 条：invoke 边界拒绝。
        assert!(normalize_user_glossary(Some(&serde_json::json!([42]))).is_err());
        assert!(normalize_user_glossary(Some(&serde_json::json!("张伟"))).is_err());
        assert!(normalize_user_glossary(Some(&serde_json::json!(["x".repeat(65)]))).is_err());
        let many: Vec<String> = (0..120).map(|i| format!("term{i}")).collect();
        assert!(normalize_user_glossary(Some(&serde_json::json!(many))).is_err());
    }

    #[test]
    fn normalize_filler_cleanup_accepts_only_known_modes() {
        assert_eq!(normalize_filler_cleanup("off").as_deref(), Some("off"));
        assert_eq!(normalize_filler_cleanup("rules").as_deref(), Some("rules"));
        assert_eq!(normalize_filler_cleanup("llm").as_deref(), Some("llm"));
        assert_eq!(normalize_filler_cleanup("drop-everything"), None);
    }
}
