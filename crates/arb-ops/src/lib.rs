//! `arb-ops` 运营只读工具。
//!
//! 中文说明：本 crate 只消费结构化事实，生成日报、风控拒绝报告、事故报告和
//! 只读查询结果。它不下单、不撤单、不签名、不转账、不修改账户，也不写账本历史。

#![forbid(unsafe_code)]

use arb_contracts::{
    ExecutionActionType, ExecutionMode, ExecutionPlan, ExecutionReport, Incident, IncidentAction,
    LedgerEntry, NormalizedEvent, RiskCheckResult, RiskCheckStatus, RiskDecision, RiskDecisionKind,
    RiskSeverity,
};
use arb_eventstore::{EventReader, EventStoreError, StoredEvent};
use arb_reconciliation::{ReconciliationReport, ReconciliationStatus};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

const DAILY_TEMPLATE: &str = include_str!("../../../templates/daily_operations_report.template.md");

/// 运营模块统一返回类型。
pub type OpsResult<T> = Result<T, OpsError>;

/// 运营模块错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpsError {
    EventStoreRead {
        message: String,
    },
    MissingReportField {
        report: &'static str,
        field: &'static str,
    },
    TemplateMissingPlaceholder {
        placeholder: &'static str,
    },
    TemplateUnrenderedPlaceholder,
    ManualApprovalMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for OpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventStoreRead { message } => write!(f, "event store read failed: {message}"),
            Self::MissingReportField { report, field } => {
                write!(f, "{report} is missing required field `{field}`")
            }
            Self::TemplateMissingPlaceholder { placeholder } => {
                write!(f, "daily report template is missing `{placeholder}`")
            }
            Self::TemplateUnrenderedPlaceholder => {
                f.write_str("daily report template still contains an unrendered placeholder")
            }
            Self::ManualApprovalMismatch {
                field,
                expected,
                actual,
            } => write!(
                f,
                "manual approval material field `{field}` mismatch: expected `{expected}`, got `{actual}`"
            ),
        }
    }
}

impl Error for OpsError {}

impl From<EventStoreError> for OpsError {
    fn from(error: EventStoreError) -> Self {
        Self::EventStoreRead {
            message: error.to_string(),
        }
    }
}

/// 运营报告输入事实。
///
/// 中文说明：该类型只持有已生成的事实副本，调用方需要先由事件存储、账本、
/// 风控和对账模块生成事实；运营模块不会回写任何上游模块。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OperationsFacts {
    pub events: Vec<StoredEvent>,
    pub risk_decisions: Vec<RiskDecision>,
    pub execution_plans: Vec<ExecutionPlan>,
    pub execution_reports: Vec<ExecutionReport>,
    pub ledger_entries: Vec<LedgerEntry>,
    pub reconciliation_reports: Vec<ReconciliationReport>,
    pub incidents: Vec<Incident>,
    pub manual_approval_records: Vec<ManualApprovalAuditRecord>,
}

impl OperationsFacts {
    /// 从事件只读接口加载事件事实。
    pub fn from_event_reader(reader: &impl EventReader) -> OpsResult<Self> {
        Ok(Self {
            events: reader.read_all_ordered()?,
            ..Self::default()
        })
    }

    pub fn with_risk_decisions(mut self, risk_decisions: Vec<RiskDecision>) -> Self {
        self.risk_decisions = risk_decisions;
        self
    }

    pub fn with_execution_reports(mut self, execution_reports: Vec<ExecutionReport>) -> Self {
        self.execution_reports = execution_reports;
        self
    }

    pub fn with_execution_plans(mut self, execution_plans: Vec<ExecutionPlan>) -> Self {
        self.execution_plans = execution_plans;
        self
    }

    pub fn with_ledger_entries(mut self, ledger_entries: Vec<LedgerEntry>) -> Self {
        self.ledger_entries = ledger_entries;
        self
    }

    pub fn with_reconciliation_reports(
        mut self,
        reconciliation_reports: Vec<ReconciliationReport>,
    ) -> Self {
        self.reconciliation_reports = reconciliation_reports;
        self
    }

    pub fn with_incidents(mut self, incidents: Vec<Incident>) -> Self {
        self.incidents = incidents;
        self
    }

    pub fn with_manual_approval_records(
        mut self,
        manual_approval_records: Vec<ManualApprovalAuditRecord>,
    ) -> Self {
        self.manual_approval_records = manual_approval_records;
        self
    }
}

/// 只读事实读取接口。
///
/// 中文说明：接口只暴露读取事实能力，不暴露写事件、写账本、下单、撤单、转账
/// 或签名方法。
pub trait OpsFactReader {
    fn read_facts(&self) -> OpsResult<OperationsFacts>;
}

/// 内存只读事实源。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMemoryOpsFactReader {
    facts: OperationsFacts,
}

impl InMemoryOpsFactReader {
    pub fn new(facts: OperationsFacts) -> Self {
        Self { facts }
    }
}

impl OpsFactReader for InMemoryOpsFactReader {
    fn read_facts(&self) -> OpsResult<OperationsFacts> {
        Ok(self.facts.clone())
    }
}

/// 只读运营命令。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpsReadOnlyCommand {
    DailyReport {
        report_date: String,
        generated_at: String,
    },
    RejectionReport {
        generated_at: String,
    },
    IncidentReport {
        generated_at: String,
    },
    Query(OpsQuery),
}

/// 只读运营命令输出。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpsCommandOutput {
    DailyReport(DailyOperationsReport),
    RejectionReport(RejectionReport),
    IncidentReport(IncidentReport),
    Query(OpsQueryResult),
}

/// 只读运营引擎。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadOnlyOpsEngine;

impl ReadOnlyOpsEngine {
    pub fn run(
        self,
        reader: &impl OpsFactReader,
        command: OpsReadOnlyCommand,
    ) -> OpsResult<OpsCommandOutput> {
        let facts = reader.read_facts()?;
        match command {
            OpsReadOnlyCommand::DailyReport {
                report_date,
                generated_at,
            } => Ok(OpsCommandOutput::DailyReport(generate_daily_report(
                &facts,
                report_date,
                generated_at,
            )?)),
            OpsReadOnlyCommand::RejectionReport { generated_at } => Ok(
                OpsCommandOutput::RejectionReport(generate_rejection_report(&facts, generated_at)?),
            ),
            OpsReadOnlyCommand::IncidentReport { generated_at } => Ok(
                OpsCommandOutput::IncidentReport(generate_incident_report(&facts, generated_at)?),
            ),
            OpsReadOnlyCommand::Query(query) => {
                Ok(OpsCommandOutput::Query(query_facts(&facts, query)?))
            }
        }
    }
}

/// 标签计数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CountByLabel {
    pub label: String,
    pub count: usize,
}

impl CountByLabel {
    fn new(label: impl Into<String>, count: usize) -> Self {
        Self {
            label: label.into(),
            count,
        }
    }
}

/// 日报。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DailyOperationsReport {
    pub report_date: String,
    pub generated_at: String,
    pub read_only: bool,
    pub source_event_count: usize,
    pub risk_decision_count: usize,
    pub rejected_decision_count: usize,
    pub execution_report_count: usize,
    pub ledger_entry_count: usize,
    pub reconciliation_run_count: usize,
    pub incident_count: usize,
    pub risk_decisions_by_kind: Vec<CountByLabel>,
    pub execution_reports_by_status: Vec<CountByLabel>,
    pub ledger_entries_by_namespace: Vec<CountByLabel>,
    pub reconciliation_by_status: Vec<CountByLabel>,
    pub incident_status_counts: Vec<CountByLabel>,
    pub incident_severity_counts: Vec<CountByLabel>,
    pub highest_reconciliation_severity: Option<String>,
    pub notes: Vec<String>,
}

impl DailyOperationsReport {
    pub fn validate_complete(&self) -> OpsResult<()> {
        require_non_empty("daily_operations_report", "report_date", &self.report_date)?;
        require_non_empty(
            "daily_operations_report",
            "generated_at",
            &self.generated_at,
        )?;
        if !self.read_only {
            return Err(OpsError::MissingReportField {
                report: "daily_operations_report",
                field: "read_only",
            });
        }
        Ok(())
    }

    pub fn render_markdown(&self) -> OpsResult<String> {
        self.validate_complete()?;
        let mut rendered = DAILY_TEMPLATE.to_owned();
        replace_placeholder(&mut rendered, "{{report_date}}", &self.report_date)?;
        replace_placeholder(&mut rendered, "{{generated_at}}", &self.generated_at)?;
        replace_placeholder(&mut rendered, "{{read_only}}", "true")?;
        replace_placeholder(
            &mut rendered,
            "{{source_event_count}}",
            &self.source_event_count.to_string(),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{risk_decision_count}}",
            &self.risk_decision_count.to_string(),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{rejected_decision_count}}",
            &self.rejected_decision_count.to_string(),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{execution_report_count}}",
            &self.execution_report_count.to_string(),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{ledger_entry_count}}",
            &self.ledger_entry_count.to_string(),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{reconciliation_run_count}}",
            &self.reconciliation_run_count.to_string(),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{incident_count}}",
            &self.incident_count.to_string(),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{risk_decisions_by_kind}}",
            &render_counts(&self.risk_decisions_by_kind),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{execution_reports_by_status}}",
            &render_counts(&self.execution_reports_by_status),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{ledger_entries_by_namespace}}",
            &render_counts(&self.ledger_entries_by_namespace),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{reconciliation_by_status}}",
            &render_counts(&self.reconciliation_by_status),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{incident_status_counts}}",
            &render_counts(&self.incident_status_counts),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{incident_severity_counts}}",
            &render_counts(&self.incident_severity_counts),
        )?;
        replace_placeholder(
            &mut rendered,
            "{{highest_reconciliation_severity}}",
            self.highest_reconciliation_severity
                .as_deref()
                .unwrap_or("none"),
        )?;
        replace_placeholder(&mut rendered, "{{notes}}", &render_notes(&self.notes))?;
        if rendered.contains("{{") {
            return Err(OpsError::TemplateUnrenderedPlaceholder);
        }
        Ok(rendered)
    }
}

