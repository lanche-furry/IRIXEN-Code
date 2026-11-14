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

function currentVersions() {
  const pkg = JSON.parse(read(files.pkg));
  const conf = JSON.parse(read(files.conf));
  const cargo = /^version\s*=\s*"([^"]+)"/m.exec(read(files.cargo))?.[1] ?? '?';
  const label = /pub const DISPLAY_VERSION:\s*&str\s*=\s*"([^"]+)"/.exec(read(files.update))?.[1] ?? '?';
  return { package: pkg.version, tauri: conf.version, cargo, label };
}

const before = currentVersions();
const args = process.argv.slice(2);
if (args.length === 0) {
  console.log('[irixen] current versions');
  for (const [k, v] of Object.entries(before)) console.log(`  ${k.padEnd(9)} ${v}`);
  const same = before.package === before.tauri && before.tauri === before.cargo;
  console.log(same ? '  manifests in sync' : '  WARNING: manifests differ — run a bump to fix');
  process.exit(same ? 0 : 1);
}

const next = args[0];
if (!semverRe.test(next)) {
  console.error(`[irixen] not a valid semver: ${next}`);
  console.error('  expected e.g. 0.1.0, 0.2.0-beta.1, 1.0.0-rc.2');
  process.exit(1);
}
const labelAt = args.indexOf('--label');
const label = labelAt >= 0 ? args[labelAt + 1] : null;
if (labelAt >= 0 && !label) {
  console.error('[irixen] --label needs a value, e.g. --label "Beta 0.2"');
  process.exit(1);
}

const writeJson = (p, mutate) => {
  const raw = read(p);
  const data = JSON.parse(raw);
  mutate(data);
  const eol = raw.includes('\r\n') ? '\r\n' : '\n';
  writeFileSync(p, JSON.stringify(data, null, 2).replace(/\n/g, eol) + eol);
};

writeJson(files.pkg, (d) => { d.version = next; });
writeJson(files.conf, (d) => { d.version = next; });

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
console.log('Next steps for a GitHub-managed release:');
console.log('  1. npm run build && npm run app:release');
console.log(`  2. git tag ${releaseTag} && git push origin ${releaseTag}`);
console.log('  3. Create a GitHub Release for that tag and attach the installer / exe.');
console.log('     Tags use the product label with no leading "v" and no spaces (e.g. Beta0.2).');
console.log('     The About page reads /releases/latest and compares its tag with this semver.');
