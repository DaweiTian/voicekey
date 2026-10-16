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

overlay 窗口的紧凑状态胶囊（无边框透明窗口，由主进程定位到光标所在屏幕的工作区底部居中、任务栏上方约 48px 处）。窗口尺寸随内容自适应：渲染层在每次状态 / 文案变化后于 `requestAnimationFrame` 中测量胶囊布局尺寸（外层无 transform 的测量容器 `getBoundingClientRect`，不受入场 / 按压缩放动画影响），经 `setOverlaySize`（Rust `set_overlay_size`，含尺寸合法性钳制与底部居中重定位）把窗口缩到内容大小 + 每侧 2px 内边距，因此不存在固定 280×64 矩形的不可见点击死区，整窗即可点击区域。鼠标事件语义：所有状态保持可点击（`set_ignore_mouse_events(false)`，不做穿透）——Tauri `set_ignore_cursor_events` 只有 bool、无 Electron 式 forward 转发，hover 穿透会让 `mouseenter` 不再可达（取消按钮将永久点不到），故不做 hover 穿透切换。胶囊为常驻待机启动器：启动后一直显示，状态机 idle → recording → processing → success/error → 回 idle（会话结束不再隐藏，仅托盘「显示悬浮胶囊」可隐藏）。待机态（idle）为小尺寸低调胶囊（36px 高，小圆麦克风 + 「按住说话」），**点击语义 = 按住说话**：`pointerdown` 开始录音、`pointerup` / `pointercancel` / `pointerleave` 结束并转写（左键 / 中键均可，与 PTT 心智一致，不做切换）；持有标记保证只结束本胶囊发起的会话，取消按钮的按下/松开经 `stopPropagation` 与按住说话互斥。录音态为 268×52 深色玻璃胶囊 + 细边框 + 大圆角，状态只用左侧 28px 圆章的小面积色彩语义——录音为琥珀主色麦克风圆章（随音量呼吸 + 涟漪，无流式文本时右侧配迷你波形），处理为阶段色转圈包住阶段图标，成功为绿勾，错误为红叉；文字始终单行截断（`title` 提示全文），取消按钮仅在录音/处理态显示。流式识别时实时文本尾部弱化 + 流光光标（录音与转写处理中均显示，尾部切分按 Unicode 码点）；配色为固定深色方案，不依赖明暗主题。点击不抢焦点依赖 Rust 侧 `WS_EX_NOACTIVATE` 硬化（见 `src-tauri/README.md`）。

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

### `LogViewerDialog.tsx`

日志查看对话框，负责展示主进程日志尾部内容。

### `StatsOverview.tsx`

主页统计磁贴行（累计字数 / 累计时长 / 活跃天数 / 峰值日），接收 `HomePage` 合并后的 `DictationStats`（累计字段来自 Rust 独立统计存储 `get_stats`，历史删除不清零）。

### `InteractiveCharts.tsx`

首页趋势图组件，聚合历史记录并绘制字符识别趋势，含 7/30/90 天分段切换；每日字符数走 `lib/stats.ts` 的 `countCharacters`（Unicode 码点口径），与统计页、历史页一致。

### `TitleBar.tsx`

自定义窗口标题栏组件，提供：

- 品牌区（小 Logo + 应用名）
- 可拖拽区域（`data-tauri-drag-region`，双击最大化/还原）
- 最小化 / 最大化（还原） / 关闭按钮
- macOS 兼容（左侧留出交通灯空间，控制按钮保持右侧与 Windows 一致）
