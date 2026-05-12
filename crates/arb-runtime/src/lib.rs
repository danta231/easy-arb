//! `arb-runtime` 端到端离线装配入口。
//!
//! 中文说明：运行时只负责把已有模块按固定 fixture 装配起来。策略规则、风控
//! 规则、账本规则和执行状态机规则仍由对应 crate 提供；本 crate 不连接真实
//! 交易 API，不下单、不撤单、不转账、不签名。

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arb_config::ExecutionMode as ConfigExecutionMode;
use arb_contracts::{
    from_json_strict, to_canonical_json, CandidatePortfolioTransition, CanonicalJson,
    ExecutionMode as ContractExecutionMode, ExecutionPlan, ExecutionReport, Incident,
    LedgerDirection as ContractLedgerDirection, LedgerEntry as ContractLedgerEntry,
    LedgerEntryType as ContractLedgerEntryType, LedgerNamespace as ContractLedgerNamespace,
    NormalizedEvent, NormalizedEventType, PortfolioState, RiskDecision, VenueCapabilityDescriptor,
};
use arb_domain::{
    AccountId, Amount, AssetId, CandidateTransitionId, Decimal, EventId, ExecutionPlanId,
    InstrumentId, LedgerEntryId, Price, Quantity, StrategyId, UtcTimestamp, VenueId,
};
use arb_eventstore::{EventReader, EventWriter, JsonlEventStore};
use arb_execution::{
    build_execution_plan, simulate_execution, simulated_ledger_entries_from_execution_report,
    ExecutionPlanBuildInput,
};
use arb_ledger::{
    AdjustmentReasonCode, IdempotencyKey, JournalEntryId, LedgerBook,
    LedgerDirection as DomainLedgerDirection, LedgerEntry as DomainLedgerEntry, LedgerEntryDraft,
    LedgerEntryHeader, LedgerEntryType as DomainLedgerEntryType, LedgerLeg, LedgerLegId,
    LedgerNamespace as DomainLedgerNamespace,
};
use arb_ops::{
    InMemoryOpsFactReader, OperationsFacts, OpsCommandOutput, OpsReadOnlyCommand, ReadOnlyOpsEngine,
};
use arb_reconciliation::{
    CoreReconciliationRunner, FeeAmount, FillId, FillSnapshot, ReconciliationReport,
    ReconciliationRequest, ReconciliationRunId, ReconciliationRunner,
};
use arb_replay::{ReplayInput, TimeSource as ReplayTimeSource};
use arb_risk::{RiskEvaluationInput, RiskEvaluator, StaticRiskEvaluator};
use arb_strategies::{
    evaluate_spot_perp_basis_signal, sample_spot_strategy, spot_perp_basis_strategy,
    SpotPerpBasisSignalInput,
};
use arb_strategy_api::{
    CandidateTransitionOutput, FixedTimeSource as StrategyFixedTimeSource, ReadOnlySnapshot,
    Strategy, StrategyConfigSnapshot, StrategyEvaluation, StrategyInput, VenueCapabilitySnapshot,
};
use arb_venue_data::{
    BinancePublicBookTickerAdapter, BinancePublicInstrument, BinancePublicMarket,
    BinancePublicTicker24hAdapter, BinanceUsdmPremiumIndexAdapter,
};

const DEFAULT_FULL_PIPELINE_FIXTURE: &str = "fixtures/replay/full_pipeline_simulated";
const BINANCE_MARKET_DATA_BASE_URL: &str = "https://data-api.binance.vision";
const BINANCE_SPOT_REST_BASE_URL: &str = "https://data-api.binance.vision";
const BINANCE_USDM_FUTURES_BASE_URL: &str = "https://fapi.binance.com";
const RAW_TICKER_FILE: &str = "raw/binance_ticker_24hr.redacted.json";
const RAW_TICKER_REF: &str = "raw/binance_ticker_24hr.redacted.json";
const RISK_POLICY_FILE: &str = "risk_policy.yaml";
const STRATEGY_MANIFEST_FILE: &str = "strategy_manifest.yaml";
const SIM_VENUE_ID: &str = "venue:SIM";
const SIM_SYMBOL: &str = "BTCUSDT";
const SIM_INSTRUMENT_ID: &str = "inst:BTC-USDC";
const SIM_BASE_ASSET_ID: &str = "asset:BTC";
const SIM_QUOTE_ASSET_ID: &str = "asset:USDC";
const SIM_SETTLEMENT_ASSET_ID: &str = "asset:USDC";
const BASIS_SYMBOL: &str = "BTCUSDT";
const BASIS_SPOT_VENUE_ID: &str = "venue:BINANCE-SPOT";
const BASIS_PERP_VENUE_ID: &str = "venue:BINANCE-USDM";
const BASIS_SPOT_INSTRUMENT_ID: &str = "inst:BINANCE:BTCUSDT:SPOT";
const BASIS_PERP_INSTRUMENT_ID: &str = "inst:BINANCE:BTCUSDT:USDM-PERP";
const BASIS_BASE_ASSET_ID: &str = "asset:BTC";
const BASIS_QUOTE_ASSET_ID: &str = "asset:USDT";
const BASIS_SETTLEMENT_ASSET_ID: &str = "asset:USDT";
const MARKET_DATA_MAX_AGE_MS: u64 = 5_000;
const BASIS_MONITOR_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8796";
const BASIS_MONITOR_DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
const BASIS_MONITOR_DEFAULT_MIN_ABS_FUNDING_RATE: &str = "0";
const BASIS_MONITOR_DEFAULT_NOTIONAL_USD: &str = "100.00";
const BASIS_MONITOR_DEFAULT_SPOT_TAKER_FEE_BPS: i128 = 10;
const BASIS_MONITOR_DEFAULT_PERP_TAKER_FEE_BPS: i128 = 5;
const BASIS_MONITOR_DEFAULT_SLIPPAGE_BUFFER_BPS: i128 = 5;
const BASIS_MONITOR_DEFAULT_MIN_NET_BPS: i128 = 5;
const RECONCILIATION_RUN_ID: &str = "recon:full-pipeline-simulated";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 运行时统一返回类型。
pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// 运行时装配错误。
#[derive(Debug)]
pub enum RuntimeError {
    Io {
        path: PathBuf,
        message: String,
    },
    Module {
        module: &'static str,
        message: String,
    },
    UnsafeConfig {
        message: String,
    },
    StartupRejected {
        reasons: Vec<String>,
    },
    MissingFixture {
        path: PathBuf,
    },
    LiveMarketData {
        message: String,
    },
    StrategyRejected {
        reason: String,
        detail: Option<String>,
    },
    GoldenMismatch {
        artifact: &'static str,
        path: PathBuf,
        expected_bytes: usize,
        actual_bytes: usize,
        first_difference: Option<usize>,
    },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Module { module, message } => write!(f, "{module}: {message}"),
            Self::UnsafeConfig { message } => write!(f, "unsafe runtime config: {message}"),
            Self::StartupRejected { reasons } => {
                write!(f, "runtime startup rejected: {}", reasons.join("; "))
            }
            Self::MissingFixture { path } => {
                write!(f, "{}: expected fixture file is missing", path.display())
            }
            Self::LiveMarketData { message } => write!(f, "live market data failed: {message}"),
            Self::StrategyRejected { reason, detail } => {
                write!(f, "strategy rejected candidate with reason `{reason}`")?;
                if let Some(detail) = detail {
                    write!(f, ": {detail}")?;
                }
                Ok(())
            }
            Self::GoldenMismatch {
                artifact,
                path,
                expected_bytes,
                actual_bytes,
                first_difference,
            } => write!(
                f,
                "{}: golden mismatch for {artifact}; expected {expected_bytes} bytes, got {actual_bytes} bytes, first difference at {:?}",
                path.display(),
                first_difference
            ),
        }
    }
}

impl Error for RuntimeError {}

macro_rules! module_error_from {
    ($source:ty, $module:literal) => {
        impl From<$source> for RuntimeError {
            fn from(error: $source) -> Self {
                Self::Module {
                    module: $module,
                    message: error.to_string(),
                }
            }
        }
    };
}

module_error_from!(arb_config::ConfigError, "arb-config");
module_error_from!(arb_contracts::ContractError, "arb-contracts");
module_error_from!(arb_domain::DomainError, "arb-domain");
module_error_from!(arb_eventstore::EventStoreError, "arb-eventstore");
module_error_from!(arb_execution::ExecutionError, "arb-execution");
module_error_from!(arb_ledger::LedgerError, "arb-ledger");
module_error_from!(arb_ops::OpsError, "arb-ops");
module_error_from!(
    arb_reconciliation::ReconciliationError,
    "arb-reconciliation"
);
module_error_from!(arb_replay::ReplayError, "arb-replay");
module_error_from!(arb_risk::RiskError, "arb-risk");
module_error_from!(arb_strategy_api::StrategyApiError, "arb-strategy-api");
module_error_from!(arb_venue_data::VenueDataError, "arb-venue-data");

/// 运行选项。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeOptions {
    pub accept_golden: bool,
}

/// 运行时启动检查状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeCheckStatus {
    Pass,
    Warning,
    Fail,
}

impl RuntimeCheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warning => "warning",
            Self::Fail => "fail",
        }
    }
}

/// 启动或健康检查结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeCheck {
    pub name: String,
    pub status: RuntimeCheckStatus,
    pub message: String,
}

impl RuntimeCheck {
    fn pass(name: &str, message: impl Into<String>) -> Self {
        Self::new(name, RuntimeCheckStatus::Pass, message)
    }

    fn warning(name: &str, message: impl Into<String>) -> Self {
        Self::new(name, RuntimeCheckStatus::Warning, message)
    }

    fn fail(name: &str, message: impl Into<String>) -> Self {
        Self::new(name, RuntimeCheckStatus::Fail, message)
    }

    fn new(name: &str, status: RuntimeCheckStatus, message: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            status,
            message: message.into(),
        }
    }
}

/// 运行时健康状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeHealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Stopped,
}

impl RuntimeHealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
            Self::Stopped => "stopped",
        }
    }
}

/// 运行时任务状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeTaskState {
    Running,
    Exited,
    Skipped,
    Failed,
}

impl RuntimeTaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

/// 任务退出原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeTaskExitReason {
    Completed,
    GracefulShutdown,
    StartupSkipped,
    Failed,
}

impl RuntimeTaskExitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::GracefulShutdown => "graceful_shutdown",
            Self::StartupSkipped => "startup_skipped",
            Self::Failed => "failed",
        }
    }
}

/// 单个运行时任务的可观测生命周期记录。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeTaskStatus {
    pub name: String,
    pub state: RuntimeTaskState,
    pub exit_reason: Option<RuntimeTaskExitReason>,
    pub detail: String,
}

impl RuntimeTaskStatus {
    fn running(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            state: RuntimeTaskState::Running,
            exit_reason: None,
            detail: detail.into(),
        }
    }

    fn exited(name: &str, reason: RuntimeTaskExitReason, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            state: RuntimeTaskState::Exited,
            exit_reason: Some(reason),
            detail: detail.into(),
        }
    }

    fn skipped(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            state: RuntimeTaskState::Skipped,
            exit_reason: Some(RuntimeTaskExitReason::StartupSkipped),
            detail: detail.into(),
        }
    }

    fn failed(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            state: RuntimeTaskState::Failed,
            exit_reason: Some(RuntimeTaskExitReason::Failed),
            detail: detail.into(),
        }
    }
}

/// 运行时健康快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeHealthSnapshot {
    pub status: RuntimeHealthStatus,
    pub config_hash: String,
    pub execution_mode: String,
    pub kill_switch_triggered: bool,
    pub mutable_execution_started: bool,
    pub shutdown_requested: bool,
    pub checks: Vec<RuntimeCheck>,
    pub tasks: Vec<RuntimeTaskStatus>,
}

impl RuntimeHealthSnapshot {
    pub fn task(&self, name: &str) -> Option<&RuntimeTaskStatus> {
        self.tasks.iter().find(|task| task.name == name)
    }
}

/// 优雅退出报告。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeShutdownReport {
    pub reason: String,
    pub exited_tasks: Vec<RuntimeTaskStatus>,
    pub health: RuntimeHealthSnapshot,
}

/// 已启动的运行时服务句柄。
///
/// 中文说明：该类型只记录装配层任务和健康状态；它不持有可变交易适配器，
/// 也不实现策略、风控、账本或执行状态机规则。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeService {
    config_hash: String,
    execution_mode: ConfigExecutionMode,
    kill_switch_triggered: bool,
    checks: Vec<RuntimeCheck>,
    tasks: RuntimeTaskRegistry,
    shutdown_requested: bool,
}

impl RuntimeService {
    pub fn health(&self) -> RuntimeHealthSnapshot {
        runtime_health_snapshot(
            &self.config_hash,
            self.execution_mode,
            self.kill_switch_triggered,
            self.shutdown_requested,
            &self.checks,
            self.tasks.statuses(),
        )
    }

    /// 请求优雅退出，并把仍在运行的装配任务标记为可观测退出。
    pub fn request_graceful_shutdown(
        &mut self,
        reason: impl Into<String>,
    ) -> RuntimeShutdownReport {
        let reason = reason.into();
        self.shutdown_requested = true;
        let exited_tasks = self.tasks.graceful_shutdown(&reason);
        RuntimeShutdownReport {
            reason,
            exited_tasks,
            health: self.health(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RuntimeTaskRegistry {
    tasks: Vec<RuntimeTaskStatus>,
}

impl RuntimeTaskRegistry {
    fn push_running(&mut self, name: &str, detail: impl Into<String>) {
        self.tasks.push(RuntimeTaskStatus::running(name, detail));
    }

    fn push_exited(
        &mut self,
        name: &str,
        reason: RuntimeTaskExitReason,
        detail: impl Into<String>,
    ) {
        self.tasks
            .push(RuntimeTaskStatus::exited(name, reason, detail));
    }

    fn push_skipped(&mut self, name: &str, detail: impl Into<String>) {
        self.tasks.push(RuntimeTaskStatus::skipped(name, detail));
    }

    fn push_failed(&mut self, name: &str, detail: impl Into<String>) {
        self.tasks.push(RuntimeTaskStatus::failed(name, detail));
    }

    fn statuses(&self) -> Vec<RuntimeTaskStatus> {
        self.tasks.clone()
    }

    fn graceful_shutdown(&mut self, reason: &str) -> Vec<RuntimeTaskStatus> {
        let mut exited = Vec::new();
        for task in &mut self.tasks {
            if task.state == RuntimeTaskState::Running {
                task.state = RuntimeTaskState::Exited;
                task.exit_reason = Some(RuntimeTaskExitReason::GracefulShutdown);
                task.detail = format!("graceful shutdown requested: {reason}");
                exited.push(task.clone());
            }
        }
        exited
    }
}

const TASK_STARTUP_CHECKS: &str = "startup-checks";
const TASK_READ_ONLY_INGEST: &str = "read-only-data-ingest";
const TASK_EVENT_STORE: &str = "event-store";
const TASK_HEALTH_REPORTER: &str = "health-reporter";
const TASK_SHUTDOWN_LISTENER: &str = "shutdown-listener";
const TASK_MUTABLE_EXECUTION: &str = "mutable-execution-adapter";

/// 黄金比较结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoldenComparison {
    pub artifact: &'static str,
    pub path: PathBuf,
    pub bytes: usize,
}

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

/// 单次 Binance 现货-永续 basis 只读扫描结果。
///
/// 中文说明：该报告只包含公开行情、标准化事件、策略候选或拒绝诊断；不包含
/// API key、账户余额、签名、下单、撤单或转账。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceBasisScanReport {
    pub symbol: String,
    pub spot_book_ticker_url: String,
    pub perp_book_ticker_url: String,
    pub premium_index_url: String,
    pub ingested_at: String,
    pub stored_events_jsonl: String,
    pub candidate_transitions_jsonl: String,
    pub rejection_reason: Option<String>,
    pub rejection_detail: Option<String>,
    pub diagnostics: Vec<String>,
    pub output_dir: Option<PathBuf>,
}

/// Binance basis 长驻监控选项。
///
/// 中文说明：监控仅使用公开行情，不读取账户，不提交订单。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceBasisMonitorOptions {
    pub bind_addr: String,
    pub poll_interval_secs: u64,
    pub min_abs_funding_rate: String,
    pub notional_usd: String,
    pub spot_taker_fee_bps: i128,
    pub perp_taker_fee_bps: i128,
    pub slippage_buffer_bps: i128,
    pub min_net_bps: i128,
    pub once: bool,
    pub output_dir: Option<PathBuf>,
}

impl Default for BinanceBasisMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: BASIS_MONITOR_DEFAULT_BIND_ADDR.to_owned(),
            poll_interval_secs: BASIS_MONITOR_DEFAULT_POLL_INTERVAL_SECS,
            min_abs_funding_rate: BASIS_MONITOR_DEFAULT_MIN_ABS_FUNDING_RATE.to_owned(),
            notional_usd: BASIS_MONITOR_DEFAULT_NOTIONAL_USD.to_owned(),
            spot_taker_fee_bps: BASIS_MONITOR_DEFAULT_SPOT_TAKER_FEE_BPS,
            perp_taker_fee_bps: BASIS_MONITOR_DEFAULT_PERP_TAKER_FEE_BPS,
            slippage_buffer_bps: BASIS_MONITOR_DEFAULT_SLIPPAGE_BUFFER_BPS,
            min_net_bps: BASIS_MONITOR_DEFAULT_MIN_NET_BPS,
            once: false,
            output_dir: None,
        }
    }
}

