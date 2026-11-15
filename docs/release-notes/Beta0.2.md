# IRIXEN Code Beta 0.2

Beta 0.1 的安装包有三个会让人以为程序坏了的问题，这一版全部处理掉了；同时这是第一个带 Linux 包的版本。

Beta 0.1 shipped with three problems that all looked like a broken app. This release fixes every one of them, and adds the first Linux builds.

## 修复 / Fixes

### 安装后不再跳过初始化 / Setup is no longer skipped after install

开发时的调试实例和安装版共用同一个配置目录（`%APPDATA%\IRIXEN Code`）：只要调试时走过一次首次启动向导，安装版就会读到那份「已完成」的标记，直接进主界面、停在「未配置供应商」，看起来像初始化没跑。

Dev instances and installed builds shared one config directory (`%APPDATA%\IRIXEN Code`). Once a debugging session had walked through the first-run wizard, the installed build picked up that "already done" flag, jumped into the main window and sat on "no provider configured" — as if setup had never run.

- 全新安装后的第一次启动把旧的 `config.json` 备份成 `config.json.preinstall` 再清除，向导一定会出现；安装器本身不再写 `%APPDATA%` / On the first launch after a fresh install the old `config.json` is backed up to `config.json.preinstall` and cleared, so the wizard always appears — the installer itself no longer writes to `%APPDATA%` at all
- 覆盖安装：你的所有设置原样保留 / Over-the-top install: every setting is left untouched

### 重复启动不再崩溃 / Launching twice no longer crashes

已经有一个窗口在运行时再次双击快捷方式，第二个进程会因为 WebView2 的用户数据目录被占用而直接退出——没有窗口、没有报错，只有一行 `0x8007139F` 留在控制台。

Double-clicking the shortcut while a window was already open made the second process exit immediately: WebView2's user data directory was taken, so no window, no error dialog, just `0x8007139F` in the console.

- 第二次启动现在会把已有窗口唤到前台，然后干净退出 / The second launch now brings the existing window to the front and exits cleanly

### 关于页多了「维护」 / A "Maintenance" section on the About page

设置 → 关于 → 维护，提供两个按钮：

Settings → About → Maintenance, with two buttons:

- **重新运行首次启动设置** —— 想重挑主题、语言、工作区时用 / **Run first-run setup again** — for re-picking the theme, language or workspace
- **恢复默认设置** —— 重置前自动备份成 `config.json.bak-<时间戳>` / **Reset to defaults** — backs the current config up to `config.json.bak-<timestamp>` first

## 安装 / Install

- **Windows x64**：`IRIXEN.Code_0.2.0-beta.1_x64-setup.exe`，双击安装，会创建开始菜单与桌面快捷方式并自动补齐 WebView2 Runtime / `IRIXEN.Code_0.2.0-beta.1_x64-setup.exe` — run it; it creates Start Menu and desktop shortcuts and bootstraps the WebView2 Runtime
- **Linux x64（通用）**：`IRIXEN_Code_0.2.0-beta.1_amd64.AppImage`，`chmod +x` 后直接运行 / `IRIXEN_Code_0.2.0-beta.1_amd64.AppImage` — `chmod +x` it, then run it
- **Linux x64（Debian/Ubuntu）**：`sudo apt install ./IRIXEN_Code_0.2.0-beta.1_amd64.deb`，依赖会自动拉取 / `sudo apt install ./IRIXEN_Code_0.2.0-beta.1_amd64.deb` — dependencies come with it

Linux 版需要系统已安装 `libwebkit2gtk-4.1-0` 与 `libgtk-3-0`（deb 会自己拉依赖，AppImage 不会）。Ubuntu 22.04+ / Debian 12+ 可直接运行。

The Linux builds expect `libwebkit2gtk-4.1-0` and `libgtk-3-0` to be installed (the deb pulls them in, the AppImage does not). Ubuntu 22.04+ / Debian 12+ run it as-is.

## 说明 / Notes

- Windows 安装包带**自签名证书**（签名者 `CN=IRIXEN`），能压掉部分杀软对「未签名安装包」的启发式告警；但证书链不被系统信任，SmartScreen 仍可能提示「未知发布者」，首次运行点「更多信息 → 仍要运行」。彻底消掉需要 OV/EV 证书 / The installer is **self-signed** (`CN=IRIXEN`), which calms some antivirus heuristics about unsigned installers. The chain is not trusted by the OS, so SmartScreen may still warn about an unknown publisher — click "More info → Run anyway" on first launch. Removing that prompt for good needs an OV/EV certificate
- 多引擎扫描（VirusTotal）对 Beta 0.1 与 Beta 0.2 都是 1/28：Avira 报 `TR/Hijacker.Gen`，其余 27 家无检出。这类通用家族名来自启发式——新程序、用户量小、安装包会改动 `%APPDATA%`——这一版已把安装阶段的配置清理移进应用首次启动，安装器只写自己的安装目录；若仍被报，可向引擎提交误报并附上本仓库源码 / Multi-engine scans flag both Beta 0.1 and Beta 0.2 at 1/28: Avira says `TR/Hijacker.Gen`, the other 27 engines are clean. Generic family names like that come from heuristics — new binary, small user base, an installer that touches `%APPDATA%`. This release moved the profile cleanup out of the installer into the app's first launch, so the installer only writes its own directory; if it is still flagged, submit a false-positive report and point the engine at this repository
- Linux 版是**首个发布**，尚未在真机跑过：CI 只做了静态校验（包结构、ELF 架构、动态库清单、desktop entry 与图标），遇到启动即退出请附上终端输出 / The Linux build is a **first release** and has not been smoke-tested on real hardware: CI only verifies the packages statically (archive structure, ELF architecture, shared-library list, desktop entry, icons). If it exits on startup, please attach the terminal output
- 每日构建仍在 `nightly` 预发布频道，随时会变，不会自动推送给正式版用户 / Nightly builds stay on the `nightly` pre-release channel. They change daily and are never pushed to stable users automatically
- 从 Beta 0.1 升级：应用内「检查更新」会提示，也可以直接下载上面的安装包覆盖安装，设置会保留 / Upgrading from Beta 0.1: in-app "Check for updates" will offer it, or just download an installer above and install over the top — your settings are preserved

## 许可证 / License

- 许可证：Apache-2.0 / License: Apache-2.0
