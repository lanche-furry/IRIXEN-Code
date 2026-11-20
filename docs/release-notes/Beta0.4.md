# IRIXEN Code Beta 0.4

这一版只做一件事：让代理在改代码之前，先能查出代码里已经存在的危险。为此给它加了一个确定性的扫描器、一份把扫描结果变成攻击路径的审计技能，以及审查面板上的一个按钮。

This release does one thing: let the agent find the dangers already in the code before it changes any. That means a deterministic scanner, an audit skill that turns its output into attack paths, and a button in the review panel.

## 新功能 / New features

### 安全审查代理 / A security auditor

审查面板标题栏多了一个盾牌按钮「安全审查」。点一下不发送文件改动，只发一句审计指令，代理进入审计模式，先扫再读。

The review panel header now has a shield button. It does not send a diff — it sends one audit instruction, and the agent switches into audit mode: scan first, read second.

- 代理新增第八个工具 `security_scan`：固定规则表机械扫一遍，不靠模型猜哪里可能有密钥 / The agent gains an eighth tool, `security_scan`, which sweeps with a fixed rule table instead of asking a model to guess where credentials might be
- 十条规则：私钥块、云厂商凭据、JWT、硬编码密钥、关闭证书校验、动态执行、SQL 字符串拼接、全局可写权限、密钥文件、未锁 SHA 的 CI action / Ten rules: private key blocks, cloud credentials, JWTs, secrets assigned literal values, disabled certificate verification, dynamic execution, SQL built by interpolation, world-writable permissions, secret files, and CI actions not pinned to a SHA
- 三种范围：`scope=workspace` 扫整棵树、`scope=changed` 只看未提交改动、`scope=path` 只扫一个文件或目录；默认最多回 80 条，可调到 300 / Three scopes: `scope=workspace` for the whole tree, `scope=changed` for uncommitted work, `scope=path` for one file or directory; 80 findings by default, up to 300
- 每条结果都带 `路径:行号`、证据和一句修法，密钥只留前 4 位、`***` 与后 3 位——扫描器永远不把完整凭据交出来 / Every finding carries `path:line`, the evidence and a one-line fix. Secrets are cut down to four leading characters, `***` and three trailing ones — the scanner never hands over a usable credential
- 自动跳过 `node_modules`、`target`、`.git` 与二进制文件；单文件上限 1.5MB，扫过 6000 个文件就停并标注 `truncated`，不会把一次审查拖成十分钟 / Dependency directories, `target`, `.git` and binary files are skipped; a single file is capped at 1.5MB and the sweep stops after 6000 files, marked `truncated`, so one audit cannot turn into a ten-minute wait
- 行内豁免：`// irixen-audit: ignore` 放行整行，`ignore=jwt` 只放行一条规则。扫描器的测试夹具靠这个活着，否则它第一天就报自己 / Inline suppression: `// irixen-audit: ignore` silences a line, `ignore=jwt` silences one rule. The scanner's own fixtures live on this, or it would report itself on day one
- 审计技能先扫后读：逐条回读命中处的上下文，只报能把入口、输入和效果说成一条攻击路径的问题，分「可利用、有条件、不是缺陷」三档，结尾必须交代没查什么 / The audit skill reads after it scans, re-reading the code around every hit and reporting only findings it can state as an attack path with entry point, input and effect. Verdicts are exploitable, conditional or not-a-bug, and the report ends by naming what was not checked

## 可靠性与流程 / Reliability and process

- 夜间构建从 UTC 12:00 挪到 12:07：整点是 GitHub 定时任务排队的最高峰，12:00 的任务经常晚十几分钟，偶尔直接被丢掉 / The nightly build moved from 12:00 to 12:07 UTC — the top of the hour is GitHub's busiest queue, where a 12:00 schedule shows up ten-plus minutes late or not at all
- 文档写清了三件容易反复踩的事：`schedule` 不是定时器、只认默认分支上的 cron、手动 `workflow_dispatch` 与定时运行互不替代；排查时先按 `event=schedule` 过滤运行记录 / The docs now spell out three things that keep tripping people up: `schedule` is not a timer, only the default branch's cron counts, and a manual `workflow_dispatch` does not stand in for a scheduled run. Filter runs by `event=schedule` first

## 修复 / Fixes

- 「安全审查」这四个字以前会同时命中通用代码审查与安全审计，两套报告格式在同一个回答里打架；现在关键词分流，审计文档也写明安全问题上以它为准 / The words "security review" used to load both the general code review and the security audit, leaving two report formats fighting in one answer. Keywords now route cleanly, and the audit document states that it wins for anything security-related
- 扫描器第一版把自己的测试夹具当成了真凭据。处理方式不是放宽规则，而是加行内豁免——一个总是误报的扫描器没人会跑第二次 / The scanner's first run flagged its own test fixtures as live credentials. The fix was inline suppression, not a looser rule: a scanner that cries wolf is a scanner nobody runs twice

## 安装 / Install

- **Windows x64**：`IRIXEN.Code_0.4.0-beta.1_x64-setup.exe`，双击安装，会创建开始菜单与桌面快捷方式并自动补齐 WebView2 Runtime / `IRIXEN.Code_0.4.0-beta.1_x64-setup.exe` — run it; it creates Start Menu and desktop shortcuts and bootstraps the WebView2 Runtime
- **Linux x64（通用）**：`IRIXEN_Code_0.4.0-beta.1_amd64.AppImage`，`chmod +x` 后直接运行 / `IRIXEN_Code_0.4.0-beta.1_amd64.AppImage` — `chmod +x` it, then run it
- **Linux x64（Debian/Ubuntu）**：`sudo apt install ./IRIXEN_Code_0.4.0-beta.1_amd64.deb`，依赖会自动拉取 / `sudo apt install ./IRIXEN_Code_0.4.0-beta.1_amd64.deb` — dependencies come with it

## 说明 / Notes

- **可以直接在应用里升级**：一键更新从 Beta 0.3 起可用，0.3 的用户点「检查更新 → 下载并安装」就能装完这一版，不用再去发布页 / **You can upgrade from inside the app**: one-click updating has worked since Beta 0.3, so 0.3 users can hit check, download and install without visiting the release page
- **扫描干净不等于安全**：规则表只管模式级问题，依赖漏洞、基础设施配置、运行时状态都不在范围内，报告结尾会明说，别把它当成一次完整渗透测试 / **A clean scan is not safety**: the rule table only covers pattern-level issues. Dependency CVEs, infrastructure configuration and runtime state are out of scope, the report says so, and none of this substitutes for a penetration test
- Windows 安装包带**自签名证书**（签名者 `CN=IRIXEN`），SmartScreen 仍可能提示「未知发布者」，首次运行点「更多信息 → 仍要运行」；彻底消掉需要 OV/EV 证书 / The installer is **self-signed** (`CN=IRIXEN`), so SmartScreen may still warn about an unknown publisher — click "More info → Run anyway" on first launch. Removing that for good needs an OV/EV certificate
- Linux 版仍未经真机冒烟测试，CI 只做静态校验；遇到启动即退出请附上终端输出 / The Linux build is still not smoke-tested on real hardware — CI only checks the packages statically. If it exits on startup, please attach the terminal output

## 许可证 / License

- 许可证：Apache-2.0 / License: Apache-2.0
