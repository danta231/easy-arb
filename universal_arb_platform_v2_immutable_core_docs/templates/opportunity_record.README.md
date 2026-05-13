# Opportunity Record CSV / 机会记录 CSV

中文说明：`opportunity_record.csv` 是机器可读的 CSV（逗号分隔值）表头模板。字段名保持稳定英文标识，避免破坏 CSV 导入器和下游脚本；中文解释放在本文档中。

English note: `opportunity_record.csv` keeps stable English column names for importers and downstream scripts.

## 读取规则

- CSV 读取器应跳过以 `#` 开头的注释行。
- 字段值不得包含密钥、接口密钥、私钥、令牌、会话或完整账户标识。
- 未知外部状态不能写成成功；应写入明确的 `decision`（决策）和 `reason_codes`（原因码）。

## 字段说明

| Column / 字段 | 中文解释 |
|---|---|
| `timestamp` | 记录时间戳 |
| `strategy_id` | 策略 ID |
| `strategy_version` | 策略版本 |
| `transition_id` | 候选组合转换 ID |
| `holding_period` | 持仓周期 |
| `venue_ids` | 涉及的交易场所 ID 列表 |
| `instrument_ids` | 涉及的工具/合约 ID 列表 |
| `expected_profit_usd` | 预期美元收益 |
| `expected_profit_bps` | 预期收益基点 |
| `expected_apr` | 非即时策略的预期年化收益 |
| `required_capital_usd` | 所需美元资本 |
| `risk_flags` | 风险标记 |
| `decision` | 风控或审批决策 |
| `reason_codes` | 机器可读原因码 |
| `correlation_id` | 关联 ID，用于事件链路追踪 |
