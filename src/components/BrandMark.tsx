interface BrandMarkProps {
  className?: string
}

/**
 * 芷若品牌标记：兰草叶片 + 声波弧，与 `src-tauri/icons-src/icon.svg` 造型一致的单色版。
 * 全部走 currentColor，可直接放进渐变底色块中重新着色。
 */
export function BrandMark({ className }: BrandMarkProps) {
  return (
    <svg viewBox="0 0 24 24" fill="none" className={className} aria-hidden="true">
      {/* 叶片（实心） */}
      <path
        d="M5.9 18.1C6.4 14.2 9.5 11 13.4 10.6C12.9 14.5 9.8 17.7 5.9 18.1Z"
        fill="currentColor"
      />
      {/* 内侧声波弧（自叶尖向外发散） */}
      <path
        d="M13.17 8.01A2.58 2.58 0 0 1 16 10.83"
        stroke="currentColor"
        strokeWidth={1.7}
        strokeLinecap="round"
      />
      {/* 外侧声波弧 */}
      <path
        d="M13 6.16A4.45 4.45 0 0 1 17.84 11"
        stroke="currentColor"
        strokeWidth={1.4}
        strokeLinecap="round"
      />
    </svg>
  )
}