/// 生成日报。
pub fn generate_daily_report(
    facts: &OperationsFacts,
    report_date: impl Into<String>,
    generated_at: impl Into<String>,
) -> OpsResult<DailyOperationsReport> {
    let mut notes = Vec::new();
    if facts.reconciliation_reports.iter().any(|report| {
        matches!(
            report.summary.status,
            ReconciliationStatus::Mismatch | ReconciliationStatus::IncidentRecommended
        )
    }) {
        notes.push(
            "Reconciliation has unresolved differences; review incident workflow.".to_owned(),
        );
    }
    if facts
        .execution_reports
        .iter()
        .any(|report| report.status.as_str() == "UnknownState")
    {
        notes.push(
            "Execution reports contain unknown state; unknown external state is risk-critical."
                .to_owned(),
        );
    }
    if facts.incidents.iter().any(|incident| {
        matches!(
            incident.status.as_str(),
            "Open" | "Mitigating" | "PostmortemRequired"
        )
    }) {
        notes.push("Open incidents require operator review.".to_owned());
    }

    let report = DailyOperationsReport {
        report_date: report_date.into(),
        generated_at: generated_at.into(),
        read_only: true,
        source_event_count: facts.events.len(),
        risk_decision_count: facts.risk_decisions.len(),
        rejected_decision_count: facts
            .risk_decisions
            .iter()
            .filter(|decision| is_rejection_like(decision))
            .count(),
        execution_report_count: facts.execution_reports.len(),
        ledger_entry_count: facts.ledger_entries.len(),
        reconciliation_run_count: facts.reconciliation_reports.len(),
        incident_count: facts.incidents.len(),
        risk_decisions_by_kind: counts_by(
            facts
                .risk_decisions
                .iter()
                .map(|decision| decision.decision.as_str()),
        ),
        execution_reports_by_status: counts_by(
            facts
                .execution_reports
                .iter()
                .map(|report| report.status.as_str()),
        ),
        ledger_entries_by_namespace: counts_by(
            facts
                .ledger_entries
                .iter()
                .map(|entry| entry.namespace.as_str()),
        ),
        reconciliation_by_status: counts_by(
            facts
                .reconciliation_reports
                .iter()
                .map(|report| report.summary.status.as_str()),
        ),
        incident_status_counts: counts_by(
            facts
                .incidents
                .iter()
                .map(|incident| incident.status.as_str()),
        ),
        incident_severity_counts: counts_by(
            facts
                .incidents
                .iter()
                .map(|incident| incident.severity.as_str()),
        ),
        highest_reconciliation_severity: highest_reconciliation_severity(facts),
        notes: notes
            .into_iter()
            .map(|note| redact_sensitive_text(&note))
            .collect(),
    };
    report.validate_complete()?;
    Ok(report)
}

/// 拒绝报告。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RejectionReport {
    pub generated_at: String,
    pub total_decision_count: usize,
    pub rejection_count: usize,
    pub reason_code_counts: Vec<CountByLabel>,
    pub records: Vec<RejectionRecord>,
}

impl RejectionReport {
    pub fn validate_complete(&self) -> OpsResult<()> {
        require_non_empty("rejection_report", "generated_at", &self.generated_at)?;
        if self.rejection_count != self.records.len() {
            return Err(OpsError::MissingReportField {
                report: "rejection_report",
                field: "records",
            });
        }
        for record in &self.records {
            record.validate_complete()?;
        }
        Ok(())
    }

    pub fn render_markdown(&self) -> OpsResult<String> {
        self.validate_complete()?;
        let mut out = String::new();
        out.push_str("# Risk Rejection Report\n\n");
        out.push_str("中文说明：本报告只展示风控拒绝或未批准事实，不生成执行计划。\n\n");
        out.push_str(&format!("- Generated at: {}\n", self.generated_at));
        out.push_str(&format!(
            "- Total decisions: {}\n",
            self.total_decision_count
        ));
        out.push_str(&format!("- Rejections: {}\n\n", self.rejection_count));
        out.push_str("## Reason Codes\n");
        out.push_str(&render_counts(&self.reason_code_counts));
        out.push_str("\n\n## Records\n");
        if self.records.is_empty() {
            out.push_str("- none\n");
        } else {
            for record in &self.records {
                out.push_str(&format!(
                    "- {} transition={} decision={} reasons={}\n",
                    record.decision_id,
                    record.transition_id,
                    record.decision,
                    record.reason_codes.join(",")
                ));
            }
        }
        Ok(out)
    }
}

/// 单条拒绝记录。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RejectionRecord {
    pub decision_id: String,
    pub transition_id: String,
    pub evaluated_at: String,
    pub decision: String,
    pub policy_version: String,
    pub policy_hash: String,
    pub policy_signature_ref: String,
    pub input_state_ref: String,
    pub reason_codes: Vec<String>,
    pub failed_checks: Vec<RiskCheckSummary>,
    pub detail: Option<String>,
}

impl RejectionRecord {
    fn validate_complete(&self) -> OpsResult<()> {
        require_non_empty("rejection_record", "decision_id", &self.decision_id)?;
        require_non_empty("rejection_record", "transition_id", &self.transition_id)?;
        require_non_empty("rejection_record", "evaluated_at", &self.evaluated_at)?;
        require_non_empty("rejection_record", "decision", &self.decision)?;
        require_non_empty("rejection_record", "policy_version", &self.policy_version)?;
        require_non_empty("rejection_record", "policy_hash", &self.policy_hash)?;
        require_non_empty(
            "rejection_record",
            "policy_signature_ref",
            &self.policy_signature_ref,
        )?;
        require_non_empty("rejection_record", "input_state_ref", &self.input_state_ref)?;
        if self.reason_codes.is_empty() {
            return Err(OpsError::MissingReportField {
                report: "rejection_record",
                field: "reason_codes",
            });
        }
        Ok(())
    }
}

/// 风控检查摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RiskCheckSummary {
    pub check_id: String,
    pub check_type: String,
    pub status: String,
    pub severity: String,
    pub reason_code: String,
    pub detail: Option<String>,
}

/// 人工审批材料报告。
///
/// 中文说明：该报告只汇总已存在的风控决策、执行计划预览和审批记录；报告本身
/// 不批准、不调度、不写账本。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualApprovalMaterialReport {
    pub generated_at: String,
    pub approval_summary: ApprovalSummary,
    pub risk_checks: Vec<RiskCheckSummary>,
    pub execution_plan_summary: ExecutionPlanSummary,
    pub approved_records: Vec<ManualApprovalAuditSummary>,
    pub rejected_records: Vec<ManualApprovalAuditSummary>,
    pub expired_records: Vec<ManualApprovalAuditSummary>,
    pub duplicate_records: Vec<ManualApprovalAuditSummary>,
}

impl ManualApprovalMaterialReport {
    pub fn validate_complete(&self) -> OpsResult<()> {
        require_non_empty(
            "manual_approval_material",
            "generated_at",
            &self.generated_at,
        )?;
        self.approval_summary.validate_complete()?;
        self.execution_plan_summary.validate_complete()?;
        if self.risk_checks.is_empty() {
            return Err(OpsError::MissingReportField {
                report: "manual_approval_material",
                field: "risk_checks",
            });
        }
        Ok(())
    }

    pub fn render_markdown(&self) -> OpsResult<String> {
        self.validate_complete()?;
        let mut out = String::new();
        out.push_str("# Manual Approval Material\n\n");
        out.push_str(
            "中文说明：审批只解除同一计划哈希的人工门禁，不能绕过风控、账本、对账、kill switch 或执行权限。\n\n",
        );
        out.push_str(&format!("- Generated at: {}\n", self.generated_at));
        out.push_str(&format!(
            "- Risk decision: {}\n",
            self.approval_summary.risk_decision_id
        ));
        out.push_str(&format!(
            "- Transition: {}\n",
            self.approval_summary.transition_id
        ));
        out.push_str(&format!("- Plan: {}\n", self.approval_summary.plan_id));
        out.push_str(&format!(
            "- Plan hash: {}\n",
            self.approval_summary.plan_hash
        ));
        out.push_str(&format!(
            "- Approval required: {}\n",
            self.approval_summary.approval_required
        ));
        out.push_str(&format!(
            "- Dispatchable before approval: {}\n\n",
            self.approval_summary.dispatchable_before_approval
        ));

        out.push_str("## Risk Checks\n");
        for check in &self.risk_checks {
            out.push_str(&format!(
                "- {} {} {} reason={}{}\n",
                check.check_id,
                check.status,
                check.severity,
                check.reason_code,
                check
                    .detail
                    .as_ref()
                    .map(|detail| format!(" detail={}", redact_sensitive_text(detail)))
                    .unwrap_or_default()
            ));
        }

        out.push_str("\n## Execution Plan\n");
        out.push_str(&format!(
            "- Mode: {}\n- Legs: {}\n- Manual gates: {}\n- Dependencies: {}\n",
            self.execution_plan_summary.execution_mode,
            self.execution_plan_summary.leg_count,
            self.execution_plan_summary.manual_gate_count,
            self.execution_plan_summary.dependency_count
        ));
        out.push_str(&render_counts(
            &self.execution_plan_summary.action_type_counts,
        ));
        out.push_str("\n\n## Approval Records\n");
        render_manual_approval_records("Approved", &self.approved_records, &mut out);
        render_manual_approval_records("Rejected", &self.rejected_records, &mut out);
        render_manual_approval_records("Expired", &self.expired_records, &mut out);
        render_manual_approval_records("Duplicate", &self.duplicate_records, &mut out);
        Ok(out)
    }
}

