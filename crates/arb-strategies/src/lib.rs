//! `arb-strategies` 样例策略集合。
//!
//! 中文说明：本 crate 只依赖 `arb-strategy-api`，策略只能读取只读上下文并
//! 输出 `CandidatePortfolioTransition` 或明确拒绝原因。这里不暴露下单、
//! 签名、转账、账本写入或运行时装配能力。

#![forbid(unsafe_code)]

use arb_strategy_api::{
    candidate_from_json_strict, validate_candidate_for_context, CandidatePortfolioTransition,
    DataSurface, Identifier, JsonValue, MarketCapability, NormalizedEvent, NormalizedEventType,
    Strategy, StrategyApiError, StrategyApiResult, StrategyDiagnostic, StrategyEvaluation,
    StrategyMetadata, StrategyReadContext, StrategyRejectReason, StrategyRejection,
};

const SAMPLE_STRATEGY_ID: &str = "strat:sample-spot-demo";
const SAMPLE_STRATEGY_VERSION: &str = "1.0.0";
const SAMPLE_CODE_VERSION: &str = "code:sample-spot-demo-1";
const SAMPLE_VENUE_ID: &str = "venue:SIM";
const SAMPLE_INSTRUMENT_ID: &str = "inst:BTC-USDC";
const SAMPLE_TRANSITION_ID: &str = "trans:sample-spot-001";
const BINANCE_BASIS_STRATEGY_ID: &str = "strat:binance-spot-perp-basis";
const SPOT_PERP_BASIS_STRATEGY_VERSION: &str = "1.0.0";
const BINANCE_BASIS_CODE_VERSION: &str = "code:binance-spot-perp-basis-1";
const BINANCE_BASIS_SPOT_VENUE_ID: &str = "venue:BINANCE-SPOT";
const BINANCE_BASIS_PERP_VENUE_ID: &str = "venue:BINANCE-USDM";
const BINANCE_BASIS_SPOT_INSTRUMENT_ID: &str = "inst:BINANCE:BTCUSDT:SPOT";
const BINANCE_BASIS_PERP_INSTRUMENT_ID: &str = "inst:BINANCE:BTCUSDT:USDM-PERP";
const BINANCE_BASIS_TRANSITION_ID: &str = "trans:binance-basis-btcusdt-001";
const DEFAULT_BASIS_NOTIONAL_USD: &str = "100.00";
const BINANCE_BASIS_SPOT_TAKER_FEE_BPS: &str = "7.5";
const BINANCE_BASIS_PERP_TAKER_FEE_BPS: &str = "4.5";
const BYBIT_BASIS_SPOT_TAKER_FEE_BPS: &str = "10";
const BYBIT_BASIS_PERP_TAKER_FEE_BPS: &str = "5";
const OKX_BASIS_SPOT_TAKER_FEE_BPS: &str = "10";
const OKX_BASIS_PERP_TAKER_FEE_BPS: &str = "5";
const DEFAULT_BASIS_SLIPPAGE_BUFFER_BPS: i128 = 5;
const DEFAULT_BASIS_MIN_NET_BPS: i128 = 5;
const DEFAULT_BASIS_EXIT_POLICY_REF: &str = "exit-policy:spot-perp-basis-composite-v1";
const BYBIT_BASIS_STRATEGY_ID: &str = "strat:bybit-spot-perp-basis";
const BYBIT_BASIS_CODE_VERSION: &str = "code:bybit-spot-perp-basis-1";
const BYBIT_BASIS_SPOT_VENUE_ID: &str = "venue:BYBIT-SPOT";
const BYBIT_BASIS_PERP_VENUE_ID: &str = "venue:BYBIT-LINEAR";
const BYBIT_BASIS_SPOT_INSTRUMENT_ID: &str = "inst:BYBIT:BTCUSDT:SPOT";
const BYBIT_BASIS_PERP_INSTRUMENT_ID: &str = "inst:BYBIT:BTCUSDT:LINEAR-PERP";
const BYBIT_BASIS_TRANSITION_ID: &str = "trans:bybit-basis-btcusdt-001";
const OKX_BASIS_STRATEGY_ID: &str = "strat:okx-spot-swap-basis";
const OKX_BASIS_CODE_VERSION: &str = "code:okx-spot-swap-basis-1";
const OKX_BASIS_SPOT_VENUE_ID: &str = "venue:OKX-SPOT";
const OKX_BASIS_PERP_VENUE_ID: &str = "venue:OKX-SWAP";
const OKX_BASIS_SPOT_INSTRUMENT_ID: &str = "inst:OKX:BTC-USDT:SPOT";
const OKX_BASIS_PERP_INSTRUMENT_ID: &str = "inst:OKX:BTC-USDT-SWAP:SWAP";
const OKX_BASIS_TRANSITION_ID: &str = "trans:okx-basis-btc-usdt-001";
const CROSS_EXCHANGE_FUNDING_ARB_STRATEGY_ID: &str = "strat:cross-exchange-funding-arb";
const CROSS_EXCHANGE_FUNDING_ARB_STRATEGY_VERSION: &str = "1.0.0";
const CROSS_EXCHANGE_FUNDING_ARB_CODE_VERSION: &str = "code:cross-exchange-funding-arb-1";
const CROSS_EXCHANGE_FUNDING_ARB_TRANSITION_ID: &str =
    "trans:cross-exchange-funding-arb-btcusdt-001";
const CROSS_EXCHANGE_MAX_MARK_INDEX_DIVERGENCE_BPS: i128 = 100;
const FIXED_SCALE: i128 = 100_000_000;
const FIXED_SCALE_DIGITS: usize = 8;

/// 第一个只读样例策略。
///
/// 中文说明：策略只检查固定样例场所是否声明现货市场和 REST 轮询数据能力；
/// 能力满足时输出候选组合转换，能力不足、熔断或配置禁用时返回拒绝。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SampleSpotStrategy {
    metadata: StrategyMetadata,
}

impl SampleSpotStrategy {
    /// 创建样例策略。
    pub fn new() -> StrategyApiResult<Self> {
        Ok(Self {
            metadata: StrategyMetadata::new(
                SAMPLE_STRATEGY_ID,
                SAMPLE_STRATEGY_VERSION,
                SAMPLE_CODE_VERSION,
            )?,
        })
    }

    fn reject(
        &self,
        context: &dyn StrategyReadContext,
        reason: StrategyRejectReason,
        detail: impl Into<String>,
    ) -> StrategyApiResult<StrategyEvaluation> {
        let detail = detail.into();
        let rejection = StrategyRejection::new(
            &self.metadata,
            context.time().now(),
            reason,
            Some(detail.clone()),
            source_event_refs(context),
            context.snapshot().portfolio_state_id(),
        )?;
        let diagnostic =
            StrategyDiagnostic::new("SAMPLE_SPOT_REJECTED", detail, context.time().now())?;
        Ok(StrategyEvaluation::rejected(rejection).with_diagnostic(diagnostic))
    }

    fn build_candidate(
        &self,
        context: &dyn StrategyReadContext,
        quote: &MarketQuoteInput,
    ) -> StrategyApiResult<CandidatePortfolioTransition> {
        let input_event_refs = source_event_refs(context);
        let input_event_refs_json = json_string_array(&input_event_refs);
        let config_version = context.config().config_version();
        let config_hash = context.config().config_hash();
        let assumption = format!(
            "Sample strategy read market quote {} with best_bid={}, best_ask={}, last_price={} using config {config_version} hash {config_hash}.",
            quote.event_id, quote.best_bid, quote.best_ask, quote.last_price
        );

        let candidate_json = format!(
            r#"{{
  "schema_version": "1.0.0",
  "transition_id": {},
  "strategy_id": {},
  "strategy_version": {},
  "code_version": {},
  "config_version": {},
  "created_at": {},
  "input_event_refs": {},
  "current_portfolio_state_ref": {},
  "holding_period": {{
    "kind": "Instant"
  }},
  "legs": [
    {{
      "leg_id": "candleg:sample-spot-buy",
      "leg_type": "Trade",
      "venue_id": "venue:SIM",
      "instrument_id": "inst:BTC-USDC",
      "account_id": "acct:sim",
      "side": "Buy",
      "asset_flows": [
        {{
          "asset_id": "asset:USDC",
          "direction": "Out",
          "amount": "100.00",
          "account_id": "acct:sim"
        }}
      ],
      "constraints": {{
        "max_slippage_bps": "5",
        "post_only": false,
        "reference_best_ask": {},
        "reference_best_bid": {},
        "reference_last_price": {},
        "reference_market_event_id": {}
      }},
      "failure_modes": [
        "NoOpFailure"
      ]
    }}
  ],
  "expected_post_state_delta": {{
    "asset_flows": [],
    "position_deltas": [
      {{
        "instrument_id": "inst:BTC-USDC",
        "account_id": "acct:sim",
        "quantity_delta": "0.001"
      }}
    ]
  }},
  "expected_economics": {{
    "expected_profit_usd": "1.23",
    "expected_profit_bps": "12.30",
    "fee_estimate_usd": "0.10",
    "slippage_estimate_usd": "0.05",
    "confidence": 0.95
  }},
  "required_capital": {{
    "asset_requirements": [
      {{
        "asset_id": "asset:USDC",
        "direction": "Out",
        "amount": "100.00",
        "account_id": "acct:sim"
      }}
    ],
    "recovery_buffer_usd": "1.00"
  }},
  "failure_modes": [
    "NoOpFailure"
  ],
  "risk_flags": [],
  "assumptions": [
    {{
      "assumption_id": "asm:sample-spot-readonly",
      "statement": {},
      "confidence": 0.9,
      "source_event_refs": {}
    }}
  ]
}}"#,
            json_string(SAMPLE_TRANSITION_ID),
            json_string(self.metadata.strategy_id()),
            json_string(self.metadata.strategy_version()),
            json_string(self.metadata.code_version()),
            json_string(config_version),
            json_string(&context.time().now_rfc3339_z()),
            input_event_refs_json,
            json_string(context.snapshot().portfolio_state_id()),
            json_string(&quote.best_ask),
            json_string(&quote.best_bid),
            json_string(&quote.last_price),
            json_string(&quote.event_id),
            json_string(&assumption),
            json_string_array(&input_event_refs),
        );

        candidate_from_json_strict(&candidate_json)
    }
}

impl Default for SampleSpotStrategy {
    fn default() -> Self {
        Self::new().expect("sample strategy metadata is static and valid")
    }
}

impl Strategy for SampleSpotStrategy {
    fn metadata(&self) -> &StrategyMetadata {
        &self.metadata
    }

    fn evaluate(&self, context: &dyn StrategyReadContext) -> StrategyApiResult<StrategyEvaluation> {
        if context.config().kill_switch_triggered() {
            return self.reject(
                context,
                StrategyRejectReason::KillSwitchTriggered,
                "global or execution kill switch is active",
            );
        }
        if context
            .config()
            .strategy_disabled(self.metadata.strategy_id())
        {
            return self.reject(
                context,
                StrategyRejectReason::ConfigDisabled,
                "sample spot strategy is disabled by read-only config",
            );
        }
        if context.config().venue_disabled(SAMPLE_VENUE_ID) {
            return self.reject(
                context,
                StrategyRejectReason::ConfigDisabled,
                "sample venue is disabled by read-only config",
            );
        }
        if context.snapshot().source_event_refs().is_empty() {
            return self.reject(
                context,
                StrategyRejectReason::MissingData,
                "portfolio snapshot has no input event references",
            );
        }
        if !context
            .capabilities()
            .has_market_capability(SAMPLE_VENUE_ID, &MarketCapability::ProvidesSpotMarkets)
        {
            return self.reject(
                context,
                StrategyRejectReason::VenueCapabilityMissing,
                "sample venue lacks ProvidesSpotMarkets capability",
            );
        }
        if !context
            .capabilities()
            .has_data_surface(SAMPLE_VENUE_ID, &DataSurface::RestPolling)
        {
            return self.reject(
                context,
                StrategyRejectReason::VenueCapabilityMissing,
                "sample venue lacks RESTPolling data surface",
            );
        }

        let quote = match latest_market_quote(context) {
            Ok(quote) => quote,
            Err(detail) => {
                return self.reject(context, StrategyRejectReason::MissingData, detail);
            }
        };

        let candidate = self.build_candidate(context, &quote)?;
        validate_candidate_for_context(context, self.metadata(), &candidate)?;
        let diagnostic = StrategyDiagnostic::new(
            "SAMPLE_SPOT_CANDIDATE",
            format!(
                "candidate {SAMPLE_TRANSITION_ID} emitted in {} mode using bid {}, ask {}, last {}",
                context.config().execution_mode().as_str(),
                quote.best_bid,
                quote.best_ask,
                quote.last_price
            ),
            context.time().now(),
        )?;
        Ok(StrategyEvaluation::candidate(candidate).with_diagnostic(diagnostic))
    }
}

/// 返回默认样例策略，供回放和运行时后续装配使用。
pub fn sample_spot_strategy() -> StrategyApiResult<SampleSpotStrategy> {
    SampleSpotStrategy::new()
}

/// spot-perp basis 策略实例配置。
///
/// 中文说明：算法只读取这里提供的场所、合约、账户和输出参数；策略层不关心
/// 具体交易所 API，也不持有任何可变执行能力。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotPerpBasisStrategyConfig {
    pub instance: StrategyInstanceConfig,
    pub symbol: BasisSymbolConfig,
    pub economics: BasisEconomicsConfig,
    pub output: BasisOutputConfig,
}

impl SpotPerpBasisStrategyConfig {
    /// 返回当前兼容的 Binance BTCUSDT 默认配置。
    pub fn binance_btcusdt() -> Self {
        Self {
            instance: StrategyInstanceConfig {
                strategy_id: BINANCE_BASIS_STRATEGY_ID.to_owned(),
                strategy_version: SPOT_PERP_BASIS_STRATEGY_VERSION.to_owned(),
                code_version: BINANCE_BASIS_CODE_VERSION.to_owned(),
                strategy_label: "spot-perp basis strategy".to_owned(),
                venue_family_label: "Binance".to_owned(),
            },
            symbol: BasisSymbolConfig {
                symbol: "BTCUSDT".to_owned(),
                base_asset_id: "asset:BTC".to_owned(),
                quote_asset_id: "asset:USDT".to_owned(),
                spot: BasisLegConfig {
                    venue_id: BINANCE_BASIS_SPOT_VENUE_ID.to_owned(),
                    instrument_id: BINANCE_BASIS_SPOT_INSTRUMENT_ID.to_owned(),
                    account_id: "acct:binance-basis-readonly".to_owned(),
                    leg_id: "candleg:binance-basis-buy-spot-btcusdt".to_owned(),
                    basis_role: "Spot".to_owned(),
                    basis_leg_role: "spot_buy".to_owned(),
                    venue_label: "Binance spot".to_owned(),
                    instrument_label: "spot".to_owned(),
                },
                perp: BasisLegConfig {
                    venue_id: BINANCE_BASIS_PERP_VENUE_ID.to_owned(),
                    instrument_id: BINANCE_BASIS_PERP_INSTRUMENT_ID.to_owned(),
                    account_id: "acct:binance-basis-readonly".to_owned(),
                    leg_id: "candleg:binance-basis-short-usdm-perp-btcusdt".to_owned(),
                    basis_role: "Perp".to_owned(),
                    basis_leg_role: "perp_short".to_owned(),
                    venue_label: "Binance USD-M".to_owned(),
                    instrument_label: "USD-M perp".to_owned(),
                },
            },
            economics: BasisEconomicsConfig {
                notional_usd: DEFAULT_BASIS_NOTIONAL_USD.to_owned(),
                spot_taker_fee_bps: BINANCE_BASIS_SPOT_TAKER_FEE_BPS.to_owned(),
                perp_taker_fee_bps: BINANCE_BASIS_PERP_TAKER_FEE_BPS.to_owned(),
                slippage_buffer_bps: DEFAULT_BASIS_SLIPPAGE_BUFFER_BPS,
                min_net_bps: DEFAULT_BASIS_MIN_NET_BPS,
            },
            output: BasisOutputConfig {
                transition_id: BINANCE_BASIS_TRANSITION_ID.to_owned(),
                exit_policy_ref: DEFAULT_BASIS_EXIT_POLICY_REF.to_owned(),
                assumption_id: "asm:binance-basis-public-data-readonly".to_owned(),
                premium_index_label: "premiumIndex".to_owned(),
                expected_economics_confidence: "0.72".to_owned(),
                funding_impact_confidence: "0.6".to_owned(),
                assumption_confidence: "0.72".to_owned(),
                recovery_buffer_usd: "1.00".to_owned(),
            },
        }
    }

    /// 返回 Bybit BTCUSDT 参数化策略配置。
    pub fn bybit_btcusdt() -> Self {
        Self {
            instance: StrategyInstanceConfig {
                strategy_id: BYBIT_BASIS_STRATEGY_ID.to_owned(),
                strategy_version: SPOT_PERP_BASIS_STRATEGY_VERSION.to_owned(),
                code_version: BYBIT_BASIS_CODE_VERSION.to_owned(),
                strategy_label: "bybit spot-perp basis strategy".to_owned(),
                venue_family_label: "Bybit".to_owned(),
            },
            symbol: BasisSymbolConfig {
                symbol: "BTCUSDT".to_owned(),
                base_asset_id: "asset:BTC".to_owned(),
                quote_asset_id: "asset:USDT".to_owned(),
                spot: BasisLegConfig {
                    venue_id: BYBIT_BASIS_SPOT_VENUE_ID.to_owned(),
                    instrument_id: BYBIT_BASIS_SPOT_INSTRUMENT_ID.to_owned(),
                    account_id: "acct:bybit-basis-readonly".to_owned(),
                    leg_id: "candleg:bybit-basis-buy-spot-btcusdt".to_owned(),
                    basis_role: "Spot".to_owned(),
                    basis_leg_role: "spot_buy".to_owned(),
                    venue_label: "Bybit spot".to_owned(),
                    instrument_label: "spot".to_owned(),
                },
                perp: BasisLegConfig {
                    venue_id: BYBIT_BASIS_PERP_VENUE_ID.to_owned(),
                    instrument_id: BYBIT_BASIS_PERP_INSTRUMENT_ID.to_owned(),
                    account_id: "acct:bybit-basis-readonly".to_owned(),
                    leg_id: "candleg:bybit-basis-short-linear-perp-btcusdt".to_owned(),
                    basis_role: "Perp".to_owned(),
                    basis_leg_role: "perp_short".to_owned(),
                    venue_label: "Bybit linear".to_owned(),
                    instrument_label: "linear perp".to_owned(),
                },
            },
            economics: BasisEconomicsConfig {
                notional_usd: DEFAULT_BASIS_NOTIONAL_USD.to_owned(),
                spot_taker_fee_bps: BYBIT_BASIS_SPOT_TAKER_FEE_BPS.to_owned(),
                perp_taker_fee_bps: BYBIT_BASIS_PERP_TAKER_FEE_BPS.to_owned(),
                slippage_buffer_bps: DEFAULT_BASIS_SLIPPAGE_BUFFER_BPS,
                min_net_bps: DEFAULT_BASIS_MIN_NET_BPS,
            },
            output: BasisOutputConfig {
                transition_id: BYBIT_BASIS_TRANSITION_ID.to_owned(),
                exit_policy_ref: DEFAULT_BASIS_EXIT_POLICY_REF.to_owned(),
                assumption_id: "asm:bybit-basis-public-data-readonly".to_owned(),
                premium_index_label: "premiumIndex".to_owned(),
                expected_economics_confidence: "0.72".to_owned(),
                funding_impact_confidence: "0.6".to_owned(),
                assumption_confidence: "0.72".to_owned(),
                recovery_buffer_usd: "1.00".to_owned(),
            },
        }
    }

    /// 返回 OKX BTC-USDT 参数化策略配置。
    pub fn okx_btc_usdt() -> Self {
        Self {
            instance: StrategyInstanceConfig {
                strategy_id: OKX_BASIS_STRATEGY_ID.to_owned(),
                strategy_version: SPOT_PERP_BASIS_STRATEGY_VERSION.to_owned(),
                code_version: OKX_BASIS_CODE_VERSION.to_owned(),
                strategy_label: "okx spot-swap basis strategy".to_owned(),
                venue_family_label: "OKX".to_owned(),
            },
            symbol: BasisSymbolConfig {
                symbol: "BTC-USDT".to_owned(),
                base_asset_id: "asset:BTC".to_owned(),
                quote_asset_id: "asset:USDT".to_owned(),
                spot: BasisLegConfig {
                    venue_id: OKX_BASIS_SPOT_VENUE_ID.to_owned(),
                    instrument_id: OKX_BASIS_SPOT_INSTRUMENT_ID.to_owned(),
                    account_id: "acct:okx-basis-readonly".to_owned(),
                    leg_id: "candleg:okx-basis-buy-spot-btc-usdt".to_owned(),
                    basis_role: "Spot".to_owned(),
                    basis_leg_role: "spot_buy".to_owned(),
                    venue_label: "OKX spot".to_owned(),
                    instrument_label: "spot".to_owned(),
                },
                perp: BasisLegConfig {
                    venue_id: OKX_BASIS_PERP_VENUE_ID.to_owned(),
                    instrument_id: OKX_BASIS_PERP_INSTRUMENT_ID.to_owned(),
                    account_id: "acct:okx-basis-readonly".to_owned(),
                    leg_id: "candleg:okx-basis-short-swap-btc-usdt".to_owned(),
                    basis_role: "Perp".to_owned(),
                    basis_leg_role: "perp_short".to_owned(),
                    venue_label: "OKX swap".to_owned(),
                    instrument_label: "USDT swap".to_owned(),
                },
            },
            economics: BasisEconomicsConfig {
                notional_usd: DEFAULT_BASIS_NOTIONAL_USD.to_owned(),
                spot_taker_fee_bps: OKX_BASIS_SPOT_TAKER_FEE_BPS.to_owned(),
                perp_taker_fee_bps: OKX_BASIS_PERP_TAKER_FEE_BPS.to_owned(),
                slippage_buffer_bps: DEFAULT_BASIS_SLIPPAGE_BUFFER_BPS,
                min_net_bps: DEFAULT_BASIS_MIN_NET_BPS,
            },
            output: BasisOutputConfig {
                transition_id: OKX_BASIS_TRANSITION_ID.to_owned(),
                exit_policy_ref: DEFAULT_BASIS_EXIT_POLICY_REF.to_owned(),
                assumption_id: "asm:okx-basis-public-data-readonly".to_owned(),
                premium_index_label: "fundingRate".to_owned(),
                expected_economics_confidence: "0.72".to_owned(),
                funding_impact_confidence: "0.6".to_owned(),
                assumption_confidence: "0.72".to_owned(),
                recovery_buffer_usd: "1.00".to_owned(),
            },
        }
    }
}

/// 策略身份和实例文案。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategyInstanceConfig {
    pub strategy_id: String,
    pub strategy_version: String,
    pub code_version: String,
    pub strategy_label: String,
    pub venue_family_label: String,
}

/// 单个 basis 交易对配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BasisSymbolConfig {
    pub symbol: String,
    pub base_asset_id: String,
    pub quote_asset_id: String,
    pub spot: BasisLegConfig,
    pub perp: BasisLegConfig,
}

/// basis 单腿配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BasisLegConfig {
    pub venue_id: String,
    pub instrument_id: String,
    pub account_id: String,
    pub leg_id: String,
    pub basis_role: String,
    pub basis_leg_role: String,
    pub venue_label: String,
    pub instrument_label: String,
}

/// basis 静态经济参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BasisEconomicsConfig {
    pub notional_usd: String,
    pub spot_taker_fee_bps: String,
    pub perp_taker_fee_bps: String,
    pub slippage_buffer_bps: i128,
    pub min_net_bps: i128,
}

/// candidate 输出参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BasisOutputConfig {
    pub transition_id: String,
    pub exit_policy_ref: String,
    pub assumption_id: String,
    pub premium_index_label: String,
    pub expected_economics_confidence: String,
    pub funding_impact_confidence: String,
    pub assumption_confidence: String,
    pub recovery_buffer_usd: String,
}

/// 跨交易所资金费率套利策略配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingArbStrategyConfig {
    pub instance: StrategyInstanceConfig,
    pub symbol: CrossExchangeFundingSymbolConfig,
    pub venues: CrossExchangeFundingVenuesConfig,
    pub economics: CrossExchangeFundingEconomicsConfig,
    pub output: CrossExchangeFundingOutputConfig,
}

