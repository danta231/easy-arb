use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arb_domain::{AssetId, InstrumentId, Price, Quantity, UtcTimestamp, VenueId};
use arb_venue_data::{
    BinancePublicBookTickerAdapter, BinancePublicInstrument, BinancePublicMarket,
    BinancePublicWssBookTickerClient, BinancePublicWssBookTickerConfig,
    BinancePublicWssTextStreamClient, BybitPublicInstrument, BybitPublicMarket,
    BybitPublicWssTextStreamClient, DataFreshness, HybridMarketDataInput, HybridMarketDataStatus,
    HybridMarketDataUpdate, MarketDataQuery, MarketDataReader, MarketDataTransport, MarketQuote,
    PublicJsonWssTextStreamClient, RestWssMarketDataCoordinator, WssQuoteUpdate,
};

use crate::{
    aster_futures_book_ticker_all_url, aster_futures_book_ticker_url, basis_identifier_component,
    binance_spot_book_ticker_all_url, binance_spot_book_ticker_url,
    binance_usdm_book_ticker_all_url, binance_usdm_book_ticker_url, bitget_spot_tickers_url,
    bitget_ticker_value_to_string, bitget_usdt_futures_tickers_url, bybit_linear_tickers_url,
    bybit_spot_tickers_url, cli_arg_error, current_utc_timestamp, current_utc_timestamp_string,
    decode_json_string_literal, fetch_public_json_post_with_curl, fetch_public_json_with_curl,
    funding_display_symbol, gate_usdt_futures_tickers_url, hyperliquid_info_request_body,
    json_array_value_slices, json_object_slices, json_option_string, json_string, json_string_end,
    normalize_cex_usdt_basis_symbol, normalize_okx_usdt_basis_symbol, okx_tickers_url,
    optional_json_value_string, parse_bitget_spot_ticker_rows, parse_book_ticker_rows,
    parse_bybit_linear_ticker_rows, parse_bybit_spot_ticker_rows, parse_flat_json_object,
    parse_gate_futures_ticker_rows, parse_hyperliquid_perp_context_rows,
    parse_json_object_value_slices, parse_okx_ticker_rows, required_first_json_value_string,
    required_json_string, required_json_value_string, runtime_timestamp_millis,
    static_dashboard_gone_json, write_http_json, GateFuturesTickerRow, MonitorBookTickerRow,
    MonitorJsonScalar, OkxTickerRow, RuntimeError, RuntimeResult, BASIS_SYMBOL,
    BITGET_BASIS_SYMBOL, BYBIT_BASIS_PERP_VENUE_ID, BYBIT_BASIS_SPOT_VENUE_ID,
    HYPERLIQUID_INFO_URL, MULTI_VENUE_BITGET_SPOT_WSS_BIND_ADDR,
    MULTI_VENUE_OKX_SPOT_WSS_BIND_ADDR, OKX_BASIS_SYMBOL,
};

pub(crate) const HYPERLIQUID_PUBLIC_WSS_URL: &str = "wss://api.hyperliquid.xyz/ws";
pub(crate) const ASTER_PUBLIC_WSS_BASE_URL: &str = "wss://fstream.asterdex.com";
pub(crate) const GATE_PUBLIC_FUTURES_WSS_URL: &str = "wss://fx-ws.gateio.ws/v4/ws/usdt";
pub(crate) const LIGHTER_PUBLIC_WSS_URL: &str = "wss://mainnet.zklighter.elliot.ai/stream";
pub(crate) const PUBLIC_WSS_MONITOR_MAX_AGE_MS: u64 = 20_000;
pub(crate) const PUBLIC_WSS_RECONNECT_BACKOFF_MAX_SECS: u64 = 60;
pub(crate) const PUBLIC_WSS_FAILURE_LOG_REPEAT_INTERVAL: u32 = 500;
pub(crate) const PUBLIC_WSS_DEGRADED_STALE_ROW_RATIO_BPS: u64 = 2_500;
pub(crate) const PUBLIC_WSS_DEGRADED_RECONNECT_COUNT: u64 = 100;
pub(crate) const BINANCE_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8801";
pub(crate) const BINANCE_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS: u64 = 2;
pub(crate) const BINANCE_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS: &str = "ALL_USDT";
pub(crate) const BYBIT_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8802";
pub(crate) const BYBIT_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS: u64 = 2;
pub(crate) const BYBIT_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS: &str = "ALL_USDT";
pub(crate) const OKX_PUBLIC_WSS_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";
pub(crate) const BITGET_PUBLIC_WSS_URL: &str = "wss://ws.bitget.com/v2/ws/public";
pub(crate) const OKX_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS: u64 = 2;
pub(crate) const BITGET_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS: u64 = 10;
pub(crate) const OKX_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS: &str = "ALL_USDT";
pub(crate) const BITGET_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS: &str = "ALL_USDT";
pub(crate) const BITGET_WSS_TICKER_DEFAULT_INST_ID: &str = "default";
pub(crate) const OKX_WSS_SUBSCRIBE_PAYLOAD_MAX_BYTES: usize = 4096;
pub(crate) const OKX_WSS_SUBSCRIBE_MAX_ARGS_PER_PAYLOAD: usize = 50;
pub(crate) const BITGET_WSS_SUBSCRIBE_PAYLOAD_MAX_BYTES: usize = 4096;
pub(crate) const BITGET_WSS_SUBSCRIBE_MAX_ARGS_PER_PAYLOAD: usize = 25;
pub(crate) const ASTER_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8794";
pub(crate) const HYPERLIQUID_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8795";
pub(crate) const LIGHTER_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8824";
pub(crate) const GATE_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8825";
pub(crate) const ASTER_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS: u64 = 2;
pub(crate) const HYPERLIQUID_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS: u64 = 2;
pub(crate) const LIGHTER_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS: u64 = 2;
pub(crate) const GATE_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS: u64 = 2;
pub(crate) const HYPERLIQUID_WSS_SUBSCRIBE_DELAY_MS: u64 = 10;
pub(crate) const LIGHTER_WSS_SUBSCRIBE_DELAY_MS: u64 = 10;
pub(crate) const GATE_WSS_SUBSCRIBE_DELAY_MS: u64 = 10;
pub(crate) const PUBLIC_WSS_BROAD_EXPLICIT_SYMBOL_SCOPE_MIN_SYMBOLS: usize = 32;
pub(crate) const ASTER_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS: &str = "ALL_USDT";
pub(crate) const HYPERLIQUID_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS: &str = "ALL_USDT";
pub(crate) const LIGHTER_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS: &str = "ALL_USDT";
pub(crate) const GATE_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS: &str = "ALL_USDT";
pub(crate) const BYBIT_SPOT_PUBLIC_WSS_BASE_URL: &str = "wss://stream.bybit.com/v5/public/spot";
pub(crate) const BYBIT_LINEAR_PUBLIC_WSS_BASE_URL: &str = "wss://stream.bybit.com/v5/public/linear";
pub(crate) const BYBIT_LINEAR_WSS_ORDERBOOK_TOPIC_SCOPE_LIMIT: usize = 100;
pub(crate) const BYBIT_LINEAR_WSS_TICKER_TOPIC_SUBSCRIBE_LIMIT: usize = 50;
pub(crate) const BYBIT_LINEAR_WSS_PRIORITY_SYMBOLS: &[&str] = &[
    "BTCUSDT", "ETHUSDT", "SOLUSDT", "BNBUSDT", "XRPUSDT", "DOGEUSDT", "ADAUSDT", "SUIUSDT",
    "TRXUSDT", "LINKUSDT", "AVAXUSDT", "LTCUSDT",
];

/// Binance `bookTicker` WSS 公开行情常驻任务选项。
///
/// 中文说明：该选项只允许公开行情 WSS，不读取账户、不下单、不撤单、不转账、
/// 不签名。REST 快照始终先于 WSS，用于启动和异常后的重建。默认是常驻任务；
/// `once` 只用于手动验收旧的有限条探测路径。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceWssBookTickerMonitorOptions {
    pub bind_addr: String,
    pub symbol: String,
    pub market: BinancePublicMarket,
    pub updates: usize,
    pub reconnect_delay_secs: u64,
    pub once: bool,
}

impl Default for BinanceWssBookTickerMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: BINANCE_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR.to_owned(),
            symbol: BINANCE_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS.to_owned(),
            market: BinancePublicMarket::Spot,
            updates: 3,
            reconnect_delay_secs: BINANCE_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS,
            once: false,
        }
    }
}

/// 兼容旧名称：历史上该命令只做有限条探测。
pub type BinanceWssBookTickerProbeOptions = BinanceWssBookTickerMonitorOptions;

/// Bybit V5 WSS 公开行情常驻任务选项。
///
/// 中文说明：该选项只允许公开行情 WSS。Bybit V5 连接后通过公开 topic 订阅
/// 小范围 `orderbook.1.<symbol>` 或大范围 Linear Perp 的 `tickers.<symbol>`，
/// 不读取账户、不下单、不撤单、不转账、不签名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitWssBookTickerMonitorOptions {
    pub bind_addr: String,
    pub symbol: String,
    pub market: BybitPublicMarket,
    pub updates: usize,
    pub reconnect_delay_secs: u64,
    pub once: bool,
}

impl Default for BybitWssBookTickerMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: BYBIT_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR.to_owned(),
            symbol: BYBIT_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS.to_owned(),
            market: BybitPublicMarket::Spot,
            updates: 3,
            reconnect_delay_secs: BYBIT_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS,
            once: false,
        }
    }
}

pub type BybitWssBookTickerProbeOptions = BybitWssBookTickerMonitorOptions;

/// OKX 公开 WSS ticker 支持的市场。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OkxPublicWssMarket {
    Spot,
    Swap,
}

impl OkxPublicWssMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spot => "SPOT",
            Self::Swap => "SWAP",
        }
    }

    pub(crate) fn venue_id(self) -> &'static str {
        match self {
            Self::Spot => "venue:OKX-SPOT",
            Self::Swap => "venue:OKX-SWAP",
        }
    }
}

/// Bitget 公开 WSS ticker 支持的市场。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BitgetPublicWssMarket {
    Spot,
    UsdtFutures,
}

impl BitgetPublicWssMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spot => "spot",
            Self::UsdtFutures => "usdt-futures",
        }
    }

    pub(crate) fn inst_type(self) -> &'static str {
        match self {
            Self::Spot => "SPOT",
            Self::UsdtFutures => "USDT-FUTURES",
        }
    }

    pub(crate) fn venue_id(self) -> &'static str {
        match self {
            Self::Spot => "venue:BITGET-SPOT",
            Self::UsdtFutures => "venue:BITGET-USDT-FUTURES",
        }
    }
}

/// Aster 公开 WSS bookTicker 支持的市场。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AsterPublicWssMarket {
    UsdtFutures,
}

impl AsterPublicWssMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UsdtFutures => "usdt-futures",
        }
    }

    pub(crate) fn venue_id(self) -> &'static str {
        match self {
            Self::UsdtFutures => "venue:ASTER-USDT-FUTURES",
        }
    }
}

/// Hyperliquid 公开 WSS 顶层盘口支持的市场。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HyperliquidPublicWssMarket {
    Perp,
}

impl HyperliquidPublicWssMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Perp => "perp",
        }
    }

    pub(crate) fn venue_id(self) -> &'static str {
        match self {
            Self::Perp => "venue:HYPERLIQUID-PERP",
        }
    }
}

/// Lighter 公开 WSS 顶层盘口支持的市场。
///
/// 中文说明：该 staged 接入只订阅公开 `ticker/{MARKET_INDEX}`，不使用账户、
/// auth token、API key、nonce 或 sendTx。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LighterPublicWssMarket {
    Perp,
}

impl LighterPublicWssMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Perp => "perp",
        }
    }

    pub(crate) fn venue_id(self) -> &'static str {
        match self {
            Self::Perp => "venue:LIGHTER-PERP",
        }
    }
}

/// Gate 公开 WSS `futures.book_ticker` 支持的市场。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GatePublicWssMarket {
    UsdtFutures,
}

impl GatePublicWssMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UsdtFutures => "usdt-futures",
        }
    }

    pub(crate) fn venue_id(self) -> &'static str {
        match self {
            Self::UsdtFutures => "venue:GATE-USDT-FUTURES",
        }
    }
}

/// OKX `tickers` WSS 公开行情常驻任务选项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OkxWssBookTickerMonitorOptions {
    pub bind_addr: String,
    pub symbol: String,
    pub market: OkxPublicWssMarket,
    pub updates: usize,
    pub reconnect_delay_secs: u64,
    pub once: bool,
}

impl Default for OkxWssBookTickerMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: MULTI_VENUE_OKX_SPOT_WSS_BIND_ADDR.to_owned(),
            symbol: OKX_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS.to_owned(),
            market: OkxPublicWssMarket::Spot,
            updates: 3,
            reconnect_delay_secs: OKX_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS,
            once: false,
        }
    }
}

/// Bitget `ticker` WSS 公开行情常驻任务选项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BitgetWssBookTickerMonitorOptions {
    pub bind_addr: String,
    pub symbol: String,
    pub market: BitgetPublicWssMarket,
    pub updates: usize,
    pub reconnect_delay_secs: u64,
    pub once: bool,
}

impl Default for BitgetWssBookTickerMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: MULTI_VENUE_BITGET_SPOT_WSS_BIND_ADDR.to_owned(),
            symbol: BITGET_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS.to_owned(),
            market: BitgetPublicWssMarket::Spot,
            updates: 3,
            reconnect_delay_secs: BITGET_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS,
            once: false,
        }
    }
}

/// Aster `bookTicker` WSS 公开行情常驻任务选项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AsterWssBookTickerMonitorOptions {
    pub bind_addr: String,
    pub symbol: String,
    pub market: AsterPublicWssMarket,
    pub updates: usize,
    pub reconnect_delay_secs: u64,
    pub once: bool,
}

impl Default for AsterWssBookTickerMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: ASTER_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR.to_owned(),
            symbol: ASTER_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS.to_owned(),
            market: AsterPublicWssMarket::UsdtFutures,
            updates: 3,
            reconnect_delay_secs: ASTER_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS,
            once: false,
        }
    }
}

/// Hyperliquid 顶层盘口 WSS 公开行情常驻任务选项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidWssBookTickerMonitorOptions {
    pub bind_addr: String,
    pub symbol: String,
    pub market: HyperliquidPublicWssMarket,
    pub updates: usize,
    pub reconnect_delay_secs: u64,
    pub once: bool,
}

impl Default for HyperliquidWssBookTickerMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: HYPERLIQUID_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR.to_owned(),
            symbol: HYPERLIQUID_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS.to_owned(),
            market: HyperliquidPublicWssMarket::Perp,
            updates: 3,
            reconnect_delay_secs: HYPERLIQUID_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS,
            once: false,
        }
    }
}

/// Lighter `ticker/{MARKET_INDEX}` WSS 公开行情常驻任务选项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LighterWssBookTickerMonitorOptions {
    pub bind_addr: String,
    pub symbol: String,
    pub market: LighterPublicWssMarket,
    pub updates: usize,
    pub reconnect_delay_secs: u64,
    pub once: bool,
}

impl Default for LighterWssBookTickerMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: LIGHTER_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR.to_owned(),
            symbol: LIGHTER_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS.to_owned(),
            market: LighterPublicWssMarket::Perp,
            updates: 3,
            reconnect_delay_secs: LIGHTER_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS,
            once: false,
        }
    }
}

/// Gate `futures.book_ticker` WSS 公开行情常驻任务选项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GateWssBookTickerMonitorOptions {
    pub bind_addr: String,
    pub symbol: String,
    pub market: GatePublicWssMarket,
    pub updates: usize,
    pub reconnect_delay_secs: u64,
    pub once: bool,
}

impl Default for GateWssBookTickerMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: GATE_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR.to_owned(),
            symbol: GATE_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS.to_owned(),
            market: GatePublicWssMarket::UsdtFutures,
            updates: 3,
            reconnect_delay_secs: GATE_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS,
            once: false,
        }
    }
}

/// Binance `bookTicker` WSS 公开行情探测结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceWssBookTickerProbeReport {
    pub symbol: String,
    pub market: BinancePublicMarket,
    pub stream_url: String,
    pub coordinator_status: String,
    pub update_count: usize,
    pub fail_closed_count: usize,
    pub latest_best_bid: Option<String>,
    pub latest_best_ask: Option<String>,
}

/// Bybit V5 `orderbook.1` WSS 公开行情探测结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitWssBookTickerProbeReport {
    pub symbol: String,
    pub market: BybitPublicMarket,
    pub stream_url: String,
    pub coordinator_status: String,
    pub update_count: usize,
    pub fail_closed_count: usize,
    pub latest_best_bid: Option<String>,
    pub latest_best_ask: Option<String>,
}

/// 公开 WSS top-of-book 最新报价快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicTopOfBookQuoteSnapshot {
    pub symbol: String,
    pub venue_id: String,
    pub instrument_id: String,
    pub best_bid: Option<String>,
    pub best_ask: Option<String>,
    pub bid_size: Option<String>,
    pub ask_size: Option<String>,
    pub source_sequence: Option<String>,
    pub source_event_id: Option<String>,
    pub observed_at: String,
    pub ingested_at: String,
    pub freshness_status: String,
}

/// 公开 WSS top-of-book 常驻任务状态快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicTopOfBookMonitorSnapshot {
    pub status: String,
    pub updated_at: String,
    pub symbol: String,
    pub market: String,
    pub stream_url: String,
    pub coordinator_status: String,
    pub latest_quote: Option<PublicTopOfBookQuoteSnapshot>,
    pub rows: Vec<PublicTopOfBookQuoteSnapshot>,
    pub total_rows: usize,
    pub fail_closed: bool,
    pub fail_closed_count: u64,
    pub disconnect_count: u64,
    pub rest_rebuild_count: u64,
    pub wss_update_count: u64,
    pub last_error: Option<String>,
}

pub(crate) fn bootstrap_binance_wss_book_ticker_client(
    symbol: &str,
    market: BinancePublicMarket,
    venue_id: &VenueId,
    instrument: &BinancePublicInstrument,
) -> RuntimeResult<(BinancePublicWssBookTickerClient, HybridMarketDataUpdate)> {
    let rest_url = match market {
        BinancePublicMarket::Spot => binance_spot_book_ticker_url(symbol),
        BinancePublicMarket::UsdmPerpetual => binance_usdm_book_ticker_url(symbol),
    };
    let raw_rest_snapshot = fetch_public_json_with_curl(&rest_url)?;
    let started_at = current_utc_timestamp()?;

    let mut rest_adapter = BinancePublicBookTickerAdapter::new(
        venue_id.clone(),
        instrument.clone(),
        market,
        started_at,
        PUBLIC_WSS_MONITOR_MAX_AGE_MS,
    )?;
    let rest_batch = rest_adapter.ingest_book_ticker_json(
        &raw_rest_snapshot,
        &rest_url,
        current_utc_timestamp()?,
    )?;

    let config = BinancePublicWssBookTickerConfig::new(
        venue_id.clone(),
        instrument.clone(),
        market,
        PUBLIC_WSS_MONITOR_MAX_AGE_MS,
    )?;
    let mut client = BinancePublicWssBookTickerClient::new(config, started_at)?;
    let rest_update = client.apply_rest_snapshot(rest_batch.quote)?;
    Ok((client, rest_update))
}

/// 运行一次 Binance 公开 `bookTicker` WSS 探测。
///
/// 中文说明：该路径先用 REST bookTicker 建立启动快照，然后连接 Binance 真实
/// WSS 公开行情读取有限条更新。它不使用 API key、不读取账户、不产生任何账户
/// 变更；异常时协调器会 fail closed，REST 快照仍是补洞和重建入口。
pub fn run_binance_wss_book_ticker_probe(
    options: BinanceWssBookTickerProbeOptions,
) -> RuntimeResult<BinanceWssBookTickerProbeReport> {
    validate_binance_wss_probe_options(&options)?;
    let symbol = validate_binance_public_wss_symbol(&options.symbol)?;
    let venue_id = binance_public_wss_venue_id(options.market)?;
    let instrument = binance_public_wss_instrument(&symbol, options.market)?;
    let (mut client, _rest_update) =
        bootstrap_binance_wss_book_ticker_client(&symbol, options.market, &venue_id, &instrument)?;
    let stream_url = client.stream_url();
    let updates = client.read_live_wss_updates(options.updates)?;
    let fail_closed_count = updates.iter().filter(|update| update.fail_closed).count();
    let latest_quote = client.coordinator().latest_quote(&MarketDataQuery::new(
        venue_id,
        instrument.instrument_id.clone(),
    ))?;

    Ok(BinanceWssBookTickerProbeReport {
        symbol,
        market: options.market,
        stream_url,
        coordinator_status: client.coordinator().status().as_str().to_owned(),
        update_count: updates.len(),
        fail_closed_count,
        latest_best_bid: latest_quote
            .as_ref()
            .and_then(|quote| quote.best_bid.map(|price| price.to_string())),
        latest_best_ask: latest_quote
            .as_ref()
            .and_then(|quote| quote.best_ask.map(|price| price.to_string())),
    })
}

/// 运行 Binance 公开 `bookTicker` WSS 常驻任务。
///
/// 中文说明：默认启动本地 HTTP API 并持续连接真实 Binance 公开 WSS。每次连接前
/// 都先走 REST 快照；断线、读失败或异常结束后 fail closed，等待固定间隔后重新
/// 通过 REST 快照重建，再接回 WSS。该任务不读取账户、不签名、不提交可变操作。
pub fn run_binance_wss_book_ticker_monitor(
    options: BinanceWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    validate_binance_wss_probe_options(&options)?;
    let symbol_scope = normalize_binance_wss_symbol_scope(&options.symbol)?;
    let state = Arc::new(RwLock::new(PublicTopOfBookMonitorSnapshot::empty(
        &symbol_scope,
        options.market,
        "pending-rest-bootstrap",
    )));
    if !options.once {
        start_binance_wss_book_ticker_http_api(&options.bind_addr, state.clone())?;
        println!(
            "binance-wss-book-ticker: api=http://{} symbol_scope={} market={} reconnect_delay_secs={} mutable_execution_started=false",
            options.bind_addr,
            symbol_scope,
            options.market.as_str(),
            options.reconnect_delay_secs,
        );
    }

    let mut rebuild_from_rest = false;
    let mut consecutive_failures = 0_u32;
    let mut failure_logger = PublicWssCycleFailureLogger::new("binance-wss-book-ticker");
    loop {
        let cycle = run_binance_wss_book_ticker_monitor_cycle(
            &options,
            state.clone(),
            &symbol_scope,
            rebuild_from_rest,
        );
        match cycle {
            Ok(()) if options.once => return Ok(()),
            Ok(()) => {
                consecutive_failures = 0;
                failure_logger.record_success();
            }
            Err(error) if options.once => return Err(error),
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let reconnect_backoff = public_wss_reconnect_backoff(
                    options.reconnect_delay_secs,
                    consecutive_failures,
                );
                failure_logger.record_failure(&error, consecutive_failures, reconnect_backoff);
                rebuild_from_rest = true;
                thread::sleep(reconnect_backoff);
                continue;
            }
        }
        rebuild_from_rest = true;
        thread::sleep(public_wss_reconnect_backoff(
            options.reconnect_delay_secs,
            consecutive_failures,
        ));
    }
}

pub(crate) fn run_binance_wss_book_ticker_monitor_cycle(
    options: &BinanceWssBookTickerMonitorOptions,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
    symbol_scope: &str,
    rebuild_from_rest: bool,
) -> RuntimeResult<()> {
    if rebuild_from_rest {
        state
            .write()
            .expect("Public WSS monitor state lock poisoned")
            .begin_rest_rebuild();
    }
    let mut market_state =
        match bootstrap_binance_wss_book_ticker_all_market(symbol_scope, options.market) {
            Ok(bootstrap) => bootstrap,
            Err(error) => {
                state
                    .write()
                    .expect("Public WSS monitor state lock poisoned")
                    .record_failure(
                        format!("REST snapshot bootstrap/rebuild failed: {error}"),
                        false,
                    );
                return Err(error);
            }
        };
    {
        let mut snapshot = state
            .write()
            .expect("Public WSS monitor state lock poisoned");
        snapshot.stream_url = market_state.stream_url.clone();
        snapshot.symbol = symbol_scope.to_owned();
        snapshot.market = options.market.as_str().to_owned();
        snapshot.rows.clear();
        snapshot.latest_quote = None;
        snapshot.total_rows = 0;
        for update in &market_state.rest_updates {
            snapshot.record_update(update);
        }
    }

    let connected_at = current_utc_timestamp()?;
    for coordinator in market_state.coordinators.values_mut() {
        let update = coordinator.apply(HybridMarketDataInput::WssConnected {
            occurred_at: connected_at,
            ingested_at: connected_at,
        })?;
        state
            .write()
            .expect("Public WSS monitor state lock poisoned")
            .record_update(&update);
    }

    let text_client = BinancePublicWssTextStreamClient::new(
        market_state.venue_id.clone(),
        market_state.stream_url.clone(),
    )?;
    let max_text_messages = if options.once {
        options.updates
    } else {
        usize::MAX
    };
    let mut observed_wss_event = false;
    let mut observer_error = None;
    let read_result =
        text_client.read_live_text_messages_observed(max_text_messages, |raw_json, ingested_at| {
            observed_wss_event = true;
            match apply_binance_wss_book_ticker_text(
                raw_json,
                ingested_at,
                options.market,
                &mut market_state,
            ) {
                Ok(Some(update)) => {
                    let keep_going = !update.fail_closed;
                    state
                        .write()
                        .expect("Public WSS monitor state lock poisoned")
                        .record_update(&update);
                    keep_going
                }
                Ok(None) => true,
                Err(error) => {
                    observer_error = Some(error.to_string());
                    state
                        .write()
                        .expect("Public WSS monitor state lock poisoned")
                        .record_failure(error.to_string(), false);
                    false
                }
            }
        });

    if let Some(error) = observer_error {
        return Err(RuntimeError::LiveMarketData { message: error });
    }
    match read_result {
        Ok(()) => {
            if !options.once {
                state
                    .write()
                    .expect("Public WSS monitor state lock poisoned")
                    .record_stream_end_with_label_and_observed(
                        "Binance public WSS",
                        observed_wss_event,
                    );
            }
            Ok(())
        }
        Err(error) => {
            state
                .write()
                .expect("Public WSS monitor state lock poisoned")
                .record_wss_read_error(error.to_string(), observed_wss_event);
            public_wss_monitor_cycle_result_after_read_error(error, observed_wss_event)
        }
    }
}

/// 运行一次 Bybit V5 公开 `orderbook.1` WSS 探测。
pub fn run_bybit_wss_book_ticker_probe(
    options: BybitWssBookTickerProbeOptions,
) -> RuntimeResult<BybitWssBookTickerProbeReport> {
    validate_bybit_wss_probe_options(&options)?;
    let symbol = validate_bybit_public_wss_symbol(&options.symbol)?;
    let venue_id = bybit_public_wss_venue_id(options.market)?;
    let instrument = bybit_public_wss_instrument(&symbol, options.market)?;
    let mut market_state = bootstrap_bybit_wss_book_ticker_all_market(&symbol, options.market)?;
    let connected_at = current_utc_timestamp()?;
    for coordinator in market_state.coordinators.values_mut() {
        let _ = coordinator.apply(HybridMarketDataInput::WssConnected {
            occurred_at: connected_at,
            ingested_at: connected_at,
        })?;
    }
    let text_client = BybitPublicWssTextStreamClient::new(
        market_state.venue_id.clone(),
        market_state.stream_url.clone(),
    )?;
    let subscribe_args = market_state.subscribe_args.clone();
    let mut updates = Vec::new();
    let mut observer_error = None;
    text_client.read_live_text_messages_observed(
        &subscribe_args,
        options.updates,
        |raw_json, ingested_at| match apply_bybit_wss_book_ticker_text(
            raw_json,
            ingested_at,
            options.market,
            &mut market_state,
        ) {
            Ok(Some(update)) => {
                let keep_going = !update.fail_closed;
                updates.push(update);
                keep_going
            }
            Ok(None) => true,
            Err(error) => {
                observer_error = Some(error.to_string());
                false
            }
        },
    )?;
    if let Some(error) = observer_error {
        return Err(RuntimeError::LiveMarketData { message: error });
    }
    let fail_closed_count = updates.iter().filter(|update| update.fail_closed).count();
    let latest_quote = market_state
        .coordinators
        .get(&symbol)
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("Bybit WSS coordinator missing symbol `{symbol}`"),
        })?
        .latest_quote(&MarketDataQuery::new(
            venue_id,
            instrument.instrument_id.clone(),
        ))?;

    Ok(BybitWssBookTickerProbeReport {
        symbol,
        market: options.market,
        stream_url: market_state.stream_url,
        coordinator_status: market_state
            .coordinators
            .values()
            .next()
            .map(|coordinator| coordinator.status().as_str().to_owned())
            .unwrap_or_else(|| "missing".to_owned()),
        update_count: updates.len(),
        fail_closed_count,
        latest_best_bid: latest_quote
            .as_ref()
            .and_then(|quote| quote.best_bid.map(|price| price.to_string())),
        latest_best_ask: latest_quote
            .as_ref()
            .and_then(|quote| quote.best_ask.map(|price| price.to_string())),
    })
}

/// 运行 Bybit V5 公开 `orderbook.1` WSS 常驻任务。
pub fn run_bybit_wss_book_ticker_monitor(
    options: BybitWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    validate_bybit_wss_probe_options(&options)?;
    let symbol_scope = normalize_bybit_wss_symbol_scope(&options.symbol)?;
    let state = Arc::new(RwLock::new(
        PublicTopOfBookMonitorSnapshot::empty_with_market(
            &symbol_scope,
            options.market.as_str(),
            "pending-rest-bootstrap",
        ),
    ));
    if !options.once {
        start_bybit_wss_book_ticker_http_api(&options.bind_addr, state.clone())?;
        println!(
            "bybit-wss-book-ticker: api=http://{} symbol_scope={} market={} reconnect_delay_secs={} mutable_execution_started=false",
            options.bind_addr,
            symbol_scope,
            options.market.as_str(),
            options.reconnect_delay_secs,
        );
    }

    let mut rebuild_from_rest = false;
    let mut consecutive_failures = 0_u32;
    let mut failure_logger = PublicWssCycleFailureLogger::new("bybit-wss-book-ticker");
    loop {
        let cycle = run_bybit_wss_book_ticker_monitor_cycle(
            &options,
            state.clone(),
            &symbol_scope,
            rebuild_from_rest,
        );
        match cycle {
            Ok(()) if options.once => return Ok(()),
            Ok(()) => {
                consecutive_failures = 0;
                failure_logger.record_success();
            }
            Err(error) if options.once => return Err(error),
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let reconnect_backoff = public_wss_reconnect_backoff(
                    options.reconnect_delay_secs,
                    consecutive_failures,
                );
                failure_logger.record_failure(&error, consecutive_failures, reconnect_backoff);
                rebuild_from_rest = true;
                thread::sleep(reconnect_backoff);
                continue;
            }
        }
        rebuild_from_rest = true;
        thread::sleep(public_wss_reconnect_backoff(
            options.reconnect_delay_secs,
            consecutive_failures,
        ));
    }
}

