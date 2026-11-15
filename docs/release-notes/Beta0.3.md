# IRIXEN Code Beta 0.3

这一版是四个日常用起来最硌手的地方：改错了没法退、只能打字、会话开多了找不回来、有新版本还得自己去下载。四个都做完了。

This release is the four things that hurt most in daily use: no way back after a bad edit, text-only input, sessions you can never find again, and updates you had to go download yourself. All four are done.

## 新功能 / New features

### 撤销一整轮改动 / Undo a whole turn

代理改完文件，输入框上方会出现一条 `3 个文件已改动 ⟲ 撤销这一轮`。点一下，这一轮碰过的文件全部回到改动前的样子——包括这一轮新建的文件会被删掉。

After the agent edits files, a bar above the composer reads `3 files changed ⟲ Undo this turn`. One click puts every file the turn touched back the way it was, and deletes the files the turn created.

- 记的是每个文件**第一次**被写之前的原内容，同一轮改十次也只回滚到最初状态 / What is recorded is each file's content **before its first write**, so ten edits in one turn still roll back to the original
- 不碰 git：没有仓库、没有暂存区、工作区很脏，都一样能撤 / Git is not involved: no repository, no index, a dirty worktree — it works the same
- 一轮改动超过 8MB、或碰到二进制文件时，这一轮会标注不可撤销并给出原因，宁可不做也不做一半 / A turn that exceeds 8MB or touches a binary is marked not undoable with the reason shown — better to refuse than to half-roll-back
- 只保留最近 48 轮，更早的快照自动丢弃，内存不会一路涨 / Only the last 48 turns are kept; older snapshots are dropped so memory stays flat

### 直接粘贴截图 / Paste a screenshot

`Ctrl+V` 贴图、拖文件进来都行，图片跟着你的问题一起发给模型。

`Ctrl+V` to paste, or drag a file in — the image goes to the model alongside your question.

- 长边超过 1600px 的图会自动等比缩放并铺白底（透明 PNG 不会变黑），几 MB 的截图发出去只有几百 KB / Images wider than 1600px are scaled down proportionally onto a white background (transparent PNGs do not turn black), so a multi-megabyte screenshot leaves as a few hundred KB
- 三套协议各自按原生形状发送：OpenAI 的 `image_url` data URL、Anthropic 的 `source.base64`、Gemini 的 `inlineData` / Each protocol gets its native shape: OpenAI `image_url` data URLs, Anthropic `source.base64`, Gemini `inlineData`
- 纯文字对话仍然发字符串而不是分段数组，不浪费 token 也不改变老行为 / Text-only turns still send a plain string instead of a parts array — no wasted tokens, no change in behaviour
- 单张上限 4MB、一次最多 8 张；每张按 1000 token 计入上下文预算，超了会提示而不是静默截断 / 4MB per image and 8 images per message; each counts as 1000 tokens against the context budget, and going over is reported instead of silently truncated

### 会话历史 / Session history

左侧多了会话列表：标题、消息条数、相对时间，点一下就切回去接着聊。

The sidebar now lists your sessions with title, message count and relative time. Click one to pick the conversation back up.

- 支持搜索标题与内容，开几十个会话也找得到 / Search titles and contents, so dozens of sessions are still findable
- 悬停出现删除；删掉的正好是当前会话时自动新建一个，不会留在空白界面里 / Hover to delete; deleting the session you are in starts a new one instead of leaving you on a blank screen
- 换会话时会清空待发送的图片，避免把上一轮的截图误发给下一个问题 / Switching sessions clears pending images, so a screenshot from the last conversation cannot leak into the next question

### 应用内一键更新 / One-click updates in the app

设置 → 关于里那条「有新版本」不再只是一个链接：点「下载并安装」，进度条走完、校验通过，程序自己退出并把安装器交出去。

The "update available" row in Settings → About is no longer just a link. Click **Download and install**: the progress bar fills, the hash is verified, the app quits and hands over to the installer.

- 安装包落地后与发布页的 `checksums.txt` 比对 SHA-256，对不上就删掉不执行 / After download the file is hashed against the release's `checksums.txt`; a mismatch deletes it instead of running it
- 大小也要对得上，防止代理返回一个被截断的文件 / File size must match too, so a truncated response cannot pass as a good download
- 只挑本平台的包：Windows 用 `*setup.exe`，Linux 用 `.AppImage`（先 `chmod 755`）/ Only this platform's asset is picked: `*setup.exe` on Windows, `.AppImage` on Linux (with `chmod 755` first)
- 校验失败会明确告诉你「校验未通过」，不会假装装完了 / A failed check says so plainly; it never pretends the install succeeded

