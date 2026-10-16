# Voice Key — Tauri Rewrite Design

## Goal

Replace Electron with Tauri 2 to cut idle memory (system WebView instead of bundled
Chromium) while keeping the existing React product UI and feature set.

## Style / Product Continuity

- Keep the current dense, task-focused desktop UI (shadcn + Tailwind).
- No marketing-page patterns inside the app shell.
- Same routes: Home / Settings / History, plus HUD overlay and hidden recorder.

## Architecture

```text
┌─────────────────────────────────────────────┐
│  WebViews (React + Vite)                    │
│  main · overlay HUD · background recorder   │
└──────────────────┬──────────────────────────┘
                   │ invoke / event (tauri)
┌──────────────────▼──────────────────────────┐
│  Rust core (src-tauri)                      │
│  config · history · hotkey · inject         │
│  audio session · ASR · LLM refine/translate │
│  tray · platform (Hyprland/X11) · updater   │
└─────────────────────────────────────────────┘
```

| Layer       | Choice                                                                 |
| ----------- | ---------------------------------------------------------------------- |
| Shell       | Tauri 2                                                                |
| UI          | React 18 + Tailwind 4 + shadcn (existing)                              |
| Config      | JSON under app config dir (`voice-key-config.json`)                    |
| History     | JSON store, 90-day retention                                           |
| Hotkeys     | `tauri-plugin-global-shortcut` + optional uiohook-style fallback later |
| Text inject | `enigo` (Win/mac) + `wl-copy`/`wtype` (Hyprland)                       |
| ASR         | sherpa-onnx via `sherpa-rs` (classic SenseVoice + streaming X-ASR)     |
| LLM         | `reqwest` → OpenAI-compatible chat completions                         |
| Download    | HTTPS + fixed SHA-256/size (same allowlist sources as Electron)        |

## Windows

| Window       | Role                                          |
| ------------ | --------------------------------------------- |
| `main`       | Home / Settings / History                     |
| `overlay`    | Always-on-top HUD (transcript, status)        |
| `background` | Hidden recorder (getUserMedia + AudioWorklet) |

## Security Defaults

- `withGlobalTauri: false`; frontend talks only through `invoke`.
- Config secret get restricted to the `main` window label.
- `open_external` allowlisted to GitHub Releases.
- Model downloads: HTTPS only + SHA-256 + size check.
- No `eval`, no remote HTML in the shell.

## Migration Map (Electron → Rust)

| Electron                          | Tauri                                             |
| --------------------------------- | ------------------------------------------------- |
| `config-manager.ts`               | `src-tauri/src/config.rs`                         |
| `history-manager.ts`              | `src-tauri/src/history.rs`                        |
| `hotkey/` + `iohook-manager`      | `src-tauri/src/hotkey.rs`                         |
| `text-injector.ts`                | `src-tauri/src/inject.rs`                         |
| `audio/`                          | `src-tauri/src/audio/` + recorder WebView         |
| `local-asr-*` / `streaming-asr-*` | `src-tauri/src/asr/`                              |
| `refine/` + `translation/`        | `src-tauri/src/llm/`                              |
| `tray/`                           | `src-tauri/src/tray.rs`                           |
| `platform/hyprland-*`             | `src-tauri/src/platform/`                         |
| `preload.ts`                      | `src/lib/tauri-api.ts` (same `electronAPI` shape) |

## Non-goals (this pass)

- Perfect parity of every platform edge case on day one.
- TokenDance OAuth browser flow (stubbed / follow-up).
- Installer polish and code signing.
