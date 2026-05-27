# 六交易所双策略长时间全自动实盘运行手册

本文用于已经完成长时间 dry-run 验证后的正式实盘阶段。入口固定使用 `scripts/start-arb-runtime-live.sh`，由它先启动公开 WSS monitor，再启动 `arb-runtime live --i-understand-live-orders`，进入监控、筛选、风控、下单、平仓监督的常驻流程。

本手册不包含任何 API key、secret、私钥、token、助记词、签名命令正文或 webhook secret。所有凭证和签名材料只能放在本机未提交的环境变量、密钥管理器或本地未跟踪文件中。

## 当前闭环边界

- `spot-perp-basis`：常驻实盘当前覆盖 Binance、Bybit、OKX、Bitget 的现货和永续基差闭环。Aster、Hyperliquid 的 spot-perp basis 仍按 fail closed 处理，不作为该策略的实盘开仓来源。
- `cross-exchange-funding-arb`：常驻实盘覆盖 Binance、Bybit、OKX、Bitget、Aster、Hyperliquid 六个交易所的 perp 资金费率套利链路。
- WSS 前置：启动脚本会启动 Binance、Bybit、OKX、Bitget 的 spot/perp WSS，以及 Aster、Hyperliquid 的 perp WSS，并等待 `status=streaming`、`total_rows>0`、`wss_update_count>0` 后才进入实盘 runtime。
- 如果任一外部状态未知、WSS 健康未知、账户/仓位只读快照未知、订单确认未知，按风险状态处理，不按成功处理。

## 路径约束

长时间实盘阶段建议先使用默认路径：

```text
target/arb-runtime/live
target/arb-runtime/live-prereq
```

外层启动脚本会把 `ARB_RUNTIME_LIVE_ROOT` 显式传给 `arb-runtime live --out`，portfolio JSON API 的 `--resident-root` 和停止脚本也读取同一目录。自定义 `ARB_RUNTIME_LIVE_ROOT` 或 `ARB_RUNTIME_LIVE_PREREQ_ROOT` 后，仍建议先做一次短周期前台验证，确认 live artifacts、只读 API 和停止脚本读取的是同一组目录。

## 启动前检查

在仓库根目录执行：

```bash
cd /Users/danta/WebstormProjects/easy-arb
```

版本切换、拉取新代码或改动实盘相关代码后，先跑完整本地验证：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

实盘启动前至少确认脚本语法、live-exec 构建和配置健康：

```bash
bash -n scripts/start-arb-runtime-live.sh
bash -n scripts/stop-arb-runtime-live.sh
bash -n scripts/start-basis-opportunity-observer.sh
cargo build -p arb-runtime --features live-exec
target/debug/arb-runtime health-config templates/personal_guarded_live.preflight.yaml
```

`health-config` 必须明确看到 `GuardedLive` 配置形态，并确认 `kill_switch.global=false`、`kill_switch.execution=false` 是本次实盘的主动选择。如果任何 kill switch 被打开，执行链路应 fail closed，不要绕过。

## 本地环境文件

建议使用项目外的本地未提交文件，例如 `~/easy-arb-live.env`。文件权限建议限制为当前用户可读写：

```bash
chmod 600 ~/easy-arb-live.env
```

下面是变量清单模板。带 `<...>` 的值必须由本机真实配置替换，不能原样启动；不要把实际密钥或签名命令正文写入本手册、提交记录、日志或聊天内容。

