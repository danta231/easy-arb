//! `arb-strategy-api` 策略只读接口。
//!
//! 中文说明：本 crate 只暴露组合快照、场所能力、只读配置和固定时间源。
//! 策略只能返回候选组合转换或明确拒绝原因，不能获得下单、签名、转账、
//! 账本写入或运行时装配能力。
//!
//! ```compile_fail
//! use arb_strategy_api::StrategyReadContext;
//!
//! fn cannot_execute(context: &dyn StrategyReadContext) {
//!     let _ = context.execution_adapter();
//! }
//! ```
//!
//! ```compile_fail
//! use arb_strategy_api::StrategyReadContext;
//!
//! fn cannot_sign(context: &dyn StrategyReadContext) {
//!     let _ = context.signing_provider();
//! }
//! ```
//!
//! ```compile_fail
//! use arb_strategy_api::StrategyReadContext;
//!
//! fn cannot_write_ledger(context: &dyn StrategyReadContext) {
//!     let _ = context.ledger_writer();
//! }
//! ```

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use arb_config::ConfigError;
pub use arb_config::ExecutionMode;
use arb_contracts::{from_json_strict, ContractError};
pub use arb_contracts::{
    to_canonical_json, AuthMode, CandidatePortfolioTransition, DataSurface, ExecutionCapability,
    Identifier, MarketCapability, NormalizedEvent, PortfolioState, SettlementMode,
    VenueCapabilityDescriptor,
};
use arb_domain::{DomainError, UtcTimestamp};

/// 策略 API 统一返回类型。
///
/// 中文说明：接口错误只表示策略 API 边界、时间或合同转换失败；策略本身
/// 因数据不足或能力不足而拒绝，应返回 `StrategyOutcome::Rejected`。
pub type StrategyApiResult<T> = Result<T, StrategyApiError>;

/// 策略 API 边界错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StrategyApiError {
    /// 输入字段为空或格式不满足策略 API 的最小要求。
    InvalidInput {
        field: &'static str,
        message: String,
    },
    /// 领域层时间或基础类型错误。
    Domain(DomainError),
    /// 合同层候选转换或 fixture 解析错误。
    Contract(ContractError),
    /// 配置层只读快照解析错误。
    Config(ConfigError),
    /// 策略输出和上下文或策略元数据不一致。
    OutputMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for StrategyApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { field, message } => {
                write!(f, "{field}: invalid strategy API input: {message}")
            }
            Self::Domain(error) => write!(f, "{error}"),
            Self::Contract(error) => write!(f, "{error}"),
            Self::Config(error) => write!(f, "{error}"),
            Self::OutputMismatch {
                field,
                expected,
                actual,
            } => write!(
                f,
                "{field}: strategy output mismatch, expected `{expected}`, got `{actual}`"
            ),
        }
    }
}

impl Error for StrategyApiError {}

impl From<DomainError> for StrategyApiError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<ContractError> for StrategyApiError {
    fn from(error: ContractError) -> Self {
        Self::Contract(error)
    }
}

impl From<ConfigError> for StrategyApiError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

/// 固定时间源和可回放时间源接口。
///
/// 中文说明：策略不能直接读取系统时间。回放、测试和线上装配都必须显式
/// 注入时间源。
pub trait StrategyTimeSource {
    /// 返回当前策略时间。
    fn now(&self) -> UtcTimestamp;

    /// 返回 RFC3339 UTC 字符串，便于填充合同字段。
    fn now_rfc3339_z(&self) -> String {
        self.now().to_string()
    }
}

/// 固定 UTC 时间源。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FixedTimeSource {
    now: UtcTimestamp,
}

impl FixedTimeSource {
    /// 使用已解析的 UTC 时间创建固定时间源。
    pub fn new(now: UtcTimestamp) -> Self {
        Self { now }
    }

    /// 从严格 RFC3339 UTC 字符串创建固定时间源。
    pub fn from_rfc3339_z(value: &str) -> StrategyApiResult<Self> {
        Ok(Self {
            now: UtcTimestamp::parse_rfc3339_z(value)?,
        })
    }
}

