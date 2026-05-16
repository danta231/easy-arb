//! `arb-contracts` 合同类型和严格 JSON 边界。
//!
//! 中文说明：本 crate 只负责 schema 对应类型、严格反序列化、规范序列化
//! 和合同字段校验；不做风控判断、不生成执行计划、不写账本，也不访问外部网络。

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

/// 合同层统一返回类型。
pub type ContractResult<T> = Result<T, ContractError>;

/// 合同解析和校验错误。
///
/// 中文说明：错误必须指向具体 JSON 路径，调用方不能把未知或非法输入当作成功。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    Json { path: String, message: String },
    ExpectedObject { path: String },
    MissingField { path: String, field: String },
    UnknownField { path: String, field: String },
    WrongType { path: String, expected: String },
    InvalidValue { path: String, message: String },
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { path, message } => write!(f, "{path}: invalid JSON: {message}"),
            Self::ExpectedObject { path } => write!(f, "{path}: expected JSON object"),
            Self::MissingField { path, field } => {
                write!(f, "{path}: missing required field `{field}`")
            }
            Self::UnknownField { path, field } => write!(f, "{path}: unknown field `{field}`"),
            Self::WrongType { path, expected } => write!(f, "{path}: expected {expected}"),
            Self::InvalidValue { path, message } => write!(f, "{path}: invalid value: {message}"),
        }
    }
}

impl Error for ContractError {}

/// JSON number stored without lossy conversion.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct JsonNumber(String);

impl JsonNumber {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Minimal JSON value used for open payloads and canonical serialization.
///
/// 中文说明：开放字段仅用于 schema 明确允许的 payload/constraints，不用于绕过
/// 顶层合同字段校验。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    fn write_canonical(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(value) => out.push_str(value.as_str()),
            Self::String(value) => write_json_string(value, out),
            Self::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    value.write_canonical(out);
                }
                out.push(']');
            }
            Self::Object(values) => {
                out.push('{');
                for (index, (key, value)) in values.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_json_string(key, out);
                    out.push(':');
                    value.write_canonical(out);
                }
                out.push('}');
            }
        }
    }

    pub fn to_canonical_json(&self) -> String {
        let mut out = String::new();
        self.write_canonical(&mut out);
        out
    }
}

/// 可转换为规范 JSON 的合同类型。
pub trait CanonicalJson {
    fn to_json_value(&self) -> JsonValue;
}

/// 可从严格 JSON 解码的顶层合同类型。
pub trait ContractJson: Sized + CanonicalJson + ValidateContract {
    const SCHEMA_NAME: &'static str;

    fn from_json_value(value: &JsonValue) -> ContractResult<Self>;
}

/// 合同级校验接口。
pub trait ValidateContract {
    fn validate(&self) -> ContractResult<()>;
}

/// 带 `schema_version` 的核心合同。
pub trait HasSchemaVersion {
    fn schema_version(&self) -> &Version;
}

/// 严格反序列化入口。
pub fn from_json_strict<T: ContractJson>(input: &str) -> ContractResult<T> {
    let value = JsonParser::new(input).parse()?;
    let contract = T::from_json_value(&value)?;
    contract.validate()?;
    Ok(contract)
}

/// 规范 JSON 序列化入口。
///
/// 中文说明：对象字段按字典序输出，decimal 字符串原样保留，避免字段顺序和
/// 二进制浮点导致的回放差异。
pub fn to_canonical_json<T: CanonicalJson>(value: &T) -> String {
    value.to_json_value().to_canonical_json()
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Identifier(String);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Version(String);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DateTime(String);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DurationMs(u64);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DecimalString(String);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NonNegativeDecimalString(String);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PositiveDecimalString(String);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReasonCode(String);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HashString(String);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SignatureRef(String);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Confidence(JsonNumber);

macro_rules! string_newtype {
    ($name:ident) => {
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl CanonicalJson for $name {
            fn to_json_value(&self) -> JsonValue {
                JsonValue::String(self.0.clone())
            }
        }
    };
}

string_newtype!(Identifier);
string_newtype!(Version);
string_newtype!(DateTime);
string_newtype!(DecimalString);
string_newtype!(NonNegativeDecimalString);
string_newtype!(PositiveDecimalString);
string_newtype!(ReasonCode);
string_newtype!(HashString);
string_newtype!(SignatureRef);

impl DurationMs {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl CanonicalJson for DurationMs {
    fn to_json_value(&self) -> JsonValue {
        JsonValue::Number(JsonNumber(self.0.to_string()))
    }
}

impl Confidence {
    pub fn as_json_number(&self) -> &str {
        self.0.as_str()
    }
}

impl CanonicalJson for Confidence {
    fn to_json_value(&self) -> JsonValue {
        JsonValue::Number(self.0.clone())
    }
}

impl CanonicalJson for bool {
    fn to_json_value(&self) -> JsonValue {
        JsonValue::Bool(*self)
    }
}

impl CanonicalJson for u64 {
    fn to_json_value(&self) -> JsonValue {
        JsonValue::Number(JsonNumber(self.to_string()))
    }
}

impl CanonicalJson for String {
    fn to_json_value(&self) -> JsonValue {
        JsonValue::String(self.clone())
    }
}

impl CanonicalJson for JsonValue {
    fn to_json_value(&self) -> JsonValue {
        self.clone()
    }
}

impl<T: CanonicalJson> CanonicalJson for Vec<T> {
    fn to_json_value(&self) -> JsonValue {
        JsonValue::Array(self.iter().map(CanonicalJson::to_json_value).collect())
    }
}

impl CanonicalJson for BTreeMap<String, JsonValue> {
    fn to_json_value(&self) -> JsonValue {
        JsonValue::Object(self.clone())
    }
}

macro_rules! define_string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }
        }

        impl EnumContract for $name {
            fn parse_enum(value: &str, path: &str) -> ContractResult<Self> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    other => Err(invalid(path, format!("unsupported enum value `{other}`"))),
                }
            }
        }

        impl CanonicalJson for $name {
            fn to_json_value(&self) -> JsonValue {
                JsonValue::String(self.as_str().to_owned())
            }
        }
    };
}

trait EnumContract: Sized {
    fn parse_enum(value: &str, path: &str) -> ContractResult<Self>;
}

macro_rules! impl_contract {
    ($type:ident, $schema_name:literal) => {
        impl ContractJson for $type {
            const SCHEMA_NAME: &'static str = $schema_name;

            fn from_json_value(value: &JsonValue) -> ContractResult<Self> {
                Self::parse(value, "$")
            }
        }

        impl ValidateContract for $type {
            fn validate(&self) -> ContractResult<()> {
                Self::from_json_value(&self.to_json_value()).map(|_| ())
            }
        }
    };
}

macro_rules! impl_schema_version {
    ($type:ident) => {
        impl HasSchemaVersion for $type {
            fn schema_version(&self) -> &Version {
                &self.schema_version
            }
        }
    };
}

