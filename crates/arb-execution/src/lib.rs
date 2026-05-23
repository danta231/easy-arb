//! `arb-execution` 执行计划构建入口。
//!
//! 中文说明：本 crate 只把已经通过风控的 `RiskDecision` 和候选转换转换为
//! 可回放的 `ExecutionPlan` 或人工审批预览。这里不下单、不撤单、不签名、
//! 不写实盘账本，也不依赖任何真实交易 API。

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use arb_contracts::{
    from_json_strict, to_canonical_json, CancelDefaultAction, CandidatePortfolioTransition,
    CapitalReservation, CapitalReservationState, ContractError, ExecutionActionType,
    ExecutionFailureSeverity, ExecutionLegState, ExecutionMode, ExecutionOrderType, ExecutionPlan,
    ExecutionReport, ExecutionReportStatus, ExecutionTimeInForce, FailureMode, FillSide,
    HedgeResidualAction, Incident, LedgerEntry, LedgerNamespace, LegReportStatus,
    PartialFillAction, PortfolioState, ReconciliationStatus, RiskCheckType, RiskConstraint,
    RiskConstraintType, RiskDecision, RiskDecisionKind, TransitionLeg, TransitionLegType,
    TransitionSide,
};

/// 执行层统一返回类型。
pub type ExecutionResult<T> = Result<T, ExecutionError>;

/// S6-01 默认计划超时。
pub const DEFAULT_PLAN_TIMEOUT_MS: u64 = 60_000;
/// S6-01 默认单腿超时。
pub const DEFAULT_LEG_TIMEOUT_MS: u64 = 10_000;
/// S6-01 默认未知状态进入对账/事故前的等待时间。
pub const DEFAULT_UNKNOWN_STATE_AFTER_MS: u64 = 15_000;

/// 执行计划构建错误。
///
/// 中文说明：错误只表达计划构建失败；风控拒绝本身不是异常，但必须阻断计划生成。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionError {
    RiskDecisionNotApproved {
        decision: RiskDecisionKind,
        risk_decision_id: String,
    },
    TransitionMismatch {
        risk_transition_id: String,
        candidate_transition_id: String,
    },
    MissingRequiredField {
        field: &'static str,
        detail: String,
    },
    LiveExecutionUnavailable {
        mode: ExecutionMode,
    },
    PendingManualApproval {
        plan_id: String,
    },
    ManualApprovalPlanHashMismatch {
        expected: String,
        actual: String,
    },
    ManualApprovalRecordMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    ManualApprovalNotReleased {
        plan_id: String,
        status: ManualApprovalStatus,
    },
    ExecutionReportPlanMismatch {
        plan_id: String,
        report_plan_id: String,
    },
    InvalidCapitalReservationTransition {
        reservation_id: String,
        from: CapitalReservationState,
        to: CapitalReservationState,
    },
    InvalidExecutionLegTransition {
        plan_leg_id: String,
        from: ExecutionLegState,
        to: ExecutionLegState,
    },
    Contract(ContractError),
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RiskDecisionNotApproved {
                decision,
                risk_decision_id,
            } => write!(
                f,
                "risk decision `{risk_decision_id}` is `{}` and cannot produce a dispatchable plan",
                decision.as_str()
            ),
            Self::TransitionMismatch {
                risk_transition_id,
                candidate_transition_id,
            } => write!(
                f,
                "risk decision transition `{risk_transition_id}` does not match candidate transition `{candidate_transition_id}`"
            ),
            Self::MissingRequiredField { field, detail } => {
                write!(f, "{field}: missing required execution planning field: {detail}")
            }
            Self::LiveExecutionUnavailable { mode } => write!(
                f,
                "execution mode `{}` is closed in S6-01 because no live order, transfer, or signing implementation exists",
                mode.as_str()
            ),
            Self::PendingManualApproval { plan_id } => write!(
                f,
                "execution plan preview `{plan_id}` is pending manual approval and is not dispatchable"
            ),
            Self::ManualApprovalPlanHashMismatch { expected, actual } => write!(
                f,
                "manual approval plan hash mismatch: expected `{expected}`, got `{actual}`"
            ),
            Self::ManualApprovalRecordMismatch {
                field,
                expected,
                actual,
            } => write!(
                f,
                "manual approval record field `{field}` mismatch: expected `{expected}`, got `{actual}`"
            ),
            Self::ManualApprovalNotReleased { plan_id, status } => write!(
                f,
                "manual approval for plan `{plan_id}` has status `{}` and cannot release the approval gate",
                status.as_str()
            ),
            Self::ExecutionReportPlanMismatch {
                plan_id,
                report_plan_id,
            } => write!(
                f,
                "execution report plan `{report_plan_id}` does not match execution plan `{plan_id}`"
            ),
            Self::InvalidCapitalReservationTransition {
                reservation_id,
                from,
                to,
            } => write!(
                f,
                "capital reservation `{reservation_id}` cannot transition from `{}` to `{}`",
                from.as_str(),
                to.as_str()
            ),
            Self::InvalidExecutionLegTransition {
                plan_leg_id,
                from,
                to,
            } => write!(
                f,
                "execution leg `{plan_leg_id}` cannot transition from `{}` to `{}`",
                from.as_str(),
                to.as_str()
            ),
            Self::Contract(error) => write!(f, "{error}"),
        }
    }
}

impl Error for ExecutionError {}

impl From<ContractError> for ExecutionError {
    fn from(error: ContractError) -> Self {
        Self::Contract(error)
    }
}

/// 执行计划构建输入。
///
/// 中文说明：执行计划必须同时引用风控决策和候选转换，并校验二者的
/// `transition_id` 一致，避免绕过风控传入另一组执行腿。
#[derive(Clone)]
pub struct ExecutionPlanBuildInput<'a> {
    risk_decision: &'a RiskDecision,
    candidate: &'a CandidatePortfolioTransition,
    execution_mode: ExecutionMode,
    created_at: &'a str,
}

impl<'a> ExecutionPlanBuildInput<'a> {
    pub fn new(
        risk_decision: &'a RiskDecision,
        candidate: &'a CandidatePortfolioTransition,
        execution_mode: ExecutionMode,
        created_at: &'a str,
    ) -> Self {
        Self {
            risk_decision,
            candidate,
            execution_mode,
            created_at,
        }
    }

    pub fn risk_decision(&self) -> &'a RiskDecision {
        self.risk_decision
    }

    pub fn candidate(&self) -> &'a CandidatePortfolioTransition {
        self.candidate
    }

    pub fn execution_mode(&self) -> &ExecutionMode {
        &self.execution_mode
    }

    pub fn created_at(&self) -> &str {
        self.created_at
    }
}

/// 资本预留请求输入。
///
/// 中文说明：这里创建的是可回放的预留状态事实，不触发真实划转、真实锁仓或
/// 账本写入。调用方必须先由事件存储和账本模块产生引用，再把引用传入状态机。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapitalReservationRequest<'a> {
    pub reservation_id: &'a str,
    pub asset_id: &'a str,
    pub amount: &'a str,
    pub reserved_for: &'a str,
    pub expires_at: &'a str,
    pub refs: CapitalReservationRefs<'a>,
}

impl<'a> CapitalReservationRequest<'a> {
    pub fn new(
        reservation_id: &'a str,
        asset_id: &'a str,
        amount: &'a str,
        reserved_for: &'a str,
        expires_at: &'a str,
        refs: CapitalReservationRefs<'a>,
    ) -> Self {
        Self {
            reservation_id,
            asset_id,
            amount,
            reserved_for,
            expires_at,
            refs,
        }
    }
}

/// 资本预留事实引用。
///
/// 中文说明：`source_event_id` 指向资本预留事件；`ledger_entry_id` 指向追加式
/// 账本中的预留、释放、过期或对账差异分录。执行层只保留引用，不改账本历史。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapitalReservationRefs<'a> {
    pub source_event_id: &'a str,
    pub ledger_entry_id: &'a str,
}

impl<'a> CapitalReservationRefs<'a> {
    pub fn new(source_event_id: &'a str, ledger_entry_id: &'a str) -> Self {
        Self {
            source_event_id,
            ledger_entry_id,
        }
    }
}

/// 资本预留状态转换结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapitalReservationTransition {
    pub reservation: CapitalReservation,
    pub from_state: Option<CapitalReservationState>,
    pub to_state: CapitalReservationState,
    pub source_event_id: String,
    pub ledger_entry_id: String,
    pub idempotent: bool,
}

/// 执行腿状态迁移结果。
///
/// 中文说明：执行腿状态机只记录模拟和回放可验证的状态事实，不调用任何场所、
/// 签名器或真实账户动作。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionLegTransition {
    pub plan_leg_id: String,
    pub from_state: ExecutionLegState,
    pub to_state: ExecutionLegState,
    pub source_event_id: String,
    pub idempotent: bool,
}

/// 资本预留状态机接口。
///
/// 中文说明：状态机只消费明确事件和账本引用，输出新的预留状态事实。它不读取
/// 外部余额、不绕过风控，也不提交真实资金动作。
pub trait CapitalReservationStateMachine {
    fn request(
        &self,
        input: CapitalReservationRequest<'_>,
    ) -> ExecutionResult<CapitalReservationTransition>;

    fn reserve(
        &self,
        reservation: &CapitalReservation,
        refs: CapitalReservationRefs<'_>,
    ) -> ExecutionResult<CapitalReservationTransition>;

    fn convert_to_execution(
        &self,
        reservation: &CapitalReservation,
        refs: CapitalReservationRefs<'_>,
    ) -> ExecutionResult<CapitalReservationTransition>;

    fn release(
        &self,
        reservation: &CapitalReservation,
        refs: CapitalReservationRefs<'_>,
    ) -> ExecutionResult<CapitalReservationTransition>;

    fn expire(
        &self,
        reservation: &CapitalReservation,
        refs: CapitalReservationRefs<'_>,
    ) -> ExecutionResult<CapitalReservationTransition>;

    fn mark_reconciled_mismatch(
        &self,
        reservation: &CapitalReservation,
        refs: CapitalReservationRefs<'_>,
    ) -> ExecutionResult<CapitalReservationTransition>;
}

/// 默认资本预留状态机。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaticCapitalReservationStateMachine;

impl CapitalReservationStateMachine for StaticCapitalReservationStateMachine {
    fn request(
        &self,
        input: CapitalReservationRequest<'_>,
    ) -> ExecutionResult<CapitalReservationTransition> {
        validate_required_reservation_refs(input.refs)?;
        let reservation = capital_reservation_from_parts(CapitalReservationParts {
            reservation_id: input.reservation_id,
            state: CapitalReservationState::Requested,
            asset_id: input.asset_id,
            amount: input.amount,
            reserved_for: input.reserved_for,
            expires_at: input.expires_at,
            refs: input.refs,
        })?;
        Ok(CapitalReservationTransition {
            reservation,
            from_state: None,
            to_state: CapitalReservationState::Requested,
            source_event_id: input.refs.source_event_id.to_owned(),
            ledger_entry_id: input.refs.ledger_entry_id.to_owned(),
            idempotent: false,
        })
    }

    fn reserve(
        &self,
        reservation: &CapitalReservation,
        refs: CapitalReservationRefs<'_>,
    ) -> ExecutionResult<CapitalReservationTransition> {
        transition_capital_reservation_to(reservation, CapitalReservationState::Reserved, refs)
    }

    fn convert_to_execution(
        &self,
        reservation: &CapitalReservation,
        refs: CapitalReservationRefs<'_>,
    ) -> ExecutionResult<CapitalReservationTransition> {
        transition_capital_reservation_to(
            reservation,
            CapitalReservationState::ConvertedToExecution,
            refs,
        )
    }

    fn release(
        &self,
        reservation: &CapitalReservation,
        refs: CapitalReservationRefs<'_>,
    ) -> ExecutionResult<CapitalReservationTransition> {
        transition_capital_reservation_to(reservation, CapitalReservationState::Released, refs)
    }

    fn expire(
        &self,
        reservation: &CapitalReservation,
        refs: CapitalReservationRefs<'_>,
    ) -> ExecutionResult<CapitalReservationTransition> {
        transition_capital_reservation_to(reservation, CapitalReservationState::Expired, refs)
    }

    fn mark_reconciled_mismatch(
        &self,
        reservation: &CapitalReservation,
        refs: CapitalReservationRefs<'_>,
    ) -> ExecutionResult<CapitalReservationTransition> {
        transition_capital_reservation_to(
            reservation,
            CapitalReservationState::ReconciledMismatch,
            refs,
        )
    }
}

/// 执行腿状态机接口。
pub trait ExecutionLegStateMachine {
    fn advance(
        &self,
        plan_leg_id: &str,
        from_state: &ExecutionLegState,
        to_state: ExecutionLegState,
        source_event_id: &str,
    ) -> ExecutionResult<ExecutionLegTransition>;
}

/// 默认执行腿状态机。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaticExecutionLegStateMachine;

impl ExecutionLegStateMachine for StaticExecutionLegStateMachine {
    fn advance(
        &self,
        plan_leg_id: &str,
        from_state: &ExecutionLegState,
        to_state: ExecutionLegState,
        source_event_id: &str,
    ) -> ExecutionResult<ExecutionLegTransition> {
        if source_event_id.trim().is_empty() {
            return Err(ExecutionError::MissingRequiredField {
                field: "execution_leg_transition.source_event_id",
                detail: "execution leg transitions must reference a deterministic event".to_owned(),
            });
        }

        let idempotent = from_state == &to_state;
        if !idempotent && !is_allowed_execution_leg_transition(from_state, &to_state) {
            return Err(ExecutionError::InvalidExecutionLegTransition {
                plan_leg_id: plan_leg_id.to_owned(),
                from: from_state.clone(),
                to: to_state,
            });
        }

        Ok(ExecutionLegTransition {
            plan_leg_id: plan_leg_id.to_owned(),
            from_state: from_state.clone(),
            to_state,
            source_event_id: source_event_id.to_owned(),
            idempotent,
        })
    }
}

pub fn request_capital_reservation(
    input: CapitalReservationRequest<'_>,
) -> ExecutionResult<CapitalReservationTransition> {
    StaticCapitalReservationStateMachine.request(input)
}

pub fn reserve_capital_reservation(
    reservation: &CapitalReservation,
    refs: CapitalReservationRefs<'_>,
) -> ExecutionResult<CapitalReservationTransition> {
    StaticCapitalReservationStateMachine.reserve(reservation, refs)
}

pub fn convert_capital_reservation_to_execution(
    reservation: &CapitalReservation,
    refs: CapitalReservationRefs<'_>,
) -> ExecutionResult<CapitalReservationTransition> {
    StaticCapitalReservationStateMachine.convert_to_execution(reservation, refs)
}

pub fn release_capital_reservation(
    reservation: &CapitalReservation,
    refs: CapitalReservationRefs<'_>,
) -> ExecutionResult<CapitalReservationTransition> {
    StaticCapitalReservationStateMachine.release(reservation, refs)
}

pub fn expire_capital_reservation(
    reservation: &CapitalReservation,
    refs: CapitalReservationRefs<'_>,
) -> ExecutionResult<CapitalReservationTransition> {
    StaticCapitalReservationStateMachine.expire(reservation, refs)
}

pub fn mark_capital_reservation_reconciled_mismatch(
    reservation: &CapitalReservation,
    refs: CapitalReservationRefs<'_>,
) -> ExecutionResult<CapitalReservationTransition> {
    StaticCapitalReservationStateMachine.mark_reconciled_mismatch(reservation, refs)
}

pub fn transition_execution_leg(
    plan_leg_id: &str,
    from_state: &ExecutionLegState,
    to_state: ExecutionLegState,
    source_event_id: &str,
) -> ExecutionResult<ExecutionLegTransition> {
    StaticExecutionLegStateMachine.advance(plan_leg_id, from_state, to_state, source_event_id)
}

/// 执行计划策略参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionPlanPolicy {
    plan_timeout_ms: u64,
    leg_timeout_ms: u64,
    unknown_state_after_ms: u64,
    retry_limit: u64,
    retry_backoff_ms: Option<u64>,
}

impl ExecutionPlanPolicy {
    pub fn new(plan_timeout_ms: u64, leg_timeout_ms: u64, unknown_state_after_ms: u64) -> Self {
        Self {
            plan_timeout_ms,
            leg_timeout_ms,
            unknown_state_after_ms,
            retry_limit: 0,
            retry_backoff_ms: None,
        }
    }

    pub fn with_retry_policy(mut self, retry_limit: u64, retry_backoff_ms: Option<u64>) -> Self {
        self.retry_limit = retry_limit;
        self.retry_backoff_ms = retry_backoff_ms;
        self
    }
}

impl Default for ExecutionPlanPolicy {
    fn default() -> Self {
        Self::new(
            DEFAULT_PLAN_TIMEOUT_MS,
            DEFAULT_LEG_TIMEOUT_MS,
            DEFAULT_UNKNOWN_STATE_AFTER_MS,
        )
    }
}

/// 执行计划构建入口 trait。
pub trait ExecutionPlanner {
    fn build(&self, input: ExecutionPlanBuildInput<'_>) -> ExecutionResult<PlanBuildOutcome>;
}

/// 模拟执行输入。
///
/// 中文说明：模拟执行必须使用显式计划、固定时间和可选脚本；默认脚本也只由
/// 计划内容派生，避免读取系统时间、网络或真实账户状态。
#[derive(Clone, Copy)]
pub struct SimulatedExecutionInput<'a> {
    plan: &'a ExecutionPlan,
    generated_at: &'a str,
    directives: &'a [SimulationLegDirective],
}

impl<'a> SimulatedExecutionInput<'a> {
    pub fn new(
        plan: &'a ExecutionPlan,
        generated_at: &'a str,
        directives: &'a [SimulationLegDirective],
    ) -> Self {
        Self {
            plan,
            generated_at,
            directives,
        }
    }

    pub fn plan(&self) -> &'a ExecutionPlan {
        self.plan
    }

    pub fn generated_at(&self) -> &str {
        self.generated_at
    }

    pub fn directives(&self) -> &'a [SimulationLegDirective] {
        self.directives
    }
}

/// 单条执行腿的模拟脚本。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulationLegDirective {
    pub plan_leg_id: String,
    pub outcome: SimulationLegOutcome,
}

impl SimulationLegDirective {
    pub fn new(plan_leg_id: impl Into<String>, outcome: SimulationLegOutcome) -> Self {
        Self {
            plan_leg_id: plan_leg_id.into(),
            outcome,
        }
    }
}

/// 模拟执行腿结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SimulationLegOutcome {
    FullFill(SimulatedFill),
    PartialFill(SimulatedFill),
    PartialFillThenCancel(SimulatedFill, SimulatedCancel),
    Timeout(SimulatedTimeout),
    Failure(SimulatedFailure),
    Unknown(SimulatedUnknown),
    CompensatedUnknown(SimulatedUnknown),
    Skip,
}

/// 模拟成交参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatedFill {
    pub side: FillSide,
    pub price: String,
    pub quantity: String,
    pub fee_asset_id: String,
    pub fee_amount: String,
    pub source_event_id: String,
    pub ledger_entry_id: Option<String>,
}

impl SimulatedFill {
    pub fn new(
        side: FillSide,
        price: impl Into<String>,
        quantity: impl Into<String>,
        fee_asset_id: impl Into<String>,
        fee_amount: impl Into<String>,
        source_event_id: impl Into<String>,
    ) -> Self {
        Self {
            side,
            price: price.into(),
            quantity: quantity.into(),
            fee_asset_id: fee_asset_id.into(),
            fee_amount: fee_amount.into(),
            source_event_id: source_event_id.into(),
            ledger_entry_id: None,
        }
    }

    pub fn with_ledger_entry_id(mut self, ledger_entry_id: impl Into<String>) -> Self {
        self.ledger_entry_id = Some(ledger_entry_id.into());
        self
    }
}

/// 模拟撤单参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatedCancel {
    pub source_event_id: String,
    pub detail: String,
}

impl SimulatedCancel {
    pub fn new(source_event_id: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            source_event_id: source_event_id.into(),
            detail: detail.into(),
        }
    }
}

/// 模拟超时参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatedTimeout {
    pub source_event_id: String,
    pub elapsed_ms: u64,
    pub detail: String,
}

impl SimulatedTimeout {
    pub fn new(
        source_event_id: impl Into<String>,
        elapsed_ms: u64,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            source_event_id: source_event_id.into(),
            elapsed_ms,
            detail: detail.into(),
        }
    }
}

/// 模拟失败参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatedFailure {
    pub failure_type: FailureMode,
    pub severity: ExecutionFailureSeverity,
    pub source_event_id: String,
    pub detail: String,
}

impl SimulatedFailure {
    pub fn new(
        failure_type: FailureMode,
        severity: ExecutionFailureSeverity,
        source_event_id: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            failure_type,
            severity,
            source_event_id: source_event_id.into(),
            detail: detail.into(),
        }
    }
}

/// 模拟未知状态参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatedUnknown {
    pub source_event_id: String,
    pub detail: String,
    pub compensation_event_id: Option<String>,
}

impl SimulatedUnknown {
    pub fn new(source_event_id: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            source_event_id: source_event_id.into(),
            detail: detail.into(),
            compensation_event_id: None,
        }
    }

    pub fn with_compensation_event_id(mut self, event_id: impl Into<String>) -> Self {
        self.compensation_event_id = Some(event_id.into());
        self
    }
}

/// 私有订单确认来源。
///
/// 中文说明：REST 下单回执不是最终成交事实；只有私有订单流或查单结果能进入
/// 执行报告生成路径。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateOrderConfirmationSource {
    PrivateStream,
    OrderQuery,
}

impl PrivateOrderConfirmationSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrivateStream => "PrivateStream",
            Self::OrderQuery => "OrderQuery",
        }
    }
}

/// 私有订单确认状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateOrderConfirmationStatus {
    Acknowledged,
    Filled,
    PartiallyFilled,
    Cancelled,
    Rejected,
    Expired,
    Unknown,
}

impl PrivateOrderConfirmationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acknowledged => "Acknowledged",
            Self::Filled => "Filled",
            Self::PartiallyFilled => "PartiallyFilled",
            Self::Cancelled => "Cancelled",
            Self::Rejected => "Rejected",
            Self::Expired => "Expired",
            Self::Unknown => "Unknown",
        }
    }
}

