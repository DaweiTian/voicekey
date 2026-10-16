# Changelog

All notable changes to this project will be documented in this file. See [standard-version](https://github.com/conventional-changelog/standard-version) for commit guidelines.

## [0.5.1](https://github.com/BuildWithAIs/voicekey/compare/v0.5.0...v0.5.1) (2026-10-16)

### Features

- **app:** 防止多实例运行，重复启动时聚焦已存在的窗口
- **overlay:** 待机胶囊支持拖拽，带边缘吸附与位置持久化；双击可复位
- **ui:** 新增版本徽章与更新日志弹窗（侧边栏 / 关于卡片）
- **ui:** 新增六种小米 SU7 风格主题色（默认橄榄绿）及设置「外观」分区
- **brand:** 应用更名为芷若 (Zhiruo)，口号「言若有声，芷若知心。」，启用新的兰叶 + 声波图标并更新官网视觉
- **session:** 新增 Rust 侧 5 分钟会话看门狗，作为渲染层计时器之外的第二道防线

### Bug Fixes

- **stats:** 首页统计与历史删除/清空完全解耦；今日/近 7 天窗口不再随删除收缩，旧统计文件无损升级
- **overlay:** 胶囊关闭后可由托盘重新打开（标志位与窗口可见性不同步，窗口缺失时从未重建）
- **ui:** 移除圆角矩形边框，仅显示胶囊主体
- **mouse:** 滚轮输入不再中断进行中的中键录音（松开时重新检测物理按键状态，容忍硬件抖动）
- **overlay:** 修复拖拽状态卡死（指针捕获 + 看门狗复位），双击复位不再误报空录音错误，拖拽调整大小后不会回弹到旧位置
- **config/tray:** 以进程级锁串行化跨线程配置写入，杜绝更新丢失；托盘菜单变更回派主线程处理

## [0.5.0] - 2026-10-16

本版本一并补记 0.4.0 未写入 changelog 的 Tauri 迁移主线，以及本轮桌面可靠性修复。

### Features

- 桌面后端从 Electron 迁移到 Tauri 2（Rust 主进程 + 系统 WebView），常驻内存显著下降
- **window:** 自定义标题栏与无装饰窗口（主窗口关闭系统装饰，由前端自绘标题栏与边框）
- **asr:** Rust 主进程重建流式转写链路（X-ASR，0.3.0 首次引入），并简化本地模型选择流程
- **overlay:** 悬浮胶囊常驻待机，鼠标按住点击即可说话，托盘可显隐（默认开启）
- **hotkey:** 支持鼠标中键触发录音（设置中可开关，默认关闭，仅 Windows）
- **stats:** 首页累计统计独立持久化（`voice-key-stats.json`），删除或清理历史不再清空累计数据，存量用户首次启动自动回填
- **translate:** 选中文字快捷键翻译落地（选区捕获 → LLM 翻译 → 原位回贴，剪贴板自动恢复）

### Bug Fixes

- **ui:** 消除 Windows 11 下 HUD 胶囊与主界面的系统蓝色焦点边框
- **hotkey:** 设置界面录入快捷键不再误触发录音（应用内聚焦或录制态自动屏蔽）
- **hotkey:** 修复右 Alt（AltGr）设置时报错并反复弹窗的问题（左右修饰键区分、非法值一次性提示并自动回滚）
- **window:** 主界面最小化到托盘 5 分钟后自动释放 WebView 进程，重开自动恢复（显著降低后台内存占用）
- **deps:** 补回缺失的 `axios` 依赖，修复测试套件加载失败

### Performance

- **overlay:** HUD 窗口随内容自适应缩放，消除胶囊四周的点击死区
- **stats:** 统计读-改-写全程持进程级锁，并发累加不再丢更新
- **session:** 代码审查加固——转写/润色期间拒绝重复启动、中键只停止自己启动的会话、剪贴板不可读时不覆写用户内容

## [0.3.0](https://github.com/BuildWithAIs/voicekey/compare/v0.2.1...v0.3.0) (2026-09-01)

### Features

- migrate streaming ASR to X-ASR ([bdfc35e](https://github.com/BuildWithAIs/voicekey/commit/bdfc35ea1c75dd179adff455f17e74285a75c629))
- **refine:** add TokenDance LLM provider with OAuth key authorization ([8fa9ac4](https://github.com/BuildWithAIs/voicekey/commit/8fa9ac436ff2facc58483ea2a600f04047aad745))
- **settings:** simplify ASR copy and warn on weak hosts ([1a8578a](https://github.com/BuildWithAIs/voicekey/commit/1a8578a6b8a84f63ef20d24c975b145c4d4632ee))
- **website:** deploy site on Cloudflare ([8271aec](https://github.com/BuildWithAIs/voicekey/commit/8271aeca46859cdb24806ec83d531a17080c5dd7))

### Bug Fixes

- **release:** generate portable checksums ([56b9269](https://github.com/BuildWithAIs/voicekey/commit/56b9269bf4ddae946807c099ed07a7b1c73df052))
- **settings:** rebind changed microphone device ids ([323c595](https://github.com/BuildWithAIs/voicekey/commit/323c59509736dcb0ede861d994d32fbe7e8fc7a3))

### [0.2.1](https://github.com/BuildWithAIs/voicekey/compare/v0.2.0...v0.2.1) (2026-08-27)

### Features

- **linux:** add Omarchy and Hyprland support ([edf2166](https://github.com/BuildWithAIs/voicekey/commit/edf2166df2bc6305e7b43a3b3cfa593499d35a27))

### Bug Fixes

- **linux:** make Wayland text injection reliable ([272991e](https://github.com/BuildWithAIs/voicekey/commit/272991ee43624b7b0aa4bba4b95cce70da4ab371))

## [0.2.0](https://github.com/BuildWithAIs/voicekey/compare/v0.1.22...v0.2.0) (2026-08-27)

### Features

- **asr:** add local streaming transcription ([be0a4fb](https://github.com/BuildWithAIs/voicekey/commit/be0a4fb3b8df0e77e37208733919893d9a9a74d6))
- **settings:** add local model management ([86ba0b0](https://github.com/BuildWithAIs/voicekey/commit/86ba0b0952253d57096c80648bb0a4869c2b3170))

### Bug Fixes

- **llm:** use low reasoning_effort for official DeepSeek ([0a06a82](https://github.com/BuildWithAIs/voicekey/commit/0a06a82ac51c11453cbc35e4158765fa5d0682b9))
- **settings:** restore refine toggles after LLM provider switch ([98fee38](https://github.com/BuildWithAIs/voicekey/commit/98fee38fcf9ae7238a3150e5c1ffa99de4dc24f8))

### [0.1.22](https://github.com/BuildWithAIs/voicekey/compare/v0.1.21...v0.1.22) (2026-08-11)

### Features

- add fallback source for local ASR downloads ([ac2f0f5](https://github.com/BuildWithAIs/voicekey/commit/ac2f0f52c7dd9a56b34025022687cc6cc7c52645))
- add fixed OpenRouter model presets ([50db94d](https://github.com/BuildWithAIs/voicekey/commit/50db94d21dbf52e458adad2ff4071db33bf3afdc))
- add official OpenAI provider ([7f6d045](https://github.com/BuildWithAIs/voicekey/commit/7f6d0451b3afafea232bcb2d6362c89e693db8b3))
- **asr:** use local-only speech recognition ([6da5673](https://github.com/BuildWithAIs/voicekey/commit/6da5673fd9c781ed348b6ff58450d789d6013acc))
- **settings:** improve API key and refinement controls ([ab6f853](https://github.com/BuildWithAIs/voicekey/commit/ab6f8538e427aadd674dd0ef7cc0148f9ad25b56))

### Bug Fixes

- preserve preload during dev builds ([2573625](https://github.com/BuildWithAIs/voicekey/commit/25736256c20139fb2630f500fbdd2f97f33bce15))
- prevent dev Electron auto-launch ([3474eec](https://github.com/BuildWithAIs/voicekey/commit/3474eecbdd22caacbdd0e7ac0ea2c795d9d22ac4))

### [0.1.21](https://github.com/BuildWithAIs/voicekey/compare/v0.1.20...v0.1.21) (2026-07-15)

### Bug Fixes

- avoid system credential store lockouts ([6d2b32e](https://github.com/BuildWithAIs/voicekey/commit/6d2b32e881a7088da0d97395097326a5b8d10930))

### [0.1.20](https://github.com/BuildWithAIs/voicekey/compare/v0.1.19...v0.1.20) (2026-07-11)

### Features

- add provider-aware reasoning config ([0a0ab32](https://github.com/BuildWithAIs/voicekey/commit/0a0ab321c23ac69f55f1e5d24f05b3de41382be8))

### Bug Fixes

- release idle local ASR worker ([15afaa0](https://github.com/BuildWithAIs/voicekey/commit/15afaa0638dcc9427e1b3a3ef8a96a87f2d19bf8))

### [0.1.19](https://github.com/BuildWithAIs/voicekey/compare/v0.1.18...v0.1.19) (2026-07-07)

### Features

- add microphone input selection ([32ccc45](https://github.com/BuildWithAIs/voicekey/commit/32ccc457f4b4800510c5c3a739431a9cbe06b08d))

### [0.1.18](https://github.com/BuildWithAIs/voicekey/compare/v0.1.17...v0.1.18) (2026-07-07)

### Bug Fixes

- add xrandr headers for uiohook ci builds ([6e6a5e5](https://github.com/BuildWithAIs/voicekey/commit/6e6a5e5b31bde3ce1e801ba4001cc3a10e0ea623))
- harden recording pipeline and builds ([ce6f379](https://github.com/BuildWithAIs/voicekey/commit/ce6f379d5a03eeafa6c4cc46627e0cededb72cf3))
- install linux deps for uiohook ci builds ([f1eb7e7](https://github.com/BuildWithAIs/voicekey/commit/f1eb7e701756d4c69ffad262eb29d7392c321d06))
- stabilize local asr and settings window startup ([d6edf9d](https://github.com/BuildWithAIs/voicekey/commit/d6edf9dc95cff3892dac83fb5bc3d967d81f7a61))

### [0.1.17](https://github.com/BuildWithAIs/voicekey/compare/v0.1.16...v0.1.17) (2026-06-27)

### Features

- add local SenseVoice ASR support ([27178cb](https://github.com/BuildWithAIs/voicekey/commit/27178cb87c176828fefd2c856637eb262178d53b))
- **refine:** improve polish and translation prompts ([3d6258d](https://github.com/BuildWithAIs/voicekey/commit/3d6258d1d1af5eb4f9aa33c6dda648fe146c3822))

### Bug Fixes

- harden local ASR runtime fallback ([3d4d874](https://github.com/BuildWithAIs/voicekey/commit/3d4d8741136681b909267bfcae7b2efcc42e6494))

### [0.1.16](https://github.com/BuildWithAIs/voicekey/compare/v0.1.15...v0.1.16) (2026-06-22)

### Features

- **settings:** consolidate refine & translation, unify target language ([99b1636](https://github.com/BuildWithAIs/voicekey/commit/99b1636c1cbf66cffd25a19b41cc513974991e50))
- **ui:** redesign home, settings, and history pages ([9ef8fc4](https://github.com/BuildWithAIs/voicekey/commit/9ef8fc44664dfd7d2802289ba41d1f210fcd9c74))

### Bug Fixes

- **hotkey:** prevent bare-modifier PTT from misfiring on combo hotkeys ([90483c7](https://github.com/BuildWithAIs/voicekey/commit/90483c768ade582aaf9408ecca84db47a33e1d10))

### [0.1.15](https://github.com/BuildWithAIs/voicekey/compare/v0.1.14...v0.1.15) (2026-06-03)

### Features

- **translation:** enhance translation system prompt for clarity and functionality ([6c2b6b0](https://github.com/BuildWithAIs/voicekey/commit/6c2b6b002b8792fd0e3a16e6d44488e902fc8ab3))

### [0.1.14](https://github.com/BuildWithAIs/voicekey/compare/v0.1.13...v0.1.14) (2026-06-02)

### Features

- **refine:** enhance transcript handling for empty or minimal inputs ([5b8fa68](https://github.com/BuildWithAIs/voicekey/commit/5b8fa687a5272b6230b1f78a77a3c2010780485e))
- **stats:** add today's session statistics and peak day tracking ([7b1f555](https://github.com/BuildWithAIs/voicekey/commit/7b1f555a517fa1f47e7eb45ee11faef0a94f854e))

### Bug Fixes

- **translation:** harden selected text replacement ([0622874](https://github.com/BuildWithAIs/voicekey/commit/06228744c35dc4f8918be9379e002e14c29d5917))

### [0.1.13](https://github.com/BuildWithAIs/voicekey/compare/v0.1.12...v0.1.13) (2026-04-20)

### Features

- **hud:** split processing into transcribe and refine stages ([810e512](https://github.com/BuildWithAIs/voicekey/commit/810e512c6775f7ddfe54a139dbee9d134bfb05ac))
- **refine:** add remote glossary refresh ([8b77a1b](https://github.com/BuildWithAIs/voicekey/commit/8b77a1b41d9496b51cf3f58d50cbec0d942942a5))
- **ui:** distinguish ASR and LLM refinement stages in HUD ([dd0b027](https://github.com/BuildWithAIs/voicekey/commit/dd0b027d15bce5074be68353f679d212594f418d))

### [0.1.12](https://github.com/BuildWithAIs/voicekey/compare/v0.1.11...v0.1.12) (2026-03-30)

### Features

- add optional English output mode for transcript refine ([bc658dd](https://github.com/BuildWithAIs/voicekey/commit/bc658ddc64ff337dd6e3a480eae06646360f2e4f))
- **refine:** improve structured formatting and multiline injection ([466f219](https://github.com/BuildWithAIs/voicekey/commit/466f219d9046264caf6a5f3cfb5686d038782907))
- simplify refine base url config ([881ae4e](https://github.com/BuildWithAIs/voicekey/commit/881ae4e89b50c35d043e3e050e54de0f3f2cfa0c))

### [0.1.11](https://github.com/BuildWithAIs/voicekey/compare/v0.1.10...v0.1.11) (2026-03-29)

### Features

- **audio:** support 3-minute chunked glm asr sessions ([31df453](https://github.com/BuildWithAIs/voicekey/commit/31df4537e22090b7e81310eb033609811fc07d04))
- improve refine prompt guidance ([633385a](https://github.com/BuildWithAIs/voicekey/commit/633385a0df49fa6ce62deb18fa72fcde055afec8))

### [0.1.10](https://github.com/BuildWithAIs/voicekey/compare/v0.1.9...v0.1.10) (2026-03-29)

### Features

- **settings:** improve autosave status feedback ([d691c9f](https://github.com/BuildWithAIs/voicekey/commit/d691c9f84ba8a966f8b2966f4d17ae8bbe24c55a))

### Bug Fixes

- keep auto launch startup in background ([bdd677e](https://github.com/BuildWithAIs/voicekey/commit/bdd677e4243dfed15d37a3bcacba6cc31e16dea8))

### [0.1.9](https://github.com/BuildWithAIs/voicekey/compare/v0.1.8...v0.1.9) (2026-03-29)

### Features

- **chart:** enhance localization for chart labels and range selector ([0137700](https://github.com/BuildWithAIs/voicekey/commit/0137700f2e02ecdf0a2744a0c41ad45552734e07))
- refactor text refinement flow ([b65bae1](https://github.com/BuildWithAIs/voicekey/commit/b65bae1f8f61964353e05a583303fd27d2b8cc31))

### Bug Fixes

- **security:** defer API key encryption migration to after app.ready ([59c173f](https://github.com/BuildWithAIs/voicekey/commit/59c173f47be0a21476eb76f2230f40e7799a5ffd))
- **security:** encrypt API keys with safeStorage and remove keystroke logging ([1655d6f](https://github.com/BuildWithAIs/voicekey/commit/1655d6f1ad57ef00e270138e1f9f6e3b2da1de24))

### [0.1.8](https://github.com/BuildWithAIs/voicekey/compare/v0.1.7...v0.1.8) (2026-03-12)

### Features

- **audio:** add low-volume enhancement mode ([2aae04d](https://github.com/BuildWithAIs/voicekey/commit/2aae04d07e266ba3b1dc00b21499164f3b51ea97))
- **llm:** add glm refine mvp with asr reuse ([6b094cd](https://github.com/BuildWithAIs/voicekey/commit/6b094cd03774fcdb6fbb96bc0a0aab86f1cf7d0a))
- **website:** add automatic version detection from GitHub releases ([b6c756b](https://github.com/BuildWithAIs/voicekey/commit/b6c756b2aa971cf3b60b8a94364a1cdf3b536427))

### Bug Fixes

- **website:** add localStorage caching to reduce GitHub API calls ([41cdf6d](https://github.com/BuildWithAIs/voicekey/commit/41cdf6d43d914dcaf874db867be4ebec18bfc4f2))

### [0.1.7](https://github.com/BuildWithAIs/voicekey/compare/v0.1.6...v0.1.7) (2026-01-26)

### Features

- add official website project structure ([6aa2678](https://github.com/BuildWithAIs/voicekey/commit/6aa26781f06db42e57381044faf2f70fb515ca62))
- **i18n:** auto-sync system locale when language setting is 'system' ([4618ead](https://github.com/BuildWithAIs/voicekey/commit/4618eadad3f247cc4db8890bf507b03da009714e))
- **i18n:** implement real-time language sync across all windows ([91a2b5b](https://github.com/BuildWithAIs/voicekey/commit/91a2b5b3fa4dd4b98bead22103bac0d626cbb1ff))
- **website:** add bilingual support and deploy workflow ([f4daca9](https://github.com/BuildWithAIs/voicekey/commit/f4daca91f82bded24ef0a4e12ce40c2543c4d5e2))
- **website:** add canonical URLs, hreflang, and 404 page ([d5f4773](https://github.com/BuildWithAIs/voicekey/commit/d5f47735c0b7bb3dc6ec6900ebb0b1be62746c73))
- **website:** add light/dark theme toggle ([3481d18](https://github.com/BuildWithAIs/voicekey/commit/3481d18978085fe413be91c7f1a6738775b7c28e))
- **website:** add sitemap generation and improve favicon setup ([bc0401b](https://github.com/BuildWithAIs/voicekey/commit/bc0401b32b9b94423cf377132038c8f3a79a5384))
- **website:** redesign with geek-style aesthetic ([1a4fe84](https://github.com/BuildWithAIs/voicekey/commit/1a4fe8409a3ce8052ebe8c8648acd36df06966ba))

### Bug Fixes

- **i18n:** improve fallback language resolution in renderer ([0ee7adc](https://github.com/BuildWithAIs/voicekey/commit/0ee7adc1f0deab9703a053b1ecd8bb66292233ae))
- **website:** add localStorage error handling for private browsing ([4f3d72a](https://github.com/BuildWithAIs/voicekey/commit/4f3d72a32252cdb0034f256ce403cd704bb9d60c))
- **website:** correct site URL and improve deployment workflow ([795ed1a](https://github.com/BuildWithAIs/voicekey/commit/795ed1a5143b61cc2d1be45e271a36b3a2aae7ee))

### [0.1.6](https://github.com/BuildWithAIs/voicekey/compare/v0.1.5...v0.1.6) (2026-01-25)

### Features

- **logging:** add persistent logging system with retention and UI ([05314f0](https://github.com/BuildWithAIs/voicekey/commit/05314f053f700a932650576db62d81649158ac0d))
- **session:** add cancellation checks in audio processing pipeline ([5133a8a](https://github.com/BuildWithAIs/voicekey/commit/5133a8a840126d6bbd0e6757ab4a0707b3ba88bf))
- **session:** implement cancel session functionality ([037f178](https://github.com/BuildWithAIs/voicekey/commit/037f178cb247cda7a0285f366359ed38fe789397))

### Bug Fixes

- **logging:** correct archiveLog callback to use LogFile.path property ([47ba0af](https://github.com/BuildWithAIs/voicekey/commit/47ba0afc599d707c13c6e19304365e9a8efc600a))

### [0.1.5](https://github.com/BuildWithAIs/voicekey/compare/v0.1.4...v0.1.5) (2026-01-22)

### [0.1.4](https://github.com/BuildWithAIs/voicekey/compare/v0.1.3...v0.1.4) (2026-01-22)

### [0.1.3](https://github.com/BuildWithAIs/voicekey/compare/v0.0.6...v0.1.3) (2026-01-22)

### [0.0.6](https://github.com/BuildWithAIs/voicekey/compare/v0.0.5...v0.0.6) (2026-01-22)

### 0.0.5 (2026-01-22)

### Features

- init commit ([4a9f11a](https://github.com/BuildWithAIs/voicekey/commit/4a9f11a903fb65682909cd18f2f4467a43fa07db))
- **update-check:** add startup update check ([d240577](https://github.com/BuildWithAIs/voicekey/commit/d2405771e3947ea6771ca4f5ea9c784f9d013d32))

## [0.1.2] - 2026-01-21

- Initial public release.

[0.1.2]: https://github.com/BuildWithAIs/voicekey/releases/tag/v0.1.2