/// 单个 symbol 的实时 basis 行情行。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceBasisMarketRow {
    pub symbol: String,
    pub spot_bid: Option<String>,
    pub spot_ask: Option<String>,
    pub spot_bid_qty: Option<String>,
    pub spot_ask_qty: Option<String>,
    pub perp_bid: Option<String>,
    pub perp_ask: Option<String>,
    pub perp_bid_qty: Option<String>,
    pub perp_ask_qty: Option<String>,
    pub mark_price: String,
    pub index_price: String,
    pub last_funding_rate: String,
    pub next_funding_time_ms: String,
    pub gross_basis_bps: Option<String>,
    pub total_cost_bps: Option<String>,
    pub net_basis_bps: Option<String>,
    pub quantity: Option<String>,
    pub expected_profit_usd: Option<String>,
    pub is_candidate: bool,
    pub reason: Option<String>,
    pub source_status: String,
}

/// Binance basis 监控快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceBasisMonitorSnapshot {
    pub status: String,
    pub updated_at: String,
    pub min_abs_funding_rate: String,
    pub min_net_bps: String,
    pub total_rows: usize,
    pub candidate_count: usize,
    pub filtered_funding_count: usize,
    pub missing_spot_count: usize,
    pub missing_perp_count: usize,
    pub last_error: Option<String>,
    pub rows: Vec<BinanceBasisMarketRow>,
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
            ArtifactFile::new("replay smoke", "replay_smoke.txt", &self.replay_smoke_txt),
            ArtifactFile::new(
                "stored events",
                "stored_events.jsonl",
                &self.stored_events_jsonl,
            ),
            ArtifactFile::new(
                "candidate transitions",
                "candidate_transitions.jsonl",
                &self.candidate_transitions_jsonl,
            ),
            ArtifactFile::new(
                "risk decisions",
                "risk_decisions.jsonl",
                &self.risk_decisions_jsonl,
            ),
            ArtifactFile::new(
                "execution plans",
                "execution_plans.jsonl",
                &self.execution_plans_jsonl,
            ),
            ArtifactFile::new(
                "execution reports",
                "execution_reports.jsonl",
                &self.execution_reports_jsonl,
            ),
            ArtifactFile::new(
                "ledger entries",
                "ledger_entries.jsonl",
                &self.ledger_entries_jsonl,
            ),
            ArtifactFile::new(
                "reconciliation reports",
                "reconciliation_reports.jsonl",
                &self.reconciliation_reports_jsonl,
            ),
            ArtifactFile::new("incidents", "incidents.jsonl", &self.incidents_jsonl),
            ArtifactFile::new(
                "operations daily report",
                "operations_daily_report.md",
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

/// 从配置字符串启动运行时装配服务。
pub fn start_runtime_from_config_yaml(input: &str) -> RuntimeResult<RuntimeService> {
    let config = arb_config::ArbConfig::from_yaml_str(input)?;
    start_runtime_with_config(&config)
}

/// 从配置文件启动运行时装配服务。
///
/// 中文说明：该入口只读取本地配置并运行启动检查，不访问网络、不读取凭证、
/// 不启动真实交易 API，也不提交任何真实账户动作。
pub fn start_runtime_from_config_path(path: impl AsRef<Path>) -> RuntimeResult<RuntimeService> {
    let config = arb_config::ArbConfig::from_path(path)?;
    start_runtime_with_config(&config)
}

/// 从 replay fixture 启动运行时装配服务。
///
/// 中文说明：该入口只读取 fixture 和已校验配置，用于暴露健康状态；不会启动
/// 真实交易 API、真实签名或可变账户动作。
pub fn start_runtime_for_fixture(fixture_root: impl AsRef<Path>) -> RuntimeResult<RuntimeService> {
    let fixture_root = resolve_fixture_root(fixture_root.as_ref());
    validate_full_pipeline_context(&fixture_root)?;
    let replay = arb_replay::load_fixture(&fixture_root)?;
    start_runtime_with_config(replay.config())
}

fn resolve_fixture_root(fixture_root: &Path) -> PathBuf {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_fixture_root_from(fixture_root, &current_dir, &workspace_root())
}

fn resolve_fixture_root_from(
    fixture_root: &Path,
    current_dir: &Path,
    workspace_root: &Path,
) -> PathBuf {
    if fixture_root.is_absolute() || current_dir.join(fixture_root).exists() {
        return fixture_root.to_path_buf();
    }

    let workspace_relative = workspace_root.join(fixture_root);
    if workspace_relative.exists() {
        return workspace_relative;
    }

    fixture_root.to_path_buf()
}

fn workspace_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

/// 根据已校验配置启动运行时装配服务。
pub fn start_runtime_with_config(config: &arb_config::ArbConfig) -> RuntimeResult<RuntimeService> {
    let checks = run_startup_checks(config);
    let failed_reasons = checks
        .iter()
        .filter(|check| check.status == RuntimeCheckStatus::Fail)
        .map(|check| format!("{}: {}", check.name, check.message))
        .collect::<Vec<_>>();
    if !failed_reasons.is_empty() {
        return Err(RuntimeError::StartupRejected {
            reasons: failed_reasons,
        });
    }

    let mut tasks = RuntimeTaskRegistry::default();
    tasks.push_exited(
        TASK_STARTUP_CHECKS,
        RuntimeTaskExitReason::Completed,
        "启动检查已完成；配置和熔断状态已进入健康快照",
    );
    tasks.push_running(
        TASK_READ_ONLY_INGEST,
        "只读 fixture 数据采集任务已装配，不连接真实交易 API",
    );
    tasks.push_running(
        TASK_EVENT_STORE,
        "追加式事件存储任务已装配，只写离线运行时事件",
    );
    tasks.push_running(TASK_HEALTH_REPORTER, "健康状态发布任务已装配");
    tasks.push_running(TASK_SHUTDOWN_LISTENER, "优雅退出监听任务已装配");
    append_mutable_execution_task(config, &mut tasks);

    Ok(RuntimeService {
        config_hash: config.hash().as_str().to_owned(),
        execution_mode: config.execution().mode(),
        kill_switch_triggered: config.kill_switch().is_triggered(),
        checks,
        tasks,
        shutdown_requested: false,
    })
}

fn run_startup_checks(config: &arb_config::ArbConfig) -> Vec<RuntimeCheck> {
    let mut checks = Vec::new();
    checks.push(RuntimeCheck::pass(
        "config-loaded",
        format!(
            "配置版本 {} 已加载，哈希 {}",
            config.version().as_str(),
            config.hash().as_str()
        ),
    ));

    checks.push(check_real_signing_disabled(config));
    checks.push(check_execution_permission(config));
    checks.push(check_circuit_breaker(config));
    checks
}

fn check_real_signing_disabled(config: &arb_config::ArbConfig) -> RuntimeCheck {
    if config.signing().real_signing_enabled() {
        RuntimeCheck::fail(
            "real-signing",
            "阶段 9 运行时禁止真实签名；real_signing_enabled 必须为 false",
        )
    } else {
        RuntimeCheck::pass("real-signing", "真实签名关闭，使用空签名策略引用")
    }
}

fn check_execution_permission(config: &arb_config::ArbConfig) -> RuntimeCheck {
    let mode = config.execution().mode();
    if !mode.requires_live_permission() {
        return RuntimeCheck::pass(
            "execution-permission",
            format!("执行模式 {mode} 不需要可变账户权限"),
        );
    }

    if config.kill_switch().blocks_execution_mode(mode) {
        RuntimeCheck::warning(
            "execution-permission",
            format!("执行模式 {mode} 已被熔断阻止；可变执行任务保持跳过"),
        )
    } else {
        RuntimeCheck::fail(
            "execution-permission",
            format!("执行模式 {mode} 会请求可变账户权限，但阶段 9 运行时不能启动可变执行"),
        )
    }
}

fn check_circuit_breaker(config: &arb_config::ArbConfig) -> RuntimeCheck {
    let kill_switch = config.kill_switch();
    if !kill_switch.is_triggered() {
        return RuntimeCheck::pass("circuit-breaker", "熔断未打开");
    }

    let mut scopes = Vec::new();
    if kill_switch.global() {
        scopes.push("global".to_owned());
    }
    if kill_switch.execution() {
        scopes.push("execution".to_owned());
    }
    scopes.extend(
        kill_switch
            .execution_modes()
            .iter()
            .map(|mode| format!("execution_mode:{mode}")),
    );
    scopes.extend(
        kill_switch
            .strategies()
            .iter()
            .map(|value| format!("strategy:{value}")),
    );
    scopes.extend(
        kill_switch
            .venues()
            .iter()
            .map(|value| format!("venue:{value}")),
    );
    scopes.extend(
        kill_switch
            .accounts()
            .iter()
            .map(|value| format!("account:{value}")),
    );
    scopes.extend(
        kill_switch
            .instruments()
            .iter()
            .map(|value| format!("instrument:{value}")),
    );
    scopes.extend(
        kill_switch
            .assets()
            .iter()
            .map(|value| format!("asset:{value}")),
    );
    scopes.extend(
        kill_switch
            .chains()
            .iter()
            .map(|value| format!("chain:{value}")),
    );

    RuntimeCheck::warning(
        "circuit-breaker",
        format!("熔断已打开，范围：{}", scopes.join(",")),
    )
}

fn append_mutable_execution_task(config: &arb_config::ArbConfig, tasks: &mut RuntimeTaskRegistry) {
    let mode = config.execution().mode();
    if config.kill_switch().blocks_execution_mode(mode) {
        tasks.push_skipped(
            TASK_MUTABLE_EXECUTION,
            format!("熔断阻止执行模式 {mode}；未启动可变执行适配器"),
        );
    } else if !config.allows_account_changes() {
        tasks.push_skipped(
            TASK_MUTABLE_EXECUTION,
            format!("执行模式 {mode} 不允许真实账户变化；未启动可变执行适配器"),
        );
    } else {
        tasks.push_failed(
            TASK_MUTABLE_EXECUTION,
            "启动检查应在可变执行被允许前拒绝阶段 9 运行时",
        );
    }
}

fn runtime_health_snapshot(
    config_hash: &str,
    execution_mode: ConfigExecutionMode,
    kill_switch_triggered: bool,
    shutdown_requested: bool,
    checks: &[RuntimeCheck],
    tasks: Vec<RuntimeTaskStatus>,
) -> RuntimeHealthSnapshot {
    let mutable_execution_started = tasks.iter().any(|task| {
        task.name == TASK_MUTABLE_EXECUTION
            && matches!(
                task.state,
                RuntimeTaskState::Running | RuntimeTaskState::Exited
            )
            && task.exit_reason != Some(RuntimeTaskExitReason::StartupSkipped)
    });
    let status = if shutdown_requested {
        RuntimeHealthStatus::Stopped
    } else if checks
        .iter()
        .any(|check| check.status == RuntimeCheckStatus::Fail)
        || tasks
            .iter()
            .any(|task| task.state == RuntimeTaskState::Failed)
    {
        RuntimeHealthStatus::Unhealthy
    } else if kill_switch_triggered
        || checks
            .iter()
            .any(|check| check.status == RuntimeCheckStatus::Warning)
    {
        RuntimeHealthStatus::Degraded
    } else {
        RuntimeHealthStatus::Healthy
    };

    RuntimeHealthSnapshot {
        status,
        config_hash: config_hash.to_owned(),
        execution_mode: execution_mode.as_str().to_owned(),
        kill_switch_triggered,
        mutable_execution_started,
        shutdown_requested,
        checks: checks.to_vec(),
        tasks,
    }
}

fn assemble_full_pipeline(fixture_root: &Path) -> RuntimeResult<EndToEndArtifacts> {
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

/// 拉取一次 Binance 公开 spot/perp basis 数据并运行只读策略扫描。
///
/// 中文说明：该路径只访问公开 REST 端点，不使用 API key，不访问账户，不下单、
/// 不撤单、不转账、不签名。输出是候选转换或明确拒绝原因。
pub fn run_binance_basis_scan(
    symbol: &str,
    output_dir: Option<PathBuf>,
) -> RuntimeResult<BinanceBasisScanReport> {
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

    let raw_inputs = BinanceBasisRawInputs {
        symbol: &symbol,
        raw_spot_book: &raw_spot_book,
        spot_book_ref: &spot_book_ticker_url,
        raw_perp_book: &raw_perp_book,
        perp_book_ref: &perp_book_ticker_url,
        raw_premium_index: &raw_premium_index,
        premium_index_ref: &premium_index_url,
    };
    let events = ingest_binance_basis_public_json(raw_inputs, ingested_at)?;

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
    let portfolio_state = build_binance_basis_portfolio_state(&source_event_refs, ingested_at)?;
    ensure_portfolio_state_source_refs_exist(&portfolio_state, &stored_events)?;
    let venue_capabilities = load_binance_basis_capabilities()?;
    let evaluation = run_spot_perp_basis_strategy(
        replay.config(),
        &stored_events,
        &portfolio_state,
        &venue_capabilities,
        &ingested_at.to_string(),
    )?;

    let candidate_transitions_jsonl = evaluation
        .candidate()
        .map(|candidate| canonical_jsonl(std::slice::from_ref(candidate)))
        .unwrap_or_default();
    let (rejection_reason, rejection_detail) = evaluation
        .rejection()
        .map(|rejection| {
            (
                Some(rejection.reason().as_str().to_owned()),
                rejection.detail().map(str::to_owned),
            )
        })
        .unwrap_or((None, None));
    let diagnostics = evaluation
        .diagnostics()
        .iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.code(), diagnostic.detail()))
        .collect::<Vec<_>>();
    let stored_events_jsonl = stored_events_jsonl(&stored_events);

    let report = BinanceBasisScanReport {
        symbol,
        spot_book_ticker_url,
        perp_book_ticker_url,
        premium_index_url,
        ingested_at: ingested_at.to_string(),
        stored_events_jsonl,
        candidate_transitions_jsonl,
        rejection_reason,
        rejection_detail,
        diagnostics,
        output_dir,
    };
    if let Some(dir) = &report.output_dir {
        write_binance_basis_scan_artifacts(dir, &report)?;
    }
    Ok(report)
}