pub(crate) fn run_bybit_wss_book_ticker_monitor_cycle(
    options: &BybitWssBookTickerMonitorOptions,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
    symbol_scope: &str,
    rebuild_from_rest: bool,
) -> RuntimeResult<()> {
    if rebuild_from_rest {
        state
            .write()
            .expect("Bybit WSS monitor state lock poisoned")
            .begin_rest_rebuild();
    }
    let mut market_state =
        match bootstrap_bybit_wss_book_ticker_all_market(symbol_scope, options.market) {
            Ok(bootstrap) => bootstrap,
            Err(error) => {
                state
                    .write()
                    .expect("Bybit WSS monitor state lock poisoned")
                    .record_failure(
                        format!("REST snapshot bootstrap/rebuild failed: {error}"),
                        false,
                    );
                return Err(error);
            }
        };
    {
        let mut snapshot = state
            .write()
            .expect("Bybit WSS monitor state lock poisoned");
        snapshot.stream_url = market_state.stream_url.clone();
        snapshot.symbol = symbol_scope.to_owned();
        snapshot.market = options.market.as_str().to_owned();
        snapshot.rows.clear();
        snapshot.latest_quote = None;
        snapshot.total_rows = 0;
        for update in &market_state.rest_updates {
            snapshot.record_update(update);
        }
    }

    let connected_at = current_utc_timestamp()?;
    for coordinator in market_state.coordinators.values_mut() {
        let update = coordinator.apply(HybridMarketDataInput::WssConnected {
            occurred_at: connected_at,
            ingested_at: connected_at,
        })?;
        state
            .write()
            .expect("Bybit WSS monitor state lock poisoned")
            .record_update(&update);
    }

    let text_client = BybitPublicWssTextStreamClient::new(
        market_state.venue_id.clone(),
        market_state.stream_url.clone(),
    )?;
    let subscribe_args = market_state.subscribe_args.clone();
    let max_text_messages = if options.once {
        options.updates
    } else {
        usize::MAX
    };
    let mut observed_wss_event = false;
    let mut observer_error = None;
    let read_result = text_client.read_live_text_messages_observed(
        &subscribe_args,
        max_text_messages,
        |raw_json, ingested_at| match apply_bybit_wss_book_ticker_text(
            raw_json,
            ingested_at,
            options.market,
            &mut market_state,
        ) {
            Ok(Some(update)) => {
                observed_wss_event = true;
                let keep_going = !update.fail_closed;
                state
                    .write()
                    .expect("Bybit WSS monitor state lock poisoned")
                    .record_update(&update);
                keep_going
            }
            Ok(None) => true,
            Err(error) => {
                observer_error = Some(error.to_string());
                state
                    .write()
                    .expect("Bybit WSS monitor state lock poisoned")
                    .record_failure(error.to_string(), false);
                false
            }
        },
    );

    if let Some(error) = observer_error {
        return Err(RuntimeError::LiveMarketData { message: error });
    }
    match read_result {
        Ok(()) => {
            if !options.once {
                state
                    .write()
                    .expect("Bybit WSS monitor state lock poisoned")
                    .record_stream_end_with_label_and_observed(
                        "Bybit public WSS",
                        observed_wss_event,
                    );
            }
            Ok(())
        }
        Err(error) => {
            state
                .write()
                .expect("Bybit WSS monitor state lock poisoned")
                .record_wss_read_error(error.to_string(), observed_wss_event);
            public_wss_monitor_cycle_result_after_read_error(error, observed_wss_event)
        }
    }
}

/// 运行 OKX 公开 `tickers` WSS 常驻任务。
pub fn run_okx_wss_book_ticker_monitor(
    options: OkxWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    validate_okx_wss_probe_options(&options)?;
    let symbol_scope = normalize_okx_wss_symbol_scope(&options.symbol)?;
    let state = Arc::new(RwLock::new(
        PublicTopOfBookMonitorSnapshot::empty_with_market(
            &symbol_scope,
            options.market.as_str(),
            "pending-rest-bootstrap",
        ),
    ));
    if !options.once {
        start_okx_wss_book_ticker_http_api(&options.bind_addr, state.clone())?;
        println!(
            "okx-wss-book-ticker: api=http://{} symbol_scope={} market={} reconnect_delay_secs={} mutable_execution_started=false",
            options.bind_addr,
            symbol_scope,
            options.market.as_str(),
            options.reconnect_delay_secs,
        );
    }

    let mut rebuild_from_rest = false;
    let mut consecutive_failures = 0_u32;
    let mut failure_logger = PublicWssCycleFailureLogger::new("okx-wss-book-ticker");
    loop {
        let cycle = run_okx_wss_book_ticker_monitor_cycle(
            &options,
            state.clone(),
            &symbol_scope,
            rebuild_from_rest,
        );
        match cycle {
            Ok(()) if options.once => return Ok(()),
            Ok(()) => {
                consecutive_failures = 0;
                failure_logger.record_success();
            }
            Err(error) if options.once => return Err(error),
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let reconnect_backoff = public_wss_reconnect_backoff(
                    options.reconnect_delay_secs,
                    consecutive_failures,
                );
                failure_logger.record_failure(&error, consecutive_failures, reconnect_backoff);
                rebuild_from_rest = true;
                thread::sleep(reconnect_backoff);
                continue;
            }
        }
        rebuild_from_rest = true;
        thread::sleep(public_wss_reconnect_backoff(
            options.reconnect_delay_secs,
            consecutive_failures,
        ));
    }
}

pub(crate) fn run_okx_wss_book_ticker_monitor_cycle(
    options: &OkxWssBookTickerMonitorOptions,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
    symbol_scope: &str,
    rebuild_from_rest: bool,
) -> RuntimeResult<()> {
    if rebuild_from_rest {
        state
            .write()
            .expect("OKX WSS monitor state lock poisoned")
            .begin_rest_rebuild();
    }
    let mut market_state = match bootstrap_okx_wss_book_ticker(symbol_scope, options.market) {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            state
                .write()
                .expect("OKX WSS monitor state lock poisoned")
                .record_failure(
                    format!("REST snapshot bootstrap/rebuild failed: {error}"),
                    false,
                );
            return Err(error);
        }
    };
    {
        let mut snapshot = state.write().expect("OKX WSS monitor state lock poisoned");
        snapshot.stream_url = market_state.stream_url.clone();
        snapshot.symbol = symbol_scope.to_owned();
        snapshot.market = options.market.as_str().to_owned();
        snapshot.rows.clear();
        snapshot.latest_quote = None;
        snapshot.total_rows = 0;
        for update in &market_state.rest_updates {
            snapshot.record_update(update);
        }
    }

    let connected_at = current_utc_timestamp()?;
    for coordinator in market_state.coordinators.values_mut() {
        let update = coordinator.apply(HybridMarketDataInput::WssConnected {
            occurred_at: connected_at,
            ingested_at: connected_at,
        })?;
        state
            .write()
            .expect("OKX WSS monitor state lock poisoned")
            .record_update(&update);
    }

    let text_client = PublicJsonWssTextStreamClient::new(
        VenueId::new(options.market.venue_id())?,
        market_state.stream_url.clone(),
        "OKX",
    )?;
    let subscribe_payloads = market_state.subscribe_args.clone();
    let max_text_messages = if options.once {
        options.updates
    } else {
        usize::MAX
    };
    let mut observed_wss_event = false;
    let mut observer_error = None;
    let read_result = text_client.read_live_text_messages_observed_many(
        &subscribe_payloads,
        max_text_messages,
        |raw_json, ingested_at| match apply_okx_wss_book_ticker_text(
            raw_json,
            ingested_at,
            options.market,
            &mut market_state,
        ) {
            Ok(Some(update)) => {
                observed_wss_event = true;
                let keep_going = !update.fail_closed;
                state
                    .write()
                    .expect("OKX WSS monitor state lock poisoned")
                    .record_update(&update);
                keep_going
            }
            Ok(None) => true,
            Err(error) => {
                observer_error = Some(error.to_string());
                state
                    .write()
                    .expect("OKX WSS monitor state lock poisoned")
                    .record_failure(error.to_string(), false);
                false
            }
        },
    );

    if let Some(error) = observer_error {
        return Err(RuntimeError::LiveMarketData { message: error });
    }
    match read_result {
        Ok(()) => {
            if !options.once {
                state
                    .write()
                    .expect("OKX WSS monitor state lock poisoned")
                    .record_stream_end_with_label_and_observed(
                        "OKX public WSS",
                        observed_wss_event,
                    );
            }
            Ok(())
        }
        Err(error) => {
            state
                .write()
                .expect("OKX WSS monitor state lock poisoned")
                .record_wss_read_error(error.to_string(), observed_wss_event);
            public_wss_monitor_cycle_result_after_read_error(error, observed_wss_event)
        }
    }
}

/// 运行 Bitget 公开 `ticker` WSS 常驻任务。
pub fn run_bitget_wss_book_ticker_monitor(
    options: BitgetWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    validate_bitget_wss_probe_options(&options)?;
    let symbol_scope = normalize_bitget_wss_symbol_scope(&options.symbol)?;
    let state = Arc::new(RwLock::new(
        PublicTopOfBookMonitorSnapshot::empty_with_market(
            &symbol_scope,
            options.market.as_str(),
            "pending-rest-bootstrap",
        ),
    ));
    if !options.once {
        start_bitget_wss_book_ticker_http_api(&options.bind_addr, state.clone())?;
        println!(
            "bitget-wss-book-ticker: api=http://{} symbol_scope={} market={} reconnect_delay_secs={} mutable_execution_started=false",
            options.bind_addr,
            symbol_scope,
            options.market.as_str(),
            options.reconnect_delay_secs,
        );
    }

    let mut rebuild_from_rest = false;
    let mut consecutive_failures = 0_u32;
    let mut failure_logger = PublicWssCycleFailureLogger::new("bitget-wss-book-ticker");
    loop {
        let cycle = run_bitget_wss_book_ticker_monitor_cycle(
            &options,
            state.clone(),
            &symbol_scope,
            rebuild_from_rest,
        );
        match cycle {
            Ok(()) if options.once => return Ok(()),
            Ok(()) => {
                consecutive_failures = 0;
                failure_logger.record_success();
            }
            Err(error) if options.once => return Err(error),
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let reconnect_backoff = public_wss_reconnect_backoff(
                    options.reconnect_delay_secs,
                    consecutive_failures,
                );
                failure_logger.record_failure(&error, consecutive_failures, reconnect_backoff);
                rebuild_from_rest = true;
                thread::sleep(reconnect_backoff);
                continue;
            }
        }
        rebuild_from_rest = true;
        thread::sleep(public_wss_reconnect_backoff(
            options.reconnect_delay_secs,
            consecutive_failures,
        ));
    }
}

pub(crate) fn run_bitget_wss_book_ticker_monitor_cycle(
    options: &BitgetWssBookTickerMonitorOptions,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
    symbol_scope: &str,
    rebuild_from_rest: bool,
) -> RuntimeResult<()> {
    if rebuild_from_rest {
        state
            .write()
            .expect("Bitget WSS monitor state lock poisoned")
            .begin_rest_rebuild();
    }
    let mut market_state = match bootstrap_bitget_wss_book_ticker(symbol_scope, options.market) {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            state
                .write()
                .expect("Bitget WSS monitor state lock poisoned")
                .record_failure(
                    format!("REST snapshot bootstrap/rebuild failed: {error}"),
                    false,
                );
            return Err(error);
        }
    };
    {
        let mut snapshot = state
            .write()
            .expect("Bitget WSS monitor state lock poisoned");
        snapshot.stream_url = market_state.stream_url.clone();
        snapshot.symbol = symbol_scope.to_owned();
        snapshot.market = options.market.as_str().to_owned();
        snapshot.rows.clear();
        snapshot.latest_quote = None;
        snapshot.total_rows = 0;
        for update in &market_state.rest_updates {
            snapshot.record_update(update);
        }
    }

    let connected_at = current_utc_timestamp()?;
    for coordinator in market_state.coordinators.values_mut() {
        let update = coordinator.apply(HybridMarketDataInput::WssConnected {
            occurred_at: connected_at,
            ingested_at: connected_at,
        })?;
        state
            .write()
            .expect("Bitget WSS monitor state lock poisoned")
            .record_update(&update);
    }

    let text_client = PublicJsonWssTextStreamClient::new(
        VenueId::new(options.market.venue_id())?,
        market_state.stream_url.clone(),
        "Bitget",
    )?;
    let max_text_messages = if options.once {
        options.updates
    } else {
        usize::MAX
    };
    let subscribe_payloads = market_state.subscribe_args.clone();
    let mut observed_wss_event = false;
    let mut observer_error = None;
    let read_result = text_client.read_live_text_messages_observed_many(
        &subscribe_payloads,
        max_text_messages,
        |raw_json, ingested_at| match apply_bitget_wss_book_ticker_text(
            raw_json,
            ingested_at,
            options.market,
            &mut market_state,
        ) {
            Ok(Some(update)) => {
                observed_wss_event = true;
                let keep_going = !update.fail_closed;
                state
                    .write()
                    .expect("Bitget WSS monitor state lock poisoned")
                    .record_update(&update);
                keep_going
            }
            Ok(None) => true,
            Err(error) => {
                observer_error = Some(error.to_string());
                state
                    .write()
                    .expect("Bitget WSS monitor state lock poisoned")
                    .record_failure(error.to_string(), false);
                false
            }
        },
    );

    if let Some(error) = observer_error {
        return Err(RuntimeError::LiveMarketData { message: error });
    }
    match read_result {
        Ok(()) => {
            if !options.once {
                state
                    .write()
                    .expect("Bitget WSS monitor state lock poisoned")
                    .record_stream_end_with_label_and_observed(
                        "Bitget public WSS",
                        observed_wss_event,
                    );
            }
            Ok(())
        }
        Err(error) => {
            state
                .write()
                .expect("Bitget WSS monitor state lock poisoned")
                .record_wss_read_error(error.to_string(), observed_wss_event);
            public_wss_monitor_cycle_result_after_read_error(error, observed_wss_event)
        }
    }
}

/// 运行 Aster 公开 `bookTicker` WSS 常驻任务。
pub fn run_aster_wss_book_ticker_monitor(
    options: AsterWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    validate_aster_wss_probe_options(&options)?;
    let symbol_scope = normalize_aster_wss_symbol_scope(&options.symbol)?;
    let state = Arc::new(RwLock::new(
        PublicTopOfBookMonitorSnapshot::empty_with_market(
            &symbol_scope,
            options.market.as_str(),
            "pending-rest-bootstrap",
        ),
    ));
    if !options.once {
        start_aster_wss_book_ticker_http_api(&options.bind_addr, state.clone())?;
        println!(
            "aster-wss-book-ticker: api=http://{} symbol_scope={} market={} reconnect_delay_secs={} mutable_execution_started=false",
            options.bind_addr,
            symbol_scope,
            options.market.as_str(),
            options.reconnect_delay_secs,
        );
    }

    let mut rebuild_from_rest = false;
    let mut consecutive_failures = 0_u32;
    let mut failure_logger = PublicWssCycleFailureLogger::new("aster-wss-book-ticker");
    loop {
        let cycle = run_aster_wss_book_ticker_monitor_cycle(
            &options,
            state.clone(),
            &symbol_scope,
            rebuild_from_rest,
        );
        match cycle {
            Ok(()) if options.once => return Ok(()),
            Ok(()) => {
                consecutive_failures = 0;
                failure_logger.record_success();
            }
            Err(error) if options.once => return Err(error),
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let reconnect_backoff = public_wss_reconnect_backoff(
                    options.reconnect_delay_secs,
                    consecutive_failures,
                );
                failure_logger.record_failure(&error, consecutive_failures, reconnect_backoff);
                rebuild_from_rest = true;
                thread::sleep(reconnect_backoff);
                continue;
            }
        }
        rebuild_from_rest = true;
        thread::sleep(public_wss_reconnect_backoff(
            options.reconnect_delay_secs,
            consecutive_failures,
        ));
    }
}

pub(crate) fn run_aster_wss_book_ticker_monitor_cycle(
    options: &AsterWssBookTickerMonitorOptions,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
    symbol_scope: &str,
    rebuild_from_rest: bool,
) -> RuntimeResult<()> {
    if rebuild_from_rest {
        state
            .write()
            .expect("Aster WSS monitor state lock poisoned")
            .begin_rest_rebuild();
    }
    let mut market_state = match bootstrap_aster_wss_book_ticker(symbol_scope, options.market) {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            state
                .write()
                .expect("Aster WSS monitor state lock poisoned")
                .record_failure(
                    format!("REST snapshot bootstrap/rebuild failed: {error}"),
                    false,
                );
            return Err(error);
        }
    };
    {
        let mut snapshot = state
            .write()
            .expect("Aster WSS monitor state lock poisoned");
        snapshot.stream_url = market_state.stream_url.clone();
        snapshot.symbol = symbol_scope.to_owned();
        snapshot.market = options.market.as_str().to_owned();
        snapshot.rows.clear();
        snapshot.latest_quote = None;
        snapshot.total_rows = 0;
        for update in &market_state.rest_updates {
            snapshot.record_update(update);
        }
    }

    let connected_at = current_utc_timestamp()?;
    for coordinator in market_state.coordinators.values_mut() {
        let update = coordinator.apply(HybridMarketDataInput::WssConnected {
            occurred_at: connected_at,
            ingested_at: connected_at,
        })?;
        state
            .write()
            .expect("Aster WSS monitor state lock poisoned")
            .record_update(&update);
    }

    let text_client = PublicJsonWssTextStreamClient::new(
        VenueId::new(options.market.venue_id())?,
        market_state.stream_url.clone(),
        "Aster",
    )?;
    let max_text_messages = if options.once {
        options.updates
    } else {
        usize::MAX
    };
    let mut observed_wss_event = false;
    let mut observer_error = None;
    let read_result = text_client.read_live_text_messages_without_subscribe_observed(
        max_text_messages,
        |raw_json, ingested_at| match apply_aster_wss_book_ticker_text(
            raw_json,
            ingested_at,
            options.market,
            &mut market_state,
        ) {
            Ok(Some(update)) => {
                observed_wss_event = true;
                let keep_going = !update.fail_closed;
                state
                    .write()
                    .expect("Aster WSS monitor state lock poisoned")
                    .record_update(&update);
                keep_going
            }
            Ok(None) => true,
            Err(error) => {
                observer_error = Some(error.to_string());
                state
                    .write()
                    .expect("Aster WSS monitor state lock poisoned")
                    .record_failure(error.to_string(), false);
                false
            }
        },
    );

    if let Some(error) = observer_error {
        return Err(RuntimeError::LiveMarketData { message: error });
    }
    match read_result {
        Ok(()) => {
            if !options.once {
                state
                    .write()
                    .expect("Aster WSS monitor state lock poisoned")
                    .record_stream_end_with_label_and_observed(
                        "Aster public WSS",
                        observed_wss_event,
                    );
            }
            Ok(())
        }
        Err(error) => {
            state
                .write()
                .expect("Aster WSS monitor state lock poisoned")
                .record_wss_read_error(error.to_string(), observed_wss_event);
            public_wss_monitor_cycle_result_after_read_error(error, observed_wss_event)
        }
    }
}

/// 运行 Hyperliquid 公开顶层盘口 WSS 常驻任务。
pub fn run_hyperliquid_wss_book_ticker_monitor(
    options: HyperliquidWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    validate_hyperliquid_wss_probe_options(&options)?;
    let symbol_scope = normalize_hyperliquid_wss_symbol_scope(&options.symbol)?;
    let state = Arc::new(RwLock::new(
        PublicTopOfBookMonitorSnapshot::empty_with_market(
            &symbol_scope,
            options.market.as_str(),
            "pending-rest-bootstrap",
        ),
    ));
    if !options.once {
        start_hyperliquid_wss_book_ticker_http_api(&options.bind_addr, state.clone())?;
        println!(
            "hyperliquid-wss-book-ticker: api=http://{} symbol_scope={} market={} reconnect_delay_secs={} mutable_execution_started=false",
            options.bind_addr,
            symbol_scope,
            options.market.as_str(),
            options.reconnect_delay_secs,
        );
    }

    let mut rebuild_from_rest = false;
    let mut consecutive_failures = 0_u32;
    let mut failure_logger = PublicWssCycleFailureLogger::new("hyperliquid-wss-book-ticker");
    loop {
        let cycle = run_hyperliquid_wss_book_ticker_monitor_cycle(
            &options,
            state.clone(),
            &symbol_scope,
            rebuild_from_rest,
        );
        match cycle {
            Ok(()) if options.once => return Ok(()),
            Ok(()) => {
                consecutive_failures = 0;
                failure_logger.record_success();
            }
            Err(error) if options.once => return Err(error),
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let reconnect_backoff = public_wss_reconnect_backoff(
                    options.reconnect_delay_secs,
                    consecutive_failures,
                );
                failure_logger.record_failure(&error, consecutive_failures, reconnect_backoff);
                rebuild_from_rest = true;
                thread::sleep(reconnect_backoff);
                continue;
            }
        }
        rebuild_from_rest = true;
        thread::sleep(public_wss_reconnect_backoff(
            options.reconnect_delay_secs,
            consecutive_failures,
        ));
    }
}

pub(crate) fn run_hyperliquid_wss_book_ticker_monitor_cycle(
    options: &HyperliquidWssBookTickerMonitorOptions,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
    symbol_scope: &str,
    rebuild_from_rest: bool,
) -> RuntimeResult<()> {
    if rebuild_from_rest {
        state
            .write()
            .expect("Hyperliquid WSS monitor state lock poisoned")
            .begin_rest_rebuild();
    }
    let mut market_state = match bootstrap_hyperliquid_wss_book_ticker(symbol_scope, options.market)
    {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            state
                .write()
                .expect("Hyperliquid WSS monitor state lock poisoned")
                .record_failure(
                    format!("REST snapshot bootstrap/rebuild failed: {error}"),
                    false,
                );
            return Err(error);
        }
    };
    {
        let mut snapshot = state
            .write()
            .expect("Hyperliquid WSS monitor state lock poisoned");
        snapshot.stream_url = market_state.stream_url.clone();
        snapshot.symbol = symbol_scope.to_owned();
        snapshot.market = options.market.as_str().to_owned();
        snapshot.rows.clear();
        snapshot.latest_quote = None;
        snapshot.total_rows = 0;
        for update in &market_state.rest_updates {
            snapshot.record_update(update);
        }
    }

    let connected_at = current_utc_timestamp()?;
    for coordinator in market_state.coordinators.values_mut() {
        let update = coordinator.apply(HybridMarketDataInput::WssConnected {
            occurred_at: connected_at,
            ingested_at: connected_at,
        })?;
        state
            .write()
            .expect("Hyperliquid WSS monitor state lock poisoned")
            .record_update(&update);
    }

    let text_client = PublicJsonWssTextStreamClient::new(
        VenueId::new(options.market.venue_id())?,
        market_state.stream_url.clone(),
        "Hyperliquid",
    )?;
    let subscribe_payloads = market_state.subscribe_args.clone();
    let max_text_messages = if options.once {
        options.updates
    } else {
        usize::MAX
    };
    let mut observed_wss_event = false;
    let mut observer_error = None;
    let read_result = text_client.read_live_text_messages_observed_many_with_subscribe_delay(
        &subscribe_payloads,
        max_text_messages,
        Duration::from_millis(HYPERLIQUID_WSS_SUBSCRIBE_DELAY_MS),
        |raw_json, ingested_at| match apply_hyperliquid_wss_book_ticker_text(
            raw_json,
            ingested_at,
            options.market,
            &mut market_state,
        ) {
            Ok(Some(update)) => {
                observed_wss_event = true;
                let keep_going = !update.fail_closed;
                state
                    .write()
                    .expect("Hyperliquid WSS monitor state lock poisoned")
                    .record_update(&update);
                keep_going
            }
            Ok(None) => true,
            Err(error) => {
                observer_error = Some(error.to_string());
                state
                    .write()
                    .expect("Hyperliquid WSS monitor state lock poisoned")
                    .record_failure(error.to_string(), false);
                false
            }
        },
    );

    if let Some(error) = observer_error {
        return Err(RuntimeError::LiveMarketData { message: error });
    }
    match read_result {
        Ok(()) => {
            if !options.once {
                state
                    .write()
                    .expect("Hyperliquid WSS monitor state lock poisoned")
                    .record_stream_end_with_label_and_observed(
                        "Hyperliquid public WSS",
                        observed_wss_event,
                    );
            }
            Ok(())
        }
        Err(error) => {
            state
                .write()
                .expect("Hyperliquid WSS monitor state lock poisoned")
                .record_wss_read_error(error.to_string(), observed_wss_event);
            public_wss_monitor_cycle_result_after_read_error(error, observed_wss_event)
        }
    }
}

/// 运行 Lighter 公开 `ticker/{MARKET_INDEX}` WSS 常驻任务。
pub fn run_lighter_wss_book_ticker_monitor(
    options: LighterWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    validate_lighter_wss_probe_options(&options)?;
    let symbol_scope = normalize_lighter_wss_symbol_scope(&options.symbol)?;
    let state = Arc::new(RwLock::new(
        PublicTopOfBookMonitorSnapshot::empty_with_market(
            &symbol_scope,
            options.market.as_str(),
            "pending-rest-bootstrap",
        ),
    ));
    if !options.once {
        start_lighter_wss_book_ticker_http_api(&options.bind_addr, state.clone())?;
        println!(
            "lighter-wss-book-ticker: api=http://{} symbol_scope={} market={} reconnect_delay_secs={} mutable_execution_started=false",
            options.bind_addr,
            symbol_scope,
            options.market.as_str(),
            options.reconnect_delay_secs,
        );
    }

    let mut rebuild_from_rest = false;
    let mut consecutive_failures = 0_u32;
    let mut failure_logger = PublicWssCycleFailureLogger::new("lighter-wss-book-ticker");
    loop {
        let cycle = run_lighter_wss_book_ticker_monitor_cycle(
            &options,
            state.clone(),
            &symbol_scope,
            rebuild_from_rest,
        );
        match cycle {
            Ok(()) if options.once => return Ok(()),
            Ok(()) => {
                consecutive_failures = 0;
                failure_logger.record_success();
            }
            Err(error) if options.once => return Err(error),
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let reconnect_backoff = public_wss_reconnect_backoff(
                    options.reconnect_delay_secs,
                    consecutive_failures,
                );
                failure_logger.record_failure(&error, consecutive_failures, reconnect_backoff);
                rebuild_from_rest = true;
                thread::sleep(reconnect_backoff);
                continue;
            }
        }
        rebuild_from_rest = true;
        thread::sleep(public_wss_reconnect_backoff(
            options.reconnect_delay_secs,
            consecutive_failures,
        ));
    }
}

pub(crate) fn run_lighter_wss_book_ticker_monitor_cycle(
    options: &LighterWssBookTickerMonitorOptions,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
    symbol_scope: &str,
    rebuild_from_rest: bool,
) -> RuntimeResult<()> {
    if rebuild_from_rest {
        state
            .write()
            .expect("Lighter WSS monitor state lock poisoned")
            .begin_rest_rebuild();
    }
    let mut market_state = match bootstrap_lighter_wss_book_ticker(symbol_scope, options.market) {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            state
                .write()
                .expect("Lighter WSS monitor state lock poisoned")
                .record_failure(
                    format!("REST metadata bootstrap/rebuild failed: {error}"),
                    false,
                );
            return Err(error);
        }
    };
    {
        let mut snapshot = state
            .write()
            .expect("Lighter WSS monitor state lock poisoned");
        snapshot.stream_url = market_state.stream_url.clone();
        snapshot.symbol = symbol_scope.to_owned();
        snapshot.market = options.market.as_str().to_owned();
        snapshot.rows.clear();
        snapshot.latest_quote = None;
        snapshot.total_rows = 0;
        for update in &market_state.rest_updates {
            snapshot.record_update(update);
        }
    }

    let connected_at = current_utc_timestamp()?;
    for coordinator in market_state.coordinators.values_mut() {
        let update = coordinator.apply(HybridMarketDataInput::WssConnected {
            occurred_at: connected_at,
            ingested_at: connected_at,
        })?;
        state
            .write()
            .expect("Lighter WSS monitor state lock poisoned")
            .record_update(&update);
    }

    let text_client = PublicJsonWssTextStreamClient::new(
        VenueId::new(options.market.venue_id())?,
        market_state.stream_url.clone(),
        "Lighter",
    )?;
    let subscribe_payloads = market_state.subscribe_args.clone();
    let max_text_messages = if options.once {
        options.updates
    } else {
        usize::MAX
    };
    let mut observed_wss_event = false;
    let mut observer_error = None;
    let read_result = text_client.read_live_text_messages_observed_many_with_subscribe_delay(
        &subscribe_payloads,
        max_text_messages,
        Duration::from_millis(LIGHTER_WSS_SUBSCRIBE_DELAY_MS),
        |raw_json, ingested_at| match apply_lighter_wss_book_ticker_text(
            raw_json,
            ingested_at,
            options.market,
            &mut market_state,
        ) {
            Ok(Some(update)) => {
                observed_wss_event = true;
                let keep_going = !update.fail_closed;
                state
                    .write()
                    .expect("Lighter WSS monitor state lock poisoned")
                    .record_update(&update);
                keep_going
            }
            Ok(None) => true,
            Err(error) => {
                observer_error = Some(error.to_string());
                state
                    .write()
                    .expect("Lighter WSS monitor state lock poisoned")
                    .record_failure(error.to_string(), false);
                false
            }
        },
    );

    if let Some(error) = observer_error {
        return Err(RuntimeError::LiveMarketData { message: error });
    }
    match read_result {
        Ok(()) => {
            if !options.once {
                state
                    .write()
                    .expect("Lighter WSS monitor state lock poisoned")
                    .record_stream_end_with_label_and_observed(
                        "Lighter public WSS",
                        observed_wss_event,
                    );
            }
            Ok(())
        }
        Err(error) => {
            state
                .write()
                .expect("Lighter WSS monitor state lock poisoned")
                .record_wss_read_error(error.to_string(), observed_wss_event);
            public_wss_monitor_cycle_result_after_read_error(error, observed_wss_event)
        }
    }
}

/// 运行 Gate 公开 `futures.book_ticker` WSS 常驻任务。
pub fn run_gate_wss_book_ticker_monitor(
    options: GateWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    validate_gate_wss_probe_options(&options)?;
    let symbol_scope = normalize_gate_wss_symbol_scope(&options.symbol)?;
    let state = Arc::new(RwLock::new(
        PublicTopOfBookMonitorSnapshot::empty_with_market(
            &symbol_scope,
            options.market.as_str(),
            "pending-rest-bootstrap",
        ),
    ));
    if !options.once {
        start_gate_wss_book_ticker_http_api(&options.bind_addr, state.clone())?;
        println!(
            "gate-wss-book-ticker: api=http://{} symbol_scope={} market={} reconnect_delay_secs={} mutable_execution_started=false",
            options.bind_addr,
            symbol_scope,
            options.market.as_str(),
            options.reconnect_delay_secs,
        );
    }

    let mut rebuild_from_rest = false;
    let mut consecutive_failures = 0_u32;
    let mut failure_logger = PublicWssCycleFailureLogger::new("gate-wss-book-ticker");
    loop {
        let cycle = run_gate_wss_book_ticker_monitor_cycle(
            &options,
            state.clone(),
            &symbol_scope,
            rebuild_from_rest,
        );
        match cycle {
            Ok(()) if options.once => return Ok(()),
            Ok(()) => {
                consecutive_failures = 0;
                failure_logger.record_success();
            }
            Err(error) if options.once => return Err(error),
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let reconnect_backoff = public_wss_reconnect_backoff(
                    options.reconnect_delay_secs,
                    consecutive_failures,
                );
                failure_logger.record_failure(&error, consecutive_failures, reconnect_backoff);
                rebuild_from_rest = true;
                thread::sleep(reconnect_backoff);
                continue;
            }
        }
        rebuild_from_rest = true;
        thread::sleep(public_wss_reconnect_backoff(
            options.reconnect_delay_secs,
            consecutive_failures,
        ));
    }
}