define_string_enum!(AssetFlowDirection { In => "In", Out => "Out" });
define_string_enum!(SettlementKind {
    OffChainLedger => "OffChainLedger",
    OnChainFinality => "OnChainFinality",
    BridgeFinality => "BridgeFinality",
    InternalTreasury => "InternalTreasury",
});
define_string_enum!(InstrumentKind {
    SpotPair => "SpotPair",
    MarginSpotPair => "MarginSpotPair",
    PerpetualSwap => "PerpetualSwap",
    DatedFuture => "DatedFuture",
    OptionContract => "Option",
    AmmPool => "AMMPool",
    OrderBookMarket => "OrderBookMarket",
    FundingStream => "FundingStream",
    BorrowRateMarket => "BorrowRateMarket",
    LendingRateMarket => "LendingRateMarket",
    RfqRoute => "RFQRoute",
    AggregatorRoute => "AggregatorRoute",
    BridgeRoute => "BridgeRoute",
    OraclePriceFeed => "OraclePriceFeed",
    SyntheticIndex => "SyntheticIndex",
    StructuredPosition => "StructuredPosition",
});
define_string_enum!(ScheduleKind {
    FixedInterval => "FixedInterval",
    VenueDefined => "VenueDefined",
    OnDemand => "OnDemand",
});
define_string_enum!(LiquidityKind {
    OrderBook => "OrderBook",
    Amm => "AMM",
    Rfq => "RFQ",
    Aggregator => "Aggregator",
    RateMarket => "RateMarket",
    OracleOnly => "OracleOnly",
});
define_string_enum!(OrderType {
    Market => "Market",
    Limit => "Limit",
    PostOnly => "PostOnly",
    ReduceOnly => "ReduceOnly",
    Rfq => "RFQ",
    OnChainSwap => "OnChainSwap",
});
define_string_enum!(KnownFailureMode {
    PartialFill => "PartialFill",
    LateFill => "LateFill",
    VenueOutage => "VenueOutage",
    StaleQuote => "StaleQuote",
    OracleDivergence => "OracleDivergence",
    SettlementDelay => "SettlementDelay",
    ChainReorg => "ChainReorg",
    ManualInterventionRequired => "ManualInterventionRequired",
});
define_string_enum!(NormalizedEventType {
    RawMarketDataEvent => "RawMarketDataEvent",
    NormalizedMarketDataEvent => "NormalizedMarketDataEvent",
    VenueHealthEvent => "VenueHealthEvent",
    StrategySignalEvent => "StrategySignalEvent",
    CandidateOpportunityEvent => "CandidateOpportunityEvent",
    StrategyRejectEvent => "StrategyRejectEvent",
    RiskDecisionEvent => "RiskDecisionEvent",
    CapitalReservationEvent => "CapitalReservationEvent",
    ApprovalEvent => "ApprovalEvent",
    ExecutionPlanEvent => "ExecutionPlanEvent",
    ExecutionDispatchEvent => "ExecutionDispatchEvent",
    ExecutionReportEvent => "ExecutionReportEvent",
    FillEvent => "FillEvent",
    TransferEvent => "TransferEvent",
    FundingEvent => "FundingEvent",
    FeeEvent => "FeeEvent",
    BalanceSnapshotEvent => "BalanceSnapshotEvent",
    PositionSnapshotEvent => "PositionSnapshotEvent",
    LedgerEntryEvent => "LedgerEntryEvent",
    ReconciliationEvent => "ReconciliationEvent",
    IncidentEvent => "IncidentEvent",
    AuditEvent => "AuditEvent",
});
define_string_enum!(CapitalReservationState {
    Requested => "Requested",
    Reserved => "Reserved",
    ConvertedToExecution => "ConvertedToExecution",
    Released => "Released",
    Expired => "Expired",
    ReconciledMismatch => "ReconciledMismatch",
});
define_string_enum!(OpenOrderStatus {
    Open => "Open",
    PartiallyFilled => "PartiallyFilled",
    PendingCancel => "PendingCancel",
    Unknown => "Unknown",
});
define_string_enum!(PendingTransferStatus {
    Pending => "Pending",
    Submitted => "Submitted",
    Confirming => "Confirming",
    Settled => "Settled",
    Failed => "Failed",
    Unknown => "Unknown",
});
define_string_enum!(HoldingPeriodKind {
    Instant => "Instant",
    Seconds => "Seconds",
    Minutes => "Minutes",
    Hours => "Hours",
    Days => "Days",
    UntilFundingTimestamp => "UntilFundingTimestamp",
    UntilExpiry => "UntilExpiry",
    UntilBasisConvergence => "UntilBasisConvergence",
    UntilManualExit => "UntilManualExit",
});
define_string_enum!(TransitionLegType {
    Trade => "Trade",
    Transfer => "Transfer",
    Borrow => "Borrow",
    Lend => "Lend",
    Repay => "Repay",
    Bridge => "Bridge",
    Hedge => "Hedge",
    FundingCapture => "FundingCapture",
    Observation => "Observation",
});
define_string_enum!(TransitionSide {
    Buy => "Buy",
    Sell => "Sell",
    Long => "Long",
    Short => "Short",
    Deposit => "Deposit",
    Withdraw => "Withdraw",
    Borrow => "Borrow",
    Lend => "Lend",
    Repay => "Repay",
    Receive => "Receive",
    Pay => "Pay",
    None => "None",
});
define_string_enum!(FailureMode {
    NoOpFailure => "NoOpFailure",
    RetryableFailure => "RetryableFailure",
    PartialFill => "PartialFill",
    LateFill => "LateFill",
    DuplicateEvent => "DuplicateEvent",
    UnknownState => "UnknownState",
    StuckTransaction => "StuckTransaction",
    VenueOutage => "VenueOutage",
    RateLimit => "RateLimit",
    ManualInterventionRequired => "ManualInterventionRequired",
});
define_string_enum!(RiskFlag {
    StaleMarketData => "StaleMarketData",
    InsufficientLiquidity => "InsufficientLiquidity",
    HighGas => "HighGas",
    HighSlippage => "HighSlippage",
    InventoryLimitExceeded => "InventoryLimitExceeded",
    MarginInsufficient => "MarginInsufficient",
    LiquidationTooClose => "LiquidationTooClose",
    FundingRateUnstable => "FundingRateUnstable",
    BasisWidening => "BasisWidening",
    VenueUnhealthy => "VenueUnhealthy",
    ChainCongested => "ChainCongested",
    ApiRateLimited => "ApiRateLimited",
    OneLegExecutionRisk => "OneLegExecutionRisk",
    OracleDivergence => "OracleDivergence",
    SettlementDelay => "SettlementDelay",
    CustodyRisk => "CustodyRisk",
    BridgeRisk => "BridgeRisk",
    ModelUncertainty => "ModelUncertainty",
    UnknownState => "UnknownState",
});
define_string_enum!(RiskDecisionKind {
    Approved => "Approved",
    ApprovedWithConstraints => "ApprovedWithConstraints",
    Rejected => "Rejected",
    RequiresManualApproval => "RequiresManualApproval",
    RequiresMoreData => "RequiresMoreData",
    SuspendedByCircuitBreaker => "SuspendedByCircuitBreaker",
});
define_string_enum!(RiskCheckType {
    DataFreshness => "DataFreshness",
    VenueHealth => "VenueHealth",
    RateLimitState => "RateLimitState",
    LiquiditySufficiency => "LiquiditySufficiency",
    SlippageBounds => "SlippageBounds",
    FeeAndGasInclusion => "FeeAndGasInclusion",
    InventoryBounds => "InventoryBounds",
    BalanceSufficiency => "BalanceSufficiency",
    CapitalReservationAvailability => "CapitalReservationAvailability",
    MarginSufficiency => "MarginSufficiency",
    LiquidationDistance => "LiquidationDistance",
    FundingRateUncertainty => "FundingRateUncertainty",
    BasisWideningRisk => "BasisWideningRisk",
    OneLegExecutionRisk => "OneLegExecutionRisk",
    ChainCongestion => "ChainCongestion",
    NonceConflict => "NonceConflict",
    OpenOrderConflict => "OpenOrderConflict",
    StrategyExposureLimit => "StrategyExposureLimit",
    DailyLossLimit => "DailyLossLimit",
    DrawdownLimit => "DrawdownLimit",
    CorrelationConcentrationLimit => "CorrelationConcentrationLimit",
    ReconciliationCompleteness => "ReconciliationCompleteness",
});
define_string_enum!(RiskCheckStatus {
    Pass => "Pass",
    Fail => "Fail",
    Warning => "Warning",
    Unknown => "Unknown",
    NotApplicable => "NotApplicable",
});
define_string_enum!(RiskSeverity {
    Info => "Info",
    Warn => "Warn",
    Block => "Block",
    Critical => "Critical",
});
define_string_enum!(RiskConstraintType {
    MaxNotional => "MaxNotional",
    MaxSlippage => "MaxSlippage",
    MaxFee => "MaxFee",
    MinLiquidity => "MinLiquidity",
    ExecutionModeLimit => "ExecutionModeLimit",
    RequiresManualApproval => "RequiresManualApproval",
    ReduceHoldingPeriod => "ReduceHoldingPeriod",
    ForceHedge => "ForceHedge",
    DisableVenue => "DisableVenue",
    DisableStrategy => "DisableStrategy",
});
define_string_enum!(ExecutionMode {
    ReadOnly => "ReadOnly",
    Simulated => "Simulated",
    ManualApproval => "ManualApproval",
    GuardedLive => "GuardedLive",
    AutonomousLive => "AutonomousLive",
});
define_string_enum!(ExecutionActionType {
    RecordOnly => "RecordOnly",
    SimulatedFill => "SimulatedFill",
    PlaceOrder => "PlaceOrder",
    CancelOrder => "CancelOrder",
    Transfer => "Transfer",
    SignTransaction => "SignTransaction",
    SubmitTransaction => "SubmitTransaction",
    Borrow => "Borrow",
    Lend => "Lend",
    Repay => "Repay",
    AdjustMargin => "AdjustMargin",
    Hedge => "Hedge",
    ManualApprovalGate => "ManualApprovalGate",
});
define_string_enum!(ExecutionOrderType {
    Market => "Market",
    Limit => "Limit",
    PostOnly => "PostOnly",
});
define_string_enum!(ExecutionLegState {
    Prepared => "Prepared",
    WaitingDependency => "WaitingDependency",
    Ready => "Ready",
    Dispatched => "Dispatched",
    Acknowledged => "Acknowledged",
    PartiallyFilled => "PartiallyFilled",
    Filled => "Filled",
    CancelRequested => "CancelRequested",
    Cancelled => "Cancelled",
    Failed => "Failed",
    Unknown => "Unknown",
    Compensating => "Compensating",
    Compensated => "Compensated",
});
define_string_enum!(DependencyCondition {
    OnSuccess => "OnSuccess",
    OnAcknowledged => "OnAcknowledged",
    OnFilled => "OnFilled",
    OnPartialFill => "OnPartialFill",
    OnFailure => "OnFailure",
    ManualRelease => "ManualRelease",
});
define_string_enum!(CancelDefaultAction {
    None => "None",
    CancelOpenOrders => "CancelOpenOrders",
    CancelAndHedgeResidual => "CancelAndHedgeResidual",
    ManualIntervention => "ManualIntervention",
});
define_string_enum!(HedgeResidualAction {
    IgnoreBelowThreshold => "IgnoreBelowThreshold",
    HedgeImmediately => "HedgeImmediately",
    HedgeAfterTimeout => "HedgeAfterTimeout",
    ManualIntervention => "ManualIntervention",
});
define_string_enum!(PartialFillAction {
    ContinueIfWithinBounds => "ContinueIfWithinBounds",
    CancelRemainder => "CancelRemainder",
    HedgeFilledPortion => "HedgeFilledPortion",
    ManualIntervention => "ManualIntervention",
});
define_string_enum!(UnknownStateAction {
    HaltAndIncident => "HaltAndIncident",
    ReconcileThenResume => "ReconcileThenResume",
    ManualIntervention => "ManualIntervention",
});
define_string_enum!(ExecutionReportStatus {
    NotDispatched => "NotDispatched",
    Simulated => "Simulated",
    Succeeded => "Succeeded",
    PartiallySucceeded => "PartiallySucceeded",
    Failed => "Failed",
    UnknownState => "UnknownState",
    ManualInterventionRequired => "ManualInterventionRequired",
});
define_string_enum!(LegReportStatus {
    Skipped => "Skipped",
    Simulated => "Simulated",
    Dispatched => "Dispatched",
    Acknowledged => "Acknowledged",
    Filled => "Filled",
    PartiallyFilled => "PartiallyFilled",
    Cancelled => "Cancelled",
    Failed => "Failed",
    Unknown => "Unknown",
});
define_string_enum!(ExecutionFailureSeverity {
    Info => "Info",
    Warn => "Warn",
    RiskCritical => "RiskCritical",
});
define_string_enum!(ReconciliationStatus {
    NotStarted => "NotStarted",
    Matched => "Matched",
    Mismatch => "Mismatch",
    Pending => "Pending",
    Unknown => "Unknown",
});
define_string_enum!(FillSide {
    Buy => "Buy",
    Sell => "Sell",
    Long => "Long",
    Short => "Short",
    Receive => "Receive",
    Pay => "Pay",
});
define_string_enum!(LedgerNamespace {
    Live => "Live",
    Simulation => "Simulation",
    Backtest => "Backtest",
    Adjustment => "Adjustment",
});
define_string_enum!(LedgerEntryType {
    TradeFill => "TradeFill",
    Fee => "Fee",
    Funding => "Funding",
    Transfer => "Transfer",
    CapitalReservation => "CapitalReservation",
    CapitalRelease => "CapitalRelease",
    Borrow => "Borrow",
    Lend => "Lend",
    Repay => "Repay",
    RealizedPnl => "RealizedPnl",
    UnrealizedPnlSnapshot => "UnrealizedPnlSnapshot",
    ReconciliationAdjustment => "ReconciliationAdjustment",
    ManualAdjustment => "ManualAdjustment",
});
define_string_enum!(LedgerDirection {
    Debit => "Debit",
    Credit => "Credit",
});
define_string_enum!(MarketCapability {
    ProvidesSpotMarkets => "ProvidesSpotMarkets",
    ProvidesMarginMarkets => "ProvidesMarginMarkets",
    ProvidesPerpetuals => "ProvidesPerpetuals",
    ProvidesDatedFutures => "ProvidesDatedFutures",
    ProvidesOptions => "ProvidesOptions",
    ProvidesAmmPools => "ProvidesAMMPools",
    ProvidesOrderBookMarkets => "ProvidesOrderBookMarkets",
    ProvidesRfq => "ProvidesRFQ",
    ProvidesAggregatorRoutes => "ProvidesAggregatorRoutes",
    ProvidesFundingRates => "ProvidesFundingRates",
    ProvidesBorrowLend => "ProvidesBorrowLend",
    ProvidesOraclePrices => "ProvidesOraclePrices",
    ProvidesBridgeRoutes => "ProvidesBridgeRoutes",
});
define_string_enum!(ExecutionCapability {
    SupportsMarketOrders => "SupportsMarketOrders",
    SupportsLimitOrders => "SupportsLimitOrders",
    SupportsPostOnly => "SupportsPostOnly",
    SupportsReduceOnly => "SupportsReduceOnly",
    SupportsCancelReplace => "SupportsCancelReplace",
    SupportsBatchOrders => "SupportsBatchOrders",
    SupportsAtomicSwap => "SupportsAtomicSwap",
    SupportsOnChainTransaction => "SupportsOnChainTransaction",
    SupportsOffChainMatching => "SupportsOffChainMatching",
    SupportsPrivateOrderStream => "SupportsPrivateOrderStream",
    SupportsManualApprovalOnly => "SupportsManualApprovalOnly",
});
define_string_enum!(AuthMode {
    PublicOnly => "PublicOnly",
    ApiKeyAuth => "APIKeyAuth",
    WalletSignatureAuth => "WalletSignatureAuth",
    SessionTokenAuth => "SessionTokenAuth",
    MultiSigApproval => "MultiSigApproval",
    HardwareSignerRequired => "HardwareSignerRequired",
});
define_string_enum!(DataSurface {
    RestPolling => "RESTPolling",
    WebSocketStreaming => "WebSocketStreaming",
    HistoricalCandles => "HistoricalCandles",
    HistoricalTrades => "HistoricalTrades",
    FundingHistory => "FundingHistory",
    PositionHistory => "PositionHistory",
    BalanceHistory => "BalanceHistory",
    RateLimitHeaders => "RateLimitHeaders",
    SequenceNumbers => "SequenceNumbers",
});
define_string_enum!(SettlementMode {
    OnChainSettlement => "OnChainSettlement",
    OffChainCustody => "OffChainCustody",
    SelfCustodyWallet => "SelfCustodyWallet",
    SubaccountModel => "SubaccountModel",
    PortfolioMargin => "PortfolioMargin",
    IsolatedMargin => "IsolatedMargin",
    CrossMargin => "CrossMargin",
    ExternalCustodian => "ExternalCustodian",
    RequiresBridgeSettlement => "RequiresBridgeSettlement",
});
define_string_enum!(RateLimitUnit {
    Request => "Request",
    Weight => "Weight",
    Order => "Order",
    Connection => "Connection",
});
define_string_enum!(IncidentSeverity {
    Sev0 => "SEV0",
    Sev1 => "SEV1",
    Sev2 => "SEV2",
    Sev3 => "SEV3",
});
define_string_enum!(IncidentStatus {
    Open => "Open",
    Mitigating => "Mitigating",
    Resolved => "Resolved",
    PostmortemRequired => "PostmortemRequired",
    Closed => "Closed",
});
define_string_enum!(IncidentActionType {
    KillSwitchActivated => "KillSwitchActivated",
    TradingPaused => "TradingPaused",
    VenueDisabled => "VenueDisabled",
    StrategyDisabled => "StrategyDisabled",
    ManualReview => "ManualReview",
    ReconciliationStarted => "ReconciliationStarted",
    NotificationSent => "NotificationSent",
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetFlow {
    pub asset_id: Identifier,
    pub direction: AssetFlowDirection,
    pub amount: NonNegativeDecimalString,
    pub account_id: Option<Identifier>,
    pub custody_location_id: Option<Identifier>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MoneyUsd {
    pub amount_usd: DecimalString,
    pub valuation_source: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub schema_version: Version,
    pub asset_id: Identifier,
    pub canonical_symbol: String,
    pub chain_identifiers: Option<Vec<ChainIdentifier>>,
    pub decimal_precision: u64,
    pub risk_group: Identifier,
    pub settlement_properties: SettlementProperties,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChainIdentifier {
    pub chain_id: Identifier,
    pub address: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettlementProperties {
    pub settlement_kind: SettlementKind,
    pub can_be_collateral: bool,
    pub min_confirmations: Option<u64>,
    pub finality_risk_note: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Instrument {
    pub schema_version: Version,
    pub instrument_id: Identifier,
    pub venue_id: Identifier,
    pub kind: InstrumentKind,
    pub base_asset_id: Option<Identifier>,
    pub quote_asset_id: Option<Identifier>,
    pub settlement_asset_id: Identifier,
    pub margin_asset_id: Option<Identifier>,
    pub expiry_time: Option<DateTime>,
    pub funding_schedule: Option<Schedule>,
    pub contract_multiplier: Option<PositiveDecimalString>,
    pub tick_size: Option<PositiveDecimalString>,
    pub lot_size: Option<PositiveDecimalString>,
    pub pricing_properties: PricingProperties,
    pub liquidity_model: Option<LiquidityModel>,
    pub execution_model: ExecutionModel,
    pub failure_model: FailureModel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Schedule {
    pub kind: Option<ScheduleKind>,
    pub interval_ms: Option<DurationMs>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PricingProperties {
    pub price_source: Option<String>,
    pub mark_price_required: Option<bool>,
    pub index_price_required: Option<bool>,
    pub oracle_price_required: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquidityModel {
    pub kind: Option<LiquidityKind>,
    pub depth_required: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionModel {
    pub supports_live_execution: bool,
    pub allowed_order_types: Option<Vec<OrderType>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailureModel {
    pub unknown_state_is_critical: bool,
    pub known_failure_modes: Option<Vec<KnownFailureMode>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortfolioState {
    pub schema_version: Version,
    pub portfolio_state_id: Identifier,
    pub as_of: DateTime,
    pub source_event_refs: Vec<Identifier>,
    pub balances: Vec<Balance>,
    pub positions: Vec<Position>,
    pub reservations: Vec<CapitalReservation>,
    pub open_orders: Vec<OpenOrder>,
    pub pending_transfers: Vec<PendingTransfer>,
    pub confidence: Confidence,
    pub missing_data_flags: Vec<ReasonCode>,
    pub state_hash: HashString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Balance {
    pub balance_id: Identifier,
    pub asset_id: Identifier,
    pub account_id: Identifier,
    pub custody_location_id: Identifier,
    pub free: NonNegativeDecimalString,
    pub locked: NonNegativeDecimalString,
    pub reserved: NonNegativeDecimalString,
    pub pending: NonNegativeDecimalString,
    pub borrowed: NonNegativeDecimalString,
    pub lent: NonNegativeDecimalString,
    pub unsettled: NonNegativeDecimalString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    pub position_id: Identifier,
    pub instrument_id: Identifier,
    pub account_id: Identifier,
    pub quantity: DecimalString,
    pub entry_price: Option<NonNegativeDecimalString>,
    pub mark_price: NonNegativeDecimalString,
    pub unrealized_pnl: DecimalString,
    pub liquidation_price: Option<NonNegativeDecimalString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapitalReservation {
    pub reservation_id: Identifier,
    pub state: CapitalReservationState,
    pub asset_id: Identifier,
    pub amount: NonNegativeDecimalString,
    pub reserved_for: Identifier,
    pub expires_at: DateTime,
    pub source_event_id: Option<Identifier>,
    pub ledger_entry_id: Option<Identifier>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenOrder {
    pub order_id: Identifier,
    pub venue_id: Identifier,
    pub instrument_id: Identifier,
    pub status: OpenOrderStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingTransfer {
    pub transfer_id: Identifier,
    pub asset_id: Identifier,
    pub amount: NonNegativeDecimalString,
    pub status: PendingTransferStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedEvent {
    pub event_id: Identifier,
    pub event_type: NormalizedEventType,
    pub event_version: Version,
    pub timestamp_event: DateTime,
    pub timestamp_ingested: DateTime,
    pub source: String,
    pub sequence: Option<u64>,
    pub source_sequence: Option<String>,
    pub correlation_id: Identifier,
    pub causation_id: Option<Option<Identifier>>,
    pub schema_version: Version,
    pub venue_id: Option<Option<Identifier>>,
    pub instrument_id: Option<Option<Identifier>>,
    pub strategy_id: Option<Option<Identifier>>,
    pub portfolio_state_ref: Option<Identifier>,
    pub payload: BTreeMap<String, JsonValue>,
    pub checksum: HashString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidatePortfolioTransition {
    pub schema_version: Version,
    pub transition_id: Identifier,
    pub strategy_id: Identifier,
    pub strategy_version: Version,
    pub code_version: Version,
    pub config_version: Version,
    pub created_at: DateTime,
    pub input_event_refs: Vec<Identifier>,
    pub current_portfolio_state_ref: Identifier,
    pub holding_period: HoldingPeriod,
    pub legs: Vec<TransitionLeg>,
    pub expected_post_state_delta: ExpectedPostStateDelta,
    pub expected_economics: ExpectedEconomics,
    pub required_capital: RequiredCapital,
    pub margin_impact: Option<ImpactBlock>,
    pub inventory_impact: Option<ImpactBlock>,
    pub liquidity_impact: Option<ImpactBlock>,
    pub funding_impact: Option<ImpactBlock>,
    pub borrow_lend_impact: Option<ImpactBlock>,
    pub failure_modes: Vec<FailureMode>,
    pub risk_flags: Vec<RiskFlag>,
    pub assumptions: Vec<Assumption>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoldingPeriod {
    pub kind: HoldingPeriodKind,
    pub duration_ms: Option<DurationMs>,
    pub until_timestamp: Option<DateTime>,
    pub exit_policy_ref: Option<Identifier>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionLeg {
    pub leg_id: Identifier,
    pub leg_type: TransitionLegType,
    pub venue_id: Option<Identifier>,
    pub instrument_id: Option<Identifier>,
    pub account_id: Option<Identifier>,
    pub side: Option<TransitionSide>,
    pub asset_flows: Vec<AssetFlow>,
    pub constraints: BTreeMap<String, JsonValue>,
    pub failure_modes: Vec<FailureMode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedPostStateDelta {
    pub asset_flows: Vec<AssetFlow>,
    pub position_deltas: Vec<PositionDelta>,
    pub reserve_deltas: Option<Vec<ReserveDelta>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PositionDelta {
    pub instrument_id: Identifier,
    pub account_id: Option<Identifier>,
    pub quantity_delta: DecimalString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReserveDelta {
    pub asset_id: Identifier,
    pub amount_delta: DecimalString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedEconomics {
    pub expected_profit_usd: DecimalString,
    pub expected_profit_bps: DecimalString,
    pub expected_apr: Option<DecimalString>,
    pub fee_estimate_usd: NonNegativeDecimalString,
    pub slippage_estimate_usd: NonNegativeDecimalString,
    pub gas_estimate_usd: Option<NonNegativeDecimalString>,
    pub confidence: Confidence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequiredCapital {
    pub asset_requirements: Vec<AssetFlow>,
    pub recovery_buffer_usd: NonNegativeDecimalString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImpactBlock {
    pub summary: Option<String>,
    pub impact_usd: Option<DecimalString>,
    pub confidence: Option<Confidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assumption {
    pub assumption_id: Identifier,
    pub statement: String,
    pub confidence: Confidence,
    pub source_event_refs: Option<Vec<Identifier>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RiskDecision {
    pub schema_version: Version,
    pub decision_id: Identifier,
    pub transition_id: Identifier,
    pub evaluated_at: DateTime,
    pub decision: RiskDecisionKind,
    pub policy_version: Version,
    pub policy_hash: HashString,
    pub policy_signature_ref: SignatureRef,
    pub input_state_ref: Identifier,
    pub checks: Vec<RiskCheckResult>,
    pub constraints: Vec<RiskConstraint>,
    pub reason_codes: Vec<ReasonCode>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RiskCheckResult {
    pub check_id: Identifier,
    pub check_type: RiskCheckType,
    pub status: RiskCheckStatus,
    pub severity: RiskSeverity,
    pub threshold: Option<MeasuredValue>,
    pub observed: Option<MeasuredValue>,
    pub reason_code: ReasonCode,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeasuredValue {
    pub decimal_value: Option<DecimalString>,
    pub string_value: Option<String>,
    pub unit: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RiskConstraint {
    pub constraint_id: Identifier,
    pub constraint_type: RiskConstraintType,
    pub field_path: String,
    pub limit: Option<MeasuredValue>,
    pub expires_at: Option<DateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionPlan {
    pub schema_version: Version,
    pub plan_id: Identifier,
    pub transition_id: Identifier,
    pub risk_decision_id: Identifier,
    pub created_at: DateTime,
    pub execution_mode: ExecutionMode,
    pub idempotency_key: Identifier,
    pub approval_event_id: Option<Identifier>,
    pub legs: Vec<ExecutionLeg>,
    pub dependency_graph: DependencyGraph,
    pub constraints: ExecutionConstraints,
    pub timeout_policy: TimeoutPolicy,
    pub cancel_policy: CancelPolicy,
    pub hedge_policy: HedgePolicy,
    pub partial_fill_policy: PartialFillPolicy,
    pub failure_policy: FailurePolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionLeg {
    pub plan_leg_id: Identifier,
    pub candidate_leg_id: Identifier,
    pub action_type: ExecutionActionType,
    pub venue_id: Option<Identifier>,
    pub instrument_id: Option<Identifier>,
    pub account_id: Identifier,
    pub client_order_id: Option<Identifier>,
    pub venue_symbol: Option<Identifier>,
    pub side: Option<TransitionSide>,
    pub order_type: Option<ExecutionOrderType>,
    pub quantity: Option<NonNegativeDecimalString>,
    pub limit_price: Option<NonNegativeDecimalString>,
    pub notional_usd: Option<NonNegativeDecimalString>,
    pub basis_leg_role: Option<Identifier>,
    pub idempotency_key: Identifier,
    pub depends_on: Option<Vec<Identifier>>,
    pub expected_asset_flows: Option<Vec<AssetFlow>>,
    pub state: ExecutionLegState,
    pub failure_semantics: FailureMode,
    pub dispatch_after: Option<DateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyGraph {
    pub edges: Vec<DependencyEdge>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyEdge {
    pub from_leg_id: Identifier,
    pub to_leg_id: Identifier,
    pub condition: DependencyCondition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionConstraints {
    pub max_notional_usd: Option<NonNegativeDecimalString>,
    pub slippage_limit_bps: Option<NonNegativeDecimalString>,
    pub max_fee_usd: Option<NonNegativeDecimalString>,
    pub min_receive_amount: Option<NonNegativeDecimalString>,
    pub requires_fresh_market_data_ms: Option<DurationMs>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeoutPolicy {
    pub plan_timeout_ms: DurationMs,
    pub leg_timeout_ms: DurationMs,
    pub unknown_state_after_ms: Option<DurationMs>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelPolicy {
    pub default_action: CancelDefaultAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HedgePolicy {
    pub residual_exposure_action: HedgeResidualAction,
    pub threshold_usd: Option<NonNegativeDecimalString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartialFillPolicy {
    pub action: PartialFillAction,
    pub max_unhedged_usd: Option<NonNegativeDecimalString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailurePolicy {
    pub unknown_state_action: UnknownStateAction,
    pub retry_limit: u64,
    pub retry_backoff_ms: Option<DurationMs>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionReport {
    pub schema_version: Version,
    pub report_id: Identifier,
    pub plan_id: Identifier,
    pub generated_at: DateTime,
    pub status: ExecutionReportStatus,
    pub leg_reports: Vec<LegReport>,
    pub fills: Vec<Fill>,
    pub failures: Vec<ExecutionFailure>,
    pub reconciliation_status: ReconciliationStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegReport {
    pub plan_leg_id: Identifier,
    pub status: LegReportStatus,
    pub source_event_refs: Option<Vec<Identifier>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionFailure {
    pub failure_id: Identifier,
    pub plan_leg_id: Option<Identifier>,
    pub failure_type: FailureMode,
    pub severity: ExecutionFailureSeverity,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fill {
    pub schema_version: Version,
    pub fill_id: Identifier,
    pub plan_id: Identifier,
    pub plan_leg_id: Identifier,
    pub venue_id: Identifier,
    pub instrument_id: Identifier,
    pub venue_order_id: Option<String>,
    pub client_order_id: Option<Identifier>,
    pub timestamp: DateTime,
    pub side: FillSide,
    pub price: NonNegativeDecimalString,
    pub quantity: NonNegativeDecimalString,
    pub fee: AssetFlow,
    pub source_event_id: Identifier,
    pub ledger_entry_id: Option<Identifier>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerEntry {
    pub ledger_entry_id: Identifier,
    pub journal_entry_id: Identifier,
    pub schema_version: Version,
    pub timestamp: DateTime,
    pub namespace: LedgerNamespace,
    pub entry_type: LedgerEntryType,
    pub source_event_id: Identifier,
    pub idempotency_key: Identifier,
    pub strategy_id: Option<Identifier>,
    pub opportunity_id: Option<Identifier>,
    pub execution_plan_id: Option<Identifier>,
    pub reversal_of: Option<Identifier>,
    pub adjustment_of: Option<Identifier>,
    pub adjustment_reason_code: Option<ReasonCode>,
    pub legs: Vec<LedgerLeg>,
    pub balance_assertion: BalanceAssertion,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerLeg {
    pub leg_id: Identifier,
    pub account_id: Identifier,
    pub custody_location_id: Option<Identifier>,
    pub asset_id: Identifier,
    pub direction: LedgerDirection,
    pub amount: NonNegativeDecimalString,
    pub valuation_usd: Option<DecimalString>,
    pub memo: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BalanceAssertion {
    pub balanced: bool,
    pub assertion_hash: HashString,
    pub checked_by: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VenueCapabilityDescriptor {
    pub schema_version: Version,
    pub venue_id: Identifier,
    pub venue_name: Option<String>,
    pub capability_version: Version,
    pub market_capabilities: Vec<MarketCapability>,
    pub execution_capabilities: Vec<ExecutionCapability>,
    pub auth_modes: Vec<AuthMode>,
    pub data_surfaces: Vec<DataSurface>,
    pub settlement_modes: Vec<SettlementMode>,
    pub permission_model: Option<PermissionModel>,
    pub rate_limit_model: RateLimitModel,
    pub health_model: HealthModel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionModel {
    pub can_read_public_data: Option<bool>,
    pub can_read_private_data: Option<bool>,
    pub can_trade: Option<bool>,
    pub can_withdraw: Option<bool>,
    pub requires_ip_binding: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitModel {
    pub unit: RateLimitUnit,
    pub limit: u64,
    pub window_ms: DurationMs,
    pub source: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthModel {
    pub freshness_threshold_ms: DurationMs,
    pub disconnect_threshold: u64,
    pub unknown_state_is_critical: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Incident {
    pub schema_version: Version,
    pub incident_id: Identifier,
    pub severity: IncidentSeverity,
    pub status: IncidentStatus,
    pub opened_at: DateTime,
    pub closed_at: Option<DateTime>,
    pub trigger: ReasonCode,
    pub source_event_refs: Vec<Identifier>,
    pub impacted: ImpactedScope,
    pub automatic_actions: Vec<IncidentAction>,
    pub manual_actions: Vec<IncidentAction>,
    pub root_cause: Option<String>,
    pub corrective_action: Option<String>,
    pub prevention_action: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImpactedScope {
    pub venue_ids: Option<Vec<Identifier>>,
    pub strategy_ids: Option<Vec<Identifier>>,
    pub capital_at_risk_usd: Option<NonNegativeDecimalString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncidentAction {
    pub action_id: Identifier,
    pub action_type: IncidentActionType,
    pub timestamp: DateTime,
    pub detail: Option<String>,
}

impl CanonicalJson for AssetFlow {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("asset_id", &self.asset_id),
            req("direction", &self.direction),
            req("amount", &self.amount),
            opt("account_id", &self.account_id),
            opt("custody_location_id", &self.custody_location_id),
        ])
    }
}

impl AssetFlow {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            asset_id: object.required("asset_id", parse_identifier)?,
            direction: object.required("direction", parse_enum::<AssetFlowDirection>)?,
            amount: object.required("amount", parse_non_negative_decimal)?,
            account_id: object.optional("account_id", parse_identifier)?,
            custody_location_id: object.optional("custody_location_id", parse_identifier)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for MoneyUsd {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("amount_usd", &self.amount_usd),
            opt("valuation_source", &self.valuation_source),
        ])
    }
}

impl MoneyUsd {
    pub fn from_json_value(value: &JsonValue) -> ContractResult<Self> {
        Self::parse(value, "$")
    }

    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            amount_usd: object.required("amount_usd", parse_decimal)?,
            valuation_source: object.optional("valuation_source", |value, path| {
                parse_bounded_string(value, path, 0, 128)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl_contract!(Asset, "asset");
impl_schema_version!(Asset);

impl Asset {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let decimal_precision = object.required("decimal_precision", |value, path| {
            let value = parse_u64(value, path)?;
            if value > 38 {
                return Err(invalid(path, "decimal_precision must be <= 38"));
            }
            Ok(value)
        })?;
        let parsed = Self {
            schema_version: object.required("schema_version", parse_version)?,
            asset_id: object.required("asset_id", parse_identifier)?,
            canonical_symbol: object.required("canonical_symbol", |value, path| {
                parse_bounded_string(value, path, 1, 32)
            })?,
            chain_identifiers: object
                .optional_array("chain_identifiers", ChainIdentifier::parse)?,
            decimal_precision,
            risk_group: object.required("risk_group", parse_identifier)?,
            settlement_properties: object
                .required("settlement_properties", SettlementProperties::parse)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for Asset {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("schema_version", &self.schema_version),
            req("asset_id", &self.asset_id),
            req("canonical_symbol", &self.canonical_symbol),
            opt("chain_identifiers", &self.chain_identifiers),
            req("decimal_precision", &self.decimal_precision),
            req("risk_group", &self.risk_group),
            req("settlement_properties", &self.settlement_properties),
        ])
    }
}

impl ChainIdentifier {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            chain_id: object.required("chain_id", parse_identifier)?,
            address: object.required("address", |value, path| {
                parse_bounded_string(value, path, 1, 128)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ChainIdentifier {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("chain_id", &self.chain_id),
            req("address", &self.address),
        ])
    }
}

impl SettlementProperties {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            settlement_kind: object.required("settlement_kind", parse_enum::<SettlementKind>)?,
            can_be_collateral: object.required("can_be_collateral", parse_bool)?,
            min_confirmations: object.optional("min_confirmations", parse_u64)?,
            finality_risk_note: object.optional("finality_risk_note", |value, path| {
                parse_bounded_string(value, path, 0, 512)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for SettlementProperties {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("settlement_kind", &self.settlement_kind),
            req("can_be_collateral", &self.can_be_collateral),
            opt("min_confirmations", &self.min_confirmations),
            opt("finality_risk_note", &self.finality_risk_note),
        ])
    }
}

impl_contract!(Instrument, "instrument");
impl_schema_version!(Instrument);

impl Instrument {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            schema_version: object.required("schema_version", parse_version)?,
            instrument_id: object.required("instrument_id", parse_identifier)?,
            venue_id: object.required("venue_id", parse_identifier)?,
            kind: object.required("kind", parse_enum::<InstrumentKind>)?,
            base_asset_id: object.optional("base_asset_id", parse_identifier)?,
            quote_asset_id: object.optional("quote_asset_id", parse_identifier)?,
            settlement_asset_id: object.required("settlement_asset_id", parse_identifier)?,
            margin_asset_id: object.optional("margin_asset_id", parse_identifier)?,
            expiry_time: object.optional("expiry_time", parse_datetime)?,
            funding_schedule: object.optional("funding_schedule", Schedule::parse)?,
            contract_multiplier: object.optional("contract_multiplier", parse_positive_decimal)?,
            tick_size: object.optional("tick_size", parse_positive_decimal)?,
            lot_size: object.optional("lot_size", parse_positive_decimal)?,
            pricing_properties: object.required("pricing_properties", PricingProperties::parse)?,
            liquidity_model: object.optional("liquidity_model", LiquidityModel::parse)?,
            execution_model: object.required("execution_model", ExecutionModel::parse)?,
            failure_model: object.required("failure_model", FailureModel::parse)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for Instrument {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("schema_version", &self.schema_version),
            req("instrument_id", &self.instrument_id),
            req("venue_id", &self.venue_id),
            req("kind", &self.kind),
            opt("base_asset_id", &self.base_asset_id),
            opt("quote_asset_id", &self.quote_asset_id),
            req("settlement_asset_id", &self.settlement_asset_id),
            opt("margin_asset_id", &self.margin_asset_id),
            opt("expiry_time", &self.expiry_time),
            opt("funding_schedule", &self.funding_schedule),
            opt("contract_multiplier", &self.contract_multiplier),
            opt("tick_size", &self.tick_size),
            opt("lot_size", &self.lot_size),
            req("pricing_properties", &self.pricing_properties),
            opt("liquidity_model", &self.liquidity_model),
            req("execution_model", &self.execution_model),
            req("failure_model", &self.failure_model),
        ])
    }
}

impl Schedule {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            kind: object.optional("kind", parse_enum::<ScheduleKind>)?,
            interval_ms: object.optional("interval_ms", parse_duration_ms)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for Schedule {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            opt("kind", &self.kind),
            opt("interval_ms", &self.interval_ms),
        ])
    }
}

impl PricingProperties {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            price_source: object.optional("price_source", |value, path| {
                parse_bounded_string(value, path, 0, 128)
            })?,
            mark_price_required: object.optional("mark_price_required", parse_bool)?,
            index_price_required: object.optional("index_price_required", parse_bool)?,
            oracle_price_required: object.optional("oracle_price_required", parse_bool)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for PricingProperties {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            opt("price_source", &self.price_source),
            opt("mark_price_required", &self.mark_price_required),
            opt("index_price_required", &self.index_price_required),
            opt("oracle_price_required", &self.oracle_price_required),
        ])
    }
}

impl LiquidityModel {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            kind: object.optional("kind", parse_enum::<LiquidityKind>)?,
            depth_required: object.optional("depth_required", parse_bool)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for LiquidityModel {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            opt("kind", &self.kind),
            opt("depth_required", &self.depth_required),
        ])
    }
}

impl ExecutionModel {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            supports_live_execution: object.required("supports_live_execution", parse_bool)?,
            allowed_order_types: object
                .optional_unique_enum_array("allowed_order_types", parse_enum::<OrderType>)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ExecutionModel {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("supports_live_execution", &self.supports_live_execution),
            opt("allowed_order_types", &self.allowed_order_types),
        ])
    }
}

impl FailureModel {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let unknown_state_is_critical =
            object.required("unknown_state_is_critical", parse_true_const)?;
        let parsed = Self {
            unknown_state_is_critical,
            known_failure_modes: object.optional_unique_enum_array(
                "known_failure_modes",
                parse_enum::<KnownFailureMode>,
            )?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for FailureModel {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("unknown_state_is_critical", &self.unknown_state_is_critical),
            opt("known_failure_modes", &self.known_failure_modes),
        ])
    }
}

impl_contract!(PortfolioState, "portfolio_state");
impl_schema_version!(PortfolioState);

impl PortfolioState {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            schema_version: object.required("schema_version", parse_version)?,
            portfolio_state_id: object.required("portfolio_state_id", parse_identifier)?,
            as_of: object.required("as_of", parse_datetime)?,
            source_event_refs: object.required_array("source_event_refs", parse_identifier)?,
            balances: object.required_array("balances", Balance::parse)?,
            positions: object.required_array("positions", Position::parse)?,
            reservations: object.required_array("reservations", CapitalReservation::parse)?,
            open_orders: object.required_array("open_orders", OpenOrder::parse)?,
            pending_transfers: object
                .required_array("pending_transfers", PendingTransfer::parse)?,
            confidence: object.required("confidence", parse_confidence)?,
            missing_data_flags: object
                .required_unique_array("missing_data_flags", parse_reason_code)?,
            state_hash: object.required("state_hash", parse_hash_string)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for PortfolioState {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("schema_version", &self.schema_version),
            req("portfolio_state_id", &self.portfolio_state_id),
            req("as_of", &self.as_of),
            req("source_event_refs", &self.source_event_refs),
            req("balances", &self.balances),
            req("positions", &self.positions),
            req("reservations", &self.reservations),
            req("open_orders", &self.open_orders),
            req("pending_transfers", &self.pending_transfers),
            req("confidence", &self.confidence),
            req("missing_data_flags", &self.missing_data_flags),
            req("state_hash", &self.state_hash),
        ])
    }
}

impl Balance {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            balance_id: object.required("balance_id", parse_identifier)?,
            asset_id: object.required("asset_id", parse_identifier)?,
            account_id: object.required("account_id", parse_identifier)?,
            custody_location_id: object.required("custody_location_id", parse_identifier)?,
            free: object.required("free", parse_non_negative_decimal)?,
            locked: object.required("locked", parse_non_negative_decimal)?,
            reserved: object.required("reserved", parse_non_negative_decimal)?,
            pending: object.required("pending", parse_non_negative_decimal)?,
            borrowed: object.required("borrowed", parse_non_negative_decimal)?,
            lent: object.required("lent", parse_non_negative_decimal)?,
            unsettled: object.required("unsettled", parse_non_negative_decimal)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for Balance {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("balance_id", &self.balance_id),
            req("asset_id", &self.asset_id),
            req("account_id", &self.account_id),
            req("custody_location_id", &self.custody_location_id),
            req("free", &self.free),
            req("locked", &self.locked),
            req("reserved", &self.reserved),
            req("pending", &self.pending),
            req("borrowed", &self.borrowed),
            req("lent", &self.lent),
            req("unsettled", &self.unsettled),
        ])
    }
}

impl Position {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            position_id: object.required("position_id", parse_identifier)?,
            instrument_id: object.required("instrument_id", parse_identifier)?,
            account_id: object.required("account_id", parse_identifier)?,
            quantity: object.required("quantity", parse_decimal)?,
            entry_price: object.optional("entry_price", parse_non_negative_decimal)?,
            mark_price: object.required("mark_price", parse_non_negative_decimal)?,
            unrealized_pnl: object.required("unrealized_pnl", parse_decimal)?,
            liquidation_price: object.optional("liquidation_price", parse_non_negative_decimal)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for Position {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("position_id", &self.position_id),
            req("instrument_id", &self.instrument_id),
            req("account_id", &self.account_id),
            req("quantity", &self.quantity),
            opt("entry_price", &self.entry_price),
            req("mark_price", &self.mark_price),
            req("unrealized_pnl", &self.unrealized_pnl),
            opt("liquidation_price", &self.liquidation_price),
        ])
    }
}

impl CapitalReservation {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            reservation_id: object.required("reservation_id", parse_identifier)?,
            state: object.required("state", parse_enum::<CapitalReservationState>)?,
            asset_id: object.required("asset_id", parse_identifier)?,
            amount: object.required("amount", parse_non_negative_decimal)?,
            reserved_for: object.required("reserved_for", parse_identifier)?,
            expires_at: object.required("expires_at", parse_datetime)?,
            source_event_id: object.optional("source_event_id", parse_identifier)?,
            ledger_entry_id: object.optional("ledger_entry_id", parse_identifier)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for CapitalReservation {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("reservation_id", &self.reservation_id),
            req("state", &self.state),
            req("asset_id", &self.asset_id),
            req("amount", &self.amount),
            req("reserved_for", &self.reserved_for),
            req("expires_at", &self.expires_at),
            opt("source_event_id", &self.source_event_id),
            opt("ledger_entry_id", &self.ledger_entry_id),
        ])
    }
}

impl OpenOrder {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            order_id: object.required("order_id", parse_identifier)?,
            venue_id: object.required("venue_id", parse_identifier)?,
            instrument_id: object.required("instrument_id", parse_identifier)?,
            status: object.required("status", parse_enum::<OpenOrderStatus>)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for OpenOrder {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("order_id", &self.order_id),
            req("venue_id", &self.venue_id),
            req("instrument_id", &self.instrument_id),
            req("status", &self.status),
        ])
    }
}

impl PendingTransfer {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            transfer_id: object.required("transfer_id", parse_identifier)?,
            asset_id: object.required("asset_id", parse_identifier)?,
            amount: object.required("amount", parse_non_negative_decimal)?,
            status: object.required("status", parse_enum::<PendingTransferStatus>)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for PendingTransfer {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("transfer_id", &self.transfer_id),
            req("asset_id", &self.asset_id),
            req("amount", &self.amount),
            req("status", &self.status),
        ])
    }
}

impl_contract!(NormalizedEvent, "normalized_event");
impl_schema_version!(NormalizedEvent);

impl NormalizedEvent {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            event_id: object.required("event_id", parse_identifier)?,
            event_type: object.required("event_type", parse_enum::<NormalizedEventType>)?,
            event_version: object.required("event_version", parse_version)?,
            timestamp_event: object.required("timestamp_event", parse_datetime)?,
            timestamp_ingested: object.required("timestamp_ingested", parse_datetime)?,
            source: object.required("source", |value, path| {
                parse_bounded_string(value, path, 1, 128)
            })?,
            sequence: object.optional("sequence", parse_u64)?,
            source_sequence: object.optional("source_sequence", |value, path| {
                parse_bounded_string(value, path, 0, 128)
            })?,
            correlation_id: object.required("correlation_id", parse_identifier)?,
            causation_id: object.optional_nullable("causation_id", parse_identifier)?,
            schema_version: object.required("schema_version", parse_version)?,
            venue_id: object.optional_nullable("venue_id", parse_identifier)?,
            instrument_id: object.optional_nullable("instrument_id", parse_identifier)?,
            strategy_id: object.optional_nullable("strategy_id", parse_identifier)?,
            portfolio_state_ref: object.optional("portfolio_state_ref", parse_identifier)?,
            payload: object.required("payload", parse_open_object)?,
            checksum: object.required("checksum", parse_hash_string)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for NormalizedEvent {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("event_id", &self.event_id),
            req("event_type", &self.event_type),
            req("event_version", &self.event_version),
            req("timestamp_event", &self.timestamp_event),
            req("timestamp_ingested", &self.timestamp_ingested),
            req("source", &self.source),
            opt("sequence", &self.sequence),
            opt("source_sequence", &self.source_sequence),
            req("correlation_id", &self.correlation_id),
            opt_nullable("causation_id", &self.causation_id),
            req("schema_version", &self.schema_version),
            opt_nullable("venue_id", &self.venue_id),
            opt_nullable("instrument_id", &self.instrument_id),
            opt_nullable("strategy_id", &self.strategy_id),
            opt("portfolio_state_ref", &self.portfolio_state_ref),
            req("payload", &self.payload),
            req("checksum", &self.checksum),
        ])
    }
}

impl_contract!(
    CandidatePortfolioTransition,
    "candidate_portfolio_transition"
);
impl_schema_version!(CandidatePortfolioTransition);

impl CandidatePortfolioTransition {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let input_event_refs = object.required_array("input_event_refs", parse_identifier)?;
        ensure_min_items(&input_event_refs, path, "input_event_refs", 1)?;
        let legs = object.required_array("legs", TransitionLeg::parse)?;
        ensure_min_items(&legs, path, "legs", 1)?;
        let failure_modes = object.required_array("failure_modes", parse_enum::<FailureMode>)?;
        ensure_min_items(&failure_modes, path, "failure_modes", 1)?;
        let assumptions = object.required_array("assumptions", Assumption::parse)?;
        ensure_min_items(&assumptions, path, "assumptions", 1)?;

        let parsed = Self {
            schema_version: object.required("schema_version", parse_version)?,
            transition_id: object.required("transition_id", parse_identifier)?,
            strategy_id: object.required("strategy_id", parse_identifier)?,
            strategy_version: object.required("strategy_version", parse_version)?,
            code_version: object.required("code_version", parse_version)?,
            config_version: object.required("config_version", parse_version)?,
            created_at: object.required("created_at", parse_datetime)?,
            input_event_refs,
            current_portfolio_state_ref: object
                .required("current_portfolio_state_ref", parse_identifier)?,
            holding_period: object.required("holding_period", HoldingPeriod::parse)?,
            legs,
            expected_post_state_delta: object
                .required("expected_post_state_delta", ExpectedPostStateDelta::parse)?,
            expected_economics: object.required("expected_economics", ExpectedEconomics::parse)?,
            required_capital: object.required("required_capital", RequiredCapital::parse)?,
            margin_impact: object.optional("margin_impact", ImpactBlock::parse)?,
            inventory_impact: object.optional("inventory_impact", ImpactBlock::parse)?,
            liquidity_impact: object.optional("liquidity_impact", ImpactBlock::parse)?,
            funding_impact: object.optional("funding_impact", ImpactBlock::parse)?,
            borrow_lend_impact: object.optional("borrow_lend_impact", ImpactBlock::parse)?,
            failure_modes,
            risk_flags: object.required_unique_array("risk_flags", parse_enum::<RiskFlag>)?,
            assumptions,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for CandidatePortfolioTransition {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("schema_version", &self.schema_version),
            req("transition_id", &self.transition_id),
            req("strategy_id", &self.strategy_id),
            req("strategy_version", &self.strategy_version),
            req("code_version", &self.code_version),
            req("config_version", &self.config_version),
            req("created_at", &self.created_at),
            req("input_event_refs", &self.input_event_refs),
            req(
                "current_portfolio_state_ref",
                &self.current_portfolio_state_ref,
            ),
            req("holding_period", &self.holding_period),
            req("legs", &self.legs),
            req("expected_post_state_delta", &self.expected_post_state_delta),
            req("expected_economics", &self.expected_economics),
            req("required_capital", &self.required_capital),
            opt("margin_impact", &self.margin_impact),
            opt("inventory_impact", &self.inventory_impact),
            opt("liquidity_impact", &self.liquidity_impact),
            opt("funding_impact", &self.funding_impact),
            opt("borrow_lend_impact", &self.borrow_lend_impact),
            req("failure_modes", &self.failure_modes),
            req("risk_flags", &self.risk_flags),
            req("assumptions", &self.assumptions),
        ])
    }
}

impl HoldingPeriod {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            kind: object.required("kind", parse_enum::<HoldingPeriodKind>)?,
            duration_ms: object.optional("duration_ms", parse_duration_ms)?,
            until_timestamp: object.optional("until_timestamp", parse_datetime)?,
            exit_policy_ref: object.optional("exit_policy_ref", parse_identifier)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for HoldingPeriod {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("kind", &self.kind),
            opt("duration_ms", &self.duration_ms),
            opt("until_timestamp", &self.until_timestamp),
            opt("exit_policy_ref", &self.exit_policy_ref),
        ])
    }
}

impl TransitionLeg {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let failure_modes = object.required_array("failure_modes", parse_enum::<FailureMode>)?;
        ensure_min_items(&failure_modes, path, "failure_modes", 1)?;
        let parsed = Self {
            leg_id: object.required("leg_id", parse_identifier)?,
            leg_type: object.required("leg_type", parse_enum::<TransitionLegType>)?,
            venue_id: object.optional("venue_id", parse_identifier)?,
            instrument_id: object.optional("instrument_id", parse_identifier)?,
            account_id: object.optional("account_id", parse_identifier)?,
            side: object.optional("side", parse_enum::<TransitionSide>)?,
            asset_flows: object.required_array("asset_flows", AssetFlow::parse)?,
            constraints: object.required("constraints", parse_scalar_object)?,
            failure_modes,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for TransitionLeg {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("leg_id", &self.leg_id),
            req("leg_type", &self.leg_type),
            opt("venue_id", &self.venue_id),
            opt("instrument_id", &self.instrument_id),
            opt("account_id", &self.account_id),
            opt("side", &self.side),
            req("asset_flows", &self.asset_flows),
            req("constraints", &self.constraints),
            req("failure_modes", &self.failure_modes),
        ])
    }
}

impl ExpectedPostStateDelta {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            asset_flows: object.required_array("asset_flows", AssetFlow::parse)?,
            position_deltas: object.required_array("position_deltas", PositionDelta::parse)?,
            reserve_deltas: object.optional_array("reserve_deltas", ReserveDelta::parse)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ExpectedPostStateDelta {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("asset_flows", &self.asset_flows),
            req("position_deltas", &self.position_deltas),
            opt("reserve_deltas", &self.reserve_deltas),
        ])
    }
}

impl PositionDelta {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            instrument_id: object.required("instrument_id", parse_identifier)?,
            account_id: object.optional("account_id", parse_identifier)?,
            quantity_delta: object.required("quantity_delta", parse_decimal)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for PositionDelta {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("instrument_id", &self.instrument_id),
            opt("account_id", &self.account_id),
            req("quantity_delta", &self.quantity_delta),
        ])
    }
}

impl ReserveDelta {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            asset_id: object.required("asset_id", parse_identifier)?,
            amount_delta: object.required("amount_delta", parse_decimal)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ReserveDelta {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("asset_id", &self.asset_id),
            req("amount_delta", &self.amount_delta),
        ])
    }
}

impl ExpectedEconomics {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            expected_profit_usd: object.required("expected_profit_usd", parse_decimal)?,
            expected_profit_bps: object.required("expected_profit_bps", parse_decimal)?,
            expected_apr: object.optional("expected_apr", parse_decimal)?,
            fee_estimate_usd: object.required("fee_estimate_usd", parse_non_negative_decimal)?,
            slippage_estimate_usd: object
                .required("slippage_estimate_usd", parse_non_negative_decimal)?,
            gas_estimate_usd: object.optional("gas_estimate_usd", parse_non_negative_decimal)?,
            confidence: object.required("confidence", parse_confidence)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ExpectedEconomics {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("expected_profit_usd", &self.expected_profit_usd),
            req("expected_profit_bps", &self.expected_profit_bps),
            opt("expected_apr", &self.expected_apr),
            req("fee_estimate_usd", &self.fee_estimate_usd),
            req("slippage_estimate_usd", &self.slippage_estimate_usd),
            opt("gas_estimate_usd", &self.gas_estimate_usd),
            req("confidence", &self.confidence),
        ])
    }
}

impl RequiredCapital {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            asset_requirements: object.required_array("asset_requirements", AssetFlow::parse)?,
            recovery_buffer_usd: object
                .required("recovery_buffer_usd", parse_non_negative_decimal)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for RequiredCapital {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("asset_requirements", &self.asset_requirements),
            req("recovery_buffer_usd", &self.recovery_buffer_usd),
        ])
    }
}

impl ImpactBlock {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            summary: object.optional("summary", |value, path| {
                parse_bounded_string(value, path, 0, 512)
            })?,
            impact_usd: object.optional("impact_usd", parse_decimal)?,
            confidence: object.optional("confidence", parse_confidence)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ImpactBlock {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            opt("summary", &self.summary),
            opt("impact_usd", &self.impact_usd),
            opt("confidence", &self.confidence),
        ])
    }
}

impl Assumption {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            assumption_id: object.required("assumption_id", parse_identifier)?,
            statement: object.required("statement", |value, path| {
                parse_bounded_string(value, path, 1, 1024)
            })?,
            confidence: object.required("confidence", parse_confidence)?,
            source_event_refs: object.optional_array("source_event_refs", parse_identifier)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for Assumption {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("assumption_id", &self.assumption_id),
            req("statement", &self.statement),
            req("confidence", &self.confidence),
            opt("source_event_refs", &self.source_event_refs),
        ])
    }
}

impl_contract!(RiskDecision, "risk_decision");
impl_schema_version!(RiskDecision);

impl RiskDecision {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let checks = object.required_array("checks", RiskCheckResult::parse)?;
        ensure_min_items(&checks, path, "checks", 1)?;
        let parsed = Self {
            schema_version: object.required("schema_version", parse_version)?,
            decision_id: object.required("decision_id", parse_identifier)?,
            transition_id: object.required("transition_id", parse_identifier)?,
            evaluated_at: object.required("evaluated_at", parse_datetime)?,
            decision: object.required("decision", parse_enum::<RiskDecisionKind>)?,
            policy_version: object.required("policy_version", parse_version)?,
            policy_hash: object.required("policy_hash", parse_hash_string)?,
            policy_signature_ref: object.required("policy_signature_ref", parse_signature_ref)?,
            input_state_ref: object.required("input_state_ref", parse_identifier)?,
            checks,
            constraints: object.required_array("constraints", RiskConstraint::parse)?,
            reason_codes: object.required_unique_array("reason_codes", parse_reason_code)?,
            detail: object.optional("detail", |value, path| {
                parse_bounded_string(value, path, 0, 2048)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for RiskDecision {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("schema_version", &self.schema_version),
            req("decision_id", &self.decision_id),
            req("transition_id", &self.transition_id),
            req("evaluated_at", &self.evaluated_at),
            req("decision", &self.decision),
            req("policy_version", &self.policy_version),
            req("policy_hash", &self.policy_hash),
            req("policy_signature_ref", &self.policy_signature_ref),
            req("input_state_ref", &self.input_state_ref),
            req("checks", &self.checks),
            req("constraints", &self.constraints),
            req("reason_codes", &self.reason_codes),
            opt("detail", &self.detail),
        ])
    }
}

impl RiskCheckResult {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            check_id: object.required("check_id", parse_identifier)?,
            check_type: object.required("check_type", parse_enum::<RiskCheckType>)?,
            status: object.required("status", parse_enum::<RiskCheckStatus>)?,
            severity: object.required("severity", parse_enum::<RiskSeverity>)?,
            threshold: object.optional("threshold", MeasuredValue::parse)?,
            observed: object.optional("observed", MeasuredValue::parse)?,
            reason_code: object.required("reason_code", parse_reason_code)?,
            detail: object.optional("detail", |value, path| {
                parse_bounded_string(value, path, 0, 1024)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for RiskCheckResult {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("check_id", &self.check_id),
            req("check_type", &self.check_type),
            req("status", &self.status),
            req("severity", &self.severity),
            opt("threshold", &self.threshold),
            opt("observed", &self.observed),
            req("reason_code", &self.reason_code),
            opt("detail", &self.detail),
        ])
    }
}

impl MeasuredValue {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            decimal_value: object.optional("decimal_value", parse_decimal)?,
            string_value: object.optional("string_value", |value, path| {
                parse_bounded_string(value, path, 0, 256)
            })?,
            unit: object.optional("unit", |value, path| {
                parse_bounded_string(value, path, 0, 64)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for MeasuredValue {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            opt("decimal_value", &self.decimal_value),
            opt("string_value", &self.string_value),
            opt("unit", &self.unit),
        ])
    }
}

impl RiskConstraint {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            constraint_id: object.required("constraint_id", parse_identifier)?,
            constraint_type: object
                .required("constraint_type", parse_enum::<RiskConstraintType>)?,
            field_path: object.required("field_path", |value, path| {
                parse_bounded_string(value, path, 1, 256)
            })?,
            limit: object.optional("limit", MeasuredValue::parse)?,
            expires_at: object.optional("expires_at", parse_datetime)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for RiskConstraint {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("constraint_id", &self.constraint_id),
            req("constraint_type", &self.constraint_type),
            req("field_path", &self.field_path),
            opt("limit", &self.limit),
            opt("expires_at", &self.expires_at),
        ])
    }
}

impl_contract!(ExecutionPlan, "execution_plan");
impl_schema_version!(ExecutionPlan);

impl ExecutionPlan {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let legs = object.required_array("legs", ExecutionLeg::parse)?;
        ensure_min_items(&legs, path, "legs", 1)?;
        let parsed = Self {
            schema_version: object.required("schema_version", parse_version)?,
            plan_id: object.required("plan_id", parse_identifier)?,
            transition_id: object.required("transition_id", parse_identifier)?,
            risk_decision_id: object.required("risk_decision_id", parse_identifier)?,
            created_at: object.required("created_at", parse_datetime)?,
            execution_mode: object.required("execution_mode", parse_enum::<ExecutionMode>)?,
            idempotency_key: object.required("idempotency_key", parse_identifier)?,
            approval_event_id: object.optional("approval_event_id", parse_identifier)?,
            legs,
            dependency_graph: object.required("dependency_graph", DependencyGraph::parse)?,
            constraints: object.required("constraints", ExecutionConstraints::parse)?,
            timeout_policy: object.required("timeout_policy", TimeoutPolicy::parse)?,
            cancel_policy: object.required("cancel_policy", CancelPolicy::parse)?,
            hedge_policy: object.required("hedge_policy", HedgePolicy::parse)?,
            partial_fill_policy: object
                .required("partial_fill_policy", PartialFillPolicy::parse)?,
            failure_policy: object.required("failure_policy", FailurePolicy::parse)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ExecutionPlan {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("schema_version", &self.schema_version),
            req("plan_id", &self.plan_id),
            req("transition_id", &self.transition_id),
            req("risk_decision_id", &self.risk_decision_id),
            req("created_at", &self.created_at),
            req("execution_mode", &self.execution_mode),
            req("idempotency_key", &self.idempotency_key),
            opt("approval_event_id", &self.approval_event_id),
            req("legs", &self.legs),
            req("dependency_graph", &self.dependency_graph),
            req("constraints", &self.constraints),
            req("timeout_policy", &self.timeout_policy),
            req("cancel_policy", &self.cancel_policy),
            req("hedge_policy", &self.hedge_policy),
            req("partial_fill_policy", &self.partial_fill_policy),
            req("failure_policy", &self.failure_policy),
        ])
    }
}

impl ExecutionLeg {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            plan_leg_id: object.required("plan_leg_id", parse_identifier)?,
            candidate_leg_id: object.required("candidate_leg_id", parse_identifier)?,
            action_type: object.required("action_type", parse_enum::<ExecutionActionType>)?,
            venue_id: object.optional("venue_id", parse_identifier)?,
            instrument_id: object.optional("instrument_id", parse_identifier)?,
            account_id: object.required("account_id", parse_identifier)?,
            client_order_id: object.optional("client_order_id", parse_identifier)?,
            venue_symbol: object.optional("venue_symbol", parse_identifier)?,
            side: object.optional("side", parse_enum::<TransitionSide>)?,
            order_type: object.optional("order_type", parse_enum::<ExecutionOrderType>)?,
            quantity: object.optional("quantity", parse_non_negative_decimal)?,
            limit_price: object.optional("limit_price", parse_non_negative_decimal)?,
            notional_usd: object.optional("notional_usd", parse_non_negative_decimal)?,
            basis_leg_role: object.optional("basis_leg_role", parse_identifier)?,
            idempotency_key: object.required("idempotency_key", parse_identifier)?,
            depends_on: object.optional_array("depends_on", parse_identifier)?,
            expected_asset_flows: object
                .optional_array("expected_asset_flows", AssetFlow::parse)?,
            state: object.required("state", parse_enum::<ExecutionLegState>)?,
            failure_semantics: object.required("failure_semantics", parse_enum::<FailureMode>)?,
            dispatch_after: object.optional("dispatch_after", parse_datetime)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ExecutionLeg {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("plan_leg_id", &self.plan_leg_id),
            req("candidate_leg_id", &self.candidate_leg_id),
            req("action_type", &self.action_type),
            opt("venue_id", &self.venue_id),
            opt("instrument_id", &self.instrument_id),
            req("account_id", &self.account_id),
            opt("client_order_id", &self.client_order_id),
            opt("venue_symbol", &self.venue_symbol),
            opt("side", &self.side),
            opt("order_type", &self.order_type),
            opt("quantity", &self.quantity),
            opt("limit_price", &self.limit_price),
            opt("notional_usd", &self.notional_usd),
            opt("basis_leg_role", &self.basis_leg_role),
            req("idempotency_key", &self.idempotency_key),
            opt("depends_on", &self.depends_on),
            opt("expected_asset_flows", &self.expected_asset_flows),
            req("state", &self.state),
            req("failure_semantics", &self.failure_semantics),
            opt("dispatch_after", &self.dispatch_after),
        ])
    }
}

impl DependencyGraph {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            edges: object.required_array("edges", DependencyEdge::parse)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for DependencyGraph {
    fn to_json_value(&self) -> JsonValue {
        object(vec![req("edges", &self.edges)])
    }
}

impl DependencyEdge {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            from_leg_id: object.required("from_leg_id", parse_identifier)?,
            to_leg_id: object.required("to_leg_id", parse_identifier)?,
            condition: object.required("condition", parse_enum::<DependencyCondition>)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for DependencyEdge {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("from_leg_id", &self.from_leg_id),
            req("to_leg_id", &self.to_leg_id),
            req("condition", &self.condition),
        ])
    }
}

impl ExecutionConstraints {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            max_notional_usd: object.optional("max_notional_usd", parse_non_negative_decimal)?,
            slippage_limit_bps: object
                .optional("slippage_limit_bps", parse_non_negative_decimal)?,
            max_fee_usd: object.optional("max_fee_usd", parse_non_negative_decimal)?,
            min_receive_amount: object
                .optional("min_receive_amount", parse_non_negative_decimal)?,
            requires_fresh_market_data_ms: object
                .optional("requires_fresh_market_data_ms", parse_duration_ms)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ExecutionConstraints {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            opt("max_notional_usd", &self.max_notional_usd),
            opt("slippage_limit_bps", &self.slippage_limit_bps),
            opt("max_fee_usd", &self.max_fee_usd),
            opt("min_receive_amount", &self.min_receive_amount),
            opt(
                "requires_fresh_market_data_ms",
                &self.requires_fresh_market_data_ms,
            ),
        ])
    }
}

impl TimeoutPolicy {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            plan_timeout_ms: object.required("plan_timeout_ms", parse_duration_ms)?,
            leg_timeout_ms: object.required("leg_timeout_ms", parse_duration_ms)?,
            unknown_state_after_ms: object.optional("unknown_state_after_ms", parse_duration_ms)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for TimeoutPolicy {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("plan_timeout_ms", &self.plan_timeout_ms),
            req("leg_timeout_ms", &self.leg_timeout_ms),
            opt("unknown_state_after_ms", &self.unknown_state_after_ms),
        ])
    }
}