/// 运行 Binance basis 7*24 只读监控。
///
/// 中文说明：默认会启动本地 HTTP API，并按固定间隔刷新全量公开行情；`once`
/// 仅用于手动验收或测试，不代表 7*24 模式。
pub fn run_binance_basis_monitor(options: BinanceBasisMonitorOptions) -> RuntimeResult<()> {
    validate_monitor_options(&options)?;
    let state = Arc::new(RwLock::new(BinanceBasisMonitorSnapshot::empty(&options)));
    if !options.once {
        start_binance_basis_http_api(&options.bind_addr, state.clone())?;
        println!(
            "binance-basis-monitor: api=http://{} poll_interval_secs={} min_abs_funding_rate={} mutable_execution_started=false",
            options.bind_addr, options.poll_interval_secs, options.min_abs_funding_rate
        );
    }

    loop {
        match fetch_binance_basis_monitor_snapshot(&options) {
            Ok(snapshot) => {
                if let Some(dir) = &options.output_dir {
                    write_binance_basis_monitor_snapshot(dir, &snapshot)?;
                }
                *state.write().expect("monitor state lock poisoned") = snapshot;
            }
            Err(error) => {
                if options.once {
                    return Err(error);
                }
                let mut snapshot = state.write().expect("monitor state lock poisoned");
                snapshot.status = "degraded".to_owned();
                snapshot.last_error = Some(error.to_string());
                snapshot.updated_at = current_utc_timestamp()
                    .map(|timestamp| timestamp.to_string())
                    .unwrap_or_else(|_| "unknown".to_owned());
            }
        }

        if options.once {
            break;
        }
        thread::sleep(Duration::from_secs(options.poll_interval_secs));
    }
    Ok(())
}

fn validate_monitor_options(options: &BinanceBasisMonitorOptions) -> RuntimeResult<()> {
    if options.poll_interval_secs == 0 {
        return Err(cli_arg_error("poll interval must be greater than zero"));
    }
    MonitorDecimal::parse("min_abs_funding_rate", &options.min_abs_funding_rate)?;
    MonitorDecimal::parse("notional_usd", &options.notional_usd)?;
    Ok(())
}

fn fetch_binance_basis_monitor_snapshot(
    options: &BinanceBasisMonitorOptions,
) -> RuntimeResult<BinanceBasisMonitorSnapshot> {
    let spot_json = fetch_public_json_with_curl(&binance_spot_book_ticker_all_url())?;
    let perp_json = fetch_public_json_with_curl(&binance_usdm_book_ticker_all_url())?;
    let premium_json = fetch_public_json_with_curl(&binance_usdm_premium_index_all_url())?;
    build_binance_basis_monitor_snapshot_from_json(&spot_json, &perp_json, &premium_json, options)
}

fn build_binance_basis_monitor_snapshot_from_json(
    spot_json: &str,
    perp_json: &str,
    premium_json: &str,
    options: &BinanceBasisMonitorOptions,
) -> RuntimeResult<BinanceBasisMonitorSnapshot> {
    let updated_at = current_utc_timestamp()?.to_string();
    let min_abs_funding_rate =
        MonitorDecimal::parse("min_abs_funding_rate", &options.min_abs_funding_rate)?;
    let spot_books = parse_book_ticker_rows(spot_json, "spot")?
        .into_iter()
        .map(|row| (row.symbol.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let perp_books = parse_book_ticker_rows(perp_json, "usdm-perp")?
        .into_iter()
        .map(|row| (row.symbol.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let premiums = parse_premium_index_rows(premium_json)?;

    let mut rows = Vec::new();
    let mut filtered_funding_count = 0_usize;
    let mut missing_spot_count = 0_usize;
    let mut missing_perp_count = 0_usize;

    for premium in premiums {
        if !premium.symbol.ends_with("USDT") {
            continue;
        }
        let funding_rate = MonitorDecimal::parse("lastFundingRate", &premium.last_funding_rate)?;
        if funding_rate.abs_less_than(min_abs_funding_rate) {
            filtered_funding_count += 1;
            continue;
        }

        let spot = spot_books.get(&premium.symbol);
        let perp = perp_books.get(&premium.symbol);
        let (mut source_status, reason) = match (spot, perp) {
            (Some(_), Some(_)) => ("complete".to_owned(), None),
            (None, Some(_)) => {
                missing_spot_count += 1;
                (
                    "missing_spot".to_owned(),
                    Some("MISSING_SPOT_BOOK_TICKER".to_owned()),
                )
            }
            (Some(_), None) => {
                missing_perp_count += 1;
                (
                    "missing_perp".to_owned(),
                    Some("MISSING_PERP_BOOK_TICKER".to_owned()),
                )
            }
            (None, None) => {
                missing_spot_count += 1;
                missing_perp_count += 1;
                (
                    "missing_spot_and_perp".to_owned(),
                    Some("MISSING_SPOT_AND_PERP_BOOK_TICKER".to_owned()),
                )
            }
        };

        let mut signal_error = None;
        let signal = match (spot, perp) {
            (Some(spot), Some(perp)) => {
                match evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
                    symbol: premium.symbol.clone(),
                    spot_best_bid: spot.bid_price.clone(),
                    spot_best_ask: spot.ask_price.clone(),
                    perp_best_bid: perp.bid_price.clone(),
                    perp_best_ask: perp.ask_price.clone(),
                    notional_usd: options.notional_usd.clone(),
                    spot_taker_fee_bps: options.spot_taker_fee_bps,
                    perp_taker_fee_bps: options.perp_taker_fee_bps,
                    slippage_buffer_bps: options.slippage_buffer_bps,
                    min_net_bps: options.min_net_bps,
                }) {
                    Ok(signal) => Some(signal),
                    Err(message) => {
                        source_status = "invalid_quote".to_owned();
                        signal_error = Some(message);
                        None
                    }
                }
            }
            _ => None,
        };
        let reason = signal
            .as_ref()
            .and_then(|signal| signal.reason.clone())
            .or(signal_error)
            .or(reason);

        rows.push(BinanceBasisMarketRow {
            symbol: premium.symbol,
            spot_bid: spot.map(|row| row.bid_price.clone()),
            spot_ask: spot.map(|row| row.ask_price.clone()),
            spot_bid_qty: spot.map(|row| row.bid_qty.clone()),
            spot_ask_qty: spot.map(|row| row.ask_qty.clone()),
            perp_bid: perp.map(|row| row.bid_price.clone()),
            perp_ask: perp.map(|row| row.ask_price.clone()),
            perp_bid_qty: perp.map(|row| row.bid_qty.clone()),
            perp_ask_qty: perp.map(|row| row.ask_qty.clone()),
            mark_price: premium.mark_price,
            index_price: premium.index_price,
            last_funding_rate: premium.last_funding_rate,
            next_funding_time_ms: premium.next_funding_time_ms,
            gross_basis_bps: signal.as_ref().map(|signal| signal.gross_bps.to_string()),
            total_cost_bps: signal
                .as_ref()
                .map(|signal| signal.total_cost_bps.to_string()),
            net_basis_bps: signal.as_ref().map(|signal| signal.net_bps.to_string()),
            quantity: signal.as_ref().map(|signal| signal.quantity.clone()),
            expected_profit_usd: signal
                .as_ref()
                .map(|signal| signal.expected_profit_usd.clone()),
            is_candidate: signal.as_ref().is_some_and(|signal| signal.is_candidate),
            reason,
            source_status,
        });
    }

    rows.sort_by(|left, right| {
        monitor_optional_i128(&right.net_basis_bps)
            .cmp(&monitor_optional_i128(&left.net_basis_bps))
            .then_with(|| left.symbol.cmp(&right.symbol))
    });
    let candidate_count = rows.iter().filter(|row| row.is_candidate).count();

    Ok(BinanceBasisMonitorSnapshot {
        status: "healthy".to_owned(),
        updated_at,
        min_abs_funding_rate: options.min_abs_funding_rate.clone(),
        min_net_bps: options.min_net_bps.to_string(),
        total_rows: rows.len(),
        candidate_count,
        filtered_funding_count,
        missing_spot_count,
        missing_perp_count,
        last_error: None,
        rows,
    })
}

fn validate_live_market_symbol(symbol: &str) -> RuntimeResult<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol == SIM_SYMBOL {
        Ok(symbol)
    } else {
        Err(RuntimeError::LiveMarketData {
            message: format!(
                "only {SIM_SYMBOL} is currently wired to the full-pipeline fixture; got `{symbol}`"
            ),
        })
    }
}

fn validate_binance_basis_symbol(symbol: &str) -> RuntimeResult<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol == BASIS_SYMBOL {
        Ok(symbol)
    } else {
        Err(RuntimeError::LiveMarketData {
            message: format!(
                "only {BASIS_SYMBOL} is currently wired to the Binance spot-perp basis strategy; got `{symbol}`"
            ),
        })
    }
}

fn binance_ticker_24h_url(symbol: &str) -> String {
    format!("{BINANCE_MARKET_DATA_BASE_URL}/api/v3/ticker/24hr?symbol={symbol}")
}

fn binance_spot_book_ticker_url(symbol: &str) -> String {
    format!("{BINANCE_SPOT_REST_BASE_URL}/api/v3/ticker/bookTicker?symbol={symbol}")
}

fn binance_spot_book_ticker_all_url() -> String {
    format!("{BINANCE_SPOT_REST_BASE_URL}/api/v3/ticker/bookTicker")
}

fn binance_usdm_book_ticker_url(symbol: &str) -> String {
    format!("{BINANCE_USDM_FUTURES_BASE_URL}/fapi/v1/ticker/bookTicker?symbol={symbol}")
}

fn binance_usdm_book_ticker_all_url() -> String {
    format!("{BINANCE_USDM_FUTURES_BASE_URL}/fapi/v1/ticker/bookTicker")
}

fn binance_usdm_premium_index_url(symbol: &str) -> String {
    format!("{BINANCE_USDM_FUTURES_BASE_URL}/fapi/v1/premiumIndex?symbol={symbol}")
}

fn binance_usdm_premium_index_all_url() -> String {
    format!("{BINANCE_USDM_FUTURES_BASE_URL}/fapi/v1/premiumIndex")
}

fn fetch_public_json_with_curl(url: &str) -> RuntimeResult<String> {
    let output = Command::new("curl")
        .args(["-fsS", "--max-time", "10", url])
        .output()
        .map_err(|error| RuntimeError::LiveMarketData {
            message: format!("cannot run curl for public market data: {error}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RuntimeError::LiveMarketData {
            message: format!("curl returned {}; stderr={}", output.status, stderr.trim()),
        });
    }

    let body = String::from_utf8(output.stdout).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("public market data response was not UTF-8: {error}"),
    })?;
    if body.trim().is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: "public market data response was empty".to_owned(),
        });
    }
    Ok(body)
}

