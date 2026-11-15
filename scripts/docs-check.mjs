#!/usr/bin/env node
// Every Markdown file in the repository ships in both languages.
//
// Three shapes are allowed, and nothing else:
//
//   1. a mirrored pair     README.md + README.en.md — each file one language,
//                          each linking to the other, same skeleton (heading
//                          depths and fenced-block count must match)
//   2. inline bilingual    docs/release-notes/*.md — one document, `中文 / English`
//                          on every heading and bullet; validated by notes-check.mjs
//   3. a runtime prompt    src-tauri/src/skills/**/*.md — fed to the model at
//                          runtime, not read by humans; translating it would
//                          double the prompt for every conversation
//
//   node scripts/docs-check.mjs            # gate, fails on a one-language doc
//   node scripts/docs-check.mjs --verbose  # also list what passed and why
//
// Runs in .github/workflows/ci.yml; the payload is the `git ls-files` list, so a
// file must be tracked before it is checked.

import { execFileSync } from "node:child_process";
import { readFileSync, existsSync } from "node:fs";
import { dirname, join, posix } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const verbose = process.argv.includes("--verbose");

/** Paths that are deliberately not human documentation. */
const RUNTIME_PROMPTS = /^src-tauri\/src\/skills\//;
/** Inline bilingual documents, one language pair per line rather than per file. */
const INLINE_BILINGUAL = /^docs\/release-notes\/[^/]+\.md$/;

const cjk = (s) => (s.match(/[\u4e00-\u9fff]/g) ?? []).length;
const latin = (s) => (s.match(/[A-Za-z]/g) ?? []).length;

