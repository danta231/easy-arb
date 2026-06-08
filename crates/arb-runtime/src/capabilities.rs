use arb_contracts::{from_json_strict, VenueCapabilityDescriptor};

use crate::json_util::{json_string, json_string_array};
use crate::{RuntimeError, RuntimeResult};

/// 交易所套利能力画像。
///
/// 中文说明：这是运行时对交易所能力的单一事实来源。策略和管线仍消费合同层的
/// `VenueCapabilityDescriptor`，但这些 descriptor 由这里的画像派生，避免在多个
/// monitor 或测试入口重复编码交易所假设。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbVenueCapabilityProfile {
    pub venue_family: String,
    pub supports_spot: bool,
    pub supports_linear_perp: bool,
    pub supports_swap: bool,
    pub supports_funding_rate: bool,
    pub supports_mark_index_price: bool,
    pub supports_top_of_book_size: bool,
    pub funding_interval_hours: Option<u64>,
    pub funding_settlement_price_source: FundingSettlementPriceSource,
    pub dry_run_execution_supported: bool,
    pub runtime_live_execution_supported: bool,
    pub runtime_private_order_confirmation_supported: bool,
    pub runtime_auto_funding_settlement_supported: bool,
}

/// 资金费率结算价格来源。
///
/// 中文说明：不同永续交易场所的资金费率并不总是按同一价格口径结算。运行时先把
/// 该差异显式建模，后续候选和对账必须按场所口径解释预期收益。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FundingSettlementPriceSource {
    MarkPrice,
    OraclePrice,
}

impl FundingSettlementPriceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MarkPrice => "mark_price",
            Self::OraclePrice => "oracle_price",
        }
    }
}

impl ArbVenueCapabilityProfile {
    pub fn supports_spot_perp_basis(&self) -> bool {
        self.supports_spot
            && (self.supports_linear_perp || self.supports_swap)
            && self.supports_funding_rate
            && self.supports_mark_index_price
            && self.supports_top_of_book_size
            && self.dry_run_execution_supported
    }

    pub fn supports_cross_exchange_funding_arb(&self) -> bool {
        (self.supports_linear_perp || self.supports_swap)
            && self.supports_funding_rate
            && self.supports_mark_index_price
            && self.supports_top_of_book_size
            && self.dry_run_execution_supported
    }
}

/// 单交易所策略支持矩阵行。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbVenueStrategySupport {
    pub venue_family: String,
    pub supports_spot_perp_basis: bool,
    pub supports_cross_exchange_funding_arb: bool,
}

/// 跨交易所资金费率套利的无向交易所组合。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingArbVenuePair {
    pub venue_a: String,
    pub venue_b: String,
}

