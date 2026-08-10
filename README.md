# CF Monitor

基于 Rust、WebAssembly 和 Cloudflare Workers 的服务器监控，支持 Linux、Windows 和 macOS Agent。

## 能力

- 采集 CPU、GPU、负载、内存、磁盘、网络、连接数、进程和系统信息
- TCP/ICMP 延迟监测，可按节点分配测试点和周期
- Komari-Glass 风格总览、节点卡片、搜索/分组筛选、响应式深浅主题
- 节点详情与历史图表，支持实时、1/4/24/168/720 小时范围
- WebSocket 实时刷新，断线后自动轮询
- 计费、到期、流量和多币种资产统计，Worker 每日更新汇率
- 用户名密码登录、Cloudflare Turnstile、节点隐藏和公开备注
- 节点管理、批量删除、拖拽排序、Agent 密钥轮换和在线配置
- Telegram 通知、资源告警、离线/到期提醒和数据维护
- 内置主题与 CFSM Theme Store，支持主题预览、应用和自定义设置
- 多站点聚合，支持最多 16 个公开 CF Monitor 站点
- Rust Agent 支持 Linux x86_64/ARM64、Windows x86_64 和 macOS ARM64，可按节点自动更新

## 部署到 Cloudflare

1. Fork 本项目，在 Cloudflare Dashboard 打开 **Workers & Pages**，选择 **Continue with GitHub** 并导入仓库。
2. 使用生产分支 `main`，根目录 `/`，构建命令留空，部署命令填写 `bun run deploy`。
3. 在 Worker 的 **Settings → Variables and Secrets** 中设置 `ADMIN_PASSWORD`；用户名默认为 `admin`，也可以设置 `ADMIN_USERNAME`。`SESSION_SECRET`、Cloudflare 用量查询凭据按需添加。
4. 保存并部署。`DB` binding 会自动创建或连接 D1，部署脚本会自动执行 migration，无需手动填写 `database_id`。
5. 打开生成的 Worker 地址登录管理面板，添加节点并生成对应平台的 Agent 安装命令。

部署后的节点、延迟、通知、主题、汇率和 Cloudflare 用量都在管理面板中配置。

## 配置

`wrangler.toml` 中可调整：

| 变量 | 默认值 | 说明 |
| --- | ---: | --- |
| `SITE_NAME` | `CF Monitor` | 初始站点名，可在管理端覆盖 |
| `OFFLINE_THRESHOLD_SECONDS` | `180` | 初始离线判定秒数 |
| `HISTORY_RETENTION_DAYS` | `30` | 历史保留天数，范围 1-365 |

管理凭据存入 Worker Secrets 或后台“安全”设置，不要写入 `wrangler.toml`。开启 Turnstile 后，填写 Site Key 和 Secret Key。

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

第三方主题可在构建目录提供 `theme-settings.json`（`schema: 1`）。后台按字段生成设置控件，保存值通过 `/api/config` 的 `theme_options` 返回。

## License

MIT