/// 审批摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalSummary {
    pub risk_decision_id: String,
    pub transition_id: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub decision: String,
    pub reason_codes: Vec<String>,
    pub approval_required: bool,
    pub dispatchable_before_approval: bool,
    pub approval_requirement: String,
}

impl ApprovalSummary {
    fn validate_complete(&self) -> OpsResult<()> {
        require_non_empty(
            "approval_summary",
            "risk_decision_id",
            &self.risk_decision_id,
        )?;
        require_non_empty("approval_summary", "transition_id", &self.transition_id)?;
        require_non_empty("approval_summary", "plan_id", &self.plan_id)?;
        require_non_empty("approval_summary", "plan_hash", &self.plan_hash)?;
        require_non_empty("approval_summary", "decision", &self.decision)?;
        require_non_empty(
            "approval_summary",
            "approval_requirement",
            &self.approval_requirement,
        )?;
        Ok(())
    }
}

/// 执行计划摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionPlanSummary {
    pub plan_id: String,
    pub execution_mode: String,
    pub approval_event_id: Option<String>,
    pub leg_count: usize,
    pub manual_gate_count: usize,
    pub ready_leg_count: usize,
    pub waiting_dependency_count: usize,
    pub dependency_count: usize,
    pub action_type_counts: Vec<CountByLabel>,
    pub timeout_policy: TimeoutPolicySummary,
    pub cancel_action: String,
    pub hedge_action: String,
    pub partial_fill_action: String,
    pub unknown_state_action: String,
    pub retry_limit: u64,
    pub controlled_flow_note: String,
}

impl ExecutionPlanSummary {
    fn validate_complete(&self) -> OpsResult<()> {
        require_non_empty("execution_plan_summary", "plan_id", &self.plan_id)?;
        require_non_empty(
            "execution_plan_summary",
            "execution_mode",
            &self.execution_mode,
        )?;
        require_non_empty(
            "execution_plan_summary",
            "controlled_flow_note",
            &self.controlled_flow_note,
        )?;
        if self.leg_count == 0 {
            return Err(OpsError::MissingReportField {
                report: "execution_plan_summary",
                field: "leg_count",
            });
        }
        Ok(())
    }
}

/// 超时策略摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeoutPolicySummary {
    pub plan_timeout_ms: u64,
    pub leg_timeout_ms: u64,
    pub unknown_state_after_ms: Option<u64>,
}

/// 人工审批审计记录输入。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualApprovalAuditRecord {
    pub record_id: String,
    pub approval_event_id: String,
    pub risk_decision_id: String,
    pub transition_id: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub decision: String,
    pub status: String,
    pub reviewer_id: String,
    pub decided_at: String,
    pub expires_at: String,
    pub reason: Option<String>,
    pub duplicate_of: Option<String>,
    pub releases_manual_gate: bool,
    pub controlled_next_step: String,
}

/// 人工审批审计记录摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualApprovalAuditSummary {
    pub record_id: String,
    pub approval_event_id: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub decision: String,
    pub status: String,
    pub reviewer_id: String,
    pub decided_at: String,
    pub expires_at: String,
    pub reason: Option<String>,
    pub duplicate_of: Option<String>,
    pub releases_manual_gate: bool,
    pub controlled_next_step: String,
}

/// 生成人工审批材料。
pub fn generate_manual_approval_material(
    risk_decision: &RiskDecision,
    plan: &ExecutionPlan,
    plan_hash: impl Into<String>,
    approval_records: &[ManualApprovalAuditRecord],
    generated_at: impl Into<String>,
) -> OpsResult<ManualApprovalMaterialReport> {
    validate_manual_approval_links(risk_decision, plan)?;
    let plan_hash = plan_hash.into();
    require_non_empty("manual_approval_material", "plan_hash", &plan_hash)?;

    let relevant_records = approval_records
        .iter()
        .filter(|record| record.plan_id == plan.plan_id.as_str() && record.plan_hash == plan_hash)
        .map(manual_approval_record_summary)
        .collect::<Vec<_>>();

    let report = ManualApprovalMaterialReport {
        generated_at: generated_at.into(),
        approval_summary: approval_summary(risk_decision, plan, &plan_hash),
        risk_checks: risk_decision
            .checks
            .iter()
            .map(risk_check_summary)
            .collect(),
        execution_plan_summary: execution_plan_summary(plan),
        approved_records: filter_manual_records(&relevant_records, "Approved"),
        rejected_records: filter_manual_records(&relevant_records, "Rejected"),
        expired_records: filter_manual_records(&relevant_records, "Expired"),
        duplicate_records: filter_manual_records(&relevant_records, "Duplicate"),
    };
    report.validate_complete()?;
    Ok(report)
}

/// 生成拒绝报告。
pub fn generate_rejection_report(
    facts: &OperationsFacts,
    generated_at: impl Into<String>,
) -> OpsResult<RejectionReport> {
    let records = facts
        .risk_decisions
        .iter()
        .filter(|decision| is_rejection_like(decision))
        .map(rejection_record)
        .collect::<Vec<_>>();
    let reason_code_counts = counts_by(
        records
            .iter()
            .flat_map(|record| record.reason_codes.iter().map(String::as_str)),
    );
    let report = RejectionReport {
        generated_at: generated_at.into(),
        total_decision_count: facts.risk_decisions.len(),
        rejection_count: records.len(),
        reason_code_counts,
        records,
    };
    report.validate_complete()?;
    Ok(report)
}

/// 事故报告。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncidentReport {
    pub generated_at: String,
    pub incident_count: usize,
    pub open_incident_count: usize,
    pub status_counts: Vec<CountByLabel>,
    pub severity_counts: Vec<CountByLabel>,
    pub records: Vec<IncidentRecord>,
    pub reconciliation_suggestions: Vec<IncidentSuggestionRecord>,
}

impl IncidentReport {
    pub fn validate_complete(&self) -> OpsResult<()> {
        require_non_empty("incident_report", "generated_at", &self.generated_at)?;
        if self.incident_count != self.records.len() {
            return Err(OpsError::MissingReportField {
                report: "incident_report",
                field: "records",
            });
        }
        for record in &self.records {
            record.validate_complete()?;
        }
        for suggestion in &self.reconciliation_suggestions {
            suggestion.validate_complete()?;
        }
        Ok(())
    }

    pub fn render_markdown(&self) -> OpsResult<String> {
        self.validate_complete()?;
        let mut out = String::new();
        out.push_str("# Incident Report\n\n");
        out.push_str("中文说明：本报告只展示已记录事故和对账事故建议，不执行自动补偿。\n\n");
        out.push_str(&format!("- Generated at: {}\n", self.generated_at));
        out.push_str(&format!("- Incidents: {}\n", self.incident_count));
        out.push_str(&format!(
            "- Open incidents: {}\n\n",
            self.open_incident_count
        ));
        out.push_str("## Severity\n");
        out.push_str(&render_counts(&self.severity_counts));
        out.push_str("\n\n## Records\n");
        if self.records.is_empty() {
            out.push_str("- none\n");
        } else {
            for record in &self.records {
                out.push_str(&format!(
                    "- {} severity={} status={} trigger={}\n",
                    record.incident_id, record.severity, record.status, record.trigger
                ));
            }
        }
        Ok(out)
    }
}

/// 单条事故记录。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncidentRecord {
    pub incident_id: String,
    pub severity: String,
    pub status: String,
    pub opened_at: String,
    pub closed_at: Option<String>,
    pub trigger: String,
    pub source_event_refs: Vec<String>,
    pub venue_ids: Vec<String>,
    pub strategy_ids: Vec<String>,
    pub capital_at_risk_usd: Option<String>,
    pub automatic_actions: Vec<IncidentActionSummary>,
    pub manual_actions: Vec<IncidentActionSummary>,
    pub root_cause: Option<String>,
    pub corrective_action: Option<String>,
    pub prevention_action: Option<String>,
}