pub(crate) fn run_gate_wss_book_ticker_monitor_cycle(
    options: &GateWssBookTickerMonitorOptions,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
    symbol_scope: &str,
    rebuild_from_rest: bool,
) -> RuntimeResult<()> {
    if rebuild_from_rest {
        state
            .write()
            .expect("Gate WSS monitor state lock poisoned")
            .begin_rest_rebuild();
    }
    let mut market_state = match bootstrap_gate_wss_book_ticker(symbol_scope, options.market) {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            state
                .write()
                .expect("Gate WSS monitor state lock poisoned")
                .record_failure(
                    format!("REST ticker bootstrap/rebuild failed: {error}"),
                    false,
                );
            return Err(error);
        }
    };
    {
        let mut snapshot = state.write().expect("Gate WSS monitor state lock poisoned");
        snapshot.stream_url = market_state.stream_url.clone();
        snapshot.symbol = symbol_scope.to_owned();
        snapshot.market = options.market.as_str().to_owned();
        snapshot.rows.clear();
        snapshot.latest_quote = None;
        snapshot.total_rows = 0;
        for update in &market_state.rest_updates {
            snapshot.record_update(update);
        }
    }

    let connected_at = current_utc_timestamp()?;
    for coordinator in market_state.coordinators.values_mut() {
        let update = coordinator.apply(HybridMarketDataInput::WssConnected {
            occurred_at: connected_at,
            ingested_at: connected_at,
        })?;
        state
            .write()
            .expect("Gate WSS monitor state lock poisoned")
            .record_update(&update);
    }

    let text_client = PublicJsonWssTextStreamClient::new(
        VenueId::new(options.market.venue_id())?,
        market_state.stream_url.clone(),
        "Gate",
    )?;
    let subscribe_payloads = market_state.subscribe_args.clone();
    let max_text_messages = if options.once {
        options.updates
    } else {
        usize::MAX
    };
    let mut observed_wss_event = false;
    let mut observer_error = None;
    let read_result = text_client.read_live_text_messages_observed_many_with_subscribe_delay(
        &subscribe_payloads,
        max_text_messages,
        Duration::from_millis(GATE_WSS_SUBSCRIBE_DELAY_MS),
        |raw_json, ingested_at| match apply_gate_wss_book_ticker_text(
            raw_json,
            ingested_at,
            options.market,
            &mut market_state,
        ) {
            Ok(Some(update)) => {
                observed_wss_event = true;
                let keep_going = !update.fail_closed;
                state
                    .write()
                    .expect("Gate WSS monitor state lock poisoned")
                    .record_update(&update);
                keep_going
            }
            Ok(None) => true,
            Err(error) => {
                observer_error = Some(error.to_string());
                state
                    .write()
                    .expect("Gate WSS monitor state lock poisoned")
                    .record_failure(error.to_string(), false);
                false
            }
        },
    );

    if let Some(error) = observer_error {
        return Err(RuntimeError::LiveMarketData { message: error });
    }
    match read_result {
        Ok(()) => {
            if !options.once {
                state
                    .write()
                    .expect("Gate WSS monitor state lock poisoned")
                    .record_stream_end_with_label_and_observed(
                        "Gate public WSS",
                        observed_wss_event,
                    );
            }
            Ok(())
        }
        Err(error) => {
            state
                .write()
                .expect("Gate WSS monitor state lock poisoned")
                .record_wss_read_error(error.to_string(), observed_wss_event);
            public_wss_monitor_cycle_result_after_read_error(error, observed_wss_event)
        }
    }
}

pub(crate) struct PublicTopOfBookAllMarketState {
    pub(crate) venue_id: VenueId,
    pub(crate) stream_url: String,
    pub(crate) subscribe_args: Vec<String>,
    pub(crate) ignore_untracked_wss_symbols: bool,
    pub(crate) coordinators: BTreeMap<String, RestWssMarketDataCoordinator>,
    pub(crate) local_sequences: BTreeMap<String, u64>,
    pub(crate) last_exchange_update_ids: BTreeMap<String, u64>,
    pub(crate) rest_updates: Vec<HybridMarketDataUpdate>,
}

pub(crate) struct PublicTopOfBookRuntimeRaw {
    pub(crate) symbol: String,
    pub(crate) update_id: u64,
    pub(crate) best_bid: Price,
    pub(crate) best_ask: Price,
    pub(crate) bid_size: Quantity,
    pub(crate) ask_size: Quantity,
    pub(crate) observed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LighterOrderBookMetadataRow {
    pub(crate) market_id: String,
    pub(crate) symbol: String,
}

pub(crate) struct BybitTopOfBookRuntimeRaw {
    pub(crate) symbol: String,
    pub(crate) update_id: u64,
    pub(crate) best_bid: Option<Price>,
    pub(crate) best_ask: Option<Price>,
    pub(crate) bid_size: Option<Quantity>,
    pub(crate) ask_size: Option<Quantity>,
    pub(crate) observed_at: UtcTimestamp,
}

pub(crate) struct PublicWssTickerRuntimeRaw {
    pub(crate) symbol: String,
    pub(crate) venue_symbol: String,
    pub(crate) best_bid: Price,
    pub(crate) best_ask: Price,
    pub(crate) bid_size: Quantity,
    pub(crate) ask_size: Quantity,
    pub(crate) observed_at: UtcTimestamp,
}

pub(crate) fn bootstrap_binance_wss_book_ticker_all_market(
    symbol_scope: &str,
    market: BinancePublicMarket,
) -> RuntimeResult<PublicTopOfBookAllMarketState> {
    let symbol_scope = normalize_binance_wss_symbol_scope(symbol_scope)?;
    let venue_id = binance_public_wss_venue_id(market)?;
    let target_symbols =
        explicit_wss_symbol_scope_set(&symbol_scope, is_binance_wss_all_symbols_scope);
    let all_symbols_scope = target_symbols.is_none();
    let rest_url = match target_symbols.as_ref() {
        None => match market {
            BinancePublicMarket::Spot => binance_spot_book_ticker_all_url(),
            BinancePublicMarket::UsdmPerpetual => binance_usdm_book_ticker_all_url(),
        },
        Some(symbols) if symbols.len() == 1 => {
            let symbol = symbols
                .iter()
                .next()
                .expect("target symbol set is non-empty");
            match market {
                BinancePublicMarket::Spot => binance_spot_book_ticker_url(symbol),
                BinancePublicMarket::UsdmPerpetual => binance_usdm_book_ticker_url(symbol),
            }
        }
        Some(_) => match market {
            BinancePublicMarket::Spot => binance_spot_book_ticker_all_url(),
            BinancePublicMarket::UsdmPerpetual => binance_usdm_book_ticker_all_url(),
        },
    };
    let raw_rest_snapshot = fetch_public_json_with_curl(&rest_url)?;
    let rows = prepare_binance_wss_book_ticker_rest_rows(
        parse_book_ticker_rows(&raw_rest_snapshot, "binance bookTicker")?,
        &symbol_scope,
        all_symbols_scope,
    )?;
    if rows.is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS bookTicker REST bootstrap returned no rows for `{symbol_scope}`"
            ),
        });
    }

    let started_at = current_utc_timestamp()?;
    let mut coordinators = BTreeMap::new();
    let mut local_sequences = BTreeMap::new();
    let mut rest_updates = Vec::with_capacity(rows.len());
    let mut symbols = Vec::with_capacity(rows.len());
    for row in rows {
        let symbol = row.symbol.clone();
        let instrument = binance_public_wss_instrument(&symbol, market)?;
        let mut quote = binance_wss_rest_quote_from_row(&row, &venue_id, &instrument, started_at)?;
        let sequence = 1_u64;
        quote.source_sequence = Some(sequence.to_string());
        let mut coordinator = RestWssMarketDataCoordinator::new(
            venue_id.clone(),
            instrument.instrument_id.clone(),
            started_at,
            PUBLIC_WSS_MONITOR_MAX_AGE_MS,
        )?;
        let update = coordinator.apply(HybridMarketDataInput::RestSnapshot { quote })?;
        local_sequences.insert(symbol.clone(), sequence);
        coordinators.insert(symbol.clone(), coordinator);
        rest_updates.push(update);
        symbols.push(symbol);
    }

    let stream_url =
        binance_wss_book_ticker_all_market_stream_url(market, &symbols, all_symbols_scope)?;
    let ignore_untracked_wss_symbols = all_symbols_scope
        || binance_usdm_wss_book_ticker_uses_all_market_stream(market, &symbols, all_symbols_scope);
    Ok(PublicTopOfBookAllMarketState {
        venue_id,
        stream_url,
        subscribe_args: Vec::new(),
        ignore_untracked_wss_symbols,
        coordinators,
        local_sequences,
        last_exchange_update_ids: BTreeMap::new(),
        rest_updates,
    })
}

pub(crate) fn prepare_binance_wss_book_ticker_rest_rows(
    rows: Vec<MonitorBookTickerRow>,
    symbol_scope: &str,
    all_symbols_scope: bool,
) -> RuntimeResult<Vec<MonitorBookTickerRow>> {
    let normalized_scope = normalize_binance_wss_symbol_scope(symbol_scope)?;
    let all_symbols_scope =
        all_symbols_scope || is_binance_wss_all_symbols_scope(&normalized_scope);
    let target_symbols =
        explicit_wss_symbol_scope_set(&normalized_scope, is_binance_wss_all_symbols_scope);
    let mut prepared = Vec::with_capacity(rows.len());
    for mut row in rows {
        match validate_binance_public_wss_symbol(&row.symbol) {
            Ok(symbol)
                if wss_symbol_in_scope(&symbol, all_symbols_scope, target_symbols.as_ref()) =>
            {
                row.symbol = symbol;
                prepared.push(row);
            }
            Ok(_) => {}
            Err(_) if all_symbols_scope || target_symbols.is_some() => {}
            Err(error) => return Err(error),
        }
    }
    prepared.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    prepared.dedup_by(|left, right| left.symbol == right.symbol);
    if all_symbols_scope {
        if let Some(index) = prepared.iter().position(|row| row.symbol == BASIS_SYMBOL) {
            let row = prepared.remove(index);
            prepared.insert(0, row);
        }
    }
    ensure_wss_requested_symbols_present(
        "Binance WSS bookTicker REST bootstrap",
        target_symbols.as_ref(),
        prepared.iter().map(|row| row.symbol.clone()).collect(),
    )?;
    Ok(prepared)
}

pub(crate) fn bootstrap_bybit_wss_book_ticker_all_market(
    symbol_scope: &str,
    market: BybitPublicMarket,
) -> RuntimeResult<PublicTopOfBookAllMarketState> {
    let symbol_scope = normalize_bybit_wss_symbol_scope(symbol_scope)?;
    let venue_id = bybit_public_wss_venue_id(market)?;
    let all_symbols_scope = is_bybit_wss_all_symbols_scope(&symbol_scope);
    let raw_rest_snapshot = match market {
        BybitPublicMarket::Spot => fetch_public_json_with_curl(&bybit_spot_tickers_url())?,
        BybitPublicMarket::LinearPerpetual => {
            fetch_public_json_with_curl(&bybit_linear_tickers_url())?
        }
    };
    let rows = match market {
        BybitPublicMarket::Spot => parse_bybit_spot_ticker_rows(&raw_rest_snapshot)?,
        BybitPublicMarket::LinearPerpetual => parse_bybit_linear_ticker_rows(&raw_rest_snapshot)?
            .into_iter()
            .map(|row| MonitorBookTickerRow {
                symbol: row.symbol,
                bid_price: row.bid_price,
                bid_qty: row.bid_qty,
                ask_price: row.ask_price,
                ask_qty: row.ask_qty,
                bid_depth: row.bid_depth,
                ask_depth: row.ask_depth,
            })
            .collect(),
    };
    let rows = prepare_bybit_wss_book_ticker_rest_rows(rows, &symbol_scope, all_symbols_scope)?;
    if rows.is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Bybit WSS orderbook REST bootstrap returned no rows for `{symbol_scope}`"
            ),
        });
    }
    let (rows, subscribe_args) =
        bybit_wss_tracked_rest_rows_and_subscribe_topics(rows, market, all_symbols_scope);

    let started_at = current_utc_timestamp()?;
    let mut coordinators = BTreeMap::new();
    let mut local_sequences = BTreeMap::new();
    let mut rest_updates = Vec::with_capacity(rows.len());
    for row in rows {
        let symbol = row.symbol.clone();
        let instrument = bybit_public_wss_instrument(&symbol, market)?;
        let mut quote =
            bybit_wss_rest_quote_from_row(&row, &venue_id, &instrument, market, started_at)?;
        let sequence = 1_u64;
        quote.source_sequence = Some(sequence.to_string());
        let mut coordinator = RestWssMarketDataCoordinator::new(
            venue_id.clone(),
            instrument.instrument_id.clone(),
            started_at,
            PUBLIC_WSS_MONITOR_MAX_AGE_MS,
        )?;
        let update = coordinator.apply(HybridMarketDataInput::RestSnapshot { quote })?;
        local_sequences.insert(symbol.clone(), sequence);
        coordinators.insert(symbol.clone(), coordinator);
        rest_updates.push(update);
    }

    Ok(PublicTopOfBookAllMarketState {
        venue_id,
        stream_url: bybit_wss_book_ticker_public_stream_url(market),
        subscribe_args,
        ignore_untracked_wss_symbols: all_symbols_scope,
        coordinators,
        local_sequences,
        last_exchange_update_ids: BTreeMap::new(),
        rest_updates,
    })
}

pub(crate) fn prepare_bybit_wss_book_ticker_rest_rows(
    rows: Vec<MonitorBookTickerRow>,
    symbol_scope: &str,
    all_symbols_scope: bool,
) -> RuntimeResult<Vec<MonitorBookTickerRow>> {
    let normalized_scope = normalize_bybit_wss_symbol_scope(symbol_scope)?;
    let all_symbols_scope = all_symbols_scope || is_bybit_wss_all_symbols_scope(&normalized_scope);
    let target_symbols =
        explicit_wss_symbol_scope_set(&normalized_scope, is_bybit_wss_all_symbols_scope);
    let broad_explicit_scope = target_symbols
        .as_ref()
        .is_some_and(|symbols| symbols.len() > BYBIT_LINEAR_WSS_ORDERBOOK_TOPIC_SCOPE_LIMIT);
    let mut prepared = Vec::with_capacity(rows.len());
    for mut row in rows {
        match validate_bybit_public_wss_symbol(&row.symbol) {
            Ok(symbol)
                if wss_symbol_in_scope(&symbol, all_symbols_scope, target_symbols.as_ref()) =>
            {
                if !monitor_book_ticker_row_has_usable_top_of_book(&row) {
                    if all_symbols_scope || broad_explicit_scope {
                        continue;
                    }
                    return Err(RuntimeError::LiveMarketData {
                        message: format!(
                            "Bybit WSS orderbook REST bootstrap row for `{symbol}` has empty top-of-book field"
                        ),
                    });
                }
                row.symbol = symbol;
                prepared.push(row);
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    prepared.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    prepared.dedup_by(|left, right| left.symbol == right.symbol);
    if !broad_explicit_scope {
        ensure_wss_requested_symbols_present(
            "Bybit WSS orderbook REST bootstrap",
            target_symbols.as_ref(),
            prepared.iter().map(|row| row.symbol.clone()).collect(),
        )?;
    }
    Ok(prepared)
}

pub(crate) fn monitor_book_ticker_row_has_usable_top_of_book(row: &MonitorBookTickerRow) -> bool {
    [
        row.bid_price.as_str(),
        row.bid_qty.as_str(),
        row.ask_price.as_str(),
        row.ask_qty.as_str(),
    ]
    .into_iter()
    .all(|value| !value.trim().is_empty())
}

pub(crate) fn bootstrap_okx_wss_book_ticker(
    symbol_scope: &str,
    market: OkxPublicWssMarket,
) -> RuntimeResult<PublicTopOfBookAllMarketState> {
    let symbol_scope = normalize_okx_wss_symbol_scope(symbol_scope)?;
    let all_symbols_scope = is_okx_wss_all_symbols_scope(&symbol_scope);
    let venue_id = VenueId::new(market.venue_id())?;
    let raw_rest_snapshot = fetch_public_json_with_curl(&okx_tickers_url(market.as_str()))?;
    let rows = prepare_okx_wss_book_ticker_rest_rows(
        parse_okx_ticker_rows(&raw_rest_snapshot, "okx wss bootstrap tickers")?,
        &symbol_scope,
        market,
        all_symbols_scope,
    )?;
    if rows.is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: format!("OKX WSS REST bootstrap returned no tickers for `{symbol_scope}`"),
        });
    }
    let observed_at = current_utc_timestamp()?;
    let mut coordinators = BTreeMap::new();
    let mut local_sequences = BTreeMap::new();
    let mut rest_updates = Vec::with_capacity(rows.len());
    let mut subscribe_symbols = Vec::with_capacity(rows.len());
    for (symbol, row) in rows {
        let instrument_id = okx_public_wss_instrument_id(&symbol, market)?;
        let quote = okx_wss_rest_quote_from_row(
            &row,
            &symbol,
            &venue_id,
            instrument_id.clone(),
            observed_at,
        )?;
        let mut coordinator = RestWssMarketDataCoordinator::new(
            venue_id.clone(),
            instrument_id,
            observed_at,
            PUBLIC_WSS_MONITOR_MAX_AGE_MS,
        )?;
        let update = coordinator.apply(HybridMarketDataInput::RestSnapshot { quote })?;
        local_sequences.insert(symbol.clone(), 1_u64);
        coordinators.insert(symbol.clone(), coordinator);
        rest_updates.push(update);
        subscribe_symbols.push(symbol);
    }
    Ok(PublicTopOfBookAllMarketState {
        venue_id,
        stream_url: OKX_PUBLIC_WSS_URL.to_owned(),
        subscribe_args: okx_wss_ticker_subscribe_payloads(&subscribe_symbols, market),
        ignore_untracked_wss_symbols: all_symbols_scope,
        coordinators,
        local_sequences,
        last_exchange_update_ids: BTreeMap::new(),
        rest_updates,
    })
}

pub(crate) fn bootstrap_bitget_wss_book_ticker(
    symbol_scope: &str,
    market: BitgetPublicWssMarket,
) -> RuntimeResult<PublicTopOfBookAllMarketState> {
    let symbol_scope = normalize_bitget_wss_symbol_scope(symbol_scope)?;
    let all_symbols_scope = is_bitget_wss_all_symbols_scope(&symbol_scope);
    let venue_id = VenueId::new(market.venue_id())?;
    let raw_rest_snapshot = match market {
        BitgetPublicWssMarket::Spot => fetch_public_json_with_curl(&bitget_spot_tickers_url())?,
        BitgetPublicWssMarket::UsdtFutures => {
            fetch_public_json_with_curl(&bitget_usdt_futures_tickers_url())?
        }
    };
    let rows = match market {
        BitgetPublicWssMarket::Spot => parse_bitget_spot_ticker_rows(&raw_rest_snapshot)?
            .into_iter()
            .collect::<Vec<_>>(),
        BitgetPublicWssMarket::UsdtFutures => {
            parse_bitget_usdt_futures_ticker_rows_for_wss_bootstrap(&raw_rest_snapshot)?
        }
    };
    let rows =
        prepare_bitget_wss_book_ticker_rest_rows(rows, &symbol_scope, market, all_symbols_scope)?;
    if rows.is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: format!("Bitget WSS REST bootstrap returned no tickers for `{symbol_scope}`"),
        });
    }
    let observed_at = current_utc_timestamp()?;
    let mut coordinators = BTreeMap::new();
    let mut local_sequences = BTreeMap::new();
    let mut rest_updates = Vec::with_capacity(rows.len());
    let mut subscribe_symbols = Vec::with_capacity(rows.len());
    for row in rows {
        let symbol = row.symbol.clone();
        let instrument_id = bitget_public_wss_instrument_id(&symbol, market)?;
        let quote = bitget_wss_rest_quote_from_row(
            &row,
            &venue_id,
            instrument_id.clone(),
            market,
            observed_at,
        )?;
        let mut coordinator = RestWssMarketDataCoordinator::new(
            venue_id.clone(),
            instrument_id,
            observed_at,
            PUBLIC_WSS_MONITOR_MAX_AGE_MS,
        )?;
        let update = coordinator.apply(HybridMarketDataInput::RestSnapshot { quote })?;
        local_sequences.insert(symbol.clone(), 1_u64);
        coordinators.insert(symbol.clone(), coordinator);
        rest_updates.push(update);
        subscribe_symbols.push(symbol);
    }
    Ok(PublicTopOfBookAllMarketState {
        venue_id,
        stream_url: BITGET_PUBLIC_WSS_URL.to_owned(),
        subscribe_args: bitget_wss_ticker_subscribe_payloads_for_scope(
            &subscribe_symbols,
            market,
            all_symbols_scope,
        ),
        ignore_untracked_wss_symbols: all_symbols_scope,
        coordinators,
        local_sequences,
        last_exchange_update_ids: BTreeMap::new(),
        rest_updates,
    })
}

pub(crate) fn prepare_okx_wss_book_ticker_rest_rows(
    rows: Vec<OkxTickerRow>,
    symbol_scope: &str,
    market: OkxPublicWssMarket,
    all_symbols_scope: bool,
) -> RuntimeResult<Vec<(String, OkxTickerRow)>> {
    let normalized_scope = normalize_okx_wss_symbol_scope(symbol_scope)?;
    let all_symbols_scope = all_symbols_scope || is_okx_wss_all_symbols_scope(&normalized_scope);
    let target_symbols =
        explicit_wss_symbol_scope_set(&normalized_scope, is_okx_wss_all_symbols_scope);
    let mut prepared = Vec::with_capacity(rows.len());
    for row in rows {
        match okx_public_wss_symbol_from_inst_id(&row.inst_id, market) {
            Ok(symbol)
                if wss_symbol_in_scope(&symbol, all_symbols_scope, target_symbols.as_ref()) =>
            {
                if !okx_ticker_row_has_complete_top_of_book(&row) {
                    if all_symbols_scope {
                        continue;
                    }
                    return Err(RuntimeError::LiveMarketData {
                        message: format!(
                            "OKX WSS REST bootstrap ticker `{}` lacks a complete top-of-book quote",
                            row.inst_id
                        ),
                    });
                }
                prepared.push((symbol, row));
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    prepared.sort_by(|left, right| left.0.cmp(&right.0));
    prepared.dedup_by(|left, right| left.0 == right.0);
    if all_symbols_scope {
        if let Some(index) = prepared
            .iter()
            .position(|(symbol, _)| symbol == OKX_BASIS_SYMBOL)
        {
            let row = prepared.remove(index);
            prepared.insert(0, row);
        }
    }
    ensure_wss_requested_symbols_present(
        "OKX WSS REST bootstrap",
        target_symbols.as_ref(),
        prepared.iter().map(|(symbol, _)| symbol.clone()).collect(),
    )?;
    Ok(prepared)
}

pub(crate) fn okx_ticker_row_has_complete_top_of_book(row: &OkxTickerRow) -> bool {
    [
        row.bid_price.as_str(),
        row.bid_qty.as_str(),
        row.ask_price.as_str(),
        row.ask_qty.as_str(),
    ]
    .iter()
    .all(|value| !value.trim().is_empty())
}

pub(crate) fn prepare_bitget_wss_book_ticker_rest_rows(
    rows: Vec<MonitorBookTickerRow>,
    symbol_scope: &str,
    market: BitgetPublicWssMarket,
    all_symbols_scope: bool,
) -> RuntimeResult<Vec<MonitorBookTickerRow>> {
    let normalized_scope = normalize_bitget_wss_symbol_scope(symbol_scope)?;
    let all_symbols_scope = all_symbols_scope || is_bitget_wss_all_symbols_scope(&normalized_scope);
    let target_symbols =
        explicit_wss_symbol_scope_set(&normalized_scope, is_bitget_wss_all_symbols_scope);
    let mut prepared = Vec::with_capacity(rows.len());
    for mut row in rows {
        match normalize_cex_usdt_basis_symbol(&row.symbol, "Bitget") {
            Ok(symbol)
                if wss_symbol_in_scope(&symbol, all_symbols_scope, target_symbols.as_ref()) =>
            {
                if let Err(error) = bitget_public_wss_instrument_id(&symbol, market) {
                    if all_symbols_scope {
                        continue;
                    }
                    return Err(error);
                }
                if !monitor_book_ticker_row_has_usable_top_of_book(&row) {
                    if all_symbols_scope {
                        continue;
                    }
                    return Err(RuntimeError::LiveMarketData {
                        message: format!(
                            "Bitget WSS REST bootstrap row for `{symbol}` has empty top-of-book field"
                        ),
                    });
                }
                if let Err(error) = validate_bitget_wss_rest_top_of_book(&row) {
                    if all_symbols_scope {
                        continue;
                    }
                    return Err(error);
                }
                row.symbol = symbol;
                prepared.push(row);
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    prepared.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    prepared.dedup_by(|left, right| left.symbol == right.symbol);
    if all_symbols_scope {
        if let Some(index) = prepared
            .iter()
            .position(|row| row.symbol == BITGET_BASIS_SYMBOL)
        {
            let row = prepared.remove(index);
            prepared.insert(0, row);
        }
    }
    ensure_wss_requested_symbols_present(
        "Bitget WSS REST bootstrap",
        target_symbols.as_ref(),
        prepared.iter().map(|row| row.symbol.clone()).collect(),
    )?;
    Ok(prepared)
}

fn validate_bitget_wss_rest_top_of_book(row: &MonitorBookTickerRow) -> RuntimeResult<()> {
    Price::from_str(&row.bid_price).map_err(|error| RuntimeError::LiveMarketData {
        message: format!(
            "Bitget WSS REST bootstrap row for `{}` has invalid bid price `{}`: {error}",
            row.symbol, row.bid_price
        ),
    })?;
    Price::from_str(&row.ask_price).map_err(|error| RuntimeError::LiveMarketData {
        message: format!(
            "Bitget WSS REST bootstrap row for `{}` has invalid ask price `{}`: {error}",
            row.symbol, row.ask_price
        ),
    })?;
    Quantity::from_str(&row.bid_qty).map_err(|error| RuntimeError::LiveMarketData {
        message: format!(
            "Bitget WSS REST bootstrap row for `{}` has invalid bid size `{}`: {error}",
            row.symbol, row.bid_qty
        ),
    })?;
    Quantity::from_str(&row.ask_qty).map_err(|error| RuntimeError::LiveMarketData {
        message: format!(
            "Bitget WSS REST bootstrap row for `{}` has invalid ask size `{}`: {error}",
            row.symbol, row.ask_qty
        ),
    })?;
    Ok(())
}

pub(crate) fn parse_bitget_usdt_futures_ticker_rows_for_wss_bootstrap(
    input: &str,
) -> RuntimeResult<Vec<MonitorBookTickerRow>> {
    const SOURCE: &str = "bitget usdt-futures WSS REST bootstrap tickers";
    let top_fields = parse_json_object_value_slices(input)?;
    let code = required_json_value_string(&top_fields, "code", SOURCE)?;
    if code != "00000" {
        let msg = optional_json_value_string(&top_fields, "msg", SOURCE)?
            .unwrap_or_else(|| "unknown".to_owned());
        return Err(RuntimeError::LiveMarketData {
            message: format!("{SOURCE} returned code={code}, msg={msg}"),
        });
    }
    let data = top_fields
        .get("data")
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("{SOURCE} response is missing data"),
        })?;
    let mut rows = Vec::new();
    for object in json_object_slices(data)? {
        let fields = parse_json_object_value_slices(object)?;
        let symbol = match required_json_value_string(&fields, "symbol", SOURCE) {
            Ok(symbol) => symbol,
            Err(_) => continue,
        };
        let Some(bid_price) = optional_first_bitget_wss_ticker_value_string(
            &fields,
            &["bidPr", "bidPx", "bidPrice"],
            SOURCE,
        )?
        else {
            continue;
        };
        let Some(bid_qty) = optional_first_bitget_wss_ticker_value_string(
            &fields,
            &["bidSz", "bidSize", "bidQty"],
            SOURCE,
        )?
        else {
            continue;
        };
        let Some(ask_price) = optional_first_bitget_wss_ticker_value_string(
            &fields,
            &["askPr", "askPx", "askPrice"],
            SOURCE,
        )?
        else {
            continue;
        };
        let Some(ask_qty) = optional_first_bitget_wss_ticker_value_string(
            &fields,
            &["askSz", "askSize", "askQty"],
            SOURCE,
        )?
        else {
            continue;
        };
        rows.push(MonitorBookTickerRow {
            symbol,
            bid_price,
            bid_qty,
            ask_price,
            ask_qty,
            bid_depth: Vec::new(),
            ask_depth: Vec::new(),
        });
    }
    Ok(rows)
}

pub(crate) fn bootstrap_aster_wss_book_ticker(
    symbol_scope: &str,
    market: AsterPublicWssMarket,
) -> RuntimeResult<PublicTopOfBookAllMarketState> {
    let symbol_scope = normalize_aster_wss_symbol_scope(symbol_scope)?;
    let venue_id = VenueId::new(market.venue_id())?;
    let target_symbols =
        explicit_wss_symbol_scope_set(&symbol_scope, is_aster_wss_all_symbols_scope);
    let all_symbols_scope = target_symbols.is_none();
    let ignore_untracked_wss_symbols = all_symbols_scope
        || target_symbols
            .as_ref()
            .is_some_and(|symbols| symbols.len() > 1);
    let raw_rest_snapshot = match target_symbols.as_ref() {
        None => fetch_public_json_with_curl(&aster_futures_book_ticker_all_url())?,
        Some(symbols) if symbols.len() == 1 => {
            let symbol = symbols
                .iter()
                .next()
                .expect("target symbol set is non-empty");
            fetch_public_json_with_curl(&aster_futures_book_ticker_url(symbol))?
        }
        Some(_) => fetch_public_json_with_curl(&aster_futures_book_ticker_all_url())?,
    };
    let rows = prepare_aster_wss_book_ticker_rest_rows(
        parse_book_ticker_rows(&raw_rest_snapshot, "aster wss bootstrap bookTicker")?,
        &symbol_scope,
        all_symbols_scope,
    )?;
    if rows.is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: format!("Aster WSS REST bootstrap returned no rows for `{symbol_scope}`"),
        });
    }

    let observed_at = current_utc_timestamp()?;
    let mut coordinators = BTreeMap::new();
    let mut local_sequences = BTreeMap::new();
    let mut rest_updates = Vec::with_capacity(rows.len());
    for row in rows {
        let symbol = row.symbol.clone();
        let instrument_id = aster_public_wss_instrument_id(&symbol, market)?;
        let quote = public_wss_top_of_book_quote(
            &symbol,
            venue_id.clone(),
            instrument_id.clone(),
            PublicTopOfBookRuntimeRaw {
                symbol: symbol.clone(),
                update_id: 1,
                best_bid: Price::from_str(&row.bid_price)?,
                best_ask: Price::from_str(&row.ask_price)?,
                bid_size: Quantity::from_str(&row.bid_qty)?,
                ask_size: Quantity::from_str(&row.ask_qty)?,
                observed_at,
            },
            format!("aster:rest-bookTicker:{}:{}", row.symbol, observed_at),
        )?;
        let mut coordinator = RestWssMarketDataCoordinator::new(
            venue_id.clone(),
            instrument_id,
            observed_at,
            PUBLIC_WSS_MONITOR_MAX_AGE_MS,
        )?;
        let update = coordinator.apply(HybridMarketDataInput::RestSnapshot { quote })?;
        local_sequences.insert(symbol.clone(), 1_u64);
        coordinators.insert(symbol, coordinator);
        rest_updates.push(update);
    }

    Ok(PublicTopOfBookAllMarketState {
        venue_id,
        stream_url: aster_wss_book_ticker_stream_url(market, &symbol_scope)?,
        subscribe_args: Vec::new(),
        ignore_untracked_wss_symbols,
        coordinators,
        local_sequences,
        last_exchange_update_ids: BTreeMap::new(),
        rest_updates,
    })
}

pub(crate) fn bootstrap_hyperliquid_wss_book_ticker(
    symbol_scope: &str,
    market: HyperliquidPublicWssMarket,
) -> RuntimeResult<PublicTopOfBookAllMarketState> {
    let symbol_scope = normalize_hyperliquid_wss_symbol_scope(symbol_scope)?;
    let target_symbols =
        explicit_wss_symbol_scope_set(&symbol_scope, is_hyperliquid_wss_all_symbols_scope);
    let all_symbols_scope = target_symbols.is_none();
    let raw_rest_snapshot = fetch_public_json_post_with_curl(
        HYPERLIQUID_INFO_URL,
        &hyperliquid_info_request_body("metaAndAssetCtxs"),
    )?;
    let mut rows = parse_hyperliquid_perp_context_rows(&raw_rest_snapshot)?;
    if let Some(symbols) = target_symbols.as_ref() {
        rows.retain(|row| symbols.contains(&row.coin));
    }
    rows.sort_by(|left, right| left.coin.cmp(&right.coin));
    rows.dedup_by(|left, right| left.coin == right.coin);
    ensure_wss_requested_symbols_present(
        "Hyperliquid WSS REST bootstrap",
        target_symbols.as_ref(),
        rows.iter().map(|row| row.coin.clone()).collect(),
    )?;
    if rows.is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Hyperliquid WSS REST bootstrap returned no rows for `{symbol_scope}`"
            ),
        });
    }

    let observed_at = current_utc_timestamp()?;
    let venue_id = VenueId::new(market.venue_id())?;
    let mut coordinators = BTreeMap::new();
    let mut local_sequences = BTreeMap::new();
    let mut rest_updates = Vec::with_capacity(rows.len());
    let mut subscribe_args = Vec::with_capacity(rows.len());
    for row in rows {
        let instrument_id = hyperliquid_public_wss_instrument_id(&row.coin, market)?;
        let freshness =
            DataFreshness::new(observed_at, observed_at, PUBLIC_WSS_MONITOR_MAX_AGE_MS)?;
        let quote = MarketQuote {
            venue_id: venue_id.clone(),
            instrument_id: instrument_id.clone(),
            last_price: None,
            best_bid: Some(Price::from_str(&row.price)?),
            best_ask: Some(Price::from_str(&row.price)?),
            mark_price: Some(Price::from_str(&row.mark_price)?),
            index_price: Some(Price::from_str(&row.oracle_price)?),
            bid_size: None,
            ask_size: None,
            source_sequence: Some("1".to_owned()),
            source_event_id: Some(format!(
                "hyperliquid:rest-context:{}:{}",
                row.coin, observed_at
            )),
            freshness,
        };
        let mut coordinator = RestWssMarketDataCoordinator::new(
            venue_id.clone(),
            instrument_id,
            observed_at,
            PUBLIC_WSS_MONITOR_MAX_AGE_MS,
        )?;
        let update = coordinator.apply(HybridMarketDataInput::RestSnapshot { quote })?;
        local_sequences.insert(row.coin.clone(), 1_u64);
        coordinators.insert(row.coin.clone(), coordinator);
        rest_updates.push(update);
        subscribe_args.push(hyperliquid_wss_top_of_book_subscribe_payload(&row.coin)?);
    }

    Ok(PublicTopOfBookAllMarketState {
        venue_id,
        stream_url: HYPERLIQUID_PUBLIC_WSS_URL.to_owned(),
        subscribe_args,
        ignore_untracked_wss_symbols: all_symbols_scope,
        coordinators,
        local_sequences,
        last_exchange_update_ids: BTreeMap::new(),
        rest_updates,
    })
}