## 扩展兼容性 / Extension compatibility

- 一个慢的补全扩展不再冻住整个窗口：语言服务调用原本是同步命令，会阻塞主线程最多 6 秒，现在是异步执行，前端 4.5 秒超时并丢弃被新输入超期的旧结果 / A slow completion extension no longer freezes the whole window: language calls used to be synchronous commands that blocked the main thread for up to 6 seconds. They run asynchronously now, with a 4.5 s timeout in the renderer and stale replies dropped
- 扩展详情页新增「用到的 API」：扫描扩展入口里真实出现过的调用，未实现的能力（诊断、定义跳转、引用、代码操作、符号、CodeLens）标红说明 / Extension details now list the VS Code APIs the entry file actually calls, with the unimplemented ones in red (diagnostics, go-to-definition, references, code actions, symbols, CodeLens)
- 格式化会区分「扩展宿主没启动」与「没有格式化器」，并补上 `Shift+Alt+F` 与「保存时自动格式化」开关（默认关闭）/ Formatting now distinguishes "extension host is not running" from "no formatter", and gains `Shift+Alt+F` plus an opt-in format-on-save setting

## 修复 / Fixes

- 发行流程现在发布前强制检查发行说明是双语排版，格式漂了就发不出去 / The release workflow now refuses to publish notes that are not in the bilingual layout
- CI 从这一版开始为每个附件生成 `checksums.txt`——没有它，一键更新无从校验 / CI starts publishing `checksums.txt` for every asset in this release; without it the one-click updater has nothing to verify against

## 安装 / Install

- **Windows x64**：`IRIXEN.Code_0.3.0-beta.1_x64-setup.exe`，双击安装，会创建开始菜单与桌面快捷方式并自动补齐 WebView2 Runtime / `IRIXEN.Code_0.3.0-beta.1_x64-setup.exe` — run it; it creates Start Menu and desktop shortcuts and bootstraps the WebView2 Runtime
- **Linux x64（通用）**：`IRIXEN_Code_0.3.0-beta.1_amd64.AppImage`，`chmod +x` 后直接运行 / `IRIXEN_Code_0.3.0-beta.1_amd64.AppImage` — `chmod +x` it, then run it
- **Linux x64（Debian/Ubuntu）**：`sudo apt install ./IRIXEN_Code_0.3.0-beta.1_amd64.deb`，依赖会自动拉取 / `sudo apt install ./IRIXEN_Code_0.3.0-beta.1_amd64.deb` — dependencies come with it

## 说明 / Notes

- **一键更新从 Beta 0.3 才开始有**：还在 Beta 0.2 的用户点「检查更新」能看到这一版，但下载安装仍要手动来一次；装上 Beta 0.3 之后，下一个版本就能在应用里点完了 / **One-click updating starts with Beta 0.3**: on Beta 0.2 the check finds this release but the download is still manual. Once Beta 0.3 is installed, the next release can be installed from inside the app
- 本页附件已重建过一次：同样是 Beta 0.3，但包含上面的扩展兼容性修复，SHA-256 与最早下载到的那批不同，请以 `checksums.txt` 为准 / The assets on this page were rebuilt once. It is still Beta 0.3, but it carries the extension fixes above, so the SHA-256 differs from the first batch — trust `checksums.txt`
- Windows 安装包带**自签名证书**（签名者 `CN=IRIXEN`），SmartScreen 仍可能提示「未知发布者」，首次运行点「更多信息 → 仍要运行」；彻底消掉需要 OV/EV 证书 / The installer is **self-signed** (`CN=IRIXEN`), so SmartScreen may still warn about an unknown publisher — click "More info → Run anyway" on first launch. Removing that for good needs an OV/EV certificate
- 上一版把安装阶段的配置清理移进了应用首次启动，这一版的安装器只写自己的安装目录；若仍被杀软报毒，请提交误报并附上本仓库源码 / The previous release moved profile cleanup out of the installer into the app's first launch; this installer only writes its own directory. If an engine still flags it, submit a false positive and point it at this repository
- Linux 版仍未经真机冒烟测试，CI 只做静态校验；遇到启动即退出请附上终端输出 / The Linux build is still not smoke-tested on real hardware — CI only checks the packages statically. If it exits on startup, please attach the terminal output

## 许可证 / License

- 许可证：Apache-2.0 / License: Apache-2.0
