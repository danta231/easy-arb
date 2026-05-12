//! `arb-risk` 风控评估入口。
//!
//! 中文说明：本 crate 只读取候选组合转换、组合状态、只读配置和场所能力，
//! 输出 `RiskDecision`。这里不调度执行、不签名、不写账本，也不依赖运行时装配。
//!
//! ```compile_fail
//! use arb_risk::RiskEvaluator;
//!
//! fn cannot_execute(evaluator: &dyn RiskEvaluator) {
//!     let _ = evaluator.execution_adapter();
//! }
//! ```
//!
//! ```compile_fail
//! use arb_risk::RiskEvaluator;
//!
//! fn cannot_sign(evaluator: &dyn RiskEvaluator) {
//!     let _ = evaluator.signing_provider();
//! }
//! ```

#![forbid(unsafe_code)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::str::FromStr;

use arb_config::{ArbConfig, ConfigError, ExecutionMode as ConfigExecutionMode};
use arb_contracts::{
    from_json_strict, to_canonical_json, AssetFlowDirection, CandidatePortfolioTransition,
    CapitalReservationState, ContractError, ExecutionCapability, FailureMode, OpenOrderStatus,
    PendingTransferStatus, PortfolioState, RiskCheckStatus, RiskCheckType, RiskConstraintType,
    RiskDecision, RiskDecisionKind, RiskFlag, RiskSeverity, TransitionLegType,
    VenueCapabilityDescriptor,
};
use arb_domain::{Decimal, DomainError, UtcTimestamp};

/// 当前 S5-02 默认风控策略版本。
pub const DEFAULT_RISK_POLICY_VERSION: &str = "risk-policy:s5-02";
/// 当前 S5-02 默认风控策略哈希引用。
pub const DEFAULT_RISK_POLICY_HASH: &str = "hash:risk-policy:s5-02";
/// 风控策略签名引用占位。中文说明：这是策略引用，不包含任何真实签名或密钥。
pub const DEFAULT_RISK_POLICY_SIGNATURE_REF: &str = "sigref:risk-policy-unsigned";
/// 默认组合状态新鲜度阈值。
pub const DEFAULT_MAX_PORTFOLIO_STATE_AGE_MS: u64 = 5_000;

/// 风控层统一返回类型。
pub type RiskResult<T> = Result<T, RiskError>;

/// 风控入口错误。
///
/// 中文说明：错误只表达输入、领域类型或合同转换失败；正常风控拒绝必须
/// 返回 `RiskDecision::Rejected`，不能用异常绕开可审计决策。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RiskError {
    InvalidInput {
        field: &'static str,
        message: String,
    },
    Domain(DomainError),
    Contract(ContractError),
    Config(ConfigError),
}

impl fmt::Display for RiskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { field, message } => {
                write!(f, "{field}: invalid risk input: {message}")
            }
            Self::Domain(error) => write!(f, "{error}"),
            Self::Contract(error) => write!(f, "{error}"),
            Self::Config(error) => write!(f, "{error}"),
        }
    }
}

impl Error for RiskError {}

impl From<DomainError> for RiskError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<ContractError> for RiskError {
    fn from(error: ContractError) -> Self {
        Self::Contract(error)
    }
}

impl From<ConfigError> for RiskError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

/// 风控评估入口 trait。
///
/// 中文说明：调用方只传入不可变快照，返回值只有 `RiskDecision`。
pub trait RiskEvaluator {
    fn evaluate(&self, input: RiskEvaluationInput<'_>) -> RiskResult<RiskDecision>;
}

/// 风控输入快照。
///
/// 中文说明：该类型只借用候选转换、组合状态、只读配置和能力描述，不提供
/// 任何账户可变动作接口。
#[derive(Clone, Copy)]
pub struct RiskEvaluationInput<'a> {
    candidate: &'a CandidatePortfolioTransition,
    portfolio_state: &'a PortfolioState,
    config: &'a ArbConfig,
    venue_capabilities: &'a [VenueCapabilityDescriptor],
    evaluated_at: UtcTimestamp,
}

impl<'a> RiskEvaluationInput<'a> {
    pub fn new(
        candidate: &'a CandidatePortfolioTransition,
        portfolio_state: &'a PortfolioState,
        config: &'a ArbConfig,
        venue_capabilities: &'a [VenueCapabilityDescriptor],
        evaluated_at: UtcTimestamp,
    ) -> Self {
        Self {
            candidate,
            portfolio_state,
            config,
            venue_capabilities,
            evaluated_at,
        }
    }

    pub fn candidate(&self) -> &CandidatePortfolioTransition {
        self.candidate
    }

    pub fn portfolio_state(&self) -> &PortfolioState {
        self.portfolio_state
    }

    pub fn config(&self) -> &ArbConfig {
        self.config
    }

    pub fn venue_capabilities(&self) -> &[VenueCapabilityDescriptor] {
        self.venue_capabilities
    }

    pub fn evaluated_at(&self) -> UtcTimestamp {
        self.evaluated_at
    }
}

/// 已加载的风控策略快照。
///
/// 中文说明：S5-02 使用静态策略快照表达核心风控阈值；后续任务包可以在
/// 不改变输出合同的前提下扩展阈值来源。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RiskPolicySnapshot {
    policy_version: String,
    policy_hash: String,
    policy_signature_ref: String,
    max_portfolio_state_age_ms: u64,
    max_fee_usd: Decimal,
    max_slippage_usd: Decimal,
    max_gas_usd: Decimal,
    max_total_fee_slippage_usd: Decimal,
    min_liquidity_confidence: Decimal,
    min_margin_buffer_usd: Decimal,
    max_daily_loss_usd: Decimal,
}

impl RiskPolicySnapshot {
    pub fn new(
        policy_version: impl Into<String>,
        policy_hash: impl Into<String>,
        policy_signature_ref: impl Into<String>,
        max_portfolio_state_age_ms: u64,
    ) -> RiskResult<Self> {
        if max_portfolio_state_age_ms == 0 {
            return Err(RiskError::InvalidInput {
                field: "max_portfolio_state_age_ms",
                message: "freshness threshold must be positive".to_owned(),
            });
        }
        Ok(Self {
            policy_version: policy_version.into(),
            policy_hash: policy_hash.into(),
            policy_signature_ref: policy_signature_ref.into(),
            max_portfolio_state_age_ms,
            max_fee_usd: Decimal::from_scaled_atoms(2, 0),
            max_slippage_usd: Decimal::from_scaled_atoms(1, 0),
            max_gas_usd: Decimal::from_scaled_atoms(5, 0),
            max_total_fee_slippage_usd: Decimal::from_scaled_atoms(5, 0),
            min_liquidity_confidence: Decimal::from_scaled_atoms(80, 2),
            min_margin_buffer_usd: Decimal::from_scaled_atoms(0, 0),
            max_daily_loss_usd: Decimal::from_scaled_atoms(100, 0),
        })
    }

    pub fn s5_02_default() -> Self {
        Self {
            policy_version: DEFAULT_RISK_POLICY_VERSION.to_owned(),
            policy_hash: DEFAULT_RISK_POLICY_HASH.to_owned(),
            policy_signature_ref: DEFAULT_RISK_POLICY_SIGNATURE_REF.to_owned(),
            max_portfolio_state_age_ms: DEFAULT_MAX_PORTFOLIO_STATE_AGE_MS,
            max_fee_usd: Decimal::from_scaled_atoms(2, 0),
            max_slippage_usd: Decimal::from_scaled_atoms(1, 0),
            max_gas_usd: Decimal::from_scaled_atoms(5, 0),
            max_total_fee_slippage_usd: Decimal::from_scaled_atoms(5, 0),
            min_liquidity_confidence: Decimal::from_scaled_atoms(80, 2),
            min_margin_buffer_usd: Decimal::from_scaled_atoms(0, 0),
            max_daily_loss_usd: Decimal::from_scaled_atoms(100, 0),
        }
    }

    pub fn s5_01_default() -> Self {
        Self::s5_02_default()
    }

    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }

    pub fn policy_hash(&self) -> &str {
        &self.policy_hash
    }

    pub fn policy_signature_ref(&self) -> &str {
        &self.policy_signature_ref
    }

    pub fn max_portfolio_state_age_ms(&self) -> u64 {
        self.max_portfolio_state_age_ms
    }

    fn max_fee_usd(&self) -> Decimal {
        self.max_fee_usd
    }

    fn max_slippage_usd(&self) -> Decimal {
        self.max_slippage_usd
    }

    fn max_gas_usd(&self) -> Decimal {
        self.max_gas_usd
    }

    fn max_total_fee_slippage_usd(&self) -> Decimal {
        self.max_total_fee_slippage_usd
    }

    fn min_liquidity_confidence(&self) -> Decimal {
        self.min_liquidity_confidence
    }

    fn min_margin_buffer_usd(&self) -> Decimal {
        self.min_margin_buffer_usd
    }

    fn max_daily_loss_usd(&self) -> Decimal {
        self.max_daily_loss_usd
    }
}

impl Default for RiskPolicySnapshot {
    fn default() -> Self {
        Self::s5_02_default()
    }
}

/// 默认静态风控评估器。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticRiskEvaluator {
    policy: RiskPolicySnapshot,
}

impl StaticRiskEvaluator {
    pub fn new(policy: RiskPolicySnapshot) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &RiskPolicySnapshot {
        &self.policy
    }
}

impl Default for StaticRiskEvaluator {
    fn default() -> Self {
        Self::new(RiskPolicySnapshot::default())
    }
}

impl RiskEvaluator for StaticRiskEvaluator {
    fn evaluate(&self, input: RiskEvaluationInput<'_>) -> RiskResult<RiskDecision> {
        evaluate_with_policy(input, &self.policy)
    }
}

/// 使用默认 S5-01 策略评估候选转换。
pub fn evaluate_risk(input: RiskEvaluationInput<'_>) -> RiskResult<RiskDecision> {
    StaticRiskEvaluator::default().evaluate(input)
}

/// 严格解析候选转换 JSON，便于回放和测试入口复用同一合同。
pub fn candidate_from_json_strict(input: &str) -> RiskResult<CandidatePortfolioTransition> {
    Ok(from_json_strict(input)?)
}

/// 严格解析组合状态 JSON。
pub fn portfolio_state_from_json_strict(input: &str) -> RiskResult<PortfolioState> {
    Ok(from_json_strict(input)?)
}

/// 严格解析场所能力 JSON。
pub fn venue_capability_from_json_strict(input: &str) -> RiskResult<VenueCapabilityDescriptor> {
    Ok(from_json_strict(input)?)
}

/// 输出规范 JSON，供回放 golden fixture 后续使用。
pub fn risk_decision_to_canonical_json(decision: &RiskDecision) -> String {
    to_canonical_json(decision)
}

