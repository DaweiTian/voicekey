//! 选中文字翻译编排（翻译快捷键触发）：捕获前台应用选区 → LLM 翻译 → 模拟粘贴回原焦点。
//!
//! 移植自遗留 Electron 蓝本 `electron/main/translation/translator.ts`（只读参考）：
//! 用「剪贴板哨兵 + 模拟 Ctrl+C + 轮询读取」判定是否真的选中了文字；`ClipboardGuard`
//! 的 `Drop` 在**每条**退出路径（成功 / 错误提前返回 / 意外 panic 展开）恢复剪贴板
//! 原始内容，绝不把哨兵或译文泄漏在剪贴板里。快照是三态的：`Text` 恢复原文本、
//! `Empty` 恢复为空（清掉哨兵）、`Unreadable` **跳过恢复**（覆写从未快照到的内容
//! 等于毁掉用户剪贴板），只在当前内容仍是我们的哨兵/译文残留时才清理。所有失败
//! 路径都通过 `error` 事件
//! （设置窗口 SessionNotifications toast）与 HUD 胶囊 `overlay-update` 给出可见中文
//! 提示，不静默吞错。

use crate::audio::SessionStatus;
use crate::state::AppState;
use crate::{inject, llm, platform};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime};

const CLIPBOARD_SENTINEL_PREFIX: &str = "voice-key-translation-copy-sentinel";
/// 等翻译快捷键松开后再发 Ctrl+C，避免松键事件干扰前台应用的选区。
const HOTKEY_RELEASE_DELAY_MS: u64 = 150;
/// 哨兵 / 译文写入剪贴板后等它稳定，再发读取剪贴板的按键。
const CLIPBOARD_WRITE_DELAY_MS: u64 = 80;
/// 部分应用异步响应 Ctrl+C：在预算时间内轮询等待剪贴板离开哨兵值，
/// 而不是固定睡一拍后只读一次（蓝本的单次读取是超时语义的近似）。
const SELECTION_COPY_POLL_BUDGET_MS: u64 = 600;
const SELECTION_COPY_POLL_INTERVAL_MS: u64 = 50;
/// 粘贴后给 GUI 时间消费内容，再把剪贴板换回原内容。
const GUI_PASTE_SETTLE_DELAY_MS: u64 = 500;

/// 翻译进行中标志（蓝本 `isTranslating`）：快捷键连按只跑一条流水线。
/// 通过 [`TranslatingGuard`] 的 RAII 语义复位，任何 return / panic 路径都不会
/// 把标志永久卡在 true。
static TRANSLATING: AtomicBool = AtomicBool::new(false);

/// `TRANSLATING` 状态守卫：`acquire` 成功后持有标志，`Drop` 时无条件复位。
struct TranslatingGuard;

