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
/// `Command` / `Alt` can be registered as a push-to-talk key.
fn bare_modifier_code(name: &str) -> Option<Code> {
    Some(match name.to_ascii_lowercase().as_str() {
        "control" | "ctrl" => Code::ControlLeft,
        "shift" => Code::ShiftLeft,
        "alt" | "option" => Code::AltLeft,
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
            "control" | "ctrl" | "commandorcontrol" | "cmdorctrl" => mods |= Modifiers::CONTROL,
            "shift" => mods |= Modifiers::SHIFT,
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
                        if let Some(win) = handle.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                }
                "translate" => {
                    if event.state() == ShortcutState::Pressed {
                        let _ = handle.emit(
                            "translate",
                            HotkeyEvent {
                                action: "translate".into(),
                            },
                        );
                    }
                }
                _ => {}
            }
        })
        .map_err(|e| format!("Failed to bind {accelerator}: {e}"))
}