```bash
BASIS_OBSERVER_CONFIG=templates/personal_guarded_live.preflight.yaml # 风控和执行配置文件路径，长时间实盘默认使用个人 guarded live 配置。
BASIS_OBSERVER_STRATEGIES=spot-perp-basis,cross-exchange-funding-arb # 启用的策略列表，默认同时运行现货-永续 basis 和跨交易所资金费率套利。
BASIS_OBSERVER_MIN_NET_BPS=5 # 最小净收益阈值，单位 bps；低于该阈值的机会不会进入执行。
BASIS_OBSERVER_MIN_ABS_FUNDING_RATE=0 # 最小绝对资金费率过滤阈值，0 表示不按资金费率绝对值预过滤。
BASIS_OBSERVER_NOTIONAL_USD=100.00 # 单次候选机会用于计算和下单的目标名义本金，单位美元。
BASIS_OBSERVER_AUTO_PRICE_GUARD_BPS=2 # 自动价格保护缓冲，单位 bps，用于限制可接受成交价偏离。

ARB_RUNTIME_LIVE_WSS_READY_TIMEOUT_SECS=180 # 等待全部 WSS monitor 进入 streaming 且收到真实更新的最长秒数。

BASIS_OBSERVER_BASIS_RESIDENT_INTERVAL_SECS=60 # spot-perp-basis 常驻 runner 每轮扫描间隔秒数。
BASIS_OBSERVER_BASIS_RESIDENT_MAX_LIVE_ENTRIES=1 # spot-perp-basis 单轮最多新开实盘 entry 数。
BASIS_OBSERVER_BASIS_RESIDENT_MAX_CONCURRENT_POSITIONS=1 # spot-perp-basis 最多同时持有的未平仓 position 数。
BASIS_OBSERVER_BASIS_RESIDENT_MAX_TOTAL_NOTIONAL_USDT=100.00 # spot-perp-basis 总名义本金上限，单位 USDT。

BASIS_OBSERVER_PERP_TARGET_LEVERAGE=1 # 所有永续交易所默认目标杠杆；非 reduce-only 实盘开仓前会先设置该杠杆。
BASIS_OBSERVER_BINANCE_USDM_LEVERAGE=1 # 可选覆盖 Binance USD-M 永续目标杠杆。
BASIS_OBSERVER_BYBIT_LINEAR_LEVERAGE=1 # 可选覆盖 Bybit linear 永续目标杠杆。
BASIS_OBSERVER_OKX_SWAP_LEVERAGE=1 # 可选覆盖 OKX swap 永续目标杠杆。
BASIS_OBSERVER_BITGET_USDT_FUTURES_LEVERAGE=1 # 可选覆盖 Bitget USDT-FUTURES 目标杠杆。
BASIS_OBSERVER_ASTER_PERP_LEVERAGE=1 # 可选覆盖 Aster USDT perp 目标杠杆。
BASIS_OBSERVER_HYPERLIQUID_PERP_LEVERAGE=1 # 可选覆盖 Hyperliquid perp 目标杠杆。

BASIS_OBSERVER_FUNDING_ARB_MODE=resident # cross-exchange-funding-arb 运行模式；resident 表示常驻扫描、入场和退出监督。
BASIS_OBSERVER_FUNDING_ARB_RESIDENT_INTERVAL_SECS=60 # cross-exchange-funding-arb 常驻 runner 每轮扫描间隔秒数。
BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_LIVE_ENTRIES=1 # cross-exchange-funding-arb 单轮最多新开实盘 entry 数。
BASIS_OBSERVER_FUNDING_ARB_RESIDENT_MAX_CYCLES= # cross-exchange-funding-arb 最大循环次数；留空表示长期运行。

BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT=target/arb-runtime/live/private-readonly/funding_settlement_raw_snapshot.json # 资金费率结算原始只读快照输出路径，当前推荐启用。
BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER= # 稳定结算账本输入路径；接 raw snapshot 时必须留空，不能同时启用。

ARB_RUNTIME_PORTFOLIO_ACCOUNT_SNAPSHOT= # portfolio JSON API 可选账户快照覆盖输入；留空时从 resident root 自动发现私有只读账户快照。
ARB_RUNTIME_PORTFOLIO_POSITION_SNAPSHOT= # portfolio JSON API 可选仓位快照覆盖输入；留空时从 resident root 自动汇总常驻仓位。

ASTER_USER=<aster-user-address> # Aster 账户/user 地址，用于账户归属、查询和订单归属。
ASTER_SIGNER=<aster-signer-address> # Aster 实际签名/API 地址，必须与 signer 私钥匹配。
ASTER_SIGNER_PRIVATE=<aster-signer-private-key> # Aster signer/API 地址对应的私钥，只放在本机 env 文件，不要提交或写入日志。

HYPERLIQUID_USER=<hyperliquid-user-address> # Hyperliquid 账户/user 地址，用于账户归属、查询和订单归属。
HYPERLIQUID_SIGNER=<hyperliquid-signer-address> # Hyperliquid 实际签名/API/agent 地址，必须与 signer 私钥匹配。
HYPERLIQUID_SIGNER_PRIVATE=<hyperliquid-signer-private-key> # Hyperliquid signer/API/agent 地址对应的私钥，只放在本机 env 文件，不要提交或写入日志。
```

