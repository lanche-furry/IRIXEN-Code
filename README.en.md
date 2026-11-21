<p align="center">
  <img src="public/assets/irixen-wordmark.png" alt="IRIXEN Code" width="420" />
</p>

<p align="center">
  <b>IRIXEN Code</b> — a lightweight desktop IDE with a native code agent<br />
  Tauri 2 · WebView2 · Rust · zero frontend framework
</p>

<p align="center">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-blue" /></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.77%2B-orange" />
  <img alt="Tauri" src="https://img.shields.io/badge/tauri-2.x-24C8DB" />
  <img alt="Size" src="https://img.shields.io/badge/installer-small-brightgreen" />
</p>

<p align="center">
  <strong>English</strong> &nbsp;|&nbsp; <a href="./README.md">中文</a>
</p>

IRIXEN Code has two parts:

| Component | Description |
| --- | --- |
| **IRIXEN Agent** | The Rust agent core: one abstraction over three protocols, native tools, an MCP client, a skill system, and a three-stage context compressor |
| **IRIXEN Code** | The desktop shell: Tauri 2 + the system WebView2, a borderless window with no splash (visible the moment it is created), a left nav rail + a center conversation area + a right quick bar; opening a file **overlays** the editor on the conversation instead of opening a third column |

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
- **Whole-turn undo**: before a turn edits anything, the content of each file **as it was before its first write** is recorded, and a bar above the composer reads "N files changed ⟲ Undo this turn". It rolls back in reverse and deletes the files the turn created, without touching git. A turn over 8MB or touching a binary is marked not undoable with the reason shown (better to refuse than to half-roll-back); only the last 48 turns are kept
- **MCP client**: JSON-RPC over stdio, handshake `2024-11-05`; MCP tools are scheduled in the same pool as native tools, and on Windows `npx` / `uvx` shims are wrapped in a shell automatically

**Three-stage context compression** (the key to saving tokens)

1. **Tool-result elision** — historical tool output above a threshold is collapsed into a one-line summary (paths and conclusions preserved)
2. **Sliding window + task anchors** — keep the most recent K messages; older ones are merged into anchors (goal / decision / file)
3. **LLM structured digest** — when still over budget, the model produces a `Goal / Decisions / Files / State / Pending` summary; on failure it falls back to a heuristic summary. The digest phase takes **no lock** and never blocks the UI

**IDE shell**

The layout is deliberately **not** the VS Code "three permanent columns": a **left nav rail + a center conversation area**, with code files **overlaid** on the conversation. Close the tab and you are back to the conversation — the visual focus is always singular.

