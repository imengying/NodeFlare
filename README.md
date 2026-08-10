# CF Monitor

基于 Rust、WebAssembly 和 Cloudflare Workers 的轻量服务器监控。单个 Worker 同时提供 API、WebSocket 实时推送和静态前端，D1 保存节点与历史指标，Durable Object 负责实时广播；Linux、Windows 和 macOS agent 同样由 Rust 独立实现。

## 能力

- CPU/多 GPU、三段负载、内存/Swap、多磁盘容量与 IO、磁盘 await/利用率、网络、连接数、进程和系统信息
- 独立延迟任务，支持 TCP/ICMP、按节点分配、默认分配给新节点和独立测试周期
- Komari-Glass 式资产总览、高密度节点卡、可开关的搜索/分组筛选和响应式深浅主题
- 独立节点详情页，支持实时以及 1/4/24/168/720 小时负载和延迟图表
- Durable Object WebSocket 实时刷新，支持总览或单节点订阅，断线时 60 秒轮询兜底
- 节点计费/到期、币种、流量计算口径、公开备注和隐藏控制；后端每日更新汇率并在 D1 保存原子快照，混合币种资产可选 12 种展示币种
- 用户名+密码管理登录，可选 Cloudflare Turnstile 登录验证和全站盾验证
- 后台工作区：节点增删改、批量删除、拖拽排序和独立 Agent 密钥轮换
- 后台安全、外观、主题、告警、数据维护工作区，支持 Telegram 通知测试、离线/到期提醒，以及按节点、指标、时间窗口和平均/持续方式配置的资源告警规则
- 接入 CFSM Theme Store，支持 Emerald、Pulse、指定 GitHub 构建目录的签名预览、应用和恢复内置主题
- 可配置总览项、资产折算币种、毛玻璃效果、默认主题、明暗独立背景图、历史保留和私有仪表盘
- 节点 Agent 采样间隔、批量上报间隔、统计网卡、延迟任务和上下行流量修正可在线下发
- Agent 默认每 5 秒采样、每 60 秒批量上报；D1 每分钟保存一个代表点，7 天后压缩为小时数据，保留周期可配置
- 可聚合最多 16 个公开 CF Monitor 站点，远端节点、详情和 WebSocket 数据保持各自来源隔离
- 公开前端支持简体中文和 English，可配置 favicon、API/WebSocket CORS 来源和第三方主题 CSP 来源
- 汇率由 Worker 每日从 ER API 拉取、Frankfurter 兜底；失败时保留旧快照并按小时退避重试
- 独立 Rust Agent，支持 Linux x86_64/ARM64、Windows x86_64 和 macOS ARM64，支持后台按节点开启自动更新

## 架构

```text
Rust cross-platform agent -- HTTPS report --> Rust Worker (WASM) -- batch write --> D1
                                      |
                                      +-- push --> Durable Object --> WebSocket clients
                                      |
                                      +-- daily FX refresh -----------> D1 snapshot
                                      |
Browser <-- static assets / REST API -+
```

## 本地开发

需要 Bun 1.3+、Rust stable、`wasm32-unknown-unknown` target 和 Cloudflare Wrangler。

```bash
rustup target add wasm32-unknown-unknown
bun install
cp .dev.vars.example .dev.vars
bun run dev
```

首次运行时，`bun run dev` 会编译 Agent 和两个前端入口，自动创建本地 D1 并应用尚未执行的 migration，然后启动 Worker。访问 `http://localhost:8787`。仅调试前端时运行 `bun run dev:frontend`，Vite 会把 `/api` 代理到 `8787`。使用 `http://localhost:5173/?demo=1` 可查看完整演示数据。

## 部署到 Cloudflare

1. 登录 Cloudflare：

```bash
bunx wrangler login
```

2. 设置初始管理员凭据。生产环境至少设置密码，用户名默认为 `admin`：

```bash
bunx wrangler secret put ADMIN_USERNAME
bunx wrangler secret put ADMIN_PASSWORD
# 可选：固定会话签名密钥，避免密码修改后旧会话全部失效
bunx wrangler secret put SESSION_SECRET
# 可选：后台查询 D1/Workers 用量，Token 需要 Account Analytics Read 权限
bunx wrangler secret put CLOUDFLARE_ACCOUNT_ID
bunx wrangler secret put CLOUDFLARE_API_TOKEN
```

3. 部署：

```bash
bun install
bun run deploy
```