impl StrategyTimeSource for FixedTimeSource {
    fn now(&self) -> UtcTimestamp {
        self.now
    }
}

/// 组合状态只读快照接口。
///
/// 中文说明：接口只返回不可变引用，不提供账户修改、执行动作或账本写入。
pub trait PortfolioSnapshotReader {
    /// 返回当前组合状态合同。
    fn portfolio_state(&self) -> &PortfolioState;

    /// 返回当前组合状态 ID。
    fn portfolio_state_id(&self) -> &str {
        self.portfolio_state().portfolio_state_id.as_str()
    }

    /// 返回构成该状态的输入事件引用。
    fn source_event_refs(&self) -> &[Identifier] {
        &self.portfolio_state().source_event_refs
    }
}

/// 市场事件只读快照接口。
///
/// 中文说明：市场状态以标准化事件表示，策略只能读取事件，不能追加或改写事件。
pub trait MarketSnapshotReader {
    /// 返回策略本次评估允许读取的标准化事件窗口。
    fn market_events(&self) -> &[NormalizedEvent];
}

/// 策略只读快照集合。
pub trait StrategySnapshotReader: PortfolioSnapshotReader + MarketSnapshotReader {}

impl<T> StrategySnapshotReader for T where T: PortfolioSnapshotReader + MarketSnapshotReader {}

/// 拥有型只读快照，可用于测试、回放和运行时装配。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadOnlySnapshot {
    portfolio_state: PortfolioState,
    market_events: Vec<NormalizedEvent>,
}

impl ReadOnlySnapshot {
    /// 创建只读快照。
    pub fn new(portfolio_state: PortfolioState, market_events: Vec<NormalizedEvent>) -> Self {
        Self {
            portfolio_state,
            market_events,
        }
    }
}

impl PortfolioSnapshotReader for ReadOnlySnapshot {
    fn portfolio_state(&self) -> &PortfolioState {
        &self.portfolio_state
    }
}

impl MarketSnapshotReader for ReadOnlySnapshot {
    fn market_events(&self) -> &[NormalizedEvent] {
        &self.market_events
    }
}

/// 场所能力只读读取接口。
///
/// 中文说明：能力读取只说明场所“声明支持什么”，不提供任何提交、撤销、
/// 转账或签名方法。
pub trait VenueCapabilityReader {
    /// 返回全部可读能力描述。
    fn venue_capabilities(&self) -> &[VenueCapabilityDescriptor];

    /// 按场所 ID 查询能力描述。
    fn venue_capability(&self, venue_id: &str) -> Option<&VenueCapabilityDescriptor> {
        self.venue_capabilities()
            .iter()
            .find(|capability| capability.venue_id.as_str() == venue_id)
    }

    /// 判断场所是否声明某个市场能力。
    fn has_market_capability(&self, venue_id: &str, capability: &MarketCapability) -> bool {
        self.venue_capability(venue_id).is_some_and(|descriptor| {
            descriptor
                .market_capabilities
                .iter()
                .any(|item| item.as_str() == capability.as_str())
        })
    }

    /// 判断场所是否声明某个只读数据面。
    fn has_data_surface(&self, venue_id: &str, surface: &DataSurface) -> bool {
        self.venue_capability(venue_id).is_some_and(|descriptor| {
            descriptor
                .data_surfaces
                .iter()
                .any(|item| item.as_str() == surface.as_str())
        })
    }
}

/// 拥有型场所能力快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VenueCapabilitySnapshot {
    capabilities: Vec<VenueCapabilityDescriptor>,
}

impl VenueCapabilitySnapshot {
    /// 创建能力快照，并拒绝重复场所 ID。
    pub fn new(capabilities: Vec<VenueCapabilityDescriptor>) -> StrategyApiResult<Self> {
        let mut seen = BTreeSet::new();
        for capability in &capabilities {
            let venue_id = capability.venue_id.as_str();
            if !seen.insert(venue_id.to_owned()) {
                return Err(StrategyApiError::InvalidInput {
                    field: "venue_capabilities",
                    message: format!("duplicate venue capability for `{venue_id}`"),
                });
            }
        }
        Ok(Self { capabilities })
    }
}