/// 返回交易所能力矩阵。
///
/// 中文说明：Gate 和 Lighter 已进入公开行情、私有只读和历史同步链路；可变执行
/// adapter（适配器）仍未开放，因此不能进入策略支持矩阵或默认实盘下单路径。
pub fn arb_venue_capability_profiles() -> Vec<ArbVenueCapabilityProfile> {
    vec![
        ArbVenueCapabilityProfile {
            venue_family: "binance".to_owned(),
            supports_spot: true,
            supports_linear_perp: true,
            supports_swap: false,
            supports_funding_rate: true,
            supports_mark_index_price: true,
            supports_top_of_book_size: true,
            funding_interval_hours: Some(8),
            funding_settlement_price_source: FundingSettlementPriceSource::MarkPrice,
            dry_run_execution_supported: true,
            runtime_live_execution_supported: true,
            runtime_private_order_confirmation_supported: true,
            runtime_auto_funding_settlement_supported: true,
        },
        ArbVenueCapabilityProfile {
            venue_family: "bybit".to_owned(),
            supports_spot: true,
            supports_linear_perp: true,
            supports_swap: false,
            supports_funding_rate: true,
            supports_mark_index_price: true,
            supports_top_of_book_size: true,
            funding_interval_hours: Some(8),
            funding_settlement_price_source: FundingSettlementPriceSource::MarkPrice,
            dry_run_execution_supported: true,
            runtime_live_execution_supported: true,
            runtime_private_order_confirmation_supported: true,
            runtime_auto_funding_settlement_supported: true,
        },
        ArbVenueCapabilityProfile {
            venue_family: "okx".to_owned(),
            supports_spot: true,
            supports_linear_perp: false,
            supports_swap: true,
            supports_funding_rate: true,
            supports_mark_index_price: true,
            supports_top_of_book_size: true,
            funding_interval_hours: Some(8),
            funding_settlement_price_source: FundingSettlementPriceSource::MarkPrice,
            dry_run_execution_supported: true,
            runtime_live_execution_supported: true,
            runtime_private_order_confirmation_supported: true,
            runtime_auto_funding_settlement_supported: true,
        },
        ArbVenueCapabilityProfile {
            venue_family: "bitget".to_owned(),
            supports_spot: true,
            supports_linear_perp: true,
            supports_swap: false,
            supports_funding_rate: true,
            supports_mark_index_price: true,
            supports_top_of_book_size: true,
            funding_interval_hours: Some(8),
            funding_settlement_price_source: FundingSettlementPriceSource::MarkPrice,
            dry_run_execution_supported: true,
            runtime_live_execution_supported: true,
            runtime_private_order_confirmation_supported: true,
            runtime_auto_funding_settlement_supported: true,
        },
        ArbVenueCapabilityProfile {
            venue_family: "aster".to_owned(),
            supports_spot: false,
            supports_linear_perp: true,
            supports_swap: false,
            supports_funding_rate: true,
            supports_mark_index_price: true,
            supports_top_of_book_size: true,
            funding_interval_hours: Some(8),
            funding_settlement_price_source: FundingSettlementPriceSource::MarkPrice,
            dry_run_execution_supported: true,
            runtime_live_execution_supported: true,
            runtime_private_order_confirmation_supported: true,
            runtime_auto_funding_settlement_supported: true,
        },
        ArbVenueCapabilityProfile {
            venue_family: "hyperliquid".to_owned(),
            supports_spot: false,
            supports_linear_perp: true,
            supports_swap: false,
            supports_funding_rate: true,
            supports_mark_index_price: true,
            supports_top_of_book_size: true,
            funding_interval_hours: Some(1),
            funding_settlement_price_source: FundingSettlementPriceSource::OraclePrice,
            dry_run_execution_supported: true,
            runtime_live_execution_supported: true,
            runtime_private_order_confirmation_supported: true,
            runtime_auto_funding_settlement_supported: true,
        },
        ArbVenueCapabilityProfile {
            venue_family: "gate".to_owned(),
            supports_spot: true,
            supports_linear_perp: true,
            supports_swap: false,
            supports_funding_rate: true,
            supports_mark_index_price: true,
            supports_top_of_book_size: true,
            funding_interval_hours: Some(8),
            funding_settlement_price_source: FundingSettlementPriceSource::MarkPrice,
            dry_run_execution_supported: false,
            runtime_live_execution_supported: false,
            runtime_private_order_confirmation_supported: false,
            runtime_auto_funding_settlement_supported: false,
        },
        ArbVenueCapabilityProfile {
            venue_family: "lighter".to_owned(),
            supports_spot: false,
            supports_linear_perp: true,
            supports_swap: false,
            supports_funding_rate: true,
            supports_mark_index_price: true,
            supports_top_of_book_size: true,
            funding_interval_hours: Some(1),
            funding_settlement_price_source: FundingSettlementPriceSource::MarkPrice,
            dry_run_execution_supported: false,
            runtime_live_execution_supported: false,
            runtime_private_order_confirmation_supported: false,
            runtime_auto_funding_settlement_supported: false,
        },
    ]
}

pub fn arb_venue_capability_profile(venue_family: &str) -> Option<ArbVenueCapabilityProfile> {
    let normalized = normalize_venue_family(venue_family);
    arb_venue_capability_profiles()
        .into_iter()
        .find(|profile| profile.venue_family == normalized)
}

/// 返回双策略支持矩阵。
pub fn arb_venue_strategy_support_matrix() -> Vec<ArbVenueStrategySupport> {
    arb_venue_capability_profiles()
        .into_iter()
        .map(|profile| ArbVenueStrategySupport {
            venue_family: profile.venue_family.clone(),
            supports_spot_perp_basis: profile.supports_spot_perp_basis(),
            supports_cross_exchange_funding_arb: profile.supports_cross_exchange_funding_arb(),
        })
        .collect()
}

/// 返回已完成 dry-run 验收、支持跨所 funding arb 的 15 个无向交易所组合。
pub fn cross_exchange_funding_arb_venue_pairs() -> Vec<CrossExchangeFundingArbVenuePair> {
    let venues = arb_venue_capability_profiles()
        .into_iter()
        .filter(|profile| profile.supports_cross_exchange_funding_arb())
        .map(|profile| profile.venue_family)
        .collect::<Vec<_>>();
    let mut pairs = Vec::new();
    for left in 0..venues.len() {
        for right in (left + 1)..venues.len() {
            pairs.push(CrossExchangeFundingArbVenuePair {
                venue_a: venues[left].clone(),
                venue_b: venues[right].clone(),
            });
        }
    }
    pairs
}