impl CrossExchangeFundingArbStrategyConfig {
    /// 返回 Binance/Bybit BTCUSDT 默认只读配置。
    pub fn binance_bybit_btcusdt() -> Self {
        Self {
            instance: StrategyInstanceConfig {
                strategy_id: CROSS_EXCHANGE_FUNDING_ARB_STRATEGY_ID.to_owned(),
                strategy_version: CROSS_EXCHANGE_FUNDING_ARB_STRATEGY_VERSION.to_owned(),
                code_version: CROSS_EXCHANGE_FUNDING_ARB_CODE_VERSION.to_owned(),
                strategy_label: "cross-exchange funding arbitrage strategy".to_owned(),
                venue_family_label: "Binance-Bybit".to_owned(),
            },
            symbol: CrossExchangeFundingSymbolConfig {
                symbol: "BTCUSDT".to_owned(),
                base_asset_id: "asset:BTC".to_owned(),
                settlement_asset_id: "asset:USDT".to_owned(),
            },
            venues: CrossExchangeFundingVenuesConfig {
                venue_a: CrossExchangeFundingLegConfig {
                    venue_id: BINANCE_BASIS_PERP_VENUE_ID.to_owned(),
                    instrument_id: BINANCE_BASIS_PERP_INSTRUMENT_ID.to_owned(),
                    account_id: "acct:binance-funding-arb-readonly".to_owned(),
                    leg_id: "candleg:funding-arb-binance-usdm-btcusdt".to_owned(),
                    venue_label: "Binance USD-M".to_owned(),
                    instrument_label: "USD-M perp".to_owned(),
                },
                venue_b: CrossExchangeFundingLegConfig {
                    venue_id: BYBIT_BASIS_PERP_VENUE_ID.to_owned(),
                    instrument_id: BYBIT_BASIS_PERP_INSTRUMENT_ID.to_owned(),
                    account_id: "acct:bybit-funding-arb-readonly".to_owned(),
                    leg_id: "candleg:funding-arb-bybit-linear-btcusdt".to_owned(),
                    venue_label: "Bybit linear".to_owned(),
                    instrument_label: "linear perp".to_owned(),
                },
            },
            economics: CrossExchangeFundingEconomicsConfig {
                notional_usd: "100.00".to_owned(),
                venue_a_taker_fee_bps: BINANCE_BASIS_PERP_TAKER_FEE_BPS.to_owned(),
                venue_b_taker_fee_bps: BYBIT_BASIS_PERP_TAKER_FEE_BPS.to_owned(),
                slippage_buffer_bps: DEFAULT_BASIS_SLIPPAGE_BUFFER_BPS,
                max_entry_price_divergence_bps: 20,
                min_net_funding_bps: 5,
            },
            output: CrossExchangeFundingOutputConfig {
                transition_id: CROSS_EXCHANGE_FUNDING_ARB_TRANSITION_ID.to_owned(),
                assumption_id: "asm:cross-exchange-funding-public-data-readonly".to_owned(),
                expected_economics_confidence: "0.70".to_owned(),
                funding_impact_confidence: "0.70".to_owned(),
                liquidity_impact_confidence: "0.90".to_owned(),
                margin_impact_confidence: "0.90".to_owned(),
                assumption_confidence: "0.70".to_owned(),
                recovery_buffer_usd: "2.00".to_owned(),
            },
        }
    }
}

/// 跨所 funding arb 标的配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingSymbolConfig {
    pub symbol: String,
    pub base_asset_id: String,
    pub settlement_asset_id: String,
}

/// 跨所 funding arb 双场所配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingVenuesConfig {
    pub venue_a: CrossExchangeFundingLegConfig,
    pub venue_b: CrossExchangeFundingLegConfig,
}

/// 跨所 funding arb 单腿配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingLegConfig {
    pub venue_id: String,
    pub instrument_id: String,
    pub account_id: String,
    pub leg_id: String,
    pub venue_label: String,
    pub instrument_label: String,
}

/// 跨所 funding arb 静态经济参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingEconomicsConfig {
    pub notional_usd: String,
    pub venue_a_taker_fee_bps: String,
    pub venue_b_taker_fee_bps: String,
    pub slippage_buffer_bps: i128,
    pub max_entry_price_divergence_bps: i128,
    pub min_net_funding_bps: i128,
}

/// 跨所 funding arb candidate 输出参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingOutputConfig {
    pub transition_id: String,
    pub assumption_id: String,
    pub expected_economics_confidence: String,
    pub funding_impact_confidence: String,
    pub liquidity_impact_confidence: String,
    pub margin_impact_confidence: String,
    pub assumption_confidence: String,
    pub recovery_buffer_usd: String,
}

fn validate_spot_perp_basis_config(config: &SpotPerpBasisStrategyConfig) -> StrategyApiResult<()> {
    ensure_non_empty_basis_config("symbol.symbol", &config.symbol.symbol)?;
    let notional = parse_non_negative_basis_config_decimal(
        "economics.notional_usd",
        &config.economics.notional_usd,
    )?;
    if notional.is_zero() {
        return Err(invalid_basis_config(
            "economics.notional_usd",
            "notional must be greater than zero",
        ));
    }
    parse_non_negative_basis_config_decimal(
        "economics.spot_taker_fee_bps",
        &config.economics.spot_taker_fee_bps,
    )?;
    parse_non_negative_basis_config_decimal(
        "economics.perp_taker_fee_bps",
        &config.economics.perp_taker_fee_bps,
    )?;
    ensure_non_negative_basis_bps(
        "economics.slippage_buffer_bps",
        config.economics.slippage_buffer_bps,
    )?;
    ensure_non_negative_basis_bps("economics.min_net_bps", config.economics.min_net_bps)?;
    parse_non_negative_basis_config_decimal(
        "output.recovery_buffer_usd",
        &config.output.recovery_buffer_usd,
    )?;
    ensure_non_empty_basis_config("output.exit_policy_ref", &config.output.exit_policy_ref)?;
    validate_basis_confidence(
        "output.expected_economics_confidence",
        &config.output.expected_economics_confidence,
    )?;
    validate_basis_confidence(
        "output.funding_impact_confidence",
        &config.output.funding_impact_confidence,
    )?;
    validate_basis_confidence(
        "output.assumption_confidence",
        &config.output.assumption_confidence,
    )?;
    Ok(())
}

fn validate_cross_exchange_funding_arb_config(
    config: &CrossExchangeFundingArbStrategyConfig,
) -> StrategyApiResult<()> {
    ensure_non_empty_basis_config("symbol.symbol", &config.symbol.symbol)?;
    ensure_non_empty_basis_config("symbol.base_asset_id", &config.symbol.base_asset_id)?;
    ensure_non_empty_basis_config(
        "symbol.settlement_asset_id",
        &config.symbol.settlement_asset_id,
    )?;
    ensure_non_empty_funding_leg("venues.venue_a", &config.venues.venue_a)?;
    ensure_non_empty_funding_leg("venues.venue_b", &config.venues.venue_b)?;
    if config.venues.venue_a.venue_id == config.venues.venue_b.venue_id {
        return Err(invalid_basis_config(
            "venues",
            "cross-exchange funding arbitrage requires two distinct venues",
        ));
    }

    let notional = parse_non_negative_basis_config_decimal(
        "economics.notional_usd",
        &config.economics.notional_usd,
    )?;
    if notional.is_zero() {
        return Err(invalid_basis_config(
            "economics.notional_usd",
            "notional must be greater than zero",
        ));
    }
    parse_non_negative_basis_config_decimal(
        "economics.venue_a_taker_fee_bps",
        &config.economics.venue_a_taker_fee_bps,
    )?;
    parse_non_negative_basis_config_decimal(
        "economics.venue_b_taker_fee_bps",
        &config.economics.venue_b_taker_fee_bps,
    )?;
    ensure_non_negative_basis_bps(
        "economics.slippage_buffer_bps",
        config.economics.slippage_buffer_bps,
    )?;
    ensure_non_negative_basis_bps(
        "economics.max_entry_price_divergence_bps",
        config.economics.max_entry_price_divergence_bps,
    )?;
    ensure_non_negative_basis_bps(
        "economics.min_net_funding_bps",
        config.economics.min_net_funding_bps,
    )?;
    parse_non_negative_basis_config_decimal(
        "output.recovery_buffer_usd",
        &config.output.recovery_buffer_usd,
    )?;
    validate_basis_confidence(
        "output.expected_economics_confidence",
        &config.output.expected_economics_confidence,
    )?;
    validate_basis_confidence(
        "output.funding_impact_confidence",
        &config.output.funding_impact_confidence,
    )?;
    validate_basis_confidence(
        "output.liquidity_impact_confidence",
        &config.output.liquidity_impact_confidence,
    )?;
    validate_basis_confidence(
        "output.margin_impact_confidence",
        &config.output.margin_impact_confidence,
    )?;
    validate_basis_confidence(
        "output.assumption_confidence",
        &config.output.assumption_confidence,
    )?;
    Ok(())
}

fn ensure_non_empty_funding_leg(
    prefix: &'static str,
    leg: &CrossExchangeFundingLegConfig,
) -> StrategyApiResult<()> {
    ensure_non_empty_basis_config(prefix, &leg.venue_id)?;
    ensure_non_empty_basis_config(prefix, &leg.instrument_id)?;
    ensure_non_empty_basis_config(prefix, &leg.account_id)?;
    ensure_non_empty_basis_config(prefix, &leg.leg_id)?;
    ensure_non_empty_basis_config(prefix, &leg.venue_label)?;
    ensure_non_empty_basis_config(prefix, &leg.instrument_label)
}

fn ensure_non_empty_basis_config(field: &'static str, value: &str) -> StrategyApiResult<()> {
    if value.trim().is_empty() {
        return Err(invalid_basis_config(field, "value cannot be empty"));
    }
    Ok(())
}

fn ensure_non_negative_basis_bps(field: &'static str, value: i128) -> StrategyApiResult<()> {
    if value < 0 {
        return Err(invalid_basis_config(field, "bps value cannot be negative"));
    }
    Ok(())
}

fn parse_non_negative_basis_config_decimal(
    field: &'static str,
    value: &str,
) -> StrategyApiResult<FixedDecimal> {
    FixedDecimal::parse_non_negative(field, value)
        .map_err(|message| invalid_basis_config(field, message))
}

fn validate_basis_confidence(field: &'static str, value: &str) -> StrategyApiResult<()> {
    let confidence = parse_non_negative_basis_config_decimal(field, value)?;
    if confidence.raw > FIXED_SCALE {
        return Err(invalid_basis_config(
            field,
            "confidence must be between 0 and 1",
        ));
    }
    Ok(())
}

fn invalid_basis_config(field: &'static str, message: impl Into<String>) -> StrategyApiError {
    StrategyApiError::InvalidInput {
        field,
        message: message.into(),
    }
}

/// 参数化现货-永续 basis 只读策略。
///
/// 中文说明：策略只读取已经标准化的公共行情事件，计算买现货、做空永续的
/// 正向 basis 是否在扣除静态手续费和滑点缓冲后仍为正。它只输出候选组合转换或
/// 拒绝原因，不下单、不签名、不访问账户。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotPerpBasisStrategy {
    metadata: StrategyMetadata,
    config: SpotPerpBasisStrategyConfig,
}

impl SpotPerpBasisStrategy {
    /// 创建 Binance spot-perp basis 只读策略。
    pub fn new() -> StrategyApiResult<Self> {
        Self::with_config(SpotPerpBasisStrategyConfig::binance_btcusdt())
    }

    /// 使用显式配置创建 spot-perp basis 只读策略。
    pub fn with_config(config: SpotPerpBasisStrategyConfig) -> StrategyApiResult<Self> {
        validate_spot_perp_basis_config(&config)?;
        let metadata = StrategyMetadata::new(
            &config.instance.strategy_id,
            &config.instance.strategy_version,
            &config.instance.code_version,
        )?;
        Ok(Self { metadata, config })
    }

    /// 返回当前策略实例配置。
    pub fn config(&self) -> &SpotPerpBasisStrategyConfig {
        &self.config
    }

    fn reject(
        &self,
        context: &dyn StrategyReadContext,
        reason: StrategyRejectReason,
        detail: impl Into<String>,
    ) -> StrategyApiResult<StrategyEvaluation> {
        let detail = detail.into();
        let rejection = StrategyRejection::new(
            &self.metadata,
            context.time().now(),
            reason,
            Some(detail.clone()),
            source_event_refs(context),
            context.snapshot().portfolio_state_id(),
        )?;
        let diagnostic =
            StrategyDiagnostic::new("SPOT_PERP_BASIS_REJECTED", detail, context.time().now())?;
        Ok(StrategyEvaluation::rejected(rejection).with_diagnostic(diagnostic))
    }

    fn ensure_capabilities(
        &self,
        context: &dyn StrategyReadContext,
    ) -> StrategyApiResult<Option<StrategyEvaluation>> {
        let spot = &self.config.symbol.spot;
        let perp = &self.config.symbol.perp;
        let required_market = [
            (
                spot,
                MarketCapability::ProvidesSpotMarkets,
                format!(
                    "{} venue lacks ProvidesSpotMarkets capability",
                    spot.venue_label
                ),
            ),
            (
                spot,
                MarketCapability::ProvidesOrderBookMarkets,
                format!(
                    "{} venue lacks ProvidesOrderBookMarkets capability",
                    spot.venue_label
                ),
            ),
            (
                perp,
                MarketCapability::ProvidesPerpetuals,
                format!(
                    "{} venue lacks ProvidesPerpetuals capability",
                    perp.venue_label
                ),
            ),
            (
                perp,
                MarketCapability::ProvidesOrderBookMarkets,
                format!(
                    "{} venue lacks ProvidesOrderBookMarkets capability",
                    perp.venue_label
                ),
            ),
            (
                perp,
                MarketCapability::ProvidesFundingRates,
                format!(
                    "{} venue lacks ProvidesFundingRates capability",
                    perp.venue_label
                ),
            ),
        ];
        for (leg, capability, detail) in required_market {
            if !context
                .capabilities()
                .has_market_capability(&leg.venue_id, &capability)
            {
                return Ok(Some(self.reject(
                    context,
                    StrategyRejectReason::VenueCapabilityMissing,
                    detail,
                )?));
            }
        }

        for leg in [spot, perp] {
            if !context
                .capabilities()
                .has_data_surface(&leg.venue_id, &DataSurface::RestPolling)
            {
                return Ok(Some(self.reject(
                    context,
                    StrategyRejectReason::VenueCapabilityMissing,
                    format!("{} lacks RESTPolling data surface", leg.venue_id),
                )?));
            }
        }

        Ok(None)
    }

    fn build_candidate(
        &self,
        context: &dyn StrategyReadContext,
        opportunity: &BasisOpportunity,
    ) -> StrategyApiResult<CandidatePortfolioTransition> {
        let config = &self.config;
        let spot_leg = &config.symbol.spot;
        let perp_leg = &config.symbol.perp;
        let input_event_refs = source_event_refs(context);
        let input_event_refs_json = json_string_array(&input_event_refs);
        let config_version = context.config().config_version();
        let quantity = &opportunity.signal.quantity;
        let expected_profit_usd = &opportunity.signal.expected_profit_usd;
        let expected_profit_bps = opportunity.signal.expected_profit_bps.clone();
        let funding_impact_usd = &opportunity.signal.funding_impact_usd;
        let fee_estimate_usd = &opportunity.signal.fee_estimate_usd;
        let slippage_estimate_usd = &opportunity.signal.slippage_estimate_usd;
        let gross_bps = opportunity.signal.gross_bps.clone();
        let net_bps = opportunity.signal.net_bps.clone();
        let funding_bps = opportunity.signal.funding_bps.clone();
        let total_cost_bps = opportunity.signal.total_cost_bps.clone();
        let assumption = format!(
            "Read-only {} public data signal: buy {} at {}, short {} at {}, gross_basis_bps={}, total_cost_bps={}, net_basis_bps={}. Static fee/slippage assumptions must be replaced with account-specific checks before any order path.",
            config.instance.venue_family_label,
            spot_leg.instrument_label,
            opportunity.spot.best_ask.format_trimmed(),
            perp_leg.instrument_label,
            opportunity.perp.best_bid.format_trimmed(),
            gross_bps,
            total_cost_bps,
            net_bps
        );
        let funding_summary = format!(
            "Public {} lastFundingRate={}, mark_price={}, index_price={}, nextFundingTimeMs={}.",
            config.output.premium_index_label,
            opportunity.premium.last_funding_rate,
            opportunity.premium.mark_price.format_trimmed(),
            opportunity.premium.index_price.format_trimmed(),
            opportunity.premium.next_funding_time_ms
        );
        let liquidity_summary = format!(
            "Depth/VWAP liquidity checked before candidate emission: liquidity_model={}, spot_ask_depth_usd={}, perp_bid_depth_usd={}, spot_ask_vwap={}, perp_bid_vwap={}, required_notional_usd={}.",
            opportunity.signal.liquidity_model,
            opportunity
                .signal
                .spot_ask_depth_usd
                .as_deref()
                .unwrap_or("missing"),
            opportunity
                .signal
                .perp_bid_depth_usd
                .as_deref()
                .unwrap_or("missing"),
            opportunity
                .signal
                .spot_ask_vwap
                .as_deref()
                .unwrap_or("missing"),
            opportunity
                .signal
                .perp_bid_vwap
                .as_deref()
                .unwrap_or("missing"),
            opportunity.signal.liquidity_required_usd
        );

        let candidate_json = format!(
            r#"{{
  "schema_version": "1.0.0",
  "transition_id": {},
  "strategy_id": {},
  "strategy_version": {},
  "code_version": {},
  "config_version": {},
  "created_at": {},
  "input_event_refs": {},
  "current_portfolio_state_ref": {},
  "holding_period": {{
    "kind": "UntilBasisConvergence",
    "exit_policy_ref": {}
  }},
  "legs": [
    {{
      "leg_id": {},
      "leg_type": "Trade",
      "venue_id": {},
      "instrument_id": {},
      "account_id": {},
      "side": "Buy",
      "asset_flows": [
        {{
          "asset_id": {},
          "direction": "Out",
          "amount": {},
          "account_id": {}
        }},
        {{
          "asset_id": {},
          "direction": "In",
          "amount": {},
          "account_id": {}
        }}
      ],
      "constraints": {{
        "basis_leg_role": {},
        "gross_basis_bps": {},
        "max_slippage_bps": {},
        "net_basis_bps": {},
        "notional_usdt": {},
        "reference_best_ask": {},
        "reference_best_bid": {},
        "reference_bid_size": {},
        "reference_ask_size": {},
        "reference_market_event_id": {}
      }},
      "failure_modes": [
        "PartialFill",
        "VenueOutage"
      ]
    }},
    {{
      "leg_id": {},
      "leg_type": "Trade",
      "venue_id": {},
      "instrument_id": {},
      "account_id": {},
      "side": "Short",
      "asset_flows": [],
      "constraints": {{
        "basis_leg_role": {},
        "funding_bps": {},
        "gross_basis_bps": {},
        "last_funding_rate": {},
        "max_slippage_bps": {},
        "net_basis_bps": {},
        "notional_usdt": {},
        "reference_best_ask": {},
        "reference_best_bid": {},
        "reference_bid_size": {},
        "reference_ask_size": {},
        "reference_market_event_id": {},
        "reference_premium_event_id": {}
      }},
      "failure_modes": [
        "PartialFill",
        "VenueOutage"
      ]
    }}
  ],
  "expected_post_state_delta": {{
    "asset_flows": [
      {{
        "asset_id": {},
        "direction": "Out",
        "amount": {},
        "account_id": {}
      }},
      {{
        "asset_id": {},
        "direction": "In",
        "amount": {},
        "account_id": {}
      }}
    ],
    "position_deltas": [
      {{
        "instrument_id": {},
        "account_id": {},
        "quantity_delta": {}
      }},
      {{
        "instrument_id": {},
        "account_id": {},
        "quantity_delta": {}
      }}
    ]
  }},
  "expected_economics": {{
    "expected_profit_usd": {},
    "expected_profit_bps": {},
    "fee_estimate_usd": {},
    "slippage_estimate_usd": {},
    "confidence": {}
  }},
  "required_capital": {{
    "asset_requirements": [
      {{
        "asset_id": {},
        "direction": "Out",
        "amount": {},
        "account_id": {}
      }}
    ],
    "recovery_buffer_usd": {}
  }},
  "funding_impact": {{
    "summary": {},
    "impact_usd": {},
    "confidence": {}
  }},
  "liquidity_impact": {{
    "summary": {},
    "impact_usd": "0",
    "confidence": 0.90
  }},
  "failure_modes": [
    "PartialFill",
    "VenueOutage"
  ],
  "risk_flags": [
  ],
  "assumptions": [
    {{
      "assumption_id": {},
      "statement": {},
      "confidence": {},
      "source_event_refs": {}
    }}
  ]
}}"#,
            json_string(&config.output.transition_id),
            json_string(self.metadata.strategy_id()),
            json_string(self.metadata.strategy_version()),
            json_string(self.metadata.code_version()),
            json_string(config_version),
            json_string(&context.time().now_rfc3339_z()),
            input_event_refs_json,
            json_string(context.snapshot().portfolio_state_id()),
            json_string(&config.output.exit_policy_ref),
            json_string(&spot_leg.leg_id),
            json_string(&spot_leg.venue_id),
            json_string(&spot_leg.instrument_id),
            json_string(&spot_leg.account_id),
            json_string(&config.symbol.quote_asset_id),
            json_string(&config.economics.notional_usd),
            json_string(&spot_leg.account_id),
            json_string(&config.symbol.base_asset_id),
            json_string(quantity),
            json_string(&spot_leg.account_id),
            json_string(&spot_leg.basis_leg_role),
            json_string(&gross_bps),
            json_string(&config.economics.slippage_buffer_bps.to_string()),
            json_string(&net_bps),
            json_string(&config.economics.notional_usd),
            json_string(&opportunity.spot.best_ask.format_trimmed()),
            json_string(&opportunity.spot.best_bid.format_trimmed()),
            json_string(&opportunity.spot.bid_size.format_trimmed()),
            json_string(&opportunity.spot.ask_size.format_trimmed()),
            json_string(&opportunity.spot.event_id),
            json_string(&perp_leg.leg_id),
            json_string(&perp_leg.venue_id),
            json_string(&perp_leg.instrument_id),
            json_string(&perp_leg.account_id),
            json_string(&perp_leg.basis_leg_role),
            json_string(&funding_bps),
            json_string(&gross_bps),
            json_string(&opportunity.premium.last_funding_rate),
            json_string(&config.economics.slippage_buffer_bps.to_string()),
            json_string(&net_bps),
            json_string(&config.economics.notional_usd),
            json_string(&opportunity.perp.best_ask.format_trimmed()),
            json_string(&opportunity.perp.best_bid.format_trimmed()),
            json_string(&opportunity.perp.bid_size.format_trimmed()),
            json_string(&opportunity.perp.ask_size.format_trimmed()),
            json_string(&opportunity.perp.event_id),
            json_string(&opportunity.premium.event_id),
            json_string(&config.symbol.quote_asset_id),
            json_string(&config.economics.notional_usd),
            json_string(&spot_leg.account_id),
            json_string(&config.symbol.base_asset_id),
            json_string(quantity),
            json_string(&spot_leg.account_id),
            json_string(&spot_leg.instrument_id),
            json_string(&spot_leg.account_id),
            json_string(quantity),
            json_string(&perp_leg.instrument_id),
            json_string(&perp_leg.account_id),
            json_string(&format!("-{quantity}")),
            json_string(expected_profit_usd),
            json_string(&expected_profit_bps),
            json_string(fee_estimate_usd),
            json_string(slippage_estimate_usd),
            config.output.expected_economics_confidence.as_str(),
            json_string(&config.symbol.quote_asset_id),
            json_string(&config.economics.notional_usd),
            json_string(&spot_leg.account_id),
            json_string(&config.output.recovery_buffer_usd),
            json_string(&funding_summary),
            json_string(funding_impact_usd),
            config.output.funding_impact_confidence.as_str(),
            json_string(&liquidity_summary),
            json_string(&config.output.assumption_id),
            json_string(&assumption),
            config.output.assumption_confidence.as_str(),
            json_string_array(&input_event_refs),
        );

        candidate_from_json_strict(&candidate_json)
    }
}

impl Default for SpotPerpBasisStrategy {
    fn default() -> Self {
        Self::new().expect("spot-perp basis strategy metadata is static and valid")
    }
}

impl Strategy for SpotPerpBasisStrategy {
    fn metadata(&self) -> &StrategyMetadata {
        &self.metadata
    }

