//! `arb-reconciliation` 对账和差异发现核心模块。
//!
//! 中文说明：本 crate 只读取账本视图、只读余额/仓位/成交/资本预留快照，
//! 输出对账报告、差异项和事故建议；不改写账本历史、不下单补偿、不调用签名，
//! 也不绕过事故流程。

#![forbid(unsafe_code)]

use arb_domain::{
    AccountId, Amount, AssetId, CandidateTransitionId, CapitalReservationStatus, Decimal,
    DomainError, EventId, ExecutionPlanId, IncidentSeverity, IncidentStatus, InstrumentId,
    LedgerEntryId, Price, Quantity, StrategyId, UtcTimestamp, VenueId,
};
use arb_ledger::{BalanceView, LedgerEntry, LedgerError, LedgerNamespace};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::str::FromStr;

/// 对账模块统一返回类型。
///
/// 中文说明：只有输入非法或内部十进制运算失败才返回错误；业务差异会进入
/// `ReconciliationReport`，不能被静默吞掉。
pub type ReconciliationResult<T> = Result<T, ReconciliationError>;

/// 对账核心错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconciliationError {
    /// 领域类型解析或十进制运算失败。
    Domain(DomainError),
    /// 账本视图推导失败。
    Ledger(LedgerError),
    /// 本地 ID 非法。
    InvalidIdentifier {
        type_name: &'static str,
        value: String,
        reason: &'static str,
    },
    /// 策略参数非法。
    InvalidPolicy {
        field: &'static str,
        reason: &'static str,
    },
    /// 十进制比较无法在安全范围内完成。
    DecimalComparisonOverflow { left: Decimal, right: Decimal },
}

impl fmt::Display for ReconciliationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(error) => write!(f, "{error}"),
            Self::Ledger(error) => write!(f, "{error}"),
            Self::InvalidIdentifier {
                type_name,
                value,
                reason,
            } => write!(f, "{type_name} `{value}` is invalid: {reason}"),
            Self::InvalidPolicy { field, reason } => {
                write!(
                    f,
                    "reconciliation policy field `{field}` is invalid: {reason}"
                )
            }
            Self::DecimalComparisonOverflow { left, right } => {
                write!(f, "cannot compare decimals `{left}` and `{right}` safely")
            }
        }
    }
}

impl Error for ReconciliationError {}

impl From<DomainError> for ReconciliationError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<LedgerError> for ReconciliationError {
    fn from(error: LedgerError) -> Self {
        Self::Ledger(error)
    }
}

macro_rules! define_reconciliation_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        ///
        /// 中文说明：对账本地 ID 使用稳定 ASCII 字符串，便于报告和事故追踪。
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> ReconciliationResult<Self> {
                let value = value.into();
                validate_identifier(stringify!($name), &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = ReconciliationError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }
    };
}

define_reconciliation_id!(ReconciliationRunId, "对账运行 ID。");
define_reconciliation_id!(ReconciliationDifferenceId, "对账差异 ID。");
define_reconciliation_id!(IncidentSuggestionId, "事故建议 ID。");
define_reconciliation_id!(FillId, "成交 ID。");
define_reconciliation_id!(CapitalReservationId, "资本预留 ID。");

fn validate_identifier(type_name: &'static str, value: &str) -> ReconciliationResult<()> {
    if value.is_empty() {
        return Err(ReconciliationError::InvalidIdentifier {
            type_name,
            value: value.to_owned(),
            reason: "value cannot be empty",
        });
    }

    if value.bytes().any(|byte| {
        !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/'))
    }) {
        return Err(ReconciliationError::InvalidIdentifier {
            type_name,
            value: value.to_owned(),
            reason:
                "only ASCII letters, digits, underscore, dash, dot, colon and slash are allowed",
        });
    }

    Ok(())
}

/// 对账整体状态。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReconciliationStatus {
    Matched,
    Mismatch,
    IncidentRecommended,
}

impl ReconciliationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Matched => "Matched",
            Self::Mismatch => "Mismatch",
            Self::IncidentRecommended => "IncidentRecommended",
        }
    }
}

/// 差异所属业务类别。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DifferenceCategory {
    Balance,
    Position,
    Fill,
    Ledger,
    CapitalReservation,
}

impl DifferenceCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Balance => "Balance",
            Self::Position => "Position",
            Self::Fill => "Fill",
            Self::Ledger => "Ledger",
            Self::CapitalReservation => "CapitalReservation",
        }
    }
}

/// 机器可聚合的差异分类。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DifferenceClassification {
    ValueMismatch,
    StateMismatch,
    MissingExternal,
    MissingInternal,
    LedgerLinkMissing,
    InvalidLedger,
    DuplicateInternal,
    UnknownExternalState,
}

impl DifferenceClassification {
    pub fn as_reason_code(self) -> &'static str {
        match self {
            Self::ValueMismatch => "RECON_VALUE_MISMATCH",
            Self::StateMismatch => "RECON_STATE_MISMATCH",
            Self::MissingExternal => "RECON_MISSING_EXTERNAL",
            Self::MissingInternal => "RECON_MISSING_INTERNAL",
            Self::LedgerLinkMissing => "RECON_LEDGER_LINK_MISSING",
            Self::InvalidLedger => "RECON_INVALID_LEDGER",
            Self::DuplicateInternal => "RECON_DUPLICATE_INTERNAL",
            Self::UnknownExternalState => "UNKNOWN_STATE",
        }
    }
}

/// 差异严重等级。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DifferenceSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl DifferenceSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Critical => "Critical",
        }
    }

    pub fn requires_incident(self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }

    fn to_incident_severity(self) -> IncidentSeverity {
        match self {
            Self::Low => IncidentSeverity::Low,
            Self::Medium => IncidentSeverity::Medium,
            Self::High => IncidentSeverity::High,
            Self::Critical => IncidentSeverity::Critical,
        }
    }
}

/// 对账容忍阈值。
///
/// 中文说明：阈值只用于避免舍入或场所最小单位导致的误报，不能用于忽略未知
/// 外部状态、缺失成交或账本链接缺失。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconciliationPolicy {
    pub balance_tolerance: Decimal,
    pub position_tolerance: Decimal,
    pub fill_quantity_tolerance: Decimal,
    pub fill_price_tolerance: Decimal,
    pub fee_tolerance: Decimal,
    pub capital_reservation_tolerance: Decimal,
    pub critical_difference_threshold: Decimal,
}

impl Default for ReconciliationPolicy {
    fn default() -> Self {
        Self {
            balance_tolerance: zero(),
            position_tolerance: zero(),
            fill_quantity_tolerance: zero(),
            fill_price_tolerance: zero(),
            fee_tolerance: zero(),
            capital_reservation_tolerance: zero(),
            critical_difference_threshold: Decimal::from_scaled_atoms(1, 0),
        }
    }
}

impl ReconciliationPolicy {
    pub fn with_balance_tolerance(mut self, tolerance: Decimal) -> Self {
        self.balance_tolerance = tolerance;
        self
    }

    pub fn with_position_tolerance(mut self, tolerance: Decimal) -> Self {
        self.position_tolerance = tolerance;
        self
    }

    pub fn with_fill_quantity_tolerance(mut self, tolerance: Decimal) -> Self {
        self.fill_quantity_tolerance = tolerance;
        self
    }

    pub fn with_fill_price_tolerance(mut self, tolerance: Decimal) -> Self {
        self.fill_price_tolerance = tolerance;
        self
    }

    pub fn with_fee_tolerance(mut self, tolerance: Decimal) -> Self {
        self.fee_tolerance = tolerance;
        self
    }

    pub fn with_capital_reservation_tolerance(mut self, tolerance: Decimal) -> Self {
        self.capital_reservation_tolerance = tolerance;
        self
    }

    pub fn with_critical_difference_threshold(mut self, threshold: Decimal) -> Self {
        self.critical_difference_threshold = threshold;
        self
    }

    fn validate(self) -> ReconciliationResult<()> {
        let values = [
            ("balance_tolerance", self.balance_tolerance),
            ("position_tolerance", self.position_tolerance),
            ("fill_quantity_tolerance", self.fill_quantity_tolerance),
            ("fill_price_tolerance", self.fill_price_tolerance),
            ("fee_tolerance", self.fee_tolerance),
            (
                "capital_reservation_tolerance",
                self.capital_reservation_tolerance,
            ),
            (
                "critical_difference_threshold",
                self.critical_difference_threshold,
            ),
        ];

        for (field, value) in values {
            if value.is_negative() {
                return Err(ReconciliationError::InvalidPolicy {
                    field,
                    reason: "tolerance and severity thresholds must be non-negative",
                });
            }
        }
        Ok(())
    }
}

