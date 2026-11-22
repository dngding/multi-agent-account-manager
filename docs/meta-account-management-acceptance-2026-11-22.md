# Muse Meta 账户管理实机验收（2026-11-22）

## 环境

- Windows 11 桌面版
- 叠号 0.2.1，提交 `4ca2f66`
- 使用已有 Muse 账号、独立 WebView2 资料目录和已配置的固定代理入口
- Windows CI：Rust 单元测试、TypeScript / Vite 构建、EXE 与 NSIS 安装包均通过

## 验收结果

1. 当前账号卡片显示“打开 Meta 账户管理”按钮。
2. 点击后，右侧同一账号 WebView 从 Muse 导航到 `accountscenter.meta.com`，账号保持登录态。
3. Meta Account 首页成功加载，显示当前账号、`Login and security`、`Your information and permissions` 和 `Manage account`。
4. 进入账户资料弹窗后，`Your account` 显示默认联系方式，并可进入 `Contact points`。
5. `Contact points` 页面显示 `Add or edit contact info`，说明邮箱等联系方式编辑入口可正常访问。
6. 再次点击同一账号卡片的账户管理按钮后，窗口标题恢复为 `Muse · <账号>`，右侧 WebView 返回 `https://muse.ai/`，原 Muse 登录状态仍在。

本次验收没有提交邮箱修改，没有输入密码、安全码或验证码，也没有复制或记录 Cookie。

## 使用路径

1. 在 Muse 账号卡片点击“打开 Meta 账户管理”。
2. 点击当前账号卡片，进入 `Your account`。
3. 点击默认邮箱进入 `Contact points`。
4. 点击 `Add or edit contact info`，按 Meta 页面提示完成联系方式修改。
5. 完成操作后，再次点击叠号中同一账号的账户管理按钮返回 Muse。

Meta 可能要求重新输入密码、安全码或完成验证码。叠号只复用当前账号的隔离浏览环境，不绕过这些验证。

## 隔离边界

- 账户管理页面与 Muse 共用目标账号的 WebView2 资料目录、Cookie 和 Windows per-account 代理。
- 不会把 Cookie 复制到系统浏览器。
- 后台代理断线观察器继续作用于该账号视图；连续失败时仍按既有逻辑关闭视图。
- 本功能解决账户管理入口和登录态延续，不代表 DNS、WebRTC、设备指纹或系统级 fail-closed 已获得独立隔离。
