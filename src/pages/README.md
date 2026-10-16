# pages/

## Microphone Selection

`SettingsPage.tsx` detects available audio-input devices in the renderer, lets
the user persist a selected microphone, and keeps the system default as the
unchanged fallback. If Chromium or the OS replaces a saved device ID,
Settings automatically rebinds it only when exactly one currently available
device has the same exposed label; ambiguous duplicate labels still require an
explicit user choice.

路由页面组件目录。

## 文件

- `HomePage.tsx` - 首页仪表盘，显示 PTT 提示、统计汇总与区间趋势图；并行拉取历史与 `get_stats` 累计统计后合并计算（累计项不受历史删除/裁剪影响），趋势图仍按历史实时聚合。
- `SettingsPage.tsx` - 设置页：语言切换即时生效，其余设置防抖自动保存；在标准与实时识别模型卡片上互斥选择模式并打开各自目录，同时管理模型下载、麦克风、快捷键、开机自启、Linux/Omarchy 集成，以及 DeepSeek 排在首位的共用 LLM 连接和润色、语音输入翻译、选中文本翻译快捷键三个独立开关。启用任一功能时校验连接；目标语言随时可设置。Windows 下另显示「鼠标触发」卡片（中键按住说话开关，随防抖自动保存经 `set_config` 即时安装/卸载钩子；非 Windows 隐藏）。
- `settings-config.ts` - 设置页配置归一化、润色连接字段完整性判断、连接变更后的润色功能开关 reconcile（完整保留/恢复、不完整快照后关闭）、按连接指纹（provider/端点/模型/密钥）短时缓存连接检测结果（TTL 内切回不再重复实测）与保存后密钥遮罩同步（覆盖 LLM 各 provider 与 MiMo ASR 密钥）；仅替换本次已保存且未再次编辑的密钥，避免 IPC 往返覆盖并发输入。另含 `applyPersistedHotkeyState`：`set_config` 回滚快捷键（`hotkeyRolledBack`）时用持久化值覆盖渲染层 hotkey 状态并清除 dirty，防止非法值随每次自动保存反复重发、反复弹错；`normalizeRendererConfig` 同时补全 `mouse.middleClickPtt` 等新字段默认值。
- `settings-config.test.ts` - 设置页原有 ASR 模式选择兼容、润色连接完整性、Provider 切换功能开关 reconcile、连接指纹稳定性/失效与检测缓存 TTL、保存后密钥遮罩与并发编辑保留、MiMo 连接解析/归一化与 ASR 密钥遮罩回归测试。
- `HistoryPage.tsx` - 历史记录页：搜索、排序、分组与首屏优先渲染，支持复制、删除与清空。
