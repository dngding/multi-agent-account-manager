# Windows 新增账号、后台状态与代理阻断验收：2026-11-21

结论：在两个未登录合成账号和两条本地 HTTP CONNECT 代理的场景下，已验证新增账号先配置代理、仅保存不打开、缺代理打开失败时保留当前账号、后台代理中断后自动更新对应卡片，以及恢复后手动打开并清除错误。最终提交 `4dce77d` 的独立 EXE 另行通过清空必需代理、阻止重开和应用重启的最小回归；Windows CI 全部成功，Rust 测试 **23 passed / 0 failed**。

本轮按构建版本区分证据：新增六项、后台中断与恢复使用 `e3ec4d2`；最终 `4dce77d` 只复测已打开账号的关键状态、清空代理和重启行为，**没有重新执行完整新增流程或再次注入后台断线**。不能把前一构建的全部实机场景写成最终构建已逐项重测。

全部账号均为合成测试资料，只访问 Muse 未登录公开页面，未输入登录凭据。本轮结果属于应用层行为验证；机场订阅及真实节点、公网出口、DNS/WebRTC、设备环境、历史账号关系、系统级拒绝直连和 10 多账号规模均未验收。

## 版本与证据范围

| 项目 | 当前核实结果 |
| --- | --- |
| 新增、后台断线及恢复场景提交 | `e3ec4d2cc5f3d49493a6ec3a0c9e3b4e6eac3c8b` |
| 该提交的 CI | [Windows validation 44913495984](https://github.com/dngding/multi-agent-account-manager/actions/runs/44913495984)，Rust 测试 23/23 通过 |
| 最终提交 | `4dce77d5a42d686f7c8e9e60096f6c32cf877116` |
| 最终提交的 CI | [Windows validation 44915899771](https://github.com/dngding/multi-agent-account-manager/actions/runs/44915899771)，全部成功 |
| 最终 artifact | [windows-x64-nsis-acceptance](https://github.com/dngding/multi-agent-account-manager/actions/runs/44915899771/artifacts/14711721802)，ID `14711721802` |
| 最终提交的本地日志 | 工作区 `_acceptance/ci-4dce77d.log`，不随本报告提交原始日志 |
| UI 验收程序 | `e3ec4d2` 的 CI 独立 EXE；最终 `4dce77d` EXE 完成下述最小回归 |
| 程序版本 | `0.1.5` |
| Windows | Windows 11 Pro，`10.0.26200` |
| WebView2 | `157.0.4341.60` |
| Artifact ZIP SHA-256 | `f7481712273f99dac0fe7e98204e0b121ded67be2b53a9632aee4a18889a64ff` |
| 独立 EXE SHA-256 | `80c1da8004404a27b3d45f7a0c9ef9f410e70dedcb51473492a24c95afd8e05a` |
| NSIS SHA-256 | `baa11cdeef28b1a58b957ccfebdf9da2e775c07668ba173cc86fd74a3a0aebfa` |
| 时区 | 北京时间，UTC+8 |

以下 Rust 日志已现场回读：

```text
running 23 tests
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

最终 CI 完成前端构建、Windows Rust 测试、变更 Rust 格式检查、独立 EXE 和 NSIS 构建与上传。格式日志没有 `Diff in`。本机没有 Rust/Cargo/MSVC，Rust 编译与单测结果来自 Windows runner；构建通过不能代替下面的 WebView2 实机观察。

## 测试准备

- 使用全新、显式指定的 `DIEHAO_DATA_DIR`，从 UI 新建合成账号 `Draft A` 和 `Acceptance B`，不预置生产账号或复制 Cookie、token、密钥。
- A 使用本地 HTTP CONNECT 代理 `http://127.0.0.1:52026`；B 使用 `http://127.0.0.1:52027`。两条端口仅用于本轮故障注入，不代表两个公网出口。
- 未向 Muse 页面输入邮箱、密码或验证码，也未提交登录操作。
- 不点击更新或安装按钮；本轮使用独立 EXE 验收。
- 未读取或改动生产 Mihomo 配置、系统代理、TUN、防火墙或远控服务。

## 新增账号六项验收：`e3ec4d2`

| 步骤 | 操作 | 实际观测 | 判定及范围 |
| --- | --- | --- | --- |
| 1 | 新增账号时不填代理，选择检查并打开 | 前端拦截操作；没有创建账号记录 | 通过：空代理新增打开入口被阻止 |
| 2 | 将 `Draft A` 选择为仅保存 | 只读配置观测：`proxyRequired=true`、`pendingActivation=true`；`profileExists=false`，`browserPids=[]` | 通过：保存账号不触发首次浏览器创建 |
| 3 | 从新增流程为 B 填入 `52027` 代理并检查、打开 | B 公开登录页加载；首次 WebView2 主进程 PID `31376`，实际带 `--proxy-server=http://127.0.0.1:52027` | 通过本轮本地 HTTP CONNECT 场景 |
| 4 | 在 B 当前打开时，尝试打开未配代理的 A | 显示必须先配置代理、已阻止打开；B 标题仍为 `Muse · Acceptance B`，B 页面与进程保留 | 通过：A 失败不替换当前 B |
| 5 | 为 A 保存 `52026` 代理 | A 卡片改为“已配置 · 待打开”；只读配置仍为 `pendingActivation=true`；尚无 A profile / 浏览器进程，B 继续显示 | 通过：保存代理不自动激活待配置账号 |
| 6 | 手动打开已配代理的 A | `pendingActivation=false`；创建 A 的 WebView2 主进程 PID `3380`，实际带 `--proxy-server=http://127.0.0.1:52026` | 通过：手动打开后才激活，并使用 A 的代理参数 |

UI 文本证据与配置、进程观测分别取证。截图和 UI 树可以证明提示、卡片状态及当前页面；`proxyRequired`、`pendingActivation`、profile 存在性和 PID / 代理命令行来自同轮只读状态与进程检查，不能仅凭截图推断。

本轮 `e3ec4d2` UI 中的历史文案为“已打开 · 代理正常”。这只是当时截图内容，不作为公网出口或完整代理隔离的验收结论；最终构建已将文案收窄为“已打开 · 预检通过”。

## 后台断线自动刷新与恢复：`e3ec4d2`

| 时间及操作 | 实际观测 | 判定及限度 |
| --- | --- | --- |
| B 前台、A 后台；23:03:33.8868819 停止 A 的 `52026` 代理 | 停止后没有再点击账号或手动刷新，A 卡片自动变为“代理中断 · 已阻止打开”；B 公开页与窗口标题保持 | 通过：后台故障触发对应账号状态自动更新，不依靠切号刷新 |
| 23:04:34.0312245 读取进程与配置 | A 的 `browserPids=[]`；B 主进程 PID `31376` 保留，仍带 `52027` 代理参数 | 通过：A 视图回收，B 保留 |
| 23:05:48.665903 恢复 A 代理，再手动打开 A | A 公开页面重新加载，中断错误清除，卡片恢复已打开状态 | 通过：恢复后可以手动重开，并更新对应错误状态 |

没有采样到 A 关闭或卡片变化的精确瞬间。进程检查发生在停止代理约 60 秒后，因此**不报告“15 秒内关闭”、精确刷新延迟或其他故障类型的时间上界**。本轮没有直接导出运行中 `warmAccounts` 字段；视图回收与状态同步由无后续操作时的卡片自动变化和独立进程检查共同支持。

## 最终 EXE 最小回归：`4dce77d`

| 时间及操作 | 实际观测 | 判定 |
| --- | --- | --- |
| 23:13:18 启动最终构建，使用已有合成 A/B 资料 | 两账号视图通过预检并打开；最终 UI 使用“已打开 · 预检通过” | 通过：既有测试资料在最终构建上可用 |
| 从 UI 清空 A 代理并保存；23:15:03.8292958 读取状态 | A 为 `proxyUrl=null`、`proxyRequired=true`、`pendingActivation=false`；A 浏览器主进程消失，B PID `31260` 保留 | 通过：已激活账号清空必需代理后关闭视图，没有回落系统默认网络创建 A 视图 |
| B 前台，再手动打开 A | 明确提示缺代理、已阻止打开；A 卡片为“待配置 · 已阻止打开”；B 页面与标题保留 | 通过：清空后重开被阻止，B 不受影响 |
| 23:16:50 重启最终应用；23:17:52.7459952 读取状态 | A 无浏览器主进程；B 自动恢复为 PID `33508`，带 `--proxy-server=http://127.0.0.1:52027`；截图 07 显示 A 待配置、B 预检通过 | 通过：重启恢复和预热没有绕过 A 缺代理的限制 |

A 的 profile 目录在清空代理后仍存在，保留已有独立浏览资料；“关闭视图”不等于删除 profile。本次只核实未新建 A 浏览器进程和 UI 阻断，未做系统网络抓包，不据此宣称系统级拒绝直连。

## 截图与本地记录

原始证据保存在工作区 `_acceptance/scoped-webview2-20261121-224635/`。该目录位于仓库外；原始 UI 树和进程记录不随本报告直接上传。

| 文件 | 可直接支持的观察 |
| --- | --- |
| `01-draft-blocked-b-retained.png` / `.txt`（仅本地） | A 为“待配置 · 已阻止打开”，缺代理阻断提示可见；B 的窗口标题和 Muse 公开登录页保留 |
| `02-proxy-saved-not-opened.png` / `.txt`（仅本地） | A 已变为“已配置 · 待打开”；B 标题与公开页面保留 |
| [03 后台 A 中断、B 保留](acceptance/2026-11-21-proxy-setup/03-background-a-interrupted-b-retained.png) | A 自动显示中断阻止，B 标题与公开页保持 |
| [04 恢复 A 后手动重开](acceptance/2026-11-21-proxy-setup/04-proxy-recovered.png) | A 公开页恢复，中断提示清除；该图仍是历史状态文案 |
| [06 清空代理后阻止 A 重开](acceptance/2026-11-21-proxy-setup/06-cleared-reopen-blocked-b-retained.png) | A 缺代理被阻止，B 保留；最终文案为预检通过 |
| [07 最终构建重启后 B 恢复](acceptance/2026-11-21-proxy-setup/07-final-restart-b-restored.png) | A 为待配置阻止，B 自动恢复并显示预检通过 |

03、04 来自 `e3ec4d2`；06、07 来自最终 `4dce77d` 最小回归。截图仅含合成别名和未登录公开页。

`03-background-a-stopped-processes.json`、`05-cleared-processes.json`、`07-restart-processes.json` 保存同轮合成账号的进程与配置检查；`events.log` 保存停止 A 代理时间，`completed-state.json` 保存清理结论。原始 UI 树、配置或进程命令行不随本报告直接提交。

## 清理与未覆盖范围

23:18:54 完成清理：测试独立 EXE 和两条本地代理退出，测试端口监听数为 0、测试 WebView2 进程数为 0。只删除该轮临时 `data` 目录，保留脱敏证据。生产配置前后哈希与默认目录存在状态一致；没有改动生产代理、系统设置或远控服务。

本轮未验收：

- 机场订阅导入、真实 Mihomo 固定节点入口、订阅更新和节点失效后的路由行为。
- 已登录生产或测试账号的会话持续性、认证弹窗与实际业务操作。
- SOCKS5 实机，以及 DNS、TCP、握手黑洞等故障。
- 浏览器实际公网出口 IP / ASN、DNS / WebRTC / IPv6 / UDP 泄漏。
- 系统级拒绝直连、设备环境或历史账号关系隔离。
- 5 / 10 多账号的内存、并发和稳定性。

机场订阅配置说明见 [机场订阅到固定本机代理端口](airport-subscription-proxy-setup.md)。旧版 `86679ef` 的独立断线与重启证据保留在 [Windows 代理断线实机验收记录](windows-proxy-acceptance-results-2026-11-21.md)，复现约定见 [Windows 代理断线验收手册](windows-proxy-disconnect-acceptance.md)。各份记录按对应构建和场景引用，不能互相替代未执行的测试。
