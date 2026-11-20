# 架构设计

<p align="center">
  中文 &nbsp;|&nbsp; <a href="./ARCHITECTURE.en.md">English</a>
</p>

IRIXEN Code 只有两层：**Rust 进程**（IRIXEN Agent + 系统能力）和 **WebView 渲染层**（IRIXEN Code 界面）。两者通过 Tauri IPC 通信，没有 HTTP 中间层，没有本地服务端口。

```
┌─────────────────────────── WebView2 (renderer) ───────────────────────────┐
│  src/core/      dom · i18n · api · state（一个手写事件总线）               │
│  src/ui/        shell · editor · explorer · search · git · skills ·        │
│                 stats · chat · review · bridge · settings · theme ·        │
│                 markdown · toast                                          │
│  src/styles/    base.css（设计令牌）· app.css（布局）                      │
└───────────────────────────────────┬───────────────────────────────────────┘
                     invoke()       │        listen('agent://event')
┌───────────────────────────────────┴───────────────────────────────────────┐
│                              src-tauri (Rust)                             │
│  commands.rs   ← 全部 #[tauri::command]，薄封装，只做参数转发与错误转换     │
│  state.rs      AppState { config, engine, skills, mcp }                    │
│  bridge.rs     Web2API：手写 HTTP/1.1 + SSE，OpenAI / Anthropic 出口       │
│  websession.rs 官网浏览器会话：每源一个 WebView，页面内同源 fetch + 回调   │
│  agent/                                                                   │
│    provider.rs  LlmProvider trait + OpenAi/Anthropic/Gemini 三个实现       │
│    models.rs    模型目录 + 思考能力推断 + 10 分钟磁盘缓存                   │
│    context.rs   Compressor：折叠 / 窗口 / 摘要                             │
│    tools.rs     8 个原生工具（工作区内路径沙箱）                            │
│    meter.rs     UsageMeter：按模型 / 按会话计量                             │
│    mod.rs       AgentEngine：会话、回合循环、事件发射                       │
│  mcp.rs         McpHost / McpConn：JSON-RPC over stdio                     │
│  skills.rs      SkillRegistry：编译期内置 + 用户安装                        │
│  git.rs         直接调用系统 git                                           │
└───────────────────────────────────────────────────────────────────────────┘
```

## 1. 窗口生命周期

只有一个 `main` 窗口，在 `tauri.conf.json` 里就是 `visible: true`，并且 `backgroundColor` 与浅色主题的 `--bg-0` 一致。

启动路径刻意做到最短：**没有 splash 窗口，没有入场动画，也没有"等 UI 就绪再显示"的门禁**。

1. 进程起来 → 窗口立即出现，底色是 `#eaedf2`（深色主题下 `index.html` 的内联脚本会在解析后第一帧换成 `#0a0d13`）
2. 前端 `mountShell()` 完成 → 调用 `main_loaded`
3. `lib.rs::mark_main_loaded` 做两件事：把焦点收回主窗口（`vite` 冷启动期间可能有别的窗口抢走）、把挂载耗时打进日志

窗口的可见性不依赖任何 IPC，所以前端即使崩在启动阶段，用户看到的也是一个可见窗口 + devtools 报错，不会出现"进程在跑但没有窗口"的状态。

## 2. 三协议统一抽象

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn protocol(&self) -> Protocol;
    async fn chat(&self, req: ChatRequest) -> Result<StreamResult, ProviderError>;
    async fn list_models(&self) -> Result<Vec<RawModel>, ProviderError>;
}
```

`chat()` 返回 `StreamResult`：一个归一化 `StreamEvent` 流 + 最终 `Usage`。

```rust
enum StreamEvent {
    Text(String),
    Reasoning(String),
    ToolCallStart { id, name },
    ToolCallArgs { id, delta },
    Usage(Usage),
}
```

三个实现各自把厂商格式翻译成这套事件，`AgentEngine` 只认这套事件，因此新增厂商 = 新增一个 `impl`，不动引擎。

| | OpenAI | Anthropic | Gemini |
| --- | --- | --- | --- |
| 端点 | `POST {base}/chat/completions` | `POST {base}/messages` | `POST {base}/models/{m}:streamGenerateContent?alt=sse` |
| 鉴权 | `Authorization: Bearer` | `x-api-key` + `anthropic-version` | `x-goog-api-key` |
| 思考 | `reasoning_effort` | `thinking.budget_tokens` | `generationConfig.thinkingConfig.thinkingBudget` |
| 工具 | `tools[].function` | `tools[].input_schema` | `tools[].functionDeclarations` |
| 工具结果 | `role: tool` | `tool_result` block | `functionResponse` part |

`extra_body` 会 merge 进请求体最外层，`extra_headers` 会加进请求头——用来处理诸如 Qwen `enable_thinking`、部分中转站自定义字段这类厂商特例，不必改代码。

### 思考强度映射

`Effort { Auto, Off, Minimal, Low, Medium, High }` 是内核统一枚举，落到协议时再翻译：

- OpenAI（chat/completions）：顶层 `reasoning_effort`，`off → "none"`。实测 deepseek-flash / v4-pro 都认这一字段，`none` 会把 `reasoning_tokens` 压到 0，是真正的开关而不是提示
- Anthropic：`budget_tokens` 按档位取 1024 / 2048 / 8192 / 16384，且必须 `max_tokens > budget`
- Gemini：`thinkingBudget` 0 / 1024 / 4096 / 12288
- `Auto` 一律不传，交给厂商默认

厂商不认识这个字段时会回 400/422；`provider.rs::rejects_thinking()` 只在状态码是 400/422 **且** body 提到 reasoning/thinking/unknown field 时重发一次（去掉该字段），401/429/500 绝不重试。这样「给不支持的模型选档位」的代价是一次请求，不是一个回合。

### 档位是粘性的

- 单个会话：`agent_set_model(effort)` 写进 session
- 全局默认：`set_default_effort(effort)` 写进 `config.effort`，新会话从这里起步，重启不丢
- 聊天面板的强度选择同时更新两者；`设置 → Agent → 默认思考强度` 只改后者

## 3. 模型目录与思考能力

三层来源，优先级从高到低：

1. **fetched** —— 真实调用 `list_models()` 拿到
2. **heuristic** —— 用命名规则推断（`models.rs::detect_thinking`）
3. **cached** —— `model-cache.json`，TTL 10 分钟

`detect_thinking()` 的规则表：

| 模型名匹配 | ThinkingSupport |
| --- | --- |
| `deepseek*`（除 `*reasoner*` / `*-r1*`） | `Effort { levels: [minimal, low, medium, high] }` |
| `gpt-5*` `o1*` `o3*` `o4*` | `Effort { levels: [minimal, low, medium, high] }` |
| `glm-z1*` `glm-4.5*` `glm-4.6*` | `Effort { levels: [low, high] }` |
| `*qwq*` `*thinking*` | `Budget { min: 0, max: 32768 }` |
| `claude-3-7*` `claude-*-4*` | `Budget { min: 1024, max: 32000 }` |
| `gemini-2.5*` `gemini-3*` | `Budget { min: 0, max: 24576 }` |
| `deepseek*reasoner*` `*-r1*` | `AlwaysOn` |
| 其它 | `Unsupported` |

缓存命中也走一遍 `ModelInfo::refresh_thinking()`：能力是「从 id 推出来的」，所以探测规则修好后老缓存立刻生效，不必等 TTL 过期。

前端 `effortChoices()` 在 `unsupported` / `unknown` 时**仍然给出完整档位**，因为不少 OpenAI 兼容厂商默默接受 `reasoning_effort`；真正不认的由上面的重试兜底。只有 `always_on`（如 `deepseek-reasoner`）退化成仅 `auto`——那种模型的思考关不掉，给档位就是假交互。

## 4. 工具循环

`AgentEngine::run_turn()` 是一个 `loop`：

```
while iteration < max_iterations:
    1. 压缩检查（maybe_compress）
    2. 组装请求（system + 技能注入 + 历史 + 工具表）
    3. 流式请求，边收边 emit（delta / reasoning / tool_call）
    4. 若模型返回 tool_calls：
         - 原生工具 → tools::execute_native()
         - mcp__* 工具 → McpHost::call()
         结果 push 进消息，iteration += 1，继续
    5. 否则 emit(done) 并结束