/// 余额对账键。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BalanceReconciliationKey {
    pub namespace: LedgerNamespace,
    pub account_id: AccountId,
    pub asset_id: AssetId,
}

/// 只读余额快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadOnlyBalanceSnapshot {
    pub key: BalanceReconciliationKey,
    pub reported_total: Decimal,
    pub source_event_id: Option<EventId>,
}

impl ReadOnlyBalanceSnapshot {
    pub fn new(
        namespace: LedgerNamespace,
        account_id: AccountId,
        asset_id: AssetId,
        reported_total: Decimal,
    ) -> Self {
        Self {
            key: BalanceReconciliationKey {
                namespace,
                account_id,
                asset_id,
            },
            reported_total,
            source_event_id: None,
        }
    }

    pub fn with_source_event_id(mut self, source_event_id: EventId) -> Self {
        self.source_event_id = Some(source_event_id);
        self
    }
}

/// 仓位对账键。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PositionReconciliationKey {
    pub account_id: AccountId,
    pub instrument_id: InstrumentId,
}

/// 仓位快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PositionSnapshot {
    pub key: PositionReconciliationKey,
    pub quantity: Decimal,
    pub source_event_id: Option<EventId>,
}

impl PositionSnapshot {
    pub fn new(account_id: AccountId, instrument_id: InstrumentId, quantity: Decimal) -> Self {
        Self {
            key: PositionReconciliationKey {
                account_id,
                instrument_id,
            },
            quantity,
            source_event_id: None,
        }
    }

    pub fn with_source_event_id(mut self, source_event_id: EventId) -> Self {
        self.source_event_id = Some(source_event_id);
        self
    }
}

/// 成交对账键。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FillReconciliationKey {
    pub fill_id: FillId,
}

/// 成交快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FillSnapshot {
    pub key: FillReconciliationKey,
    pub plan_id: ExecutionPlanId,
    pub venue_id: VenueId,
    pub instrument_id: InstrumentId,
    pub price: Price,
    pub quantity: Quantity,
    pub fee_asset_id: AssetId,
    pub fee_amount: Amount,
    pub ledger_entry_id: Option<LedgerEntryId>,
    pub source_event_id: Option<EventId>,
}

impl FillSnapshot {
    pub fn new(
        fill_id: FillId,
        plan_id: ExecutionPlanId,
        venue_id: VenueId,
        instrument_id: InstrumentId,
        price: Price,
        quantity: Quantity,
        fee: FeeAmount,
    ) -> Self {
        Self {
            key: FillReconciliationKey { fill_id },
            plan_id,
            venue_id,
            instrument_id,
            price,
            quantity,
            fee_asset_id: fee.asset_id,
            fee_amount: fee.amount,
            ledger_entry_id: None,
            source_event_id: None,
        }
    }

    pub fn with_ledger_entry_id(mut self, ledger_entry_id: LedgerEntryId) -> Self {
        self.ledger_entry_id = Some(ledger_entry_id);
        self
    }

    pub fn with_source_event_id(mut self, source_event_id: EventId) -> Self {
        self.source_event_id = Some(source_event_id);
        self
    }
}

/// 成交费用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeAmount {
    pub asset_id: AssetId,
    pub amount: Amount,
}

impl FeeAmount {
    pub fn new(asset_id: AssetId, amount: Amount) -> Self {
        Self { asset_id, amount }
    }
}

/// 资本预留对账键。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CapitalReservationKey {
    pub reservation_id: CapitalReservationId,
}

/// 资本预留快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapitalReservationSnapshot {
    pub key: CapitalReservationKey,
    pub state: CapitalReservationStatus,
    pub asset_id: AssetId,
    pub amount: Amount,
    pub reserved_for: CandidateTransitionId,
    pub ledger_entry_id: Option<LedgerEntryId>,
    pub source_event_id: Option<EventId>,
}

impl CapitalReservationSnapshot {
    pub fn new(
        reservation_id: CapitalReservationId,
        state: CapitalReservationStatus,
        asset_id: AssetId,
        amount: Amount,
        reserved_for: CandidateTransitionId,
    ) -> Self {
        Self {
            key: CapitalReservationKey { reservation_id },
            state,
            asset_id,
            amount,
            reserved_for,
            ledger_entry_id: None,
            source_event_id: None,
        }
    }

    pub fn with_ledger_entry_id(mut self, ledger_entry_id: LedgerEntryId) -> Self {
        self.ledger_entry_id = Some(ledger_entry_id);
        self
    }

    pub fn with_source_event_id(mut self, source_event_id: EventId) -> Self {
        self.source_event_id = Some(source_event_id);
        self
    }
}

/// 对账请求。
///
/// 中文说明：请求只持有只读引用；对账核心没有任何账本写入、下单或签名入口。
pub struct ReconciliationRequest<'a> {
    pub run_id: ReconciliationRunId,
    pub as_of: UtcTimestamp,
    pub ledger_entries: &'a [LedgerEntry],
    pub observed_balances: &'a [ReadOnlyBalanceSnapshot],
    pub expected_positions: &'a [PositionSnapshot],
    pub observed_positions: &'a [PositionSnapshot],
    pub expected_fills: &'a [FillSnapshot],
    pub observed_fills: &'a [FillSnapshot],
    pub expected_reservations: &'a [CapitalReservationSnapshot],
    pub observed_reservations: &'a [CapitalReservationSnapshot],
}

impl<'a> ReconciliationRequest<'a> {
    pub fn new(
        run_id: ReconciliationRunId,
        as_of: UtcTimestamp,
        ledger_entries: &'a [LedgerEntry],
    ) -> Self {
        Self {
            run_id,
            as_of,
            ledger_entries,
            observed_balances: &[],
            expected_positions: &[],
            observed_positions: &[],
            expected_fills: &[],
            observed_fills: &[],
            expected_reservations: &[],
            observed_reservations: &[],
        }
    }
}

/// 差异影响范围。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DifferenceScope {
    pub namespace: Option<LedgerNamespace>,
    pub account_id: Option<AccountId>,
    pub asset_id: Option<AssetId>,
    pub instrument_id: Option<InstrumentId>,
    pub fill_id: Option<FillId>,
    pub reservation_id: Option<CapitalReservationId>,
    pub ledger_entry_id: Option<LedgerEntryId>,
    pub venue_id: Option<VenueId>,
    pub strategy_id: Option<StrategyId>,
}

impl DifferenceScope {
    fn from_balance_key(key: &BalanceReconciliationKey) -> Self {
        Self {
            namespace: Some(key.namespace),
            account_id: Some(key.account_id.clone()),
            asset_id: Some(key.asset_id.clone()),
            ..Self::default()
        }
    }

    fn from_position_key(key: &PositionReconciliationKey) -> Self {
        Self {
            account_id: Some(key.account_id.clone()),
            instrument_id: Some(key.instrument_id.clone()),
            ..Self::default()
        }
    }

    fn from_fill(fill: &FillSnapshot) -> Self {
        Self {
            fill_id: Some(fill.key.fill_id.clone()),
            instrument_id: Some(fill.instrument_id.clone()),
            ledger_entry_id: fill.ledger_entry_id.clone(),
            venue_id: Some(fill.venue_id.clone()),
            ..Self::default()
        }
    }

    fn from_reservation(reservation: &CapitalReservationSnapshot) -> Self {
        Self {
            reservation_id: Some(reservation.key.reservation_id.clone()),
            asset_id: Some(reservation.asset_id.clone()),
            ledger_entry_id: reservation.ledger_entry_id.clone(),
            ..Self::default()
        }
    }
}

/// 对账差异项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationDifference {
    pub difference_id: ReconciliationDifferenceId,
    pub category: DifferenceCategory,
    pub classification: DifferenceClassification,
    pub severity: DifferenceSeverity,
    pub reason_code: String,
    pub scope: DifferenceScope,
    pub expected_value: Option<Decimal>,
    pub observed_value: Option<Decimal>,
    pub difference_value: Option<Decimal>,
    pub absolute_difference: Option<Decimal>,
    pub tolerance: Option<Decimal>,
    pub source_event_ids: Vec<EventId>,
    pub ledger_entry_ids: Vec<LedgerEntryId>,
    pub description: String,
}

/// 事故建议动作。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IncidentSuggestedAction {
    OpenIncidentRecord,
    PauseAffectedScopeUntilReviewed,
    RefreshReadOnlySnapshots,
    ReviewAppendOnlyLedgerAdjustment,
    ManualInvestigation,
}

