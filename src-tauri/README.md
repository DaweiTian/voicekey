# src-tauri/

Rust 主进程（Tauri v2）。app 入口 `main.rs` → `lib.rs::run()`：初始化日志、插件、
窗口事件、托盘与全局快捷键。

## 文件

- `src/main.rs` - 二进制入口，仅转发到 `voice_key_lib::run()`。
- `src/lib.rs` - Builder 装配：插件注册（**`tauri-plugin-single-instance` 必须注册在
  链首**：二次启动把 argv 交给首个实例后立即退出，回调里 `windowing::open_main_window`
  聚焦/重建已有实例主窗口，不会重复建托盘/overlay/热键、也不会竞态统计数据文件；
  其余含 `tauri-plugin-autostart`）、主窗口 CloseRequested → 隐藏到托盘并启动 5 分钟
  延迟销毁（`windowing`）（唯一真正退出路径是托盘 Quit）、overlay CloseRequested →
  `prevent_close` + `commands::hide_overlay`（悬浮胶囊绝不销毁，渲染层持有
  `core:window:allow-close`）、setup 时对现有窗口做 Windows 焦点框硬化首遍，并初始化
  鼠标中键 PTT（`mouse.rs`）与常驻待机 HUD 胶囊（`commands::show_overlay_idle`）。
- `src/windowing.rs` - 窗口生命周期共用逻辑：Windows 焦点框硬化（overlay：
  `WS_EX_NOACTIVATE` + `DWMWA_BORDER_COLOR=NONE` 幂等重施；**另剥离原生标题栏/边框
  样式位** `WS_CAPTION|WS_SYSMENU|WS_THICKFRAME|WS_MINIMIZEBOX|WS_MAXIMIZEBOX` 并配
  `SWP_FRAMECHANGED` 提交——tao 给每个窗口都带这些位、只靠收起非客户区隐藏，一旦存在
  非客户区 Windows 就会画原生标题栏（最小化/还原/关闭三键）；overlay 另在
  `tauri.conf.json` 用 `"shadow": false` 消除该非客户区（默认 `shadow: true` 会给无装饰
  窗口加 DWM 边框/阴影/圆角），此处样式剥离作二次兜底；overlay 另调
  `clear_dwm_window_backdrop` 用 `DWMWA_NCRENDERING_POLICY=DWMNCRP_DISABLED`、
  `DWMWA_WINDOW_CORNER_PREFERENCE=DWMWCP_DONOTROUND`、`DWMWA_SYSTEMBACKDROP_TYPE=DWMSBT_NONE`
  清掉 DWM 自绘的不透明底——Win11 默认 `DWMSBT_AUTO` 会给无边框置顶 `WS_POPUP` 胶囊
  自动套一层次生 Acrylic 背板，而背板是"采样其下层"的，故胶囊叠在自家浅色主窗口上才
  显白/灰矩形（叠桌面/他应用无此采样）；main：仅去 DWM 焦点框，
  **不加** `WS_EX_NOACTIVATE`，保留应用自绘 CSS 边框）。**回归警示**：
  `DWMWA_COLOR_NONE` 必须使用 windows-sys 导出的常量（= `0xFFFF_FFFE`，MSDN
  DWMWINDOWATTRIBUTE）；早前手写错误字面量 `0x0100_0000` 被 DWM 当作 COLORREF，
  边框被染成近黑色而非去除，令整项修复静默失效。**另注**：`DWMWA_*` 属性只需已启用的
  `Win32_Graphics_Dwm` feature；**不复用** tao 建窗时的 `DwmEnableBlurBehindWindow`
  空区域透明（`DWM_BLURBEHIND`/`CreateRectRgn` 需 `Win32_Graphics_Gdi`，当前未启用）；
  `tauri.conf.json` 的 `noRedirectionBitmap` 在本版 Tauri 是**死配置**
  （tauri-runtime-wry 从未调用 tao 的 `with_no_redirection_bitmap`），不要依赖。主窗口隐藏到托盘后延迟 5 分
  钟 `destroy()` 整个 main WebView 进程，5 分钟内重开则通过 generation 计数取消销毁；
  `ensure_main_window` 幂等重建（label 仍为 `"main"`，尺寸/位置/decorations 与
  `tauri.conf.json` 的 main 条目一致），`open_main_window` 为所有打开路径（托盘
  x2、settings 快捷键、`show_settings_window` 命令）的安全入口（重建在工作线程执行，
  避开 Windows 上 wry#583 主线程建窗死锁）。另有 `ensure_overlay_window`：悬浮胶囊
  正常只 hide/show，但若某条关闭路径真的销毁了 overlay 窗口，它按 `tauri.conf.json`
  的 overlay 条目同形幂等重建（同样只在工作线程调用），保证托盘"显示悬浮胶囊"永远
  能把胶囊找回来。
