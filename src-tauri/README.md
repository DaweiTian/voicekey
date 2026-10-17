# src-tauri/

Rust 主进程（Tauri v2）。app 入口 `main.rs` → `lib.rs::run()`：初始化日志、插件、
窗口事件、托盘与全局快捷键。

## 文件

- `src/main.rs` - 二进制入口，仅转发到 `voice_key_lib::run()`。
- `src/lib.rs` - Builder 装配：插件注册（**`tauri-plugin-single-instance` 必须注册在
  链首**：二次启动把 argv 交给首个实例后立即退出，回调里 `windowing::open_main_window`
  聚焦/重建已有实例主窗口，不会重复建托盘/overlay/热键、也不会竞态统计数据文件；
  其余含 `tauri-plugin-autostart`；**`AppState` 在 builder 链上 `manage`（早于任何窗口 /
  WebView 加载——配置窗口的 WebView 在 `setup()` 之前就会调用 `get_config`，一旦挪回
  `setup()` 即 `state not managed`，overlay HUD 会丢失持久化贴边形态、首启胶囊半出屏）**）、
  主窗口 CloseRequested → 隐藏到托盘并启动 5 分钟
  延迟销毁（`windowing`）（唯一真正退出路径是托盘 Quit）、overlay CloseRequested →
  `prevent_close` + `commands::hide_overlay`（悬浮胶囊绝不销毁，渲染层持有
  `core:window:allow-close`）、session CloseRequested → `prevent_close` +
  `commands::hide_session`（会话胶囊同样绝不销毁；**只隐藏、绝不动 overlay 的
  `app.overlayVisible`**，两窗显隐彻底解耦），`Focused` 分支与幽灵标题栏清理均按
  label 覆盖 main/overlay/session 三窗（session 复用 `harden_overlay_window` 同款
  硬化——同形同需求，漏掉即重蹈蓝边框/白条两个 bug），setup 时对现有窗口做 Windows
  焦点框硬化首遍，并初始化
  鼠标中键 PTT（`mouse.rs`）与常驻待机 HUD 胶囊（`commands::show_overlay_idle`）。`run()`
  **第一行**调 `windowing::enable_transparent_webview_background()`（必须在任何窗口/WebView
  创建之前，把 WebView2 垫底层设为全透明，见 `windowing` 条目「白闪」）。
