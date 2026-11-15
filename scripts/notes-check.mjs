#!/usr/bin/env node
// Release notes are a deliverable for two audiences, so every notes file must
// carry both halves: "## 简体中文" first, then "## English" after a `---`.
//
//   node scripts/notes-check.mjs Beta0.2                 # gate, fails when not bilingual
//   node scripts/notes-check.mjs Beta0.2 --allow-missing # warn instead when no file
//
// Same check runs in .github/workflows/release.yml (before publishing) and in
// release-notes.yml (when re-syncing the body of an existing release), so the
// rule lives in one place instead of being copy-pasted into YAML.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const args = process.argv.slice(2);
const tag = args.find((arg) => !arg.startsWith("--"));
const allowMissing = args.includes("--allow-missing");

if (!tag) {
  console.error("usage: node scripts/notes-check.mjs <tag> [--allow-missing]");
  process.exit(2);
}

const path = join(root, "docs", "release-notes", `${tag}.md`);
const shown = relative(root, path).replace(/\\/g, "/");

if (!existsSync(path)) {
  const message = `${shown} is missing — GitHub will generate the notes from the commit range`;
  if (allowMissing) {
    console.log(`::warning::${message}`);
    process.exit(0);
  }
  console.error(`::error::${message}`);
  process.exit(1);
}

const text = readFileSync(path, "utf8");

// Anchored to a heading line: the phrase also shows up in prose ("see the
// 简体中文 section"), and that must not count as the section itself.
const zhAt = text.search(/^##+[ \t]*简体中文[ \t]*$/m);
const enAt = text.search(/^##+[ \t]*English[ \t]*$/m);

const problems = [];
if (zhAt < 0) problems.push('no "## 简体中文" heading');
if (enAt < 0) problems.push('no "## English" heading');
if (zhAt >= 0 && enAt >= 0 && enAt < zhAt) {
  problems.push('"## English" comes before "## 简体中文" — Chinese goes first');
}

if (problems.length > 0) {
  console.error(`::error::${shown} must be bilingual (Chinese, then English): ${problems.join("; ")}`);
  console.error("  template: https://github.com/lanche-furry/IRIXEN-Code/blob/main/docs/release-notes/Beta0.2.md");
  process.exit(1);
}

// A heading with nothing under it satisfies the pattern but says nothing, and
// that is exactly the mistake this check exists to catch.
const zhBody = text.slice(zhAt, enAt).trim();
const enBody = text.slice(enAt).trim();
const thin = [];
if (zhBody.length < 200) thin.push(`简体中文 is only ${zhBody.length} chars`);
if (enBody.length < 200) thin.push(`English is only ${enBody.length} chars`);

if (thin.length > 0) {
  console.error(`::error::${shown} has empty or stub sections: ${thin.join("; ")}`);
  process.exit(1);
}

console.log(`${shown}: bilingual (简体中文 ${zhBody.length} chars, English ${enBody.length} chars)`);
