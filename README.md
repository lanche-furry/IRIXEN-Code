<p align="center">
  <img src="public/assets/irixen-wordmark.png" alt="IRIXEN Code" width="420" />
</p>

<p align="center">
  <b>IRIXEN Code</b> — 轻量桌面 IDE + 原生 code agent<br />
  Tauri 2 · WebView2 · Rust · 无前端框架
</p>

<p align="center">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-blue" /></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.77%2B-orange" />
  <img alt="Tauri" src="https://img.shields.io/badge/tauri-2.x-24C8DB" />
  <img alt="Size" src="https://img.shields.io/badge/installer-small-brightgreen" />
</p>

<!-- ===================== LANGUAGE TOGGLE ===================== -->
<!-- Works in any Markdown viewer that allows raw HTML + <style> (VS Code preview,
     Typora, Obsidian, exported HTML). On GitHub the <style> is sanitized, so both
     language blocks are shown stacked — content is identical either way. -->
<input type="radio" name="irixen-lang" id="irixen-lang-zh" class="irixen-lang-radio" checked>
<input type="radio" name="irixen-lang" id="irixen-lang-en" class="irixen-lang-radio">

<p align="center" class="irixen-langswitch">
  <label for="irixen-lang-en" class="irixen-lang-en-opt">English</label>
  <span class="irixen-lang-sep">|</span>
  <label for="irixen-lang-zh" class="irixen-lang-zh-opt">中文</label>
</p>

