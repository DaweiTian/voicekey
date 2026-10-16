# components/

## Microphone Selection

`AudioRecorder.tsx` accepts an optional microphone device ID from
`RecordingStartPayload`, records from that device when available, falls back to
the system default when unavailable, and asks main to stop if the stream ends
mid-session.

React 组件目录，包含应用级组件与 `ui/` 组件库。

## 子目录

### `ui/`

基于 shadcn/ui 与 Radix UI 的基础组件集合。

## 文件

### `AudioRecorder.tsx`

后台无头录音组件，负责：

- 监听主进程的录音开始/停止事件。
- 在单次会话内保持同一条 `MediaStream` 与 `AudioContext`。
- 经典模式每 30 秒轮转一次 `MediaRecorder` 并发送独立音频 `chunk`。
- 流式模式通过 AudioWorklet 每约 100 ms 发送 Float32 PCM；停止时先 flush 最后一帧再发送 final marker。
- 以 16 kHz `AudioContext` 采样压缩上报 PCM 体积（约 48 kHz 硬件默认的 1/3），设备拒绝自定义采样率时回退默认值；worklet 按实际采样率上报，Rust 侧照实编码 WAV。
- 在 5 分钟上限时自动请求停止会话。
- 向主进程同步音量电平与录音错误。

### `HUD.tsx`

overlay 窗口的紧凑状态胶囊（无边框透明窗口，由主进程定位到光标所在屏幕的工作区底部居中、任务栏上方约 48px 处；位置可被用户拖拽改变并由 Rust 持久化）。**窗口几何恒定**：overlay 窗口常驻 `tauri.conf.json` 的 280×64，**不再随状态缩放**——0.5.3 起彻底移除 renderer 的 `setOverlaySize` / 内容测量 / `scheduleOverlayResize` 路径（状态机也不再发出任何 resize 命令），因为「会话 / 手势进行中改变窗口尺寸」会在 WebView2 上打断指针手势（详见下文根因）。所有状态的内容尺寸都装得下 280×64（idle ≈104×36；recording / processing / success / error = 268×52），胶囊居中于窗口、不被裁切。**按下热区挂在整窗容器上**（外层 `fixed inset-0` 的容器带 `pointer-events-auto` 并承载 `pointerdown` / `dblclick` / `contextmenu`），待机胶囊 `<button>` 只作视觉主体。**手势后续（move / up / cancel / blur）一律由 window 级监听器驱动状态机**，容器不再直接承载 `pointerup` / `pointercancel`。鼠标事件语义：所有状态保持可点击（`set_ignore_mouse_events(false)`，不做穿透）——Tauri `set_ignore_cursor_events` 只有 bool、无 Electron 式 forward 转发，hover 穿透会让 `mouseenter` 不再可达（取消按钮将永久点不到），故不做 hover 穿透切换。胶囊为常驻待机启动器：启动后一直显示，状态机 idle → recording → processing → success/error → 回 idle（会话结束不再隐藏，仅托盘「显示悬浮胶囊」可隐藏）。待机胶囊**不设 `title`**（避免悬停弹出原生 tooltip），仅保留 `aria-label`；结果 / 处理文案与取消按钮的 `title` 保留。

**待机态（idle）交互（0.5.3 重设计，用户拍板）**：小尺寸低调胶囊（36px 高，小圆麦克风 + 「按住说话」）。**指针按下不再立即开录**——`pointerdown` 只开始一次「手势」，等手势歧义消除后才决定动作：

- **轻点 = 切换（latch）**：短按（松开早于 `HOLD_LATCH_MAX_MS = 500ms`、位移未超 `DRAG_THRESHOLD_PX = 8`）→ 松手时发出 `start-session` 但**不 stop**：胶囊进入持续录音态，说完**再点一下**（此时 overlay status 为 `recording` 且状态机 `latched === true`）→ `stop-session` 正常识别。状态机只多记一个 `latched` 标志（前后端会话状态仍以 Rust `status` 为准）。
- **按住 = PTT 保留**：按下停留满 `HOLD_LATCH_MAX_MS` 且未拖拽 → 进入 `holding` 并 `start-session`，松手 `stop-session`。
- **拖拽 = 只移窗**：位移超过 `DRAG_THRESHOLD_PX` → 进入 `dragging` 跟随指针移动窗口，**全程不启动、也不取消会话**。

指针事件处理与拖拽：

