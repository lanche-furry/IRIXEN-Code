# 构建与工具链

## 前置

| 需要 | 说明 |
| --- | --- |
| Node.js 18+ | 前端构建、Tauri CLI |
| Rust 1.77+，**GNU target** | 本仓库走 `x86_64-pc-windows-gnu`，不依赖 MSVC |
| WebView2 Runtime | Windows 11 自带；Windows 10 由安装包 `downloadBootstrapper` 兜底 |

## 便携工具链（本仓库默认）

为了不污染系统，Rust 与构建工具都在仓库内的 `.tools/`：

```
.tools/
  cargo/                  # CARGO_HOME（registry 缓存 + cargo.exe）
  rustup/                 # RUSTUP_HOME（toolchains/…-windows-gnu）
  mingw64/                # dlltool 等 binutils（仅生成导入库时需要）
```

这些目录已在 `.gitignore` 里，不进版本库。

`scripts/build.mjs` 负责把环境变量接好，然后转交 cargo 或 Tauri CLI：

```bash
npm run check                 # cargo check（不产出可执行文件）
npm run app:dev               # tauri dev（vite + 窗口，仅开发）
npm run app:release           # tauri build --no-bundle（只出 release exe）
npm run app:build             # tauri build（NSIS 安装包）
node scripts/build.mjs tauri dev --no-watch
```

它做的事：

```js
env.RUSTUP_HOME = .tools/rustup
env.CARGO_HOME  = .tools/cargo
env.PATH        = .tools/mingw64/bin : toolchain/bin : cargo/bin : $PATH
env.CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = <toolchain>/lib/rustlib/x86_64-pc-windows-gnu/bin/rust-lld.exe
```

### 为什么链接器不是 gcc

MinGW 的 gcc 会从自身安装路径推导 `specs` 与 `default-manifest.o`，而这段路径在内部**没有加引号**。只要安装路径含空格（例如 `E:\IRIXEN Code\.tools\mingw64`），链接就会报：

```
ld.exe: cannot find E:/IRIXEN: Permission denied
ld.exe: cannot find Code/.tools/.../default-manifest.o: No such file or directory
```

绕开方式：用 Rust 自带的 `rust-lld` 链接（GNU target 已经捆绑了 self-contained 的 MinGW 运行库，见 `lib/rustlib/x86_64-pc-windows-gnu/lib/self-contained/`）。MinGW 仍然保留在 PATH 上，因为 `rustc` 生成导入库时还要调 `dlltool.exe`。

`CARGO_TARGET_<TRIPLE>_LINKER` 是 cargo 的环境变量形式，天然支持带空格的路径，比塞进 `RUSTFLAGS`（按空格切分）安全。

## 用系统工具链构建（可选）

如果你的机器已装 Rust + MinGW（路径无空格），可以完全不看 `.tools/`：

```bash
rustup default stable-x86_64-pc-windows-gnu
npm install
npx tauri build
```

MSVC 工具链同样可用，只是产物依赖 VC 运行库、体积略大：

```bash
rustup default stable-x86_64-pc-msvc
npx tauri build
```

## Linux 构建

Windows 之外只有一条受支持的路径，且 Linux 的包必须显式点名——`tauri.conf.json` 的 `bundle.targets` 写的是 `["nsis"]`：

```bash
npx tauri build --bundles deb,appimage
```

