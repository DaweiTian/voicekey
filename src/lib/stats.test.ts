import { describe, expect, it } from 'vitest'
import { computeStats, countCharacters, type HistoryItem, type StoredDictationStats } from './stats'

/**
 * 本套用例是 Rust 单测 `count_characters_matches_frontend_whitespace_rule`
 * （`src-tauri/src/stats.rs`）的 JS 镜像：两侧必须逐字使用同一计数口径——
 * **Unicode 码点**（Rust `chars()` / JS `Array.from`）剔除全部空白字符后计数。
 * 改任一侧的口径都要同步另一侧与本测试。
 */
describe('countCharacters (mirrors Rust count_characters_matches_frontend_whitespace_rule)', () => {
  it('drops every whitespace character and counts the rest (same asserts as Rust)', () => {
    // 前三个断言与 Rust 单测逐字相同，其后为 JS 侧补充边界
    expect(countCharacters('hello world')).toBe(10)
    expect(countCharacters(' 你 好\n\t世界 ')).toBe(4)
    expect(countCharacters('   \n\t')).toBe(0)
    expect(countCharacters('')).toBe(0)
    expect(countCharacters('你好 world')).toBe(7)
  })

  it('counts each emoji as one Unicode code point, not two UTF-16 units', () => {
    // "👋🏽a b" = 👋 + 🏽 + a + b = 4 个码点（Rust chars() 计 4）
    expect(countCharacters('👋🏽a b')).toBe(4)
    // 单个 emoji：码点口径 1；旧 UTF-16 口径（'👍'.length）会记 2
    expect(countCharacters('👍')).toBe(1)
    expect('👍'.length).toBe(2)
    // emoji 序列中间的空白同样剔除
    expect(countCharacters('👍 👍')).toBe(2)
  })

  it('treats Unicode space separators as whitespace like Rust is_whitespace', () => {
    // NBSP / EN QUAD / LINE SEPARATOR / 全角空格均按空白剔除
    expect(countCharacters('\u00a0a\u2000b\u2028c\u3000d')).toBe(4)
    // U+200B 零宽空格不是空白字符，两侧都计数
    expect(countCharacters('\u200b')).toBe(1)
    // 混合：CJK + emoji + 制表符 + 换行 = 你 好 👋 🏽 世 界 = 6 个码点
    expect(countCharacters('你好\t👋🏽\n世界')).toBe(6)
  })
})

/**
 * 问题④回归锁定：删除/清空历史后，Home 页所有统计**不得减少**。
 * 修复前「今日次数/今日时长/今日字数/近 7 日」由 historyItems 实时聚合，
 * 删历史即缩水；修复后 stored 存在时全部取自持久化统计（`daily`）。
 */
describe('computeStats (persisted window fields survive history deletion)', () => {
  const now = new Date()
  const buildKey = (d: Date) =>
    `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(
      2,
      '0',
    )}`
  const todayKey = buildKey(now)
  const yesterdayKey = buildKey(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1))

  const stored: StoredDictationStats = {
    version: 2,
    backfilled: true,
    totals: { sessions: 10, characters: 1000, audioMs: 60000 },
    daily: {
      [todayKey]: { sessions: 4, characters: 200, audioMs: 12000 },
      [yesterdayKey]: { sessions: 3, characters: 150, audioMs: 9000 },
    },
  }

  const historyItem = (
    id: string,
    text: string,
    duration: number,
    timestamp: number,
  ): HistoryItem => ({
    id,
    text,
    timestamp,
    duration,
  })

  const todaysHistory = [historyItem('1', 'hello', 3000, now.getTime())]

  it('reads today / recent window from stored, unaffected by emptying history', () => {
    const withHistory = computeStats(todaysHistory, stored)
    const withoutHistory = computeStats([], stored)
    // 唯一权威来源是 stored：删掉全部历史后每个字段都必须逐字相等。
    expect(withoutHistory).toEqual(withHistory)
    expect(withoutHistory.todaySessions).toBe(4)
    expect(withoutHistory.todayDuration).toBe(12000)
    expect(withoutHistory.todayCharacters).toBe(200)
    // 近 7 日 = 今天 + 昨天（日历日窗）
    expect(withoutHistory.recentCharacters).toBe(350)
    expect(withoutHistory.recentAudioMs).toBe(21000)
    expect(withoutHistory.totalCharacters).toBe(1000)
    expect(withoutHistory.activeDays).toBe(2)
    expect(withoutHistory.peakDayCharacters).toBe(200)
    expect(withoutHistory.peakDayKey).toBe(todayKey)
  })

  it('degrades to pure history aggregation when stored is missing', () => {
    const stats = computeStats(todaysHistory)
    expect(stats.totalCharacters).toBe(5)
    expect(stats.todaySessions).toBe(1)
    expect(stats.todayCharacters).toBe(5)
    expect(stats.todayDuration).toBe(3000)
    expect(stats.recentCharacters).toBe(5)
    expect(stats.activeDays).toBe(1)
  })
})