pub(crate) fn bootstrap_lighter_wss_book_ticker(
    symbol_scope: &str,
    market: LighterPublicWssMarket,
) -> RuntimeResult<PublicTopOfBookAllMarketState> {
    let symbol_scope = normalize_lighter_wss_symbol_scope(symbol_scope)?;
    let target_symbols =
        explicit_wss_symbol_scope_set(&symbol_scope, is_lighter_wss_all_symbols_scope);
    let all_symbols_scope = target_symbols.is_none();
    let raw_rest_snapshot = fetch_public_json_with_curl(&lighter_order_books_url(market))?;
    let mut rows = parse_lighter_order_book_metadata_rows(&raw_rest_snapshot)?;
    rows.retain(|row| {
        target_symbols
            .as_ref()
            .map(|symbols| symbols.contains(&row.symbol))
            .unwrap_or(true)
    });
    rows.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    rows.dedup_by(|left, right| left.symbol == right.symbol);
    ensure_wss_requested_symbols_present(
        "Lighter WSS orderBooks metadata bootstrap",
        target_symbols.as_ref(),
        rows.iter().map(|row| row.symbol.clone()).collect(),
    )?;
    if rows.is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Lighter WSS metadata bootstrap returned no rows for `{symbol_scope}`"
            ),
        });
    }

    let observed_at = current_utc_timestamp()?;
    let venue_id = VenueId::new(market.venue_id())?;
    let mut coordinators = BTreeMap::new();
    let mut local_sequences = BTreeMap::new();
    let mut subscribe_args = Vec::with_capacity(rows.len());
    for row in rows {
        let instrument_id = lighter_public_wss_instrument_id(&row.symbol, market)?;
        let coordinator = RestWssMarketDataCoordinator::new(
            venue_id.clone(),
            instrument_id,
            observed_at,
            PUBLIC_WSS_MONITOR_MAX_AGE_MS,
        )?;
        local_sequences.insert(row.symbol.clone(), 0_u64);
        subscribe_args.push(lighter_wss_ticker_subscribe_payload(&row.market_id)?);
        coordinators.insert(row.symbol, coordinator);
    }

    Ok(PublicTopOfBookAllMarketState {
        venue_id,
        stream_url: LIGHTER_PUBLIC_WSS_URL.to_owned(),
        subscribe_args,
        ignore_untracked_wss_symbols: all_symbols_scope,
        coordinators,
        local_sequences,
        last_exchange_update_ids: BTreeMap::new(),
        rest_updates: Vec::new(),
    })
}

pub(crate) fn bootstrap_gate_wss_book_ticker(
    symbol_scope: &str,
    market: GatePublicWssMarket,
) -> RuntimeResult<PublicTopOfBookAllMarketState> {
    let symbol_scope = normalize_gate_wss_symbol_scope(symbol_scope)?;
    let venue_id = VenueId::new(market.venue_id())?;
    let target_symbols =
        explicit_wss_symbol_scope_set(&symbol_scope, is_gate_wss_all_symbols_scope);
    let all_symbols_scope = target_symbols.is_none();
    let ignore_untracked_wss_symbols = all_symbols_scope
        || target_symbols
            .as_ref()
            .is_some_and(|symbols| symbols.len() > 1);
    let raw_rest_snapshot = fetch_public_json_with_curl(&gate_usdt_futures_tickers_url())?;
    let rows = prepare_gate_wss_book_ticker_rest_rows(
        parse_gate_futures_ticker_rows(&raw_rest_snapshot)?,
        &symbol_scope,
        all_symbols_scope,
    )?;
    if rows.is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Gate WSS REST ticker bootstrap returned no rows for `{symbol_scope}`"
            ),
        });
    }

    let observed_at = current_utc_timestamp()?;
    let mut coordinators = BTreeMap::new();
    let mut local_sequences = BTreeMap::new();
    let mut rest_updates = Vec::with_capacity(rows.len());
    let mut subscribe_args = Vec::with_capacity(rows.len());
    for row in rows {
        let symbol = row.symbol.clone();
        let instrument_id = gate_public_wss_instrument_id(&symbol, market)?;
        let quote = public_wss_top_of_book_quote(
            &symbol,
            venue_id.clone(),
            instrument_id.clone(),
            PublicTopOfBookRuntimeRaw {
                symbol: symbol.clone(),
                update_id: 1,
                best_bid: Price::from_str(row.bid_price.as_deref().expect("bid present"))?,
                best_ask: Price::from_str(row.ask_price.as_deref().expect("ask present"))?,
                bid_size: Quantity::from_str(row.bid_qty.as_deref().expect("bid size present"))?,
                ask_size: Quantity::from_str(row.ask_qty.as_deref().expect("ask size present"))?,
                observed_at,
            },
            format!("gate:rest-tickers:{}:{}", row.symbol, observed_at),
        )?;
        let mut coordinator = RestWssMarketDataCoordinator::new(
            venue_id.clone(),
            instrument_id,
            observed_at,
            PUBLIC_WSS_MONITOR_MAX_AGE_MS,
        )?;
        let update = coordinator.apply(HybridMarketDataInput::RestSnapshot { quote })?;
        local_sequences.insert(symbol.clone(), 1_u64);
        subscribe_args.push(gate_wss_book_ticker_subscribe_payload(&symbol)?);
        coordinators.insert(symbol, coordinator);
        rest_updates.push(update);
    }

    Ok(PublicTopOfBookAllMarketState {
        venue_id,
        stream_url: GATE_PUBLIC_FUTURES_WSS_URL.to_owned(),
        subscribe_args,
        ignore_untracked_wss_symbols,
        coordinators,
        local_sequences,
        last_exchange_update_ids: BTreeMap::new(),
        rest_updates,
    })
}

pub(crate) fn prepare_gate_wss_book_ticker_rest_rows(
    rows: Vec<GateFuturesTickerRow>,
    symbol_scope: &str,
    all_symbols_scope: bool,
) -> RuntimeResult<Vec<GateFuturesTickerRow>> {
    let normalized_scope = normalize_gate_wss_symbol_scope(symbol_scope)?;
    let all_symbols_scope = all_symbols_scope || is_gate_wss_all_symbols_scope(&normalized_scope);
    let target_symbols =
        explicit_wss_symbol_scope_set(&normalized_scope, is_gate_wss_all_symbols_scope);
    let mut prepared = Vec::with_capacity(rows.len());
    for mut row in rows {
        match validate_gate_public_wss_symbol(&row.symbol) {
            Ok(symbol)
                if wss_symbol_in_scope(&symbol, all_symbols_scope, target_symbols.as_ref()) =>
            {
                if gate_wss_ticker_row_has_usable_top_of_book(&row) {
                    row.symbol = symbol;
                    prepared.push(row);
                } else if !all_symbols_scope {
                    return Err(RuntimeError::LiveMarketData {
                        message: format!(
                            "Gate WSS REST ticker row for `{symbol}` has incomplete top-of-book"
                        ),
                    });
                }
            }
            Ok(_) => {}
            Err(_) if all_symbols_scope || target_symbols.is_some() => {}
            Err(error) => return Err(error),
        }
    }
    prepared.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    prepared.dedup_by(|left, right| left.symbol == right.symbol);
    ensure_wss_requested_symbols_present(
        "Gate WSS REST ticker bootstrap",
        target_symbols.as_ref(),
        prepared.iter().map(|row| row.symbol.clone()).collect(),
    )?;
    Ok(prepared)
}

fn gate_wss_ticker_row_has_usable_top_of_book(row: &GateFuturesTickerRow) -> bool {
    [
        row.bid_price.as_deref(),
        row.bid_qty.as_deref(),
        row.ask_price.as_deref(),
        row.ask_qty.as_deref(),
    ]
    .into_iter()
    .all(|value| value.is_some_and(|value| !value.trim().is_empty()))
}

pub(crate) fn prepare_aster_wss_book_ticker_rest_rows(
    rows: Vec<MonitorBookTickerRow>,
    symbol_scope: &str,
    all_symbols_scope: bool,
) -> RuntimeResult<Vec<MonitorBookTickerRow>> {
    let normalized_scope = normalize_aster_wss_symbol_scope(symbol_scope)?;
    let all_symbols_scope = all_symbols_scope || is_aster_wss_all_symbols_scope(&normalized_scope);
    let target_symbols =
        explicit_wss_symbol_scope_set(&normalized_scope, is_aster_wss_all_symbols_scope);
    let mut prepared = Vec::with_capacity(rows.len());
    for mut row in rows {
        match validate_aster_public_wss_symbol(&row.symbol) {
            Ok(symbol)
                if wss_symbol_in_scope(&symbol, all_symbols_scope, target_symbols.as_ref()) =>
            {
                row.symbol = symbol;
                prepared.push(row);
            }
            Ok(_) => {}
            Err(_) if all_symbols_scope || target_symbols.is_some() => {}
            Err(error) => return Err(error),
        }
    }
    prepared.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    prepared.dedup_by(|left, right| left.symbol == right.symbol);
    ensure_wss_requested_symbols_present(
        "Aster WSS REST bootstrap",
        target_symbols.as_ref(),
        prepared.iter().map(|row| row.symbol.clone()).collect(),
    )?;
    Ok(prepared)
}

pub(crate) fn binance_wss_rest_quote_from_row(
    row: &MonitorBookTickerRow,
    venue_id: &VenueId,
    instrument: &BinancePublicInstrument,
    observed_at: UtcTimestamp,
) -> RuntimeResult<MarketQuote> {
    let freshness = DataFreshness::new(observed_at, observed_at, PUBLIC_WSS_MONITOR_MAX_AGE_MS)?;
    Ok(MarketQuote {
        venue_id: venue_id.clone(),
        instrument_id: instrument.instrument_id.clone(),
        last_price: None,
        best_bid: Some(Price::from_str(&row.bid_price)?),
        best_ask: Some(Price::from_str(&row.ask_price)?),
        mark_price: None,
        index_price: None,
        bid_size: Some(Quantity::from_str(&row.bid_qty)?),
        ask_size: Some(Quantity::from_str(&row.ask_qty)?),
        source_sequence: None,
        source_event_id: Some(format!("binance:rest-bookTicker:{}", row.symbol)),
        freshness,
    })
}

pub(crate) fn bybit_wss_rest_quote_from_row(
    row: &MonitorBookTickerRow,
    venue_id: &VenueId,
    instrument: &BybitPublicInstrument,
    market: BybitPublicMarket,
    observed_at: UtcTimestamp,
) -> RuntimeResult<MarketQuote> {
    let freshness = DataFreshness::new(observed_at, observed_at, PUBLIC_WSS_MONITOR_MAX_AGE_MS)?;
    Ok(MarketQuote {
        venue_id: venue_id.clone(),
        instrument_id: instrument.instrument_id.clone(),
        last_price: None,
        best_bid: Some(Price::from_str(&row.bid_price)?),
        best_ask: Some(Price::from_str(&row.ask_price)?),
        mark_price: None,
        index_price: None,
        bid_size: Some(Quantity::from_str(&row.bid_qty)?),
        ask_size: Some(Quantity::from_str(&row.ask_qty)?),
        source_sequence: None,
        source_event_id: Some(format!(
            "bybit:rest-tickers:{}:{}",
            bybit_public_market_event_scope(market),
            row.symbol
        )),
        freshness,
    })
}

pub(crate) fn okx_wss_rest_quote_from_row(
    row: &OkxTickerRow,
    symbol: &str,
    venue_id: &VenueId,
    instrument_id: InstrumentId,
    observed_at: UtcTimestamp,
) -> RuntimeResult<MarketQuote> {
    public_wss_top_of_book_quote(
        symbol,
        venue_id.clone(),
        instrument_id,
        PublicTopOfBookRuntimeRaw {
            symbol: symbol.to_owned(),
            update_id: 1,
            best_bid: Price::from_str(&row.bid_price)?,
            best_ask: Price::from_str(&row.ask_price)?,
            bid_size: Quantity::from_str(&row.bid_qty)?,
            ask_size: Quantity::from_str(&row.ask_qty)?,
            observed_at,
        },
        format!("okx:rest-tickers:{}:{}", row.inst_id, observed_at),
    )
}

pub(crate) fn bitget_wss_rest_quote_from_row(
    row: &MonitorBookTickerRow,
    venue_id: &VenueId,
    instrument_id: InstrumentId,
    market: BitgetPublicWssMarket,
    observed_at: UtcTimestamp,
) -> RuntimeResult<MarketQuote> {
    public_wss_top_of_book_quote(
        &row.symbol,
        venue_id.clone(),
        instrument_id,
        PublicTopOfBookRuntimeRaw {
            symbol: row.symbol.clone(),
            update_id: 1,
            best_bid: Price::from_str(&row.bid_price)?,
            best_ask: Price::from_str(&row.ask_price)?,
            bid_size: Quantity::from_str(&row.bid_qty)?,
            ask_size: Quantity::from_str(&row.ask_qty)?,
            observed_at,
        },
        format!(
            "bitget:rest-tickers:{}:{}:{}",
            market.as_str(),
            row.symbol,
            observed_at
        ),
    )
}

pub(crate) fn public_wss_top_of_book_quote(
    symbol: &str,
    venue_id: VenueId,
    instrument_id: InstrumentId,
    raw: PublicTopOfBookRuntimeRaw,
    source_event_id: String,
) -> RuntimeResult<MarketQuote> {
    let freshness = DataFreshness::new(
        raw.observed_at,
        raw.observed_at,
        PUBLIC_WSS_MONITOR_MAX_AGE_MS,
    )?;
    Ok(MarketQuote {
        venue_id,
        instrument_id,
        last_price: None,
        best_bid: Some(raw.best_bid),
        best_ask: Some(raw.best_ask),
        mark_price: None,
        index_price: None,
        bid_size: Some(raw.bid_size),
        ask_size: Some(raw.ask_size),
        source_sequence: Some(raw.update_id.to_string()),
        source_event_id: Some(source_event_id.replace(['\n', '\r'], " ")),
        freshness,
    })
    .map(|mut quote| {
        if quote.source_event_id.is_none() {
            quote.source_event_id = Some(format!("public:wss-rest:{symbol}"));
        }
        quote
    })
}

pub(crate) fn apply_binance_wss_book_ticker_text(
    raw_json: &str,
    ingested_at: UtcTimestamp,
    market: BinancePublicMarket,
    state: &mut PublicTopOfBookAllMarketState,
) -> RuntimeResult<Option<HybridMarketDataUpdate>> {
    let mut raw = parse_binance_wss_book_ticker_runtime_raw(raw_json, ingested_at)?;
    raw.symbol = match validate_binance_public_wss_symbol(&raw.symbol) {
        Ok(symbol) => symbol,
        Err(_) if state.ignore_untracked_wss_symbols => return Ok(None),
        Err(error) => return Err(error),
    };
    if !state.coordinators.contains_key(&raw.symbol) {
        if state.ignore_untracked_wss_symbols {
            return Ok(None);
        }
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "WSS symbol `{}` was not present in REST bootstrap; REST rebuild required",
                raw.symbol
            ),
        });
    }
    if let Some(previous) = state.last_exchange_update_ids.get(&raw.symbol) {
        if raw.update_id <= *previous {
            let update = state
                .coordinators
                .get_mut(&raw.symbol)
                .expect("coordinator exists")
                .apply(HybridMarketDataInput::WssGapDetected {
                    expected_sequence: None,
                    observed_sequence: state.local_sequences.get(&raw.symbol).copied(),
                    occurred_at: raw.observed_at,
                    ingested_at,
                    detail: format!(
                        "Binance WSS bookTicker updateId `{}` did not advance beyond `{previous}`; REST rebuild required",
                        raw.update_id
                    ),
                })?;
            return Ok(Some(update));
        }
    }
    let local_sequence = next_public_wss_local_sequence(state, &raw.symbol)?;
    let instrument = binance_public_wss_instrument(&raw.symbol, market)?;
    let update = state
        .coordinators
        .get_mut(&raw.symbol)
        .expect("coordinator exists")
        .apply(HybridMarketDataInput::WssQuote {
            update: WssQuoteUpdate {
                venue_id: state.venue_id.clone(),
                instrument_id: instrument.instrument_id,
                last_price: None,
                best_bid: Some(raw.best_bid),
                best_ask: Some(raw.best_ask),
                mark_price: None,
                index_price: None,
                bid_size: Some(raw.bid_size),
                ask_size: Some(raw.ask_size),
                source_sequence: local_sequence,
                source_event_id: Some(format!(
                    "binance:wss-book-ticker:{}:{}:{}",
                    market.as_str(),
                    raw.symbol,
                    raw.update_id
                )),
                observed_at: raw.observed_at,
                ingested_at,
            },
        })?;
    state
        .last_exchange_update_ids
        .insert(raw.symbol, raw.update_id);
    Ok(Some(update))
}

pub(crate) fn apply_bybit_wss_book_ticker_text(
    raw_json: &str,
    ingested_at: UtcTimestamp,
    market: BybitPublicMarket,
    state: &mut PublicTopOfBookAllMarketState,
) -> RuntimeResult<Option<HybridMarketDataUpdate>> {
    let Some(mut raw) = parse_bybit_wss_book_ticker_runtime_raw(raw_json, ingested_at)? else {
        return Ok(None);
    };
    raw.symbol = match validate_bybit_public_wss_symbol(&raw.symbol) {
        Ok(symbol) => symbol,
        Err(_) if state.ignore_untracked_wss_symbols => return Ok(None),
        Err(error) => return Err(error),
    };
    if !state.coordinators.contains_key(&raw.symbol) {
        if state.ignore_untracked_wss_symbols {
            return Ok(None);
        }
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Bybit WSS symbol `{}` was not present in REST bootstrap; REST rebuild required",
                raw.symbol
            ),
        });
    }
    if let Some(previous) = state.last_exchange_update_ids.get(&raw.symbol) {
        if raw.update_id <= *previous {
            let update = state
                .coordinators
                .get_mut(&raw.symbol)
                .expect("coordinator exists")
                .apply(HybridMarketDataInput::WssGapDetected {
                    expected_sequence: None,
                    observed_sequence: state.local_sequences.get(&raw.symbol).copied(),
                    occurred_at: raw.observed_at,
                    ingested_at,
                    detail: format!(
                        "Bybit WSS orderbook updateId `{}` did not advance beyond `{previous}`; REST rebuild required",
                        raw.update_id
                    ),
                })?;
            return Ok(Some(update));
        }
    }
    let instrument = bybit_public_wss_instrument(&raw.symbol, market)?;
    let previous_quote = state
        .coordinators
        .get(&raw.symbol)
        .expect("coordinator exists")
        .latest_quote(&MarketDataQuery::new(
            state.venue_id.clone(),
            instrument.instrument_id.clone(),
        ))?;
    let best_bid = raw
        .best_bid
        .or_else(|| previous_quote.as_ref().and_then(|quote| quote.best_bid));
    let best_ask = raw
        .best_ask
        .or_else(|| previous_quote.as_ref().and_then(|quote| quote.best_ask));
    let bid_size = raw
        .bid_size
        .or_else(|| previous_quote.as_ref().and_then(|quote| quote.bid_size));
    let ask_size = raw
        .ask_size
        .or_else(|| previous_quote.as_ref().and_then(|quote| quote.ask_size));
    let (Some(best_bid), Some(best_ask), Some(bid_size), Some(ask_size)) =
        (best_bid, best_ask, bid_size, ask_size)
    else {
        let detail = format!(
            "Bybit WSS top-of-book update for `{}` lacks a complete bid/ask and previous quote cannot complete it; REST rebuild required",
            raw.symbol
        );
        let update = state
            .coordinators
            .get_mut(&raw.symbol)
            .expect("coordinator exists")
            .apply(HybridMarketDataInput::WssGapDetected {
                expected_sequence: None,
                observed_sequence: state.local_sequences.get(&raw.symbol).copied(),
                occurred_at: raw.observed_at,
                ingested_at,
                detail,
            })?;
        return Ok(Some(update));
    };
    let local_sequence = next_public_wss_local_sequence(state, &raw.symbol)?;
    let update = state
        .coordinators
        .get_mut(&raw.symbol)
        .expect("coordinator exists")
        .apply(HybridMarketDataInput::WssQuote {
            update: WssQuoteUpdate {
                venue_id: state.venue_id.clone(),
                instrument_id: instrument.instrument_id,
                last_price: None,
                best_bid: Some(best_bid),
                best_ask: Some(best_ask),
                mark_price: None,
                index_price: None,
                bid_size: Some(bid_size),
                ask_size: Some(ask_size),
                source_sequence: local_sequence,
                source_event_id: Some(format!(
                    "bybit:wss-book-ticker:{}:{}:{}",
                    bybit_public_market_event_scope(market),
                    raw.symbol,
                    raw.update_id
                )),
                observed_at: raw.observed_at,
                ingested_at,
            },
        })?;
    state
        .last_exchange_update_ids
        .insert(raw.symbol, raw.update_id);
    Ok(Some(update))
}

pub(crate) fn apply_okx_wss_book_ticker_text(
    raw_json: &str,
    ingested_at: UtcTimestamp,
    market: OkxPublicWssMarket,
    state: &mut PublicTopOfBookAllMarketState,
) -> RuntimeResult<Option<HybridMarketDataUpdate>> {
    let Some(raw) = parse_okx_wss_book_ticker_runtime_raw(raw_json, ingested_at, market)? else {
        return Ok(None);
    };
    if !state.coordinators.contains_key(&raw.symbol) {
        if state.ignore_untracked_wss_symbols {
            return Ok(None);
        }
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "OKX WSS symbol `{}` was not present in REST bootstrap; REST rebuild required",
                raw.symbol
            ),
        });
    }
    let local_sequence = next_public_wss_local_sequence(state, &raw.symbol)?;
    let instrument_id = okx_public_wss_instrument_id(&raw.symbol, market)?;
    let update = state
        .coordinators
        .get_mut(&raw.symbol)
        .expect("coordinator exists")
        .apply(HybridMarketDataInput::WssQuote {
            update: WssQuoteUpdate {
                venue_id: state.venue_id.clone(),
                instrument_id,
                last_price: None,
                best_bid: Some(raw.best_bid),
                best_ask: Some(raw.best_ask),
                mark_price: None,
                index_price: None,
                bid_size: Some(raw.bid_size),
                ask_size: Some(raw.ask_size),
                source_sequence: local_sequence,
                source_event_id: Some(format!(
                    "event:venue-data:okx-public:wss-book-ticker:{}:{}:{}",
                    okx_public_wss_market_event_scope(market),
                    raw.venue_symbol,
                    local_sequence
                )),
                observed_at: raw.observed_at,
                ingested_at,
            },
        })?;
    Ok(Some(update))
}

pub(crate) fn apply_bitget_wss_book_ticker_text(
    raw_json: &str,
    ingested_at: UtcTimestamp,
    market: BitgetPublicWssMarket,
    state: &mut PublicTopOfBookAllMarketState,
) -> RuntimeResult<Option<HybridMarketDataUpdate>> {
    let Some(raw) = parse_bitget_wss_book_ticker_runtime_raw(raw_json, ingested_at)? else {
        return Ok(None);
    };
    if !state.coordinators.contains_key(&raw.symbol) {
        if state.ignore_untracked_wss_symbols {
            return Ok(None);
        }
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Bitget WSS symbol `{}` was not present in REST bootstrap; REST rebuild required",
                raw.symbol
            ),
        });
    }
    let local_sequence = next_public_wss_local_sequence(state, &raw.symbol)?;
    let instrument_id = bitget_public_wss_instrument_id(&raw.symbol, market)?;
    let update = state
        .coordinators
        .get_mut(&raw.symbol)
        .expect("coordinator exists")
        .apply(HybridMarketDataInput::WssQuote {
            update: WssQuoteUpdate {
                venue_id: state.venue_id.clone(),
                instrument_id,
                last_price: None,
                best_bid: Some(raw.best_bid),
                best_ask: Some(raw.best_ask),
                mark_price: None,
                index_price: None,
                bid_size: Some(raw.bid_size),
                ask_size: Some(raw.ask_size),
                source_sequence: local_sequence,
                source_event_id: Some(format!(
                    "event:venue-data:bitget-public:wss-book-ticker:{}:{}:{}",
                    bitget_public_wss_market_event_scope(market),
                    raw.venue_symbol,
                    local_sequence
                )),
                observed_at: raw.observed_at,
                ingested_at,
            },
        })?;
    Ok(Some(update))
}

pub(crate) fn apply_aster_wss_book_ticker_text(
    raw_json: &str,
    ingested_at: UtcTimestamp,
    market: AsterPublicWssMarket,
    state: &mut PublicTopOfBookAllMarketState,
) -> RuntimeResult<Option<HybridMarketDataUpdate>> {
    let mut raw = parse_aster_wss_book_ticker_runtime_raw(raw_json, ingested_at)?;
    raw.symbol = match validate_aster_public_wss_symbol(&raw.symbol) {
        Ok(symbol) => symbol,
        Err(_) if state.ignore_untracked_wss_symbols => return Ok(None),
        Err(error) => return Err(error),
    };
    if !state.coordinators.contains_key(&raw.symbol) {
        if state.ignore_untracked_wss_symbols {
            return Ok(None);
        }
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Aster WSS symbol `{}` was not present in REST bootstrap; REST rebuild required",
                raw.symbol
            ),
        });
    }
    if let Some(previous) = state.last_exchange_update_ids.get(&raw.symbol) {
        if raw.update_id <= *previous {
            let update = state
                .coordinators
                .get_mut(&raw.symbol)
                .expect("coordinator exists")
                .apply(HybridMarketDataInput::WssGapDetected {
                    expected_sequence: None,
                    observed_sequence: state.local_sequences.get(&raw.symbol).copied(),
                    occurred_at: raw.observed_at,
                    ingested_at,
                    detail: format!(
                        "Aster WSS bookTicker updateId `{}` did not advance beyond `{previous}`; REST rebuild required",
                        raw.update_id
                    ),
                })?;
            return Ok(Some(update));
        }
    }
    let local_sequence = next_public_wss_local_sequence(state, &raw.symbol)?;
    let instrument_id = aster_public_wss_instrument_id(&raw.symbol, market)?;
    let update = state
        .coordinators
        .get_mut(&raw.symbol)
        .expect("coordinator exists")
        .apply(HybridMarketDataInput::WssQuote {
            update: WssQuoteUpdate {
                venue_id: state.venue_id.clone(),
                instrument_id,
                last_price: None,
                best_bid: Some(raw.best_bid),
                best_ask: Some(raw.best_ask),
                mark_price: None,
                index_price: None,
                bid_size: Some(raw.bid_size),
                ask_size: Some(raw.ask_size),
                source_sequence: local_sequence,
                source_event_id: Some(format!(
                    "aster:wss-book-ticker:{}:{}:{}",
                    aster_public_wss_market_event_scope(market),
                    raw.symbol,
                    raw.update_id
                )),
                observed_at: raw.observed_at,
                ingested_at,
            },
        })?;
    state
        .last_exchange_update_ids
        .insert(raw.symbol, raw.update_id);
    Ok(Some(update))
}

pub(crate) fn apply_hyperliquid_wss_book_ticker_text(
    raw_json: &str,
    ingested_at: UtcTimestamp,
    market: HyperliquidPublicWssMarket,
    state: &mut PublicTopOfBookAllMarketState,
) -> RuntimeResult<Option<HybridMarketDataUpdate>> {
    let Some(raw) = parse_hyperliquid_wss_bbo_runtime_raw(raw_json, ingested_at)? else {
        return Ok(None);
    };
    if !state.coordinators.contains_key(&raw.symbol) {
        if state.ignore_untracked_wss_symbols {
            return Ok(None);
        }
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Hyperliquid WSS coin `{}` was not present in REST bootstrap; REST rebuild required",
                raw.symbol
            ),
        });
    }
    let local_sequence = next_public_wss_local_sequence(state, &raw.symbol)?;
    let instrument_id = hyperliquid_public_wss_instrument_id(&raw.symbol, market)?;
    let update = state
        .coordinators
        .get_mut(&raw.symbol)
        .expect("coordinator exists")
        .apply(HybridMarketDataInput::WssQuote {
            update: WssQuoteUpdate {
                venue_id: state.venue_id.clone(),
                instrument_id,
                last_price: None,
                best_bid: Some(raw.best_bid),
                best_ask: Some(raw.best_ask),
                mark_price: None,
                index_price: None,
                bid_size: Some(raw.bid_size),
                ask_size: Some(raw.ask_size),
                source_sequence: local_sequence,
                source_event_id: Some(format!(
                    "hyperliquid:wss-book-ticker:{}:{}:{}",
                    hyperliquid_public_wss_market_event_scope(market),
                    raw.symbol,
                    raw.update_id
                )),
                observed_at: raw.observed_at,
                ingested_at,
            },
        })?;
    Ok(Some(update))
}

