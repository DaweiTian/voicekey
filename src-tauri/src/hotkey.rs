use crate::config::HotkeyConfig;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

#[derive(Clone, serde::Serialize)]
pub struct HotkeyEvent {
    pub action: String,
}

/// Map a normalized, lowercase key name to a `Code`.
///
/// Covers the extra tokens the renderer's `normalizeKey` can produce beyond
/// letters/digits: punctuation names, arrows, navigation keys and Enter/Tab.
fn named_key_code(name: &str) -> Option<Code> {
    Some(match name {
        "space" => Code::Space,
        "comma" | "," => Code::Comma,
        "period" | "." => Code::Period,
        "slash" | "/" => Code::Slash,
        "semicolon" | ";" => Code::Semicolon,
        "quote" | "'" => Code::Quote,
        "backslash" | "\\" => Code::Backslash,
        "bracketleft" | "[" => Code::BracketLeft,
        "bracketright" | "]" => Code::BracketRight,
        "minus" | "-" => Code::Minus,
        "equal" | "=" => Code::Equal,
        "backquote" | "`" => Code::Backquote,
        "enter" | "return" => Code::Enter,
        "escape" | "esc" => Code::Escape,
        "tab" => Code::Tab,
        "backspace" => Code::Backspace,
        "delete" | "del" => Code::Delete,
        "insert" | "ins" => Code::Insert,
        "up" | "arrowup" => Code::ArrowUp,
        "down" | "arrowdown" => Code::ArrowDown,
        "left" | "arrowleft" => Code::ArrowLeft,
        "right" | "arrowright" => Code::ArrowRight,
        "home" => Code::Home,
        "end" => Code::End,
        "pageup" => Code::PageUp,
        "pagedown" => Code::PageDown,
        _ => return None,
    })
}

/// Map "f1".."f24" to function keys.
fn function_key_code(name: &str) -> Option<Code> {
    let n: u8 = name.strip_prefix('f')?.parse().ok()?;
    Some(match n {
        1 => Code::F1,
        2 => Code::F2,
        3 => Code::F3,
        4 => Code::F4,
        5 => Code::F5,
        6 => Code::F6,
        7 => Code::F7,
        8 => Code::F8,
        9 => Code::F9,
        10 => Code::F10,
        11 => Code::F11,
        12 => Code::F12,
        13 => Code::F13,
        14 => Code::F14,
        15 => Code::F15,
        16 => Code::F16,
        17 => Code::F17,
        18 => Code::F18,
        19 => Code::F19,
        20 => Code::F20,
        21 => Code::F21,
        22 => Code::F22,
        23 => Code::F23,
        24 => Code::F24,
        _ => return None,
    })
}

/// Map a single ASCII alphanumeric character to a `Code`.
fn char_key_code(name: &str) -> Option<Code> {
    let mut chars = name.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(match c.to_ascii_uppercase() {
        'A' => Code::KeyA,
        'B' => Code::KeyB,
        'C' => Code::KeyC,
        'D' => Code::KeyD,
        'E' => Code::KeyE,
        'F' => Code::KeyF,
        'G' => Code::KeyG,
        'H' => Code::KeyH,
        'I' => Code::KeyI,
        'J' => Code::KeyJ,
        'K' => Code::KeyK,
        'L' => Code::KeyL,
        'M' => Code::KeyM,
        'N' => Code::KeyN,
        'O' => Code::KeyO,
        'P' => Code::KeyP,
        'Q' => Code::KeyQ,
        'R' => Code::KeyR,
        'S' => Code::KeyS,
        'T' => Code::KeyT,
        'U' => Code::KeyU,
        'V' => Code::KeyV,
        'W' => Code::KeyW,
        'X' => Code::KeyX,
        'Y' => Code::KeyY,
        'Z' => Code::KeyZ,
        '0' => Code::Digit0,
        '1' => Code::Digit1,
        '2' => Code::Digit2,
        '3' => Code::Digit3,
        '4' => Code::Digit4,
        '5' => Code::Digit5,
        '6' => Code::Digit6,
        '7' => Code::Digit7,
        '8' => Code::Digit8,
        '9' => Code::Digit9,
        _ => return None,
    })
}