fn current_utc_timestamp() -> RuntimeResult<UtcTimestamp> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RuntimeError::LiveMarketData {
            message: format!("system time is before Unix epoch: {error}"),
        })?;
    let seconds = i64::try_from(now.as_secs()).map_err(|_| RuntimeError::LiveMarketData {
        message: "current Unix timestamp does not fit i64".to_owned(),
    })?;
    Ok(UtcTimestamp::from_unix_parts(seconds, now.subsec_nanos())?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MonitorDecimal {
    raw: i128,
}

impl MonitorDecimal {
    const SCALE_DIGITS: usize = 8;

    fn parse(field: &'static str, value: &str) -> RuntimeResult<Self> {
        let value = value.trim();
        if value.is_empty() {
            return Err(RuntimeError::LiveMarketData {
                message: format!("decimal field `{field}` cannot be empty"),
            });
        }
        let negative = value.starts_with('-');
        let unsigned = value.strip_prefix('-').unwrap_or(value);
        if unsigned.is_empty() || unsigned.starts_with('.') || unsigned.ends_with('.') {
            return Err(RuntimeError::LiveMarketData {
                message: format!("decimal field `{field}` is malformed: `{value}`"),
            });
        }
        let mut raw = 0_i128;
        let mut dot_seen = false;
        let mut digits_seen = false;
        let mut fraction_digits = 0_usize;
        for byte in unsigned.bytes() {
            match byte {
                b'0'..=b'9' => {
                    digits_seen = true;
                    if dot_seen {
                        if fraction_digits == Self::SCALE_DIGITS {
                            return Err(RuntimeError::LiveMarketData {
                                message: format!(
                                    "decimal field `{field}` exceeds {} fractional digits",
                                    Self::SCALE_DIGITS
                                ),
                            });
                        }
                        fraction_digits += 1;
                    }
                    raw = raw
                        .checked_mul(10)
                        .and_then(|scaled| scaled.checked_add(i128::from(byte - b'0')))
                        .ok_or_else(|| RuntimeError::LiveMarketData {
                            message: format!("decimal field `{field}` overflowed"),
                        })?;
                }
                b'.' if !dot_seen => dot_seen = true,
                _ => {
                    return Err(RuntimeError::LiveMarketData {
                        message: format!("decimal field `{field}` is malformed: `{value}`"),
                    });
                }
            }
        }
        if !digits_seen {
            return Err(RuntimeError::LiveMarketData {
                message: format!("decimal field `{field}` has no digits"),
            });
        }
        for _ in fraction_digits..Self::SCALE_DIGITS {
            raw = raw
                .checked_mul(10)
                .ok_or_else(|| RuntimeError::LiveMarketData {
                    message: format!("decimal field `{field}` overflowed"),
                })?;
        }
        if negative {
            raw = raw
                .checked_neg()
                .ok_or_else(|| RuntimeError::LiveMarketData {
                    message: format!("decimal field `{field}` overflowed"),
                })?;
        }
        Ok(Self { raw })
    }

    fn abs_less_than(self, other: Self) -> bool {
        self.raw.abs() < other.raw.abs()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MonitorBookTickerRow {
    symbol: String,
    bid_price: String,
    bid_qty: String,
    ask_price: String,
    ask_qty: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MonitorPremiumIndexRow {
    symbol: String,
    mark_price: String,
    index_price: String,
    last_funding_rate: String,
    next_funding_time_ms: String,
}

fn parse_book_ticker_rows(
    input: &str,
    source_name: &'static str,
) -> RuntimeResult<Vec<MonitorBookTickerRow>> {
    json_object_slices(input)?
        .into_iter()
        .map(|object| {
            let fields = parse_flat_json_object(object)?;
            Ok(MonitorBookTickerRow {
                symbol: required_json_string(&fields, "symbol", source_name)?,
                bid_price: required_json_string(&fields, "bidPrice", source_name)?,
                bid_qty: required_json_string(&fields, "bidQty", source_name)?,
                ask_price: required_json_string(&fields, "askPrice", source_name)?,
                ask_qty: required_json_string(&fields, "askQty", source_name)?,
            })
        })
        .collect()
}

fn parse_premium_index_rows(input: &str) -> RuntimeResult<Vec<MonitorPremiumIndexRow>> {
    json_object_slices(input)?
        .into_iter()
        .map(|object| {
            let fields = parse_flat_json_object(object)?;
            Ok(MonitorPremiumIndexRow {
                symbol: required_json_string(&fields, "symbol", "premiumIndex")?,
                mark_price: required_json_string(&fields, "markPrice", "premiumIndex")?,
                index_price: required_json_string(&fields, "indexPrice", "premiumIndex")?,
                last_funding_rate: required_json_string(
                    &fields,
                    "lastFundingRate",
                    "premiumIndex",
                )?,
                next_funding_time_ms: required_json_scalar(
                    &fields,
                    "nextFundingTime",
                    "premiumIndex",
                )?,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum MonitorJsonScalar {
    String(String),
    Number(String),
    Bool(String),
    Null,
}

fn json_object_slices(input: &str) -> RuntimeResult<Vec<&str>> {
    let trimmed = input.trim();
    if trimmed.starts_with('{') {
        return Ok(vec![trimmed]);
    }
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return Err(RuntimeError::LiveMarketData {
            message: "expected JSON object or array of objects".to_owned(),
        });
    }

    let mut objects = Vec::new();
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut object_start = None;
    for (index, ch) in trimmed.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    object_start = Some(index);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return Err(RuntimeError::LiveMarketData {
                        message: "malformed JSON array: unmatched object close".to_owned(),
                    });
                }
                if depth == 0 {
                    let start = object_start.ok_or_else(|| RuntimeError::LiveMarketData {
                        message: "malformed JSON array: missing object start".to_owned(),
                    })?;
                    objects.push(&trimmed[start..index + ch.len_utf8()]);
                    object_start = None;
                }
            }
            _ => {}
        }
    }
    if depth != 0 || in_string {
        return Err(RuntimeError::LiveMarketData {
            message: "malformed JSON array".to_owned(),
        });
    }
    Ok(objects)
}

fn parse_flat_json_object(input: &str) -> RuntimeResult<BTreeMap<String, MonitorJsonScalar>> {
    let trimmed = input.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err(RuntimeError::LiveMarketData {
            message: "expected JSON object".to_owned(),
        });
    }
    let mut fields = BTreeMap::new();
    let bytes = trimmed.as_bytes();
    let mut index = 1_usize;
    while index + 1 < bytes.len() {
        index = skip_json_ws(bytes, index);
        if bytes.get(index) == Some(&b'}') {
            break;
        }
        let (key, after_key) = parse_json_string(trimmed, index)?;
        index = skip_json_ws(bytes, after_key);
        if bytes.get(index) != Some(&b':') {
            return Err(RuntimeError::LiveMarketData {
                message: "expected ':' after JSON object key".to_owned(),
            });
        }
        index = skip_json_ws(bytes, index + 1);
        let (value, after_value) = parse_json_scalar(trimmed, index)?;
        fields.insert(key, value);
        index = skip_json_ws(bytes, after_value);
        match bytes.get(index) {
            Some(b',') => index += 1,
            Some(b'}') => break,
            _ => {
                return Err(RuntimeError::LiveMarketData {
                    message: "expected ',' or '}' after JSON object value".to_owned(),
                });
            }
        }
    }
    Ok(fields)
}

fn parse_json_scalar(input: &str, index: usize) -> RuntimeResult<(MonitorJsonScalar, usize)> {
    let bytes = input.as_bytes();
    match bytes.get(index) {
        Some(b'"') => {
            let (value, after) = parse_json_string(input, index)?;
            Ok((MonitorJsonScalar::String(value), after))
        }
        Some(b'n') if input[index..].starts_with("null") => {
            Ok((MonitorJsonScalar::Null, index + 4))
        }
        Some(b't') if input[index..].starts_with("true") => {
            Ok((MonitorJsonScalar::Bool("true".to_owned()), index + 4))
        }
        Some(b'f') if input[index..].starts_with("false") => {
            Ok((MonitorJsonScalar::Bool("false".to_owned()), index + 5))
        }
        Some(_) => {
            let mut end = index;
            while let Some(byte) = bytes.get(end) {
                if byte.is_ascii_whitespace() || matches!(byte, b',' | b'}') {
                    break;
                }
                end += 1;
            }
            if end == index {
                return Err(RuntimeError::LiveMarketData {
                    message: "empty JSON scalar".to_owned(),
                });
            }
            Ok((MonitorJsonScalar::Number(input[index..end].to_owned()), end))
        }
        None => Err(RuntimeError::LiveMarketData {
            message: "unexpected end of JSON while parsing scalar".to_owned(),
        }),
    }
}

fn parse_json_string(input: &str, quote_start: usize) -> RuntimeResult<(String, usize)> {
    let bytes = input.as_bytes();
    if bytes.get(quote_start) != Some(&b'"') {
        return Err(RuntimeError::LiveMarketData {
            message: "expected JSON string opening quote".to_owned(),
        });
    }
    let mut out = String::new();
    let mut index = quote_start + 1;
    while index < input.len() {
        let ch = input[index..]
            .chars()
            .next()
            .ok_or_else(|| RuntimeError::LiveMarketData {
                message: "unexpected end of JSON string".to_owned(),
            })?;
        if ch == '"' {
            return Ok((out, index + ch.len_utf8()));
        }
        if ch == '\\' {
            let escape_index = index + ch.len_utf8();
            let escaped = input[escape_index..].chars().next().ok_or_else(|| {
                RuntimeError::LiveMarketData {
                    message: "unterminated JSON escape".to_owned(),
                }
            })?;
            match escaped {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'b' => out.push('\u{0008}'),
                'f' => out.push('\u{000c}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    return Err(RuntimeError::LiveMarketData {
                        message: "unicode JSON escapes are not supported in monitor parser"
                            .to_owned(),
                    });
                }
                _ => {
                    return Err(RuntimeError::LiveMarketData {
                        message: "unsupported JSON escape".to_owned(),
                    });
                }
            }
            index = escape_index + escaped.len_utf8();
        } else {
            out.push(ch);
            index += ch.len_utf8();
        }
    }
    Err(RuntimeError::LiveMarketData {
        message: "unterminated JSON string".to_owned(),
    })
}

fn skip_json_ws(bytes: &[u8], mut index: usize) -> usize {
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        index += 1;
    }
    index
}

fn required_json_string(
    fields: &BTreeMap<String, MonitorJsonScalar>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<String> {
    match fields.get(field) {
        Some(MonitorJsonScalar::String(value)) => Ok(value.clone()),
        Some(MonitorJsonScalar::Number(value)) => Ok(value.clone()),
        _ => Err(RuntimeError::LiveMarketData {
            message: format!("{source} object is missing string field `{field}`"),
        }),
    }
}

fn required_json_scalar(
    fields: &BTreeMap<String, MonitorJsonScalar>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<String> {
    match fields.get(field) {
        Some(MonitorJsonScalar::String(value))
        | Some(MonitorJsonScalar::Number(value))
        | Some(MonitorJsonScalar::Bool(value)) => Ok(value.clone()),
        Some(MonitorJsonScalar::Null) | None => Err(RuntimeError::LiveMarketData {
            message: format!("{source} object is missing scalar field `{field}`"),
        }),
    }
}

fn monitor_optional_i128(value: &Option<String>) -> i128 {
    value
        .as_deref()
        .and_then(|value| value.parse::<i128>().ok())
        .unwrap_or(i128::MIN)
}

fn validate_full_pipeline_context(fixture_root: &Path) -> RuntimeResult<()> {
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

fn ensure_simulated_offline_config(config: &arb_config::ArbConfig) -> RuntimeResult<()> {
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

struct BinanceBasisRawInputs<'a> {
    symbol: &'a str,
    raw_spot_book: &'a str,
    spot_book_ref: &'a str,
    raw_perp_book: &'a str,
    perp_book_ref: &'a str,
    raw_premium_index: &'a str,
    premium_index_ref: &'a str,
}

fn ingest_binance_basis_public_json(
    inputs: BinanceBasisRawInputs<'_>,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<Vec<NormalizedEvent>> {
    let mut spot_adapter = BinancePublicBookTickerAdapter::new(
        VenueId::new(BASIS_SPOT_VENUE_ID)?,
        binance_basis_instrument(inputs.symbol, BASIS_SPOT_INSTRUMENT_ID)?,
        BinancePublicMarket::Spot,
        ingested_at,
        MARKET_DATA_MAX_AGE_MS,
    )?;
    let mut perp_adapter = BinancePublicBookTickerAdapter::new(
        VenueId::new(BASIS_PERP_VENUE_ID)?,
        binance_basis_instrument(inputs.symbol, BASIS_PERP_INSTRUMENT_ID)?,
        BinancePublicMarket::UsdmPerpetual,
        ingested_at,
        MARKET_DATA_MAX_AGE_MS,
    )?;
    let premium_adapter = BinanceUsdmPremiumIndexAdapter::new(
        VenueId::new(BASIS_PERP_VENUE_ID)?,
        binance_basis_instrument(inputs.symbol, BASIS_PERP_INSTRUMENT_ID)?,
        MARKET_DATA_MAX_AGE_MS,
    )?;

    let spot_batch = spot_adapter.ingest_book_ticker_json(
        inputs.raw_spot_book,
        inputs.spot_book_ref,
        ingested_at,
    )?;
    let perp_batch = perp_adapter.ingest_book_ticker_json(
        inputs.raw_perp_book,
        inputs.perp_book_ref,
        ingested_at,
    )?;
    let premium_batch = premium_adapter.ingest_premium_index_json(
        inputs.raw_premium_index,
        inputs.premium_index_ref,
        ingested_at,
    )?;

    Ok(vec![
        spot_batch.raw_event,
        spot_batch.normalized_event,
        perp_batch.raw_event,
        perp_batch.normalized_event,
        premium_batch.raw_event,
        premium_batch.normalized_event,
    ])
}

fn binance_basis_instrument(
    symbol: &str,
    instrument_id: &str,
) -> RuntimeResult<BinancePublicInstrument> {
    Ok(BinancePublicInstrument::new(
        symbol,
        InstrumentId::new(instrument_id)?,
        AssetId::new(BASIS_BASE_ASSET_ID)?,
        AssetId::new(BASIS_QUOTE_ASSET_ID)?,
        AssetId::new(BASIS_SETTLEMENT_ASSET_ID)?,
    )?
    .with_tick_size(Price::from_str("0.01")?)
    .with_lot_size(Quantity::from_str("0.000001")?))
}

fn build_binance_basis_portfolio_state(
    source_event_refs: &[String],
    as_of: UtcTimestamp,
) -> RuntimeResult<PortfolioState> {
    let portfolio_json = format!(
        r#"{{
  "schema_version": "1.0.0",
  "portfolio_state_id": "state:binance-basis-public-readonly-01",
  "as_of": {},
  "source_event_refs": {},
  "balances": [],
  "positions": [],
  "reservations": [],
  "open_orders": [],
  "pending_transfers": [],
  "confidence": 0.5,
  "missing_data_flags": [
    "PRIVATE_BALANCE_UNAVAILABLE"
  ],
  "state_hash": "hash:binance-basis-public-readonly-01"
}}"#,
        json_string(&as_of.to_string()),
        json_string_array(source_event_refs),
    );
    Ok(from_json_strict::<PortfolioState>(&portfolio_json)?)
}

fn build_portfolio_state_from_fixture(
    fixture_root: &Path,
    stored_events: &[arb_eventstore::StoredEvent],
    source_event_ref_override: Option<&str>,
    as_of_override: Option<String>,
) -> RuntimeResult<PortfolioState> {
    let path = fixture_root.join("portfolio_state.json");
    let mut input = read_utf8(&path)?;
    if let Some(as_of) = as_of_override {
        input = replace_json_string_field(&input, "as_of", &as_of)?;
    }
    if let Some(event_ref) = source_event_ref_override {
        input = replace_json_string_array_field(&input, "source_event_refs", &[event_ref])?;
    }
    let state = from_json_strict::<PortfolioState>(&input)?;
    ensure_portfolio_state_source_refs_exist(&state, stored_events)?;
    Ok(state)
}

fn ensure_portfolio_state_source_refs_exist(
    state: &PortfolioState,
    stored_events: &[arb_eventstore::StoredEvent],
) -> RuntimeResult<()> {
    let event_ids = stored_events
        .iter()
        .map(|record| record.event.event_id.as_str().to_owned())
        .collect::<BTreeSet<_>>();

    for event_ref in &state.source_event_refs {
        if !event_ids.contains(event_ref.as_str()) {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: format!(
                    "portfolio state `{}` references missing event `{}`",
                    state.portfolio_state_id.as_str(),
                    event_ref.as_str()
                ),
            });
        }
    }
    Ok(())
}

fn replace_json_string_field(input: &str, field: &str, value: &str) -> RuntimeResult<String> {
    let key = format!("\"{field}\"");
    let key_start = input.find(&key).ok_or_else(|| RuntimeError::Module {
        module: "arb-runtime",
        message: format!("portfolio fixture is missing string field `{field}`"),
    })?;
    let after_key = key_start + key.len();
    let colon = after_key
        + input[after_key..]
            .find(':')
            .ok_or_else(|| RuntimeError::Module {
                module: "arb-runtime",
                message: format!("portfolio fixture field `{field}` is missing ':'"),
            })?;
    let quote_start = colon
        + 1
        + input[colon + 1..]
            .find('"')
            .ok_or_else(|| RuntimeError::Module {
                module: "arb-runtime",
                message: format!("portfolio fixture field `{field}` is not a JSON string"),
            })?;
    let quote_end = json_string_end(input, quote_start)?;

    let mut output = String::with_capacity(input.len() + value.len());
    output.push_str(&input[..quote_start]);
    output.push_str(&json_string(value));
    output.push_str(&input[quote_end..]);
    Ok(output)
}

fn replace_json_string_array_field(
    input: &str,
    field: &str,
    values: &[&str],
) -> RuntimeResult<String> {
    let key = format!("\"{field}\"");
    let key_start = input.find(&key).ok_or_else(|| RuntimeError::Module {
        module: "arb-runtime",
        message: format!("portfolio fixture is missing array field `{field}`"),
    })?;
    let after_key = key_start + key.len();
    let colon = after_key
        + input[after_key..]
            .find(':')
            .ok_or_else(|| RuntimeError::Module {
                module: "arb-runtime",
                message: format!("portfolio fixture field `{field}` is missing ':'"),
            })?;
    let bracket_start = colon
        + 1
        + input[colon + 1..]
            .find('[')
            .ok_or_else(|| RuntimeError::Module {
                module: "arb-runtime",
                message: format!("portfolio fixture field `{field}` is not a JSON array"),
            })?;
    let bracket_end = json_array_end(input, bracket_start)?;
    let replacement = format!(
        "[{}]",
        values
            .iter()
            .map(|value| json_string(value))
            .collect::<Vec<_>>()
            .join(",")
    );

    let mut output = String::with_capacity(input.len() + replacement.len());
    output.push_str(&input[..bracket_start]);
    output.push_str(&replacement);
    output.push_str(&input[bracket_end..]);
    Ok(output)
}

fn json_string_end(input: &str, quote_start: usize) -> RuntimeResult<usize> {
    let bytes = input.as_bytes();
    if bytes.get(quote_start) != Some(&b'"') {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: "internal JSON replacement expected a string opening quote".to_owned(),
        });
    }

    let mut escaped = false;
    for (offset, ch) in input[quote_start + 1..].char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Ok(quote_start + 1 + offset + ch.len_utf8());
        }
    }
    Err(RuntimeError::Module {
        module: "arb-runtime",
        message: "portfolio fixture contains an unterminated JSON string".to_owned(),
    })
}

fn json_array_end(input: &str, bracket_start: usize) -> RuntimeResult<usize> {
    let bytes = input.as_bytes();
    if bytes.get(bracket_start) != Some(&b'[') {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: "internal JSON replacement expected an array opening bracket".to_owned(),
        });
    }

    let mut depth = 0_u32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in input[bracket_start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth = depth.checked_sub(1).ok_or_else(|| RuntimeError::Module {
                    module: "arb-runtime",
                    message: "portfolio fixture has an unmatched JSON array bracket".to_owned(),
                })?;
                if depth == 0 {
                    return Ok(bracket_start + offset + ch.len_utf8());
                }
            }
            _ => {}
        }
    }

    Err(RuntimeError::Module {
        module: "arb-runtime",
        message: "portfolio fixture contains an unterminated JSON array".to_owned(),
    })
}