impl IncidentSuggestedAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenIncidentRecord => "OpenIncidentRecord",
            Self::PauseAffectedScopeUntilReviewed => "PauseAffectedScopeUntilReviewed",
            Self::RefreshReadOnlySnapshots => "RefreshReadOnlySnapshots",
            Self::ReviewAppendOnlyLedgerAdjustment => "ReviewAppendOnlyLedgerAdjustment",
            Self::ManualInvestigation => "ManualInvestigation",
        }
    }
}

/// 事故建议影响范围。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IncidentImpactedScope {
    pub venue_ids: Vec<VenueId>,
    pub strategy_ids: Vec<StrategyId>,
    pub account_ids: Vec<AccountId>,
    pub asset_ids: Vec<AssetId>,
    pub ledger_entry_ids: Vec<LedgerEntryId>,
    pub capital_at_risk: Option<Decimal>,
}

/// 事故建议。
///
/// 中文说明：这是事故流程输入建议，不会自动修改账本、提交补偿订单或调用签名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncidentSuggestion {
    pub suggestion_id: IncidentSuggestionId,
    pub severity: IncidentSeverity,
    pub status: IncidentStatus,
    pub opened_at: UtcTimestamp,
    pub trigger_reason_code: String,
    pub source_difference_ids: Vec<ReconciliationDifferenceId>,
    pub impacted: IncidentImpactedScope,
    pub suggested_actions: Vec<IncidentSuggestedAction>,
}

/// 对账摘要，供运营报告只读读取。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationSummary {
    pub status: ReconciliationStatus,
    pub checked_balances: usize,
    pub checked_positions: usize,
    pub checked_fills: usize,
    pub checked_ledger_entries: usize,
    pub checked_capital_reservations: usize,
    pub difference_count: usize,
    pub incident_suggestion_count: usize,
    pub highest_severity: Option<DifferenceSeverity>,
}

/// 对账报告。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationReport {
    pub run_id: ReconciliationRunId,
    pub as_of: UtcTimestamp,
    pub summary: ReconciliationSummary,
    pub differences: Vec<ReconciliationDifference>,
    pub incident_suggestions: Vec<IncidentSuggestion>,
}

impl ReconciliationReport {
    pub fn status(&self) -> ReconciliationStatus {
        self.summary.status
    }

    pub fn has_differences(&self) -> bool {
        !self.differences.is_empty()
    }

    pub fn unresolved_differences(&self) -> &[ReconciliationDifference] {
        &self.differences
    }

    pub fn has_incident_suggestions(&self) -> bool {
        !self.incident_suggestions.is_empty()
    }
}

/// 对账运行接口。
pub trait ReconciliationRunner {
    fn run(&self, request: ReconciliationRequest<'_>)
        -> ReconciliationResult<ReconciliationReport>;
}

/// 默认对账核心实现。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CoreReconciliationRunner {
    policy: ReconciliationPolicy,
}