如果某个交易所的 user 和 signer 确认是同一个地址，可以改用便捷别名：`ASTER_ADDRESS` + `ASTER_PRIVATE_KEY`，或 `HYPERLIQUID_ADDRESS` + `HYPERLIQUID_PRIVATE_KEY`。如果两者不同，必须使用上面的 user/signer 分离配置，不要用单地址别名。

高级覆盖项只在确实需要时再加：`BASIS_OBSERVER_ASTER_USER`、`BASIS_OBSERVER_ASTER_SIGNER`、`ASTER_SIGNER_ADDRESS`、`ASTER_API_ADDRESS`、`ARB_WALLET_SIGNER_PATH`、`BASIS_OBSERVER_HYPERLIQUID_SOURCE`、`HYPERLIQUID_SIGNER_ADDRESS`、`HYPERLIQUID_API_ADDRESS`、`BASIS_OBSERVER_HYPERLIQUID_VAULT_ADDRESS`、`BASIS_OBSERVER_HYPERLIQUID_EXPIRES_AFTER_MS`、`BASIS_OBSERVER_HYPERLIQUID_ASSET_IDS`。其中 `BASIS_OBSERVER_HYPERLIQUID_ASSET_IDS` 只是自动解析失败或你要强制指定 asset id 时的覆盖项，不再是默认必填项。

杠杆设置是开仓前置门禁：Binance USD-M、Bybit linear、OKX swap、Bitget USDT-FUTURES、Aster perp、Hyperliquid perp 都会在非 reduce-only 下单前设置 `BASIS_OBSERVER_PERP_TARGET_LEVERAGE` 或对应交易所覆盖值。默认值是 1；如果交易所返回拒绝、网络状态未知或配置值超出 1 到 125，开仓会 fail closed。reduce-only 平仓不会先改杠杆，避免事故退出被杠杆设置拦住。

`BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT` 是可写 JSON 快照路径，用来镜像最新资金费率结算原始只读快照，并给退出和平仓监督做 reconciliation 输入。它不是人工维护的最终 ledger。只有当 raw snapshot 结构和 symbol、账户、币种归一化都稳定后，再切到 `BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER`。

`BASIS_OBSERVER_FUNDING_SETTLEMENT_RAW_SNAPSHOT` 和 `BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER` 不能同时设置；脚本会拒绝组合使用。

## 启动

首次进入长时间实盘建议使用 detach 模式，让进程独立于当前终端：

```bash
scripts/start-arb-runtime-live.sh --detach --env-file ~/easy-arb-live.env
```

启动脚本会先构建 `arb-runtime` 的 `live-exec` 版本和本地 `arb-wallet-signer`，然后启动 10 个 WSS monitor：

```text
127.0.0.1:8786  Binance spot WSS
127.0.0.1:8806  Binance perp WSS
127.0.0.1:8788  Bybit spot WSS
127.0.0.1:8789  Bybit perp WSS
127.0.0.1:8790  OKX spot WSS
127.0.0.1:8791  OKX perp WSS
127.0.0.1:8792  Bitget spot WSS
127.0.0.1:8793  Bitget perp WSS
127.0.0.1:8794  Aster perp WSS
127.0.0.1:8795  Hyperliquid perp WSS
```

