# Daily Operations Report

中文说明：本模板由 `arb-ops` 使用结构化事实渲染，默认只读，不包含凭证明文、真实签名材料或账户变更动作。

- Report date: {{report_date}}
- Generated at: {{generated_at}}
- Read-only mode: {{read_only}}

## Summary

- Source events: {{source_event_count}}
- Risk decisions: {{risk_decision_count}}
- Rejected decisions: {{rejected_decision_count}}
- Execution reports: {{execution_report_count}}
- Ledger entries: {{ledger_entry_count}}
- Reconciliation runs: {{reconciliation_run_count}}
- Incidents: {{incident_count}}
- Highest reconciliation severity: {{highest_reconciliation_severity}}

## Risk Decisions

{{risk_decisions_by_kind}}

## Execution Reports

{{execution_reports_by_status}}

## Ledger Namespaces

{{ledger_entries_by_namespace}}

## Reconciliation

{{reconciliation_by_status}}

## Incident Status

{{incident_status_counts}}

## Incident Severity

{{incident_severity_counts}}

## Notes

{{notes}}
