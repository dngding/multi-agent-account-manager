# Windows 代理断线验收

这份流程验证的是 Windows WebView2 应用层行为：账号绑定代理后，代理运行时断开，账号视图应被关闭，且代理恢复前不能重新打开。它不替代公网出口、DNS、WebRTC 或系统防火墙级别的验证。

## 1. 准备

在项目根目录打开两个 PowerShell 窗口。第一窗口启动本地 HTTP CONNECT 转发代理：

```powershell
python scripts/http_connect_proxy.py --port 0
```

记录输出中的 `port`。脚本只转发 CONNECT 隧道，不接收用户名、密码或账号资料。停止它使用 `Ctrl+C`。

第二窗口确认 Rust 工具链和 Tauri 依赖：

```powershell
rustc --version
cargo --version
npm ci
npm run tauri dev
```

也可以使用已构建的 Windows 安装包；验收重点是运行中的 WebView2，而不是 Vite 页面。

## 2. 基线

1. 在“叠号”中新增或选择一个测试账号 A。
2. 给 A 配置 `http://127.0.0.1:<port>`，保存后重新打开 A。
3. 等待 Muse 页面完成加载，确认账号 A 可以正常操作。
4. 如有第二个测试账号 B，给 B 配置另一个可用代理并打开 B，确认 B 的页面正常。

## 3. 断线

1. 回到运行本地代理的窗口，按 `Ctrl+C` 停止代理。
2. 保持叠号窗口打开，等待最多约 15 秒。当前观察器在启动阶段快速检查，随后约每 6.5 秒复核一次；连续两次失败后会关闭 A 的 WebView。
3. 通过以下现象判定应用层断线保护生效：
   - A 的页面消失，管理页重新显示；
   - 页面提示包含“代理连接中断，已关闭该账号视图”；
   - 若 B 使用另一个代理，B 仍保持可操作；
   - 再次切换到 A 时，应用提示代理不可用，不创建新的 A WebView。

## 4. 恢复

1. 重新启动本地 CONNECT 代理，使用新的端口时同步更新 A 的代理配置；若复用原端口，直接重新启动即可。
2. 再次打开 A，确认切号前代理握手通过并能加载页面。
3. 重启叠号，确认 A 的代理绑定仍然存在，且停止代理后仍会阻断打开。

## 5. 记录

至少保留以下信息：

- 测试提交 SHA；
- Windows 版本、WebView2 版本、Rust/Cargo 版本；
- A/B 使用的代理端口和停止/恢复时间；
- 断线后的截图或屏幕录制；
- A 被关闭、A 重开被阻断、B 不受影响三个结果。

不要记录代理用户名、密码、Cookie、token 或完整账号邮箱。若代理停止后 A 仍能继续访问，验收失败，应停止宣称网络隔离，并优先改用拒绝 `DIRECT` 的本地代理承载层或独立 Chromium/VM。

## 6. 自动化检查

Windows runner 或本机 Rust 工具链可执行：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib --target x86_64-pc-windows-msvc
```

该命令覆盖代理地址校验、HTTP CONNECT / SOCKS5 握手、认证拒绝、截断响应、超大响应、代理握手中途断开，以及账号共享代理检测。真实 WebView2 断线结果必须按本手册操作记录，Rust 单测不能替代它。

推送功能分支后，`Windows validation` 工作流会在单测通过后继续构建 NSIS 安装包，并以 `windows-x64-nsis-acceptance` artifact 上传。该安装包用于本手册的实机验收；若安装包构建失败，单测通过也不代表桌面应用可交付。