fn load_venue_capabilities(fixture_root: &Path) -> RuntimeResult<Vec<VenueCapabilityDescriptor>> {
    let path = fixture_root.join("venue_capabilities.jsonl");
    let input = read_utf8(&path)?;
    let mut capabilities = Vec::new();
    for (index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: format!(
                    "{} line {}: blank venue capability line is not allowed",
                    path.display(),
                    index + 1
                ),
            });
        }
        capabilities.push(from_json_strict::<VenueCapabilityDescriptor>(line)?);
    }
    if capabilities.is_empty() {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!("{}: venue capabilities fixture is empty", path.display()),
        });
    }
    Ok(capabilities)
}

fn load_binance_basis_capabilities() -> RuntimeResult<Vec<VenueCapabilityDescriptor>> {
    let spot = r#"{"auth_modes":["PublicOnly"],"capability_version":"1.0.0","data_surfaces":["RESTPolling","RateLimitHeaders"],"execution_capabilities":["SupportsManualApprovalOnly"],"health_model":{"disconnect_threshold":3,"freshness_threshold_ms":5000,"unknown_state_is_critical":true},"market_capabilities":["ProvidesSpotMarkets","ProvidesOrderBookMarkets"],"permission_model":{"can_read_private_data":false,"can_read_public_data":true,"can_trade":false,"can_withdraw":false},"rate_limit_model":{"limit":1200,"source":"binance-public-rest","unit":"Request","window_ms":60000},"schema_version":"1.0.0","settlement_modes":["OffChainCustody"],"venue_id":"venue:BINANCE-SPOT","venue_name":"Binance Spot Public REST"}"#;
    let perp = r#"{"auth_modes":["PublicOnly"],"capability_version":"1.0.0","data_surfaces":["RESTPolling","RateLimitHeaders","FundingHistory"],"execution_capabilities":["SupportsManualApprovalOnly"],"health_model":{"disconnect_threshold":3,"freshness_threshold_ms":5000,"unknown_state_is_critical":true},"market_capabilities":["ProvidesPerpetuals","ProvidesOrderBookMarkets","ProvidesFundingRates"],"permission_model":{"can_read_private_data":false,"can_read_public_data":true,"can_trade":false,"can_withdraw":false},"rate_limit_model":{"limit":2400,"source":"binance-public-futures-rest","unit":"Request","window_ms":60000},"schema_version":"1.0.0","settlement_modes":["OffChainCustody"],"venue_id":"venue:BINANCE-USDM","venue_name":"Binance USD-M Public REST"}"#;
    Ok(vec![
        from_json_strict::<VenueCapabilityDescriptor>(spot)?,
        from_json_strict::<VenueCapabilityDescriptor>(perp)?,
    ])
}

fn run_strategy(
    replay: &ReplayInput,
    stored_events: &[arb_eventstore::StoredEvent],
    portfolio_state: &PortfolioState,
    venue_capabilities: &[VenueCapabilityDescriptor],
    fixed_time: &str,
) -> RuntimeResult<CandidatePortfolioTransition> {
    let market_events = stored_events
        .iter()
        .map(|record| record.event.clone())
        .collect::<Vec<_>>();
    let snapshot = ReadOnlySnapshot::new(portfolio_state.clone(), market_events);
    let capabilities = VenueCapabilitySnapshot::new(venue_capabilities.to_vec())?;
    let config = StrategyConfigSnapshot::from_config(replay.config())?;
    let time = StrategyFixedTimeSource::from_rfc3339_z(fixed_time)?;
    let input = StrategyInput::new(snapshot, capabilities, config, time);
    let strategy = sample_spot_strategy()?;
    let evaluation = strategy.evaluate(&input)?;
    if let Some(candidate) = evaluation.candidate() {
        return Ok(candidate.clone());
    }
    let rejection = evaluation.rejection().ok_or_else(|| RuntimeError::Module {
        module: "arb-strategy-api",
        message: "strategy returned neither candidate nor rejection".to_owned(),
    })?;
    Err(RuntimeError::StrategyRejected {
        reason: rejection.reason().as_str().to_owned(),
        detail: rejection.detail().map(str::to_owned),
    })
}

fn run_spot_perp_basis_strategy(
    config: &arb_config::ArbConfig,
    stored_events: &[arb_eventstore::StoredEvent],
    portfolio_state: &PortfolioState,
    venue_capabilities: &[VenueCapabilityDescriptor],
    fixed_time: &str,
) -> RuntimeResult<StrategyEvaluation> {
    let market_events = stored_events
        .iter()
        .map(|record| record.event.clone())
        .collect::<Vec<_>>();
    let snapshot = ReadOnlySnapshot::new(portfolio_state.clone(), market_events);
    let capabilities = VenueCapabilitySnapshot::new(venue_capabilities.to_vec())?;
    let config = StrategyConfigSnapshot::from_config(config)?;
    let time = StrategyFixedTimeSource::from_rfc3339_z(fixed_time)?;
    let input = StrategyInput::new(snapshot, capabilities, config, time);
    let strategy = spot_perp_basis_strategy()?;
    Ok(strategy.evaluate(&input)?)
}

fn run_risk(
    candidate: &CandidatePortfolioTransition,
    portfolio_state: &PortfolioState,
    config: &arb_config::ArbConfig,
    venue_capabilities: &[VenueCapabilityDescriptor],
    evaluated_at: UtcTimestamp,
) -> RuntimeResult<RiskDecision> {
    let evaluator = StaticRiskEvaluator::default();
    Ok(evaluator.evaluate(RiskEvaluationInput::new(
        candidate,
        portfolio_state,
        config,
        venue_capabilities,
        evaluated_at,
    ))?)
}

fn risk_decision_allows_execution(decision: &RiskDecision) -> bool {
    matches!(
        decision.decision.as_str(),
        "Approved" | "ApprovedWithConstraints"
    )
}