impl CoreReconciliationRunner {
    pub fn new(policy: ReconciliationPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(self) -> ReconciliationPolicy {
        self.policy
    }

    fn reconcile_ledger_entries(
        self,
        builder: &mut ReportBuilder,
        ledger_entries: &[LedgerEntry],
    ) -> ReconciliationResult<BTreeSet<LedgerEntryId>> {
        let mut ledger_entry_ids = BTreeSet::new();
        for entry in ledger_entries {
            if !ledger_entry_ids.insert(entry.ledger_entry_id.clone()) {
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Ledger,
                    classification: DifferenceClassification::DuplicateInternal,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope {
                        ledger_entry_id: Some(entry.ledger_entry_id.clone()),
                        ..DifferenceScope::default()
                    },
                    expected_value: None,
                    observed_value: None,
                    difference_value: None,
                    absolute_difference: None,
                    tolerance: None,
                    source_event_ids: vec![entry.source_event_id.clone()],
                    ledger_entry_ids: vec![entry.ledger_entry_id.clone()],
                    description: "ledger entry id appears more than once in reconciliation input"
                        .to_owned(),
                })?;
            }
        }
        builder.checked_ledger_entries = ledger_entries.len();
        Ok(ledger_entry_ids)
    }

    fn reconcile_balances(
        self,
        builder: &mut ReportBuilder,
        balance_view: &BalanceView,
        observed_balances: &[ReadOnlyBalanceSnapshot],
    ) -> ReconciliationResult<()> {
        let mut observed_by_key = BTreeMap::new();
        for observed in observed_balances {
            if observed_by_key
                .insert(observed.key.clone(), observed)
                .is_some()
            {
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Balance,
                    classification: DifferenceClassification::UnknownExternalState,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_balance_key(&observed.key),
                    expected_value: None,
                    observed_value: Some(observed.reported_total),
                    difference_value: None,
                    absolute_difference: None,
                    tolerance: Some(self.policy.balance_tolerance),
                    source_event_ids: optional_event(observed.source_event_id.as_ref()),
                    ledger_entry_ids: Vec::new(),
                    description:
                        "duplicate read-only balance snapshot makes external state ambiguous"
                            .to_owned(),
                })?;
            }
        }

        let mut expected_by_key = BTreeMap::new();
        for row in balance_view.rows() {
            expected_by_key.insert(
                BalanceReconciliationKey {
                    namespace: row.namespace,
                    account_id: row.account_id.clone(),
                    asset_id: row.asset_id.clone(),
                },
                row.net_debit,
            );
        }

        for (key, expected) in &expected_by_key {
            match observed_by_key.get(key) {
                Some(observed) => self.push_amount_mismatch_if_needed(
                    builder,
                    AmountMismatchInput {
                        category: DifferenceCategory::Balance,
                        classification: DifferenceClassification::ValueMismatch,
                        scope: DifferenceScope::from_balance_key(key),
                        expected: *expected,
                        observed: observed.reported_total,
                        tolerance: self.policy.balance_tolerance,
                        source_event_ids: optional_event(observed.source_event_id.as_ref()),
                        ledger_entry_ids: Vec::new(),
                        description: "ledger-derived balance and read-only venue balance differ"
                            .to_owned(),
                    },
                )?,
                None if !decimal_within_abs(*expected, self.policy.balance_tolerance)? => {
                    let signed = zero().checked_sub(*expected)?;
                    let absolute = abs_decimal(signed)?;
                    builder.push_difference(DifferenceInput {
                        category: DifferenceCategory::Balance,
                        classification: DifferenceClassification::MissingExternal,
                        severity: DifferenceSeverity::Critical,
                        scope: DifferenceScope::from_balance_key(key),
                        expected_value: Some(*expected),
                        observed_value: None,
                        difference_value: Some(signed),
                        absolute_difference: Some(absolute),
                        tolerance: Some(self.policy.balance_tolerance),
                        source_event_ids: Vec::new(),
                        ledger_entry_ids: Vec::new(),
                        description:
                            "ledger has a non-zero balance with no matching read-only snapshot"
                                .to_owned(),
                    })?;
                }
                None => {}
            }
        }

        for (key, observed) in observed_by_key {
            if !expected_by_key.contains_key(&key)
                && !decimal_within_abs(observed.reported_total, self.policy.balance_tolerance)?
            {
                let absolute = abs_decimal(observed.reported_total)?;
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Balance,
                    classification: DifferenceClassification::MissingInternal,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_balance_key(&key),
                    expected_value: None,
                    observed_value: Some(observed.reported_total),
                    difference_value: Some(observed.reported_total),
                    absolute_difference: Some(absolute),
                    tolerance: Some(self.policy.balance_tolerance),
                    source_event_ids: optional_event(observed.source_event_id.as_ref()),
                    ledger_entry_ids: Vec::new(),
                    description: "read-only venue balance has no matching ledger-derived row"
                        .to_owned(),
                })?;
            }
        }

        builder.checked_balances = expected_by_key.len().max(observed_balances.len());
        Ok(())
    }

    fn reconcile_positions(
        self,
        builder: &mut ReportBuilder,
        expected_positions: &[PositionSnapshot],
        observed_positions: &[PositionSnapshot],
    ) -> ReconciliationResult<()> {
        let mut expected_by_key = BTreeMap::new();
        for expected in expected_positions {
            if expected_by_key
                .insert(expected.key.clone(), expected)
                .is_some()
            {
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Position,
                    classification: DifferenceClassification::DuplicateInternal,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_position_key(&expected.key),
                    expected_value: Some(expected.quantity),
                    observed_value: None,
                    difference_value: None,
                    absolute_difference: None,
                    tolerance: Some(self.policy.position_tolerance),
                    source_event_ids: optional_event(expected.source_event_id.as_ref()),
                    ledger_entry_ids: Vec::new(),
                    description: "duplicate expected position makes internal state ambiguous"
                        .to_owned(),
                })?;
            }
        }

        let mut observed_by_key = BTreeMap::new();
        for observed in observed_positions {
            if observed_by_key
                .insert(observed.key.clone(), observed)
                .is_some()
            {
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Position,
                    classification: DifferenceClassification::UnknownExternalState,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_position_key(&observed.key),
                    expected_value: None,
                    observed_value: Some(observed.quantity),
                    difference_value: None,
                    absolute_difference: None,
                    tolerance: Some(self.policy.position_tolerance),
                    source_event_ids: optional_event(observed.source_event_id.as_ref()),
                    ledger_entry_ids: Vec::new(),
                    description:
                        "duplicate read-only position snapshot makes external state ambiguous"
                            .to_owned(),
                })?;
            }
        }

        for (key, expected) in &expected_by_key {
            match observed_by_key.get(key) {
                Some(observed) => self.push_amount_mismatch_if_needed(
                    builder,
                    AmountMismatchInput {
                        category: DifferenceCategory::Position,
                        classification: DifferenceClassification::ValueMismatch,
                        scope: DifferenceScope::from_position_key(key),
                        expected: expected.quantity,
                        observed: observed.quantity,
                        tolerance: self.policy.position_tolerance,
                        source_event_ids: merge_events(
                            expected.source_event_id.as_ref(),
                            observed.source_event_id.as_ref(),
                        ),
                        ledger_entry_ids: Vec::new(),
                        description:
                            "expected position quantity and read-only venue position differ"
                                .to_owned(),
                    },
                )?,
                None if !decimal_within_abs(expected.quantity, self.policy.position_tolerance)? => {
                    let signed = zero().checked_sub(expected.quantity)?;
                    let absolute = abs_decimal(signed)?;
                    builder.push_difference(DifferenceInput {
                        category: DifferenceCategory::Position,
                        classification: DifferenceClassification::MissingExternal,
                        severity: DifferenceSeverity::Critical,
                        scope: DifferenceScope::from_position_key(key),
                        expected_value: Some(expected.quantity),
                        observed_value: None,
                        difference_value: Some(signed),
                        absolute_difference: Some(absolute),
                        tolerance: Some(self.policy.position_tolerance),
                        source_event_ids: optional_event(expected.source_event_id.as_ref()),
                        ledger_entry_ids: Vec::new(),
                        description:
                            "internal state has a non-zero position missing from read-only venue snapshot"
                                .to_owned(),
                    })?;
                }
                None => {}
            }
        }

        for (key, observed) in observed_by_key {
            if !expected_by_key.contains_key(&key)
                && !decimal_within_abs(observed.quantity, self.policy.position_tolerance)?
            {
                let absolute = abs_decimal(observed.quantity)?;
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Position,
                    classification: DifferenceClassification::MissingInternal,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_position_key(&key),
                    expected_value: None,
                    observed_value: Some(observed.quantity),
                    difference_value: Some(observed.quantity),
                    absolute_difference: Some(absolute),
                    tolerance: Some(self.policy.position_tolerance),
                    source_event_ids: optional_event(observed.source_event_id.as_ref()),
                    ledger_entry_ids: Vec::new(),
                    description: "read-only venue position has no matching internal position"
                        .to_owned(),
                })?;
            }
        }

        builder.checked_positions = expected_positions.len().max(observed_positions.len());
        Ok(())
    }

    fn reconcile_fills(
        self,
        builder: &mut ReportBuilder,
        ledger_entry_ids: &BTreeSet<LedgerEntryId>,
        expected_fills: &[FillSnapshot],
        observed_fills: &[FillSnapshot],
    ) -> ReconciliationResult<()> {
        let mut expected_by_key = BTreeMap::new();
        for expected in expected_fills {
            if expected_by_key
                .insert(expected.key.clone(), expected)
                .is_some()
            {
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Fill,
                    classification: DifferenceClassification::DuplicateInternal,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_fill(expected),
                    expected_value: Some(expected.quantity.as_decimal()),
                    observed_value: None,
                    difference_value: None,
                    absolute_difference: None,
                    tolerance: Some(self.policy.fill_quantity_tolerance),
                    source_event_ids: optional_event(expected.source_event_id.as_ref()),
                    ledger_entry_ids: optional_ledger(expected.ledger_entry_id.as_ref()),
                    description: "duplicate expected fill makes internal execution state ambiguous"
                        .to_owned(),
                })?;
            }

            match expected.ledger_entry_id.as_ref() {
                Some(ledger_entry_id) if !ledger_entry_ids.contains(ledger_entry_id) => {
                    builder.push_difference(DifferenceInput {
                        category: DifferenceCategory::Ledger,
                        classification: DifferenceClassification::LedgerLinkMissing,
                        severity: DifferenceSeverity::Critical,
                        scope: DifferenceScope::from_fill(expected),
                        expected_value: None,
                        observed_value: None,
                        difference_value: None,
                        absolute_difference: None,
                        tolerance: None,
                        source_event_ids: optional_event(expected.source_event_id.as_ref()),
                        ledger_entry_ids: vec![ledger_entry_id.clone()],
                        description:
                            "fill references a ledger entry that is absent from the ledger slice"
                                .to_owned(),
                    })?;
                }
                None => builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Ledger,
                    classification: DifferenceClassification::LedgerLinkMissing,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_fill(expected),
                    expected_value: None,
                    observed_value: None,
                    difference_value: None,
                    absolute_difference: None,
                    tolerance: None,
                    source_event_ids: optional_event(expected.source_event_id.as_ref()),
                    ledger_entry_ids: Vec::new(),
                    description: "expected fill is not linked to an append-only ledger entry"
                        .to_owned(),
                })?,
                Some(_) => {}
            }
        }

        let mut observed_by_key = BTreeMap::new();
        for observed in observed_fills {
            if observed_by_key
                .insert(observed.key.clone(), observed)
                .is_some()
            {
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Fill,
                    classification: DifferenceClassification::UnknownExternalState,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_fill(observed),
                    expected_value: None,
                    observed_value: Some(observed.quantity.as_decimal()),
                    difference_value: None,
                    absolute_difference: None,
                    tolerance: Some(self.policy.fill_quantity_tolerance),
                    source_event_ids: optional_event(observed.source_event_id.as_ref()),
                    ledger_entry_ids: optional_ledger(observed.ledger_entry_id.as_ref()),
                    description:
                        "duplicate read-only fill snapshot makes external execution state ambiguous"
                            .to_owned(),
                })?;
            }
        }

        for (key, expected) in &expected_by_key {
            match observed_by_key.get(key) {
                Some(observed) => {
                    self.reconcile_fill_pair(builder, expected, observed)?;
                }
                None => {
                    builder.push_difference(DifferenceInput {
                        category: DifferenceCategory::Fill,
                        classification: DifferenceClassification::MissingExternal,
                        severity: DifferenceSeverity::Critical,
                        scope: DifferenceScope::from_fill(expected),
                        expected_value: Some(expected.quantity.as_decimal()),
                        observed_value: None,
                        difference_value: Some(expected.quantity.as_decimal().checked_neg()?),
                        absolute_difference: Some(expected.quantity.as_decimal()),
                        tolerance: Some(self.policy.fill_quantity_tolerance),
                        source_event_ids: optional_event(expected.source_event_id.as_ref()),
                        ledger_entry_ids: optional_ledger(expected.ledger_entry_id.as_ref()),
                        description: "expected fill is missing from read-only venue fill snapshot"
                            .to_owned(),
                    })?;
                }
            }
        }

        for (key, observed) in observed_by_key {
            if !expected_by_key.contains_key(&key) {
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Fill,
                    classification: DifferenceClassification::MissingInternal,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_fill(observed),
                    expected_value: None,
                    observed_value: Some(observed.quantity.as_decimal()),
                    difference_value: Some(observed.quantity.as_decimal()),
                    absolute_difference: Some(observed.quantity.as_decimal()),
                    tolerance: Some(self.policy.fill_quantity_tolerance),
                    source_event_ids: optional_event(observed.source_event_id.as_ref()),
                    ledger_entry_ids: optional_ledger(observed.ledger_entry_id.as_ref()),
                    description:
                        "venue reports a fill that is missing from internal execution state"
                            .to_owned(),
                })?;
            }
        }

        builder.checked_fills = expected_fills.len().max(observed_fills.len());
        Ok(())
    }

    fn reconcile_fill_pair(
        self,
        builder: &mut ReportBuilder,
        expected: &FillSnapshot,
        observed: &FillSnapshot,
    ) -> ReconciliationResult<()> {
        let events = merge_events(
            expected.source_event_id.as_ref(),
            observed.source_event_id.as_ref(),
        );
        let ledger_ids = merge_ledger_ids(
            expected.ledger_entry_id.as_ref(),
            observed.ledger_entry_id.as_ref(),
        );

        self.push_amount_mismatch_if_needed(
            builder,
            AmountMismatchInput {
                category: DifferenceCategory::Fill,
                classification: DifferenceClassification::ValueMismatch,
                scope: DifferenceScope::from_fill(expected),
                expected: expected.quantity.as_decimal(),
                observed: observed.quantity.as_decimal(),
                tolerance: self.policy.fill_quantity_tolerance,
                source_event_ids: events.clone(),
                ledger_entry_ids: ledger_ids.clone(),
                description: "expected fill quantity and read-only venue fill quantity differ"
                    .to_owned(),
            },
        )?;
        self.push_amount_mismatch_if_needed(
            builder,
            AmountMismatchInput {
                category: DifferenceCategory::Fill,
                classification: DifferenceClassification::ValueMismatch,
                scope: DifferenceScope::from_fill(expected),
                expected: expected.price.as_decimal(),
                observed: observed.price.as_decimal(),
                tolerance: self.policy.fill_price_tolerance,
                source_event_ids: events.clone(),
                ledger_entry_ids: ledger_ids.clone(),
                description: "expected fill price and read-only venue fill price differ".to_owned(),
            },
        )?;

        if expected.fee_asset_id != observed.fee_asset_id {
            builder.push_difference(DifferenceInput {
                category: DifferenceCategory::Fill,
                classification: DifferenceClassification::ValueMismatch,
                severity: DifferenceSeverity::Critical,
                scope: DifferenceScope::from_fill(expected),
                expected_value: None,
                observed_value: None,
                difference_value: None,
                absolute_difference: None,
                tolerance: None,
                source_event_ids: events.clone(),
                ledger_entry_ids: ledger_ids.clone(),
                description: "expected fill fee asset and read-only venue fee asset differ"
                    .to_owned(),
            })?;
        }

        self.push_amount_mismatch_if_needed(
            builder,
            AmountMismatchInput {
                category: DifferenceCategory::Fill,
                classification: DifferenceClassification::ValueMismatch,
                scope: DifferenceScope::from_fill(expected),
                expected: expected.fee_amount.as_decimal(),
                observed: observed.fee_amount.as_decimal(),
                tolerance: self.policy.fee_tolerance,
                source_event_ids: events,
                ledger_entry_ids: ledger_ids,
                description: "expected fill fee and read-only venue fill fee differ".to_owned(),
            },
        )
    }

    fn reconcile_capital_reservations(
        self,
        builder: &mut ReportBuilder,
        ledger_entry_ids: &BTreeSet<LedgerEntryId>,
        expected_reservations: &[CapitalReservationSnapshot],
        observed_reservations: &[CapitalReservationSnapshot],
    ) -> ReconciliationResult<()> {
        let mut expected_by_key = BTreeMap::new();
        for expected in expected_reservations {
            if expected_by_key
                .insert(expected.key.clone(), expected)
                .is_some()
            {
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::CapitalReservation,
                    classification: DifferenceClassification::DuplicateInternal,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_reservation(expected),
                    expected_value: Some(expected.amount.as_decimal()),
                    observed_value: None,
                    difference_value: None,
                    absolute_difference: None,
                    tolerance: Some(self.policy.capital_reservation_tolerance),
                    source_event_ids: optional_event(expected.source_event_id.as_ref()),
                    ledger_entry_ids: optional_ledger(expected.ledger_entry_id.as_ref()),
                    description:
                        "duplicate expected capital reservation makes internal state ambiguous"
                            .to_owned(),
                })?;
            }

            if reservation_requires_ledger_link(expected.state) {
                match expected.ledger_entry_id.as_ref() {
                    Some(ledger_entry_id) if !ledger_entry_ids.contains(ledger_entry_id) => {
                        builder.push_difference(DifferenceInput {
                            category: DifferenceCategory::CapitalReservation,
                            classification: DifferenceClassification::LedgerLinkMissing,
                            severity: DifferenceSeverity::Critical,
                            scope: DifferenceScope::from_reservation(expected),
                            expected_value: Some(expected.amount.as_decimal()),
                            observed_value: None,
                            difference_value: None,
                            absolute_difference: None,
                            tolerance: None,
                            source_event_ids: optional_event(expected.source_event_id.as_ref()),
                            ledger_entry_ids: vec![ledger_entry_id.clone()],
                            description:
                                "capital reservation references a ledger entry absent from the ledger slice"
                                    .to_owned(),
                        })?;
                    }
                    None => builder.push_difference(DifferenceInput {
                        category: DifferenceCategory::CapitalReservation,
                        classification: DifferenceClassification::LedgerLinkMissing,
                        severity: DifferenceSeverity::Critical,
                        scope: DifferenceScope::from_reservation(expected),
                        expected_value: Some(expected.amount.as_decimal()),
                        observed_value: None,
                        difference_value: None,
                        absolute_difference: None,
                        tolerance: None,
                        source_event_ids: optional_event(expected.source_event_id.as_ref()),
                        ledger_entry_ids: Vec::new(),
                        description:
                            "capital reservation state requires an append-only ledger link"
                                .to_owned(),
                    })?,
                    Some(_) => {}
                }
            }
        }

        let mut observed_by_key = BTreeMap::new();
        for observed in observed_reservations {
            if observed_by_key
                .insert(observed.key.clone(), observed)
                .is_some()
            {
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::CapitalReservation,
                    classification: DifferenceClassification::UnknownExternalState,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_reservation(observed),
                    expected_value: None,
                    observed_value: Some(observed.amount.as_decimal()),
                    difference_value: None,
                    absolute_difference: None,
                    tolerance: Some(self.policy.capital_reservation_tolerance),
                    source_event_ids: optional_event(observed.source_event_id.as_ref()),
                    ledger_entry_ids: optional_ledger(observed.ledger_entry_id.as_ref()),
                    description:
                        "duplicate read-only capital reservation snapshot makes external state ambiguous"
                            .to_owned(),
                })?;
            }
        }

        for (key, expected) in &expected_by_key {
            match observed_by_key.get(key) {
                Some(observed) => {
                    if expected.state != observed.state {
                        builder.push_difference(DifferenceInput {
                            category: DifferenceCategory::CapitalReservation,
                            classification: DifferenceClassification::StateMismatch,
                            severity: DifferenceSeverity::Critical,
                            scope: DifferenceScope::from_reservation(expected),
                            expected_value: Some(expected.amount.as_decimal()),
                            observed_value: Some(observed.amount.as_decimal()),
                            difference_value: None,
                            absolute_difference: None,
                            tolerance: None,
                            source_event_ids: merge_events(
                                expected.source_event_id.as_ref(),
                                observed.source_event_id.as_ref(),
                            ),
                            ledger_entry_ids: merge_ledger_ids(
                                expected.ledger_entry_id.as_ref(),
                                observed.ledger_entry_id.as_ref(),
                            ),
                            description: "capital reservation state differs".to_owned(),
                        })?;
                    }

                    self.push_amount_mismatch_if_needed(
                        builder,
                        AmountMismatchInput {
                            category: DifferenceCategory::CapitalReservation,
                            classification: DifferenceClassification::ValueMismatch,
                            scope: DifferenceScope::from_reservation(expected),
                            expected: expected.amount.as_decimal(),
                            observed: observed.amount.as_decimal(),
                            tolerance: self.policy.capital_reservation_tolerance,
                            source_event_ids: merge_events(
                                expected.source_event_id.as_ref(),
                                observed.source_event_id.as_ref(),
                            ),
                            ledger_entry_ids: merge_ledger_ids(
                                expected.ledger_entry_id.as_ref(),
                                observed.ledger_entry_id.as_ref(),
                            ),
                            description:
                                "expected capital reservation amount and read-only reservation amount differ"
                                    .to_owned(),
                        },
                    )?;
                }
                None if reservation_is_active(expected.state)
                    && !decimal_within_abs(
                        expected.amount.as_decimal(),
                        self.policy.capital_reservation_tolerance,
                    )? =>
                {
                    builder.push_difference(DifferenceInput {
                        category: DifferenceCategory::CapitalReservation,
                        classification: DifferenceClassification::MissingExternal,
                        severity: DifferenceSeverity::Critical,
                        scope: DifferenceScope::from_reservation(expected),
                        expected_value: Some(expected.amount.as_decimal()),
                        observed_value: None,
                        difference_value: Some(expected.amount.as_decimal().checked_neg()?),
                        absolute_difference: Some(expected.amount.as_decimal()),
                        tolerance: Some(self.policy.capital_reservation_tolerance),
                        source_event_ids: optional_event(expected.source_event_id.as_ref()),
                        ledger_entry_ids: optional_ledger(expected.ledger_entry_id.as_ref()),
                        description:
                            "active internal capital reservation is missing from read-only reservation snapshot"
                                .to_owned(),
                    })?;
                }
                None => {}
            }
        }

        for (key, observed) in observed_by_key {
            if !expected_by_key.contains_key(&key)
                && reservation_is_active(observed.state)
                && !decimal_within_abs(
                    observed.amount.as_decimal(),
                    self.policy.capital_reservation_tolerance,
                )?
            {
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::CapitalReservation,
                    classification: DifferenceClassification::MissingInternal,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::from_reservation(observed),
                    expected_value: None,
                    observed_value: Some(observed.amount.as_decimal()),
                    difference_value: Some(observed.amount.as_decimal()),
                    absolute_difference: Some(observed.amount.as_decimal()),
                    tolerance: Some(self.policy.capital_reservation_tolerance),
                    source_event_ids: optional_event(observed.source_event_id.as_ref()),
                    ledger_entry_ids: optional_ledger(observed.ledger_entry_id.as_ref()),
                    description:
                        "read-only reservation snapshot contains an active reservation absent from internal state"
                            .to_owned(),
                })?;
            }
        }

        builder.checked_capital_reservations =
            expected_reservations.len().max(observed_reservations.len());
        Ok(())
    }

    fn push_amount_mismatch_if_needed(
        self,
        builder: &mut ReportBuilder,
        input: AmountMismatchInput,
    ) -> ReconciliationResult<()> {
        let signed_difference = input.observed.checked_sub(input.expected)?;
        let absolute = abs_decimal(signed_difference)?;
        if decimal_lte(absolute, input.tolerance)? {
            return Ok(());
        }
        let severity = self.severity_for_difference(input.classification, absolute)?;
        builder.push_difference(DifferenceInput {
            category: input.category,
            classification: input.classification,
            severity,
            scope: input.scope,
            expected_value: Some(input.expected),
            observed_value: Some(input.observed),
            difference_value: Some(signed_difference),
            absolute_difference: Some(absolute),
            tolerance: Some(input.tolerance),
            source_event_ids: input.source_event_ids,
            ledger_entry_ids: input.ledger_entry_ids,
            description: input.description,
        })
    }

    fn severity_for_difference(
        self,
        classification: DifferenceClassification,
        absolute_difference: Decimal,
    ) -> ReconciliationResult<DifferenceSeverity> {
        let severe_by_class = matches!(
            classification,
            DifferenceClassification::MissingExternal
                | DifferenceClassification::MissingInternal
                | DifferenceClassification::LedgerLinkMissing
                | DifferenceClassification::InvalidLedger
                | DifferenceClassification::DuplicateInternal
                | DifferenceClassification::UnknownExternalState
                | DifferenceClassification::StateMismatch
        );
        if severe_by_class
            || decimal_gt(
                absolute_difference,
                self.policy.critical_difference_threshold,
            )?
        {
            Ok(DifferenceSeverity::Critical)
        } else {
            Ok(DifferenceSeverity::Medium)
        }
    }
}

