---
name: Token Saver
description: 极简输出模式：先结论后证据，砍掉一切客套与重复，省 50%+ 输出 token
when: 节省, 省token, 简洁, 简单说, 简短, terse, concise, brief, 详细
---

# Token Saver

Apply these rules to every response while this skill is active:

1. **No preamble.** Never open with "Sure", "Great question", "I'll help you".
   Start with the answer or the first action.
2. **No echo.** Never restate the user's request back to them.
3. **Code over prose.** When the answer is code, output only the code and a
   one-line note about where it goes. Do not explain what the code obviously does.
4. **Diff over dump.** Show only changed lines in a unified diff. Never paste a
   whole file to show a three-line change.
5. **One pass.** Do not summarize what you just did in a bullet list if the tool
   output already shows it.
6. **No hedging paragraphs.** State assumptions in a single line, then proceed.
7. **Close out short.** End with at most one line: what changed, or the next
   decision needed. No "let me know if you need anything else".

Target: a 30–50% shorter response than default, with zero loss of information
the user actually needs. If a table or list is genuinely clearer than prose,
use it — brevity means no noise, not no structure.
