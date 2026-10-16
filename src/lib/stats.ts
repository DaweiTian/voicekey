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
export interface StoredDayStats {
  /** 当日成功识别会话数 */
  sessions: number
  /** 当日识别字符数（非空白，见 `countCharacters`） */
  characters: number
  /** 当日录音音频毫秒数（未知时长计 0） */
  audioMs: number
}

export interface StoredDictationStats {
  version: number
  backfilled: boolean
  totals: {
    sessions: number
    characters: number
    audioMs: number
  }
  /** 本地日键 `YYYY-MM-DD` -> 当日统计（次数 / 字符数 / 音频毫秒） */
  daily: Record<string, StoredDayStats>
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
 * 近 `rangeDays` 个**日历日**（含今天）的窗口求和，按本地 dayKey 逐日取
 * `stored.daily`。持久化日粒度只到「天」，无法还原历史聚合原来的滑动毫秒窗，
 * 但日历日窗更直观，且与 dayKey 口径一致。
 */
function sumRecentCalendarDays(
  daily: Record<string, StoredDayStats>,
  rangeDays: number,
  now: Date,
): { sessions: number; characters: number; audioMs: number } {
  let sessions = 0
  let characters = 0
  let audioMs = 0
  for (let i = 0; i < rangeDays; i += 1) {
    const entry =
      daily[buildDateKey(new Date(now.getFullYear(), now.getMonth(), now.getDate() - i))]
    if (!entry) continue
    sessions += entry.sessions
    characters += entry.characters
    audioMs += entry.audioMs
  }
  return { sessions, characters, audioMs }
}

/**
 * 从听写历史与 Rust 持久化统计计算概览（今日 / 近 N 天 / 累计 / 活跃天数 / 峰值日）。
 *
 * 合并语义：`stored` 存在时**所有字段**均取 `stored`（Rust 侧独立持久化、只增不减，
 * 历史删除 / 90 天裁剪不影响）——累计类直接读 `totals`；今日读 `daily[todayKey]`；
 * 近 N 天按日历日求和 `daily`。`stored` 缺失（老版本回退 / `get_stats` 失败）时
 * 整体降级为纯历史实时聚合，不抛错（此时窗口字段才会随历史删除而缩水）。
 */
export function computeStats(
  historyItems: HistoryItem[],
  stored?: StoredDictationStats | null,
  rangeDays = 7,
): DictationStats {
  const historyTotalCharacters = sumValues(historyItems.map((item) => countCharacters(item.text)))
  const historyTotalAudioMs = sumValues(historyItems.map((item) => item.duration ?? 0))

  const now = new Date()
  // 回退口径（仅 stored 缺失时使用）：滑动毫秒窗 + 当日时间戳过滤。
  const cutoff = new Date(now.getTime() - rangeDays * 24 * 60 * 60 * 1000)
  const recentItems = historyItems.filter((item) => item.timestamp >= cutoff.getTime())
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

  const historyDailyMap = new Map<string, number>()
  historyItems.forEach((item) => {
    const key = buildDateKey(new Date(item.timestamp))
    historyDailyMap.set(key, (historyDailyMap.get(key) ?? 0) + countCharacters(item.text))
  })

  const storedToday = stored?.daily[buildDateKey(now)]
  const storedRecent = stored ? sumRecentCalendarDays(stored.daily, rangeDays, now) : null
  const dailyCharsForPeak: Map<string, number> | Record<string, number> = stored
    ? Object.fromEntries(
        Object.entries(stored.daily).map(([key, day]) => [key, day.characters] as const),
      )
    : historyDailyMap
  const { peakDayCharacters, peakDayKey } = derivePeak(dailyCharsForPeak)

  return {
    totalCharacters: stored ? stored.totals.characters : historyTotalCharacters,
    totalAudioMs: stored ? stored.totals.audioMs : historyTotalAudioMs,
    recentCharacters: storedRecent
      ? storedRecent.characters
      : sumValues(recentItems.map((item) => countCharacters(item.text))),
    recentAudioMs: storedRecent
      ? storedRecent.audioMs
      : sumValues(recentItems.map((item) => item.duration ?? 0)),
    todaySessions: stored ? (storedToday?.sessions ?? 0) : todayItems.length,
    todayDuration: stored
      ? (storedToday?.audioMs ?? 0)
      : sumValues(todayItems.map((item) => item.duration ?? 0)),
    todayCharacters: stored
      ? (storedToday?.characters ?? 0)
      : sumValues(todayItems.map((item) => countCharacters(item.text))),
    activeDays: stored ? Object.keys(stored.daily).length : historyDailyMap.size,
    peakDayCharacters,
    peakDayKey,
  }
}
