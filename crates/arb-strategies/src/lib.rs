//! `arb-strategies` 样例策略集合。
//!
//! 中文说明：本 crate 只依赖 `arb-strategy-api`，策略只能读取只读上下文并
//! 输出 `CandidatePortfolioTransition` 或明确拒绝原因。这里不暴露下单、
//! 签名、转账、账本写入或运行时装配能力。

#![forbid(unsafe_code)]

use arb_strategy_api::{
    candidate_from_json_strict, validate_candidate_for_context, CandidatePortfolioTransition,
    DataSurface, MarketCapability, Strategy, StrategyApiResult, StrategyDiagnostic,
    StrategyEvaluation, StrategyMetadata, StrategyReadContext, StrategyRejectReason,
    StrategyRejection,
};

const SAMPLE_STRATEGY_ID: &str = "strat:sample-spot-demo";
const SAMPLE_STRATEGY_VERSION: &str = "1.0.0";
const SAMPLE_CODE_VERSION: &str = "code:sample-spot-demo-1";
const SAMPLE_VENUE_ID: &str = "venue:SIM";
const SAMPLE_TRANSITION_ID: &str = "trans:sample-spot-001";

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
    ) -> StrategyApiResult<CandidatePortfolioTransition> {
        let input_event_refs = source_event_refs(context);
        let input_event_refs_json = json_string_array(&input_event_refs);
        let config_version = context.config().config_version();
        let config_hash = context.config().config_hash();
        let assumption = format!(
            "Sample strategy used config {config_version} with hash {config_hash} and only emitted a candidate transition."
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
        "post_only": false
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

        let candidate = self.build_candidate(context)?;
        validate_candidate_for_context(context, self.metadata(), &candidate)?;
        let diagnostic = StrategyDiagnostic::new(
            "SAMPLE_SPOT_CANDIDATE",
            format!(
                "candidate {SAMPLE_TRANSITION_ID} emitted in {} mode",
                context.config().execution_mode().as_str()
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
        let expected =
            candidate_from_json_strict(EXPECTED_CANDIDATE).expect("expected candidate fixture");
        TestContext {
            snapshot: TestSnapshot {
                portfolio_state_id: expected.current_portfolio_state_ref.as_str().to_owned(),
                source_event_refs: expected.input_event_refs,
            },
            capabilities: TestCapabilities { has_spot, has_rest },
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
            &[]
        }
    }

    struct TestCapabilities {
        has_spot: bool,
        has_rest: bool,
    }

    impl VenueCapabilityReader for TestCapabilities {
        fn venue_capabilities(&self) -> &[VenueCapabilityDescriptor] {
            &[]
        }

        fn has_market_capability(&self, venue_id: &str, capability: &MarketCapability) -> bool {
            venue_id == SAMPLE_VENUE_ID
                && *capability == MarketCapability::ProvidesSpotMarkets
                && self.has_spot
        }

        fn has_data_surface(&self, venue_id: &str, surface: &DataSurface) -> bool {
            venue_id == SAMPLE_VENUE_ID && *surface == DataSurface::RestPolling && self.has_rest
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
            strategy_id == SAMPLE_STRATEGY_ID && self.disabled_strategy
        }

        fn venue_disabled(&self, venue_id: &str) -> bool {
            venue_id == SAMPLE_VENUE_ID && self.disabled_venue
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