impl VenueCapabilityReader for VenueCapabilitySnapshot {
    fn venue_capabilities(&self) -> &[VenueCapabilityDescriptor] {
        &self.capabilities
    }
}

/// 策略可见的只读配置接口。
///
/// 中文说明：该接口只暴露配置版本、配置哈希、执行模式和熔断禁用状态；
/// 不暴露签名策略对象、密钥、执行适配器或运行时装配。
pub trait StrategyConfigReader {
    /// 配置版本。
    fn config_version(&self) -> &str;

    /// 配置规范哈希。
    fn config_hash(&self) -> &str;

    /// 当前执行权限模式，仅供策略决定是否提出候选或拒绝。
    fn execution_mode(&self) -> ExecutionMode;

    /// 是否存在全局或执行熔断。
    fn kill_switch_triggered(&self) -> bool;

    /// 指定策略是否被配置禁用。
    fn strategy_disabled(&self, strategy_id: &str) -> bool;

    /// 指定场所是否被配置禁用。
    fn venue_disabled(&self, venue_id: &str) -> bool;
}

/// 策略可见的只读配置快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategyConfigSnapshot {
    config_version: String,
    config_hash: String,
    execution_mode: ExecutionMode,
    kill_switch_triggered: bool,
    disabled_strategies: Vec<String>,
    disabled_venues: Vec<String>,
}

impl StrategyConfigSnapshot {
    /// 创建策略只读配置快照。
    pub fn new(
        config_version: impl Into<String>,
        config_hash: impl Into<String>,
        execution_mode: ExecutionMode,
        kill_switch_triggered: bool,
        disabled_strategies: Vec<String>,
        disabled_venues: Vec<String>,
    ) -> StrategyApiResult<Self> {
        let config_version = config_version.into();
        let config_hash = config_hash.into();
        ensure_non_empty("config_version", &config_version)?;
        ensure_non_empty("config_hash", &config_hash)?;
        Ok(Self {
            config_version,
            config_hash,
            execution_mode,
            kill_switch_triggered,
            disabled_strategies,
            disabled_venues,
        })
    }

    /// 从完整配置生成策略只读视图。
    pub fn from_config(config: &arb_config::ArbConfig) -> StrategyApiResult<Self> {
        Self::new(
            config.version().as_str(),
            config.hash().as_str(),
            config.execution().mode(),
            config.kill_switch().is_triggered(),
            config.kill_switch().strategies().to_vec(),
            config.kill_switch().venues().to_vec(),
        )
    }

    /// 从完整 YAML 配置生成策略只读视图。
    ///
    /// 中文说明：该函数只返回策略可见字段，不把签名策略、密钥或执行装配暴露给策略。
    pub fn from_yaml_str(input: &str) -> StrategyApiResult<Self> {
        let config = arb_config::ArbConfig::from_yaml_str(input)?;
        Self::from_config(&config)
    }
}

impl StrategyConfigReader for StrategyConfigSnapshot {
    fn config_version(&self) -> &str {
        &self.config_version
    }

    fn config_hash(&self) -> &str {
        &self.config_hash
    }

    fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }

    fn kill_switch_triggered(&self) -> bool {
        self.kill_switch_triggered
    }

    fn strategy_disabled(&self, strategy_id: &str) -> bool {
        self.disabled_strategies
            .iter()
            .any(|disabled| disabled == strategy_id)
    }

    fn venue_disabled(&self, venue_id: &str) -> bool {
        self.disabled_venues
            .iter()
            .any(|disabled| disabled == venue_id)
    }
}

/// 策略评估只读上下文。
pub trait StrategyReadContext {
    /// 状态和市场事件快照。
    fn snapshot(&self) -> &dyn StrategySnapshotReader;

    /// 场所能力读取器。
    fn capabilities(&self) -> &dyn VenueCapabilityReader;