    fn evaluate(&self, context: &dyn StrategyReadContext) -> StrategyApiResult<StrategyEvaluation> {
        if context.config().kill_switch_triggered() {
            return self.reject(
                context,
                StrategyRejectReason::KillSwitchTriggered,
                "global or execution kill switch is active",
            );
        }
        if context
            .config()
            .strategy_disabled(self.metadata.strategy_id())
        {
            return self.reject(
                context,
                StrategyRejectReason::ConfigDisabled,
                format!(
                    "{} is disabled by read-only config",
                    self.config.instance.strategy_label
                ),
            );
        }
        for leg in [&self.config.symbol.spot, &self.config.symbol.perp] {
            if context.config().venue_disabled(&leg.venue_id) {
                return self.reject(
                    context,
                    StrategyRejectReason::ConfigDisabled,
                    format!("{} is disabled by read-only config", leg.venue_id),
                );
            }
        }
        if context.snapshot().source_event_refs().is_empty() {
            return self.reject(
                context,
                StrategyRejectReason::MissingData,
                "portfolio snapshot has no input event references",
            );
        }
        if let Some(rejection) = self.ensure_capabilities(context)? {
            return Ok(rejection);
        }

        let spot = match latest_basis_book_ticker(
            context,
            &self.config.symbol.spot.venue_id,
            &self.config.symbol.spot.instrument_id,
            &self.config.symbol.spot.basis_role,
            &self.config.instance.venue_family_label,
        ) {
            Ok(quote) => quote,
            Err(detail) => {
                return self.reject(context, StrategyRejectReason::MissingData, detail);
            }
        };
        let perp = match latest_basis_book_ticker(
            context,
            &self.config.symbol.perp.venue_id,
            &self.config.symbol.perp.instrument_id,
            &self.config.symbol.perp.basis_role,
            &self.config.instance.venue_family_label,
        ) {
            Ok(quote) => quote,
            Err(detail) => {
                return self.reject(context, StrategyRejectReason::MissingData, detail);
            }
        };
        let premium = match latest_basis_premium_index(
            context,
            &self.config.symbol.perp.venue_id,
            &self.config.symbol.perp.instrument_id,
            &self.config.instance.venue_family_label,
        ) {
            Ok(premium) => premium,
            Err(detail) => {
                return self.reject(context, StrategyRejectReason::MissingData, detail);
            }
        };
        if spot.is_stale || perp.is_stale || premium.is_stale {
            return self.reject(
                context,
                StrategyRejectReason::DataStale,
                format!(
                    "one or more required {} public market events are stale",
                    self.config.instance.venue_family_label
                ),
            );
        }

        let signal = match evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
            symbol: self.config.symbol.symbol.clone(),
            spot_best_bid: spot.best_bid.format_trimmed(),
            spot_best_ask: spot.best_ask.format_trimmed(),
            spot_ask_size: Some(spot.ask_size.format_trimmed()),
            spot_ask_depth: spot.ask_depth.clone(),
            perp_best_bid: perp.best_bid.format_trimmed(),
            perp_best_ask: perp.best_ask.format_trimmed(),
            perp_bid_size: Some(perp.bid_size.format_trimmed()),
            perp_bid_depth: perp.bid_depth.clone(),
            last_funding_rate: premium.last_funding_rate.clone(),
            notional_usd: self.config.economics.notional_usd.clone(),
            spot_taker_fee_bps: self.config.economics.spot_taker_fee_bps.clone(),
            perp_taker_fee_bps: self.config.economics.perp_taker_fee_bps.clone(),
            slippage_buffer_bps: self.config.economics.slippage_buffer_bps,
            min_net_bps: self.config.economics.min_net_bps,
        }) {
            Ok(signal) => signal,
            Err(detail) => return self.reject(context, StrategyRejectReason::UnknownState, detail),
        };
        if !signal.is_candidate {
            return self.reject(
                context,
                StrategyRejectReason::NoCandidate,
                signal
                    .reason
                    .clone()
                    .unwrap_or_else(|| "basis signal did not pass threshold".to_owned()),
            );
        }

        let opportunity = BasisOpportunity {
            spot,
            perp,
            premium,
            signal,
        };
        let candidate = self.build_candidate(context, &opportunity)?;
        validate_candidate_for_context(context, self.metadata(), &candidate)?;
        let diagnostic = StrategyDiagnostic::new(
            "SPOT_PERP_BASIS_CANDIDATE",
            format!(
                "candidate {} emitted using {} spot ask {}, perp bid {}, gross_bps {}, net_bps {}",
                self.config.output.transition_id,
                self.config.instance.venue_family_label,
                opportunity.spot.best_ask.format_trimmed(),
                opportunity.perp.best_bid.format_trimmed(),
                opportunity.signal.gross_bps,
                opportunity.signal.net_bps
            ),
            context.time().now(),
        )?;
        Ok(StrategyEvaluation::candidate(candidate).with_diagnostic(diagnostic))
    }
}

/// 跨交易所资金费率套利只读策略。
///
/// 中文说明：策略只读取两个 perp/swap 市场的公开 top-of-book 与 funding 数据；
/// 缺能力、缺 funding、缺深度或数据 stale 时失败关闭。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingArbStrategy {
    metadata: StrategyMetadata,
    config: CrossExchangeFundingArbStrategyConfig,
}

impl CrossExchangeFundingArbStrategy {
    pub fn new() -> StrategyApiResult<Self> {
        Self::with_config(CrossExchangeFundingArbStrategyConfig::binance_bybit_btcusdt())
    }

    pub fn with_config(config: CrossExchangeFundingArbStrategyConfig) -> StrategyApiResult<Self> {
        validate_cross_exchange_funding_arb_config(&config)?;
        Ok(Self {
            metadata: StrategyMetadata::new(
                &config.instance.strategy_id,
                &config.instance.strategy_version,
                &config.instance.code_version,
            )?,
            config,
        })
    }

    fn reject(
        &self,
        context: &dyn StrategyReadContext,
        reason: StrategyRejectReason,
        detail: impl Into<String>,
    ) -> StrategyApiResult<StrategyEvaluation> {
        let detail = detail.into();
        let rejection = StrategyRejection::new(
            &self.metadata,
            context.time().now(),
            reason,
            Some(detail.clone()),
            source_event_refs(context),
            context.snapshot().portfolio_state_id(),
        )?;
        let diagnostic = StrategyDiagnostic::new(
            "CROSS_EXCHANGE_FUNDING_ARB_REJECTED",
            detail,
            context.time().now(),
        )?;
        Ok(StrategyEvaluation::rejected(rejection).with_diagnostic(diagnostic))
    }

    fn ensure_capabilities(
        &self,
        context: &dyn StrategyReadContext,
    ) -> StrategyApiResult<Option<StrategyEvaluation>> {
        for leg in [&self.config.venues.venue_a, &self.config.venues.venue_b] {
            for (capability, detail) in [
                (
                    MarketCapability::ProvidesPerpetuals,
                    format!("{} lacks ProvidesPerpetuals capability", leg.venue_label),
                ),
                (
                    MarketCapability::ProvidesOrderBookMarkets,
                    format!(
                        "{} lacks ProvidesOrderBookMarkets capability",
                        leg.venue_label
                    ),
                ),
                (
                    MarketCapability::ProvidesFundingRates,
                    format!("{} lacks ProvidesFundingRates capability", leg.venue_label),
                ),
            ] {
                if !context
                    .capabilities()
                    .has_market_capability(&leg.venue_id, &capability)
                {
                    return Ok(Some(self.reject(
                        context,
                        StrategyRejectReason::VenueCapabilityMissing,
                        detail,
                    )?));
                }
            }
            if !context
                .capabilities()
                .has_data_surface(&leg.venue_id, &DataSurface::RestPolling)
            {
                return Ok(Some(self.reject(
                    context,
                    StrategyRejectReason::VenueCapabilityMissing,
                    format!("{} lacks RESTPolling data surface", leg.venue_id),
                )?));
            }
        }
        Ok(None)
    }

    fn build_candidate(
        &self,
        context: &dyn StrategyReadContext,
        opportunity: &CrossExchangeFundingOpportunity<'_>,
    ) -> StrategyApiResult<CandidatePortfolioTransition> {
        let config = &self.config;
        let input_event_refs = source_event_refs(context);
        let input_event_refs_json = json_string_array(&input_event_refs);
        let created_at = context.time().now_rfc3339_z();
        let expected_profit_bps = opportunity.signal.net_funding_bps.clone();
        let gross_spread_bps = opportunity.signal.gross_funding_spread_bps.clone();
        let entry_price_divergence_bps = opportunity.signal.entry_price_divergence_bps.to_string();
        let entry_price_edge_bps = opportunity.signal.entry_price_edge_bps.clone();
        let source_ref_seed = input_event_refs.join("|");
        let long_client_order_id = funding_client_order_id(
            &opportunity.long_leg.venue_id,
            "L",
            &format!(
                "{}:{}:{}:{}:{}:perp_long",
                config.output.transition_id,
                config.symbol.symbol,
                opportunity.long_leg.leg_id,
                created_at,
                source_ref_seed
            ),
        );
        let short_client_order_id = funding_client_order_id(
            &opportunity.short_leg.venue_id,
            "S",
            &format!(
                "{}:{}:{}:{}:{}:perp_short",
                config.output.transition_id,
                config.symbol.symbol,
                opportunity.short_leg.leg_id,
                created_at,
                source_ref_seed
            ),
        );
        let funding_summary = format!(
            "Cross-exchange funding spread: long {} rate={}, short {} rate={}, normalized_gross_spread_bps={}, net_funding_bps={}, interval_hours={}.",
            opportunity.long_leg.venue_label,
            opportunity.long_premium.last_funding_rate,
            opportunity.short_leg.venue_label,
            opportunity.short_premium.last_funding_rate,
            gross_spread_bps,
            expected_profit_bps,
            opportunity.funding_interval_hours
        );
        let liquidity_summary = format!(
            "Depth/VWAP liquidity checked before candidate emission: liquidity_model={}, long_ask_depth_usd={}, short_bid_depth_usd={}, long_ask_vwap={}, short_bid_vwap={}, required_notional_usd={}.",
            opportunity.signal.liquidity_model,
            opportunity
                .signal
                .long_ask_depth_usd
                .as_deref()
                .unwrap_or("missing"),
            opportunity
                .signal
                .short_bid_depth_usd
                .as_deref()
                .unwrap_or("missing"),
            opportunity
                .signal
                .long_ask_vwap
                .as_deref()
                .unwrap_or("missing"),
            opportunity
                .signal
                .short_bid_vwap
                .as_deref()
                .unwrap_or("missing"),
            opportunity.signal.liquidity_required_usd
        );
        let margin_summary = "Dry-run candidate reserves notional on both venues; private margin and account balances are not read by the strategy.";
        let assumption = format!(
            "Read-only cross-exchange funding arbitrage signal for {}: long {} at {}, short {} at {}. Static fees and public top-of-book data are assumptions, not execution authorization.",
            config.symbol.symbol,
            opportunity.long_leg.venue_label,
            opportunity.long_book.best_ask.format_trimmed(),
            opportunity.short_leg.venue_label,
            opportunity.short_book.best_bid.format_trimmed()
        );

        let candidate_json = format!(
            r#"{{
  "schema_version": "1.0.0",
  "transition_id": {},
  "strategy_id": {},
  "strategy_version": {},
  "code_version": {},
  "config_version": {},
  "created_at": {},
  "input_event_refs": {},
  "current_portfolio_state_ref": {},
  "holding_period": {{
    "kind": "UntilFundingTimestamp"
  }},
  "legs": [
    {{
      "leg_id": {},
      "leg_type": "Trade",
      "venue_id": {},
      "instrument_id": {},
      "account_id": {},
      "side": "Long",
      "asset_flows": [],
      "constraints": {{
        "basis_leg_role": "perp_long",
        "client_order_id": {},
        "entry_price_divergence_bps": {},
        "entry_price_edge_bps": {},
        "funding_interval_hours": {},
        "funding_rate": {},
        "gross_funding_spread_bps": {},
        "max_slippage_bps": {},
        "net_funding_bps": {},
        "notional_usd": {},
        "reference_best_ask": {},
        "reference_best_bid": {},
        "reference_bid_size": {},
        "reference_ask_size": {},
        "reference_market_event_id": {},
        "reference_premium_event_id": {},
        "venue_symbol": {}
      }},
      "failure_modes": [
        "PartialFill",
        "VenueOutage"
      ]
    }},
    {{
      "leg_id": {},
      "leg_type": "Trade",
      "venue_id": {},
      "instrument_id": {},
      "account_id": {},
      "side": "Short",
      "asset_flows": [],
      "constraints": {{
        "basis_leg_role": "perp_short",
        "client_order_id": {},
        "entry_price_divergence_bps": {},
        "entry_price_edge_bps": {},
        "funding_interval_hours": {},
        "funding_rate": {},
        "gross_funding_spread_bps": {},
        "max_slippage_bps": {},
        "net_funding_bps": {},
        "notional_usd": {},
        "reference_best_ask": {},
        "reference_best_bid": {},
        "reference_bid_size": {},
        "reference_ask_size": {},
        "reference_market_event_id": {},
        "reference_premium_event_id": {},
        "venue_symbol": {}
      }},
      "failure_modes": [
        "PartialFill",
        "VenueOutage"
      ]
    }}
  ],
  "expected_post_state_delta": {{
    "asset_flows": [],
    "position_deltas": [
      {{
        "instrument_id": {},
        "account_id": {},
        "quantity_delta": {}
      }},
      {{
        "instrument_id": {},
        "account_id": {},
        "quantity_delta": {}
      }}
    ]
  }},
  "expected_economics": {{
    "expected_profit_usd": {},
    "expected_profit_bps": {},
    "fee_estimate_usd": {},
    "slippage_estimate_usd": {},
    "confidence": {}
  }},
  "required_capital": {{
    "asset_requirements": [
      {{
        "asset_id": {},
        "direction": "Out",
        "amount": {},
        "account_id": {}
      }},
      {{
        "asset_id": {},
        "direction": "Out",
        "amount": {},
        "account_id": {}
      }}
    ],
    "recovery_buffer_usd": {}
  }},
  "margin_impact": {{
    "summary": {},
    "impact_usd": "0",
    "confidence": {}
  }},
  "funding_impact": {{
    "summary": {},
    "impact_usd": {},
    "confidence": {}
  }},
  "liquidity_impact": {{
    "summary": {},
    "impact_usd": {},
    "confidence": {}
  }},
  "failure_modes": [
    "PartialFill",
    "VenueOutage"
  ],
  "risk_flags": [],
  "assumptions": [
    {{
      "assumption_id": {},
      "statement": {},
      "confidence": {},
      "source_event_refs": {}
    }}
  ]
}}"#,
            json_string(&config.output.transition_id),
            json_string(self.metadata.strategy_id()),
            json_string(self.metadata.strategy_version()),
            json_string(self.metadata.code_version()),
            json_string(context.config().config_version()),
            json_string(&created_at),
            input_event_refs_json,
            json_string(context.snapshot().portfolio_state_id()),
            json_string(&opportunity.long_leg.leg_id),
            json_string(&opportunity.long_leg.venue_id),
            json_string(&opportunity.long_leg.instrument_id),
            json_string(&opportunity.long_leg.account_id),
            json_string(&long_client_order_id),
            json_string(&entry_price_divergence_bps),
            json_string(&entry_price_edge_bps),
            json_string(&opportunity.funding_interval_hours),
            json_string(&opportunity.long_premium.last_funding_rate),
            json_string(&gross_spread_bps),
            json_string(&config.economics.slippage_buffer_bps.to_string()),
            json_string(&expected_profit_bps),
            json_string(&config.economics.notional_usd),
            json_string(&opportunity.long_book.best_ask.format_trimmed()),
            json_string(&opportunity.long_book.best_bid.format_trimmed()),
            json_string(&opportunity.long_book.bid_size.format_trimmed()),
            json_string(&opportunity.long_book.ask_size.format_trimmed()),
            json_string(&opportunity.long_book.event_id),
            json_string(&opportunity.long_premium.event_id),
            json_string(&config.symbol.symbol),
            json_string(&opportunity.short_leg.leg_id),
            json_string(&opportunity.short_leg.venue_id),
            json_string(&opportunity.short_leg.instrument_id),
            json_string(&opportunity.short_leg.account_id),
            json_string(&short_client_order_id),
            json_string(&entry_price_divergence_bps),
            json_string(&entry_price_edge_bps),
            json_string(&opportunity.funding_interval_hours),
            json_string(&opportunity.short_premium.last_funding_rate),
            json_string(&gross_spread_bps),
            json_string(&config.economics.slippage_buffer_bps.to_string()),
            json_string(&expected_profit_bps),
            json_string(&config.economics.notional_usd),
            json_string(&opportunity.short_book.best_ask.format_trimmed()),
            json_string(&opportunity.short_book.best_bid.format_trimmed()),
            json_string(&opportunity.short_book.bid_size.format_trimmed()),
            json_string(&opportunity.short_book.ask_size.format_trimmed()),
            json_string(&opportunity.short_book.event_id),
            json_string(&opportunity.short_premium.event_id),
            json_string(&config.symbol.symbol),
            json_string(&opportunity.long_leg.instrument_id),
            json_string(&opportunity.long_leg.account_id),
            json_string(&opportunity.signal.quantity),
            json_string(&opportunity.short_leg.instrument_id),
            json_string(&opportunity.short_leg.account_id),
            json_string(&format!("-{}", opportunity.signal.quantity)),
            json_string(&opportunity.signal.expected_funding_usd),
            json_string(&expected_profit_bps),
            json_string(&opportunity.signal.fee_estimate_usd),
            json_string(&opportunity.signal.slippage_estimate_usd),
            config.output.expected_economics_confidence.as_str(),
            json_string(&config.symbol.settlement_asset_id),
            json_string(&config.economics.notional_usd),
            json_string(&opportunity.long_leg.account_id),
            json_string(&config.symbol.settlement_asset_id),
            json_string(&config.economics.notional_usd),
            json_string(&opportunity.short_leg.account_id),
            json_string(&config.output.recovery_buffer_usd),
            json_string(margin_summary),
            config.output.margin_impact_confidence.as_str(),
            json_string(&funding_summary),
            json_string(&opportunity.signal.expected_funding_usd),
            config.output.funding_impact_confidence.as_str(),
            json_string(&liquidity_summary),
            json_string(&opportunity.signal.slippage_estimate_usd),
            config.output.liquidity_impact_confidence.as_str(),
            json_string(&config.output.assumption_id),
            json_string(&assumption),
            config.output.assumption_confidence.as_str(),
            json_string_array(&input_event_refs),
        );

        candidate_from_json_strict(&candidate_json)
    }
}

impl Default for CrossExchangeFundingArbStrategy {
    fn default() -> Self {
        Self::new().expect("cross-exchange funding strategy metadata is static and valid")
    }
}

impl Strategy for CrossExchangeFundingArbStrategy {
    fn metadata(&self) -> &StrategyMetadata {
        &self.metadata
    }

    fn evaluate(&self, context: &dyn StrategyReadContext) -> StrategyApiResult<StrategyEvaluation> {
        if context.config().kill_switch_triggered() {
            return self.reject(
                context,
                StrategyRejectReason::KillSwitchTriggered,
                "global or execution kill switch is active",
            );
        }
        if context
            .config()
            .strategy_disabled(self.metadata.strategy_id())
        {
            return self.reject(
                context,
                StrategyRejectReason::ConfigDisabled,
                "cross-exchange funding arbitrage strategy is disabled by read-only config",
            );
        }
        for leg in [&self.config.venues.venue_a, &self.config.venues.venue_b] {
            if context.config().venue_disabled(&leg.venue_id) {
                return self.reject(
                    context,
                    StrategyRejectReason::ConfigDisabled,
                    format!("{} is disabled by read-only config", leg.venue_id),
                );
            }
        }
        if context.snapshot().source_event_refs().is_empty() {
            return self.reject(
                context,
                StrategyRejectReason::MissingData,
                "portfolio snapshot has no input event references",
            );
        }
        if let Some(rejection) = self.ensure_capabilities(context)? {
            return Ok(rejection);
        }

        let venue_a = &self.config.venues.venue_a;
        let venue_b = &self.config.venues.venue_b;
        let book_a = match latest_basis_book_ticker(
            context,
            &venue_a.venue_id,
            &venue_a.instrument_id,
            "Perp",
            &venue_a.venue_label,
        ) {
            Ok(book) => book,
            Err(detail) => return self.reject(context, StrategyRejectReason::MissingData, detail),
        };
        let book_b = match latest_basis_book_ticker(
            context,
            &venue_b.venue_id,
            &venue_b.instrument_id,
            "Perp",
            &venue_b.venue_label,
        ) {
            Ok(book) => book,
            Err(detail) => return self.reject(context, StrategyRejectReason::MissingData, detail),
        };
        let premium_a = match latest_basis_premium_index(
            context,
            &venue_a.venue_id,
            &venue_a.instrument_id,
            &venue_a.venue_label,
        ) {
            Ok(premium) => premium,
            Err(detail) => return self.reject(context, StrategyRejectReason::MissingData, detail),
        };
        let premium_b = match latest_basis_premium_index(
            context,
            &venue_b.venue_id,
            &venue_b.instrument_id,
            &venue_b.venue_label,
        ) {
            Ok(premium) => premium,
            Err(detail) => return self.reject(context, StrategyRejectReason::MissingData, detail),
        };
        if book_a.is_stale || book_b.is_stale || premium_a.is_stale || premium_b.is_stale {
            return self.reject(
                context,
                StrategyRejectReason::DataStale,
                "funding arbitrage input contains stale public market data",
            );
        }
        for (label, premium) in [("venue A", &premium_a), ("venue B", &premium_b)] {
            let divergence_bps =
                match mark_index_divergence_bps(premium.mark_price, premium.index_price) {
                    Ok(value) => value,
                    Err(detail) => {
                        return self.reject(context, StrategyRejectReason::MissingData, detail)
                    }
                };
            if divergence_bps > CROSS_EXCHANGE_MAX_MARK_INDEX_DIVERGENCE_BPS {
                return self.reject(
                    context,
                    StrategyRejectReason::NoCandidate,
                    format!(
                        "{label} mark/index divergence {divergence_bps} bps exceeds maximum {} bps",
                        CROSS_EXCHANGE_MAX_MARK_INDEX_DIVERGENCE_BPS
                    ),
                );
            }
        }

        let interval_a = match premium_a.funding_interval_hours.as_deref() {
            Some(value) => value,
            None => {
                return self.reject(
                    context,
                    StrategyRejectReason::MissingData,
                    "venue A premium event is missing funding_interval_hours",
                );
            }
        };
        let interval_b = match premium_b.funding_interval_hours.as_deref() {
            Some(value) => value,
            None => {
                return self.reject(
                    context,
                    StrategyRejectReason::MissingData,
                    "venue B premium event is missing funding_interval_hours",
                );
            }
        };
        let rate_a = match FixedDecimal::parse_signed_rate(
            "venue_a_funding_rate",
            &premium_a.last_funding_rate,
        ) {
            Ok(rate) => rate,
            Err(detail) => return self.reject(context, StrategyRejectReason::MissingData, detail),
        };
        let rate_b = match FixedDecimal::parse_signed_rate(
            "venue_b_funding_rate",
            &premium_b.last_funding_rate,
        ) {
            Ok(rate) => rate,
            Err(detail) => return self.reject(context, StrategyRejectReason::MissingData, detail),
        };
        let interval_a_hours =
            match parse_positive_u64("venue_a_funding_interval_hours", interval_a) {
                Ok(value) => value,
                Err(detail) => {
                    return self.reject(context, StrategyRejectReason::MissingData, detail)
                }
            };
        let interval_b_hours =
            match parse_positive_u64("venue_b_funding_interval_hours", interval_b) {
                Ok(value) => value,
                Err(detail) => {
                    return self.reject(context, StrategyRejectReason::MissingData, detail)
                }
            };
        let normalized_rate_a = match normalize_funding_rate_to_8h(rate_a, interval_a_hours) {
            Ok(rate) => rate,
            Err(detail) => return self.reject(context, StrategyRejectReason::MissingData, detail),
        };
        let normalized_rate_b = match normalize_funding_rate_to_8h(rate_b, interval_b_hours) {
            Ok(rate) => rate,
            Err(detail) => return self.reject(context, StrategyRejectReason::MissingData, detail),
        };

        let (
            long_leg,
            short_leg,
            long_book,
            short_book,
            long_premium,
            short_premium,
            long_signal_funding_rate,
            short_signal_funding_rate,
            long_fee,
            short_fee,
        ) = if normalized_rate_a.raw <= normalized_rate_b.raw {
            (
                venue_a,
                venue_b,
                &book_a,
                &book_b,
                &premium_a,
                &premium_b,
                normalized_rate_a.format_trimmed(),
                normalized_rate_b.format_trimmed(),
                self.config.economics.venue_a_taker_fee_bps.clone(),
                self.config.economics.venue_b_taker_fee_bps.clone(),
            )
        } else {
            (
                venue_b,
                venue_a,
                &book_b,
                &book_a,
                &premium_b,
                &premium_a,
                normalized_rate_b.format_trimmed(),
                normalized_rate_a.format_trimmed(),
                self.config.economics.venue_b_taker_fee_bps.clone(),
                self.config.economics.venue_a_taker_fee_bps.clone(),
            )
        };
        let signal =
            match evaluate_cross_exchange_funding_arb_signal(&CrossExchangeFundingArbSignalInput {
                symbol: self.config.symbol.symbol.clone(),
                long_venue_id: long_leg.venue_id.clone(),
                short_venue_id: short_leg.venue_id.clone(),
                long_best_bid: long_book.best_bid.format_trimmed(),
                long_best_ask: long_book.best_ask.format_trimmed(),
                long_ask_size: Some(long_book.ask_size.format_trimmed()),
                long_ask_depth: long_book.ask_depth.clone(),
                short_best_bid: short_book.best_bid.format_trimmed(),
                short_best_ask: short_book.best_ask.format_trimmed(),
                short_bid_size: Some(short_book.bid_size.format_trimmed()),
                short_bid_depth: short_book.bid_depth.clone(),
                long_funding_rate: long_signal_funding_rate,
                short_funding_rate: short_signal_funding_rate,
                funding_interval_hours: "8".to_owned(),
                notional_usd: self.config.economics.notional_usd.clone(),
                long_taker_fee_bps: long_fee,
                short_taker_fee_bps: short_fee,
                slippage_buffer_bps: self.config.economics.slippage_buffer_bps,
                max_entry_price_divergence_bps: self
                    .config
                    .economics
                    .max_entry_price_divergence_bps,
                min_net_funding_bps: self.config.economics.min_net_funding_bps,
            }) {
                Ok(signal) => signal,
                Err(detail) => {
                    return self.reject(context, StrategyRejectReason::MissingData, detail)
                }
            };
        if !signal.is_candidate {
            return self.reject(
                context,
                StrategyRejectReason::NoCandidate,
                signal.reason.clone().unwrap_or_else(|| {
                    "funding spread did not satisfy candidate checks".to_owned()
                }),
            );
        }

        let opportunity = CrossExchangeFundingOpportunity {
            long_leg,
            short_leg,
            long_book,
            short_book,
            long_premium,
            short_premium,
            funding_interval_hours: "8".to_owned(),
            signal,
        };
        let candidate = self.build_candidate(context, &opportunity)?;
        validate_candidate_for_context(context, self.metadata(), &candidate)?;
        let diagnostic = StrategyDiagnostic::new(
            "CROSS_EXCHANGE_FUNDING_ARB_CANDIDATE",
            format!(
                "candidate {} emitted with long venue {} and short venue {}",
                candidate.transition_id.as_str(),
                opportunity.long_leg.venue_id,
                opportunity.short_leg.venue_id
            ),
            context.time().now(),
        )?;
        Ok(StrategyEvaluation::candidate(candidate).with_diagnostic(diagnostic))
    }
}

