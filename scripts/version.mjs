#!/usr/bin/env node
// Version bumps for a GitHub-managed release.
//
// The semver lives in three manifests and must never drift apart: Cargo
// compiles it into the exe, Tauri stamps it into the bundle metadata, and npm
// exposes it to the frontend build. A mismatch makes the About page report a
// version that the installer does not actually carry.
//
//   node scripts/version.mjs                        # show current versions
//   node scripts/version.mjs 0.2.0-beta.1          # bump all three
//   node scripts/version.mjs 0.2.0-beta.1 --label "Beta 0.2"
//
// The human-facing label (`Beta 0.1`) is deliberately separate from semver;
// only pass --label when the product name changes.

import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const files = {
  pkg: path.join(root, 'package.json'),
  conf: path.join(root, 'src-tauri', 'tauri.conf.json'),
  cargo: path.join(root, 'src-tauri', 'Cargo.toml'),
  lock: path.join(root, 'src-tauri', 'Cargo.lock'),
  update: path.join(root, 'src-tauri', 'src', 'update.rs'),
};

const read = (p) => readFileSync(p, 'utf8');
const semverRe = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

// WiX wants a four-field numeric ProductVersion, so `0.4.0-beta.1` cannot be
// written into the MSI as-is — `tauri build` stops with "optional pre-release
// identifier in app version must be numeric-only". Map the semver onto
// `<core>.<n>`, n being the last number in the pre-release (Beta 0.4's
// `0.4.0-beta.1` becomes `0.4.0.1`; a plain `0.5.0` becomes `0.5.0.0`).
// The fourth field is what Windows orders upgrades by, so it has to move.
function msiVersion(v) {
  const [core, pre = ''] = v.split('+')[0].split('-');
  const numbers = pre.match(/\d+/g) ?? [];
  const build = numbers.length ? Number(numbers[numbers.length - 1]) : 0;
  if (build > 65535) {
    console.error(`[irixen] pre-release number ${build} does not fit the MSI build field (max 65535)`);
    process.exit(1);
  }
  return `${core}.${build}`;
}

function currentVersions() {
  const pkg = JSON.parse(read(files.pkg));
  const conf = JSON.parse(read(files.conf));
  const cargo = /^version\s*=\s*"([^"]+)"/m.exec(read(files.cargo))?.[1] ?? '?';
  const label = /pub const DISPLAY_VERSION:\s*&str\s*=\s*"([^"]+)"/.exec(read(files.update))?.[1] ?? '?';
  const msi = conf.bundle?.windows?.wix?.version ?? '?';
  return { package: pkg.version, tauri: conf.version, cargo, msi, label };
}

const before = currentVersions();
const args = process.argv.slice(2);
if (args.length === 0) {
  console.log('[irixen] current versions');
  for (const [k, v] of Object.entries(before)) console.log(`  ${k.padEnd(9)} ${v}`);
  const same =
    before.package === before.tauri && before.tauri === before.cargo && before.msi === msiVersion(before.package);
  console.log(same ? '  manifests in sync' : '  WARNING: manifests differ — run a bump to fix');
  process.exit(same ? 0 : 1);
}

const next = args[0];
if (!semverRe.test(next)) {
  console.error(`[irixen] not a valid semver: ${next}`);
  console.error('  expected e.g. 0.1.0, 0.2.0-beta.1, 1.0.0-rc.2');
  process.exit(1);
}
// npm strips quotes before handing argv to the script, so `--label "Beta 0.2"`
// arrives as three separate words. Everything after --label up to the next
// flag belongs to the label; `--label=Beta 0.2` is accepted too.
function readLabel(argv) {
  const inline = argv.find((a) => a.startsWith('--label='));
  if (inline) return inline.slice('--label='.length).trim();
  const at = argv.indexOf('--label');
  if (at < 0) return null;
  const words = [];
  for (const a of argv.slice(at + 1)) {
    if (a.startsWith('-')) break;
    words.push(a);
  }
  return words.join(' ').trim();
}

const label = readLabel(args);
if (args.includes('--label') || args.some((a) => a.startsWith('--label='))) {
  if (!label) {
    console.error('[irixen] --label needs a value, e.g. --label "Beta 0.2"');
    process.exit(1);
  }
}

// Replace the version in place instead of re-serialising: JSON.stringify would
// expand every inline array in tauri.conf.json and bury the real change in
// formatting noise.
const versionField = /("version"\s*:\s*")[^"]*(")/;

const writeJsonVersion = (p) => {
  const raw = read(p);
  if (!versionField.test(raw)) {
    console.error(`[irixen] no "version" field found in ${p}`);
    process.exit(1);
  }
  // Bumping to the version that is already there is a no-op, not an error.
  writeFileSync(p, raw.replace(versionField, `$1${next}$2`));
};

writeJsonVersion(files.pkg);
writeJsonVersion(files.conf);

// The MSI carries its own numeric version (see msiVersion above). Written here
// so a bump cannot leave the installer claiming the previous release, which
// Windows would either refuse to install over or treat as a downgrade.
const msi = msiVersion(next);
let conf = read(files.conf);
if (/"wix"\s*:\s*\{[\s\S]*?"version"/.test(conf)) {
  conf = conf.replace(/("wix"\s*:\s*\{[\s\S]*?"version"\s*:\s*")[^"]*(")/, `$1${msi}$2`);
} else {
  conf = conf.replace(/("wix"\s*:\s*\{)/, `$1\n        "version": "${msi}",`);
}
if (!conf.includes(`"${msi}"`)) {
  console.error('[irixen] could not write bundle.windows.wix.version in tauri.conf.json');
  process.exit(1);
}
writeFileSync(files.conf, conf);

let cargo = read(files.cargo);
cargo = cargo.replace(/^version\s*=\s*"[^"]+"/m, `version = "${next}"`);
writeFileSync(files.cargo, cargo);

// Cargo.lock pins the root package version too; leaving it stale makes the
// next build rewrite the lockfile in the release commit.
if (existsSync(files.lock)) {
  let lock = read(files.lock);
  lock = lock.replace(
    /(\[\[package\]\]\r?\nname = "irixen-code"\r?\nversion = ")[^"]+(")/,
    `$1${next}$2`,
  );
  writeFileSync(files.lock, lock);
}

if (label) {
  let update = read(files.update);
  update = update.replace(
    /pub const DISPLAY_VERSION:\s*&str\s*=\s*"[^"]+"/,
    `pub const DISPLAY_VERSION: &str = ${JSON.stringify(label)}`,
  );
  writeFileSync(files.update, update);
}

console.log(`[irixen] version set to ${next}${label ? ` (label: ${label})` : ''}`);
console.log('  package.json / tauri.conf.json / Cargo.toml / Cargo.lock');
console.log('');
const releaseTag = (label ?? next).replace(/\s+/g, '');
console.log('Next steps:');
console.log('  1. commit the bump (manifests + src-tauri/src/update.rs label)');
console.log(`  2. git tag ${releaseTag} && git push origin ${releaseTag}`);
console.log('     pushing the tag runs .github/workflows/release.yml: it builds');
console.log('     Windows and Linux and publishes the release on the latest channel.');
console.log('  3. notes come from docs/release-notes/<tag>.md when that file exists,');
console.log('     otherwise GitHub generates them from the commit range.');
console.log('     Write that file in the Beta 0.1 layout: one document, `中文 / English`');
console.log('     on every heading and bullet, prose as a Chinese then an English paragraph.');
console.log(`     Tags are the product label with no leading "v" and no spaces (${releaseTag}).`);
