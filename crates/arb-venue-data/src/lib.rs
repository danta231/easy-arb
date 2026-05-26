//! `arb-venue-data` 只读场所数据适配接口。
//!
//! 中文说明：本 crate 只定义读取外部行情、余额、仓位、工具信息和场所健康
//! 状态的边界。接口不表达下单、撤单、转账、签名或账户变更动作；可变执行能力
//! 必须留在后续 `arb-venue-exec` 边界中。
//!
//! ```compile_fail
//! use arb_venue_data::VenueReadAdapter;
//!
//! fn cannot_place_order(adapter: &dyn VenueReadAdapter) {
//!     let _ = adapter.place_order();
//! }
//! ```
//!
//! ```compile_fail
//! use arb_venue_data::VenueReadAdapter;
//!
//! fn cannot_cancel_order(adapter: &dyn VenueReadAdapter) {
//!     let _ = adapter.cancel_order();
//! }
//! ```
//!
//! ```compile_fail
//! use arb_venue_data::VenueReadAdapter;
//!
//! fn cannot_transfer(adapter: &dyn VenueReadAdapter) {
//!     let _ = adapter.transfer();
//! }
//! ```
//!
//! ```compile_fail
//! use arb_venue_data::VenueReadAdapter;
//!
//! fn cannot_sign(adapter: &dyn VenueReadAdapter) {
//!     let _ = adapter.sign_request();
//! }
//! ```

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use arb_contracts::InstrumentKind;
use arb_contracts::{
    from_json_strict, CanonicalJson, JsonValue, NormalizedEvent, NormalizedEventType,
};
use arb_domain::{
    AccountId, Amount, AssetId, Decimal, DomainError, InstrumentId, Pnl, PositionId, Price,
    Quantity, UtcTimestamp, VenueId,
};

/// 只读场所数据模块统一返回类型。
///
/// 中文说明：读取失败、字段非法、未知外部状态或数据不可用都必须显式返回错误，
/// 不能被适配器当作成功快照。
pub type VenueDataResult<T> = Result<T, VenueDataError>;

/// 只读数据面。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReadOnlySurface {
    MarketData,
    Balance,
    Position,
    InstrumentInfo,
    VenueHealth,
}

impl ReadOnlySurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MarketData => "MarketData",
            Self::Balance => "Balance",
            Self::Position => "Position",
            Self::InstrumentInfo => "InstrumentInfo",
            Self::VenueHealth => "VenueHealth",
        }
    }
}

/// 只读场所数据错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VenueDataError {
    /// 领域基础类型解析或校验失败。
    Domain(DomainError),
    /// 已分类的外部场所错误。
    External(ClassifiedExternalError),
    /// 查询参数非法。
    InvalidQuery {
        field: &'static str,
        reason: &'static str,
    },
    /// 指定只读数据面当前不可用。
    DataUnavailable {
        venue_id: VenueId,
        surface: ReadOnlySurface,
        reason: String,
    },
    /// 外部状态未知，调用方必须 fail closed。
    UnknownExternalState {
        venue_id: VenueId,
        surface: ReadOnlySurface,
        detail: String,
    },
}

impl fmt::Display for VenueDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(error) => write!(f, "{error}"),
            Self::External(error) => write!(f, "{error}"),
            Self::InvalidQuery { field, reason } => {
                write!(f, "venue data query field `{field}` is invalid: {reason}")
            }
            Self::DataUnavailable {
                venue_id,
                surface,
                reason,
            } => write!(
                f,
                "venue `{venue_id}` read-only surface `{}` is unavailable: {reason}",
                surface.as_str()
            ),
            Self::UnknownExternalState {
                venue_id,
                surface,
                detail,
            } => write!(
                f,
                "venue `{venue_id}` read-only surface `{}` is unknown: {detail}",
                surface.as_str()
            ),
        }
    }
}

impl Error for VenueDataError {}

impl From<DomainError> for VenueDataError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

/// Parse Binance public server time response milliseconds.
///
/// 中文说明：该函数只解析 `/api/v3/time`、`/fapi/v1/time` 等公开 REST 响应中的
/// `serverTime` 字段，不访问私有账户，也不表达签名或可变执行能力。
pub fn parse_binance_server_time_millis(body: &str) -> VenueDataResult<u64> {
    let value = json_scalar_field(body, "serverTime")?;
    value
        .parse::<u64>()
        .map_err(|_| invalid_payload("serverTime", "expected unsigned integer milliseconds"))
}

fn json_scalar_field<'a>(body: &'a str, field: &'static str) -> VenueDataResult<&'a str> {
    let trimmed = body.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err(invalid_payload("body", "expected JSON object"));
    }
    let key = format!("\"{field}\"");
    let key_start = trimmed
        .find(&key)
        .ok_or_else(|| invalid_payload(field, "missing field"))?;
    let after_key = key_start + key.len();
    let colon = after_key
        + trimmed[after_key..]
            .find(':')
            .ok_or_else(|| invalid_payload(field, "missing ':' after field"))?;
    let value_start = skip_json_ws(trimmed, colon + 1);
    if trimmed[value_start..].starts_with('"') {
        let value_end = json_string_end(trimmed, value_start)?;
        return Ok(&trimmed[value_start + 1..value_end - 1]);
    }
    let value_end = trimmed[value_start..]
        .find([',', '}'])
        .map(|offset| value_start + offset)
        .unwrap_or(trimmed.len());
    let value = trimmed[value_start..value_end].trim();
    if value.is_empty() {
        Err(invalid_payload(field, "empty scalar value"))
    } else {
        Ok(value)
    }
}

fn skip_json_ws(input: &str, mut index: usize) -> usize {
    while input
        .as_bytes()
        .get(index)
        .is_some_and(u8::is_ascii_whitespace)
    {
        index += 1;
    }
    index
}

fn json_string_end(input: &str, quote_start: usize) -> VenueDataResult<usize> {
    let mut escaped = false;
    for (offset, ch) in input[quote_start + 1..].char_indices() {
        let absolute = quote_start + 1 + offset;
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Ok(absolute + 1);
        }
    }
    Err(invalid_payload("serverTime", "unterminated JSON string"))
}

fn invalid_payload(field: &'static str, reason: &'static str) -> VenueDataError {
    VenueDataError::InvalidQuery { field, reason }
}

/// 外部只读数据错误分类。
///
/// 中文说明：适配器不能把网络、限频、字段缺失、乱序或重复消息静默吞掉；
/// 这些分类会进入健康事件或返回错误，供风控 fail closed。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExternalErrorClass {
    Disconnected,
    Reconnecting,
    RateLimited,
    Timeout,
    MalformedPayload,
    MissingField,
    OutOfOrderMessage,
    DuplicateMessage,
    UnknownExternalState,
}

impl ExternalErrorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Reconnecting => "Reconnecting",
            Self::RateLimited => "RateLimited",
            Self::Timeout => "Timeout",
            Self::MalformedPayload => "MalformedPayload",
            Self::MissingField => "MissingField",
            Self::OutOfOrderMessage => "OutOfOrderMessage",
            Self::DuplicateMessage => "DuplicateMessage",
            Self::UnknownExternalState => "UnknownExternalState",
        }
    }

    pub fn reason_code(self) -> &'static str {
        match self {
            Self::Disconnected | Self::Reconnecting => "VENUE_UNHEALTHY",
            Self::RateLimited => "RATE_LIMITED",
            Self::Timeout => "VENUE_UNHEALTHY",
            Self::MalformedPayload | Self::MissingField => "REQUIRES_MORE_DATA",
            Self::OutOfOrderMessage | Self::UnknownExternalState => "UNKNOWN_STATE",
            Self::DuplicateMessage => "DUPLICATE_MESSAGE",
        }
    }

    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::Disconnected
                | Self::Reconnecting
                | Self::RateLimited
                | Self::Timeout
                | Self::DuplicateMessage
        )
    }

    pub fn fail_closed(self) -> bool {
        !matches!(self, Self::DuplicateMessage)
    }
}

/// 已分类外部错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassifiedExternalError {
    pub venue_id: VenueId,
    pub surface: ReadOnlySurface,
    pub class: ExternalErrorClass,
    pub reason_code: String,
    pub detail: String,
    pub retryable: bool,
    pub fail_closed: bool,
}

impl ClassifiedExternalError {
    pub fn new(
        venue_id: VenueId,
        surface: ReadOnlySurface,
        class: ExternalErrorClass,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            venue_id,
            surface,
            class,
            reason_code: class.reason_code().to_owned(),
            detail: detail.into(),
            retryable: class.retryable(),
            fail_closed: class.fail_closed(),
        }
    }
}

impl fmt::Display for ClassifiedExternalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "venue `{}` read-only surface `{}` external error `{}` reason `{}`: {}",
            self.venue_id,
            self.surface.as_str(),
            self.class.as_str(),
            self.reason_code,
            self.detail
        )
    }
}

/// 数据新鲜度状态。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FreshnessStatus {
    Fresh,
    Stale,
}

impl FreshnessStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "Fresh",
            Self::Stale => "Stale",
        }
    }
}

/// 只读快照的新鲜度元数据。
///
/// 中文说明：适配器必须显式记录业务观察时间、摄入时间和最大允许年龄。过期数据
/// 可以被读取，但必须标记为 `Stale`，供风控按风险处理。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DataFreshness {
    pub observed_at: UtcTimestamp,
    pub ingested_at: UtcTimestamp,
    pub max_age_ms: u64,
    pub status: FreshnessStatus,
}

impl DataFreshness {
    /// 根据观察时间和摄入时间计算新鲜度。
    pub fn new(
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let age_ms =
            timestamp_delta_ms(observed_at, ingested_at).ok_or(VenueDataError::InvalidQuery {
                field: "freshness.ingested_at",
                reason: "ingested timestamp must be greater than or equal to observed timestamp",
            })?;
        let status = if age_ms <= max_age_ms {
            FreshnessStatus::Fresh
        } else {
            FreshnessStatus::Stale
        };
        Ok(Self {
            observed_at,
            ingested_at,
            max_age_ms,
            status,
        })
    }

    /// 快照年龄，单位毫秒。
    pub fn age_ms(self) -> u64 {
        timestamp_delta_ms(self.observed_at, self.ingested_at).unwrap_or(u64::MAX)
    }

    /// 是否已过期。
    pub fn is_stale(self) -> bool {
        self.status == FreshnessStatus::Stale
    }
}

fn timestamp_delta_ms(earlier: UtcTimestamp, later: UtcTimestamp) -> Option<u64> {
    if later < earlier {
        return None;
    }

    let second_delta = later.unix_seconds().checked_sub(earlier.unix_seconds())?;
    let mut millis = u64::try_from(second_delta).ok()?.checked_mul(1_000)?;
    let nanos_delta = if later.nanoseconds() >= earlier.nanoseconds() {
        later.nanoseconds() - earlier.nanoseconds()
    } else {
        millis = millis.checked_sub(1_000)?;
        1_000_000_000 + later.nanoseconds() - earlier.nanoseconds()
    };
    millis.checked_add(u64::from(nanos_delta / 1_000_000))
}

/// 行情查询。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MarketDataQuery {
    pub venue_id: VenueId,
    pub instrument_id: InstrumentId,
}

impl MarketDataQuery {
    pub fn new(venue_id: VenueId, instrument_id: InstrumentId) -> Self {
        Self {
            venue_id,
            instrument_id,
        }
    }
}

/// 单档订单簿深度。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrderBookLevel {
    pub price: Price,
    pub quantity: Quantity,
}

/// 行情报价快照。
///
/// 中文说明：报价只描述观察到的市场状态，不包含下单方向、订单参数或执行意图。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarketQuote {
    pub venue_id: VenueId,
    pub instrument_id: InstrumentId,
    pub last_price: Option<Price>,
    pub best_bid: Option<Price>,
    pub best_ask: Option<Price>,
    pub mark_price: Option<Price>,
    pub index_price: Option<Price>,
    pub bid_size: Option<Quantity>,
    pub ask_size: Option<Quantity>,
    pub source_sequence: Option<String>,
    pub source_event_id: Option<String>,
    pub freshness: DataFreshness,
}

/// 订单簿快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderBookSnapshot {
    pub venue_id: VenueId,
    pub instrument_id: InstrumentId,
    pub bids: Vec<OrderBookLevel>,
    pub asks: Vec<OrderBookLevel>,
    pub source_sequence: Option<String>,
    pub source_event_id: Option<String>,
    pub freshness: DataFreshness,
}

/// 行情只读接口。
pub trait MarketDataReader {
    /// 读取最近报价。
    fn latest_quote(&self, query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>>;

    /// 读取订单簿快照。
    fn order_book(&self, query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>>;
}

/// 余额查询。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BalanceQuery {
    pub venue_id: VenueId,
    pub account_id: Option<AccountId>,
    pub asset_id: Option<AssetId>,
}

impl BalanceQuery {
    pub fn new(venue_id: VenueId) -> Self {
        Self {
            venue_id,
            account_id: None,
            asset_id: None,
        }
    }

    pub fn for_account(mut self, account_id: AccountId) -> Self {
        self.account_id = Some(account_id);
        self
    }

    pub fn for_asset(mut self, asset_id: AssetId) -> Self {
        self.asset_id = Some(asset_id);
        self
    }
}

/// 场所只读余额快照。
///
/// 中文说明：余额快照只表达场所报告的账户状态，不提供划转、冻结、释放或修改接口。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VenueBalance {
    pub venue_id: VenueId,
    pub account_id: AccountId,
    pub asset_id: AssetId,
    pub free: Amount,
    pub locked: Amount,
    pub reserved: Amount,
    pub pending: Amount,
    pub borrowed: Amount,
    pub lent: Amount,
    pub unsettled: Amount,
    pub source_event_id: Option<String>,
    pub freshness: DataFreshness,
}

/// 余额只读接口。
pub trait BalanceReader {
    fn balances(&self, query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>>;
}

/// 仓位查询。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PositionQuery {
    pub venue_id: VenueId,
    pub account_id: Option<AccountId>,
    pub instrument_id: Option<InstrumentId>,
}

impl PositionQuery {
    pub fn new(venue_id: VenueId) -> Self {
        Self {
            venue_id,
            account_id: None,
            instrument_id: None,
        }
    }

    pub fn for_account(mut self, account_id: AccountId) -> Self {
        self.account_id = Some(account_id);
        self
    }

    pub fn for_instrument(mut self, instrument_id: InstrumentId) -> Self {
        self.instrument_id = Some(instrument_id);
        self
    }
}

/// 场所只读仓位快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VenuePosition {
    pub venue_id: VenueId,
    pub position_id: Option<PositionId>,
    pub account_id: AccountId,
    pub instrument_id: InstrumentId,
    pub quantity: Decimal,
    pub entry_price: Option<Price>,
    pub mark_price: Price,
    pub unrealized_pnl: Pnl,
    pub liquidation_price: Option<Price>,
    pub source_event_id: Option<String>,
    pub freshness: DataFreshness,
}

/// 仓位只读接口。
pub trait PositionReader {
    fn positions(&self, query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>>;
}

/// 工具信息查询。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InstrumentInfoQuery {
    pub venue_id: VenueId,
    pub instrument_id: Option<InstrumentId>,
}

impl InstrumentInfoQuery {
    pub fn new(venue_id: VenueId) -> Self {
        Self {
            venue_id,
            instrument_id: None,
        }
    }

    pub fn for_instrument(mut self, instrument_id: InstrumentId) -> Self {
        self.instrument_id = Some(instrument_id);
        self
    }
}

/// 场所只读工具信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstrumentInfo {
    pub venue_id: VenueId,
    pub instrument_id: InstrumentId,
    pub kind: InstrumentKind,
    pub base_asset_id: Option<AssetId>,
    pub quote_asset_id: Option<AssetId>,
    pub settlement_asset_id: AssetId,
    pub margin_asset_id: Option<AssetId>,
    pub tick_size: Option<Price>,
    pub lot_size: Option<Quantity>,
    pub contract_multiplier: Option<Decimal>,
    pub is_active: bool,
    pub source_event_id: Option<String>,
    pub freshness: DataFreshness,
}

/// 工具信息只读接口。
pub trait InstrumentInfoReader {
    fn instruments(&self, query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>>;
}

/// 场所连接状态。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum VenueConnectionStatus {
    Connected,
    Reconnecting,
    Disconnected,
    Unknown,
}

impl VenueConnectionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Reconnecting => "Reconnecting",
            Self::Disconnected => "Disconnected",
            Self::Unknown => "Unknown",
        }
    }
}

/// 场所健康状态。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum VenueHealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

impl VenueHealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "Healthy",
            Self::Degraded => "Degraded",
            Self::Unhealthy => "Unhealthy",
            Self::Unknown => "Unknown",
        }
    }
}

/// 限频只读快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitSnapshot {
    pub limit: u64,
    pub remaining: Option<u64>,
    pub window_ms: u64,
    pub resets_at: Option<UtcTimestamp>,
}

/// 场所健康只读快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VenueHealthSnapshot {
    pub venue_id: VenueId,
    pub status: VenueHealthStatus,
    pub connection: VenueConnectionStatus,
    pub reason_codes: Vec<String>,
    pub rate_limit: Option<RateLimitSnapshot>,
    pub source_event_id: Option<String>,
    pub freshness: DataFreshness,
}

/// 健康状态只读接口。
pub trait VenueHealthReader {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot>;
}

/// 场所只读适配器聚合接口。
///
/// 中文说明：该 trait 聚合五类只读能力，故意不包含提交订单、撤单、转账、签名、
/// 改保证金或任何会改变账户状态的方法。
pub trait VenueReadAdapter:
    MarketDataReader + BalanceReader + PositionReader + InstrumentInfoReader + VenueHealthReader
{
    fn venue_id(&self) -> &VenueId;
}

/// 行情传输方式。
///
/// 中文说明：REST 快照用于启动、补洞和重建；WebSocket 流用于低延迟更新。
/// 两者都只能产生只读事实，不能表达下单、撤单、转账或签名。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MarketDataTransport {
    RestSnapshot,
    WebSocketStream,
}

impl MarketDataTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RestSnapshot => "RestSnapshot",
            Self::WebSocketStream => "WebSocketStream",
        }
    }
}

/// REST + WSS 混合行情状态。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HybridMarketDataStatus {
    AwaitingRestSnapshot,
    SnapshotReady,
    Streaming,
    Reconnecting,
    Halted,
}

impl HybridMarketDataStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingRestSnapshot => "AwaitingRestSnapshot",
            Self::SnapshotReady => "SnapshotReady",
            Self::Streaming => "Streaming",
            Self::Reconnecting => "Reconnecting",
            Self::Halted => "Halted",
        }
    }
}

/// 标准化后的 WebSocket 报价更新。
///
/// 中文说明：真实交易所的推送字段可以不同，但进入核心前必须先归一化成
/// 单调连续的 `source_sequence`。如果适配器无法证明没有缺口，必须生成
/// `WssGapDetected` 或返回未知外部状态，不能继续沿用可能残缺的行情。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WssQuoteUpdate {
    pub venue_id: VenueId,
    pub instrument_id: InstrumentId,
    pub last_price: Option<Price>,
    pub best_bid: Option<Price>,
    pub best_ask: Option<Price>,
    pub mark_price: Option<Price>,
    pub index_price: Option<Price>,
    pub bid_size: Option<Quantity>,
    pub ask_size: Option<Quantity>,
    pub source_sequence: u64,
    pub source_event_id: Option<String>,
    pub observed_at: UtcTimestamp,
    pub ingested_at: UtcTimestamp,
}

/// 混合行情输入事件。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HybridMarketDataInput {
    /// REST 启动快照或断线后重建快照。
    RestSnapshot { quote: MarketQuote },
    /// WSS 连接已经建立。
    WssConnected {
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
    },
    /// WSS 报价增量或 top-of-book 快照更新。
    WssQuote { update: WssQuoteUpdate },
    /// WSS 心跳。
    WssHeartbeat {
        source_sequence: Option<u64>,
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
    },
    /// WSS 连接断开。
    WssDisconnected {
        reason: String,
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
    },
    /// 适配器已经发现序号缺口。
    WssGapDetected {
        expected_sequence: Option<u64>,
        observed_sequence: Option<u64>,
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        detail: String,
    },
}

/// 混合行情处理结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HybridMarketDataUpdate {
    pub status: HybridMarketDataStatus,
    pub transport: MarketDataTransport,
    pub quote: Option<MarketQuote>,
    pub health: VenueHealthSnapshot,
    pub reason_codes: Vec<String>,
    pub fail_closed: bool,
}

/// REST + WSS 混合行情协调器。
///
/// 中文说明：该结构只维护只读行情状态。它要求先用 REST 快照建立基线，再接受
/// WSS 更新；断线、乱序、缺口和未知状态都会进入不可继续交易的健康状态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestWssMarketDataCoordinator {
    venue_id: VenueId,
    instrument_id: InstrumentId,
    max_age_ms: u64,
    status: HybridMarketDataStatus,
    latest_quote: Option<MarketQuote>,
    health: VenueHealthSnapshot,
    last_wss_sequence: Option<u64>,
}

impl RestWssMarketDataCoordinator {
    pub fn new(
        venue_id: VenueId,
        instrument_id: InstrumentId,
        started_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let freshness = DataFreshness::new(started_at, started_at, max_age_ms)?;
        Ok(Self {
            venue_id: venue_id.clone(),
            instrument_id,
            max_age_ms,
            status: HybridMarketDataStatus::AwaitingRestSnapshot,
            latest_quote: None,
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Unknown,
                connection: VenueConnectionStatus::Unknown,
                reason_codes: vec!["REST_SNAPSHOT_REQUIRED".to_owned()],
                rate_limit: None,
                source_event_id: None,
                freshness,
            },
            last_wss_sequence: None,
        })
    }

    pub fn status(&self) -> HybridMarketDataStatus {
        self.status
    }

    pub fn latest_quote_snapshot(&self) -> Option<&MarketQuote> {
        self.latest_quote.as_ref()
    }

    pub fn last_wss_sequence(&self) -> Option<u64> {
        self.last_wss_sequence
    }

    pub fn apply(
        &mut self,
        input: HybridMarketDataInput,
    ) -> VenueDataResult<HybridMarketDataUpdate> {
        match input {
            HybridMarketDataInput::RestSnapshot { quote } => self.apply_rest_snapshot(quote),
            HybridMarketDataInput::WssConnected {
                occurred_at,
                ingested_at,
            } => self.apply_wss_connected(occurred_at, ingested_at),
            HybridMarketDataInput::WssQuote { update } => self.apply_wss_quote(update),
            HybridMarketDataInput::WssHeartbeat {
                source_sequence,
                occurred_at,
                ingested_at,
            } => self.apply_wss_heartbeat(source_sequence, occurred_at, ingested_at),
            HybridMarketDataInput::WssDisconnected {
                reason,
                occurred_at,
                ingested_at,
            } => self.apply_wss_disconnected(reason, occurred_at, ingested_at),
            HybridMarketDataInput::WssGapDetected {
                expected_sequence,
                observed_sequence,
                occurred_at,
                ingested_at,
                detail,
            } => self.apply_wss_gap(
                expected_sequence,
                observed_sequence,
                occurred_at,
                ingested_at,
                detail,
            ),
        }
    }

    fn apply_rest_snapshot(
        &mut self,
        quote: MarketQuote,
    ) -> VenueDataResult<HybridMarketDataUpdate> {
        self.validate_quote_identity(&quote)?;
        self.last_wss_sequence = quote
            .source_sequence
            .as_deref()
            .and_then(|sequence| sequence.parse::<u64>().ok());
        self.status = HybridMarketDataStatus::SnapshotReady;
        self.latest_quote = Some(quote.clone());
        self.set_health_from_freshness(
            VenueConnectionStatus::Connected,
            quote.freshness,
            quote.source_event_id.clone(),
            vec!["REST_SNAPSHOT_READY".to_owned()],
        );
        Ok(self.update(
            MarketDataTransport::RestSnapshot,
            self.health.status != VenueHealthStatus::Healthy,
        ))
    }

    fn apply_wss_connected(
        &mut self,
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<HybridMarketDataUpdate> {
        let freshness = DataFreshness::new(occurred_at, ingested_at, self.max_age_ms)?;
        if self.latest_quote.is_none() {
            self.status = HybridMarketDataStatus::AwaitingRestSnapshot;
            self.set_health(
                VenueHealthStatus::Degraded,
                VenueConnectionStatus::Connected,
                freshness,
                None,
                vec!["REST_SNAPSHOT_REQUIRED".to_owned()],
            );
            return Ok(self.update(MarketDataTransport::WebSocketStream, true));
        }

        self.status = HybridMarketDataStatus::Streaming;
        self.set_health_from_freshness(
            VenueConnectionStatus::Connected,
            freshness,
            self.latest_quote
                .as_ref()
                .and_then(|quote| quote.source_event_id.clone()),
            vec!["WSS_CONNECTED".to_owned()],
        );
        Ok(self.update(MarketDataTransport::WebSocketStream, false))
    }

    fn apply_wss_quote(
        &mut self,
        update: WssQuoteUpdate,
    ) -> VenueDataResult<HybridMarketDataUpdate> {
        if self.latest_quote.is_none() {
            let freshness =
                DataFreshness::new(update.observed_at, update.ingested_at, self.max_age_ms)?;
            self.mark_unknown_wss_state(
                freshness,
                update.source_event_id.clone(),
                "WSS_WITHOUT_REST_SNAPSHOT",
            );
            return Err(VenueDataError::UnknownExternalState {
                venue_id: self.venue_id.clone(),
                surface: ReadOnlySurface::MarketData,
                detail: "WSS quote arrived before REST snapshot bootstrap".to_owned(),
            });
        }
        if update.venue_id != self.venue_id || update.instrument_id != self.instrument_id {
            let freshness =
                DataFreshness::new(update.observed_at, update.ingested_at, self.max_age_ms)?;
            self.mark_unknown_wss_state(freshness, update.source_event_id, "WSS_SYMBOL_MISMATCH");
            return Err(VenueDataError::UnknownExternalState {
                venue_id: self.venue_id.clone(),
                surface: ReadOnlySurface::MarketData,
                detail: "WSS quote venue or instrument does not match configured stream".to_owned(),
            });
        }

        if let Some(previous) = self.last_wss_sequence {
            let expected = previous
                .checked_add(1)
                .ok_or(VenueDataError::UnknownExternalState {
                    venue_id: self.venue_id.clone(),
                    surface: ReadOnlySurface::MarketData,
                    detail: "WSS sequence overflow".to_owned(),
                })?;
            if update.source_sequence != expected {
                let freshness =
                    DataFreshness::new(update.observed_at, update.ingested_at, self.max_age_ms)?;
                self.mark_unknown_wss_state(
                    freshness,
                    update.source_event_id.clone(),
                    "WSS_SEQUENCE_GAP",
                );
                return Err(VenueDataError::UnknownExternalState {
                    venue_id: self.venue_id.clone(),
                    surface: ReadOnlySurface::MarketData,
                    detail: format!(
                        "WSS sequence gap expected `{expected}` observed `{}`",
                        update.source_sequence
                    ),
                });
            }
        }

        let freshness =
            DataFreshness::new(update.observed_at, update.ingested_at, self.max_age_ms)?;
        let quote = MarketQuote {
            venue_id: update.venue_id,
            instrument_id: update.instrument_id,
            last_price: update.last_price,
            best_bid: update.best_bid,
            best_ask: update.best_ask,
            mark_price: update.mark_price,
            index_price: update.index_price,
            bid_size: update.bid_size,
            ask_size: update.ask_size,
            source_sequence: Some(update.source_sequence.to_string()),
            source_event_id: update.source_event_id,
            freshness,
        };
        self.last_wss_sequence = Some(update.source_sequence);
        self.status = HybridMarketDataStatus::Streaming;
        self.latest_quote = Some(quote.clone());
        self.set_health_from_freshness(
            VenueConnectionStatus::Connected,
            freshness,
            quote.source_event_id.clone(),
            vec!["WSS_QUOTE_APPLIED".to_owned()],
        );
        Ok(self.update(MarketDataTransport::WebSocketStream, freshness.is_stale()))
    }

    fn apply_wss_heartbeat(
        &mut self,
        source_sequence: Option<u64>,
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<HybridMarketDataUpdate> {
        let freshness = DataFreshness::new(occurred_at, ingested_at, self.max_age_ms)?;
        if let (Some(previous), Some(observed)) = (self.last_wss_sequence, source_sequence) {
            if observed < previous {
                self.mark_unknown_wss_state(freshness, None, "WSS_HEARTBEAT_OUT_OF_ORDER");
                return Err(VenueDataError::UnknownExternalState {
                    venue_id: self.venue_id.clone(),
                    surface: ReadOnlySurface::MarketData,
                    detail: format!(
                        "WSS heartbeat sequence `{observed}` is older than current `{previous}`"
                    ),
                });
            }
        }
        if let Some(observed) = source_sequence {
            self.last_wss_sequence = Some(observed);
        }
        if self.latest_quote.is_none() {
            self.status = HybridMarketDataStatus::AwaitingRestSnapshot;
            self.set_health(
                VenueHealthStatus::Degraded,
                VenueConnectionStatus::Connected,
                freshness,
                None,
                vec![
                    "REST_SNAPSHOT_REQUIRED".to_owned(),
                    "WSS_HEARTBEAT".to_owned(),
                ],
            );
            return Ok(self.update(MarketDataTransport::WebSocketStream, true));
        }
        self.status = HybridMarketDataStatus::Streaming;
        self.set_health_from_freshness(
            VenueConnectionStatus::Connected,
            freshness,
            self.latest_quote
                .as_ref()
                .and_then(|quote| quote.source_event_id.clone()),
            vec!["WSS_HEARTBEAT".to_owned()],
        );
        Ok(self.update(MarketDataTransport::WebSocketStream, freshness.is_stale()))
    }

    fn apply_wss_disconnected(
        &mut self,
        reason: String,
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<HybridMarketDataUpdate> {
        let freshness = DataFreshness::new(occurred_at, ingested_at, self.max_age_ms)?;
        self.status = HybridMarketDataStatus::Reconnecting;
        self.set_health(
            VenueHealthStatus::Unhealthy,
            VenueConnectionStatus::Disconnected,
            freshness,
            self.latest_quote
                .as_ref()
                .and_then(|quote| quote.source_event_id.clone()),
            vec!["WSS_DISCONNECTED".to_owned(), reason],
        );
        Ok(self.update(MarketDataTransport::WebSocketStream, true))
    }

    fn apply_wss_gap(
        &mut self,
        expected_sequence: Option<u64>,
        observed_sequence: Option<u64>,
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        detail: String,
    ) -> VenueDataResult<HybridMarketDataUpdate> {
        let freshness = DataFreshness::new(occurred_at, ingested_at, self.max_age_ms)?;
        let mut reason_codes = vec!["WSS_SEQUENCE_GAP".to_owned()];
        if let Some(expected) = expected_sequence {
            reason_codes.push(format!("expected={expected}"));
        }
        if let Some(observed) = observed_sequence {
            reason_codes.push(format!("observed={observed}"));
        }
        if !detail.is_empty() {
            reason_codes.push(detail);
        }
        self.status = HybridMarketDataStatus::Halted;
        self.set_health(
            VenueHealthStatus::Unhealthy,
            VenueConnectionStatus::Unknown,
            freshness,
            self.latest_quote
                .as_ref()
                .and_then(|quote| quote.source_event_id.clone()),
            reason_codes,
        );
        Ok(self.update(MarketDataTransport::WebSocketStream, true))
    }

    fn validate_quote_identity(&self, quote: &MarketQuote) -> VenueDataResult<()> {
        if quote.venue_id != self.venue_id || quote.instrument_id != self.instrument_id {
            return Err(VenueDataError::UnknownExternalState {
                venue_id: self.venue_id.clone(),
                surface: ReadOnlySurface::MarketData,
                detail: "REST snapshot venue or instrument does not match configured stream"
                    .to_owned(),
            });
        }
        Ok(())
    }

    fn set_health_from_freshness(
        &mut self,
        connection: VenueConnectionStatus,
        freshness: DataFreshness,
        source_event_id: Option<String>,
        mut reason_codes: Vec<String>,
    ) {
        let status = if freshness.is_stale() {
            reason_codes.push("DATA_STALE".to_owned());
            VenueHealthStatus::Degraded
        } else {
            VenueHealthStatus::Healthy
        };
        self.set_health(status, connection, freshness, source_event_id, reason_codes);
    }

    fn set_health(
        &mut self,
        status: VenueHealthStatus,
        connection: VenueConnectionStatus,
        freshness: DataFreshness,
        source_event_id: Option<String>,
        reason_codes: Vec<String>,
    ) {
        self.health = VenueHealthSnapshot {
            venue_id: self.venue_id.clone(),
            status,
            connection,
            reason_codes,
            rate_limit: None,
            source_event_id,
            freshness,
        };
    }

    fn mark_unknown_wss_state(
        &mut self,
        freshness: DataFreshness,
        source_event_id: Option<String>,
        reason_code: &str,
    ) {
        self.status = HybridMarketDataStatus::Halted;
        self.set_health(
            VenueHealthStatus::Unhealthy,
            VenueConnectionStatus::Unknown,
            freshness,
            source_event_id,
            vec![reason_code.to_owned()],
        );
    }

    fn update(&self, transport: MarketDataTransport, fail_closed: bool) -> HybridMarketDataUpdate {
        HybridMarketDataUpdate {
            status: self.status,
            transport,
            quote: self.latest_quote.clone(),
            health: self.health.clone(),
            reason_codes: self.health.reason_codes.clone(),
            fail_closed,
        }
    }
}

impl MarketDataReader for RestWssMarketDataCoordinator {
    fn latest_quote(&self, query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
        Ok(
            (query.venue_id == self.venue_id && query.instrument_id == self.instrument_id)
                .then(|| self.latest_quote.clone())
                .flatten(),
        )
    }

    fn order_book(&self, query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>> {
        if query.venue_id != self.venue_id || query.instrument_id != self.instrument_id {
            return Ok(None);
        }
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "REST+WSS coordinator currently tracks top-of-book quote, not full depth"
                .to_owned(),
        })
    }
}

impl VenueHealthReader for RestWssMarketDataCoordinator {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
        if venue_id == &self.venue_id {
            Ok(self.health.clone())
        } else {
            Err(VenueDataError::DataUnavailable {
                venue_id: venue_id.clone(),
                surface: ReadOnlySurface::VenueHealth,
                reason: "coordinator only tracks its configured venue".to_owned(),
            })
        }
    }
}

/// Binance 公共 24hr ticker 只读工具配置。
///
/// 中文说明：该配置只描述公开市场数据如何映射到平台工具，不包含 API key、
/// 账户、签名或任何可变执行能力。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePublicInstrument {
    pub symbol: String,
    pub instrument_id: InstrumentId,
    pub base_asset_id: AssetId,
    pub quote_asset_id: AssetId,
    pub settlement_asset_id: AssetId,
    pub tick_size: Option<Price>,
    pub lot_size: Option<Quantity>,
}

impl BinancePublicInstrument {
    pub fn new(
        symbol: impl Into<String>,
        instrument_id: InstrumentId,
        base_asset_id: AssetId,
        quote_asset_id: AssetId,
        settlement_asset_id: AssetId,
    ) -> VenueDataResult<Self> {
        let symbol = symbol.into();
        validate_binance_symbol(&symbol)?;
        Ok(Self {
            symbol,
            instrument_id,
            base_asset_id,
            quote_asset_id,
            settlement_asset_id,
            tick_size: None,
            lot_size: None,
        })
    }

    pub fn with_tick_size(mut self, tick_size: Price) -> Self {
        self.tick_size = Some(tick_size);
        self
    }

    pub fn with_lot_size(mut self, lot_size: Quantity) -> Self {
        self.lot_size = Some(lot_size);
        self
    }
}

/// 原始响应到标准化事件的输出批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePublicTickerBatch {
    pub raw_event: NormalizedEvent,
    pub normalized_event: NormalizedEvent,
    pub quote: MarketQuote,
}

/// Binance 公共市场类型。
///
/// 中文说明：该枚举只用于区分公开现货行情和公开 USDⓈ-M 永续行情，不代表
/// 账户权限，也不包含任何交易动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinancePublicMarket {
    Spot,
    UsdmPerpetual,
}

impl BinancePublicMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spot => "Spot",
            Self::UsdmPerpetual => "UsdmPerpetual",
        }
    }

    fn basis_role(self) -> &'static str {
        match self {
            Self::Spot => "Spot",
            Self::UsdmPerpetual => "Perp",
        }
    }

    fn event_scope(self) -> &'static str {
        match self {
            Self::Spot => "spot",
            Self::UsdmPerpetual => "usdm-perp",
        }
    }

    fn instrument_kind(self) -> InstrumentKind {
        match self {
            Self::Spot => InstrumentKind::SpotPair,
            Self::UsdmPerpetual => InstrumentKind::PerpetualSwap,
        }
    }
}

/// Binance bookTicker 原始响应到标准化事件的输出批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePublicBookTickerBatch {
    pub raw_event: NormalizedEvent,
    pub normalized_event: NormalizedEvent,
    pub quote: MarketQuote,
}

/// Bybit 公共 V5 ticker 只读工具配置。
///
/// 中文说明：该配置只描述公开市场数据如何映射到平台工具，不包含 API key、
/// 账户、签名或任何可变执行能力。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitPublicInstrument {
    pub symbol: String,
    pub instrument_id: InstrumentId,
    pub base_asset_id: AssetId,
    pub quote_asset_id: AssetId,
    pub settlement_asset_id: AssetId,
    pub tick_size: Option<Price>,
    pub lot_size: Option<Quantity>,
}

impl BybitPublicInstrument {
    pub fn new(
        symbol: impl Into<String>,
        instrument_id: InstrumentId,
        base_asset_id: AssetId,
        quote_asset_id: AssetId,
        settlement_asset_id: AssetId,
    ) -> VenueDataResult<Self> {
        let symbol = symbol.into();
        validate_bybit_symbol(&symbol)?;
        Ok(Self {
            symbol,
            instrument_id,
            base_asset_id,
            quote_asset_id,
            settlement_asset_id,
            tick_size: None,
            lot_size: None,
        })
    }

    pub fn with_tick_size(mut self, tick_size: Price) -> Self {
        self.tick_size = Some(tick_size);
        self
    }

    pub fn with_lot_size(mut self, lot_size: Quantity) -> Self {
        self.lot_size = Some(lot_size);
        self
    }
}

/// Bybit 公共市场类型。
///
/// 中文说明：该枚举只用于区分公开现货行情和公开 USDT 线性永续行情，不代表
/// 账户权限，也不包含任何交易动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BybitPublicMarket {
    Spot,
    LinearPerpetual,
}

impl BybitPublicMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spot => "Spot",
            Self::LinearPerpetual => "LinearPerp",
        }
    }

    fn category(self) -> &'static str {
        match self {
            Self::Spot => "spot",
            Self::LinearPerpetual => "linear",
        }
    }

    fn basis_role(self) -> &'static str {
        match self {
            Self::Spot => "Spot",
            Self::LinearPerpetual => "Perp",
        }
    }

    fn event_scope(self) -> &'static str {
        match self {
            Self::Spot => "spot",
            Self::LinearPerpetual => "linear-perp",
        }
    }

    fn instrument_kind(self) -> InstrumentKind {
        match self {
            Self::Spot => InstrumentKind::SpotPair,
            Self::LinearPerpetual => InstrumentKind::PerpetualSwap,
        }
    }
}

/// Bybit 公共 ticker 原始响应到标准化事件的输出批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitPublicTickerBatch {
    pub raw_event: NormalizedEvent,
    pub normalized_event: NormalizedEvent,
    pub quote: MarketQuote,
}

/// Bybit 线性永续 premium index 标准化事件输出批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitLinearPremiumIndexBatch {
    pub raw_event: NormalizedEvent,
    pub normalized_event: NormalizedEvent,
}

const BINANCE_SPOT_PUBLIC_WSS_BASE_URL: &str = "wss://data-stream.binance.vision/ws";
const BINANCE_USDM_PUBLIC_WSS_BASE_URL: &str = "wss://fstream.binance.com/public/ws";

/// Binance 公开 `bookTicker` WSS 客户端配置。
///
/// 中文说明：该配置只包含公开行情 WSS 地址和工具映射，不包含 API key、listenKey、
/// 账户、签名或任何可变执行权限。Spot 默认使用 Binance 的 market-data-only
/// `data-stream.binance.vision`；USD-M bookTicker 默认使用 `/public` 路由。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePublicWssBookTickerConfig {
    pub venue_id: VenueId,
    pub instrument: BinancePublicInstrument,
    pub market: BinancePublicMarket,
    pub endpoint_base_url: String,
    pub max_age_ms: u64,
}

impl BinancePublicWssBookTickerConfig {
    pub fn new(
        venue_id: VenueId,
        instrument: BinancePublicInstrument,
        market: BinancePublicMarket,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let endpoint_base_url = match market {
            BinancePublicMarket::Spot => BINANCE_SPOT_PUBLIC_WSS_BASE_URL,
            BinancePublicMarket::UsdmPerpetual => BINANCE_USDM_PUBLIC_WSS_BASE_URL,
        };
        Self::with_endpoint_base_url(venue_id, instrument, market, endpoint_base_url, max_age_ms)
    }

    pub fn with_endpoint_base_url(
        venue_id: VenueId,
        instrument: BinancePublicInstrument,
        market: BinancePublicMarket,
        endpoint_base_url: impl Into<String>,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let endpoint_base_url = endpoint_base_url.into();
        if !endpoint_base_url.starts_with("wss://") {
            return Err(VenueDataError::InvalidQuery {
                field: "binance.wss.endpoint_base_url",
                reason: "Binance public stream endpoint must use wss://",
            });
        }
        Ok(Self {
            venue_id,
            instrument,
            market,
            endpoint_base_url: endpoint_base_url.trim_end_matches('/').to_owned(),
            max_age_ms,
        })
    }

    pub fn stream_name(&self) -> String {
        format!("{}@bookTicker", self.instrument.symbol.to_ascii_lowercase())
    }

    pub fn stream_url(&self) -> String {
        format!("{}/{}", self.endpoint_base_url, self.stream_name())
    }
}

/// Binance 公开 `bookTicker` WSS 客户端。
///
/// 中文说明：客户端把真实 WSS 文本消息转换成 `RestWssMarketDataCoordinator`
/// 可消费的只读事件。调用方必须先用 `apply_rest_snapshot` 注入 REST 快照；
/// WSS 只负责快照后的低延迟更新。断线、重复或倒退的 updateId 会 fail closed，
/// 调用方随后应再次走 REST 快照路径补洞或重建。
#[derive(Debug)]
pub struct BinancePublicWssBookTickerClient {
    config: BinancePublicWssBookTickerConfig,
    coordinator: RestWssMarketDataCoordinator,
    local_sequence: u64,
    last_exchange_update_id: Option<u64>,
}

/// Binance 公开 WSS 文本流客户端。
///
/// 中文说明：该客户端只负责连接公开行情 WSS 并把文本消息交给调用方，不解析账户、
/// 不下单、不撤单、不转账、不签名。多 symbol 的状态管理由上层 runtime 完成。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePublicWssTextStreamClient {
    venue_id: VenueId,
    stream_url: String,
}

/// Bybit 公开 WSS 文本流客户端。
///
/// 中文说明：Bybit V5 公开 WSS 需要连接后发送 `subscribe` 请求；该客户端只发送
/// 公开行情订阅参数，不读取账户、不签名、不下单、不撤单、不转账。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitPublicWssTextStreamClient {
    venue_id: VenueId,
    stream_url: String,
}

/// 通用公开 WSS 文本流客户端。
///
/// 中文说明：用于需要连接后发送公开订阅 JSON 的交易所行情流。它只处理公开
/// 行情文本消息，不读取账户、不签名、不下单、不撤单、不转账。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicJsonWssTextStreamClient {
    venue_id: VenueId,
    stream_url: String,
    label: &'static str,
}

impl BinancePublicWssTextStreamClient {
    pub fn new(venue_id: VenueId, stream_url: impl Into<String>) -> VenueDataResult<Self> {
        let stream_url = stream_url.into();
        if !stream_url.starts_with("wss://") {
            return Err(VenueDataError::InvalidQuery {
                field: "binance.wss.stream_url",
                reason: "Binance public stream URL must use wss://",
            });
        }
        Ok(Self {
            venue_id,
            stream_url,
        })
    }

    pub fn stream_url(&self) -> &str {
        &self.stream_url
    }

    /// 连接真实 Binance WSS 并按文本消息回调。
    ///
    /// 中文说明：观察者返回 `false` 时停止读取，调用方随后应按本地状态决定是否
    /// fail closed 或 REST rebuild。连接层错误仍会返回分类后的只读外部错误。
    pub fn read_live_text_messages_observed<F>(
        &self,
        max_text_messages: usize,
        mut observer: F,
    ) -> VenueDataResult<()>
    where
        F: FnMut(&str, UtcTimestamp) -> bool,
    {
        if max_text_messages == 0 {
            return Err(VenueDataError::InvalidQuery {
                field: "binance.wss.max_text_messages",
                reason: "must be greater than zero",
            });
        }

        let (mut socket, _response) =
            tungstenite::connect(self.stream_url.as_str()).map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::Disconnected,
                    format!(
                        "cannot connect Binance public WSS `{}`: {error}",
                        self.stream_url
                    ),
                ))
            })?;
        let mut text_messages = 0_usize;
        while text_messages < max_text_messages {
            let message = socket.read().map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::Disconnected,
                    format!("Binance public WSS read failed: {error}"),
                ))
            })?;
            match message {
                tungstenite::Message::Text(text) => {
                    let ingested_at = current_utc_timestamp(&self.venue_id)?;
                    text_messages += 1;
                    if !observer(&text, ingested_at) {
                        break;
                    }
                }
                tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => {
                    socket.flush().map_err(|error| {
                        VenueDataError::External(ClassifiedExternalError::new(
                            self.venue_id.clone(),
                            ReadOnlySurface::MarketData,
                            ExternalErrorClass::Disconnected,
                            format!("Binance public WSS heartbeat flush failed: {error}"),
                        ))
                    })?;
                }
                tungstenite::Message::Close(_) => break,
                tungstenite::Message::Binary(_) | tungstenite::Message::Frame(_) => break,
            }
        }
        Ok(())
    }
}

impl BybitPublicWssTextStreamClient {
    pub fn new(venue_id: VenueId, stream_url: impl Into<String>) -> VenueDataResult<Self> {
        let stream_url = stream_url.into();
        if !stream_url.starts_with("wss://") {
            return Err(VenueDataError::InvalidQuery {
                field: "bybit.wss.stream_url",
                reason: "Bybit public stream URL must use wss://",
            });
        }
        Ok(Self {
            venue_id,
            stream_url,
        })
    }

    pub fn stream_url(&self) -> &str {
        &self.stream_url
    }

    /// 连接真实 Bybit V5 公开 WSS，发送订阅请求，并按文本消息回调。
    pub fn read_live_text_messages_observed<F>(
        &self,
        subscribe_args: &[String],
        max_text_messages: usize,
        mut observer: F,
    ) -> VenueDataResult<()>
    where
        F: FnMut(&str, UtcTimestamp) -> bool,
    {
        if subscribe_args.is_empty() {
            return Err(VenueDataError::InvalidQuery {
                field: "bybit.wss.subscribe_args",
                reason: "must contain at least one public topic",
            });
        }
        if max_text_messages == 0 {
            return Err(VenueDataError::InvalidQuery {
                field: "bybit.wss.max_text_messages",
                reason: "must be greater than zero",
            });
        }

        let (mut socket, _response) =
            tungstenite::connect(self.stream_url.as_str()).map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::Disconnected,
                    format!(
                        "cannot connect Bybit public WSS `{}`: {error}",
                        self.stream_url
                    ),
                ))
            })?;
        let subscribe_payloads = bybit_public_wss_subscribe_payloads(subscribe_args);
        for (index, subscribe) in subscribe_payloads.iter().enumerate() {
            socket
                .send(tungstenite::Message::Text(subscribe.to_owned()))
                .map_err(|error| {
                    VenueDataError::External(ClassifiedExternalError::new(
                        self.venue_id.clone(),
                        ReadOnlySurface::MarketData,
                        ExternalErrorClass::Disconnected,
                        format!("Bybit public WSS subscribe send failed: {error}"),
                    ))
                })?;
            if index + 1 < subscribe_payloads.len() {
                std::thread::sleep(Duration::from_millis(120));
            }
        }

        let mut text_messages = 0_usize;
        while text_messages < max_text_messages {
            let message = socket.read().map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::Disconnected,
                    format!("Bybit public WSS read failed: {error}"),
                ))
            })?;
            match message {
                tungstenite::Message::Text(text) => {
                    let ingested_at = current_utc_timestamp(&self.venue_id)?;
                    text_messages += 1;
                    if !observer(&text, ingested_at) {
                        break;
                    }
                }
                tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => {
                    socket.flush().map_err(|error| {
                        VenueDataError::External(ClassifiedExternalError::new(
                            self.venue_id.clone(),
                            ReadOnlySurface::MarketData,
                            ExternalErrorClass::Disconnected,
                            format!("Bybit public WSS heartbeat flush failed: {error}"),
                        ))
                    })?;
                }
                tungstenite::Message::Close(_) => break,
                tungstenite::Message::Binary(_) | tungstenite::Message::Frame(_) => break,
            }
        }
        Ok(())
    }
}

impl PublicJsonWssTextStreamClient {
    pub fn new(
        venue_id: VenueId,
        stream_url: impl Into<String>,
        label: &'static str,
    ) -> VenueDataResult<Self> {
        let stream_url = stream_url.into();
        if !stream_url.starts_with("wss://") {
            return Err(VenueDataError::InvalidQuery {
                field: "public_json.wss.stream_url",
                reason: "public WSS stream URL must use wss://",
            });
        }
        Ok(Self {
            venue_id,
            stream_url,
            label,
        })
    }

    pub fn stream_url(&self) -> &str {
        &self.stream_url
    }

    /// 连接真实公开 WSS，发送订阅请求，并按文本消息回调。
    pub fn read_live_text_messages_observed<F>(
        &self,
        subscribe_payload: &str,
        max_text_messages: usize,
        observer: F,
    ) -> VenueDataResult<()>
    where
        F: FnMut(&str, UtcTimestamp) -> bool,
    {
        self.read_live_text_messages_observed_many(
            &[subscribe_payload.to_owned()],
            max_text_messages,
            observer,
        )
    }

    /// 连接真实公开 WSS，发送多条订阅请求，并按文本消息回调。
    ///
    /// 中文说明：部分交易所（例如 Hyperliquid）要求每个公开频道独立发送 subscribe
    /// 消息；该入口仍只处理公开行情文本，不读取账户、不签名、不提交可变操作。
    pub fn read_live_text_messages_observed_many<F>(
        &self,
        subscribe_payloads: &[String],
        max_text_messages: usize,
        mut observer: F,
    ) -> VenueDataResult<()>
    where
        F: FnMut(&str, UtcTimestamp) -> bool,
    {
        if subscribe_payloads.is_empty()
            || subscribe_payloads
                .iter()
                .any(|payload| payload.trim().is_empty())
        {
            return Err(VenueDataError::InvalidQuery {
                field: "public_json.wss.subscribe_payloads",
                reason: "must contain at least one non-empty public subscribe payload",
            });
        }
        self.read_live_text_messages_after_connect(
            subscribe_payloads,
            max_text_messages,
            &mut observer,
        )
    }

    /// 连接真实公开 WSS，不发送订阅请求，并按文本消息回调。
    ///
    /// 中文说明：用于 Aster/Binance 这类通过 URL path 选择公开 stream 的只读行情流。
    pub fn read_live_text_messages_without_subscribe_observed<F>(
        &self,
        max_text_messages: usize,
        mut observer: F,
    ) -> VenueDataResult<()>
    where
        F: FnMut(&str, UtcTimestamp) -> bool,
    {
        self.read_live_text_messages_after_connect(&[], max_text_messages, &mut observer)
    }

    fn read_live_text_messages_after_connect<F>(
        &self,
        subscribe_payloads: &[String],
        max_text_messages: usize,
        observer: &mut F,
    ) -> VenueDataResult<()>
    where
        F: FnMut(&str, UtcTimestamp) -> bool,
    {
        if max_text_messages == 0 {
            return Err(VenueDataError::InvalidQuery {
                field: "public_json.wss.max_text_messages",
                reason: "must be greater than zero",
            });
        }
        let (mut socket, _response) =
            tungstenite::connect(self.stream_url.as_str()).map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::Disconnected,
                    format!(
                        "cannot connect {} public WSS `{}`: {error}",
                        self.label, self.stream_url
                    ),
                ))
            })?;
        for (index, subscribe_payload) in subscribe_payloads.iter().enumerate() {
            socket
                .send(tungstenite::Message::Text(subscribe_payload.to_owned()))
                .map_err(|error| {
                    VenueDataError::External(ClassifiedExternalError::new(
                        self.venue_id.clone(),
                        ReadOnlySurface::MarketData,
                        ExternalErrorClass::Disconnected,
                        format!("{} public WSS subscribe send failed: {error}", self.label),
                    ))
                })?;
            if index + 1 < subscribe_payloads.len() {
                std::thread::sleep(Duration::from_millis(120));
            }
        }

        let mut text_messages = 0_usize;
        while text_messages < max_text_messages {
            let message = socket.read().map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::Disconnected,
                    format!("{} public WSS read failed: {error}", self.label),
                ))
            })?;
            match message {
                tungstenite::Message::Text(text) => {
                    if text.trim().eq_ignore_ascii_case("ping") {
                        socket
                            .send(tungstenite::Message::Text("pong".to_owned()))
                            .map_err(|error| {
                                VenueDataError::External(ClassifiedExternalError::new(
                                    self.venue_id.clone(),
                                    ReadOnlySurface::MarketData,
                                    ExternalErrorClass::Disconnected,
                                    format!(
                                        "{} public WSS heartbeat response failed: {error}",
                                        self.label
                                    ),
                                ))
                            })?;
                        continue;
                    }
                    let ingested_at = current_utc_timestamp(&self.venue_id)?;
                    text_messages += 1;
                    if !observer(&text, ingested_at) {
                        break;
                    }
                }
                tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => {
                    socket.flush().map_err(|error| {
                        VenueDataError::External(ClassifiedExternalError::new(
                            self.venue_id.clone(),
                            ReadOnlySurface::MarketData,
                            ExternalErrorClass::Disconnected,
                            format!("{} public WSS heartbeat flush failed: {error}", self.label),
                        ))
                    })?;
                }
                tungstenite::Message::Close(_) => break,
                tungstenite::Message::Binary(_) | tungstenite::Message::Frame(_) => break,
            }
        }
        Ok(())
    }
}

impl BinancePublicWssBookTickerClient {
    pub fn new(
        config: BinancePublicWssBookTickerConfig,
        started_at: UtcTimestamp,
    ) -> VenueDataResult<Self> {
        let coordinator = RestWssMarketDataCoordinator::new(
            config.venue_id.clone(),
            config.instrument.instrument_id.clone(),
            started_at,
            config.max_age_ms,
        )?;
        Ok(Self {
            config,
            coordinator,
            local_sequence: 0,
            last_exchange_update_id: None,
        })
    }

    pub fn config(&self) -> &BinancePublicWssBookTickerConfig {
        &self.config
    }

    pub fn stream_url(&self) -> String {
        self.config.stream_url()
    }

    pub fn coordinator(&self) -> &RestWssMarketDataCoordinator {
        &self.coordinator
    }

    pub fn coordinator_mut(&mut self) -> &mut RestWssMarketDataCoordinator {
        &mut self.coordinator
    }

    pub fn last_exchange_update_id(&self) -> Option<u64> {
        self.last_exchange_update_id
    }

    /// 注入 REST 启动快照或补洞快照。
    ///
    /// 中文说明：Binance spot REST bookTicker 不提供 WSS updateId，因此协调器使用
    /// 本地连续序号做因果顺序；原始 REST 事件仍保留在 `source_event_id` 中。
    pub fn apply_rest_snapshot(
        &mut self,
        mut quote: MarketQuote,
    ) -> VenueDataResult<HybridMarketDataUpdate> {
        self.validate_quote_identity(&quote)?;
        quote.source_sequence = Some(self.next_local_sequence()?.to_string());
        self.last_exchange_update_id = None;
        self.coordinator
            .apply(HybridMarketDataInput::RestSnapshot { quote })
    }

    /// 解析并应用一条 Binance WSS 文本消息。
    pub fn apply_wss_text_message(
        &mut self,
        raw_json: &str,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<HybridMarketDataUpdate> {
        let raw = self.parse_wss_book_ticker(raw_json, ingested_at)?;
        let effective_ingested_at = binance_wss_effective_ingested_at(raw.observed_at, ingested_at);
        if let Some(previous) = self.last_exchange_update_id {
            if raw.update_id <= previous {
                return self.apply_non_advancing_update_gap(previous, &raw, effective_ingested_at);
            }
        }

        let source_sequence = self.next_local_sequence()?;
        let update = WssQuoteUpdate {
            venue_id: self.config.venue_id.clone(),
            instrument_id: self.config.instrument.instrument_id.clone(),
            last_price: None,
            best_bid: Some(raw.best_bid),
            best_ask: Some(raw.best_ask),
            mark_price: None,
            index_price: None,
            bid_size: Some(raw.bid_size),
            ask_size: Some(raw.ask_size),
            source_sequence,
            source_event_id: Some(binance_public_wss_source_event_id(
                self.config.market,
                &raw.symbol,
                source_sequence,
                raw.update_id,
            )),
            observed_at: raw.observed_at,
            ingested_at: effective_ingested_at,
        };
        let applied = self
            .coordinator
            .apply(HybridMarketDataInput::WssQuote { update })?;
        self.last_exchange_update_id = Some(raw.update_id);
        Ok(applied)
    }

    /// 连接真实 Binance WSS 并读取有限条公开 `bookTicker` 消息。
    ///
    /// 中文说明：这是显式联网入口，默认测试不会调用。返回的每条更新都已经进入
    /// `RestWssMarketDataCoordinator`。如果连接失败或消息异常，调用方必须 fail closed，
    /// 并可用 REST 快照重新启动本客户端状态。
    pub fn read_live_wss_updates(
        &mut self,
        max_text_messages: usize,
    ) -> VenueDataResult<Vec<HybridMarketDataUpdate>> {
        let mut updates = Vec::new();
        self.read_live_wss_updates_observed(max_text_messages, |update| {
            updates.push(update.clone());
        })?;
        Ok(updates)
    }

    /// 连接真实 Binance WSS 并在每条公开 `bookTicker` 更新后回调观察者。
    ///
    /// 中文说明：常驻任务用该入口把协调器状态同步到本地 `/health` API。观察者
    /// 不能改变 WSS 连接语义；连接失败、断线和异常消息仍由本客户端 fail closed。
    pub fn read_live_wss_updates_observed<F>(
        &mut self,
        max_text_messages: usize,
        mut observer: F,
    ) -> VenueDataResult<()>
    where
        F: FnMut(&HybridMarketDataUpdate),
    {
        if max_text_messages == 0 {
            return Err(VenueDataError::InvalidQuery {
                field: "binance.wss.max_text_messages",
                reason: "must be greater than zero",
            });
        }

        let stream_url = self.stream_url();
        let (mut socket, _response) =
            tungstenite::connect(stream_url.as_str()).map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.config.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::Disconnected,
                    format!("cannot connect Binance public WSS `{stream_url}`: {error}"),
                ))
            })?;

        let connected_at = current_utc_timestamp(&self.config.venue_id)?;
        let connected_update = self
            .coordinator
            .apply(HybridMarketDataInput::WssConnected {
                occurred_at: connected_at,
                ingested_at: connected_at,
            })?;
        observer(&connected_update);
        let mut text_messages = 0_usize;

        while text_messages < max_text_messages {
            let message = match socket.read() {
                Ok(message) => message,
                Err(error) => {
                    let occurred_at = current_utc_timestamp(&self.config.venue_id)?;
                    let detail = format!("Binance public WSS read failed: {error}");
                    let _ = self
                        .coordinator
                        .apply(HybridMarketDataInput::WssDisconnected {
                            reason: detail.clone(),
                            occurred_at,
                            ingested_at: occurred_at,
                        })
                        .map(|update| observer(&update));
                    return Err(VenueDataError::External(ClassifiedExternalError::new(
                        self.config.venue_id.clone(),
                        ReadOnlySurface::MarketData,
                        ExternalErrorClass::Disconnected,
                        detail,
                    )));
                }
            };
            match message {
                tungstenite::Message::Text(text) => {
                    let ingested_at = current_utc_timestamp(&self.config.venue_id)?;
                    let update = self.apply_wss_text_message(&text, ingested_at)?;
                    observer(&update);
                    text_messages += 1;
                }
                tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => {
                    socket.flush().map_err(|error| {
                        VenueDataError::External(ClassifiedExternalError::new(
                            self.config.venue_id.clone(),
                            ReadOnlySurface::MarketData,
                            ExternalErrorClass::Disconnected,
                            format!("Binance public WSS heartbeat flush failed: {error}"),
                        ))
                    })?;
                    let occurred_at = current_utc_timestamp(&self.config.venue_id)?;
                    let update = self
                        .coordinator
                        .apply(HybridMarketDataInput::WssHeartbeat {
                            source_sequence: Some(self.local_sequence),
                            occurred_at,
                            ingested_at: occurred_at,
                        })?;
                    observer(&update);
                }
                tungstenite::Message::Close(frame) => {
                    let occurred_at = current_utc_timestamp(&self.config.venue_id)?;
                    let reason = frame
                        .map(|frame| frame.reason.to_string())
                        .filter(|reason| !reason.is_empty())
                        .unwrap_or_else(|| "Binance public WSS closed".to_owned());
                    let update =
                        self.coordinator
                            .apply(HybridMarketDataInput::WssDisconnected {
                                reason,
                                occurred_at,
                                ingested_at: occurred_at,
                            })?;
                    observer(&update);
                    break;
                }
                tungstenite::Message::Binary(_) | tungstenite::Message::Frame(_) => {
                    let occurred_at = current_utc_timestamp(&self.config.venue_id)?;
                    let update = self
                        .coordinator
                        .apply(HybridMarketDataInput::WssGapDetected {
                            expected_sequence: None,
                            observed_sequence: Some(self.local_sequence),
                            occurred_at,
                            ingested_at: occurred_at,
                            detail: "Binance public WSS emitted non-text payload".to_owned(),
                        })?;
                    observer(&update);
                    break;
                }
            }
        }

        Ok(())
    }

    fn validate_quote_identity(&self, quote: &MarketQuote) -> VenueDataResult<()> {
        if quote.venue_id != self.config.venue_id
            || quote.instrument_id != self.config.instrument.instrument_id
        {
            return Err(VenueDataError::UnknownExternalState {
                venue_id: self.config.venue_id.clone(),
                surface: ReadOnlySurface::MarketData,
                detail: "REST snapshot venue or instrument does not match Binance WSS client"
                    .to_owned(),
            });
        }
        Ok(())
    }

    fn parse_wss_book_ticker(
        &self,
        raw_json: &str,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BinanceWssBookTickerRaw> {
        let object = FlatJsonParser::new(raw_json).parse().map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.config.venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::MalformedPayload,
                error.to_string(),
            ))
        })?;
        if let Some(FlatJsonValue::String(event_type)) = object.get("e") {
            if event_type != "bookTicker" {
                return Err(VenueDataError::External(ClassifiedExternalError::new(
                    self.config.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::MalformedPayload,
                    format!("WSS event type `{event_type}` is not bookTicker"),
                )));
            }
        }
        let symbol = required_string(
            &object,
            "s",
            self.config.venue_id.clone(),
            ReadOnlySurface::MarketData,
        )?;
        if symbol != self.config.instrument.symbol {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.config.venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::UnknownExternalState,
                format!(
                    "WSS symbol `{symbol}` does not match configured symbol `{}`",
                    self.config.instrument.symbol
                ),
            )));
        }
        let observed_at = optional_u64(&object, "T")?
            .or(optional_u64(&object, "E")?)
            .map(timestamp_from_unix_millis)
            .transpose()
            .map_err(|detail| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.config.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::MalformedPayload,
                    detail,
                ))
            })?
            .unwrap_or(ingested_at);

        Ok(BinanceWssBookTickerRaw {
            symbol,
            update_id: required_u64(
                &object,
                "u",
                self.config.venue_id.clone(),
                ReadOnlySurface::MarketData,
            )?,
            best_bid: parse_price_field(&object, "b", &self.config.venue_id)?,
            best_ask: parse_price_field(&object, "a", &self.config.venue_id)?,
            bid_size: parse_quantity_field(&object, "B", &self.config.venue_id)?,
            ask_size: parse_quantity_field(&object, "A", &self.config.venue_id)?,
            observed_at,
        })
    }

    fn apply_non_advancing_update_gap(
        &mut self,
        previous: u64,
        raw: &BinanceWssBookTickerRaw,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<HybridMarketDataUpdate> {
        self.coordinator.apply(HybridMarketDataInput::WssGapDetected {
            expected_sequence: previous.checked_add(1),
            observed_sequence: Some(raw.update_id),
            occurred_at: raw.observed_at,
            ingested_at,
            detail: "Binance WSS bookTicker updateId did not advance; REST snapshot required for rebuild"
                .to_owned(),
        })
    }

    fn next_local_sequence(&mut self) -> VenueDataResult<u64> {
        self.local_sequence =
            self.local_sequence
                .checked_add(1)
                .ok_or(VenueDataError::UnknownExternalState {
                    venue_id: self.config.venue_id.clone(),
                    surface: ReadOnlySurface::MarketData,
                    detail: "local Binance WSS sequence overflow".to_owned(),
                })?;
        Ok(self.local_sequence)
    }
}

fn binance_wss_effective_ingested_at(
    observed_at: UtcTimestamp,
    ingested_at: UtcTimestamp,
) -> UtcTimestamp {
    if ingested_at < observed_at {
        observed_at
    } else {
        ingested_at
    }
}

/// Binance USDⓈ-M premiumIndex 原始响应到标准化事件的输出批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceUsdmPremiumIndexBatch {
    pub raw_event: NormalizedEvent,
    pub normalized_event: NormalizedEvent,
}

/// Binance 公共 bookTicker 只读适配器。
///
/// 中文说明：该适配器只消费调用方传入的公开 REST 响应，不主动联网，不读取
/// 账户，不下单、不撤单、不转账、不签名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePublicBookTickerAdapter {
    venue_id: VenueId,
    instrument: BinancePublicInstrument,
    market: BinancePublicMarket,
    max_age_ms: u64,
    latest_quote: Option<MarketQuote>,
    health: VenueHealthSnapshot,
}

impl BinancePublicBookTickerAdapter {
    pub fn new(
        venue_id: VenueId,
        instrument: BinancePublicInstrument,
        market: BinancePublicMarket,
        started_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let freshness = DataFreshness::new(started_at, started_at, max_age_ms)?;
        Ok(Self {
            venue_id: venue_id.clone(),
            instrument,
            market,
            max_age_ms,
            latest_quote: None,
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Healthy,
                connection: VenueConnectionStatus::Connected,
                reason_codes: Vec::new(),
                rate_limit: None,
                source_event_id: None,
                freshness,
            },
        })
    }

    /// 解析 Binance bookTicker 原始响应并生成原始事件与标准化行情事件。
    pub fn ingest_book_ticker_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BinancePublicBookTickerBatch> {
        let raw_response_ref = raw_response_ref.into();
        let raw = self.parse_book_ticker(raw_json, ingested_at)?;
        let freshness = DataFreshness::new(raw.observed_at, ingested_at, self.max_age_ms)?;
        let quote = MarketQuote {
            venue_id: self.venue_id.clone(),
            instrument_id: self.instrument.instrument_id.clone(),
            last_price: None,
            best_bid: Some(raw.best_bid),
            best_ask: Some(raw.best_ask),
            mark_price: None,
            index_price: None,
            bid_size: Some(raw.bid_size),
            ask_size: Some(raw.ask_size),
            source_sequence: Some(raw.source_sequence.clone()),
            source_event_id: Some(binance_public_raw_event_id(
                "book-ticker",
                self.market,
                &raw.symbol,
                &raw.source_sequence,
            )),
            freshness,
        };

        let raw_event = self.raw_book_ticker_event(&raw, &raw_response_ref, ingested_at)?;
        let normalized_event =
            self.normalized_book_ticker_event(&raw, &raw_event, &quote, ingested_at)?;

        self.latest_quote = Some(quote.clone());
        self.health.status = if freshness.is_stale() {
            VenueHealthStatus::Degraded
        } else {
            VenueHealthStatus::Healthy
        };
        self.health.connection = VenueConnectionStatus::Connected;
        self.health.reason_codes = if freshness.is_stale() {
            vec!["DATA_STALE".to_owned()]
        } else {
            Vec::new()
        };
        self.health.source_event_id = Some(normalized_event.event_id.as_str().to_owned());
        self.health.freshness = freshness;

        Ok(BinancePublicBookTickerBatch {
            raw_event,
            normalized_event,
            quote,
        })
    }

    fn parse_book_ticker(
        &self,
        raw_json: &str,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BinanceBookTickerRaw> {
        let object = FlatJsonParser::new(raw_json).parse().map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::MalformedPayload,
                error.to_string(),
            ))
        })?;
        let symbol = required_string(
            &object,
            "symbol",
            self.venue_id.clone(),
            ReadOnlySurface::MarketData,
        )?;
        if symbol != self.instrument.symbol {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::UnknownExternalState,
                format!(
                    "raw symbol `{symbol}` does not match configured symbol `{}`",
                    self.instrument.symbol
                ),
            )));
        }

        let observed_at = optional_u64(&object, "time")?
            .map(timestamp_from_unix_millis)
            .transpose()
            .map_err(|detail| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::MalformedPayload,
                    detail,
                ))
            })?
            .unwrap_or(ingested_at);
        let source_sequence = optional_u64(&object, "time")?.map_or_else(
            || {
                format!(
                    "{}{:09}",
                    ingested_at.unix_seconds(),
                    ingested_at.nanoseconds()
                )
            },
            |time_ms| time_ms.to_string(),
        );

        Ok(BinanceBookTickerRaw {
            symbol,
            best_bid: parse_price_field(&object, "bidPrice", &self.venue_id)?,
            best_ask: parse_price_field(&object, "askPrice", &self.venue_id)?,
            bid_size: parse_quantity_field(&object, "bidQty", &self.venue_id)?,
            ask_size: parse_quantity_field(&object, "askQty", &self.venue_id)?,
            source_sequence,
            observed_at,
        })
    }

    fn raw_book_ticker_event(
        &self,
        raw: &BinanceBookTickerRaw,
        raw_response_ref: &str,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let payload = format!(
            "{{\"adapter\":\"BinancePublicBookTickerAdapter\",\"basis_role\":{},\"market\":\"{}\",\"raw_response_ref\":{},\"redaction\":\"public_market_data_only_no_account_fields\",\"symbol\":{}}}",
            json_string(self.market.basis_role()),
            self.market.as_str(),
            json_string(raw_response_ref),
            json_string(&raw.symbol),
        );
        build_normalized_event(EventEnvelope {
            event_id: binance_public_raw_event_id(
                "book-ticker",
                self.market,
                &raw.symbol,
                &raw.source_sequence,
            ),
            event_type: NormalizedEventType::RawMarketDataEvent,
            timestamp_event: raw.observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-public-book-ticker".to_owned(),
            source_sequence: Some(format!(
                "binance:{}:{}:{}:raw",
                self.market.event_scope(),
                raw.symbol,
                raw.source_sequence
            )),
            correlation_id: binance_public_correlation_id(
                "book-ticker",
                self.market,
                &raw.symbol,
                &raw.source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: Some(self.instrument.instrument_id.as_str().to_owned()),
            payload_json: payload,
        })
    }

    fn normalized_book_ticker_event(
        &self,
        raw: &BinanceBookTickerRaw,
        raw_event: &NormalizedEvent,
        quote: &MarketQuote,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let freshness = quote.freshness;
        let payload = format!(
            "{{\"adapter\":\"BinancePublicBookTickerAdapter\",\"ask_size\":{},\"basis_role\":{},\"best_ask\":{},\"best_bid\":{},\"bid_size\":{},\"freshness\":{},\"kind\":\"BookTicker\",\"market\":\"{}\",\"raw_event_ref\":{},\"risk_reason_code\":{},\"venue_symbol\":{}}}",
            json_string(&raw.ask_size.to_string()),
            json_string(self.market.basis_role()),
            json_string(&raw.best_ask.to_string()),
            json_string(&raw.best_bid.to_string()),
            json_string(&raw.bid_size.to_string()),
            freshness_payload_json(freshness),
            self.market.as_str(),
            json_string(raw_event.event_id.as_str()),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
            json_string(&raw.symbol),
        );
        build_normalized_event(EventEnvelope {
            event_id: binance_public_normalized_event_id(
                "book-ticker",
                self.market,
                &raw.symbol,
                &raw.source_sequence,
            ),
            event_type: NormalizedEventType::NormalizedMarketDataEvent,
            timestamp_event: raw.observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-public-book-ticker".to_owned(),
            source_sequence: Some(format!(
                "binance:{}:{}:{}:normalized",
                self.market.event_scope(),
                raw.symbol,
                raw.source_sequence
            )),
            correlation_id: binance_public_correlation_id(
                "book-ticker",
                self.market,
                &raw.symbol,
                &raw.source_sequence,
            ),
            causation_id: Some(raw_event.event_id.as_str().to_owned()),
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: Some(self.instrument.instrument_id.as_str().to_owned()),
            payload_json: payload,
        })
    }
}

impl VenueReadAdapter for BinancePublicBookTickerAdapter {
    fn venue_id(&self) -> &VenueId {
        &self.venue_id
    }
}

impl MarketDataReader for BinancePublicBookTickerAdapter {
    fn latest_quote(&self, query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
        Ok((query.venue_id == self.venue_id
            && query.instrument_id == self.instrument.instrument_id)
            .then(|| self.latest_quote.clone())
            .flatten())
    }

    fn order_book(&self, query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>> {
        if query.venue_id != self.venue_id || query.instrument_id != self.instrument.instrument_id {
            return Ok(None);
        }
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Binance public bookTicker only exposes top-of-book, not full depth".to_owned(),
        })
    }
}

impl BalanceReader for BinancePublicBookTickerAdapter {
    fn balances(&self, _query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::Balance,
            reason: "public market data adapter has no account balance surface".to_owned(),
        })
    }
}

impl PositionReader for BinancePublicBookTickerAdapter {
    fn positions(&self, _query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::Position,
            reason: "public market data adapter has no account position surface".to_owned(),
        })
    }
}

impl InstrumentInfoReader for BinancePublicBookTickerAdapter {
    fn instruments(&self, query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>> {
        if query.venue_id != self.venue_id {
            return Ok(Vec::new());
        }
        if query
            .instrument_id
            .as_ref()
            .is_some_and(|instrument_id| instrument_id != &self.instrument.instrument_id)
        {
            return Ok(Vec::new());
        }

        Ok(vec![InstrumentInfo {
            venue_id: self.venue_id.clone(),
            instrument_id: self.instrument.instrument_id.clone(),
            kind: self.market.instrument_kind(),
            base_asset_id: Some(self.instrument.base_asset_id.clone()),
            quote_asset_id: Some(self.instrument.quote_asset_id.clone()),
            settlement_asset_id: self.instrument.settlement_asset_id.clone(),
            margin_asset_id: (self.market == BinancePublicMarket::UsdmPerpetual)
                .then(|| self.instrument.settlement_asset_id.clone()),
            tick_size: self.instrument.tick_size,
            lot_size: self.instrument.lot_size,
            contract_multiplier: None,
            is_active: true,
            source_event_id: self
                .latest_quote
                .as_ref()
                .and_then(|quote| quote.source_event_id.clone()),
            freshness: self.health.freshness,
        }])
    }
}

impl VenueHealthReader for BinancePublicBookTickerAdapter {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
        if venue_id == &self.venue_id {
            Ok(self.health.clone())
        } else {
            Err(VenueDataError::DataUnavailable {
                venue_id: venue_id.clone(),
                surface: ReadOnlySurface::VenueHealth,
                reason: "adapter only tracks its configured venue".to_owned(),
            })
        }
    }
}

/// Binance USDⓈ-M premiumIndex 只读适配器。
///
/// 中文说明：该适配器只解析公开 mark price、index price 和 funding rate，
/// 不读取仓位、不签名，也不提交任何账户动作。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceUsdmPremiumIndexAdapter {
    venue_id: VenueId,
    instrument: BinancePublicInstrument,
    max_age_ms: u64,
}

impl BinanceUsdmPremiumIndexAdapter {
    pub fn new(
        venue_id: VenueId,
        instrument: BinancePublicInstrument,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        Ok(Self {
            venue_id,
            instrument,
            max_age_ms,
        })
    }

    pub fn ingest_premium_index_json(
        &self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BinanceUsdmPremiumIndexBatch> {
        let raw_response_ref = raw_response_ref.into();
        let raw = self.parse_premium_index(raw_json)?;
        let freshness = DataFreshness::new(raw.observed_at, ingested_at, self.max_age_ms)?;
        let raw_event = self.raw_premium_index_event(&raw, &raw_response_ref, ingested_at)?;
        let normalized_event =
            self.normalized_premium_index_event(&raw, &raw_event, freshness, ingested_at)?;
        Ok(BinanceUsdmPremiumIndexBatch {
            raw_event,
            normalized_event,
        })
    }

    fn parse_premium_index(&self, raw_json: &str) -> VenueDataResult<BinancePremiumIndexRaw> {
        let object = FlatJsonParser::new(raw_json).parse().map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::MalformedPayload,
                error.to_string(),
            ))
        })?;
        let symbol = required_string(
            &object,
            "symbol",
            self.venue_id.clone(),
            ReadOnlySurface::MarketData,
        )?;
        if symbol != self.instrument.symbol {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::UnknownExternalState,
                format!(
                    "raw symbol `{symbol}` does not match configured symbol `{}`",
                    self.instrument.symbol
                ),
            )));
        }
        let time_ms = required_u64(
            &object,
            "time",
            self.venue_id.clone(),
            ReadOnlySurface::MarketData,
        )?;
        Ok(BinancePremiumIndexRaw {
            symbol,
            mark_price: parse_price_field(&object, "markPrice", &self.venue_id)?,
            index_price: parse_price_field(&object, "indexPrice", &self.venue_id)?,
            last_funding_rate: parse_decimal_string_field(
                &object,
                "lastFundingRate",
                &self.venue_id,
            )?,
            interest_rate: parse_decimal_string_field(&object, "interestRate", &self.venue_id)?,
            next_funding_time_ms: required_u64(
                &object,
                "nextFundingTime",
                self.venue_id.clone(),
                ReadOnlySurface::MarketData,
            )?,
            time_ms,
            observed_at: timestamp_from_unix_millis(time_ms).map_err(|detail| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::MalformedPayload,
                    detail,
                ))
            })?,
        })
    }

    fn raw_premium_index_event(
        &self,
        raw: &BinancePremiumIndexRaw,
        raw_response_ref: &str,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let payload = format!(
            "{{\"adapter\":\"BinanceUsdmPremiumIndexAdapter\",\"basis_role\":\"Perp\",\"raw_response_ref\":{},\"redaction\":\"public_market_data_only_no_account_fields\",\"symbol\":{}}}",
            json_string(raw_response_ref),
            json_string(&raw.symbol),
        );
        build_normalized_event(EventEnvelope {
            event_id: binance_public_raw_event_id(
                "premium-index",
                BinancePublicMarket::UsdmPerpetual,
                &raw.symbol,
                &raw.time_ms.to_string(),
            ),
            event_type: NormalizedEventType::RawMarketDataEvent,
            timestamp_event: raw.observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-usdm-premium-index".to_owned(),
            source_sequence: Some(format!(
                "binance:usdm-perp:{}:{}:raw",
                raw.symbol, raw.time_ms
            )),
            correlation_id: binance_public_correlation_id(
                "premium-index",
                BinancePublicMarket::UsdmPerpetual,
                &raw.symbol,
                &raw.time_ms.to_string(),
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: Some(self.instrument.instrument_id.as_str().to_owned()),
            payload_json: payload,
        })
    }

    fn normalized_premium_index_event(
        &self,
        raw: &BinancePremiumIndexRaw,
        raw_event: &NormalizedEvent,
        freshness: DataFreshness,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let payload = format!(
            "{{\"adapter\":\"BinanceUsdmPremiumIndexAdapter\",\"basis_role\":\"Perp\",\"freshness\":{},\"index_price\":{},\"interest_rate\":{},\"kind\":\"PerpPremiumIndex\",\"last_funding_rate\":{},\"mark_price\":{},\"next_funding_time_ms\":{},\"raw_event_ref\":{},\"risk_reason_code\":{},\"venue_symbol\":{}}}",
            freshness_payload_json(freshness),
            json_string(&raw.index_price.to_string()),
            json_string(&raw.interest_rate),
            json_string(&raw.last_funding_rate),
            json_string(&raw.mark_price.to_string()),
            raw.next_funding_time_ms,
            json_string(raw_event.event_id.as_str()),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
            json_string(&raw.symbol),
        );
        build_normalized_event(EventEnvelope {
            event_id: binance_public_normalized_event_id(
                "premium-index",
                BinancePublicMarket::UsdmPerpetual,
                &raw.symbol,
                &raw.time_ms.to_string(),
            ),
            event_type: NormalizedEventType::NormalizedMarketDataEvent,
            timestamp_event: raw.observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-usdm-premium-index".to_owned(),
            source_sequence: Some(format!(
                "binance:usdm-perp:{}:{}:normalized",
                raw.symbol, raw.time_ms
            )),
            correlation_id: binance_public_correlation_id(
                "premium-index",
                BinancePublicMarket::UsdmPerpetual,
                &raw.symbol,
                &raw.time_ms.to_string(),
            ),
            causation_id: Some(raw_event.event_id.as_str().to_owned()),
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: Some(self.instrument.instrument_id.as_str().to_owned()),
            payload_json: payload,
        })
    }
}

/// Bybit 公共 V5 ticker 只读适配器。
///
/// 中文说明：该适配器只消费调用方传入的公开 REST 响应，不主动联网，不读取
/// 账户，不下单、不撤单、不转账、不签名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitPublicTickerAdapter {
    venue_id: VenueId,
    instrument: BybitPublicInstrument,
    market: BybitPublicMarket,
    max_age_ms: u64,
    latest_quote: Option<MarketQuote>,
    health: VenueHealthSnapshot,
}

impl BybitPublicTickerAdapter {
    pub fn new(
        venue_id: VenueId,
        instrument: BybitPublicInstrument,
        market: BybitPublicMarket,
        started_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let freshness = DataFreshness::new(started_at, started_at, max_age_ms)?;
        Ok(Self {
            venue_id: venue_id.clone(),
            instrument,
            market,
            max_age_ms,
            latest_quote: None,
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Healthy,
                connection: VenueConnectionStatus::Connected,
                reason_codes: Vec::new(),
                rate_limit: None,
                source_event_id: None,
                freshness,
            },
        })
    }

    /// 解析 Bybit V5 `market/tickers` 原始响应并生成原始事件与标准化行情事件。
    pub fn ingest_ticker_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BybitPublicTickerBatch> {
        let raw_response_ref = raw_response_ref.into();
        let raw = self.parse_ticker(raw_json)?;
        let freshness = DataFreshness::new(raw.observed_at, ingested_at, self.max_age_ms)?;
        let quote = MarketQuote {
            venue_id: self.venue_id.clone(),
            instrument_id: self.instrument.instrument_id.clone(),
            last_price: None,
            best_bid: Some(raw.best_bid),
            best_ask: Some(raw.best_ask),
            mark_price: None,
            index_price: None,
            bid_size: Some(raw.bid_size),
            ask_size: Some(raw.ask_size),
            source_sequence: Some(raw.source_sequence.clone()),
            source_event_id: Some(bybit_public_raw_event_id(
                "ticker",
                self.market,
                &raw.symbol,
                &raw.source_sequence,
            )),
            freshness,
        };

        let raw_event = self.raw_ticker_event(&raw, &raw_response_ref, ingested_at)?;
        let normalized_event =
            self.normalized_ticker_event(&raw, &raw_event, &quote, ingested_at)?;

        self.latest_quote = Some(quote.clone());
        self.health.status = if freshness.is_stale() {
            VenueHealthStatus::Degraded
        } else {
            VenueHealthStatus::Healthy
        };
        self.health.connection = VenueConnectionStatus::Connected;
        self.health.reason_codes = if freshness.is_stale() {
            vec!["DATA_STALE".to_owned()]
        } else {
            Vec::new()
        };
        self.health.source_event_id = Some(normalized_event.event_id.as_str().to_owned());
        self.health.freshness = freshness;

        Ok(BybitPublicTickerBatch {
            raw_event,
            normalized_event,
            quote,
        })
    }

    fn parse_ticker(&self, raw_json: &str) -> VenueDataResult<BybitTickerRaw> {
        let raw = parse_bybit_v5_ticker_row(
            raw_json,
            &self.venue_id,
            self.market,
            &self.instrument.symbol,
        )?;
        let best_bid = parse_price_field(&raw.row, "bid1Price", &self.venue_id)?;
        let best_ask = parse_price_field(&raw.row, "ask1Price", &self.venue_id)?;
        let bid_size = parse_quantity_field(&raw.row, "bid1Size", &self.venue_id)?;
        let ask_size = parse_quantity_field(&raw.row, "ask1Size", &self.venue_id)?;
        Ok(BybitTickerRaw {
            symbol: raw.symbol,
            best_bid,
            best_ask,
            bid_size,
            ask_size,
            source_sequence: raw.time_ms.to_string(),
            observed_at: raw.observed_at,
        })
    }

    fn raw_ticker_event(
        &self,
        raw: &BybitTickerRaw,
        raw_response_ref: &str,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let payload = format!(
            "{{\"adapter\":\"BybitPublicTickerAdapter\",\"basis_role\":{},\"category\":{},\"market\":\"{}\",\"raw_response_ref\":{},\"redaction\":\"public_market_data_only_no_account_fields\",\"symbol\":{}}}",
            json_string(self.market.basis_role()),
            json_string(self.market.category()),
            self.market.as_str(),
            json_string(raw_response_ref),
            json_string(&raw.symbol),
        );
        build_normalized_event(EventEnvelope {
            event_id: bybit_public_raw_event_id(
                "ticker",
                self.market,
                &raw.symbol,
                &raw.source_sequence,
            ),
            event_type: NormalizedEventType::RawMarketDataEvent,
            timestamp_event: raw.observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:bybit-public-ticker".to_owned(),
            source_sequence: Some(format!(
                "bybit:{}:{}:{}:raw",
                self.market.event_scope(),
                raw.symbol,
                raw.source_sequence
            )),
            correlation_id: bybit_public_correlation_id(
                "ticker",
                self.market,
                &raw.symbol,
                &raw.source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: Some(self.instrument.instrument_id.as_str().to_owned()),
            payload_json: payload,
        })
    }

    fn normalized_ticker_event(
        &self,
        raw: &BybitTickerRaw,
        raw_event: &NormalizedEvent,
        quote: &MarketQuote,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let freshness = quote.freshness;
        let payload = format!(
            "{{\"adapter\":\"BybitPublicTickerAdapter\",\"ask_size\":{},\"basis_role\":{},\"best_ask\":{},\"best_bid\":{},\"bid_size\":{},\"freshness\":{},\"kind\":\"BookTicker\",\"market\":\"{}\",\"raw_event_ref\":{},\"risk_reason_code\":{},\"venue_symbol\":{}}}",
            json_string(&raw.ask_size.to_string()),
            json_string(self.market.basis_role()),
            json_string(&raw.best_ask.to_string()),
            json_string(&raw.best_bid.to_string()),
            json_string(&raw.bid_size.to_string()),
            freshness_payload_json(freshness),
            self.market.as_str(),
            json_string(raw_event.event_id.as_str()),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
            json_string(&raw.symbol),
        );
        build_normalized_event(EventEnvelope {
            event_id: bybit_public_normalized_event_id(
                "ticker",
                self.market,
                &raw.symbol,
                &raw.source_sequence,
            ),
            event_type: NormalizedEventType::NormalizedMarketDataEvent,
            timestamp_event: raw.observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:bybit-public-ticker".to_owned(),
            source_sequence: Some(format!(
                "bybit:{}:{}:{}:normalized",
                self.market.event_scope(),
                raw.symbol,
                raw.source_sequence
            )),
            correlation_id: bybit_public_correlation_id(
                "ticker",
                self.market,
                &raw.symbol,
                &raw.source_sequence,
            ),
            causation_id: Some(raw_event.event_id.as_str().to_owned()),
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: Some(self.instrument.instrument_id.as_str().to_owned()),
            payload_json: payload,
        })
    }
}

impl VenueReadAdapter for BybitPublicTickerAdapter {
    fn venue_id(&self) -> &VenueId {
        &self.venue_id
    }
}

impl MarketDataReader for BybitPublicTickerAdapter {
    fn latest_quote(&self, query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
        Ok((query.venue_id == self.venue_id
            && query.instrument_id == self.instrument.instrument_id)
            .then(|| self.latest_quote.clone())
            .flatten())
    }

    fn order_book(&self, query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>> {
        if query.venue_id != self.venue_id || query.instrument_id != self.instrument.instrument_id {
            return Ok(None);
        }
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Bybit public ticker only exposes top-of-book, not full depth".to_owned(),
        })
    }
}

impl BalanceReader for BybitPublicTickerAdapter {
    fn balances(&self, _query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::Balance,
            reason: "public market data adapter has no account balance surface".to_owned(),
        })
    }
}

impl PositionReader for BybitPublicTickerAdapter {
    fn positions(&self, _query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::Position,
            reason: "public market data adapter has no account position surface".to_owned(),
        })
    }
}

impl InstrumentInfoReader for BybitPublicTickerAdapter {
    fn instruments(&self, query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>> {
        if query.venue_id != self.venue_id {
            return Ok(Vec::new());
        }
        if query
            .instrument_id
            .as_ref()
            .is_some_and(|instrument_id| instrument_id != &self.instrument.instrument_id)
        {
            return Ok(Vec::new());
        }

        Ok(vec![InstrumentInfo {
            venue_id: self.venue_id.clone(),
            instrument_id: self.instrument.instrument_id.clone(),
            kind: self.market.instrument_kind(),
            base_asset_id: Some(self.instrument.base_asset_id.clone()),
            quote_asset_id: Some(self.instrument.quote_asset_id.clone()),
            settlement_asset_id: self.instrument.settlement_asset_id.clone(),
            margin_asset_id: (self.market == BybitPublicMarket::LinearPerpetual)
                .then(|| self.instrument.settlement_asset_id.clone()),
            tick_size: self.instrument.tick_size,
            lot_size: self.instrument.lot_size,
            contract_multiplier: None,
            is_active: true,
            source_event_id: self
                .latest_quote
                .as_ref()
                .and_then(|quote| quote.source_event_id.clone()),
            freshness: self.health.freshness,
        }])
    }
}

impl VenueHealthReader for BybitPublicTickerAdapter {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
        if venue_id == &self.venue_id {
            Ok(self.health.clone())
        } else {
            Err(VenueDataError::DataUnavailable {
                venue_id: venue_id.clone(),
                surface: ReadOnlySurface::VenueHealth,
                reason: "adapter only tracks its configured venue".to_owned(),
            })
        }
    }
}

/// Bybit 线性永续 premium index 只读适配器。
///
/// 中文说明：Bybit V5 把 mark price、index price 和 funding rate 放在
/// `market/tickers?category=linear` 响应中。该适配器只解析公开字段，不读取仓位、
/// 不签名，也不提交任何账户动作。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitLinearPremiumIndexAdapter {
    venue_id: VenueId,
    instrument: BybitPublicInstrument,
    max_age_ms: u64,
}

impl BybitLinearPremiumIndexAdapter {
    pub fn new(
        venue_id: VenueId,
        instrument: BybitPublicInstrument,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        Ok(Self {
            venue_id,
            instrument,
            max_age_ms,
        })
    }

    pub fn ingest_premium_index_json(
        &self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BybitLinearPremiumIndexBatch> {
        let raw_response_ref = raw_response_ref.into();
        let raw = self.parse_premium_index(raw_json)?;
        let freshness = DataFreshness::new(raw.observed_at, ingested_at, self.max_age_ms)?;
        let raw_event = self.raw_premium_index_event(&raw, &raw_response_ref, ingested_at)?;
        let normalized_event =
            self.normalized_premium_index_event(&raw, &raw_event, freshness, ingested_at)?;
        Ok(BybitLinearPremiumIndexBatch {
            raw_event,
            normalized_event,
        })
    }

    fn parse_premium_index(&self, raw_json: &str) -> VenueDataResult<BybitPremiumIndexRaw> {
        let raw = parse_bybit_v5_ticker_row(
            raw_json,
            &self.venue_id,
            BybitPublicMarket::LinearPerpetual,
            &self.instrument.symbol,
        )?;
        let mark_price = parse_price_field(&raw.row, "markPrice", &self.venue_id)?;
        let index_price = parse_price_field(&raw.row, "indexPrice", &self.venue_id)?;
        let last_funding_rate =
            parse_decimal_string_field(&raw.row, "fundingRate", &self.venue_id)?;
        let next_funding_time_ms = required_u64(
            &raw.row,
            "nextFundingTime",
            self.venue_id.clone(),
            ReadOnlySurface::MarketData,
        )?;
        Ok(BybitPremiumIndexRaw {
            symbol: raw.symbol,
            mark_price,
            index_price,
            last_funding_rate,
            next_funding_time_ms,
            time_ms: raw.time_ms,
            observed_at: raw.observed_at,
        })
    }

    fn raw_premium_index_event(
        &self,
        raw: &BybitPremiumIndexRaw,
        raw_response_ref: &str,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let payload = format!(
            "{{\"adapter\":\"BybitLinearPremiumIndexAdapter\",\"basis_role\":\"Perp\",\"category\":\"linear\",\"raw_response_ref\":{},\"redaction\":\"public_market_data_only_no_account_fields\",\"symbol\":{}}}",
            json_string(raw_response_ref),
            json_string(&raw.symbol),
        );
        build_normalized_event(EventEnvelope {
            event_id: bybit_public_raw_event_id(
                "premium-index",
                BybitPublicMarket::LinearPerpetual,
                &raw.symbol,
                &raw.time_ms.to_string(),
            ),
            event_type: NormalizedEventType::RawMarketDataEvent,
            timestamp_event: raw.observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:bybit-linear-premium-index".to_owned(),
            source_sequence: Some(format!(
                "bybit:linear-perp:{}:{}:raw",
                raw.symbol, raw.time_ms
            )),
            correlation_id: bybit_public_correlation_id(
                "premium-index",
                BybitPublicMarket::LinearPerpetual,
                &raw.symbol,
                &raw.time_ms.to_string(),
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: Some(self.instrument.instrument_id.as_str().to_owned()),
            payload_json: payload,
        })
    }

    fn normalized_premium_index_event(
        &self,
        raw: &BybitPremiumIndexRaw,
        raw_event: &NormalizedEvent,
        freshness: DataFreshness,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let payload = format!(
            "{{\"adapter\":\"BybitLinearPremiumIndexAdapter\",\"basis_role\":\"Perp\",\"freshness\":{},\"index_price\":{},\"kind\":\"PerpPremiumIndex\",\"last_funding_rate\":{},\"mark_price\":{},\"next_funding_time_ms\":{},\"raw_event_ref\":{},\"risk_reason_code\":{},\"venue_symbol\":{}}}",
            freshness_payload_json(freshness),
            json_string(&raw.index_price.to_string()),
            json_string(&raw.last_funding_rate),
            json_string(&raw.mark_price.to_string()),
            raw.next_funding_time_ms,
            json_string(raw_event.event_id.as_str()),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
            json_string(&raw.symbol),
        );
        build_normalized_event(EventEnvelope {
            event_id: bybit_public_normalized_event_id(
                "premium-index",
                BybitPublicMarket::LinearPerpetual,
                &raw.symbol,
                &raw.time_ms.to_string(),
            ),
            event_type: NormalizedEventType::NormalizedMarketDataEvent,
            timestamp_event: raw.observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:bybit-linear-premium-index".to_owned(),
            source_sequence: Some(format!(
                "bybit:linear-perp:{}:{}:normalized",
                raw.symbol, raw.time_ms
            )),
            correlation_id: bybit_public_correlation_id(
                "premium-index",
                BybitPublicMarket::LinearPerpetual,
                &raw.symbol,
                &raw.time_ms.to_string(),
            ),
            causation_id: Some(raw_event.event_id.as_str().to_owned()),
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: Some(self.instrument.instrument_id.as_str().to_owned()),
            payload_json: payload,
        })
    }
}

/// Binance 公共 24hr ticker 离线只读适配器。
///
/// 中文说明：这是阶段 8 的第一个场所适配器样例。它只消费调用方传入的公开
/// ticker 响应字符串，不主动联网；默认测试完全依赖脱敏 fixture。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePublicTicker24hAdapter {
    venue_id: VenueId,
    instrument: BinancePublicInstrument,
    max_age_ms: u64,
    seen_source_sequences: BTreeSet<u64>,
    last_source_sequence: Option<u64>,
    latest_quote: Option<MarketQuote>,
    health: VenueHealthSnapshot,
}

impl BinancePublicTicker24hAdapter {
    pub fn new(
        venue_id: VenueId,
        instrument: BinancePublicInstrument,
        started_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let freshness = DataFreshness::new(started_at, started_at, max_age_ms)?;
        Ok(Self {
            venue_id: venue_id.clone(),
            instrument,
            max_age_ms,
            seen_source_sequences: BTreeSet::new(),
            last_source_sequence: None,
            latest_quote: None,
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Healthy,
                connection: VenueConnectionStatus::Connected,
                reason_codes: Vec::new(),
                rate_limit: None,
                source_event_id: None,
                freshness,
            },
        })
    }

    /// 解析脱敏 24hr ticker 原始响应并生成原始事件与标准化行情事件。
    pub fn ingest_ticker_24h_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BinancePublicTickerBatch> {
        let raw_response_ref = raw_response_ref.into();
        let raw = self.parse_ticker_24h(raw_json)?;
        self.reject_duplicate_or_out_of_order(raw.last_id)?;

        let freshness = DataFreshness::new(raw.observed_at, ingested_at, self.max_age_ms)?;
        let quote = MarketQuote {
            venue_id: self.venue_id.clone(),
            instrument_id: self.instrument.instrument_id.clone(),
            last_price: Some(raw.last_price),
            best_bid: Some(raw.best_bid),
            best_ask: Some(raw.best_ask),
            mark_price: Some(raw.last_price),
            index_price: None,
            bid_size: Some(raw.bid_size),
            ask_size: Some(raw.ask_size),
            source_sequence: Some(raw.last_id.to_string()),
            source_event_id: Some(raw_event_id(&raw.symbol, raw.last_id)),
            freshness,
        };

        let raw_event = self.raw_market_event(&raw, &raw_response_ref, ingested_at)?;
        let normalized_event =
            self.normalized_market_event(&raw, &raw_event, &quote, ingested_at)?;

        self.last_source_sequence = Some(raw.last_id);
        self.seen_source_sequences.insert(raw.last_id);
        self.latest_quote = Some(quote.clone());
        self.health.status = if freshness.is_stale() {
            VenueHealthStatus::Degraded
        } else {
            VenueHealthStatus::Healthy
        };
        self.health.connection = VenueConnectionStatus::Connected;
        self.health.reason_codes = if freshness.is_stale() {
            vec!["DATA_STALE".to_owned()]
        } else {
            Vec::new()
        };
        self.health.source_event_id = Some(normalized_event.event_id.as_str().to_owned());
        self.health.freshness = freshness;

        Ok(BinancePublicTickerBatch {
            raw_event,
            normalized_event,
            quote,
        })
    }

    /// 记录只读数据连接断开并输出场所健康事件。
    pub fn record_disconnect(
        &mut self,
        reason: impl Into<String>,
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let reason = reason.into();
        let freshness = DataFreshness::new(occurred_at, ingested_at, self.max_age_ms)?;
        let error = ClassifiedExternalError::new(
            self.venue_id.clone(),
            ReadOnlySurface::VenueHealth,
            ExternalErrorClass::Disconnected,
            reason.clone(),
        );
        let source_sequence = format!(
            "binance:connection:{}:disconnected",
            occurred_at.unix_seconds()
        );
        let event_id = format!(
            "event:venue-data:binance-public:health:{}:disconnected",
            occurred_at.unix_seconds()
        );
        let payload = format!(
            "{{\"adapter\":\"BinancePublicTicker24hAdapter\",\"connection\":\"{}\",\"fail_closed\":{},\"freshness\":{},\"reason_code\":{},\"retryable\":{},\"status\":\"{}\",\"venue_error_class\":\"{}\",\"venue_reason\":{}}}",
            VenueConnectionStatus::Disconnected.as_str(),
            error.fail_closed,
            freshness_payload_json(freshness),
            json_string(&error.reason_code),
            error.retryable,
            VenueHealthStatus::Unhealthy.as_str(),
            error.class.as_str(),
            json_string(&reason),
        );
        let event = build_normalized_event(EventEnvelope {
            event_id,
            event_type: NormalizedEventType::VenueHealthEvent,
            timestamp_event: occurred_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-public-24hr-ticker".to_owned(),
            source_sequence: Some(source_sequence),
            correlation_id: "corr:venue-data:binance-public:health".to_owned(),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })?;

        self.health = VenueHealthSnapshot {
            venue_id: self.venue_id.clone(),
            status: VenueHealthStatus::Unhealthy,
            connection: VenueConnectionStatus::Disconnected,
            reason_codes: vec![error.reason_code],
            rate_limit: None,
            source_event_id: Some(event.event_id.as_str().to_owned()),
            freshness,
        };
        Ok(event)
    }

    /// 记录只读数据连接正在重连并输出场所健康事件。
    pub fn record_reconnecting(
        &mut self,
        reason: impl Into<String>,
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let reason = reason.into();
        let freshness = DataFreshness::new(occurred_at, ingested_at, self.max_age_ms)?;
        let error = ClassifiedExternalError::new(
            self.venue_id.clone(),
            ReadOnlySurface::VenueHealth,
            ExternalErrorClass::Reconnecting,
            reason.clone(),
        );
        let source_sequence = format!(
            "binance:connection:{}:reconnecting",
            occurred_at.unix_seconds()
        );
        let event_id = format!(
            "event:venue-data:binance-public:health:{}:reconnecting",
            occurred_at.unix_seconds()
        );
        let payload = format!(
            "{{\"adapter\":\"BinancePublicTicker24hAdapter\",\"connection\":\"{}\",\"fail_closed\":{},\"freshness\":{},\"reason_code\":{},\"retryable\":{},\"status\":\"{}\",\"venue_error_class\":\"{}\",\"venue_reason\":{}}}",
            VenueConnectionStatus::Reconnecting.as_str(),
            error.fail_closed,
            freshness_payload_json(freshness),
            json_string(&error.reason_code),
            error.retryable,
            VenueHealthStatus::Degraded.as_str(),
            error.class.as_str(),
            json_string(&reason),
        );
        let event = build_normalized_event(EventEnvelope {
            event_id,
            event_type: NormalizedEventType::VenueHealthEvent,
            timestamp_event: occurred_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-public-24hr-ticker".to_owned(),
            source_sequence: Some(source_sequence),
            correlation_id: "corr:venue-data:binance-public:health".to_owned(),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })?;

        self.health = VenueHealthSnapshot {
            venue_id: self.venue_id.clone(),
            status: VenueHealthStatus::Degraded,
            connection: VenueConnectionStatus::Reconnecting,
            reason_codes: vec![error.reason_code],
            rate_limit: None,
            source_event_id: Some(event.event_id.as_str().to_owned()),
            freshness,
        };
        Ok(event)
    }

    /// 记录限频状态并输出场所健康事件。
    pub fn record_rate_limit(
        &mut self,
        rate_limit: RateLimitSnapshot,
        reason: impl Into<String>,
        occurred_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let reason = reason.into();
        let freshness = DataFreshness::new(occurred_at, ingested_at, self.max_age_ms)?;
        let error = ClassifiedExternalError::new(
            self.venue_id.clone(),
            ReadOnlySurface::VenueHealth,
            ExternalErrorClass::RateLimited,
            reason.clone(),
        );
        let source_sequence = format!(
            "binance:rate-limit:{}:rate-limited",
            occurred_at.unix_seconds()
        );
        let event_id = format!(
            "event:venue-data:binance-public:health:{}:rate-limited",
            occurred_at.unix_seconds()
        );
        let payload = format!(
            "{{\"adapter\":\"BinancePublicTicker24hAdapter\",\"connection\":\"{}\",\"fail_closed\":{},\"freshness\":{},\"rate_limit\":{},\"reason_code\":{},\"retryable\":{},\"status\":\"{}\",\"venue_error_class\":\"{}\",\"venue_reason\":{}}}",
            VenueConnectionStatus::Connected.as_str(),
            error.fail_closed,
            freshness_payload_json(freshness),
            rate_limit_payload_json(&rate_limit),
            json_string(&error.reason_code),
            error.retryable,
            VenueHealthStatus::Degraded.as_str(),
            error.class.as_str(),
            json_string(&reason),
        );
        let event = build_normalized_event(EventEnvelope {
            event_id,
            event_type: NormalizedEventType::VenueHealthEvent,
            timestamp_event: occurred_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-public-24hr-ticker".to_owned(),
            source_sequence: Some(source_sequence),
            correlation_id: "corr:venue-data:binance-public:health".to_owned(),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })?;

        self.health = VenueHealthSnapshot {
            venue_id: self.venue_id.clone(),
            status: VenueHealthStatus::Degraded,
            connection: VenueConnectionStatus::Connected,
            reason_codes: vec![error.reason_code],
            rate_limit: Some(rate_limit),
            source_event_id: Some(event.event_id.as_str().to_owned()),
            freshness,
        };
        Ok(event)
    }

    /// 将 HTTP 状态或上游错误映射为适配器错误分类；该函数不联网。
    pub fn classify_http_status(
        &self,
        surface: ReadOnlySurface,
        status_code: u16,
        detail: impl Into<String>,
    ) -> ClassifiedExternalError {
        let class = match status_code {
            408 | 504 => ExternalErrorClass::Timeout,
            418 | 429 => ExternalErrorClass::RateLimited,
            500..=599 => ExternalErrorClass::Disconnected,
            _ => ExternalErrorClass::UnknownExternalState,
        };
        ClassifiedExternalError::new(self.venue_id.clone(), surface, class, detail)
    }

    fn parse_ticker_24h(&self, raw_json: &str) -> VenueDataResult<BinanceTicker24hRaw> {
        let object = FlatJsonParser::new(raw_json).parse().map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::MalformedPayload,
                error.to_string(),
            ))
        })?;
        let symbol = required_string(
            &object,
            "symbol",
            self.venue_id.clone(),
            ReadOnlySurface::MarketData,
        )?;
        if symbol != self.instrument.symbol {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::UnknownExternalState,
                format!(
                    "raw symbol `{symbol}` does not match configured symbol `{}`",
                    self.instrument.symbol
                ),
            )));
        }

        let last_id = required_u64(
            &object,
            "lastId",
            self.venue_id.clone(),
            ReadOnlySurface::MarketData,
        )?;
        let close_time_ms = required_u64(
            &object,
            "closeTime",
            self.venue_id.clone(),
            ReadOnlySurface::MarketData,
        )?;

        Ok(BinanceTicker24hRaw {
            symbol,
            last_price: parse_price_field(&object, "lastPrice", &self.venue_id)?,
            best_bid: parse_price_field(&object, "bidPrice", &self.venue_id)?,
            best_ask: parse_price_field(&object, "askPrice", &self.venue_id)?,
            bid_size: parse_quantity_field(&object, "bidQty", &self.venue_id)?,
            ask_size: parse_quantity_field(&object, "askQty", &self.venue_id)?,
            close_time_ms,
            last_id,
            trade_count: optional_u64(&object, "count")?,
            observed_at: timestamp_from_unix_millis(close_time_ms).map_err(|detail| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::MarketData,
                    ExternalErrorClass::MalformedPayload,
                    detail,
                ))
            })?,
        })
    }

    fn reject_duplicate_or_out_of_order(&self, source_sequence: u64) -> VenueDataResult<()> {
        if self.seen_source_sequences.contains(&source_sequence) {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::DuplicateMessage,
                format!("source sequence {source_sequence} was already ingested"),
            )));
        }

        if self
            .last_source_sequence
            .is_some_and(|last| source_sequence <= last)
        {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::OutOfOrderMessage,
                format!(
                    "source sequence {source_sequence} is not greater than previous sequence {}",
                    self.last_source_sequence.expect("checked above")
                ),
            )));
        }

        Ok(())
    }

    fn raw_market_event(
        &self,
        raw: &BinanceTicker24hRaw,
        raw_response_ref: &str,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let payload = format!(
            "{{\"adapter\":\"BinancePublicTicker24hAdapter\",\"last_id\":{},\"raw_response_ref\":{},\"redaction\":\"public_market_data_only_no_account_fields\",\"symbol\":{},\"trade_count\":{}}}",
            raw.last_id,
            json_string(raw_response_ref),
            json_string(&raw.symbol),
            raw.trade_count
                .map_or_else(|| "null".to_owned(), |value| value.to_string()),
        );
        build_normalized_event(EventEnvelope {
            event_id: raw_event_id(&raw.symbol, raw.last_id),
            event_type: NormalizedEventType::RawMarketDataEvent,
            timestamp_event: raw.observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-public-24hr-ticker".to_owned(),
            source_sequence: Some(format!("binance:{}:{}:raw", raw.symbol, raw.last_id)),
            correlation_id: correlation_id(&raw.symbol, raw.last_id),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: Some(self.instrument.instrument_id.as_str().to_owned()),
            payload_json: payload,
        })
    }

    fn normalized_market_event(
        &self,
        raw: &BinanceTicker24hRaw,
        raw_event: &NormalizedEvent,
        quote: &MarketQuote,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<NormalizedEvent> {
        let freshness = quote.freshness;
        let payload = format!(
            "{{\"adapter\":\"BinancePublicTicker24hAdapter\",\"ask_size\":{},\"best_ask\":{},\"best_bid\":{},\"bid_size\":{},\"freshness\":{},\"kind\":\"MarketQuote\",\"last_price\":{},\"raw_event_ref\":{},\"risk_reason_code\":{},\"venue_symbol\":{}}}",
            json_string(&raw.ask_size.to_string()),
            json_string(&raw.best_ask.to_string()),
            json_string(&raw.best_bid.to_string()),
            json_string(&raw.bid_size.to_string()),
            freshness_payload_json(freshness),
            json_string(&raw.last_price.to_string()),
            json_string(raw_event.event_id.as_str()),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
            json_string(&raw.symbol),
        );
        build_normalized_event(EventEnvelope {
            event_id: normalized_event_id(&raw.symbol, raw.last_id),
            event_type: NormalizedEventType::NormalizedMarketDataEvent,
            timestamp_event: raw.observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-public-24hr-ticker".to_owned(),
            source_sequence: Some(format!("binance:{}:{}:normalized", raw.symbol, raw.last_id)),
            correlation_id: correlation_id(&raw.symbol, raw.last_id),
            causation_id: Some(raw_event.event_id.as_str().to_owned()),
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: Some(self.instrument.instrument_id.as_str().to_owned()),
            payload_json: payload,
        })
    }
}

impl VenueReadAdapter for BinancePublicTicker24hAdapter {
    fn venue_id(&self) -> &VenueId {
        &self.venue_id
    }
}

impl MarketDataReader for BinancePublicTicker24hAdapter {
    fn latest_quote(&self, query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
        Ok((query.venue_id == self.venue_id
            && query.instrument_id == self.instrument.instrument_id)
            .then(|| self.latest_quote.clone())
            .flatten())
    }

    fn order_book(&self, query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>> {
        if query.venue_id != self.venue_id || query.instrument_id != self.instrument.instrument_id {
            return Ok(None);
        }
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Binance public 24hr ticker fixture does not include order book depth"
                .to_owned(),
        })
    }
}

impl BalanceReader for BinancePublicTicker24hAdapter {
    fn balances(&self, _query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::Balance,
            reason: "public market data adapter has no account balance surface".to_owned(),
        })
    }
}

impl PositionReader for BinancePublicTicker24hAdapter {
    fn positions(&self, _query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::Position,
            reason: "public market data adapter has no account position surface".to_owned(),
        })
    }
}

impl InstrumentInfoReader for BinancePublicTicker24hAdapter {
    fn instruments(&self, query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>> {
        if query.venue_id != self.venue_id {
            return Ok(Vec::new());
        }
        if query
            .instrument_id
            .as_ref()
            .is_some_and(|instrument_id| instrument_id != &self.instrument.instrument_id)
        {
            return Ok(Vec::new());
        }

        Ok(vec![InstrumentInfo {
            venue_id: self.venue_id.clone(),
            instrument_id: self.instrument.instrument_id.clone(),
            kind: InstrumentKind::SpotPair,
            base_asset_id: Some(self.instrument.base_asset_id.clone()),
            quote_asset_id: Some(self.instrument.quote_asset_id.clone()),
            settlement_asset_id: self.instrument.settlement_asset_id.clone(),
            margin_asset_id: None,
            tick_size: self.instrument.tick_size,
            lot_size: self.instrument.lot_size,
            contract_multiplier: None,
            is_active: true,
            source_event_id: self
                .latest_quote
                .as_ref()
                .and_then(|quote| quote.source_event_id.clone()),
            freshness: self.health.freshness,
        }])
    }
}

impl VenueHealthReader for BinancePublicTicker24hAdapter {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
        if venue_id == &self.venue_id {
            Ok(self.health.clone())
        } else {
            Err(VenueDataError::DataUnavailable {
                venue_id: venue_id.clone(),
                surface: ReadOnlySurface::VenueHealth,
                reason: "adapter only tracks its configured venue".to_owned(),
            })
        }
    }
}

/// Binance 私有账户只读市场类型。
///
/// 中文说明：该枚举只描述账户快照来源，不表达交易、撤单、划转或签名能力。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinancePrivateAccountMarket {
    Spot,
    UsdmFutures,
    PortfolioMargin,
}

impl BinancePrivateAccountMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spot => "Spot",
            Self::UsdmFutures => "UsdmFutures",
            Self::PortfolioMargin => "PortfolioMargin",
        }
    }

    fn event_scope(self) -> &'static str {
        match self {
            Self::Spot => "spot",
            Self::UsdmFutures => "usdm-futures",
            Self::PortfolioMargin => "portfolio-margin",
        }
    }

    fn account_endpoint(self) -> &'static str {
        match self {
            Self::Spot => "/api/v3/account",
            Self::UsdmFutures => "/fapi/v3/account",
            Self::PortfolioMargin => "/papi/v1/account",
        }
    }

    fn position_endpoint(self) -> &'static str {
        match self {
            Self::Spot => "",
            Self::UsdmFutures => "/fapi/v3/positionRisk",
            Self::PortfolioMargin => "/papi/v1/um/positionRisk",
        }
    }
}

/// Binance 私有账户余额快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePrivateBalanceBatch {
    pub balance_event: NormalizedEvent,
    pub balances: Vec<VenueBalance>,
}

/// Binance 私有账户仓位快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePrivatePositionBatch {
    pub position_event: NormalizedEvent,
    pub positions: Vec<VenuePosition>,
}

/// Bybit 私有账户只读市场类型。
///
/// 中文说明：该枚举只描述账户快照来源，不表达交易、撤单、划转或签名能力。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BybitPrivateAccountMarket {
    UnifiedAccount,
    LinearPerpetual,
}

impl BybitPrivateAccountMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnifiedAccount => "UnifiedAccount",
            Self::LinearPerpetual => "LinearPerpetual",
        }
    }

    fn event_scope(self) -> &'static str {
        match self {
            Self::UnifiedAccount => "unified",
            Self::LinearPerpetual => "linear-perp",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::UnifiedAccount => "/v5/account/wallet-balance",
            Self::LinearPerpetual => "/v5/position/list",
        }
    }
}

/// Bybit 私有账户余额快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitPrivateBalanceBatch {
    pub balance_event: NormalizedEvent,
    pub balances: Vec<VenueBalance>,
}

/// Bybit 私有账户仓位快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitPrivatePositionBatch {
    pub position_event: NormalizedEvent,
    pub positions: Vec<VenuePosition>,
}

/// OKX 私有账户只读市场类型。
///
/// 中文说明：该枚举只描述账户快照来源，不表达交易、撤单、划转或签名能力。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OkxPrivateAccountMarket {
    TradingAccount,
}

impl OkxPrivateAccountMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TradingAccount => "TradingAccount",
        }
    }

    fn event_scope(self) -> &'static str {
        match self {
            Self::TradingAccount => "trading",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::TradingAccount => "/api/v5/account/balance",
        }
    }
}

/// OKX 私有账户余额快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OkxPrivateBalanceBatch {
    pub balance_event: NormalizedEvent,
    pub balances: Vec<VenueBalance>,
}

/// OKX 私有账户仓位快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OkxPrivatePositionBatch {
    pub position_event: NormalizedEvent,
    pub positions: Vec<VenuePosition>,
}

/// Bitget 私有账户只读市场类型。
///
/// 中文说明：该枚举只描述账户快照来源，不表达交易、撤单、划转或签名能力。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitgetPrivateAccountMarket {
    SpotAccount,
    UsdtFuturesAccount,
}

impl BitgetPrivateAccountMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SpotAccount => "SpotAccount",
            Self::UsdtFuturesAccount => "UsdtFuturesAccount",
        }
    }

    fn event_scope(self) -> &'static str {
        match self {
            Self::SpotAccount => "spot",
            Self::UsdtFuturesAccount => "usdt-futures",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::SpotAccount => "/api/v2/spot/account/assets",
            Self::UsdtFuturesAccount => "/api/v2/mix/account/accounts",
        }
    }
}

/// Bitget 私有账户余额快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BitgetPrivateBalanceBatch {
    pub balance_event: NormalizedEvent,
    pub balances: Vec<VenueBalance>,
}

/// Bitget 私有账户仓位快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BitgetPrivatePositionBatch {
    pub position_event: NormalizedEvent,
    pub positions: Vec<VenuePosition>,
}

/// Aster 私有账户只读市场类型。
///
/// 中文说明：该枚举只描述 Aster Futures V3 账户快照来源，不表达交易、撤单、
/// 划转、签名或链上授权能力。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AsterPrivateAccountMarket {
    UsdtFuturesAccount,
}

impl AsterPrivateAccountMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UsdtFuturesAccount => "UsdtFuturesAccount",
        }
    }

    fn event_scope(self) -> &'static str {
        match self {
            Self::UsdtFuturesAccount => "usdt-futures",
        }
    }

    fn balance_endpoint(self) -> &'static str {
        match self {
            Self::UsdtFuturesAccount => "/fapi/v3/balance",
        }
    }

    fn position_endpoint(self) -> &'static str {
        match self {
            Self::UsdtFuturesAccount => "/fapi/v3/positionRisk",
        }
    }
}

/// Aster 私有账户余额快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AsterPrivateBalanceBatch {
    pub balance_event: NormalizedEvent,
    pub balances: Vec<VenueBalance>,
}

/// Aster 私有账户仓位快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AsterPrivatePositionBatch {
    pub position_event: NormalizedEvent,
    pub positions: Vec<VenuePosition>,
}

/// Hyperliquid 私有账户只读市场类型。
///
/// 中文说明：该枚举只描述 Hyperliquid perp 账户状态来源，不表达下单、撤单、
/// 转账、签名或链上授权能力。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HyperliquidPrivateAccountMarket {
    PerpetualAccount,
}

impl HyperliquidPrivateAccountMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PerpetualAccount => "PerpetualAccount",
        }
    }

    fn event_scope(self) -> &'static str {
        match self {
            Self::PerpetualAccount => "perp",
        }
    }

    fn account_endpoint(self) -> &'static str {
        match self {
            Self::PerpetualAccount => "info:clearinghouseState",
        }
    }

    fn market_context_endpoint(self) -> &'static str {
        match self {
            Self::PerpetualAccount => "info:metaAndAssetCtxs",
        }
    }
}

/// Hyperliquid 私有账户余额快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidPrivateBalanceBatch {
    pub balance_event: NormalizedEvent,
    pub balances: Vec<VenueBalance>,
}

/// Hyperliquid 私有账户仓位快照批次。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidPrivatePositionBatch {
    pub position_event: NormalizedEvent,
    pub positions: Vec<VenuePosition>,
}

/// Binance 私有账户只读适配器。
///
/// 中文说明：该适配器只消费调用方已经获取到的私有账户 JSON 响应，负责把
/// `USER_DATA` 响应映射为余额和仓位只读快照。它不持有 API key，不生成签名，
/// 不主动联网，也不提供下单、撤单、划转或改杠杆等可变动作。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePrivateAccountAdapter {
    venue_id: VenueId,
    account_id: AccountId,
    market: BinancePrivateAccountMarket,
    max_age_ms: u64,
    balances: Vec<VenueBalance>,
    positions: Vec<VenuePosition>,
    health: VenueHealthSnapshot,
}

impl BinancePrivateAccountAdapter {
    pub fn new(
        venue_id: VenueId,
        account_id: AccountId,
        market: BinancePrivateAccountMarket,
        started_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let freshness = DataFreshness::new(started_at, started_at, max_age_ms)?;
        Ok(Self {
            venue_id: venue_id.clone(),
            account_id,
            market,
            max_age_ms,
            balances: Vec::new(),
            positions: Vec::new(),
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Unknown,
                connection: VenueConnectionStatus::Unknown,
                reason_codes: vec!["PRIVATE_ACCOUNT_NOT_INGESTED".to_owned()],
                rate_limit: None,
                source_event_id: None,
                freshness,
            },
        })
    }

    pub fn market(&self) -> BinancePrivateAccountMarket {
        self.market
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// 解析 Binance Spot `/api/v3/account` 响应为余额快照。
    pub fn ingest_spot_account_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BinancePrivateBalanceBatch> {
        self.ensure_market(BinancePrivateAccountMarket::Spot, "binance.spot.account")?;
        let raw_response_ref = raw_response_ref.into();
        let object =
            parse_binance_private_object(raw_json, &self.venue_id, ReadOnlySurface::Balance)?;

        if optional_string(
            &object,
            "accountType",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?
        .is_some_and(|account_type| account_type != "SPOT")
        {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::UnknownExternalState,
                "Binance spot account response reported a non-SPOT accountType",
            )));
        }

        let update_time_ms = required_u64(
            &object,
            "updateTime",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        let observed_at = timestamp_from_unix_millis(update_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = update_time_ms.to_string();
        let balance_event_id =
            binance_private_event_id("balance", self.market, &self.account_id, &source_sequence);

        let balance_values = required_array(
            &object,
            "balances",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        let mut balances = Vec::with_capacity(balance_values.len());
        for (index, value) in balance_values.iter().enumerate() {
            let balance_object = required_object_value(
                value,
                "balances",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            let asset = required_string(
                balance_object,
                "asset",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            validate_binance_asset_symbol(&asset)?;
            balances.push(VenueBalance {
                venue_id: self.venue_id.clone(),
                account_id: self.account_id.clone(),
                asset_id: binance_asset_id(&asset)?,
                free: parse_amount_surface_field(
                    balance_object,
                    "free",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                locked: parse_amount_surface_field(
                    balance_object,
                    "locked",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                reserved: zero_amount(),
                pending: zero_amount(),
                borrowed: zero_amount(),
                lent: zero_amount(),
                unsettled: zero_amount(),
                source_event_id: Some(balance_event_id.clone()),
                freshness,
            });
        }

        let balance_event = self.balance_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &balances,
        )?;
        self.balances = balances.clone();
        self.positions.clear();
        self.update_health(freshness, balance_event.event_id.as_str());

        Ok(BinancePrivateBalanceBatch {
            balance_event,
            balances,
        })
    }

    /// 解析 Binance USD-M `/fapi/v3/account` 响应为保证金资产余额快照。
    pub fn ingest_usdm_account_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BinancePrivateBalanceBatch> {
        self.ensure_market(
            BinancePrivateAccountMarket::UsdmFutures,
            "binance.usdm.account",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let object =
            parse_binance_private_object(raw_json, &self.venue_id, ReadOnlySurface::Balance)?;
        let asset_values = required_array(
            &object,
            "assets",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        if asset_values.is_empty() {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MissingField,
                "Binance USD-M account response contains no assets",
            )));
        }

        let mut max_update_time_ms = 0_u64;
        let mut asset_objects = Vec::with_capacity(asset_values.len());
        for (index, value) in asset_values.iter().enumerate() {
            let asset_object = required_object_value(
                value,
                "assets",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            let update_time_ms = required_u64(
                asset_object,
                "updateTime",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            max_update_time_ms = max_update_time_ms.max(update_time_ms);
            asset_objects.push(asset_object);
        }

        let observed_at = timestamp_from_unix_millis(max_update_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = max_update_time_ms.to_string();
        let balance_event_id =
            binance_private_event_id("balance", self.market, &self.account_id, &source_sequence);

        let mut balances = Vec::with_capacity(asset_objects.len());
        for asset_object in asset_objects {
            let asset = required_string(
                asset_object,
                "asset",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            validate_binance_asset_symbol(&asset)?;
            balances.push(VenueBalance {
                venue_id: self.venue_id.clone(),
                account_id: self.account_id.clone(),
                asset_id: binance_asset_id(&asset)?,
                free: parse_amount_surface_field(
                    asset_object,
                    "availableBalance",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                locked: parse_amount_surface_field(
                    asset_object,
                    "initialMargin",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                reserved: parse_amount_surface_field(
                    asset_object,
                    "openOrderInitialMargin",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                pending: zero_amount(),
                borrowed: zero_amount(),
                lent: zero_amount(),
                unsettled: zero_amount(),
                source_event_id: Some(balance_event_id.clone()),
                freshness,
            });
        }

        let balance_event = self.balance_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &balances,
        )?;
        self.balances = balances.clone();
        self.update_health(freshness, balance_event.event_id.as_str());

        Ok(BinancePrivateBalanceBatch {
            balance_event,
            balances,
        })
    }

    /// 解析 Binance Portfolio Margin `/papi/v1/account` 响应为余额快照。
    ///
    /// 中文说明：该接口返回 USD 口径账户可用额度，不是逐资产余额。这里将
    /// `virtualMaxWithdrawAmount` / `totalAvailableBalance` 映射为 USDT 保守可用额，
    /// 仅用于统一账户 live preflight 的可用资金检查，不输出原始私有金额。
    pub fn ingest_portfolio_margin_account_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BinancePrivateBalanceBatch> {
        self.ensure_market(
            BinancePrivateAccountMarket::PortfolioMargin,
            "binance.portfolio_margin.account",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let object =
            parse_binance_private_object(raw_json, &self.venue_id, ReadOnlySurface::Balance)?;
        let update_time_ms = required_u64(
            &object,
            "updateTime",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        let observed_at = timestamp_from_unix_millis(update_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = update_time_ms.to_string();
        let balance_event_id =
            binance_private_event_id("balance", self.market, &self.account_id, &source_sequence);

        let balances = vec![VenueBalance {
            venue_id: self.venue_id.clone(),
            account_id: self.account_id.clone(),
            asset_id: binance_asset_id("USDT")?,
            free: parse_amount_surface_field_any_non_empty(
                &object,
                &["totalAvailableBalance", "virtualMaxWithdrawAmount"],
                &self.venue_id,
                ReadOnlySurface::Balance,
            )?,
            locked: parse_optional_amount_surface_field_non_empty(
                &object,
                "accountInitialMargin",
                &self.venue_id,
                ReadOnlySurface::Balance,
            )?,
            reserved: parse_optional_amount_surface_field_non_empty(
                &object,
                "totalMarginOpenLoss",
                &self.venue_id,
                ReadOnlySurface::Balance,
            )?,
            pending: zero_amount(),
            borrowed: zero_amount(),
            lent: zero_amount(),
            unsettled: zero_amount(),
            source_event_id: Some(balance_event_id.clone()),
            freshness,
        }];

        let balance_event = self.balance_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &balances,
        )?;
        self.balances = balances.clone();
        self.update_health(freshness, balance_event.event_id.as_str());

        Ok(BinancePrivateBalanceBatch {
            balance_event,
            balances,
        })
    }

    /// 解析 Binance USD-M `/fapi/v3/positionRisk` 响应为仓位快照。
    ///
    /// 中文说明：`/fapi/v3/account` 的 positions 字段不提供 mark price，不能单独
    /// 构造完整 `VenuePosition`。因此非零仓位需要使用 positionRisk 快照补足
    /// entry price、mark price、unrealized PnL 和 liquidation price。
    pub fn ingest_usdm_position_risk_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BinancePrivatePositionBatch> {
        self.ensure_usdm_position_market("binance.usdm.position_risk")?;
        let raw_response_ref = raw_response_ref.into();
        let value = FlatJsonParser::new(raw_json)
            .parse_value_root()
            .map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::Position,
                    ExternalErrorClass::MalformedPayload,
                    error.to_string(),
                ))
            })?;
        let FlatJsonValue::Array(position_values) = value else {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                "Binance USD-M positionRisk response must be a JSON array",
            )));
        };

        let mut max_update_time_ms = 0_u64;
        let mut position_objects = Vec::with_capacity(position_values.len());
        for (index, value) in position_values.iter().enumerate() {
            let position_object = required_object_value(
                value,
                "positions",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            let update_time_ms = required_u64(
                position_object,
                "updateTime",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            max_update_time_ms = max_update_time_ms.max(update_time_ms);
            position_objects.push(position_object);
        }

        let observed_at = timestamp_from_unix_millis(max_update_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = max_update_time_ms.to_string();
        let position_event_id =
            binance_private_event_id("position", self.market, &self.account_id, &source_sequence);

        let mut positions = Vec::new();
        for position_object in position_objects {
            let quantity = parse_decimal_surface_field(
                position_object,
                "positionAmt",
                &self.venue_id,
                ReadOnlySurface::Position,
            )?;
            if quantity.is_zero() {
                continue;
            }

            let symbol = required_string(
                position_object,
                "symbol",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            validate_binance_symbol(&symbol)?;
            let position_side = optional_string(
                position_object,
                "positionSide",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?
            .unwrap_or_else(|| "BOTH".to_owned());
            validate_binance_position_side(&position_side)?;
            positions.push(VenuePosition {
                venue_id: self.venue_id.clone(),
                position_id: Some(binance_usdm_position_id(
                    &self.account_id,
                    &symbol,
                    &position_side,
                )?),
                account_id: self.account_id.clone(),
                instrument_id: binance_usdm_instrument_id(&symbol)?,
                quantity,
                entry_price: optional_nonzero_price_surface_field(
                    position_object,
                    "entryPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                mark_price: parse_price_surface_field(
                    position_object,
                    "markPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                unrealized_pnl: parse_pnl_surface_field_any(
                    position_object,
                    &["unRealizedProfit", "unrealizedProfit"],
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                liquidation_price: optional_nonzero_price_surface_field(
                    position_object,
                    "liquidationPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                source_event_id: Some(position_event_id.clone()),
                freshness,
            });
        }

        let position_event = self.position_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &positions,
        )?;
        self.positions = positions.clone();
        self.update_health(freshness, position_event.event_id.as_str());

        Ok(BinancePrivatePositionBatch {
            position_event,
            positions,
        })
    }

    pub fn classify_http_status(
        &self,
        surface: ReadOnlySurface,
        status_code: u16,
        detail: impl Into<String>,
    ) -> ClassifiedExternalError {
        let class = match status_code {
            408 | 504 => ExternalErrorClass::Timeout,
            418 | 429 => ExternalErrorClass::RateLimited,
            500..=599 => ExternalErrorClass::Disconnected,
            _ => ExternalErrorClass::UnknownExternalState,
        };
        ClassifiedExternalError::new(self.venue_id.clone(), surface, class, detail)
    }

    fn ensure_market(
        &self,
        expected: BinancePrivateAccountMarket,
        field: &'static str,
    ) -> VenueDataResult<()> {
        if self.market == expected {
            Ok(())
        } else {
            Err(VenueDataError::InvalidQuery {
                field,
                reason: "adapter was configured for a different Binance private account market",
            })
        }
    }

    fn ensure_usdm_position_market(&self, field: &'static str) -> VenueDataResult<()> {
        if matches!(
            self.market,
            BinancePrivateAccountMarket::UsdmFutures | BinancePrivateAccountMarket::PortfolioMargin
        ) {
            Ok(())
        } else {
            Err(VenueDataError::InvalidQuery {
                field,
                reason: "adapter was not configured for a Binance UM private position market",
            })
        }
    }

    fn balance_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        balances: &[VenueBalance],
    ) -> VenueDataResult<NormalizedEvent> {
        let asset_ids = balances
            .iter()
            .map(|balance| balance.asset_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"BinancePrivateAccountAdapter\",\"asset_ids\":{},\"balance_count\":{},\"endpoint\":{},\"freshness\":{},\"kind\":\"BinancePrivateBalanceSnapshot\",\"market\":\"{}\",\"raw_response_ref\":{},\"redaction\":\"private_account_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            json_string_array(asset_ids.iter().map(String::as_str)),
            balances.len(),
            json_string(self.market.account_endpoint()),
            freshness_payload_json(freshness),
            self.market.as_str(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: binance_private_event_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::BalanceSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-private-account".to_owned(),
            source_sequence: Some(format!(
                "binance:private:{}:{}:{source_sequence}:balance",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: binance_private_correlation_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn position_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        positions: &[VenuePosition],
    ) -> VenueDataResult<NormalizedEvent> {
        let instrument_ids = positions
            .iter()
            .map(|position| position.instrument_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"BinancePrivateAccountAdapter\",\"endpoint\":{},\"freshness\":{},\"instrument_ids\":{},\"kind\":\"BinancePrivatePositionSnapshot\",\"market\":\"{}\",\"position_count\":{},\"raw_response_ref\":{},\"redaction\":\"private_position_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            json_string(self.market.position_endpoint()),
            freshness_payload_json(freshness),
            json_string_array(instrument_ids.iter().map(String::as_str)),
            self.market.as_str(),
            positions.len(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: binance_private_event_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::PositionSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:binance-private-account".to_owned(),
            source_sequence: Some(format!(
                "binance:private:{}:{}:{source_sequence}:position",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: binance_private_correlation_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn update_health(&mut self, freshness: DataFreshness, source_event_id: &str) {
        self.health.status = if freshness.is_stale() {
            VenueHealthStatus::Degraded
        } else {
            VenueHealthStatus::Healthy
        };
        self.health.connection = VenueConnectionStatus::Connected;
        self.health.reason_codes = if freshness.is_stale() {
            vec!["DATA_STALE".to_owned()]
        } else {
            Vec::new()
        };
        self.health.source_event_id = Some(source_event_id.to_owned());
        self.health.freshness = freshness;
    }
}

impl VenueReadAdapter for BinancePrivateAccountAdapter {
    fn venue_id(&self) -> &VenueId {
        &self.venue_id
    }
}

impl MarketDataReader for BinancePrivateAccountAdapter {
    fn latest_quote(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Binance private account adapter has no market data surface".to_owned(),
        })
    }

    fn order_book(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Binance private account adapter has no order book surface".to_owned(),
        })
    }
}

impl BalanceReader for BinancePrivateAccountAdapter {
    fn balances(&self, query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>> {
        Ok(self
            .balances
            .iter()
            .filter(|balance| balance.venue_id == query.venue_id)
            .filter(|balance| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &balance.account_id)
            })
            .filter(|balance| {
                query
                    .asset_id
                    .as_ref()
                    .is_none_or(|asset_id| asset_id == &balance.asset_id)
            })
            .cloned()
            .collect())
    }
}

impl PositionReader for BinancePrivateAccountAdapter {
    fn positions(&self, query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>> {
        Ok(self
            .positions
            .iter()
            .filter(|position| position.venue_id == query.venue_id)
            .filter(|position| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &position.account_id)
            })
            .filter(|position| {
                query
                    .instrument_id
                    .as_ref()
                    .is_none_or(|instrument_id| instrument_id == &position.instrument_id)
            })
            .cloned()
            .collect())
    }
}

impl InstrumentInfoReader for BinancePrivateAccountAdapter {
    fn instruments(&self, _query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::InstrumentInfo,
            reason: "Binance private account adapter does not define instrument metadata"
                .to_owned(),
        })
    }
}

impl VenueHealthReader for BinancePrivateAccountAdapter {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
        if venue_id == &self.venue_id {
            Ok(self.health.clone())
        } else {
            Err(VenueDataError::DataUnavailable {
                venue_id: venue_id.clone(),
                surface: ReadOnlySurface::VenueHealth,
                reason: "adapter only tracks its configured venue".to_owned(),
            })
        }
    }
}

/// Bybit 私有账户只读适配器。
///
/// 中文说明：该适配器只消费调用方已经获取到的私有账户 JSON 响应，负责把
/// V5 私有只读响应映射为余额和仓位快照。它不持有 API key，不生成签名，
/// 不主动联网，也不提供下单、撤单、划转或改杠杆等可变动作。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitPrivateAccountAdapter {
    venue_id: VenueId,
    account_id: AccountId,
    market: BybitPrivateAccountMarket,
    max_age_ms: u64,
    balances: Vec<VenueBalance>,
    positions: Vec<VenuePosition>,
    health: VenueHealthSnapshot,
}

impl BybitPrivateAccountAdapter {
    pub fn new(
        venue_id: VenueId,
        account_id: AccountId,
        market: BybitPrivateAccountMarket,
        started_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let freshness = DataFreshness::new(started_at, started_at, max_age_ms)?;
        Ok(Self {
            venue_id: venue_id.clone(),
            account_id,
            market,
            max_age_ms,
            balances: Vec::new(),
            positions: Vec::new(),
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Unknown,
                connection: VenueConnectionStatus::Unknown,
                reason_codes: vec!["PRIVATE_ACCOUNT_NOT_INGESTED".to_owned()],
                rate_limit: None,
                source_event_id: None,
                freshness,
            },
        })
    }

    pub fn market(&self) -> BybitPrivateAccountMarket {
        self.market
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// 解析 Bybit V5 `/v5/account/wallet-balance` 响应为统一账户余额快照。
    pub fn ingest_unified_wallet_balance_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BybitPrivateBalanceBatch> {
        self.ensure_market(
            BybitPrivateAccountMarket::UnifiedAccount,
            "bybit.unified.wallet_balance",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let object =
            parse_bybit_private_object(raw_json, &self.venue_id, ReadOnlySurface::Balance)?;
        validate_bybit_v5_ret_code(&object, &self.venue_id, ReadOnlySurface::Balance)?;
        let time_ms = required_u64(
            &object,
            "time",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        let observed_at = timestamp_from_unix_millis(time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = time_ms.to_string();
        let balance_event_id =
            bybit_private_event_id("balance", self.market, &self.account_id, &source_sequence);
        let result = required_object_field(
            &object,
            "result",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        let accounts = required_array(
            result,
            "list",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;

        let mut balances = Vec::new();
        let mut found_unified = false;
        for (account_index, value) in accounts.iter().enumerate() {
            let account = required_object_value(
                value,
                "list",
                account_index,
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            let account_type = required_string(
                account,
                "accountType",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            if account_type != "UNIFIED" {
                continue;
            }
            found_unified = true;
            let coins = required_array(
                account,
                "coin",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            for (coin_index, value) in coins.iter().enumerate() {
                let coin = required_object_value(
                    value,
                    "coin",
                    coin_index,
                    self.venue_id.clone(),
                    ReadOnlySurface::Balance,
                )?;
                let asset = required_string(
                    coin,
                    "coin",
                    self.venue_id.clone(),
                    ReadOnlySurface::Balance,
                )?;
                validate_bybit_asset_symbol(&asset)?;
                balances.push(VenueBalance {
                    venue_id: self.venue_id.clone(),
                    account_id: self.account_id.clone(),
                    asset_id: bybit_asset_id(&asset)?,
                    free: parse_bybit_wallet_free(coin, &self.venue_id)?,
                    locked: parse_optional_amount_surface_field(
                        coin,
                        "locked",
                        &self.venue_id,
                        ReadOnlySurface::Balance,
                    )?,
                    reserved: parse_optional_amount_surface_field(
                        coin,
                        "totalOrderIM",
                        &self.venue_id,
                        ReadOnlySurface::Balance,
                    )?,
                    pending: zero_amount(),
                    borrowed: parse_optional_amount_surface_field(
                        coin,
                        "borrowAmount",
                        &self.venue_id,
                        ReadOnlySurface::Balance,
                    )?,
                    lent: zero_amount(),
                    unsettled: parse_optional_amount_surface_field(
                        coin,
                        "spotHedgingQty",
                        &self.venue_id,
                        ReadOnlySurface::Balance,
                    )?,
                    source_event_id: Some(balance_event_id.clone()),
                    freshness,
                });
            }
        }

        if !found_unified {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MissingField,
                "Bybit wallet-balance response contains no UNIFIED account row",
            )));
        }

        let balance_event = self.balance_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &balances,
        )?;
        self.balances = balances.clone();
        self.update_health(freshness, balance_event.event_id.as_str());

        Ok(BybitPrivateBalanceBatch {
            balance_event,
            balances,
        })
    }

    /// 解析 Bybit V5 `/v5/position/list?category=linear` 响应为线性永续仓位快照。
    pub fn ingest_linear_position_list_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BybitPrivatePositionBatch> {
        self.ensure_market(
            BybitPrivateAccountMarket::LinearPerpetual,
            "bybit.linear.position_list",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let object =
            parse_bybit_private_object(raw_json, &self.venue_id, ReadOnlySurface::Position)?;
        validate_bybit_v5_ret_code(&object, &self.venue_id, ReadOnlySurface::Position)?;
        let top_time_ms = required_u64(
            &object,
            "time",
            self.venue_id.clone(),
            ReadOnlySurface::Position,
        )?;
        let result = required_object_field(
            &object,
            "result",
            self.venue_id.clone(),
            ReadOnlySurface::Position,
        )?;
        let category = required_string(
            result,
            "category",
            self.venue_id.clone(),
            ReadOnlySurface::Position,
        )?;
        if category != "linear" {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::UnknownExternalState,
                format!("Bybit position/list category `{category}` is not `linear`"),
            )));
        }
        let position_values = required_array(
            result,
            "list",
            self.venue_id.clone(),
            ReadOnlySurface::Position,
        )?;

        let mut max_update_time_ms = top_time_ms;
        let mut position_objects = Vec::with_capacity(position_values.len());
        for (index, value) in position_values.iter().enumerate() {
            let position_object = required_object_value(
                value,
                "list",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            if let Some(update_time_ms) = optional_u64(position_object, "updatedTime")? {
                max_update_time_ms = max_update_time_ms.max(update_time_ms);
            }
            position_objects.push(position_object);
        }

        let observed_at = timestamp_from_unix_millis(max_update_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = max_update_time_ms.to_string();
        let position_event_id =
            bybit_private_event_id("position", self.market, &self.account_id, &source_sequence);

        let mut positions = Vec::new();
        for position_object in position_objects {
            let size = parse_decimal_surface_field(
                position_object,
                "size",
                &self.venue_id,
                ReadOnlySurface::Position,
            )?;
            if size.is_zero() {
                continue;
            }
            let side = required_string(
                position_object,
                "side",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            let quantity = bybit_signed_position_quantity(size, &side)?;
            let symbol = required_string(
                position_object,
                "symbol",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            validate_bybit_symbol(&symbol)?;
            positions.push(VenuePosition {
                venue_id: self.venue_id.clone(),
                position_id: Some(bybit_linear_position_id(&self.account_id, &symbol, &side)?),
                account_id: self.account_id.clone(),
                instrument_id: bybit_linear_instrument_id(&symbol)?,
                quantity,
                entry_price: optional_nonzero_price_surface_field(
                    position_object,
                    "avgPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                mark_price: parse_price_surface_field(
                    position_object,
                    "markPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                unrealized_pnl: parse_pnl_surface_field_any(
                    position_object,
                    &["unrealisedPnl", "unrealizedPnl"],
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                liquidation_price: optional_nonzero_price_surface_field(
                    position_object,
                    "liqPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                source_event_id: Some(position_event_id.clone()),
                freshness,
            });
        }

        let position_event = self.position_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &positions,
        )?;
        self.positions = positions.clone();
        self.update_health(freshness, position_event.event_id.as_str());

        Ok(BybitPrivatePositionBatch {
            position_event,
            positions,
        })
    }

    pub fn classify_http_status(
        &self,
        surface: ReadOnlySurface,
        status_code: u16,
        detail: impl Into<String>,
    ) -> ClassifiedExternalError {
        let class = match status_code {
            408 | 504 => ExternalErrorClass::Timeout,
            429 => ExternalErrorClass::RateLimited,
            500..=599 => ExternalErrorClass::Disconnected,
            _ => ExternalErrorClass::UnknownExternalState,
        };
        ClassifiedExternalError::new(self.venue_id.clone(), surface, class, detail)
    }

    fn ensure_market(
        &self,
        expected: BybitPrivateAccountMarket,
        field: &'static str,
    ) -> VenueDataResult<()> {
        if self.market == expected {
            Ok(())
        } else {
            Err(VenueDataError::InvalidQuery {
                field,
                reason: "adapter was configured for a different Bybit private account market",
            })
        }
    }

    fn balance_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        balances: &[VenueBalance],
    ) -> VenueDataResult<NormalizedEvent> {
        let asset_ids = balances
            .iter()
            .map(|balance| balance.asset_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"BybitPrivateAccountAdapter\",\"asset_ids\":{},\"balance_count\":{},\"endpoint\":{},\"freshness\":{},\"kind\":\"BybitPrivateBalanceSnapshot\",\"market\":\"{}\",\"raw_response_ref\":{},\"redaction\":\"private_account_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            json_string_array(asset_ids.iter().map(String::as_str)),
            balances.len(),
            json_string(self.market.endpoint()),
            freshness_payload_json(freshness),
            self.market.as_str(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: bybit_private_event_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::BalanceSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:bybit-private-account".to_owned(),
            source_sequence: Some(format!(
                "bybit:private:{}:{}:{source_sequence}:balance",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: bybit_private_correlation_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn position_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        positions: &[VenuePosition],
    ) -> VenueDataResult<NormalizedEvent> {
        let instrument_ids = positions
            .iter()
            .map(|position| position.instrument_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"BybitPrivateAccountAdapter\",\"endpoint\":{},\"freshness\":{},\"instrument_ids\":{},\"kind\":\"BybitPrivatePositionSnapshot\",\"market\":\"{}\",\"position_count\":{},\"raw_response_ref\":{},\"redaction\":\"private_position_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            json_string(self.market.endpoint()),
            freshness_payload_json(freshness),
            json_string_array(instrument_ids.iter().map(String::as_str)),
            self.market.as_str(),
            positions.len(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: bybit_private_event_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::PositionSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:bybit-private-account".to_owned(),
            source_sequence: Some(format!(
                "bybit:private:{}:{}:{source_sequence}:position",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: bybit_private_correlation_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn update_health(&mut self, freshness: DataFreshness, source_event_id: &str) {
        self.health.status = if freshness.is_stale() {
            VenueHealthStatus::Degraded
        } else {
            VenueHealthStatus::Healthy
        };
        self.health.connection = VenueConnectionStatus::Connected;
        self.health.reason_codes = if freshness.is_stale() {
            vec!["DATA_STALE".to_owned()]
        } else {
            Vec::new()
        };
        self.health.source_event_id = Some(source_event_id.to_owned());
        self.health.freshness = freshness;
    }
}

impl VenueReadAdapter for BybitPrivateAccountAdapter {
    fn venue_id(&self) -> &VenueId {
        &self.venue_id
    }
}

impl MarketDataReader for BybitPrivateAccountAdapter {
    fn latest_quote(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Bybit private account adapter has no market data surface".to_owned(),
        })
    }

    fn order_book(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Bybit private account adapter has no order book surface".to_owned(),
        })
    }
}

impl BalanceReader for BybitPrivateAccountAdapter {
    fn balances(&self, query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>> {
        Ok(self
            .balances
            .iter()
            .filter(|balance| balance.venue_id == query.venue_id)
            .filter(|balance| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &balance.account_id)
            })
            .filter(|balance| {
                query
                    .asset_id
                    .as_ref()
                    .is_none_or(|asset_id| asset_id == &balance.asset_id)
            })
            .cloned()
            .collect())
    }
}

impl PositionReader for BybitPrivateAccountAdapter {
    fn positions(&self, query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>> {
        Ok(self
            .positions
            .iter()
            .filter(|position| position.venue_id == query.venue_id)
            .filter(|position| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &position.account_id)
            })
            .filter(|position| {
                query
                    .instrument_id
                    .as_ref()
                    .is_none_or(|instrument_id| instrument_id == &position.instrument_id)
            })
            .cloned()
            .collect())
    }
}

impl InstrumentInfoReader for BybitPrivateAccountAdapter {
    fn instruments(&self, _query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::InstrumentInfo,
            reason: "Bybit private account adapter does not define instrument metadata".to_owned(),
        })
    }
}

impl VenueHealthReader for BybitPrivateAccountAdapter {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
        if venue_id == &self.venue_id {
            Ok(self.health.clone())
        } else {
            Err(VenueDataError::DataUnavailable {
                venue_id: venue_id.clone(),
                surface: ReadOnlySurface::VenueHealth,
                reason: "adapter only tracks its configured venue".to_owned(),
            })
        }
    }
}

/// OKX 私有账户只读适配器。
///
/// 中文说明：该适配器只消费调用方已经获取到的 OKX V5 私有账户 JSON 响应，
/// 负责把 `/api/v5/account/balance` 映射为余额只读快照。它不持有 API key、
/// 不生成签名、不主动联网，也不提供下单、撤单、划转或改杠杆等可变动作。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OkxPrivateAccountAdapter {
    venue_id: VenueId,
    account_id: AccountId,
    market: OkxPrivateAccountMarket,
    max_age_ms: u64,
    balances: Vec<VenueBalance>,
    positions: Vec<VenuePosition>,
    health: VenueHealthSnapshot,
}

impl OkxPrivateAccountAdapter {
    pub fn new(
        venue_id: VenueId,
        account_id: AccountId,
        market: OkxPrivateAccountMarket,
        started_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let freshness = DataFreshness::new(started_at, started_at, max_age_ms)?;
        Ok(Self {
            venue_id: venue_id.clone(),
            account_id,
            market,
            max_age_ms,
            balances: Vec::new(),
            positions: Vec::new(),
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Unknown,
                connection: VenueConnectionStatus::Unknown,
                reason_codes: vec!["PRIVATE_ACCOUNT_NOT_INGESTED".to_owned()],
                rate_limit: None,
                source_event_id: None,
                freshness,
            },
        })
    }

    pub fn market(&self) -> OkxPrivateAccountMarket {
        self.market
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// 解析 OKX V5 `/api/v5/account/balance` 响应为交易账户余额快照。
    pub fn ingest_account_balance_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<OkxPrivateBalanceBatch> {
        let raw_response_ref = raw_response_ref.into();
        let object = parse_okx_private_object(raw_json, &self.venue_id, ReadOnlySurface::Balance)?;
        validate_okx_v5_code(&object, &self.venue_id, ReadOnlySurface::Balance)?;
        let accounts = required_array(
            &object,
            "data",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        if accounts.is_empty() {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MissingField,
                "OKX account balance response contains no data rows",
            )));
        }

        let mut max_update_time_ms = 0_u64;
        let mut account_objects = Vec::with_capacity(accounts.len());
        for (account_index, value) in accounts.iter().enumerate() {
            let account = required_object_value(
                value,
                "data",
                account_index,
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            let update_time_ms = required_u64(
                account,
                "uTime",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            max_update_time_ms = max_update_time_ms.max(update_time_ms);
            account_objects.push(account);
        }
        let observed_at = timestamp_from_unix_millis(max_update_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = max_update_time_ms.to_string();
        let balance_event_id =
            okx_private_event_id("balance", self.market, &self.account_id, &source_sequence);

        let mut balances = Vec::new();
        for account in account_objects {
            let details = required_array(
                account,
                "details",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            if details.is_empty() {
                return Err(VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::Balance,
                    ExternalErrorClass::MissingField,
                    "OKX account balance response contains an empty details array",
                )));
            }
            for (detail_index, value) in details.iter().enumerate() {
                let detail = required_object_value(
                    value,
                    "details",
                    detail_index,
                    self.venue_id.clone(),
                    ReadOnlySurface::Balance,
                )?;
                let asset = required_string(
                    detail,
                    "ccy",
                    self.venue_id.clone(),
                    ReadOnlySurface::Balance,
                )?;
                validate_okx_asset_symbol(&asset)?;
                balances.push(VenueBalance {
                    venue_id: self.venue_id.clone(),
                    account_id: self.account_id.clone(),
                    asset_id: okx_asset_id(&asset)?,
                    free: parse_amount_surface_field(
                        detail,
                        "availBal",
                        &self.venue_id,
                        ReadOnlySurface::Balance,
                    )?,
                    locked: parse_optional_amount_surface_field(
                        detail,
                        "frozenBal",
                        &self.venue_id,
                        ReadOnlySurface::Balance,
                    )?,
                    reserved: parse_optional_amount_surface_field(
                        detail,
                        "ordFrozen",
                        &self.venue_id,
                        ReadOnlySurface::Balance,
                    )?,
                    pending: zero_amount(),
                    borrowed: parse_optional_amount_surface_field(
                        detail,
                        "liab",
                        &self.venue_id,
                        ReadOnlySurface::Balance,
                    )?,
                    lent: zero_amount(),
                    unsettled: zero_amount(),
                    source_event_id: Some(balance_event_id.clone()),
                    freshness,
                });
            }
        }

        let balance_event = self.balance_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &balances,
        )?;
        self.balances = balances.clone();
        self.positions.clear();
        self.update_health(freshness, balance_event.event_id.as_str());

        Ok(OkxPrivateBalanceBatch {
            balance_event,
            balances,
        })
    }

    /// 解析 OKX V5 `/api/v5/account/positions` 响应为交易账户仓位快照。
    pub fn ingest_account_positions_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<OkxPrivatePositionBatch> {
        let raw_response_ref = raw_response_ref.into();
        let object = parse_okx_private_object(raw_json, &self.venue_id, ReadOnlySurface::Position)?;
        validate_okx_v5_code(&object, &self.venue_id, ReadOnlySurface::Position)?;
        let position_values = required_array(
            &object,
            "data",
            self.venue_id.clone(),
            ReadOnlySurface::Position,
        )?;

        let mut max_update_time_ms = timestamp_to_unix_millis(ingested_at)?;
        let mut position_objects = Vec::with_capacity(position_values.len());
        for (index, value) in position_values.iter().enumerate() {
            let position = required_object_value(
                value,
                "data",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            if let Some(update_time_ms) = optional_u64(position, "uTime")? {
                max_update_time_ms = max_update_time_ms.max(update_time_ms);
            }
            position_objects.push(position);
        }

        let observed_at = timestamp_from_unix_millis(max_update_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = max_update_time_ms.to_string();
        let position_event_id =
            okx_private_event_id("position", self.market, &self.account_id, &source_sequence);

        let mut positions = Vec::new();
        for position_object in position_objects {
            let quantity = parse_decimal_surface_field(
                position_object,
                "pos",
                &self.venue_id,
                ReadOnlySurface::Position,
            )?;
            if quantity.is_zero() {
                continue;
            }

            let inst_id = required_string(
                position_object,
                "instId",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            validate_okx_inst_id(&inst_id)?;
            let pos_side = optional_string(
                position_object,
                "posSide",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?
            .unwrap_or_else(|| "net".to_owned());
            let quantity = okx_signed_position_quantity(quantity, &pos_side)?;
            positions.push(VenuePosition {
                venue_id: self.venue_id.clone(),
                position_id: Some(okx_swap_position_id(&self.account_id, &inst_id, &pos_side)?),
                account_id: self.account_id.clone(),
                instrument_id: okx_swap_instrument_id(&inst_id)?,
                quantity,
                entry_price: optional_nonzero_price_surface_field(
                    position_object,
                    "avgPx",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                mark_price: parse_price_surface_field(
                    position_object,
                    "markPx",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                unrealized_pnl: parse_pnl_surface_field_any(
                    position_object,
                    &["upl", "uplLastPx"],
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                liquidation_price: optional_nonzero_price_surface_field(
                    position_object,
                    "liqPx",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                source_event_id: Some(position_event_id.clone()),
                freshness,
            });
        }

        let position_event = self.position_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &positions,
        )?;
        self.positions = positions.clone();
        self.update_health(freshness, position_event.event_id.as_str());

        Ok(OkxPrivatePositionBatch {
            position_event,
            positions,
        })
    }

    pub fn classify_http_status(
        &self,
        surface: ReadOnlySurface,
        status_code: u16,
        detail: impl Into<String>,
    ) -> ClassifiedExternalError {
        let class = match status_code {
            408 | 504 => ExternalErrorClass::Timeout,
            429 => ExternalErrorClass::RateLimited,
            500..=599 => ExternalErrorClass::Disconnected,
            _ => ExternalErrorClass::UnknownExternalState,
        };
        ClassifiedExternalError::new(self.venue_id.clone(), surface, class, detail)
    }

    fn balance_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        balances: &[VenueBalance],
    ) -> VenueDataResult<NormalizedEvent> {
        let asset_ids = balances
            .iter()
            .map(|balance| balance.asset_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"OkxPrivateAccountAdapter\",\"asset_ids\":{},\"balance_count\":{},\"endpoint\":{},\"freshness\":{},\"kind\":\"OkxPrivateBalanceSnapshot\",\"market\":\"{}\",\"raw_response_ref\":{},\"redaction\":\"private_account_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            json_string_array(asset_ids.iter().map(String::as_str)),
            balances.len(),
            json_string(self.market.endpoint()),
            freshness_payload_json(freshness),
            self.market.as_str(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: okx_private_event_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::BalanceSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:okx-private-account".to_owned(),
            source_sequence: Some(format!(
                "okx:private:{}:{}:{source_sequence}:balance",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: okx_private_correlation_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn position_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        positions: &[VenuePosition],
    ) -> VenueDataResult<NormalizedEvent> {
        let instrument_ids = positions
            .iter()
            .map(|position| position.instrument_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"OkxPrivateAccountAdapter\",\"endpoint\":\"/api/v5/account/positions\",\"freshness\":{},\"instrument_ids\":{},\"kind\":\"OkxPrivatePositionSnapshot\",\"market\":\"{}\",\"position_count\":{},\"raw_response_ref\":{},\"redaction\":\"private_position_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            freshness_payload_json(freshness),
            json_string_array(instrument_ids.iter().map(String::as_str)),
            self.market.as_str(),
            positions.len(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: okx_private_event_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::PositionSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:okx-private-account".to_owned(),
            source_sequence: Some(format!(
                "okx:private:{}:{}:{source_sequence}:position",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: okx_private_correlation_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn update_health(&mut self, freshness: DataFreshness, source_event_id: &str) {
        self.health.status = if freshness.is_stale() {
            VenueHealthStatus::Degraded
        } else {
            VenueHealthStatus::Healthy
        };
        self.health.connection = VenueConnectionStatus::Connected;
        self.health.reason_codes = if freshness.is_stale() {
            vec!["DATA_STALE".to_owned()]
        } else {
            Vec::new()
        };
        self.health.source_event_id = Some(source_event_id.to_owned());
        self.health.freshness = freshness;
    }
}

impl VenueReadAdapter for OkxPrivateAccountAdapter {
    fn venue_id(&self) -> &VenueId {
        &self.venue_id
    }
}

impl MarketDataReader for OkxPrivateAccountAdapter {
    fn latest_quote(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "OKX private account adapter has no market data surface".to_owned(),
        })
    }

    fn order_book(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "OKX private account adapter has no order book surface".to_owned(),
        })
    }
}

impl BalanceReader for OkxPrivateAccountAdapter {
    fn balances(&self, query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>> {
        Ok(self
            .balances
            .iter()
            .filter(|balance| balance.venue_id == query.venue_id)
            .filter(|balance| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &balance.account_id)
            })
            .filter(|balance| {
                query
                    .asset_id
                    .as_ref()
                    .is_none_or(|asset_id| asset_id == &balance.asset_id)
            })
            .cloned()
            .collect())
    }
}

impl PositionReader for OkxPrivateAccountAdapter {
    fn positions(&self, query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>> {
        Ok(self
            .positions
            .iter()
            .filter(|position| position.venue_id == query.venue_id)
            .filter(|position| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &position.account_id)
            })
            .filter(|position| {
                query
                    .instrument_id
                    .as_ref()
                    .is_none_or(|instrument_id| instrument_id == &position.instrument_id)
            })
            .cloned()
            .collect())
    }
}

impl InstrumentInfoReader for OkxPrivateAccountAdapter {
    fn instruments(&self, _query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::InstrumentInfo,
            reason: "OKX private account adapter does not define instrument metadata".to_owned(),
        })
    }
}

impl VenueHealthReader for OkxPrivateAccountAdapter {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
        if venue_id == &self.venue_id {
            Ok(self.health.clone())
        } else {
            Err(VenueDataError::DataUnavailable {
                venue_id: venue_id.clone(),
                surface: ReadOnlySurface::VenueHealth,
                reason: "adapter only tracks its configured venue".to_owned(),
            })
        }
    }
}

/// Bitget 私有账户只读适配器。
///
/// 中文说明：该适配器只消费调用方已经获取到的 Bitget 私有账户 JSON 响应，负责把
/// Spot 资产和 USDT-FUTURES 账户映射为余额只读快照。它不持有 API key、
/// 不生成签名、不主动联网，也不提供下单、撤单、划转或改杠杆等可变动作。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BitgetPrivateAccountAdapter {
    venue_id: VenueId,
    account_id: AccountId,
    market: BitgetPrivateAccountMarket,
    max_age_ms: u64,
    balances: Vec<VenueBalance>,
    positions: Vec<VenuePosition>,
    health: VenueHealthSnapshot,
}

impl BitgetPrivateAccountAdapter {
    pub fn new(
        venue_id: VenueId,
        account_id: AccountId,
        market: BitgetPrivateAccountMarket,
        started_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let freshness = DataFreshness::new(started_at, started_at, max_age_ms)?;
        Ok(Self {
            venue_id: venue_id.clone(),
            account_id,
            market,
            max_age_ms,
            balances: Vec::new(),
            positions: Vec::new(),
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Unknown,
                connection: VenueConnectionStatus::Unknown,
                reason_codes: vec!["PRIVATE_ACCOUNT_NOT_INGESTED".to_owned()],
                rate_limit: None,
                source_event_id: None,
                freshness,
            },
        })
    }

    pub fn market(&self) -> BitgetPrivateAccountMarket {
        self.market
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// 解析 Bitget Spot `/api/v2/spot/account/assets` 响应为余额快照。
    pub fn ingest_spot_assets_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BitgetPrivateBalanceBatch> {
        self.ensure_market(
            BitgetPrivateAccountMarket::SpotAccount,
            "bitget.spot.account.assets",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let object =
            parse_bitget_private_object(raw_json, &self.venue_id, ReadOnlySurface::Balance)?;
        validate_bitget_code(&object, &self.venue_id, ReadOnlySurface::Balance)?;
        let request_time_ms = required_u64(
            &object,
            "requestTime",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        let observed_at = timestamp_from_unix_millis(request_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = request_time_ms.to_string();
        let balance_event_id =
            bitget_private_event_id("balance", self.market, &self.account_id, &source_sequence);

        let assets = required_array(
            &object,
            "data",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        let mut balances = Vec::with_capacity(assets.len());
        for (index, value) in assets.iter().enumerate() {
            let asset_object = required_object_value(
                value,
                "data",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            let asset = required_string(
                asset_object,
                "coin",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            let asset = bitget_asset_symbol(&asset)?;
            balances.push(VenueBalance {
                venue_id: self.venue_id.clone(),
                account_id: self.account_id.clone(),
                asset_id: bitget_asset_id(&asset)?,
                free: parse_amount_surface_field(
                    asset_object,
                    "available",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                locked: parse_optional_amount_surface_field(
                    asset_object,
                    "frozen",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                reserved: parse_optional_amount_surface_field(
                    asset_object,
                    "locked",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                pending: zero_amount(),
                borrowed: zero_amount(),
                lent: zero_amount(),
                unsettled: zero_amount(),
                source_event_id: Some(balance_event_id.clone()),
                freshness,
            });
        }

        let balance_event = self.balance_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &balances,
        )?;
        self.balances = balances.clone();
        self.positions.clear();
        self.update_health(freshness, balance_event.event_id.as_str());

        Ok(BitgetPrivateBalanceBatch {
            balance_event,
            balances,
        })
    }

    /// 解析 Bitget USDT-FUTURES `/api/v2/mix/account/accounts` 响应为余额快照。
    pub fn ingest_usdt_futures_accounts_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BitgetPrivateBalanceBatch> {
        self.ensure_market(
            BitgetPrivateAccountMarket::UsdtFuturesAccount,
            "bitget.mix.account.accounts",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let object =
            parse_bitget_private_object(raw_json, &self.venue_id, ReadOnlySurface::Balance)?;
        validate_bitget_code(&object, &self.venue_id, ReadOnlySurface::Balance)?;
        let request_time_ms = required_u64(
            &object,
            "requestTime",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        let observed_at = timestamp_from_unix_millis(request_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = request_time_ms.to_string();
        let balance_event_id =
            bitget_private_event_id("balance", self.market, &self.account_id, &source_sequence);

        let accounts = required_array(
            &object,
            "data",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        if accounts.is_empty() {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MissingField,
                "Bitget USDT-FUTURES account response contains no data rows",
            )));
        }
        let mut balances = Vec::with_capacity(accounts.len());
        for (index, value) in accounts.iter().enumerate() {
            let account = required_object_value(
                value,
                "data",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            let asset = required_string(
                account,
                "marginCoin",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            let asset = bitget_asset_symbol(&asset)?;
            balances.push(VenueBalance {
                venue_id: self.venue_id.clone(),
                account_id: self.account_id.clone(),
                asset_id: bitget_asset_id(&asset)?,
                free: parse_amount_surface_field(
                    account,
                    "available",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                locked: parse_optional_amount_surface_field(
                    account,
                    "locked",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                reserved: zero_amount(),
                pending: zero_amount(),
                borrowed: zero_amount(),
                lent: zero_amount(),
                unsettled: zero_amount(),
                source_event_id: Some(balance_event_id.clone()),
                freshness,
            });
        }

        let balance_event = self.balance_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &balances,
        )?;
        self.balances = balances.clone();
        self.positions.clear();
        self.update_health(freshness, balance_event.event_id.as_str());

        Ok(BitgetPrivateBalanceBatch {
            balance_event,
            balances,
        })
    }

    /// 解析 Bitget USDT-FUTURES `/api/v2/mix/position/all-position` 响应为仓位快照。
    pub fn ingest_usdt_futures_positions_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<BitgetPrivatePositionBatch> {
        self.ensure_market(
            BitgetPrivateAccountMarket::UsdtFuturesAccount,
            "bitget.mix.position.all_position",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let object =
            parse_bitget_private_object(raw_json, &self.venue_id, ReadOnlySurface::Position)?;
        validate_bitget_code(&object, &self.venue_id, ReadOnlySurface::Position)?;
        let request_time_ms = required_u64(
            &object,
            "requestTime",
            self.venue_id.clone(),
            ReadOnlySurface::Position,
        )?;
        let position_values = required_array(
            &object,
            "data",
            self.venue_id.clone(),
            ReadOnlySurface::Position,
        )?;

        let mut max_update_time_ms = request_time_ms;
        let mut position_objects = Vec::with_capacity(position_values.len());
        for (index, value) in position_values.iter().enumerate() {
            let position = required_object_value(
                value,
                "data",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            if let Some(update_time_ms) = optional_u64(position, "uTime")? {
                max_update_time_ms = max_update_time_ms.max(update_time_ms);
            }
            position_objects.push(position);
        }

        let observed_at = timestamp_from_unix_millis(max_update_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = max_update_time_ms.to_string();
        let position_event_id =
            bitget_private_event_id("position", self.market, &self.account_id, &source_sequence);

        let mut positions = Vec::new();
        for position_object in position_objects {
            let total = parse_decimal_surface_field(
                position_object,
                "total",
                &self.venue_id,
                ReadOnlySurface::Position,
            )?;
            if total.is_zero() {
                continue;
            }
            let hold_side = required_string(
                position_object,
                "holdSide",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            let quantity = bitget_signed_position_quantity(total, &hold_side)?;
            let symbol = required_string(
                position_object,
                "symbol",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            let symbol = bitget_symbol(&symbol)?;
            positions.push(VenuePosition {
                venue_id: self.venue_id.clone(),
                position_id: Some(bitget_usdt_futures_position_id(
                    &self.account_id,
                    &symbol,
                    &hold_side,
                )?),
                account_id: self.account_id.clone(),
                instrument_id: bitget_usdt_futures_instrument_id(&symbol)?,
                quantity,
                entry_price: optional_nonzero_price_surface_field_any(
                    position_object,
                    &["openPriceAvg", "averageOpenPrice"],
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                mark_price: parse_price_surface_field(
                    position_object,
                    "markPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                unrealized_pnl: parse_pnl_surface_field_any(
                    position_object,
                    &["unrealizedPL", "unrealizedPnl"],
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                liquidation_price: optional_nonzero_price_surface_field(
                    position_object,
                    "liquidationPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                source_event_id: Some(position_event_id.clone()),
                freshness,
            });
        }

        let position_event = self.position_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &positions,
        )?;
        self.positions = positions.clone();
        self.update_health(freshness, position_event.event_id.as_str());

        Ok(BitgetPrivatePositionBatch {
            position_event,
            positions,
        })
    }

    pub fn classify_http_status(
        &self,
        surface: ReadOnlySurface,
        status_code: u16,
        detail: impl Into<String>,
    ) -> ClassifiedExternalError {
        let class = match status_code {
            408 | 504 => ExternalErrorClass::Timeout,
            429 => ExternalErrorClass::RateLimited,
            500..=599 => ExternalErrorClass::Disconnected,
            _ => ExternalErrorClass::UnknownExternalState,
        };
        ClassifiedExternalError::new(self.venue_id.clone(), surface, class, detail)
    }

    fn ensure_market(
        &self,
        expected: BitgetPrivateAccountMarket,
        field: &'static str,
    ) -> VenueDataResult<()> {
        if self.market == expected {
            Ok(())
        } else {
            Err(VenueDataError::InvalidQuery {
                field,
                reason: "Bitget private account adapter market does not match this ingestion path",
            })
        }
    }

    fn balance_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        balances: &[VenueBalance],
    ) -> VenueDataResult<NormalizedEvent> {
        let asset_ids = balances
            .iter()
            .map(|balance| balance.asset_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"BitgetPrivateAccountAdapter\",\"asset_ids\":{},\"balance_count\":{},\"endpoint\":{},\"freshness\":{},\"kind\":\"BitgetPrivateBalanceSnapshot\",\"market\":\"{}\",\"raw_response_ref\":{},\"redaction\":\"private_account_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            json_string_array(asset_ids.iter().map(String::as_str)),
            balances.len(),
            json_string(self.market.endpoint()),
            freshness_payload_json(freshness),
            self.market.as_str(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: bitget_private_event_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::BalanceSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:bitget-private-account".to_owned(),
            source_sequence: Some(format!(
                "bitget:private:{}:{}:{source_sequence}:balance",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: bitget_private_correlation_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn position_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        positions: &[VenuePosition],
    ) -> VenueDataResult<NormalizedEvent> {
        let instrument_ids = positions
            .iter()
            .map(|position| position.instrument_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"BitgetPrivateAccountAdapter\",\"endpoint\":\"/api/v2/mix/position/all-position\",\"freshness\":{},\"instrument_ids\":{},\"kind\":\"BitgetPrivatePositionSnapshot\",\"market\":\"{}\",\"position_count\":{},\"raw_response_ref\":{},\"redaction\":\"private_position_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            freshness_payload_json(freshness),
            json_string_array(instrument_ids.iter().map(String::as_str)),
            self.market.as_str(),
            positions.len(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: bitget_private_event_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::PositionSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:bitget-private-account".to_owned(),
            source_sequence: Some(format!(
                "bitget:private:{}:{}:{source_sequence}:position",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: bitget_private_correlation_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn update_health(&mut self, freshness: DataFreshness, source_event_id: &str) {
        self.health.status = if freshness.is_stale() {
            VenueHealthStatus::Degraded
        } else {
            VenueHealthStatus::Healthy
        };
        self.health.connection = VenueConnectionStatus::Connected;
        self.health.reason_codes = if freshness.is_stale() {
            vec!["DATA_STALE".to_owned()]
        } else {
            Vec::new()
        };
        self.health.source_event_id = Some(source_event_id.to_owned());
        self.health.freshness = freshness;
    }
}

impl VenueReadAdapter for BitgetPrivateAccountAdapter {
    fn venue_id(&self) -> &VenueId {
        &self.venue_id
    }
}

impl MarketDataReader for BitgetPrivateAccountAdapter {
    fn latest_quote(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Bitget private account adapter has no market data surface".to_owned(),
        })
    }

    fn order_book(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Bitget private account adapter has no order book surface".to_owned(),
        })
    }
}

impl BalanceReader for BitgetPrivateAccountAdapter {
    fn balances(&self, query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>> {
        Ok(self
            .balances
            .iter()
            .filter(|balance| balance.venue_id == query.venue_id)
            .filter(|balance| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &balance.account_id)
            })
            .filter(|balance| {
                query
                    .asset_id
                    .as_ref()
                    .is_none_or(|asset_id| asset_id == &balance.asset_id)
            })
            .cloned()
            .collect())
    }
}

impl PositionReader for BitgetPrivateAccountAdapter {
    fn positions(&self, query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>> {
        Ok(self
            .positions
            .iter()
            .filter(|position| position.venue_id == query.venue_id)
            .filter(|position| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &position.account_id)
            })
            .filter(|position| {
                query
                    .instrument_id
                    .as_ref()
                    .is_none_or(|instrument_id| instrument_id == &position.instrument_id)
            })
            .cloned()
            .collect())
    }
}

impl InstrumentInfoReader for BitgetPrivateAccountAdapter {
    fn instruments(&self, _query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::InstrumentInfo,
            reason: "Bitget private account adapter does not define instrument metadata".to_owned(),
        })
    }
}

impl VenueHealthReader for BitgetPrivateAccountAdapter {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
        if venue_id == &self.venue_id {
            Ok(self.health.clone())
        } else {
            Err(VenueDataError::DataUnavailable {
                venue_id: venue_id.clone(),
                surface: ReadOnlySurface::VenueHealth,
                reason: "adapter only tracks its configured venue".to_owned(),
            })
        }
    }
}

/// Aster 私有账户只读适配器。
///
/// 中文说明：该适配器只消费调用方已经获取到的 Aster Futures V3 `USER_DATA`
/// JSON 响应，负责把余额和仓位映射为只读快照。它不持有 API key、不生成签名、
/// 不主动联网，也不提供下单、撤单、划转、改杠杆或链上授权等可变动作。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AsterPrivateAccountAdapter {
    venue_id: VenueId,
    account_id: AccountId,
    market: AsterPrivateAccountMarket,
    max_age_ms: u64,
    balances: Vec<VenueBalance>,
    positions: Vec<VenuePosition>,
    health: VenueHealthSnapshot,
}

impl AsterPrivateAccountAdapter {
    pub fn new(
        venue_id: VenueId,
        account_id: AccountId,
        market: AsterPrivateAccountMarket,
        started_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let freshness = DataFreshness::new(started_at, started_at, max_age_ms)?;
        Ok(Self {
            venue_id: venue_id.clone(),
            account_id,
            market,
            max_age_ms,
            balances: Vec::new(),
            positions: Vec::new(),
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Unknown,
                connection: VenueConnectionStatus::Unknown,
                reason_codes: vec!["PRIVATE_ACCOUNT_NOT_INGESTED".to_owned()],
                rate_limit: None,
                source_event_id: None,
                freshness,
            },
        })
    }

    pub fn market(&self) -> AsterPrivateAccountMarket {
        self.market
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// 解析 Aster Futures V3 `/fapi/v3/balance` 响应为余额快照。
    pub fn ingest_usdt_futures_balance_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<AsterPrivateBalanceBatch> {
        self.ensure_market(
            AsterPrivateAccountMarket::UsdtFuturesAccount,
            "aster.futures.balance",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let value = parse_aster_private_value(raw_json, &self.venue_id, ReadOnlySurface::Balance)?;
        let FlatJsonValue::Array(balance_values) = value else {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                "Aster futures balance response must be a JSON array",
            )));
        };
        if balance_values.is_empty() {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MissingField,
                "Aster futures balance response contains no balances",
            )));
        }

        let mut max_update_time_ms = 0_u64;
        let mut balance_objects = Vec::with_capacity(balance_values.len());
        for (index, value) in balance_values.iter().enumerate() {
            let balance_object = required_object_value(
                value,
                "balances",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            let update_time_ms = required_u64(
                balance_object,
                "updateTime",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            max_update_time_ms = max_update_time_ms.max(update_time_ms);
            balance_objects.push(balance_object);
        }

        let observed_at = timestamp_from_unix_millis(max_update_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = max_update_time_ms.to_string();
        let balance_event_id =
            aster_private_event_id("balance", self.market, &self.account_id, &source_sequence);

        let mut balances = Vec::with_capacity(balance_objects.len());
        for balance_object in balance_objects {
            let asset = required_string(
                balance_object,
                "asset",
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
            )?;
            let asset = aster_asset_symbol(&asset)?;
            balances.push(VenueBalance {
                venue_id: self.venue_id.clone(),
                account_id: self.account_id.clone(),
                asset_id: aster_asset_id(&asset)?,
                free: parse_amount_surface_field(
                    balance_object,
                    "availableBalance",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                locked: parse_non_negative_amount_difference_surface_fields(
                    balance_object,
                    "balance",
                    "availableBalance",
                    &self.venue_id,
                    ReadOnlySurface::Balance,
                )?,
                reserved: zero_amount(),
                pending: zero_amount(),
                borrowed: zero_amount(),
                lent: zero_amount(),
                unsettled: zero_amount(),
                source_event_id: Some(balance_event_id.clone()),
                freshness,
            });
        }

        let balance_event = self.balance_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &balances,
        )?;
        self.balances = balances.clone();
        self.positions.clear();
        self.update_health(freshness, balance_event.event_id.as_str());

        Ok(AsterPrivateBalanceBatch {
            balance_event,
            balances,
        })
    }

    /// 解析 Aster Futures V3 `/fapi/v3/positionRisk` 响应为仓位快照。
    ///
    /// 中文说明：非零仓位必须携带 mark price、entry price、unrealized PnL 等风险
    /// 字段；缺失时按外部数据不完整处理，调用方应失败关闭。
    pub fn ingest_usdt_futures_position_risk_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<AsterPrivatePositionBatch> {
        self.ensure_market(
            AsterPrivateAccountMarket::UsdtFuturesAccount,
            "aster.futures.position_risk",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let value = parse_aster_private_value(raw_json, &self.venue_id, ReadOnlySurface::Position)?;
        let FlatJsonValue::Array(position_values) = value else {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                "Aster futures positionRisk response must be a JSON array",
            )));
        };

        let mut max_update_time_ms = 0_u64;
        let mut has_nonzero_position = false;
        let mut position_objects = Vec::with_capacity(position_values.len());
        for (index, value) in position_values.iter().enumerate() {
            let position_object = required_object_value(
                value,
                "positions",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            let quantity = parse_decimal_surface_field(
                position_object,
                "positionAmt",
                &self.venue_id,
                ReadOnlySurface::Position,
            )?;
            has_nonzero_position |= !quantity.is_zero();
            let update_time_ms = required_u64(
                position_object,
                "updateTime",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            max_update_time_ms = max_update_time_ms.max(update_time_ms);
            position_objects.push(position_object);
        }

        if !has_nonzero_position && max_update_time_ms == 0 {
            max_update_time_ms = timestamp_to_unix_millis(ingested_at)?;
        }

        let observed_at = timestamp_from_unix_millis(max_update_time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = max_update_time_ms.to_string();
        let position_event_id =
            aster_private_event_id("position", self.market, &self.account_id, &source_sequence);

        let mut positions = Vec::new();
        for position_object in position_objects {
            let quantity = parse_decimal_surface_field(
                position_object,
                "positionAmt",
                &self.venue_id,
                ReadOnlySurface::Position,
            )?;
            if quantity.is_zero() {
                continue;
            }

            let symbol = required_string(
                position_object,
                "symbol",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            let symbol = aster_symbol(&symbol)?;
            let position_side = optional_string(
                position_object,
                "positionSide",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?
            .unwrap_or_else(|| "BOTH".to_owned());
            validate_aster_position_side(&position_side)?;
            positions.push(VenuePosition {
                venue_id: self.venue_id.clone(),
                position_id: Some(aster_usdt_futures_position_id(
                    &self.account_id,
                    &symbol,
                    &position_side,
                )?),
                account_id: self.account_id.clone(),
                instrument_id: aster_usdt_futures_instrument_id(&symbol)?,
                quantity,
                entry_price: optional_nonzero_price_surface_field(
                    position_object,
                    "entryPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                mark_price: parse_price_surface_field(
                    position_object,
                    "markPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                unrealized_pnl: parse_pnl_surface_field_any(
                    position_object,
                    &["unRealizedProfit", "unrealizedProfit"],
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                liquidation_price: optional_nonzero_price_surface_field(
                    position_object,
                    "liquidationPrice",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                source_event_id: Some(position_event_id.clone()),
                freshness,
            });
        }

        let position_event = self.position_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &positions,
        )?;
        self.positions = positions.clone();
        self.update_health(freshness, position_event.event_id.as_str());

        Ok(AsterPrivatePositionBatch {
            position_event,
            positions,
        })
    }

    pub fn classify_http_status(
        &self,
        surface: ReadOnlySurface,
        status_code: u16,
        detail: impl Into<String>,
    ) -> ClassifiedExternalError {
        let class = match status_code {
            408 | 504 => ExternalErrorClass::Timeout,
            418 | 429 => ExternalErrorClass::RateLimited,
            500..=599 => ExternalErrorClass::Disconnected,
            _ => ExternalErrorClass::UnknownExternalState,
        };
        ClassifiedExternalError::new(self.venue_id.clone(), surface, class, detail)
    }

    fn ensure_market(
        &self,
        expected: AsterPrivateAccountMarket,
        field: &'static str,
    ) -> VenueDataResult<()> {
        if self.market == expected {
            Ok(())
        } else {
            Err(VenueDataError::InvalidQuery {
                field,
                reason: "Aster private account adapter market does not match this ingestion path",
            })
        }
    }

    fn balance_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        balances: &[VenueBalance],
    ) -> VenueDataResult<NormalizedEvent> {
        let asset_ids = balances
            .iter()
            .map(|balance| balance.asset_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"AsterPrivateAccountAdapter\",\"asset_ids\":{},\"balance_count\":{},\"endpoint\":{},\"freshness\":{},\"kind\":\"AsterPrivateBalanceSnapshot\",\"market\":\"{}\",\"raw_response_ref\":{},\"redaction\":\"private_account_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            json_string_array(asset_ids.iter().map(String::as_str)),
            balances.len(),
            json_string(self.market.balance_endpoint()),
            freshness_payload_json(freshness),
            self.market.as_str(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: aster_private_event_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::BalanceSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:aster-private-account".to_owned(),
            source_sequence: Some(format!(
                "aster:private:{}:{}:{source_sequence}:balance",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: aster_private_correlation_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn position_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        positions: &[VenuePosition],
    ) -> VenueDataResult<NormalizedEvent> {
        let instrument_ids = positions
            .iter()
            .map(|position| position.instrument_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"AsterPrivateAccountAdapter\",\"endpoint\":{},\"freshness\":{},\"instrument_ids\":{},\"kind\":\"AsterPrivatePositionSnapshot\",\"market\":\"{}\",\"position_count\":{},\"raw_response_ref\":{},\"redaction\":\"private_position_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            json_string(self.market.position_endpoint()),
            freshness_payload_json(freshness),
            json_string_array(instrument_ids.iter().map(String::as_str)),
            self.market.as_str(),
            positions.len(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: aster_private_event_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::PositionSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:aster-private-account".to_owned(),
            source_sequence: Some(format!(
                "aster:private:{}:{}:{source_sequence}:position",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: aster_private_correlation_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn update_health(&mut self, freshness: DataFreshness, source_event_id: &str) {
        self.health.status = if freshness.is_stale() {
            VenueHealthStatus::Degraded
        } else {
            VenueHealthStatus::Healthy
        };
        self.health.connection = VenueConnectionStatus::Connected;
        self.health.reason_codes = if freshness.is_stale() {
            vec!["DATA_STALE".to_owned()]
        } else {
            Vec::new()
        };
        self.health.source_event_id = Some(source_event_id.to_owned());
        self.health.freshness = freshness;
    }
}

impl VenueReadAdapter for AsterPrivateAccountAdapter {
    fn venue_id(&self) -> &VenueId {
        &self.venue_id
    }
}

impl MarketDataReader for AsterPrivateAccountAdapter {
    fn latest_quote(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Aster private account adapter has no market data surface".to_owned(),
        })
    }

    fn order_book(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Aster private account adapter has no order book surface".to_owned(),
        })
    }
}

impl BalanceReader for AsterPrivateAccountAdapter {
    fn balances(&self, query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>> {
        Ok(self
            .balances
            .iter()
            .filter(|balance| balance.venue_id == query.venue_id)
            .filter(|balance| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &balance.account_id)
            })
            .filter(|balance| {
                query
                    .asset_id
                    .as_ref()
                    .is_none_or(|asset_id| asset_id == &balance.asset_id)
            })
            .cloned()
            .collect())
    }
}

impl PositionReader for AsterPrivateAccountAdapter {
    fn positions(&self, query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>> {
        Ok(self
            .positions
            .iter()
            .filter(|position| position.venue_id == query.venue_id)
            .filter(|position| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &position.account_id)
            })
            .filter(|position| {
                query
                    .instrument_id
                    .as_ref()
                    .is_none_or(|instrument_id| instrument_id == &position.instrument_id)
            })
            .cloned()
            .collect())
    }
}

impl InstrumentInfoReader for AsterPrivateAccountAdapter {
    fn instruments(&self, _query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::InstrumentInfo,
            reason: "Aster private account adapter does not define instrument metadata".to_owned(),
        })
    }
}

impl VenueHealthReader for AsterPrivateAccountAdapter {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
        if venue_id == &self.venue_id {
            Ok(self.health.clone())
        } else {
            Err(VenueDataError::DataUnavailable {
                venue_id: venue_id.clone(),
                surface: ReadOnlySurface::VenueHealth,
                reason: "adapter only tracks its configured venue".to_owned(),
            })
        }
    }
}

/// Hyperliquid 私有账户只读适配器。
///
/// 中文说明：该适配器只消费调用方已经获取到的 Hyperliquid `info`
/// `clearinghouseState` 和 `metaAndAssetCtxs` JSON 响应。`clearinghouseState`
/// 不直接提供 mark price，因此仓位映射必须同时给出公共资产上下文；缺失时按
/// 外部数据不完整失败关闭。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidPrivateAccountAdapter {
    venue_id: VenueId,
    account_id: AccountId,
    market: HyperliquidPrivateAccountMarket,
    max_age_ms: u64,
    balances: Vec<VenueBalance>,
    positions: Vec<VenuePosition>,
    health: VenueHealthSnapshot,
}

impl HyperliquidPrivateAccountAdapter {
    pub fn new(
        venue_id: VenueId,
        account_id: AccountId,
        market: HyperliquidPrivateAccountMarket,
        started_at: UtcTimestamp,
        max_age_ms: u64,
    ) -> VenueDataResult<Self> {
        let freshness = DataFreshness::new(started_at, started_at, max_age_ms)?;
        Ok(Self {
            venue_id: venue_id.clone(),
            account_id,
            market,
            max_age_ms,
            balances: Vec::new(),
            positions: Vec::new(),
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Unknown,
                connection: VenueConnectionStatus::Unknown,
                reason_codes: vec!["PRIVATE_ACCOUNT_NOT_INGESTED".to_owned()],
                rate_limit: None,
                source_event_id: None,
                freshness,
            },
        })
    }

    pub fn market(&self) -> HyperliquidPrivateAccountMarket {
        self.market
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// 解析 Hyperliquid `clearinghouseState` 响应为 USDC 保证金余额快照。
    pub fn ingest_clearinghouse_state_balance_json(
        &mut self,
        raw_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<HyperliquidPrivateBalanceBatch> {
        self.ensure_market(
            HyperliquidPrivateAccountMarket::PerpetualAccount,
            "hyperliquid.clearinghouse_state.balance",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let object =
            parse_hyperliquid_private_object(raw_json, &self.venue_id, ReadOnlySurface::Balance)?;
        let time_ms = required_u64(
            &object,
            "time",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        let observed_at = timestamp_from_unix_millis(time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = time_ms.to_string();
        let balance_event_id = hyperliquid_private_event_id(
            "balance",
            self.market,
            &self.account_id,
            &source_sequence,
        );
        let margin_summary = required_object_field(
            &object,
            "marginSummary",
            self.venue_id.clone(),
            ReadOnlySurface::Balance,
        )?;
        let account_value = parse_decimal_surface_field(
            margin_summary,
            "accountValue",
            &self.venue_id,
            ReadOnlySurface::Balance,
        )?;
        let withdrawable = parse_decimal_surface_field(
            &object,
            "withdrawable",
            &self.venue_id,
            ReadOnlySurface::Balance,
        )?;
        let locked = account_value.checked_sub(withdrawable).map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Balance,
                ExternalErrorClass::MalformedPayload,
                format!("Hyperliquid accountValue minus withdrawable is invalid: {error}"),
            ))
        })?;
        let balances = vec![VenueBalance {
            venue_id: self.venue_id.clone(),
            account_id: self.account_id.clone(),
            asset_id: AssetId::new("asset:USDC").map_err(VenueDataError::from)?,
            free: Amount::new(withdrawable).map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::Balance,
                    ExternalErrorClass::MalformedPayload,
                    format!("Hyperliquid withdrawable is not a valid amount: {error}"),
                ))
            })?,
            locked: Amount::new(locked).map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::Balance,
                    ExternalErrorClass::MalformedPayload,
                    format!(
                        "Hyperliquid accountValue must be greater than or equal to withdrawable: {error}"
                    ),
                ))
            })?,
            reserved: zero_amount(),
            pending: zero_amount(),
            borrowed: zero_amount(),
            lent: zero_amount(),
            unsettled: zero_amount(),
            source_event_id: Some(balance_event_id),
            freshness,
        }];

        let balance_event = self.balance_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &balances,
        )?;
        self.balances = balances.clone();
        self.positions.clear();
        self.update_health(freshness, balance_event.event_id.as_str());

        Ok(HyperliquidPrivateBalanceBatch {
            balance_event,
            balances,
        })
    }

    /// 解析 Hyperliquid `clearinghouseState` 和 `metaAndAssetCtxs` 为仓位快照。
    pub fn ingest_clearinghouse_state_positions_json(
        &mut self,
        raw_clearinghouse_state_json: &str,
        raw_asset_contexts_json: &str,
        raw_response_ref: impl Into<String>,
        ingested_at: UtcTimestamp,
    ) -> VenueDataResult<HyperliquidPrivatePositionBatch> {
        self.ensure_market(
            HyperliquidPrivateAccountMarket::PerpetualAccount,
            "hyperliquid.clearinghouse_state.positions",
        )?;
        let raw_response_ref = raw_response_ref.into();
        let object = parse_hyperliquid_private_object(
            raw_clearinghouse_state_json,
            &self.venue_id,
            ReadOnlySurface::Position,
        )?;
        let mark_prices = parse_hyperliquid_mark_prices(raw_asset_contexts_json, &self.venue_id)?;
        let time_ms = required_u64(
            &object,
            "time",
            self.venue_id.clone(),
            ReadOnlySurface::Position,
        )?;
        let observed_at = timestamp_from_unix_millis(time_ms).map_err(|detail| {
            VenueDataError::External(ClassifiedExternalError::new(
                self.venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                detail,
            ))
        })?;
        let freshness = DataFreshness::new(observed_at, ingested_at, self.max_age_ms)?;
        let source_sequence = time_ms.to_string();
        let position_event_id = hyperliquid_private_event_id(
            "position",
            self.market,
            &self.account_id,
            &source_sequence,
        );
        let position_values = required_array(
            &object,
            "assetPositions",
            self.venue_id.clone(),
            ReadOnlySurface::Position,
        )?;

        let mut positions = Vec::new();
        for (index, value) in position_values.iter().enumerate() {
            let wrapper = required_object_value(
                value,
                "assetPositions",
                index,
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            let position_object = required_object_field(
                wrapper,
                "position",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            let quantity = parse_decimal_surface_field(
                position_object,
                "szi",
                &self.venue_id,
                ReadOnlySurface::Position,
            )?;
            if quantity.is_zero() {
                continue;
            }
            let coin = required_string(
                position_object,
                "coin",
                self.venue_id.clone(),
                ReadOnlySurface::Position,
            )?;
            let coin = hyperliquid_coin(&coin)?;
            let Some(mark_price) = mark_prices.get(&coin).copied() else {
                return Err(VenueDataError::External(ClassifiedExternalError::new(
                    self.venue_id.clone(),
                    ReadOnlySurface::Position,
                    ExternalErrorClass::MissingField,
                    format!("Hyperliquid metaAndAssetCtxs response is missing markPx for `{coin}`"),
                )));
            };
            positions.push(VenuePosition {
                venue_id: self.venue_id.clone(),
                position_id: Some(hyperliquid_perp_position_id(
                    &self.account_id,
                    &coin,
                    if quantity.is_negative() {
                        "short"
                    } else {
                        "long"
                    },
                )?),
                account_id: self.account_id.clone(),
                instrument_id: hyperliquid_perp_instrument_id(&coin)?,
                quantity,
                entry_price: optional_nonzero_price_surface_field(
                    position_object,
                    "entryPx",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                mark_price,
                unrealized_pnl: parse_pnl_surface_field_any(
                    position_object,
                    &["unrealizedPnl"],
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                liquidation_price: optional_nonzero_price_surface_field(
                    position_object,
                    "liquidationPx",
                    &self.venue_id,
                    ReadOnlySurface::Position,
                )?,
                source_event_id: Some(position_event_id.clone()),
                freshness,
            });
        }

        let position_event = self.position_snapshot_event(
            &raw_response_ref,
            &source_sequence,
            observed_at,
            ingested_at,
            freshness,
            &positions,
        )?;
        self.positions = positions.clone();
        self.update_health(freshness, position_event.event_id.as_str());

        Ok(HyperliquidPrivatePositionBatch {
            position_event,
            positions,
        })
    }

    pub fn classify_http_status(
        &self,
        surface: ReadOnlySurface,
        status_code: u16,
        detail: impl Into<String>,
    ) -> ClassifiedExternalError {
        let class = match status_code {
            408 | 504 => ExternalErrorClass::Timeout,
            429 => ExternalErrorClass::RateLimited,
            500..=599 => ExternalErrorClass::Disconnected,
            _ => ExternalErrorClass::UnknownExternalState,
        };
        ClassifiedExternalError::new(self.venue_id.clone(), surface, class, detail)
    }

    fn ensure_market(
        &self,
        expected: HyperliquidPrivateAccountMarket,
        field: &'static str,
    ) -> VenueDataResult<()> {
        if self.market == expected {
            Ok(())
        } else {
            Err(VenueDataError::InvalidQuery {
                field,
                reason:
                    "Hyperliquid private account adapter market does not match this ingestion path",
            })
        }
    }

    fn balance_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        balances: &[VenueBalance],
    ) -> VenueDataResult<NormalizedEvent> {
        let asset_ids = balances
            .iter()
            .map(|balance| balance.asset_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"HyperliquidPrivateAccountAdapter\",\"asset_ids\":{},\"balance_count\":{},\"endpoint\":{},\"freshness\":{},\"kind\":\"HyperliquidPrivateBalanceSnapshot\",\"market\":\"{}\",\"raw_response_ref\":{},\"redaction\":\"private_account_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            json_string_array(asset_ids.iter().map(String::as_str)),
            balances.len(),
            json_string(self.market.account_endpoint()),
            freshness_payload_json(freshness),
            self.market.as_str(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: hyperliquid_private_event_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::BalanceSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:hyperliquid-private-account".to_owned(),
            source_sequence: Some(format!(
                "hyperliquid:private:{}:{}:{source_sequence}:balance",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: hyperliquid_private_correlation_id(
                "balance",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn position_snapshot_event(
        &self,
        raw_response_ref: &str,
        source_sequence: &str,
        observed_at: UtcTimestamp,
        ingested_at: UtcTimestamp,
        freshness: DataFreshness,
        positions: &[VenuePosition],
    ) -> VenueDataResult<NormalizedEvent> {
        let instrument_ids = positions
            .iter()
            .map(|position| position.instrument_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let payload = format!(
            "{{\"account_id\":{},\"adapter\":\"HyperliquidPrivateAccountAdapter\",\"endpoint\":{},\"freshness\":{},\"instrument_ids\":{},\"kind\":\"HyperliquidPrivatePositionSnapshot\",\"market\":\"{}\",\"market_context_endpoint\":{},\"position_count\":{},\"raw_response_ref\":{},\"redaction\":\"private_position_amounts_available_in_typed_snapshot_not_event_payload\",\"risk_reason_code\":{}}}",
            json_string(self.account_id.as_str()),
            json_string(self.market.account_endpoint()),
            freshness_payload_json(freshness),
            json_string_array(instrument_ids.iter().map(String::as_str)),
            self.market.as_str(),
            json_string(self.market.market_context_endpoint()),
            positions.len(),
            json_string(raw_response_ref),
            json_string(if freshness.is_stale() {
                "DATA_STALE"
            } else {
                "CHECK_PASSED"
            }),
        );
        build_normalized_event(EventEnvelope {
            event_id: hyperliquid_private_event_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            event_type: NormalizedEventType::PositionSnapshotEvent,
            timestamp_event: observed_at,
            timestamp_ingested: ingested_at,
            source: "adapter:hyperliquid-private-account".to_owned(),
            source_sequence: Some(format!(
                "hyperliquid:private:{}:{}:{source_sequence}:position",
                self.market.event_scope(),
                self.account_id
            )),
            correlation_id: hyperliquid_private_correlation_id(
                "position",
                self.market,
                &self.account_id,
                source_sequence,
            ),
            causation_id: None,
            venue_id: Some(self.venue_id.as_str().to_owned()),
            instrument_id: None,
            payload_json: payload,
        })
    }

    fn update_health(&mut self, freshness: DataFreshness, source_event_id: &str) {
        self.health.status = if freshness.is_stale() {
            VenueHealthStatus::Degraded
        } else {
            VenueHealthStatus::Healthy
        };
        self.health.connection = VenueConnectionStatus::Connected;
        self.health.reason_codes = if freshness.is_stale() {
            vec!["DATA_STALE".to_owned()]
        } else {
            Vec::new()
        };
        self.health.source_event_id = Some(source_event_id.to_owned());
        self.health.freshness = freshness;
    }
}

impl VenueReadAdapter for HyperliquidPrivateAccountAdapter {
    fn venue_id(&self) -> &VenueId {
        &self.venue_id
    }
}

impl MarketDataReader for HyperliquidPrivateAccountAdapter {
    fn latest_quote(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Hyperliquid private account adapter has no market data surface".to_owned(),
        })
    }

    fn order_book(&self, _query: &MarketDataQuery) -> VenueDataResult<Option<OrderBookSnapshot>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::MarketData,
            reason: "Hyperliquid private account adapter has no order book surface".to_owned(),
        })
    }
}

impl BalanceReader for HyperliquidPrivateAccountAdapter {
    fn balances(&self, query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>> {
        Ok(self
            .balances
            .iter()
            .filter(|balance| balance.venue_id == query.venue_id)
            .filter(|balance| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &balance.account_id)
            })
            .filter(|balance| {
                query
                    .asset_id
                    .as_ref()
                    .is_none_or(|asset_id| asset_id == &balance.asset_id)
            })
            .cloned()
            .collect())
    }
}

impl PositionReader for HyperliquidPrivateAccountAdapter {
    fn positions(&self, query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>> {
        Ok(self
            .positions
            .iter()
            .filter(|position| position.venue_id == query.venue_id)
            .filter(|position| {
                query
                    .account_id
                    .as_ref()
                    .is_none_or(|account_id| account_id == &position.account_id)
            })
            .filter(|position| {
                query
                    .instrument_id
                    .as_ref()
                    .is_none_or(|instrument_id| instrument_id == &position.instrument_id)
            })
            .cloned()
            .collect())
    }
}

impl InstrumentInfoReader for HyperliquidPrivateAccountAdapter {
    fn instruments(&self, _query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>> {
        Err(VenueDataError::DataUnavailable {
            venue_id: self.venue_id.clone(),
            surface: ReadOnlySurface::InstrumentInfo,
            reason: "Hyperliquid private account adapter does not define instrument metadata"
                .to_owned(),
        })
    }
}

impl VenueHealthReader for HyperliquidPrivateAccountAdapter {
    fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
        if venue_id == &self.venue_id {
            Ok(self.health.clone())
        } else {
            Err(VenueDataError::DataUnavailable {
                venue_id: venue_id.clone(),
                surface: ReadOnlySurface::VenueHealth,
                reason: "adapter only tracks its configured venue".to_owned(),
            })
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EventEnvelope {
    event_id: String,
    event_type: NormalizedEventType,
    timestamp_event: UtcTimestamp,
    timestamp_ingested: UtcTimestamp,
    source: String,
    source_sequence: Option<String>,
    correlation_id: String,
    causation_id: Option<String>,
    venue_id: Option<String>,
    instrument_id: Option<String>,
    payload_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BinanceTicker24hRaw {
    symbol: String,
    last_price: Price,
    best_bid: Price,
    best_ask: Price,
    bid_size: Quantity,
    ask_size: Quantity,
    close_time_ms: u64,
    last_id: u64,
    trade_count: Option<u64>,
    observed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BinanceBookTickerRaw {
    symbol: String,
    best_bid: Price,
    best_ask: Price,
    bid_size: Quantity,
    ask_size: Quantity,
    source_sequence: String,
    observed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BinanceWssBookTickerRaw {
    symbol: String,
    update_id: u64,
    best_bid: Price,
    best_ask: Price,
    bid_size: Quantity,
    ask_size: Quantity,
    observed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BinancePremiumIndexRaw {
    symbol: String,
    mark_price: Price,
    index_price: Price,
    last_funding_rate: String,
    interest_rate: String,
    next_funding_time_ms: u64,
    time_ms: u64,
    observed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BybitTickerRaw {
    symbol: String,
    best_bid: Price,
    best_ask: Price,
    bid_size: Quantity,
    ask_size: Quantity,
    source_sequence: String,
    observed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BybitPremiumIndexRaw {
    symbol: String,
    mark_price: Price,
    index_price: Price,
    last_funding_rate: String,
    next_funding_time_ms: u64,
    time_ms: u64,
    observed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BybitTickerRow {
    symbol: String,
    row: BTreeMap<String, FlatJsonValue>,
    time_ms: u64,
    observed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FlatJsonValue {
    String(String),
    Number(String),
    Bool,
    Null,
    Array(Vec<FlatJsonValue>),
    Object(BTreeMap<String, FlatJsonValue>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FlatJsonError {
    offset: usize,
    message: String,
}

impl fmt::Display for FlatJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "offset {}: {}", self.offset, self.message)
    }
}

struct FlatJsonParser<'a> {
    input: &'a str,
    chars: Vec<char>,
    pos: usize,
}

impl<'a> FlatJsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn parse(mut self) -> Result<BTreeMap<String, FlatJsonValue>, FlatJsonError> {
        self.skip_ws();
        let value = self.parse_value()?;
        self.finish()?;
        match value {
            FlatJsonValue::Object(object) => Ok(object),
            _ => Err(self.error("expected top-level JSON object")),
        }
    }

    fn parse_value_root(mut self) -> Result<FlatJsonValue, FlatJsonError> {
        self.skip_ws();
        let value = self.parse_value()?;
        self.finish()?;
        Ok(value)
    }

    fn parse_object(&mut self) -> Result<BTreeMap<String, FlatJsonValue>, FlatJsonError> {
        self.expect('{')?;
        self.skip_ws();
        let mut object = BTreeMap::new();
        if self.peek() == Some('}') {
            self.pos += 1;
            return Ok(object);
        }

        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(':')?;
            self.skip_ws();
            let value = self.parse_value()?;
            if object.insert(key.clone(), value).is_some() {
                return Err(self.error(format!("duplicate key `{key}`")));
            }
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some('}') => {
                    self.pos += 1;
                    return Ok(object);
                }
                _ => return Err(self.error("expected comma or closing brace")),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Vec<FlatJsonValue>, FlatJsonError> {
        self.expect('[')?;
        self.skip_ws();
        let mut values = Vec::new();
        if self.peek() == Some(']') {
            self.pos += 1;
            return Ok(values);
        }

        loop {
            self.skip_ws();
            values.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some(']') => {
                    self.pos += 1;
                    return Ok(values);
                }
                _ => return Err(self.error("expected comma or closing bracket")),
            }
        }
    }

    fn parse_value(&mut self) -> Result<FlatJsonValue, FlatJsonError> {
        match self.peek() {
            Some('"') => self.parse_string().map(FlatJsonValue::String),
            Some('-' | '0'..='9') => self.parse_number().map(FlatJsonValue::Number),
            Some('t') => {
                self.expect_literal("true")?;
                Ok(FlatJsonValue::Bool)
            }
            Some('f') => {
                self.expect_literal("false")?;
                Ok(FlatJsonValue::Bool)
            }
            Some('n') => {
                self.expect_literal("null")?;
                Ok(FlatJsonValue::Null)
            }
            Some('{') => self.parse_object().map(FlatJsonValue::Object),
            Some('[') => self.parse_array().map(FlatJsonValue::Array),
            Some(_) => Err(self.error("unexpected JSON value")),
            None => Err(self.error("unexpected end of JSON")),
        }
    }

    fn parse_string(&mut self) -> Result<String, FlatJsonError> {
        self.expect('"')?;
        let mut value = String::new();
        loop {
            let Some(ch) = self.peek() else {
                return Err(self.error("unterminated string"));
            };
            self.pos += 1;
            match ch {
                '"' => return Ok(value),
                '\\' => {
                    let Some(escaped) = self.peek() else {
                        return Err(self.error("unterminated escape"));
                    };
                    self.pos += 1;
                    match escaped {
                        '"' => value.push('"'),
                        '\\' => value.push('\\'),
                        '/' => value.push('/'),
                        'b' => value.push('\u{0008}'),
                        'f' => value.push('\u{000c}'),
                        'n' => value.push('\n'),
                        'r' => value.push('\r'),
                        't' => value.push('\t'),
                        'u' => {
                            return Err(self
                                .error("unicode escapes are not supported in this fixture parser"))
                        }
                        _ => return Err(self.error("unsupported escape sequence")),
                    }
                }
                other => value.push(other),
            }
        }
    }

    fn parse_number(&mut self) -> Result<String, FlatJsonError> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        self.consume_digits();
        if self.peek() == Some('.') {
            self.pos += 1;
            let before_fraction = self.pos;
            self.consume_digits();
            if self.pos == before_fraction {
                return Err(self.error("number fraction must contain digits"));
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            return Err(self.error("exponent notation is not accepted in ticker fixtures"));
        }
        if self.pos == start || (self.pos == start + 1 && self.chars[start] == '-') {
            return Err(self.error("number must contain digits"));
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }

    fn consume_digits(&mut self) {
        while matches!(self.peek(), Some('0'..='9')) {
            self.pos += 1;
        }
    }

    fn expect_literal(&mut self, literal: &str) -> Result<(), FlatJsonError> {
        for expected in literal.chars() {
            if self.peek() != Some(expected) {
                return Err(self.error(format!("expected literal `{literal}`")));
            }
            self.pos += 1;
        }
        Ok(())
    }

    fn expect(&mut self, expected: char) -> Result<(), FlatJsonError> {
        if self.peek() == Some(expected) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected `{expected}`")))
        }
    }

    fn finish(&mut self) -> Result<(), FlatJsonError> {
        self.skip_ws();
        if self.pos == self.chars.len() {
            Ok(())
        } else {
            Err(self.error("trailing characters"))
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

    fn error(&self, message: impl Into<String>) -> FlatJsonError {
        let byte_offset = self
            .chars
            .iter()
            .take(self.pos)
            .map(|ch| ch.len_utf8())
            .sum::<usize>()
            .min(self.input.len());
        FlatJsonError {
            offset: byte_offset,
            message: message.into(),
        }
    }
}

fn validate_binance_symbol(symbol: &str) -> VenueDataResult<()> {
    if symbol.len() < 3 || symbol.len() > 64 {
        return Err(VenueDataError::InvalidQuery {
            field: "binance.symbol",
            reason: "symbol length must be 3..=64",
        });
    }
    if !symbol.chars().all(|ch| {
        ch.is_ascii_uppercase()
            || ch.is_ascii_digit()
            || (!ch.is_ascii() && !ch.is_control() && !ch.is_whitespace())
    }) {
        return Err(VenueDataError::InvalidQuery {
            field: "binance.symbol",
            reason:
                "symbol must contain uppercase ASCII letters/digits or non-ASCII symbol characters without whitespace",
        });
    }
    Ok(())
}

fn validate_bybit_symbol(symbol: &str) -> VenueDataResult<()> {
    if symbol.len() < 3 || symbol.len() > 32 {
        return Err(VenueDataError::InvalidQuery {
            field: "bybit.symbol",
            reason: "symbol length must be 3..=32",
        });
    }
    if !symbol
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(VenueDataError::InvalidQuery {
            field: "bybit.symbol",
            reason: "symbol must contain only uppercase ASCII letters and digits",
        });
    }
    Ok(())
}

fn validate_okx_inst_id(inst_id: &str) -> VenueDataResult<()> {
    if inst_id.len() < 3 || inst_id.len() > 64 {
        return Err(VenueDataError::InvalidQuery {
            field: "okx.instId",
            reason: "instrument ID length must be 3..=64",
        });
    }
    if !inst_id
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(VenueDataError::InvalidQuery {
            field: "okx.instId",
            reason: "instrument ID must contain only uppercase ASCII letters, digits and hyphens",
        });
    }
    Ok(())
}

fn validate_bitget_symbol(symbol: &str) -> VenueDataResult<()> {
    if symbol.len() < 3 || symbol.len() > 32 {
        return Err(VenueDataError::InvalidQuery {
            field: "bitget.symbol",
            reason: "symbol length must be 3..=32",
        });
    }
    if !symbol
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(VenueDataError::InvalidQuery {
            field: "bitget.symbol",
            reason: "symbol must contain only uppercase ASCII letters and digits",
        });
    }
    Ok(())
}

fn validate_aster_symbol(symbol: &str) -> VenueDataResult<()> {
    if symbol.len() < 3 || symbol.len() > 32 {
        return Err(VenueDataError::InvalidQuery {
            field: "aster.symbol",
            reason: "symbol length must be 3..=32",
        });
    }
    if !symbol
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(VenueDataError::InvalidQuery {
            field: "aster.symbol",
            reason: "symbol must contain only uppercase ASCII letters and digits",
        });
    }
    Ok(())
}

fn validate_hyperliquid_coin(coin: &str) -> VenueDataResult<()> {
    if coin.is_empty() || coin.len() > 64 {
        return Err(VenueDataError::InvalidQuery {
            field: "hyperliquid.coin",
            reason: "coin length must be 1..=64",
        });
    }
    if !coin
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.'))
    {
        return Err(VenueDataError::InvalidQuery {
            field: "hyperliquid.coin",
            reason:
                "coin must contain only ASCII letters, digits, colon, underscore, hyphen or dot",
        });
    }
    Ok(())
}

fn validate_bybit_asset_symbol(asset: &str) -> VenueDataResult<()> {
    if asset.is_empty() || asset.len() > 32 {
        return Err(VenueDataError::InvalidQuery {
            field: "bybit.asset",
            reason: "asset symbol length must be 1..=32",
        });
    }
    if !asset
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(VenueDataError::InvalidQuery {
            field: "bybit.asset",
            reason: "asset symbol must contain only uppercase ASCII letters and digits",
        });
    }
    Ok(())
}

fn validate_okx_asset_symbol(asset: &str) -> VenueDataResult<()> {
    if asset.is_empty() || asset.len() > 32 {
        return Err(VenueDataError::InvalidQuery {
            field: "okx.asset",
            reason: "asset symbol length must be 1..=32",
        });
    }
    if !asset
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(VenueDataError::InvalidQuery {
            field: "okx.asset",
            reason: "asset symbol must contain only uppercase ASCII letters and digits",
        });
    }
    Ok(())
}

fn validate_bitget_asset_symbol(asset: &str) -> VenueDataResult<()> {
    if asset.is_empty() || asset.len() > 32 {
        return Err(VenueDataError::InvalidQuery {
            field: "bitget.asset",
            reason: "asset symbol length must be 1..=32",
        });
    }
    if !asset
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(VenueDataError::InvalidQuery {
            field: "bitget.asset",
            reason: "asset symbol must contain only uppercase ASCII letters and digits",
        });
    }
    Ok(())
}

fn validate_aster_asset_symbol(asset: &str) -> VenueDataResult<()> {
    if asset.is_empty() || asset.len() > 32 {
        return Err(VenueDataError::InvalidQuery {
            field: "aster.asset",
            reason: "asset symbol length must be 1..=32",
        });
    }
    if !asset
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(VenueDataError::InvalidQuery {
            field: "aster.asset",
            reason: "asset symbol must contain only uppercase ASCII letters and digits",
        });
    }
    Ok(())
}

fn validate_binance_asset_symbol(asset: &str) -> VenueDataResult<()> {
    if asset.is_empty() || asset.len() > 32 {
        return Err(VenueDataError::InvalidQuery {
            field: "binance.asset",
            reason: "asset symbol length must be 1..=32",
        });
    }
    if !asset
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(VenueDataError::InvalidQuery {
            field: "binance.asset",
            reason: "asset symbol must contain only uppercase ASCII letters and digits",
        });
    }
    Ok(())
}

fn validate_binance_position_side(position_side: &str) -> VenueDataResult<()> {
    if matches!(position_side, "BOTH" | "LONG" | "SHORT") {
        Ok(())
    } else {
        Err(VenueDataError::InvalidQuery {
            field: "binance.positionSide",
            reason: "position side must be BOTH, LONG or SHORT",
        })
    }
}

fn validate_aster_position_side(position_side: &str) -> VenueDataResult<()> {
    if matches!(position_side, "BOTH" | "LONG" | "SHORT") {
        Ok(())
    } else {
        Err(VenueDataError::InvalidQuery {
            field: "aster.positionSide",
            reason: "position side must be BOTH, LONG or SHORT",
        })
    }
}

fn parse_binance_private_object(
    raw_json: &str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<BTreeMap<String, FlatJsonValue>> {
    FlatJsonParser::new(raw_json).parse().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            error.to_string(),
        ))
    })
}

fn parse_bybit_private_object(
    raw_json: &str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<BTreeMap<String, FlatJsonValue>> {
    FlatJsonParser::new(raw_json).parse().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            error.to_string(),
        ))
    })
}

fn parse_okx_private_object(
    raw_json: &str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<BTreeMap<String, FlatJsonValue>> {
    FlatJsonParser::new(raw_json).parse().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            error.to_string(),
        ))
    })
}

fn parse_bitget_private_object(
    raw_json: &str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<BTreeMap<String, FlatJsonValue>> {
    FlatJsonParser::new(raw_json).parse().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            error.to_string(),
        ))
    })
}

fn parse_aster_private_value(
    raw_json: &str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<FlatJsonValue> {
    FlatJsonParser::new(raw_json)
        .parse_value_root()
        .map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                surface,
                ExternalErrorClass::MalformedPayload,
                error.to_string(),
            ))
        })
}

fn parse_hyperliquid_private_object(
    raw_json: &str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<BTreeMap<String, FlatJsonValue>> {
    FlatJsonParser::new(raw_json).parse().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            error.to_string(),
        ))
    })
}

fn parse_hyperliquid_mark_prices(
    raw_json: &str,
    venue_id: &VenueId,
) -> VenueDataResult<BTreeMap<String, Price>> {
    let value = FlatJsonParser::new(raw_json)
        .parse_value_root()
        .map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                error.to_string(),
            ))
        })?;
    let FlatJsonValue::Array(values) = value else {
        return Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            ReadOnlySurface::Position,
            ExternalErrorClass::MalformedPayload,
            "Hyperliquid metaAndAssetCtxs response must be a JSON array",
        )));
    };
    if values.len() != 2 {
        return Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            ReadOnlySurface::Position,
            ExternalErrorClass::MalformedPayload,
            "Hyperliquid metaAndAssetCtxs response must contain metadata and asset contexts",
        )));
    }
    let meta = match &values[0] {
        FlatJsonValue::Object(object) => object,
        _ => {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                "Hyperliquid metaAndAssetCtxs metadata item must be an object",
            )))
        }
    };
    let universe = required_array(
        meta,
        "universe",
        venue_id.clone(),
        ReadOnlySurface::Position,
    )?;
    let contexts = match &values[1] {
        FlatJsonValue::Array(values) => values,
        _ => {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MalformedPayload,
                "Hyperliquid metaAndAssetCtxs context item must be an array",
            )))
        }
    };
    let mut prices = BTreeMap::new();
    for (index, item) in universe.iter().enumerate() {
        let meta_row = required_object_value(
            item,
            "universe",
            index,
            venue_id.clone(),
            ReadOnlySurface::Position,
        )?;
        let coin = required_string(
            meta_row,
            "name",
            venue_id.clone(),
            ReadOnlySurface::Position,
        )?;
        validate_hyperliquid_coin(&coin)?;
        let Some(context) = contexts.get(index) else {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                ReadOnlySurface::Position,
                ExternalErrorClass::MissingField,
                format!("Hyperliquid metaAndAssetCtxs missing context row for `{coin}`"),
            )));
        };
        let context = required_object_value(
            context,
            "asset_contexts",
            index,
            venue_id.clone(),
            ReadOnlySurface::Position,
        )?;
        let mark_price =
            parse_price_surface_field(context, "markPx", venue_id, ReadOnlySurface::Position)?;
        prices.insert(coin, mark_price);
    }
    Ok(prices)
}

fn validate_bybit_v5_ret_code(
    object: &BTreeMap<String, FlatJsonValue>,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<()> {
    let ret_code = required_u64(object, "retCode", venue_id.clone(), surface)?;
    if ret_code == 0 {
        return Ok(());
    }
    let ret_msg = optional_string(object, "retMsg", venue_id.clone(), surface)?
        .unwrap_or_else(|| "missing retMsg".to_owned());
    Err(VenueDataError::External(ClassifiedExternalError::new(
        venue_id.clone(),
        surface,
        ExternalErrorClass::UnknownExternalState,
        format!("Bybit V5 private response returned retCode={ret_code}: {ret_msg}"),
    )))
}

fn validate_okx_v5_code(
    object: &BTreeMap<String, FlatJsonValue>,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<()> {
    let code = required_string(object, "code", venue_id.clone(), surface)?;
    if code == "0" {
        return Ok(());
    }
    let msg = optional_string(object, "msg", venue_id.clone(), surface)?
        .unwrap_or_else(|| "missing msg".to_owned());
    Err(VenueDataError::External(ClassifiedExternalError::new(
        venue_id.clone(),
        surface,
        ExternalErrorClass::UnknownExternalState,
        format!("OKX V5 private response returned code={code}: {msg}"),
    )))
}

fn validate_bitget_code(
    object: &BTreeMap<String, FlatJsonValue>,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<()> {
    let code = required_string(object, "code", venue_id.clone(), surface)?;
    if code == "00000" {
        return Ok(());
    }
    let msg = optional_string(object, "msg", venue_id.clone(), surface)?
        .unwrap_or_else(|| "missing msg".to_owned());
    Err(VenueDataError::External(ClassifiedExternalError::new(
        venue_id.clone(),
        surface,
        ExternalErrorClass::UnknownExternalState,
        format!("Bitget private response returned code={code}: {msg}"),
    )))
}

fn parse_bybit_v5_ticker_row(
    raw_json: &str,
    venue_id: &VenueId,
    market: BybitPublicMarket,
    expected_symbol: &str,
) -> VenueDataResult<BybitTickerRow> {
    let object = FlatJsonParser::new(raw_json).parse().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            ReadOnlySurface::MarketData,
            ExternalErrorClass::MalformedPayload,
            error.to_string(),
        ))
    })?;
    let ret_code = required_u64(
        &object,
        "retCode",
        venue_id.clone(),
        ReadOnlySurface::MarketData,
    )?;
    if ret_code != 0 {
        let ret_msg = optional_string(
            &object,
            "retMsg",
            venue_id.clone(),
            ReadOnlySurface::MarketData,
        )?
        .unwrap_or_else(|| "missing retMsg".to_owned());
        return Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            ReadOnlySurface::MarketData,
            ExternalErrorClass::UnknownExternalState,
            format!("Bybit V5 ticker returned retCode={ret_code}: {ret_msg}"),
        )));
    }

    let time_ms = required_u64(
        &object,
        "time",
        venue_id.clone(),
        ReadOnlySurface::MarketData,
    )?;
    let observed_at = timestamp_from_unix_millis(time_ms).map_err(|detail| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            ReadOnlySurface::MarketData,
            ExternalErrorClass::MalformedPayload,
            detail,
        ))
    })?;
    let result = required_object_field(
        &object,
        "result",
        venue_id.clone(),
        ReadOnlySurface::MarketData,
    )?;
    let category = required_string(
        result,
        "category",
        venue_id.clone(),
        ReadOnlySurface::MarketData,
    )?;
    if category != market.category() {
        return Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            ReadOnlySurface::MarketData,
            ExternalErrorClass::UnknownExternalState,
            format!(
                "Bybit V5 ticker category `{category}` does not match configured category `{}`",
                market.category()
            ),
        )));
    }

    let list = required_array(
        result,
        "list",
        venue_id.clone(),
        ReadOnlySurface::MarketData,
    )?;
    for (index, value) in list.iter().enumerate() {
        let row = required_object_value(
            value,
            "list",
            index,
            venue_id.clone(),
            ReadOnlySurface::MarketData,
        )?;
        let symbol = required_string(row, "symbol", venue_id.clone(), ReadOnlySurface::MarketData)?;
        if symbol == expected_symbol {
            return Ok(BybitTickerRow {
                symbol,
                row: row.clone(),
                time_ms,
                observed_at,
            });
        }
    }

    Err(VenueDataError::External(ClassifiedExternalError::new(
        venue_id.clone(),
        ReadOnlySurface::MarketData,
        ExternalErrorClass::UnknownExternalState,
        format!("Bybit V5 ticker list does not contain configured symbol `{expected_symbol}`"),
    )))
}

fn required_string(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<String> {
    match object.get(field) {
        Some(FlatJsonValue::String(value)) => Ok(value.clone()),
        Some(_) => Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id,
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` must be a string"),
        ))),
        None => Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id,
            surface,
            ExternalErrorClass::MissingField,
            format!("required field `{field}` is missing"),
        ))),
    }
}

fn optional_string(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Option<String>> {
    match object.get(field) {
        Some(FlatJsonValue::String(value)) => Ok(Some(value.clone())),
        Some(FlatJsonValue::Null) | None => Ok(None),
        Some(_) => Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id,
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` must be a string when present"),
        ))),
    }
}

fn required_array<'a>(
    object: &'a BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<&'a [FlatJsonValue]> {
    match object.get(field) {
        Some(FlatJsonValue::Array(values)) => Ok(values),
        Some(_) => Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id,
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` must be an array"),
        ))),
        None => Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id,
            surface,
            ExternalErrorClass::MissingField,
            format!("required field `{field}` is missing"),
        ))),
    }
}

fn required_object_field<'a>(
    object: &'a BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<&'a BTreeMap<String, FlatJsonValue>> {
    match object.get(field) {
        Some(FlatJsonValue::Object(value)) => Ok(value),
        Some(_) => Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id,
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` must be an object"),
        ))),
        None => Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id,
            surface,
            ExternalErrorClass::MissingField,
            format!("required field `{field}` is missing"),
        ))),
    }
}

fn required_object_value<'a>(
    value: &'a FlatJsonValue,
    field: &'static str,
    index: usize,
    venue_id: VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<&'a BTreeMap<String, FlatJsonValue>> {
    match value {
        FlatJsonValue::Object(object) => Ok(object),
        _ => Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id,
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` item {index} must be an object"),
        ))),
    }
}

fn required_u64(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<u64> {
    let raw = match object.get(field) {
        Some(FlatJsonValue::Number(value) | FlatJsonValue::String(value)) => value,
        Some(_) => {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                venue_id,
                surface,
                ExternalErrorClass::MalformedPayload,
                format!("field `{field}` must be an integer"),
            )))
        }
        None => {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                venue_id,
                surface,
                ExternalErrorClass::MissingField,
                format!("required field `{field}` is missing"),
            )))
        }
    };
    raw.parse::<u64>().map_err(|_| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id,
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` must be a non-negative integer"),
        ))
    })
}

fn optional_u64(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
) -> VenueDataResult<Option<u64>> {
    match object.get(field) {
        Some(FlatJsonValue::Number(value) | FlatJsonValue::String(value)) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| VenueDataError::InvalidQuery {
                field,
                reason: "optional integer field is malformed",
            }),
        Some(FlatJsonValue::Null) | None => Ok(None),
        Some(_) => Err(VenueDataError::InvalidQuery {
            field,
            reason: "optional integer field has the wrong type",
        }),
    }
}

fn required_decimal_text(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<String> {
    let value = match object.get(field) {
        Some(FlatJsonValue::String(value) | FlatJsonValue::Number(value)) => value.clone(),
        Some(_) => {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                surface,
                ExternalErrorClass::MalformedPayload,
                format!("field `{field}` must be a decimal string or number"),
            )))
        }
        None => {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                surface,
                ExternalErrorClass::MissingField,
                format!("required field `{field}` is missing"),
            )))
        }
    };
    value.parse::<Decimal>().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` is not a valid decimal: {error}"),
        ))
    })?;
    Ok(value)
}

fn optional_decimal_text(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Option<String>> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    match value {
        FlatJsonValue::String(value) | FlatJsonValue::Number(value) => {
            value.parse::<Decimal>().map_err(|error| {
                VenueDataError::External(ClassifiedExternalError::new(
                    venue_id.clone(),
                    surface,
                    ExternalErrorClass::MalformedPayload,
                    format!("field `{field}` is not a valid decimal: {error}"),
                ))
            })?;
            Ok(Some(value.clone()))
        }
        FlatJsonValue::Null => Ok(None),
        _ => Err(VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` must be a decimal string or number when present"),
        ))),
    }
}

fn parse_amount_surface_field(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Amount> {
    required_decimal_text(object, field, venue_id, surface)?
        .parse::<Amount>()
        .map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                surface,
                ExternalErrorClass::MalformedPayload,
                format!("field `{field}` is not a valid non-negative amount: {error}"),
            ))
        })
}

fn parse_amount_surface_field_any_non_empty(
    object: &BTreeMap<String, FlatJsonValue>,
    fields: &[&'static str],
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Amount> {
    for field in fields {
        let Some(value) = object.get(*field) else {
            continue;
        };
        let value = match value {
            FlatJsonValue::String(value) | FlatJsonValue::Number(value) => value.trim(),
            FlatJsonValue::Null => continue,
            _ => {
                return Err(VenueDataError::External(ClassifiedExternalError::new(
                    venue_id.clone(),
                    surface,
                    ExternalErrorClass::MalformedPayload,
                    format!("field `{field}` must be a decimal string or number when present"),
                )))
            }
        };
        if value.is_empty() {
            continue;
        }
        return value.parse::<Amount>().map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                surface,
                ExternalErrorClass::MalformedPayload,
                format!("field `{field}` is not a valid non-negative amount: {error}"),
            ))
        });
    }
    Err(VenueDataError::External(ClassifiedExternalError::new(
        venue_id.clone(),
        surface,
        ExternalErrorClass::MissingField,
        format!("required field `{}` is missing", fields.join("` or `")),
    )))
}

fn parse_optional_amount_surface_field(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Amount> {
    let Some(value) = optional_decimal_text(object, field, venue_id, surface)? else {
        return Ok(zero_amount());
    };
    value.parse::<Amount>().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` is not a valid non-negative amount: {error}"),
        ))
    })
}

fn parse_optional_amount_surface_field_non_empty(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Amount> {
    let Some(value) = object.get(field) else {
        return Ok(zero_amount());
    };
    let value = match value {
        FlatJsonValue::String(value) | FlatJsonValue::Number(value) => value.trim(),
        FlatJsonValue::Null => return Ok(zero_amount()),
        _ => {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                surface,
                ExternalErrorClass::MalformedPayload,
                format!("field `{field}` must be a decimal string or number when present"),
            )))
        }
    };
    if value.is_empty() {
        return Ok(zero_amount());
    }
    value.parse::<Amount>().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` is not a valid non-negative amount: {error}"),
        ))
    })
}

fn parse_non_negative_amount_difference_surface_fields(
    object: &BTreeMap<String, FlatJsonValue>,
    minuend_field: &'static str,
    subtrahend_field: &'static str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Amount> {
    let minuend = parse_decimal_surface_field(object, minuend_field, venue_id, surface)?;
    let subtrahend = parse_decimal_surface_field(object, subtrahend_field, venue_id, surface)?;
    let difference = minuend.checked_sub(subtrahend).map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{minuend_field}` minus `{subtrahend_field}` is invalid: {error}"),
        ))
    })?;
    Amount::new(difference).map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{minuend_field}` must be greater than or equal to `{subtrahend_field}`: {error}"),
        ))
    })
}

fn parse_decimal_surface_field(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Decimal> {
    required_decimal_text(object, field, venue_id, surface)?
        .parse::<Decimal>()
        .map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                surface,
                ExternalErrorClass::MalformedPayload,
                format!("field `{field}` is not a valid decimal: {error}"),
            ))
        })
}

fn parse_price_surface_field(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Price> {
    required_decimal_text(object, field, venue_id, surface)?
        .parse::<Price>()
        .map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                surface,
                ExternalErrorClass::MalformedPayload,
                format!("field `{field}` is not a valid non-negative price: {error}"),
            ))
        })
}

fn optional_nonzero_price_surface_field(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Option<Price>> {
    let Some(raw) = object.get(field) else {
        return Ok(None);
    };
    let value = match raw {
        FlatJsonValue::String(value) | FlatJsonValue::Number(value) => value,
        FlatJsonValue::Null => return Ok(None),
        _ => {
            return Err(VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                surface,
                ExternalErrorClass::MalformedPayload,
                format!("field `{field}` must be a decimal string or number when present"),
            )))
        }
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    let price = value.parse::<Price>().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            surface,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` is not a valid non-negative price: {error}"),
        ))
    })?;
    Ok((!price.as_decimal().is_zero()).then_some(price))
}

fn optional_nonzero_price_surface_field_any(
    object: &BTreeMap<String, FlatJsonValue>,
    fields: &[&'static str],
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Option<Price>> {
    for field in fields {
        if object.contains_key(*field) {
            return optional_nonzero_price_surface_field(object, field, venue_id, surface);
        }
    }
    Ok(None)
}

fn parse_pnl_surface_field_any(
    object: &BTreeMap<String, FlatJsonValue>,
    fields: &[&'static str],
    venue_id: &VenueId,
    surface: ReadOnlySurface,
) -> VenueDataResult<Pnl> {
    for field in fields {
        if object.contains_key(*field) {
            return required_decimal_text(object, field, venue_id, surface)?
                .parse::<Pnl>()
                .map_err(|error| {
                    VenueDataError::External(ClassifiedExternalError::new(
                        venue_id.clone(),
                        surface,
                        ExternalErrorClass::MalformedPayload,
                        format!("field `{field}` is not a valid PnL decimal: {error}"),
                    ))
                });
        }
    }
    Err(VenueDataError::External(ClassifiedExternalError::new(
        venue_id.clone(),
        surface,
        ExternalErrorClass::MissingField,
        format!("required field `{}` is missing", fields.join("` or `")),
    )))
}

fn parse_price_field(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
) -> VenueDataResult<Price> {
    let value = required_string(object, field, venue_id.clone(), ReadOnlySurface::MarketData)?;
    value.parse::<Price>().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            ReadOnlySurface::MarketData,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` is not a valid price: {error}"),
        ))
    })
}

fn parse_quantity_field(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
) -> VenueDataResult<Quantity> {
    let value = required_string(object, field, venue_id.clone(), ReadOnlySurface::MarketData)?;
    value.parse::<Quantity>().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            ReadOnlySurface::MarketData,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` is not a valid quantity: {error}"),
        ))
    })
}

fn parse_decimal_string_field(
    object: &BTreeMap<String, FlatJsonValue>,
    field: &'static str,
    venue_id: &VenueId,
) -> VenueDataResult<String> {
    let value = required_string(object, field, venue_id.clone(), ReadOnlySurface::MarketData)?;
    value.parse::<Decimal>().map_err(|error| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            ReadOnlySurface::MarketData,
            ExternalErrorClass::MalformedPayload,
            format!("field `{field}` is not a valid decimal: {error}"),
        ))
    })?;
    Ok(value)
}

fn zero_amount() -> Amount {
    Amount::new(Decimal::from_scaled_atoms(0, 0)).expect("zero is a valid amount")
}

fn parse_bybit_wallet_free(
    object: &BTreeMap<String, FlatJsonValue>,
    venue_id: &VenueId,
) -> VenueDataResult<Amount> {
    if object.contains_key("availableToWithdraw") {
        return parse_amount_surface_field(
            object,
            "availableToWithdraw",
            venue_id,
            ReadOnlySurface::Balance,
        );
    }
    parse_amount_surface_field(object, "walletBalance", venue_id, ReadOnlySurface::Balance)
}

fn binance_asset_id(asset: &str) -> VenueDataResult<AssetId> {
    validate_binance_asset_symbol(asset)?;
    AssetId::new(format!("asset:{asset}")).map_err(VenueDataError::from)
}

fn bybit_asset_id(asset: &str) -> VenueDataResult<AssetId> {
    validate_bybit_asset_symbol(asset)?;
    AssetId::new(format!("asset:{asset}")).map_err(VenueDataError::from)
}

fn okx_asset_id(asset: &str) -> VenueDataResult<AssetId> {
    validate_okx_asset_symbol(asset)?;
    AssetId::new(format!("asset:{asset}")).map_err(VenueDataError::from)
}

fn bitget_asset_symbol(asset: &str) -> VenueDataResult<String> {
    let asset = asset.trim().to_ascii_uppercase();
    validate_bitget_asset_symbol(&asset)?;
    Ok(asset)
}

fn bitget_symbol(symbol: &str) -> VenueDataResult<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    validate_bitget_symbol(&symbol)?;
    Ok(symbol)
}

fn aster_asset_symbol(asset: &str) -> VenueDataResult<String> {
    let asset = asset.trim().to_ascii_uppercase();
    validate_aster_asset_symbol(&asset)?;
    Ok(asset)
}

fn aster_symbol(symbol: &str) -> VenueDataResult<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    validate_aster_symbol(&symbol)?;
    Ok(symbol)
}

fn hyperliquid_coin(coin: &str) -> VenueDataResult<String> {
    let coin = coin.trim().to_owned();
    validate_hyperliquid_coin(&coin)?;
    Ok(coin)
}

fn bitget_asset_id(asset: &str) -> VenueDataResult<AssetId> {
    validate_bitget_asset_symbol(asset)?;
    AssetId::new(format!("asset:{asset}")).map_err(VenueDataError::from)
}

fn aster_asset_id(asset: &str) -> VenueDataResult<AssetId> {
    validate_aster_asset_symbol(asset)?;
    AssetId::new(format!("asset:{asset}")).map_err(VenueDataError::from)
}

fn binance_usdm_instrument_id(symbol: &str) -> VenueDataResult<InstrumentId> {
    validate_binance_symbol(symbol)?;
    InstrumentId::new(format!("inst:BINANCE:{symbol}:USDM-PERP")).map_err(VenueDataError::from)
}

fn bybit_linear_instrument_id(symbol: &str) -> VenueDataResult<InstrumentId> {
    validate_bybit_symbol(symbol)?;
    InstrumentId::new(format!("inst:BYBIT:{symbol}:LINEAR-PERP")).map_err(VenueDataError::from)
}

fn okx_swap_instrument_id(inst_id: &str) -> VenueDataResult<InstrumentId> {
    validate_okx_inst_id(inst_id)?;
    if !inst_id.ends_with("-SWAP") {
        return Err(VenueDataError::InvalidQuery {
            field: "okx.instId",
            reason: "only OKX SWAP positions are currently mapped by this adapter",
        });
    }
    InstrumentId::new(format!("inst:OKX:{inst_id}:SWAP")).map_err(VenueDataError::from)
}

fn bitget_usdt_futures_instrument_id(symbol: &str) -> VenueDataResult<InstrumentId> {
    validate_bitget_symbol(symbol)?;
    InstrumentId::new(format!("inst:BITGET:{symbol}:USDT-FUTURES")).map_err(VenueDataError::from)
}

fn aster_usdt_futures_instrument_id(symbol: &str) -> VenueDataResult<InstrumentId> {
    validate_aster_symbol(symbol)?;
    InstrumentId::new(format!("inst:ASTER:{symbol}:USDT-FUTURES")).map_err(VenueDataError::from)
}

fn hyperliquid_perp_instrument_id(coin: &str) -> VenueDataResult<InstrumentId> {
    validate_hyperliquid_coin(coin)?;
    InstrumentId::new(format!("inst:HYPERLIQUID:{coin}:PERP")).map_err(VenueDataError::from)
}

fn binance_usdm_position_id(
    account_id: &AccountId,
    symbol: &str,
    position_side: &str,
) -> VenueDataResult<PositionId> {
    validate_binance_symbol(symbol)?;
    validate_binance_position_side(position_side)?;
    PositionId::new(format!(
        "pos:{}:binance-usdm:{symbol}:{}",
        account_id,
        position_side.to_ascii_lowercase()
    ))
    .map_err(VenueDataError::from)
}

fn okx_swap_position_id(
    account_id: &AccountId,
    inst_id: &str,
    pos_side: &str,
) -> VenueDataResult<PositionId> {
    validate_okx_inst_id(inst_id)?;
    let pos_side = match pos_side {
        "net" | "long" | "short" => pos_side,
        _ => {
            return Err(VenueDataError::InvalidQuery {
                field: "okx.position.posSide",
                reason: "position side must be net, long or short",
            });
        }
    };
    PositionId::new(format!(
        "pos:{}:okx-swap:{}:{pos_side}",
        account_id.as_str(),
        symbol_identifier_component(inst_id)
    ))
    .map_err(VenueDataError::from)
}

fn bitget_usdt_futures_position_id(
    account_id: &AccountId,
    symbol: &str,
    hold_side: &str,
) -> VenueDataResult<PositionId> {
    validate_bitget_symbol(symbol)?;
    let hold_side = match hold_side {
        "long" | "short" => hold_side,
        _ => {
            return Err(VenueDataError::InvalidQuery {
                field: "bitget.position.holdSide",
                reason: "position side must be long or short",
            });
        }
    };
    PositionId::new(format!(
        "pos:{}:bitget-usdt-futures:{symbol}:{hold_side}",
        account_id.as_str()
    ))
    .map_err(VenueDataError::from)
}

fn aster_usdt_futures_position_id(
    account_id: &AccountId,
    symbol: &str,
    position_side: &str,
) -> VenueDataResult<PositionId> {
    validate_aster_symbol(symbol)?;
    validate_aster_position_side(position_side)?;
    PositionId::new(format!(
        "pos:{}:aster-usdt-futures:{symbol}:{}",
        account_id.as_str(),
        position_side.to_ascii_lowercase()
    ))
    .map_err(VenueDataError::from)
}

fn hyperliquid_perp_position_id(
    account_id: &AccountId,
    coin: &str,
    side: &str,
) -> VenueDataResult<PositionId> {
    validate_hyperliquid_coin(coin)?;
    let side = match side {
        "long" | "short" => side,
        _ => {
            return Err(VenueDataError::InvalidQuery {
                field: "hyperliquid.position.side",
                reason: "position side must be long or short",
            });
        }
    };
    PositionId::new(format!(
        "pos:{}:hyperliquid-perp:{}:{side}",
        account_id.as_str(),
        symbol_identifier_component(coin)
    ))
    .map_err(VenueDataError::from)
}

fn bybit_linear_position_id(
    account_id: &AccountId,
    symbol: &str,
    side: &str,
) -> VenueDataResult<PositionId> {
    validate_bybit_symbol(symbol)?;
    let side = match side {
        "Buy" => "long",
        "Sell" => "short",
        _ => {
            return Err(VenueDataError::InvalidQuery {
                field: "bybit.position.side",
                reason: "position side must be Buy or Sell for non-zero size",
            });
        }
    };
    PositionId::new(format!(
        "pos:{}:bybit-linear:{symbol}:{side}",
        account_id.as_str()
    ))
    .map_err(VenueDataError::from)
}

fn bybit_signed_position_quantity(size: Decimal, side: &str) -> VenueDataResult<Decimal> {
    match side {
        "Buy" => Ok(size),
        "Sell" => Decimal::from_scaled_atoms(0, 0)
            .checked_sub(size)
            .map_err(VenueDataError::from),
        _ => Err(VenueDataError::InvalidQuery {
            field: "bybit.position.side",
            reason: "position side must be Buy or Sell for non-zero size",
        }),
    }
}

fn okx_signed_position_quantity(quantity: Decimal, pos_side: &str) -> VenueDataResult<Decimal> {
    match pos_side {
        "net" => Ok(quantity),
        "long" => {
            if quantity.is_negative() {
                return Err(VenueDataError::InvalidQuery {
                    field: "okx.position.pos",
                    reason: "long position quantity must not be negative",
                });
            }
            Ok(quantity)
        }
        "short" => {
            if quantity.is_negative() {
                Ok(quantity)
            } else {
                quantity.checked_neg().map_err(VenueDataError::from)
            }
        }
        _ => Err(VenueDataError::InvalidQuery {
            field: "okx.position.posSide",
            reason: "position side must be net, long or short",
        }),
    }
}

fn bitget_signed_position_quantity(quantity: Decimal, hold_side: &str) -> VenueDataResult<Decimal> {
    if quantity.is_negative() {
        return Err(VenueDataError::InvalidQuery {
            field: "bitget.position.total",
            reason: "position total must not be negative",
        });
    }
    match hold_side {
        "long" => Ok(quantity),
        "short" => quantity.checked_neg().map_err(VenueDataError::from),
        _ => Err(VenueDataError::InvalidQuery {
            field: "bitget.position.holdSide",
            reason: "position side must be long or short",
        }),
    }
}

fn timestamp_from_unix_millis(value: u64) -> Result<UtcTimestamp, String> {
    let seconds = i64::try_from(value / 1_000)
        .map_err(|_| "closeTime milliseconds overflow i64 seconds".to_owned())?;
    let nanos = u32::try_from((value % 1_000) * 1_000_000)
        .map_err(|_| "closeTime millisecond remainder overflowed nanoseconds".to_owned())?;
    UtcTimestamp::from_unix_parts(seconds, nanos).map_err(|error| error.to_string())
}

fn timestamp_to_unix_millis(timestamp: UtcTimestamp) -> VenueDataResult<u64> {
    let seconds =
        u64::try_from(timestamp.unix_seconds()).map_err(|_| VenueDataError::InvalidQuery {
            field: "timestamp",
            reason: "timestamp must be at or after Unix epoch",
        })?;
    seconds
        .checked_mul(1_000)
        .and_then(|value| value.checked_add(u64::from(timestamp.nanoseconds() / 1_000_000)))
        .ok_or(VenueDataError::InvalidQuery {
            field: "timestamp",
            reason: "timestamp milliseconds overflowed",
        })
}

fn current_utc_timestamp(venue_id: &VenueId) -> VenueDataResult<UtcTimestamp> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            VenueDataError::External(ClassifiedExternalError::new(
                venue_id.clone(),
                ReadOnlySurface::MarketData,
                ExternalErrorClass::UnknownExternalState,
                format!("system clock is before Unix epoch: {error}"),
            ))
        })?;
    let seconds = i64::try_from(duration.as_secs()).map_err(|_| {
        VenueDataError::External(ClassifiedExternalError::new(
            venue_id.clone(),
            ReadOnlySurface::MarketData,
            ExternalErrorClass::UnknownExternalState,
            "system clock seconds overflow i64",
        ))
    })?;
    UtcTimestamp::from_unix_parts(seconds, duration.subsec_nanos()).map_err(VenueDataError::from)
}

fn push_ascii_hex_component(out: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('u');
    out.push(HEX[(byte >> 4) as usize] as char);
    out.push(HEX[(byte & 0x0f) as usize] as char);
}

fn symbol_identifier_component(symbol: &str) -> String {
    if symbol.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return symbol.to_owned();
    }

    let mut component = String::new();
    for byte in symbol.bytes() {
        if byte.is_ascii_alphanumeric() {
            component.push(byte as char);
        } else {
            push_ascii_hex_component(&mut component, byte);
        }
    }
    if component.is_empty() {
        "symbol".to_owned()
    } else {
        component
    }
}

fn raw_event_id(symbol: &str, source_sequence: u64) -> String {
    let symbol = symbol_identifier_component(symbol);
    format!("event:venue-data:binance-public:{symbol}:{source_sequence}:raw")
}

fn normalized_event_id(symbol: &str, source_sequence: u64) -> String {
    let symbol = symbol_identifier_component(symbol);
    format!("event:venue-data:binance-public:{symbol}:{source_sequence}:normalized")
}

fn correlation_id(symbol: &str, source_sequence: u64) -> String {
    let symbol = symbol_identifier_component(symbol);
    format!("corr:venue-data:binance-public:{symbol}:{source_sequence}")
}

fn binance_public_raw_event_id(
    stream: &str,
    market: BinancePublicMarket,
    symbol: &str,
    source_sequence: &str,
) -> String {
    let symbol = symbol_identifier_component(symbol);
    format!(
        "event:venue-data:binance-public:{stream}:{}:{symbol}:{source_sequence}:raw",
        market.event_scope()
    )
}

fn binance_public_normalized_event_id(
    stream: &str,
    market: BinancePublicMarket,
    symbol: &str,
    source_sequence: &str,
) -> String {
    let symbol = symbol_identifier_component(symbol);
    format!(
        "event:venue-data:binance-public:{stream}:{}:{symbol}:{source_sequence}:normalized",
        market.event_scope()
    )
}

fn binance_public_correlation_id(
    stream: &str,
    market: BinancePublicMarket,
    symbol: &str,
    source_sequence: &str,
) -> String {
    let symbol = symbol_identifier_component(symbol);
    format!(
        "corr:venue-data:binance-public:{stream}:{}:{symbol}:{source_sequence}",
        market.event_scope()
    )
}

fn bybit_public_raw_event_id(
    stream: &str,
    market: BybitPublicMarket,
    symbol: &str,
    source_sequence: &str,
) -> String {
    format!(
        "event:venue-data:bybit-public:{stream}:{}:{symbol}:{source_sequence}:raw",
        market.event_scope()
    )
}

fn bybit_public_normalized_event_id(
    stream: &str,
    market: BybitPublicMarket,
    symbol: &str,
    source_sequence: &str,
) -> String {
    format!(
        "event:venue-data:bybit-public:{stream}:{}:{symbol}:{source_sequence}:normalized",
        market.event_scope()
    )
}

fn bybit_public_correlation_id(
    stream: &str,
    market: BybitPublicMarket,
    symbol: &str,
    source_sequence: &str,
) -> String {
    format!(
        "corr:venue-data:bybit-public:{stream}:{}:{symbol}:{source_sequence}",
        market.event_scope()
    )
}

const BYBIT_PUBLIC_WSS_SUBSCRIBE_BATCH_SIZE: usize = 10;

fn bybit_public_wss_subscribe_payloads(args: &[String]) -> Vec<String> {
    args.chunks(BYBIT_PUBLIC_WSS_SUBSCRIBE_BATCH_SIZE)
        .map(bybit_public_wss_subscribe_payload)
        .collect()
}

fn bybit_public_wss_subscribe_payload(args: &[String]) -> String {
    format!(
        "{{\"args\":[{}],\"op\":\"subscribe\"}}",
        args.iter()
            .map(|arg| json_string(arg))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn binance_public_wss_source_event_id(
    market: BinancePublicMarket,
    symbol: &str,
    local_sequence: u64,
    update_id: u64,
) -> String {
    let symbol = symbol_identifier_component(symbol);
    format!(
        "event:venue-data:binance-public:wss-book-ticker:{}:{symbol}:{local_sequence}:u{update_id}",
        market.event_scope()
    )
}

fn binance_private_event_id(
    kind: &str,
    market: BinancePrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "event:venue-data:binance-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn binance_private_correlation_id(
    kind: &str,
    market: BinancePrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "corr:venue-data:binance-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn bybit_private_event_id(
    kind: &str,
    market: BybitPrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "event:venue-data:bybit-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn bybit_private_correlation_id(
    kind: &str,
    market: BybitPrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "corr:venue-data:bybit-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn okx_private_event_id(
    kind: &str,
    market: OkxPrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "event:venue-data:okx-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn okx_private_correlation_id(
    kind: &str,
    market: OkxPrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "corr:venue-data:okx-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn bitget_private_event_id(
    kind: &str,
    market: BitgetPrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "event:venue-data:bitget-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn bitget_private_correlation_id(
    kind: &str,
    market: BitgetPrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "corr:venue-data:bitget-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn aster_private_event_id(
    kind: &str,
    market: AsterPrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "event:venue-data:aster-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn aster_private_correlation_id(
    kind: &str,
    market: AsterPrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "corr:venue-data:aster-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn hyperliquid_private_event_id(
    kind: &str,
    market: HyperliquidPrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "event:venue-data:hyperliquid-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn hyperliquid_private_correlation_id(
    kind: &str,
    market: HyperliquidPrivateAccountMarket,
    account_id: &AccountId,
    source_sequence: &str,
) -> String {
    format!(
        "corr:venue-data:hyperliquid-private:{kind}:{}:{account_id}:{source_sequence}",
        market.event_scope()
    )
}

fn freshness_payload_json(freshness: DataFreshness) -> String {
    format!(
        "{{\"age_ms\":{},\"ingested_at\":{},\"max_age_ms\":{},\"observed_at\":{},\"status\":\"{}\"}}",
        freshness.age_ms(),
        json_string(&freshness.ingested_at.to_string()),
        freshness.max_age_ms,
        json_string(&freshness.observed_at.to_string()),
        freshness.status.as_str(),
    )
}

fn rate_limit_payload_json(rate_limit: &RateLimitSnapshot) -> String {
    format!(
        "{{\"limit\":{},\"remaining\":{},\"resets_at\":{},\"window_ms\":{}}}",
        rate_limit.limit,
        rate_limit
            .remaining
            .map_or_else(|| "null".to_owned(), |value| value.to_string()),
        rate_limit.resets_at.map_or_else(
            || "null".to_owned(),
            |timestamp| json_string(&timestamp.to_string())
        ),
        rate_limit.window_ms,
    )
}

fn build_normalized_event(envelope: EventEnvelope) -> VenueDataResult<NormalizedEvent> {
    let placeholder_json = event_json(
        &envelope,
        "sha256:placeholder00000000000000000000000000000000000000000000000000000000",
    );
    let placeholder = parse_normalized_event(&placeholder_json)?;
    let checksum = canonical_normalized_event_hash(&placeholder);
    let event = parse_normalized_event(&event_json(&envelope, &checksum))?;
    debug_assert_eq!(
        canonical_normalized_event_hash(&event),
        event.checksum.as_str()
    );
    Ok(event)
}

fn event_json(envelope: &EventEnvelope, checksum: &str) -> String {
    let mut fields = vec![
        format!("\"event_id\":{}", json_string(&envelope.event_id)),
        format!(
            "\"event_type\":{}",
            json_string(envelope.event_type.as_str())
        ),
        "\"event_version\":\"1.0.0\"".to_owned(),
        format!(
            "\"timestamp_event\":{}",
            json_string(&envelope.timestamp_event.to_string())
        ),
        format!(
            "\"timestamp_ingested\":{}",
            json_string(&envelope.timestamp_ingested.to_string())
        ),
        format!("\"source\":{}", json_string(&envelope.source)),
        format!(
            "\"correlation_id\":{}",
            json_string(&envelope.correlation_id)
        ),
        "\"schema_version\":\"1.0.0\"".to_owned(),
        format!("\"payload\":{}", envelope.payload_json),
        format!("\"checksum\":{}", json_string(checksum)),
    ];

    if let Some(source_sequence) = &envelope.source_sequence {
        fields.push(format!(
            "\"source_sequence\":{}",
            json_string(source_sequence)
        ));
    }
    if let Some(causation_id) = &envelope.causation_id {
        fields.push(format!("\"causation_id\":{}", json_string(causation_id)));
    }
    if let Some(venue_id) = &envelope.venue_id {
        fields.push(format!("\"venue_id\":{}", json_string(venue_id)));
    }
    if let Some(instrument_id) = &envelope.instrument_id {
        fields.push(format!("\"instrument_id\":{}", json_string(instrument_id)));
    }

    format!("{{{}}}", fields.join(","))
}

fn parse_normalized_event(input: &str) -> VenueDataResult<NormalizedEvent> {
    from_json_strict::<NormalizedEvent>(input).map_err(|_error| VenueDataError::InvalidQuery {
        field: "normalized_event",
        reason: "constructed normalized event failed contract validation",
    })
}

/// 计算标准化事件哈希。
///
/// 中文说明：为了保持 `arb-venue-data` 不依赖事件存储 crate，这里只复制事件哈希
/// 的纯函数算法：规范 JSON 去掉自引用 `checksum` 字段后计算 SHA-256。
pub fn canonical_normalized_event_hash(event: &NormalizedEvent) -> String {
    match event.to_json_value() {
        JsonValue::Object(mut fields) => {
            fields.remove("checksum");
            hash_canonical_bytes(JsonValue::Object(fields).to_canonical_json().as_bytes())
        }
        value => hash_canonical_bytes(value.to_canonical_json().as_bytes()),
    }
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn json_string_array<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::from("[");
    for (index, value) in values.enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_string(value));
    }
    out.push(']');
    out
}

fn hash_canonical_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

fn sha256_hex(input: &[u8]) -> String {
    let digest = sha256(input);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

fn sha256(input: &[u8]) -> [u8; 32] {
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

    let mut message = input.to_vec();
    let bit_len = (message.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    let mut state = H0;
    for chunk in message.chunks_exact(64) {
        let mut schedule = [0_u32; 64];
        let mut index = 0;
        while index < 16 {
            let offset = index * 4;
            schedule[index] = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
            index += 1;
        }

        while index < 64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
            index += 1;
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];

        let mut round = 0;
        while round < 64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[round])
                .wrapping_add(schedule[round]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);

            round += 1;
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

    let mut digest = [0_u8; 32];
    for (index, word) in state.into_iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_contracts::{to_canonical_json, VenueCapabilityDescriptor};
    use std::fs;
    use std::path::PathBuf;
    use std::str::FromStr;

    #[test]
    fn binance_server_time_parser_reads_public_rest_millis() {
        let body = r#"{"serverTime":1779274612345}"#;

        let server_time = parse_binance_server_time_millis(body).expect("server time");

        assert_eq!(server_time, 1_779_274_612_345);
    }

    #[test]
    fn binance_server_time_parser_fails_closed_on_missing_or_invalid_field() {
        assert!(parse_binance_server_time_millis("{}").is_err());
        assert!(parse_binance_server_time_millis(r#"{"serverTime":"not-a-number"}"#).is_err());
    }

    #[derive(Clone)]
    struct FixtureReadOnlyAdapter {
        venue_id: VenueId,
        quote: MarketQuote,
        balances: Vec<VenueBalance>,
        positions: Vec<VenuePosition>,
        instruments: Vec<InstrumentInfo>,
        health: VenueHealthSnapshot,
    }

    impl VenueReadAdapter for FixtureReadOnlyAdapter {
        fn venue_id(&self) -> &VenueId {
            &self.venue_id
        }
    }

    impl MarketDataReader for FixtureReadOnlyAdapter {
        fn latest_quote(&self, query: &MarketDataQuery) -> VenueDataResult<Option<MarketQuote>> {
            Ok((query.venue_id == self.quote.venue_id
                && query.instrument_id == self.quote.instrument_id)
                .then(|| self.quote.clone()))
        }

        fn order_book(
            &self,
            _query: &MarketDataQuery,
        ) -> VenueDataResult<Option<OrderBookSnapshot>> {
            Ok(None)
        }
    }

    impl BalanceReader for FixtureReadOnlyAdapter {
        fn balances(&self, query: &BalanceQuery) -> VenueDataResult<Vec<VenueBalance>> {
            Ok(self
                .balances
                .iter()
                .filter(|balance| balance.venue_id == query.venue_id)
                .filter(|balance| {
                    query
                        .account_id
                        .as_ref()
                        .is_none_or(|account_id| account_id == &balance.account_id)
                })
                .filter(|balance| {
                    query
                        .asset_id
                        .as_ref()
                        .is_none_or(|asset_id| asset_id == &balance.asset_id)
                })
                .cloned()
                .collect())
        }
    }

    impl PositionReader for FixtureReadOnlyAdapter {
        fn positions(&self, query: &PositionQuery) -> VenueDataResult<Vec<VenuePosition>> {
            Ok(self
                .positions
                .iter()
                .filter(|position| position.venue_id == query.venue_id)
                .filter(|position| {
                    query
                        .account_id
                        .as_ref()
                        .is_none_or(|account_id| account_id == &position.account_id)
                })
                .filter(|position| {
                    query
                        .instrument_id
                        .as_ref()
                        .is_none_or(|instrument_id| instrument_id == &position.instrument_id)
                })
                .cloned()
                .collect())
        }
    }

    impl InstrumentInfoReader for FixtureReadOnlyAdapter {
        fn instruments(&self, query: &InstrumentInfoQuery) -> VenueDataResult<Vec<InstrumentInfo>> {
            Ok(self
                .instruments
                .iter()
                .filter(|instrument| instrument.venue_id == query.venue_id)
                .filter(|instrument| {
                    query
                        .instrument_id
                        .as_ref()
                        .is_none_or(|instrument_id| instrument_id == &instrument.instrument_id)
                })
                .cloned()
                .collect())
        }
    }

    impl VenueHealthReader for FixtureReadOnlyAdapter {
        fn venue_health(&self, venue_id: &VenueId) -> VenueDataResult<VenueHealthSnapshot> {
            if venue_id == &self.health.venue_id {
                Ok(self.health.clone())
            } else {
                Err(VenueDataError::DataUnavailable {
                    venue_id: venue_id.clone(),
                    surface: ReadOnlySurface::VenueHealth,
                    reason: "fixture has no health snapshot for venue".to_owned(),
                })
            }
        }
    }

    #[test]
    fn bybit_public_wss_subscribe_payloads_are_chunked() {
        let args = (0..25)
            .map(|index| format!("orderbook.1.TEST{index}USDT"))
            .collect::<Vec<_>>();

        let payloads = bybit_public_wss_subscribe_payloads(&args);

        assert_eq!(payloads.len(), 3);
        assert!(payloads[0].contains("orderbook.1.TEST0USDT"));
        assert!(payloads[0].contains("orderbook.1.TEST9USDT"));
        assert!(!payloads[0].contains("orderbook.1.TEST10USDT"));
        assert!(payloads[1].contains("orderbook.1.TEST10USDT"));
        assert!(payloads[1].contains("orderbook.1.TEST19USDT"));
        assert!(payloads[2].contains("orderbook.1.TEST20USDT"));
        assert!(payloads[2].contains("orderbook.1.TEST24USDT"));
    }

    #[test]
    fn freshness_classifies_fresh_and_stale_snapshots() {
        let observed = timestamp("2026-01-01T00:00:00Z");
        let fresh_ingested = timestamp("2026-01-01T00:00:03Z");
        let stale_ingested = timestamp("2026-01-01T00:00:06Z");

        let fresh = DataFreshness::new(observed, fresh_ingested, 5_000).expect("freshness");
        assert_eq!(fresh.status, FreshnessStatus::Fresh);
        assert_eq!(fresh.age_ms(), 3_000);
        assert!(!fresh.is_stale());

        let stale = DataFreshness::new(observed, stale_ingested, 5_000).expect("freshness");
        assert_eq!(stale.status, FreshnessStatus::Stale);
        assert_eq!(stale.age_ms(), 6_000);
        assert!(stale.is_stale());
    }

    #[test]
    fn freshness_rejects_inverted_timestamps() {
        let error = DataFreshness::new(
            timestamp("2026-01-01T00:00:03Z"),
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect_err("inverted timestamps must fail");

        assert!(matches!(
            error,
            VenueDataError::InvalidQuery {
                field: "freshness.ingested_at",
                ..
            }
        ));
    }

    #[test]
    fn adapter_exposes_all_required_read_only_surfaces() {
        let adapter = fixture_adapter();
        let venue_id = VenueId::new("venue:SIM").expect("venue id");
        let account_id = AccountId::new("acct:cash").expect("account id");
        let asset_id = AssetId::new("asset:USDC").expect("asset id");
        let instrument_id = InstrumentId::new("inst:BTC-USDC").expect("instrument id");

        let quote = adapter
            .latest_quote(&MarketDataQuery::new(
                venue_id.clone(),
                instrument_id.clone(),
            ))
            .expect("quote read")
            .expect("quote");
        assert_eq!(quote.best_bid.expect("bid").to_string(), "30000.00");

        let balances = adapter
            .balances(
                &BalanceQuery::new(venue_id.clone())
                    .for_account(account_id.clone())
                    .for_asset(asset_id.clone()),
            )
            .expect("balance read");
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].free.to_string(), "1000.00");

        let positions = adapter
            .positions(
                &PositionQuery::new(venue_id.clone())
                    .for_account(account_id)
                    .for_instrument(instrument_id.clone()),
            )
            .expect("position read");
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].quantity.to_string(), "0.10");

        let instruments = adapter
            .instruments(&InstrumentInfoQuery::new(venue_id.clone()).for_instrument(instrument_id))
            .expect("instrument read");
        assert_eq!(instruments.len(), 1);
        assert_eq!(instruments[0].kind, InstrumentKind::SpotPair);

        let health = adapter.venue_health(&venue_id).expect("health read");
        assert_eq!(health.status, VenueHealthStatus::Healthy);
        assert_eq!(health.connection, VenueConnectionStatus::Connected);
    }

    #[test]
    fn okx_private_account_balance_maps_trading_account_details() {
        let venue_id = VenueId::new("venue:OKX-PRIVATE").expect("venue id");
        let account_id = AccountId::new("account:okx-trading-unit").expect("account id");
        let mut adapter = OkxPrivateAccountAdapter::new(
            venue_id.clone(),
            account_id.clone(),
            OkxPrivateAccountMarket::TradingAccount,
            timestamp("2023-11-14T22:13:20Z"),
            5_000,
        )
        .expect("adapter");

        let batch = adapter
            .ingest_account_balance_json(
                r#"{"code":"0","msg":"","data":[{"uTime":"1700000000123","details":[{"ccy":"USDT","availBal":"123.45","frozenBal":"1.00","ordFrozen":"2.00","liab":"0.50"},{"ccy":"BTC","availBal":"0.01000000","frozenBal":"0","ordFrozen":"0","liab":"0"}]}]}"#,
                "test:okx-account-balance",
                timestamp("2023-11-14T22:13:22Z"),
            )
            .expect("OKX account balance");

        assert_eq!(batch.balances.len(), 2);
        let usdt = batch
            .balances
            .iter()
            .find(|balance| balance.asset_id.as_str() == "asset:USDT")
            .expect("USDT balance");
        assert_eq!(usdt.account_id, account_id);
        assert_eq!(usdt.free, amount("123.45"));
        assert_eq!(usdt.locked, amount("1.00"));
        assert_eq!(usdt.reserved, amount("2.00"));
        assert_eq!(usdt.borrowed, amount("0.50"));
        assert_eq!(
            batch.balance_event.event_type,
            NormalizedEventType::BalanceSnapshotEvent
        );
        assert!(payload_string(&batch.balance_event, "redaction")
            .contains("private_account_amounts_available"));

        let queried = adapter
            .balances(
                &BalanceQuery::new(venue_id.clone())
                    .for_account(AccountId::new("account:okx-trading-unit").expect("account id"))
                    .for_asset(AssetId::new("asset:USDT").expect("asset id")),
            )
            .expect("balance query");
        assert_eq!(queried.len(), 1);
        let health = adapter.venue_health(&venue_id).expect("health");
        assert_eq!(health.status, VenueHealthStatus::Healthy);

        assert!(matches!(
            adapter.latest_quote(&MarketDataQuery::new(
                venue_id,
                InstrumentId::new("inst:OKX:BTC-USDT:SPOT").expect("instrument")
            )),
            Err(VenueDataError::DataUnavailable {
                surface: ReadOnlySurface::MarketData,
                ..
            })
        ));
    }

    #[test]
    fn okx_private_trading_account_maps_swap_positions() {
        let venue_id = VenueId::new("venue:OKX-PRIVATE").expect("venue id");
        let account_id = AccountId::new("account:okx-trading-unit").expect("account id");
        let mut adapter = OkxPrivateAccountAdapter::new(
            venue_id.clone(),
            account_id.clone(),
            OkxPrivateAccountMarket::TradingAccount,
            timestamp("2023-11-14T22:13:20Z"),
            5_000,
        )
        .expect("adapter");

        let batch = adapter
            .ingest_account_positions_json(
                r#"{"code":"0","msg":"","data":[{"instType":"SWAP","instId":"BTC-USDT-SWAP","pos":"2","posSide":"short","avgPx":"43100.00","markPx":"43250.50","upl":"-301.00","liqPx":"45000.00","uTime":"1700000001500"},{"instType":"SWAP","instId":"ETH-USDT-SWAP","pos":"0","posSide":"net","avgPx":"0","markPx":"2500.00","upl":"0","liqPx":"","uTime":"1700000001500"}]}"#,
                "test:okx-account-positions",
                timestamp("2023-11-14T22:13:22Z"),
            )
            .expect("OKX account positions");

        assert_eq!(
            batch.position_event.event_type,
            NormalizedEventType::PositionSnapshotEvent
        );
        assert_eq!(
            payload_string(&batch.position_event, "endpoint"),
            "/api/v5/account/positions"
        );
        assert_eq!(batch.positions.len(), 1);
        let position = &batch.positions[0];
        assert_eq!(position.account_id, account_id);
        assert_eq!(
            position.instrument_id.as_str(),
            "inst:OKX:BTC-USDT-SWAP:SWAP"
        );
        assert_eq!(position.quantity.to_string(), "-2");
        assert_eq!(position.entry_price.expect("entry").to_string(), "43100.00");
        assert_eq!(position.mark_price.to_string(), "43250.50");
        assert_eq!(position.unrealized_pnl.to_string(), "-301.00");
        assert_eq!(
            position.liquidation_price.expect("liq").to_string(),
            "45000.00"
        );
        assert_eq!(
            position.source_event_id.as_deref(),
            Some(batch.position_event.event_id.as_str())
        );

        let queried = adapter
            .positions(
                &PositionQuery::new(venue_id.clone())
                    .for_account(AccountId::new("account:okx-trading-unit").expect("account id"))
                    .for_instrument(
                        InstrumentId::new("inst:OKX:BTC-USDT-SWAP:SWAP").expect("instrument"),
                    ),
            )
            .expect("position query");
        assert_eq!(queried, batch.positions);
        let rendered = to_canonical_json(&batch.position_event);
        assert!(!rendered.contains("43100.00"));
        assert!(!rendered.contains("api_key"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn okx_private_position_snapshot_accepts_empty_positions() {
        let venue_id = VenueId::new("venue:OKX-PRIVATE").expect("venue id");
        let mut adapter = OkxPrivateAccountAdapter::new(
            venue_id.clone(),
            AccountId::new("account:okx-trading-unit").expect("account id"),
            OkxPrivateAccountMarket::TradingAccount,
            timestamp("2023-11-14T22:13:20Z"),
            5_000,
        )
        .expect("adapter");

        let batch = adapter
            .ingest_account_positions_json(
                r#"{"code":"0","msg":"","data":[]}"#,
                "test:okx-account-positions-empty",
                timestamp("2023-11-14T22:13:22Z"),
            )
            .expect("empty OKX account positions");

        assert!(batch.positions.is_empty());
        assert_eq!(
            payload_string(&batch.position_event, "risk_reason_code"),
            "CHECK_PASSED"
        );
        let queried = adapter
            .positions(&PositionQuery::new(venue_id))
            .expect("position query");
        assert!(queried.is_empty());
    }

    #[test]
    fn bitget_private_spot_assets_map_account_balances() {
        let venue_id = VenueId::new("venue:BITGET-SPOT-PRIVATE").expect("venue id");
        let account_id = AccountId::new("account:bitget-spot-unit").expect("account id");
        let mut adapter = BitgetPrivateAccountAdapter::new(
            venue_id.clone(),
            account_id.clone(),
            BitgetPrivateAccountMarket::SpotAccount,
            timestamp("2023-11-14T22:13:20Z"),
            5_000,
        )
        .expect("adapter");

        let batch = adapter
            .ingest_spot_assets_json(
                r#"{"code":"00000","msg":"success","requestTime":1700000000123,"data":[{"coin":"usdt","available":"123.45","frozen":"1.00","locked":"2.00"},{"coin":"BTC","available":"0.01000000","frozen":"0","locked":"0"}]}"#,
                "test:bitget-spot-assets",
                timestamp("2023-11-14T22:13:22Z"),
            )
            .expect("Bitget spot assets");

        assert_eq!(batch.balances.len(), 2);
        let usdt = batch
            .balances
            .iter()
            .find(|balance| balance.asset_id.as_str() == "asset:USDT")
            .expect("USDT balance");
        assert_eq!(usdt.account_id, account_id);
        assert_eq!(usdt.free, amount("123.45"));
        assert_eq!(usdt.locked, amount("1.00"));
        assert_eq!(usdt.reserved, amount("2.00"));
        assert_eq!(usdt.unsettled, amount("0"));
        assert_eq!(
            payload_string(&batch.balance_event, "endpoint"),
            "/api/v2/spot/account/assets"
        );
        assert!(payload_string(&batch.balance_event, "redaction")
            .contains("private_account_amounts_available"));

        let queried = adapter
            .balances(
                &BalanceQuery::new(venue_id.clone())
                    .for_account(AccountId::new("account:bitget-spot-unit").expect("account id"))
                    .for_asset(AssetId::new("asset:USDT").expect("asset id")),
            )
            .expect("balance query");
        assert_eq!(queried.len(), 1);
        let health = adapter.venue_health(&venue_id).expect("health");
        assert_eq!(health.status, VenueHealthStatus::Healthy);
    }

    #[test]
    fn bitget_private_usdt_futures_accounts_map_margin_balance() {
        let venue_id = VenueId::new("venue:BITGET-USDT-FUTURES-PRIVATE").expect("venue id");
        let account_id = AccountId::new("account:bitget-futures-unit").expect("account id");
        let mut adapter = BitgetPrivateAccountAdapter::new(
            venue_id.clone(),
            account_id.clone(),
            BitgetPrivateAccountMarket::UsdtFuturesAccount,
            timestamp("2023-11-14T22:13:20Z"),
            5_000,
        )
        .expect("adapter");

        let batch = adapter
            .ingest_usdt_futures_accounts_json(
                r#"{"code":"00000","msg":"success","requestTime":1700000000123,"data":[{"marginCoin":"USDT","available":"99.00","locked":"1.00","unrealizedPL":"-0.50"}]}"#,
                "test:bitget-futures-accounts",
                timestamp("2023-11-14T22:13:22Z"),
            )
            .expect("Bitget futures accounts");

        assert_eq!(batch.balances.len(), 1);
        let usdt = &batch.balances[0];
        assert_eq!(usdt.account_id, account_id);
        assert_eq!(usdt.asset_id.as_str(), "asset:USDT");
        assert_eq!(usdt.free, amount("99.00"));
        assert_eq!(usdt.locked, amount("1.00"));
        assert_eq!(usdt.unsettled, amount("0"));
        assert_eq!(
            payload_string(&batch.balance_event, "endpoint"),
            "/api/v2/mix/account/accounts"
        );

        let health = adapter.venue_health(&venue_id).expect("health");
        assert_eq!(health.status, VenueHealthStatus::Healthy);
    }

    #[test]
    fn bitget_private_usdt_futures_positions_map_signed_exposure() {
        let venue_id = VenueId::new("venue:BITGET-USDT-FUTURES-PRIVATE").expect("venue id");
        let account_id = AccountId::new("account:bitget-futures-unit").expect("account id");
        let mut adapter = BitgetPrivateAccountAdapter::new(
            venue_id.clone(),
            account_id.clone(),
            BitgetPrivateAccountMarket::UsdtFuturesAccount,
            timestamp("2023-11-14T22:13:20Z"),
            5_000,
        )
        .expect("adapter");

        let batch = adapter
            .ingest_usdt_futures_positions_json(
                r#"{"code":"00000","msg":"success","requestTime":1700000000123,"data":[{"symbol":"BTCUSDT","marginCoin":"USDT","holdSide":"short","openDelegateSize":"0","marginSize":"100","available":"1.000","locked":"0.500","total":"1.500","leverage":"5","openPriceAvg":"43100.00","unrealizedPL":"-225.75","liquidationPrice":"45000.00","markPrice":"43250.50","uTime":"1700000001500"},{"symbol":"ETHUSDT","marginCoin":"USDT","holdSide":"long","total":"0","openPriceAvg":"0","unrealizedPL":"0","liquidationPrice":"0","markPrice":"2500.00","uTime":"1700000001500"}]}"#,
                "test:bitget-futures-positions",
                timestamp("2023-11-14T22:13:22Z"),
            )
            .expect("Bitget futures positions");

        assert_eq!(
            batch.position_event.event_type,
            NormalizedEventType::PositionSnapshotEvent
        );
        assert_eq!(
            payload_string(&batch.position_event, "endpoint"),
            "/api/v2/mix/position/all-position"
        );
        assert_eq!(batch.positions.len(), 1);
        let position = &batch.positions[0];
        assert_eq!(position.account_id, account_id);
        assert_eq!(
            position.instrument_id.as_str(),
            "inst:BITGET:BTCUSDT:USDT-FUTURES"
        );
        assert_eq!(position.quantity.to_string(), "-1.500");
        assert_eq!(position.entry_price.expect("entry").to_string(), "43100.00");
        assert_eq!(position.mark_price.to_string(), "43250.50");
        assert_eq!(position.unrealized_pnl.to_string(), "-225.75");
        assert_eq!(
            position.liquidation_price.expect("liq").to_string(),
            "45000.00"
        );

        let queried = adapter
            .positions(
                &PositionQuery::new(venue_id.clone())
                    .for_account(AccountId::new("account:bitget-futures-unit").expect("account id"))
                    .for_instrument(
                        InstrumentId::new("inst:BITGET:BTCUSDT:USDT-FUTURES").expect("instrument"),
                    ),
            )
            .expect("position query");
        assert_eq!(queried, batch.positions);
        let rendered = to_canonical_json(&batch.position_event);
        assert!(!rendered.contains("43100.00"));
        assert!(!rendered.contains("api_key"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn bitget_private_position_requires_mark_price_for_nonzero_position() {
        let mut adapter = BitgetPrivateAccountAdapter::new(
            VenueId::new("venue:BITGET-USDT-FUTURES-PRIVATE").expect("venue id"),
            AccountId::new("account:bitget-futures-unit").expect("account id"),
            BitgetPrivateAccountMarket::UsdtFuturesAccount,
            timestamp("2023-11-14T22:13:20Z"),
            5_000,
        )
        .expect("adapter");

        let error = adapter
            .ingest_usdt_futures_positions_json(
                r#"{"code":"00000","msg":"success","requestTime":1700000000123,"data":[{"symbol":"BTCUSDT","holdSide":"long","total":"1.000","openPriceAvg":"43100.00","unrealizedPL":"1.00","liquidationPrice":"0","uTime":"1700000001500"}]}"#,
                "test:bitget-futures-positions-missing-mark",
                timestamp("2023-11-14T22:13:22Z"),
            )
            .expect_err("nonzero Bitget position without mark price is unsafe");

        let error = expect_external(error);
        assert_eq!(error.surface, ReadOnlySurface::Position);
        assert_eq!(error.class, ExternalErrorClass::MissingField);
        assert!(error.fail_closed);
    }

    #[test]
    fn bitget_private_balance_fails_closed_on_non_success_code() {
        let mut adapter = BitgetPrivateAccountAdapter::new(
            VenueId::new("venue:BITGET-SPOT-PRIVATE").expect("venue id"),
            AccountId::new("account:bitget-spot-unit").expect("account id"),
            BitgetPrivateAccountMarket::SpotAccount,
            timestamp("2023-11-14T22:13:20Z"),
            5_000,
        )
        .expect("adapter");

        let error = adapter
            .ingest_spot_assets_json(
                r#"{"code":"40001","msg":"invalid request","requestTime":1700000000123,"data":[]}"#,
                "test:bitget-spot-assets-error",
                timestamp("2023-11-14T22:13:22Z"),
            )
            .expect_err("Bitget non-success code must fail closed");

        assert!(matches!(error, VenueDataError::External(_)));
    }

    #[test]
    fn aster_private_usdt_futures_balance_maps_available_amounts_without_credentials() {
        let venue_id = VenueId::new("venue:ASTER-USDT-FUTURES-PRIVATE").expect("venue id");
        let account_id = AccountId::new("account:aster-futures-unit").expect("account id");
        let mut adapter = AsterPrivateAccountAdapter::new(
            venue_id.clone(),
            account_id.clone(),
            AsterPrivateAccountMarket::UsdtFuturesAccount,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("adapter");

        let batch = adapter
            .ingest_usdt_futures_balance_json(
                r#"[{"accountAlias":"SgsR","asset":"USDT","balance":"124.45","crossWalletBalance":"124.45","crossUnPnl":"0.00000000","availableBalance":"123.45","maxWithdrawAmount":"123.45","marginAvailable":true,"updateTime":1767225601000},{"accountAlias":"SgsR","asset":"BTC","balance":"0.01000000","crossWalletBalance":"0.01000000","crossUnPnl":"0","availableBalance":"0.01000000","maxWithdrawAmount":"0.01000000","marginAvailable":true,"updateTime":1767225601000}]"#,
                "test:aster-futures-balance",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("Aster futures balance");

        assert_eq!(
            batch.balance_event.event_type,
            NormalizedEventType::BalanceSnapshotEvent
        );
        assert_eq!(
            payload_string(&batch.balance_event, "endpoint"),
            "/fapi/v3/balance"
        );
        assert_eq!(
            payload_string(&batch.balance_event, "risk_reason_code"),
            "CHECK_PASSED"
        );
        assert_eq!(batch.balances.len(), 2);
        let usdt = batch
            .balances
            .iter()
            .find(|balance| balance.asset_id.as_str() == "asset:USDT")
            .expect("USDT balance");
        assert_eq!(usdt.account_id, account_id);
        assert_eq!(usdt.free, amount("123.45"));
        assert_eq!(usdt.locked, amount("1.00"));
        assert_eq!(
            usdt.source_event_id.as_deref(),
            Some(batch.balance_event.event_id.as_str())
        );

        let queried = adapter
            .balances(
                &BalanceQuery::new(venue_id.clone())
                    .for_account(AccountId::new("account:aster-futures-unit").expect("account id"))
                    .for_asset(AssetId::new("asset:USDT").expect("asset id")),
            )
            .expect("balance query");
        assert_eq!(queried.len(), 1);
        let health = adapter.venue_health(&venue_id).expect("health");
        assert_eq!(health.status, VenueHealthStatus::Healthy);

        let rendered = to_canonical_json(&batch.balance_event);
        assert!(!rendered.contains("123.45"));
        assert!(!rendered.contains("api_key"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn aster_private_usdt_futures_position_risk_maps_signed_positions() {
        let venue_id = VenueId::new("venue:ASTER-USDT-FUTURES-PRIVATE").expect("venue id");
        let account_id = AccountId::new("account:aster-futures-unit").expect("account id");
        let mut adapter = AsterPrivateAccountAdapter::new(
            venue_id.clone(),
            account_id.clone(),
            AsterPrivateAccountMarket::UsdtFuturesAccount,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("adapter");

        let batch = adapter
            .ingest_usdt_futures_position_risk_json(
                r#"[{"symbol":"BTCUSDT","positionSide":"SHORT","positionAmt":"-0.500","entryPrice":"43100.00","markPrice":"43250.50","unRealizedProfit":"-75.25","liquidationPrice":"45000.00","updateTime":1767225601500},{"symbol":"ETHUSDT","positionSide":"BOTH","positionAmt":"0","entryPrice":"0","markPrice":"2500.00","unRealizedProfit":"0","liquidationPrice":"0","updateTime":1767225601500}]"#,
                "test:aster-futures-position-risk",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("Aster futures positions");

        assert_eq!(
            batch.position_event.event_type,
            NormalizedEventType::PositionSnapshotEvent
        );
        assert_eq!(
            payload_string(&batch.position_event, "endpoint"),
            "/fapi/v3/positionRisk"
        );
        assert_eq!(batch.positions.len(), 1);
        let position = &batch.positions[0];
        assert_eq!(position.account_id, account_id);
        assert_eq!(
            position.instrument_id.as_str(),
            "inst:ASTER:BTCUSDT:USDT-FUTURES"
        );
        assert_eq!(position.quantity.to_string(), "-0.500");
        assert_eq!(position.entry_price.expect("entry").to_string(), "43100.00");
        assert_eq!(position.mark_price.to_string(), "43250.50");
        assert_eq!(position.unrealized_pnl.to_string(), "-75.25");
        assert_eq!(
            position.liquidation_price.expect("liq").to_string(),
            "45000.00"
        );
        assert_eq!(
            position.source_event_id.as_deref(),
            Some(batch.position_event.event_id.as_str())
        );

        let queried = adapter
            .positions(
                &PositionQuery::new(venue_id.clone())
                    .for_account(AccountId::new("account:aster-futures-unit").expect("account id"))
                    .for_instrument(
                        InstrumentId::new("inst:ASTER:BTCUSDT:USDT-FUTURES").expect("instrument"),
                    ),
            )
            .expect("position query");
        assert_eq!(queried, batch.positions);

        let rendered = to_canonical_json(&batch.position_event);
        assert!(!rendered.contains("43100.00"));
        assert!(!rendered.contains("api_key"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn aster_private_position_requires_mark_price_for_nonzero_position() {
        let mut adapter = AsterPrivateAccountAdapter::new(
            VenueId::new("venue:ASTER-USDT-FUTURES-PRIVATE").expect("venue id"),
            AccountId::new("account:aster-futures-unit").expect("account id"),
            AsterPrivateAccountMarket::UsdtFuturesAccount,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("adapter");

        let error = adapter
            .ingest_usdt_futures_position_risk_json(
                r#"[{"symbol":"BTCUSDT","positionSide":"LONG","positionAmt":"1.000","entryPrice":"43100.00","unRealizedProfit":"1.00","liquidationPrice":"0","updateTime":1767225601500}]"#,
                "test:aster-futures-position-risk-missing-mark",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect_err("nonzero Aster position without mark price is unsafe");

        let error = expect_external(error);
        assert_eq!(error.surface, ReadOnlySurface::Position);
        assert_eq!(error.class, ExternalErrorClass::MissingField);
        assert!(error.fail_closed);
    }

    #[test]
    fn hyperliquid_private_clearinghouse_state_maps_balances_and_positions() {
        let venue_id = VenueId::new("venue:HYPERLIQUID-PERP-PRIVATE").expect("venue id");
        let account_id = AccountId::new("account:hyperliquid-unit").expect("account id");
        let mut adapter = HyperliquidPrivateAccountAdapter::new(
            venue_id.clone(),
            account_id.clone(),
            HyperliquidPrivateAccountMarket::PerpetualAccount,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("adapter");
        let clearinghouse_state = r#"{"marginSummary":{"accountValue":"1000.00","totalNtlPos":"4325.05","totalRawUsd":"1000.00","totalMarginUsed":"200.00"},"crossMarginSummary":{"accountValue":"1000.00","totalNtlPos":"4325.05","totalRawUsd":"1000.00","totalMarginUsed":"200.00"},"crossMaintenanceMarginUsed":"50.00","withdrawable":"800.00","assetPositions":[{"type":"oneWay","position":{"coin":"BTC","szi":"0.100","leverage":{"type":"cross","value":10},"entryPx":"43000.00","positionValue":"4325.05","unrealizedPnl":"25.05","returnOnEquity":"0.01","liquidationPx":"35000.00","marginUsed":"100.00","maxLeverage":50,"cumFunding":{"allTime":"0","sinceOpen":"0","sinceChange":"0"}}},{"type":"oneWay","position":{"coin":"ETH","szi":"0","entryPx":"0","positionValue":"0","unrealizedPnl":"0","liquidationPx":null}}],"time":1767225601500}"#;
        let asset_contexts = r#"[{"universe":[{"name":"BTC","szDecimals":5,"maxLeverage":50},{"name":"ETH","szDecimals":4,"maxLeverage":50}],"marginTables":[]},[{"dayNtlVlm":"1","funding":"0.0001","markPx":"43250.50","midPx":"43250.45","openInterest":"123","oraclePx":"43249.0","premium":"0.0001","prevDayPx":"43000.0"},{"dayNtlVlm":"1","funding":"0.0001","markPx":"2500.00","midPx":"2500.00","openInterest":"123","oraclePx":"2500.0","premium":"0.0001","prevDayPx":"2490.0"}]]"#;

        let balances = adapter
            .ingest_clearinghouse_state_balance_json(
                clearinghouse_state,
                "test:hyperliquid-clearinghouse-state-balance",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("Hyperliquid balance");
        assert_eq!(
            balances.balance_event.event_type,
            NormalizedEventType::BalanceSnapshotEvent
        );
        assert_eq!(
            payload_string(&balances.balance_event, "endpoint"),
            "info:clearinghouseState"
        );
        assert_eq!(balances.balances.len(), 1);
        assert_eq!(balances.balances[0].asset_id.as_str(), "asset:USDC");
        assert_eq!(balances.balances[0].free, amount("800.00"));
        assert_eq!(balances.balances[0].locked, amount("200.00"));

        let positions = adapter
            .ingest_clearinghouse_state_positions_json(
                clearinghouse_state,
                asset_contexts,
                "test:hyperliquid-clearinghouse-state-positions+meta",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("Hyperliquid positions");
        assert_eq!(
            positions.position_event.event_type,
            NormalizedEventType::PositionSnapshotEvent
        );
        assert_eq!(
            payload_string(&positions.position_event, "endpoint"),
            "info:clearinghouseState"
        );
        assert_eq!(
            payload_string(&positions.position_event, "market_context_endpoint"),
            "info:metaAndAssetCtxs"
        );
        assert_eq!(positions.positions.len(), 1);
        let position = &positions.positions[0];
        assert_eq!(position.account_id, account_id);
        assert_eq!(position.instrument_id.as_str(), "inst:HYPERLIQUID:BTC:PERP");
        assert_eq!(position.quantity.to_string(), "0.100");
        assert_eq!(position.entry_price.expect("entry").to_string(), "43000.00");
        assert_eq!(position.mark_price.to_string(), "43250.50");
        assert_eq!(position.unrealized_pnl.to_string(), "25.05");
        assert_eq!(
            position.liquidation_price.expect("liq").to_string(),
            "35000.00"
        );

        let queried = adapter
            .positions(
                &PositionQuery::new(venue_id.clone())
                    .for_account(AccountId::new("account:hyperliquid-unit").expect("account id"))
                    .for_instrument(
                        InstrumentId::new("inst:HYPERLIQUID:BTC:PERP").expect("instrument"),
                    ),
            )
            .expect("position query");
        assert_eq!(queried, positions.positions);
        let health = adapter.venue_health(&venue_id).expect("health");
        assert_eq!(health.status, VenueHealthStatus::Healthy);

        let rendered = to_canonical_json(&positions.position_event);
        assert!(!rendered.contains("43000.00"));
        assert!(!rendered.contains("private_key"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn hyperliquid_private_position_requires_public_mark_price_context() {
        let mut adapter = HyperliquidPrivateAccountAdapter::new(
            VenueId::new("venue:HYPERLIQUID-PERP-PRIVATE").expect("venue id"),
            AccountId::new("account:hyperliquid-unit").expect("account id"),
            HyperliquidPrivateAccountMarket::PerpetualAccount,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("adapter");
        let clearinghouse_state = r#"{"marginSummary":{"accountValue":"1000.00","totalNtlPos":"4325.05","totalRawUsd":"1000.00","totalMarginUsed":"200.00"},"withdrawable":"800.00","assetPositions":[{"type":"oneWay","position":{"coin":"BTC","szi":"0.100","entryPx":"43000.00","positionValue":"4325.05","unrealizedPnl":"25.05","liquidationPx":"35000.00"}}],"time":1767225601500}"#;
        let asset_contexts_without_btc = r#"[{"universe":[{"name":"ETH","szDecimals":4,"maxLeverage":50}],"marginTables":[]},[{"markPx":"2500.00"}]]"#;

        let error = adapter
            .ingest_clearinghouse_state_positions_json(
                clearinghouse_state,
                asset_contexts_without_btc,
                "test:hyperliquid-missing-mark",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect_err("Hyperliquid position without mark context is unsafe");

        let error = expect_external(error);
        assert_eq!(error.surface, ReadOnlySurface::Position);
        assert_eq!(error.class, ExternalErrorClass::MissingField);
        assert!(error.fail_closed);
    }

    #[test]
    fn unknown_health_is_explicit_and_not_successful() {
        let venue_id = VenueId::new("venue:UNKNOWN").expect("venue id");
        let error = VenueDataError::UnknownExternalState {
            venue_id: venue_id.clone(),
            surface: ReadOnlySurface::VenueHealth,
            detail: "health endpoint returned indeterminate state".to_owned(),
        };

        assert_eq!(
            error.to_string(),
            "venue `venue:UNKNOWN` read-only surface `VenueHealth` is unknown: health endpoint returned indeterminate state"
        );
    }

    #[test]
    fn hybrid_market_data_requires_rest_snapshot_before_wss_quote() {
        let mut coordinator = hybrid_coordinator();

        let error = coordinator
            .apply(HybridMarketDataInput::WssQuote {
                update: wss_quote_update(101, "43187.90", "43188.40"),
            })
            .expect_err("WSS quote without REST snapshot must fail closed");

        assert!(matches!(
            error,
            VenueDataError::UnknownExternalState {
                surface: ReadOnlySurface::MarketData,
                ..
            }
        ));
        assert_eq!(coordinator.status(), HybridMarketDataStatus::Halted);
        let health = coordinator
            .venue_health(&VenueId::new("venue:BINANCE-SPOT").expect("venue"))
            .expect("health");
        assert_eq!(health.status, VenueHealthStatus::Unhealthy);
        assert_eq!(health.reason_codes, vec!["WSS_WITHOUT_REST_SNAPSHOT"]);
    }

    #[test]
    fn hybrid_market_data_uses_rest_snapshot_then_wss_updates() {
        let mut coordinator = hybrid_coordinator();

        let snapshot_update = coordinator
            .apply(HybridMarketDataInput::RestSnapshot {
                quote: rest_snapshot_quote(100, "43187.40", "43188.10"),
            })
            .expect("REST snapshot");
        assert_eq!(snapshot_update.transport, MarketDataTransport::RestSnapshot);
        assert_eq!(
            snapshot_update.status,
            HybridMarketDataStatus::SnapshotReady
        );
        assert!(!snapshot_update.fail_closed);
        assert_eq!(coordinator.last_wss_sequence(), Some(100));

        let connected = coordinator
            .apply(HybridMarketDataInput::WssConnected {
                occurred_at: timestamp("2026-01-01T00:00:02Z"),
                ingested_at: timestamp("2026-01-01T00:00:02Z"),
            })
            .expect("WSS connected");
        assert_eq!(connected.status, HybridMarketDataStatus::Streaming);
        assert!(!connected.fail_closed);

        let stream_update = coordinator
            .apply(HybridMarketDataInput::WssQuote {
                update: wss_quote_update(101, "43187.90", "43188.40"),
            })
            .expect("WSS update");
        assert_eq!(
            stream_update.transport,
            MarketDataTransport::WebSocketStream
        );
        assert_eq!(stream_update.status, HybridMarketDataStatus::Streaming);
        assert!(!stream_update.fail_closed);

        let quote = coordinator
            .latest_quote(&MarketDataQuery::new(
                VenueId::new("venue:BINANCE-SPOT").expect("venue"),
                InstrumentId::new("inst:BINANCE:BTCUSDT:SPOT").expect("instrument"),
            ))
            .expect("quote read")
            .expect("latest quote");
        assert_eq!(quote.best_bid.expect("bid").to_string(), "43187.90");
        assert_eq!(quote.best_ask.expect("ask").to_string(), "43188.40");
        assert_eq!(quote.source_sequence.as_deref(), Some("101"));
        assert_eq!(quote.freshness.status, FreshnessStatus::Fresh);
    }

    #[test]
    fn hybrid_market_data_detects_wss_sequence_gap_and_fails_closed() {
        let mut coordinator = hybrid_coordinator();
        coordinator
            .apply(HybridMarketDataInput::RestSnapshot {
                quote: rest_snapshot_quote(100, "43187.40", "43188.10"),
            })
            .expect("REST snapshot");

        let error = coordinator
            .apply(HybridMarketDataInput::WssQuote {
                update: wss_quote_update(105, "43188.00", "43188.50"),
            })
            .expect_err("sequence gap must fail closed");

        assert!(error.to_string().contains("expected `101` observed `105`"));
        assert_eq!(coordinator.status(), HybridMarketDataStatus::Halted);
        let health = coordinator
            .venue_health(&VenueId::new("venue:BINANCE-SPOT").expect("venue"))
            .expect("health");
        assert_eq!(health.status, VenueHealthStatus::Unhealthy);
        assert_eq!(health.connection, VenueConnectionStatus::Unknown);
        assert_eq!(health.reason_codes, vec!["WSS_SEQUENCE_GAP"]);
        let quote = coordinator
            .latest_quote_snapshot()
            .expect("REST snapshot remains last trusted quote");
        assert_eq!(quote.source_sequence.as_deref(), Some("100"));
    }

    #[test]
    fn hybrid_market_data_disconnect_is_observable_and_fail_closed() {
        let mut coordinator = hybrid_coordinator();
        coordinator
            .apply(HybridMarketDataInput::RestSnapshot {
                quote: rest_snapshot_quote(100, "43187.40", "43188.10"),
            })
            .expect("REST snapshot");
        coordinator
            .apply(HybridMarketDataInput::WssConnected {
                occurred_at: timestamp("2026-01-01T00:00:02Z"),
                ingested_at: timestamp("2026-01-01T00:00:02Z"),
            })
            .expect("WSS connected");

        let update = coordinator
            .apply(HybridMarketDataInput::WssDisconnected {
                reason: "connection closed by peer".to_owned(),
                occurred_at: timestamp("2026-01-01T00:00:03Z"),
                ingested_at: timestamp("2026-01-01T00:00:03Z"),
            })
            .expect("disconnect update");

        assert_eq!(update.status, HybridMarketDataStatus::Reconnecting);
        assert!(update.fail_closed);
        assert_eq!(update.health.status, VenueHealthStatus::Unhealthy);
        assert_eq!(
            update.health.connection,
            VenueConnectionStatus::Disconnected
        );
        assert!(update.reason_codes.contains(&"WSS_DISCONNECTED".to_owned()));
    }

    #[test]
    fn binance_public_wss_book_ticker_builds_public_stream_urls() {
        let spot = binance_wss_config(BinancePublicMarket::Spot);
        assert_eq!(spot.stream_name(), "btcusdt@bookTicker");
        assert_eq!(
            spot.stream_url(),
            "wss://data-stream.binance.vision/ws/btcusdt@bookTicker"
        );

        let usdm = binance_wss_config(BinancePublicMarket::UsdmPerpetual);
        assert_eq!(
            usdm.stream_url(),
            "wss://fstream.binance.com/public/ws/btcusdt@bookTicker"
        );
    }

    #[test]
    fn binance_public_wss_client_requires_rest_snapshot_before_wss_text() {
        let mut client = binance_wss_client(BinancePublicMarket::Spot);

        let error = client
            .apply_wss_text_message(
                r#"{"u":400900217,"s":"BTCUSDT","b":"43187.90","B":"2.00000000","a":"43188.40","A":"2.50000000"}"#,
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect_err("WSS text before REST snapshot must fail closed");

        assert!(matches!(
            error,
            VenueDataError::UnknownExternalState {
                surface: ReadOnlySurface::MarketData,
                ..
            }
        ));
        assert_eq!(
            client.coordinator().status(),
            HybridMarketDataStatus::Halted
        );
    }

    #[test]
    fn binance_public_wss_client_applies_rest_snapshot_then_spot_update() {
        let mut client = binance_wss_client(BinancePublicMarket::Spot);

        let snapshot = client
            .apply_rest_snapshot(rest_snapshot_quote(100, "43187.40", "43188.10"))
            .expect("REST snapshot");
        assert_eq!(snapshot.transport, MarketDataTransport::RestSnapshot);
        assert_eq!(snapshot.status, HybridMarketDataStatus::SnapshotReady);
        assert_eq!(client.coordinator().last_wss_sequence(), Some(1));

        let update = client
            .apply_wss_text_message(
                r#"{"u":400900217,"s":"BTCUSDT","b":"43187.90","B":"2.00000000","a":"43188.40","A":"2.50000000"}"#,
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("WSS bookTicker");
        assert_eq!(update.transport, MarketDataTransport::WebSocketStream);
        assert_eq!(update.status, HybridMarketDataStatus::Streaming);
        assert!(!update.fail_closed);
        assert_eq!(client.last_exchange_update_id(), Some(400900217));

        let quote = client
            .coordinator()
            .latest_quote(&MarketDataQuery::new(
                VenueId::new("venue:BINANCE-SPOT").expect("venue"),
                InstrumentId::new("inst:BINANCE:BTCUSDT:SPOT").expect("instrument"),
            ))
            .expect("quote read")
            .expect("latest quote");
        assert_eq!(quote.best_bid.expect("bid").to_string(), "43187.90");
        assert_eq!(quote.best_ask.expect("ask").to_string(), "43188.40");
        assert_eq!(quote.source_sequence.as_deref(), Some("2"));
        assert_eq!(
            quote.source_event_id.as_deref(),
            Some("event:venue-data:binance-public:wss-book-ticker:spot:BTCUSDT:2:u400900217")
        );
    }

    #[test]
    fn binance_public_wss_client_accepts_usdm_event_time() {
        let mut client = binance_wss_client(BinancePublicMarket::UsdmPerpetual);

        client
            .apply_rest_snapshot(perp_rest_snapshot_quote("43250.00", "43251.00"))
            .expect("REST snapshot");
        let update = client
            .apply_wss_text_message(
                r#"{"e":"bookTicker","u":400900300,"E":1767225602123,"T":1767225602120,"s":"BTCUSDT","b":"43250.10","B":"1.00000000","a":"43251.20","A":"1.50000000"}"#,
                timestamp("2026-01-01T00:00:03Z"),
            )
            .expect("USDM WSS bookTicker");

        let quote = update.quote.expect("quote");
        assert_eq!(
            quote.freshness.observed_at.to_string(),
            "2026-01-01T00:00:02.12Z"
        );
        assert_eq!(quote.freshness.age_ms(), 880);
        assert_eq!(quote.best_bid.expect("bid").to_string(), "43250.10");
    }

    #[test]
    fn binance_public_wss_client_tolerates_small_future_exchange_time() {
        let mut client = binance_wss_client(BinancePublicMarket::UsdmPerpetual);

        client
            .apply_rest_snapshot(perp_rest_snapshot_quote("43250.00", "43251.00"))
            .expect("REST snapshot");
        let update = client
            .apply_wss_text_message(
                r#"{"e":"bookTicker","u":400900301,"E":1767225602123,"T":1767225602120,"s":"BTCUSDT","b":"43250.10","B":"1.00000000","a":"43251.20","A":"1.50000000"}"#,
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("USDM WSS bookTicker with exchange time slightly ahead of local ingest");

        let quote = update.quote.expect("quote");
        assert_eq!(
            quote.freshness.observed_at.to_string(),
            "2026-01-01T00:00:02.12Z"
        );
        assert_eq!(
            quote.freshness.ingested_at.to_string(),
            "2026-01-01T00:00:02.12Z"
        );
        assert_eq!(quote.freshness.age_ms(), 0);
        assert_eq!(quote.best_bid.expect("bid").to_string(), "43250.10");
    }

    #[test]
    fn binance_public_wss_client_detects_duplicate_update_and_rest_rebuilds() {
        let mut client = binance_wss_client(BinancePublicMarket::Spot);
        client
            .apply_rest_snapshot(rest_snapshot_quote(100, "43187.40", "43188.10"))
            .expect("REST snapshot");
        client
            .apply_wss_text_message(
                r#"{"u":400900217,"s":"BTCUSDT","b":"43187.90","B":"2.00000000","a":"43188.40","A":"2.50000000"}"#,
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("first WSS bookTicker");

        let gap = client
            .apply_wss_text_message(
                r#"{"u":400900217,"s":"BTCUSDT","b":"43188.00","B":"2.00000000","a":"43188.50","A":"2.50000000"}"#,
                timestamp("2026-01-01T00:00:03Z"),
            )
            .expect("duplicate update should be represented as fail-closed gap");

        assert_eq!(gap.status, HybridMarketDataStatus::Halted);
        assert!(gap.fail_closed);
        assert_eq!(gap.health.status, VenueHealthStatus::Unhealthy);
        assert!(gap.reason_codes.contains(&"WSS_SEQUENCE_GAP".to_owned()));

        let rebuilt = client
            .apply_rest_snapshot(rest_snapshot_quote(100, "43188.20", "43188.70"))
            .expect("REST rebuild snapshot");
        assert_eq!(rebuilt.status, HybridMarketDataStatus::SnapshotReady);
        assert!(!rebuilt.fail_closed);
        assert_eq!(client.last_exchange_update_id(), None);
        assert_eq!(client.coordinator().last_wss_sequence(), Some(3));
    }

    #[test]
    fn binance_public_ticker_maps_raw_response_to_normalized_events() {
        let mut adapter = binance_adapter();
        let batch = adapter
            .ingest_ticker_24h_json(
                &read_fixture("raw/binance_ticker_24hr.redacted.json"),
                "fixtures/replay/venue_data_smoke/raw/binance_ticker_24hr.redacted.json",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("public ticker fixture should normalize");

        assert_eq!(
            batch.raw_event.event_type,
            NormalizedEventType::RawMarketDataEvent
        );
        assert_eq!(
            batch.normalized_event.event_type,
            NormalizedEventType::NormalizedMarketDataEvent
        );
        assert_eq!(batch.quote.best_bid.expect("bid").to_string(), "43187.40");
        assert_eq!(batch.quote.best_ask.expect("ask").to_string(), "43188.10");
        assert_eq!(batch.quote.freshness.status, FreshnessStatus::Fresh);
        assert_eq!(batch.quote.freshness.age_ms(), 2_000);
        assert_eq!(
            payload_string(&batch.normalized_event, "raw_event_ref"),
            batch.raw_event.event_id.as_str()
        );
        assert_eq!(
            canonical_normalized_event_hash(&batch.normalized_event),
            batch.normalized_event.checksum.as_str()
        );

        let quote = adapter
            .latest_quote(&MarketDataQuery::new(
                VenueId::new("venue:BINANCE_PUBLIC").expect("venue"),
                InstrumentId::new("inst:BTC-USDT").expect("instrument"),
            ))
            .expect("quote read")
            .expect("latest quote");
        assert_eq!(quote.source_sequence.as_deref(), Some("900100"));

        let balances_error = adapter
            .balances(&BalanceQuery::new(
                VenueId::new("venue:BINANCE_PUBLIC").expect("venue"),
            ))
            .expect_err("public adapter must not fake account balances");
        assert!(matches!(
            balances_error,
            VenueDataError::DataUnavailable {
                surface: ReadOnlySurface::Balance,
                ..
            }
        ));
    }

    #[test]
    fn binance_public_book_ticker_maps_spot_and_perp_top_of_book() {
        let mut spot = binance_book_adapter(
            "venue:BINANCE-SPOT",
            "inst:BINANCE:BTCUSDT:SPOT",
            BinancePublicMarket::Spot,
        );
        let spot_batch = spot
            .ingest_book_ticker_json(
                r#"{"symbol":"BTCUSDT","bidPrice":"43187.40","bidQty":"1.20000000","askPrice":"43188.10","askQty":"0.80000000"}"#,
                "https://api.binance.com/api/v3/ticker/bookTicker?symbol=BTCUSDT",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("spot book ticker");

        assert_eq!(
            spot_batch.quote.best_bid.expect("bid").to_string(),
            "43187.40"
        );
        assert_eq!(
            spot_batch.quote.best_ask.expect("ask").to_string(),
            "43188.10"
        );
        assert_eq!(
            payload_string(&spot_batch.normalized_event, "basis_role"),
            "Spot"
        );
        assert_eq!(
            canonical_normalized_event_hash(&spot_batch.normalized_event),
            spot_batch.normalized_event.checksum.as_str()
        );

        let mut perp = binance_book_adapter(
            "venue:BINANCE-USDM",
            "inst:BINANCE:BTCUSDT:USDM-PERP",
            BinancePublicMarket::UsdmPerpetual,
        );
        let perp_batch = perp
            .ingest_book_ticker_json(
                r#"{"symbol":"BTCUSDT","bidPrice":"43250.00","bidQty":"2.00000000","askPrice":"43251.00","askQty":"1.50000000","time":1767225601000}"#,
                "https://fapi.binance.com/fapi/v1/ticker/bookTicker?symbol=BTCUSDT",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("perp book ticker");

        assert_eq!(
            payload_string(&perp_batch.normalized_event, "basis_role"),
            "Perp"
        );
        assert_eq!(
            perp_batch.quote.source_sequence.as_deref(),
            Some("1767225601000")
        );
        let instruments = perp
            .instruments(&InstrumentInfoQuery::new(
                VenueId::new("venue:BINANCE-USDM").expect("venue"),
            ))
            .expect("instrument info");
        assert_eq!(instruments[0].kind, InstrumentKind::PerpetualSwap);
        assert_eq!(
            instruments[0].margin_asset_id.as_ref().map(AssetId::as_str),
            Some("asset:USDT")
        );
    }

    #[test]
    fn binance_public_book_ticker_keeps_non_ascii_symbol_payload_with_encoded_ids() {
        let symbol = "中文USDT";
        let component = symbol_identifier_component(symbol);
        let asset_usdt = AssetId::new("asset:USDT").expect("asset id");
        let instrument = BinancePublicInstrument::new(
            symbol,
            InstrumentId::new(format!("inst:BINANCE:{component}:SPOT")).expect("instrument id"),
            AssetId::new(format!("asset:{}", symbol_identifier_component("中文")))
                .expect("asset id"),
            asset_usdt.clone(),
            asset_usdt,
        )
        .expect("instrument");
        let mut adapter = BinancePublicBookTickerAdapter::new(
            VenueId::new("venue:BINANCE-SPOT").expect("venue id"),
            instrument,
            BinancePublicMarket::Spot,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("adapter");

        let batch = adapter
            .ingest_book_ticker_json(
                r#"{"symbol":"中文USDT","bidPrice":"99.90","bidQty":"1.20000000","askPrice":"100.00","askQty":"0.80000000"}"#,
                "test:binance-non-ascii-book-ticker",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("non-ASCII book ticker");

        assert_eq!(
            payload_string(&batch.normalized_event, "venue_symbol"),
            symbol
        );
        assert!(batch.raw_event.event_id.as_str().contains(&component));
        assert!(batch
            .normalized_event
            .correlation_id
            .as_str()
            .contains(&component));
    }

    #[test]
    fn binance_usdm_premium_index_maps_mark_index_and_funding() {
        let adapter = binance_premium_index_adapter();
        let batch = adapter
            .ingest_premium_index_json(
                r#"{"symbol":"BTCUSDT","markPrice":"43240.12345678","indexPrice":"43190.00000000","estimatedSettlePrice":"43189.00","lastFundingRate":"0.00010000","interestRate":"0.00010000","nextFundingTime":1767254400000,"time":1767225601000}"#,
                "https://fapi.binance.com/fapi/v1/premiumIndex?symbol=BTCUSDT",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("premium index");

        assert_eq!(
            batch.raw_event.event_type,
            NormalizedEventType::RawMarketDataEvent
        );
        assert_eq!(
            batch.normalized_event.event_type,
            NormalizedEventType::NormalizedMarketDataEvent
        );
        assert_eq!(
            payload_string(&batch.normalized_event, "kind"),
            "PerpPremiumIndex"
        );
        assert_eq!(
            payload_string(&batch.normalized_event, "last_funding_rate"),
            "0.00010000"
        );
        assert_eq!(
            canonical_normalized_event_hash(&batch.normalized_event),
            batch.normalized_event.checksum.as_str()
        );
    }

    #[test]
    fn bybit_public_ticker_maps_spot_and_linear_top_of_book() {
        let mut spot = bybit_ticker_adapter(
            "venue:BYBIT-SPOT",
            "inst:BYBIT:BTCUSDT:SPOT",
            BybitPublicMarket::Spot,
        );
        let spot_batch = spot
            .ingest_ticker_json(
                r#"{"retCode":0,"retMsg":"OK","result":{"category":"spot","list":[{"symbol":"ETHUSDT","bid1Price":"50.00","bid1Size":"3.0","ask1Price":"50.10","ask1Size":"4.0"},{"symbol":"BTCUSDT","bid1Price":"43187.40","bid1Size":"1.20000000","ask1Price":"43188.10","ask1Size":"0.80000000"}]},"retExtInfo":{},"time":1767225601000}"#,
                "https://api.bybit.com/v5/market/tickers?category=spot",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("spot ticker");

        assert_eq!(
            spot_batch.quote.best_bid.expect("bid").to_string(),
            "43187.40"
        );
        assert_eq!(
            payload_string(&spot_batch.normalized_event, "basis_role"),
            "Spot"
        );
        assert_eq!(
            payload_string(&spot_batch.normalized_event, "market"),
            "Spot"
        );
        assert_eq!(
            spot_batch.quote.source_sequence.as_deref(),
            Some("1767225601000")
        );
        assert_eq!(
            canonical_normalized_event_hash(&spot_batch.normalized_event),
            spot_batch.normalized_event.checksum.as_str()
        );

        let mut linear = bybit_ticker_adapter(
            "venue:BYBIT-LINEAR",
            "inst:BYBIT:BTCUSDT:LINEAR-PERP",
            BybitPublicMarket::LinearPerpetual,
        );
        let linear_batch = linear
            .ingest_ticker_json(
                r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[{"symbol":"BTCUSDT","bid1Price":"43250.00","bid1Size":"2.00000000","ask1Price":"43251.00","ask1Size":"1.50000000","markPrice":"43240.12345678","indexPrice":"43190.00000000","fundingRate":"0.00010000","nextFundingTime":"1767254400000"}]},"retExtInfo":{},"time":1767225601000}"#,
                "https://api.bybit.com/v5/market/tickers?category=linear",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("linear ticker");

        assert_eq!(
            payload_string(&linear_batch.normalized_event, "basis_role"),
            "Perp"
        );
        assert_eq!(
            payload_string(&linear_batch.normalized_event, "market"),
            "LinearPerp"
        );
        let instruments = linear
            .instruments(&InstrumentInfoQuery::new(
                VenueId::new("venue:BYBIT-LINEAR").expect("venue"),
            ))
            .expect("instrument info");
        assert_eq!(instruments[0].kind, InstrumentKind::PerpetualSwap);
        assert_eq!(
            instruments[0].margin_asset_id.as_ref().map(AssetId::as_str),
            Some("asset:USDT")
        );
    }

    #[test]
    fn bybit_linear_ticker_maps_mark_index_and_funding() {
        let adapter = bybit_premium_index_adapter();
        let batch = adapter
            .ingest_premium_index_json(
                r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[{"symbol":"BTCUSDT","bid1Price":"43250.00","bid1Size":"2.00000000","ask1Price":"43251.00","ask1Size":"1.50000000","markPrice":"43240.12345678","indexPrice":"43190.00000000","fundingRate":"0.00010000","nextFundingTime":"1767254400000"}]},"retExtInfo":{},"time":1767225601000}"#,
                "https://api.bybit.com/v5/market/tickers?category=linear",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("premium index");

        assert_eq!(
            batch.raw_event.event_type,
            NormalizedEventType::RawMarketDataEvent
        );
        assert_eq!(
            batch.normalized_event.event_type,
            NormalizedEventType::NormalizedMarketDataEvent
        );
        assert_eq!(
            payload_string(&batch.normalized_event, "kind"),
            "PerpPremiumIndex"
        );
        assert_eq!(
            payload_string(&batch.normalized_event, "last_funding_rate"),
            "0.00010000"
        );
        assert_eq!(
            canonical_normalized_event_hash(&batch.normalized_event),
            batch.normalized_event.checksum.as_str()
        );
    }

    #[test]
    fn bybit_public_ticker_rejects_failed_ret_code() {
        let mut adapter = bybit_ticker_adapter(
            "venue:BYBIT-SPOT",
            "inst:BYBIT:BTCUSDT:SPOT",
            BybitPublicMarket::Spot,
        );
        let error = adapter
            .ingest_ticker_json(
                r#"{"retCode":10001,"retMsg":"symbol invalid","result":{"category":"spot","list":[]},"retExtInfo":{},"time":1767225601000}"#,
                "https://api.bybit.com/v5/market/tickers?category=spot",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect_err("non-zero retCode must fail closed");

        assert!(matches!(
            error,
            VenueDataError::External(ClassifiedExternalError {
                class: ExternalErrorClass::UnknownExternalState,
                ..
            })
        ));
    }

    #[test]
    fn binance_public_ticker_marks_stale_data_for_risk() {
        let mut adapter = binance_adapter();
        let batch = adapter
            .ingest_ticker_24h_json(
                &read_fixture("raw/binance_ticker_24hr_stale.redacted.json"),
                "fixtures/replay/venue_data_smoke/raw/binance_ticker_24hr_stale.redacted.json",
                timestamp("2026-01-01T00:00:07Z"),
            )
            .expect("stale ticker still becomes an explicit stale event");

        assert_eq!(batch.quote.freshness.status, FreshnessStatus::Stale);
        assert_eq!(batch.quote.freshness.age_ms(), 7_000);
        assert_eq!(
            payload_string(&batch.normalized_event, "risk_reason_code"),
            "DATA_STALE"
        );
        let health = adapter
            .venue_health(&VenueId::new("venue:BINANCE_PUBLIC").expect("venue"))
            .expect("health read");
        assert_eq!(health.status, VenueHealthStatus::Degraded);
        assert_eq!(health.reason_codes, vec!["DATA_STALE"]);
    }

    #[test]
    fn binance_public_ticker_classifies_missing_duplicate_and_out_of_order() {
        let mut adapter = binance_adapter();
        adapter
            .ingest_ticker_24h_json(
                &read_fixture("raw/binance_ticker_24hr.redacted.json"),
                "fixtures/replay/venue_data_smoke/raw/binance_ticker_24hr.redacted.json",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("first event");

        let duplicate = adapter
            .ingest_ticker_24h_json(
                &read_fixture("raw/binance_ticker_24hr.redacted.json"),
                "fixtures/replay/venue_data_smoke/raw/binance_ticker_24hr.redacted.json",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect_err("duplicate source sequence must not emit a second event");
        let duplicate = expect_external(duplicate);
        assert_eq!(duplicate.class, ExternalErrorClass::DuplicateMessage);
        assert_eq!(duplicate.reason_code, "DUPLICATE_MESSAGE");
        assert!(!duplicate.fail_closed);

        let out_of_order = adapter
            .ingest_ticker_24h_json(
                &read_fixture("raw/binance_ticker_24hr_out_of_order.redacted.json"),
                "fixtures/replay/venue_data_smoke/raw/binance_ticker_24hr_out_of_order.redacted.json",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect_err("lower source sequence must fail closed");
        let out_of_order = expect_external(out_of_order);
        assert_eq!(out_of_order.class, ExternalErrorClass::OutOfOrderMessage);
        assert_eq!(out_of_order.reason_code, "UNKNOWN_STATE");
        assert!(out_of_order.fail_closed);

        let missing = binance_adapter()
            .ingest_ticker_24h_json(
                &read_fixture("raw/binance_ticker_24hr_missing_bid.redacted.json"),
                "fixtures/replay/venue_data_smoke/raw/binance_ticker_24hr_missing_bid.redacted.json",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect_err("missing bidPrice must be classified");
        let missing = expect_external(missing);
        assert_eq!(missing.class, ExternalErrorClass::MissingField);
        assert_eq!(missing.reason_code, "REQUIRES_MORE_DATA");
        assert!(missing.detail.contains("bidPrice"));
    }

    #[test]
    fn binance_public_disconnect_produces_health_event() {
        let mut adapter = binance_adapter();
        let event = adapter
            .record_disconnect(
                "fixture websocket closed before next public ticker update",
                timestamp("2026-01-01T00:00:10Z"),
                timestamp("2026-01-01T00:00:11Z"),
            )
            .expect("disconnect should produce health event");

        assert_eq!(event.event_type, NormalizedEventType::VenueHealthEvent);
        assert_eq!(payload_string(&event, "reason_code"), "VENUE_UNHEALTHY");
        assert_eq!(payload_string(&event, "venue_error_class"), "Disconnected");
        assert_eq!(
            canonical_normalized_event_hash(&event),
            event.checksum.as_str()
        );

        let health = adapter
            .venue_health(&VenueId::new("venue:BINANCE_PUBLIC").expect("venue"))
            .expect("health read");
        assert_eq!(health.status, VenueHealthStatus::Unhealthy);
        assert_eq!(health.connection, VenueConnectionStatus::Disconnected);
        assert_eq!(health.reason_codes, vec!["VENUE_UNHEALTHY"]);
    }

    #[test]
    fn binance_public_reconnecting_produces_health_event() {
        let mut adapter = binance_adapter();
        let event = adapter
            .record_reconnecting(
                "fixture websocket reconnect scheduled",
                timestamp("2026-01-01T00:00:12Z"),
                timestamp("2026-01-01T00:00:13Z"),
            )
            .expect("reconnecting should produce health event");

        assert_eq!(event.event_type, NormalizedEventType::VenueHealthEvent);
        assert_eq!(payload_string(&event, "reason_code"), "VENUE_UNHEALTHY");
        assert_eq!(payload_string(&event, "venue_error_class"), "Reconnecting");

        let health = adapter
            .venue_health(&VenueId::new("venue:BINANCE_PUBLIC").expect("venue"))
            .expect("health read");
        assert_eq!(health.status, VenueHealthStatus::Degraded);
        assert_eq!(health.connection, VenueConnectionStatus::Reconnecting);
        assert_eq!(health.reason_codes, vec!["VENUE_UNHEALTHY"]);
    }

    #[test]
    fn binance_public_rate_limit_records_health_event() {
        let mut adapter = binance_adapter();
        let event = adapter
            .record_rate_limit(
                fixture_rate_limit(),
                "fixture HTTP 429 public ticker rate limit header observed",
                timestamp("2026-01-01T00:00:14Z"),
                timestamp("2026-01-01T00:00:15Z"),
            )
            .expect("rate limit should produce health event");

        assert_eq!(event.event_type, NormalizedEventType::VenueHealthEvent);
        assert_eq!(payload_string(&event, "reason_code"), "RATE_LIMITED");
        assert_eq!(payload_string(&event, "venue_error_class"), "RateLimited");

        let health = adapter
            .venue_health(&VenueId::new("venue:BINANCE_PUBLIC").expect("venue"))
            .expect("health read");
        assert_eq!(health.status, VenueHealthStatus::Degraded);
        assert_eq!(health.connection, VenueConnectionStatus::Connected);
        assert_eq!(health.reason_codes, vec!["RATE_LIMITED"]);
        assert_eq!(
            health.rate_limit.expect("rate limit snapshot").remaining,
            Some(0)
        );
    }

    #[test]
    fn binance_public_http_status_classification_is_explicit() {
        let adapter = binance_adapter();
        let rate_limit =
            adapter.classify_http_status(ReadOnlySurface::MarketData, 429, "fixture 429");
        assert_eq!(rate_limit.class, ExternalErrorClass::RateLimited);
        assert_eq!(rate_limit.reason_code, "RATE_LIMITED");
        assert!(rate_limit.retryable);
        assert!(rate_limit.fail_closed);

        let unknown =
            adapter.classify_http_status(ReadOnlySurface::MarketData, 451, "fixture unknown");
        assert_eq!(unknown.class, ExternalErrorClass::UnknownExternalState);
        assert_eq!(unknown.reason_code, "UNKNOWN_STATE");
        assert!(unknown.fail_closed);
    }

    #[test]
    fn binance_private_spot_account_maps_balances_without_credentials() {
        let mut adapter = binance_private_spot_adapter();
        let batch = adapter
            .ingest_spot_account_json(
                r#"{"makerCommission":15,"takerCommission":15,"canTrade":true,"canWithdraw":false,"canDeposit":true,"updateTime":1767225600000,"accountType":"SPOT","balances":[{"asset":"BTC","free":"0.50000000","locked":"0.10000000"},{"asset":"USDT","free":"1000.00000000","locked":"25.00000000"}],"permissions":["SPOT"]}"#,
                "redacted:binance-spot-account",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("spot account snapshot");

        assert_eq!(
            batch.balance_event.event_type,
            NormalizedEventType::BalanceSnapshotEvent
        );
        assert_eq!(
            payload_string(&batch.balance_event, "endpoint"),
            "/api/v3/account"
        );
        assert_eq!(
            payload_string(&batch.balance_event, "risk_reason_code"),
            "CHECK_PASSED"
        );
        assert_eq!(batch.balances.len(), 2);

        let usdt = adapter
            .balances(
                &BalanceQuery::new(VenueId::new("venue:BINANCE-SPOT-PRIVATE").expect("venue"))
                    .for_account(AccountId::new("account:binance-read-only").expect("account"))
                    .for_asset(AssetId::new("asset:USDT").expect("asset")),
            )
            .expect("balances");
        assert_eq!(usdt.len(), 1);
        assert_eq!(usdt[0].free.to_string(), "1000.00000000");
        assert_eq!(usdt[0].locked.to_string(), "25.00000000");
        assert_eq!(
            usdt[0].source_event_id.as_deref(),
            Some(batch.balance_event.event_id.as_str())
        );

        let rendered = to_canonical_json(&batch.balance_event);
        assert!(!rendered.contains("1000.00000000"));
        assert!(!rendered.contains("api_key"));
        assert!(!rendered.contains("secret"));

        let market_data_error = adapter
            .latest_quote(&MarketDataQuery::new(
                VenueId::new("venue:BINANCE-SPOT-PRIVATE").expect("venue"),
                InstrumentId::new("inst:BINANCE:BTCUSDT:SPOT").expect("instrument"),
            ))
            .expect_err("private account adapter must not expose market data");
        assert!(matches!(
            market_data_error,
            VenueDataError::DataUnavailable {
                surface: ReadOnlySurface::MarketData,
                ..
            }
        ));

        let health = adapter
            .venue_health(&VenueId::new("venue:BINANCE-SPOT-PRIVATE").expect("venue"))
            .expect("health");
        assert_eq!(health.status, VenueHealthStatus::Healthy);
        assert_eq!(health.connection, VenueConnectionStatus::Connected);
    }

    #[test]
    fn binance_private_usdm_maps_balances_and_position_risk() {
        let mut adapter = binance_private_usdm_adapter();
        let balances = adapter
            .ingest_usdm_account_json(
                r#"{"totalWalletBalance":"126.72469206","assets":[{"asset":"USDT","walletBalance":"103.12345678","unrealizedProfit":"0.00000000","marginBalance":"103.12345678","maintMargin":"0.00000000","initialMargin":"5.00000000","positionInitialMargin":"4.00000000","openOrderInitialMargin":"1.00000000","crossWalletBalance":"103.12345678","crossUnPnl":"0.00000000","availableBalance":"98.12345678","maxWithdrawAmount":"98.12345678","updateTime":1767225601000}],"positions":[{"symbol":"BTCUSDT","positionSide":"BOTH","positionAmt":"1.000","unrealizedProfit":"0.00000000","initialMargin":"0","maintMargin":"0","updateTime":1767225601000}]}"#,
                "redacted:binance-usdm-account",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("usdm account snapshot");
        assert_eq!(balances.balances.len(), 1);
        assert_eq!(balances.balances[0].free.to_string(), "98.12345678");
        assert_eq!(balances.balances[0].locked.to_string(), "5.00000000");
        assert_eq!(balances.balances[0].reserved.to_string(), "1.00000000");

        let positions = adapter
            .ingest_usdm_position_risk_json(
                r#"[{"symbol":"BTCUSDT","positionSide":"BOTH","positionAmt":"1.000","entryPrice":"43100.00","breakEvenPrice":"43100.00","markPrice":"43250.50","unRealizedProfit":"150.50","liquidationPrice":"35000.00","updateTime":1767225601500},{"symbol":"ETHUSDT","positionSide":"BOTH","positionAmt":"0","entryPrice":"0.0","markPrice":"2500.00","unRealizedProfit":"0","liquidationPrice":"0","updateTime":1767225601500}]"#,
                "redacted:binance-usdm-position-risk",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("usdm position risk snapshot");

        assert_eq!(
            positions.position_event.event_type,
            NormalizedEventType::PositionSnapshotEvent
        );
        assert_eq!(
            payload_string(&positions.position_event, "endpoint"),
            "/fapi/v3/positionRisk"
        );
        assert_eq!(positions.positions.len(), 1);
        let position = &positions.positions[0];
        assert_eq!(
            position.instrument_id.as_str(),
            "inst:BINANCE:BTCUSDT:USDM-PERP"
        );
        assert_eq!(position.quantity.to_string(), "1.000");
        assert_eq!(position.mark_price.to_string(), "43250.50");
        assert_eq!(position.unrealized_pnl.to_string(), "150.50");
        assert_eq!(
            position.liquidation_price.expect("liq").to_string(),
            "35000.00"
        );

        let queried = adapter
            .positions(
                &PositionQuery::new(VenueId::new("venue:BINANCE-USDM-PRIVATE").expect("venue"))
                    .for_account(AccountId::new("account:binance-usdm-read-only").expect("account"))
                    .for_instrument(
                        InstrumentId::new("inst:BINANCE:BTCUSDT:USDM-PERP").expect("instrument"),
                    ),
            )
            .expect("positions");
        assert_eq!(queried, positions.positions);
    }

    #[test]
    fn binance_private_portfolio_margin_maps_account_and_um_position_risk() {
        let mut adapter = binance_private_portfolio_margin_adapter();
        let balances = adapter
            .ingest_portfolio_margin_account_json(
                r#"{"uniMMR":"5167.92171923","accountEquity":"122607.35137903","actualEquity":"73.47428058","accountInitialMargin":"23.72469206","accountMaintMargin":"23.72469206","accountStatus":"NORMAL","virtualMaxWithdrawAmount":"1627523.32459208","totalAvailableBalance":"","totalMarginOpenLoss":"","updateTime":1767225601000}"#,
                "redacted:binance-papi-account",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("portfolio margin account snapshot");
        assert_eq!(
            payload_string(&balances.balance_event, "endpoint"),
            "/papi/v1/account"
        );
        assert_eq!(
            payload_string(&balances.balance_event, "market"),
            "PortfolioMargin"
        );
        assert_eq!(balances.balances.len(), 1);
        assert_eq!(balances.balances[0].asset_id.as_str(), "asset:USDT");
        assert_eq!(balances.balances[0].free.to_string(), "1627523.32459208");
        assert_eq!(balances.balances[0].locked.to_string(), "23.72469206");

        let positions = adapter
            .ingest_usdm_position_risk_json(
                r#"[{"symbol":"BTCUSDT","positionSide":"LONG","positionAmt":"0.001","entryPrice":"43100.00","markPrice":"43250.50","unRealizedProfit":"0.15","liquidationPrice":"35000.00","updateTime":1767225601500}]"#,
                "redacted:binance-papi-um-position-risk",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("portfolio margin UM position risk snapshot");
        assert_eq!(
            payload_string(&positions.position_event, "endpoint"),
            "/papi/v1/um/positionRisk"
        );
        assert_eq!(positions.positions.len(), 1);
        assert_eq!(
            positions.positions[0].instrument_id.as_str(),
            "inst:BINANCE:BTCUSDT:USDM-PERP"
        );
    }

    #[test]
    fn binance_private_position_requires_mark_price_for_nonzero_position() {
        let mut adapter = binance_private_usdm_adapter();
        let error = adapter
            .ingest_usdm_position_risk_json(
                r#"[{"symbol":"BTCUSDT","positionSide":"BOTH","positionAmt":"1.000","entryPrice":"43100.00","unRealizedProfit":"150.50","updateTime":1767225601500}]"#,
                "redacted:binance-usdm-position-risk",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect_err("nonzero position without mark price is unsafe");

        let error = expect_external(error);
        assert_eq!(error.surface, ReadOnlySurface::Position);
        assert_eq!(error.class, ExternalErrorClass::MissingField);
        assert!(error.fail_closed);
    }

    #[test]
    fn bybit_private_unified_wallet_maps_balances_without_credentials() {
        let mut adapter = bybit_private_unified_adapter();
        let batch = adapter
            .ingest_unified_wallet_balance_json(
                r#"{"retCode":0,"retMsg":"OK","result":{"list":[{"accountType":"UNIFIED","coin":[{"coin":"USDT","walletBalance":"1000.00000000","availableToWithdraw":"975.00000000","locked":"10.00000000","borrowAmount":"5.00000000","totalOrderIM":"15.00000000"},{"coin":"BTC","walletBalance":"0.50000000","availableToWithdraw":"0.40000000","locked":"0.05000000","borrowAmount":"0","totalOrderIM":"0.05000000"}]}]},"retExtInfo":{},"time":1767225601000}"#,
                "redacted:bybit-wallet-balance",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("wallet balance snapshot");

        assert_eq!(
            batch.balance_event.event_type,
            NormalizedEventType::BalanceSnapshotEvent
        );
        assert_eq!(
            payload_string(&batch.balance_event, "endpoint"),
            "/v5/account/wallet-balance"
        );
        assert_eq!(
            payload_string(&batch.balance_event, "risk_reason_code"),
            "CHECK_PASSED"
        );
        assert_eq!(batch.balances.len(), 2);

        let usdt = adapter
            .balances(
                &BalanceQuery::new(VenueId::new("venue:BYBIT-PRIVATE").expect("venue"))
                    .for_account(AccountId::new("account:bybit-read-only").expect("account"))
                    .for_asset(AssetId::new("asset:USDT").expect("asset")),
            )
            .expect("balances");
        assert_eq!(usdt.len(), 1);
        assert_eq!(usdt[0].free.to_string(), "975.00000000");
        assert_eq!(usdt[0].locked.to_string(), "10.00000000");
        assert_eq!(usdt[0].reserved.to_string(), "15.00000000");
        assert_eq!(usdt[0].borrowed.to_string(), "5.00000000");
        assert_eq!(
            usdt[0].source_event_id.as_deref(),
            Some(batch.balance_event.event_id.as_str())
        );

        let rendered = to_canonical_json(&batch.balance_event);
        assert!(!rendered.contains("975.00000000"));
        assert!(!rendered.contains("api_key"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn bybit_private_linear_maps_signed_position_list() {
        let mut adapter = bybit_private_linear_adapter();
        let batch = adapter
            .ingest_linear_position_list_json(
                r#"{"retCode":0,"retMsg":"OK","result":{"category":"linear","list":[{"symbol":"BTCUSDT","side":"Sell","size":"1.000","avgPrice":"43100.00","markPrice":"43250.50","unrealisedPnl":"-150.50","liqPrice":"45000.00","updatedTime":"1767225601500"},{"symbol":"ETHUSDT","side":"Buy","size":"0","avgPrice":"0","markPrice":"2500.00","unrealisedPnl":"0","liqPrice":"","updatedTime":"1767225601500"}]},"retExtInfo":{},"time":1767225601000}"#,
                "redacted:bybit-linear-positions",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("position list snapshot");

        assert_eq!(
            batch.position_event.event_type,
            NormalizedEventType::PositionSnapshotEvent
        );
        assert_eq!(
            payload_string(&batch.position_event, "endpoint"),
            "/v5/position/list"
        );
        assert_eq!(batch.positions.len(), 1);
        let position = &batch.positions[0];
        assert_eq!(
            position.instrument_id.as_str(),
            "inst:BYBIT:BTCUSDT:LINEAR-PERP"
        );
        assert_eq!(position.quantity.to_string(), "-1.000");
        assert_eq!(position.mark_price.to_string(), "43250.50");
        assert_eq!(position.unrealized_pnl.to_string(), "-150.50");
        assert_eq!(
            position.liquidation_price.expect("liq").to_string(),
            "45000.00"
        );

        let queried = adapter
            .positions(
                &PositionQuery::new(VenueId::new("venue:BYBIT-LINEAR-PRIVATE").expect("venue"))
                    .for_account(AccountId::new("account:bybit-read-only").expect("account"))
                    .for_instrument(
                        InstrumentId::new("inst:BYBIT:BTCUSDT:LINEAR-PERP").expect("instrument"),
                    ),
            )
            .expect("positions");
        assert_eq!(queried, batch.positions);
    }

    #[test]
    fn bybit_private_ret_code_fails_closed() {
        let mut adapter = bybit_private_unified_adapter();
        let error = adapter
            .ingest_unified_wallet_balance_json(
                r#"{"retCode":10001,"retMsg":"request parameter error","result":{"list":[]},"retExtInfo":{},"time":1767225601000}"#,
                "redacted:bybit-wallet-balance",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect_err("non-zero retCode must fail closed");

        let error = expect_external(error);
        assert_eq!(error.surface, ReadOnlySurface::Balance);
        assert_eq!(error.class, ExternalErrorClass::UnknownExternalState);
        assert!(error.fail_closed);
    }

    #[test]
    fn venue_data_smoke_fixture_matches_adapter_output_and_is_replayable() {
        let mut adapter = binance_adapter();
        let batch = adapter
            .ingest_ticker_24h_json(
                &read_fixture("raw/binance_ticker_24hr.redacted.json"),
                "fixtures/replay/venue_data_smoke/raw/binance_ticker_24hr.redacted.json",
                timestamp("2026-01-01T00:00:02Z"),
            )
            .expect("ticker");
        let disconnect = adapter
            .record_disconnect(
                "fixture websocket closed before next public ticker update",
                timestamp("2026-01-01T00:00:10Z"),
                timestamp("2026-01-01T00:00:11Z"),
            )
            .expect("disconnect");
        let reconnecting = adapter
            .record_reconnecting(
                "fixture websocket reconnect scheduled",
                timestamp("2026-01-01T00:00:12Z"),
                timestamp("2026-01-01T00:00:13Z"),
            )
            .expect("reconnecting");
        let rate_limited = adapter
            .record_rate_limit(
                fixture_rate_limit(),
                "fixture HTTP 429 public ticker rate limit header observed",
                timestamp("2026-01-01T00:00:14Z"),
                timestamp("2026-01-01T00:00:15Z"),
            )
            .expect("rate limit");

        let actual = [
            to_canonical_json(&batch.raw_event),
            to_canonical_json(&batch.normalized_event),
            to_canonical_json(&disconnect),
            to_canonical_json(&reconnecting),
            to_canonical_json(&rate_limited),
        ]
        .join("\n")
            + "\n";

        assert_eq!(
            actual,
            read_fixture("expected/normalized_events.jsonl"),
            "expected normalized event fixture must match adapter output"
        );
        assert_eq!(
            actual,
            read_fixture("events.jsonl"),
            "events.jsonl must be directly replayable normalized events"
        );

        for (index, line) in actual.lines().enumerate() {
            let event = from_json_strict::<NormalizedEvent>(line)
                .unwrap_or_else(|error| panic!("line {} should parse: {error}", index + 1));
            assert_eq!(
                canonical_normalized_event_hash(&event),
                event.checksum.as_str()
            );
        }

        let capability_line = read_fixture("venue_capabilities.jsonl");
        let capability_line = capability_line.trim();
        let capability = from_json_strict::<VenueCapabilityDescriptor>(capability_line)
            .expect("venue capability fixture should parse");
        assert_eq!(to_canonical_json(&capability), capability_line);
        assert_eq!(capability.venue_id.as_str(), "venue:BINANCE_PUBLIC");
        let permission = capability
            .permission_model
            .as_ref()
            .expect("permission model");
        assert_eq!(permission.can_read_public_data, Some(true));
        assert_eq!(permission.can_read_private_data, Some(false));
        assert_eq!(permission.can_trade, Some(false));
        assert_eq!(permission.can_withdraw, Some(false));

        let classifications = read_fixture("expected/error_classifications.jsonl");
        assert!(classifications.contains("\"scenario\":\"rate_limit\""));
        assert!(classifications.contains("\"scenario\":\"reconnecting\""));
    }

    fn fixture_adapter() -> FixtureReadOnlyAdapter {
        let venue_id = VenueId::new("venue:SIM").expect("venue id");
        let instrument_id = InstrumentId::new("inst:BTC-USDC").expect("instrument id");
        let account_id = AccountId::new("acct:cash").expect("account id");
        let asset_usdc = AssetId::new("asset:USDC").expect("asset id");
        let asset_btc = AssetId::new("asset:BTC").expect("asset id");
        let freshness = DataFreshness::new(
            timestamp("2026-01-01T00:00:00Z"),
            timestamp("2026-01-01T00:00:02Z"),
            5_000,
        )
        .expect("freshness");

        FixtureReadOnlyAdapter {
            venue_id: venue_id.clone(),
            quote: MarketQuote {
                venue_id: venue_id.clone(),
                instrument_id: instrument_id.clone(),
                last_price: Some(price("30000.50")),
                best_bid: Some(price("30000.00")),
                best_ask: Some(price("30001.00")),
                mark_price: Some(price("30000.40")),
                index_price: None,
                bid_size: Some(quantity("1.25")),
                ask_size: Some(quantity("0.75")),
                source_sequence: Some("fixture-seq-1".to_owned()),
                source_event_id: Some("event:venue-data:quote:1".to_owned()),
                freshness,
            },
            balances: vec![VenueBalance {
                venue_id: venue_id.clone(),
                account_id: account_id.clone(),
                asset_id: asset_usdc.clone(),
                free: amount("1000.00"),
                locked: amount("0"),
                reserved: amount("0"),
                pending: amount("0"),
                borrowed: amount("0"),
                lent: amount("0"),
                unsettled: amount("0"),
                source_event_id: Some("event:venue-data:balance:1".to_owned()),
                freshness,
            }],
            positions: vec![VenuePosition {
                venue_id: venue_id.clone(),
                position_id: Some(PositionId::new("pos:BTC-USDC").expect("position id")),
                account_id,
                instrument_id: instrument_id.clone(),
                quantity: Decimal::from_str("0.10").expect("decimal"),
                entry_price: Some(price("29900.00")),
                mark_price: price("30000.50"),
                unrealized_pnl: Pnl::from_str("10.05").expect("pnl"),
                liquidation_price: None,
                source_event_id: Some("event:venue-data:position:1".to_owned()),
                freshness,
            }],
            instruments: vec![InstrumentInfo {
                venue_id: venue_id.clone(),
                instrument_id,
                kind: InstrumentKind::SpotPair,
                base_asset_id: Some(asset_btc),
                quote_asset_id: Some(asset_usdc.clone()),
                settlement_asset_id: asset_usdc,
                margin_asset_id: None,
                tick_size: Some(price("0.01")),
                lot_size: Some(quantity("0.0001")),
                contract_multiplier: None,
                is_active: true,
                source_event_id: Some("event:venue-data:instrument:1".to_owned()),
                freshness,
            }],
            health: VenueHealthSnapshot {
                venue_id,
                status: VenueHealthStatus::Healthy,
                connection: VenueConnectionStatus::Connected,
                reason_codes: Vec::new(),
                rate_limit: Some(RateLimitSnapshot {
                    limit: 60,
                    remaining: Some(59),
                    window_ms: 60_000,
                    resets_at: Some(timestamp("2026-01-01T00:01:00Z")),
                }),
                source_event_id: Some("event:venue-data:health:1".to_owned()),
                freshness,
            },
        }
    }

    fn hybrid_coordinator() -> RestWssMarketDataCoordinator {
        RestWssMarketDataCoordinator::new(
            VenueId::new("venue:BINANCE-SPOT").expect("venue id"),
            InstrumentId::new("inst:BINANCE:BTCUSDT:SPOT").expect("instrument id"),
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("coordinator")
    }

    fn rest_snapshot_quote(sequence: u64, bid: &str, ask: &str) -> MarketQuote {
        let freshness = DataFreshness::new(
            timestamp("2026-01-01T00:00:00Z"),
            timestamp("2026-01-01T00:00:01Z"),
            5_000,
        )
        .expect("freshness");
        MarketQuote {
            venue_id: VenueId::new("venue:BINANCE-SPOT").expect("venue id"),
            instrument_id: InstrumentId::new("inst:BINANCE:BTCUSDT:SPOT").expect("instrument id"),
            last_price: None,
            best_bid: Some(price(bid)),
            best_ask: Some(price(ask)),
            mark_price: None,
            index_price: None,
            bid_size: Some(quantity("1.0")),
            ask_size: Some(quantity("1.5")),
            source_sequence: Some(sequence.to_string()),
            source_event_id: Some(format!("event:rest-snapshot:{sequence}")),
            freshness,
        }
    }

    fn perp_rest_snapshot_quote(bid: &str, ask: &str) -> MarketQuote {
        let freshness = DataFreshness::new(
            timestamp("2026-01-01T00:00:00Z"),
            timestamp("2026-01-01T00:00:01Z"),
            5_000,
        )
        .expect("freshness");
        MarketQuote {
            venue_id: VenueId::new("venue:BINANCE-USDM").expect("venue id"),
            instrument_id: InstrumentId::new("inst:BINANCE:BTCUSDT:USDM-PERP")
                .expect("instrument id"),
            last_price: None,
            best_bid: Some(price(bid)),
            best_ask: Some(price(ask)),
            mark_price: None,
            index_price: None,
            bid_size: Some(quantity("1.0")),
            ask_size: Some(quantity("1.5")),
            source_sequence: Some("100".to_owned()),
            source_event_id: Some("event:rest-snapshot:usdm:100".to_owned()),
            freshness,
        }
    }

    fn wss_quote_update(sequence: u64, bid: &str, ask: &str) -> WssQuoteUpdate {
        WssQuoteUpdate {
            venue_id: VenueId::new("venue:BINANCE-SPOT").expect("venue id"),
            instrument_id: InstrumentId::new("inst:BINANCE:BTCUSDT:SPOT").expect("instrument id"),
            last_price: None,
            best_bid: Some(price(bid)),
            best_ask: Some(price(ask)),
            mark_price: None,
            index_price: None,
            bid_size: Some(quantity("2.0")),
            ask_size: Some(quantity("2.5")),
            source_sequence: sequence,
            source_event_id: Some(format!("event:wss-quote:{sequence}")),
            observed_at: timestamp("2026-01-01T00:00:02Z"),
            ingested_at: timestamp("2026-01-01T00:00:02Z"),
        }
    }

    fn binance_wss_config(market: BinancePublicMarket) -> BinancePublicWssBookTickerConfig {
        let (venue_id, instrument_id) = match market {
            BinancePublicMarket::Spot => ("venue:BINANCE-SPOT", "inst:BINANCE:BTCUSDT:SPOT"),
            BinancePublicMarket::UsdmPerpetual => {
                ("venue:BINANCE-USDM", "inst:BINANCE:BTCUSDT:USDM-PERP")
            }
        };
        let asset_usdt = AssetId::new("asset:USDT").expect("asset id");
        let instrument = BinancePublicInstrument::new(
            "BTCUSDT",
            InstrumentId::new(instrument_id).expect("instrument id"),
            AssetId::new("asset:BTC").expect("asset id"),
            asset_usdt.clone(),
            asset_usdt,
        )
        .expect("instrument")
        .with_tick_size(price("0.01"))
        .with_lot_size(quantity("0.000001"));
        BinancePublicWssBookTickerConfig::new(
            VenueId::new(venue_id).expect("venue id"),
            instrument,
            market,
            5_000,
        )
        .expect("WSS config")
    }

    fn binance_wss_client(market: BinancePublicMarket) -> BinancePublicWssBookTickerClient {
        BinancePublicWssBookTickerClient::new(
            binance_wss_config(market),
            timestamp("2026-01-01T00:00:00Z"),
        )
        .expect("WSS client")
    }

    fn timestamp(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse_rfc3339_z(value).expect("timestamp")
    }

    fn amount(value: &str) -> Amount {
        Amount::from_str(value).expect("amount")
    }

    fn price(value: &str) -> Price {
        Price::from_str(value).expect("price")
    }

    fn quantity(value: &str) -> Quantity {
        Quantity::from_str(value).expect("quantity")
    }

    fn binance_adapter() -> BinancePublicTicker24hAdapter {
        let venue_id = VenueId::new("venue:BINANCE_PUBLIC").expect("venue id");
        let asset_usdt = AssetId::new("asset:USDT").expect("asset id");
        let instrument = BinancePublicInstrument::new(
            "BTCUSDT",
            InstrumentId::new("inst:BTC-USDT").expect("instrument id"),
            AssetId::new("asset:BTC").expect("asset id"),
            asset_usdt.clone(),
            asset_usdt,
        )
        .expect("instrument")
        .with_tick_size(price("0.01"))
        .with_lot_size(quantity("0.000001"));
        BinancePublicTicker24hAdapter::new(
            venue_id,
            instrument,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("adapter")
    }

    fn binance_book_adapter(
        venue_id: &str,
        instrument_id: &str,
        market: BinancePublicMarket,
    ) -> BinancePublicBookTickerAdapter {
        let asset_usdt = AssetId::new("asset:USDT").expect("asset id");
        let instrument = BinancePublicInstrument::new(
            "BTCUSDT",
            InstrumentId::new(instrument_id).expect("instrument id"),
            AssetId::new("asset:BTC").expect("asset id"),
            asset_usdt.clone(),
            asset_usdt,
        )
        .expect("instrument")
        .with_tick_size(price("0.01"))
        .with_lot_size(quantity("0.000001"));
        BinancePublicBookTickerAdapter::new(
            VenueId::new(venue_id).expect("venue id"),
            instrument,
            market,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("adapter")
    }

    fn binance_premium_index_adapter() -> BinanceUsdmPremiumIndexAdapter {
        let asset_usdt = AssetId::new("asset:USDT").expect("asset id");
        let instrument = BinancePublicInstrument::new(
            "BTCUSDT",
            InstrumentId::new("inst:BINANCE:BTCUSDT:USDM-PERP").expect("instrument id"),
            AssetId::new("asset:BTC").expect("asset id"),
            asset_usdt.clone(),
            asset_usdt,
        )
        .expect("instrument");
        BinanceUsdmPremiumIndexAdapter::new(
            VenueId::new("venue:BINANCE-USDM").expect("venue id"),
            instrument,
            5_000,
        )
        .expect("adapter")
    }

    fn bybit_ticker_adapter(
        venue_id: &str,
        instrument_id: &str,
        market: BybitPublicMarket,
    ) -> BybitPublicTickerAdapter {
        let asset_usdt = AssetId::new("asset:USDT").expect("asset id");
        let instrument = BybitPublicInstrument::new(
            "BTCUSDT",
            InstrumentId::new(instrument_id).expect("instrument id"),
            AssetId::new("asset:BTC").expect("asset id"),
            asset_usdt.clone(),
            asset_usdt,
        )
        .expect("instrument")
        .with_tick_size(price("0.01"))
        .with_lot_size(quantity("0.000001"));
        BybitPublicTickerAdapter::new(
            VenueId::new(venue_id).expect("venue id"),
            instrument,
            market,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("adapter")
    }

    fn bybit_premium_index_adapter() -> BybitLinearPremiumIndexAdapter {
        let asset_usdt = AssetId::new("asset:USDT").expect("asset id");
        let instrument = BybitPublicInstrument::new(
            "BTCUSDT",
            InstrumentId::new("inst:BYBIT:BTCUSDT:LINEAR-PERP").expect("instrument id"),
            AssetId::new("asset:BTC").expect("asset id"),
            asset_usdt.clone(),
            asset_usdt,
        )
        .expect("instrument");
        BybitLinearPremiumIndexAdapter::new(
            VenueId::new("venue:BYBIT-LINEAR").expect("venue id"),
            instrument,
            5_000,
        )
        .expect("adapter")
    }

    fn binance_private_spot_adapter() -> BinancePrivateAccountAdapter {
        BinancePrivateAccountAdapter::new(
            VenueId::new("venue:BINANCE-SPOT-PRIVATE").expect("venue"),
            AccountId::new("account:binance-read-only").expect("account"),
            BinancePrivateAccountMarket::Spot,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("private spot adapter")
    }

    fn binance_private_usdm_adapter() -> BinancePrivateAccountAdapter {
        BinancePrivateAccountAdapter::new(
            VenueId::new("venue:BINANCE-USDM-PRIVATE").expect("venue"),
            AccountId::new("account:binance-usdm-read-only").expect("account"),
            BinancePrivateAccountMarket::UsdmFutures,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("private usdm adapter")
    }

    fn binance_private_portfolio_margin_adapter() -> BinancePrivateAccountAdapter {
        BinancePrivateAccountAdapter::new(
            VenueId::new("venue:BINANCE-USDM-PRIVATE").expect("venue"),
            AccountId::new("account:binance-usdm-read-only").expect("account"),
            BinancePrivateAccountMarket::PortfolioMargin,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("private portfolio margin adapter")
    }

    fn bybit_private_unified_adapter() -> BybitPrivateAccountAdapter {
        BybitPrivateAccountAdapter::new(
            VenueId::new("venue:BYBIT-PRIVATE").expect("venue"),
            AccountId::new("account:bybit-read-only").expect("account"),
            BybitPrivateAccountMarket::UnifiedAccount,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("private unified adapter")
    }

    fn bybit_private_linear_adapter() -> BybitPrivateAccountAdapter {
        BybitPrivateAccountAdapter::new(
            VenueId::new("venue:BYBIT-LINEAR-PRIVATE").expect("venue"),
            AccountId::new("account:bybit-read-only").expect("account"),
            BybitPrivateAccountMarket::LinearPerpetual,
            timestamp("2026-01-01T00:00:00Z"),
            5_000,
        )
        .expect("private linear adapter")
    }

    fn fixture_rate_limit() -> RateLimitSnapshot {
        RateLimitSnapshot {
            limit: 1200,
            remaining: Some(0),
            window_ms: 60_000,
            resets_at: Some(timestamp("2026-01-01T00:01:00Z")),
        }
    }

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/replay/venue_data_smoke")
    }

    fn read_fixture(relative: &str) -> String {
        fs::read_to_string(fixture_root().join(relative)).expect("fixture should be readable")
    }

    fn expect_external(error: VenueDataError) -> ClassifiedExternalError {
        match error {
            VenueDataError::External(error) => error,
            other => panic!("expected classified external error, got {other}"),
        }
    }

    fn payload_string<'a>(event: &'a NormalizedEvent, key: &str) -> &'a str {
        match event.payload.get(key) {
            Some(JsonValue::String(value)) => value,
            other => panic!("payload key `{key}` should be a string, got {other:?}"),
        }
    }
}
