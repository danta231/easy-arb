# 六交易所双策略 dry-run 监听运行手册

本文记录 Binance、Bybit、OKX、Bitget、Aster、Hyperliquid 六条公开行情链路，以及 `spot-perp-basis`（单所现货-永续基差）和 `cross-exchange-funding-arb`（跨交易所资金费率套利）两个策略的 dry-run 观察方式。该流程只运行公开行情 monitor、`/opportunities` 轮询和受控 dry-run 下单前验证，不传 `--execute-live`，不真实下单、不撤单、不转账、不签名。

## 适用目标

- 实时监听六家交易所的公开行情健康状态和 basis 机会。
- 对支持 auto-once 的 Binance、Bybit、OKX、Bitget basis 候选 symbol 自动跑完整下单前 dry-run 流程。
- 启动专用 `funding-arb-monitor`，从六家本机 basis `/status` 快照聚合 perp top-of-book、盘口数量和 funding rate，生成跨交易所资金费率套利机会，并对真实候选自动触发 funding arb 受控 dry-run 报告。
- 记录两个策略的策略信号、风控决策、人工门禁释放、执行计划构建和分发前检查结果。
- 支持手动运行 1 小时，也可扩展为更长时间的观察。
- Aster 和 Hyperliquid 的 spot-perp basis 能力不足项必须 fail closed，不得当作可执行候选；两者优先用于 perp funding arb。

## 前置条件

在仓库根目录执行：

```bash
cd /Users/danta/WebstormProjects/easy-arb
```

建议启动前先确认脚本语法和配置健康：

```bash
bash -n scripts/start-basis-opportunity-observer.sh
bash -n scripts/stop-basis-opportunity-observer.sh
cargo build -p arb-runtime --features live-exec
target/debug/arb-runtime health-config templates/personal_guarded_live.preflight.yaml
```

健康配置应显示 `execution_mode=GuardedLive`，并且真实执行仍由命令行显式确认控制。

## 手动启动 1 小时监听

推荐用前台监督模式启动：

```bash
BASIS_OBSERVER_FOREGROUND=1 \
BASIS_OBSERVER_STRATEGIES=spot-perp-basis,cross-exchange-funding-arb \
BASIS_OBSERVER_INTERVAL_SECS=5 \
BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS=60 \
BASIS_OBSERVER_STARTUP_WAIT_SECS=180 \
BASIS_OBSERVER_STOP_DRAIN_SECS=15 \
scripts/start-basis-opportunity-observer.sh
```

说明：

- `BASIS_OBSERVER_FOREGROUND=1`：前台监督 monitor 和 recorder，任一核心进程退出会失败退出。
- `BASIS_OBSERVER_STRATEGIES=spot-perp-basis,cross-exchange-funding-arb`：启用两个策略的标准 observer 产物；`spot-perp-basis` 轮询六家 basis `/opportunities`，`cross-exchange-funding-arb` 轮询专用 funding arb `/opportunities`。
- `BASIS_OBSERVER_INTERVAL_SECS=5`：每 5 秒轮询一次六家 basis `/opportunities` 和 funding arb `/opportunities`。
- `BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS=60`：同一交易所同一 symbol 的 basis dry-run、同一 funding arb `pair_id` 的 funding dry-run 至少间隔 60 秒。
- `BASIS_OBSERVER_STARTUP_WAIT_SECS=180`：六交易所首轮公开行情刷新可能超过 1 分钟，启动检查建议保留 180 秒，避免 OKX、Bitget、Aster、Hyperliquid 或 funding arb 在 `starting` 阶段被误判失败。
- `BASIS_OBSERVER_STOP_DRAIN_SECS=15`：停止时先阻止 recorder 继续触发新 validation，并等待已启动 validation 汇总 report，降低停机瞬间 artifact 已生成但 `dry-run-reports.jsonl` 未追加的概率。
- 默认监听 Binance、Bybit、OKX、Bitget、Aster、Hyperliquid 六家。
- 脚本启动时会先检查六家 basis `/opportunities` 和 funding arb `/opportunities`，检查失败不会误报启动成功。

启动成功后会看到七个 dashboard 地址：

