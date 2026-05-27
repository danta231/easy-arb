# 六交易所双套利策略接入方案（Codex 执行版）

本文用于指导 Codex 分阶段实现“六个交易所 + 两个套利策略”的 dry-run 全路径。目标交易所为 Binance、Bybit、OKX、Bitget、Aster、Hyperliquid。目标策略为现有单所 spot-perp basis 套利策略，以及新增跨交易所资金费率套利策略。

## 总目标

- 保留并稳定现有四交易所 dry-run 全路径。
- 新增 Aster 和 Hyperliquid，最终形成六交易所公开行情监听、策略候选、风控、执行计划 dry-run 和报告链路。
- 新增跨交易所资金费率套利策略，覆盖可提供永续或 swap funding rate 的交易所组合。
- 所有新增链路默认只做 dry-run，不真实下单、不撤单、不转账、不签名。
- 所有外部状态未知时按失败或风险状态处理，不能当作成功。

## 架构边界

- 策略层只放在 `crates/arb-strategies`，只依赖 `arb-strategy-api`。
- 策略只能读取 `StrategyReadContext`，输出 `CandidatePortfolioTransition` 或明确拒绝原因。
- 公开行情解析、WSS/REST monitor 和交易所字段映射放在 `crates/arb-venue-data` 或 `crates/arb-runtime` 的 monitor 入口。
- 风控放在 `crates/arb-risk`，不能在策略层绕过。
- 执行计划和 dry-run 调度放在 `crates/arb-execution`、`crates/arb-venue-exec` 和 `crates/arb-runtime`。
- 不引入 Node.js 作为正式项目依赖。
- 不写入任何密钥、接口密钥、私钥、令牌或凭证。

## 当前基线

Codex 开始前必须先确认当前状态：