```

每次工具调用都 emit `tool_call`（开始）和 `tool_result`（结束），前端据此画出带状态点的工具卡片。

**路径解析**：`tools::resolve()` 把相对路径拼到工作区根，`canonicalize` 后比较前缀；越界能不能放行由当前访问模式决定（见下）。`run_command` 在工作区 cwd 执行，超时 120s，输出截断 18k 字符。

### 访问模式：三档，判断放在纯函数里

`access.rs` 不碰 IO、不碰状态，签名是 `decide(mode, tool, args, workspace) -> Allow | Ask(Request) | Deny(String)`，所以规则集全是单测能覆盖的纯逻辑。三档：

| 模式 | 工作区内读 | 工作区内写 | 越界 | `run_command` |
| --- | --- | --- | --- | --- |
| 受限 `restricted` | 放行 | 每次问 | **拒绝**，不是问 | 每次问 |
| 自动 `auto`（默认） | 放行 | 放行 | 问 | 只问 cwd 越界的 |
| 完全 `full` | 放行 | 放行 | 放行 | 放行 |

三个刻意的选择：

- **受限模式下越界直接拒，不问。** 问一次就能放行，等于一张无限次口头许可，跟自动模式没区别；受限的含义是这条路不存在，而不是门口站着个人。
- **`run_command` 不解析 shell 字符串。** `sh -c` 里能写出多少种绕过方式，就有多少种解析器会看错；一个读了 shell 字符串、于是自以为知道它要动什么的权限系统，比一个承认自己看不出来的权限系统更危险。所以命令在受限模式下永远问，在自动模式下只在 cwd 越界时问。
- **`full` 必须是显式选择。** 配置里没有这个字段等于 `auto`；前端选到 `full` 会提示「这之后不再拦截」。

「问」是真的挂起：`ask_approval()` 把 `oneshot::Sender` 存进 `pending` 表，emit 一条 `approval` 事件带 id，前端把卡片画进对话；用户点允许/拒绝才 `send(bool)` 唤醒那边等着的 `execute_tool`。15 分钟没回答按拒绝处理；`agent_stop` 会把该会话所有挂起的审批一次性按拒绝释放——不能让一个被停掉的回合继续挂在内存里。被拒之后返回给模型的是「用户拒绝了这次调用」加上 `is_error = true`，模型会换个办法，而不是以为工具坏了。

模式也写进 system prompt，后面跟一句「这是用户的选择，你不能自行更改」：模型知道自己什么时候会被拦，就不必花三轮去试同一件事。

**`security_scan` 与 `audit.rs`**：唯一会遍历整棵树的工具，所以放在 `spawn_blocking` 里跑——一次六千文件的清扫不该占住所有事件共用的 async 线程。`audit.rs` 用 `ignore` 走一遍，刻意 `hidden(false)`：`.env` 和 `.github/workflows` 本来就是审计对象。规则表是固定的（云厂商密钥、私钥块、JWT、赋给字面量的密钥、关掉的 TLS 校验、动态执行、SQL 字符串插值、全局可写，外加按路径判的密钥文件与未锁 SHA 的 CI action），命中先脱敏再拼进 evidence：匹配区段换成「4 位头 + *** + 3 位尾」，模型看到的是 `AKIA***F7Q2` 而不是真值。扫描器只负责枚举，能不能利用由模型读代码判断——`render()` 结尾把这句话直接写给模型看，并明说「没命中不等于安全」。

## 5. 上下文压缩

`context.rs` 三步，每步都以"保住结论、丢掉重复"为原则：

**① 工具结果折叠**（`trim_tool_results`）
距离当前超过 `tool_result_age` 条的工具结果，若长度超过 `tool_result_cap`，替换为 `[elided N chars] 首行摘要`。

**② 滑动窗口**（`split_window`）
保留最近 `keep_recent` 条；更早的消息合并成一条"任务锚点"系统消息，抽取：用户原始目标（首条 user 消息，截断）、已决策项、涉及文件路径（从工具调用参数里抽 `path` 字段去重）。

**③ LLM 摘要**（`digest`）
估算 token 仍超过 `max_context_tokens * threshold` 时，用当前模型生成结构化摘要：

```
Goal / Decisions / Files / State / Pending
```

`digest_max_tokens` 限制长度。**这一步不持锁**：先在锁内裁剪并 clone 出待摘要消息，释放锁后再发请求，避免 `async` 跨 `await` 持锁导致 UI 卡死。请求失败则退回 `heuristic_digest`（纯规则拼接），保证流程不断。

压缩完成后 emit `compressed { tokens_before, tokens_after, tool_results_elided, messages_dropped, digest }`，前端显示为一条系统提示。

## 6. MCP

`mcp.rs` 是手写的 stdio 客户端，不依赖 SDK：

- `McpConn::spawn()`：启动子进程，stdin/stdout 走 JSON-RPC 2.0（每行一条），stderr 保留最近 40 行供 UI 排障
- 握手：`initialize`（protocolVersion `2024-11-05`）→ `notifications/initialized` → `tools/list`
- 调用：`tools/call { name, arguments }`，结果把 content 数组里的 text 拼起来
- 工具名在引擎侧统一加前缀 `mcp__{server}__{tool}`，与原生工具同池，模型看不出差别
- Windows 下如果命令是 `npx` / `uvx` / `node` 等 `.cmd` 垫片，自动用 shell 包装执行

## 7. 技能系统

```
src-tauri/src/skills/builtin/*.md     ← include_str! 编译进二进制
~/.irixen-code/skills/<id>/SKILL.md   ← 用户安装
```

frontmatter 解析是手写的（不引 YAML 库）：

```markdown
---
name: token-saver
description: 一句话说明
when: 省token, 太长, 精简, 冗长
---
正文（注入系统提示词的部分）
```

`match_for(prompt, auto)` 用 `when` 关键词做子串匹配（大小写不敏感）。只有命中才注入——这是刻意的：把所有技能塞进 system prompt 既贵又会让模型分心。

安装来源三种：本地目录、`http(s)` 的 `SKILL.md`、Git 仓库（浅克隆后找 `SKILL.md`）。

## 8. IPC 命令面（节选）

| 分组 | 命令 |
| --- | --- |
| 配置 | `get_config` `save_config` `app_info` `set_workspace` |
| 供应商 | `provider_upsert` `provider_delete` `provider_test` `models_list` |
| 会话 | `agent_new_session` `agent_set_active` `agent_messages` `agent_send` `agent_stop` `agent_clear` `agent_set_model` `agent_usage` |
| 访问控制 | `set_agent_access` `agent_approve` |
| 文件 | `fs_list` `fs_read` `fs_write` `fs_create_file` `fs_create_dir` `fs_delete` `fs_rename` `fs_search` |
| Git | `git_is_repo` `git_status` `git_diff` `git_stage` `git_unstage` `git_commit` `git_branch(es)` `git_checkout` `git_log` `git_push` `git_pull` |
| MCP | `mcp_servers` `mcp_upsert` `mcp_delete` `mcp_start` `mcp_stop` `mcp_tools` |
| 技能 | `skills_list` `skills_install` `skills_uninstall` `skills_toggle` `skills_read` |
| 扩展 | `extensions_list` `extensions_install` `extensions_uninstall` `extensions_toggle` `extensions_search` `extensions_theme` `extensions_snippets` |
| 桥接 | `web2api_status` `web2api_start` `web2api_stop` `web2api_log` `web2api_open_session` `web2api_test` |
| 窗口 | `main_loaded` `window_control` |

事件只有一个通道 `agent://event`，payload 是 `AgentEvent`（serde `tag = "kind"`），前端按 `session` 过滤。

## 9. 布局契约

外壳只有一条硬规则：**同时只有一个焦点区**。对话是主区，代码是**覆盖层**，不是第三列。

```
.app        grid-template-columns: minmax(0, 1fr)   /* 显式单列，禁止隐式列撑宽 */
 └ .body    grid-template-columns: var(--act-w) var(--sidebar-w) 3px minmax(0,1fr) var(--rail-w)
    ├ .activity  56px（文件 / 搜索 / Git / 技能 / 统计 / 审查，icon + 短标签）
    ├ .sidebar   264px（视图名 + 视图工具栏 + 内容）
    ├ .resizer   3px
    ├ .stage     minmax(0, 1fr)
    │  ├ .dock[data-active=chat|review|bridge|extensions]   面板切换靠 data-active，非销毁重建
    │  └ .editor-area  position:absolute; inset:0; z-index:3   ← 覆盖对话区
    └ .rail      40px（对话 / Web2API / 扩展 / 文件 / 终端 / 设置）