- Borderless window, custom-drawn title bar and window buttons (no agent badge; the title bar keeps only the workspace and window controls)
- **Left nav column** (fixed 56px, icon + short label, fixed position and order): Files / Search / Git / Skills / Stats / Review. Beside it is the content column for the current view, with the view name and its tool buttons on top; the explorer has three quick buttons — **New File / New Folder / Collapse All** (also in the right-click menu)
- **Right 40px quick bar**: Chat, Extensions (badge = number of running hosts), Files (only when a tab is open), Terminal, Settings
- **The conversation area is an overlayable canvas**: open a file → the editor plus its tab bar `inset: 0` covers the conversation; a pinned "back to conversation" button on the right of the tab bar stays put and never scrolls away
- Startup: **no splash window, no intro animation**. The window is created with `visible: true` (background = theme `--bg-0`), so it is visible as soon as the process starts and the first UI frame lands directly on it; `main_loaded` only reclaims focus and records mount time
- CodeMirror 6 editor: multi-tab, syntax highlighting, dirty markers, `Ctrl+S` save, jump to line from search results
- **Image input**: `Ctrl+V` to paste or drop a file in; anything wider than 1600px is scaled down onto a white background (transparent PNGs do not turn black). Images go out in each protocol's native shape (OpenAI `image_url` data URLs / Anthropic `source.base64` / Gemini `inlineData`), while text-only turns still send a plain string. 4MB per image, 8 images per message, 1000 tokens each against the context budget
- **Session history**: a searchable list of sessions (title / message count / relative time) — click to resume, hover to delete. Deleting the session you are in starts a new one, and switching sessions clears pending images
- **In-app updates**: "Download and install" on the About page streams the installer, hashes it against the release's `checksums.txt` along with its size, then launches the installer and quits. A Windows exe cannot replace itself, so installing is always a separate process
- Lazy file tree, full-workspace search (respects `ignore` rules), Git panel (stage / commit / push / pull / branch / diff)
- **Review panel**: scan all workspace changes at a glance (`git status` + diff stats), then hand "what changed and is it risky" plus the current diff to the agent in one click. Line-level diffs are colored red/green and linked with the status bar and quick-bar badges
- **Security review**: the "Security review" button only states the intent — the agent runs `security_scan` (a native tool with a fixed rule table: cloud credentials, private-key blocks, disabled TLS verification, shell and SQL injection, world-writable files, key material, CI actions not pinned to a SHA), then reads the code around every hit to decide whether it is reachable. Secrets are masked in the output (4 characters of head and tail); the scanner enumerates, the model judges
- **Access modes**: three levels above the composer — "Restricted" asks before every write and command and makes paths outside the workspace unreachable, "Auto-approve" works freely inside the workspace and asks only when leaving it, and "Full access" asks nothing and can read or write outside it. The agent parks on an approval card until you answer, and Stop also releases any pending approval
- Workspace stats (file count / code lines / size / per-extension distribution / largest file) and API usage (calls, input/output, reasoning, cache hits, grouped by model and session)
- Themes: Dark / Light / **Custom** (override any CSS variable; `--accent-soft` and the contrast color are derived automatically)
- Languages: Simplified Chinese / Traditional Chinese / English / Japanese / Korean / Russian / Spanish / French (8 locales, ~540 keys; the key set is locked by a TS type, so a missing translation is a **compile error**, not a runtime fallback; switching is instant and needs no restart)

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

When Node is missing, the panel prompts and offers a **one-click Node runtime download** (~30 MB, only `node.exe` is extracted to `<config>/runtime/node/`), or you can point it at an existing `node`. Each extension's host can be started/stopped independently; logs and commands live in the extension card.

The **extension store** is built in — no browser needed: a search box + categories (Themes / Languages / Snippets / Formatters / Linters / Other) + sorting (downloads / rating / update time). Open an item to see its README, rating, license, all versions (install a specific one), and repo link. Data comes from **Open VSX** — the open-source mirror of the VS Code Marketplace.

Four install sources, all pasted into one input box in the extensions panel:

- **Online**: `publisher.name` (e.g. `sdras.night-owl`) → resolved through **Open VSX** (the open-source mirror of the VS Code Marketplace) and downloaded as a `.vsix`
- **Local**: pick a `.vsix` file
- **Direct link**: any `https://…/x.vsix`
- **Git**: `github:owner/repo`, `owner/repo`, `git@…`

A `.vsix` is just a ZIP, so there is a built-in ZIP reader (central directory + `flate2` inflate) with no external unzip dependency; on unpack it strips the `extension/` prefix, rejects absolute paths and `..`, and caps file count (20k) and total size (512 MB).

DSH-style repos (skill packs with a `SKILL.md`) are recognized as type `dsh` and, on install, automatically find `SKILL.md` recursively and import it into the skill system — extensions work immediately, no second step.

Where they live: in the **app folder** (next to the executable) by default, under `extensions/` and `skills/` — installed with the program, easy to find, easy to carry along. Settings → Advanced → add-ons folder points them anywhere else; clearing it and pressing "use default" goes back. Coming from an older build, the first start copies whatever was in `%APPDATA%\IRIXEN Code` and leaves the originals in place.

> Why GitHub repos go through `codeload.github.com/{owner}/{repo}/zip/HEAD` instead of `git clone`: on Windows `git` uses its own OpenSSL CA bundle and reports `certificate signer not trusted` in environments that do TLS interception, while the app's in-process `reqwest` uses system schannel and the same network works. Non-GitHub repos fall back to `git clone` (with `-c http.sslBackend=schannel`).