    /// 策略可见配置。
    fn config(&self) -> &dyn StrategyConfigReader;

    /// 策略时间源。
    fn time(&self) -> &dyn StrategyTimeSource;
}

/// 借用型策略上下文，用于把外部只读对象装配给策略。
#[derive(Clone, Copy)]
pub struct ReadOnlyStrategyContext<'a> {
    snapshot: &'a dyn StrategySnapshotReader,
    capabilities: &'a dyn VenueCapabilityReader,
    config: &'a dyn StrategyConfigReader,
    time: &'a dyn StrategyTimeSource,
}

impl<'a> ReadOnlyStrategyContext<'a> {
    /// 创建借用型只读上下文。
    pub fn new(
        snapshot: &'a dyn StrategySnapshotReader,
        capabilities: &'a dyn VenueCapabilityReader,
        config: &'a dyn StrategyConfigReader,
        time: &'a dyn StrategyTimeSource,
    ) -> Self {
        Self {
            snapshot,
            capabilities,
            config,
            time,
        }
    }
}

impl StrategyReadContext for ReadOnlyStrategyContext<'_> {
    fn snapshot(&self) -> &dyn StrategySnapshotReader {
        self.snapshot
    }

    fn capabilities(&self) -> &dyn VenueCapabilityReader {
        self.capabilities
    }

    fn config(&self) -> &dyn StrategyConfigReader {
        self.config
    }

    fn time(&self) -> &dyn StrategyTimeSource {
        self.time
    }
}

/// 拥有型策略输入，适合离线测试和回放。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategyInput {
    snapshot: ReadOnlySnapshot,
    capabilities: VenueCapabilitySnapshot,
    config: StrategyConfigSnapshot,
    time: FixedTimeSource,
}

impl StrategyInput {
    /// 创建拥有型策略输入。
    pub fn new(
        snapshot: ReadOnlySnapshot,
        capabilities: VenueCapabilitySnapshot,
        config: StrategyConfigSnapshot,
        time: FixedTimeSource,
    ) -> Self {
        Self {
            snapshot,
            capabilities,
            config,
            time,
        }
    }
}

impl StrategyReadContext for StrategyInput {
    fn snapshot(&self) -> &dyn StrategySnapshotReader {
        &self.snapshot
    }

    fn capabilities(&self) -> &dyn VenueCapabilityReader {
        &self.capabilities
    }

    fn config(&self) -> &dyn StrategyConfigReader {
        &self.config
    }

    fn time(&self) -> &dyn StrategyTimeSource {
        &self.time
    }
}

/// 策略元数据。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StrategyMetadata {
    strategy_id: String,
    strategy_version: String,
    code_version: String,
}

impl StrategyMetadata {
    /// 创建策略元数据。
    pub fn new(
        strategy_id: impl Into<String>,
        strategy_version: impl Into<String>,
        code_version: impl Into<String>,
    ) -> StrategyApiResult<Self> {
        let strategy_id = strategy_id.into();
        let strategy_version = strategy_version.into();
        let code_version = code_version.into();
        ensure_non_empty("strategy_id", &strategy_id)?;
        ensure_non_empty("strategy_version", &strategy_version)?;
        ensure_non_empty("code_version", &code_version)?;
        Ok(Self {
            strategy_id,
            strategy_version,
            code_version,
        })
    }

    /// 策略 ID。
    pub fn strategy_id(&self) -> &str {
        &self.strategy_id
    }

    /// 策略版本。
    pub fn strategy_version(&self) -> &str {
        &self.strategy_version
    }

    /// 代码版本。
    pub fn code_version(&self) -> &str {
        &self.code_version
    }
}

/// 策略拒绝原因。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StrategyRejectReason {
    NoCandidate,
    DataStale,
    VenueCapabilityMissing,
    VenueUnhealthy,
    ConfigDisabled,
    KillSwitchTriggered,
    MissingData,
    UnknownState,
    Other(String),
}

