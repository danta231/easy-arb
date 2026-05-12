//! `arb-strategies` 样例策略集合。
//!
//! 中文说明：本 crate 只依赖 `arb-strategy-api`，策略只能读取只读上下文并
//! 输出 `CandidatePortfolioTransition` 或明确拒绝原因。这里不暴露下单、
//! 签名、转账、账本写入或运行时装配能力。

#![forbid(unsafe_code)]

use arb_strategy_api::{
    candidate_from_json_strict, validate_candidate_for_context, CandidatePortfolioTransition,
    DataSurface, Identifier, JsonValue, MarketCapability, NormalizedEvent, NormalizedEventType,
    Strategy, StrategyApiResult, StrategyDiagnostic, StrategyEvaluation, StrategyMetadata,
    StrategyReadContext, StrategyRejectReason, StrategyRejection,
};

const SAMPLE_STRATEGY_ID: &str = "strat:sample-spot-demo";
const SAMPLE_STRATEGY_VERSION: &str = "1.0.0";
const SAMPLE_CODE_VERSION: &str = "code:sample-spot-demo-1";
const SAMPLE_VENUE_ID: &str = "venue:SIM";
const SAMPLE_INSTRUMENT_ID: &str = "inst:BTC-USDC";
const SAMPLE_TRANSITION_ID: &str = "trans:sample-spot-001";
const BASIS_STRATEGY_ID: &str = "strat:binance-spot-perp-basis";
const BASIS_STRATEGY_VERSION: &str = "1.0.0";
const BASIS_CODE_VERSION: &str = "code:binance-spot-perp-basis-1";
const BASIS_SPOT_VENUE_ID: &str = "venue:BINANCE-SPOT";
const BASIS_PERP_VENUE_ID: &str = "venue:BINANCE-USDM";
const BASIS_SPOT_INSTRUMENT_ID: &str = "inst:BINANCE:BTCUSDT:SPOT";
const BASIS_PERP_INSTRUMENT_ID: &str = "inst:BINANCE:BTCUSDT:USDM-PERP";
const BASIS_TRANSITION_ID: &str = "trans:binance-basis-btcusdt-001";
const BASIS_NOTIONAL_USD: &str = "100.00";
const BASIS_SPOT_TAKER_FEE_BPS: i128 = 10;
const BASIS_PERP_TAKER_FEE_BPS: i128 = 5;
const BASIS_SLIPPAGE_BUFFER_BPS: i128 = 5;
const BASIS_MIN_NET_BPS: i128 = 5;
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

/// Binance 现货-永续 basis 只读策略。
///
/// 中文说明：策略只读取已经标准化的 Binance 公共行情事件，计算买现货、做空
/// USDⓈ-M 永续的正向 basis 是否在扣除静态手续费和滑点缓冲后仍为正。它只输出
/// 候选组合转换或拒绝原因，不下单、不签名、不访问账户。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotPerpBasisStrategy {
    metadata: StrategyMetadata,
}