struct CrossExchangeFundingOpportunity<'a> {
    long_leg: &'a CrossExchangeFundingLegConfig,
    short_leg: &'a CrossExchangeFundingLegConfig,
    long_book: &'a BasisBookTickerInput,
    short_book: &'a BasisBookTickerInput,
    long_premium: &'a BasisPremiumIndexInput,
    short_premium: &'a BasisPremiumIndexInput,
    funding_interval_hours: String,
    signal: CrossExchangeFundingArbSignal,
}

/// 返回 Binance spot-perp basis 只读策略。
pub fn binance_spot_perp_basis_strategy() -> StrategyApiResult<SpotPerpBasisStrategy> {
    SpotPerpBasisStrategy::new()
}

/// 返回 Bybit spot-perp basis 参数化只读策略。
pub fn bybit_spot_perp_basis_strategy() -> StrategyApiResult<SpotPerpBasisStrategy> {
    SpotPerpBasisStrategy::with_config(SpotPerpBasisStrategyConfig::bybit_btcusdt())
}

/// 返回 OKX spot-swap basis 参数化只读策略。
pub fn okx_spot_swap_basis_strategy() -> StrategyApiResult<SpotPerpBasisStrategy> {
    SpotPerpBasisStrategy::with_config(SpotPerpBasisStrategyConfig::okx_btc_usdt())
}

/// 返回默认 spot-perp basis 只读策略；当前默认保持为 Binance。
pub fn spot_perp_basis_strategy() -> StrategyApiResult<SpotPerpBasisStrategy> {
    binance_spot_perp_basis_strategy()
}

/// 返回默认跨交易所资金费率套利只读策略。
pub fn cross_exchange_funding_arb_strategy() -> StrategyApiResult<CrossExchangeFundingArbStrategy> {
    CrossExchangeFundingArbStrategy::new()
}

/// 订单簿单档深度。
///
/// 中文说明：`price` 是该档价格，`size` 是该档基础币数量。策略会按目标名义本金
/// 逐档吃单并计算 VWAP（成交量加权平均价）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignalDepthLevel {
    pub price: String,
    pub size: String,
}

/// spot-perp basis 只读信号输入。
///
/// 中文说明：这是给运行时监控批量复用的纯计算输入；它不包含账户、订单、签名
/// 或任何可变执行能力。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotPerpBasisSignalInput {
    pub symbol: String,
    pub spot_best_bid: String,
    pub spot_best_ask: String,
    pub spot_ask_size: Option<String>,
    pub spot_ask_depth: Vec<SignalDepthLevel>,
    pub perp_best_bid: String,
    pub perp_best_ask: String,
    pub perp_bid_size: Option<String>,
    pub perp_bid_depth: Vec<SignalDepthLevel>,
    pub last_funding_rate: String,
    pub notional_usd: String,
    pub spot_taker_fee_bps: String,
    pub perp_taker_fee_bps: String,
    pub slippage_buffer_bps: i128,
    pub min_net_bps: i128,
}

/// spot-perp basis 只读信号输出。
///
/// 中文说明：该输出只说明机会是否满足阈值，不能被当作订单或执行授权。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotPerpBasisSignal {
    pub symbol: String,
    pub gross_bps: String,
    pub total_cost_bps: String,
    pub net_bps: String,
    pub funding_bps: String,
    pub expected_profit_bps: String,
    pub quantity: String,
    pub expected_profit_usd: String,
    pub funding_impact_usd: String,
    pub fee_estimate_usd: String,
    pub slippage_estimate_usd: String,
    pub liquidity_required_usd: String,
    pub spot_ask_depth_usd: Option<String>,
    pub perp_bid_depth_usd: Option<String>,
    pub spot_ask_vwap: Option<String>,
    pub perp_bid_vwap: Option<String>,
    pub spot_ask_levels_used: usize,
    pub perp_bid_levels_used: usize,
    pub spot_ask_price_impact_bps: Option<i128>,
    pub perp_bid_price_impact_bps: Option<i128>,
    pub liquidity_model: String,
    pub is_candidate: bool,
    pub reason: Option<String>,
}

/// 跨交易所资金费率套利只读信号输入。
///
/// 中文说明：调用方必须先根据 funding rate 决定 long/short 方向。这里仅基于公开
/// 行情、静态手续费和阈值计算候选资格，不代表真实执行授权。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingArbSignalInput {
    pub symbol: String,
    pub long_venue_id: String,
    pub short_venue_id: String,
    pub long_best_bid: String,
    pub long_best_ask: String,
    pub long_ask_size: Option<String>,
    pub long_ask_depth: Vec<SignalDepthLevel>,
    pub short_best_bid: String,
    pub short_best_ask: String,
    pub short_bid_size: Option<String>,
    pub short_bid_depth: Vec<SignalDepthLevel>,
    pub long_funding_rate: String,
    pub short_funding_rate: String,
    pub funding_interval_hours: String,
    pub notional_usd: String,
    pub long_taker_fee_bps: String,
    pub short_taker_fee_bps: String,
    pub slippage_buffer_bps: i128,
    pub max_entry_price_divergence_bps: i128,
    pub min_net_funding_bps: i128,
}

/// 跨交易所资金费率套利只读信号输出。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingArbSignal {
    pub symbol: String,
    pub gross_funding_spread_bps: String,
    pub total_cost_bps: String,
    pub net_funding_bps: String,
    pub entry_price_divergence_bps: i128,
    pub entry_price_edge_bps: String,
    pub quantity: String,
    pub expected_funding_usd: String,
    pub fee_estimate_usd: String,
    pub slippage_estimate_usd: String,
    pub liquidity_required_usd: String,
    pub long_ask_depth_usd: Option<String>,
    pub short_bid_depth_usd: Option<String>,
    pub long_ask_vwap: Option<String>,
    pub short_bid_vwap: Option<String>,
    pub long_ask_levels_used: usize,
    pub short_bid_levels_used: usize,
    pub long_ask_price_impact_bps: Option<i128>,
    pub short_bid_price_impact_bps: Option<i128>,
    pub liquidity_model: String,
    pub is_candidate: bool,
    pub reason: Option<String>,
}

/// 计算 spot-perp basis 只读信号。
pub fn evaluate_spot_perp_basis_signal(
    input: &SpotPerpBasisSignalInput,
) -> Result<SpotPerpBasisSignal, String> {
    ensure_non_negative_signal_bps("slippage_buffer_bps", input.slippage_buffer_bps)?;
    ensure_non_negative_signal_bps("min_net_bps", input.min_net_bps)?;
    let spot_ask = FixedDecimal::parse_non_negative("spot_best_ask", &input.spot_best_ask)?;
    let perp_bid = FixedDecimal::parse_non_negative("perp_best_bid", &input.perp_best_bid)?;
    if spot_ask.raw <= 0 || perp_bid.raw <= 0 {
        return Err("entry top-of-book prices must be greater than zero".to_owned());
    }
    let notional = FixedDecimal::parse_non_negative("notional_usd", &input.notional_usd)?;
    if notional.is_zero() {
        return Err("notional_usd must be greater than zero".to_owned());
    }
    let funding_rate =
        FixedDecimal::parse_signed_rate("last_funding_rate", &input.last_funding_rate)?;
    let spot_taker_fee_bps =
        FixedDecimal::parse_non_negative("spot_taker_fee_bps", &input.spot_taker_fee_bps)?;
    let perp_taker_fee_bps =
        FixedDecimal::parse_non_negative("perp_taker_fee_bps", &input.perp_taker_fee_bps)?;
    let slippage_buffer_bps = FixedDecimal::bps_from_i128(input.slippage_buffer_bps)?;
    let min_net_bps = FixedDecimal::bps_from_i128(input.min_net_bps)?;
    let (spot_ask_levels, spot_model) = depth_levels_or_top_of_book(
        &input.spot_ask_depth,
        spot_ask,
        input.spot_ask_size.as_deref(),
        "spot_ask_size",
    )?;
    let (perp_bid_levels, perp_model) = depth_levels_or_top_of_book(
        &input.perp_bid_depth,
        perp_bid,
        input.perp_bid_size.as_deref(),
        "perp_bid_size",
    )?;
    let spot_execution =
        depth_execution_for_notional(DepthSide::Ask, spot_ask_levels, notional, spot_model)?;
    let perp_execution =
        depth_execution_for_notional(DepthSide::Bid, perp_bid_levels, notional, perp_model)?;
    let execution_spot_ask = depth_execution_price(&spot_execution, spot_ask);
    let execution_perp_bid = depth_execution_price(&perp_execution, perp_bid);
    let liquidity_model =
        if spot_execution.model == "order_book_vwap" || perp_execution.model == "order_book_vwap" {
            "order_book_vwap"
        } else {
            "top_of_book_as_single_level"
        }
        .to_owned();
    let single_leg_gross_bps = gross_basis_bps_decimal(execution_perp_bid, execution_spot_ask)?;
    let single_leg_round_trip_fee_bps =
        round_trip_taker_fee_bps(spot_taker_fee_bps, perp_taker_fee_bps)?;
    let single_leg_total_cost_bps = single_leg_round_trip_fee_bps
        .checked_add(slippage_buffer_bps, "total cost bps calculation overflowed")?;
    let single_leg_net_bps = single_leg_gross_bps.checked_sub(
        single_leg_total_cost_bps,
        "net basis bps calculation overflowed",
    )?;
    let single_leg_funding_bps = FixedDecimal::bps_from_rate_decimal(funding_rate)?;
    let single_leg_expected_profit_bps = single_leg_net_bps.checked_add(
        single_leg_funding_bps,
        "expected profit bps calculation overflowed",
    )?;
    let gross_bps = single_leg_gross_bps;
    let total_cost_bps = single_leg_total_cost_bps;
    let net_bps = two_leg_return_bps(single_leg_net_bps);
    let funding_bps = single_leg_funding_bps;
    let expected_profit_bps = two_leg_return_bps(single_leg_expected_profit_bps);
    let quantity = spot_execution
        .quantity
        .unwrap_or(FixedDecimal::quantity_for_notional(
            notional,
            execution_spot_ask,
        )?);
    let basis_profit_usd = FixedDecimal::usd_from_bps_decimal(notional, single_leg_net_bps)?;
    let funding_impact_usd = FixedDecimal::usd_from_rate(notional, funding_rate)?;
    let expected_profit_usd = basis_profit_usd.checked_add(
        funding_impact_usd,
        "expected profit USD calculation overflowed",
    )?;
    let fee_estimate_usd =
        FixedDecimal::usd_from_bps_decimal(notional, single_leg_round_trip_fee_bps)?;
    let slippage_estimate_usd = FixedDecimal::usd_from_bps_decimal(notional, slippage_buffer_bps)?;
    let spot_ask_depth_usd = spot_execution.depth_usd;
    let perp_bid_depth_usd = perp_execution.depth_usd;
    let depth_is_sufficient = spot_execution.covered_notional_usd.raw >= notional.raw
        && perp_execution.covered_notional_usd.raw >= notional.raw;
    let threshold_is_satisfied = expected_profit_bps.raw >= min_net_bps.raw;
    let is_candidate = depth_is_sufficient && threshold_is_satisfied;
    let reason = if !depth_is_sufficient {
        Some(format!(
            "insufficient order-book depth: spot_ask_depth_usd={}, perp_bid_depth_usd={}, required_notional_usd={}, spot_ask_levels_used={}, perp_bid_levels_used={}, liquidity_model={}",
            spot_ask_depth_usd.format_trimmed(),
            perp_bid_depth_usd.format_trimmed(),
            notional.format_trimmed(),
            spot_execution.levels_used,
            perp_execution.levels_used,
            liquidity_model
        ))
    } else if !threshold_is_satisfied {
        Some(format!(
            "basis expected_profit_bps={} below minimum {}; gross_bps={}, total_cost_bps={}, net_basis_bps={}, funding_bps={}, spot_ask_vwap={}, perp_bid_vwap={}",
            expected_profit_bps.format_trimmed(),
            input.min_net_bps,
            gross_bps.format_trimmed(),
            total_cost_bps.format_trimmed(),
            net_bps.format_trimmed(),
            funding_bps.format_trimmed(),
            execution_spot_ask.format_trimmed(),
            execution_perp_bid.format_trimmed()
        ))
    } else {
        None
    };

    Ok(SpotPerpBasisSignal {
        symbol: input.symbol.clone(),
        gross_bps: gross_bps.format_trimmed(),
        total_cost_bps: total_cost_bps.format_trimmed(),
        net_bps: net_bps.format_trimmed(),
        funding_bps: funding_bps.format_trimmed(),
        expected_profit_bps: expected_profit_bps.format_trimmed(),
        quantity: quantity.format_trimmed(),
        expected_profit_usd: expected_profit_usd.format_trimmed(),
        funding_impact_usd: funding_impact_usd.format_trimmed(),
        fee_estimate_usd: fee_estimate_usd.format_trimmed(),
        slippage_estimate_usd: slippage_estimate_usd.format_trimmed(),
        liquidity_required_usd: notional.format_trimmed(),
        spot_ask_depth_usd: Some(spot_ask_depth_usd.format_trimmed()),
        perp_bid_depth_usd: Some(perp_bid_depth_usd.format_trimmed()),
        spot_ask_vwap: spot_execution.vwap.map(FixedDecimal::format_trimmed),
        perp_bid_vwap: perp_execution.vwap.map(FixedDecimal::format_trimmed),
        spot_ask_levels_used: spot_execution.levels_used,
        perp_bid_levels_used: perp_execution.levels_used,
        spot_ask_price_impact_bps: Some(ask_price_impact_bps(execution_spot_ask, spot_ask)?),
        perp_bid_price_impact_bps: Some(bid_price_impact_bps(execution_perp_bid, perp_bid)?),
        liquidity_model,
        is_candidate,
        reason,
    })
}

/// 计算跨交易所资金费率套利只读信号。
pub fn evaluate_cross_exchange_funding_arb_signal(
    input: &CrossExchangeFundingArbSignalInput,
) -> Result<CrossExchangeFundingArbSignal, String> {
    ensure_non_negative_signal_bps("slippage_buffer_bps", input.slippage_buffer_bps)?;
    ensure_non_negative_signal_bps(
        "max_entry_price_divergence_bps",
        input.max_entry_price_divergence_bps,
    )?;
    ensure_non_negative_signal_bps("min_net_funding_bps", input.min_net_funding_bps)?;
    if input.long_venue_id == input.short_venue_id {
        return Err("long_venue_id and short_venue_id must be distinct".to_owned());
    }

    let long_bid = FixedDecimal::parse_non_negative("long_best_bid", &input.long_best_bid)?;
    let long_ask = FixedDecimal::parse_non_negative("long_best_ask", &input.long_best_ask)?;
    let short_bid = FixedDecimal::parse_non_negative("short_best_bid", &input.short_best_bid)?;
    let short_ask = FixedDecimal::parse_non_negative("short_best_ask", &input.short_best_ask)?;
    if long_bid.raw <= 0 || long_ask.raw <= 0 || short_bid.raw <= 0 || short_ask.raw <= 0 {
        return Err("top-of-book prices must be greater than zero".to_owned());
    }
    let notional = FixedDecimal::parse_non_negative("notional_usd", &input.notional_usd)?;
    if notional.is_zero() {
        return Err("notional_usd must be greater than zero".to_owned());
    }
    let long_taker_fee_bps =
        FixedDecimal::parse_non_negative("long_taker_fee_bps", &input.long_taker_fee_bps)?;
    let short_taker_fee_bps =
        FixedDecimal::parse_non_negative("short_taker_fee_bps", &input.short_taker_fee_bps)?;
    let slippage_buffer_bps = FixedDecimal::bps_from_i128(input.slippage_buffer_bps)?;
    let min_net_funding_bps = FixedDecimal::bps_from_i128(input.min_net_funding_bps)?;
    let (long_ask_levels, long_model) = depth_levels_or_top_of_book(
        &input.long_ask_depth,
        long_ask,
        input.long_ask_size.as_deref(),
        "long_ask_size",
    )?;
    let (short_bid_levels, short_model) = depth_levels_or_top_of_book(
        &input.short_bid_depth,
        short_bid,
        input.short_bid_size.as_deref(),
        "short_bid_size",
    )?;
    let long_execution =
        depth_execution_for_notional(DepthSide::Ask, long_ask_levels, notional, long_model)?;
    let short_execution =
        depth_execution_for_notional(DepthSide::Bid, short_bid_levels, notional, short_model)?;
    let execution_long_ask = depth_execution_price(&long_execution, long_ask);
    let execution_short_bid = depth_execution_price(&short_execution, short_bid);
    let liquidity_model = if long_execution.model == "order_book_vwap"
        || short_execution.model == "order_book_vwap"
    {
        "order_book_vwap"
    } else {
        "top_of_book_as_single_level"
    }
    .to_owned();
    let interval_hours =
        parse_positive_u64("funding_interval_hours", &input.funding_interval_hours)?;
    let long_rate = FixedDecimal::parse_signed_rate("long_funding_rate", &input.long_funding_rate)?;
    let short_rate =
        FixedDecimal::parse_signed_rate("short_funding_rate", &input.short_funding_rate)?;
    let raw_spread = short_rate
        .raw
        .checked_sub(long_rate.raw)
        .ok_or_else(|| "funding spread calculation overflowed".to_owned())?;
    let normalized_spread = raw_spread
        .checked_mul(8)
        .and_then(|value| value.checked_div(i128::from(interval_hours)))
        .ok_or_else(|| "funding interval normalization overflowed".to_owned())?;
    let single_leg_gross_funding_spread_bps = FixedDecimal::bps_from_rate_decimal(FixedDecimal {
        raw: normalized_spread,
    })?;
    let single_leg_round_trip_fee_bps =
        round_trip_taker_fee_bps(long_taker_fee_bps, short_taker_fee_bps)?;
    let single_leg_total_cost_bps = single_leg_round_trip_fee_bps.checked_add(
        slippage_buffer_bps,
        "total funding cost bps calculation overflowed",
    )?;
    let single_leg_net_funding_bps_before_entry = single_leg_gross_funding_spread_bps.checked_sub(
        single_leg_total_cost_bps,
        "net funding bps calculation overflowed",
    )?;
    let entry_price_edge_bps =
        signed_entry_price_edge_bps(execution_long_ask, execution_short_bid)?;
    let entry_price_adverse_bps = if entry_price_edge_bps.raw < 0 {
        FixedDecimal {
            raw: entry_price_edge_bps
                .raw
                .checked_neg()
                .ok_or_else(|| "entry price adverse bps calculation overflowed".to_owned())?,
        }
    } else {
        FixedDecimal { raw: 0 }
    };
    let single_leg_net_funding_bps = single_leg_net_funding_bps_before_entry.checked_sub(
        entry_price_adverse_bps,
        "entry-adjusted net funding bps calculation overflowed",
    )?;
    let gross_funding_spread_bps = single_leg_gross_funding_spread_bps;
    let total_cost_bps = single_leg_total_cost_bps;
    let net_funding_bps = two_leg_return_bps(single_leg_net_funding_bps);
    let entry_price_divergence_bps = price_divergence_bps(execution_long_ask, execution_short_bid)?;
    let quantity = long_execution
        .quantity
        .unwrap_or(FixedDecimal::quantity_for_notional(
            notional,
            execution_long_ask,
        )?);
    let expected_funding_usd =
        FixedDecimal::usd_from_bps_decimal(notional, single_leg_net_funding_bps)?;
    let fee_estimate_usd =
        FixedDecimal::usd_from_bps_decimal(notional, single_leg_round_trip_fee_bps)?;
    let slippage_estimate_usd = FixedDecimal::usd_from_bps_decimal(notional, slippage_buffer_bps)?;
    let long_ask_depth_usd = long_execution.depth_usd;
    let short_bid_depth_usd = short_execution.depth_usd;
    let depth_is_sufficient = long_execution.covered_notional_usd.raw >= notional.raw
        && short_execution.covered_notional_usd.raw >= notional.raw;
    let divergence_is_allowed = entry_price_divergence_bps <= input.max_entry_price_divergence_bps;
    let threshold_is_satisfied = net_funding_bps.raw >= min_net_funding_bps.raw;
    let is_candidate = depth_is_sufficient && divergence_is_allowed && threshold_is_satisfied;
    let reason = if !depth_is_sufficient {
        Some(format!(
            "insufficient order-book depth: long_ask_depth_usd={}, short_bid_depth_usd={}, required_notional_usd={}, long_ask_levels_used={}, short_bid_levels_used={}, liquidity_model={}",
            long_ask_depth_usd.format_trimmed(),
            short_bid_depth_usd.format_trimmed(),
            notional.format_trimmed(),
            long_execution.levels_used,
            short_execution.levels_used,
            liquidity_model
        ))
    } else if !divergence_is_allowed {
        Some(format!(
            "entry_price_divergence_bps={entry_price_divergence_bps} exceeds maximum {}; long_ask_vwap={}, short_bid_vwap={}",
            input.max_entry_price_divergence_bps,
            execution_long_ask.format_trimmed(),
            execution_short_bid.format_trimmed()
        ))
    } else if !threshold_is_satisfied {
        Some(format!(
            "net_funding_bps={} below minimum {}; gross_funding_spread_bps={}, total_cost_bps={}, entry_price_edge_bps={}, entry_price_adverse_bps={}",
            net_funding_bps.format_trimmed(),
            input.min_net_funding_bps,
            gross_funding_spread_bps.format_trimmed(),
            total_cost_bps.format_trimmed(),
            entry_price_edge_bps.format_trimmed(),
            entry_price_adverse_bps.format_trimmed()
        ))
    } else {
        None
    };

    Ok(CrossExchangeFundingArbSignal {
        symbol: input.symbol.clone(),
        gross_funding_spread_bps: gross_funding_spread_bps.format_trimmed(),
        total_cost_bps: total_cost_bps.format_trimmed(),
        net_funding_bps: net_funding_bps.format_trimmed(),
        entry_price_divergence_bps,
        entry_price_edge_bps: entry_price_edge_bps.format_trimmed(),
        quantity: quantity.format_trimmed(),
        expected_funding_usd: expected_funding_usd.format_trimmed(),
        fee_estimate_usd: fee_estimate_usd.format_trimmed(),
        slippage_estimate_usd: slippage_estimate_usd.format_trimmed(),
        liquidity_required_usd: notional.format_trimmed(),
        long_ask_depth_usd: Some(long_ask_depth_usd.format_trimmed()),
        short_bid_depth_usd: Some(short_bid_depth_usd.format_trimmed()),
        long_ask_vwap: long_execution.vwap.map(FixedDecimal::format_trimmed),
        short_bid_vwap: short_execution.vwap.map(FixedDecimal::format_trimmed),
        long_ask_levels_used: long_execution.levels_used,
        short_bid_levels_used: short_execution.levels_used,
        long_ask_price_impact_bps: Some(ask_price_impact_bps(execution_long_ask, long_ask)?),
        short_bid_price_impact_bps: Some(bid_price_impact_bps(execution_short_bid, short_bid)?),
        liquidity_model,
        is_candidate,
        reason,
    })
}

