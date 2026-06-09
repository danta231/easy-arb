# 同机云服务器配置建议

本文用于把 `easy-arb` 和 `Easy Tool` 部署到同一台云服务器。`easy-arb` 是 Rust（系统编程语言）工作区；`Easy Tool` 是 Node.js（JavaScript 运行时）+ TypeScript（类型化 JavaScript）+ Vue（前端框架）项目。本配置不把 Node.js 引入 `easy-arb` 的正式项目依赖，只为同机运行另一个项目预留运行环境。

## 推荐规格

先按 `target` 行情范围部署，不做全市场 WSS（WebSocket 行情流）订阅时：

| 场景 | CPU | 内存 | 磁盘 | 说明 |
| --- | --- | --- | --- | --- |
| 最小试运行 | 2 vCPU | 8 GiB | 120 GiB SSD | 只跑 `target` 行情、低并发、PostgreSQL 数据量小。 |
| 推荐生产起点 | 4 vCPU | 16 GiB | 200 GiB NVMe/SSD | 同时跑 `easy-arb`、`Easy Tool`、PostgreSQL、Nginx 和定时备份。 |
| 全市场或研究任务 | 8 vCPU | 32 GiB | 500 GiB NVMe/SSD | `ARB_RUNTIME_LIVE_CEX_WSS_SCOPE=all`、更多回测或研究任务时使用。 |

建议选择 x86_64 架构、Ubuntu 24.04 LTS、固定公网 IP、自动快照和至少 4 GiB swap（交换空间）。如果选择突发型实例，必须观察 CPU credit（CPU 积分）或等价指标；长时间 WSS 和数据库写入更适合稳定性能实例。

外部规格参考：AWS EC2 文档说明 T3/T4g 是 burstable performance instances（突发性能实例），会受 CPU credit 模型影响；阿里云 ECS 文档提示实例可用性按地域变化并有计算型/内存型族；腾讯云 CVM 和 DigitalOcean Droplets 都支持弹性扩缩容或按规格计费。最终购买前按目标地域重新确认可购规格。

## 端口规划

同机部署的关键冲突是 `Easy Tool` 默认 `8787`。本方案让 `Easy Tool runtime` 保留 `8787`，并把 `easy-arb` 的 Binance perp WSS monitor（币安永续行情监听）固定到 `127.0.0.1:8806`：

| 服务 | 监听地址 | 对外暴露 |
| --- | --- | --- |
| Easy Tool runtime（运行时接口） | `0.0.0.0:8787` | 仅 Nginx 代理，不开放安全组端口 |
| Easy Tool Web | `80/443` | 开放 |
| easy-arb portfolio JSON API（组合状态接口） | `127.0.0.1:8805` | 默认不公开，建议由 Easy Tool 代理读取 |
| easy-arb WSS/basis/funding 内部 JSON API | `127.0.0.1:8786-8830+` | 不开放公网 |
| PostgreSQL（关系型数据库） | `127.0.0.1:5432` | 不开放公网 |
| SSH（远程登录） | `22` 或自定义端口 | 只允许你的固定 IP |

云安全组和本机防火墙只开放 `22`、`80`、`443`。不要开放 `8786-8830+`、`8787`、`5432`。

## 目录与用户

建议创建两个 Unix 用户（Linux 系统用户）隔离权限：

```bash
sudo useradd --system --create-home --shell /usr/sbin/nologin easyarb
sudo useradd --system --create-home --shell /usr/sbin/nologin easytool

sudo mkdir -p /opt/easy-arb/current /etc/easy-arb /var/lib/easy-arb
sudo mkdir -p /opt/easy-tool/current /etc/easy-tool /var/backups/easy-tool
sudo chown -R easyarb:easyarb /opt/easy-arb /var/lib/easy-arb
sudo chown -R easytool:easytool /opt/easy-tool /var/backups/easy-tool
sudo chmod 750 /etc/easy-arb /etc/easy-tool
```

凭证类变量放入 `/etc/easy-arb/easy-arb-secrets.env` 和 `/etc/easy-tool/easy-tool.env`，权限使用 `chmod 600`，不要写入仓库、日志、样例或聊天记录。