/// 私有成交明细。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivateExecutionFill {
    pub side: FillSide,
    pub price: String,
    pub quantity: String,
    pub fee_asset_id: String,
    pub fee_amount: String,
    pub source_event_id: String,
    pub timestamp: Option<String>,
    pub venue_order_id: Option<String>,
    pub client_order_id: Option<String>,
    pub ledger_entry_id: Option<String>,
}

impl PrivateExecutionFill {
    pub fn new(
        side: FillSide,
        price: impl Into<String>,
        quantity: impl Into<String>,
        fee_asset_id: impl Into<String>,
        fee_amount: impl Into<String>,
        source_event_id: impl Into<String>,
    ) -> Self {
        Self {
            side,
            price: price.into(),
            quantity: quantity.into(),
            fee_asset_id: fee_asset_id.into(),
            fee_amount: fee_amount.into(),
            source_event_id: source_event_id.into(),
            timestamp: None,
            venue_order_id: None,
            client_order_id: None,
            ledger_entry_id: None,
        }
    }

    pub fn with_timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    pub fn with_venue_order_id(mut self, venue_order_id: impl Into<String>) -> Self {
        self.venue_order_id = Some(venue_order_id.into());
        self
    }

    pub fn with_client_order_id(mut self, client_order_id: impl Into<String>) -> Self {
        self.client_order_id = Some(client_order_id.into());
        self
    }

    pub fn with_ledger_entry_id(mut self, ledger_entry_id: impl Into<String>) -> Self {
        self.ledger_entry_id = Some(ledger_entry_id.into());
        self
    }
}

/// 单个执行腿的私有订单确认。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivateOrderConfirmation {
    pub plan_leg_id: String,
    pub status: PrivateOrderConfirmationStatus,
    pub source: PrivateOrderConfirmationSource,
    pub source_event_refs: Vec<String>,
    pub venue_order_id: Option<String>,
    pub client_order_id: Option<String>,
    pub fills: Vec<PrivateExecutionFill>,
    pub detail: Option<String>,
}

impl PrivateOrderConfirmation {
    pub fn new(
        plan_leg_id: impl Into<String>,
        status: PrivateOrderConfirmationStatus,
        source: PrivateOrderConfirmationSource,
        source_event_id: impl Into<String>,
    ) -> Self {
        Self {
            plan_leg_id: plan_leg_id.into(),
            status,
            source,
            source_event_refs: vec![source_event_id.into()],
            venue_order_id: None,
            client_order_id: None,
            fills: Vec::new(),
            detail: None,
        }
    }

    pub fn with_venue_order_id(mut self, venue_order_id: impl Into<String>) -> Self {
        self.venue_order_id = Some(venue_order_id.into());
        self
    }

    pub fn with_client_order_id(mut self, client_order_id: impl Into<String>) -> Self {
        self.client_order_id = Some(client_order_id.into());
        self
    }

    pub fn with_fill(mut self, fill: PrivateExecutionFill) -> Self {
        self.fills.push(fill);
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// 私有确认生成执行报告的输入。
#[derive(Clone, Copy)]
pub struct PrivateExecutionReportInput<'a> {
    pub plan: &'a ExecutionPlan,
    pub generated_at: &'a str,
    pub confirmations: &'a [PrivateOrderConfirmation],
}

impl<'a> PrivateExecutionReportInput<'a> {
    pub fn new(
        plan: &'a ExecutionPlan,
        generated_at: &'a str,
        confirmations: &'a [PrivateOrderConfirmation],
    ) -> Self {
        Self {
            plan,
            generated_at,
            confirmations,
        }
    }
}

/// 模拟执行输出。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatedExecution {
    pub report: ExecutionReport,
    pub leg_transitions: Vec<ExecutionLegTransition>,
}

/// 模拟执行器接口。
pub trait SimulatedExecutor {
    fn simulate(&self, input: SimulatedExecutionInput<'_>) -> ExecutionResult<SimulatedExecution>;
}

/// 默认静态执行计划构建器。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticExecutionPlanner {
    policy: ExecutionPlanPolicy,
}

impl StaticExecutionPlanner {
    pub fn new(policy: ExecutionPlanPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &ExecutionPlanPolicy {
        &self.policy
    }
}

impl Default for StaticExecutionPlanner {
    fn default() -> Self {
        Self::new(ExecutionPlanPolicy::default())
    }
}

impl ExecutionPlanner for StaticExecutionPlanner {
    fn build(&self, input: ExecutionPlanBuildInput<'_>) -> ExecutionResult<PlanBuildOutcome> {
        validate_transition_link(&input)?;

        let decision = input.risk_decision.decision.clone();
        let execution_mode = input.execution_mode.clone();

        match decision {
            RiskDecisionKind::Approved | RiskDecisionKind::ApprovedWithConstraints => {
                match execution_mode {
                    ExecutionMode::ReadOnly | ExecutionMode::Simulated => {
                        let plan = build_plan_contract(&input, &self.policy, false)?;
                        Ok(PlanBuildOutcome::Schedulable(plan))
                    }
                    ExecutionMode::ManualApproval => {
                        let pending = build_pending_manual_plan(&input, &self.policy)?;
                        Ok(PlanBuildOutcome::PendingManualApproval(pending))
                    }
                    ExecutionMode::GuardedLive | ExecutionMode::AutonomousLive => {
                        Err(ExecutionError::LiveExecutionUnavailable {
                            mode: execution_mode,
                        })
                    }
                }
            }
            RiskDecisionKind::RequiresManualApproval => {
                let pending = build_pending_manual_plan(&input, &self.policy)?;
                Ok(PlanBuildOutcome::PendingManualApproval(pending))
            }
            RiskDecisionKind::Rejected
            | RiskDecisionKind::RequiresMoreData
            | RiskDecisionKind::SuspendedByCircuitBreaker => {
                Err(ExecutionError::RiskDecisionNotApproved {
                    decision,
                    risk_decision_id: input.risk_decision.decision_id.as_str().to_owned(),
                })
            }
        }
    }
}

/// 默认离线模拟执行器。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaticSimulatedExecutor;

impl SimulatedExecutor for StaticSimulatedExecutor {
    fn simulate(&self, input: SimulatedExecutionInput<'_>) -> ExecutionResult<SimulatedExecution> {
        simulate_plan(input)
    }
}

/// 构建可调度执行计划。
///
/// 中文说明：该便捷函数只返回可调度计划；人工审批预览会以错误返回，
/// 防止调用方把审批前计划当成可分发计划。
pub fn build_execution_plan(input: ExecutionPlanBuildInput<'_>) -> ExecutionResult<ExecutionPlan> {
    match StaticExecutionPlanner::default().build(input)? {
        PlanBuildOutcome::Schedulable(plan) => Ok(plan),
        PlanBuildOutcome::PendingManualApproval(pending) => {
            Err(ExecutionError::PendingManualApproval {
                plan_id: pending.plan_preview.plan_id.as_str().to_owned(),
            })
        }
    }
}

/// 构建计划或人工审批预览。
pub fn build_execution_plan_preview(
    input: ExecutionPlanBuildInput<'_>,
) -> ExecutionResult<PlanBuildOutcome> {
    StaticExecutionPlanner::default().build(input)
}

/// 用默认脚本模拟执行计划。
pub fn simulate_execution(
    plan: &ExecutionPlan,
    generated_at: &str,
) -> ExecutionResult<ExecutionReport> {
    Ok(
        simulate_execution_with_script(SimulatedExecutionInput::new(plan, generated_at, &[]))?
            .report,
    )
}

/// 用显式脚本模拟执行计划。
pub fn simulate_execution_with_script(
    input: SimulatedExecutionInput<'_>,
) -> ExecutionResult<SimulatedExecution> {
    StaticSimulatedExecutor.simulate(input)
}

/// 用私有订单确认生成执行报告。
///
/// 中文说明：该入口不消费 REST 下单回执。调用方必须先用私有 user data stream
/// 或查单结果证明订单状态；缺失确认、确认未终态或成交明细不足都会按未知状态
/// 或人工处理输出，不能被汇总成最终成功。
pub fn execution_report_from_private_confirmations(
    input: PrivateExecutionReportInput<'_>,
) -> ExecutionResult<ExecutionReport> {
    build_private_execution_report(input)
}

/// 将模拟执行结果转换为模拟账本分录输入。
///
/// 中文说明：执行模块只生成 `LedgerEntry` 合同对象，调用方可以把这些对象交给
/// `arb-ledger` 追加；这里不直接写账本，也不会写入实盘命名空间。
pub fn simulated_ledger_entries_from_execution_report(
    plan: &ExecutionPlan,
    report: &ExecutionReport,
) -> ExecutionResult<Vec<LedgerEntry>> {
    ledger_entries_from_execution_report(plan, report, LedgerNamespace::Simulation)
}

/// 将私有确认后的执行报告转换为实盘账本分录输入。
///
/// 中文说明：只有已经进入 `ExecutionReport.fills` 的私有流/查单确认成交会生成
/// `Live`（实盘）命名空间分录；未知订单状态不会凭 REST 回执入账。
pub fn private_ledger_entries_from_execution_report(
    plan: &ExecutionPlan,
    report: &ExecutionReport,
) -> ExecutionResult<Vec<LedgerEntry>> {
    ledger_entries_from_execution_report(plan, report, LedgerNamespace::Live)
}

/// 从私有确认后的失败或未知执行报告生成事故记录。
pub fn incidents_from_private_execution_report(
    plan: &ExecutionPlan,
    report: &ExecutionReport,
    opened_at: &str,
) -> ExecutionResult<Vec<Incident>> {
    if report.failures.is_empty()
        && matches!(
            &report.status,
            ExecutionReportStatus::Succeeded | ExecutionReportStatus::Simulated
        )
    {
        return Ok(Vec::new());
    }

    let source_event_refs = private_execution_incident_source_refs(report);
    let venue_ids = private_execution_incident_venues(plan);
    let trigger = private_execution_incident_trigger(report);
    let severity = private_execution_incident_severity(report);
    let incident_id = format!("incident:{}:private-execution", report.report_id.as_str());
    let detail = private_execution_incident_detail(report);

    let incident_json = format!(
        "{{\"automatic_actions\":[{{\"action_id\":{},\"action_type\":\"TradingPaused\",\"detail\":{},\"timestamp\":{} }},{{\"action_id\":{},\"action_type\":\"ReconciliationStarted\",\"detail\":{},\"timestamp\":{} }}],\"impacted\":{{\"capital_at_risk_usd\":{},\"venue_ids\":[{}]}},\"incident_id\":{},\"manual_actions\":[{{\"action_id\":{},\"action_type\":\"ManualReview\",\"detail\":{},\"timestamp\":{} }}],\"opened_at\":{},\"schema_version\":{},\"severity\":{},\"source_event_refs\":[{}],\"status\":\"Open\",\"trigger\":{}}}",
        json_string(&format!("iact:{}:pause", report.report_id.as_str())),
        json_string("Private execution confirmation failed closed; trading must remain paused for affected scope."),
        json_string(opened_at),
        json_string(&format!("iact:{}:reconcile", report.report_id.as_str())),
        json_string("Start reconciliation from private stream, order query, fills and ledger entries before any retry."),
        json_string(opened_at),
        json_string(private_execution_capital_at_risk(plan)),
        venue_ids
            .iter()
            .map(|venue_id| json_string(venue_id))
            .collect::<Vec<_>>()
            .join(","),
        json_string(&incident_id),
        json_string(&format!("iact:{}:manual", report.report_id.as_str())),
        json_string(&detail),
        json_string(opened_at),
        json_string(opened_at),
        json_string(report.schema_version.as_str()),
        json_string(severity),
        source_event_refs
            .iter()
            .map(|event_id| json_string(event_id))
            .collect::<Vec<_>>()
            .join(","),
        json_string(trigger),
    );
    Ok(vec![from_json_strict::<Incident>(&incident_json)?])
}

fn ledger_entries_from_execution_report(
    plan: &ExecutionPlan,
    report: &ExecutionReport,
    namespace: LedgerNamespace,
) -> ExecutionResult<Vec<LedgerEntry>> {
    if plan.plan_id != report.plan_id {
        return Err(ExecutionError::ExecutionReportPlanMismatch {
            plan_id: plan.plan_id.as_str().to_owned(),
            report_plan_id: report.plan_id.as_str().to_owned(),
        });
    }

    if report.status == ExecutionReportStatus::NotDispatched {
        return Ok(Vec::new());
    }

    report
        .fills
        .iter()
        .map(|fill| render_ledger_entry(plan, report, fill, &namespace))
        .collect()
}

/// 执行计划构建结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanBuildOutcome {
    Schedulable(ExecutionPlan),
    PendingManualApproval(PendingManualApprovalPlan),
}

impl PlanBuildOutcome {
    pub fn dispatchable_plan(&self) -> ExecutionResult<&ExecutionPlan> {
        match self {
            Self::Schedulable(plan) => Ok(plan),
            Self::PendingManualApproval(pending) => Err(ExecutionError::PendingManualApproval {
                plan_id: pending.plan_preview.plan_id.as_str().to_owned(),
            }),
        }
    }
}

/// 人工审批前的计划预览。
///
/// 中文说明：预览用于审查计划哈希、执行腿、依赖和风控来源；审批前
/// `is_dispatchable` 永远为 false，不能分发执行。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingManualApprovalPlan {
    pub plan_preview: ExecutionPlan,
    pub approval_material: ManualApprovalMaterial,
}

impl PendingManualApprovalPlan {
    pub fn is_dispatchable(&self) -> bool {
        false
    }

    pub fn dispatchable_plan(&self) -> ExecutionResult<&ExecutionPlan> {
        Err(ExecutionError::PendingManualApproval {
            plan_id: self.plan_preview.plan_id.as_str().to_owned(),
        })
    }
}

/// 人工审批材料。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualApprovalMaterial {
    pub risk_decision_id: String,
    pub transition_id: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub reason_codes: Vec<String>,
    pub approval_requirement: String,
}

/// 人工审批动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManualApprovalDecision {
    Approve,
    Reject,
}

impl ManualApprovalDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "Approve",
            Self::Reject => "Reject",
        }
    }
}

/// 人工审批记录状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManualApprovalStatus {
    Approved,
    Rejected,
    Expired,
    Duplicate,
}

impl ManualApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "Approved",
            Self::Rejected => "Rejected",
            Self::Expired => "Expired",
            Self::Duplicate => "Duplicate",
        }
    }
}

/// 人工审批审核输入。
///
/// 中文说明：审核只能针对同一个 `PendingManualApprovalPlan` 和同一个计划哈希。
/// 这里不触发下单、签名或账本写入；批准只产生可审计记录和后续门禁释放依据。
#[derive(Clone, Copy)]
pub struct ManualApprovalReviewInput<'a> {
    pending: &'a PendingManualApprovalPlan,
    approval_event_id: &'a str,
    reviewer_id: &'a str,
    decided_at: &'a str,
    expires_at: &'a str,
    decision: ManualApprovalDecision,
    reason: Option<&'a str>,
    prior_records: &'a [ManualApprovalRecord],
}

impl<'a> ManualApprovalReviewInput<'a> {
    pub fn new(
        pending: &'a PendingManualApprovalPlan,
        approval_event_id: &'a str,
        reviewer_id: &'a str,
        decided_at: &'a str,
        expires_at: &'a str,
        decision: ManualApprovalDecision,
    ) -> Self {
        Self {
            pending,
            approval_event_id,
            reviewer_id,
            decided_at,
            expires_at,
            decision,
            reason: None,
            prior_records: &[],
        }
    }

    pub fn with_reason(mut self, reason: &'a str) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn with_prior_records(mut self, prior_records: &'a [ManualApprovalRecord]) -> Self {
        self.prior_records = prior_records;
        self
    }
}

/// 人工审批审计记录。
///
/// 中文说明：记录是可回放事实。`releases_manual_gate` 为 true 也只表示人工门禁
/// 可被释放；后续仍必须走执行模式、资本预留、账本、对账和熔断检查。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualApprovalRecord {
    pub record_id: String,
    pub approval_event_id: String,
    pub risk_decision_id: String,
    pub transition_id: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub decision: ManualApprovalDecision,
    pub status: ManualApprovalStatus,
    pub reviewer_id: String,
    pub decided_at: String,
    pub expires_at: String,
    pub reason: Option<String>,
    pub duplicate_of: Option<String>,
    pub releases_manual_gate: bool,
    pub controlled_next_step: String,
}

impl ManualApprovalRecord {
    pub fn to_audit_json(&self) -> String {
        let mut fields = vec![
            render_pair("approval_event_id", json_string(&self.approval_event_id)),
            render_pair(
                "controlled_next_step",
                json_string(&self.controlled_next_step),
            ),
            render_pair("decided_at", json_string(&self.decided_at)),
            render_pair("decision", json_string(self.decision.as_str())),
        ];
        if let Some(duplicate_of) = &self.duplicate_of {
            fields.push(render_pair("duplicate_of", json_string(duplicate_of)));
        }
        fields.extend([
            render_pair("expires_at", json_string(&self.expires_at)),
            render_pair("plan_hash", json_string(&self.plan_hash)),
            render_pair("plan_id", json_string(&self.plan_id)),
            render_pair("record_id", json_string(&self.record_id)),
        ]);
        if let Some(reason) = &self.reason {
            fields.push(render_pair("reason", json_string(reason)));
        }
        fields.extend([
            render_pair(
                "releases_manual_gate",
                self.releases_manual_gate.to_string(),
            ),
            render_pair("reviewer_id", json_string(&self.reviewer_id)),
            render_pair("risk_decision_id", json_string(&self.risk_decision_id)),
            render_pair("status", json_string(self.status.as_str())),
            render_pair("transition_id", json_string(&self.transition_id)),
        ]);
        format!("{{{}}}", fields.join(","))
    }
}

/// 人工审批门禁释放结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualApprovalGateRelease {
    pub plan_id: String,
    pub plan_hash: String,
    pub approval_event_id: String,
    pub gate_transition: ExecutionLegTransition,
    pub dependent_transitions: Vec<ExecutionLegTransition>,
    pub controlled_next_step: String,
}

/// 计算执行计划规范哈希。
///
/// 中文说明：人工审批必须引用这个哈希；审批后如果计划内容变化，哈希会变化，
/// 旧审批不能继续沿用。
pub fn execution_plan_hash(plan: &ExecutionPlan) -> String {
    stable_plan_hash(&to_canonical_json(plan))
}

/// 生成人工审批审计记录。
pub fn review_manual_approval(
    input: ManualApprovalReviewInput<'_>,
) -> ExecutionResult<ManualApprovalRecord> {
    validate_manual_approval_review_input(&input)?;
    validate_pending_manual_material(input.pending)?;

    let plan_hash = execution_plan_hash(&input.pending.plan_preview);
    if plan_hash != input.pending.approval_material.plan_hash {
        return Err(ExecutionError::ManualApprovalPlanHashMismatch {
            expected: input.pending.approval_material.plan_hash.clone(),
            actual: plan_hash,
        });
    }

    if let Some(existing) = input.prior_records.iter().find(|record| {
        record.plan_hash == plan_hash && record.status != ManualApprovalStatus::Duplicate
    }) {
        return Ok(manual_approval_record_from_input(
            input,
            plan_hash,
            ManualApprovalStatus::Duplicate,
            false,
            Some(existing.record_id.clone()),
        ));
    }

    let status = if timestamp_after(input.decided_at, input.expires_at) {
        ManualApprovalStatus::Expired
    } else {
        match input.decision {
            ManualApprovalDecision::Approve => ManualApprovalStatus::Approved,
            ManualApprovalDecision::Reject => ManualApprovalStatus::Rejected,
        }
    };
    let releases_manual_gate = status == ManualApprovalStatus::Approved;
    Ok(manual_approval_record_from_input(
        input,
        plan_hash,
        status,
        releases_manual_gate,
        None,
    ))
}

/// 根据已批准记录释放人工审批门禁。
///
/// 中文说明：该函数只生成状态迁移事实，不分发订单、不签名、不写账本。调用方
/// 仍需把这些事实交给事件存储，并继续执行模式、资本预留、账本和对账流程。
pub fn release_manual_approval_gate(
    pending: &PendingManualApprovalPlan,
    record: &ManualApprovalRecord,
) -> ExecutionResult<ManualApprovalGateRelease> {
    validate_pending_manual_material(pending)?;
    validate_manual_approval_record_matches(pending, record)?;
    if record.status != ManualApprovalStatus::Approved || !record.releases_manual_gate {
        return Err(ExecutionError::ManualApprovalNotReleased {
            plan_id: pending.plan_preview.plan_id.as_str().to_owned(),
            status: record.status,
        });
    }

    let gate = pending
        .plan_preview
        .legs
        .iter()
        .find(|leg| leg.action_type == ExecutionActionType::ManualApprovalGate)
        .ok_or_else(|| ExecutionError::MissingRequiredField {
            field: "execution_plan.legs[].ManualApprovalGate",
            detail: "pending manual plan has no manual approval gate leg".to_owned(),
        })?;

    let gate_transition = transition_execution_leg(
        gate.plan_leg_id.as_str(),
        &gate.state,
        ExecutionLegState::Ready,
        &record.approval_event_id,
    )?;
    let dependent_transitions = pending
        .plan_preview
        .legs
        .iter()
        .filter(|leg| {
            leg.depends_on.as_ref().is_some_and(|dependencies| {
                dependencies
                    .iter()
                    .any(|dependency| dependency.as_str() == gate.plan_leg_id.as_str())
            })
        })
        .map(|leg| {
            transition_execution_leg(
                leg.plan_leg_id.as_str(),
                &leg.state,
                ExecutionLegState::Ready,
                &record.approval_event_id,
            )
        })
        .collect::<ExecutionResult<Vec<_>>>()?;

    Ok(ManualApprovalGateRelease {
        plan_id: pending.plan_preview.plan_id.as_str().to_owned(),
        plan_hash: record.plan_hash.clone(),
        approval_event_id: record.approval_event_id.clone(),
        gate_transition,
        dependent_transitions,
        controlled_next_step: record.controlled_next_step.clone(),
    })
}