- `src/windowing.rs` - 窗口生命周期共用逻辑：Windows 焦点框硬化（overlay：
  `WS_EX_NOACTIVATE` + `DWMWA_BORDER_COLOR=NONE` 幂等重施；**另剥离原生标题栏/边框
  样式位** `WS_CAPTION|WS_SYSMENU|WS_THICKFRAME|WS_MINIMIZEBOX|WS_MAXIMIZEBOX` 并配
  `SWP_FRAMECHANGED` 提交——tao 给每个窗口都带这些位、只靠收起非客户区隐藏，一旦存在
  非客户区 Windows 就会画原生标题栏（最小化/还原/关闭三键）；overlay 另在
  `tauri.conf.json` 用 `"shadow": false` 消除该非客户区（默认 `shadow: true` 会给无装饰
  窗口加 DWM 边框/阴影/圆角），此处样式剥离作二次兜底；两胶囊另用 `"focusable": false`
  （见下文幽灵标题栏「0.5.8 根因定案」：可聚焦窗口会在 `show()` 时被 tao 覆写掉
  `WS_EX_NOACTIVATE` 而遭激活）；overlay 另调
  `clear_dwm_window_backdrop` 用 `DWMWA_NCRENDERING_POLICY=DWMNCRP_DISABLED`、
  `DWMWA_WINDOW_CORNER_PREFERENCE=DWMWCP_DONOTROUND`、`DWMWA_SYSTEMBACKDROP_TYPE=DWMSBT_NONE`
  清掉 DWM 自绘的不透明底——Win11 默认 `DWMSBT_AUTO` 会给无边框置顶 `WS_POPUP` 胶囊
  自动套一层次生 Acrylic 背板，而背板是"采样其下层"的，故胶囊叠在自家浅色主窗口上才
  显白/灰矩形（叠桌面/他应用无此采样）；main：仅去 DWM 焦点框
  （`DWMWA_BORDER_COLOR=NONE`），
  **不加** `WS_EX_NOACTIVATE`，保留应用自绘 CSS 边框）。**主窗蓝色焦点边框（0.5.5 实测第三条）**：
  Win11 主窗获焦时 DWM 会重画系统强调色 1px 焦点边框——MSDN 明示 app「负责按状态变化（如窗口
  激活）改边框色」，故**创建时设一次不够**，必须在**每次激活重申** `DWMWA_BORDER_COLOR=DWMWA_COLOR_NONE`
  （= `0xFFFF_FFFE`；`DWMWA_COLOR_DEFAULT`=`0xFFFF_FFFF` 反而恢复强调色，勿用）。由 `lib.rs` 的
  `Focused` 事件处理器对**获焦窗口**按其 label 重申 hardening（main→`harden_main_window`、
  overlay/session→`harden_overlay_window`，均幂等且廉价）解决；overlay 与 session 因
  `WS_EX_NOACTIVATE` 通常不获焦，同设仅为预防。**该分支只做硬化重申与 `focused` 插桩，绝不
  再做任何几何变更**（0.5.9 删掉了原先在此的 `clear_capsule_ghost_titlebars` 幽灵清扫——它位移
  可见胶囊窗，正是点按瞬间白条的来源；详见 windowing 条目「0.5.9 根因定案」）。该属性需 Win11 build 22000+，旧系统静默忽略——已加 HRESULT `< 0` 检查 +
  `log::warn!` 便于诊断。**回归警示**：
  `DWMWA_COLOR_NONE` 必须使用 windows-sys 导出的常量（= `0xFFFF_FFFE`，MSDN
  DWMWINDOWATTRIBUTE）；早前手写错误字面量 `0x0100_0000` 被 DWM 当作 COLORREF，
  边框被染成近黑色而非去除，令整项修复静默失效。**另注**：`DWMWA_*` 属性只需已启用的
  `Win32_Graphics_Dwm` feature；**不复用** tao 建窗时的 `DwmEnableBlurBehindWindow`
  空区域透明（`DWM_BLURBEHIND`/`CreateRectRgn` 需 `Win32_Graphics_Gdi`，当前未启用）；
  `tauri.conf.json` 的 `noRedirectionBitmap` 在本版 Tauri 是**活配置**（0.5.7 核实更正：
  `tauri-runtime-wry` 2.12.0 `lib.rs` 建窗链逐窗传 `config.no_redirection_bitmap`，落到
  tao 0.37.1 的 `with_no_redirection_bitmap` → `WS_EX_NOREDIRECTIONBITMAP`；此前 README
  记为「死配置」有误），且 tauri 官方文档把它列为**透明窗白闪**的官方规避手段
  （`transparent()` / `no_redirection_bitmap()` 两处 doc 原文："can avoid the white flash
  that may appear before the webview content is rendered when using a transparent
  window"）；该字段只在 Windows 生效（mac/Linux 忽略）。**0.5.9 起 overlay + session 双胶囊窗
  全部启用**（两处 `"noRedirectionBitmap": true`：先只在 session 试验、A/B 收敛后补齐 overlay）。
  **A/B 实机收敛证据**：① session 窗带该标志 → 用户全部测试轮次多轮录音正常渲染，**从未**出现
  白条 / 白闪 / 空白；② overlay 窗不带 → 实机截图显示待机胶囊（`按住说话`）正上方一条**常驻**
  浅蓝白横条、条内左侧「芷若」文字，占据 420×96 窗的顶部区域（@1.5x 截图 ≈637×150，与窗几何
  吻合）——即宿主窗口标题层的**幽灵标题栏本体**；③ 0.5.9-1 摘除 focus 路径的可见窗轻推后，该条
  **由「一闪而过」转为「常驻」**（此前每次 Focused 的 1px 位移把它擦掉）——因果链闭合。
  **结论：病灶 = 窗口 redirection bitmap 缓存了带标题栏的旧帧**，正是 #14831/#15490 里 tauri
  维护者推荐 `noRedirectionBitmap` 的病类。**0.5.7 记录的 Sciter 反证（DComp 会影响 wry 的
  WebView2 `WS_CHILD` 子窗合成）对本栈（WebView2 155 + wry 0.57）正式判定为「不成立 / 已证伪」**
  ——实机证据（155 + 双窗带标志渲染完全正常）胜过论坛间接推断；wry 那段历史（#1513 传
  `--enable-features=RemoveRedirectionBitmap` → #1525 崩溃 → **#1572 移除**）是**浏览器旗标
  变体**的问题，与 `WS_EX_NOREDIRECTIONBITMAP` 窗口样式路径不同，同样不再作为否决依据。
  **回退步骤**：删掉 `overlay` 与 `session` 条目各一行 `"noRedirectionBitmap": true`（其余不动）。
  **失败判据（务必先看）**：若任一胶囊窗**内容空白 / 不渲染**，立即删掉该窗那一行并重建。
  **全链插桩**：`windowing::trace()` 把窗口操作直接追加进
  `voice-key.log`（渲染层 `log` 命令写的同一个文件——注意 Rust `log::*` 走的是 stderr，
  **不会**进这个文件，故不能用 `log::debug!` 代替），输出 `[win]` 行：session start 入口、
  overlay/session show 全链（含「已可见短路」分支）、每次 ghost-nudge（带 label，只在隐藏态）、
  `session hide` 的 park 坐标、`focused label=… focused=…`；每会话约 6-9 行，
  无转写/凭据，永久保留供后续白闪/白条/焦点问题定案。**WebView2 幽灵标题栏（0.5.4 残留白条的根因）**：样式剥离 + DWM 清理之后，胶囊
  上方仍可能出现一条浅色横条（横条文字 = 页面 `<title>`，即「芷若」）、随胶囊拖动、并在窗口
  激活 / 聚焦切换时出现、点胶囊（触发形变 = resize）或切走后又消失。它**不是窗口样式**
  （`GWL_STYLE` 已无 `WS_CAPTION`，正是两轮样式剥离除不掉的原因），而是 **WebView2 运行时
  144.x 回归**（tauri#14764 / WebView2Feedback#5492；Edge 146 起上游已修，未升级运行时的机器
  仍在）。**0.5.9 实机更正**：用户机运行时 **155** 上该条**仍复现，且为「常驻」**（实机截图：
  待机胶囊正上方一条浅蓝白横条、条内左侧「芷若」文字）——说明上游 146 的修复**未覆盖此变体**；
  本栈最终解 = **双胶囊窗 `noRedirectionBitmap`**（窗口 redirection bitmap 不再缓存带标题栏的旧
  帧，见上「A/B 实机收敛证据」），隐藏态轻推自此退化为对 144.x 老运行时的廉价兜底。**清除（masking）手段**是**改变窗口尺寸**：`clear_webview_ghost_titlebar` 对胶囊做一次
  「高度 +1 再还原」的 1px 轻推 —— 带 `AtomicBool` 重入守卫（轻推会触发 `Resized`，若
  `Resized` 处理器也轻推即死循环）。**0.5.9 起只在 `overlay_show` / `session_show`
  （`show_overlay_window` / `show_session_window`）的「窗口仍隐藏」阶段调用，且 `overlay`
  路径也已改成 `harden → position → 轻推（隐藏态）→ show → harden`**；**不再挂 `Focused`
  事件**（见下「0.5.9 根因定案」）。**新不变量：任何情况下不得对【可见】胶囊窗做 bounds 位移。**
  **0.5.9 根因定案（点按瞬间的「闪」＝我们自己造的；条本体仍在 155 上）**：主代理读本机 `voice-key.log`
  的 `[win]` 插桩，时间戳严丝合缝——13:06:47.917 `focused label=overlay focused=true` →
  `ghost-sweep (focus path)` → 13:06:47.918 `ghost-nudge label=overlay size=420x96`（**位移的是
  可见的 overlay 窗**）→ 13:06:48.006 `session start: entry (HUD tap)`；另有 focus 事件风暴
  （13:05:51.955–52.005 内 main/background/overlay 共 7 次 Focused，每次都对可见 overlay 轻推）
  与单次点按共 3-4 次可见窗位移。**0.5.8 的 `WS_EX_NOACTIVATE`/`focusable:false` 挡的是「激活」，
  挡不住 Focused 事件**（WebView2 子窗点击即取焦，tao 照样发 Focused）→ 于是 focus 路径上的
  `clear_capsule_ghost_titlebars` 每事件位移一次可见窗，而 bounds 变化正是 WebView2 出白帧的
  文档化触发面（WebView2Feedback#5607）；而它针对的幽灵标题栏在用户机 **155** 上**仍在**（见上
  「0.5.9 实机更正」：条本体一直存在，只是每次 Focused 的位移把它擦掉才表现为「一闪」）——
  **补丁本身成了闪烁病灶**（擦掉→闪；0.5.9-1 摘除后条转为常驻），最终由**双窗
  `noRedirectionBitmap`** 从源头消除（redirection bitmap 不再缓存带标题栏的旧帧）。修法：删掉 `lib.rs` Focused 分支的
  `clear_capsule_ghost_titlebars`（连带函数一并删除，零残留 dead code），overlay 显示路径的轻推
  挪到 `show()` 之前，隐藏态调用保留（对 144.x 老运行时仍是廉价兜底，对 155 是无害 no-op）。
  **0.5.8 根因定案（点击瞬间残留「芷若」白条的真正触发源）**：`WebviewWindow::show()` →
  tao `WindowState::set_window_flags(VISIBLE)` → `apply_diff` 会**先整字覆写**
  `SetWindowLongW(GWL_STYLE/GWL_EXSTYLE, to_window_styles())` **再** `ShowWindow(SW_SHOW)`；
  而 tao 的 `to_window_styles` 只在窗口**不可聚焦**时才带 `WS_EX_NOACTIVATE`
  （`if !flags.contains(FOCUSABLE)`），tauri 的 `focusable` 默认 `true` —— 于是每次 show 都把
  `prevent_activation` 设的 `WS_EX_NOACTIVATE` **抹掉**，紧随其后的 `ShowWindow(SW_SHOW)`
  便**真的激活了胶囊**。激活正是 #14764 文档化的幽灵触发面（"drag or focus change"），条上
  文字取自页面 `<title>`；这也回答了「为什么偏偏在点击那一瞬间」——点击是唯一 show 胶囊的时刻
  （`start_session_internal` → `session_show`；overlay 因 0.5.7 已可见短路而不 show）。0.5.6
  用 show 后的轻推把它清掉（同一轻推也是 0.5.7 修掉的**白背景**帧来源），0.5.7 把轻推挪到
  show 前 → 它原本负责清除的那条白条重新可见。**0.5.8 修法**：overlay/session 两窗
  `"focusable": false`（`tauri.conf.json` + 两个重建 builder 的 `.focusable(false)` +
  `harden_overlay_window` 内幂等 `set_focusable(false)` 断言），使 **tao 自己的样式永远带
  `WS_EX_NOACTIVATE`** → show 不可能激活胶囊 → 幽灵无触发面，且**不需要任何 show 后几何变更**
  （0.5.7 白闪修复与 0.5.4「手势/会话期间几何不可变」都不动）；轻推与 `Focused` 硬化自此降级
  为兜底（保留）。顺带堵住潜在缺陷：会话启动不再把前台从用户正在听写的应用抢走。
  **0.5.7 起 `show_overlay_window` 只在窗口「此前不可见」时才 show/硬化/轻推**（见
  `commands.rs` 条目）：常驻待机胶囊本就一直可见，会话启动再跑一遍等于对可见透明窗做一次
  无谓尺寸变化，正是白闪来源。另
  `clear_dwm_window_backdrop` 的三个 `DwmSetWindowAttribute` 返回值由 `let _ =` 改为查
  `HRESULT`、`< 0` 时 `log::warn!`（旧系统会静默拒绝 `DWMWA_SYSTEMBACKDROP_TYPE` 等新
  属性），便于诊断。**白闪（0.5.6 引入、0.5.7 修复）**：轻推 = **可见透明窗上的尺寸变化**；
  而透明窗一旦「表面被重建」（`hide()`/`show()`，或 bounds 变化），WebView2/DWM 可能在页面
  内容就位前先呈现一帧，呈现的正是 WebView2 的 `DefaultBackgroundColor` —— 默认**白色**
  （MSDN："renders this color when there is no web content loaded"）或窗口自身表面。于是 0.5.6
  用轻推换掉了白条却换来白闪，三条时序都对得上：① 点胶囊 → `start_session_internal` 重跑
  `overlay_show` 并对**已可见**的 overlay 轻推；② session 窗每次会话 `show()` 后紧跟一次轻推
  （= 可见态 bounds 变更，WebView2Feedback#5607 文档化白帧）；③ session 窗 `hide()` 本身也会
  闪（tauri#14515 "Creating a hidden transparent window results in a white flash when
  displayed"）。0.5.7 三管齐下（**不动 `transparent`**，故 mac/Linux 透明不受影响）：
  ① `show_overlay_window` 在窗口**已可见**时不再 show/硬化/轻推（见 `commands.rs` 条目）；
  ② `show_session_window` 把轻推挪到 `show()` **之前**（定位 → 轻推 → show，隐藏态重绘不可见；
  session 窗 `WS_EX_NOACTIVATE` 且不可拖拽，#14764 的两个触发面都不占，show 后轻推无可见职责）；
  ③ `hide_session` 先挪屏外（`session_park_position`，按真实显示器布局并集右下角再外推 1000px，
  非硬编码 -9999）再 `hide()`，隐藏帧物理不可见（tauri#14515 报告者实测配方）。另
  `windowing::enable_transparent_webview_background` 在 `run()` 第一行设
  `WEBVIEW2_DEFAULT_BACKGROUND_COLOR=00000000`（进程环境变量，WebView2 环境创建时读取一次）
  把垫底层改成全透明——MSDN 明示**只走属性**（wry 的 `SetDefaultBackgroundColor`，建窗时已设
  透明）"can still leave the app with a white flicker before the DefaultBackgroundColor
  property takes effect. Setting the color via environment variable solves this issue."；
  已存在的同名环境变量不覆盖（尊重外部覆盖）。主窗口隐藏到托盘后延迟 5 分
  钟 `destroy()` 整个 main WebView 进程，5 分钟内重开则通过 generation 计数取消销毁；
  `ensure_main_window` 幂等重建（label 仍为 `"main"`，尺寸/位置/decorations 与
  `tauri.conf.json` 的 main 条目一致），`open_main_window` 为所有打开路径（托盘
  x2、settings 快捷键、`show_settings_window` 命令）的安全入口（重建在工作线程执行，
  避开 Windows 上 wry#583 主线程建窗死锁）。另有 `ensure_overlay_window`：悬浮胶囊
  正常只 hide/show，但若某条关闭路径真的销毁了 overlay 窗口，它按 `tauri.conf.json`
  的 overlay 条目同形幂等重建（同样只在工作线程调用），保证托盘"显示悬浮胶囊"永远
  能把胶囊找回来。`ensure_session_window` 是会话胶囊（0.5.6 解耦新增的 `session`
  窗，`tauri.conf.json` 参数与 overlay 条目完全同源、共用 `harden_overlay_window`
  全套硬化）的同款兜底重建：会话窗正常只 hide/show，真被销毁后由会话启动路径在
  工作线程按 `session` 条目同形重建，保证「会话启动总能让下方居中胶囊出现」。
  `harden_app_windows` 的 label 分支同步覆盖 session（→ `harden_overlay_window`）。
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
  只渲染待机形态；会话态视觉在 session 窗），定位优先使用持久化拖拽位置
  `app.overlayPosition`（越界时按所在显示器工作区重钳制，
  不回弹），否则「光标所在屏幕工作区底部居中、任务栏上方 ~48px」（
  `position_overlay_bottom_center` / `position_overlay`，底部居中数学抽成纯函数
  `bottom_center_in` 并单测），先定位后显示，并在 show
  前后幂等重施 overlay 硬化（常驻 show 路径同样走 `overlay_show`，硬化必生效）；
  **0.5.7**：`show_overlay_window` 先查 `is_visible()`，\*\*已可见时只做 `position_overlay`
  - flag 同步，不再 `show()` / 硬化 / 轻推**——会话启动每次都走这条路径，而轻推是可见透明窗
    上的尺寸变化，正是白闪来源（硬化在首次显示时已施加、且由 `lib.rs` 的 `Focused` 处理器
    每次激活重申；轻推只在可见性变化时有意义）；
    `overlay_show` 尊重 `app.overlayVisible`（托盘隐藏后自动 show 跳过，胶囊保持隐藏），
    但托盘入口走 `show_overlay_forced` 强制复开，且 overlay 窗口缺失时经
    `windowing::ensure_overlay_window` 在工作线程重建。每次 show/hide 都把
    `app.overlayVisible` 与托盘勾选态同步到真实状态（`hide_overlay` 同步隐藏语义；
    同步函数可从 overlay 重建工作线程调用：配置写盘走 `config.rs` 的 `CONFIG_LOCK`，
    托盘勾选态变更经 `run_on_main_thread` 派回主线程，失败记 `log::warn!`）。
    **会话胶囊（`session` 窗，0.5.6 解耦）**：`start_session_internal` 在每次会话
    成功启动后（无论键盘 PTT / 鼠标中键 / hold / latch 触发）无条件
    `session_show`——**与 overlay 显隐、拖拽位置完全无关**（托盘隐藏悬浮胶囊后
    触发录音，下方居中大胶囊照常出现）；每次 show 把 session 窗一次性定位到
    光标所在监视器工作区底部居中（`position_session_bottom_center`，复用同一
    `bottom_center_in`；会话期间不再移动，且**绝不触碰 overlay 窗几何**——hold
    手势在指针下不受干扰）。**0.5.7 白闪修复的次序**：`show_session_window` 改为
    `harden → position_session_bottom_center → clear_webview_ghost_titlebar（轻推）
→ show()`——轻推（= bounds 变更）改在**隐藏态**执行，可见态不再有任何几何变更；
    `set_size` 用 `SWP_NOMOVE`（tao）故不会移动窗口。隐藏收口唯一：`overlay_emit_idle`
    （回 standby 的
    单一漏斗：成功/错误展示 1.2s 超时 `overlay_idle_later`、取消 `cancel_session`、
    误触静默、托盘强制 show 时无会话、翻译反馈超时）先 `hide_session` 再广播
    idle；`hide_session` 不带任何 flag 同步（与 `hide_overlay` 彻底解耦），且 0.5.7 起
    **先 `set_position(session_park_position)`（按全部显示器并集右下角外推 1000px 的屏外位）
    再 `hide()`**，让 `hide()` 可能呈现的帧物理不可见（tauri#14515 报告者实测配方），
    也使下次 show 从屏外起步（show 前必定重锚定）。翻译
    反馈（`translate.rs::emit_overlay`）经 `session_show_for_feedback` 点亮
    session 窗，会话进行中自动跳过（不重锚定）。会话窗口缺失时经
    `windowing::ensure_session_window` 在工作线程重建。
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
    `OVERLAY_SNAP_THRESHOLD = 48` 物理像素，距离**带符号**（正 = 距边尚有间隙、0 = 恰好
    接触、负 = 胶囊已溢出边外）：任一边距离 ≤ 阈值即吸附，**溢出（负距离）一律吸附**——
    拖过头是比接触更强的贴边信号（0.5.7 修复：旧实现对距离取 `abs()`，溢出 >48px 反被
    判为「离边太远」拒吸）；仅当距所有工作区边都超过阈值（且无溢出）才**不吸附**，
    仅钳制（按胶囊中心）并按原地持久化。无 `capsule`（或 malformed——非数值 / 非有限 /
    超出窗口尺寸，静默忽略）时回退旧的窗口外框吸附（间隙 10 物理像素、无阈值、整窗
    钳制，行为不变）。吸附落点计算
    抽成纯函数 `snap_overlay_position`（窗口尺寸/胶囊尺寸/候选位置/work area →
    `SnapLanding { pos, edge }` 或 `None`；`edge` 即吸附到的 `left`/`right`/`top`/
    `bottom`），单测覆盖两态胶囊四边中线贴边 ≤2px（半隐，且所报 edge 与目标边一致）、
    四边溢出 10px / 100px 仍吸附该边（`overflowing_drop_still_docks_on_every_edge`；多边
    同时满足时最强信号胜出、平手沿用 left > right > top > bottom）、超阈值不吸附、
    回退旧行为、malformed 校验、半隐坐标再钳制存活与紧凑 docked 态四边复吸附不抖动。落点钳制后移动窗口并
    持久化到 `app.overlayPosition: {x, y}`，同时同批写 `app.overlayDocked`
    （`"left"|"right"|"top"|"bottom"` 或 `null`；config.rs merge 字段级校验：未知字符串
    忽略、`null` 清除）（多显示器/DPI 安全，与 `position_overlay_bottom_center`
    同一套物理像素换算；钳制语义为**胶囊中心线**留在工作区内——中线贴边即半隐停靠位、
    合法，show/托盘复开/重启重锚定绝不回退半隐坐标，中心出界才拉回且至少露出半胶囊）；
    传 `null`（任一坐标）则 position 与 docked 一并清除并回到底部居中默认位
    （默认/复位位走整窗钳制、**完整显示**，不经过吸附）。
    **返回值** `Result<Option<String>, String>`（invoke 序列化为 JSON `string | null`，
    旧调用忽略返回值、向后兼容）：吸附成功（含溢出贴边）→ `Some(edge)`；超阈值仅钳制的
    落位与复位分支 → `None`。渲染层据此切换待机胶囊形态（贴边 → 紧凑仅图标；拖拽一进入 dragging
    即还原全形、松手 persist 返回值定夺），并经 `get_config` 读 `app.overlayDocked`
    在重启后恢复同一形态。返回值与配置补丁由单测
    `snap_reports_docked_edge_for_renderer_contract` /
    `config_patches_persist_dock_state_with_position` 覆盖。
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
    复核）判定过期时，走与渲染层定时器**相同\*\*的 stop 管线（转写 + 注入，不走
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
- `src/asr.rs` - 通用云端识别：按 `CloudDialect` 分叉请求载荷——`chat_asr_data_url`
  （MiMo / 千问：`input_audio.data` 传 data URL + 顶层 `asr_options`，与 0.5.11 载荷逐字节
  一致）、`chat_openai_audio`（OpenAI / 豆包 Ark：纯 base64 + `format:"wav"` + 转写指令
  part）、`openai_transcribe`（multipart `{base}/audio/transcriptions`，非流式）。流式
  `transcribe_cloud_stream`（SSE，HUD 边生成边显示）+ 非流式 `transcribe_cloud` 回退；audio
  管线共用 WAV 编码 `encode_wav_mono_f32`。本地识别（SenseVoice / X-ASR）已在 0.5.11 移除。
- `src/audio.rs` / `src/inject.rs` / `src/llm.rs` /
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
