# IRIXEN Code 0.5.0-beta.2

这是 Beta 0.5 的第二个切包：入口回到了右侧栏，但多了一道「完全访问」的二次确认。下面是这一版的改动，以及 Beta 0.5 的全部内容——它们就在这个安装包里。

This is the second cut of Beta 0.5: the entry point is back in the right rail, with a second confirmation for full access. What follows is what changed in this cut plus everything from Beta 0.5, because that is what this installer contains.

## 这一版的改动 / What changed in this cut

- 高级页的开关打开后，右侧栏出现对应入口；关掉就消失 / Turning the switch on in Advanced puts its entry in the right rail, and turning it off takes the entry away again
- 命令面板里同步出现同名命令，同样受这个开关控制 / The command palette gains the same command under the same switch
- 高级页不再内嵌整个面板，只留开关、风险说明和「打开面板」 / Advanced no longer embeds the whole panel: the switch, the warning and an "Open panel" button stay
- **开关只负责显示，不负责开门。** 面板顶部新加了一道「完全访问」确认，确认之前启动按钮是灰的 / **The switch shows the panel; it does not open the door.** A full-access confirmation now sits at the top of the panel, and the start button stays disabled until it is given
- 确认之后才开始监听：任何持有密钥的本地程序，都能通过它调用你已登录的网页模型 / Only after that confirmation does the endpoint listen, and any local program holding the token can call the web models you are signed in to
- 关掉开关会撤销这条确认并立即关闭端口，重新打开需要再确认一次 / Switching off revokes the confirmation and closes the port at once; switching back on asks again
- 配置文件里写着 `full_access: true` 而开关是关的，一律按未确认处理，不会继承 / A config file that says `full_access: true` while the switch is off is treated as unconfirmed and is never inherited
- 修复上一版沿用同一版本号、已安装的副本收不到更新提示的问题：版本升到 `0.5.0-beta.2`，标签 `Beta0.5.2` / Fixes the re-cut that kept the same version number, which left installed copies with no update prompt: the version is now `0.5.0-beta.2` under the tag `Beta0.5.2`

## 插件兼容性 / Extension compatibility

- 清单里有一个字段类型不对，不再让整个扩展从列表里消失：`main`、`displayName`、`description`、`activationEvents` 都按"能读就读、读不懂就忽略"处理 / One field with the wrong type no longer makes an extension disappear from the list: `main`, `displayName`, `description` and `activationEvents` are read when readable and skipped when not
- `"activationEvents": "*"` 这种旧写法不再被当成解析失败，它被当作一个事件读入 / The old `"activationEvents": "*"` form is no longer a parse failure; it is read as a single event
- `description` / `displayName` 写成 `{ "value": "…" }` 的本地化形式也能解析 / Localised `description` / `displayName` written as `{ "value": "…" }` now parse
- `package.json` 完全读不出来时，扩展会以"损坏"出现在列表里并附上解析器的原话，而不是静默缺席 / When `package.json` cannot be read at all, the extension is listed as broken with the parser's own message instead of silently missing
- `main` 指向目录（`"./out"`）时按 Node 的规则找 `index.js` / `extension.js`，不再直接判"没有入口" / A directory `main` (`"./out"`) resolves through the Node convention (`index.js`, `extension.js`) instead of being reported as "no entry"
- 只声明了 `browser` 入口的扩展现在也能加载，`.mjs` 入口按 ES 模块加载 / Extensions that declare only a `browser` entry now load, and an `.mjs` entry loads as an ES module
- 手工拷进扩展目录的包不再被贴上"技能包"标签：`kind` 与 `source` 改为从清单推断 / A package copied into the extensions directory by hand is no longer labelled a skill pack: `kind` and `source` are inferred from the manifest
- 清单改了 `activationEvents` 之后会立即生效，不必重装 / Editing `activationEvents` takes effect without a reinstall
- 以上每一条都有单测，`cargo test` 里能跑 / Each of the above has a unit test that runs under `cargo test`

## 设置 / Settings

- 设置多了一页「高级」，专门放需要用户本人拍板的能力 / Settings gained an **Advanced** tab for capabilities that are the user's call
- 这一页的开关默认关闭；关闭时不起监听、不开浏览器会话、模型选择器里也不出现对应条目，右侧栏同样没有入口 / The switch on that page is off by default: nothing listens, no browser session opens, no matching entry appears in the model picker, and the rail stays without a button
- 第一层是开关，第二层是面板里的「完全访问」确认——两道都过了，端口才会打开 / The switch is the first gate and the panel's full-access confirmation is the second; the port opens only after both
- 退回关闭状态会撤销第二道确认、停掉正在运行的服务，不留一个面板再也够不着的端口 / Going back to off revokes that confirmation and stops a running instance, so no port is left behind that no panel can reach

