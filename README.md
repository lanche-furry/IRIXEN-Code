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

<p align="center">
  <a href="./README.en.md"><strong>English</strong></a> &nbsp;|&nbsp; 中文
</p>

IRIXEN Code 由两部分组成：

| 组件 | 说明 |
| --- | --- |
| **IRIXEN Agent** | Rust 写的 agent 核心：三协议统一抽象、原生工具、MCP 客户端、技能系统、三级上下文压缩 |
| **IRIXEN Code** | 桌面外壳：Tauri 2 + 系统 WebView2，无边框窗口、无启动画面（创建即显示）、左侧导航栏 + 对话主区 + 右侧快捷栏；打开文件时编辑器**覆盖**对话区而不是新开一列 |

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
- **整轮可撤销**：每轮改动前记下每个文件**第一次**被写之前的原内容，输入框上方显示「N 个文件已改动 ⟲ 撤销这一轮」；逆序回滚、删掉本轮新建的文件，不依赖 git。单轮超过 8MB 或碰到二进制文件则标注不可撤销并给出原因（宁可不做也不做一半），只保留最近 48 轮
- **MCP 客户端**：JSON-RPC over stdio，握手 `2024-11-05`，MCP 工具与原生工具同池调度，Windows 下自动用 shell 包装 npx / uvx 垫片

**三级上下文压缩**（省 token 的关键）

1. **工具结果折叠** —— 超过阈值的历史工具输出压成一行摘要（保留路径与结论）
2. **滑动窗口 + 任务锚点** —— 保留最近 K 条消息，更早的合并为锚点（目标 / 决策 / 文件）
3. **LLM 结构化摘要** —— 仍超预算时调用模型生成 `Goal / Decisions / Files / State / Pending` 摘要，失败自动回退启发式摘要；摘要阶段**不持锁**，不阻塞 UI

**IDE 外壳**

布局不是 VS Code 那种「三列常驻」：**左侧导航栏 + 中间对话区**，代码文件是**覆盖**在对话区上打开的，关掉标签就回到对话，视觉焦点永远只有一个。

- 无边框窗口、自绘标题栏与窗口按钮（没有 agent 徽标，标题栏只留工作区与窗口操作）
- **左侧导航列**（固定 56px，图标 + 短标签，位置与顺序不变）：文件 / 搜索 / Git / 技能 / 统计 / 审查。旁边是当前视图的内容栏，顶部显示视图名与对应工具按钮；资源管理器有 **新建文件 / 新建文件夹 / 全部折叠** 三个快捷按钮（右键菜单同样有）
- **右侧 40px 快捷栏**：对话、扩展（角标 = 正在运行的宿主数）、文件（有打开的标签时才出现）、终端、设置
- **对话区是可被覆盖的画布**：点开文件 → 编辑器连标签栏一起 `inset: 0` 盖住对话区；标签栏右侧固定一个「回到对话」按钮，常驻不随标签滚动
- 启动：**没有 splash 窗口、没有入场动画**。窗口以 `visible: true` 创建（底色 = 主题 `--bg-0`），进程起来就能看到，UI 首帧直接落在上面；`main_loaded` 只用来回收焦点并记录挂载耗时
- CodeMirror 6 编辑器：多标签、语法高亮、脏标记、`Ctrl+S` 保存、从搜索结果跳转到行
- **图片输入**：`Ctrl+V` 粘贴或直接拖进来，长边超过 1600px 自动等比缩放并铺白底（透明 PNG 不变黑）；按协议原生形状发送（OpenAI `image_url` data URL / Anthropic `source.base64` / Gemini `inlineData`），纯文字回合仍发字符串。单张 ≤ 4MB、一次 ≤ 8 张，每张按 1000 token 计入上下文预算
- **会话历史**：可搜索的会话列表（标题 / 消息数 / 相对时间），点开即接着聊，悬停删除；删掉当前会话时自动新建，切会话清空待发图片
- **应用内更新**：关于页的「下载并安装」流式下载 → 与发布页 `checksums.txt` 比对 SHA-256 与文件大小 → 启动安装器后退出。Windows 的 exe 换不掉自己，所以安装永远是独立进程
- 文件树（懒加载）、全工作区搜索（遵循 `ignore` 规则）、Git 面板（暂存 / 提交 / 推送 / 拉取 / 分支 / 差异）
- **审查面板**：一眼扫完工作区改动（`git status` + diff 统计），一键把「这些文件改了什么、有没有风险」连同当前 diff 交给 Agent 审。行级差异用红绿着色，和状态栏、快捷栏角标联动
- **安全审查**：审查面板里的「安全审查」只发一句意图，Agent 先跑 `security_scan`（原生工具，固定规则表：云厂商密钥 / 私钥块 / 关掉的 TLS 校验 / shell 与 SQL 注入 / 全局可写 / 密钥文件 / CI 里没锁 SHA 的 action），再逐条读代码判断是否真的可达；扫描输出里密钥一律脱敏（只留 4 位头尾），枚举交给扫描器、判断留给模型
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

没有 Node 时会提示并**一键下载 Node 运行时**（约 30 MB，只解出 `node.exe` 存到 `<config>/runtime/node/`），也可以手动指定已有的 `node`。每个扩展的宿主可以单独启停，日志与命令都在扩展卡片里。

