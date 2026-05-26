use std::path::{Path, PathBuf};

use arb_domain::UtcTimestamp;
use arb_replay::ReplayInput;

use crate::*;

const REPLAY_SMOKE_FILE: &str = "replay_smoke.txt";
const STORED_EVENTS_FILE: &str = "stored_events.jsonl";
const CANDIDATE_TRANSITIONS_FILE: &str = "candidate_transitions.jsonl";
const RISK_DECISIONS_FILE: &str = "risk_decisions.jsonl";
const EXECUTION_PLANS_FILE: &str = "execution_plans.jsonl";
const EXECUTION_REPORTS_FILE: &str = "execution_reports.jsonl";
const LEDGER_ENTRIES_FILE: &str = "ledger_entries.jsonl";
const RECONCILIATION_REPORTS_FILE: &str = "reconciliation_reports.jsonl";
const INCIDENTS_FILE: &str = "incidents.jsonl";
const OPERATIONS_DAILY_REPORT_FILE: &str = "operations_daily_report.md";

/// 单次端到端运行结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndToEndRunReport {
    pub fixture_root: PathBuf,
    pub artifacts: EndToEndArtifacts,
    pub comparisons: Vec<GoldenComparison>,
}

/// 单次真实只读行情 + 模拟执行运行结果。
///
/// 中文说明：该报告只表示公开行情被读取并进入模拟管线；它不包含真实下单、
/// 撤单、转账、签名或任何真实账户变化。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveMarketSimulationReport {
    pub fixture_root: PathBuf,
    pub symbol: String,
    pub source_url: String,
    pub ingested_at: String,
    pub artifacts: EndToEndArtifacts,
    pub output_dir: Option<PathBuf>,
}

/// 端到端管线的稳定输出。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndToEndArtifacts {
    pub replay_smoke_txt: String,
    pub stored_events_jsonl: String,
    pub candidate_transitions_jsonl: String,
    pub risk_decisions_jsonl: String,
    pub execution_plans_jsonl: String,
    pub execution_reports_jsonl: String,
    pub ledger_entries_jsonl: String,
    pub reconciliation_reports_jsonl: String,
    pub incidents_jsonl: String,
    pub operations_daily_report_md: String,
}

impl EndToEndArtifacts {
    fn files(&self) -> Vec<ArtifactFile<'_>> {
        vec![
            ArtifactFile::new("replay smoke", REPLAY_SMOKE_FILE, &self.replay_smoke_txt),
            ArtifactFile::new(
                "stored events",
                STORED_EVENTS_FILE,
                &self.stored_events_jsonl,
            ),
            ArtifactFile::new(
                "candidate transitions",
                CANDIDATE_TRANSITIONS_FILE,
                &self.candidate_transitions_jsonl,
            ),
            ArtifactFile::new(
                "risk decisions",
                RISK_DECISIONS_FILE,
                &self.risk_decisions_jsonl,
            ),
            ArtifactFile::new(
                "execution plans",
                EXECUTION_PLANS_FILE,
                &self.execution_plans_jsonl,
            ),
            ArtifactFile::new(
                "execution reports",
                EXECUTION_REPORTS_FILE,
                &self.execution_reports_jsonl,
            ),
            ArtifactFile::new(
                "ledger entries",
                LEDGER_ENTRIES_FILE,
                &self.ledger_entries_jsonl,
            ),
            ArtifactFile::new(
                "reconciliation reports",
                RECONCILIATION_REPORTS_FILE,
                &self.reconciliation_reports_jsonl,
            ),
            ArtifactFile::new("incidents", INCIDENTS_FILE, &self.incidents_jsonl),
            ArtifactFile::new(
                "operations daily report",
                OPERATIONS_DAILY_REPORT_FILE,
                &self.operations_daily_report_md,
            ),
        ]
    }
}

struct ArtifactFile<'a> {
    artifact: &'static str,
    file_name: &'static str,
    contents: &'a str,
}

