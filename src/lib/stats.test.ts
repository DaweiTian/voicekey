import { describe, expect, it } from 'vitest'
import { countCharacters } from './stats'

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