```text
http://127.0.0.1:8796/dashboard  # Binance
http://127.0.0.1:8797/dashboard  # Bybit
http://127.0.0.1:8798/dashboard  # OKX
http://127.0.0.1:8803/dashboard  # Bitget
http://127.0.0.1:8800/dashboard  # Aster
http://127.0.0.1:8799/dashboard  # Hyperliquid
http://127.0.0.1:8804/dashboard  # Funding arb
```

## 停止监听

如果在启动终端中运行前台模式，按 `Ctrl-C` 停止。

也可以另开终端执行：

```bash
cd /Users/danta/WebstormProjects/easy-arb
scripts/stop-basis-opportunity-observer.sh
```

停止后确认端口已释放：

```bash
lsof -nP -iTCP:8796 -sTCP:LISTEN
lsof -nP -iTCP:8797 -sTCP:LISTEN
lsof -nP -iTCP:8798 -sTCP:LISTEN
lsof -nP -iTCP:8803 -sTCP:LISTEN
lsof -nP -iTCP:8800 -sTCP:LISTEN
lsof -nP -iTCP:8799 -sTCP:LISTEN
lsof -nP -iTCP:8804 -sTCP:LISTEN
```

没有输出表示端口未被占用。

## 实时观察

最常用的实时反馈：

```bash
tail -f target/arb-opportunity-observer/logs/realtime-feedback.log
```

六家轮询健康状态：

```bash
tail -f target/arb-opportunity-observer/logs/health-events.jsonl
```

所有机会汇总：

```bash
tail -f target/arb-opportunity-observer/opportunities/all-opportunities.jsonl
```

按策略拆分的机会文件：

```bash
tail -f target/arb-opportunity-observer/opportunities/spot-perp-basis.jsonl
tail -f target/arb-opportunity-observer/opportunities/cross-exchange-funding-arb.jsonl
```

dry-run 汇总报告：

```bash
tail -f target/arb-opportunity-observer/dry-run/dry-run-reports.jsonl
```

dry-run 触发事件：

```bash
tail -f target/arb-opportunity-observer/dry-run/validation-events.jsonl
```

单交易所机会文件：

```text
target/arb-opportunity-observer/opportunities/binance-opportunities.jsonl
target/arb-opportunity-observer/opportunities/bybit-opportunities.jsonl
target/arb-opportunity-observer/opportunities/okx-opportunities.jsonl
target/arb-opportunity-observer/opportunities/bitget-opportunities.jsonl
target/arb-opportunity-observer/opportunities/aster-opportunities.jsonl
target/arb-opportunity-observer/opportunities/hyperliquid-opportunities.jsonl
```

## dry-run 结果位置

每次 basis 或 funding arb 机会触发 dry-run 后，会生成一个目录：

```text
target/arb-opportunity-observer/dry-run/<run-id>/
```

basis auto-once 重点查看：

```text
basis_auto_once_report.json
basis_auto_once_report.md
preview/risk_decision.json
preview/plan_preview.json
preview/manual_gate_release_preview.json
market/stored_events.jsonl
market/usdt_futures_funding_rate.raw.json  # Bitget 使用 funding 专用公开接口时存在
```

funding arb 重点查看：

```text
funding_arb_guarded_dry_run_report.json
funding_arb_guarded_dry_run_report.md
funding_arb_candidate_row.json
funding_arb_opportunities_snapshot.input.json
funding_arb_opportunities_snapshot.json
```

其中 `basis_auto_once_report.json` 或 `funding_arb_guarded_dry_run_report.json` 必须包含以下关键字段：

- `signal_allowed`
- `risk_decision`
- `manual_gate_released`
- `dispatch_plan_built`
- `dispatch_request_count`

可以用下面命令检查最近一条报告：

```bash
tail -n 1 target/arb-opportunity-observer/dry-run/dry-run-reports.jsonl | jq '{
  strategy: (.strategy // "spot-perp-basis"),
  venue: (.venue // null),
  pair_or_symbol: (.pair_id // .symbol),
  signal_allowed,
  risk_decision,
  manual_gate_released,
  dispatch_plan_built,
  dispatch_request_count,
  dispatch_attempted,
  dispatch_allowed: (.dispatch_allowed // false),
  mutable_execution_started,
  validation_result_class
}'
```

期望 dry-run 阻断真实分发：