impl TranslatingGuard {
    fn acquire() -> Option<Self> {
        TRANSLATING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for TranslatingGuard {
    fn drop(&mut self) {
        TRANSLATING.store(false, Ordering::SeqCst);
    }
}

/// 剪贴板快照的三态：把「读取失败」与「无文本」区分开，避免 Drop 把从未快照到的
/// 内容覆写为 ""。
enum ClipboardSnapshot {
    /// 快照到了纯文本，Drop 恢复之。
    Text(String),
    /// 快照时就没有文本（空剪贴板 / 无文本格式，如纯图片——见模块头取舍），
    /// Drop 写回空字符串以清掉哨兵。
    Empty,
    /// 快照读取失败（剪贴板被占用等）：原内容未知，Drop 跳过恢复。
    Unreadable,
}

/// 剪贴板快照守卫：`capture()` 记下原始内容，`Drop` 无条件处理（成功、错误
/// 提前返回、panic 展开都会触发）。
///
/// 取舍：只快照/恢复纯文本（`arboard` 文本通道），图片 / HTML / RTF 等富格式
/// 不在恢复范围——与 `inject.rs` 注入路径的剪贴板能力一致。
struct ClipboardGuard {
    original: ClipboardSnapshot,
    /// 最近一次**经由本守卫**写入剪贴板的文本（哨兵 / 译文）：恢复不可用时用它
    /// 识别"当前内容是我们自己的残留"，只清理我们写过的东西。
    last_written: Option<String>,
}

impl ClipboardGuard {
    fn capture() -> Self {
        let original = match read_clipboard_text() {
            Ok(text) if text.is_empty() => ClipboardSnapshot::Empty,
            Ok(text) => ClipboardSnapshot::Text(text),
            Err(e) => {
                log::warn!(
                    "translate: clipboard snapshot unreadable ({e}); restore will be skipped"
                );
                ClipboardSnapshot::Unreadable
            }
        };
        Self {
            original,
            last_written: None,
        }
    }

    /// 经由守卫写剪贴板（哨兵 / 译文），让 Drop 认得我们写过的残留。
    fn write_owned(&mut self, text: &str) -> Result<(), String> {
        write_clipboard_text(text)?;
        self.last_written = Some(text.to_string());
        Ok(())
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        match &self.original {
            ClipboardSnapshot::Text(text) => {
                if let Err(e) = write_clipboard_text(text) {
                    log::warn!("translate: clipboard restore failed: {e}");
                }
            }
            ClipboardSnapshot::Empty => {
                if let Err(e) = write_clipboard_text("") {
                    log::warn!("translate: clipboard clear failed: {e}");
                }
            }
            ClipboardSnapshot::Unreadable => {
                // 原内容从未快照成功，任何覆写都会毁掉未知的用户内容：跳过恢复。
                // 但我们自己的哨兵 / 译文绝不能泄漏给用户——仅当当前剪贴板内容仍
                // 是我们写入的残留时才清空。
                match read_clipboard_text() {
                    Ok(current)
                        if !current.is_empty()
                            && (current.starts_with(CLIPBOARD_SENTINEL_PREFIX)
                                || Some(&current) == self.last_written.as_ref()) =>
                    {
                        if let Err(e) = write_clipboard_text("") {
                            log::warn!("translate: clipboard residue cleanup failed: {e}");
                        }
                    }
                    Ok(_) => {
                        log::info!(
                            "translate: clipboard was unreadable at capture; skipping restore to preserve unknown content"
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "translate: clipboard unreadable at capture and at drop ({e}); skipping restore"
                        );
                    }
                }
            }
        }
    }
}

/// 读剪贴板文本。空剪贴板 / 无文本格式返回 `Ok("")`；剪贴板被占用等瞬时
/// 故障返回 `Err`，让轮询循环重试而不是误报"未选中文字"。
fn read_clipboard_text() -> Result<String, String> {
    if platform::is_hyprland_session() {
        return match std::process::Command::new("wl-paste")
            .arg("--no-newline")
            .env("XDG_SESSION_TYPE", "wayland")
            .output()
        {
            // wl-paste 在选区没有文本时以非零码退出。
            Ok(out) if out.status.success() => {
                String::from_utf8(out.stdout).map_err(|e| format!("wl-paste 输出非法: {e}"))
            }
            Ok(_) => Ok(String::new()),
            Err(e) => Err(format!("wl-paste 调用失败: {e}")),
        };
    }
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("剪贴板不可用: {e}"))?;
    match clipboard.get_text() {
        Ok(text) => Ok(text),
        // 剪贴板没有文本格式（例如纯图片）：视为空文本。
        Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
        Err(e) => Err(format!("读取剪贴板失败: {e}")),
    }
}

/// 写剪贴板文本（Hyprland 走 `wl-copy`，其余平台走 `arboard`）。
fn write_clipboard_text(text: &str) -> Result<(), String> {
    if platform::is_hyprland_session() {
        return inject::hyprland_set_clipboard(text);
    }
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("剪贴板不可用: {e}"))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| format!("写入剪贴板失败: {e}"))
}

/// 模拟 Ctrl+C 把选区写入剪贴板，用哨兵 + 超时轮询判定结果。
/// 哨兵经 `guard` 写入，Drop 才认得这是我们的残留。
/// `Ok(None)`：没有可用选区（哨兵未被替换，或复制到的内容为空白）。
async fn capture_selection(guard: &mut ClipboardGuard) -> Result<Option<String>, String> {
    let sentinel = format!("{CLIPBOARD_SENTINEL_PREFIX}:{}", uuid::Uuid::new_v4());
    guard.write_owned(&sentinel)?;
    tokio::time::sleep(Duration::from_millis(CLIPBOARD_WRITE_DELAY_MS)).await;

    inject::simulate_copy()?;

    let deadline = Instant::now() + Duration::from_millis(SELECTION_COPY_POLL_BUDGET_MS);
    let mut saw_sentinel = false;
    let mut last_read_error: Option<String> = None;
    loop {
        tokio::time::sleep(Duration::from_millis(SELECTION_COPY_POLL_INTERVAL_MS)).await;
        match read_clipboard_text() {
            // 哨兵被替换：Ctrl+C 复制到了新内容。
            Ok(current) if current != sentinel => {
                return Ok(if current.trim().is_empty() {
                    None
                } else {
                    Some(current)
                });
            }
            // 仍是哨兵：前台应用还没响应 Ctrl+C，继续等。
            Ok(_) => saw_sentinel = true,
            // 瞬时读失败（如剪贴板被占用）：预算内重试，超时后仅在从未读到过
            // 哨兵时才把读失败当错误上报。
            Err(e) => last_read_error = Some(e),
        }
        if Instant::now() >= deadline {
            return match (saw_sentinel, last_read_error) {
                (false, Some(e)) => Err(e),
                _ => Ok(None),
            };
        }
    }
}