impl<'a> ArtifactFile<'a> {
    fn new(artifact: &'static str, file_name: &'static str, contents: &'a str) -> Self {
        Self {
            artifact,
            file_name,
            contents,
        }
    }
}

/// 运行默认 S9-01 固定 fixture 并比较黄金输出。
pub fn run_default_full_pipeline_fixture() -> RuntimeResult<EndToEndRunReport> {
    run_full_pipeline_fixture(DEFAULT_FULL_PIPELINE_FIXTURE)
}

/// 运行固定 fixture 并比较黄金输出。
pub fn run_full_pipeline_fixture(
    fixture_root: impl AsRef<Path>,
) -> RuntimeResult<EndToEndRunReport> {
    run_full_pipeline_fixture_with_options(fixture_root, RuntimeOptions::default())
}

/// 运行固定 fixture，可选择写入黄金输出。
pub fn run_full_pipeline_fixture_with_options(
    fixture_root: impl AsRef<Path>,
    options: RuntimeOptions,
) -> RuntimeResult<EndToEndRunReport> {
    let fixture_root = resolve_fixture_root(fixture_root.as_ref());
    let artifacts = assemble_full_pipeline(&fixture_root)?;
    let comparisons = if options.accept_golden {
        write_expected_artifacts(&fixture_root, &artifacts)?
    } else {
        compare_expected_artifacts(&fixture_root, &artifacts)?
    };
    Ok(EndToEndRunReport {
        fixture_root,
        artifacts,
        comparisons,
    })
}

pub(crate) fn assemble_full_pipeline(fixture_root: &Path) -> RuntimeResult<EndToEndArtifacts> {
    validate_full_pipeline_context(fixture_root)?;
    let replay = arb_replay::load_fixture(fixture_root)?;
    ensure_simulated_offline_config(replay.config())?;

    let fixed_timestamp = UtcTimestamp::parse_rfc3339_z(replay.time_source().now())?;
    assemble_full_pipeline_with_market_source(
        fixture_root,
        &replay,
        MarketDataSource::Fixture,
        fixed_timestamp,
    )
}