pub(crate) fn apply_lighter_wss_book_ticker_text(
    raw_json: &str,
    ingested_at: UtcTimestamp,
    market: LighterPublicWssMarket,
    state: &mut PublicTopOfBookAllMarketState,
) -> RuntimeResult<Option<HybridMarketDataUpdate>> {
    let Some(mut raw) = parse_lighter_wss_ticker_runtime_raw(raw_json, ingested_at)? else {
        return Ok(None);
    };
    raw.symbol = match validate_lighter_public_wss_symbol(&raw.symbol) {
        Ok(symbol) => symbol,
        Err(_) if state.ignore_untracked_wss_symbols => return Ok(None),
        Err(error) => return Err(error),
    };
    if !state.coordinators.contains_key(&raw.symbol) {
        if state.ignore_untracked_wss_symbols {
            return Ok(None);
        }
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Lighter WSS symbol `{}` was not present in metadata bootstrap; metadata rebuild required",
                raw.symbol
            ),
        });
    }
    if let Some(previous) = state.last_exchange_update_ids.get(&raw.symbol) {
        if raw.update_id <= *previous {
            let update = state
                .coordinators
                .get_mut(&raw.symbol)
                .expect("coordinator exists")
                .apply(HybridMarketDataInput::WssGapDetected {
                    expected_sequence: None,
                    observed_sequence: state.local_sequences.get(&raw.symbol).copied(),
                    occurred_at: raw.observed_at,
                    ingested_at,
                    detail: format!(
                        "Lighter WSS ticker nonce `{}` did not advance beyond `{previous}`; metadata rebuild required",
                        raw.update_id
                    ),
                })?;
            return Ok(Some(update));
        }
    }
    let local_sequence = next_public_wss_local_sequence(state, &raw.symbol)?;
    let instrument_id = lighter_public_wss_instrument_id(&raw.symbol, market)?;
    let source_event_id = format!(
        "lighter:wss-book-ticker:{}:{}:{}",
        lighter_public_wss_market_event_scope(market),
        raw.symbol,
        raw.update_id
    );
    let update = if state
        .coordinators
        .get(&raw.symbol)
        .expect("coordinator exists")
        .latest_quote_snapshot()
        .is_none()
    {
        let quote = lighter_wss_bootstrap_quote(
            state.venue_id.clone(),
            instrument_id,
            &raw,
            source_event_id,
        )?;
        state
            .coordinators
            .get_mut(&raw.symbol)
            .expect("coordinator exists")
            .apply(HybridMarketDataInput::RestSnapshot { quote })?
    } else {
        state
            .coordinators
            .get_mut(&raw.symbol)
            .expect("coordinator exists")
            .apply(HybridMarketDataInput::WssQuote {
                update: WssQuoteUpdate {
                    venue_id: state.venue_id.clone(),
                    instrument_id,
                    last_price: None,
                    best_bid: Some(raw.best_bid),
                    best_ask: Some(raw.best_ask),
                    mark_price: None,
                    index_price: None,
                    bid_size: Some(raw.bid_size),
                    ask_size: Some(raw.ask_size),
                    source_sequence: local_sequence,
                    source_event_id: Some(source_event_id),
                    observed_at: raw.observed_at,
                    ingested_at,
                },
            })?
    };
    state
        .last_exchange_update_ids
        .insert(raw.symbol, raw.update_id);
    Ok(Some(update))
}

pub(crate) fn lighter_wss_bootstrap_quote(
    venue_id: VenueId,
    instrument_id: InstrumentId,
    raw: &PublicTopOfBookRuntimeRaw,
    source_event_id: String,
) -> RuntimeResult<MarketQuote> {
    let freshness = DataFreshness::new(
        raw.observed_at,
        raw.observed_at,
        PUBLIC_WSS_MONITOR_MAX_AGE_MS,
    )?;
    Ok(MarketQuote {
        venue_id,
        instrument_id,
        last_price: None,
        best_bid: Some(raw.best_bid),
        best_ask: Some(raw.best_ask),
        mark_price: None,
        index_price: None,
        bid_size: Some(raw.bid_size),
        ask_size: Some(raw.ask_size),
        source_sequence: None,
        source_event_id: Some(source_event_id.replace(['\n', '\r'], " ")),
        freshness,
    })
}

pub(crate) fn apply_gate_wss_book_ticker_text(
    raw_json: &str,
    ingested_at: UtcTimestamp,
    market: GatePublicWssMarket,
    state: &mut PublicTopOfBookAllMarketState,
) -> RuntimeResult<Option<HybridMarketDataUpdate>> {
    let Some(mut raw) = parse_gate_wss_book_ticker_runtime_raw(raw_json, ingested_at)? else {
        return Ok(None);
    };
    raw.symbol = match validate_gate_public_wss_symbol(&raw.symbol) {
        Ok(symbol) => symbol,
        Err(_) if state.ignore_untracked_wss_symbols => return Ok(None),
        Err(error) => return Err(error),
    };
    if !state.coordinators.contains_key(&raw.symbol) {
        if state.ignore_untracked_wss_symbols {
            return Ok(None);
        }
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Gate WSS symbol `{}` was not present in REST ticker bootstrap; REST rebuild required",
                raw.symbol
            ),
        });
    }
    if let Some(previous) = state.last_exchange_update_ids.get(&raw.symbol) {
        if raw.update_id <= *previous {
            let update = state
                .coordinators
                .get_mut(&raw.symbol)
                .expect("coordinator exists")
                .apply(HybridMarketDataInput::WssGapDetected {
                    expected_sequence: None,
                    observed_sequence: state.local_sequences.get(&raw.symbol).copied(),
                    occurred_at: raw.observed_at,
                    ingested_at,
                    detail: format!(
                        "Gate WSS bookTicker updateId `{}` did not advance beyond `{previous}`; REST rebuild required",
                        raw.update_id
                    ),
                })?;
            return Ok(Some(update));
        }
    }
    let local_sequence = next_public_wss_local_sequence(state, &raw.symbol)?;
    let instrument_id = gate_public_wss_instrument_id(&raw.symbol, market)?;
    let update = state
        .coordinators
        .get_mut(&raw.symbol)
        .expect("coordinator exists")
        .apply(HybridMarketDataInput::WssQuote {
            update: WssQuoteUpdate {
                venue_id: state.venue_id.clone(),
                instrument_id,
                last_price: None,
                best_bid: Some(raw.best_bid),
                best_ask: Some(raw.best_ask),
                mark_price: None,
                index_price: None,
                bid_size: Some(raw.bid_size),
                ask_size: Some(raw.ask_size),
                source_sequence: local_sequence,
                source_event_id: Some(format!(
                    "gate:wss-book-ticker:{}:{}:{}",
                    gate_public_wss_market_event_scope(market),
                    raw.symbol,
                    raw.update_id
                )),
                observed_at: raw.observed_at,
                ingested_at,
            },
        })?;
    state
        .last_exchange_update_ids
        .insert(raw.symbol, raw.update_id);
    Ok(Some(update))
}

pub(crate) fn next_public_wss_local_sequence(
    state: &mut PublicTopOfBookAllMarketState,
    symbol: &str,
) -> RuntimeResult<u64> {
    let sequence =
        state
            .local_sequences
            .get_mut(symbol)
            .ok_or_else(|| RuntimeError::LiveMarketData {
                message: format!("missing local WSS sequence for `{symbol}`"),
            })?;
    *sequence = sequence
        .checked_add(1)
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("local Binance WSS sequence overflow for `{symbol}`"),
        })?;
    Ok(*sequence)
}

pub(crate) fn parse_bybit_wss_book_ticker_runtime_raw(
    raw_json: &str,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<Option<BybitTopOfBookRuntimeRaw>> {
    let root = parse_json_object_value_slices(raw_json)?;
    if let Some(op) = optional_json_value_string(&root, "op", "bybit wss")? {
        if op == "subscribe" {
            if optional_json_scalar_bool(&root, "success", "bybit wss subscribe")? == Some(false) {
                let ret_msg = optional_json_value_string(&root, "ret_msg", "bybit wss subscribe")?
                    .unwrap_or_else(|| "unknown error".to_owned());
                return Err(RuntimeError::LiveMarketData {
                    message: format!("Bybit public WSS subscribe failed: {ret_msg}"),
                });
            }
            return Ok(None);
        }
        if op == "pong" {
            return Ok(None);
        }
    }
    let Some(data) = root.get("data") else {
        return Ok(None);
    };
    let topic = optional_json_value_string(&root, "topic", "bybit wss")?.unwrap_or_default();
    if !topic.starts_with("orderbook.1.") && !topic.starts_with("tickers.") {
        return Ok(None);
    }
    let is_orderbook = topic.starts_with("orderbook.1.");
    let fields = parse_json_object_value_slices(data)?;
    let symbol = required_json_value_string(&fields, "s", "bybit wss orderbook")
        .or_else(|_| required_json_value_string(&fields, "symbol", "bybit wss ticker"))?;
    let update_id = optional_json_scalar_u64(&fields, "u", "bybit wss orderbook")?
        .or(optional_json_scalar_u64(
            &fields,
            "seq",
            "bybit wss ticker",
        )?)
        .or(optional_json_scalar_u64(&root, "cs", "bybit wss ticker")?)
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: "Bybit WSS top-of-book update lacks `u`, `seq`, or `cs` update id".to_owned(),
        })?;
    let bid = if is_orderbook {
        fields
            .get("b")
            .map(|bids| bybit_wss_first_price_size(bids, "bid"))
            .transpose()?
            .flatten()
    } else {
        bybit_wss_optional_ticker_price_size(&fields, "bid1Price", "bid1Size", "bid")?
    };
    let ask = if is_orderbook {
        fields
            .get("a")
            .map(|asks| bybit_wss_first_price_size(asks, "ask"))
            .transpose()?
            .flatten()
    } else {
        bybit_wss_optional_ticker_price_size(&fields, "ask1Price", "ask1Size", "ask")?
    };
    if bid.is_none() && ask.is_none() {
        return Ok(None);
    }
    let (best_bid, bid_size) = bid
        .map(|(price, size)| (Some(price), Some(size)))
        .unwrap_or((None, None));
    let (best_ask, ask_size) = ask
        .map(|(price, size)| (Some(price), Some(size)))
        .unwrap_or((None, None));
    let observed_at = optional_json_scalar_u64(&root, "ts", "bybit wss")?
        .or(optional_json_scalar_u64(&fields, "ts", "bybit wss")?)
        .map(timestamp_from_unix_millis)
        .transpose()?
        .unwrap_or(ingested_at);
    let observed_at = observed_at_not_after_ingested(observed_at, ingested_at)?;
    Ok(Some(BybitTopOfBookRuntimeRaw {
        symbol,
        update_id,
        best_bid,
        best_ask,
        bid_size,
        ask_size,
        observed_at,
    }))
}

pub(crate) fn parse_okx_wss_book_ticker_runtime_raw(
    raw_json: &str,
    ingested_at: UtcTimestamp,
    market: OkxPublicWssMarket,
) -> RuntimeResult<Option<PublicWssTickerRuntimeRaw>> {
    let root = parse_json_object_value_slices(raw_json)?;
    if let Some(event) = optional_json_value_string(&root, "event", "okx wss")? {
        if event == "subscribe" || event == "error" {
            return Ok(None);
        }
    }
    let Some(data) = root.get("data") else {
        return Ok(None);
    };
    let object = first_json_array_object(data, "okx wss tickers")?;
    let fields = parse_json_object_value_slices(object)?;
    let venue_symbol = required_json_value_string(&fields, "instId", "okx wss tickers")?;
    let symbol = okx_public_wss_symbol_from_inst_id(&venue_symbol, market)?;
    let observed_at = optional_json_scalar_u64(&fields, "ts", "okx wss tickers")?
        .map(timestamp_from_unix_millis)
        .transpose()?
        .unwrap_or(ingested_at);
    let observed_at = observed_at_not_after_ingested(observed_at, ingested_at)?;
    let bid_price = required_json_value_string(&fields, "bidPx", "okx wss tickers")?;
    let ask_price = required_json_value_string(&fields, "askPx", "okx wss tickers")?;
    let bid_qty = required_json_value_string(&fields, "bidSz", "okx wss tickers")?;
    let ask_qty = required_json_value_string(&fields, "askSz", "okx wss tickers")?;
    if [
        bid_price.as_str(),
        ask_price.as_str(),
        bid_qty.as_str(),
        ask_qty.as_str(),
    ]
    .iter()
    .any(|value| value.trim().is_empty())
    {
        return Ok(None);
    }
    Ok(Some(PublicWssTickerRuntimeRaw {
        symbol,
        venue_symbol,
        best_bid: Price::from_str(&bid_price)?,
        best_ask: Price::from_str(&ask_price)?,
        bid_size: Quantity::from_str(&bid_qty)?,
        ask_size: Quantity::from_str(&ask_qty)?,
        observed_at,
    }))
}