```

`.activity` 与 `.rail` 是两条不同的轴：左边是「侧栏在看什么」（`app.view`），右边是「中间在看什么」（`app.surface`）。审查面板属于后者，所以它放在 `.activity` 末尾但高亮的是 surface——这解释了为什么看审查时左边会同时有两个高亮项。

三个细节是踩过坑之后才定下来的，改动时不要回退：

1. **`1fr` 必须写成 `minmax(0, 1fr)`**。`1fr` 等价于 `minmax(auto, 1fr)`，会接受子元素的 min-content 宽度；只要有一个 `white-space: nowrap` 且没有 `min-width: 0` 的子元素（曾经的候选：状态栏文字），整条网格线就被撑到一个不可能的值，布局看起来像"崩了"。`.app` / `.body` / `.stage` 全部加 `minmax(0, …)`，且每层 `overflow: hidden`。
2. **`.app` 必须显式声明 `grid-template-columns`**。留空会生成一个隐式列，宽度由最宽子元素决定——同上。
3. **状态栏只许放标签，不许放内容**。`agent:done` 事件带的是完整回答；早期直接 `setStatus(ev.text)` 让 5000 字的回答进了状态栏。现在三层兜底：调用点只放 ≤24 字的短语、`setStatus()` 再截到 90 字、CSS 上 `.sb-item` 一律 `min-width:0; overflow:hidden; text-overflow:ellipsis`。

界面"假死"还有一个非 CSS 的来源：**Tauri 的同步 `fn` 命令跑在主线程上**（和 IPC、窗口事件同一条线程）。`workspace_stats` 会遍历工作区、把 1.5 MB 以下的文件全部读一遍数行数，`fs_search` 会全量正则，`git_*` 会起子进程——这些一旦是同步命令，窗口就真的不响应了。现在这类命令统一 `async` + `spawn_blocking`，并且 walker 一律跳过 `node_modules` / `target` / `dist` 等目录（`commands.rs::walker()`）。实测统计 97 ms、全库搜索 114 ms，期间最长帧间隔 17 ms。


面板切换用 `.dock[data-active]` + `display:none`，不卸载 DOM：库里滚动的对话历史、编辑器标签、桥接请求日志都靠这一点保持状态。编辑器覆盖也**不销毁** dock，只 `hidden`，所以「回到对话」是瞬时的。

## 10. 前端

没有框架，也没有虚拟 DOM：

- `h()` 建节点，`clear()` 清空，状态变化时显式重绘局部
- `state.ts` 的 `app` 是唯一可变状态对象；`on/emit` 是 30 行的发布订阅
- 对话流用**增量 DOM**：每个消息 id 对应一个 `MsgView`，`delta` 事件只改文本节点；流式期间渲染纯文本（`white-space: pre-wrap`），`done` 时才跑 markdown——避免 O(n²) 的反复解析
- 主题靠 CSS 变量：`applyTheme()` 在 `<html>` 上设 `data-theme` 或直接覆写变量；自定义主题额外派生 `--accent-soft`（accent 的 14% alpha）与对比色
- i18n 是平铺字典 + `{name}` 插值，共 8 种语言。`zh-CN` 用 `satisfies Dict` 作为**唯一事实来源**，其余语言被标成 `Record<keyof typeof zhCN, string>` 并各自 `satisfies`，所以少一个键是 `tsc` 报错，不是运行期静默回退
- 切语言是**实时的**：`setLang()` 只在值真的变化时 `emit('lang:changed')`，外壳的 `relabel()` 重画标题栏 / 导航列 / 侧栏头 / 右侧栏 / 状态栏，并从 `app` 状态**重新挂载** dock 面板（对话记录由 `renderLog()` 从 `app.chat` 重建）。如果切换时正在流式输出，面板重挂载会推迟到 `agent:busy=false`，避免把半截回答撕掉
- **后端不下发句子**：像"这个扩展的哪部分可用"这种说明，Rust 只给 token（`kind` + `parts: ["themes","snippets"]`），措辞由前端字典决定，否则中文句子会漏进法语界面

## 11. Web2API 桥接

目标：把**网页端 AI** 反向代理成客户端眼中的标准 API。`OpenAI SDK`、Cursor 等只改 `base_url` 即可接入。应用里 `设置 → 供应商` 的 API key（DeepSeek 等）是给 agent 用的，**不是** Web2API 的上游；`provider` 只剩一个「仅调试 agent」的开关。

```
客户端  ──HTTP/1.1──▶  Bridge (127.0.0.1:8791)
                         ├─ 鉴权：Bearer / x-api-key
                         ├─ 路由：/v1/models · /v1/chat/completions · /v1/messages · /health
                         ├─ 按 model 前缀选网页源（可编辑路由表 + 默认源）
                         │    claude* → Claude Web · gpt-*/o1/o3 → ChatGPT Web · gemini* → Gemini Web
                         ├─ 每个源：会话凭证 + 端点/鉴权头/请求体模板/文本路径
                         └─ 流式：自己的 SSE + chunked 编码器