fn assemble_full_pipeline_with_market_source(
    fixture_root: &Path,
    replay: &ReplayInput,
    market_source: MarketDataSource<'_>,
    pipeline_time: UtcTimestamp,
) -> RuntimeResult<EndToEndArtifacts> {
    let fixed_time = pipeline_time.to_string();
    let _temp_dir = RuntimeTempDir::new()?;
    let event_store = JsonlEventStore::open(_temp_dir.path().join("events.jsonl"));

    for event in replay.events() {
        event_store.append(event)?;
    }
    let market_data = ingest_market_data_source(fixture_root, market_source, pipeline_time)?;
    for event in &market_data.events {
        event_store.append(event)?;
    }

    let stored_events = event_store.read_all_ordered()?;
    let portfolio_state = build_portfolio_state_from_fixture(
        fixture_root,
        &stored_events,
        market_data.portfolio_state_source_event_ref.as_deref(),
        market_data
            .portfolio_state_as_of
            .map(|timestamp| timestamp.to_string()),
    )?;
    let venue_capabilities = load_venue_capabilities(fixture_root)?;

    let candidate = run_strategy(
        replay,
        &stored_events,
        &portfolio_state,
        &venue_capabilities,
        &fixed_time,
    )?;
    let risk_decision = run_risk(
        &candidate,
        &portfolio_state,
        replay.config(),
        &venue_capabilities,
        pipeline_time,
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
        run_reconciliation(pipeline_time, &domain_ledger_entries, &fill_snapshots)?;
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

#[derive(Clone, Copy)]
enum MarketDataSource<'a> {
    Fixture,
    BinancePublicTicker24h {
        raw_json: &'a str,
        raw_response_ref: &'a str,
        ingested_at: UtcTimestamp,
    },
}

struct MarketDataEvents {
    events: Vec<NormalizedEvent>,
    portfolio_state_source_event_ref: Option<String>,
    portfolio_state_as_of: Option<UtcTimestamp>,
}

fn ingest_market_data_source(
    fixture_root: &Path,
    source: MarketDataSource<'_>,
    default_ingested_at: UtcTimestamp,
) -> RuntimeResult<MarketDataEvents> {
    match source {
        MarketDataSource::Fixture => Ok(MarketDataEvents {
            events: ingest_read_only_fixture_data(fixture_root, default_ingested_at)?,
            portfolio_state_source_event_ref: None,
            portfolio_state_as_of: None,
        }),
        MarketDataSource::BinancePublicTicker24h {
            raw_json,
            raw_response_ref,
            ingested_at,
        } => {
            let events =
                ingest_binance_public_ticker_json(raw_json, raw_response_ref, ingested_at)?;
            let live_event_ref = events
                .last()
                .map(|event| event.event_id.as_str().to_owned())
                .ok_or_else(|| RuntimeError::LiveMarketData {
                    message: "live ticker ingestion produced no normalized event".to_owned(),
                })?;
            Ok(MarketDataEvents {
                events,
                portfolio_state_source_event_ref: Some(live_event_ref),
                portfolio_state_as_of: Some(ingested_at),
            })
        }
    }
}

/// 拉取一次公开真实行情并运行模拟管线。
///
/// 中文说明：当前命令只支持与主 fixture 对齐的 `BTCUSDT` 公共市场数据。
/// 它不使用 API key，不访问账户数据，不提交真实订单，也不会更新黄金文件。
pub fn run_live_market_simulation(
    fixture_root: impl AsRef<Path>,
    symbol: &str,
    output_dir: Option<PathBuf>,
) -> RuntimeResult<LiveMarketSimulationReport> {
    let symbol = validate_live_market_symbol(symbol)?;
    let fixture_root = resolve_fixture_root(fixture_root.as_ref());
    validate_full_pipeline_context(&fixture_root)?;
    let replay = arb_replay::load_fixture(&fixture_root)?;
    ensure_simulated_offline_config(replay.config())?;

    let source_url = binance_ticker_24h_url(&symbol);
    let raw_json = fetch_public_json_with_curl(&source_url)?;
    let ingested_at = current_utc_timestamp()?;
    let artifacts = assemble_full_pipeline_with_market_source(
        &fixture_root,
        &replay,
        MarketDataSource::BinancePublicTicker24h {
            raw_json: &raw_json,
            raw_response_ref: &source_url,
            ingested_at,
        },
        ingested_at,
    )?;

    if let Some(dir) = &output_dir {
        write_artifacts_to_dir(dir, &artifacts)?;
    }

    Ok(LiveMarketSimulationReport {
        fixture_root,
        symbol,
        source_url,
        ingested_at: ingested_at.to_string(),
        artifacts,
        output_dir,
    })
}

pub(crate) fn validate_full_pipeline_context(fixture_root: &Path) -> RuntimeResult<()> {
    ensure_context_file(
        fixture_root,
        RISK_POLICY_FILE,
        &["policy_version", "policy_hash", "policy_signature_ref"],
    )?;
    ensure_context_file(
        fixture_root,
        STRATEGY_MANIFEST_FILE,
        &["strategy_id", "strategy_version", "code_version"],
    )?;
    Ok(())
}

fn ensure_context_file(
    fixture_root: &Path,
    file_name: &'static str,
    required_markers: &[&str],
) -> RuntimeResult<()> {
    let path = fixture_root.join(file_name);
    if !path.is_file() {
        return Err(RuntimeError::MissingFixture { path });
    }
    let contents = read_utf8(&path)?;
    if contents.trim().is_empty() {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!("{file_name} must not be empty"),
        });
    }
    for marker in required_markers {
        if !contents.contains(marker) {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: format!("{file_name} is missing `{marker}`"),
            });
        }
    }
    Ok(())
}

pub(crate) fn ensure_simulated_offline_config(config: &arb_config::ArbConfig) -> RuntimeResult<()> {
    if config.execution().mode().requires_live_permission()
        || config.allows_account_changes()
        || config.signing().real_signing_enabled()
    {
        return Err(RuntimeError::UnsafeConfig {
            message:
                "S9-01 runtime fixture must stay simulated/offline and must not enable real signing"
                    .to_owned(),
        });
    }
    Ok(())
}

