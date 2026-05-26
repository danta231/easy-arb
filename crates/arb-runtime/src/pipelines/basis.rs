use std::path::{Path, PathBuf};

use arb_domain::UtcTimestamp;
use arb_replay::ReplayInput;

use super::full_pipeline::{
    ensure_simulated_offline_config, validate_full_pipeline_context, write_artifacts_to_dir,
};
use crate::*;

/// 单次 Binance 现货-永续 basis 正式模拟管线结果。
///
/// 中文说明：该报告把公开 basis 行情推进到候选转换、风控、模拟执行或拒绝事故、
/// 账本、对账和运营报告。它不读取私有账户、不下单、不撤单、不转账、不签名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceBasisPipelineReport {
    pub fixture_root: PathBuf,
    pub symbol: String,
    pub spot_book_ticker_url: String,
    pub perp_book_ticker_url: String,
    pub premium_index_url: String,
    pub ingested_at: String,
    pub artifacts: EndToEndArtifacts,
    pub output_dir: Option<PathBuf>,
}

/// 单次 Bybit 现货-线性永续 basis 正式模拟管线结果。
///
/// 中文说明：字段语义与 Binance 管线报告一致，公共行情来自 Bybit V5 ticker；
/// 后续仍进入同一条只读策略、风控和模拟执行管线。
pub type BybitBasisPipelineReport = BinanceBasisPipelineReport;

/// 拉取一次 Binance 公开 spot/perp basis 数据并进入正式模拟管线。
///
/// 中文说明：该入口使用公开 REST 数据创建标准化事件和候选转换，然后继续经过
/// 风控、执行计划、模拟执行、模拟账本、对账和运营报告。缺私有余额或未知状态
/// 必须被风控拒绝，不能当作通过。
pub fn run_binance_basis_pipeline(
    symbol: &str,
    output_dir: Option<PathBuf>,
) -> RuntimeResult<BinanceBasisPipelineReport> {
    let symbol = validate_binance_basis_symbol(symbol)?;
    let fixture_root = resolve_fixture_root(Path::new(DEFAULT_FULL_PIPELINE_FIXTURE));
    validate_full_pipeline_context(&fixture_root)?;
    let replay = arb_replay::load_fixture(&fixture_root)?;
    ensure_simulated_offline_config(replay.config())?;

    let spot_book_ticker_url = binance_spot_book_ticker_url(&symbol);
    let perp_book_ticker_url = binance_usdm_book_ticker_url(&symbol);
    let premium_index_url = binance_usdm_premium_index_url(&symbol);
    let raw_spot_book = fetch_public_json_with_curl(&spot_book_ticker_url)?;
    let raw_perp_book = fetch_public_json_with_curl(&perp_book_ticker_url)?;
    let raw_premium_index = fetch_public_json_with_curl(&premium_index_url)?;
    let ingested_at = current_utc_timestamp()?;

    let artifacts = assemble_binance_basis_pipeline_from_raw_json(
        &replay,
        BinanceBasisRawInputs {
            symbol: &symbol,
            raw_spot_book: &raw_spot_book,
            spot_book_ref: &spot_book_ticker_url,
            raw_perp_book: &raw_perp_book,
            perp_book_ref: &perp_book_ticker_url,
            raw_premium_index: &raw_premium_index,
            premium_index_ref: &premium_index_url,
        },
        ingested_at,
    )?;

    if let Some(dir) = &output_dir {
        write_artifacts_to_dir(dir, &artifacts)?;
    }

    Ok(BinanceBasisPipelineReport {
        fixture_root,
        symbol,
        spot_book_ticker_url,
        perp_book_ticker_url,
        premium_index_url,
        ingested_at: ingested_at.to_string(),
        artifacts,
        output_dir,
    })
}

/// 拉取一次 Bybit 公开 spot/linear-perp basis 数据并进入正式模拟管线。
///
/// 中文说明：该入口使用公开 REST 数据创建标准化事件和候选转换，然后继续经过
/// 风控、执行计划、模拟执行、模拟账本、对账和运营报告。缺私有余额或未知状态
/// 必须被风控拒绝，不能当作通过。
pub fn run_bybit_basis_pipeline(
    symbol: &str,
    output_dir: Option<PathBuf>,
) -> RuntimeResult<BybitBasisPipelineReport> {
    let symbol = validate_bybit_basis_symbol(symbol)?;
    let fixture_root = resolve_fixture_root(Path::new(DEFAULT_FULL_PIPELINE_FIXTURE));
    validate_full_pipeline_context(&fixture_root)?;
    let replay = arb_replay::load_fixture(&fixture_root)?;
    ensure_simulated_offline_config(replay.config())?;

    let spot_book_ticker_url = bybit_spot_tickers_url();
    let perp_book_ticker_url = bybit_linear_tickers_url();
    let premium_index_url = perp_book_ticker_url.clone();
    let raw_spot_ticker = fetch_public_json_with_curl(&spot_book_ticker_url)?;
    let raw_linear_ticker = fetch_public_json_with_curl(&perp_book_ticker_url)?;
    let ingested_at = current_utc_timestamp()?;

    let artifacts = assemble_bybit_basis_pipeline_from_raw_json(
        &replay,
        BybitBasisRawInputs {
            symbol: &symbol,
            raw_spot_ticker: &raw_spot_ticker,
            spot_ticker_ref: &spot_book_ticker_url,
            raw_linear_ticker: &raw_linear_ticker,
            linear_ticker_ref: &perp_book_ticker_url,
        },
        ingested_at,
    )?;

    if let Some(dir) = &output_dir {
        write_artifacts_to_dir(dir, &artifacts)?;
    }

    Ok(BybitBasisPipelineReport {
        fixture_root,
        symbol,
        spot_book_ticker_url,
        perp_book_ticker_url,
        premium_index_url,
        ingested_at: ingested_at.to_string(),
        artifacts,
        output_dir,
    })
}

