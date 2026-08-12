# NodeFlare

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
- 内置 Komari Glass 主题，并提供远程主题商店
- 远程主题仅支持 GitHub `tree` 地址，Worker 代理 `index.html` 与 `assets/`
- Rust Agent 支持 Linux x86_64/ARM64、Windows x86_64 和 macOS ARM64，可按节点自动更新

## 部署到 Cloudflare

1. Fork 本项目后，点击下方按钮进入 Cloudflare 官方部署流程：

   [![Deploy to Cloudflare](https://deploy.workers.cloudflare.com/button)](https://deploy.workers.cloudflare.com/?url=https://github.com/imengying/NodeFlare)

2. Worker 名填写 `nodeflare`（必须与 `wrangler.toml` 一致），使用生产分支 `main`，根目录 `/`，构建命令填写 `bun run build`，部署命令填写 `bun run deploy`。手动配置时，请从 **Workers & Pages → Settings → Builds → Connect** 连接 GitHub 仓库，不要只创建空 Worker。
3. 在创建页展开 **高级设置**，按下表添加变量。

需要查看详细部署日志时，可在项目根目录执行 `wrangler login` 后运行：

```sh
WRANGLER_LOG=debug bun run deploy 2>&1 | tee nodeflare-deploy.log
```

首次直接用命令行部署会自动创建 `nodeflare` D1，后续部署会先执行迁移再上传 Worker。

| 变量 | 必填/可选 | 说明 |
| --- | ---: | --- |
| `ADMIN_USERNAME` | 可选 | 管理员登录用户名，未设置时使用 `admin` |
| `ADMIN_PASSWORD` | 必填 | 初始管理员密码，勾选“加密” |
| `SITE_NAME` | 可选 | 站点名称，未设置时使用 `NodeFlare` |
| `TURNSTILE_SITE_KEY` | 可选 | Turnstile Site Key，无需加密；也可在后台设置 |
| `TURNSTILE_SECRET_KEY` | 可选 | Turnstile Secret Key，勾选“加密”；也可在后台设置 |
| `OFFLINE_THRESHOLD_SECONDS` | 可选 | 离线判定秒数，未设置时使用 180，范围 30-3600 |
| `HISTORY_RETENTION_DAYS` | 可选 | 历史保留天数，未设置时使用 30，范围 1-365 |
| `CF_USAGE_ACCOUNT_ID` | 可选 | Cloudflare 用量查询的账户 ID，不设置则不启用用量查询 |
| `CF_USAGE_API_TOKEN` | 可选 | 用量查询 Token，勾选“加密”；需要 Account Analytics Read 权限 |

## API

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| `GET` | `/api/config` | 公开站点配置 |
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
| `GET/POST` | `/api/admin/themes` | 查询或添加主题 |
| `POST` | `/api/admin/themes/:id/activate` | 启用主题 |
| `POST` | `/api/admin/themes/:id/preview` | 创建短时主题预览链接 |
| `DELETE` | `/api/admin/themes/:id` | 删除主题 |
| `GET` | `/api/admin/theme-settings` | 读取当前前端提供的主题设置描述 |
| `POST` | `/api/turnstile/verify` | 校验全站 Turnstile 令牌 |
| `GET` | `/api/admin/database` | 查询 D1 节点、历史和数据库统计 |
| `GET` | `/api/admin/cloudflare-usage` | 查询 UTC 今日/昨日的 D1 与 Workers 用量 |
| `POST` | `/api/admin/exchange-rates/refresh` | 立即拉取并更新 D1 汇率快照 |
| `DELETE` | `/api/admin/history` | 清理历史指标 |
| `POST` | `/api/admin/notifications/test` | 发送一条通知测试消息 |

## 主题

GitHub `tree` 地址对应的目录需提供 `index.html` 和 `assets/`，并使用 NodeFlare 公开 API 读取数据。启用第三方主题前请确认来源可信。

## 鸣谢

感谢 [CF-Server-Monitor](https://github.com/huilang-me/CF-Server-Monitor) 项目提供的思路。

## License

MIT
