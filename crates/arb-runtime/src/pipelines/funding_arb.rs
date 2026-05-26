use arb_domain::UtcTimestamp;
use arb_replay::ReplayInput;

use crate::*;

/// 跨交易所资金费率套利管线实例输入。
///
/// 中文说明：该规格只描述只读策略配置、公开 venue capability 和 dry-run
/// 组合状态引用；不包含真实账户凭证或可变执行授权。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrossExchangeFundingArbPipelineSpec {
    pub strategy_config: CrossExchangeFundingArbStrategyConfig,
    pub venue_capabilities: Vec<VenueCapabilityDescriptor>,
    pub portfolio_state_id: String,
    pub portfolio_state_hash: String,
}

impl CrossExchangeFundingArbPipelineSpec {
    pub fn new(
        strategy_config: CrossExchangeFundingArbStrategyConfig,
        venue_capabilities: Vec<VenueCapabilityDescriptor>,
        portfolio_state_id: impl Into<String>,
        portfolio_state_hash: impl Into<String>,
    ) -> RuntimeResult<Self> {
        if venue_capabilities.is_empty() {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: "funding arb pipeline spec requires at least one venue capability"
                    .to_owned(),
            });
        }

        let missing_venues = [
            strategy_config.venues.venue_a.venue_id.as_str(),
            strategy_config.venues.venue_b.venue_id.as_str(),
        ]
        .into_iter()
        .filter(|required_venue| {
            !venue_capabilities
                .iter()
                .any(|capability| capability.venue_id.as_str() == *required_venue)
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
        if !missing_venues.is_empty() {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: format!(
                    "funding arb pipeline spec is missing venue capabilities for {}",
                    missing_venues.join(", ")
                ),
            });
        }

        let portfolio_state_id = portfolio_state_id.into();
        if portfolio_state_id.trim().is_empty() {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: "funding arb pipeline spec requires a portfolio_state_id".to_owned(),
            });
        }
        let portfolio_state_hash = portfolio_state_hash.into();
        if portfolio_state_hash.trim().is_empty() {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: "funding arb pipeline spec requires a portfolio_state_hash".to_owned(),
            });
        }

        CrossExchangeFundingArbStrategy::with_config(strategy_config.clone())?;
        Ok(Self {
            strategy_config,
            venue_capabilities,
            portfolio_state_id,
            portfolio_state_hash,
        })
    }

    pub fn binance_bybit_btcusdt() -> RuntimeResult<Self> {
        let mut venue_capabilities = arb_venue_capability_descriptors("binance")?;
        venue_capabilities.extend(arb_venue_capability_descriptors("bybit")?);
        Self::new(
            CrossExchangeFundingArbStrategyConfig::binance_bybit_btcusdt(),
            venue_capabilities,
            "state:funding-arb-public-readonly-01",
            "hash:funding-arb-public-readonly-01",
        )
    }
}