impl CancelPolicy {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            default_action: object.required("default_action", parse_enum::<CancelDefaultAction>)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for CancelPolicy {
    fn to_json_value(&self) -> JsonValue {
        object(vec![req("default_action", &self.default_action)])
    }
}

impl HedgePolicy {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            residual_exposure_action: object.required(
                "residual_exposure_action",
                parse_enum::<HedgeResidualAction>,
            )?,
            threshold_usd: object.optional("threshold_usd", parse_non_negative_decimal)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for HedgePolicy {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("residual_exposure_action", &self.residual_exposure_action),
            opt("threshold_usd", &self.threshold_usd),
        ])
    }
}

impl PartialFillPolicy {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            action: object.required("action", parse_enum::<PartialFillAction>)?,
            max_unhedged_usd: object.optional("max_unhedged_usd", parse_non_negative_decimal)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for PartialFillPolicy {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("action", &self.action),
            opt("max_unhedged_usd", &self.max_unhedged_usd),
        ])
    }
}

impl FailurePolicy {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            unknown_state_action: object
                .required("unknown_state_action", parse_enum::<UnknownStateAction>)?,
            retry_limit: object.required("retry_limit", parse_u64)?,
            retry_backoff_ms: object.optional("retry_backoff_ms", parse_duration_ms)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for FailurePolicy {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("unknown_state_action", &self.unknown_state_action),
            req("retry_limit", &self.retry_limit),
            opt("retry_backoff_ms", &self.retry_backoff_ms),
        ])
    }
}