- **根因修复（0.5.2→0.5.3）+ 硬不变量**：旧实现（0.5.2）在 `pointerdown` 就 `start-session`；0.5.3 初版把 `pending` 解耦，但 **holding 阶段（500ms 阈值）仍会 resize**。两者都会让胶囊切换布局并 `set_overlay_size`（Rust `set_size` + `position_overlay` 重锚定），这次**在手势进行中**的窗口尺寸变化打断指针手势（WebView2 表现为 pointercancel / 捕获丢失）——0.5.2 表现为「拖不动 + 闪烁」，0.5.3 表现为「按住大胶囊闪一瞬骤停」。**定稿硬不变量：窗口几何在任何指针手势进行中（pending / holding / dragging）以及任何会话存活期间都不得改变**。实现方式：① 窗口常驻 280×64、renderer 不再 resize；② 状态机命令词汇里没有任何 resize / 重锚定命令（`overlayDrag.test.ts` 的 `window geometry invariant` 组锁定）。拖拽状态机在 `src/lib/overlayDrag.ts` 纯函数模块，HUD.tsx 仅保留薄的命令执行层。
- 由于 overlay 窗口标记为 `WS_EX_NOACTIVATE`，原生 `startDragging` 在某些平台可能无法捕获鼠标；因此采用**指针跟随 + `setPosition` 的自绘位移方案**，移动过程经 `requestAnimationFrame` 节流，释放后才调用 `setOverlayPosition` 持久化。
- 使用 `core:window:allow-outer-position` / `allow-set-position` / `allow-scale-factor` 读取窗口物理坐标、显示器缩放因子，并实时更新窗口位置；`pointerup` 后把最终坐标经 `setOverlayPosition` 交给 Rust 做贴边吸附、工作区钳制与持久化。persist 时**一次性只读测量**胶囊布局盒（`capsuleBoxRef` 的 `offsetWidth/offsetHeight`，逻辑像素）随坐标带给 Rust：吸附为**半隐半现（抽屉式）**——胶囊中线压在屏幕边、一半嵌入屏幕、一半露出（磁性阈值 48 物理像素），而不是按 280×64 窗口外框（否则待机胶囊吸附后会离屏幕边 ~(280-104)/2 ≈ 88px）。测量仅在手势结束后进行，不触发 resize / 重绘回路，不违反手势/会话期几何不变量。
- **防 pointerup 丢失**：`pointerdown` 时通过 `setPointerCapture(pointerId)` 把指针事件锁定到根容器；同时挂载 window 级别的 `pointermove` / `pointerup` / `pointercancel` / `blur` 监听器作为唯一的手势后续通道，并配**看门狗定时器**（仅 `pending` / `dragging` 状态 N 秒无事件则强制复位），确保异常路径不会把状态卡在 `dragging`。
- 选择位移阈值而非专用抓取区，是为了不缩减 PTT 热区、不在胶囊上增加额外装饰，且与系统常见拖拽判定一致。
- 拖拽释放后 Rust 会吸附到屏幕边缘；胶囊本身用 CSS `transition-[opacity,transform,filter]` 保持入场/按压缩放动效，吸附动画由窗口位置变化呈现。
- **双击复位保留**：第 1 击 latch 启动会话；第 2 击（`DOUBLE_CLICK_MAX_MS` 内、距上次轻点位移 ≤ `TAP_DISTANCE_PX`）走 `cancel-session`（静默，不出空录音报错），随后 `dblclick` 调用 `onDoubleClick` → `setOverlayPosition(null, null)`，Rust 恢复底部居中默认位——正好是「点错了撤销」。`lastTap` 在每次轻点 `pointerup` 记录并**跨下一次 `pointerdown` 保留**；dblclick 的门禁**不用 status**（第 2 击时 overlay 可能仍是 `recording`），而以 `onDoubleClick` 对 `lastTap` 的时效检查为准（PTT 按住不会留下新鲜 `lastTap`，故不会被误复位）。
- **latched 会话豁免误杀**：`onPointerCancel` / `onWindowBlur` / `onWatchdog` 的 `cancel-session` 只作用于「指针仍按下」的活跃 `pending` / `holding` / `dragging`；已松手的 latch 会话 phase 恒为 `idle`，上述路径对其早退。故点其他应用、窗口失焦或看门狗都不会误杀 latch 录音。`holding` 期间若指针被系统取消则 `stop-session`（保存音频而非丢弃）。
- **静音自动停止**：仅对 latched（已松手）会话生效——renderer 借 AudioRecorder 持续上报的音频电平（`onAudioLevel`）刷新「最后说话时刻」，静音超 `SILENCE_AUTO_STOP_MS = 5000ms` 自动 `stop-session`；PTT 按住不受影响。判定为 `overlayDrag.ts` 的纯函数 `isSilenceAutoStopDue`。

**录音 / 处理态**：268×52 深色玻璃胶囊 + 大圆角，状态只用左侧 28px 圆章的小面积色彩语义——录音为琥珀主色麦克风圆章（随音量呼吸 + 涟漪，无流式文本时右侧配迷你波形），处理为阶段色转圈包住阶段图标，成功为绿勾，错误为红叉；文字始终单行截断（`title` 提示全文），取消按钮仅在录音/处理态显示。**latched 录音仍可拖拽移窗**（录音继续）；在 latched 录音上轻点胶囊本体即停止识别。流式识别时实时文本尾部弱化 + 流光光标（录音与转写处理中均显示，尾部切分按 Unicode 码点）；配色为固定深色方案，不依赖明暗主题。

