# Beta 0.2 · 发行说明 / Release Notes

简体中文在前，English follows below. Both halves describe the same release; keep them in sync.

---

## 简体中文

Beta 0.1 的安装包有三个会让人以为程序坏了的问题，Beta 0.2 全部处理掉了。另外这是第一个带 Linux 包的版本。

### 安装后不再跳过初始化

开发时的调试实例和安装版共用同一个配置目录（`%APPDATA%\IRIXEN Code`），只要调试时走过一次首次启动向导，安装版就会读到那份"已完成"的标记，直接进主界面、停在"未配置供应商"，看起来像初始化没跑。

现在安装程序在**全新安装**时会先把旧的 `config.json` 备份成 `config.json.preinstall` 再清除，向导一定会出现；**覆盖安装**则原样保留你的所有设置。

### 重复启动不再崩溃

已经有一个窗口在运行时再次双击快捷方式，第二个进程会因为 WebView2 的用户数据目录被占用而直接退出——没有窗口、没有报错，只有一行 `0x8007139F` 留在控制台。现在加了单实例保护：第二次启动会把已有窗口唤到前台，然后干净退出。

### 关于页多了"维护"

设置 → 关于 → 维护，提供两个按钮：

- **重新运行首次启动设置** —— 想重挑主题、语言、工作区时用
- **恢复默认设置** —— 重置前自动备份成 `config.json.bak-<时间戳>`

### 首个 Linux 版本

| 平台 | 文件 |
| --- | --- |
| Windows x64 | `IRIXEN.Code_0.2.0-beta.1_x64-setup.exe` |
| Linux x64（通用） | `IRIXEN_Code_0.2.0-beta.1_amd64.AppImage` |
| Linux x64（Debian/Ubuntu） | `IRIXEN_Code_0.2.0-beta.1_amd64.deb` |

```bash
# AppImage
chmod +x IRIXEN_Code_0.2.0-beta.1_amd64.AppImage && ./IRIXEN_Code_0.2.0-beta.1_amd64.AppImage

# deb
sudo apt install ./IRIXEN_Code_0.2.0-beta.1_amd64.deb
```

Linux 版需要系统已安装 `libwebkit2gtk-4.1-0` 与 `libgtk-3-0`（deb 会自己拉依赖，AppImage 不会）。Ubuntu 22.04+ / Debian 12+ 可直接运行。

### 已知说明

- Windows 安装包带**自签名证书**（签名者 `CN=IRIXEN`）。它能压掉部分杀软对"未签名安装包"的启发式告警，但证书链不被系统信任，SmartScreen 仍可能提示"未知发布者"，首次运行时点"更多信息 → 仍要运行"。要彻底消掉这个提示需要 OV/EV 证书。
- 多引擎扫描（VirusTotal）对 Beta 0.1 的安装包出现过 1/28 的误报（Avira `TR/Hijacker.Gen`）：新版程序、用户量小、无签名时，这类"通用"家族名基本都是低信誉度触发，而不是真的行为。若再次出现，可直接向对应引擎提交误报，附上本仓库源码地址。
- Linux 版是**首个发布**，尚未在真机跑过：CI 里只做了静态校验（包结构、ELF 架构、动态库清单、desktop entry 与图标）。遇到启动即退出，请附上终端输出。
- 每日构建仍在 `nightly` 预发布频道，随时会变，不会自动推送给正式版用户。
- 从 Beta 0.1 升级：应用内"检查更新"会提示，也可以直接下载上面的安装包覆盖安装（设置会保留）。

---

## English

Beta 0.1 shipped with three problems that all looked like the app itself was broken. Beta 0.2 fixes every one of them, and adds the first Linux builds.

### Setup no longer gets skipped after install

Dev instances and installed builds shared a single config directory (`%APPDATA%\IRIXEN Code`). Once a debugging session had walked through the first-run wizard, the installed build picked up that "already done" flag, jumped straight into the main window and sat there on "no provider configured" — as if setup had never run.

A **fresh install** now backs up any existing `config.json` to `config.json.preinstall` and then clears it, so the wizard always appears. An **over-the-top install** leaves all your settings untouched.

### Launching twice no longer crashes

Double-clicking the shortcut while a window was already open made the second process exit immediately: WebView2's user data directory was taken, so you got no window, no error dialog, just `0x8007139F` in the console. Single-instance handling is now in place — the second launch brings the existing window to the front and exits cleanly.

### A "Maintenance" section on the About page

Settings → About → Maintenance, with two buttons:

- **Run first-run setup again** — for when you want to re-pick the theme, language, or workspace
- **Reset to defaults** — backs the current config up to `config.json.bak-<timestamp>` first

### First Linux release

| Platform | File |
| --- | --- |
| Windows x64 | `IRIXEN.Code_0.2.0-beta.1_x64-setup.exe` |
| Linux x64 (generic) | `IRIXEN_Code_0.2.0-beta.1_amd64.AppImage` |
| Linux x64 (Debian/Ubuntu) | `IRIXEN_Code_0.2.0-beta.1_amd64.deb` |

```bash
# AppImage
chmod +x IRIXEN_Code_0.2.0-beta.1_amd64.AppImage && ./IRIXEN_Code_0.2.0-beta.1_amd64.AppImage

# deb
sudo apt install ./IRIXEN_Code_0.2.0-beta.1_amd64.deb
```

The Linux builds expect `libwebkit2gtk-4.1-0` and `libgtk-3-0` to be present (the deb pulls them in, the AppImage does not). Ubuntu 22.04+ / Debian 12+ run it as-is.

### Known notes

- The Windows installer carries a **self-signed certificate** (subject `CN=IRIXEN`). It calms some antivirus heuristics that flag unsigned installers, but the chain is not trusted by the OS, so SmartScreen may still say "unknown publisher" — click "More info → Run anyway" on first launch. Removing that prompt for good needs an OV/EV certificate.
- A multi-engine scan (VirusTotal) flagged the Beta 0.1 installer 1/28 (Avira, `TR/Hijacker.Gen`). Generic family names like that are almost always a low-reputation trigger — new binary, small user base, no signature — rather than real behaviour. If it recurs, submit a false-positive report to the engine and point it at this repository.
- The Linux build is a **first release** and has not been smoke-tested on real hardware: CI only verifies the packages statically (archive structure, ELF architecture, shared-library list, desktop entry, icons). If it exits on startup, please attach the terminal output.
- Nightly builds stay on the `nightly` pre-release channel. They change daily and are never pushed to stable users automatically.
- Upgrading from Beta 0.1: in-app "Check for updates" will offer it, or just download an installer above and install over the top — your settings are preserved.