fn ingest_read_only_fixture_data(
    fixture_root: &Path,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<Vec<NormalizedEvent>> {
    let raw_path = fixture_root.join(RAW_TICKER_FILE);
    let raw_json = read_utf8(&raw_path)?;
    ingest_binance_public_ticker_json(&raw_json, RAW_TICKER_REF, ingested_at)
}

fn ingest_binance_public_ticker_json(
    raw_json: &str,
    raw_response_ref: &str,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<Vec<NormalizedEvent>> {
    let venue_id = VenueId::new(SIM_VENUE_ID)?;
    let instrument = BinancePublicInstrument::new(
        SIM_SYMBOL,
        InstrumentId::new(SIM_INSTRUMENT_ID)?,
        AssetId::new(SIM_BASE_ASSET_ID)?,
        AssetId::new(SIM_QUOTE_ASSET_ID)?,
        AssetId::new(SIM_SETTLEMENT_ASSET_ID)?,
    )?;
    let mut adapter = BinancePublicTicker24hAdapter::new(
        venue_id,
        instrument,
        ingested_at,
        MARKET_DATA_MAX_AGE_MS,
    )?;
    let batch = adapter.ingest_ticker_24h_json(raw_json, raw_response_ref, ingested_at)?;
    Ok(vec![batch.raw_event, batch.normalized_event])
}

fn write_expected_artifacts(
    fixture_root: &Path,
    artifacts: &EndToEndArtifacts,
) -> RuntimeResult<Vec<GoldenComparison>> {
    let expected_dir = fixture_root.join("expected");
    ensure_dir(&expected_dir)?;
    let mut comparisons = Vec::new();
    for artifact in artifacts.files() {
        let path = write_utf8_in_dir(&expected_dir, artifact.file_name, artifact.contents)?;
        comparisons.push(GoldenComparison {
            artifact: artifact.artifact,
            path,
            bytes: artifact.contents.len(),
        });
    }
    Ok(comparisons)
}

pub(crate) fn write_artifacts_to_dir(
    output_dir: &Path,
    artifacts: &EndToEndArtifacts,
) -> RuntimeResult<Vec<GoldenComparison>> {
    ensure_dir(output_dir)?;
    let mut written = Vec::new();
    for artifact in artifacts.files() {
        let path = write_utf8_in_dir(output_dir, artifact.file_name, artifact.contents)?;
        written.push(GoldenComparison {
            artifact: artifact.artifact,
            path,
            bytes: artifact.contents.len(),
        });
    }
    Ok(written)
}

fn compare_expected_artifacts(
    fixture_root: &Path,
    artifacts: &EndToEndArtifacts,
) -> RuntimeResult<Vec<GoldenComparison>> {
    let expected_dir = fixture_root.join("expected");
    let mut comparisons = Vec::new();
    for artifact in artifacts.files() {
        let path = expected_dir.join(artifact.file_name);
        if !path.exists() {
            return Err(RuntimeError::MissingFixture { path });
        }
        let expected = read_utf8(&path)?;
        if expected != artifact.contents {
            return Err(RuntimeError::GoldenMismatch {
                artifact: artifact.artifact,
                path,
                expected_bytes: expected.len(),
                actual_bytes: artifact.contents.len(),
                first_difference: first_difference(
                    expected.as_bytes(),
                    artifact.contents.as_bytes(),
                ),
            });
        }
        comparisons.push(GoldenComparison {
            artifact: artifact.artifact,
            path,
            bytes: artifact.contents.len(),
        });
    }
    Ok(comparisons)
}

fn first_difference(left: &[u8], right: &[u8]) -> Option<usize> {
    left.iter()
        .zip(right.iter())
        .position(|(left, right)| left != right)
        .or_else(|| (left.len() != right.len()).then_some(left.len().min(right.len())))
}