/** Split into lines while remembering which ones are real prose (not code). */
function outline(text) {
  const lines = text.split(/\r?\n/);
  const headings = [];
  let inFence = false;
  let blocks = 0;
  for (const line of lines) {
    if (/^\s*(```|~~~)/.test(line)) {
      if (!inFence) blocks += 1;
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;
    const m = /^(#{1,6})\s+\S/.exec(line);
    if (m) headings.push({ depth: m[1].length, text: line.replace(/^#{1,6}\s+/, "").trim() });
  }
  return { headings, blocks, unbalanced: inFence };
}

const problems = [];
const fail = (file, message) => problems.push(`${file}: ${message}`);

function trackedMarkdown() {
  // `--others` so a mirror written but not yet committed is checked too; the
  // exclude-standard pair keeps build output and ignored scratch files out.
  const out = execFileSync(
    "git",
    ["ls-files", "--cached", "--others", "--exclude-standard", "*.md"],
    { cwd: root, encoding: "utf8" },
  );
  return [...new Set(out.split(/\r?\n/).filter(Boolean))].sort();
}

const files = trackedMarkdown();
const known = new Set(files);

/** A pair is `X.md` + `X.en.md`; the base is whichever one is being looked at. */
function pairOf(file) {
  if (file.endsWith(".en.md")) {
    const base = file.slice(0, -".en.md".length) + ".md";
    return { base, english: file };
  }
  const english = file.slice(0, -".md".length) + ".en.md";
  return { base: file, english };
}

function checkPair(base, english) {
  if (!known.has(english)) {
    fail(base, `no English mirror — add ${posix.basename(english)} next to it`);
    return false;
  }
  const zhText = readFileSync(join(root, base), "utf8");
  const enText = readFileSync(join(root, english), "utf8");

  // Each side must point at the other, so a reader is never stuck in the wrong language.
  if (!zhText.includes(posix.basename(english))) {
    fail(base, `does not link to ${posix.basename(english)} — add the language switch under the title`);
  }
  if (!enText.includes(posix.basename(base))) {
    fail(english, `does not link back to ${posix.basename(base)}`);
  }

  // The Chinese side must actually be Chinese; a copy-paste of the English file
  // would otherwise pass every structural check below.
  if (cjk(zhText) < 50) fail(base, `only ${cjk(zhText)} Chinese characters — is this the Chinese file?`);
  if (latin(enText) < cjk(enText) * 8) {
    fail(english, `${latin(enText)} Latin letters against ${cjk(enText)} Chinese characters — most of it is untranslated`);
  }

  const zh = outline(zhText);
  const en = outline(enText);
  if (zh.unbalanced) fail(base, "unbalanced ``` fence");
  if (en.unbalanced) fail(english, "unbalanced ``` fence");

  // Same skeleton: a section added to one language and forgotten in the other is
  // exactly the drift this check exists to catch.
  const zhShape = zh.headings.map((h) => h.depth).join(",");
  const enShape = en.headings.map((h) => h.depth).join(",");
  if (zhShape !== enShape) {
    const at = zh.headings.findIndex((h, i) => !en.headings[i] || en.headings[i].depth !== h.depth);
    if (at < 0) {
      // One side has trailing sections the other never got.
      const longer = en.headings.length > zh.headings.length ? en : zh;
      const shorter = longer === en ? zh : en;
      const name = longer === en ? english : base;
      fail(name, `has ${longer.headings.length} headings, the other side ${shorter.headings.length} — ` +
        `the extra one starts at "${longer.headings[shorter.headings.length].text}"`);
    } else {
      fail(english, `heading structure differs at heading #${at + 1} ` +
        `("${zh.headings[at].text}" vs "${en.headings[at]?.text ?? "—"}")`);
    }
  }
  if (zh.blocks !== en.blocks) {
    fail(english, `${en.blocks} code blocks against ${zh.blocks} — a block is missing or extra`);
  }

  // Code blocks and quoted examples may legitimately contain Chinese; headings
  // may not, because the headings are the document's table of contents.
  const cjkHeadings = en.headings.filter((h) => cjk(h.text) > 0);
  if (cjkHeadings.length > 0) {
    fail(english, `headings must be English: ${cjkHeadings.map((h) => `"${h.text}"`).join(", ")}`);
  }

  if (verbose) {
    console.log(`  ok  ${base} + ${posix.basename(english)} — ${zh.headings.length} headings, ${zh.blocks} code blocks`);
  }
  return true;
}

let pairs = 0;
let inline = 0;
let prompts = 0;

for (const file of files) {
  if (RUNTIME_PROMPTS.test(file)) {
    prompts += 1;
    continue;
  }
  if (INLINE_BILINGUAL.test(file)) {
    // One implementation of the inline rule, not a copy of it here.
    const tag = posix.basename(file, ".md");
    try {
      execFileSync("node", [join(root, "scripts", "notes-check.mjs"), tag], { cwd: root, stdio: "pipe" });
      inline += 1;
      if (verbose) console.log(`  ok  ${file} — inline bilingual (notes-check ${tag})`);
    } catch (err) {
      fail(file, `inline bilingual layout rejected by notes-check ${tag}: ${String(err.stdout ?? err.message).trim()}`);
    }
    continue;
  }

  // An `.en.md` is checked through its base so a pair is reported once.
  if (file.endsWith(".en.md")) {
    const base = file.slice(0, -".en.md".length) + ".md";
    if (!known.has(base)) fail(file, "English mirror with no Chinese original");
    continue;
  }
  const { base, english } = pairOf(file);
  if (existsSync(join(root, english)) && checkPair(base, english)) pairs += 1;
}

if (problems.length > 0) {
  for (const problem of problems) console.error(`::error::${problem}`);
  console.error("  rule: every .md is either a mirrored pair (X.md + X.en.md),");
  console.error("        inline bilingual (docs/release-notes), or a runtime prompt");
  console.error("        under src-tauri/src/skills/. See CONTRIBUTING.md, section 文档.");
  process.exit(1);
}

console.log(
  `docs: bilingual — ${pairs} mirrored pair(s), ${inline} inline release note(s), ` +
    `${prompts} runtime prompt(s) left untranslated`,
);