随后启动：

```text
target/debug/arb-runtime live --i-understand-live-orders
```

该入口会把 `BASIS_OBSERVER_EXECUTE_LIVE=1` 和 `BASIS_OBSERVER_LIVE_ACK=1` 传给常驻 observer。不要直接运行底层 observer 进入实盘，除非是在诊断脚本本身。

`scripts/start-arb-runtime-live.sh` 会把 `BASIS_OBSERVER_CONFIG`、`BASIS_OBSERVER_MIN_NET_BPS`、`BASIS_OBSERVER_INTERVAL_SECS`、`BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS` 和 `BASIS_OBSERVER_VALIDATE_AUTO_ONCE` 映射为 `arb-runtime live` 参数；也可以用同名 `ARB_RUNTIME_LIVE_*` 变量覆盖这些入口参数。改动阈值后先跑 dry-run 或小额实盘验证，不要直接切到大额长期运行。

启动前脚本会默认回收本仓库 `target/.../arb-runtime` 残留进程占用的只读 API、WSS 和 basis/funding monitor 端口，避免旧 monitor 占用 `127.0.0.1:8804` 这类端口导致新 live 进程启动失败。如果端口被非本仓库进程占用，脚本会提前报错并要求手动处理；如需禁用自动回收，设置 `ARB_RUNTIME_LIVE_RECLAIM_STALE_MONITOR_PORTS=0` 或 `BASIS_OBSERVER_RECLAIM_STALE_MONITOR_PORTS=0`。

## 启动后健康检查

先看启动日志。前台运行时，启动脚本自身输出写入 `arb-runtime-live-precheck.log`；后台运行（`--detach`，即分离到后台）时，`arb-runtime live` 的进程输出另写入 `arb-runtime-live.log`。

```bash
tail -f target/arb-runtime/live-prereq/logs/arb-runtime-live-precheck.log
tail -f target/arb-runtime/live/logs/realtime-feedback.log
```

仅后台运行（`--detach`，即分离到后台）时再看 live 进程输出：

```bash
tail -f target/arb-runtime/live-prereq/logs/arb-runtime-live.log
```

检查 WSS 状态。全部必须是 `streaming`，且 `wss_update_count` 持续增加：

```bash
for url in \
  http://127.0.0.1:8786/api/binance-wss-book-ticker/status \
  http://127.0.0.1:8806/api/binance-wss-book-ticker/status \
  http://127.0.0.1:8788/api/bybit-wss-book-ticker/status \
  http://127.0.0.1:8789/api/bybit-wss-book-ticker/status \
  http://127.0.0.1:8790/api/okx-wss-book-ticker/status \
  http://127.0.0.1:8791/api/okx-wss-book-ticker/status \
  http://127.0.0.1:8792/api/bitget-wss-book-ticker/status \
  http://127.0.0.1:8793/api/bitget-wss-book-ticker/status \
  http://127.0.0.1:8794/api/aster-wss-book-ticker/status \
  http://127.0.0.1:8795/api/hyperliquid-wss-book-ticker/status
do
  echo "$url"
  curl -fsS --max-time 5 "$url" | jq '{status,total_rows,wss_update_count,fail_closed,last_error,updated_at}'
done
```

检查六个 basis monitor 和 funding arb monitor：

```bash
curl -fsS --max-time 10 http://127.0.0.1:8796/api/basis/status | jq '{status,total_rows,candidate_count,updated_at,last_error}'
curl -fsS --max-time 10 http://127.0.0.1:8797/api/bybit-basis/status | jq '{status,total_rows,candidate_count,updated_at,last_error}'
curl -fsS --max-time 10 http://127.0.0.1:8798/api/okx-basis/status | jq '{status,total_rows,candidate_count,updated_at,last_error}'
curl -fsS --max-time 10 http://127.0.0.1:8803/api/bitget-basis/status | jq '{status,total_rows,candidate_count,updated_at,last_error}'
curl -fsS --max-time 10 http://127.0.0.1:8800/api/aster-basis/status | jq '{status,total_rows,candidate_count,updated_at,last_error}'
curl -fsS --max-time 10 http://127.0.0.1:8799/api/hyperliquid-basis/status | jq '{status,total_rows,candidate_count,updated_at,last_error}'
curl -fsS --max-time 10 http://127.0.0.1:8804/api/funding-arb/status | jq '{status,total_rows,candidate_count,updated_at,last_error}'
```

