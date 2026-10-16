# lib/

通用工具函数目录。

## 文件

- `utils.ts` - `cn()`：合并 `className`（`clsx` + `tailwind-merge`）。
- `hotkey-utils.ts` - 快捷键工具：规范化、构建、格式化与校验 Accelerator，并内置系统保留键规则；修饰键按物理键 `code` 区分左右（右 Alt/Control/Shift 产出 `AltRight/ControlRight/ShiftRight` token，可单独作为 PTT 键；组合键中归一为普通修饰 token），Windows AltGr 的合成 Control 按键直接丢弃；F1-F24 与方向/标点 token 与 Rust `hotkey.rs` 解析器逐字对齐（含"纯修饰键只能是单个裸修饰键"预校验），未识别的键返回原始键以触发 `unsupported` 提示而非静默丢弃。
- `tauri-api.ts` - Tauri IPC 桥，挂载为 `window.electronAPI`（与 Electron preload 同形）；`setConfig` 返回 `SetConfigResult`（持久化配置 + `hotkeyRolledBack` 回滚标记），`setHotkeyCaptureActive` 通知 Rust 快捷键录制态守卫，`setOverlaySize` 把 overlay 窗口缩到 HUD 胶囊内容尺寸（`set_overlay_size`，Rust 侧钳制尺寸合法性并保持底部居中）。鼠标中键 PTT 设置（`mouse.middleClickPtt`）随 `setConfig` 流程下发、无独立桥方法；悬浮胶囊显隐由托盘菜单直接控制（`app.overlayVisible`），`onOverlayUpdate` 的 `OverlayStatus` 含常驻待机态 `idle`。
- `logger.ts` - 渲染进程日志初始化：转发 `console` 与未捕获错误到主进程日志。
- `stats.ts` - `computeStats()`：合并 Rust 侧独立累计统计（`StoredDictationStats`，`get_stats` 取数，删除/裁剪历史不影响）与历史实时聚合，计算今日/近 7 天/累计/活跃天数/峰值日；累计字段优先取 stored、窗口字段取历史，stored 缺失时降级为纯历史聚合；`countCharacters()` 按 **Unicode 码点** 剔除空白计数（`Array.from` 拆码点，emoji 计 1），与 Rust `stats.rs` `count_characters` 逐字对齐。`stats.test.ts` 为其回归测试，是 Rust 单测 `count_characters_matches_frontend_whitespace_rule` 的 JS 镜像（含 emoji 用例），两侧口径必须同步改。
- `useStatFormatters.ts` - 统一的数字 / 时长 / 峰值日格式化 Hook，供主页与统计组件共用。
- `useTheme.ts` - 明暗主题 Hook：持久化到 `localStorage`，同步切换 `<html>` 的 `.dark` 类，并在切换瞬间抑制全局过渡。
