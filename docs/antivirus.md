# 杀软误报记录

Windows 安装包在 VirusTotal 上被 Avira 判为 `TR/Hijacker.Gen`，其余引擎全部无检出。这份文件记录证据、已经做过的处置，以及再次出现时怎么报误报。

## 已知结果

| 版本 | 安装包 SHA256 | 检出 | 家族 |
| --- | --- | --- | --- |
| Beta 0.1（未签名） | `6bec4ff2…` | 1/28 | Avira `TR/Hijacker.Gen` |
| Beta 0.2（自签名 `CN=IRIXEN`） | `5dda64075b4ef06dcd4c708865179f67b47eb0d31505aa596197a7906afcfc8b` | 1/28 | Avira `TR/Hijacker.Gen` |

签名没有改变结论，说明这不是"未签名"触发的信誉问题，而是**启发式**看安装器做了什么。

原始报告（VirusTotal 导出的 PDF，含完整引擎列表与 ATT&CK 行为项）：

- `docs/antivirus/Beta0.2-installer-5dda6407.pdf` —— Beta 0.2 安装包，SHA256 与上表一致
- `docs/antivirus/sample-f0fcb54f.pdf` —— 样本 `f0fcb54f…` 的报告，与上表两个安装包都不是同一个文件

这两份是上面结论的原始依据，所以留在仓库里，而不是只贴一行结论。

## 为什么会命中

NSIS 安装器是自解压 PE，通用启发式主要看它是否改动自己目录之外的东西。Beta 0.1/0.2 的安装器在 `PREINSTALL` 阶段复制并删除 `%APPDATA%\IRIXEN Code\config.json`、`model-cache.json`——"安装包动 AppData 里的配置文件"正是 Hijacker 家族的典型特征。

VirusTotal 的行为报告（MITRE ATT&CK 那几屏）基本是"这是一个安装程序"的通用结论，不是恶意行为：

| 报告条目 | 实际来源 |
| --- | --- |
| 在用户目录下创建可执行文件 | Tauri 默认 `currentUser` 安装模式，装到 `%LOCALAPPDATA%\IRIXEN Code` |
| 创建快捷方式、注册卸载方法 | 安装器的本职工作 |
| 修改文件默认打开方式 | 不是我们做的：`bundle.fileAssociations` 未配置，代码里没有 shell 关联调用 |
| 获取按键信息 / 安装消息钩子 | NSIS 安装界面与 WebView2 运行时；应用代码里没有 `SetWindowsHookEx` |
| 试图解除沙箱监控钩子 | 沙箱对 WebView2 / 安装器的误判；代码里没有反调试、没有 unhook |
| 收集已安装程序信息 | 读取卸载注册表判断是否覆盖安装 |

## 已经做的处置

安装阶段不再碰 `%APPDATA%`：安装器只在**自己的安装目录**写一个空的 `first-run.marker`，应用启动时由 `config::adopt_fresh_install` 备份并清除配置。见 `installer.nsh` 与 `docs/BUILD.md`「安装包与残留配置」。

## 再被报时怎么提交误报

Avira：<https://www.avira.com/en/support-threats>（选 False Positive，附文件或 SHA256）。

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

同时可以在 VirusTotal 里对具体文件点 "Community → 提交误报"（如果有权限），并把上面这段贴上。

## 长期解法

- **OV / EV 代码签名证书**：消掉信誉类判定和 SmartScreen 的"未知发布者"。这是唯一能一次性解决所有引擎的办法。
- 用户量上来后，启发式的误报会自然下降——但对新版本仍可能反复触发。