```

**没有引 web 框架。** `bridge.rs` 直接在 `tokio::net::TcpListener` 上手写 HTTP/1.1：读头、按 `Content-Length` 读体、写响应、写 `Transfer-Encoding: chunked` 的 SSE 帧。

### 可配置的网页源

`BridgeConfig` 里 `sources: Vec<WebSource>` + `routes: Vec<ModelRoute>` + `default_source`。老配置的 `claude_web` 与 `claude_session_key` 等在 `AppConfig::load()` 里由 `migrate()` 折进预设，默认上游从 `provider` 切到 `web`。

每个源有 `mode`：`browser`（默认）在 `websession.rs` 创建的常驻 WebView 里，用官网页面上下文的同源 `fetch` 发请求——cookie、UA、origin、CSRF 都由浏览器提供，不需要复制凭证；`http` 是旧的 `reqwest` 直连模板，需要手填 `session_key`。结果通过 `eval_with_callback` 轮询页面上的全局变量回传，不走本地回调端点，因此不受官网 CSP `connect-src` 与 mixed-content 限制。窗口在独立线程创建（Windows 上同步命令里建窗口会死锁）。

`web2api_test` 对每个源发一条最小请求并返回真实 HTTP 状态，面板「测试连接」直接展示；旧配置缺 `home_url` 时由 `migrate()` 从内置预设或 endpoint 的 origin 补齐。

| 字段 | 作用 |
| --- | --- |
| `mode` / `home_url` | 浏览器会话 / 直连模板；官网登录页地址 |
| `session_key` / `auth_header` | cookie、bearer token 或完整 cookie 头；`{session}` 占位符（浏览器模式下 cookie / authorization 由页面提供，模板里这两项会被忽略） |
| `endpoint` / `body_template` | `{org}` `{uuid}` `{model}` `{session}` `{prompt}` `{prompt_json}` `{req}` 占位符 |
| `text_path` / `text_guard` | 从 JSON 事件里取文本；`auto` 取最长字符串；guard 形如 `message.author.role=assistant` |
| `cumulative` | ChatGPT / Gemini 每帧重发全文，只推新增后缀；Claude 是增量，直接透传 |
| `org_endpoint` / `org_path` | Claude 未填组织 ID 时先取 `/api/organizations` |

响应解析器不假设 SSE：逐行去掉 `data:` / `)]}'` 前缀与分块长度行，凡能解析成 JSON 的都交给 `text_path`。请求日志会带上命中的 `web:<source id>`。私有接口变动时改面板「高级」里的模板即可，不必重新构建。

### 协议出口

两种请求形态共享同一套归一化数据结构（`messages` 数组 + `system` + `stream` + `max_tokens`），所以「OpenAI 进、Anthropic 出」这种混搭天然成立。

- **OpenAI**：`choices[].delta.content`，推理走非标准的 `reasoning_content`；流以 `data: [DONE]` 收尾
- **Anthropic**：`message_start` → `content_block_start` → 若干 `content_block_delta` → `content_block_stop` → `message_delta` → `message_stop`，事件名与 data 各一行
- **模型回显**：响应的 `model` 是客户端请求的名字；`/v1/models` 额外列出 `gpt-4o`、`claude-sonnet-4-5`、`gemini-2.5-pro` 等别名，专治客户端写死模型名
- **纯文本**：网页上游把多轮消息拼成一段 prompt，tools / 图片 / 附件不支持

### 三个实现坑（都已在代码里注释）

1. **锁重入死锁**：`start()` 持着 `state` 的 `Mutex` 调 `status()`，而 `status()` 也要这把**非可重入**锁 → 整个 `invoke` 永不返回。修法是把 guard 缩进一个块，出了块再查状态。
2. **空正文**：推理模型（如 `deepseek-flash`）在小 `max_tokens` 下会把预算全花在思考上，Anthropic 侧于是出现 `content_block_start` 紧接 `content_block_stop` 的零正文。四条响应路径（OpenAI / Anthropic × 流式 / 非流式）都加了兜底：没有可见正文时，把思考文本当正文发出去。
3. **UTF-8 切片**：`&body[..600]` 在中文错误体上会切在码点中间直接 panic，而 release 是 `panic = "abort"` → 进程被杀。统一改用 `util::cap_chars(body, 600)`。


## 12. 扩展（VSC / DSH 兼容）

扩展分两层：**声明式贡献点**（本节的 12.1–12.5）与**真正的 Node 扩展宿主**（§13）。判定只看 `package.json` 有没有 `main`：

