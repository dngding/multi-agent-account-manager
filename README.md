# 叠号（Multi-Agent Account Manager）

[![LINUX DO](https://img.shields.io/badge/Community-LINUX%20DO-0064e0?style=flat-square)](https://linux.do/)

基于 Tauri 2 构建的跨平台（macOS / Windows / Android）AI 助手多账号隔离与快速切换工具，安装后应用名称为 **叠号**。

> **来源说明**：本仓库 fork 自 [ldjx7/multi-agent-account-manager](https://github.com/ldjx7/multi-agent-account-manager)，并在此基础上开发 Windows 按账号代理功能。原始项目及贡献者的版权归其作者所有，本项目继续采用 [Apache-2.0](LICENSE) 许可。当前分支是独立修改，不代表上游维护者认可、背书或发布。

## 支持服务与核心特性

| 服务 | 状态 | 说明 |
| --- | --- | --- |
| **Muse · Meta** | 已启用 | 独立会话隔离、邮箱识别、额度明细同步 |
| **Hark** | 已启用 | 独立会话隔离、额度与余额同步 |

- **原生级多账号隔离**：macOS 使用独立 UUID `WKWebsiteDataStore`，Windows 使用独立 WebView2 `UserDataFolder`，Android 使用独立 WebView `MULTI_PROFILE`。
- **Windows 按账号代理（功能分支）**：为账号 WebView 与登录弹窗绑定 HTTP / SOCKS5 代理，打开前握手检查，连续健康检查失败后关闭视图。新增支持“仅保存 / 检查并打开”：Windows 新账号必须使用代理，待配置账号不自动预热；后台代理中断后按账号推送最新状态。历史新增流程基线 `4dce77d` 的 Windows Rust 测试 23/23、独立 EXE 和 NSIS 构建通过；两个未登录测试账号已完成新增流程、后台中断状态更新，以及该版清空代理、阻止重开和重启恢复回归，版本归属见[2026-11-21 验收记录](docs/windows-proxy-setup-state-acceptance-2026-11-21.md)。界面“预检通过”仅表示有限握手检查，不代表公网出口、DNS / WebRTC、系统级拒绝直连、设备或账号关系隔离已验证。
- **从现有代理池选择**：Windows 新增账号默认读取现有本地 YAML 固定端口池，支持搜索、来源选择、手动刷新及每 10 秒 / 窗口恢复焦点时同步。入口删除或目标变化需重新确认，不自动替换已绑定账号；保留手动地址。最终 `01c596d` Windows Rust 42/42、EXE / NSIS 构建通过；合成池候选变化在 `a4ac022` 实测，最终版另通过保存绑定、定义漂移关闭 A、重开阻断与 B 保留。真实池 / 机场链路、分项刷新和其他未覆盖项见[验收记录](docs/windows-proxy-pool-acceptance-results.md)。来源只读，Telegram 占用未知，配置一致性不证明运行中代理已重载或公网出口独立，操作见[使用说明](docs/existing-proxy-pool-setup.md)。
- **一键复制 Cookie**：Windows、macOS 和 Android 账号卡片支持把当前服务的登录 Cookie 写入系统剪贴板；桌面端会尽量禁止 Cookie 进入系统剪贴板历史或云同步。Cookie 属于登录凭据，请勿发送给不可信软件或网站。
- **桌面端快捷切号**：支持 `⌘1~9` / `Ctrl+1~9` 以及 `⌘↑`/`⌘↓`/`⌘[`/`⌘]`（Windows 为 `Ctrl`）快速切换账号。
- **应用内检查更新**：支持在客户端内检查 GitHub Releases 最新版本、带进度条下载、`SHA-256` 校验及覆盖更新。

---

## 界面预览

### macOS

<p align="center">
  <img src="docs/screenshots/macos-muse.png" alt="macOS Muse 多账号管理与额度同步" width="49%" />
  <img src="docs/screenshots/macos-hark.png" alt="macOS Hark 账号与双上限额度" width="49%" />
</p>

### Windows

<p align="center">
  <img src="docs/screenshots/windows-muse.png" alt="Windows Muse 多账号管理与额度同步" width="49%" />
  <img src="docs/screenshots/windows-hark.png" alt="Windows Hark 账号与双上限额度" width="49%" />
</p>

### Android

<p align="center">
  <img src="docs/screenshots/android-muse-accounts.png" alt="Android Muse 多账号与额度" width="210" />
  <img src="docs/screenshots/android-hark-accounts.png" alt="Android Hark 账号与双上限额度" width="210" />
  <img src="docs/screenshots/android-muse-chat.png" alt="Android 独立会话隔离界面" width="210" />
  <img src="docs/screenshots/android-update-modal.png" alt="Android 应用内检查更新" width="210" />
</p>

---

## 下载与安装

### 本分支 Windows 验收构建

Windows 按账号代理增强在用户 fork 的 `codex/windows-account-proxy` 功能分支开发。进入 [dngding 仓库的 Windows CI](https://github.com/dngding/multi-agent-account-manager/actions/workflows/windows-ci.yml?query=branch%3Acodex%2Fwindows-account-proxy)，选择该分支中**源码提交匹配、构建成功**的运行，在页面底部下载 `windows-x64-nsis-acceptance` artifact。它包含独立 EXE 和 NSIS 安装器；下载 artifact 通常需要登录 GitHub。

本轮构建来自 [01c596d 的成功 CI](https://github.com/dngding/multi-agent-account-manager/actions/runs/44926476039)，可直接进入[最终 artifact 下载页](https://github.com/dngding/multi-agent-account-manager/actions/runs/44926476039/artifacts/14715672060)。Rust 42/42，测试归属、未覆盖项与文件哈希见[代理池验收记录](docs/windows-proxy-pool-acceptance-results.md)。此前 `4dce77d` 新增流程与后台状态的历史基线见[2026-11-21 报告](docs/windows-proxy-setup-state-acceptance-2026-11-21.md)。首次使用仍建议按[断线验收手册](docs/windows-proxy-disconnect-acceptance.md)，用独立 EXE 和全新测试数据目录检查自己的代理链路，再用于正式账号。

已有 Telegram Panel / Mihomo 本地固定端口池时，可按[从现有代理池选择](docs/existing-proxy-pool-setup.md)使用选择器，无需为叠号重复导入机场订阅或再开代理服务。选择器包含在上述 `01c596d` 构建中；真实机场链路仍需用空白 profile 单独核验，不能把合成配置验收当成现有生产池已通过。

没有现成代理池时，机场订阅接入和固定账号端口配置见[机场订阅 → Mihomo 配置指引](docs/airport-subscription-proxy-setup.md)。订阅链接先由代理客户端解析，叠号使用固定 HTTP / SOCKS5 端口；现有池选择器只读固定入口配置，不直接解析订阅。

当前应用内更新仍指向上游发布源。测试本分支时不要通过应用内更新覆盖程序；本分支构建应从上述 fork 的对应 CI 运行获取。

### 上游正式发布

[上游 GitHub Releases](https://github.com/ldjx7/multi-agent-account-manager/releases) 提供原项目的平台安装包，**不包含本功能分支新增的 Windows 代理增强**：

| 平台 | 下载文件 | 架构 / 系统要求 |
| --- | --- | --- |
| macOS | `account-manager-macos-universal.dmg` | Apple Silicon & Intel（macOS 14.0+） |
| Windows | `account-manager-windows-x64-setup.exe` | x64（Windows 10 / 11） |
| Android | `account-manager-android-universal-release.apk` | 通用 ABI（Android 7.0+） |

> `account-manager-macos-universal.app.tar.gz` 仅用于 macOS 应用内自动更新，首次安装请下载 `.dmg`。

### macOS 安装注意事项

1. **拖入应用程序目录**：打开 `account-manager-macos-universal.dmg` 后，请务必将 `叠号.app` 拖入 `/Applications`（应用程序）文件夹再启动。切勿直接在 DMG 挂载镜像中运行，否则无法写入替换，应用内自动更新将改为打开解压目录提示手动替换。
2. **提示「“叠号”已损坏，无法打开」或「无法验证开发者」**：
   - 因未购买 Apple 付费开发者证书签名与公证（Notarization），macOS Gatekeeper 会对浏览器下载的应用附加隔离属性 `com.apple.quarantine`，从而触发「已损坏」拦截提示。
   - **解决方法一（推荐，终端移除隔离属性）**：

     ```bash
     xattr -rd com.apple.quarantine /Applications/叠号.app
     ```

     若提示权限不足，可使用：

     ```bash
     sudo xattr -rd com.apple.quarantine /Applications/叠号.app
     ```

   - **解决方法二（系统设置放行）**：先点击一次「取消」，随后打开「系统设置 → 隐私与安全性」，在页面底部找到被拦截提示并点击「仍要打开」。
   - 若直接打开 `.dmg` 也提示异常，可先在终端对 `.dmg` 执行一次 `xattr -d com.apple.quarantine ~/Downloads/account-manager-macos-universal.dmg`。
3. **数据保留**：账号配置保存在 `~/Library/Application Support/com.ldjx.accountmanager/`，Web 登录态保存在系统 `WKWebsiteDataStore` 中。后续覆盖安装或应用内更新均不会丢失已登录账号。

### Windows 安装注意事项

1. **SmartScreen 安全拦截（「Windows 已保护你的电脑」）**：
   - 因安装包未使用付费 EV/OV 代码签名证书签名，首次运行 `account-manager-windows-x64-setup.exe` 时 Microsoft Defender SmartScreen 可能弹出拦截窗口。
   - 点击窗口中的 **「更多信息」**，再点击 **「仍要运行」** 即可正常安装。
2. **安全软件误报与校验**：
   - 若杀毒软件误报拦截，可在 Release 页面查看 `latest.json` 中 `platforms.windows.sha256` 的官方哈希，与本地安装包核对一致后放行：

     ```powershell
     Get-FileHash .\account-manager-windows-x64-setup.exe -Algorithm SHA256
     ```

3. **WebView2 运行时依赖**：
   - 依赖系统内置的 Microsoft Edge WebView2 Runtime。Windows 11 默认已自带；部分精简版或老旧 Windows 10 若首次打开空白或无法启动，请前往 [Microsoft Edge WebView2 官方下载页](https://developer.microsoft.com/microsoft-edge/webview2/) 安装「常青独立安装程序（x64）」。
4. **安装权限与数据保留**：
   - 如安装到受系统保护目录需通过 UAC 授权。账号配置与各账号独立 WebView2 数据目录均保存在 `%APPDATA%\com.ldjx.accountmanager\`，覆盖安装或应用内更新均不会清除已有登录状态。

### Android 安装注意事项

1. **允许安装未知来源应用**：首次安装 `.apk` 以及使用应用内更新时，系统会要求授予「允许安装未知来源应用」权限，授权后返回即可调起系统安装器覆盖更新。
2. **WebView 内核要求**：多账号隔离依赖 Android System WebView 的 `MULTI_PROFILE` 能力；如提示不支持，请在应用商店或系统更新中升级 Android System WebView（或 Chrome）。
3. **签名一致说明**：所有 Release APK 均使用固定发布密钥签名，可直接覆盖升级并完整保留多账号登录态。

---

## 应用内更新与发版

- **客户端检查更新**：桌面端点击左侧栏底部「更新」按钮，Android 端点击右上角下载图标，可检查最新版本并一键下载校验安装；启动后也会静默检查一次并展示红点提醒。
- **维护者发布新版本**：
  1. 同步更新 [package.json](package.json)、[src-tauri/Cargo.toml](src-tauri/Cargo.toml) 与 [src-tauri/tauri.conf.json](src-tauri/tauri.conf.json) 中的版本号。
  2. 提交并推送 `v*` 标签（GitHub Actions 将自动编译三端安装包、生成 `latest.json` 并创建 Release）：

     ```bash
     git tag v0.1.1
     git push origin v0.1.1
     ```

---

## 本地开发

Windows 多账号隔离增强的设计范围、当前实现状态及后续网络 fail-closed 验收见[开发指引](docs/muse-multi-account-isolation-development-guide.md)。复用已有本地池见[代理池选择说明](docs/existing-proxy-pool-setup.md)，没有现成入口时再参考[机场代理配置指引](docs/airport-subscription-proxy-setup.md)。

环境要求：Node.js 22+、Rust 1.85+，Android 构建需额外配置 Android SDK 与 NDK 26+。

```bash
npm ci
npm run tauri -- dev
npm run tauri -- build
```

---

## 社区与开源协议

本项目已加入并致谢 [LINUX DO](https://linux.do/) 开源社区。

本项目基于 [Apache-2.0](LICENSE) 协议开源。