**插件商店**是内置的，不需要开浏览器：搜索框 + 分类（主题 / 语言 / 片段 / 格式化 / 检查 / 其他）+ 排序（下载量 / 评分 / 更新时间），点开即见 README、评分、许可证、全部版本（可装指定版本）、仓库链接。数据来自 **Open VSX**——VS Code Marketplace 的开源镜像。

安装来源四种，都在扩展面板里一个输入框粘贴即可：

- **在线**：`publisher.name`（如 `sdras.night-owl`）→ 直接查 **Open VSX**（VS Code Marketplace 的开源镜像）并下载 `.vsix`
- **本地**：选一个 `.vsix` 文件
- **直链**：任何 `https://…/x.vsix`
- **Git**：`github:owner/repo`、`owner/repo`、`git@…`

`.vsix` 就是一个 ZIP，所以自带了一个 ZIP 读取器（中央目录 + `flate2` 解压），不依赖外部解压程序；解包时剥掉 `extension/` 前缀、拒绝绝对路径与 `..`，并限制文件数（2 万）与总大小（512 MB）。

DSH 风格的仓库（带 `SKILL.md` 的技能包）会被识别为 `dsh` 类型，安装时自动递归查找 `SKILL.md` 并导入技能系统——扩展装完即生效，不用二次操作。

> 为什么 GitHub 仓库走 `codeload.github.com/{owner}/{repo}/zip/HEAD` 而不是 `git clone`：Windows 上 `git` 用自己的 OpenSSL CA bundle，在会做 TLS 拦截的环境里会报 `certificate signer not trusted`；而应用内的 `reqwest` 走系统 schannel，同一条网络是通的。非 GitHub 的仓库才回退到 `git clone`（并加 `-c http.sslBackend=schannel`）。

**技能（Skills）**

- `SKILL.md` + frontmatter（`name` / `description` / `when`），`when` 命中用户输入即自动注入系统提示词
- 5 个自研内置技能，全部是"省 token、少绕路"取向：`token-saver` `diff-surgeon` `repo-scout` `commit-craft` `code-reviewer`
- 交互式安装：本地目录、`SKILL.md` 的 http(s) 链接、Git 仓库
- 只注入"匹配到的技能"，不把全部技能塞进上下文——这本身就是省 token 的设计

## 安装

发布资产按平台提供：

- **Windows 安装程序（推荐）**：在 [Releases](https://github.com/lanche-furry/IRIXEN-Code/releases/latest) 下载 `IRIXEN Code_*_x64-setup.exe`，双击运行即可安装，会在开始菜单与桌面创建快捷方式，并自动补齐 WebView2 Runtime（Win10 需联网；Win11 自带）。安装程序内置 **English / 简体中文** 语言选择界面。
- **Linux**：同一页下载 `IRIXEN_Code_*_amd64.AppImage`（`chmod +x` 后直接运行）或 `IRIXEN_Code_*_amd64.deb`（`sudo apt install ./…deb`）。需要系统提供 `libwebkit2gtk-4.1-0` 与 `libgtk-3-0`，deb 会自动拉依赖；AppImage 不带系统库。Ubuntu 22.04+ / Debian 12+ 可直接运行。

> 需要 WebView2 Runtime：Win11 自带；Win10 由安装程序联网补齐。

每日构建在 [nightly](https://github.com/lanche-furry/IRIXEN-Code/releases/tag/nightly) 预发布频道，随 `main` 变动，不保证稳定；正式版以 latest 频道为准。

## 快速开始

```bash
npm install
npm run app:dev      # 开发（自动拉起 vite + tauri）
npm run app:release  # 出 release 独立 exe（--no-bundle）
npm run app:build    # 出 NSIS 安装包
```

首次启动后：`设置 → 供应商`，填入 Base URL 与 API Key，点**测试连接**，模型列表会自动出现；然后在对话框上方选模型与思考强度。

> 构建环境：Rust 1.77+（GNU target）、Node 18+、Windows 10/11 自带 WebView2。
> 本仓库采用**自带工具链**方案，Rust 与构建工具全部位于 `.tools/`，详见 [docs/BUILD.md](docs/BUILD.md)。

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
  src/extensions.rs     扩展子系统：VSIX 解包（自带 ZIP 读取器）/ Open VSX 商店 / 主题·片段·技能
  src/exthost/          Node 扩展宿主：进程管理 + NDJSON RPC + bootstrap.js（vscode API 垫片）
  src/commands.rs       IPC 命令面
  src/mcp.rs            MCP stdio 客户端
  src/skills.rs         技能注册表（编译期内置 + 用户安装）
  src/skills/builtin/   5 个内置 SKILL.md
public/assets/         字标等静态资源
assets/icon.png        应用图标母版（1254×1254 透明 PNG，`npm run icons` 生成全套尺寸）
scripts/                图标生成 / 工具链启动器
docs/                   架构、构建、技能开发
```

## 文档

- [架构设计](docs/ARCHITECTURE.md)
- [构建与工具链](docs/BUILD.md)
- [技能开发](docs/SKILLS.md)
- [杀软误报记录](docs/antivirus.md)
- [贡献指南](CONTRIBUTING.md)

## 许可

[Apache License 2.0](LICENSE) © lanche-furry