- `src/tray.rs` - **唯一**托盘来源（`tauri.conf.json` 不再配置 `app.trayIcon`，
  否则会出现两个图标）。右键菜单：Open Zhiruo / Settings / Launch at login
  （可勾选，纯 Rust 侧 enable/disable 并同步勾选态）/ Show floating capsule
  （可勾选，切换 HUD 常驻胶囊显隐并持久化到 `app.overlayVisible`；隐藏后 PTT
  快捷键与鼠标中键触发不受影响）。**该入口必须永远有效**：切换方向以窗口
  `is_visible()` 的真实状态为准（不信任持久化 flag，二者可能被关闭/隐藏路径弄失步），
  显示走 `commands::show_overlay_forced`（无视 flag 强制复开）、隐藏走
  `commands::hide_overlay`，两者都会把 flag 与勾选态同步到实际达到的状态
  （`set_overlay_menu_checked` 注册的 checkmark setter 供其他隐藏路径复用）/ Quit。
- `src/commands.rs` - 前端 invoke 命令。HUD overlay 为常驻待机胶囊（启动时显示，
  idle → recording → processing → success/error → 回 idle，会话结束不再隐藏），
  定位优先使用持久化拖拽位置 `app.overlayPosition`（越界时按所在显示器工作区重钳制，
  不回弹），否则「光标所在屏幕工作区底部居中、任务栏上方 ~48px」（
  `position_overlay_bottom_center` / `position_overlay`），先定位后显示，并在 show
  前后幂等重施 overlay 硬化（常驻 show 路径同样走 `overlay_show`，硬化必生效）；
  `overlay_show` 尊重 `app.overlayVisible`（托盘隐藏后自动 show 跳过，胶囊保持隐藏），
  但托盘入口走 `show_overlay_forced` 强制复开，且 overlay 窗口缺失时经
  `windowing::ensure_overlay_window` 在工作线程重建。每次 show/hide 都把
  `app.overlayVisible` 与托盘勾选态同步到真实状态（`hide_overlay` 同步隐藏语义；
  同步函数可从 overlay 重建工作线程调用：配置写盘走 `config.rs` 的 `CONFIG_LOCK`，
  托盘勾选态变更经 `run_on_main_thread` 派回主线程，失败记 `log::warn!`）。
  `set_overlay_size` 命令（渲染层契约：invoke 名 `set_overlay_size`、参数
  `{ width, height }`，逻辑像素）把 overlay `set_size` 为夹取后的尺寸
  （40..=400 × 32..=120）并经 `position_overlay` 保持持久化位置/底部居中。
  `set_overlay_position` 命令（渲染层契约：invoke 名 `set_overlay_position`、参数
  `x: f64 | null, y: f64 | null`，**物理像素**、直接来自窗口 `outerPosition()`，可选
  `capsule: { width, height }` 为可见胶囊实际尺寸、**逻辑像素**（DOM
  `offsetWidth/offsetHeight`））：拖拽落点按**胶囊可视区**吸附到所属显示器工作区最近的
  边（左/右/上/下），落点为**半隐半现（抽屉式）**——胶囊**中线**压在工作区边界线上，
  一半嵌进屏幕边、一半露出（`windowX = workLeft - capsuleOffsetX - capsuleW/2`，其余
  三向同理；因 `capsuleOffset + capsule/2 = window/2`，落点与胶囊尺寸无关，待机/会话
  两态胶囊共用同一中线、形变不跳位）。磁性阈值
  `OVERLAY_SNAP_THRESHOLD = 48` 物理像素：胶囊边距所有工作区边都超过阈值则**不吸附**，
  仅钳制（按胶囊中心）并按原地持久化。无 `capsule`（或 malformed——非数值 / 非有限 /
  超出窗口尺寸，静默忽略）时回退旧的窗口外框吸附（间隙 10 物理像素、无阈值、整窗
  钳制，行为不变）。吸附落点计算
  抽成纯函数 `snap_overlay_position`（窗口尺寸/胶囊尺寸/候选位置/work area → 吸附后
  窗口位置或 `None`），单测覆盖两态胶囊四边中线贴边 ≤2px（半隐）、超阈值不吸附、回退
  旧行为、malformed 校验与半隐坐标再钳制存活。落点钳制后移动窗口并持久化到
  `app.overlayPosition: {x, y}`（多显示器/DPI 安全，与 `position_overlay_bottom_center`
  同一套物理像素换算；钳制语义为**胶囊中心线**留在工作区内——中线贴边即半隐停靠位、
  合法，show/托盘复开/重启重锚定绝不回退半隐坐标，中心出界才拉回且至少露出半胶囊）；
  传 `null`（任一坐标）则清除持久化并回到底部居中默认位（默认/复位位走整窗钳制、
  **完整显示**，不经过吸附）。
  `OverlayFrame::apply` 在目标坐标与 `outer_position` 相同（±1 物理像素）时跳过
  `set_position`：同坐标 SetWindowPos 无视觉效果但会进入窗口过程，而
  `start_session_internal` 每次启动会话都途经此处且彼时可能仍有指针手势在途
  （0.5.3 的 pointercancel 教训），故不得空放。`start_session_internal` 是 PTT / 中键 / HUD pointerdown 三路的统一
  入口：仅 Idle / Completed / Error 可启动，Recording / Processing 一律拒绝并安静
  失败（防止转写/润色的 Processing 窗口内按 PTT 顶掉在途会话造成孤儿录音）；
  启动 / 停止 / 取消 / 出错路径都会清掉 mouse.rs 的中键会话 token。启动时同时
  调度 Rust 侧 5 分钟会话看门狗（`spawn_session_time_watchdog`，与渲染层
  `SESSION_MAX_DURATION_SECONDS` 的 `setTimeout` 同预算、双兜底）：到期仅当会话
  id 未变、仍 `Recording` 且 `audio::session_time_limit_reached`（按 `started_at`
  复核）判定过期时，走与渲染层定时器**相同**的 stop 管线（转写 + 注入，不走
  cancel 丢弃音频；Rust 侧即可释放会话状态并使鼠标 token 失效，麦克风关闭仍由
  渲染层收 `stop-recording` 事件完成）；与正常 stop 竞态时按会话 id 抢锁，抢不到
  静默让路，绝不发"无录音"假 toast。stop 管线的「没有录到声音」按会话时长分流
  （产品决策：误触/快速点击静默不报错）：无 PCM 且无 chunk 时，会话时长 <
  `EMPTY_SESSION_QUIET_MS`（250ms；`getUserMedia` 出流通常需 ~100-200ms）判为误触，
  静默 `Ok("")` 完成会话并直接回 idle（不 emit `error`/`transcription`、不弹 toast、
  不写历史/统计，渲染层丢弃 `stop_session` 返回值故空串安全）；≥250ms 视为真实
  麦克风故障，保留原「没有录到声音，请检查麦克风后重试。」错误。判定为纯函数
  `empty_session_is_quiet`（附单测），时长在 stop 抢锁时按 `started_at` 采样，
  不混入 flush 等待。
  `overlay_idle_later` 为 `pub(crate)`，`translate.rs` 直接复用（1.2s 回 idle）。
  `show_settings_window` 为 async 命令并经
  `windowing::open_main_window` 打开/重建主窗口。`set_config` 在 rebind 失败时回滚
  快捷键并返回 `{ config, hotkeyRolledBack }`（渲染层据此覆盖 hotkey 状态清除
  dirty），并即时应用 `mouse.middleClickPtt`（`mouse.rs` 钩子安装/卸载）；
  `set_hotkey_capture_active` 维护快捷键录制态守卫。