/// basis 管线实例输入。
///
/// 中文说明：运行时只依赖这里注入的策略配置和场所能力，不再在策略执行入口
/// 隐式选择具体交易所。调用方可以用 fixture 读取出的 capability JSONL 组装该
/// 规格，也可以使用内置的 Binance/Bybit 兼容默认值做 smoke test。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BasisPipelineSpec {
    pub strategy_config: SpotPerpBasisStrategyConfig,
    pub venue_capabilities: Vec<VenueCapabilityDescriptor>,
    pub portfolio_state_id: String,
    pub portfolio_state_hash: String,
}

impl BasisPipelineSpec {
    /// 使用显式策略配置和场所能力创建 basis 管线实例。
    pub fn new(
        strategy_config: SpotPerpBasisStrategyConfig,
        venue_capabilities: Vec<VenueCapabilityDescriptor>,
        portfolio_state_id: impl Into<String>,
        portfolio_state_hash: impl Into<String>,
    ) -> RuntimeResult<Self> {
        if venue_capabilities.is_empty() {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: "basis pipeline spec requires at least one venue capability".to_owned(),
            });
        }

        let missing_venues = [
            strategy_config.symbol.spot.venue_id.as_str(),
            strategy_config.symbol.perp.venue_id.as_str(),
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
                    "basis pipeline spec is missing venue capabilities for {}",
                    missing_venues.join(", ")
                ),
            });
        }

        let portfolio_state_id = portfolio_state_id.into();
        if portfolio_state_id.trim().is_empty() {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: "basis pipeline spec requires a portfolio_state_id".to_owned(),
            });
        }
        let portfolio_state_hash = portfolio_state_hash.into();
        if portfolio_state_hash.trim().is_empty() {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: "basis pipeline spec requires a portfolio_state_hash".to_owned(),
            });
        }

        SpotPerpBasisStrategy::with_config(strategy_config.clone())?;
        Ok(Self {
            strategy_config,
            venue_capabilities,
            portfolio_state_id,
            portfolio_state_hash,
        })
    }

    /// Binance BTCUSDT 兼容默认实例。
    pub fn binance_btcusdt() -> RuntimeResult<Self> {
        Self::new(
            SpotPerpBasisStrategyConfig::binance_btcusdt(),
            load_binance_basis_capabilities()?,
            "state:binance-basis-public-readonly-01",
            "hash:binance-basis-public-readonly-01",
        )
    }

    /// Bybit BTCUSDT 兼容默认实例。
    pub fn bybit_btcusdt() -> RuntimeResult<Self> {
        Self::new(
            SpotPerpBasisStrategyConfig::bybit_btcusdt(),
            load_bybit_basis_capabilities()?,
            "state:bybit-basis-public-readonly-01",
            "hash:bybit-basis-public-readonly-01",
        )
    }
}

pub(crate) fn assemble_binance_basis_pipeline_from_raw_json(
    replay: &ReplayInput,
    inputs: BinanceBasisRawInputs<'_>,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<EndToEndArtifacts> {
    let events = ingest_binance_basis_public_json(inputs, ingested_at)?;
    let spec = BasisPipelineSpec::binance_btcusdt()?;
    assemble_public_basis_pipeline_from_normalized_events(replay, &spec, events, ingested_at)
}

pub(crate) fn assemble_bybit_basis_pipeline_from_raw_json(
    replay: &ReplayInput,
    inputs: BybitBasisRawInputs<'_>,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<EndToEndArtifacts> {
    let events = ingest_bybit_basis_public_json(inputs, ingested_at)?;
    let spec = BasisPipelineSpec::bybit_btcusdt()?;
    assemble_public_basis_pipeline_from_normalized_events(replay, &spec, events, ingested_at)
}

/// 使用已标准化的 basis 公共行情事件进入策略、风控和后续模拟管线。
///
/// 中文说明：该入口不关心事件来自 Binance、Bybit 或其他 adapter，只要求调用方
/// 提供和策略配置一致的标准化行情事件与场所能力。缺私有余额等外部状态仍按
/// 风险状态处理，不能被当作通过。
pub fn assemble_public_basis_pipeline_from_normalized_events(
    replay: &ReplayInput,
    spec: &BasisPipelineSpec,
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
        build_public_basis_portfolio_state(spec, &source_event_refs, ingested_at)?;
    ensure_portfolio_state_source_refs_exist(&portfolio_state, &stored_events)?;
    let fixed_time = ingested_at.to_string();
    let evaluation = run_spot_perp_basis_strategy(
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