看 portfolio JSON API 的聚合状态：

```bash
curl -fsS --max-time 10 http://127.0.0.1:8805/api/portfolio/status | jq '.'
curl -fsS --max-time 10 http://127.0.0.1:8805/api/portfolio/positions | jq '.'
```

只读 JSON API 入口：

```text
http://127.0.0.1:8805/api/navigation/pages
http://127.0.0.1:8805/api/portfolio/status
http://127.0.0.1:8805/api/errors/logs
http://127.0.0.1:8804/api/funding-arb/status
```

## 实时查看下单门禁错误

优先查看错误日志 API：

```text
http://127.0.0.1:8805/api/errors/logs
```

该 API 会聚合本次运行目录中的 `logs/*.log`、`logs/*.jsonl`、`live/live-reports.jsonl`、`live/validation-events.jsonl` 和 resident event JSONL，返回所有已收集到的错误、阻断、未知状态和门禁通过后下单失败事件。

`funding_arb_resident_live_events.jsonl` 是 `cross-exchange-funding-arb` 常驻 runner 的门禁和下单事件流。启动后如果 Easy Tool 页面或只读 API 显示“待校验”“下单阻断”或签名失败，先实时跟这个文件：

运行方式：先按“启动”小节启动实盘进程，然后另开一个终端，在仓库根目录完整复制下面命令。默认运行目录是 `target/arb-runtime/live`；如果启动时设置了自定义 `ARB_RUNTIME_LIVE_ROOT`，把下面的 `RUN_ROOT` 改成同一个目录。不要把 `...` 省略号当作 jq 程序执行。

```bash
cd /Users/danta/WebstormProjects/easy-arb
RUN_ROOT=target/arb-runtime/live

tail -F "$RUN_ROOT/resident-live/cross-exchange-funding-arb/funding_arb_resident_live_events.jsonl" \
| jq -r --unbuffered '
  def reasons:
    if (.blocking_reason_details | type) == "array" then .blocking_reason_details
    elif (.blocking_reasons | type) == "array" then .blocking_reasons
    elif .reason then [.reason]
    elif .error then [.error]
    else [] end;

  select(.event_type == "candidate_cycle" or .event_type == "no_candidate" or .event_type == "cycle_error")
  | [
      (.recorded_at // "-"),
      .event_type,
      (.pair_id // "-"),
      (.symbol // "-"),
      ((reasons | map(select(. != null)) | join(" | ")))
    ]
  | @tsv
'
```

如果要实时查看“下单前门禁已通过，但真实分发或下单仍失败”的事件，用下面的结构化过滤命令。这里的“门禁已通过”按字段判断：`dry_run_live_ready=true`、`manual_gate_released=true`、`dispatch_plan_built=true`，并且已经进入 `dispatch_attempted` 或 `mutable_execution_started`。它会覆盖所有交易所和所有失败原因，不按错误字符串白名单过滤。

