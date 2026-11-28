# Proxy Gate 接入说明

当前 Windows 版把代理池解析、静态链校验、健康检测和每账号固定端口拆到独立的
`proxy-gate.exe`。叠号只保存账号关系、调用 Gate CLI，并在打开账号前执行原有
代理预检。代理池内部变化不会污染主程序，后续跟随上游版本时只需保留少量
Tauri 命令和前端入口。

## 安装

先在仓库内的 `proxy-gate` 子项目完成 Windows Release 构建，再在叠号源码目录运行：

```powershell
.\scripts\install-proxy-gate.ps1 -PoolPath "C:\path\to\sidecar.yaml"
```

如果本机只有一个可自动发现的 `sidecar.yaml`，可以省略 `-PoolPath`：

```powershell
.\scripts\install-proxy-gate.ps1
```

也可以显式指定 Gate 可执行文件：

```powershell
.\scripts\install-proxy-gate.ps1 `
  -GateExe "C:\path\to\proxy-gate.exe" `
  -PoolPath "C:\path\to\sidecar.yaml"
```

脚本会把程序复制到：

```text
%APPDATA%\com.ldjx.accountmanager\proxy-gate.exe
```

并创建同目录下的 `proxy-gate.json`。如果设置了 `DIEHAO_DATA_DIR`，叠号运行时的
数据目录也会随之改变；此时可改用环境变量显式指定：

```powershell
$env:PROXY_GATE_EXE = "C:\path\to\proxy-gate.exe"
$env:PROXY_GATE_CONFIG = "C:\path\to\proxy-gate.json"
```

## 在叠号中使用

1. 打开已有账号的“修改代理”。
2. 选择 **Proxy Gate**。
3. 点击“刷新”，等待自动健康检测完成。
4. 列表会按 `available`、`handshake_only`、`unavailable`、未检测排序，
   同一状态内按延迟排序。
5. 选择固定入口并保存。叠号会为账号分配稳定的 `127.0.0.1:191xx` 端点。
6. 打开账号时仍会重新预检；Gate 不可用、上游断线或指纹变化都会阻断，不会直连回退。

账号 ID 使用 `providerId:uuid`，因此 Muse 和其他服务的同名 UUID 不会串绑。

## 每周代理池更新

Gate 按 `healthIntervalSeconds` 热重载 `sidecar.yaml`：

- 新入口会出现在下一次刷新/健康检测结果中。
- 已绑定入口的节点定义或任意静态 `dialer-proxy` 跳发生变化时，指纹漂移，
  该账号端点返回 HTTP 503，已有隧道立即关闭。
- 用户重新选择同一入口并确认后，新指纹才会写入绑定。
- 删除入口同样会被阻断。

这是故意设计的 fail-closed 行为，避免每周更新后静默把账号切到未经确认的新链路。

## 托盘与叠号托管二选一

`proxy-gate.exe tray` 适合不打开叠号时保持代理入口运行；HKCU 自启动也是
显式开启。叠号会在绑定或解除账号时重启自己托管的 Gate 进程。

同一个配置不能同时由托盘和叠号修改。若叠号检测到端口已由托盘或其他进程占用，
会提示先退出托盘进程，避免启动重复 Gate 或使用旧 assignment。

## 隔离边界

当前实现提供：

- 账号到固定本地代理端口的绑定。
- 代理链指纹和 revision 追踪。
- 上游不可用时的拒绝直连。
- 各账号独立 WebView2 数据目录。
- 代理漂移时关闭已有连接并阻断重开。

它不证明两个入口拥有不同的公网出口，也不等价于独立虚拟机、独立设备或系统级
沙箱。Android 不支持 WebView2 的按账号代理参数，因此本轮网络隔离只落在 Windows。