impl_contract!(ExecutionReport, "execution_report");
impl_schema_version!(ExecutionReport);

impl ExecutionReport {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            schema_version: object.required("schema_version", parse_version)?,
            report_id: object.required("report_id", parse_identifier)?,
            plan_id: object.required("plan_id", parse_identifier)?,
            generated_at: object.required("generated_at", parse_datetime)?,
            status: object.required("status", parse_enum::<ExecutionReportStatus>)?,
            leg_reports: object.required_array("leg_reports", LegReport::parse)?,
            fills: object.required_array("fills", Fill::parse)?,
            failures: object.required_array("failures", ExecutionFailure::parse)?,
            reconciliation_status: object
                .required("reconciliation_status", parse_enum::<ReconciliationStatus>)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ExecutionReport {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("schema_version", &self.schema_version),
            req("report_id", &self.report_id),
            req("plan_id", &self.plan_id),
            req("generated_at", &self.generated_at),
            req("status", &self.status),
            req("leg_reports", &self.leg_reports),
            req("fills", &self.fills),
            req("failures", &self.failures),
            req("reconciliation_status", &self.reconciliation_status),
        ])
    }
}

impl LegReport {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            plan_leg_id: object.required("plan_leg_id", parse_identifier)?,
            status: object.required("status", parse_enum::<LegReportStatus>)?,
            source_event_refs: object.optional_array("source_event_refs", parse_identifier)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for LegReport {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("plan_leg_id", &self.plan_leg_id),
            req("status", &self.status),
            opt("source_event_refs", &self.source_event_refs),
        ])
    }
}