/// spot-perp basis 持仓后的 ADL 状态。
///
/// 中文说明：ADL 是交易所自动减仓风险信号。`Warning` 表示进入自动减仓队列或
/// 预警区间，应主动退出；`Deleveraging` 表示交易所已发生自动减仓或外部仓位
/// 状态可能被改变，后续必须先对账。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpotPerpBasisAdlState {
    None,
    Warning,
    Deleveraging,
}

impl SpotPerpBasisAdlState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Warning => "adl_warning",
            Self::Deleveraging => "adl_deleveraging",
        }
    }
}

/// spot-perp basis 平仓决策。
///
/// 中文说明：`EmergencyReconcileAndDeRisk` 不是等待人工；它表示不能按原始
/// 仓位假设直接普通平仓，运行时应先取消挂单、重拉私有状态、对账真实敞口，
/// 再用 reduce-only 或等价受限动作自动降风险；只有状态仍未知时才升级人工。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpotPerpBasisExitDecision {
    Hold,
    Close,
    EmergencyReconcileAndDeRisk,
}

impl SpotPerpBasisExitDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Close => "close",
            Self::EmergencyReconcileAndDeRisk => "emergency_reconcile_and_de_risk",
        }
    }
}

/// spot-perp basis 平仓触发原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpotPerpBasisExitReason {
    TakeProfit,
    BasisConverged,
    FundingNoLongerPays,
    BasisWidened,
    StopLoss,
    LiquidationBufferTooThin,
    LiquidationBufferMissing,
    PositionImbalance,
    DataStale,
    UnknownExternalState,
    AdlWarning,
    AdlDeleveraging,
}

impl SpotPerpBasisExitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TakeProfit => "TAKE_PROFIT",
            Self::BasisConverged => "BASIS_CONVERGED",
            Self::FundingNoLongerPays => "FUNDING_NO_LONGER_PAYS",
            Self::BasisWidened => "BASIS_WIDENED",
            Self::StopLoss => "STOP_LOSS",
            Self::LiquidationBufferTooThin => "LIQUIDATION_BUFFER_TOO_THIN",
            Self::LiquidationBufferMissing => "LIQUIDATION_BUFFER_MISSING",
            Self::PositionImbalance => "POSITION_IMBALANCE",
            Self::DataStale => "DATA_STALE",
            Self::UnknownExternalState => "UNKNOWN_EXTERNAL_STATE",
            Self::AdlWarning => "ADL_WARNING",
            Self::AdlDeleveraging => "ADL_DELEVERAGING",
        }
    }
}

/// spot-perp basis 平仓信号输入。
///
/// 中文说明：`entry_gross_basis_bps` 是开仓时的毛基差；`entry_total_cost_bps`
/// 是开仓成本；当前平仓基差用现货 bid 和永续 ask 的可成交价格保守计算。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotPerpBasisExitSignalInput {
    pub symbol: String,
    pub spot_best_bid: String,
    pub perp_best_ask: String,
    pub notional_usd: String,
    pub entry_gross_basis_bps: i128,
    pub entry_total_cost_bps: i128,
    pub accumulated_funding_bps: i128,
    pub expected_next_funding_bps: i128,
    pub exit_spot_taker_fee_bps: i128,
    pub exit_perp_taker_fee_bps: i128,
    pub exit_slippage_buffer_bps: i128,
    pub target_profit_bps: i128,
    pub convergence_buffer_bps: i128,
    pub min_next_funding_bps: i128,
    pub max_basis_widen_bps: i128,
    pub max_loss_bps: i128,
    pub liquidation_buffer_bps: Option<i128>,
    pub min_liquidation_buffer_bps: i128,
    pub position_imbalance_bps: i128,
    pub max_position_imbalance_bps: i128,
    pub data_is_stale: bool,
    pub external_state_unknown: bool,
    pub adl_state: SpotPerpBasisAdlState,
}

/// spot-perp basis 平仓信号输出。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotPerpBasisExitSignal {
    pub symbol: String,
    pub decision: SpotPerpBasisExitDecision,
    pub current_close_basis_bps: i128,
    pub basis_widen_bps: i128,
    pub exit_total_cost_bps: i128,
    pub estimated_exit_profit_bps: i128,
    pub estimated_exit_profit_usd: String,
    pub remaining_basis_edge_bps: i128,
    pub reason_codes: Vec<SpotPerpBasisExitReason>,
    pub detail: String,
}

/// 计算 spot-perp basis 持仓是否应平仓。
pub fn evaluate_spot_perp_basis_exit_signal(
    input: &SpotPerpBasisExitSignalInput,
) -> Result<SpotPerpBasisExitSignal, String> {
    ensure_non_negative_signal_bps("entry_total_cost_bps", input.entry_total_cost_bps)?;
    ensure_non_negative_signal_bps("exit_spot_taker_fee_bps", input.exit_spot_taker_fee_bps)?;
    ensure_non_negative_signal_bps("exit_perp_taker_fee_bps", input.exit_perp_taker_fee_bps)?;
    ensure_non_negative_signal_bps("exit_slippage_buffer_bps", input.exit_slippage_buffer_bps)?;
    ensure_non_negative_signal_bps("target_profit_bps", input.target_profit_bps)?;
    ensure_non_negative_signal_bps("convergence_buffer_bps", input.convergence_buffer_bps)?;
    ensure_non_negative_signal_bps("max_basis_widen_bps", input.max_basis_widen_bps)?;
    ensure_non_negative_signal_bps("max_loss_bps", input.max_loss_bps)?;
    ensure_non_negative_signal_bps(
        "min_liquidation_buffer_bps",
        input.min_liquidation_buffer_bps,
    )?;
    ensure_non_negative_signal_bps(
        "max_position_imbalance_bps",
        input.max_position_imbalance_bps,
    )?;

    let spot_bid = FixedDecimal::parse_non_negative("spot_best_bid", &input.spot_best_bid)?;
    let perp_ask = FixedDecimal::parse_non_negative("perp_best_ask", &input.perp_best_ask)?;
    let notional = FixedDecimal::parse_non_negative("notional_usd", &input.notional_usd)?;
    if notional.is_zero() {
        return Err("notional_usd must be greater than zero".to_owned());
    }
    if spot_bid.raw <= 0 {
        return Err("spot bid price must be greater than zero".to_owned());
    }
    if perp_ask.raw <= 0 {
        return Err("perp ask price must be greater than zero".to_owned());
    }

    let current_close_basis_bps = tradable_close_basis_bps(perp_ask, spot_bid)?;
    let exit_total_cost_bps = input
        .exit_spot_taker_fee_bps
        .checked_add(input.exit_perp_taker_fee_bps)
        .and_then(|value| value.checked_add(input.exit_slippage_buffer_bps))
        .ok_or_else(|| "exit cost bps calculation overflowed".to_owned())?;
    let entry_after_open_cost_bps = input
        .entry_gross_basis_bps
        .checked_sub(input.entry_total_cost_bps)
        .ok_or_else(|| "entry net basis bps calculation overflowed".to_owned())?;
    let estimated_exit_profit_bps = entry_after_open_cost_bps
        .checked_sub(current_close_basis_bps)
        .and_then(|value| value.checked_sub(exit_total_cost_bps))
        .and_then(|value| value.checked_add(input.accumulated_funding_bps))
        .ok_or_else(|| "exit profit bps calculation overflowed".to_owned())?;
    let estimated_exit_profit_usd =
        FixedDecimal::usd_from_bps(notional, estimated_exit_profit_bps)?.format_trimmed();
    let basis_widen_bps = current_close_basis_bps
        .checked_sub(input.entry_gross_basis_bps)
        .ok_or_else(|| "basis widening calculation overflowed".to_owned())?;
    let remaining_basis_edge_bps = current_close_basis_bps
        .checked_sub(exit_total_cost_bps)
        .ok_or_else(|| "remaining basis edge calculation overflowed".to_owned())?;
    let convergence_threshold_bps = exit_total_cost_bps
        .checked_add(input.convergence_buffer_bps)
        .ok_or_else(|| "convergence threshold bps calculation overflowed".to_owned())?;
    let stop_loss_threshold_bps = input
        .max_loss_bps
        .checked_neg()
        .ok_or_else(|| "stop loss bps calculation overflowed".to_owned())?;

    let mut reason_codes = Vec::new();
    if input.data_is_stale {
        reason_codes.push(SpotPerpBasisExitReason::DataStale);
    }
    if input.external_state_unknown {
        reason_codes.push(SpotPerpBasisExitReason::UnknownExternalState);
    }
    match input.adl_state {
        SpotPerpBasisAdlState::None => {}
        SpotPerpBasisAdlState::Warning => reason_codes.push(SpotPerpBasisExitReason::AdlWarning),
        SpotPerpBasisAdlState::Deleveraging => {
            reason_codes.push(SpotPerpBasisExitReason::AdlDeleveraging)
        }
    }
    match input.liquidation_buffer_bps {
        Some(buffer) if buffer <= input.min_liquidation_buffer_bps => {
            reason_codes.push(SpotPerpBasisExitReason::LiquidationBufferTooThin);
        }
        Some(_) => {}
        None => reason_codes.push(SpotPerpBasisExitReason::LiquidationBufferMissing),
    }
    if estimated_exit_profit_bps >= input.target_profit_bps {
        reason_codes.push(SpotPerpBasisExitReason::TakeProfit);
    }
    if current_close_basis_bps <= convergence_threshold_bps {
        reason_codes.push(SpotPerpBasisExitReason::BasisConverged);
    }
    if input.expected_next_funding_bps <= input.min_next_funding_bps {
        reason_codes.push(SpotPerpBasisExitReason::FundingNoLongerPays);
    }
    if basis_widen_bps >= input.max_basis_widen_bps {
        reason_codes.push(SpotPerpBasisExitReason::BasisWidened);
    }
    if estimated_exit_profit_bps <= stop_loss_threshold_bps {
        reason_codes.push(SpotPerpBasisExitReason::StopLoss);
    }
    if signed_bps_abs(input.position_imbalance_bps)? > input.max_position_imbalance_bps {
        reason_codes.push(SpotPerpBasisExitReason::PositionImbalance);
    }

    let requires_emergency_derisk = reason_codes.iter().any(|reason| {
        matches!(
            reason,
            SpotPerpBasisExitReason::DataStale
                | SpotPerpBasisExitReason::UnknownExternalState
                | SpotPerpBasisExitReason::LiquidationBufferMissing
                | SpotPerpBasisExitReason::AdlDeleveraging
        )
    });
    let decision = if requires_emergency_derisk {
        SpotPerpBasisExitDecision::EmergencyReconcileAndDeRisk
    } else if reason_codes.is_empty() {
        SpotPerpBasisExitDecision::Hold
    } else {
        SpotPerpBasisExitDecision::Close
    };
    let detail = if reason_codes.is_empty() {
        format!(
            "hold: close_basis_bps={current_close_basis_bps}, estimated_exit_profit_bps={estimated_exit_profit_bps}, expected_next_funding_bps={}",
            input.expected_next_funding_bps
        )
    } else {
        let reasons = reason_codes
            .iter()
            .map(|reason| reason.as_str())
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{}: {reasons}; close_basis_bps={current_close_basis_bps}, estimated_exit_profit_bps={estimated_exit_profit_bps}, adl_state={}",
            decision.as_str(),
            input.adl_state.as_str()
        )
    };

    Ok(SpotPerpBasisExitSignal {
        symbol: input.symbol.clone(),
        decision,
        current_close_basis_bps,
        basis_widen_bps,
        exit_total_cost_bps,
        estimated_exit_profit_bps,
        estimated_exit_profit_usd,
        remaining_basis_edge_bps,
        reason_codes,
        detail,
    })
}

fn ensure_non_negative_signal_bps(field: &'static str, value: i128) -> Result<(), String> {
    if value < 0 {
        return Err(format!("`{field}` cannot be negative"));
    }
    Ok(())
}

fn signed_bps_abs(value: i128) -> Result<i128, String> {
    if value < 0 {
        value
            .checked_neg()
            .ok_or_else(|| "signed bps absolute value overflowed".to_owned())
    } else {
        Ok(value)
    }
}

fn two_leg_return_bps(single_leg_bps: FixedDecimal) -> FixedDecimal {
    single_leg_bps
        .checked_div_i128(2, "two-leg bps calculation overflowed")
        .expect("dividing fixed decimal bps by two cannot overflow")
}

