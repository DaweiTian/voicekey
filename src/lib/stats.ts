export interface HistoryItem {
  id: string
  text: string
  timestamp: number
  duration?: number
}

export interface DictationStats {
  totalCharacters: number
  totalAudioMs: number
  recentCharacters: number
  recentAudioMs: number
  todaySessions: number
  todayDuration: number
  todayCharacters: number
  activeDays: number
  peakDayCharacters: number
  peakDayKey: string | null
}

/**
 * Rust `src-tauri/src/stats.rs` 独立持久化的累计统计（`voice-key-stats.json`，
 * serde camelCase）。只增不减：不受历史删除与 90 天保留裁剪影响。
 */
export interface StoredDictationStats {
  version: number
  backfilled: boolean
  totals: {
    sessions: number
    characters: number
    audioMs: number
  }
  /** 本地日键 `YYYY-MM-DD` -> 当日识别字符数 */
  daily: Record<string, number>
}

/**
 * 非空白字符数，按 **Unicode 码点** 计数，与 Rust `src-tauri/src/stats.rs`
 * `count_characters`（`text.chars().filter(|c| !c.is_whitespace()).count()`）逐字对齐：
 * `Array.from` 把字符串拆成码点（emoji 等代理对计 1，而非 UTF-16 码元的 2），
 * 再剔除空白字符。历史字数、图表趋势与 Rust 累计统计共用这一口径，
 * 任何改动必须同步两侧。回归锁定见 `stats.test.ts`（镜像 Rust 单测
 * `count_characters_matches_frontend_whitespace_rule`）。
 */
export const countCharacters = (text: string): number =>
  Array.from(text).filter((c) => !/\s/u.test(c)).length

const sumValues = (values: number[]): number => values.reduce((total, value) => total + value, 0)

const buildDateKey = (date: Date): string => {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

/** 峰值日：按 dayKey 升序遍历、严格大于 → 平局取最早的一天（与 Rust `BTreeMap` 序一致）。 */
function derivePeak(daily: Map<string, number> | Record<string, number>): {
  peakDayCharacters: number
  peakDayKey: string | null
} {
  const entries = daily instanceof Map ? [...daily.entries()] : Object.entries(daily)
  let peakDayCharacters = 0
  let peakDayKey: string | null = null
  entries
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .forEach(([key, chars]) => {
      if (chars > peakDayCharacters) {
        peakDayCharacters = chars
        peakDayKey = key
      }
    })
  return { peakDayCharacters, peakDayKey }
}

/**
 * 从听写历史计算概览统计（今日 / 近 N 天 / 累计 / 活跃天数 / 峰值日）。
 *
 * 合并语义：累计类字段（totalCharacters / totalAudioMs / activeDays / 峰值日）
 * 优先取 `stored`（Rust 侧独立持久化、无限累加，历史删除 / 90 天裁剪不影响）；
 * 窗口类字段（今日、近 N 天）始终由 `historyItems` 实时聚合。`stored` 缺失
 * （老版本回退 / get_stats 失败）时优雅降级为纯历史聚合，不抛错。
 */
export function computeStats(
  historyItems: HistoryItem[],
  stored?: StoredDictationStats | null,
  rangeDays = 7,
): DictationStats {
  const historyTotalCharacters = sumValues(historyItems.map((item) => countCharacters(item.text)))
  const historyTotalAudioMs = sumValues(historyItems.map((item) => item.duration ?? 0))

  const now = new Date()
  const cutoff = new Date(now.getTime() - rangeDays * 24 * 60 * 60 * 1000)
  const recentItems = historyItems.filter((item) => item.timestamp >= cutoff.getTime())
  const recentCharacters = sumValues(recentItems.map((item) => countCharacters(item.text)))
  const recentAudioMs = sumValues(recentItems.map((item) => item.duration ?? 0))

  const todayStart = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate(),
    0,
    0,
    0,
    0,
  ).getTime()
  const todayEnd = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate(),
    23,
    59,
    59,
    999,
  ).getTime()
  const todayItems = historyItems.filter(
    (item) => item.timestamp >= todayStart && item.timestamp <= todayEnd,
  )
  const todaySessions = todayItems.length
  const todayDuration = sumValues(todayItems.map((item) => item.duration ?? 0))
  const todayCharacters = sumValues(todayItems.map((item) => countCharacters(item.text)))

  const dailyMap = new Map<string, number>()
  historyItems.forEach((item) => {
    const key = buildDateKey(new Date(item.timestamp))
    dailyMap.set(key, (dailyMap.get(key) ?? 0) + countCharacters(item.text))
  })

  const { peakDayCharacters, peakDayKey } = stored ? derivePeak(stored.daily) : derivePeak(dailyMap)

  return {
    totalCharacters: stored ? stored.totals.characters : historyTotalCharacters,
    totalAudioMs: stored ? stored.totals.audioMs : historyTotalAudioMs,
    recentCharacters,
    recentAudioMs,
    todaySessions,
    todayDuration,
    todayCharacters,
    activeDays: stored ? Object.keys(stored.daily).length : dailyMap.size,
    peakDayCharacters,
    peakDayKey,
  }
}
