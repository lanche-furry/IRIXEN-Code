# Architecture

<p align="center">
  <strong>English</strong> &nbsp;|&nbsp; <a href="./ARCHITECTURE.md">中文</a>
</p>

IRIXEN Code has exactly two layers: a **Rust process** (the IRIXEN Agent plus system capabilities) and a **WebView renderer** (the IRIXEN Code UI). They talk over Tauri IPC — no HTTP middle layer, no local service port.

```
┌───────────────────────────────── WebView2 (renderer) ───────────────────────────────┐
│  src/core/      dom · i18n · api · state (one hand-written event bus)               │
│  src/ui/        shell · editor · explorer · search · git · skills ·                 │
│                 stats · chat · review · settings · theme ·                         │
│                 markdown · toast                                                    │
│  src/styles/    base.css (design tokens) · app.css (layout)                         │
└──────────────────────────────────────────┬──────────────────────────────────────────┘
  invoke()                              │  listen('agent://event')
┌──────────────────────────────────────────┴──────────────────────────────────────────┐
│  commands.rs    <- every #[tauri::command]; thin: forward args, map errors          │
│  state.rs       AppState { config, engine, skills, mcp }                            │
│  agent/                                                                             │
│    provider.rs  LlmProvider trait + OpenAi/Anthropic/Gemini implementations         │
│    models.rs    model catalog + thinking inference + 10-minute disk cache           │
│    context.rs   Compressor: elision / window / digest                               │
│    tools.rs     8 native tools (path sandbox inside the workspace)                  │
│    meter.rs     UsageMeter: per model / per session                                 │
│    mod.rs       AgentEngine: sessions, turn loop, event emission                    │
│  mcp.rs         McpHost / McpConn: JSON-RPC over stdio                              │
│  skills.rs      SkillRegistry: compiled-in built-ins + user installs                │
│  git.rs         shells out to the system git                                        │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## 1. Window lifecycle

There is a single `main` window, declared `visible: true` in `tauri.conf.json`, with a `backgroundColor` matching the light theme's `--bg-0`.

The startup path is deliberately as short as possible: **no splash window, no intro animation, no "wait until the UI is ready" gate**.

1. The process starts → the window appears immediately on `#eaedf2` (under a dark theme, the inline script in `index.html` swaps it to `#0a0d13` on the first frame after parsing)
2. The front end finishes `mountShell()` → calls `main_loaded`
3. `lib.rs::mark_main_loaded` does two things: pulls focus back to the main window (another window may have taken it during `vite`'s cold start) and logs the mount time

Window visibility depends on no IPC at all, so even if the front end crashes during startup the user sees a visible window plus a devtools error — never "a process is running with no window".

## 2. One abstraction over three protocols

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

`chat()` returns a `StreamResult`: a normalised stream of `StreamEvent` plus the final `Usage`.

```rust
enum StreamEvent {
    Text(String),
    Reasoning(String),
    ToolCallStart { id, name },
    ToolCallArgs { id, delta },
    Usage(Usage),
}
```

Each of the three implementations translates its vendor format into these events, and `AgentEngine` only understands these events — so adding a vendor means adding one `impl` and touching no engine code.

| | OpenAI | Anthropic | Gemini |
| --- | --- | --- | --- |
| Endpoint | `POST {base}/chat/completions` | `POST {base}/messages` | `POST {base}/models/{m}:streamGenerateContent?alt=sse` |
| Auth | `Authorization: Bearer` | `x-api-key` + `anthropic-version` | `x-goog-api-key` |
| Thinking | `reasoning_effort` | `thinking.budget_tokens` | `generationConfig.thinkingConfig.thinkingBudget` |
| Tools | `tools[].function` | `tools[].input_schema` | `tools[].functionDeclarations` |
| Tool results | `role: tool` | `tool_result` block | `functionResponse` part |

`extra_body` is merged into the outermost request body and `extra_headers` into the headers — that is how vendor quirks such as Qwen's `enable_thinking` or a relay's custom fields are handled without code changes.

### Mapping thinking effort

`Effort { Auto, Off, Minimal, Low, Medium, High }` is the core's single enum, translated only when it hits a protocol:

- OpenAI (chat/completions): top-level `reasoning_effort`, with `off → "none"`. Measured against deepseek-flash / v4-pro, which both accept the field; `none` drives `reasoning_tokens` to 0, so it is a real switch rather than a hint
- Anthropic: `budget_tokens` of 1024 / 2048 / 8192 / 16384 by level, and `max_tokens > budget` is mandatory
- Gemini: `thinkingBudget` of 0 / 1024 / 4096 / 12288
- `Auto` is never sent; the vendor default applies

When a vendor does not know the field it answers 400/422; `provider.rs::rejects_thinking()` resends exactly once — without the field — only when the status is 400/422 **and** the body mentions reasoning/thinking/unknown field. A 401/429/500 is never retried. That keeps the cost of "picking a level on a model that does not support it" at one request instead of one turn.

### The level is sticky

- Per session: `agent_set_model(effort)` writes it into the session
- Global default: `set_default_effort(effort)` writes `config.effort`; new sessions start from it and it survives a restart
- The level picker in the chat panel updates both; `Settings → Agent → Default thinking effort` updates only the latter

## 3. Model catalog and thinking capability

Three layers of truth, highest priority first:

1. **fetched** — what a real `list_models()` call returned
2. **heuristic** — inferred from naming rules (`models.rs::detect_thinking`)
3. **cached** — `model-cache.json`, 10-minute TTL

The `detect_thinking()` rule table:

| Model name matches | ThinkingSupport |
| --- | --- |
| `deepseek*` (except `*reasoner*` / `*-r1*`) | `Effort { levels: [minimal, low, medium, high] }` |
| `gpt-5*` `o1*` `o3*` `o4*` | `Effort { levels: [minimal, low, medium, high] }` |
| `glm-z1*` `glm-4.5*` `glm-4.6*` | `Effort { levels: [low, high] }` |
| `*qwq*` `*thinking*` | `Budget { min: 0, max: 32768 }` |
| `claude-3-7*` `claude-*-4*` | `Budget { min: 1024, max: 32000 }` |
| `gemini-2.5*` `gemini-3*` | `Budget { min: 0, max: 24576 }` |
| `deepseek*reasoner*` `*-r1*` | `AlwaysOn` |
| anything else | `Unsupported` |

A cache hit still goes through `ModelInfo::refresh_thinking()`: capability is *derived from the id*, so a fixed rule takes effect on old cache entries immediately, without waiting for the TTL.

The front end's `effortChoices()` **still offers the full range** for `unsupported` / `unknown`, because plenty of OpenAI-compatible vendors quietly accept `reasoning_effort`; the retry above covers the ones that really do not. Only `always_on` (such as `deepseek-reasoner`) degrades to `auto` alone — that model's thinking cannot be switched off, and offering levels would be fake interaction.

## 4. The tool loop

`AgentEngine::run_turn()` is a `loop`:

```
while iteration < max_iterations:
    1. compression check (maybe_compress)
    2. assemble the request (system + injected skills + history + tool table)
    3. stream the request, emitting as it goes (delta / reasoning / tool_call)
    4. if the model returns tool_calls:
         - native tool -> tools::execute_native()
         - mcp__* tool -> McpHost::call()
         push the result into the messages, iteration += 1, continue
    5. otherwise emit(done) and stop
```

Every tool call emits `tool_call` (start) and `tool_result` (end), which is what the UI draws its status-dotted tool cards from.

**Path resolution**: `tools::resolve()` joins every relative path onto the workspace root, then `canonicalize`s and compares prefixes; whether an escape is allowed at all is the current access mode's call (below). `run_command` executes with the workspace as cwd, times out at 120 s, and truncates output at 18k characters.

### Access modes: three steps, decided by a pure function

`access.rs` touches no IO and no state: `decide(mode, tool, args, workspace) -> Allow | Ask(Request) | Deny(String)`, so the whole rule set is pure logic the unit tests can hold down. Three steps:

| Mode | Read inside | Write inside | Outside | `run_command` |
| --- | --- | --- | --- | --- |
| Restricted | allowed | asks every time | **denied**, not asked | asks every time |
| Auto (default) | allowed | allowed | asks | asks only when cwd is outside |
| Full | allowed | allowed | allowed | allowed |

Three deliberate choices:

- **Restricted denies an escape instead of asking about it.** An ask that can be answered yes once is an unlimited verbal licence, indistinguishable from auto; restricted has to mean the road is not there, not that someone is standing in front of it.
- **`run_command` does not parse the shell string.** There are as many ways to look wrong at `sh -c` as there are ways to sneak past it, and a permission system that reads a shell string and therefore believes it knows what the command will touch is more dangerous than one that admits it cannot see. So commands always ask under restricted, and under auto they ask only when the cwd leaves the workspace.
- **`full` has to be a decision.** A config without the field reads as `auto`; picking `full` in the UI says once that nothing will be intercepted after this.

An ask is a real suspension: `ask_approval()` parks a `oneshot::Sender` in the `pending` map, emits an `approval` event carrying an id, and the UI draws a card into the transcript; allow or deny is what calls `send(bool)` and wakes the `execute_tool` waiting on it. No answer within fifteen minutes counts as a denial, and `agent_stop` denies everything still pending for that session at once — a stopped turn must not stay in memory holding senders. What the model gets back after a denial is "the user rejected this call" with `is_error = true`, so it looks for another route instead of concluding the tool is broken.

The mode also goes into the system prompt, followed by "this is the user's choice, you cannot change it": a model that knows where the fence is does not spend three rounds walking into it.

**`security_scan` and `audit.rs`**: the only tool that walks the whole tree, so it runs on `spawn_blocking` — a six-thousand-file sweep must not hold the async thread every other event is queued on. `audit.rs` walks with `ignore`, deliberately `hidden(false)`: `.env` and `.github/workflows` are exactly what an audit is for. The rule table is fixed (cloud credentials, private-key blocks, JWTs, secrets assigned a literal, disabled TLS verification, dynamic execution, SQL built by interpolation, world-writable files, plus two path rules for key material and CI actions not pinned to a SHA). A hit is masked before it becomes evidence: the matched span turns into "4-character head + *** + 3-character tail", so the model sees `AKIA***F7Q2` rather than the key. The scanner only enumerates — whether a hit is exploitable is the model's reading job, and `render()` ends by telling it exactly that, plus that no hit is not the same as safe.

## 5. Context compression

`context.rs` has three stages, all guided by "keep the conclusions, drop the repetition":

**① Tool-result elision** (`trim_tool_results`)
Any tool result older than `tool_result_age` messages that exceeds `tool_result_cap` in length is replaced with `[elided N chars]` plus its first line.

**② Sliding window** (`split_window`)
Keep the most recent `keep_recent` messages; merge everything older into one "task anchor" system message that extracts the original goal (first user message, truncated), the decisions taken, and the file paths involved (pulled from the `path` field of tool-call arguments, de-duplicated).

**③ LLM digest** (`digest`)
When the estimated token count still exceeds `max_context_tokens * threshold`, the current model produces a structured summary:

```
Goal / Decisions / Files / State / Pending
```

`digest_max_tokens` bounds the length. **This stage holds no lock**: trim and clone the messages to digest while holding the lock, release it, then send — otherwise an `async` lock held across an `await` freezes the UI. If the request fails it falls back to `heuristic_digest` (pure rule-based assembly) so the flow never breaks.

When compression finishes it emits `compressed { tokens_before, tokens_after, tool_results_elided, messages_dropped, digest }`, which the UI shows as a system notice.

## 6. MCP

`mcp.rs` is a hand-written stdio client with no SDK dependency:

- `McpConn::spawn()`: starts the child process; stdin/stdout carry JSON-RPC 2.0 (one message per line), and stderr keeps the last 40 lines for UI troubleshooting
- Handshake: `initialize` (protocolVersion `2024-11-05`) → `notifications/initialized` → `tools/list`
- Calls: `tools/call { name, arguments }`, with the text parts of the content array joined
- Tool names get a uniform `mcp__{server}__{tool}` prefix on the engine side, sharing the pool with native tools so the model cannot tell the difference
- On Windows, if the command is a `.cmd` shim such as `npx` / `uvx` / `node`, it is wrapped in a shell automatically

## 7. Skills

```
src-tauri/src/skills/builtin/*.md     <- include_str! into the binary
~/.irixen-code/skills/<id>/SKILL.md   <- user installs
```

Frontmatter parsing is hand-written (no YAML crate):

```markdown
---
name: token-saver
description: 一句话说明
when: 省token, 太长, 精简, 冗长
---
正文（注入系统提示词的部分）
```

`match_for(prompt, auto)` does a case-insensitive substring match on the `when` keywords. Injection happens only on a hit — deliberately: cramming every skill into the system prompt is both expensive and distracting.

Three install sources: a local directory, a `http(s)` `SKILL.md`, or a Git repository (shallow clone, then find `SKILL.md`).

## 8. IPC command surface (excerpt)

| Group | Commands |
| --- | --- |
| Config | `get_config` `save_config` `app_info` `set_workspace` |
| Providers | `provider_upsert` `provider_delete` `provider_test` `models_list` |
| Sessions | `agent_new_session` `agent_set_active` `agent_messages` `agent_send` `agent_stop` `agent_clear` `agent_set_model` `agent_usage` |
| Access control | `set_agent_access` `agent_approve` |
| Files | `fs_list` `fs_read` `fs_write` `fs_create_file` `fs_create_dir` `fs_delete` `fs_rename` `fs_search` |
| Git | `git_is_repo` `git_status` `git_diff` `git_stage` `git_unstage` `git_commit` `git_branch(es)` `git_checkout` `git_log` `git_push` `git_pull` |
| MCP | `mcp_servers` `mcp_upsert` `mcp_delete` `mcp_start` `mcp_stop` `mcp_tools` |
| Skills | `skills_list` `skills_install` `skills_uninstall` `skills_toggle` `skills_read` |
| Extensions | `extensions_list` `extensions_install` `extensions_uninstall` `extensions_toggle` `extensions_search` `extensions_theme` `extensions_snippets` |
| Window | `main_loaded` `window_control` |

There is one event channel, `agent://event`, whose payload is an `AgentEvent` (serde `tag = "kind"`); the front end filters by `session`.

## 9. Layout contract

The shell has one hard rule: **exactly one focal area at a time**. The conversation is the main area; code is an **overlay**, not a third column.

```
.app        grid-template-columns: minmax(0, 1fr)   /* explicit single column; no implicit column may stretch it */
 └ .body    grid-template-columns: var(--act-w) var(--sidebar-w) 3px minmax(0,1fr) var(--rail-w)
    ├ .activity  56px (Files / Search / Git / Skills / Stats / Review, icon + short label)
    ├ .sidebar   264px (view name + view toolbar + content)
    ├ .resizer   3px
    ├ .stage     minmax(0, 1fr)
    │  ├ .dock[data-active=chat|review|extensions]          panels switch via data-active, never destroy-and-rebuild
    │  └ .editor-area  position:absolute; inset:0; z-index:3   <- covers the conversation
    └ .rail      40px (Chat / Extensions / Files / Terminal / Settings)
```

`.activity` and `.rail` are two different axes: the left is "what the sidebar is looking at" (`app.view`), the right is "what the middle is looking at" (`app.surface`). The review panel belongs to the latter, which is why opening review lights up two items on the left at once.

Three details were settled only after being burned; do not regress them:

1. **`1fr` must be written `minmax(0, 1fr)`.** `1fr` equals `minmax(auto, 1fr)` and accepts a child's min-content width; a single `white-space: nowrap` child without `min-width: 0` (the former offender: status-bar text) stretches the whole grid line to an impossible value and the layout looks broken. `.app` / `.body` / `.stage` all use `minmax(0, …)` and every level sets `overflow: hidden`.
2. **`.app` must declare `grid-template-columns` explicitly.** Leaving it empty creates an implicit column sized by the widest child — same failure.
3. **The status bar may hold labels, never content.** An `agent:done` event carries the whole answer; an early version called `setStatus(ev.text)` and pushed a 5000-character answer into the status bar. Three layers now guard it: call sites pass at most 24 characters, `setStatus()` truncates to 90, and CSS gives every `.sb-item` `min-width:0; overflow:hidden; text-overflow:ellipsis`.

There is also a non-CSS source of "fake freezes": **a synchronous Tauri `fn` command runs on the main thread** (the same thread as IPC and window events). `workspace_stats` walks the workspace and reads every file under 1.5 MB just to count lines, `fs_search` runs a full regex scan, `git_*` spawns a subprocess — make any of those synchronous and the window really does stop responding. Commands like these are now uniformly `async` + `spawn_blocking`, and the walker skips `node_modules` / `target` / `dist` (`commands.rs::walker()`). Measured: stats 97 ms, full-repo search 114 ms, longest frame gap 17 ms.

Panel switching uses `.dock[data-active]` + `display:none` instead of unmounting the DOM: scroll position in the conversation and editor tabs all depend on that to keep their state. Covering with the editor also **does not destroy** the dock, only hides it, so "back to conversation" is instant.

## 10. Front end

No framework, no virtual DOM:

- `h()` builds nodes, `clear()` empties them, and state changes repaint the affected region explicitly
- `app` in `state.ts` is the single mutable state object; `on/emit` is a 30-line pub/sub
- The conversation stream uses **incremental DOM**: each message id owns a `MsgView` and a `delta` event only touches the text node; during streaming it renders plain text (`white-space: pre-wrap`) and only runs markdown on `done` — avoiding repeated O(n²) parsing
- Theming is CSS variables: `applyTheme()` sets `data-theme` on `<html>` or overwrites variables directly; a custom theme additionally derives `--accent-soft` (accent at 14% alpha) and a contrast colour
- i18n is a flat dictionary with `{name}` interpolation across 8 languages. `zh-CN` uses `satisfies Dict` as the **single source of truth**; every other language is typed `Record<keyof typeof zhCN, string>` and individually `satisfies`-checked, so a missing key is a `tsc` error rather than a silent runtime fallback
- Switching language is **live**: `setLang()` emits `lang:changed` only when the value actually changes, and the shell's `relabel()` repaints the title bar, nav rail, sidebar header, right rail and status bar, then **remounts** dock panels from `app` state (the conversation log is rebuilt by `renderLog()` from `app.chat`). If a stream is in flight, the remount is deferred until `agent:busy=false` so a half-written answer is not torn away
- **The backend never sends sentences**: for text like "which parts of this extension are usable", Rust returns tokens only (`kind` + `parts: ["themes","snippets"]`) and the front end's dictionary decides the wording — otherwise Chinese sentences leak into the French UI

## 11. Extensions (VSC / DSH compatible)

Extensions come in two layers: **declarative contribution points** (11.1–11.5 in this section) and the **real Node extension host** (§12). The deciding factor is simply whether `package.json` has a `main`:

```
.vsix (ZIP)
  └─ extension/package.json
       ├─ contributes.themes      → colour keys → this project's CSS variables  ✅ really applied
       ├─ contributes.snippets    → CodeMirror autocompletion source            ✅ really applied
       ├─ contributes.languages   → extension → language id (highlighting/snippets) ✅ really applied
       ├─ contributes.commands    → panel/command palette; executable with main  ✅ really applied
       ├─ main: "./extension.js"  → the Node extension host (§12)                ✅ really runs
       ├─ completion/hover/format providers → registerXxxProvider (4)             ✅ really applied
       ├─ TreeView / Webview      → registerTreeDataProvider and friends          ✅ really applied
       └─ diagnostics/goto/symbols/actions → registerXxxProvider (the rest)       ⏳ registers, nothing happens
```

### 11.1 Four install sources, one entry point

`extensions_install(source)` dispatches on the shape of the string, so the front end needs just one input box:

| Shape | Test | Path |
| --- | --- | --- |
| An existing file | `Path::exists()` | `install_vsix` |
| `https://…/x.vsix` | `http(s)://` and ends with `.vsix` | download → `install_vsix_bytes` |
| `sdras.night-owl` | `publisher.name[@ver]` | `install_online` (Open VSX) |
| `github:o/r` · `o/r` · `git@…` | contains `/` or is a git URL | `install_git` |

Open VSX is the open-source mirror of the VS Code Marketplace and its API is tiny: `GET https://open-vsx.org/api/{publisher}/{name}/latest` returns `files.download`, and that `.vsix` URL feeds the same unpacking path. No Marketplace private protocol is involved.

### 11.2 A built-in ZIP reader instead of another dependency

A `.vsix` is a ZIP. `extensions::zip` does two things only: walk the EOCD to find the central directory, and extract entries by method (0 = store, 8 = deflate, via `flate2` which is already in the dependency tree). That means:

- one less decompression crate (`zip` drags in `bzip2`/`zstd`/`time` and more)
- safety checks come for free during extraction:
  - strip the conventional `extension/` prefix
  - `safe_rel()` rejects absolute paths, drive letters and `..` segments (zip-slip)
  - `MAX_FILES = 20_000`, `MAX_TOTAL_BYTES = 512 MB`

### 11.3 Why GitHub repositories are not `git clone`d

`install_git` originally *was* `git clone --depth 1`, and it failed on this machine:

```
fatal: unable to access 'https://github.com/…': SSL certificate problem:
       unable to get local issuer certificate (certificate signer not trusted)
```

The cause is two different CA stores: on Windows, `git` uses its own bundled OpenSSL set, which does not include the TLS-intercepting proxy, while the app's `reqwest` uses `default-tls` (schannel) and trusts the system store — the same network works there. So it became:

```
GET https://codeload.github.com/{owner}/{repo}/zip/HEAD
  → unpack with the ZIP reader from 12.2
  → strip the leading `<repo>-HEAD/` segment
  → finish_repo()
```

Only non-GitHub repositories fall back to `git clone`, with `-c http.sslBackend=schannel` added so git uses the system certificate store too.

### 11.4 How contribution points reach the UI

**Themes**: `theme_colors(id, label)` maps VS Code colour keys as "one variable, several candidate keys" and writes the first hit:

```rust
set("--bg-0",  &["editor.background", "editorGroup.background"]);
set("--bg-1",  &["sideBar.background", "activityBar.background"]);
set("--bg-2",  &["editorWidget.background", "panel.background"]);
set("--border", &["panel.border", "sideBar.border", "editorGroup.border"]);
set("--text-0", &["editor.foreground", "foreground"]);
set("--accent", &["focusBorder", "button.background", "activityBarBadge.background"]);
```

The front end's `applyExtensionTheme(vars)` picks `data-theme` (dark/light) from the brightness of `--bg-0`, then derives `--accent-soft` and a contrast colour. Extension themes are **never persisted**: this is a preview, switching themes or restarting restores yours, so nobody gets locked into a stranger's palette.

**Snippets**: `snippets_for(language)` reads the JSON that `contributes.snippets` points at (`strip_jsonc()` removes `//` comments first, which VS Code allows) and produces `{prefix, body, description}`, at most 400 entries. The front end registers it as CodeMirror's `autocompletion({ override: [snippetSource] })`, and the `VS_LANG` table maps this project's `ts/tsx/py/…` ids onto VS Code's `typescript/typescriptreact/python/…`. On insert, `expandSnippet()` expands placeholders: `${1|a,b|}` takes the first option, `${1:label}` takes the label, a bare `${n}` is deleted — no `LSP` snippet-syntax parser required.

**DSH packages**: many "skill pack" repositories are really just a `SKILL.md`. On install, `find_skills()` looks for every `SKILL.md` within 4 levels (skipping `node_modules/.git/target/dist`) and feeds each through `registry.install_from_text()` into the skill system. Installing an extension and installing a skill are therefore one action — no second trip through the skills panel.

### 11.5 Storage and state

```
<config>/extensions/<publisher.name>-<version>/
  ├─ package.json            <- the VSIX original
  ├─ themes/*.json …
  └─ .irixen-ext.json        <- sidecar: source, install time, enabled flag, warning
```

`list()` is a directory scan plus sidecar reads — no database. Enabling and disabling flips one boolean in the sidecar, which is what the theme dropdown, snippet lookup and `extensions:count` event all filter on. Installing emits `extensions:count` once, and the editor clears its snippet cache in response (`resetSnippetCache()`) — otherwise new snippets only appear after a restart.

## 12. The Node extension host

`src-tauri/src/exthost/` is what makes `extension.js` actually run. It has exactly one design goal: **a crashing extension must not take the IDE down, and a stuck extension must not make the IDE stuck**. The second half is harder, because APIs like `showInformationMessage` are blocking by nature.

### 12.1 One `node` process per extension

```
ExtHost (Rust, singleton)
  ├─ Inner.hosts: HashMap<extId, Host{ tx, child, status }>
  └─ each Host: std::process::Child
       ├─ stdin   <- frames (core → host)
       ├─ stdout  → frames (host → core)
       └─ stderr  → log ring, prefixed "err"
```

There are three practical reasons not to share a process: extensions' `require` caches stay invisible to each other (VS Code isolates them too); a `while(true)` burns one core of its own; and `process.exit()` kills only itself. The price is memory (~40 MB per Node), so hosts **start on demand** — installed but not running costs nothing.

### 12.2 Frame format: NDJSON, stdout *is* the protocol

```jsonc
{ "t":"req",  "id":7, "method":"window.message", "params":{…} }  // host → core, wants a reply
{ "t":"res",  "id":7, "result":"Details" }                        // core → host
{ "t":"rreq", "id":3, "method":"executeCommand", "params":{…} }   // core → host, wants a reply
{ "t":"rres", "id":3, "result":null }                             // host → core
```

`id: 0` means "fire and forget" (`log`, `output`, `commands.put`, `statusbar` all use it), which removes half the round trips.

stdout is owned by the protocol, so the first thing `bootstrap.js` does at startup is redirect the extension's writes:

```js
process.stdout.write = (chunk, enc, cb) => process.stderr.write(chunk, enc, cb);
console.log = console.info = … = toStderr;   // even util.inspect is preserved
```

Otherwise a single `console.log` shreds the next JSON frame, `JSON.parse` fails, and the whole channel goes silently dead — the easiest bug to create and the hardest to find, so it is blocked at the source rather than tolerated in the parser.

### 12.3 The `vscode` module: load once, two syntaxes

Extensions come as both CommonJS and ESM, and both must receive the **same** singleton:

```js
const originalLoad = Module._load;
Module._load = function (request) {
  if (request === 'vscode') return vscode;
  return originalLoad.apply(this, arguments);
};
```

The hook covers CJS; ESM's `import { commands } from 'vscode'` goes through Node's resolver, which the hook never sees. So startup also writes a real package into `<config>/node_modules/vscode/`:

```js
// index.js — every named export has to be spelled out
const api = global.__IRIXEN_VSCODE__ || {};
module.exports = api;
module.exports.commands = api.commands;
module.exports.window = api.window;
…
```

`node_modules/vscode` lives under `<config>/` rather than inside each extension because Node's resolution walks up from `<config>/extensions/<id>/`, so one shim serves every extension. **The named exports cannot be omitted**: `cjs-module-lexer` is static analysis and cannot see what is inside `module.exports = api`; only `module.exports.commands = …` lets `import { commands }` pass the linking check.

### 12.4 Two sets of ids — do not mix them

| Table | Key | Who waits | Timeout |
| --- | --- | --- | --- |
| `rev_waiters` | core's own `rev_seq` | `exthost_run` waiting for a command's return value | 60 s |
| `ui_waiters` | `ui_seq` (the id sent to the renderer) | the extension waiting for a user click | none (the user may never click) |

The key in `ui_waiters` and the `id` in the reply frame **are not the same number**, and that is a trap that was already stepped in:

```rust
struct UiWaiter {
    ext: String,
    req: u64,          // the host's own request id; the reply frame must use it
    tx: Sender<String>,
}
```

The renderer needs an id unique across extensions (`ui_seq`), while the host only recognises its own request id (`req`). An early shortcut used one number for both, so as soon as the extension sent any other request between two prompts (`globalState.update`, say) the numbering drifted and reply frames were dropped as "unknown id" — the symptom being **a prompt you could click, whose `await` never returned**, after which the extension sat silently dead. The two numbers are now stored separately: `ui_seq` guarantees renderer-side uniqueness, and the reply frame is translated back through `req`.

When a host process exits, `drop_waiters()` clears the entries under its name: a prompt nobody is waiting for should not stay in the table.

### 12.5 Blocking UI: waiter tables plus renderer-side answers

`showInformationMessage` / `showQuickPick` / `showInputBox` return promises in VS Code, and the user decides when they resolve. The implementation:

```
extension awaits showInformationMessage(msg)
  → req window.message
  → core: ui_seq++ → register in ui_waiters → emit "exthost://ui"
  → renderer answerPrompt(): draw the dialog
  → user clicks a button → api.exthostReply(id, value)
  → core: reply_ui() takes the waiter → writes a res frame to the host's stdin
  → the extension's promise settles
```

Every path through `answerPrompt()` (button, Esc, clicking the overlay) must reply **exactly once**: a missing reply means a promise that never settles, and a duplicate lands on an id that was already taken and is silently ignored.

Two semantic details:

- **`quickPick` returns an index, not a label.** The renderer only understands `label/description/detail`, but the extension expects back the object it put in (`{ label, value: 42 }`). So the UI returns `number | number[]` and the host maps it with `list[sel]`; likewise `canPickMany` returns an array.
- **`messageBox` gets an added "Close" when it has no buttons**, returning `undefined` — which matches VS Code's "the dialog was dismissed".

### 12.6 The main thread must never wait

Tauri commands run on the main thread by default. `exthost_run` waits for an extension's return value (up to 60 s) and `exthost_start` waits for the process spawn plus the `ready` frame; on the main thread that freezes the entire window. So anything that can block is uniformly:

```rust
tauri::async_runtime::spawn_blocking(move || host.reverse(&id, "executeCommand", args))
```

### 12.7 Document mirroring: editor state is the only truth

An extension's `activeTextEditor` is not maintained by the extension; it is mirrored from the IDE:

```
editor open/change/save/close
  → exthost_doc(kind, doc)            // full payload { path, text, languageId, dirty }
  → every running host receives rreq "doc"
```

- **220 ms debounce**: continuous typing must not produce one frame per keystroke.
- **Nothing is sent when no host runs**: `doc_event` checks the `running` state first, so megabytes of text are not serialised for a channel nobody listens to.
- **Full payloads rather than deltas**: the extension side rebuilds `TextDocument` straight from the text, with `getText()`, `offsetAt` and `lineAt` computed on demand. The bandwidth saved by delta sync is not worth an entire class of offset-misalignment bugs.

Extension edits travel the other way: `editor.edit(builder)` → `rreq "editor.edit"` → `exthost://edit` → the renderer turns `{start,end,text}` into offsets and dispatches one `view.dispatch` bottom-up (bottom-up so earlier edits do not shift the offsets of later ones).

### 12.8 Bootstrapping the Node runtime

Do not assume the user has Node (half of this IDE's target users do not). `exthost_install_node`:

```
GET https://nodejs.org/dist/index.json   → take the first version with a non-empty lts and a win-x64 build
GET …/node-v{ver}-win-x64.zip            → unpack in memory
  → the ZIP reader from §11.2, keeping only node.exe (+ LICENSE)
  → <config>/runtime/node/node.exe
```

Reusing the reader from §11.2 is deliberate: it already sits on `flate2`, so "download Node" does not justify another decompression crate. Detection order is config path → bundled runtime → `PATH` → `%ProgramFiles%\nodejs\node.exe`, with `node -v` for the version; if none is found the app shows a banner instead of letting host startup fail.

### 12.9 Lifecycle

An extension with `extensions.autostart` true gets its host started `autostart_delay_ms` after launch (default 1500 ms) — after first paint, so it does not compete with startup. Closing the window goes through:

```rust
.build(…).run(|app, event| {
    if let RunEvent::ExitRequested { .. } = event { host.stop_all(); }
});
```

Without that step `node.exe` lingers in Task Manager — fatal during development, where a few hot restarts pile up a screen of orphan processes that all hold log files under `<config>/runtime`.

### 12.10 The implemented API surface

| Group | Implemented |
| --- | --- |
| `commands` | `registerCommand`, `executeCommand` (including cross-extension dispatch), `getCommands` |
| `window` | `showInformationMessage`/`showWarning`/`showError`, `showQuickPick`, `showInputBox`, `createOutputChannel`, `createStatusBarItem`, `activeTextEditor`, `showTextDocument`, `withProgress` (pass-through) |
| `workspace` | `rootPath`, `workspaceFolders`, `fs.readDirectory/readFile/writeFile/stat`, `getConfiguration`, `onDidChangeTextDocument` |
| `env` | `appName`, `language`, `machineId`, `openExternal` |
| Documents/editing | `TextDocument` (`getText`/`lineAt`/`offsetAt`/`positionAt`/`lineCount`), `TextEditor.edit`, `workspace.applyEdit` |
| Types | `Uri`, `Position`, `Range`, `Selection`, `Location`, `Disposable`, `EventEmitter`, `ThemeIcon`, `MarkdownString`, `StatusBarAlignment`, `ViewColumn` |
| `context` | `globalState`/`workspaceState` (persisted to `<extension dir>/.state/ext-state.json`), `extensionPath`, `asAbsolutePath`, `logPath`, `storageUri` |
| `languages` | `registerCompletionItemProvider`, `registerHoverProvider`, `registerDocumentFormattingEditProvider`, `registerDocumentRangeFormattingEditProvider` — all four reach CodeMirror; every request has a 4.5 s timeout and stale replies are dropped, so a wedged host cannot freeze the editor |
| TreeView | `registerTreeDataProvider`, `createTreeView` (children are pulled from the extension on demand) |
| Webview | `createWebviewPanel`, `registerWebviewViewProvider`, `postMessage` / `onDidReceiveMessage` (panel pages load over the custom `irixen-wv` scheme) |
| Not implemented | diagnostic collections (`createDiagnosticCollection` accepts the data and drops it), go to definition, find references, code actions, document/workspace symbols, CodeLens, rename, semantic tokens, `Task`/`Terminal` APIs |

**Do not make the user guess.** The detail page of an installed extension has an "APIs used" section: `extensions::scan_api_usage` looks for the call sites that really appear in the extension's entry file, and every unimplemented capability is shown in red with an explanation. When something installed does nothing, look there first — it tells "this extension is broken" apart from "this host never implemented that API".

Unimplemented APIs **exist but throw `not implemented`** instead of being `undefined` — a feature probe like `if (vscode.window.createTreeView)` would silently downgrade an extension on `undefined`, whereas a throwing function at least puts a name in the log.

## 13. The built-in marketplace

The panel has two tabs: **Installed** and **Store**. The store talks to Open VSX directly, with no browser.

### 13.1 One API, three requests

```
GET /-/search?query=&category=&sortBy=&size=24&offset=N
GET /api/{publisher}/{name}          → details: README, version history, rating, repository
GET /api/{publisher}/{name}/{ver}    → files.download at install time
```

`sortBy` accepts `downloadCount` (default) / `averageRating` / `timestamp`, plus `relevance` when there is a query. Categories use Open VSX's own strings (`Programming Languages`, `Themes`, …) with no local mapping table — when they rename something we follow, which is cheaper than maintaining a translation that no longer matches.

### 13.2 READMEs need washing

A detail page's README is raw Markdown full of badges, relative-path images and HTML such as `<picture>`/`<div align>`. Handing that to the project's `renderMarkdown` produces dozens of broken icons and piles of blank lines:

```ts
md.replace(/<img[^>]*>/gi, '')        // badges
  .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')  // keep alt text, drop the image
  .replace(/<[^>]+>/g, '')            // any remaining HTML tags
```

Keeping the alt text is what preserves information like "1M+ downloads". The README is truncated at 60 kB on the Rust side — the longest ones (`ms-python.python`) exceed 200 kB, and pushing a whole one through IPC blanks the detail page for half a second.

### 13.3 Installed means usable

The store's "Install" button and the panel's input box go through the **same** `extensions_install`. The only difference: the store knows the exact version, so it calls `/api/{pub}/{name}/{ver}` directly and avoids `latest` disagreeing with the version the user just read.

The list refreshes after installing; if the extension has a `main`, its card grows a host row with "Start host", and its commands appear in the command palette (`Ctrl+Shift+P`). Nowhere in that chain is there a "restart to take effect" step.

## 14. Panel interaction and error theming: two UI rules

### 14.1 The `hidden` attribute is not a guarantee

The extensions panel once ate an entire page of clicks: `.store` is `position: absolute; inset: 0; display: flex`, and `.ext-body` / `.ext-detail` are `display: flex` too. `hidden` only declares `display: none` in the **UA stylesheet**, and any `display` in an author stylesheet wins at higher priority:

```
Installed tab: storeEl.hidden = true, yet it still covers the panel and swallows every click on the cards
Store tab:     listEl.hidden = true, yet the installed list keeps painting underneath
```

Hit-testing against the real CSS from the build output (headless Edge + `elementFromPoint`) quantified it: without the global rule, `store` computes to `flex` at 756×488 and the "Output" and "Disable" buttons both hit-test to `store-list`; with the rule they hit themselves.

The conclusion is one global rule in `base.css` instead of an `.xxx[hidden]` variant per component:

```css
[hidden] { display: none !important; }
```

Every use of `hidden` in the project (`dock` / `editor-area` / the extensions panel) is a genuine hide with no "hidden but shown by CSS" exception, so the rule has no side effects. **Any new full-screen layer with `display` must toggle visibility through `hidden`** — do not write `style.display` again.

The same round also set the `.toasts` container to `pointer-events: none` (each toast restores `auto`): a notification stack sitting over a button in the bottom-right should never make the whole UI feel dead.

### 14.2 Error colours must come from semantic variables

Error surfaces used to carry scattered hard-coded rgba values: chat bubbles used `rgba(248,113,113,…)` (the dark-theme red), extension dialog badges used `rgba(220,38,38,…)` (the light-theme red). After a theme switch half the error components stayed in the other palette, and a custom theme overriding `--err` could not reach them at all.

`base.css` now defines `--ok-soft` / `--warn-soft` / `--err-soft` / `--err-line`, each with a literal fallback for both themes; WebView2 builds with `color-mix()` support recompute them from the current `--err`/`--warn`/`--ok`, so changing `--err` in a custom theme moves every error surface at once:

```css
@supports (color: color-mix(in srgb, red 10%, transparent)) {
  :root, :root[data-theme='dark'] {
    --err-soft: color-mix(in srgb, var(--err) 11%, transparent);
    --err-line: color-mix(in srgb, var(--err) 40%, transparent);
  }
}
```

Consumers: error/success toasts, inline error blocks, chat error bubbles, `probe-err`, extension dialog severity badges, the missing-Node banner. The `✕/!/i` text glyphs inside those badges became SVG icons (`alert` / `info`, inheriting `currentColor`) — `✕` looked more like a close button than "something went wrong".

One detail that is easy to miss: toasts float over editor content, and `--err-soft` is translucent, so using it directly as `background` smears the text into the code underneath. Toasts therefore keep an opaque `--bg-3` base with the semantic colour layered on top:

```css
background: linear-gradient(var(--err-soft), var(--err-soft)), var(--bg-3);
```

In-panel errors (failed search, failed list load, failed log read) no longer use a toast that flashes past; they stay where the user is looking, as an inline error block with a "Retry" button, and every title goes through i18n with keys in all 8 languages.

## 15. End-to-end test: installing a real Open VSX extension

Every earlier verification used a hand-built smoke extension. The final round switched to `esbenp.prettier-vscode` 12.4.0 (a 2 MB VSIX, `"type": "module"`, ESM bundles, 35 configuration entries) and walked from store search all the way to command execution, which surfaced five bugs that only exist in the real ecosystem.

**① `sortBy=averageRating` is wrong.** Open VSX accepts only `relevance | timestamp | rating | downloadCount`; `averageRating` is a **response field** name, not a parameter. Sending it returned HTTP 400 → sorting by rating produced an empty list, and because every later request carried the bad parameter, even the detail page would not open. Changed to `"rating" => "rating"`.

**② Concurrent searches overwrote each other.** Opening the store tab fires a default query, and the user typing immediately fires a second one; the slower reply landed last and wrote itself into the list, so "search prettier, get Python". `loadStore()` now carries a `storeSeq` and only the last request may write the DOM; `openDetail()` gained `detailSeq` the same way. A successful install from the detail page also calls `markInstalled()` to sync the "Installed" tag on the row behind it.

**③ ESM named exports crashed at link time.** The error was `SyntaxError: Named export 'CodeAction' not found`. `cjs-module-lexer` only recognises **static** `module.exports.X = ...` lines, and a Proxy cannot fake named exports. `<config>/node_modules/vscode/index.js` is now **generated**: VS Code's public API list ∪ the names found by scanning the extension entry for `import { … } from 'vscode'` ∪ the names already in the shim (so the file only grows, and several hosts starting at once cannot delete each other's names). Unimplemented names get a permissive `stub()` (property access, callable, `new`-able, deliberately not thenable) so the extension passes linking and only errors when it actually uses a provider.

**④ `CodeActionKind` had two placeholder keys.** Prettier evaluates `CodeActionKind.SourceFixAll.append('prettier')` at module top level, and the placeholder object had no `SourceFixAll` → `Cannot read properties of undefined`. It is now a real class: `Empty/QuickFix/Refactor*/Source*/Notebook`, with `append()/contains()/intersects()/value`.

**⑤ The `getConfiguration` cache was never written.** `read()` dropped the async result on the floor, so `.get()` always returned the caller's default; and real extensions often use **property access** such as `const { enable } = workspace.getConfiguration('prettier')`, which a plain object never had. It is now a Proxy (unknown properties map to configuration entries) that prefers this extension's `contributes.configuration.properties[key].default`; `workspace.isTrusted` defaults to `true`, because otherwise prettier takes its "untrusted" branch, flattens the config object and loses every property. `languages` also got a fallback proxy: unimplemented provider methods return a writable `Disposable`, so an extension registering a formatter in `activate()` does not die on "not a function".

**Measured result**: search → category (Themes) → sort (rating) → detail (15 kB README, 40 versions, no broken images, no horizontal overflow, the `JS` marker decided from `main` in `files.manifest`) → install → card with a host row → start → `activate()` succeeds, the output channel prints name and version → 3 commands `live: true` → clicking `prettier.openOutput` returns `✓`. No IDE restart anywhere in the chain.

Still explicitly out of scope: real language-service providers (completion/diagnostics/definition), Webview, TreeView, Task/Terminal. The table in §12.10 stands — "can register without crashing" and "actually completes your code" are two different things.
