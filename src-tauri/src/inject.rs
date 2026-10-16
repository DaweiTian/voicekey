use crate::platform;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::process::Command;
use std::thread;
use std::time::Duration;

/// Inject text into the focused application.
/// Windows/macOS: clipboard paste (best fidelity for CJK + multiline).
/// Hyprland: wl-copy --sensitive + wtype paste.
pub fn inject_text(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }

    if platform::is_hyprland_session() {
        return paste_via_hyprland(text);
    }

    paste_via_clipboard(text)
}

fn paste_via_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| format!("clipboard write failed: {e}"))?;

    thread::sleep(Duration::from_millis(40));

    // Ctrl+V on Windows/Linux, Cmd+V on macOS
    chord(modifier_key(), Key::Unicode('v'))?;

    thread::sleep(Duration::from_millis(50));
    Ok(())
}

fn paste_via_hyprland(text: &str) -> Result<(), String> {
    hyprland_set_clipboard(text)?;

    thread::sleep(Duration::from_millis(75));

    // Shift+Insert is the conventional paste on Hyprland GUI targets.
    chord(Key::Shift, Key::Insert)?;
    thread::sleep(Duration::from_millis(100));
    Ok(())
}

/// Write text into the Wayland clipboard via `wl-copy --sensitive` (Hyprland).
/// Public so the selection-translation flow (translate.rs) can set / restore
/// clipboard contents on the same path paste uses.
pub fn hyprland_set_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new("wl-copy")
        .arg("--sensitive")
        .env("XDG_SESSION_TYPE", "wayland")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("wl-copy spawn failed: {e}"))?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("wl-copy write failed: {e}"))?;
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("wl-copy failed".into());
    }
    Ok(())
}

/// Ctrl on Windows/Linux, Cmd on macOS.
fn modifier_key() -> Key {
    if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    }
}

/// Press `modifier` down, tap `key`, release `modifier` (the clipboard chord
/// pattern used by copy / paste). No sleeps here: callers own their settle
/// delays.
fn chord(modifier: Key, key: Key) -> Result<(), String> {
    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("keyboard backend failed: {e}"))?;
    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| e.to_string())?;
    enigo
        .key(key, Direction::Click)
        .map_err(|e| e.to_string())?;
    enigo
        .key(modifier, Direction::Release)
        .map_err(|e| e.to_string())
}

/// Simulate the platform copy chord (Ctrl+C / Cmd+C) so the foreground app
/// writes its current selection to the clipboard. Used by the selection
/// translation flow (translate.rs); deliberately does not touch clipboard
/// contents itself.
pub fn simulate_copy() -> Result<(), String> {
    chord(modifier_key(), Key::Unicode('c'))
}

/// Simulate the platform paste chord *without* writing the clipboard: callers
/// that manage clipboard save/restore themselves (translate.rs) use this so
/// they can restore the original clipboard content afterwards.
pub fn simulate_paste() -> Result<(), String> {
    if platform::is_hyprland_session() {
        return chord(Key::Shift, Key::Insert);
    }
    chord(modifier_key(), Key::Unicode('v'))
}

pub fn press_key(key: Key) -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.key(key, Direction::Click).map_err(|e| e.to_string())
}
