# Data Schemas and Contracts / 数据合同

This chapter defines schemas as executable contracts, not illustrative examples.

中文说明：本章将 schema 定义为“可执行合同”，不是示意字段列表。Rust 结构体、事件 payload、账本分录、回放 fixture 和跨进程消息都必须以这些 schema 为准。

## Canonical schema set / 权威 schema 集合

Core contract schemas:

- `common.defs.schema.json` - shared identifiers, versions, timestamps, decimal strings, hashes, reason codes. 中文：通用基础类型。
- `asset.schema.json` - canonical asset definition. 中文：资产定义。
- `instrument.schema.json` - canonical instrument definition. 中文：交易、借贷、报价、结算或观察对象定义。
- `portfolio_state.schema.json` - normalized portfolio state. 中文：跨场所组合状态快照。
- `normalized_event.schema.json` - canonical event envelope. 中文：统一事件信封。
- `candidate_portfolio_transition.schema.json` - strategy output contract. 中文：策略候选组合转换输出。
- `risk_decision.schema.json` - risk decision contract. 中文：风控决策合同。
- `execution_plan.schema.json` - executable plan contract. 中文：风控批准后的唯一可执行计划。
- `execution_report.schema.json` - execution outcome contract. 中文：执行结果报告。
- `fill.schema.json` - fill record contract. 中文：成交记录。
- `ledger_entry.schema.json` - append-only double-entry ledger contract. 中文：追加式复式账本分录。
- `venue_capability.schema.json` - capability-based venue descriptor. 中文：能力模型场所描述。
- `incident.schema.json` - incident record. 中文：事故记录。

## Contract rules / 合同规则

- Schemas are canonical across implementation languages. 中文：schema 是跨语言权威来源。
- Rust structs are valid only if they serialize to and deserialize from the canonical schema without lossy conversion. 中文：Rust 类型必须可无损往返 schema。
- Every schema must set `$id`, `description`, `required`, and `additionalProperties: false` unless the field is intentionally open-ended. 中文：除明确开放的 payload 外，禁止隐式字段漂移。
- Money, quantity, price, basis points, PnL, and rates must use decimal strings, never JSON floats. 中文：金额、数量、价格、bps、PnL、利率都用 decimal 字符串，Rust 禁止用 `f64` 表达。
- Reason codes must be enumerated and machine-readable. Free text is supplemental only. 中文：原因码必须可聚合，自由文本只能补充说明。
- Incident records must include `source_event_refs` with at least one event ID. 中文：事故记录必须包含至少一个触发或证明该事故的事件 ID，保证运营报告可追溯。
- Ledger adjustment entries must carry `adjustment_reason_code`; operator detail belongs in leg `memo`. 中文：账本调整分录必须携带 `adjustment_reason_code`，人工说明只能放在分录腿的 `memo`。
- Event ordering for replay must use event-store sequence first, then source sequence when available, then timestamp only as a tie-breaker. 中文：回放排序优先使用事件存储序号，不依赖墙钟时间。

## Versioning / 版本策略

- Breaking schema changes require a new major version. 中文：破坏性变更必须升主版本。
- Additive fields require explicit default handling in Rust. 中文：新增字段必须定义 Rust 默认值或迁移规则。
- Deprecated fields remain readable until a migration plan is complete. 中文：弃用字段在迁移完成前仍需可读。
- Events store both `event_version` and `schema_version`. 中文：事件同时携带事件版本和 schema 版本。
- Replay must load historical schema versions. 中文：回放必须能加载历史 schema。

## Rust validation requirements / Rust 校验要求

Rust implementation should include these checks in CI:

- Parse every schema as JSON. 中文：所有 schema 必须是合法 JSON。
- Validate positive and negative fixtures for every schema. 中文：每个 schema 都要有正例和反例 fixture。
- Round-trip Rust domain structs through JSON and compare canonical hashes. 中文：Rust 结构体 JSON 往返后必须保持规范哈希一致。
- Reject unknown fields where `additionalProperties: false`. 中文：严格拒绝未知字段，防止合同漂移。
- Run replay golden tests with fixed event sequences, fixed config versions, fixed policy versions, and fixed strategy versions. 中文：回放黄金测试必须锁定事件、配置、策略和风控版本。

## Reason code examples / 原因码示例

Reason codes are not free text. Examples:

- `NO_EDGE`
- `APPROVED`
- `APPROVED_WITH_CONSTRAINTS`
- `REQUIRES_MORE_DATA`
- `DATA_STALE`
- `VENUE_UNHEALTHY`
- `RATE_LIMITED`
- `INSUFFICIENT_LIQUIDITY`
- `HIGH_FEE`
- `HIGH_SLIPPAGE`
- `HIGH_GAS`
- `HIGH_FEE_AND_SLIPPAGE`
- `INSUFFICIENT_BALANCE`
- `CAPITAL_RESERVED`
- `MARGIN_INSUFFICIENT`
- `LIQUIDATION_TOO_CLOSE`
- `FUNDING_UNSTABLE`
- `EXECUTION_MODE_FORBIDS_ACTION`
- `DAILY_LOSS_LIMIT`
- `REQUIRES_MANUAL_APPROVAL`
- `UNKNOWN_STATE`

中文说明：上面的原因码只是初始集合。新增原因码需要登记、测试并映射到风控检查、告警和运营报告。