fn validate_manual_approval_review_input(
    input: &ManualApprovalReviewInput<'_>,
) -> ExecutionResult<()> {
    require_non_empty_execution_field(
        "manual_approval.approval_event_id",
        input.approval_event_id,
        "manual approval must reference an ApprovalEvent",
    )?;
    require_non_empty_execution_field(
        "manual_approval.reviewer_id",
        input.reviewer_id,
        "manual approval must identify the reviewer or approval group",
    )?;
    require_non_empty_execution_field(
        "manual_approval.decided_at",
        input.decided_at,
        "manual approval decision time must be explicit",
    )?;
    require_non_empty_execution_field(
        "manual_approval.expires_at",
        input.expires_at,
        "manual approval expiry time must be explicit",
    )?;
    Ok(())
}

fn require_non_empty_execution_field(
    field: &'static str,
    value: &str,
    detail: &str,
) -> ExecutionResult<()> {
    if value.trim().is_empty() {
        Err(ExecutionError::MissingRequiredField {
            field,
            detail: detail.to_owned(),
        })
    } else {
        Ok(())
    }
}

fn validate_pending_manual_material(pending: &PendingManualApprovalPlan) -> ExecutionResult<()> {
    let material = &pending.approval_material;
    compare_manual_approval_field(
        "plan_id",
        pending.plan_preview.plan_id.as_str(),
        &material.plan_id,
    )?;
    compare_manual_approval_field(
        "risk_decision_id",
        pending.plan_preview.risk_decision_id.as_str(),
        &material.risk_decision_id,
    )?;
    compare_manual_approval_field(
        "transition_id",
        pending.plan_preview.transition_id.as_str(),
        &material.transition_id,
    )?;
    let actual_plan_hash = execution_plan_hash(&pending.plan_preview);
    if material.plan_hash != actual_plan_hash {
        return Err(ExecutionError::ManualApprovalPlanHashMismatch {
            expected: material.plan_hash.clone(),
            actual: actual_plan_hash,
        });
    }
    Ok(())
}

fn validate_manual_approval_record_matches(
    pending: &PendingManualApprovalPlan,
    record: &ManualApprovalRecord,
) -> ExecutionResult<()> {
    let material = &pending.approval_material;
    compare_manual_approval_field("plan_id", &material.plan_id, &record.plan_id)?;
    compare_manual_approval_field(
        "risk_decision_id",
        &material.risk_decision_id,
        &record.risk_decision_id,
    )?;
    compare_manual_approval_field(
        "transition_id",
        &material.transition_id,
        &record.transition_id,
    )?;
    compare_manual_approval_field("plan_hash", &material.plan_hash, &record.plan_hash)?;
    require_non_empty_execution_field(
        "manual_approval_record.approval_event_id",
        &record.approval_event_id,
        "approved manual approval records must reference an ApprovalEvent",
    )?;
    Ok(())
}

fn compare_manual_approval_field(
    field: &'static str,
    expected: &str,
    actual: &str,
) -> ExecutionResult<()> {
    if expected != actual {
        Err(ExecutionError::ManualApprovalRecordMismatch {
            field,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    } else {
        Ok(())
    }
}

fn manual_approval_record_from_input(
    input: ManualApprovalReviewInput<'_>,
    plan_hash: String,
    status: ManualApprovalStatus,
    releases_manual_gate: bool,
    duplicate_of: Option<String>,
) -> ManualApprovalRecord {
    let material = &input.pending.approval_material;
    ManualApprovalRecord {
        record_id: format!(
            "approval-record:{}:{}",
            material.plan_id, input.approval_event_id
        ),
        approval_event_id: input.approval_event_id.to_owned(),
        risk_decision_id: material.risk_decision_id.clone(),
        transition_id: material.transition_id.clone(),
        plan_id: material.plan_id.clone(),
        plan_hash,
        decision: input.decision,
        status,
        reviewer_id: input.reviewer_id.to_owned(),
        decided_at: input.decided_at.to_owned(),
        expires_at: input.expires_at.to_owned(),
        reason: input.reason.map(str::to_owned),
        duplicate_of,
        releases_manual_gate,
        controlled_next_step: manual_approval_controlled_next_step(status).to_owned(),
    }
}

fn manual_approval_controlled_next_step(status: ManualApprovalStatus) -> &'static str {
    match status {
        ManualApprovalStatus::Approved => {
            "Release manual gate only; continue through execution mode, capital reservation, ledger, reconciliation, kill switch, and permission checks."
        }
        ManualApprovalStatus::Rejected => {
            "Stop before dispatch; record the manual rejection as an audit fact."
        }
        ManualApprovalStatus::Expired => {
            "Stop before dispatch; rebuild approval material if the opportunity is still valid."
        }
        ManualApprovalStatus::Duplicate => {
            "Ignore duplicate approval attempt; keep the first terminal approval record authoritative."
        }
    }
}

fn timestamp_after(left: &str, right: &str) -> bool {
    left > right
}

fn validate_transition_link(input: &ExecutionPlanBuildInput<'_>) -> ExecutionResult<()> {
    let risk_transition_id = input.risk_decision.transition_id.as_str();
    let candidate_transition_id = input.candidate.transition_id.as_str();
    if risk_transition_id != candidate_transition_id {
        return Err(ExecutionError::TransitionMismatch {
            risk_transition_id: risk_transition_id.to_owned(),
            candidate_transition_id: candidate_transition_id.to_owned(),
        });
    }
    Ok(())
}

fn transition_capital_reservation_to(
    reservation: &CapitalReservation,
    to_state: CapitalReservationState,
    refs: CapitalReservationRefs<'_>,
) -> ExecutionResult<CapitalReservationTransition> {
    validate_required_reservation_refs(refs)?;
    validate_existing_reservation_refs(reservation)?;

    let from_state = reservation.state.clone();
    if !is_allowed_capital_reservation_transition(&from_state, &to_state) {
        return Err(ExecutionError::InvalidCapitalReservationTransition {
            reservation_id: reservation.reservation_id.as_str().to_owned(),
            from: from_state,
            to: to_state,
        });
    }

    let validated_next = capital_reservation_from_parts(CapitalReservationParts {
        reservation_id: reservation.reservation_id.as_str(),
        state: to_state.clone(),
        asset_id: reservation.asset_id.as_str(),
        amount: reservation.amount.as_str(),
        reserved_for: reservation.reserved_for.as_str(),
        expires_at: reservation.expires_at.as_str(),
        refs,
    })?;
    let idempotent = from_state == to_state;
    let next = if idempotent {
        reservation.clone()
    } else {
        validated_next
    };

    Ok(CapitalReservationTransition {
        reservation: next,
        from_state: Some(from_state),
        to_state,
        source_event_id: refs.source_event_id.to_owned(),
        ledger_entry_id: refs.ledger_entry_id.to_owned(),
        idempotent,
    })
}

fn is_allowed_capital_reservation_transition(
    from: &CapitalReservationState,
    to: &CapitalReservationState,
) -> bool {
    matches!(
        (from, to),
        (
            CapitalReservationState::Requested,
            CapitalReservationState::Reserved
        ) | (
            CapitalReservationState::Reserved,
            CapitalReservationState::ConvertedToExecution
        ) | (
            CapitalReservationState::Requested,
            CapitalReservationState::Released
        ) | (
            CapitalReservationState::Reserved,
            CapitalReservationState::Released
        ) | (
            CapitalReservationState::Released,
            CapitalReservationState::Released
        ) | (
            CapitalReservationState::Requested,
            CapitalReservationState::Expired
        ) | (
            CapitalReservationState::Reserved,
            CapitalReservationState::Expired
        ) | (
            CapitalReservationState::Expired,
            CapitalReservationState::Expired
        ) | (
            CapitalReservationState::Requested,
            CapitalReservationState::ReconciledMismatch
        ) | (
            CapitalReservationState::Reserved,
            CapitalReservationState::ReconciledMismatch
        ) | (
            CapitalReservationState::ConvertedToExecution,
            CapitalReservationState::ReconciledMismatch
        ) | (
            CapitalReservationState::Released,
            CapitalReservationState::ReconciledMismatch
        ) | (
            CapitalReservationState::Expired,
            CapitalReservationState::ReconciledMismatch
        ) | (
            CapitalReservationState::ReconciledMismatch,
            CapitalReservationState::ReconciledMismatch
        )
    )
}

fn is_allowed_execution_leg_transition(from: &ExecutionLegState, to: &ExecutionLegState) -> bool {
    matches!(
        (from, to),
        (ExecutionLegState::Prepared, ExecutionLegState::Ready)
            | (
                ExecutionLegState::Prepared,
                ExecutionLegState::WaitingDependency
            )
            | (
                ExecutionLegState::WaitingDependency,
                ExecutionLegState::Ready
            )
            | (ExecutionLegState::Ready, ExecutionLegState::Dispatched)
            | (
                ExecutionLegState::Dispatched,
                ExecutionLegState::Acknowledged
            )
            | (ExecutionLegState::Acknowledged, ExecutionLegState::Filled)
            | (
                ExecutionLegState::Acknowledged,
                ExecutionLegState::PartiallyFilled
            )
            | (
                ExecutionLegState::PartiallyFilled,
                ExecutionLegState::Filled
            )
            | (
                ExecutionLegState::PartiallyFilled,
                ExecutionLegState::CancelRequested
            )
            | (
                ExecutionLegState::CancelRequested,
                ExecutionLegState::Cancelled
            )
            | (ExecutionLegState::Dispatched, ExecutionLegState::Failed)
            | (ExecutionLegState::Acknowledged, ExecutionLegState::Failed)
            | (
                ExecutionLegState::PartiallyFilled,
                ExecutionLegState::Failed
            )
            | (
                ExecutionLegState::CancelRequested,
                ExecutionLegState::Failed
            )
            | (ExecutionLegState::Dispatched, ExecutionLegState::Unknown)
            | (ExecutionLegState::Acknowledged, ExecutionLegState::Unknown)
            | (
                ExecutionLegState::PartiallyFilled,
                ExecutionLegState::Unknown
            )
            | (
                ExecutionLegState::CancelRequested,
                ExecutionLegState::Unknown
            )
            | (ExecutionLegState::Unknown, ExecutionLegState::Compensating)
            | (
                ExecutionLegState::Compensating,
                ExecutionLegState::Compensated
            )
    )
}

fn validate_required_reservation_refs(refs: CapitalReservationRefs<'_>) -> ExecutionResult<()> {
    if refs.source_event_id.trim().is_empty() {
        return Err(ExecutionError::MissingRequiredField {
            field: "capital_reservation.source_event_id",
            detail: "capital reservation transitions must reference a persisted event".to_owned(),
        });
    }
    if refs.ledger_entry_id.trim().is_empty() {
        return Err(ExecutionError::MissingRequiredField {
            field: "capital_reservation.ledger_entry_id",
            detail: "capital reservation transitions must reference an append-only ledger entry"
                .to_owned(),
        });
    }
    Ok(())
}

fn validate_existing_reservation_refs(reservation: &CapitalReservation) -> ExecutionResult<()> {
    if reservation.source_event_id.is_none() {
        return Err(ExecutionError::MissingRequiredField {
            field: "capital_reservation.source_event_id",
            detail: format!(
                "existing reservation `{}` has no source event reference",
                reservation.reservation_id.as_str()
            ),
        });
    }
    if reservation.ledger_entry_id.is_none() {
        return Err(ExecutionError::MissingRequiredField {
            field: "capital_reservation.ledger_entry_id",
            detail: format!(
                "existing reservation `{}` has no ledger entry reference",
                reservation.reservation_id.as_str()
            ),
        });
    }
    Ok(())
}

struct CapitalReservationParts<'a> {
    reservation_id: &'a str,
    state: CapitalReservationState,
    asset_id: &'a str,
    amount: &'a str,
    reserved_for: &'a str,
    expires_at: &'a str,
    refs: CapitalReservationRefs<'a>,
}

fn capital_reservation_from_parts(
    parts: CapitalReservationParts<'_>,
) -> ExecutionResult<CapitalReservation> {
    let reservation_json = format!(
        "{{\"reservation_id\":{},\"state\":{},\"asset_id\":{},\"amount\":{},\"reserved_for\":{},\"expires_at\":{},\"source_event_id\":{},\"ledger_entry_id\":{}}}",
        json_string(parts.reservation_id),
        json_string(parts.state.as_str()),
        json_string(parts.asset_id),
        json_string(parts.amount),
        json_string(parts.reserved_for),
        json_string(parts.expires_at),
        json_string(parts.refs.source_event_id),
        json_string(parts.refs.ledger_entry_id),
    );
    let portfolio_json = format!(
        "{{\"schema_version\":\"1.0.0\",\"portfolio_state_id\":\"state:capital_reservation_validator\",\"as_of\":{},\"source_event_refs\":[{}],\"balances\":[],\"positions\":[],\"reservations\":[{}],\"open_orders\":[],\"pending_transfers\":[],\"confidence\":1,\"missing_data_flags\":[],\"state_hash\":\"hash:capital-reservation-validator\"}}",
        json_string(parts.expires_at),
        json_string(parts.refs.source_event_id),
        reservation_json,
    );
    let mut portfolio = from_json_strict::<PortfolioState>(&portfolio_json)?;
    portfolio
        .reservations
        .pop()
        .ok_or_else(|| ExecutionError::MissingRequiredField {
            field: "portfolio_state.reservations",
            detail: "capital reservation validator produced no reservation".to_owned(),
        })
}

fn build_pending_manual_plan(
    input: &ExecutionPlanBuildInput<'_>,
    policy: &ExecutionPlanPolicy,
) -> ExecutionResult<PendingManualApprovalPlan> {
    let plan_preview = build_plan_contract(input, policy, true)?;
    let canonical_plan = to_canonical_json(&plan_preview);
    let approval_material = ManualApprovalMaterial {
        risk_decision_id: input.risk_decision.decision_id.as_str().to_owned(),
        transition_id: input.risk_decision.transition_id.as_str().to_owned(),
        plan_id: plan_preview.plan_id.as_str().to_owned(),
        plan_hash: stable_plan_hash(&canonical_plan),
        reason_codes: input
            .risk_decision
            .reason_codes
            .iter()
            .map(|code| code.as_str().to_owned())
            .collect(),
        approval_requirement:
            "人工审批必须引用同一个 plan_hash；审批不能替代风控、账本、熔断或执行权限。".to_owned(),
    };
    Ok(PendingManualApprovalPlan {
        plan_preview,
        approval_material,
    })
}

fn build_plan_contract(
    input: &ExecutionPlanBuildInput<'_>,
    policy: &ExecutionPlanPolicy,
    pending_manual_approval: bool,
) -> ExecutionResult<ExecutionPlan> {
    if input.created_at.trim().is_empty() {
        return Err(ExecutionError::MissingRequiredField {
            field: "created_at",
            detail: "execution plan creation time must be explicit".to_owned(),
        });
    }

    validate_candidate_legs(input.candidate)?;

    let plan_id = plan_id(input.risk_decision);
    let mut leg_json = Vec::new();
    let mut edges = Vec::new();
    let mut previous_plan_leg_id: Option<String> = None;
    let manual_gate_id = if pending_manual_approval {
        let gate_id = bounded_identifier("pleg", &format!("{plan_id}:manual-gate"));
        leg_json.push(render_manual_gate_leg(input, &gate_id));
        Some(gate_id)
    } else {
        None
    };

    for (index, leg) in input.candidate.legs.iter().enumerate() {
        let plan_leg_id = bounded_identifier("pleg", &format!("{plan_id}:{:04}", index + 1));
        let mut dependencies = Vec::new();
        let mut initial_state = if index == 0 {
            ExecutionLegState::Ready
        } else {
            ExecutionLegState::WaitingDependency
        };

        if let Some(gate_id) = &manual_gate_id {
            if index == 0 {
                dependencies.push(gate_id.clone());
                initial_state = ExecutionLegState::WaitingDependency;
                edges.push(render_dependency_edge(
                    gate_id,
                    &plan_leg_id,
                    "ManualRelease",
                ));
            }
        }

        if let Some(previous_id) = &previous_plan_leg_id {
            dependencies.push(previous_id.clone());
            initial_state = ExecutionLegState::WaitingDependency;
            edges.push(render_dependency_edge(
                previous_id,
                &plan_leg_id,
                "OnSuccess",
            ));
        }

        leg_json.push(render_execution_leg(ExecutionLegRenderInput {
            execution_mode: input.execution_mode(),
            candidate: input.candidate,
            leg,
            plan_id: &plan_id,
            plan_leg_id: &plan_leg_id,
            dependencies: &dependencies,
            initial_state: &initial_state,
            pending_manual_approval,
        })?);
        previous_plan_leg_id = Some(plan_leg_id);
    }

    let constraints = render_execution_constraints(input.risk_decision, input.candidate);
    let cancel_action = cancel_action(input.candidate);
    let hedge_action = hedge_action(input.risk_decision, input.candidate);
    let partial_fill_action = partial_fill_action(input.candidate);
    let mode = if pending_manual_approval {
        "ManualApproval"
    } else {
        input.execution_mode.as_str()
    };

    let plan_json = format!(
        "{{\"schema_version\":{},\"plan_id\":{},\"transition_id\":{},\"risk_decision_id\":{},\"created_at\":{},\"execution_mode\":{},\"idempotency_key\":{},\"legs\":[{}],\"dependency_graph\":{{\"edges\":[{}]}},\"constraints\":{},\"timeout_policy\":{{\"plan_timeout_ms\":{},\"leg_timeout_ms\":{},\"unknown_state_after_ms\":{}}},\"cancel_policy\":{{\"default_action\":{}}},\"hedge_policy\":{},\"partial_fill_policy\":{},\"failure_policy\":{}}}",
        json_string(input.candidate.schema_version.as_str()),
        json_string(&plan_id),
        json_string(input.risk_decision.transition_id.as_str()),
        json_string(input.risk_decision.decision_id.as_str()),
        json_string(input.created_at),
        json_string(mode),
        json_string(&bounded_identifier("idem", &plan_id)),
        leg_json.join(","),
        edges.join(","),
        constraints,
        policy.plan_timeout_ms,
        policy.leg_timeout_ms,
        policy.unknown_state_after_ms,
        json_string(cancel_action.as_str()),
        render_hedge_policy(hedge_action, input.candidate),
        render_partial_fill_policy(partial_fill_action, input.candidate),
        render_failure_policy(policy),
    );

    Ok(from_json_strict::<ExecutionPlan>(&plan_json)?)
}

fn validate_candidate_legs(candidate: &CandidatePortfolioTransition) -> ExecutionResult<()> {
    for (index, leg) in candidate.legs.iter().enumerate() {
        if leg.account_id.is_none() {
            return Err(ExecutionError::MissingRequiredField {
                field: "candidate.legs[].account_id",
                detail: format!(
                    "candidate leg `{}` at index {index} has no account_id, but ExecutionPlan legs require one",
                    leg.leg_id.as_str()
                ),
            });
        }
        if requires_venue(&leg.leg_type) && leg.venue_id.is_none() {
            return Err(ExecutionError::MissingRequiredField {
                field: "candidate.legs[].venue_id",
                detail: format!(
                    "candidate leg `{}` at index {index} requires venue_id for execution planning",
                    leg.leg_id.as_str()
                ),
            });
        }
        if requires_instrument(&leg.leg_type) && leg.instrument_id.is_none() {
            return Err(ExecutionError::MissingRequiredField {
                field: "candidate.legs[].instrument_id",
                detail: format!(
                    "candidate leg `{}` at index {index} requires instrument_id for execution planning",
                    leg.leg_id.as_str()
                ),
            });
        }
    }
    Ok(())
}

fn render_manual_gate_leg(input: &ExecutionPlanBuildInput<'_>, gate_id: &str) -> String {
    let account_id = input
        .candidate
        .legs
        .iter()
        .find_map(|leg| leg.account_id.as_ref())
        .map(|id| id.as_str())
        .unwrap_or("acct:manual-approval");
    format!(
        "{{\"plan_leg_id\":{},\"candidate_leg_id\":{},\"action_type\":{},\"account_id\":{},\"idempotency_key\":{},\"state\":{},\"failure_semantics\":{}}}",
        json_string(gate_id),
        json_string(&format!(
            "manual-gate:{}",
            input.risk_decision.decision_id.as_str()
        )),
        json_string(ExecutionActionType::ManualApprovalGate.as_str()),
        json_string(account_id),
        json_string(&bounded_identifier("idem", gate_id)),
        json_string(ExecutionLegState::Prepared.as_str()),
        json_string(FailureMode::ManualInterventionRequired.as_str()),
    )
}

struct ExecutionLegRenderInput<'a> {
    execution_mode: &'a ExecutionMode,
    candidate: &'a CandidatePortfolioTransition,
    leg: &'a TransitionLeg,
    plan_id: &'a str,
    plan_leg_id: &'a str,
    dependencies: &'a [String],
    initial_state: &'a ExecutionLegState,
    pending_manual_approval: bool,
}

