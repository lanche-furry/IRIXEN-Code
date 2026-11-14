---
name: Commit Craft
description: 规范提交信息：Conventional Commits，从 diff 自动归纳，一句话说清动机
when: 提交, commit, git, 暂存, push, 版本, changelog
---

# Commit Craft

When asked to commit (or when a change is complete and a commit is expected):

1. Run `git status` and `git diff` (staged and unstaged) before writing anything.
2. Group changes by intent. If the working tree mixes unrelated concerns, stage
   selective paths and make **separate commits** — never one giant blob.
3. Message format:

   ```
   <type>(<scope>): <imperative summary, ≤ 60 chars>

   <why it was needed, 1–3 lines, wrapped at 72 cols>

   <optional: notable trade-offs or follow-ups>
   ```

   Types: `feat` `fix` `refactor` `perf` `docs` `test` `build` `chore` `style`.
   Scope = the module or area touched. Summary in imperative mood ("add", not
   "added"), lowercase, no trailing period.
4. Never claim tests pass unless you ran them. If not run, say "not run".
5. Never commit secrets, `.env`, lockfile churn unrelated to the change, or
   build artifacts.
6. After committing, report the short hash and the subject line. Nothing more.