```bash
cd /Users/danta/WebstormProjects/easy-arb
RUN_ROOT=target/arb-runtime/live

tail -F "$RUN_ROOT/resident-live/cross-exchange-funding-arb/funding_arb_resident_live_events.jsonl" \
| jq -r --unbuffered '
  def reasons:
    if (.blocking_reason_details | type) == "array" then .blocking_reason_details
    elif (.blocking_reasons | type) == "array" then .blocking_reasons
    elif .reason then [.reason]
    elif .error then [.error]
    else [] end;

  select(.event_type == "candidate_cycle")
  | select(
      (.dry_run_live_ready == true)
      and (.manual_gate_released == true)
      and (.dispatch_plan_built == true)
      and ((.dispatch_attempted == true) or (.mutable_execution_started == true))
      and ((.dispatch_allowed == false) or ((reasons | length) > 0))
    )
  | [
      (.recorded_at // "-"),
      (.pair_id // "-"),
      (.symbol // "-"),
      ("dispatch_attempted=" + ((.dispatch_attempted // false) | tostring)),
      ("mutable_execution_started=" + ((.mutable_execution_started // false) | tostring)),
      ((reasons | map(select(. != null)) | join(" | ")))
    ]
  | @tsv
'
```

也可以直接轮询 funding arb 的下单门禁 API，确认当前读到的最新 `observed_at`、交易对和阻断原因：

```bash
cd /Users/danta/WebstormProjects/easy-arb

while true; do
  curl -sS http://127.0.0.1:8804/api/funding-arb/execution-status \
  | jq -r '.rows[] | [.observed_at,.pair_id,.dispatch_status,((.blocking_reasons // []) | join(" | "))] | @tsv'
  sleep 2
done
```

`target/arb-runtime/live/logs/realtime-feedback.log` 适合看整体机会、健康状态和轮询节奏；下单门禁、私有只读快照失败、签名器退出、第一腿/第二腿下单失败等细节，以 `funding_arb_resident_live_events.jsonl` 和 `/api/funding-arb/execution-status` 为准。

## 实盘产物位置

常驻日志：

```text
target/arb-runtime/live/logs/realtime-feedback.log
target/arb-runtime/live-prereq/logs/arb-runtime-live-precheck.log
target/arb-runtime/live-prereq/logs/arb-runtime-live.log # 仅后台运行（--detach）时创建
target/arb-runtime/live-prereq/logs/portfolio-dashboard.log
```

策略产物：

```text
target/arb-runtime/live/resident-live/spot-perp-basis
target/arb-runtime/live/resident-live/cross-exchange-funding-arb
target/arb-runtime/live/live/live-reports.jsonl
target/arb-runtime/live/private-order-events
target/arb-runtime/live/opportunities/all-opportunities.jsonl
target/arb-runtime/live/opportunities/spot-perp-basis.jsonl
target/arb-runtime/live/opportunities/cross-exchange-funding-arb.jsonl
```

资金费率 raw snapshot：

```text
target/arb-runtime/live/private-readonly/funding_settlement_raw_snapshot.json
```

如果该文件不存在、不是合法 JSON、`status` 非健康状态，或账户和 symbol 无法匹配，退出和平仓监督必须按未知结算状态处理。

## 长时间运行规则

- 首次长时间实盘保持 `max_live_entries=1`、`max_concurrent_positions=1` 和小额 `notional`。连续稳定后一次只提升一个维度。
- 每次提升名义本金、并发数、交易所范围或 funding threshold 前，先回看最近一段 `live-reports.jsonl`、`private-order-events` 和 portfolio positions；涉及入口参数的调整必须先确认脚本传参实际生效。
- `wss_update_count` 不增长、`last_error` 非空、basis/funding arb 状态过旧、portfolio source error 非空时，不要新增仓位。
- Aster、Hyperliquid 的 WSS 是 funding arb 命中率、行情延迟和风控通过率的前置输入。任一缺失或不健康时，相关 funding arb 候选不应视为可执行。
- 任何仓位状态进入 `unknown` 后，新开仓应停止，先处理未知仓位和残余风险。
- 不用 `BASIS_OBSERVER_FUNDING_SETTLEMENT_LEDGER` 直接替代 raw snapshot，除非已经用 raw snapshot 证明交易所原始结算字段、symbol 归一化、账户归属和时间窗口长期稳定。

## 停止

正常停止：

```bash
scripts/stop-arb-runtime-live.sh
```