fn round_trip_taker_fee_bps(
    first_leg_taker_fee_bps: FixedDecimal,
    second_leg_taker_fee_bps: FixedDecimal,
) -> Result<FixedDecimal, String> {
    first_leg_taker_fee_bps
        .checked_add(
            second_leg_taker_fee_bps,
            "round-trip taker fee bps calculation overflowed",
        )?
        .checked_mul_i128(2, "round-trip taker fee bps calculation overflowed")
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MarketQuoteInput {
    event_id: String,
    best_bid: String,
    best_ask: String,
    last_price: String,
}

fn latest_market_quote(context: &dyn StrategyReadContext) -> Result<MarketQuoteInput, String> {
    let event = context
        .snapshot()
        .market_events()
        .iter()
        .rev()
        .find(|event| is_sample_market_quote_event(event))
        .ok_or_else(|| {
            format!(
                "missing normalized MarketQuote event for venue {SAMPLE_VENUE_ID} instrument {SAMPLE_INSTRUMENT_ID}"
            )
        })?;

    let best_bid = required_payload_decimal_string(event, "best_bid")?;
    let best_ask = required_payload_decimal_string(event, "best_ask")?;
    let last_price = required_payload_decimal_string(event, "last_price")?;

    Ok(MarketQuoteInput {
        event_id: event.event_id.as_str().to_owned(),
        best_bid,
        best_ask,
        last_price,
    })
}

fn is_sample_market_quote_event(event: &NormalizedEvent) -> bool {
    event.event_type == NormalizedEventType::NormalizedMarketDataEvent
        && nullable_identifier_matches(&event.venue_id, SAMPLE_VENUE_ID)
        && nullable_identifier_matches(&event.instrument_id, SAMPLE_INSTRUMENT_ID)
        && payload_string(event, "kind").is_some_and(|kind| kind == "MarketQuote")
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FixedDecimal {
    raw: i128,
}

impl FixedDecimal {
    fn parse_non_negative(field: &'static str, value: &str) -> Result<Self, String> {
        validate_market_decimal(field, value)?;
        Self::parse_decimal_body(field, value, false, false)
    }

    fn parse_non_negative_depth(field: &'static str, value: &str) -> Result<Self, String> {
        validate_market_decimal(field, value)?;
        Self::parse_decimal_body(field, value, false, true)
    }

    fn parse_signed_rate(field: &'static str, value: &str) -> Result<Self, String> {
        validate_signed_market_decimal(field, value)?;
        if let Some(unsigned) = value.strip_prefix('-') {
            Self::parse_decimal_body(field, unsigned, true, true)
        } else {
            Self::parse_decimal_body(field, value, false, true)
        }
    }

    fn parse_decimal_body(
        field: &'static str,
        value: &str,
        negative: bool,
        allow_extra_fractional_digits: bool,
    ) -> Result<Self, String> {
        let mut raw = 0_i128;
        let mut dot_seen = false;
        let mut frac_digits = 0_usize;
        for byte in value.bytes() {
            match byte {
                b'0'..=b'9' => {
                    if dot_seen {
                        if frac_digits == FIXED_SCALE_DIGITS {
                            if allow_extra_fractional_digits {
                                continue;
                            }
                            return Err(format!(
                                "decimal field `{field}` exceeds {FIXED_SCALE_DIGITS} fractional digits"
                            ));
                        }
                        frac_digits += 1;
                    }
                    raw = raw
                        .checked_mul(10)
                        .and_then(|value| value.checked_add(i128::from(byte - b'0')))
                        .ok_or_else(|| format!("decimal field `{field}` overflowed"))?;
                }
                b'.' => dot_seen = true,
                _ => {
                    return Err(format!("decimal field `{field}` must be a decimal string"));
                }
            }
        }
        for _ in frac_digits..FIXED_SCALE_DIGITS {
            raw = raw
                .checked_mul(10)
                .ok_or_else(|| format!("decimal field `{field}` overflowed"))?;
        }
        let raw = if negative {
            raw.checked_neg()
                .ok_or_else(|| format!("decimal field `{field}` overflowed"))?
        } else {
            raw
        };
        Ok(Self { raw })
    }

    fn is_zero(self) -> bool {
        self.raw == 0
    }

    fn quantity_for_notional(notional: Self, price: Self) -> Result<Self, String> {
        if price.raw <= 0 {
            return Err("spot ask price must be greater than zero".to_owned());
        }
        let raw = notional
            .raw
            .checked_mul(FIXED_SCALE)
            .and_then(|value| value.checked_div(price.raw))
            .ok_or_else(|| "quantity calculation overflowed".to_owned())?;
        Ok(Self { raw })
    }

    fn price_for_notional_quantity(
        notional: Self,
        quantity: Self,
        message: &'static str,
    ) -> Result<Self, String> {
        if quantity.raw <= 0 {
            return Err(message.to_owned());
        }
        let raw = notional
            .raw
            .checked_mul(FIXED_SCALE)
            .and_then(|value| value.checked_div(quantity.raw))
            .ok_or_else(|| message.to_owned())?;
        Ok(Self { raw })
    }

    fn checked_add(self, other: Self, message: &'static str) -> Result<Self, String> {
        let raw = self
            .raw
            .checked_add(other.raw)
            .ok_or_else(|| message.to_owned())?;
        Ok(Self { raw })
    }

    fn checked_sub(self, other: Self, message: &'static str) -> Result<Self, String> {
        let raw = self
            .raw
            .checked_sub(other.raw)
            .ok_or_else(|| message.to_owned())?;
        Ok(Self { raw })
    }

    fn checked_mul_i128(self, value: i128, message: &'static str) -> Result<Self, String> {
        let raw = self
            .raw
            .checked_mul(value)
            .ok_or_else(|| message.to_owned())?;
        Ok(Self { raw })
    }

    fn checked_div_i128(self, value: i128, message: &'static str) -> Result<Self, String> {
        if value == 0 {
            return Err(message.to_owned());
        }
        let raw = self
            .raw
            .checked_div(value)
            .ok_or_else(|| message.to_owned())?;
        Ok(Self { raw })
    }

    fn checked_mul_decimal(self, other: Self, message: &'static str) -> Result<Self, String> {
        let raw = self
            .raw
            .checked_mul(other.raw)
            .and_then(|value| value.checked_div(FIXED_SCALE))
            .ok_or_else(|| message.to_owned())?;
        Ok(Self { raw })
    }

    fn usd_from_bps(notional: Self, bps: i128) -> Result<Self, String> {
        let raw = notional
            .raw
            .checked_mul(bps)
            .and_then(|value| value.checked_div(10_000))
            .ok_or_else(|| "basis USD calculation overflowed".to_owned())?;
        Ok(Self { raw })
    }

    fn usd_from_bps_decimal(notional: Self, bps: Self) -> Result<Self, String> {
        notional
            .checked_mul_decimal(bps, "basis USD calculation overflowed")?
            .checked_div_i128(10_000, "basis USD calculation overflowed")
    }

    fn usd_from_rate(notional: Self, rate: Self) -> Result<Self, String> {
        notional.checked_mul_decimal(rate, "funding USD calculation overflowed")
    }

    fn bps_from_i128(value: i128) -> Result<Self, String> {
        let raw = value
            .checked_mul(FIXED_SCALE)
            .ok_or_else(|| "bps decimal calculation overflowed".to_owned())?;
        Ok(Self { raw })
    }

    fn bps_from_rate_decimal(rate: Self) -> Result<Self, String> {
        let raw = rate
            .raw
            .checked_mul(10_000)
            .ok_or_else(|| "funding bps calculation overflowed".to_owned())?;
        Ok(Self { raw })
    }

    fn format_trimmed(self) -> String {
        let negative = self.raw < 0;
        let raw = if negative {
            self.raw
                .checked_neg()
                .expect("fixed decimal raw value should never be i128::MIN")
        } else {
            self.raw
        };
        let integer = raw / FIXED_SCALE;
        let fraction = raw % FIXED_SCALE;
        let sign = if negative { "-" } else { "" };
        if fraction == 0 {
            return format!("{sign}{integer}");
        }
        let mut fraction_text = format!("{fraction:0width$}", width = FIXED_SCALE_DIGITS);
        while fraction_text.ends_with('0') {
            fraction_text.pop();
        }
        format!("{sign}{integer}.{fraction_text}")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DepthSide {
    Ask,
    Bid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedDepthLevel {
    price: FixedDecimal,
    size: FixedDecimal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DepthExecution {
    depth_usd: FixedDecimal,
    covered_notional_usd: FixedDecimal,
    vwap: Option<FixedDecimal>,
    quantity: Option<FixedDecimal>,
    levels_used: usize,
    model: &'static str,
}

fn depth_levels_or_top_of_book(
    levels: &[SignalDepthLevel],
    fallback_price: FixedDecimal,
    fallback_size: Option<&str>,
    fallback_size_field: &'static str,
) -> Result<(Vec<ParsedDepthLevel>, &'static str), String> {
    if !levels.is_empty() {
        let parsed = levels
            .iter()
            .map(|level| {
                let price = FixedDecimal::parse_non_negative("depth_price", &level.price)?;
                if price.raw <= 0 {
                    return Err("depth_price must be greater than zero".to_owned());
                }
                let size = FixedDecimal::parse_non_negative_depth("depth_size", &level.size)?;
                Ok(ParsedDepthLevel { price, size })
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok((parsed, "order_book_vwap"));
    }

    let size = fallback_size
        .ok_or_else(|| format!("{fallback_size_field} is required for fail-closed depth checks"))
        .and_then(|value| FixedDecimal::parse_non_negative_depth(fallback_size_field, value))?;
    Ok((
        vec![ParsedDepthLevel {
            price: fallback_price,
            size,
        }],
        "top_of_book_as_single_level",
    ))
}

fn depth_execution_for_notional(
    side: DepthSide,
    mut levels: Vec<ParsedDepthLevel>,
    notional: FixedDecimal,
    model: &'static str,
) -> Result<DepthExecution, String> {
    match side {
        DepthSide::Ask => levels.sort_by(|left, right| left.price.cmp(&right.price)),
        DepthSide::Bid => levels.sort_by(|left, right| right.price.cmp(&left.price)),
    }

    let mut depth_usd = FixedDecimal { raw: 0 };
    for level in &levels {
        let level_notional = level
            .price
            .checked_mul_decimal(level.size, "depth notional calculation overflowed")?;
        depth_usd = depth_usd.checked_add(level_notional, "depth notional sum overflowed")?;
    }

    let mut remaining = notional;
    let mut covered = FixedDecimal { raw: 0 };
    let mut quantity = FixedDecimal { raw: 0 };
    let mut levels_used = 0_usize;
    for level in levels {
        if remaining.raw <= 0 {
            break;
        }
        let level_notional = level
            .price
            .checked_mul_decimal(level.size, "depth notional calculation overflowed")?;
        if level_notional.raw <= 0 {
            continue;
        }
        let consumed_notional = if level_notional.raw >= remaining.raw {
            remaining
        } else {
            level_notional
        };
        let consumed_quantity =
            FixedDecimal::quantity_for_notional(consumed_notional, level.price)?;
        quantity = quantity.checked_add(consumed_quantity, "depth quantity sum overflowed")?;
        covered = covered.checked_add(consumed_notional, "depth covered notional overflowed")?;
        remaining.raw = remaining
            .raw
            .checked_sub(consumed_notional.raw)
            .ok_or_else(|| "depth remaining notional underflowed".to_owned())?;
        levels_used += 1;
    }

    let vwap = if quantity.raw > 0 {
        Some(FixedDecimal::price_for_notional_quantity(
            covered,
            quantity,
            "depth VWAP calculation overflowed",
        )?)
    } else {
        None
    };

    Ok(DepthExecution {
        depth_usd,
        covered_notional_usd: covered,
        vwap,
        quantity: (quantity.raw > 0).then_some(quantity),
        levels_used,
        model,
    })
}

fn depth_execution_price(execution: &DepthExecution, fallback_price: FixedDecimal) -> FixedDecimal {
    execution.vwap.unwrap_or(fallback_price)
}

fn ask_price_impact_bps(vwap: FixedDecimal, best_ask: FixedDecimal) -> Result<i128, String> {
    if best_ask.raw <= 0 {
        return Err("best ask price must be greater than zero".to_owned());
    }
    vwap.raw
        .checked_sub(best_ask.raw)
        .and_then(|value| value.checked_mul(10_000))
        .and_then(|value| value.checked_div(best_ask.raw))
        .ok_or_else(|| "ask price impact calculation overflowed".to_owned())
}

fn bid_price_impact_bps(vwap: FixedDecimal, best_bid: FixedDecimal) -> Result<i128, String> {
    if best_bid.raw <= 0 {
        return Err("best bid price must be greater than zero".to_owned());
    }
    best_bid
        .raw
        .checked_sub(vwap.raw)
        .and_then(|value| value.checked_mul(10_000))
        .and_then(|value| value.checked_div(best_bid.raw))
        .ok_or_else(|| "bid price impact calculation overflowed".to_owned())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BasisBookTickerInput {
    event_id: String,
    best_bid: FixedDecimal,
    best_ask: FixedDecimal,
    bid_size: FixedDecimal,
    ask_size: FixedDecimal,
    bid_depth: Vec<SignalDepthLevel>,
    ask_depth: Vec<SignalDepthLevel>,
    is_stale: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BasisPremiumIndexInput {
    event_id: String,
    mark_price: FixedDecimal,
    index_price: FixedDecimal,
    last_funding_rate: String,
    funding_interval_hours: Option<String>,
    next_funding_time_ms: String,
    is_stale: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BasisOpportunity {
    spot: BasisBookTickerInput,
    perp: BasisBookTickerInput,
    premium: BasisPremiumIndexInput,
    signal: SpotPerpBasisSignal,
}

fn latest_basis_book_ticker(
    context: &dyn StrategyReadContext,
    venue_id: &str,
    instrument_id: &str,
    basis_role: &str,
    venue_family_label: &str,
) -> Result<BasisBookTickerInput, String> {
    let event = context
        .snapshot()
        .market_events()
        .iter()
        .rev()
        .find(|event| is_basis_book_ticker_event(event, venue_id, instrument_id, basis_role))
        .ok_or_else(|| {
            format!(
                "missing {venue_family_label} {basis_role} BookTicker event for venue {venue_id} instrument {instrument_id}"
            )
        })?;

    Ok(BasisBookTickerInput {
        event_id: event.event_id.as_str().to_owned(),
        best_bid: required_payload_fixed_decimal(event, "best_bid")?,
        best_ask: required_payload_fixed_decimal(event, "best_ask")?,
        bid_size: required_payload_fixed_decimal(event, "bid_size")?,
        ask_size: required_payload_fixed_decimal(event, "ask_size")?,
        bid_depth: payload_depth_levels(event, "bid_depth_levels")?,
        ask_depth: payload_depth_levels(event, "ask_depth_levels")?,
        is_stale: payload_string(event, "risk_reason_code") == Some("DATA_STALE"),
    })
}

fn latest_basis_premium_index(
    context: &dyn StrategyReadContext,
    venue_id: &str,
    instrument_id: &str,
    venue_family_label: &str,
) -> Result<BasisPremiumIndexInput, String> {
    let event = context
        .snapshot()
        .market_events()
        .iter()
        .rev()
        .find(|event| is_basis_premium_index_event(event, venue_id, instrument_id))
        .ok_or_else(|| {
            format!(
                "missing {venue_family_label} PerpPremiumIndex event for venue {venue_id} instrument {instrument_id}"
            )
        })?;

    Ok(BasisPremiumIndexInput {
        event_id: event.event_id.as_str().to_owned(),
        mark_price: required_payload_fixed_decimal(event, "mark_price")?,
        index_price: required_payload_fixed_decimal(event, "index_price")?,
        last_funding_rate: payload_string(event, "last_funding_rate")
            .ok_or_else(|| {
                format!(
                    "premium index event {} is missing string payload field `last_funding_rate`",
                    event.event_id.as_str()
                )
            })?
            .to_owned(),
        funding_interval_hours: match event.payload.get("funding_interval_hours") {
            Some(JsonValue::Number(value)) => Some(value.as_str().to_owned()),
            Some(JsonValue::String(value)) => Some(value.to_owned()),
            _ => None,
        },
        next_funding_time_ms: match event.payload.get("next_funding_time_ms") {
            Some(JsonValue::Number(value)) => value.as_str().to_owned(),
            Some(JsonValue::String(value)) => value.to_owned(),
            _ => {
                return Err(format!(
                    "premium index event {} is missing scalar payload field `next_funding_time_ms`",
                    event.event_id.as_str()
                ));
            }
        },
        is_stale: payload_string(event, "risk_reason_code") == Some("DATA_STALE"),
    })
}

fn is_basis_book_ticker_event(
    event: &NormalizedEvent,
    venue_id: &str,
    instrument_id: &str,
    basis_role: &str,
) -> bool {
    event.event_type == NormalizedEventType::NormalizedMarketDataEvent
        && nullable_identifier_matches(&event.venue_id, venue_id)
        && nullable_identifier_matches(&event.instrument_id, instrument_id)
        && payload_string(event, "kind").is_some_and(|kind| kind == "BookTicker")
        && payload_string(event, "basis_role").is_some_and(|role| role == basis_role)
}

fn is_basis_premium_index_event(
    event: &NormalizedEvent,
    venue_id: &str,
    instrument_id: &str,
) -> bool {
    event.event_type == NormalizedEventType::NormalizedMarketDataEvent
        && nullable_identifier_matches(&event.venue_id, venue_id)
        && nullable_identifier_matches(&event.instrument_id, instrument_id)
        && payload_string(event, "kind").is_some_and(|kind| kind == "PerpPremiumIndex")
}

fn required_payload_fixed_decimal(
    event: &NormalizedEvent,
    field: &'static str,
) -> Result<FixedDecimal, String> {
    let value = required_payload_decimal_string(event, field)?;
    FixedDecimal::parse_non_negative(field, &value)
}

fn gross_basis_bps_decimal(
    perp_bid: FixedDecimal,
    spot_ask: FixedDecimal,
) -> Result<FixedDecimal, String> {
    if spot_ask.raw <= 0 {
        return Err("spot ask price must be greater than zero".to_owned());
    }
    let raw = perp_bid
        .raw
        .checked_sub(spot_ask.raw)
        .and_then(|value| value.checked_mul(10_000))
        .and_then(|value| value.checked_mul(FIXED_SCALE))
        .and_then(|value| value.checked_div(spot_ask.raw))
        .ok_or_else(|| "basis bps calculation overflowed".to_owned())?;
    Ok(FixedDecimal { raw })
}

fn price_divergence_bps(left: FixedDecimal, right: FixedDecimal) -> Result<i128, String> {
    if left.raw <= 0 || right.raw <= 0 {
        return Err("entry prices must be greater than zero".to_owned());
    }
    let diff = left
        .raw
        .checked_sub(right.raw)
        .and_then(|value| value.checked_abs())
        .ok_or_else(|| "entry price divergence calculation overflowed".to_owned())?;
    let midpoint = left
        .raw
        .checked_add(right.raw)
        .and_then(|value| value.checked_div(2))
        .ok_or_else(|| "entry price midpoint calculation overflowed".to_owned())?;
    diff.checked_mul(10_000)
        .and_then(|value| value.checked_div(midpoint))
        .ok_or_else(|| "entry price divergence calculation overflowed".to_owned())
}

fn signed_entry_price_edge_bps(
    long_price: FixedDecimal,
    short_price: FixedDecimal,
) -> Result<FixedDecimal, String> {
    if long_price.raw <= 0 || short_price.raw <= 0 {
        return Err("entry prices must be greater than zero".to_owned());
    }
    let midpoint = long_price
        .raw
        .checked_add(short_price.raw)
        .and_then(|value| value.checked_div(2))
        .ok_or_else(|| "entry price midpoint calculation overflowed".to_owned())?;
    if midpoint <= 0 {
        return Err("entry price midpoint must be greater than zero".to_owned());
    }
    let raw = short_price
        .raw
        .checked_sub(long_price.raw)
        .and_then(|value| value.checked_mul(10_000))
        .and_then(|value| value.checked_mul(FIXED_SCALE))
        .and_then(|value| value.checked_div(midpoint))
        .ok_or_else(|| "entry price edge calculation overflowed".to_owned())?;
    Ok(FixedDecimal { raw })
}

fn mark_index_divergence_bps(
    mark_price: FixedDecimal,
    index_price: FixedDecimal,
) -> Result<i128, String> {
    price_divergence_bps(mark_price, index_price)
}

fn tradable_close_basis_bps(
    perp_ask: FixedDecimal,
    spot_bid: FixedDecimal,
) -> Result<i128, String> {
    if spot_bid.raw <= 0 {
        return Err("spot bid price must be greater than zero".to_owned());
    }
    perp_ask
        .raw
        .checked_sub(spot_bid.raw)
        .and_then(|value| value.checked_mul(10_000))
        .and_then(|value| value.checked_div(spot_bid.raw))
        .ok_or_else(|| "close basis bps calculation overflowed".to_owned())
}

fn parse_positive_u64(field: &'static str, value: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("field `{field}` must be a positive integer"))?;
    if parsed == 0 {
        return Err(format!("field `{field}` must be greater than zero"));
    }
    Ok(parsed)
}

fn normalize_funding_rate_to_8h(
    rate: FixedDecimal,
    interval_hours: u64,
) -> Result<FixedDecimal, String> {
    if interval_hours == 0 {
        return Err("funding interval must be greater than zero".to_owned());
    }
    let raw = rate
        .raw
        .checked_mul(8)
        .and_then(|value| value.checked_div(i128::from(interval_hours)))
        .ok_or_else(|| "funding interval normalization overflowed".to_owned())?;
    Ok(FixedDecimal { raw })
}

fn nullable_identifier_matches(value: &Option<Option<Identifier>>, expected: &str) -> bool {
    matches!(value, Some(Some(identifier)) if identifier.as_str() == expected)
}

fn required_payload_decimal_string(
    event: &NormalizedEvent,
    field: &'static str,
) -> Result<String, String> {
    let value = payload_string(event, field).ok_or_else(|| {
        format!(
            "market quote event {} is missing string payload field `{field}`",
            event.event_id.as_str()
        )
    })?;
    validate_market_decimal(field, value)?;
    Ok(value.to_owned())
}

fn payload_string<'a>(event: &'a NormalizedEvent, field: &'static str) -> Option<&'a str> {
    match event.payload.get(field)? {
        JsonValue::String(value) => Some(value.as_str()),
        _ => None,
    }
}

fn payload_depth_levels(
    event: &NormalizedEvent,
    field: &'static str,
) -> Result<Vec<SignalDepthLevel>, String> {
    let Some(value) = event.payload.get(field) else {
        return Ok(Vec::new());
    };
    let JsonValue::Array(values) = value else {
        return Err(format!(
            "market quote event {} payload field `{field}` must be an array",
            event.event_id.as_str()
        ));
    };
    values
        .iter()
        .map(|value| {
            let JsonValue::Object(fields) = value else {
                return Err(format!(
                    "market quote event {} payload field `{field}` contains a non-object level",
                    event.event_id.as_str()
                ));
            };
            let Some(JsonValue::String(price)) = fields.get("price") else {
                return Err(format!(
                    "market quote event {} payload field `{field}` level is missing string `price`",
                    event.event_id.as_str()
                ));
            };
            let Some(JsonValue::String(size)) = fields.get("size") else {
                return Err(format!(
                    "market quote event {} payload field `{field}` level is missing string `size`",
                    event.event_id.as_str()
                ));
            };
            Ok(SignalDepthLevel {
                price: price.clone(),
                size: size.clone(),
            })
        })
        .collect()
}

fn validate_market_decimal(field: &'static str, value: &str) -> Result<(), String> {
    validate_decimal_string(field, value, false)
}

fn validate_signed_market_decimal(field: &'static str, value: &str) -> Result<(), String> {
    validate_decimal_string(field, value, true)
}

fn validate_decimal_string(
    field: &'static str,
    value: &str,
    allow_negative: bool,
) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("market quote field `{field}` cannot be empty"));
    }

    let bytes = value.as_bytes();
    let mut start_index = 0_usize;
    if bytes[0] == b'-' {
        if !allow_negative {
            return Err(format!(
                "market quote field `{field}` must be a non-negative decimal string"
            ));
        }
        start_index = 1;
        if start_index == bytes.len() {
            return Err(format!(
                "market quote field `{field}` must be a signed decimal string"
            ));
        }
    }

    let mut dot_seen = false;
    let mut digits_seen = false;
    for byte in &bytes[start_index..] {
        match *byte {
            b'0'..=b'9' => digits_seen = true,
            b'.' if !dot_seen => dot_seen = true,
            _ => {
                let expected = if allow_negative {
                    "signed decimal string"
                } else {
                    "non-negative decimal string"
                };
                return Err(format!("market quote field `{field}` must be a {expected}"));
            }
        }
    }

    let unsigned = &value[start_index..];
    if !digits_seen || unsigned.starts_with('.') || unsigned.ends_with('.') {
        return Err(format!(
            "market quote field `{field}` must include integer and fractional digits when decimal point is present"
        ));
    }
    Ok(())
}

fn source_event_refs(context: &dyn StrategyReadContext) -> Vec<String> {
    context
        .snapshot()
        .source_event_refs()
        .iter()
        .map(|event_ref| event_ref.as_str().to_owned())
        .collect()
}

fn funding_client_order_id(venue_id: &str, role_tag: &str, seed: &str) -> String {
    if venue_id.to_ascii_uppercase().contains("HYPERLIQUID") {
        let high = stable_client_order_hash(seed);
        let low = stable_client_order_hash(&format!("{seed}:hyperliquid-cloid"));
        return format!("0x{high:016x}{low:016x}");
    }

    format!("rvf{role_tag}{:016x}", stable_client_order_hash(seed))
}

fn stable_client_order_hash(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
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

#[cfg(test)]
mod tests {
    use super::*;
    use arb_strategy_api::{
        candidate_to_canonical_json, normalized_event_from_json_strict,
        portfolio_state_from_json_strict, venue_capability_from_json_strict,
        CandidateTransitionOutput, ExecutionMode, FixedTimeSource, Identifier, NormalizedEvent,
        PortfolioState, ReadOnlySnapshot, StrategyConfigReader, StrategyConfigSnapshot,
        StrategyInput, StrategyTimeSource, VenueCapabilityDescriptor, VenueCapabilityReader,
        VenueCapabilitySnapshot,
    };
    use std::fs;
    use std::path::PathBuf;

    const EXPECTED_CANDIDATE: &str = include_str!(
        "../../../fixtures/replay/strategy_smoke/expected/sample_spot_candidate.canonical.json"
    );
    const EXPECTED_CANDIDATE_TRANSITIONS: &str = include_str!(
        "../../../fixtures/replay/strategy_smoke/expected/candidate_transitions.jsonl"
    );
    const STRATEGY_SMOKE_CONFIG_VERSION: &str = "arb-config-v1";
    const STRATEGY_SMOKE_CONFIG_HASH: &str =
        "sha256:59a151604355ed343e67508b921be858ec2064e62c18a8c19d1f9375bf3c237d";

    #[test]
    fn sample_strategy_outputs_golden_candidate() {
        let strategy = SampleSpotStrategy::new().expect("strategy");
        let context = test_context(true, true);

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        let candidate = evaluation.candidate().expect("candidate");
        assert_eq!(
            candidate_to_canonical_json(candidate),
            EXPECTED_CANDIDATE.trim()
        );
        assert!(evaluation.rejection().is_none());
        assert_eq!(evaluation.diagnostics()[0].code(), "SAMPLE_SPOT_CANDIDATE");
        assert!(evaluation.diagnostics()[0]
            .detail()
            .contains("bid 43187.40, ask 43188.10, last 43187.50"));
        assert_eq!(
            payload_constraint(candidate, "reference_best_bid"),
            Some("43187.40")
        );
        assert_eq!(
            payload_constraint(candidate, "reference_best_ask"),
            Some("43188.10")
        );
        assert_eq!(
            payload_constraint(candidate, "reference_last_price"),
            Some("43187.50")
        );
    }

    #[test]
    fn sample_strategy_output_is_stable_for_fixed_input() {
        let strategy = SampleSpotStrategy::new().expect("strategy");
        let context = test_context(true, true);

        let first = strategy.evaluate(&context).expect("first evaluation");
        let second = strategy.evaluate(&context).expect("second evaluation");

        assert_eq!(
            candidate_to_canonical_json(first.candidate().expect("first candidate")),
            candidate_to_canonical_json(second.candidate().expect("second candidate"))
        );
    }

    #[test]
    fn sample_strategy_replay_fixture_is_deterministic() {
        let strategy = SampleSpotStrategy::new().expect("strategy");
        let first_context = strategy_replay_context();
        let second_context = strategy_replay_context();

        let first = strategy
            .evaluate(&first_context)
            .expect("first replay evaluation");
        let second = strategy
            .evaluate(&second_context)
            .expect("second replay evaluation");
        let first_candidate = first.candidate().expect("first candidate");
        let second_candidate = second.candidate().expect("second candidate");
        let expected = EXPECTED_CANDIDATE_TRANSITIONS
            .lines()
            .next()
            .expect("candidate transitions fixture line");

        assert_eq!(EXPECTED_CANDIDATE.trim(), expected);
        assert_eq!(candidate_to_canonical_json(first_candidate), expected);
        assert_eq!(
            candidate_to_canonical_json(first_candidate),
            candidate_to_canonical_json(second_candidate)
        );
        assert_eq!(first_context.snapshot().market_events().len(), 1);
        assert_eq!(
            first_context.config().config_hash(),
            STRATEGY_SMOKE_CONFIG_HASH
        );

        let manifest = fs::read_to_string(strategy_fixture_dir().join("strategy_manifest.yaml"))
            .expect("strategy manifest should be readable");
        assert!(manifest.contains(SAMPLE_STRATEGY_ID));
        assert!(manifest.contains(SAMPLE_STRATEGY_VERSION));
        assert!(manifest.contains(SAMPLE_CODE_VERSION));
    }

    #[test]
    fn sample_strategy_rejects_when_spot_capability_is_missing() {
        let strategy = SampleSpotStrategy::new().expect("strategy");
        let context = test_context(false, true);

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        let rejection = evaluation.rejection().expect("rejection");
        assert_eq!(
            rejection.reason().as_str(),
            StrategyRejectReason::VenueCapabilityMissing.as_str()
        );
        assert_eq!(rejection.strategy_version(), SAMPLE_STRATEGY_VERSION);
    }

    #[test]
    fn sample_strategy_rejects_when_read_surface_is_missing() {
        let strategy = SampleSpotStrategy::new().expect("strategy");
        let context = test_context(true, false);

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        assert_eq!(
            evaluation.rejection().expect("rejection").reason().as_str(),
            StrategyRejectReason::VenueCapabilityMissing.as_str()
        );
    }

    #[test]
    fn sample_strategy_rejects_when_market_quote_is_missing() {
        let strategy = SampleSpotStrategy::new().expect("strategy");
        let context = test_context_with_market_events(true, true, Vec::new());

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        let rejection = evaluation.rejection().expect("rejection");
        assert_eq!(
            rejection.reason().as_str(),
            StrategyRejectReason::MissingData.as_str()
        );
        assert!(rejection
            .detail()
            .expect("detail")
            .contains("missing normalized MarketQuote event"));
    }

    #[test]
    fn spot_perp_basis_strategy_outputs_candidate_when_net_basis_survives_costs() {
        let strategy = SpotPerpBasisStrategy::new().expect("strategy");
        let context = basis_test_context(vec![
            basis_book_event(
                "spot",
                BINANCE_BASIS_SPOT_VENUE_ID,
                BINANCE_BASIS_SPOT_INSTRUMENT_ID,
                "Spot",
                "99.90",
                "100.00",
                "CHECK_PASSED",
            ),
            basis_book_event(
                "perp",
                BINANCE_BASIS_PERP_VENUE_ID,
                BINANCE_BASIS_PERP_INSTRUMENT_ID,
                "Perp",
                "101.00",
                "101.10",
                "CHECK_PASSED",
            ),
            basis_premium_event("0.00010000", "CHECK_PASSED"),
        ]);

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        let candidate = evaluation.candidate().expect("candidate");
        assert_eq!(
            candidate.transition_id.as_str(),
            BINANCE_BASIS_TRANSITION_ID
        );
        assert_eq!(candidate.legs.len(), 2);
        assert_eq!(
            candidate.expected_economics.expected_profit_bps.as_str(),
            "36.0000505"
        );
        assert_eq!(
            candidate
                .holding_period
                .exit_policy_ref
                .as_ref()
                .expect("exit policy ref")
                .as_str(),
            DEFAULT_BASIS_EXIT_POLICY_REF
        );
        assert_eq!(
            candidate.expected_economics.expected_profit_usd.as_str(),
            "0.72000101"
        );
        assert_eq!(
            candidate.expected_economics.fee_estimate_usd.as_str(),
            "0.24"
        );
        assert_eq!(
            candidate.expected_economics.slippage_estimate_usd.as_str(),
            "0.05"
        );
        assert_eq!(
            payload_constraint(candidate, "net_basis_bps"),
            Some("35.5000505")
        );
        assert_eq!(
            payload_constraint(candidate, "reference_ask_size"),
            Some("2")
        );
        assert!(candidate.risk_flags.is_empty());
        assert!(candidate
            .failure_modes
            .iter()
            .all(|mode| mode.as_str() != "UnknownState"));
        assert_eq!(
            candidate
                .funding_impact
                .as_ref()
                .and_then(|impact| impact.impact_usd.as_ref())
                .map(|value| value.as_str()),
            Some("0.01")
        );
        assert_eq!(
            candidate
                .liquidity_impact
                .as_ref()
                .and_then(|impact| impact.impact_usd.as_ref())
                .map(|value| value.as_str()),
            Some("0")
        );
        assert!(evaluation.rejection().is_none());
        assert_eq!(
            evaluation.diagnostics()[0].code(),
            "SPOT_PERP_BASIS_CANDIDATE"
        );
    }

    #[test]
    fn cross_exchange_funding_strategy_outputs_candidate() {
        let strategy = cross_exchange_funding_arb_strategy().expect("strategy");
        let context = basis_test_context(cross_exchange_funding_events(
            "0.00010000",
            "0.00600000",
            "8",
            "8",
        ));

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        let candidate = evaluation.candidate().expect("candidate");
        assert_eq!(
            candidate.transition_id.as_str(),
            CROSS_EXCHANGE_FUNDING_ARB_TRANSITION_ID
        );
        assert_eq!(candidate.legs.len(), 2);
        assert_eq!(
            candidate.legs[0].side.as_ref().expect("side").as_str(),
            "Long"
        );
        assert_eq!(
            candidate.legs[1].side.as_ref().expect("side").as_str(),
            "Short"
        );
        assert_eq!(
            leg_constraint(candidate, 0, "basis_leg_role"),
            Some("perp_long")
        );
        assert_eq!(
            leg_constraint(candidate, 1, "basis_leg_role"),
            Some("perp_short")
        );
        let long_client_order_id =
            leg_constraint(candidate, 0, "client_order_id").expect("long client order id");
        let short_client_order_id =
            leg_constraint(candidate, 1, "client_order_id").expect("short client order id");
        assert!(long_client_order_id.starts_with("rvfL"));
        assert!(short_client_order_id.starts_with("rvfS"));
        assert_ne!(long_client_order_id, short_client_order_id);
        assert!(long_client_order_id.len() <= 32);
        assert!(short_client_order_id.len() <= 32);
        assert!(long_client_order_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric()));
        assert!(short_client_order_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric()));
        assert_eq!(
            candidate.expected_economics.expected_profit_bps.as_str(),
            "17.5"
        );
        assert_eq!(
            candidate.expected_economics.expected_profit_usd.as_str(),
            "0.35"
        );
        assert!(candidate.margin_impact.is_some());
        assert!(candidate.risk_flags.is_empty());
    }

    #[test]
    fn funding_client_order_id_uses_venue_safe_shapes() {
        let okx_id = funding_client_order_id("venue:OKX-SWAP", "S", "seed:okx");
        assert!(okx_id.starts_with("rvfS"));
        assert!(okx_id.len() <= 32);
        assert!(okx_id.bytes().all(|byte| byte.is_ascii_alphanumeric()));

        let hyperliquid_id =
            funding_client_order_id("venue:HYPERLIQUID-PERP", "L", "seed:hyperliquid");
        assert!(hyperliquid_id.starts_with("0x"));
        assert_eq!(hyperliquid_id.len(), 34);
        assert!(hyperliquid_id[2..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn cross_exchange_funding_strategy_normalizes_interval_mismatch() {
        let strategy = cross_exchange_funding_arb_strategy().expect("strategy");
        let context = basis_test_context(cross_exchange_funding_events(
            "0.00010000",
            "0.00090000",
            "8",
            "1",
        ));

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        let candidate = evaluation.candidate().expect("candidate");
        assert_eq!(
            candidate.expected_economics.expected_profit_bps.as_str(),
            "23.5"
        );
    }

    #[test]
    fn cross_exchange_funding_strategy_rejects_mark_index_divergence() {
        let strategy = cross_exchange_funding_arb_strategy().expect("strategy");
        let mut events = cross_exchange_funding_events("0.00010000", "0.00600000", "8", "8");
        events[2].payload.insert(
            "mark_price".to_owned(),
            JsonValue::String("120.00".to_owned()),
        );
        let context = basis_test_context(events);

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        let rejection = evaluation.rejection().expect("rejection");
        assert_eq!(
            rejection.reason().as_str(),
            StrategyRejectReason::NoCandidate.as_str()
        );
        assert!(rejection
            .detail()
            .expect("detail")
            .contains("mark/index divergence"));
    }

    #[test]
    fn cross_exchange_funding_strategy_rejects_missing_capability() {
        let strategy = cross_exchange_funding_arb_strategy().expect("strategy");
        let mut context = basis_test_context(cross_exchange_funding_events(
            "0.00010000",
            "0.00600000",
            "8",
            "8",
        ));
        context.capabilities.has_basis_funding = false;

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        assert_eq!(
            evaluation.rejection().expect("rejection").reason().as_str(),
            StrategyRejectReason::VenueCapabilityMissing.as_str()
        );
    }

    #[test]
    fn spot_perp_basis_default_factories_keep_binance_metadata() {
        let direct = SpotPerpBasisStrategy::new().expect("direct strategy");
        let binance = binance_spot_perp_basis_strategy().expect("binance strategy");
        let default_alias = spot_perp_basis_strategy().expect("default strategy");

        for strategy in [&direct, &binance, &default_alias] {
            assert_eq!(strategy.metadata().strategy_id(), BINANCE_BASIS_STRATEGY_ID);
            assert_eq!(
                strategy.metadata().strategy_version(),
                SPOT_PERP_BASIS_STRATEGY_VERSION
            );
            assert_eq!(
                strategy.metadata().code_version(),
                BINANCE_BASIS_CODE_VERSION
            );
            assert_eq!(
                strategy.config(),
                &SpotPerpBasisStrategyConfig::binance_btcusdt()
            );
        }
    }

    #[test]
    fn spot_perp_basis_strategy_rejects_when_costs_remove_opportunity() {
        let strategy = SpotPerpBasisStrategy::new().expect("strategy");
        let context = basis_test_context(vec![
            basis_book_event(
                "spot",
                BINANCE_BASIS_SPOT_VENUE_ID,
                BINANCE_BASIS_SPOT_INSTRUMENT_ID,
                "Spot",
                "99.90",
                "100.00",
                "CHECK_PASSED",
            ),
            basis_book_event(
                "perp",
                BINANCE_BASIS_PERP_VENUE_ID,
                BINANCE_BASIS_PERP_INSTRUMENT_ID,
                "Perp",
                "100.15",
                "100.20",
                "CHECK_PASSED",
            ),
            basis_premium_event("0.00010000", "CHECK_PASSED"),
        ]);

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        let rejection = evaluation.rejection().expect("rejection");
        assert_eq!(
            rejection.reason().as_str(),
            StrategyRejectReason::NoCandidate.as_str()
        );
        assert!(rejection
            .detail()
            .expect("detail")
            .contains("below minimum"));
    }

    #[test]
    fn spot_perp_basis_strategy_rejects_stale_public_data() {
        let strategy = SpotPerpBasisStrategy::new().expect("strategy");
        let context = basis_test_context(vec![
            basis_book_event(
                "spot",
                BINANCE_BASIS_SPOT_VENUE_ID,
                BINANCE_BASIS_SPOT_INSTRUMENT_ID,
                "Spot",
                "99.90",
                "100.00",
                "DATA_STALE",
            ),
            basis_book_event(
                "perp",
                BINANCE_BASIS_PERP_VENUE_ID,
                BINANCE_BASIS_PERP_INSTRUMENT_ID,
                "Perp",
                "101.00",
                "101.10",
                "CHECK_PASSED",
            ),
            basis_premium_event("0.00010000", "CHECK_PASSED"),
        ]);

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        assert_eq!(
            evaluation.rejection().expect("rejection").reason().as_str(),
            StrategyRejectReason::DataStale.as_str()
        );
    }

    #[test]
    fn bybit_spot_perp_basis_strategy_outputs_configured_candidate() {
        let strategy = bybit_spot_perp_basis_strategy().expect("strategy");
        let context = bybit_basis_test_context("CHECK_PASSED", "101.00", "101.10", "CHECK_PASSED");

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        let candidate = evaluation.candidate().expect("candidate");
        assert_eq!(candidate.transition_id.as_str(), BYBIT_BASIS_TRANSITION_ID);
        assert_eq!(candidate.strategy_id.as_str(), BYBIT_BASIS_STRATEGY_ID);
        assert_eq!(candidate.code_version.as_str(), BYBIT_BASIS_CODE_VERSION);
        assert_eq!(candidate.legs.len(), 2);
        assert_eq!(
            candidate.legs[0].leg_id.as_str(),
            "candleg:bybit-basis-buy-spot-btcusdt"
        );
        assert_eq!(
            candidate.legs[0]
                .venue_id
                .as_ref()
                .expect("spot venue")
                .as_str(),
            BYBIT_BASIS_SPOT_VENUE_ID
        );
        assert_eq!(
            candidate.legs[0]
                .instrument_id
                .as_ref()
                .expect("spot instrument")
                .as_str(),
            BYBIT_BASIS_SPOT_INSTRUMENT_ID
        );
        assert_eq!(
            candidate.legs[0]
                .account_id
                .as_ref()
                .expect("spot account")
                .as_str(),
            "acct:bybit-basis-readonly"
        );
        assert_eq!(
            leg_constraint(candidate, 0, "basis_leg_role"),
            Some("spot_buy")
        );
        assert_eq!(
            candidate.legs[1].leg_id.as_str(),
            "candleg:bybit-basis-short-linear-perp-btcusdt"
        );
        assert_eq!(
            candidate.legs[1]
                .venue_id
                .as_ref()
                .expect("perp venue")
                .as_str(),
            BYBIT_BASIS_PERP_VENUE_ID
        );
        assert_eq!(
            candidate.legs[1]
                .instrument_id
                .as_ref()
                .expect("perp instrument")
                .as_str(),
            BYBIT_BASIS_PERP_INSTRUMENT_ID
        );
        assert_eq!(
            candidate.legs[1]
                .account_id
                .as_ref()
                .expect("perp account")
                .as_str(),
            "acct:bybit-basis-readonly"
        );
        assert_eq!(
            leg_constraint(candidate, 1, "basis_leg_role"),
            Some("perp_short")
        );
        assert_eq!(
            candidate.expected_post_state_delta.position_deltas[0]
                .instrument_id
                .as_str(),
            BYBIT_BASIS_SPOT_INSTRUMENT_ID
        );
        assert_eq!(
            candidate.expected_post_state_delta.position_deltas[1]
                .instrument_id
                .as_str(),
            BYBIT_BASIS_PERP_INSTRUMENT_ID
        );
        assert!(evaluation.rejection().is_none());
    }

    #[test]
    fn okx_spot_swap_basis_strategy_outputs_configured_candidate() {
        let strategy = okx_spot_swap_basis_strategy().expect("strategy");
        let context = okx_basis_test_context("CHECK_PASSED", "101.00", "101.10", "CHECK_PASSED");

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        let candidate = evaluation.candidate().expect("candidate");
        assert_eq!(candidate.transition_id.as_str(), OKX_BASIS_TRANSITION_ID);
        assert_eq!(candidate.strategy_id.as_str(), OKX_BASIS_STRATEGY_ID);
        assert_eq!(candidate.code_version.as_str(), OKX_BASIS_CODE_VERSION);
        assert_eq!(candidate.legs.len(), 2);
        assert_eq!(
            candidate.legs[0].leg_id.as_str(),
            "candleg:okx-basis-buy-spot-btc-usdt"
        );
        assert_eq!(
            candidate.legs[0]
                .venue_id
                .as_ref()
                .expect("spot venue")
                .as_str(),
            OKX_BASIS_SPOT_VENUE_ID
        );
        assert_eq!(
            candidate.legs[0]
                .instrument_id
                .as_ref()
                .expect("spot instrument")
                .as_str(),
            OKX_BASIS_SPOT_INSTRUMENT_ID
        );
        assert_eq!(
            candidate.legs[0]
                .account_id
                .as_ref()
                .expect("spot account")
                .as_str(),
            "acct:okx-basis-readonly"
        );
        assert_eq!(
            leg_constraint(candidate, 0, "basis_leg_role"),
            Some("spot_buy")
        );
        assert_eq!(
            candidate.legs[1].leg_id.as_str(),
            "candleg:okx-basis-short-swap-btc-usdt"
        );
        assert_eq!(
            candidate.legs[1]
                .venue_id
                .as_ref()
                .expect("swap venue")
                .as_str(),
            OKX_BASIS_PERP_VENUE_ID
        );
        assert_eq!(
            candidate.legs[1]
                .instrument_id
                .as_ref()
                .expect("swap instrument")
                .as_str(),
            OKX_BASIS_PERP_INSTRUMENT_ID
        );
        assert_eq!(
            candidate.legs[1]
                .account_id
                .as_ref()
                .expect("swap account")
                .as_str(),
            "acct:okx-basis-readonly"
        );
        assert_eq!(
            leg_constraint(candidate, 1, "basis_leg_role"),
            Some("perp_short")
        );
        assert_eq!(
            candidate.expected_post_state_delta.position_deltas[0]
                .instrument_id
                .as_str(),
            OKX_BASIS_SPOT_INSTRUMENT_ID
        );
        assert_eq!(
            candidate.expected_post_state_delta.position_deltas[1]
                .instrument_id
                .as_str(),
            OKX_BASIS_PERP_INSTRUMENT_ID
        );
        assert!(evaluation.rejection().is_none());
    }

    #[test]
    fn bybit_spot_perp_basis_strategy_rejects_missing_capability() {
        let strategy = bybit_spot_perp_basis_strategy().expect("strategy");
        let mut context =
            bybit_basis_test_context("CHECK_PASSED", "101.00", "101.10", "CHECK_PASSED");
        context.capabilities.has_basis_spot = false;

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        let rejection = evaluation.rejection().expect("rejection");
        assert_eq!(
            rejection.reason().as_str(),
            StrategyRejectReason::VenueCapabilityMissing.as_str()
        );
        assert!(rejection
            .detail()
            .expect("detail")
            .contains("Bybit spot venue lacks ProvidesSpotMarkets capability"));
    }

    #[test]
    fn bybit_spot_perp_basis_strategy_rejects_missing_event() {
        let strategy = bybit_spot_perp_basis_strategy().expect("strategy");
        let context = basis_test_context(vec![
            basis_book_event(
                "bybit-perp",
                BYBIT_BASIS_PERP_VENUE_ID,
                BYBIT_BASIS_PERP_INSTRUMENT_ID,
                "Perp",
                "101.00",
                "101.10",
                "CHECK_PASSED",
            ),
            basis_premium_event_for(
                BYBIT_BASIS_PERP_VENUE_ID,
                BYBIT_BASIS_PERP_INSTRUMENT_ID,
                "0.00010000",
                "CHECK_PASSED",
            ),
        ]);

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        let rejection = evaluation.rejection().expect("rejection");
        assert_eq!(
            rejection.reason().as_str(),
            StrategyRejectReason::MissingData.as_str()
        );
        assert!(rejection
            .detail()
            .expect("detail")
            .contains("missing Bybit Spot BookTicker event"));
    }

    #[test]
    fn bybit_spot_perp_basis_strategy_rejects_stale_public_data() {
        let strategy = bybit_spot_perp_basis_strategy().expect("strategy");
        let context = bybit_basis_test_context("DATA_STALE", "101.00", "101.10", "CHECK_PASSED");

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        assert_eq!(
            evaluation.rejection().expect("rejection").reason().as_str(),
            StrategyRejectReason::DataStale.as_str()
        );
    }

    #[test]
    fn bybit_spot_perp_basis_strategy_rejects_when_net_basis_below_threshold() {
        let strategy = bybit_spot_perp_basis_strategy().expect("strategy");
        let context = bybit_basis_test_context("CHECK_PASSED", "100.15", "100.20", "CHECK_PASSED");

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        assert!(evaluation.candidate().is_none());
        let rejection = evaluation.rejection().expect("rejection");
        assert_eq!(
            rejection.reason().as_str(),
            StrategyRejectReason::NoCandidate.as_str()
        );
        assert!(rejection
            .detail()
            .expect("detail")
            .contains("below minimum"));
    }

    #[test]
    fn spot_perp_basis_signal_reuses_strategy_math_for_monitoring() {
        let signal = evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
            symbol: "BTCUSDT".to_owned(),
            spot_best_bid: "99.90".to_owned(),
            spot_best_ask: "100.00".to_owned(),
            spot_ask_size: Some("2.0".to_owned()),
            spot_ask_depth: Vec::new(),
            perp_best_bid: "101.00".to_owned(),
            perp_best_ask: "101.10".to_owned(),
            perp_bid_size: Some("1.5".to_owned()),
            perp_bid_depth: Vec::new(),
            last_funding_rate: "0.00010000".to_owned(),
            notional_usd: "100.00".to_owned(),
            spot_taker_fee_bps: "10".to_owned(),
            perp_taker_fee_bps: "5".to_owned(),
            slippage_buffer_bps: 5,
            min_net_bps: 5,
        })
        .expect("signal");

        assert!(signal.is_candidate);
        assert_eq!(signal.gross_bps, "100.000101");
        assert_eq!(signal.net_bps, "32.5000505");
        assert_eq!(signal.funding_bps, "1");
        assert_eq!(signal.expected_profit_bps, "33.0000505");
        assert_eq!(signal.expected_profit_usd, "0.66000101");
        assert_eq!(signal.funding_impact_usd, "0.01");
        assert_eq!(signal.spot_ask_depth_usd.as_deref(), Some("200"));
        assert_eq!(signal.perp_bid_depth_usd.as_deref(), Some("151.5"));
        assert_eq!(signal.quantity, "1");
    }

    #[test]
    fn spot_perp_basis_signal_accepts_high_precision_depth_sizes() {
        let signal = evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
            symbol: "BTCUSDT".to_owned(),
            spot_best_bid: "99.90".to_owned(),
            spot_best_ask: "100.00".to_owned(),
            spot_ask_size: Some("2.000000009".to_owned()),
            spot_ask_depth: Vec::new(),
            perp_best_bid: "101.00".to_owned(),
            perp_best_ask: "101.10".to_owned(),
            perp_bid_size: Some("1.500000009".to_owned()),
            perp_bid_depth: Vec::new(),
            last_funding_rate: "0.00010000".to_owned(),
            notional_usd: "100.00".to_owned(),
            spot_taker_fee_bps: "10".to_owned(),
            perp_taker_fee_bps: "5".to_owned(),
            slippage_buffer_bps: 5,
            min_net_bps: 5,
        })
        .expect("high precision depth sizes are truncated conservatively");

        assert!(signal.is_candidate);
        assert_eq!(signal.spot_ask_depth_usd.as_deref(), Some("200"));
        assert_eq!(signal.perp_bid_depth_usd.as_deref(), Some("151.5"));
    }

    #[test]
    fn spot_perp_basis_signal_rejects_insufficient_top_of_book_depth() {
        let signal = evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
            symbol: "BTCUSDT".to_owned(),
            spot_best_bid: "99.90".to_owned(),
            spot_best_ask: "100.00".to_owned(),
            spot_ask_size: Some("0.5".to_owned()),
            spot_ask_depth: Vec::new(),
            perp_best_bid: "101.00".to_owned(),
            perp_best_ask: "101.10".to_owned(),
            perp_bid_size: Some("1.5".to_owned()),
            perp_bid_depth: Vec::new(),
            last_funding_rate: "0.00010000".to_owned(),
            notional_usd: "100.00".to_owned(),
            spot_taker_fee_bps: "10".to_owned(),
            perp_taker_fee_bps: "5".to_owned(),
            slippage_buffer_bps: 5,
            min_net_bps: 5,
        })
        .expect("signal");

        assert!(!signal.is_candidate);
        assert_eq!(signal.spot_ask_depth_usd.as_deref(), Some("50"));
        assert!(signal
            .reason
            .as_deref()
            .expect("reason")
            .contains("insufficient order-book depth"));
    }

    #[test]
    fn spot_perp_basis_signal_uses_depth_vwap_for_candidate_math() {
        let signal = evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
            symbol: "BTCUSDT".to_owned(),
            spot_best_bid: "99.90".to_owned(),
            spot_best_ask: "100.00".to_owned(),
            spot_ask_size: Some("0.5".to_owned()),
            spot_ask_depth: vec![
                SignalDepthLevel {
                    price: "100.00".to_owned(),
                    size: "0.5".to_owned(),
                },
                SignalDepthLevel {
                    price: "100.50".to_owned(),
                    size: "1.0".to_owned(),
                },
            ],
            perp_best_bid: "102.00".to_owned(),
            perp_best_ask: "102.10".to_owned(),
            perp_bid_size: Some("0.5".to_owned()),
            perp_bid_depth: vec![
                SignalDepthLevel {
                    price: "102.00".to_owned(),
                    size: "0.5".to_owned(),
                },
                SignalDepthLevel {
                    price: "101.50".to_owned(),
                    size: "1.0".to_owned(),
                },
            ],
            last_funding_rate: "0.00010000".to_owned(),
            notional_usd: "100.00".to_owned(),
            spot_taker_fee_bps: "10".to_owned(),
            perp_taker_fee_bps: "5".to_owned(),
            slippage_buffer_bps: 5,
            min_net_bps: 5,
        })
        .expect("signal");

        assert!(signal.is_candidate);
        assert_eq!(signal.liquidity_model, "order_book_vwap");
        assert_eq!(signal.spot_ask_depth_usd.as_deref(), Some("150.5"));
        assert_eq!(signal.perp_bid_depth_usd.as_deref(), Some("152.5"));
        assert_eq!(signal.spot_ask_levels_used, 2);
        assert_eq!(signal.perp_bid_levels_used, 2);
        assert_eq!(signal.spot_ask_vwap.as_deref(), Some("100.24937734"));
        assert_eq!(signal.perp_bid_vwap.as_deref(), Some("101.75438603"));
        assert_eq!(signal.gross_bps, "150.12648755");
        assert_eq!(signal.net_bps, "57.56324377");
    }

    #[test]
    fn cross_exchange_funding_signal_outputs_candidate_when_spread_survives_costs() {
        let signal =
            evaluate_cross_exchange_funding_arb_signal(&cross_exchange_funding_signal_input())
                .expect("signal");

        assert!(signal.is_candidate);
        assert_eq!(signal.gross_funding_spread_bps, "59");
        assert_eq!(signal.total_cost_bps, "24");
        assert_eq!(signal.net_funding_bps, "17.5");
        assert_eq!(signal.entry_price_edge_bps, "4.99884826");
        assert_eq!(signal.expected_funding_usd, "0.35");
        assert_eq!(signal.fee_estimate_usd, "0.19");
        assert_eq!(signal.slippage_estimate_usd, "0.05");
        assert_eq!(signal.long_ask_depth_usd.as_deref(), Some("200"));
        assert_eq!(signal.short_bid_depth_usd.as_deref(), Some("200.1"));
        assert_eq!(signal.quantity, "1");
    }

    #[test]
    fn cross_exchange_funding_signal_accepts_high_precision_depth_sizes() {
        let mut input = cross_exchange_funding_signal_input();
        input.long_ask_size = Some("2.000000009".to_owned());
        input.short_bid_size = Some("2.001000009".to_owned());

        let signal = evaluate_cross_exchange_funding_arb_signal(&input)
            .expect("high precision depth sizes are truncated conservatively");

        assert!(signal.is_candidate);
        assert_eq!(signal.long_ask_depth_usd.as_deref(), Some("200"));
        assert_eq!(signal.short_bid_depth_usd.as_deref(), Some("200.20005"));
    }

    #[test]
    fn cross_exchange_funding_signal_keeps_price_precision_strict() {
        let mut input = cross_exchange_funding_signal_input();
        input.long_best_ask = "100.000000001".to_owned();

        let error =
            evaluate_cross_exchange_funding_arb_signal(&input).expect_err("price stays strict");

        assert!(error.contains("long_best_ask"));
        assert!(error.contains("exceeds 8 fractional digits"));
    }

    #[test]
    fn cross_exchange_funding_signal_rejects_insufficient_spread() {
        let mut input = cross_exchange_funding_signal_input();
        input.short_funding_rate = "0.00100000".to_owned();

        let signal = evaluate_cross_exchange_funding_arb_signal(&input).expect("signal");

        assert!(!signal.is_candidate);
        assert!(signal
            .reason
            .as_deref()
            .expect("reason")
            .contains("below minimum"));
    }

    #[test]
    fn cross_exchange_funding_signal_deducts_adverse_entry_price() {
        let mut input = cross_exchange_funding_signal_input();
        input.long_best_bid = "100.05".to_owned();
        input.long_best_ask = "100.10".to_owned();
        input.short_best_bid = "100.00".to_owned();
        input.short_best_ask = "100.05".to_owned();
        input.short_funding_rate = "0.00410000".to_owned();

        let signal = evaluate_cross_exchange_funding_arb_signal(&input).expect("signal");

        assert!(!signal.is_candidate);
        assert!(signal.entry_price_edge_bps.starts_with("-9."));
        let reason = signal.reason.as_deref().expect("reason");
        assert!(reason.contains("below minimum"));
        assert!(reason.contains("entry_price_adverse_bps"));
    }

    #[test]
    fn cross_exchange_funding_signal_rejects_insufficient_depth() {
        let mut input = cross_exchange_funding_signal_input();
        input.long_ask_size = Some("0.5".to_owned());

        let signal = evaluate_cross_exchange_funding_arb_signal(&input).expect("signal");

        assert!(!signal.is_candidate);
        assert_eq!(signal.long_ask_depth_usd.as_deref(), Some("50"));
        assert!(signal
            .reason
            .as_deref()
            .expect("reason")
            .contains("insufficient order-book depth"));
    }

    #[test]
    fn cross_exchange_funding_signal_normalizes_hourly_funding_interval() {
        let mut input = cross_exchange_funding_signal_input();
        input.short_funding_rate = "0.00090000".to_owned();
        input.funding_interval_hours = "1".to_owned();

        let signal = evaluate_cross_exchange_funding_arb_signal(&input).expect("signal");

        assert!(signal.is_candidate);
        assert_eq!(signal.gross_funding_spread_bps, "64");
        assert_eq!(signal.net_funding_bps, "20");
    }

    #[test]
    fn cross_exchange_funding_signal_rejects_entry_price_divergence() {
        let mut input = cross_exchange_funding_signal_input();
        input.short_best_bid = "104.00".to_owned();

        let signal = evaluate_cross_exchange_funding_arb_signal(&input).expect("signal");

        assert!(!signal.is_candidate);
        assert!(signal
            .reason
            .as_deref()
            .expect("reason")
            .contains("entry_price_divergence_bps"));
    }

    #[test]
    fn cross_exchange_funding_signal_rejects_invalid_config() {
        let mut input = cross_exchange_funding_signal_input();
        input.notional_usd = "0".to_owned();
        assert!(evaluate_cross_exchange_funding_arb_signal(&input)
            .expect_err("zero notional must fail")
            .contains("notional_usd"));

        let mut input = cross_exchange_funding_signal_input();
        input.long_taker_fee_bps = "-1".to_owned();
        assert!(evaluate_cross_exchange_funding_arb_signal(&input)
            .expect_err("negative fee must fail")
            .contains("long_taker_fee_bps"));
    }

    #[test]
    fn spot_perp_basis_exit_signal_closes_on_profit_or_convergence() {
        let signal = evaluate_spot_perp_basis_exit_signal(&basis_exit_input("100.90", "101.00"))
            .expect("exit signal");

        assert_eq!(signal.decision, SpotPerpBasisExitDecision::Close);
        assert_eq!(signal.current_close_basis_bps, 9);
        assert_eq!(signal.exit_total_cost_bps, 20);
        assert_eq!(signal.estimated_exit_profit_bps, 55);
        assert_eq!(signal.estimated_exit_profit_usd, "0.55");
        assert!(signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::TakeProfit));
        assert!(signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::BasisConverged));
    }

    #[test]
    fn spot_perp_basis_exit_signal_closes_when_funding_turns_or_basis_widens() {
        let mut input = basis_exit_input("99.00", "101.50");
        input.expected_next_funding_bps = -1;

        let signal = evaluate_spot_perp_basis_exit_signal(&input).expect("exit signal");

        assert_eq!(signal.decision, SpotPerpBasisExitDecision::Close);
        assert!(signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::FundingNoLongerPays));
        assert!(signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::BasisWidened));
        assert!(signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::StopLoss));
    }

    #[test]
    fn spot_perp_basis_exit_signal_handles_adl_warning_and_event() {
        let mut warning = basis_exit_input("100.20", "101.00");
        warning.adl_state = SpotPerpBasisAdlState::Warning;

        let warning_signal =
            evaluate_spot_perp_basis_exit_signal(&warning).expect("ADL warning signal");

        assert_eq!(warning_signal.decision, SpotPerpBasisExitDecision::Close);
        assert!(warning_signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::AdlWarning));

        let mut deleveraging = basis_exit_input("100.20", "101.00");
        deleveraging.adl_state = SpotPerpBasisAdlState::Deleveraging;

        let deleveraging_signal =
            evaluate_spot_perp_basis_exit_signal(&deleveraging).expect("ADL event signal");

        assert_eq!(
            deleveraging_signal.decision,
            SpotPerpBasisExitDecision::EmergencyReconcileAndDeRisk
        );
        assert!(deleveraging_signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::AdlDeleveraging));
    }

    #[test]
    fn spot_perp_basis_exit_signal_closes_on_liquidation_or_imbalance_risk() {
        let mut input = basis_exit_input("100.20", "101.00");
        input.liquidation_buffer_bps = Some(120);
        input.position_imbalance_bps = 6;

        let signal = evaluate_spot_perp_basis_exit_signal(&input).expect("exit signal");

        assert_eq!(signal.decision, SpotPerpBasisExitDecision::Close);
        assert!(signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::LiquidationBufferTooThin));
        assert!(signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::PositionImbalance));
    }

    #[test]
    fn spot_perp_basis_exit_signal_requires_emergency_derisk_on_unknown_state() {
        let mut input = basis_exit_input("100.20", "101.00");
        input.external_state_unknown = true;
        input.liquidation_buffer_bps = None;

        let signal = evaluate_spot_perp_basis_exit_signal(&input).expect("exit signal");

        assert_eq!(
            signal.decision,
            SpotPerpBasisExitDecision::EmergencyReconcileAndDeRisk
        );
        assert!(signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::UnknownExternalState));
        assert!(signal
            .reason_codes
            .contains(&SpotPerpBasisExitReason::LiquidationBufferMissing));
    }

    #[test]
    fn spot_perp_basis_strategy_rejects_invalid_economics_config() {
        let mut config = SpotPerpBasisStrategyConfig::binance_btcusdt();
        config.economics.notional_usd = "0".to_owned();

        let error = SpotPerpBasisStrategy::with_config(config).expect_err("invalid config");

        assert!(error
            .to_string()
            .contains("notional must be greater than zero"));
    }

    #[test]
    fn strategy_manifest_only_depends_on_strategy_api() {
        let manifest = include_str!("../Cargo.toml");
        assert!(manifest.contains("arb-strategy-api"));
        for forbidden in [
            "arb-domain",
            "arb-contracts",
            "arb-config",
            "arb-eventstore",
            "arb-ledger",
            "arb-reconciliation",
            "arb-risk",
            "arb-execution",
            "arb-venue-data",
            "arb-venue-exec",
            "arb-signing",
            "arb-replay",
            "arb-ops",
            "arb-runtime",
        ] {
            assert!(
                !manifest.contains(forbidden),
                "sample strategies crate must not directly depend on {forbidden}"
            );
        }
    }

    fn test_context(has_spot: bool, has_rest: bool) -> TestContext {
        test_context_with_market_events(has_spot, has_rest, strategy_market_events())
    }

    fn test_context_with_market_events(
        has_spot: bool,
        has_rest: bool,
        market_events: Vec<NormalizedEvent>,
    ) -> TestContext {
        let expected =
            candidate_from_json_strict(EXPECTED_CANDIDATE).expect("expected candidate fixture");
        TestContext {
            snapshot: TestSnapshot {
                portfolio_state_id: expected.current_portfolio_state_ref.as_str().to_owned(),
                source_event_refs: expected.input_event_refs,
                market_events,
            },
            capabilities: TestCapabilities {
                has_spot,
                has_rest,
                has_basis_spot: true,
                has_basis_perp: true,
                has_basis_funding: true,
                has_basis_rest: true,
            },
            config: TestConfig {
                config_version: STRATEGY_SMOKE_CONFIG_VERSION.to_owned(),
                config_hash: STRATEGY_SMOKE_CONFIG_HASH.to_owned(),
                kill_switch: false,
                disabled_strategy: false,
                disabled_venue: false,
            },
            time: FixedTimeSource::from_rfc3339_z("2026-01-01T00:00:02Z").expect("fixed time"),
        }
    }

    fn payload_constraint<'a>(
        candidate: &'a CandidatePortfolioTransition,
        key: &str,
    ) -> Option<&'a str> {
        leg_constraint(candidate, 0, key)
    }

    fn leg_constraint<'a>(
        candidate: &'a CandidatePortfolioTransition,
        leg_index: usize,
        key: &str,
    ) -> Option<&'a str> {
        let leg = candidate.legs.get(leg_index)?;
        match leg.constraints.get(key)? {
            JsonValue::String(value) => Some(value.as_str()),
            _ => None,
        }
    }

    struct TestContext {
        snapshot: TestSnapshot,
        capabilities: TestCapabilities,
        config: TestConfig,
        time: FixedTimeSource,
    }

    impl StrategyReadContext for TestContext {
        fn snapshot(&self) -> &dyn arb_strategy_api::StrategySnapshotReader {
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

    struct TestSnapshot {
        portfolio_state_id: String,
        source_event_refs: Vec<Identifier>,
        market_events: Vec<NormalizedEvent>,
    }

    impl arb_strategy_api::PortfolioSnapshotReader for TestSnapshot {
        fn portfolio_state(&self) -> &PortfolioState {
            panic!("sample strategy tests do not require full portfolio state")
        }

        fn portfolio_state_id(&self) -> &str {
            &self.portfolio_state_id
        }

        fn source_event_refs(&self) -> &[Identifier] {
            &self.source_event_refs
        }
    }

    impl arb_strategy_api::MarketSnapshotReader for TestSnapshot {
        fn market_events(&self) -> &[NormalizedEvent] {
            &self.market_events
        }
    }

    struct TestCapabilities {
        has_spot: bool,
        has_rest: bool,
        has_basis_spot: bool,
        has_basis_perp: bool,
        has_basis_funding: bool,
        has_basis_rest: bool,
    }

    impl VenueCapabilityReader for TestCapabilities {
        fn venue_capabilities(&self) -> &[VenueCapabilityDescriptor] {
            &[]
        }

        fn has_market_capability(&self, venue_id: &str, capability: &MarketCapability) -> bool {
            match (venue_id, capability) {
                (SAMPLE_VENUE_ID, MarketCapability::ProvidesSpotMarkets) => self.has_spot,
                (
                    BINANCE_BASIS_SPOT_VENUE_ID
                    | BYBIT_BASIS_SPOT_VENUE_ID
                    | OKX_BASIS_SPOT_VENUE_ID,
                    MarketCapability::ProvidesSpotMarkets
                    | MarketCapability::ProvidesOrderBookMarkets,
                ) => self.has_basis_spot,
                (
                    BINANCE_BASIS_PERP_VENUE_ID
                    | BYBIT_BASIS_PERP_VENUE_ID
                    | OKX_BASIS_PERP_VENUE_ID,
                    MarketCapability::ProvidesPerpetuals
                    | MarketCapability::ProvidesOrderBookMarkets,
                ) => self.has_basis_perp,
                (
                    BINANCE_BASIS_PERP_VENUE_ID
                    | BYBIT_BASIS_PERP_VENUE_ID
                    | OKX_BASIS_PERP_VENUE_ID,
                    MarketCapability::ProvidesFundingRates,
                ) => self.has_basis_funding,
                _ => false,
            }
        }

        fn has_data_surface(&self, venue_id: &str, surface: &DataSurface) -> bool {
            if *surface != DataSurface::RestPolling {
                return false;
            }
            match venue_id {
                SAMPLE_VENUE_ID => self.has_rest,
                BINANCE_BASIS_SPOT_VENUE_ID
                | BINANCE_BASIS_PERP_VENUE_ID
                | BYBIT_BASIS_SPOT_VENUE_ID
                | BYBIT_BASIS_PERP_VENUE_ID
                | OKX_BASIS_SPOT_VENUE_ID
                | OKX_BASIS_PERP_VENUE_ID => self.has_basis_rest,
                _ => false,
            }
        }
    }

    struct TestConfig {
        config_version: String,
        config_hash: String,
        kill_switch: bool,
        disabled_strategy: bool,
        disabled_venue: bool,
    }

    impl StrategyConfigReader for TestConfig {
        fn config_version(&self) -> &str {
            &self.config_version
        }

        fn config_hash(&self) -> &str {
            &self.config_hash
        }

        fn execution_mode(&self) -> ExecutionMode {
            ExecutionMode::ReadOnly
        }

        fn kill_switch_triggered(&self) -> bool {
            self.kill_switch
        }

        fn strategy_disabled(&self, strategy_id: &str) -> bool {
            matches!(
                strategy_id,
                SAMPLE_STRATEGY_ID
                    | BINANCE_BASIS_STRATEGY_ID
                    | BYBIT_BASIS_STRATEGY_ID
                    | OKX_BASIS_STRATEGY_ID
                    | CROSS_EXCHANGE_FUNDING_ARB_STRATEGY_ID
            ) && self.disabled_strategy
        }

        fn venue_disabled(&self, venue_id: &str) -> bool {
            matches!(
                venue_id,
                SAMPLE_VENUE_ID
                    | BINANCE_BASIS_SPOT_VENUE_ID
                    | BINANCE_BASIS_PERP_VENUE_ID
                    | BYBIT_BASIS_SPOT_VENUE_ID
                    | BYBIT_BASIS_PERP_VENUE_ID
                    | OKX_BASIS_SPOT_VENUE_ID
                    | OKX_BASIS_PERP_VENUE_ID
            ) && self.disabled_venue
        }
    }

    fn basis_test_context(market_events: Vec<NormalizedEvent>) -> TestContext {
        TestContext {
            snapshot: TestSnapshot {
                portfolio_state_id: "state:binance-basis-test-01".to_owned(),
                source_event_refs: market_events
                    .iter()
                    .map(|event| event.event_id.clone())
                    .collect(),
                market_events,
            },
            capabilities: TestCapabilities {
                has_spot: true,
                has_rest: true,
                has_basis_spot: true,
                has_basis_perp: true,
                has_basis_funding: true,
                has_basis_rest: true,
            },
            config: TestConfig {
                config_version: STRATEGY_SMOKE_CONFIG_VERSION.to_owned(),
                config_hash: STRATEGY_SMOKE_CONFIG_HASH.to_owned(),
                kill_switch: false,
                disabled_strategy: false,
                disabled_venue: false,
            },
            time: FixedTimeSource::from_rfc3339_z("2026-01-01T00:00:02Z").expect("fixed time"),
        }
    }

    fn cross_exchange_funding_signal_input() -> CrossExchangeFundingArbSignalInput {
        CrossExchangeFundingArbSignalInput {
            symbol: "BTCUSDT".to_owned(),
            long_venue_id: BINANCE_BASIS_PERP_VENUE_ID.to_owned(),
            short_venue_id: BYBIT_BASIS_PERP_VENUE_ID.to_owned(),
            long_best_bid: "99.95".to_owned(),
            long_best_ask: "100.00".to_owned(),
            long_ask_size: Some("2.0".to_owned()),
            long_ask_depth: Vec::new(),
            short_best_bid: "100.05".to_owned(),
            short_best_ask: "100.10".to_owned(),
            short_bid_size: Some("2.0".to_owned()),
            short_bid_depth: Vec::new(),
            long_funding_rate: "0.00010000".to_owned(),
            short_funding_rate: "0.00600000".to_owned(),
            funding_interval_hours: "8".to_owned(),
            notional_usd: "100.00".to_owned(),
            long_taker_fee_bps: BINANCE_BASIS_PERP_TAKER_FEE_BPS.to_owned(),
            short_taker_fee_bps: BYBIT_BASIS_PERP_TAKER_FEE_BPS.to_owned(),
            slippage_buffer_bps: DEFAULT_BASIS_SLIPPAGE_BUFFER_BPS,
            max_entry_price_divergence_bps: 20,
            min_net_funding_bps: 5,
        }
    }

    fn cross_exchange_funding_events(
        binance_funding_rate: &str,
        bybit_funding_rate: &str,
        binance_interval_hours: &str,
        bybit_interval_hours: &str,
    ) -> Vec<NormalizedEvent> {
        vec![
            basis_book_event(
                "funding-binance",
                BINANCE_BASIS_PERP_VENUE_ID,
                BINANCE_BASIS_PERP_INSTRUMENT_ID,
                "Perp",
                "99.95",
                "100.00",
                "CHECK_PASSED",
            ),
            basis_book_event(
                "funding-bybit",
                BYBIT_BASIS_PERP_VENUE_ID,
                BYBIT_BASIS_PERP_INSTRUMENT_ID,
                "Perp",
                "100.05",
                "100.10",
                "CHECK_PASSED",
            ),
            funding_premium_event(
                "binance",
                BINANCE_BASIS_PERP_VENUE_ID,
                BINANCE_BASIS_PERP_INSTRUMENT_ID,
                binance_funding_rate,
                binance_interval_hours,
            ),
            funding_premium_event(
                "bybit",
                BYBIT_BASIS_PERP_VENUE_ID,
                BYBIT_BASIS_PERP_INSTRUMENT_ID,
                bybit_funding_rate,
                bybit_interval_hours,
            ),
        ]
    }

    fn funding_premium_event(
        tag: &str,
        venue_id: &str,
        instrument_id: &str,
        last_funding_rate: &str,
        funding_interval_hours: &str,
    ) -> NormalizedEvent {
        normalized_event_from_json_strict(&format!(
            r#"{{
  "event_id": "event:funding-arb-test:{tag}:premium-index",
  "event_type": "NormalizedMarketDataEvent",
  "event_version": "1.0.0",
  "timestamp_event": "2026-01-01T00:00:01Z",
  "timestamp_ingested": "2026-01-01T00:00:02Z",
  "source": "test:funding-arb",
  "source_sequence": "funding-arb-test:{tag}:premium-index",
  "correlation_id": "corr:funding-arb-test:{tag}:premium-index",
  "schema_version": "1.0.0",
  "venue_id": {},
  "instrument_id": {},
  "payload": {{
    "basis_role": "Perp",
    "funding_interval_hours": {},
    "index_price": "100.00",
    "kind": "PerpPremiumIndex",
    "last_funding_rate": {},
    "mark_price": "100.00",
    "next_funding_time_ms": 1767254400000,
    "risk_reason_code": "CHECK_PASSED"
  }},
  "checksum": "sha256:fixture-funding-arb-premium-{tag}"
}}"#,
            json_string(venue_id),
            json_string(instrument_id),
            json_string(funding_interval_hours),
            json_string(last_funding_rate),
        ))
        .expect("funding premium event")
    }

    fn bybit_basis_test_context(
        spot_risk_reason_code: &str,
        perp_best_bid: &str,
        perp_best_ask: &str,
        premium_risk_reason_code: &str,
    ) -> TestContext {
        basis_test_context(vec![
            basis_book_event(
                "bybit-spot",
                BYBIT_BASIS_SPOT_VENUE_ID,
                BYBIT_BASIS_SPOT_INSTRUMENT_ID,
                "Spot",
                "99.90",
                "100.00",
                spot_risk_reason_code,
            ),
            basis_book_event(
                "bybit-perp",
                BYBIT_BASIS_PERP_VENUE_ID,
                BYBIT_BASIS_PERP_INSTRUMENT_ID,
                "Perp",
                perp_best_bid,
                perp_best_ask,
                "CHECK_PASSED",
            ),
            basis_premium_event_for(
                BYBIT_BASIS_PERP_VENUE_ID,
                BYBIT_BASIS_PERP_INSTRUMENT_ID,
                "0.00010000",
                premium_risk_reason_code,
            ),
        ])
    }

    fn okx_basis_test_context(
        spot_risk_reason_code: &str,
        perp_best_bid: &str,
        perp_best_ask: &str,
        premium_risk_reason_code: &str,
    ) -> TestContext {
        basis_test_context(vec![
            basis_book_event(
                "okx-spot",
                OKX_BASIS_SPOT_VENUE_ID,
                OKX_BASIS_SPOT_INSTRUMENT_ID,
                "Spot",
                "99.90",
                "100.00",
                spot_risk_reason_code,
            ),
            basis_book_event(
                "okx-swap",
                OKX_BASIS_PERP_VENUE_ID,
                OKX_BASIS_PERP_INSTRUMENT_ID,
                "Perp",
                perp_best_bid,
                perp_best_ask,
                "CHECK_PASSED",
            ),
            basis_premium_event_for(
                OKX_BASIS_PERP_VENUE_ID,
                OKX_BASIS_PERP_INSTRUMENT_ID,
                "0.00010000",
                premium_risk_reason_code,
            ),
        ])
    }

    fn basis_book_event(
        tag: &str,
        venue_id: &str,
        instrument_id: &str,
        basis_role: &str,
        best_bid: &str,
        best_ask: &str,
        risk_reason_code: &str,
    ) -> NormalizedEvent {
        normalized_event_from_json_strict(&format!(
            r#"{{
  "event_id": "event:basis-test:{tag}:book-ticker",
  "event_type": "NormalizedMarketDataEvent",
  "event_version": "1.0.0",
  "timestamp_event": "2026-01-01T00:00:01Z",
  "timestamp_ingested": "2026-01-01T00:00:02Z",
  "source": "test:binance-basis",
  "source_sequence": "basis-test:{tag}:book-ticker",
  "correlation_id": "corr:basis-test:{tag}:book-ticker",
  "schema_version": "1.0.0",
  "venue_id": {},
  "instrument_id": {},
	  "payload": {{
	    "basis_role": {},
	    "ask_size": "2.0",
	    "best_ask": {},
	    "best_bid": {},
	    "bid_size": "1.0",
	    "kind": "BookTicker",
	    "risk_reason_code": {}
	  }},
  "checksum": "sha256:fixture-basis-book-{tag}"
}}"#,
            json_string(venue_id),
            json_string(instrument_id),
            json_string(basis_role),
            json_string(best_ask),
            json_string(best_bid),
            json_string(risk_reason_code),
        ))
        .expect("basis book event")
    }

    fn basis_premium_event(last_funding_rate: &str, risk_reason_code: &str) -> NormalizedEvent {
        basis_premium_event_for(
            BINANCE_BASIS_PERP_VENUE_ID,
            BINANCE_BASIS_PERP_INSTRUMENT_ID,
            last_funding_rate,
            risk_reason_code,
        )
    }

    fn basis_premium_event_for(
        venue_id: &str,
        instrument_id: &str,
        last_funding_rate: &str,
        risk_reason_code: &str,
    ) -> NormalizedEvent {
        basis_premium_event_for_with_interval(
            venue_id,
            instrument_id,
            last_funding_rate,
            "8",
            risk_reason_code,
        )
    }

    fn basis_premium_event_for_with_interval(
        venue_id: &str,
        instrument_id: &str,
        last_funding_rate: &str,
        funding_interval_hours: &str,
        risk_reason_code: &str,
    ) -> NormalizedEvent {
        normalized_event_from_json_strict(&format!(
            r#"{{
  "event_id": "event:basis-test:premium-index",
  "event_type": "NormalizedMarketDataEvent",
  "event_version": "1.0.0",
  "timestamp_event": "2026-01-01T00:00:01Z",
  "timestamp_ingested": "2026-01-01T00:00:02Z",
  "source": "test:binance-basis",
  "source_sequence": "basis-test:premium-index",
  "correlation_id": "corr:basis-test:premium-index",
  "schema_version": "1.0.0",
  "venue_id": {},
  "instrument_id": {},
  "payload": {{
    "basis_role": "Perp",
    "funding_interval_hours": {},
    "index_price": "100.00",
    "kind": "PerpPremiumIndex",
    "last_funding_rate": {},
    "mark_price": "101.00",
    "next_funding_time_ms": 1767254400000,
    "risk_reason_code": {}
  }},
  "checksum": "sha256:fixture-basis-premium-index"
}}"#,
            json_string(venue_id),
            json_string(instrument_id),
            json_string(funding_interval_hours),
            json_string(last_funding_rate),
            json_string(risk_reason_code),
        ))
        .expect("basis premium event")
    }

    fn basis_exit_input(spot_best_bid: &str, perp_best_ask: &str) -> SpotPerpBasisExitSignalInput {
        SpotPerpBasisExitSignalInput {
            symbol: "BTCUSDT".to_owned(),
            spot_best_bid: spot_best_bid.to_owned(),
            perp_best_ask: perp_best_ask.to_owned(),
            notional_usd: "100.00".to_owned(),
            entry_gross_basis_bps: 100,
            entry_total_cost_bps: 20,
            accumulated_funding_bps: 4,
            expected_next_funding_bps: 1,
            exit_spot_taker_fee_bps: 10,
            exit_perp_taker_fee_bps: 5,
            exit_slippage_buffer_bps: 5,
            target_profit_bps: 40,
            convergence_buffer_bps: 2,
            min_next_funding_bps: 0,
            max_basis_widen_bps: 50,
            max_loss_bps: 50,
            liquidation_buffer_bps: Some(500),
            min_liquidation_buffer_bps: 150,
            position_imbalance_bps: 0,
            max_position_imbalance_bps: 5,
            data_is_stale: false,
            external_state_unknown: false,
            adl_state: SpotPerpBasisAdlState::None,
        }
    }

    fn strategy_replay_context() -> StrategyInput {
        let root = strategy_fixture_dir();
        let portfolio = portfolio_state_from_json_strict(&read_fixture("portfolio_state.json"))
            .expect("portfolio fixture should parse");
        let events = read_fixture("events.jsonl")
            .lines()
            .map(|line| {
                normalized_event_from_json_strict(line).expect("event fixture should parse")
            })
            .collect::<Vec<_>>();
        let capabilities = read_fixture("venue_capabilities.jsonl")
            .lines()
            .map(|line| {
                venue_capability_from_json_strict(line).expect("capability fixture should parse")
            })
            .collect::<Vec<_>>();
        let config = StrategyConfigSnapshot::from_yaml_str(&read_fixture("config.yaml"))
            .expect("config fixture should parse into strategy view");
        let fixed_time = yaml_scalar(
            &fs::read_to_string(root.join("replay.yaml")).expect("replay metadata"),
            "fixed_time",
        );

        StrategyInput::new(
            ReadOnlySnapshot::new(portfolio, events),
            VenueCapabilitySnapshot::new(capabilities).expect("capabilities should be unique"),
            config,
            FixedTimeSource::from_rfc3339_z(&fixed_time).expect("fixed time should parse"),
        )
    }

    fn read_fixture(name: &str) -> String {
        fs::read_to_string(strategy_fixture_dir().join(name)).expect("fixture should be readable")
    }

    fn strategy_market_events() -> Vec<NormalizedEvent> {
        read_fixture("events.jsonl")
            .lines()
            .map(|line| {
                normalized_event_from_json_strict(line).expect("event fixture should parse")
            })
            .collect()
    }

    fn strategy_fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/replay/strategy_smoke")
    }

    fn yaml_scalar(input: &str, key: &str) -> String {
        let prefix = format!("{key}:");
        input
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(&prefix))
            .map(str::trim)
            .map(|value| value.trim_matches('"').to_owned())
            .unwrap_or_else(|| panic!("missing YAML scalar `{key}`"))
    }
}
