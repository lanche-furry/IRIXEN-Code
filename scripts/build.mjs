#!/usr/bin/env node
// Portable toolchain launcher.
//
// Everything IRIXEN Code needs to build lives in .tools/ — no system-wide Rust,
// no global MinGW. This wrapper resolves that layout and hands off to cargo or
// the Tauri CLI with the right environment.
//
//   node scripts/build.mjs check          # cargo check (no binary shipped)
//   node scripts/build.mjs tauri dev      # tauri dev (development only)
//   node scripts/build.mjs tauri build --no-bundle   # release exe only
//   node scripts/build.mjs tauri build    # NSIS installer
//
// No debug binary is ever produced. A bare `cargo build` / `cargo run` does
// not go through the Tauri CLI build context: it resolves `devUrl` instead of
// the embedded `frontendDist`, so the window opens "localhost refused to
// connect". Release is the only distributable profile.
//
// The linker is rust-lld (shipped with the toolchain) because MinGW's gcc
// specs break on install paths that contain spaces. dlltool from the bundled
// MinGW is still on PATH for import-library generation.

import { spawn } from 'node:child_process';
import { existsSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
// The toolchain is 2.6 GB and gitignored; it can live outside the tree (this
// machine keeps it under the build root) via IRIXEN_TOOLS. `.tools/` stays the
// default so a checkout that already has it needs no configuration.
const tools = process.env.IRIXEN_TOOLS || path.join(root, '.tools');
const triple = 'x86_64-pc-windows-gnu';
const toolchain = path.join(tools, 'rustup', 'toolchains', `stable-${triple}`);
const cargoBin = path.join(tools, 'cargo', 'bin', 'cargo.exe');
const lld = path.join(toolchain, 'lib', 'rustlib', triple, 'bin', 'rust-lld.exe');
const mingwBin = path.join(tools, 'mingw64', 'bin');

const env = { ...process.env };
env.RUSTUP_HOME = path.join(tools, 'rustup');
env.CARGO_HOME = path.join(tools, 'cargo');

const pathParts = [path.join(toolchain, 'bin'), path.join(tools, 'cargo', 'bin')];
if (existsSync(mingwBin)) pathParts.unshift(mingwBin);
env.PATH = `${pathParts.join(path.delimiter)}${path.delimiter}${env.PATH || ''}`;

if (existsSync(lld)) {
  // Target-scoped linker var: no quoting pitfalls, unlike RUSTFLAGS.
  env[`CARGO_TARGET_${triple.toUpperCase().replace(/-/g, '_')}_LINKER`] = lld;
}

// MinGW's windres cannot preprocess paths that contain spaces, and cargo puts
// OUT_DIR under the target dir. Keep the build cache on a space-free path
// (artifacts are disposable; sources and the toolchain stay in the repo).
env.CARGO_TARGET_DIR = process.env.CARGO_TARGET_DIR || path.join(path.parse(root).root, 'irixen-build');

// Tauri on Windows defaults to the MSVC toolchain; pin ours.
env.TAURI_ENV_TARGET_TRIPLE = triple;

const args = process.argv.slice(2);
const mode = args.shift() || 'check';

let cmd;
let cmdArgs;

if (mode === 'tauri') {
  const cli = path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
  if (!existsSync(cli)) {
    console.error('Tauri CLI missing — run `npm install` first.');
    process.exit(1);
  }
  cmd = process.execPath;
  cmdArgs = [cli, ...args];
} else {
  if (!existsSync(cargoBin)) {
    console.error('Portable cargo missing at', cargoBin);
    console.error('Install the toolchain there, or point IRIXEN_TOOLS at an existing one.');
    process.exit(1);
  }
  cmd = cargoBin;
  // `dev`/`run` used to launch a 300 MB debug exe that loaded devUrl. That
  // binary is never shippable, so refuse it and point at the two real paths.
  if (mode === 'dev' || mode === 'run') {
    console.error(
      '[irixen] debug binaries are not shipped. Use `npm run app:dev` (Tauri CLI) ' +
        'to develop, or `npm run app:release` to build the release exe.',
    );
    process.exit(1);
  }
  // A bare `build` must not leave a debug exe behind either.
  if (mode === 'build' && !args.includes('--release')) args.unshift('--release');
  cmdArgs = [mode === 'check' ? 'check' : mode, ...args];
}

const child = spawn(cmd, cmdArgs, {
  cwd: path.join(root, 'src-tauri'),
  env,
  stdio: 'inherit',
  shell: false,
});

child.on('exit', (code) => process.exit(code ?? 0));
child.on('error', (err) => {
  console.error('[irixen] failed to launch:', err.message);
  process.exit(1);
});