停止脚本会停止 live observer、portfolio JSON API 和 WSS monitor。它停止的是自动化进程，不保证交易所真实仓位已经平掉。停止后必须检查：

```bash
curl -fsS --max-time 10 http://127.0.0.1:8805/api/portfolio/positions | jq '.'
tail -n 20 target/arb-runtime/live/live/live-reports.jsonl
tail -n 20 target/arb-runtime/live/resident-live/cross-exchange-funding-arb/funding_arb_resident_positions.jsonl
```

如果只读 API 已停止或状态未知，直接到交易所后台或只读私有快照确认仓位。存在真实持仓时，按 reduce-only 或交易所后台手动降风险，不要假设 stop 脚本已经完成平仓。

## 释放 funding-arb unknown 仓位门禁

`funding_arb_resident_positions.jsonl` 中存在 `position_unknown` 时，resident 会停止新开仓。这不是人工审批门禁，而是残余风险门禁；不能通过删除 lock 或清空文件释放。

如果 `position_unknown` 记录带有原始 `cycle_dir`，新版 resident 在启动后会先尝试从原始 cycle 快照恢复一个可管理的 position state，然后进入退出监督，只提交 IOC reduce-only 退出/降风险订单；恢复期间仍不会开新仓。若恢复或退出再次产生未知状态，resident 会重新写入 `position_unknown` 并停止。

只有在已经确认交易所侧订单、挂单和仓位全部无风险时，才允许人工释放门禁。确认后用脚本只追加一条 `position_closed` 审计记录：

```bash
scripts/release-funding-arb-unknown-position.sh \
  --position-id pos:funding-arb:manual-reconcile:binance-bybit-chipusdt:1 \
  --order-id 2d8fc08c-0f64-4fa5-8340-2583892bcbd8 \
  --confirmed-flat \
  --dry-run
```

确认 dry-run 输出无误后移除 `--dry-run` 再执行。脚本不会查询交易所、下单、撤单或写入任何凭证，只会追加本地审计记录；下一次启动 resident 时会重新读取 registry 并释放该 unknown 门禁。存在真实仓位时不要执行释放脚本，必须先等待 resident 的 reduce-only 退出或在交易所后台手动降风险。

## 紧急处理

满足任一条件时，进入紧急处理：

- WSS 或 basis/funding arb monitor 进入 fail closed、停止更新或连续报错。
- 订单 receipt、exchange confirmation、private position snapshot 三者无法互相印证。
- funding arb 结算状态未知，且持仓已跨过预期资金费率结算窗口。
- portfolio JSON API 或 Easy Tool 页面显示 source error、unknown position 或无法读取 resident root。
- 本地时间、交易所时间、结算时间窗口明显不一致。

处理顺序：

1. 停止自动化：`scripts/stop-arb-runtime-live.sh`。
2. 从交易所后台或只读私有快照确认真实仓位和挂单。
3. 取消非 reduce-only 挂单。
4. 对残余真实仓位执行 reduce-only 降风险或手动平仓。
5. 记录 incident：保留 `target/arb-runtime/live`、`target/arb-runtime/live-prereq`、`private-order-events`、`live-reports.jsonl` 和交易所确认记录。
6. 未完成复盘前，不要重新启动实盘。

## 复盘检查

每个实盘时段结束后至少检查：

```bas
tail -n 50 target/arb-runtime/live/live/live-reports.jsonl | jq -c '.'
tail -n 50 target/arb-runtime/live/opportunities/cross-exchange-funding-arb.jsonl | jq -c '.'
tail -n 50 target/arb-runtime/live/private-order-events/*.jsonl
```

复盘结论必须能回答：

- 开仓候选是否来自健康 WSS 和健康 monitor。
- 策略信号、风控、价格保护、签名、下单确认是否完整。
- 平仓监督是否按预期触发，是否存在 unknown position。
- raw settlement snapshot 是否能解释资金费率收入或支出。
- 当前限额是否应该保持、降低或只在下一次 dry-run 后调整。