```
.vsix (ZIP)
  └─ extension/package.json
       ├─ contributes.themes      → 颜色键 → 本项目 CSS 变量         ✅ 真实生效
       ├─ contributes.snippets    → CodeMirror autocompletion 源     ✅ 真实生效
       ├─ contributes.languages   → 扩展名 → 语言 id（高亮/片段域）   ✅ 真实生效
       ├─ contributes.commands    → 面板/命令面板；有 main 时可执行   ✅ 真实生效
       ├─ main: "./extension.js"  → Node 扩展宿主（§13）             ✅ 真实运行
       ├─ 补全/悬停/格式化 provider → registerXxxProvider（4 个）      ✅ 真实生效
       ├─ TreeView / Webview       → registerTreeDataProvider 等      ✅ 真实生效
       └─ 诊断/跳转/符号/代码操作   → registerXxxProvider（其余）      ⏳ 注册了但不生效
```

### 12.1 四种安装来源，一个入口

`extensions_install(source)` 按字符串形状分派，前端只给一个输入框：

| 形状 | 判定 | 路径 |
| --- | --- | --- |
| 已存在的文件 | `Path::exists()` | `install_vsix` |
| `https://…/x.vsix` | `http(s)://` 且以 `.vsix` 结尾 | 下载 → `install_vsix_bytes` |
| `sdras.night-owl` | `publisher.name[@ver]` | `install_online`（Open VSX） |
| `github:o/r` · `o/r` · `git@…` | 含 `/` 或是 git URL | `install_git` |

Open VSX 是 VS Code Marketplace 的开源镜像，接口很小：`GET https://open-vsx.org/api/{publisher}/{name}/latest` 返回 `files.download`，拿到 `.vsix` 直链再走同一条解包路径。不引 Marketplace 的私有协议。

### 12.2 自带 ZIP 读取器，而不是拉一个解压依赖

`.vsix` 就是 ZIP。`extensions::zip` 只做两件事：走 EOCD 找回中央目录、按 method 解出条目（0 = store，8 = deflate，走已经在依赖树里的 `flate2`）。这样：

- 不多一个解压 crate（`zip` 会连带 `bzip2`/`zstd`/`time` 一堆）
- 解包过程可以顺手做安全检查：
  - 剥掉 VSIX 惯例的 `extension/` 前缀
  - `safe_rel()` 拒绝绝对路径、盘符、`..` 段（防 zip-slip）
  - `MAX_FILES = 20_000`、`MAX_TOTAL_BYTES = 512 MB`

### 12.3 为什么 GitHub 仓库不 `git clone`

最初 `install_git` 就是 `git clone --depth 1`，然后在这台机器上失败了：

```
fatal: unable to access 'https://github.com/…': SSL certificate problem:
       unable to get local issuer certificate (certificate signer not trusted)
```

原因是两套 CA：Windows 上 `git` 用自带的 OpenSSL bundle，会做 TLS 拦截的代理不在里面；而应用内的 `reqwest` 用的是 `default-tls`（schannel），信任系统证书库，同一条网络是通的。所以改成：

```
GET https://codeload.github.com/{owner}/{repo}/zip/HEAD
  → 用 12.2 的 ZIP 读取器解开
  → 剥掉 `<repo>-HEAD/` 第一段
  → finish_repo()
```

只有非 GitHub 的仓库才回退到 `git clone`，并补 `-c http.sslBackend=schannel` 让 git 也走系统证书。

### 12.4 贡献点怎么落到界面上

**主题**：`theme_colors(id, label)` 把 VS Code 的颜色键按「一个变量多个候选键」映射，命中第一个就写：

```rust
set("--bg-0",  &["editor.background", "editorGroup.background"]);
set("--bg-1",  &["sideBar.background", "activityBar.background"]);
set("--bg-2",  &["editorWidget.background", "panel.background"]);
set("--border", &["panel.border", "sideBar.border", "editorGroup.border"]);
set("--text-0", &["editor.foreground", "foreground"]);
set("--accent", &["focusBorder", "button.background", "activityBarBadge.background"]);
```

前端 `applyExtensionTheme(vars)` 依据 `--bg-0` 的亮度决定 `data-theme`（深/浅），再派生 `--accent-soft` 与对比色。扩展主题**不写盘**：它是预览，换个主题或重启就还原，避免用户被一个陌生配色永久锁死。

**代码片段**：`snippets_for(language)` 读 `contributes.snippets` 指向的 JSON（`strip_jsonc()` 先剥掉 `//` 注释，VS Code 允许），产出 `{prefix, body, description}`，最多 400 条。前端注册为 CodeMirror 的 `autocompletion({ override: [snippetSource] })`，语言 id 通过 `VS_LANG` 表把本项目的 `ts/tsx/py/…` 映射成 VS Code 的 `typescript/typescriptreact/python/…`。插入时 `expandSnippet()` 展开占位符：`${1|a,b|}` 取首选项、`${1:label}` 取 label、裸 `${n}` 直接删掉——不引入 `LSP` 的 snippet 语法解析器。

**DSH 包**：很多「技能包」仓库其实就是一个 `SKILL.md`。安装时 `find_skills()` 在 ≤4 层内找所有 `SKILL.md`（跳过 `node_modules/.git/target/dist`），逐个 `registry.install_from_text()` 进技能系统。所以「装扩展」和「装技能」是同一次操作，不需要用户再走一遍技能面板。

### 12.5 存储与状态

```
<config>/extensions/<publisher.name>-<version>/
  ├─ package.json            ← VSIX 原样
  ├─ themes/*.json …
  └─ .irixen-ext.json        ← sidecar：来源、安装时间、启用状态、warning
```

`list()` 是目录扫描 + 读 sidecar，不做数据库。启用/停用只翻 sidecar 的一个布尔值；主题下拉、片段查询、`extensions:count` 事件都据此过滤。装完 emit 一次 `extensions:count`，编辑器据此清空片段缓存（`resetSnippetCache()`），否则换了扩展还得重启才看到新片段。

## 13. Node 扩展宿主

`src-tauri/src/exthost/` 负责让 `extension.js` 真的跑起来。设计目标只有一个：**扩展崩溃不能带崩 IDE，扩展卡住不能让 IDE 卡住**。后者比前者难，因为 `showInformationMessage` 这类 API 天生是阻塞的。

### 13.1 一个扩展一个 `node` 进程

```
ExtHost (Rust, 单例)
  ├─ Inner.hosts: HashMap<extId, Host{ tx, child, status }>
  └─ 每个 Host: std::process::Child
       ├─ stdin   ← 帧（core → 宿主）
       ├─ stdout  → 帧（宿主 → core）
       └─ stderr  → 日志环，前缀 "err"
```