fn evaluate_with_policy(
    input: RiskEvaluationInput<'_>,
    policy: &RiskPolicySnapshot,
) -> RiskResult<RiskDecision> {
    let mut draft = DecisionDraft::default();
    let used_venues = used_venue_ids(input.candidate);
    let used_accounts = used_account_ids(input.candidate);
    let used_instruments = used_instrument_ids(input.candidate);
    let used_assets = used_asset_ids(input.candidate);

    check_strategy_and_input_refs(input, &mut draft);
    check_state_reference(input, &mut draft);
    check_config(
        input,
        &used_venues,
        &used_accounts,
        &used_instruments,
        &used_assets,
        &mut draft,
    );
    check_data_freshness(input, policy, &used_venues, &mut draft)?;
    check_venue_health(input, &used_venues, &mut draft);
    check_venue_capabilities(input, &used_venues, &mut draft);
    check_liquidity(input.candidate, policy, &mut draft)?;
    check_fee_and_slippage(input.candidate, policy, &mut draft)?;
    check_margin(input, policy, &mut draft)?;
    check_capital_reservation(input, &mut draft)?;
    check_daily_loss(input, policy, &mut draft)?;
    check_balances(input, &mut draft)?;
    check_unknown_state(input, &mut draft);
    check_candidate_risk_markers(input.candidate, &mut draft);

    let decision = draft.decision_kind();
    let reason_codes = draft.reason_codes_for(decision.clone());
    let constraints = draft.constraints_for(decision.clone());
    let detail = decision_detail(decision.clone());
    let json = render_decision_json(
        input,
        policy,
        decision,
        &draft.checks,
        &constraints,
        &reason_codes,
        detail,
    );
    Ok(from_json_strict(&json)?)
}

fn check_strategy_and_input_refs(input: RiskEvaluationInput<'_>, draft: &mut DecisionDraft) {
    draft.pass(
        "strategy-version",
        RiskCheckType::StrategyExposureLimit,
        "CHECK_PASSED",
        Some(MeasuredDraft::string(
            input.candidate.strategy_id.as_str(),
            "strategy_id",
        )),
        Some(MeasuredDraft::string(
            input.candidate.strategy_version.as_str(),
            "strategy_version",
        )),
        "候选转换携带策略版本，风控决策可追溯到策略输出版本。",
    );

    let input_refs = input
        .candidate
        .input_event_refs
        .iter()
        .map(|value| value.as_str())
        .collect::<Vec<_>>()
        .join(",");
    draft.pass(
        "input-event-refs",
        RiskCheckType::ReconciliationCompleteness,
        "CHECK_PASSED",
        Some(MeasuredDraft::string(
            input.portfolio_state.portfolio_state_id.as_str(),
            "portfolio_state_ref",
        )),
        Some(MeasuredDraft::string(input_refs, "input_event_refs")),
        "候选转换携带输入事件引用，风控决策可回放。",
    );
}

fn check_state_reference(input: RiskEvaluationInput<'_>, draft: &mut DecisionDraft) {
    let expected = input.portfolio_state.portfolio_state_id.as_str();
    let observed = input.candidate.current_portfolio_state_ref.as_str();
    if expected == observed {
        draft.pass(
            "state-ref",
            RiskCheckType::ReconciliationCompleteness,
            "CHECK_PASSED",
            None,
            Some(MeasuredDraft::string(expected, "portfolio_state_id")),
            "候选转换引用的组合状态与输入快照一致。",
        );
    } else {
        draft.fail(
            "state-ref",
            RiskCheckType::ReconciliationCompleteness,
            "PORTFOLIO_STATE_MISMATCH",
            Some(MeasuredDraft::string(expected, "expected_state_ref")),
            Some(MeasuredDraft::string(observed, "candidate_state_ref")),
            "候选转换引用的组合状态与输入快照不一致，禁止批准。",
        );
    }
}

fn check_config(
    input: RiskEvaluationInput<'_>,
    used_venues: &BTreeSet<String>,
    used_accounts: &BTreeSet<String>,
    used_instruments: &BTreeSet<String>,
    used_assets: &BTreeSet<String>,
    draft: &mut DecisionDraft,
) {
    let expected_version = input.config.version().as_str();
    let observed_version = input.candidate.config_version.as_str();
    if expected_version != observed_version {
        draft.fail(
            "config-version",
            RiskCheckType::ReconciliationCompleteness,
            "CONFIG_VERSION_MISMATCH",
            Some(MeasuredDraft::string(
                expected_version,
                "loaded_config_version",
            )),
            Some(MeasuredDraft::string(
                observed_version,
                "candidate_config_version",
            )),
            "候选转换配置版本与当前只读配置不一致。",
        );
        return;
    }

    let kill_switch = input.config.kill_switch();
    if kill_switch.blocks_execution_mode(input.config.execution().mode()) {
        draft.fail(
            "config-kill-switch",
            RiskCheckType::StrategyExposureLimit,
            "EXECUTION_MODE_FORBIDS_ACTION",
            Some(MeasuredDraft::string(
                input.config.execution().mode().as_str(),
                "execution_mode",
            )),
            Some(MeasuredDraft::string("blocked", "kill_switch")),
            "熔断开关阻断当前执行模式，风控拒绝。",
        );
        return;
    }
    if kill_switch.blocks_strategy(input.candidate.strategy_id.as_str()) {
        draft.fail(
            "config-strategy",
            RiskCheckType::StrategyExposureLimit,
            "STRATEGY_DISABLED",
            None,
            Some(MeasuredDraft::string(
                input.candidate.strategy_id.as_str(),
                "strategy_id",
            )),
            "策略被只读配置熔断，风控拒绝。",
        );
        return;
    }
    if let Some(disabled_venue) = used_venues
        .iter()
        .find(|venue_id| kill_switch.blocks_venue(venue_id))
    {
        draft.fail(
            "config-venue",
            RiskCheckType::VenueHealth,
            "VENUE_DISABLED",
            None,
            Some(MeasuredDraft::string(disabled_venue, "venue_id")),
            "场所被只读配置熔断，风控拒绝。",
        );
        return;
    }
    if let Some(disabled_account) = used_accounts
        .iter()
        .find(|account_id| kill_switch.blocks_account(account_id))
    {
        draft.fail(
            "config-account",
            RiskCheckType::StrategyExposureLimit,
            "ACCOUNT_DISABLED",
            None,
            Some(MeasuredDraft::string(disabled_account, "account_id")),
            "账户被只读配置熔断，风控拒绝。",
        );
        return;
    }
    if let Some(disabled_instrument) = used_instruments
        .iter()
        .find(|instrument_id| kill_switch.blocks_instrument(instrument_id))
    {
        draft.fail(
            "config-instrument",
            RiskCheckType::StrategyExposureLimit,
            "INSTRUMENT_DISABLED",
            None,
            Some(MeasuredDraft::string(disabled_instrument, "instrument_id")),
            "交易工具被只读配置熔断，风控拒绝。",
        );
        return;
    }
    if let Some(disabled_asset) = used_assets
        .iter()
        .find(|asset_id| kill_switch.blocks_asset(asset_id))
    {
        draft.fail(
            "config-asset",
            RiskCheckType::StrategyExposureLimit,
            "ASSET_DISABLED",
            None,
            Some(MeasuredDraft::string(disabled_asset, "asset_id")),
            "资产被只读配置熔断，风控拒绝。",
        );
        return;
    }

    if input.config.execution().mode() == ConfigExecutionMode::ManualApproval {
        draft.manual(
            "config-manual-approval",
            RiskCheckType::StrategyExposureLimit,
            "REQUIRES_MANUAL_APPROVAL",
            Some(MeasuredDraft::string("ManualApproval", "execution_mode")),
            "只读配置要求人工审批，风控输出人工审批决策。",
        );
    } else {
        draft.pass(
            "config",
            RiskCheckType::StrategyExposureLimit,
            "CHECK_PASSED",
            None,
            Some(MeasuredDraft::string(expected_version, "config_version")),
            "只读配置版本和熔断状态允许继续风控评估。",
        );
    }
}

fn check_data_freshness(
    input: RiskEvaluationInput<'_>,
    policy: &RiskPolicySnapshot,
    used_venues: &BTreeSet<String>,
    draft: &mut DecisionDraft,
) -> RiskResult<()> {
    let threshold = freshness_threshold_ms(input, policy, used_venues);
    let age_ms = elapsed_ms(input.portfolio_state.as_of.as_str(), input.evaluated_at)?;
    if age_ms <= threshold {
        draft.pass(
            "data-freshness",
            RiskCheckType::DataFreshness,
            "CHECK_PASSED",
            Some(MeasuredDraft::decimal(threshold.to_string(), "ms")),
            Some(MeasuredDraft::decimal(age_ms.to_string(), "ms")),
            "组合状态新鲜度在阈值内。",
        );
    } else {
        draft.fail(
            "data-freshness",
            RiskCheckType::DataFreshness,
            "DATA_STALE",
            Some(MeasuredDraft::decimal(threshold.to_string(), "ms")),
            Some(MeasuredDraft::decimal(age_ms.to_string(), "ms")),
            "组合状态超过新鲜度阈值，风控拒绝。",
        );
    }
    Ok(())
}

fn check_venue_capabilities(
    input: RiskEvaluationInput<'_>,
    used_venues: &BTreeSet<String>,
    draft: &mut DecisionDraft,
) {
    if used_venues.is_empty() {
        draft.not_applicable(
            "venue-capability",
            RiskCheckType::VenueHealth,
            "NOT_APPLICABLE",
            "候选转换没有声明场所腿，能力检查不适用。",
        );
        return;
    }

    let mut manual_venues = Vec::new();
    for venue_id in used_venues {
        let Some(capability) = find_venue_capability(input.venue_capabilities, venue_id) else {
            draft.more_data(
                "venue-capability",
                RiskCheckType::VenueHealth,
                "REQUIRES_MORE_DATA",
                Some(MeasuredDraft::string(
                    "venue capability snapshot",
                    "required_input",
                )),
                Some(MeasuredDraft::string(venue_id, "venue_id")),
                "候选转换使用了缺失能力描述的场所，风控需要更多只读数据，不能批准。",
            );
            return;
        };

        if capability.rate_limit_model.limit == 0 {
            draft.fail(
                "rate-limit",
                RiskCheckType::RateLimitState,
                "RATE_LIMITED",
                Some(MeasuredDraft::decimal("1", "minimum_limit")),
                Some(MeasuredDraft::decimal("0", "rate_limit")),
                "场所能力描述显示限频额度为零，风控拒绝。",
            );
            return;
        }
        if capability.data_surfaces.is_empty() {
            draft.fail(
                "venue-data-surface",
                RiskCheckType::DataFreshness,
                "UNKNOWN_STATE",
                None,
                Some(MeasuredDraft::string(venue_id, "venue_id")),
                "场所没有声明任何只读数据面，不能批准。",
            );
            return;
        }

        let manual_only =
            has_execution_capability(capability, &ExecutionCapability::SupportsManualApprovalOnly);
        let can_trade = capability
            .permission_model
            .as_ref()
            .and_then(|permission| permission.can_trade);
        match can_trade {
            Some(true) => {}
            Some(false) if manual_only => manual_venues.push(venue_id.clone()),
            Some(false) => {
                draft.fail(
                    "venue-permission",
                    RiskCheckType::VenueHealth,
                    "VENUE_CAPABILITY_MISSING",
                    None,
                    Some(MeasuredDraft::string(venue_id, "venue_id")),
                    "场所不允许交易且未声明仅人工审批能力，风控拒绝。",
                );
                return;
            }
            None if manual_only => manual_venues.push(venue_id.clone()),
            None => {
                draft.fail(
                    "venue-permission",
                    RiskCheckType::VenueHealth,
                    "UNKNOWN_STATE",
                    None,
                    Some(MeasuredDraft::string(venue_id, "venue_id")),
                    "场所交易权限未知且不能降级到人工审批，风控拒绝。",
                );
                return;
            }
        }
    }

    if manual_venues.is_empty() {
        let observed = used_venues.iter().cloned().collect::<Vec<_>>().join(",");
        draft.pass(
            "venue-capability",
            RiskCheckType::VenueHealth,
            "CHECK_PASSED",
            None,
            Some(MeasuredDraft::string(observed, "venue_ids")),
            "候选转换使用的场所能力和权限满足自动风控入口要求。",
        );
    } else {
        draft.manual(
            "venue-manual-approval",
            RiskCheckType::VenueHealth,
            "REQUIRES_MANUAL_APPROVAL",
            Some(MeasuredDraft::string(
                manual_venues.join(","),
                "manual_approval_venues",
            )),
            "候选转换使用仅允许人工审批的场所能力，风控要求人工审批。",
        );
    }
}

