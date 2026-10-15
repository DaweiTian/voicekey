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

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("keyboard backend failed: {e}"))?;

    // Ctrl+V on Windows/Linux, Cmd+V on macOS
    let modifier = if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    };
    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| e.to_string())?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| e.to_string())?;
    enigo
        .key(modifier, Direction::Release)
        .map_err(|e| e.to_string())?;

    thread::sleep(Duration::from_millis(50));
    Ok(())
}

fn paste_via_hyprland(text: &str) -> Result<(), String> {
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

    thread::sleep(Duration::from_millis(75));

    // Shift+Insert is the conventional paste on Hyprland GUI targets.
    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("keyboard backend failed: {e}"))?;
    enigo
        .key(Key::Shift, Direction::Press)
        .map_err(|e| e.to_string())?;
    enigo
        .key(Key::Insert, Direction::Click)
        .map_err(|e| e.to_string())?;
    enigo
        .key(Key::Shift, Direction::Release)
        .map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(100));
    Ok(())
}

pub fn press_key(key: Key) -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.key(key, Direction::Click).map_err(|e| e.to_string())
}
