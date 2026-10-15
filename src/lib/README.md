# lib/

通用工具函数目录。

## 文件

- `utils.ts` - `cn()`：合并 `className`（`clsx` + `tailwind-merge`）。
- `hotkey-utils.ts` - 快捷键工具：规范化、构建、格式化与校验 Accelerator，并内置系统保留键规则；F1-F24 与方向/标点 token 与 Rust 解析器对齐，未识别的键返回原始键以触发 `unsupported` 提示而非静默丢弃。
- `logger.ts` - 渲染进程日志初始化：转发 `console` 与未捕获错误到主进程日志。
- `stats.ts` - `computeStats()`：从听写历史计算今日/近 7 天/累计/活跃天数/峰值日等概览统计。
- `useStatFormatters.ts` - 统一的数字 / 时长 / 峰值日格式化 Hook，供主页与统计组件共用。
- `useTheme.ts` - 明暗主题 Hook：持久化到 `localStorage`，同步切换 `<html>` 的 `.dark` 类，并在切换瞬间抑制全局过渡。