impl ReconciliationRunner for CoreReconciliationRunner {
    fn run(
        &self,
        request: ReconciliationRequest<'_>,
    ) -> ReconciliationResult<ReconciliationReport> {
        self.policy.validate()?;
        let mut builder = ReportBuilder::new(request.run_id, request.as_of);

        let ledger_entry_ids =
            self.reconcile_ledger_entries(&mut builder, request.ledger_entries)?;
        match BalanceView::from_entries(request.ledger_entries) {
            Ok(balance_view) => {
                self.reconcile_balances(&mut builder, &balance_view, request.observed_balances)?;
            }
            Err(error) => {
                builder.checked_ledger_entries = request.ledger_entries.len();
                builder.push_difference(DifferenceInput {
                    category: DifferenceCategory::Ledger,
                    classification: DifferenceClassification::InvalidLedger,
                    severity: DifferenceSeverity::Critical,
                    scope: DifferenceScope::default(),
                    expected_value: None,
                    observed_value: None,
                    difference_value: None,
                    absolute_difference: None,
                    tolerance: None,
                    source_event_ids: Vec::new(),
                    ledger_entry_ids: Vec::new(),
                    description: format!("ledger balance view cannot be derived: {error}"),
                })?;
            }
        }

        self.reconcile_positions(
            &mut builder,
            request.expected_positions,
            request.observed_positions,
        )?;
        self.reconcile_fills(
            &mut builder,
            &ledger_entry_ids,
            request.expected_fills,
            request.observed_fills,
        )?;
        self.reconcile_capital_reservations(
            &mut builder,
            &ledger_entry_ids,
            request.expected_reservations,
            request.observed_reservations,
        )?;

        builder.finish()
    }
}