**视觉描边**：待机与录音 / 处理胶囊本体已去除 `border border-white/10` 矩形描边，改为极淡的 `ring-1 ring-white/[0.06]` 配合原有阴影，在浅色背景下维持轮廓但不再呈现明显矩形边框。状态圆章（ProcessingBadge / SuccessBadge / ErrorBadge）内部的 ring / border 保留，用于小面积语义区分。

**顶部细条关闭问题排查**：HUD.tsx 中没有任何关闭 / 隐藏 overlay 窗口的前端调用；可能触发会话结束的路径（`handleHoldEnd`、`handleCancel`）均不会隐藏窗口。原胶囊顶部的「细条」最可能是 CSS `border-white/10` 与透明窗口 / DWM 边框共同造成的视觉工件；移除胶囊边框后该视觉条应消失。若移除后点击胶囊顶部仍会关闭悬浮窗，则根因在 Rust / DWM 侧（`windowing.rs` 的 `suppress_focus_frame` 或窗口阴影处理），需由 Rust 侧继续排查。

点击不抢焦点依赖 Rust 侧 `WS_EX_NOACTIVATE` 硬化（见 `src-tauri/README.md`）。

### `SessionNotifications.tsx`

主窗口录音反馈监听组件（渲染 `null`）：订阅主进程 `error` / `transcription` / `session-status` 事件并用 sonner toast 呈现——错误与转写结果必报，`recording`/`processing` 等瞬态状态跳过以免刷屏；仅在 main 窗口挂载（overlay/background 窗口不响应）。

### `HotkeyRecorder.tsx`

快捷键录制组件，用于设置页录制和校验快捷键。录制开始/结束（提交、Esc、点击外部、失焦、卸载）时通过 `setHotkeyCaptureActive` 切换 Rust 端录制态守卫，防止录入组合键时误触发全局录音/翻译；AltGr 松开时的合成 Control keyup 被忽略，裸右修饰键提交集合内的 `*Right` token。

### `Waveform.tsx`

根据实时音频电平渲染波形动画（HUD 录音浮窗使用）：每根柱子采样略有延迟的历史电平形成行波，配合钟形包络与缓动跟随，静止时有轻微呼吸感。

### `VoiceWave.tsx`

品牌装饰声波：一排高度各异的竖条，`active` 时上下脉动，颜色继承 `currentColor`。纯装饰，不读取真实音频，用于侧栏待命卡、主页英雄区与历史记录。

### `HotkeyKeys.tsx`

将 Electron Accelerator 字符串渲染成一组实体键帽（按平台映射 ⌘ ⌥ ⇧ ⌃ 等符号）。

### `HotkeySettings.tsx`

快捷键配置卡片：录音、打开设置与翻译快捷键统一使用录制控件，含校验与重置；录音快捷键支持单修饰键。

### `AccentSelector.tsx`

强调色选择器：以色块 + 名称展示可用主题强调色，支持键盘导航与 `aria-checked` 选中态。

### `ChangelogDialog.tsx`

更新日志对话框：构建期嵌入仓库根 `CHANGELOG.md?raw`，按版本分节展示，最新版本在顶部。

### `LogViewerDialog.tsx`

日志查看对话框，负责展示主进程日志尾部内容。

### `StatsOverview.tsx`

主页统计磁贴行（累计字数 / 累计时长 / 活跃天数 / 峰值日），接收 `HomePage` 合并后的 `DictationStats`（累计字段来自 Rust 独立统计存储 `get_stats`，历史删除不清零）。

### `InteractiveCharts.tsx`

首页趋势图组件，聚合历史记录并绘制字符识别趋势，含 7/30/90 天分段切换；每日字符数走 `lib/stats.ts` 的 `countCharacters`（Unicode 码点口径），与统计页、历史页一致。

### `BrandMark.tsx`

芷若 (Zhiruo) 品牌标记：兰草叶片 + 声波弧的单色 SVG（`currentColor` 着色），与 `src-tauri/icons-src/icon.svg` 主图标造型一致。被 `TitleBar` 与 `MainLayout` 侧栏品牌区共用；改造型时两处同时生效。

### `TitleBar.tsx`

自定义窗口标题栏组件，提供：

- 品牌区（小 Logo + 应用名）
- 可拖拽区域（`data-tauri-drag-region`，双击最大化/还原）
- 最小化 / 最大化（还原） / 关闭按钮
- macOS 兼容（左侧留出交通灯空间，控制按钮保持右侧与 Windows 一致）
