/**
 * Which app surface the current renderer window should render.
 *
 * Tauri creates every window from `src-tauri/tauri.conf.json` and the label
 * decides the surface: `main` (settings shell), `overlay` (draggable standby
 * capsule), `session` (fixed bottom-center session capsule), `background`
 * (headless recorder). In a browser preview (no Tauri) the location hash
 * stands in for the label.
 *
 * Kept as a pure function so the label / hash routing is unit-testable
 * (`window-kind.test.ts`); `App.tsx` owns the Tauri / DOM reads.
 */
export type WindowKind = 'main' | 'overlay' | 'session' | 'background' | 'unknown'

/**
 * Resolve the window kind from the Tauri window label (or `null` when not
 * running inside Tauri) plus the location hash (without the leading `#`).
 *
 * A recognized label always wins; otherwise the hash routes
 * `/overlay` → overlay, `/session` → session, empty or `/` → background
 * (the headless recorder is the browser-preview default), anything else →
 * main.
 */
export function resolveWindowKind(label: string | null, hash: string): WindowKind {
  if (label === 'main' || label === 'overlay' || label === 'session' || label === 'background') {
    return label
  }
  if (hash === '/overlay') return 'overlay'
  if (hash === '/session') return 'session'
  if (!hash || hash === '/') return 'background'
  return 'main'
}