impl IncidentRecord {
    fn validate_complete(&self) -> OpsResult<()> {
        require_non_empty("incident_record", "incident_id", &self.incident_id)?;
        require_non_empty("incident_record", "severity", &self.severity)?;
        require_non_empty("incident_record", "status", &self.status)?;
        require_non_empty("incident_record", "opened_at", &self.opened_at)?;
        require_non_empty("incident_record", "trigger", &self.trigger)?;
        if self.source_event_refs.is_empty() {
            return Err(OpsError::MissingReportField {
                report: "incident_record",
                field: "source_event_refs",
            });
        }
        Ok(())
    }
}

/// 事故动作摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncidentActionSummary {
    pub action_id: String,
    pub action_type: String,
    pub timestamp: String,
    pub detail: Option<String>,
}

/// 对账事故建议摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncidentSuggestionRecord {
    pub suggestion_id: String,
    pub severity: String,
    pub status: String,
    pub opened_at: String,
    pub trigger_reason_code: String,
    pub source_difference_ids: Vec<String>,
    pub venue_ids: Vec<String>,
    pub strategy_ids: Vec<String>,
    pub account_ids: Vec<String>,
    pub asset_ids: Vec<String>,
    pub ledger_entry_ids: Vec<String>,
    pub suggested_actions: Vec<String>,
}

impl IncidentSuggestionRecord {
    fn validate_complete(&self) -> OpsResult<()> {
        require_non_empty(
            "incident_suggestion_record",
            "suggestion_id",
            &self.suggestion_id,
        )?;
        require_non_empty("incident_suggestion_record", "severity", &self.severity)?;
        require_non_empty("incident_suggestion_record", "status", &self.status)?;
        require_non_empty("incident_suggestion_record", "opened_at", &self.opened_at)?;
        require_non_empty(
            "incident_suggestion_record",
            "trigger_reason_code",
            &self.trigger_reason_code,
        )?;
        Ok(())
    }
}

/// 生成事故报告。
pub fn generate_incident_report(
    facts: &OperationsFacts,
    generated_at: impl Into<String>,
) -> OpsResult<IncidentReport> {
    let records = facts
        .incidents
        .iter()
        .map(incident_record)
        .collect::<Vec<_>>();
    let reconciliation_suggestions = facts
        .reconciliation_reports
        .iter()
        .flat_map(|report| report.incident_suggestions.iter())
        .map(|suggestion| IncidentSuggestionRecord {
            suggestion_id: suggestion.suggestion_id.as_str().to_owned(),
            severity: suggestion.severity.as_str().to_owned(),
            status: suggestion.status.as_str().to_owned(),
            opened_at: suggestion.opened_at.to_string(),
            trigger_reason_code: redact_sensitive_text(&suggestion.trigger_reason_code),
            source_difference_ids: suggestion
                .source_difference_ids
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            venue_ids: suggestion
                .impacted
                .venue_ids
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            strategy_ids: suggestion
                .impacted
                .strategy_ids
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            account_ids: suggestion
                .impacted
                .account_ids
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            asset_ids: suggestion
                .impacted
                .asset_ids
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            ledger_entry_ids: suggestion
                .impacted
                .ledger_entry_ids
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            suggested_actions: suggestion
                .suggested_actions
                .iter()
                .map(|action| action.as_str().to_owned())
                .collect(),
        })
        .collect::<Vec<_>>();
    let report = IncidentReport {
        generated_at: generated_at.into(),
        incident_count: records.len(),
        open_incident_count: records
            .iter()
            .filter(|record| record.status != "Closed" && record.status != "Resolved")
            .count(),
        status_counts: counts_by(records.iter().map(|record| record.status.as_str())),
        severity_counts: counts_by(records.iter().map(|record| record.severity.as_str())),
        records,
        reconciliation_suggestions,
    };
    report.validate_complete()?;
    Ok(report)
}

/// 只读查询。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpsQuery {
    EventId(String),
    CorrelationId(String),
    StrategyId(String),
    VenueId(String),
    AccountId(String),
    RiskDecisionId(String),
    LedgerEntryId(String),
    IncidentId(String),
}

/// 查询结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpsQueryResult {
    pub query: OpsQuery,
    pub read_only: bool,
    pub scanned: QueryScanSummary,
    pub matches: Vec<QueryMatch>,
}

/// 查询扫描摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryScanSummary {
    pub event_count: usize,
    pub risk_decision_count: usize,
    pub execution_report_count: usize,
    pub ledger_entry_count: usize,
    pub reconciliation_run_count: usize,
    pub incident_count: usize,
}

/// 查询命中。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryMatch {
    pub fact_type: String,
    pub fact_id: String,
    pub timestamp: Option<String>,
    pub summary: String,
    pub event_refs: Vec<String>,
    pub ledger_entry_refs: Vec<String>,
}

/// 执行只读查询。
pub fn query_facts(facts: &OperationsFacts, query: OpsQuery) -> OpsResult<OpsQueryResult> {
    let mut matches = Vec::new();
    query_events(facts, &query, &mut matches);
    query_risk_decisions(facts, &query, &mut matches);
    query_execution_reports(facts, &query, &mut matches);
    query_ledger_entries(facts, &query, &mut matches);
    query_reconciliation_reports(facts, &query, &mut matches);
    query_incidents(facts, &query, &mut matches);

    Ok(OpsQueryResult {
        query,
        read_only: true,
        scanned: QueryScanSummary {
            event_count: facts.events.len(),
            risk_decision_count: facts.risk_decisions.len(),
            execution_report_count: facts.execution_reports.len(),
            ledger_entry_count: facts.ledger_entries.len(),
            reconciliation_run_count: facts.reconciliation_reports.len(),
            incident_count: facts.incidents.len(),
        },
        matches,
    })
}

/// 敏感文本脱敏。
///
/// 中文说明：报告渲染前统一处理自由文本，避免 API key、secret、私钥、token、
/// credential 等敏感材料进入运营输出。
pub fn redact_sensitive_text(input: &str) -> String {
    let mut redacted = input.to_owned();
    for marker in SENSITIVE_MARKERS {
        redacted = redact_marker_values(&redacted, marker);
    }
    redact_bearer_values(&redacted)
}

const SENSITIVE_MARKERS: &[&str] = &[
    "api_key",
    "apikey",
    "api-secret",
    "api_secret",
    "secret",
    "private_key",
    "private-key",
    "token",
    "credential",
    "password",
    "mnemonic",
    "seed_phrase",
    "session_token",
    "webhook_token",
];

fn require_non_empty(report: &'static str, field: &'static str, value: &str) -> OpsResult<()> {
    if value.is_empty() {
        Err(OpsError::MissingReportField { report, field })
    } else {
        Ok(())
    }
}

