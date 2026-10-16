# Voice Key Development Guide

## Overview

Voice Key is a Tauri 2 (Rust) + React + TypeScript desktop app for push-to-talk
voice input, transcription, optional text refinement/translation, history, and
text injection into the focused application. The Electron backend was replaced by
Tauri so the app uses the system WebView instead of a bundled Chromium; the React
product UI and the `window.electronAPI` renderer surface were kept intact.

## Current Structure

```text
voicekey/
  src-tauri/          Rust main process (Tauri v2). See src-tauri/README.md.
    src/
      main.rs         Binary entry; forwards to voice_key_lib::run().
      lib.rs          Builder wiring: plugins, window events, tray, global
                      hotkeys, setup-time focus hardening, middle-click PTT,
                      resident HUD capsule.
      commands.rs     Renderer invoke commands: session start/stop/cancel,
                      overlay lifecycle and position, set_overlay_size,
                      set_config with hotkey rollback, hotkey capture guard.
      config.rs       JSON config store (voice-key-config.json); partial updates
                      are merged, and API keys stay on this side of the bridge.
      history.rs      Transcript history store with 90-day retention.
      stats.rs        Independent cumulative stats store (voice-key-stats.json),
                      process-level lock, one-time backfill from raw history.
      windowing.rs    Window lifecycle: Windows focus-border hardening, hide to
                      tray with delayed main WebView destroy/rebuild.
      hotkey.rs       Global hotkey parsing/binding, left/right modifier tokens,
                      in-app and capture-active guards.
      mouse.rs        Windows-only mouse middle-button PTT (low-level hook on a
                      background message loop; no-op stub elsewhere).
      translate.rs    Selected-text translation orchestration (clipboard snapshot,
                      sentinel-based selection capture, LLM translate, restore).
      asr.rs          Local transcription (SenseVoice classic + X-ASR streaming).
      llm.rs          OpenAI-compatible refinement and translation calls.
      inject.rs       Text injection (enigo on Win/mac, wl-copy/wtype on Hyprland).
      audio.rs        Audio chunk handling and session audio plumbing.
      state.rs        Shared app state (config, session, guards).
      tray.rs         Single tray source: menu, launch-at-login, capsule toggle.
      platform.rs     Platform specifics (Hyprland/X11 detection and paths).
      updater.rs      GitHub Releases update checks and allowlisted links.
    build.rs          Tauri build script.
    Cargo.toml / Cargo.lock
    tauri.conf.json   App windows, security CSP, bundling, plugin config.
    capabilities/default.json  Renderer permissions.
    icons/            Bundler icons (referenced by tauri.conf.json).
  src/                React renderer (Vite).
    components/      React UI, HUD, recorder bridge, charts, logs, hotkey
                     widgets, custom TitleBar.
    components/ui/   shadcn/ui primitives.
    layouts/         Main app shell.
    lib/             Renderer utilities, stats, logger, theme, hotkey helpers,
                     tauri-api.ts (invoke/event bridge).
    pages/           Home, Settings, and History routes.
    shared/          Types, constants, i18n resources, URL/host helpers
                     (shared with tests via vitest).
    App.tsx          Window-label routing for main, overlay HUD, and background
                     recorder.
    main.tsx         Renderer entrypoint; installs the Tauri API bridge.
    i18n.ts          Renderer i18n bootstrap.
    index.css        Tailwind base and theme variables.
  website/           Astro static marketing site (Cloudflare deploy).
  public/            Desktop app static assets (PCM worklet, icons, tray icons).
  scripts/           Local CommonJS helper scripts.
```

Legacy, **not part of the build** — kept only as a functional reference for
porting behavior to Rust. Do not add features there; do not import from it.

```text
  electron/               Former Electron main/preload/shared code.
  electron-builder.json5  Former packaging config (Tauri bundler is used now).
  build/                  Former installer icon assets (src-tauri/icons/ is used).
  dist-electron/          Stale Electron build output.
```

Key root config files:

- `package.json` / `package-lock.json` - root npm scripts and app dependencies.
- `vite.config.ts` - React renderer build only (dev server on port 5173, output
  to `dist/`, `src-tauri/` watch-ignored). Rust is compiled by the Tauri CLI.
- `src-tauri/tauri.conf.json` - windows, CSP, bundling targets (`appimage`,
  `nsis`, `dmg`), plugin config.
- `eslint.config.mjs`, `prettier.config.mjs`, `commitlint.config.js` - quality tooling.
- `components.json` - shadcn/ui configuration.
- `tsconfig.json` / `tsconfig.node.json` - TypeScript configuration.