#[derive(Clone, Debug)]
struct AmountMismatchInput {
    category: DifferenceCategory,
    classification: DifferenceClassification,
    scope: DifferenceScope,
    expected: Decimal,
    observed: Decimal,
    tolerance: Decimal,
    source_event_ids: Vec<EventId>,
    ledger_entry_ids: Vec<LedgerEntryId>,
    description: String,
}

#[derive(Clone, Debug)]
struct DifferenceInput {
    category: DifferenceCategory,
    classification: DifferenceClassification,
    severity: DifferenceSeverity,
    scope: DifferenceScope,
    expected_value: Option<Decimal>,
    observed_value: Option<Decimal>,
    difference_value: Option<Decimal>,
    absolute_difference: Option<Decimal>,
    tolerance: Option<Decimal>,
    source_event_ids: Vec<EventId>,
    ledger_entry_ids: Vec<LedgerEntryId>,
    description: String,
}

#[derive(Clone, Debug)]
struct ReportBuilder {
    run_id: ReconciliationRunId,
    as_of: UtcTimestamp,
    differences: Vec<ReconciliationDifference>,
    checked_balances: usize,
    checked_positions: usize,
    checked_fills: usize,
    checked_ledger_entries: usize,
    checked_capital_reservations: usize,
}

impl ReportBuilder {
    fn new(run_id: ReconciliationRunId, as_of: UtcTimestamp) -> Self {
        Self {
            run_id,
            as_of,
            differences: Vec::new(),
            checked_balances: 0,
            checked_positions: 0,
            checked_fills: 0,
            checked_ledger_entries: 0,
            checked_capital_reservations: 0,
        }
    }

    fn push_difference(&mut self, input: DifferenceInput) -> ReconciliationResult<()> {
        let difference_id = ReconciliationDifferenceId::new(format!(
            "difference:{}:{}",
            self.run_id.as_str(),
            self.differences.len() + 1
        ))?;
        self.differences.push(ReconciliationDifference {
            difference_id,
            category: input.category,
            classification: input.classification,
            severity: input.severity,
            reason_code: input.classification.as_reason_code().to_owned(),
            scope: input.scope,
            expected_value: input.expected_value,
            observed_value: input.observed_value,
            difference_value: input.difference_value,
            absolute_difference: input.absolute_difference,
            tolerance: input.tolerance,
            source_event_ids: input.source_event_ids,
            ledger_entry_ids: input.ledger_entry_ids,
            description: input.description,
        });
        Ok(())
    }

    fn finish(self) -> ReconciliationResult<ReconciliationReport> {
        let incident_suggestions =
            build_incident_suggestions(&self.run_id, self.as_of, &self.differences)?;
        let highest_severity = self
            .differences
            .iter()
            .map(|difference| difference.severity)
            .max();
        let status = if !incident_suggestions.is_empty() {
            ReconciliationStatus::IncidentRecommended
        } else if self.differences.is_empty() {
            ReconciliationStatus::Matched
        } else {
            ReconciliationStatus::Mismatch
        };
        let summary = ReconciliationSummary {
            status,
            checked_balances: self.checked_balances,
            checked_positions: self.checked_positions,
            checked_fills: self.checked_fills,
            checked_ledger_entries: self.checked_ledger_entries,
            checked_capital_reservations: self.checked_capital_reservations,
            difference_count: self.differences.len(),
            incident_suggestion_count: incident_suggestions.len(),
            highest_severity,
        };

        Ok(ReconciliationReport {
            run_id: self.run_id,
            as_of: self.as_of,
            summary,
            differences: self.differences,
            incident_suggestions,
        })
    }
}