pub(crate) fn parse_bitget_wss_book_ticker_runtime_raw(
    raw_json: &str,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<Option<PublicWssTickerRuntimeRaw>> {
    let root = parse_json_object_value_slices(raw_json)?;
    if let Some(event) = optional_json_value_string(&root, "event", "bitget wss")? {
        if event == "subscribe" || event == "error" {
            return Ok(None);
        }
    }
    let Some(data) = root.get("data") else {
        return Ok(None);
    };
    let object = first_json_array_object(data, "bitget wss ticker")?;
    let fields = parse_json_object_value_slices(object)?;
    let venue_symbol =
        required_first_json_value_string(&fields, &["instId", "symbol"], "bitget wss ticker")?;
    let symbol = normalize_bitget_wss_symbol_scope(&venue_symbol)?;
    let observed_at = optional_json_scalar_u64(&fields, "ts", "bitget wss ticker")?
        .or(optional_json_scalar_u64(
            &fields,
            "systemTime",
            "bitget wss ticker",
        )?)
        .map(timestamp_from_unix_millis)
        .transpose()?
        .unwrap_or(ingested_at);
    let observed_at = observed_at_not_after_ingested(observed_at, ingested_at)?;
    if !bitget_wss_ticker_has_top_of_book_fields(&fields) {
        return Ok(None);
    }
    let Some(bid_price) = optional_first_bitget_wss_ticker_value_string(
        &fields,
        &["bidPr", "bidPx", "bidPrice"],
        "bitget wss ticker",
    )?
    else {
        return Ok(None);
    };
    let Some(ask_price) = optional_first_bitget_wss_ticker_value_string(
        &fields,
        &["askPr", "askPx", "askPrice"],
        "bitget wss ticker",
    )?
    else {
        return Ok(None);
    };
    let Some(bid_qty) = optional_first_bitget_wss_ticker_value_string(
        &fields,
        &["bidSz", "bidSize", "bidQty"],
        "bitget wss ticker",
    )?
    else {
        return Ok(None);
    };
    let Some(ask_qty) = optional_first_bitget_wss_ticker_value_string(
        &fields,
        &["askSz", "askSize", "askQty"],
        "bitget wss ticker",
    )?
    else {
        return Ok(None);
    };
    if [
        bid_price.as_str(),
        ask_price.as_str(),
        bid_qty.as_str(),
        ask_qty.as_str(),
    ]
    .iter()
    .any(|value| value.trim().is_empty())
    {
        return Ok(None);
    }
    let Ok(best_bid) = Price::from_str(&bid_price) else {
        return Ok(None);
    };
    let Ok(best_ask) = Price::from_str(&ask_price) else {
        return Ok(None);
    };
    let Ok(bid_size) = Quantity::from_str(&bid_qty) else {
        return Ok(None);
    };
    let Ok(ask_size) = Quantity::from_str(&ask_qty) else {
        return Ok(None);
    };
    Ok(Some(PublicWssTickerRuntimeRaw {
        symbol,
        venue_symbol,
        best_bid,
        best_ask,
        bid_size,
        ask_size,
        observed_at,
    }))
}

fn bitget_wss_ticker_has_top_of_book_fields(fields: &BTreeMap<String, &str>) -> bool {
    [
        &["bidPr", "bidPx", "bidPrice"][..],
        &["askPr", "askPx", "askPrice"][..],
        &["bidSz", "bidSize", "bidQty"][..],
        &["askSz", "askSize", "askQty"][..],
    ]
    .iter()
    .all(|names| names.iter().any(|name| fields.contains_key(*name)))
}

fn optional_first_bitget_wss_ticker_value_string(
    fields: &BTreeMap<String, &str>,
    field_names: &[&'static str],
    source: &'static str,
) -> RuntimeResult<Option<String>> {
    for field in field_names {
        let Some(value) = fields.get(*field) else {
            continue;
        };
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed == "null" {
            return Ok(None);
        }
        match bitget_ticker_value_to_string(trimmed, field, source) {
            Ok(value) if value.trim().is_empty() => return Ok(None),
            Ok(value) => return Ok(Some(value)),
            Err(error) if bitget_wss_ticker_value_error_is_unusable_quote(&error) => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

fn bitget_wss_ticker_value_error_is_unusable_quote(error: &RuntimeError) -> bool {
    let message = error.to_string();
    message.contains("is not a scalar string")
        || message.contains("is an ambiguous JSON array")
        || message.contains("JSON array nesting is too deep")
}

pub(crate) fn parse_aster_wss_book_ticker_runtime_raw(
    raw_json: &str,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<PublicTopOfBookRuntimeRaw> {
    parse_binance_wss_book_ticker_runtime_raw(raw_json, ingested_at)
}

pub(crate) fn parse_hyperliquid_wss_bbo_runtime_raw(
    raw_json: &str,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<Option<PublicTopOfBookRuntimeRaw>> {
    let root = parse_json_object_value_slices(raw_json)?;
    let channel = optional_json_value_string(&root, "channel", "hyperliquid wss")?;
    if matches!(
        channel.as_deref(),
        Some("subscriptionResponse") | Some("pong")
    ) {
        return Ok(None);
    }
    if !matches!(channel.as_deref(), Some("bbo") | Some("l2Book")) {
        return Ok(None);
    }
    let Some(data) = root.get("data") else {
        return Ok(None);
    };
    let fields = parse_json_object_value_slices(data)?;
    let coin = required_json_value_string(&fields, "coin", "hyperliquid wss bbo")?;
    let top_of_book = if channel.as_deref() == Some("l2Book") {
        let levels = fields
            .get("levels")
            .ok_or_else(|| RuntimeError::LiveMarketData {
                message: "Hyperliquid WSS l2Book lacks `levels` array".to_owned(),
            })?;
        hyperliquid_wss_l2_book_top_of_book(levels)?
    } else {
        let bbo = fields
            .get("bbo")
            .ok_or_else(|| RuntimeError::LiveMarketData {
                message: "Hyperliquid WSS bbo lacks `bbo` array".to_owned(),
            })?;
        hyperliquid_wss_bbo_top_of_book(bbo)?
    };
    let Some((best_bid, bid_size, best_ask, ask_size)) = top_of_book else {
        return Ok(None);
    };
    let update_id = optional_json_scalar_u64(&fields, "time", "hyperliquid wss bbo")?
        .or(optional_json_scalar_u64(
            &fields,
            "ts",
            "hyperliquid wss bbo",
        )?)
        .unwrap_or_else(|| {
            u64::try_from(runtime_timestamp_millis(ingested_at).unwrap_or(0)).unwrap_or(0)
        });
    let observed_at =
        observed_at_not_after_ingested(timestamp_from_unix_millis(update_id)?, ingested_at)?;
    Ok(Some(PublicTopOfBookRuntimeRaw {
        symbol: coin,
        update_id,
        best_bid,
        best_ask,
        bid_size,
        ask_size,
        observed_at,
    }))
}

pub(crate) fn parse_lighter_wss_ticker_runtime_raw(
    raw_json: &str,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<Option<PublicTopOfBookRuntimeRaw>> {
    let root = parse_json_object_value_slices(raw_json)?;
    let channel = optional_json_value_string(&root, "channel", "lighter wss")?;
    if channel
        .as_deref()
        .is_some_and(|channel| !channel.starts_with("ticker:") && !channel.starts_with("ticker/"))
    {
        return Ok(None);
    }
    if let Some(event_type) = optional_json_value_string(&root, "type", "lighter wss")? {
        if event_type == "error" {
            return Err(RuntimeError::LiveMarketData {
                message: "Lighter public WSS ticker returned error event".to_owned(),
            });
        }
        if event_type != "update/ticker"
            && !event_type.starts_with("subscribed/")
            && !event_type.starts_with("unsubscribed/")
        {
            return Ok(None);
        }
        if event_type != "update/ticker" {
            return Ok(None);
        }
    }
    let Some(ticker) = root.get("ticker") else {
        return Ok(None);
    };
    let fields = parse_json_object_value_slices(ticker)?;
    let symbol = required_first_json_value_string(&fields, &["s", "symbol"], "lighter wss ticker")?;
    let update_id = optional_json_scalar_u64(&root, "nonce", "lighter wss ticker")?
        .or(optional_json_scalar_u64(
            &fields,
            "nonce",
            "lighter wss ticker",
        )?)
        .or(optional_json_scalar_u64(
            &fields,
            "last_updated_at",
            "lighter wss ticker",
        )?)
        .or(optional_json_scalar_u64(
            &root,
            "last_updated_at",
            "lighter wss ticker",
        )?)
        .or(optional_json_scalar_u64(
            &root,
            "timestamp",
            "lighter wss ticker",
        )?)
        .unwrap_or_else(|| {
            u64::try_from(runtime_timestamp_millis(ingested_at).unwrap_or(0)).unwrap_or(0)
        });
    let Some((best_bid, bid_size)) = fields
        .get("b")
        .map(|level| lighter_wss_ticker_level(level, "bid"))
        .transpose()?
        .flatten()
    else {
        return Ok(None);
    };
    let Some((best_ask, ask_size)) = fields
        .get("a")
        .map(|level| lighter_wss_ticker_level(level, "ask"))
        .transpose()?
        .flatten()
    else {
        return Ok(None);
    };
    let observed_at = lighter_wss_timestamp(&root, &fields, ingested_at)?;
    Ok(Some(PublicTopOfBookRuntimeRaw {
        symbol,
        update_id,
        best_bid,
        best_ask,
        bid_size,
        ask_size,
        observed_at,
    }))
}

fn lighter_wss_ticker_level(
    value: &str,
    side: &'static str,
) -> RuntimeResult<Option<(Price, Quantity)>> {
    let value = value.trim();
    if value == "null" {
        return Ok(None);
    }
    let fields = parse_json_object_value_slices(value)?;
    let price = required_json_value_string(&fields, "price", "lighter wss ticker level")?;
    let size = required_json_value_string(&fields, "size", "lighter wss ticker level")?;
    let price = price.trim();
    let size = size.trim();
    if price.is_empty() || size.is_empty() {
        return Ok(None);
    }
    Ok(Some((
        Price::from_str(price).map_err(|error| RuntimeError::LiveMarketData {
            message: format!("Lighter WSS `{side}` price `{price}` is invalid: {error}"),
        })?,
        Quantity::from_str(size).map_err(|error| RuntimeError::LiveMarketData {
            message: format!("Lighter WSS `{side}` size `{size}` is invalid: {error}"),
        })?,
    )))
}

fn lighter_wss_timestamp(
    root: &BTreeMap<String, &str>,
    fields: &BTreeMap<String, &str>,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<UtcTimestamp> {
    let observed = optional_json_scalar_u64(root, "timestamp", "lighter wss ticker")?
        .map(timestamp_from_lighter_millis_or_micros)
        .transpose()?
        .or(
            optional_json_scalar_u64(root, "last_updated_at", "lighter wss ticker")?
                .map(timestamp_from_lighter_millis_or_micros)
                .transpose()?,
        )
        .or(
            optional_json_scalar_u64(fields, "last_updated_at", "lighter wss ticker")?
                .map(timestamp_from_lighter_millis_or_micros)
                .transpose()?,
        );
    observed_at_not_after_ingested(observed.unwrap_or(ingested_at), ingested_at)
}

fn timestamp_from_lighter_millis_or_micros(value: u64) -> RuntimeResult<UtcTimestamp> {
    if value >= 10_000_000_000_000 {
        timestamp_from_unix_millis(value / 1_000)
    } else {
        timestamp_from_unix_millis(value)
    }
}

pub(crate) fn parse_lighter_order_book_metadata_rows(
    input: &str,
) -> RuntimeResult<Vec<LighterOrderBookMetadataRow>> {
    let rows = lighter_order_book_metadata_object_slices(input)?
        .into_iter()
        .filter_map(|object| {
            let fields = parse_json_object_value_slices(object).ok()?;
            let market_id = required_first_json_value_string(
                &fields,
                &["market_id", "market_index", "id"],
                "lighter orderBooks metadata",
            )
            .ok()?;
            let symbol = required_first_json_value_string(
                &fields,
                &["symbol", "name", "market_symbol", "ticker"],
                "lighter orderBooks metadata",
            )
            .ok()
            .and_then(|symbol| validate_lighter_public_wss_symbol(&symbol).ok())?;
            Some(LighterOrderBookMetadataRow { market_id, symbol })
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: "Lighter orderBooks metadata contains no usable perp rows".to_owned(),
        });
    }
    Ok(rows)
}

fn lighter_order_book_metadata_object_slices(input: &str) -> RuntimeResult<Vec<&str>> {
    let trimmed = input.trim();
    if trimmed.starts_with('{') {
        let fields = parse_json_object_value_slices(trimmed)?;
        for key in ["order_books", "orderBooks", "data", "result", "items"] {
            if let Some(value) = fields.get(key) {
                if value.trim_start().starts_with('{') {
                    let nested = parse_json_object_value_slices(value)?;
                    for nested_key in ["order_books", "orderBooks", "data", "result", "items"] {
                        if let Some(nested_value) = nested.get(nested_key) {
                            return json_object_slices(nested_value);
                        }
                    }
                }
                return json_object_slices(value);
            }
        }
    }
    json_object_slices(trimmed)
}

pub(crate) fn parse_gate_wss_book_ticker_runtime_raw(
    raw_json: &str,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<Option<PublicTopOfBookRuntimeRaw>> {
    let root = parse_json_object_value_slices(raw_json)?;
    let channel = optional_json_value_string(&root, "channel", "gate wss")?;
    if channel.as_deref() != Some("futures.book_ticker") {
        return Ok(None);
    }
    let event = optional_json_value_string(&root, "event", "gate wss")?;
    match event.as_deref() {
        Some("update") | None => {}
        Some("subscribe") => {
            if gate_wss_subscribe_failed(&root)? {
                return Err(RuntimeError::LiveMarketData {
                    message: "Gate public WSS futures.book_ticker subscribe failed".to_owned(),
                });
            }
            return Ok(None);
        }
        Some(_) => return Ok(None),
    }
    let Some(result) = root.get("result") else {
        return Ok(None);
    };
    let fields = parse_json_object_value_slices(result)?;
    let symbol =
        required_first_json_value_string(&fields, &["s", "symbol"], "gate wss bookTicker")?;
    let update_id = optional_json_scalar_u64(&fields, "u", "gate wss bookTicker")?
        .or(optional_json_scalar_u64(
            &fields,
            "t",
            "gate wss bookTicker",
        )?)
        .or(optional_json_scalar_u64(
            &root,
            "time_ms",
            "gate wss bookTicker",
        )?)
        .or(optional_json_scalar_u64(
            &root,
            "time",
            "gate wss bookTicker",
        )?)
        .unwrap_or_else(|| {
            u64::try_from(runtime_timestamp_millis(ingested_at).unwrap_or(0)).unwrap_or(0)
        });
    let bid_price =
        required_first_json_value_string(&fields, &["b", "bid"], "gate wss bookTicker")?;
    let bid_qty =
        required_first_json_value_string(&fields, &["B", "bid_size"], "gate wss bookTicker")?;
    let ask_price =
        required_first_json_value_string(&fields, &["a", "ask"], "gate wss bookTicker")?;
    let ask_qty =
        required_first_json_value_string(&fields, &["A", "ask_size"], "gate wss bookTicker")?;
    if [
        bid_price.as_str(),
        bid_qty.as_str(),
        ask_price.as_str(),
        ask_qty.as_str(),
    ]
    .iter()
    .any(|value| value.trim().is_empty())
    {
        return Ok(None);
    }
    let observed_at = optional_json_scalar_u64(&fields, "t", "gate wss bookTicker")?
        .or(optional_json_scalar_u64(
            &root,
            "time_ms",
            "gate wss bookTicker",
        )?)
        .map(timestamp_from_gate_seconds_or_millis)
        .transpose()?
        .unwrap_or(ingested_at);
    let observed_at = observed_at_not_after_ingested(observed_at, ingested_at)?;
    Ok(Some(PublicTopOfBookRuntimeRaw {
        symbol,
        update_id,
        best_bid: Price::from_str(&bid_price)?,
        best_ask: Price::from_str(&ask_price)?,
        bid_size: Quantity::from_str(&bid_qty)?,
        ask_size: Quantity::from_str(&ask_qty)?,
        observed_at,
    }))
}

fn gate_wss_subscribe_failed(fields: &BTreeMap<String, &str>) -> RuntimeResult<bool> {
    let Some(result) = fields.get("result") else {
        return Ok(false);
    };
    if result.trim() == "null" {
        return Ok(false);
    }
    let result_fields = parse_json_object_value_slices(result)?;
    if let Some(status) =
        optional_json_value_string(&result_fields, "status", "gate wss subscribe")?
    {
        return Ok(!status.eq_ignore_ascii_case("success"));
    }
    Ok(false)
}

fn timestamp_from_gate_seconds_or_millis(value: u64) -> RuntimeResult<UtcTimestamp> {
    if value < 10_000_000_000 {
        timestamp_from_unix_millis(value.saturating_mul(1_000))
    } else {
        timestamp_from_unix_millis(value)
    }
}

fn hyperliquid_wss_bbo_top_of_book(
    value: &str,
) -> RuntimeResult<Option<(Price, Quantity, Price, Quantity)>> {
    let levels = json_array_value_slices(value)?;
    if levels.len() < 2 {
        return Err(RuntimeError::LiveMarketData {
            message: "Hyperliquid WSS bbo must contain bid and ask levels".to_owned(),
        });
    }
    let Some((best_bid, bid_size)) = hyperliquid_wss_optional_level(levels[0], "bid")? else {
        return Ok(None);
    };
    let Some((best_ask, ask_size)) = hyperliquid_wss_optional_level(levels[1], "ask")? else {
        return Ok(None);
    };
    Ok(Some((best_bid, bid_size, best_ask, ask_size)))
}

fn hyperliquid_wss_l2_book_top_of_book(
    value: &str,
) -> RuntimeResult<Option<(Price, Quantity, Price, Quantity)>> {
    let sides = json_array_value_slices(value)?;
    if sides.len() < 2 {
        return Err(RuntimeError::LiveMarketData {
            message: "Hyperliquid WSS l2Book must contain bid and ask sides".to_owned(),
        });
    }
    let bids = json_array_value_slices(sides[0])?;
    let asks = json_array_value_slices(sides[1])?;
    let Some(bid) = bids.first() else {
        return Ok(None);
    };
    let Some(ask) = asks.first() else {
        return Ok(None);
    };
    let (best_bid, bid_size) = hyperliquid_wss_bbo_level(bid, "bid")?;
    let (best_ask, ask_size) = hyperliquid_wss_bbo_level(ask, "ask")?;
    Ok(Some((best_bid, bid_size, best_ask, ask_size)))
}

fn hyperliquid_wss_optional_level(
    value: &str,
    side: &'static str,
) -> RuntimeResult<Option<(Price, Quantity)>> {
    if value.trim() == "null" {
        return Ok(None);
    }
    hyperliquid_wss_bbo_level(value, side).map(Some)
}

pub(crate) fn hyperliquid_wss_bbo_level(
    value: &str,
    side: &'static str,
) -> RuntimeResult<(Price, Quantity)> {
    let value = value.trim();
    if value == "null" {
        return Err(RuntimeError::LiveMarketData {
            message: format!("Hyperliquid WSS bbo `{side}` side is null"),
        });
    }
    if value.starts_with('[') {
        let fields = json_array_value_slices(value)?;
        if fields.len() < 2 {
            return Err(RuntimeError::LiveMarketData {
                message: format!("Hyperliquid WSS bbo `{side}` array lacks price or size"),
            });
        }
        let price = decode_json_scalar_string(fields[0], "hyperliquid wss bbo price")?;
        let size = decode_json_scalar_string(fields[1], "hyperliquid wss bbo size")?;
        return Ok((Price::from_str(&price)?, Quantity::from_str(&size)?));
    }
    let fields = parse_json_object_value_slices(value)?;
    let price = required_first_json_value_string(&fields, &["px", "price"], "hyperliquid wss bbo")?;
    let size = required_first_json_value_string(&fields, &["sz", "size"], "hyperliquid wss bbo")?;
    Ok((Price::from_str(&price)?, Quantity::from_str(&size)?))
}

pub(crate) fn first_json_array_object<'a>(
    value: &'a str,
    source: &'static str,
) -> RuntimeResult<&'a str> {
    json_array_value_slices(value)?
        .into_iter()
        .find(|entry| entry.trim_start().starts_with('{'))
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("{source} data array has no object"),
        })
}

pub(crate) fn bybit_wss_first_price_size(
    value: &str,
    side: &'static str,
) -> RuntimeResult<Option<(Price, Quantity)>> {
    let levels = json_array_value_slices(value)?;
    let Some(first) = levels.first() else {
        return Ok(None);
    };
    let first = first.trim();
    let fields = json_array_value_slices(first)?;
    if fields.len() < 2 {
        return Err(RuntimeError::LiveMarketData {
            message: format!("Bybit WSS orderbook `{side}` level lacks price or size"),
        });
    }
    let price = decode_json_scalar_string(fields[0], "bybit wss price")?;
    let size = decode_json_scalar_string(fields[1], "bybit wss size")?;
    bybit_wss_optional_price_size(&price, &size, side)
}

pub(crate) fn bybit_wss_optional_ticker_price_size(
    fields: &BTreeMap<String, &str>,
    price_field: &'static str,
    size_field: &'static str,
    side: &'static str,
) -> RuntimeResult<Option<(Price, Quantity)>> {
    let price = optional_json_value_string(fields, price_field, "bybit wss ticker")?;
    let size = optional_json_value_string(fields, size_field, "bybit wss ticker")?;
    let (Some(price), Some(size)) = (price, size) else {
        return Ok(None);
    };
    bybit_wss_optional_price_size(&price, &size, side)
}

pub(crate) fn bybit_wss_optional_price_size(
    price: &str,
    size: &str,
    side: &'static str,
) -> RuntimeResult<Option<(Price, Quantity)>> {
    let price = price.trim();
    let size = size.trim();
    if price.is_empty() || size.is_empty() {
        return Ok(None);
    }
    Ok(Some((
        Price::from_str(price).map_err(|error| RuntimeError::LiveMarketData {
            message: format!("Bybit WSS `{side}` price `{price}` is invalid: {error}"),
        })?,
        Quantity::from_str(size).map_err(|error| RuntimeError::LiveMarketData {
            message: format!("Bybit WSS `{side}` size `{size}` is invalid: {error}"),
        })?,
    )))
}

pub(crate) fn optional_json_scalar_u64(
    fields: &BTreeMap<String, &str>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<Option<u64>> {
    let Some(value) = fields.get(field) else {
        return Ok(None);
    };
    decode_json_scalar_string(value, source)?
        .parse::<u64>()
        .map(Some)
        .map_err(|error| RuntimeError::LiveMarketData {
            message: format!("{source} field `{field}` is not u64: {error}"),
        })
}

pub(crate) fn optional_json_scalar_bool(
    fields: &BTreeMap<String, &str>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<Option<bool>> {
    let Some(value) = optional_json_value_string(fields, field, source)? else {
        return Ok(None);
    };
    match value.as_str() {
        "true" => Ok(Some(true)),
        "false" => Ok(Some(false)),
        other => Err(RuntimeError::LiveMarketData {
            message: format!("{source} field `{field}` is not bool: `{other}`"),
        }),
    }
}

pub(crate) fn decode_json_scalar_string(
    value: &str,
    source: &'static str,
) -> RuntimeResult<String> {
    let value = value.trim();
    if value.starts_with('"') {
        let quote_end = json_string_end(value, 0)?;
        decode_json_string_literal(&value[1..quote_end - 1])
    } else if value.starts_with('{') || value.starts_with('[') {
        Err(RuntimeError::LiveMarketData {
            message: format!("{source} expected scalar but received nested JSON"),
        })
    } else {
        Ok(value.trim_end_matches(',').to_owned())
    }
}

pub(crate) fn parse_binance_wss_book_ticker_runtime_raw(
    raw_json: &str,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<PublicTopOfBookRuntimeRaw> {
    let payload = binance_wss_book_ticker_payload_json(raw_json)?;
    let fields = parse_flat_json_object(payload)?;
    if let Some(MonitorJsonScalar::String(event_type)) = fields.get("e") {
        if event_type != "bookTicker" {
            return Err(RuntimeError::LiveMarketData {
                message: format!("WSS event type `{event_type}` is not bookTicker"),
            });
        }
    }
    let observed_at = optional_binance_wss_millis(&fields, "T")?
        .or(optional_binance_wss_millis(&fields, "E")?)
        .map(timestamp_from_unix_millis)
        .transpose()?
        .unwrap_or(ingested_at);
    let observed_at = observed_at_not_after_ingested(observed_at, ingested_at)?;
    Ok(PublicTopOfBookRuntimeRaw {
        symbol: required_json_string(&fields, "s", "binance wss bookTicker")?,
        update_id: required_json_string(&fields, "u", "binance wss bookTicker")?
            .parse::<u64>()
            .map_err(|_| RuntimeError::LiveMarketData {
                message: "Binance WSS bookTicker field `u` must be u64".to_owned(),
            })?,
        best_bid: Price::from_str(&required_json_string(
            &fields,
            "b",
            "binance wss bookTicker",
        )?)?,
        best_ask: Price::from_str(&required_json_string(
            &fields,
            "a",
            "binance wss bookTicker",
        )?)?,
        bid_size: Quantity::from_str(&required_json_string(
            &fields,
            "B",
            "binance wss bookTicker",
        )?)?,
        ask_size: Quantity::from_str(&required_json_string(
            &fields,
            "A",
            "binance wss bookTicker",
        )?)?,
        observed_at,
    })
}

pub(crate) fn binance_wss_book_ticker_payload_json(raw_json: &str) -> RuntimeResult<&str> {
    let fields = parse_json_object_value_slices(raw_json)?;
    match fields.get("data") {
        Some(data) => Ok(data.trim()),
        None => Ok(raw_json.trim()),
    }
}

pub(crate) fn optional_binance_wss_millis(
    fields: &BTreeMap<String, MonitorJsonScalar>,
    field: &'static str,
) -> RuntimeResult<Option<u64>> {
    match fields.get(field) {
        Some(MonitorJsonScalar::String(value)) | Some(MonitorJsonScalar::Number(value)) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| RuntimeError::LiveMarketData {
                message: format!("Binance WSS bookTicker field `{field}` must be u64 millis"),
            }),
        Some(MonitorJsonScalar::Null) | None => Ok(None),
        Some(MonitorJsonScalar::Bool(_)) => Err(RuntimeError::LiveMarketData {
            message: format!("Binance WSS bookTicker field `{field}` must be millis"),
        }),
    }
}

pub(crate) fn timestamp_from_unix_millis(value: u64) -> RuntimeResult<UtcTimestamp> {
    let seconds_u64 = value / 1_000;
    let millis = value % 1_000;
    let seconds = i64::try_from(seconds_u64).map_err(|_| RuntimeError::LiveMarketData {
        message: format!("Unix millis `{value}` does not fit i64 seconds"),
    })?;
    let nanos = u32::try_from(millis * 1_000_000).map_err(|_| RuntimeError::LiveMarketData {
        message: format!("Unix millis `{value}` does not fit nanoseconds"),
    })?;
    Ok(UtcTimestamp::from_unix_parts(seconds, nanos)?)
}

pub(crate) fn observed_at_not_after_ingested(
    observed_at: UtcTimestamp,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<UtcTimestamp> {
    if runtime_timestamp_millis(observed_at)? > runtime_timestamp_millis(ingested_at)? {
        Ok(ingested_at)
    } else {
        Ok(observed_at)
    }
}

pub(crate) fn binance_wss_book_ticker_all_market_stream_url(
    market: BinancePublicMarket,
    symbols: &[String],
    all_symbols_scope: bool,
) -> RuntimeResult<String> {
    match market {
        BinancePublicMarket::Spot => {
            if symbols.len() > 1_024 {
                return Err(RuntimeError::LiveMarketData {
                    message: format!(
                        "Binance spot combined stream supports at most 1024 streams; got {}",
                        symbols.len()
                    ),
                });
            }
            let streams = symbols
                .iter()
                .map(|symbol| format!("{}@bookTicker", symbol.to_ascii_lowercase()))
                .collect::<Vec<_>>()
                .join("/");
            Ok(format!(
                "wss://data-stream.binance.vision/stream?streams={streams}"
            ))
        }
        BinancePublicMarket::UsdmPerpetual => {
            if binance_usdm_wss_book_ticker_uses_all_market_stream(
                market,
                symbols,
                all_symbols_scope,
            ) {
                Ok("wss://fstream.binance.com/public/ws/!bookTicker".to_owned())
            } else if symbols.len() == 1 {
                Ok(format!(
                    "wss://fstream.binance.com/public/ws/{}@bookTicker",
                    symbols[0].to_ascii_lowercase()
                ))
            } else {
                if symbols.len() > 1_024 {
                    return Err(RuntimeError::LiveMarketData {
                        message: format!(
                            "Binance USD-M combined stream supports at most 1024 streams; got {}",
                            symbols.len()
                        ),
                    });
                }
                let streams = symbols
                    .iter()
                    .map(|symbol| format!("{}@bookTicker", symbol.to_ascii_lowercase()))
                    .collect::<Vec<_>>()
                    .join("/");
                Ok(format!(
                    "wss://fstream.binance.com/stream?streams={streams}"
                ))
            }
        }
    }
}

pub(crate) fn binance_usdm_wss_book_ticker_uses_all_market_stream(
    market: BinancePublicMarket,
    symbols: &[String],
    all_symbols_scope: bool,
) -> bool {
    matches!(market, BinancePublicMarket::UsdmPerpetual) && (all_symbols_scope || symbols.len() > 1)
}

pub(crate) fn aster_wss_book_ticker_stream_url(
    market: AsterPublicWssMarket,
    symbol_scope: &str,
) -> RuntimeResult<String> {
    match market {
        AsterPublicWssMarket::UsdtFutures => {
            if is_aster_wss_all_symbols_scope(symbol_scope) {
                Ok(format!("{ASTER_PUBLIC_WSS_BASE_URL}/ws/!bookTicker"))
            } else {
                let symbol_scope = normalize_aster_wss_symbol_scope(symbol_scope)?;
                let symbols =
                    explicit_wss_symbol_scope_set(&symbol_scope, is_aster_wss_all_symbols_scope)
                        .unwrap_or_default();
                if symbols.len() == 1 {
                    let symbol = symbols.iter().next().expect("symbol set is non-empty");
                    Ok(format!(
                        "{ASTER_PUBLIC_WSS_BASE_URL}/ws/{}@bookTicker",
                        symbol.to_ascii_lowercase()
                    ))
                } else {
                    Ok(format!("{ASTER_PUBLIC_WSS_BASE_URL}/ws/!bookTicker"))
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn hyperliquid_wss_bbo_subscribe_payload(coin: &str) -> RuntimeResult<String> {
    let coin = validate_hyperliquid_public_wss_coin(coin)?;
    Ok(format!(
        "{{\"method\":\"subscribe\",\"subscription\":{{\"type\":\"bbo\",\"coin\":{}}}}}",
        json_string(&coin)
    ))
}

pub(crate) fn hyperliquid_wss_top_of_book_subscribe_payload(coin: &str) -> RuntimeResult<String> {
    let coin = validate_hyperliquid_public_wss_coin(coin)?;
    Ok(format!(
        "{{\"method\":\"subscribe\",\"subscription\":{{\"type\":\"l2Book\",\"coin\":{}}}}}",
        json_string(&coin)
    ))
}

pub(crate) fn lighter_order_books_url(market: LighterPublicWssMarket) -> String {
    match market {
        LighterPublicWssMarket::Perp => {
            "https://mainnet.zklighter.elliot.ai/api/v1/orderBooks?market_id=255&filter=perp"
                .to_owned()
        }
    }
}

pub(crate) fn public_wss_reconnect_backoff(base_secs: u64, consecutive_failures: u32) -> Duration {
    let jitter_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::from(duration.subsec_nanos()))
        .unwrap_or(0);
    public_wss_reconnect_backoff_with_jitter_seed(base_secs, consecutive_failures, jitter_seed)
}

fn public_wss_monitor_cycle_result_after_read_error<E>(
    error: E,
    observed_wss_event: bool,
) -> RuntimeResult<()>
where
    E: Into<RuntimeError>,
{
    if observed_wss_event {
        Ok(())
    } else {
        Err(error.into())
    }
}

fn public_wss_reconnect_backoff_with_jitter_seed(
    base_secs: u64,
    consecutive_failures: u32,
    jitter_seed: u64,
) -> Duration {
    let shift = consecutive_failures.saturating_sub(1).min(5);
    let multiplier = 1_u64.checked_shl(shift).unwrap_or(32);
    let base_secs = base_secs.max(1);
    let seconds = base_secs
        .saturating_mul(multiplier)
        .min(PUBLIC_WSS_RECONNECT_BACKOFF_MAX_SECS);
    let jitter_window_secs = base_secs.min(5);
    let jitter_secs = if seconds >= PUBLIC_WSS_RECONNECT_BACKOFF_MAX_SECS || jitter_window_secs <= 1
    {
        0
    } else {
        jitter_seed % jitter_window_secs
    };
    Duration::from_secs(
        seconds
            .saturating_add(jitter_secs)
            .min(PUBLIC_WSS_RECONNECT_BACKOFF_MAX_SECS),
    )
}

#[derive(Debug)]
struct PublicWssCycleFailureLogger {
    label: &'static str,
    last_message: Option<String>,
    repeated_count: u32,
}

impl PublicWssCycleFailureLogger {
    fn new(label: &'static str) -> Self {
        Self {
            label,
            last_message: None,
            repeated_count: 0,
        }
    }

    fn record_success(&mut self) {
        self.last_message = None;
        self.repeated_count = 0;
    }

    fn record_failure(
        &mut self,
        error: &impl Display,
        consecutive_failures: u32,
        reconnect_backoff: Duration,
    ) {
        let message = error.to_string();
        let changed = self.last_message.as_deref() != Some(message.as_str());
        if changed {
            self.last_message = Some(message.clone());
            self.repeated_count = 1;
        } else {
            self.repeated_count = self.repeated_count.saturating_add(1);
        }
        if public_wss_should_log_failure(changed, self.repeated_count) {
            eprintln!(
                "{} cycle failed: {}; consecutive_failures={} repeated_count={} next_reconnect_delay_secs={}",
                self.label,
                message,
                consecutive_failures,
                self.repeated_count,
                reconnect_backoff.as_secs()
            );
        }
    }
}

fn public_wss_should_log_failure(message_changed: bool, repeated_count: u32) -> bool {
    message_changed
        || repeated_count == 1
        || repeated_count % PUBLIC_WSS_FAILURE_LOG_REPEAT_INTERVAL == 0
}

pub(crate) fn validate_binance_wss_probe_options(
    options: &BinanceWssBookTickerProbeOptions,
) -> RuntimeResult<()> {
    if options.bind_addr.trim().is_empty() {
        return Err(cli_arg_error("--bind must not be empty"));
    }
    if options.updates == 0 {
        return Err(cli_arg_error("--updates must be greater than zero"));
    }
    if options.reconnect_delay_secs == 0 {
        return Err(cli_arg_error(
            "--reconnect-delay-secs must be greater than zero",
        ));
    }
    normalize_binance_wss_symbol_scope(&options.symbol)?;
    Ok(())
}

pub(crate) fn validate_bybit_wss_probe_options(
    options: &BybitWssBookTickerProbeOptions,
) -> RuntimeResult<()> {
    if options.bind_addr.trim().is_empty() {
        return Err(cli_arg_error("--bind must not be empty"));
    }
    if options.updates == 0 {
        return Err(cli_arg_error("--updates must be greater than zero"));
    }
    if options.reconnect_delay_secs == 0 {
        return Err(cli_arg_error(
            "--reconnect-delay-secs must be greater than zero",
        ));
    }
    normalize_bybit_wss_symbol_scope(&options.symbol)?;
    Ok(())
}

pub(crate) fn validate_okx_wss_probe_options(
    options: &OkxWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    if options.bind_addr.trim().is_empty() {
        return Err(cli_arg_error("--bind must not be empty"));
    }
    if options.updates == 0 {
        return Err(cli_arg_error("--updates must be greater than zero"));
    }
    if options.reconnect_delay_secs == 0 {
        return Err(cli_arg_error(
            "--reconnect-delay-secs must be greater than zero",
        ));
    }
    normalize_okx_wss_symbol_scope(&options.symbol)?;
    Ok(())
}

pub(crate) fn validate_bitget_wss_probe_options(
    options: &BitgetWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    if options.bind_addr.trim().is_empty() {
        return Err(cli_arg_error("--bind must not be empty"));
    }
    if options.updates == 0 {
        return Err(cli_arg_error("--updates must be greater than zero"));
    }
    if options.reconnect_delay_secs == 0 {
        return Err(cli_arg_error(
            "--reconnect-delay-secs must be greater than zero",
        ));
    }
    normalize_bitget_wss_symbol_scope(&options.symbol)?;
    Ok(())
}

pub(crate) fn validate_aster_wss_probe_options(
    options: &AsterWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    if options.bind_addr.trim().is_empty() {
        return Err(cli_arg_error("--bind must not be empty"));
    }
    if options.updates == 0 {
        return Err(cli_arg_error("--updates must be greater than zero"));
    }
    if options.reconnect_delay_secs == 0 {
        return Err(cli_arg_error(
            "--reconnect-delay-secs must be greater than zero",
        ));
    }
    normalize_aster_wss_symbol_scope(&options.symbol)?;
    Ok(())
}

pub(crate) fn validate_hyperliquid_wss_probe_options(
    options: &HyperliquidWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    if options.bind_addr.trim().is_empty() {
        return Err(cli_arg_error("--bind must not be empty"));
    }
    if options.updates == 0 {
        return Err(cli_arg_error("--updates must be greater than zero"));
    }
    if options.reconnect_delay_secs == 0 {
        return Err(cli_arg_error(
            "--reconnect-delay-secs must be greater than zero",
        ));
    }
    normalize_hyperliquid_wss_symbol_scope(&options.symbol)?;
    Ok(())
}

pub(crate) fn validate_lighter_wss_probe_options(
    options: &LighterWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    if options.bind_addr.trim().is_empty() {
        return Err(cli_arg_error("--bind must not be empty"));
    }
    if options.updates == 0 {
        return Err(cli_arg_error("--updates must be greater than zero"));
    }
    if options.reconnect_delay_secs == 0 {
        return Err(cli_arg_error(
            "--reconnect-delay-secs must be greater than zero",
        ));
    }
    normalize_lighter_wss_symbol_scope(&options.symbol)?;
    Ok(())
}

pub(crate) fn validate_gate_wss_probe_options(
    options: &GateWssBookTickerMonitorOptions,
) -> RuntimeResult<()> {
    if options.bind_addr.trim().is_empty() {
        return Err(cli_arg_error("--bind must not be empty"));
    }
    if options.updates == 0 {
        return Err(cli_arg_error("--updates must be greater than zero"));
    }
    if options.reconnect_delay_secs == 0 {
        return Err(cli_arg_error(
            "--reconnect-delay-secs must be greater than zero",
        ));
    }
    normalize_gate_wss_symbol_scope(&options.symbol)?;
    Ok(())
}

pub(crate) fn normalize_binance_wss_symbol_scope(symbol: &str) -> RuntimeResult<String> {
    normalize_wss_symbol_scope_list(
        symbol,
        BINANCE_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS,
        is_binance_wss_all_symbols_scope,
        validate_binance_public_wss_symbol,
    )
}

pub(crate) fn is_binance_wss_all_symbols_scope(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "ALL" | "ALL_USDT" | "*"
    )
}

pub(crate) fn normalize_bybit_wss_symbol_scope(symbol: &str) -> RuntimeResult<String> {
    normalize_wss_symbol_scope_list(
        symbol,
        BYBIT_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS,
        is_bybit_wss_all_symbols_scope,
        validate_bybit_public_wss_symbol,
    )
}

pub(crate) fn is_bybit_wss_all_symbols_scope(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "ALL" | "ALL_USDT" | "*"
    )
}

pub(crate) fn normalize_okx_wss_symbol_scope(symbol: &str) -> RuntimeResult<String> {
    normalize_wss_symbol_scope_list(
        symbol,
        OKX_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS,
        is_okx_wss_all_symbols_scope,
        normalize_okx_usdt_basis_symbol,
    )
}

pub(crate) fn is_okx_wss_all_symbols_scope(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "ALL" | "ALL_USDT" | "*"
    )
}

pub(crate) fn normalize_bitget_wss_symbol_scope(symbol: &str) -> RuntimeResult<String> {
    normalize_wss_symbol_scope_list(
        symbol,
        BITGET_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS,
        is_bitget_wss_all_symbols_scope,
        |value| normalize_cex_usdt_basis_symbol(value, "Bitget"),
    )
}

pub(crate) fn is_bitget_wss_all_symbols_scope(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "ALL" | "ALL_USDT" | "*"
    )
}

pub(crate) fn normalize_aster_wss_symbol_scope(symbol: &str) -> RuntimeResult<String> {
    normalize_wss_symbol_scope_list(
        symbol,
        ASTER_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS,
        is_aster_wss_all_symbols_scope,
        validate_aster_public_wss_symbol,
    )
}

pub(crate) fn is_aster_wss_all_symbols_scope(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "ALL" | "ALL_USDT" | "*"
    )
}

pub(crate) fn normalize_hyperliquid_wss_symbol_scope(symbol: &str) -> RuntimeResult<String> {
    normalize_wss_symbol_scope_list(
        symbol,
        HYPERLIQUID_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS,
        is_hyperliquid_wss_all_symbols_scope,
        validate_hyperliquid_public_wss_coin,
    )
}

pub(crate) fn normalize_lighter_wss_symbol_scope(symbol: &str) -> RuntimeResult<String> {
    normalize_wss_symbol_scope_list(
        symbol,
        LIGHTER_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS,
        is_lighter_wss_all_symbols_scope,
        validate_lighter_public_wss_symbol,
    )
}

pub(crate) fn normalize_gate_wss_symbol_scope(symbol: &str) -> RuntimeResult<String> {
    normalize_wss_symbol_scope_list(
        symbol,
        GATE_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS,
        is_gate_wss_all_symbols_scope,
        validate_gate_public_wss_symbol,
    )
}

pub(crate) fn is_hyperliquid_wss_all_symbols_scope(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "ALL" | "ALL_USDT" | "*"
    )
}

pub(crate) fn is_lighter_wss_all_symbols_scope(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "ALL" | "ALL_USDT" | "*"
    )
}

pub(crate) fn is_gate_wss_all_symbols_scope(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "ALL" | "ALL_USDT" | "*"
    )
}

fn normalize_wss_symbol_scope_list<F, A>(
    symbol_scope: &str,
    all_symbol_scope: &str,
    is_all_scope: A,
    normalize_symbol: F,
) -> RuntimeResult<String>
where
    F: Fn(&str) -> RuntimeResult<String>,
    A: Fn(&str) -> bool,
{
    let parts = symbol_scope
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Err(cli_arg_error("WSS symbol scope must not be empty"));
    }
    if parts.len() == 1 && is_all_scope(parts[0]) {
        return Ok(all_symbol_scope.to_owned());
    }
    if parts.iter().any(|part| is_all_scope(part)) {
        return Err(cli_arg_error(
            "WSS symbol scope cannot mix ALL/ALL_USDT/* with explicit symbols",
        ));
    }

    let mut symbols = BTreeSet::new();
    for part in parts {
        symbols.insert(normalize_symbol(part)?);
    }
    Ok(symbols.into_iter().collect::<Vec<_>>().join(","))
}

fn explicit_wss_symbol_scope_set<A>(symbol_scope: &str, is_all_scope: A) -> Option<BTreeSet<String>>
where
    A: Fn(&str) -> bool,
{
    if is_all_scope(symbol_scope) {
        return None;
    }
    Some(
        symbol_scope
            .split(',')
            .map(str::trim)
            .filter(|symbol| !symbol.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

fn wss_symbol_in_scope(
    symbol: &str,
    all_symbols_scope: bool,
    target_symbols: Option<&BTreeSet<String>>,
) -> bool {
    all_symbols_scope
        || target_symbols
            .map(|symbols| symbols.contains(symbol))
            .unwrap_or(false)
}

fn ensure_wss_requested_symbols_present(
    source: &str,
    target_symbols: Option<&BTreeSet<String>>,
    available_symbols: BTreeSet<String>,
) -> RuntimeResult<()> {
    let Some(target_symbols) = target_symbols else {
        return Ok(());
    };
    if target_symbols.len() > PUBLIC_WSS_BROAD_EXPLICIT_SYMBOL_SCOPE_MIN_SYMBOLS {
        return Ok(());
    }
    let missing = target_symbols
        .difference(&available_symbols)
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    Err(RuntimeError::LiveMarketData {
        message: format!(
            "{source} missing requested symbol(s): {}",
            missing.join(",")
        ),
    })
}

pub(crate) fn validate_binance_public_wss_symbol(symbol: &str) -> RuntimeResult<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol.len() < 3 || symbol.len() > 64 {
        return Err(cli_arg_error("Binance WSS symbol length must be 3..=64"));
    }
    if !symbol.chars().all(|ch| {
        ch.is_ascii_uppercase()
            || ch.is_ascii_digit()
            || (!ch.is_ascii() && !ch.is_control() && !ch.is_whitespace())
    }) {
        return Err(cli_arg_error(
            "Binance WSS symbol must contain uppercase ASCII letters/digits or non-ASCII symbol characters without whitespace",
        ));
    }
    if !symbol.ends_with("USDT") {
        return Err(cli_arg_error(
            "current Binance WSS probe maps only USDT-quoted public instruments",
        ));
    }
    Ok(symbol)
}

pub(crate) fn validate_bybit_public_wss_symbol(symbol: &str) -> RuntimeResult<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol.len() < 3 || symbol.len() > 32 {
        return Err(cli_arg_error("Bybit WSS symbol length must be 3..=32"));
    }
    if !symbol
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(cli_arg_error(
            "Bybit WSS symbol must contain only uppercase ASCII letters and digits",
        ));
    }
    if !symbol.ends_with("USDT") {
        return Err(cli_arg_error(
            "current Bybit WSS monitor maps only USDT-quoted public instruments",
        ));
    }
    Ok(symbol)
}

pub(crate) fn validate_aster_public_wss_symbol(symbol: &str) -> RuntimeResult<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol.len() < 3 || symbol.len() > 64 {
        return Err(cli_arg_error("Aster WSS symbol length must be 3..=64"));
    }
    if !symbol
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(cli_arg_error(
            "Aster WSS symbol must contain only uppercase ASCII letters and digits",
        ));
    }
    if !symbol.ends_with("USDT") || symbol.starts_with("TEST") {
        return Err(cli_arg_error(
            "current Aster WSS monitor maps only non-test USDT-quoted futures",
        ));
    }
    Ok(symbol)
}

pub(crate) fn validate_hyperliquid_public_wss_coin(symbol: &str) -> RuntimeResult<String> {
    let trimmed = symbol.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err(cli_arg_error("Hyperliquid WSS coin length must be 1..=64"));
    }
    let coin = trimmed
        .strip_suffix("USDT")
        .or_else(|| trimmed.strip_suffix("-USDT"))
        .unwrap_or(trimmed);
    if coin.is_empty()
        || !coin
            .chars()
            .all(|ch| !ch.is_control() && !ch.is_whitespace() && ch != '"' && ch != '\\')
    {
        return Err(cli_arg_error(
            "Hyperliquid WSS coin must not contain whitespace, quotes, or control characters",
        ));
    }
    Ok(coin.to_owned())
}

pub(crate) fn validate_lighter_public_wss_symbol(symbol: &str) -> RuntimeResult<String> {
    let trimmed = symbol.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err(cli_arg_error("Lighter WSS symbol length must be 1..=64"));
    }
    let uppercase = trimmed.to_ascii_uppercase();
    let symbol = uppercase
        .strip_suffix("-USDT")
        .or_else(|| uppercase.strip_suffix("_USDT"))
        .or_else(|| uppercase.strip_suffix("/USDT"))
        .or_else(|| uppercase.strip_suffix("USDT"))
        .or_else(|| uppercase.strip_suffix("-USD"))
        .or_else(|| uppercase.strip_suffix("_USD"))
        .or_else(|| uppercase.strip_suffix("/USD"))
        .or_else(|| uppercase.strip_suffix("USD"))
        .unwrap_or(&uppercase)
        .to_owned();
    if symbol.is_empty()
        || !symbol
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(cli_arg_error(
            "Lighter WSS symbol must contain only uppercase ASCII letters and digits",
        ));
    }
    Ok(symbol)
}

pub(crate) fn validate_gate_public_wss_symbol(symbol: &str) -> RuntimeResult<String> {
    let mut symbol = symbol.trim().to_ascii_uppercase().replace('-', "_");
    if !symbol.contains('_') && symbol.ends_with("USDT") && symbol.len() > 4 {
        let base = symbol.trim_end_matches("USDT");
        symbol = format!("{base}_USDT");
    }
    if symbol.len() < 6 || symbol.len() > 64 {
        return Err(cli_arg_error("Gate WSS symbol length must be 6..=64"));
    }
    if !symbol
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(cli_arg_error(
            "Gate WSS symbol must contain only uppercase ASCII letters, digits, and underscore",
        ));
    }
    if !symbol.ends_with("_USDT") || symbol.starts_with("TEST_") {
        return Err(cli_arg_error(
            "current Gate WSS monitor maps only non-test USDT futures contracts",
        ));
    }
    Ok(symbol)
}

pub(crate) fn parse_binance_public_wss_market(value: &str) -> RuntimeResult<BinancePublicMarket> {
    match value.trim().to_ascii_lowercase().as_str() {
        "spot" => Ok(BinancePublicMarket::Spot),
        "usdm" | "usdm-perp" | "usdm_perp" | "perp" => Ok(BinancePublicMarket::UsdmPerpetual),
        _ => Err(cli_arg_error(
            "--market must be `spot` or `usdm-perp` for Binance WSS bookTicker",
        )),
    }
}

pub(crate) fn parse_bybit_public_wss_market(value: &str) -> RuntimeResult<BybitPublicMarket> {
    match value.trim().to_ascii_lowercase().as_str() {
        "spot" => Ok(BybitPublicMarket::Spot),
        "linear" | "linear-perp" | "linear_perp" | "perp" => Ok(BybitPublicMarket::LinearPerpetual),
        _ => Err(cli_arg_error(
            "--market must be `spot` or `linear-perp` for Bybit WSS bookTicker",
        )),
    }
}