/// Map a lone modifier token to its physical key so a bare `Control` / `Shift` /
/// `Command` / `Alt` (and their right-side variants) can be registered as a
/// push-to-talk key.
///
/// Token strings are matched case-insensitively against the renderer's
/// `normalizeKey` output (`Alt` / `AltRight` / `Control` / `ControlRight` /
/// `Shift` / `ShiftRight` / `Command`); keep both sides literally aligned.
fn bare_modifier_code(name: &str) -> Option<Code> {
    Some(match name.to_ascii_lowercase().as_str() {
        "control" | "ctrl" => Code::ControlLeft,
        "controlright" => Code::ControlRight,
        "shift" => Code::ShiftLeft,
        "shiftright" => Code::ShiftRight,
        "alt" | "option" => Code::AltLeft,
        "altright" => Code::AltRight,
        "super" | "meta" | "cmd" | "command" => Code::Super,
        _ => return None,
    })
}

fn parse_accelerator(accelerator: &str) -> Option<Shortcut> {
    // A lone modifier is a valid push-to-talk key (the renderer allows PTT to be a single
    // modifier), so resolve it to the physical key before parsing combinations.
    if !accelerator.contains('+') {
        if let Some(code) = bare_modifier_code(accelerator.trim()) {
            return Some(Shortcut::new(None, code));
        }
    }

    let mut mods = Modifiers::empty();
    let mut key: Option<Code> = None;
    let mut saw_key_token = false;

    for part in accelerator.split('+') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        match p.to_ascii_lowercase().as_str() {
            // Right-side modifiers collapse into the same Modifiers flags in
            // combinations: Tauri `Modifiers` has no left/right distinction.
            // (Bare right modifiers are resolved by `bare_modifier_code` above.)
            "control" | "ctrl" | "commandorcontrol" | "cmdorctrl" | "controlright" => {
                mods |= Modifiers::CONTROL
            }
            "shift" | "shiftright" => mods |= Modifiers::SHIFT,
            "altright" => mods |= Modifiers::ALT,
            "alt" | "option" => {
                // Inside this loop the accelerator always contains a "+", so Alt/Option acts
                // as a modifier here; a lone Alt is resolved by `bare_modifier_code`.
                if accelerator.contains('+') && key.is_some() {
                    mods |= Modifiers::ALT;
                } else if accelerator.contains('+') && key.is_none() {
                    // Could still be "Alt+Something" where Something comes later;
                    // treat as modifier first, and only if no key token is found
                    // we will fail the parse (bare Alt is handled below).
                    mods |= Modifiers::ALT;
                } else if key.is_none() {
                    saw_key_token = true;
                    key = Some(Code::AltLeft);
                } else {
                    mods |= Modifiers::ALT;
                }
            }
            "super" | "meta" | "cmd" | "command" => mods |= Modifiers::SUPER,
            "space" => {
                saw_key_token = true;
                key = Some(Code::Space);
            }
            "comma" | "," => {
                saw_key_token = true;
                key = Some(Code::Comma);
            }
            "enter" | "return" => {
                saw_key_token = true;
                key = Some(Code::Enter);
            }
            "escape" | "esc" => {
                saw_key_token = true;
                key = Some(Code::Escape);
            }
            "tab" => {
                saw_key_token = true;
                key = Some(Code::Tab);
            }
            other => {
                // Named keys (F1-F24, arrows, navigation, punctuation), single
                // characters (A-Z / 0-9), or renderer tokens such as "KeyT" /
                // "Digit5".
                if let Some(mapped) = named_key_code(other)
                    .or_else(|| function_key_code(other))
                    .or_else(|| char_key_code(other))
                    .or_else(|| {
                        other
                            .strip_prefix("key")
                            .and_then(char_key_code)
                            .or_else(|| other.strip_prefix("digit").and_then(char_key_code))
                    })
                {
                    saw_key_token = true;
                    key = Some(mapped);
                }
            }
        }
    }

    // Require a non-modifier key token so "Alt+Shift" alone is rejected,
    // but bare "Alt" / "Control+Shift+Space" works.
    if !saw_key_token && key.is_none() {
        return None;
    }
    let code = key?;
    Some(Shortcut::new(Some(mods), code))
}

pub fn bind_default_hotkeys<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let config = app
        .try_state::<crate::state::AppState>()
        .map(|s| s.config.load())
        .unwrap_or_default();
    // Never take the whole app down because one accelerator is unsupported.
    if let Err(e) = rebind(app, &config.hotkey) {
        log::warn!("hotkey binding failed: {e}");
        // Fall back to the built-in PTT shortcut so the app remains usable.
        let fallback = HotkeyConfig::default();
        if let Err(e2) = rebind(app, &fallback) {
            log::error!("fallback hotkey binding failed: {e2}");
        }
    }
    Ok(())
}