/// 使用已标准化的公开 funding arb 事件进入策略、风控和 dry-run 报告链路。
pub fn assemble_public_funding_arb_pipeline_from_normalized_events(
    replay: &ReplayInput,
    spec: &CrossExchangeFundingArbPipelineSpec,
    events: Vec<NormalizedEvent>,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<EndToEndArtifacts> {
    let _temp_dir = RuntimeTempDir::new()?;
    let event_store = JsonlEventStore::open(_temp_dir.path().join("events.jsonl"));
    for event in &events {
        event_store.append(event)?;
    }

    let stored_events = event_store.read_all_ordered()?;
    let source_event_refs = events
        .iter()
        .filter(|event| event.event_type == NormalizedEventType::NormalizedMarketDataEvent)
        .map(|event| event.event_id.as_str().to_owned())
        .collect::<Vec<_>>();
    let portfolio_state =
        build_public_funding_arb_portfolio_state(spec, &source_event_refs, ingested_at)?;
    ensure_portfolio_state_source_refs_exist(&portfolio_state, &stored_events)?;
    let fixed_time = ingested_at.to_string();
    let evaluation = run_cross_exchange_funding_arb_strategy(
        replay.config(),
        &stored_events,
        &portfolio_state,
        &fixed_time,
        spec,
    )?;
    let Some(candidate) = evaluation.candidate().cloned() else {
        let operations_report =
            run_operations_report(&event_store, &[], &[], &[], &[], &[], &fixed_time)?;
        return Ok(EndToEndArtifacts {
            replay_smoke_txt: replay.run_smoke_replay().to_stable_text(),
            stored_events_jsonl: stored_events_jsonl(&stored_events),
            candidate_transitions_jsonl: String::new(),
            risk_decisions_jsonl: String::new(),
            execution_plans_jsonl: String::new(),
            execution_reports_jsonl: String::new(),
            ledger_entries_jsonl: String::new(),
            reconciliation_reports_jsonl: String::new(),
            incidents_jsonl: String::new(),
            operations_daily_report_md: operations_report,
        });
    };

    let risk_decision = run_risk(
        &candidate,
        &portfolio_state,
        replay.config(),
        &spec.venue_capabilities,
        ingested_at,
    )?;

    if !risk_decision_allows_execution(&risk_decision) {
        let incidents = incidents_from_risk_rejection(&candidate, &risk_decision, &fixed_time)?;
        let operations_report = run_operations_report(
            &event_store,
            std::slice::from_ref(&risk_decision),
            &[],
            &[],
            &[],
            &incidents,
            &fixed_time,
        )?;

        return Ok(EndToEndArtifacts {
            replay_smoke_txt: replay.run_smoke_replay().to_stable_text(),
            stored_events_jsonl: stored_events_jsonl(&stored_events),
            candidate_transitions_jsonl: canonical_jsonl(std::slice::from_ref(&candidate)),
            risk_decisions_jsonl: canonical_jsonl(std::slice::from_ref(&risk_decision)),
            execution_plans_jsonl: String::new(),
            execution_reports_jsonl: String::new(),
            ledger_entries_jsonl: String::new(),
            reconciliation_reports_jsonl: String::new(),
            incidents_jsonl: canonical_jsonl(&incidents),
            operations_daily_report_md: operations_report,
        });
    }

    let execution_plan =
        run_execution_plan(&candidate, &risk_decision, replay.config(), &fixed_time)?;
    let execution_report = simulate_execution(&execution_plan, &fixed_time)?;
    let contract_ledger_entries =
        simulated_ledger_entries_from_execution_report(&execution_plan, &execution_report)?;
    let domain_ledger_entries = append_to_simulated_ledger(&contract_ledger_entries)?;
    let fill_snapshots = fill_snapshots_from_report(&execution_report, &contract_ledger_entries)?;
    let reconciliation_report =
        run_reconciliation(ingested_at, &domain_ledger_entries, &fill_snapshots)?;
    let operations_report = run_operations_report(
        &event_store,
        std::slice::from_ref(&risk_decision),
        std::slice::from_ref(&execution_report),
        &contract_ledger_entries,
        std::slice::from_ref(&reconciliation_report),
        &[],
        &fixed_time,
    )?;

    Ok(EndToEndArtifacts {
        replay_smoke_txt: replay.run_smoke_replay().to_stable_text(),
        stored_events_jsonl: stored_events_jsonl(&stored_events),
        candidate_transitions_jsonl: canonical_jsonl(std::slice::from_ref(&candidate)),
        risk_decisions_jsonl: canonical_jsonl(std::slice::from_ref(&risk_decision)),
        execution_plans_jsonl: canonical_jsonl(std::slice::from_ref(&execution_plan)),
        execution_reports_jsonl: canonical_jsonl(std::slice::from_ref(&execution_report)),
        ledger_entries_jsonl: canonical_jsonl(&contract_ledger_entries),
        reconciliation_reports_jsonl: jsonl_from_lines(vec![stable_reconciliation_report_json(
            &reconciliation_report,
        )]),
        incidents_jsonl: String::new(),
        operations_daily_report_md: operations_report,
    })
}

pub(crate) fn funding_arb_pipeline_spec_from_monitor_row(
    row: &FundingArbMarketRow,
    options: &FundingArbGuardedDryRunOnceOptions,
) -> RuntimeResult<CrossExchangeFundingArbPipelineSpec> {
    let mut config = CrossExchangeFundingArbStrategyConfig::binance_bybit_btcusdt();
    let symbol = funding_display_symbol(&funding_base_asset_from_symbol(&row.symbol));
    let symbol_component = basis_identifier_component(&symbol).to_ascii_lowercase();
    let venue_a_family = normalize_venue_family(&row.venue_a_family);
    let venue_b_family = normalize_venue_family(&row.venue_b_family);
    config.instance.venue_family_label = format!("{}-{}", row.venue_a_family, row.venue_b_family);
    config.symbol.symbol = symbol.clone();
    config.symbol.base_asset_id = format!(
        "asset:{}",
        basis_identifier_component(&funding_base_asset_from_symbol(&symbol)).to_ascii_uppercase()
    );
    config.symbol.settlement_asset_id = "asset:USDT".to_owned();
    config.venues.venue_a = funding_arb_leg_config(&venue_a_family, &symbol)?;
    config.venues.venue_b = funding_arb_leg_config(&venue_b_family, &symbol)?;
    config.economics.notional_usd = options.notional_usd.clone();
    config.economics.venue_a_taker_fee_bps = options.taker_fee_bps;
    config.economics.venue_b_taker_fee_bps = options.taker_fee_bps;
    config.economics.slippage_buffer_bps = options.slippage_buffer_bps;
    config.economics.max_entry_price_divergence_bps = options.max_entry_price_divergence_bps;
    config.economics.min_net_funding_bps = options.min_net_funding_bps;
    config.output.transition_id = format!(
        "trans:cross-exchange-funding-arb-{}-{}-{}-observer",
        basis_identifier_component(&venue_a_family),
        basis_identifier_component(&venue_b_family),
        symbol_component
    );
    config.output.assumption_id = format!(
        "asm:cross-exchange-funding-arb-{}-{}-public-readonly",
        basis_identifier_component(&venue_a_family),
        basis_identifier_component(&venue_b_family)
    );

    let mut venue_capabilities = arb_venue_capability_descriptors(&venue_a_family)?;
    venue_capabilities.extend(arb_venue_capability_descriptors(&venue_b_family)?);
    CrossExchangeFundingArbPipelineSpec::new(
        config,
        venue_capabilities,
        format!(
            "state:funding-arb-{}-{}-{}-readonly",
            basis_identifier_component(&venue_a_family),
            basis_identifier_component(&venue_b_family),
            symbol_component
        ),
        format!(
            "hash:funding-arb-{}-{}-{}-readonly",
            basis_identifier_component(&venue_a_family),
            basis_identifier_component(&venue_b_family),
            symbol_component
        ),
    )
}
