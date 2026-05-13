# Daily Operations Report / 每日运营报告模板

中文说明：本模板用于从结构化事件、风控记录、执行报告、账本、对账结果和事故记录生成运营视图。它不能替代事实来源，也不能手工编造执行或账本事实。

## Metadata / 元数据

- Report date / 报告日期:
- Generated at / 生成时间:
- Architecture version / 架构版本:
- Execution mode / 执行模式:
- Config version / 配置版本:
- Risk policy version / 风控策略版本:
- Read-only mode / 只读模式:

## Summary / 摘要

- Venues enabled / 已启用场所:
- Strategies enabled / 已启用策略:
- Opportunities considered / 已评估机会:
- Opportunities rejected / 已拒绝机会:
- Risk decisions / 风控决策:
- Execution reports / 执行报告:
- Ledger entries / 账本分录:
- Reconciliation runs / 对账运行次数:
- Incidents / 事故数量:

## Venue Health / 场所健康

| Venue / 场所 | Status / 状态 | Latency / 延迟 | Disconnects / 断连 | Rate limit / 限频 | Notes / 备注 |
|---|---:|---:|---:|---:|---|

## Risk Decisions / 风控决策

| Strategy / 策略 | Approved / 批准 | Rejected / 拒绝 | Manual approval / 人工审批 | Top reject reason / 主要拒绝原因 |
|---|---:|---:|---:|---|

## Execution Reports / 执行报告

| Status / 状态 | Count / 数量 | Notes / 备注 |
|---|---:|---|

## Ledger Namespaces / 账本命名空间

| Namespace / 命名空间 | Entries / 分录数 | Notes / 备注 |
|---|---:|---|

## Reconciliation / 对账

| Account or custody / 账户或托管位置 | Status / 状态 | Mismatches / 差异 | Notes / 备注 |
|---|---:|---:|---|

## Incidents / 事故

| ID | Severity / 级别 | Status / 状态 | Summary / 摘要 | Action / 动作 |
|---|---:|---:|---|---|

## Notes / 备注

- 未知外部状态必须按风险处理，不能在报告中写成成功。
- 报告不得包含密钥、接口密钥、私钥、令牌或完整账户标识。