不共享进程有三个现实理由：扩展的 `require` 缓存互不可见（VS Code 也是隔离的）；一个扩展 `while(true)` 只烧自己一个核；`process.exit()` 只杀自己。代价是内存（每个 Node 约 40 MB），所以宿主**按需启动**——装了不跑不占内存。

### 13.2 帧格式：NDJSON，stdout 就是协议

```jsonc
{ "t":"req",  "id":7, "method":"window.message", "params":{…} }  // 宿主 → core，要回
{ "t":"res",  "id":7, "result":"Details" }                        // core → 宿主
{ "t":"rreq", "id":3, "method":"executeCommand", "params":{…} }   // core → 宿主，要回
{ "t":"rres", "id":3, "result":null }                             // 宿主 → core
```

`id: 0` 表示「发了就不等回」（`log`、`output`、`commands.put`、`statusbar` 全走这条），省掉一半往返。

stdout 被协议独占，所以 `bootstrap.js` 启动第一件事就是把扩展的写入改道：

```js
process.stdout.write = (chunk, enc, cb) => process.stderr.write(chunk, enc, cb);
console.log = console.info = … = toStderr;   // 连 util.inspect 都保留
```

否则任何一句 `console.log` 都会把下一帧的 JSON 切碎，`JSON.parse` 失败后整条通道静默失效——这是最容易踩且最难查的一类 bug，所以选择在源头堵死而不是在解析端容错。

### 13.3 `vscode` 模块：一次加载，两种写法

扩展有 CommonJS 也有 ESM，必须让两者拿到**同一个**单例：

```js
const originalLoad = Module._load;
Module._load = function (request) {
  if (request === 'vscode') return vscode;
  return originalLoad.apply(this, arguments);
};
```

CJS 走 hook 就够了；ESM 的 `import { commands } from 'vscode'` 走 Node 的解析器，hook 管不着。所以启动时还会在 `<config>/node_modules/vscode/` 写一个真实包：

```js
// index.js —— 具名导出必须逐个写出来
const api = global.__IRIXEN_VSCODE__ || {};
module.exports = api;
module.exports.commands = api.commands;
module.exports.window = api.window;
…
```

`node_modules/vscode` 放在 `<config>/` 而不是扩展目录，是因为 Node 的解析会从 `<config>/extensions/<id>/` 一路往上找，一份 shim 服务所有扩展。**具名导出不能省**：`cjs-module-lexer` 是静态分析，看不到 `module.exports = api` 里有什么，只有写成 `module.exports.commands = …` 才能让 `import { commands }` 通过链接期校验。

### 13.4 两套 id，别混用

| 表 | 键 | 谁等待 | 超时 |
| --- | --- | --- | --- |
| `rev_waiters` | core 自己的 `rev_seq` | `exthost_run` 等命令返回值 | 60 s |
| `ui_waiters` | `ui_seq`（发给渲染进程的 id） | 扩展等用户点按钮 | 无（用户可能一直不点） |

`ui_waiters` 的键和回帧里的 `id` **不是同一个数**，这是踩过的坑：

```rust
struct UiWaiter {
    ext: String,
    req: u64,          // 宿主自己的请求号，回帧必须用它
    tx: Sender<String>,
}
```

渲染进程需要跨扩展唯一的 id（`ui_seq`），而宿主只认自己那个请求号（`req`）。早期图省事用一个数，结果只要扩展在两次弹窗之间发过任何别的请求（比如 `globalState.update`），编号就错位，回帧被宿主当成「不认识的 id」丢掉——表现是**弹窗点得动、但 `await` 永远不返回**，扩展从此静默卡死。现在两个数分开存：`ui_seq` 保证渲染进程侧跨扩展唯一，回帧用 `req` 还原成宿主自己的请求号。

宿主进程退出时 `drop_waiters()` 清掉它名下的等待者：没人等的弹窗不该留在表里。

### 13.5 阻塞式 UI：等待者表 + 渲染进程代答

`showInformationMessage` / `showQuickPick` / `showInputBox` 在 VS Code 里是返回 Promise 的，Promise 的 resolve 时机由用户决定。落地方式：

```
扩展 await showInformationMessage(msg)
  → req window.message
  → core: ui_seq++ → ui_waiters 登记 → emit "exthost://ui"
  → 渲染进程 answerPrompt()：渲染弹窗
  → 用户点按钮 → api.exthostReply(id, value)
  → core: reply_ui() 取走等待者 → 往宿主 stdin 写 res
  → 扩展的 Promise 落地
```

`answerPrompt()` 的每条路径（按钮、Esc、点遮罩）都必须**恰好回一次**，漏回 = Promise 永不落地；重复回 = 打到已取走的 id 上被静默忽略。

两个语义细节：

- **`quickPick` 回传的是索引，不是标签。** 渲染进程只认识 `label/description/detail`，但扩展期望拿回自己塞进去的对象（`{ label, value: 42 }`）。所以 UI 回 `number | number[]`，宿主侧 `list[sel]` 映射回原对象；同理 `canPickMany` 回数组。
- **`messageBox` 没有按钮时补一个「关闭」**，返回 `undefined` —— 对应 VS Code 的「弹窗被关掉」。

### 13.6 主线程不能等

Tauri 的命令默认在主线程执行。`exthost_run` 要等扩展返回值（最长 60 s），`exthost_start` 要等进程 spawn + `ready` 帧，写在主线程上就是整个窗口假死。所以凡是有可能阻塞的命令一律：

```rust
tauri::async_runtime::spawn_blocking(move || host.reverse(&id, "executeCommand", args))
```

### 13.7 文档镜像：编辑器状态是唯一的真相

扩展的 `activeTextEditor` 不是它自己维护的，是从 IDE 镜像过去的：

```
editor open/change/save/close
  → exthost_doc(kind, doc)            // 全量 { path, text, languageId, dirty }
  → 每个 running 宿主收到 rreq "doc"
```

- **220 ms 防抖**：连续敲字不能每键一帧。
- **没有宿主时不发**：`doc_event` 先查 `running` 状态，避免为没人听的通道序列化几 MB 文本。
- **全量而非增量**：扩展侧 `TextDocument` 直接按文本重建，`getText()`、`offsetAt`、`lineAt` 全部现算。增量同步省的那点带宽，换来的是偏移量对不齐的整类 bug。

扩展改文档走反方向：`editor.edit(builder)` → `rreq "editor.edit"` → `exthost://edit` → 渲染进程把 `{start,end,text}` 换成偏移量，自下而上一次性 `view.dispatch`（自下而上是为了让前面的编辑不移动后面还没用的偏移量）。

