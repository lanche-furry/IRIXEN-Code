## 这次修的都是"装完了却用不了"

Beta 0.1 的安装包有三个会让人以为程序坏了的问题，Beta 0.2 全部处理掉了。

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

- Linux 版是**首个发布**，冒烟测试只在 CI 的虚拟显示环境里做过，没有覆盖所有发行版与显卡组合。遇到启动即退出，请附上终端输出。
- 每日构建仍在 `nightly` 预发布频道，随时会变，不会自动推送给正式版用户。
- 从 Beta 0.1 升级：应用内"检查更新"会提示，也可以直接下载上面的安装包覆盖安装（设置会保留）。
