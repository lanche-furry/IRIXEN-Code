# Contributing

<p align="center">
  <strong>English</strong> &nbsp;|&nbsp; <a href="./CONTRIBUTING.md">中文</a>
</p>

## Development setup

```bash
npm install
npm run app:dev      # full app with hot reload
npm run check        # compile Rust only (fast)
npm run build        # frontend build + type check
```

Toolchain details are in [docs/BUILD.md](docs/BUILD.md).

## Code map

Where to go for what:

| Want to change | Go to |
| --- | --- |
| Add a vendor / fix protocol compatibility | `src-tauri/src/agent/provider.rs` (add an `impl LlmProvider`) |
| Thinking-effort detection rules | `src-tauri/src/agent/models.rs::detect_thinking` |
| Add a native tool | `src-tauri/src/agent/tools.rs` (`native_tool_specs` + `execute_native`) |
| Compaction policy | `src-tauri/src/agent/context.rs` |
| System prompt / turn limit | `AgentConfig::default` in `src-tauri/src/config.rs` |
| Built-in skills | `src-tauri/src/skills/builtin/*.md` |
| UI layout | `src/styles/app.css` (each region is its own block) |
| Colors / corner radius / type scale | `:root` tokens at the top of `src/styles/base.css` |
| UI strings | `src/core/i18n.ts` (five languages, keys must line up) |
| Chat rendering | `src/ui/chat.ts` |

## Conventions

**Rust**

- The command layer (`commands.rs`) only forwards calls and maps errors with `map_err(|e| e.to_string())` — no business logic there.
- With `async` and shared state, never hold a `MutexGuard` across an `await`: clone what you need inside the lock, then release it.
- User-facing errors are written in Chinese and must say **how to fix it** ("no provider configured → Settings → Providers").

**TypeScript**

- No framework: build nodes with `h()`, redraw explicitly when state changes.
- Types must match the `serde` output from Rust (`src/types.ts` is the only mirror).
- New strings go into all five dictionaries at once; a missing key falls back to English, but don't leave gaps on purpose.

**CSS**

- Use only the variables defined in `base.css` — no magic color values (custom themes work by overriding those variables).
- Add new rules to the matching block in `app.css`, keeping the file's order: title bar → activity bar → sidebar → editor → panel → chat → status bar → settings.

## Commits

One line that states what changed and why:

```
Fix usage being dropped from Gemini streaming responses

- Read promptTokenCount / candidatesTokenCount when parsing usageMetadata
- Previously only the last chunk was read; some proxies split it across chunks
```

## Pull requests

1. Explain the motivation (what problem it solves, what capability it adds).
2. UI changes: attach a screenshot or GIF.
3. Protocol changes: list the vendors and models you tested.
4. Keep `npm run build` and `npm run check` passing.

## License

By contributing you agree to license your work under the [Apache License 2.0](LICENSE).