依赖（Ubuntu/Debian）：

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev patchelf
```

产物在 `src-tauri/target/release/bundle/{deb,appimage}/`。两个坑：

- **AppImage 不自带 WebKit。** 它只打非系统库，宿主仍要装 `libwebkit2gtk-4.1-0`；CI 用 `ubuntu-22.04` 构建（glibc 2.35），22.04 及以上可直接运行。
- **deb 的 `Depends` 必须补 `libssl3`。** `reqwest` 走 native-tls，二进制直接链了 `libssl.so.3` / `libcrypto.so.3`；漏声明在精简系统上会装完启动即失败，且没有任何提示。注意 Tauri 是把自己的默认项（`libwebkit2gtk-4.1-0`、`libgtk-3-0`）**追加**在你写的数组后面，不是替换，所以 `bundle.linux.deb.depends` 里只写 `["libssl3"]` 这一项增量即可，写全反而会重复。

每日构建见 `.github/workflows/nightly.yml`：Windows 与 Linux 各自出包，最后一个 publish 任务统一发布到滚动 `nightly` 预发布，避免两个平台争抢同一个 tag。

## 图标

```bash
npm run icons
```

`scripts/gen-icons.mjs` 以 `assets/icon.png`（高分辨率透明母版，中心裁成正方形）为源，先归一化到 1024 px，再用 lanczos3 重采样出 16/32/48/64/128/256/512/1024、Tauri 标准图标名、Windows Store logos、`icon.ico` 与手写的 `icon.icns`（ic07/ic08/ic09/ic10 段）。同一张 512 px 图会写入 `public/irixen-icon.png`，供启动画面、首次启动向导、标题栏与编辑器空状态在渲染进程里复用。更换 logo 只需替换 `assets/icon.png` 后执行 `npm run icons`。

**改完图标必须重新执行 `npm run app:release`。** exe 的图标由 `build.rs` 里的 Windows 资源编译器写进 PE 资源段，Tauri 的默认窗口图标也由 `generate_context!()` 编译进二进制；只替换 `src-tauri/icons/*` 会因 Cargo 缓存复用旧的 `.res`，导致任务栏和 exe 仍显示旧图标。`build.rs` 已把 `icon.ico` / `icon.png` / `icon.icns` 等声明为 `cargo:rerun-if-changed`，正常增量构建会自动重跑资源编译器；若曾手动 `cargo clean -p irixen-code`，重新构建即可。

## 产物

```
E:/irixen-build/release/irixen-code.exe
E:/irixen-build/release/bundle/nsis/IRIXEN Code_0.1.0-beta.1_x64-setup.exe
```

`CARGO_TARGET_DIR` 固定在盘根 `E:/irixen-build`（路径不能含空格，否则 MinGW `windres` 预处理失败）。`node scripts/build.mjs tauri dev/build` 这类命令必须走 Tauri CLI：裸 `cargo build` 拿不到构建上下文，会去加载 `devUrl` 的 `http://localhost:5173/` 而不是内嵌的 `dist/`，窗口就会显示"localhost 拒绝连接"。

**不产出 debug 版本。** `build.mjs` 的 `dev` / `run` 已禁用，裸 `build` 自动补 `--release`；`cargo check` 只做类型检查不留可执行文件。发行版一律用 `npm run app:release`（只出 exe）或 `npm run app:build`（额外出 NSIS 包）。界面显示的版本号统一为 `Beta 0.1`，包元数据保持合法 semver `0.1.0-beta.1`。

release profile 已针对体积调优：

```toml
opt-level = "s"      # 优先体积
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

## 调试界面

Tauri 的 devtools 快捷键足以看报错，但**量布局**要靠 CDP。WebView2 认 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`：

```bash
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 npm run app:dev
```

之后 `http://127.0.0.1:9222/json/list` 能拿到页面，用任意 CDP 客户端（甚至一个 30 行的 `WebSocket` 脚本）执行 `Runtime.evaluate` 读 `getBoundingClientRect()`、截图、或直接 `window.__TAURI_INTERNALS__.invoke(...)` 调命令——不用在应用里加任何调试代码。

排查布局问题时的固定动作：量 `.app / .body / .sidebar / .stage / .chat` 的 `getBoundingClientRect()` 和 `scrollWidth > clientWidth`。只要有一层宽度远超窗口，就是 min-content 撑破（见 [架构设计 · 布局契约](ARCHITECTURE.md#9-布局契约)）。

### 测试实例必须隔离配置目录（`IRIXEN_HOME`）

窗口读的是**真实用户配置** `%APPDATA%\IRIXEN Code`（`config::base_dir`）。用 CDP 点完首次启动向导、改过主题、选过供应商，写下的就是这份正式配置；之后再安装 NSIS 包，安装出来的应用会认为"已经初始化过"，**首次启动设置不会再出现**，看起来就像安装后没有初始化。

所以任何自动化/手动验证都要把配置指到临时目录：

```bash
IRIXEN_HOME="E:/irixen-build/_probe-home" \
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9555 \
  ./irixen-build/release/irixen-code.exe
```

`IRIXEN_HOME` 会同时接管 config.json、sessions、extensions、skills 的位置，删掉整个目录就等于全新安装。验证完不要再用默认目录跑探针。

### 安装包与残留配置

`src-tauri/installer.nsh` 通过 `bundle.windows.nsis.installerHooks` 挂进 NSIS：

- **全新安装**（`$UpdateMode <> 1`）：把旧的 `config.json` 备份成 `config.json.preinstall` 后删除，保证向导会跑；覆盖升级不动任何设置。
- **卸载且勾选"删除应用数据"**：连 `%APPDATA%\IRIXEN Code`（真实配置目录）和 `%LOCALAPPDATA%\com.irixen.code`（WebView2 缓存）一起删。模板自带的清理只认 bundle id，删不到 `IRIXEN Code` 这个目录。

已经装坏的机器不必重装：设置 → 关于 → 维护 → **恢复默认设置**（`config_reset`，旧文件备份成 `config.json.bak-<时间戳>` 后重载窗口，向导会重新出现），或调色板里的 **重新运行首次启动设置**。

## 持续集成

三个工作流，职责不重叠：

| 文件 | 触发 | 做什么 |
| --- | --- | --- |
| `.github/workflows/ci.yml` | push 到 `main`、任意 PR、手动 | 双平台（ubuntu-22.04 + windows-latest）快检：`npm run build`（tsc + vite）、`cargo check --all-targets`、`cargo test --lib`。**不打包、不发布** |
| `.github/workflows/nightly.yml` | 每天 UTC 12:00（北京 20:00）、手动 | 双平台打包，发布到滚动的 `nightly` **预发布**频道 |
| `.github/workflows/release.yml` | 推 `Beta*` / `v*` tag | 双平台打包，发布为 **latest** 正式频道 |

`ci.yml` 必须跑 Windows：真正有意思的代码在 `#[cfg(windows)]` 后面，只跑 Linux 一行都编不到。反过来 Linux 那次也跑 `cargo test`，因为它出结果最快，逻辑错误能在打包前拦住。

`nightly` 是预发布，且 tag 名不含数字，`is_version_tag` 会把它排除，所以每日构建永远不会弹窗提示普通用户升级。

## Windows 签名

`release.yml` 在打包后调用 `scripts/sign-windows.ps1`，给 `irixen-code.exe` 和 NSIS 安装包加上 Authenticode 签名（SHA256 + DigiCert 时间戳）。证书来自两个仓库 Secrets：

| Secret | 内容 |
| --- | --- |
| `WINDOWS_SIGN_PFX` | `.pfx` 的 base64（`[Convert]::ToBase64String([IO.File]::ReadAllBytes('x.pfx'))`） |
| `WINDOWS_SIGN_PFX_PASSWORD` | 导出 PFX 时设置的密码 |

两个 Secret 缺任意一个，签名步骤自动跳过，工作流只打一条 warning 继续发布——签名是发布增强，不该成为发布的单点故障。

**自签证书的定位**：文件里会带上 `CN=IRIXEN` 的签名者信息，杀软"未签名二进制"这类启发式不再命中；但证书链在用户机器上不受信任，SmartScreen 依然可能提示"未知发布者"，用户需要点"更多信息 → 仍要运行"。要彻底消除这个提示只能买 OV/EV 代码签名证书，届时把新 PFX 换进同一个 Secret 即可，脚本不用改。

**本地签名**（需要证书和密码在环境变量里）：

```powershell
$env:SIGN_PFX = Get-Content E:\irixen-build\signing\pfx.base64 -Raw
$env:SIGN_PFX_PASSWORD = Get-Content E:\irixen-build\signing\password.txt -Raw
.\scripts\sign-windows.ps1 src-tauri\target\release\irixen-code.exe
```

安装包内嵌的 exe 是在打包时压进去的，所以**后置签名只覆盖安装包本身和独立 exe**。如果连内嵌 exe 也要签，得在 `bundle.windows.signCommand` 里挂同一个脚本（那样本地构建也必须带证书，故未默认启用）。

## 版本管理（GitHub Releases）

版本真值是 semver，同时写在三处，任何时候都必须一致（否则「关于」页报告的版本和安装包实际携带的版本会不一样）：

| 文件 | 字段 |
| --- | --- |
| `package.json` | `version` |
| `src-tauri/tauri.conf.json` | `version` |
| `src-tauri/Cargo.toml` | `[package].version`（编译进 exe） |

界面上显示的 `Beta 0.1` 是产品标签，独立保存在 `src-tauri/src/update.rs` 的 `DISPLAY_VERSION`，不要求等于 semver。`npm run bump`（不带参数）会检查三处是否一致。

**发一个版本**

```bash
npm run bump 0.2.0-beta.1 --label "Beta 0.2"   # 三处 manifest + Cargo.lock + DISPLAY_VERSION
git add -A && git commit -m "release: Beta 0.2"
git push origin main
git tag Beta0.2 && git push origin Beta0.2
```

推 tag 会触发 `.github/workflows/release.yml`：Windows 打 NSIS、Linux 打 deb + AppImage，然后发布为 **latest 频道**（`--latest`、非 prerelease），所以应用内「检查更新」会把它当作最新版。发布说明取自 `docs/release-notes/<tag>.md`，该文件不存在则用 GitHub 自动生成的版本。

**发布说明用 Beta 0.1 的排版：一份文档，每行都带两种语言**，不做「中文半篇 + 英文半篇」的分段：

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

- 标题行、`##`/`###` 标题、`-` 要点：一律 `中文 / English`，斜杠两边**各留一个空格**（脚本按这个切分）。
- 长段落用两段：先中文段，空行，再英文段。
- 命令、文件名、版本号两边都写全 —— 别让英文读者回翻中文段去找 `chmod +x`。
- 不要写成 `## 简体中文` / `## English` 两半：读者只想读一种语言时，得先跳过整份译文。

`docs/release-notes/Beta0.2.md` 是现成的样板；`nightly` 频道的说明由 `nightly.yml` 现场生成，同一套排版，不需要手写文件。

这条规则有闸门，不靠自觉：`npm run notes:check -- Beta0.2` 会检查标题是 `# IRIXEN Code <版本>`、每个标题行与要点都是 `中文 / English`（双语要点至少 6 条）、中英文字数都够，不达标直接指出是哪一行。`release.yml` 在发布前跑同一个脚本（`scripts/notes-check.mjs`），不达标就停在上传之前。已经发出去的 release 想改正文，不必为了几行字重建两个平台的包：用令牌直接改，只动正文、附件不受影响。

```bash
TOKEN=…   # 细粒度 PAT，需要 contents: write
GH_TOKEN=$TOKEN gh release edit Beta0.2 --repo lanche-furry/IRIXEN-Code \
  --notes-file docs/release-notes/Beta0.2.md
```

**不要手动建 Release**：手动创建的 release 不会带 Linux 包，还容易和 tag 触发的那次抢同一个 tag。工作流发布前会核对 tag 的数字化形式与 `package.json` 版本，对不上直接失败——这就是为了拦住「忘了 bump 就发版」。

**标签与版本的关系**：tag 用**产品标签、不带 `v`、不带空格**（`Beta0.1`、`Beta0.2`）。`compare_semver` 把 `Beta0.2` 解析成 `0.2`，因此 `Beta0.2` 对 `0.1.0-beta.1` 判为更新。但标签本身不带预发布后缀，对 `0.2.0-beta.1` 反而会被判成更高，所以 `from_release` 先用 `is_current_label` 比对产品标签，相同就直接认定「无更新」——否则刚装上 Beta0.2 的人会被提示升级到 Beta0.2。`nightly` 由 `is_version_tag` 排除在版本判断之外：它是每日构建，不是版本。

**应用内检查更新**：设置 → 关于 → 检查更新。`app_check_update` 读 `/releases/latest`；404 时回退 `/releases` 列表（覆盖只有草稿或预发布的仓库）；仍为空就显示「GitHub 上还没有发布版本」并给出发布页入口。结果缓存 10 分钟，按钮强制刷新；网络走 `net::build_client`，遵循设置 → 网络的代理配置。应用**只读取**元数据并打开下载页，不会自行下载或替换正在运行的 exe —— Windows 上运行的 exe 无法覆盖自身，更新说明也应该由用户先看到。

**已安装插件详情**：扩展面板 → 已安装 → 每张卡片的「详情」。`extensions_inspect` 完全离线读取本地安装目录（`package.json`、README、CHANGELOG、LICENSE、目录大小、贡献点与激活事件），不依赖 Open VSX；商店里的 `extensions_detail` 仍用于在线包的详情，两者互不干扰。

## 常见问题

**`error calling dlltool 'dlltool.exe': program not found`**
PATH 里没有 MinGW `bin`。用 `npm run check` / `npm run app:build` 启动即可，脚本会自动补上。

**`cannot find E:/IRIXEN: Permission denied`**
链接器退化成了 gcc，且安装路径含空格。确认 `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` 指向 `rust-lld.exe`。

**窗口一片空白、看不到界面**
窗口是 `visible: true` 创建的，所以空白通常意味着前端崩在首屏：打开 devtools 看控制台第一个报错。`vite` 冷启动（首次依赖预构建）时纯色窗口可能持续数秒，日志里的 `[irixen] ui mounted after Nms` 会给出真实耗时；生产构建是本地单包，通常在 1s 内。

**`npm run app:dev` 端口被占用**
`vite.config.ts` 里 `strictPort: true`，5173 被占会直接失败而不是换端口。先释放 5173。

**Web2API 起不来（默认 8791 被占）**
端口在 `Web2API` 面板里可改，改完点保存会自动重启监听；启动失败的原因会显示在面板状态行上（`last_error`），不是静默退出。注意它只绑 `127.0.0.1`，不需要也不应该对局域网暴露。
