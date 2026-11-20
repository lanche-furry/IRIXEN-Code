# Skill development

<p align="center">
  <strong>English</strong> &nbsp;|&nbsp; <a href="./SKILLS.md">中文</a>
</p>

A skill is a piece of instruction text that can be **triggered by condition**. It runs no code; it only shapes how the model behaves — which is why injecting the right few sentences at the right moment is cheaper and steadier than piling on tools.

## File layout

A skill is a directory containing at least a `SKILL.md`:

```
my-skill/
  SKILL.md          # required
  references/       # optional: extra material the model can read on demand
```

`SKILL.md` is a frontmatter block plus a body:

```markdown
---
name: token-saver
description: 用最少的 token 把事做完
when: 省token, token, 精简, 太长, 冗长, 简短
---

# 输出纪律

1. 不复述需求，不写"好的，我来…"。
2. 改代码只给 diff 或改动块，不重贴整文件。
3. 一次说清，不要分多轮试探。
```

| Field | Meaning |
| --- | --- |
| `name` | Display name, also used for de-duplication |
| `description` | One-line description shown in the skills panel |
| `when` | Comma-separated keywords; **injected when they match the user input** |
| Body | Spliced verbatim into the system prompt (under the `# 当前激活的 Skill 指引` section) |

Keyword matching is a case-insensitive substring match. Chinese has no word segmentation, so keep `when` entries to short 2–4 character words for a better hit rate.

## Built-in skills

Six of them, with sources in `src-tauri/src/skills/builtin/`, compiled into the binary via `include_str!`, so they are always available and cannot be deleted (they can be disabled).

| Skill | Trigger | Effect |
| --- | --- | --- |
| `token-saver` | "save tokens", "too long", "be brief" | Output discipline: no restating, diffs only, no multi-round probing |
| `diff-surgeon` | "change this", "refactor", "replace" | Read before editing, minimal edits, no whole-file rewrites |
| `repo-scout` | "this project", "structure", "entry point" | Map the directories and entry points before acting instead of guessing paths |
| `commit-craft` | "commit", "write a message" | Commit-message convention: one-line subject plus a bullet list |
| `code-reviewer` | "review", "check", "anything wrong" | Reports in the order correctness → edge cases → performance → style |
| `security-auditor` | "security review", "vulnerability", "audit", "secret" | Runs `security_scan` first, then verifies every hit; secrets stay masked, only findings with an attack path get reported |

## Installation

Three sources, all pasted into the input box on the **Skills** panel:

| Source | Example |
| --- | --- |
| Local directory (containing `SKILL.md`) | `D:\skills\my-skill` |
| Single-file URL | `https://example.com/SKILL.md` |
| Git repository | `https://github.com/user/skill-repo` |

Installs land in `~/.irixen-code/skills/<id>/` and survive a restart.

## Toggles and auto-matching

- The switch on the panel controls **whether the skill participates in matching** (a disabled skill is never injected, even on a keyword hit)
- Settings → Agent → "load skills by keyword automatically" is the master switch
- The engine recomputes matches before every send, so toggles take effect immediately

## What to write

**Do**

- Output-format constraints ("diffs only", "conclusion first", "at most 5 lines")
- Domain conventions ("this project's migrations live in `db/migrations`")
- Explicit prohibitions ("do not add dependencies")

**Don't**

- Long background material → put it in `references/` and let the model read it
- General programming common sense → the model already knows it; writing it down just burns tokens
- Anything that duplicates `system_prompt`

**Keep keywords narrow**
`when: 代码` makes a skill fire on almost every request, multiplying cost across every conversation. Narrow is better.

## Debugging

```bash
# see which skills are currently loaded
cat ~/.irixen-code/skills/*/SKILL.md

# rebuild after changing a built-in skill
npm run app:build
```

If the conversation shows a `已加载 skill: xxx` status line, the injection took effect.
