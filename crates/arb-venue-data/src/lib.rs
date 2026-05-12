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
enum FlatJsonValue {
    String(String),
    Number(String),
    Bool,
    Null,
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
        self.expect('{')?;
        self.skip_ws();
        let mut object = BTreeMap::new();
        if self.peek() == Some('}') {
            self.pos += 1;
            self.finish()?;
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
                    self.finish()?;
                    return Ok(object);
                }
                _ => return Err(self.error("expected comma or closing brace")),
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
            Some('{' | '[') => {
                Err(self.error("nested JSON is not supported for this flat public ticker response"))
            }
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
    if symbol.len() < 3 || symbol.len() > 32 {
        return Err(VenueDataError::InvalidQuery {
            field: "binance.symbol",
            reason: "symbol length must be 3..=32",
        });
    }
    if !symbol
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(VenueDataError::InvalidQuery {
            field: "binance.symbol",
            reason: "symbol must contain only uppercase ASCII letters and digits",
        });
    }
    Ok(())
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

fn timestamp_from_unix_millis(value: u64) -> Result<UtcTimestamp, String> {
    let seconds = i64::try_from(value / 1_000)
        .map_err(|_| "closeTime milliseconds overflow i64 seconds".to_owned())?;
    let nanos = u32::try_from((value % 1_000) * 1_000_000)
        .map_err(|_| "closeTime millisecond remainder overflowed nanoseconds".to_owned())?;
    UtcTimestamp::from_unix_parts(seconds, nanos).map_err(|error| error.to_string())
}

fn raw_event_id(symbol: &str, source_sequence: u64) -> String {
    format!("event:venue-data:binance-public:{symbol}:{source_sequence}:raw")
}

fn normalized_event_id(symbol: &str, source_sequence: u64) -> String {
    format!("event:venue-data:binance-public:{symbol}:{source_sequence}:normalized")
}

fn correlation_id(symbol: &str, source_sequence: u64) -> String {
    format!("corr:venue-data:binance-public:{symbol}:{source_sequence}")
}

fn binance_public_raw_event_id(
    stream: &str,
    market: BinancePublicMarket,
    symbol: &str,
    source_sequence: &str,
) -> String {
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
    format!(
        "corr:venue-data:binance-public:{stream}:{}:{symbol}:{source_sequence}",
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