impl ExecutionFailure {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            failure_id: object.required("failure_id", parse_identifier)?,
            plan_leg_id: object.optional("plan_leg_id", parse_identifier)?,
            failure_type: object.required("failure_type", parse_enum::<FailureMode>)?,
            severity: object.required("severity", parse_enum::<ExecutionFailureSeverity>)?,
            detail: object.optional("detail", |value, path| {
                parse_bounded_string(value, path, 0, 1024)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ExecutionFailure {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("failure_id", &self.failure_id),
            opt("plan_leg_id", &self.plan_leg_id),
            req("failure_type", &self.failure_type),
            req("severity", &self.severity),
            opt("detail", &self.detail),
        ])
    }
}

impl_contract!(Fill, "fill");
impl_schema_version!(Fill);

impl Fill {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            schema_version: object.required("schema_version", parse_version)?,
            fill_id: object.required("fill_id", parse_identifier)?,
            plan_id: object.required("plan_id", parse_identifier)?,
            plan_leg_id: object.required("plan_leg_id", parse_identifier)?,
            venue_id: object.required("venue_id", parse_identifier)?,
            instrument_id: object.required("instrument_id", parse_identifier)?,
            venue_order_id: object.optional("venue_order_id", |value, path| {
                parse_bounded_string(value, path, 0, 128)
            })?,
            client_order_id: object.optional("client_order_id", parse_identifier)?,
            timestamp: object.required("timestamp", parse_datetime)?,
            side: object.required("side", parse_enum::<FillSide>)?,
            price: object.required("price", parse_non_negative_decimal)?,
            quantity: object.required("quantity", parse_non_negative_decimal)?,
            fee: object.required("fee", AssetFlow::parse)?,
            source_event_id: object.required("source_event_id", parse_identifier)?,
            ledger_entry_id: object.optional("ledger_entry_id", parse_identifier)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for Fill {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("schema_version", &self.schema_version),
            req("fill_id", &self.fill_id),
            req("plan_id", &self.plan_id),
            req("plan_leg_id", &self.plan_leg_id),
            req("venue_id", &self.venue_id),
            req("instrument_id", &self.instrument_id),
            opt("venue_order_id", &self.venue_order_id),
            opt("client_order_id", &self.client_order_id),
            req("timestamp", &self.timestamp),
            req("side", &self.side),
            req("price", &self.price),
            req("quantity", &self.quantity),
            req("fee", &self.fee),
            req("source_event_id", &self.source_event_id),
            opt("ledger_entry_id", &self.ledger_entry_id),
        ])
    }
}

