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
    ExecutionPlan, TransitionSide,
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

/// Binance 私有订单市场。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinancePrivateOrderMarket {
    Spot,
    UsdmFutures,
}

impl BinancePrivateOrderMarket {
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

    fn instrument_suffix(self) -> &'static str {
        match self {
            Self::Spot => "SPOT",
            Self::UsdmFutures => "USDM-PERP",
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
    pub market: BinancePrivateOrderMarket,
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
            quantity,
            limit_price,
            client_order_id,
            idempotency_key,
        }
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
        }
    }

    fn fingerprint(&self) -> RequestFingerprint {
        RequestFingerprint(format!(
            "kind={};venue={};account={};instrument={};side={};order_type={};quantity={};limit_price={};client_order_id={}",
            MutableActionKind::SubmitOrder,
            self.venue_id,
            self.account_id,
            self.instrument_id,
            self.side.as_str(),
            self.order_type.as_str(),
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

    let spot = legs
        .iter()
        .copied()
        .find(|leg| basis_role(leg) == Some(BasisLegRole::Spot));
    let perp = legs
        .iter()
        .copied()
        .find(|leg| basis_role(leg) == Some(BasisLegRole::Perp));
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

    let request = SubmitOrderRequest::new(
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
    if is_basis_dispatch(dispatch_plan)
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
    Some(ResidualRisk {
        severity: "RiskCritical",
        detail: format!(
            "{} accepted leg(s) preceded failure on `{}`; stop remaining legs and reconcile before any retry",
            receipts.len(),
            failed.plan_leg_id
        ),
    })
}

fn is_basis_dispatch(dispatch_plan: &ExecutionDispatchPlan) -> bool {
    dispatch_plan
        .requests
        .iter()
        .any(|request| request.basis_leg_role.is_some())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BasisLegRole {
    Spot,
    Perp,
}

fn basis_role(leg: &ExecutionLeg) -> Option<BasisLegRole> {
    match leg.basis_leg_role.as_ref().map(|role| role.as_str()) {
        Some("spot_buy") | Some("spot_long") => return Some(BasisLegRole::Spot),
        Some("perp_short") | Some("perp_sell") => return Some(BasisLegRole::Perp),
        _ => {}
    }
    let instrument_id = leg.instrument_id.as_ref()?.as_str();
    if instrument_id.ends_with(":SPOT") {
        Some(BasisLegRole::Spot)
    } else if instrument_id.ends_with(":USDM-PERP") || instrument_id.ends_with(":PERP") {
        Some(BasisLegRole::Perp)
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
        BinancePrivateOrderMarket::Spot,
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
        BinancePrivateOrderMarket::UsdmFutures,
        OrderConfirmationSource::PrivateStream,
        venue_id,
        account_id,
        source_event_id.into(),
        order,
    )
}

/// 解析 Binance signed REST 查单响应。
pub fn parse_binance_order_query_confirmation(
    market: BinancePrivateOrderMarket,
    venue_id: VenueId,
    account_id: AccountId,
    source_event_id: impl Into<String>,
    body: &str,
) -> VenueExecResult<PrivateOrderUpdate> {
    parse_binance_private_order_fields(
        market,
        OrderConfirmationSource::OrderQuery,
        venue_id,
        account_id,
        source_event_id.into(),
        body,
    )
}

fn parse_binance_private_order_fields(
    market: BinancePrivateOrderMarket,
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

fn required_json_field(body: &str, field: &'static str) -> VenueExecResult<String> {
    json_field_value(body, field).ok_or(VenueExecError::InvalidRequest {
        field,
        reason: "required Binance JSON field is missing",
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
    if value.is_empty() || value == "null" {
        None
    } else {
        Some(value.to_owned())
    }
}

fn json_object_field<'a>(body: &'a str, field: &str) -> Option<&'a str> {
    let pattern = format!("\"{field}\"");
    let mut rest = body.get(body.find(&pattern)? + pattern.len()..)?;
    rest = rest.trim_start();
    rest = rest.strip_prefix(':')?.trim_start();
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

    use arb_domain::{AccountId, InstrumentId, OrderId, VenueId};
    use arb_signing::real::{
        BinanceHmacSigningInput, BinanceRequestParam, BinanceSignedEndpoint, RealSigningProvider,
    };
    use arb_signing::{SigningPolicy, SigningPurpose, SigningRequestId};

    use super::{
        parse_binance_order_query_confirmation, unknown_status_report, BinancePrivateOrderMarket,
        CancelOrder, CancelOrderRequest, ConfirmOrderStatus, ConfirmOrderStatusRequest,
        ExternalActionRef, ExternalOrderId, IdempotencyKey, MutableActionId, MutableActionKind,
        MutableActionReceipt, MutableActionStatus, MutableActionStatusReport, MutableOrderType,
        OrderReference, OrderSide, PrivateOrderUpdate, QueryActionStatus, QueryActionStatusRequest,
        RequestFingerprint, RequestTransfer, SubmitOrder, SubmitOrderRequest, TransferRequest,
        VenueExecError, VenueExecResult,
    };

    /// Binance Spot 下单、撤单和查单 endpoint。
    pub const BINANCE_SPOT_ORDER_ENDPOINT: &str = "/api/v3/order";
    /// Binance USD-M Futures 下单、撤单和查单 endpoint。
    pub const BINANCE_USDM_ORDER_ENDPOINT: &str = "/fapi/v1/order";
    /// 默认 Binance signed endpoint 接收窗口。
    pub const DEFAULT_BINANCE_RECV_WINDOW_MS: u64 = 5_000;
    const MAX_BINANCE_RECV_WINDOW_MS: u64 = 60_000;
    const CURL_STATUS_MARKER: &str = "\n__ARB_BINANCE_HTTP_STATUS__:";

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
                signing_policy,
            })
        }

        pub fn with_recv_window_ms(mut self, recv_window_ms: u64) -> VenueExecResult<Self> {
            validate_recv_window(recv_window_ms)?;
            self.recv_window_ms = recv_window_ms;
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
            let signing_request_id = SigningRequestId::new(format!(
                "signing-request/binance-exec/query-order/{}",
                request.source_event_id
            ))
            .map_err(signing_error)?;
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
        let mut params = vec![
            binance_param("symbol", symbol)?,
            binance_param("side", binance_side(request.side))?,
        ];
        match (config.market, request.order_type) {
            (_, MutableOrderType::Market) => {
                params.push(binance_param("type", "MARKET")?);
                params.push(binance_param("quantity", request.quantity.to_string())?);
            }
            (BinanceExecMarket::Spot, MutableOrderType::Limit) => {
                params.push(binance_param("type", "LIMIT")?);
                params.push(binance_param("timeInForce", "GTC")?);
                params.push(binance_param("quantity", request.quantity.to_string())?);
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
                params.push(binance_param("quantity", request.quantity.to_string())?);
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
                params.push(binance_param("timeInForce", "GTC")?);
                params.push(binance_param("quantity", request.quantity.to_string())?);
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
                params.push(binance_param("quantity", request.quantity.to_string())?);
                params.push(binance_param(
                    "price",
                    request
                        .limit_price
                        .expect("validated post-only order price")
                        .to_string(),
                )?);
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

    fn private_market_from_exec_market(market: BinanceExecMarket) -> BinancePrivateOrderMarket {
        match market {
            BinanceExecMarket::Spot => BinancePrivateOrderMarket::Spot,
            BinanceExecMarket::UsdmFutures => BinancePrivateOrderMarket::UsdmFutures,
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
        assert_eq!(update.market, BinancePrivateOrderMarket::Spot);
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

        assert_eq!(update.market, BinancePrivateOrderMarket::UsdmFutures);
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
            BinancePrivateOrderMarket::Spot,
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
        assert_eq!(requests[0].quantity.to_string(), "0.001");
        assert_eq!(
            requests[0].limit_price.expect("spot limit").to_string(),
            "50000.00"
        );
        assert_eq!(requests[0].idempotency_key.as_str(), "idem:plan:basis:spot");
        assert_eq!(requests[1].venue_id.as_str(), "venue:BINANCE-USDM");
        assert_eq!(requests[1].side, OrderSide::Sell);
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
    #[derive(Clone, Copy, Debug)]
    struct FixedBinanceTimestamp(u64);

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
}