/// 换行归一（蓝本 `normalizeClipboardText`）：选区复查时忽略 CRLF 差异。
fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// 语音会话是否正占用 HUD 胶囊（Recording / Processing）。
fn session_active<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<AppState>()
        .map(|s| {
            s.session
                .lock()
                .map(|st| matches!(st.status, SessionStatus::Recording | SessionStatus::Processing))
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// 更新 HUD 胶囊状态，除非语音会话正占用它：翻译反馈绝不能覆盖
/// recording / processing 显示。
fn emit_overlay<R: Runtime>(app: &AppHandle<R>, payload: Value) {
    if session_active(app) {
        return;
    }
    let _ = app.emit("overlay-update", payload);
}

// 翻译反馈（success / error）短暂停留后把胶囊送回待机：复用
// `commands::overlay_idle_later`（同款语义：若期间开始了新的语音会话则不动）。

/// 翻译快捷键入口：跑完整编排，并把结果反馈到 `error` toast 与 HUD 胶囊。
/// 由 `hotkey.rs` spawn 到 async runtime 上调用（参考 PTT 松开 →
/// `stop_session_internal` 的处理方式），绝不阻塞快捷键回调。
pub async fn translate_selection<R: Runtime>(app: &AppHandle<R>) {
    // RAII guard: the flag is reset on every return / panic path.
    let Some(_translating) = TranslatingGuard::acquire() else {
        log::warn!("translate: translation already in progress, ignoring trigger");
        return;
    };
    let result = run(app).await;

    match result {
        Ok(translated) => {
            log::info!("translate: selection translated");
            // 与会话成功路径同款反馈（HUD success 徽标 + 译文单行预览）。
            emit_overlay(
                app,
                json!({ "status": "success", "transcript": translated }),
            );
            crate::commands::overlay_idle_later(app);
        }
        Err(message) => {
            log::warn!("translate: {message}");
            // 设置窗口的 SessionNotifications toast（既有 error 事件契约）。
            let _ = app.emit("error", json!({ "message": message.clone() }));
            // 用户多半正对着别的应用：HUD 胶囊上也要有可见反馈。
            emit_overlay(app, json!({ "status": "error", "message": message }));
            crate::commands::overlay_idle_later(app);
        }
    }
}

/// 真正的编排（蓝本 `translate()` + `captureSelectedText()`），成功时返回译文。
/// 任何一步失败都带中文提示返回；剪贴板恢复由 `ClipboardGuard` 的 Drop 兜底。
async fn run<R: Runtime>(app: &AppHandle<R>) -> Result<String, String> {
    let config = app
        .try_state::<AppState>()
        .map(|s| s.config.load())
        .unwrap_or_default();

    // 1. 功能门控：与蓝本的静默跳过不同，禁用时给出明确的去处提示。
    if !config.translation.enabled {
        return Err("翻译功能未启用：请先在「设置 → 翻译」中开启后再使用翻译快捷键。".into());
    }

    // 2. 连接门控在任何剪贴板副作用之前（蓝本 `resolveConfig` 预检）。
    if !llm::connection_ready(&config.llm_refine) {
        return Err("翻译配置不完整：请先在「设置 → 智能润色」中配置 LLM 服务与 API Key。".into());
    }

    emit_overlay(
        app,
        json!({ "status": "processing", "processingStage": "translating" }),
    );

    // 3. 剪贴板快照（三态，见 ClipboardGuard）。此后的每条 return 路径都由
    //    Drop 处理剪贴板（恢复原文 / 清哨兵 / 不可读则跳过）。
    let mut guard = ClipboardGuard::capture();
    tokio::time::sleep(Duration::from_millis(HOTKEY_RELEASE_DELAY_MS)).await;

    // 4. 哨兵 + 模拟复制 + 轮询读取选区。
    let original = capture_selection(&mut guard)
        .await?
        .ok_or_else(|| "未选中任何文字：请先选中要翻译的文本，再按翻译快捷键。".to_string())?;

    log::info!(
        "translate: captured selection ({} chars)",
        original.chars().count()
    );

    // 5. LLM 翻译（复用 llm::translate_text，自带 60s 请求超时）。
    let target_language = if config.translation.target_language.trim().is_empty() {
        "English"
    } else {
        config.translation.target_language.trim()
    };
    let translated = llm::translate_text(&original, target_language, &config.llm_refine)
        .await
        .map_err(|e| format!("翻译失败：{e}"))?;
    if translated.trim().is_empty() {
        return Err("翻译结果为空，未写回译文，已保留原文。".into());
    }

    // 6. 粘贴前复查选区未变（蓝本 `confirmSelectionStillMatches`）：防止 LLM
    //    往返期间焦点 / 选区漂移后把译文贴到错误位置。
    match capture_selection(&mut guard).await? {
        Some(current)
            if normalize_line_endings(&current) == normalize_line_endings(&original) => {}
        _ => {
            return Err("选中内容已变化或已取消，为避免误替换未写回译文，请重新选中后重试。".into())
        }
    }

    // 7. 译文写入剪贴板（经守卫，Drop 才认得它是我们的残留）→ 模拟粘贴覆盖
    //    选区 → 等 GUI 消费；随后 `guard` 的 Drop 把剪贴板换回原内容（成功路径
    //    同样恢复）。
    guard.write_owned(&translated)?;
    tokio::time::sleep(Duration::from_millis(CLIPBOARD_WRITE_DELAY_MS)).await;
    inject::simulate_paste().map_err(|e| format!("模拟粘贴按键失败，译文未写回：{e}"))?;
    tokio::time::sleep(Duration::from_millis(GUI_PASTE_SETTLE_DELAY_MS)).await;

    log::info!("translate: translation pasted back into the focused app");
    Ok(translated)
}