impl StrategyRejectReason {
    /// 返回机器可读原因码。
    pub fn as_str(&self) -> &str {
        match self {
            Self::NoCandidate => "NO_CANDIDATE",
            Self::DataStale => "DATA_STALE",
            Self::VenueCapabilityMissing => "VENUE_CAPABILITY_MISSING",
            Self::VenueUnhealthy => "VENUE_UNHEALTHY",
            Self::ConfigDisabled => "CONFIG_DISABLED",
            Self::KillSwitchTriggered => "KILL_SWITCH_TRIGGERED",
            Self::MissingData => "MISSING_DATA",
            Self::UnknownState => "UNKNOWN_STATE",
            Self::Other(reason) => reason,
        }
    }
}

/// 策略拒绝事件。
///
/// 中文说明：拒绝是策略的正常只读输出，不代表执行失败，也不能触发账户变化。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategyRejection {
    strategy_id: String,
    strategy_version: String,
    rejected_at: UtcTimestamp,
    reason: StrategyRejectReason,
    detail: Option<String>,
    input_event_refs: Vec<String>,
    portfolio_state_ref: String,
}

impl StrategyRejection {
    /// 创建策略拒绝输出。
    pub fn new(
        metadata: &StrategyMetadata,
        rejected_at: UtcTimestamp,
        reason: StrategyRejectReason,
        detail: Option<String>,
        input_event_refs: Vec<String>,
        portfolio_state_ref: impl Into<String>,
    ) -> StrategyApiResult<Self> {
        let portfolio_state_ref = portfolio_state_ref.into();
        ensure_non_empty("portfolio_state_ref", &portfolio_state_ref)?;
        Ok(Self {
            strategy_id: metadata.strategy_id.clone(),
            strategy_version: metadata.strategy_version.clone(),
            rejected_at,
            reason,
            detail,
            input_event_refs,
            portfolio_state_ref,
        })
    }

    /// 策略 ID。
    pub fn strategy_id(&self) -> &str {
        &self.strategy_id
    }

    /// 策略版本。
    pub fn strategy_version(&self) -> &str {
        &self.strategy_version
    }

    /// 拒绝时间。
    pub fn rejected_at(&self) -> UtcTimestamp {
        self.rejected_at
    }

    /// 拒绝原因。
    pub fn reason(&self) -> &StrategyRejectReason {
        &self.reason
    }

    /// 补充说明。
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// 输入事件引用。
    pub fn input_event_refs(&self) -> &[String] {
        &self.input_event_refs
    }

    /// 当前组合状态引用。
    pub fn portfolio_state_ref(&self) -> &str {
        &self.portfolio_state_ref
    }
}

/// 策略诊断信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategyDiagnostic {
    code: String,
    detail: String,
    observed_at: UtcTimestamp,
}

impl StrategyDiagnostic {
    /// 创建策略诊断信息。
    pub fn new(
        code: impl Into<String>,
        detail: impl Into<String>,
        observed_at: UtcTimestamp,
    ) -> StrategyApiResult<Self> {
        let code = code.into();
        let detail = detail.into();
        ensure_non_empty("diagnostic_code", &code)?;
        ensure_non_empty("diagnostic_detail", &detail)?;
        Ok(Self {
            code,
            detail,
            observed_at,
        })
    }

    /// 诊断代码。
    pub fn code(&self) -> &str {
        &self.code
    }

    /// 诊断说明。
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// 观察时间。
    pub fn observed_at(&self) -> UtcTimestamp {
        self.observed_at
    }
}

/// 策略输出结果：候选转换或拒绝。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StrategyOutcome {
    Candidate(Box<CandidatePortfolioTransition>),
    Rejected(StrategyRejection),
}

/// 候选转换输出读取接口。
pub trait CandidateTransitionOutput {
    /// 返回候选转换；拒绝时返回 `None`。
    fn candidate(&self) -> Option<&CandidatePortfolioTransition>;

    /// 返回拒绝原因；有候选时返回 `None`。
    fn rejection(&self) -> Option<&StrategyRejection>;