impl_contract!(LedgerEntry, "ledger_entry");
impl_schema_version!(LedgerEntry);

impl LedgerEntry {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let legs = object.required_array("legs", LedgerLeg::parse)?;
        ensure_min_items(&legs, path, "legs", 2)?;
        let parsed = Self {
            ledger_entry_id: object.required("ledger_entry_id", parse_identifier)?,
            journal_entry_id: object.required("journal_entry_id", parse_identifier)?,
            schema_version: object.required("schema_version", parse_version)?,
            timestamp: object.required("timestamp", parse_datetime)?,
            namespace: object.required("namespace", parse_enum::<LedgerNamespace>)?,
            entry_type: object.required("entry_type", parse_enum::<LedgerEntryType>)?,
            source_event_id: object.required("source_event_id", parse_identifier)?,
            idempotency_key: object.required("idempotency_key", parse_identifier)?,
            strategy_id: object.optional("strategy_id", parse_identifier)?,
            opportunity_id: object.optional("opportunity_id", parse_identifier)?,
            execution_plan_id: object.optional("execution_plan_id", parse_identifier)?,
            reversal_of: object.optional("reversal_of", parse_identifier)?,
            adjustment_of: object.optional("adjustment_of", parse_identifier)?,
            adjustment_reason_code: object.optional("adjustment_reason_code", parse_reason_code)?,
            legs,
            balance_assertion: object.required("balance_assertion", BalanceAssertion::parse)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for LedgerEntry {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("ledger_entry_id", &self.ledger_entry_id),
            req("journal_entry_id", &self.journal_entry_id),
            req("schema_version", &self.schema_version),
            req("timestamp", &self.timestamp),
            req("namespace", &self.namespace),
            req("entry_type", &self.entry_type),
            req("source_event_id", &self.source_event_id),
            req("idempotency_key", &self.idempotency_key),
            opt("strategy_id", &self.strategy_id),
            opt("opportunity_id", &self.opportunity_id),
            opt("execution_plan_id", &self.execution_plan_id),
            opt("reversal_of", &self.reversal_of),
            opt("adjustment_of", &self.adjustment_of),
            opt("adjustment_reason_code", &self.adjustment_reason_code),
            req("legs", &self.legs),
            req("balance_assertion", &self.balance_assertion),
        ])
    }
}

impl LedgerLeg {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            leg_id: object.required("leg_id", parse_identifier)?,
            account_id: object.required("account_id", parse_identifier)?,
            custody_location_id: object.optional("custody_location_id", parse_identifier)?,
            asset_id: object.required("asset_id", parse_identifier)?,
            direction: object.required("direction", parse_enum::<LedgerDirection>)?,
            amount: object.required("amount", parse_non_negative_decimal)?,
            valuation_usd: object.optional("valuation_usd", parse_decimal)?,
            memo: object.optional("memo", |value, path| {
                parse_bounded_string(value, path, 0, 512)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for LedgerLeg {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("leg_id", &self.leg_id),
            req("account_id", &self.account_id),
            opt("custody_location_id", &self.custody_location_id),
            req("asset_id", &self.asset_id),
            req("direction", &self.direction),
            req("amount", &self.amount),
            opt("valuation_usd", &self.valuation_usd),
            opt("memo", &self.memo),
        ])
    }
}

impl BalanceAssertion {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            balanced: object.required("balanced", parse_true_const)?,
            assertion_hash: object.required("assertion_hash", parse_hash_string)?,
            checked_by: object.optional("checked_by", |value, path| {
                parse_bounded_string(value, path, 0, 128)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for BalanceAssertion {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("balanced", &self.balanced),
            req("assertion_hash", &self.assertion_hash),
            opt("checked_by", &self.checked_by),
        ])
    }
}

impl_contract!(VenueCapabilityDescriptor, "venue_capability");
impl_schema_version!(VenueCapabilityDescriptor);

impl VenueCapabilityDescriptor {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let auth_modes = object.required_unique_array("auth_modes", parse_enum::<AuthMode>)?;
        ensure_min_items(&auth_modes, path, "auth_modes", 1)?;
        let data_surfaces =
            object.required_unique_array("data_surfaces", parse_enum::<DataSurface>)?;
        ensure_min_items(&data_surfaces, path, "data_surfaces", 1)?;
        let parsed = Self {
            schema_version: object.required("schema_version", parse_version)?,
            venue_id: object.required("venue_id", parse_identifier)?,
            venue_name: object.optional("venue_name", |value, path| {
                parse_bounded_string(value, path, 1, 128)
            })?,
            capability_version: object.required("capability_version", parse_version)?,
            market_capabilities: object
                .required_unique_array("market_capabilities", parse_enum::<MarketCapability>)?,
            execution_capabilities: object.required_unique_array(
                "execution_capabilities",
                parse_enum::<ExecutionCapability>,
            )?,
            auth_modes,
            data_surfaces,
            settlement_modes: object
                .required_unique_array("settlement_modes", parse_enum::<SettlementMode>)?,
            permission_model: object.optional("permission_model", PermissionModel::parse)?,
            rate_limit_model: object.required("rate_limit_model", RateLimitModel::parse)?,
            health_model: object.required("health_model", HealthModel::parse)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for VenueCapabilityDescriptor {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("schema_version", &self.schema_version),
            req("venue_id", &self.venue_id),
            opt("venue_name", &self.venue_name),
            req("capability_version", &self.capability_version),
            req("market_capabilities", &self.market_capabilities),
            req("execution_capabilities", &self.execution_capabilities),
            req("auth_modes", &self.auth_modes),
            req("data_surfaces", &self.data_surfaces),
            req("settlement_modes", &self.settlement_modes),
            opt("permission_model", &self.permission_model),
            req("rate_limit_model", &self.rate_limit_model),
            req("health_model", &self.health_model),
        ])
    }
}

impl PermissionModel {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let can_withdraw = object.optional("can_withdraw", parse_bool)?;
        if can_withdraw == Some(true) {
            return Err(invalid(
                &field_path(path, "can_withdraw"),
                "can_withdraw must be false",
            ));
        }
        let parsed = Self {
            can_read_public_data: object.optional("can_read_public_data", parse_bool)?,
            can_read_private_data: object.optional("can_read_private_data", parse_bool)?,
            can_trade: object.optional("can_trade", parse_bool)?,
            can_withdraw,
            requires_ip_binding: object.optional("requires_ip_binding", parse_bool)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for PermissionModel {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            opt("can_read_public_data", &self.can_read_public_data),
            opt("can_read_private_data", &self.can_read_private_data),
            opt("can_trade", &self.can_trade),
            opt("can_withdraw", &self.can_withdraw),
            opt("requires_ip_binding", &self.requires_ip_binding),
        ])
    }
}

impl RateLimitModel {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            unit: object.required("unit", parse_enum::<RateLimitUnit>)?,
            limit: object.required("limit", parse_u64)?,
            window_ms: object.required("window_ms", parse_duration_ms)?,
            source: object.optional("source", |value, path| {
                parse_bounded_string(value, path, 0, 256)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for RateLimitModel {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("unit", &self.unit),
            req("limit", &self.limit),
            req("window_ms", &self.window_ms),
            opt("source", &self.source),
        ])
    }
}

impl HealthModel {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            freshness_threshold_ms: object.required("freshness_threshold_ms", parse_duration_ms)?,
            disconnect_threshold: object.required("disconnect_threshold", parse_u64)?,
            unknown_state_is_critical: object
                .required("unknown_state_is_critical", parse_true_const)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for HealthModel {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("freshness_threshold_ms", &self.freshness_threshold_ms),
            req("disconnect_threshold", &self.disconnect_threshold),
            req("unknown_state_is_critical", &self.unknown_state_is_critical),
        ])
    }
}

impl_contract!(Incident, "incident");
impl_schema_version!(Incident);

impl Incident {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let source_event_refs =
            object.required_unique_array("source_event_refs", parse_identifier)?;
        ensure_min_items(&source_event_refs, path, "source_event_refs", 1)?;
        let parsed = Self {
            schema_version: object.required("schema_version", parse_version)?,
            incident_id: object.required("incident_id", parse_identifier)?,
            severity: object.required("severity", parse_enum::<IncidentSeverity>)?,
            status: object.required("status", parse_enum::<IncidentStatus>)?,
            opened_at: object.required("opened_at", parse_datetime)?,
            closed_at: object.optional("closed_at", parse_datetime)?,
            trigger: object.required("trigger", parse_reason_code)?,
            source_event_refs,
            impacted: object.required("impacted", ImpactedScope::parse)?,
            automatic_actions: object.required_array("automatic_actions", IncidentAction::parse)?,
            manual_actions: object.required_array("manual_actions", IncidentAction::parse)?,
            root_cause: object.optional("root_cause", |value, path| {
                parse_bounded_string(value, path, 0, 2048)
            })?,
            corrective_action: object.optional("corrective_action", |value, path| {
                parse_bounded_string(value, path, 0, 2048)
            })?,
            prevention_action: object.optional("prevention_action", |value, path| {
                parse_bounded_string(value, path, 0, 2048)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for Incident {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("schema_version", &self.schema_version),
            req("incident_id", &self.incident_id),
            req("severity", &self.severity),
            req("status", &self.status),
            req("opened_at", &self.opened_at),
            opt("closed_at", &self.closed_at),
            req("trigger", &self.trigger),
            req("source_event_refs", &self.source_event_refs),
            req("impacted", &self.impacted),
            req("automatic_actions", &self.automatic_actions),
            req("manual_actions", &self.manual_actions),
            opt("root_cause", &self.root_cause),
            opt("corrective_action", &self.corrective_action),
            opt("prevention_action", &self.prevention_action),
        ])
    }
}

impl ImpactedScope {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            venue_ids: object.optional_unique_array("venue_ids", parse_identifier)?,
            strategy_ids: object.optional_unique_array("strategy_ids", parse_identifier)?,
            capital_at_risk_usd: object
                .optional("capital_at_risk_usd", parse_non_negative_decimal)?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for ImpactedScope {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            opt("venue_ids", &self.venue_ids),
            opt("strategy_ids", &self.strategy_ids),
            opt("capital_at_risk_usd", &self.capital_at_risk_usd),
        ])
    }
}

impl IncidentAction {
    fn parse(value: &JsonValue, path: &str) -> ContractResult<Self> {
        let mut object = ObjectDecoder::new(value, path)?;
        let parsed = Self {
            action_id: object.required("action_id", parse_identifier)?,
            action_type: object.required("action_type", parse_enum::<IncidentActionType>)?,
            timestamp: object.required("timestamp", parse_datetime)?,
            detail: object.optional("detail", |value, path| {
                parse_bounded_string(value, path, 0, 1024)
            })?,
        };
        object.finish()?;
        Ok(parsed)
    }
}