```bash
git status --short
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

如果工作区已有用户改动，必须保留并兼容，不得回退。

## 术语约定

- 单所 basis 策略：同一交易所内买 spot、做空 perp/swap，收益来自现货和永续价格基差及资金费率。
- 跨所资金费率套利：在两个交易所的同一标的永续或 swap 上，一边做多、一边做空，收益核心来自资金费率差。
- dry-run 全路径：公开行情读取、标准化事件、策略候选、风控决策、人工门禁预览、执行计划构建、分发前阻断、报告落盘。
- 交易所能力矩阵：用于声明交易所是否支持 spot、perp/swap、funding rate、mark/index、top-of-book size、dry-run 下单计划等能力。

## 总体执行顺序

1. 冻结四交易所现有 dry-run 基线。
2. 抽象并落地交易所能力矩阵。
3. 在现有四交易所上新增跨所资金费率套利策略，只做策略和 pipeline dry-run。
4. 泛化两腿执行 dry-run，使其支持 perp long + perp short。
5. 接入 Aster，并分别跑通两个策略的 dry-run。
6. 接入 Hyperliquid，并分别跑通两个策略的 dry-run。
7. 做六交易所 + 双策略矩阵测试和运行手册更新。

## 阶段 R0：冻结四交易所 dry-run 基线

### 目标

确认 Binance、Bybit、OKX、Bitget 的现有 spot-perp basis dry-run 全路径可重复运行。

### Codex 操作

1. 检查当前命令入口和脚本：
   - `crates/arb-runtime/src/lib.rs`
   - `scripts/start-basis-monitors.sh`
   - `scripts/start-basis-opportunity-observer.sh`
   - `runbooks/basis-dry-run-observer.md`
2. 运行现有测试：

```bash
cargo test -p arb-strategies
cargo test -p arb-runtime basis_monitor
cargo test -p arb-runtime basis_pipeline
cargo test -p arb-runtime --features live-exec basis_guarded_live_auto_once --no-default-features
cargo test --workspace
```

### 验收标准

- 四交易所现有 dry-run 测试全部通过。
- 当前 spot-perp basis 策略行为无回归。
- 未新增真实执行默认开启路径。

## 阶段 R1：交易所能力矩阵

### 目标

建立交易所能力的单一事实来源，后续策略和 pipeline 不再硬编码假设。

### 建议数据结构

新增或扩展运行时内部结构：

```rust
pub struct ArbVenueCapabilityProfile {
    pub venue_family: String,
    pub supports_spot: bool,
    pub supports_linear_perp: bool,
    pub supports_swap: bool,
    pub supports_funding_rate: bool,
    pub supports_mark_index_price: bool,
    pub supports_top_of_book_size: bool,
    pub funding_interval_hours: Option<u64>,
    pub dry_run_execution_supported: bool,
}
```

### 六交易所初始矩阵

| 交易所 | spot-perp basis | 跨所资金费率套利 | 说明 |
| --- | --- | --- | --- |
| Binance | 支持 | 支持 | spot + USD-M perp |
| Bybit | 支持 | 支持 | spot + linear perp |
| OKX | 支持 | 支持 | spot + swap |
| Bitget | 支持 | 支持 | spot + USDT futures |
| Aster | 待接入 | 支持优先 | 先接 perp funding 和 top-of-book，再补 spot |
| Hyperliquid | 有条件支持 | 支持优先 | 先接 perp context 和 funding；spot 深度能力单独确认 |

### Codex 操作

1. 在 `arb-runtime` 中新增能力 profile 或等价结构。
2. 所有 monitor 和 strategy spec 优先读取 profile。
3. 增加单测覆盖每个交易所的 profile。

### 验收标准

- 六交易所 profile 均可被测试读取。
- 不支持某策略的交易所组合必须 fail closed，并返回明确原因。

## 阶段 R2：新增跨所资金费率套利策略

### 策略语义

跨所资金费率套利使用两个 perp/swap 市场：

- 若交易所 A funding rate 高于交易所 B，则 A 做空、B 做多。
- 若交易所 B funding rate 高于交易所 A，则 B 做空、A 做多。
- 候选收益使用统一周期后的 funding spread，扣除双边手续费、滑点、盘口深度缓冲和入场价格偏离保护。

### 建议策略输入

在 `crates/arb-strategies/src/lib.rs` 新增独立结构，不复用 `SpotPerpBasisSignalInput`：

```rust
pub struct CrossExchangeFundingArbSignalInput {
    pub symbol: String,
    pub long_venue_id: String,
    pub short_venue_id: String,
    pub long_best_bid: String,
    pub long_best_ask: String,
    pub long_ask_size: Option<String>,
    pub short_best_bid: String,
    pub short_best_ask: String,
    pub short_bid_size: Option<String>,
    pub long_funding_rate: String,
    pub short_funding_rate: String,
    pub funding_interval_hours: String,
    pub notional_usd: String,
    pub long_taker_fee_bps: i128,
    pub short_taker_fee_bps: i128,
    pub slippage_buffer_bps: i128,
    pub max_entry_price_divergence_bps: i128,
    pub min_net_funding_bps: i128,
}
```

### 建议策略输出

```rust
pub struct CrossExchangeFundingArbSignal {
    pub symbol: String,
    pub gross_funding_spread_bps: i128,
    pub total_cost_bps: i128,
    pub net_funding_bps: i128,
    pub entry_price_divergence_bps: i128,
    pub quantity: String,
    pub expected_funding_usd: String,
    pub fee_estimate_usd: String,
    pub slippage_estimate_usd: String,
    pub is_candidate: bool,
    pub reason: Option<String>,
}
```

### 候选转换要求

候选必须包含两条腿：

- `perp_long`：低 funding 一侧做多。
- `perp_short`：高 funding 一侧做空。

候选必须包含：

- `expected_economics.expected_profit_usd`
- `expected_economics.expected_profit_bps`
- `funding_impact`
- `liquidity_impact`
- `margin_impact`，如果保证金状态未知则必须让后续风控拒绝或要求更多数据
- `required_capital`，分别表达两个交易所账户资金需求
- `assumptions`，说明只使用公开行情和静态手续费，不代表真实执行授权

默认不要写入阻断型 `risk_flags`。如果数据不足，应直接拒绝候选，而不是产出带 `UnknownState` 的候选。

### Codex 操作

1. 在 `arb-strategies` 新增：
   - `CrossExchangeFundingArbStrategy`
   - `CrossExchangeFundingArbStrategyConfig`
   - `CrossExchangeFundingArbSignalInput`
   - `CrossExchangeFundingArbSignal`
   - `evaluate_cross_exchange_funding_arb_signal`
2. 增加策略单测：
   - funding spread 足够时输出候选
   - funding spread 不足时拒绝
   - top-of-book 深度不足时拒绝
   - funding interval 不一致时归一化或拒绝
   - entry price divergence 超阈值时拒绝
   - 配置 notional 为 0 或费用为负时拒绝

### 验收标准

```bash
cargo test -p arb-strategies cross_exchange_funding
```

必须通过，且 `cargo clippy --workspace --all-targets -- -D warnings` 不报错。

## 阶段 R3：新增跨所资金费率 pipeline

### 目标

让新策略从标准化事件进入风控和 dry-run 报告。

### 建议运行时结构

在 `crates/arb-runtime/src/lib.rs` 新增：

- `CrossExchangeFundingArbPipelineSpec`
- `assemble_public_funding_arb_pipeline_from_normalized_events`
- `funding_arb_monitor_snapshot_candidate_events`
- `run_cross_exchange_funding_arb_strategy`

### 标准化事件要求

每个参与交易所至少需要：

- perp/swap `BookTicker`
- `PerpPremiumIndex` 或等价 funding event
- `mark_price`
- `index_price`
- `last_funding_rate`
- `next_funding_time_ms` 或 funding interval
- freshness / risk reason

### 组合枚举规则

六交易所最多形成 15 个无向组合：

```text
Binance-Bybit
Binance-OKX
Binance-Bitget
Binance-Aster
Binance-Hyperliquid
Bybit-OKX
Bybit-Bitget
Bybit-Aster
Bybit-Hyperliquid
OKX-Bitget
OKX-Aster
OKX-Hyperliquid
Bitget-Aster
Bitget-Hyperliquid
Aster-Hyperliquid
```

策略内部根据 funding spread 决定 long/short 方向，不需要把方向组合扩展为 30 个测试用例。

### 验收标准

```bash
cargo test -p arb-runtime funding_arb_pipeline
cargo test -p arb-runtime funding_arb_monitor
```

至少覆盖：

- 两交易所 funding spread 足够时进入 risk。
- funding spread 不足时无候选。
- 某交易所缺 funding 时 fail closed。
- 某交易所 profile 不支持 funding 时 fail closed。
- 数据 stale 时 fail closed。

## 阶段 R4：泛化两腿 dry-run 执行

### 目标

现有 basis dry-run 偏向 spot buy + perp short。跨所资金费率套利需要 perp long + perp short。

### Codex 操作

1. 检查 `arb-execution` 和 `arb-runtime` 中根据 `basis_leg_role` 分支的逻辑。
2. 泛化为更中性的 `arb_leg_role` 或兼容现有字段：
   - `spot_buy`
   - `perp_short`
   - `perp_long`
3. dry-run 执行计划必须能生成两个 perp/swap submit order request。
4. partial fill 处理必须明确残余敞口：
   - long leg 成交、short leg 未成交
   - short leg 成交、long leg 未成交
   - 两腿部分成交数量不一致

### 验收标准

```bash
cargo test -p arb-execution funding_arb
cargo test -p arb-runtime funding_arb_guarded_dry_run
cargo test -p arb-runtime --features live-exec funding_arb_guarded_dry_run --no-default-features
```

dry-run 报告必须显示：

- `signal_allowed`
- `risk_decision`
- `manual_gate_released`
- `dispatch_plan_built`
- `dispatch_request_count = 2`
- `dispatch_attempted = false`
- `mutable_execution_started = false`

## 阶段 R5：接入 Aster

### 接入顺序

1. public REST/WSS 数据解析。
2. funding rate、mark/index、top-of-book size 标准化。
3. basis monitor row 输出。
4. spot-perp basis dry-run。
5. 跨所资金费率套利 dry-run。
6. guarded live auto-once 仍默认 dry-run，真实执行必须阻断。

### Codex 操作入口

优先检查和扩展：

- `crates/arb-venue-data/src/lib.rs`
- `crates/arb-runtime/src/lib.rs`
- `crates/arb-venue-exec/src/lib.rs`
- `scripts/start-basis-monitors.sh`
- `scripts/start-basis-opportunity-observer.sh`

### Aster 验收测试

```bash
cargo test -p arb-runtime aster_basis_monitor
cargo test -p arb-runtime aster_basis_guarded_live_auto_once
cargo test -p arb-runtime aster_funding_arb
```

如果 Aster spot 能力不足，spot-perp basis 必须显式标记不支持或只开放 perp funding arb，不能伪造支持。

## 阶段 R6：接入 Hyperliquid

### 接入顺序

1. perp context 和 funding 数据解析。
2. spot context 或等价 top-of-book 数据解析。
3. funding arb 优先接入。
4. spot-perp basis 根据 spot 能力决定是否开启。
5. dry-run execution market 和 report 接入。

### Codex 操作入口

优先检查和扩展：

- `crates/arb-venue-data/src/lib.rs`
- `crates/arb-runtime/src/lib.rs`
- `crates/arb-venue-exec/src/lib.rs`
- monitor status API（监控状态接口）和 opportunities API（机会接口）

### Hyperliquid 验收测试

```bash
cargo test -p arb-runtime hyperliquid_basis_monitor
cargo test -p arb-runtime hyperliquid_funding_arb
cargo test -p arb-runtime hyperliquid_guarded_dry_run
```

如果 Hyperliquid 的 spot 数据无法提供和其他 CEX 一致的 top-of-book size，spot-perp basis 必须 fail closed 或降低为观察，不得进入可执行候选。

## 阶段 R7：六交易所双策略矩阵

### 目标

形成统一 observer，可同时观察两个策略：

- `spot_perp_basis`
- `cross_exchange_funding_arb`

### 建议输出文件

```text
target/arb-opportunity-observer/opportunities/spot-perp-basis.jsonl
target/arb-opportunity-observer/opportunities/cross-exchange-funding-arb.jsonl
target/arb-opportunity-observer/dry-run/dry-run-reports.jsonl
target/arb-opportunity-observer/logs/health-events.jsonl
```

### CLI 建议

新增或扩展 observer 参数：

```text
--strategies spot-perp-basis,cross-exchange-funding-arb
--venues binance,bybit,okx,bitget,aster,hyperliquid
--symbols BTCUSDT,ETHUSDT
--dry-run
```

### 验收测试

```bash
cargo test -p arb-runtime opportunity_observer
cargo test -p arb-runtime six_venue_strategy_matrix
cargo test --workspace
```

### 运行验收

启动前检查：

```bash
cargo build -p arb-runtime --features live-exec
bash -n scripts/start-basis-monitors.sh
bash -n scripts/start-basis-opportunity-observer.sh
```

短周期 dry-run 观察：

```bash
BASIS_OBSERVER_FOREGROUND=1 \
BASIS_OBSERVER_INTERVAL_SECS=5 \
BASIS_OBSERVER_AUTO_ONCE_COOLDOWN_SECS=60 \
scripts/start-basis-opportunity-observer.sh
```

后续如果脚本改名为通用套利 observer，应同步更新运行手册和测试。

## Codex 分工提示

每次只做一个阶段，不要跨阶段大改。每阶段开始前先执行：

```bash
git status --short
rg -n "相关关键词" crates scripts runbooks
```

每阶段结束必须至少执行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

如果阶段涉及 `live-exec`：

```bash
cargo test -p arb-runtime --features live-exec <相关测试过滤词> --no-default-features
```

## 推荐 Codex 提示词模板

### R2 策略实现提示词

```text
按 runbooks/six-venues-two-strategies-codex-plan.md 的 R2 阶段，实现跨交易所资金费率套利策略。只修改策略层和必要测试，不接运行时 pipeline。保持 arb-strategies 只依赖 arb-strategy-api。完成后运行 cargo fmt --all -- --check、cargo test -p arb-strategies、cargo clippy --workspace --all-targets -- -D warnings。
```

### R3 pipeline 实现提示词

```text
按 runbooks/six-venues-two-strategies-codex-plan.md 的 R3 阶段，把跨交易所资金费率套利策略接入 runtime pipeline。先覆盖 Binance/Bybit/OKX/Bitget，新增 funding_arb_pipeline 和 funding_arb_monitor 测试。不要接 Aster/Hyperliquid。完成后运行相关 runtime 测试和 cargo test --workspace。
```

### R5 Aster 接入提示词

```text
按 runbooks/six-venues-two-strategies-codex-plan.md 的 R5 阶段接入 Aster。先完成公开行情、funding、标准化事件和 monitor，再接 spot-perp basis 与 cross-exchange funding arb 的 dry-run。任何无法确认的外部状态都 fail closed。完成后运行 Aster 相关测试、live-exec dry-run 测试和 workspace 验证。
```

### R6 Hyperliquid 接入提示词

```text
按 runbooks/six-venues-two-strategies-codex-plan.md 的 R6 阶段接入 Hyperliquid。优先完成 perp funding arb，spot-perp basis 只有在 spot top-of-book 能力满足时开启，否则显式 fail closed。完成后运行 Hyperliquid 相关测试、live-exec dry-run 测试和 workspace 验证。
```

## 风险控制清单

- 不允许策略层直接访问网络或账户。
- 不允许 dry-run 报告暗示真实执行成功。
- 不允许缺 funding、缺深度、缺 mark/index 时产生候选。
- 不允许把未知状态写成成功。
- 不允许为了通过测试降低风控阈值。
- 不允许把 `UnknownState` 默认塞进候选后再期望风控放行。
- 不允许在样例、日志、文档或测试中写入真实密钥。

## 最终验收标准

最终完成时必须满足：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask quality-gate
```

并且人工检查：

- 六交易所 monitor 至少能输出健康或明确失败状态。
- 两个策略都能输出机会 JSONL。
- dry-run 报告中真实分发始终被阻断。
- 风控拒绝和 requires-more-data 都有明确 reason code。
- Aster 和 Hyperliquid 的能力不足项不会被默认为成功。