<style>
.irixen-lang-radio { position: absolute; opacity: 0; width: 0; height: 0; pointer-events: none; }
.irixen-langswitch { font: 600 14px/1 -apple-system, "Segoe UI", system-ui, sans-serif; margin: 4px 0 12px; user-select: none; }
.irixen-langswitch label { cursor: pointer; padding: 4px 10px; border-radius: 6px; color: #6b7280; }
.irixen-lang-sep { color: #c0c4cc; padding: 0 2px; }
.irixen-lang-zh, .irixen-lang-en { display: none; }
#irixen-lang-zh:checked ~ .irixen-lang-zh { display: block; }
#irixen-lang-en:checked ~ .irixen-lang-en { display: block; }
#irixen-lang-zh:checked ~ .irixen-langswitch .irixen-lang-zh-opt { background: #eef2ff; color: #2f54eb; }
#irixen-lang-en:checked ~ .irixen-langswitch .irixen-lang-en-opt { background: #eef2ff; color: #2f54eb; }
</style>

<!-- ===================== 中文 ===================== -->
<div class="irixen-lang-zh">

IRIXEN Code 由两部分组成：

| 组件 | 说明 |
| --- | --- |
| **IRIXEN Agent** | Rust 写的 agent 核心：三协议统一抽象、原生工具、MCP 客户端、技能系统、三级上下文压缩 |
| **IRIXEN Code** | 桌面外壳：Tauri 2 + 系统 WebView2，无边框窗口、无启动画面（创建即显示）、左侧导航栏 + 对话主区 + 右侧快捷栏；打开文件时编辑器**覆盖**对话区而不是新开一列 |
| **Web2API** | 内置桥接服务：把网页端 AI（Claude / ChatGPT / Gemini Web，走官网浏览器会话）包装成标准的 OpenAI / Anthropic 兼容接口，`OpenAI SDK`、Cursor 这类现成客户端可直接接入 |

设计目标：**输出高质量、低 token 消耗、不降智**。所有取舍都围绕这三条展开。

## 功能

**多厂商 API**

- 一套 `LlmProvider` trait 覆盖三种协议：OpenAI `chat/completions`、Anthropic `messages`、Gemini `streamGenerateContent`
- 内置 OpenAI / Anthropic / Gemini / DeepSeek / Moonshot(Kimi) / 智谱 GLM / 通义千问 / SiliconFlow / OpenRouter 预设，可任意新增
- **自动拉取模型列表**（`/models`、`/v1/models`、Anthropic 分页接口），结果磁盘缓存 10 分钟
- **思考强度可调**：输入框上方的三格档位条（自动 / 关闭 / 低 / 中 / 高）落到 `reasoning_effort`（OpenAI 兼容，含 DeepSeek）、`thinking.budget_tokens`（Claude）、`thinkingConfig.thinkingBudget`（Gemini）；厂商不认该字段时自动去掉重发一次，不会卡住一个回合。默认档位存进配置，新会话沿用
- `extra_body` / `extra_headers` 处理各家怪癖（如 Qwen 的 `enable_thinking`）

**Agent 能力**

- 原生工具 7 个：`read_file` `write_file` `replace_in_file` `list_dir` `search_files` `run_command` `git`
- **路径硬约束**：任何文件工具都无法越出工作区（`resolve()` 统一校验）
- 工具轮次上限、随时停止、逐事件流式回传（文本 / 思考 / 工具调用 / 用量）
- **MCP 客户端**：JSON-RPC over stdio，握手 `2024-11-05`，MCP 工具与原生工具同池调度，Windows 下自动用 shell 包装 npx / uvx 垫片

**三级上下文压缩**（省 token 的关键）

1. **工具结果折叠** —— 超过阈值的历史工具输出压成一行摘要（保留路径与结论）
2. **滑动窗口 + 任务锚点** —— 保留最近 K 条消息，更早的合并为锚点（目标 / 决策 / 文件）
3. **LLM 结构化摘要** —— 仍超预算时调用模型生成 `Goal / Decisions / Files / State / Pending` 摘要，失败自动回退启发式摘要；摘要阶段**不持锁**，不阻塞 UI

**IDE 外壳**

布局不是 VS Code 那种「三列常驻」：**左侧导航栏 + 中间对话区**，代码文件是**覆盖**在对话区上打开的，关掉标签就回到对话，视觉焦点永远只有一个。

- 无边框窗口、自绘标题栏与窗口按钮（没有 agent 徽标，标题栏只留工作区与窗口操作）
- **左侧导航列**（固定 56px，图标 + 短标签，位置与顺序不变）：文件 / 搜索 / Git / 技能 / 统计 / 审查。旁边是当前视图的内容栏，顶部显示视图名与对应工具按钮；资源管理器有 **新建文件 / 新建文件夹 / 全部折叠** 三个快捷按钮（右键菜单同样有）
- **右侧 40px 快捷栏**：对话、Web2API、扩展（角标 = 正在运行的宿主数）、文件（有打开的标签时才出现）、终端、设置
- **对话区是可被覆盖的画布**：点开文件 → 编辑器连标签栏一起 `inset: 0` 盖住对话区；标签栏右侧固定一个「回到对话」按钮，常驻不随标签滚动
- 启动：**没有 splash 窗口、没有入场动画**。窗口以 `visible: true` 创建（底色 = 主题 `--bg-0`），进程起来就能看到，UI 首帧直接落在上面；`main_loaded` 只用来回收焦点并记录挂载耗时
- CodeMirror 6 编辑器：多标签、语法高亮、脏标记、`Ctrl+S` 保存、从搜索结果跳转到行
- 文件树（懒加载）、全工作区搜索（遵循 `ignore` 规则）、Git 面板（暂存 / 提交 / 推送 / 拉取 / 分支 / 差异）
- **审查面板**：一眼扫完工作区改动（`git status` + diff 统计），一键把「这些文件改了什么、有没有风险」连同当前 diff 交给 Agent 审。行级差异用红绿着色，和状态栏、快捷栏角标联动
- 工作区统计（文件数 / 代码行 / 体积 / 按扩展名分布 / 最大文件）与 API 用量（调用次数、输入输出、推理、缓存命中，按模型与会话分组）
- 主题：深色 / 浅色 / **自定义**（任意 CSS 变量覆盖，自动派生 `--accent-soft` 与对比色）
- 语言：简体中文 / 繁體中文 / English / 日本語 / 한국어 / Русский / Español / Français（8 种，约 280 键；键集合用 TS 类型锁死，漏翻是**编译错误**而不是运行期回退；切换即时生效，不需要重启）

**扩展（VSC / DSH 兼容，带 Node 扩展宿主 + 内置商店）**

不是"模拟"VS Code 扩展：声明式的部分直接接进 IDE，需要代码的部分真的跑在一个 `node` 子进程里。

| 贡献点 | 状态 | 说明 |
| --- | --- | --- |
| `contributes.themes` | ✅ | 把 VS Code 主题 JSON 的颜色键映射到本项目的 CSS 变量（`editor.background` → `--bg-0` 等），选中即预览 |
| `contributes.snippets` | ✅ | 解析代码片段 JSON，接进 CodeMirror 6 的自动补全（`${1:label}` / `${1\|a,b\|}` 占位符会展开） |
| `contributes.languages` | ✅ | 扩展名 → 语言 id，决定高亮与片段作用域 |
| `contributes.commands` + `extension.js` | ✅ | 每个扩展一个 Node 子进程，注入 `vscode` API 垫片；命令进命令面板（`Ctrl+Shift+P`）与扩展卡片，可执行 |
| 语言特性 provider（补全 / 悬停 / 诊断） | ⏳ 已注册未驱动 | 垫片里有 `languages.register*Provider`，但还没有把编辑器的请求接过去 |

**Node 扩展宿主**跑的是真扩展：`vscode.commands`（注册 + 执行 + 跨扩展转发）、`window.showInformationMessage / showQuickPick / showInputBox`（真弹窗，Promise 真的 await）、`createStatusBarItem`（底部状态栏真出现，可点）、`createOutputChannel`（进扩展卡片的输出面板）、`workspace.fs`（读写 / 列目录 / 新建 / 删除 / 重命名）、`workspace.rootPath` 与 `workspaceFolders`、`window.activeTextEditor`（文档内容、选区、`edit()` 增删改会打到真的编辑器）、`context.globalState / workspaceState`（落盘到扩展目录的 `.state/`）、`env.openExternal`、`Uri / Position / Range / Selection / EventEmitter / Disposable` 等基础类型。

没有 Node 时会提示并**一键下载便携版**（约 30 MB，只解出 `node.exe` 存到 `<config>/runtime/node/`），也可以手动指定已有的 `node`。每个扩展的宿主可以单独启停，日志与命令都在扩展卡片里。

**插件商店**是内置的，不需要开浏览器：搜索框 + 分类（主题 / 语言 / 片段 / 格式化 / 检查 / 其他）+ 排序（下载量 / 评分 / 更新时间），点开即见 README、评分、许可证、全部版本（可装指定版本）、仓库链接。数据来自 **Open VSX**——VS Code Marketplace 的开源镜像。

安装来源四种，都在扩展面板里一个输入框粘贴即可：

- **在线**：`publisher.name`（如 `sdras.night-owl`）→ 直接查 **Open VSX**（VS Code Marketplace 的开源镜像）并下载 `.vsix`
- **本地**：选一个 `.vsix` 文件
- **直链**：任何 `https://…/x.vsix`
- **Git**：`github:owner/repo`、`owner/repo`、`git@…`

`.vsix` 就是一个 ZIP，所以自带了一个 ZIP 读取器（中央目录 + `flate2` 解压），不依赖外部解压程序；解包时剥掉 `extension/` 前缀、拒绝绝对路径与 `..`，并限制文件数（2 万）与总大小（512 MB）。

DSH 风格的仓库（带 `SKILL.md` 的技能包）会被识别为 `dsh` 类型，安装时自动递归查找 `SKILL.md` 并导入技能系统——扩展装完即生效，不用二次操作。

> 为什么 GitHub 仓库走 `codeload.github.com/{owner}/{repo}/zip/HEAD` 而不是 `git clone`：Windows 上 `git` 用自己的 OpenSSL CA bundle，在会做 TLS 拦截的环境里会报 `certificate signer not trusted`；而应用内的 `reqwest` 走系统 schannel，同一条网络是通的。非 GitHub 的仓库才回退到 `git clone`（并加 `-c http.sslBackend=schannel`）。

**Web2API 桥接（网页端反向代理）**

把「网页里的 AI」变成「一个 endpoint」，客户端零改动。注意：这里代理的是**网页端 AI**，不是应用里配置的 API 供应商——`设置 → 供应商` 里的 DeepSeek 等 key 是给 agent 用的，只有调试桥接本身时才临时切到「应用内供应商（仅调试 agent）」。

- 默认按**模型名路由**：`claude*` → Claude Web，`gpt-*` / `o1` / `o3` → ChatGPT Web，`gemini*` → Gemini Web；路由表可在面板里增删改，未命中走默认网页源
- 每个网页源有两种请求方式：`官网 + 浏览器会话`（默认，在 `websession.rs` 打开的常驻 WebView 里驱动官网自己的输入框发消息、再读取页面回答，cookie / UA / origin / PoW / sentinel 令牌全部由页面提供，无需复制凭证）与 `直连模板`（旧方式，手填 sessionKey 后用 `reqwest` 直连）。面板上「打开官网」用于登录，窗口关闭只隐藏不销毁、登录态长期保留；「测试连接」会真实发一条最小请求并列出每个源的 HTTP 状态
- 每个网页源独立保存网页模型、官网地址、端点。端点、鉴权头、请求体、文本字段路径、事件过滤、是否累积全文都在「高级」里可改——**私有接口变动时不必重新构建**
- 实测：直连私有端点在无浏览器会话时分别返回 `403`（claude.ai）、`401`（chatgpt.com）、`405`（gemini.google.com）；改走官网浏览器会话后请求能到达 ChatGPT 私有 API 并返回官网自身的响应（未登录时为 403 反爬提示）。部分站点的私有接口还有页面生成的 PoW / sentinel 令牌，登录后可能仍需针对性适配
- 双协议出口：`POST /v1/chat/completions`（OpenAI）与 `POST /v1/messages`（Anthropic），**都支持 `stream: true`**，SSE 分帧（chunked）自己写，不引 web 框架
- `/v1/models` 列出各网页源模型及 `gpt-4o` / `claude-sonnet-4-5` / `gemini-2.5-pro` 别名；响应的 `model` 回显客户端请求的名字
- 本地鉴权：`Authorization: Bearer sk-irixen-…` 或 `x-api-key`；默认只监听 `127.0.0.1`，不对外
- 面板内可改端口、开自动启动、看实时请求日志（**命中的网页源** / 路径 / 状态码 / 耗时 / 错误），启动失败会把原因留在面板上
- **思考内容不丢**：推理模型的 `reasoning` 在 OpenAI 侧映射到 `reasoning_content`，在 Anthropic 侧并进正文；正文为空时用思考文本兜底
- 纯文本：默认只把**最新一条用户消息**发给官网（页面本身是有状态会话，人设与历史由官网自己维护，不重复注入 IRIXEN 的系统提示词）；网页源的「发送内容」可切回 `完整提示词 + 历史`。`直连模板` / `api` 驱动这类无状态通道始终需要完整 prompt。tools / 图片 / 附件不支持；网页端是非官方接口，可能随时失效，凭证只保存在本机配置里

用法：面板里点各网页源的 **打开官网** 登录（或切「直连模板」填会话凭证）→ **保存并应用** → **启动** → 客户端把 OpenAI `base_url` 指向 `http://127.0.0.1:8791/v1`（Anthropic SDK 填 `http://127.0.0.1:8791`），模型名按路由表填即可。`/v1/models` 免鉴权，`/v1/chat/completions` 与 `/v1/messages` 需要面板里的本地密钥。

**技能（Skills）**

- `SKILL.md` + frontmatter（`name` / `description` / `when`），`when` 命中用户输入即自动注入系统提示词
- 5 个自研内置技能，全部是"省 token、少绕路"取向：`token-saver` `diff-surgeon` `repo-scout` `commit-craft` `code-reviewer`
- 交互式安装：本地目录、`SKILL.md` 的 http(s) 链接、Git 仓库
- 只注入"匹配到的技能"，不把全部技能塞进上下文——这本身就是省 token 的设计

## 安装

有两个发布资产，安装程序为推荐方式：

- **Windows 安装程序（推荐）**：在 [Releases](https://github.com/lanche-furry/IRIXEN-Code/releases) 下载 `IRIXEN Code_*_x64-setup.exe`，双击运行即可安装，会在开始菜单与桌面创建快捷方式，并自动补齐 WebView2 Runtime（Win10 需联网；Win11 自带）。安装程序内置 **English / 简体中文** 语言选择界面。
- **便携版（Portable）**：`irixen-code.exe` 可直接运行，无需安装。

> 需要 WebView2 Runtime：Win11 自带；Win10 由安装程序联网补齐。

## 快速开始

```bash
npm install
npm run app:dev      # 开发（自动拉起 vite + tauri）
npm run app:release  # 出 release 独立 exe（--no-bundle）
npm run app:build    # 出 NSIS 安装包
```

首次启动后：`设置 → 供应商`，填入 Base URL 与 API Key，点**测试连接**，模型列表会自动出现；然后在对话框上方选模型与思考强度。

> 构建环境：Rust 1.77+（GNU target）、Node 18+、Windows 10/11 自带 WebView2。
> 本仓库采用**便携工具链**方案，Rust 与构建工具全部位于 `.tools/`，详见 [docs/BUILD.md](docs/BUILD.md)。

## 目录结构

```
src/                    前端（TypeScript，零框架）
  core/                 dom / i18n（8 语言）/ api / state
  core/locales/         zh-TW / es-ES / fr-FR（其余内联在 i18n.ts）
  ui/                   shell（布局与快捷栏）· chat · editor · explorer · search
                        git · skills · stats · review · bridge · extensions · settings
  styles/               base.css + app.css（全部设计令牌）
src-tauri/              Rust 后端
  src/agent/            provider（三协议）/ models / context / tools / meter / types
  src/bridge.rs         Web2API：手写 HTTP/1.1 + SSE，OpenAI / Anthropic 双协议出口
  src/extensions.rs     扩展子系统：VSIX 解包（自带 ZIP 读取器）/ Open VSX 商店 / 主题·片段·技能
  src/exthost/          Node 扩展宿主：进程管理 + NDJSON RPC + bootstrap.js（vscode API 垫片）
  src/commands.rs       IPC 命令面
  src/mcp.rs            MCP stdio 客户端
  src/skills.rs         技能注册表（编译期内置 + 用户安装）
  src/skills/builtin/   5 个内置 SKILL.md
public/assets/         字标等静态资源
assets/icon.png        应用图标母版（1254×1254 透明 PNG，`npm run icons` 生成全套尺寸）
scripts/                图标生成 / 便携工具链启动器
docs/                   架构、构建、技能开发
```

## 文档

- [架构设计](docs/ARCHITECTURE.md)
- [构建与工具链](docs/BUILD.md)
- [技能开发](docs/SKILLS.md)
- [贡献指南](CONTRIBUTING.md)

## 许可

[Apache License 2.0](LICENSE) © lanche-furry

</div>

<!-- ===================== English ===================== -->
<div class="irixen-lang-en">

IRIXEN Code has three parts:

| Component | Description |
| --- | --- |
| **IRIXEN Agent** | The Rust agent core: one abstraction over three protocols, native tools, an MCP client, a skill system, and a three-stage context compressor |
| **IRIXEN Code** | The desktop shell: Tauri 2 + the system WebView2, a borderless window with no splash (visible the moment it is created), a left nav rail + a center conversation area + a right quick bar; opening a file **overlays** the editor on the conversation instead of opening a third column |
| **Web2API** | A built-in bridge that wraps browser-based AI (Claude / ChatGPT / Gemini Web, driven through the vendor's own browser session) as standard OpenAI / Anthropic compatible endpoints, so off-the-shelf clients like the `OpenAI SDK` or Cursor can connect directly |

Design goal: **high-quality output, low token cost, no loss of intelligence**. Every trade-off serves those three principles.

## Features

**Multi-vendor API**

- One `LlmProvider` trait covers three protocols: OpenAI `chat/completions`, Anthropic `messages`, Gemini `streamGenerateContent`
- Built-in presets for OpenAI / Anthropic / Gemini / DeepSeek / Moonshot (Kimi) / Zhipu GLM / Qwen / SiliconFlow / OpenRouter; add any vendor easily
- **Automatic model discovery** (`/models`, `/v1/models`, Anthropic's paginated endpoint), with results cached on disk for 10 minutes
- **Adjustable thinking effort**: a 3-step knob above the input box (auto / off / low / medium / high) maps to `reasoning_effort` (OpenAI-compatible, incl. DeepSeek), `thinking.budget_tokens` (Claude), and `thinkingConfig.thinkingBudget` (Gemini); if a vendor rejects the field it is stripped and retried once, so a turn never gets stuck. The default level is saved to config and reused for new sessions
- `extra_body` / `extra_headers` handle per-vendor quirks (such as Qwen's `enable_thinking`)

**Agent capabilities**

- Seven native tools: `read_file` `write_file` `replace_in_file` `list_dir` `search_files` `run_command` `git`
- **Hard path sandbox**: no file tool can escape the workspace (`resolve()` validates uniformly)
- Per-tool turn caps, stop-at-any-time, and per-event streaming (text / thinking / tool call / usage)
- **MCP client**: JSON-RPC over stdio, handshake `2024-11-05`; MCP tools are scheduled in the same pool as native tools, and on Windows `npx` / `uvx` shims are wrapped in a shell automatically

**Three-stage context compression** (the key to saving tokens)

1. **Tool-result elision** — historical tool output above a threshold is collapsed into a one-line summary (paths and conclusions preserved)
2. **Sliding window + task anchors** — keep the most recent K messages; older ones are merged into anchors (goal / decision / file)
3. **LLM structured digest** — when still over budget, the model produces a `Goal / Decisions / Files / State / Pending` summary; on failure it falls back to a heuristic summary. The digest phase takes **no lock** and never blocks the UI

**IDE shell**

The layout is deliberately **not** the VS Code "three permanent columns": a **left nav rail + a center conversation area**, with code files **overlaid** on the conversation. Close the tab and you are back to the conversation — the visual focus is always singular.

- Borderless window, custom-drawn title bar and window buttons (no agent badge; the title bar keeps only the workspace and window controls)
- **Left nav column** (fixed 56px, icon + short label, fixed position and order): Files / Search / Git / Skills / Stats / Review. Beside it is the content column for the current view, with the view name and its tool buttons on top; the explorer has three quick buttons — **New File / New Folder / Collapse All** (also in the right-click menu)
- **Right 40px quick bar**: Chat, Web2API, Extensions (badge = number of running hosts), Files (only when a tab is open), Terminal, Settings
- **The conversation area is an overlayable canvas**: open a file → the editor plus its tab bar `inset: 0` covers the conversation; a pinned "back to conversation" button on the right of the tab bar stays put and never scrolls away
- Startup: **no splash window, no intro animation**. The window is created with `visible: true` (background = theme `--bg-0`), so it is visible as soon as the process starts and the first UI frame lands directly on it; `main_loaded` only reclaims focus and records mount time
- CodeMirror 6 editor: multi-tab, syntax highlighting, dirty markers, `Ctrl+S` save, jump to line from search results
- Lazy file tree, full-workspace search (respects `ignore` rules), Git panel (stage / commit / push / pull / branch / diff)
- **Review panel**: scan all workspace changes at a glance (`git status` + diff stats), then hand "what changed and is it risky" plus the current diff to the agent in one click. Line-level diffs are colored red/green and linked with the status bar and quick-bar badges
- Workspace stats (file count / code lines / size / per-extension distribution / largest file) and API usage (calls, input/output, reasoning, cache hits, grouped by model and session)
- Themes: Dark / Light / **Custom** (override any CSS variable; `--accent-soft` and the contrast color are derived automatically)
- Languages: Simplified Chinese / Traditional Chinese / English / Japanese / Korean / Russian / Spanish / French (8 locales, ~280 keys; the key set is locked by a TS type, so a missing translation is a **compile error**, not a runtime fallback; switching is instant and needs no restart)

**Extensions (VSC / DSH compatible, with a Node extension host + a built-in store)**

This is not "simulating" VS Code extensions: declarative parts plug straight into the IDE, and parts that need code really run inside a `node` child process.

| Contribution point | Status | Description |
| --- | --- | --- |
| `contributes.themes` | ✅ | Maps VS Code theme JSON color keys to this project's CSS variables (`editor.background` → `--bg-0`, etc.); preview on select |
| `contributes.snippets` | ✅ | Parses snippet JSON into CodeMirror 6 autocompletion (`${1:label}` / `${1\|a,b\|}` placeholders expand) |
| `contributes.languages` | ✅ | Extension → language id, which decides highlighting and snippet scope |
| `contributes.commands` + `extension.js` | ✅ | One Node child process per extension, injected with a `vscode` API shim; commands enter the command palette (`Ctrl+Shift+P`) and the extension card, and run |
| Language feature providers (completion / hover / diagnostics) | ⏳ registered, not yet driven | The shim has `languages.register*Provider`, but the editor's requests are not wired through yet |

The **Node extension host** runs real extensions: `vscode.commands` (register + execute + cross-extension forwarding), `window.showInformationMessage / showQuickPick / showInputBox` (real dialogs, Promise really awaits), `createStatusBarItem` (really appears in the bottom status bar, clickable), `createOutputChannel` (streams into the extension card's output panel), `workspace.fs` (read/write / list / create / delete / rename), `workspace.rootPath` and `workspaceFolders`, `window.activeTextEditor` (document content, selection, `edit()` add/remove/change hit the real editor), `context.globalState / workspaceState` (persisted to the extension's `.state/`), `env.openExternal`, and base types like `Uri / Position / Range / Selection / EventEmitter / Disposable`.

When Node is missing, the panel prompts and offers a **one-click portable download** (~30 MB, only `node.exe` is extracted to `<config>/runtime/node/`), or you can point it at an existing `node`. Each extension's host can be started/stopped independently; logs and commands live in the extension card.

The **extension store** is built in — no browser needed: a search box + categories (Themes / Languages / Snippets / Formatters / Linters / Other) + sorting (downloads / rating / update time). Open an item to see its README, rating, license, all versions (install a specific one), and repo link. Data comes from **Open VSX** — the open-source mirror of the VS Code Marketplace.

Four install sources, all pasted into one input box in the extensions panel:

- **Online**: `publisher.name` (e.g. `sdras.night-owl`) → resolved through **Open VSX** (the open-source mirror of the VS Code Marketplace) and downloaded as a `.vsix`
- **Local**: pick a `.vsix` file
- **Direct link**: any `https://…/x.vsix`
- **Git**: `github:owner/repo`, `owner/repo`, `git@…`

A `.vsix` is just a ZIP, so there is a built-in ZIP reader (central directory + `flate2` inflate) with no external unzip dependency; on unpack it strips the `extension/` prefix, rejects absolute paths and `..`, and caps file count (20k) and total size (512 MB).

DSH-style repos (skill packs with a `SKILL.md`) are recognized as type `dsh` and, on install, automatically find `SKILL.md` recursively and import it into the skill system — extensions work immediately, no second step.

> Why GitHub repos go through `codeload.github.com/{owner}/{repo}/zip/HEAD` instead of `git clone`: on Windows `git` uses its own OpenSSL CA bundle and reports `certificate signer not trusted` in environments that do TLS interception, while the app's in-process `reqwest` uses system schannel and the same network works. Non-GitHub repos fall back to `git clone` (with `-c http.sslBackend=schannel`).

**Web2API bridge (browser-side reverse proxy)**

It turns "the AI inside a web page" into "one endpoint" with zero client changes. Note: this proxies **browser-based AI**, not the API providers configured in the app — the DeepSeek etc. keys under `Settings → Providers` are for the agent; the provider mode is only a debug-only toggle for the bridge itself.

- By default routes by **model name**: `claude*` → Claude Web, `gpt-*` / `o1` / `o3` → ChatGPT Web, `gemini*` → Gemini Web; the routing table is editable in the panel, with an unmatched default web source
- Each web source has two request modes: `vendor site + browser session` (default — drives the vendor's own input box inside a persistent WebView opened by `websession.rs`, then reads the page's answer; cookie / UA / origin / PoW / sentinel tokens all come from the page, no credential copying) and `direct template` (legacy — fill a sessionKey by hand, then `reqwest` connects directly). "Open vendor site" on the panel is for login; closing the window only hides it, not destroys it, so the login state persists; "Test connection" really sends a minimal request and lists each source's HTTP status
- Each web source stores its web models, site URL, and endpoint independently. Endpoint, auth header, request body, text-field path, event filter, and whether to accumulate full text are all editable under "Advanced" — **a private API change needs no rebuild**
- Measured: hitting the private endpoints directly without a browser session returns `403` (claude.ai), `401` (chatgpt.com), `405` (gemini.google.com); switching to the vendor browser session lets the request reach ChatGPT's private API and returns the site's own response (403 anti-bot when not logged in). Some sites' private APIs also use page-generated PoW / sentinel tokens that may still need targeted adaptation after login
- Dual-protocol egress: `POST /v1/chat/completions` (OpenAI) and `POST /v1/messages` (Anthropic), **both support `stream: true`**; SSE framing (chunked) is hand-written with no web framework
- `/v1/models` lists each web source's models plus `gpt-4o` / `claude-sonnet-4-5` / `gemini-2.5-pro` aliases; the response echoes back the model name the client requested
- Local auth: `Authorization: Bearer sk-irixen-…` or `x-api-key`; by default it only listens on `127.0.0.1`, not exposed
- The panel can change the port, enable auto-start, and show live request logs (**matched web source** / path / status / latency / error); a failed start leaves the reason on the panel
- **No loss of thinking content**: a reasoning model's `reasoning` maps to `reasoning_content` on the OpenAI side and folds into the body on the Anthropic side; if the body is empty, the thinking text is used as a fallback
- Plain text: by default only the **latest user message** is sent to the vendor site (the page itself is a stateful session that maintains persona and history, so IRIXEN's system prompt is not re-injected); a web source's "send content" can switch back to `full prompt + history`. Stateless channels like `direct template` / `api` always need the full prompt. tools / images / attachments are not supported; the web side is an unofficial interface and may break at any time; credentials are stored only in the local config

Usage: in the panel, click **Open vendor site** for each web source to log in (or switch to "direct template" and fill the session credential) → **Save & Apply** → **Start** → point the client's OpenAI `base_url` at `http://127.0.0.1:8791/v1` (Anthropic SDK: `http://127.0.0.1:8791`), and fill the model name per the routing table. `/v1/models` needs no auth; `/v1/chat/completions` and `/v1/messages` need the local key from the panel.

**Skills**

- `SKILL.md` + frontmatter (`name` / `description` / `when`); when `when` matches the user input, the system prompt is injected automatically
- 5 in-house built-in skills, all oriented toward "save tokens, fewer detours": `token-saver` `diff-surgeon` `repo-scout` `commit-craft` `code-reviewer`
- Interactive install: a local directory, an http(s) link to a `SKILL.md`, or a Git repo
- Only matched skills are injected — the full skill set is never stuffed into context, which is itself a token-saving design

## Installation

Two release assets are published; the installer is the recommended path:

- **Windows installer (recommended)**: on [Releases](https://github.com/lanche-furry/IRIXEN-Code/releases) download `IRIXEN Code_*_x64-setup.exe` and run it. It creates Start Menu / desktop shortcuts and bootstraps the WebView2 Runtime (Win10 needs internet; Win11 has it built in). The installer has a built-in **English / Simplified Chinese** language-selection screen.
- **Portable**: `irixen-code.exe` runs without installation.

> WebView2 Runtime is required: Win11 has it built in; Win10 gets it via the installer over the internet.

## Quick Start

```bash
npm install
npm run app:dev      # develop (auto-starts vite + tauri)
npm run app:release  # build the standalone release exe (--no-bundle)
npm run app:build    # build the NSIS installer
```

After first launch: go to `Settings → Providers`, enter the Base URL and API Key, click **Test connection**, and the model list appears automatically; then pick a model and a thinking-effort level above the chat box.

> Build environment: Rust 1.77+ (GNU target), Node 18+, WebView2 on Windows 10/11.
> This repo uses a **portable toolchain**: Rust and build tools all live under `.tools/` — see [docs/BUILD.md](docs/BUILD.md).

## Project Layout

```
src/                    Frontend (TypeScript, zero framework)
  core/                dom / i18n (8 languages) / api / state
  core/locales/        zh-TW / es-ES / fr-FR (the rest inline in i18n.ts)
  ui/                  shell (layout + quick bar) · chat · editor · explorer · search
                       git · skills · stats · review · bridge · extensions · settings
  styles/              base.css + app.css (all design tokens)
src-tauri/             Rust backend
  src/agent/           provider (three protocols) / models / context / tools / meter / types
  src/bridge.rs        Web2API: hand-written HTTP/1.1 + SSE, OpenAI / Anthropic dual egress
  src/extensions.rs    Extension subsystem: VSIX unpacking (built-in ZIP reader) / Open VSX store / themes·snippets·skills
  src/exthost/         Node extension host: process management + NDJSON RPC + bootstrap.js (vscode API shim)
  src/commands.rs      IPC command surface
  src/mcp.rs           MCP stdio client
  src/skills.rs        Skill registry (compile-time built-ins + user installs)
  src/skills/builtin/  5 built-in SKILL.md
public/assets/         Wordmark and other static assets
assets/icon.png        App icon master (1254×1254 transparent PNG; `npm run icons` generates the full size set)
scripts/               Icon generation / portable toolchain launcher
docs/                  Architecture, build, skill development
```

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Build & Toolchain](docs/BUILD.md)
- [Skill Development](docs/SKILLS.md)
- [Contributing](CONTRIBUTING.md)

## License

[Apache License 2.0](LICENSE) © lanche-furry

</div>