fn render_execution_leg(input: ExecutionLegRenderInput<'_>) -> ExecutionResult<String> {
    let leg = input.leg;
    let action_type = if input.pending_manual_approval {
        intended_action_type(&leg.leg_type)
    } else {
        scheduled_action_type(input.execution_mode, &leg.leg_type)
    };
    let mut fields = vec![
        render_pair("plan_leg_id", json_string(input.plan_leg_id)),
        render_pair("candidate_leg_id", json_string(leg.leg_id.as_str())),
        render_pair("action_type", json_string(action_type.as_str())),
    ];
    if let Some(venue_id) = &leg.venue_id {
        fields.push(render_pair("venue_id", json_string(venue_id.as_str())));
    }
    if let Some(instrument_id) = &leg.instrument_id {
        fields.push(render_pair(
            "instrument_id",
            json_string(instrument_id.as_str()),
        ));
    }
    fields.push(render_pair(
        "account_id",
        json_string(leg.account_id.as_ref().expect("validated").as_str()),
    ));
    if let Some(venue_symbol) = execution_venue_symbol(leg) {
        fields.push(render_pair("venue_symbol", json_string(&venue_symbol)));
    }
    if let Some(side) = &leg.side {
        fields.push(render_pair("side", json_string(side.as_str())));
    }
    if requires_order_intent(&action_type) {
        if let Some(order_type) = execution_order_type(leg) {
            fields.push(render_pair("order_type", json_string(order_type.as_str())));
        }
        if let Some(time_in_force) = execution_time_in_force(leg)? {
            fields.push(render_pair(
                "time_in_force",
                json_string(time_in_force.as_str()),
            ));
        }
        if bool_constraint(leg, "reduce_only") == Some(true) {
            fields.push(render_pair("reduce_only", "true".to_owned()));
        }
        if let Some(quantity) = execution_quantity(input.candidate, leg) {
            fields.push(render_pair("quantity", json_string(&quantity)));
        }
        if let Some(limit_price) = execution_limit_price(leg) {
            fields.push(render_pair("limit_price", json_string(&limit_price)));
        }
        if let Some(notional) = execution_notional_usd(leg) {
            fields.push(render_pair("notional_usd", json_string(&notional)));
        }
        if let Some(client_order_id) = string_constraint(leg, "client_order_id") {
            fields.push(render_pair(
                "client_order_id",
                json_string(&client_order_id),
            ));
        }
    }
    if let Some(role) = string_constraint(leg, "basis_leg_role") {
        fields.push(render_pair("basis_leg_role", json_string(&role)));
    }
    fields.push(render_pair(
        "idempotency_key",
        json_string(&bounded_identifier(
            "idem",
            &format!("{}:{}", input.plan_id, leg.leg_id.as_str()),
        )),
    ));
    if !input.dependencies.is_empty() {
        fields.push(render_pair(
            "depends_on",
            format!(
                "[{}]",
                input
                    .dependencies
                    .iter()
                    .map(|dependency| json_string(dependency))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ));
    }
    if !leg.asset_flows.is_empty() {
        fields.push(render_pair(
            "expected_asset_flows",
            to_canonical_json(&leg.asset_flows),
        ));
    }
    fields.push(render_pair(
        "state",
        json_string(input.initial_state.as_str()),
    ));
    fields.push(render_pair(
        "failure_semantics",
        json_string(selected_failure_mode(&leg.failure_modes).as_str()),
    ));
    Ok(format!("{{{}}}", fields.join(",")))
}

fn requires_order_intent(action_type: &ExecutionActionType) -> bool {
    matches!(
        action_type,
        ExecutionActionType::PlaceOrder | ExecutionActionType::Hedge
    )
}

fn execution_order_type(leg: &TransitionLeg) -> Option<ExecutionOrderType> {
    if bool_constraint(leg, "post_only") == Some(true) {
        Some(ExecutionOrderType::PostOnly)
    } else if execution_limit_price(leg).is_some() {
        Some(ExecutionOrderType::Limit)
    } else if leg.leg_type == TransitionLegType::Trade || leg.leg_type == TransitionLegType::Hedge {
        Some(ExecutionOrderType::Market)
    } else {
        None
    }
}

fn execution_time_in_force(leg: &TransitionLeg) -> ExecutionResult<Option<ExecutionTimeInForce>> {
    if bool_constraint(leg, "post_only") == Some(true) {
        if leg.constraints.contains_key("time_in_force") {
            return Err(ExecutionError::MissingRequiredField {
                field: "candidate.legs[].constraints.time_in_force",
                detail: format!(
                    "candidate leg `{}` cannot combine post_only=true with time_in_force",
                    leg.leg_id.as_str()
                ),
            });
        }
        return Ok(None);
    }
    match string_constraint(leg, "time_in_force").as_deref() {
        Some("GTC") => Ok(Some(ExecutionTimeInForce::Gtc)),
        Some("IOC") => Ok(Some(ExecutionTimeInForce::Ioc)),
        Some("FOK") => Ok(Some(ExecutionTimeInForce::Fok)),
        Some(other) => Err(ExecutionError::MissingRequiredField {
            field: "candidate.legs[].constraints.time_in_force",
            detail: format!(
                "candidate leg `{}` time_in_force `{other}` must be GTC, IOC, or FOK",
                leg.leg_id.as_str()
            ),
        }),
        None => Ok(None),
    }
}

fn execution_quantity(
    candidate: &CandidatePortfolioTransition,
    leg: &TransitionLeg,
) -> Option<String> {
    let instrument_id = leg.instrument_id.as_ref()?;
    let account_id = leg.account_id.as_ref().map(|id| id.as_str());
    candidate
        .expected_post_state_delta
        .position_deltas
        .iter()
        .find(|delta| {
            delta.instrument_id.as_str() == instrument_id.as_str()
                && delta.account_id.as_ref().map(|id| id.as_str()) == account_id
        })
        .map(|delta| non_negative_decimal_abs(delta.quantity_delta.as_str()))
}

fn execution_limit_price(leg: &TransitionLeg) -> Option<String> {
    if let Some(value) = string_constraint(leg, "limit_price") {
        return Some(value);
    }
    match leg.side.as_ref()? {
        TransitionSide::Buy | TransitionSide::Long => string_constraint(leg, "reference_best_ask")
            .or_else(|| string_constraint(leg, "reference_last_price")),
        TransitionSide::Sell | TransitionSide::Short => {
            string_constraint(leg, "reference_best_bid")
                .or_else(|| string_constraint(leg, "reference_last_price"))
        }
        _ => None,
    }
}

fn execution_notional_usd(leg: &TransitionLeg) -> Option<String> {
    string_constraint(leg, "notional_usd")
        .or_else(|| string_constraint(leg, "notional_usdt"))
        .or_else(|| string_constraint(leg, "max_notional_usdt"))
        .or_else(|| {
            leg.asset_flows
                .iter()
                .find(|flow| flow.direction.as_str() == "Out")
                .map(|flow| flow.amount.as_str().to_owned())
        })
}

fn execution_venue_symbol(leg: &TransitionLeg) -> Option<String> {
    string_constraint(leg, "venue_symbol").or_else(|| {
        let instrument_id = leg.instrument_id.as_ref()?.as_str();
        let mut parts = instrument_id.split(':');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some("inst"), Some("BINANCE"), Some(symbol), Some(_)) if parts.next().is_none() => {
                Some(symbol.to_owned())
            }
            (Some("inst"), Some(symbol), None, None) => Some(symbol.to_owned()),
            _ => None,
        }
    })
}

fn string_constraint(leg: &TransitionLeg, field_name: &str) -> Option<String> {
    leg.constraints
        .get(field_name)
        .and_then(|value| match value {
            arb_contracts::JsonValue::String(value) => Some(value.clone()),
            arb_contracts::JsonValue::Number(value) => Some(value.as_str().to_owned()),
            _ => None,
        })
}

fn bool_constraint(leg: &TransitionLeg, field_name: &str) -> Option<bool> {
    leg.constraints
        .get(field_name)
        .and_then(|value| match value {
            arb_contracts::JsonValue::Bool(value) => Some(*value),
            _ => None,
        })
}

fn non_negative_decimal_abs(value: &str) -> String {
    value.strip_prefix('-').unwrap_or(value).to_owned()
}

fn render_dependency_edge(from_leg_id: &str, to_leg_id: &str, condition: &str) -> String {
    format!(
        "{{\"from_leg_id\":{},\"to_leg_id\":{},\"condition\":{}}}",
        json_string(from_leg_id),
        json_string(to_leg_id),
        json_string(condition),
    )
}

fn render_execution_constraints(
    risk_decision: &RiskDecision,
    candidate: &CandidatePortfolioTransition,
) -> String {
    let mut fields = Vec::new();
    if let Some(value) =
        risk_decimal_constraint(&risk_decision.constraints, RiskConstraintType::MaxNotional)
    {
        fields.push(render_pair("max_notional_usd", json_string(value)));
    }
    if let Some(value) =
        risk_decimal_constraint(&risk_decision.constraints, RiskConstraintType::MaxFee)
    {
        fields.push(render_pair("max_fee_usd", json_string(value)));
    }
    if let Some(value) = candidate_constraint(candidate, "max_slippage_bps") {
        fields.push(render_pair("slippage_limit_bps", json_string(&value)));
    } else if let Some(value) =
        risk_bps_constraint(&risk_decision.constraints, RiskConstraintType::MaxSlippage)
    {
        fields.push(render_pair("slippage_limit_bps", json_string(value)));
    }
    if let Some(value) = candidate_constraint(candidate, "min_receive_amount") {
        fields.push(render_pair("min_receive_amount", json_string(&value)));
    }
    if let Some(value) = data_freshness_ms(risk_decision) {
        fields.push(render_pair("requires_fresh_market_data_ms", value));
    }
    format!("{{{}}}", fields.join(","))
}

fn render_hedge_policy(
    action: HedgeResidualAction,
    candidate: &CandidatePortfolioTransition,
) -> String {
    let mut fields = vec![render_pair(
        "residual_exposure_action",
        json_string(action.as_str()),
    )];
    if has_failure_mode(candidate, FailureMode::PartialFill)
        || has_failure_mode(candidate, FailureMode::UnknownState)
    {
        fields.push(render_pair("threshold_usd", json_string("0")));
    }
    format!("{{{}}}", fields.join(","))
}

fn render_partial_fill_policy(
    action: PartialFillAction,
    candidate: &CandidatePortfolioTransition,
) -> String {
    let mut fields = vec![render_pair("action", json_string(action.as_str()))];
    if has_failure_mode(candidate, FailureMode::PartialFill) {
        fields.push(render_pair("max_unhedged_usd", json_string("0")));
    }
    format!("{{{}}}", fields.join(","))
}

fn render_failure_policy(policy: &ExecutionPlanPolicy) -> String {
    let mut fields = vec![
        render_pair("unknown_state_action", json_string("HaltAndIncident")),
        render_pair("retry_limit", policy.retry_limit.to_string()),
    ];
    if let Some(retry_backoff_ms) = policy.retry_backoff_ms {
        fields.push(render_pair(
            "retry_backoff_ms",
            retry_backoff_ms.to_string(),
        ));
    }
    format!("{{{}}}", fields.join(","))
}

fn scheduled_action_type(
    mode: &ExecutionMode,
    leg_type: &TransitionLegType,
) -> ExecutionActionType {
    match mode {
        ExecutionMode::ReadOnly => ExecutionActionType::RecordOnly,
        ExecutionMode::Simulated => match leg_type {
            TransitionLegType::Trade
            | TransitionLegType::Hedge
            | TransitionLegType::FundingCapture => ExecutionActionType::SimulatedFill,
            _ => ExecutionActionType::RecordOnly,
        },
        ExecutionMode::ManualApproval => ExecutionActionType::ManualApprovalGate,
        ExecutionMode::GuardedLive | ExecutionMode::AutonomousLive => {
            intended_action_type(leg_type)
        }
    }
}

fn intended_action_type(leg_type: &TransitionLegType) -> ExecutionActionType {
    match leg_type {
        TransitionLegType::Trade | TransitionLegType::FundingCapture => {
            ExecutionActionType::PlaceOrder
        }
        TransitionLegType::Transfer | TransitionLegType::Bridge => ExecutionActionType::Transfer,
        TransitionLegType::Borrow => ExecutionActionType::Borrow,
        TransitionLegType::Lend => ExecutionActionType::Lend,
        TransitionLegType::Repay => ExecutionActionType::Repay,
        TransitionLegType::Hedge => ExecutionActionType::Hedge,
        TransitionLegType::Observation => ExecutionActionType::RecordOnly,
    }
}

fn cancel_action(candidate: &CandidatePortfolioTransition) -> CancelDefaultAction {
    if has_failure_mode(candidate, FailureMode::PartialFill)
        || has_failure_mode(candidate, FailureMode::UnknownState)
        || has_failure_mode(candidate, FailureMode::LateFill)
    {
        CancelDefaultAction::CancelAndHedgeResidual
    } else {
        CancelDefaultAction::CancelOpenOrders
    }
}

fn hedge_action(
    risk_decision: &RiskDecision,
    candidate: &CandidatePortfolioTransition,
) -> HedgeResidualAction {
    if risk_decision
        .constraints
        .iter()
        .any(|constraint| constraint.constraint_type == RiskConstraintType::ForceHedge)
        || candidate
            .legs
            .iter()
            .any(|leg| leg.leg_type == TransitionLegType::Hedge)
    {
        HedgeResidualAction::HedgeImmediately
    } else if has_failure_mode(candidate, FailureMode::PartialFill)
        || has_failure_mode(candidate, FailureMode::UnknownState)
    {
        HedgeResidualAction::HedgeAfterTimeout
    } else {
        HedgeResidualAction::ManualIntervention
    }
}

fn partial_fill_action(candidate: &CandidatePortfolioTransition) -> PartialFillAction {
    if has_failure_mode(candidate, FailureMode::PartialFill) {
        PartialFillAction::HedgeFilledPortion
    } else {
        PartialFillAction::ManualIntervention
    }
}

fn selected_failure_mode(modes: &[FailureMode]) -> &FailureMode {
    const PRIORITY: &[FailureMode] = &[
        FailureMode::UnknownState,
        FailureMode::ManualInterventionRequired,
        FailureMode::PartialFill,
        FailureMode::LateFill,
        FailureMode::StuckTransaction,
        FailureMode::VenueOutage,
        FailureMode::RateLimit,
        FailureMode::RetryableFailure,
        FailureMode::DuplicateEvent,
        FailureMode::NoOpFailure,
    ];
    PRIORITY
        .iter()
        .find_map(|target| modes.iter().find(|mode| *mode == target))
        .unwrap_or_else(|| {
            modes
                .first()
                .expect("contract validates at least one failure mode")
        })
}

fn requires_venue(leg_type: &TransitionLegType) -> bool {
    !matches!(leg_type, TransitionLegType::Observation)
}

fn requires_instrument(leg_type: &TransitionLegType) -> bool {
    matches!(
        leg_type,
        TransitionLegType::Trade | TransitionLegType::Hedge | TransitionLegType::FundingCapture
    )
}

fn has_failure_mode(candidate: &CandidatePortfolioTransition, target: FailureMode) -> bool {
    candidate.failure_modes.iter().any(|mode| mode == &target)
        || candidate
            .legs
            .iter()
            .any(|leg| leg.failure_modes.iter().any(|mode| mode == &target))
}

fn risk_decimal_constraint(
    constraints: &[RiskConstraint],
    constraint_type: RiskConstraintType,
) -> Option<&str> {
    constraints.iter().find_map(|constraint| {
        if constraint.constraint_type == constraint_type {
            constraint
                .limit
                .as_ref()
                .and_then(|limit| limit.decimal_value.as_ref())
                .map(|value| value.as_str())
        } else {
            None
        }
    })
}

fn risk_bps_constraint(
    constraints: &[RiskConstraint],
    constraint_type: RiskConstraintType,
) -> Option<&str> {
    constraints.iter().find_map(|constraint| {
        if constraint.constraint_type != constraint_type {
            return None;
        }
        let limit = constraint.limit.as_ref()?;
        if limit.unit.as_deref() == Some("bps") {
            limit.decimal_value.as_ref().map(|value| value.as_str())
        } else {
            None
        }
    })
}

fn candidate_constraint(
    candidate: &CandidatePortfolioTransition,
    field_name: &str,
) -> Option<String> {
    candidate.legs.iter().find_map(|leg| {
        leg.constraints
            .get(field_name)
            .and_then(|value| match value {
                arb_contracts::JsonValue::String(value) => Some(value.clone()),
                arb_contracts::JsonValue::Number(value) => Some(value.as_str().to_owned()),
                _ => None,
            })
    })
}

fn data_freshness_ms(risk_decision: &RiskDecision) -> Option<String> {
    risk_decision.checks.iter().find_map(|check| {
        if check.check_type != RiskCheckType::DataFreshness {
            return None;
        }
        check
            .threshold
            .as_ref()
            .and_then(|threshold| threshold.decimal_value.as_ref())
            .map(|value| value.as_str().to_owned())
    })
}

fn plan_id(risk_decision: &RiskDecision) -> String {
    bounded_identifier("plan", risk_decision.decision_id.as_str())
}

fn bounded_identifier(prefix: &str, source: &str) -> String {
    let candidate = format!("{prefix}:{source}");
    if identifier_is_contract_safe(&candidate) {
        candidate
    } else {
        format!("{prefix}:sha256:{}", sha256_hex(source.as_bytes()))
    }
}

pub fn bounded_source_event_id(source_event_id: &str) -> String {
    if source_event_id.trim().is_empty() {
        return format!("event:sha256:{}", sha256_hex(source_event_id.as_bytes()));
    }
    let candidate = if source_event_id.starts_with("event:") {
        source_event_id.to_owned()
    } else {
        format!("event:{source_event_id}")
    };
    if identifier_is_contract_safe(&candidate) {
        candidate
    } else {
        format!("event:sha256:{}", sha256_hex(source_event_id.as_bytes()))
    }
}

fn execution_event_id(scope: &str, plan_id: &str, index: usize, suffix: &str) -> String {
    bounded_identifier(
        "event",
        &format!("{scope}:{plan_id}:{:04}:{suffix}", index + 1),
    )
}

fn identifier_is_contract_safe(value: &str) -> bool {
    let len = value.len();
    if !(2..=128).contains(&len) {
        return false;
    }
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-'))
}

fn render_pair(key: &str, value: String) -> String {
    format!("{}:{value}", json_string(key))
}

fn json_string(value: &str) -> String {
    to_canonical_json(&value.to_owned())
}

fn simulate_plan(input: SimulatedExecutionInput<'_>) -> ExecutionResult<SimulatedExecution> {
    if input.generated_at.trim().is_empty() {
        return Err(ExecutionError::MissingRequiredField {
            field: "generated_at",
            detail: "simulated execution report time must be explicit".to_owned(),
        });
    }

    match &input.plan.execution_mode {
        ExecutionMode::ReadOnly => build_read_only_report(input.plan, input.generated_at),
        ExecutionMode::Simulated => simulate_dispatchable_plan(input),
        ExecutionMode::ManualApproval => Err(ExecutionError::PendingManualApproval {
            plan_id: input.plan.plan_id.as_str().to_owned(),
        }),
        ExecutionMode::GuardedLive | ExecutionMode::AutonomousLive => {
            Err(ExecutionError::LiveExecutionUnavailable {
                mode: input.plan.execution_mode.clone(),
            })
        }
    }
}

fn build_read_only_report(
    plan: &ExecutionPlan,
    generated_at: &str,
) -> ExecutionResult<SimulatedExecution> {
    let leg_reports = plan
        .legs
        .iter()
        .enumerate()
        .map(|(index, leg)| {
            render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Skipped,
                &[execution_event_id(
                    "sim",
                    plan.plan_id.as_str(),
                    index,
                    "read-only",
                )],
            )
        })
        .collect::<Vec<_>>();
    let report = build_execution_report_contract(
        plan,
        generated_at,
        ExecutionReportStatus::NotDispatched,
        &leg_reports,
        &[],
        &[],
        ReconciliationStatus::NotStarted,
    )?;
    Ok(SimulatedExecution {
        report,
        leg_transitions: Vec::new(),
    })
}

#[derive(Default)]
struct PrivateExecutionAggregate {
    leg_reports: Vec<String>,
    fills: Vec<String>,
    failures: Vec<String>,
    any_fill: bool,
    any_partial: bool,
    any_failure: bool,
    any_unknown: bool,
    any_pending: bool,
    order_leg_count: usize,
    filled_order_leg_count: usize,
}

fn build_private_execution_report(
    input: PrivateExecutionReportInput<'_>,
) -> ExecutionResult<ExecutionReport> {
    if input.generated_at.trim().is_empty() {
        return Err(ExecutionError::MissingRequiredField {
            field: "generated_at",
            detail: "private execution report time must be explicit".to_owned(),
        });
    }

    let mut aggregate = PrivateExecutionAggregate::default();
    for (index, leg) in input.plan.legs.iter().enumerate() {
        if !is_private_order_leg(leg) {
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Skipped,
                &[],
            ));
            continue;
        }
        aggregate.order_leg_count += 1;
        let confirmation = input
            .confirmations
            .iter()
            .rev()
            .find(|confirmation| confirmation.plan_leg_id == leg.plan_leg_id.as_str());
        apply_private_confirmation(
            input.plan,
            input.generated_at,
            index,
            leg,
            confirmation,
            &mut aggregate,
        )?;
    }

    let status = private_execution_report_status(&aggregate);
    let reconciliation_status = private_execution_reconciliation_status(&aggregate);
    build_execution_report_contract(
        input.plan,
        input.generated_at,
        status,
        &aggregate.leg_reports,
        &aggregate.fills,
        &aggregate.failures,
        reconciliation_status,
    )
}