pub(crate) fn parse_okx_public_wss_market(value: &str) -> RuntimeResult<OkxPublicWssMarket> {
    match value.trim().to_ascii_lowercase().as_str() {
        "spot" => Ok(OkxPublicWssMarket::Spot),
        "swap" | "perp" | "linear" => Ok(OkxPublicWssMarket::Swap),
        _ => Err(cli_arg_error(
            "--market must be `spot` or `swap` for OKX WSS bookTicker",
        )),
    }
}

pub(crate) fn parse_bitget_public_wss_market(value: &str) -> RuntimeResult<BitgetPublicWssMarket> {
    match value.trim().to_ascii_lowercase().as_str() {
        "spot" => Ok(BitgetPublicWssMarket::Spot),
        "usdt-futures" | "usdt_futures" | "linear" | "perp" => {
            Ok(BitgetPublicWssMarket::UsdtFutures)
        }
        _ => Err(cli_arg_error(
            "--market must be `spot` or `usdt-futures` for Bitget WSS bookTicker",
        )),
    }
}

pub(crate) fn parse_aster_public_wss_market(value: &str) -> RuntimeResult<AsterPublicWssMarket> {
    match value.trim().to_ascii_lowercase().as_str() {
        "usdt-futures" | "usdt_futures" | "perp" | "futures" => {
            Ok(AsterPublicWssMarket::UsdtFutures)
        }
        _ => Err(cli_arg_error(
            "--market must be `usdt-futures` for Aster WSS bookTicker",
        )),
    }
}

pub(crate) fn parse_hyperliquid_public_wss_market(
    value: &str,
) -> RuntimeResult<HyperliquidPublicWssMarket> {
    match value.trim().to_ascii_lowercase().as_str() {
        "perp" | "perps" => Ok(HyperliquidPublicWssMarket::Perp),
        _ => Err(cli_arg_error(
            "--market must be `perp` for Hyperliquid WSS bookTicker",
        )),
    }
}

pub(crate) fn parse_lighter_public_wss_market(
    value: &str,
) -> RuntimeResult<LighterPublicWssMarket> {
    match value.trim().to_ascii_lowercase().as_str() {
        "perp" | "perps" => Ok(LighterPublicWssMarket::Perp),
        _ => Err(cli_arg_error(
            "--market must be `perp` for Lighter WSS bookTicker",
        )),
    }
}

pub(crate) fn parse_gate_public_wss_market(value: &str) -> RuntimeResult<GatePublicWssMarket> {
    match value.trim().to_ascii_lowercase().as_str() {
        "usdt-futures" | "usdt_futures" | "perp" | "futures" => {
            Ok(GatePublicWssMarket::UsdtFutures)
        }
        _ => Err(cli_arg_error(
            "--market must be `usdt-futures` for Gate WSS bookTicker",
        )),
    }
}

pub(crate) fn binance_public_wss_venue_id(market: BinancePublicMarket) -> RuntimeResult<VenueId> {
    let value = match market {
        BinancePublicMarket::Spot => "venue:BINANCE-SPOT",
        BinancePublicMarket::UsdmPerpetual => "venue:BINANCE-USDM",
    };
    Ok(VenueId::new(value)?)
}

pub(crate) fn bybit_public_wss_venue_id(market: BybitPublicMarket) -> RuntimeResult<VenueId> {
    let value = match market {
        BybitPublicMarket::Spot => BYBIT_BASIS_SPOT_VENUE_ID,
        BybitPublicMarket::LinearPerpetual => BYBIT_BASIS_PERP_VENUE_ID,
    };
    Ok(VenueId::new(value)?)
}

pub(crate) fn binance_public_wss_instrument(
    symbol: &str,
    market: BinancePublicMarket,
) -> RuntimeResult<BinancePublicInstrument> {
    let symbol = validate_binance_public_wss_symbol(symbol)?;
    let base = symbol
        .strip_suffix("USDT")
        .filter(|base| !base.is_empty())
        .ok_or_else(|| cli_arg_error("Binance WSS symbol must have a non-empty USDT base asset"))?
        .to_owned();
    let asset_usdt = AssetId::new("asset:USDT")?;
    let symbol_component = basis_identifier_component(&symbol);
    let base_component = basis_identifier_component(&base);
    let instrument_id = match market {
        BinancePublicMarket::Spot => format!("inst:BINANCE:{symbol_component}:SPOT"),
        BinancePublicMarket::UsdmPerpetual => {
            format!("inst:BINANCE:{symbol_component}:USDM-PERP")
        }
    };
    BinancePublicInstrument::new(
        symbol,
        InstrumentId::new(instrument_id)?,
        AssetId::new(format!("asset:{base_component}"))?,
        asset_usdt.clone(),
        asset_usdt,
    )
    .map_err(RuntimeError::from)
}

pub(crate) fn bybit_public_wss_instrument(
    symbol: &str,
    market: BybitPublicMarket,
) -> RuntimeResult<BybitPublicInstrument> {
    let symbol = validate_bybit_public_wss_symbol(symbol)?;
    let base = symbol
        .strip_suffix("USDT")
        .filter(|base| !base.is_empty())
        .ok_or_else(|| cli_arg_error("Bybit WSS symbol must have a non-empty USDT base asset"))?
        .to_owned();
    let asset_usdt = AssetId::new("asset:USDT")?;
    let instrument_id = match market {
        BybitPublicMarket::Spot => format!("inst:BYBIT:{symbol}:SPOT"),
        BybitPublicMarket::LinearPerpetual => format!("inst:BYBIT:{symbol}:LINEAR-PERP"),
    };
    BybitPublicInstrument::new(
        symbol,
        InstrumentId::new(instrument_id)?,
        AssetId::new(format!("asset:{base}"))?,
        asset_usdt.clone(),
        asset_usdt,
    )
    .map_err(RuntimeError::from)
}

pub(crate) fn okx_public_wss_instrument_id(
    symbol: &str,
    market: OkxPublicWssMarket,
) -> RuntimeResult<InstrumentId> {
    let symbol = normalize_okx_wss_symbol_scope(symbol)?;
    let value = match market {
        OkxPublicWssMarket::Spot => format!("inst:OKX:{symbol}:SPOT"),
        OkxPublicWssMarket::Swap => format!("inst:OKX:{symbol}-SWAP:SWAP"),
    };
    Ok(InstrumentId::new(value)?)
}

pub(crate) fn bitget_public_wss_instrument_id(
    symbol: &str,
    market: BitgetPublicWssMarket,
) -> RuntimeResult<InstrumentId> {
    let symbol = normalize_bitget_wss_symbol_scope(symbol)?;
    let value = match market {
        BitgetPublicWssMarket::Spot => format!("inst:BITGET:{symbol}:SPOT"),
        BitgetPublicWssMarket::UsdtFutures => format!("inst:BITGET:{symbol}:USDT-FUTURES"),
    };
    Ok(InstrumentId::new(value)?)
}

pub(crate) fn aster_public_wss_instrument_id(
    symbol: &str,
    market: AsterPublicWssMarket,
) -> RuntimeResult<InstrumentId> {
    let symbol = validate_aster_public_wss_symbol(symbol)?;
    let value = match market {
        AsterPublicWssMarket::UsdtFutures => format!("inst:ASTER:{symbol}:USDT-FUTURES"),
    };
    Ok(InstrumentId::new(value)?)
}

pub(crate) fn hyperliquid_public_wss_instrument_id(
    coin: &str,
    market: HyperliquidPublicWssMarket,
) -> RuntimeResult<InstrumentId> {
    let coin = validate_hyperliquid_public_wss_coin(coin)?;
    let symbol = funding_display_symbol(&coin);
    let value = match market {
        HyperliquidPublicWssMarket::Perp => format!("inst:HYPERLIQUID:{symbol}:PERP"),
    };
    Ok(InstrumentId::new(value)?)
}

pub(crate) fn lighter_public_wss_instrument_id(
    symbol: &str,
    market: LighterPublicWssMarket,
) -> RuntimeResult<InstrumentId> {
    let symbol = validate_lighter_public_wss_symbol(symbol)?;
    let value = match market {
        LighterPublicWssMarket::Perp => format!("inst:LIGHTER:{symbol}:PERP"),
    };
    Ok(InstrumentId::new(value)?)
}

pub(crate) fn gate_public_wss_instrument_id(
    symbol: &str,
    market: GatePublicWssMarket,
) -> RuntimeResult<InstrumentId> {
    let symbol = validate_gate_public_wss_symbol(symbol)?;
    let value = match market {
        GatePublicWssMarket::UsdtFutures => format!("inst:GATE:{symbol}:USDT-FUTURES"),
    };
    Ok(InstrumentId::new(value)?)
}

pub(crate) fn bybit_public_market_event_scope(market: BybitPublicMarket) -> &'static str {
    match market {
        BybitPublicMarket::Spot => "spot",
        BybitPublicMarket::LinearPerpetual => "linear-perp",
    }
}

#[cfg(feature = "live-exec")]
pub(crate) fn bybit_public_market_category(market: BybitPublicMarket) -> &'static str {
    match market {
        BybitPublicMarket::Spot => "spot",
        BybitPublicMarket::LinearPerpetual => "linear",
    }
}

pub(crate) fn okx_public_wss_market_event_scope(market: OkxPublicWssMarket) -> &'static str {
    match market {
        OkxPublicWssMarket::Spot => "spot",
        OkxPublicWssMarket::Swap => "swap",
    }
}

pub(crate) fn bitget_public_wss_market_event_scope(market: BitgetPublicWssMarket) -> &'static str {
    match market {
        BitgetPublicWssMarket::Spot => "spot",
        BitgetPublicWssMarket::UsdtFutures => "usdt-futures",
    }
}

pub(crate) fn aster_public_wss_market_event_scope(market: AsterPublicWssMarket) -> &'static str {
    match market {
        AsterPublicWssMarket::UsdtFutures => "usdt-futures",
    }
}

pub(crate) fn hyperliquid_public_wss_market_event_scope(
    market: HyperliquidPublicWssMarket,
) -> &'static str {
    match market {
        HyperliquidPublicWssMarket::Perp => "perp",
    }
}

pub(crate) fn lighter_public_wss_market_event_scope(
    market: LighterPublicWssMarket,
) -> &'static str {
    match market {
        LighterPublicWssMarket::Perp => "perp",
    }
}

pub(crate) fn gate_public_wss_market_event_scope(market: GatePublicWssMarket) -> &'static str {
    match market {
        GatePublicWssMarket::UsdtFutures => "usdt-futures",
    }
}

pub(crate) fn okx_public_wss_inst_id(symbol: &str, market: OkxPublicWssMarket) -> String {
    match market {
        OkxPublicWssMarket::Spot => symbol.to_owned(),
        OkxPublicWssMarket::Swap => format!("{symbol}-SWAP"),
    }
}

pub(crate) fn okx_public_wss_symbol_from_inst_id(
    inst_id: &str,
    market: OkxPublicWssMarket,
) -> RuntimeResult<String> {
    match market {
        OkxPublicWssMarket::Spot => normalize_okx_wss_symbol_scope(inst_id),
        OkxPublicWssMarket::Swap => inst_id
            .strip_suffix("-SWAP")
            .ok_or_else(|| cli_arg_error("OKX swap WSS instId must end with -SWAP"))
            .and_then(normalize_okx_wss_symbol_scope),
    }
}

pub(crate) fn okx_wss_ticker_subscribe_payloads(
    symbols: &[String],
    market: OkxPublicWssMarket,
) -> Vec<String> {
    let mut payloads = Vec::new();
    let mut current_args = Vec::new();

    for symbol in symbols {
        let arg = okx_wss_ticker_subscribe_arg(symbol, market);
        let would_exceed_count = current_args.len() >= OKX_WSS_SUBSCRIBE_MAX_ARGS_PER_PAYLOAD;
        let would_exceed_bytes = !current_args.is_empty()
            && okx_wss_ticker_subscribe_payload_len(&current_args, Some(&arg))
                > OKX_WSS_SUBSCRIBE_PAYLOAD_MAX_BYTES;

        if would_exceed_count || would_exceed_bytes {
            payloads.push(okx_wss_ticker_subscribe_payload_from_args(&current_args));
            current_args.clear();
        }
        current_args.push(arg);
    }

    if !current_args.is_empty() {
        payloads.push(okx_wss_ticker_subscribe_payload_from_args(&current_args));
    }

    payloads
}

pub(crate) fn okx_wss_ticker_subscribe_arg(symbol: &str, market: OkxPublicWssMarket) -> String {
    format!(
        "{{\"channel\":\"tickers\",\"instId\":{}}}",
        json_string(&okx_public_wss_inst_id(symbol, market))
    )
}

pub(crate) fn okx_wss_ticker_subscribe_payload_from_args(args: &[String]) -> String {
    format!("{{\"op\":\"subscribe\",\"args\":[{}]}}", args.join(","))
}

pub(crate) fn okx_wss_ticker_subscribe_payload_len(
    args: &[String],
    extra_arg: Option<&String>,
) -> usize {
    let mut len = "{\"op\":\"subscribe\",\"args\":[]}".len();
    len += args.iter().map(String::len).sum::<usize>();
    if !args.is_empty() {
        len += args.len() - 1;
    }
    if let Some(arg) = extra_arg {
        len += arg.len();
        if !args.is_empty() {
            len += 1;
        }
    }
    len
}

pub(crate) fn bitget_wss_ticker_subscribe_payloads(
    symbols: &[String],
    market: BitgetPublicWssMarket,
) -> Vec<String> {
    let mut payloads = Vec::new();
    let mut current_args = Vec::new();

    for symbol in symbols {
        let arg = bitget_wss_ticker_subscribe_arg(symbol, market);
        let would_exceed_count = current_args.len() >= BITGET_WSS_SUBSCRIBE_MAX_ARGS_PER_PAYLOAD;
        let would_exceed_bytes = !current_args.is_empty()
            && bitget_wss_ticker_subscribe_payload_len(&current_args, Some(&arg))
                > BITGET_WSS_SUBSCRIBE_PAYLOAD_MAX_BYTES;

        if would_exceed_count || would_exceed_bytes {
            payloads.push(bitget_wss_ticker_subscribe_payload_from_args(&current_args));
            current_args.clear();
        }
        current_args.push(arg);
    }

    if !current_args.is_empty() {
        payloads.push(bitget_wss_ticker_subscribe_payload_from_args(&current_args));
    }

    payloads
}

pub(crate) fn bitget_wss_ticker_subscribe_payloads_for_scope(
    symbols: &[String],
    market: BitgetPublicWssMarket,
    all_symbols_scope: bool,
) -> Vec<String> {
    if all_symbols_scope {
        if symbols.is_empty() {
            return vec![bitget_wss_ticker_subscribe_payload_from_args(&[
                bitget_wss_ticker_subscribe_arg(BITGET_WSS_TICKER_DEFAULT_INST_ID, market),
            ])];
        }
        return bitget_wss_ticker_subscribe_payloads(symbols, market);
    }
    bitget_wss_ticker_subscribe_payloads(symbols, market)
}

pub(crate) fn bitget_wss_ticker_subscribe_arg(
    symbol: &str,
    market: BitgetPublicWssMarket,
) -> String {
    format!(
        "{{\"instType\":{},\"channel\":\"ticker\",\"instId\":{}}}",
        json_string(market.inst_type()),
        json_string(symbol)
    )
}

pub(crate) fn bitget_wss_ticker_subscribe_payload_from_args(args: &[String]) -> String {
    format!("{{\"op\":\"subscribe\",\"args\":[{}]}}", args.join(","))
}

pub(crate) fn bitget_wss_ticker_subscribe_payload_len(
    args: &[String],
    extra_arg: Option<&String>,
) -> usize {
    let mut len = "{\"op\":\"subscribe\",\"args\":[]}".len();
    len += args.iter().map(String::len).sum::<usize>();
    if !args.is_empty() {
        len += args.len() - 1;
    }
    if let Some(arg) = extra_arg {
        len += arg.len();
        if !args.is_empty() {
            len += 1;
        }
    }
    len
}

pub(crate) fn lighter_wss_ticker_subscribe_payload(market_id: &str) -> RuntimeResult<String> {
    let market_id = validate_lighter_wss_market_id(market_id)?;
    Ok(format!(
        "{{\"type\":\"subscribe\",\"channel\":\"ticker/{market_id}\"}}"
    ))
}

pub(crate) fn gate_wss_book_ticker_subscribe_payload(symbol: &str) -> RuntimeResult<String> {
    let symbol = validate_gate_public_wss_symbol(symbol)?;
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Ok(format!(
        "{{\"time\":{seconds},\"channel\":\"futures.book_ticker\",\"event\":\"subscribe\",\"payload\":[{}]}}",
        json_string(&symbol)
    ))
}

pub(crate) fn validate_lighter_wss_market_id(value: &str) -> RuntimeResult<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 16 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(cli_arg_error(
            "Lighter WSS market_id must be a non-empty decimal integer",
        ));
    }
    Ok(value.to_owned())
}

pub(crate) fn bybit_wss_orderbook_topic(symbol: &str) -> String {
    format!("orderbook.1.{symbol}")
}

pub(crate) fn bybit_wss_ticker_topic(symbol: &str) -> String {
    format!("tickers.{symbol}")
}

pub(crate) fn bybit_wss_top_of_book_topic(symbol: &str, use_ticker_topic: bool) -> String {
    if use_ticker_topic {
        bybit_wss_ticker_topic(symbol)
    } else {
        bybit_wss_orderbook_topic(symbol)
    }
}

pub(crate) fn bybit_wss_should_use_ticker_topics(
    market: BybitPublicMarket,
    symbol_count: usize,
    all_symbols_scope: bool,
) -> bool {
    market == BybitPublicMarket::LinearPerpetual
        && (all_symbols_scope || symbol_count > BYBIT_LINEAR_WSS_ORDERBOOK_TOPIC_SCOPE_LIMIT)
}

pub(crate) fn bybit_wss_subscribe_topic_limit(
    market: BybitPublicMarket,
    symbol_count: usize,
    all_symbols_scope: bool,
) -> Option<usize> {
    if market == BybitPublicMarket::LinearPerpetual
        && (all_symbols_scope || symbol_count > BYBIT_LINEAR_WSS_ORDERBOOK_TOPIC_SCOPE_LIMIT)
    {
        Some(BYBIT_LINEAR_WSS_TICKER_TOPIC_SUBSCRIBE_LIMIT)
    } else {
        None
    }
}

pub(crate) fn bybit_wss_subscribe_topics_for_rows(
    rows: &[MonitorBookTickerRow],
    market: BybitPublicMarket,
    all_symbols_scope: bool,
) -> Vec<String> {
    let use_ticker_topics =
        bybit_wss_should_use_ticker_topics(market, rows.len(), all_symbols_scope);
    bybit_wss_tracked_rest_rows_for_subscribe_scope(rows.to_vec(), market, all_symbols_scope)
        .into_iter()
        .map(|row| bybit_wss_top_of_book_topic(&row.symbol, use_ticker_topics))
        .collect()
}

pub(crate) fn bybit_wss_tracked_rest_rows_and_subscribe_topics(
    rows: Vec<MonitorBookTickerRow>,
    market: BybitPublicMarket,
    all_symbols_scope: bool,
) -> (Vec<MonitorBookTickerRow>, Vec<String>) {
    let subscribe_args = bybit_wss_subscribe_topics_for_rows(&rows, market, all_symbols_scope);
    let tracked_rows =
        bybit_wss_tracked_rest_rows_for_subscribe_scope(rows, market, all_symbols_scope);
    (tracked_rows, subscribe_args)
}

pub(crate) fn bybit_wss_tracked_rest_rows_for_subscribe_scope(
    mut rows: Vec<MonitorBookTickerRow>,
    market: BybitPublicMarket,
    all_symbols_scope: bool,
) -> Vec<MonitorBookTickerRow> {
    let topic_limit = bybit_wss_subscribe_topic_limit(market, rows.len(), all_symbols_scope);
    if market == BybitPublicMarket::LinearPerpetual && topic_limit.is_some() {
        rows.sort_by(|left, right| {
            let left_rank = bybit_linear_wss_priority_rank(&left.symbol).unwrap_or(usize::MAX);
            let right_rank = bybit_linear_wss_priority_rank(&right.symbol).unwrap_or(usize::MAX);
            left_rank
                .cmp(&right_rank)
                .then_with(|| left.symbol.cmp(&right.symbol))
        });
    }
    rows.into_iter()
        .take(topic_limit.unwrap_or(usize::MAX))
        .collect()
}

fn bybit_linear_wss_priority_rank(symbol: &str) -> Option<usize> {
    BYBIT_LINEAR_WSS_PRIORITY_SYMBOLS
        .iter()
        .position(|priority| *priority == symbol)
}

pub(crate) fn bybit_wss_book_ticker_public_stream_url(market: BybitPublicMarket) -> String {
    match market {
        BybitPublicMarket::Spot => BYBIT_SPOT_PUBLIC_WSS_BASE_URL,
        BybitPublicMarket::LinearPerpetual => BYBIT_LINEAR_PUBLIC_WSS_BASE_URL,
    }
    .to_owned()
}

impl PublicTopOfBookQuoteSnapshot {
    pub(crate) fn from_quote(quote: &MarketQuote) -> Self {
        Self::from_quote_with_symbol(quote, public_top_of_book_snapshot_symbol(quote))
    }

    pub(crate) fn from_quote_with_symbol(quote: &MarketQuote, symbol: &str) -> Self {
        Self {
            symbol: symbol.to_owned(),
            venue_id: quote.venue_id.as_str().to_owned(),
            instrument_id: quote.instrument_id.as_str().to_owned(),
            best_bid: quote.best_bid.map(|price| price.to_string()),
            best_ask: quote.best_ask.map(|price| price.to_string()),
            bid_size: quote.bid_size.map(|quantity| quantity.to_string()),
            ask_size: quote.ask_size.map(|quantity| quantity.to_string()),
            source_sequence: quote.source_sequence.clone(),
            source_event_id: quote.source_event_id.clone(),
            observed_at: quote.freshness.observed_at.to_string(),
            ingested_at: quote.freshness.ingested_at.to_string(),
            freshness_status: quote.freshness.status.as_str().to_owned(),
        }
    }

    fn to_json_with_now(&self, now: Option<UtcTimestamp>) -> String {
        let freshness_status = public_wss_quote_effective_freshness_status(self, now);
        format!(
            "{{\"ask_size\":{},\"best_ask\":{},\"best_bid\":{},\"bid_size\":{},\"freshness_status\":{},\"ingested_at\":{},\"instrument_id\":{},\"observed_at\":{},\"source_event_id\":{},\"source_sequence\":{},\"symbol\":{},\"venue_id\":{}}}",
            json_option_string(&self.ask_size),
            json_option_string(&self.best_ask),
            json_option_string(&self.best_bid),
            json_option_string(&self.bid_size),
            json_string(&freshness_status),
            json_string(&self.ingested_at),
            json_string(&self.instrument_id),
            json_string(&self.observed_at),
            json_option_string(&self.source_event_id),
            json_option_string(&self.source_sequence),
            json_string(&self.symbol),
            json_string(&self.venue_id),
        )
    }
}

pub(crate) fn public_top_of_book_snapshot_symbol(quote: &MarketQuote) -> &str {
    let symbol = quote
        .instrument_id
        .as_str()
        .split(':')
        .nth(2)
        .unwrap_or_else(|| quote.instrument_id.as_str());
    if quote.venue_id.as_str() == "venue:OKX-SWAP" {
        symbol.strip_suffix("-SWAP").unwrap_or(symbol)
    } else {
        symbol
    }
}

fn public_wss_quote_effective_freshness_status(
    quote: &PublicTopOfBookQuoteSnapshot,
    now: Option<UtcTimestamp>,
) -> String {
    if quote.freshness_status == "Fresh"
        && now.is_some_and(|now| !public_wss_quote_timestamps_are_current(quote, now))
    {
        "Stale".to_owned()
    } else {
        quote.freshness_status.clone()
    }
}

fn public_wss_quote_is_current_usable(
    quote: &PublicTopOfBookQuoteSnapshot,
    now: UtcTimestamp,
) -> bool {
    quote.freshness_status == "Fresh"
        && public_wss_source_event_id_is_book_ticker(quote.source_event_id.as_deref())
        && public_wss_quote_timestamps_are_current(quote, now)
}

fn public_wss_source_event_id_is_book_ticker(source_event_id: Option<&str>) -> bool {
    source_event_id.is_some_and(|source| source.contains(":wss-book-ticker:"))
}

fn market_quote_is_public_wss_book_ticker(quote: &MarketQuote) -> bool {
    public_wss_source_event_id_is_book_ticker(quote.source_event_id.as_deref())
}

fn public_wss_quote_timestamps_are_current(
    quote: &PublicTopOfBookQuoteSnapshot,
    now: UtcTimestamp,
) -> bool {
    public_wss_timestamp_string_is_current(&quote.observed_at, now)
        && public_wss_timestamp_string_is_current(&quote.ingested_at, now)
}

fn public_wss_timestamp_string_is_current(value: &str, now: UtcTimestamp) -> bool {
    let Ok(timestamp) = UtcTimestamp::from_str(value) else {
        return false;
    };
    let Ok(timestamp_ms) = runtime_timestamp_millis(timestamp) else {
        return false;
    };
    let Ok(now_ms) = runtime_timestamp_millis(now) else {
        return false;
    };
    if timestamp_ms > now_ms + 1_000 {
        return false;
    }
    now_ms.saturating_sub(timestamp_ms) <= i128::from(PUBLIC_WSS_MONITOR_MAX_AGE_MS)
}

fn public_wss_update_is_row_level_stale(update: &HybridMarketDataUpdate) -> bool {
    update.fail_closed
        && update.status == HybridMarketDataStatus::Streaming
        && update.transport == MarketDataTransport::WebSocketStream
        && update
            .quote
            .as_ref()
            .is_some_and(|quote| quote.freshness.is_stale())
        && update.reason_codes.iter().any(|code| code == "DATA_STALE")
        && !update.reason_codes.iter().any(|code| {
            matches!(
                code.as_str(),
                "REST_SNAPSHOT_REQUIRED"
                    | "VENUE_UNHEALTHY"
                    | "WSS_DISCONNECTED"
                    | "WSS_SEQUENCE_GAP"
                    | "WSS_SYMBOL_MISMATCH"
                    | "WSS_WITHOUT_REST_SNAPSHOT"
            )
        })
}

impl PublicTopOfBookMonitorSnapshot {
    pub(crate) fn empty(symbol: &str, market: BinancePublicMarket, stream_url: &str) -> Self {
        Self::empty_with_market(symbol, market.as_str(), stream_url)
    }

    pub(crate) fn empty_with_market(symbol: &str, market: &str, stream_url: &str) -> Self {
        Self {
            status: "starting".to_owned(),
            updated_at: "not-yet-updated".to_owned(),
            symbol: symbol.to_owned(),
            market: market.to_owned(),
            stream_url: stream_url.to_owned(),
            coordinator_status: HybridMarketDataStatus::AwaitingRestSnapshot
                .as_str()
                .to_owned(),
            latest_quote: None,
            rows: Vec::new(),
            total_rows: 0,
            fail_closed: false,
            fail_closed_count: 0,
            disconnect_count: 0,
            rest_rebuild_count: 0,
            wss_update_count: 0,
            last_error: None,
        }
    }

    pub(crate) fn begin_rest_rebuild(&mut self) {
        self.rest_rebuild_count += 1;
        self.status = "rebuilding".to_owned();
        self.updated_at = current_utc_timestamp_string();
        self.latest_quote = None;
        self.rows.clear();
        self.total_rows = 0;
        self.wss_update_count = 0;
    }

    pub(crate) fn record_update(&mut self, update: &HybridMarketDataUpdate) {
        self.record_update_with_symbol(update, None);
    }

    pub(crate) fn record_update_with_symbol(
        &mut self,
        update: &HybridMarketDataUpdate,
        symbol_override: Option<&str>,
    ) {
        self.updated_at = current_utc_timestamp_string();
        self.coordinator_status = update.status.as_str().to_owned();
        let row_level_stale = public_wss_update_is_row_level_stale(update);
        self.status =
            public_wss_monitor_status(update.status, update.fail_closed && !row_level_stale)
                .to_owned();
        self.fail_closed = update.fail_closed && !row_level_stale;
        if update.fail_closed && !row_level_stale {
            self.fail_closed_count += 1;
            self.last_error = Some(if update.reason_codes.is_empty() {
                "fail_closed".to_owned()
            } else {
                format!("fail_closed: {}", update.reason_codes.join(","))
            });
        } else {
            if row_level_stale {
                self.fail_closed_count += 1;
            }
            self.last_error = None;
        }
        if update.status == HybridMarketDataStatus::Reconnecting {
            self.disconnect_count += 1;
        }
        let has_wss_book_ticker_quote = update.transport == MarketDataTransport::WebSocketStream
            && update
                .quote
                .as_ref()
                .is_some_and(market_quote_is_public_wss_book_ticker);
        if has_wss_book_ticker_quote {
            self.wss_update_count += 1;
        }
        if let Some(quote) = &update.quote {
            let quote_snapshot = symbol_override
                .map(|symbol| PublicTopOfBookQuoteSnapshot::from_quote_with_symbol(quote, symbol))
                .unwrap_or_else(|| PublicTopOfBookQuoteSnapshot::from_quote(quote));
            self.upsert_quote_row(quote_snapshot.clone());
            if has_wss_book_ticker_quote {
                self.latest_quote = Some(quote_snapshot);
            }
        } else if update.status == HybridMarketDataStatus::Streaming {
            self.latest_quote = None;
        }
    }

    pub(crate) fn upsert_quote_row(&mut self, quote: PublicTopOfBookQuoteSnapshot) {
        match self.rows.iter_mut().find(|row| row.symbol == quote.symbol) {
            Some(row) => *row = quote,
            None => self.rows.push(quote),
        }
        self.rows
            .sort_by(|left, right| left.symbol.cmp(&right.symbol));
        self.total_rows = self.rows.len();
    }

    pub(crate) fn record_failure(&mut self, detail: impl Into<String>, count_disconnect: bool) {
        self.status = "fail_closed".to_owned();
        self.updated_at = current_utc_timestamp_string();
        self.fail_closed = true;
        self.fail_closed_count += 1;
        if count_disconnect {
            self.disconnect_count += 1;
        }
        self.last_error = Some(detail.into());
    }

    pub(crate) fn record_reconnect_required(
        &mut self,
        detail: impl Into<String>,
        count_disconnect: bool,
    ) {
        self.status = "reconnecting".to_owned();
        self.updated_at = current_utc_timestamp_string();
        self.fail_closed = false;
        if count_disconnect {
            self.disconnect_count += 1;
        }
        self.last_error = Some(detail.into());
    }

    pub(crate) fn record_stream_end_with_label(&mut self, label: &str) {
        if !self.fail_closed {
            self.record_failure(
                format!("{label} ended before reconnect; rebuilding from REST"),
                true,
            );
            return;
        }
        if self.last_error.is_none() {
            self.last_error = Some(format!("{label} ended; rebuilding from REST"));
        }
    }

    pub(crate) fn record_stream_end_with_label_and_observed(
        &mut self,
        label: &str,
        observed_wss_event: bool,
    ) {
        if observed_wss_event {
            self.record_reconnect_required(
                format!("{label} ended before reconnect; rebuilding from REST"),
                true,
            );
        } else {
            self.record_stream_end_with_label(label);
        }
    }

    pub(crate) fn record_wss_read_error(
        &mut self,
        detail: impl Into<String>,
        observed_wss_event: bool,
    ) {
        let detail = detail.into();
        if observed_wss_event {
            self.record_reconnect_required(detail, true);
        } else {
            self.record_failure(detail, true);
        }
    }

