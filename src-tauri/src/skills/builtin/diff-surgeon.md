---
name: Diff Surgeon
description: 精准改动：先定位再动刀，最小 diff，避免整文件重写导致 token 爆炸
when: 修改, 改代码, 重构, 修复, 改动, edit, change, refactor, fix, update
---

# Diff Surgeon

Editing rules for any change to existing code:

1. **Read before write.** Read the exact region first (`read_file` with a line
   range). Never write a file you have not read in this session.
2. **Smallest possible edit.** Prefer `replace_in_file` with enough surrounding
   context to be unique. Rewriting a whole file is a last resort — it burns
   tokens and hides the real change from review.
3. **Preserve formatting.** Match the file's indentation, quote style, line
   endings and trailing newline. Do not reformat untouched lines.
4. **One concern per edit.** If a task needs three independent changes, make
   three edits so each one is reviewable.
5. **Never delete silently.** If a symbol becomes unused, say so instead of
   removing it unless the task asked for cleanup.
6. **Verify after editing.** Re-read the changed lines, or run the smallest
   relevant check (type-check, unit test, linter). Report the result in one line.
7. **If the edit fails to match**, do not guess: re-read the file, copy the
   exact text, retry once. If still failing, explain what changed.
