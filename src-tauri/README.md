# src-tauri/

Rust 主进程（Tauri v2）。app 入口 `main.rs` → `lib.rs::run()`：初始化日志、插件、
窗口事件、托盘与全局快捷键。

## 文件

- `src/main.rs` - 二进制入口，仅转发到 `voice_key_lib::run()`。
- `src/lib.rs` - Builder 装配：插件注册（含 `tauri-plugin-autostart`）、主窗口
  CloseRequested → 隐藏到托盘并启动 5 分钟延迟销毁（`windowing`）（唯一真正退出
  路径是托盘 Quit）、setup 时对现有窗口做 Windows 焦点框硬化首遍，并初始化鼠标
  中键 PTT（`mouse.rs`）与常驻待机 HUD 胶囊（`commands::show_overlay_idle`）。
- `src/windowing.rs` - 窗口生命周期共用逻辑：Windows 焦点框硬化（overlay：
  `WS_EX_NOACTIVATE` + `DWMWA_BORDER_COLOR=NONE` 幂等重施；main：仅去 DWM 焦点框，
  **不加** `WS_EX_NOACTIVATE`，保留应用自绘 CSS 边框）。**回归警示**：
  `DWMWA_COLOR_NONE` 必须使用 windows-sys 导出的常量（= `0xFFFF_FFFE`，MSDN
  DWMWINDOWATTRIBUTE）；早前手写错误字面量 `0x0100_0000` 被 DWM 当作 COLORREF，
  边框被染成近黑色而非去除，令整项修复静默失效。主窗口隐藏到托盘后延迟 5 分
  钟 `destroy()` 整个 main WebView 进程，5 分钟内重开则通过 generation 计数取消销毁；
  `ensure_main_window` 幂等重建（label 仍为 `"main"`，尺寸/位置/decorations 与
  `tauri.conf.json` 的 main 条目一致），`open_main_window` 为所有打开路径（托盘
  x2、settings 快捷键、`show_settings_window` 命令）的安全入口（重建在工作线程执行，
  避开 Windows 上 wry#583 主线程建窗死锁）。
- `src/tray.rs` - **唯一**托盘来源（`tauri.conf.json` 不再配置 `app.trayIcon`，
  否则会出现两个图标）。右键菜单：Open Voice Key / Settings / Launch at login
  （可勾选，纯 Rust 侧 enable/disable 并同步勾选态）/ Show floating capsule
  （可勾选，切换 HUD 常驻胶囊显隐并持久化到 `app.overlayVisible`；隐藏后 PTT
  快捷键与鼠标中键触发不受影响）/ Quit。
- `src/commands.rs` - 前端 invoke 命令。HUD overlay 为常驻待机胶囊（启动时显示，
  idle → recording → processing → success/error → 回 idle，会话结束不再隐藏），
  定位为「光标所在屏幕工作区底部居中、任务栏上方 ~48px」（
  `position_overlay_bottom_center`），先定位后显示，并在 show 前后幂等重施
  overlay 硬化（常驻 show 路径同样走 `overlay_show`，硬化必生效）；托盘隐藏后
  `overlay_show` 直接跳过。`set_overlay_size` 命令（渲染层契约：invoke 名
  `set_overlay_size`、参数 `{ width, height }`，逻辑像素）把 overlay `set_size`
  为夹取后的尺寸（40..=400 × 32..=120）并经 `position_overlay_bottom_center` 保持
  底部居中。`start_session_internal` 是 PTT / 中键 / HUD pointerdown 三路的统一
  入口：仅 Idle / Completed / Error 可启动，Recording / Processing 一律拒绝并安静
  失败（防止转写/润色的 Processing 窗口内按 PTT 顶掉在途会话造成孤儿录音）；
  启动 / 停止 / 取消 / 出错路径都会清掉 mouse.rs 的中键会话 token。
  `overlay_idle_later` 为 `pub(crate)`，`translate.rs` 直接复用（1.2s 回 idle）。
  `show_settings_window` 为 async 命令并经
  `windowing::open_main_window` 打开/重建主窗口。`set_config` 在 rebind 失败时回滚
  快捷键并返回 `{ config, hotkeyRolledBack }`（渲染层据此覆盖 hotkey 状态清除
  dirty），并即时应用 `mouse.middleClickPtt`（`mouse.rs` 钩子安装/卸载）；
  `set_hotkey_capture_active` 维护快捷键录制态守卫。
- `src/stats.rs` - 独立累计统计存储（`voice-key-stats.json`）：识别字符总数 /
  录音总时长 / 按日字符数只增不减，不受历史删除与 90 天裁剪影响；字符口径与
  `src/lib/stats.ts::countCharacters` 对齐（剔除空白后计数），首次使用时从
  **未裁剪**的历史原始条目幂等回填（`backfilled` 一次性标记）。读-改-写全程持
  进程级 `STATS_LOCK`（`get` 的首跑回填与 `record_add` 的累加并发时不会丢更新）；
  `StatsTotals` 带 `#[serde(default)]`，`totals` 缺字段不再导致整文件反序列化失败
  把终身统计静默重置。`stop_session`
  里在 `HistoryManager::add` 之前累加（避免首跑回填重复计数），`get_stats`
  命令返回该结构。