## 系统包

推荐基础包：

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev ca-certificates curl jq git nginx certbot python3-certbot-nginx postgresql postgresql-client
```

Rust 工具链用于构建 `easy-arb`。Node.js 只用于 `Easy Tool`，不作为 `easy-arb` 项目依赖。若生产机不直接构建，也可以在 CI（持续集成）或构建机产出 release artifact（发布产物）后同步到 `/opt`。

## easy-arb 部署

在服务器上准备代码后先验证：

```bash
cd /opt/easy-arb/current
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask quality-gate
cargo build --release -p arb-runtime --features live-exec
cargo build --release -p arb-wallet-signer
```

安装环境模板和服务：

```bash
sudo cp deploy/env/easy-arb-live.env.example /etc/easy-arb/easy-arb-live.env
sudo install -m 0644 deploy/systemd/easy-arb-runtime-live.service /etc/systemd/system/easy-arb-runtime-live.service
sudo chown root:root /etc/systemd/system/easy-arb-runtime-live.service
sudo chmod 600 /etc/easy-arb/easy-arb-live.env
sudo touch /etc/easy-arb/easy-arb-secrets.env
sudo chmod 600 /etc/easy-arb/easy-arb-secrets.env
sudo systemctl daemon-reload
```

`systemd` 会直接读取 `/etc/easy-arb/easy-arb-live.env` 和 `/etc/easy-arb/easy-arb-secrets.env`，服务启动命令不再额外 `source`（加载）这些文件，因此可以保持 root-only（仅 root 可读）权限。

`/etc/easy-arb/easy-arb-live.env` 默认使用 `ARB_RUNTIME_LIVE_CEX_WSS_SCOPE=target`，用于降低同机部署压力。确认 dry-run（模拟运行）和小额 live（实盘）稳定后，再考虑改成 `all`。

启动和查看：

```bash
sudo systemctl enable --now easy-arb-runtime-live
sudo systemctl status easy-arb-runtime-live
journalctl -u easy-arb-runtime-live -f
```

健康检查：

```bash
curl -fsS http://127.0.0.1:8805/api/portfolio/status | jq '.'
curl -fsS http://127.0.0.1:8786/api/binance-wss-book-ticker/status | jq '{status,total_rows,wss_update_count,last_error}'
```

如果任一外部状态未知、行情健康未知、账户快照未知或订单确认未知，按失败或风险状态处理，不当作成功。

## Easy Tool 部署

`Easy Tool` 已有 `deploy/systemd` 和 `deploy/nginx` 模板。同机运行时可以保留默认运行时端口：

```bash
RUNTIME_SERVER_PORT=8787
EASY_TOOL_HEALTH_BASE_URL=http://127.0.0.1:8787
RUNTIME_ALLOWED_ORIGIN=https://easy-tool.example.com
HISTORY_DATABASE_URL=postgres://easy_tool:<password>@127.0.0.1:5432/easy_tool
HISTORY_PG_POOL_MAX=4
EASY_ARB_CONFIG_ENV_FILE=/etc/easy-arb/easy-arb-live.env
```

这里的 `<password>` 只表示本机要替换的数据库密码，不要把真实值写入仓库。

`Easy Tool` 的 easy-arb 配置页只写入 `EASY_ARB_CONFIG_ENV_FILE` 指向的非密钥 env 文件，并按白名单更新运行参数。凭证仍然放在 `/etc/easy-arb/easy-arb-secrets.env`，不要给页面写入权限。若生产机启用该页面保存功能，需要让 `easy-tool-runtime` 的 systemd（系统服务管理器）沙箱允许写入该 env 文件，并只给 `easytool` 用户或受控用户组写这个文件的权限；保存后仍需重启 `easy-arb-runtime-live` 才会被 systemd 重新加载。

构建和迁移：

```bash
cd /opt/easy-tool/current
npm ci
npm run build
npm run history:migrate
sudo systemctl enable --now easy-tool-runtime
sudo systemctl enable --now easy-tool-healthcheck.timer easy-tool-db-backup.timer
```

`Easy Tool` 的 `server/runtime-server.mjs` 绑定 `0.0.0.0`，所以必须靠云安全组和防火墙阻断 `8787` 公网访问，只允许 Nginx 在本机代理。

## Nginx 与 TLS

合并反向代理模板在 `deploy/nginx/easy-tool-and-easy-arb.conf`。安装示例：

```bash
sudo cp deploy/nginx/easy-tool-and-easy-arb.conf /etc/nginx/sites-available/easy-stack.conf
sudo ln -s /etc/nginx/sites-available/easy-stack.conf /etc/nginx/sites-enabled/easy-stack.conf
sudo nginx -t
sudo systemctl reload nginx
```

上线前替换：

- `easy-tool.example.com` 为 Easy Tool 域名。
- `easy-arb.example.com` 为 easy-arb 只读 JSON API 域名；默认建议删除该 server block（服务块），让 Easy Tool 在本机代理读取。
- `/etc/nginx/.htpasswd-easy-tool` 和 `/etc/nginx/.htpasswd-easy-arb` 为 Basic Auth（基础认证）密码文件。

配置 HTTPS：

```bash
sudo certbot --nginx -d easy-tool.example.com -d easy-arb.example.com
```

如果需要临时直连 easy-arb JSON API，使用 SSH tunnel：

```bash
ssh -L 8805:127.0.0.1:8805 -L 8804:127.0.0.1:8804 user@server
```

然后本地访问 `http://127.0.0.1:8805/api/navigation/pages`。