### 13.8 Node 运行时自举

不假设用户装了 Node（IDE 的目标用户里有一半没有）。`exthost_install_node`：

```
GET https://nodejs.org/dist/index.json   → 取第一个 lts 非空且 win-x64 有包的版本
GET …/node-v{ver}-win-x64.zip            → 内存里解开
  → §12.2 的 ZIP 读取器只挑 node.exe（+ LICENSE）
  → <config>/runtime/node/node.exe
```

复用 §12.2 那个 ZIP 读取器是刻意的：它已经在 `flate2` 上，不必为「下载 Node」再引一个解压库。检测顺序是 配置路径 → 自带运行时 → `PATH` → `%ProgramFiles%\nodejs\node.exe`，`node -v` 拿版本号，拿不到就当没装，界面上给一条横幅而不是让宿主启动失败。

### 13.9 生命周期

`extensions.autostart` 为真的扩展在启动后 `autostart_delay_ms`（默认 1500 ms）自动起宿主——晚于首屏，不抢启动时间。窗口关闭走：

```rust
.build(…).run(|app, event| {
    if let RunEvent::ExitRequested { .. } = event { host.stop_all(); }
});
```

没有这一步，`node.exe` 会留在任务管理器里——开发期尤其致命，几次热重启就堆出一屏孤儿进程，还都占着 `<config>/runtime` 的日志文件。

### 13.10 已实现的 API 面

| 分组 | 已实现 |
| --- | --- |
| `commands` | `registerCommand`、`executeCommand`（含跨扩展跳转）、`getCommands` |
| `window` | `showInformationMessage`/`showWarning`/`showError`、`showQuickPick`、`showInputBox`、`createOutputChannel`、`createStatusBarItem`、`activeTextEditor`、`showTextDocument`、`withProgress`（直通） |
| `workspace` | `rootPath`、`workspaceFolders`、`fs.readDirectory/readFile/writeFile/stat`、`getConfiguration`、`onDidChangeTextDocument` |
| `env` | `appName`、`language`、`machineId`、`openExternal` |
| 文档/编辑 | `TextDocument`（`getText`/`lineAt`/`offsetAt`/`positionAt`/`lineCount`）、`TextEditor.edit`、`workspace.applyEdit` |
| 类型 | `Uri`、`Position`、`Range`、`Selection`、`Location`、`Disposable`、`EventEmitter`、`ThemeIcon`、`MarkdownString`、`StatusBarAlignment`、`ViewColumn` |
| `context` | `globalState`/`workspaceState`（落 `<扩展目录>/.state/ext-state.json`）、`extensionPath`、`asAbsolutePath`、`logPath`、`storageUri` |
| `languages` | `registerCompletionItemProvider`、`registerHoverProvider`、`registerDocumentFormattingEditProvider`、`registerDocumentRangeFormattingEditProvider`——四个都接到 CodeMirror；请求有 4.5 s 超时与序号丢弃，宿主卡住不会冻结编辑器 |
| TreeView | `registerTreeDataProvider`、`createTreeView`（子节点按需向扩展要） |
| Webview | `createWebviewPanel`、`registerWebviewViewProvider`、`postMessage` / `onDidReceiveMessage`（面板页走 `irixen-wv` 自定义协议） |
| 未实现 | 诊断集合（`createDiagnosticCollection` 收了但扔掉）、定义跳转、引用查找、代码操作、文档/工作区符号、CodeLens、重命名、语义着色、`Task`/`Terminal` API |

**别让用户自己猜**：已安装扩展的详情页有一节「用到的 API」，`extensions::scan_api_usage` 扫扩展入口里真实出现过的调用点，未实现的能力标红并给出说明。装完发现某个功能没反应时，先看那里——它区分得了「这个扩展有问题」和「这个宿主没实现这个 API」。

未实现的 API 一律**存在但是抛 `not implemented`**，而不是 `undefined`——`if (vscode.window.createTreeView)` 这种特性探测拿到 `undefined` 会让扩展悄悄降级，拿到能抛错的函数至少日志里有名字。

## 14. 内置插件商店

面板有两个页签：**已安装**、**商店**。商店直连 Open VSX，不开浏览器。

### 14.1 只用一个 API，三条请求

```
GET /-/search?query=&category=&sortBy=&size=24&offset=N
GET /api/{publisher}/{name}          → 详情：README、版本历史、评分、仓库
GET /api/{publisher}/{name}/{ver}    → 安装时拿 files.download
```

`sortBy` 支持 `downloadCount`（默认）/`averageRating`/`timestamp`，与 `relevance`（有 query 时）。分类用 Open VSX 自己的字符串（`Programming Languages`、`Themes`…），不做本地映射表——它改名了我们跟着变，比翻译一份对不上的分类便宜。

### 14.2 README 要洗一遍

详情页的 README 是原始 Markdown，里面塞满了徽章、相对路径图片、以及 `<picture>`/`<div align>` 这类 HTML。直接丢给项目的 `renderMarkdown` 会渲染出几十个碎图标和一堆空行：

```ts
md.replace(/<img[^>]*>/gi, '')        // 徽章
  .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')  // 保留 alt 文字，去掉图
  .replace(/<[^>]+>/g, '')            // 其余 HTML 标签
```

保留 alt 文字是为了「下载量 1M+」这类信息还看得见。README 在 Rust 侧截到 60 kB——最长的几个（`ms-python.python`）有 200 kB 以上，整篇塞进 IPC 会让详情页白屏半秒。

### 14.3 装完就能跑

商店的「安装」和面板上的输入框走**同一个** `extensions_install`。区别只有一处：商店知道确切版本，所以直接 `/api/{pub}/{name}/{ver}`，避免 `latest` 跟用户看到的版本号不一致。

装完刷新列表；若该扩展有 `main`，卡片上直接出现宿主条与「启动宿主」，命令面板（`Ctrl+Shift+P`）里同步出现它的命令。整条链路没有「重启生效」这一步。

## 15. 面板交互与报错主题：两条 UI 约定

### 15.1 `hidden` 属性不是保险

插件面板踩过一次「整页点不动」：`.store` 是 `position: absolute; inset: 0; display: flex`，`.ext-body` / `.ext-detail` 也都是 `display: flex`。`hidden` 只在 **UA 样式表**里声明 `display: none`，作者样式表里任何一条 `display` 都会以更高优先级赢下来，于是：

```
已安装页：storeEl.hidden = true，但它仍然铺满 panel，盖在卡片上吞掉所有点击
商店页：  listEl.hidden = true，已安装列表仍在下面绘制
```

