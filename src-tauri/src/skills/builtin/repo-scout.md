---
name: Repo Scout
description: 高效摸清陌生仓库：目录→入口→符号，最多 5 次工具调用定位问题
when: 这个项目, 代码库, 仓库, 找一下, 在哪, 结构, 架构, explore, codebase, repo, locate, find
---

# Repo Scout

Cheapest-first exploration order. Stop as soon as you can act.

1. `list_dir` at depth 2 from the root — identifies language, build system,
   entry points and test layout in a single call.
2. Read the manifest (`package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod`)
   — dependencies reveal the framework and the conventions in use.
3. `search_files` for the symbol, route, command or error string you were asked
   about. One targeted search beats ten speculative reads.
4. `read_file` only the matched region (line range), not the whole file.
5. Follow imports outward only as far as needed to understand the contract.

Hard limits:
- Do not read more than ~1500 lines of source to answer a question unless the
  task explicitly requires a full audit.
- Do not dump directory trees longer than 60 lines into the answer.
- Cache what you learn in your reasoning: "auth lives in src/auth/session.ts,
  token refresh in refresh.ts" — so you never search the same thing twice.