- `dispatch_attempted` 为 `false`
- `dispatch_allowed` 为 `false`
- `mutable_execution_started` 为 `false`
- `blocking_reasons` 中说明当前为 dry-run 模式

## 直接检查 `/opportunities`

```bash
curl -fsS --max-time 10 http://127.0.0.1:8796/api/basis/opportunities | jq '{status,candidate_count,total_rows:(.rows|length),updated_at}'
curl -fsS --max-time 10 http://127.0.0.1:8797/api/bybit-basis/opportunities | jq '{status,candidate_count,total_rows:(.rows|length),updated_at}'
curl -fsS --max-time 10 http://127.0.0.1:8798/api/okx-basis/opportunities | jq '{status,candidate_count,total_rows:(.rows|length),updated_at}'
curl -fsS --max-time 10 http://127.0.0.1:8803/api/bitget-basis/opportunities | jq '{status,candidate_count,total_rows:(.rows|length),updated_at}'
curl -fsS --max-time 10 http://127.0.0.1:8800/api/aster-basis/opportunities | jq '{status,candidate_count,total_rows:(.rows|length),updated_at}'
curl -fsS --max-time 10 http://127.0.0.1:8799/api/hyperliquid-basis/opportunities | jq '{status,candidate_count,total_rows:(.rows|length),updated_at}'
curl -fsS --max-time 10 http://127.0.0.1:8804/api/funding-arb/opportunities | jq '{status,candidate_count,total_rows:(.rows|length),updated_at}'
```

说明：

- `status=healthy` 表示 monitor 已完成公开行情刷新。
- `candidate_count=0` 表示当前没有符合策略条件的机会，不代表链路失败。
- OKX、Bitget、Aster、Hyperliquid 和 funding arb 首轮刷新可能较慢，短时间显示 `starting` 时应继续观察；observer 启动检查只把 `healthy` 当作通过。
- Bitget 的 `fundingRate`、`fundingRateInterval` 和 `nextUpdate` 来自 `/api/v2/mix/market/current-fund-rate?productType=USDT-FUTURES`；`/api/v2/mix/market/tickers?productType=USDT-FUTURES` 只作为 bid/ask、mark 和 index 的公开行情来源，不能再假设 ticker 内含 `nextFundingTime`。
- Aster 和 Hyperliquid 的 spot-perp basis 如果缺少满足能力矩阵的 spot/top-of-book size 或 dry-run execution profile，必须按 fail closed 处理；不要把 monitor row 直接升级成可执行候选。
- `funding-arb-monitor` 只消费本机 basis `/status` 快照；任一交易所缺 perp bid/ask、盘口数量、funding rate 或上游状态异常时，该 pair 会按非候选记录原因。

## 1 小时运行后的汇总检查

查看六家 basis 和 funding arb 轮询成功、失败次数：

```bash
jq -r '[(.venue // .strategy // "observer"),.event] | @tsv' target/arb-opportunity-observer/logs/health-events.jsonl \
  | sort \
  | uniq -c
```

查看触发过 dry-run 的 symbol 或 pair：

```bash
jq -r 'select(.event=="validation_started") | [(.venue // .strategy),(.pair_id // .symbol),.run_id] | @tsv' \
  target/arb-opportunity-observer/dry-run/validation-events.jsonl
```

查看完整下单前流程完成的报告：

```bash
jq -r 'select(.validation_result_class=="pre_trade_flow_complete") | [(.venue // .strategy),(.pair_id // .symbol),.dispatch_request_count,.manual_gate_released,.dispatch_plan_built] | @tsv' \
  target/arb-opportunity-observer/dry-run/dry-run-reports.jsonl
```

## 常见问题

### 启动时报端口占用

检查占用进程：

```bash
lsof -nP -iTCP:8796 -sTCP:LISTEN
lsof -nP -iTCP:8797 -sTCP:LISTEN
lsof -nP -iTCP:8798 -sTCP:LISTEN
lsof -nP -iTCP:8803 -sTCP:LISTEN
```

先用停止脚本清理：

```bash
scripts/stop-basis-opportunity-observer.sh
```

如果仍有残留进程，确认进程属于本仓库的 `arb-runtime` 后再手动停止。

### `/opportunities` 一直是 `starting`

先看对应 monitor 日志：