fn counts_by<'a>(values: impl Iterator<Item = &'a str>) -> Vec<CountByLabel> {
    let mut counts = BTreeMap::<String, usize>::new();
    for value in values {
        *counts.entry(value.to_owned()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(label, count)| CountByLabel::new(label, count))
        .collect()
}

fn highest_reconciliation_severity(facts: &OperationsFacts) -> Option<String> {
    facts
        .reconciliation_reports
        .iter()
        .filter_map(|report| report.summary.highest_severity)
        .max()
        .map(|severity| severity.as_str().to_owned())
}

fn replace_placeholder(
    rendered: &mut String,
    placeholder: &'static str,
    value: &str,
) -> OpsResult<()> {
    if !rendered.contains(placeholder) {
        return Err(OpsError::TemplateMissingPlaceholder { placeholder });
    }
    *rendered = rendered.replace(placeholder, value);
    Ok(())
}

fn render_counts(counts: &[CountByLabel]) -> String {
    if counts.is_empty() {
        return "- none".to_owned();
    }
    counts
        .iter()
        .map(|item| format!("- {}: {}", redact_sensitive_text(&item.label), item.count))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_notes(notes: &[String]) -> String {
    if notes.is_empty() {
        "- none".to_owned()
    } else {
        notes
            .iter()
            .map(|note| format!("- {}", redact_sensitive_text(note)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn is_rejection_like(decision: &RiskDecision) -> bool {
    matches!(
        decision.decision,
        RiskDecisionKind::Rejected
            | RiskDecisionKind::RequiresMoreData
            | RiskDecisionKind::SuspendedByCircuitBreaker
    )
}

fn rejection_record(decision: &RiskDecision) -> RejectionRecord {
    RejectionRecord {
        decision_id: decision.decision_id.as_str().to_owned(),
        transition_id: decision.transition_id.as_str().to_owned(),
        evaluated_at: decision.evaluated_at.as_str().to_owned(),
        decision: decision.decision.as_str().to_owned(),
        policy_version: decision.policy_version.as_str().to_owned(),
        policy_hash: redact_sensitive_text(decision.policy_hash.as_str()),
        policy_signature_ref: redact_sensitive_text(decision.policy_signature_ref.as_str()),
        input_state_ref: decision.input_state_ref.as_str().to_owned(),
        reason_codes: decision
            .reason_codes
            .iter()
            .map(|code| redact_sensitive_text(code.as_str()))
            .collect(),
        failed_checks: decision
            .checks
            .iter()
            .filter(|check| is_failed_or_blocking_check(check))
            .map(risk_check_summary)
            .collect(),
        detail: decision.detail.as_deref().map(redact_sensitive_text),
    }
}

fn is_failed_or_blocking_check(check: &RiskCheckResult) -> bool {
    !matches!(
        check.status,
        RiskCheckStatus::Pass | RiskCheckStatus::NotApplicable
    ) || matches!(check.severity, RiskSeverity::Block | RiskSeverity::Critical)
}

fn risk_check_summary(check: &RiskCheckResult) -> RiskCheckSummary {
    RiskCheckSummary {
        check_id: check.check_id.as_str().to_owned(),
        check_type: check.check_type.as_str().to_owned(),
        status: check.status.as_str().to_owned(),
        severity: check.severity.as_str().to_owned(),
        reason_code: redact_sensitive_text(check.reason_code.as_str()),
        detail: check.detail.as_deref().map(redact_sensitive_text),
    }
}

fn validate_manual_approval_links(
    risk_decision: &RiskDecision,
    plan: &ExecutionPlan,
) -> OpsResult<()> {
    if risk_decision.decision_id.as_str() != plan.risk_decision_id.as_str() {
        return Err(OpsError::ManualApprovalMismatch {
            field: "risk_decision_id",
            expected: risk_decision.decision_id.as_str().to_owned(),
            actual: plan.risk_decision_id.as_str().to_owned(),
        });
    }
    if risk_decision.transition_id.as_str() != plan.transition_id.as_str() {
        return Err(OpsError::ManualApprovalMismatch {
            field: "transition_id",
            expected: risk_decision.transition_id.as_str().to_owned(),
            actual: plan.transition_id.as_str().to_owned(),
        });
    }
    Ok(())
}

fn approval_summary(
    risk_decision: &RiskDecision,
    plan: &ExecutionPlan,
    plan_hash: &str,
) -> ApprovalSummary {
    let manual_gate_count = manual_gate_count(plan);
    let approval_required = risk_decision.decision == RiskDecisionKind::RequiresManualApproval
        || plan.execution_mode == ExecutionMode::ManualApproval
        || manual_gate_count > 0;
    ApprovalSummary {
        risk_decision_id: risk_decision.decision_id.as_str().to_owned(),
        transition_id: risk_decision.transition_id.as_str().to_owned(),
        plan_id: plan.plan_id.as_str().to_owned(),
        plan_hash: redact_sensitive_text(plan_hash),
        decision: risk_decision.decision.as_str().to_owned(),
        reason_codes: risk_decision
            .reason_codes
            .iter()
            .map(|code| redact_sensitive_text(code.as_str()))
            .collect(),
        approval_required,
        dispatchable_before_approval: !approval_required,
        approval_requirement: "人工审批必须引用同一个 plan_hash；审批通过也只能进入受控流程。"
            .to_owned(),
    }
}

fn execution_plan_summary(plan: &ExecutionPlan) -> ExecutionPlanSummary {
    ExecutionPlanSummary {
        plan_id: plan.plan_id.as_str().to_owned(),
        execution_mode: plan.execution_mode.as_str().to_owned(),
        approval_event_id: plan
            .approval_event_id
            .as_ref()
            .map(|event_id| event_id.as_str().to_owned()),
        leg_count: plan.legs.len(),
        manual_gate_count: manual_gate_count(plan),
        ready_leg_count: plan
            .legs
            .iter()
            .filter(|leg| leg.state.as_str() == "Ready")
            .count(),
        waiting_dependency_count: plan
            .legs
            .iter()
            .filter(|leg| leg.state.as_str() == "WaitingDependency")
            .count(),
        dependency_count: plan.dependency_graph.edges.len(),
        action_type_counts: counts_by(plan.legs.iter().map(|leg| leg.action_type.as_str())),
        timeout_policy: TimeoutPolicySummary {
            plan_timeout_ms: plan.timeout_policy.plan_timeout_ms.as_u64(),
            leg_timeout_ms: plan.timeout_policy.leg_timeout_ms.as_u64(),
            unknown_state_after_ms: plan
                .timeout_policy
                .unknown_state_after_ms
                .map(|duration| duration.as_u64()),
        },
        cancel_action: plan.cancel_policy.default_action.as_str().to_owned(),
        hedge_action: plan
            .hedge_policy
            .residual_exposure_action
            .as_str()
            .to_owned(),
        partial_fill_action: plan.partial_fill_policy.action.as_str().to_owned(),
        unknown_state_action: plan.failure_policy.unknown_state_action.as_str().to_owned(),
        retry_limit: plan.failure_policy.retry_limit,
        controlled_flow_note:
            "Approval releases only the manual gate; ledger, reconciliation, kill switch, and execution permissions remain mandatory."
                .to_owned(),
    }
}

fn manual_gate_count(plan: &ExecutionPlan) -> usize {
    plan.legs
        .iter()
        .filter(|leg| leg.action_type == ExecutionActionType::ManualApprovalGate)
        .count()
}

fn manual_approval_record_summary(
    record: &ManualApprovalAuditRecord,
) -> ManualApprovalAuditSummary {
    ManualApprovalAuditSummary {
        record_id: redact_sensitive_text(&record.record_id),
        approval_event_id: record.approval_event_id.clone(),
        plan_id: record.plan_id.clone(),
        plan_hash: redact_sensitive_text(&record.plan_hash),
        decision: redact_sensitive_text(&record.decision),
        status: redact_sensitive_text(&record.status),
        reviewer_id: redact_sensitive_text(&record.reviewer_id),
        decided_at: record.decided_at.clone(),
        expires_at: record.expires_at.clone(),
        reason: record.reason.as_deref().map(redact_sensitive_text),
        duplicate_of: record
            .duplicate_of
            .as_ref()
            .map(|id| redact_sensitive_text(id)),
        releases_manual_gate: record.releases_manual_gate,
        controlled_next_step: redact_sensitive_text(&record.controlled_next_step),
    }
}

fn filter_manual_records(
    records: &[ManualApprovalAuditSummary],
    status: &str,
) -> Vec<ManualApprovalAuditSummary> {
    records
        .iter()
        .filter(|record| record.status == status)
        .cloned()
        .collect()
}

fn render_manual_approval_records(
    label: &str,
    records: &[ManualApprovalAuditSummary],
    out: &mut String,
) {
    out.push_str(&format!("\n### {label}\n"));
    if records.is_empty() {
        out.push_str("- none\n");
        return;
    }
    for record in records {
        out.push_str(&format!(
            "- {} event={} reviewer={} releases_gate={}{}\n",
            record.record_id,
            record.approval_event_id,
            record.reviewer_id,
            record.releases_manual_gate,
            record
                .reason
                .as_ref()
                .map(|reason| format!(" reason={reason}"))
                .unwrap_or_default()
        ));
    }
}

fn incident_record(incident: &Incident) -> IncidentRecord {
    IncidentRecord {
        incident_id: incident.incident_id.as_str().to_owned(),
        severity: incident.severity.as_str().to_owned(),
        status: incident.status.as_str().to_owned(),
        opened_at: incident.opened_at.as_str().to_owned(),
        closed_at: incident
            .closed_at
            .as_ref()
            .map(|timestamp| timestamp.as_str().to_owned()),
        trigger: redact_sensitive_text(incident.trigger.as_str()),
        source_event_refs: incident
            .source_event_refs
            .iter()
            .map(|event_id| event_id.as_str().to_owned())
            .collect(),
        venue_ids: incident
            .impacted
            .venue_ids
            .as_ref()
            .map(|ids| ids.iter().map(|id| id.as_str().to_owned()).collect())
            .unwrap_or_default(),
        strategy_ids: incident
            .impacted
            .strategy_ids
            .as_ref()
            .map(|ids| ids.iter().map(|id| id.as_str().to_owned()).collect())
            .unwrap_or_default(),
        capital_at_risk_usd: incident
            .impacted
            .capital_at_risk_usd
            .as_ref()
            .map(|value| value.as_str().to_owned()),
        automatic_actions: incident
            .automatic_actions
            .iter()
            .map(incident_action_summary)
            .collect(),
        manual_actions: incident
            .manual_actions
            .iter()
            .map(incident_action_summary)
            .collect(),
        root_cause: incident.root_cause.as_deref().map(redact_sensitive_text),
        corrective_action: incident
            .corrective_action
            .as_deref()
            .map(redact_sensitive_text),
        prevention_action: incident
            .prevention_action
            .as_deref()
            .map(redact_sensitive_text),
    }
}

fn incident_action_summary(action: &IncidentAction) -> IncidentActionSummary {
    IncidentActionSummary {
        action_id: action.action_id.as_str().to_owned(),
        action_type: action.action_type.as_str().to_owned(),
        timestamp: action.timestamp.as_str().to_owned(),
        detail: action.detail.as_deref().map(redact_sensitive_text),
    }
}

fn query_events(facts: &OperationsFacts, query: &OpsQuery, matches: &mut Vec<QueryMatch>) {
    for stored in &facts.events {
        let event = &stored.event;
        if event_matches(event, query) {
            matches.push(QueryMatch {
                fact_type: "event".to_owned(),
                fact_id: event.event_id.as_str().to_owned(),
                timestamp: Some(event.timestamp_event.as_str().to_owned()),
                summary: redact_sensitive_text(&format!(
                    "{} source={} sequence={}",
                    event.event_type.as_str(),
                    event.source,
                    stored.sequence
                )),
                event_refs: vec![event.event_id.as_str().to_owned()],
                ledger_entry_refs: Vec::new(),
            });
        }
    }
}

fn event_matches(event: &NormalizedEvent, query: &OpsQuery) -> bool {
    match query {
        OpsQuery::EventId(target) => event.event_id.as_str() == target,
        OpsQuery::CorrelationId(target) => event.correlation_id.as_str() == target,
        OpsQuery::StrategyId(target) => nested_identifier_matches(&event.strategy_id, target),
        OpsQuery::VenueId(target) => nested_identifier_matches(&event.venue_id, target),
        _ => false,
    }
}

fn query_risk_decisions(facts: &OperationsFacts, query: &OpsQuery, matches: &mut Vec<QueryMatch>) {
    for decision in &facts.risk_decisions {
        if match query {
            OpsQuery::RiskDecisionId(target) => decision.decision_id.as_str() == target,
            _ => false,
        } {
            matches.push(QueryMatch {
                fact_type: "risk_decision".to_owned(),
                fact_id: decision.decision_id.as_str().to_owned(),
                timestamp: Some(decision.evaluated_at.as_str().to_owned()),
                summary: redact_sensitive_text(&format!(
                    "{} transition={} reasons={}",
                    decision.decision.as_str(),
                    decision.transition_id.as_str(),
                    decision
                        .reason_codes
                        .iter()
                        .map(|code| code.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                )),
                event_refs: Vec::new(),
                ledger_entry_refs: Vec::new(),
            });
        }
    }
}

fn query_execution_reports(
    facts: &OperationsFacts,
    query: &OpsQuery,
    matches: &mut Vec<QueryMatch>,
) {
    for report in &facts.execution_reports {
        if match query {
            OpsQuery::VenueId(target) => report
                .fills
                .iter()
                .any(|fill| fill.venue_id.as_str() == target),
            _ => false,
        } {
            matches.push(QueryMatch {
                fact_type: "execution_report".to_owned(),
                fact_id: report.report_id.as_str().to_owned(),
                timestamp: Some(report.generated_at.as_str().to_owned()),
                summary: redact_sensitive_text(&format!(
                    "{} plan={} fills={} failures={}",
                    report.status.as_str(),
                    report.plan_id.as_str(),
                    report.fills.len(),
                    report.failures.len()
                )),
                event_refs: report
                    .leg_reports
                    .iter()
                    .flat_map(|leg| leg.source_event_refs.as_ref().into_iter().flatten())
                    .map(|event_id| event_id.as_str().to_owned())
                    .collect(),
                ledger_entry_refs: report
                    .fills
                    .iter()
                    .filter_map(|fill| fill.ledger_entry_id.as_ref())
                    .map(|id| id.as_str().to_owned())
                    .collect(),
            });
        }
    }
}

fn query_ledger_entries(facts: &OperationsFacts, query: &OpsQuery, matches: &mut Vec<QueryMatch>) {
    for entry in &facts.ledger_entries {
        if ledger_entry_matches(entry, query) {
            matches.push(QueryMatch {
                fact_type: "ledger_entry".to_owned(),
                fact_id: entry.ledger_entry_id.as_str().to_owned(),
                timestamp: Some(entry.timestamp.as_str().to_owned()),
                summary: redact_sensitive_text(&format!(
                    "{} namespace={} source_event={}",
                    entry.entry_type.as_str(),
                    entry.namespace.as_str(),
                    entry.source_event_id.as_str()
                )),
                event_refs: vec![entry.source_event_id.as_str().to_owned()],
                ledger_entry_refs: vec![entry.ledger_entry_id.as_str().to_owned()],
            });
        }
    }
}

fn ledger_entry_matches(entry: &LedgerEntry, query: &OpsQuery) -> bool {
    match query {
        OpsQuery::LedgerEntryId(target) => entry.ledger_entry_id.as_str() == target,
        OpsQuery::StrategyId(target) => entry
            .strategy_id
            .as_ref()
            .is_some_and(|strategy_id| strategy_id.as_str() == target),
        OpsQuery::AccountId(target) => entry
            .legs
            .iter()
            .any(|leg| leg.account_id.as_str() == target),
        OpsQuery::EventId(target) => entry.source_event_id.as_str() == target,
        _ => false,
    }
}

fn query_reconciliation_reports(
    facts: &OperationsFacts,
    query: &OpsQuery,
    matches: &mut Vec<QueryMatch>,
) {
    for report in &facts.reconciliation_reports {
        let difference_matches = report.differences.iter().any(|difference| match query {
            OpsQuery::EventId(target) => difference
                .source_event_ids
                .iter()
                .any(|event_id| event_id.as_str() == target),
            OpsQuery::LedgerEntryId(target) => difference
                .ledger_entry_ids
                .iter()
                .any(|ledger_entry_id| ledger_entry_id.as_str() == target),
            OpsQuery::StrategyId(target) => difference
                .scope
                .strategy_id
                .as_ref()
                .is_some_and(|strategy_id| strategy_id.as_str() == target),
            OpsQuery::VenueId(target) => difference
                .scope
                .venue_id
                .as_ref()
                .is_some_and(|venue_id| venue_id.as_str() == target),
            OpsQuery::AccountId(target) => difference
                .scope
                .account_id
                .as_ref()
                .is_some_and(|account_id| account_id.as_str() == target),
            _ => false,
        });
        if difference_matches {
            matches.push(QueryMatch {
                fact_type: "reconciliation_report".to_owned(),
                fact_id: report.run_id.as_str().to_owned(),
                timestamp: Some(report.as_of.to_string()),
                summary: redact_sensitive_text(&format!(
                    "{} differences={} incident_suggestions={}",
                    report.summary.status.as_str(),
                    report.summary.difference_count,
                    report.summary.incident_suggestion_count
                )),
                event_refs: report
                    .differences
                    .iter()
                    .flat_map(|difference| difference.source_event_ids.iter())
                    .map(|event_id| event_id.as_str().to_owned())
                    .collect(),
                ledger_entry_refs: report
                    .differences
                    .iter()
                    .flat_map(|difference| difference.ledger_entry_ids.iter())
                    .map(|ledger_entry_id| ledger_entry_id.as_str().to_owned())
                    .collect(),
            });
        }
    }
}

fn query_incidents(facts: &OperationsFacts, query: &OpsQuery, matches: &mut Vec<QueryMatch>) {
    for incident in &facts.incidents {
        if incident_matches(incident, query) {
            matches.push(QueryMatch {
                fact_type: "incident".to_owned(),
                fact_id: incident.incident_id.as_str().to_owned(),
                timestamp: Some(incident.opened_at.as_str().to_owned()),
                summary: redact_sensitive_text(&format!(
                    "{} status={} trigger={}",
                    incident.severity.as_str(),
                    incident.status.as_str(),
                    incident.trigger.as_str()
                )),
                event_refs: incident
                    .source_event_refs
                    .iter()
                    .map(|event_id| event_id.as_str().to_owned())
                    .collect(),
                ledger_entry_refs: Vec::new(),
            });
        }
    }
}

fn incident_matches(incident: &Incident, query: &OpsQuery) -> bool {
    match query {
        OpsQuery::IncidentId(target) => incident.incident_id.as_str() == target,
        OpsQuery::EventId(target) => incident
            .source_event_refs
            .iter()
            .any(|event_id| event_id.as_str() == target),
        OpsQuery::VenueId(target) => incident
            .impacted
            .venue_ids
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == target)),
        OpsQuery::StrategyId(target) => incident
            .impacted
            .strategy_ids
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == target)),
        _ => false,
    }
}

fn nested_identifier_matches(
    value: &Option<Option<arb_contracts::Identifier>>,
    target: &str,
) -> bool {
    value
        .as_ref()
        .and_then(|inner| inner.as_ref())
        .is_some_and(|id| id.as_str() == target)
}

fn redact_marker_values(input: &str, marker: &str) -> String {
    input
        .split_inclusive(char::is_whitespace)
        .map(|token| redact_marker_token(token, marker))
        .collect::<String>()
}

fn redact_marker_token(token: &str, marker: &str) -> String {
    let lower = token.to_ascii_lowercase();
    let Some(marker_index) = lower.find(marker) else {
        return token.to_owned();
    };
    let after_marker = marker_index + marker.len();
    let Some(delimiter_offset) = lower[after_marker..].find(['=', ':']) else {
        return token.to_owned();
    };
    let value_start = after_marker + delimiter_offset + 1;
    let value_end = token[value_start..]
        .char_indices()
        .find_map(|(offset, ch)| matches!(ch, ',' | ';').then_some(value_start + offset))
        .unwrap_or(token.len());
    let mut out = String::new();
    out.push_str(&token[..value_start]);
    out.push_str("[REDACTED]");
    out.push_str(&token[value_end..]);
    out
}

fn redact_bearer_values(input: &str) -> String {
    let mut output = String::new();
    let mut words = input.split_whitespace().peekable();
    while let Some(word) = words.next() {
        if !output.is_empty() {
            output.push(' ');
        }
        if word.eq_ignore_ascii_case("bearer") && words.peek().is_some() {
            output.push_str(word);
            output.push(' ');
            output.push_str("[REDACTED]");
            let _ = words.next();
        } else {
            output.push_str(word);
        }
    }
    if input.ends_with(char::is_whitespace) {
        output.push(' ');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_contracts::{from_json_strict, to_canonical_json};

    const RISK_REJECT: &str =
        include_str!("../../../fixtures/replay/risk_reject/expected/risk_decisions.jsonl");
    const EXECUTION_READ_ONLY_REPORT: &str = include_str!(
        "../../../fixtures/replay/execution_read_only/expected/execution_reports.jsonl"
    );
    const MANUAL_APPROVAL_APPROVED_RECORDS: &str = include_str!(
        "../../../fixtures/replay/manual_approval_approved/expected/approval_records.jsonl"
    );
    const LEDGER_ENTRY: &str =
        include_str!("../../../fixtures/schema/valid/ledger_entry.valid.json");
    const NORMALIZED_EVENT: &str =
        include_str!("../../../fixtures/schema/valid/normalized_event.valid.json");
    const INCIDENT: &str = include_str!("../../../fixtures/schema/valid/incident.valid.json");

    #[test]
    fn daily_report_fields_are_complete_and_render_from_structured_facts() {
        let facts = sample_facts();
        let report =
            generate_daily_report(&facts, "2026-01-01", "2026-01-01T23:59:59Z").expect("report");

        report.validate_complete().expect("complete fields");
        assert_eq!(report.source_event_count, 1);
        assert_eq!(report.risk_decision_count, 1);
        assert_eq!(report.rejected_decision_count, 1);
        assert_eq!(report.execution_report_count, 1);
        assert_eq!(report.ledger_entry_count, 1);
        assert_eq!(report.incident_count, 1);
        assert_eq!(
            report.risk_decisions_by_kind,
            vec![CountByLabel::new("Rejected", 1)]
        );

        let rendered = report.render_markdown().expect("markdown");
        assert!(rendered.contains("Daily Operations Report"));
        assert!(rendered.contains("- Rejected: 1"));
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn rejection_and_incident_reports_are_complete() {
        let facts = sample_facts();
        let rejection =
            generate_rejection_report(&facts, "2026-01-01T23:59:59Z").expect("rejection report");
        rejection.validate_complete().expect("complete rejection");
        assert_eq!(rejection.rejection_count, 1);
        assert_eq!(rejection.records[0].decision_id, "risk:trans:01");
        assert_eq!(rejection.records[0].reason_codes, vec!["HIGH_SLIPPAGE"]);
        assert!(!rejection.records[0].failed_checks.is_empty());

        let incident =
            generate_incident_report(&facts, "2026-01-01T23:59:59Z").expect("incident report");
        incident.validate_complete().expect("complete incident");
        assert_eq!(incident.incident_count, 1);
        assert_eq!(incident.records[0].incident_id, "incident:01");
        assert_eq!(incident.records[0].venue_ids, vec!["venue:SIM"]);
    }

    #[test]
    fn sensitive_material_is_redacted_from_reports() {
        let mut facts = sample_facts();
        facts.risk_decisions[0].detail = Some(
            "operator note api_key=super-secret secret:wallet-token private_key=deadbeef Bearer abc"
                .to_owned(),
        );
        facts.incidents[0].manual_actions[0].detail =
            Some("manual note token=hidden credential=raw".to_owned());

        let rejection =
            generate_rejection_report(&facts, "2026-01-01T23:59:59Z").expect("rejection report");
        let incident =
            generate_incident_report(&facts, "2026-01-01T23:59:59Z").expect("incident report");
        let rendered = format!(
            "{}\n{}",
            rejection.render_markdown().expect("rejection markdown"),
            incident.render_markdown().expect("incident markdown")
        );

        assert!(!rendered.contains("super-secret"));
        assert!(!rendered.contains("wallet-token"));
        assert!(!rendered.contains("deadbeef"));
        assert!(!rendered.contains("hidden"));
        assert!(!rendered.contains("raw"));
        assert!(redact_sensitive_text("api_key=super-secret").contains("[REDACTED]"));
    }

    #[test]
    fn read_only_query_traces_event_strategy_venue_and_account() {
        let facts = sample_facts();

        let event_result =
            query_facts(&facts, OpsQuery::EventId("event:01".to_owned())).expect("event query");
        assert!(event_result.read_only);
        assert!(event_result
            .matches
            .iter()
            .any(|item| item.fact_type == "event"));
        assert!(event_result
            .matches
            .iter()
            .any(|item| item.fact_type == "ledger_entry"));
        assert!(event_result
            .matches
            .iter()
            .any(|item| item.fact_type == "incident"));

        let strategy_result = query_facts(&facts, OpsQuery::StrategyId("strat:demo".to_owned()))
            .expect("strategy query");
        assert!(strategy_result
            .matches
            .iter()
            .any(|item| item.fact_type == "ledger_entry"));

        let venue_result =
            query_facts(&facts, OpsQuery::VenueId("venue:SIM".to_owned())).expect("venue query");
        assert!(venue_result
            .matches
            .iter()
            .any(|item| item.fact_type == "incident"));

        let account_result =
            query_facts(&facts, OpsQuery::AccountId("acct:sim".to_owned())).expect("account query");
        assert!(account_result
            .matches
            .iter()
            .any(|item| item.fact_type == "ledger_entry"));
    }

    #[test]
    fn read_only_command_runner_does_not_trigger_mutable_actions() {
        let reader = ProbeReader::new(sample_facts());
        let output = ReadOnlyOpsEngine
            .run(
                &reader,
                OpsReadOnlyCommand::Query(OpsQuery::AccountId("acct:sim".to_owned())),
            )
            .expect("query command");

        assert!(matches!(output, OpsCommandOutput::Query(_)));
        assert_eq!(reader.read_count.get(), 1);
        assert_eq!(reader.mutable_action_count.get(), 0);
    }

    #[test]
    fn reports_are_canonical_fact_derived_not_hand_written() {
        let facts = sample_facts();
        let canonical_decision = to_canonical_json(&facts.risk_decisions[0]);
        let rejection =
            generate_rejection_report(&facts, "2026-01-01T23:59:59Z").expect("rejection report");

        assert!(canonical_decision.contains("\"decision\":\"Rejected\""));
        assert_eq!(rejection.records[0].transition_id, "trans:01");
        assert_eq!(rejection.records[0].policy_version, "risk-policy:s5-02");
    }

    #[test]
    fn manual_approval_material_contains_required_summaries_and_records() {
        let risk_decision = manual_risk_decision();
        let plan = manual_plan(MANUAL_PLAN);
        let records = manual_approval_records();
        let report = generate_manual_approval_material(
            &risk_decision,
            &plan,
            "hash:sha256:fixture-manual-plan",
            &records,
            "2026-01-01T00:01:30Z",
        )
        .expect("manual approval material");

        report.validate_complete().expect("complete report");
        assert!(report.approval_summary.approval_required);
        assert!(!report.approval_summary.dispatchable_before_approval);
        assert_eq!(report.risk_checks.len(), risk_decision.checks.len());
        assert_eq!(report.execution_plan_summary.manual_gate_count, 1);
        assert_eq!(report.execution_plan_summary.waiting_dependency_count, 1);
        assert_eq!(report.approved_records.len(), 1);
        assert_eq!(report.rejected_records.len(), 1);
        assert_eq!(report.expired_records.len(), 1);
        assert_eq!(report.duplicate_records.len(), 1);
        assert!(MANUAL_APPROVAL_APPROVED_RECORDS.contains("approval-record:approved:01"));

        let rendered = report.render_markdown().expect("markdown");
        assert!(rendered.contains("Manual Approval Material"));
        assert!(rendered.contains("审批只解除同一计划哈希"));
        assert!(rendered.contains("- ManualApprovalGate: 1"));
        assert!(rendered.contains("approval-record:approved:01"));
    }

    #[test]
    fn manual_approval_material_is_redacted() {
        let mut risk_decision = manual_risk_decision();
        risk_decision.checks[0].detail =
            Some("operator note api_key=raw-key token=raw-token".to_owned());
        let plan = manual_plan(MANUAL_PLAN);
        let records = vec![ManualApprovalAuditRecord {
            record_id: "approval-record:redact:01".to_owned(),
            approval_event_id: "event:approval:redact:01".to_owned(),
            risk_decision_id: "risk:trans:01".to_owned(),
            transition_id: "trans:01".to_owned(),
            plan_id: "plan:risk:trans:01".to_owned(),
            plan_hash: "hash:sha256:fixture-manual-plan".to_owned(),
            decision: "Approve".to_owned(),
            status: "Approved".to_owned(),
            reviewer_id: "operator api_secret=raw-secret".to_owned(),
            decided_at: "2026-01-01T00:01:00Z".to_owned(),
            expires_at: "2026-01-01T00:05:00Z".to_owned(),
            reason: Some("reviewed Bearer raw-bearer credential=raw-credential".to_owned()),
            duplicate_of: None,
            releases_manual_gate: true,
            controlled_next_step: "release gate only; private_key=raw-private".to_owned(),
        }];

        let rendered = generate_manual_approval_material(
            &risk_decision,
            &plan,
            "hash:sha256:fixture-manual-plan",
            &records,
            "2026-01-01T00:01:30Z",
        )
        .expect("manual approval material")
        .render_markdown()
        .expect("markdown");

        assert!(!rendered.contains("raw-key"));
        assert!(!rendered.contains("raw-token"));
        assert!(!rendered.contains("raw-secret"));
        assert!(!rendered.contains("raw-bearer"));
        assert!(!rendered.contains("raw-credential"));
        assert!(!rendered.contains("raw-private"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn manual_approval_material_rejects_risk_plan_mismatch() {
        let risk_decision = manual_risk_decision();
        let mismatched_plan = manual_plan(&MANUAL_PLAN.replace(
            "\"risk_decision_id\":\"risk:trans:01\"",
            "\"risk_decision_id\":\"risk:other\"",
        ));
        let error = generate_manual_approval_material(
            &risk_decision,
            &mismatched_plan,
            "hash:sha256:fixture-manual-plan",
            &[],
            "2026-01-01T00:01:30Z",
        )
        .expect_err("risk and plan mismatch must fail");

        assert!(matches!(
            error,
            OpsError::ManualApprovalMismatch {
                field: "risk_decision_id",
                ..
            }
        ));
    }

    fn sample_facts() -> OperationsFacts {
        let risk_decision = parse_jsonl_first::<RiskDecision>(RISK_REJECT);
        let execution_report = parse_jsonl_first::<ExecutionReport>(EXECUTION_READ_ONLY_REPORT);
        let ledger_entry = from_json_strict::<LedgerEntry>(LEDGER_ENTRY).expect("ledger fixture");
        let incident = from_json_strict::<Incident>(INCIDENT).expect("incident fixture");
        let event = from_json_strict::<NormalizedEvent>(NORMALIZED_EVENT).expect("event fixture");
        let event_canonical = to_canonical_json(&event);
        let stored_event = StoredEvent {
            sequence: event.sequence.expect("fixture sequence"),
            event_hash: event.checksum.as_str().to_owned(),
            event,
            canonical_json: event_canonical,
        };

        OperationsFacts {
            events: vec![stored_event],
            risk_decisions: vec![risk_decision],
            execution_plans: Vec::new(),
            execution_reports: vec![execution_report],
            ledger_entries: vec![ledger_entry],
            reconciliation_reports: Vec::new(),
            incidents: vec![incident],
            manual_approval_records: Vec::new(),
        }
    }

    fn parse_jsonl_first<T>(input: &str) -> T
    where
        T: arb_contracts::ContractJson,
    {
        from_json_strict(input.lines().next().expect("one JSONL line")).expect("valid fixture")
    }

    fn manual_risk_decision() -> RiskDecision {
        let json = RISK_REJECT
            .trim()
            .replace(
                "\"decision\":\"Rejected\"",
                "\"decision\":\"RequiresManualApproval\"",
            )
            .replace(
                "\"reason_codes\":[\"HIGH_SLIPPAGE\"]",
                "\"reason_codes\":[\"REQUIRES_MANUAL_APPROVAL\"]",
            )
            .replace(
                "风控入口拒绝候选转换，不得生成可执行计划。",
                "风控入口要求人工审批；审批前不得分发执行。",
            );
        from_json_strict(&json).expect("manual risk decision")
    }

    fn manual_plan(input: &str) -> ExecutionPlan {
        from_json_strict(input).expect("manual execution plan")
    }

    fn manual_approval_records() -> Vec<ManualApprovalAuditRecord> {
        vec![
            ManualApprovalAuditRecord {
                record_id: "approval-record:approved:01".to_owned(),
                approval_event_id: "event:approval:approved:01".to_owned(),
                risk_decision_id: "risk:trans:01".to_owned(),
                transition_id: "trans:01".to_owned(),
                plan_id: "plan:risk:trans:01".to_owned(),
                plan_hash: "hash:sha256:fixture-manual-plan".to_owned(),
                decision: "Approve".to_owned(),
                status: "Approved".to_owned(),
                reviewer_id: "operator:alice".to_owned(),
                decided_at: "2026-01-01T00:01:00Z".to_owned(),
                expires_at: "2026-01-01T00:05:00Z".to_owned(),
                reason: Some("Risk and plan hash reviewed.".to_owned()),
                duplicate_of: None,
                releases_manual_gate: true,
                controlled_next_step: "Release manual gate only.".to_owned(),
            },
            ManualApprovalAuditRecord {
                record_id: "approval-record:rejected:01".to_owned(),
                approval_event_id: "event:approval:rejected:01".to_owned(),
                risk_decision_id: "risk:trans:01".to_owned(),
                transition_id: "trans:01".to_owned(),
                plan_id: "plan:risk:trans:01".to_owned(),
                plan_hash: "hash:sha256:fixture-manual-plan".to_owned(),
                decision: "Reject".to_owned(),
                status: "Rejected".to_owned(),
                reviewer_id: "operator:bob".to_owned(),
                decided_at: "2026-01-01T00:02:00Z".to_owned(),
                expires_at: "2026-01-01T00:05:00Z".to_owned(),
                reason: Some("Operator rejected execution.".to_owned()),
                duplicate_of: None,
                releases_manual_gate: false,
                controlled_next_step: "Stop before dispatch.".to_owned(),
            },
            ManualApprovalAuditRecord {
                record_id: "approval-record:expired:01".to_owned(),
                approval_event_id: "event:approval:expired:01".to_owned(),
                risk_decision_id: "risk:trans:01".to_owned(),
                transition_id: "trans:01".to_owned(),
                plan_id: "plan:risk:trans:01".to_owned(),
                plan_hash: "hash:sha256:fixture-manual-plan".to_owned(),
                decision: "Approve".to_owned(),
                status: "Expired".to_owned(),
                reviewer_id: "operator:carol".to_owned(),
                decided_at: "2026-01-01T00:06:00Z".to_owned(),
                expires_at: "2026-01-01T00:05:00Z".to_owned(),
                reason: Some("Approval arrived after expiry.".to_owned()),
                duplicate_of: None,
                releases_manual_gate: false,
                controlled_next_step: "Stop before dispatch.".to_owned(),
            },
            ManualApprovalAuditRecord {
                record_id: "approval-record:duplicate:01".to_owned(),
                approval_event_id: "event:approval:duplicate:01".to_owned(),
                risk_decision_id: "risk:trans:01".to_owned(),
                transition_id: "trans:01".to_owned(),
                plan_id: "plan:risk:trans:01".to_owned(),
                plan_hash: "hash:sha256:fixture-manual-plan".to_owned(),
                decision: "Approve".to_owned(),
                status: "Duplicate".to_owned(),
                reviewer_id: "operator:dana".to_owned(),
                decided_at: "2026-01-01T00:03:00Z".to_owned(),
                expires_at: "2026-01-01T00:05:00Z".to_owned(),
                reason: Some("Duplicate attempt ignored.".to_owned()),
                duplicate_of: Some("approval-record:approved:01".to_owned()),
                releases_manual_gate: false,
                controlled_next_step: "Keep first terminal record authoritative.".to_owned(),
            },
        ]
    }

    const MANUAL_PLAN: &str = r#"{"schema_version":"1.0.0","plan_id":"plan:risk:trans:01","transition_id":"trans:01","risk_decision_id":"risk:trans:01","created_at":"2026-01-01T00:00:04Z","execution_mode":"ManualApproval","idempotency_key":"idem:plan:risk:trans:01","legs":[{"plan_leg_id":"pleg:plan:risk:trans:01:manual-gate","candidate_leg_id":"manual-gate:risk:trans:01","action_type":"ManualApprovalGate","account_id":"acct:sim","idempotency_key":"idem:pleg:plan:risk:trans:01:manual-gate","state":"Prepared","failure_semantics":"ManualInterventionRequired"},{"plan_leg_id":"pleg:plan:risk:trans:01:0001","candidate_leg_id":"candleg:01","action_type":"PlaceOrder","venue_id":"venue:SIM","instrument_id":"inst:BTC-USDC","account_id":"acct:sim","idempotency_key":"idem:plan:risk:trans:01:candleg:01","depends_on":["pleg:plan:risk:trans:01:manual-gate"],"state":"WaitingDependency","failure_semantics":"NoOpFailure"}],"dependency_graph":{"edges":[{"from_leg_id":"pleg:plan:risk:trans:01:manual-gate","to_leg_id":"pleg:plan:risk:trans:01:0001","condition":"ManualRelease"}]},"constraints":{"requires_fresh_market_data_ms":5000,"slippage_limit_bps":"5"},"timeout_policy":{"plan_timeout_ms":60000,"leg_timeout_ms":10000,"unknown_state_after_ms":15000},"cancel_policy":{"default_action":"CancelOpenOrders"},"hedge_policy":{"residual_exposure_action":"ManualIntervention"},"partial_fill_policy":{"action":"ManualIntervention"},"failure_policy":{"unknown_state_action":"HaltAndIncident","retry_limit":0}}"#;

    struct ProbeReader {
        facts: OperationsFacts,
        read_count: std::cell::Cell<usize>,
        mutable_action_count: std::cell::Cell<usize>,
    }

    impl ProbeReader {
        fn new(facts: OperationsFacts) -> Self {
            Self {
                facts,
                read_count: std::cell::Cell::new(0),
                mutable_action_count: std::cell::Cell::new(0),
            }
        }
    }

    impl OpsFactReader for ProbeReader {
        fn read_facts(&self) -> OpsResult<OperationsFacts> {
            self.read_count.set(self.read_count.get() + 1);
            Ok(self.facts.clone())
        }
    }
}