## 资源与备份

建议：

- 开启云盘自动快照，保留至少 7 天。
- `/var/lib/easy-arb` 和 `/var/backups/easy-tool` 使用独立云盘或至少单独目录监控容量。
- PostgreSQL 使用 `Easy Tool` 已有 `easy-tool-db-backup.timer` 每日备份。
- `easy-arb` 的 `/var/lib/easy-arb/live` 是交易状态和审计产物目录，升级前先停止服务并保留快照。
- 监控 CPU、内存、磁盘 I/O、磁盘剩余空间、网络连接数、Nginx 4xx/5xx、PostgreSQL 连接数。

## 上线顺序

1. 购买推荐生产起点规格，装 Ubuntu 24.04 LTS。
2. 配好安全组，只开放 SSH、HTTP、HTTPS。
3. 创建 `easyarb`、`easytool` 用户和目录。
4. 部署 PostgreSQL，创建 `easy_tool` 数据库和最小权限用户。
5. 部署 Easy Tool，确认 `8787` 本机健康检查通过。
6. 部署 easy-arb，先使用 `target` 行情范围，确认 `8805` 和 WSS 状态。
7. 安装 Nginx、TLS 和 Basic Auth，确认公网只看到预期域名。
8. 开启 systemd（Linux 服务管理器）服务、timer（定时器）、云快照和日志监控。

## 回滚

升级前记录当前 commit（提交）和 release artifact（发布产物）路径。回滚时：

```bash
sudo systemctl stop easy-arb-runtime-live
sudo systemctl stop easy-tool-runtime
sudo ln -sfn /opt/easy-arb/releases/<previous> /opt/easy-arb/current
sudo ln -sfn /opt/easy-tool/releases/<previous> /opt/easy-tool/current
sudo systemctl start easy-tool-runtime
sudo systemctl start easy-arb-runtime-live
```

如果 `easy-arb` 已进入实盘状态，回滚前先按运行手册检查持仓、订单确认、账本和 private-order-events（私有订单事件），不能只按服务进程状态判断成功。

## 参考来源

- AWS EC2 general purpose instance specs（通用实例规格）：https://docs.aws.amazon.com/ec2/latest/instancetypes/gp.html
- Alibaba Cloud ECS instance families（ECS 实例族）：https://www.alibabacloud.com/help/en/ecs/user-guide/overview-of-instance-families
- Tencent Cloud CVM（云服务器）：https://cloud.tencent.com/product/cvm
- DigitalOcean pricing/backups（价格和备份）：https://www.digitalocean.com/pricing