**Skills**

- `SKILL.md` + frontmatter (`name` / `description` / `when`); when `when` matches the user input, the system prompt is injected automatically
- 5 in-house built-in skills, all oriented toward "save tokens, fewer detours": `token-saver` `diff-surgeon` `repo-scout` `commit-craft` `code-reviewer`
- Interactive install: a local directory, an http(s) link to a `SKILL.md`, or a Git repo
- Only matched skills are injected — the full skill set is never stuffed into context, which is itself a token-saving design

## Installation

Release assets are published per platform:

- **Windows installer (recommended)**: on [Releases](https://github.com/lanche-furry/IRIXEN-Code/releases/latest) download `IRIXEN.Code_*_x64-setup.exe` and run it. The **NSIS** installer is a per-user install under `%LOCALAPPDATA%\IRIXEN Code`: no administrator rights, a language choice while it runs, Start Menu / desktop shortcuts, and the WebView2 Runtime bootstrapped for you (Win10 needs internet; Win11 has it built in). It never touches your profile, so settings and sessions survive an upgrade. Machines that installed the early Beta 0.4 MSI should uninstall that build first.
- **Linux**: from the same page take `IRIXEN_Code_*_amd64.AppImage` (`chmod +x`, then run) or `IRIXEN_Code_*_amd64.deb` (`sudo apt install ./…deb`). The system must provide `libwebkit2gtk-4.1-0` and `libgtk-3-0`; the deb pulls them in, the AppImage does not bundle system libraries. Ubuntu 22.04+ / Debian 12+ run it directly.

> WebView2 Runtime is required: Win11 has it built in; Win10 gets it via the installer over the internet.

Daily builds live in the [nightly](https://github.com/lanche-furry/IRIXEN-Code/releases/tag/nightly) pre-release channel. They track `main` and are not stable — the latest channel is the supported release.

## Quick Start

```bash
npm install
npm run app:dev      # develop (auto-starts vite + tauri)
npm run app:release  # build the standalone release exe (--no-bundle)
npm run app:build    # build the NSIS installer
```

After first launch: go to `Settings → Providers`, enter the Base URL and API Key, click **Test connection**, and the model list appears automatically; then pick a model and a thinking-effort level above the chat box.

> Build environment: Rust 1.77+ (GNU target), Node 18+, WebView2 on Windows 10/11.
> This repo uses a **bundled toolchain**: Rust and build tools all live under `.tools/` — see [docs/BUILD.md](docs/BUILD.md).

## Project Layout

```
src/                    Frontend (TypeScript, zero framework)
  core/                dom / i18n (8 languages) / api / state
  core/locales/        zh-TW / es-ES / fr-FR (the rest inline in i18n.ts)
  ui/                  shell (layout + quick bar) · chat · editor · explorer · search
                             git · skills · stats · review · extensions · settings
  styles/              base.css + app.css (all design tokens)
src-tauri/             Rust backend
  src/agent/           provider (three protocols) / models / context / tools / meter / types
  src/extensions.rs    Extension subsystem: VSIX unpacking (built-in ZIP reader) / Open VSX store / themes·snippets·skills
  src/exthost/         Node extension host: process management + NDJSON RPC + bootstrap.js (vscode API shim)
  src/commands.rs      IPC command surface
  src/mcp.rs           MCP stdio client
  src/skills.rs        Skill registry (compile-time built-ins + user installs)
  src/skills/builtin/  5 built-in SKILL.md
public/assets/         Wordmark and other static assets
assets/icon.png        App icon master (1254×1254 transparent PNG; `npm run icons` generates the full size set)
scripts/               Icon generation / toolchain launcher
docs/                  Architecture, build, skill development
```

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Build & Toolchain](docs/BUILD.md)
- [Skill Development](docs/SKILLS.md)
- [Antivirus false positives](docs/antivirus.md)
- [Contributing](CONTRIBUTING.en.md)

## License

[Apache License 2.0](LICENSE) © lanche-furry