fn build_incident_suggestions(
    run_id: &ReconciliationRunId,
    opened_at: UtcTimestamp,
    differences: &[ReconciliationDifference],
) -> ReconciliationResult<Vec<IncidentSuggestion>> {
    let mut suggestions = Vec::new();
    for difference in differences
        .iter()
        .filter(|difference| difference.severity.requires_incident())
    {
        let suggestion_id = IncidentSuggestionId::new(format!(
            "incident-suggestion:{}:{}",
            run_id.as_str(),
            suggestions.len() + 1
        ))?;
        suggestions.push(IncidentSuggestion {
            suggestion_id,
            severity: difference.severity.to_incident_severity(),
            status: IncidentStatus::Open,
            opened_at,
            trigger_reason_code: difference.reason_code.clone(),
            source_difference_ids: vec![difference.difference_id.clone()],
            impacted: incident_scope_from_difference(difference),
            suggested_actions: suggested_actions_for(difference),
        });
    }
    Ok(suggestions)
}

fn incident_scope_from_difference(difference: &ReconciliationDifference) -> IncidentImpactedScope {
    IncidentImpactedScope {
        venue_ids: difference.scope.venue_id.clone().into_iter().collect(),
        strategy_ids: difference.scope.strategy_id.clone().into_iter().collect(),
        account_ids: difference.scope.account_id.clone().into_iter().collect(),
        asset_ids: difference.scope.asset_id.clone().into_iter().collect(),
        ledger_entry_ids: difference
            .scope
            .ledger_entry_id
            .clone()
            .into_iter()
            .chain(difference.ledger_entry_ids.clone())
            .collect(),
        capital_at_risk: difference.absolute_difference,
    }
}

fn suggested_actions_for(difference: &ReconciliationDifference) -> Vec<IncidentSuggestedAction> {
    let mut actions = vec![
        IncidentSuggestedAction::OpenIncidentRecord,
        IncidentSuggestedAction::PauseAffectedScopeUntilReviewed,
        IncidentSuggestedAction::ManualInvestigation,
    ];
    if matches!(
        difference.classification,
        DifferenceClassification::MissingExternal
            | DifferenceClassification::MissingInternal
            | DifferenceClassification::UnknownExternalState
    ) {
        actions.push(IncidentSuggestedAction::RefreshReadOnlySnapshots);
    }
    if matches!(
        difference.category,
        DifferenceCategory::Ledger | DifferenceCategory::CapitalReservation
    ) {
        actions.push(IncidentSuggestedAction::ReviewAppendOnlyLedgerAdjustment);
    }
    actions
}

fn reservation_is_active(status: CapitalReservationStatus) -> bool {
    matches!(
        status,
        CapitalReservationStatus::Reserved
            | CapitalReservationStatus::InExecution
            | CapitalReservationStatus::ReconciliationMismatch
    )
}

fn reservation_requires_ledger_link(status: CapitalReservationStatus) -> bool {
    matches!(
        status,
        CapitalReservationStatus::Reserved
            | CapitalReservationStatus::InExecution
            | CapitalReservationStatus::Released
            | CapitalReservationStatus::Expired
            | CapitalReservationStatus::ReconciliationMismatch
    )
}

fn merge_events(left: Option<&EventId>, right: Option<&EventId>) -> Vec<EventId> {
    let mut events = Vec::new();
    if let Some(left) = left {
        events.push(left.clone());
    }
    if let Some(right) = right {
        if !events.contains(right) {
            events.push(right.clone());
        }
    }
    events
}

fn optional_event(event_id: Option<&EventId>) -> Vec<EventId> {
    event_id.cloned().into_iter().collect()
}

fn merge_ledger_ids(
    left: Option<&LedgerEntryId>,
    right: Option<&LedgerEntryId>,
) -> Vec<LedgerEntryId> {
    let mut ledger_entry_ids = Vec::new();
    if let Some(left) = left {
        ledger_entry_ids.push(left.clone());
    }
    if let Some(right) = right {
        if !ledger_entry_ids.contains(right) {
            ledger_entry_ids.push(right.clone());
        }
    }
    ledger_entry_ids
}

fn optional_ledger(ledger_entry_id: Option<&LedgerEntryId>) -> Vec<LedgerEntryId> {
    ledger_entry_id.cloned().into_iter().collect()
}

fn zero() -> Decimal {
    Decimal::from_scaled_atoms(0, 0)
}

fn abs_decimal(value: Decimal) -> ReconciliationResult<Decimal> {
    if value.is_negative() {
        Ok(value.checked_neg()?)
    } else {
        Ok(value)
    }
}

fn decimal_within_abs(value: Decimal, tolerance: Decimal) -> ReconciliationResult<bool> {
    decimal_lte(abs_decimal(value)?, tolerance)
}

fn decimal_lte(left: Decimal, right: Decimal) -> ReconciliationResult<bool> {
    Ok(compare_decimal(left, right)? != Ordering::Greater)
}

fn decimal_gt(left: Decimal, right: Decimal) -> ReconciliationResult<bool> {
    Ok(compare_decimal(left, right)? == Ordering::Greater)
}

