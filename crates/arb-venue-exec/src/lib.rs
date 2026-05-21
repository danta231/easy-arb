//! `arb-venue-exec` 可变执行适配器边界。
//!
//! 中文说明：本 crate 只定义可变执行 trait、模拟实现和幂等键处理。默认
//! feature 为空，不连接真实交易场所、不提交真实资金动作、不做策略判断或
//! 风控批准。
//!
//! ```rust,ignore
//! #[cfg(feature = "live-exec")]
//! use arb_venue_exec::live::{BinanceSpotExecAdapter, BinanceUsdmExecAdapter};
//! ```

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::str::FromStr;

use arb_contracts::{
    ExecutionActionType, ExecutionLeg, ExecutionLegState, ExecutionMode, ExecutionOrderType,
    ExecutionPlan, ExecutionTimeInForce, TransitionSide,
};
use arb_domain::{
    AccountId, Amount, AssetId, Decimal, InstrumentId, OrderId, Price, Quantity, UtcTimestamp,
    VenueId,
};

/// 可变执行模块统一返回类型。
pub type VenueExecResult<T> = Result<T, VenueExecError>;

/// 实盘可变执行 feature 是否已显式开启。
///
/// 中文说明：默认构建必须返回 `false`。即使未来开启 `live-exec`，也仍需要
/// 运行时配置、熔断、权限和签名边界继续拦截。
pub const LIVE_EXEC_FEATURE_ENABLED: bool = cfg!(feature = "live-exec");

/// 可变执行边界错误。
///
/// 中文说明：外部未知状态、幂等冲突和实盘能力不可用都不能被调用方当作成功。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VenueExecError {
    InvalidToken {
        field: &'static str,
        value: String,
        reason: &'static str,
    },
    InvalidRequest {
        field: &'static str,
        reason: &'static str,
    },
    DispatchBlocked {
        reason: String,
    },
    IdempotencyConflict {
        idempotency_key: IdempotencyKey,
        existing_fingerprint: String,
        incoming_fingerprint: String,
    },
    SigningFailed {
        reason: String,
    },
    ExternalRejected {
        venue_id: VenueId,
        endpoint: String,
        status_code: u16,
        reason: String,
    },
    UnknownExternalState {
        venue_id: VenueId,
        detail: String,
    },
}

impl fmt::Display for VenueExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken {
                field,
                value,
                reason,
            } => write!(f, "{field} `{value}` is invalid: {reason}"),
            Self::InvalidRequest { field, reason } => {
                write!(f, "{field}: invalid mutable execution request: {reason}")
            }
            Self::DispatchBlocked { reason } => write!(
                f,
                "mutable execution dispatch blocked before venue submission: {reason}"
            ),
            Self::IdempotencyConflict {
                idempotency_key,
                existing_fingerprint,
                incoming_fingerprint,
            } => write!(
                f,
                "idempotency key `{idempotency_key}` was reused with a different request: existing `{existing_fingerprint}`, incoming `{incoming_fingerprint}`"
            ),
            Self::SigningFailed { reason } => {
                write!(f, "mutable execution signing failed: {reason}")
            }
            Self::ExternalRejected {
                venue_id,
                endpoint,
                status_code,
                reason,
            } => write!(
                f,
                "venue `{venue_id}` rejected mutable execution endpoint `{endpoint}` with status {status_code}: {reason}"
            ),
            Self::UnknownExternalState { venue_id, detail } => write!(
                f,
                "venue `{venue_id}` mutable execution external state is unknown: {detail}"
            ),
        }
    }
}

impl Error for VenueExecError {}

macro_rules! token_type {
    ($name:ident, $field:literal, $doc:literal) => {
        #[doc = $doc]
        ///
        /// 中文说明：执行边界标识只允许稳定 ASCII token，避免日志和 fixture
        /// 出现凭证、空白或不可见字符。
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> VenueExecResult<Self> {
                let value = value.into();
                validate_token($field, &value)?;
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
    };
}

token_type!(IdempotencyKey, "idempotency_key", "可变动作幂等键。");
token_type!(MutableActionId, "action_id", "模拟可变动作 ID。");
token_type!(ExternalOrderId, "external_order_id", "外部订单引用。");
token_type!(ExternalTransferId, "external_transfer_id", "外部转账引用。");

fn validate_token(field: &'static str, value: &str) -> VenueExecResult<()> {
    if value.is_empty() {
        return Err(VenueExecError::InvalidToken {
            field,
            value: value.to_owned(),
            reason: "value cannot be empty",
        });
    }

    if value.bytes().any(|byte| {
        !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/'))
    }) {
        return Err(VenueExecError::InvalidToken {
            field,
            value: value.to_owned(),
            reason:
                "only ASCII letters, digits, underscore, dash, dot, colon and slash are allowed",
        });
    }

    Ok(())
}

/// 可变动作类型。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MutableActionKind {
    SubmitOrder,
    CancelOrder,
    TransferRequest,
}

impl MutableActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubmitOrder => "submit_order",
            Self::CancelOrder => "cancel_order",
            Self::TransferRequest => "transfer_request",
        }
    }
}

impl fmt::Display for MutableActionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 可变动作状态。
///
/// 中文说明：`Unknown` 是明确失败闭合状态，不能被上层解释为已成功。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutableActionStatus {
    Accepted,
    Unknown,
    Rejected,
}

impl MutableActionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Unknown => "unknown",
            Self::Rejected => "rejected",
        }
    }
}

impl fmt::Display for MutableActionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 私有订单确认来源。
///
/// 中文说明：REST 下单回执只能证明请求已发出或被接收，不能证明最终成交结果。
/// 最终状态必须来自私有 user data stream（用户数据流）或查单结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderConfirmationSource {
    PrivateStream,
    OrderQuery,
}

impl OrderConfirmationSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrivateStream => "private_stream",
            Self::OrderQuery => "order_query",
        }
    }
}

/// 私有订单确认状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderConfirmationStatus {
    Acknowledged,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
    Expired,
    Unknown,
}

impl OrderConfirmationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acknowledged => "acknowledged",
            Self::PartiallyFilled => "partially_filled",
            Self::Filled => "filled",
            Self::Cancelled => "cancelled",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Unknown => "unknown",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Filled | Self::Cancelled | Self::Rejected | Self::Expired | Self::Unknown
        )
    }
}

/// 私有订单市场。
///
/// 中文说明：该枚举只描述执行适配和私有确认边界上的市场类型，不能被策略或
/// 风控层直接依赖。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateOrderMarket {
    Spot,
    UsdmFutures,
    BybitSpot,
    BybitLinear,
    OkxSpot,
    OkxSwap,
    BitgetSpot,
    BitgetUsdtFutures,
    AsterPerp,
    HyperliquidPerp,
}

impl PrivateOrderMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spot => "Spot",
            Self::UsdmFutures => "UsdmFutures",
            Self::BybitSpot => "BybitSpot",
            Self::BybitLinear => "BybitLinear",
            Self::OkxSpot => "OkxSpot",
            Self::OkxSwap => "OkxSwap",
            Self::BitgetSpot => "BitgetSpot",
            Self::BitgetUsdtFutures => "BitgetUsdtFutures",
            Self::AsterPerp => "AsterPerp",
            Self::HyperliquidPerp => "HyperliquidPerp",
        }
    }

    fn token(self) -> &'static str {
        match self {
            Self::Spot => "spot",
            Self::UsdmFutures => "usdm",
            Self::BybitSpot => "bybit-spot",
            Self::BybitLinear => "bybit-linear",
            Self::OkxSpot => "okx-spot",
            Self::OkxSwap => "okx-swap",
            Self::BitgetSpot => "bitget-spot",
            Self::BitgetUsdtFutures => "bitget-usdt-futures",
            Self::AsterPerp => "aster-perp",
            Self::HyperliquidPerp => "hyperliquid-perp",
        }
    }

    fn instrument_suffix(self) -> &'static str {
        match self {
            Self::Spot => "SPOT",
            Self::UsdmFutures => "USDM-PERP",
            Self::BybitSpot => "SPOT",
            Self::BybitLinear => "LINEAR-PERP",
            Self::OkxSpot => "SPOT",
            Self::OkxSwap => "SWAP",
            Self::BitgetSpot => "SPOT",
            Self::BitgetUsdtFutures => "USDT-FUTURES",
            Self::AsterPerp => "USDT-FUTURES",
            Self::HyperliquidPerp => "PERP",
        }
    }

    fn venue_family(self) -> &'static str {
        match self {
            Self::Spot | Self::UsdmFutures => "BINANCE",
            Self::BybitSpot | Self::BybitLinear => "BYBIT",
            Self::OkxSpot | Self::OkxSwap => "OKX",
            Self::BitgetSpot | Self::BitgetUsdtFutures => "BITGET",
            Self::AsterPerp => "ASTER",
            Self::HyperliquidPerp => "HYPERLIQUID",
        }
    }
}

/// 私有成交增量。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivateOrderFillUpdate {
    pub source_event_id: String,
    pub timestamp: String,
    pub price: String,
    pub quantity: String,
    pub fee_asset_id: Option<AssetId>,
    pub fee_amount: Option<Amount>,
    pub trade_id: Option<String>,
}

/// 标准化私有订单更新。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivateOrderUpdate {
    pub source: OrderConfirmationSource,
    pub market: PrivateOrderMarket,
    pub venue_id: VenueId,
    pub account_id: AccountId,
    pub instrument_id: InstrumentId,
    pub symbol: String,
    pub source_event_id: String,
    pub event_time: String,
    pub status: OrderConfirmationStatus,
    pub execution_type: Option<String>,
    pub side: Option<OrderSide>,
    pub venue_order_id: Option<ExternalOrderId>,
    pub exchange_order_id: Option<String>,
    pub client_order_id: Option<OrderId>,
    pub cumulative_filled_quantity: Option<Quantity>,
    pub last_fill: Option<PrivateOrderFillUpdate>,
}

/// 查单确认请求。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmOrderStatusRequest {
    pub venue_id: VenueId,
    pub account_id: AccountId,
    pub instrument_id: InstrumentId,
    pub order_ref: OrderReference,
    pub source_event_id: String,
}

impl ConfirmOrderStatusRequest {
    pub fn new(
        venue_id: VenueId,
        account_id: AccountId,
        instrument_id: InstrumentId,
        order_ref: OrderReference,
        source_event_id: impl Into<String>,
    ) -> Self {
        Self {
            venue_id,
            account_id,
            instrument_id,
            order_ref,
            source_event_id: source_event_id.into(),
        }
    }
}

/// 订单状态确认 trait。
pub trait ConfirmOrderStatus {
    fn confirm_order_status(
        &mut self,
        request: ConfirmOrderStatusRequest,
    ) -> VenueExecResult<PrivateOrderUpdate>;
}

/// 订单方向。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

/// 永续合约持仓方向。
///
/// 中文说明：部分交易所的双向持仓模式要求下单时显式携带持仓方向，例如
/// Binance USD-M 的 `positionSide`。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PerpPositionSide {
    Both,
    Long,
    Short,
}

impl PerpPositionSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Both => "BOTH",
            Self::Long => "LONG",
            Self::Short => "SHORT",
        }
    }
}

/// 订单类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutableOrderType {
    Market,
    Limit,
    PostOnly,
}

impl MutableOrderType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Limit => "limit",
            Self::PostOnly => "post_only",
        }
    }
}

/// 限价订单有效期/成交策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutableTimeInForce {
    Gtc,
    Ioc,
    Fok,
}

impl MutableTimeInForce {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gtc => "GTC",
            Self::Ioc => "IOC",
            Self::Fok => "FOK",
        }
    }
}

/// 订单引用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OrderReference {
    ClientOrderId(OrderId),
    VenueOrderId(ExternalOrderId),
}

impl OrderReference {
    fn fingerprint(&self) -> String {
        match self {
            Self::ClientOrderId(value) => format!("client_order_id:{value}"),
            Self::VenueOrderId(value) => format!("venue_order_id:{value}"),
        }
    }
}

/// 外部动作引用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalActionRef {
    Order(ExternalOrderId),
    Cancel(MutableActionId),
    Transfer(ExternalTransferId),
}

/// 提交订单请求。
///
/// 中文说明：这是适配器边界请求，不是策略输出，也不是风控批准。调用方必须
/// 已经在上游完成执行计划、权限和熔断检查。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmitOrderRequest {
    pub venue_id: VenueId,
    pub account_id: AccountId,
    pub instrument_id: InstrumentId,
    pub side: OrderSide,
    pub order_type: MutableOrderType,
    pub time_in_force: Option<MutableTimeInForce>,
    pub reduce_only: bool,
    pub position_side: Option<PerpPositionSide>,
    pub quantity: Quantity,
    pub limit_price: Option<Price>,
    pub client_order_id: Option<OrderId>,
    pub idempotency_key: IdempotencyKey,
}

impl SubmitOrderRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        venue_id: VenueId,
        account_id: AccountId,
        instrument_id: InstrumentId,
        side: OrderSide,
        order_type: MutableOrderType,
        quantity: Quantity,
        limit_price: Option<Price>,
        client_order_id: Option<OrderId>,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        Self {
            venue_id,
            account_id,
            instrument_id,
            side,
            order_type,
            time_in_force: None,
            reduce_only: false,
            position_side: None,
            quantity,
            limit_price,
            client_order_id,
            idempotency_key,
        }
    }

    pub fn with_time_in_force(mut self, time_in_force: MutableTimeInForce) -> Self {
        self.time_in_force = Some(time_in_force);
        self
    }

    pub fn with_reduce_only(mut self, reduce_only: bool) -> Self {
        self.reduce_only = reduce_only;
        self
    }

    pub fn with_position_side(mut self, position_side: PerpPositionSide) -> Self {
        self.position_side = Some(position_side);
        self
    }

    pub fn validate(&self) -> VenueExecResult<()> {
        match (self.order_type, self.limit_price) {
            (MutableOrderType::Limit | MutableOrderType::PostOnly, None) => {
                Err(VenueExecError::InvalidRequest {
                    field: "limit_price",
                    reason: "limit and post-only orders require a limit price",
                })
            }
            (MutableOrderType::Market, Some(_)) => Err(VenueExecError::InvalidRequest {
                field: "limit_price",
                reason: "market orders must not carry a limit price",
            }),
            _ => Ok(()),
        }?;
        match (self.order_type, self.time_in_force) {
            (MutableOrderType::Limit, _) | (_, None) => Ok(()),
            (MutableOrderType::Market, Some(_)) => Err(VenueExecError::InvalidRequest {
                field: "time_in_force",
                reason: "market orders must not carry time_in_force",
            }),
            (MutableOrderType::PostOnly, Some(_)) => Err(VenueExecError::InvalidRequest {
                field: "time_in_force",
                reason: "post-only orders encode their own maker-only time-in-force",
            }),
        }
    }

    fn fingerprint(&self) -> RequestFingerprint {
        RequestFingerprint(format!(
            "kind={};venue={};account={};instrument={};side={};order_type={};time_in_force={};reduce_only={};position_side={};quantity={};limit_price={};client_order_id={}",
            MutableActionKind::SubmitOrder,
            self.venue_id,
            self.account_id,
            self.instrument_id,
            self.side.as_str(),
            self.order_type.as_str(),
            self.time_in_force.map(MutableTimeInForce::as_str).unwrap_or(""),
            self.reduce_only,
            self.position_side.map(PerpPositionSide::as_str).unwrap_or(""),
            self.quantity,
            optional_display(self.limit_price),
            optional_display(self.client_order_id.as_ref())
        ))
    }
}

/// 撤单请求。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelOrderRequest {
    pub venue_id: VenueId,
    pub account_id: AccountId,
    pub order_ref: OrderReference,
    pub idempotency_key: IdempotencyKey,
}

impl CancelOrderRequest {
    pub fn new(
        venue_id: VenueId,
        account_id: AccountId,
        order_ref: OrderReference,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        Self {
            venue_id,
            account_id,
            order_ref,
            idempotency_key,
        }
    }

    fn fingerprint(&self) -> RequestFingerprint {
        RequestFingerprint(format!(
            "kind={};venue={};account={};order_ref={}",
            MutableActionKind::CancelOrder,
            self.venue_id,
            self.account_id,
            self.order_ref.fingerprint()
        ))
    }
}

/// 查询可变动作状态请求。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryActionStatusRequest {
    ByActionId(MutableActionId),
    ByIdempotencyKey(IdempotencyKey),
}

/// 转账请求。
///
/// 中文说明：阶段 11 只允许模拟实现接收该请求，不能触发真实资金移动。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferRequest {
    pub venue_id: VenueId,
    pub from_account_id: AccountId,
    pub to_account_id: AccountId,
    pub asset_id: AssetId,
    pub amount: Amount,
    pub idempotency_key: IdempotencyKey,
}

impl TransferRequest {
    pub fn new(
        venue_id: VenueId,
        from_account_id: AccountId,
        to_account_id: AccountId,
        asset_id: AssetId,
        amount: Amount,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        Self {
            venue_id,
            from_account_id,
            to_account_id,
            asset_id,
            amount,
            idempotency_key,
        }
    }

    fn fingerprint(&self) -> RequestFingerprint {
        RequestFingerprint(format!(
            "kind={};venue={};from_account={};to_account={};asset={};amount={}",
            MutableActionKind::TransferRequest,
            self.venue_id,
            self.from_account_id,
            self.to_account_id,
            self.asset_id,
            self.amount
        ))
    }
}

/// 可变动作回执。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutableActionReceipt {
    pub action_id: MutableActionId,
    pub kind: MutableActionKind,
    pub status: MutableActionStatus,
    pub idempotency_key: IdempotencyKey,
    pub venue_id: VenueId,
    pub external_ref: Option<ExternalActionRef>,
    pub duplicate: bool,
    pub simulated: bool,
}

/// 可变动作状态报告。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutableActionStatusReport {
    pub action_id: Option<MutableActionId>,
    pub kind: Option<MutableActionKind>,
    pub status: MutableActionStatus,
    pub idempotency_key: Option<IdempotencyKey>,
    pub external_ref: Option<ExternalActionRef>,
    pub fail_closed: bool,
    pub simulated: bool,
}

/// 提交订单 trait。
pub trait SubmitOrder {
    fn submit_order(
        &mut self,
        request: SubmitOrderRequest,
    ) -> VenueExecResult<MutableActionReceipt>;
}

/// 撤单 trait。
pub trait CancelOrder {
    fn cancel_order(
        &mut self,
        request: CancelOrderRequest,
    ) -> VenueExecResult<MutableActionReceipt>;
}

/// 查询状态 trait。
pub trait QueryActionStatus {
    fn query_action_status(
        &self,
        request: QueryActionStatusRequest,
    ) -> VenueExecResult<MutableActionStatusReport>;
}

/// 转账请求 trait。
pub trait RequestTransfer {
    fn request_transfer(
        &mut self,
        request: TransferRequest,
    ) -> VenueExecResult<MutableActionReceipt>;
}

/// 可变执行适配器组合 trait。
///
/// 中文说明：运行时或执行层只能通过该边界调用可变动作；策略和风控不应依赖
/// 本 crate，也不能把这里的模拟回执当作风控批准。
pub trait MutableExecutionAdapter:
    SubmitOrder + CancelOrder + QueryActionStatus + RequestTransfer
{
}

impl<T> MutableExecutionAdapter for T where
    T: SubmitOrder + CancelOrder + QueryActionStatus + RequestTransfer
{
}

/// 执行计划分发前策略。
///
/// 中文说明：该策略是 `ExecutionPlan` 到 `SubmitOrderRequest` 的最后一道本地门禁。
/// 白名单、notional（名义金额）上限、人工门禁和熔断都必须在调用场所适配器前通过。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionDispatchPolicy {
    allowed_symbols: BTreeSet<String>,
    small_notional_cap: Amount,
    manual_gate_released: bool,
    kill_switch: DispatchKillSwitch,
}

impl ExecutionDispatchPolicy {
    pub fn new(small_notional_cap: Amount) -> Self {
        Self {
            allowed_symbols: BTreeSet::new(),
            small_notional_cap,
            manual_gate_released: false,
            kill_switch: DispatchKillSwitch::default(),
        }
    }

    pub fn allow_symbol(mut self, symbol: impl Into<String>) -> VenueExecResult<Self> {
        let symbol = symbol.into();
        validate_token("symbol", &symbol)?;
        self.allowed_symbols.insert(symbol);
        Ok(self)
    }

    pub fn with_manual_gate_released(mut self, released: bool) -> Self {
        self.manual_gate_released = released;
        self
    }

    pub fn with_kill_switch(mut self, kill_switch: DispatchKillSwitch) -> Self {
        self.kill_switch = kill_switch;
        self
    }

    pub fn allowed_symbols(&self) -> &BTreeSet<String> {
        &self.allowed_symbols
    }

    pub fn small_notional_cap(&self) -> Amount {
        self.small_notional_cap
    }

    pub fn manual_gate_released(&self) -> bool {
        self.manual_gate_released
    }

    pub fn kill_switch(&self) -> &DispatchKillSwitch {
        &self.kill_switch
    }
}

/// 分发熔断配置。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DispatchKillSwitch {
    global: bool,
    symbols: BTreeSet<String>,
    venues: BTreeSet<VenueId>,
    accounts: BTreeSet<AccountId>,
    instruments: BTreeSet<InstrumentId>,
}

impl DispatchKillSwitch {
    pub fn global() -> Self {
        Self {
            global: true,
            ..Self::default()
        }
    }

    pub fn with_symbol(mut self, symbol: impl Into<String>) -> VenueExecResult<Self> {
        let symbol = symbol.into();
        validate_token("symbol", &symbol)?;
        self.symbols.insert(symbol);
        Ok(self)
    }

    pub fn with_venue(mut self, venue_id: VenueId) -> Self {
        self.venues.insert(venue_id);
        self
    }

    pub fn with_account(mut self, account_id: AccountId) -> Self {
        self.accounts.insert(account_id);
        self
    }

    pub fn with_instrument(mut self, instrument_id: InstrumentId) -> Self {
        self.instruments.insert(instrument_id);
        self
    }

    pub fn is_triggered(&self) -> bool {
        self.global
            || !self.symbols.is_empty()
            || !self.venues.is_empty()
            || !self.accounts.is_empty()
            || !self.instruments.is_empty()
    }

    fn blocks_leg(
        &self,
        symbol: &str,
        venue_id: &VenueId,
        account_id: &AccountId,
        instrument_id: &InstrumentId,
    ) -> Option<String> {
        if self.global {
            return Some("global dispatch kill switch is active".to_owned());
        }
        if self.symbols.contains(symbol) {
            return Some(format!("symbol `{symbol}` is kill-switched"));
        }
        if self.venues.contains(venue_id) {
            return Some(format!("venue `{venue_id}` is kill-switched"));
        }
        if self.accounts.contains(account_id) {
            return Some(format!("account `{account_id}` is kill-switched"));
        }
        if self.instruments.contains(instrument_id) {
            return Some(format!("instrument `{instrument_id}` is kill-switched"));
        }
        None
    }
}

/// 已准备分发的订单腿。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedSubmitOrder {
    pub plan_leg_id: String,
    pub venue_symbol: String,
    pub basis_leg_role: Option<String>,
    pub notional_usd: Amount,
    pub request: SubmitOrderRequest,
}

/// 执行计划到订单请求的映射结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionDispatchPlan {
    pub plan_id: String,
    pub requests: Vec<PlannedSubmitOrder>,
}

/// 分发后的结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionDispatchOutcome {
    pub dispatch_plan: ExecutionDispatchPlan,
    pub receipts: Vec<MutableActionReceipt>,
    pub failure: Option<DispatchLegFailure>,
    pub residual_risk: Option<ResidualRisk>,
}

impl ExecutionDispatchOutcome {
    pub fn completed(&self) -> bool {
        self.submission_completed() && !self.requires_private_confirmation()
    }

    pub fn submission_completed(&self) -> bool {
        self.failure.is_none() && self.receipts.len() == self.dispatch_plan.requests.len()
    }

    pub fn requires_private_confirmation(&self) -> bool {
        self.receipts.iter().any(|receipt| {
            receipt.kind == MutableActionKind::SubmitOrder
                && receipt.status == MutableActionStatus::Accepted
                && !receipt.simulated
        })
    }
}

/// 单腿提交失败记录。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchLegFailure {
    pub plan_leg_id: String,
    pub detail: String,
}

/// 一腿成功后一腿失败或未知时的残余风险。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResidualRisk {
    pub severity: &'static str,
    pub detail: String,
}

/// 只执行映射，不调用适配器。
pub fn submit_order_requests_from_execution_plan(
    plan: &ExecutionPlan,
    policy: &ExecutionDispatchPolicy,
    now: UtcTimestamp,
) -> VenueExecResult<Vec<SubmitOrderRequest>> {
    Ok(build_execution_dispatch_plan(plan, policy, now)?
        .requests
        .into_iter()
        .map(|planned| planned.request)
        .collect())
}

/// 构建可审计的分发计划。
pub fn build_execution_dispatch_plan(
    plan: &ExecutionPlan,
    policy: &ExecutionDispatchPolicy,
    now: UtcTimestamp,
) -> VenueExecResult<ExecutionDispatchPlan> {
    validate_plan_dispatch_preconditions(plan, policy, now)?;

    let order_legs = plan
        .legs
        .iter()
        .filter(|leg| dispatches_submit_order(leg))
        .collect::<Vec<_>>();
    if order_legs.is_empty() {
        return Err(VenueExecError::DispatchBlocked {
            reason: format!(
                "plan `{}` has no PlaceOrder or Hedge legs to map",
                plan.plan_id.as_str()
            ),
        });
    }

    let ordered_legs = ordered_dispatch_legs(order_legs)?;
    let requests = ordered_legs
        .into_iter()
        .map(|leg| planned_submit_order_from_leg(plan, leg, policy, now))
        .collect::<VenueExecResult<Vec<_>>>()?;

    Ok(ExecutionDispatchPlan {
        plan_id: plan.plan_id.as_str().to_owned(),
        requests,
    })
}

/// 按计划顺序分发订单；一腿失败后停止后续腿并返回残余风险。
pub fn dispatch_execution_plan<A: SubmitOrder>(
    adapter: &mut A,
    plan: &ExecutionPlan,
    policy: &ExecutionDispatchPolicy,
    now: UtcTimestamp,
) -> VenueExecResult<ExecutionDispatchOutcome> {
    let dispatch_plan = build_execution_dispatch_plan(plan, policy, now)?;
    let mut receipts = Vec::new();

    for planned in &dispatch_plan.requests {
        match adapter.submit_order(planned.request.clone()) {
            Ok(receipt) if receipt.status == MutableActionStatus::Accepted => {
                let requires_private_confirmation = !receipt.simulated;
                receipts.push(receipt);
                if requires_private_confirmation {
                    let detail = format!(
                        "leg `{}` has only a REST submission receipt; wait for private order stream or order query confirmation before treating it as final success",
                        planned.plan_leg_id
                    );
                    return Ok(ExecutionDispatchOutcome {
                        residual_risk: residual_risk_after_failure(
                            &dispatch_plan,
                            &receipts,
                            planned,
                        ),
                        failure: Some(DispatchLegFailure {
                            plan_leg_id: planned.plan_leg_id.clone(),
                            detail,
                        }),
                        dispatch_plan,
                        receipts,
                    });
                }
            }
            Ok(receipt) => {
                let detail = format!(
                    "leg `{}` returned non-accepted status `{}`",
                    planned.plan_leg_id, receipt.status
                );
                receipts.push(receipt);
                return Ok(ExecutionDispatchOutcome {
                    residual_risk: residual_risk_after_failure(&dispatch_plan, &receipts, planned),
                    failure: Some(DispatchLegFailure {
                        plan_leg_id: planned.plan_leg_id.clone(),
                        detail,
                    }),
                    dispatch_plan,
                    receipts,
                });
            }
            Err(error) => {
                let detail = error.to_string();
                return Ok(ExecutionDispatchOutcome {
                    residual_risk: residual_risk_after_failure(&dispatch_plan, &receipts, planned),
                    failure: Some(DispatchLegFailure {
                        plan_leg_id: planned.plan_leg_id.clone(),
                        detail,
                    }),
                    dispatch_plan,
                    receipts,
                });
            }
        }
    }

    Ok(ExecutionDispatchOutcome {
        dispatch_plan,
        receipts,
        failure: None,
        residual_risk: None,
    })
}

fn validate_plan_dispatch_preconditions(
    plan: &ExecutionPlan,
    policy: &ExecutionDispatchPolicy,
    now: UtcTimestamp,
) -> VenueExecResult<()> {
    match &plan.execution_mode {
        ExecutionMode::ReadOnly | ExecutionMode::Simulated => {
            return Err(VenueExecError::DispatchBlocked {
                reason: format!(
                    "execution mode `{}` is not allowed to submit mutable orders",
                    plan.execution_mode.as_str()
                ),
            });
        }
        ExecutionMode::ManualApproval
        | ExecutionMode::GuardedLive
        | ExecutionMode::AutonomousLive => {}
    }

    if policy.allowed_symbols.is_empty() {
        return Err(VenueExecError::DispatchBlocked {
            reason: "symbol whitelist is empty".to_owned(),
        });
    }
    if policy.kill_switch.global {
        return Err(VenueExecError::DispatchBlocked {
            reason: "global dispatch kill switch is active".to_owned(),
        });
    }
    if manual_gate_required(plan) && !policy.manual_gate_released {
        return Err(VenueExecError::DispatchBlocked {
            reason: "manual approval gate has not been released for this plan".to_owned(),
        });
    }
    if plan.timeout_policy.leg_timeout_ms.as_u64() == 0 {
        return Err(VenueExecError::DispatchBlocked {
            reason: "leg timeout must be greater than zero".to_owned(),
        });
    }

    let elapsed_ms = elapsed_since_created_ms(plan, now)?;
    if elapsed_ms > plan.timeout_policy.plan_timeout_ms.as_u64() {
        return Err(VenueExecError::DispatchBlocked {
            reason: format!(
                "plan `{}` timed out before dispatch: elapsed_ms={elapsed_ms}, plan_timeout_ms={}",
                plan.plan_id.as_str(),
                plan.timeout_policy.plan_timeout_ms.as_u64()
            ),
        });
    }
    Ok(())
}

fn manual_gate_required(plan: &ExecutionPlan) -> bool {
    plan.execution_mode == ExecutionMode::ManualApproval
        || plan
            .legs
            .iter()
            .any(|leg| leg.action_type == ExecutionActionType::ManualApprovalGate)
}

fn dispatches_submit_order(leg: &ExecutionLeg) -> bool {
    matches!(
        &leg.action_type,
        ExecutionActionType::PlaceOrder | ExecutionActionType::Hedge
    )
}

fn ordered_dispatch_legs(legs: Vec<&ExecutionLeg>) -> VenueExecResult<Vec<&ExecutionLeg>> {
    if !legs.iter().any(|leg| basis_role(leg).is_some()) {
        return Ok(legs);
    }

    if legs
        .iter()
        .any(|leg| basis_role(leg) == Some(BasisLegRole::Spot))
    {
        return ordered_spot_perp_basis_dispatch_legs(legs);
    }

    ordered_funding_arb_dispatch_legs(legs)
}

fn ordered_spot_perp_basis_dispatch_legs(
    legs: Vec<&ExecutionLeg>,
) -> VenueExecResult<Vec<&ExecutionLeg>> {
    let spot = legs
        .iter()
        .copied()
        .find(|leg| basis_role(leg) == Some(BasisLegRole::Spot));
    let perp = legs
        .iter()
        .copied()
        .find(|leg| basis_role(leg) == Some(BasisLegRole::PerpShort));
    let (Some(spot), Some(perp)) = (spot, perp) else {
        return Err(VenueExecError::DispatchBlocked {
            reason: "basis dispatch requires both spot and perp order legs".to_owned(),
        });
    };
    if legs.len() != 2 {
        return Err(VenueExecError::DispatchBlocked {
            reason: "basis dispatch currently supports exactly two order legs".to_owned(),
        });
    }
    if execution_leg_symbol(spot) != execution_leg_symbol(perp) {
        return Err(VenueExecError::DispatchBlocked {
            reason: "basis spot and perp legs must share the same venue symbol".to_owned(),
        });
    }
    if !matches!(
        spot.side.as_ref(),
        Some(TransitionSide::Buy | TransitionSide::Long)
    ) {
        return Err(VenueExecError::DispatchBlocked {
            reason: "basis spot leg must be buy/long before perp hedge".to_owned(),
        });
    }
    if !matches!(
        perp.side.as_ref(),
        Some(TransitionSide::Sell | TransitionSide::Short)
    ) {
        return Err(VenueExecError::DispatchBlocked {
            reason: "basis perp leg must be sell/short after spot leg".to_owned(),
        });
    }
    Ok(vec![spot, perp])
}

fn ordered_funding_arb_dispatch_legs(
    legs: Vec<&ExecutionLeg>,
) -> VenueExecResult<Vec<&ExecutionLeg>> {
    if legs.len() != 2 {
        return Err(VenueExecError::DispatchBlocked {
            reason: "funding arb dispatch currently supports exactly two perp order legs"
                .to_owned(),
        });
    }
    let long = legs
        .iter()
        .copied()
        .find(|leg| basis_role(leg) == Some(BasisLegRole::PerpLong));
    let short = legs
        .iter()
        .copied()
        .find(|leg| basis_role(leg) == Some(BasisLegRole::PerpShort));
    let (Some(long), Some(short)) = (long, short) else {
        return Err(VenueExecError::DispatchBlocked {
            reason: "funding arb dispatch requires both perp_long and perp_short order legs"
                .to_owned(),
        });
    };
    if execution_leg_symbol(long) != execution_leg_symbol(short) {
        return Err(VenueExecError::DispatchBlocked {
            reason: "funding arb perp legs must share the same venue symbol".to_owned(),
        });
    }
    if !matches!(
        long.side.as_ref(),
        Some(TransitionSide::Buy | TransitionSide::Long)
    ) {
        return Err(VenueExecError::DispatchBlocked {
            reason: "funding arb long leg must be buy/long".to_owned(),
        });
    }
    if !matches!(
        short.side.as_ref(),
        Some(TransitionSide::Sell | TransitionSide::Short)
    ) {
        return Err(VenueExecError::DispatchBlocked {
            reason: "funding arb short leg must be sell/short".to_owned(),
        });
    }
    Ok(vec![long, short])
}

fn planned_submit_order_from_leg(
    plan: &ExecutionPlan,
    leg: &ExecutionLeg,
    policy: &ExecutionDispatchPolicy,
    now: UtcTimestamp,
) -> VenueExecResult<PlannedSubmitOrder> {
    validate_leg_dispatch_state(leg, now)?;
    let venue_id = parse_venue_id(required_leg_field(leg.venue_id.as_ref(), "venue_id")?)?;
    let account_id = parse_account_id(leg.account_id.as_str())?;
    let instrument_id = parse_instrument_id(required_leg_field(
        leg.instrument_id.as_ref(),
        "instrument_id",
    )?)?;
    let symbol = execution_leg_symbol(leg).ok_or_else(|| VenueExecError::DispatchBlocked {
        reason: format!(
            "leg `{}` lacks venue_symbol for whitelist enforcement",
            leg.plan_leg_id.as_str()
        ),
    })?;
    validate_token("symbol", &symbol)?;
    if !policy.allowed_symbols.contains(&symbol) {
        return Err(VenueExecError::DispatchBlocked {
            reason: format!("symbol `{symbol}` is not in the dispatch whitelist"),
        });
    }
    if let Some(reason) =
        policy
            .kill_switch
            .blocks_leg(&symbol, &venue_id, &account_id, &instrument_id)
    {
        return Err(VenueExecError::DispatchBlocked { reason });
    }

    let notional = parse_amount(
        "notional_usd",
        leg.notional_usd
            .as_ref()
            .ok_or_else(|| VenueExecError::DispatchBlocked {
                reason: format!(
                    "leg `{}` lacks notional_usd for small notional cap",
                    leg.plan_leg_id.as_str()
                ),
            })?
            .as_str(),
    )?;
    if notional > policy.small_notional_cap {
        return Err(VenueExecError::DispatchBlocked {
            reason: format!(
                "leg `{}` notional_usd={} exceeds small cap {}",
                leg.plan_leg_id.as_str(),
                notional,
                policy.small_notional_cap
            ),
        });
    }
    if let Some(max_notional) = &plan.constraints.max_notional_usd {
        let max_notional =
            parse_amount("plan.constraints.max_notional_usd", max_notional.as_str())?;
        if notional > max_notional {
            return Err(VenueExecError::DispatchBlocked {
                reason: format!(
                    "leg `{}` notional_usd={} exceeds plan max_notional_usd {}",
                    leg.plan_leg_id.as_str(),
                    notional,
                    max_notional
                ),
            });
        }
    }

    let mut request = SubmitOrderRequest::new(
        venue_id,
        account_id,
        instrument_id,
        order_side_from_transition_side(leg.side.as_ref())?,
        mutable_order_type(leg.order_type.as_ref())?,
        parse_quantity(
            "quantity",
            leg.quantity
                .as_ref()
                .ok_or_else(|| VenueExecError::DispatchBlocked {
                    reason: format!(
                        "leg `{}` lacks quantity for order submission",
                        leg.plan_leg_id.as_str()
                    ),
                })?
                .as_str(),
        )?,
        optional_price(
            "limit_price",
            leg.limit_price.as_ref().map(|value| value.as_str()),
        )?,
        leg.client_order_id
            .as_ref()
            .map(|value| OrderId::new(value.as_str()))
            .transpose()
            .map_err(domain_invalid_request)?,
        IdempotencyKey::new(leg.idempotency_key.as_str())?,
    );
    if let Some(time_in_force) = leg.time_in_force.as_ref() {
        request = request.with_time_in_force(mutable_time_in_force(time_in_force));
    }
    if leg.reduce_only.unwrap_or(false) {
        request = request.with_reduce_only(true);
    }
    request.validate()?;

    Ok(PlannedSubmitOrder {
        plan_leg_id: leg.plan_leg_id.as_str().to_owned(),
        venue_symbol: symbol,
        basis_leg_role: leg
            .basis_leg_role
            .as_ref()
            .map(|role| role.as_str().to_owned()),
        notional_usd: notional,
        request,
    })
}

fn validate_leg_dispatch_state(leg: &ExecutionLeg, now: UtcTimestamp) -> VenueExecResult<()> {
    if !matches!(
        &leg.state,
        ExecutionLegState::Ready | ExecutionLegState::WaitingDependency
    ) {
        return Err(VenueExecError::DispatchBlocked {
            reason: format!(
                "leg `{}` is in state `{}` and cannot be dispatched",
                leg.plan_leg_id.as_str(),
                leg.state.as_str()
            ),
        });
    }
    if let Some(dispatch_after) = &leg.dispatch_after {
        let dispatch_after = parse_utc("dispatch_after", dispatch_after.as_str())?;
        if timestamp_millis(now)? < timestamp_millis(dispatch_after)? {
            return Err(VenueExecError::DispatchBlocked {
                reason: format!(
                    "leg `{}` dispatch_after is still in the future",
                    leg.plan_leg_id.as_str()
                ),
            });
        }
    }
    Ok(())
}

fn residual_risk_after_failure(
    dispatch_plan: &ExecutionDispatchPlan,
    receipts: &[MutableActionReceipt],
    failed: &PlannedSubmitOrder,
) -> Option<ResidualRisk> {
    if receipts.is_empty() {
        return None;
    }
    if is_spot_perp_basis_dispatch(dispatch_plan)
        && failed.basis_leg_role.as_deref() == Some("perp_short")
        && receipts
            .iter()
            .any(|receipt| receipt.status == MutableActionStatus::Accepted)
    {
        return Some(ResidualRisk {
            severity: "RiskCritical",
            detail: "basis spot leg was accepted before the perp leg failed or became unknown; residual long spot exposure requires manual hedge or unwind".to_owned(),
        });
    }
    if is_funding_arb_dispatch(dispatch_plan)
        && receipts
            .iter()
            .any(|receipt| receipt.status == MutableActionStatus::Accepted)
    {
        return Some(ResidualRisk {
            severity: "RiskCritical",
            detail: "funding arb one perp leg was accepted before the opposing perp leg failed or became unknown; residual directional perp exposure requires manual hedge or unwind".to_owned(),
        });
    }
    Some(ResidualRisk {
        severity: "RiskCritical",
        detail: format!(
            "{} accepted leg(s) preceded failure on `{}`; stop remaining legs and reconcile before any retry",
            receipts.len(),
            failed.plan_leg_id
        ),
    })
}

fn is_spot_perp_basis_dispatch(dispatch_plan: &ExecutionDispatchPlan) -> bool {
    dispatch_plan
        .requests
        .iter()
        .any(|request| request.basis_leg_role.as_deref() == Some("spot_buy"))
}

fn is_funding_arb_dispatch(dispatch_plan: &ExecutionDispatchPlan) -> bool {
    let has_long = dispatch_plan
        .requests
        .iter()
        .any(|request| request.basis_leg_role.as_deref() == Some("perp_long"));
    let has_short = dispatch_plan
        .requests
        .iter()
        .any(|request| request.basis_leg_role.as_deref() == Some("perp_short"));
    has_long && has_short
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BasisLegRole {
    Spot,
    PerpLong,
    PerpShort,
}

fn basis_role(leg: &ExecutionLeg) -> Option<BasisLegRole> {
    match leg.basis_leg_role.as_ref().map(|role| role.as_str()) {
        Some("spot_buy") | Some("spot_long") => return Some(BasisLegRole::Spot),
        Some("perp_long") | Some("perp_buy") => return Some(BasisLegRole::PerpLong),
        Some("perp_short") | Some("perp_sell") => return Some(BasisLegRole::PerpShort),
        _ => {}
    }
    let instrument_id = leg.instrument_id.as_ref()?.as_str();
    if instrument_id.ends_with(":SPOT") {
        Some(BasisLegRole::Spot)
    } else if instrument_id.ends_with(":USDM-PERP") || instrument_id.ends_with(":PERP") {
        match leg.side.as_ref() {
            Some(TransitionSide::Buy | TransitionSide::Long) => Some(BasisLegRole::PerpLong),
            Some(TransitionSide::Sell | TransitionSide::Short) => Some(BasisLegRole::PerpShort),
            _ => None,
        }
    } else {
        None
    }
}

fn execution_leg_symbol(leg: &ExecutionLeg) -> Option<String> {
    leg.venue_symbol
        .as_ref()
        .map(|symbol| symbol.as_str().to_owned())
        .or_else(|| symbol_from_instrument(leg.instrument_id.as_ref()?.as_str()))
}

fn symbol_from_instrument(instrument_id: &str) -> Option<String> {
    let mut parts = instrument_id.split(':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("inst"), Some("BINANCE"), Some(symbol), Some(_)) if parts.next().is_none() => {
            Some(symbol.to_owned())
        }
        (Some("inst"), Some("OKX"), Some(symbol), Some(_)) if parts.next().is_none() => {
            Some(symbol.to_owned())
        }
        (Some("inst"), Some(symbol), None, None) => Some(symbol.to_owned()),
        _ => None,
    }
}

fn order_side_from_transition_side(side: Option<&TransitionSide>) -> VenueExecResult<OrderSide> {
    match side {
        Some(TransitionSide::Buy | TransitionSide::Long) => Ok(OrderSide::Buy),
        Some(TransitionSide::Sell | TransitionSide::Short) => Ok(OrderSide::Sell),
        _ => Err(VenueExecError::DispatchBlocked {
            reason: "order leg side must be Buy, Sell, Long, or Short".to_owned(),
        }),
    }
}

fn mutable_order_type(
    order_type: Option<&ExecutionOrderType>,
) -> VenueExecResult<MutableOrderType> {
    match order_type {
        Some(ExecutionOrderType::Market) => Ok(MutableOrderType::Market),
        Some(ExecutionOrderType::Limit) => Ok(MutableOrderType::Limit),
        Some(ExecutionOrderType::PostOnly) => Ok(MutableOrderType::PostOnly),
        None => Err(VenueExecError::DispatchBlocked {
            reason: "order leg lacks order_type".to_owned(),
        }),
    }
}

fn mutable_time_in_force(time_in_force: &ExecutionTimeInForce) -> MutableTimeInForce {
    match time_in_force {
        ExecutionTimeInForce::Gtc => MutableTimeInForce::Gtc,
        ExecutionTimeInForce::Ioc => MutableTimeInForce::Ioc,
        ExecutionTimeInForce::Fok => MutableTimeInForce::Fok,
    }
}

fn required_leg_field<'a>(
    value: Option<&'a arb_contracts::Identifier>,
    field: &'static str,
) -> VenueExecResult<&'a str> {
    value
        .map(|value| value.as_str())
        .ok_or(VenueExecError::InvalidRequest {
            field,
            reason: "execution order leg is missing a required field",
        })
}

fn parse_venue_id(value: &str) -> VenueExecResult<VenueId> {
    VenueId::new(value).map_err(domain_invalid_request)
}

fn parse_account_id(value: &str) -> VenueExecResult<AccountId> {
    AccountId::new(value).map_err(domain_invalid_request)
}

fn parse_instrument_id(value: &str) -> VenueExecResult<InstrumentId> {
    InstrumentId::new(value).map_err(domain_invalid_request)
}

fn parse_quantity(field: &'static str, value: &str) -> VenueExecResult<Quantity> {
    Quantity::new(
        Decimal::from_str(value).map_err(|_| VenueExecError::InvalidRequest {
            field,
            reason: "quantity decimal is invalid",
        })?,
    )
    .map_err(domain_invalid_request)
}

fn parse_amount(field: &'static str, value: &str) -> VenueExecResult<Amount> {
    Amount::new(
        Decimal::from_str(value).map_err(|_| VenueExecError::InvalidRequest {
            field,
            reason: "amount decimal is invalid",
        })?,
    )
    .map_err(domain_invalid_request)
}

fn optional_price(field: &'static str, value: Option<&str>) -> VenueExecResult<Option<Price>> {
    value
        .map(|value| {
            Price::new(
                Decimal::from_str(value).map_err(|_| VenueExecError::InvalidRequest {
                    field,
                    reason: "price decimal is invalid",
                })?,
            )
            .map_err(domain_invalid_request)
        })
        .transpose()
}

fn elapsed_since_created_ms(plan: &ExecutionPlan, now: UtcTimestamp) -> VenueExecResult<u64> {
    let created_at = parse_utc("created_at", plan.created_at.as_str())?;
    let created_ms = timestamp_millis(created_at)?;
    let now_ms = timestamp_millis(now)?;
    if now_ms < created_ms {
        return Err(VenueExecError::DispatchBlocked {
            reason: format!(
                "plan `{}` created_at is after dispatch time",
                plan.plan_id.as_str()
            ),
        });
    }
    u64::try_from(now_ms - created_ms).map_err(|_| VenueExecError::DispatchBlocked {
        reason: "plan dispatch elapsed time overflowed".to_owned(),
    })
}

fn parse_utc(field: &'static str, value: &str) -> VenueExecResult<UtcTimestamp> {
    UtcTimestamp::parse_rfc3339_z(value).map_err(|_| VenueExecError::InvalidRequest {
        field,
        reason: "timestamp must be strict UTC RFC3339 ending with Z",
    })
}

fn timestamp_millis(timestamp: UtcTimestamp) -> VenueExecResult<i128> {
    let seconds = i128::from(timestamp.unix_seconds())
        .checked_mul(1_000)
        .ok_or_else(|| VenueExecError::DispatchBlocked {
            reason: "timestamp seconds overflowed while computing milliseconds".to_owned(),
        })?;
    Ok(seconds + i128::from(timestamp.nanoseconds() / 1_000_000))
}

fn domain_invalid_request(error: arb_domain::DomainError) -> VenueExecError {
    VenueExecError::DispatchBlocked {
        reason: error.to_string(),
    }
}

/// 解析 Binance Spot user data stream 的 `executionReport` 订单更新。
pub fn parse_binance_spot_execution_report_update(
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    let event_type = required_json_field(body, "e")?;
    if event_type != "executionReport" {
        return Err(VenueExecError::InvalidRequest {
            field: "event_type",
            reason: "Binance Spot private order update must be executionReport",
        });
    }
    parse_binance_private_order_fields(
        PrivateOrderMarket::Spot,
        OrderConfirmationSource::PrivateStream,
        venue_id,
        account_id,
        source_event_id.into(),
        body,
    )
}

/// 解析 Binance USD-M Futures user data stream 的 `ORDER_TRADE_UPDATE` 订单更新。
pub fn parse_binance_usdm_order_trade_update(
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    let event_type = required_json_field(body, "e")?;
    if event_type != "ORDER_TRADE_UPDATE" {
        return Err(VenueExecError::InvalidRequest {
            field: "event_type",
            reason: "Binance USD-M private order update must be ORDER_TRADE_UPDATE",
        });
    }
    let order = json_object_field(body, "o").ok_or(VenueExecError::InvalidRequest {
        field: "o",
        reason: "Binance USD-M ORDER_TRADE_UPDATE lacks order payload",
    })?;
    parse_binance_private_order_fields(
        PrivateOrderMarket::UsdmFutures,
        OrderConfirmationSource::PrivateStream,
        venue_id,
        account_id,
        source_event_id.into(),
        order,
    )
}

/// 解析 Binance signed REST 查单响应。
pub fn parse_binance_order_query_confirmation(
    market: PrivateOrderMarket,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    if matches!(
        market,
        PrivateOrderMarket::BybitSpot
            | PrivateOrderMarket::BybitLinear
            | PrivateOrderMarket::OkxSpot
            | PrivateOrderMarket::OkxSwap
            | PrivateOrderMarket::BitgetSpot
            | PrivateOrderMarket::BitgetUsdtFutures
            | PrivateOrderMarket::AsterPerp
            | PrivateOrderMarket::HyperliquidPerp
    ) {
        return Err(VenueExecError::InvalidRequest {
            field: "market",
            reason: "Binance order confirmation requires a Binance private order market",
        });
    }
    parse_binance_private_order_fields(
        market,
        OrderConfirmationSource::OrderQuery,
        venue_id,
        account_id,
        source_event_id.into(),
        body,
    )
}

/// 解析 Aster Futures V3 REST 查单响应。
///
/// 中文说明：Aster V3 的订单字段与 Binance futures 接近，但 venue family 和
/// signer/endpoint 完全不同，因此单独标准化，避免把 Aster 私有确认误标为
/// Binance。
pub fn parse_aster_order_query_confirmation(
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    parse_aster_private_order_fields(
        OrderConfirmationSource::OrderQuery,
        venue_id,
        account_id,
        source_event_id.into(),
        body,
    )
}

/// 解析 Hyperliquid `info` / `orderStatus` 查单响应。
pub fn parse_hyperliquid_order_query_confirmation(
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    let root_status = required_json_field(body, "status")?;
    if root_status == "unknownOid" {
        return Err(VenueExecError::UnknownExternalState {
            venue_id,
            detail: "Hyperliquid orderStatus returned unknownOid".to_owned(),
        });
    }
    if root_status != "order" {
        return Err(VenueExecError::InvalidRequest {
            field: "status",
            reason: "Hyperliquid orderStatus response must have status=order",
        });
    }
    let wrapper = json_object_field(body, "order").ok_or(VenueExecError::InvalidRequest {
        field: "order",
        reason: "Hyperliquid orderStatus response lacks order wrapper",
    })?;
    let order = json_object_field(wrapper, "order").ok_or(VenueExecError::InvalidRequest {
        field: "order.order",
        reason: "Hyperliquid orderStatus response lacks order payload",
    })?;
    let status_text = required_json_field(wrapper, "status")?;
    parse_hyperliquid_private_order_fields(
        OrderConfirmationSource::OrderQuery,
        venue_id,
        account_id,
        source_event_id.into(),
        order,
        &status_text,
        json_field_value(wrapper, "statusTimestamp").as_deref(),
    )
}

/// 解析 Bybit V5 REST 查单响应。
///
/// 中文说明：Bybit 下单 REST 回执仍不能代表最终成交；该函数只把私有查单返回的
/// 订单状态转换为统一确认模型，供运行时在下单后显式确认。
pub fn parse_bybit_order_query_confirmation(
    market: PrivateOrderMarket,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    let ret_code = required_json_field(body, "retCode")?;
    if ret_code != "0" {
        return Err(VenueExecError::UnknownExternalState {
            venue_id,
            detail: format!(
                "Bybit order query returned retCode={ret_code}: {}",
                json_field_value(body, "retMsg").unwrap_or_else(|| "missing retMsg".to_owned())
            ),
        });
    }
    let result = json_object_field(body, "result").ok_or(VenueExecError::InvalidRequest {
        field: "result",
        reason: "Bybit order query response lacks result object",
    })?;
    let category = required_json_field(result, "category")?;
    validate_bybit_order_category(market, &category)?;
    let list = json_array_field(result, "list").ok_or(VenueExecError::InvalidRequest {
        field: "list",
        reason: "Bybit order query response lacks result.list array",
    })?;
    let order = first_json_object_in_array(list).ok_or(VenueExecError::InvalidRequest {
        field: "list",
        reason: "Bybit order query result.list contains no order object",
    })?;
    let fallback_time = json_field_value(body, "time");
    parse_bybit_private_order_fields(
        market,
        OrderConfirmationSource::OrderQuery,
        venue_id,
        account_id,
        source_event_id.into(),
        order,
        fallback_time.as_deref(),
    )
}

/// 解析 Bybit V5 private order stream 的订单更新。
///
/// 中文说明：Bybit 私有订单流仍只作为确认来源，不代表 runtime 可跳过执行门禁。
/// 若事件缺少明确 `category`，则只在 topic 明确携带 `spot` 或 `linear` 时接受。
pub fn parse_bybit_private_order_stream_update(
    market: PrivateOrderMarket,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    let topic = required_json_field(body, "topic")?;
    if !topic.starts_with("order") {
        return Err(VenueExecError::InvalidRequest {
            field: "topic",
            reason: "Bybit private order stream update must use an order topic",
        });
    }
    let data = json_array_field(body, "data").ok_or(VenueExecError::InvalidRequest {
        field: "data",
        reason: "Bybit private order stream update lacks data array",
    })?;
    let order = first_json_object_in_array(data).ok_or(VenueExecError::InvalidRequest {
        field: "data",
        reason: "Bybit private order stream data contains no order object",
    })?;
    let category = json_field_value(order, "category")
        .or_else(|| bybit_category_from_order_topic(&topic).map(str::to_owned))
        .ok_or(VenueExecError::InvalidRequest {
            field: "category",
            reason: "Bybit private order stream update lacks category",
        })?;
    validate_bybit_order_category(market, &category)?;
    let fallback_time =
        json_field_value(body, "creationTime").or_else(|| json_field_value(body, "ts"));
    parse_bybit_private_order_fields(
        market,
        OrderConfirmationSource::PrivateStream,
        venue_id,
        account_id,
        source_event_id.into(),
        order,
        fallback_time.as_deref(),
    )
}

/// 解析 OKX V5 REST 查单响应。
///
/// 中文说明：OKX 下单 REST 回执只代表交易所接收请求；该函数只接受
/// `/api/v5/trade/order` 查单返回的明确订单状态，用于真实下单后的二次确认。
pub fn parse_okx_order_query_confirmation(
    market: PrivateOrderMarket,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    validate_okx_private_market(market)?;
    let code = required_json_field(body, "code")?;
    if code != "0" {
        return Err(VenueExecError::UnknownExternalState {
            venue_id,
            detail: format!(
                "OKX order query returned code={code}: {}",
                json_field_value(body, "msg").unwrap_or_else(|| "missing msg".to_owned())
            ),
        });
    }
    let data = json_array_field(body, "data").ok_or(VenueExecError::InvalidRequest {
        field: "data",
        reason: "OKX order query response lacks data array",
    })?;
    let order = first_json_object_in_array(data).ok_or(VenueExecError::InvalidRequest {
        field: "data",
        reason: "OKX order query data contains no order object",
    })?;
    parse_okx_private_order_fields(
        market,
        OrderConfirmationSource::OrderQuery,
        venue_id,
        account_id,
        source_event_id.into(),
        order,
        None,
    )
}

/// 解析 OKX V5 private `orders` channel 的订单更新。
///
/// 中文说明：OKX 私有订单流只作为确认来源；调用方仍必须先通过执行门禁和真实
/// 签名边界下单，不能把 stream 事件当成下单授权。
pub fn parse_okx_private_order_stream_update(
    market: PrivateOrderMarket,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    validate_okx_private_market(market)?;
    let channel = json_field_value(body, "channel").ok_or(VenueExecError::InvalidRequest {
        field: "channel",
        reason: "OKX private order stream update lacks channel",
    })?;
    if channel != "orders" {
        return Err(VenueExecError::InvalidRequest {
            field: "channel",
            reason: "OKX private order stream update must use orders channel",
        });
    }
    let data = json_array_field(body, "data").ok_or(VenueExecError::InvalidRequest {
        field: "data",
        reason: "OKX private order stream update lacks data array",
    })?;
    let order = first_json_object_in_array(data).ok_or(VenueExecError::InvalidRequest {
        field: "data",
        reason: "OKX private order stream data contains no order object",
    })?;
    let fallback_time = json_field_value(body, "ts");
    parse_okx_private_order_fields(
        market,
        OrderConfirmationSource::PrivateStream,
        venue_id,
        account_id,
        source_event_id.into(),
        order,
        fallback_time.as_deref(),
    )
}

/// 解析 Bitget REST 查单响应。
///
/// 中文说明：Bitget 下单 REST 回执只代表交易所接收请求；该函数只接受
/// `/api/v2/spot/trade/orderInfo` 或 `/api/v2/mix/order/detail` 查单返回的明确
/// 订单状态，用于真实下单后的二次确认。
pub fn parse_bitget_order_query_confirmation(
    market: PrivateOrderMarket,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    validate_bitget_private_market(market)?;
    let code = required_json_field(body, "code")?;
    if code != "00000" {
        return Err(VenueExecError::UnknownExternalState {
            venue_id,
            detail: format!(
                "Bitget order query returned code={code}: {}",
                json_field_value(body, "msg")
                    .or_else(|| json_field_value(body, "message"))
                    .unwrap_or_else(|| "missing msg".to_owned())
            ),
        });
    }
    let data_object = json_object_field(body, "data").or_else(|| {
        json_array_field(body, "data").and_then(|array| first_json_object_in_array(array))
    });
    let order = data_object.ok_or(VenueExecError::InvalidRequest {
        field: "data",
        reason: "Bitget order query response lacks order data",
    })?;
    let fallback_time = json_field_value(body, "requestTime");
    parse_bitget_private_order_fields(
        market,
        OrderConfirmationSource::OrderQuery,
        venue_id,
        account_id,
        source_event_id.into(),
        order,
        fallback_time.as_deref(),
    )
}

/// 解析 Bitget private `orders` channel 的订单更新。
///
/// 中文说明：Bitget 私有订单流只作为确认来源；调用方仍必须先通过执行门禁和
/// 真实签名边界下单，不能把 stream 事件当成下单授权。
pub fn parse_bitget_private_order_stream_update(
    market: PrivateOrderMarket,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    validate_bitget_private_market(market)?;
    let arg = json_object_field(body, "arg").ok_or(VenueExecError::InvalidRequest {
        field: "arg",
        reason: "Bitget private order stream update lacks arg object",
    })?;
    let channel = required_json_field(arg, "channel")?;
    if channel != "orders" {
        return Err(VenueExecError::InvalidRequest {
            field: "channel",
            reason: "Bitget private order stream update must use orders channel",
        });
    }
    let inst_type = required_json_field(arg, "instType")?;
    validate_bitget_order_inst_type(market, &inst_type)?;
    let data = json_array_field(body, "data").ok_or(VenueExecError::InvalidRequest {
        field: "data",
        reason: "Bitget private order stream update lacks data array",
    })?;
    let order = first_json_object_in_array(data).ok_or(VenueExecError::InvalidRequest {
        field: "data",
        reason: "Bitget private order stream data contains no order object",
    })?;
    let fallback_time = json_field_value(body, "ts");
    parse_bitget_private_order_fields(
        market,
        OrderConfirmationSource::PrivateStream,
        venue_id,
        account_id,
        source_event_id.into(),
        order,
        fallback_time.as_deref(),
    )
}

fn parse_binance_private_order_fields(
    market: PrivateOrderMarket,
    source: OrderConfirmationSource,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: String,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    validate_token("source_event_id", &source_event_id)?;
    let symbol = required_json_field(body, "s").or_else(|_| required_json_field(body, "symbol"))?;
    validate_binance_private_symbol(&symbol)?;
    let status_text =
        required_json_field(body, "X").or_else(|_| required_json_field(body, "status"))?;
    let status = binance_order_confirmation_status(&status_text);
    let event_time = binance_event_time(body)?;
    let execution_type = json_field_value(body, "x");
    let exchange_order_id =
        json_field_value(body, "i").or_else(|| json_field_value(body, "orderId"));
    let venue_order_id = exchange_order_id
        .as_ref()
        .map(|order_id| {
            ExternalOrderId::new(format!("binance:{}:order:{order_id}", market.token()))
        })
        .transpose()?;
    let client_order_id = json_field_value(body, "c")
        .or_else(|| json_field_value(body, "clientOrderId"))
        .filter(|value| !value.is_empty())
        .map(OrderId::new)
        .transpose()
        .map_err(domain_invalid_request)?;
    let side = json_field_value(body, "S")
        .or_else(|| json_field_value(body, "side"))
        .map(|side| binance_private_side(&side))
        .transpose()?;
    let cumulative_filled_quantity = json_field_value(body, "z")
        .or_else(|| json_field_value(body, "executedQty"))
        .filter(|value| !value.is_empty())
        .map(|value| parse_quantity("cumulative_filled_quantity", &value))
        .transpose()?;
    let instrument_id = InstrumentId::new(format!(
        "inst:BINANCE:{}:{}",
        symbol,
        market.instrument_suffix()
    ))
    .map_err(domain_invalid_request)?;
    let last_fill = parse_binance_private_fill(
        source,
        &source_event_id,
        &event_time,
        body,
        cumulative_filled_quantity,
    )?;

    Ok(PrivateOrderUpdate {
        source,
        market,
        venue_id,
        account_id,
        instrument_id,
        symbol,
        source_event_id,
        event_time,
        status,
        execution_type,
        side,
        venue_order_id,
        exchange_order_id,
        client_order_id,
        cumulative_filled_quantity,
        last_fill,
    })
}

fn parse_aster_private_order_fields(
    source: OrderConfirmationSource,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: String,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    validate_token("source_event_id", &source_event_id)?;
    let symbol = required_json_field(body, "symbol")?;
    validate_binance_private_symbol(&symbol)?;
    let status_text = required_json_field(body, "status")?;
    let status = binance_order_confirmation_status(&status_text);
    let event_time = aster_event_time(body)?;
    let exchange_order_id = json_field_value(body, "orderId");
    let venue_order_id = exchange_order_id
        .as_ref()
        .map(|order_id| ExternalOrderId::new(format!("aster-perp:order:{order_id}")))
        .transpose()?;
    let client_order_id = json_field_value(body, "clientOrderId")
        .filter(|value| !value.is_empty())
        .map(OrderId::new)
        .transpose()
        .map_err(domain_invalid_request)?;
    let side = json_field_value(body, "side")
        .map(|side| binance_private_side(&side))
        .transpose()?;
    let cumulative_filled_quantity = json_field_value(body, "executedQty")
        .filter(|value| !value.is_empty())
        .map(|value| parse_quantity("cumulative_filled_quantity", &value))
        .transpose()?;
    let instrument_id = InstrumentId::new(format!("inst:ASTER:{symbol}:USDT-FUTURES"))
        .map_err(domain_invalid_request)?;
    let last_fill = parse_binance_private_fill(
        source,
        &source_event_id,
        &event_time,
        body,
        cumulative_filled_quantity,
    )?;

    Ok(PrivateOrderUpdate {
        source,
        market: PrivateOrderMarket::AsterPerp,
        venue_id,
        account_id,
        instrument_id,
        symbol,
        source_event_id,
        event_time,
        status,
        execution_type: json_field_value(body, "origType")
            .or_else(|| json_field_value(body, "type")),
        side,
        venue_order_id,
        exchange_order_id,
        client_order_id,
        cumulative_filled_quantity,
        last_fill,
    })
}

fn parse_hyperliquid_private_order_fields(
    source: OrderConfirmationSource,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: String,
    body: &str,
    status_text: &str,
    fallback_time_ms: Option<&str>,
) -> VenueExecResult<PrivateOrderUpdate> {
    validate_token("source_event_id", &source_event_id)?;
    let coin = required_json_field(body, "coin")?;
    validate_hyperliquid_coin(&coin)?;
    let status = hyperliquid_order_confirmation_status(status_text);
    let event_time = hyperliquid_event_time(body, fallback_time_ms)?;
    let exchange_order_id = json_field_value(body, "oid");
    let venue_order_id = exchange_order_id
        .as_ref()
        .map(|order_id| ExternalOrderId::new(format!("hyperliquid-perp:order:{order_id}")))
        .transpose()?;
    let client_order_id = json_field_value(body, "cloid")
        .filter(|value| !value.is_empty() && value != "null")
        .map(OrderId::new)
        .transpose()
        .map_err(domain_invalid_request)?;
    let side = json_field_value(body, "side")
        .map(|side| hyperliquid_private_side(&side))
        .transpose()?;
    let cumulative_filled_quantity = json_field_value(body, "origSz")
        .filter(|_| status == OrderConfirmationStatus::Filled)
        .map(|value| parse_quantity("cumulative_filled_quantity", &value))
        .transpose()?;
    let runtime_symbol = hyperliquid_runtime_symbol_from_coin(&coin);
    let instrument_id = InstrumentId::new(format!("inst:HYPERLIQUID:{runtime_symbol}:PERP"))
        .map_err(domain_invalid_request)?;

    Ok(PrivateOrderUpdate {
        source,
        market: PrivateOrderMarket::HyperliquidPerp,
        venue_id,
        account_id,
        instrument_id,
        symbol: runtime_symbol,
        source_event_id,
        event_time,
        status,
        execution_type: json_field_value(body, "orderType"),
        side,
        venue_order_id,
        exchange_order_id,
        client_order_id,
        cumulative_filled_quantity,
        last_fill: None,
    })
}

fn hyperliquid_runtime_symbol_from_coin(coin: &str) -> String {
    if coin.ends_with("USDT") {
        coin.to_owned()
    } else {
        format!("{coin}USDT")
    }
}

fn parse_bybit_private_order_fields(
    market: PrivateOrderMarket,
    source: OrderConfirmationSource,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: String,
    body: &str,
    fallback_time_ms: Option<&str>,
) -> VenueExecResult<PrivateOrderUpdate> {
    validate_token("source_event_id", &source_event_id)?;
    let symbol = required_json_field(body, "symbol")?;
    validate_bybit_private_symbol(&symbol)?;
    let status_text = required_json_field(body, "orderStatus")?;
    let status = bybit_order_confirmation_status(&status_text);
    let event_time = bybit_event_time(body, fallback_time_ms)?;
    let exchange_order_id = json_field_value(body, "orderId");
    let venue_order_id = exchange_order_id
        .as_ref()
        .map(|order_id| ExternalOrderId::new(format!("{}:order:{order_id}", market.token())))
        .transpose()?;
    let client_order_id = json_field_value(body, "orderLinkId")
        .filter(|value| !value.is_empty())
        .map(OrderId::new)
        .transpose()
        .map_err(domain_invalid_request)?;
    let side = json_field_value(body, "side")
        .map(|side| bybit_private_side(&side))
        .transpose()?;
    let cumulative_filled_quantity = json_field_value(body, "cumExecQty")
        .filter(|value| !value.is_empty())
        .map(|value| parse_quantity("cumulative_filled_quantity", &value))
        .transpose()?;
    let instrument_id = InstrumentId::new(format!(
        "inst:{}:{}:{}",
        market.venue_family(),
        symbol,
        market.instrument_suffix()
    ))
    .map_err(domain_invalid_request)?;
    let last_fill = parse_bybit_private_fill(
        source,
        &source_event_id,
        &event_time,
        body,
        cumulative_filled_quantity,
    )?;

    Ok(PrivateOrderUpdate {
        source,
        market,
        venue_id,
        account_id,
        instrument_id,
        symbol,
        source_event_id,
        event_time,
        status,
        execution_type: json_field_value(body, "execType"),
        side,
        venue_order_id,
        exchange_order_id,
        client_order_id,
        cumulative_filled_quantity,
        last_fill,
    })
}

fn parse_okx_private_order_fields(
    market: PrivateOrderMarket,
    source: OrderConfirmationSource,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: String,
    body: &str,
    fallback_time_ms: Option<&str>,
) -> VenueExecResult<PrivateOrderUpdate> {
    validate_token("source_event_id", &source_event_id)?;
    validate_okx_private_market(market)?;
    let inst_id = required_json_field(body, "instId")?;
    validate_okx_private_inst_id(market, &inst_id)?;
    let status_text = required_json_field(body, "state")?;
    let status = okx_order_confirmation_status(&status_text);
    let event_time = okx_event_time(body, fallback_time_ms)?;
    let exchange_order_id = json_field_value(body, "ordId").filter(|value| !value.is_empty());
    let venue_order_id = exchange_order_id
        .as_ref()
        .map(|order_id| ExternalOrderId::new(format!("{}:order:{order_id}", market.token())))
        .transpose()?;
    let client_order_id = json_field_value(body, "clOrdId")
        .filter(|value| !value.is_empty())
        .map(OrderId::new)
        .transpose()
        .map_err(domain_invalid_request)?;
    let side = json_field_value(body, "side")
        .map(|side| okx_private_side(&side))
        .transpose()?;
    let cumulative_filled_quantity = json_field_value(body, "accFillSz")
        .filter(|value| !value.is_empty())
        .map(|value| parse_quantity("cumulative_filled_quantity", &value))
        .transpose()?;
    let symbol = okx_symbol_from_inst_id(market, &inst_id)?;
    let instrument_id = InstrumentId::new(format!(
        "inst:{}:{}:{}",
        market.venue_family(),
        inst_id,
        market.instrument_suffix()
    ))
    .map_err(domain_invalid_request)?;
    let last_fill = parse_okx_private_fill(
        source,
        &source_event_id,
        &event_time,
        body,
        cumulative_filled_quantity,
    )?;

    Ok(PrivateOrderUpdate {
        source,
        market,
        venue_id,
        account_id,
        instrument_id,
        symbol,
        source_event_id,
        event_time,
        status,
        execution_type: json_field_value(body, "execType"),
        side,
        venue_order_id,
        exchange_order_id,
        client_order_id,
        cumulative_filled_quantity,
        last_fill,
    })
}

fn parse_bitget_private_order_fields(
    market: PrivateOrderMarket,
    source: OrderConfirmationSource,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: String,
    body: &str,
    fallback_time_ms: Option<&str>,
) -> VenueExecResult<PrivateOrderUpdate> {
    validate_token("source_event_id", &source_event_id)?;
    validate_bitget_private_market(market)?;
    let symbol = json_field_value(body, "symbol")
        .or_else(|| json_field_value(body, "instId"))
        .ok_or(VenueExecError::InvalidRequest {
            field: "symbol",
            reason: "Bitget private order update lacks symbol or instId",
        })?;
    let symbol = bitget_symbol_upper(&symbol)?;
    let status_text = json_field_value(body, "status")
        .or_else(|| json_field_value(body, "state"))
        .ok_or(VenueExecError::InvalidRequest {
            field: "status",
            reason: "Bitget private order update lacks status or state",
        })?;
    let status = bitget_order_confirmation_status(&status_text);
    let event_time = bitget_event_time(body, fallback_time_ms)?;
    let exchange_order_id = json_field_value(body, "orderId").filter(|value| !value.is_empty());
    let venue_order_id = exchange_order_id
        .as_ref()
        .map(|order_id| ExternalOrderId::new(format!("{}:order:{order_id}", market.token())))
        .transpose()?;
    let client_order_id = json_field_value(body, "clientOid")
        .filter(|value| !value.is_empty())
        .map(OrderId::new)
        .transpose()
        .map_err(domain_invalid_request)?;
    let side = json_field_value(body, "side")
        .map(|side| bitget_private_side(&side))
        .transpose()?;
    let cumulative_filled_quantity = json_field_value(body, "accBaseVolume")
        .or_else(|| json_field_value(body, "baseVolume"))
        .filter(|value| !value.is_empty())
        .map(|value| parse_quantity("cumulative_filled_quantity", &value))
        .transpose()?;
    let instrument_id = InstrumentId::new(format!(
        "inst:{}:{}:{}",
        market.venue_family(),
        symbol,
        market.instrument_suffix()
    ))
    .map_err(domain_invalid_request)?;
    let last_fill = parse_bitget_private_fill(
        source,
        &source_event_id,
        &event_time,
        body,
        cumulative_filled_quantity,
    )?;

    Ok(PrivateOrderUpdate {
        source,
        market,
        venue_id,
        account_id,
        instrument_id,
        symbol,
        source_event_id,
        event_time,
        status,
        execution_type: json_field_value(body, "enterPointSource")
            .or_else(|| json_field_value(body, "orderSource")),
        side,
        venue_order_id,
        exchange_order_id,
        client_order_id,
        cumulative_filled_quantity,
        last_fill,
    })
}

fn parse_bybit_private_fill(
    source: OrderConfirmationSource,
    source_event_id: &str,
    event_time: &str,
    body: &str,
    cumulative_filled_quantity: Option<Quantity>,
) -> VenueExecResult<Option<PrivateOrderFillUpdate>> {
    let quantity_text = json_field_value(body, "lastExecQty").or_else(|| {
        if source == OrderConfirmationSource::OrderQuery {
            json_field_value(body, "cumExecQty")
        } else {
            None
        }
    });
    let Some(quantity_text) = quantity_text else {
        return Ok(None);
    };
    if !decimal_text_is_positive(&quantity_text) {
        return Ok(None);
    }
    let price = json_field_value(body, "lastExecPrice")
        .or_else(|| {
            if source == OrderConfirmationSource::OrderQuery {
                json_field_value(body, "avgPrice").or_else(|| json_field_value(body, "price"))
            } else {
                None
            }
        })
        .unwrap_or_else(|| "0".to_owned());
    let fee_amount = json_field_value(body, "cumExecFee")
        .or_else(|| json_field_value(body, "execFee"))
        .filter(|value| !value.is_empty())
        .map(|value| parse_amount("fee_amount", &value))
        .transpose()?;
    let fee_asset_id = json_field_value(body, "feeCurrency")
        .or_else(|| json_field_value(body, "execFeeToken"))
        .filter(|value| !value.is_empty())
        .map(|value| AssetId::new(format!("asset:{value}")))
        .transpose()
        .map_err(domain_invalid_request)?;
    let quantity = parse_quantity("last_fill_quantity", &quantity_text)?;
    if let Some(cumulative) = cumulative_filled_quantity {
        if quantity > cumulative {
            return Err(VenueExecError::InvalidRequest {
                field: "last_fill_quantity",
                reason: "last fill quantity exceeds cumulative filled quantity",
            });
        }
    }
    Ok(Some(PrivateOrderFillUpdate {
        source_event_id: source_event_id.to_owned(),
        timestamp: event_time.to_owned(),
        price,
        quantity: quantity_text,
        fee_asset_id,
        fee_amount,
        trade_id: json_field_value(body, "execId"),
    }))
}

fn parse_binance_private_fill(
    source: OrderConfirmationSource,
    source_event_id: &str,
    event_time: &str,
    body: &str,
    cumulative_filled_quantity: Option<Quantity>,
) -> VenueExecResult<Option<PrivateOrderFillUpdate>> {
    let quantity_text = json_field_value(body, "l")
        .or_else(|| json_field_value(body, "lastFilledQty"))
        .or_else(|| {
            if source == OrderConfirmationSource::OrderQuery {
                json_field_value(body, "executedQty")
            } else {
                None
            }
        });
    let Some(quantity_text) = quantity_text else {
        return Ok(None);
    };
    if !decimal_text_is_positive(&quantity_text) {
        return Ok(None);
    }

    let price = json_field_value(body, "L")
        .or_else(|| json_field_value(body, "lastFilledPrice"))
        .or_else(|| {
            if source == OrderConfirmationSource::OrderQuery {
                json_field_value(body, "avgPrice").or_else(|| json_field_value(body, "price"))
            } else {
                None
            }
        })
        .unwrap_or_else(|| "0".to_owned());
    let fee_amount = json_field_value(body, "n")
        .filter(|value| !value.is_empty())
        .map(|value| parse_amount("commission_amount", &value))
        .transpose()?;
    let fee_asset_id = json_field_value(body, "N")
        .filter(|value| !value.is_empty())
        .map(|value| AssetId::new(format!("asset:{value}")))
        .transpose()
        .map_err(domain_invalid_request)?;
    let quantity = parse_quantity("last_fill_quantity", &quantity_text)?;
    if let Some(cumulative) = cumulative_filled_quantity {
        if quantity > cumulative {
            return Err(VenueExecError::InvalidRequest {
                field: "last_fill_quantity",
                reason: "last fill quantity exceeds cumulative filled quantity",
            });
        }
    }

    Ok(Some(PrivateOrderFillUpdate {
        source_event_id: source_event_id.to_owned(),
        timestamp: event_time.to_owned(),
        price,
        quantity: quantity_text,
        fee_asset_id,
        fee_amount,
        trade_id: json_field_value(body, "t").or_else(|| json_field_value(body, "tradeId")),
    }))
}

fn parse_okx_private_fill(
    source: OrderConfirmationSource,
    source_event_id: &str,
    event_time: &str,
    body: &str,
    cumulative_filled_quantity: Option<Quantity>,
) -> VenueExecResult<Option<PrivateOrderFillUpdate>> {
    let quantity_text = json_field_value(body, "fillSz").or_else(|| {
        if source == OrderConfirmationSource::OrderQuery {
            json_field_value(body, "accFillSz")
        } else {
            None
        }
    });
    let Some(quantity_text) = quantity_text else {
        return Ok(None);
    };
    if !decimal_text_is_positive(&quantity_text) {
        return Ok(None);
    }
    let price = json_field_value(body, "fillPx")
        .or_else(|| {
            if source == OrderConfirmationSource::OrderQuery {
                json_field_value(body, "avgPx").or_else(|| json_field_value(body, "px"))
            } else {
                None
            }
        })
        .unwrap_or_else(|| "0".to_owned());
    let fee_amount = json_field_value(body, "fillFee")
        .or_else(|| json_field_value(body, "fee"))
        .filter(|value| !value.is_empty())
        .map(|value| parse_okx_fee_amount(&value))
        .transpose()?;
    let fee_asset_id = json_field_value(body, "fillFeeCcy")
        .or_else(|| json_field_value(body, "feeCcy"))
        .filter(|value| !value.is_empty())
        .map(|value| AssetId::new(format!("asset:{value}")))
        .transpose()
        .map_err(domain_invalid_request)?;
    let quantity = parse_quantity("last_fill_quantity", &quantity_text)?;
    if let Some(cumulative) = cumulative_filled_quantity {
        if quantity > cumulative {
            return Err(VenueExecError::InvalidRequest {
                field: "last_fill_quantity",
                reason: "last fill quantity exceeds cumulative filled quantity",
            });
        }
    }
    Ok(Some(PrivateOrderFillUpdate {
        source_event_id: source_event_id.to_owned(),
        timestamp: event_time.to_owned(),
        price,
        quantity: quantity_text,
        fee_asset_id,
        fee_amount,
        trade_id: json_field_value(body, "tradeId"),
    }))
}

fn parse_bitget_private_fill(
    source: OrderConfirmationSource,
    source_event_id: &str,
    event_time: &str,
    body: &str,
    cumulative_filled_quantity: Option<Quantity>,
) -> VenueExecResult<Option<PrivateOrderFillUpdate>> {
    let quantity_text = json_field_value(body, "baseVolume").or_else(|| {
        if source == OrderConfirmationSource::OrderQuery {
            json_field_value(body, "accBaseVolume")
        } else {
            None
        }
    });
    let Some(quantity_text) = quantity_text else {
        return Ok(None);
    };
    if !decimal_text_is_positive(&quantity_text) {
        return Ok(None);
    }
    let price = json_field_value(body, "fillPrice")
        .or_else(|| {
            if source == OrderConfirmationSource::OrderQuery {
                json_field_value(body, "priceAvg").or_else(|| json_field_value(body, "price"))
            } else {
                None
            }
        })
        .unwrap_or_else(|| "0".to_owned());
    let fee_amount = json_field_value(body, "fillFee")
        .or_else(|| json_field_value(body, "fee"))
        .filter(|value| !value.is_empty())
        .map(|value| parse_bitget_fee_amount(&value))
        .transpose()?;
    let fee_asset_id = json_field_value(body, "fillFeeCoin")
        .filter(|value| !value.is_empty())
        .map(|value| AssetId::new(format!("asset:{}", value.to_ascii_uppercase())))
        .transpose()
        .map_err(domain_invalid_request)?;
    let quantity = parse_quantity("last_fill_quantity", &quantity_text)?;
    if let Some(cumulative) = cumulative_filled_quantity {
        if quantity > cumulative {
            return Err(VenueExecError::InvalidRequest {
                field: "last_fill_quantity",
                reason: "last fill quantity exceeds cumulative filled quantity",
            });
        }
    }
    Ok(Some(PrivateOrderFillUpdate {
        source_event_id: source_event_id.to_owned(),
        timestamp: event_time.to_owned(),
        price,
        quantity: quantity_text,
        fee_asset_id,
        fee_amount,
        trade_id: json_field_value(body, "tradeId"),
    }))
}

fn required_json_field(body: &str, field: &'static str) -> VenueExecResult<String> {
    json_field_value(body, field).ok_or(VenueExecError::InvalidRequest {
        field,
        reason: "required Binance JSON field is missing",
    })
}

fn json_field_value(body: &str, field: &str) -> Option<String> {
    let rest = json_field_tail(body, field)?;
    if let Some(after_quote) = rest.strip_prefix('"') {
        let end = after_quote.find('"')?;
        return Some(after_quote[..end].to_owned());
    }
    let end = rest
        .find(|byte: char| byte == ',' || byte == '}' || byte.is_ascii_whitespace())
        .unwrap_or(rest.len());
    let value = rest[..end].trim();
    if value.is_empty() || value == "null" {
        None
    } else {
        Some(value.to_owned())
    }
}

fn json_field_tail<'a>(body: &'a str, field: &str) -> Option<&'a str> {
    let pattern = format!("\"{field}\"");
    let mut rest = body;
    loop {
        let index = rest.find(&pattern)?;
        let after_name = rest.get(index + pattern.len()..)?;
        let after_ws = after_name.trim_start();
        if let Some(value) = after_ws.strip_prefix(':') {
            return Some(value.trim_start());
        }
        rest = after_name;
    }
}

fn json_object_field<'a>(body: &'a str, field: &str) -> Option<&'a str> {
    let rest = json_field_tail(body, field)?;
    let start = rest.find('{')?;
    let bytes = rest.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        let byte = *byte;
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return rest.get(start..=index);
                }
            }
            _ => {}
        }
    }
    None
}

fn json_array_field<'a>(body: &'a str, field: &str) -> Option<&'a str> {
    let rest = json_field_tail(body, field)?;
    let start = rest.find('[')?;
    let bytes = rest.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        let byte = *byte;
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return rest.get(start..=index);
                }
            }
            _ => {}
        }
    }
    None
}

fn first_json_object_in_array(array: &str) -> Option<&str> {
    let start = array.find('{')?;
    let bytes = array.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        let byte = *byte;
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return array.get(start..=index);
                }
            }
            _ => {}
        }
    }
    None
}

fn binance_event_time(body: &str) -> VenueExecResult<String> {
    let millis = json_field_value(body, "T")
        .or_else(|| json_field_value(body, "E"))
        .or_else(|| json_field_value(body, "time"))
        .ok_or(VenueExecError::InvalidRequest {
            field: "event_time",
            reason: "Binance private order update lacks event time",
        })?;
    binance_millis_to_utc(&millis)
}

fn aster_event_time(body: &str) -> VenueExecResult<String> {
    let millis = json_field_value(body, "updateTime")
        .or_else(|| json_field_value(body, "time"))
        .ok_or(VenueExecError::InvalidRequest {
            field: "event_time",
            reason: "Aster private order update lacks event time",
        })?;
    binance_millis_to_utc(&millis)
}

fn hyperliquid_event_time(body: &str, fallback_time_ms: Option<&str>) -> VenueExecResult<String> {
    let millis = json_field_value(body, "timestamp")
        .or_else(|| fallback_time_ms.map(str::to_owned))
        .ok_or(VenueExecError::InvalidRequest {
            field: "event_time",
            reason: "Hyperliquid private order update lacks event time",
        })?;
    binance_millis_to_utc(&millis)
}

fn bybit_event_time(body: &str, fallback_time_ms: Option<&str>) -> VenueExecResult<String> {
    let millis = json_field_value(body, "updatedTime")
        .or_else(|| json_field_value(body, "createdTime"))
        .or_else(|| fallback_time_ms.map(str::to_owned))
        .ok_or(VenueExecError::InvalidRequest {
            field: "event_time",
            reason: "Bybit private order update lacks event time",
        })?;
    binance_millis_to_utc(&millis)
}

fn okx_event_time(body: &str, fallback_time_ms: Option<&str>) -> VenueExecResult<String> {
    let millis = json_field_value(body, "uTime")
        .or_else(|| json_field_value(body, "cTime"))
        .or_else(|| fallback_time_ms.map(str::to_owned))
        .ok_or(VenueExecError::InvalidRequest {
            field: "event_time",
            reason: "OKX private order update lacks event time",
        })?;
    okx_millis_to_utc(&millis)
}

fn bitget_event_time(body: &str, fallback_time_ms: Option<&str>) -> VenueExecResult<String> {
    let millis = json_field_value(body, "uTime")
        .or_else(|| json_field_value(body, "fillTime"))
        .or_else(|| json_field_value(body, "cTime"))
        .or_else(|| fallback_time_ms.map(str::to_owned))
        .ok_or(VenueExecError::InvalidRequest {
            field: "event_time",
            reason: "Bitget private order update lacks event time",
        })?;
    bitget_millis_to_utc(&millis)
}

fn binance_millis_to_utc(value: &str) -> VenueExecResult<String> {
    let millis = value
        .parse::<u64>()
        .map_err(|_| VenueExecError::InvalidRequest {
            field: "event_time",
            reason: "Binance event time must be Unix milliseconds",
        })?;
    let seconds = i64::try_from(millis / 1_000).map_err(|_| VenueExecError::InvalidRequest {
        field: "event_time",
        reason: "Binance event time is outside supported UTC range",
    })?;
    let nanos = u32::try_from((millis % 1_000) * 1_000_000).expect("millisecond nanos fit u32");
    Ok(UtcTimestamp::from_unix_parts(seconds, nanos)
        .map_err(domain_invalid_request)?
        .to_string())
}

fn okx_millis_to_utc(value: &str) -> VenueExecResult<String> {
    let millis = value
        .parse::<u64>()
        .map_err(|_| VenueExecError::InvalidRequest {
            field: "event_time",
            reason: "OKX event time must be Unix milliseconds",
        })?;
    let seconds = i64::try_from(millis / 1_000).map_err(|_| VenueExecError::InvalidRequest {
        field: "event_time",
        reason: "OKX event time is outside supported UTC range",
    })?;
    let nanos = u32::try_from((millis % 1_000) * 1_000_000).expect("millisecond nanos fit u32");
    Ok(UtcTimestamp::from_unix_parts(seconds, nanos)
        .map_err(domain_invalid_request)?
        .to_string())
}

fn bitget_millis_to_utc(value: &str) -> VenueExecResult<String> {
    let millis = value
        .parse::<u64>()
        .map_err(|_| VenueExecError::InvalidRequest {
            field: "event_time",
            reason: "Bitget event time must be Unix milliseconds",
        })?;
    let seconds = i64::try_from(millis / 1_000).map_err(|_| VenueExecError::InvalidRequest {
        field: "event_time",
        reason: "Bitget event time is outside supported UTC range",
    })?;
    let nanos = u32::try_from((millis % 1_000) * 1_000_000).expect("millisecond nanos fit u32");
    Ok(UtcTimestamp::from_unix_parts(seconds, nanos)
        .map_err(domain_invalid_request)?
        .to_string())
}

fn bybit_order_confirmation_status(value: &str) -> OrderConfirmationStatus {
    match value {
        "New" | "Created" | "Untriggered" => OrderConfirmationStatus::Acknowledged,
        "PartiallyFilled" => OrderConfirmationStatus::PartiallyFilled,
        "Filled" => OrderConfirmationStatus::Filled,
        "Cancelled" | "Canceled" => OrderConfirmationStatus::Cancelled,
        "Rejected" | "Deactivated" => OrderConfirmationStatus::Rejected,
        "Expired" => OrderConfirmationStatus::Expired,
        _ => OrderConfirmationStatus::Unknown,
    }
}

fn binance_order_confirmation_status(value: &str) -> OrderConfirmationStatus {
    match value {
        "NEW" => OrderConfirmationStatus::Acknowledged,
        "PARTIALLY_FILLED" => OrderConfirmationStatus::PartiallyFilled,
        "FILLED" => OrderConfirmationStatus::Filled,
        "CANCELED" | "CANCELLED" => OrderConfirmationStatus::Cancelled,
        "REJECTED" => OrderConfirmationStatus::Rejected,
        "EXPIRED" | "EXPIRED_IN_MATCH" => OrderConfirmationStatus::Expired,
        _ => OrderConfirmationStatus::Unknown,
    }
}

fn hyperliquid_order_confirmation_status(value: &str) -> OrderConfirmationStatus {
    match value {
        "open" | "triggered" => OrderConfirmationStatus::Acknowledged,
        "filled" => OrderConfirmationStatus::Filled,
        "canceled"
        | "marginCanceled"
        | "vaultWithdrawalCanceled"
        | "openInterestCapCanceled"
        | "selfTradeCanceled"
        | "reduceOnlyCanceled"
        | "siblingFilledCanceled"
        | "delistedCanceled"
        | "liquidatedCanceled"
        | "scheduledCancel" => OrderConfirmationStatus::Cancelled,
        "rejected"
        | "tickRejected"
        | "minTradeNtlRejected"
        | "perpMarginRejected"
        | "reduceOnlyRejected"
        | "badAloPxRejected"
        | "iocCancelRejected"
        | "badTriggerPxRejected"
        | "marketOrderNoLiquidityRejected"
        | "positionIncreaseAtOpenInterestCapRejected"
        | "positionFlipAtOpenInterestCapRejected"
        | "tooAggressiveAtOpenInterestCapRejected"
        | "openInterestIncreaseRejected"
        | "insufficientSpotBalanceRejected"
        | "oracleRejected"
        | "perpMaxPositionRejected" => OrderConfirmationStatus::Rejected,
        _ => OrderConfirmationStatus::Unknown,
    }
}

fn okx_order_confirmation_status(value: &str) -> OrderConfirmationStatus {
    match value {
        "live" | "effective" => OrderConfirmationStatus::Acknowledged,
        "partially_filled" => OrderConfirmationStatus::PartiallyFilled,
        "filled" => OrderConfirmationStatus::Filled,
        "canceled" | "cancelled" | "mmp_canceled" => OrderConfirmationStatus::Cancelled,
        "order_failed" | "failed" => OrderConfirmationStatus::Rejected,
        _ => OrderConfirmationStatus::Unknown,
    }
}

fn bitget_order_confirmation_status(value: &str) -> OrderConfirmationStatus {
    match value {
        "live" | "new" | "init" => OrderConfirmationStatus::Acknowledged,
        "partially_filled" | "partial-fill" => OrderConfirmationStatus::PartiallyFilled,
        "filled" | "full-fill" => OrderConfirmationStatus::Filled,
        "canceled" | "cancelled" => OrderConfirmationStatus::Cancelled,
        "rejected" | "fail" | "order_failed" => OrderConfirmationStatus::Rejected,
        "expired" => OrderConfirmationStatus::Expired,
        _ => OrderConfirmationStatus::Unknown,
    }
}

fn bybit_private_side(value: &str) -> VenueExecResult<OrderSide> {
    match value {
        "Buy" => Ok(OrderSide::Buy),
        "Sell" => Ok(OrderSide::Sell),
        _ => Err(VenueExecError::InvalidRequest {
            field: "side",
            reason: "Bybit order side must be Buy or Sell",
        }),
    }
}

fn bitget_private_side(value: &str) -> VenueExecResult<OrderSide> {
    match value {
        "buy" | "Buy" | "BUY" => Ok(OrderSide::Buy),
        "sell" | "Sell" | "SELL" => Ok(OrderSide::Sell),
        _ => Err(VenueExecError::InvalidRequest {
            field: "side",
            reason: "Bitget order side must be buy or sell",
        }),
    }
}

fn okx_private_side(value: &str) -> VenueExecResult<OrderSide> {
    match value {
        "buy" => Ok(OrderSide::Buy),
        "sell" => Ok(OrderSide::Sell),
        _ => Err(VenueExecError::InvalidRequest {
            field: "side",
            reason: "OKX order side must be buy or sell",
        }),
    }
}

fn binance_private_side(value: &str) -> VenueExecResult<OrderSide> {
    match value {
        "BUY" => Ok(OrderSide::Buy),
        "SELL" => Ok(OrderSide::Sell),
        _ => Err(VenueExecError::InvalidRequest {
            field: "side",
            reason: "Binance order side must be BUY or SELL",
        }),
    }
}

fn hyperliquid_private_side(value: &str) -> VenueExecResult<OrderSide> {
    match value {
        "B" | "buy" | "Buy" => Ok(OrderSide::Buy),
        "A" | "sell" | "Sell" => Ok(OrderSide::Sell),
        _ => Err(VenueExecError::InvalidRequest {
            field: "side",
            reason: "Hyperliquid order side must be B/buy or A/sell",
        }),
    }
}

fn validate_okx_private_market(market: PrivateOrderMarket) -> VenueExecResult<()> {
    match market {
        PrivateOrderMarket::OkxSpot | PrivateOrderMarket::OkxSwap => Ok(()),
        _ => Err(VenueExecError::InvalidRequest {
            field: "market",
            reason: "OKX order confirmation requires an OKX private order market",
        }),
    }
}

fn validate_bitget_private_market(market: PrivateOrderMarket) -> VenueExecResult<()> {
    match market {
        PrivateOrderMarket::BitgetSpot | PrivateOrderMarket::BitgetUsdtFutures => Ok(()),
        _ => Err(VenueExecError::InvalidRequest {
            field: "market",
            reason: "Bitget order confirmation requires a Bitget private order market",
        }),
    }
}

fn validate_bitget_order_inst_type(
    market: PrivateOrderMarket,
    inst_type: &str,
) -> VenueExecResult<()> {
    match (market, inst_type) {
        (PrivateOrderMarket::BitgetSpot, "SPOT")
        | (PrivateOrderMarket::BitgetUsdtFutures, "USDT-FUTURES") => Ok(()),
        (PrivateOrderMarket::BitgetSpot | PrivateOrderMarket::BitgetUsdtFutures, _) => {
            Err(VenueExecError::InvalidRequest {
                field: "instType",
                reason: "Bitget instType does not match configured private order market",
            })
        }
        _ => validate_bitget_private_market(market),
    }
}

fn bitget_symbol_upper(value: &str) -> VenueExecResult<String> {
    if value.is_empty() || value.len() > 32 {
        return Err(VenueExecError::InvalidRequest {
            field: "symbol",
            reason: "Bitget symbol must be 1 to 32 bytes",
        });
    }
    if value
        .bytes()
        .any(|byte| !(byte.is_ascii_alphabetic() || byte.is_ascii_digit()))
    {
        return Err(VenueExecError::InvalidRequest {
            field: "symbol",
            reason: "Bitget symbol must use ASCII letters and digits",
        });
    }
    Ok(value.to_ascii_uppercase())
}

fn parse_bitget_fee_amount(value: &str) -> VenueExecResult<Amount> {
    let normalized = value.strip_prefix('-').unwrap_or(value);
    parse_amount("fee_amount", normalized)
}

fn validate_okx_private_inst_id(market: PrivateOrderMarket, inst_id: &str) -> VenueExecResult<()> {
    validate_okx_inst_id_text(inst_id)?;
    match market {
        PrivateOrderMarket::OkxSpot if !inst_id.ends_with("-SWAP") => Ok(()),
        PrivateOrderMarket::OkxSwap if inst_id.ends_with("-SWAP") => Ok(()),
        PrivateOrderMarket::OkxSpot | PrivateOrderMarket::OkxSwap => {
            Err(VenueExecError::InvalidRequest {
                field: "instId",
                reason: "OKX instId does not match configured private order market",
            })
        }
        _ => validate_okx_private_market(market),
    }
}

fn validate_okx_inst_id_text(value: &str) -> VenueExecResult<()> {
    if value.is_empty() || value.len() > 64 {
        return Err(VenueExecError::InvalidRequest {
            field: "instId",
            reason: "OKX instId must be 1 to 64 bytes",
        });
    }
    if value
        .bytes()
        .any(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-'))
    {
        return Err(VenueExecError::InvalidRequest {
            field: "instId",
            reason: "OKX instId must use uppercase ASCII letters, digits or dash",
        });
    }
    Ok(())
}

fn okx_symbol_from_inst_id(market: PrivateOrderMarket, inst_id: &str) -> VenueExecResult<String> {
    validate_okx_private_inst_id(market, inst_id)?;
    Ok(match market {
        PrivateOrderMarket::OkxSwap => inst_id
            .strip_suffix("-SWAP")
            .expect("swap instId suffix validated")
            .to_owned(),
        PrivateOrderMarket::OkxSpot => inst_id.to_owned(),
        _ => {
            return Err(VenueExecError::InvalidRequest {
                field: "market",
                reason: "OKX symbol derivation requires an OKX private order market",
            })
        }
    })
}

fn parse_okx_fee_amount(value: &str) -> VenueExecResult<Amount> {
    let normalized = value.strip_prefix('-').unwrap_or(value);
    parse_amount("fee_amount", normalized)
}

fn validate_bybit_order_category(
    market: PrivateOrderMarket,
    category: &str,
) -> VenueExecResult<()> {
    match (market, category) {
        (PrivateOrderMarket::BybitSpot, "spot") | (PrivateOrderMarket::BybitLinear, "linear") => {
            Ok(())
        }
        (
            PrivateOrderMarket::Spot
            | PrivateOrderMarket::UsdmFutures
            | PrivateOrderMarket::OkxSpot
            | PrivateOrderMarket::OkxSwap
            | PrivateOrderMarket::BitgetSpot
            | PrivateOrderMarket::BitgetUsdtFutures
            | PrivateOrderMarket::AsterPerp
            | PrivateOrderMarket::HyperliquidPerp,
            _,
        ) => Err(VenueExecError::InvalidRequest {
            field: "market",
            reason: "Bybit order confirmation requires a Bybit private order market",
        }),
        _ => Err(VenueExecError::InvalidRequest {
            field: "category",
            reason: "Bybit order query category does not match configured market",
        }),
    }
}

fn bybit_category_from_order_topic(topic: &str) -> Option<&'static str> {
    if topic == "order.spot" || topic.ends_with(".spot") {
        Some("spot")
    } else if topic == "order.linear" || topic.ends_with(".linear") {
        Some("linear")
    } else {
        None
    }
}

fn validate_bybit_private_symbol(value: &str) -> VenueExecResult<()> {
    if value.is_empty() || value.len() > 32 {
        return Err(VenueExecError::InvalidRequest {
            field: "symbol",
            reason: "Bybit symbol must be 1 to 32 bytes",
        });
    }
    if value
        .bytes()
        .any(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit()))
    {
        return Err(VenueExecError::InvalidRequest {
            field: "symbol",
            reason: "Bybit symbol must use uppercase ASCII letters and digits",
        });
    }
    Ok(())
}

fn validate_hyperliquid_coin(value: &str) -> VenueExecResult<()> {
    if value.is_empty() || value.len() > 64 {
        return Err(VenueExecError::InvalidRequest {
            field: "coin",
            reason: "Hyperliquid coin must be 1 to 64 bytes",
        });
    }
    if value
        .bytes()
        .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'/')))
    {
        return Err(VenueExecError::InvalidRequest {
            field: "coin",
            reason: "Hyperliquid coin contains an unsupported byte",
        });
    }
    Ok(())
}

fn validate_binance_private_symbol(value: &str) -> VenueExecResult<()> {
    if value.is_empty() || value.len() > 32 {
        return Err(VenueExecError::InvalidRequest {
            field: "symbol",
            reason: "Binance symbol must be 1 to 32 bytes",
        });
    }
    if value
        .bytes()
        .any(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit()))
    {
        return Err(VenueExecError::InvalidRequest {
            field: "symbol",
            reason: "Binance symbol must use uppercase ASCII letters and digits",
        });
    }
    Ok(())
}

fn decimal_text_is_positive(value: &str) -> bool {
    value
        .parse::<Decimal>()
        .map(|decimal| !decimal.is_zero() && !decimal.is_negative())
        .unwrap_or(false)
}

/// 内存模拟可变执行适配器。
///
/// 中文说明：该实现只记录模拟动作和回执，不访问网络、不签名、不移动资金。
#[derive(Clone, Debug, Default)]
pub struct SimulatedVenueExecAdapter {
    records_by_key: BTreeMap<IdempotencyKey, SimulatedActionRecord>,
    key_by_action_id: BTreeMap<MutableActionId, IdempotencyKey>,
    executed_counts: BTreeMap<MutableActionKind, u64>,
    next_sequence: u64,
}

impl SimulatedVenueExecAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn recorded_action_count(&self) -> usize {
        self.records_by_key.len()
    }

    pub fn executed_action_count(&self, kind: MutableActionKind) -> u64 {
        self.executed_counts.get(&kind).copied().unwrap_or(0)
    }

    pub fn contains_idempotency_key(&self, key: &IdempotencyKey) -> bool {
        self.records_by_key.contains_key(key)
    }

    fn accept_action(
        &mut self,
        venue_id: VenueId,
        kind: MutableActionKind,
        idempotency_key: IdempotencyKey,
        fingerprint: RequestFingerprint,
        external_ref_kind: ExternalRefKind,
    ) -> VenueExecResult<MutableActionReceipt> {
        if let Some(existing) = self.records_by_key.get(&idempotency_key) {
            if existing.fingerprint != fingerprint {
                return Err(VenueExecError::IdempotencyConflict {
                    idempotency_key,
                    existing_fingerprint: existing.fingerprint.0.clone(),
                    incoming_fingerprint: fingerprint.0,
                });
            }

            let mut duplicate_receipt = existing.receipt.clone();
            duplicate_receipt.duplicate = true;
            return Ok(duplicate_receipt);
        }

        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("simulated mutable action sequence overflowed");
        let action_id =
            MutableActionId::new(format!("sim:{}:{}", kind.as_str(), self.next_sequence))?;
        let external_ref = self.external_ref(external_ref_kind, &action_id)?;
        let receipt = MutableActionReceipt {
            action_id: action_id.clone(),
            kind,
            status: MutableActionStatus::Accepted,
            idempotency_key: idempotency_key.clone(),
            venue_id,
            external_ref,
            duplicate: false,
            simulated: true,
        };

        self.records_by_key.insert(
            idempotency_key.clone(),
            SimulatedActionRecord {
                fingerprint,
                receipt: receipt.clone(),
            },
        );
        self.key_by_action_id.insert(action_id, idempotency_key);
        *self.executed_counts.entry(kind).or_insert(0) += 1;

        Ok(receipt)
    }

    fn external_ref(
        &self,
        kind: ExternalRefKind,
        action_id: &MutableActionId,
    ) -> VenueExecResult<Option<ExternalActionRef>> {
        match kind {
            ExternalRefKind::Order => Ok(Some(ExternalActionRef::Order(ExternalOrderId::new(
                format!("sim-order:{}", self.next_sequence),
            )?))),
            ExternalRefKind::Cancel => Ok(Some(ExternalActionRef::Cancel(action_id.clone()))),
            ExternalRefKind::Transfer => Ok(Some(ExternalActionRef::Transfer(
                ExternalTransferId::new(format!("sim-transfer:{}", self.next_sequence))?,
            ))),
        }
    }

    fn report_for_key(&self, key: &IdempotencyKey) -> MutableActionStatusReport {
        self.records_by_key.get(key).map_or_else(
            || unknown_status_report(None, Some(key.clone())),
            |record| status_report_from_receipt(&record.receipt),
        )
    }
}

impl SubmitOrder for SimulatedVenueExecAdapter {
    fn submit_order(
        &mut self,
        request: SubmitOrderRequest,
    ) -> VenueExecResult<MutableActionReceipt> {
        request.validate()?;
        let fingerprint = request.fingerprint();
        self.accept_action(
            request.venue_id,
            MutableActionKind::SubmitOrder,
            request.idempotency_key,
            fingerprint,
            ExternalRefKind::Order,
        )
    }
}

impl CancelOrder for SimulatedVenueExecAdapter {
    fn cancel_order(
        &mut self,
        request: CancelOrderRequest,
    ) -> VenueExecResult<MutableActionReceipt> {
        let fingerprint = request.fingerprint();
        self.accept_action(
            request.venue_id,
            MutableActionKind::CancelOrder,
            request.idempotency_key,
            fingerprint,
            ExternalRefKind::Cancel,
        )
    }
}

impl QueryActionStatus for SimulatedVenueExecAdapter {
    fn query_action_status(
        &self,
        request: QueryActionStatusRequest,
    ) -> VenueExecResult<MutableActionStatusReport> {
        Ok(match request {
            QueryActionStatusRequest::ByActionId(action_id) => {
                if let Some(key) = self.key_by_action_id.get(&action_id) {
                    self.report_for_key(key)
                } else {
                    unknown_status_report(Some(action_id), None)
                }
            }
            QueryActionStatusRequest::ByIdempotencyKey(key) => self.report_for_key(&key),
        })
    }
}

impl RequestTransfer for SimulatedVenueExecAdapter {
    fn request_transfer(
        &mut self,
        request: TransferRequest,
    ) -> VenueExecResult<MutableActionReceipt> {
        let fingerprint = request.fingerprint();
        self.accept_action(
            request.venue_id,
            MutableActionKind::TransferRequest,
            request.idempotency_key,
            fingerprint,
            ExternalRefKind::Transfer,
        )
    }
}

#[cfg(feature = "live-exec")]
pub mod live {
    use std::collections::BTreeMap;
    use std::fmt;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    use arb_domain::{AccountId, InstrumentId, OrderId, Quantity, VenueId};
    use arb_signing::real::{
        AsterRealSigningProvider, AsterRequestParam, AsterSignedEndpoint, AsterV3SigningInput,
        BinanceHmacSigningInput, BinanceRequestParam, BinanceSignedEndpoint,
        BitgetHmacSigningInput, BitgetRealSigningProvider, BitgetRestMethod, BitgetSignedEndpoint,
        BybitHmacSigningInput, BybitRealSigningProvider, BybitSignedEndpoint,
        BybitSigningPayloadKind, OkxHmacSigningInput, OkxRealSigningProvider, OkxRestMethod,
        OkxSignedEndpoint, RealSigningProvider,
    };
    use arb_signing::{SigningPolicy, SigningPolicyMode, SigningPurpose, SigningRequestId};

    use super::{
        parse_aster_order_query_confirmation, parse_binance_order_query_confirmation,
        parse_bitget_order_query_confirmation, parse_bybit_order_query_confirmation,
        parse_hyperliquid_order_query_confirmation, parse_okx_order_query_confirmation,
        unknown_status_report, CancelOrder, CancelOrderRequest, ConfirmOrderStatus,
        ConfirmOrderStatusRequest, ExternalActionRef, ExternalOrderId, IdempotencyKey,
        MutableActionId, MutableActionKind, MutableActionReceipt, MutableActionStatus,
        MutableActionStatusReport, MutableOrderType, MutableTimeInForce, OrderReference, OrderSide,
        PrivateOrderMarket, PrivateOrderUpdate, QueryActionStatus, QueryActionStatusRequest,
        RequestFingerprint, RequestTransfer, SubmitOrder, SubmitOrderRequest, TransferRequest,
        VenueExecError, VenueExecResult,
    };

    /// Binance Spot 下单、撤单和查单 endpoint。
    pub const BINANCE_SPOT_ORDER_ENDPOINT: &str = "/api/v3/order";
    /// Binance USD-M Futures 下单、撤单和查单 endpoint。
    pub const BINANCE_USDM_ORDER_ENDPOINT: &str = "/fapi/v1/order";
    /// Binance USD-M Futures 调整初始杠杆 endpoint。
    pub const BINANCE_USDM_LEVERAGE_ENDPOINT: &str = "/fapi/v1/leverage";
    /// 默认 Binance signed endpoint 接收窗口。
    pub const DEFAULT_BINANCE_RECV_WINDOW_MS: u64 = 5_000;
    /// 默认所有 live perp 适配器使用的目标杠杆。
    pub const DEFAULT_PERP_TARGET_LEVERAGE: u32 = 1;
    const MAX_BINANCE_RECV_WINDOW_MS: u64 = 60_000;
    const MAX_PERP_TARGET_LEVERAGE: u32 = 125;
    const CURL_STATUS_MARKER: &str = "\n__ARB_BINANCE_HTTP_STATUS__:";

    fn limit_time_in_force(request: &SubmitOrderRequest) -> MutableTimeInForce {
        request.time_in_force.unwrap_or(MutableTimeInForce::Gtc)
    }

    fn validate_venue_quantity_step(step: Quantity) -> VenueExecResult<()> {
        if step.atoms() <= 0 {
            return Err(VenueExecError::InvalidRequest {
                field: "quantity_step",
                reason: "venue quantity step must be greater than zero",
            });
        }
        Ok(())
    }

    fn validate_perp_target_leverage(field: &'static str, leverage: u32) -> VenueExecResult<()> {
        if (1..=MAX_PERP_TARGET_LEVERAGE).contains(&leverage) {
            Ok(())
        } else {
            Err(VenueExecError::InvalidRequest {
                field,
                reason: "perp target leverage must be between 1 and 125",
            })
        }
    }

    fn leverage_signing_request_id(
        adapter: &'static str,
        symbol: &str,
        leverage: u32,
    ) -> VenueExecResult<SigningRequestId> {
        SigningRequestId::new(format!(
            "signing-request/{adapter}/set-leverage/{symbol}/{leverage}"
        ))
        .map_err(signing_error)
    }

    fn order_query_signing_request_id(
        adapter: &'static str,
        source_event_id: &str,
    ) -> VenueExecResult<SigningRequestId> {
        let full = format!("signing-request/{adapter}/query-order/{source_event_id}");
        if full.len() <= 160 {
            return SigningRequestId::new(full).map_err(signing_error);
        }
        SigningRequestId::new(format!(
            "signing-request/{adapter}/query-order/h{:016x}",
            stable_boundary_ref_hash(source_event_id)
        ))
        .map_err(signing_error)
    }

    fn stable_boundary_ref_hash(value: &str) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in value.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    fn format_quantity_at_venue_step(
        quantity: Quantity,
        step: Quantity,
    ) -> VenueExecResult<String> {
        validate_venue_quantity_step(step)?;
        let scale = quantity.scale().max(step.scale());
        let raw = rescale_quantity_atoms(quantity, scale)?;
        let step_raw = rescale_quantity_atoms(step, scale)?;
        if step_raw <= 0 {
            return Err(VenueExecError::InvalidRequest {
                field: "quantity_step",
                reason: "venue quantity step must be greater than zero",
            });
        }
        let floored = raw
            .checked_div(step_raw)
            .and_then(|multiple| multiple.checked_mul(step_raw))
            .ok_or_else(|| VenueExecError::DispatchBlocked {
                reason: "quantity overflowed while applying venue quantity step".to_owned(),
            })?;
        if floored <= 0 {
            return Err(VenueExecError::DispatchBlocked {
                reason: format!("order quantity {quantity} is below venue quantity step {step}"),
            });
        }
        Ok(format_scaled_atoms_trimmed(floored, scale))
    }

    fn rescale_quantity_atoms(quantity: Quantity, target_scale: u32) -> VenueExecResult<i128> {
        if target_scale < quantity.scale() {
            return Err(VenueExecError::DispatchBlocked {
                reason: "quantity target scale is lower than current scale".to_owned(),
            });
        }
        let scale_delta = target_scale - quantity.scale();
        let multiplier =
            checked_pow10_i128(scale_delta).ok_or_else(|| VenueExecError::DispatchBlocked {
                reason: "quantity scale overflowed while applying venue quantity step".to_owned(),
            })?;
        quantity
            .atoms()
            .checked_mul(multiplier)
            .ok_or_else(|| VenueExecError::DispatchBlocked {
                reason: "quantity overflowed while applying venue quantity step".to_owned(),
            })
    }

    fn checked_pow10_i128(exponent: u32) -> Option<i128> {
        let mut value = 1_i128;
        for _ in 0..exponent {
            value = value.checked_mul(10)?;
        }
        Some(value)
    }

    fn format_scaled_atoms_trimmed(atoms: i128, scale: u32) -> String {
        let digits = atoms.unsigned_abs().to_string();
        if scale == 0 {
            return digits;
        }
        let scale = scale as usize;
        let mut value = if digits.len() > scale {
            let split = digits.len() - scale;
            format!("{}.{}", &digits[..split], &digits[split..])
        } else {
            let mut value = String::from("0.");
            for _ in 0..(scale - digits.len()) {
                value.push('0');
            }
            value.push_str(&digits);
            value
        };
        while value.contains('.') && value.ends_with('0') {
            value.pop();
        }
        if value.ends_with('.') {
            value.pop();
        }
        value
    }

    /// Binance 可变执行市场。
    ///
    /// 中文说明：该枚举用于强制把现货和 USD-M 永续执行路径分开，避免把
    /// `inst:...:SPOT` 错发到合约 endpoint，或把 `USDM-PERP` 错发到现货。
    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub enum BinanceExecMarket {
        Spot,
        UsdmFutures,
    }

    impl BinanceExecMarket {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Spot => "Spot",
                Self::UsdmFutures => "UsdmFutures",
            }
        }

        fn token(self) -> &'static str {
            match self {
                Self::Spot => "spot",
                Self::UsdmFutures => "usdm",
            }
        }

        fn order_endpoint(self) -> &'static str {
            match self {
                Self::Spot => BINANCE_SPOT_ORDER_ENDPOINT,
                Self::UsdmFutures => BINANCE_USDM_ORDER_ENDPOINT,
            }
        }

        fn expected_instrument_suffix(self) -> &'static str {
            match self {
                Self::Spot => "SPOT",
                Self::UsdmFutures => "USDM-PERP",
            }
        }
    }

    impl fmt::Display for BinanceExecMarket {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.as_str())
        }
    }

    /// Binance 执行适配器配置。
    ///
    /// 中文说明：配置只保存 endpoint、账户引用、签名策略和接收窗口，不保存
    /// API key、secret key、签名 query 或任何凭证原文。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct BinanceExecConfig {
        market: BinanceExecMarket,
        venue_id: VenueId,
        account_id: AccountId,
        base_url: String,
        recv_window_ms: u64,
        target_leverage: Option<u32>,
        quantity_step_by_symbol: BTreeMap<String, Quantity>,
        signing_policy: SigningPolicy,
    }

    impl BinanceExecConfig {
        pub fn spot(
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            Self::new(
                BinanceExecMarket::Spot,
                venue_id,
                account_id,
                base_url,
                DEFAULT_BINANCE_RECV_WINDOW_MS,
                signing_policy,
            )
        }

        pub fn usdm_futures(
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            Self::new(
                BinanceExecMarket::UsdmFutures,
                venue_id,
                account_id,
                base_url,
                DEFAULT_BINANCE_RECV_WINDOW_MS,
                signing_policy,
            )
        }

        pub fn new(
            market: BinanceExecMarket,
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            recv_window_ms: u64,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            validate_recv_window(recv_window_ms)?;
            Ok(Self {
                market,
                venue_id,
                account_id,
                base_url: normalize_base_url(base_url.into())?,
                recv_window_ms,
                target_leverage: None,
                quantity_step_by_symbol: BTreeMap::new(),
                signing_policy,
            })
        }

        pub fn with_recv_window_ms(mut self, recv_window_ms: u64) -> VenueExecResult<Self> {
            validate_recv_window(recv_window_ms)?;
            self.recv_window_ms = recv_window_ms;
            Ok(self)
        }

        pub fn with_quantity_step(
            mut self,
            symbol: impl Into<String>,
            step: Quantity,
        ) -> VenueExecResult<Self> {
            let symbol = symbol.into();
            validate_binance_symbol(&symbol)?;
            validate_venue_quantity_step(step)?;
            self.quantity_step_by_symbol.insert(symbol, step);
            Ok(self)
        }

        pub fn with_target_leverage(mut self, leverage: u32) -> VenueExecResult<Self> {
            if self.market != BinanceExecMarket::UsdmFutures {
                return Err(VenueExecError::InvalidRequest {
                    field: "target_leverage",
                    reason: "Binance target leverage can only be configured for USD-M futures",
                });
            }
            validate_perp_target_leverage("target_leverage", leverage)?;
            self.target_leverage = Some(leverage);
            Ok(self)
        }

        pub fn market(&self) -> BinanceExecMarket {
            self.market
        }

        pub fn venue_id(&self) -> &VenueId {
            &self.venue_id
        }

        pub fn account_id(&self) -> &AccountId {
            &self.account_id
        }

        pub fn base_url(&self) -> &str {
            &self.base_url
        }

        pub fn recv_window_ms(&self) -> u64 {
            self.recv_window_ms
        }

        pub fn quantity_step(&self, symbol: &str) -> Option<Quantity> {
            self.quantity_step_by_symbol.get(symbol).copied()
        }

        pub fn target_leverage(&self) -> Option<u32> {
            self.target_leverage
        }

        pub fn signing_policy(&self) -> &SigningPolicy {
            &self.signing_policy
        }
    }

    /// Binance signed REST HTTP 方法。
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BinanceExecHttpMethod {
        Get,
        Post,
        Delete,
    }

    impl BinanceExecHttpMethod {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Get => "GET",
                Self::Post => "POST",
                Self::Delete => "DELETE",
            }
        }
    }

    impl fmt::Display for BinanceExecHttpMethod {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.as_str())
        }
    }

    /// 已签名 Binance HTTP 请求。
    ///
    /// 中文说明：transport 只能通过该对象取得发送所需的 API key header 和
    /// signed query。`Debug` 永远脱敏，避免日志输出凭证或签名材料。
    pub struct BinanceSignedRequest<'a> {
        market: BinanceExecMarket,
        method: BinanceExecHttpMethod,
        base_url: &'a str,
        endpoint: &'static str,
        signed_endpoint: &'a BinanceSignedEndpoint,
    }

    impl BinanceSignedRequest<'_> {
        pub fn market(&self) -> BinanceExecMarket {
            self.market
        }

        pub fn method(&self) -> BinanceExecHttpMethod {
            self.method
        }

        pub fn base_url(&self) -> &str {
            self.base_url
        }

        pub fn endpoint(&self) -> &'static str {
            self.endpoint
        }

        pub fn api_key_header_name(&self) -> &'static str {
            self.signed_endpoint.api_key_header_name()
        }

        pub fn api_key_header_value(&self) -> &str {
            self.signed_endpoint.api_key_header_value()
        }

        pub fn signed_query_for_transport(&self) -> &str {
            self.signed_endpoint.signed_query_for_transport()
        }

        pub fn timestamp_millis(&self) -> u64 {
            self.signed_endpoint.timestamp_millis()
        }
    }

    impl fmt::Debug for BinanceSignedRequest<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BinanceSignedRequest")
                .field("market", &self.market)
                .field("method", &self.method)
                .field("base_url", &self.base_url)
                .field("endpoint", &self.endpoint)
                .field("api_key_header_name", &self.api_key_header_name())
                .field("api_key_header_value", &"<redacted>")
                .field("signed_query", &"<redacted>")
                .field("timestamp_millis", &self.timestamp_millis())
                .finish()
        }
    }

    /// Binance transport 返回。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct BinanceExecHttpResponse {
        status_code: u16,
        body: String,
    }

    impl BinanceExecHttpResponse {
        pub fn new(status_code: u16, body: impl Into<String>) -> Self {
            Self {
                status_code,
                body: body.into(),
            }
        }

        pub fn status_code(&self) -> u16 {
            self.status_code
        }

        pub fn body(&self) -> &str {
            &self.body
        }

        pub fn is_success(&self) -> bool {
            (200..=299).contains(&self.status_code)
        }
    }

    /// Binance 可变执行 transport。
    ///
    /// 中文说明：适配器负责风控之后的请求映射、签名和幂等；具体 HTTP/TLS、
    /// 重试、限频和代理由运行时注入的 transport 实现。transport 遇到网络
    /// 断连或不确定提交状态时必须返回 `UnknownExternalState` 类错误。
    pub trait BinanceExecTransport {
        fn send_signed(
            &mut self,
            request: BinanceSignedRequest<'_>,
        ) -> VenueExecResult<BinanceExecHttpResponse>;
    }

    /// 使用系统 `curl` 发送 Binance signed REST 请求的真实 transport。
    ///
    /// 中文说明：该实现把 URL、签名 query 和 API key header 通过 `curl --config -`
    /// 的标准输入传给 curl，避免把凭证材料暴露在进程命令行参数中。网络断连、
    /// TLS 失败或 curl 未返回 HTTP 状态码时按未知外部状态处理。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct BinanceCurlExecTransport {
        connect_timeout_secs: u64,
        max_time_secs: u64,
    }

    impl BinanceCurlExecTransport {
        pub fn new(connect_timeout_secs: u64, max_time_secs: u64) -> VenueExecResult<Self> {
            if connect_timeout_secs == 0 || max_time_secs == 0 {
                return Err(VenueExecError::InvalidRequest {
                    field: "curl_timeout",
                    reason: "curl timeouts must be greater than zero",
                });
            }
            Ok(Self {
                connect_timeout_secs,
                max_time_secs,
            })
        }

        pub fn connect_timeout_secs(&self) -> u64 {
            self.connect_timeout_secs
        }

        pub fn max_time_secs(&self) -> u64 {
            self.max_time_secs
        }
    }

    impl Default for BinanceCurlExecTransport {
        fn default() -> Self {
            Self {
                connect_timeout_secs: 10,
                max_time_secs: 30,
            }
        }
    }

    impl BinanceExecTransport for BinanceCurlExecTransport {
        fn send_signed(
            &mut self,
            request: BinanceSignedRequest<'_>,
        ) -> VenueExecResult<BinanceExecHttpResponse> {
            let venue_id = transport_venue_id(request.market());
            let url = signed_request_url(&request)?;
            let header = format!(
                "{}: {}",
                request.api_key_header_name(),
                request.api_key_header_value()
            );
            let config = format!(
                "url = \"{}\"\nheader = \"{}\"\n",
                curl_config_quote(&url)?,
                curl_config_quote(&header)?
            );
            let mut child = Command::new("curl")
                .arg("--silent")
                .arg("--show-error")
                .arg("--request")
                .arg(request.method().as_str())
                .arg("--connect-timeout")
                .arg(self.connect_timeout_secs.to_string())
                .arg("--max-time")
                .arg(self.max_time_secs.to_string())
                .arg("--write-out")
                .arg(format!("{CURL_STATUS_MARKER}%{{http_code}}"))
                .arg("--config")
                .arg("-")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "failed to start curl for Binance signed REST request".to_owned(),
                })?;

            child
                .stdin
                .as_mut()
                .ok_or_else(|| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "curl stdin is unavailable for Binance signed REST request".to_owned(),
                })?
                .write_all(config.as_bytes())
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "failed to write curl config for Binance signed REST request"
                        .to_owned(),
                })?;

            let output =
                child
                    .wait_with_output()
                    .map_err(|_| VenueExecError::UnknownExternalState {
                        venue_id: venue_id.clone(),
                        detail: "curl transport did not return a Binance signed REST response"
                            .to_owned(),
                    })?;
            if !output.status.success() {
                return Err(VenueExecError::UnknownExternalState {
                    venue_id,
                    detail: "curl transport failed before a reliable HTTP response was available"
                        .to_owned(),
                });
            }

            parse_curl_http_response(&output.stdout, request.market())
        }
    }

    /// Binance Spot 可变执行适配器。
    pub struct BinanceSpotExecAdapter<S, T> {
        inner: BinanceExecAdapterCore<S, T>,
    }

    impl<S, T> BinanceSpotExecAdapter<S, T> {
        pub fn new(config: BinanceExecConfig, signer: S, transport: T) -> VenueExecResult<Self> {
            ensure_config_market(&config, BinanceExecMarket::Spot)?;
            Ok(Self {
                inner: BinanceExecAdapterCore::new(config, signer, transport),
            })
        }

        pub fn config(&self) -> &BinanceExecConfig {
            self.inner.config()
        }

        pub fn transport(&self) -> &T {
            self.inner.transport()
        }

        pub fn transport_mut(&mut self) -> &mut T {
            self.inner.transport_mut()
        }
    }

    impl<S, T> fmt::Debug for BinanceSpotExecAdapter<S, T>
    where
        S: fmt::Debug,
        T: fmt::Debug,
    {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BinanceSpotExecAdapter")
                .field("inner", &self.inner)
                .finish()
        }
    }

    /// Binance USD-M Futures 可变执行适配器。
    pub struct BinanceUsdmExecAdapter<S, T> {
        inner: BinanceExecAdapterCore<S, T>,
    }

    impl<S, T> BinanceUsdmExecAdapter<S, T> {
        pub fn new(config: BinanceExecConfig, signer: S, transport: T) -> VenueExecResult<Self> {
            ensure_config_market(&config, BinanceExecMarket::UsdmFutures)?;
            Ok(Self {
                inner: BinanceExecAdapterCore::new(config, signer, transport),
            })
        }

        pub fn config(&self) -> &BinanceExecConfig {
            self.inner.config()
        }

        pub fn transport(&self) -> &T {
            self.inner.transport()
        }

        pub fn transport_mut(&mut self) -> &mut T {
            self.inner.transport_mut()
        }
    }

    impl<S, T> fmt::Debug for BinanceUsdmExecAdapter<S, T>
    where
        S: fmt::Debug,
        T: fmt::Debug,
    {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BinanceUsdmExecAdapter")
                .field("inner", &self.inner)
                .finish()
        }
    }

    impl<S, T> SubmitOrder for BinanceSpotExecAdapter<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.submit_order(request)
        }
    }

    impl<S, T> CancelOrder for BinanceSpotExecAdapter<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.cancel_order(request)
        }
    }

    impl<S, T> QueryActionStatus for BinanceSpotExecAdapter<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            self.inner.query_action_status(request)
        }
    }

    impl<S, T> ConfirmOrderStatus for BinanceSpotExecAdapter<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.inner.confirm_order_status(request)
        }
    }

    impl<S, T> RequestTransfer for BinanceSpotExecAdapter<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.request_transfer(request)
        }
    }

    impl<S, T> SubmitOrder for BinanceUsdmExecAdapter<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.submit_order(request)
        }
    }

    impl<S, T> CancelOrder for BinanceUsdmExecAdapter<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.cancel_order(request)
        }
    }

    impl<S, T> QueryActionStatus for BinanceUsdmExecAdapter<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            self.inner.query_action_status(request)
        }
    }

    impl<S, T> ConfirmOrderStatus for BinanceUsdmExecAdapter<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.inner.confirm_order_status(request)
        }
    }

    impl<S, T> RequestTransfer for BinanceUsdmExecAdapter<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.request_transfer(request)
        }
    }

    #[derive(Debug)]
    struct BinanceExecAdapterCore<S, T> {
        config: BinanceExecConfig,
        signer: S,
        transport: T,
        records_by_key: BTreeMap<IdempotencyKey, LiveActionRecord>,
        key_by_action_id: BTreeMap<MutableActionId, IdempotencyKey>,
        orders_by_client_id: BTreeMap<OrderId, BinanceKnownOrder>,
        orders_by_external_id: BTreeMap<ExternalOrderId, BinanceKnownOrder>,
        next_sequence: u64,
    }

    impl<S, T> BinanceExecAdapterCore<S, T> {
        fn new(config: BinanceExecConfig, signer: S, transport: T) -> Self {
            Self {
                config,
                signer,
                transport,
                records_by_key: BTreeMap::new(),
                key_by_action_id: BTreeMap::new(),
                orders_by_client_id: BTreeMap::new(),
                orders_by_external_id: BTreeMap::new(),
                next_sequence: 0,
            }
        }

        fn config(&self) -> &BinanceExecConfig {
            &self.config
        }

        fn transport(&self) -> &T {
            &self.transport
        }

        fn transport_mut(&mut self) -> &mut T {
            &mut self.transport
        }

        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            Ok(match request {
                QueryActionStatusRequest::ByActionId(action_id) => {
                    if let Some(key) = self.key_by_action_id.get(&action_id) {
                        self.report_for_key(key)
                    } else {
                        unknown_status_report(Some(action_id), None)
                    }
                }
                QueryActionStatusRequest::ByIdempotencyKey(key) => self.report_for_key(&key),
            })
        }

        fn report_for_key(&self, key: &IdempotencyKey) -> MutableActionStatusReport {
            self.records_by_key.get(key).map_or_else(
                || unknown_status_report(None, Some(key.clone())),
                |record| super::status_report_from_receipt(&record.receipt),
            )
        }

        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.from_account_id)?;
            Err(VenueExecError::InvalidRequest {
                field: "transfer",
                reason: "Binance live transfer is not implemented by this execution adapter",
            })
        }

        fn ensure_request_scope(
            &self,
            venue_id: &VenueId,
            account_id: &AccountId,
        ) -> VenueExecResult<()> {
            if venue_id != &self.config.venue_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "venue_id",
                    reason: "request venue does not match Binance execution adapter config",
                });
            }
            if account_id != &self.config.account_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "account_id",
                    reason: "request account does not match Binance execution adapter config",
                });
            }
            Ok(())
        }
    }

    impl<S, T> BinanceExecAdapterCore<S, T>
    where
        S: RealSigningProvider,
        T: BinanceExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            request.validate()?;
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;

            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }

            let symbol =
                binance_symbol_from_instrument(self.config.market, &request.instrument_id)?;
            self.ensure_target_leverage(&symbol, request.reduce_only)?;
            let params = submit_order_params(&self.config, &symbol, &request)?;
            let action_id = self.next_action_id(MutableActionKind::SubmitOrder)?;
            let signed = self.sign(SigningPurpose::SubmitOrder, &action_id, params)?;
            let response = self.dispatch_signed(
                BinanceExecHttpMethod::Post,
                self.config.market.order_endpoint(),
                &signed,
            )?;
            self.ensure_success(self.config.market.order_endpoint(), &response)?;

            let known_order = known_order_from_response(
                self.config.market,
                &symbol,
                request.client_order_id.clone(),
                response.body(),
                &action_id,
            )?;
            let external_ref = known_order
                .external_order_id
                .clone()
                .map(ExternalActionRef::Order);
            let receipt = MutableActionReceipt {
                action_id,
                kind: MutableActionKind::SubmitOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref,
                duplicate: false,
                simulated: false,
            };

            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            self.record_known_order(known_order);
            Ok(receipt)
        }

        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }

            let known_order = self.lookup_known_order(&request.order_ref).ok_or(
                VenueExecError::InvalidRequest {
                    field: "order_ref",
                    reason: "Binance cancel requires an order previously submitted through this adapter so its symbol is known",
                },
            )?;
            let params = cancel_order_params(&self.config, &request, known_order)?;
            let action_id = self.next_action_id(MutableActionKind::CancelOrder)?;
            let signed = self.sign(SigningPurpose::CancelOrder, &action_id, params)?;
            let response = self.dispatch_signed(
                BinanceExecHttpMethod::Delete,
                self.config.market.order_endpoint(),
                &signed,
            )?;
            self.ensure_success(self.config.market.order_endpoint(), &response)?;

            let receipt = MutableActionReceipt {
                action_id: action_id.clone(),
                kind: MutableActionKind::CancelOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref: Some(ExternalActionRef::Cancel(action_id)),
                duplicate: false,
                simulated: false,
            };
            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            Ok(receipt)
        }

        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let symbol =
                binance_symbol_from_instrument(self.config.market, &request.instrument_id)?;
            let params = query_order_params(&self.config, &symbol, &request.order_ref)?;
            let signing_request_id =
                order_query_signing_request_id("binance-exec", &request.source_event_id)?;
            let signed =
                self.sign_with_request_id(SigningPurpose::QueryOrder, signing_request_id, params)?;
            let response = self.dispatch_signed(
                BinanceExecHttpMethod::Get,
                self.config.market.order_endpoint(),
                &signed,
            )?;
            self.ensure_success(self.config.market.order_endpoint(), &response)?;
            parse_binance_order_query_confirmation(
                private_market_from_exec_market(self.config.market),
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                request.source_event_id,
                response.body(),
            )
        }

        fn ensure_target_leverage(
            &mut self,
            symbol: &str,
            reduce_only: bool,
        ) -> VenueExecResult<()> {
            if reduce_only || self.config.market != BinanceExecMarket::UsdmFutures {
                return Ok(());
            }
            let Some(leverage) = self.config.target_leverage else {
                return Ok(());
            };
            let params = binance_usdm_leverage_params(&self.config, symbol, leverage)?;
            let signing_request_id = leverage_signing_request_id("binance-exec", symbol, leverage)?;
            let signed =
                self.sign_with_request_id(SigningPurpose::SubmitOrder, signing_request_id, params)?;
            let response = self.dispatch_signed(
                BinanceExecHttpMethod::Post,
                BINANCE_USDM_LEVERAGE_ENDPOINT,
                &signed,
            )?;
            self.ensure_success(BINANCE_USDM_LEVERAGE_ENDPOINT, &response)
        }

        fn duplicate_receipt(
            &self,
            idempotency_key: &IdempotencyKey,
            fingerprint: &RequestFingerprint,
        ) -> VenueExecResult<Option<MutableActionReceipt>> {
            let Some(existing) = self.records_by_key.get(idempotency_key) else {
                return Ok(None);
            };
            if existing.fingerprint != *fingerprint {
                return Err(VenueExecError::IdempotencyConflict {
                    idempotency_key: idempotency_key.clone(),
                    existing_fingerprint: existing.fingerprint.0.clone(),
                    incoming_fingerprint: fingerprint.0.clone(),
                });
            }

            let mut receipt = existing.receipt.clone();
            receipt.duplicate = true;
            Ok(Some(receipt))
        }

        fn next_action_id(&mut self, kind: MutableActionKind) -> VenueExecResult<MutableActionId> {
            self.next_sequence = self
                .next_sequence
                .checked_add(1)
                .expect("Binance mutable action sequence overflowed");
            MutableActionId::new(format!(
                "binance:{}:{}:{}",
                self.config.market.token(),
                kind.as_str(),
                self.next_sequence
            ))
        }

        fn sign(
            &self,
            purpose: SigningPurpose,
            action_id: &MutableActionId,
            params: Vec<BinanceRequestParam>,
        ) -> VenueExecResult<BinanceSignedEndpoint> {
            self.sign_with_request_id(
                purpose,
                SigningRequestId::new(format!(
                    "signing-request/binance-exec/{}",
                    action_id.as_str()
                ))
                .map_err(signing_error)?,
                params,
            )
        }

        fn sign_with_request_id(
            &self,
            purpose: SigningPurpose,
            signing_request_id: SigningRequestId,
            params: Vec<BinanceRequestParam>,
        ) -> VenueExecResult<BinanceSignedEndpoint> {
            let input = BinanceHmacSigningInput::new(
                signing_request_id,
                self.config.signing_policy.policy_ref().clone(),
                purpose,
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                params,
            )
            .map_err(signing_error)?;
            self.signer
                .sign_binance_hmac(input, &self.config.signing_policy)
                .map_err(signing_error)
        }

        fn dispatch_signed(
            &mut self,
            method: BinanceExecHttpMethod,
            endpoint: &'static str,
            signed_endpoint: &BinanceSignedEndpoint,
        ) -> VenueExecResult<BinanceExecHttpResponse> {
            let request = BinanceSignedRequest {
                market: self.config.market,
                method,
                base_url: &self.config.base_url,
                endpoint,
                signed_endpoint,
            };
            self.transport.send_signed(request)
        }

        fn ensure_success(
            &self,
            endpoint: &'static str,
            response: &BinanceExecHttpResponse,
        ) -> VenueExecResult<()> {
            if response.is_success() {
                return Ok(());
            }
            Err(VenueExecError::ExternalRejected {
                venue_id: self.config.venue_id.clone(),
                endpoint: endpoint.to_owned(),
                status_code: response.status_code(),
                reason: response_body_snippet(response.body()),
            })
        }

        fn record_action(
            &mut self,
            idempotency_key: IdempotencyKey,
            fingerprint: RequestFingerprint,
            receipt: MutableActionReceipt,
        ) {
            self.key_by_action_id
                .insert(receipt.action_id.clone(), idempotency_key.clone());
            self.records_by_key.insert(
                idempotency_key,
                LiveActionRecord {
                    fingerprint,
                    receipt,
                },
            );
        }

        fn record_known_order(&mut self, known_order: BinanceKnownOrder) {
            if let Some(client_order_id) = known_order.client_order_id.clone() {
                self.orders_by_client_id
                    .insert(client_order_id, known_order.clone());
            }
            if let Some(external_order_id) = known_order.external_order_id.clone() {
                self.orders_by_external_id
                    .insert(external_order_id, known_order);
            }
        }

        fn lookup_known_order(&self, order_ref: &OrderReference) -> Option<&BinanceKnownOrder> {
            match order_ref {
                OrderReference::ClientOrderId(order_id) => self.orders_by_client_id.get(order_id),
                OrderReference::VenueOrderId(order_id) => self.orders_by_external_id.get(order_id),
            }
        }
    }

    fn binance_usdm_leverage_params(
        config: &BinanceExecConfig,
        symbol: &str,
        leverage: u32,
    ) -> VenueExecResult<Vec<BinanceRequestParam>> {
        validate_perp_target_leverage("target_leverage", leverage)?;
        Ok(vec![
            binance_param("symbol", symbol)?,
            binance_param("leverage", leverage.to_string())?,
            binance_param("recvWindow", config.recv_window_ms.to_string())?,
        ])
    }

    #[derive(Clone, Debug)]
    struct LiveActionRecord {
        fingerprint: RequestFingerprint,
        receipt: MutableActionReceipt,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct BinanceKnownOrder {
        symbol: String,
        order_id_param: Option<String>,
        client_order_id: Option<OrderId>,
        external_order_id: Option<ExternalOrderId>,
    }

    fn ensure_config_market(
        config: &BinanceExecConfig,
        expected: BinanceExecMarket,
    ) -> VenueExecResult<()> {
        if config.market == expected {
            Ok(())
        } else {
            Err(VenueExecError::InvalidRequest {
                field: "market",
                reason: "Binance execution adapter received config for a different market",
            })
        }
    }

    fn validate_recv_window(recv_window_ms: u64) -> VenueExecResult<()> {
        if (1..=MAX_BINANCE_RECV_WINDOW_MS).contains(&recv_window_ms) {
            Ok(())
        } else {
            Err(VenueExecError::InvalidRequest {
                field: "recv_window_ms",
                reason: "Binance recvWindow must be between 1 and 60000 milliseconds",
            })
        }
    }

    fn normalize_base_url(value: String) -> VenueExecResult<String> {
        let trimmed = value.trim().trim_end_matches('/').to_owned();
        if trimmed.is_empty() {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Binance base URL cannot be empty",
            });
        }
        if trimmed
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Binance base URL contains a control byte",
            });
        }
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://127.0.0.1")) {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Binance base URL must use https or an explicit localhost test URL",
            });
        }
        Ok(trimmed)
    }

    fn submit_order_params(
        config: &BinanceExecConfig,
        symbol: &str,
        request: &SubmitOrderRequest,
    ) -> VenueExecResult<Vec<BinanceRequestParam>> {
        let quantity = binance_order_quantity(config, symbol, request.quantity)?;
        let mut params = vec![
            binance_param("symbol", symbol)?,
            binance_param("side", binance_side(request.side))?,
        ];
        match (config.market, request.order_type) {
            (_, MutableOrderType::Market) => {
                params.push(binance_param("type", "MARKET")?);
                params.push(binance_param("quantity", &quantity)?);
            }
            (BinanceExecMarket::Spot, MutableOrderType::Limit) => {
                params.push(binance_param("type", "LIMIT")?);
                params.push(binance_param(
                    "timeInForce",
                    binance_time_in_force(limit_time_in_force(request)),
                )?);
                params.push(binance_param("quantity", &quantity)?);
                params.push(binance_param(
                    "price",
                    request
                        .limit_price
                        .expect("validated limit order price")
                        .to_string(),
                )?);
            }
            (BinanceExecMarket::Spot, MutableOrderType::PostOnly) => {
                params.push(binance_param("type", "LIMIT_MAKER")?);
                params.push(binance_param("quantity", &quantity)?);
                params.push(binance_param(
                    "price",
                    request
                        .limit_price
                        .expect("validated post-only order price")
                        .to_string(),
                )?);
            }
            (BinanceExecMarket::UsdmFutures, MutableOrderType::Limit) => {
                params.push(binance_param("type", "LIMIT")?);
                params.push(binance_param(
                    "timeInForce",
                    binance_time_in_force(limit_time_in_force(request)),
                )?);
                params.push(binance_param("quantity", &quantity)?);
                params.push(binance_param(
                    "price",
                    request
                        .limit_price
                        .expect("validated limit order price")
                        .to_string(),
                )?);
            }
            (BinanceExecMarket::UsdmFutures, MutableOrderType::PostOnly) => {
                params.push(binance_param("type", "LIMIT")?);
                params.push(binance_param("timeInForce", "GTX")?);
                params.push(binance_param("quantity", &quantity)?);
                params.push(binance_param(
                    "price",
                    request
                        .limit_price
                        .expect("validated post-only order price")
                        .to_string(),
                )?);
            }
        }
        if config.market == BinanceExecMarket::UsdmFutures {
            if let Some(position_side) = request.position_side {
                params.push(binance_param("positionSide", position_side.as_str())?);
            }
        }
        if request.reduce_only {
            match config.market {
                BinanceExecMarket::Spot => {
                    return Err(VenueExecError::InvalidRequest {
                        field: "reduce_only",
                        reason: "Binance spot orders do not support reduce_only",
                    });
                }
                BinanceExecMarket::UsdmFutures => {
                    params.push(binance_param("reduceOnly", "true")?);
                }
            }
        }
        if let Some(client_order_id) = &request.client_order_id {
            validate_binance_client_order_id(client_order_id.as_str())?;
            params.push(binance_param("newClientOrderId", client_order_id.as_str())?);
        }
        params.push(binance_param(
            "recvWindow",
            config.recv_window_ms.to_string(),
        )?);
        Ok(params)
    }

    fn binance_order_quantity(
        config: &BinanceExecConfig,
        symbol: &str,
        quantity: Quantity,
    ) -> VenueExecResult<String> {
        match config.quantity_step(symbol) {
            Some(step) => format_quantity_at_venue_step(quantity, step),
            None => Ok(quantity.to_string()),
        }
    }

    fn cancel_order_params(
        config: &BinanceExecConfig,
        request: &CancelOrderRequest,
        known_order: &BinanceKnownOrder,
    ) -> VenueExecResult<Vec<BinanceRequestParam>> {
        let mut params = vec![binance_param("symbol", known_order.symbol.as_str())?];
        match &request.order_ref {
            OrderReference::VenueOrderId(_) => {
                if let Some(order_id) = &known_order.order_id_param {
                    params.push(binance_param("orderId", order_id.as_str())?);
                } else if let Some(client_order_id) = &known_order.client_order_id {
                    params.push(binance_param(
                        "origClientOrderId",
                        client_order_id.as_str(),
                    )?);
                } else {
                    return Err(VenueExecError::InvalidRequest {
                        field: "order_ref",
                        reason: "known Binance venue order lacks orderId and client order ID",
                    });
                }
            }
            OrderReference::ClientOrderId(client_order_id) => {
                params.push(binance_param(
                    "origClientOrderId",
                    client_order_id.as_str(),
                )?);
            }
        }
        params.push(binance_param(
            "recvWindow",
            config.recv_window_ms.to_string(),
        )?);
        Ok(params)
    }

    fn query_order_params(
        config: &BinanceExecConfig,
        symbol: &str,
        order_ref: &OrderReference,
    ) -> VenueExecResult<Vec<BinanceRequestParam>> {
        let mut params = vec![binance_param("symbol", symbol)?];
        match order_ref {
            OrderReference::VenueOrderId(order_id) => {
                let raw_order_id = binance_order_id_param_from_external(config.market, order_id)?;
                params.push(binance_param("orderId", raw_order_id)?);
            }
            OrderReference::ClientOrderId(client_order_id) => {
                params.push(binance_param(
                    "origClientOrderId",
                    client_order_id.as_str(),
                )?);
            }
        }
        params.push(binance_param(
            "recvWindow",
            config.recv_window_ms.to_string(),
        )?);
        Ok(params)
    }

    fn binance_order_id_param_from_external(
        market: BinanceExecMarket,
        order_id: &ExternalOrderId,
    ) -> VenueExecResult<&str> {
        let expected_prefix = format!("binance:{}:order:", market.token());
        let value = order_id.as_str();
        let Some(raw_order_id) = value.strip_prefix(&expected_prefix) else {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "Binance venue order ref must come from the same market adapter",
            });
        };
        if raw_order_id.is_empty() || raw_order_id.bytes().any(|byte| !byte.is_ascii_digit()) {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "Binance venue order ref lacks numeric orderId",
            });
        }
        Ok(raw_order_id)
    }

    fn private_market_from_exec_market(market: BinanceExecMarket) -> PrivateOrderMarket {
        match market {
            BinanceExecMarket::Spot => PrivateOrderMarket::Spot,
            BinanceExecMarket::UsdmFutures => PrivateOrderMarket::UsdmFutures,
        }
    }

    fn binance_symbol_from_instrument(
        market: BinanceExecMarket,
        instrument_id: &InstrumentId,
    ) -> VenueExecResult<String> {
        let value = instrument_id.as_str();
        let mut parts = value.split(':');
        let prefix = parts.next();
        let venue = parts.next();
        let symbol = parts.next();
        let suffix = parts.next();
        if parts.next().is_some()
            || prefix != Some("inst")
            || venue != Some("BINANCE")
            || suffix != Some(market.expected_instrument_suffix())
        {
            return Err(VenueExecError::InvalidRequest {
                field: "instrument_id",
                reason: "Binance execution requires instrument IDs shaped as inst:BINANCE:<SYMBOL>:SPOT or inst:BINANCE:<SYMBOL>:USDM-PERP",
            });
        }
        let symbol = symbol.expect("symbol checked above");
        validate_binance_symbol(symbol)?;
        Ok(symbol.to_owned())
    }

    fn validate_binance_symbol(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 32 {
            return Err(VenueExecError::InvalidRequest {
                field: "symbol",
                reason: "Binance symbol must be 1 to 32 bytes",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit()))
        {
            return Err(VenueExecError::InvalidRequest {
                field: "symbol",
                reason: "Binance symbol must use uppercase ASCII letters and digits",
            });
        }
        Ok(())
    }

    fn validate_binance_client_order_id(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 36 {
            return Err(VenueExecError::InvalidRequest {
                field: "client_order_id",
                reason: "Binance client order ID must be 1 to 36 bytes",
            });
        }
        if value.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'/' | b'_' | b'-'))
        }) {
            return Err(VenueExecError::InvalidRequest {
                field: "client_order_id",
                reason: "Binance client order ID contains an unsupported byte",
            });
        }
        Ok(())
    }

    fn known_order_from_response(
        market: BinanceExecMarket,
        symbol: &str,
        client_order_id: Option<OrderId>,
        body: &str,
        action_id: &MutableActionId,
    ) -> VenueExecResult<BinanceKnownOrder> {
        let order_id_param = json_field_value(body, "orderId");
        let response_client_order_id =
            json_field_value(body, "clientOrderId").and_then(|value| OrderId::new(value).ok());
        let client_order_id = client_order_id.or(response_client_order_id);
        let external_order_id = if let Some(order_id) = &order_id_param {
            Some(ExternalOrderId::new(format!(
                "binance:{}:order:{order_id}",
                market.token()
            ))?)
        } else if let Some(client_order_id) = &client_order_id {
            Some(ExternalOrderId::new(format!(
                "binance:{}:client:{}",
                market.token(),
                client_order_id.as_str()
            ))?)
        } else {
            Some(ExternalOrderId::new(format!(
                "binance:{}:action:{}",
                market.token(),
                action_id.as_str()
            ))?)
        };
        Ok(BinanceKnownOrder {
            symbol: symbol.to_owned(),
            order_id_param,
            client_order_id,
            external_order_id,
        })
    }

    fn json_field_value(body: &str, field: &str) -> Option<String> {
        let pattern = format!("\"{field}\"");
        let mut rest = body.get(body.find(&pattern)? + pattern.len()..)?;
        rest = rest.trim_start();
        rest = rest.strip_prefix(':')?.trim_start();
        if let Some(after_quote) = rest.strip_prefix('"') {
            let end = after_quote.find('"')?;
            return Some(after_quote[..end].to_owned());
        }
        let end = rest
            .find(|byte: char| byte == ',' || byte == '}' || byte.is_ascii_whitespace())
            .unwrap_or(rest.len());
        let value = rest[..end].trim();
        (!value.is_empty()).then(|| value.to_owned())
    }

    fn binance_side(side: OrderSide) -> &'static str {
        match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        }
    }

    fn binance_time_in_force(time_in_force: MutableTimeInForce) -> &'static str {
        match time_in_force {
            MutableTimeInForce::Gtc => "GTC",
            MutableTimeInForce::Ioc => "IOC",
            MutableTimeInForce::Fok => "FOK",
        }
    }

    fn binance_param(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> VenueExecResult<BinanceRequestParam> {
        BinanceRequestParam::new(name, value).map_err(signing_error)
    }

    fn signing_error(error: arb_signing::SigningError) -> VenueExecError {
        VenueExecError::SigningFailed {
            reason: error.to_string(),
        }
    }

    fn response_body_snippet(body: &str) -> String {
        const MAX_LEN: usize = 256;
        if body.len() <= MAX_LEN {
            body.to_owned()
        } else {
            format!("{}...", &body[..MAX_LEN])
        }
    }

    fn signed_request_url(request: &BinanceSignedRequest<'_>) -> VenueExecResult<String> {
        let base = request.base_url();
        let endpoint = request.endpoint();
        if endpoint.is_empty() || !endpoint.starts_with('/') {
            return Err(VenueExecError::InvalidRequest {
                field: "endpoint",
                reason: "Binance signed REST endpoint must be an absolute path",
            });
        }
        Ok(format!(
            "{base}{endpoint}?{}",
            request.signed_query_for_transport()
        ))
    }

    fn curl_config_quote(value: &str) -> VenueExecResult<String> {
        let mut escaped = String::with_capacity(value.len());
        for byte in value.bytes() {
            match byte {
                b'\\' => escaped.push_str("\\\\"),
                b'"' => escaped.push_str("\\\""),
                0 | b'\n' | b'\r' => {
                    return Err(VenueExecError::InvalidRequest {
                        field: "curl_config",
                        reason: "curl config value contains an unsupported control byte",
                    });
                }
                byte if byte.is_ascii_control() => {
                    return Err(VenueExecError::InvalidRequest {
                        field: "curl_config",
                        reason: "curl config value contains an unsupported control byte",
                    });
                }
                _ => escaped.push(byte as char),
            }
        }
        Ok(escaped)
    }

    fn append_curl_header_config(config: &mut String, header: &str) -> VenueExecResult<()> {
        config.push_str("header = \"");
        config.push_str(&curl_config_quote(header)?);
        config.push_str("\"\n");
        Ok(())
    }

    fn parse_curl_http_response(
        stdout: &[u8],
        market: BinanceExecMarket,
    ) -> VenueExecResult<BinanceExecHttpResponse> {
        let output = String::from_utf8_lossy(stdout);
        let Some((body, status)) = output.rsplit_once(CURL_STATUS_MARKER) else {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: transport_venue_id(market),
                detail: "curl transport response lacked an HTTP status marker".to_owned(),
            });
        };
        let status_code =
            status
                .trim()
                .parse::<u16>()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: transport_venue_id(market),
                    detail: "curl transport returned a malformed HTTP status".to_owned(),
                })?;
        if status_code == 0 {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: transport_venue_id(market),
                detail: "curl transport did not receive an HTTP response from Binance".to_owned(),
            });
        }
        Ok(BinanceExecHttpResponse::new(status_code, body.to_owned()))
    }

    fn transport_venue_id(market: BinanceExecMarket) -> VenueId {
        let value = match market {
            BinanceExecMarket::Spot => "venue:BINANCE-SPOT",
            BinanceExecMarket::UsdmFutures => "venue:BINANCE-USDM",
        };
        VenueId::new(value).expect("static Binance transport venue ID")
    }

    /// Aster Futures V3 下单、撤单和查单 endpoint。
    pub const ASTER_FUTURES_V3_ORDER_ENDPOINT: &str = "/fapi/v3/order";
    /// Aster Futures 调整初始杠杆 endpoint。
    pub const ASTER_FUTURES_V3_LEVERAGE_ENDPOINT: &str = "/fapi/v1/leverage";
    /// 默认 Aster Futures V3 REST base URL。
    ///
    /// 中文说明：V3 path 当前可通过 `fapi.asterdex.com` 访问；部分出网 IP 访问
    /// `fapi3.asterdex.com` 会先被负载均衡/WAF 拒绝，无法进入 Aster 应用层。
    pub const ASTER_FUTURES_V3_BASE_URL: &str = "https://fapi.asterdex.com";
    const CURL_ASTER_STATUS_MARKER: &str = "\n__ARB_ASTER_HTTP_STATUS__:";
    const ASTER_SIGNED_REST_USER_AGENT: &str = "easy-arb-runtime/aster-signed-rest";

    /// Aster signed REST HTTP 方法。
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum AsterExecHttpMethod {
        Get,
        Post,
        Delete,
    }

    impl AsterExecHttpMethod {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Get => "GET",
                Self::Post => "POST",
                Self::Delete => "DELETE",
            }
        }
    }

    impl fmt::Display for AsterExecHttpMethod {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.as_str())
        }
    }

    /// Aster Futures V3 执行适配器配置。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct AsterExecConfig {
        venue_id: VenueId,
        account_id: AccountId,
        base_url: String,
        user: Option<String>,
        signer: String,
        target_leverage: Option<u32>,
        signing_policy: SigningPolicy,
    }

    impl AsterExecConfig {
        pub fn perp(
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            user: Option<String>,
            signer: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            let signer = signer.into();
            validate_ethereum_address_text("aster_signer", &signer)?;
            if let Some(user) = &user {
                validate_ethereum_address_text("aster_user", user)?;
            }
            Ok(Self {
                venue_id,
                account_id,
                base_url: normalize_aster_base_url(base_url.into())?,
                user,
                signer,
                target_leverage: None,
                signing_policy,
            })
        }

        pub fn with_target_leverage(mut self, leverage: u32) -> VenueExecResult<Self> {
            validate_perp_target_leverage("target_leverage", leverage)?;
            self.target_leverage = Some(leverage);
            Ok(self)
        }

        pub fn venue_id(&self) -> &VenueId {
            &self.venue_id
        }

        pub fn account_id(&self) -> &AccountId {
            &self.account_id
        }

        pub fn base_url(&self) -> &str {
            &self.base_url
        }

        pub fn user(&self) -> Option<&str> {
            self.user.as_deref()
        }

        pub fn signer(&self) -> &str {
            &self.signer
        }

        pub fn target_leverage(&self) -> Option<u32> {
            self.target_leverage
        }

        pub fn signing_policy(&self) -> &SigningPolicy {
            &self.signing_policy
        }
    }

    pub struct AsterSignedRequest<'a> {
        method: AsterExecHttpMethod,
        base_url: &'a str,
        endpoint: &'static str,
        signed_endpoint: &'a AsterSignedEndpoint,
    }

    impl AsterSignedRequest<'_> {
        pub fn method(&self) -> AsterExecHttpMethod {
            self.method
        }

        pub fn base_url(&self) -> &str {
            self.base_url
        }

        pub fn endpoint(&self) -> &'static str {
            self.endpoint
        }

        pub fn signed_query_for_transport(&self) -> &str {
            self.signed_endpoint.signed_query_for_transport()
        }
    }

    impl fmt::Debug for AsterSignedRequest<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("AsterSignedRequest")
                .field("method", &self.method)
                .field("base_url", &self.base_url)
                .field("endpoint", &self.endpoint)
                .field("signed_query", &"<redacted>")
                .finish()
        }
    }

    /// Aster transport 返回。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct AsterExecHttpResponse {
        status_code: u16,
        body: String,
    }

    impl AsterExecHttpResponse {
        pub fn new(status_code: u16, body: impl Into<String>) -> Self {
            Self {
                status_code,
                body: body.into(),
            }
        }

        pub fn status_code(&self) -> u16 {
            self.status_code
        }

        pub fn body(&self) -> &str {
            &self.body
        }

        pub fn is_success(&self) -> bool {
            (200..=299).contains(&self.status_code)
        }
    }

    pub trait AsterExecTransport {
        fn send_signed(
            &mut self,
            request: AsterSignedRequest<'_>,
        ) -> VenueExecResult<AsterExecHttpResponse>;
    }

    /// 使用系统 `curl` 发送 Aster signed REST 请求的真实 transport。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct AsterCurlExecTransport {
        connect_timeout_secs: u64,
        max_time_secs: u64,
    }

    impl AsterCurlExecTransport {
        pub fn new(connect_timeout_secs: u64, max_time_secs: u64) -> VenueExecResult<Self> {
            if connect_timeout_secs == 0 || max_time_secs == 0 {
                return Err(VenueExecError::InvalidRequest {
                    field: "curl_timeout",
                    reason: "curl timeouts must be greater than zero",
                });
            }
            Ok(Self {
                connect_timeout_secs,
                max_time_secs,
            })
        }
    }

    impl Default for AsterCurlExecTransport {
        fn default() -> Self {
            Self {
                connect_timeout_secs: 10,
                max_time_secs: 30,
            }
        }
    }

    impl AsterExecTransport for AsterCurlExecTransport {
        fn send_signed(
            &mut self,
            request: AsterSignedRequest<'_>,
        ) -> VenueExecResult<AsterExecHttpResponse> {
            let url = aster_signed_request_url(&request)?;
            let mut config = format!("url = \"{}\"\n", curl_config_quote(&url)?);
            append_curl_header_config(
                &mut config,
                "Content-Type: application/x-www-form-urlencoded",
            )?;
            append_curl_header_config(&mut config, "Accept: application/json")?;
            append_curl_header_config(
                &mut config,
                &format!("User-Agent: {ASTER_SIGNED_REST_USER_AGENT}"),
            )?;
            let mut child = Command::new("curl")
                .arg("--silent")
                .arg("--show-error")
                .arg("--request")
                .arg(request.method().as_str())
                .arg("--connect-timeout")
                .arg(self.connect_timeout_secs.to_string())
                .arg("--max-time")
                .arg(self.max_time_secs.to_string())
                .arg("--write-out")
                .arg(format!("{CURL_ASTER_STATUS_MARKER}%{{http_code}}"))
                .arg("--config")
                .arg("-")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: aster_transport_venue_id(),
                    detail: "failed to start curl for Aster signed REST request".to_owned(),
                })?;

            child
                .stdin
                .as_mut()
                .ok_or_else(|| VenueExecError::UnknownExternalState {
                    venue_id: aster_transport_venue_id(),
                    detail: "curl stdin is unavailable for Aster signed REST request".to_owned(),
                })?
                .write_all(config.as_bytes())
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: aster_transport_venue_id(),
                    detail: "failed to write curl config for Aster signed REST request".to_owned(),
                })?;

            let output =
                child
                    .wait_with_output()
                    .map_err(|_| VenueExecError::UnknownExternalState {
                        venue_id: aster_transport_venue_id(),
                        detail: "curl transport did not return an Aster signed REST response"
                            .to_owned(),
                    })?;
            if !output.status.success() {
                return Err(VenueExecError::UnknownExternalState {
                    venue_id: aster_transport_venue_id(),
                    detail: "curl transport failed before a reliable HTTP response was available"
                        .to_owned(),
                });
            }
            parse_aster_curl_http_response(&output.stdout)
        }
    }

    /// Aster perp 可变执行适配器。
    pub struct AsterPerpExecAdapter<S, T> {
        inner: AsterExecAdapterCore<S, T>,
    }

    impl<S, T> AsterPerpExecAdapter<S, T> {
        pub fn new(config: AsterExecConfig, signer: S, transport: T) -> VenueExecResult<Self> {
            Ok(Self {
                inner: AsterExecAdapterCore::new(config, signer, transport),
            })
        }

        pub fn config(&self) -> &AsterExecConfig {
            self.inner.config()
        }

        pub fn transport(&self) -> &T {
            self.inner.transport()
        }

        pub fn transport_mut(&mut self) -> &mut T {
            self.inner.transport_mut()
        }
    }

    impl<S, T> fmt::Debug for AsterPerpExecAdapter<S, T>
    where
        S: fmt::Debug,
        T: fmt::Debug,
    {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("AsterPerpExecAdapter")
                .field("inner", &self.inner)
                .finish()
        }
    }

    impl<S, T> SubmitOrder for AsterPerpExecAdapter<S, T>
    where
        S: AsterRealSigningProvider,
        T: AsterExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.submit_order(request)
        }
    }

    impl<S, T> CancelOrder for AsterPerpExecAdapter<S, T>
    where
        S: AsterRealSigningProvider,
        T: AsterExecTransport,
    {
        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.cancel_order(request)
        }
    }

    impl<S, T> QueryActionStatus for AsterPerpExecAdapter<S, T>
    where
        S: AsterRealSigningProvider,
        T: AsterExecTransport,
    {
        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            self.inner.query_action_status(request)
        }
    }

    impl<S, T> ConfirmOrderStatus for AsterPerpExecAdapter<S, T>
    where
        S: AsterRealSigningProvider,
        T: AsterExecTransport,
    {
        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.inner.confirm_order_status(request)
        }
    }

    impl<S, T> RequestTransfer for AsterPerpExecAdapter<S, T>
    where
        S: AsterRealSigningProvider,
        T: AsterExecTransport,
    {
        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.request_transfer(request)
        }
    }

    #[derive(Debug)]
    struct AsterExecAdapterCore<S, T> {
        config: AsterExecConfig,
        signer: S,
        transport: T,
        records_by_key: BTreeMap<IdempotencyKey, LiveActionRecord>,
        key_by_action_id: BTreeMap<MutableActionId, IdempotencyKey>,
        orders_by_client_id: BTreeMap<OrderId, AsterKnownOrder>,
        orders_by_external_id: BTreeMap<ExternalOrderId, AsterKnownOrder>,
        next_sequence: u64,
    }

    impl<S, T> AsterExecAdapterCore<S, T> {
        fn new(config: AsterExecConfig, signer: S, transport: T) -> Self {
            Self {
                config,
                signer,
                transport,
                records_by_key: BTreeMap::new(),
                key_by_action_id: BTreeMap::new(),
                orders_by_client_id: BTreeMap::new(),
                orders_by_external_id: BTreeMap::new(),
                next_sequence: 0,
            }
        }

        fn config(&self) -> &AsterExecConfig {
            &self.config
        }

        fn transport(&self) -> &T {
            &self.transport
        }

        fn transport_mut(&mut self) -> &mut T {
            &mut self.transport
        }

        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            Ok(match request {
                QueryActionStatusRequest::ByActionId(action_id) => {
                    if let Some(key) = self.key_by_action_id.get(&action_id) {
                        self.report_for_key(key)
                    } else {
                        unknown_status_report(Some(action_id), None)
                    }
                }
                QueryActionStatusRequest::ByIdempotencyKey(key) => self.report_for_key(&key),
            })
        }

        fn report_for_key(&self, key: &IdempotencyKey) -> MutableActionStatusReport {
            self.records_by_key.get(key).map_or_else(
                || unknown_status_report(None, Some(key.clone())),
                |record| super::status_report_from_receipt(&record.receipt),
            )
        }

        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.from_account_id)?;
            Err(VenueExecError::InvalidRequest {
                field: "transfer",
                reason: "Aster live transfer is not implemented by this execution adapter",
            })
        }

        fn ensure_request_scope(
            &self,
            venue_id: &VenueId,
            account_id: &AccountId,
        ) -> VenueExecResult<()> {
            if venue_id != &self.config.venue_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "venue_id",
                    reason: "request venue does not match Aster execution adapter config",
                });
            }
            if account_id != &self.config.account_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "account_id",
                    reason: "request account does not match Aster execution adapter config",
                });
            }
            Ok(())
        }
    }

    impl<S, T> AsterExecAdapterCore<S, T>
    where
        S: AsterRealSigningProvider,
        T: AsterExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            request.validate()?;
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }
            let symbol = aster_symbol_from_instrument(&request.instrument_id)?;
            self.ensure_target_leverage(&symbol, request.reduce_only)?;
            let params = aster_submit_order_params(&symbol, &request)?;
            let action_id = self.next_action_id(MutableActionKind::SubmitOrder)?;
            let signed = self.sign(SigningPurpose::SubmitOrder, &action_id, params)?;
            let response = self.dispatch_signed(
                AsterExecHttpMethod::Post,
                ASTER_FUTURES_V3_ORDER_ENDPOINT,
                &signed,
            )?;
            self.ensure_success(ASTER_FUTURES_V3_ORDER_ENDPOINT, &response)?;
            let known_order = aster_known_order_from_response(
                &symbol,
                request.client_order_id.clone(),
                response.body(),
                &action_id,
            )?;
            let external_ref = known_order
                .external_order_id
                .clone()
                .map(ExternalActionRef::Order);
            let receipt = MutableActionReceipt {
                action_id,
                kind: MutableActionKind::SubmitOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref,
                duplicate: false,
                simulated: false,
            };
            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            self.record_known_order(known_order);
            Ok(receipt)
        }

        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }
            let known_order =
                self.lookup_known_order(&request.order_ref)
                    .ok_or(VenueExecError::InvalidRequest {
                        field: "order_ref",
                        reason: "Aster cancel requires an order previously submitted through this adapter so its symbol is known",
                    })?;
            let params = aster_cancel_order_params(&request.order_ref, known_order)?;
            let action_id = self.next_action_id(MutableActionKind::CancelOrder)?;
            let signed = self.sign(SigningPurpose::CancelOrder, &action_id, params)?;
            let response = self.dispatch_signed(
                AsterExecHttpMethod::Delete,
                ASTER_FUTURES_V3_ORDER_ENDPOINT,
                &signed,
            )?;
            self.ensure_success(ASTER_FUTURES_V3_ORDER_ENDPOINT, &response)?;
            let receipt = MutableActionReceipt {
                action_id: action_id.clone(),
                kind: MutableActionKind::CancelOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref: Some(ExternalActionRef::Cancel(action_id)),
                duplicate: false,
                simulated: false,
            };
            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            Ok(receipt)
        }

        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let symbol = aster_symbol_from_instrument(&request.instrument_id)?;
            let params = aster_query_order_params(&symbol, &request.order_ref)?;
            let signing_request_id =
                order_query_signing_request_id("aster-exec", &request.source_event_id)?;
            let signed =
                self.sign_with_request_id(SigningPurpose::QueryOrder, signing_request_id, params)?;
            let response = self.dispatch_signed(
                AsterExecHttpMethod::Get,
                ASTER_FUTURES_V3_ORDER_ENDPOINT,
                &signed,
            )?;
            self.ensure_success(ASTER_FUTURES_V3_ORDER_ENDPOINT, &response)?;
            parse_aster_order_query_confirmation(
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                request.source_event_id,
                response.body(),
            )
        }

        fn ensure_target_leverage(
            &mut self,
            symbol: &str,
            reduce_only: bool,
        ) -> VenueExecResult<()> {
            if reduce_only {
                return Ok(());
            }
            let Some(leverage) = self.config.target_leverage else {
                return Ok(());
            };
            let params = aster_leverage_params(symbol, leverage)?;
            let signing_request_id = leverage_signing_request_id("aster-exec", symbol, leverage)?;
            let signed =
                self.sign_with_request_id(SigningPurpose::SubmitOrder, signing_request_id, params)?;
            let response = self.dispatch_signed(
                AsterExecHttpMethod::Post,
                ASTER_FUTURES_V3_LEVERAGE_ENDPOINT,
                &signed,
            )?;
            self.ensure_success(ASTER_FUTURES_V3_LEVERAGE_ENDPOINT, &response)
        }

        fn duplicate_receipt(
            &self,
            idempotency_key: &IdempotencyKey,
            fingerprint: &RequestFingerprint,
        ) -> VenueExecResult<Option<MutableActionReceipt>> {
            let Some(existing) = self.records_by_key.get(idempotency_key) else {
                return Ok(None);
            };
            if existing.fingerprint != *fingerprint {
                return Err(VenueExecError::IdempotencyConflict {
                    idempotency_key: idempotency_key.clone(),
                    existing_fingerprint: existing.fingerprint.0.clone(),
                    incoming_fingerprint: fingerprint.0.clone(),
                });
            }
            let mut receipt = existing.receipt.clone();
            receipt.duplicate = true;
            Ok(Some(receipt))
        }

        fn next_action_id(&mut self, kind: MutableActionKind) -> VenueExecResult<MutableActionId> {
            self.next_sequence = self
                .next_sequence
                .checked_add(1)
                .expect("Aster mutable action sequence overflowed");
            MutableActionId::new(format!(
                "aster:perp:{}:{}",
                kind.as_str(),
                self.next_sequence
            ))
        }

        fn sign(
            &self,
            purpose: SigningPurpose,
            action_id: &MutableActionId,
            params: Vec<AsterRequestParam>,
        ) -> VenueExecResult<AsterSignedEndpoint> {
            self.sign_with_request_id(
                purpose,
                SigningRequestId::new(format!("signing-request/aster-exec/{}", action_id.as_str()))
                    .map_err(signing_error)?,
                params,
            )
        }

        fn sign_with_request_id(
            &self,
            purpose: SigningPurpose,
            signing_request_id: SigningRequestId,
            params: Vec<AsterRequestParam>,
        ) -> VenueExecResult<AsterSignedEndpoint> {
            let input = AsterV3SigningInput::new(
                signing_request_id,
                self.config.signing_policy.policy_ref().clone(),
                purpose,
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                self.config.user.clone(),
                self.config.signer.clone(),
                params,
            )
            .map_err(signing_error)?;
            self.signer
                .sign_aster_eip712_external(input, &self.config.signing_policy)
                .map_err(signing_error)
        }

        fn dispatch_signed(
            &mut self,
            method: AsterExecHttpMethod,
            endpoint: &'static str,
            signed_endpoint: &AsterSignedEndpoint,
        ) -> VenueExecResult<AsterExecHttpResponse> {
            self.transport.send_signed(AsterSignedRequest {
                method,
                base_url: &self.config.base_url,
                endpoint,
                signed_endpoint,
            })
        }

        fn ensure_success(
            &self,
            endpoint: &'static str,
            response: &AsterExecHttpResponse,
        ) -> VenueExecResult<()> {
            if response.is_success() {
                return Ok(());
            }
            Err(VenueExecError::ExternalRejected {
                venue_id: self.config.venue_id.clone(),
                endpoint: endpoint.to_owned(),
                status_code: response.status_code(),
                reason: response_body_snippet(response.body()),
            })
        }

        fn record_action(
            &mut self,
            idempotency_key: IdempotencyKey,
            fingerprint: RequestFingerprint,
            receipt: MutableActionReceipt,
        ) {
            self.key_by_action_id
                .insert(receipt.action_id.clone(), idempotency_key.clone());
            self.records_by_key.insert(
                idempotency_key,
                LiveActionRecord {
                    fingerprint,
                    receipt,
                },
            );
        }

        fn record_known_order(&mut self, known_order: AsterKnownOrder) {
            if let Some(client_order_id) = known_order.client_order_id.clone() {
                self.orders_by_client_id
                    .insert(client_order_id, known_order.clone());
            }
            if let Some(external_order_id) = known_order.external_order_id.clone() {
                self.orders_by_external_id
                    .insert(external_order_id, known_order);
            }
        }

        fn lookup_known_order(&self, order_ref: &OrderReference) -> Option<&AsterKnownOrder> {
            match order_ref {
                OrderReference::ClientOrderId(order_id) => self.orders_by_client_id.get(order_id),
                OrderReference::VenueOrderId(order_id) => self.orders_by_external_id.get(order_id),
            }
        }
    }

    fn aster_leverage_params(
        symbol: &str,
        leverage: u32,
    ) -> VenueExecResult<Vec<AsterRequestParam>> {
        validate_perp_target_leverage("target_leverage", leverage)?;
        Ok(vec![
            aster_param("symbol", symbol)?,
            aster_param("leverage", leverage.to_string())?,
        ])
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct AsterKnownOrder {
        symbol: String,
        order_id_param: Option<String>,
        client_order_id: Option<OrderId>,
        external_order_id: Option<ExternalOrderId>,
    }

    fn normalize_aster_base_url(value: String) -> VenueExecResult<String> {
        let trimmed = value.trim().trim_end_matches('/').to_owned();
        if trimmed.is_empty() {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Aster base URL cannot be empty",
            });
        }
        if trimmed
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Aster base URL contains a control byte",
            });
        }
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://127.0.0.1")) {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Aster base URL must use https or an explicit localhost test URL",
            });
        }
        Ok(trimmed)
    }

    fn aster_submit_order_params(
        symbol: &str,
        request: &SubmitOrderRequest,
    ) -> VenueExecResult<Vec<AsterRequestParam>> {
        let mut params = vec![
            aster_param("symbol", symbol)?,
            aster_param("side", binance_side(request.side))?,
        ];
        match request.order_type {
            MutableOrderType::Market => {
                params.push(aster_param("type", "MARKET")?);
                params.push(aster_param("quantity", request.quantity.to_string())?);
            }
            MutableOrderType::Limit => {
                params.push(aster_param("type", "LIMIT")?);
                params.push(aster_param(
                    "timeInForce",
                    binance_time_in_force(limit_time_in_force(request)),
                )?);
                params.push(aster_param("quantity", request.quantity.to_string())?);
                params.push(aster_param(
                    "price",
                    request
                        .limit_price
                        .expect("validated limit order price")
                        .to_string(),
                )?);
            }
            MutableOrderType::PostOnly => {
                params.push(aster_param("type", "LIMIT")?);
                params.push(aster_param("timeInForce", "GTX")?);
                params.push(aster_param("quantity", request.quantity.to_string())?);
                params.push(aster_param(
                    "price",
                    request
                        .limit_price
                        .expect("validated post-only order price")
                        .to_string(),
                )?);
            }
        }
        if request.reduce_only {
            params.push(aster_param("reduceOnly", "true")?);
        }
        if let Some(client_order_id) = &request.client_order_id {
            validate_aster_client_order_id(client_order_id.as_str())?;
            params.push(aster_param("newClientOrderId", client_order_id.as_str())?);
        }
        Ok(params)
    }

    fn aster_cancel_order_params(
        order_ref: &OrderReference,
        known_order: &AsterKnownOrder,
    ) -> VenueExecResult<Vec<AsterRequestParam>> {
        let mut params = vec![aster_param("symbol", known_order.symbol.as_str())?];
        match order_ref {
            OrderReference::VenueOrderId(_) => {
                if let Some(order_id) = &known_order.order_id_param {
                    params.push(aster_param("orderId", order_id.as_str())?);
                } else if let Some(client_order_id) = &known_order.client_order_id {
                    params.push(aster_param("origClientOrderId", client_order_id.as_str())?);
                } else {
                    return Err(VenueExecError::InvalidRequest {
                        field: "order_ref",
                        reason: "known Aster venue order lacks orderId and client order ID",
                    });
                }
            }
            OrderReference::ClientOrderId(client_order_id) => {
                params.push(aster_param("origClientOrderId", client_order_id.as_str())?);
            }
        }
        Ok(params)
    }

    fn aster_query_order_params(
        symbol: &str,
        order_ref: &OrderReference,
    ) -> VenueExecResult<Vec<AsterRequestParam>> {
        let mut params = vec![aster_param("symbol", symbol)?];
        match order_ref {
            OrderReference::VenueOrderId(order_id) => {
                params.push(aster_param(
                    "orderId",
                    aster_order_id_param_from_external(order_id)?,
                )?);
            }
            OrderReference::ClientOrderId(client_order_id) => {
                params.push(aster_param("origClientOrderId", client_order_id.as_str())?);
            }
        }
        Ok(params)
    }

    fn aster_order_id_param_from_external(order_id: &ExternalOrderId) -> VenueExecResult<&str> {
        let value = order_id.as_str();
        let Some(raw_order_id) = value.strip_prefix("aster-perp:order:") else {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "Aster venue order ref must come from the Aster perp adapter",
            });
        };
        if raw_order_id.is_empty() || raw_order_id.bytes().any(|byte| !byte.is_ascii_digit()) {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "Aster venue order ref lacks numeric orderId",
            });
        }
        Ok(raw_order_id)
    }

    fn aster_symbol_from_instrument(instrument_id: &InstrumentId) -> VenueExecResult<String> {
        let value = instrument_id.as_str();
        let mut parts = value.split(':');
        let prefix = parts.next();
        let venue = parts.next();
        let symbol = parts.next();
        let suffix = parts.next();
        if parts.next().is_some()
            || prefix != Some("inst")
            || venue != Some("ASTER")
            || suffix != Some("USDT-FUTURES")
        {
            return Err(VenueExecError::InvalidRequest {
                field: "instrument_id",
                reason: "Aster execution requires instrument IDs shaped as inst:ASTER:<SYMBOL>:USDT-FUTURES",
            });
        }
        let symbol = symbol.expect("symbol checked above");
        validate_binance_symbol(symbol)?;
        Ok(symbol.to_owned())
    }

    fn validate_aster_client_order_id(value: &str) -> VenueExecResult<()> {
        validate_binance_client_order_id(value)
    }

    fn aster_known_order_from_response(
        symbol: &str,
        client_order_id: Option<OrderId>,
        body: &str,
        action_id: &MutableActionId,
    ) -> VenueExecResult<AsterKnownOrder> {
        let order_id_param = json_field_value(body, "orderId");
        let response_client_order_id =
            json_field_value(body, "clientOrderId").and_then(|value| OrderId::new(value).ok());
        let client_order_id = client_order_id.or(response_client_order_id);
        let external_order_id = if let Some(order_id) = &order_id_param {
            Some(ExternalOrderId::new(format!(
                "aster-perp:order:{order_id}"
            ))?)
        } else if let Some(client_order_id) = &client_order_id {
            Some(ExternalOrderId::new(format!(
                "aster-perp:client:{}",
                client_order_id.as_str()
            ))?)
        } else {
            Some(ExternalOrderId::new(format!(
                "aster-perp:action:{}",
                action_id.as_str()
            ))?)
        };
        Ok(AsterKnownOrder {
            symbol: symbol.to_owned(),
            order_id_param,
            client_order_id,
            external_order_id,
        })
    }

    fn aster_param(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> VenueExecResult<AsterRequestParam> {
        AsterRequestParam::new(name, value).map_err(signing_error)
    }

    fn aster_signed_request_url(request: &AsterSignedRequest<'_>) -> VenueExecResult<String> {
        let endpoint = request.endpoint();
        if endpoint.is_empty() || !endpoint.starts_with('/') {
            return Err(VenueExecError::InvalidRequest {
                field: "endpoint",
                reason: "Aster signed REST endpoint must be an absolute path",
            });
        }
        Ok(format!(
            "{}{}?{}",
            request.base_url(),
            endpoint,
            request.signed_query_for_transport()
        ))
    }

    fn parse_aster_curl_http_response(stdout: &[u8]) -> VenueExecResult<AsterExecHttpResponse> {
        let output = String::from_utf8_lossy(stdout);
        let Some((body, status)) = output.rsplit_once(CURL_ASTER_STATUS_MARKER) else {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: aster_transport_venue_id(),
                detail: "curl transport response lacked an HTTP status marker".to_owned(),
            });
        };
        let status_code =
            status
                .trim()
                .parse::<u16>()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: aster_transport_venue_id(),
                    detail: "curl transport returned a malformed HTTP status".to_owned(),
                })?;
        if status_code == 0 {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: aster_transport_venue_id(),
                detail: "curl transport did not receive an HTTP response from Aster".to_owned(),
            });
        }
        Ok(AsterExecHttpResponse::new(status_code, body.to_owned()))
    }

    fn aster_transport_venue_id() -> VenueId {
        VenueId::new("venue:ASTER-USDT-FUTURES").expect("static Aster transport venue ID")
    }

    /// Hyperliquid exchange endpoint。
    pub const HYPERLIQUID_EXCHANGE_ENDPOINT: &str = "/exchange";
    /// Hyperliquid info endpoint。
    pub const HYPERLIQUID_INFO_ENDPOINT: &str = "/info";
    /// 默认 Hyperliquid API base URL。
    pub const HYPERLIQUID_API_BASE_URL: &str = "https://api.hyperliquid.xyz";
    const CURL_HYPERLIQUID_STATUS_MARKER: &str = "\n__ARB_HYPERLIQUID_HTTP_STATUS__:";

    /// Hyperliquid L1 action signer 请求。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct HyperliquidSigningInput {
        pub source: String,
        pub nonce: u64,
        pub vault_address: Option<String>,
        pub expires_after: Option<u64>,
        pub action_json: String,
    }

    pub trait HyperliquidExchangeSigner {
        fn sign_l1_action(
            &self,
            input: HyperliquidSigningInput,
        ) -> VenueExecResult<HyperliquidSignatureJson>;
    }

    /// Hyperliquid 签名 JSON。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct HyperliquidSignatureJson {
        json: String,
    }

    impl HyperliquidSignatureJson {
        pub fn new(json: impl Into<String>) -> VenueExecResult<Self> {
            let json = json.into();
            validate_hyperliquid_signature_json(&json)?;
            Ok(Self { json })
        }

        pub fn as_json(&self) -> &str {
            &self.json
        }
    }

    /// 使用 `arb-wallet-signer hyperliquid-l1-action` 的 Hyperliquid signer。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct HyperliquidExternalSigner {
        command: String,
    }

    impl HyperliquidExternalSigner {
        pub fn new(command: impl Into<String>) -> VenueExecResult<Self> {
            let command = command.into();
            validate_command_path("hyperliquid_signer_command", &command)?;
            Ok(Self { command })
        }
    }

    impl HyperliquidExchangeSigner for HyperliquidExternalSigner {
        fn sign_l1_action(
            &self,
            input: HyperliquidSigningInput,
        ) -> VenueExecResult<HyperliquidSignatureJson> {
            let mut command = Command::new(&self.command);
            command
                .arg("hyperliquid-l1-action")
                .arg("--source")
                .arg(&input.source)
                .arg("--nonce")
                .arg(input.nonce.to_string());
            if let Some(vault_address) = &input.vault_address {
                command.arg("--vault-address").arg(vault_address);
            }
            if let Some(expires_after) = input.expires_after {
                command
                    .arg("--expires-after")
                    .arg(expires_after.to_string());
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| VenueExecError::SigningFailed {
                    reason: "cannot start external Hyperliquid signer command".to_owned(),
                })?;
            child
                .stdin
                .as_mut()
                .ok_or_else(|| VenueExecError::SigningFailed {
                    reason: "external Hyperliquid signer stdin is unavailable".to_owned(),
                })?
                .write_all(input.action_json.as_bytes())
                .map_err(|_| VenueExecError::SigningFailed {
                    reason: "cannot write Hyperliquid action to external signer".to_owned(),
                })?;
            let output = child
                .wait_with_output()
                .map_err(|_| VenueExecError::SigningFailed {
                    reason: "external Hyperliquid signer did not return a reliable result"
                        .to_owned(),
                })?;
            if !output.status.success() {
                return Err(VenueExecError::SigningFailed {
                    reason: "external Hyperliquid signer exited unsuccessfully".to_owned(),
                });
            }
            let rendered =
                String::from_utf8(output.stdout).map_err(|_| VenueExecError::SigningFailed {
                    reason: "external Hyperliquid signer output is not valid UTF-8".to_owned(),
                })?;
            HyperliquidSignatureJson::new(rendered)
        }
    }

    pub trait HyperliquidExecTransport {
        fn post_json(
            &mut self,
            base_url: &str,
            endpoint: &'static str,
            body: &str,
        ) -> VenueExecResult<HyperliquidExecHttpResponse>;
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct HyperliquidExecHttpResponse {
        status_code: u16,
        body: String,
    }

    impl HyperliquidExecHttpResponse {
        pub fn new(status_code: u16, body: impl Into<String>) -> Self {
            Self {
                status_code,
                body: body.into(),
            }
        }

        pub fn status_code(&self) -> u16 {
            self.status_code
        }

        pub fn body(&self) -> &str {
            &self.body
        }

        pub fn is_success(&self) -> bool {
            (200..=299).contains(&self.status_code)
        }
    }

    /// 使用系统 `curl` 发送 Hyperliquid JSON 请求的真实 transport。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct HyperliquidCurlExecTransport {
        connect_timeout_secs: u64,
        max_time_secs: u64,
    }

    impl HyperliquidCurlExecTransport {
        pub fn new(connect_timeout_secs: u64, max_time_secs: u64) -> VenueExecResult<Self> {
            if connect_timeout_secs == 0 || max_time_secs == 0 {
                return Err(VenueExecError::InvalidRequest {
                    field: "curl_timeout",
                    reason: "curl timeouts must be greater than zero",
                });
            }
            Ok(Self {
                connect_timeout_secs,
                max_time_secs,
            })
        }
    }

    impl Default for HyperliquidCurlExecTransport {
        fn default() -> Self {
            Self {
                connect_timeout_secs: 10,
                max_time_secs: 30,
            }
        }
    }

    impl HyperliquidExecTransport for HyperliquidCurlExecTransport {
        fn post_json(
            &mut self,
            base_url: &str,
            endpoint: &'static str,
            body: &str,
        ) -> VenueExecResult<HyperliquidExecHttpResponse> {
            let url = hyperliquid_request_url(base_url, endpoint)?;
            let mut config = format!("url = \"{}\"\n", curl_config_quote(&url)?);
            push_curl_header(&mut config, "Content-Type", "application/json")?;
            config.push_str("data = \"");
            config.push_str(&curl_config_quote(body)?);
            config.push_str("\"\n");
            let mut child = Command::new("curl")
                .arg("--silent")
                .arg("--show-error")
                .arg("--request")
                .arg("POST")
                .arg("--connect-timeout")
                .arg(self.connect_timeout_secs.to_string())
                .arg("--max-time")
                .arg(self.max_time_secs.to_string())
                .arg("--write-out")
                .arg(format!("{CURL_HYPERLIQUID_STATUS_MARKER}%{{http_code}}"))
                .arg("--config")
                .arg("-")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: hyperliquid_transport_venue_id(),
                    detail: "failed to start curl for Hyperliquid REST request".to_owned(),
                })?;
            child
                .stdin
                .as_mut()
                .ok_or_else(|| VenueExecError::UnknownExternalState {
                    venue_id: hyperliquid_transport_venue_id(),
                    detail: "curl stdin is unavailable for Hyperliquid REST request".to_owned(),
                })?
                .write_all(config.as_bytes())
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: hyperliquid_transport_venue_id(),
                    detail: "failed to write curl config for Hyperliquid REST request".to_owned(),
                })?;
            let output =
                child
                    .wait_with_output()
                    .map_err(|_| VenueExecError::UnknownExternalState {
                        venue_id: hyperliquid_transport_venue_id(),
                        detail: "curl transport did not return a Hyperliquid REST response"
                            .to_owned(),
                    })?;
            if !output.status.success() {
                return Err(VenueExecError::UnknownExternalState {
                    venue_id: hyperliquid_transport_venue_id(),
                    detail: "curl transport failed before a reliable HTTP response was available"
                        .to_owned(),
                });
            }
            parse_hyperliquid_curl_http_response(&output.stdout)
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct HyperliquidExecConfig {
        venue_id: VenueId,
        account_id: AccountId,
        base_url: String,
        user: String,
        source: String,
        vault_address: Option<String>,
        expires_after_ms: Option<u64>,
        target_leverage: Option<u32>,
        signing_policy: SigningPolicy,
        asset_ids_by_symbol: BTreeMap<String, u32>,
    }

    impl HyperliquidExecConfig {
        pub fn perp(
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            user: impl Into<String>,
            source: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            let user = user.into();
            validate_ethereum_address_text("hyperliquid_user", &user)?;
            let source = source.into();
            validate_hyperliquid_source(&source)?;
            Ok(Self {
                venue_id,
                account_id,
                base_url: normalize_hyperliquid_base_url(base_url.into())?,
                user: user.to_ascii_lowercase(),
                source,
                vault_address: None,
                expires_after_ms: None,
                target_leverage: None,
                signing_policy,
                asset_ids_by_symbol: BTreeMap::new(),
            })
        }

        pub fn with_vault_address(
            mut self,
            vault_address: impl Into<String>,
        ) -> VenueExecResult<Self> {
            let vault_address = vault_address.into();
            validate_ethereum_address_text("hyperliquid_vault_address", &vault_address)?;
            self.vault_address = Some(vault_address.to_ascii_lowercase());
            Ok(self)
        }

        pub fn with_expires_after_ms(mut self, expires_after_ms: u64) -> Self {
            self.expires_after_ms = Some(expires_after_ms);
            self
        }

        pub fn with_asset_id(
            mut self,
            symbol: impl Into<String>,
            asset_id: u32,
        ) -> VenueExecResult<Self> {
            let symbol = symbol.into();
            validate_hyperliquid_symbol(&symbol)?;
            self.asset_ids_by_symbol.insert(symbol, asset_id);
            Ok(self)
        }

        pub fn with_target_leverage(mut self, leverage: u32) -> VenueExecResult<Self> {
            validate_perp_target_leverage("target_leverage", leverage)?;
            self.target_leverage = Some(leverage);
            Ok(self)
        }

        pub fn venue_id(&self) -> &VenueId {
            &self.venue_id
        }

        pub fn account_id(&self) -> &AccountId {
            &self.account_id
        }

        pub fn target_leverage(&self) -> Option<u32> {
            self.target_leverage
        }
    }

    /// Hyperliquid perp 可变执行适配器。
    pub struct HyperliquidPerpExecAdapter<S, T> {
        inner: HyperliquidExecAdapterCore<S, T>,
    }

    impl<S, T> HyperliquidPerpExecAdapter<S, T> {
        pub fn new(
            config: HyperliquidExecConfig,
            signer: S,
            transport: T,
        ) -> VenueExecResult<Self> {
            Ok(Self {
                inner: HyperliquidExecAdapterCore::new(config, signer, transport),
            })
        }

        pub fn config(&self) -> &HyperliquidExecConfig {
            self.inner.config()
        }

        pub fn transport(&self) -> &T {
            self.inner.transport()
        }

        pub fn transport_mut(&mut self) -> &mut T {
            self.inner.transport_mut()
        }
    }

    impl<S, T> fmt::Debug for HyperliquidPerpExecAdapter<S, T>
    where
        S: fmt::Debug,
        T: fmt::Debug,
    {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("HyperliquidPerpExecAdapter")
                .field("inner", &self.inner)
                .finish()
        }
    }

    impl<S, T> SubmitOrder for HyperliquidPerpExecAdapter<S, T>
    where
        S: HyperliquidExchangeSigner,
        T: HyperliquidExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.submit_order(request)
        }
    }

    impl<S, T> CancelOrder for HyperliquidPerpExecAdapter<S, T>
    where
        S: HyperliquidExchangeSigner,
        T: HyperliquidExecTransport,
    {
        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.cancel_order(request)
        }
    }

    impl<S, T> QueryActionStatus for HyperliquidPerpExecAdapter<S, T>
    where
        S: HyperliquidExchangeSigner,
        T: HyperliquidExecTransport,
    {
        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            self.inner.query_action_status(request)
        }
    }

    impl<S, T> ConfirmOrderStatus for HyperliquidPerpExecAdapter<S, T>
    where
        S: HyperliquidExchangeSigner,
        T: HyperliquidExecTransport,
    {
        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.inner.confirm_order_status(request)
        }
    }

    impl<S, T> RequestTransfer for HyperliquidPerpExecAdapter<S, T>
    where
        S: HyperliquidExchangeSigner,
        T: HyperliquidExecTransport,
    {
        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.request_transfer(request)
        }
    }

    #[derive(Debug)]
    struct HyperliquidExecAdapterCore<S, T> {
        config: HyperliquidExecConfig,
        signer: S,
        transport: T,
        records_by_key: BTreeMap<IdempotencyKey, LiveActionRecord>,
        key_by_action_id: BTreeMap<MutableActionId, IdempotencyKey>,
        orders_by_client_id: BTreeMap<OrderId, HyperliquidKnownOrder>,
        orders_by_external_id: BTreeMap<ExternalOrderId, HyperliquidKnownOrder>,
        next_sequence: u64,
        last_nonce_millis: Option<u64>,
    }

    impl<S, T> HyperliquidExecAdapterCore<S, T> {
        fn new(config: HyperliquidExecConfig, signer: S, transport: T) -> Self {
            Self {
                config,
                signer,
                transport,
                records_by_key: BTreeMap::new(),
                key_by_action_id: BTreeMap::new(),
                orders_by_client_id: BTreeMap::new(),
                orders_by_external_id: BTreeMap::new(),
                next_sequence: 0,
                last_nonce_millis: None,
            }
        }

        fn config(&self) -> &HyperliquidExecConfig {
            &self.config
        }

        fn transport(&self) -> &T {
            &self.transport
        }

        fn transport_mut(&mut self) -> &mut T {
            &mut self.transport
        }

        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            Ok(match request {
                QueryActionStatusRequest::ByActionId(action_id) => {
                    if let Some(key) = self.key_by_action_id.get(&action_id) {
                        self.report_for_key(key)
                    } else {
                        unknown_status_report(Some(action_id), None)
                    }
                }
                QueryActionStatusRequest::ByIdempotencyKey(key) => self.report_for_key(&key),
            })
        }

        fn report_for_key(&self, key: &IdempotencyKey) -> MutableActionStatusReport {
            self.records_by_key.get(key).map_or_else(
                || unknown_status_report(None, Some(key.clone())),
                |record| super::status_report_from_receipt(&record.receipt),
            )
        }

        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.from_account_id)?;
            Err(VenueExecError::InvalidRequest {
                field: "transfer",
                reason: "Hyperliquid live transfer is not implemented by this execution adapter",
            })
        }

        fn ensure_request_scope(
            &self,
            venue_id: &VenueId,
            account_id: &AccountId,
        ) -> VenueExecResult<()> {
            if venue_id != &self.config.venue_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "venue_id",
                    reason: "request venue does not match Hyperliquid execution adapter config",
                });
            }
            if account_id != &self.config.account_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "account_id",
                    reason: "request account does not match Hyperliquid execution adapter config",
                });
            }
            Ok(())
        }
    }

    impl<S, T> HyperliquidExecAdapterCore<S, T>
    where
        S: HyperliquidExchangeSigner,
        T: HyperliquidExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            request.validate()?;
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }
            ensure_hyperliquid_real_signing_policy(&self.config.signing_policy)?;
            let symbol = hyperliquid_symbol_from_instrument(&request.instrument_id)?;
            let asset_id = self.hyperliquid_asset_id(&symbol)?;
            self.ensure_target_leverage(asset_id, request.reduce_only)?;
            let action_json = hyperliquid_submit_order_action_json(asset_id, &request)?;
            let action_id = self.next_action_id(MutableActionKind::SubmitOrder)?;
            let body = self.sign_exchange_body(action_json)?;
            let response = self.transport.post_json(
                &self.config.base_url,
                HYPERLIQUID_EXCHANGE_ENDPOINT,
                &body,
            )?;
            self.ensure_success(HYPERLIQUID_EXCHANGE_ENDPOINT, &response)?;
            ensure_hyperliquid_exchange_ok(
                &self.config.venue_id,
                HYPERLIQUID_EXCHANGE_ENDPOINT,
                &response,
            )?;
            let known_order = hyperliquid_known_order_from_response(
                &symbol,
                asset_id,
                request.client_order_id.clone(),
                response.body(),
                &action_id,
            )?;
            let external_ref = known_order
                .external_order_id
                .clone()
                .map(ExternalActionRef::Order);
            let receipt = MutableActionReceipt {
                action_id,
                kind: MutableActionKind::SubmitOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref,
                duplicate: false,
                simulated: false,
            };
            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            self.record_known_order(known_order);
            Ok(receipt)
        }

        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }
            ensure_hyperliquid_real_signing_policy(&self.config.signing_policy)?;
            let known_order =
                self.lookup_known_order(&request.order_ref)
                    .ok_or(VenueExecError::InvalidRequest {
                        field: "order_ref",
                        reason: "Hyperliquid cancel requires an order previously submitted through this adapter so asset id is known",
                    })?;
            let action_json =
                hyperliquid_cancel_order_action_json(&request.order_ref, known_order)?;
            let action_id = self.next_action_id(MutableActionKind::CancelOrder)?;
            let body = self.sign_exchange_body(action_json)?;
            let response = self.transport.post_json(
                &self.config.base_url,
                HYPERLIQUID_EXCHANGE_ENDPOINT,
                &body,
            )?;
            self.ensure_success(HYPERLIQUID_EXCHANGE_ENDPOINT, &response)?;
            ensure_hyperliquid_exchange_ok(
                &self.config.venue_id,
                HYPERLIQUID_EXCHANGE_ENDPOINT,
                &response,
            )?;
            let receipt = MutableActionReceipt {
                action_id: action_id.clone(),
                kind: MutableActionKind::CancelOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref: Some(ExternalActionRef::Cancel(action_id)),
                duplicate: false,
                simulated: false,
            };
            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            Ok(receipt)
        }

        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let oid = hyperliquid_order_status_oid(&request.order_ref)?;
            let body = format!(
                "{{\"type\":\"orderStatus\",\"user\":{},\"oid\":{}}}",
                json_string_literal(&self.config.user)?,
                oid
            );
            let response = self.transport.post_json(
                &self.config.base_url,
                HYPERLIQUID_INFO_ENDPOINT,
                &body,
            )?;
            self.ensure_success(HYPERLIQUID_INFO_ENDPOINT, &response)?;
            parse_hyperliquid_order_query_confirmation(
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                request.source_event_id,
                response.body(),
            )
        }

        fn hyperliquid_asset_id(&self, symbol: &str) -> VenueExecResult<u32> {
            self.config
                .asset_ids_by_symbol
                .get(symbol)
                .or_else(|| {
                    symbol
                        .strip_suffix("USDT")
                        .and_then(|coin| self.config.asset_ids_by_symbol.get(coin))
                })
                .copied()
                .ok_or(VenueExecError::InvalidRequest {
                    field: "asset_id",
                reason: "Hyperliquid perp asset id mapping is required before live order submission",
            })
        }

        fn ensure_target_leverage(
            &mut self,
            asset_id: u32,
            reduce_only: bool,
        ) -> VenueExecResult<()> {
            if reduce_only {
                return Ok(());
            }
            let Some(leverage) = self.config.target_leverage else {
                return Ok(());
            };
            validate_perp_target_leverage("target_leverage", leverage)?;
            let action_json = hyperliquid_update_leverage_action_json(asset_id, leverage);
            let body = self.sign_exchange_body(action_json)?;
            let response = self.transport.post_json(
                &self.config.base_url,
                HYPERLIQUID_EXCHANGE_ENDPOINT,
                &body,
            )?;
            self.ensure_success(HYPERLIQUID_EXCHANGE_ENDPOINT, &response)?;
            ensure_hyperliquid_exchange_ok(
                &self.config.venue_id,
                HYPERLIQUID_EXCHANGE_ENDPOINT,
                &response,
            )
        }

        fn next_hyperliquid_nonce(&mut self) -> VenueExecResult<u64> {
            let now = current_unix_millis()?;
            let nonce = match self.last_nonce_millis {
                Some(last) if now <= last => {
                    last.checked_add(1)
                        .ok_or_else(|| VenueExecError::UnknownExternalState {
                            venue_id: self.config.venue_id.clone(),
                            detail: "Hyperliquid nonce milliseconds overflowed".to_owned(),
                        })?
                }
                _ => now,
            };
            self.last_nonce_millis = Some(nonce);
            Ok(nonce)
        }

        fn sign_exchange_body(&mut self, action_json: String) -> VenueExecResult<String> {
            let nonce = self.next_hyperliquid_nonce()?;
            let signature = self.signer.sign_l1_action(HyperliquidSigningInput {
                source: self.config.source.clone(),
                nonce,
                vault_address: self.config.vault_address.clone(),
                expires_after: self.config.expires_after_ms,
                action_json: action_json.clone(),
            })?;
            let mut body = format!(
                "{{\"action\":{},\"nonce\":{},\"signature\":{}",
                action_json,
                nonce,
                signature.as_json()
            );
            if let Some(vault_address) = &self.config.vault_address {
                body.push_str(",\"vaultAddress\":");
                body.push_str(&json_string_literal(vault_address)?);
            }
            if let Some(expires_after) = self.config.expires_after_ms {
                body.push_str(",\"expiresAfter\":");
                body.push_str(&expires_after.to_string());
            }
            body.push('}');
            Ok(body)
        }

        fn duplicate_receipt(
            &self,
            idempotency_key: &IdempotencyKey,
            fingerprint: &RequestFingerprint,
        ) -> VenueExecResult<Option<MutableActionReceipt>> {
            let Some(existing) = self.records_by_key.get(idempotency_key) else {
                return Ok(None);
            };
            if existing.fingerprint != *fingerprint {
                return Err(VenueExecError::IdempotencyConflict {
                    idempotency_key: idempotency_key.clone(),
                    existing_fingerprint: existing.fingerprint.0.clone(),
                    incoming_fingerprint: fingerprint.0.clone(),
                });
            }
            let mut receipt = existing.receipt.clone();
            receipt.duplicate = true;
            Ok(Some(receipt))
        }

        fn next_action_id(&mut self, kind: MutableActionKind) -> VenueExecResult<MutableActionId> {
            self.next_sequence = self
                .next_sequence
                .checked_add(1)
                .expect("Hyperliquid mutable action sequence overflowed");
            MutableActionId::new(format!(
                "hyperliquid:perp:{}:{}",
                kind.as_str(),
                self.next_sequence
            ))
        }

        fn ensure_success(
            &self,
            endpoint: &'static str,
            response: &HyperliquidExecHttpResponse,
        ) -> VenueExecResult<()> {
            if response.is_success() {
                return Ok(());
            }
            Err(VenueExecError::ExternalRejected {
                venue_id: self.config.venue_id.clone(),
                endpoint: endpoint.to_owned(),
                status_code: response.status_code(),
                reason: response_body_snippet(response.body()),
            })
        }

        fn record_action(
            &mut self,
            idempotency_key: IdempotencyKey,
            fingerprint: RequestFingerprint,
            receipt: MutableActionReceipt,
        ) {
            self.key_by_action_id
                .insert(receipt.action_id.clone(), idempotency_key.clone());
            self.records_by_key.insert(
                idempotency_key,
                LiveActionRecord {
                    fingerprint,
                    receipt,
                },
            );
        }

        fn record_known_order(&mut self, known_order: HyperliquidKnownOrder) {
            if let Some(client_order_id) = known_order.client_order_id.clone() {
                self.orders_by_client_id
                    .insert(client_order_id, known_order.clone());
            }
            if let Some(external_order_id) = known_order.external_order_id.clone() {
                self.orders_by_external_id
                    .insert(external_order_id, known_order);
            }
        }

        fn lookup_known_order(&self, order_ref: &OrderReference) -> Option<&HyperliquidKnownOrder> {
            match order_ref {
                OrderReference::ClientOrderId(order_id) => self.orders_by_client_id.get(order_id),
                OrderReference::VenueOrderId(order_id) => self.orders_by_external_id.get(order_id),
            }
        }
    }

    fn hyperliquid_update_leverage_action_json(asset_id: u32, leverage: u32) -> String {
        format!(
            "{{\"type\":\"updateLeverage\",\"asset\":{asset_id},\"isCross\":true,\"leverage\":{leverage}}}"
        )
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct HyperliquidKnownOrder {
        symbol: String,
        asset_id: u32,
        raw_order_id: Option<String>,
        client_order_id: Option<OrderId>,
        external_order_id: Option<ExternalOrderId>,
    }

    fn normalize_hyperliquid_base_url(value: String) -> VenueExecResult<String> {
        let trimmed = value.trim().trim_end_matches('/').to_owned();
        if trimmed.is_empty() {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Hyperliquid base URL cannot be empty",
            });
        }
        if trimmed
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Hyperliquid base URL contains a control byte",
            });
        }
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://127.0.0.1")) {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Hyperliquid base URL must use https or an explicit localhost test URL",
            });
        }
        Ok(trimmed)
    }

    fn hyperliquid_submit_order_action_json(
        asset_id: u32,
        request: &SubmitOrderRequest,
    ) -> VenueExecResult<String> {
        if request.order_type == MutableOrderType::Market {
            return Err(VenueExecError::InvalidRequest {
                field: "order_type",
                reason: "Hyperliquid live market order requires an explicit protective limit price; use limit IOC",
            });
        }
        let tif = match request.order_type {
            MutableOrderType::PostOnly => "Alo",
            MutableOrderType::Limit => match limit_time_in_force(request) {
                MutableTimeInForce::Gtc => "Gtc",
                MutableTimeInForce::Ioc => "Ioc",
                MutableTimeInForce::Fok => {
                    return Err(VenueExecError::InvalidRequest {
                        field: "time_in_force",
                        reason: "Hyperliquid exchange endpoint supports Gtc, Ioc and Alo; FOK is not mapped",
                    });
                }
            },
            MutableOrderType::Market => unreachable!("market rejected above"),
        };
        let mut order = format!(
            "{{\"a\":{},\"b\":{},\"p\":{},\"s\":{},\"r\":{},\"t\":{{\"limit\":{{\"tif\":{}}}}}",
            asset_id,
            request.side == OrderSide::Buy,
            json_string_literal(
                &request
                    .limit_price
                    .expect("validated Hyperliquid limit price")
                    .to_string()
            )?,
            json_string_literal(&request.quantity.to_string())?,
            request.reduce_only,
            json_string_literal(tif)?
        );
        if let Some(client_order_id) = &request.client_order_id {
            validate_hyperliquid_client_order_id(client_order_id.as_str())?;
            order.push_str(",\"c\":");
            order.push_str(&json_string_literal(client_order_id.as_str())?);
        }
        order.push('}');
        Ok(format!(
            "{{\"type\":\"order\",\"orders\":[{}],\"grouping\":\"na\"}}",
            order
        ))
    }

    fn hyperliquid_cancel_order_action_json(
        order_ref: &OrderReference,
        known_order: &HyperliquidKnownOrder,
    ) -> VenueExecResult<String> {
        match order_ref {
            OrderReference::VenueOrderId(_) => {
                let raw_order_id =
                    known_order
                        .raw_order_id
                        .as_ref()
                        .ok_or(VenueExecError::InvalidRequest {
                            field: "order_ref",
                            reason: "known Hyperliquid venue order lacks numeric oid",
                        })?;
                Ok(format!(
                    "{{\"type\":\"cancel\",\"cancels\":[{{\"a\":{},\"o\":{}}}]}}",
                    known_order.asset_id, raw_order_id
                ))
            }
            OrderReference::ClientOrderId(client_order_id) => {
                validate_hyperliquid_client_order_id(client_order_id.as_str())?;
                Ok(format!(
                    "{{\"type\":\"cancelByCloid\",\"cancels\":[{{\"asset\":{},\"cloid\":{}}}]}}",
                    known_order.asset_id,
                    json_string_literal(client_order_id.as_str())?
                ))
            }
        }
    }

    fn hyperliquid_order_status_oid(order_ref: &OrderReference) -> VenueExecResult<String> {
        match order_ref {
            OrderReference::VenueOrderId(order_id) => {
                let value = order_id.as_str();
                let Some(raw_order_id) = value.strip_prefix("hyperliquid-perp:order:") else {
                    return Err(VenueExecError::InvalidRequest {
                        field: "order_ref",
                        reason: "Hyperliquid venue order ref must come from the Hyperliquid perp adapter",
                    });
                };
                if raw_order_id.is_empty()
                    || raw_order_id.bytes().any(|byte| !byte.is_ascii_digit())
                {
                    return Err(VenueExecError::InvalidRequest {
                        field: "order_ref",
                        reason: "Hyperliquid venue order ref lacks numeric oid",
                    });
                }
                Ok(raw_order_id.to_owned())
            }
            OrderReference::ClientOrderId(client_order_id) => {
                validate_hyperliquid_client_order_id(client_order_id.as_str())?;
                Ok(json_string_literal(client_order_id.as_str())?)
            }
        }
    }

    fn hyperliquid_symbol_from_instrument(instrument_id: &InstrumentId) -> VenueExecResult<String> {
        let value = instrument_id.as_str();
        let mut parts = value.split(':');
        let prefix = parts.next();
        let venue = parts.next();
        let symbol = parts.next();
        let suffix = parts.next();
        if parts.next().is_some()
            || prefix != Some("inst")
            || venue != Some("HYPERLIQUID")
            || suffix != Some("PERP")
        {
            return Err(VenueExecError::InvalidRequest {
                field: "instrument_id",
                reason: "Hyperliquid execution requires instrument IDs shaped as inst:HYPERLIQUID:<SYMBOL>:PERP",
            });
        }
        let symbol = symbol.expect("symbol checked above");
        validate_hyperliquid_symbol(symbol)?;
        Ok(symbol.to_owned())
    }

    fn hyperliquid_known_order_from_response(
        symbol: &str,
        asset_id: u32,
        client_order_id: Option<OrderId>,
        body: &str,
        action_id: &MutableActionId,
    ) -> VenueExecResult<HyperliquidKnownOrder> {
        let raw_order_id = json_field_value(body, "oid")
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()));
        let response_client_order_id = json_field_value(body, "cloid")
            .filter(|value| !value.is_empty() && value != "null")
            .and_then(|value| OrderId::new(value).ok());
        let client_order_id = client_order_id.or(response_client_order_id);
        let external_order_id = if let Some(order_id) = &raw_order_id {
            Some(ExternalOrderId::new(format!(
                "hyperliquid-perp:order:{order_id}"
            ))?)
        } else if let Some(client_order_id) = &client_order_id {
            Some(ExternalOrderId::new(format!(
                "hyperliquid-perp:client:{}",
                client_order_id.as_str()
            ))?)
        } else {
            Some(ExternalOrderId::new(format!(
                "hyperliquid-perp:action:{}",
                action_id.as_str()
            ))?)
        };
        Ok(HyperliquidKnownOrder {
            symbol: symbol.to_owned(),
            asset_id,
            raw_order_id,
            client_order_id,
            external_order_id,
        })
    }

    fn ensure_hyperliquid_real_signing_policy(policy: &SigningPolicy) -> VenueExecResult<()> {
        if policy.mode() == SigningPolicyMode::RealSigningEnabled {
            Ok(())
        } else {
            Err(VenueExecError::SigningFailed {
                reason: "Hyperliquid live exchange signing requires real signing policy".to_owned(),
            })
        }
    }

    fn ensure_hyperliquid_exchange_ok(
        venue_id: &VenueId,
        endpoint: &'static str,
        response: &HyperliquidExecHttpResponse,
    ) -> VenueExecResult<()> {
        if json_field_value(response.body(), "status").as_deref() == Some("ok")
            && !response.body().contains("\"error\"")
        {
            return Ok(());
        }
        Err(VenueExecError::ExternalRejected {
            venue_id: venue_id.clone(),
            endpoint: endpoint.to_owned(),
            status_code: response.status_code(),
            reason: response_body_snippet(response.body()),
        })
    }

    fn validate_hyperliquid_signature_json(value: &str) -> VenueExecResult<()> {
        let trimmed = value.trim();
        let r = json_field_value(trimmed, "r").ok_or(VenueExecError::SigningFailed {
            reason: "Hyperliquid signature JSON lacks r".to_owned(),
        })?;
        let s = json_field_value(trimmed, "s").ok_or(VenueExecError::SigningFailed {
            reason: "Hyperliquid signature JSON lacks s".to_owned(),
        })?;
        let v = json_field_value(trimmed, "v").ok_or(VenueExecError::SigningFailed {
            reason: "Hyperliquid signature JSON lacks v".to_owned(),
        })?;
        validate_signature_hex_32("hyperliquid_signature_r", &r)?;
        validate_signature_hex_32("hyperliquid_signature_s", &s)?;
        if v != "27" && v != "28" {
            return Err(VenueExecError::SigningFailed {
                reason: "Hyperliquid signature JSON v must be 27 or 28".to_owned(),
            });
        }
        Ok(())
    }

    fn validate_signature_hex_32(field: &'static str, value: &str) -> VenueExecResult<()> {
        let hex = value.strip_prefix("0x").unwrap_or(value);
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(VenueExecError::InvalidRequest {
                field,
                reason: "signature part must be 32 bytes encoded as hex",
            });
        }
        Ok(())
    }

    fn validate_hyperliquid_symbol(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 64 {
            return Err(VenueExecError::InvalidRequest {
                field: "symbol",
                reason: "Hyperliquid symbol must be 1 to 64 bytes",
            });
        }
        if value.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'/'))
        }) {
            return Err(VenueExecError::InvalidRequest {
                field: "symbol",
                reason: "Hyperliquid symbol contains an unsupported byte",
            });
        }
        Ok(())
    }

    fn validate_hyperliquid_client_order_id(value: &str) -> VenueExecResult<()> {
        let hex = value
            .strip_prefix("0x")
            .ok_or(VenueExecError::InvalidRequest {
                field: "client_order_id",
                reason: "Hyperliquid cloid must be a 0x-prefixed 16-byte hex string",
            })?;
        if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(VenueExecError::InvalidRequest {
                field: "client_order_id",
                reason: "Hyperliquid cloid must be a 0x-prefixed 16-byte hex string",
            });
        }
        Ok(())
    }

    fn validate_hyperliquid_source(value: &str) -> VenueExecResult<()> {
        if matches!(value, "a" | "b") {
            Ok(())
        } else {
            Err(VenueExecError::InvalidRequest {
                field: "source",
                reason: "Hyperliquid source must be `a` for mainnet or `b` for testnet",
            })
        }
    }

    fn validate_ethereum_address_text(field: &'static str, value: &str) -> VenueExecResult<()> {
        if value.len() != 42 || !value.starts_with("0x") {
            return Err(VenueExecError::InvalidRequest {
                field,
                reason: "address must be a 42-character 0x-prefixed hex string",
            });
        }
        if !value.as_bytes().iter().skip(2).all(u8::is_ascii_hexdigit) {
            return Err(VenueExecError::InvalidRequest {
                field,
                reason: "address must contain only hexadecimal characters",
            });
        }
        Ok(())
    }

    fn validate_command_path(field: &'static str, value: &str) -> VenueExecResult<()> {
        if value.trim().is_empty() {
            return Err(VenueExecError::InvalidRequest {
                field,
                reason: "command path cannot be empty",
            });
        }
        if value.len() > 4096
            || value
                .bytes()
                .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(VenueExecError::InvalidRequest {
                field,
                reason: "command path contains an unsupported byte",
            });
        }
        Ok(())
    }

    fn current_unix_millis() -> VenueExecResult<u64> {
        let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
            VenueExecError::UnknownExternalState {
                venue_id: hyperliquid_transport_venue_id(),
                detail: "system clock is before Unix epoch".to_owned(),
            }
        })?;
        u64::try_from(duration.as_millis()).map_err(|_| VenueExecError::UnknownExternalState {
            venue_id: hyperliquid_transport_venue_id(),
            detail: "system clock milliseconds overflow u64".to_owned(),
        })
    }

    fn hyperliquid_request_url(base_url: &str, endpoint: &'static str) -> VenueExecResult<String> {
        if endpoint.is_empty() || !endpoint.starts_with('/') {
            return Err(VenueExecError::InvalidRequest {
                field: "endpoint",
                reason: "Hyperliquid REST endpoint must be an absolute path",
            });
        }
        Ok(format!("{base_url}{endpoint}"))
    }

    fn parse_hyperliquid_curl_http_response(
        stdout: &[u8],
    ) -> VenueExecResult<HyperliquidExecHttpResponse> {
        let output = String::from_utf8_lossy(stdout);
        let Some((body, status)) = output.rsplit_once(CURL_HYPERLIQUID_STATUS_MARKER) else {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: hyperliquid_transport_venue_id(),
                detail: "curl transport response lacked an HTTP status marker".to_owned(),
            });
        };
        let status_code =
            status
                .trim()
                .parse::<u16>()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: hyperliquid_transport_venue_id(),
                    detail: "curl transport returned a malformed HTTP status".to_owned(),
                })?;
        if status_code == 0 {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: hyperliquid_transport_venue_id(),
                detail: "curl transport did not receive an HTTP response from Hyperliquid"
                    .to_owned(),
            });
        }
        Ok(HyperliquidExecHttpResponse::new(
            status_code,
            body.to_owned(),
        ))
    }

    fn hyperliquid_transport_venue_id() -> VenueId {
        VenueId::new("venue:HYPERLIQUID-PERP").expect("static Hyperliquid transport venue ID")
    }

    fn json_string_literal(value: &str) -> VenueExecResult<String> {
        let mut body = String::new();
        body.push('"');
        push_json_escaped(&mut body, value)?;
        body.push('"');
        Ok(body)
    }

    /// Bybit V5 下单 endpoint。
    pub const BYBIT_ORDER_CREATE_ENDPOINT: &str = "/v5/order/create";
    /// Bybit V5 设置杠杆 endpoint。
    pub const BYBIT_SET_LEVERAGE_ENDPOINT: &str = "/v5/position/set-leverage";
    /// Bybit V5 撤单 endpoint。
    pub const BYBIT_ORDER_CANCEL_ENDPOINT: &str = "/v5/order/cancel";
    /// Bybit V5 查单 endpoint。
    pub const BYBIT_ORDER_REALTIME_ENDPOINT: &str = "/v5/order/realtime";
    /// 默认 Bybit signed endpoint 接收窗口。
    pub const DEFAULT_BYBIT_RECV_WINDOW_MS: u64 = 5_000;
    const MAX_BYBIT_RECV_WINDOW_MS: u64 = 60_000;
    const CURL_BYBIT_STATUS_MARKER: &str = "\n__ARB_BYBIT_HTTP_STATUS__:";

    /// Bybit 可变执行市场。
    ///
    /// 中文说明：该枚举只覆盖当前接入范围内的现货和 USDT 线性永续执行路径。
    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub enum BybitExecMarket {
        Spot,
        LinearPerpetual,
    }

    impl BybitExecMarket {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Spot => "Spot",
                Self::LinearPerpetual => "LinearPerpetual",
            }
        }

        fn token(self) -> &'static str {
            match self {
                Self::Spot => "bybit-spot",
                Self::LinearPerpetual => "bybit-linear",
            }
        }

        fn category(self) -> &'static str {
            match self {
                Self::Spot => "spot",
                Self::LinearPerpetual => "linear",
            }
        }

        fn expected_instrument_suffix(self) -> &'static str {
            match self {
                Self::Spot => "SPOT",
                Self::LinearPerpetual => "LINEAR-PERP",
            }
        }
    }

    impl fmt::Display for BybitExecMarket {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.as_str())
        }
    }

    /// Bybit 执行适配器配置。
    ///
    /// 中文说明：配置只保存 endpoint、账户引用、签名策略和接收窗口，不保存
    /// API key、secret key、签名 payload 或任何凭证原文。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct BybitExecConfig {
        market: BybitExecMarket,
        venue_id: VenueId,
        account_id: AccountId,
        base_url: String,
        recv_window_ms: u64,
        target_leverage: Option<u32>,
        quantity_step_by_symbol: BTreeMap<String, Quantity>,
        signing_policy: SigningPolicy,
    }

    impl BybitExecConfig {
        pub fn spot(
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            Self::new(
                BybitExecMarket::Spot,
                venue_id,
                account_id,
                base_url,
                DEFAULT_BYBIT_RECV_WINDOW_MS,
                signing_policy,
            )
        }

        pub fn linear_perpetual(
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            Self::new(
                BybitExecMarket::LinearPerpetual,
                venue_id,
                account_id,
                base_url,
                DEFAULT_BYBIT_RECV_WINDOW_MS,
                signing_policy,
            )
        }

        pub fn new(
            market: BybitExecMarket,
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            recv_window_ms: u64,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            validate_bybit_recv_window(recv_window_ms)?;
            Ok(Self {
                market,
                venue_id,
                account_id,
                base_url: normalize_bybit_base_url(base_url.into())?,
                recv_window_ms,
                target_leverage: None,
                quantity_step_by_symbol: BTreeMap::new(),
                signing_policy,
            })
        }

        pub fn with_recv_window_ms(mut self, recv_window_ms: u64) -> VenueExecResult<Self> {
            validate_bybit_recv_window(recv_window_ms)?;
            self.recv_window_ms = recv_window_ms;
            Ok(self)
        }

        pub fn with_quantity_step(
            mut self,
            symbol: impl Into<String>,
            step: Quantity,
        ) -> VenueExecResult<Self> {
            let symbol = symbol.into();
            validate_bybit_symbol(&symbol)?;
            validate_venue_quantity_step(step)?;
            self.quantity_step_by_symbol.insert(symbol, step);
            Ok(self)
        }

        pub fn with_target_leverage(mut self, leverage: u32) -> VenueExecResult<Self> {
            if self.market != BybitExecMarket::LinearPerpetual {
                return Err(VenueExecError::InvalidRequest {
                    field: "target_leverage",
                    reason: "Bybit target leverage can only be configured for linear perpetuals",
                });
            }
            validate_perp_target_leverage("target_leverage", leverage)?;
            self.target_leverage = Some(leverage);
            Ok(self)
        }

        pub fn market(&self) -> BybitExecMarket {
            self.market
        }

        pub fn venue_id(&self) -> &VenueId {
            &self.venue_id
        }

        pub fn account_id(&self) -> &AccountId {
            &self.account_id
        }

        pub fn base_url(&self) -> &str {
            &self.base_url
        }

        pub fn recv_window_ms(&self) -> u64 {
            self.recv_window_ms
        }

        pub fn quantity_step(&self, symbol: &str) -> Option<Quantity> {
            self.quantity_step_by_symbol.get(symbol).copied()
        }

        pub fn target_leverage(&self) -> Option<u32> {
            self.target_leverage
        }

        pub fn signing_policy(&self) -> &SigningPolicy {
            &self.signing_policy
        }
    }

    /// Bybit signed REST HTTP 方法。
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BybitExecHttpMethod {
        Get,
        Post,
    }

    impl BybitExecHttpMethod {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Get => "GET",
                Self::Post => "POST",
            }
        }
    }

    impl fmt::Display for BybitExecHttpMethod {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.as_str())
        }
    }

    /// 已签名 Bybit HTTP 请求。
    ///
    /// 中文说明：transport 只能通过该对象取得发送所需的认证头和 query/body。
    /// `Debug` 永远脱敏，避免日志输出凭证或签名材料。
    pub struct BybitSignedRequest<'a> {
        market: BybitExecMarket,
        method: BybitExecHttpMethod,
        base_url: &'a str,
        endpoint: &'static str,
        signed_endpoint: &'a BybitSignedEndpoint,
    }

    impl BybitSignedRequest<'_> {
        pub fn market(&self) -> BybitExecMarket {
            self.market
        }

        pub fn method(&self) -> BybitExecHttpMethod {
            self.method
        }

        pub fn base_url(&self) -> &str {
            self.base_url
        }

        pub fn endpoint(&self) -> &'static str {
            self.endpoint
        }

        pub fn api_key_header_name(&self) -> &'static str {
            self.signed_endpoint.api_key_header_name()
        }

        pub fn api_key_header_value(&self) -> &str {
            self.signed_endpoint.api_key_header_value()
        }

        pub fn timestamp_header_name(&self) -> &'static str {
            self.signed_endpoint.timestamp_header_name()
        }

        pub fn signature_header_name(&self) -> &'static str {
            self.signed_endpoint.signature_header_name()
        }

        pub fn recv_window_header_name(&self) -> &'static str {
            self.signed_endpoint.recv_window_header_name()
        }

        pub fn timestamp_millis(&self) -> u64 {
            self.signed_endpoint.timestamp_millis()
        }

        pub fn recv_window_ms(&self) -> u64 {
            self.signed_endpoint.recv_window_ms()
        }

        pub fn payload_kind(&self) -> BybitSigningPayloadKind {
            self.signed_endpoint.payload_kind()
        }

        pub fn payload_for_transport(&self) -> &str {
            self.signed_endpoint.payload_for_transport()
        }

        pub fn signature_header_value(&self) -> &str {
            self.signed_endpoint.signature_header_value()
        }
    }

    impl fmt::Debug for BybitSignedRequest<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BybitSignedRequest")
                .field("market", &self.market)
                .field("method", &self.method)
                .field("base_url", &self.base_url)
                .field("endpoint", &self.endpoint)
                .field("api_key_header_name", &self.api_key_header_name())
                .field("api_key_header_value", &"<redacted>")
                .field("timestamp_header_name", &self.timestamp_header_name())
                .field("signature_header_name", &self.signature_header_name())
                .field("recv_window_header_name", &self.recv_window_header_name())
                .field("timestamp_millis", &self.timestamp_millis())
                .field("recv_window_ms", &self.recv_window_ms())
                .field("payload_kind", &self.payload_kind())
                .field("payload", &"<redacted>")
                .field("signature", &"<redacted>")
                .finish()
        }
    }

    /// Bybit transport 返回。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct BybitExecHttpResponse {
        status_code: u16,
        body: String,
    }

    impl BybitExecHttpResponse {
        pub fn new(status_code: u16, body: impl Into<String>) -> Self {
            Self {
                status_code,
                body: body.into(),
            }
        }

        pub fn status_code(&self) -> u16 {
            self.status_code
        }

        pub fn body(&self) -> &str {
            &self.body
        }

        pub fn is_success(&self) -> bool {
            (200..=299).contains(&self.status_code)
        }
    }

    /// Bybit 可变执行 transport。
    ///
    /// 中文说明：适配器负责风控之后的请求映射、签名和幂等；具体 HTTP/TLS、
    /// 重试、限频和代理由运行时注入的 transport 实现。transport 遇到网络
    /// 断连或不确定提交状态时必须返回 `UnknownExternalState` 类错误。
    pub trait BybitExecTransport {
        fn send_signed(
            &mut self,
            request: BybitSignedRequest<'_>,
        ) -> VenueExecResult<BybitExecHttpResponse>;
    }

    /// 使用系统 `curl` 发送 Bybit signed REST 请求的真实 transport。
    ///
    /// 中文说明：该实现把 URL、认证头和 body 通过 `curl --config -` 的标准输入
    /// 传给 curl，避免把凭证材料暴露在进程命令行参数中。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct BybitCurlExecTransport {
        connect_timeout_secs: u64,
        max_time_secs: u64,
    }

    impl BybitCurlExecTransport {
        pub fn new(connect_timeout_secs: u64, max_time_secs: u64) -> VenueExecResult<Self> {
            if connect_timeout_secs == 0 || max_time_secs == 0 {
                return Err(VenueExecError::InvalidRequest {
                    field: "curl_timeout",
                    reason: "curl timeouts must be greater than zero",
                });
            }
            Ok(Self {
                connect_timeout_secs,
                max_time_secs,
            })
        }

        pub fn connect_timeout_secs(&self) -> u64 {
            self.connect_timeout_secs
        }

        pub fn max_time_secs(&self) -> u64 {
            self.max_time_secs
        }
    }

    impl Default for BybitCurlExecTransport {
        fn default() -> Self {
            Self {
                connect_timeout_secs: 10,
                max_time_secs: 30,
            }
        }
    }

    impl BybitExecTransport for BybitCurlExecTransport {
        fn send_signed(
            &mut self,
            request: BybitSignedRequest<'_>,
        ) -> VenueExecResult<BybitExecHttpResponse> {
            let venue_id = bybit_transport_venue_id(request.market());
            let url = bybit_signed_request_url(&request)?;
            let config = bybit_curl_config(&request, &url)?;
            let mut child = Command::new("curl")
                .arg("--silent")
                .arg("--show-error")
                .arg("--request")
                .arg(request.method().as_str())
                .arg("--connect-timeout")
                .arg(self.connect_timeout_secs.to_string())
                .arg("--max-time")
                .arg(self.max_time_secs.to_string())
                .arg("--write-out")
                .arg(format!("{CURL_BYBIT_STATUS_MARKER}%{{http_code}}"))
                .arg("--config")
                .arg("-")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "failed to start curl for Bybit signed REST request".to_owned(),
                })?;

            child
                .stdin
                .as_mut()
                .ok_or_else(|| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "curl stdin is unavailable for Bybit signed REST request".to_owned(),
                })?
                .write_all(config.as_bytes())
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "failed to write curl config for Bybit signed REST request".to_owned(),
                })?;

            let output =
                child
                    .wait_with_output()
                    .map_err(|_| VenueExecError::UnknownExternalState {
                        venue_id: venue_id.clone(),
                        detail: "curl transport did not return a Bybit signed REST response"
                            .to_owned(),
                    })?;
            if !output.status.success() {
                return Err(VenueExecError::UnknownExternalState {
                    venue_id,
                    detail: "curl transport failed before a reliable HTTP response was available"
                        .to_owned(),
                });
            }

            parse_bybit_curl_http_response(&output.stdout, request.market())
        }
    }

    /// Bybit Spot 可变执行适配器。
    pub struct BybitSpotExecAdapter<S, T> {
        inner: BybitExecAdapterCore<S, T>,
    }

    impl<S, T> BybitSpotExecAdapter<S, T> {
        pub fn new(config: BybitExecConfig, signer: S, transport: T) -> VenueExecResult<Self> {
            ensure_bybit_config_market(&config, BybitExecMarket::Spot)?;
            Ok(Self {
                inner: BybitExecAdapterCore::new(config, signer, transport),
            })
        }

        pub fn config(&self) -> &BybitExecConfig {
            self.inner.config()
        }

        pub fn transport(&self) -> &T {
            self.inner.transport()
        }

        pub fn transport_mut(&mut self) -> &mut T {
            self.inner.transport_mut()
        }
    }

    impl<S, T> fmt::Debug for BybitSpotExecAdapter<S, T>
    where
        S: fmt::Debug,
        T: fmt::Debug,
    {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BybitSpotExecAdapter")
                .field("inner", &self.inner)
                .finish()
        }
    }

    /// Bybit 线性永续可变执行适配器。
    pub struct BybitLinearExecAdapter<S, T> {
        inner: BybitExecAdapterCore<S, T>,
    }

    impl<S, T> BybitLinearExecAdapter<S, T> {
        pub fn new(config: BybitExecConfig, signer: S, transport: T) -> VenueExecResult<Self> {
            ensure_bybit_config_market(&config, BybitExecMarket::LinearPerpetual)?;
            Ok(Self {
                inner: BybitExecAdapterCore::new(config, signer, transport),
            })
        }

        pub fn config(&self) -> &BybitExecConfig {
            self.inner.config()
        }

        pub fn transport(&self) -> &T {
            self.inner.transport()
        }

        pub fn transport_mut(&mut self) -> &mut T {
            self.inner.transport_mut()
        }
    }

    impl<S, T> fmt::Debug for BybitLinearExecAdapter<S, T>
    where
        S: fmt::Debug,
        T: fmt::Debug,
    {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BybitLinearExecAdapter")
                .field("inner", &self.inner)
                .finish()
        }
    }

    impl<S, T> SubmitOrder for BybitSpotExecAdapter<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.submit_order(request)
        }
    }

    impl<S, T> CancelOrder for BybitSpotExecAdapter<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.cancel_order(request)
        }
    }

    impl<S, T> QueryActionStatus for BybitSpotExecAdapter<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            self.inner.query_action_status(request)
        }
    }

    impl<S, T> ConfirmOrderStatus for BybitSpotExecAdapter<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.inner.confirm_order_status(request)
        }
    }

    impl<S, T> RequestTransfer for BybitSpotExecAdapter<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.request_transfer(request)
        }
    }

    impl<S, T> SubmitOrder for BybitLinearExecAdapter<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.submit_order(request)
        }
    }

    impl<S, T> CancelOrder for BybitLinearExecAdapter<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.cancel_order(request)
        }
    }

    impl<S, T> QueryActionStatus for BybitLinearExecAdapter<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            self.inner.query_action_status(request)
        }
    }

    impl<S, T> ConfirmOrderStatus for BybitLinearExecAdapter<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.inner.confirm_order_status(request)
        }
    }

    impl<S, T> RequestTransfer for BybitLinearExecAdapter<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.inner.request_transfer(request)
        }
    }

    #[derive(Debug)]
    struct BybitExecAdapterCore<S, T> {
        config: BybitExecConfig,
        signer: S,
        transport: T,
        records_by_key: BTreeMap<IdempotencyKey, LiveActionRecord>,
        key_by_action_id: BTreeMap<MutableActionId, IdempotencyKey>,
        orders_by_client_id: BTreeMap<OrderId, BybitKnownOrder>,
        orders_by_external_id: BTreeMap<ExternalOrderId, BybitKnownOrder>,
        next_sequence: u64,
    }

    impl<S, T> BybitExecAdapterCore<S, T> {
        fn new(config: BybitExecConfig, signer: S, transport: T) -> Self {
            Self {
                config,
                signer,
                transport,
                records_by_key: BTreeMap::new(),
                key_by_action_id: BTreeMap::new(),
                orders_by_client_id: BTreeMap::new(),
                orders_by_external_id: BTreeMap::new(),
                next_sequence: 0,
            }
        }

        fn config(&self) -> &BybitExecConfig {
            &self.config
        }

        fn transport(&self) -> &T {
            &self.transport
        }

        fn transport_mut(&mut self) -> &mut T {
            &mut self.transport
        }

        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            Ok(match request {
                QueryActionStatusRequest::ByActionId(action_id) => {
                    if let Some(key) = self.key_by_action_id.get(&action_id) {
                        self.report_for_key(key)
                    } else {
                        unknown_status_report(Some(action_id), None)
                    }
                }
                QueryActionStatusRequest::ByIdempotencyKey(key) => self.report_for_key(&key),
            })
        }

        fn report_for_key(&self, key: &IdempotencyKey) -> MutableActionStatusReport {
            self.records_by_key.get(key).map_or_else(
                || unknown_status_report(None, Some(key.clone())),
                |record| super::status_report_from_receipt(&record.receipt),
            )
        }

        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.from_account_id)?;
            Err(VenueExecError::InvalidRequest {
                field: "transfer",
                reason: "Bybit live transfer is not implemented by this execution adapter",
            })
        }

        fn ensure_request_scope(
            &self,
            venue_id: &VenueId,
            account_id: &AccountId,
        ) -> VenueExecResult<()> {
            if venue_id != &self.config.venue_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "venue_id",
                    reason: "request venue does not match Bybit execution adapter config",
                });
            }
            if account_id != &self.config.account_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "account_id",
                    reason: "request account does not match Bybit execution adapter config",
                });
            }
            Ok(())
        }
    }

    impl<S, T> BybitExecAdapterCore<S, T>
    where
        S: BybitRealSigningProvider,
        T: BybitExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            request.validate()?;
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;

            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }

            let symbol = bybit_symbol_from_instrument(self.config.market, &request.instrument_id)?;
            self.ensure_target_leverage(&symbol, request.reduce_only)?;
            let body = bybit_order_create_body(&self.config, &symbol, &request)?;
            let action_id = self.next_action_id(MutableActionKind::SubmitOrder)?;
            let signed = self.sign(
                SigningPurpose::SubmitOrder,
                &action_id,
                BybitSigningPayloadKind::JsonBody,
                body,
            )?;
            let response = self.dispatch_signed(
                BybitExecHttpMethod::Post,
                BYBIT_ORDER_CREATE_ENDPOINT,
                &signed,
            )?;
            self.ensure_business_success(BYBIT_ORDER_CREATE_ENDPOINT, &response)?;

            let known_order = bybit_known_order_from_response(
                self.config.market,
                &symbol,
                request.client_order_id.clone(),
                response.body(),
                &action_id,
            )?;
            let external_ref = known_order
                .external_order_id
                .clone()
                .map(ExternalActionRef::Order);
            let receipt = MutableActionReceipt {
                action_id,
                kind: MutableActionKind::SubmitOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref,
                duplicate: false,
                simulated: false,
            };

            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            self.record_known_order(known_order);
            Ok(receipt)
        }

        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }

            let known_order = self.lookup_known_order(&request.order_ref).ok_or(
                VenueExecError::InvalidRequest {
                    field: "order_ref",
                    reason: "Bybit cancel requires an order previously submitted through this adapter so its symbol is known",
                },
            )?;
            let body = bybit_cancel_order_body(&self.config, &request, known_order)?;
            let action_id = self.next_action_id(MutableActionKind::CancelOrder)?;
            let signed = self.sign(
                SigningPurpose::CancelOrder,
                &action_id,
                BybitSigningPayloadKind::JsonBody,
                body,
            )?;
            let response = self.dispatch_signed(
                BybitExecHttpMethod::Post,
                BYBIT_ORDER_CANCEL_ENDPOINT,
                &signed,
            )?;
            self.ensure_business_success(BYBIT_ORDER_CANCEL_ENDPOINT, &response)?;

            let receipt = MutableActionReceipt {
                action_id: action_id.clone(),
                kind: MutableActionKind::CancelOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref: Some(ExternalActionRef::Cancel(action_id)),
                duplicate: false,
                simulated: false,
            };
            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            Ok(receipt)
        }

        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let symbol = bybit_symbol_from_instrument(self.config.market, &request.instrument_id)?;
            let query = bybit_query_order_payload(&self.config, &symbol, &request.order_ref)?;
            let signing_request_id =
                order_query_signing_request_id("bybit-exec", &request.source_event_id)?;
            let signed = self.sign_with_request_id(
                SigningPurpose::QueryOrder,
                signing_request_id,
                BybitSigningPayloadKind::QueryString,
                query,
            )?;
            let response = self.dispatch_signed(
                BybitExecHttpMethod::Get,
                BYBIT_ORDER_REALTIME_ENDPOINT,
                &signed,
            )?;
            self.ensure_http_success(BYBIT_ORDER_REALTIME_ENDPOINT, &response)?;
            parse_bybit_order_query_confirmation(
                private_market_from_bybit_exec_market(self.config.market),
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                request.source_event_id,
                response.body(),
            )
        }

        fn ensure_target_leverage(
            &mut self,
            symbol: &str,
            reduce_only: bool,
        ) -> VenueExecResult<()> {
            if reduce_only || self.config.market != BybitExecMarket::LinearPerpetual {
                return Ok(());
            }
            let Some(leverage) = self.config.target_leverage else {
                return Ok(());
            };
            let body = bybit_set_leverage_body(&self.config, symbol, leverage)?;
            let signing_request_id = leverage_signing_request_id("bybit-exec", symbol, leverage)?;
            let signed = self.sign_with_request_id(
                SigningPurpose::SubmitOrder,
                signing_request_id,
                BybitSigningPayloadKind::JsonBody,
                body,
            )?;
            let response = self.dispatch_signed(
                BybitExecHttpMethod::Post,
                BYBIT_SET_LEVERAGE_ENDPOINT,
                &signed,
            )?;
            self.ensure_bybit_set_leverage_success(&response)
        }

        fn duplicate_receipt(
            &self,
            idempotency_key: &IdempotencyKey,
            fingerprint: &RequestFingerprint,
        ) -> VenueExecResult<Option<MutableActionReceipt>> {
            let Some(existing) = self.records_by_key.get(idempotency_key) else {
                return Ok(None);
            };
            if existing.fingerprint != *fingerprint {
                return Err(VenueExecError::IdempotencyConflict {
                    idempotency_key: idempotency_key.clone(),
                    existing_fingerprint: existing.fingerprint.0.clone(),
                    incoming_fingerprint: fingerprint.0.clone(),
                });
            }

            let mut receipt = existing.receipt.clone();
            receipt.duplicate = true;
            Ok(Some(receipt))
        }

        fn next_action_id(&mut self, kind: MutableActionKind) -> VenueExecResult<MutableActionId> {
            self.next_sequence = self
                .next_sequence
                .checked_add(1)
                .expect("Bybit mutable action sequence overflowed");
            MutableActionId::new(format!(
                "{}:{}:{}",
                self.config.market.token(),
                kind.as_str(),
                self.next_sequence
            ))
        }

        fn sign(
            &self,
            purpose: SigningPurpose,
            action_id: &MutableActionId,
            payload_kind: BybitSigningPayloadKind,
            payload: String,
        ) -> VenueExecResult<BybitSignedEndpoint> {
            self.sign_with_request_id(
                purpose,
                SigningRequestId::new(format!("signing-request/bybit-exec/{}", action_id.as_str()))
                    .map_err(signing_error)?,
                payload_kind,
                payload,
            )
        }

        fn sign_with_request_id(
            &self,
            purpose: SigningPurpose,
            signing_request_id: SigningRequestId,
            payload_kind: BybitSigningPayloadKind,
            payload: String,
        ) -> VenueExecResult<BybitSignedEndpoint> {
            let input = BybitHmacSigningInput::new(
                signing_request_id,
                self.config.signing_policy.policy_ref().clone(),
                purpose,
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                self.config.recv_window_ms,
                payload_kind,
                payload,
            )
            .map_err(signing_error)?;
            self.signer
                .sign_bybit_hmac(input, &self.config.signing_policy)
                .map_err(signing_error)
        }

        fn dispatch_signed(
            &mut self,
            method: BybitExecHttpMethod,
            endpoint: &'static str,
            signed_endpoint: &BybitSignedEndpoint,
        ) -> VenueExecResult<BybitExecHttpResponse> {
            let request = BybitSignedRequest {
                market: self.config.market,
                method,
                base_url: &self.config.base_url,
                endpoint,
                signed_endpoint,
            };
            self.transport.send_signed(request)
        }

        fn ensure_http_success(
            &self,
            endpoint: &'static str,
            response: &BybitExecHttpResponse,
        ) -> VenueExecResult<()> {
            if response.is_success() {
                return Ok(());
            }
            Err(VenueExecError::ExternalRejected {
                venue_id: self.config.venue_id.clone(),
                endpoint: endpoint.to_owned(),
                status_code: response.status_code(),
                reason: response_body_snippet(response.body()),
            })
        }

        fn ensure_business_success(
            &self,
            endpoint: &'static str,
            response: &BybitExecHttpResponse,
        ) -> VenueExecResult<()> {
            self.ensure_http_success(endpoint, response)?;
            match json_field_value(response.body(), "retCode").as_deref() {
                Some("0") => Ok(()),
                Some(ret_code) => Err(VenueExecError::ExternalRejected {
                    venue_id: self.config.venue_id.clone(),
                    endpoint: endpoint.to_owned(),
                    status_code: response.status_code(),
                    reason: format!(
                        "Bybit retCode={ret_code}: {}",
                        json_field_value(response.body(), "retMsg")
                            .unwrap_or_else(|| "missing retMsg".to_owned())
                    ),
                }),
                None => Err(VenueExecError::UnknownExternalState {
                    venue_id: self.config.venue_id.clone(),
                    detail: format!("Bybit response from {endpoint} lacks retCode"),
                }),
            }
        }

        fn ensure_bybit_set_leverage_success(
            &self,
            response: &BybitExecHttpResponse,
        ) -> VenueExecResult<()> {
            self.ensure_http_success(BYBIT_SET_LEVERAGE_ENDPOINT, response)?;
            match json_field_value(response.body(), "retCode").as_deref() {
                Some("0" | "110043") => Ok(()),
                Some(ret_code) => Err(VenueExecError::ExternalRejected {
                    venue_id: self.config.venue_id.clone(),
                    endpoint: BYBIT_SET_LEVERAGE_ENDPOINT.to_owned(),
                    status_code: response.status_code(),
                    reason: format!(
                        "Bybit retCode={ret_code}: {}",
                        json_field_value(response.body(), "retMsg")
                            .unwrap_or_else(|| "missing retMsg".to_owned())
                    ),
                }),
                None => Err(VenueExecError::UnknownExternalState {
                    venue_id: self.config.venue_id.clone(),
                    detail: format!(
                        "Bybit response from {BYBIT_SET_LEVERAGE_ENDPOINT} lacks retCode"
                    ),
                }),
            }
        }

        fn record_action(
            &mut self,
            idempotency_key: IdempotencyKey,
            fingerprint: RequestFingerprint,
            receipt: MutableActionReceipt,
        ) {
            self.key_by_action_id
                .insert(receipt.action_id.clone(), idempotency_key.clone());
            self.records_by_key.insert(
                idempotency_key,
                LiveActionRecord {
                    fingerprint,
                    receipt,
                },
            );
        }

        fn record_known_order(&mut self, known_order: BybitKnownOrder) {
            if let Some(client_order_id) = known_order.client_order_id.clone() {
                self.orders_by_client_id
                    .insert(client_order_id, known_order.clone());
            }
            if let Some(external_order_id) = known_order.external_order_id.clone() {
                self.orders_by_external_id
                    .insert(external_order_id, known_order);
            }
        }

        fn lookup_known_order(&self, order_ref: &OrderReference) -> Option<&BybitKnownOrder> {
            match order_ref {
                OrderReference::ClientOrderId(order_id) => self.orders_by_client_id.get(order_id),
                OrderReference::VenueOrderId(order_id) => self.orders_by_external_id.get(order_id),
            }
        }
    }

    fn bybit_set_leverage_body(
        config: &BybitExecConfig,
        symbol: &str,
        leverage: u32,
    ) -> VenueExecResult<String> {
        validate_perp_target_leverage("target_leverage", leverage)?;
        let leverage = leverage.to_string();
        let mut body = String::from("{");
        let mut first = true;
        push_json_string_field(&mut body, &mut first, "category", config.market.category())?;
        push_json_string_field(&mut body, &mut first, "symbol", symbol)?;
        push_json_string_field(&mut body, &mut first, "buyLeverage", &leverage)?;
        push_json_string_field(&mut body, &mut first, "sellLeverage", &leverage)?;
        body.push('}');
        Ok(body)
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct BybitKnownOrder {
        symbol: String,
        order_id_param: Option<String>,
        client_order_id: Option<OrderId>,
        external_order_id: Option<ExternalOrderId>,
    }

    fn ensure_bybit_config_market(
        config: &BybitExecConfig,
        expected: BybitExecMarket,
    ) -> VenueExecResult<()> {
        if config.market == expected {
            Ok(())
        } else {
            Err(VenueExecError::InvalidRequest {
                field: "market",
                reason: "Bybit execution adapter received config for a different market",
            })
        }
    }

    fn validate_bybit_recv_window(recv_window_ms: u64) -> VenueExecResult<()> {
        if (1..=MAX_BYBIT_RECV_WINDOW_MS).contains(&recv_window_ms) {
            Ok(())
        } else {
            Err(VenueExecError::InvalidRequest {
                field: "recv_window_ms",
                reason: "Bybit recvWindow must be between 1 and 60000 milliseconds",
            })
        }
    }

    fn normalize_bybit_base_url(value: String) -> VenueExecResult<String> {
        let trimmed = value.trim().trim_end_matches('/').to_owned();
        if trimmed.is_empty() {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Bybit base URL cannot be empty",
            });
        }
        if trimmed
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Bybit base URL contains a control byte",
            });
        }
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://127.0.0.1")) {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Bybit base URL must use https or an explicit localhost test URL",
            });
        }
        Ok(trimmed)
    }

    fn bybit_order_create_body(
        config: &BybitExecConfig,
        symbol: &str,
        request: &SubmitOrderRequest,
    ) -> VenueExecResult<String> {
        let quantity = bybit_order_quantity(config, symbol, request.quantity)?;
        let mut body = String::from("{");
        let mut first = true;
        push_json_string_field(&mut body, &mut first, "category", config.market.category())?;
        push_json_string_field(&mut body, &mut first, "symbol", symbol)?;
        push_json_string_field(&mut body, &mut first, "side", bybit_side(request.side))?;
        match request.order_type {
            MutableOrderType::Market => {
                push_json_string_field(&mut body, &mut first, "orderType", "Market")?;
                push_json_string_field(&mut body, &mut first, "qty", &quantity)?;
                if config.market == BybitExecMarket::Spot {
                    push_json_string_field(&mut body, &mut first, "marketUnit", "baseCoin")?;
                }
            }
            MutableOrderType::Limit => {
                push_json_string_field(&mut body, &mut first, "orderType", "Limit")?;
                push_json_string_field(&mut body, &mut first, "qty", &quantity)?;
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "price",
                    &request
                        .limit_price
                        .expect("validated limit order price")
                        .to_string(),
                )?;
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "timeInForce",
                    bybit_time_in_force(limit_time_in_force(request)),
                )?;
            }
            MutableOrderType::PostOnly => {
                push_json_string_field(&mut body, &mut first, "orderType", "Limit")?;
                push_json_string_field(&mut body, &mut first, "qty", &quantity)?;
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "price",
                    &request
                        .limit_price
                        .expect("validated post-only order price")
                        .to_string(),
                )?;
                push_json_string_field(&mut body, &mut first, "timeInForce", "PostOnly")?;
            }
        }
        if request.reduce_only {
            match config.market {
                BybitExecMarket::Spot => {
                    return Err(VenueExecError::InvalidRequest {
                        field: "reduce_only",
                        reason: "Bybit spot orders do not support reduce_only",
                    });
                }
                BybitExecMarket::LinearPerpetual => {
                    push_json_bool_field(&mut body, &mut first, "reduceOnly", true);
                }
            }
        }
        if let Some(client_order_id) = &request.client_order_id {
            validate_bybit_client_order_id(client_order_id.as_str())?;
            push_json_string_field(
                &mut body,
                &mut first,
                "orderLinkId",
                client_order_id.as_str(),
            )?;
        }
        body.push('}');
        Ok(body)
    }

    fn bybit_order_quantity(
        config: &BybitExecConfig,
        symbol: &str,
        quantity: Quantity,
    ) -> VenueExecResult<String> {
        match config.quantity_step(symbol) {
            Some(step) => format_quantity_at_venue_step(quantity, step),
            None => Ok(quantity.to_string()),
        }
    }

    fn bybit_cancel_order_body(
        config: &BybitExecConfig,
        request: &CancelOrderRequest,
        known_order: &BybitKnownOrder,
    ) -> VenueExecResult<String> {
        let mut body = String::from("{");
        let mut first = true;
        push_json_string_field(&mut body, &mut first, "category", config.market.category())?;
        push_json_string_field(&mut body, &mut first, "symbol", known_order.symbol.as_str())?;
        match &request.order_ref {
            OrderReference::VenueOrderId(_) => {
                if let Some(order_id) = &known_order.order_id_param {
                    push_json_string_field(&mut body, &mut first, "orderId", order_id)?;
                } else if let Some(client_order_id) = &known_order.client_order_id {
                    push_json_string_field(
                        &mut body,
                        &mut first,
                        "orderLinkId",
                        client_order_id.as_str(),
                    )?;
                } else {
                    return Err(VenueExecError::InvalidRequest {
                        field: "order_ref",
                        reason: "known Bybit venue order lacks orderId and orderLinkId",
                    });
                }
            }
            OrderReference::ClientOrderId(client_order_id) => {
                validate_bybit_client_order_id(client_order_id.as_str())?;
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "orderLinkId",
                    client_order_id.as_str(),
                )?;
            }
        }
        body.push('}');
        Ok(body)
    }

    fn bybit_query_order_payload(
        config: &BybitExecConfig,
        symbol: &str,
        order_ref: &OrderReference,
    ) -> VenueExecResult<String> {
        let mut params = vec![
            ("category".to_owned(), config.market.category().to_owned()),
            ("symbol".to_owned(), symbol.to_owned()),
        ];
        match order_ref {
            OrderReference::VenueOrderId(order_id) => {
                let raw_order_id = bybit_order_id_param_from_external(config.market, order_id)?;
                params.push(("orderId".to_owned(), raw_order_id.to_owned()));
            }
            OrderReference::ClientOrderId(client_order_id) => {
                validate_bybit_client_order_id(client_order_id.as_str())?;
                params.push((
                    "orderLinkId".to_owned(),
                    client_order_id.as_str().to_owned(),
                ));
            }
        }
        Ok(query_string_from_pairs(&params))
    }

    fn bybit_order_id_param_from_external(
        market: BybitExecMarket,
        order_id: &ExternalOrderId,
    ) -> VenueExecResult<&str> {
        let expected_prefix = format!("{}:order:", market.token());
        let value = order_id.as_str();
        let Some(raw_order_id) = value.strip_prefix(&expected_prefix) else {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "Bybit venue order ref must come from the same market adapter",
            });
        };
        validate_bybit_order_id(raw_order_id)?;
        Ok(raw_order_id)
    }

    fn private_market_from_bybit_exec_market(market: BybitExecMarket) -> PrivateOrderMarket {
        match market {
            BybitExecMarket::Spot => PrivateOrderMarket::BybitSpot,
            BybitExecMarket::LinearPerpetual => PrivateOrderMarket::BybitLinear,
        }
    }

    fn bybit_symbol_from_instrument(
        market: BybitExecMarket,
        instrument_id: &InstrumentId,
    ) -> VenueExecResult<String> {
        let value = instrument_id.as_str();
        let mut parts = value.split(':');
        let prefix = parts.next();
        let venue = parts.next();
        let symbol = parts.next();
        let suffix = parts.next();
        if parts.next().is_some()
            || prefix != Some("inst")
            || venue != Some("BYBIT")
            || suffix != Some(market.expected_instrument_suffix())
        {
            return Err(VenueExecError::InvalidRequest {
                field: "instrument_id",
                reason: "Bybit execution requires instrument IDs shaped as inst:BYBIT:<SYMBOL>:SPOT or inst:BYBIT:<SYMBOL>:LINEAR-PERP",
            });
        }
        let symbol = symbol.expect("symbol checked above");
        validate_bybit_symbol(symbol)?;
        Ok(symbol.to_owned())
    }

    fn validate_bybit_symbol(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 32 {
            return Err(VenueExecError::InvalidRequest {
                field: "symbol",
                reason: "Bybit symbol must be 1 to 32 bytes",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit()))
        {
            return Err(VenueExecError::InvalidRequest {
                field: "symbol",
                reason: "Bybit symbol must use uppercase ASCII letters and digits",
            });
        }
        Ok(())
    }

    fn validate_bybit_client_order_id(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 36 {
            return Err(VenueExecError::InvalidRequest {
                field: "client_order_id",
                reason: "Bybit orderLinkId must be 1 to 36 bytes",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
        {
            return Err(VenueExecError::InvalidRequest {
                field: "client_order_id",
                reason: "Bybit orderLinkId must use ASCII letters, digits, dash or underscore",
            });
        }
        Ok(())
    }

    fn validate_bybit_order_id(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 96 {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "Bybit orderId must be 1 to 96 bytes",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
        {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "Bybit orderId contains an unsupported byte",
            });
        }
        Ok(())
    }

    fn bybit_known_order_from_response(
        market: BybitExecMarket,
        symbol: &str,
        client_order_id: Option<OrderId>,
        body: &str,
        action_id: &MutableActionId,
    ) -> VenueExecResult<BybitKnownOrder> {
        let order_id_param = json_field_value(body, "orderId").filter(|value| !value.is_empty());
        let response_client_order_id = json_field_value(body, "orderLinkId")
            .filter(|value| !value.is_empty())
            .and_then(|value| OrderId::new(value).ok());
        let client_order_id = client_order_id.or(response_client_order_id);
        let external_order_id = if let Some(order_id) = &order_id_param {
            Some(ExternalOrderId::new(format!(
                "{}:order:{order_id}",
                market.token()
            ))?)
        } else if let Some(client_order_id) = &client_order_id {
            Some(ExternalOrderId::new(format!(
                "{}:client:{}",
                market.token(),
                client_order_id.as_str()
            ))?)
        } else {
            Some(ExternalOrderId::new(format!(
                "{}:action:{}",
                market.token(),
                action_id.as_str()
            ))?)
        };
        Ok(BybitKnownOrder {
            symbol: symbol.to_owned(),
            order_id_param,
            client_order_id,
            external_order_id,
        })
    }

    fn bybit_side(side: OrderSide) -> &'static str {
        match side {
            OrderSide::Buy => "Buy",
            OrderSide::Sell => "Sell",
        }
    }

    fn bybit_time_in_force(time_in_force: MutableTimeInForce) -> &'static str {
        match time_in_force {
            MutableTimeInForce::Gtc => "GTC",
            MutableTimeInForce::Ioc => "IOC",
            MutableTimeInForce::Fok => "FOK",
        }
    }

    fn push_json_string_field(
        body: &mut String,
        first: &mut bool,
        name: &str,
        value: &str,
    ) -> VenueExecResult<()> {
        if !*first {
            body.push(',');
        }
        *first = false;
        body.push('"');
        body.push_str(name);
        body.push_str("\":\"");
        push_json_escaped(body, value)?;
        body.push('"');
        Ok(())
    }

    fn push_json_bool_field(body: &mut String, first: &mut bool, name: &str, value: bool) {
        if !*first {
            body.push(',');
        }
        *first = false;
        body.push('"');
        body.push_str(name);
        body.push_str("\":");
        body.push_str(if value { "true" } else { "false" });
    }

    fn push_json_escaped(body: &mut String, value: &str) -> VenueExecResult<()> {
        for byte in value.bytes() {
            match byte {
                b'"' => body.push_str("\\\""),
                b'\\' => body.push_str("\\\\"),
                0 | b'\n' | b'\r' => {
                    return Err(VenueExecError::InvalidRequest {
                        field: "json_body",
                        reason: "JSON value contains an unsupported control byte",
                    });
                }
                byte if byte.is_ascii_control() => {
                    return Err(VenueExecError::InvalidRequest {
                        field: "json_body",
                        reason: "JSON value contains an unsupported control byte",
                    });
                }
                _ => body.push(byte as char),
            }
        }
        Ok(())
    }

    fn query_string_from_pairs(pairs: &[(String, String)]) -> String {
        let mut query = String::new();
        for (index, (name, value)) in pairs.iter().enumerate() {
            if index > 0 {
                query.push('&');
            }
            query.push_str(&url_encode_component(name));
            query.push('=');
            query.push_str(&url_encode_component(value));
        }
        query
    }

    fn url_encode_component(value: &str) -> String {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut encoded = String::with_capacity(value.len());
        for byte in value.as_bytes() {
            if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_' | b'~') {
                encoded.push(*byte as char);
            } else {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
        encoded
    }

    fn bybit_signed_request_url(request: &BybitSignedRequest<'_>) -> VenueExecResult<String> {
        let base = request.base_url();
        let endpoint = request.endpoint();
        if endpoint.is_empty() || !endpoint.starts_with('/') {
            return Err(VenueExecError::InvalidRequest {
                field: "endpoint",
                reason: "Bybit signed REST endpoint must be an absolute path",
            });
        }
        match request.payload_kind() {
            BybitSigningPayloadKind::QueryString if !request.payload_for_transport().is_empty() => {
                Ok(format!(
                    "{base}{endpoint}?{}",
                    request.payload_for_transport()
                ))
            }
            _ => Ok(format!("{base}{endpoint}")),
        }
    }

    fn bybit_curl_config(request: &BybitSignedRequest<'_>, url: &str) -> VenueExecResult<String> {
        let mut config = format!("url = \"{}\"\n", curl_config_quote(url)?);
        push_curl_header(
            &mut config,
            request.api_key_header_name(),
            request.api_key_header_value(),
        )?;
        push_curl_header(
            &mut config,
            request.timestamp_header_name(),
            &request.timestamp_millis().to_string(),
        )?;
        push_curl_header(
            &mut config,
            request.signature_header_name(),
            request.signature_header_value(),
        )?;
        push_curl_header(
            &mut config,
            request.recv_window_header_name(),
            &request.recv_window_ms().to_string(),
        )?;
        if request.payload_kind() == BybitSigningPayloadKind::JsonBody {
            push_curl_header(&mut config, "Content-Type", "application/json")?;
            config.push_str("data = \"");
            config.push_str(&curl_config_quote(request.payload_for_transport())?);
            config.push_str("\"\n");
        }
        Ok(config)
    }

    fn push_curl_header(config: &mut String, name: &str, value: &str) -> VenueExecResult<()> {
        let header = format!("{name}: {value}");
        config.push_str("header = \"");
        config.push_str(&curl_config_quote(&header)?);
        config.push_str("\"\n");
        Ok(())
    }

    fn parse_bybit_curl_http_response(
        stdout: &[u8],
        market: BybitExecMarket,
    ) -> VenueExecResult<BybitExecHttpResponse> {
        let output = String::from_utf8_lossy(stdout);
        let Some((body, status)) = output.rsplit_once(CURL_BYBIT_STATUS_MARKER) else {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: bybit_transport_venue_id(market),
                detail: "curl transport response lacked an HTTP status marker".to_owned(),
            });
        };
        let status_code =
            status
                .trim()
                .parse::<u16>()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: bybit_transport_venue_id(market),
                    detail: "curl transport returned a malformed HTTP status".to_owned(),
                })?;
        if status_code == 0 {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: bybit_transport_venue_id(market),
                detail: "curl transport did not receive an HTTP response from Bybit".to_owned(),
            });
        }
        Ok(BybitExecHttpResponse::new(status_code, body.to_owned()))
    }

    fn bybit_transport_venue_id(market: BybitExecMarket) -> VenueId {
        let value = match market {
            BybitExecMarket::Spot => "venue:BYBIT-SPOT",
            BybitExecMarket::LinearPerpetual => "venue:BYBIT-LINEAR",
        };
        VenueId::new(value).expect("static Bybit transport venue ID")
    }

    /// OKX V5 下单和查单 endpoint。
    pub const OKX_ORDER_ENDPOINT: &str = "/api/v5/trade/order";
    /// OKX V5 设置杠杆 endpoint。
    pub const OKX_SET_LEVERAGE_ENDPOINT: &str = "/api/v5/account/set-leverage";
    /// OKX V5 撤单 endpoint。
    pub const OKX_CANCEL_ORDER_ENDPOINT: &str = "/api/v5/trade/cancel-order";
    const CURL_OKX_STATUS_MARKER: &str = "\n__ARB_OKX_HTTP_STATUS__:";

    /// OKX 可变执行市场。
    ///
    /// 中文说明：OKX 现货使用 `tdMode=cash`；USDT 永续 swap 默认使用
    /// `tdMode=cross` 和净持仓模式。若账户启用 long/short mode，可通过
    /// `with_pos_side` 显式设置 `posSide`，否则让 OKX 拒绝并失败关闭。
    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub enum OkxExecMarket {
        Spot,
        Swap,
    }

    impl OkxExecMarket {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Spot => "Spot",
                Self::Swap => "Swap",
            }
        }

        fn token(self) -> &'static str {
            match self {
                Self::Spot => "okx-spot",
                Self::Swap => "okx-swap",
            }
        }

        fn default_td_mode(self) -> &'static str {
            match self {
                Self::Spot => "cash",
                Self::Swap => "cross",
            }
        }

        fn expected_instrument_suffix(self) -> &'static str {
            match self {
                Self::Spot => "SPOT",
                Self::Swap => "SWAP",
            }
        }
    }

    impl fmt::Display for OkxExecMarket {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.as_str())
        }
    }

    /// OKX 执行适配器配置。
    ///
    /// 中文说明：配置只保存 endpoint、账户引用、签名策略、tdMode 和可选
    /// posSide，不保存 API key、secret、passphrase、签名或任何凭证原文。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct OkxExecConfig {
        market: OkxExecMarket,
        venue_id: VenueId,
        account_id: AccountId,
        base_url: String,
        td_mode: String,
        pos_side: Option<String>,
        target_leverage: Option<u32>,
        quantity_step_by_inst_id: BTreeMap<String, Quantity>,
        signing_policy: SigningPolicy,
    }

    impl OkxExecConfig {
        pub fn spot(
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            Self::new(
                OkxExecMarket::Spot,
                venue_id,
                account_id,
                base_url,
                OkxExecMarket::Spot.default_td_mode(),
                signing_policy,
            )
        }

        pub fn swap(
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            Self::new(
                OkxExecMarket::Swap,
                venue_id,
                account_id,
                base_url,
                OkxExecMarket::Swap.default_td_mode(),
                signing_policy,
            )
        }

        pub fn new(
            market: OkxExecMarket,
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            td_mode: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            let td_mode = td_mode.into();
            validate_okx_td_mode(market, &td_mode)?;
            Ok(Self {
                market,
                venue_id,
                account_id,
                base_url: normalize_okx_base_url(base_url.into())?,
                td_mode,
                pos_side: None,
                target_leverage: None,
                quantity_step_by_inst_id: BTreeMap::new(),
                signing_policy,
            })
        }

        pub fn with_pos_side(mut self, pos_side: impl Into<String>) -> VenueExecResult<Self> {
            let pos_side = pos_side.into();
            validate_okx_pos_side(&pos_side)?;
            self.pos_side = Some(pos_side);
            Ok(self)
        }

        pub fn with_quantity_step(
            mut self,
            inst_id: impl Into<String>,
            step: Quantity,
        ) -> VenueExecResult<Self> {
            let inst_id = inst_id.into();
            validate_okx_exec_inst_id(self.market, &inst_id)?;
            validate_venue_quantity_step(step)?;
            self.quantity_step_by_inst_id.insert(inst_id, step);
            Ok(self)
        }

        pub fn with_target_leverage(mut self, leverage: u32) -> VenueExecResult<Self> {
            if self.market != OkxExecMarket::Swap {
                return Err(VenueExecError::InvalidRequest {
                    field: "target_leverage",
                    reason: "OKX target leverage can only be configured for swap",
                });
            }
            validate_perp_target_leverage("target_leverage", leverage)?;
            self.target_leverage = Some(leverage);
            Ok(self)
        }

        pub fn market(&self) -> OkxExecMarket {
            self.market
        }

        pub fn venue_id(&self) -> &VenueId {
            &self.venue_id
        }

        pub fn account_id(&self) -> &AccountId {
            &self.account_id
        }

        pub fn base_url(&self) -> &str {
            &self.base_url
        }

        pub fn td_mode(&self) -> &str {
            &self.td_mode
        }

        pub fn pos_side(&self) -> Option<&str> {
            self.pos_side.as_deref()
        }

        pub fn quantity_step(&self, inst_id: &str) -> Option<Quantity> {
            self.quantity_step_by_inst_id.get(inst_id).copied()
        }

        pub fn target_leverage(&self) -> Option<u32> {
            self.target_leverage
        }

        pub fn signing_policy(&self) -> &SigningPolicy {
            &self.signing_policy
        }
    }

    /// 已签名 OKX HTTP 请求。
    pub struct OkxSignedRequest<'a> {
        market: OkxExecMarket,
        base_url: &'a str,
        signed_endpoint: &'a OkxSignedEndpoint,
    }

    impl OkxSignedRequest<'_> {
        pub fn market(&self) -> OkxExecMarket {
            self.market
        }

        pub fn method(&self) -> OkxRestMethod {
            self.signed_endpoint.method()
        }

        pub fn base_url(&self) -> &str {
            self.base_url
        }

        pub fn request_path(&self) -> &str {
            self.signed_endpoint.request_path_for_transport()
        }

        pub fn body_for_transport(&self) -> &str {
            self.signed_endpoint.body_for_transport()
        }

        pub fn api_key_header_name(&self) -> &'static str {
            self.signed_endpoint.api_key_header_name()
        }

        pub fn api_key_header_value(&self) -> &str {
            self.signed_endpoint.api_key_header_value()
        }

        pub fn signature_header_name(&self) -> &'static str {
            self.signed_endpoint.signature_header_name()
        }

        pub fn signature_header_value(&self) -> &str {
            self.signed_endpoint.signature_header_value()
        }

        pub fn timestamp_header_name(&self) -> &'static str {
            self.signed_endpoint.timestamp_header_name()
        }

        pub fn timestamp_header_value(&self) -> &str {
            self.signed_endpoint.timestamp_header_value()
        }

        pub fn passphrase_header_name(&self) -> &'static str {
            self.signed_endpoint.passphrase_header_name()
        }

        pub fn passphrase_header_value(&self) -> &str {
            self.signed_endpoint.passphrase_header_value()
        }
    }

    impl fmt::Debug for OkxSignedRequest<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("OkxSignedRequest")
                .field("market", &self.market)
                .field("method", &self.method())
                .field("base_url", &self.base_url)
                .field("request_path", &"<redacted>")
                .field("api_key_header_name", &self.api_key_header_name())
                .field("api_key_header_value", &"<redacted>")
                .field("signature_header_name", &self.signature_header_name())
                .field("timestamp_header_name", &self.timestamp_header_name())
                .field("passphrase_header_name", &self.passphrase_header_name())
                .field("body", &"<redacted>")
                .field("signature", &"<redacted>")
                .finish()
        }
    }

    /// OKX transport 返回。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct OkxExecHttpResponse {
        status_code: u16,
        body: String,
    }

    impl OkxExecHttpResponse {
        pub fn new(status_code: u16, body: impl Into<String>) -> Self {
            Self {
                status_code,
                body: body.into(),
            }
        }

        pub fn status_code(&self) -> u16 {
            self.status_code
        }

        pub fn body(&self) -> &str {
            &self.body
        }

        pub fn is_success(&self) -> bool {
            (200..=299).contains(&self.status_code)
        }
    }

    /// OKX 可变执行 transport。
    ///
    /// 中文说明：transport 遇到网络断连、TLS 失败或无 HTTP 状态码时必须返回
    /// `UnknownExternalState`，不能把外部未知状态当成下单失败或成功。
    pub trait OkxExecTransport {
        fn send_signed(
            &mut self,
            request: OkxSignedRequest<'_>,
        ) -> VenueExecResult<OkxExecHttpResponse>;
    }

    /// 使用系统 `curl` 发送 OKX signed REST 请求的真实 transport。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct OkxCurlExecTransport {
        connect_timeout_secs: u64,
        max_time_secs: u64,
    }

    impl OkxCurlExecTransport {
        pub fn new(connect_timeout_secs: u64, max_time_secs: u64) -> VenueExecResult<Self> {
            if connect_timeout_secs == 0 || max_time_secs == 0 {
                return Err(VenueExecError::InvalidRequest {
                    field: "curl_timeout",
                    reason: "curl timeouts must be greater than zero",
                });
            }
            Ok(Self {
                connect_timeout_secs,
                max_time_secs,
            })
        }
    }

    impl Default for OkxCurlExecTransport {
        fn default() -> Self {
            Self {
                connect_timeout_secs: 10,
                max_time_secs: 30,
            }
        }
    }

    impl OkxExecTransport for OkxCurlExecTransport {
        fn send_signed(
            &mut self,
            request: OkxSignedRequest<'_>,
        ) -> VenueExecResult<OkxExecHttpResponse> {
            let venue_id = okx_transport_venue_id(request.market());
            let url = okx_signed_request_url(&request)?;
            let config = okx_curl_config(&request, &url)?;
            let mut child = Command::new("curl")
                .arg("--silent")
                .arg("--show-error")
                .arg("--request")
                .arg(request.method().as_str())
                .arg("--connect-timeout")
                .arg(self.connect_timeout_secs.to_string())
                .arg("--max-time")
                .arg(self.max_time_secs.to_string())
                .arg("--write-out")
                .arg(format!("{CURL_OKX_STATUS_MARKER}%{{http_code}}"))
                .arg("--config")
                .arg("-")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "failed to start curl for OKX signed REST request".to_owned(),
                })?;

            child
                .stdin
                .as_mut()
                .ok_or_else(|| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "curl stdin is unavailable for OKX signed REST request".to_owned(),
                })?
                .write_all(config.as_bytes())
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "failed to write curl config for OKX signed REST request".to_owned(),
                })?;

            let output =
                child
                    .wait_with_output()
                    .map_err(|_| VenueExecError::UnknownExternalState {
                        venue_id: venue_id.clone(),
                        detail: "curl transport did not return an OKX signed REST response"
                            .to_owned(),
                    })?;
            if !output.status.success() {
                return Err(VenueExecError::UnknownExternalState {
                    venue_id,
                    detail: "curl transport failed before a reliable HTTP response was available"
                        .to_owned(),
                });
            }

            parse_okx_curl_http_response(&output.stdout, request.market())
        }
    }

    /// OKX Spot 可变执行适配器。
    pub struct OkxSpotExecAdapter<S, T> {
        inner: OkxExecAdapterCore<S, T>,
    }

    impl<S, T> OkxSpotExecAdapter<S, T> {
        pub fn new(config: OkxExecConfig, signer: S, transport: T) -> VenueExecResult<Self> {
            ensure_okx_config_market(&config, OkxExecMarket::Spot)?;
            Ok(Self {
                inner: OkxExecAdapterCore::new(config, signer, transport),
            })
        }

        pub fn config(&self) -> &OkxExecConfig {
            self.inner.config()
        }

        pub fn transport(&self) -> &T {
            self.inner.transport()
        }

        pub fn transport_mut(&mut self) -> &mut T {
            self.inner.transport_mut()
        }
    }

    impl<S, T> fmt::Debug for OkxSpotExecAdapter<S, T>
    where
        S: fmt::Debug,
        T: fmt::Debug,
    {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("OkxSpotExecAdapter")
                .field("inner", &self.inner)
                .finish()
        }
    }

    /// OKX Swap 可变执行适配器。
    pub struct OkxSwapExecAdapter<S, T> {
        inner: OkxExecAdapterCore<S, T>,
    }

    impl<S, T> OkxSwapExecAdapter<S, T> {
        pub fn new(config: OkxExecConfig, signer: S, transport: T) -> VenueExecResult<Self> {
            ensure_okx_config_market(&config, OkxExecMarket::Swap)?;
            Ok(Self {
                inner: OkxExecAdapterCore::new(config, signer, transport),
            })
        }

        pub fn config(&self) -> &OkxExecConfig {
            self.inner.config()
        }

        pub fn transport(&self) -> &T {
            self.inner.transport()
        }

        pub fn transport_mut(&mut self) -> &mut T {
            self.inner.transport_mut()
        }
    }

    impl<S, T> fmt::Debug for OkxSwapExecAdapter<S, T>
    where
        S: fmt::Debug,
        T: fmt::Debug,
    {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("OkxSwapExecAdapter")
                .field("inner", &self.inner)
                .finish()
        }
    }

    macro_rules! impl_okx_adapter_traits {
        ($adapter:ty) => {
            impl<S, T> SubmitOrder for $adapter
            where
                S: OkxRealSigningProvider,
                T: OkxExecTransport,
            {
                fn submit_order(
                    &mut self,
                    request: SubmitOrderRequest,
                ) -> VenueExecResult<MutableActionReceipt> {
                    self.inner.submit_order(request)
                }
            }

            impl<S, T> CancelOrder for $adapter
            where
                S: OkxRealSigningProvider,
                T: OkxExecTransport,
            {
                fn cancel_order(
                    &mut self,
                    request: CancelOrderRequest,
                ) -> VenueExecResult<MutableActionReceipt> {
                    self.inner.cancel_order(request)
                }
            }

            impl<S, T> QueryActionStatus for $adapter
            where
                S: OkxRealSigningProvider,
                T: OkxExecTransport,
            {
                fn query_action_status(
                    &self,
                    request: QueryActionStatusRequest,
                ) -> VenueExecResult<MutableActionStatusReport> {
                    self.inner.query_action_status(request)
                }
            }

            impl<S, T> ConfirmOrderStatus for $adapter
            where
                S: OkxRealSigningProvider,
                T: OkxExecTransport,
            {
                fn confirm_order_status(
                    &mut self,
                    request: ConfirmOrderStatusRequest,
                ) -> VenueExecResult<PrivateOrderUpdate> {
                    self.inner.confirm_order_status(request)
                }
            }

            impl<S, T> RequestTransfer for $adapter
            where
                S: OkxRealSigningProvider,
                T: OkxExecTransport,
            {
                fn request_transfer(
                    &mut self,
                    request: TransferRequest,
                ) -> VenueExecResult<MutableActionReceipt> {
                    self.inner.request_transfer(request)
                }
            }
        };
    }

    impl_okx_adapter_traits!(OkxSpotExecAdapter<S, T>);
    impl_okx_adapter_traits!(OkxSwapExecAdapter<S, T>);

    #[derive(Debug)]
    struct OkxExecAdapterCore<S, T> {
        config: OkxExecConfig,
        signer: S,
        transport: T,
        records_by_key: BTreeMap<IdempotencyKey, LiveActionRecord>,
        key_by_action_id: BTreeMap<MutableActionId, IdempotencyKey>,
        orders_by_client_id: BTreeMap<OrderId, OkxKnownOrder>,
        orders_by_external_id: BTreeMap<ExternalOrderId, OkxKnownOrder>,
        next_sequence: u64,
    }

    impl<S, T> OkxExecAdapterCore<S, T> {
        fn new(config: OkxExecConfig, signer: S, transport: T) -> Self {
            Self {
                config,
                signer,
                transport,
                records_by_key: BTreeMap::new(),
                key_by_action_id: BTreeMap::new(),
                orders_by_client_id: BTreeMap::new(),
                orders_by_external_id: BTreeMap::new(),
                next_sequence: 0,
            }
        }

        fn config(&self) -> &OkxExecConfig {
            &self.config
        }

        fn transport(&self) -> &T {
            &self.transport
        }

        fn transport_mut(&mut self) -> &mut T {
            &mut self.transport
        }

        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            Ok(match request {
                QueryActionStatusRequest::ByActionId(action_id) => {
                    if let Some(key) = self.key_by_action_id.get(&action_id) {
                        self.report_for_key(key)
                    } else {
                        unknown_status_report(Some(action_id), None)
                    }
                }
                QueryActionStatusRequest::ByIdempotencyKey(key) => self.report_for_key(&key),
            })
        }

        fn report_for_key(&self, key: &IdempotencyKey) -> MutableActionStatusReport {
            self.records_by_key.get(key).map_or_else(
                || unknown_status_report(None, Some(key.clone())),
                |record| super::status_report_from_receipt(&record.receipt),
            )
        }

        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.from_account_id)?;
            Err(VenueExecError::InvalidRequest {
                field: "transfer",
                reason: "OKX live transfer is not implemented by this execution adapter",
            })
        }

        fn ensure_request_scope(
            &self,
            venue_id: &VenueId,
            account_id: &AccountId,
        ) -> VenueExecResult<()> {
            if venue_id != &self.config.venue_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "venue_id",
                    reason: "request venue does not match OKX execution adapter config",
                });
            }
            if account_id != &self.config.account_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "account_id",
                    reason: "request account does not match OKX execution adapter config",
                });
            }
            Ok(())
        }
    }

    impl<S, T> OkxExecAdapterCore<S, T>
    where
        S: OkxRealSigningProvider,
        T: OkxExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            request.validate()?;
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;

            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }

            let inst_id = okx_inst_id_from_instrument(self.config.market, &request.instrument_id)?;
            self.ensure_target_leverage(&inst_id, request.reduce_only)?;
            let body = okx_order_create_body(&self.config, &inst_id, &request)?;
            let action_id = self.next_action_id(MutableActionKind::SubmitOrder)?;
            let signed = self.sign(
                SigningPurpose::SubmitOrder,
                &action_id,
                OkxRestMethod::Post,
                OKX_ORDER_ENDPOINT.to_owned(),
                body,
            )?;
            let response = self.dispatch_signed(&signed)?;
            self.ensure_business_success(OKX_ORDER_ENDPOINT, &response)?;

            let known_order = okx_known_order_from_response(
                self.config.market,
                &inst_id,
                request.client_order_id.clone(),
                response.body(),
                &action_id,
            )?;
            let external_ref = known_order
                .external_order_id
                .clone()
                .map(ExternalActionRef::Order);
            let receipt = MutableActionReceipt {
                action_id,
                kind: MutableActionKind::SubmitOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref,
                duplicate: false,
                simulated: false,
            };

            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            self.record_known_order(known_order);
            Ok(receipt)
        }

        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }

            let known_order = self.lookup_known_order(&request.order_ref).ok_or(
                VenueExecError::InvalidRequest {
                    field: "order_ref",
                    reason: "OKX cancel requires an order previously submitted through this adapter so its instId is known",
                },
            )?;
            let body = okx_cancel_order_body(&request, known_order)?;
            let action_id = self.next_action_id(MutableActionKind::CancelOrder)?;
            let signed = self.sign(
                SigningPurpose::CancelOrder,
                &action_id,
                OkxRestMethod::Post,
                OKX_CANCEL_ORDER_ENDPOINT.to_owned(),
                body,
            )?;
            let response = self.dispatch_signed(&signed)?;
            self.ensure_business_success(OKX_CANCEL_ORDER_ENDPOINT, &response)?;

            let receipt = MutableActionReceipt {
                action_id: action_id.clone(),
                kind: MutableActionKind::CancelOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref: Some(ExternalActionRef::Cancel(action_id)),
                duplicate: false,
                simulated: false,
            };
            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            Ok(receipt)
        }

        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let inst_id = okx_inst_id_from_instrument(self.config.market, &request.instrument_id)?;
            let request_path = okx_query_order_request_path(&inst_id, &request.order_ref)?;
            let signing_request_id =
                order_query_signing_request_id("okx-exec", &request.source_event_id)?;
            let signed = self.sign_with_request_id(
                SigningPurpose::QueryOrder,
                signing_request_id,
                OkxRestMethod::Get,
                request_path,
                String::new(),
            )?;
            let response = self.dispatch_signed(&signed)?;
            self.ensure_http_success(OKX_ORDER_ENDPOINT, &response)?;
            parse_okx_order_query_confirmation(
                private_market_from_okx_exec_market(self.config.market),
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                request.source_event_id,
                response.body(),
            )
        }

        fn ensure_target_leverage(
            &mut self,
            inst_id: &str,
            reduce_only: bool,
        ) -> VenueExecResult<()> {
            if reduce_only || self.config.market != OkxExecMarket::Swap {
                return Ok(());
            }
            let Some(leverage) = self.config.target_leverage else {
                return Ok(());
            };
            let body = okx_set_leverage_body(&self.config, inst_id, leverage)?;
            let signing_request_id = leverage_signing_request_id("okx-exec", inst_id, leverage)?;
            let signed = self.sign_with_request_id(
                SigningPurpose::SubmitOrder,
                signing_request_id,
                OkxRestMethod::Post,
                OKX_SET_LEVERAGE_ENDPOINT.to_owned(),
                body,
            )?;
            let response = self.dispatch_signed(&signed)?;
            self.ensure_business_success(OKX_SET_LEVERAGE_ENDPOINT, &response)
        }

        fn duplicate_receipt(
            &self,
            idempotency_key: &IdempotencyKey,
            fingerprint: &RequestFingerprint,
        ) -> VenueExecResult<Option<MutableActionReceipt>> {
            let Some(existing) = self.records_by_key.get(idempotency_key) else {
                return Ok(None);
            };
            if existing.fingerprint != *fingerprint {
                return Err(VenueExecError::IdempotencyConflict {
                    idempotency_key: idempotency_key.clone(),
                    existing_fingerprint: existing.fingerprint.0.clone(),
                    incoming_fingerprint: fingerprint.0.clone(),
                });
            }

            let mut receipt = existing.receipt.clone();
            receipt.duplicate = true;
            Ok(Some(receipt))
        }

        fn next_action_id(&mut self, kind: MutableActionKind) -> VenueExecResult<MutableActionId> {
            self.next_sequence = self
                .next_sequence
                .checked_add(1)
                .expect("OKX mutable action sequence overflowed");
            MutableActionId::new(format!(
                "{}:{}:{}",
                self.config.market.token(),
                kind.as_str(),
                self.next_sequence
            ))
        }

        fn sign(
            &self,
            purpose: SigningPurpose,
            action_id: &MutableActionId,
            method: OkxRestMethod,
            request_path: String,
            body: String,
        ) -> VenueExecResult<OkxSignedEndpoint> {
            self.sign_with_request_id(
                purpose,
                SigningRequestId::new(format!("signing-request/okx-exec/{}", action_id.as_str()))
                    .map_err(signing_error)?,
                method,
                request_path,
                body,
            )
        }

        fn sign_with_request_id(
            &self,
            purpose: SigningPurpose,
            signing_request_id: SigningRequestId,
            method: OkxRestMethod,
            request_path: String,
            body: String,
        ) -> VenueExecResult<OkxSignedEndpoint> {
            let input = OkxHmacSigningInput::new(
                signing_request_id,
                self.config.signing_policy.policy_ref().clone(),
                purpose,
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                method,
                request_path,
                body,
            )
            .map_err(signing_error)?;
            self.signer
                .sign_okx_hmac(input, &self.config.signing_policy)
                .map_err(signing_error)
        }

        fn dispatch_signed(
            &mut self,
            signed_endpoint: &OkxSignedEndpoint,
        ) -> VenueExecResult<OkxExecHttpResponse> {
            let request = OkxSignedRequest {
                market: self.config.market,
                base_url: &self.config.base_url,
                signed_endpoint,
            };
            self.transport.send_signed(request)
        }

        fn ensure_http_success(
            &self,
            endpoint: &'static str,
            response: &OkxExecHttpResponse,
        ) -> VenueExecResult<()> {
            if response.is_success() {
                return Ok(());
            }
            Err(VenueExecError::ExternalRejected {
                venue_id: self.config.venue_id.clone(),
                endpoint: endpoint.to_owned(),
                status_code: response.status_code(),
                reason: response_body_snippet(response.body()),
            })
        }

        fn ensure_business_success(
            &self,
            endpoint: &'static str,
            response: &OkxExecHttpResponse,
        ) -> VenueExecResult<()> {
            self.ensure_http_success(endpoint, response)?;
            match json_field_value(response.body(), "code").as_deref() {
                Some("0") => {}
                Some(code) => {
                    return Err(VenueExecError::ExternalRejected {
                        venue_id: self.config.venue_id.clone(),
                        endpoint: endpoint.to_owned(),
                        status_code: response.status_code(),
                        reason: okx_business_rejection_reason(response.body(), code),
                    });
                }
                None => {
                    return Err(VenueExecError::UnknownExternalState {
                        venue_id: self.config.venue_id.clone(),
                        detail: format!("OKX response from {endpoint} lacks code"),
                    });
                }
            }
            match json_field_value(response.body(), "sCode").as_deref() {
                Some("0") | None => Ok(()),
                Some(s_code) => Err(VenueExecError::ExternalRejected {
                    venue_id: self.config.venue_id.clone(),
                    endpoint: endpoint.to_owned(),
                    status_code: response.status_code(),
                    reason: format!(
                        "OKX sCode={s_code}: {}",
                        json_field_value(response.body(), "sMsg")
                            .unwrap_or_else(|| "missing sMsg".to_owned())
                    ),
                }),
            }
        }

        fn record_action(
            &mut self,
            idempotency_key: IdempotencyKey,
            fingerprint: RequestFingerprint,
            receipt: MutableActionReceipt,
        ) {
            self.key_by_action_id
                .insert(receipt.action_id.clone(), idempotency_key.clone());
            self.records_by_key.insert(
                idempotency_key,
                LiveActionRecord {
                    fingerprint,
                    receipt,
                },
            );
        }

        fn record_known_order(&mut self, known_order: OkxKnownOrder) {
            if let Some(client_order_id) = known_order.client_order_id.clone() {
                self.orders_by_client_id
                    .insert(client_order_id, known_order.clone());
            }
            if let Some(external_order_id) = known_order.external_order_id.clone() {
                self.orders_by_external_id
                    .insert(external_order_id, known_order);
            }
        }

        fn lookup_known_order(&self, order_ref: &OrderReference) -> Option<&OkxKnownOrder> {
            match order_ref {
                OrderReference::ClientOrderId(order_id) => self.orders_by_client_id.get(order_id),
                OrderReference::VenueOrderId(order_id) => self.orders_by_external_id.get(order_id),
            }
        }
    }

    fn okx_set_leverage_body(
        config: &OkxExecConfig,
        inst_id: &str,
        leverage: u32,
    ) -> VenueExecResult<String> {
        validate_perp_target_leverage("target_leverage", leverage)?;
        let mut body = String::from("{");
        let mut first = true;
        push_json_string_field(&mut body, &mut first, "instId", inst_id)?;
        push_json_string_field(&mut body, &mut first, "lever", &leverage.to_string())?;
        push_json_string_field(&mut body, &mut first, "mgnMode", config.td_mode())?;
        if let Some(pos_side) = config.pos_side() {
            push_json_string_field(&mut body, &mut first, "posSide", pos_side)?;
        }
        body.push('}');
        Ok(body)
    }

    fn okx_business_rejection_reason(body: &str, code: &str) -> String {
        let mut reason = format!(
            "OKX code={code}: {}",
            json_field_value(body, "msg").unwrap_or_else(|| "missing msg".to_owned())
        );
        if let Some(s_code) = json_field_value(body, "sCode") {
            let s_msg = json_field_value(body, "sMsg").unwrap_or_else(|| "missing sMsg".to_owned());
            reason.push_str(&format!("; OKX sCode={s_code}: {s_msg}"));
        }
        reason
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct OkxKnownOrder {
        inst_id: String,
        order_id_param: Option<String>,
        client_order_id: Option<OrderId>,
        external_order_id: Option<ExternalOrderId>,
    }

    fn ensure_okx_config_market(
        config: &OkxExecConfig,
        expected: OkxExecMarket,
    ) -> VenueExecResult<()> {
        if config.market == expected {
            Ok(())
        } else {
            Err(VenueExecError::InvalidRequest {
                field: "market",
                reason: "OKX execution adapter received config for a different market",
            })
        }
    }

    fn validate_okx_td_mode(market: OkxExecMarket, td_mode: &str) -> VenueExecResult<()> {
        match (market, td_mode) {
            (OkxExecMarket::Spot, "cash")
            | (OkxExecMarket::Spot, "cross")
            | (OkxExecMarket::Spot, "isolated")
            | (OkxExecMarket::Swap, "cross")
            | (OkxExecMarket::Swap, "isolated") => Ok(()),
            _ => Err(VenueExecError::InvalidRequest {
                field: "td_mode",
                reason:
                    "OKX tdMode must be cash/cross/isolated for spot and cross/isolated for swap",
            }),
        }
    }

    fn validate_okx_pos_side(value: &str) -> VenueExecResult<()> {
        match value {
            "long" | "short" | "net" => Ok(()),
            _ => Err(VenueExecError::InvalidRequest {
                field: "pos_side",
                reason: "OKX posSide must be long, short or net",
            }),
        }
    }

    fn normalize_okx_base_url(value: String) -> VenueExecResult<String> {
        let trimmed = value.trim().trim_end_matches('/').to_owned();
        if trimmed.is_empty() {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "OKX base URL cannot be empty",
            });
        }
        if trimmed
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "OKX base URL contains a control byte",
            });
        }
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://127.0.0.1")) {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "OKX base URL must use https or an explicit localhost test URL",
            });
        }
        Ok(trimmed)
    }

    fn okx_order_create_body(
        config: &OkxExecConfig,
        inst_id: &str,
        request: &SubmitOrderRequest,
    ) -> VenueExecResult<String> {
        let quantity = okx_order_quantity(config, inst_id, request.quantity)?;
        let mut body = String::from("{");
        let mut first = true;
        push_json_string_field(&mut body, &mut first, "instId", inst_id)?;
        push_json_string_field(&mut body, &mut first, "tdMode", config.td_mode())?;
        if let Some(pos_side) = config.pos_side() {
            push_json_string_field(&mut body, &mut first, "posSide", pos_side)?;
        }
        push_json_string_field(&mut body, &mut first, "side", okx_side(request.side))?;
        match request.order_type {
            MutableOrderType::Market => {
                push_json_string_field(&mut body, &mut first, "ordType", "market")?;
                push_json_string_field(&mut body, &mut first, "sz", &quantity)?;
            }
            MutableOrderType::Limit => {
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "ordType",
                    okx_limit_ord_type(limit_time_in_force(request)),
                )?;
                push_json_string_field(&mut body, &mut first, "sz", &quantity)?;
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "px",
                    &request
                        .limit_price
                        .expect("validated limit order price")
                        .to_string(),
                )?;
            }
            MutableOrderType::PostOnly => {
                push_json_string_field(&mut body, &mut first, "ordType", "post_only")?;
                push_json_string_field(&mut body, &mut first, "sz", &quantity)?;
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "px",
                    &request
                        .limit_price
                        .expect("validated post-only order price")
                        .to_string(),
                )?;
            }
        }
        if request.reduce_only {
            match config.market {
                OkxExecMarket::Spot => {
                    return Err(VenueExecError::InvalidRequest {
                        field: "reduce_only",
                        reason: "OKX spot orders do not support reduce_only",
                    });
                }
                OkxExecMarket::Swap => {
                    push_json_string_field(&mut body, &mut first, "reduceOnly", "true")?;
                }
            }
        }
        if let Some(client_order_id) = &request.client_order_id {
            validate_okx_client_order_id(client_order_id.as_str())?;
            push_json_string_field(&mut body, &mut first, "clOrdId", client_order_id.as_str())?;
        }
        body.push('}');
        Ok(body)
    }

    fn okx_order_quantity(
        config: &OkxExecConfig,
        inst_id: &str,
        quantity: Quantity,
    ) -> VenueExecResult<String> {
        match config.quantity_step(inst_id) {
            Some(step) => format_quantity_at_venue_step(quantity, step),
            None => Ok(quantity.to_string()),
        }
    }

    fn okx_cancel_order_body(
        request: &CancelOrderRequest,
        known_order: &OkxKnownOrder,
    ) -> VenueExecResult<String> {
        let mut body = String::from("{");
        let mut first = true;
        push_json_string_field(
            &mut body,
            &mut first,
            "instId",
            known_order.inst_id.as_str(),
        )?;
        match &request.order_ref {
            OrderReference::VenueOrderId(_) => {
                if let Some(order_id) = &known_order.order_id_param {
                    push_json_string_field(&mut body, &mut first, "ordId", order_id)?;
                } else if let Some(client_order_id) = &known_order.client_order_id {
                    push_json_string_field(
                        &mut body,
                        &mut first,
                        "clOrdId",
                        client_order_id.as_str(),
                    )?;
                } else {
                    return Err(VenueExecError::InvalidRequest {
                        field: "order_ref",
                        reason: "known OKX venue order lacks ordId and clOrdId",
                    });
                }
            }
            OrderReference::ClientOrderId(client_order_id) => {
                validate_okx_client_order_id(client_order_id.as_str())?;
                push_json_string_field(&mut body, &mut first, "clOrdId", client_order_id.as_str())?;
            }
        }
        body.push('}');
        Ok(body)
    }

    fn okx_query_order_request_path(
        inst_id: &str,
        order_ref: &OrderReference,
    ) -> VenueExecResult<String> {
        let mut params = vec![("instId".to_owned(), inst_id.to_owned())];
        match order_ref {
            OrderReference::VenueOrderId(order_id) => {
                let raw_order_id = okx_order_id_param_from_external(order_id)?;
                params.push(("ordId".to_owned(), raw_order_id.to_owned()));
            }
            OrderReference::ClientOrderId(client_order_id) => {
                validate_okx_client_order_id(client_order_id.as_str())?;
                params.push(("clOrdId".to_owned(), client_order_id.as_str().to_owned()));
            }
        }
        Ok(format!(
            "{OKX_ORDER_ENDPOINT}?{}",
            query_string_from_pairs(&params)
        ))
    }

    fn okx_order_id_param_from_external(order_id: &ExternalOrderId) -> VenueExecResult<&str> {
        let value = order_id.as_str();
        let raw_order_id = value
            .strip_prefix("okx-spot:order:")
            .or_else(|| value.strip_prefix("okx-swap:order:"))
            .ok_or(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "OKX venue order ref must come from an OKX adapter",
            })?;
        validate_okx_order_id(raw_order_id)?;
        Ok(raw_order_id)
    }

    fn private_market_from_okx_exec_market(market: OkxExecMarket) -> PrivateOrderMarket {
        match market {
            OkxExecMarket::Spot => PrivateOrderMarket::OkxSpot,
            OkxExecMarket::Swap => PrivateOrderMarket::OkxSwap,
        }
    }

    fn okx_inst_id_from_instrument(
        market: OkxExecMarket,
        instrument_id: &InstrumentId,
    ) -> VenueExecResult<String> {
        let value = instrument_id.as_str();
        let mut parts = value.split(':');
        let prefix = parts.next();
        let venue = parts.next();
        let inst_id = parts.next();
        let suffix = parts.next();
        if parts.next().is_some()
            || prefix != Some("inst")
            || venue != Some("OKX")
            || suffix != Some(market.expected_instrument_suffix())
        {
            return Err(VenueExecError::InvalidRequest {
                field: "instrument_id",
                reason: "OKX execution requires instrument IDs shaped as inst:OKX:<INST-ID>:SPOT or inst:OKX:<INST-ID>:SWAP",
            });
        }
        let inst_id = inst_id.expect("instId checked above");
        validate_okx_exec_inst_id(market, inst_id)?;
        Ok(inst_id.to_owned())
    }

    fn validate_okx_exec_inst_id(market: OkxExecMarket, value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 64 {
            return Err(VenueExecError::InvalidRequest {
                field: "inst_id",
                reason: "OKX instId must be 1 to 64 bytes",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-'))
        {
            return Err(VenueExecError::InvalidRequest {
                field: "inst_id",
                reason: "OKX instId must use uppercase ASCII letters, digits or dash",
            });
        }
        match market {
            OkxExecMarket::Spot if !value.ends_with("-SWAP") => Ok(()),
            OkxExecMarket::Swap if value.ends_with("-SWAP") => Ok(()),
            _ => Err(VenueExecError::InvalidRequest {
                field: "inst_id",
                reason: "OKX instId does not match configured execution market",
            }),
        }
    }

    fn validate_okx_client_order_id(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 32 {
            return Err(VenueExecError::InvalidRequest {
                field: "client_order_id",
                reason: "OKX clOrdId must be 1 to 32 bytes",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
        {
            return Err(VenueExecError::InvalidRequest {
                field: "client_order_id",
                reason: "OKX clOrdId must use ASCII letters, digits, dash or underscore",
            });
        }
        Ok(())
    }

    fn validate_okx_order_id(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 96 {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "OKX ordId must be 1 to 96 bytes",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
        {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "OKX ordId contains an unsupported byte",
            });
        }
        Ok(())
    }

    fn okx_known_order_from_response(
        market: OkxExecMarket,
        inst_id: &str,
        client_order_id: Option<OrderId>,
        body: &str,
        action_id: &MutableActionId,
    ) -> VenueExecResult<OkxKnownOrder> {
        let order_id_param = json_field_value(body, "ordId").filter(|value| !value.is_empty());
        let response_client_order_id = json_field_value(body, "clOrdId")
            .filter(|value| !value.is_empty())
            .and_then(|value| OrderId::new(value).ok());
        let client_order_id = client_order_id.or(response_client_order_id);
        let external_order_id = if let Some(order_id) = &order_id_param {
            Some(ExternalOrderId::new(format!(
                "{}:order:{order_id}",
                market.token()
            ))?)
        } else if let Some(client_order_id) = &client_order_id {
            Some(ExternalOrderId::new(format!(
                "{}:client:{}",
                market.token(),
                client_order_id.as_str()
            ))?)
        } else {
            Some(ExternalOrderId::new(format!(
                "{}:action:{}",
                market.token(),
                action_id.as_str()
            ))?)
        };
        Ok(OkxKnownOrder {
            inst_id: inst_id.to_owned(),
            order_id_param,
            client_order_id,
            external_order_id,
        })
    }

    fn okx_side(side: OrderSide) -> &'static str {
        match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }
    }

    fn okx_limit_ord_type(time_in_force: MutableTimeInForce) -> &'static str {
        match time_in_force {
            MutableTimeInForce::Gtc => "limit",
            MutableTimeInForce::Ioc => "ioc",
            MutableTimeInForce::Fok => "fok",
        }
    }

    fn okx_signed_request_url(request: &OkxSignedRequest<'_>) -> VenueExecResult<String> {
        let request_path = request.request_path();
        if request_path.is_empty() || !request_path.starts_with('/') {
            return Err(VenueExecError::InvalidRequest {
                field: "request_path",
                reason: "OKX signed REST request path must be an absolute path",
            });
        }
        Ok(format!("{}{}", request.base_url(), request_path))
    }

    fn okx_curl_config(request: &OkxSignedRequest<'_>, url: &str) -> VenueExecResult<String> {
        let mut config = format!("url = \"{}\"\n", curl_config_quote(url)?);
        push_curl_header(
            &mut config,
            request.api_key_header_name(),
            request.api_key_header_value(),
        )?;
        push_curl_header(
            &mut config,
            request.signature_header_name(),
            request.signature_header_value(),
        )?;
        push_curl_header(
            &mut config,
            request.timestamp_header_name(),
            request.timestamp_header_value(),
        )?;
        push_curl_header(
            &mut config,
            request.passphrase_header_name(),
            request.passphrase_header_value(),
        )?;
        if !request.body_for_transport().is_empty() {
            push_curl_header(&mut config, "Content-Type", "application/json")?;
            config.push_str("data = \"");
            config.push_str(&curl_config_quote(request.body_for_transport())?);
            config.push_str("\"\n");
        }
        Ok(config)
    }

    fn parse_okx_curl_http_response(
        stdout: &[u8],
        market: OkxExecMarket,
    ) -> VenueExecResult<OkxExecHttpResponse> {
        let output = String::from_utf8_lossy(stdout);
        let Some((body, status)) = output.rsplit_once(CURL_OKX_STATUS_MARKER) else {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: okx_transport_venue_id(market),
                detail: "curl transport response lacked an HTTP status marker".to_owned(),
            });
        };
        let status_code =
            status
                .trim()
                .parse::<u16>()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: okx_transport_venue_id(market),
                    detail: "curl transport returned a malformed HTTP status".to_owned(),
                })?;
        if status_code == 0 {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: okx_transport_venue_id(market),
                detail: "curl transport did not receive an HTTP response from OKX".to_owned(),
            });
        }
        Ok(OkxExecHttpResponse::new(status_code, body.to_owned()))
    }

    fn okx_transport_venue_id(market: OkxExecMarket) -> VenueId {
        let value = match market {
            OkxExecMarket::Spot => "venue:OKX-SPOT",
            OkxExecMarket::Swap => "venue:OKX-SWAP",
        };
        VenueId::new(value).expect("static OKX transport venue ID")
    }

    /// Bitget Spot 下单 endpoint。
    pub const BITGET_SPOT_PLACE_ORDER_ENDPOINT: &str = "/api/v2/spot/trade/place-order";
    /// Bitget Spot 撤单 endpoint。
    pub const BITGET_SPOT_CANCEL_ORDER_ENDPOINT: &str = "/api/v2/spot/trade/cancel-order";
    /// Bitget Spot 查单 endpoint。
    pub const BITGET_SPOT_ORDER_INFO_ENDPOINT: &str = "/api/v2/spot/trade/orderInfo";
    /// Bitget USDT-FUTURES 下单 endpoint。
    pub const BITGET_MIX_PLACE_ORDER_ENDPOINT: &str = "/api/v2/mix/order/place-order";
    /// Bitget USDT-FUTURES 设置杠杆 endpoint。
    pub const BITGET_MIX_SET_LEVERAGE_ENDPOINT: &str = "/api/v2/mix/account/set-leverage";
    /// Bitget USDT-FUTURES 撤单 endpoint。
    pub const BITGET_MIX_CANCEL_ORDER_ENDPOINT: &str = "/api/v2/mix/order/cancel-order";
    /// Bitget USDT-FUTURES 查单 endpoint。
    pub const BITGET_MIX_ORDER_DETAIL_ENDPOINT: &str = "/api/v2/mix/order/detail";
    const CURL_BITGET_STATUS_MARKER: &str = "\n__ARB_BITGET_HTTP_STATUS__:";

    /// Bitget 可变执行市场。
    ///
    /// 中文说明：该枚举用于强制把 Bitget 现货和 USDT-FUTURES 执行路径分开，
    /// 避免把现货 instrument 错发到合约 endpoint，或反向误发。
    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub enum BitgetExecMarket {
        Spot,
        UsdtFutures,
    }

    impl BitgetExecMarket {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Spot => "Spot",
                Self::UsdtFutures => "UsdtFutures",
            }
        }

        fn token(self) -> &'static str {
            match self {
                Self::Spot => "bitget-spot",
                Self::UsdtFutures => "bitget-usdt-futures",
            }
        }

        fn product_type(self) -> Option<&'static str> {
            match self {
                Self::Spot => None,
                Self::UsdtFutures => Some("USDT-FUTURES"),
            }
        }

        fn expected_instrument_suffix(self) -> &'static str {
            match self {
                Self::Spot => "SPOT",
                Self::UsdtFutures => "USDT-FUTURES",
            }
        }

        fn place_order_endpoint(self) -> &'static str {
            match self {
                Self::Spot => BITGET_SPOT_PLACE_ORDER_ENDPOINT,
                Self::UsdtFutures => BITGET_MIX_PLACE_ORDER_ENDPOINT,
            }
        }

        fn cancel_order_endpoint(self) -> &'static str {
            match self {
                Self::Spot => BITGET_SPOT_CANCEL_ORDER_ENDPOINT,
                Self::UsdtFutures => BITGET_MIX_CANCEL_ORDER_ENDPOINT,
            }
        }

        fn order_query_endpoint(self) -> &'static str {
            match self {
                Self::Spot => BITGET_SPOT_ORDER_INFO_ENDPOINT,
                Self::UsdtFutures => BITGET_MIX_ORDER_DETAIL_ENDPOINT,
            }
        }
    }

    impl fmt::Display for BitgetExecMarket {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.as_str())
        }
    }

    /// Bitget 执行适配器配置。
    ///
    /// 中文说明：配置只保存 endpoint、账户引用、签名策略、合约保证金参数和
    /// 可选 tradeSide，不保存 API key、secret、passphrase、签名或任何凭证原文。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct BitgetExecConfig {
        market: BitgetExecMarket,
        venue_id: VenueId,
        account_id: AccountId,
        base_url: String,
        margin_mode: String,
        margin_coin: String,
        trade_side: Option<String>,
        target_leverage: Option<u32>,
        quantity_step_by_symbol: BTreeMap<String, Quantity>,
        signing_policy: SigningPolicy,
    }

    impl BitgetExecConfig {
        pub fn spot(
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            Self::new(
                BitgetExecMarket::Spot,
                venue_id,
                account_id,
                base_url,
                "crossed",
                "USDT",
                signing_policy,
            )
        }

        pub fn usdt_futures(
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            Self::new(
                BitgetExecMarket::UsdtFutures,
                venue_id,
                account_id,
                base_url,
                "crossed",
                "USDT",
                signing_policy,
            )
        }

        pub fn new(
            market: BitgetExecMarket,
            venue_id: VenueId,
            account_id: AccountId,
            base_url: impl Into<String>,
            margin_mode: impl Into<String>,
            margin_coin: impl Into<String>,
            signing_policy: SigningPolicy,
        ) -> VenueExecResult<Self> {
            let margin_mode = margin_mode.into();
            let margin_coin = margin_coin.into();
            validate_bitget_margin_mode(market, &margin_mode)?;
            validate_bitget_margin_coin(market, &margin_coin)?;
            Ok(Self {
                market,
                venue_id,
                account_id,
                base_url: normalize_bitget_base_url(base_url.into())?,
                margin_mode,
                margin_coin,
                trade_side: None,
                target_leverage: None,
                quantity_step_by_symbol: BTreeMap::new(),
                signing_policy,
            })
        }

        pub fn with_trade_side(mut self, trade_side: impl Into<String>) -> VenueExecResult<Self> {
            let trade_side = trade_side.into();
            validate_bitget_trade_side(&trade_side)?;
            self.trade_side = Some(trade_side);
            Ok(self)
        }

        pub fn with_quantity_step(
            mut self,
            symbol: impl Into<String>,
            step: Quantity,
        ) -> VenueExecResult<Self> {
            let symbol = symbol.into();
            validate_bitget_symbol(&symbol)?;
            validate_venue_quantity_step(step)?;
            self.quantity_step_by_symbol.insert(symbol, step);
            Ok(self)
        }

        pub fn with_target_leverage(mut self, leverage: u32) -> VenueExecResult<Self> {
            if self.market != BitgetExecMarket::UsdtFutures {
                return Err(VenueExecError::InvalidRequest {
                    field: "target_leverage",
                    reason: "Bitget target leverage can only be configured for USDT-FUTURES",
                });
            }
            validate_perp_target_leverage("target_leverage", leverage)?;
            self.target_leverage = Some(leverage);
            Ok(self)
        }

        pub fn market(&self) -> BitgetExecMarket {
            self.market
        }

        pub fn venue_id(&self) -> &VenueId {
            &self.venue_id
        }

        pub fn account_id(&self) -> &AccountId {
            &self.account_id
        }

        pub fn base_url(&self) -> &str {
            &self.base_url
        }

        pub fn margin_mode(&self) -> &str {
            &self.margin_mode
        }

        pub fn margin_coin(&self) -> &str {
            &self.margin_coin
        }

        pub fn trade_side(&self) -> Option<&str> {
            self.trade_side.as_deref()
        }

        pub fn quantity_step(&self, symbol: &str) -> Option<Quantity> {
            self.quantity_step_by_symbol.get(symbol).copied()
        }

        pub fn target_leverage(&self) -> Option<u32> {
            self.target_leverage
        }

        pub fn signing_policy(&self) -> &SigningPolicy {
            &self.signing_policy
        }
    }

    /// 已签名 Bitget HTTP 请求。
    pub struct BitgetSignedRequest<'a> {
        market: BitgetExecMarket,
        base_url: &'a str,
        signed_endpoint: &'a BitgetSignedEndpoint,
    }

    impl BitgetSignedRequest<'_> {
        pub fn market(&self) -> BitgetExecMarket {
            self.market
        }

        pub fn method(&self) -> BitgetRestMethod {
            self.signed_endpoint.method()
        }

        pub fn base_url(&self) -> &str {
            self.base_url
        }

        pub fn request_path(&self) -> &str {
            self.signed_endpoint.request_path_for_transport()
        }

        pub fn body_for_transport(&self) -> &str {
            self.signed_endpoint.body_for_transport()
        }

        pub fn api_key_header_name(&self) -> &'static str {
            self.signed_endpoint.api_key_header_name()
        }

        pub fn api_key_header_value(&self) -> &str {
            self.signed_endpoint.api_key_header_value()
        }

        pub fn signature_header_name(&self) -> &'static str {
            self.signed_endpoint.signature_header_name()
        }

        pub fn signature_header_value(&self) -> &str {
            self.signed_endpoint.signature_header_value()
        }

        pub fn timestamp_header_name(&self) -> &'static str {
            self.signed_endpoint.timestamp_header_name()
        }

        pub fn timestamp_header_value(&self) -> String {
            self.signed_endpoint.timestamp_header_value()
        }

        pub fn passphrase_header_name(&self) -> &'static str {
            self.signed_endpoint.passphrase_header_name()
        }

        pub fn passphrase_header_value(&self) -> &str {
            self.signed_endpoint.passphrase_header_value()
        }
    }

    impl fmt::Debug for BitgetSignedRequest<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BitgetSignedRequest")
                .field("market", &self.market)
                .field("method", &self.method())
                .field("base_url", &self.base_url)
                .field("request_path", &"<redacted>")
                .field("api_key_header_name", &self.api_key_header_name())
                .field("api_key_header_value", &"<redacted>")
                .field("signature_header_name", &self.signature_header_name())
                .field("timestamp_header_name", &self.timestamp_header_name())
                .field("passphrase_header_name", &self.passphrase_header_name())
                .field("body", &"<redacted>")
                .field("signature", &"<redacted>")
                .finish()
        }
    }

    /// Bitget transport 返回。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct BitgetExecHttpResponse {
        status_code: u16,
        body: String,
    }

    impl BitgetExecHttpResponse {
        pub fn new(status_code: u16, body: impl Into<String>) -> Self {
            Self {
                status_code,
                body: body.into(),
            }
        }

        pub fn status_code(&self) -> u16 {
            self.status_code
        }

        pub fn body(&self) -> &str {
            &self.body
        }

        pub fn is_success(&self) -> bool {
            (200..=299).contains(&self.status_code)
        }
    }

    /// Bitget 可变执行 transport。
    ///
    /// 中文说明：transport 遇到网络断连、TLS 失败或无 HTTP 状态码时必须返回
    /// `UnknownExternalState`，不能把外部未知状态当成下单失败或成功。
    pub trait BitgetExecTransport {
        fn send_signed(
            &mut self,
            request: BitgetSignedRequest<'_>,
        ) -> VenueExecResult<BitgetExecHttpResponse>;
    }

    /// 使用系统 `curl` 发送 Bitget signed REST 请求的真实 transport。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct BitgetCurlExecTransport {
        connect_timeout_secs: u64,
        max_time_secs: u64,
    }

    impl BitgetCurlExecTransport {
        pub fn new(connect_timeout_secs: u64, max_time_secs: u64) -> VenueExecResult<Self> {
            if connect_timeout_secs == 0 || max_time_secs == 0 {
                return Err(VenueExecError::InvalidRequest {
                    field: "curl_timeout",
                    reason: "curl timeouts must be greater than zero",
                });
            }
            Ok(Self {
                connect_timeout_secs,
                max_time_secs,
            })
        }
    }

    impl Default for BitgetCurlExecTransport {
        fn default() -> Self {
            Self {
                connect_timeout_secs: 10,
                max_time_secs: 30,
            }
        }
    }

    impl BitgetExecTransport for BitgetCurlExecTransport {
        fn send_signed(
            &mut self,
            request: BitgetSignedRequest<'_>,
        ) -> VenueExecResult<BitgetExecHttpResponse> {
            let venue_id = bitget_transport_venue_id(request.market());
            let url = bitget_signed_request_url(&request)?;
            let config = bitget_curl_config(&request, &url)?;
            let mut child = Command::new("curl")
                .arg("--silent")
                .arg("--show-error")
                .arg("--request")
                .arg(request.method().as_str())
                .arg("--connect-timeout")
                .arg(self.connect_timeout_secs.to_string())
                .arg("--max-time")
                .arg(self.max_time_secs.to_string())
                .arg("--write-out")
                .arg(format!("{CURL_BITGET_STATUS_MARKER}%{{http_code}}"))
                .arg("--config")
                .arg("-")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "failed to start curl for Bitget signed REST request".to_owned(),
                })?;

            child
                .stdin
                .as_mut()
                .ok_or_else(|| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "curl stdin is unavailable for Bitget signed REST request".to_owned(),
                })?
                .write_all(config.as_bytes())
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: venue_id.clone(),
                    detail: "failed to write curl config for Bitget signed REST request".to_owned(),
                })?;

            let output =
                child
                    .wait_with_output()
                    .map_err(|_| VenueExecError::UnknownExternalState {
                        venue_id: venue_id.clone(),
                        detail: "curl transport did not return a Bitget signed REST response"
                            .to_owned(),
                    })?;
            if !output.status.success() {
                return Err(VenueExecError::UnknownExternalState {
                    venue_id,
                    detail: "curl transport failed before a reliable HTTP response was available"
                        .to_owned(),
                });
            }

            parse_bitget_curl_http_response(&output.stdout, request.market())
        }
    }

    /// Bitget Spot 可变执行适配器。
    pub struct BitgetSpotExecAdapter<S, T> {
        inner: BitgetExecAdapterCore<S, T>,
    }

    impl<S, T> BitgetSpotExecAdapter<S, T> {
        pub fn new(config: BitgetExecConfig, signer: S, transport: T) -> VenueExecResult<Self> {
            ensure_bitget_config_market(&config, BitgetExecMarket::Spot)?;
            Ok(Self {
                inner: BitgetExecAdapterCore::new(config, signer, transport),
            })
        }

        pub fn config(&self) -> &BitgetExecConfig {
            self.inner.config()
        }

        pub fn transport(&self) -> &T {
            self.inner.transport()
        }

        pub fn transport_mut(&mut self) -> &mut T {
            self.inner.transport_mut()
        }
    }

    impl<S, T> fmt::Debug for BitgetSpotExecAdapter<S, T>
    where
        S: fmt::Debug,
        T: fmt::Debug,
    {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BitgetSpotExecAdapter")
                .field("inner", &self.inner)
                .finish()
        }
    }

    /// Bitget USDT-FUTURES 可变执行适配器。
    pub struct BitgetUsdtFuturesExecAdapter<S, T> {
        inner: BitgetExecAdapterCore<S, T>,
    }

    impl<S, T> BitgetUsdtFuturesExecAdapter<S, T> {
        pub fn new(config: BitgetExecConfig, signer: S, transport: T) -> VenueExecResult<Self> {
            ensure_bitget_config_market(&config, BitgetExecMarket::UsdtFutures)?;
            Ok(Self {
                inner: BitgetExecAdapterCore::new(config, signer, transport),
            })
        }

        pub fn config(&self) -> &BitgetExecConfig {
            self.inner.config()
        }

        pub fn transport(&self) -> &T {
            self.inner.transport()
        }

        pub fn transport_mut(&mut self) -> &mut T {
            self.inner.transport_mut()
        }
    }

    impl<S, T> fmt::Debug for BitgetUsdtFuturesExecAdapter<S, T>
    where
        S: fmt::Debug,
        T: fmt::Debug,
    {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BitgetUsdtFuturesExecAdapter")
                .field("inner", &self.inner)
                .finish()
        }
    }

    macro_rules! impl_bitget_adapter_traits {
        ($adapter:ty) => {
            impl<S, T> SubmitOrder for $adapter
            where
                S: BitgetRealSigningProvider,
                T: BitgetExecTransport,
            {
                fn submit_order(
                    &mut self,
                    request: SubmitOrderRequest,
                ) -> VenueExecResult<MutableActionReceipt> {
                    self.inner.submit_order(request)
                }
            }

            impl<S, T> CancelOrder for $adapter
            where
                S: BitgetRealSigningProvider,
                T: BitgetExecTransport,
            {
                fn cancel_order(
                    &mut self,
                    request: CancelOrderRequest,
                ) -> VenueExecResult<MutableActionReceipt> {
                    self.inner.cancel_order(request)
                }
            }

            impl<S, T> QueryActionStatus for $adapter
            where
                S: BitgetRealSigningProvider,
                T: BitgetExecTransport,
            {
                fn query_action_status(
                    &self,
                    request: QueryActionStatusRequest,
                ) -> VenueExecResult<MutableActionStatusReport> {
                    self.inner.query_action_status(request)
                }
            }

            impl<S, T> ConfirmOrderStatus for $adapter
            where
                S: BitgetRealSigningProvider,
                T: BitgetExecTransport,
            {
                fn confirm_order_status(
                    &mut self,
                    request: ConfirmOrderStatusRequest,
                ) -> VenueExecResult<PrivateOrderUpdate> {
                    self.inner.confirm_order_status(request)
                }
            }

            impl<S, T> RequestTransfer for $adapter
            where
                S: BitgetRealSigningProvider,
                T: BitgetExecTransport,
            {
                fn request_transfer(
                    &mut self,
                    request: TransferRequest,
                ) -> VenueExecResult<MutableActionReceipt> {
                    self.inner.request_transfer(request)
                }
            }
        };
    }

    impl_bitget_adapter_traits!(BitgetSpotExecAdapter<S, T>);
    impl_bitget_adapter_traits!(BitgetUsdtFuturesExecAdapter<S, T>);

    #[derive(Debug)]
    struct BitgetExecAdapterCore<S, T> {
        config: BitgetExecConfig,
        signer: S,
        transport: T,
        records_by_key: BTreeMap<IdempotencyKey, LiveActionRecord>,
        key_by_action_id: BTreeMap<MutableActionId, IdempotencyKey>,
        orders_by_client_id: BTreeMap<OrderId, BitgetKnownOrder>,
        orders_by_external_id: BTreeMap<ExternalOrderId, BitgetKnownOrder>,
        next_sequence: u64,
    }

    impl<S, T> BitgetExecAdapterCore<S, T> {
        fn new(config: BitgetExecConfig, signer: S, transport: T) -> Self {
            Self {
                config,
                signer,
                transport,
                records_by_key: BTreeMap::new(),
                key_by_action_id: BTreeMap::new(),
                orders_by_client_id: BTreeMap::new(),
                orders_by_external_id: BTreeMap::new(),
                next_sequence: 0,
            }
        }

        fn config(&self) -> &BitgetExecConfig {
            &self.config
        }

        fn transport(&self) -> &T {
            &self.transport
        }

        fn transport_mut(&mut self) -> &mut T {
            &mut self.transport
        }

        fn query_action_status(
            &self,
            request: QueryActionStatusRequest,
        ) -> VenueExecResult<MutableActionStatusReport> {
            Ok(match request {
                QueryActionStatusRequest::ByActionId(action_id) => {
                    if let Some(key) = self.key_by_action_id.get(&action_id) {
                        self.report_for_key(key)
                    } else {
                        unknown_status_report(Some(action_id), None)
                    }
                }
                QueryActionStatusRequest::ByIdempotencyKey(key) => self.report_for_key(&key),
            })
        }

        fn report_for_key(&self, key: &IdempotencyKey) -> MutableActionStatusReport {
            self.records_by_key.get(key).map_or_else(
                || unknown_status_report(None, Some(key.clone())),
                |record| super::status_report_from_receipt(&record.receipt),
            )
        }

        fn request_transfer(
            &mut self,
            request: TransferRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.from_account_id)?;
            Err(VenueExecError::InvalidRequest {
                field: "transfer",
                reason: "Bitget live transfer is not implemented by this execution adapter",
            })
        }

        fn ensure_request_scope(
            &self,
            venue_id: &VenueId,
            account_id: &AccountId,
        ) -> VenueExecResult<()> {
            if venue_id != &self.config.venue_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "venue_id",
                    reason: "request venue does not match Bitget execution adapter config",
                });
            }
            if account_id != &self.config.account_id {
                return Err(VenueExecError::InvalidRequest {
                    field: "account_id",
                    reason: "request account does not match Bitget execution adapter config",
                });
            }
            Ok(())
        }
    }

    impl<S, T> BitgetExecAdapterCore<S, T>
    where
        S: BitgetRealSigningProvider,
        T: BitgetExecTransport,
    {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            request.validate()?;
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;

            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }

            let symbol = bitget_symbol_from_instrument(self.config.market, &request.instrument_id)?;
            self.ensure_target_leverage(&symbol, request.reduce_only)?;
            let body = bitget_order_create_body(&self.config, &symbol, &request)?;
            let action_id = self.next_action_id(MutableActionKind::SubmitOrder)?;
            let endpoint = self.config.market.place_order_endpoint();
            let signed = self.sign(
                SigningPurpose::SubmitOrder,
                &action_id,
                BitgetRestMethod::Post,
                endpoint.to_owned(),
                body,
            )?;
            let response = self.dispatch_signed(&signed)?;
            self.ensure_business_success(endpoint, &response)?;

            let known_order = bitget_known_order_from_response(
                self.config.market,
                &symbol,
                request.client_order_id.clone(),
                response.body(),
                &action_id,
            )?;
            let external_ref = known_order
                .external_order_id
                .clone()
                .map(ExternalActionRef::Order);
            let receipt = MutableActionReceipt {
                action_id,
                kind: MutableActionKind::SubmitOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref,
                duplicate: false,
                simulated: false,
            };

            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            self.record_known_order(known_order);
            Ok(receipt)
        }

        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let fingerprint = request.fingerprint();
            if let Some(receipt) = self.duplicate_receipt(&request.idempotency_key, &fingerprint)? {
                return Ok(receipt);
            }

            let known_order = self.lookup_known_order(&request.order_ref).ok_or(
                VenueExecError::InvalidRequest {
                    field: "order_ref",
                    reason: "Bitget cancel requires an order previously submitted through this adapter so its symbol is known",
                },
            )?;
            let body = bitget_cancel_order_body(&self.config, &request, known_order)?;
            let action_id = self.next_action_id(MutableActionKind::CancelOrder)?;
            let endpoint = self.config.market.cancel_order_endpoint();
            let signed = self.sign(
                SigningPurpose::CancelOrder,
                &action_id,
                BitgetRestMethod::Post,
                endpoint.to_owned(),
                body,
            )?;
            let response = self.dispatch_signed(&signed)?;
            self.ensure_business_success(endpoint, &response)?;

            let receipt = MutableActionReceipt {
                action_id: action_id.clone(),
                kind: MutableActionKind::CancelOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key.clone(),
                venue_id: request.venue_id,
                external_ref: Some(ExternalActionRef::Cancel(action_id)),
                duplicate: false,
                simulated: false,
            };
            self.record_action(request.idempotency_key, fingerprint, receipt.clone());
            Ok(receipt)
        }

        fn confirm_order_status(
            &mut self,
            request: ConfirmOrderStatusRequest,
        ) -> VenueExecResult<PrivateOrderUpdate> {
            self.ensure_request_scope(&request.venue_id, &request.account_id)?;
            let symbol = bitget_symbol_from_instrument(self.config.market, &request.instrument_id)?;
            let request_path =
                bitget_query_order_request_path(&self.config, &symbol, &request.order_ref)?;
            let signing_request_id =
                order_query_signing_request_id("bitget-exec", &request.source_event_id)?;
            let signed = self.sign_with_request_id(
                SigningPurpose::QueryOrder,
                signing_request_id,
                BitgetRestMethod::Get,
                request_path,
                String::new(),
            )?;
            let response = self.dispatch_signed(&signed)?;
            self.ensure_http_success(self.config.market.order_query_endpoint(), &response)?;
            parse_bitget_order_query_confirmation(
                private_market_from_bitget_exec_market(self.config.market),
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                request.source_event_id,
                response.body(),
            )
        }

        fn ensure_target_leverage(
            &mut self,
            symbol: &str,
            reduce_only: bool,
        ) -> VenueExecResult<()> {
            if reduce_only
                || self.config.market != BitgetExecMarket::UsdtFutures
                || self.config.trade_side.as_deref() == Some("close")
            {
                return Ok(());
            }
            let Some(leverage) = self.config.target_leverage else {
                return Ok(());
            };
            let body = bitget_set_leverage_body(&self.config, symbol, leverage)?;
            let signing_request_id = leverage_signing_request_id("bitget-exec", symbol, leverage)?;
            let signed = self.sign_with_request_id(
                SigningPurpose::SubmitOrder,
                signing_request_id,
                BitgetRestMethod::Post,
                BITGET_MIX_SET_LEVERAGE_ENDPOINT.to_owned(),
                body,
            )?;
            let response = self.dispatch_signed(&signed)?;
            self.ensure_business_success(BITGET_MIX_SET_LEVERAGE_ENDPOINT, &response)
        }

        fn duplicate_receipt(
            &self,
            idempotency_key: &IdempotencyKey,
            fingerprint: &RequestFingerprint,
        ) -> VenueExecResult<Option<MutableActionReceipt>> {
            let Some(existing) = self.records_by_key.get(idempotency_key) else {
                return Ok(None);
            };
            if existing.fingerprint != *fingerprint {
                return Err(VenueExecError::IdempotencyConflict {
                    idempotency_key: idempotency_key.clone(),
                    existing_fingerprint: existing.fingerprint.0.clone(),
                    incoming_fingerprint: fingerprint.0.clone(),
                });
            }

            let mut receipt = existing.receipt.clone();
            receipt.duplicate = true;
            Ok(Some(receipt))
        }

        fn next_action_id(&mut self, kind: MutableActionKind) -> VenueExecResult<MutableActionId> {
            self.next_sequence = self
                .next_sequence
                .checked_add(1)
                .expect("Bitget mutable action sequence overflowed");
            MutableActionId::new(format!(
                "{}:{}:{}",
                self.config.market.token(),
                kind.as_str(),
                self.next_sequence
            ))
        }

        fn sign(
            &self,
            purpose: SigningPurpose,
            action_id: &MutableActionId,
            method: BitgetRestMethod,
            request_path: String,
            body: String,
        ) -> VenueExecResult<BitgetSignedEndpoint> {
            self.sign_with_request_id(
                purpose,
                SigningRequestId::new(format!(
                    "signing-request/bitget-exec/{}",
                    action_id.as_str()
                ))
                .map_err(signing_error)?,
                method,
                request_path,
                body,
            )
        }

        fn sign_with_request_id(
            &self,
            purpose: SigningPurpose,
            signing_request_id: SigningRequestId,
            method: BitgetRestMethod,
            request_path: String,
            body: String,
        ) -> VenueExecResult<BitgetSignedEndpoint> {
            let input = BitgetHmacSigningInput::new(
                signing_request_id,
                self.config.signing_policy.policy_ref().clone(),
                purpose,
                self.config.venue_id.clone(),
                self.config.account_id.clone(),
                method,
                request_path,
                body,
            )
            .map_err(signing_error)?;
            self.signer
                .sign_bitget_hmac(input, &self.config.signing_policy)
                .map_err(signing_error)
        }

        fn dispatch_signed(
            &mut self,
            signed_endpoint: &BitgetSignedEndpoint,
        ) -> VenueExecResult<BitgetExecHttpResponse> {
            let request = BitgetSignedRequest {
                market: self.config.market,
                base_url: &self.config.base_url,
                signed_endpoint,
            };
            self.transport.send_signed(request)
        }

        fn ensure_http_success(
            &self,
            endpoint: &'static str,
            response: &BitgetExecHttpResponse,
        ) -> VenueExecResult<()> {
            if response.is_success() {
                return Ok(());
            }
            Err(VenueExecError::ExternalRejected {
                venue_id: self.config.venue_id.clone(),
                endpoint: endpoint.to_owned(),
                status_code: response.status_code(),
                reason: response_body_snippet(response.body()),
            })
        }

        fn ensure_business_success(
            &self,
            endpoint: &'static str,
            response: &BitgetExecHttpResponse,
        ) -> VenueExecResult<()> {
            self.ensure_http_success(endpoint, response)?;
            match json_field_value(response.body(), "code").as_deref() {
                Some("00000") => Ok(()),
                Some(code) => Err(VenueExecError::ExternalRejected {
                    venue_id: self.config.venue_id.clone(),
                    endpoint: endpoint.to_owned(),
                    status_code: response.status_code(),
                    reason: format!(
                        "Bitget code={code}: {}",
                        json_field_value(response.body(), "msg")
                            .or_else(|| json_field_value(response.body(), "message"))
                            .unwrap_or_else(|| "missing msg".to_owned())
                    ),
                }),
                None => Err(VenueExecError::UnknownExternalState {
                    venue_id: self.config.venue_id.clone(),
                    detail: format!("Bitget response from {endpoint} lacks code"),
                }),
            }
        }

        fn record_action(
            &mut self,
            idempotency_key: IdempotencyKey,
            fingerprint: RequestFingerprint,
            receipt: MutableActionReceipt,
        ) {
            self.key_by_action_id
                .insert(receipt.action_id.clone(), idempotency_key.clone());
            self.records_by_key.insert(
                idempotency_key,
                LiveActionRecord {
                    fingerprint,
                    receipt,
                },
            );
        }

        fn record_known_order(&mut self, known_order: BitgetKnownOrder) {
            if let Some(client_order_id) = known_order.client_order_id.clone() {
                self.orders_by_client_id
                    .insert(client_order_id, known_order.clone());
            }
            if let Some(external_order_id) = known_order.external_order_id.clone() {
                self.orders_by_external_id
                    .insert(external_order_id, known_order);
            }
        }

        fn lookup_known_order(&self, order_ref: &OrderReference) -> Option<&BitgetKnownOrder> {
            match order_ref {
                OrderReference::ClientOrderId(order_id) => self.orders_by_client_id.get(order_id),
                OrderReference::VenueOrderId(order_id) => self.orders_by_external_id.get(order_id),
            }
        }
    }

    fn bitget_set_leverage_body(
        config: &BitgetExecConfig,
        symbol: &str,
        leverage: u32,
    ) -> VenueExecResult<String> {
        validate_perp_target_leverage("target_leverage", leverage)?;
        let mut body = String::from("{");
        let mut first = true;
        push_json_string_field(&mut body, &mut first, "symbol", symbol)?;
        push_json_string_field(&mut body, &mut first, "productType", "USDT-FUTURES")?;
        push_json_string_field(&mut body, &mut first, "marginCoin", config.margin_coin())?;
        push_json_string_field(&mut body, &mut first, "leverage", &leverage.to_string())?;
        body.push('}');
        Ok(body)
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct BitgetKnownOrder {
        symbol: String,
        order_id_param: Option<String>,
        client_order_id: Option<OrderId>,
        external_order_id: Option<ExternalOrderId>,
    }

    fn ensure_bitget_config_market(
        config: &BitgetExecConfig,
        expected: BitgetExecMarket,
    ) -> VenueExecResult<()> {
        if config.market == expected {
            Ok(())
        } else {
            Err(VenueExecError::InvalidRequest {
                field: "market",
                reason: "Bitget execution adapter received config for a different market",
            })
        }
    }

    fn validate_bitget_margin_mode(
        market: BitgetExecMarket,
        margin_mode: &str,
    ) -> VenueExecResult<()> {
        match (market, margin_mode) {
            (BitgetExecMarket::Spot, _) => Ok(()),
            (BitgetExecMarket::UsdtFutures, "crossed" | "isolated") => Ok(()),
            _ => Err(VenueExecError::InvalidRequest {
                field: "margin_mode",
                reason: "Bitget futures marginMode must be crossed or isolated",
            }),
        }
    }

    fn validate_bitget_margin_coin(
        market: BitgetExecMarket,
        margin_coin: &str,
    ) -> VenueExecResult<()> {
        match market {
            BitgetExecMarket::Spot => Ok(()),
            BitgetExecMarket::UsdtFutures if margin_coin == "USDT" => Ok(()),
            BitgetExecMarket::UsdtFutures => Err(VenueExecError::InvalidRequest {
                field: "margin_coin",
                reason: "Bitget USDT-FUTURES marginCoin must be USDT",
            }),
        }
    }

    fn validate_bitget_trade_side(value: &str) -> VenueExecResult<()> {
        match value {
            "open" | "close" => Ok(()),
            _ => Err(VenueExecError::InvalidRequest {
                field: "trade_side",
                reason: "Bitget tradeSide must be open or close",
            }),
        }
    }

    fn normalize_bitget_base_url(value: String) -> VenueExecResult<String> {
        let trimmed = value.trim().trim_end_matches('/').to_owned();
        if trimmed.is_empty() {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Bitget base URL cannot be empty",
            });
        }
        if trimmed
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Bitget base URL contains a control byte",
            });
        }
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://127.0.0.1")) {
            return Err(VenueExecError::InvalidRequest {
                field: "base_url",
                reason: "Bitget base URL must use https or an explicit localhost test URL",
            });
        }
        Ok(trimmed)
    }

    fn bitget_order_create_body(
        config: &BitgetExecConfig,
        symbol: &str,
        request: &SubmitOrderRequest,
    ) -> VenueExecResult<String> {
        let quantity = bitget_order_quantity(config, symbol, request.quantity)?;
        let mut body = String::from("{");
        let mut first = true;
        push_json_string_field(&mut body, &mut first, "symbol", symbol)?;
        if let Some(product_type) = config.market.product_type() {
            push_json_string_field(&mut body, &mut first, "productType", product_type)?;
            push_json_string_field(&mut body, &mut first, "marginMode", config.margin_mode())?;
            push_json_string_field(&mut body, &mut first, "marginCoin", config.margin_coin())?;
        }
        push_json_string_field(&mut body, &mut first, "side", bitget_side(request.side))?;
        if let Some(trade_side) = config.trade_side() {
            push_json_string_field(&mut body, &mut first, "tradeSide", trade_side)?;
        }
        match request.order_type {
            MutableOrderType::Market => {
                push_json_string_field(&mut body, &mut first, "orderType", "market")?;
                push_json_string_field(&mut body, &mut first, "size", &quantity)?;
            }
            MutableOrderType::Limit => {
                push_json_string_field(&mut body, &mut first, "orderType", "limit")?;
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "force",
                    bitget_force(limit_time_in_force(request)),
                )?;
                push_json_string_field(&mut body, &mut first, "size", &quantity)?;
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "price",
                    &request
                        .limit_price
                        .expect("validated limit order price")
                        .to_string(),
                )?;
            }
            MutableOrderType::PostOnly => {
                push_json_string_field(&mut body, &mut first, "orderType", "limit")?;
                push_json_string_field(&mut body, &mut first, "force", "post_only")?;
                push_json_string_field(&mut body, &mut first, "size", &quantity)?;
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "price",
                    &request
                        .limit_price
                        .expect("validated post-only order price")
                        .to_string(),
                )?;
            }
        }
        if request.reduce_only {
            match config.market {
                BitgetExecMarket::Spot => {
                    return Err(VenueExecError::InvalidRequest {
                        field: "reduce_only",
                        reason: "Bitget spot orders do not support reduce_only",
                    });
                }
                BitgetExecMarket::UsdtFutures => {
                    push_json_string_field(&mut body, &mut first, "reduceOnly", "YES")?;
                }
            }
        }
        if let Some(client_order_id) = &request.client_order_id {
            validate_bitget_client_order_id(client_order_id.as_str())?;
            push_json_string_field(&mut body, &mut first, "clientOid", client_order_id.as_str())?;
        }
        body.push('}');
        Ok(body)
    }

    fn bitget_order_quantity(
        config: &BitgetExecConfig,
        symbol: &str,
        quantity: Quantity,
    ) -> VenueExecResult<String> {
        match config.quantity_step(symbol) {
            Some(step) => format_quantity_at_venue_step(quantity, step),
            None => Ok(quantity.to_string()),
        }
    }

    fn bitget_cancel_order_body(
        config: &BitgetExecConfig,
        request: &CancelOrderRequest,
        known_order: &BitgetKnownOrder,
    ) -> VenueExecResult<String> {
        let mut body = String::from("{");
        let mut first = true;
        push_json_string_field(&mut body, &mut first, "symbol", known_order.symbol.as_str())?;
        if let Some(product_type) = config.market.product_type() {
            push_json_string_field(&mut body, &mut first, "productType", product_type)?;
            push_json_string_field(&mut body, &mut first, "marginCoin", config.margin_coin())?;
        }
        match &request.order_ref {
            OrderReference::VenueOrderId(_) => {
                if let Some(order_id) = &known_order.order_id_param {
                    push_json_string_field(&mut body, &mut first, "orderId", order_id)?;
                } else if let Some(client_order_id) = &known_order.client_order_id {
                    push_json_string_field(
                        &mut body,
                        &mut first,
                        "clientOid",
                        client_order_id.as_str(),
                    )?;
                } else {
                    return Err(VenueExecError::InvalidRequest {
                        field: "order_ref",
                        reason: "known Bitget venue order lacks orderId and clientOid",
                    });
                }
            }
            OrderReference::ClientOrderId(client_order_id) => {
                validate_bitget_client_order_id(client_order_id.as_str())?;
                push_json_string_field(
                    &mut body,
                    &mut first,
                    "clientOid",
                    client_order_id.as_str(),
                )?;
            }
        }
        body.push('}');
        Ok(body)
    }

    fn bitget_query_order_request_path(
        config: &BitgetExecConfig,
        symbol: &str,
        order_ref: &OrderReference,
    ) -> VenueExecResult<String> {
        let mut params = if let Some(product_type) = config.market.product_type() {
            vec![
                ("symbol".to_owned(), symbol.to_owned()),
                ("productType".to_owned(), product_type.to_owned()),
            ]
        } else {
            Vec::new()
        };
        match order_ref {
            OrderReference::VenueOrderId(order_id) => {
                let raw_order_id = bitget_order_id_param_from_external(order_id)?;
                params.push(("orderId".to_owned(), raw_order_id.to_owned()));
            }
            OrderReference::ClientOrderId(client_order_id) => {
                validate_bitget_client_order_id(client_order_id.as_str())?;
                params.push(("clientOid".to_owned(), client_order_id.as_str().to_owned()));
            }
        }
        Ok(format!(
            "{}?{}",
            config.market.order_query_endpoint(),
            query_string_from_pairs(&params)
        ))
    }

    fn bitget_order_id_param_from_external(order_id: &ExternalOrderId) -> VenueExecResult<&str> {
        let value = order_id.as_str();
        let raw_order_id = value
            .strip_prefix("bitget-spot:order:")
            .or_else(|| value.strip_prefix("bitget-usdt-futures:order:"))
            .ok_or(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "Bitget venue order ref must come from a Bitget adapter",
            })?;
        validate_bitget_order_id(raw_order_id)?;
        Ok(raw_order_id)
    }

    fn private_market_from_bitget_exec_market(market: BitgetExecMarket) -> PrivateOrderMarket {
        match market {
            BitgetExecMarket::Spot => PrivateOrderMarket::BitgetSpot,
            BitgetExecMarket::UsdtFutures => PrivateOrderMarket::BitgetUsdtFutures,
        }
    }

    fn bitget_symbol_from_instrument(
        market: BitgetExecMarket,
        instrument_id: &InstrumentId,
    ) -> VenueExecResult<String> {
        let value = instrument_id.as_str();
        let mut parts = value.split(':');
        let prefix = parts.next();
        let venue = parts.next();
        let symbol = parts.next();
        let suffix = parts.next();
        if parts.next().is_some()
            || prefix != Some("inst")
            || venue != Some("BITGET")
            || suffix != Some(market.expected_instrument_suffix())
        {
            return Err(VenueExecError::InvalidRequest {
                field: "instrument_id",
                reason: "Bitget execution requires instrument IDs shaped as inst:BITGET:<SYMBOL>:SPOT or inst:BITGET:<SYMBOL>:USDT-FUTURES",
            });
        }
        let symbol = symbol.expect("symbol checked above");
        validate_bitget_symbol(symbol)?;
        Ok(symbol.to_owned())
    }

    fn validate_bitget_symbol(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 32 {
            return Err(VenueExecError::InvalidRequest {
                field: "symbol",
                reason: "Bitget symbol must be 1 to 32 bytes",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit()))
        {
            return Err(VenueExecError::InvalidRequest {
                field: "symbol",
                reason: "Bitget symbol must use uppercase ASCII letters and digits",
            });
        }
        Ok(())
    }

    fn validate_bitget_client_order_id(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 64 {
            return Err(VenueExecError::InvalidRequest {
                field: "client_order_id",
                reason: "Bitget clientOid must be 1 to 64 bytes",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
        {
            return Err(VenueExecError::InvalidRequest {
                field: "client_order_id",
                reason: "Bitget clientOid must use ASCII letters, digits, dash or underscore",
            });
        }
        Ok(())
    }

    fn validate_bitget_order_id(value: &str) -> VenueExecResult<()> {
        if value.is_empty() || value.len() > 96 {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "Bitget orderId must be 1 to 96 bytes",
            });
        }
        if value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
        {
            return Err(VenueExecError::InvalidRequest {
                field: "order_ref",
                reason: "Bitget orderId contains an unsupported byte",
            });
        }
        Ok(())
    }

    fn bitget_known_order_from_response(
        market: BitgetExecMarket,
        symbol: &str,
        client_order_id: Option<OrderId>,
        body: &str,
        action_id: &MutableActionId,
    ) -> VenueExecResult<BitgetKnownOrder> {
        let order_id_param = json_field_value(body, "orderId").filter(|value| !value.is_empty());
        let response_client_order_id = json_field_value(body, "clientOid")
            .filter(|value| !value.is_empty())
            .and_then(|value| OrderId::new(value).ok());
        let client_order_id = client_order_id.or(response_client_order_id);
        let external_order_id = if let Some(order_id) = &order_id_param {
            Some(ExternalOrderId::new(format!(
                "{}:order:{order_id}",
                market.token()
            ))?)
        } else if let Some(client_order_id) = &client_order_id {
            Some(ExternalOrderId::new(format!(
                "{}:client:{}",
                market.token(),
                client_order_id.as_str()
            ))?)
        } else {
            Some(ExternalOrderId::new(format!(
                "{}:action:{}",
                market.token(),
                action_id.as_str()
            ))?)
        };
        Ok(BitgetKnownOrder {
            symbol: symbol.to_owned(),
            order_id_param,
            client_order_id,
            external_order_id,
        })
    }

    fn bitget_side(side: OrderSide) -> &'static str {
        match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }
    }

    fn bitget_force(time_in_force: MutableTimeInForce) -> &'static str {
        match time_in_force {
            MutableTimeInForce::Gtc => "gtc",
            MutableTimeInForce::Ioc => "ioc",
            MutableTimeInForce::Fok => "fok",
        }
    }

    fn bitget_signed_request_url(request: &BitgetSignedRequest<'_>) -> VenueExecResult<String> {
        let request_path = request.request_path();
        if request_path.is_empty() || !request_path.starts_with('/') {
            return Err(VenueExecError::InvalidRequest {
                field: "request_path",
                reason: "Bitget signed REST request path must be an absolute path",
            });
        }
        Ok(format!("{}{}", request.base_url(), request_path))
    }

    fn bitget_curl_config(request: &BitgetSignedRequest<'_>, url: &str) -> VenueExecResult<String> {
        let mut config = format!("url = \"{}\"\n", curl_config_quote(url)?);
        push_curl_header(
            &mut config,
            request.api_key_header_name(),
            request.api_key_header_value(),
        )?;
        push_curl_header(
            &mut config,
            request.signature_header_name(),
            request.signature_header_value(),
        )?;
        let timestamp = request.timestamp_header_value();
        push_curl_header(&mut config, request.timestamp_header_name(), &timestamp)?;
        push_curl_header(
            &mut config,
            request.passphrase_header_name(),
            request.passphrase_header_value(),
        )?;
        push_curl_header(&mut config, "locale", "en-US")?;
        if !request.body_for_transport().is_empty() {
            push_curl_header(&mut config, "Content-Type", "application/json")?;
            config.push_str("data = \"");
            config.push_str(&curl_config_quote(request.body_for_transport())?);
            config.push_str("\"\n");
        }
        Ok(config)
    }

    fn parse_bitget_curl_http_response(
        stdout: &[u8],
        market: BitgetExecMarket,
    ) -> VenueExecResult<BitgetExecHttpResponse> {
        let output = String::from_utf8_lossy(stdout);
        let Some((body, status)) = output.rsplit_once(CURL_BITGET_STATUS_MARKER) else {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: bitget_transport_venue_id(market),
                detail: "curl transport response lacked an HTTP status marker".to_owned(),
            });
        };
        let status_code =
            status
                .trim()
                .parse::<u16>()
                .map_err(|_| VenueExecError::UnknownExternalState {
                    venue_id: bitget_transport_venue_id(market),
                    detail: "curl transport returned a malformed HTTP status".to_owned(),
                })?;
        if status_code == 0 {
            return Err(VenueExecError::UnknownExternalState {
                venue_id: bitget_transport_venue_id(market),
                detail: "curl transport did not receive an HTTP response from Bitget".to_owned(),
            });
        }
        Ok(BitgetExecHttpResponse::new(status_code, body.to_owned()))
    }

    fn bitget_transport_venue_id(market: BitgetExecMarket) -> VenueId {
        let value = match market {
            BitgetExecMarket::Spot => "venue:BITGET-SPOT",
            BitgetExecMarket::UsdtFutures => "venue:BITGET-USDT-FUTURES",
        };
        VenueId::new(value).expect("static Bitget transport venue ID")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExternalRefKind {
    Order,
    Cancel,
    Transfer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RequestFingerprint(String);

#[derive(Clone, Debug)]
struct SimulatedActionRecord {
    fingerprint: RequestFingerprint,
    receipt: MutableActionReceipt,
}

fn status_report_from_receipt(receipt: &MutableActionReceipt) -> MutableActionStatusReport {
    MutableActionStatusReport {
        action_id: Some(receipt.action_id.clone()),
        kind: Some(receipt.kind),
        status: receipt.status,
        idempotency_key: Some(receipt.idempotency_key.clone()),
        external_ref: receipt.external_ref.clone(),
        fail_closed: receipt.status != MutableActionStatus::Accepted,
        simulated: receipt.simulated,
    }
}

fn unknown_status_report(
    action_id: Option<MutableActionId>,
    idempotency_key: Option<IdempotencyKey>,
) -> MutableActionStatusReport {
    MutableActionStatusReport {
        action_id,
        kind: None,
        status: MutableActionStatus::Unknown,
        idempotency_key,
        external_ref: None,
        fail_closed: true,
        simulated: true,
    }
}

fn optional_display<T: fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "none".to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use arb_contracts::from_json_strict;
    use arb_domain::{Decimal, DomainResult};

    use super::*;

    #[cfg(not(feature = "live-exec"))]
    #[test]
    fn default_feature_does_not_enable_live_exec() {
        assert!(!LIVE_EXEC_FEATURE_ENABLED);
    }

    #[test]
    fn duplicate_order_idempotency_key_replays_receipt_without_extra_execution() {
        let mut adapter = SimulatedVenueExecAdapter::new();
        let request = sample_order("idem:order:1", "1.2500").expect("valid order");

        let first = adapter
            .submit_order(request.clone())
            .expect("first simulated order is accepted");
        let second = adapter
            .submit_order(request)
            .expect("duplicate simulated order replays receipt");

        assert!(!first.duplicate);
        assert!(second.duplicate);
        assert_eq!(first.action_id, second.action_id);
        assert_eq!(adapter.recorded_action_count(), 1);
        assert_eq!(
            adapter.executed_action_count(MutableActionKind::SubmitOrder),
            1
        );
    }

    #[test]
    fn same_idempotency_key_with_different_order_payload_fails_closed() {
        let mut adapter = SimulatedVenueExecAdapter::new();
        let first = sample_order("idem:order:conflict", "1.0000").expect("valid first order");
        let second = sample_order("idem:order:conflict", "2.0000").expect("valid second order");

        adapter
            .submit_order(first)
            .expect("first simulated order is accepted");
        let error = adapter
            .submit_order(second)
            .expect_err("payload conflict must fail closed");

        assert!(matches!(error, VenueExecError::IdempotencyConflict { .. }));
        assert_eq!(adapter.recorded_action_count(), 1);
        assert_eq!(
            adapter.executed_action_count(MutableActionKind::SubmitOrder),
            1
        );
    }

    #[test]
    fn cancel_order_trait_records_simulated_cancel_once_per_key() {
        let mut adapter = SimulatedVenueExecAdapter::new();
        let request = CancelOrderRequest::new(
            venue("sim-venue"),
            account("acct:main"),
            OrderReference::VenueOrderId(ExternalOrderId::new("venue-order:1").unwrap()),
            IdempotencyKey::new("idem:cancel:1").unwrap(),
        );

        let first = adapter
            .cancel_order(request.clone())
            .expect("first cancel accepted");
        let second = adapter
            .cancel_order(request)
            .expect("duplicate cancel accepted");

        assert_eq!(first.action_id, second.action_id);
        assert!(second.duplicate);
        assert_eq!(
            adapter.executed_action_count(MutableActionKind::CancelOrder),
            1
        );
    }

    #[test]
    fn query_unknown_action_status_is_explicit_and_fail_closed() {
        let adapter = SimulatedVenueExecAdapter::new();
        let unknown_id = MutableActionId::new("sim:missing:1").unwrap();

        let report = adapter
            .query_action_status(QueryActionStatusRequest::ByActionId(unknown_id.clone()))
            .expect("status query itself is local and deterministic");

        assert_eq!(report.action_id, Some(unknown_id));
        assert_eq!(report.status, MutableActionStatus::Unknown);
        assert!(report.fail_closed);
    }

    #[test]
    fn query_known_action_status_returns_accepted_simulated_report() {
        let mut adapter = SimulatedVenueExecAdapter::new();
        let request = sample_order("idem:order:status", "1.0000").expect("valid order");
        let receipt = adapter
            .submit_order(request)
            .expect("simulated order accepted");

        let report = adapter
            .query_action_status(QueryActionStatusRequest::ByIdempotencyKey(
                receipt.idempotency_key.clone(),
            ))
            .expect("known status query succeeds");

        assert_eq!(report.action_id, Some(receipt.action_id));
        assert_eq!(report.status, MutableActionStatus::Accepted);
        assert!(!report.fail_closed);
        assert!(report.simulated);
    }

    #[test]
    fn binance_spot_execution_report_becomes_private_order_update() {
        let update = parse_binance_spot_execution_report_update(
            venue("venue:BINANCE-SPOT"),
            account("account:binance-unit"),
            "event:binance:spot:execution-report:1",
            r#"{"e":"executionReport","E":1700000000123,"s":"BTCUSDT","c":"client:spot:1","S":"BUY","x":"TRADE","X":"FILLED","i":12345,"l":"0.001","L":"43100.50","z":"0.001","n":"0.01","N":"USDT","T":1700000000123}"#,
        )
        .expect("spot private order update");

        assert_eq!(update.source, OrderConfirmationSource::PrivateStream);
        assert_eq!(update.market, PrivateOrderMarket::Spot);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.instrument_id.as_str(), "inst:BINANCE:BTCUSDT:SPOT");
        assert_eq!(
            update.client_order_id.as_ref().expect("client id").as_str(),
            "client:spot:1"
        );
        assert_eq!(
            update
                .venue_order_id
                .as_ref()
                .expect("venue order")
                .as_str(),
            "binance:spot:order:12345"
        );
        let fill = update.last_fill.expect("trade event carries last fill");
        assert_eq!(fill.timestamp, "2023-11-14T22:13:20.123Z");
        assert_eq!(fill.price, "43100.50");
        assert_eq!(fill.quantity, "0.001");
        assert_eq!(
            fill.fee_asset_id.as_ref().expect("fee asset").as_str(),
            "asset:USDT"
        );
        assert_eq!(fill.fee_amount.expect("fee amount").to_string(), "0.01");
    }

    #[test]
    fn binance_usdm_order_trade_update_parses_nested_order_payload() {
        let update = parse_binance_usdm_order_trade_update(
            venue("venue:BINANCE-USDM"),
            account("account:binance-usdm-unit"),
            "event:binance:usdm:order-trade-update:1",
            r#"{"e":"ORDER_TRADE_UPDATE","E":1700000000456,"T":1700000000456,"o":{"s":"BTCUSDT","c":"client:usdm:1","S":"SELL","x":"TRADE","X":"PARTIALLY_FILLED","i":991,"l":"0.002","L":"43200.00","z":"0.002","n":"0","N":"USDT","T":1700000000456}}"#,
        )
        .expect("usdm private order update");

        assert_eq!(update.market, PrivateOrderMarket::UsdmFutures);
        assert_eq!(update.status, OrderConfirmationStatus::PartiallyFilled);
        assert_eq!(
            update.instrument_id.as_str(),
            "inst:BINANCE:BTCUSDT:USDM-PERP"
        );
        assert_eq!(update.side, Some(OrderSide::Sell));
        assert_eq!(
            update.last_fill.as_ref().expect("fill").timestamp,
            "2023-11-14T22:13:20.456Z"
        );
    }

    #[test]
    fn binance_order_query_confirms_status_without_treating_rest_submit_as_final() {
        let update = parse_binance_order_query_confirmation(
            PrivateOrderMarket::Spot,
            venue("venue:BINANCE-SPOT"),
            account("account:binance-unit"),
            "event:binance:spot:query:1",
            r#"{"symbol":"BTCUSDT","orderId":12345,"clientOrderId":"client:spot:1","status":"NEW","executedQty":"0","price":"43100.50","time":1700000000123}"#,
        )
        .expect("order query confirmation");

        assert_eq!(update.source, OrderConfirmationSource::OrderQuery);
        assert_eq!(update.status, OrderConfirmationStatus::Acknowledged);
        assert!(!update.status.is_terminal());
        assert!(update.last_fill.is_none());
    }

    #[test]
    fn aster_order_query_confirms_filled_perp_order() {
        let update = parse_aster_order_query_confirmation(
            venue("venue:ASTER-USDT-FUTURES"),
            account("account:aster-unit"),
            "event:aster:perp:query:filled",
            r#"{"symbol":"BTCUSDT","orderId":22542179,"clientOrderId":"asterUnit1","side":"BUY","status":"FILLED","type":"LIMIT","origType":"LIMIT","executedQty":"0.001","avgPrice":"43100.50","updateTime":1700000001500}"#,
        )
        .expect("Aster order query confirmation");

        assert_eq!(update.source, OrderConfirmationSource::OrderQuery);
        assert_eq!(update.market, PrivateOrderMarket::AsterPerp);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.side, Some(OrderSide::Buy));
        assert_eq!(
            update.instrument_id.as_str(),
            "inst:ASTER:BTCUSDT:USDT-FUTURES"
        );
        assert_eq!(
            update
                .venue_order_id
                .as_ref()
                .expect("venue order id")
                .as_str(),
            "aster-perp:order:22542179"
        );
        assert_eq!(
            update.client_order_id.as_ref().expect("client id").as_str(),
            "asterUnit1"
        );
        assert_eq!(
            update
                .cumulative_filled_quantity
                .expect("cumulative quantity")
                .to_string(),
            "0.001"
        );
        let fill = update.last_fill.expect("filled query carries fill summary");
        assert_eq!(fill.timestamp, "2023-11-14T22:13:21.5Z");
        assert_eq!(fill.price, "43100.50");
        assert_eq!(fill.quantity, "0.001");
    }

    #[test]
    fn hyperliquid_order_status_confirms_filled_perp_order() {
        let update = parse_hyperliquid_order_query_confirmation(
            venue("venue:HYPERLIQUID-PERP"),
            account("account:hyperliquid-unit"),
            "event:hyperliquid:perp:query:filled",
            r#"{"status":"order","order":{"order":{"coin":"BTC","side":"B","limitPx":"43100.50","sz":"0.0","oid":77747314,"timestamp":1700000001000,"reduceOnly":false,"orderType":"Limit","origSz":"0.001","tif":"Ioc","cloid":"0x1234567890abcdef1234567890abcdef"},"status":"filled","statusTimestamp":1700000001500}}"#,
        )
        .expect("Hyperliquid orderStatus confirmation");

        assert_eq!(update.source, OrderConfirmationSource::OrderQuery);
        assert_eq!(update.market, PrivateOrderMarket::HyperliquidPerp);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.side, Some(OrderSide::Buy));
        assert_eq!(
            update.instrument_id.as_str(),
            "inst:HYPERLIQUID:BTCUSDT:PERP"
        );
        assert_eq!(
            update
                .venue_order_id
                .as_ref()
                .expect("venue order id")
                .as_str(),
            "hyperliquid-perp:order:77747314"
        );
        assert_eq!(
            update.client_order_id.as_ref().expect("client id").as_str(),
            "0x1234567890abcdef1234567890abcdef"
        );
        assert_eq!(
            update
                .cumulative_filled_quantity
                .expect("cumulative quantity")
                .to_string(),
            "0.001"
        );
        assert!(update.last_fill.is_none());
    }

    #[test]
    fn bybit_order_query_confirms_filled_linear_order() {
        let update = parse_bybit_order_query_confirmation(
            PrivateOrderMarket::BybitLinear,
            venue("venue:BYBIT-LINEAR"),
            account("account:bybit-unit"),
            "event:bybit:linear:query:filled",
            r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[{"symbol":"BTCUSDT","orderId":"123456789","orderLinkId":"client:bybit:1","side":"Sell","orderStatus":"Filled","cumExecQty":"0.001","avgPrice":"43100.50","cumExecFee":"0.0100","feeCurrency":"USDT","updatedTime":"1700000001500"}]},"retExtInfo":{},"time":1700000002000}"#,
        )
        .expect("bybit order query confirmation");

        assert_eq!(update.source, OrderConfirmationSource::OrderQuery);
        assert_eq!(update.market, PrivateOrderMarket::BybitLinear);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.side, Some(OrderSide::Sell));
        assert_eq!(
            update.instrument_id.as_str(),
            "inst:BYBIT:BTCUSDT:LINEAR-PERP"
        );
        assert_eq!(
            update
                .venue_order_id
                .as_ref()
                .expect("venue order id")
                .as_str(),
            "bybit-linear:order:123456789"
        );
        assert_eq!(
            update.client_order_id.as_ref().expect("client id").as_str(),
            "client:bybit:1"
        );
        assert_eq!(
            update
                .cumulative_filled_quantity
                .expect("cumulative quantity")
                .to_string(),
            "0.001"
        );
        let fill = update.last_fill.expect("filled query carries fill summary");
        assert_eq!(fill.timestamp, "2023-11-14T22:13:21.5Z");
        assert_eq!(fill.price, "43100.50");
        assert_eq!(fill.quantity, "0.001");
        assert_eq!(
            fill.fee_asset_id.as_ref().expect("fee asset").as_str(),
            "asset:USDT"
        );
        assert_eq!(fill.fee_amount.expect("fee amount").to_string(), "0.0100");
    }

    #[test]
    fn bybit_order_query_fails_closed_on_nonzero_ret_code() {
        let err = parse_bybit_order_query_confirmation(
            PrivateOrderMarket::BybitLinear,
            venue("venue:BYBIT-LINEAR"),
            account("account:bybit-unit"),
            "event:bybit:linear:query:rejected",
            r#"{"retCode":10001,"retMsg":"request parameter error","result":{"category":"linear","list":[]},"time":1700000002000}"#,
        )
        .expect_err("non-zero Bybit retCode fails closed");

        assert!(matches!(err, VenueExecError::UnknownExternalState { .. }));
    }

    #[test]
    fn bybit_private_order_stream_confirms_filled_linear_order() {
        let update = parse_bybit_private_order_stream_update(
            PrivateOrderMarket::BybitLinear,
            venue("venue:BYBIT-LINEAR"),
            account("account:bybit-unit"),
            "event:bybit:private-order-stream:linear:1",
            r#"{"topic":"order","id":"stream-1","creationTime":1700000002500,"data":[{"category":"linear","symbol":"BTCUSDT","orderId":"123456789","orderLinkId":"client:bybit:1","side":"Sell","orderStatus":"Filled","cumExecQty":"0.001","avgPrice":"43100.50","cumExecFee":"0.0100","feeCurrency":"USDT","updatedTime":"1700000002500"}]}"#,
        )
        .expect("bybit private stream update");

        assert_eq!(update.source, OrderConfirmationSource::PrivateStream);
        assert_eq!(update.market, PrivateOrderMarket::BybitLinear);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(
            update.instrument_id.as_str(),
            "inst:BYBIT:BTCUSDT:LINEAR-PERP"
        );
        assert_eq!(
            update.client_order_id.as_ref().expect("client id").as_str(),
            "client:bybit:1"
        );
        assert_eq!(
            update
                .cumulative_filled_quantity
                .expect("cumulative quantity")
                .to_string(),
            "0.001"
        );
    }

    #[test]
    fn bybit_private_order_stream_rejects_category_market_mismatch() {
        let err = parse_bybit_private_order_stream_update(
            PrivateOrderMarket::BybitSpot,
            venue("venue:BYBIT-SPOT"),
            account("account:bybit-unit"),
            "event:bybit:private-order-stream:spot:mismatch",
            r#"{"topic":"order.linear","creationTime":1700000002500,"data":[{"symbol":"BTCUSDT","orderId":"123456789","orderLinkId":"client:bybit:1","side":"Buy","orderStatus":"New","cumExecQty":"0","updatedTime":"1700000002500"}]}"#,
        )
        .expect_err("topic category and configured market must match");

        assert!(matches!(
            err,
            VenueExecError::InvalidRequest {
                field: "category",
                ..
            }
        ));
    }

    #[test]
    fn bybit_order_query_rejects_category_market_mismatch() {
        let err = parse_bybit_order_query_confirmation(
            PrivateOrderMarket::BybitSpot,
            venue("venue:BYBIT-SPOT"),
            account("account:bybit-unit"),
            "event:bybit:spot:query:mismatch",
            r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[{"symbol":"BTCUSDT","orderId":"123456789","side":"Buy","orderStatus":"New","cumExecQty":"0","updatedTime":"1700000001500"}]},"time":1700000002000}"#,
        )
        .expect_err("category and configured market must match");

        assert!(matches!(
            err,
            VenueExecError::InvalidRequest {
                field: "category",
                ..
            }
        ));
    }

    #[test]
    fn okx_order_query_confirms_filled_swap_order() {
        let update = parse_okx_order_query_confirmation(
            PrivateOrderMarket::OkxSwap,
            venue("venue:OKX-SWAP"),
            account("account:okx-unit"),
            "event:okx:swap:query:filled",
            r#"{"code":"0","msg":"","data":[{"instId":"BTC-USDT-SWAP","ordId":"312269865356374016","clOrdId":"rvoP1778630400","side":"sell","state":"filled","accFillSz":"0.001","avgPx":"43100.50","fillFee":"-0.0100","fillFeeCcy":"USDT","uTime":"1700000001500","tradeId":"901"}]}"#,
        )
        .expect("okx order query confirmation");

        assert_eq!(update.source, OrderConfirmationSource::OrderQuery);
        assert_eq!(update.market, PrivateOrderMarket::OkxSwap);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.side, Some(OrderSide::Sell));
        assert_eq!(update.instrument_id.as_str(), "inst:OKX:BTC-USDT-SWAP:SWAP");
        assert_eq!(update.symbol, "BTC-USDT");
        assert_eq!(
            update
                .venue_order_id
                .as_ref()
                .expect("venue order id")
                .as_str(),
            "okx-swap:order:312269865356374016"
        );
        assert_eq!(
            update.client_order_id.as_ref().expect("client id").as_str(),
            "rvoP1778630400"
        );
        assert_eq!(
            update
                .cumulative_filled_quantity
                .expect("cumulative quantity")
                .to_string(),
            "0.001"
        );
        let fill = update.last_fill.expect("filled query carries fill summary");
        assert_eq!(fill.timestamp, "2023-11-14T22:13:21.5Z");
        assert_eq!(fill.price, "43100.50");
        assert_eq!(fill.quantity, "0.001");
        assert_eq!(
            fill.fee_asset_id.as_ref().expect("fee asset").as_str(),
            "asset:USDT"
        );
        assert_eq!(fill.fee_amount.expect("fee amount").to_string(), "0.0100");
        assert_eq!(fill.trade_id.as_deref(), Some("901"));
    }

    #[test]
    fn okx_private_order_stream_confirms_filled_spot_order() {
        let update = parse_okx_private_order_stream_update(
            PrivateOrderMarket::OkxSpot,
            venue("venue:OKX-SPOT"),
            account("account:okx-unit"),
            "event:okx:private-order-stream:spot:1",
            r#"{"arg":{"channel":"orders","instType":"SPOT"},"ts":"1700000002500","data":[{"instId":"BTC-USDT","ordId":"512269865356374016","clOrdId":"rvoS1778630400","side":"buy","state":"filled","accFillSz":"0.001","fillSz":"0.001","fillPx":"43100.50","fillFee":"-0.0100","fillFeeCcy":"USDT","uTime":"1700000002500","tradeId":"902"}]}"#,
        )
        .expect("okx private stream update");

        assert_eq!(update.source, OrderConfirmationSource::PrivateStream);
        assert_eq!(update.market, PrivateOrderMarket::OkxSpot);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.side, Some(OrderSide::Buy));
        assert_eq!(update.instrument_id.as_str(), "inst:OKX:BTC-USDT:SPOT");
        assert_eq!(
            update
                .cumulative_filled_quantity
                .expect("cumulative quantity")
                .to_string(),
            "0.001"
        );
        assert_eq!(
            update.last_fill.as_ref().expect("fill").trade_id.as_deref(),
            Some("902")
        );
    }

    #[test]
    fn okx_private_order_stream_rejects_market_mismatch() {
        let err = parse_okx_private_order_stream_update(
            PrivateOrderMarket::OkxSpot,
            venue("venue:OKX-SPOT"),
            account("account:okx-unit"),
            "event:okx:private-order-stream:spot:mismatch",
            r#"{"arg":{"channel":"orders","instType":"SWAP"},"data":[{"instId":"BTC-USDT-SWAP","ordId":"312","side":"sell","state":"live","uTime":"1700000002500"}]}"#,
        )
        .expect_err("OKX spot parser must reject swap instId");

        assert!(matches!(
            err,
            VenueExecError::InvalidRequest {
                field: "instId",
                ..
            }
        ));
    }

    #[test]
    fn bitget_order_query_confirms_filled_usdt_futures_order() {
        let update = parse_bitget_order_query_confirmation(
            PrivateOrderMarket::BitgetUsdtFutures,
            venue("venue:BITGET-USDT-FUTURES"),
            account("account:bitget-unit"),
            "event:bitget:usdt-futures:query:filled",
            r#"{"code":"00000","msg":"success","requestTime":1700000002000,"data":{"symbol":"BTCUSDT","orderId":"1234567890","clientOid":"bitgetUnit1","side":"sell","status":"filled","priceAvg":"43100.50","baseVolume":"0.001","fee":"-0.0100","uTime":"1700000001500","tradeId":"901","orderSource":"normal"}}"#,
        )
        .expect("bitget order query confirmation");

        assert_eq!(update.source, OrderConfirmationSource::OrderQuery);
        assert_eq!(update.market, PrivateOrderMarket::BitgetUsdtFutures);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.side, Some(OrderSide::Sell));
        assert_eq!(
            update.instrument_id.as_str(),
            "inst:BITGET:BTCUSDT:USDT-FUTURES"
        );
        assert_eq!(
            update
                .venue_order_id
                .as_ref()
                .expect("venue order id")
                .as_str(),
            "bitget-usdt-futures:order:1234567890"
        );
        assert_eq!(
            update.client_order_id.as_ref().expect("client id").as_str(),
            "bitgetUnit1"
        );
        assert_eq!(
            update
                .cumulative_filled_quantity
                .expect("cumulative quantity")
                .to_string(),
            "0.001"
        );
        let fill = update.last_fill.expect("filled query carries fill summary");
        assert_eq!(fill.timestamp, "2023-11-14T22:13:21.5Z");
        assert_eq!(fill.price, "43100.50");
        assert_eq!(fill.quantity, "0.001");
        assert_eq!(fill.fee_amount.expect("fee amount").to_string(), "0.0100");
        assert_eq!(fill.trade_id.as_deref(), Some("901"));
    }

    #[test]
    fn bitget_private_order_stream_confirms_filled_spot_order() {
        let update = parse_bitget_private_order_stream_update(
            PrivateOrderMarket::BitgetSpot,
            venue("venue:BITGET-SPOT"),
            account("account:bitget-unit"),
            "event:bitget:private-order-stream:spot:1",
            r#"{"arg":{"channel":"orders","instType":"SPOT"},"ts":"1700000002500","data":[{"symbol":"btcusdt","orderId":"22334455","clientOid":"bitgetSpot1","side":"buy","status":"filled","baseVolume":"0.002","fillPrice":"43100.50","fillFee":"-0.0200","fillFeeCoin":"USDT","fillTime":"1700000002500","tradeId":"902"}]}"#,
        )
        .expect("bitget private stream update");

        assert_eq!(update.source, OrderConfirmationSource::PrivateStream);
        assert_eq!(update.market, PrivateOrderMarket::BitgetSpot);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.side, Some(OrderSide::Buy));
        assert_eq!(update.instrument_id.as_str(), "inst:BITGET:BTCUSDT:SPOT");
        assert_eq!(
            update
                .cumulative_filled_quantity
                .expect("cumulative quantity")
                .to_string(),
            "0.002"
        );
        let fill = update.last_fill.expect("stream trade carries last fill");
        assert_eq!(
            fill.fee_asset_id.as_ref().expect("fee asset").as_str(),
            "asset:USDT"
        );
        assert_eq!(fill.fee_amount.expect("fee amount").to_string(), "0.0200");
        assert_eq!(fill.trade_id.as_deref(), Some("902"));
    }

    #[test]
    fn bitget_private_order_stream_rejects_market_mismatch() {
        let err = parse_bitget_private_order_stream_update(
            PrivateOrderMarket::BitgetSpot,
            venue("venue:BITGET-SPOT"),
            account("account:bitget-unit"),
            "event:bitget:private-order-stream:mismatch",
            r#"{"arg":{"channel":"orders","instType":"USDT-FUTURES"},"ts":"1700000002500","data":[{"symbol":"BTCUSDT","orderId":"22334455","side":"sell","status":"live","uTime":"1700000002500"}]}"#,
        )
        .expect_err("Bitget spot parser must reject futures instType");

        assert!(matches!(
            err,
            VenueExecError::InvalidRequest {
                field: "instType",
                ..
            }
        ));
    }

    #[test]
    fn duplicate_transfer_idempotency_key_does_not_move_funds_twice() {
        let mut adapter = SimulatedVenueExecAdapter::new();
        let request = TransferRequest::new(
            venue("sim-venue"),
            account("acct:source"),
            account("acct:destination"),
            asset("asset:USDC"),
            amount("100.000000").expect("valid amount"),
            IdempotencyKey::new("idem:transfer:1").unwrap(),
        );

        let first = adapter
            .request_transfer(request.clone())
            .expect("first simulated transfer accepted");
        let second = adapter
            .request_transfer(request)
            .expect("duplicate simulated transfer accepted");

        assert_eq!(first.action_id, second.action_id);
        assert!(second.duplicate);
        assert_eq!(
            adapter.executed_action_count(MutableActionKind::TransferRequest),
            1
        );
    }

    #[test]
    fn limit_order_requires_limit_price_and_market_order_rejects_limit_price() {
        let missing_limit = SubmitOrderRequest::new(
            venue("sim-venue"),
            account("acct:main"),
            instrument("BTC-USDC-PERP"),
            OrderSide::Buy,
            MutableOrderType::Limit,
            quantity("1.0").unwrap(),
            None,
            None,
            IdempotencyKey::new("idem:order:missing-limit").unwrap(),
        );
        let market_with_limit = SubmitOrderRequest::new(
            venue("sim-venue"),
            account("acct:main"),
            instrument("BTC-USDC-PERP"),
            OrderSide::Buy,
            MutableOrderType::Market,
            quantity("1.0").unwrap(),
            Some(price("10.0").unwrap()),
            None,
            IdempotencyKey::new("idem:order:market-limit").unwrap(),
        );

        assert!(missing_limit.validate().is_err());
        assert!(market_with_limit.validate().is_err());
    }

    #[test]
    fn time_in_force_is_only_valid_for_plain_limit_orders() {
        let market_ioc = SubmitOrderRequest::new(
            venue("sim-venue"),
            account("acct:main"),
            instrument("BTC-USDC-PERP"),
            OrderSide::Buy,
            MutableOrderType::Market,
            quantity("1.0").unwrap(),
            None,
            None,
            IdempotencyKey::new("idem:order:market-ioc").unwrap(),
        )
        .with_time_in_force(MutableTimeInForce::Ioc);
        let post_only_ioc = SubmitOrderRequest::new(
            venue("sim-venue"),
            account("acct:main"),
            instrument("BTC-USDC-PERP"),
            OrderSide::Buy,
            MutableOrderType::PostOnly,
            quantity("1.0").unwrap(),
            Some(price("10.0").unwrap()),
            None,
            IdempotencyKey::new("idem:order:post-only-ioc").unwrap(),
        )
        .with_time_in_force(MutableTimeInForce::Ioc);
        let limit_ioc = SubmitOrderRequest::new(
            venue("sim-venue"),
            account("acct:main"),
            instrument("BTC-USDC-PERP"),
            OrderSide::Buy,
            MutableOrderType::Limit,
            quantity("1.0").unwrap(),
            Some(price("10.0").unwrap()),
            None,
            IdempotencyKey::new("idem:order:limit-ioc").unwrap(),
        )
        .with_time_in_force(MutableTimeInForce::Ioc);

        assert!(market_ioc.validate().is_err());
        assert!(post_only_ioc.validate().is_err());
        assert!(limit_ioc.validate().is_ok());
    }

    #[test]
    fn execution_plan_maps_to_submit_order_requests_after_final_gates() {
        let plan = basis_execution_plan();
        let policy = dispatch_policy("10.00")
            .with_manual_gate_released(true)
            .allow_symbol("BTCUSDT")
            .expect("symbol");

        let requests =
            submit_order_requests_from_execution_plan(&plan, &policy, dispatch_time("00:00:10"))
                .expect("mapped requests");

        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].venue_id.as_str(), "venue:BINANCE-SPOT");
        assert_eq!(
            requests[0].instrument_id.as_str(),
            "inst:BINANCE:BTCUSDT:SPOT"
        );
        assert_eq!(requests[0].side, OrderSide::Buy);
        assert_eq!(requests[0].order_type, MutableOrderType::Limit);
        assert_eq!(requests[0].time_in_force, Some(MutableTimeInForce::Ioc));
        assert_eq!(requests[0].quantity.to_string(), "0.001");
        assert_eq!(
            requests[0].limit_price.expect("spot limit").to_string(),
            "50000.00"
        );
        assert_eq!(requests[0].idempotency_key.as_str(), "idem:plan:basis:spot");
        assert_eq!(requests[1].venue_id.as_str(), "venue:BINANCE-USDM");
        assert_eq!(requests[1].side, OrderSide::Sell);
        assert_eq!(requests[1].time_in_force, Some(MutableTimeInForce::Ioc));
        assert_eq!(requests[1].idempotency_key.as_str(), "idem:plan:basis:perp");
    }

    #[test]
    fn execution_dispatch_blocks_manual_gate_kill_switch_cap_and_timeout() {
        let plan = basis_execution_plan();
        let base = dispatch_policy("10.00")
            .allow_symbol("BTCUSDT")
            .expect("symbol");

        assert!(matches!(
            submit_order_requests_from_execution_plan(&plan, &base, dispatch_time("00:00:10")),
            Err(VenueExecError::DispatchBlocked { .. })
        ));

        let released = base.clone().with_manual_gate_released(true);
        let too_small = dispatch_policy("1.00")
            .with_manual_gate_released(true)
            .allow_symbol("BTCUSDT")
            .expect("symbol");
        assert!(matches!(
            submit_order_requests_from_execution_plan(&plan, &too_small, dispatch_time("00:00:10")),
            Err(VenueExecError::DispatchBlocked { .. })
        ));

        let killed = released.clone().with_kill_switch(
            DispatchKillSwitch::default()
                .with_symbol("BTCUSDT")
                .expect("kill switch symbol"),
        );
        assert!(matches!(
            submit_order_requests_from_execution_plan(&plan, &killed, dispatch_time("00:00:10")),
            Err(VenueExecError::DispatchBlocked { .. })
        ));

        assert!(matches!(
            submit_order_requests_from_execution_plan(&plan, &released, dispatch_time("00:02:00")),
            Err(VenueExecError::DispatchBlocked { .. })
        ));
    }

    #[test]
    fn basis_dispatch_spot_first_and_reports_residual_risk_when_perp_fails() {
        let plan = basis_execution_plan();
        let policy = dispatch_policy("10.00")
            .with_manual_gate_released(true)
            .allow_symbol("BTCUSDT")
            .expect("symbol");
        let mut adapter = FailSecondSubmitOrder::default();

        let outcome =
            dispatch_execution_plan(&mut adapter, &plan, &policy, dispatch_time("00:00:10"))
                .expect("dispatch outcome");

        assert!(!outcome.completed());
        assert_eq!(outcome.receipts.len(), 1);
        assert_eq!(outcome.receipts[0].venue_id.as_str(), "venue:BINANCE-SPOT");
        assert_eq!(
            outcome.failure.as_ref().expect("failure").plan_leg_id,
            "pleg:plan:basis:0002"
        );
        assert!(outcome
            .residual_risk
            .as_ref()
            .expect("residual risk")
            .detail
            .contains("residual long spot exposure"));
    }

    #[test]
    fn funding_arb_dispatch_builds_perp_long_and_short_requests() {
        let plan = funding_arb_execution_plan();
        let policy = dispatch_policy("10.00")
            .with_manual_gate_released(true)
            .allow_symbol("BTCUSDT")
            .expect("symbol");

        let dispatch_plan =
            build_execution_dispatch_plan(&plan, &policy, dispatch_time("00:00:10"))
                .expect("funding arb dispatch plan");

        assert_eq!(dispatch_plan.requests.len(), 2);
        assert_eq!(
            dispatch_plan.requests[0].basis_leg_role.as_deref(),
            Some("perp_long")
        );
        assert_eq!(
            dispatch_plan.requests[1].basis_leg_role.as_deref(),
            Some("perp_short")
        );
        assert_eq!(
            dispatch_plan.requests[0].request.venue_id.as_str(),
            "venue:BINANCE-USDM"
        );
        assert_eq!(dispatch_plan.requests[0].request.side, OrderSide::Buy);
        assert_eq!(
            dispatch_plan.requests[1].request.venue_id.as_str(),
            "venue:BYBIT-LINEAR"
        );
        assert_eq!(dispatch_plan.requests[1].request.side, OrderSide::Sell);
    }

    #[test]
    fn funding_arb_dispatch_reports_residual_perp_exposure_when_second_leg_fails() {
        let plan = funding_arb_execution_plan();
        let policy = dispatch_policy("10.00")
            .with_manual_gate_released(true)
            .allow_symbol("BTCUSDT")
            .expect("symbol");
        let mut adapter = FailSecondSubmitOrder::default();

        let outcome =
            dispatch_execution_plan(&mut adapter, &plan, &policy, dispatch_time("00:00:10"))
                .expect("dispatch outcome");

        assert!(!outcome.completed());
        assert_eq!(outcome.receipts.len(), 1);
        assert_eq!(
            outcome
                .residual_risk
                .as_ref()
                .expect("residual risk")
                .severity,
            "RiskCritical"
        );
        assert!(outcome
            .residual_risk
            .as_ref()
            .expect("residual risk")
            .detail
            .contains("directional perp exposure"));
    }

    #[test]
    fn live_like_rest_receipt_stops_before_dependent_leg_until_private_confirmation() {
        let plan = basis_execution_plan();
        let policy = dispatch_policy("10.00")
            .with_manual_gate_released(true)
            .allow_symbol("BTCUSDT")
            .expect("symbol");
        let mut adapter = LiveLikeAcceptedSubmitOrder::default();

        let outcome =
            dispatch_execution_plan(&mut adapter, &plan, &policy, dispatch_time("00:00:10"))
                .expect("dispatch outcome");

        assert!(!outcome.completed());
        assert!(!outcome.submission_completed());
        assert!(outcome.requires_private_confirmation());
        assert_eq!(outcome.receipts.len(), 1);
        assert_eq!(adapter.calls, 1);
        assert!(outcome
            .failure
            .as_ref()
            .expect("confirmation gate")
            .detail
            .contains("private order stream or order query confirmation"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_spot_adapter_signs_dispatches_and_deduplicates_order() {
        let mut adapter = live::BinanceSpotExecAdapter::new(
            live::BinanceExecConfig::spot(
                venue("venue:BINANCE-SPOT"),
                account("account:binance-unit"),
                "https://api.binance.com",
                binance_signing_policy("kms-policy/binance-spot-exec-unit"),
            )
            .unwrap(),
            binance_test_signer(1_700_000_000_123),
            RecordingTransport::ok(
                200,
                r#"{"symbol":"BTCUSDT","orderId":12345,"clientOrderId":"client:spot:1","status":"NEW"}"#,
            ),
        )
        .unwrap();
        let request = SubmitOrderRequest::new(
            venue("venue:BINANCE-SPOT"),
            account("account:binance-unit"),
            instrument("inst:BINANCE:BTCUSDT:SPOT"),
            OrderSide::Buy,
            MutableOrderType::Limit,
            quantity("0.001").unwrap(),
            Some(price("43100.50").unwrap()),
            Some(OrderId::new("client:spot:1").unwrap()),
            IdempotencyKey::new("idem:binance:spot:1").unwrap(),
        );

        let receipt = adapter
            .submit_order(request.clone())
            .expect("signed spot order dispatches");
        let duplicate = adapter
            .submit_order(request)
            .expect("same idempotency key replays receipt");

        assert!(!receipt.simulated);
        assert_eq!(receipt.status, MutableActionStatus::Accepted);
        assert!(duplicate.duplicate);
        assert_eq!(adapter.transport().calls.len(), 1);
        let call = &adapter.transport().calls[0];
        assert_eq!(call.market, live::BinanceExecMarket::Spot);
        assert_eq!(call.method, live::BinanceExecHttpMethod::Post);
        assert_eq!(call.endpoint, live::BINANCE_SPOT_ORDER_ENDPOINT);
        assert_eq!(call.api_key_header_name, "X-MBX-APIKEY");
        assert!(call.signed_query.contains("symbol=BTCUSDT"));
        assert!(call.signed_query.contains("side=BUY"));
        assert!(call.signed_query.contains("type=LIMIT"));
        assert!(call.signed_query.contains("timeInForce=GTC"));
        assert!(call.signed_query.contains("quantity=0.001"));
        assert!(call.signed_query.contains("price=43100.50"));
        assert!(call.signed_query.contains("timestamp=1700000000123"));
        assert!(call.signed_query.contains("&signature="));
        assert!(!call.debug.contains(&call.api_key_header_value));
        assert!(!call.debug.contains(&call.signed_query));

        let external_order_id = match receipt.external_ref.expect("external order ref") {
            ExternalActionRef::Order(order_id) => order_id,
            other => panic!("unexpected external ref: {other:?}"),
        };
        let cancel = CancelOrderRequest::new(
            venue("venue:BINANCE-SPOT"),
            account("account:binance-unit"),
            OrderReference::VenueOrderId(external_order_id),
            IdempotencyKey::new("idem:binance:spot:cancel:1").unwrap(),
        );
        adapter
            .cancel_order(cancel)
            .expect("known Binance order can be canceled with symbol context");

        assert_eq!(adapter.transport().calls.len(), 2);
        let cancel_call = &adapter.transport().calls[1];
        assert_eq!(cancel_call.method, live::BinanceExecHttpMethod::Delete);
        assert!(cancel_call.signed_query.contains("symbol=BTCUSDT"));
        assert!(cancel_call.signed_query.contains("orderId=12345"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_usdm_adapter_maps_post_only_to_gtx() {
        let mut adapter = live::BinanceUsdmExecAdapter::new(
            live::BinanceExecConfig::usdm_futures(
                venue("venue:BINANCE-USDM"),
                account("account:binance-usdm-unit"),
                "https://fapi.binance.com",
                binance_signing_policy("kms-policy/binance-usdm-exec-unit"),
            )
            .unwrap(),
            binance_test_signer(1_700_000_000_456),
            RecordingTransport::ok(
                200,
                r#"{"symbol":"BTCUSDT","orderId":991,"clientOrderId":"client:usdm:1","status":"NEW"}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(SubmitOrderRequest::new(
                venue("venue:BINANCE-USDM"),
                account("account:binance-usdm-unit"),
                instrument("inst:BINANCE:BTCUSDT:USDM-PERP"),
                OrderSide::Sell,
                MutableOrderType::PostOnly,
                quantity("0.002").unwrap(),
                Some(price("43200.00").unwrap()),
                Some(OrderId::new("client:usdm:1").unwrap()),
                IdempotencyKey::new("idem:binance:usdm:1").unwrap(),
            ))
            .expect("signed usdm order dispatches");

        let call = &adapter.transport().calls[0];
        assert_eq!(call.market, live::BinanceExecMarket::UsdmFutures);
        assert_eq!(call.endpoint, live::BINANCE_USDM_ORDER_ENDPOINT);
        assert!(call.signed_query.contains("symbol=BTCUSDT"));
        assert!(call.signed_query.contains("side=SELL"));
        assert!(call.signed_query.contains("type=LIMIT"));
        assert!(call.signed_query.contains("timeInForce=GTX"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_usdm_adapter_maps_explicit_position_side() {
        let mut adapter = live::BinanceUsdmExecAdapter::new(
            live::BinanceExecConfig::usdm_futures(
                venue("venue:BINANCE-USDM"),
                account("account:binance-usdm-unit"),
                "https://fapi.binance.com",
                binance_signing_policy("kms-policy/binance-usdm-exec-unit"),
            )
            .unwrap(),
            binance_test_signer(1_700_000_000_457),
            RecordingTransport::ok(
                200,
                r#"{"symbol":"BTCUSDT","orderId":991,"clientOrderId":"client:usdm:2","status":"NEW"}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:BINANCE-USDM"),
                    account("account:binance-usdm-unit"),
                    instrument("inst:BINANCE:BTCUSDT:USDM-PERP"),
                    OrderSide::Sell,
                    MutableOrderType::Limit,
                    quantity("0.002").unwrap(),
                    Some(price("43200.00").unwrap()),
                    Some(OrderId::new("client:usdm:2").unwrap()),
                    IdempotencyKey::new("idem:binance:usdm:2").unwrap(),
                )
                .with_position_side(PerpPositionSide::Short),
            )
            .expect("signed usdm order dispatches with position side");

        let call = &adapter.transport().calls[0];
        assert!(call.signed_query.contains("positionSide=SHORT"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_usdm_adapter_maps_ioc_reduce_only_limit() {
        let mut adapter = live::BinanceUsdmExecAdapter::new(
            live::BinanceExecConfig::usdm_futures(
                venue("venue:BINANCE-USDM"),
                account("account:binance-usdm-unit"),
                "https://fapi.binance.com",
                binance_signing_policy("kms-policy/binance-usdm-exec-unit"),
            )
            .unwrap(),
            binance_test_signer(1_700_000_000_789),
            RecordingTransport::ok(
                200,
                r#"{"symbol":"BTCUSDT","orderId":992,"clientOrderId":"client:usdm:ioc","status":"NEW"}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:BINANCE-USDM"),
                    account("account:binance-usdm-unit"),
                    instrument("inst:BINANCE:BTCUSDT:USDM-PERP"),
                    OrderSide::Buy,
                    MutableOrderType::Limit,
                    quantity("0.002").unwrap(),
                    Some(price("43200.00").unwrap()),
                    Some(OrderId::new("client:usdm:ioc").unwrap()),
                    IdempotencyKey::new("idem:binance:usdm:ioc").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc)
                .with_reduce_only(true),
            )
            .expect("signed reduce-only IOC usdm order dispatches");

        let call = &adapter.transport().calls[0];
        assert!(call.signed_query.contains("type=LIMIT"));
        assert!(call.signed_query.contains("timeInForce=IOC"));
        assert!(call.signed_query.contains("reduceOnly=true"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_usdm_adapter_applies_symbol_quantity_step() {
        let mut adapter = live::BinanceUsdmExecAdapter::new(
            live::BinanceExecConfig::usdm_futures(
                venue("venue:BINANCE-USDM"),
                account("account:binance-usdm-unit"),
                "https://fapi.binance.com",
                binance_signing_policy("kms-policy/binance-usdm-step-unit"),
            )
            .unwrap()
            .with_quantity_step("USARUSDT", quantity("0.1").unwrap())
            .unwrap(),
            binance_test_signer(1_700_000_000_790),
            RecordingTransport::ok(
                200,
                r#"{"symbol":"USARUSDT","orderId":993,"clientOrderId":"client:usdm:usar","status":"NEW"}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:BINANCE-USDM"),
                    account("account:binance-usdm-unit"),
                    instrument("inst:BINANCE:USARUSDT:USDM-PERP"),
                    OrderSide::Sell,
                    MutableOrderType::Limit,
                    quantity("4.786").unwrap(),
                    Some(price("20.86").unwrap()),
                    Some(OrderId::new("client:usdm:usar").unwrap()),
                    IdempotencyKey::new("idem:binance:usdm:usar").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc),
            )
            .expect("signed USD-M order dispatches with quantity step");

        let call = &adapter.transport().calls[0];
        assert!(call.signed_query.contains("symbol=USARUSDT"));
        assert!(call.signed_query.contains("quantity=4.7"));
        assert!(!call.signed_query.contains("quantity=4.786"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_order_query_uses_signed_get_confirmation_path() {
        let mut adapter = live::BinanceSpotExecAdapter::new(
            live::BinanceExecConfig::spot(
                venue("venue:BINANCE-SPOT"),
                account("account:binance-unit"),
                "https://api.binance.com",
                binance_signing_policy("kms-policy/binance-spot-query-unit"),
            )
            .unwrap(),
            binance_test_signer(1_700_000_000_123),
            RecordingTransport::ok(
                200,
                r#"{"symbol":"BTCUSDT","orderId":12345,"clientOrderId":"client:spot:1","status":"FILLED","executedQty":"0.001","avgPrice":"43100.50","time":1700000000123}"#,
            ),
        )
        .unwrap();

        let update = adapter
            .confirm_order_status(ConfirmOrderStatusRequest::new(
                venue("venue:BINANCE-SPOT"),
                account("account:binance-unit"),
                instrument("inst:BINANCE:BTCUSDT:SPOT"),
                OrderReference::VenueOrderId(
                    ExternalOrderId::new("binance:spot:order:12345").unwrap(),
                ),
                "event:binance:spot:query:filled",
            ))
            .expect("signed query confirms order");

        assert_eq!(update.source, OrderConfirmationSource::OrderQuery);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(adapter.transport().calls.len(), 1);
        let call = &adapter.transport().calls[0];
        assert_eq!(call.method, live::BinanceExecHttpMethod::Get);
        assert!(call.signed_query.contains("symbol=BTCUSDT"));
        assert!(call.signed_query.contains("orderId=12345"));
        assert!(call.signed_query.contains("&signature="));
    }

    #[cfg(all(feature = "live-exec", unix))]
    #[test]
    fn aster_perp_adapter_signs_submits_cancels_and_queries_order() {
        let mut adapter = live::AsterPerpExecAdapter::new(
            live::AsterExecConfig::perp(
                venue("venue:ASTER-USDT-FUTURES"),
                account("account:aster-unit"),
                live::ASTER_FUTURES_V3_BASE_URL,
                Some("0x1111111111111111111111111111111111111111".to_owned()),
                "0x2222222222222222222222222222222222222222",
                aster_signing_policy("kms-policy/aster-perp-exec-unit"),
            )
            .unwrap(),
            aster_test_signer(1_748_310_859_508_867),
            RecordingAsterTransport::ok([
                r#"{"symbol":"BTCUSDT","orderId":22542179,"clientOrderId":"asterUnit1","side":"BUY","status":"NEW","executedQty":"0","updateTime":1700000000123}"#,
                r#"{"symbol":"BTCUSDT","orderId":22542179,"clientOrderId":"asterUnit1","side":"BUY","status":"CANCELED","executedQty":"0","updateTime":1700000001123}"#,
                r#"{"symbol":"BTCUSDT","orderId":22542179,"clientOrderId":"asterUnit1","side":"BUY","status":"CANCELED","executedQty":"0","updateTime":1700000002123}"#,
            ]),
        )
        .unwrap();

        let receipt = adapter
            .submit_order(SubmitOrderRequest::new(
                venue("venue:ASTER-USDT-FUTURES"),
                account("account:aster-unit"),
                instrument("inst:ASTER:BTCUSDT:USDT-FUTURES"),
                OrderSide::Buy,
                MutableOrderType::PostOnly,
                quantity("0.001").unwrap(),
                Some(price("43100.50").unwrap()),
                Some(OrderId::new("asterUnit1").unwrap()),
                IdempotencyKey::new("idem:aster:perp:submit:1").unwrap(),
            ))
            .expect("signed Aster order dispatches");

        assert_eq!(receipt.status, MutableActionStatus::Accepted);
        let submit_call = &adapter.transport().calls[0];
        assert_eq!(submit_call.method, live::AsterExecHttpMethod::Post);
        assert_eq!(submit_call.endpoint, live::ASTER_FUTURES_V3_ORDER_ENDPOINT);
        assert!(submit_call.signed_query.contains("symbol=BTCUSDT"));
        assert!(submit_call.signed_query.contains("side=BUY"));
        assert!(submit_call.signed_query.contains("type=LIMIT"));
        assert!(submit_call.signed_query.contains("timeInForce=GTX"));
        assert!(submit_call.signed_query.contains("quantity=0.001"));
        assert!(submit_call.signed_query.contains("price=43100.50"));
        assert!(submit_call
            .signed_query
            .contains("newClientOrderId=asterUnit1"));
        assert!(submit_call
            .signed_query
            .contains("signer=0x2222222222222222222222222222222222222222"));
        assert!(submit_call.signed_query.contains("&signature=0x"));
        assert!(!submit_call.debug.contains(&submit_call.signed_query));

        let order_ref = match receipt.external_ref.expect("Aster order ref") {
            ExternalActionRef::Order(order_id) => OrderReference::VenueOrderId(order_id),
            other => panic!("unexpected Aster ref: {other:?}"),
        };
        adapter
            .cancel_order(CancelOrderRequest::new(
                venue("venue:ASTER-USDT-FUTURES"),
                account("account:aster-unit"),
                order_ref.clone(),
                IdempotencyKey::new("idem:aster:perp:cancel:1").unwrap(),
            ))
            .expect("known Aster order can be canceled");
        let cancel_call = &adapter.transport().calls[1];
        assert_eq!(cancel_call.method, live::AsterExecHttpMethod::Delete);
        assert!(cancel_call.signed_query.contains("symbol=BTCUSDT"));
        assert!(cancel_call.signed_query.contains("orderId=22542179"));

        let update = adapter
            .confirm_order_status(ConfirmOrderStatusRequest::new(
                venue("venue:ASTER-USDT-FUTURES"),
                account("account:aster-unit"),
                instrument("inst:ASTER:BTCUSDT:USDT-FUTURES"),
                order_ref,
                "event:aster:perp:query:cancelled",
            ))
            .expect("signed Aster query confirms order");
        assert_eq!(update.status, OrderConfirmationStatus::Cancelled);
        assert_eq!(
            adapter.transport().calls[2].method,
            live::AsterExecHttpMethod::Get
        );
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn hyperliquid_perp_adapter_signs_exchange_actions_and_queries_order_status() {
        let mut adapter = live::HyperliquidPerpExecAdapter::new(
            live::HyperliquidExecConfig::perp(
                venue("venue:HYPERLIQUID-PERP"),
                account("account:hyperliquid-unit"),
                live::HYPERLIQUID_API_BASE_URL,
                "0x3333333333333333333333333333333333333333",
                "a",
                hyperliquid_signing_policy("kms-policy/hyperliquid-perp-exec-unit"),
            )
            .unwrap()
            .with_asset_id("BTCUSDT", 0)
            .unwrap(),
            StaticHyperliquidSigner,
            RecordingHyperliquidTransport::ok([
                r#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"resting":{"oid":77747314}}]}}}"#,
                r#"{"status":"ok","response":{"type":"cancel","data":{"statuses":["success"]}}}"#,
                r#"{"status":"order","order":{"order":{"coin":"BTC","side":"B","limitPx":"43100.50","sz":"0.0","oid":77747314,"timestamp":1700000001000,"reduceOnly":false,"orderType":"Limit","origSz":"0.001","tif":"Ioc","cloid":"0x1234567890abcdef1234567890abcdef"},"status":"filled","statusTimestamp":1700000001500}}"#,
            ]),
        )
        .unwrap();

        let receipt = adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:HYPERLIQUID-PERP"),
                    account("account:hyperliquid-unit"),
                    instrument("inst:HYPERLIQUID:BTCUSDT:PERP"),
                    OrderSide::Buy,
                    MutableOrderType::Limit,
                    quantity("0.001").unwrap(),
                    Some(price("43100.50").unwrap()),
                    Some(OrderId::new("0x1234567890abcdef1234567890abcdef").unwrap()),
                    IdempotencyKey::new("idem:hyperliquid:perp:submit:1").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc),
            )
            .expect("signed Hyperliquid order dispatches");

        let submit_call = &adapter.transport().calls[0];
        assert_eq!(submit_call.endpoint, live::HYPERLIQUID_EXCHANGE_ENDPOINT);
        assert!(submit_call.body.contains(r#""type":"order""#));
        assert!(submit_call.body.contains(r#""a":0"#));
        assert!(submit_call.body.contains(r#""b":true"#));
        assert!(submit_call.body.contains(r#""tif":"Ioc""#));
        assert!(submit_call
            .body
            .contains(r#""c":"0x1234567890abcdef1234567890abcdef""#));
        assert!(submit_call.body.contains(r#""signature":{"r":"0x"#));

        let order_ref = match receipt.external_ref.expect("Hyperliquid order ref") {
            ExternalActionRef::Order(order_id) => OrderReference::VenueOrderId(order_id),
            other => panic!("unexpected Hyperliquid ref: {other:?}"),
        };
        adapter
            .cancel_order(CancelOrderRequest::new(
                venue("venue:HYPERLIQUID-PERP"),
                account("account:hyperliquid-unit"),
                order_ref.clone(),
                IdempotencyKey::new("idem:hyperliquid:perp:cancel:1").unwrap(),
            ))
            .expect("known Hyperliquid order can be canceled");
        let cancel_call = &adapter.transport().calls[1];
        assert_eq!(cancel_call.endpoint, live::HYPERLIQUID_EXCHANGE_ENDPOINT);
        assert!(cancel_call.body.contains(r#""type":"cancel""#));
        assert!(cancel_call.body.contains(r#""o":77747314"#));

        let update = adapter
            .confirm_order_status(ConfirmOrderStatusRequest::new(
                venue("venue:HYPERLIQUID-PERP"),
                account("account:hyperliquid-unit"),
                instrument("inst:HYPERLIQUID:BTCUSDT:PERP"),
                order_ref,
                "event:hyperliquid:perp:query:filled",
            ))
            .expect("Hyperliquid orderStatus confirms order");
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        let query_call = &adapter.transport().calls[2];
        assert_eq!(query_call.endpoint, live::HYPERLIQUID_INFO_ENDPOINT);
        assert!(query_call.body.contains(r#""type":"orderStatus""#));
        assert!(query_call.body.contains(r#""oid":77747314"#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bybit_linear_adapter_signs_post_only_submit_without_final_confirmation() {
        let mut adapter = live::BybitLinearExecAdapter::new(
            live::BybitExecConfig::linear_perpetual(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                "https://api.bybit.com",
                bybit_signing_policy("kms-policy/bybit-linear-submit-unit"),
            )
            .unwrap(),
            bybit_test_signer(1_700_000_000_123),
            RecordingBybitTransport::ok(
                200,
                r#"{"retCode":0,"retMsg":"OK","result":{"orderId":"c6f055d9-7f21-4079-913d-e6523a9cfffa","orderLinkId":"bybitUnit1"},"retExtInfo":{},"time":1700000000124}"#,
            ),
        )
        .unwrap();

        let request = SubmitOrderRequest::new(
            venue("venue:BYBIT-LINEAR"),
            account("account:bybit-unit"),
            instrument("inst:BYBIT:BTCUSDT:LINEAR-PERP"),
            OrderSide::Sell,
            MutableOrderType::PostOnly,
            quantity("0.001").unwrap(),
            Some(price("43100.50").unwrap()),
            Some(OrderId::new("bybitUnit1").unwrap()),
            IdempotencyKey::new("idem:bybit:linear:submit:1").unwrap(),
        );

        let receipt = adapter
            .submit_order(request.clone())
            .expect("signed Bybit order dispatches");
        let duplicate = adapter
            .submit_order(request)
            .expect("duplicate Bybit order is idempotent");

        assert_eq!(receipt.status, MutableActionStatus::Accepted);
        assert!(!receipt.simulated);
        assert_eq!(duplicate.action_id, receipt.action_id);
        assert!(duplicate.duplicate);
        assert_eq!(adapter.transport().calls.len(), 1);
        assert_eq!(
            receipt.external_ref,
            Some(ExternalActionRef::Order(
                ExternalOrderId::new("bybit-linear:order:c6f055d9-7f21-4079-913d-e6523a9cfffa")
                    .unwrap()
            ))
        );

        let call = &adapter.transport().calls[0];
        assert_eq!(call.market, live::BybitExecMarket::LinearPerpetual);
        assert_eq!(call.method, live::BybitExecHttpMethod::Post);
        assert_eq!(call.endpoint, live::BYBIT_ORDER_CREATE_ENDPOINT);
        assert_eq!(call.api_key_header_name, "X-BAPI-API-KEY");
        assert_eq!(call.timestamp_millis, 1_700_000_000_123);
        assert_eq!(call.recv_window_ms, live::DEFAULT_BYBIT_RECV_WINDOW_MS);
        assert!(call.payload.contains(r#""category":"linear""#));
        assert!(call.payload.contains(r#""side":"Sell""#));
        assert!(call.payload.contains(r#""timeInForce":"PostOnly""#));
        assert!(call.payload.contains(r#""orderLinkId":"bybitUnit1""#));
        assert_eq!(call.signature.len(), 64);
        assert!(!call.debug.contains(&call.api_key_header_value));
        assert!(!call.debug.contains(&call.signature));
        assert!(!call.debug.contains(&call.payload));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bybit_linear_adapter_maps_ioc_reduce_only_limit() {
        let mut adapter = live::BybitLinearExecAdapter::new(
            live::BybitExecConfig::linear_perpetual(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                "https://api.bybit.com",
                bybit_signing_policy("kms-policy/bybit-linear-submit-unit"),
            )
            .unwrap(),
            bybit_test_signer(1_700_000_000_123),
            RecordingBybitTransport::ok(
                200,
                r#"{"retCode":0,"retMsg":"OK","result":{"orderId":"c6f055d9-7f21-4079-913d-e6523a9cfffb","orderLinkId":"bybitUnitIoc"},"retExtInfo":{},"time":1700000000124}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:BYBIT-LINEAR"),
                    account("account:bybit-unit"),
                    instrument("inst:BYBIT:BTCUSDT:LINEAR-PERP"),
                    OrderSide::Buy,
                    MutableOrderType::Limit,
                    quantity("0.001").unwrap(),
                    Some(price("43100.50").unwrap()),
                    Some(OrderId::new("bybitUnitIoc").unwrap()),
                    IdempotencyKey::new("idem:bybit:linear:ioc").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc)
                .with_reduce_only(true),
            )
            .expect("signed Bybit reduce-only IOC order dispatches");

        let call = &adapter.transport().calls[0];
        assert!(call.payload.contains(r#""orderType":"Limit""#));
        assert!(call.payload.contains(r#""timeInForce":"IOC""#));
        assert!(call.payload.contains(r#""reduceOnly":true"#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bybit_linear_adapter_applies_symbol_quantity_step() {
        let mut adapter = live::BybitLinearExecAdapter::new(
            live::BybitExecConfig::linear_perpetual(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                "https://api.bybit.com",
                bybit_signing_policy("kms-policy/bybit-linear-submit-unit"),
            )
            .unwrap()
            .with_quantity_step("CHIPUSDT", quantity("10").unwrap())
            .unwrap(),
            bybit_test_signer(1_700_000_000_123),
            RecordingBybitTransport::ok(
                200,
                r#"{"retCode":0,"retMsg":"OK","result":{"orderId":"c6f055d9-7f21-4079-913d-e6523a9cfffc","orderLinkId":"bybitChipIoc"},"retExtInfo":{},"time":1700000000124}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:BYBIT-LINEAR"),
                    account("account:bybit-unit"),
                    instrument("inst:BYBIT:CHIPUSDT:LINEAR-PERP"),
                    OrderSide::Buy,
                    MutableOrderType::Limit,
                    quantity("1872.6591").unwrap(),
                    Some(price("0.05340").unwrap()),
                    Some(OrderId::new("bybitChipIoc").unwrap()),
                    IdempotencyKey::new("idem:bybit:linear:chip:ioc").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc),
            )
            .expect("signed Bybit order dispatches with quantity step");

        let call = &adapter.transport().calls[0];
        assert!(call.payload.contains(r#""symbol":"CHIPUSDT""#));
        assert!(call.payload.contains(r#""qty":"1870""#));
        assert!(!call.payload.contains(r#""qty":"1872.6591""#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bybit_spot_market_order_sets_base_coin_market_unit() {
        let mut adapter = live::BybitSpotExecAdapter::new(
            live::BybitExecConfig::spot(
                venue("venue:BYBIT-SPOT"),
                account("account:bybit-unit"),
                "https://api.bybit.com",
                bybit_signing_policy("kms-policy/bybit-spot-submit-unit"),
            )
            .unwrap(),
            bybit_test_signer(1_700_000_000_123),
            RecordingBybitTransport::ok(
                200,
                r#"{"retCode":0,"retMsg":"OK","result":{"orderId":"1523347543495541248","orderLinkId":"bybitSpot1"},"retExtInfo":{},"time":1700000000124}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(SubmitOrderRequest::new(
                venue("venue:BYBIT-SPOT"),
                account("account:bybit-unit"),
                instrument("inst:BYBIT:BTCUSDT:SPOT"),
                OrderSide::Buy,
                MutableOrderType::Market,
                quantity("0.001").unwrap(),
                None,
                Some(OrderId::new("bybitSpot1").unwrap()),
                IdempotencyKey::new("idem:bybit:spot:market:1").unwrap(),
            ))
            .expect("spot market order dispatches");

        let call = &adapter.transport().calls[0];
        assert_eq!(call.market, live::BybitExecMarket::Spot);
        assert!(call.payload.contains(r#""category":"spot""#));
        assert!(call.payload.contains(r#""orderType":"Market""#));
        assert!(call.payload.contains(r#""marketUnit":"baseCoin""#));
        assert!(!call.payload.contains(r#""price":"#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bybit_cancel_uses_signed_post_after_known_submit() {
        let mut adapter = live::BybitLinearExecAdapter::new(
            live::BybitExecConfig::linear_perpetual(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                "https://api.bybit.com",
                bybit_signing_policy("kms-policy/bybit-linear-cancel-unit"),
            )
            .unwrap(),
            bybit_test_signer(1_700_000_000_123),
            RecordingBybitTransport::ok(
                200,
                r#"{"retCode":0,"retMsg":"OK","result":{"orderId":"c6f055d9-7f21-4079-913d-e6523a9cfffa","orderLinkId":"bybitUnit2"},"retExtInfo":{},"time":1700000000124}"#,
            ),
        )
        .unwrap();

        let submit = adapter
            .submit_order(SubmitOrderRequest::new(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                instrument("inst:BYBIT:BTCUSDT:LINEAR-PERP"),
                OrderSide::Buy,
                MutableOrderType::Limit,
                quantity("0.001").unwrap(),
                Some(price("43100.50").unwrap()),
                Some(OrderId::new("bybitUnit2").unwrap()),
                IdempotencyKey::new("idem:bybit:linear:submit:2").unwrap(),
            ))
            .expect("submit before cancel");
        adapter.transport_mut().body = r#"{"retCode":0,"retMsg":"OK","result":{"orderId":"c6f055d9-7f21-4079-913d-e6523a9cfffa","orderLinkId":"bybitUnit2"},"retExtInfo":{},"time":1700000001124}"#.to_owned();

        let order_ref = match submit.external_ref.expect("order ref") {
            ExternalActionRef::Order(order_id) => OrderReference::VenueOrderId(order_id),
            _ => panic!("submit should return order ref"),
        };
        let cancel = adapter
            .cancel_order(CancelOrderRequest::new(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                order_ref,
                IdempotencyKey::new("idem:bybit:linear:cancel:2").unwrap(),
            ))
            .expect("cancel dispatches");

        assert_eq!(cancel.kind, MutableActionKind::CancelOrder);
        assert_eq!(adapter.transport().calls.len(), 2);
        let call = &adapter.transport().calls[1];
        assert_eq!(call.method, live::BybitExecHttpMethod::Post);
        assert_eq!(call.endpoint, live::BYBIT_ORDER_CANCEL_ENDPOINT);
        assert!(call.payload.contains(r#""category":"linear""#));
        assert!(call
            .payload
            .contains(r#""orderId":"c6f055d9-7f21-4079-913d-e6523a9cfffa""#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bybit_order_query_uses_signed_get_confirmation_path() {
        let mut adapter = live::BybitLinearExecAdapter::new(
            live::BybitExecConfig::linear_perpetual(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                "https://api.bybit.com",
                bybit_signing_policy("kms-policy/bybit-linear-query-unit"),
            )
            .unwrap(),
            bybit_test_signer(1_700_000_000_123),
            RecordingBybitTransport::ok(
                200,
                r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[{"symbol":"BTCUSDT","orderId":"c6f055d9-7f21-4079-913d-e6523a9cfffa","orderLinkId":"bybitUnit3","side":"Buy","orderStatus":"Filled","cumExecQty":"0.001","avgPrice":"43100.50","cumExecFee":"0.01","feeCurrency":"USDT","updatedTime":"1700000001500"}]},"retExtInfo":{},"time":1700000002000}"#,
            ),
        )
        .unwrap();

        let update = adapter
            .confirm_order_status(ConfirmOrderStatusRequest::new(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                instrument("inst:BYBIT:BTCUSDT:LINEAR-PERP"),
                OrderReference::VenueOrderId(
                    ExternalOrderId::new("bybit-linear:order:c6f055d9-7f21-4079-913d-e6523a9cfffa")
                        .unwrap(),
                ),
                "event:bybit:linear:query:filled",
            ))
            .expect("signed Bybit query confirms order");

        assert_eq!(update.source, OrderConfirmationSource::OrderQuery);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.market, PrivateOrderMarket::BybitLinear);
        assert_eq!(adapter.transport().calls.len(), 1);
        let call = &adapter.transport().calls[0];
        assert_eq!(call.method, live::BybitExecHttpMethod::Get);
        assert_eq!(call.endpoint, live::BYBIT_ORDER_REALTIME_ENDPOINT);
        assert!(call.payload.contains("category=linear"));
        assert!(call.payload.contains("symbol=BTCUSDT"));
        assert!(call
            .payload
            .contains("orderId=c6f055d9-7f21-4079-913d-e6523a9cfffa"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bybit_order_query_compacts_long_source_event_for_signing_boundary() {
        let mut adapter = live::BybitLinearExecAdapter::new(
            live::BybitExecConfig::linear_perpetual(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                "https://api.bybit.com",
                bybit_signing_policy("kms-policy/bybit-linear-query-long-source-unit"),
            )
            .unwrap(),
            bybit_test_signer(1_700_000_000_123),
            RecordingBybitTransport::ok(
                200,
                r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[{"symbol":"CHIPUSDT","orderId":"2d8fc08c-0f64-4fa5-8340-2583892bcbd8","orderLinkId":"bybitChipQuery","side":"Buy","orderStatus":"Filled","cumExecQty":"10","avgPrice":"0.053","cumExecFee":"0.01","feeCurrency":"USDT","updatedTime":"1700000001500"}]},"retExtInfo":{},"time":1700000002000}"#,
            ),
        )
        .unwrap();
        let long_source_event_id = format!(
            "event:funding-arb-live-canary-order-query:first:{}:{}",
            "pleg:plan:risk:trans:cross-exchange-funding-arb-binance-bybit-chipusdt-observer:0001",
            "x".repeat(80)
        );
        assert!(long_source_event_id.len() > 160);

        let update = adapter
            .confirm_order_status(ConfirmOrderStatusRequest::new(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                instrument("inst:BYBIT:CHIPUSDT:LINEAR-PERP"),
                OrderReference::VenueOrderId(
                    ExternalOrderId::new("bybit-linear:order:2d8fc08c-0f64-4fa5-8340-2583892bcbd8")
                        .unwrap(),
                ),
                long_source_event_id.clone(),
            ))
            .expect("long source event ID must not overflow signing request ID");

        assert_eq!(update.source_event_id, long_source_event_id);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(adapter.transport().calls.len(), 1);
        let call = &adapter.transport().calls[0];
        assert_eq!(call.method, live::BybitExecHttpMethod::Get);
        assert!(call.payload.contains("symbol=CHIPUSDT"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bybit_order_query_medium_source_event_does_not_overflow_audit_ref() {
        let mut adapter = live::BybitLinearExecAdapter::new(
            live::BybitExecConfig::linear_perpetual(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                "https://api.bybit.com",
                bybit_signing_policy("kms-policy/bybit-linear-query-medium-source-unit"),
            )
            .unwrap(),
            bybit_test_signer(1_700_000_000_123),
            RecordingBybitTransport::ok(
                200,
                r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[{"symbol":"CHIPUSDT","orderId":"49049529-7e25-4c97-b21c-419f69986a39","orderLinkId":"bybitChipExit","side":"Sell","orderStatus":"Filled","cumExecQty":"1890","avgPrice":"0.0528","cumExecFee":"0.01","feeCurrency":"USDT","updatedTime":"1700000001500"}]},"retExtInfo":{},"time":1700000002000}"#,
            ),
        )
        .unwrap();
        let source_event_id = format!("event:funding-arb-exit-order-query:{}", "x".repeat(70));
        let full_request_id = format!("signing-request/bybit-exec/query-order/{source_event_id}");
        assert!(full_request_id.len() <= 160);
        assert!(format!("signing-audit/{full_request_id}/pending").len() > 160);

        let update = adapter
            .confirm_order_status(ConfirmOrderStatusRequest::new(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                instrument("inst:BYBIT:CHIPUSDT:LINEAR-PERP"),
                OrderReference::VenueOrderId(
                    ExternalOrderId::new("bybit-linear:order:49049529-7e25-4c97-b21c-419f69986a39")
                        .unwrap(),
                ),
                source_event_id.clone(),
            ))
            .expect("medium source event ID must not overflow signing audit ref");

        assert_eq!(update.source_event_id, source_event_id);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn okx_spot_adapter_signs_submits_and_cancels_known_order() {
        let mut adapter = live::OkxSpotExecAdapter::new(
            live::OkxExecConfig::spot(
                venue("venue:OKX-SPOT"),
                account("account:okx-unit"),
                "https://www.okx.com",
                okx_signing_policy("kms-policy/okx-spot-submit-unit"),
            )
            .unwrap(),
            okx_test_signer("2026-05-17T12:34:56.789Z"),
            RecordingOkxTransport::ok(
                200,
                r#"{"code":"0","msg":"","data":[{"ordId":"512269865356374016","clOrdId":"rvoS1","sCode":"0","sMsg":""}]}"#,
            ),
        )
        .unwrap();

        let receipt = adapter
            .submit_order(SubmitOrderRequest::new(
                venue("venue:OKX-SPOT"),
                account("account:okx-unit"),
                instrument("inst:OKX:BTC-USDT:SPOT"),
                OrderSide::Buy,
                MutableOrderType::Limit,
                quantity("0.001").unwrap(),
                Some(price("43100.50").unwrap()),
                Some(OrderId::new("rvoS1").unwrap()),
                IdempotencyKey::new("idem:okx:spot:submit:1").unwrap(),
            ))
            .expect("signed OKX spot order dispatches");

        assert_eq!(receipt.status, MutableActionStatus::Accepted);
        assert!(!receipt.simulated);
        assert_eq!(
            receipt.external_ref,
            Some(ExternalActionRef::Order(
                ExternalOrderId::new("okx-spot:order:512269865356374016").unwrap()
            ))
        );
        assert_eq!(adapter.transport().calls.len(), 1);
        let call = &adapter.transport().calls[0];
        assert_eq!(call.market, live::OkxExecMarket::Spot);
        assert_eq!(call.method, arb_signing::real::OkxRestMethod::Post);
        assert_eq!(call.request_path, live::OKX_ORDER_ENDPOINT);
        assert_eq!(call.api_key_header_name, "OK-ACCESS-KEY");
        assert_eq!(call.signature_header_name, "OK-ACCESS-SIGN");
        assert_eq!(call.timestamp_header_value, "2026-05-17T12:34:56.789Z");
        assert_eq!(call.passphrase_header_name, "OK-ACCESS-PASSPHRASE");
        assert!(call.body.contains(r#""instId":"BTC-USDT""#));
        assert!(call.body.contains(r#""tdMode":"cash""#));
        assert!(call.body.contains(r#""side":"buy""#));
        assert!(call.body.contains(r#""ordType":"limit""#));
        assert!(call.body.contains(r#""clOrdId":"rvoS1""#));
        assert!(!call.signature_header_value.is_empty());
        assert!(!call.debug.contains(&call.api_key_header_value));
        assert!(!call.debug.contains(&call.passphrase_header_value));
        assert!(!call.debug.contains(&call.signature_header_value));
        assert!(!call.debug.contains(&call.body));

        adapter.transport_mut().body =
            r#"{"code":"0","msg":"","data":[{"ordId":"512269865356374016","clOrdId":"rvoS1","sCode":"0","sMsg":""}]}"#
                .to_owned();
        let order_ref = match receipt.external_ref.expect("order ref") {
            ExternalActionRef::Order(order_id) => OrderReference::VenueOrderId(order_id),
            _ => panic!("submit should return order ref"),
        };
        let cancel = adapter
            .cancel_order(CancelOrderRequest::new(
                venue("venue:OKX-SPOT"),
                account("account:okx-unit"),
                order_ref,
                IdempotencyKey::new("idem:okx:spot:cancel:1").unwrap(),
            ))
            .expect("OKX cancel dispatches");

        assert_eq!(cancel.kind, MutableActionKind::CancelOrder);
        assert_eq!(adapter.transport().calls.len(), 2);
        let cancel_call = &adapter.transport().calls[1];
        assert_eq!(cancel_call.method, arb_signing::real::OkxRestMethod::Post);
        assert_eq!(cancel_call.request_path, live::OKX_CANCEL_ORDER_ENDPOINT);
        assert!(cancel_call.body.contains(r#""instId":"BTC-USDT""#));
        assert!(cancel_call.body.contains(r#""ordId":"512269865356374016""#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn okx_swap_adapter_maps_ioc_reduce_only_limit() {
        let mut adapter = live::OkxSwapExecAdapter::new(
            live::OkxExecConfig::swap(
                venue("venue:OKX-SWAP"),
                account("account:okx-unit"),
                "https://www.okx.com",
                okx_signing_policy("kms-policy/okx-swap-submit-unit"),
            )
            .unwrap(),
            okx_test_signer("2026-05-17T12:34:56.789Z"),
            RecordingOkxTransport::ok(
                200,
                r#"{"code":"0","msg":"","data":[{"ordId":"612269865356374016","clOrdId":"rvoP2","sCode":"0","sMsg":""}]}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:OKX-SWAP"),
                    account("account:okx-unit"),
                    instrument("inst:OKX:BTC-USDT-SWAP:SWAP"),
                    OrderSide::Buy,
                    MutableOrderType::Limit,
                    quantity("0.001").unwrap(),
                    Some(price("43100.50").unwrap()),
                    Some(OrderId::new("rvoP2").unwrap()),
                    IdempotencyKey::new("idem:okx:swap:ioc").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc)
                .with_reduce_only(true),
            )
            .expect("signed OKX reduce-only IOC order dispatches");

        let call = &adapter.transport().calls[0];
        assert!(call.body.contains(r#""ordType":"ioc""#));
        assert!(call.body.contains(r#""reduceOnly":"true""#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn okx_swap_adapter_applies_inst_id_quantity_step() {
        let mut adapter = live::OkxSwapExecAdapter::new(
            live::OkxExecConfig::swap(
                venue("venue:OKX-SWAP"),
                account("account:okx-unit"),
                "https://www.okx.com",
                okx_signing_policy("kms-policy/okx-swap-step-unit"),
            )
            .unwrap()
            .with_quantity_step("USAR-USDT-SWAP", quantity("0.1").unwrap())
            .unwrap(),
            okx_test_signer("2026-05-17T12:34:56.789Z"),
            RecordingOkxTransport::ok(
                200,
                r#"{"code":"0","msg":"","data":[{"ordId":"612269865356374017","clOrdId":"rvoP4","sCode":"0","sMsg":""}]}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:OKX-SWAP"),
                    account("account:okx-unit"),
                    instrument("inst:OKX:USAR-USDT-SWAP:SWAP"),
                    OrderSide::Buy,
                    MutableOrderType::Limit,
                    quantity("4.786").unwrap(),
                    Some(price("20.89").unwrap()),
                    Some(OrderId::new("rvoP4").unwrap()),
                    IdempotencyKey::new("idem:okx:swap:usar").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc),
            )
            .expect("signed OKX swap order dispatches with quantity step");

        let call = &adapter.transport().calls[0];
        assert!(call.body.contains(r#""instId":"USAR-USDT-SWAP""#));
        assert!(call.body.contains(r#""sz":"4.7""#));
        assert!(!call.body.contains(r#""sz":"4.786""#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn okx_business_rejection_reports_nested_s_code() {
        let mut adapter = live::OkxSwapExecAdapter::new(
            live::OkxExecConfig::swap(
                venue("venue:OKX-SWAP"),
                account("account:okx-unit"),
                "https://www.okx.com",
                okx_signing_policy("kms-policy/okx-swap-reject-unit"),
            )
            .unwrap(),
            okx_test_signer("2026-05-17T12:34:56.789Z"),
            RecordingOkxTransport::ok(
                200,
                r#"{"code":"1","msg":"All operations failed","data":[{"ordId":"","clOrdId":"rvoP3","sCode":"51000","sMsg":"Parameter posSide error"}]}"#,
            ),
        )
        .unwrap();

        let error = adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:OKX-SWAP"),
                    account("account:okx-unit"),
                    instrument("inst:OKX:BTC-USDT-SWAP:SWAP"),
                    OrderSide::Buy,
                    MutableOrderType::Limit,
                    quantity("0.001").unwrap(),
                    Some(price("43100.50").unwrap()),
                    Some(OrderId::new("rvoP3").unwrap()),
                    IdempotencyKey::new("idem:okx:swap:reject").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc),
            )
            .expect_err("OKX business rejection must fail closed");
        let message = error.to_string();

        assert!(message.contains("OKX code=1: All operations failed"));
        assert!(message.contains("OKX sCode=51000: Parameter posSide error"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn okx_swap_order_query_uses_signed_get_confirmation_path() {
        let mut adapter = live::OkxSwapExecAdapter::new(
            live::OkxExecConfig::swap(
                venue("venue:OKX-SWAP"),
                account("account:okx-unit"),
                "https://www.okx.com",
                okx_signing_policy("kms-policy/okx-swap-query-unit"),
            )
            .unwrap(),
            okx_test_signer("2026-05-17T12:34:56.789Z"),
            RecordingOkxTransport::ok(
                200,
                r#"{"code":"0","msg":"","data":[{"instId":"BTC-USDT-SWAP","ordId":"312269865356374016","clOrdId":"rvoP1","side":"sell","state":"filled","accFillSz":"0.001","avgPx":"43100.50","uTime":"1700000001500"}]}"#,
            ),
        )
        .unwrap();

        let update = adapter
            .confirm_order_status(ConfirmOrderStatusRequest::new(
                venue("venue:OKX-SWAP"),
                account("account:okx-unit"),
                instrument("inst:OKX:BTC-USDT-SWAP:SWAP"),
                OrderReference::VenueOrderId(
                    ExternalOrderId::new("okx-swap:order:312269865356374016").unwrap(),
                ),
                "event:okx:swap:query:filled",
            ))
            .expect("signed OKX query confirms order");

        assert_eq!(update.source, OrderConfirmationSource::OrderQuery);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.market, PrivateOrderMarket::OkxSwap);
        assert_eq!(adapter.transport().calls.len(), 1);
        let call = &adapter.transport().calls[0];
        assert_eq!(call.method, arb_signing::real::OkxRestMethod::Get);
        assert_eq!(
            call.request_path,
            "/api/v5/trade/order?instId=BTC-USDT-SWAP&ordId=312269865356374016"
        );
        assert!(call.body.is_empty());
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bitget_usdt_futures_adapter_signs_submits_and_cancels_known_order() {
        let mut adapter = live::BitgetUsdtFuturesExecAdapter::new(
            live::BitgetExecConfig::usdt_futures(
                venue("venue:BITGET-USDT-FUTURES"),
                account("account:bitget-unit"),
                "https://api.bitget.com",
                bitget_signing_policy("kms-policy/bitget-usdt-futures-submit-unit"),
            )
            .unwrap()
            .with_trade_side("open")
            .unwrap(),
            bitget_test_signer(1_700_000_000_123),
            RecordingBitgetTransport::ok(
                200,
                r#"{"code":"00000","msg":"success","requestTime":1700000000124,"data":{"orderId":"1234567890","clientOid":"bitgetUnit1"}}"#,
            ),
        )
        .unwrap();

        let receipt = adapter
            .submit_order(SubmitOrderRequest::new(
                venue("venue:BITGET-USDT-FUTURES"),
                account("account:bitget-unit"),
                instrument("inst:BITGET:BTCUSDT:USDT-FUTURES"),
                OrderSide::Sell,
                MutableOrderType::PostOnly,
                quantity("0.001").unwrap(),
                Some(price("43100.50").unwrap()),
                Some(OrderId::new("bitgetUnit1").unwrap()),
                IdempotencyKey::new("idem:bitget:usdt-futures:submit:1").unwrap(),
            ))
            .expect("signed Bitget futures order dispatches");

        assert_eq!(receipt.status, MutableActionStatus::Accepted);
        assert!(!receipt.simulated);
        assert_eq!(
            receipt.external_ref,
            Some(ExternalActionRef::Order(
                ExternalOrderId::new("bitget-usdt-futures:order:1234567890").unwrap()
            ))
        );
        assert_eq!(adapter.transport().calls.len(), 1);
        let call = &adapter.transport().calls[0];
        assert_eq!(call.market, live::BitgetExecMarket::UsdtFutures);
        assert_eq!(call.method, arb_signing::real::BitgetRestMethod::Post);
        assert_eq!(call.request_path, live::BITGET_MIX_PLACE_ORDER_ENDPOINT);
        assert_eq!(call.api_key_header_name, "ACCESS-KEY");
        assert_eq!(call.signature_header_name, "ACCESS-SIGN");
        assert_eq!(call.timestamp_header_value, "1700000000123");
        assert_eq!(call.passphrase_header_name, "ACCESS-PASSPHRASE");
        assert!(call.body.contains(r#""symbol":"BTCUSDT""#));
        assert!(call.body.contains(r#""productType":"USDT-FUTURES""#));
        assert!(call.body.contains(r#""marginMode":"crossed""#));
        assert!(call.body.contains(r#""marginCoin":"USDT""#));
        assert!(call.body.contains(r#""side":"sell""#));
        assert!(call.body.contains(r#""tradeSide":"open""#));
        assert!(call.body.contains(r#""orderType":"limit""#));
        assert!(call.body.contains(r#""force":"post_only""#));
        assert!(call.body.contains(r#""clientOid":"bitgetUnit1""#));
        assert!(!call.signature_header_value.is_empty());
        assert!(!call.debug.contains(&call.api_key_header_value));
        assert!(!call.debug.contains(&call.passphrase_header_value));
        assert!(!call.debug.contains(&call.signature_header_value));
        assert!(!call.debug.contains(&call.body));

        adapter.transport_mut().body =
            r#"{"code":"00000","msg":"success","requestTime":1700000001124,"data":{"orderId":"1234567890","clientOid":"bitgetUnit1"}}"#
                .to_owned();
        let order_ref = match receipt.external_ref.expect("order ref") {
            ExternalActionRef::Order(order_id) => OrderReference::VenueOrderId(order_id),
            _ => panic!("submit should return order ref"),
        };
        let cancel = adapter
            .cancel_order(CancelOrderRequest::new(
                venue("venue:BITGET-USDT-FUTURES"),
                account("account:bitget-unit"),
                order_ref,
                IdempotencyKey::new("idem:bitget:usdt-futures:cancel:1").unwrap(),
            ))
            .expect("Bitget cancel dispatches");

        assert_eq!(cancel.kind, MutableActionKind::CancelOrder);
        assert_eq!(adapter.transport().calls.len(), 2);
        let cancel_call = &adapter.transport().calls[1];
        assert_eq!(
            cancel_call.method,
            arb_signing::real::BitgetRestMethod::Post
        );
        assert_eq!(
            cancel_call.request_path,
            live::BITGET_MIX_CANCEL_ORDER_ENDPOINT
        );
        assert!(cancel_call.body.contains(r#""symbol":"BTCUSDT""#));
        assert!(cancel_call.body.contains(r#""productType":"USDT-FUTURES""#));
        assert!(cancel_call.body.contains(r#""marginCoin":"USDT""#));
        assert!(cancel_call.body.contains(r#""orderId":"1234567890""#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bitget_usdt_futures_adapter_maps_ioc_reduce_only_limit() {
        let mut adapter = live::BitgetUsdtFuturesExecAdapter::new(
            live::BitgetExecConfig::usdt_futures(
                venue("venue:BITGET-USDT-FUTURES"),
                account("account:bitget-unit"),
                "https://api.bitget.com",
                bitget_signing_policy("kms-policy/bitget-usdt-futures-submit-unit"),
            )
            .unwrap()
            .with_trade_side("close")
            .unwrap(),
            bitget_test_signer(1_700_000_000_123),
            RecordingBitgetTransport::ok(
                200,
                r#"{"code":"00000","msg":"success","requestTime":1700000000124,"data":{"orderId":"1234567891","clientOid":"bitgetIoc1"}}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:BITGET-USDT-FUTURES"),
                    account("account:bitget-unit"),
                    instrument("inst:BITGET:BTCUSDT:USDT-FUTURES"),
                    OrderSide::Buy,
                    MutableOrderType::Limit,
                    quantity("0.001").unwrap(),
                    Some(price("43100.50").unwrap()),
                    Some(OrderId::new("bitgetIoc1").unwrap()),
                    IdempotencyKey::new("idem:bitget:usdt-futures:ioc").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc)
                .with_reduce_only(true),
            )
            .expect("signed Bitget reduce-only IOC order dispatches");

        let call = &adapter.transport().calls[0];
        assert!(call.body.contains(r#""tradeSide":"close""#));
        assert!(call.body.contains(r#""force":"ioc""#));
        assert!(call.body.contains(r#""reduceOnly":"YES""#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bitget_usdt_futures_adapter_applies_symbol_quantity_step_without_trade_side() {
        let mut adapter = live::BitgetUsdtFuturesExecAdapter::new(
            live::BitgetExecConfig::usdt_futures(
                venue("venue:BITGET-USDT-FUTURES"),
                account("account:bitget-unit"),
                "https://api.bitget.com",
                bitget_signing_policy("kms-policy/bitget-usdt-futures-submit-unit"),
            )
            .unwrap()
            .with_quantity_step("CHIPUSDT", quantity("1").unwrap())
            .unwrap(),
            bitget_test_signer(1_700_000_000_123),
            RecordingBitgetTransport::ok(
                200,
                r#"{"code":"00000","msg":"success","requestTime":1700000000124,"data":{"orderId":"1234567892","clientOid":"bitgetChipIoc"}}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:BITGET-USDT-FUTURES"),
                    account("account:bitget-unit"),
                    instrument("inst:BITGET:CHIPUSDT:USDT-FUTURES"),
                    OrderSide::Buy,
                    MutableOrderType::Limit,
                    quantity("1869.8588").unwrap(),
                    Some(price("0.05348").unwrap()),
                    Some(OrderId::new("bitgetChipIoc").unwrap()),
                    IdempotencyKey::new("idem:bitget:usdt-futures:chip:ioc").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc),
            )
            .expect("signed Bitget order dispatches with quantity step");

        let call = &adapter.transport().calls[0];
        assert!(call.body.contains(r#""symbol":"CHIPUSDT""#));
        assert!(call.body.contains(r#""size":"1869""#));
        assert!(!call.body.contains(r#""size":"1869.8588""#));
        assert!(!call.body.contains(r#""tradeSide":"#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bitget_spot_market_order_uses_real_submit_body_without_price() {
        let mut adapter = live::BitgetSpotExecAdapter::new(
            live::BitgetExecConfig::spot(
                venue("venue:BITGET-SPOT"),
                account("account:bitget-unit"),
                "https://api.bitget.com",
                bitget_signing_policy("kms-policy/bitget-spot-submit-unit"),
            )
            .unwrap(),
            bitget_test_signer(1_700_000_000_123),
            RecordingBitgetTransport::ok(
                200,
                r#"{"code":"00000","msg":"success","requestTime":1700000000124,"data":{"orderId":"22334455","clientOid":"bitgetSpot1"}}"#,
            ),
        )
        .unwrap();

        adapter
            .submit_order(SubmitOrderRequest::new(
                venue("venue:BITGET-SPOT"),
                account("account:bitget-unit"),
                instrument("inst:BITGET:BTCUSDT:SPOT"),
                OrderSide::Buy,
                MutableOrderType::Market,
                quantity("0.001").unwrap(),
                None,
                Some(OrderId::new("bitgetSpot1").unwrap()),
                IdempotencyKey::new("idem:bitget:spot:market:1").unwrap(),
            ))
            .expect("Bitget spot market order dispatches");

        let call = &adapter.transport().calls[0];
        assert_eq!(call.market, live::BitgetExecMarket::Spot);
        assert_eq!(call.request_path, live::BITGET_SPOT_PLACE_ORDER_ENDPOINT);
        assert!(call.body.contains(r#""symbol":"BTCUSDT""#));
        assert!(call.body.contains(r#""side":"buy""#));
        assert!(call.body.contains(r#""orderType":"market""#));
        assert!(call.body.contains(r#""size":"0.001""#));
        assert!(!call.body.contains(r#""price":"#));
        assert!(!call.body.contains(r#""productType":"#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn bitget_order_query_uses_signed_get_confirmation_path() {
        let mut adapter = live::BitgetUsdtFuturesExecAdapter::new(
            live::BitgetExecConfig::usdt_futures(
                venue("venue:BITGET-USDT-FUTURES"),
                account("account:bitget-unit"),
                "https://api.bitget.com",
                bitget_signing_policy("kms-policy/bitget-usdt-futures-query-unit"),
            )
            .unwrap(),
            bitget_test_signer(1_700_000_000_123),
            RecordingBitgetTransport::ok(
                200,
                r#"{"code":"00000","msg":"success","requestTime":1700000002000,"data":{"symbol":"BTCUSDT","orderId":"1234567890","clientOid":"bitgetUnit1","side":"sell","status":"filled","priceAvg":"43100.50","baseVolume":"0.001","uTime":"1700000001500"}}"#,
            ),
        )
        .unwrap();

        let update = adapter
            .confirm_order_status(ConfirmOrderStatusRequest::new(
                venue("venue:BITGET-USDT-FUTURES"),
                account("account:bitget-unit"),
                instrument("inst:BITGET:BTCUSDT:USDT-FUTURES"),
                OrderReference::VenueOrderId(
                    ExternalOrderId::new("bitget-usdt-futures:order:1234567890").unwrap(),
                ),
                "event:bitget:usdt-futures:query:filled",
            ))
            .expect("signed Bitget query confirms order");

        assert_eq!(update.source, OrderConfirmationSource::OrderQuery);
        assert_eq!(update.status, OrderConfirmationStatus::Filled);
        assert_eq!(update.market, PrivateOrderMarket::BitgetUsdtFutures);
        assert_eq!(adapter.transport().calls.len(), 1);
        let call = &adapter.transport().calls[0];
        assert_eq!(call.method, arb_signing::real::BitgetRestMethod::Get);
        assert_eq!(
            call.request_path,
            "/api/v2/mix/order/detail?symbol=BTCUSDT&productType=USDT-FUTURES&orderId=1234567890"
        );
        assert!(call.body.is_empty());
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn cex_perp_adapters_set_target_leverage_before_open_order() {
        let mut binance = live::BinanceUsdmExecAdapter::new(
            live::BinanceExecConfig::usdm_futures(
                venue("venue:BINANCE-USDM"),
                account("account:binance-usdm-unit"),
                "https://fapi.binance.com",
                binance_signing_policy("kms-policy/binance-usdm-leverage-unit"),
            )
            .unwrap()
            .with_target_leverage(1)
            .unwrap(),
            binance_test_signer(1_700_000_000_123),
            RecordingTransport::ok(
                200,
                r#"{"symbol":"BTCUSDT","orderId":991,"clientOrderId":"clientUsdmLev1","status":"NEW"}"#,
            ),
        )
        .unwrap();
        binance
            .submit_order(SubmitOrderRequest::new(
                venue("venue:BINANCE-USDM"),
                account("account:binance-usdm-unit"),
                instrument("inst:BINANCE:BTCUSDT:USDM-PERP"),
                OrderSide::Sell,
                MutableOrderType::Limit,
                quantity("0.001").unwrap(),
                Some(price("43100.50").unwrap()),
                Some(OrderId::new("clientUsdmLev1").unwrap()),
                IdempotencyKey::new("idem:binance:usdm:leverage:1").unwrap(),
            ))
            .expect("Binance leverage then order dispatches");
        assert_eq!(binance.transport().calls.len(), 2);
        assert_eq!(
            binance.transport().calls[0].endpoint,
            live::BINANCE_USDM_LEVERAGE_ENDPOINT
        );
        assert!(binance.transport().calls[0]
            .signed_query
            .contains("leverage=1"));
        assert_eq!(
            binance.transport().calls[1].endpoint,
            live::BINANCE_USDM_ORDER_ENDPOINT
        );

        let mut bybit = live::BybitLinearExecAdapter::new(
            live::BybitExecConfig::linear_perpetual(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                "https://api.bybit.com",
                bybit_signing_policy("kms-policy/bybit-linear-leverage-unit"),
            )
            .unwrap()
            .with_target_leverage(1)
            .unwrap(),
            bybit_test_signer(1_700_000_000_123),
            RecordingBybitTransport::ok(
                200,
                r#"{"retCode":0,"retMsg":"OK","result":{"orderId":"c6f055d9-7f21-4079-913d-e6523a9cfffa","orderLinkId":"bybitLev1"},"retExtInfo":{},"time":1700000000124}"#,
            ),
        )
        .unwrap();
        bybit
            .submit_order(SubmitOrderRequest::new(
                venue("venue:BYBIT-LINEAR"),
                account("account:bybit-unit"),
                instrument("inst:BYBIT:BTCUSDT:LINEAR-PERP"),
                OrderSide::Sell,
                MutableOrderType::PostOnly,
                quantity("0.001").unwrap(),
                Some(price("43100.50").unwrap()),
                Some(OrderId::new("bybitLev1").unwrap()),
                IdempotencyKey::new("idem:bybit:linear:leverage:1").unwrap(),
            ))
            .expect("Bybit leverage then order dispatches");
        assert_eq!(bybit.transport().calls.len(), 2);
        assert_eq!(
            bybit.transport().calls[0].endpoint,
            live::BYBIT_SET_LEVERAGE_ENDPOINT
        );
        assert!(bybit.transport().calls[0]
            .payload
            .contains(r#""buyLeverage":"1""#));
        assert!(bybit.transport().calls[0]
            .payload
            .contains(r#""sellLeverage":"1""#));
        assert_eq!(
            bybit.transport().calls[1].endpoint,
            live::BYBIT_ORDER_CREATE_ENDPOINT
        );

        let mut okx = live::OkxSwapExecAdapter::new(
            live::OkxExecConfig::swap(
                venue("venue:OKX-SWAP"),
                account("account:okx-unit"),
                "https://www.okx.com",
                okx_signing_policy("kms-policy/okx-swap-leverage-unit"),
            )
            .unwrap()
            .with_target_leverage(1)
            .unwrap(),
            okx_test_signer("2026-05-17T12:34:56.789Z"),
            RecordingOkxTransport::ok(
                200,
                r#"{"code":"0","msg":"","data":[{"ordId":"612269865356374016","clOrdId":"okxLev1","sCode":"0","sMsg":""}]}"#,
            ),
        )
        .unwrap();
        okx.submit_order(SubmitOrderRequest::new(
            venue("venue:OKX-SWAP"),
            account("account:okx-unit"),
            instrument("inst:OKX:BTC-USDT-SWAP:SWAP"),
            OrderSide::Buy,
            MutableOrderType::Limit,
            quantity("0.001").unwrap(),
            Some(price("43100.50").unwrap()),
            Some(OrderId::new("okxLev1").unwrap()),
            IdempotencyKey::new("idem:okx:swap:leverage:1").unwrap(),
        ))
        .expect("OKX leverage then order dispatches");
        assert_eq!(okx.transport().calls.len(), 2);
        assert_eq!(
            okx.transport().calls[0].request_path,
            live::OKX_SET_LEVERAGE_ENDPOINT
        );
        assert!(okx.transport().calls[0]
            .body
            .contains(r#""instId":"BTC-USDT-SWAP""#));
        assert!(okx.transport().calls[0].body.contains(r#""lever":"1""#));
        assert_eq!(
            okx.transport().calls[1].request_path,
            live::OKX_ORDER_ENDPOINT
        );

        let mut bitget = live::BitgetUsdtFuturesExecAdapter::new(
            live::BitgetExecConfig::usdt_futures(
                venue("venue:BITGET-USDT-FUTURES"),
                account("account:bitget-unit"),
                "https://api.bitget.com",
                bitget_signing_policy("kms-policy/bitget-usdt-futures-leverage-unit"),
            )
            .unwrap()
            .with_target_leverage(1)
            .unwrap(),
            bitget_test_signer(1_700_000_000_123),
            RecordingBitgetTransport::ok(
                200,
                r#"{"code":"00000","msg":"success","requestTime":1700000000124,"data":{"orderId":"1234567890","clientOid":"bitgetLev1"}}"#,
            ),
        )
        .unwrap();
        bitget
            .submit_order(SubmitOrderRequest::new(
                venue("venue:BITGET-USDT-FUTURES"),
                account("account:bitget-unit"),
                instrument("inst:BITGET:BTCUSDT:USDT-FUTURES"),
                OrderSide::Sell,
                MutableOrderType::Limit,
                quantity("0.001").unwrap(),
                Some(price("43100.50").unwrap()),
                Some(OrderId::new("bitgetLev1").unwrap()),
                IdempotencyKey::new("idem:bitget:usdt-futures:leverage:1").unwrap(),
            ))
            .expect("Bitget leverage then order dispatches");
        assert_eq!(bitget.transport().calls.len(), 2);
        assert_eq!(
            bitget.transport().calls[0].request_path,
            live::BITGET_MIX_SET_LEVERAGE_ENDPOINT
        );
        assert!(bitget.transport().calls[0]
            .body
            .contains(r#""leverage":"1""#));
        assert_eq!(
            bitget.transport().calls[1].request_path,
            live::BITGET_MIX_PLACE_ORDER_ENDPOINT
        );
    }

    #[cfg(all(feature = "live-exec", unix))]
    #[test]
    fn aster_perp_adapter_sets_target_leverage_before_open_order() {
        let mut adapter = live::AsterPerpExecAdapter::new(
            live::AsterExecConfig::perp(
                venue("venue:ASTER-USDT-FUTURES"),
                account("account:aster-unit"),
                live::ASTER_FUTURES_V3_BASE_URL,
                Some("0x1111111111111111111111111111111111111111".to_owned()),
                "0x2222222222222222222222222222222222222222",
                aster_signing_policy("kms-policy/aster-perp-leverage-unit"),
            )
            .unwrap()
            .with_target_leverage(1)
            .unwrap(),
            aster_test_signer(1_748_310_859_508_867),
            RecordingAsterTransport::ok([
                r#"{"symbol":"BTCUSDT","leverage":1,"maxNotionalValue":"1000000"}"#,
                r#"{"symbol":"BTCUSDT","orderId":22542179,"clientOrderId":"asterLev1","side":"BUY","status":"NEW","executedQty":"0","updateTime":1700000000123}"#,
            ]),
        )
        .unwrap();

        adapter
            .submit_order(SubmitOrderRequest::new(
                venue("venue:ASTER-USDT-FUTURES"),
                account("account:aster-unit"),
                instrument("inst:ASTER:BTCUSDT:USDT-FUTURES"),
                OrderSide::Buy,
                MutableOrderType::PostOnly,
                quantity("0.001").unwrap(),
                Some(price("43100.50").unwrap()),
                Some(OrderId::new("asterLev1").unwrap()),
                IdempotencyKey::new("idem:aster:perp:leverage:1").unwrap(),
            ))
            .expect("Aster leverage then order dispatches");
        assert_eq!(adapter.transport().calls.len(), 2);
        assert_eq!(
            adapter.transport().calls[0].endpoint,
            live::ASTER_FUTURES_V3_LEVERAGE_ENDPOINT
        );
        assert!(adapter.transport().calls[0]
            .signed_query
            .contains("leverage=1"));
        assert_eq!(
            adapter.transport().calls[1].endpoint,
            live::ASTER_FUTURES_V3_ORDER_ENDPOINT
        );
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn hyperliquid_perp_adapter_sets_target_leverage_before_open_order() {
        let mut adapter = live::HyperliquidPerpExecAdapter::new(
            live::HyperliquidExecConfig::perp(
                venue("venue:HYPERLIQUID-PERP"),
                account("account:hyperliquid-unit"),
                live::HYPERLIQUID_API_BASE_URL,
                "0x3333333333333333333333333333333333333333",
                "a",
                hyperliquid_signing_policy("kms-policy/hyperliquid-perp-leverage-unit"),
            )
            .unwrap()
            .with_target_leverage(1)
            .unwrap()
            .with_asset_id("BTCUSDT", 0)
            .unwrap(),
            StaticHyperliquidSigner,
            RecordingHyperliquidTransport::ok([
                r#"{"status":"ok","response":{"type":"default"}}"#,
                r#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"resting":{"oid":77747314}}]}}}"#,
            ]),
        )
        .unwrap();

        adapter
            .submit_order(
                SubmitOrderRequest::new(
                    venue("venue:HYPERLIQUID-PERP"),
                    account("account:hyperliquid-unit"),
                    instrument("inst:HYPERLIQUID:BTCUSDT:PERP"),
                    OrderSide::Buy,
                    MutableOrderType::Limit,
                    quantity("0.001").unwrap(),
                    Some(price("43100.50").unwrap()),
                    Some(OrderId::new("0x1234567890abcdef1234567890abcdef").unwrap()),
                    IdempotencyKey::new("idem:hyperliquid:perp:leverage:1").unwrap(),
                )
                .with_time_in_force(MutableTimeInForce::Ioc),
            )
            .expect("Hyperliquid leverage then order dispatches");
        assert_eq!(adapter.transport().calls.len(), 2);
        assert!(adapter.transport().calls[0]
            .body
            .contains(r#""type":"updateLeverage""#));
        assert!(adapter.transport().calls[0]
            .body
            .contains(r#""isCross":true"#));
        assert!(adapter.transport().calls[0]
            .body
            .contains(r#""leverage":1"#));
        assert!(adapter.transport().calls[1]
            .body
            .contains(r#""type":"order""#));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_adapter_rejects_wrong_market_instrument_before_transport() {
        let mut adapter = live::BinanceSpotExecAdapter::new(
            live::BinanceExecConfig::spot(
                venue("venue:BINANCE-SPOT"),
                account("account:binance-unit"),
                "https://api.binance.com",
                binance_signing_policy("kms-policy/binance-spot-wrong-market-unit"),
            )
            .unwrap(),
            binance_test_signer(1_700_000_000_123),
            RecordingTransport::ok(200, "{}"),
        )
        .unwrap();

        let error = adapter
            .submit_order(SubmitOrderRequest::new(
                venue("venue:BINANCE-SPOT"),
                account("account:binance-unit"),
                instrument("inst:BINANCE:BTCUSDT:USDM-PERP"),
                OrderSide::Buy,
                MutableOrderType::Market,
                quantity("0.001").unwrap(),
                None,
                Some(OrderId::new("client:spot:wrong").unwrap()),
                IdempotencyKey::new("idem:binance:spot:wrong").unwrap(),
            ))
            .expect_err("spot adapter must reject usdm instrument");

        assert!(matches!(
            error,
            VenueExecError::InvalidRequest {
                field: "instrument_id",
                ..
            }
        ));
        assert!(adapter.transport().calls.is_empty());
    }

    fn sample_order(idempotency_key: &str, qty: &str) -> DomainResult<SubmitOrderRequest> {
        Ok(SubmitOrderRequest::new(
            venue("sim-venue"),
            account("acct:main"),
            instrument("BTC-USDC-PERP"),
            OrderSide::Buy,
            MutableOrderType::Limit,
            quantity(qty)?,
            Some(price("50000.00")?),
            Some(OrderId::new(format!("client:{idempotency_key}"))?),
            IdempotencyKey::new(idempotency_key).unwrap(),
        ))
    }

    fn venue(value: &str) -> VenueId {
        VenueId::new(value).unwrap()
    }

    fn account(value: &str) -> AccountId {
        AccountId::new(value).unwrap()
    }

    fn instrument(value: &str) -> InstrumentId {
        InstrumentId::new(value).unwrap()
    }

    fn asset(value: &str) -> AssetId {
        AssetId::new(value).unwrap()
    }

    fn quantity(value: &str) -> DomainResult<Quantity> {
        Quantity::new(Decimal::from_str(value)?)
    }

    fn price(value: &str) -> DomainResult<Price> {
        Price::new(Decimal::from_str(value)?)
    }

    fn amount(value: &str) -> DomainResult<Amount> {
        Amount::new(Decimal::from_str(value)?)
    }

    fn dispatch_policy(cap: &str) -> ExecutionDispatchPolicy {
        ExecutionDispatchPolicy::new(amount(cap).expect("cap"))
    }

    fn dispatch_time(time: &str) -> UtcTimestamp {
        UtcTimestamp::parse_rfc3339_z(&format!("2026-01-01T{time}Z")).expect("time")
    }

    fn basis_execution_plan() -> ExecutionPlan {
        from_json_strict(
            r#"{
  "schema_version": "1.0.0",
  "plan_id": "plan:basis",
  "transition_id": "trans:basis",
  "risk_decision_id": "risk:basis",
  "created_at": "2026-01-01T00:00:00Z",
  "execution_mode": "ManualApproval",
  "idempotency_key": "idem:plan:basis",
  "legs": [
    {
      "plan_leg_id": "pleg:plan:basis:manual-gate",
      "candidate_leg_id": "manual-gate:risk:basis",
      "action_type": "ManualApprovalGate",
      "account_id": "acct:basis",
      "idempotency_key": "idem:plan:basis:manual",
      "state": "Ready",
      "failure_semantics": "ManualInterventionRequired"
    },
    {
      "plan_leg_id": "pleg:plan:basis:0001",
      "candidate_leg_id": "candleg:basis:spot",
      "action_type": "PlaceOrder",
      "venue_id": "venue:BINANCE-SPOT",
      "instrument_id": "inst:BINANCE:BTCUSDT:SPOT",
      "account_id": "acct:basis",
      "venue_symbol": "BTCUSDT",
      "side": "Buy",
      "order_type": "Limit",
      "time_in_force": "IOC",
      "quantity": "0.001",
      "limit_price": "50000.00",
      "notional_usd": "5.00",
      "basis_leg_role": "spot_buy",
      "idempotency_key": "idem:plan:basis:spot",
      "depends_on": ["pleg:plan:basis:manual-gate"],
      "state": "WaitingDependency",
      "failure_semantics": "UnknownState"
    },
    {
      "plan_leg_id": "pleg:plan:basis:0002",
      "candidate_leg_id": "candleg:basis:perp",
      "action_type": "PlaceOrder",
      "venue_id": "venue:BINANCE-USDM",
      "instrument_id": "inst:BINANCE:BTCUSDT:USDM-PERP",
      "account_id": "acct:basis",
      "venue_symbol": "BTCUSDT",
      "side": "Short",
      "order_type": "Limit",
      "time_in_force": "IOC",
      "quantity": "0.001",
      "limit_price": "50100.00",
      "notional_usd": "5.00",
      "basis_leg_role": "perp_short",
      "idempotency_key": "idem:plan:basis:perp",
      "depends_on": ["pleg:plan:basis:0001"],
      "state": "WaitingDependency",
      "failure_semantics": "UnknownState"
    }
  ],
  "dependency_graph": {
    "edges": [
      {
        "from_leg_id": "pleg:plan:basis:manual-gate",
        "to_leg_id": "pleg:plan:basis:0001",
        "condition": "ManualRelease"
      },
      {
        "from_leg_id": "pleg:plan:basis:0001",
        "to_leg_id": "pleg:plan:basis:0002",
        "condition": "OnSuccess"
      }
    ]
  },
  "constraints": {
    "max_notional_usd": "10.00",
    "slippage_limit_bps": "5"
  },
  "timeout_policy": {
    "plan_timeout_ms": 60000,
    "leg_timeout_ms": 10000,
    "unknown_state_after_ms": 15000
  },
  "cancel_policy": {"default_action": "CancelAndHedgeResidual"},
  "hedge_policy": {"residual_exposure_action": "HedgeAfterTimeout", "threshold_usd": "0"},
  "partial_fill_policy": {"action": "HedgeFilledPortion", "max_unhedged_usd": "0"},
  "failure_policy": {"unknown_state_action": "HaltAndIncident", "retry_limit": 0}
}"#,
        )
        .expect("basis execution plan")
    }

    fn funding_arb_execution_plan() -> ExecutionPlan {
        from_json_strict(
            r#"{
  "schema_version": "1.0.0",
  "plan_id": "plan:funding-arb",
  "transition_id": "trans:funding-arb",
  "risk_decision_id": "risk:funding-arb",
  "created_at": "2026-01-01T00:00:00Z",
  "execution_mode": "ManualApproval",
  "idempotency_key": "idem:plan:funding-arb",
  "legs": [
    {
      "plan_leg_id": "pleg:plan:funding-arb:manual-gate",
      "candidate_leg_id": "manual-gate:risk:funding-arb",
      "action_type": "ManualApprovalGate",
      "account_id": "acct:funding-arb",
      "idempotency_key": "idem:plan:funding-arb:manual",
      "state": "Ready",
      "failure_semantics": "ManualInterventionRequired"
    },
    {
      "plan_leg_id": "pleg:plan:funding-arb:0001",
      "candidate_leg_id": "candleg:funding-arb:long",
      "action_type": "PlaceOrder",
      "venue_id": "venue:BINANCE-USDM",
      "instrument_id": "inst:BINANCE:BTCUSDT:USDM-PERP",
      "account_id": "acct:binance-funding",
      "venue_symbol": "BTCUSDT",
      "side": "Long",
      "order_type": "Limit",
      "quantity": "1.0",
      "limit_price": "100.00",
      "notional_usd": "5.00",
      "basis_leg_role": "perp_long",
      "idempotency_key": "idem:plan:funding-arb:long",
      "depends_on": ["pleg:plan:funding-arb:manual-gate"],
      "state": "WaitingDependency",
      "failure_semantics": "UnknownState"
    },
    {
      "plan_leg_id": "pleg:plan:funding-arb:0002",
      "candidate_leg_id": "candleg:funding-arb:short",
      "action_type": "PlaceOrder",
      "venue_id": "venue:BYBIT-LINEAR",
      "instrument_id": "inst:BYBIT:BTCUSDT:LINEAR-PERP",
      "account_id": "acct:bybit-funding",
      "venue_symbol": "BTCUSDT",
      "side": "Short",
      "order_type": "Limit",
      "quantity": "1.0",
      "limit_price": "100.05",
      "notional_usd": "5.00",
      "basis_leg_role": "perp_short",
      "idempotency_key": "idem:plan:funding-arb:short",
      "depends_on": ["pleg:plan:funding-arb:0001"],
      "state": "WaitingDependency",
      "failure_semantics": "UnknownState"
    }
  ],
  "dependency_graph": {
    "edges": [
      {
        "from_leg_id": "pleg:plan:funding-arb:manual-gate",
        "to_leg_id": "pleg:plan:funding-arb:0001",
        "condition": "ManualRelease"
      },
      {
        "from_leg_id": "pleg:plan:funding-arb:0001",
        "to_leg_id": "pleg:plan:funding-arb:0002",
        "condition": "OnSuccess"
      }
    ]
  },
  "constraints": {
    "max_notional_usd": "10.00",
    "slippage_limit_bps": "5"
  },
  "timeout_policy": {
    "plan_timeout_ms": 60000,
    "leg_timeout_ms": 10000,
    "unknown_state_after_ms": 15000
  },
  "cancel_policy": {"default_action": "CancelAndHedgeResidual"},
  "hedge_policy": {"residual_exposure_action": "HedgeAfterTimeout", "threshold_usd": "0"},
  "partial_fill_policy": {"action": "HedgeFilledPortion", "max_unhedged_usd": "0"},
  "failure_policy": {"unknown_state_action": "HaltAndIncident", "retry_limit": 0}
}"#,
        )
        .expect("funding arb execution plan")
    }

    #[derive(Default)]
    struct FailSecondSubmitOrder {
        inner: SimulatedVenueExecAdapter,
        calls: usize,
    }

    impl SubmitOrder for FailSecondSubmitOrder {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.calls += 1;
            if self.calls == 2 {
                let venue_id = request.venue_id.clone();
                return Err(VenueExecError::UnknownExternalState {
                    venue_id,
                    detail: "unit test injected perp leg failure".to_owned(),
                });
            }
            self.inner.submit_order(request)
        }
    }

    #[derive(Default)]
    struct LiveLikeAcceptedSubmitOrder {
        calls: usize,
    }

    impl SubmitOrder for LiveLikeAcceptedSubmitOrder {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> VenueExecResult<MutableActionReceipt> {
            self.calls += 1;
            Ok(MutableActionReceipt {
                action_id: MutableActionId::new(format!("live-like:submit:{}", self.calls))?,
                kind: MutableActionKind::SubmitOrder,
                status: MutableActionStatus::Accepted,
                idempotency_key: request.idempotency_key,
                venue_id: request.venue_id,
                external_ref: Some(ExternalActionRef::Order(ExternalOrderId::new(format!(
                    "binance:spot:order:{}",
                    self.calls
                ))?)),
                duplicate: false,
                simulated: false,
            })
        }
    }

    #[cfg(feature = "live-exec")]
    fn binance_signing_policy(value: &str) -> arb_signing::SigningPolicy {
        arb_signing::SigningPolicy::real_signing_enabled(
            arb_signing::SigningPolicyRef::new(value).expect("policy ref"),
        )
    }

    #[cfg(feature = "live-exec")]
    fn binance_test_signer(
        timestamp_millis: u64,
    ) -> arb_signing::real::BinanceHmacSha256SigningProvider<
        GeneratedBinanceCredentialProvider,
        FixedBinanceTimestamp,
    > {
        arb_signing::real::BinanceHmacSha256SigningProvider::new(
            GeneratedBinanceCredentialProvider,
            FixedBinanceTimestamp(timestamp_millis),
        )
    }

    #[cfg(feature = "live-exec")]
    fn bybit_signing_policy(value: &str) -> arb_signing::SigningPolicy {
        arb_signing::SigningPolicy::real_signing_enabled(
            arb_signing::SigningPolicyRef::new(value).expect("policy ref"),
        )
    }

    #[cfg(feature = "live-exec")]
    fn bybit_test_signer(
        timestamp_millis: u64,
    ) -> arb_signing::real::BybitHmacSha256SigningProvider<
        GeneratedBinanceCredentialProvider,
        FixedBinanceTimestamp,
    > {
        arb_signing::real::BybitHmacSha256SigningProvider::new(
            GeneratedBinanceCredentialProvider,
            FixedBinanceTimestamp(timestamp_millis),
        )
    }

    #[cfg(feature = "live-exec")]
    fn okx_signing_policy(value: &str) -> arb_signing::SigningPolicy {
        arb_signing::SigningPolicy::real_signing_enabled(
            arb_signing::SigningPolicyRef::new(value).expect("policy ref"),
        )
    }

    #[cfg(feature = "live-exec")]
    fn okx_test_signer(
        timestamp_rfc3339: &'static str,
    ) -> arb_signing::real::OkxHmacSha256SigningProvider<
        GeneratedBinanceCredentialProvider,
        FixedOkxTimestamp,
    > {
        arb_signing::real::OkxHmacSha256SigningProvider::new(
            GeneratedBinanceCredentialProvider,
            FixedOkxTimestamp(timestamp_rfc3339),
        )
    }

    #[cfg(feature = "live-exec")]
    fn bitget_signing_policy(value: &str) -> arb_signing::SigningPolicy {
        arb_signing::SigningPolicy::real_signing_enabled(
            arb_signing::SigningPolicyRef::new(value).expect("policy ref"),
        )
    }

    #[cfg(feature = "live-exec")]
    fn bitget_test_signer(
        timestamp_millis: u64,
    ) -> arb_signing::real::BitgetHmacSha256SigningProvider<
        GeneratedBinanceCredentialProvider,
        FixedBinanceTimestamp,
    > {
        arb_signing::real::BitgetHmacSha256SigningProvider::new(
            GeneratedBinanceCredentialProvider,
            FixedBinanceTimestamp(timestamp_millis),
        )
    }

    #[cfg(all(feature = "live-exec", unix))]
    fn aster_signing_policy(value: &str) -> arb_signing::SigningPolicy {
        arb_signing::SigningPolicy::real_signing_enabled(
            arb_signing::SigningPolicyRef::new(value).expect("policy ref"),
        )
    }

    #[cfg(all(feature = "live-exec", unix))]
    fn aster_test_signer(
        nonce_micros: u64,
    ) -> arb_signing::real::AsterEip712ExternalSigningProvider<
        arb_signing::real::LiteralAsterExternalSignerCommandProvider,
        FixedAsterNonce,
    > {
        let signature = format!("0x{}", "a".repeat(130));
        let signer_script = write_aster_signer_script(&signature);
        arb_signing::real::AsterEip712ExternalSigningProvider::new(
            arb_signing::real::LiteralAsterExternalSignerCommandProvider::new(
                signer_script.display().to_string(),
            )
            .expect("literal Aster signer command"),
            FixedAsterNonce(nonce_micros),
        )
    }

    #[cfg(feature = "live-exec")]
    fn hyperliquid_signing_policy(value: &str) -> arb_signing::SigningPolicy {
        arb_signing::SigningPolicy::real_signing_enabled(
            arb_signing::SigningPolicyRef::new(value).expect("policy ref"),
        )
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Copy, Debug)]
    struct FixedBinanceTimestamp(u64);

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Copy, Debug)]
    struct FixedOkxTimestamp(&'static str);

    #[cfg(all(feature = "live-exec", unix))]
    #[derive(Clone, Copy, Debug)]
    struct FixedAsterNonce(u64);

    #[cfg(feature = "live-exec")]
    impl arb_signing::real::BinanceTimestampProvider for FixedBinanceTimestamp {
        fn timestamp_millis(
            &self,
            _audit_ref: &arb_signing::SigningAuditRef,
        ) -> arb_signing::SigningResult<u64> {
            Ok(self.0)
        }
    }

    #[cfg(feature = "live-exec")]
    impl arb_signing::real::BybitTimestampProvider for FixedBinanceTimestamp {
        fn timestamp_millis(
            &self,
            _audit_ref: &arb_signing::SigningAuditRef,
        ) -> arb_signing::SigningResult<u64> {
            Ok(self.0)
        }
    }

    #[cfg(feature = "live-exec")]
    impl arb_signing::real::BitgetTimestampProvider for FixedBinanceTimestamp {
        fn timestamp_millis(
            &self,
            _audit_ref: &arb_signing::SigningAuditRef,
        ) -> arb_signing::SigningResult<u64> {
            Ok(self.0)
        }
    }

    #[cfg(feature = "live-exec")]
    impl arb_signing::real::OkxTimestampProvider for FixedOkxTimestamp {
        fn timestamp_rfc3339(
            &self,
            _audit_ref: &arb_signing::SigningAuditRef,
        ) -> arb_signing::SigningResult<String> {
            Ok(self.0.to_owned())
        }
    }

    #[cfg(all(feature = "live-exec", unix))]
    impl arb_signing::real::AsterNonceProvider for FixedAsterNonce {
        fn nonce_micros(
            &self,
            _audit_ref: &arb_signing::SigningAuditRef,
        ) -> arb_signing::SigningResult<u64> {
            Ok(self.0)
        }
    }

    #[cfg(all(feature = "live-exec", unix))]
    fn write_aster_signer_script(signature: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "arb-venue-exec-aster-test-{}-{}",
            std::process::id(),
            signature.len()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let script_path = dir.join("signer.sh");
        std::fs::write(
            &script_path,
            format!("#!/bin/sh\ncat >/dev/null\nprintf '{}'\n", signature),
        )
        .expect("write Aster signer script");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script_path, permissions).expect("chmod signer script");
        script_path
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Copy, Debug)]
    struct GeneratedBinanceCredentialProvider;

    #[cfg(feature = "live-exec")]
    impl arb_signing::real::BinanceCredentialProvider for GeneratedBinanceCredentialProvider {
        fn load_binance_credentials(
            &self,
            _audit_ref: &arb_signing::SigningAuditRef,
        ) -> arb_signing::SigningResult<arb_signing::real::BinanceApiCredentials> {
            arb_signing::real::BinanceApiCredentials::new(
                "test-api-key-binance-exec-unit",
                "test-api-secret-binance-exec-unit",
            )
        }
    }

    #[cfg(feature = "live-exec")]
    impl arb_signing::real::BybitCredentialProvider for GeneratedBinanceCredentialProvider {
        fn load_bybit_credentials(
            &self,
            _audit_ref: &arb_signing::SigningAuditRef,
        ) -> arb_signing::SigningResult<arb_signing::real::BybitApiCredentials> {
            arb_signing::real::BybitApiCredentials::new(
                "test-api-key-bybit-exec-unit",
                "test-api-secret-bybit-exec-unit",
            )
        }
    }

    #[cfg(feature = "live-exec")]
    impl arb_signing::real::OkxCredentialProvider for GeneratedBinanceCredentialProvider {
        fn load_okx_credentials(
            &self,
            _audit_ref: &arb_signing::SigningAuditRef,
        ) -> arb_signing::SigningResult<arb_signing::real::OkxApiCredentials> {
            arb_signing::real::OkxApiCredentials::new(
                "test-api-key-okx-exec-unit",
                "test-api-secret-okx-exec-unit",
                "test-passphrase-okx-exec-unit",
            )
        }
    }

    #[cfg(feature = "live-exec")]
    impl arb_signing::real::BitgetCredentialProvider for GeneratedBinanceCredentialProvider {
        fn load_bitget_credentials(
            &self,
            _audit_ref: &arb_signing::SigningAuditRef,
        ) -> arb_signing::SigningResult<arb_signing::real::BitgetApiCredentials> {
            arb_signing::real::BitgetApiCredentials::new(
                "test-api-key-bitget-exec-unit",
                "test-api-secret-bitget-exec-unit",
                "test-passphrase-bitget-exec-unit",
            )
        }
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedBinanceCall {
        market: live::BinanceExecMarket,
        method: live::BinanceExecHttpMethod,
        endpoint: &'static str,
        api_key_header_name: String,
        api_key_header_value: String,
        signed_query: String,
        debug: String,
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordingTransport {
        status_code: u16,
        body: String,
        calls: Vec<RecordedBinanceCall>,
    }

    #[cfg(feature = "live-exec")]
    impl RecordingTransport {
        fn ok(status_code: u16, body: impl Into<String>) -> Self {
            Self {
                status_code,
                body: body.into(),
                calls: Vec::new(),
            }
        }
    }

    #[cfg(feature = "live-exec")]
    impl live::BinanceExecTransport for RecordingTransport {
        fn send_signed(
            &mut self,
            request: live::BinanceSignedRequest<'_>,
        ) -> VenueExecResult<live::BinanceExecHttpResponse> {
            self.calls.push(RecordedBinanceCall {
                market: request.market(),
                method: request.method(),
                endpoint: request.endpoint(),
                api_key_header_name: request.api_key_header_name().to_owned(),
                api_key_header_value: request.api_key_header_value().to_owned(),
                signed_query: request.signed_query_for_transport().to_owned(),
                debug: format!("{request:?}"),
            });
            Ok(live::BinanceExecHttpResponse::new(
                self.status_code,
                self.body.clone(),
            ))
        }
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedBybitCall {
        market: live::BybitExecMarket,
        method: live::BybitExecHttpMethod,
        endpoint: &'static str,
        api_key_header_name: String,
        api_key_header_value: String,
        timestamp_millis: u64,
        recv_window_ms: u64,
        payload: String,
        signature: String,
        debug: String,
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordingBybitTransport {
        status_code: u16,
        body: String,
        calls: Vec<RecordedBybitCall>,
    }

    #[cfg(feature = "live-exec")]
    impl RecordingBybitTransport {
        fn ok(status_code: u16, body: impl Into<String>) -> Self {
            Self {
                status_code,
                body: body.into(),
                calls: Vec::new(),
            }
        }
    }

    #[cfg(feature = "live-exec")]
    impl live::BybitExecTransport for RecordingBybitTransport {
        fn send_signed(
            &mut self,
            request: live::BybitSignedRequest<'_>,
        ) -> VenueExecResult<live::BybitExecHttpResponse> {
            self.calls.push(RecordedBybitCall {
                market: request.market(),
                method: request.method(),
                endpoint: request.endpoint(),
                api_key_header_name: request.api_key_header_name().to_owned(),
                api_key_header_value: request.api_key_header_value().to_owned(),
                timestamp_millis: request.timestamp_millis(),
                recv_window_ms: request.recv_window_ms(),
                payload: request.payload_for_transport().to_owned(),
                signature: request.signature_header_value().to_owned(),
                debug: format!("{request:?}"),
            });
            Ok(live::BybitExecHttpResponse::new(
                self.status_code,
                self.body.clone(),
            ))
        }
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedOkxCall {
        market: live::OkxExecMarket,
        method: arb_signing::real::OkxRestMethod,
        request_path: String,
        api_key_header_name: String,
        api_key_header_value: String,
        signature_header_name: String,
        signature_header_value: String,
        timestamp_header_name: String,
        timestamp_header_value: String,
        passphrase_header_name: String,
        passphrase_header_value: String,
        body: String,
        debug: String,
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordingOkxTransport {
        status_code: u16,
        body: String,
        calls: Vec<RecordedOkxCall>,
    }

    #[cfg(feature = "live-exec")]
    impl RecordingOkxTransport {
        fn ok(status_code: u16, body: impl Into<String>) -> Self {
            Self {
                status_code,
                body: body.into(),
                calls: Vec::new(),
            }
        }
    }

    #[cfg(feature = "live-exec")]
    impl live::OkxExecTransport for RecordingOkxTransport {
        fn send_signed(
            &mut self,
            request: live::OkxSignedRequest<'_>,
        ) -> VenueExecResult<live::OkxExecHttpResponse> {
            self.calls.push(RecordedOkxCall {
                market: request.market(),
                method: request.method(),
                request_path: request.request_path().to_owned(),
                api_key_header_name: request.api_key_header_name().to_owned(),
                api_key_header_value: request.api_key_header_value().to_owned(),
                signature_header_name: request.signature_header_name().to_owned(),
                signature_header_value: request.signature_header_value().to_owned(),
                timestamp_header_name: request.timestamp_header_name().to_owned(),
                timestamp_header_value: request.timestamp_header_value().to_owned(),
                passphrase_header_name: request.passphrase_header_name().to_owned(),
                passphrase_header_value: request.passphrase_header_value().to_owned(),
                body: request.body_for_transport().to_owned(),
                debug: format!("{request:?}"),
            });
            Ok(live::OkxExecHttpResponse::new(
                self.status_code,
                self.body.clone(),
            ))
        }
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedBitgetCall {
        market: live::BitgetExecMarket,
        method: arb_signing::real::BitgetRestMethod,
        request_path: String,
        api_key_header_name: String,
        api_key_header_value: String,
        signature_header_name: String,
        signature_header_value: String,
        timestamp_header_name: String,
        timestamp_header_value: String,
        passphrase_header_name: String,
        passphrase_header_value: String,
        body: String,
        debug: String,
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordingBitgetTransport {
        status_code: u16,
        body: String,
        calls: Vec<RecordedBitgetCall>,
    }

    #[cfg(feature = "live-exec")]
    impl RecordingBitgetTransport {
        fn ok(status_code: u16, body: impl Into<String>) -> Self {
            Self {
                status_code,
                body: body.into(),
                calls: Vec::new(),
            }
        }
    }

    #[cfg(feature = "live-exec")]
    impl live::BitgetExecTransport for RecordingBitgetTransport {
        fn send_signed(
            &mut self,
            request: live::BitgetSignedRequest<'_>,
        ) -> VenueExecResult<live::BitgetExecHttpResponse> {
            self.calls.push(RecordedBitgetCall {
                market: request.market(),
                method: request.method(),
                request_path: request.request_path().to_owned(),
                api_key_header_name: request.api_key_header_name().to_owned(),
                api_key_header_value: request.api_key_header_value().to_owned(),
                signature_header_name: request.signature_header_name().to_owned(),
                signature_header_value: request.signature_header_value().to_owned(),
                timestamp_header_name: request.timestamp_header_name().to_owned(),
                timestamp_header_value: request.timestamp_header_value(),
                passphrase_header_name: request.passphrase_header_name().to_owned(),
                passphrase_header_value: request.passphrase_header_value().to_owned(),
                body: request.body_for_transport().to_owned(),
                debug: format!("{request:?}"),
            });
            Ok(live::BitgetExecHttpResponse::new(
                self.status_code,
                self.body.clone(),
            ))
        }
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedAsterCall {
        method: live::AsterExecHttpMethod,
        base_url: String,
        endpoint: &'static str,
        signed_query: String,
        debug: String,
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordingAsterTransport {
        status_code: u16,
        bodies: Vec<String>,
        calls: Vec<RecordedAsterCall>,
    }

    #[cfg(feature = "live-exec")]
    impl RecordingAsterTransport {
        fn ok<const N: usize>(bodies: [&'static str; N]) -> Self {
            Self {
                status_code: 200,
                bodies: bodies.into_iter().map(str::to_owned).collect(),
                calls: Vec::new(),
            }
        }
    }

    #[cfg(feature = "live-exec")]
    impl live::AsterExecTransport for RecordingAsterTransport {
        fn send_signed(
            &mut self,
            request: live::AsterSignedRequest<'_>,
        ) -> VenueExecResult<live::AsterExecHttpResponse> {
            self.calls.push(RecordedAsterCall {
                method: request.method(),
                base_url: request.base_url().to_owned(),
                endpoint: request.endpoint(),
                signed_query: request.signed_query_for_transport().to_owned(),
                debug: format!("{request:?}"),
            });
            let body = self
                .bodies
                .get(self.calls.len().saturating_sub(1))
                .cloned()
                .unwrap_or_else(|| "{}".to_owned());
            Ok(live::AsterExecHttpResponse::new(self.status_code, body))
        }
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct StaticHyperliquidSigner;

    #[cfg(feature = "live-exec")]
    impl live::HyperliquidExchangeSigner for StaticHyperliquidSigner {
        fn sign_l1_action(
            &self,
            _input: live::HyperliquidSigningInput,
        ) -> VenueExecResult<live::HyperliquidSignatureJson> {
            live::HyperliquidSignatureJson::new(format!(
                r#"{{"r":"0x{}","s":"0x{}","v":27}}"#,
                "1".repeat(64),
                "2".repeat(64)
            ))
        }
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedHyperliquidCall {
        base_url: String,
        endpoint: &'static str,
        body: String,
    }

    #[cfg(feature = "live-exec")]
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordingHyperliquidTransport {
        status_code: u16,
        bodies: Vec<String>,
        calls: Vec<RecordedHyperliquidCall>,
    }

    #[cfg(feature = "live-exec")]
    impl RecordingHyperliquidTransport {
        fn ok<const N: usize>(bodies: [&'static str; N]) -> Self {
            Self {
                status_code: 200,
                bodies: bodies.into_iter().map(str::to_owned).collect(),
                calls: Vec::new(),
            }
        }
    }

    #[cfg(feature = "live-exec")]
    impl live::HyperliquidExecTransport for RecordingHyperliquidTransport {
        fn post_json(
            &mut self,
            base_url: &str,
            endpoint: &'static str,
            body: &str,
        ) -> VenueExecResult<live::HyperliquidExecHttpResponse> {
            self.calls.push(RecordedHyperliquidCall {
                base_url: base_url.to_owned(),
                endpoint,
                body: body.to_owned(),
            });
            let body = self
                .bodies
                .get(self.calls.len().saturating_sub(1))
                .cloned()
                .unwrap_or_else(|| "{}".to_owned());
            Ok(live::HyperliquidExecHttpResponse::new(
                self.status_code,
                body,
            ))
        }
    }
}
