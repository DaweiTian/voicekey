export interface ChangelogEntry {
  version: string
  date?: string
  body: string
}

const VERSION_HEADING_RE =
  /^#{2,3}\s+\[([^\]]+)\](?:\([^)]*\))?\s*(?:-\s*(\d{4}-\d{2}-\d{2})|\s*\((\d{4}-\d{2}-\d{2})\))?\s*$/

/**
 * 将仓库 CHANGELOG.md 按版本分节。
 *
 * 支持标准 standard-version 格式：
 * - `## [0.5.0] - 2026-10-16`
 * - `## [0.3.0](compare-link) (2026-09-01)`
 * - `### [0.2.1](compare-link) (2026-08-27)`
 *
 * 返回数组按 markdown 中顺序排列（最新版本通常在顶部）。
 */
export function parseChangelog(raw: string): ChangelogEntry[] {
  const lines = raw.split(/\r?\n/)
  const entries: ChangelogEntry[] = []
  let current: { version: string; date?: string; bodyLines: string[] } | null = null

  const flush = () => {
    if (current) {
      entries.push({
        version: current.version,
        date: current.date,
        body: current.bodyLines.join('\n').trim(),
      })
    }
  }

  for (const line of lines) {
    const match = line.match(VERSION_HEADING_RE)
    if (match) {
      flush()
      current = {
        version: match[1],
        date: match[2] ?? match[3],
        bodyLines: [],
      }
    } else if (current) {
      current.bodyLines.push(line)
    }
  }

  flush()
  return entries
}