fn compare_decimal(left: Decimal, right: Decimal) -> ReconciliationResult<Ordering> {
    left.partial_cmp(&right)
        .ok_or(ReconciliationError::DecimalComparisonOverflow { left, right })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_ledger::{
        IdempotencyKey, JournalEntryId, LedgerBook, LedgerDirection, LedgerEntryDraft,
        LedgerEntryHeader, LedgerEntryType, LedgerLeg, LedgerLegId,
    };

    #[test]
    fn matched_inputs_produce_report_for_ops_without_differences() {
        let entry = demo_entry("ledger:matched", "100.00");
        let observed_balances = matching_balances("100.00");
        let expected_position = position("1.5");
        let observed_position = position("1.5");
        let expected_fill = fill("fill:matched", "100.00", "1.5", "0.10")
            .with_ledger_entry_id(ledger_entry_id("ledger:matched"));
        let observed_fill = fill("fill:matched", "100.00", "1.5", "0.10")
            .with_ledger_entry_id(ledger_entry_id("ledger:matched"));
        let expected_reservation = reservation("reservation:matched", "10.00")
            .with_ledger_entry_id(ledger_entry_id("ledger:matched"));
        let observed_reservation = reservation("reservation:matched", "10.00")
            .with_ledger_entry_id(ledger_entry_id("ledger:matched"));

        let request = ReconciliationRequest {
            run_id: run_id("recon:matched"),
            as_of: ts(),
            ledger_entries: std::slice::from_ref(&entry),
            observed_balances: &observed_balances,
            expected_positions: std::slice::from_ref(&expected_position),
            observed_positions: std::slice::from_ref(&observed_position),
            expected_fills: std::slice::from_ref(&expected_fill),
            observed_fills: std::slice::from_ref(&observed_fill),
            expected_reservations: std::slice::from_ref(&expected_reservation),
            observed_reservations: std::slice::from_ref(&observed_reservation),
        };

        let report = CoreReconciliationRunner::default()
            .run(request)
            .expect("reconciliation succeeds");

        assert_eq!(report.status(), ReconciliationStatus::Matched);
        assert!(!report.has_differences());
        assert_eq!(report.summary.checked_balances, 2);
        assert_eq!(report.summary.checked_fills, 1);
        assert_eq!(report.summary.checked_capital_reservations, 1);
    }

    #[test]
    fn tolerance_does_not_report_small_balance_difference() {
        let entry = demo_entry("ledger:tolerance", "100.00");
        let observed_balances = vec![
            balance("acct:cash", "asset:USDC", "100.005"),
            balance("acct:clearing", "asset:USDC", "-100.005"),
        ];
        let policy = ReconciliationPolicy::default()
            .with_balance_tolerance(decimal("0.01"))
            .with_critical_difference_threshold(decimal("10.00"));
        let request = ReconciliationRequest {
            run_id: run_id("recon:tolerance"),
            as_of: ts(),
            ledger_entries: std::slice::from_ref(&entry),
            observed_balances: &observed_balances,
            expected_positions: &[],
            observed_positions: &[],
            expected_fills: &[],
            observed_fills: &[],
            expected_reservations: &[],
            observed_reservations: &[],
        };

        let report = CoreReconciliationRunner::new(policy)
            .run(request)
            .expect("reconciliation succeeds");

        assert_eq!(report.status(), ReconciliationStatus::Matched);
        assert!(report.differences.is_empty());
    }

    #[test]
    fn differences_are_found_across_all_core_categories() {
        let entry = demo_entry("ledger:mismatch", "100.00");
        let observed_balances = vec![
            balance("acct:cash", "asset:USDC", "97.00"),
            balance("acct:clearing", "asset:USDC", "-100.00"),
        ];
        let expected_position = position("2.0");
        let observed_position = position("1.0");
        let expected_fill = fill("fill:mismatch", "100.00", "2.0", "0.10")
            .with_ledger_entry_id(ledger_entry_id("ledger:mismatch"));
        let observed_fill = fill("fill:mismatch", "100.00", "1.0", "0.10")
            .with_ledger_entry_id(ledger_entry_id("ledger:mismatch"));
        let expected_reservation = reservation("reservation:mismatch", "25.00")
            .with_ledger_entry_id(ledger_entry_id("ledger:mismatch"));
        let observed_reservation = reservation("reservation:mismatch", "20.00")
            .with_ledger_entry_id(ledger_entry_id("ledger:mismatch"));
        let unlinked_fill = fill("fill:unlinked", "100.00", "1.0", "0.10");

        let request = ReconciliationRequest {
            run_id: run_id("recon:mismatch"),
            as_of: ts(),
            ledger_entries: std::slice::from_ref(&entry),
            observed_balances: &observed_balances,
            expected_positions: std::slice::from_ref(&expected_position),
            observed_positions: std::slice::from_ref(&observed_position),
            expected_fills: &[expected_fill, unlinked_fill],
            observed_fills: std::slice::from_ref(&observed_fill),
            expected_reservations: std::slice::from_ref(&expected_reservation),
            observed_reservations: std::slice::from_ref(&observed_reservation),
        };

        let report = CoreReconciliationRunner::default()
            .run(request)
            .expect("reconciliation succeeds");
        let categories = report
            .differences
            .iter()
            .map(|difference| difference.category)
            .collect::<BTreeSet<_>>();

        assert!(categories.contains(&DifferenceCategory::Balance));
        assert!(categories.contains(&DifferenceCategory::Position));
        assert!(categories.contains(&DifferenceCategory::Fill));
        assert!(categories.contains(&DifferenceCategory::Ledger));
        assert!(categories.contains(&DifferenceCategory::CapitalReservation));
    }

    #[test]
    fn severe_difference_generates_incident_suggestion() {
        let entry = demo_entry("ledger:incident", "100.00");
        let observed_balances = vec![
            balance("acct:cash", "asset:USDC", "50.00"),
            balance("acct:clearing", "asset:USDC", "-100.00"),
        ];
        let request = ReconciliationRequest {
            run_id: run_id("recon:incident"),
            as_of: ts(),
            ledger_entries: std::slice::from_ref(&entry),
            observed_balances: &observed_balances,
            expected_positions: &[],
            observed_positions: &[],
            expected_fills: &[],
            observed_fills: &[],
            expected_reservations: &[],
            observed_reservations: &[],
        };

        let report = CoreReconciliationRunner::default()
            .run(request)
            .expect("reconciliation succeeds");

        assert_eq!(report.status(), ReconciliationStatus::IncidentRecommended);
        assert!(report.has_incident_suggestions());
        assert_eq!(report.incident_suggestions[0].status, IncidentStatus::Open);
        assert!(report.incident_suggestions[0]
            .suggested_actions
            .contains(&IncidentSuggestedAction::PauseAffectedScopeUntilReviewed));
    }

    #[test]
    fn reconciliation_does_not_mutate_ledger_history() {
        let mut book = LedgerBook::default();
        let entry = demo_entry("ledger:immutable", "100.00");
        book.append(entry).expect("append succeeds");
        let before_entries = book.entries().to_vec();
        let before_count = book.entry_count();
        let observed_balances = matching_balances("100.00");
        let request = ReconciliationRequest {
            run_id: run_id("recon:immutable"),
            as_of: ts(),
            ledger_entries: book.entries(),
            observed_balances: &observed_balances,
            expected_positions: &[],
            observed_positions: &[],
            expected_fills: &[],
            observed_fills: &[],
            expected_reservations: &[],
            observed_reservations: &[],
        };

        let report = CoreReconciliationRunner::default()
            .run(request)
            .expect("reconciliation succeeds");

        assert_eq!(report.status(), ReconciliationStatus::Matched);
        assert_eq!(book.entry_count(), before_count);
        assert_eq!(book.entries(), before_entries.as_slice());
    }

    fn demo_entry(entry_id: &str, amount: &str) -> LedgerEntry {
        LedgerEntry::from_draft(LedgerEntryDraft::new(
            LedgerEntryHeader::new(
                ledger_entry_id(entry_id),
                JournalEntryId::new(format!("journal:{entry_id}")).expect("journal id"),
                ts(),
                LedgerNamespace::Simulation,
                LedgerEntryType::TradeFill,
                EventId::new(format!("event:{entry_id}")).expect("event id"),
                IdempotencyKey::new(format!("idem:{entry_id}")).expect("idempotency id"),
            ),
            vec![
                LedgerLeg::new(
                    LedgerLegId::new(format!("leg:{entry_id}:debit")).expect("leg id"),
                    account_id("acct:cash"),
                    asset_id("asset:USDC"),
                    LedgerDirection::Debit,
                    amount_value(amount),
                ),
                LedgerLeg::new(
                    LedgerLegId::new(format!("leg:{entry_id}:credit")).expect("leg id"),
                    account_id("acct:clearing"),
                    asset_id("asset:USDC"),
                    LedgerDirection::Credit,
                    amount_value(amount),
                ),
            ],
        ))
        .expect("balanced ledger entry")
    }

    fn matching_balances(amount: &str) -> Vec<ReadOnlyBalanceSnapshot> {
        vec![
            balance("acct:cash", "asset:USDC", amount),
            balance("acct:clearing", "asset:USDC", &format!("-{amount}")),
        ]
    }

    fn balance(account: &str, asset: &str, amount: &str) -> ReadOnlyBalanceSnapshot {
        ReadOnlyBalanceSnapshot::new(
            LedgerNamespace::Simulation,
            account_id(account),
            asset_id(asset),
            decimal(amount),
        )
    }

    fn position(quantity: &str) -> PositionSnapshot {
        PositionSnapshot::new(
            account_id("acct:perp"),
            InstrumentId::new("instrument:BTC-PERP").expect("instrument id"),
            decimal(quantity),
        )
    }

    fn fill(fill_id_value: &str, price: &str, quantity: &str, fee: &str) -> FillSnapshot {
        FillSnapshot::new(
            FillId::new(fill_id_value).expect("fill id"),
            ExecutionPlanId::new("plan:demo").expect("plan id"),
            VenueId::new("venue:demo").expect("venue id"),
            InstrumentId::new("instrument:BTC-USDC").expect("instrument id"),
            Price::from_str(price).expect("price"),
            Quantity::from_str(quantity).expect("quantity"),
            FeeAmount::new(asset_id("asset:USDC"), amount_value(fee)),
        )
    }

    fn reservation(reservation_id: &str, amount: &str) -> CapitalReservationSnapshot {
        CapitalReservationSnapshot::new(
            CapitalReservationId::new(reservation_id).expect("reservation id"),
            CapitalReservationStatus::Reserved,
            asset_id("asset:USDC"),
            amount_value(amount),
            CandidateTransitionId::new("candidate:demo").expect("candidate id"),
        )
    }

    fn amount_value(value: &str) -> Amount {
        Amount::from_str(value).expect("amount")
    }

    fn decimal(value: &str) -> Decimal {
        Decimal::from_str(value).expect("decimal")
    }

    fn ledger_entry_id(value: &str) -> LedgerEntryId {
        LedgerEntryId::new(value).expect("ledger entry id")
    }

    fn account_id(value: &str) -> AccountId {
        AccountId::new(value).expect("account id")
    }

    fn asset_id(value: &str) -> AssetId {
        AssetId::new(value).expect("asset id")
    }

    fn run_id(value: &str) -> ReconciliationRunId {
        ReconciliationRunId::new(value).expect("run id")
    }

    fn ts() -> UtcTimestamp {
        UtcTimestamp::parse_rfc3339_z("2026-05-10T00:00:00Z").expect("timestamp")
    }
}