fn check_venue_health(
    input: RiskEvaluationInput<'_>,
    used_venues: &BTreeSet<String>,
    draft: &mut DecisionDraft,
) {
    if used_venues.is_empty() {
        draft.not_applicable(
            "venue-health",
            RiskCheckType::VenueHealth,
            "NOT_APPLICABLE",
            "候选转换没有声明场所腿，场所健康检查不适用。",
        );
        return;
    }

    for venue_id in used_venues {
        let Some(capability) = find_venue_capability(input.venue_capabilities, venue_id) else {
            draft.more_data(
                "venue-health",
                RiskCheckType::VenueHealth,
                "REQUIRES_MORE_DATA",
                Some(MeasuredDraft::string(
                    "venue health and capability snapshot",
                    "required_input",
                )),
                Some(MeasuredDraft::string(venue_id, "venue_id")),
                "缺失场所健康模型，风控需要更多只读数据，不能批准。",
            );
            return;
        };

        if capability.health_model.freshness_threshold_ms.as_u64() == 0 {
            draft.fail(
                "venue-health",
                RiskCheckType::VenueHealth,
                "UNKNOWN_STATE",
                Some(MeasuredDraft::decimal("1", "minimum_freshness_ms")),
                Some(MeasuredDraft::decimal("0", "freshness_threshold_ms")),
                "场所健康模型的新鲜度阈值为零，按未知状态拒绝。",
            );
            return;
        }

        if capability.health_model.disconnect_threshold == 0 {
            draft.fail(
                "venue-health",
                RiskCheckType::VenueHealth,
                "VENUE_UNHEALTHY",
                Some(MeasuredDraft::decimal("1", "minimum_disconnect_threshold")),
                Some(MeasuredDraft::decimal("0", "disconnect_threshold")),
                "场所健康模型显示断连阈值为零，风控拒绝。",
            );
            return;
        }
    }

    draft.pass(
        "venue-health",
        RiskCheckType::VenueHealth,
        "CHECK_PASSED",
        None,
        Some(MeasuredDraft::string(
            used_venues.iter().cloned().collect::<Vec<_>>().join(","),
            "venue_ids",
        )),
        "候选转换使用的场所健康模型满足默认风控阈值。",
    );
}

fn check_liquidity(
    candidate: &CandidatePortfolioTransition,
    policy: &RiskPolicySnapshot,
    draft: &mut DecisionDraft,
) -> RiskResult<()> {
    if candidate
        .risk_flags
        .contains(&RiskFlag::InsufficientLiquidity)
    {
        draft.fail(
            "liquidity",
            RiskCheckType::LiquiditySufficiency,
            "INSUFFICIENT_LIQUIDITY",
            None,
            Some(MeasuredDraft::string(
                RiskFlag::InsufficientLiquidity.as_str(),
                "candidate_risk_flag",
            )),
            "候选转换声明流动性不足，风控拒绝。",
        );
        return Ok(());
    }

    if let Some(impact) = candidate.liquidity_impact.as_ref() {
        if let Some(confidence) = impact.confidence.as_ref() {
            let observed = Decimal::from_str(confidence.as_json_number())?;
            if decimal_cmp(&observed, &policy.min_liquidity_confidence())? == Ordering::Less {
                draft.fail(
                    "liquidity",
                    RiskCheckType::LiquiditySufficiency,
                    "INSUFFICIENT_LIQUIDITY",
                    Some(MeasuredDraft::decimal(
                        policy.min_liquidity_confidence().to_string(),
                        "minimum_confidence",
                    )),
                    Some(MeasuredDraft::decimal(
                        observed.to_string(),
                        "liquidity_confidence",
                    )),
                    "候选转换流动性影响置信度低于阈值，风控拒绝。",
                );
                return Ok(());
            }
        }
    }

    draft.pass(
        "liquidity",
        RiskCheckType::LiquiditySufficiency,
        "CHECK_PASSED",
        Some(MeasuredDraft::decimal(
            policy.min_liquidity_confidence().to_string(),
            "minimum_confidence",
        )),
        Some(MeasuredDraft::string(
            "no insufficient-liquidity flag",
            "liquidity_status",
        )),
        "候选转换未声明流动性不足，流动性检查通过。",
    );
    Ok(())
}

fn check_fee_and_slippage(
    candidate: &CandidatePortfolioTransition,
    policy: &RiskPolicySnapshot,
    draft: &mut DecisionDraft,
) -> RiskResult<()> {
    let fee = Decimal::from_str(candidate.expected_economics.fee_estimate_usd.as_str())?;
    let slippage = Decimal::from_str(candidate.expected_economics.slippage_estimate_usd.as_str())?;
    let gas = match candidate.expected_economics.gas_estimate_usd.as_ref() {
        Some(value) => Decimal::from_str(value.as_str())?,
        None => decimal_zero(),
    };

    if decimal_cmp(&fee, &policy.max_fee_usd())? == Ordering::Greater {
        draft.fail(
            "fee",
            RiskCheckType::FeeAndGasInclusion,
            "HIGH_FEE",
            Some(MeasuredDraft::decimal(
                policy.max_fee_usd().to_string(),
                "usd",
            )),
            Some(MeasuredDraft::decimal(fee.to_string(), "usd")),
            "手续费估算超过风控阈值。",
        );
        return Ok(());
    }

    if decimal_cmp(&slippage, &policy.max_slippage_usd())? == Ordering::Greater {
        draft.fail(
            "slippage",
            RiskCheckType::SlippageBounds,
            "HIGH_SLIPPAGE",
            Some(MeasuredDraft::decimal(
                policy.max_slippage_usd().to_string(),
                "usd",
            )),
            Some(MeasuredDraft::decimal(slippage.to_string(), "usd")),
            "滑点估算超过风控阈值。",
        );
        return Ok(());
    }

    let constrained_slippage_threshold = Decimal::from_scaled_atoms(80, 2);
    if decimal_cmp(&slippage, &constrained_slippage_threshold)? != Ordering::Less {
        let limit = Some(MeasuredDraft::decimal(
            policy.max_slippage_usd().to_string(),
            "usd",
        ));
        draft.approve_with_constraint(
            CheckDraft {
                suffix: "slippage-constraint",
                check_type: RiskCheckType::SlippageBounds,
                status: RiskCheckStatus::Warning,
                severity: RiskSeverity::Warn,
                threshold: limit.clone(),
                observed: Some(MeasuredDraft::decimal(slippage.to_string(), "usd")),
                reason_code: "APPROVED_WITH_CONSTRAINTS".to_owned(),
                detail: "滑点估算接近上限但未超阈值，风控带最大滑点约束批准。".to_owned(),
            },
            ConstraintDraft {
                suffix: "slippage-constraint",
                constraint_type: RiskConstraintType::MaxSlippage,
                field_path: "$.expected_economics.slippage_estimate_usd".to_owned(),
                limit,
                expires_at: None,
            },
        );
    }

    if decimal_cmp(&gas, &policy.max_gas_usd())? == Ordering::Greater {
        draft.fail(
            "gas",
            RiskCheckType::FeeAndGasInclusion,
            "HIGH_GAS",
            Some(MeasuredDraft::decimal(
                policy.max_gas_usd().to_string(),
                "usd",
            )),
            Some(MeasuredDraft::decimal(gas.to_string(), "usd")),
            "gas 估算超过风控阈值。",
        );
        return Ok(());
    }

    let total = fee.checked_add(slippage)?.checked_add(gas)?;
    if decimal_cmp(&total, &policy.max_total_fee_slippage_usd())? == Ordering::Greater {
        draft.fail(
            "fee-slippage-total",
            RiskCheckType::FeeAndGasInclusion,
            "HIGH_FEE_AND_SLIPPAGE",
            Some(MeasuredDraft::decimal(
                policy.max_total_fee_slippage_usd().to_string(),
                "usd",
            )),
            Some(MeasuredDraft::decimal(total.to_string(), "usd")),
            "手续费、滑点和 gas 合计超过风控阈值。",
        );
        return Ok(());
    }

    draft.pass(
        "fee-slippage",
        RiskCheckType::FeeAndGasInclusion,
        "CHECK_PASSED",
        Some(MeasuredDraft::decimal(
            policy.max_total_fee_slippage_usd().to_string(),
            "usd",
        )),
        Some(MeasuredDraft::decimal(total.to_string(), "usd")),
        "手续费、滑点和 gas 估算均在阈值内。",
    );
    Ok(())
}

fn check_margin(
    input: RiskEvaluationInput<'_>,
    policy: &RiskPolicySnapshot,
    draft: &mut DecisionDraft,
) -> RiskResult<()> {
    if input
        .candidate
        .risk_flags
        .contains(&RiskFlag::MarginInsufficient)
    {
        draft.fail(
            "margin",
            RiskCheckType::MarginSufficiency,
            "MARGIN_INSUFFICIENT",
            None,
            Some(MeasuredDraft::string(
                RiskFlag::MarginInsufficient.as_str(),
                "candidate_risk_flag",
            )),
            "候选转换声明保证金不足，风控拒绝。",
        );
        return Ok(());
    }

    if input
        .candidate
        .risk_flags
        .contains(&RiskFlag::LiquidationTooClose)
    {
        draft.fail(
            "liquidation-distance",
            RiskCheckType::LiquidationDistance,
            "LIQUIDATION_TOO_CLOSE",
            None,
            Some(MeasuredDraft::string(
                RiskFlag::LiquidationTooClose.as_str(),
                "candidate_risk_flag",
            )),
            "候选转换声明强平距离过近，风控拒绝。",
        );
        return Ok(());
    }

    if let Some(impact) = input.candidate.margin_impact.as_ref() {
        let Some(impact_usd) = impact.impact_usd.as_ref() else {
            draft.fail(
                "margin",
                RiskCheckType::MarginSufficiency,
                "UNKNOWN_STATE",
                None,
                Some(MeasuredDraft::string("missing impact_usd", "margin_impact")),
                "候选转换声明保证金影响但缺少数值，未知状态不能批准。",
            );
            return Ok(());
        };
        let observed = Decimal::from_str(impact_usd.as_str())?;
        let minimum = policy.min_margin_buffer_usd().checked_neg()?;
        if decimal_cmp(&observed, &minimum)? == Ordering::Less {
            draft.fail(
                "margin",
                RiskCheckType::MarginSufficiency,
                "MARGIN_INSUFFICIENT",
                Some(MeasuredDraft::decimal(minimum.to_string(), "usd")),
                Some(MeasuredDraft::decimal(observed.to_string(), "usd")),
                "候选转换保证金影响低于最低缓冲阈值。",
            );
            return Ok(());
        }

        if let Some(confidence) = impact.confidence.as_ref() {
            let confidence = Decimal::from_str(confidence.as_json_number())?;
            if decimal_cmp(&confidence, &policy.min_liquidity_confidence())? == Ordering::Less {
                draft.fail(
                    "margin-confidence",
                    RiskCheckType::MarginSufficiency,
                    "UNKNOWN_STATE",
                    Some(MeasuredDraft::decimal(
                        policy.min_liquidity_confidence().to_string(),
                        "minimum_confidence",
                    )),
                    Some(MeasuredDraft::decimal(
                        confidence.to_string(),
                        "margin_confidence",
                    )),
                    "保证金影响置信度低于阈值，按未知状态拒绝。",
                );
                return Ok(());
            }
        }
    }

    draft.pass(
        "margin",
        RiskCheckType::MarginSufficiency,
        "CHECK_PASSED",
        Some(MeasuredDraft::decimal(
            policy.min_margin_buffer_usd().to_string(),
            "usd",
        )),
        Some(MeasuredDraft::string(
            "no margin insufficiency marker",
            "margin_status",
        )),
        "候选转换未声明保证金不足或强平距离过近，保证金检查通过。",
    );
    Ok(())
}