## 存放位置 / Where add-ons live

- 技能与扩展的默认位置从 `%APPDATA%` 改成**应用安装目录**（exe 旁边）：装在哪就在哪，能找到、能带走 / Skills and extensions now default to the app's install folder (next to the executable) instead of `%APPDATA%` — where you installed it is where they are
- 「设置 → 高级」新增「插件与技能存放位置」：可以填任意文件夹，可以「浏览…」选，也可以「恢复默认」 / Settings → Advanced gained an add-ons folder field: type any folder, pick one with Browse, or go back with Use default
- 升级后第一次启动会把旧位置的内容**复制**过去，原件不动；想继续用旧位置，把这一项填回 `%APPDATA%\IRIXEN Code` 即可 / The first start after an upgrade **copies** the old contents over; the originals are left alone. To keep using the old spot, point the field back at `%APPDATA%\IRIXEN Code`
- 目录不可写时（比如装进了 `Program Files`）自动退回配置目录，设置页显示实际在用的路径 / When the folder is not writable (a `Program Files` install, say) it falls back to the profile folder, and the settings page shows the path actually in use
- 升级不会清掉这些文件：安装器只删除它自己放进去的东西，删除目录时用的是不带 `/r` 的 `RMDir` / Upgrades do not sweep these files away: the installer deletes only what it put there — its directory removal is `RMDir` without `/r`
- 改路径后，已经在运行的插件会继续用旧目录，重启应用才完全切换 / Extensions that are already running keep the old folder until the app restarts

## 安全 / Security

- **打开外链不再经过命令解释器。** 之前的实现是 `cmd /C start "" <url>`，而 `cmd` 连参数里的 `&`、`|`、`^` 都当语法，所以一条 `https://example.com/?a=1&calc` 会在打开网页的同时执行 `calc`——模型回答里任何一个可点链接都可能变成一条命令。现在 URL 作为单个参数交给 `FileProtocolHandler`，没有东西解析它 / **Opening a link no longer goes through a command interpreter.** The old code ran `cmd /C start "" <url>`, and `cmd` treats `&`, `|` and `^` as syntax even inside one argument, so `https://example.com/?a=1&calc` opened the page *and* ran `calc` — which made every clickable link in a model's answer a potential command. The URL is now one argument to `FileProtocolHandler`, and nothing parses it
- 链接校验独立成纯函数，带三个单测：只放行 `http` / `https` / `mailto`，拒绝控制字符，但保留正常查询串 / Link validation is its own pure function with three tests: only `http` / `https` / `mailto`, no control characters, and query strings still work
- 本地兼容端点在首次启动时自动生成访问令牌，不再允许"没有密钥等于不鉴权" / The local compatibility endpoint generates an access token on first start; "empty key means no auth" is gone
- 令牌比较改成常数时间，逐字节比较不再泄露长度与前缀 / Token comparison is constant-time, so neither length nor prefix leaks
- 配置文件在 Unix 上以 `0600` 写入——里面有各家 API key / The config file is written `0600` on Unix — it holds provider API keys
- **已知限制**：渲染层仍未设置 CSP。加上它需要先验证自定义协议与扩展 webview 的行为，这一版不做，列进下一版 / **Known limitation**: the renderer still has no CSP. Adding one needs the custom protocol and extension webviews verified first, so it is deferred to the next release

## 升级 / Upgrading

- 从 Beta 0.4 或 Beta 0.5 直接覆盖安装即可，per-user 安装按卸载登记覆盖 / Install straight over Beta 0.4 or Beta 0.5; the per-user installer replaces through its uninstall registry entry
- 「高级」页的开关状态不继承：升级后一律是关闭，需要时手动打开 / The Advanced switch is not inherited: after an upgrade it is off, and you turn it on when you want it
- 上一版已经在高级页里打开过开关的，升级后同样回到关闭，且「完全访问」需要重新确认 / Anyone who had turned the switch on in the previous build also starts off again, and full access has to be confirmed afresh
- 配置项只增不删；旧的技能/扩展目录是复制，不是移动，原件仍在 `%APPDATA%\IRIXEN Code` / Config only gains a field; the old skills/extensions folders are copied, not moved, and the originals stay in `%APPDATA%\IRIXEN Code`