用构建产物里的真实 CSS 做命中测试（headless Edge + `elementFromPoint`）能量化这个 bug：去掉全局规则时 `store` 计算样式为 `flex`、尺寸 756×488，「输出」「停用」两个按钮的命中目标都是 `store-list`；加上规则后两者命中自身。

结论是 `base.css` 里一条全局规则，而不是给每个组件补 `.xxx[hidden]` 变体：

```css
[hidden] { display: none !important; }
```

项目里所有 `hidden` 用法（`dock` / `editor-area` / `bridge` 的 provider 行 / 扩展面板）都是真隐藏，没有「挂着 hidden 但靠 CSS 显示」的例外，所以这条规则没有副作用。**任何新增的、带 `display` 的全屏层都必须靠 `hidden` 切换可见性**，不要再写 `style.display`。

同一轮还把 `.toasts` 容器设成 `pointer-events: none`（单个 toast 恢复 `auto`）：右下角的通知堆恰好压在按钮上时，也不该让整个界面「点了没反应」。

### 15.2 报错颜色必须来自语义变量

错误提示原来散着几处写死的 rgba：聊天气泡用 `rgba(248,113,113,…)`（暗色主题的红），扩展弹窗徽章用 `rgba(220,38,38,…)`（亮色主题的红）。结果是切主题后，一半的报错组件仍停留在另一种配色里，且自定义主题覆盖 `--err` 时完全跟不上。

现在 `base.css` 定义 `--ok-soft` / `--warn-soft` / `--err-soft` / `--err-line`，亮暗两套各有一份字面量兜底；支持 `color-mix()` 的 WebView2 再用当前 `--err`/`--warn`/`--ok` 现算，所以自定义主题改了 `--err`，所有错误面会一起变：

```css
@supports (color: color-mix(in srgb, red 10%, transparent)) {
  :root, :root[data-theme='dark'] {
    --err-soft: color-mix(in srgb, var(--err) 11%, transparent);
    --err-line: color-mix(in srgb, var(--err) 40%, transparent);
  }
}
```

用到它们的组件：错误/成功 toast、行内错误块、聊天错误气泡、`probe-err`、扩展弹窗的 severity 徽章、Node 缺失横幅。徽章里的 `✕/!/i` 文本字形也换成了 SVG 图标（`alert` / `info`，继承 `currentColor`）——`✕` 本来更像关闭按钮而不是「出错了」。

一个容易漏的细节：toast 浮在编辑器内容上方，`--err-soft` 是半透明的，直接当 `background` 会让文字和底下的代码糊在一起。所以 toast 保留不透明的 `--bg-3` 底，再叠一层语义色：

```css
background: linear-gradient(var(--err-soft), var(--err-soft)), var(--bg-3);
```

面板内的错误（搜索失败、列表加载失败、日志读取失败）不再用一闪而过的 toast，而是留在用户正在看的位置，用带「重试」按钮的行内错误块；标题全部走 i18n，8 种语言都有对应键。

## 16. 端到端实测：装一个真正的 Open VSX 扩展

前面所有验证都用的是手工造的 smoke 扩展。最后一轮换成 `esbenp.prettier-vscode` 12.4.0（2 MB VSIX、`"type": "module"`、ESM 打包、35 个配置项），从商店搜索一路点到命令执行，暴露了 5 个只在真实生态里才会遇到的 bug。

**① `sortBy=averageRating` 是错的。** Open VSX 只接受 `relevance | timestamp | rating | downloadCount`，`averageRating` 是**响应字段**名不是请求参数。发过去直接 HTTP 400 → 按评分排序得到空列表，之后所有请求都带着这个错误参数，连详情都打不开。改成 `"rating" => "rating"`。

**② 并发搜索互相覆盖。** 切到商店页会发一次默认查询，用户紧接着打字会发第二次；慢的那个后到，把结果写回列表，表现是「搜 prettier 出来 Python」。`loadStore()` 现在带 `storeSeq` 序号，只有最后一次请求的响应可以写 DOM；`openDetail()` 同理加 `detailSeq`。详情页安装成功还要调 `markInstalled()` 同步左侧行的「已安装」标签。

**③ ESM 具名导出的链接期崩溃。** 报错是 `SyntaxError: Named export 'CodeAction' not found`。`cjs-module-lexer` 只认**静态**的 `module.exports.X = ...` 行，Proxy 造不出具名导出。现在 `<config>/node_modules/vscode/index.js` 是**生成**的：一张 VS Code 公共 API 名单 ∪ 扫描扩展入口里 `import { … } from 'vscode'` 得到的名字 ∪ 已有 shim 里的名字（所以文件单调增长，多个宿主同时启动也不会互相删名字）。未实现的名字给一个宽松 `stub()`（可属性访问、可调用、可 `new`，刻意不 thenable），让扩展先过链接期，真要用某个 provider 时再明确报错。

**④ `CodeActionKind` 只有两个占位键。** Prettier 在模块顶层就算 `CodeActionKind.SourceFixAll.append('prettier')`，占位对象没有 `SourceFixAll` → `Cannot read properties of undefined`。补成真的类：`Empty/QuickFix/Refactor*/Source*/Notebook`，带 `append()/contains()/intersects()/value`。

**⑤ `getConfiguration` 的缓存从来没被写过。** `read()` 拿到异步结果就丢了，`.get()` 永远返回调用方给的默认值；而且真实扩展常用 `const { enable } = workspace.getConfiguration('prettier')` 这种**属性访问**，我们返回的普通对象上根本没有 `enable`。现在改成 Proxy（未知属性映射到配置项），并优先读本扩展 `contributes.configuration.properties[key].default`；`workspace.isTrusted` 默认 `true`，否则 prettier 会走「不信任」分支把配置对象摊平、丢掉所有属性。`languages` 也加了兜底代理：未实现的 provider 类方法统一返回可写的 `Disposable`，扩展在 `activate()` 里注册一个 formatter 不该因为「not a function」整个崩掉。

**实测结果**：搜索 → 分类（Themes）→ 排序（评分）→ 详情（15 kB README、40 个版本、无碎图/无横向溢出、`JS` 标记由 `files.manifest` 的 `main` 判定）→ 安装 → 已安装卡片出现宿主条 → 启动 → `activate()` 成功、输出通道打印扩展名与版本 → 3 个命令 `live: true` → 点 `prettier.openOutput` 返回 `✓`。全程没有重启 IDE。

仍然明确不做：语言服务类 provider 的真实实现（补全/诊断/跳转）、Webview、TreeView、Task/Terminal。§13.10 的表格不变——「能注册、不崩」和「真的提供补全」是两件事。