    /// 返回诊断信息。
    fn diagnostics(&self) -> &[StrategyDiagnostic];
}

/// 策略评估输出。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategyEvaluation {
    outcome: StrategyOutcome,
    diagnostics: Vec<StrategyDiagnostic>,
}

impl StrategyEvaluation {
    /// 输出候选组合转换。
    pub fn candidate(candidate: CandidatePortfolioTransition) -> Self {
        Self {
            outcome: StrategyOutcome::Candidate(Box::new(candidate)),
            diagnostics: Vec::new(),
        }
    }

    /// 输出策略拒绝。
    pub fn rejected(rejection: StrategyRejection) -> Self {
        Self {
            outcome: StrategyOutcome::Rejected(rejection),
            diagnostics: Vec::new(),
        }
    }

    /// 附加诊断信息。
    pub fn with_diagnostic(mut self, diagnostic: StrategyDiagnostic) -> Self {
        self.diagnostics.push(diagnostic);
        self
    }

    /// 返回输出结果。
    pub fn outcome(&self) -> &StrategyOutcome {
        &self.outcome
    }
}

impl CandidateTransitionOutput for StrategyEvaluation {
    fn candidate(&self) -> Option<&CandidatePortfolioTransition> {
        match &self.outcome {
            StrategyOutcome::Candidate(candidate) => Some(candidate.as_ref()),
            StrategyOutcome::Rejected(_) => None,
        }
    }

    fn rejection(&self) -> Option<&StrategyRejection> {
        match &self.outcome {
            StrategyOutcome::Candidate(_) => None,
            StrategyOutcome::Rejected(rejection) => Some(rejection),
        }
    }

    fn diagnostics(&self) -> &[StrategyDiagnostic] {
        &self.diagnostics
    }
}

/// 策略 trait。
///
/// 中文说明：策略实现只能读取 `StrategyReadContext`，并返回候选转换或拒绝；
/// trait 中没有执行、签名、转账、账本写入或运行时装配入口。
pub trait Strategy {
    /// 策略元数据。
    fn metadata(&self) -> &StrategyMetadata;

    /// 执行一次只读策略评估。
    fn evaluate(&self, context: &dyn StrategyReadContext) -> StrategyApiResult<StrategyEvaluation>;
}

/// 从严格 JSON 解析候选转换合同。
pub fn candidate_from_json_strict(input: &str) -> StrategyApiResult<CandidatePortfolioTransition> {
    Ok(from_json_strict::<CandidatePortfolioTransition>(input)?)
}

/// 从严格 JSON 解析组合状态快照合同。
pub fn portfolio_state_from_json_strict(input: &str) -> StrategyApiResult<PortfolioState> {
    Ok(from_json_strict::<PortfolioState>(input)?)
}

/// 从严格 JSON 解析标准化事件合同。
pub fn normalized_event_from_json_strict(input: &str) -> StrategyApiResult<NormalizedEvent> {
    Ok(from_json_strict::<NormalizedEvent>(input)?)
}

/// 从严格 JSON 解析场所能力合同。
pub fn venue_capability_from_json_strict(
    input: &str,
) -> StrategyApiResult<VenueCapabilityDescriptor> {
    Ok(from_json_strict::<VenueCapabilityDescriptor>(input)?)
}

/// 将候选转换输出为规范 JSON。
pub fn candidate_to_canonical_json(candidate: &CandidatePortfolioTransition) -> String {
    to_canonical_json(candidate)
}

/// 验证候选转换能按合同规范 JSON 往返。
pub fn validate_candidate_contract(
    candidate: &CandidatePortfolioTransition,
) -> StrategyApiResult<()> {
    let canonical = candidate_to_canonical_json(candidate);
    let reparsed = candidate_from_json_strict(&canonical)?;
    let reparsed_canonical = candidate_to_canonical_json(&reparsed);
    if canonical != reparsed_canonical {
        return Err(StrategyApiError::OutputMismatch {
            field: "candidate_canonical_json",
            expected: canonical,
            actual: reparsed_canonical,
        });
    }
    Ok(())
}

