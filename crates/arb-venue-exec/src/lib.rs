//! `arb-venue-exec` 可变执行适配器边界。
//!
//! 中文说明：本 crate 只定义可变执行 trait、模拟实现和幂等键处理。默认
//! feature 为空，不连接真实交易场所、不提交真实资金动作、不做策略判断或
//! 风控批准。
//!
//! ```compile_fail
//! use arb_venue_exec::live::RealVenueExecAdapter;
//! ```

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use arb_domain::{AccountId, Amount, AssetId, InstrumentId, OrderId, Price, Quantity, VenueId};

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
    IdempotencyConflict {
        idempotency_key: IdempotencyKey,
        existing_fingerprint: String,
        incoming_fingerprint: String,
    },
    LiveExecutionUnavailable,
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
            Self::IdempotencyConflict {
                idempotency_key,
                existing_fingerprint,
                incoming_fingerprint,
            } => write!(
                f,
                "idempotency key `{idempotency_key}` was reused with a different request: existing `{existing_fingerprint}`, incoming `{incoming_fingerprint}`"
            ),
            Self::LiveExecutionUnavailable => {
                f.write_str("live mutable execution is unavailable in this build")
            }
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
    use super::{VenueExecError, VenueExecResult};

    /// 显式 feature 下仍不可用的实盘占位。
    ///
    /// 中文说明：S11-01 不实现真实交易场所连接。该类型只让调用方得到明确
    /// fail-closed 错误，不能提交真实订单、撤单或转账。
    #[derive(Clone, Debug, Default)]
    pub struct LiveExecutionUnavailable;

    impl LiveExecutionUnavailable {
        pub fn connect() -> VenueExecResult<Self> {
            Err(VenueExecError::LiveExecutionUnavailable)
        }
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

    use arb_domain::{Decimal, DomainResult};

    use super::*;

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
}