fn check_capital_reservation(
    input: RiskEvaluationInput<'_>,
    draft: &mut DecisionDraft,
) -> RiskResult<()> {
    let requirements = required_outgoing_capital(input.candidate)?;
    if requirements.is_empty() {
        draft.not_applicable(
            "capital-reservation",
            RiskCheckType::CapitalReservationAvailability,
            "NOT_APPLICABLE",
            "候选转换没有出向资本需求，资本预留检查不适用。",
        );
        return Ok(());
    }

    let mut reserved_by_asset = BTreeMap::<String, Decimal>::new();
    for reservation in &input.portfolio_state.reservations {
        if reservation.state == CapitalReservationState::ReconciledMismatch {
            draft.fail(
                "capital-reservation",
                RiskCheckType::CapitalReservationAvailability,
                "UNKNOWN_STATE",
                None,
                Some(MeasuredDraft::string(
                    reservation.reservation_id.as_str(),
                    "reservation_id",
                )),
                "资本预留存在对账不一致状态，未知状态不能批准。",
            );
            return Ok(());
        }

        let active = matches!(
            reservation.state,
            CapitalReservationState::Requested
                | CapitalReservationState::Reserved
                | CapitalReservationState::ConvertedToExecution
        );
        if !active {
            continue;
        }

        let expires_at = UtcTimestamp::parse_rfc3339_z(reservation.expires_at.as_str())?;
        if expires_at <= input.evaluated_at {
            draft.fail(
                "capital-reservation",
                RiskCheckType::CapitalReservationAvailability,
                "UNKNOWN_STATE",
                None,
                Some(MeasuredDraft::string(
                    reservation.reservation_id.as_str(),
                    "expired_reservation_id",
                )),
                "活跃资本预留已经过期但状态未关闭，未知状态不能批准。",
            );
            return Ok(());
        }

        if reservation.reserved_for.as_str() == input.candidate.transition_id.as_str() {
            continue;
        }

        add_decimal_to_asset_map(
            &mut reserved_by_asset,
            reservation.asset_id.as_str(),
            Decimal::from_str(reservation.amount.as_str())?,
        )?;
    }

    let balances = available_balances(input.portfolio_state)?;
    for (key, required) in requirements {
        let available = available_for_requirement(&balances, &key);
        let reserved = reserved_by_asset
            .get(&key.asset_id)
            .copied()
            .unwrap_or_else(decimal_zero);
        if reserved.is_zero() {
            continue;
        }
        let available_after_reservations = available.checked_sub(reserved)?;
        if decimal_cmp(&available_after_reservations, &required)? == Ordering::Less {
            draft.fail(
                "capital-reservation",
                RiskCheckType::CapitalReservationAvailability,
                "CAPITAL_RESERVED",
                Some(MeasuredDraft::string(
                    format!("{} required={required}", key.describe()),
                    "required_capital",
                )),
                Some(MeasuredDraft::string(
                    format!(
                        "{} free_after_reservations={available_after_reservations}",
                        key.describe()
                    ),
                    "available_after_reservations",
                )),
                "已有资本预留占用资金，候选转换不能批准。",
            );
            return Ok(());
        }
    }

    draft.pass(
        "capital-reservation",
        RiskCheckType::CapitalReservationAvailability,
        "CHECK_PASSED",
        None,
        Some(MeasuredDraft::string(
            "capital available after active reservations",
            "reservation_status",
        )),
        "活跃资本预留不阻断候选转换资本需求。",
    );
    Ok(())
}

fn check_daily_loss(
    input: RiskEvaluationInput<'_>,
    policy: &RiskPolicySnapshot,
    draft: &mut DecisionDraft,
) -> RiskResult<()> {
    let mut unrealized_pnl = decimal_zero();
    for position in &input.portfolio_state.positions {
        unrealized_pnl =
            unrealized_pnl.checked_add(Decimal::from_str(position.unrealized_pnl.as_str())?)?;
    }

    let observed_loss = if unrealized_pnl.is_negative() {
        unrealized_pnl.checked_neg()?
    } else {
        decimal_zero()
    };

    if decimal_cmp(&observed_loss, &policy.max_daily_loss_usd())? == Ordering::Greater {
        draft.fail(
            "daily-loss",
            RiskCheckType::DailyLossLimit,
            "DAILY_LOSS_LIMIT",
            Some(MeasuredDraft::decimal(
                policy.max_daily_loss_usd().to_string(),
                "usd",
            )),
            Some(MeasuredDraft::decimal(observed_loss.to_string(), "usd")),
            "组合状态未实现亏损超过日亏损阈值，风控拒绝。",
        );
        return Ok(());
    }

    draft.pass(
        "daily-loss",
        RiskCheckType::DailyLossLimit,
        "CHECK_PASSED",
        Some(MeasuredDraft::decimal(
            policy.max_daily_loss_usd().to_string(),
            "usd",
        )),
        Some(MeasuredDraft::decimal(observed_loss.to_string(), "usd")),
        "组合状态未实现亏损未超过日亏损阈值。",
    );
    Ok(())
}

fn check_balances(input: RiskEvaluationInput<'_>, draft: &mut DecisionDraft) -> RiskResult<()> {
    let requirements = required_outgoing_capital(input.candidate)?;
    if requirements.is_empty() {
        draft.not_applicable(
            "balance",
            RiskCheckType::BalanceSufficiency,
            "NOT_APPLICABLE",
            "候选转换没有出向资本需求，余额检查不适用。",
        );
        return Ok(());
    }

    let balances = available_balances(input.portfolio_state)?;
    for (key, required) in requirements {
        let available = available_for_requirement(&balances, &key);
        if decimal_cmp(&available, &required)? == Ordering::Less {
            draft.fail(
                "balance",
                RiskCheckType::BalanceSufficiency,
                "INSUFFICIENT_BALANCE",
                Some(MeasuredDraft::string(
                    format!("{} required={required}", key.describe()),
                    "required_capital",
                )),
                Some(MeasuredDraft::string(
                    format!("{} free={available}", key.describe()),
                    "available_balance",
                )),
                "组合状态可用余额不足以覆盖候选转换资本需求。",
            );
            return Ok(());
        }
    }

    draft.pass(
        "balance",
        RiskCheckType::BalanceSufficiency,
        "CHECK_PASSED",
        Some(MeasuredDraft::string(
            "all required outflows covered",
            "required_capital",
        )),
        Some(MeasuredDraft::string(
            "portfolio free balances cover requirements",
            "available_balance",
        )),
        "组合状态可用余额覆盖候选转换资本需求。",
    );
    Ok(())
}

fn check_unknown_state(input: RiskEvaluationInput<'_>, draft: &mut DecisionDraft) {
    if !input.portfolio_state.missing_data_flags.is_empty() {
        let flags = input
            .portfolio_state
            .missing_data_flags
            .iter()
            .map(|flag| flag.as_str())
            .collect::<Vec<_>>()
            .join(",");
        draft.fail(
            "unknown-state",
            RiskCheckType::ReconciliationCompleteness,
            "UNKNOWN_STATE",
            None,
            Some(MeasuredDraft::string(flags, "missing_data_flags")),
            "组合状态带有缺失数据标记，未知状态按风险处理。",
        );
        return;
    }

    if let Some(order) = input
        .portfolio_state
        .open_orders
        .iter()
        .find(|order| order.status == OpenOrderStatus::Unknown)
    {
        draft.fail(
            "unknown-state",
            RiskCheckType::ReconciliationCompleteness,
            "UNKNOWN_STATE",
            None,
            Some(MeasuredDraft::string(order.order_id.as_str(), "order_id")),
            "组合状态包含未知订单状态，不能批准。",
        );
        return;
    }

    if let Some(transfer) = input
        .portfolio_state
        .pending_transfers
        .iter()
        .find(|transfer| transfer.status == PendingTransferStatus::Unknown)
    {
        draft.fail(
            "unknown-state",
            RiskCheckType::ReconciliationCompleteness,
            "UNKNOWN_STATE",
            None,
            Some(MeasuredDraft::string(
                transfer.transfer_id.as_str(),
                "transfer_id",
            )),
            "组合状态包含未知转账状态，不能批准。",
        );
        return;
    }

    if let Some(reservation) = input
        .portfolio_state
        .reservations
        .iter()
        .find(|reservation| reservation.state == CapitalReservationState::ReconciledMismatch)
    {
        draft.fail(
            "unknown-state",
            RiskCheckType::ReconciliationCompleteness,
            "UNKNOWN_STATE",
            None,
            Some(MeasuredDraft::string(
                reservation.reservation_id.as_str(),
                "reservation_id",
            )),
            "资本预留对账不一致，不能批准。",
        );
        return;
    }

    if input
        .candidate
        .failure_modes
        .contains(&FailureMode::UnknownState)
        || input
            .candidate
            .legs
            .iter()
            .any(|leg| leg.failure_modes.contains(&FailureMode::UnknownState))
        || input.candidate.risk_flags.contains(&RiskFlag::UnknownState)
    {
        draft.fail(
            "unknown-state",
            RiskCheckType::ReconciliationCompleteness,
            "UNKNOWN_STATE",
            None,
            Some(MeasuredDraft::string(
                input.candidate.transition_id.as_str(),
                "transition_id",
            )),
            "候选转换声明未知状态，不能批准。",
        );
        return;
    }

    draft.pass(
        "unknown-state",
        RiskCheckType::ReconciliationCompleteness,
        "CHECK_PASSED",
        None,
        Some(MeasuredDraft::string(
            "no unknown state markers",
            "unknown_state_status",
        )),
        "组合状态和候选转换没有未知状态标记。",
    );
}

