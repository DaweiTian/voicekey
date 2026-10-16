//! Lifetime dictation statistics, persisted independently from history.
//!
//! Home-page "cumulative" stats (total characters / total audio / active days /
//! peak day) must survive history deletion and the 90-day retention prune, so
//! they live in their own file (`voice-key-stats.json`) and only ever grow.
//! Windowed stats (today / last N days / trend charts) are intentionally still
//! computed from history in the frontend.
//!
//! Character counting MUST stay aligned with the frontend
//! `src/lib/stats.ts` `countCharacters`: strip every whitespace char, then
//! count the remaining characters.

use crate::config::stats_file;
use crate::history::{HistoryItem, HistoryManager};
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

const STATS_VERSION: u32 = 1;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatsTotals {
    /// Recognized transcripts ever recorded (one per successful session).
    pub sessions: u64,
    /// Lifetime characters (whitespace excluded, see `count_characters`).
    pub characters: u64,
    /// Lifetime audio duration in milliseconds (unknown duration counts 0).
    pub audio_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatsStore {
    /// Schema version of the stats file.
    pub version: u32,
    /// One-shot flag: lifetime totals were seeded from the raw history file.
    /// Once true, history deletion / 90-day pruning can never change totals.
    pub backfilled: bool,
    pub totals: StatsTotals,
    /// Local day key `YYYY-MM-DD` -> characters recognized that day.
    /// Active days = `daily.len()`; peak day = the max entry. BTreeMap keeps
    /// JSON output ordered oldest -> newest, so the frontend (and the peak
    /// tie-break below) sees a deterministic, chronologically sorted map.
    pub daily: BTreeMap<String, u64>,
}

impl Default for StatsStore {
    fn default() -> Self {
        Self {
            version: STATS_VERSION,
            backfilled: false,
            totals: StatsTotals::default(),
            daily: BTreeMap::new(),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Same rule as the frontend `countCharacters` (`src/lib/stats.ts:21`): drop
/// every whitespace character, then count what remains.
pub fn count_characters(text: &str) -> u64 {
    text.chars().filter(|c| !c.is_whitespace()).count() as u64
}

/// Local calendar day for an epoch-ms timestamp as `YYYY-MM-DD`, matching the
/// frontend `buildDateKey` (local time, zero-padded).
fn day_key(timestamp: u64) -> String {
    let dt = Local
        .timestamp_millis_opt(timestamp as i64)
        .single()
        .unwrap_or_else(Local::now);
    dt.format("%Y-%m-%d").to_string()
}

pub struct StatsManager;

/// Process-wide guard for the read-modify-write cycle on the stats file.
///
/// `get()` (first run triggers the backfill and persists it) and
/// `record_add_at_in()` (a transcription just finished) both do
/// load -> mutate -> save; interleaved they can lose an accumulation. Every
/// full sequence runs under this lock. A poisoned mutex is recovered instead
/// of panicking: the authoritative data is on disk, not in the guard.
static STATS_LOCK: Mutex<()> = Mutex::new(());

impl StatsManager {
    fn load_from(path: &Path) -> StatsStore {
        match fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => StatsStore::default(),
        }
    }

    fn save_to(path: &Path, store: &StatsStore) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
        fs::write(path, json).map_err(|e| e.to_string())
    }

    /// Fold one successful transcript into the store.
    fn accumulate(store: &mut StatsStore, text: &str, duration_ms: Option<u64>, timestamp: u64) {
        let chars = count_characters(text);
        store.totals.sessions += 1;
        store.totals.characters += chars;
        store.totals.audio_ms += duration_ms.unwrap_or(0);
        *store.daily.entry(day_key(timestamp)).or_insert(0) += chars;
    }

    /// One-shot seeding from the RAW history items (pass `HistoryManager::
    /// load_raw()`, which unlike `get_all()` does NOT apply the 90-day prune),
    /// so entries past the retention window still count toward lifetime totals.
    ///
    /// Idempotent: returns `false` and changes nothing once `backfilled` is
    /// true; returns `true` when it actually seeded (caller should persist).
    pub fn backfill_from_history(store: &mut StatsStore, items: &[HistoryItem]) -> bool {
        if store.backfilled {
            return false;
        }
        for item in items {
            Self::accumulate(store, &item.text, item.duration, item.timestamp);
        }
        store.backfilled = true;
        true
    }

    fn load_or_backfill_from(path: &Path, raw_items: &[HistoryItem]) -> StatsStore {
        let mut store = Self::load_from(path);
        if Self::backfill_from_history(&mut store, raw_items) {
            if let Err(e) = Self::save_to(path, &store) {
                log::warn!("stats backfill save failed: {e}");
            }
        }
        store
    }

    /// Current lifetime stats. Self-heals: when the stats file is missing but
    /// history exists (first run / deleted stats file), it backfills once.
    ///
    /// Runs the whole load-or-backfill (+ persist) sequence under
    /// [`STATS_LOCK`] so a concurrent `record_add_at_in` can never be lost.
    pub fn get() -> StatsStore {
        let _guard = STATS_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        Self::load_or_backfill_from(&stats_file(), &HistoryManager::load_raw())
    }

    /// Record one successful transcription into lifetime stats.
    ///
    /// ORDERING: call this BEFORE `HistoryManager::add` for the same item.
    /// On first run the backfill seeds totals from raw history; if the new
    /// item were already in history at that point it would be counted twice.
    pub fn record_add(text: &str, duration_ms: Option<u64>) -> Result<(), String> {
        Self::record_add_at(text, duration_ms, now_ms())
    }

    pub fn record_add_at(
        text: &str,
        duration_ms: Option<u64>,
        timestamp: u64,
    ) -> Result<(), String> {
        Self::record_add_at_in(
            &stats_file(),
            &HistoryManager::load_raw(),
            text,
            duration_ms,
            timestamp,
        )
    }

    fn record_add_at_in(
        path: &Path,
        raw_items: &[HistoryItem],
        text: &str,
        duration_ms: Option<u64>,
        timestamp: u64,
    ) -> Result<(), String> {
        // Whole load-or-backfill -> accumulate -> save sequence under the lock
        // (see [`STATS_LOCK`]): concurrent `get` / `record_add` calls can never
        // interleave and drop one accumulation.
        let _guard = STATS_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let mut store = Self::load_or_backfill_from(path, raw_items);
        Self::accumulate(&mut store, text, duration_ms, timestamp);
        Self::save_to(path, &store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "voice-key-stats-test-{}-{label}.json",
            uuid::Uuid::new_v4().simple()
        ))
    }

    fn item(id: &str, timestamp: u64, text: &str, duration: Option<u64>) -> HistoryItem {
        HistoryItem {
            id: id.into(),
            timestamp,
            text: text.into(),
            duration,
        }
    }

    #[test]
    fn count_characters_matches_frontend_whitespace_rule() {
        assert_eq!(count_characters("hello world"), 10);
        assert_eq!(count_characters(" 你 好\n\t世界 "), 4);
        assert_eq!(count_characters("   \n\t"), 0);
    }

    #[test]
    fn record_add_accumulates_totals_and_daily() {
        let path = temp_path("accumulate");
        // Same timestamp on purpose: both records must land in ONE daily bucket.
        let ts = now_ms();
        StatsManager::record_add_at_in(&path, &[], "你好 world", Some(1500), ts)
            .expect("first record");
        StatsManager::record_add_at_in(&path, &[], "ab c", Some(500), ts).expect("second record");
        let store = StatsManager::load_from(&path);
        assert_eq!(store.totals.sessions, 2);
        let expected_chars = count_characters("你好 world") + count_characters("ab c");
        assert_eq!(store.totals.characters, expected_chars);
        assert_eq!(store.totals.audio_ms, 2000);
        assert_eq!(store.daily.len(), 1);
        assert_eq!(store.daily.get(&day_key(ts)), Some(&expected_chars));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn backfill_is_idempotent() {
        let items = vec![
            item("1", 1_700_000_000_000, "第一条记录", Some(1000)),
            item("2", 1_600_000_000_000, "second one", None),
        ];
        let mut store = StatsStore::default();
        assert!(StatsManager::backfill_from_history(&mut store, &items));
        assert!(store.backfilled);
        assert_eq!(store.totals.sessions, 2);
        assert_eq!(
            store.totals.characters,
            count_characters("第一条记录") + count_characters("second one")
        );
        assert_eq!(store.totals.audio_ms, 1000);

        // A second backfill must change nothing.
        let snapshot = store.clone();
        assert!(!StatsManager::backfill_from_history(&mut store, &items));
        assert_eq!(store, snapshot);
    }

    #[test]
    fn deleting_history_keeps_lifetime_stats() {
        let path = temp_path("delete-history");
        let items = vec![item("1", 1_700_000_000_000, "keep me", Some(700))];
        // First run: seed from history, then record a live session.
        let seeded = StatsManager::load_or_backfill_from(&path, &items);
        assert!(seeded.backfilled);
        StatsManager::record_add_at_in(&path, &items, "new", Some(100), 1_700_100_000_000)
            .expect("record");
        let before = fs::read_to_string(&path).expect("read stats");

        // Simulate clear/delete history: raw history is now empty. A reload
        // must NOT re-seed or otherwise change the persisted stats.
        let after_clear = StatsManager::load_or_backfill_from(&path, &[]);
        assert_eq!(after_clear.totals.sessions, 2);
        assert_eq!(
            after_clear.totals.characters,
            count_characters("keep me") + count_characters("new")
        );
        assert_eq!(fs::read_to_string(&path).expect("read stats"), before);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn retention_prune_does_not_touch_stats() {
        let path = temp_path("prune");
        // 400 days old: far beyond HISTORY_RETENTION_DAYS (90). The raw
        // (unpruned) backfill must still count it...
        let old_ts = now_ms().saturating_sub(400 * 24 * 60 * 60 * 1000);
        let items = vec![item("old", old_ts, "pre-retention", Some(900))];
        StatsManager::load_or_backfill_from(&path, &items);
        // ...and once history is pruned down to empty, totals stay put.
        let store = StatsManager::load_or_backfill_from(&path, &[]);
        assert_eq!(store.totals.sessions, 1);
        assert_eq!(store.totals.characters, count_characters("pre-retention"));
        assert_eq!(store.totals.audio_ms, 900);
        assert_eq!(store.daily.len(), 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn stats_json_uses_camel_case_keys() {
        let mut store = StatsStore::default();
        StatsManager::accumulate(&mut store, "hi", Some(5), 0);
        let json = serde_json::to_value(&store).expect("serialize");
        assert!(json.get("totals").and_then(|t| t.get("audioMs")).is_some());
        assert!(json.get("totals").and_then(|t| t.get("audio_ms")).is_none());
        assert!(json.get("backfilled").is_some());
    }
}