impl CanonicalJson for IncidentAction {
    fn to_json_value(&self) -> JsonValue {
        object(vec![
            req("action_id", &self.action_id),
            req("action_type", &self.action_type),
            req("timestamp", &self.timestamp),
            opt("detail", &self.detail),
        ])
    }
}

fn object(fields: Vec<(&'static str, Option<JsonValue>)>) -> JsonValue {
    let mut values = BTreeMap::new();
    for (key, value) in fields {
        if let Some(value) = value {
            values.insert(key.to_owned(), value);
        }
    }
    JsonValue::Object(values)
}

fn req<T: CanonicalJson>(key: &'static str, value: &T) -> (&'static str, Option<JsonValue>) {
    (key, Some(value.to_json_value()))
}

fn opt<T: CanonicalJson>(
    key: &'static str,
    value: &Option<T>,
) -> (&'static str, Option<JsonValue>) {
    (key, value.as_ref().map(CanonicalJson::to_json_value))
}

fn opt_nullable<T: CanonicalJson>(
    key: &'static str,
    value: &Option<Option<T>>,
) -> (&'static str, Option<JsonValue>) {
    (
        key,
        value.as_ref().map(|inner| {
            inner
                .as_ref()
                .map_or(JsonValue::Null, CanonicalJson::to_json_value)
        }),
    )
}

struct ObjectDecoder<'a> {
    path: String,
    values: &'a BTreeMap<String, JsonValue>,
    seen: BTreeSet<String>,
}

impl<'a> ObjectDecoder<'a> {
    fn new(value: &'a JsonValue, path: &str) -> ContractResult<Self> {
        match value {
            JsonValue::Object(values) => Ok(Self {
                path: path.to_owned(),
                values,
                seen: BTreeSet::new(),
            }),
            _ => Err(ContractError::ExpectedObject {
                path: path.to_owned(),
            }),
        }
    }

    fn required<T>(
        &mut self,
        field: &str,
        parse: impl FnOnce(&JsonValue, &str) -> ContractResult<T>,
    ) -> ContractResult<T> {
        self.seen.insert(field.to_owned());
        let Some(value) = self.values.get(field) else {
            return Err(ContractError::MissingField {
                path: self.path.clone(),
                field: field.to_owned(),
            });
        };
        let path = field_path(&self.path, field);
        parse(value, &path)
    }

    fn optional<T>(
        &mut self,
        field: &str,
        parse: impl FnOnce(&JsonValue, &str) -> ContractResult<T>,
    ) -> ContractResult<Option<T>> {
        self.seen.insert(field.to_owned());
        self.values.get(field).map_or(Ok(None), |value| {
            let path = field_path(&self.path, field);
            parse(value, &path).map(Some)
        })
    }

    fn optional_nullable<T>(
        &mut self,
        field: &str,
        parse: impl FnOnce(&JsonValue, &str) -> ContractResult<T>,
    ) -> ContractResult<Option<Option<T>>> {
        self.seen.insert(field.to_owned());
        let Some(value) = self.values.get(field) else {
            return Ok(None);
        };
        match value {
            JsonValue::Null => Ok(Some(None)),
            other => {
                let path = field_path(&self.path, field);
                parse(other, &path).map(Some).map(Some)
            }
        }
    }

    fn required_array<T>(
        &mut self,
        field: &str,
        parse: impl Fn(&JsonValue, &str) -> ContractResult<T>,
    ) -> ContractResult<Vec<T>> {
        self.required(field, |value, path| parse_array(value, path, parse))
    }

    fn optional_array<T>(
        &mut self,
        field: &str,
        parse: impl Fn(&JsonValue, &str) -> ContractResult<T>,
    ) -> ContractResult<Option<Vec<T>>> {
        self.optional(field, |value, path| parse_array(value, path, parse))
    }

    fn required_unique_array<T: CanonicalJson>(
        &mut self,
        field: &str,
        parse: impl Fn(&JsonValue, &str) -> ContractResult<T>,
    ) -> ContractResult<Vec<T>> {
        let values = self.required_array(field, parse)?;
        ensure_unique_json_values(&values, &field_path(&self.path, field))?;
        Ok(values)
    }

    fn optional_unique_array<T: CanonicalJson>(
        &mut self,
        field: &str,
        parse: impl Fn(&JsonValue, &str) -> ContractResult<T>,
    ) -> ContractResult<Option<Vec<T>>> {
        let values = self.optional_array(field, parse)?;
        if let Some(values) = &values {
            ensure_unique_json_values(values, &field_path(&self.path, field))?;
        }
        Ok(values)
    }

    fn optional_unique_enum_array<T: EnumContract + CanonicalJson>(
        &mut self,
        field: &str,
        parse: impl Fn(&JsonValue, &str) -> ContractResult<T>,
    ) -> ContractResult<Option<Vec<T>>> {
        self.optional_unique_array(field, parse)
    }

    fn finish(self) -> ContractResult<()> {
        for field in self.values.keys() {
            if !self.seen.contains(field) {
                return Err(ContractError::UnknownField {
                    path: self.path,
                    field: field.clone(),
                });
            }
        }
        Ok(())
    }
}

fn field_path(path: &str, field: &str) -> String {
    if path == "$" {
        format!("$.{field}")
    } else {
        format!("{path}.{field}")
    }
}

fn index_path(path: &str, index: usize) -> String {
    format!("{path}[{index}]")
}

fn invalid(path: &str, message: impl Into<String>) -> ContractError {
    ContractError::InvalidValue {
        path: path.to_owned(),
        message: message.into(),
    }
}

fn wrong_type(path: &str, expected: &str) -> ContractError {
    ContractError::WrongType {
        path: path.to_owned(),
        expected: expected.to_owned(),
    }
}

fn parse_array<T>(
    value: &JsonValue,
    path: &str,
    parse: impl Fn(&JsonValue, &str) -> ContractResult<T>,
) -> ContractResult<Vec<T>> {
    let JsonValue::Array(values) = value else {
        return Err(wrong_type(path, "array"));
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let path = index_path(path, index);
            parse(value, &path)
        })
        .collect()
}

fn ensure_min_items<T>(values: &[T], path: &str, field: &str, min: usize) -> ContractResult<()> {
    if values.len() < min {
        return Err(invalid(
            &field_path(path, field),
            format!("must contain at least {min} item(s)"),
        ));
    }
    Ok(())
}

fn ensure_unique_json_values<T: CanonicalJson>(values: &[T], path: &str) -> ContractResult<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        let canonical = value.to_json_value().to_canonical_json();
        if !seen.insert(canonical) {
            return Err(invalid(path, "array items must be unique"));
        }
    }
    Ok(())
}

fn parse_open_object(value: &JsonValue, path: &str) -> ContractResult<BTreeMap<String, JsonValue>> {
    match value {
        JsonValue::Object(values) => Ok(values.clone()),
        _ => Err(wrong_type(path, "object")),
    }
}

fn parse_scalar_object(
    value: &JsonValue,
    path: &str,
) -> ContractResult<BTreeMap<String, JsonValue>> {
    let values = parse_open_object(value, path)?;
    for (key, value) in &values {
        match value {
            JsonValue::String(_) | JsonValue::Number(_) | JsonValue::Bool(_) => {}
            JsonValue::Null | JsonValue::Array(_) | JsonValue::Object(_) => {
                return Err(wrong_type(
                    &field_path(path, key),
                    "string, number, integer or boolean",
                ));
            }
        }
    }
    Ok(values)
}

fn parse_bool(value: &JsonValue, path: &str) -> ContractResult<bool> {
    match value {
        JsonValue::Bool(value) => Ok(*value),
        _ => Err(wrong_type(path, "boolean")),
    }
}

fn parse_true_const(value: &JsonValue, path: &str) -> ContractResult<bool> {
    let value = parse_bool(value, path)?;
    if value {
        Ok(true)
    } else {
        Err(invalid(path, "must be true"))
    }
}

fn parse_string(value: &JsonValue, path: &str) -> ContractResult<String> {
    match value {
        JsonValue::String(value) => Ok(value.clone()),
        _ => Err(wrong_type(path, "string")),
    }
}

fn parse_bounded_string(
    value: &JsonValue,
    path: &str,
    min_len: usize,
    max_len: usize,
) -> ContractResult<String> {
    let value = parse_string(value, path)?;
    let len = value.chars().count();
    if len < min_len {
        return Err(invalid(path, format!("string length must be >= {min_len}")));
    }
    if len > max_len {
        return Err(invalid(path, format!("string length must be <= {max_len}")));
    }
    Ok(value)
}

fn parse_enum<T: EnumContract>(value: &JsonValue, path: &str) -> ContractResult<T> {
    let value = parse_string(value, path)?;
    T::parse_enum(&value, path)
}

fn parse_identifier(value: &JsonValue, path: &str) -> ContractResult<Identifier> {
    let value = parse_string(value, path)?;
    validate_identifier(&value, path)?;
    Ok(Identifier(value))
}

fn parse_version(value: &JsonValue, path: &str) -> ContractResult<Version> {
    let value = parse_string(value, path)?;
    validate_version(&value, path)?;
    Ok(Version(value))
}

fn parse_datetime(value: &JsonValue, path: &str) -> ContractResult<DateTime> {
    let value = parse_string(value, path)?;
    validate_utc_datetime(&value, path)?;
    Ok(DateTime(value))
}

fn parse_duration_ms(value: &JsonValue, path: &str) -> ContractResult<DurationMs> {
    parse_u64(value, path).map(DurationMs)
}

fn parse_decimal(value: &JsonValue, path: &str) -> ContractResult<DecimalString> {
    let value = parse_string(value, path)?;
    validate_decimal_string(&value, path, DecimalKind::Any)?;
    Ok(DecimalString(value))
}

fn parse_non_negative_decimal(
    value: &JsonValue,
    path: &str,
) -> ContractResult<NonNegativeDecimalString> {
    let value = parse_string(value, path)?;
    validate_decimal_string(&value, path, DecimalKind::NonNegative)?;
    Ok(NonNegativeDecimalString(value))
}

fn parse_positive_decimal(value: &JsonValue, path: &str) -> ContractResult<PositiveDecimalString> {
    let value = parse_string(value, path)?;
    validate_decimal_string(&value, path, DecimalKind::Positive)?;
    Ok(PositiveDecimalString(value))
}

fn parse_reason_code(value: &JsonValue, path: &str) -> ContractResult<ReasonCode> {
    let value = parse_string(value, path)?;
    validate_reason_code(&value, path)?;
    Ok(ReasonCode(value))
}

fn parse_hash_string(value: &JsonValue, path: &str) -> ContractResult<HashString> {
    let value = parse_string(value, path)?;
    validate_hash_string(&value, path)?;
    Ok(HashString(value))
}

fn parse_signature_ref(value: &JsonValue, path: &str) -> ContractResult<SignatureRef> {
    let value = parse_string(value, path)?;
    validate_signature_ref(&value, path)?;
    Ok(SignatureRef(value))
}

fn parse_confidence(value: &JsonValue, path: &str) -> ContractResult<Confidence> {
    match value {
        JsonValue::Number(number) => {
            validate_confidence_number(number.as_str(), path)?;
            Ok(Confidence(number.clone()))
        }
        _ => Err(wrong_type(path, "number")),
    }
}

fn parse_u64(value: &JsonValue, path: &str) -> ContractResult<u64> {
    match value {
        JsonValue::Number(number) => {
            let raw = number.as_str();
            if raw.starts_with('-') || raw.contains(['.', 'e', 'E']) {
                return Err(invalid(path, "must be a non-negative integer"));
            }
            raw.parse::<u64>()
                .map_err(|_| invalid(path, "integer is out of range"))
        }
        _ => Err(wrong_type(path, "integer")),
    }
}

fn validate_identifier(value: &str, path: &str) -> ContractResult<()> {
    let len = value.len();
    if !(2..=128).contains(&len) {
        return Err(invalid(path, "identifier length must be 2..=128"));
    }
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid(path, "identifier cannot be empty"));
    };
    if !first.is_ascii_alphanumeric() {
        return Err(invalid(
            path,
            "identifier must start with ASCII letter or digit",
        ));
    }
    if bytes
        .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-')))
    {
        return Err(invalid(
            path,
            "identifier contains characters outside [A-Za-z0-9_.:-]",
        ));
    }
    Ok(())
}

fn validate_version(value: &str, path: &str) -> ContractResult<()> {
    let len = value.len();
    if !(1..=128).contains(&len) {
        return Err(invalid(path, "version length must be 1..=128"));
    }
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid(path, "version cannot be empty"));
    };
    if !first.is_ascii_alphanumeric() {
        return Err(invalid(
            path,
            "version must start with ASCII letter or digit",
        ));
    }
    if bytes.any(|byte| {
        !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'+' | b'-'))
    }) {
        return Err(invalid(
            path,
            "version contains characters outside [A-Za-z0-9_.:+-]",
        ));
    }
    Ok(())
}

fn validate_reason_code(value: &str, path: &str) -> ContractResult<()> {
    let len = value.len();
    if !(2..=64).contains(&len) {
        return Err(invalid(path, "reason code length must be 2..=64"));
    }
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid(path, "reason code cannot be empty"));
    };
    if !first.is_ascii_uppercase() {
        return Err(invalid(path, "reason code must start with A-Z"));
    }
    if bytes.any(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')) {
        return Err(invalid(path, "reason code must match [A-Z][A-Z0-9_]+"));
    }
    Ok(())
}

fn validate_hash_string(value: &str, path: &str) -> ContractResult<()> {
    validate_ascii_pattern(value, path, 16, 160, "hash string", |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'+' | b'.' | b'-')
    })
}

fn validate_signature_ref(value: &str, path: &str) -> ContractResult<()> {
    validate_ascii_pattern(value, path, 8, 256, "signature ref", |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b':' | b'+' | b'-')
    })
}

fn validate_ascii_pattern(
    value: &str,
    path: &str,
    min_len: usize,
    max_len: usize,
    label: &str,
    allowed: impl Fn(u8) -> bool,
) -> ContractResult<()> {
    let len = value.len();
    if len < min_len || len > max_len {
        return Err(invalid(
            path,
            format!("{label} length must be {min_len}..={max_len}"),
        ));
    }
    if value.bytes().any(|byte| !allowed(byte)) {
        return Err(invalid(
            path,
            format!("{label} contains illegal characters"),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum DecimalKind {
    Any,
    NonNegative,
    Positive,
}

fn validate_decimal_string(value: &str, path: &str, kind: DecimalKind) -> ContractResult<()> {
    if value.is_empty() {
        return Err(invalid(path, "decimal cannot be empty"));
    }
    if value.contains(['e', 'E']) {
        return Err(invalid(path, "decimal exponent notation is not allowed"));
    }

    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let is_negative = unsigned.len() != value.len();
    match kind {
        DecimalKind::Any => {}
        DecimalKind::NonNegative | DecimalKind::Positive if is_negative => {
            return Err(invalid(path, "decimal must be non-negative"));
        }
        DecimalKind::NonNegative | DecimalKind::Positive => {}
    }
    if unsigned.is_empty() {
        return Err(invalid(path, "decimal is missing digits"));
    }

    let (integer, fraction) = unsigned
        .split_once('.')
        .map_or((unsigned, None), |(left, right)| (left, Some(right)));
    if integer.is_empty() {
        return Err(invalid(path, "decimal integer part is required"));
    }
    if integer.len() > 1 && integer.starts_with('0') {
        return Err(invalid(
            path,
            "decimal integer part cannot have leading zeroes",
        ));
    }
    if !integer.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(
            path,
            "decimal integer part must contain digits only",
        ));
    }
    if let Some(fraction) = fraction {
        if fraction.is_empty() {
            return Err(invalid(
                path,
                "decimal fractional part is required after dot",
            ));
        }
        if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid(
                path,
                "decimal fractional part must contain digits only",
            ));
        }
    }
    if matches!(kind, DecimalKind::Positive)
        && !unsigned.bytes().any(|byte| matches!(byte, b'1'..=b'9'))
    {
        return Err(invalid(path, "decimal must be greater than zero"));
    }
    Ok(())
}