pub fn rebind<R: Runtime>(app: &AppHandle<R>, hotkeys: &HotkeyConfig) -> Result<(), String> {
    let manager = app.global_shortcut();
    let _ = manager.unregister_all();

    let mut errors: Vec<String> = Vec::new();
    if let Err(e) = bind_one(app, &hotkeys.ptt_key, "ptt") {
        errors.push(e);
    }
    if let Err(e) = bind_one(app, &hotkeys.toggle_settings, "settings") {
        errors.push(e);
    }
    if !hotkeys.translate_key.is_empty() {
        if let Err(e) = bind_one(app, &hotkeys.translate_key, "translate") {
            errors.push(e);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// While the settings hotkey recorder is capturing keystrokes, global hotkey
/// presses must not fire actions: the combos being recorded would otherwise
/// start a recording or trigger translation.
fn hotkey_capture_active<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<crate::state::AppState>()
        .map(|s| s.hotkey_capture_active())
        .unwrap_or(false)
}

/// True when the user is inside the app (main window focused). Push-to-talk is
/// app-external only, so PTT presses are ignored while the settings UI is up.
/// The main window may have been destroyed by the delayed reclaim, in which
/// case there is nothing focused and the hotkey must keep working.
/// Shared with the middle-button PTT guard (`mouse.rs`).
pub fn main_window_focused<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false)
}

fn bind_one<R: Runtime>(app: &AppHandle<R>, accelerator: &str, action: &str) -> Result<(), String> {
    let shortcut = parse_accelerator(accelerator)
        .ok_or_else(|| format!("Unsupported accelerator: {accelerator}"))?;
    let action = action.to_string();
    let handle = app.clone();

    app.global_shortcut()
        .on_shortcut(shortcut, move |app, _shortcut, event| {
            match action.as_str() {
                "ptt" => {
                    // Push-to-talk: press starts, release stops and transcribes.
                    if event.state() == ShortcutState::Pressed {
                        // Never start recording while the user is inside the app
                        // (main window focused) or while the settings hotkey
                        // recorder is capturing keystrokes: PTT is app-external only.
                        if hotkey_capture_active(app) || main_window_focused(app) {
                            return;
                        }
                        let state = app.state::<crate::state::AppState>();
                        let recording = state
                            .session
                            .lock()
                            .map(|s| s.status == crate::audio::SessionStatus::Recording)
                            .unwrap_or(false);
                        if !recording {
                            let _ = crate::commands::start_session_internal(app);
                        }
                    } else if event.state() == ShortcutState::Released {
                        // Release is never suppressed: if a session is already
                        // running (e.g. PTT was pressed outside the app and
                        // released after focus came back, or capture started
                        // mid-press) it must always be stopped and transcribed,
                        // otherwise the recording would hang forever. With no
                        // active session this is a no-op.
                        let state = app.state::<crate::state::AppState>();
                        let recording = state
                            .session
                            .lock()
                            .map(|s| s.status == crate::audio::SessionStatus::Recording)
                            .unwrap_or(false);
                        if recording {
                            let handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                // Errors are emitted to the UI inside the call;
                                // log them too so a failed stop is diagnosable.
                                if let Err(e) =
                                    crate::commands::stop_session_internal(&handle).await
                                {
                                    log::warn!("stop_session_internal failed: {e}");
                                }
                            });
                        }
                    }
                    let _ = handle.emit(
                        "hotkey",
                        HotkeyEvent {
                            action: "ptt".into(),
                        },
                    );
                }
                "settings" => {
                    if event.state() == ShortcutState::Pressed {
                        // Rebuilds the main window if the delayed destroy
                        // reclaimed it (windowing::ensure_main_window).
                        crate::windowing::open_main_window(&handle);
                    }
                }
                "translate" => {
                    if event.state() == ShortcutState::Pressed {
                        // The settings hotkey recorder owns the keyboard while
                        // capturing; the settings window itself is no
                        // translation target either (same app-external posture
                        // as PTT) — never fire translation from those
                        // keystrokes.
                        if hotkey_capture_active(app) || main_window_focused(app) {
                            return;
                        }
                        // Capture -> LLM -> paste can take up to a minute; run
                        // it on the async runtime (mirrors the PTT release ->
                        // stop_session spawn) so the shortcut handler never
                        // blocks. `translate::translate_selection` surfaces
                        // every failure itself (error toast + HUD capsule).
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            crate::translate::translate_selection(&handle).await;
                        });
                    }
                }
                _ => {}
            }
        })
        .map_err(|e| format!("Failed to bind {accelerator}: {e}"))
}
