#!/usr/bin/env node
// Release notes follow the Beta 0.1 layout: one document, both languages on
// every line, `中文 / English`. Long prose is two paragraphs (Chinese first),
// headings and bullets carry the pair on the same line — see
// docs/release-notes/Beta0.2.md.
//
//   node scripts/notes-check.mjs Beta0.2                 # gate, fails when it drifts
//   node scripts/notes-check.mjs Beta0.2 --allow-missing # warn instead when no file
//
// Runs in .github/workflows/release.yml before publishing, and locally as
// `npm run notes:check -- <tag>` — one implementation, no copy-paste into YAML.

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
const lines = text.split(/\r?\n/);

const hasCjk = (s) => /[\u4e00-\u9fff]/.test(s);
const hasLatin = (s) => /[A-Za-z]{2,}/.test(s);

/** Both languages on one line: Chinese on the left, English after the first
 *  ` / `. Returns null when the pair is well formed, or the reason it is not. */
function pairProblem(line) {
  const at = line.indexOf(" / ");
  if (at < 0) return "no ` / ` separator";
  const zh = line.slice(0, at);
  const en = line.slice(at + 3);
  if (!hasCjk(zh)) return "the part before ` / ` is not Chinese";
  if (!hasLatin(en)) return "the part after ` / ` is not English";
  return null;
}

const problems = [];

const title = lines.find((line) => line.trim()) ?? "";
if (!/^#\s+IRIXEN Code\s+\S/.test(title)) {
  problems.push(`first line must be "# IRIXEN Code <version>", found "${title.trim()}"`);
}

// The old two-halves layout is not wrong prose, it is just not this layout —
// say so instead of listing every heading it fails.
if (/^##+\s*(简体中文|English)\s*$/m.test(text)) {
  problems.push(
    "looks like the two-halves layout — this repo uses the Beta 0.1 layout: " +
      "`## 修复 / Fixes`, `- 中文 / English` on every line",
  );
}

const badHeadings = [];
for (const line of lines) {
  if (!/^#{2,6}\s+\S/.test(line)) continue;
  const heading = line.replace(/^#{2,6}\s+/, "").trim();
  const why = pairProblem(heading);
  if (why) badHeadings.push(`"${heading}" (${why})`);
}
if (badHeadings.length > 0) {
  problems.push(`headings must read "中文 / English": ${badHeadings.join(", ")}`);
}

// Bullets are where the split shows up most often: one language written, the
// other forgotten, leaving a Chinese-only list under a bilingual heading.
const bullets = lines.filter((line) => /^[-*]\s+\S/.test(line));
const paired = bullets.filter((line) => pairProblem(line.replace(/^[-*]\s+/, "")) === null);
const MIN_PAIRED = 6;
if (paired.length < MIN_PAIRED) {
  problems.push(`only ${paired.length} bilingual bullet(s), need at least ${MIN_PAIRED}`);
}

// A stub satisfies the shapes above while saying nothing, which is exactly the
// mistake this check exists to catch.
const cjk = (text.match(/[\u4e00-\u9fff]/g) ?? []).length;
const latin = (text.match(/[A-Za-z]/g) ?? []).length;
if (cjk < 150) problems.push(`only ${cjk} Chinese characters — that is not a release note`);
if (latin < 250) problems.push(`only ${latin} Latin letters — the English half looks empty`);

if (problems.length > 0) {
  for (const problem of problems) console.error(`::error::${shown}: ${problem}`);
  console.error("  format: https://github.com/lanche-furry/IRIXEN-Code/blob/main/docs/release-notes/Beta0.2.md");
  process.exit(1);
}

console.log(
  `${shown}: bilingual (Beta 0.1 layout) — ${paired.length} paired bullets, ` +
    `${cjk} Chinese chars, ${latin} Latin letters`,
);