Wrangler 4.45+ 的自动资源配置会根据 `wrangler.toml` 中的 `DB` binding 创建并绑定 D1，不需要手动创建数据库或填写 `database_id`。`bun run deploy` 会先检查 binding；仅在部署环境尚未预配置 D1 时，执行一次用于触发自动配置的初始上传；随后应用所有待执行 migration，并完成最终 Worker 部署。Cloudflare Deploy Button 会预先配置资源，普通 CLI 与 Workers Builds 是否需要初始上传则由脚本自动判断。自动资源配置目前仍是 Cloudflare Beta 能力；如需使用既有 D1，再显式填入对应的 `database_id`。参见 [Wrangler 自动资源配置](https://developers.cloudflare.com/workers/wrangler/configuration/)、[Deploy Button](https://developers.cloudflare.com/workers/platform/deploy-buttons/) 和 [D1 migrations](https://developers.cloudflare.com/d1/reference/migrations/)。

部署后打开站点，点击右上角管理按钮添加节点。新建节点或重置密钥时会显示平台安装命令：Linux 自动安装 systemd 或 OpenRC 服务，Windows 使用 SYSTEM 启动任务，macOS ARM64 使用 LaunchDaemon；均支持开机启动、异常重启和 Agent 自动更新。TCP 延迟由 Rust 直接测试，ICMP 任务需要系统 `ping`；Linux 的 `ip`、`df` 及各平台的 `nvidia-smi` 是可选采集依赖。

Linux 保留 `/proc` 提供的磁盘 IO 和 TCP/UDP 连接统计。Windows 与 macOS 使用跨平台系统接口采集 CPU、内存、Swap、磁盘、网络、进程和系统信息；当前没有可靠统一来源的磁盘 IO 与连接数会上报为 `0`。自动更新优先下载 Worker 同源文件，未提供相应平台文件时回退到 `imengying/CF-Monitor` 最新正式 GitHub Release。

第三方主题只替换公开首页，`/admin` 始终使用内置管理界面。应用前会验证 GitHub `tree` 目录中的构建产物，主题资源设置长效浏览器缓存。第三方主题代码与站点同源运行，只应安装可信作者提供的主题。

定时任务每 5 分钟检查一次汇率快照，但只有成功数据超过 24 小时才访问上游。ER API 失败时自动尝试 Frankfurter；两者都失败不会清空 D1，下一次尝试至少间隔一小时。后台“监控数据库”工作区可以查看来源、日期和 12 种资产币种，也可以手动刷新。Cloudflare 用量可在同一页面填写 Account ID 和具有 Account Analytics Read 权限的 API Token；输入留空时回退到 `CLOUDFLARE_ACCOUNT_ID` 与 `CLOUDFLARE_API_TOKEN` Worker Secret。

后台“通知”工作区可创建最多 20 条资源规则。指标支持 CPU、内存、磁盘、上行和下行；网络阈值单位为 MiB/s。空服务器列表表示应用到全部服务器，也可逐台指定。时间窗口为 1-1440 分钟，“平均”按窗口平均值判断，“持续”要求窗口内所有采样均超过阈值。

后台“登录与安全”中的 CORS 列表同时保护 API 和 WebSocket Origin。配置多站点聚合时，应在每个远端站点的允许来源中加入主站 Origin。CSP 外部资源列表用于可信第三方主题所需的脚本、样式、字体和 API 域名。

后台“延迟”工作区管理独立任务。每个任务可选择 TCP 或 ICMP、目标、30-3600 秒周期和任意节点；“默认分配给新服务器”只影响以后添加的节点，不会覆盖当前选择。TCP 目标支持域名、IPv4 和 `host:port`，省略端口时使用 443；ICMP 目标只接受域名或 IPv4。每轮测试四次，保存成功 RTT 中位数和丢包率，配置会在 Agent 下一次上报后生效。

## GitHub Actions

推送三段版本 Tag（`1.2.3` 或 `v1.2.3`）时，`release.yml` 会先校验 Tag 与根项目、前端、Worker 和 Agent 的包版本完全一致，再构建 Linux x86_64/ARM64、Windows x86_64、macOS ARM64 Agent 以及公开前端，将产物附加到同一个 GitHub Release。发布前应同步修改四个 `package.version`；GitHub Actions 不编译或部署 Rust/WASM Worker，Worker 仍由 Cloudflare 在代码 push 或同步时构建。

登录页和管理面板使用独立前端入口，并在 Rust Worker release 构建时嵌入 WASM；`/admin` 和 `/admin-assets/*` 由 Worker 直接响应，不受第三方公开主题影响。直接执行 Worker 构建前应先运行 `bun run build:frontend`，项目提供的构建脚本会自动保证该顺序。

## Cloudflare Git 部署

Worker 由 Cloudflare Workers Builds 直接从 GitHub 仓库构建。创建仓库并推送代码后，在 Cloudflare Workers 中导入 `imengying/CF-Monitor`，使用以下设置：

- Worker 名称：`cf-monitor`
- 生产分支：`main`
- 根目录：`/`
- 构建命令：留空
- 部署命令：`bun run deploy`

`bun run deploy` 会依次编译 Rust Agent、公开前端、嵌入式管理端，确保 D1 已自动配置，应用 migration，最后编译和发布 Rust/WASM Worker。以后每次 push 到 `main`，以及 Cloudflare 同步到新的提交时，都会重复同一套可幂等流程，不再需要单独的 D1 初始化步骤。

项目尚未发布，数据库和 Agent 协议按当前版本直接演进，不包含旧 schema 或旧单样本上报协议的兼容层。开发期间若初始 schema 发生变化，请重新创建未投入使用的 D1 数据库后执行唯一的 `0001_initial.sql`。

## 配置

`wrangler.toml` 中可调整：

| 变量 | 默认值 | 说明 |
| --- | ---: | --- |
| `SITE_NAME` | `CF Monitor` | 初始站点名，可在管理端覆盖 |
| `OFFLINE_THRESHOLD_SECONDS` | `180` | 初始离线判定秒数 |
| `HISTORY_RETENTION_DAYS` | `30` | 历史保留天数，范围 1-365 |

管理凭据使用 Worker secrets 或后台“安全”工作区保存，不要写进 `wrangler.toml`。开启 Turnstile 后，在“安全”工作区填写站点密钥和密钥；服务端会调用 Cloudflare Siteverify 校验令牌。部署前请把本地测试密钥替换为正式密钥。

本地调试可以使用 Cloudflare 官方 Turnstile 测试站点密钥和密钥；测试令牌固定为 `XXXX.DUMMY.TOKEN.XXXX`，只适用于开发环境。

## API

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| `GET` | `/api/config` | 公开站点配置 |
| `GET` | `/api/themes` | 第三方主题商店 |
| `GET` | `/api/exchange-rates` | D1 中的 CNY 基准汇率快照 |
| `GET` | `/api/servers` | 公开节点和最新指标 |
| `GET` | `/api/history/:id?hours=24` | 节点历史 |
| `GET` | `/api/latency/:id?hours=24` | 节点延迟任务历史 |
| `GET` | `/api/ws` | 实时 WebSocket |
| `POST` | `/api/agent/report` | 探针上报，节点 Bearer token |
| `POST` | `/api/admin/login` | 管理登录 |
| `GET/POST` | `/api/admin/servers` | 管理节点 |
| `GET/POST` | `/api/admin/latency-tasks` | 查询或创建延迟任务 |
| `PATCH/DELETE` | `/api/admin/latency-tasks/:id` | 编辑或删除延迟任务 |
| `GET/POST` | `/api/admin/alert-rules` | 查询或创建资源告警规则 |
| `PATCH/DELETE` | `/api/admin/alert-rules/:id` | 编辑或删除资源告警规则 |
| `PATCH/DELETE` | `/api/admin/servers/:id` | 编辑或删除节点 |
| `PATCH` | `/api/admin/servers/order` | 更新全部节点顺序 |
| `POST` | `/api/admin/servers/:id/token` | 轮换探针密钥 |
| `DELETE` | `/api/admin/servers` | 批量删除节点 |
| `GET/PATCH` | `/api/admin/settings` | 读写站点与展示设置 |
| `GET` | `/api/admin/theme-settings` | 读取当前前端提供的主题设置描述 |
| `POST` | `/api/admin/themes/preview` | 创建短时签名主题预览 |
| `POST` | `/api/turnstile/verify` | 校验全站 Turnstile 令牌 |
| `GET` | `/api/admin/database` | 查询 D1 节点、历史和数据库统计 |
| `GET` | `/api/admin/cloudflare-usage` | 查询 UTC 今日/昨日的 D1 与 Workers 用量 |
| `POST` | `/api/admin/exchange-rates/refresh` | 立即拉取并更新 D1 汇率快照 |
| `DELETE` | `/api/admin/history` | 清理历史指标 |
| `POST` | `/api/admin/notifications/test` | 发送一条通知测试消息 |

第三方主题可在构建目录根部提供 `theme-settings.json`。文件使用 `{ "schema": 1, "settings": [...] }`，字段支持 `text`、`textarea`、`url`、`color`、`select`、`toggle` 和 `number` 类型。后台会按描述生成控件，保存值通过 `/api/config` 的 `theme_options` 返回给主题。

## License

MIT