fn apply_private_confirmation(
    plan: &ExecutionPlan,
    generated_at: &str,
    index: usize,
    leg: &arb_contracts::ExecutionLeg,
    confirmation: Option<&PrivateOrderConfirmation>,
    aggregate: &mut PrivateExecutionAggregate,
) -> ExecutionResult<()> {
    let Some(confirmation) = confirmation else {
        let event_id = execution_event_id(
            "private",
            plan.plan_id.as_str(),
            index,
            "missing-confirmation",
        );
        aggregate.failures.push(render_failure(
            plan,
            index,
            Some(leg.plan_leg_id.as_str()),
            FailureMode::UnknownState,
            ExecutionFailureSeverity::RiskCritical,
            "没有私有订单流或查单确认，REST 回执不能证明最终订单状态。",
            "private-missing",
        ));
        aggregate.leg_reports.push(render_leg_report(
            leg.plan_leg_id.as_str(),
            LegReportStatus::Unknown,
            &[event_id],
        ));
        aggregate.any_unknown = true;
        aggregate.any_failure = true;
        return Ok(());
    };

    let refs = private_confirmation_refs(confirmation);
    match confirmation.status {
        PrivateOrderConfirmationStatus::Acknowledged => {
            aggregate.any_pending = true;
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Acknowledged,
                &refs,
            ));
            aggregate.failures.push(render_failure(
                plan,
                index,
                Some(leg.plan_leg_id.as_str()),
                FailureMode::ManualInterventionRequired,
                ExecutionFailureSeverity::Warn,
                "私有确认只证明订单已被场所接受，尚未证明最终成交、撤单或拒单状态。",
                "private-pending",
            ));
        }
        PrivateOrderConfirmationStatus::Filled => {
            if confirmation.fills.is_empty() {
                aggregate.any_unknown = true;
                aggregate.any_failure = true;
                aggregate.failures.push(render_failure(
                    plan,
                    index,
                    Some(leg.plan_leg_id.as_str()),
                    FailureMode::UnknownState,
                    ExecutionFailureSeverity::RiskCritical,
                    "查单或私有流显示已成交，但没有可入账成交明细；必须对账后处理。",
                    "private-fill-missing",
                ));
                aggregate.leg_reports.push(render_leg_report(
                    leg.plan_leg_id.as_str(),
                    LegReportStatus::Unknown,
                    &refs,
                ));
                return Ok(());
            }
            push_private_fills(plan, generated_at, index, leg, confirmation, aggregate)?;
            aggregate.filled_order_leg_count += 1;
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Filled,
                &refs,
            ));
        }
        PrivateOrderConfirmationStatus::PartiallyFilled => {
            if confirmation.fills.is_empty() {
                aggregate.any_unknown = true;
                aggregate.any_failure = true;
                aggregate.failures.push(render_failure(
                    plan,
                    index,
                    Some(leg.plan_leg_id.as_str()),
                    FailureMode::UnknownState,
                    ExecutionFailureSeverity::RiskCritical,
                    "私有确认显示部分成交，但没有可入账成交明细。",
                    "private-partial-missing",
                ));
                aggregate.leg_reports.push(render_leg_report(
                    leg.plan_leg_id.as_str(),
                    LegReportStatus::Unknown,
                    &refs,
                ));
                return Ok(());
            }
            push_private_fills(plan, generated_at, index, leg, confirmation, aggregate)?;
            aggregate.any_partial = true;
            aggregate.any_failure = true;
            aggregate.failures.push(render_failure(
                plan,
                index,
                Some(leg.plan_leg_id.as_str()),
                FailureMode::PartialFill,
                ExecutionFailureSeverity::Warn,
                confirmation
                    .detail
                    .as_deref()
                    .unwrap_or("私有确认显示部分成交；剩余敞口必须进入撤单、对冲或人工处理。"),
                "private-partial",
            ));
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::PartiallyFilled,
                &refs,
            ));
        }
        PrivateOrderConfirmationStatus::Cancelled => {
            if !confirmation.fills.is_empty() {
                push_private_fills(plan, generated_at, index, leg, confirmation, aggregate)?;
                aggregate.any_partial = true;
            }
            aggregate.any_failure = true;
            aggregate.failures.push(render_failure(
                plan,
                index,
                Some(leg.plan_leg_id.as_str()),
                if confirmation.fills.is_empty() {
                    FailureMode::NoOpFailure
                } else {
                    FailureMode::PartialFill
                },
                ExecutionFailureSeverity::Warn,
                confirmation
                    .detail
                    .as_deref()
                    .unwrap_or("私有确认显示订单已取消；不能作为完整成交成功处理。"),
                "private-cancelled",
            ));
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Cancelled,
                &refs,
            ));
        }
        PrivateOrderConfirmationStatus::Rejected | PrivateOrderConfirmationStatus::Expired => {
            aggregate.any_failure = true;
            aggregate.failures.push(render_failure(
                plan,
                index,
                Some(leg.plan_leg_id.as_str()),
                FailureMode::RetryableFailure,
                ExecutionFailureSeverity::RiskCritical,
                confirmation.detail.as_deref().unwrap_or_else(|| {
                    if confirmation.status == PrivateOrderConfirmationStatus::Rejected {
                        "私有确认显示订单被场所拒绝。"
                    } else {
                        "私有确认显示订单已过期。"
                    }
                }),
                if confirmation.status == PrivateOrderConfirmationStatus::Rejected {
                    "private-rejected"
                } else {
                    "private-expired"
                },
            ));
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Failed,
                &refs,
            ));
        }
        PrivateOrderConfirmationStatus::Unknown => {
            aggregate.any_unknown = true;
            aggregate.any_failure = true;
            aggregate.failures.push(render_failure(
                plan,
                index,
                Some(leg.plan_leg_id.as_str()),
                FailureMode::UnknownState,
                ExecutionFailureSeverity::RiskCritical,
                confirmation
                    .detail
                    .as_deref()
                    .unwrap_or("私有流或查单返回未知状态；必须失败闭合并对账。"),
                "private-unknown",
            ));
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Unknown,
                &refs,
            ));
        }
    }
    Ok(())
}

fn push_private_fills(
    plan: &ExecutionPlan,
    generated_at: &str,
    index: usize,
    leg: &arb_contracts::ExecutionLeg,
    confirmation: &PrivateOrderConfirmation,
    aggregate: &mut PrivateExecutionAggregate,
) -> ExecutionResult<()> {
    for (fill_index, fill) in confirmation.fills.iter().enumerate() {
        aggregate.fills.push(render_private_fill(
            plan,
            leg,
            index,
            fill_index,
            generated_at,
            confirmation,
            fill,
        )?);
    }
    aggregate.any_fill = true;
    Ok(())
}

fn is_private_order_leg(leg: &arb_contracts::ExecutionLeg) -> bool {
    matches!(
        leg.action_type,
        ExecutionActionType::PlaceOrder | ExecutionActionType::Hedge
    )
}

fn private_confirmation_refs(confirmation: &PrivateOrderConfirmation) -> Vec<String> {
    let mut refs = BTreeSet::new();
    for event_id in &confirmation.source_event_refs {
        refs.insert(bounded_source_event_id(event_id));
    }
    for fill in &confirmation.fills {
        refs.insert(bounded_source_event_id(&fill.source_event_id));
    }
    refs.into_iter().collect()
}

fn private_execution_report_status(aggregate: &PrivateExecutionAggregate) -> ExecutionReportStatus {
    if aggregate.any_unknown {
        ExecutionReportStatus::UnknownState
    } else if aggregate.any_pending {
        ExecutionReportStatus::ManualInterventionRequired
    } else if aggregate.any_partial || (aggregate.any_fill && aggregate.any_failure) {
        ExecutionReportStatus::PartiallySucceeded
    } else if aggregate.any_failure {
        ExecutionReportStatus::Failed
    } else if aggregate.order_leg_count > 0
        && aggregate.filled_order_leg_count == aggregate.order_leg_count
    {
        ExecutionReportStatus::Succeeded
    } else {
        ExecutionReportStatus::NotDispatched
    }
}

fn private_execution_reconciliation_status(
    aggregate: &PrivateExecutionAggregate,
) -> ReconciliationStatus {
    if aggregate.any_unknown {
        ReconciliationStatus::Unknown
    } else if aggregate.any_pending || aggregate.any_partial || aggregate.any_failure {
        ReconciliationStatus::Pending
    } else if aggregate.order_leg_count > 0
        && aggregate.filled_order_leg_count == aggregate.order_leg_count
    {
        ReconciliationStatus::Matched
    } else {
        ReconciliationStatus::NotStarted
    }
}

#[derive(Default)]
struct SimulationAggregate {
    leg_reports: Vec<String>,
    fills: Vec<String>,
    failures: Vec<String>,
    transitions: Vec<ExecutionLegTransition>,
    any_fill: bool,
    any_partial: bool,
    any_failure: bool,
    any_unknown: bool,
}

fn simulate_dispatchable_plan(
    input: SimulatedExecutionInput<'_>,
) -> ExecutionResult<SimulatedExecution> {
    let mut aggregate = SimulationAggregate::default();
    let mut blocked = false;

    for (index, leg) in input.plan.legs.iter().enumerate() {
        if blocked {
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Skipped,
                &[execution_event_id(
                    "sim",
                    input.plan.plan_id.as_str(),
                    index,
                    "blocked",
                )],
            ));
            continue;
        }

        let outcome = input
            .directives
            .iter()
            .find(|directive| directive.plan_leg_id == leg.plan_leg_id.as_str())
            .map(|directive| directive.outcome.clone())
            .unwrap_or_else(|| default_simulation_outcome(input.plan, leg, index));

        let leg_blocked = simulate_leg(
            input.plan,
            input.generated_at,
            index,
            leg,
            outcome,
            &mut aggregate,
        )?;
        blocked = leg_blocked;
    }

    let status = simulation_report_status(&aggregate);
    let reconciliation_status = simulation_reconciliation_status(&aggregate);
    let report = build_execution_report_contract(
        input.plan,
        input.generated_at,
        status,
        &aggregate.leg_reports,
        &aggregate.fills,
        &aggregate.failures,
        reconciliation_status,
    )?;

    Ok(SimulatedExecution {
        report,
        leg_transitions: aggregate.transitions,
    })
}

fn default_simulation_outcome(
    plan: &ExecutionPlan,
    leg: &arb_contracts::ExecutionLeg,
    index: usize,
) -> SimulationLegOutcome {
    if leg.action_type != ExecutionActionType::SimulatedFill {
        return SimulationLegOutcome::Skip;
    }

    SimulationLegOutcome::FullFill(SimulatedFill::new(
        fill_side_from_execution_leg(leg),
        "1",
        "1",
        default_fee_asset_id(leg),
        "0",
        execution_event_id("sim", plan.plan_id.as_str(), index, "fill"),
    ))
}

fn fill_side_from_execution_leg(leg: &arb_contracts::ExecutionLeg) -> FillSide {
    match leg.side.as_ref() {
        Some(TransitionSide::Sell) => FillSide::Sell,
        Some(TransitionSide::Long) => FillSide::Long,
        Some(TransitionSide::Short) => FillSide::Short,
        Some(TransitionSide::Receive) => FillSide::Receive,
        Some(TransitionSide::Pay) => FillSide::Pay,
        _ => FillSide::Buy,
    }
}

fn default_fee_asset_id(leg: &arb_contracts::ExecutionLeg) -> String {
    leg.expected_asset_flows
        .as_ref()
        .and_then(|flows| flows.first())
        .map(|flow| flow.asset_id.as_str().to_owned())
        .unwrap_or_else(|| "asset:SIM".to_owned())
}

fn simulate_leg(
    plan: &ExecutionPlan,
    generated_at: &str,
    index: usize,
    leg: &arb_contracts::ExecutionLeg,
    outcome: SimulationLegOutcome,
    aggregate: &mut SimulationAggregate,
) -> ExecutionResult<bool> {
    match outcome {
        SimulationLegOutcome::Skip => {
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Skipped,
                &[execution_event_id(
                    "sim",
                    plan.plan_id.as_str(),
                    index,
                    "skipped",
                )],
            ));
            Ok(false)
        }
        SimulationLegOutcome::FullFill(fill) => {
            let mut refs = Vec::new();
            let acknowledged = drive_leg_to_acknowledged(plan, index, leg, &mut refs, aggregate)?;
            push_transition(
                &mut aggregate.transitions,
                &mut refs,
                leg.plan_leg_id.as_str(),
                &acknowledged,
                ExecutionLegState::Filled,
                execution_event_id("sim", plan.plan_id.as_str(), index, "filled"),
            )?;
            refs.push(fill.source_event_id.clone());
            aggregate
                .fills
                .push(render_fill(plan, leg, index, generated_at, &fill, "full")?);
            aggregate.any_fill = true;
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Filled,
                &refs,
            ));
            Ok(false)
        }
        SimulationLegOutcome::PartialFill(fill) => {
            let mut refs = Vec::new();
            let acknowledged = drive_leg_to_acknowledged(plan, index, leg, &mut refs, aggregate)?;
            push_transition(
                &mut aggregate.transitions,
                &mut refs,
                leg.plan_leg_id.as_str(),
                &acknowledged,
                ExecutionLegState::PartiallyFilled,
                execution_event_id("sim", plan.plan_id.as_str(), index, "partial"),
            )?;
            refs.push(fill.source_event_id.clone());
            aggregate.fills.push(render_fill(
                plan,
                leg,
                index,
                generated_at,
                &fill,
                "partial",
            )?);
            aggregate.failures.push(render_failure(
                plan,
                index,
                Some(leg.plan_leg_id.as_str()),
                FailureMode::PartialFill,
                ExecutionFailureSeverity::Warn,
                "模拟部分成交；残余敞口必须进入对冲、取消或人工处理路径。",
                "partial",
            ));
            aggregate.any_fill = true;
            aggregate.any_partial = true;
            aggregate.any_failure = true;
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::PartiallyFilled,
                &refs,
            ));
            Ok(true)
        }
        SimulationLegOutcome::PartialFillThenCancel(fill, cancel) => {
            let mut refs = Vec::new();
            let acknowledged = drive_leg_to_acknowledged(plan, index, leg, &mut refs, aggregate)?;
            let partially_filled = push_transition(
                &mut aggregate.transitions,
                &mut refs,
                leg.plan_leg_id.as_str(),
                &acknowledged,
                ExecutionLegState::PartiallyFilled,
                execution_event_id("sim", plan.plan_id.as_str(), index, "partial"),
            )?;
            let cancel_requested = push_transition(
                &mut aggregate.transitions,
                &mut refs,
                leg.plan_leg_id.as_str(),
                &partially_filled,
                ExecutionLegState::CancelRequested,
                cancel.source_event_id.clone(),
            )?;
            push_transition(
                &mut aggregate.transitions,
                &mut refs,
                leg.plan_leg_id.as_str(),
                &cancel_requested,
                ExecutionLegState::Cancelled,
                execution_event_id("sim", plan.plan_id.as_str(), index, "cancelled"),
            )?;
            refs.push(fill.source_event_id.clone());
            aggregate.fills.push(render_fill(
                plan,
                leg,
                index,
                generated_at,
                &fill,
                "partial-cancel",
            )?);
            aggregate.failures.push(render_failure(
                plan,
                index,
                Some(leg.plan_leg_id.as_str()),
                FailureMode::PartialFill,
                ExecutionFailureSeverity::Warn,
                &cancel.detail,
                "cancel",
            ));
            aggregate.any_fill = true;
            aggregate.any_partial = true;
            aggregate.any_failure = true;
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Cancelled,
                &refs,
            ));
            Ok(true)
        }
        SimulationLegOutcome::Timeout(timeout) => {
            simulate_timeout_leg(plan, index, leg, timeout, aggregate)
        }
        SimulationLegOutcome::Failure(failure) => {
            let mut refs = Vec::new();
            let acknowledged = drive_leg_to_acknowledged(plan, index, leg, &mut refs, aggregate)?;
            push_transition(
                &mut aggregate.transitions,
                &mut refs,
                leg.plan_leg_id.as_str(),
                &acknowledged,
                ExecutionLegState::Failed,
                failure.source_event_id.clone(),
            )?;
            aggregate.failures.push(render_failure(
                plan,
                index,
                Some(leg.plan_leg_id.as_str()),
                failure.failure_type,
                failure.severity,
                &failure.detail,
                "failure",
            ));
            aggregate.any_failure = true;
            aggregate.leg_reports.push(render_leg_report(
                leg.plan_leg_id.as_str(),
                LegReportStatus::Failed,
                &refs,
            ));
            Ok(true)
        }
        SimulationLegOutcome::Unknown(unknown) => {
            simulate_unknown_leg(plan, index, leg, unknown, aggregate, false)
        }
        SimulationLegOutcome::CompensatedUnknown(unknown) => {
            simulate_unknown_leg(plan, index, leg, unknown, aggregate, true)
        }
    }
}

fn simulate_unknown_leg(
    plan: &ExecutionPlan,
    index: usize,
    leg: &arb_contracts::ExecutionLeg,
    unknown: SimulatedUnknown,
    aggregate: &mut SimulationAggregate,
    compensate: bool,
) -> ExecutionResult<bool> {
    let mut refs = Vec::new();
    let acknowledged = drive_leg_to_acknowledged(plan, index, leg, &mut refs, aggregate)?;
    let unknown_state = push_transition(
        &mut aggregate.transitions,
        &mut refs,
        leg.plan_leg_id.as_str(),
        &acknowledged,
        ExecutionLegState::Unknown,
        unknown.source_event_id.clone(),
    )?;
    aggregate.failures.push(render_failure(
        plan,
        index,
        Some(leg.plan_leg_id.as_str()),
        FailureMode::UnknownState,
        ExecutionFailureSeverity::RiskCritical,
        &unknown.detail,
        "unknown",
    ));

    if compensate {
        let compensation_event_id = unknown.compensation_event_id.unwrap_or_else(|| {
            execution_event_id("sim", plan.plan_id.as_str(), index, "compensate")
        });
        let compensating = push_transition(
            &mut aggregate.transitions,
            &mut refs,
            leg.plan_leg_id.as_str(),
            &unknown_state,
            ExecutionLegState::Compensating,
            compensation_event_id.clone(),
        )?;
        push_transition(
            &mut aggregate.transitions,
            &mut refs,
            leg.plan_leg_id.as_str(),
            &compensating,
            ExecutionLegState::Compensated,
            execution_event_id("sim", plan.plan_id.as_str(), index, "compensated"),
        )?;
        aggregate.failures.push(render_failure(
            plan,
            index,
            Some(leg.plan_leg_id.as_str()),
            FailureMode::ManualInterventionRequired,
            ExecutionFailureSeverity::Warn,
            "模拟补偿路径已执行；未知状态仍必须进入后续对账或事故流程。",
            "compensation",
        ));
    }

    aggregate.any_unknown = true;
    aggregate.any_failure = true;
    aggregate.leg_reports.push(render_leg_report(
        leg.plan_leg_id.as_str(),
        LegReportStatus::Unknown,
        &refs,
    ));
    Ok(true)
}

fn simulate_timeout_leg(
    plan: &ExecutionPlan,
    index: usize,
    leg: &arb_contracts::ExecutionLeg,
    timeout: SimulatedTimeout,
    aggregate: &mut SimulationAggregate,
) -> ExecutionResult<bool> {
    let mut refs = Vec::new();
    let acknowledged = drive_leg_to_acknowledged(plan, index, leg, &mut refs, aggregate)?;
    push_transition(
        &mut aggregate.transitions,
        &mut refs,
        leg.plan_leg_id.as_str(),
        &acknowledged,
        ExecutionLegState::Unknown,
        timeout.source_event_id,
    )?;
    aggregate.failures.push(render_failure(
        plan,
        index,
        Some(leg.plan_leg_id.as_str()),
        FailureMode::UnknownState,
        ExecutionFailureSeverity::RiskCritical,
        &format!("模拟执行超时 {}ms；{}", timeout.elapsed_ms, timeout.detail),
        "timeout",
    ));
    aggregate.any_unknown = true;
    aggregate.any_failure = true;
    aggregate.leg_reports.push(render_leg_report(
        leg.plan_leg_id.as_str(),
        LegReportStatus::Unknown,
        &refs,
    ));
    Ok(true)
}

fn drive_leg_to_acknowledged(
    plan: &ExecutionPlan,
    index: usize,
    leg: &arb_contracts::ExecutionLeg,
    refs: &mut Vec<String>,
    aggregate: &mut SimulationAggregate,
) -> ExecutionResult<ExecutionLegState> {
    ensure_simulated_fill_leg(leg)?;
    let ready = drive_leg_to_ready(plan, index, leg, refs, aggregate)?;
    let dispatched = push_transition(
        &mut aggregate.transitions,
        refs,
        leg.plan_leg_id.as_str(),
        &ready,
        ExecutionLegState::Dispatched,
        execution_event_id("sim", plan.plan_id.as_str(), index, "dispatch"),
    )?;
    push_transition(
        &mut aggregate.transitions,
        refs,
        leg.plan_leg_id.as_str(),
        &dispatched,
        ExecutionLegState::Acknowledged,
        execution_event_id("sim", plan.plan_id.as_str(), index, "ack"),
    )
}

fn drive_leg_to_ready(
    plan: &ExecutionPlan,
    index: usize,
    leg: &arb_contracts::ExecutionLeg,
    refs: &mut Vec<String>,
    aggregate: &mut SimulationAggregate,
) -> ExecutionResult<ExecutionLegState> {
    let mut current = leg.state.clone();
    if current == ExecutionLegState::Prepared {
        current = push_transition(
            &mut aggregate.transitions,
            refs,
            leg.plan_leg_id.as_str(),
            &current,
            ExecutionLegState::WaitingDependency,
            execution_event_id("sim", plan.plan_id.as_str(), index, "waiting"),
        )?;
    }
    if current == ExecutionLegState::WaitingDependency {
        current = push_transition(
            &mut aggregate.transitions,
            refs,
            leg.plan_leg_id.as_str(),
            &current,
            ExecutionLegState::Ready,
            execution_event_id("sim", plan.plan_id.as_str(), index, "ready"),
        )?;
    }
    if current != ExecutionLegState::Ready {
        return Err(ExecutionError::InvalidExecutionLegTransition {
            plan_leg_id: leg.plan_leg_id.as_str().to_owned(),
            from: current,
            to: ExecutionLegState::Ready,
        });
    }
    Ok(current)
}

fn push_transition(
    transitions: &mut Vec<ExecutionLegTransition>,
    refs: &mut Vec<String>,
    plan_leg_id: &str,
    from_state: &ExecutionLegState,
    to_state: ExecutionLegState,
    source_event_id: String,
) -> ExecutionResult<ExecutionLegState> {
    let source_event_id = bounded_source_event_id(&source_event_id);
    let transition =
        transition_execution_leg(plan_leg_id, from_state, to_state.clone(), &source_event_id)?;
    refs.push(source_event_id);
    transitions.push(transition);
    Ok(to_state)
}

fn ensure_simulated_fill_leg(leg: &arb_contracts::ExecutionLeg) -> ExecutionResult<()> {
    if leg.action_type != ExecutionActionType::SimulatedFill {
        return Err(ExecutionError::MissingRequiredField {
            field: "execution_plan.legs[].action_type",
            detail: format!(
                "simulation fill outcome requires SimulatedFill action, got `{}` on leg `{}`",
                leg.action_type.as_str(),
                leg.plan_leg_id.as_str()
            ),
        });
    }
    if leg.venue_id.is_none() {
        return Err(ExecutionError::MissingRequiredField {
            field: "execution_plan.legs[].venue_id",
            detail: format!(
                "simulated fill leg `{}` must carry a venue_id",
                leg.plan_leg_id.as_str()
            ),
        });
    }
    if leg.instrument_id.is_none() {
        return Err(ExecutionError::MissingRequiredField {
            field: "execution_plan.legs[].instrument_id",
            detail: format!(
                "simulated fill leg `{}` must carry an instrument_id",
                leg.plan_leg_id.as_str()
            ),
        });
    }
    Ok(())
}