```bash
tail -n 80 target/arb-opportunity-observer/logs/binance-basis-monitor.log
tail -n 80 target/arb-opportunity-observer/logs/bybit-basis-monitor.log
tail -n 80 target/arb-opportunity-observer/logs/okx-basis-monitor.log
tail -n 80 target/arb-opportunity-observer/logs/bitget-basis-monitor.log
tail -n 80 target/arb-opportunity-observer/logs/aster-basis-monitor.log
tail -n 80 target/arb-opportunity-observer/logs/hyperliquid-basis-monitor.log
tail -n 80 target/arb-opportunity-observer/logs/funding-arb-monitor.log
```

再看公开行情请求错误：

```bash
tail -n 80 target/arb-opportunity-observer/logs/curl-errors.log
```

如果外部状态未知或公开接口超时，应按失败或风险状态处理，不应当作成功。

### 有机会但没有 dry-run report

检查 dry-run 事件：

```bash
tail -n 80 target/arb-opportunity-observer/dry-run/validation-events.jsonl
```

常见原因：

- 同一 symbol 仍在 cooldown 窗口内。
- 已有同一 symbol 的 validation 正在运行。
- auto-once 命令失败，查看对应 `logs/*-dry-run-*.log`。
- 如果 `validation_result_class=input_parse_failed`，优先检查对应 dry-run 目录下的 `market/*.raw.json` 和 `basis_auto_once_report.json` 中的 `blocking_reasons`。
- funding arb 候选失败时，查看对应 `logs/funding-arb-dry-run-*.log`，再检查 dry-run 目录下的 `funding_arb_candidate_row.json`、`funding_arb_opportunities_snapshot.input.json` 和 `funding_arb_guarded_dry_run_report.json`。

### 非 Binance 交易所长时间没有 report

如果 `/opportunities` 为 `healthy` 且 `candidate_count=0`，表示当前没有符合策略条件的真实机会。生产观察中这是正常结果。需要验证 Bybit、OKX、Bitget 的完整下单前流程时，应使用受控测试机会或降低阈值的验收方式，但不要把受控测试结果误记为真实盈利机会。Aster 和 Hyperliquid 当前 spot-perp basis 缺少可执行 dry-run profile，monitor 层必须按非候选 fail closed；两者的 perp/funding 数据仍可供跨所资金费率套利使用。跨所资金费率套利由 `funding-arb-monitor` 独立观察，并在候选通过阈值和 cooldown 后触发 `funding-arb-guarded-dry-run-once`。

### cross-exchange-funding-arb 文件为空

`target/arb-opportunity-observer/opportunities/cross-exchange-funding-arb.jsonl` 是跨交易所资金费率套利的标准产物位置。文件为空通常表示 `funding-arb-monitor` 当前 `candidate_count=0`，或者 `/api/funding-arb/opportunities` 仍未达到 `healthy`。先检查 `http://127.0.0.1:8804/api/funding-arb/status`、`logs/funding-arb-monitor.log` 和六家 basis `/status`；不要手工伪造机会。若该文件有真实候选但 dry-run 报告为空，继续检查 `dry-run/validation-events.jsonl` 和 `logs/funding-arb-dry-run-*.log`。

## 交付判定

一次 1 小时 dry-run 监听可以按以下标准判断：

- 六家 `/opportunities` 至少都有 `poll_ok`，或对外部不可用项记录明确失败原因。
- `poll_failed` 比例可解释，且没有连续不可恢复失败。
- 所有真实 basis 机会都写入 `opportunities/spot-perp-basis.jsonl` 和对应单交易所文件。
- 所有真实 funding arb 机会都写入 `opportunities/cross-exchange-funding-arb.jsonl`，对应 API 为 `/api/funding-arb/opportunities`；通过 cooldown 的候选还应生成 `funding_arb_guarded_dry_run_report.json`，不允许伪造机会。
- 所有触发 dry-run 的机会都写入 `dry-run/dry-run-reports.jsonl`。
- 每条完整报告都包含 `signal_allowed`、`risk_decision`、`manual_gate_released`、`dispatch_plan_built`、`dispatch_request_count`。
- 没有真实下单迹象：`dispatch_attempted=false`、`submitted_receipt_count=0`、`private_confirmation_count=0`。