- `src/hotkey.rs` - 全局快捷键解析/绑定：bare token `AltRight/ControlRight/ShiftRight`
  映射 `Code::AltRight/ControlRight/ShiftRight`（与渲染层 `normalizeKey` 逐字对齐），
  组合键内归一为无左右位的 Modifiers；PTT 按下在主窗口聚焦或快捷键录制态
  （`state.rs::hotkey_capture_active`）时不启动录音，松开永远执行停止/转写
  （防"应用外按下、应用内松开"卡死）。translate 分支（仅 Pressed）受主窗口聚焦 +
  录制态双守卫，触发时 spawn `translate::translate_selection`（不再向渲染层
  emit 无任何监听者的 `translate` 事件）。
  `main_window_focused` 为 `pub`，供 `mouse.rs` 中键守卫只读复用（逻辑不变）。
- `src/mouse.rs` - 鼠标中键 PTT（仅 Windows）：独立后台线程跑消息循环并持有
  `SetWindowsHookExW(WH_MOUSE_LL)` 钩子；`WM_MBUTTONDOWN` 启动会话、
  `WM_MBUTTONUP` 停止，观察不吞键（始终 `CallNextHookEx`）。钩子回调只做**本地
  Win32 调用**（`WindowFromPoint`/`GetAncestor` 命中检测）+ 非阻塞入队——LL 钩子
  要求快速返回（~300ms 超时会被系统静默移除）；一切 Tauri 调用（焦点守卫、
  overlay 判定、启动会话）由**单个串行 worker** 按事件顺序处理，Start 记录所启动
  会话的 id 作 token，Stop 仅当 `session.id == token && status == Recording` 才停
  并清 token，其余启动/停止/取消/出错路径也清 token（`clear_owned_session`），
  保证松中键永不误杀键盘 PTT / HUD 启动的会话、也不产生虚假错误 toast；快速
  click（down 紧接 up）因串行处理天然不留孤儿录音。守卫与键盘 PTT 一致（主窗口
  聚焦 / 快捷键录制态不触发）；落在 HUD 胶囊上的中键（钩子内记下的 root hwnd 与
  overlay 比对）跳过（归悬浮胶囊处理）。`mouse.middleClickPtt`（默认 false）即时
  控制钩子安装/卸载（线程安全；禁用时补发一次 stop 决策防孤儿录音）；
  非 Windows 为 no-op stub。
- `src/translate.rs` - 选中文字翻译编排（翻译快捷键触发，移植自遗留 Electron 蓝本
  `electron/main/translation/translator.ts`）：剪贴板快照 → 模拟 Ctrl+C +
  哨兵写入/超时轮询读取判定选区 → `llm::translate_text` → 粘贴前复查选区未变 →
  译文入剪贴板 + 模拟粘贴覆盖选区。`ClipboardGuard` 在 Drop 中处理原剪贴板
  （成功 / 失败 / 提前返回全覆盖，绝不泄漏哨兵或译文；仅恢复纯文本，取舍见模块
  头注释）。剪贴板快照是三态的（`Text` / `Empty` / `Unreadable`）：初始读取失败
  与"无文本"严格区分，`Unreadable` 时**跳过恢复**（绝不覆写从未快照到的用户内容，
  只清理我们自己的哨兵/译文残留）并记日志。未启用翻译 / LLM 配置不完整 / 未选中
  文字 / LLM 失败 / 译文为空 /
  选区变化 / 粘贴失败均 emit `error` 事件（设置窗口 toast）+ HUD 胶囊可见提示；
  `llm::connection_ready` 预检在任何剪贴板副作用之前；`TRANSLATING` 原子标志防
  重入（RAII guard 在 Drop 内复位，panic / 提前返回都不会卡在"翻译中"），全流程
  spawn 到 async runtime 执行，不阻塞快捷键回调；反馈后 1.2s 回 idle 复用
  `commands::overlay_idle_later`；Hyprland 会话剪贴板
  走 `wl-paste`/`wl-copy`（`inject.rs`），其余平台走 `arboard`。
- `src/asr.rs` / `src/audio.rs` / `src/inject.rs` / `src/llm.rs` /
  `src/history.rs` / `src/config.rs` / `src/platform.rs` / `src/state.rs` /
  `src/updater.rs` - 各子系统实现（详见各自文件头注释）。
- `capabilities/default.json` - renderer 权限；托盘/自启动等主进程逻辑不需要
  capability。

## 验证

```bash
cd src-tauri && cargo check
```
