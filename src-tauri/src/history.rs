use crate::config::{history_file, HISTORY_RETENTION_DAYS};
use serde::{Deserialize, Serialize};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: String,
    pub timestamp: u64,
    pub text: String,
    /// Audio duration in milliseconds. Field name must stay `duration` so
    /// `rename_all = "camelCase"` serializes it as `duration` (the frontend
    /// reads `item.duration` in ms). Legacy records lack the key and
    /// deserialize to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HistoryStore {
    items: Vec<HistoryItem>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct HistoryManager;

impl HistoryManager {
    fn load() -> HistoryStore {
        match fs::read_to_string(history_file()) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => HistoryStore::default(),
        }
    }

    fn save(store: &HistoryStore) -> Result<(), String> {
        let path = history_file();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
        fs::write(path, json).map_err(|e| e.to_string())
    }

    fn prune(items: Vec<HistoryItem>) -> Vec<HistoryItem> {
        let cutoff = now_ms().saturating_sub(HISTORY_RETENTION_DAYS as u64 * 24 * 60 * 60 * 1000);
        items.into_iter().filter(|i| i.timestamp >= cutoff).collect()
    }

    pub fn get_all() -> Vec<HistoryItem> {
        Self::prune(Self::load().items)
    }

    /// Read every stored item WITHOUT the 90-day retention prune applied by
    /// `get_all()`. Used by `stats` backfill so entries past the retention
    /// window still count toward lifetime totals.
    pub fn load_raw() -> Vec<HistoryItem> {
        Self::load().items
    }

    pub fn add(text: String, duration_ms: Option<u64>) -> Result<HistoryItem, String> {
        let mut store = Self::load();
        let item = HistoryItem {
            id: format!("{}-{}", now_ms(), uuid::Uuid::new_v4().simple()),
            timestamp: now_ms(),
            text,
            duration: duration_ms,
        };
        store.items.insert(0, item.clone());
        store.items = Self::prune(store.items);
        Self::save(&store)?;
        Ok(item)
    }

    pub fn delete(id: &str) -> Result<bool, String> {
        let mut store = Self::load();
        let before = store.items.len();
        store.items.retain(|i| i.id != id);
        let changed = store.items.len() != before;
        if changed {
            Self::save(&store)?;
        }
        Ok(changed)
    }

    pub fn clear() -> Result<(), String> {
        Self::save(&HistoryStore::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_serializes_as_duration_ms() {
        let item = HistoryItem {
            id: "1".into(),
            timestamp: 0,
            text: "hello".into(),
            duration: Some(1500),
        };
        let json = serde_json::to_value(&item).expect("serialize");
        // Frontend reads `item.duration` in milliseconds; must NOT be durationMs.
        assert_eq!(json.get("duration"), Some(&serde_json::json!(1500)));
        assert!(json.get("durationMs").is_none());
    }

    #[test]
    fn legacy_item_without_duration_deserializes_to_none() {
        let raw = r#"{"id":"1","timestamp":0,"text":"hello"}"#;
        let item: HistoryItem = serde_json::from_str(raw).expect("deserialize legacy");
        assert_eq!(item.duration, None);
    }

    #[test]
    fn duration_none_is_omitted_from_json() {
        let item = HistoryItem {
            id: "1".into(),
            timestamp: 0,
            text: "hello".into(),
            duration: None,
        };
        let json = serde_json::to_value(&item).expect("serialize");
        assert!(json.get("duration").is_none());
    }
}
