# Antivirus false-positive log

<p align="center">
  <strong>English</strong> &nbsp;|&nbsp; <a href="./antivirus.md">中文</a>
</p>

VirusTotal flags the Windows installer as `TR/Hijacker.Gen` under Avira; every other engine is clean. This file records the evidence, what has already been done about it, and how to report it again if it comes back.

## Known results

| Version | Installer SHA256 | Detections | Family |
| --- | --- | --- | --- |
| Beta 0.1 (unsigned) | `6bec4ff2…` | 1/28 | Avira `TR/Hijacker.Gen` |
| Beta 0.2 (self-signed, `CN=IRIXEN`) | `5dda64075b4ef06dcd4c708865179f67b47eb0d31505aa596197a7906afcfc8b` | 1/28 | Avira `TR/Hijacker.Gen` |

Signing did not change the verdict, so this is not a reputation problem caused by an unsigned binary — it is a **heuristic** reacting to what the installer does.

Raw reports (PDFs exported from VirusTotal, with the full engine list and the ATT&CK behaviour items):

- `docs/antivirus/Beta0.2-installer-5dda6407.pdf` — the Beta 0.2 installer; SHA256 matches the table above
- `docs/antivirus/sample-f0fcb54f.pdf` — the report for sample `f0fcb54f…`, which is not the same file as either installer above

Those two are the primary evidence behind the conclusions here, which is why they live in the repository instead of a one-line summary.

## Why it matches

The NSIS installer is a self-extracting PE, and generic heuristics mostly watch whether it touches anything outside its own directory. The Beta 0.1 / 0.2 installers copied and then deleted `%APPDATA%\IRIXEN Code\config.json` and `model-cache.json` during `PREINSTALL` — "an installer fiddling with config files under AppData" is the textbook signature of the Hijacker family.

The behaviour report on VirusTotal (the MITRE ATT&CK screens) is essentially the generic conclusion "this is an installer", not malicious behaviour:

| Report item | Actual source |
| --- | --- |
| Creates an executable in the user directory | Tauri's default `currentUser` install mode, installing into `%LOCALAPPDATA%\IRIXEN Code` |
| Creates shortcuts, registers an uninstaller | The installer doing its job |
| Changes the default program for a file type | Not us: `bundle.fileAssociations` is unset and no shell-association call exists in the code |
| Captures keystrokes / installs a message hook | The NSIS installer UI and the WebView2 runtime; there is no `SetWindowsHookEx` anywhere in the app |
| Tries to unhook the sandbox monitor | The sandbox misreading WebView2 / the installer; there is no anti-debugging and no unhooking in the code |
| Collects installed-program information | Reading the uninstall registry to decide whether this is an over-the-top install |

## What has already been done

Installation no longer touches `%APPDATA%`: the installer writes an empty `first-run.marker` inside **its own installation directory** only, and `config::adopt_fresh_install` backs up and clears the profile when the app starts. See `installer.nsh` and the "Installer and leftover profile" section of `docs/BUILD.md`.

## How to report it next time

Avira: <https://www.avira.com/en/support-threats> (choose False Positive, attach the file or its SHA256).

> Subject: False positive — IRIXEN Code installer flagged as TR/Hijacker.Gen
>
> The file `IRIXEN.Code_<version>_x64-setup.exe` (SHA256 `<hash>`) is flagged as
> `TR/Hijacker.Gen`. It is an NSIS installer for an open-source desktop IDE built
> with Tauri; the full source is at https://github.com/lanche-furry/IRIXEN-Code
> and the installer script is `src-tauri/installer.nsh` in that repository.
>
> The detection looks like a heuristic on the installer writing to `%APPDATA%`.
> That behaviour has been removed: the installer now only creates an empty marker
> file inside its own installation directory, and the application cleans up its
> own profile on first launch. The binary is signed with a self-signed certificate
> (`CN=IRIXEN`), which is expected for a pre-release build.
>
> Please review and whitelist. Thank you.

You can also use "Community → Submit false positive" on the specific file in VirusTotal (if you have the permission) and paste the text above.

## Long-term fix

- **OV / EV code-signing certificate**: clears both the reputation verdicts and SmartScreen's "unknown publisher". It is the only way to satisfy every engine at once.
- Heuristic false positives fade naturally as the user base grows — but they can still come back for a new version.
