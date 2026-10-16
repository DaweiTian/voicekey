# 图标与品牌位图源文件

- `icon.svg` —— 芷若 (Zhiruo) 应用图标主稿（兰草叶片 + 声波，橄榄绿渐变）。
  UI 内的单色描边版本在 `src/components/BrandMark.tsx`，造型需与本稿保持一致。
- `icon-1024.png` —— 由 `icon.svg` 渲染出的 1024×1024 位图，作为 `tauri icon` 的输入。
- `og.svg` / `og.png`（生成目标 `website/public/og.png`）—— 官网社交分享图源稿与生成脚本。

## 生成应用图标全套（含 `icon.icns`）

```bash
# 1) 渲染 SVG -> icon-1024.png（依赖：npm i @resvg/resvg-js png-to-ico，装在任意临时目录）
#    Node 从脚本自身目录向上解析依赖，若装在别处需设置 NODE_PATH 指向其 node_modules
NODE_PATH=/path/to/temp/node_modules node src-tauri/icons-src/render.cjs
# 2) 生成 src-tauri/icons/ 全套
npm run tauri -- icon src-tauri/icons-src/icon-1024.png
```

`render.cjs` 同时输出各处品牌位图：`public/zhiruo-icon-1024.png`（应用 favicon）、
`imgs/logo.png`（根 README）、`website/src/assets/logo.png` 与官网 favicon/apple-touch/android 图标。

## 生成官网分享图 og.png

```bash
# 依赖：npm i @resvg/resvg-js；需要系统中文字体（默认 Microsoft YaHei）
node src-tauri/icons-src/render-og.cjs .
```

`src-tauri/icons/` 为生成产物，不建议手工编辑；改动图标请改 `icon.svg` 后重新生成。
托盘图标取自 `app.default_window_icon()`，会自动跟随这里生成的图标。
NSIS 安装器/卸载器图标在 `tauri.conf.json` 的 `bundle.windows.nsis.installerIcon` /
`uninstallerIcon` 显式指向 `icons/icon.ico`（不设置时 makensis 回退 NSIS 自带默认图标，
`bundle.icon` 首项不会被安装器采用），重新生成图标后该 ico 会一并更新、配置无需改动。
