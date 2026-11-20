# 杀软误报记录

<p align="center">
  中文 &nbsp;|&nbsp; <a href="./antivirus.en.md">English</a>
</p>

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

Beta 0.1 到 0.3 用的是 NSIS 安装器（自解压 PE），通用启发式主要看它是否改动自己目录之外的东西。那三个版本的安装器在 `PREINSTALL` 阶段复制并删除 `%APPDATA%\IRIXEN Code\config.json`、`model-cache.json`——"安装包动 AppData 里的配置文件"正是 Hijacker 家族的典型特征。Beta 0.4 仍然是 NSIS，但 `installer.nsh` 已经整个删掉：安装器只解压自己的文件、写卸载登记，`%APPDATA%` 一概不碰。这条路不存在了，模板没有换。

VirusTotal 的行为报告（MITRE ATT&CK 那几屏）基本是"这是一个安装程序"的通用结论，不是恶意行为：

| 报告条目 | 实际来源 |
| --- | --- |
| 在用户目录下创建可执行文件 | NSIS 默认 `currentUser` 安装模式，装到 `%LOCALAPPDATA%\IRIXEN Code`（用户可改路径）。per-user 安装必然发生在用户目录里，这条是安装模式，不是伪装 |
| 创建快捷方式、注册卸载方法 | 安装器的本职工作 |
| 修改文件默认打开方式 | 不是我们做的：`bundle.fileAssociations` 未配置，代码里没有 shell 关联调用 |
| 获取按键信息 / 安装消息钩子 | 旧 NSIS 安装界面与 WebView2 运行时；应用代码里没有 `SetWindowsHookEx` |
| 试图解除沙箱监控钩子 | 沙箱对 WebView2 / 安装器的误判；代码里没有反调试、没有 unhook |
| 收集已安装程序信息 | 读取卸载注册表判断是否覆盖安装 |

## 已经做的处置

Beta 0.4 起 `src-tauri/installer.nsh` 被整个删除：安装器不再有 `PREINSTALL` / `POSTINSTALL` 钩子，也就没有可以读写 `%APPDATA%` 的地方。剩下的行为都是安装器的本职工作——解压、建快捷方式、写卸载登记。

代价是安装器不再"帮你清空旧配置"：从 Beta 0.3 升级上来，设置和会话会原样保留；真要干净重来，手动删 `%APPDATA%\IRIXEN Code`，或用设置 → 关于 → 维护 → 恢复默认设置。这条不打算加回去，见 `docs/BUILD.md`「安装包与残留配置」。

## 再被报时怎么提交误报

Avira：<https://www.avira.com/en/support-threats>（选 False Positive，附文件或 SHA256）。

> Subject: False positive — IRIXEN Code installer flagged as TR/Hijacker.Gen
>
> The file `IRIXEN.Code_<version>_x64-setup.exe` (SHA256 `<hash>`) is flagged as
> `TR/Hijacker.Gen`. It is an NSIS installer for an open-source desktop IDE built
> with Tauri; the full source is at https://github.com/lanche-furry/IRIXEN-Code
> and the installer definition is `src-tauri/tauri.conf.json` in that repository.
>
> The detection looks like a heuristic inherited from earlier builds of this same
> installer, which copied and deleted config files under `%APPDATA%` before and
> after installing. That hook file (`src-tauri/installer.nsh`) was deleted in this
> release: the installer now only unpacks its own files and registers an
> uninstaller, and it does not read or write the user profile at all.
> The binary is signed with a self-signed certificate
> (`CN=IRIXEN`), which is expected for a pre-release build.
>
> Please review and whitelist. Thank you.

同时可以在 VirusTotal 里对具体文件点 "Community → 提交误报"（如果有权限），并把上面这段贴上。

## 长期解法

- **OV / EV 代码签名证书**：消掉信誉类判定和 SmartScreen 的"未知发布者"。这是唯一能一次性解决所有引擎的办法。
- 用户量上来后，启发式的误报会自然下降——但对新版本仍可能反复触发。