fn incidents_from_risk_rejection(
    candidate: &CandidatePortfolioTransition,
    decision: &RiskDecision,
    opened_at: &str,
) -> RuntimeResult<Vec<Incident>> {
    let reason_code = decision
        .reason_codes
        .first()
        .map(|reason| reason.as_str())
        .unwrap_or("UNKNOWN_STATE");
    let source_event_refs = if candidate.input_event_refs.is_empty() {
        vec![json_string(decision.transition_id.as_str())]
    } else {
        candidate
            .input_event_refs
            .iter()
            .map(|event_ref| json_string(event_ref.as_str()))
            .collect::<Vec<_>>()
    };
    let venue_ids = candidate
        .legs
        .iter()
        .filter_map(|leg| leg.venue_id.as_ref())
        .map(|venue_id| json_string(venue_id.as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let incident_suffix = incident_suffix(reason_code);
    let incident_json = format!(
        "{{\"automatic_actions\":[{{\"action_id\":{},\"action_type\":\"TradingPaused\",\"detail\":{},\"timestamp\":{}}}],\"impacted\":{{\"capital_at_risk_usd\":\"0\",\"strategy_ids\":[{}],\"venue_ids\":[{}]}},\"incident_id\":{},\"manual_actions\":[{{\"action_id\":{},\"action_type\":\"ManualReview\",\"detail\":{},\"timestamp\":{}}}],\"opened_at\":{},\"schema_version\":\"1.0.0\",\"severity\":{},\"source_event_refs\":[{}],\"status\":\"Open\",\"trigger\":{}}}",
        json_string(&format!("iact:{incident_suffix}:auto")),
        json_string("Runtime risk rejection stopped the simulated pipeline before any execution plan."),
        json_string(opened_at),
        json_string(candidate.strategy_id.as_str()),
        venue_ids.join(","),
        json_string(&format!("incident:{incident_suffix}")),
        json_string(&format!("iact:{incident_suffix}:manual")),
        json_string("Operator review required before this fixture path can continue."),
        json_string(opened_at),
        json_string(opened_at),
        json_string(incident_severity_for_reason(reason_code)),
        source_event_refs.join(","),
        json_string(reason_code),
    );
    Ok(vec![from_json_strict::<Incident>(&incident_json)?])
}

fn incident_suffix(reason_code: &str) -> String {
    let normalized = reason_code.to_ascii_lowercase().replace('_', "-");
    format!("full-pipeline-{normalized}")
}

fn incident_severity_for_reason(reason_code: &str) -> &'static str {
    match reason_code {
        "UNKNOWN_STATE" => "SEV1",
        "DATA_STALE" => "SEV2",
        _ => "SEV3",
    }
}

fn run_execution_plan(
    candidate: &CandidatePortfolioTransition,
    risk_decision: &RiskDecision,
    config: &arb_config::ArbConfig,
    created_at: &str,
) -> RuntimeResult<ExecutionPlan> {
    let execution_mode = contract_execution_mode(config.execution().mode());
    Ok(build_execution_plan(ExecutionPlanBuildInput::new(
        risk_decision,
        candidate,
        execution_mode,
        created_at,
    ))?)
}

fn append_to_simulated_ledger(
    entries: &[ContractLedgerEntry],
) -> RuntimeResult<Vec<DomainLedgerEntry>> {
    let mut book = LedgerBook::default();
    for entry in entries {
        let domain_entry = contract_ledger_entry_to_domain(entry)?;
        let _outcome = book.append(domain_entry)?;
    }
    Ok(book.entries().to_vec())
}

fn contract_ledger_entry_to_domain(
    entry: &ContractLedgerEntry,
) -> RuntimeResult<DomainLedgerEntry> {
    let header = LedgerEntryHeader::new(
        LedgerEntryId::new(entry.ledger_entry_id.as_str())?,
        JournalEntryId::new(entry.journal_entry_id.as_str())?,
        UtcTimestamp::parse_rfc3339_z(entry.timestamp.as_str())?,
        ledger_namespace_from_contract(&entry.namespace)?,
        ledger_entry_type_from_contract(&entry.entry_type)?,
        EventId::new(entry.source_event_id.as_str())?,
        IdempotencyKey::new(entry.idempotency_key.as_str())?,
    );
    let legs = entry
        .legs
        .iter()
        .map(contract_ledger_leg_to_domain)
        .collect::<RuntimeResult<Vec<_>>>()?;
    let mut draft = LedgerEntryDraft::new(header, legs);

    if let Some(strategy_id) = &entry.strategy_id {
        draft = draft.with_strategy_id(StrategyId::new(strategy_id.as_str())?);
    }
    if let Some(opportunity_id) = &entry.opportunity_id {
        draft = draft.with_opportunity_id(CandidateTransitionId::new(opportunity_id.as_str())?);
    }
    if let Some(plan_id) = &entry.execution_plan_id {
        draft = draft.with_execution_plan_id(ExecutionPlanId::new(plan_id.as_str())?);
    }
    if let Some(reversal_of) = &entry.reversal_of {
        draft = draft.with_reversal_of(LedgerEntryId::new(reversal_of.as_str())?);
    }
    if let Some(adjustment_of) = &entry.adjustment_of {
        draft = draft.with_adjustment_of(LedgerEntryId::new(adjustment_of.as_str())?);
    }
    if let Some(reason_code) = &entry.adjustment_reason_code {
        draft = draft.with_adjustment_reason_code(AdjustmentReasonCode::new(reason_code.as_str())?);
    }

    Ok(DomainLedgerEntry::from_draft(draft)?)
}

fn contract_ledger_leg_to_domain(leg: &arb_contracts::LedgerLeg) -> RuntimeResult<LedgerLeg> {
    let mut domain_leg = LedgerLeg::new(
        LedgerLegId::new(leg.leg_id.as_str())?,
        AccountId::new(leg.account_id.as_str())?,
        AssetId::new(leg.asset_id.as_str())?,
        ledger_direction_from_contract(&leg.direction)?,
        Amount::from_str(leg.amount.as_str())?,
    );
    if let Some(custody_location_id) = &leg.custody_location_id {
        domain_leg =
            domain_leg.with_custody_location(AccountId::new(custody_location_id.as_str())?);
    }
    if let Some(valuation_usd) = &leg.valuation_usd {
        domain_leg = domain_leg.with_valuation_usd(Decimal::from_str(valuation_usd.as_str())?);
    }
    if let Some(memo) = &leg.memo {
        domain_leg = domain_leg.with_memo(memo.clone());
    }
    Ok(domain_leg)
}

fn fill_snapshots_from_report(
    report: &ExecutionReport,
    ledger_entries: &[ContractLedgerEntry],
) -> RuntimeResult<Vec<FillSnapshot>> {
    let ledger_by_source_event = ledger_entries
        .iter()
        .map(|entry| {
            (
                entry.source_event_id.as_str().to_owned(),
                entry.ledger_entry_id.as_str().to_owned(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    report
        .fills
        .iter()
        .map(|fill| {
            let fee = FeeAmount::new(
                AssetId::new(fill.fee.asset_id.as_str())?,
                Amount::from_str(fill.fee.amount.as_str())?,
            );
            let mut snapshot = FillSnapshot::new(
                FillId::new(fill.fill_id.as_str())?,
                ExecutionPlanId::new(fill.plan_id.as_str())?,
                VenueId::new(fill.venue_id.as_str())?,
                InstrumentId::new(fill.instrument_id.as_str())?,
                Price::from_str(fill.price.as_str())?,
                Quantity::from_str(fill.quantity.as_str())?,
                fee,
            )
            .with_source_event_id(EventId::new(fill.source_event_id.as_str())?);

            if let Some(ledger_entry_id) = ledger_by_source_event.get(fill.source_event_id.as_str())
            {
                snapshot = snapshot.with_ledger_entry_id(LedgerEntryId::new(ledger_entry_id)?);
            }
            Ok(snapshot)
        })
        .collect()
}

fn run_reconciliation(
    as_of: UtcTimestamp,
    ledger_entries: &[DomainLedgerEntry],
    fill_snapshots: &[FillSnapshot],
) -> RuntimeResult<ReconciliationReport> {
    let mut request = ReconciliationRequest::new(
        ReconciliationRunId::new(RECONCILIATION_RUN_ID)?,
        as_of,
        ledger_entries,
    );
    request.expected_fills = fill_snapshots;
    request.observed_fills = fill_snapshots;
    Ok(CoreReconciliationRunner::default().run(request)?)
}

fn run_operations_report(
    event_store: &JsonlEventStore,
    risk_decisions: &[RiskDecision],
    execution_reports: &[ExecutionReport],
    ledger_entries: &[ContractLedgerEntry],
    reconciliation_reports: &[ReconciliationReport],
    incidents: &[Incident],
    fixed_time: &str,
) -> RuntimeResult<String> {
    let report_date = fixed_time.get(0..10).unwrap_or("unknown").to_owned();
    let facts = OperationsFacts::from_event_reader(event_store)?
        .with_risk_decisions(risk_decisions.to_vec())
        .with_execution_reports(execution_reports.to_vec())
        .with_ledger_entries(ledger_entries.to_vec())
        .with_reconciliation_reports(reconciliation_reports.to_vec())
        .with_incidents(incidents.to_vec());
    let reader = InMemoryOpsFactReader::new(facts);
    match ReadOnlyOpsEngine.run(
        &reader,
        OpsReadOnlyCommand::DailyReport {
            report_date,
            generated_at: fixed_time.to_owned(),
        },
    )? {
        OpsCommandOutput::DailyReport(report) => Ok(report.render_markdown()?),
        _ => Err(RuntimeError::Module {
            module: "arb-ops",
            message: "daily report command returned a non-daily output".to_owned(),
        }),
    }
}

fn contract_execution_mode(mode: ConfigExecutionMode) -> ContractExecutionMode {
    match mode {
        ConfigExecutionMode::ReadOnly => ContractExecutionMode::ReadOnly,
        ConfigExecutionMode::Simulated => ContractExecutionMode::Simulated,
        ConfigExecutionMode::ManualApproval => ContractExecutionMode::ManualApproval,
        ConfigExecutionMode::GuardedLive => ContractExecutionMode::GuardedLive,
        ConfigExecutionMode::AutonomousLive => ContractExecutionMode::AutonomousLive,
    }
}

fn ledger_namespace_from_contract(
    value: &ContractLedgerNamespace,
) -> RuntimeResult<DomainLedgerNamespace> {
    match value.as_str() {
        "Live" => Ok(DomainLedgerNamespace::Live),
        "Simulation" => Ok(DomainLedgerNamespace::Simulation),
        "Backtest" => Ok(DomainLedgerNamespace::Backtest),
        "Adjustment" => Ok(DomainLedgerNamespace::Adjustment),
        other => Err(unknown_contract_enum("ledger namespace", other)),
    }
}

fn ledger_entry_type_from_contract(
    value: &ContractLedgerEntryType,
) -> RuntimeResult<DomainLedgerEntryType> {
    match value.as_str() {
        "TradeFill" => Ok(DomainLedgerEntryType::TradeFill),
        "Fee" => Ok(DomainLedgerEntryType::Fee),
        "Funding" => Ok(DomainLedgerEntryType::Funding),
        "Transfer" => Ok(DomainLedgerEntryType::Transfer),
        "CapitalReservation" => Ok(DomainLedgerEntryType::CapitalReservation),
        "CapitalRelease" => Ok(DomainLedgerEntryType::CapitalRelease),
        "Borrow" => Ok(DomainLedgerEntryType::Borrow),
        "Lend" => Ok(DomainLedgerEntryType::Lend),
        "Repay" => Ok(DomainLedgerEntryType::Repay),
        "RealizedPnl" => Ok(DomainLedgerEntryType::RealizedPnl),
        "UnrealizedPnlSnapshot" => Ok(DomainLedgerEntryType::UnrealizedPnlSnapshot),
        "ReconciliationAdjustment" => Ok(DomainLedgerEntryType::ReconciliationAdjustment),
        "ManualAdjustment" => Ok(DomainLedgerEntryType::ManualAdjustment),
        other => Err(unknown_contract_enum("ledger entry type", other)),
    }
}

fn ledger_direction_from_contract(
    value: &ContractLedgerDirection,
) -> RuntimeResult<DomainLedgerDirection> {
    match value.as_str() {
        "Debit" => Ok(DomainLedgerDirection::Debit),
        "Credit" => Ok(DomainLedgerDirection::Credit),
        other => Err(unknown_contract_enum("ledger direction", other)),
    }
}

fn unknown_contract_enum(kind: &'static str, value: &str) -> RuntimeError {
    RuntimeError::Module {
        module: "arb-runtime",
        message: format!("unknown {kind} `{value}` while adapting contract output"),
    }
}

fn canonical_jsonl<T: CanonicalJson>(values: &[T]) -> String {
    jsonl_from_lines(values.iter().map(to_canonical_json).collect())
}

fn stored_events_jsonl(records: &[arb_eventstore::StoredEvent]) -> String {
    jsonl_from_lines(
        records
            .iter()
            .map(|record| record.canonical_json.clone())
            .collect(),
    )
}

fn jsonl_from_lines(lines: Vec<String>) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

fn stable_reconciliation_report_json(report: &ReconciliationReport) -> String {
    let highest_severity = report
        .summary
        .highest_severity
        .map(|severity| json_string(severity.as_str()))
        .unwrap_or_else(|| "null".to_owned());
    format!(
        "{{\"as_of\":{},\"checked_balances\":{},\"checked_capital_reservations\":{},\"checked_fills\":{},\"checked_ledger_entries\":{},\"checked_positions\":{},\"difference_count\":{},\"highest_severity\":{},\"incident_suggestion_count\":{},\"run_id\":{},\"status\":{}}}",
        json_string(&report.as_of.to_string()),
        report.summary.checked_balances,
        report.summary.checked_capital_reservations,
        report.summary.checked_fills,
        report.summary.checked_ledger_entries,
        report.summary.checked_positions,
        report.summary.difference_count,
        highest_severity,
        report.summary.incident_suggestion_count,
        json_string(report.run_id.as_str()),
        json_string(report.summary.status.as_str()),
    )
}

fn write_expected_artifacts(
    fixture_root: &Path,
    artifacts: &EndToEndArtifacts,
) -> RuntimeResult<Vec<GoldenComparison>> {
    let expected_dir = fixture_root.join("expected");
    fs::create_dir_all(&expected_dir).map_err(|error| RuntimeError::Io {
        path: expected_dir.clone(),
        message: error.to_string(),
    })?;
    let mut comparisons = Vec::new();
    for artifact in artifacts.files() {
        let path = expected_dir.join(artifact.file_name);
        fs::write(&path, artifact.contents).map_err(|error| RuntimeError::Io {
            path: path.clone(),
            message: error.to_string(),
        })?;
        comparisons.push(GoldenComparison {
            artifact: artifact.artifact,
            path,
            bytes: artifact.contents.len(),
        });
    }
    Ok(comparisons)
}

fn write_artifacts_to_dir(
    output_dir: &Path,
    artifacts: &EndToEndArtifacts,
) -> RuntimeResult<Vec<GoldenComparison>> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut written = Vec::new();
    for artifact in artifacts.files() {
        let path = output_dir.join(artifact.file_name);
        fs::write(&path, artifact.contents).map_err(|error| RuntimeError::Io {
            path: path.clone(),
            message: error.to_string(),
        })?;
        written.push(GoldenComparison {
            artifact: artifact.artifact,
            path,
            bytes: artifact.contents.len(),
        });
    }
    Ok(written)
}

fn write_binance_basis_scan_artifacts(
    output_dir: &Path,
    report: &BinanceBasisScanReport,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("binance_basis_events.jsonl"),
        &report.stored_events_jsonl,
    )?;
    write_utf8(
        output_dir.join("binance_basis_candidate_transitions.jsonl"),
        &report.candidate_transitions_jsonl,
    )?;
    let rejection = report
        .rejection_reason
        .as_ref()
        .map(|reason| {
            format!(
                "reason={reason}\ndetail={}\n",
                report.rejection_detail.as_deref().unwrap_or("")
            )
        })
        .unwrap_or_default();
    write_utf8(output_dir.join("binance_basis_rejection.txt"), &rejection)?;
    write_utf8(
        output_dir.join("binance_basis_diagnostics.txt"),
        &report.diagnostics.join("\n"),
    )?;
    Ok(())
}

fn write_binance_basis_monitor_snapshot(
    output_dir: &Path,
    snapshot: &BinanceBasisMonitorSnapshot,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("binance_basis_monitor_snapshot.json"),
        &snapshot.to_json(),
    )
}

fn write_utf8(path: PathBuf, contents: &str) -> RuntimeResult<()> {
    fs::write(&path, contents).map_err(|error| RuntimeError::Io {
        path,
        message: error.to_string(),
    })
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

fn read_utf8(path: &Path) -> RuntimeResult<String> {
    fs::read_to_string(path).map_err(|error| RuntimeError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
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

impl BinanceBasisMonitorSnapshot {
    fn empty(options: &BinanceBasisMonitorOptions) -> Self {
        Self {
            status: "starting".to_owned(),
            updated_at: "not-yet-updated".to_owned(),
            min_abs_funding_rate: options.min_abs_funding_rate.clone(),
            min_net_bps: options.min_net_bps.to_string(),
            total_rows: 0,
            candidate_count: 0,
            filtered_funding_count: 0,
            missing_spot_count: 0,
            missing_perp_count: 0,
            last_error: None,
            rows: Vec::new(),
        }
    }

    fn to_json(&self) -> String {
        format!(
            "{{\"candidate_count\":{},\"filtered_funding_count\":{},\"last_error\":{},\"min_abs_funding_rate\":{},\"min_net_bps\":{},\"missing_perp_count\":{},\"missing_spot_count\":{},\"rows\":[{}],\"status\":{},\"total_rows\":{},\"updated_at\":{}}}",
            self.candidate_count,
            self.filtered_funding_count,
            json_option_string(&self.last_error),
            json_string(&self.min_abs_funding_rate),
            json_string(&self.min_net_bps),
            self.missing_perp_count,
            self.missing_spot_count,
            self.rows
                .iter()
                .map(BinanceBasisMarketRow::to_json)
                .collect::<Vec<_>>()
                .join(","),
            json_string(&self.status),
            self.total_rows,
            json_string(&self.updated_at),
        )
    }

    fn opportunities_json(&self) -> String {
        format!(
            "{{\"candidate_count\":{},\"rows\":[{}],\"status\":{},\"updated_at\":{}}}",
            self.candidate_count,
            self.rows
                .iter()
                .filter(|row| row.is_candidate)
                .map(BinanceBasisMarketRow::to_json)
                .collect::<Vec<_>>()
                .join(","),
            json_string(&self.status),
            json_string(&self.updated_at),
        )
    }

    fn symbol_json(&self, symbol: &str) -> Option<String> {
        let symbol = symbol.to_ascii_uppercase();
        self.rows
            .iter()
            .find(|row| row.symbol == symbol)
            .map(BinanceBasisMarketRow::to_json)
    }
}

impl BinanceBasisMarketRow {
    fn to_json(&self) -> String {
        format!(
            "{{\"expected_profit_usd\":{},\"gross_basis_bps\":{},\"index_price\":{},\"is_candidate\":{},\"last_funding_rate\":{},\"mark_price\":{},\"net_basis_bps\":{},\"next_funding_time_ms\":{},\"perp_ask\":{},\"perp_ask_qty\":{},\"perp_bid\":{},\"perp_bid_qty\":{},\"quantity\":{},\"reason\":{},\"source_status\":{},\"spot_ask\":{},\"spot_ask_qty\":{},\"spot_bid\":{},\"spot_bid_qty\":{},\"symbol\":{},\"total_cost_bps\":{}}}",
            json_option_string(&self.expected_profit_usd),
            json_option_string(&self.gross_basis_bps),
            json_string(&self.index_price),
            self.is_candidate,
            json_string(&self.last_funding_rate),
            json_string(&self.mark_price),
            json_option_string(&self.net_basis_bps),
            json_string(&self.next_funding_time_ms),
            json_option_string(&self.perp_ask),
            json_option_string(&self.perp_ask_qty),
            json_option_string(&self.perp_bid),
            json_option_string(&self.perp_bid_qty),
            json_option_string(&self.quantity),
            json_option_string(&self.reason),
            json_string(&self.source_status),
            json_option_string(&self.spot_ask),
            json_option_string(&self.spot_ask_qty),
            json_option_string(&self.spot_bid),
            json_option_string(&self.spot_bid_qty),
            json_string(&self.symbol),
            json_option_string(&self.total_cost_bps),
        )
    }
}

fn json_option_string(value: &Option<String>) -> String {
    value
        .as_deref()
        .map(json_string)
        .unwrap_or_else(|| "null".to_owned())
}

fn start_binance_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<BinanceBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_basis_http_connection(stream, &state),
                Err(error) => eprintln!("binance-basis-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

fn handle_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<BinanceBasisMonitorSnapshot>>,
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
    if route == "/" || route == "/dashboard" {
        let _ = write_http_html(&mut stream, 200, basis_dashboard_html());
        return;
    }

    let snapshot = state.read().expect("monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (
            200,
            format!(
                "{{\"status\":{},\"updated_at\":{}}}",
                json_string(&snapshot.status),
                json_string(&snapshot.updated_at)
            ),
        )
    } else if route == "/api/basis/status" {
        (200, snapshot.to_json())
    } else if route == "/api/basis/opportunities" {
        (200, snapshot.opportunities_json())
    } else if let Some(symbol) = route.strip_prefix("/api/basis/status/") {
        match snapshot.symbol_json(symbol.trim_matches('/')) {
            Some(row) => (200, row),
            None => (
                404,
                format!(
                    "{{\"error\":\"symbol_not_found\",\"symbol\":{}}}",
                    json_string(symbol.trim_matches('/'))
                ),
            ),
        }
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/\",\"/dashboard\",\"/health\",\"/api/basis/status\",\"/api/basis/opportunities\",\"/api/basis/status/<SYMBOL>\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn write_http_json(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    write_http_response(stream, status, "application/json; charset=utf-8", body)
}

fn write_http_html(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    write_http_response(stream, status, "text/html; charset=utf-8", body)
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn basis_dashboard_html() -> &'static str {
    r##"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>实时套利监控</title>
  <style>
    :root {
      color-scheme: dark;
      --bg: #101112;
      --band: #151719;
      --panel: #1b1d20;
      --panel-strong: #23262a;
      --line: #353941;
      --text: #f0eadc;
      --muted: #a5a8aa;
      --amber: #e2b650;
      --green: #58c383;
      --red: #e06a6a;
      --blue: #77a7d9;
    }

    * {
      box-sizing: border-box;
    }

    body {
      margin: 0;
      min-width: 320px;
      background: var(--bg);
      color: var(--text);
      font-family: "IBM Plex Mono", "JetBrains Mono", "SFMono-Regular", Consolas, monospace;
      font-size: 14px;
      letter-spacing: 0;
    }

    button,
    input,
    select {
      font: inherit;
    }

    .topbar {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 24px;
      padding: 22px 28px;
      background: #191b1e;
      border-bottom: 1px solid var(--line);
    }

    h1 {
      margin: 0;
      font-size: 24px;
      font-weight: 700;
      line-height: 1.2;
    }

    .kicker {
      margin: 0 0 6px;
      color: var(--amber);
      font-size: 12px;
      text-transform: uppercase;
    }

    .status-strip {
      display: flex;
      flex-wrap: wrap;
      align-items: center;
      justify-content: flex-end;
      gap: 10px;
      color: var(--muted);
      text-align: right;
    }

    .pill {
      display: inline-flex;
      align-items: center;
      min-height: 28px;
      padding: 4px 10px;
      border: 1px solid var(--line);
      border-radius: 6px;
      background: var(--panel-strong);
      color: var(--muted);
      white-space: nowrap;
    }

    .pill.healthy,
    .candidate {
      color: var(--green);
      border-color: rgba(88, 195, 131, 0.45);
    }

    .pill.error,
    .negative {
      color: var(--red);
      border-color: rgba(224, 106, 106, 0.45);
    }

    .positive {
      color: var(--green);
    }

    main {
      padding: 18px 28px 32px;
    }

    .summary-grid {
      display: grid;
      grid-template-columns: repeat(6, minmax(130px, 1fr));
      gap: 10px;
      margin-bottom: 14px;
    }

    .metric {
      min-height: 72px;
      padding: 12px;
      border: 1px solid var(--line);
      border-radius: 6px;
      background: var(--panel);
    }

    .metric span {
      display: block;
      color: var(--muted);
      font-size: 12px;
      white-space: nowrap;
    }

    .metric strong {
      display: block;
      margin-top: 8px;
      font-size: 22px;
      line-height: 1.1;
      overflow-wrap: anywhere;
    }

    .control-bar {
      display: grid;
      grid-template-columns: minmax(150px, 220px) repeat(2, minmax(116px, max-content)) minmax(170px, 220px) minmax(100px, max-content) minmax(130px, max-content);
      gap: 10px;
      align-items: end;
      padding: 12px;
      margin-bottom: 14px;
      border: 1px solid var(--line);
      background: var(--band);
    }

    label span,
    .field-label {
      display: block;
      margin-bottom: 6px;
      color: var(--muted);
      font-size: 12px;
    }

    input[type="search"],
    select {
      width: 100%;
      min-height: 36px;
      border: 1px solid var(--line);
      border-radius: 6px;
      background: #101214;
      color: var(--text);
      padding: 7px 9px;
    }

    .toggle {
      display: inline-flex;
      align-items: center;
      gap: 8px;
      min-height: 36px;
      padding: 8px 10px;
      border: 1px solid var(--line);
      border-radius: 6px;
      color: var(--text);
      background: var(--panel);
      white-space: nowrap;
    }

    .toggle input {
      width: 16px;
      height: 16px;
      accent-color: var(--amber);
    }

    button {
      min-height: 36px;
      border: 1px solid var(--line);
      border-radius: 6px;
      background: var(--panel-strong);
      color: var(--text);
      padding: 8px 12px;
      cursor: pointer;
    }

    button:hover {
      border-color: var(--amber);
    }

    .table-wrap,
    .api-wrap {
      border: 1px solid var(--line);
      background: var(--band);
    }

    .table-head,
    .api-head {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      min-height: 44px;
      padding: 10px 12px;
      border-bottom: 1px solid var(--line);
      color: var(--muted);
    }

    .table-scroll {
      overflow-x: auto;
      max-height: 58vh;
    }

    table {
      width: 100%;
      min-width: 1180px;
      border-collapse: collapse;
    }

    th,
    td {
      height: 38px;
      padding: 8px 10px;
      border-bottom: 1px solid rgba(53, 57, 65, 0.7);
      text-align: right;
      white-space: nowrap;
    }

    th {
      position: sticky;
      top: 0;
      z-index: 1;
      background: #202327;
      color: var(--muted);
      font-size: 12px;
      font-weight: 600;
    }

    th:first-child,
    td:first-child,
    th:last-child,
    td:last-child {
      text-align: left;
    }

    tbody tr:hover {
      background: rgba(226, 182, 80, 0.08);
    }

    .api-wrap {
      margin-top: 14px;
    }

    .endpoint-grid {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
    }

    pre {
      margin: 0;
      max-height: 260px;
      overflow: auto;
      padding: 12px;
      background: #0c0d0e;
      color: #d7d1c2;
      border-top: 1px solid var(--line);
      font-size: 12px;
      line-height: 1.5;
    }

    .empty-row {
      height: 88px;
      color: var(--muted);
      text-align: center;
    }

    @media (max-width: 980px) {
      .topbar {
        align-items: flex-start;
        flex-direction: column;
      }

      .status-strip {
        justify-content: flex-start;
        text-align: left;
      }

      .summary-grid {
        grid-template-columns: repeat(2, minmax(0, 1fr));
      }

      .control-bar {
        grid-template-columns: 1fr;
      }
    }

    @media (max-width: 560px) {
      main,
      .topbar {
        padding-left: 14px;
        padding-right: 14px;
      }

      .summary-grid {
        grid-template-columns: 1fr;
      }
    }
  </style>
</head>
<body>
  <header class="topbar">
    <div>
      <p class="kicker">Binance public basis</p>
      <h1>实时套利监控</h1>
    </div>
    <div class="status-strip">
      <span id="runtime-status" class="pill">starting</span>
      <span id="updated-at">not-yet-updated</span>
    </div>
  </header>
  <main>
    <section class="summary-grid" aria-label="summary">
      <article class="metric"><span>机会</span><strong id="metric-candidates">0</strong></article>
      <article class="metric"><span>市场</span><strong id="metric-total">0</strong></article>
      <article class="metric"><span>Funding 过滤</span><strong id="metric-filtered">0</strong></article>
      <article class="metric"><span>缺现货</span><strong id="metric-missing-spot">0</strong></article>
      <article class="metric"><span>缺永续</span><strong id="metric-missing-perp">0</strong></article>
      <article class="metric"><span>最小净 Basis</span><strong id="metric-min-net">0</strong></article>
    </section>

    <section class="control-bar" aria-label="controls">
      <label>
        <span>Symbol</span>
        <input id="symbol-filter" type="search" autocomplete="off" placeholder="BTCUSDT">
      </label>
      <label class="toggle"><input id="only-candidates" type="checkbox">只看机会</label>
      <label class="toggle"><input id="only-complete" type="checkbox" checked>只看完整报价</label>
      <label>
        <span>排序</span>
        <select id="sort-mode">
          <option value="net-desc">净 Basis 降序</option>
          <option value="funding-abs-desc">Funding 绝对值</option>
          <option value="profit-desc">预期收益</option>
          <option value="symbol-asc">Symbol</option>
        </select>
      </label>
      <button id="refresh-button" type="button">刷新</button>
      <label class="toggle"><input id="auto-refresh" type="checkbox" checked>自动刷新</label>
    </section>

    <section class="table-wrap" aria-label="basis table">
      <div class="table-head">
        <strong>行情与机会</strong>
        <span id="api-state">waiting</span>
      </div>
      <div class="table-scroll">
        <table>
          <thead>
            <tr>
              <th>Symbol</th>
              <th>Spot Bid</th>
              <th>Spot Ask</th>
              <th>Perp Bid</th>
              <th>Perp Ask</th>
              <th>Mark</th>
              <th>Index</th>
              <th>Funding</th>
              <th>Gross bps</th>
              <th>Cost bps</th>
              <th>Net bps</th>
              <th>Qty</th>
              <th>Profit USD</th>
              <th>状态</th>
              <th>原因</th>
            </tr>
          </thead>
          <tbody id="basis-rows">
            <tr><td class="empty-row" colspan="15">loading</td></tr>
          </tbody>
        </table>
      </div>
    </section>

    <section class="api-wrap" aria-label="realtime api">
      <div class="api-head">
        <strong>实时 API</strong>
        <div class="endpoint-grid">
          <button type="button" data-endpoint="/api/basis/status">status</button>
          <button type="button" data-endpoint="/api/basis/opportunities">opportunities</button>
        </div>
      </div>
      <pre id="api-preview">{}</pre>
    </section>
  </main>

  <script>
    const statusUrl = "/api/basis/status";
    const opportunitiesUrl = "/api/basis/opportunities";
    const state = { snapshot: null, timer: null };
    const $ = (id) => document.getElementById(id);

    function escapeHtml(value) {
      return String(value ?? "-").replace(/[&<>"']/g, (ch) => ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        "\"": "&quot;",
        "'": "&#39;"
      }[ch]));
    }

    function numeric(value) {
      const number = Number(value);
      return Number.isFinite(number) ? number : null;
    }

    function signedClass(value) {
      const number = numeric(value);
      if (number === null || number === 0) return "";
      return number > 0 ? "positive" : "negative";
    }

    async function requestJson(url) {
      const response = await fetch(url, { cache: "no-store" });
      const text = await response.text();
      let data;
      try {
        data = JSON.parse(text);
      } catch (error) {
        throw new Error(`invalid json from ${url}: ${error.message}`);
      }
      if (!response.ok) {
        throw new Error(data.error || `http ${response.status}`);
      }
      return data;
    }

    function filteredRows(rows) {
      const symbolFilter = $("symbol-filter").value.trim().toUpperCase();
      const onlyCandidates = $("only-candidates").checked;
      const onlyComplete = $("only-complete").checked;
      const sortMode = $("sort-mode").value;
      const result = rows.filter((row) => {
        if (symbolFilter && !row.symbol.includes(symbolFilter)) return false;
        if (onlyCandidates && !row.is_candidate) return false;
        if (onlyComplete && row.source_status !== "complete") return false;
        return true;
      });
      result.sort((left, right) => {
        if (sortMode === "symbol-asc") return left.symbol.localeCompare(right.symbol);
        if (sortMode === "funding-abs-desc") {
          return Math.abs(numeric(right.last_funding_rate) ?? 0) - Math.abs(numeric(left.last_funding_rate) ?? 0);
        }
        if (sortMode === "profit-desc") {
          return (numeric(right.expected_profit_usd) ?? -Infinity) - (numeric(left.expected_profit_usd) ?? -Infinity);
        }
        return (numeric(right.net_basis_bps) ?? -Infinity) - (numeric(left.net_basis_bps) ?? -Infinity);
      });
      return result;
    }

    function renderRows(rows) {
      const body = $("basis-rows");
      const view = filteredRows(rows);
      if (!view.length) {
        body.innerHTML = `<tr><td class="empty-row" colspan="15">no rows</td></tr>`;
        return;
      }
      body.innerHTML = view.map((row) => {
        const candidateClass = row.is_candidate ? "candidate" : "";
        return `<tr>
          <td class="${candidateClass}">${escapeHtml(row.symbol)}</td>
          <td>${escapeHtml(row.spot_bid)}</td>
          <td>${escapeHtml(row.spot_ask)}</td>
          <td>${escapeHtml(row.perp_bid)}</td>
          <td>${escapeHtml(row.perp_ask)}</td>
          <td>${escapeHtml(row.mark_price)}</td>
          <td>${escapeHtml(row.index_price)}</td>
          <td class="${signedClass(row.last_funding_rate)}">${escapeHtml(row.last_funding_rate)}</td>
          <td class="${signedClass(row.gross_basis_bps)}">${escapeHtml(row.gross_basis_bps)}</td>
          <td>${escapeHtml(row.total_cost_bps)}</td>
          <td class="${signedClass(row.net_basis_bps)}">${escapeHtml(row.net_basis_bps)}</td>
          <td>${escapeHtml(row.quantity)}</td>
          <td class="${signedClass(row.expected_profit_usd)}">${escapeHtml(row.expected_profit_usd)}</td>
          <td>${escapeHtml(row.source_status)}</td>
          <td>${escapeHtml(row.reason)}</td>
        </tr>`;
      }).join("");
    }

    function render(snapshot) {
      $("runtime-status").textContent = snapshot.status || "unknown";
      $("runtime-status").className = `pill ${snapshot.status === "healthy" ? "healthy" : ""}`;
      $("updated-at").textContent = snapshot.updated_at || "not-yet-updated";
      $("metric-candidates").textContent = snapshot.candidate_count ?? 0;
      $("metric-total").textContent = snapshot.total_rows ?? 0;
      $("metric-filtered").textContent = snapshot.filtered_funding_count ?? 0;
      $("metric-missing-spot").textContent = snapshot.missing_spot_count ?? 0;
      $("metric-missing-perp").textContent = snapshot.missing_perp_count ?? 0;
      $("metric-min-net").textContent = snapshot.min_net_bps ?? "0";
      $("api-state").textContent = snapshot.last_error ? snapshot.last_error : "ok";
      $("api-preview").textContent = JSON.stringify(snapshot, null, 2);
      renderRows(snapshot.rows || []);
    }

    async function refreshStatus() {
      $("api-state").textContent = "loading";
      try {
        const snapshot = await requestJson(statusUrl);
        state.snapshot = snapshot;
        render(snapshot);
      } catch (error) {
        $("runtime-status").textContent = "error";
        $("runtime-status").className = "pill error";
        $("api-state").textContent = error.message;
      }
    }

    async function previewEndpoint(url) {
      $("api-preview").textContent = "loading";
      try {
        const data = await requestJson(url);
        $("api-preview").textContent = JSON.stringify(data, null, 2);
      } catch (error) {
        $("api-preview").textContent = error.message;
      }
    }

    function schedule() {
      if (state.timer) clearInterval(state.timer);
      if ($("auto-refresh").checked) {
        state.timer = setInterval(refreshStatus, 2000);
      }
    }

    ["symbol-filter", "only-candidates", "only-complete", "sort-mode"].forEach((id) => {
      $(id).addEventListener("input", () => {
        if (state.snapshot) renderRows(state.snapshot.rows || []);
      });
      $(id).addEventListener("change", () => {
        if (state.snapshot) renderRows(state.snapshot.rows || []);
      });
    });
    $("refresh-button").addEventListener("click", refreshStatus);
    $("auto-refresh").addEventListener("change", schedule);
    document.querySelectorAll("[data-endpoint]").forEach((button) => {
      button.addEventListener("click", () => previewEndpoint(button.dataset.endpoint || opportunitiesUrl));
    });

    schedule();
    refreshStatus();
  </script>
</body>
</html>"##
}

struct RuntimeTempDir {
    path: PathBuf,
}

impl RuntimeTempDir {
    fn new() -> RuntimeResult<Self> {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("arb-runtime-s9-{}-{counter}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| RuntimeError::Io {
                path: path.clone(),
                message: error.to_string(),
            })?;
        }
        fs::create_dir_all(&path).map_err(|error| RuntimeError::Io {
            path: path.clone(),
            message: error.to_string(),
        })?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RuntimeTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// CLI 入口。
pub fn main_cli() -> i32 {
    match run_cli(std::env::args().skip(1).collect()) {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err(error) => {
            eprintln!("arb-runtime failed: {error}");
            1
        }
    }
}

fn run_cli(args: Vec<String>) -> RuntimeResult<String> {
    if args.is_empty() || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(help_text());
    }
    if args[0] == "live-market-sim" {
        let options = parse_live_market_sim_args(&args[1..])?;
        let report = run_live_market_simulation(
            &options.fixture_root,
            &options.symbol,
            options.output_dir.clone(),
        )?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: fetched live public market data for {}; execution_mode=Simulated; mutable_execution_started=false; execution_reports={}; incidents={}{}",
            report.symbol,
            count_jsonl_records(&report.artifacts.execution_reports_jsonl),
            count_jsonl_records(&report.artifacts.incidents_jsonl),
            output_note
        ));
    }
    if args[0] == "binance-basis-scan" {
        let options = parse_binance_basis_scan_args(&args[1..])?;
        let report = run_binance_basis_scan(&options.symbol, options.output_dir.clone())?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        let candidate_count = count_jsonl_records(&report.candidate_transitions_jsonl);
        let outcome = report
            .rejection_reason
            .as_ref()
            .map(|reason| {
                format!(
                    "rejected; reason={reason}; detail={}",
                    report.rejection_detail.as_deref().unwrap_or("")
                )
            })
            .unwrap_or_else(|| "candidate=true".to_owned());
        return Ok(format!(
            "ok: fetched Binance public spot/perp basis data for {}; {}; candidate_transitions={}; mutable_execution_started=false{}",
            report.symbol, outcome, candidate_count, output_note
        ));
    }
    if args[0] == "binance-basis-monitor" {
        let options = parse_binance_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_binance_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one Binance basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: Binance basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "health" {
        let fixture_root = args.get(1).map_or_else(
            || PathBuf::from(DEFAULT_FULL_PIPELINE_FIXTURE),
            PathBuf::from,
        );
        let service = start_runtime_for_fixture(&fixture_root)?;
        let health = service.health();
        return Ok(format!(
            "health: {}; execution_mode={}; kill_switch_triggered={}; mutable_execution_started={}; tasks={}",
            health.status.as_str(),
            health.execution_mode,
            health.kill_switch_triggered,
            health.mutable_execution_started,
            health.tasks.len()
        ));
    }
    if args[0] == "health-config" {
        let Some(config_path) = args.get(1) else {
            return Err(cli_arg_error("health-config requires a config path"));
        };
        if args.len() > 2 {
            return Err(cli_arg_error(
                "health-config accepts exactly one config path",
            ));
        }
        let service = start_runtime_from_config_path(config_path)?;
        let health = service.health();
        return Ok(format!(
            "health-config: {}; execution_mode={}; kill_switch_triggered={}; mutable_execution_started={}; tasks={}",
            health.status.as_str(),
            health.execution_mode,
            health.kill_switch_triggered,
            health.mutable_execution_started,
            health.tasks.len()
        ));
    }

    if args[0] != "replay" {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "unknown command `{}`; supported commands: replay, health, health-config, live-market-sim, binance-basis-scan, binance-basis-monitor",
                args[0]
            ),
        });
    }

    let accept_golden = args.iter().skip(1).any(|arg| arg == "--accept");
    let fixture_root = args
        .iter()
        .skip(1)
        .find(|arg| arg.as_str() != "--accept")
        .map_or_else(
            || PathBuf::from(DEFAULT_FULL_PIPELINE_FIXTURE),
            PathBuf::from,
        );
    let report =
        run_full_pipeline_fixture_with_options(&fixture_root, RuntimeOptions { accept_golden })?;
    let action = if accept_golden { "wrote" } else { "matched" };
    Ok(format!(
        "ok: {action} {} S9-01 artifacts for {}",
        report.comparisons.len(),
        report.fixture_root.display()
    ))
}