fn check_candidate_risk_markers(
    candidate: &CandidatePortfolioTransition,
    draft: &mut DecisionDraft,
) {
    for failure_mode in &candidate.failure_modes {
        match failure_mode {
            FailureMode::UnknownState => {
                draft.fail(
                    "failure-mode",
                    RiskCheckType::OneLegExecutionRisk,
                    "UNKNOWN_STATE",
                    None,
                    Some(MeasuredDraft::string(
                        failure_mode.as_str(),
                        "candidate_failure_mode",
                    )),
                    "候选转换声明未知状态失败模式，不能批准。",
                );
                return;
            }
            FailureMode::ManualInterventionRequired => {
                draft.manual(
                    "failure-mode-manual",
                    RiskCheckType::OneLegExecutionRisk,
                    "REQUIRES_MANUAL_APPROVAL",
                    Some(MeasuredDraft::string(
                        failure_mode.as_str(),
                        "candidate_failure_mode",
                    )),
                    "候选转换声明需要人工介入，风控要求人工审批。",
                );
                return;
            }
            _ => {}
        }
    }

    for risk_flag in &candidate.risk_flags {
        if let Some(reason_code) = blocking_reason_for_risk_flag(risk_flag) {
            draft.fail(
                "risk-flag",
                risk_check_type_for_flag(risk_flag),
                reason_code,
                None,
                Some(MeasuredDraft::string(
                    risk_flag.as_str(),
                    "candidate_risk_flag",
                )),
                "候选转换包含阻断型风险标记，风控拒绝。",
            );
            return;
        }
        if manual_reason_for_risk_flag(risk_flag).is_some() {
            draft.manual(
                "risk-flag-manual",
                risk_check_type_for_flag(risk_flag),
                "REQUIRES_MANUAL_APPROVAL",
                Some(MeasuredDraft::string(
                    risk_flag.as_str(),
                    "candidate_risk_flag",
                )),
                "候选转换包含需人工复核的风险标记，风控要求人工审批。",
            );
            return;
        }
    }

    draft.pass(
        "risk-markers",
        RiskCheckType::OneLegExecutionRisk,
        "CHECK_PASSED",
        None,
        Some(MeasuredDraft::string(
            "no blocking risk flags",
            "risk_markers",
        )),
        "候选转换没有阻断型风险标记。",
    );
}

fn used_venue_ids(candidate: &CandidatePortfolioTransition) -> BTreeSet<String> {
    candidate
        .legs
        .iter()
        .filter(|leg| leg.leg_type != TransitionLegType::Observation)
        .filter_map(|leg| leg.venue_id.as_ref())
        .map(|venue_id| venue_id.as_str().to_owned())
        .collect()
}

fn used_account_ids(candidate: &CandidatePortfolioTransition) -> BTreeSet<String> {
    let mut account_ids = BTreeSet::new();
    for leg in &candidate.legs {
        if let Some(account_id) = &leg.account_id {
            account_ids.insert(account_id.as_str().to_owned());
        }
        for flow in &leg.asset_flows {
            if let Some(account_id) = &flow.account_id {
                account_ids.insert(account_id.as_str().to_owned());
            }
        }
    }
    for flow in &candidate.expected_post_state_delta.asset_flows {
        if let Some(account_id) = &flow.account_id {
            account_ids.insert(account_id.as_str().to_owned());
        }
    }
    for position_delta in &candidate.expected_post_state_delta.position_deltas {
        if let Some(account_id) = &position_delta.account_id {
            account_ids.insert(account_id.as_str().to_owned());
        }
    }
    for flow in &candidate.required_capital.asset_requirements {
        if let Some(account_id) = &flow.account_id {
            account_ids.insert(account_id.as_str().to_owned());
        }
    }
    account_ids
}

fn used_instrument_ids(candidate: &CandidatePortfolioTransition) -> BTreeSet<String> {
    let mut instrument_ids = BTreeSet::new();
    for leg in &candidate.legs {
        if let Some(instrument_id) = &leg.instrument_id {
            instrument_ids.insert(instrument_id.as_str().to_owned());
        }
    }
    for position_delta in &candidate.expected_post_state_delta.position_deltas {
        instrument_ids.insert(position_delta.instrument_id.as_str().to_owned());
    }
    instrument_ids
}

fn used_asset_ids(candidate: &CandidatePortfolioTransition) -> BTreeSet<String> {
    let mut asset_ids = BTreeSet::new();
    for leg in &candidate.legs {
        for flow in &leg.asset_flows {
            asset_ids.insert(flow.asset_id.as_str().to_owned());
        }
    }
    for flow in &candidate.expected_post_state_delta.asset_flows {
        asset_ids.insert(flow.asset_id.as_str().to_owned());
    }
    for reserve_delta in candidate
        .expected_post_state_delta
        .reserve_deltas
        .iter()
        .flatten()
    {
        asset_ids.insert(reserve_delta.asset_id.as_str().to_owned());
    }
    for flow in &candidate.required_capital.asset_requirements {
        asset_ids.insert(flow.asset_id.as_str().to_owned());
    }
    asset_ids
}

fn find_venue_capability<'a>(
    capabilities: &'a [VenueCapabilityDescriptor],
    venue_id: &str,
) -> Option<&'a VenueCapabilityDescriptor> {
    capabilities
        .iter()
        .find(|capability| capability.venue_id.as_str() == venue_id)
}

fn has_execution_capability(
    capability: &VenueCapabilityDescriptor,
    expected: &ExecutionCapability,
) -> bool {
    capability
        .execution_capabilities
        .iter()
        .any(|capability| capability == expected)
}

fn freshness_threshold_ms(
    input: RiskEvaluationInput<'_>,
    policy: &RiskPolicySnapshot,
    used_venues: &BTreeSet<String>,
) -> u64 {
    used_venues
        .iter()
        .filter_map(|venue_id| find_venue_capability(input.venue_capabilities, venue_id))
        .map(|capability| capability.health_model.freshness_threshold_ms.as_u64())
        .chain(std::iter::once(policy.max_portfolio_state_age_ms()))
        .min()
        .unwrap_or_else(|| policy.max_portfolio_state_age_ms())
}

fn elapsed_ms(start: &str, end: UtcTimestamp) -> RiskResult<u64> {
    let start = UtcTimestamp::parse_rfc3339_z(start)?;
    let start_ms = timestamp_millis(start);
    let end_ms = timestamp_millis(end);
    if end_ms < start_ms {
        return Err(RiskError::InvalidInput {
            field: "evaluated_at",
            message: "evaluation time is before portfolio state timestamp".to_owned(),
        });
    }
    Ok((end_ms - start_ms) as u64)
}

fn timestamp_millis(value: UtcTimestamp) -> i128 {
    i128::from(value.unix_seconds()) * 1_000 + i128::from(value.nanoseconds() / 1_000_000)
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct CapitalKey {
    asset_id: String,
    account_id: Option<String>,
}

impl CapitalKey {
    fn describe(&self) -> String {
        match &self.account_id {
            Some(account_id) => format!("asset={} account={account_id}", self.asset_id),
            None => format!("asset={} account=*", self.asset_id),
        }
    }
}

fn required_outgoing_capital(
    candidate: &CandidatePortfolioTransition,
) -> RiskResult<BTreeMap<CapitalKey, Decimal>> {
    let mut requirements = BTreeMap::new();
    for flow in &candidate.required_capital.asset_requirements {
        if flow.direction != AssetFlowDirection::Out {
            continue;
        }
        let key = CapitalKey {
            asset_id: flow.asset_id.as_str().to_owned(),
            account_id: flow
                .account_id
                .as_ref()
                .map(|value| value.as_str().to_owned()),
        };
        let amount = Decimal::from_str(flow.amount.as_str())?;
        add_decimal_to_map(&mut requirements, key, amount)?;
    }
    Ok(requirements)
}

fn available_balances(
    portfolio_state: &PortfolioState,
) -> RiskResult<BTreeMap<CapitalKey, Decimal>> {
    let mut balances = BTreeMap::new();
    for balance in &portfolio_state.balances {
        let key = CapitalKey {
            asset_id: balance.asset_id.as_str().to_owned(),
            account_id: Some(balance.account_id.as_str().to_owned()),
        };
        let free = Decimal::from_str(balance.free.as_str())?;
        add_decimal_to_map(&mut balances, key, free)?;
    }
    Ok(balances)
}

fn add_decimal_to_map(
    values: &mut BTreeMap<CapitalKey, Decimal>,
    key: CapitalKey,
    amount: Decimal,
) -> RiskResult<()> {
    let current = values
        .remove(&key)
        .unwrap_or_else(|| Decimal::from_scaled_atoms(0, 0));
    values.insert(key, current.checked_add(amount)?);
    Ok(())
}

fn add_decimal_to_asset_map(
    values: &mut BTreeMap<String, Decimal>,
    asset_id: &str,
    amount: Decimal,
) -> RiskResult<()> {
    let current = values.remove(asset_id).unwrap_or_else(decimal_zero);
    values.insert(asset_id.to_owned(), current.checked_add(amount)?);
    Ok(())
}

fn available_for_requirement(
    balances: &BTreeMap<CapitalKey, Decimal>,
    requirement: &CapitalKey,
) -> Decimal {
    if let Some(account_id) = &requirement.account_id {
        return balances
            .get(&CapitalKey {
                asset_id: requirement.asset_id.clone(),
                account_id: Some(account_id.clone()),
            })
            .copied()
            .unwrap_or_else(|| Decimal::from_scaled_atoms(0, 0));
    }

    balances
        .iter()
        .filter(|(key, _)| key.asset_id == requirement.asset_id)
        .map(|(_, value)| *value)
        .try_fold(Decimal::from_scaled_atoms(0, 0), Decimal::checked_add)
        .unwrap_or_else(|_| Decimal::from_scaled_atoms(0, 0))
}

fn decimal_cmp(left: &Decimal, right: &Decimal) -> RiskResult<Ordering> {
    left.partial_cmp(right)
        .ok_or_else(|| RiskError::InvalidInput {
            field: "decimal",
            message: "decimal comparison overflowed".to_owned(),
        })
}

fn decimal_zero() -> Decimal {
    Decimal::from_scaled_atoms(0, 0)
}

fn blocking_reason_for_risk_flag(flag: &RiskFlag) -> Option<&'static str> {
    match flag {
        RiskFlag::StaleMarketData => Some("DATA_STALE"),
        RiskFlag::InsufficientLiquidity => Some("INSUFFICIENT_LIQUIDITY"),
        RiskFlag::HighGas => Some("HIGH_GAS"),
        RiskFlag::HighSlippage => Some("HIGH_SLIPPAGE"),
        RiskFlag::InventoryLimitExceeded => Some("INVENTORY_LIMIT_EXCEEDED"),
        RiskFlag::MarginInsufficient => Some("MARGIN_INSUFFICIENT"),
        RiskFlag::LiquidationTooClose => Some("LIQUIDATION_TOO_CLOSE"),
        RiskFlag::FundingRateUnstable => Some("FUNDING_UNSTABLE"),
        RiskFlag::VenueUnhealthy => Some("VENUE_UNHEALTHY"),
        RiskFlag::ApiRateLimited => Some("RATE_LIMITED"),
        RiskFlag::UnknownState => Some("UNKNOWN_STATE"),
        _ => None,
    }
}