/// 验证候选转换和策略上下文、策略元数据一致。
pub fn validate_candidate_for_context(
    context: &dyn StrategyReadContext,
    metadata: &StrategyMetadata,
    candidate: &CandidatePortfolioTransition,
) -> StrategyApiResult<()> {
    validate_candidate_contract(candidate)?;
    expect_equal(
        "candidate.strategy_id",
        metadata.strategy_id(),
        candidate.strategy_id.as_str(),
    )?;
    expect_equal(
        "candidate.strategy_version",
        metadata.strategy_version(),
        candidate.strategy_version.as_str(),
    )?;
    expect_equal(
        "candidate.code_version",
        metadata.code_version(),
        candidate.code_version.as_str(),
    )?;
    expect_equal(
        "candidate.config_version",
        context.config().config_version(),
        candidate.config_version.as_str(),
    )?;
    expect_equal(
        "candidate.current_portfolio_state_ref",
        context.snapshot().portfolio_state_id(),
        candidate.current_portfolio_state_ref.as_str(),
    )?;
    Ok(())
}

fn ensure_non_empty(field: &'static str, value: &str) -> StrategyApiResult<()> {
    if value.is_empty() {
        return Err(StrategyApiError::InvalidInput {
            field,
            message: "value cannot be empty".to_owned(),
        });
    }
    Ok(())
}