/// 从能力画像派生合同层 venue capability descriptor。
pub fn arb_venue_capability_descriptors(
    venue_family: &str,
) -> RuntimeResult<Vec<VenueCapabilityDescriptor>> {
    let normalized = normalize_venue_family(venue_family);
    let Some(profile) = arb_venue_capability_profile(&normalized) else {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!("unknown venue family `{venue_family}`"),
        });
    };

    let mut descriptors = Vec::new();
    match normalized.as_str() {
        "binance" => {
            if profile.supports_spot {
                descriptors.push(public_venue_capability_descriptor(
                    "venue:BINANCE-SPOT",
                    "Binance Spot Public REST",
                    &["ProvidesSpotMarkets", "ProvidesOrderBookMarkets"],
                    &["RESTPolling", "RateLimitHeaders"],
                    "binance-public-rest",
                    1200,
                    60_000,
                )?);
            }
            descriptors.push(public_venue_capability_descriptor(
                "venue:BINANCE-USDM",
                "Binance USD-M Public REST",
                &[
                    "ProvidesPerpetuals",
                    "ProvidesOrderBookMarkets",
                    "ProvidesFundingRates",
                    "ProvidesOraclePrices",
                ],
                &[
                    "RESTPolling",
                    "WebSocketStreaming",
                    "RateLimitHeaders",
                    "FundingHistory",
                ],
                "binance-public-futures-rest",
                2400,
                60_000,
            )?);
        }
        "bybit" => {
            if profile.supports_spot {
                descriptors.push(public_venue_capability_descriptor(
                    "venue:BYBIT-SPOT",
                    "Bybit Spot Public REST",
                    &["ProvidesSpotMarkets", "ProvidesOrderBookMarkets"],
                    &["RESTPolling", "RateLimitHeaders"],
                    "bybit-public-spot-rest",
                    600,
                    5_000,
                )?);
            }
            descriptors.push(public_venue_capability_descriptor(
                "venue:BYBIT-LINEAR",
                "Bybit Linear Public REST",
                &[
                    "ProvidesPerpetuals",
                    "ProvidesOrderBookMarkets",
                    "ProvidesFundingRates",
                    "ProvidesOraclePrices",
                ],
                &[
                    "RESTPolling",
                    "WebSocketStreaming",
                    "RateLimitHeaders",
                    "FundingHistory",
                ],
                "bybit-public-linear-rest",
                600,
                5_000,
            )?);
        }
        "okx" => {
            if profile.supports_spot {
                descriptors.push(public_venue_capability_descriptor(
                    "venue:OKX-SPOT",
                    "OKX Spot Public REST",
                    &["ProvidesSpotMarkets", "ProvidesOrderBookMarkets"],
                    &["RESTPolling", "RateLimitHeaders"],
                    "okx-public-rest",
                    600,
                    2_000,
                )?);
            }
            descriptors.push(public_venue_capability_descriptor(
                "venue:OKX-SWAP",
                "OKX Swap Public REST",
                &[
                    "ProvidesPerpetuals",
                    "ProvidesOrderBookMarkets",
                    "ProvidesFundingRates",
                    "ProvidesOraclePrices",
                ],
                &["RESTPolling", "RateLimitHeaders", "FundingHistory"],
                "okx-public-swap-rest",
                600,
                2_000,
            )?);
        }
        "bitget" => {
            if profile.supports_spot {
                descriptors.push(public_venue_capability_descriptor(
                    "venue:BITGET-SPOT",
                    "Bitget Spot Public REST",
                    &["ProvidesSpotMarkets", "ProvidesOrderBookMarkets"],
                    &["RESTPolling", "RateLimitHeaders"],
                    "bitget-public-spot-rest",
                    600,
                    60_000,
                )?);
            }
            descriptors.push(public_venue_capability_descriptor(
                "venue:BITGET-USDT-FUTURES",
                "Bitget USDT Futures Public REST",
                &[
                    "ProvidesPerpetuals",
                    "ProvidesOrderBookMarkets",
                    "ProvidesFundingRates",
                    "ProvidesOraclePrices",
                ],
                &["RESTPolling", "RateLimitHeaders", "FundingHistory"],
                "bitget-public-usdt-futures-rest",
                600,
                60_000,
            )?);
        }
        "aster" => {
            descriptors.push(public_venue_capability_descriptor(
                "venue:ASTER-USDT-FUTURES",
                "Aster USDT Futures Public REST",
                &[
                    "ProvidesPerpetuals",
                    "ProvidesOrderBookMarkets",
                    "ProvidesFundingRates",
                    "ProvidesOraclePrices",
                ],
                &[
                    "RESTPolling",
                    "WebSocketStreaming",
                    "RateLimitHeaders",
                    "FundingHistory",
                ],
                "aster-public-futures-rest",
                2400,
                60_000,
            )?);
        }
        "hyperliquid" => {
            descriptors.push(public_venue_capability_descriptor(
                "venue:HYPERLIQUID-PERP",
                "Hyperliquid Perp Public Info",
                &[
                    "ProvidesPerpetuals",
                    "ProvidesOrderBookMarkets",
                    "ProvidesFundingRates",
                    "ProvidesOraclePrices",
                ],
                &["RESTPolling", "WebSocketStreaming", "FundingHistory"],
                "hyperliquid-public-info",
                1200,
                60_000,
            )?);
        }
        "gate" => {
            if profile.supports_spot {
                descriptors.push(public_venue_capability_descriptor(
                    "venue:GATE-SPOT",
                    "Gate Spot Public REST",
                    &["ProvidesSpotMarkets", "ProvidesOrderBookMarkets"],
                    &["RESTPolling", "WebSocketStreaming", "RateLimitHeaders"],
                    "gate-public-spot-rest",
                    600,
                    60_000,
                )?);
            }
            descriptors.push(public_venue_capability_descriptor(
                "venue:GATE-USDT-FUTURES",
                "Gate USDT Futures Public REST",
                &[
                    "ProvidesPerpetuals",
                    "ProvidesOrderBookMarkets",
                    "ProvidesFundingRates",
                    "ProvidesOraclePrices",
                ],
                &[
                    "RESTPolling",
                    "WebSocketStreaming",
                    "RateLimitHeaders",
                    "FundingHistory",
                ],
                "gate-public-usdt-futures-rest",
                600,
                60_000,
            )?);
        }
        "lighter" => {
            descriptors.push(public_venue_capability_descriptor(
                "venue:LIGHTER-PERP",
                "Lighter Perp Public API",
                &[
                    "ProvidesPerpetuals",
                    "ProvidesOrderBookMarkets",
                    "ProvidesFundingRates",
                    "ProvidesOraclePrices",
                ],
                &["RESTPolling", "WebSocketStreaming", "FundingHistory"],
                "lighter-public-api",
                60,
                60_000,
            )?);
        }
        _ => unreachable!("venue family checked above"),
    }

    Ok(descriptors)
}