fn help_text() -> String {
    [
        "arb-runtime",
        "中文说明：只运行离线、模拟、可回放的运行时装配。",
        "中文说明：fixture_root 可使用仓库根目录相对路径，例如 fixtures/replay/full_pipeline_simulated。",
        "",
        "Commands:",
        "  replay [fixture_root] [--accept]  Run S9-01 full pipeline and compare golden outputs",
        "  health [fixture_root]             Run S9-02 startup and health checks only",
        "  health-config <config_path>       Run startup checks against a local config file",
        "  live-market-sim [fixture_root] [--symbol BTCUSDT] [--out dir]",
        "                                    Fetch one public market-data snapshot and run simulated execution",
        "  binance-basis-scan [--symbol BTCUSDT] [--out dir]",
        "                                    Fetch Binance public spot/perp data and run read-only basis strategy",
        "  binance-basis-monitor [--bind 127.0.0.1:8796] [--interval-secs 5] [--min-abs-funding-rate 0] [--min-net-bps 5] [--once] [--out dir]",
        "                                    Monitor all Binance public USDT spot/perp basis rows and serve /api/basis/status",
    ]
    .join("\n")
}

struct LiveMarketSimCliOptions {
    fixture_root: PathBuf,
    symbol: String,
    output_dir: Option<PathBuf>,
}