impl SpotPerpBasisStrategy {
    /// 创建 Binance spot-perp basis 只读策略。
    pub fn new() -> StrategyApiResult<Self> {
        Ok(Self {
            metadata: StrategyMetadata::new(
                BASIS_STRATEGY_ID,
                BASIS_STRATEGY_VERSION,
                BASIS_CODE_VERSION,
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
            StrategyDiagnostic::new("SPOT_PERP_BASIS_REJECTED", detail, context.time().now())?;
        Ok(StrategyEvaluation::rejected(rejection).with_diagnostic(diagnostic))
    }

    fn ensure_capabilities(
        &self,
        context: &dyn StrategyReadContext,
    ) -> StrategyApiResult<Option<StrategyEvaluation>> {
        let required_market = [
            (
                BASIS_SPOT_VENUE_ID,
                MarketCapability::ProvidesSpotMarkets,
                "Binance spot venue lacks ProvidesSpotMarkets capability",
            ),
            (
                BASIS_SPOT_VENUE_ID,
                MarketCapability::ProvidesOrderBookMarkets,
                "Binance spot venue lacks ProvidesOrderBookMarkets capability",
            ),
            (
                BASIS_PERP_VENUE_ID,
                MarketCapability::ProvidesPerpetuals,
                "Binance USD-M venue lacks ProvidesPerpetuals capability",
            ),
            (
                BASIS_PERP_VENUE_ID,
                MarketCapability::ProvidesOrderBookMarkets,
                "Binance USD-M venue lacks ProvidesOrderBookMarkets capability",
            ),
            (
                BASIS_PERP_VENUE_ID,
                MarketCapability::ProvidesFundingRates,
                "Binance USD-M venue lacks ProvidesFundingRates capability",
            ),
        ];
        for (venue_id, capability, detail) in required_market {
            if !context
                .capabilities()
                .has_market_capability(venue_id, &capability)
            {
                return Ok(Some(self.reject(
                    context,
                    StrategyRejectReason::VenueCapabilityMissing,
                    detail,
                )?));
            }
        }

        for venue_id in [BASIS_SPOT_VENUE_ID, BASIS_PERP_VENUE_ID] {
            if !context
                .capabilities()
                .has_data_surface(venue_id, &DataSurface::RestPolling)
            {
                return Ok(Some(self.reject(
                    context,
                    StrategyRejectReason::VenueCapabilityMissing,
                    format!("{venue_id} lacks RESTPolling data surface"),
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
        let input_event_refs = source_event_refs(context);
        let input_event_refs_json = json_string_array(&input_event_refs);
        let config_version = context.config().config_version();
        let quantity = opportunity.quantity.format_trimmed();
        let expected_profit_usd = opportunity.expected_profit_usd.format_trimmed();
        let fee_estimate_usd = opportunity.fee_estimate_usd.format_trimmed();
        let slippage_estimate_usd = opportunity.slippage_estimate_usd.format_trimmed();
        let gross_bps = opportunity.gross_bps.to_string();
        let net_bps = opportunity.net_bps.to_string();
        let total_cost_bps = opportunity.total_cost_bps.to_string();
        let assumption = format!(
            "Read-only Binance public data signal: buy spot at {}, short USD-M perp at {}, gross_basis_bps={}, total_cost_bps={}, net_basis_bps={}. Static fee/slippage assumptions must be replaced with account-specific checks before any order path.",
            opportunity.spot.best_ask.format_trimmed(),
            opportunity.perp.best_bid.format_trimmed(),
            gross_bps,
            total_cost_bps,
            net_bps
        );
        let funding_summary = format!(
            "Public premiumIndex lastFundingRate={}, mark_price={}, index_price={}, nextFundingTimeMs={}.",
            opportunity.premium.last_funding_rate,
            opportunity.premium.mark_price.format_trimmed(),
            opportunity.premium.index_price.format_trimmed(),
            opportunity.premium.next_funding_time_ms
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
    "kind": "UntilBasisConvergence"
  }},
  "legs": [
    {{
      "leg_id": "candleg:binance-basis-buy-spot-btcusdt",
      "leg_type": "Trade",
      "venue_id": "venue:BINANCE-SPOT",
      "instrument_id": "inst:BINANCE:BTCUSDT:SPOT",
      "account_id": "acct:binance-basis-readonly",
      "side": "Buy",
      "asset_flows": [
        {{
          "asset_id": "asset:USDT",
          "direction": "Out",
          "amount": {},
          "account_id": "acct:binance-basis-readonly"
        }},
        {{
          "asset_id": "asset:BTC",
          "direction": "In",
          "amount": {},
          "account_id": "acct:binance-basis-readonly"
        }}
      ],
      "constraints": {{
        "basis_leg_role": "spot_buy",
        "gross_basis_bps": {},
        "max_slippage_bps": {},
        "net_basis_bps": {},
        "notional_usdt": {},
        "reference_best_ask": {},
        "reference_best_bid": {},
        "reference_market_event_id": {}
      }},
      "failure_modes": [
        "PartialFill",
        "VenueOutage",
        "UnknownState"
      ]
    }},
    {{
      "leg_id": "candleg:binance-basis-short-usdm-perp-btcusdt",
      "leg_type": "Trade",
      "venue_id": "venue:BINANCE-USDM",
      "instrument_id": "inst:BINANCE:BTCUSDT:USDM-PERP",
      "account_id": "acct:binance-basis-readonly",
      "side": "Short",
      "asset_flows": [],
      "constraints": {{
        "basis_leg_role": "perp_short",
        "gross_basis_bps": {},
        "last_funding_rate": {},
        "max_slippage_bps": {},
        "net_basis_bps": {},
        "notional_usdt": {},
        "reference_best_ask": {},
        "reference_best_bid": {},
        "reference_market_event_id": {},
        "reference_premium_event_id": {}
      }},
      "failure_modes": [
        "PartialFill",
        "VenueOutage",
        "UnknownState"
      ]
    }}
  ],
  "expected_post_state_delta": {{
    "asset_flows": [
      {{
        "asset_id": "asset:USDT",
        "direction": "Out",
        "amount": {},
        "account_id": "acct:binance-basis-readonly"
      }},
      {{
        "asset_id": "asset:BTC",
        "direction": "In",
        "amount": {},
        "account_id": "acct:binance-basis-readonly"
      }}
    ],
    "position_deltas": [
      {{
        "instrument_id": "inst:BINANCE:BTCUSDT:SPOT",
        "account_id": "acct:binance-basis-readonly",
        "quantity_delta": {}
      }},
      {{
        "instrument_id": "inst:BINANCE:BTCUSDT:USDM-PERP",
        "account_id": "acct:binance-basis-readonly",
        "quantity_delta": {}
      }}
    ]
  }},
  "expected_economics": {{
    "expected_profit_usd": {},
    "expected_profit_bps": {},
    "fee_estimate_usd": {},
    "slippage_estimate_usd": {},
    "confidence": 0.72
  }},
  "required_capital": {{
    "asset_requirements": [
      {{
        "asset_id": "asset:USDT",
        "direction": "Out",
        "amount": {},
        "account_id": "acct:binance-basis-readonly"
      }}
    ],
    "recovery_buffer_usd": "1.00"
  }},
  "funding_impact": {{
    "summary": {},
    "impact_usd": "0",
    "confidence": 0.6
  }},
  "failure_modes": [
    "PartialFill",
    "ManualInterventionRequired",
    "UnknownState"
  ],
  "risk_flags": [
    "FundingRateUnstable",
    "BasisWidening",
    "OneLegExecutionRisk"
  ],
  "assumptions": [
    {{
      "assumption_id": "asm:binance-basis-public-data-readonly",
      "statement": {},
      "confidence": 0.72,
      "source_event_refs": {}
    }}
  ]
}}"#,
            json_string(BASIS_TRANSITION_ID),
            json_string(self.metadata.strategy_id()),
            json_string(self.metadata.strategy_version()),
            json_string(self.metadata.code_version()),
            json_string(config_version),
            json_string(&context.time().now_rfc3339_z()),
            input_event_refs_json,
            json_string(context.snapshot().portfolio_state_id()),
            json_string(BASIS_NOTIONAL_USD),
            json_string(&quantity),
            json_string(&gross_bps),
            json_string(&BASIS_SLIPPAGE_BUFFER_BPS.to_string()),
            json_string(&net_bps),
            json_string(BASIS_NOTIONAL_USD),
            json_string(&opportunity.spot.best_ask.format_trimmed()),
            json_string(&opportunity.spot.best_bid.format_trimmed()),
            json_string(&opportunity.spot.event_id),
            json_string(&gross_bps),
            json_string(&opportunity.premium.last_funding_rate),
            json_string(&BASIS_SLIPPAGE_BUFFER_BPS.to_string()),
            json_string(&net_bps),
            json_string(BASIS_NOTIONAL_USD),
            json_string(&opportunity.perp.best_ask.format_trimmed()),
            json_string(&opportunity.perp.best_bid.format_trimmed()),
            json_string(&opportunity.perp.event_id),
            json_string(&opportunity.premium.event_id),
            json_string(BASIS_NOTIONAL_USD),
            json_string(&quantity),
            json_string(&quantity),
            json_string(&format!("-{quantity}")),
            json_string(&expected_profit_usd),
            json_string(&net_bps),
            json_string(&fee_estimate_usd),
            json_string(&slippage_estimate_usd),
            json_string(BASIS_NOTIONAL_USD),
            json_string(&funding_summary),
            json_string(&assumption),
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
                "spot-perp basis strategy is disabled by read-only config",
            );
        }
        for venue_id in [BASIS_SPOT_VENUE_ID, BASIS_PERP_VENUE_ID] {
            if context.config().venue_disabled(venue_id) {
                return self.reject(
                    context,
                    StrategyRejectReason::ConfigDisabled,
                    format!("{venue_id} is disabled by read-only config"),
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
            BASIS_SPOT_VENUE_ID,
            BASIS_SPOT_INSTRUMENT_ID,
            "Spot",
        ) {
            Ok(quote) => quote,
            Err(detail) => {
                return self.reject(context, StrategyRejectReason::MissingData, detail);
            }
        };
        let perp = match latest_basis_book_ticker(
            context,
            BASIS_PERP_VENUE_ID,
            BASIS_PERP_INSTRUMENT_ID,
            "Perp",
        ) {
            Ok(quote) => quote,
            Err(detail) => {
                return self.reject(context, StrategyRejectReason::MissingData, detail);
            }
        };
        let premium = match latest_basis_premium_index(context) {
            Ok(premium) => premium,
            Err(detail) => {
                return self.reject(context, StrategyRejectReason::MissingData, detail);
            }
        };
        if spot.is_stale || perp.is_stale || premium.is_stale {
            return self.reject(
                context,
                StrategyRejectReason::DataStale,
                "one or more required Binance public market events are stale",
            );
        }

        let notional =
            match FixedDecimal::parse_non_negative("basis_notional_usd", BASIS_NOTIONAL_USD) {
                Ok(value) => value,
                Err(detail) => {
                    return self.reject(context, StrategyRejectReason::UnknownState, detail)
                }
            };
        let gross_bps = match gross_basis_bps(perp.best_bid, spot.best_ask) {
            Ok(value) => value,
            Err(detail) => return self.reject(context, StrategyRejectReason::UnknownState, detail),
        };
        let total_cost_bps =
            BASIS_SPOT_TAKER_FEE_BPS + BASIS_PERP_TAKER_FEE_BPS + BASIS_SLIPPAGE_BUFFER_BPS;
        let net_bps = gross_bps - total_cost_bps;
        if net_bps < BASIS_MIN_NET_BPS {
            return self.reject(
                context,
                StrategyRejectReason::NoCandidate,
                format!(
                    "basis net_bps={net_bps} below minimum {BASIS_MIN_NET_BPS}; gross_bps={gross_bps}, total_cost_bps={total_cost_bps}"
                ),
            );
        }

        let quantity = match FixedDecimal::quantity_for_notional(notional, spot.best_ask) {
            Ok(value) => value,
            Err(detail) => return self.reject(context, StrategyRejectReason::UnknownState, detail),
        };
        let expected_profit_usd = match FixedDecimal::usd_from_bps(notional, net_bps) {
            Ok(value) => value,
            Err(detail) => return self.reject(context, StrategyRejectReason::UnknownState, detail),
        };
        let fee_estimate_usd = match FixedDecimal::usd_from_bps(
            notional,
            BASIS_SPOT_TAKER_FEE_BPS + BASIS_PERP_TAKER_FEE_BPS,
        ) {
            Ok(value) => value,
            Err(detail) => return self.reject(context, StrategyRejectReason::UnknownState, detail),
        };
        let slippage_estimate_usd =
            match FixedDecimal::usd_from_bps(notional, BASIS_SLIPPAGE_BUFFER_BPS) {
                Ok(value) => value,
                Err(detail) => {
                    return self.reject(context, StrategyRejectReason::UnknownState, detail);
                }
            };

        let opportunity = BasisOpportunity {
            spot,
            perp,
            premium,
            quantity,
            gross_bps,
            total_cost_bps,
            net_bps,
            expected_profit_usd,
            fee_estimate_usd,
            slippage_estimate_usd,
        };
        let candidate = self.build_candidate(context, &opportunity)?;
        validate_candidate_for_context(context, self.metadata(), &candidate)?;
        let diagnostic = StrategyDiagnostic::new(
            "SPOT_PERP_BASIS_CANDIDATE",
            format!(
                "candidate {BASIS_TRANSITION_ID} emitted using Binance spot ask {}, perp bid {}, gross_bps {}, net_bps {}",
                opportunity.spot.best_ask.format_trimmed(),
                opportunity.perp.best_bid.format_trimmed(),
                opportunity.gross_bps,
                opportunity.net_bps
            ),
            context.time().now(),
        )?;
        Ok(StrategyEvaluation::candidate(candidate).with_diagnostic(diagnostic))
    }
}

/// 返回 Binance spot-perp basis 只读策略。
pub fn spot_perp_basis_strategy() -> StrategyApiResult<SpotPerpBasisStrategy> {
    SpotPerpBasisStrategy::new()
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
    pub perp_best_bid: String,
    pub perp_best_ask: String,
    pub notional_usd: String,
    pub spot_taker_fee_bps: i128,
    pub perp_taker_fee_bps: i128,
    pub slippage_buffer_bps: i128,
    pub min_net_bps: i128,
}

/// spot-perp basis 只读信号输出。
///
/// 中文说明：该输出只说明机会是否满足阈值，不能被当作订单或执行授权。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpotPerpBasisSignal {
    pub symbol: String,
    pub gross_bps: i128,
    pub total_cost_bps: i128,
    pub net_bps: i128,
    pub quantity: String,
    pub expected_profit_usd: String,
    pub fee_estimate_usd: String,
    pub slippage_estimate_usd: String,
    pub is_candidate: bool,
    pub reason: Option<String>,
}

/// 计算 spot-perp basis 只读信号。
pub fn evaluate_spot_perp_basis_signal(
    input: &SpotPerpBasisSignalInput,
) -> Result<SpotPerpBasisSignal, String> {
    let spot_ask = FixedDecimal::parse_non_negative("spot_best_ask", &input.spot_best_ask)?;
    let perp_bid = FixedDecimal::parse_non_negative("perp_best_bid", &input.perp_best_bid)?;
    let notional = FixedDecimal::parse_non_negative("notional_usd", &input.notional_usd)?;
    let gross_bps = gross_basis_bps(perp_bid, spot_ask)?;
    let total_cost_bps =
        input.spot_taker_fee_bps + input.perp_taker_fee_bps + input.slippage_buffer_bps;
    let net_bps = gross_bps - total_cost_bps;
    let quantity = FixedDecimal::quantity_for_notional(notional, spot_ask)?;
    let expected_profit_usd = FixedDecimal::usd_from_bps(notional, net_bps)?;
    let fee_estimate_usd = FixedDecimal::usd_from_bps(
        notional,
        input.spot_taker_fee_bps + input.perp_taker_fee_bps,
    )?;
    let slippage_estimate_usd = FixedDecimal::usd_from_bps(notional, input.slippage_buffer_bps)?;
    let is_candidate = net_bps >= input.min_net_bps;
    let reason = (!is_candidate).then(|| {
        format!(
            "basis net_bps={net_bps} below minimum {}; gross_bps={gross_bps}, total_cost_bps={total_cost_bps}",
            input.min_net_bps
        )
    });

    Ok(SpotPerpBasisSignal {
        symbol: input.symbol.clone(),
        gross_bps,
        total_cost_bps,
        net_bps,
        quantity: quantity.format_trimmed(),
        expected_profit_usd: expected_profit_usd.format_trimmed(),
        fee_estimate_usd: fee_estimate_usd.format_trimmed(),
        slippage_estimate_usd: slippage_estimate_usd.format_trimmed(),
        is_candidate,
        reason,
    })
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FixedDecimal {
    raw: i128,
}

impl FixedDecimal {
    fn parse_non_negative(field: &'static str, value: &str) -> Result<Self, String> {
        validate_market_decimal(field, value)?;
        let mut raw = 0_i128;
        let mut dot_seen = false;
        let mut frac_digits = 0_usize;
        for byte in value.bytes() {
            match byte {
                b'0'..=b'9' => {
                    if dot_seen {
                        if frac_digits == FIXED_SCALE_DIGITS {
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
                    return Err(format!(
                        "decimal field `{field}` must be a non-negative decimal string"
                    ));
                }
            }
        }
        for _ in frac_digits..FIXED_SCALE_DIGITS {
            raw = raw
                .checked_mul(10)
                .ok_or_else(|| format!("decimal field `{field}` overflowed"))?;
        }
        Ok(Self { raw })
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

    fn usd_from_bps(notional: Self, bps: i128) -> Result<Self, String> {
        let raw = notional
            .raw
            .checked_mul(bps)
            .and_then(|value| value.checked_div(10_000))
            .ok_or_else(|| "basis USD calculation overflowed".to_owned())?;
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct BasisBookTickerInput {
    event_id: String,
    best_bid: FixedDecimal,
    best_ask: FixedDecimal,
    is_stale: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BasisPremiumIndexInput {
    event_id: String,
    mark_price: FixedDecimal,
    index_price: FixedDecimal,
    last_funding_rate: String,
    next_funding_time_ms: String,
    is_stale: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BasisOpportunity {
    spot: BasisBookTickerInput,
    perp: BasisBookTickerInput,
    premium: BasisPremiumIndexInput,
    quantity: FixedDecimal,
    gross_bps: i128,
    total_cost_bps: i128,
    net_bps: i128,
    expected_profit_usd: FixedDecimal,
    fee_estimate_usd: FixedDecimal,
    slippage_estimate_usd: FixedDecimal,
}

fn latest_basis_book_ticker(
    context: &dyn StrategyReadContext,
    venue_id: &str,
    instrument_id: &str,
    basis_role: &str,
) -> Result<BasisBookTickerInput, String> {
    let event = context
        .snapshot()
        .market_events()
        .iter()
        .rev()
        .find(|event| is_basis_book_ticker_event(event, venue_id, instrument_id, basis_role))
        .ok_or_else(|| {
            format!(
                "missing Binance {basis_role} BookTicker event for venue {venue_id} instrument {instrument_id}"
            )
        })?;

    Ok(BasisBookTickerInput {
        event_id: event.event_id.as_str().to_owned(),
        best_bid: required_payload_fixed_decimal(event, "best_bid")?,
        best_ask: required_payload_fixed_decimal(event, "best_ask")?,
        is_stale: payload_string(event, "risk_reason_code") == Some("DATA_STALE"),
    })
}

fn latest_basis_premium_index(
    context: &dyn StrategyReadContext,
) -> Result<BasisPremiumIndexInput, String> {
    let event = context
        .snapshot()
        .market_events()
        .iter()
        .rev()
        .find(is_basis_premium_index_event)
        .ok_or_else(|| {
            format!(
                "missing Binance PerpPremiumIndex event for venue {BASIS_PERP_VENUE_ID} instrument {BASIS_PERP_INSTRUMENT_ID}"
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

fn is_basis_premium_index_event(event: &&NormalizedEvent) -> bool {
    event.event_type == NormalizedEventType::NormalizedMarketDataEvent
        && nullable_identifier_matches(&event.venue_id, BASIS_PERP_VENUE_ID)
        && nullable_identifier_matches(&event.instrument_id, BASIS_PERP_INSTRUMENT_ID)
        && payload_string(event, "kind").is_some_and(|kind| kind == "PerpPremiumIndex")
}

fn required_payload_fixed_decimal(
    event: &NormalizedEvent,
    field: &'static str,
) -> Result<FixedDecimal, String> {
    let value = required_payload_decimal_string(event, field)?;
    FixedDecimal::parse_non_negative(field, &value)
}

fn gross_basis_bps(perp_bid: FixedDecimal, spot_ask: FixedDecimal) -> Result<i128, String> {
    if spot_ask.raw <= 0 {
        return Err("spot ask price must be greater than zero".to_owned());
    }
    perp_bid
        .raw
        .checked_sub(spot_ask.raw)
        .and_then(|value| value.checked_mul(10_000))
        .and_then(|value| value.checked_div(spot_ask.raw))
        .ok_or_else(|| "basis bps calculation overflowed".to_owned())
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

fn validate_market_decimal(field: &'static str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("market quote field `{field}` cannot be empty"));
    }

    let mut dot_seen = false;
    let mut digits_seen = false;
    for byte in value.bytes() {
        match byte {
            b'0'..=b'9' => digits_seen = true,
            b'.' if !dot_seen => dot_seen = true,
            _ => {
                return Err(format!(
                    "market quote field `{field}` must be a non-negative decimal string"
                ));
            }
        }
    }

    if !digits_seen || value.starts_with('.') || value.ends_with('.') {
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
                BASIS_SPOT_VENUE_ID,
                BASIS_SPOT_INSTRUMENT_ID,
                "Spot",
                "99.90",
                "100.00",
                "CHECK_PASSED",
            ),
            basis_book_event(
                "perp",
                BASIS_PERP_VENUE_ID,
                BASIS_PERP_INSTRUMENT_ID,
                "Perp",
                "101.00",
                "101.10",
                "CHECK_PASSED",
            ),
            basis_premium_event("0.00010000", "CHECK_PASSED"),
        ]);

        let evaluation = strategy.evaluate(&context).expect("evaluation");

        let candidate = evaluation.candidate().expect("candidate");
        assert_eq!(candidate.transition_id.as_str(), BASIS_TRANSITION_ID);
        assert_eq!(candidate.legs.len(), 2);
        assert_eq!(
            candidate.expected_economics.expected_profit_bps.as_str(),
            "80"
        );
        assert_eq!(
            candidate.expected_economics.expected_profit_usd.as_str(),
            "0.8"
        );
        assert_eq!(
            candidate.expected_economics.fee_estimate_usd.as_str(),
            "0.15"
        );
        assert_eq!(
            candidate.expected_economics.slippage_estimate_usd.as_str(),
            "0.05"
        );
        assert_eq!(payload_constraint(candidate, "net_basis_bps"), Some("80"));
        assert!(candidate
            .risk_flags
            .iter()
            .any(|flag| flag.as_str() == "OneLegExecutionRisk"));
        assert!(evaluation.rejection().is_none());
        assert_eq!(
            evaluation.diagnostics()[0].code(),
            "SPOT_PERP_BASIS_CANDIDATE"
        );
    }

    #[test]
    fn spot_perp_basis_strategy_rejects_when_costs_remove_opportunity() {
        let strategy = SpotPerpBasisStrategy::new().expect("strategy");
        let context = basis_test_context(vec![
            basis_book_event(
                "spot",
                BASIS_SPOT_VENUE_ID,
                BASIS_SPOT_INSTRUMENT_ID,
                "Spot",
                "99.90",
                "100.00",
                "CHECK_PASSED",
            ),
            basis_book_event(
                "perp",
                BASIS_PERP_VENUE_ID,
                BASIS_PERP_INSTRUMENT_ID,
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
                BASIS_SPOT_VENUE_ID,
                BASIS_SPOT_INSTRUMENT_ID,
                "Spot",
                "99.90",
                "100.00",
                "DATA_STALE",
            ),
            basis_book_event(
                "perp",
                BASIS_PERP_VENUE_ID,
                BASIS_PERP_INSTRUMENT_ID,
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
    fn spot_perp_basis_signal_reuses_strategy_math_for_monitoring() {
        let signal = evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
            symbol: "BTCUSDT".to_owned(),
            spot_best_bid: "99.90".to_owned(),
            spot_best_ask: "100.00".to_owned(),
            perp_best_bid: "101.00".to_owned(),
            perp_best_ask: "101.10".to_owned(),
            notional_usd: "100.00".to_owned(),
            spot_taker_fee_bps: 10,
            perp_taker_fee_bps: 5,
            slippage_buffer_bps: 5,
            min_net_bps: 5,
        })
        .expect("signal");

        assert!(signal.is_candidate);
        assert_eq!(signal.gross_bps, 100);
        assert_eq!(signal.net_bps, 80);
        assert_eq!(signal.expected_profit_usd, "0.8");
        assert_eq!(signal.quantity, "1");
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
        match candidate.legs[0].constraints.get(key)? {
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
                    BASIS_SPOT_VENUE_ID,
                    MarketCapability::ProvidesSpotMarkets
                    | MarketCapability::ProvidesOrderBookMarkets,
                ) => self.has_basis_spot,
                (
                    BASIS_PERP_VENUE_ID,
                    MarketCapability::ProvidesPerpetuals
                    | MarketCapability::ProvidesOrderBookMarkets,
                ) => self.has_basis_perp,
                (BASIS_PERP_VENUE_ID, MarketCapability::ProvidesFundingRates) => {
                    self.has_basis_funding
                }
                _ => false,
            }
        }

        fn has_data_surface(&self, venue_id: &str, surface: &DataSurface) -> bool {
            if *surface != DataSurface::RestPolling {
                return false;
            }
            match venue_id {
                SAMPLE_VENUE_ID => self.has_rest,
                BASIS_SPOT_VENUE_ID | BASIS_PERP_VENUE_ID => self.has_basis_rest,
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
            matches!(strategy_id, SAMPLE_STRATEGY_ID | BASIS_STRATEGY_ID) && self.disabled_strategy
        }

        fn venue_disabled(&self, venue_id: &str) -> bool {
            matches!(
                venue_id,
                SAMPLE_VENUE_ID | BASIS_SPOT_VENUE_ID | BASIS_PERP_VENUE_ID
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
    "best_ask": {},
    "best_bid": {},
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
  "venue_id": "venue:BINANCE-USDM",
  "instrument_id": "inst:BINANCE:BTCUSDT:USDM-PERP",
  "payload": {{
    "basis_role": "Perp",
    "index_price": "100.00",
    "kind": "PerpPremiumIndex",
    "last_funding_rate": {},
    "mark_price": "101.00",
    "next_funding_time_ms": 1767254400000,
    "risk_reason_code": {}
  }},
  "checksum": "sha256:fixture-basis-premium-index"
}}"#,
            json_string(last_funding_rate),
            json_string(risk_reason_code),
        ))
        .expect("basis premium event")
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