- `src/stats.rs` - 独立累计统计存储（`voice-key-stats.json`）：识别字符总数 /
  录音总时长 / 按日统计（`sessions` / `characters` / `audioMs`）只增不减，不受
  历史删除与 90 天裁剪影响——Home 的今日与近 N 日窗口也改读它（前端按日历日
  求和 `daily`），不再随历史缩水。字符口径与 `src/lib/stats.ts::countCharacters`
  对齐（剔除空白后计数），首次使用时从 **未裁剪**的历史原始条目幂等回填
  （`backfilled` 一次性标记）。**兼容老文件**：v1 的 `daily` 值是裸字符数，
  `DayStats` 自定义反序列化同时接受裸数字与当前对象形状，老文件升级时保留字符数
  （`sessions` / `audioMs` 补 0，此前未记录），绝不因形状不匹配把整文件读成默认
  值而清空终身统计。读-改-写全程持进程级 `STATS_LOCK`（`get` 的首跑回填与
  `record_add` 的累加并发时不会丢更新）；`StatsTotals` 带 `#[serde(default)]`，
  `totals` 缺字段不再导致整文件反序列化失败把终身统计静默重置。`stop_session`
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
  要求快速返回（~300ms 超时会被系统静默移除）；消息→动作判定为纯函数
  `classify` 且是**严格 `==` 白名单**（仅 519/520），滚轮（`WM_MOUSEWHEEL`=522 /
  `WM_MOUSEHWHEEL`=526）与其余鼠标消息一律不产生动作，因此按住中键期间滚动滚轮
  不会打断会话、也不影响 down/up 配对（附回归单测；**勿把该判定改成范围/阈值**，
  522/526 正落在 `WM_MBUTTONUP`(520) 与 `WM_MOUSELAST`(526) 之间，范围判定会重新
  引入“滚动打断录音”的 bug）；Up 事件**不立即停止**：worker 先等过一段短弹跳宽限窗
  （`RELEASE_BOUNCE_WINDOW`=120ms）——开关抖动（按住中键滚动滚轮的震动使触点瞬断）会
  产生「真实 WM_MBUTTONUP 紧跟 WM_MBUTTONDOWN」；窗内到达的按下即证明按键从未真正
  离开手指，该次 Up 判为弹跳吞掉、会话与 token 均保留（纯函数
  `release_after_bounce_window` + 回归单测），故弹跳只能**延后**停止、永不取消停止；
  窗内无按下即真实松开，执行停止。**勿再改回用 `GetAsyncKeyState` 复核按键态**：底层
  钩子回调在键的异步态更新**之前**被调用（LowLevelKeyboard/MouseProc 文档明示），worker
  紧随入队后读到的仍是事件发生前的「按下」态，真机下会把每次真实松开都误判为弹跳、只能
  等 5 分钟上限（即 P0「松中键停不下来」回归）；因果证据是成对的 down/up 事件，不是
  采样的键态。Disable 停止决策不进宽限窗（必须立即停）；一切 Tauri 调用（焦点守卫、
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
- `src/config.rs` - 配置存储（`voice-key-config.json`）。**配置写入线程安全**：
  所有写入口（`merge`：`set_config` 及其快捷键回滚、`test_refine_connection`、
  `set_app_language`、overlay 显隐 flag / 拖拽位置持久化）的
  load → mutate → save 全程持进程级 `CONFIG_LOCK`（与 `stats.rs` 的 `STATS_LOCK`
  同款），读也持锁防半截文件；overlay 重建工作线程、托盘/主线程、IPC 线程并发
  写不丢更新。锁内只做文件 I/O 与纯 JSON 合并、不嵌套其他锁（无锁序死锁风险）。
- `src/asr.rs` / `src/audio.rs` / `src/inject.rs` / `src/llm.rs` /
  `src/history.rs` / `src/platform.rs` / `src/state.rs` /
  `src/updater.rs` - 各子系统实现（详见各自文件头注释）。`audio.rs` 另含
  5 分钟会话上限常量 `SESSION_MAX_DURATION_SECONDS`（与渲染层常量同值）与
  超时判定纯函数 `session_time_limit_reached`（看门狗与单测共用）。
- `capabilities/default.json` - renderer 权限；托盘/自启动等主进程逻辑不需要
  capability。
- `icons-src/` - 图标源稿与生成脚本（`icon.svg` 主稿、`tauri icon` 输入位图、官网
  `og.svg` 分享图）。`icons/` 为生成产物，改图标走 `icons-src/README.md` 的流程。
- `icons/` - 打包图标全套（含 `icon.icns`），由 `npm run tauri -- icon` 从
  `icons-src/icon-1024.png` 生成；托盘图标取自 `app.default_window_icon()` 随之同步。

## 验证

```bash
cd src-tauri && cargo check
cd src-tauri && cargo test
```