fn validate_confidence_number(value: &str, path: &str) -> ContractResult<()> {
    if value.starts_with('-') || value.contains(['e', 'E']) {
        return Err(invalid(
            path,
            "confidence must be a non-negative decimal number without exponent",
        ));
    }
    let (integer, fraction) = value
        .split_once('.')
        .map_or((value, None), |(left, right)| (left, Some(right)));
    match integer {
        "0" => Ok(()),
        "1" if fraction.is_none_or(|digits| digits.bytes().all(|byte| byte == b'0')) => Ok(()),
        _ => Err(invalid(path, "confidence must be between 0 and 1")),
    }
}

fn validate_utc_datetime(value: &str, path: &str) -> ContractResult<()> {
    if !value.ends_with('Z') {
        return Err(invalid(path, "timestamp must be UTC and end with Z"));
    }
    let body = &value[..value.len() - 1];
    let (date, time) = body
        .split_once('T')
        .ok_or_else(|| invalid(path, "timestamp must contain T separator"))?;
    validate_date(date, path)?;
    validate_time(time, path)
}

fn validate_date(date: &str, path: &str) -> ContractResult<()> {
    if date.len() != 10 {
        return Err(invalid(path, "date must be YYYY-MM-DD"));
    }
    let bytes = date.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(invalid(path, "date must be YYYY-MM-DD"));
    }
    let year = parse_fixed_digits(&date[0..4], path, "year")?;
    let month = parse_fixed_digits(&date[5..7], path, "month")?;
    let day = parse_fixed_digits(&date[8..10], path, "day")?;
    if year == 0 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(invalid(path, "date components out of range"));
    }
    Ok(())
}

fn validate_time(time: &str, path: &str) -> ContractResult<()> {
    let (main, fraction) = time
        .split_once('.')
        .map_or((time, None), |(left, right)| (left, Some(right)));
    if main.len() != 8 {
        return Err(invalid(path, "time must be HH:MM:SS"));
    }
    let bytes = main.as_bytes();
    if bytes[2] != b':' || bytes[5] != b':' {
        return Err(invalid(path, "time must be HH:MM:SS"));
    }
    let hour = parse_fixed_digits(&main[0..2], path, "hour")?;
    let minute = parse_fixed_digits(&main[3..5], path, "minute")?;
    let second = parse_fixed_digits(&main[6..8], path, "second")?;
    if hour > 23 || minute > 59 || second > 60 {
        return Err(invalid(path, "time components out of range"));
    }
    if let Some(fraction) = fraction {
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid(path, "fractional seconds must contain digits"));
        }
    }
    Ok(())
}

fn parse_fixed_digits(value: &str, path: &str, label: &str) -> ContractResult<u32> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(path, format!("{label} must contain digits")));
    }
    value
        .parse::<u32>()
        .map_err(|_| invalid(path, format!("{label} out of range")))
}

struct JsonParser<'a> {
    chars: Vec<char>,
    pos: usize,
    input: &'a str,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
            input,
        }
    }

    fn parse(mut self) -> ContractResult<JsonValue> {
        self.skip_ws();
        let value = self.parse_value("$")?;
        self.skip_ws();
        if self.pos != self.chars.len() {
            return Err(self.json_error("$", "trailing characters"));
        }
        Ok(value)
    }

    fn parse_value(&mut self, path: &str) -> ContractResult<JsonValue> {
        self.skip_ws();
        match self.peek() {
            Some('n') => {
                self.consume_literal("null", path)?;
                Ok(JsonValue::Null)
            }
            Some('t') => {
                self.consume_literal("true", path)?;
                Ok(JsonValue::Bool(true))
            }
            Some('f') => {
                self.consume_literal("false", path)?;
                Ok(JsonValue::Bool(false))
            }
            Some('"') => self.parse_string(path).map(JsonValue::String),
            Some('[') => self.parse_array(path),
            Some('{') => self.parse_object(path),
            Some('-' | '0'..='9') => self.parse_number(path).map(JsonValue::Number),
            Some(other) => Err(self.json_error(path, format!("unexpected character `{other}`"))),
            None => Err(self.json_error(path, "unexpected end of input")),
        }
    }

    fn parse_object(&mut self, path: &str) -> ContractResult<JsonValue> {
        self.expect('{', path)?;
        self.skip_ws();
        let mut values = BTreeMap::new();
        if self.peek() == Some('}') {
            self.pos += 1;
            return Ok(JsonValue::Object(values));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string(path)?;
            self.skip_ws();
            self.expect(':', path)?;
            let value_path = field_path(path, &key);
            let value = self.parse_value(&value_path)?;
            if values.insert(key.clone(), value).is_some() {
                return Err(self.json_error(&value_path, "duplicate object key"));
            }
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some('}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.json_error(path, "expected comma or closing brace")),
            }
        }
        Ok(JsonValue::Object(values))
    }

    fn parse_array(&mut self, path: &str) -> ContractResult<JsonValue> {
        self.expect('[', path)?;
        self.skip_ws();
        let mut values = Vec::new();
        if self.peek() == Some(']') {
            self.pos += 1;
            return Ok(JsonValue::Array(values));
        }
        loop {
            let value_path = index_path(path, values.len());
            values.push(self.parse_value(&value_path)?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.json_error(path, "expected comma or closing bracket")),
            }
        }
        Ok(JsonValue::Array(values))
    }

    fn parse_string(&mut self, path: &str) -> ContractResult<String> {
        self.expect('"', path)?;
        let mut out = String::new();
        loop {
            let Some(ch) = self.next() else {
                return Err(self.json_error(path, "unterminated string"));
            };
            match ch {
                '"' => break,
                '\\' => {
                    let Some(escaped) = self.next() else {
                        return Err(self.json_error(path, "unterminated escape"));
                    };
                    match escaped {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{0008}'),
                        'f' => out.push('\u{000c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let code = self.parse_hex4(path)?;
                            let Some(ch) = char::from_u32(u32::from(code)) else {
                                return Err(self.json_error(path, "invalid unicode escape"));
                            };
                            out.push(ch);
                        }
                        other => {
                            return Err(
                                self.json_error(path, format!("invalid escape `\\{other}`"))
                            );
                        }
                    }
                }
                ch if ch <= '\u{001f}' => {
                    return Err(self.json_error(path, "control character in string"));
                }
                other => out.push(other),
            }
        }
        Ok(out)
    }

    fn parse_hex4(&mut self, path: &str) -> ContractResult<u16> {
        let mut value = 0_u16;
        for _ in 0..4 {
            let Some(ch) = self.next() else {
                return Err(self.json_error(path, "short unicode escape"));
            };
            let Some(digit) = ch.to_digit(16) else {
                return Err(self.json_error(path, "invalid unicode hex digit"));
            };
            value = (value << 4) | digit as u16;
        }
        Ok(value)
    }

    fn parse_number(&mut self, path: &str) -> ContractResult<JsonNumber> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        match self.peek() {
            Some('0') => {
                self.pos += 1;
            }
            Some('1'..='9') => {
                self.pos += 1;
                while matches!(self.peek(), Some('0'..='9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.json_error(path, "invalid number integer part")),
        }
        if self.peek() == Some('.') {
            self.pos += 1;
            let fraction_start = self.pos;
            while matches!(self.peek(), Some('0'..='9')) {
                self.pos += 1;
            }
            if self.pos == fraction_start {
                return Err(self.json_error(path, "invalid number fraction"));
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.pos += 1;
            }
            let exponent_start = self.pos;
            while matches!(self.peek(), Some('0'..='9')) {
                self.pos += 1;
            }
            if self.pos == exponent_start {
                return Err(self.json_error(path, "invalid number exponent"));
            }
        }
        Ok(JsonNumber(self.chars[start..self.pos].iter().collect()))
    }

    fn consume_literal(&mut self, literal: &str, path: &str) -> ContractResult<()> {
        for expected in literal.chars() {
            if self.next() != Some(expected) {
                return Err(self.json_error(path, format!("expected literal `{literal}`")));
            }
        }
        Ok(())
    }

    fn expect(&mut self, expected: char, path: &str) -> ContractResult<()> {
        match self.next() {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => {
                Err(self.json_error(path, format!("expected `{expected}`, got `{actual}`")))
            }
            None => Err(self.json_error(path, format!("expected `{expected}`, got end of input"))),
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\n' | '\r' | '\t')) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn json_error(&self, path: &str, message: impl Into<String>) -> ContractError {
        let _ = self.input;
        ContractError::Json {
            path: path.to_owned(),
            message: message.into(),
        }
    }
}

fn write_json_string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            ch if ch <= '\u{001f}' => {
                out.push_str("\\u");
                out.push_str(&format!("{:04x}", ch as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn valid_fixtures_parse_and_roundtrip_canonically() {
        assert_roundtrip::<Asset>("asset.valid.json");
        assert_roundtrip::<Instrument>("instrument.valid.json");
        assert_roundtrip::<PortfolioState>("portfolio_state.valid.json");
        assert_roundtrip::<NormalizedEvent>("normalized_event.valid.json");
        assert_roundtrip::<CandidatePortfolioTransition>(
            "candidate_portfolio_transition.valid.json",
        );
        assert_roundtrip::<RiskDecision>("risk_decision.valid.json");
        assert_roundtrip::<ExecutionPlan>("execution_plan.valid.json");
        assert_roundtrip::<ExecutionReport>("execution_report.valid.json");
        assert_roundtrip::<Fill>("fill.valid.json");
        assert_roundtrip::<LedgerEntry>("ledger_entry.valid.json");
        assert_roundtrip::<LedgerEntry>("ledger_entry.adjustment.valid.json");
        assert_roundtrip::<VenueCapabilityDescriptor>("venue_capability.valid.json");
        assert_roundtrip::<Incident>("incident.valid.json");
    }

    #[test]
    fn invalid_fixtures_are_rejected() {
        assert_invalid::<Asset>("asset.invalid.unknown_field.json");
        assert_invalid::<Instrument>("instrument.invalid.enum.json");
        assert_invalid::<PortfolioState>("portfolio_state.invalid.confidence.json");
        assert_invalid::<NormalizedEvent>("normalized_event.invalid.hash.json");
        assert_invalid::<CandidatePortfolioTransition>(
            "candidate_portfolio_transition.invalid.decimal.json",
        );
        assert_invalid::<RiskDecision>("risk_decision.invalid.enum.json");
        assert_invalid::<ExecutionPlan>("execution_plan.invalid.unknown_state_action.json");
        assert_invalid::<ExecutionReport>("execution_report.invalid.enum.json");
        assert_invalid::<Fill>("fill.invalid.decimal.json");
        assert_invalid::<LedgerEntry>("ledger_entry.invalid.balance_assertion.json");
        assert_invalid::<LedgerEntry>("ledger_entry.invalid.adjustment_reason_code.json");
        assert_invalid::<VenueCapabilityDescriptor>("venue_capability.invalid.withdraw.json");
        assert_invalid::<Incident>("incident.invalid.enum.json");
    }

    #[test]
    fn replay_incidents_are_event_traceable() {
        for relative in [
            "reconciliation_mismatch/expected/incidents.jsonl",
            "incident_unknown_state/expected/incidents.jsonl",
        ] {
            let input = fs::read_to_string(replay_fixture_path(relative))
                .unwrap_or_else(|error| panic!("cannot read replay fixture {relative}: {error}"));
            let mut parsed_count = 0;
            for line in input.lines().filter(|line| !line.trim().is_empty()) {
                let incident = from_json_strict::<Incident>(line)
                    .unwrap_or_else(|error| panic!("{relative} incident should parse: {error}"));
                assert!(
                    !incident.source_event_refs.is_empty(),
                    "{relative} incident must trace at least one event"
                );
                parsed_count += 1;
            }
            assert!(parsed_count > 0, "{relative} should contain incidents");
        }
    }

    #[test]
    fn unknown_fields_bad_enums_and_bad_decimals_fail() {
        let unknown = r#"{
            "schema_version":"1.0.0",
            "asset_id":"asset:BTC",
            "canonical_symbol":"BTC",
            "decimal_precision":8,
            "risk_group":"risk:MAJOR",
            "settlement_properties":{"settlement_kind":"OffChainLedger","can_be_collateral":true},
            "extra":"must fail"
        }"#;
        assert!(from_json_strict::<Asset>(unknown).is_err());

        let bad_enum = r#"{
            "schema_version":"1.0.0",
            "instrument_id":"inst:BTC-USDC",
            "venue_id":"venue:SIM",
            "kind":"Spot",
            "settlement_asset_id":"asset:USDC",
            "pricing_properties":{},
            "execution_model":{"supports_live_execution":false},
            "failure_model":{"unknown_state_is_critical":true}
        }"#;
        assert!(from_json_strict::<Instrument>(bad_enum).is_err());

        let bad_decimal = r#"{
            "schema_version":"1.0.0",
            "fill_id":"fill:01",
            "plan_id":"plan:01",
            "plan_leg_id":"leg:01",
            "venue_id":"venue:SIM",
            "instrument_id":"inst:BTC-USDC",
            "timestamp":"2026-01-01T00:00:00Z",
            "side":"Buy",
            "price":123.45,
            "quantity":"0.01",
            "fee":{"asset_id":"asset:USDC","direction":"Out","amount":"0.01"},
            "source_event_id":"event:01"
        }"#;
        assert!(from_json_strict::<Fill>(bad_decimal).is_err());
    }

    fn assert_roundtrip<T: ContractJson>(file_name: &str) {
        let input = read_fixture("valid", file_name);
        let parsed = from_json_strict::<T>(&input)
            .unwrap_or_else(|error| panic!("{file_name} should parse: {error}"));
        parsed
            .validate()
            .unwrap_or_else(|error| panic!("{file_name} should validate: {error}"));
        let canonical = to_canonical_json(&parsed);
        let reparsed = from_json_strict::<T>(&canonical)
            .unwrap_or_else(|error| panic!("{file_name} canonical should parse: {error}"));
        assert_eq!(canonical, to_canonical_json(&reparsed));
    }

    fn assert_invalid<T: ContractJson>(file_name: &str) {
        let input = read_fixture("invalid", file_name);
        assert!(
            from_json_strict::<T>(&input).is_err(),
            "{file_name} should be rejected"
        );
    }

    fn read_fixture(kind: &str, file_name: &str) -> String {
        let path = fixture_path(kind).join(file_name);
        fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
    }

    fn fixture_path(kind: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/schema")
            .join(kind)
    }

    fn replay_fixture_path(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/replay")
            .join(relative)
    }
}
