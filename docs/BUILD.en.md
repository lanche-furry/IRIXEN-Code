# Building and toolchain

<p align="center">
  <strong>English</strong> &nbsp;|&nbsp; <a href="./BUILD.md">中文</a>
</p>

## Prerequisites

| Needed | Notes |
| --- | --- |
| Node.js 18+ | Front-end build, Tauri CLI |
| Rust 1.77+, **GNU target** | This repository targets `x86_64-pc-windows-gnu` and does not depend on MSVC |
| WebView2 Runtime | Ships with Windows 11; on Windows 10 the installer's `downloadBootstrapper` covers it |

## Portable toolchain (the default here)

To avoid polluting the system, Rust and the build tools live inside the repository under `.tools/`:

```
.tools/
  cargo/                  # CARGO_HOME (registry cache + cargo.exe)
  rustup/                 # RUSTUP_HOME (toolchains/…-windows-gnu)
  mingw64/                # dlltool and other binutils (only needed to build import libraries)
```

These directories are listed in `.gitignore` and never enter version control.

`scripts/build.mjs` wires up the environment variables and then hands over to cargo or the Tauri CLI:

```bash
npm run check                 # cargo check (produces no executable)
npm run app:dev               # tauri dev (vite + window, development only)
npm run app:release           # tauri build --no-bundle (release exe only)
npm run app:build             # tauri build (NSIS installer)
node scripts/build.mjs tauri dev --no-watch
```

What it does:

```js
env.RUSTUP_HOME = .tools/rustup
env.CARGO_HOME  = .tools/cargo
env.PATH        = .tools/mingw64/bin : toolchain/bin : cargo/bin : $PATH
env.CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = <toolchain>/lib/rustlib/x86_64-pc-windows-gnu/bin/rust-lld.exe
```

### Why the linker is not gcc

MinGW's gcc derives `specs` and `default-manifest.o` from its own install path, and that path is **not quoted** internally. As soon as the install path contains a space (for example `E:\IRIXEN Code\.tools\mingw64`), linking fails with:

```
ld.exe: cannot find E:/IRIXEN: Permission denied
ld.exe: cannot find Code/.tools/.../default-manifest.o: No such file or directory
```

The way around it: link with Rust's own `rust-lld` (the GNU target already bundles self-contained MinGW runtime libraries — see `lib/rustlib/x86_64-pc-windows-gnu/lib/self-contained/`). MinGW stays on `PATH` because `rustc` still calls `dlltool.exe` when generating import libraries.

`CARGO_TARGET_<TRIPLE>_LINKER` is cargo's environment-variable form, so paths with spaces work natively — safer than stuffing it into `RUSTFLAGS`, which splits on whitespace.

## Building with the system toolchain (optional)

If your machine already has Rust + MinGW installed (at a path without spaces), you can ignore `.tools/` entirely:

```bash
rustup default stable-x86_64-pc-windows-gnu
npm install
npx tauri build
```

The MSVC toolchain works too, it just links the VC runtime and comes out slightly larger:

```bash
rustup default stable-x86_64-pc-msvc
npx tauri build
```

## Linux builds

Off Windows there is exactly one supported path, and Linux bundles must be named explicitly — `bundle.targets` in `tauri.conf.json` says `["nsis"]`:

```bash
npx tauri build --bundles deb,appimage
```