fn manual_reason_for_risk_flag(flag: &RiskFlag) -> Option<&'static str> {
    match flag {
        RiskFlag::BasisWidening
        | RiskFlag::OneLegExecutionRisk
        | RiskFlag::ChainCongested
        | RiskFlag::OracleDivergence
        | RiskFlag::SettlementDelay
        | RiskFlag::CustodyRisk
        | RiskFlag::BridgeRisk
        | RiskFlag::ModelUncertainty => Some("REQUIRES_MANUAL_APPROVAL"),
        _ => None,
    }
}

fn risk_check_type_for_flag(flag: &RiskFlag) -> RiskCheckType {
    match flag {
        RiskFlag::StaleMarketData => RiskCheckType::DataFreshness,
        RiskFlag::InsufficientLiquidity => RiskCheckType::LiquiditySufficiency,
        RiskFlag::HighGas => RiskCheckType::FeeAndGasInclusion,
        RiskFlag::HighSlippage => RiskCheckType::SlippageBounds,
        RiskFlag::InventoryLimitExceeded => RiskCheckType::InventoryBounds,
        RiskFlag::MarginInsufficient => RiskCheckType::MarginSufficiency,
        RiskFlag::LiquidationTooClose => RiskCheckType::LiquidationDistance,
        RiskFlag::FundingRateUnstable => RiskCheckType::FundingRateUncertainty,
        RiskFlag::BasisWidening => RiskCheckType::BasisWideningRisk,
        RiskFlag::VenueUnhealthy => RiskCheckType::VenueHealth,
        RiskFlag::ChainCongested => RiskCheckType::ChainCongestion,
        RiskFlag::ApiRateLimited => RiskCheckType::RateLimitState,
        RiskFlag::OneLegExecutionRisk => RiskCheckType::OneLegExecutionRisk,
        RiskFlag::OracleDivergence => RiskCheckType::CorrelationConcentrationLimit,
        RiskFlag::SettlementDelay => RiskCheckType::ReconciliationCompleteness,
        RiskFlag::CustodyRisk | RiskFlag::BridgeRisk | RiskFlag::ModelUncertainty => {
            RiskCheckType::StrategyExposureLimit
        }
        RiskFlag::UnknownState => RiskCheckType::ReconciliationCompleteness,
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct DecisionDraft {
    checks: Vec<CheckDraft>,
    blocking_reason_codes: BTreeSet<String>,
    manual_reason_codes: BTreeSet<String>,
    more_data_reason_codes: BTreeSet<String>,
    constraint_reason_codes: BTreeSet<String>,
    constraints: Vec<ConstraintDraft>,
}

impl DecisionDraft {
    fn pass(
        &mut self,
        suffix: &'static str,
        check_type: RiskCheckType,
        reason_code: &'static str,
        threshold: Option<MeasuredDraft>,
        observed: Option<MeasuredDraft>,
        detail: &'static str,
    ) {
        self.checks.push(CheckDraft {
            suffix,
            check_type,
            status: RiskCheckStatus::Pass,
            severity: RiskSeverity::Info,
            threshold,
            observed,
            reason_code: reason_code.to_owned(),
            detail: detail.to_owned(),
        });
    }

    fn not_applicable(
        &mut self,
        suffix: &'static str,
        check_type: RiskCheckType,
        reason_code: &'static str,
        detail: &'static str,
    ) {
        self.checks.push(CheckDraft {
            suffix,
            check_type,
            status: RiskCheckStatus::NotApplicable,
            severity: RiskSeverity::Info,
            threshold: None,
            observed: None,
            reason_code: reason_code.to_owned(),
            detail: detail.to_owned(),
        });
    }

    fn fail(
        &mut self,
        suffix: &'static str,
        check_type: RiskCheckType,
        reason_code: &'static str,
        threshold: Option<MeasuredDraft>,
        observed: Option<MeasuredDraft>,
        detail: &'static str,
    ) {
        self.blocking_reason_codes.insert(reason_code.to_owned());
        self.checks.push(CheckDraft {
            suffix,
            check_type,
            status: RiskCheckStatus::Fail,
            severity: RiskSeverity::Block,
            threshold,
            observed,
            reason_code: reason_code.to_owned(),
            detail: detail.to_owned(),
        });
    }

    fn manual(
        &mut self,
        suffix: &'static str,
        check_type: RiskCheckType,
        reason_code: &'static str,
        observed: Option<MeasuredDraft>,
        detail: &'static str,
    ) {
        self.manual_reason_codes.insert(reason_code.to_owned());
        self.checks.push(CheckDraft {
            suffix,
            check_type,
            status: RiskCheckStatus::Warning,
            severity: RiskSeverity::Warn,
            threshold: Some(MeasuredDraft::string(
                "manual approval required before execution",
                "approval_gate",
            )),
            observed,
            reason_code: reason_code.to_owned(),
            detail: detail.to_owned(),
        });
    }

    fn more_data(
        &mut self,
        suffix: &'static str,
        check_type: RiskCheckType,
        reason_code: &'static str,
        threshold: Option<MeasuredDraft>,
        observed: Option<MeasuredDraft>,
        detail: &'static str,
    ) {
        self.more_data_reason_codes.insert(reason_code.to_owned());
        self.checks.push(CheckDraft {
            suffix,
            check_type,
            status: RiskCheckStatus::Unknown,
            severity: RiskSeverity::Block,
            threshold,
            observed,
            reason_code: reason_code.to_owned(),
            detail: detail.to_owned(),
        });
    }

    fn approve_with_constraint(&mut self, check: CheckDraft, constraint: ConstraintDraft) {
        self.constraint_reason_codes
            .insert(check.reason_code.clone());
        self.checks.push(check);
        self.constraints.push(constraint);
    }

    fn decision_kind(&self) -> RiskDecisionKind {
        if !self.blocking_reason_codes.is_empty() {
            RiskDecisionKind::Rejected
        } else if !self.manual_reason_codes.is_empty() {
            RiskDecisionKind::RequiresManualApproval
        } else if !self.more_data_reason_codes.is_empty() {
            RiskDecisionKind::RequiresMoreData
        } else if !self.constraints.is_empty() {
            RiskDecisionKind::ApprovedWithConstraints
        } else {
            RiskDecisionKind::Approved
        }
    }

    fn reason_codes_for(&self, decision: RiskDecisionKind) -> Vec<String> {
        match decision {
            RiskDecisionKind::Rejected => self
                .blocking_reason_codes
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            RiskDecisionKind::RequiresManualApproval => {
                self.manual_reason_codes.iter().cloned().collect::<Vec<_>>()
            }
            RiskDecisionKind::Approved => vec!["APPROVED".to_owned()],
            RiskDecisionKind::ApprovedWithConstraints => self
                .constraint_reason_codes
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            RiskDecisionKind::RequiresMoreData => self
                .more_data_reason_codes
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            RiskDecisionKind::SuspendedByCircuitBreaker => {
                vec!["CIRCUIT_BREAKER_TRIGGERED".to_owned()]
            }
        }
    }

    fn constraints_for(&self, decision: RiskDecisionKind) -> Vec<ConstraintDraft> {
        let mut constraints = self.constraints.clone();
        if decision == RiskDecisionKind::RequiresManualApproval {
            constraints.push(ConstraintDraft {
                suffix: "manual-approval",
                constraint_type: RiskConstraintType::RequiresManualApproval,
                field_path: "$.decision".to_owned(),
                limit: Some(MeasuredDraft::string(
                    "manual approval must reference the same transition hash",
                    "approval_requirement",
                )),
                expires_at: None,
            });
        }
        constraints
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CheckDraft {
    suffix: &'static str,
    check_type: RiskCheckType,
    status: RiskCheckStatus,
    severity: RiskSeverity,
    threshold: Option<MeasuredDraft>,
    observed: Option<MeasuredDraft>,
    reason_code: String,
    detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConstraintDraft {
    suffix: &'static str,
    constraint_type: RiskConstraintType,
    field_path: String,
    limit: Option<MeasuredDraft>,
    expires_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MeasuredDraft {
    decimal_value: Option<String>,
    string_value: Option<String>,
    unit: Option<String>,
}

impl MeasuredDraft {
    fn decimal(value: impl Into<String>, unit: impl Into<String>) -> Self {
        Self {
            decimal_value: Some(value.into()),
            string_value: None,
            unit: Some(unit.into()),
        }
    }

    fn string(value: impl Into<String>, unit: impl Into<String>) -> Self {
        Self {
            decimal_value: None,
            string_value: Some(value.into()),
            unit: Some(unit.into()),
        }
    }
}

fn decision_detail(decision: RiskDecisionKind) -> &'static str {
    match decision {
        RiskDecisionKind::Approved => {
            "风控入口批准候选转换；后续仍必须受执行模式、资本预留、kill switch 和权限约束。"
        }
        RiskDecisionKind::RequiresManualApproval => {
            "风控入口要求人工审批；审批不能修改计划后沿用旧审批。"
        }
        RiskDecisionKind::Rejected => "风控入口拒绝候选转换，不得生成可执行计划。",
        RiskDecisionKind::ApprovedWithConstraints => "风控入口带约束批准候选转换。",
        RiskDecisionKind::RequiresMoreData => "风控入口需要更多数据，不能批准。",
        RiskDecisionKind::SuspendedByCircuitBreaker => "风控入口被熔断暂停，不能批准。",
    }
}

fn render_decision_json(
    input: RiskEvaluationInput<'_>,
    policy: &RiskPolicySnapshot,
    decision: RiskDecisionKind,
    checks: &[CheckDraft],
    constraints: &[ConstraintDraft],
    reason_codes: &[String],
    detail: &str,
) -> String {
    format!(
        r#"{{
  "schema_version": "1.0.0",
  "decision_id": {},
  "transition_id": {},
  "evaluated_at": {},
  "decision": {},
  "policy_version": {},
  "policy_hash": {},
  "policy_signature_ref": {},
  "input_state_ref": {},
  "checks": {},
  "constraints": {},
  "reason_codes": {},
  "detail": {}
}}"#,
        json_string(&derived_identifier(
            "risk",
            input.candidate.transition_id.as_str()
        )),
        json_string(input.candidate.transition_id.as_str()),
        json_string(&input.evaluated_at.to_string()),
        json_string(decision.as_str()),
        json_string(policy.policy_version()),
        json_string(policy.policy_hash()),
        json_string(policy.policy_signature_ref()),
        json_string(input.portfolio_state.portfolio_state_id.as_str()),
        render_checks(input, checks),
        render_constraints(input, constraints),
        json_string_array(reason_codes),
        json_string(detail),
    )
}

fn render_checks(input: RiskEvaluationInput<'_>, checks: &[CheckDraft]) -> String {
    let mut out = String::from("[");
    for (index, check) in checks.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let mut fields = vec![
            (
                "check_id",
                json_string(&derived_identifier(
                    "check",
                    &format!(
                        "{}:{}",
                        input.candidate.transition_id.as_str(),
                        check.suffix
                    ),
                )),
            ),
            ("check_type", json_string(check.check_type.as_str())),
            ("status", json_string(check.status.as_str())),
            ("severity", json_string(check.severity.as_str())),
        ];
        if let Some(threshold) = check.threshold.as_ref() {
            fields.push(("threshold", render_measured(threshold)));
        }
        if let Some(observed) = check.observed.as_ref() {
            fields.push(("observed", render_measured(observed)));
        }
        fields.extend([
            ("reason_code", json_string(&check.reason_code)),
            ("detail", json_string(&check.detail)),
        ]);
        out.push_str(&render_json_object(fields));
    }
    out.push(']');
    out
}

fn render_constraints(input: RiskEvaluationInput<'_>, constraints: &[ConstraintDraft]) -> String {
    let mut out = String::from("[");
    for (index, constraint) in constraints.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let mut fields = vec![
            (
                "constraint_id",
                json_string(&derived_identifier(
                    "constraint",
                    &format!(
                        "{}:{}",
                        input.candidate.transition_id.as_str(),
                        constraint.suffix
                    ),
                )),
            ),
            (
                "constraint_type",
                json_string(constraint.constraint_type.as_str()),
            ),
            ("field_path", json_string(&constraint.field_path)),
        ];
        if let Some(limit) = constraint.limit.as_ref() {
            fields.push(("limit", render_measured(limit)));
        }
        if let Some(expires_at) = constraint.expires_at.as_deref() {
            fields.push(("expires_at", json_string(expires_at)));
        }
        out.push_str(&render_json_object(fields));
    }
    out.push(']');
    out
}

fn render_measured(value: &MeasuredDraft) -> String {
    let mut fields = Vec::new();
    if let Some(decimal_value) = value.decimal_value.as_deref() {
        fields.push(("decimal_value", json_string(decimal_value)));
    }
    if let Some(string_value) = value.string_value.as_deref() {
        fields.push(("string_value", json_string(string_value)));
    }
    if let Some(unit) = value.unit.as_deref() {
        fields.push(("unit", json_string(unit)));
    }
    render_json_object(fields)
}

fn render_json_object(fields: Vec<(&str, String)>) -> String {
    let mut out = String::from("{");
    for (index, (key, value)) in fields.into_iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_string(key));
        out.push(':');
        out.push_str(&value);
    }
    out.push('}');
    out
}