    fn availability_status(&self, now: Option<UtcTimestamp>) -> String {
        if self.fail_closed {
            return "fail_closed".to_owned();
        }
        if let Some(current_time) = now {
            if self.has_current_usable_wss_quote(current_time) {
                if self.degraded_reason(current_time).is_some() {
                    return "degraded".to_owned();
                }
                return "streaming".to_owned();
            }
        }
        if self.status == "reconnecting"
            && self.rows.is_empty()
            && self.latest_quote.is_none()
            && (self.total_rows > 0 || self.wss_update_count > 0)
        {
            return "streaming".to_owned();
        }
        if self.total_rows > 0 || self.latest_quote.is_some() || self.wss_update_count > 0 {
            if now.is_some() && self.status == "streaming" {
                return "stale".to_owned();
            }
            if now.is_some() && self.status == "reconnecting" {
                return "reconnecting".to_owned();
            }
            return "streaming".to_owned();
        }
        self.status.clone()
    }

    fn freshness_counts(&self, now: Option<UtcTimestamp>) -> (usize, usize, usize) {
        let Some(now) = now else {
            return (0, 0, 0);
        };
        let mut current_usable = 0_usize;
        let mut stale = 0_usize;
        let mut unusable = 0_usize;
        for quote in &self.rows {
            if public_wss_quote_is_current_usable(quote, now) {
                current_usable += 1;
            } else if public_wss_source_event_id_is_book_ticker(quote.source_event_id.as_deref()) {
                stale += 1;
            } else {
                unusable += 1;
            }
        }
        (current_usable, stale, unusable)
    }

    fn degraded_reason(&self, now: UtcTimestamp) -> Option<String> {
        if self.fail_closed || self.total_rows == 0 {
            return None;
        }
        if self.status != "streaming" && self.status != "reconnecting" {
            return None;
        }
        let (current_usable_count, stale_row_count, unusable_row_count) =
            self.freshness_counts(Some(now));
        if current_usable_count == 0 {
            return None;
        }
        let stale_ratio_bps = public_wss_row_ratio_bps(stale_row_count, self.total_rows);
        if stale_ratio_bps >= PUBLIC_WSS_DEGRADED_STALE_ROW_RATIO_BPS {
            return Some(format!(
                "stale_wss_row_ratio_bps={stale_ratio_bps}; current_usable_row_count={current_usable_count}; stale_row_count={stale_row_count}; unusable_row_count={unusable_row_count}; total_rows={}",
                self.total_rows
            ));
        }
        let high_reconnect_pressure = self.disconnect_count >= PUBLIC_WSS_DEGRADED_RECONNECT_COUNT
            || self.rest_rebuild_count >= PUBLIC_WSS_DEGRADED_RECONNECT_COUNT
            || self.fail_closed_count >= PUBLIC_WSS_DEGRADED_RECONNECT_COUNT;
        if self.status == "reconnecting" && high_reconnect_pressure {
            return Some(format!(
                "high_reconnect_pressure; disconnect_count={}; rest_rebuild_count={}; fail_closed_count={}",
                self.disconnect_count, self.rest_rebuild_count, self.fail_closed_count
            ));
        }
        None
    }

    fn has_current_usable_wss_quote(&self, now: UtcTimestamp) -> bool {
        self.latest_quote
            .as_ref()
            .is_some_and(|quote| public_wss_quote_is_current_usable(quote, now))
            || self
                .rows
                .iter()
                .any(|quote| public_wss_quote_is_current_usable(quote, now))
    }

    fn effective_last_error(
        &self,
        availability_status: &str,
        now: Option<UtcTimestamp>,
    ) -> Option<String> {
        if availability_status == "stale" {
            Some(self.last_error.clone().unwrap_or_else(|| {
                format!(
                    "no currently usable WSS quote rows; latest update at {}",
                    self.updated_at
                )
            }))
        } else if availability_status == "degraded" {
            now.and_then(|now| self.degraded_reason(now))
                .or_else(|| self.last_error.clone())
        } else {
            self.last_error.clone()
        }
    }

    pub(crate) fn health_http_status(&self) -> u16 {
        let status = self.availability_status(current_utc_timestamp().ok());
        if self.fail_closed || status == "stale" || status == "reconnecting" || status == "degraded"
        {
            503
        } else {
            200
        }
    }

    pub(crate) fn health_json(&self) -> String {
        let now = current_utc_timestamp().ok();
        let status = self.availability_status(now);
        let (current_usable_row_count, stale_row_count, unusable_row_count) =
            self.freshness_counts(now);
        let degraded_reason = now.and_then(|now| self.degraded_reason(now));
        format!(
            "{{\"current_usable_row_count\":{},\"degraded_reason\":{},\"disconnect_count\":{},\"fail_closed\":{},\"latest_quote\":{},\"rest_rebuild_count\":{},\"stale_row_count\":{},\"status\":{},\"stream_status\":{},\"total_rows\":{},\"unusable_row_count\":{},\"updated_at\":{},\"wss_update_count\":{}}}",
            current_usable_row_count,
            json_option_string(&degraded_reason),
            self.disconnect_count,
            self.fail_closed,
            self.latest_quote_json_value_with_now(now),
            self.rest_rebuild_count,
            stale_row_count,
            json_string(&status),
            json_string(&self.status),
            self.total_rows,
            unusable_row_count,
            json_string(&self.updated_at),
            self.wss_update_count,
        )
    }

    pub(crate) fn quote_json(&self) -> String {
        let now = current_utc_timestamp().ok();
        let status = self.availability_status(now);
        format!(
            "{{\"fail_closed\":{},\"latest_quote\":{},\"status\":{},\"stream_status\":{},\"updated_at\":{}}}",
            self.fail_closed,
            self.latest_quote_json_value_with_now(now),
            json_string(&status),
            json_string(&self.status),
            json_string(&self.updated_at),
        )
    }

    pub(crate) fn quotes_json(&self) -> String {
        let now = current_utc_timestamp().ok();
        let status = self.availability_status(now);
        let (current_usable_row_count, stale_row_count, unusable_row_count) =
            self.freshness_counts(now);
        format!(
            "{{\"current_usable_row_count\":{},\"fail_closed\":{},\"rows\":[{}],\"stale_row_count\":{},\"status\":{},\"stream_status\":{},\"total_rows\":{},\"unusable_row_count\":{},\"updated_at\":{}}}",
            current_usable_row_count,
            self.fail_closed,
            self.rows
                .iter()
                .map(|quote| quote.to_json_with_now(now))
                .collect::<Vec<_>>()
                .join(","),
            stale_row_count,
            json_string(&status),
            json_string(&self.status),
            self.total_rows,
            unusable_row_count,
            json_string(&self.updated_at),
        )
    }

    pub(crate) fn to_json(&self) -> String {
        let now = current_utc_timestamp().ok();
        let status = self.availability_status(now);
        let last_error = self.effective_last_error(&status, now);
        let (current_usable_row_count, stale_row_count, unusable_row_count) =
            self.freshness_counts(now);
        let degraded_reason = now.and_then(|now| self.degraded_reason(now));
        format!(
            "{{\"coordinator_status\":{},\"current_usable_row_count\":{},\"degraded_reason\":{},\"disconnect_count\":{},\"fail_closed\":{},\"fail_closed_count\":{},\"last_error\":{},\"latest_quote\":{},\"market\":{},\"rest_rebuild_count\":{},\"rows\":[{}],\"stale_row_count\":{},\"status\":{},\"stream_status\":{},\"stream_url\":{},\"symbol\":{},\"total_rows\":{},\"unusable_row_count\":{},\"updated_at\":{},\"wss_update_count\":{}}}",
            json_string(&self.coordinator_status),
            current_usable_row_count,
            json_option_string(&degraded_reason),
            self.disconnect_count,
            self.fail_closed,
            self.fail_closed_count,
            json_option_string(&last_error),
            self.latest_quote_json_value_with_now(now),
            json_string(&self.market),
            self.rest_rebuild_count,
            self.rows
                .iter()
                .map(|quote| quote.to_json_with_now(now))
                .collect::<Vec<_>>()
                .join(","),
            stale_row_count,
            json_string(&status),
            json_string(&self.status),
            json_string(&self.stream_url),
            json_string(&self.symbol),
            self.total_rows,
            unusable_row_count,
            json_string(&self.updated_at),
            self.wss_update_count,
        )
    }

    pub(crate) fn latest_quote_json_value_with_now(&self, now: Option<UtcTimestamp>) -> String {
        self.latest_quote
            .as_ref()
            .map(|quote| quote.to_json_with_now(now))
            .unwrap_or_else(|| "null".to_owned())
    }
}

pub(crate) fn public_wss_monitor_status(
    status: HybridMarketDataStatus,
    fail_closed: bool,
) -> &'static str {
    let status = match status {
        HybridMarketDataStatus::AwaitingRestSnapshot => "starting",
        HybridMarketDataStatus::SnapshotReady => "snapshot_ready",
        HybridMarketDataStatus::Streaming => "streaming",
        HybridMarketDataStatus::Reconnecting => "reconnecting",
        HybridMarketDataStatus::Halted => "fail_closed",
    };
    if fail_closed && status != "reconnecting" {
        "fail_closed"
    } else {
        status
    }
}

fn public_wss_row_ratio_bps(row_count: usize, total_rows: usize) -> u64 {
    if total_rows == 0 {
        return 0;
    }
    (row_count as u64).saturating_mul(10_000) / (total_rows as u64)
}

pub(crate) fn start_binance_wss_book_ticker_http_api(
    bind_addr: &str,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Binance WSS bookTicker HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_binance_wss_book_ticker_http_connection(stream, &state),
                Err(error) => eprintln!("binance-wss-book-ticker api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn handle_binance_wss_book_ticker_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) {
    let mut buffer = [0_u8; 8192];
    let read = match stream.read(&mut buffer) {
        Ok(read) => read,
        Err(_) => return,
    };
    let request = String::from_utf8_lossy(&buffer[..read]);
    let first_line = request.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let route = path.split('?').next().unwrap_or(path);
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }

    let snapshot = state
        .read()
        .expect("Public WSS monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (snapshot.health_http_status(), snapshot.health_json())
    } else if route == "/api/binance-wss-book-ticker/status" {
        (200, snapshot.to_json())
    } else if route == "/api/binance-wss-book-ticker/quote" {
        (200, snapshot.quote_json())
    } else if route == "/api/binance-wss-book-ticker/quotes" {
        (200, snapshot.quotes_json())
    } else if route == "/" || route == "/dashboard" {
        let _ = write_http_json(&mut stream, 410, &static_dashboard_gone_json("Binance WSS"));
        return;
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/binance-wss-book-ticker/status\",\"/api/binance-wss-book-ticker/quote\",\"/api/binance-wss-book-ticker/quotes\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

pub(crate) fn start_bybit_wss_book_ticker_http_api(
    bind_addr: &str,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Bybit WSS bookTicker HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_bybit_wss_book_ticker_http_connection(stream, &state),
                Err(error) => eprintln!("bybit-wss-book-ticker api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn handle_bybit_wss_book_ticker_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) {
    let mut buffer = [0_u8; 8192];
    let read = match stream.read(&mut buffer) {
        Ok(read) => read,
        Err(_) => return,
    };
    let request = String::from_utf8_lossy(&buffer[..read]);
    let first_line = request.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let route = path.split('?').next().unwrap_or(path);
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }

    let snapshot = state.read().expect("Bybit WSS monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (snapshot.health_http_status(), snapshot.health_json())
    } else if route == "/api/bybit-wss-book-ticker/status" {
        (200, snapshot.to_json())
    } else if route == "/api/bybit-wss-book-ticker/quote" {
        (200, snapshot.quote_json())
    } else if route == "/api/bybit-wss-book-ticker/quotes" {
        (200, snapshot.quotes_json())
    } else if route == "/" || route == "/dashboard" {
        let _ = write_http_json(&mut stream, 410, &static_dashboard_gone_json("Bybit WSS"));
        return;
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/bybit-wss-book-ticker/status\",\"/api/bybit-wss-book-ticker/quote\",\"/api/bybit-wss-book-ticker/quotes\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

pub(crate) fn start_okx_wss_book_ticker_http_api(
    bind_addr: &str,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind OKX WSS bookTicker HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_okx_wss_book_ticker_http_connection(stream, &state),
                Err(error) => eprintln!("okx-wss-book-ticker api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn handle_okx_wss_book_ticker_http_connection(
    stream: TcpStream,
    state: &Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) {
    handle_public_wss_book_ticker_http_connection(stream, state, "okx-wss-book-ticker");
}

pub(crate) fn start_bitget_wss_book_ticker_http_api(
    bind_addr: &str,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Bitget WSS bookTicker HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_bitget_wss_book_ticker_http_connection(stream, &state),
                Err(error) => eprintln!("bitget-wss-book-ticker api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn handle_bitget_wss_book_ticker_http_connection(
    stream: TcpStream,
    state: &Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) {
    handle_public_wss_book_ticker_http_connection(stream, state, "bitget-wss-book-ticker");
}

pub(crate) fn start_aster_wss_book_ticker_http_api(
    bind_addr: &str,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Aster WSS bookTicker HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_aster_wss_book_ticker_http_connection(stream, &state),
                Err(error) => eprintln!("aster-wss-book-ticker api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn handle_aster_wss_book_ticker_http_connection(
    stream: TcpStream,
    state: &Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) {
    handle_public_wss_book_ticker_http_connection(stream, state, "aster-wss-book-ticker");
}

pub(crate) fn start_hyperliquid_wss_book_ticker_http_api(
    bind_addr: &str,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Hyperliquid WSS bookTicker HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_hyperliquid_wss_book_ticker_http_connection(stream, &state),
                Err(error) => eprintln!("hyperliquid-wss-book-ticker api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn handle_hyperliquid_wss_book_ticker_http_connection(
    stream: TcpStream,
    state: &Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) {
    handle_public_wss_book_ticker_http_connection(stream, state, "hyperliquid-wss-book-ticker");
}

pub(crate) fn start_lighter_wss_book_ticker_http_api(
    bind_addr: &str,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Lighter WSS bookTicker HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_lighter_wss_book_ticker_http_connection(stream, &state),
                Err(error) => eprintln!("lighter-wss-book-ticker api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn handle_lighter_wss_book_ticker_http_connection(
    stream: TcpStream,
    state: &Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) {
    handle_public_wss_book_ticker_http_connection(stream, state, "lighter-wss-book-ticker");
}

pub(crate) fn start_gate_wss_book_ticker_http_api(
    bind_addr: &str,
    state: Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Gate WSS bookTicker HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_gate_wss_book_ticker_http_connection(stream, &state),
                Err(error) => eprintln!("gate-wss-book-ticker api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

pub(crate) fn handle_gate_wss_book_ticker_http_connection(
    stream: TcpStream,
    state: &Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
) {
    handle_public_wss_book_ticker_http_connection(stream, state, "gate-wss-book-ticker");
}

pub(crate) fn handle_public_wss_book_ticker_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<PublicTopOfBookMonitorSnapshot>>,
    api_name: &'static str,
) {
    let mut buffer = [0_u8; 8192];
    let read = match stream.read(&mut buffer) {
        Ok(read) => read,
        Err(_) => return,
    };
    let request = String::from_utf8_lossy(&buffer[..read]);
    let first_line = request.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let route = path.split('?').next().unwrap_or(path);
    if method != "GET" {
        let _ = write_http_json(&mut stream, 405, "{\"error\":\"method_not_allowed\"}");
        return;
    }

    let snapshot = state
        .read()
        .expect("Public WSS monitor state lock poisoned");
    let status_path = format!("/api/{api_name}/status");
    let quote_path = format!("/api/{api_name}/quote");
    let quotes_path = format!("/api/{api_name}/quotes");
    let (status, body) = if route == "/health" {
        (snapshot.health_http_status(), snapshot.health_json())
    } else if route == status_path {
        (200, snapshot.to_json())
    } else if route == quote_path {
        (200, snapshot.quote_json())
    } else if route == quotes_path {
        (200, snapshot.quotes_json())
    } else if route == "/" || route == "/dashboard" {
        let _ = write_http_json(&mut stream, 410, &static_dashboard_gone_json(api_name));
        return;
    } else {
        (
            404,
            format!(
                "{{\"error\":\"not_found\",\"paths\":[\"/health\",\"/api/{api_name}/status\",\"/api/{api_name}/quote\",\"/api/{api_name}/quotes\"]}}"
            ),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_wss_reconnect_backoff_applies_exponential_cap_and_jitter() {
        assert_eq!(
            public_wss_reconnect_backoff_with_jitter_seed(2, 1, 0),
            Duration::from_secs(2)
        );
        assert_eq!(
            public_wss_reconnect_backoff_with_jitter_seed(2, 1, 1),
            Duration::from_secs(3)
        );
        assert_eq!(
            public_wss_reconnect_backoff_with_jitter_seed(2, 4, 0),
            Duration::from_secs(16)
        );
        assert_eq!(
            public_wss_reconnect_backoff_with_jitter_seed(2, 64, 1),
            Duration::from_secs(PUBLIC_WSS_RECONNECT_BACKOFF_MAX_SECS)
        );
    }

    #[test]
    fn public_wss_read_error_after_observed_event_resets_cycle() {
        assert!(public_wss_monitor_cycle_result_after_read_error(
            RuntimeError::LiveMarketData {
                message: "connection reset".to_owned()
            },
            true,
        )
        .is_ok());
        assert!(public_wss_monitor_cycle_result_after_read_error(
            RuntimeError::LiveMarketData {
                message: "connect failed".to_owned()
            },
            false,
        )
        .is_err());
    }

    #[test]
    fn public_wss_failure_logging_suppresses_repeated_network_noise() {
        assert!(public_wss_should_log_failure(true, 1));
        assert!(public_wss_should_log_failure(false, 1));
        assert!(!public_wss_should_log_failure(false, 100));
        assert!(public_wss_should_log_failure(
            false,
            PUBLIC_WSS_FAILURE_LOG_REPEAT_INTERVAL
        ));
    }

    #[test]
    fn public_wss_monitor_allows_exchange_cadence_jitter() {
        let now = UtcTimestamp::from_str("2026-05-13T00:00:20Z").expect("now");
        let mut snapshot = PublicTopOfBookMonitorSnapshot::empty_with_market(
            "ALL_USDT",
            BitgetPublicWssMarket::UsdtFutures.as_str(),
            BITGET_PUBLIC_WSS_URL,
        );
        let quote = PublicTopOfBookQuoteSnapshot {
            symbol: "BTCUSDT".to_owned(),
            venue_id: "venue:BITGET-USDT-FUTURES".to_owned(),
            instrument_id: "inst:BITGET:BTCUSDT:USDT-FUTURES".to_owned(),
            best_bid: Some("100.01".to_owned()),
            best_ask: Some("100.02".to_owned()),
            bid_size: Some("1.2".to_owned()),
            ask_size: Some("1.3".to_owned()),
            source_sequence: Some("42".to_owned()),
            source_event_id: Some(
                "event:venue-data:bitget-public:wss-book-ticker:usdt-futures:BTCUSDT:42".to_owned(),
            ),
            observed_at: "2026-05-13T00:00:01Z".to_owned(),
            ingested_at: "2026-05-13T00:00:01Z".to_owned(),
            freshness_status: "Fresh".to_owned(),
        };
        snapshot.status = "streaming".to_owned();
        snapshot.updated_at = quote.ingested_at.clone();
        snapshot.wss_update_count = 1;
        snapshot.latest_quote = Some(quote.clone());
        snapshot.upsert_quote_row(quote);

        assert_eq!(snapshot.availability_status(Some(now)), "streaming");

        let stale_now = UtcTimestamp::from_str("2026-05-13T00:00:22Z").expect("stale now");
        assert_eq!(snapshot.availability_status(Some(stale_now)), "stale");
    }

    #[test]
    fn public_wss_monitor_reports_degraded_when_many_rows_are_stale() {
        let now = current_utc_timestamp().expect("now");
        let current_at = now.to_string();
        let mut snapshot = PublicTopOfBookMonitorSnapshot::empty_with_market(
            "ALL_USDT",
            BybitPublicMarket::LinearPerpetual.as_str(),
            BYBIT_LINEAR_PUBLIC_WSS_BASE_URL,
        );
        for (symbol, ingested_at) in [
            ("BTCUSDT", current_at.as_str()),
            ("ETHUSDT", "2000-01-01T00:00:00Z"),
            ("SOLUSDT", "2000-01-01T00:00:00Z"),
            ("XRPUSDT", "2000-01-01T00:00:00Z"),
        ] {
            let quote = PublicTopOfBookQuoteSnapshot {
                symbol: symbol.to_owned(),
                venue_id: "venue:BYBIT-PERP".to_owned(),
                instrument_id: format!("inst:BYBIT:{symbol}:LINEAR-PERP"),
                best_bid: Some("100.01".to_owned()),
                best_ask: Some("100.02".to_owned()),
                bid_size: Some("1.2".to_owned()),
                ask_size: Some("1.3".to_owned()),
                source_sequence: Some("42".to_owned()),
                source_event_id: Some(format!("bybit:wss-book-ticker:linear-perp:{symbol}:42")),
                observed_at: ingested_at.to_owned(),
                ingested_at: ingested_at.to_owned(),
                freshness_status: "Fresh".to_owned(),
            };
            if symbol == "BTCUSDT" {
                snapshot.latest_quote = Some(quote.clone());
            }
            snapshot.upsert_quote_row(quote);
        }
        snapshot.status = "streaming".to_owned();
        snapshot.updated_at = current_at;
        snapshot.wss_update_count = 4;

        let status = snapshot.to_json();

        assert_eq!(snapshot.availability_status(Some(now)), "degraded");
        assert_eq!(snapshot.health_http_status(), 503);
        assert!(status.contains("\"current_usable_row_count\":1"));
        assert!(status.contains("\"stale_row_count\":3"));
        assert!(status.contains("stale_wss_row_ratio_bps=7500"));
    }

    #[test]
    fn public_wss_monitor_does_not_degrade_for_fresh_rest_bootstrap_rows() {
        let now = current_utc_timestamp().expect("now");
        let current_at = now.to_string();
        let mut snapshot = PublicTopOfBookMonitorSnapshot::empty_with_market(
            "ALL_USDT",
            BybitPublicMarket::LinearPerpetual.as_str(),
            BYBIT_LINEAR_PUBLIC_WSS_BASE_URL,
        );
        for (symbol, source_event_id) in [
            ("BTCUSDT", "bybit:wss-book-ticker:linear-perp:BTCUSDT:42"),
            ("ETHUSDT", "bybit:rest-tickers:linear-perp:ETHUSDT"),
            ("SOLUSDT", "bybit:rest-tickers:linear-perp:SOLUSDT"),
            ("XRPUSDT", "bybit:rest-tickers:linear-perp:XRPUSDT"),
        ] {
            let quote = PublicTopOfBookQuoteSnapshot {
                symbol: symbol.to_owned(),
                venue_id: "venue:BYBIT-PERP".to_owned(),
                instrument_id: format!("inst:BYBIT:{symbol}:LINEAR-PERP"),
                best_bid: Some("100.01".to_owned()),
                best_ask: Some("100.02".to_owned()),
                bid_size: Some("1.2".to_owned()),
                ask_size: Some("1.3".to_owned()),
                source_sequence: Some("42".to_owned()),
                source_event_id: Some(source_event_id.to_owned()),
                observed_at: current_at.clone(),
                ingested_at: current_at.clone(),
                freshness_status: "Fresh".to_owned(),
            };
            if symbol == "BTCUSDT" {
                snapshot.latest_quote = Some(quote.clone());
            }
            snapshot.upsert_quote_row(quote);
        }
        snapshot.status = "streaming".to_owned();
        snapshot.updated_at = current_at;
        snapshot.wss_update_count = 1;

        let status = snapshot.to_json();

        assert_eq!(snapshot.availability_status(Some(now)), "streaming");
        assert_eq!(snapshot.health_http_status(), 200);
        assert!(status.contains("\"current_usable_row_count\":1"));
        assert!(status.contains("\"unusable_row_count\":3"));
        assert!(status.contains("\"degraded_reason\":null"));
    }

    #[test]
    fn public_wss_monitor_reports_degraded_when_reconnecting_under_high_pressure() {
        let now = current_utc_timestamp().expect("now");
        let current_at = now.to_string();
        let mut snapshot = PublicTopOfBookMonitorSnapshot::empty_with_market(
            "ALL_USDT",
            BybitPublicMarket::LinearPerpetual.as_str(),
            BYBIT_LINEAR_PUBLIC_WSS_BASE_URL,
        );
        let quote = PublicTopOfBookQuoteSnapshot {
            symbol: "BTCUSDT".to_owned(),
            venue_id: "venue:BYBIT-PERP".to_owned(),
            instrument_id: "inst:BYBIT:BTCUSDT:LINEAR-PERP".to_owned(),
            best_bid: Some("100.01".to_owned()),
            best_ask: Some("100.02".to_owned()),
            bid_size: Some("1.2".to_owned()),
            ask_size: Some("1.3".to_owned()),
            source_sequence: Some("42".to_owned()),
            source_event_id: Some("bybit:wss-book-ticker:linear-perp:BTCUSDT:42".to_owned()),
            observed_at: current_at.clone(),
            ingested_at: current_at.clone(),
            freshness_status: "Fresh".to_owned(),
        };
        snapshot.status = "reconnecting".to_owned();
        snapshot.updated_at = current_at;
        snapshot.disconnect_count = PUBLIC_WSS_DEGRADED_RECONNECT_COUNT;
        snapshot.rest_rebuild_count = PUBLIC_WSS_DEGRADED_RECONNECT_COUNT;
        snapshot.fail_closed_count = PUBLIC_WSS_DEGRADED_RECONNECT_COUNT;
        snapshot.wss_update_count = 1;
        snapshot.latest_quote = Some(quote.clone());
        snapshot.upsert_quote_row(quote);

        let status = snapshot.to_json();

        assert_eq!(snapshot.availability_status(Some(now)), "degraded");
        assert_eq!(snapshot.health_http_status(), 503);
        assert!(status.contains("high_reconnect_pressure"));
    }

    #[test]
    fn public_wss_monitor_does_not_degrade_after_streaming_recovers_from_high_pressure() {
        let now = current_utc_timestamp().expect("now");
        let current_at = now.to_string();
        let mut snapshot = PublicTopOfBookMonitorSnapshot::empty_with_market(
            "ALL_USDT",
            BybitPublicMarket::LinearPerpetual.as_str(),
            BYBIT_LINEAR_PUBLIC_WSS_BASE_URL,
        );
        let quote = PublicTopOfBookQuoteSnapshot {
            symbol: "BTCUSDT".to_owned(),
            venue_id: "venue:BYBIT-PERP".to_owned(),
            instrument_id: "inst:BYBIT:BTCUSDT:LINEAR-PERP".to_owned(),
            best_bid: Some("100.01".to_owned()),
            best_ask: Some("100.02".to_owned()),
            bid_size: Some("1.2".to_owned()),
            ask_size: Some("1.3".to_owned()),
            source_sequence: Some("42".to_owned()),
            source_event_id: Some("bybit:wss-book-ticker:linear-perp:BTCUSDT:42".to_owned()),
            observed_at: current_at.clone(),
            ingested_at: current_at.clone(),
            freshness_status: "Fresh".to_owned(),
        };
        snapshot.status = "streaming".to_owned();
        snapshot.updated_at = current_at;
        snapshot.disconnect_count = PUBLIC_WSS_DEGRADED_RECONNECT_COUNT;
        snapshot.rest_rebuild_count = PUBLIC_WSS_DEGRADED_RECONNECT_COUNT;
        snapshot.fail_closed_count = PUBLIC_WSS_DEGRADED_RECONNECT_COUNT;
        snapshot.wss_update_count = 1;
        snapshot.latest_quote = Some(quote.clone());
        snapshot.upsert_quote_row(quote);

        let status = snapshot.to_json();

        assert_eq!(snapshot.availability_status(Some(now)), "streaming");
        assert_eq!(snapshot.health_http_status(), 200);
        assert!(status.contains("\"degraded_reason\":null"));
    }

    #[test]
    fn public_wss_monitor_snapshot_reports_stale_when_rows_stop_updating() {
        let mut snapshot = PublicTopOfBookMonitorSnapshot::empty_with_market(
            "ALL_USDT",
            AsterPublicWssMarket::UsdtFutures.as_str(),
            ASTER_PUBLIC_WSS_BASE_URL,
        );
        let quote = PublicTopOfBookQuoteSnapshot {
            symbol: "BTCUSDT".to_owned(),
            venue_id: "venue:ASTER-USDT-FUTURES".to_owned(),
            instrument_id: "inst:ASTER:BTCUSDT:USDT-FUTURES".to_owned(),
            best_bid: Some("100.01".to_owned()),
            best_ask: Some("100.02".to_owned()),
            bid_size: Some("1.2".to_owned()),
            ask_size: Some("1.3".to_owned()),
            source_sequence: Some("42".to_owned()),
            source_event_id: Some("aster:wss-book-ticker:usdt-futures:BTCUSDT:42".to_owned()),
            observed_at: "2000-01-01T00:00:00Z".to_owned(),
            ingested_at: "2000-01-01T00:00:00Z".to_owned(),
            freshness_status: "Fresh".to_owned(),
        };
        snapshot.status = "streaming".to_owned();
        snapshot.updated_at = "2000-01-01T00:00:00Z".to_owned();
        snapshot.wss_update_count = 1;
        snapshot.latest_quote = Some(quote.clone());
        snapshot.upsert_quote_row(quote);

        let health = snapshot.health_json();
        let status = snapshot.to_json();

        assert_eq!(snapshot.health_http_status(), 503);
        assert!(health.contains("\"status\":\"stale\""));
        assert!(status.contains("\"status\":\"stale\""));
        assert!(status.contains("\"stream_status\":\"streaming\""));
        assert!(status.contains("no currently usable WSS quote rows"));
        assert!(status.contains("\"freshness_status\":\"Stale\""));
    }

    #[test]
    fn public_wss_monitor_snapshot_reports_unhealthy_when_reconnecting_without_current_rows() {
        let mut snapshot = PublicTopOfBookMonitorSnapshot::empty_with_market(
            "ALL_USDT",
            BybitPublicMarket::LinearPerpetual.as_str(),
            BYBIT_LINEAR_PUBLIC_WSS_BASE_URL,
        );
        let quote = PublicTopOfBookQuoteSnapshot {
            symbol: "BTCUSDT".to_owned(),
            venue_id: "venue:BYBIT-PERP".to_owned(),
            instrument_id: "inst:BYBIT:BTCUSDT:LINEAR-PERP".to_owned(),
            best_bid: Some("100.01".to_owned()),
            best_ask: Some("100.02".to_owned()),
            bid_size: Some("1.2".to_owned()),
            ask_size: Some("1.3".to_owned()),
            source_sequence: Some("42".to_owned()),
            source_event_id: Some("bybit:wss-book-ticker:linear-perp:BTCUSDT:42".to_owned()),
            observed_at: "2000-01-01T00:00:00Z".to_owned(),
            ingested_at: "2000-01-01T00:00:00Z".to_owned(),
            freshness_status: "Fresh".to_owned(),
        };
        snapshot.status = "reconnecting".to_owned();
        snapshot.updated_at = "2000-01-01T00:00:00Z".to_owned();
        snapshot.wss_update_count = 1;
        snapshot.latest_quote = Some(quote.clone());
        snapshot.upsert_quote_row(quote);

        let health = snapshot.health_json();
        let status = snapshot.to_json();

        assert_eq!(snapshot.health_http_status(), 503);
        assert!(health.contains("\"status\":\"reconnecting\""));
        assert!(health.contains("\"stream_status\":\"reconnecting\""));
        assert!(status.contains("\"status\":\"reconnecting\""));
        assert!(status.contains("\"stream_status\":\"reconnecting\""));
        assert!(status.contains("\"freshness_status\":\"Stale\""));
    }
}