pub(super) fn normalize_venue_family(venue_family: &str) -> String {
    let compact = venue_family
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-', ' '], "");
    match compact.as_str() {
        value if value.starts_with("binance") => "binance".to_owned(),
        value if value.starts_with("bybit") => "bybit".to_owned(),
        value if value.starts_with("okx") => "okx".to_owned(),
        value if value.starts_with("bitget") => "bitget".to_owned(),
        value if value.starts_with("aster") => "aster".to_owned(),
        value if value.starts_with("hyperliquid") => "hyperliquid".to_owned(),
        value if value.starts_with("gateio") || value.starts_with("gate") => "gate".to_owned(),
        value if value.starts_with("zklighter") || value.starts_with("lighter") => {
            "lighter".to_owned()
        }
        _ => compact,
    }
}

fn public_venue_capability_descriptor(
    venue_id: &str,
    venue_name: &str,
    market_capabilities: &[&str],
    data_surfaces: &[&str],
    rate_limit_source: &str,
    rate_limit: u64,
    rate_limit_window_ms: u64,
) -> RuntimeResult<VenueCapabilityDescriptor> {
    let market_capabilities = json_static_string_array(market_capabilities);
    let data_surfaces = json_static_string_array(data_surfaces);
    let descriptor = format!(
        r#"{{"auth_modes":["PublicOnly"],"capability_version":"1.0.0","data_surfaces":{data_surfaces},"execution_capabilities":["SupportsManualApprovalOnly"],"health_model":{{"disconnect_threshold":3,"freshness_threshold_ms":5000,"unknown_state_is_critical":true}},"market_capabilities":{market_capabilities},"permission_model":{{"can_read_private_data":false,"can_read_public_data":true,"can_trade":false,"can_withdraw":false}},"rate_limit_model":{{"limit":{rate_limit},"source":{},"unit":"Request","window_ms":{rate_limit_window_ms}}},"schema_version":"1.0.0","settlement_modes":["OffChainCustody"],"venue_id":{},"venue_name":{}}}"#,
        json_string(rate_limit_source),
        json_string(venue_id),
        json_string(venue_name),
    );
    Ok(from_json_strict::<VenueCapabilityDescriptor>(&descriptor)?)
}

fn json_static_string_array(values: &[&str]) -> String {
    let values = values
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    json_string_array(&values)
}