struct BinanceBasisScanCliOptions {
    symbol: String,
    output_dir: Option<PathBuf>,
}

type BinanceBasisMonitorCliOptions = BinanceBasisMonitorOptions;

fn parse_live_market_sim_args(args: &[String]) -> RuntimeResult<LiveMarketSimCliOptions> {
    let mut fixture_root = PathBuf::from(DEFAULT_FULL_PIPELINE_FIXTURE);
    let mut fixture_seen = false;
    let mut symbol = SIM_SYMBOL.to_owned();
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                symbol = value.clone();
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown live-market-sim option `{value}`"
                )));
            }
            value => {
                if fixture_seen {
                    return Err(cli_arg_error(format!(
                        "unexpected extra fixture path `{value}`"
                    )));
                }
                fixture_root = PathBuf::from(value);
                fixture_seen = true;
            }
        }
        index += 1;
    }

    Ok(LiveMarketSimCliOptions {
        fixture_root,
        symbol,
        output_dir,
    })
}

fn parse_binance_basis_scan_args(args: &[String]) -> RuntimeResult<BinanceBasisScanCliOptions> {
    let mut symbol = BASIS_SYMBOL.to_owned();
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                symbol = value.clone();
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-basis-scan option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-basis-scan positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceBasisScanCliOptions { symbol, output_dir })
}

fn parse_binance_basis_monitor_args(
    args: &[String],
) -> RuntimeResult<BinanceBasisMonitorCliOptions> {
    let mut options = BinanceBasisMonitorOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires an address"));
                };
                options.bind_addr = value.clone();
            }
            "--interval-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--interval-secs requires a value"));
                };
                options.poll_interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--interval-secs must be an integer"))?;
            }
            "--min-abs-funding-rate" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-abs-funding-rate requires a decimal"));
                };
                options.min_abs_funding_rate = value.clone();
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires a value"));
                };
                options.min_net_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--min-net-bps must be an integer"))?;
            }
            "--notional-usd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--notional-usd requires a decimal"));
                };
                options.notional_usd = value.clone();
            }
            "--spot-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-fee-bps requires a value"));
                };
                options.spot_taker_fee_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--spot-fee-bps must be an integer"))?;
            }
            "--perp-fee-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-fee-bps requires a value"));
                };
                options.perp_taker_fee_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--perp-fee-bps must be an integer"))?;
            }
            "--slippage-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--slippage-bps requires a value"));
                };
                options.slippage_buffer_bps = value
                    .parse::<i128>()
                    .map_err(|_| cli_arg_error("--slippage-bps must be an integer"))?;
            }
            "--once" => options.once = true,
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                options.output_dir = Some(PathBuf::from(value));
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_monitor_options(&options)?;
    Ok(options)
}

fn cli_arg_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Module {
        module: "arb-runtime",
        message: message.into(),
    }
}

fn count_jsonl_records(input: &str) -> usize {
    input.lines().filter(|line| !line.trim().is_empty()).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_pipeline_fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/replay/full_pipeline_simulated")
    }

    fn full_pipeline_case_root(case_name: &str) -> PathBuf {
        full_pipeline_fixture_root().join("cases").join(case_name)
    }

    #[test]
    fn repo_relative_fixture_root_resolves_from_crate_subdirectory() {
        let repo_root = workspace_root();
        let crate_dir = repo_root.join("crates").join("arb-runtime");
        let requested = Path::new(DEFAULT_FULL_PIPELINE_FIXTURE);
        let resolved = resolve_fixture_root_from(requested, &crate_dir, &repo_root);

        assert_eq!(resolved, repo_root.join(requested));
        assert!(resolved.join(RISK_POLICY_FILE).is_file());
    }

    #[test]
    fn basis_dashboard_html_requests_realtime_api_paths() {
        let html = basis_dashboard_html();

        assert!(html.contains("/api/basis/status"));
        assert!(html.contains("/api/basis/opportunities"));
        assert!(html.contains("id=\"basis-rows\""));
        assert!(html.contains("fetch(url"));
    }

    #[test]
    fn binance_basis_monitor_snapshot_scans_all_symbols_and_filters_tiny_funding() {
        let spot = r#"[
          {"symbol":"BTCUSDT","bidPrice":"99.90","bidQty":"1.0","askPrice":"100.00","askQty":"2.0"},
          {"symbol":"ETHUSDT","bidPrice":"49.90","bidQty":"3.0","askPrice":"50.00","askQty":"4.0"}
        ]"#;
        let perp = r#"[
          {"symbol":"BTCUSDT","bidPrice":"101.00","bidQty":"1.5","askPrice":"101.10","askQty":"2.5","time":1778584221117},
          {"symbol":"ETHUSDT","bidPrice":"50.10","bidQty":"3.5","askPrice":"50.20","askQty":"4.5","time":1778584221117}
        ]"#;
        let premium = r#"[
          {"symbol":"BTCUSDT","markPrice":"101.00","indexPrice":"100.00","lastFundingRate":"0.00010000","interestRate":"0.00010000","nextFundingTime":1778601600000,"time":1778584220000},
          {"symbol":"ETHUSDT","markPrice":"50.10","indexPrice":"50.00","lastFundingRate":"0.00000001","interestRate":"0.00010000","nextFundingTime":1778601600000,"time":1778584220000}
        ]"#;
        let options = BinanceBasisMonitorOptions {
            min_abs_funding_rate: "0.00000100".to_owned(),
            once: true,
            ..BinanceBasisMonitorOptions::default()
        };

        let snapshot =
            build_binance_basis_monitor_snapshot_from_json(spot, perp, premium, &options)
                .expect("snapshot");

        assert_eq!(snapshot.total_rows, 1);
        assert_eq!(snapshot.filtered_funding_count, 1);
        assert_eq!(snapshot.candidate_count, 1);
        assert_eq!(snapshot.rows[0].symbol, "BTCUSDT");
        assert_eq!(snapshot.rows[0].net_basis_bps.as_deref(), Some("80"));
    }

    #[test]
    fn binance_basis_monitor_snapshot_reports_missing_spot_without_failing_open() {
        let spot = r#"[]"#;
        let perp = r#"[
          {"symbol":"BTCUSDT","bidPrice":"101.00","bidQty":"1.5","askPrice":"101.10","askQty":"2.5","time":1778584221117}
        ]"#;
        let premium = r#"[
          {"symbol":"BTCUSDT","markPrice":"101.00","indexPrice":"100.00","lastFundingRate":"0.00010000","interestRate":"0.00010000","nextFundingTime":1778601600000,"time":1778584220000}
        ]"#;
        let options = BinanceBasisMonitorOptions {
            once: true,
            ..BinanceBasisMonitorOptions::default()
        };

        let snapshot =
            build_binance_basis_monitor_snapshot_from_json(spot, perp, premium, &options)
                .expect("snapshot");

        assert_eq!(snapshot.total_rows, 1);
        assert_eq!(snapshot.candidate_count, 0);
        assert_eq!(snapshot.missing_spot_count, 1);
        assert_eq!(snapshot.rows[0].source_status, "missing_spot");
        assert_eq!(
            snapshot.rows[0].reason.as_deref(),
            Some("MISSING_SPOT_BOOK_TICKER")
        );
    }

    fn simulated_config_yaml() -> &'static str {
        r#"config_version: "arb-config-v1"
execution:
  mode: "Simulated"
  live_execution_enabled: false
  auto_live_enabled: false
kill_switch:
  global: false
  execution: false
  strategies: []
  venues: []
  accounts: []
  instruments: []
  assets: []
  chains: []
  execution_modes: []
signing:
  policy_ref: "signing-policy/null-signer-v1"
  real_signing_enabled: false
"#
    }

    fn guarded_live_blocked_by_kill_switch_yaml() -> &'static str {
        r#"config_version: "arb-config-v1"
execution:
  mode: "GuardedLive"
  live_execution_enabled: true
  auto_live_enabled: false
kill_switch:
  global: false
  execution: false
  strategies: []
  venues: []
  accounts: []
  instruments: []
  assets: []
  chains: []
  execution_modes: ["GuardedLive"]
signing:
  policy_ref: "signing-policy/null-signer-v1"
  real_signing_enabled: false
"#
    }

    #[test]
    fn full_pipeline_fixture_matches_golden_outputs() {
        let report = run_full_pipeline_fixture(full_pipeline_fixture_root())
            .expect("S9-01 full pipeline fixture should match golden outputs");
        assert_eq!(report.comparisons.len(), 10);
        assert!(report
            .artifacts
            .operations_daily_report_md
            .contains("Read-only mode: true"));
    }

    #[test]
    fn same_input_replays_deterministically() {
        let root = full_pipeline_fixture_root();
        let first =
            assemble_full_pipeline(&root).expect("first S9-01 full pipeline run should pass");
        let second =
            assemble_full_pipeline(&root).expect("second S9-01 full pipeline run should pass");
        assert_eq!(first, second);
    }

    #[test]
    fn failure_path_fixtures_reject_and_emit_incidents() {
        for (case_name, expected_reason) in [
            ("stale_data", "DATA_STALE"),
            ("insufficient_balance", "INSUFFICIENT_BALANCE"),
            ("unknown_state", "UNKNOWN_STATE"),
        ] {
            let report = run_full_pipeline_fixture(full_pipeline_case_root(case_name))
                .unwrap_or_else(|error| panic!("{case_name} failure fixture should pass: {error}"));
            assert!(
                report
                    .artifacts
                    .risk_decisions_jsonl
                    .contains(expected_reason),
                "{case_name} risk decision should explain {expected_reason}"
            );
            assert!(
                report.artifacts.incidents_jsonl.contains(expected_reason),
                "{case_name} should produce a traceable incident"
            );
            assert!(
                report.artifacts.execution_plans_jsonl.is_empty(),
                "{case_name} must not generate an execution plan after risk rejection"
            );
            assert!(
                report
                    .artifacts
                    .operations_daily_report_md
                    .contains("Open incidents require operator review."),
                "{case_name} operations report should explain the blocking incident"
            );
        }
    }

    #[test]
    fn config_error_rejects_startup_with_clear_reason() {
        let invalid = r#"config_version: "arb-config-v1"
execution:
  mode: "Simulated"
  live_execution_enabled: false
  auto_live_enabled: false
kill_switch:
  global: false
  execution: false
  strategies: []
  venues: []
  accounts: []
  instruments: []
  assets: []
  chains: []
  execution_modes: []
"#;

        let error =
            start_runtime_from_config_yaml(invalid).expect_err("missing signing config must fail");
        let message = error.to_string();
        assert!(message.contains("arb-config"));
        assert!(message.contains("$.signing"));
        assert!(message.contains("missing required field"));
    }

    #[test]
    fn open_kill_switch_skips_mutable_execution_task() {
        let service = start_runtime_from_config_yaml(guarded_live_blocked_by_kill_switch_yaml())
            .expect("kill switch should allow startup while blocking mutable execution");
        let health = service.health();
        let mutable_task = health
            .task(TASK_MUTABLE_EXECUTION)
            .expect("mutable execution task should be observable");

        assert_eq!(health.status, RuntimeHealthStatus::Degraded);
        assert!(health.kill_switch_triggered);
        assert!(!health.mutable_execution_started);
        assert_eq!(mutable_task.state, RuntimeTaskState::Skipped);
        assert_eq!(
            mutable_task.exit_reason,
            Some(RuntimeTaskExitReason::StartupSkipped)
        );
        assert!(mutable_task.detail.contains("熔断阻止执行模式 GuardedLive"));
    }

    #[test]
    fn health_config_preflight_accepts_guarded_live_when_kill_switch_blocks_it() {
        let _temp_dir = RuntimeTempDir::new().expect("temp dir");
        let config_path = _temp_dir
            .path()
            .join("personal_guarded_live.preflight.yaml");
        fs::write(&config_path, guarded_live_blocked_by_kill_switch_yaml()).expect("write config");

        let service = start_runtime_from_config_path(&config_path)
            .expect("guarded live preflight config should load while kill switch blocks it");
        let health = service.health();

        assert_eq!(health.status, RuntimeHealthStatus::Degraded);
        assert_eq!(health.execution_mode, "GuardedLive");
        assert!(health.kill_switch_triggered);
        assert!(!health.mutable_execution_started);
    }

    #[test]
    fn scoped_chain_kill_switch_is_reported_without_mutable_execution() {
        let scoped_chain =
            simulated_config_yaml().replace("chains: []", "chains: [\"chain:main\"]");
        let service = start_runtime_from_config_yaml(&scoped_chain)
            .expect("scoped chain kill switch should load in simulated mode");
        let health = service.health();

        assert_eq!(health.status, RuntimeHealthStatus::Degraded);
        assert!(health.kill_switch_triggered);
        assert!(!health.mutable_execution_started);
        assert!(health.checks.iter().any(|check| {
            check.name == "circuit-breaker"
                && check.status == RuntimeCheckStatus::Warning
                && check.message.contains("chain:chain:main")
        }));
    }

    #[test]
    fn live_execution_without_circuit_breaker_is_rejected() {
        let unsafe_live = guarded_live_blocked_by_kill_switch_yaml()
            .replace("execution_modes: [\"GuardedLive\"]", "execution_modes: []");
        let error = start_runtime_from_config_yaml(&unsafe_live)
            .expect_err("live execution without kill switch must fail closed");
        let message = error.to_string();
        assert!(message.contains("runtime startup rejected"));
        assert!(message.contains("阶段 9 运行时不能启动可变执行"));
    }

    #[test]
    fn task_exit_is_observable_after_graceful_shutdown() {
        let mut service = start_runtime_from_config_yaml(simulated_config_yaml())
            .expect("simulated runtime startup should pass");
        assert_eq!(service.health().status, RuntimeHealthStatus::Healthy);
        assert_eq!(
            service
                .health()
                .task(TASK_HEALTH_REPORTER)
                .expect("health reporter task")
                .state,
            RuntimeTaskState::Running
        );

        let shutdown = service.request_graceful_shutdown("test requested shutdown");
        assert_eq!(shutdown.health.status, RuntimeHealthStatus::Stopped);
        assert!(shutdown.health.shutdown_requested);
        assert!(shutdown
            .exited_tasks
            .iter()
            .any(|task| task.name == TASK_HEALTH_REPORTER
                && task.exit_reason == Some(RuntimeTaskExitReason::GracefulShutdown)));
        assert!(shutdown
            .health
            .tasks
            .iter()
            .filter(|task| task.name != TASK_MUTABLE_EXECUTION)
            .all(|task| task.state == RuntimeTaskState::Exited));
        assert_eq!(
            shutdown
                .health
                .task(TASK_MUTABLE_EXECUTION)
                .expect("mutable execution task")
                .state,
            RuntimeTaskState::Skipped
        );
    }
}