fn json_string_array(values: &[String]) -> String {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_string(value));
    }
    out.push(']');
    out
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => {
                use std::fmt::Write as _;

                write!(out, "\\u{:04x}", ch as u32).expect("writing to a String cannot fail");
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn derived_identifier(prefix: &str, source: &str) -> String {
    let candidate = format!("{prefix}:{source}");
    if candidate.len() <= 128 {
        candidate
    } else {
        format!("{prefix}:{:016x}", fnv1a64(source.as_bytes()))
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANDIDATE: &str =
        include_str!("../../../fixtures/schema/valid/candidate_portfolio_transition.valid.json");
    const PORTFOLIO: &str =
        include_str!("../../../fixtures/schema/valid/portfolio_state.valid.json");
    const VENUE_CAPABILITY: &str =
        include_str!("../../../fixtures/schema/valid/venue_capability.valid.json");

    #[test]
    fn approves_when_candidate_state_config_and_capabilities_match() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Approved);
        assert_reason_codes(&decision, &["APPROVED"]);
        assert!(decision
            .checks
            .iter()
            .all(|check| check.status != RiskCheckStatus::Fail));
        for check_type in [
            RiskCheckType::DataFreshness,
            RiskCheckType::VenueHealth,
            RiskCheckType::LiquiditySufficiency,
            RiskCheckType::FeeAndGasInclusion,
            RiskCheckType::MarginSufficiency,
            RiskCheckType::CapitalReservationAvailability,
            RiskCheckType::DailyLossLimit,
            RiskCheckType::ReconciliationCompleteness,
        ] {
            assert_has_check(&decision, check_type, RiskCheckStatus::Pass, "CHECK_PASSED");
        }
        assert!(decision.constraints.is_empty());
        assert!(risk_decision_to_canonical_json(&decision).contains("\"decision\":\"Approved\""));
    }

    #[test]
    fn rejects_when_available_balance_is_insufficient() {
        let candidate = test_candidate();
        let portfolio_state = portfolio_state_from_json_strict(
            &PORTFOLIO.replace("\"free\": \"1000.000000\"", "\"free\": \"50.000000\""),
        )
        .expect("portfolio state");
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_reason_codes(&decision, &["INSUFFICIENT_BALANCE"]);
        assert!(decision.checks.iter().any(|check| {
            check.check_type == RiskCheckType::BalanceSufficiency
                && check.status == RiskCheckStatus::Fail
                && check.reason_code.as_str() == "INSUFFICIENT_BALANCE"
        }));
    }

    #[test]
    fn rejects_when_liquidity_is_insufficient() {
        let candidate = candidate_from_json_strict(&candidate_with_config().replace(
            "\"risk_flags\": []",
            "\"risk_flags\": [\"InsufficientLiquidity\"]",
        ))
        .expect("candidate");
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::LiquiditySufficiency,
            RiskCheckStatus::Fail,
            "INSUFFICIENT_LIQUIDITY",
        );
        assert_reason_codes(&decision, &["INSUFFICIENT_LIQUIDITY"]);
    }

    #[test]
    fn rejects_when_slippage_is_too_high() {
        let candidate = candidate_from_json_strict(&candidate_with_config().replace(
            "\"slippage_estimate_usd\": \"0.05\"",
            "\"slippage_estimate_usd\": \"2.00\"",
        ))
        .expect("candidate");
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::SlippageBounds,
            RiskCheckStatus::Fail,
            "HIGH_SLIPPAGE",
        );
        assert_reason_codes(&decision, &["HIGH_SLIPPAGE"]);
    }

    #[test]
    fn rejects_when_fee_is_too_high() {
        let candidate = candidate_from_json_strict(&candidate_with_config().replace(
            "\"fee_estimate_usd\": \"0.10\"",
            "\"fee_estimate_usd\": \"2.50\"",
        ))
        .expect("candidate");
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::FeeAndGasInclusion,
            RiskCheckStatus::Fail,
            "HIGH_FEE",
        );
        assert_reason_codes(&decision, &["HIGH_FEE"]);
        assert!(risk_decision_to_canonical_json(&decision).contains("\"HIGH_FEE\""));
    }

    #[test]
    fn rejects_when_gas_is_too_high() {
        let candidate = candidate_from_json_strict(&candidate_with_config().replace(
            "\"confidence\": 0.95",
            "\"gas_estimate_usd\": \"6.00\",\n    \"confidence\": 0.95",
        ))
        .expect("candidate");
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::FeeAndGasInclusion,
            RiskCheckStatus::Fail,
            "HIGH_GAS",
        );
        assert_reason_codes(&decision, &["HIGH_GAS"]);
    }

    #[test]
    fn rejects_when_fee_slippage_and_gas_total_is_too_high() {
        let candidate_json = candidate_with_config()
            .replace(
                "\"fee_estimate_usd\": \"0.10\"",
                "\"fee_estimate_usd\": \"1.90\"",
            )
            .replace(
                "\"slippage_estimate_usd\": \"0.05\"",
                "\"slippage_estimate_usd\": \"0.90\"",
            )
            .replace(
                "\"confidence\": 0.95",
                "\"gas_estimate_usd\": \"3.00\",\n    \"confidence\": 0.95",
            );
        let candidate = candidate_from_json_strict(&candidate_json).expect("candidate");
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::FeeAndGasInclusion,
            RiskCheckStatus::Fail,
            "HIGH_FEE_AND_SLIPPAGE",
        );
        assert_reason_codes(&decision, &["HIGH_FEE_AND_SLIPPAGE"]);
    }

    #[test]
    fn approves_with_constraints_when_slippage_is_near_limit() {
        let candidate = candidate_from_json_strict(&candidate_with_config().replace(
            "\"slippage_estimate_usd\": \"0.05\"",
            "\"slippage_estimate_usd\": \"0.90\"",
        ))
        .expect("candidate");
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::ApprovedWithConstraints);
        assert_reason_codes(&decision, &["APPROVED_WITH_CONSTRAINTS"]);
        assert_has_check(
            &decision,
            RiskCheckType::SlippageBounds,
            RiskCheckStatus::Warning,
            "APPROVED_WITH_CONSTRAINTS",
        );
        assert_eq!(decision.constraints.len(), 1);
        assert_eq!(
            decision.constraints[0].constraint_type,
            RiskConstraintType::MaxSlippage
        );
    }

    #[test]
    fn rejects_when_margin_impact_is_negative() {
        let candidate = candidate_from_json_strict(
            &candidate_with_config().replace(
                "  \"failure_modes\": [\n    \"NoOpFailure\"\n  ],\n  \"risk_flags\": []",
                "  \"margin_impact\": {\"impact_usd\": \"-1.00\", \"confidence\": 0.95},\n  \"failure_modes\": [\n    \"NoOpFailure\"\n  ],\n  \"risk_flags\": []",
            ),
        )
        .expect("candidate");
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::MarginSufficiency,
            RiskCheckStatus::Fail,
            "MARGIN_INSUFFICIENT",
        );
        assert_reason_codes(&decision, &["MARGIN_INSUFFICIENT"]);
    }

    #[test]
    fn rejects_when_liquidation_distance_is_too_close() {
        let candidate = candidate_from_json_strict(&candidate_with_config().replace(
            "\"risk_flags\": []",
            "\"risk_flags\": [\"LiquidationTooClose\"]",
        ))
        .expect("candidate");
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::LiquidationDistance,
            RiskCheckStatus::Fail,
            "LIQUIDATION_TOO_CLOSE",
        );
        assert_reason_codes(&decision, &["LIQUIDATION_TOO_CLOSE"]);
    }

    #[test]
    fn rejects_when_capital_is_already_reserved_elsewhere() {
        let portfolio_state = portfolio_state_from_json_strict(&PORTFOLIO.replace(
            "\"reservations\": [],",
            r#""reservations": [
    {
      "reservation_id": "res:other",
      "state": "Reserved",
      "asset_id": "asset:USDC",
      "amount": "950.00",
      "reserved_for": "trans:other",
      "expires_at": "2026-01-01T00:01:00Z"
    }
  ],"#,
        ))
        .expect("portfolio state");
        let candidate = test_candidate();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::CapitalReservationAvailability,
            RiskCheckStatus::Fail,
            "CAPITAL_RESERVED",
        );
        assert_reason_codes(&decision, &["CAPITAL_RESERVED"]);
    }

    #[test]
    fn rejects_when_daily_loss_limit_is_exceeded() {
        let portfolio_state = portfolio_state_from_json_strict(&PORTFOLIO.replace(
            "\"unrealized_pnl\": \"0.10\"",
            "\"unrealized_pnl\": \"-150.00\"",
        ))
        .expect("portfolio state");
        let candidate = test_candidate();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::DailyLossLimit,
            RiskCheckStatus::Fail,
            "DAILY_LOSS_LIMIT",
        );
        assert_reason_codes(&decision, &["DAILY_LOSS_LIMIT"]);
    }

    #[test]
    fn rejects_when_venue_health_is_unhealthy() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![venue_capability_from_json_strict(
            &trading_capability_json()
                .replace("\"disconnect_threshold\": 3", "\"disconnect_threshold\": 0"),
        )
        .expect("venue")];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::VenueHealth,
            RiskCheckStatus::Fail,
            "VENUE_UNHEALTHY",
        );
        assert_reason_codes(&decision, &["VENUE_UNHEALTHY"]);
    }

    #[test]
    fn rejects_when_rate_limit_is_exhausted() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![venue_capability_from_json_strict(
            &trading_capability_json().replace("\"limit\": 60", "\"limit\": 0"),
        )
        .expect("venue")];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::RateLimitState,
            RiskCheckStatus::Fail,
            "RATE_LIMITED",
        );
        assert_reason_codes(&decision, &["RATE_LIMITED"]);
    }

    #[test]
    fn rejects_unknown_state_instead_of_approving() {
        let portfolio_state = portfolio_state_from_json_strict(&PORTFOLIO.replace(
            "\"missing_data_flags\": []",
            "\"missing_data_flags\": [\"UNKNOWN_STATE\"]",
        ))
        .expect("portfolio state");
        let candidate = test_candidate();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::ReconciliationCompleteness,
            RiskCheckStatus::Fail,
            "UNKNOWN_STATE",
        );
        assert_reason_codes(&decision, &["UNKNOWN_STATE"]);
    }

    #[test]
    fn rejects_strategy_blocked_by_kill_switch() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config_with_kill_switch(
            "ReadOnly",
            r#"strategies: ["strat:demo"]
  venues: []
  accounts: []
  instruments: []
  assets: []
  chains: []"#,
        );
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::StrategyExposureLimit,
            RiskCheckStatus::Fail,
            "STRATEGY_DISABLED",
        );
        assert_reason_codes(&decision, &["STRATEGY_DISABLED"]);
    }

    #[test]
    fn rejects_venue_blocked_by_kill_switch() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config_with_kill_switch(
            "ReadOnly",
            r#"strategies: []
  venues: ["venue:SIM"]
  accounts: []
  instruments: []
  assets: []
  chains: []"#,
        );
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::VenueHealth,
            RiskCheckStatus::Fail,
            "VENUE_DISABLED",
        );
        assert_reason_codes(&decision, &["VENUE_DISABLED"]);
    }

    #[test]
    fn rejects_account_blocked_by_kill_switch() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config_with_kill_switch(
            "ReadOnly",
            r#"strategies: []
  venues: []
  accounts: ["acct:sim"]
  instruments: []
  assets: []
  chains: []"#,
        );
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::StrategyExposureLimit,
            RiskCheckStatus::Fail,
            "ACCOUNT_DISABLED",
        );
        assert_reason_codes(&decision, &["ACCOUNT_DISABLED"]);
    }

    #[test]
    fn rejects_instrument_blocked_by_kill_switch() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config_with_kill_switch(
            "ReadOnly",
            r#"strategies: []
  venues: []
  accounts: []
  instruments: ["inst:BTC-USDC"]
  assets: []
  chains: []"#,
        );
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::StrategyExposureLimit,
            RiskCheckStatus::Fail,
            "INSTRUMENT_DISABLED",
        );
        assert_reason_codes(&decision, &["INSTRUMENT_DISABLED"]);
    }

    #[test]
    fn rejects_asset_blocked_by_kill_switch() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config_with_kill_switch(
            "ReadOnly",
            r#"strategies: []
  venues: []
  accounts: []
  instruments: []
  assets: ["asset:USDC"]
  chains: []"#,
        );
        let capabilities = vec![trading_capability()];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_has_check(
            &decision,
            RiskCheckType::StrategyExposureLimit,
            RiskCheckStatus::Fail,
            "ASSET_DISABLED",
        );
        assert_reason_codes(&decision, &["ASSET_DISABLED"]);
    }

    #[test]
    fn requires_more_data_when_venue_capability_snapshot_is_missing() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = Vec::new();
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::RequiresMoreData);
        assert_reason_codes(&decision, &["REQUIRES_MORE_DATA"]);
        assert_has_check(
            &decision,
            RiskCheckType::VenueHealth,
            RiskCheckStatus::Unknown,
            "REQUIRES_MORE_DATA",
        );
    }

    #[test]
    fn requires_manual_approval_for_manual_only_venue() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities =
            vec![venue_capability_from_json_strict(VENUE_CAPABILITY).expect("venue")];
        let decision = evaluate_test_case(&candidate, &portfolio_state, &config, &capabilities);

        assert_eq!(decision.decision, RiskDecisionKind::RequiresManualApproval);
        assert_reason_codes(&decision, &["REQUIRES_MANUAL_APPROVAL"]);
        assert!(decision.checks.iter().any(|check| {
            check.status == RiskCheckStatus::Warning
                && check.reason_code.as_str() == "REQUIRES_MANUAL_APPROVAL"
        }));
        assert_eq!(decision.constraints.len(), 1);
        assert_eq!(
            decision.constraints[0].constraint_type,
            RiskConstraintType::RequiresManualApproval
        );
    }

    #[test]
    fn rejects_stale_portfolio_state() {
        let candidate = test_candidate();
        let portfolio_state = test_portfolio_state();
        let config = test_config("ReadOnly");
        let capabilities = vec![trading_capability()];
        let evaluator = StaticRiskEvaluator::default();
        let input = RiskEvaluationInput::new(
            &candidate,
            &portfolio_state,
            &config,
            &capabilities,
            UtcTimestamp::parse_rfc3339_z("2026-01-01T00:00:10Z").expect("time"),
        );
        let decision = evaluator.evaluate(input).expect("decision");

        assert_eq!(decision.decision, RiskDecisionKind::Rejected);
        assert_reason_codes(&decision, &["DATA_STALE"]);
        assert!(decision.checks.iter().any(|check| {
            check.check_type == RiskCheckType::DataFreshness
                && check.status == RiskCheckStatus::Fail
                && check.reason_code.as_str() == "DATA_STALE"
        }));
    }

    #[test]
    fn risk_crate_manifest_keeps_forbidden_dependencies_out() {
        let manifest = include_str!("../Cargo.toml");
        for forbidden in [
            "arb-execution",
            "arb-venue-exec",
            "arb-signing",
            "arb-runtime",
            "arb-ledger",
        ] {
            assert!(
                !manifest.contains(forbidden),
                "arb-risk must not depend on {forbidden}"
            );
        }
    }

    #[test]
    fn risk_replay_fixtures_match_expected_decisions() {
        assert_risk_fixture("risk_accept", RiskDecisionKind::Approved, &["APPROVED"]);
        assert_risk_fixture(
            "risk_reject",
            RiskDecisionKind::Rejected,
            &["HIGH_SLIPPAGE"],
        );
        assert_risk_fixture(
            "risk_requires_more_data",
            RiskDecisionKind::RequiresMoreData,
            &["REQUIRES_MORE_DATA"],
        );
    }

    fn evaluate_test_case(
        candidate: &CandidatePortfolioTransition,
        portfolio_state: &PortfolioState,
        config: &ArbConfig,
        capabilities: &[VenueCapabilityDescriptor],
    ) -> RiskDecision {
        let evaluator = StaticRiskEvaluator::default();
        let input = RiskEvaluationInput::new(
            candidate,
            portfolio_state,
            config,
            capabilities,
            UtcTimestamp::parse_rfc3339_z("2026-01-01T00:00:03Z").expect("time"),
        );
        evaluator.evaluate(input).expect("decision")
    }

    fn test_candidate() -> CandidatePortfolioTransition {
        candidate_from_json_strict(&candidate_with_config()).expect("candidate")
    }

    fn candidate_with_config() -> String {
        CANDIDATE.replace(
            "\"config_version\": \"cfg:demo-1\"",
            "\"config_version\": \"arb-config-v1\"",
        )
    }

    fn test_portfolio_state() -> PortfolioState {
        portfolio_state_from_json_strict(PORTFOLIO).expect("portfolio state")
    }

    fn trading_capability() -> VenueCapabilityDescriptor {
        venue_capability_from_json_strict(&trading_capability_json()).expect("venue")
    }

    fn trading_capability_json() -> String {
        VENUE_CAPABILITY
            .replace("\"SupportsManualApprovalOnly\"", "\"SupportsMarketOrders\"")
            .replace("\"can_trade\": false", "\"can_trade\": true")
    }

    fn test_config(mode: &str) -> ArbConfig {
        test_config_with_kill_switch(
            mode,
            r#"strategies: []
  venues: []
  accounts: []
  instruments: []
  assets: []
  chains: []"#,
        )
    }

    fn test_config_with_kill_switch(mode: &str, scoped_kill_switch_fields: &str) -> ArbConfig {
        ArbConfig::from_yaml_str(&format!(
            r#"
config_version: "arb-config-v1"

execution:
  mode: "{mode}"
  live_execution_enabled: false
  auto_live_enabled: false

kill_switch:
  global: false
  execution: false
  {scoped_kill_switch_fields}
  execution_modes: []

signing:
  policy_ref: "signing-policy/null-signer-v1"
  real_signing_enabled: false
"#
        ))
        .expect("config")
    }

    fn assert_reason_codes(decision: &RiskDecision, expected: &[&str]) {
        let actual = decision
            .reason_codes
            .iter()
            .map(|reason_code| reason_code.as_str())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    fn assert_has_check(
        decision: &RiskDecision,
        check_type: RiskCheckType,
        status: RiskCheckStatus,
        reason_code: &str,
    ) {
        assert!(
            decision.checks.iter().any(|check| {
                check.check_type == check_type
                    && check.status == status
                    && check.reason_code.as_str() == reason_code
            }),
            "missing check type={} status={} reason={reason_code}; checks={:?}",
            check_type.as_str(),
            status.as_str(),
            decision.checks
        );
    }

    fn assert_risk_fixture(
        case_name: &str,
        expected_kind: RiskDecisionKind,
        expected_reason_codes: &[&str],
    ) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/replay")
            .join(case_name);
        let candidate =
            candidate_from_json_strict(&read_fixture(&root, "candidate_transition.json"))
                .expect("candidate");
        let portfolio_state =
            portfolio_state_from_json_strict(&read_fixture(&root, "portfolio_state.json"))
                .expect("portfolio state");
        let config = ArbConfig::from_yaml_str(&read_fixture(&root, "config.yaml")).expect("config");
        let capabilities = read_fixture(&root, "venue_capabilities.jsonl")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| venue_capability_from_json_strict(line).expect("venue capability"))
            .collect::<Vec<_>>();
        let replay = read_fixture(&root, "replay.yaml");
        let evaluated_at = replay
            .lines()
            .find_map(|line| line.trim().strip_prefix("fixed_time: "))
            .map(|value| value.trim_matches('"'))
            .and_then(|value| UtcTimestamp::parse_rfc3339_z(value).ok())
            .expect("fixed replay time");

        let evaluator = StaticRiskEvaluator::default();
        let input = RiskEvaluationInput::new(
            &candidate,
            &portfolio_state,
            &config,
            &capabilities,
            evaluated_at,
        );
        let decision = evaluator.evaluate(input).expect("decision");
        assert_eq!(decision.decision, expected_kind);
        assert_reason_codes(&decision, expected_reason_codes);

        let expected = read_fixture(&root, "expected/risk_decisions.jsonl");
        assert_eq!(risk_decision_to_canonical_json(&decision), expected.trim());
    }

    fn read_fixture(root: &std::path::Path, relative_path: &str) -> String {
        std::fs::read_to_string(root.join(relative_path)).unwrap_or_else(|error| {
            panic!(
                "cannot read fixture {} in {}: {error}",
                relative_path,
                root.display()
            )
        })
    }
}