fn simulation_report_status(aggregate: &SimulationAggregate) -> ExecutionReportStatus {
    if aggregate.any_unknown {
        ExecutionReportStatus::UnknownState
    } else if aggregate.any_partial || (aggregate.any_fill && aggregate.any_failure) {
        ExecutionReportStatus::PartiallySucceeded
    } else if aggregate.any_failure {
        ExecutionReportStatus::Failed
    } else {
        ExecutionReportStatus::Simulated
    }
}

fn simulation_reconciliation_status(aggregate: &SimulationAggregate) -> ReconciliationStatus {
    if aggregate.any_unknown {
        ReconciliationStatus::Unknown
    } else if aggregate.any_partial || aggregate.any_failure {
        ReconciliationStatus::Pending
    } else {
        ReconciliationStatus::Matched
    }
}

fn render_leg_report(
    plan_leg_id: &str,
    status: LegReportStatus,
    source_event_refs: &[String],
) -> String {
    let mut fields = vec![
        render_pair("plan_leg_id", json_string(plan_leg_id)),
        render_pair("status", json_string(status.as_str())),
    ];
    if !source_event_refs.is_empty() {
        fields.push(render_pair(
            "source_event_refs",
            format!(
                "[{}]",
                source_event_refs
                    .iter()
                    .map(|event_id| json_string(&bounded_source_event_id(event_id)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ));
    }
    format!("{{{}}}", fields.join(","))
}

fn render_fill(
    plan: &ExecutionPlan,
    leg: &arb_contracts::ExecutionLeg,
    index: usize,
    generated_at: &str,
    fill: &SimulatedFill,
    suffix: &str,
) -> ExecutionResult<String> {
    let venue_id = leg
        .venue_id
        .as_ref()
        .ok_or_else(|| ExecutionError::MissingRequiredField {
            field: "execution_plan.legs[].venue_id",
            detail: format!(
                "simulated fill leg `{}` must carry a venue_id",
                leg.plan_leg_id.as_str()
            ),
        })?;
    let instrument_id =
        leg.instrument_id
            .as_ref()
            .ok_or_else(|| ExecutionError::MissingRequiredField {
                field: "execution_plan.legs[].instrument_id",
                detail: format!(
                    "simulated fill leg `{}` must carry an instrument_id",
                    leg.plan_leg_id.as_str()
                ),
            })?;

    let mut fields = vec![
        render_pair("schema_version", json_string(plan.schema_version.as_str())),
        render_pair(
            "fill_id",
            json_string(&bounded_identifier(
                "fill",
                &format!("{}:{:04}:{}", plan.plan_id.as_str(), index + 1, suffix),
            )),
        ),
        render_pair("plan_id", json_string(plan.plan_id.as_str())),
        render_pair("plan_leg_id", json_string(leg.plan_leg_id.as_str())),
        render_pair("venue_id", json_string(venue_id.as_str())),
        render_pair("instrument_id", json_string(instrument_id.as_str())),
        render_pair("timestamp", json_string(generated_at)),
        render_pair("side", json_string(fill.side.as_str())),
        render_pair("price", json_string(&fill.price)),
        render_pair("quantity", json_string(&fill.quantity)),
        render_pair(
            "fee",
            format!(
                "{{\"account_id\":{},\"amount\":{},\"asset_id\":{},\"direction\":\"Out\"}}",
                json_string(leg.account_id.as_str()),
                json_string(&fill.fee_amount),
                json_string(&fill.fee_asset_id),
            ),
        ),
        render_pair(
            "source_event_id",
            json_string(&bounded_source_event_id(&fill.source_event_id)),
        ),
    ];
    if let Some(client_order_id) = &leg.client_order_id {
        fields.push(render_pair(
            "client_order_id",
            json_string(client_order_id.as_str()),
        ));
    }
    if let Some(ledger_entry_id) = &fill.ledger_entry_id {
        fields.push(render_pair("ledger_entry_id", json_string(ledger_entry_id)));
    }
    Ok(format!("{{{}}}", fields.join(",")))
}

fn render_private_fill(
    plan: &ExecutionPlan,
    leg: &arb_contracts::ExecutionLeg,
    leg_index: usize,
    fill_index: usize,
    generated_at: &str,
    confirmation: &PrivateOrderConfirmation,
    fill: &PrivateExecutionFill,
) -> ExecutionResult<String> {
    let venue_id = leg
        .venue_id
        .as_ref()
        .ok_or_else(|| ExecutionError::MissingRequiredField {
            field: "execution_plan.legs[].venue_id",
            detail: format!(
                "private order leg `{}` must carry a venue_id",
                leg.plan_leg_id.as_str()
            ),
        })?;
    let instrument_id =
        leg.instrument_id
            .as_ref()
            .ok_or_else(|| ExecutionError::MissingRequiredField {
                field: "execution_plan.legs[].instrument_id",
                detail: format!(
                    "private order leg `{}` must carry an instrument_id",
                    leg.plan_leg_id.as_str()
                ),
            })?;
    let timestamp = fill.timestamp.as_deref().unwrap_or(generated_at);
    let venue_order_id = fill
        .venue_order_id
        .as_deref()
        .or(confirmation.venue_order_id.as_deref());
    let client_order_id = fill
        .client_order_id
        .as_deref()
        .or(confirmation.client_order_id.as_deref())
        .or_else(|| leg.client_order_id.as_ref().map(|id| id.as_str()));

    let mut fields = vec![
        render_pair("schema_version", json_string(plan.schema_version.as_str())),
        render_pair(
            "fill_id",
            json_string(&bounded_identifier(
                "fill",
                &format!(
                    "{}:{:04}:private:{:04}",
                    plan.plan_id.as_str(),
                    leg_index + 1,
                    fill_index + 1
                ),
            )),
        ),
        render_pair("plan_id", json_string(plan.plan_id.as_str())),
        render_pair("plan_leg_id", json_string(leg.plan_leg_id.as_str())),
        render_pair("venue_id", json_string(venue_id.as_str())),
        render_pair("instrument_id", json_string(instrument_id.as_str())),
        render_pair("timestamp", json_string(timestamp)),
        render_pair("side", json_string(fill.side.as_str())),
        render_pair("price", json_string(&fill.price)),
        render_pair("quantity", json_string(&fill.quantity)),
        render_pair(
            "fee",
            format!(
                "{{\"account_id\":{},\"amount\":{},\"asset_id\":{},\"direction\":\"Out\"}}",
                json_string(leg.account_id.as_str()),
                json_string(&fill.fee_amount),
                json_string(&fill.fee_asset_id),
            ),
        ),
        render_pair(
            "source_event_id",
            json_string(&bounded_source_event_id(&fill.source_event_id)),
        ),
    ];
    if let Some(venue_order_id) = venue_order_id {
        fields.push(render_pair("venue_order_id", json_string(venue_order_id)));
    }
    if let Some(client_order_id) = client_order_id {
        fields.push(render_pair("client_order_id", json_string(client_order_id)));
    }
    if let Some(ledger_entry_id) = &fill.ledger_entry_id {
        fields.push(render_pair("ledger_entry_id", json_string(ledger_entry_id)));
    }
    Ok(format!("{{{}}}", fields.join(",")))
}

fn render_failure(
    plan: &ExecutionPlan,
    index: usize,
    plan_leg_id: Option<&str>,
    failure_type: FailureMode,
    severity: ExecutionFailureSeverity,
    detail: &str,
    suffix: &str,
) -> String {
    let mut fields = vec![
        render_pair(
            "failure_id",
            json_string(&bounded_identifier(
                "failure",
                &format!("{}:{:04}:{}", plan.plan_id.as_str(), index + 1, suffix),
            )),
        ),
        render_pair("failure_type", json_string(failure_type.as_str())),
        render_pair("severity", json_string(severity.as_str())),
        render_pair("detail", json_string(detail)),
    ];
    if let Some(plan_leg_id) = plan_leg_id {
        fields.push(render_pair("plan_leg_id", json_string(plan_leg_id)));
    }
    format!("{{{}}}", fields.join(","))
}

fn render_ledger_entry(
    plan: &ExecutionPlan,
    report: &ExecutionReport,
    fill: &arb_contracts::Fill,
    namespace: &LedgerNamespace,
) -> ExecutionResult<LedgerEntry> {
    let account_id =
        fill.fee
            .account_id
            .as_ref()
            .ok_or_else(|| ExecutionError::MissingRequiredField {
                field: "execution_report.fills[].fee.account_id",
                detail: format!(
                    "fill `{}` cannot become ledger input without an account_id",
                    fill.fill_id.as_str()
                ),
            })?;
    let ledger_entry_id = fill
        .ledger_entry_id
        .as_ref()
        .map(|entry_id| entry_id.as_str().to_owned())
        .unwrap_or_else(|| {
            bounded_identifier(
                "ledger",
                &format!(
                    "{}:{}",
                    ledger_namespace_token(namespace),
                    fill.fill_id.as_str()
                ),
            )
        });
    let journal_entry_id = bounded_identifier("journal", &ledger_entry_id);
    let idempotency_key = bounded_identifier("idem", &ledger_entry_id);
    let debit_leg_id = bounded_identifier("ledleg", &format!("{}:debit", fill.fill_id.as_str()));
    let credit_leg_id = bounded_identifier("ledleg", &format!("{}:credit", fill.fill_id.as_str()));
    let assertion_hash = format!(
        "hash:{}-ledger:{}",
        ledger_namespace_token(namespace),
        fill.fill_id.as_str()
    );
    let memo = format!(
        "{} ledger input for report {}",
        ledger_namespace_memo_prefix(namespace),
        report.report_id.as_str()
    );

    let ledger_json = format!(
        "{{\"ledger_entry_id\":{},\"journal_entry_id\":{},\"schema_version\":{},\"timestamp\":{},\"namespace\":{},\"entry_type\":\"TradeFill\",\"source_event_id\":{},\"idempotency_key\":{},\"opportunity_id\":{},\"execution_plan_id\":{},\"legs\":[{{\"leg_id\":{},\"account_id\":{},\"asset_id\":{},\"direction\":\"Debit\",\"amount\":{},\"memo\":{}}},{{\"leg_id\":{},\"account_id\":{},\"asset_id\":{},\"direction\":\"Credit\",\"amount\":{},\"memo\":{}}}],\"balance_assertion\":{{\"balanced\":true,\"assertion_hash\":{},\"checked_by\":\"arb-execution\"}}}}",
        json_string(&ledger_entry_id),
        json_string(&journal_entry_id),
        json_string(report.schema_version.as_str()),
        json_string(fill.timestamp.as_str()),
        json_string(namespace.as_str()),
        json_string(fill.source_event_id.as_str()),
        json_string(&idempotency_key),
        json_string(plan.transition_id.as_str()),
        json_string(report.plan_id.as_str()),
        json_string(&debit_leg_id),
        json_string(account_id.as_str()),
        json_string(fill.fee.asset_id.as_str()),
        json_string(fill.quantity.as_str()),
        json_string(&memo),
        json_string(&credit_leg_id),
        json_string(account_id.as_str()),
        json_string(fill.fee.asset_id.as_str()),
        json_string(fill.quantity.as_str()),
        json_string(&memo),
        json_string(&assertion_hash),
    );

    Ok(from_json_strict::<LedgerEntry>(&ledger_json)?)
}

fn ledger_namespace_token(namespace: &LedgerNamespace) -> &'static str {
    match namespace {
        LedgerNamespace::Live => "live",
        LedgerNamespace::Simulation => "sim",
        LedgerNamespace::Backtest => "backtest",
        LedgerNamespace::Adjustment => "adjustment",
    }
}

fn ledger_namespace_memo_prefix(namespace: &LedgerNamespace) -> &'static str {
    match namespace {
        LedgerNamespace::Live => "private confirmed live",
        LedgerNamespace::Simulation => "simulated",
        LedgerNamespace::Backtest => "backtest",
        LedgerNamespace::Adjustment => "adjustment",
    }
}

fn private_execution_incident_source_refs(report: &ExecutionReport) -> Vec<String> {
    let mut refs = BTreeSet::new();
    for leg_report in &report.leg_reports {
        if let Some(source_event_refs) = &leg_report.source_event_refs {
            for event_id in source_event_refs {
                refs.insert(event_id.as_str().to_owned());
            }
        }
    }
    for fill in &report.fills {
        refs.insert(fill.source_event_id.as_str().to_owned());
    }
    if refs.is_empty() {
        refs.insert(report.report_id.as_str().to_owned());
    }
    refs.into_iter().collect()
}

fn private_execution_incident_venues(plan: &ExecutionPlan) -> Vec<String> {
    let mut venue_ids = BTreeSet::new();
    for leg in &plan.legs {
        if let Some(venue_id) = &leg.venue_id {
            venue_ids.insert(venue_id.as_str().to_owned());
        }
    }
    if venue_ids.is_empty() {
        venue_ids.insert("venue:UNKNOWN".to_owned());
    }
    venue_ids.into_iter().collect()
}

fn private_execution_capital_at_risk(plan: &ExecutionPlan) -> &str {
    plan.constraints
        .max_notional_usd
        .as_ref()
        .map(|value| value.as_str())
        .unwrap_or("0")
}

fn private_execution_incident_trigger(report: &ExecutionReport) -> &'static str {
    match &report.status {
        ExecutionReportStatus::UnknownState => "EXECUTION_UNKNOWN_STATE",
        ExecutionReportStatus::PartiallySucceeded => "EXECUTION_PARTIAL_FILL",
        ExecutionReportStatus::ManualInterventionRequired => "EXECUTION_MANUAL_REVIEW",
        ExecutionReportStatus::Failed => "EXECUTION_FAILED",
        _ => "EXECUTION_RECONCILIATION_REQUIRED",
    }
}

fn private_execution_incident_severity(report: &ExecutionReport) -> &'static str {
    if report
        .failures
        .iter()
        .any(|failure| failure.severity == ExecutionFailureSeverity::RiskCritical)
        || report.status == ExecutionReportStatus::UnknownState
    {
        "SEV1"
    } else if report
        .failures
        .iter()
        .any(|failure| failure.severity == ExecutionFailureSeverity::Warn)
    {
        "SEV2"
    } else {
        "SEV3"
    }
}

fn private_execution_incident_detail(report: &ExecutionReport) -> String {
    if let Some(failure) = report.failures.first() {
        if let Some(detail) = &failure.detail {
            return detail.clone();
        }
        return format!(
            "Private execution report contains `{}` failure.",
            failure.failure_type.as_str()
        );
    }
    "Private execution report requires reconciliation before retry.".to_owned()
}

fn build_execution_report_contract(
    plan: &ExecutionPlan,
    generated_at: &str,
    status: ExecutionReportStatus,
    leg_reports: &[String],
    fills: &[String],
    failures: &[String],
    reconciliation_status: ReconciliationStatus,
) -> ExecutionResult<ExecutionReport> {
    let report_json = format!(
        "{{\"schema_version\":{},\"report_id\":{},\"plan_id\":{},\"generated_at\":{},\"status\":{},\"leg_reports\":[{}],\"fills\":[{}],\"failures\":[{}],\"reconciliation_status\":{}}}",
        json_string(plan.schema_version.as_str()),
        json_string(&bounded_identifier("report", plan.plan_id.as_str())),
        json_string(plan.plan_id.as_str()),
        json_string(generated_at),
        json_string(status.as_str()),
        leg_reports.join(","),
        fills.join(","),
        failures.join(","),
        json_string(reconciliation_status.as_str()),
    );
    Ok(from_json_strict::<ExecutionReport>(&report_json)?)
}

fn stable_plan_hash(canonical_plan: &str) -> String {
    format!("hash:sha256:{}", sha256_hex(canonical_plan.as_bytes()))
}

fn sha256_hex(input: &[u8]) -> String {
    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut state = H0;
    let bit_len = (input.len() as u64) * 8;
    let mut padded = Vec::with_capacity(input.len() + 72);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];

        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut output = String::with_capacity(64);
    for word in state {
        push_hex_u32(word, &mut output);
    }
    output
}