Dependencies (Ubuntu/Debian):

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev patchelf
```

Bundles land in `src-tauri/target/release/bundle/{deb,appimage}/`. Two traps:

- **The AppImage does not bundle WebKit.** It only packs non-system libraries, so the host still needs `libwebkit2gtk-4.1-0`; CI builds on `ubuntu-22.04` (glibc 2.35), so 22.04 and newer run it as-is.
- **The deb's `Depends` must add `libssl3`.** `reqwest` uses native-tls and the binary links `libssl.so.3` / `libcrypto.so.3` directly; leaving it undeclared means the app installs fine on a slim system and then fails to start with no message. Note that Tauri **appends** its own defaults (`libwebkit2gtk-4.1-0`, `libgtk-3-0`) after your array rather than replacing it, so `bundle.linux.deb.depends` only needs the incremental `["libssl3"]` — listing everything just duplicates entries.

Nightlies are described in `.github/workflows/nightly.yml`: Windows and Linux each build their bundles, and a final publish job pushes both to the rolling `nightly` pre-release so the two platforms never fight over one tag.

**GitHub's `schedule` is not a timer.** The docs say plainly that the top of the hour is the busiest slot for scheduling: `schedule` events routinely arrive minutes late, occasionally more than an hour late, and sometimes not at all that day. Only the copy of the workflow file on the **default branch** counts, cron included — a schedule edited on another branch does nothing. Manual `workflow_dispatch` and the schedule are independent triggers: a manual run neither cancels that day's scheduled run nor takes its slot (the shared `concurrency` group at most makes them queue).

That is why the cron reads **`7 12 * * *`** — 12:07 UTC, 20:07 Beijing, **deliberately off the hour**. The top of the hour is the most congested moment in the scheduler queue every repository shares; a `0 12` cron arrives late often and gets dropped outright on a busy day. Nothing downstream cares whether it is exactly 12:00, so seven minutes cost nothing.

So when "no build today" comes up, filter the run list by `event=schedule` first: a record that failed is a build problem — read the log; no record at all means the scheduler never fired, so trigger it by hand.

## Icons

```bash
npm run icons
```

`scripts/gen-icons.mjs` takes `assets/icon.png` (a high-resolution transparent master, centre-cropped to a square), normalises it to 1024 px, then resamples with lanczos3 into 16/32/48/64/128/256/512/1024, the standard Tauri icon names, the Windows Store logos, `icon.ico`, and a hand-written `icon.icns` (ic07/ic08/ic09/ic10 segments). The same 512 px image is written to `public/irixen-icon.png` for the splash screen, the first-run wizard, the title bar and the editor empty state to reuse in the renderer. To change the logo, replace `assets/icon.png` and run `npm run icons`.

**After changing icons you must re-run `npm run app:release`.** The exe icon is written into the PE resource section by the Windows resource compiler in `build.rs`, and Tauri's default window icon is compiled into the binary by `generate_context!()`; replacing `src-tauri/icons/*` alone lets Cargo reuse the old `.res` from cache, so the taskbar and the exe keep showing the old icon. `build.rs` already declares `icon.ico` / `icon.png` / `icon.icns` as `cargo:rerun-if-changed`, so a normal incremental build re-runs the resource compiler; if you ever ran `cargo clean -p irixen-code` by hand, just rebuild.

## Artifacts

```
E:/irixen-build/release/irixen-code.exe
E:/irixen-build/release/bundle/nsis/IRIXEN Code_0.1.0-beta.1_x64-setup.exe
```

`CARGO_TARGET_DIR` is pinned to the drive root at `E:/irixen-build` (the path must not contain spaces, or MinGW's `windres` preprocessing fails). Commands like `node scripts/build.mjs tauri dev/build` have to go through the Tauri CLI: a bare `cargo build` has no build context, so it loads the `devUrl` at `http://localhost:5173/` instead of the embedded `dist/`, and the window shows "localhost refused to connect".

**No debug builds.** `build.mjs` disables `dev` / `run`, a bare `build` gets `--release` appended, and `cargo check` only type-checks without leaving an executable. Releases always go through `npm run app:release` (exe only) or `npm run app:build` (adds the NSIS package). The version shown in the UI is `Beta 0.1` throughout, while the package metadata stays a valid semver `0.1.0-beta.1`.

The release profile is tuned for size:

```toml
opt-level = "s"      # size first
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

## Debugging the UI

Tauri's devtools shortcut is enough to see errors, but **measuring layout** needs CDP. WebView2 honours `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`:

```bash
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 npm run app:dev
```

After that `http://127.0.0.1:9222/json/list` lists the page, and any CDP client (even a 30-line `WebSocket` script) can run `Runtime.evaluate` to read `getBoundingClientRect()`, take screenshots, or call commands directly through `window.__TAURI_INTERNALS__.invoke(...)` — with no debug code added to the app.

The routine when chasing a layout problem: measure `getBoundingClientRect()` for `.app / .body / .sidebar / .stage / .chat`, plus `scrollWidth > clientWidth`. If any layer is much wider than the window, min-content is blowing the grid out (see [Architecture · Layout contract](ARCHITECTURE.en.md#9-layout-contract)).

### Test instances must isolate the config directory (`IRIXEN_HOME`)

The window reads the **real user profile** at `%APPDATA%\IRIXEN Code` (`config::base_dir`). Click through the first-run wizard over CDP, change the theme, pick a provider, and you have just written that real profile; install the NSIS package afterwards and the installed app believes it is "already set up", so **the first-run setup never appears** — which looks exactly like an install that skipped initialisation.

So any automated or manual verification must point the config at a scratch directory:

```bash
IRIXEN_HOME="E:/irixen-build/_probe-home" \
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9555 \
  ./irixen-build/release/irixen-code.exe
```

`IRIXEN_HOME` also takes over the location of config.json, sessions, extensions and skills, so deleting the whole directory is equivalent to a clean install. Do not run probes against the default directory afterwards.

### Installer and leftover profile

`src-tauri/installer.nsh` is hooked into NSIS through `bundle.windows.nsis.installerHooks`. **The installer does not touch `%APPDATA%`** — the app does that itself:

- **Clean install** (`$UpdateMode <> 1` and no exe in `$INSTDIR`): the installer writes an empty `first-run.marker` inside **its own installation directory**. On startup (`config::adopt_fresh_install`) the app consumes it: it backs `config.json` up to `config.json.preinstall`, deletes the live config and `model-cache.json`, and the wizard therefore runs. An over-the-top upgrade writes no marker, so every setting is preserved.
- **Uninstall with "delete application data" ticked**: removes both `%APPDATA%\IRIXEN Code` (the real profile) and `%LOCALAPPDATA%\com.irixen.code` (the WebView2 cache). The template's built-in cleanup only knows the bundle id and cannot reach the `IRIXEN Code` directory.

Why the split: an installer that copies and deletes other directories' config files under `%APPDATA%` is textbook "hijacker" behaviour to generic antivirus heuristics — Avira flagged the signed Beta 0.2 installer as `TR/Hijacker.Gen` (1 of 28 engines), and Beta 0.1 got the same family name. An app managing its own profile is unremarkable.

Machines that are already broken do not need a reinstall: Settings → About → Maintenance → **Reset to defaults** (`config_reset` backs the old file up to `config.json.bak-<timestamp>` and reloads the window, so the wizard reappears), or **Run first-run setup again** from the palette.

## Continuous integration

Three workflows with non-overlapping jobs:

| File | Trigger | What it does |
| --- | --- | --- |
| `.github/workflows/ci.yml` | push to `main`, any PR, manual | Fast check on both platforms (ubuntu-22.04 + windows-latest): `npm run build` (tsc + vite), `cargo check --all-targets`, `cargo test --lib`. **No bundling, no publishing** |
| `.github/workflows/nightly.yml` | daily at 12:07 UTC (20:07 Beijing, deliberately off the hour), manual | Bundles both platforms and publishes to the rolling `nightly` **pre-release** channel |
| `.github/workflows/release.yml` | pushing a `Beta*` / `v*` tag | Bundles both platforms and publishes to the **latest** stable channel |

`ci.yml` must run Windows: the interesting code sits behind `#[cfg(windows)]`, and a Linux-only run would not even compile a line of it. Conversely the Linux job also runs `cargo test`, because it produces results fastest and can stop a logic error before bundling.

`nightly` is a pre-release and its tag contains no digits, so `is_version_tag` excludes it — a nightly can never pop an upgrade prompt at regular users.

## Windows signing

After bundling, `release.yml` calls `scripts/sign-windows.ps1` to add an Authenticode signature (SHA256 + DigiCert timestamp) to `irixen-code.exe` and the NSIS installer. The certificate comes from two repository secrets:

| Secret | Contents |
| --- | --- |
| `WINDOWS_SIGN_PFX` | base64 of the `.pfx` (`[Convert]::ToBase64String([IO.File]::ReadAllBytes('x.pfx'))`) |
| `WINDOWS_SIGN_PFX_PASSWORD` | the password set when the PFX was exported |

If either secret is missing the signing step is skipped and the workflow emits a warning and keeps publishing — signing is a release enhancement, not a single point of failure.

**Where the self-signed certificate stands**: the file carries a `CN=IRIXEN` signer, so heuristics about "unsigned binaries" no longer fire; but the chain is untrusted on users' machines, so SmartScreen may still warn about an unknown publisher and users have to click "More info → Run anyway". Only an OV/EV code-signing certificate removes that prompt; swap the new PFX into the same secret and the script needs no changes.

**Signing locally** (certificate and password in environment variables):

```powershell
$env:SIGN_PFX = Get-Content E:\irixen-build\signing\pfx.base64 -Raw
$env:SIGN_PFX_PASSWORD = Get-Content E:\irixen-build\signing\password.txt -Raw
.\scripts\sign-windows.ps1 src-tauri\target\release\irixen-code.exe
```

The exe embedded in the installer is compressed at bundle time, so **post-hoc signing only covers the installer itself and the standalone exe**. Signing the embedded exe too would mean hooking the same script into `bundle.windows.signCommand` (which would force every local build to carry a certificate, hence not enabled by default).

## Versioning (GitHub Releases)

The source of truth is semver, written in three places that must always agree (otherwise the version the About page reports and the version the installer actually carries diverge):

| File | Field |
| --- | --- |
| `package.json` | `version` |
| `src-tauri/tauri.conf.json` | `version` |
| `src-tauri/Cargo.toml` | `[package].version` (compiled into the exe) |

The `Beta 0.1` shown in the UI is a product label kept separately in `DISPLAY_VERSION` in `src-tauri/src/update.rs`, and is not required to equal the semver. `npm run bump` (with no arguments) checks that the three places agree.

**Cutting a release**

```bash
npm run bump 0.2.0-beta.1 --label "Beta 0.2"   # three manifests + Cargo.lock + DISPLAY_VERSION
git add -A && git commit -m "release: Beta 0.2"
git push origin main
git tag Beta0.2 && git push origin Beta0.2
```

Pushing the tag triggers `.github/workflows/release.yml`: Windows builds NSIS, Linux builds deb + AppImage, and the result is published to the **latest channel** (`--latest`, non-prerelease), so the in-app "check for updates" treats it as the current version. Notes come from `docs/release-notes/<tag>.md`; if that file does not exist, GitHub's generated notes are used.

Publishing also writes the output of `sha256sum *` to `checksums.txt` and uploads it. **That file is the prerequisite for in-app one-click updates**: "Download and install" on the About page fetches it first and verifies the downloaded installer against the SHA256 recorded there; a mismatch deletes the file and reports an error, with no "continue anyway" option. The `nightly` channel produces it too.

The split inside the updater is deliberate: `check` in `update.rs` only reads metadata (cheap enough to run whenever the About page opens); `download` fetches this platform's installer and verifies it; `apply` launches the installer and exits immediately — a Windows exe cannot replace itself, so installing is always a separate process and the app never pretends it can self-replace. Linux only installs the AppImage (the deb needs root). Changing the `checksums.txt` format means changing `parse_checksums()` with it, which has unit tests.

**Release notes use the Beta 0.1 layout: one document, both languages on every line**, never "a Chinese half plus an English half":

```markdown
# IRIXEN Code Beta 0.2

中文导语（这一版解决了什么）。

English intro, same content.

## 修复 / Fixes

### 安装后不再跳过初始化 / Setup is no longer skipped after install

中文段落（需要讲清因果的地方）。

English paragraph saying the same thing.

- 中文要点 / English bullet point
```

- Title line, `##`/`###` headings, `-` bullets: always `中文 / English`, with **one space on each side of the slash** (the script splits on that).
- Long prose is two paragraphs: Chinese first, blank line, then English.
- Commands, file names and version numbers appear in both halves — never make English readers scroll back to the Chinese paragraph to find `chmod +x`.
- Never write `## 简体中文` / `## English` halves: a reader who wants one language then has to skip the entire translation.

`docs/release-notes/Beta0.2.md` is the reference sample; the `nightly` channel's notes are generated on the fly by `nightly.yml` in the same layout, so no file is needed.

The rule has a gate instead of relying on discipline: `npm run notes:check -- Beta0.2` verifies the title is `# IRIXEN Code <version>`, that every heading line and bullet is `中文 / English` (at least 6 bilingual bullets), and that both languages have enough characters — failing it names the offending line. `release.yml` runs the same script (`scripts/notes-check.mjs`) before publishing and stops before uploading. To fix the body of a release that is already out, there is no need to rebuild both platforms' packages: edit it directly with a token, which touches the body only and leaves the assets alone.

```bash
TOKEN=…   # fine-grained PAT with contents: write
GH_TOKEN=$TOKEN gh release edit Beta0.2 --repo lanche-furry/IRIXEN-Code \
  --notes-file docs/release-notes/Beta0.2.md
```

**Never create a release by hand**: a hand-made release carries no Linux packages and tends to fight the tag-triggered run over the same tag. Before publishing, the workflow compares the numeric form of the tag against the `package.json` version and fails outright on a mismatch — precisely to block "forgot to bump, released anyway".

**How tags relate to versions**: tags use the **product label, without `v` and without spaces** (`Beta0.1`, `Beta0.2`). `compare_semver` parses `Beta0.2` into `0.2`, so `Beta0.2` counts as newer than `0.1.0-beta.1`. But the label carries no pre-release suffix, so against `0.2.0-beta.1` it would rank higher; that is why `from_release` first compares product labels with `is_current_label` and declares "no update" on a match — otherwise someone who just installed Beta0.2 would be told to upgrade to Beta0.2. `nightly` is excluded from version comparison by `is_version_tag`: it is a daily build, not a version.

**Checking for updates in the app**: Settings → About → Check for updates. `app_check_update` reads `/releases/latest`, falls back to the `/releases` list on a 404 (for repositories with only drafts or pre-releases), and shows "nothing published on GitHub yet" with a link to the releases page when that is empty too. Results are cached for 10 minutes and the button forces a refresh; requests go through `net::build_client` and honour the proxy in Settings → Network. The app **only reads** metadata and opens the download page — it does not download or replace the running exe by itself, because a running exe on Windows cannot be overwritten and users should see the release notes first.

**Details of an installed plugin**: Extensions panel → Installed → "Details" on each card. `extensions_inspect` reads the local install directory entirely offline (`package.json`, README, CHANGELOG, LICENSE, directory size, contribution points and activation events) without touching Open VSX; `extensions_detail` in the store still serves online packages, and the two never interfere.

## FAQ

**`error calling dlltool 'dlltool.exe': program not found`**
MinGW's `bin` is not on `PATH`. Start through `npm run check` / `npm run app:build` and the script adds it.

**`cannot find E:/IRIXEN: Permission denied`**
The linker fell back to gcc and the install path contains a space. Check that `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` points at `rust-lld.exe`.

**The window is blank, no UI at all**
The window is created with `visible: true`, so a blank window usually means the front end crashed on first paint: open devtools and read the first console error. On a cold `vite` start (first dependency pre-bundling) the solid-colour window can last several seconds, and the `[irixen] ui mounted after Nms` log gives the real timing; a production build is a single local bundle and usually takes under a second.

**`npm run app:dev` says the port is taken**
`vite.config.ts` sets `strictPort: true`, so a busy 5173 fails instead of picking another port. Free 5173 first.

**Web2API will not start (default 8791 taken)**
The port is editable in the `Web2API` panel and saving restarts the listener automatically; the reason a start failed is shown on the panel's status line (`last_error`) rather than exiting silently. Note that it only binds `127.0.0.1` — it does not need to, and should not, be exposed to the LAN.