## Documentation Rules

Directory `README.md` files are part of the working map for this repo.

When reading code:

1. Read the target directory `README.md` before opening implementation files.
2. Read parent READMEs if the ownership or flow is unclear.
3. Use README file descriptions as a map, then verify against live code.

When changing code:

1. Update the relevant directory README when file purpose, behavior, exports, or
   directory structure changes materially.
2. Keep README descriptions concise and current; do not preserve historical plans.
3. If adding a new directory, add a short README for it.

## Commands

Development:

```bash
npm run dev         # tauri dev (starts the Vite dev server via beforeDevCommand)
npm run dev:vite    # renderer only, for browser preview (no Rust host)
npm run preview     # vite preview of the built renderer
```

Build:

```bash
npm run build       # tsc && vite build && tauri build
npm run build:vite  # tsc && vite build (renderer bundle only)
```

Rust-side verification (run from `src-tauri/`):

```bash
cd src-tauri && cargo check   # type/compile check for the main process
cd src-tauri && cargo test    # Rust unit tests (stats, history, ...)
```

Quality (frontend):

```bash
npm run lint
npm run lint:fix
npm run format
npm run format:check
npm run type-check
npm run test
npm run quality     # lint + format:check + type-check + test
```

Tauri CLI passthrough and release:

```bash
npm run tauri -- info
npm run release          # standard-version (also release:patch/minor/major)
```

Website:

```bash
npm run website:install
npm run website:dev
npm run website:build
npm run website:preview
```

Single-file examples:

```bash
npm run lint -- src/App.tsx
npm run lint -- src-tauri/
npm run format -- src/App.tsx
npm run type-check
```

## Versioning

The app version must stay identical in three places; lockfiles are refreshed by
the toolchain, not hand-edited:

- `package.json` → `version`
- `src-tauri/tauri.conf.json` → `version`
- `src-tauri/Cargo.toml` → `[package] version` (mirrored into `Cargo.lock` by
  `cargo check`/`cargo build`)

`CHANGELOG.md` follows the existing standard-version section style.

## Code Style

- Prettier: no semicolons, single quotes, trailing commas, 100-column width,
  2-space indentation, final newline.
- TypeScript strict mode is enabled.
- Avoid `any`; use concrete types or `unknown`.
- Unused parameters should be prefixed with `_`.
- Rust follows `rustfmt` defaults; keep module-level header comments that explain
  the invariant a module owns (several modules document regression traps).
- Prefer existing module boundaries and helper APIs over new abstractions.
- Keep the Rust main process in ownership of privileged operations: filesystem,
  config, hotkeys, mouse hooks, ASR, refinement, translation, history, stats,
  updater, tray, and text injection.
- Validate command payloads on the Rust side before using renderer-provided data.

## Renderer Bridge

- `src/lib/tauri-api.ts` is the single bridge: `installTauriApi()` is called in
  `src/main.tsx` and assigns a Tauri-backed object (invoke calls plus event
  listeners) to `window.electronAPI`, so existing components keep their shape.
- Add new capabilities as commands in `src-tauri/src/commands.rs` plus methods on
  that bridge, rather than calling `invoke`/`listen` ad hoc inside components.
- Renderer permissions live in `src-tauri/capabilities/default.json`; tray,
  autostart, and other main-process logic do not need a capability.

## UI And Styling

- Follow `src/index.css` theme variables.
- Use existing shadcn/ui primitives and local component patterns.
- Keep settings and operational UI dense, clear, and task-focused.
- Avoid marketing-page patterns inside the desktop app shell.
- The main window has no system decorations; `src/components/TitleBar.tsx` draws
  the title bar and app-drawn border instead. Do not re-enable decorations or add
  focus-ring styles that Windows 11 DWM would override.

## Security And Privacy

- Treat renderer data as untrusted at the invoke boundary.
- Do not log API keys, transcript bodies, or raw audio contents.
- Keep API keys in Rust-side config and never expose plaintext credentials to
  renderer windows; persistence deliberately avoids OS credential stores so
  managed macOS Keychains cannot block the app.
- Preserve the GitHub Releases URL allowlist for update links (`open_external`).
- Local ASR model assets stay under the platform app-data/config directory, not
  bundled into the installer; downloads are HTTPS-only with SHA-256 and size checks.

## Windows Encoding

Use UTF-8 for files and I/O. PowerShell may display Chinese text as mojibake;
verify with UTF-8-aware reads before concluding file content is corrupt.