fn push_hex_u32(word: u32, output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in word.to_be_bytes() {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_contracts::{
        from_json_strict, DependencyCondition, LedgerDirection, LedgerEntryType, LedgerNamespace,
        UnknownStateAction,
    };

    const CANDIDATE: &str =
        include_str!("../../../fixtures/replay/risk_accept/candidate_transition.json");
    const APPROVED_DECISION: &str =
        include_str!("../../../fixtures/replay/risk_accept/expected/risk_decisions.jsonl");
    const REJECTED_DECISION: &str =
        include_str!("../../../fixtures/replay/risk_reject/expected/risk_decisions.jsonl");
    const SIMULATED_SUCCESS_REPORT: &str = include_str!(
        "../../../fixtures/replay/execution_simulated_success/expected/execution_reports.jsonl"
    );
    const SIMULATED_SUCCESS_LEDGER: &str = include_str!(
        "../../../fixtures/replay/execution_simulated_success/expected/ledger_entries.jsonl"
    );
    const READ_ONLY_REPORT: &str = include_str!(
        "../../../fixtures/replay/execution_read_only/expected/execution_reports.jsonl"
    );
    const PARTIAL_FILL_REPORT: &str = include_str!(
        "../../../fixtures/replay/execution_partial_fill/expected/execution_reports.jsonl"
    );
    const PARTIAL_FILL_LEDGER: &str = include_str!(
        "../../../fixtures/replay/execution_partial_fill/expected/ledger_entries.jsonl"
    );
    const UNKNOWN_STATE_REPORT: &str = include_str!(
        "../../../fixtures/replay/execution_unknown_state/expected/execution_reports.jsonl"
    );

    #[test]
    fn approved_risk_decision_builds_schedulable_read_only_plan() {
        let candidate = candidate(CANDIDATE);
        let risk_decision = decision(APPROVED_DECISION);
        let plan = build_execution_plan(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect("approved decision should produce a plan");

        assert_eq!(
            plan.transition_id.as_str(),
            risk_decision.transition_id.as_str()
        );
        assert_eq!(
            plan.risk_decision_id.as_str(),
            risk_decision.decision_id.as_str()
        );
        assert_eq!(plan.execution_mode, ExecutionMode::ReadOnly);
        assert_eq!(plan.legs.len(), 1);
        assert_eq!(plan.legs[0].action_type, ExecutionActionType::RecordOnly);
        assert_eq!(plan.legs[0].state, ExecutionLegState::Ready);
        assert_eq!(
            plan.timeout_policy.plan_timeout_ms.as_u64(),
            DEFAULT_PLAN_TIMEOUT_MS
        );
        assert_eq!(
            plan.timeout_policy.leg_timeout_ms.as_u64(),
            DEFAULT_LEG_TIMEOUT_MS
        );
        assert_eq!(
            plan.timeout_policy
                .unknown_state_after_ms
                .expect("unknown state timeout")
                .as_u64(),
            DEFAULT_UNKNOWN_STATE_AFTER_MS
        );
        assert_eq!(
            plan.cancel_policy.default_action,
            CancelDefaultAction::CancelOpenOrders
        );
        assert_eq!(
            plan.hedge_policy.residual_exposure_action,
            HedgeResidualAction::ManualIntervention
        );
        assert_eq!(
            plan.partial_fill_policy.action,
            PartialFillAction::ManualIntervention
        );
        assert_eq!(
            plan.failure_policy.unknown_state_action,
            UnknownStateAction::HaltAndIncident
        );
        assert_eq!(plan.failure_policy.retry_limit, 0);
    }

    #[test]
    fn invalid_time_in_force_fails_plan_building() {
        let candidate_json = CANDIDATE.replace(
            r#""post_only": false"#,
            r#""post_only": false, "time_in_force": "DAY""#,
        );
        let candidate = candidate(&candidate_json);
        let risk_decision = decision(&manual_decision_json());
        let error = build_execution_plan_preview(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect_err("invalid time_in_force must fail closed");

        assert!(matches!(
            error,
            ExecutionError::MissingRequiredField {
                field: "candidate.legs[].constraints.time_in_force",
                ..
            }
        ));
    }

    #[test]
    fn rejected_risk_decision_cannot_generate_execution_plan() {
        let candidate = candidate(CANDIDATE);
        let risk_decision = decision(REJECTED_DECISION);
        let error = build_execution_plan(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect_err("rejected decision must not produce a plan");

        assert!(matches!(
            error,
            ExecutionError::RiskDecisionNotApproved {
                decision: RiskDecisionKind::Rejected,
                ..
            }
        ));
    }

    #[test]
    fn manual_approval_decision_only_builds_non_dispatchable_preview() {
        let candidate = candidate(CANDIDATE);
        let risk_decision = decision(&manual_decision_json());
        let outcome = build_execution_plan_preview(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect("manual approval should produce a preview");

        let pending = match outcome {
            PlanBuildOutcome::PendingManualApproval(pending) => pending,
            PlanBuildOutcome::Schedulable(_) => panic!("manual approval must not be dispatchable"),
        };

        assert!(!pending.is_dispatchable());
        assert!(pending.dispatchable_plan().is_err());
        assert!(pending
            .approval_material
            .plan_hash
            .starts_with("hash:sha256:"));
        assert_eq!(
            pending.approval_material.risk_decision_id,
            risk_decision.decision_id.as_str()
        );

        let plan = &pending.plan_preview;
        assert_eq!(plan.execution_mode, ExecutionMode::ManualApproval);
        assert_eq!(
            plan.legs[0].action_type,
            ExecutionActionType::ManualApprovalGate
        );
        assert_eq!(plan.legs[0].state, ExecutionLegState::Prepared);
        assert_eq!(plan.legs[1].state, ExecutionLegState::WaitingDependency);
        assert_eq!(plan.legs[1].action_type, ExecutionActionType::PlaceOrder);
        assert_eq!(plan.dependency_graph.edges.len(), 1);
        assert_eq!(
            plan.dependency_graph.edges[0].condition,
            DependencyCondition::ManualRelease
        );
    }

    #[test]
    fn funding_arb_preview_builds_perp_long_and_short_order_legs() {
        let candidate = candidate(FUNDING_ARB_CANDIDATE);
        let risk_decision = decision(&manual_decision_json());
        let outcome = build_execution_plan_preview(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect("manual approval should produce a funding arb preview");

        let pending = match outcome {
            PlanBuildOutcome::PendingManualApproval(pending) => pending,
            PlanBuildOutcome::Schedulable(_) => panic!("manual approval must not be dispatchable"),
        };

        assert_eq!(pending.plan_preview.legs.len(), 3);
        let long = &pending.plan_preview.legs[1];
        let short = &pending.plan_preview.legs[2];
        assert_eq!(long.action_type, ExecutionActionType::PlaceOrder);
        assert_eq!(short.action_type, ExecutionActionType::PlaceOrder);
        assert_eq!(long.side.as_ref().expect("long side").as_str(), "Long");
        assert_eq!(short.side.as_ref().expect("short side").as_str(), "Short");
        assert_eq!(
            long.basis_leg_role.as_ref().expect("long role").as_str(),
            "perp_long"
        );
        assert_eq!(
            short.basis_leg_role.as_ref().expect("short role").as_str(),
            "perp_short"
        );
        assert_eq!(long.quantity.as_ref().expect("long qty").as_str(), "1");
        assert_eq!(short.quantity.as_ref().expect("short qty").as_str(), "1");
    }

    #[test]
    fn funding_arb_preview_bounds_long_leg_idempotency_keys() {
        let transition_id =
            "trans:cross-exchange-funding-arb-binance-hyperliquid-pharosusdt-live-observer";
        let candidate_json = FUNDING_ARB_CANDIDATE
            .replace(
                "\"transition_id\": \"trans:01\"",
                &format!("\"transition_id\": \"{transition_id}\""),
            )
            .replace(
                "candleg:funding:long",
                "candleg:funding-arb-binance-usdm-pharosusdt",
            )
            .replace(
                "candleg:funding:short",
                "candleg:funding-arb-hyperliquid-perp-pharosusdt",
            );
        let risk_decision = decision(&manual_decision_json().replace("trans:01", transition_id));
        let candidate = candidate(&candidate_json);
        let outcome = build_execution_plan_preview(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect("long funding arb identifiers should still produce a valid preview");

        let pending = match outcome {
            PlanBuildOutcome::PendingManualApproval(pending) => pending,
            PlanBuildOutcome::Schedulable(_) => panic!("manual approval must not be dispatchable"),
        };

        for leg in &pending.plan_preview.legs {
            assert!(leg.idempotency_key.as_str().len() <= 128);
        }
        assert!(pending.plan_preview.legs[2]
            .idempotency_key
            .as_str()
            .starts_with("idem:sha256:"));
    }

    #[test]
    fn private_execution_report_bounds_long_source_event_refs() {
        let candidate = candidate(FUNDING_ARB_CANDIDATE);
        let risk_decision = decision(&manual_decision_json());
        let pending = match build_execution_plan_preview(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect("manual approval should produce a funding arb preview")
        {
            PlanBuildOutcome::PendingManualApproval(pending) => pending,
            PlanBuildOutcome::Schedulable(_) => panic!("manual approval must not be dispatchable"),
        };
        let order_leg = &pending.plan_preview.legs[1];
        let long_source_event_id = format!(
            "event:funding-arb-live-canary-order-query:first-perp-after-submit:{}:{}",
            order_leg.plan_leg_id.as_str(),
            "x".repeat(120)
        );
        assert!(long_source_event_id.len() > 128);
        let confirmation = PrivateOrderConfirmation::new(
            order_leg.plan_leg_id.as_str(),
            PrivateOrderConfirmationStatus::Cancelled,
            PrivateOrderConfirmationSource::OrderQuery,
            long_source_event_id,
        );

        let report = execution_report_from_private_confirmations(PrivateExecutionReportInput::new(
            &pending.plan_preview,
            "2026-01-01T00:00:06Z",
            std::slice::from_ref(&confirmation),
        ))
        .expect("long source event refs are compacted before contract validation");

        for refs in report
            .leg_reports
            .iter()
            .filter_map(|leg_report| leg_report.source_event_refs.as_ref())
        {
            for event_id in refs {
                assert!(event_id.as_str().len() <= 128);
            }
        }
        assert!(report.leg_reports[1]
            .source_event_refs
            .as_ref()
            .expect("bounded source refs")
            .iter()
            .any(|event_id| event_id.as_str().starts_with("event:sha256:")));
    }

    #[test]
    fn private_execution_report_bounds_empty_source_event_refs() {
        let candidate = candidate(FUNDING_ARB_CANDIDATE);
        let risk_decision = decision(&manual_decision_json());
        let pending = match build_execution_plan_preview(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect("manual approval should produce a funding arb preview")
        {
            PlanBuildOutcome::PendingManualApproval(pending) => pending,
            PlanBuildOutcome::Schedulable(_) => panic!("manual approval must not be dispatchable"),
        };
        let order_leg = &pending.plan_preview.legs[1];
        let confirmation = PrivateOrderConfirmation::new(
            order_leg.plan_leg_id.as_str(),
            PrivateOrderConfirmationStatus::Cancelled,
            PrivateOrderConfirmationSource::OrderQuery,
            "",
        );

        let report = execution_report_from_private_confirmations(PrivateExecutionReportInput::new(
            &pending.plan_preview,
            "2026-01-01T00:00:06Z",
            std::slice::from_ref(&confirmation),
        ))
        .expect("empty source event refs are compacted before contract validation");

        assert!(report.leg_reports[1]
            .source_event_refs
            .as_ref()
            .expect("bounded source refs")
            .iter()
            .any(|event_id| event_id.as_str().starts_with("event:sha256:")));
    }

    #[test]
    fn approved_manual_approval_record_releases_only_the_manual_gate() {
        let pending = pending_manual_plan();
        let record = review_manual_approval(
            ManualApprovalReviewInput::new(
                &pending,
                "event:approval:approved:01",
                "operator:alice",
                "2026-01-01T00:01:00Z",
                "2026-01-01T00:05:00Z",
                ManualApprovalDecision::Approve,
            )
            .with_reason("Reviewed risk summary and plan hash."),
        )
        .expect("approval record");

        assert_eq!(record.status, ManualApprovalStatus::Approved);
        assert!(record.releases_manual_gate);
        assert_eq!(record.plan_hash, execution_plan_hash(&pending.plan_preview));
        assert!(record.controlled_next_step.contains("capital reservation"));
        assert_eq!(record.to_audit_json(), record.to_audit_json());

        let release = release_manual_approval_gate(&pending, &record).expect("gate release");
        assert_eq!(release.approval_event_id, "event:approval:approved:01");
        assert_eq!(
            release.gate_transition.from_state,
            ExecutionLegState::Prepared
        );
        assert_eq!(release.gate_transition.to_state, ExecutionLegState::Ready);
        assert_eq!(release.dependent_transitions.len(), 1);
        assert_eq!(
            release.dependent_transitions[0].from_state,
            ExecutionLegState::WaitingDependency
        );
        assert_eq!(
            release.dependent_transitions[0].to_state,
            ExecutionLegState::Ready
        );
        assert!(pending.dispatchable_plan().is_err());
    }

    #[test]
    fn rejected_expired_and_duplicate_manual_approval_do_not_release_gate() {
        let pending = pending_manual_plan();
        let rejected = review_manual_approval(
            ManualApprovalReviewInput::new(
                &pending,
                "event:approval:rejected:01",
                "operator:bob",
                "2026-01-01T00:01:00Z",
                "2026-01-01T00:05:00Z",
                ManualApprovalDecision::Reject,
            )
            .with_reason("Operator rejected the opportunity."),
        )
        .expect("rejection record");
        assert_eq!(rejected.status, ManualApprovalStatus::Rejected);
        assert!(!rejected.releases_manual_gate);
        assert!(matches!(
            release_manual_approval_gate(&pending, &rejected),
            Err(ExecutionError::ManualApprovalNotReleased {
                status: ManualApprovalStatus::Rejected,
                ..
            })
        ));

        let expired = review_manual_approval(ManualApprovalReviewInput::new(
            &pending,
            "event:approval:expired:01",
            "operator:carol",
            "2026-01-01T00:06:00Z",
            "2026-01-01T00:05:00Z",
            ManualApprovalDecision::Approve,
        ))
        .expect("expired record");
        assert_eq!(expired.status, ManualApprovalStatus::Expired);
        assert!(!expired.releases_manual_gate);

        let first = review_manual_approval(ManualApprovalReviewInput::new(
            &pending,
            "event:approval:first:01",
            "operator:dana",
            "2026-01-01T00:01:00Z",
            "2026-01-01T00:05:00Z",
            ManualApprovalDecision::Approve,
        ))
        .expect("first record");
        let prior_records = vec![first.clone()];
        let duplicate = review_manual_approval(
            ManualApprovalReviewInput::new(
                &pending,
                "event:approval:duplicate:01",
                "operator:erin",
                "2026-01-01T00:02:00Z",
                "2026-01-01T00:05:00Z",
                ManualApprovalDecision::Approve,
            )
            .with_prior_records(&prior_records),
        )
        .expect("duplicate record");
        assert_eq!(duplicate.status, ManualApprovalStatus::Duplicate);
        assert_eq!(duplicate.duplicate_of, Some(first.record_id));
        assert!(!duplicate.releases_manual_gate);
    }

    #[test]
    fn manual_approval_cannot_be_reused_after_plan_hash_changes() {
        let mut pending = pending_manual_plan();
        pending.approval_material.plan_hash = "hash:sha256:stale".to_owned();
        let error = review_manual_approval(ManualApprovalReviewInput::new(
            &pending,
            "event:approval:stale:01",
            "operator:alice",
            "2026-01-01T00:01:00Z",
            "2026-01-01T00:05:00Z",
            ManualApprovalDecision::Approve,
        ))
        .expect_err("stale plan hash must fail closed");

        assert!(matches!(
            error,
            ExecutionError::ManualApprovalPlanHashMismatch { .. }
        ));
    }

    #[test]
    fn missing_required_leg_account_fails_plan_building() {
        let candidate_without_account =
            CANDIDATE.replace("      \"account_id\": \"acct:sim\",\n", "");
        let candidate = candidate(&candidate_without_account);
        let risk_decision = decision(APPROVED_DECISION);
        let error = build_execution_plan(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect_err("candidate leg account is required by execution plan");

        assert!(matches!(
            error,
            ExecutionError::MissingRequiredField {
                field: "candidate.legs[].account_id",
                ..
            }
        ));
    }

    #[test]
    fn multi_leg_plan_contains_dependency_edges() {
        let candidate = candidate(TWO_LEG_CANDIDATE);
        let risk_decision = decision(APPROVED_DECISION);
        let plan = build_execution_plan(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::Simulated,
            "2026-01-01T00:00:04Z",
        ))
        .expect("approved two-leg candidate should produce a plan");

        assert_eq!(plan.legs.len(), 2);
        assert_eq!(plan.legs[0].state, ExecutionLegState::Ready);
        assert_eq!(plan.legs[1].state, ExecutionLegState::WaitingDependency);
        assert_eq!(
            plan.legs[1].depends_on.as_ref().expect("dependency")[0].as_str(),
            plan.legs[0].plan_leg_id.as_str()
        );
        assert_eq!(plan.dependency_graph.edges.len(), 1);
        assert_eq!(
            plan.dependency_graph.edges[0].from_leg_id.as_str(),
            plan.legs[0].plan_leg_id.as_str()
        );
        assert_eq!(
            plan.dependency_graph.edges[0].to_leg_id.as_str(),
            plan.legs[1].plan_leg_id.as_str()
        );
        assert_eq!(
            plan.dependency_graph.edges[0].condition,
            DependencyCondition::OnSuccess
        );
    }

    #[test]
    fn live_modes_fail_closed_in_stage_six_planner() {
        let candidate = candidate(CANDIDATE);
        let risk_decision = decision(APPROVED_DECISION);
        let error = build_execution_plan(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::GuardedLive,
            "2026-01-01T00:00:04Z",
        ))
        .expect_err("stage 6 planner must not enable live execution");

        assert!(matches!(
            error,
            ExecutionError::LiveExecutionUnavailable {
                mode: ExecutionMode::GuardedLive
            }
        ));
    }

    #[test]
    fn execution_leg_state_machine_rejects_illegal_transition() {
        let error = transition_execution_leg(
            "pleg:illegal",
            &ExecutionLegState::Prepared,
            ExecutionLegState::Filled,
            "event:sim:illegal",
        )
        .expect_err("prepared leg cannot jump directly to filled");

        assert!(matches!(
            error,
            ExecutionError::InvalidExecutionLegTransition {
                from: ExecutionLegState::Prepared,
                to: ExecutionLegState::Filled,
                ..
            }
        ));
    }

    #[test]
    fn simulated_success_report_round_trips_and_matches_fixture() {
        let plan = simulated_plan(CANDIDATE);
        let first = simulate_execution(&plan, "2026-01-01T00:00:05Z")
            .expect("simulated plan should produce a report");
        let second = simulate_execution(&plan, "2026-01-01T00:00:05Z")
            .expect("same input should produce a stable report");

        let canonical_first = to_canonical_json(&first);
        let canonical_second = to_canonical_json(&second);
        assert_eq!(canonical_first, canonical_second);

        let round_trip: ExecutionReport =
            from_json_strict(&canonical_first).expect("report JSON round trip");
        assert_eq!(canonical_first, to_canonical_json(&round_trip));

        let expected: ExecutionReport =
            from_json_strict(SIMULATED_SUCCESS_REPORT.trim()).expect("expected report fixture");
        assert_eq!(canonical_first, to_canonical_json(&expected));
        assert_eq!(first.status, ExecutionReportStatus::Simulated);
        assert_eq!(first.reconciliation_status, ReconciliationStatus::Matched);
        assert_eq!(first.fills.len(), 1);
        assert!(first.failures.is_empty());

        let ledger_entries = simulated_ledger_entries_from_execution_report(&plan, &first)
            .expect("simulated report can become ledger input");
        assert_eq!(ledger_entries.len(), 1);
        assert_eq!(ledger_entries[0].namespace, LedgerNamespace::Simulation);
        assert_eq!(ledger_entries[0].entry_type, LedgerEntryType::TradeFill);
        assert_eq!(
            ledger_entries[0].execution_plan_id,
            Some(first.plan_id.clone())
        );
        assert_eq!(ledger_entries[0].legs.len(), 2);
        assert_eq!(ledger_entries[0].legs[0].direction, LedgerDirection::Debit);
        assert_eq!(ledger_entries[0].legs[1].direction, LedgerDirection::Credit);

        let expected_ledger: LedgerEntry =
            from_json_strict(SIMULATED_SUCCESS_LEDGER.trim()).expect("expected ledger fixture");
        assert_eq!(
            to_canonical_json(&ledger_entries[0]),
            to_canonical_json(&expected_ledger)
        );
    }

    #[test]
    fn read_only_report_matches_fixture_and_produces_no_ledger_entries() {
        let candidate = candidate(CANDIDATE);
        let risk_decision = decision(APPROVED_DECISION);
        let plan = build_execution_plan(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect("approved read-only decision should produce a plan");
        let report = simulate_execution(&plan, "2026-01-01T00:00:05Z")
            .expect("read-only plan should produce a report");

        let expected: ExecutionReport =
            from_json_strict(READ_ONLY_REPORT.trim()).expect("expected read-only report fixture");
        assert_eq!(to_canonical_json(&report), to_canonical_json(&expected));
        assert_eq!(report.status, ExecutionReportStatus::NotDispatched);
        assert!(report.fills.is_empty());
        assert!(report.failures.is_empty());
        let ledger_entries = simulated_ledger_entries_from_execution_report(&plan, &report)
            .expect("read-only report can be inspected for ledger input");
        assert!(ledger_entries.is_empty());
    }

    #[test]
    fn simulated_partial_success_blocks_dependent_legs() {
        let plan = simulated_plan(TWO_LEG_CANDIDATE);
        let directives = [SimulationLegDirective::new(
            plan.legs[0].plan_leg_id.as_str(),
            SimulationLegOutcome::PartialFill(SimulatedFill::new(
                FillSide::Buy,
                "1",
                "0.5",
                "asset:USDC",
                "0.01",
                "event:sim:partial:fill",
            )),
        )];
        let result = simulate_execution_with_script(SimulatedExecutionInput::new(
            &plan,
            "2026-01-01T00:00:05Z",
            &directives,
        ))
        .expect("partial simulation should produce a report");

        assert_eq!(
            result.report.status,
            ExecutionReportStatus::PartiallySucceeded
        );
        assert_eq!(
            result.report.reconciliation_status,
            ReconciliationStatus::Pending
        );
        assert_eq!(result.report.fills.len(), 1);
        assert_eq!(
            result.report.failures[0].failure_type,
            FailureMode::PartialFill
        );
        assert_eq!(
            result.report.leg_reports[0].status,
            LegReportStatus::PartiallyFilled
        );
        assert_eq!(
            result.report.leg_reports[1].status,
            LegReportStatus::Skipped
        );
        assert!(result
            .leg_transitions
            .iter()
            .any(|transition| transition.to_state == ExecutionLegState::PartiallyFilled));

        let expected: ExecutionReport =
            from_json_strict(PARTIAL_FILL_REPORT.trim()).expect("expected partial report fixture");
        assert_eq!(
            to_canonical_json(&result.report),
            to_canonical_json(&expected)
        );
        let ledger_entries = simulated_ledger_entries_from_execution_report(&plan, &result.report)
            .expect("partial fill can produce ledger input for filled portion");
        assert_eq!(ledger_entries.len(), 1);
        let expected_ledger: LedgerEntry =
            from_json_strict(PARTIAL_FILL_LEDGER.trim()).expect("expected partial ledger fixture");
        assert_eq!(
            to_canonical_json(&ledger_entries[0]),
            to_canonical_json(&expected_ledger)
        );
    }

    #[test]
    fn simulated_failure_produces_failed_report_without_fills() {
        let plan = simulated_plan(CANDIDATE);
        let directives = [SimulationLegDirective::new(
            plan.legs[0].plan_leg_id.as_str(),
            SimulationLegOutcome::Failure(SimulatedFailure::new(
                FailureMode::VenueOutage,
                ExecutionFailureSeverity::RiskCritical,
                "event:sim:failure:venue-outage",
                "模拟场所不可用，执行腿失败。",
            )),
        )];
        let result = simulate_execution_with_script(SimulatedExecutionInput::new(
            &plan,
            "2026-01-01T00:00:05Z",
            &directives,
        ))
        .expect("failure simulation should produce a report");

        assert_eq!(result.report.status, ExecutionReportStatus::Failed);
        assert_eq!(
            result.report.reconciliation_status,
            ReconciliationStatus::Pending
        );
        assert!(result.report.fills.is_empty());
        assert_eq!(
            result.report.failures[0].failure_type,
            FailureMode::VenueOutage
        );
        assert_eq!(
            result.report.failures[0].severity,
            ExecutionFailureSeverity::RiskCritical
        );
        assert_eq!(result.report.leg_reports[0].status, LegReportStatus::Failed);
        assert!(result
            .leg_transitions
            .iter()
            .any(|transition| transition.to_state == ExecutionLegState::Failed));
    }

    #[test]
    fn simulated_unknown_state_is_risk_critical_and_requires_reconciliation() {
        let plan = simulated_plan(CANDIDATE);
        let directives = [SimulationLegDirective::new(
            plan.legs[0].plan_leg_id.as_str(),
            SimulationLegOutcome::Unknown(SimulatedUnknown::new(
                "event:sim:unknown:state",
                "模拟场所状态不可证明，不能按成功处理。",
            )),
        )];
        let result = simulate_execution_with_script(SimulatedExecutionInput::new(
            &plan,
            "2026-01-01T00:00:05Z",
            &directives,
        ))
        .expect("unknown-state simulation should produce a report");

        assert_eq!(result.report.status, ExecutionReportStatus::UnknownState);
        assert_eq!(
            result.report.reconciliation_status,
            ReconciliationStatus::Unknown
        );
        assert_eq!(
            result.report.leg_reports[0].status,
            LegReportStatus::Unknown
        );
        assert_eq!(
            result.report.failures[0].failure_type,
            FailureMode::UnknownState
        );
        assert_eq!(
            result.report.failures[0].severity,
            ExecutionFailureSeverity::RiskCritical
        );
        assert!(result
            .leg_transitions
            .iter()
            .any(|transition| transition.to_state == ExecutionLegState::Unknown));

        let expected: ExecutionReport =
            from_json_strict(UNKNOWN_STATE_REPORT.trim()).expect("expected unknown report fixture");
        assert_eq!(
            to_canonical_json(&result.report),
            to_canonical_json(&expected)
        );
        let ledger_entries = simulated_ledger_entries_from_execution_report(&plan, &result.report)
            .expect("unknown report without fills has no ledger entries");
        assert!(ledger_entries.is_empty());
    }

    #[test]
    fn private_stream_fill_generates_succeeded_report_live_ledger_and_no_incident() {
        let plan = pending_manual_plan().plan_preview;
        let order_leg = &plan.legs[1];
        let confirmation = PrivateOrderConfirmation::new(
            order_leg.plan_leg_id.as_str(),
            PrivateOrderConfirmationStatus::Filled,
            PrivateOrderConfirmationSource::PrivateStream,
            "event:binance:spot:execution-report:fill",
        )
        .with_venue_order_id("binance:spot:order:12345")
        .with_client_order_id("client:spot:1")
        .with_fill(
            PrivateExecutionFill::new(
                FillSide::Buy,
                "43100.50",
                "0.001",
                "asset:USDC",
                "0.01",
                "event:binance:spot:execution-report:fill",
            )
            .with_timestamp("2026-01-01T00:00:05Z"),
        );

        let report = execution_report_from_private_confirmations(PrivateExecutionReportInput::new(
            &plan,
            "2026-01-01T00:00:06Z",
            std::slice::from_ref(&confirmation),
        ))
        .expect("private confirmation report");

        assert_eq!(report.status, ExecutionReportStatus::Succeeded);
        assert_eq!(report.reconciliation_status, ReconciliationStatus::Matched);
        assert_eq!(report.fills.len(), 1);
        assert_eq!(
            report.fills[0].venue_order_id.as_deref(),
            Some("binance:spot:order:12345")
        );
        assert!(report.failures.is_empty());

        let ledger_entries = private_ledger_entries_from_execution_report(&plan, &report)
            .expect("private fills can generate live ledger input");
        assert_eq!(ledger_entries.len(), 1);
        assert_eq!(ledger_entries[0].namespace, LedgerNamespace::Live);
        assert_eq!(
            ledger_entries[0].execution_plan_id,
            Some(report.plan_id.clone())
        );

        let incidents =
            incidents_from_private_execution_report(&plan, &report, "2026-01-01T00:00:07Z")
                .expect("incident generation");
        assert!(incidents.is_empty());
    }

    #[test]
    fn private_acknowledgement_only_is_not_final_success() {
        let plan = pending_manual_plan().plan_preview;
        let order_leg = &plan.legs[1];
        let confirmation = PrivateOrderConfirmation::new(
            order_leg.plan_leg_id.as_str(),
            PrivateOrderConfirmationStatus::Acknowledged,
            PrivateOrderConfirmationSource::OrderQuery,
            "event:binance:spot:query:new",
        );

        let report = execution_report_from_private_confirmations(PrivateExecutionReportInput::new(
            &plan,
            "2026-01-01T00:00:06Z",
            std::slice::from_ref(&confirmation),
        ))
        .expect("ack-only report");

        assert_eq!(
            report.status,
            ExecutionReportStatus::ManualInterventionRequired
        );
        assert_eq!(report.reconciliation_status, ReconciliationStatus::Pending);
        assert!(report.fills.is_empty());
        assert_eq!(report.failures.len(), 1);
        assert_eq!(
            report.failures[0].failure_type,
            FailureMode::ManualInterventionRequired
        );

        let ledger_entries = private_ledger_entries_from_execution_report(&plan, &report)
            .expect("ack-only report has no private fills to ledger");
        assert!(ledger_entries.is_empty());
        let incidents =
            incidents_from_private_execution_report(&plan, &report, "2026-01-01T00:00:07Z")
                .expect("ack-only incident");
        assert_eq!(incidents.len(), 1);
        assert_eq!(incidents[0].trigger.as_str(), "EXECUTION_MANUAL_REVIEW");
    }

    #[test]
    fn missing_private_confirmation_enters_unknown_state() {
        let plan = pending_manual_plan().plan_preview;
        let report = execution_report_from_private_confirmations(PrivateExecutionReportInput::new(
            &plan,
            "2026-01-01T00:00:06Z",
            &[],
        ))
        .expect("missing confirmation report");

        assert_eq!(report.status, ExecutionReportStatus::UnknownState);
        assert_eq!(report.reconciliation_status, ReconciliationStatus::Unknown);
        assert_eq!(report.leg_reports[1].status, LegReportStatus::Unknown);
        assert_eq!(report.failures[0].failure_type, FailureMode::UnknownState);
        let incidents =
            incidents_from_private_execution_report(&plan, &report, "2026-01-01T00:00:07Z")
                .expect("unknown incident");
        assert_eq!(incidents.len(), 1);
        assert_eq!(incidents[0].trigger.as_str(), "EXECUTION_UNKNOWN_STATE");
    }

    #[test]
    fn simulated_timeout_enters_unknown_state_and_requires_reconciliation() {
        let plan = simulated_plan(CANDIDATE);
        let directives = [SimulationLegDirective::new(
            plan.legs[0].plan_leg_id.as_str(),
            SimulationLegOutcome::Timeout(SimulatedTimeout::new(
                "event:sim:timeout:state",
                DEFAULT_UNKNOWN_STATE_AFTER_MS,
                "场所确认在未知状态阈值内没有返回。",
            )),
        )];
        let result = simulate_execution_with_script(SimulatedExecutionInput::new(
            &plan,
            "2026-01-01T00:00:05Z",
            &directives,
        ))
        .expect("timeout simulation should produce a report");

        assert_eq!(result.report.status, ExecutionReportStatus::UnknownState);
        assert_eq!(
            result.report.reconciliation_status,
            ReconciliationStatus::Unknown
        );
        assert_eq!(
            result.report.failures[0].failure_type,
            FailureMode::UnknownState
        );
        assert_eq!(
            result.report.failures[0].severity,
            ExecutionFailureSeverity::RiskCritical
        );
        assert!(result.report.failures[0]
            .detail
            .as_ref()
            .expect("timeout detail")
            .contains("超时"));
        assert!(result
            .leg_transitions
            .iter()
            .any(|transition| transition.to_state == ExecutionLegState::Unknown));
    }

    #[test]
    fn simulated_partial_fill_can_cancel_remainder() {
        let plan = simulated_plan(CANDIDATE);
        let directives = [SimulationLegDirective::new(
            plan.legs[0].plan_leg_id.as_str(),
            SimulationLegOutcome::PartialFillThenCancel(
                SimulatedFill::new(
                    FillSide::Buy,
                    "1",
                    "0.5",
                    "asset:USDC",
                    "0.01",
                    "event:sim:cancel:partial-fill",
                ),
                SimulatedCancel::new("event:sim:cancel:requested", "模拟部分成交后撤销剩余数量。"),
            ),
        )];
        let result = simulate_execution_with_script(SimulatedExecutionInput::new(
            &plan,
            "2026-01-01T00:00:05Z",
            &directives,
        ))
        .expect("cancel simulation should produce a report");

        assert_eq!(
            result.report.status,
            ExecutionReportStatus::PartiallySucceeded
        );
        assert_eq!(
            result.report.leg_reports[0].status,
            LegReportStatus::Cancelled
        );
        assert!(result
            .leg_transitions
            .iter()
            .any(|transition| transition.to_state == ExecutionLegState::CancelRequested));
        assert!(result
            .leg_transitions
            .iter()
            .any(|transition| transition.to_state == ExecutionLegState::Cancelled));
    }

    #[test]
    fn simulated_compensation_path_keeps_unknown_state_visible() {
        let plan = simulated_plan(CANDIDATE);
        let directives = [SimulationLegDirective::new(
            plan.legs[0].plan_leg_id.as_str(),
            SimulationLegOutcome::CompensatedUnknown(
                SimulatedUnknown::new(
                    "event:sim:unknown:compensated",
                    "模拟未知状态触发补偿路径。",
                )
                .with_compensation_event_id("event:sim:unknown:compensating"),
            ),
        )];
        let result = simulate_execution_with_script(SimulatedExecutionInput::new(
            &plan,
            "2026-01-01T00:00:05Z",
            &directives,
        ))
        .expect("compensated unknown simulation should produce a report");

        assert_eq!(result.report.status, ExecutionReportStatus::UnknownState);
        assert_eq!(
            result.report.reconciliation_status,
            ReconciliationStatus::Unknown
        );
        assert_eq!(
            result.report.leg_reports[0].status,
            LegReportStatus::Unknown
        );
        assert!(result
            .report
            .failures
            .iter()
            .any(|failure| failure.failure_type == FailureMode::ManualInterventionRequired));
        assert!(result
            .leg_transitions
            .iter()
            .any(|transition| transition.to_state == ExecutionLegState::Compensating));
        assert!(result
            .leg_transitions
            .iter()
            .any(|transition| transition.to_state == ExecutionLegState::Compensated));
    }

    #[test]
    fn capital_reservation_requested_reserved_and_converted_path_is_replayable() {
        let first = converted_reservation_path();
        let second = converted_reservation_path();

        assert_eq!(
            to_canonical_json(&first.reservation),
            to_canonical_json(&second.reservation)
        );
        assert_eq!(first.from_state, Some(CapitalReservationState::Reserved));
        assert_eq!(
            first.to_state,
            CapitalReservationState::ConvertedToExecution
        );
        assert_eq!(
            first.reservation.state,
            CapitalReservationState::ConvertedToExecution
        );
        assert_reservation_refs(
            &first.reservation,
            "event:capital:convert:01",
            "ledger:capital:convert:01",
        );
        assert_eq!(first.source_event_id, "event:capital:convert:01");
        assert_eq!(first.ledger_entry_id, "ledger:capital:convert:01");
        assert!(!first.idempotent);
    }

    #[test]
    fn capital_reservation_mismatch_state_keeps_event_and_ledger_refs() {
        let requested = requested_reservation();
        let reserved = reserve_capital_reservation(
            &requested.reservation,
            refs("event:capital:reserve:02", "ledger:capital:reserve:02"),
        )
        .expect("requested reservation can be reserved");
        let mismatch = mark_capital_reservation_reconciled_mismatch(
            &reserved.reservation,
            refs("event:capital:mismatch:01", "ledger:capital:mismatch:01"),
        )
        .expect("reserved reservation can enter reconciliation mismatch");

        assert_eq!(mismatch.from_state, Some(CapitalReservationState::Reserved));
        assert_eq!(
            mismatch.to_state,
            CapitalReservationState::ReconciledMismatch
        );
        assert_eq!(
            mismatch.reservation.state,
            CapitalReservationState::ReconciledMismatch
        );
        assert_reservation_refs(
            &mismatch.reservation,
            "event:capital:mismatch:01",
            "ledger:capital:mismatch:01",
        );
    }

    #[test]
    fn capital_reservation_rejects_illegal_state_transition() {
        let requested = requested_reservation();
        let error = convert_capital_reservation_to_execution(
            &requested.reservation,
            refs("event:capital:convert:bad", "ledger:capital:convert:bad"),
        )
        .expect_err("requested reservation must be reserved before execution conversion");

        assert!(matches!(
            error,
            ExecutionError::InvalidCapitalReservationTransition {
                from: CapitalReservationState::Requested,
                to: CapitalReservationState::ConvertedToExecution,
                ..
            }
        ));
    }

    #[test]
    fn capital_reservation_release_is_idempotent() {
        let requested = requested_reservation();
        let reserved = reserve_capital_reservation(
            &requested.reservation,
            refs("event:capital:reserve:03", "ledger:capital:reserve:03"),
        )
        .expect("requested reservation can be reserved");
        let released = release_capital_reservation(
            &reserved.reservation,
            refs("event:capital:release:01", "ledger:capital:release:01"),
        )
        .expect("reserved reservation can be released");
        let repeated = release_capital_reservation(
            &released.reservation,
            refs(
                "event:capital:release:retry",
                "ledger:capital:release:retry",
            ),
        )
        .expect("release operation is idempotent");

        assert_eq!(
            released.reservation.state,
            CapitalReservationState::Released
        );
        assert_eq!(
            to_canonical_json(&released.reservation),
            to_canonical_json(&repeated.reservation)
        );
        assert!(repeated.idempotent);
        assert_eq!(repeated.source_event_id, "event:capital:release:retry");
        assert_eq!(repeated.ledger_entry_id, "ledger:capital:release:retry");
        assert_reservation_refs(
            &repeated.reservation,
            "event:capital:release:01",
            "ledger:capital:release:01",
        );
    }

    #[test]
    fn capital_reservation_expire_is_idempotent() {
        let requested = requested_reservation();
        let expired = expire_capital_reservation(
            &requested.reservation,
            refs("event:capital:expire:01", "ledger:capital:expire:01"),
        )
        .expect("requested reservation can expire");
        let repeated = expire_capital_reservation(
            &expired.reservation,
            refs("event:capital:expire:retry", "ledger:capital:expire:retry"),
        )
        .expect("expire operation is idempotent");

        assert_eq!(expired.reservation.state, CapitalReservationState::Expired);
        assert_eq!(
            to_canonical_json(&expired.reservation),
            to_canonical_json(&repeated.reservation)
        );
        assert!(repeated.idempotent);
        assert_eq!(repeated.source_event_id, "event:capital:expire:retry");
        assert_eq!(repeated.ledger_entry_id, "ledger:capital:expire:retry");
        assert_reservation_refs(
            &repeated.reservation,
            "event:capital:expire:01",
            "ledger:capital:expire:01",
        );
    }

    fn candidate(input: &str) -> CandidatePortfolioTransition {
        from_json_strict(input).expect("candidate fixture")
    }

    fn decision(input: &str) -> RiskDecision {
        from_json_strict(input.trim()).expect("risk decision fixture")
    }

    fn pending_manual_plan() -> PendingManualApprovalPlan {
        let candidate = candidate(CANDIDATE);
        let risk_decision = decision(&manual_decision_json());
        match build_execution_plan_preview(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::ReadOnly,
            "2026-01-01T00:00:04Z",
        ))
        .expect("manual approval preview")
        {
            PlanBuildOutcome::PendingManualApproval(pending) => pending,
            PlanBuildOutcome::Schedulable(_) => panic!("manual approval must not be schedulable"),
        }
    }

    fn simulated_plan(candidate_json: &str) -> ExecutionPlan {
        let candidate = candidate(candidate_json);
        let risk_decision = decision(APPROVED_DECISION);
        build_execution_plan(ExecutionPlanBuildInput::new(
            &risk_decision,
            &candidate,
            ExecutionMode::Simulated,
            "2026-01-01T00:00:04Z",
        ))
        .expect("approved decision should produce a simulated plan")
    }

    fn requested_reservation() -> CapitalReservationTransition {
        request_capital_reservation(CapitalReservationRequest::new(
            "reserve:capital:01",
            "asset:USDC",
            "100.00",
            "trans:01",
            "2026-01-01T00:05:00Z",
            refs("event:capital:request:01", "ledger:capital:request:01"),
        ))
        .expect("requested reservation")
    }

    fn converted_reservation_path() -> CapitalReservationTransition {
        let requested = requested_reservation();
        assert_eq!(requested.to_state, CapitalReservationState::Requested);
        assert_eq!(
            requested.reservation.state,
            CapitalReservationState::Requested
        );
        assert_reservation_refs(
            &requested.reservation,
            "event:capital:request:01",
            "ledger:capital:request:01",
        );

        let reserved = reserve_capital_reservation(
            &requested.reservation,
            refs("event:capital:reserve:01", "ledger:capital:reserve:01"),
        )
        .expect("requested reservation can be reserved");
        assert_eq!(
            reserved.from_state,
            Some(CapitalReservationState::Requested)
        );
        assert_eq!(reserved.to_state, CapitalReservationState::Reserved);
        assert_eq!(
            reserved.reservation.state,
            CapitalReservationState::Reserved
        );
        assert_reservation_refs(
            &reserved.reservation,
            "event:capital:reserve:01",
            "ledger:capital:reserve:01",
        );

        convert_capital_reservation_to_execution(
            &reserved.reservation,
            refs("event:capital:convert:01", "ledger:capital:convert:01"),
        )
        .expect("reserved reservation can convert to execution")
    }

    fn refs(
        source_event_id: &'static str,
        ledger_entry_id: &'static str,
    ) -> CapitalReservationRefs<'static> {
        CapitalReservationRefs::new(source_event_id, ledger_entry_id)
    }

    fn assert_reservation_refs(
        reservation: &CapitalReservation,
        expected_event_id: &str,
        expected_ledger_entry_id: &str,
    ) {
        assert_eq!(
            reservation
                .source_event_id
                .as_ref()
                .expect("source event ref")
                .as_str(),
            expected_event_id
        );
        assert_eq!(
            reservation
                .ledger_entry_id
                .as_ref()
                .expect("ledger entry ref")
                .as_str(),
            expected_ledger_entry_id
        );
    }

    fn manual_decision_json() -> String {
        APPROVED_DECISION
            .trim()
            .replace("\"decision\":\"Approved\"", "\"decision\":\"RequiresManualApproval\"")
            .replace("\"reason_codes\":[\"APPROVED\"]", "\"reason_codes\":[\"REQUIRES_MANUAL_APPROVAL\"]")
            .replace(
                "\"constraints\":[]",
                "\"constraints\":[{\"constraint_id\":\"constraint:manual:01\",\"constraint_type\":\"RequiresManualApproval\",\"field_path\":\"$.decision\",\"limit\":{\"string_value\":\"manual approval must reference the same plan hash\",\"unit\":\"approval_requirement\"}}]",
            )
            .replace(
                "风控入口批准候选转换；后续仍必须受执行模式、资本预留、kill switch 和权限约束。",
                "风控入口要求人工审批；审批前不得分发执行。",
            )
    }

    const TWO_LEG_CANDIDATE: &str = r#"{
      "schema_version": "1.0.0",
      "transition_id": "trans:01",
      "strategy_id": "strat:demo",
      "strategy_version": "1.0.0",
      "code_version": "code:demo-1",
      "config_version": "arb-config-v1",
      "created_at": "2026-01-01T00:00:02Z",
      "input_event_refs": ["event:01"],
      "current_portfolio_state_ref": "state:01",
      "holding_period": {"kind": "Instant"},
      "legs": [
        {
          "leg_id": "candleg:01",
          "leg_type": "Trade",
          "venue_id": "venue:SIM",
          "instrument_id": "inst:BTC-USDC",
          "account_id": "acct:sim",
          "side": "Buy",
          "asset_flows": [
            {
              "asset_id": "asset:USDC",
              "direction": "Out",
              "amount": "100.00",
              "account_id": "acct:sim"
            }
          ],
          "constraints": {"max_slippage_bps": "5"},
          "failure_modes": ["NoOpFailure"]
        },
        {
          "leg_id": "candleg:02",
          "leg_type": "Hedge",
          "venue_id": "venue:SIM",
          "instrument_id": "inst:BTC-PERP",
          "account_id": "acct:sim",
          "side": "Short",
          "asset_flows": [],
          "constraints": {},
          "failure_modes": ["PartialFill"]
        }
      ],
      "expected_post_state_delta": {
        "asset_flows": [],
        "position_deltas": [
          {
            "instrument_id": "inst:BTC-USDC",
            "account_id": "acct:sim",
            "quantity_delta": "0.001"
          }
        ]
      },
      "expected_economics": {
        "expected_profit_usd": "1.230000000000000001",
        "expected_profit_bps": "12.345678901234567890",
        "fee_estimate_usd": "0.10",
        "slippage_estimate_usd": "0.05",
        "confidence": 0.95
      },
      "required_capital": {
        "asset_requirements": [
          {
            "asset_id": "asset:USDC",
            "direction": "Out",
            "amount": "100.00",
            "account_id": "acct:sim"
          }
        ],
        "recovery_buffer_usd": "1.00"
      },
      "failure_modes": ["PartialFill"],
      "risk_flags": [],
      "assumptions": [
        {
          "assumption_id": "asm:01",
          "statement": "Fixture assumes deterministic offline prices.",
          "confidence": 0.9,
          "source_event_refs": ["event:01"]
        }
      ]
    }"#;

    const FUNDING_ARB_CANDIDATE: &str = r#"{
      "schema_version": "1.0.0",
      "transition_id": "trans:01",
      "strategy_id": "strat:demo",
      "strategy_version": "1.0.0",
      "code_version": "code:demo-1",
      "config_version": "arb-config-v1",
      "created_at": "2026-01-01T00:00:02Z",
      "input_event_refs": ["event:01"],
      "current_portfolio_state_ref": "state:01",
      "holding_period": {"kind": "UntilFundingTimestamp"},
      "legs": [
        {
          "leg_id": "candleg:funding:long",
          "leg_type": "Trade",
          "venue_id": "venue:BINANCE-USDM",
          "instrument_id": "inst:BINANCE:BTCUSDT:USDM-PERP",
          "account_id": "acct:binance-funding",
          "side": "Long",
          "asset_flows": [],
          "constraints": {
            "basis_leg_role": "perp_long",
            "notional_usd": "100.00",
            "reference_best_ask": "100.00",
            "reference_best_bid": "99.95",
            "venue_symbol": "BTCUSDT"
          },
          "failure_modes": ["PartialFill"]
        },
        {
          "leg_id": "candleg:funding:short",
          "leg_type": "Trade",
          "venue_id": "venue:BYBIT-LINEAR",
          "instrument_id": "inst:BYBIT:BTCUSDT:LINEAR-PERP",
          "account_id": "acct:bybit-funding",
          "side": "Short",
          "asset_flows": [],
          "constraints": {
            "basis_leg_role": "perp_short",
            "notional_usd": "100.00",
            "reference_best_ask": "100.10",
            "reference_best_bid": "100.05",
            "venue_symbol": "BTCUSDT"
          },
          "failure_modes": ["PartialFill"]
        }
      ],
      "expected_post_state_delta": {
        "asset_flows": [],
        "position_deltas": [
          {
            "instrument_id": "inst:BINANCE:BTCUSDT:USDM-PERP",
            "account_id": "acct:binance-funding",
            "quantity_delta": "1"
          },
          {
            "instrument_id": "inst:BYBIT:BTCUSDT:LINEAR-PERP",
            "account_id": "acct:bybit-funding",
            "quantity_delta": "-1"
          }
        ]
      },
      "expected_economics": {
        "expected_profit_usd": "0.14",
        "expected_profit_bps": "14",
        "fee_estimate_usd": "0.11",
        "slippage_estimate_usd": "0.04",
        "confidence": 0.70
      },
      "required_capital": {
        "asset_requirements": [
          {
            "asset_id": "asset:USDT",
            "direction": "Out",
            "amount": "100.00",
            "account_id": "acct:binance-funding"
          },
          {
            "asset_id": "asset:USDT",
            "direction": "Out",
            "amount": "100.00",
            "account_id": "acct:bybit-funding"
          }
        ],
        "recovery_buffer_usd": "2.00"
      },
      "margin_impact": {"summary": "dry-run margin placeholder", "impact_usd": "0", "confidence": 0.90},
      "funding_impact": {"summary": "funding spread", "impact_usd": "0.14", "confidence": 0.70},
      "liquidity_impact": {"summary": "top-of-book depth checked", "impact_usd": "0.04", "confidence": 0.90},
      "failure_modes": ["PartialFill"],
      "risk_flags": [],
      "assumptions": [
        {
          "assumption_id": "asm:funding:01",
          "statement": "Fixture uses public funding and top-of-book data only.",
          "confidence": 0.7,
          "source_event_refs": ["event:01"]
        }
      ]
    }"#;
}
