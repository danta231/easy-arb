# Daily Operations Report / 每日运营报告

中文说明：本模板由 `arb-ops`（运营只读模块）使用结构化事实渲染，默认只读，不包含凭证明文、真实签名材料或账户变更动作。

英文标签保留用于 `fixture`（测试样例）和下游脚本稳定匹配；中文解释跟随展示。

- Report date: {{report_date}}（报告日期）
- Generated at: {{generated_at}}（生成时间）
- Read-only mode: {{read_only}}（只读模式）

## Summary / 摘要

- Source events: {{source_event_count}}（来源事件）
- Risk decisions: {{risk_decision_count}}（风控决策）
- Rejected decisions: {{rejected_decision_count}}（已拒绝决策）
- Execution reports: {{execution_report_count}}（执行报告）
- Ledger entries: {{ledger_entry_count}}（账本分录）
- Reconciliation runs: {{reconciliation_run_count}}（对账运行次数）
- Incidents: {{incident_count}}（事故数量）
- Highest reconciliation severity: {{highest_reconciliation_severity}}（最高对账严重等级）

## Risk Decisions / 风控决策

{{risk_decisions_by_kind}}

## Execution Reports / 执行报告

{{execution_reports_by_status}}

## Ledger Namespaces / 账本命名空间

{{ledger_entries_by_namespace}}

## Reconciliation / 对账

{{reconciliation_by_status}}

## Incident Status / 事故状态

{{incident_status_counts}}

## Incident Severity / 事故严重等级

{{incident_severity_counts}}

## Notes / 备注

{{notes}}