fn expect_equal(field: &'static str, expected: &str, actual: &str) -> StrategyApiResult<()> {
    if expected != actual {
        return Err(StrategyApiError::OutputMismatch {
            field,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_config::ArbConfig;

    const PORTFOLIO_JSON: &str =
        include_str!("../../../fixtures/schema/valid/portfolio_state.valid.json");
    const CANDIDATE_JSON: &str =
        include_str!("../../../fixtures/schema/valid/candidate_portfolio_transition.valid.json");
    const VENUE_CAPABILITY_JSON: &str =
        include_str!("../../../fixtures/schema/valid/venue_capability.valid.json");
    const CONFIG_YAML: &str = include_str!("../../../fixtures/replay/minimal_smoke/config.yaml");

    #[test]
    fn fixed_time_source_is_stable_and_strict_utc() {
        let time = FixedTimeSource::from_rfc3339_z("2026-01-01T00:00:02Z")
            .expect("fixed time should parse");

        assert_eq!(time.now_rfc3339_z(), "2026-01-01T00:00:02Z");
        assert_eq!(time.now(), time.now());
        assert!(FixedTimeSource::from_rfc3339_z("2026-01-01T08:00:02+08:00").is_err());
    }

    #[test]
    fn read_only_context_exposes_snapshot_capabilities_config_and_time() {
        let input = strategy_input();

        assert_eq!(input.snapshot().portfolio_state_id(), "state:01");
        assert_eq!(input.snapshot().source_event_refs()[0].as_str(), "event:01");
        assert!(input
            .capabilities()
            .has_market_capability("venue:SIM", &MarketCapability::ProvidesSpotMarkets));
        assert!(input
            .capabilities()
            .has_data_surface("venue:SIM", &DataSurface::RestPolling));
        assert_eq!(input.config().config_version(), "cfg:demo-1");
        assert_eq!(input.config().execution_mode(), ExecutionMode::ReadOnly);
        assert!(!input.config().kill_switch_triggered());
        assert_eq!(input.time().now_rfc3339_z(), "2026-01-01T00:00:02Z");
    }

    #[test]
    fn strategy_can_only_return_candidate_or_rejection() {
        let input = strategy_input();
        let strategy = FixtureStrategy {
            metadata: StrategyMetadata::new("strat:demo", "1.0.0", "code:demo-1")
                .expect("metadata"),
            candidate: candidate_from_json_strict(CANDIDATE_JSON).expect("candidate"),
        };

        let evaluation = strategy.evaluate(&input).expect("strategy should evaluate");

        let candidate = evaluation.candidate().expect("candidate output");
        assert_eq!(candidate.transition_id.as_str(), "trans:01");
        assert!(evaluation.rejection().is_none());
        assert!(candidate_to_canonical_json(candidate).contains("\"transition_id\":\"trans:01\""));
    }

    #[test]
    fn strategy_rejection_is_machine_readable() {
        let input = strategy_input();
        let metadata =
            StrategyMetadata::new("strat:demo", "1.0.0", "code:demo-1").expect("metadata");
        let rejection = StrategyRejection::new(
            &metadata,
            input.time().now(),
            StrategyRejectReason::VenueCapabilityMissing,
            Some("venue lacks required readonly surface".to_owned()),
            vec!["event:01".to_owned()],
            input.snapshot().portfolio_state_id(),
        )
        .expect("rejection");
        let diagnostic = StrategyDiagnostic::new(
            "CAPABILITY_CHECK",
            "checked fixture venue capabilities",
            input.time().now(),
        )
        .expect("diagnostic");

        let evaluation = StrategyEvaluation::rejected(rejection).with_diagnostic(diagnostic);

        assert!(evaluation.candidate().is_none());
        assert_eq!(
            evaluation.rejection().expect("rejection").reason().as_str(),
            "VENUE_CAPABILITY_MISSING"
        );
        assert_eq!(evaluation.diagnostics()[0].code(), "CAPABILITY_CHECK");
    }

    #[test]
    fn candidate_validation_rejects_metadata_mismatch() {
        let input = strategy_input();
        let candidate = candidate_from_json_strict(CANDIDATE_JSON).expect("candidate");
        let wrong_metadata =
            StrategyMetadata::new("strat:other", "1.0.0", "code:demo-1").expect("metadata");

        let error =
            validate_candidate_for_context(&input, &wrong_metadata, &candidate).expect_err("error");

        assert!(matches!(
            error,
            StrategyApiError::OutputMismatch {
                field: "candidate.strategy_id",
                ..
            }
        ));
    }

    #[test]
    fn strategy_api_manifest_has_no_forbidden_mutable_dependencies() {
        let manifest = include_str!("../Cargo.toml");
        for forbidden in [
            "arb-execution",
            "arb-venue-exec",
            "arb-signing",
            "arb-ledger",
            "arb-runtime",
        ] {
            assert!(
                !manifest.contains(forbidden),
                "strategy API must not depend on {forbidden}"
            );
        }
    }

    struct FixtureStrategy {
        metadata: StrategyMetadata,
        candidate: CandidatePortfolioTransition,
    }

    impl Strategy for FixtureStrategy {
        fn metadata(&self) -> &StrategyMetadata {
            &self.metadata
        }

        fn evaluate(
            &self,
            context: &dyn StrategyReadContext,
        ) -> StrategyApiResult<StrategyEvaluation> {
            validate_candidate_for_context(context, self.metadata(), &self.candidate)?;
            Ok(StrategyEvaluation::candidate(self.candidate.clone()))
        }
    }

    fn strategy_input() -> StrategyInput {
        let portfolio =
            from_json_strict::<PortfolioState>(PORTFOLIO_JSON).expect("portfolio state");
        let capability = from_json_strict::<VenueCapabilityDescriptor>(VENUE_CAPABILITY_JSON)
            .expect("venue capability");
        let config = ArbConfig::from_yaml_str(CONFIG_YAML).expect("config");
        let strategy_config = StrategyConfigSnapshot::new(
            "cfg:demo-1",
            config.hash().as_str(),
            config.execution().mode(),
            config.kill_switch().is_triggered(),
            config.kill_switch().strategies().to_vec(),
            config.kill_switch().venues().to_vec(),
        )
        .expect("strategy config snapshot");
        StrategyInput::new(
            ReadOnlySnapshot::new(portfolio, Vec::new()),
            VenueCapabilitySnapshot::new(vec![capability]).expect("capability snapshot"),
            strategy_config,
            FixedTimeSource::from_rfc3339_z("2026-01-01T00:00:02Z").expect("fixed time"),
        )
    }
}
