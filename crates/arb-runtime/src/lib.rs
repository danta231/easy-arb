//! `arb-runtime` 端到端离线装配入口。
//!
//! 中文说明：默认构建只负责把已有模块按固定 fixture 装配起来。策略规则、风控
//! 规则、账本规则和执行状态机规则仍由对应 crate 提供；默认 feature 不连接真实
//! 交易 API，不下单、不撤单、不转账、不签名。只有显式启用 `live-exec` feature
//! 并通过运行时熔断、审批、签名和确认门禁后，才暴露受控实盘分发入口。

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
    ExecutionMode as ContractExecutionMode, ExecutionPlan, ExecutionReport, Incident, JsonValue,
    LedgerDirection as ContractLedgerDirection, LedgerEntry as ContractLedgerEntry,
    LedgerEntryType as ContractLedgerEntryType, LedgerNamespace as ContractLedgerNamespace,
    NormalizedEvent, NormalizedEventType, PortfolioState, RiskDecision, VenueCapabilityDescriptor,
};
#[cfg(feature = "live-exec")]
use arb_domain::OrderId;
use arb_domain::{
    AccountId, Amount, AssetId, CandidateTransitionId, Decimal, EventId, ExecutionPlanId,
    InstrumentId, LedgerEntryId, Price, Quantity, StrategyId, UtcTimestamp, VenueId,
};
use arb_eventstore::{EventReader, EventWriter, JsonlEventStore};
use arb_execution::{
    build_execution_plan, build_execution_plan_preview, execution_plan_hash,
    release_manual_approval_gate, review_manual_approval, simulate_execution,
    simulated_ledger_entries_from_execution_report, ExecutionPlanBuildInput,
    ManualApprovalDecision, ManualApprovalGateRelease, ManualApprovalMaterial,
    ManualApprovalRecord, ManualApprovalReviewInput, ManualApprovalStatus,
    PendingManualApprovalPlan, PlanBuildOutcome,
};
use arb_ledger::{
    AdjustmentReasonCode, IdempotencyKey, JournalEntryId, LedgerBook,
    LedgerDirection as DomainLedgerDirection, LedgerEntry as DomainLedgerEntry, LedgerEntryDraft,
    LedgerEntryHeader, LedgerEntryType as DomainLedgerEntryType, LedgerLeg, LedgerLegId,
    LedgerNamespace as DomainLedgerNamespace,
};
use arb_ops::{
    generate_manual_approval_material, InMemoryOpsFactReader, ManualApprovalAuditRecord,
    OperationsFacts, OpsCommandOutput, OpsReadOnlyCommand, ReadOnlyOpsEngine,
};
use arb_reconciliation::{
    CoreReconciliationRunner, FeeAmount, FillId, FillSnapshot, ReconciliationReport,
    ReconciliationRequest, ReconciliationRunId, ReconciliationRunner,
};
use arb_replay::{ReplayInput, TimeSource as ReplayTimeSource};
use arb_risk::{RiskEvaluationInput, RiskEvaluator, StaticRiskEvaluator};
#[cfg(feature = "live-exec")]
use arb_strategies::SpotPerpBasisSignal;
use arb_strategies::{
    evaluate_spot_perp_basis_signal, sample_spot_strategy, SpotPerpBasisSignalInput,
    SpotPerpBasisStrategy, SpotPerpBasisStrategyConfig,
};
use arb_strategy_api::{
    CandidateTransitionOutput, FixedTimeSource as StrategyFixedTimeSource, ReadOnlySnapshot,
    Strategy, StrategyConfigSnapshot, StrategyEvaluation, StrategyInput, VenueCapabilitySnapshot,
};
use arb_venue_data::{
    BinancePublicBookTickerAdapter, BinancePublicInstrument, BinancePublicMarket,
    BinancePublicTicker24hAdapter, BinancePublicWssBookTickerClient,
    BinancePublicWssBookTickerConfig, BinancePublicWssTextStreamClient,
    BinanceUsdmPremiumIndexAdapter, DataFreshness, HybridMarketDataInput, HybridMarketDataStatus,
    HybridMarketDataUpdate, MarketDataQuery, MarketDataReader, MarketDataTransport, MarketQuote,
    RestWssMarketDataCoordinator, WssQuoteUpdate,
};

#[cfg(feature = "live-exec")]
use arb_signing::real::{
    BinanceHmacSigningInput, BinanceRequestParam, RealSigningProvider, RealSigningProviderFromEnv,
};
#[cfg(feature = "live-exec")]
use arb_signing::{SigningPolicy, SigningPolicyRef, SigningPurpose, SigningRequestId};
#[cfg(feature = "live-exec")]
use arb_venue_data::{
    BalanceQuery, BalanceReader, BinancePrivateAccountAdapter, BinancePrivateAccountMarket,
    VenueBalance,
};
#[cfg(feature = "live-exec")]
use arb_venue_exec::{
    build_execution_dispatch_plan, live, parse_binance_spot_execution_report_update,
    parse_binance_usdm_order_trade_update, BinancePrivateOrderMarket, CancelOrder,
    CancelOrderRequest, ConfirmOrderStatus, ConfirmOrderStatusRequest, DispatchKillSwitch,
    ExecutionDispatchPlan, ExecutionDispatchPolicy, ExternalActionRef,
    IdempotencyKey as ExecIdempotencyKey, MutableActionKind, MutableActionReceipt,
    MutableActionStatus, MutableOrderType, OrderConfirmationSource, OrderConfirmationStatus,
    OrderReference, OrderSide, PlannedSubmitOrder, PrivateOrderFillUpdate, PrivateOrderUpdate,
    SubmitOrder, SubmitOrderRequest, VenueExecError,
};

const DEFAULT_FULL_PIPELINE_FIXTURE: &str = "fixtures/replay/full_pipeline_simulated";
const BINANCE_MARKET_DATA_BASE_URL: &str = "https://data-api.binance.vision";
const BINANCE_SPOT_REST_BASE_URL: &str = "https://data-api.binance.vision";
const BINANCE_USDM_FUTURES_BASE_URL: &str = "https://fapi.binance.com";
const BYBIT_REST_BASE_URL: &str = "https://api.bybit.com";
const OKX_REST_BASE_URL: &str = "https://www.okx.com";
const HYPERLIQUID_INFO_URL: &str = "https://api.hyperliquid.xyz/info";
const ASTER_SPOT_REST_BASE_URL: &str = "https://sapi.asterdex.com";
const ASTER_FUTURES_REST_BASE_URL: &str = "https://fapi.asterdex.com";
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
const BINANCE_BASIS_PIPELINE_DEFAULT_OUT: &str = "target/binance-basis-pipeline";
const BINANCE_GUARDED_LIVE_PREVIEW_DEFAULT_OUT: &str = "target/binance-guarded-live-preview";
const BINANCE_GUARDED_LIVE_AUTO_ONCE_DEFAULT_OUT: &str = "target/binance-guarded-live-auto-once";
#[cfg(feature = "live-exec")]
const BINANCE_BASIS_GUARDED_LIVE_AUTO_ONCE_DEFAULT_OUT: &str =
    "target/binance-basis-guarded-live-auto-once";
const BINANCE_GUARDED_LIVE_ACCOUNT_REF: &str =
    "account:binance-isolated-personal-cex-subaccount-redacted";
const BINANCE_GUARDED_LIVE_STRATEGY_ID: &str = "strategy:sample-spot-v1";
const BINANCE_GUARDED_LIVE_TRANSITION_ID: &str = "trans:binance-btcusdt-guarded-live-preview-001";
#[cfg(feature = "live-exec")]
const BINANCE_BASIS_LIVE_STRATEGY_ID: &str = "strat:binance-spot-perp-basis";
#[cfg(feature = "live-exec")]
const BINANCE_BASIS_LIVE_TRANSITION_ID: &str = "trans:binance-basis-guarded-live-auto-001";
const BINANCE_GUARDED_LIVE_NOTIONAL_USDT: &str = "10.00";
const BINANCE_GUARDED_LIVE_DAILY_LOSS_LIMIT_USDT: &str = "20.00";
const BINANCE_GUARDED_LIVE_CAPITAL_LIMIT_USDT: &str = "100.00";
const BINANCE_GUARDED_LIVE_QUANTITY_BTC: &str = "0.000100";
const MARKET_DATA_MAX_AGE_MS: u64 = 5_000;
const BASIS_MONITOR_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8796";
const BASIS_MONITOR_DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
const BASIS_MONITOR_DEFAULT_MIN_ABS_FUNDING_RATE: &str = "0";
const BASIS_MONITOR_DEFAULT_NOTIONAL_USD: &str = "100.00";
const BASIS_MONITOR_DEFAULT_SPOT_TAKER_FEE_BPS: i128 = 10;
const BASIS_MONITOR_DEFAULT_PERP_TAKER_FEE_BPS: i128 = 5;
const BASIS_MONITOR_DEFAULT_SLIPPAGE_BUFFER_BPS: i128 = 5;
const BASIS_MONITOR_DEFAULT_MIN_NET_BPS: i128 = 5;
const BYBIT_BASIS_MONITOR_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8797";
const OKX_BASIS_MONITOR_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8798";
const HYPERLIQUID_BASIS_MONITOR_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8799";
const ASTER_BASIS_MONITOR_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8800";
const BINANCE_WSS_BOOK_TICKER_DEFAULT_BIND_ADDR: &str = "127.0.0.1:8801";
const BINANCE_WSS_BOOK_TICKER_DEFAULT_RECONNECT_DELAY_SECS: u64 = 2;
const BINANCE_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS: &str = "ALL_USDT";
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
#[cfg(feature = "live-exec")]
module_error_from!(arb_signing::SigningError, "arb-signing");
#[cfg(feature = "live-exec")]
module_error_from!(arb_venue_exec::VenueExecError, "arb-venue-exec");

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

/// Binance BTCUSDT 受控试运行计划预览结果。
///
/// 中文说明：该结果只生成不可调度的人工审批计划预览和审计材料；不下单、
/// 不撤单、不转账、不签名，也不释放熔断。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceGuardedLivePreviewReport {
    pub symbol: String,
    pub source_event_id: String,
    pub generated_at: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub dispatchable_before_approval: bool,
    pub approval_record_count: usize,
    pub approval_status: Option<String>,
    pub output_dir: Option<PathBuf>,
}

/// Binance BTCUSDT 人工门禁释放预览结果。
///
/// 中文说明：该结果只消费 Approved 人工审批记录并生成门禁释放事实预览；
/// 不分发订单、不下单、不签名、不写账本。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceManualGateReleasePreviewReport {
    pub plan_id: String,
    pub plan_hash: String,
    pub approval_event_id: String,
    pub released_manual_gate: bool,
    pub dependent_transition_count: usize,
    pub dispatchable_after_release: bool,
    pub output_dir: Option<PathBuf>,
}

/// Binance BTCUSDT 分发前 dry run 结果。
///
/// 中文说明：该结果只检查人工门禁释放后的分发前阻断项；它不会调用真实执行、
/// 真实签名、场所私有接口或账户变更路径。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinancePreDispatchDryRunReport {
    pub plan_id: String,
    pub plan_hash: String,
    pub approval_event_id: String,
    pub manual_gate_released: bool,
    pub dispatch_allowed: bool,
    pub blocking_reasons: Vec<String>,
    pub output_dir: Option<PathBuf>,
}

/// Binance BTCUSDT 受控实盘分发选项。
///
/// 中文说明：该选项只由显式 CLI 确认构造。`acknowledge_live_orders` 必须为 true，
/// 否则运行时不会读取凭证、签名或提交真实订单。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceGuardedLiveDispatchOptions {
    pub preview_dir: PathBuf,
    pub config_path: PathBuf,
    pub output_dir: Option<PathBuf>,
    pub acknowledge_live_orders: bool,
}

/// Binance BTCUSDT 受控实盘分发结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceGuardedLiveDispatchReport {
    pub plan_id: String,
    pub plan_hash: String,
    pub approval_event_id: String,
    pub dispatch_allowed: bool,
    pub submitted_receipt_count: usize,
    pub private_confirmation_count: usize,
    pub pre_account_balance_event_id: Option<String>,
    pub post_account_balance_event_id: Option<String>,
    pub execution_report_status: Option<String>,
    pub blocking_reasons: Vec<String>,
    pub output_dir: Option<PathBuf>,
}

/// Binance BTCUSDT 单周期自动链路选项。
///
/// 中文说明：该入口每次从最新公开行情重新生成计划和审批事实。默认只 dry run；
/// 只有 `execute_live` 和 `acknowledge_auto_live_orders` 同时为 true 才会进入真实下单。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceGuardedLiveAutoOnceOptions {
    pub config_path: PathBuf,
    pub output_dir: Option<PathBuf>,
    pub max_ask: Option<String>,
    pub execute_live: bool,
    pub acknowledge_auto_live_orders: bool,
}

/// Binance BTCUSDT 单周期自动链路结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceGuardedLiveAutoOnceReport {
    pub symbol: String,
    pub strategy_id: String,
    pub source_event_id: Option<String>,
    pub best_ask: Option<String>,
    pub max_ask: Option<String>,
    pub signal_allowed: bool,
    pub plan_hash: Option<String>,
    pub approval_event_id: Option<String>,
    pub manual_gate_released: bool,
    pub dispatch_attempted: bool,
    pub dispatch_allowed: bool,
    pub submitted_receipt_count: usize,
    pub private_confirmation_count: usize,
    pub execution_report_status: Option<String>,
    pub blocking_reasons: Vec<String>,
    pub output_dir: Option<PathBuf>,
}

/// Binance spot-perp basis 双腿单周期自动链路选项。
///
/// 中文说明：这是套利链路入口，必须同时生成 spot buy 和 USD-M perp short 两腿。
/// 默认只 dry run；真实下单必须显式设置 `execute_live` 和确认标志。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceBasisGuardedLiveAutoOnceOptions {
    pub config_path: PathBuf,
    pub output_dir: Option<PathBuf>,
    pub min_net_bps: i128,
    pub max_spot_ask: Option<String>,
    pub min_perp_bid: Option<String>,
    pub spot_wss_monitor_url: Option<String>,
    pub perp_wss_monitor_url: Option<String>,
    pub private_order_events_dir: Option<PathBuf>,
    pub execute_live: bool,
    pub acknowledge_basis_live_orders: bool,
}

/// Binance spot-perp basis 双腿单周期自动链路结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceBasisGuardedLiveAutoOnceReport {
    pub symbol: String,
    pub strategy_id: String,
    pub spot_event_id: Option<String>,
    pub perp_event_id: Option<String>,
    pub premium_event_id: Option<String>,
    pub spot_ask: Option<String>,
    pub perp_bid: Option<String>,
    pub net_bps: Option<i128>,
    pub signal_allowed: bool,
    pub plan_hash: Option<String>,
    pub approval_event_id: Option<String>,
    pub manual_gate_released: bool,
    pub dispatch_attempted: bool,
    pub dispatch_allowed: bool,
    pub planned_order_count: usize,
    pub submitted_receipt_count: usize,
    pub private_confirmation_count: usize,
    pub protection_attempted: bool,
    pub protection_actions: Vec<String>,
    pub protection_receipt_count: usize,
    pub residual_risk: Option<String>,
    pub execution_report_status: Option<String>,
    pub blocking_reasons: Vec<String>,
    pub output_dir: Option<PathBuf>,
}

/// Binance BTCUSDT 人工确认请求。
///
/// 中文说明：只有调用方显式提供同一个 `expected_plan_hash` 时才会生成审批记录。
/// 该请求不会触发真实执行或签名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceManualApprovalDecisionRequest {
    pub decision: ManualApprovalDecision,
    pub expected_plan_hash: String,
    pub approval_event_id: String,
    pub reviewer_id: String,
    pub decided_at: String,
    pub expires_at: String,
    pub reason: String,
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

/// Binance `bookTicker` WSS 最新报价快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceWssBookTickerQuoteSnapshot {
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

/// Binance `bookTicker` WSS 常驻任务状态快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinanceWssBookTickerMonitorSnapshot {
    pub status: String,
    pub updated_at: String,
    pub symbol: String,
    pub market: String,
    pub stream_url: String,
    pub coordinator_status: String,
    pub latest_quote: Option<BinanceWssBookTickerQuoteSnapshot>,
    pub rows: Vec<BinanceWssBookTickerQuoteSnapshot>,
    pub total_rows: usize,
    pub fail_closed: bool,
    pub fail_closed_count: u64,
    pub disconnect_count: u64,
    pub rest_rebuild_count: u64,
    pub wss_update_count: u64,
    pub last_error: Option<String>,
}

/// Bybit basis 长驻监控选项。
///
/// 中文说明：监控只使用 Bybit V5 公开行情接口，不读取私有账户数据，不提交订单。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitBasisMonitorOptions {
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

impl Default for BybitBasisMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: BYBIT_BASIS_MONITOR_DEFAULT_BIND_ADDR.to_owned(),
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

/// OKX basis 长驻监控选项。
///
/// 中文说明：监控只使用 OKX V5 公开行情和公开资金费率接口，不读取私有账户数据，
/// 不提交订单、不撤单、不转账、不签名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OkxBasisMonitorOptions {
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

impl Default for OkxBasisMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: OKX_BASIS_MONITOR_DEFAULT_BIND_ADDR.to_owned(),
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

/// Hyperliquid basis 长驻监控选项。
///
/// 中文说明：监控只使用 Hyperliquid 官方公开 `info` 数据端点，不读取私有账户、
/// 不调用 `exchange` 写接口，不下单、不撤单、不转账、不签名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidBasisMonitorOptions {
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

impl Default for HyperliquidBasisMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: HYPERLIQUID_BASIS_MONITOR_DEFAULT_BIND_ADDR.to_owned(),
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

/// Aster basis 长驻监控选项。
///
/// 中文说明：监控只使用 Aster 公开 spot/perp V3 行情端点，不读取私有账户、
/// 不下单、不撤单、不转账、不签名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AsterBasisMonitorOptions {
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

impl Default for AsterBasisMonitorOptions {
    fn default() -> Self {
        Self {
            bind_addr: ASTER_BASIS_MONITOR_DEFAULT_BIND_ADDR.to_owned(),
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

/// Bybit basis 行情行。
///
/// 中文说明：字段语义与 Binance monitor 对齐，来源换成 Bybit 公开行情。
pub type BybitBasisMarketRow = BinanceBasisMarketRow;

/// Bybit basis 监控快照。
///
/// 中文说明：复用同一只读快照合同，避免 dashboard/API 在不同场所间分叉。
pub type BybitBasisMonitorSnapshot = BinanceBasisMonitorSnapshot;

/// OKX basis 行情行。
///
/// 中文说明：字段语义与 Binance monitor 对齐，来源换成 OKX 公开行情。
pub type OkxBasisMarketRow = BinanceBasisMarketRow;

/// OKX basis 监控快照。
///
/// 中文说明：复用同一只读快照合同，避免 dashboard/API 在不同场所间分叉。
pub type OkxBasisMonitorSnapshot = BinanceBasisMonitorSnapshot;

/// Hyperliquid basis 行情行。
///
/// 中文说明：字段语义与 Binance monitor 对齐；Hyperliquid 公共上下文只提供
/// mid/mark/oracle/funding，所以 spot/perp bid/ask 字段使用公开 mid price 代理。
pub type HyperliquidBasisMarketRow = BinanceBasisMarketRow;

/// Hyperliquid basis 监控快照。
///
/// 中文说明：复用同一只读快照合同，便于 dashboard 和实时 API 保持一致。
pub type HyperliquidBasisMonitorSnapshot = BinanceBasisMonitorSnapshot;

/// Aster basis 行情行。
///
/// 中文说明：字段语义与 Binance monitor 对齐，来源换成 Aster 公开 spot/perp 行情。
pub type AsterBasisMarketRow = BinanceBasisMarketRow;

/// Aster basis 监控快照。
///
/// 中文说明：复用同一只读快照合同，避免 dashboard/API 在不同场所间分叉。
pub type AsterBasisMonitorSnapshot = BinanceBasisMonitorSnapshot;

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
        if cfg!(feature = "live-exec") {
            RuntimeCheck::pass(
                "real-signing",
                "真实签名已显式开启，live-exec 构建允许继续实盘预检",
            )
        } else {
            RuntimeCheck::fail(
                "real-signing",
                "默认运行时禁止真实签名；real_signing_enabled 必须为 false，或使用 live-exec 构建",
            )
        }
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
    } else if !cfg!(feature = "live-exec") {
        RuntimeCheck::fail(
            "execution-permission",
            format!("执行模式 {mode} 会请求可变账户权限，但阶段 9 运行时不能启动可变执行；默认构建必须保持关闭，或使用 live-exec 构建"),
        )
    } else if !config.signing().real_signing_enabled() {
        RuntimeCheck::fail(
            "execution-permission",
            format!("执行模式 {mode} 会请求可变账户权限，但 real_signing_enabled 未开启"),
        )
    } else {
        RuntimeCheck::pass(
            "execution-permission",
            format!("执行模式 {mode} 已通过 live-exec 可变执行启动预检"),
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
    } else if cfg!(feature = "live-exec") {
        tasks.push_running(
            TASK_MUTABLE_EXECUTION,
            "live-exec 构建已装配可变执行任务；具体下单仍需显式分发命令、审批和确认门禁",
        );
    } else {
        tasks.push_failed(
            TASK_MUTABLE_EXECUTION,
            "启动检查应在默认构建允许可变执行前拒绝运行时",
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
    let spec = BasisPipelineSpec::binance_btcusdt()?;

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
        build_public_basis_portfolio_state(&spec, &source_event_refs, ingested_at)?;
    ensure_portfolio_state_source_refs_exist(&portfolio_state, &stored_events)?;
    let evaluation = run_spot_perp_basis_strategy(
        replay.config(),
        &stored_events,
        &portfolio_state,
        &ingested_at.to_string(),
        &spec,
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

/// 生成 Binance BTCUSDT GuardedLivePersonal 计划预览。
///
/// 中文说明：该函数只读取已保存的公开行情 artifact，并生成不可调度的人工审批
/// 计划预览；它不访问私有账户、不下单、不撤单、不转账、不签名。
pub fn run_binance_guarded_live_preview(
    market_artifacts_dir: impl AsRef<Path>,
    output_dir: Option<PathBuf>,
    decision_request: Option<BinanceManualApprovalDecisionRequest>,
) -> RuntimeResult<BinanceGuardedLivePreviewReport> {
    let market_artifacts_dir = market_artifacts_dir.as_ref();
    let quote = load_binance_guarded_live_spot_quote(market_artifacts_dir)?;
    let generated_at = current_utc_timestamp()?.to_string();
    let plan_created_at = quote.observed_at.clone();
    let candidate = binance_guarded_live_candidate(&quote, &plan_created_at)?;
    let risk_decision = binance_guarded_live_manual_risk_decision(&quote, &generated_at)?;
    let outcome = build_execution_plan_preview(ExecutionPlanBuildInput::new(
        &risk_decision,
        &candidate,
        ContractExecutionMode::GuardedLive,
        &plan_created_at,
    ))?;
    let pending = match outcome {
        PlanBuildOutcome::PendingManualApproval(pending) => pending,
        PlanBuildOutcome::Schedulable(_) => {
            return Err(RuntimeError::UnsafeConfig {
                message: "Binance GuardedLive preview unexpectedly produced a dispatchable plan"
                    .to_owned(),
            });
        }
    };
    let plan_hash = execution_plan_hash(&pending.plan_preview);
    if plan_hash != pending.approval_material.plan_hash {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: "manual approval material plan_hash does not match canonical plan hash"
                .to_owned(),
        });
    }

    let mut approval_records = Vec::new();
    if let Some(request) = decision_request {
        if request.expected_plan_hash != plan_hash {
            return Err(RuntimeError::UnsafeConfig {
                message: format!(
                    "expected plan hash `{}` does not match current plan hash `{plan_hash}`",
                    request.expected_plan_hash
                ),
            });
        }
        approval_records.push(review_manual_approval(
            ManualApprovalReviewInput::new(
                &pending,
                &request.approval_event_id,
                &request.reviewer_id,
                &request.decided_at,
                &request.expires_at,
                request.decision,
            )
            .with_reason(&request.reason),
        )?);
    }
    let approval_audit_records = approval_records
        .iter()
        .map(manual_approval_audit_record_from_execution)
        .collect::<Vec<_>>();
    let manual_material = generate_manual_approval_material(
        &risk_decision,
        &pending.plan_preview,
        &plan_hash,
        &approval_audit_records,
        &generated_at,
    )?;
    let manual_material_md = manual_material.render_markdown()?;
    let approval_records_jsonl = manual_approval_records_jsonl(&approval_records);
    let confirmation_template_md = binance_manual_confirmation_template(
        &pending.plan_preview,
        &plan_hash,
        &generated_at,
        &approval_records,
    );

    let output_dir =
        Some(output_dir.unwrap_or_else(|| PathBuf::from(BINANCE_GUARDED_LIVE_PREVIEW_DEFAULT_OUT)));
    if let Some(dir) = &output_dir {
        write_binance_guarded_live_preview_artifacts(
            dir,
            BinanceGuardedLivePreviewArtifacts {
                candidate: &candidate,
                risk_decision: &risk_decision,
                plan_preview: &pending.plan_preview,
                plan_hash: &plan_hash,
                manual_material_md: &manual_material_md,
                approval_records_jsonl: &approval_records_jsonl,
                confirmation_template_md: &confirmation_template_md,
            },
        )?;
    }

    Ok(BinanceGuardedLivePreviewReport {
        symbol: BASIS_SYMBOL.to_owned(),
        source_event_id: quote.event_id,
        generated_at,
        plan_id: pending.plan_preview.plan_id.as_str().to_owned(),
        plan_hash,
        dispatchable_before_approval: pending.is_dispatchable(),
        approval_record_count: approval_records.len(),
        approval_status: approval_records
            .first()
            .map(|record| record.status.as_str().to_owned()),
        output_dir,
    })
}

/// 预览释放 Binance BTCUSDT 人工审批门禁。
///
/// 中文说明：该函数只消费本地 Approved 审批记录并生成可审计释放事实预览；
/// 不分发订单、不下单、不撤单、不转账、不签名。
pub fn run_binance_manual_gate_release_preview(
    preview_dir: impl AsRef<Path>,
    output_dir: Option<PathBuf>,
) -> RuntimeResult<BinanceManualGateReleasePreviewReport> {
    let preview_dir = preview_dir.as_ref();
    let (pending, approved_record) = load_binance_manual_gate_release_inputs(preview_dir)?;
    let release = release_manual_approval_gate(&pending, &approved_record)?;
    let dispatchable_after_release = false;
    let generated_at = current_utc_timestamp()?.to_string();
    let output_dir = Some(output_dir.unwrap_or_else(|| preview_dir.to_path_buf()));
    if let Some(dir) = &output_dir {
        write_binance_manual_gate_release_artifacts(
            dir,
            &release,
            dispatchable_after_release,
            &generated_at,
        )?;
    }

    Ok(BinanceManualGateReleasePreviewReport {
        plan_id: release.plan_id,
        plan_hash: release.plan_hash,
        approval_event_id: release.approval_event_id,
        released_manual_gate: release.gate_transition.to_state.as_str() == "Ready",
        dependent_transition_count: release.dependent_transitions.len(),
        dispatchable_after_release,
        output_dir,
    })
}

/// 运行 Binance BTCUSDT 分发前 dry run。
///
/// 中文说明：dry run 只检查释放人工门禁后的分发阻断项。当前命令必须保持
/// fail-closed：不会调用真实交易 API，不会写账本，不会签名，不会下单。
pub fn run_binance_pre_dispatch_dry_run(
    preview_dir: impl AsRef<Path>,
    config_path: impl AsRef<Path>,
    output_dir: Option<PathBuf>,
) -> RuntimeResult<BinancePreDispatchDryRunReport> {
    let preview_dir = preview_dir.as_ref();
    let config_path = config_path.as_ref();
    let release_report = run_binance_manual_gate_release_preview(preview_dir, output_dir.clone())?;
    let service = start_runtime_from_config_path(config_path)?;
    let health = service.health();
    let mut blocking_reasons = Vec::new();
    if !release_report.released_manual_gate {
        blocking_reasons.push("manual gate release preview is not Ready".to_owned());
    }
    if health.kill_switch_triggered {
        blocking_reasons.push("kill switch is triggered for the preflight config".to_owned());
    }
    if !health.mutable_execution_started {
        blocking_reasons.push("mutable execution task is not started".to_owned());
    }
    blocking_reasons.push(
        "pre-dispatch dry run does not enable live-exec, real signing, private balance reads, or order dispatch"
            .to_owned(),
    );
    blocking_reasons.push(
        "approval releases only the manual gate; execution mode, capital, ledger, reconciliation, permission, and signer gates remain mandatory"
            .to_owned(),
    );
    let dispatch_allowed = false;
    let generated_at = current_utc_timestamp()?.to_string();
    let output_dir = Some(output_dir.unwrap_or_else(|| preview_dir.to_path_buf()));
    if let Some(dir) = &output_dir {
        write_binance_pre_dispatch_dry_run_artifacts(
            dir,
            &release_report,
            &health,
            &blocking_reasons,
            dispatch_allowed,
            &generated_at,
        )?;
    }

    Ok(BinancePreDispatchDryRunReport {
        plan_id: release_report.plan_id,
        plan_hash: release_report.plan_hash,
        approval_event_id: release_report.approval_event_id,
        manual_gate_released: release_report.released_manual_gate,
        dispatch_allowed,
        blocking_reasons,
        output_dir,
    })
}

/// 运行 Binance BTCUSDT GuardedLive 受控实盘分发。
///
/// 中文说明：默认构建始终拒绝。`live-exec` 构建会先检查人工审批、熔断、真实签名、
/// 私有账户余额和分发白名单；真实 REST 下单后仍必须通过私有查单确认生成执行报告。
pub fn run_binance_guarded_live_dispatch(
    options: BinanceGuardedLiveDispatchOptions,
) -> RuntimeResult<BinanceGuardedLiveDispatchReport> {
    #[cfg(feature = "live-exec")]
    {
        run_binance_guarded_live_dispatch_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message: "当前 arb-runtime 未使用 live-exec feature 构建，拒绝真实 Binance 下单"
                .to_owned(),
        })
    }
}

/// 运行一次 Binance BTCUSDT 自动链路。
///
/// 中文说明：该函数会读取一次最新公开 spot bookTicker，生成新的策略候选和计划，
/// 自动生成同一 plan_hash 的受控审批事实，然后进入 dry-run 或显式实盘分发。
pub fn run_binance_guarded_live_auto_once(
    options: BinanceGuardedLiveAutoOnceOptions,
) -> RuntimeResult<BinanceGuardedLiveAutoOnceReport> {
    let source_url = binance_spot_book_ticker_url(BASIS_SYMBOL);
    let raw_spot_book = fetch_public_json_with_curl(&source_url)?;
    let ingested_at = current_utc_timestamp()?;
    run_binance_guarded_live_auto_once_from_spot_json(
        &raw_spot_book,
        &source_url,
        ingested_at,
        options,
    )
}

fn run_binance_guarded_live_auto_once_from_spot_json(
    raw_spot_book: &str,
    raw_response_ref: &str,
    ingested_at: UtcTimestamp,
    options: BinanceGuardedLiveAutoOnceOptions,
) -> RuntimeResult<BinanceGuardedLiveAutoOnceReport> {
    let output_root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(BINANCE_GUARDED_LIVE_AUTO_ONCE_DEFAULT_OUT));
    let output_dir = Some(output_root.clone());
    let market_dir = output_root.join("market");
    let preview_dir = output_root.join("preview");
    let dry_run_dir = output_root.join("dry-run");
    let live_dispatch_dir = output_root.join("live-dispatch");

    let quote = write_binance_guarded_live_auto_market_artifacts(
        raw_spot_book,
        raw_response_ref,
        ingested_at,
        &market_dir,
    )?;
    let mut blocking_reasons = Vec::new();
    if let Some(max_ask) = &options.max_ask {
        if Decimal::from_str(&quote.best_ask)? > Decimal::from_str(max_ask)? {
            blocking_reasons.push(format!(
                "strategy signal blocked: best_ask={} is above max_ask={max_ask}",
                quote.best_ask
            ));
        }
    } else if options.execute_live {
        blocking_reasons.push(
            "live automatic execution requires --max-ask so the strategy has an explicit price guard"
                .to_owned(),
        );
    }

    if !blocking_reasons.is_empty() {
        let report = BinanceGuardedLiveAutoOnceReport {
            symbol: BASIS_SYMBOL.to_owned(),
            strategy_id: BINANCE_GUARDED_LIVE_STRATEGY_ID.to_owned(),
            source_event_id: Some(quote.event_id),
            best_ask: Some(quote.best_ask),
            max_ask: options.max_ask,
            signal_allowed: false,
            plan_hash: None,
            approval_event_id: None,
            manual_gate_released: false,
            dispatch_attempted: false,
            dispatch_allowed: false,
            submitted_receipt_count: 0,
            private_confirmation_count: 0,
            execution_report_status: None,
            blocking_reasons,
            output_dir,
        };
        write_binance_guarded_live_auto_once_artifacts(&output_root, &report)?;
        return Ok(report);
    }

    let preview = run_binance_guarded_live_preview(&market_dir, Some(preview_dir.clone()), None)?;
    let approval_event_id = format!(
        "event:approval:auto-live:binance-btcusdt:{}",
        ingested_at.unix_seconds()
    );
    let decided_at = ingested_at.to_string();
    let expires_at =
        UtcTimestamp::from_unix_parts(ingested_at.unix_seconds() + 300, 0)?.to_string();
    let approved_preview = run_binance_guarded_live_preview(
        &market_dir,
        Some(preview_dir.clone()),
        Some(BinanceManualApprovalDecisionRequest {
            decision: ManualApprovalDecision::Approve,
            expected_plan_hash: preview.plan_hash.clone(),
            approval_event_id: approval_event_id.clone(),
            reviewer_id: "system:guarded-live-auto-strategy".to_owned(),
            decided_at,
            expires_at,
            reason: "Automatic guarded-live strategy signal approved for the same fresh plan hash."
                .to_owned(),
        }),
    )?;
    let release = run_binance_manual_gate_release_preview(&preview_dir, Some(preview_dir.clone()))?;

    let (
        dispatch_attempted,
        dispatch_allowed,
        submitted_receipt_count,
        private_confirmation_count,
        execution_report_status,
        mut dispatch_reasons,
    ) = if options.execute_live {
        if !options.acknowledge_auto_live_orders {
            return Err(RuntimeError::UnsafeConfig {
                message: "缺少 --i-understand-auto-live-orders，拒绝进入自动真实下单链路"
                    .to_owned(),
            });
        }
        let dispatch = run_binance_guarded_live_dispatch(BinanceGuardedLiveDispatchOptions {
            preview_dir: preview_dir.clone(),
            config_path: options.config_path.clone(),
            output_dir: Some(live_dispatch_dir),
            acknowledge_live_orders: true,
        })?;
        (
            true,
            dispatch.dispatch_allowed,
            dispatch.submitted_receipt_count,
            dispatch.private_confirmation_count,
            dispatch.execution_report_status,
            dispatch.blocking_reasons,
        )
    } else {
        let dry_run = run_binance_pre_dispatch_dry_run(
            &preview_dir,
            &options.config_path,
            Some(dry_run_dir),
        )?;
        let mut reasons = dry_run.blocking_reasons;
        reasons.push(
                "automatic chain ran in dry-run mode; pass --execute-live and --i-understand-auto-live-orders to submit a real order"
                    .to_owned(),
            );
        (false, false, 0, 0, None, reasons)
    };

    let mut report_reasons = Vec::new();
    report_reasons.append(&mut dispatch_reasons);
    let report = BinanceGuardedLiveAutoOnceReport {
        symbol: BASIS_SYMBOL.to_owned(),
        strategy_id: BINANCE_GUARDED_LIVE_STRATEGY_ID.to_owned(),
        source_event_id: Some(quote.event_id),
        best_ask: Some(quote.best_ask),
        max_ask: options.max_ask,
        signal_allowed: true,
        plan_hash: Some(approved_preview.plan_hash),
        approval_event_id: Some(release.approval_event_id),
        manual_gate_released: release.released_manual_gate,
        dispatch_attempted,
        dispatch_allowed,
        submitted_receipt_count,
        private_confirmation_count,
        execution_report_status,
        blocking_reasons: report_reasons,
        output_dir,
    };
    write_binance_guarded_live_auto_once_artifacts(&output_root, &report)?;
    Ok(report)
}

/// 运行一次 Binance spot-perp basis 双腿自动套利链路。
pub fn run_binance_basis_guarded_live_auto_once(
    options: BinanceBasisGuardedLiveAutoOnceOptions,
) -> RuntimeResult<BinanceBasisGuardedLiveAutoOnceReport> {
    #[cfg(feature = "live-exec")]
    {
        run_binance_basis_guarded_live_auto_once_live(options)
    }
    #[cfg(not(feature = "live-exec"))]
    {
        let _ = options;
        Err(RuntimeError::UnsafeConfig {
            message:
                "当前 arb-runtime 未使用 live-exec feature 构建，拒绝 Binance basis 实盘自动链路"
                    .to_owned(),
        })
    }
}

#[cfg(feature = "live-exec")]
fn run_binance_basis_guarded_live_auto_once_live(
    options: BinanceBasisGuardedLiveAutoOnceOptions,
) -> RuntimeResult<BinanceBasisGuardedLiveAutoOnceReport> {
    let spot_url = binance_spot_book_ticker_url(BASIS_SYMBOL);
    let perp_url = binance_usdm_book_ticker_url(BASIS_SYMBOL);
    let premium_url = binance_usdm_premium_index_url(BASIS_SYMBOL);
    let wss_monitor_urls = match (
        options.spot_wss_monitor_url.as_deref(),
        options.perp_wss_monitor_url.as_deref(),
    ) {
        (Some(spot_monitor), Some(perp_monitor)) => Some((spot_monitor, perp_monitor)),
        (None, None) if !options.execute_live => None,
        (None, None) => {
            return Err(RuntimeError::UnsafeConfig {
                message:
                    "真实 basis 自动下单必须提供 --spot-wss-monitor-url 和 --perp-wss-monitor-url"
                        .to_owned(),
            });
        }
        _ => {
            return Err(RuntimeError::UnsafeConfig {
                message:
                    "basis 自动链路必须同时提供 --spot-wss-monitor-url 和 --perp-wss-monitor-url"
                        .to_owned(),
            });
        }
    };
    let (raw_spot, spot_ref, raw_perp, perp_ref) =
        if let Some((spot_monitor, perp_monitor)) = wss_monitor_urls {
            let spot_snapshot = fetch_binance_basis_wss_monitor_book_ticker_json(
                spot_monitor,
                BinancePublicMarket::Spot,
                BASIS_SPOT_VENUE_ID,
                BASIS_SPOT_INSTRUMENT_ID,
            )?;
            let perp_snapshot = fetch_binance_basis_wss_monitor_book_ticker_json(
                perp_monitor,
                BinancePublicMarket::UsdmPerpetual,
                BASIS_PERP_VENUE_ID,
                BASIS_PERP_INSTRUMENT_ID,
            )?;
            (
                spot_snapshot.raw_book_ticker_json,
                spot_snapshot.source_ref,
                perp_snapshot.raw_book_ticker_json,
                perp_snapshot.source_ref,
            )
        } else {
            (
                fetch_public_json_with_curl(&spot_url)?,
                spot_url.clone(),
                fetch_public_json_with_curl(&perp_url)?,
                perp_url.clone(),
            )
        };
    let raw_premium = fetch_public_json_with_curl(&premium_url)?;
    let ingested_at = current_utc_timestamp()?;
    run_binance_basis_guarded_live_auto_once_from_json(
        &raw_spot,
        &spot_ref,
        &raw_perp,
        &perp_ref,
        &raw_premium,
        &premium_url,
        ingested_at,
        options,
    )
}

#[cfg(feature = "live-exec")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct BinanceBasisWssMonitorBookTicker {
    raw_book_ticker_json: String,
    source_ref: String,
}

#[cfg(feature = "live-exec")]
fn fetch_binance_basis_wss_monitor_book_ticker_json(
    monitor_url: &str,
    market: BinancePublicMarket,
    expected_venue_id: &str,
    expected_instrument_id: &str,
) -> RuntimeResult<BinanceBasisWssMonitorBookTicker> {
    let endpoint = normalize_binance_wss_monitor_status_url(monitor_url);
    let raw_monitor_snapshot = fetch_public_json_with_curl(&endpoint)?;
    let fetched_at = current_utc_timestamp()?;
    let quote = parse_binance_wss_monitor_quote_for_basis(
        &raw_monitor_snapshot,
        &endpoint,
        BASIS_SYMBOL,
        market,
        expected_venue_id,
        expected_instrument_id,
        fetched_at,
    )?;
    let source_event_id = quote
        .source_event_id
        .as_deref()
        .unwrap_or("missing-source-event-id");
    Ok(BinanceBasisWssMonitorBookTicker {
        raw_book_ticker_json: binance_wss_monitor_quote_to_book_ticker_json(&quote)?,
        source_ref: format!("wss-monitor:{endpoint}#{source_event_id}"),
    })
}

#[cfg(feature = "live-exec")]
fn normalize_binance_wss_monitor_status_url(input: &str) -> String {
    let mut url = input.trim().trim_end_matches('/').to_owned();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("http://{url}");
    }
    if url.ends_with("/api/binance-wss-book-ticker") {
        format!("{url}/status")
    } else if url.contains("/api/binance-wss-book-ticker/") || url.ends_with("/health") {
        url
    } else {
        format!("{url}/api/binance-wss-book-ticker/status")
    }
}

#[cfg(feature = "live-exec")]
#[allow(clippy::too_many_arguments)]
fn parse_binance_wss_monitor_quote_for_basis(
    raw_json: &str,
    monitor_ref: &str,
    symbol: &str,
    expected_market: BinancePublicMarket,
    expected_venue_id: &str,
    expected_instrument_id: &str,
    fetched_at: UtcTimestamp,
) -> RuntimeResult<BinanceWssBookTickerQuoteSnapshot> {
    let fields = parse_json_object_value_slices(raw_json)?;
    let status = required_json_value_string(&fields, "status", "Binance WSS monitor")?;
    if status != "streaming" {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS monitor `{monitor_ref}` status is `{status}`, not streaming"
            ),
        });
    }
    if optional_json_bool(&fields, "fail_closed", "Binance WSS monitor")?.unwrap_or(true) {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS monitor `{monitor_ref}` is fail-closed or lacks fail_closed state"
            ),
        });
    }
    if let Some(error) = optional_json_value_string(&fields, "last_error", "Binance WSS monitor")? {
        if !error.trim().is_empty() {
            return Err(RuntimeError::LiveMarketData {
                message: format!(
                    "Binance WSS monitor `{monitor_ref}` reports last_error `{error}`"
                ),
            });
        }
    }
    if let Some(market) = optional_json_value_string(&fields, "market", "Binance WSS monitor")? {
        if market != expected_market.as_str() {
            return Err(RuntimeError::LiveMarketData {
                message: format!(
                    "Binance WSS monitor `{monitor_ref}` market `{market}` does not match expected `{}`",
                    expected_market.as_str()
                ),
            });
        }
    }
    if let Some(wss_updates) =
        optional_json_value_u64(&fields, "wss_update_count", "Binance WSS monitor")?
    {
        if wss_updates == 0 {
            return Err(RuntimeError::LiveMarketData {
                message: format!("Binance WSS monitor `{monitor_ref}` has no WSS updates yet"),
            });
        }
    }

    let quote = find_binance_wss_monitor_quote(&fields, symbol, monitor_ref)?;
    ensure_binance_wss_monitor_quote_usable(
        &quote,
        monitor_ref,
        symbol,
        expected_venue_id,
        expected_instrument_id,
        fetched_at,
    )?;
    Ok(quote)
}

#[cfg(feature = "live-exec")]
fn optional_json_value_u64(
    fields: &BTreeMap<String, &str>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<Option<u64>> {
    let Some(value) = optional_json_value_string(fields, field, source)? else {
        return Ok(None);
    };
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|error| RuntimeError::LiveMarketData {
            message: format!("{source} field `{field}` is not an unsigned integer: {error}"),
        })
}

#[cfg(feature = "live-exec")]
fn find_binance_wss_monitor_quote(
    fields: &BTreeMap<String, &str>,
    symbol: &str,
    monitor_ref: &str,
) -> RuntimeResult<BinanceWssBookTickerQuoteSnapshot> {
    if let Some(rows_value) = fields.get("rows") {
        for row in json_array_value_slices(rows_value)? {
            let quote = parse_binance_wss_monitor_quote_snapshot(row)?;
            if quote.symbol == symbol {
                return Ok(quote);
            }
        }
    }
    if let Some(value) = fields.get("latest_quote") {
        if value.trim() != "null" {
            let quote = parse_binance_wss_monitor_quote_snapshot(value)?;
            if quote.symbol == symbol {
                return Ok(quote);
            }
        }
    }
    Err(RuntimeError::LiveMarketData {
        message: format!("Binance WSS monitor `{monitor_ref}` lacks quote row for `{symbol}`"),
    })
}

#[cfg(feature = "live-exec")]
fn parse_binance_wss_monitor_quote_snapshot(
    value: &str,
) -> RuntimeResult<BinanceWssBookTickerQuoteSnapshot> {
    let fields = parse_json_object_value_slices(value)?;
    Ok(BinanceWssBookTickerQuoteSnapshot {
        symbol: required_json_value_string(&fields, "symbol", "Binance WSS monitor quote")?,
        venue_id: required_json_value_string(&fields, "venue_id", "Binance WSS monitor quote")?,
        instrument_id: required_json_value_string(
            &fields,
            "instrument_id",
            "Binance WSS monitor quote",
        )?,
        best_bid: optional_json_value_string(&fields, "best_bid", "Binance WSS monitor quote")?,
        best_ask: optional_json_value_string(&fields, "best_ask", "Binance WSS monitor quote")?,
        bid_size: optional_json_value_string(&fields, "bid_size", "Binance WSS monitor quote")?,
        ask_size: optional_json_value_string(&fields, "ask_size", "Binance WSS monitor quote")?,
        source_sequence: optional_json_value_string(
            &fields,
            "source_sequence",
            "Binance WSS monitor quote",
        )?,
        source_event_id: optional_json_value_string(
            &fields,
            "source_event_id",
            "Binance WSS monitor quote",
        )?,
        observed_at: required_json_value_string(
            &fields,
            "observed_at",
            "Binance WSS monitor quote",
        )?,
        ingested_at: required_json_value_string(
            &fields,
            "ingested_at",
            "Binance WSS monitor quote",
        )?,
        freshness_status: required_json_value_string(
            &fields,
            "freshness_status",
            "Binance WSS monitor quote",
        )?,
    })
}

#[cfg(feature = "live-exec")]
fn ensure_binance_wss_monitor_quote_usable(
    quote: &BinanceWssBookTickerQuoteSnapshot,
    monitor_ref: &str,
    symbol: &str,
    expected_venue_id: &str,
    expected_instrument_id: &str,
    fetched_at: UtcTimestamp,
) -> RuntimeResult<()> {
    if quote.symbol != symbol {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS monitor `{monitor_ref}` quote symbol `{}` does not match `{symbol}`",
                quote.symbol
            ),
        });
    }
    if quote.venue_id != expected_venue_id || quote.instrument_id != expected_instrument_id {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS monitor `{monitor_ref}` quote identity `{}` `{}` does not match `{expected_venue_id}` `{expected_instrument_id}`",
                quote.venue_id, quote.instrument_id
            ),
        });
    }
    if quote.freshness_status != "Fresh" {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS monitor `{monitor_ref}` quote freshness is `{}`",
                quote.freshness_status
            ),
        });
    }
    let source_sequence = quote
        .source_sequence
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("Binance WSS monitor `{monitor_ref}` quote lacks source_sequence"),
        })?;
    source_sequence
        .parse::<u64>()
        .map_err(|error| RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS monitor `{monitor_ref}` source_sequence `{source_sequence}` is not numeric: {error}"
            ),
        })?;
    let source_event_id = quote
        .source_event_id
        .as_deref()
        .filter(|value| value.contains(":wss-book-ticker:"))
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS monitor `{monitor_ref}` latest `{symbol}` quote is not from WSS bookTicker"
            ),
        })?;
    if !source_event_id.contains(symbol) {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS monitor `{monitor_ref}` source event `{source_event_id}` does not include `{symbol}`"
            ),
        });
    }
    let observed_at = UtcTimestamp::from_str(&quote.observed_at)?;
    let ingested_at = UtcTimestamp::from_str(&quote.ingested_at)?;
    ensure_timestamp_within_max_age(observed_at, fetched_at, monitor_ref, "observed_at")?;
    ensure_timestamp_within_max_age(ingested_at, fetched_at, monitor_ref, "ingested_at")?;
    let _ = Price::from_str(required_quote_field(
        &quote.best_bid,
        monitor_ref,
        "best_bid",
    )?)?;
    let _ = Price::from_str(required_quote_field(
        &quote.best_ask,
        monitor_ref,
        "best_ask",
    )?)?;
    let _ = Quantity::from_str(required_quote_field(
        &quote.bid_size,
        monitor_ref,
        "bid_size",
    )?)?;
    let _ = Quantity::from_str(required_quote_field(
        &quote.ask_size,
        monitor_ref,
        "ask_size",
    )?)?;
    Ok(())
}

#[cfg(feature = "live-exec")]
fn required_quote_field<'a>(
    value: &'a Option<String>,
    monitor_ref: &str,
    field: &str,
) -> RuntimeResult<&'a str> {
    value
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("Binance WSS monitor `{monitor_ref}` quote lacks `{field}`"),
        })
}

#[cfg(feature = "live-exec")]
fn ensure_timestamp_within_max_age(
    observed: UtcTimestamp,
    fetched_at: UtcTimestamp,
    monitor_ref: &str,
    field: &str,
) -> RuntimeResult<()> {
    let observed_ms = runtime_timestamp_millis(observed)?;
    let fetched_ms = runtime_timestamp_millis(fetched_at)?;
    if observed_ms > fetched_ms + 1_000 {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS monitor `{monitor_ref}` quote `{field}` is in the future"
            ),
        });
    }
    let age_ms = fetched_ms.saturating_sub(observed_ms);
    if age_ms > i128::from(MARKET_DATA_MAX_AGE_MS) {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Binance WSS monitor `{monitor_ref}` quote `{field}` age {age_ms}ms exceeds {MARKET_DATA_MAX_AGE_MS}ms"
            ),
        });
    }
    Ok(())
}

#[cfg(feature = "live-exec")]
fn runtime_timestamp_millis(timestamp: UtcTimestamp) -> RuntimeResult<i128> {
    let seconds = i128::from(timestamp.unix_seconds())
        .checked_mul(1_000)
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: "timestamp seconds overflowed while computing milliseconds".to_owned(),
        })?;
    Ok(seconds + i128::from(timestamp.nanoseconds() / 1_000_000))
}

#[cfg(feature = "live-exec")]
fn binance_wss_monitor_quote_to_book_ticker_json(
    quote: &BinanceWssBookTickerQuoteSnapshot,
) -> RuntimeResult<String> {
    let observed_at = UtcTimestamp::from_str(&quote.observed_at)?;
    Ok(format!(
        "{{\"askPrice\":{},\"askQty\":{},\"bidPrice\":{},\"bidQty\":{},\"symbol\":{},\"time\":{}}}",
        json_string(required_quote_field(
            &quote.best_ask,
            "Binance WSS monitor quote",
            "best_ask"
        )?),
        json_string(required_quote_field(
            &quote.ask_size,
            "Binance WSS monitor quote",
            "ask_size"
        )?),
        json_string(required_quote_field(
            &quote.best_bid,
            "Binance WSS monitor quote",
            "best_bid"
        )?),
        json_string(required_quote_field(
            &quote.bid_size,
            "Binance WSS monitor quote",
            "bid_size"
        )?),
        json_string(&quote.symbol),
        runtime_timestamp_millis(observed_at)?
    ))
}

#[cfg(feature = "live-exec")]
#[allow(clippy::too_many_arguments)]
fn run_binance_basis_guarded_live_auto_once_from_json(
    raw_spot: &str,
    spot_ref: &str,
    raw_perp: &str,
    perp_ref: &str,
    raw_premium: &str,
    premium_ref: &str,
    ingested_at: UtcTimestamp,
    options: BinanceBasisGuardedLiveAutoOnceOptions,
) -> RuntimeResult<BinanceBasisGuardedLiveAutoOnceReport> {
    let output_root = options
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(BINANCE_BASIS_GUARDED_LIVE_AUTO_ONCE_DEFAULT_OUT));
    let market_dir = output_root.join("market");
    let preview_dir = output_root.join("preview");
    let live_dir = output_root.join("live-dispatch");
    let output_dir = Some(output_root.clone());
    let context = write_binance_basis_guarded_live_auto_market_artifacts(
        BinanceBasisRawInputs {
            symbol: BASIS_SYMBOL,
            raw_spot_book: raw_spot,
            spot_book_ref: spot_ref,
            raw_perp_book: raw_perp,
            perp_book_ref: perp_ref,
            raw_premium_index: raw_premium,
            premium_index_ref: premium_ref,
        },
        ingested_at,
        options.min_net_bps,
        &market_dir,
    )?;
    let mut blocking_reasons = Vec::new();
    if !context.signal.is_candidate {
        blocking_reasons.push(
            context
                .signal
                .reason
                .clone()
                .unwrap_or_else(|| "basis signal did not pass threshold".to_owned()),
        );
    }
    if let Some(max_spot_ask) = &options.max_spot_ask {
        if Decimal::from_str(&context.spot_ask)? > Decimal::from_str(max_spot_ask)? {
            blocking_reasons.push(format!(
                "spot leg blocked: spot_ask={} is above max_spot_ask={max_spot_ask}",
                context.spot_ask
            ));
        }
    } else if options.execute_live {
        blocking_reasons.push("live basis execution requires --max-spot-ask".to_owned());
    }
    if let Some(min_perp_bid) = &options.min_perp_bid {
        if Decimal::from_str(&context.perp_bid)? < Decimal::from_str(min_perp_bid)? {
            blocking_reasons.push(format!(
                "perp leg blocked: perp_bid={} is below min_perp_bid={min_perp_bid}",
                context.perp_bid
            ));
        }
    } else if options.execute_live {
        blocking_reasons.push("live basis execution requires --min-perp-bid".to_owned());
    }
    if !blocking_reasons.is_empty() {
        let report = BinanceBasisGuardedLiveAutoOnceReport {
            symbol: BASIS_SYMBOL.to_owned(),
            strategy_id: BINANCE_BASIS_LIVE_STRATEGY_ID.to_owned(),
            spot_event_id: Some(context.spot_event_id),
            perp_event_id: Some(context.perp_event_id),
            premium_event_id: Some(context.premium_event_id),
            spot_ask: Some(context.spot_ask),
            perp_bid: Some(context.perp_bid),
            net_bps: Some(context.signal.net_bps),
            signal_allowed: false,
            plan_hash: None,
            approval_event_id: None,
            manual_gate_released: false,
            dispatch_attempted: false,
            dispatch_allowed: false,
            planned_order_count: 0,
            submitted_receipt_count: 0,
            private_confirmation_count: 0,
            protection_attempted: false,
            protection_actions: Vec::new(),
            protection_receipt_count: 0,
            residual_risk: None,
            execution_report_status: None,
            blocking_reasons,
            output_dir,
        };
        write_binance_basis_guarded_live_auto_once_artifacts(
            &output_root,
            &report,
            &[],
            &[],
            None,
        )?;
        return Ok(report);
    }

    let generated_at = ingested_at.to_string();
    let candidate = binance_basis_guarded_live_candidate(
        &context,
        &generated_at,
        &ingested_at.unix_seconds().to_string(),
    )?;
    let risk_decision = binance_basis_guarded_live_manual_risk_decision(
        &context,
        &generated_at,
        options.min_net_bps,
    )?;
    let pending = match build_execution_plan_preview(ExecutionPlanBuildInput::new(
        &risk_decision,
        &candidate,
        ContractExecutionMode::GuardedLive,
        &generated_at,
    ))? {
        PlanBuildOutcome::PendingManualApproval(pending) => pending,
        PlanBuildOutcome::Schedulable(_) => {
            return Err(RuntimeError::UnsafeConfig {
                message: "basis guarded live auto unexpectedly produced dispatchable plan before approval"
                    .to_owned(),
            });
        }
    };
    fs::create_dir_all(&preview_dir).map_err(|error| RuntimeError::Io {
        path: preview_dir.clone(),
        message: error.to_string(),
    })?;
    write_binance_guarded_live_preview_artifacts(
        &preview_dir,
        BinanceGuardedLivePreviewArtifacts {
            candidate: &candidate,
            risk_decision: &risk_decision,
            plan_preview: &pending.plan_preview,
            plan_hash: &execution_plan_hash(&pending.plan_preview),
            manual_material_md: "",
            approval_records_jsonl: "",
            confirmation_template_md: "",
        },
    )?;
    let plan_hash = execution_plan_hash(&pending.plan_preview);
    let approved_record = review_manual_approval(
        ManualApprovalReviewInput::new(
            &pending,
            &format!(
                "event:approval:auto-live:binance-basis:{}",
                ingested_at.unix_seconds()
            ),
            "system:basis-guarded-live-auto-strategy",
            &generated_at,
            &UtcTimestamp::from_unix_parts(ingested_at.unix_seconds() + 300, 0)?.to_string(),
            ManualApprovalDecision::Approve,
        )
        .with_reason("Automatic basis strategy approved both spot and perp legs for the same fresh plan hash."),
    )?;
    write_utf8(
        preview_dir.join("approval_records.jsonl"),
        &manual_approval_records_jsonl(std::slice::from_ref(&approved_record)),
    )?;
    let release = release_manual_approval_gate(&pending, &approved_record)?;
    write_binance_manual_gate_release_artifacts(&preview_dir, &release, false, &generated_at)?;

    let config = arb_config::ArbConfig::from_path(&options.config_path)?;
    let service = start_runtime_with_config(&config)?;
    let health = service.health();
    let mut dispatch_blocking = live_dispatch_blocking_reasons(
        &config,
        &pending.plan_preview,
        &release,
        &plan_hash,
        &health,
    );
    let dispatch_plan = if dispatch_blocking.is_empty() {
        let dispatch_policy = live_dispatch_policy_from_config(&config)?;
        let dispatch_plan =
            build_execution_dispatch_plan(&pending.plan_preview, &dispatch_policy, ingested_at)?;
        if dispatch_plan.requests.len() != 2 {
            dispatch_blocking.push(format!(
                "basis arbitrage requires exactly two order legs, got {}",
                dispatch_plan.requests.len()
            ));
        }
        Some(dispatch_plan)
    } else {
        None
    };

    let mut receipts = Vec::new();
    let mut confirmations = Vec::new();
    let mut execution_report = None;
    let mut dispatch_attempted = false;
    let mut primary_submit_receipt_count = 0usize;
    let mut protection = BinanceBasisLiveProtection::default();
    if options.execute_live && dispatch_blocking.is_empty() {
        if !options.acknowledge_basis_live_orders {
            return Err(RuntimeError::UnsafeConfig {
                message: "缺少 --i-understand-basis-live-orders，拒绝进入双腿真实套利下单链路"
                    .to_owned(),
            });
        }
        let dispatch_plan = dispatch_plan
            .as_ref()
            .ok_or_else(|| RuntimeError::UnsafeConfig {
                message: "双腿套利分发计划缺失，拒绝进入真实下单链路".to_owned(),
            })?;
        let signing_policy = signing_policy_from_config(&config)?;
        let spot_account = fetch_binance_private_account_snapshot(
            &signing_policy,
            &ingested_at,
            "basis-pre-spot",
            BinancePrivateAccountMarket::Spot,
            BASIS_SPOT_VENUE_ID,
            "https://api.binance.com",
            "/api/v3/account",
        )?;
        ensure_asset_balance_covers_amount(
            "Binance Spot",
            &spot_account.balances,
            BASIS_QUOTE_ASSET_ID,
            BINANCE_GUARDED_LIVE_NOTIONAL_USDT,
        )?;
        let usdm_account = fetch_binance_private_account_snapshot(
            &signing_policy,
            &ingested_at,
            "basis-pre-usdm",
            BinancePrivateAccountMarket::UsdmFutures,
            BASIS_PERP_VENUE_ID,
            BINANCE_USDM_FUTURES_BASE_URL,
            "/fapi/v3/account",
        )?;
        ensure_asset_balance_covers_amount(
            "Binance USD-M",
            &usdm_account.balances,
            BASIS_QUOTE_ASSET_ID,
            BINANCE_GUARDED_LIVE_NOTIONAL_USDT,
        )?;
        let private_order_events = options
            .private_order_events_dir
            .as_ref()
            .map(|path| BinancePrivateOrderEventStore::from_dir(path))
            .transpose()?;
        dispatch_attempted = true;
        let mut spot_adapter = build_binance_spot_live_adapter(&signing_policy)?;
        let mut usdm_adapter = build_binance_usdm_live_adapter(&signing_policy)?;
        let protection_suffix = ingested_at.unix_seconds().to_string();
        let outcome = execute_binance_basis_live_dispatch(
            dispatch_plan,
            &mut spot_adapter,
            &mut usdm_adapter,
            BinanceBasisLiveDispatchContext {
                plan: Some(&pending.plan_preview),
                generated_at: &generated_at,
                spot_unwind_limit_price: Price::from_str(&context.spot_bid)?,
                protection_suffix: &protection_suffix,
                private_order_events: private_order_events.as_ref(),
            },
        )?;
        receipts = outcome.receipts;
        confirmations = outcome.confirmations;
        execution_report = outcome.execution_report;
        primary_submit_receipt_count = outcome.primary_submit_receipt_count;
        protection = outcome.protection;
        dispatch_blocking.extend(outcome.blocking_reasons);
        if execution_report.is_none() && dispatch_blocking.is_empty() {
            execution_report = Some(arb_execution::execution_report_from_private_confirmations(
                arb_execution::PrivateExecutionReportInput::new(
                    &pending.plan_preview,
                    &generated_at,
                    &confirmations,
                ),
            )?);
        }
    } else if !options.execute_live {
        dispatch_blocking.push(
            "basis chain ran in dry-run mode; pass --execute-live and --i-understand-basis-live-orders to submit both real legs"
                .to_owned(),
        );
    }

    if execution_report.is_none() && dispatch_blocking.is_empty() && options.execute_live {
        execution_report = Some(arb_execution::execution_report_from_private_confirmations(
            arb_execution::PrivateExecutionReportInput::new(
                &pending.plan_preview,
                &generated_at,
                &confirmations,
            ),
        )?);
    }

    let report = BinanceBasisGuardedLiveAutoOnceReport {
        symbol: BASIS_SYMBOL.to_owned(),
        strategy_id: BINANCE_BASIS_LIVE_STRATEGY_ID.to_owned(),
        spot_event_id: Some(context.spot_event_id),
        perp_event_id: Some(context.perp_event_id),
        premium_event_id: Some(context.premium_event_id),
        spot_ask: Some(context.spot_ask),
        perp_bid: Some(context.perp_bid),
        net_bps: Some(context.signal.net_bps),
        signal_allowed: true,
        plan_hash: Some(plan_hash),
        approval_event_id: Some(release.approval_event_id),
        manual_gate_released: true,
        dispatch_attempted,
        dispatch_allowed: dispatch_blocking.is_empty(),
        planned_order_count: dispatch_plan
            .as_ref()
            .map(|plan| plan.requests.len())
            .unwrap_or(0),
        submitted_receipt_count: primary_submit_receipt_count,
        private_confirmation_count: confirmations.len(),
        protection_attempted: protection.attempted,
        protection_actions: protection.actions,
        protection_receipt_count: protection.receipt_count,
        residual_risk: protection.residual_risk,
        execution_report_status: execution_report
            .as_ref()
            .map(|report| report.status.as_str().to_owned()),
        blocking_reasons: dispatch_blocking,
        output_dir,
    };
    write_binance_basis_guarded_live_auto_once_artifacts(
        &live_dir,
        &report,
        &receipts,
        &confirmations,
        execution_report.as_ref(),
    )?;
    write_binance_basis_guarded_live_auto_once_artifacts(&output_root, &report, &[], &[], None)?;
    Ok(report)
}

#[cfg(feature = "live-exec")]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct BinanceBasisLiveProtection {
    attempted: bool,
    actions: Vec<String>,
    receipt_count: usize,
    residual_risk: Option<String>,
}

#[cfg(feature = "live-exec")]
impl BinanceBasisLiveProtection {
    fn record_action(&mut self, action: impl Into<String>) {
        self.attempted = true;
        self.actions.push(action.into());
    }

    fn record_receipt(&mut self) {
        self.receipt_count += 1;
    }

    fn record_residual_risk(&mut self, detail: impl Into<String>) {
        self.residual_risk = Some(detail.into());
    }
}

#[cfg(feature = "live-exec")]
#[derive(Clone, Debug, Default)]
struct BinanceBasisLiveExecutionOutcome {
    receipts: Vec<MutableActionReceipt>,
    confirmations: Vec<arb_execution::PrivateOrderConfirmation>,
    execution_report: Option<ExecutionReport>,
    primary_submit_receipt_count: usize,
    protection: BinanceBasisLiveProtection,
    blocking_reasons: Vec<String>,
}

#[cfg(feature = "live-exec")]
#[derive(Clone, Copy)]
struct BinanceBasisLiveDispatchContext<'a> {
    plan: Option<&'a ExecutionPlan>,
    generated_at: &'a str,
    spot_unwind_limit_price: Price,
    protection_suffix: &'a str,
    private_order_events: Option<&'a BinancePrivateOrderEventStore>,
}

#[cfg(feature = "live-exec")]
impl<'a> BinanceBasisLiveDispatchContext<'a> {
    fn order_context(
        self,
        market: BinancePrivateOrderMarket,
    ) -> BinanceBasisOrderConfirmContext<'a> {
        BinanceBasisOrderConfirmContext {
            protection_suffix: self.protection_suffix,
            private_order_events: self.private_order_events,
            market,
        }
    }
}

#[cfg(feature = "live-exec")]
#[derive(Clone, Copy)]
struct BinanceBasisOrderConfirmContext<'a> {
    protection_suffix: &'a str,
    private_order_events: Option<&'a BinancePrivateOrderEventStore>,
    market: BinancePrivateOrderMarket,
}

#[cfg(feature = "live-exec")]
fn execute_binance_basis_live_dispatch<S, U>(
    dispatch_plan: &ExecutionDispatchPlan,
    spot_adapter: &mut S,
    usdm_adapter: &mut U,
    context: BinanceBasisLiveDispatchContext<'_>,
) -> RuntimeResult<BinanceBasisLiveExecutionOutcome>
where
    S: SubmitOrder + CancelOrder + ConfirmOrderStatus,
    U: SubmitOrder + CancelOrder + ConfirmOrderStatus,
{
    let (spot, perp) = binance_basis_planned_legs(dispatch_plan)?;
    let spot_context = context.order_context(BinancePrivateOrderMarket::Spot);
    let perp_context = context.order_context(BinancePrivateOrderMarket::UsdmFutures);
    let mut outcome = BinanceBasisLiveExecutionOutcome::default();

    let spot_receipt = match spot_adapter.submit_order(spot.request.clone()) {
        Ok(receipt) => {
            if receipt.kind == MutableActionKind::SubmitOrder
                && receipt.status == MutableActionStatus::Accepted
            {
                outcome.primary_submit_receipt_count += 1;
            }
            outcome.receipts.push(receipt.clone());
            receipt
        }
        Err(error) => {
            outcome.blocking_reasons.push(format!(
                "spot leg submit failed before perp leg; no hedge leg was sent: {error}"
            ));
            return Ok(outcome);
        }
    };

    let spot_update = match confirm_planned_receipt_with_private_stream_or_order_query(
        spot_adapter,
        spot,
        &spot_receipt,
        "spot-after-submit",
        spot_context.private_order_events,
        spot_context.market,
    ) {
        Ok(update) => {
            outcome
                .confirmations
                .push(private_confirmation_from_update(&spot.plan_leg_id, &update));
            update
        }
        Err(error) => {
            outcome.blocking_reasons.push(format!(
                "spot leg was accepted but private confirmation failed; perp leg skipped: {error}"
            ));
            let _ = attempt_cancel_live_order(
                spot_adapter,
                spot,
                &spot_receipt,
                "spot-confirmation-failed",
                spot_context,
                &mut outcome,
            )?;
            outcome.protection.record_residual_risk(
                "spot order state is unknown after accepted REST receipt; manual reconciliation is required",
            );
            return Ok(outcome);
        }
    };

    match spot_update.status {
        OrderConfirmationStatus::Filled => {}
        OrderConfirmationStatus::PartiallyFilled => {
            let filled = spot_update.cumulative_filled_quantity;
            outcome.blocking_reasons.push(
                "spot leg partially filled before hedge; perp leg skipped and protection path engaged"
                    .to_owned(),
            );
            let _ = attempt_cancel_live_order(
                spot_adapter,
                spot,
                &spot_receipt,
                "spot-partial-fill",
                spot_context,
                &mut outcome,
            )?;
            if let Some(quantity) = filled.filter(|value| quantity_is_positive(*value)) {
                attempt_spot_unwind(
                    spot_adapter,
                    spot,
                    quantity,
                    context.spot_unwind_limit_price,
                    "spot-partial-fill",
                    spot_context,
                    &mut outcome,
                )?;
            }
            return Ok(outcome);
        }
        OrderConfirmationStatus::Acknowledged => {
            outcome.blocking_reasons.push(
                "spot leg was only acknowledged and not filled; perp leg skipped and spot cancel requested"
                    .to_owned(),
            );
            let _ = attempt_cancel_live_order(
                spot_adapter,
                spot,
                &spot_receipt,
                "spot-not-filled",
                spot_context,
                &mut outcome,
            )?;
            return Ok(outcome);
        }
        OrderConfirmationStatus::Cancelled
        | OrderConfirmationStatus::Rejected
        | OrderConfirmationStatus::Expired => {
            outcome.blocking_reasons.push(format!(
                "spot leg reached terminal non-filled status `{}`; perp leg skipped",
                spot_update.status.as_str()
            ));
            return Ok(outcome);
        }
        OrderConfirmationStatus::Unknown => {
            outcome.blocking_reasons.push(
                "spot leg status is unknown after accepted REST receipt; perp leg skipped"
                    .to_owned(),
            );
            let _ = attempt_cancel_live_order(
                spot_adapter,
                spot,
                &spot_receipt,
                "spot-unknown",
                spot_context,
                &mut outcome,
            )?;
            outcome.protection.record_residual_risk(
                "spot order state is unknown; do not assume either leg is flat until private reconciliation completes",
            );
            return Ok(outcome);
        }
    }

    let spot_filled_quantity = spot_update
        .cumulative_filled_quantity
        .unwrap_or(spot.request.quantity);
    let perp_submit_result = usdm_adapter.submit_order(perp.request.clone());
    let perp_receipt = match perp_submit_result {
        Ok(receipt) => {
            if receipt.kind == MutableActionKind::SubmitOrder
                && receipt.status == MutableActionStatus::Accepted
            {
                outcome.primary_submit_receipt_count += 1;
            }
            outcome.receipts.push(receipt.clone());
            receipt
        }
        Err(error) => {
            outcome
                .blocking_reasons
                .push(format!("perp hedge submit failed after spot fill: {error}"));
            if binance_error_is_unknown_external_state(&error) {
                outcome.protection.record_residual_risk(
                    "perp submit result is unknown after spot fill; automatic unwind is suppressed to avoid flipping exposure",
                );
            } else {
                attempt_spot_unwind(
                    spot_adapter,
                    spot,
                    spot_filled_quantity,
                    context.spot_unwind_limit_price,
                    "perp-submit-failed",
                    spot_context,
                    &mut outcome,
                )?;
            }
            return Ok(outcome);
        }
    };

    let perp_update = match confirm_planned_receipt_with_private_stream_or_order_query(
        usdm_adapter,
        perp,
        &perp_receipt,
        "perp-after-submit",
        perp_context.private_order_events,
        perp_context.market,
    ) {
        Ok(update) => {
            outcome
                .confirmations
                .push(private_confirmation_from_update(&perp.plan_leg_id, &update));
            update
        }
        Err(error) => {
            outcome.blocking_reasons.push(format!(
                "perp hedge was accepted but private confirmation failed: {error}"
            ));
            outcome.protection.record_residual_risk(
                "perp hedge state is unknown after accepted REST receipt; manual reconciliation is required before unwind",
            );
            return Ok(outcome);
        }
    };

    match perp_update.status {
        OrderConfirmationStatus::Filled => {
            if let Some(plan) = context.plan {
                outcome.execution_report =
                    Some(arb_execution::execution_report_from_private_confirmations(
                        arb_execution::PrivateExecutionReportInput::new(
                            plan,
                            context.generated_at,
                            &outcome.confirmations,
                        ),
                    )?);
            }
        }
        OrderConfirmationStatus::PartiallyFilled => {
            outcome.blocking_reasons.push(
                "perp hedge partially filled after spot fill; cancelling hedge remainder and unwinding unmatched spot quantity"
                    .to_owned(),
            );
            let cancel_update = attempt_cancel_live_order(
                usdm_adapter,
                perp,
                &perp_receipt,
                "perp-partial-fill",
                perp_context,
                &mut outcome,
            )?;
            if cancel_update.is_some() {
                let perp_filled = cancel_update
                    .and_then(|update| update.cumulative_filled_quantity)
                    .or(perp_update.cumulative_filled_quantity);
                if let Some(unmatched) = unmatched_spot_quantity(spot_filled_quantity, perp_filled)
                    .filter(|quantity| quantity_is_positive(*quantity))
                {
                    attempt_spot_unwind(
                        spot_adapter,
                        spot,
                        unmatched,
                        context.spot_unwind_limit_price,
                        "perp-partial-fill",
                        spot_context,
                        &mut outcome,
                    )?;
                }
            } else {
                outcome.protection.record_residual_risk(
                    "perp partial hedge remainder was not confirmed cancelled; automatic spot unwind is suppressed to avoid flipping exposure",
                );
            }
        }
        OrderConfirmationStatus::Acknowledged => {
            outcome.blocking_reasons.push(
                "perp hedge was only acknowledged and not filled; cancelling hedge and unwinding spot"
                    .to_owned(),
            );
            let cancel_update = attempt_cancel_live_order(
                usdm_adapter,
                perp,
                &perp_receipt,
                "perp-not-filled",
                perp_context,
                &mut outcome,
            )?;
            match cancel_update.as_ref().map(|update| update.status) {
                Some(OrderConfirmationStatus::Filled) => {}
                Some(OrderConfirmationStatus::PartiallyFilled) => {
                    if let Some(unmatched) = unmatched_spot_quantity(
                        spot_filled_quantity,
                        cancel_update.and_then(|update| update.cumulative_filled_quantity),
                    )
                    .filter(|quantity| quantity_is_positive(*quantity))
                    {
                        attempt_spot_unwind(
                            spot_adapter,
                            spot,
                            unmatched,
                            context.spot_unwind_limit_price,
                            "perp-not-filled",
                            spot_context,
                            &mut outcome,
                        )?;
                    }
                }
                Some(
                    OrderConfirmationStatus::Cancelled
                    | OrderConfirmationStatus::Rejected
                    | OrderConfirmationStatus::Expired,
                ) => {
                    attempt_spot_unwind(
                        spot_adapter,
                        spot,
                        spot_filled_quantity,
                        context.spot_unwind_limit_price,
                        "perp-not-filled",
                        spot_context,
                        &mut outcome,
                    )?;
                }
                Some(OrderConfirmationStatus::Acknowledged | OrderConfirmationStatus::Unknown)
                | None => {
                    outcome.protection.record_residual_risk(
                        "perp hedge was not confirmed terminal after cancel attempt; automatic spot unwind is suppressed",
                    );
                }
            }
        }
        OrderConfirmationStatus::Cancelled
        | OrderConfirmationStatus::Rejected
        | OrderConfirmationStatus::Expired => {
            outcome.blocking_reasons.push(format!(
                "perp hedge reached terminal non-filled status `{}` after spot fill; unwinding spot",
                perp_update.status.as_str()
            ));
            attempt_spot_unwind(
                spot_adapter,
                spot,
                spot_filled_quantity,
                context.spot_unwind_limit_price,
                "perp-terminal-unfilled",
                spot_context,
                &mut outcome,
            )?;
        }
        OrderConfirmationStatus::Unknown => {
            outcome.blocking_reasons.push(
                "perp hedge status is unknown after spot fill; no automatic unwind submitted"
                    .to_owned(),
            );
            outcome.protection.record_residual_risk(
                "perp hedge state is unknown; automatic spot unwind is suppressed to avoid flipping exposure",
            );
        }
    }

    Ok(outcome)
}

#[cfg(feature = "live-exec")]
fn binance_basis_planned_legs(
    dispatch_plan: &ExecutionDispatchPlan,
) -> RuntimeResult<(&PlannedSubmitOrder, &PlannedSubmitOrder)> {
    let spot = dispatch_plan
        .requests
        .iter()
        .find(|planned| {
            planned.basis_leg_role.as_deref() == Some("spot_buy")
                || planned.request.venue_id.as_str() == BASIS_SPOT_VENUE_ID
        })
        .ok_or_else(|| RuntimeError::UnsafeConfig {
            message: "双腿套利分发计划缺少 spot buy 腿".to_owned(),
        })?;
    let perp = dispatch_plan
        .requests
        .iter()
        .find(|planned| {
            planned.basis_leg_role.as_deref() == Some("perp_short")
                || planned.request.venue_id.as_str() == BASIS_PERP_VENUE_ID
        })
        .ok_or_else(|| RuntimeError::UnsafeConfig {
            message: "双腿套利分发计划缺少 USD-M perp short 腿".to_owned(),
        })?;
    Ok((spot, perp))
}

#[cfg(feature = "live-exec")]
fn confirm_planned_receipt_with_private_stream_or_order_query<A>(
    adapter: &mut A,
    planned: &PlannedSubmitOrder,
    receipt: &MutableActionReceipt,
    scope: &str,
    private_order_events: Option<&BinancePrivateOrderEventStore>,
    market: BinancePrivateOrderMarket,
) -> RuntimeResult<PrivateOrderUpdate>
where
    A: ConfirmOrderStatus,
{
    if receipt.kind != MutableActionKind::SubmitOrder {
        return Err(RuntimeError::UnsafeConfig {
            message: "只允许确认 SubmitOrder 回执".to_owned(),
        });
    }
    let order_ref = order_ref_from_receipt_or_client(planned, receipt)?;
    confirm_planned_order_ref_with_private_stream_or_order_query(
        adapter,
        planned,
        order_ref,
        scope,
        private_order_events,
        market,
    )
}

#[cfg(feature = "live-exec")]
fn confirm_planned_order_ref_with_private_stream_or_order_query<A>(
    adapter: &mut A,
    planned: &PlannedSubmitOrder,
    order_ref: OrderReference,
    scope: &str,
    private_order_events: Option<&BinancePrivateOrderEventStore>,
    market: BinancePrivateOrderMarket,
) -> RuntimeResult<PrivateOrderUpdate>
where
    A: ConfirmOrderStatus,
{
    if let Some(store) = private_order_events {
        if let Some(update) = store.latest_update_for_planned(planned, market)? {
            return Ok(update);
        }
    }
    confirm_planned_order_ref_with_order_query(adapter, planned, order_ref, scope)
}

#[cfg(feature = "live-exec")]
fn confirm_planned_order_ref_with_order_query<A>(
    adapter: &mut A,
    planned: &PlannedSubmitOrder,
    order_ref: OrderReference,
    scope: &str,
) -> RuntimeResult<PrivateOrderUpdate>
where
    A: ConfirmOrderStatus,
{
    adapter
        .confirm_order_status(ConfirmOrderStatusRequest::new(
            planned.request.venue_id.clone(),
            planned.request.account_id.clone(),
            planned.request.instrument_id.clone(),
            order_ref,
            format!(
                "event:binance:basis-order-query:{}:{}",
                scope, planned.plan_leg_id
            ),
        ))
        .map_err(RuntimeError::from)
}

#[cfg(feature = "live-exec")]
#[derive(Clone, Debug, Default)]
struct BinancePrivateOrderEventStore {
    spot_events: Vec<String>,
    usdm_events: Vec<String>,
}

#[cfg(feature = "live-exec")]
impl BinancePrivateOrderEventStore {
    fn from_dir(path: &Path) -> RuntimeResult<Self> {
        Ok(Self {
            spot_events: read_optional_jsonl_lines(&path.join("spot_user_data.jsonl"))?,
            usdm_events: read_optional_jsonl_lines(&path.join("usdm_user_data.jsonl"))?,
        })
    }

    fn latest_update_for_planned(
        &self,
        planned: &PlannedSubmitOrder,
        market: BinancePrivateOrderMarket,
    ) -> RuntimeResult<Option<PrivateOrderUpdate>> {
        let events = match market {
            BinancePrivateOrderMarket::Spot => &self.spot_events,
            BinancePrivateOrderMarket::UsdmFutures => &self.usdm_events,
        };
        let Some(expected_client_order_id) = planned.request.client_order_id.as_ref() else {
            return Ok(None);
        };
        for (index, line) in events.iter().enumerate().rev() {
            let event_type = json_string_field(line, "e")?;
            if !binance_private_order_event_type_matches(market, &event_type) {
                continue;
            }
            let update = parse_binance_private_order_event_line(
                line,
                market,
                planned,
                format!(
                    "event:binance:user-data-stream:{}:{}",
                    binance_private_order_market_token(market),
                    index
                ),
            )?;
            if update.instrument_id != planned.request.instrument_id
                || update.venue_id != planned.request.venue_id
                || update.account_id != planned.request.account_id
            {
                continue;
            }
            if update
                .client_order_id
                .as_ref()
                .is_some_and(|client_order_id| client_order_id == expected_client_order_id)
            {
                return Ok(Some(update));
            }
        }
        Ok(None)
    }
}

#[cfg(feature = "live-exec")]
fn read_optional_jsonl_lines(path: &Path) -> RuntimeResult<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let input = read_utf8(path)?;
    Ok(input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

#[cfg(feature = "live-exec")]
fn binance_private_order_event_type_matches(
    market: BinancePrivateOrderMarket,
    event_type: &str,
) -> bool {
    match market {
        BinancePrivateOrderMarket::Spot => event_type == "executionReport",
        BinancePrivateOrderMarket::UsdmFutures => event_type == "ORDER_TRADE_UPDATE",
    }
}

#[cfg(feature = "live-exec")]
fn parse_binance_private_order_event_line(
    line: &str,
    market: BinancePrivateOrderMarket,
    planned: &PlannedSubmitOrder,
    source_event_id: String,
) -> RuntimeResult<PrivateOrderUpdate> {
    match market {
        BinancePrivateOrderMarket::Spot => parse_binance_spot_execution_report_update(
            planned.request.venue_id.clone(),
            planned.request.account_id.clone(),
            source_event_id,
            line,
        ),
        BinancePrivateOrderMarket::UsdmFutures => parse_binance_usdm_order_trade_update(
            planned.request.venue_id.clone(),
            planned.request.account_id.clone(),
            source_event_id,
            line,
        ),
    }
    .map_err(RuntimeError::from)
}

#[cfg(feature = "live-exec")]
fn binance_private_order_market_token(market: BinancePrivateOrderMarket) -> &'static str {
    match market {
        BinancePrivateOrderMarket::Spot => "spot",
        BinancePrivateOrderMarket::UsdmFutures => "usdm",
    }
}

#[cfg(feature = "live-exec")]
fn order_ref_from_receipt_or_client(
    planned: &PlannedSubmitOrder,
    receipt: &MutableActionReceipt,
) -> RuntimeResult<OrderReference> {
    match &receipt.external_ref {
        Some(ExternalActionRef::Order(order_id)) => {
            Ok(OrderReference::VenueOrderId(order_id.clone()))
        }
        _ => planned
            .request
            .client_order_id
            .clone()
            .map(OrderReference::ClientOrderId)
            .ok_or_else(|| RuntimeError::UnsafeConfig {
                message: "下单回执缺少外部订单号和 client_order_id，无法执行私有查单确认"
                    .to_owned(),
            }),
    }
}

#[cfg(feature = "live-exec")]
fn attempt_cancel_live_order<A>(
    adapter: &mut A,
    planned: &PlannedSubmitOrder,
    receipt: &MutableActionReceipt,
    reason: &str,
    context: BinanceBasisOrderConfirmContext<'_>,
    outcome: &mut BinanceBasisLiveExecutionOutcome,
) -> RuntimeResult<Option<PrivateOrderUpdate>>
where
    A: CancelOrder + ConfirmOrderStatus,
{
    let order_ref = order_ref_from_receipt_or_client(planned, receipt)?;
    outcome.protection.record_action(format!(
        "cancel_open_order:{reason}:{}",
        planned.plan_leg_id
    ));
    let cancel_receipt = match adapter.cancel_order(CancelOrderRequest::new(
        planned.request.venue_id.clone(),
        planned.request.account_id.clone(),
        order_ref.clone(),
        ExecIdempotencyKey::new(format!(
            "{}:cancel:{reason}:{}",
            planned.request.idempotency_key.as_str(),
            context.protection_suffix
        ))?,
    )) {
        Ok(receipt) => {
            outcome.protection.record_receipt();
            outcome.receipts.push(receipt.clone());
            receipt
        }
        Err(error) => {
            outcome.blocking_reasons.push(format!(
                "protection cancel failed for `{}`: {error}",
                planned.plan_leg_id
            ));
            outcome.protection.record_residual_risk(format!(
                "cancel protection failed for `{}`; manual reconciliation is required",
                planned.plan_leg_id
            ));
            return Ok(None);
        }
    };
    if cancel_receipt.status != MutableActionStatus::Accepted {
        outcome.blocking_reasons.push(format!(
            "protection cancel for `{}` returned `{}`",
            planned.plan_leg_id, cancel_receipt.status
        ));
        outcome.protection.record_residual_risk(format!(
            "cancel protection for `{}` was not accepted",
            planned.plan_leg_id
        ));
        return Ok(None);
    }
    match confirm_planned_order_ref_with_private_stream_or_order_query(
        adapter,
        planned,
        order_ref,
        &format!("cancel:{reason}"),
        context.private_order_events,
        context.market,
    ) {
        Ok(update) => {
            outcome.confirmations.push(private_confirmation_from_update(
                &planned.plan_leg_id,
                &update,
            ));
            if !matches!(
                update.status,
                OrderConfirmationStatus::Cancelled
                    | OrderConfirmationStatus::Rejected
                    | OrderConfirmationStatus::Expired
                    | OrderConfirmationStatus::Filled
                    | OrderConfirmationStatus::PartiallyFilled
            ) {
                outcome.protection.record_residual_risk(format!(
                    "cancel protection for `{}` confirmed status `{}`",
                    planned.plan_leg_id,
                    update.status.as_str()
                ));
            }
            Ok(Some(update))
        }
        Err(error) => {
            outcome.blocking_reasons.push(format!(
                "protection cancel confirmation failed for `{}`: {error}",
                planned.plan_leg_id
            ));
            outcome.protection.record_residual_risk(format!(
                "cancel protection for `{}` has no private confirmation",
                planned.plan_leg_id
            ));
            Ok(None)
        }
    }
}

#[cfg(feature = "live-exec")]
fn attempt_spot_unwind<S>(
    spot_adapter: &mut S,
    spot: &PlannedSubmitOrder,
    quantity: Quantity,
    limit_price: Price,
    reason: &str,
    context: BinanceBasisOrderConfirmContext<'_>,
    outcome: &mut BinanceBasisLiveExecutionOutcome,
) -> RuntimeResult<()>
where
    S: SubmitOrder + ConfirmOrderStatus,
{
    outcome
        .protection
        .record_action(format!("spot_unwind_sell:{reason}:{}", spot.plan_leg_id));
    let unwind_request = SubmitOrderRequest::new(
        spot.request.venue_id.clone(),
        spot.request.account_id.clone(),
        spot.request.instrument_id.clone(),
        OrderSide::Sell,
        MutableOrderType::Limit,
        quantity,
        Some(limit_price),
        Some(OrderId::new(format!("rvbU{}", context.protection_suffix))?),
        ExecIdempotencyKey::new(format!(
            "{}:unwind:{reason}:{}",
            spot.request.idempotency_key.as_str(),
            context.protection_suffix
        ))?,
    );
    let unwind_plan = PlannedSubmitOrder {
        plan_leg_id: format!("{}:protection-unwind", spot.plan_leg_id),
        venue_symbol: spot.venue_symbol.clone(),
        basis_leg_role: Some("spot_unwind".to_owned()),
        notional_usd: spot.notional_usd,
        request: unwind_request,
    };
    let receipt = match spot_adapter.submit_order(unwind_plan.request.clone()) {
        Ok(receipt) => {
            outcome.protection.record_receipt();
            outcome.receipts.push(receipt.clone());
            receipt
        }
        Err(error) => {
            outcome.blocking_reasons.push(format!(
                "spot unwind protection submit failed after `{reason}`: {error}"
            ));
            outcome.protection.record_residual_risk(format!(
                "spot unwind protection failed after `{reason}`; manual hedge or unwind is required"
            ));
            return Ok(());
        }
    };
    match confirm_planned_receipt_with_private_stream_or_order_query(
        spot_adapter,
        &unwind_plan,
        &receipt,
        &format!("spot-unwind:{reason}"),
        context.private_order_events,
        BinancePrivateOrderMarket::Spot,
    ) {
        Ok(update) => {
            if update.status != OrderConfirmationStatus::Filled {
                outcome.protection.record_residual_risk(format!(
                    "spot unwind protection returned `{}` instead of filled",
                    update.status.as_str()
                ));
            }
            outcome.confirmations.push(private_confirmation_from_update(
                &unwind_plan.plan_leg_id,
                &update,
            ));
        }
        Err(error) => {
            outcome.blocking_reasons.push(format!(
                "spot unwind protection confirmation failed after `{reason}`: {error}"
            ));
            outcome.protection.record_residual_risk(
                "spot unwind protection has no private confirmation; manual reconciliation is required",
            );
        }
    }
    Ok(())
}

#[cfg(feature = "live-exec")]
fn quantity_is_positive(value: Quantity) -> bool {
    !value.as_decimal().is_zero()
}

#[cfg(feature = "live-exec")]
fn unmatched_spot_quantity(
    spot_filled: Quantity,
    perp_filled: Option<Quantity>,
) -> Option<Quantity> {
    let perp_filled = perp_filled?;
    if spot_filled <= perp_filled {
        return None;
    }
    let delta = spot_filled
        .as_decimal()
        .checked_sub(perp_filled.as_decimal())
        .ok()?;
    Quantity::new(delta).ok()
}

#[cfg(feature = "live-exec")]
fn binance_error_is_unknown_external_state(error: &VenueExecError) -> bool {
    matches!(error, VenueExecError::UnknownExternalState { .. })
}

#[cfg(feature = "live-exec")]
fn run_binance_guarded_live_dispatch_live(
    options: BinanceGuardedLiveDispatchOptions,
) -> RuntimeResult<BinanceGuardedLiveDispatchReport> {
    if !options.acknowledge_live_orders {
        return Err(RuntimeError::UnsafeConfig {
            message: "缺少 --i-understand-live-orders，拒绝读取凭证或提交真实订单".to_owned(),
        });
    }

    let (pending, approved_record) = load_binance_manual_gate_release_inputs(&options.preview_dir)?;
    let release = release_manual_approval_gate(&pending, &approved_record)?;
    let plan = &pending.plan_preview;
    let plan_hash = execution_plan_hash(plan);
    let config = arb_config::ArbConfig::from_path(&options.config_path)?;
    let service = start_runtime_with_config(&config)?;
    let health = service.health();
    let generated_at = current_utc_timestamp()?;
    let generated_at_text = generated_at.to_string();
    let output_dir = Some(
        options
            .output_dir
            .clone()
            .unwrap_or_else(|| options.preview_dir.clone()),
    );

    let mut blocking_reasons =
        live_dispatch_blocking_reasons(&config, plan, &release, &plan_hash, &health);
    let mut receipts = Vec::new();
    let mut confirmations = Vec::new();
    let mut execution_report = None;
    let mut pre_account_event = None;
    let mut post_account_event = None;

    if blocking_reasons.is_empty() {
        let signing_policy = signing_policy_from_config(&config)?;
        let pre_account = fetch_binance_spot_private_account_snapshot(
            &signing_policy,
            &generated_at,
            "pre-dispatch",
        )?;
        ensure_private_balance_covers_plan(plan, &pre_account.balances)?;
        pre_account_event = Some(pre_account.balance_event_json);

        let mut adapter = build_binance_spot_live_adapter(&signing_policy)?;
        let dispatch_plan = build_execution_dispatch_plan(
            plan,
            &live_dispatch_policy_from_config(&config)?,
            generated_at,
        )?;
        if dispatch_plan.requests.len() != 1 {
            blocking_reasons.push(format!(
                "当前 GuardedLive 实盘入口只允许单腿 BTCUSDT spot 计划，收到 {} 腿",
                dispatch_plan.requests.len()
            ));
        } else {
            let planned = &dispatch_plan.requests[0];
            if planned.request.venue_id.as_str() != BASIS_SPOT_VENUE_ID {
                blocking_reasons.push(format!(
                    "当前 GuardedLive 实盘入口只允许 venue `{}`，收到 `{}`",
                    BASIS_SPOT_VENUE_ID, planned.request.venue_id
                ));
            } else {
                let receipt = adapter.submit_order(planned.request.clone())?;
                receipts.push(receipt.clone());
                let update =
                    confirm_live_receipt_with_order_query(&mut adapter, planned, &receipt)?;
                confirmations.push(private_confirmation_from_update(
                    &planned.plan_leg_id,
                    &update,
                ));
                execution_report =
                    Some(arb_execution::execution_report_from_private_confirmations(
                        arb_execution::PrivateExecutionReportInput::new(
                            plan,
                            &generated_at_text,
                            &confirmations,
                        ),
                    )?);
            }
        }

        let post_account = fetch_binance_spot_private_account_snapshot(
            &signing_policy,
            &generated_at,
            "post-dispatch",
        )?;
        post_account_event = Some(post_account.balance_event_json);
    }

    let report = BinanceGuardedLiveDispatchReport {
        plan_id: plan.plan_id.as_str().to_owned(),
        plan_hash,
        approval_event_id: release.approval_event_id,
        dispatch_allowed: blocking_reasons.is_empty(),
        submitted_receipt_count: receipts.len(),
        private_confirmation_count: confirmations.len(),
        pre_account_balance_event_id: pre_account_event
            .as_ref()
            .and_then(|event| json_string_field(event, "event_id").ok()),
        post_account_balance_event_id: post_account_event
            .as_ref()
            .and_then(|event| json_string_field(event, "event_id").ok()),
        execution_report_status: execution_report
            .as_ref()
            .map(|report| report.status.as_str().to_owned()),
        blocking_reasons,
        output_dir,
    };

    if let Some(dir) = &report.output_dir {
        write_binance_guarded_live_dispatch_artifacts(
            dir,
            &report,
            &receipts,
            &confirmations,
            execution_report.as_ref(),
            pre_account_event.as_deref(),
            post_account_event.as_deref(),
        )?;
    }
    Ok(report)
}

fn bootstrap_binance_wss_book_ticker_client(
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
        MARKET_DATA_MAX_AGE_MS,
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
        MARKET_DATA_MAX_AGE_MS,
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
    let state = Arc::new(RwLock::new(BinanceWssBookTickerMonitorSnapshot::empty(
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
    loop {
        let cycle = run_binance_wss_book_ticker_monitor_cycle(
            &options,
            state.clone(),
            &symbol_scope,
            rebuild_from_rest,
        );
        match cycle {
            Ok(()) if options.once => return Ok(()),
            Ok(()) => {}
            Err(error) if options.once => return Err(error),
            Err(error) => eprintln!("binance-wss-book-ticker cycle failed: {error}"),
        }
        rebuild_from_rest = true;
        thread::sleep(Duration::from_secs(options.reconnect_delay_secs));
    }
}

fn run_binance_wss_book_ticker_monitor_cycle(
    options: &BinanceWssBookTickerMonitorOptions,
    state: Arc<RwLock<BinanceWssBookTickerMonitorSnapshot>>,
    symbol_scope: &str,
    rebuild_from_rest: bool,
) -> RuntimeResult<()> {
    if rebuild_from_rest {
        state
            .write()
            .expect("Binance WSS monitor state lock poisoned")
            .begin_rest_rebuild();
    }
    let mut market_state =
        match bootstrap_binance_wss_book_ticker_all_market(symbol_scope, options.market) {
            Ok(bootstrap) => bootstrap,
            Err(error) => {
                state
                    .write()
                    .expect("Binance WSS monitor state lock poisoned")
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
            .expect("Binance WSS monitor state lock poisoned");
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
            .expect("Binance WSS monitor state lock poisoned")
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
                        .expect("Binance WSS monitor state lock poisoned")
                        .record_update(&update);
                    keep_going
                }
                Ok(None) => true,
                Err(error) => {
                    observer_error = Some(error.to_string());
                    state
                        .write()
                        .expect("Binance WSS monitor state lock poisoned")
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
                    .expect("Binance WSS monitor state lock poisoned")
                    .record_stream_end();
            }
            Ok(())
        }
        Err(error) => {
            state
                .write()
                .expect("Binance WSS monitor state lock poisoned")
                .record_failure(error.to_string(), !observed_wss_event);
            Err(error.into())
        }
    }
}

struct BinanceWssBookTickerAllMarketState {
    venue_id: VenueId,
    stream_url: String,
    all_symbols_scope: bool,
    coordinators: BTreeMap<String, RestWssMarketDataCoordinator>,
    local_sequences: BTreeMap<String, u64>,
    last_exchange_update_ids: BTreeMap<String, u64>,
    rest_updates: Vec<HybridMarketDataUpdate>,
}

struct BinanceWssBookTickerRuntimeRaw {
    symbol: String,
    update_id: u64,
    best_bid: Price,
    best_ask: Price,
    bid_size: Quantity,
    ask_size: Quantity,
    observed_at: UtcTimestamp,
}

fn bootstrap_binance_wss_book_ticker_all_market(
    symbol_scope: &str,
    market: BinancePublicMarket,
) -> RuntimeResult<BinanceWssBookTickerAllMarketState> {
    let venue_id = binance_public_wss_venue_id(market)?;
    let all_symbols_scope = is_binance_wss_all_symbols_scope(symbol_scope);
    let rest_url = if all_symbols_scope {
        match market {
            BinancePublicMarket::Spot => binance_spot_book_ticker_all_url(),
            BinancePublicMarket::UsdmPerpetual => binance_usdm_book_ticker_all_url(),
        }
    } else {
        match market {
            BinancePublicMarket::Spot => binance_spot_book_ticker_url(symbol_scope),
            BinancePublicMarket::UsdmPerpetual => binance_usdm_book_ticker_url(symbol_scope),
        }
    };
    let raw_rest_snapshot = fetch_public_json_with_curl(&rest_url)?;
    let rows = prepare_binance_wss_book_ticker_rest_rows(
        parse_book_ticker_rows(&raw_rest_snapshot, "binance bookTicker")?,
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
            MARKET_DATA_MAX_AGE_MS,
        )?;
        let update = coordinator.apply(HybridMarketDataInput::RestSnapshot { quote })?;
        local_sequences.insert(symbol.clone(), sequence);
        coordinators.insert(symbol.clone(), coordinator);
        rest_updates.push(update);
        symbols.push(symbol);
    }

    let stream_url = binance_wss_book_ticker_all_market_stream_url(market, &symbols)?;
    Ok(BinanceWssBookTickerAllMarketState {
        venue_id,
        stream_url,
        all_symbols_scope,
        coordinators,
        local_sequences,
        last_exchange_update_ids: BTreeMap::new(),
        rest_updates,
    })
}

fn prepare_binance_wss_book_ticker_rest_rows(
    rows: Vec<MonitorBookTickerRow>,
    all_symbols_scope: bool,
) -> RuntimeResult<Vec<MonitorBookTickerRow>> {
    let mut prepared = Vec::with_capacity(rows.len());
    for mut row in rows {
        match validate_binance_public_wss_symbol(&row.symbol) {
            Ok(symbol) => {
                row.symbol = symbol;
                prepared.push(row);
            }
            Err(_) if all_symbols_scope => {}
            Err(error) => return Err(error),
        }
    }
    prepared.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    prepared.dedup_by(|left, right| left.symbol == right.symbol);
    Ok(prepared)
}

fn binance_wss_rest_quote_from_row(
    row: &MonitorBookTickerRow,
    venue_id: &VenueId,
    instrument: &BinancePublicInstrument,
    observed_at: UtcTimestamp,
) -> RuntimeResult<MarketQuote> {
    let freshness = DataFreshness::new(observed_at, observed_at, MARKET_DATA_MAX_AGE_MS)?;
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

fn apply_binance_wss_book_ticker_text(
    raw_json: &str,
    ingested_at: UtcTimestamp,
    market: BinancePublicMarket,
    state: &mut BinanceWssBookTickerAllMarketState,
) -> RuntimeResult<Option<HybridMarketDataUpdate>> {
    let mut raw = parse_binance_wss_book_ticker_runtime_raw(raw_json, ingested_at)?;
    raw.symbol = match validate_binance_public_wss_symbol(&raw.symbol) {
        Ok(symbol) => symbol,
        Err(_) if state.all_symbols_scope => return Ok(None),
        Err(error) => return Err(error),
    };
    if !state.coordinators.contains_key(&raw.symbol) {
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
    let local_sequence = next_binance_wss_local_sequence(state, &raw.symbol)?;
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
                    "binance:wss-bookTicker:{}:{}:{}",
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

fn next_binance_wss_local_sequence(
    state: &mut BinanceWssBookTickerAllMarketState,
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

fn parse_binance_wss_book_ticker_runtime_raw(
    raw_json: &str,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<BinanceWssBookTickerRuntimeRaw> {
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
    Ok(BinanceWssBookTickerRuntimeRaw {
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

fn binance_wss_book_ticker_payload_json(raw_json: &str) -> RuntimeResult<&str> {
    let fields = parse_json_object_value_slices(raw_json)?;
    match fields.get("data") {
        Some(data) => Ok(data.trim()),
        None => Ok(raw_json.trim()),
    }
}

fn optional_binance_wss_millis(
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

fn timestamp_from_unix_millis(value: u64) -> RuntimeResult<UtcTimestamp> {
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

fn binance_wss_book_ticker_all_market_stream_url(
    market: BinancePublicMarket,
    symbols: &[String],
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
            if symbols.len() == 1 {
                Ok(format!(
                    "wss://fstream.binance.com/public/ws/{}@bookTicker",
                    symbols[0].to_ascii_lowercase()
                ))
            } else {
                Ok("wss://fstream.binance.com/public/ws/!bookTicker".to_owned())
            }
        }
    }
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

/// 运行 Bybit basis 7*24 只读监控。
///
/// 中文说明：该路径只访问 Bybit V5 公开市场数据，不使用 API key，不读取私有账户，
/// 不下单、不撤单、不转账、不签名。`once` 只做单次刷新验收。
pub fn run_bybit_basis_monitor(options: BybitBasisMonitorOptions) -> RuntimeResult<()> {
    validate_bybit_monitor_options(&options)?;
    let state = Arc::new(RwLock::new(BybitBasisMonitorSnapshot::empty_bybit(
        &options,
    )));
    if !options.once {
        start_bybit_basis_http_api(&options.bind_addr, state.clone())?;
        println!(
            "bybit-basis-monitor: api=http://{} poll_interval_secs={} min_abs_funding_rate={} mutable_execution_started=false",
            options.bind_addr, options.poll_interval_secs, options.min_abs_funding_rate
        );
    }

    loop {
        match fetch_bybit_basis_monitor_snapshot(&options) {
            Ok(snapshot) => {
                if let Some(dir) = &options.output_dir {
                    write_bybit_basis_monitor_snapshot(dir, &snapshot)?;
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

/// 运行 OKX basis 7*24 只读监控。
///
/// 中文说明：该路径只访问 OKX V5 公开市场数据，不使用 API key，不读取私有账户，
/// 不下单、不撤单、不转账、不签名。`once` 只做单次刷新验收。
pub fn run_okx_basis_monitor(options: OkxBasisMonitorOptions) -> RuntimeResult<()> {
    validate_okx_monitor_options(&options)?;
    let state = Arc::new(RwLock::new(OkxBasisMonitorSnapshot::empty_okx(&options)));
    if !options.once {
        start_okx_basis_http_api(&options.bind_addr, state.clone())?;
        println!(
            "okx-basis-monitor: api=http://{} poll_interval_secs={} min_abs_funding_rate={} mutable_execution_started=false",
            options.bind_addr, options.poll_interval_secs, options.min_abs_funding_rate
        );
    }

    loop {
        match fetch_okx_basis_monitor_snapshot(&options) {
            Ok(snapshot) => {
                if let Some(dir) = &options.output_dir {
                    write_okx_basis_monitor_snapshot(dir, &snapshot)?;
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

/// 运行 Hyperliquid basis 7*24 只读监控。
///
/// 中文说明：该路径只访问 Hyperliquid 官方公开 `info` 数据端点，不使用 API key，
/// 不读取私有账户，不下单、不撤单、不转账、不签名。`once` 只做单次刷新验收。
pub fn run_hyperliquid_basis_monitor(options: HyperliquidBasisMonitorOptions) -> RuntimeResult<()> {
    validate_hyperliquid_monitor_options(&options)?;
    let state = Arc::new(RwLock::new(
        HyperliquidBasisMonitorSnapshot::empty_hyperliquid(&options),
    ));
    if !options.once {
        start_hyperliquid_basis_http_api(&options.bind_addr, state.clone())?;
        println!(
            "hyperliquid-basis-monitor: api=http://{} poll_interval_secs={} min_abs_funding_rate={} mutable_execution_started=false",
            options.bind_addr, options.poll_interval_secs, options.min_abs_funding_rate
        );
    }

    loop {
        match fetch_hyperliquid_basis_monitor_snapshot(&options) {
            Ok(snapshot) => {
                if let Some(dir) = &options.output_dir {
                    write_hyperliquid_basis_monitor_snapshot(dir, &snapshot)?;
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

/// 运行 Aster basis 7*24 只读监控。
///
/// 中文说明：该路径只访问 Aster 公开 spot/perp V3 市场数据，不使用 API key，
/// 不读取私有账户，不下单、不撤单、不转账、不签名。`once` 只做单次刷新验收。
pub fn run_aster_basis_monitor(options: AsterBasisMonitorOptions) -> RuntimeResult<()> {
    validate_aster_monitor_options(&options)?;
    let state = Arc::new(RwLock::new(AsterBasisMonitorSnapshot::empty_aster(
        &options,
    )));
    if !options.once {
        start_aster_basis_http_api(&options.bind_addr, state.clone())?;
        println!(
            "aster-basis-monitor: api=http://{} poll_interval_secs={} min_abs_funding_rate={} mutable_execution_started=false",
            options.bind_addr, options.poll_interval_secs, options.min_abs_funding_rate
        );
    }

    loop {
        match fetch_aster_basis_monitor_snapshot(&options) {
            Ok(snapshot) => {
                if let Some(dir) = &options.output_dir {
                    write_aster_basis_monitor_snapshot(dir, &snapshot)?;
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
    validate_basis_monitor_values(
        options.poll_interval_secs,
        &options.min_abs_funding_rate,
        &options.notional_usd,
    )
}

fn validate_bybit_monitor_options(options: &BybitBasisMonitorOptions) -> RuntimeResult<()> {
    validate_basis_monitor_values(
        options.poll_interval_secs,
        &options.min_abs_funding_rate,
        &options.notional_usd,
    )
}

fn validate_okx_monitor_options(options: &OkxBasisMonitorOptions) -> RuntimeResult<()> {
    validate_basis_monitor_values(
        options.poll_interval_secs,
        &options.min_abs_funding_rate,
        &options.notional_usd,
    )
}

fn validate_hyperliquid_monitor_options(
    options: &HyperliquidBasisMonitorOptions,
) -> RuntimeResult<()> {
    validate_basis_monitor_values(
        options.poll_interval_secs,
        &options.min_abs_funding_rate,
        &options.notional_usd,
    )
}

fn validate_aster_monitor_options(options: &AsterBasisMonitorOptions) -> RuntimeResult<()> {
    validate_basis_monitor_values(
        options.poll_interval_secs,
        &options.min_abs_funding_rate,
        &options.notional_usd,
    )
}

fn validate_basis_monitor_values(
    poll_interval_secs: u64,
    min_abs_funding_rate: &str,
    notional_usd: &str,
) -> RuntimeResult<()> {
    if poll_interval_secs == 0 {
        return Err(cli_arg_error("poll interval must be greater than zero"));
    }
    MonitorDecimal::parse("min_abs_funding_rate", min_abs_funding_rate)?;
    MonitorDecimal::parse("notional_usd", notional_usd)?;
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

fn fetch_aster_basis_monitor_snapshot(
    options: &AsterBasisMonitorOptions,
) -> RuntimeResult<AsterBasisMonitorSnapshot> {
    let spot_json = fetch_public_json_with_curl(&aster_spot_book_ticker_all_url())?;
    let perp_json = fetch_public_json_with_curl(&aster_futures_book_ticker_all_url())?;
    let premium_json = fetch_public_json_with_curl(&aster_futures_premium_index_all_url())?;
    build_aster_basis_monitor_snapshot_from_json(&spot_json, &perp_json, &premium_json, options)
}

fn build_aster_basis_monitor_snapshot_from_json(
    spot_json: &str,
    perp_json: &str,
    premium_json: &str,
    options: &AsterBasisMonitorOptions,
) -> RuntimeResult<AsterBasisMonitorSnapshot> {
    let updated_at = current_utc_timestamp()?.to_string();
    let min_abs_funding_rate =
        MonitorDecimal::parse("min_abs_funding_rate", &options.min_abs_funding_rate)?;
    let spot_books = parse_book_ticker_rows(spot_json, "aster spot")?
        .into_iter()
        .map(|row| (row.symbol.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let perp_books = parse_book_ticker_rows(perp_json, "aster perp")?
        .into_iter()
        .map(|row| (row.symbol.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let premiums = parse_premium_index_rows(premium_json)?;

    let mut rows = Vec::new();
    let mut filtered_funding_count = 0_usize;
    let mut missing_spot_count = 0_usize;
    let mut missing_perp_count = 0_usize;

    for premium in premiums {
        if !premium.symbol.ends_with("USDT") || premium.symbol.starts_with("TEST") {
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
                    Some("MISSING_ASTER_SPOT_BOOK_TICKER".to_owned()),
                )
            }
            (Some(_), None) => {
                missing_perp_count += 1;
                (
                    "missing_perp".to_owned(),
                    Some("MISSING_ASTER_PERP_BOOK_TICKER".to_owned()),
                )
            }
            (None, None) => {
                missing_spot_count += 1;
                missing_perp_count += 1;
                (
                    "missing_spot_and_perp".to_owned(),
                    Some("MISSING_ASTER_SPOT_AND_PERP_BOOK_TICKER".to_owned()),
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

        rows.push(AsterBasisMarketRow {
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

    Ok(AsterBasisMonitorSnapshot {
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

fn fetch_bybit_basis_monitor_snapshot(
    options: &BybitBasisMonitorOptions,
) -> RuntimeResult<BybitBasisMonitorSnapshot> {
    let spot_json = fetch_public_json_with_curl(&bybit_spot_tickers_url())?;
    let linear_json = fetch_public_json_with_curl(&bybit_linear_tickers_url())?;
    let instrument_pages = fetch_bybit_linear_instrument_pages()?;
    build_bybit_basis_monitor_snapshot_from_json(
        &spot_json,
        &linear_json,
        &instrument_pages,
        options,
    )
}

fn fetch_bybit_linear_instrument_pages() -> RuntimeResult<Vec<String>> {
    let mut pages = Vec::new();
    let mut cursor = None::<String>;
    let mut seen_cursors = BTreeSet::new();
    loop {
        let url = bybit_linear_instruments_info_url(cursor.as_deref());
        let json = fetch_public_json_with_curl(&url)?;
        let next_cursor = bybit_response_next_page_cursor(&json, "bybit linear instruments")?;
        pages.push(json);
        let Some(next_cursor) = next_cursor.filter(|value| !value.trim().is_empty()) else {
            break;
        };
        if !seen_cursors.insert(next_cursor.clone()) {
            return Err(RuntimeError::LiveMarketData {
                message: format!("Bybit instruments pagination repeated cursor `{next_cursor}`"),
            });
        }
        cursor = Some(next_cursor);
    }
    Ok(pages)
}

fn build_bybit_basis_monitor_snapshot_from_json(
    spot_json: &str,
    linear_json: &str,
    instrument_pages: &[String],
    options: &BybitBasisMonitorOptions,
) -> RuntimeResult<BybitBasisMonitorSnapshot> {
    let updated_at = current_utc_timestamp()?.to_string();
    let min_abs_funding_rate =
        MonitorDecimal::parse("min_abs_funding_rate", &options.min_abs_funding_rate)?;
    let spot_books = parse_bybit_spot_ticker_rows(spot_json)?
        .into_iter()
        .map(|row| (row.symbol.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let linear_tickers = parse_bybit_linear_ticker_rows(linear_json)?
        .into_iter()
        .map(|row| (row.symbol.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let linear_symbols = parse_bybit_linear_perpetual_symbols(instrument_pages)?;

    let mut rows = Vec::new();
    let mut filtered_funding_count = 0_usize;
    let mut missing_spot_count = 0_usize;
    let mut missing_perp_count = 0_usize;

    for symbol in linear_symbols {
        let linear = linear_tickers.get(&symbol);
        if let Some(linear) = linear {
            let funding_rate = MonitorDecimal::parse("fundingRate", &linear.last_funding_rate)?;
            if funding_rate.abs_less_than(min_abs_funding_rate) {
                filtered_funding_count += 1;
                continue;
            }
        }

        let spot = spot_books.get(&symbol);
        let (mut source_status, reason) = match (spot, linear) {
            (Some(_), Some(_)) => ("complete".to_owned(), None),
            (None, Some(_)) => {
                missing_spot_count += 1;
                (
                    "missing_spot".to_owned(),
                    Some("MISSING_SPOT_TICKER".to_owned()),
                )
            }
            (Some(_), None) => {
                missing_perp_count += 1;
                (
                    "missing_perp".to_owned(),
                    Some("MISSING_LINEAR_PERP_TICKER".to_owned()),
                )
            }
            (None, None) => {
                missing_spot_count += 1;
                missing_perp_count += 1;
                (
                    "missing_spot_and_perp".to_owned(),
                    Some("MISSING_SPOT_AND_LINEAR_PERP_TICKER".to_owned()),
                )
            }
        };

        let mut signal_error = None;
        let signal = match (spot, linear) {
            (Some(spot), Some(linear)) => {
                match evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
                    symbol: symbol.clone(),
                    spot_best_bid: spot.bid_price.clone(),
                    spot_best_ask: spot.ask_price.clone(),
                    perp_best_bid: linear.bid_price.clone(),
                    perp_best_ask: linear.ask_price.clone(),
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

        rows.push(BybitBasisMarketRow {
            symbol,
            spot_bid: spot.map(|row| row.bid_price.clone()),
            spot_ask: spot.map(|row| row.ask_price.clone()),
            spot_bid_qty: spot.map(|row| row.bid_qty.clone()),
            spot_ask_qty: spot.map(|row| row.ask_qty.clone()),
            perp_bid: linear.map(|row| row.bid_price.clone()),
            perp_ask: linear.map(|row| row.ask_price.clone()),
            perp_bid_qty: linear.map(|row| row.bid_qty.clone()),
            perp_ask_qty: linear.map(|row| row.ask_qty.clone()),
            mark_price: linear.map(|row| row.mark_price.clone()).unwrap_or_default(),
            index_price: linear
                .map(|row| row.index_price.clone())
                .unwrap_or_default(),
            last_funding_rate: linear
                .map(|row| row.last_funding_rate.clone())
                .unwrap_or_default(),
            next_funding_time_ms: linear
                .map(|row| row.next_funding_time_ms.clone())
                .unwrap_or_default(),
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

    Ok(BybitBasisMonitorSnapshot {
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

fn fetch_okx_basis_monitor_snapshot(
    options: &OkxBasisMonitorOptions,
) -> RuntimeResult<OkxBasisMonitorSnapshot> {
    let spot_json = fetch_public_json_with_curl(&okx_tickers_url("SPOT"))?;
    let swap_json = fetch_public_json_with_curl(&okx_tickers_url("SWAP"))?;
    let mark_json = fetch_public_json_with_curl(&okx_mark_price_url())?;
    let index_json = fetch_public_json_with_curl(&okx_index_tickers_url())?;
    let swap_rows = parse_okx_ticker_rows(&swap_json, "okx swap tickers")?;
    let funding_pages = fetch_okx_usdt_swap_funding_rate_pages(&swap_rows)?;
    build_okx_basis_monitor_snapshot_from_json(
        &spot_json,
        &swap_json,
        &mark_json,
        &index_json,
        &funding_pages,
        options,
    )
}

fn fetch_okx_usdt_swap_funding_rate_pages(
    swap_rows: &[OkxTickerRow],
) -> RuntimeResult<Vec<String>> {
    let mut inst_ids = BTreeSet::new();
    for row in swap_rows {
        if okx_spot_inst_id_from_swap(&row.inst_id).is_some() {
            inst_ids.insert(row.inst_id.clone());
        }
    }

    let mut pages = Vec::new();
    for inst_id in inst_ids {
        pages.push(fetch_public_json_with_curl(&okx_funding_rate_url(
            &inst_id,
        ))?);
    }
    Ok(pages)
}

fn build_okx_basis_monitor_snapshot_from_json(
    spot_json: &str,
    swap_json: &str,
    mark_json: &str,
    index_json: &str,
    funding_pages: &[String],
    options: &OkxBasisMonitorOptions,
) -> RuntimeResult<OkxBasisMonitorSnapshot> {
    let updated_at = current_utc_timestamp()?.to_string();
    let min_abs_funding_rate =
        MonitorDecimal::parse("min_abs_funding_rate", &options.min_abs_funding_rate)?;
    let spot_books = parse_okx_ticker_rows(spot_json, "okx spot tickers")?
        .into_iter()
        .map(|row| (row.inst_id.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let swap_tickers = parse_okx_ticker_rows(swap_json, "okx swap tickers")?
        .into_iter()
        .map(|row| (row.inst_id.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let mark_prices = parse_okx_mark_price_rows(mark_json)?
        .into_iter()
        .map(|row| (row.inst_id.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let index_prices = parse_okx_index_ticker_rows(index_json)?
        .into_iter()
        .map(|row| (row.inst_id.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let funding_rates = parse_okx_funding_rate_pages(funding_pages)?
        .into_iter()
        .map(|row| (row.inst_id.clone(), row))
        .collect::<BTreeMap<_, _>>();

    let mut rows = Vec::new();
    let mut filtered_funding_count = 0_usize;
    let mut missing_spot_count = 0_usize;
    let mut missing_perp_count = 0_usize;

    for (swap_inst_id, swap) in swap_tickers {
        let Some(spot_inst_id) = okx_spot_inst_id_from_swap(&swap_inst_id) else {
            continue;
        };
        let funding = funding_rates.get(&swap_inst_id);
        if let Some(funding) = funding {
            let funding_rate = MonitorDecimal::parse("fundingRate", &funding.funding_rate)?;
            if funding_rate.abs_less_than(min_abs_funding_rate) {
                filtered_funding_count += 1;
                continue;
            }
        }

        let spot = spot_books.get(&spot_inst_id);
        let mark = mark_prices.get(&swap_inst_id);
        let index = index_prices.get(&spot_inst_id);
        let (mut source_status, reason) = match (spot, funding) {
            (Some(_), Some(_)) => ("complete".to_owned(), None),
            (None, Some(_)) => {
                missing_spot_count += 1;
                (
                    "missing_spot".to_owned(),
                    Some("MISSING_SPOT_TICKER".to_owned()),
                )
            }
            (Some(_), None) => (
                "missing_funding".to_owned(),
                Some("MISSING_FUNDING_RATE".to_owned()),
            ),
            (None, None) => {
                missing_spot_count += 1;
                (
                    "missing_spot_and_funding".to_owned(),
                    Some("MISSING_SPOT_AND_FUNDING_RATE".to_owned()),
                )
            }
        };

        let mut signal_error = None;
        let signal = match (spot, funding) {
            (Some(spot), Some(_)) => {
                match evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
                    symbol: spot_inst_id.clone(),
                    spot_best_bid: spot.bid_price.clone(),
                    spot_best_ask: spot.ask_price.clone(),
                    perp_best_bid: swap.bid_price.clone(),
                    perp_best_ask: swap.ask_price.clone(),
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

        rows.push(OkxBasisMarketRow {
            symbol: spot_inst_id,
            spot_bid: spot.map(|row| row.bid_price.clone()),
            spot_ask: spot.map(|row| row.ask_price.clone()),
            spot_bid_qty: spot.map(|row| row.bid_qty.clone()),
            spot_ask_qty: spot.map(|row| row.ask_qty.clone()),
            perp_bid: Some(swap.bid_price),
            perp_ask: Some(swap.ask_price),
            perp_bid_qty: Some(swap.bid_qty),
            perp_ask_qty: Some(swap.ask_qty),
            mark_price: mark.map(|row| row.mark_price.clone()).unwrap_or_default(),
            index_price: index.map(|row| row.index_price.clone()).unwrap_or_default(),
            last_funding_rate: funding
                .map(|row| row.funding_rate.clone())
                .unwrap_or_default(),
            next_funding_time_ms: funding
                .map(|row| row.next_funding_time_ms.clone())
                .unwrap_or_default(),
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

    if rows.is_empty() && !spot_books.is_empty() {
        missing_perp_count = spot_books
            .keys()
            .filter(|spot_inst_id| spot_inst_id.ends_with("-USDT"))
            .count();
    }

    rows.sort_by(|left, right| {
        monitor_optional_i128(&right.net_basis_bps)
            .cmp(&monitor_optional_i128(&left.net_basis_bps))
            .then_with(|| left.symbol.cmp(&right.symbol))
    });
    let candidate_count = rows.iter().filter(|row| row.is_candidate).count();

    Ok(OkxBasisMonitorSnapshot {
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

fn fetch_hyperliquid_basis_monitor_snapshot(
    options: &HyperliquidBasisMonitorOptions,
) -> RuntimeResult<HyperliquidBasisMonitorSnapshot> {
    let spot_json = fetch_public_json_post_with_curl(
        HYPERLIQUID_INFO_URL,
        &hyperliquid_info_request_body("spotMetaAndAssetCtxs"),
    )?;
    let perp_json = fetch_public_json_post_with_curl(
        HYPERLIQUID_INFO_URL,
        &hyperliquid_info_request_body("metaAndAssetCtxs"),
    )?;
    build_hyperliquid_basis_monitor_snapshot_from_json(&spot_json, &perp_json, options)
}

fn build_hyperliquid_basis_monitor_snapshot_from_json(
    spot_json: &str,
    perp_json: &str,
    options: &HyperliquidBasisMonitorOptions,
) -> RuntimeResult<HyperliquidBasisMonitorSnapshot> {
    let updated_at = current_utc_timestamp()?.to_string();
    let min_abs_funding_rate =
        MonitorDecimal::parse("min_abs_funding_rate", &options.min_abs_funding_rate)?;
    let spot_books = parse_hyperliquid_spot_context_rows(spot_json)?
        .into_iter()
        .map(|row| (row.coin.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let perp_contexts = parse_hyperliquid_perp_context_rows(perp_json)?;

    let mut rows = Vec::new();
    let mut filtered_funding_count = 0_usize;
    let mut missing_spot_count = 0_usize;
    let missing_perp_count = 0_usize;

    for perp in perp_contexts {
        let funding_rate = MonitorDecimal::parse("funding", &perp.funding_rate)?;
        if funding_rate.abs_less_than(min_abs_funding_rate) {
            filtered_funding_count += 1;
            continue;
        }

        let spot = spot_books.get(&perp.coin);
        let (mut source_status, reason) = match spot {
            Some(_) => ("complete".to_owned(), None),
            None => {
                missing_spot_count += 1;
                (
                    "missing_spot".to_owned(),
                    Some("MISSING_HYPERLIQUID_USDC_SPOT_CONTEXT".to_owned()),
                )
            }
        };

        let mut signal_error = None;
        let signal = match spot {
            Some(spot) => {
                match evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
                    symbol: perp.coin.clone(),
                    spot_best_bid: spot.price.clone(),
                    spot_best_ask: spot.price.clone(),
                    perp_best_bid: perp.price.clone(),
                    perp_best_ask: perp.price.clone(),
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
            None => None,
        };
        let reason = signal
            .as_ref()
            .and_then(|signal| signal.reason.clone())
            .or(signal_error)
            .or(reason);

        rows.push(HyperliquidBasisMarketRow {
            symbol: perp.coin,
            spot_bid: spot.map(|row| row.price.clone()),
            spot_ask: spot.map(|row| row.price.clone()),
            spot_bid_qty: None,
            spot_ask_qty: None,
            perp_bid: Some(perp.price.clone()),
            perp_ask: Some(perp.price),
            perp_bid_qty: None,
            perp_ask_qty: None,
            mark_price: perp.mark_price,
            index_price: perp.oracle_price,
            last_funding_rate: perp.funding_rate,
            next_funding_time_ms: String::new(),
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

    Ok(HyperliquidBasisMonitorSnapshot {
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

fn validate_binance_wss_probe_options(
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

fn normalize_binance_wss_symbol_scope(symbol: &str) -> RuntimeResult<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if is_binance_wss_all_symbols_scope(&symbol) {
        Ok(BINANCE_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS.to_owned())
    } else {
        validate_binance_public_wss_symbol(&symbol)
    }
}

fn is_binance_wss_all_symbols_scope(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "ALL" | "ALL_USDT" | "*"
    )
}

fn validate_binance_public_wss_symbol(symbol: &str) -> RuntimeResult<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol.len() < 3 || symbol.len() > 32 {
        return Err(cli_arg_error("Binance WSS symbol length must be 3..=32"));
    }
    if !symbol
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(cli_arg_error(
            "Binance WSS symbol must contain only uppercase ASCII letters and digits",
        ));
    }
    if !symbol.ends_with("USDT") {
        return Err(cli_arg_error(
            "current Binance WSS probe maps only USDT-quoted public instruments",
        ));
    }
    Ok(symbol)
}

fn parse_binance_public_wss_market(value: &str) -> RuntimeResult<BinancePublicMarket> {
    match value.trim().to_ascii_lowercase().as_str() {
        "spot" => Ok(BinancePublicMarket::Spot),
        "usdm" | "usdm-perp" | "usdm_perp" | "perp" => Ok(BinancePublicMarket::UsdmPerpetual),
        _ => Err(cli_arg_error(
            "--market must be `spot` or `usdm-perp` for Binance WSS bookTicker",
        )),
    }
}

fn binance_public_wss_venue_id(market: BinancePublicMarket) -> RuntimeResult<VenueId> {
    let value = match market {
        BinancePublicMarket::Spot => "venue:BINANCE-SPOT",
        BinancePublicMarket::UsdmPerpetual => "venue:BINANCE-USDM",
    };
    Ok(VenueId::new(value)?)
}

fn binance_public_wss_instrument(
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
    let instrument_id = match market {
        BinancePublicMarket::Spot => format!("inst:BINANCE:{symbol}:SPOT"),
        BinancePublicMarket::UsdmPerpetual => format!("inst:BINANCE:{symbol}:USDM-PERP"),
    };
    BinancePublicInstrument::new(
        symbol,
        InstrumentId::new(instrument_id)?,
        AssetId::new(format!("asset:{base}"))?,
        asset_usdt.clone(),
        asset_usdt,
    )
    .map_err(RuntimeError::from)
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

fn aster_spot_book_ticker_all_url() -> String {
    format!("{ASTER_SPOT_REST_BASE_URL}/api/v3/ticker/bookTicker")
}

fn aster_futures_book_ticker_all_url() -> String {
    format!("{ASTER_FUTURES_REST_BASE_URL}/fapi/v3/ticker/bookTicker")
}

fn aster_futures_premium_index_all_url() -> String {
    format!("{ASTER_FUTURES_REST_BASE_URL}/fapi/v3/premiumIndex")
}

fn bybit_spot_tickers_url() -> String {
    format!("{BYBIT_REST_BASE_URL}/v5/market/tickers?category=spot")
}

fn bybit_linear_tickers_url() -> String {
    format!("{BYBIT_REST_BASE_URL}/v5/market/tickers?category=linear")
}

fn bybit_linear_instruments_info_url(cursor: Option<&str>) -> String {
    let base = format!(
        "{BYBIT_REST_BASE_URL}/v5/market/instruments-info?category=linear&status=Trading&limit=1000"
    );
    match cursor {
        Some(cursor) => format!("{base}&cursor={cursor}"),
        None => base,
    }
}

fn okx_tickers_url(inst_type: &str) -> String {
    format!("{OKX_REST_BASE_URL}/api/v5/market/tickers?instType={inst_type}")
}

fn okx_mark_price_url() -> String {
    format!("{OKX_REST_BASE_URL}/api/v5/public/mark-price?instType=SWAP")
}

fn okx_index_tickers_url() -> String {
    format!("{OKX_REST_BASE_URL}/api/v5/market/index-tickers?quoteCcy=USDT")
}

fn okx_funding_rate_url(inst_id: &str) -> String {
    format!("{OKX_REST_BASE_URL}/api/v5/public/funding-rate?instId={inst_id}")
}

fn hyperliquid_info_request_body(request_type: &str) -> String {
    format!("{{\"type\":{}}}", json_string(request_type))
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

fn fetch_public_json_post_with_curl(url: &str, body: &str) -> RuntimeResult<String> {
    let output = Command::new("curl")
        .args([
            "-fsS",
            "--max-time",
            "10",
            "-X",
            "POST",
            url,
            "-H",
            "Content-Type: application/json",
            "--data",
            body,
        ])
        .output()
        .map_err(|error| RuntimeError::LiveMarketData {
            message: format!("cannot run curl POST for public market data: {error}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "curl POST returned {}; stderr={}",
                output.status,
                stderr.trim()
            ),
        });
    }

    let response =
        String::from_utf8(output.stdout).map_err(|error| RuntimeError::LiveMarketData {
            message: format!("public market data response was not UTF-8: {error}"),
        })?;
    if response.trim().is_empty() {
        return Err(RuntimeError::LiveMarketData {
            message: "public market data response was empty".to_owned(),
        });
    }
    Ok(response)
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
    const SCALE_DIGITS: usize = 18;

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

#[derive(Clone, Debug, Eq, PartialEq)]
struct BybitLinearTickerRow {
    symbol: String,
    bid_price: String,
    bid_qty: String,
    ask_price: String,
    ask_qty: String,
    mark_price: String,
    index_price: String,
    last_funding_rate: String,
    next_funding_time_ms: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BybitLinearInstrumentRow {
    symbol: String,
    contract_type: String,
    status: String,
    quote_coin: String,
    settle_coin: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OkxTickerRow {
    inst_id: String,
    bid_price: String,
    bid_qty: String,
    ask_price: String,
    ask_qty: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OkxMarkPriceRow {
    inst_id: String,
    mark_price: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OkxIndexTickerRow {
    inst_id: String,
    index_price: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OkxFundingRateRow {
    inst_id: String,
    funding_rate: String,
    next_funding_time_ms: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HyperliquidSpotContextRow {
    coin: String,
    price: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HyperliquidPerpContextRow {
    coin: String,
    price: String,
    mark_price: String,
    oracle_price: String,
    funding_rate: String,
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

fn parse_bybit_spot_ticker_rows(input: &str) -> RuntimeResult<Vec<MonitorBookTickerRow>> {
    bybit_response_list_object_slices(input, "bybit spot tickers", "spot")?
        .into_iter()
        .map(|object| {
            let fields = parse_json_object_value_slices(object)?;
            Ok(MonitorBookTickerRow {
                symbol: required_json_value_string(&fields, "symbol", "bybit spot tickers")?,
                bid_price: required_json_value_string(&fields, "bid1Price", "bybit spot tickers")?,
                bid_qty: required_json_value_string(&fields, "bid1Size", "bybit spot tickers")?,
                ask_price: required_json_value_string(&fields, "ask1Price", "bybit spot tickers")?,
                ask_qty: required_json_value_string(&fields, "ask1Size", "bybit spot tickers")?,
            })
        })
        .collect()
}

fn parse_bybit_linear_ticker_rows(input: &str) -> RuntimeResult<Vec<BybitLinearTickerRow>> {
    bybit_response_list_object_slices(input, "bybit linear tickers", "linear")?
        .into_iter()
        .map(|object| {
            let fields = parse_json_object_value_slices(object)?;
            Ok(BybitLinearTickerRow {
                symbol: required_json_value_string(&fields, "symbol", "bybit linear tickers")?,
                bid_price: required_json_value_string(
                    &fields,
                    "bid1Price",
                    "bybit linear tickers",
                )?,
                bid_qty: required_json_value_string(&fields, "bid1Size", "bybit linear tickers")?,
                ask_price: required_json_value_string(
                    &fields,
                    "ask1Price",
                    "bybit linear tickers",
                )?,
                ask_qty: required_json_value_string(&fields, "ask1Size", "bybit linear tickers")?,
                mark_price: required_json_value_string(
                    &fields,
                    "markPrice",
                    "bybit linear tickers",
                )?,
                index_price: required_json_value_string(
                    &fields,
                    "indexPrice",
                    "bybit linear tickers",
                )?,
                last_funding_rate: required_json_value_string(
                    &fields,
                    "fundingRate",
                    "bybit linear tickers",
                )?,
                next_funding_time_ms: required_json_value_string(
                    &fields,
                    "nextFundingTime",
                    "bybit linear tickers",
                )?,
            })
        })
        .collect()
}

fn parse_bybit_linear_perpetual_symbols(
    instrument_pages: &[String],
) -> RuntimeResult<BTreeSet<String>> {
    let mut symbols = BTreeSet::new();
    for page in instrument_pages {
        for row in parse_bybit_linear_instrument_rows(page)? {
            if row.status == "Trading"
                && row.contract_type == "LinearPerpetual"
                && row.quote_coin == "USDT"
                && row.settle_coin == "USDT"
                && row.symbol.ends_with("USDT")
            {
                symbols.insert(row.symbol);
            }
        }
    }
    Ok(symbols)
}

fn parse_bybit_linear_instrument_rows(input: &str) -> RuntimeResult<Vec<BybitLinearInstrumentRow>> {
    bybit_response_list_object_slices(input, "bybit linear instruments", "linear")?
        .into_iter()
        .map(|object| {
            let fields = parse_json_object_value_slices(object)?;
            Ok(BybitLinearInstrumentRow {
                symbol: required_json_value_string(&fields, "symbol", "bybit linear instruments")?,
                contract_type: required_json_value_string(
                    &fields,
                    "contractType",
                    "bybit linear instruments",
                )?,
                status: required_json_value_string(&fields, "status", "bybit linear instruments")?,
                quote_coin: required_json_value_string(
                    &fields,
                    "quoteCoin",
                    "bybit linear instruments",
                )?,
                settle_coin: required_json_value_string(
                    &fields,
                    "settleCoin",
                    "bybit linear instruments",
                )?,
            })
        })
        .collect()
}

fn bybit_response_list_object_slices<'a>(
    input: &'a str,
    source: &'static str,
    expected_category: &'static str,
) -> RuntimeResult<Vec<&'a str>> {
    let result_fields = bybit_response_result_fields(input, source)?;
    let category = required_json_value_string(&result_fields, "category", source)?;
    if category != expected_category {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "{source} returned category `{category}`, expected `{expected_category}`"
            ),
        });
    }
    let list = result_fields
        .get("list")
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("{source} response is missing result.list"),
        })?;
    json_object_slices(list)
}

fn bybit_response_next_page_cursor(
    input: &str,
    source: &'static str,
) -> RuntimeResult<Option<String>> {
    let result_fields = bybit_response_result_fields(input, source)?;
    optional_json_value_string(&result_fields, "nextPageCursor", source)
}

fn bybit_response_result_fields<'a>(
    input: &'a str,
    source: &'static str,
) -> RuntimeResult<BTreeMap<String, &'a str>> {
    let top_fields = parse_json_object_value_slices(input)?;
    let ret_code = required_json_value_string(&top_fields, "retCode", source)?;
    if ret_code != "0" {
        let ret_msg = optional_json_value_string(&top_fields, "retMsg", source)?
            .unwrap_or_else(|| "unknown".to_owned());
        return Err(RuntimeError::LiveMarketData {
            message: format!("{source} returned retCode={ret_code}, retMsg={ret_msg}"),
        });
    }
    let result = top_fields
        .get("result")
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("{source} response is missing result"),
        })?;
    parse_json_object_value_slices(result)
}

fn parse_okx_ticker_rows(input: &str, source: &'static str) -> RuntimeResult<Vec<OkxTickerRow>> {
    okx_response_data_object_slices(input, source)?
        .into_iter()
        .map(|object| {
            let fields = parse_json_object_value_slices(object)?;
            Ok(OkxTickerRow {
                inst_id: required_json_value_string(&fields, "instId", source)?,
                bid_price: required_json_value_string(&fields, "bidPx", source)?,
                bid_qty: required_json_value_string(&fields, "bidSz", source)?,
                ask_price: required_json_value_string(&fields, "askPx", source)?,
                ask_qty: required_json_value_string(&fields, "askSz", source)?,
            })
        })
        .collect()
}

fn parse_okx_mark_price_rows(input: &str) -> RuntimeResult<Vec<OkxMarkPriceRow>> {
    okx_response_data_object_slices(input, "okx mark price")?
        .into_iter()
        .map(|object| {
            let fields = parse_json_object_value_slices(object)?;
            Ok(OkxMarkPriceRow {
                inst_id: required_json_value_string(&fields, "instId", "okx mark price")?,
                mark_price: required_json_value_string(&fields, "markPx", "okx mark price")?,
            })
        })
        .collect()
}

fn parse_okx_index_ticker_rows(input: &str) -> RuntimeResult<Vec<OkxIndexTickerRow>> {
    okx_response_data_object_slices(input, "okx index tickers")?
        .into_iter()
        .map(|object| {
            let fields = parse_json_object_value_slices(object)?;
            Ok(OkxIndexTickerRow {
                inst_id: required_json_value_string(&fields, "instId", "okx index tickers")?,
                index_price: required_json_value_string(&fields, "idxPx", "okx index tickers")?,
            })
        })
        .collect()
}

fn parse_okx_funding_rate_pages(pages: &[String]) -> RuntimeResult<Vec<OkxFundingRateRow>> {
    let mut rows = Vec::new();
    for page in pages {
        rows.extend(parse_okx_funding_rate_rows(page)?);
    }
    Ok(rows)
}

fn parse_okx_funding_rate_rows(input: &str) -> RuntimeResult<Vec<OkxFundingRateRow>> {
    okx_response_data_object_slices(input, "okx funding rate")?
        .into_iter()
        .map(|object| {
            let fields = parse_json_object_value_slices(object)?;
            let next_funding_time_ms =
                optional_json_value_string(&fields, "nextFundingTime", "okx funding rate")?
                    .filter(|value| !value.trim().is_empty())
                    .map(Ok)
                    .unwrap_or_else(|| {
                        required_json_value_string(&fields, "fundingTime", "okx funding rate")
                    })?;
            Ok(OkxFundingRateRow {
                inst_id: required_json_value_string(&fields, "instId", "okx funding rate")?,
                funding_rate: required_json_value_string(
                    &fields,
                    "fundingRate",
                    "okx funding rate",
                )?,
                next_funding_time_ms,
            })
        })
        .collect()
}

fn okx_response_data_object_slices<'a>(
    input: &'a str,
    source: &'static str,
) -> RuntimeResult<Vec<&'a str>> {
    let top_fields = parse_json_object_value_slices(input)?;
    let code = required_json_value_string(&top_fields, "code", source)?;
    if code != "0" {
        let msg = optional_json_value_string(&top_fields, "msg", source)?
            .unwrap_or_else(|| "unknown".to_owned());
        return Err(RuntimeError::LiveMarketData {
            message: format!("{source} returned code={code}, msg={msg}"),
        });
    }
    let data = top_fields
        .get("data")
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("{source} response is missing data"),
        })?;
    json_object_slices(data)
}

fn okx_spot_inst_id_from_swap(inst_id: &str) -> Option<String> {
    inst_id
        .strip_suffix("-SWAP")
        .filter(|spot_inst_id| spot_inst_id.ends_with("-USDT"))
        .map(str::to_owned)
}

fn parse_hyperliquid_spot_context_rows(
    input: &str,
) -> RuntimeResult<Vec<HyperliquidSpotContextRow>> {
    let items = hyperliquid_meta_and_context_items(input, "hyperliquid spot contexts")?;
    let meta_fields = parse_json_object_value_slices(items.meta)?;
    let token_by_index = hyperliquid_spot_token_names(&meta_fields)?;
    let universe_value =
        meta_fields
            .get("universe")
            .ok_or_else(|| RuntimeError::LiveMarketData {
                message: "hyperliquid spot contexts response is missing meta.universe".to_owned(),
            })?;
    let universe = json_object_slices(universe_value)?;
    let contexts = json_object_slices(items.contexts)?;
    let mut rows_by_coin = BTreeMap::<String, (bool, HyperliquidSpotContextRow)>::new();

    for (index, universe_object) in universe.iter().enumerate() {
        let context = contexts
            .get(index)
            .ok_or_else(|| RuntimeError::LiveMarketData {
                message: format!(
                    "hyperliquid spot contexts missing asset context for universe index {index}"
                ),
            })?;
        let fields = parse_json_object_value_slices(universe_object)?;
        let token_indexes = required_json_usize_array(&fields, "tokens", "hyperliquid spot meta")?;
        if token_indexes.len() != 2 {
            return Err(RuntimeError::LiveMarketData {
                message: "hyperliquid spot meta tokens field must contain base and quote indexes"
                    .to_owned(),
            });
        }
        let base_coin = token_by_index
            .get(&token_indexes[0])
            .ok_or_else(|| RuntimeError::LiveMarketData {
                message: format!(
                    "hyperliquid spot meta references unknown base token index {}",
                    token_indexes[0]
                ),
            })?
            .clone();
        let quote_coin =
            token_by_index
                .get(&token_indexes[1])
                .ok_or_else(|| RuntimeError::LiveMarketData {
                    message: format!(
                        "hyperliquid spot meta references unknown quote token index {}",
                        token_indexes[1]
                    ),
                })?;
        if quote_coin != "USDC" {
            continue;
        }
        let is_canonical =
            optional_json_bool(&fields, "isCanonical", "hyperliquid spot meta")?.unwrap_or(false);
        let context_fields = parse_json_object_value_slices(context)?;
        let mid_price =
            optional_json_value_string(&context_fields, "midPx", "hyperliquid spot ctx")?
                .filter(|value| !value.trim().is_empty());
        let mark_price =
            optional_json_value_string(&context_fields, "markPx", "hyperliquid spot ctx")?
                .filter(|value| !value.trim().is_empty());
        let price = mid_price
            .or(mark_price)
            .ok_or_else(|| RuntimeError::LiveMarketData {
                message: format!(
                    "hyperliquid spot context for `{base_coin}` is missing midPx and markPx"
                ),
            })?;
        let row = HyperliquidSpotContextRow {
            coin: base_coin.clone(),
            price,
        };
        match rows_by_coin.get(&base_coin) {
            Some((existing_is_canonical, _)) if *existing_is_canonical => {}
            _ => {
                rows_by_coin.insert(base_coin, (is_canonical, row));
            }
        }
    }

    Ok(rows_by_coin
        .into_values()
        .map(|(_, row)| row)
        .collect::<Vec<_>>())
}

fn parse_hyperliquid_perp_context_rows(
    input: &str,
) -> RuntimeResult<Vec<HyperliquidPerpContextRow>> {
    let items = hyperliquid_meta_and_context_items(input, "hyperliquid perp contexts")?;
    let meta_fields = parse_json_object_value_slices(items.meta)?;
    let universe_value =
        meta_fields
            .get("universe")
            .ok_or_else(|| RuntimeError::LiveMarketData {
                message: "hyperliquid perp contexts response is missing meta.universe".to_owned(),
            })?;
    let universe = json_object_slices(universe_value)?;
    let contexts = json_object_slices(items.contexts)?;
    let mut rows = Vec::new();

    for (index, universe_object) in universe.iter().enumerate() {
        let context = contexts
            .get(index)
            .ok_or_else(|| RuntimeError::LiveMarketData {
                message: format!(
                    "hyperliquid perp contexts missing asset context for universe index {index}"
                ),
            })?;
        let fields = parse_json_object_value_slices(universe_object)?;
        if optional_json_bool(&fields, "isDelisted", "hyperliquid perp meta")?.unwrap_or(false) {
            continue;
        }
        let coin = required_json_value_string(&fields, "name", "hyperliquid perp meta")?;
        let context_fields = parse_json_object_value_slices(context)?;
        let mark_price =
            required_json_value_string(&context_fields, "markPx", "hyperliquid perp ctx")?;
        let price = optional_json_value_string(&context_fields, "midPx", "hyperliquid perp ctx")?
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| mark_price.clone());
        let oracle_price =
            optional_json_value_string(&context_fields, "oraclePx", "hyperliquid perp ctx")?
                .unwrap_or_default();
        rows.push(HyperliquidPerpContextRow {
            coin,
            price,
            mark_price,
            oracle_price,
            funding_rate: required_json_value_string(
                &context_fields,
                "funding",
                "hyperliquid perp ctx",
            )?,
        });
    }
    Ok(rows)
}

struct HyperliquidMetaAndContextItems<'a> {
    meta: &'a str,
    contexts: &'a str,
}

fn hyperliquid_meta_and_context_items<'a>(
    input: &'a str,
    source: &'static str,
) -> RuntimeResult<HyperliquidMetaAndContextItems<'a>> {
    let items = json_array_value_slices(input)?;
    if items.len() != 2 {
        return Err(RuntimeError::LiveMarketData {
            message: format!("{source} expected [meta, asset_contexts] response"),
        });
    }
    Ok(HyperliquidMetaAndContextItems {
        meta: items[0],
        contexts: items[1],
    })
}

fn hyperliquid_spot_token_names(
    meta_fields: &BTreeMap<String, &str>,
) -> RuntimeResult<BTreeMap<usize, String>> {
    let tokens_value = meta_fields
        .get("tokens")
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: "hyperliquid spot contexts response is missing meta.tokens".to_owned(),
        })?;
    let mut tokens = BTreeMap::new();
    for object in json_object_slices(tokens_value)? {
        let fields = parse_json_object_value_slices(object)?;
        let index = required_json_value_string(&fields, "index", "hyperliquid spot token")?
            .parse::<usize>()
            .map_err(|_| RuntimeError::LiveMarketData {
                message: "hyperliquid spot token index is not an unsigned integer".to_owned(),
            })?;
        let name = required_json_value_string(&fields, "name", "hyperliquid spot token")?;
        tokens.insert(index, name);
    }
    Ok(tokens)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum MonitorJsonScalar {
    String(String),
    Number(String),
    Bool(String),
    Null,
}

fn json_array_value_slices(input: &str) -> RuntimeResult<Vec<&str>> {
    let trimmed = input.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return Err(RuntimeError::LiveMarketData {
            message: "expected JSON array".to_owned(),
        });
    }
    let bytes = trimmed.as_bytes();
    let mut values = Vec::new();
    let mut index = 1_usize;
    loop {
        index = skip_json_ws(bytes, index);
        match bytes.get(index) {
            Some(b']') => break,
            Some(_) => {
                let value_start = index;
                let value_end = json_value_end(trimmed, index)?;
                values.push(trimmed[value_start..value_end].trim());
                index = skip_json_ws(bytes, value_end);
                match bytes.get(index) {
                    Some(b',') => index += 1,
                    Some(b']') => break,
                    _ => {
                        return Err(RuntimeError::LiveMarketData {
                            message: "expected ',' or ']' after JSON array value".to_owned(),
                        });
                    }
                }
            }
            None => {
                return Err(RuntimeError::LiveMarketData {
                    message: "unexpected end of JSON array".to_owned(),
                });
            }
        }
    }
    Ok(values)
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

fn parse_json_object_value_slices(input: &str) -> RuntimeResult<BTreeMap<String, &str>> {
    let trimmed = input.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err(RuntimeError::LiveMarketData {
            message: "expected JSON object".to_owned(),
        });
    }
    let bytes = trimmed.as_bytes();
    let mut fields = BTreeMap::new();
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
        let value_start = index;
        let value_end = json_value_end(trimmed, index)?;
        fields.insert(key, trimmed[value_start..value_end].trim());
        index = skip_json_ws(bytes, value_end);
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

fn json_value_end(input: &str, index: usize) -> RuntimeResult<usize> {
    let bytes = input.as_bytes();
    match bytes.get(index) {
        Some(b'"') => parse_json_string(input, index).map(|(_, after)| after),
        Some(b'{') | Some(b'[') => {
            let opening = bytes[index];
            let closing = if opening == b'{' { b'}' } else { b']' };
            let mut depth = 0_i32;
            let mut in_string = false;
            let mut escaped = false;
            for (offset, ch) in input[index..].char_indices() {
                let absolute = index + offset;
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
                    ch if ch as u8 == opening => depth += 1,
                    ch if ch as u8 == closing => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(absolute + ch.len_utf8());
                        }
                    }
                    _ => {}
                }
            }
            Err(RuntimeError::LiveMarketData {
                message: "unterminated JSON object or array value".to_owned(),
            })
        }
        Some(_) => {
            let mut end = index;
            while let Some(byte) = bytes.get(end) {
                if byte.is_ascii_whitespace() || matches!(byte, b',' | b'}' | b']') {
                    break;
                }
                end += 1;
            }
            if end == index {
                return Err(RuntimeError::LiveMarketData {
                    message: "empty JSON scalar".to_owned(),
                });
            }
            Ok(end)
        }
        None => Err(RuntimeError::LiveMarketData {
            message: "unexpected end of JSON while parsing value".to_owned(),
        }),
    }
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

fn required_json_value_string(
    fields: &BTreeMap<String, &str>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<String> {
    let value = fields
        .get(field)
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("{source} object is missing field `{field}`"),
        })?;
    json_value_to_string(value, field, source)
}

fn optional_json_value_string(
    fields: &BTreeMap<String, &str>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<Option<String>> {
    let Some(value) = fields.get(field) else {
        return Ok(None);
    };
    if value.trim() == "null" {
        return Ok(None);
    }
    json_value_to_string(value, field, source).map(Some)
}

fn required_json_usize_array(
    fields: &BTreeMap<String, &str>,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<Vec<usize>> {
    let value = fields
        .get(field)
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: format!("{source} object is missing array field `{field}`"),
        })?;
    json_array_value_slices(value)?
        .into_iter()
        .map(|item| {
            json_value_to_string(item, field, source)?
                .parse::<usize>()
                .map_err(|_| RuntimeError::LiveMarketData {
                    message: format!("{source} field `{field}` contains non-integer `{item}`"),
                })
        })
        .collect()
}

fn optional_json_bool(
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
            message: format!("{source} field `{field}` is not a JSON bool: `{other}`"),
        }),
    }
}

fn json_value_to_string(
    value: &str,
    field: &'static str,
    source: &'static str,
) -> RuntimeResult<String> {
    let value = value.trim();
    if value.starts_with('"') {
        let (decoded, after) = parse_json_string(value, 0)?;
        if skip_json_ws(value.as_bytes(), after) != value.len() {
            return Err(RuntimeError::LiveMarketData {
                message: format!("{source} field `{field}` has trailing JSON data"),
            });
        }
        return Ok(decoded);
    }
    if value.is_empty() || value == "null" || value.starts_with('{') || value.starts_with('[') {
        return Err(RuntimeError::LiveMarketData {
            message: format!("{source} field `{field}` is not a scalar string"),
        });
    }
    Ok(value.to_owned())
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

#[derive(Clone, Copy)]
struct BinanceBasisRawInputs<'a> {
    symbol: &'a str,
    raw_spot_book: &'a str,
    spot_book_ref: &'a str,
    raw_perp_book: &'a str,
    perp_book_ref: &'a str,
    raw_premium_index: &'a str,
    premium_index_ref: &'a str,
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

fn assemble_binance_basis_pipeline_from_raw_json(
    replay: &ReplayInput,
    inputs: BinanceBasisRawInputs<'_>,
    ingested_at: UtcTimestamp,
) -> RuntimeResult<EndToEndArtifacts> {
    let events = ingest_binance_basis_public_json(inputs, ingested_at)?;
    let spec = BasisPipelineSpec::binance_btcusdt()?;
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

fn build_public_basis_portfolio_state(
    spec: &BasisPipelineSpec,
    source_event_refs: &[String],
    as_of: UtcTimestamp,
) -> RuntimeResult<PortfolioState> {
    let portfolio_json = format!(
        r#"{{
  "schema_version": "1.0.0",
  "portfolio_state_id": {},
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
  "state_hash": {}
}}"#,
        json_string(&spec.portfolio_state_id),
        json_string(&as_of.to_string()),
        json_string_array(source_event_refs),
        json_string(&spec.portfolio_state_hash),
    );
    Ok(from_json_strict::<PortfolioState>(&portfolio_json)?)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BinanceGuardedLiveQuote {
    event_id: String,
    observed_at: String,
    best_bid: String,
    best_ask: String,
    bid_size: String,
    ask_size: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg(feature = "live-exec")]
struct BinanceBasisGuardedLiveSignalContext {
    spot_event_id: String,
    perp_event_id: String,
    premium_event_id: String,
    observed_at: String,
    spot_bid: String,
    spot_ask: String,
    spot_bid_size: String,
    spot_ask_size: String,
    perp_bid: String,
    perp_ask: String,
    perp_bid_size: String,
    perp_ask_size: String,
    last_funding_rate: String,
    mark_price: String,
    index_price: String,
    next_funding_time_ms: String,
    signal: SpotPerpBasisSignal,
}

fn load_binance_guarded_live_spot_quote(
    market_artifacts_dir: &Path,
) -> RuntimeResult<BinanceGuardedLiveQuote> {
    let path = market_artifacts_dir.join("stored_events.jsonl");
    let input = read_utf8(&path)?;
    for (index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event = from_json_strict::<NormalizedEvent>(line).map_err(RuntimeError::from)?;
        let instrument_id = event
            .instrument_id
            .as_ref()
            .and_then(|value| value.as_ref())
            .map(|value| value.as_str());
        let venue_id = event
            .venue_id
            .as_ref()
            .and_then(|value| value.as_ref())
            .map(|value| value.as_str());
        if event.event_type == NormalizedEventType::NormalizedMarketDataEvent
            && instrument_id == Some(BASIS_SPOT_INSTRUMENT_ID)
            && venue_id == Some(BASIS_SPOT_VENUE_ID)
            && payload_string(&event, "kind")? == "BookTicker"
            && payload_string(&event, "venue_symbol")? == BASIS_SYMBOL
        {
            return Ok(BinanceGuardedLiveQuote {
                event_id: event.event_id.as_str().to_owned(),
                observed_at: event.timestamp_event.as_str().to_owned(),
                best_bid: payload_string(&event, "best_bid")?.to_owned(),
                best_ask: payload_string(&event, "best_ask")?.to_owned(),
                bid_size: payload_string(&event, "bid_size")?.to_owned(),
                ask_size: payload_string(&event, "ask_size")?.to_owned(),
            });
        }
        if index > 1000 {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: format!(
                    "{} contains too many events for a single preflight preview",
                    path.display()
                ),
            });
        }
    }
    Err(RuntimeError::MissingFixture { path })
}

fn write_binance_guarded_live_auto_market_artifacts(
    raw_spot_book: &str,
    raw_response_ref: &str,
    ingested_at: UtcTimestamp,
    output_dir: &Path,
) -> RuntimeResult<BinanceGuardedLiveQuote> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut adapter = BinancePublicBookTickerAdapter::new(
        VenueId::new(BASIS_SPOT_VENUE_ID)?,
        binance_basis_instrument(BASIS_SYMBOL, BASIS_SPOT_INSTRUMENT_ID)?,
        BinancePublicMarket::Spot,
        ingested_at,
        MARKET_DATA_MAX_AGE_MS,
    )?;
    let batch = adapter.ingest_book_ticker_json(raw_spot_book, raw_response_ref, ingested_at)?;
    let quote = BinanceGuardedLiveQuote {
        event_id: batch.normalized_event.event_id.as_str().to_owned(),
        observed_at: batch.normalized_event.timestamp_event.as_str().to_owned(),
        best_bid: payload_string(&batch.normalized_event, "best_bid")?.to_owned(),
        best_ask: payload_string(&batch.normalized_event, "best_ask")?.to_owned(),
        bid_size: payload_string(&batch.normalized_event, "bid_size")?.to_owned(),
        ask_size: payload_string(&batch.normalized_event, "ask_size")?.to_owned(),
    };
    write_utf8(
        output_dir.join("stored_events.jsonl"),
        &jsonl_from_lines(vec![
            to_canonical_json(&batch.raw_event),
            to_canonical_json(&batch.normalized_event),
        ]),
    )?;
    write_utf8(
        output_dir.join("spot_book_ticker.raw.json"),
        &format!("{raw_spot_book}\n"),
    )?;
    Ok(quote)
}

#[cfg(feature = "live-exec")]
fn write_binance_basis_guarded_live_auto_market_artifacts(
    inputs: BinanceBasisRawInputs<'_>,
    ingested_at: UtcTimestamp,
    min_net_bps: i128,
    output_dir: &Path,
) -> RuntimeResult<BinanceBasisGuardedLiveSignalContext> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    let events = ingest_binance_basis_public_json(inputs, ingested_at)?;
    write_utf8(
        output_dir.join("stored_events.jsonl"),
        &canonical_jsonl(&events),
    )?;
    write_utf8(
        output_dir.join("spot_book_ticker.raw.json"),
        &format!("{}\n", inputs.raw_spot_book),
    )?;
    write_utf8(
        output_dir.join("perp_book_ticker.raw.json"),
        &format!("{}\n", inputs.raw_perp_book),
    )?;
    write_utf8(
        output_dir.join("premium_index.raw.json"),
        &format!("{}\n", inputs.raw_premium_index),
    )?;

    let spot =
        find_binance_basis_book_event(&events, BASIS_SPOT_VENUE_ID, BASIS_SPOT_INSTRUMENT_ID)?;
    let perp =
        find_binance_basis_book_event(&events, BASIS_PERP_VENUE_ID, BASIS_PERP_INSTRUMENT_ID)?;
    let premium = find_binance_basis_premium_event(&events)?;
    let signal = evaluate_spot_perp_basis_signal(&SpotPerpBasisSignalInput {
        symbol: BASIS_SYMBOL.to_owned(),
        spot_best_bid: payload_string(spot, "best_bid")?.to_owned(),
        spot_best_ask: payload_string(spot, "best_ask")?.to_owned(),
        perp_best_bid: payload_string(perp, "best_bid")?.to_owned(),
        perp_best_ask: payload_string(perp, "best_ask")?.to_owned(),
        notional_usd: BINANCE_GUARDED_LIVE_NOTIONAL_USDT.to_owned(),
        spot_taker_fee_bps: BASIS_MONITOR_DEFAULT_SPOT_TAKER_FEE_BPS,
        perp_taker_fee_bps: BASIS_MONITOR_DEFAULT_PERP_TAKER_FEE_BPS,
        slippage_buffer_bps: BASIS_MONITOR_DEFAULT_SLIPPAGE_BUFFER_BPS,
        min_net_bps,
    })
    .map_err(|message| RuntimeError::Module {
        module: "arb-strategies",
        message,
    })?;

    Ok(BinanceBasisGuardedLiveSignalContext {
        spot_event_id: spot.event_id.as_str().to_owned(),
        perp_event_id: perp.event_id.as_str().to_owned(),
        premium_event_id: premium.event_id.as_str().to_owned(),
        observed_at: ingested_at.to_string(),
        spot_bid: payload_string(spot, "best_bid")?.to_owned(),
        spot_ask: payload_string(spot, "best_ask")?.to_owned(),
        spot_bid_size: payload_string(spot, "bid_size")?.to_owned(),
        spot_ask_size: payload_string(spot, "ask_size")?.to_owned(),
        perp_bid: payload_string(perp, "best_bid")?.to_owned(),
        perp_ask: payload_string(perp, "best_ask")?.to_owned(),
        perp_bid_size: payload_string(perp, "bid_size")?.to_owned(),
        perp_ask_size: payload_string(perp, "ask_size")?.to_owned(),
        last_funding_rate: payload_string(premium, "last_funding_rate")?.to_owned(),
        mark_price: payload_string(premium, "mark_price")?.to_owned(),
        index_price: payload_string(premium, "index_price")?.to_owned(),
        next_funding_time_ms: payload_scalar_text(premium, "next_funding_time_ms")?,
        signal,
    })
}

#[cfg(feature = "live-exec")]
fn find_binance_basis_book_event<'a>(
    events: &'a [NormalizedEvent],
    venue_id: &str,
    instrument_id: &str,
) -> RuntimeResult<&'a NormalizedEvent> {
    events
        .iter()
        .find(|event| {
            event.event_type == NormalizedEventType::NormalizedMarketDataEvent
                && event
                    .venue_id
                    .as_ref()
                    .and_then(|value| value.as_ref())
                    .is_some_and(|value| value.as_str() == venue_id)
                && event
                    .instrument_id
                    .as_ref()
                    .and_then(|value| value.as_ref())
                    .is_some_and(|value| value.as_str() == instrument_id)
                && payload_string(event, "kind").is_ok_and(|kind| kind == "BookTicker")
        })
        .ok_or_else(|| RuntimeError::Module {
            module: "arb-runtime",
            message: format!("missing Binance basis book event for {venue_id} {instrument_id}"),
        })
}

#[cfg(feature = "live-exec")]
fn find_binance_basis_premium_event(events: &[NormalizedEvent]) -> RuntimeResult<&NormalizedEvent> {
    events
        .iter()
        .find(|event| {
            event.event_type == NormalizedEventType::NormalizedMarketDataEvent
                && event
                    .venue_id
                    .as_ref()
                    .and_then(|value| value.as_ref())
                    .is_some_and(|value| value.as_str() == BASIS_PERP_VENUE_ID)
                && event
                    .instrument_id
                    .as_ref()
                    .and_then(|value| value.as_ref())
                    .is_some_and(|value| value.as_str() == BASIS_PERP_INSTRUMENT_ID)
                && payload_string(event, "kind").is_ok_and(|kind| kind == "PerpPremiumIndex")
        })
        .ok_or_else(|| RuntimeError::Module {
            module: "arb-runtime",
            message: "missing Binance basis premium index event".to_owned(),
        })
}

fn payload_string<'a>(event: &'a NormalizedEvent, field: &'static str) -> RuntimeResult<&'a str> {
    match event.payload.get(field) {
        Some(JsonValue::String(value)) => Ok(value.as_str()),
        Some(_) => Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "normalized event `{}` payload field `{field}` is not a string",
                event.event_id.as_str()
            ),
        }),
        None => Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "normalized event `{}` is missing payload field `{field}`",
                event.event_id.as_str()
            ),
        }),
    }
}

#[cfg(feature = "live-exec")]
fn payload_scalar_text(event: &NormalizedEvent, field: &'static str) -> RuntimeResult<String> {
    match event.payload.get(field) {
        Some(JsonValue::String(value)) => Ok(value.to_owned()),
        Some(JsonValue::Number(value)) => Ok(value.as_str().to_owned()),
        Some(_) => Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "normalized event `{}` payload field `{field}` is not a string or number",
                event.event_id.as_str()
            ),
        }),
        None => Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "normalized event `{}` is missing payload field `{field}`",
                event.event_id.as_str()
            ),
        }),
    }
}

fn binance_guarded_live_candidate(
    quote: &BinanceGuardedLiveQuote,
    created_at: &str,
) -> RuntimeResult<CandidatePortfolioTransition> {
    let candidate_json = format!(
        r#"{{
  "schema_version": "1.0.0",
  "transition_id": {transition_id},
  "strategy_id": {strategy_id},
  "strategy_version": "1.0.0",
  "code_version": "code:binance-guarded-live-preview-1",
  "config_version": "arb-config-v1",
  "created_at": {created_at},
  "input_event_refs": [{source_event}],
  "current_portfolio_state_ref": "state:binance-btcusdt-guarded-live-personal-redacted",
  "holding_period": {{"kind": "Instant"}},
  "legs": [
    {{
      "leg_id": "candleg:binance-btcusdt-guarded-live-buy",
      "leg_type": "Trade",
      "venue_id": {venue_id},
      "instrument_id": {instrument_id},
      "account_id": {account_id},
      "side": "Buy",
      "asset_flows": [
        {{"account_id": {account_id}, "asset_id": {quote_asset_id}, "amount": {notional}, "direction": "Out"}}
      ],
      "constraints": {{
        "max_slippage_bps": "5",
        "max_notional_usdt": {notional},
        "post_only": false,
        "reference_best_ask": {best_ask},
        "reference_best_bid": {best_bid},
        "reference_bid_size": {bid_size},
        "reference_ask_size": {ask_size},
        "reference_market_event_id": {source_event},
        "owner_scope_ref": "owner-note:2026-05-13-final-decision-update-binance-btcusdt-v1"
      }},
      "failure_modes": ["ManualInterventionRequired", "UnknownState"]
    }}
  ],
  "expected_post_state_delta": {{
    "asset_flows": [],
    "position_deltas": [
      {{"account_id": {account_id}, "instrument_id": {instrument_id}, "quantity_delta": {quantity}}}
    ]
  }},
  "expected_economics": {{
    "expected_profit_usd": "0",
    "expected_profit_bps": "0",
    "fee_estimate_usd": "0.02",
    "slippage_estimate_usd": "0.01",
    "confidence": 0.5
  }},
  "required_capital": {{
    "asset_requirements": [
      {{"account_id": {account_id}, "asset_id": {quote_asset_id}, "amount": {notional}, "direction": "Out"}}
    ],
    "recovery_buffer_usd": "1.00"
  }},
  "failure_modes": ["ManualInterventionRequired", "UnknownState"],
  "risk_flags": [],
  "assumptions": [
    {{
      "assumption_id": "asm:binance-btcusdt-guarded-live-public-quote",
      "statement": "Plan preview uses Binance public BTCUSDT spot BookTicker only; private balance, live permission and signer state must be checked before any dispatch.",
      "confidence": 0.5,
      "source_event_refs": [{source_event}]
    }}
  ]
}}"#,
        transition_id = json_string(BINANCE_GUARDED_LIVE_TRANSITION_ID),
        strategy_id = json_string(BINANCE_GUARDED_LIVE_STRATEGY_ID),
        created_at = json_string(created_at),
        source_event = json_string(&quote.event_id),
        venue_id = json_string(BASIS_SPOT_VENUE_ID),
        instrument_id = json_string(BASIS_SPOT_INSTRUMENT_ID),
        account_id = json_string(BINANCE_GUARDED_LIVE_ACCOUNT_REF),
        quote_asset_id = json_string(BASIS_QUOTE_ASSET_ID),
        notional = json_string(BINANCE_GUARDED_LIVE_NOTIONAL_USDT),
        best_ask = json_string(&quote.best_ask),
        best_bid = json_string(&quote.best_bid),
        bid_size = json_string(&quote.bid_size),
        ask_size = json_string(&quote.ask_size),
        quantity = json_string(BINANCE_GUARDED_LIVE_QUANTITY_BTC),
    );
    Ok(from_json_strict::<CandidatePortfolioTransition>(
        &candidate_json,
    )?)
}

fn binance_guarded_live_manual_risk_decision(
    quote: &BinanceGuardedLiveQuote,
    evaluated_at: &str,
) -> RuntimeResult<RiskDecision> {
    let risk_json = format!(
        r#"{{
  "schema_version": "1.0.0",
  "decision_id": "risk:trans:binance-btcusdt-guarded-live-preview-001",
  "transition_id": {transition_id},
  "evaluated_at": {evaluated_at},
  "decision": "RequiresManualApproval",
  "policy_version": "risk-policy:binance-btcusdt-guarded-live-preview-v1",
  "policy_hash": "hash:binance-btcusdt-guarded-live-preview-v1",
  "policy_signature_ref": "sigref:risk-policy-unsigned",
  "input_state_ref": "state:binance-btcusdt-guarded-live-personal-redacted",
  "checks": [
    {{
      "check_id": "check:binance-btcusdt:public-data-freshness",
      "check_type": "DataFreshness",
      "status": "Pass",
      "severity": "Info",
      "threshold": {{"decimal_value": "5000", "unit": "ms"}},
      "observed": {{"string_value": {observed_at}, "unit": "timestamp"}},
      "reason_code": "CHECK_PASSED",
      "detail": "Binance public spot BookTicker is present for plan preview; freshness must be rechecked before dispatch."
    }},
    {{
      "check_id": "check:binance-btcusdt:manual-approval-required",
      "check_type": "OneLegExecutionRisk",
      "status": "Warning",
      "severity": "Warn",
      "observed": {{"string_value": "owner confirmation required", "unit": "manual_gate"}},
      "reason_code": "REQUIRES_MANUAL_APPROVAL",
      "detail": "GuardedLivePersonal requires owner confirmation of the same plan_hash before any account-changing action."
    }},
    {{
      "check_id": "check:binance-btcusdt:notional-cap",
      "check_type": "StrategyExposureLimit",
      "status": "Pass",
      "severity": "Info",
      "threshold": {{"decimal_value": {per_order_limit}, "unit": "USDT"}},
      "observed": {{"decimal_value": {notional}, "unit": "USDT"}},
      "reason_code": "CHECK_PASSED",
      "detail": "Plan preview notional is within the 10 USDT per-order cap."
    }},
    {{
      "check_id": "check:binance-btcusdt:daily-loss-cap",
      "check_type": "DailyLossLimit",
      "status": "Pass",
      "severity": "Info",
      "threshold": {{"decimal_value": {daily_loss_limit}, "unit": "USDT"}},
      "observed": {{"decimal_value": "0", "unit": "USDT"}},
      "reason_code": "CHECK_PASSED",
      "detail": "No live loss is recorded in this plan preview; live session must stop at the configured daily threshold."
    }},
    {{
      "check_id": "check:binance-btcusdt:capital-cap",
      "check_type": "CapitalReservationAvailability",
      "status": "Pass",
      "severity": "Info",
      "threshold": {{"decimal_value": {capital_limit}, "unit": "USDT"}},
      "observed": {{"decimal_value": {notional}, "unit": "USDT"}},
      "reason_code": "CHECK_PASSED",
      "detail": "Plan preview notional is within the GuardedLivePersonal capital cap; live private balance remains unknown and must be checked before dispatch."
    }}
  ],
  "constraints": [
    {{
      "constraint_id": "constraint:binance-btcusdt:max-notional",
      "constraint_type": "MaxNotional",
      "field_path": "$.legs[0].asset_flows[0].amount",
      "limit": {{"decimal_value": {per_order_limit}, "unit": "USDT"}}
    }},
    {{
      "constraint_id": "constraint:binance-btcusdt:manual-approval",
      "constraint_type": "RequiresManualApproval",
      "field_path": "$.decision",
      "limit": {{"string_value": "owner must approve the same plan_hash", "unit": "approval_requirement"}}
    }},
    {{
      "constraint_id": "constraint:binance-btcusdt:capital-cap",
      "constraint_type": "MaxNotional",
      "field_path": "$.required_capital.asset_requirements",
      "limit": {{"decimal_value": {capital_limit}, "unit": "USDT"}}
    }}
  ],
  "reason_codes": ["REQUIRES_MANUAL_APPROVAL"],
  "detail": "Plan preview is not dispatchable. Owner confirmation of the same plan_hash is required and does not bypass risk, kill switch, permissions, capital reservation, ledger or reconciliation."
}}"#,
        transition_id = json_string(BINANCE_GUARDED_LIVE_TRANSITION_ID),
        evaluated_at = json_string(evaluated_at),
        observed_at = json_string(&quote.observed_at),
        per_order_limit = json_string(BINANCE_GUARDED_LIVE_NOTIONAL_USDT),
        notional = json_string(BINANCE_GUARDED_LIVE_NOTIONAL_USDT),
        daily_loss_limit = json_string(BINANCE_GUARDED_LIVE_DAILY_LOSS_LIMIT_USDT),
        capital_limit = json_string(BINANCE_GUARDED_LIVE_CAPITAL_LIMIT_USDT),
    );
    Ok(from_json_strict::<RiskDecision>(&risk_json)?)
}

#[cfg(feature = "live-exec")]
fn binance_basis_guarded_live_candidate(
    context: &BinanceBasisGuardedLiveSignalContext,
    created_at: &str,
    client_order_suffix: &str,
) -> RuntimeResult<CandidatePortfolioTransition> {
    let short_quantity = format!("-{}", context.signal.quantity);
    let spot_client_order_id = format!("rvbS{client_order_suffix}");
    let perp_client_order_id = format!("rvbP{client_order_suffix}");
    let candidate_json = format!(
        r#"{{
  "schema_version": "1.0.0",
  "transition_id": {transition_id},
  "strategy_id": {strategy_id},
  "strategy_version": "1.0.0",
  "code_version": "code:binance-basis-guarded-live-auto-1",
  "config_version": "arb-config-v1",
  "created_at": {created_at},
  "input_event_refs": [{spot_event},{perp_event},{premium_event}],
  "current_portfolio_state_ref": "state:binance-basis-guarded-live-private-redacted",
  "holding_period": {{"kind": "UntilBasisConvergence"}},
  "legs": [
    {{
      "leg_id": "candleg:binance-basis-live-buy-spot-btcusdt",
      "leg_type": "Trade",
      "venue_id": {spot_venue},
      "instrument_id": {spot_instrument},
      "account_id": {account_id},
      "side": "Buy",
      "asset_flows": [
        {{"account_id": {account_id}, "asset_id": {quote_asset}, "amount": {notional}, "direction": "Out"}},
        {{"account_id": {account_id}, "asset_id": {base_asset}, "amount": {quantity}, "direction": "In"}}
      ],
      "constraints": {{
        "basis_leg_role": "spot_buy",
        "client_order_id": {spot_client_order_id},
        "gross_basis_bps": {gross_bps},
        "max_slippage_bps": "5",
        "net_basis_bps": {net_bps},
        "notional_usdt": {notional},
        "post_only": false,
        "reference_best_ask": {spot_ask},
        "reference_best_bid": {spot_bid},
        "reference_bid_size": {spot_bid_size},
        "reference_ask_size": {spot_ask_size},
        "reference_market_event_id": {spot_event}
      }},
      "failure_modes": ["PartialFill", "VenueOutage", "UnknownState"]
    }},
    {{
      "leg_id": "candleg:binance-basis-live-short-usdm-btcusdt",
      "leg_type": "Trade",
      "venue_id": {perp_venue},
      "instrument_id": {perp_instrument},
      "account_id": {account_id},
      "side": "Short",
      "asset_flows": [],
      "constraints": {{
        "basis_leg_role": "perp_short",
        "client_order_id": {perp_client_order_id},
        "gross_basis_bps": {gross_bps},
        "last_funding_rate": {last_funding_rate},
        "max_slippage_bps": "5",
        "net_basis_bps": {net_bps},
        "notional_usdt": {notional},
        "post_only": false,
        "reference_best_ask": {perp_ask},
        "reference_best_bid": {perp_bid},
        "reference_bid_size": {perp_bid_size},
        "reference_ask_size": {perp_ask_size},
        "reference_market_event_id": {perp_event},
        "reference_premium_event_id": {premium_event}
      }},
      "failure_modes": ["PartialFill", "VenueOutage", "UnknownState"]
    }}
  ],
  "expected_post_state_delta": {{
    "asset_flows": [
      {{"account_id": {account_id}, "asset_id": {quote_asset}, "amount": {notional}, "direction": "Out"}},
      {{"account_id": {account_id}, "asset_id": {base_asset}, "amount": {quantity}, "direction": "In"}}
    ],
    "position_deltas": [
      {{"account_id": {account_id}, "instrument_id": {spot_instrument}, "quantity_delta": {quantity}}},
      {{"account_id": {account_id}, "instrument_id": {perp_instrument}, "quantity_delta": {short_quantity}}}
    ]
  }},
  "expected_economics": {{
    "expected_profit_usd": {expected_profit_usd},
    "expected_profit_bps": {net_bps},
    "fee_estimate_usd": {fee_estimate_usd},
    "slippage_estimate_usd": {slippage_estimate_usd},
    "confidence": 0.60
  }},
  "required_capital": {{
    "asset_requirements": [
      {{"account_id": {account_id}, "asset_id": {quote_asset}, "amount": {notional}, "direction": "Out"}}
    ],
    "recovery_buffer_usd": "1.00"
  }},
  "funding_impact": {{
    "summary": {funding_summary},
    "impact_usd": "0",
    "confidence": 0.50
  }},
  "failure_modes": ["PartialFill", "ManualInterventionRequired", "UnknownState"],
  "risk_flags": ["FundingRateUnstable", "BasisWidening", "OneLegExecutionRisk"],
  "assumptions": [
    {{
      "assumption_id": "asm:binance-basis-live-public-signal",
      "statement": "Automatic live basis signal uses public spot/perp bookTicker and premiumIndex; private balances and both execution venues must be checked before dispatch.",
      "confidence": 0.50,
      "source_event_refs": [{spot_event},{perp_event},{premium_event}]
    }}
  ]
}}"#,
        transition_id = json_string(BINANCE_BASIS_LIVE_TRANSITION_ID),
        strategy_id = json_string(BINANCE_BASIS_LIVE_STRATEGY_ID),
        created_at = json_string(created_at),
        spot_event = json_string(&context.spot_event_id),
        perp_event = json_string(&context.perp_event_id),
        premium_event = json_string(&context.premium_event_id),
        spot_venue = json_string(BASIS_SPOT_VENUE_ID),
        perp_venue = json_string(BASIS_PERP_VENUE_ID),
        spot_instrument = json_string(BASIS_SPOT_INSTRUMENT_ID),
        perp_instrument = json_string(BASIS_PERP_INSTRUMENT_ID),
        account_id = json_string(BINANCE_GUARDED_LIVE_ACCOUNT_REF),
        quote_asset = json_string(BASIS_QUOTE_ASSET_ID),
        base_asset = json_string(BASIS_BASE_ASSET_ID),
        notional = json_string(BINANCE_GUARDED_LIVE_NOTIONAL_USDT),
        quantity = json_string(&context.signal.quantity),
        short_quantity = json_string(&short_quantity),
        spot_client_order_id = json_string(&spot_client_order_id),
        perp_client_order_id = json_string(&perp_client_order_id),
        gross_bps = json_string(&context.signal.gross_bps.to_string()),
        net_bps = json_string(&context.signal.net_bps.to_string()),
        spot_ask = json_string(&context.spot_ask),
        spot_bid = json_string(&context.spot_bid),
        spot_bid_size = json_string(&context.spot_bid_size),
        spot_ask_size = json_string(&context.spot_ask_size),
        perp_ask = json_string(&context.perp_ask),
        perp_bid = json_string(&context.perp_bid),
        perp_bid_size = json_string(&context.perp_bid_size),
        perp_ask_size = json_string(&context.perp_ask_size),
        last_funding_rate = json_string(&context.last_funding_rate),
        expected_profit_usd = json_string(&context.signal.expected_profit_usd),
        fee_estimate_usd = json_string(&context.signal.fee_estimate_usd),
        slippage_estimate_usd = json_string(&context.signal.slippage_estimate_usd),
        funding_summary = json_string(&format!(
            "lastFundingRate={}, mark_price={}, index_price={}, nextFundingTimeMs={}",
            context.last_funding_rate,
            context.mark_price,
            context.index_price,
            context.next_funding_time_ms
        )),
    );
    Ok(from_json_strict::<CandidatePortfolioTransition>(
        &candidate_json,
    )?)
}

#[cfg(feature = "live-exec")]
fn binance_basis_guarded_live_manual_risk_decision(
    context: &BinanceBasisGuardedLiveSignalContext,
    evaluated_at: &str,
    min_net_bps: i128,
) -> RuntimeResult<RiskDecision> {
    let risk_json = format!(
        r#"{{
  "schema_version": "1.0.0",
  "decision_id": "risk:trans:binance-basis-guarded-live-auto-001",
  "transition_id": {transition_id},
  "evaluated_at": {evaluated_at},
  "decision": "RequiresManualApproval",
  "policy_version": "risk-policy:binance-basis-guarded-live-auto-v1",
  "policy_hash": "hash:binance-basis-guarded-live-auto-v1",
  "policy_signature_ref": "sigref:risk-policy-unsigned",
  "input_state_ref": "state:binance-basis-guarded-live-private-redacted",
  "checks": [
    {{
      "check_id": "check:binance-basis-live:public-data-freshness",
      "check_type": "DataFreshness",
      "status": "Pass",
      "severity": "Info",
      "threshold": {{"decimal_value": "5000", "unit": "ms"}},
      "observed": {{"string_value": {observed_at}, "unit": "timestamp"}},
      "reason_code": "CHECK_PASSED",
      "detail": "Binance spot/perp public bookTicker and premiumIndex are present for this fresh automatic basis signal."
    }},
    {{
      "check_id": "check:binance-basis-live:net-basis",
      "check_type": "StrategyExposureLimit",
      "status": "Pass",
      "severity": "Info",
      "threshold": {{"decimal_value": {min_net_bps}, "unit": "bps"}},
      "observed": {{"decimal_value": {net_bps}, "unit": "bps"}},
      "reason_code": "CHECK_PASSED",
      "detail": "Public basis signal net_bps passed the automatic strategy threshold before private checks."
    }},
    {{
      "check_id": "check:binance-basis-live:manual-gate",
      "check_type": "OneLegExecutionRisk",
      "status": "Warning",
      "severity": "Warn",
      "observed": {{"string_value": "two-leg basis live order path requires guarded approval fact", "unit": "manual_gate"}},
      "reason_code": "REQUIRES_MANUAL_APPROVAL",
      "detail": "Spot buy and USD-M perp short must share the same plan_hash and still pass private balance, kill switch and execution gates."
    }},
    {{
      "check_id": "check:binance-basis-live:notional-cap",
      "check_type": "StrategyExposureLimit",
      "status": "Pass",
      "severity": "Info",
      "threshold": {{"decimal_value": {notional}, "unit": "USDT"}},
      "observed": {{"decimal_value": {notional}, "unit": "USDT"}},
      "reason_code": "CHECK_PASSED",
      "detail": "Each leg uses the configured small notional cap."
    }}
  ],
  "constraints": [
    {{
      "constraint_id": "constraint:binance-basis-live:max-notional",
      "constraint_type": "MaxNotional",
      "field_path": "$.required_capital.asset_requirements",
      "limit": {{"decimal_value": {notional}, "unit": "USDT"}}
    }},
    {{
      "constraint_id": "constraint:binance-basis-live:manual-approval",
      "constraint_type": "RequiresManualApproval",
      "field_path": "$.decision",
      "limit": {{"string_value": "automatic guarded approval must reference the same plan_hash", "unit": "approval_requirement"}}
    }}
  ],
  "reason_codes": ["REQUIRES_MANUAL_APPROVAL"],
  "detail": "Automatic basis signal is a two-leg arbitrage candidate, but dispatch remains gated by same-plan approval, private balances, kill switch, permissions, confirmation and reconciliation."
}}"#,
        transition_id = json_string(BINANCE_BASIS_LIVE_TRANSITION_ID),
        evaluated_at = json_string(evaluated_at),
        observed_at = json_string(&context.observed_at),
        min_net_bps = json_string(&min_net_bps.to_string()),
        net_bps = json_string(&context.signal.net_bps.to_string()),
        notional = json_string(BINANCE_GUARDED_LIVE_NOTIONAL_USDT),
    );
    Ok(from_json_strict::<RiskDecision>(&risk_json)?)
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
    basis_capabilities_from_json_lines(&[spot, perp])
}

fn load_bybit_basis_capabilities() -> RuntimeResult<Vec<VenueCapabilityDescriptor>> {
    let spot = r#"{"auth_modes":["PublicOnly"],"capability_version":"1.0.0","data_surfaces":["RESTPolling","RateLimitHeaders"],"execution_capabilities":["SupportsManualApprovalOnly"],"health_model":{"disconnect_threshold":3,"freshness_threshold_ms":5000,"unknown_state_is_critical":true},"market_capabilities":["ProvidesSpotMarkets","ProvidesOrderBookMarkets"],"permission_model":{"can_read_private_data":false,"can_read_public_data":true,"can_trade":false,"can_withdraw":false},"rate_limit_model":{"limit":600,"source":"bybit-public-spot-rest","unit":"Request","window_ms":5000},"schema_version":"1.0.0","settlement_modes":["OffChainCustody"],"venue_id":"venue:BYBIT-SPOT","venue_name":"Bybit Spot Public REST"}"#;
    let perp = r#"{"auth_modes":["PublicOnly"],"capability_version":"1.0.0","data_surfaces":["RESTPolling","RateLimitHeaders","FundingHistory"],"execution_capabilities":["SupportsManualApprovalOnly"],"health_model":{"disconnect_threshold":3,"freshness_threshold_ms":5000,"unknown_state_is_critical":true},"market_capabilities":["ProvidesPerpetuals","ProvidesOrderBookMarkets","ProvidesFundingRates"],"permission_model":{"can_read_private_data":false,"can_read_public_data":true,"can_trade":false,"can_withdraw":false},"rate_limit_model":{"limit":600,"source":"bybit-public-linear-rest","unit":"Request","window_ms":5000},"schema_version":"1.0.0","settlement_modes":["OffChainCustody"],"venue_id":"venue:BYBIT-LINEAR","venue_name":"Bybit Linear Public REST"}"#;
    basis_capabilities_from_json_lines(&[spot, perp])
}

fn basis_capabilities_from_json_lines(
    lines: &[&str],
) -> RuntimeResult<Vec<VenueCapabilityDescriptor>> {
    if lines.is_empty() {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: "basis capability input is empty".to_owned(),
        });
    }
    let mut capabilities = Vec::with_capacity(lines.len());
    for line in lines {
        capabilities.push(from_json_strict::<VenueCapabilityDescriptor>(line)?);
    }
    Ok(capabilities)
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
    fixed_time: &str,
    spec: &BasisPipelineSpec,
) -> RuntimeResult<StrategyEvaluation> {
    let market_events = stored_events
        .iter()
        .map(|record| record.event.clone())
        .collect::<Vec<_>>();
    let snapshot = ReadOnlySnapshot::new(portfolio_state.clone(), market_events);
    let capabilities = VenueCapabilitySnapshot::new(spec.venue_capabilities.clone())?;
    let config = StrategyConfigSnapshot::from_config(config)?;
    let time = StrategyFixedTimeSource::from_rfc3339_z(fixed_time)?;
    let input = StrategyInput::new(snapshot, capabilities, config, time);
    let strategy = SpotPerpBasisStrategy::with_config(spec.strategy_config.clone())?;
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

struct BinanceGuardedLivePreviewArtifacts<'a> {
    candidate: &'a CandidatePortfolioTransition,
    risk_decision: &'a RiskDecision,
    plan_preview: &'a ExecutionPlan,
    plan_hash: &'a str,
    manual_material_md: &'a str,
    approval_records_jsonl: &'a str,
    confirmation_template_md: &'a str,
}

fn write_binance_guarded_live_preview_artifacts(
    output_dir: &Path,
    artifacts: BinanceGuardedLivePreviewArtifacts<'_>,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("candidate_transition.json"),
        &format!("{}\n", to_canonical_json(artifacts.candidate)),
    )?;
    write_utf8(
        output_dir.join("risk_decision.json"),
        &format!("{}\n", to_canonical_json(artifacts.risk_decision)),
    )?;
    write_utf8(
        output_dir.join("plan_preview.json"),
        &format!("{}\n", to_canonical_json(artifacts.plan_preview)),
    )?;
    write_utf8(
        output_dir.join("plan_hash.txt"),
        &format!("{}\n", artifacts.plan_hash),
    )?;
    write_utf8(
        output_dir.join("manual_approval_material.md"),
        artifacts.manual_material_md,
    )?;
    write_utf8(
        output_dir.join("approval_records.jsonl"),
        artifacts.approval_records_jsonl,
    )?;
    write_utf8(
        output_dir.join("manual_confirmation_record.md"),
        artifacts.confirmation_template_md,
    )?;
    Ok(())
}

fn load_binance_manual_gate_release_inputs(
    preview_dir: &Path,
) -> RuntimeResult<(PendingManualApprovalPlan, ManualApprovalRecord)> {
    let plan_path = preview_dir.join("plan_preview.json");
    let risk_path = preview_dir.join("risk_decision.json");
    let approval_path = preview_dir.join("approval_records.jsonl");
    let plan = from_json_strict::<ExecutionPlan>(&read_utf8(&plan_path)?)?;
    let risk_decision = from_json_strict::<RiskDecision>(&read_utf8(&risk_path)?)?;
    let plan_hash = execution_plan_hash(&plan);
    let approval_records = load_manual_approval_records_jsonl(&approval_path)?;
    let Some(approved_record) = approval_records.into_iter().find(|record| {
        record.plan_hash == plan_hash
            && record.status == ManualApprovalStatus::Approved
            && record.releases_manual_gate
    }) else {
        return Err(RuntimeError::UnsafeConfig {
            message: format!(
                "{} does not contain an Approved manual approval record for plan_hash {plan_hash}",
                approval_path.display()
            ),
        });
    };
    let pending = PendingManualApprovalPlan {
        plan_preview: plan,
        approval_material: ManualApprovalMaterial {
            risk_decision_id: risk_decision.decision_id.as_str().to_owned(),
            transition_id: risk_decision.transition_id.as_str().to_owned(),
            plan_id: approved_record.plan_id.clone(),
            plan_hash,
            reason_codes: risk_decision
                .reason_codes
                .iter()
                .map(|code| code.as_str().to_owned())
                .collect(),
            approval_requirement:
                "人工审批必须引用同一个 plan_hash；审批不能替代风控、账本、熔断或执行权限。"
                    .to_owned(),
        },
    };
    Ok((pending, approved_record))
}

fn load_manual_approval_records_jsonl(path: &Path) -> RuntimeResult<Vec<ManualApprovalRecord>> {
    let input = read_utf8(path)?;
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_manual_approval_record_json)
        .collect()
}

fn parse_manual_approval_record_json(line: &str) -> RuntimeResult<ManualApprovalRecord> {
    let decision = parse_manual_approval_decision(&json_string_field(line, "decision")?)?;
    let status = parse_manual_approval_status(&json_string_field(line, "status")?)?;
    Ok(ManualApprovalRecord {
        record_id: json_string_field(line, "record_id")?,
        approval_event_id: json_string_field(line, "approval_event_id")?,
        risk_decision_id: json_string_field(line, "risk_decision_id")?,
        transition_id: json_string_field(line, "transition_id")?,
        plan_id: json_string_field(line, "plan_id")?,
        plan_hash: json_string_field(line, "plan_hash")?,
        decision,
        status,
        reviewer_id: json_string_field(line, "reviewer_id")?,
        decided_at: json_string_field(line, "decided_at")?,
        expires_at: json_string_field(line, "expires_at")?,
        reason: json_optional_string_field(line, "reason")?,
        duplicate_of: json_optional_string_field(line, "duplicate_of")?,
        releases_manual_gate: json_bool_field(line, "releases_manual_gate")?,
        controlled_next_step: json_string_field(line, "controlled_next_step")?,
    })
}

fn parse_manual_approval_status(value: &str) -> RuntimeResult<ManualApprovalStatus> {
    match value {
        "Approved" => Ok(ManualApprovalStatus::Approved),
        "Rejected" => Ok(ManualApprovalStatus::Rejected),
        "Expired" => Ok(ManualApprovalStatus::Expired),
        "Duplicate" => Ok(ManualApprovalStatus::Duplicate),
        other => Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!("unknown manual approval status `{other}`"),
        }),
    }
}

fn write_binance_manual_gate_release_artifacts(
    output_dir: &Path,
    release: &ManualApprovalGateRelease,
    dispatchable_after_release: bool,
    generated_at: &str,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("manual_gate_release_preview.json"),
        &format!(
            "{}\n",
            manual_gate_release_preview_json(release, dispatchable_after_release, generated_at)
        ),
    )?;
    write_utf8(
        output_dir.join("manual_gate_release_preview.md"),
        &manual_gate_release_preview_markdown(release, dispatchable_after_release, generated_at),
    )
}

fn manual_gate_release_preview_json(
    release: &ManualApprovalGateRelease,
    dispatchable_after_release: bool,
    generated_at: &str,
) -> String {
    format!(
        "{{\"approval_event_id\":{},\"controlled_next_step\":{},\"dependent_transitions\":[{}],\"dispatchable_after_release\":{},\"gate_transition\":{},\"generated_at\":{},\"plan_hash\":{},\"plan_id\":{},\"released_manual_gate\":true,\"schema_version\":\"1.0.0\"}}",
        json_string(&release.approval_event_id),
        json_string(&release.controlled_next_step),
        release
            .dependent_transitions
            .iter()
            .map(execution_leg_transition_json)
            .collect::<Vec<_>>()
            .join(","),
        dispatchable_after_release,
        execution_leg_transition_json(&release.gate_transition),
        json_string(generated_at),
        json_string(&release.plan_hash),
        json_string(&release.plan_id),
    )
}

fn execution_leg_transition_json(transition: &arb_execution::ExecutionLegTransition) -> String {
    format!(
        "{{\"from_state\":{},\"idempotent\":{},\"plan_leg_id\":{},\"source_event_id\":{},\"to_state\":{}}}",
        json_string(transition.from_state.as_str()),
        transition.idempotent,
        json_string(&transition.plan_leg_id),
        json_string(&transition.source_event_id),
        json_string(transition.to_state.as_str()),
    )
}

fn manual_gate_release_preview_markdown(
    release: &ManualApprovalGateRelease,
    dispatchable_after_release: bool,
    generated_at: &str,
) -> String {
    format!(
        r#"# Binance BTCUSDT Manual Gate Release Preview

中文说明：本文件只记录人工审批门禁释放预览，不分发订单、不下单、不撤单、不转账、不签名。

- Generated at: {generated_at}
- Plan: {plan_id}
- Plan hash: {plan_hash}
- Approval event: {approval_event_id}
- Released manual gate: true
- Dependent transitions: {dependent_count}
- Dispatchable after release: {dispatchable_after_release}
- Controlled next step: {controlled_next_step}

## Gate Transition

- {gate_leg}: {gate_from} -> {gate_to}

## Dependent Transitions

{dependent_transitions}
"#,
        plan_id = release.plan_id,
        plan_hash = release.plan_hash,
        approval_event_id = release.approval_event_id,
        dependent_count = release.dependent_transitions.len(),
        controlled_next_step = release.controlled_next_step,
        gate_leg = release.gate_transition.plan_leg_id,
        gate_from = release.gate_transition.from_state.as_str(),
        gate_to = release.gate_transition.to_state.as_str(),
        dependent_transitions = release
            .dependent_transitions
            .iter()
            .map(|transition| format!(
                "- {}: {} -> {}",
                transition.plan_leg_id,
                transition.from_state.as_str(),
                transition.to_state.as_str()
            ))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn write_binance_pre_dispatch_dry_run_artifacts(
    output_dir: &Path,
    release_report: &BinanceManualGateReleasePreviewReport,
    health: &RuntimeHealthSnapshot,
    blocking_reasons: &[String],
    dispatch_allowed: bool,
    generated_at: &str,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("pre_dispatch_dry_run.json"),
        &format!(
            "{}\n",
            pre_dispatch_dry_run_json(
                release_report,
                health,
                blocking_reasons,
                dispatch_allowed,
                generated_at,
            )
        ),
    )?;
    write_utf8(
        output_dir.join("pre_dispatch_dry_run.md"),
        &pre_dispatch_dry_run_markdown(
            release_report,
            health,
            blocking_reasons,
            dispatch_allowed,
            generated_at,
        ),
    )
}

fn pre_dispatch_dry_run_json(
    release_report: &BinanceManualGateReleasePreviewReport,
    health: &RuntimeHealthSnapshot,
    blocking_reasons: &[String],
    dispatch_allowed: bool,
    generated_at: &str,
) -> String {
    format!(
        "{{\"approval_event_id\":{},\"blocking_reasons\":[{}],\"dispatch_allowed\":{},\"execution_mode\":{},\"generated_at\":{},\"kill_switch_triggered\":{},\"manual_gate_released\":{},\"mutable_execution_started\":{},\"plan_hash\":{},\"plan_id\":{},\"runtime_status\":{},\"schema_version\":\"1.0.0\"}}",
        json_string(&release_report.approval_event_id),
        blocking_reasons
            .iter()
            .map(|reason| json_string(reason))
            .collect::<Vec<_>>()
            .join(","),
        dispatch_allowed,
        json_string(&health.execution_mode),
        json_string(generated_at),
        health.kill_switch_triggered,
        release_report.released_manual_gate,
        health.mutable_execution_started,
        json_string(&release_report.plan_hash),
        json_string(&release_report.plan_id),
        json_string(health.status.as_str()),
    )
}

fn pre_dispatch_dry_run_markdown(
    release_report: &BinanceManualGateReleasePreviewReport,
    health: &RuntimeHealthSnapshot,
    blocking_reasons: &[String],
    dispatch_allowed: bool,
    generated_at: &str,
) -> String {
    format!(
        r#"# Binance BTCUSDT Pre-Dispatch Dry Run

中文说明：本 dry run 只检查分发前门禁，不调用真实交易 API、不下单、不撤单、不转账、不签名。

- Generated at: {generated_at}
- Plan: {plan_id}
- Plan hash: {plan_hash}
- Approval event: {approval_event_id}
- Manual gate released: {manual_gate_released}
- Runtime status: {runtime_status}
- Execution mode: {execution_mode}
- Kill switch triggered: {kill_switch_triggered}
- Mutable execution started: {mutable_execution_started}
- Dispatch allowed: {dispatch_allowed}

## Blocking Reasons

{blocking_reasons}
"#,
        plan_id = release_report.plan_id,
        plan_hash = release_report.plan_hash,
        approval_event_id = release_report.approval_event_id,
        manual_gate_released = release_report.released_manual_gate,
        runtime_status = health.status.as_str(),
        execution_mode = health.execution_mode,
        kill_switch_triggered = health.kill_switch_triggered,
        mutable_execution_started = health.mutable_execution_started,
        blocking_reasons = blocking_reasons
            .iter()
            .map(|reason| format!("- {reason}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(feature = "live-exec")]
struct BinancePrivateAccountSnapshot {
    balance_event_json: String,
    balances: Vec<VenueBalance>,
}

#[cfg(feature = "live-exec")]
fn live_dispatch_blocking_reasons(
    config: &arb_config::ArbConfig,
    plan: &ExecutionPlan,
    release: &ManualApprovalGateRelease,
    plan_hash: &str,
    health: &RuntimeHealthSnapshot,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if plan.execution_mode != ContractExecutionMode::ManualApproval {
        reasons.push(format!(
            "受控分发只消费 ManualApproval 计划预览，收到 `{}`",
            plan.execution_mode.as_str()
        ));
    }
    if release.plan_hash != plan_hash {
        reasons.push("人工门禁释放哈希与当前计划哈希不一致".to_owned());
    }
    if config.execution().mode() != ConfigExecutionMode::GuardedLive {
        reasons.push(format!(
            "配置执行模式必须是 GuardedLive，当前为 `{}`",
            config.execution().mode()
        ));
    }
    if !config.execution().live_execution_enabled() {
        reasons.push("execution.live_execution_enabled 必须为 true".to_owned());
    }
    if config.execution().auto_live_enabled() {
        reasons.push("GuardedLive 受控入口不允许 auto_live_enabled=true".to_owned());
    }
    if !config.signing().real_signing_enabled() {
        reasons.push("signing.real_signing_enabled 必须为 true".to_owned());
    }
    if config.kill_switch().is_triggered() {
        reasons.push("任意熔断范围仍处于打开状态，拒绝实盘分发".to_owned());
    }
    if !health.mutable_execution_started {
        reasons.push("运行时健康快照显示可变执行任务未启动".to_owned());
    }
    if !release.gate_transition.to_state.as_str().eq("Ready") {
        reasons.push("人工审批门禁未释放到 Ready 状态".to_owned());
    }
    reasons
}

#[cfg(feature = "live-exec")]
fn live_dispatch_policy_from_config(
    config: &arb_config::ArbConfig,
) -> RuntimeResult<ExecutionDispatchPolicy> {
    let cap = Amount::new(Decimal::from_str(BINANCE_GUARDED_LIVE_CAPITAL_LIMIT_USDT)?)?;
    let mut kill_switch = if config
        .kill_switch()
        .blocks_execution_mode(ConfigExecutionMode::GuardedLive)
    {
        DispatchKillSwitch::global()
    } else {
        DispatchKillSwitch::default()
    };
    for venue in config.kill_switch().venues() {
        kill_switch = kill_switch.with_venue(VenueId::new(venue)?);
    }
    for account in config.kill_switch().accounts() {
        kill_switch = kill_switch.with_account(AccountId::new(account)?);
    }
    for instrument in config.kill_switch().instruments() {
        kill_switch = kill_switch.with_instrument(InstrumentId::new(instrument)?);
    }
    Ok(ExecutionDispatchPolicy::new(cap)
        .allow_symbol(BASIS_SYMBOL)?
        .with_manual_gate_released(true)
        .with_kill_switch(kill_switch))
}

#[cfg(feature = "live-exec")]
fn signing_policy_from_config(config: &arb_config::ArbConfig) -> RuntimeResult<SigningPolicy> {
    Ok(SigningPolicy::real_signing_enabled(SigningPolicyRef::new(
        config.signing().policy_ref().as_str(),
    )?))
}

#[cfg(feature = "live-exec")]
fn build_binance_spot_live_adapter(
    signing_policy: &SigningPolicy,
) -> RuntimeResult<
    live::BinanceSpotExecAdapter<RealSigningProviderFromEnv, live::BinanceCurlExecTransport>,
> {
    Ok(live::BinanceSpotExecAdapter::new(
        live::BinanceExecConfig::spot(
            VenueId::new(BASIS_SPOT_VENUE_ID)?,
            AccountId::new(BINANCE_GUARDED_LIVE_ACCOUNT_REF)?,
            "https://api.binance.com",
            signing_policy.clone(),
        )?,
        RealSigningProviderFromEnv::from_default_env()?,
        live::BinanceCurlExecTransport::default(),
    )?)
}

#[cfg(feature = "live-exec")]
fn build_binance_usdm_live_adapter(
    signing_policy: &SigningPolicy,
) -> RuntimeResult<
    live::BinanceUsdmExecAdapter<RealSigningProviderFromEnv, live::BinanceCurlExecTransport>,
> {
    Ok(live::BinanceUsdmExecAdapter::new(
        live::BinanceExecConfig::usdm_futures(
            VenueId::new(BASIS_PERP_VENUE_ID)?,
            AccountId::new(BINANCE_GUARDED_LIVE_ACCOUNT_REF)?,
            BINANCE_USDM_FUTURES_BASE_URL,
            signing_policy.clone(),
        )?,
        RealSigningProviderFromEnv::from_default_env()?,
        live::BinanceCurlExecTransport::default(),
    )?)
}

#[cfg(feature = "live-exec")]
#[allow(clippy::too_many_arguments)]
fn fetch_binance_private_account_snapshot(
    signing_policy: &SigningPolicy,
    ingested_at: &UtcTimestamp,
    scope: &str,
    market: BinancePrivateAccountMarket,
    venue_id: &str,
    base_url: &str,
    endpoint: &str,
) -> RuntimeResult<BinancePrivateAccountSnapshot> {
    let signer = RealSigningProviderFromEnv::from_default_env()?;
    let signed = signer.sign_binance_hmac(
        BinanceHmacSigningInput::new(
            SigningRequestId::new(format!("signing-request/binance-live/account/{scope}"))?,
            signing_policy.policy_ref().clone(),
            SigningPurpose::QueryAccount,
            VenueId::new(venue_id)?,
            AccountId::new(BINANCE_GUARDED_LIVE_ACCOUNT_REF)?,
            vec![BinanceRequestParam::new(
                "recvWindow",
                live::DEFAULT_BINANCE_RECV_WINDOW_MS.to_string(),
            )?],
        )?,
        signing_policy,
    )?;
    let raw_json = fetch_signed_binance_get_with_curl(
        base_url,
        endpoint,
        signed.api_key_header_name(),
        signed.api_key_header_value(),
        signed.signed_query_for_transport(),
    )?;
    let mut adapter = BinancePrivateAccountAdapter::new(
        VenueId::new(venue_id)?,
        AccountId::new(BINANCE_GUARDED_LIVE_ACCOUNT_REF)?,
        market,
        *ingested_at,
        MARKET_DATA_MAX_AGE_MS,
    )?;
    let raw_ref = format!("binance-private-account:{scope}:{endpoint}");
    let batch = match market {
        BinancePrivateAccountMarket::Spot => {
            adapter.ingest_spot_account_json(&raw_json, raw_ref, *ingested_at)?
        }
        BinancePrivateAccountMarket::UsdmFutures => {
            adapter.ingest_usdm_account_json(&raw_json, raw_ref, *ingested_at)?
        }
    };
    let balances = adapter.balances(
        &BalanceQuery::new(VenueId::new(venue_id)?)
            .for_account(AccountId::new(BINANCE_GUARDED_LIVE_ACCOUNT_REF)?)
            .for_asset(AssetId::new(BASIS_QUOTE_ASSET_ID)?),
    )?;
    Ok(BinancePrivateAccountSnapshot {
        balance_event_json: to_canonical_json(&batch.balance_event),
        balances,
    })
}

#[cfg(feature = "live-exec")]
fn fetch_binance_spot_private_account_snapshot(
    signing_policy: &SigningPolicy,
    ingested_at: &UtcTimestamp,
    scope: &str,
) -> RuntimeResult<BinancePrivateAccountSnapshot> {
    fetch_binance_private_account_snapshot(
        signing_policy,
        ingested_at,
        scope,
        BinancePrivateAccountMarket::Spot,
        BASIS_SPOT_VENUE_ID,
        "https://api.binance.com",
        "/api/v3/account",
    )
}

#[cfg(feature = "live-exec")]
fn ensure_private_balance_covers_plan(
    plan: &ExecutionPlan,
    balances: &[VenueBalance],
) -> RuntimeResult<()> {
    let required_text = plan
        .legs
        .iter()
        .find(|leg| {
            leg.venue_id
                .as_ref()
                .is_some_and(|venue| venue.as_str() == BASIS_SPOT_VENUE_ID)
        })
        .and_then(|leg| leg.notional_usd.as_ref())
        .map(|value| value.as_str())
        .unwrap_or(BINANCE_GUARDED_LIVE_NOTIONAL_USDT);
    ensure_asset_balance_covers_amount(
        "Binance 私有账户",
        balances,
        BASIS_QUOTE_ASSET_ID,
        required_text,
    )
}

#[cfg(feature = "live-exec")]
fn ensure_asset_balance_covers_amount(
    scope: &str,
    balances: &[VenueBalance],
    asset_id: &str,
    required_text: &str,
) -> RuntimeResult<()> {
    let required = Amount::new(Decimal::from_str(required_text)?)?;
    let Some(balance) = balances
        .iter()
        .find(|balance| balance.asset_id.as_str() == asset_id)
    else {
        return Err(RuntimeError::UnsafeConfig {
            message: format!("{scope} 未返回 {asset_id} 可用余额"),
        });
    };
    if balance.free < required {
        return Err(RuntimeError::UnsafeConfig {
            message: format!(
                "{scope} {asset_id} 可用余额不足：free={} required={}",
                balance.free, required
            ),
        });
    }
    Ok(())
}

#[cfg(feature = "live-exec")]
fn confirm_live_receipt_with_order_query(
    adapter: &mut live::BinanceSpotExecAdapter<
        RealSigningProviderFromEnv,
        live::BinanceCurlExecTransport,
    >,
    planned: &arb_venue_exec::PlannedSubmitOrder,
    receipt: &MutableActionReceipt,
) -> RuntimeResult<PrivateOrderUpdate> {
    if receipt.kind != MutableActionKind::SubmitOrder {
        return Err(RuntimeError::UnsafeConfig {
            message: "只允许确认 SubmitOrder 回执".to_owned(),
        });
    }
    let order_ref = match &receipt.external_ref {
        Some(ExternalActionRef::Order(order_id)) => OrderReference::VenueOrderId(order_id.clone()),
        _ => planned
            .request
            .client_order_id
            .clone()
            .map(OrderReference::ClientOrderId)
            .ok_or_else(|| RuntimeError::UnsafeConfig {
                message: "下单回执缺少外部订单号和 client_order_id，无法执行私有查单确认"
                    .to_owned(),
            })?,
    };
    adapter
        .confirm_order_status(ConfirmOrderStatusRequest::new(
            planned.request.venue_id.clone(),
            planned.request.account_id.clone(),
            planned.request.instrument_id.clone(),
            order_ref,
            format!("event:binance:order-query:{}", planned.plan_leg_id),
        ))
        .map_err(RuntimeError::from)
}

#[cfg(feature = "live-exec")]
fn private_confirmation_from_update(
    plan_leg_id: &str,
    update: &PrivateOrderUpdate,
) -> arb_execution::PrivateOrderConfirmation {
    let mut confirmation = arb_execution::PrivateOrderConfirmation::new(
        plan_leg_id,
        private_confirmation_status(update.status),
        private_confirmation_source(update.source),
        update.source_event_id.clone(),
    );
    if let Some(order_id) = &update.venue_order_id {
        confirmation = confirmation.with_venue_order_id(order_id.as_str());
    }
    if let Some(client_order_id) = &update.client_order_id {
        confirmation = confirmation.with_client_order_id(client_order_id.as_str());
    }
    if let Some(fill) = update
        .last_fill
        .as_ref()
        .and_then(|fill| private_execution_fill_from_update(update, fill))
    {
        confirmation = confirmation.with_fill(fill);
    } else if matches!(
        update.status,
        OrderConfirmationStatus::Filled | OrderConfirmationStatus::PartiallyFilled
    ) {
        confirmation = confirmation.with_detail(
            "私有查单返回成交状态但缺少手续费明细；执行报告保持失败闭合，等待 user data stream 或更完整成交明细。",
        );
    }
    confirmation
}

#[cfg(feature = "live-exec")]
fn private_confirmation_status(
    status: OrderConfirmationStatus,
) -> arb_execution::PrivateOrderConfirmationStatus {
    match status {
        OrderConfirmationStatus::Acknowledged => {
            arb_execution::PrivateOrderConfirmationStatus::Acknowledged
        }
        OrderConfirmationStatus::Filled => arb_execution::PrivateOrderConfirmationStatus::Filled,
        OrderConfirmationStatus::PartiallyFilled => {
            arb_execution::PrivateOrderConfirmationStatus::PartiallyFilled
        }
        OrderConfirmationStatus::Cancelled => {
            arb_execution::PrivateOrderConfirmationStatus::Cancelled
        }
        OrderConfirmationStatus::Rejected => {
            arb_execution::PrivateOrderConfirmationStatus::Rejected
        }
        OrderConfirmationStatus::Expired => arb_execution::PrivateOrderConfirmationStatus::Expired,
        OrderConfirmationStatus::Unknown => arb_execution::PrivateOrderConfirmationStatus::Unknown,
    }
}

#[cfg(feature = "live-exec")]
fn private_confirmation_source(
    source: OrderConfirmationSource,
) -> arb_execution::PrivateOrderConfirmationSource {
    match source {
        OrderConfirmationSource::PrivateStream => {
            arb_execution::PrivateOrderConfirmationSource::PrivateStream
        }
        OrderConfirmationSource::OrderQuery => {
            arb_execution::PrivateOrderConfirmationSource::OrderQuery
        }
    }
}

#[cfg(feature = "live-exec")]
fn private_execution_fill_from_update(
    update: &PrivateOrderUpdate,
    fill: &PrivateOrderFillUpdate,
) -> Option<arb_execution::PrivateExecutionFill> {
    let side = match update.side? {
        arb_venue_exec::OrderSide::Buy => arb_contracts::FillSide::Buy,
        arb_venue_exec::OrderSide::Sell => arb_contracts::FillSide::Sell,
    };
    let fee_asset_id = fill.fee_asset_id.as_ref()?.as_str().to_owned();
    let fee_amount = fill.fee_amount.as_ref()?.to_string();
    let mut execution_fill = arb_execution::PrivateExecutionFill::new(
        side,
        fill.price.clone(),
        fill.quantity.clone(),
        fee_asset_id,
        fee_amount,
        fill.source_event_id.clone(),
    )
    .with_timestamp(fill.timestamp.clone());
    if let Some(order_id) = &update.venue_order_id {
        execution_fill = execution_fill.with_venue_order_id(order_id.as_str());
    }
    if let Some(client_order_id) = &update.client_order_id {
        execution_fill = execution_fill.with_client_order_id(client_order_id.as_str());
    }
    Some(execution_fill)
}

#[cfg(feature = "live-exec")]
fn fetch_signed_binance_get_with_curl(
    base_url: &str,
    endpoint: &str,
    header_name: &str,
    header_value: &str,
    signed_query: &str,
) -> RuntimeResult<String> {
    let url = format!("{base_url}{endpoint}?{signed_query}");
    let header = format!("{header_name}: {header_value}");
    let config = format!(
        "url = \"{}\"\nheader = \"{}\"\n",
        curl_config_quote_runtime(&url)?,
        curl_config_quote_runtime(&header)?
    );
    let mut child = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("--request")
        .arg("GET")
        .arg("--connect-timeout")
        .arg("10")
        .arg("--max-time")
        .arg("30")
        .arg("--write-out")
        .arg("\n__ARB_BINANCE_HTTP_STATUS__:%{http_code}")
        .arg("--config")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| RuntimeError::LiveMarketData {
            message: format!("cannot start curl for Binance private signed GET: {error}"),
        })?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| RuntimeError::LiveMarketData {
            message: "curl stdin unavailable for Binance private signed GET".to_owned(),
        })?
        .write_all(config.as_bytes())
        .map_err(|error| RuntimeError::LiveMarketData {
            message: format!("cannot write curl config for Binance private signed GET: {error}"),
        })?;
    let output = child
        .wait_with_output()
        .map_err(|error| RuntimeError::LiveMarketData {
            message: format!("curl failed for Binance private signed GET: {error}"),
        })?;
    if !output.status.success() {
        return Err(RuntimeError::LiveMarketData {
            message: "curl failed before a reliable Binance private HTTP response was available"
                .to_owned(),
        });
    }
    let rendered = String::from_utf8_lossy(&output.stdout);
    let Some((body, status)) = rendered.rsplit_once("\n__ARB_BINANCE_HTTP_STATUS__:") else {
        return Err(RuntimeError::LiveMarketData {
            message: "Binance private signed GET lacked HTTP status marker".to_owned(),
        });
    };
    let status_code = status
        .trim()
        .parse::<u16>()
        .map_err(|_| RuntimeError::LiveMarketData {
            message: "Binance private signed GET returned malformed HTTP status".to_owned(),
        })?;
    if !(200..=299).contains(&status_code) {
        return Err(RuntimeError::LiveMarketData {
            message: format!(
                "Binance private signed GET returned HTTP {status_code}: {}",
                response_snippet(body)
            ),
        });
    }
    Ok(body.to_owned())
}

#[cfg(feature = "live-exec")]
fn curl_config_quote_runtime(value: &str) -> RuntimeResult<String> {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'\\' => escaped.push_str("\\\\"),
            b'"' => escaped.push_str("\\\""),
            0 | b'\n' | b'\r' => {
                return Err(RuntimeError::UnsafeConfig {
                    message: "curl config value contains unsupported control bytes".to_owned(),
                });
            }
            byte if byte.is_ascii_control() => {
                return Err(RuntimeError::UnsafeConfig {
                    message: "curl config value contains unsupported control bytes".to_owned(),
                });
            }
            _ => escaped.push(byte as char),
        }
    }
    Ok(escaped)
}

#[cfg(feature = "live-exec")]
fn response_snippet(body: &str) -> String {
    const MAX_LEN: usize = 256;
    if body.len() <= MAX_LEN {
        body.to_owned()
    } else {
        format!("{}...", &body[..MAX_LEN])
    }
}

fn write_binance_guarded_live_auto_once_artifacts(
    output_dir: &Path,
    report: &BinanceGuardedLiveAutoOnceReport,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("auto_once_report.json"),
        &format!("{}\n", auto_once_report_json(report)),
    )?;
    write_utf8(
        output_dir.join("auto_once_report.md"),
        &auto_once_report_markdown(report),
    )
}

fn auto_once_report_json(report: &BinanceGuardedLiveAutoOnceReport) -> String {
    format!(
        "{{\"approval_event_id\":{},\"best_ask\":{},\"blocking_reasons\":[{}],\"dispatch_allowed\":{},\"dispatch_attempted\":{},\"execution_report_status\":{},\"manual_gate_released\":{},\"max_ask\":{},\"plan_hash\":{},\"private_confirmation_count\":{},\"schema_version\":\"1.0.0\",\"signal_allowed\":{},\"source_event_id\":{},\"strategy_id\":{},\"submitted_receipt_count\":{},\"symbol\":{}}}",
        optional_json_string_universal(report.approval_event_id.as_deref()),
        optional_json_string_universal(report.best_ask.as_deref()),
        report
            .blocking_reasons
            .iter()
            .map(|reason| json_string(reason))
            .collect::<Vec<_>>()
            .join(","),
        report.dispatch_allowed,
        report.dispatch_attempted,
        optional_json_string_universal(report.execution_report_status.as_deref()),
        report.manual_gate_released,
        optional_json_string_universal(report.max_ask.as_deref()),
        optional_json_string_universal(report.plan_hash.as_deref()),
        report.private_confirmation_count,
        report.signal_allowed,
        optional_json_string_universal(report.source_event_id.as_deref()),
        json_string(&report.strategy_id),
        report.submitted_receipt_count,
        json_string(&report.symbol),
    )
}

fn auto_once_report_markdown(report: &BinanceGuardedLiveAutoOnceReport) -> String {
    let reasons = if report.blocking_reasons.is_empty() {
        "- none".to_owned()
    } else {
        report
            .blocking_reasons
            .iter()
            .map(|reason| format!("- {reason}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        r#"# Binance BTCUSDT Auto Once

中文说明：本文件记录从最新公开行情到策略信号、审批事实和执行门禁的一次自动链路结果。密钥、签名 query 和原始私有账户响应不会写入本文件。

- Symbol: {symbol}
- Strategy: {strategy_id}
- Source event: {source_event}
- Best ask: {best_ask}
- Max ask: {max_ask}
- Signal allowed: {signal_allowed}
- Plan hash: {plan_hash}
- Approval event: {approval_event}
- Manual gate released: {manual_gate_released}
- Dispatch attempted: {dispatch_attempted}
- Dispatch allowed: {dispatch_allowed}
- Submitted receipts: {submitted_receipt_count}
- Private confirmations: {private_confirmation_count}
- Execution report status: {execution_report_status}

## Blocking Reasons

{reasons}
"#,
        symbol = report.symbol,
        strategy_id = report.strategy_id,
        source_event = report.source_event_id.as_deref().unwrap_or("none"),
        best_ask = report.best_ask.as_deref().unwrap_or("none"),
        max_ask = report.max_ask.as_deref().unwrap_or("none"),
        signal_allowed = report.signal_allowed,
        plan_hash = report.plan_hash.as_deref().unwrap_or("none"),
        approval_event = report.approval_event_id.as_deref().unwrap_or("none"),
        manual_gate_released = report.manual_gate_released,
        dispatch_attempted = report.dispatch_attempted,
        dispatch_allowed = report.dispatch_allowed,
        submitted_receipt_count = report.submitted_receipt_count,
        private_confirmation_count = report.private_confirmation_count,
        execution_report_status = report.execution_report_status.as_deref().unwrap_or("none"),
        reasons = reasons,
    )
}

fn optional_json_string_universal(value: Option<&str>) -> String {
    value.map(json_string).unwrap_or_else(|| "null".to_owned())
}

#[cfg(feature = "live-exec")]
fn write_binance_basis_guarded_live_auto_once_artifacts(
    output_dir: &Path,
    report: &BinanceBasisGuardedLiveAutoOnceReport,
    receipts: &[MutableActionReceipt],
    confirmations: &[arb_execution::PrivateOrderConfirmation],
    execution_report: Option<&ExecutionReport>,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("basis_auto_once_report.json"),
        &format!("{}\n", basis_auto_once_report_json(report)),
    )?;
    write_utf8(
        output_dir.join("basis_auto_once_report.md"),
        &basis_auto_once_report_markdown(report),
    )?;
    write_utf8(
        output_dir.join("mutable_receipts.jsonl"),
        &mutable_receipts_jsonl(receipts),
    )?;
    write_utf8(
        output_dir.join("private_confirmations.jsonl"),
        &private_confirmations_jsonl(confirmations),
    )?;
    write_utf8(
        output_dir.join("execution_reports.jsonl"),
        &execution_report
            .map(|report| canonical_jsonl(std::slice::from_ref(report)))
            .unwrap_or_default(),
    )
}

#[cfg(feature = "live-exec")]
fn basis_auto_once_report_json(report: &BinanceBasisGuardedLiveAutoOnceReport) -> String {
    format!(
        "{{\"approval_event_id\":{},\"blocking_reasons\":[{}],\"dispatch_allowed\":{},\"dispatch_attempted\":{},\"execution_report_status\":{},\"manual_gate_released\":{},\"net_bps\":{},\"perp_bid\":{},\"perp_event_id\":{},\"planned_order_count\":{},\"plan_hash\":{},\"premium_event_id\":{},\"private_confirmation_count\":{},\"protection_actions\":[{}],\"protection_attempted\":{},\"protection_receipt_count\":{},\"residual_risk\":{},\"schema_version\":\"1.0.0\",\"signal_allowed\":{},\"spot_ask\":{},\"spot_event_id\":{},\"strategy_id\":{},\"submitted_receipt_count\":{},\"symbol\":{}}}",
        optional_json_string_universal(report.approval_event_id.as_deref()),
        report
            .blocking_reasons
            .iter()
            .map(|reason| json_string(reason))
            .collect::<Vec<_>>()
            .join(","),
        report.dispatch_allowed,
        report.dispatch_attempted,
        optional_json_string_universal(report.execution_report_status.as_deref()),
        report.manual_gate_released,
        report
            .net_bps
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_owned()),
        optional_json_string_universal(report.perp_bid.as_deref()),
        optional_json_string_universal(report.perp_event_id.as_deref()),
        report.planned_order_count,
        optional_json_string_universal(report.plan_hash.as_deref()),
        optional_json_string_universal(report.premium_event_id.as_deref()),
        report.private_confirmation_count,
        report
            .protection_actions
            .iter()
            .map(|action| json_string(action))
            .collect::<Vec<_>>()
            .join(","),
        report.protection_attempted,
        report.protection_receipt_count,
        optional_json_string_universal(report.residual_risk.as_deref()),
        report.signal_allowed,
        optional_json_string_universal(report.spot_ask.as_deref()),
        optional_json_string_universal(report.spot_event_id.as_deref()),
        json_string(&report.strategy_id),
        report.submitted_receipt_count,
        json_string(&report.symbol),
    )
}

#[cfg(feature = "live-exec")]
fn basis_auto_once_report_markdown(report: &BinanceBasisGuardedLiveAutoOnceReport) -> String {
    let reasons = if report.blocking_reasons.is_empty() {
        "- none".to_owned()
    } else {
        report
            .blocking_reasons
            .iter()
            .map(|reason| format!("- {reason}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let protection_actions = if report.protection_actions.is_empty() {
        "- none".to_owned()
    } else {
        report
            .protection_actions
            .iter()
            .map(|action| format!("- {action}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        r#"# Binance Basis GuardedLive Auto Once

中文说明：本文件记录 Binance spot-perp basis 双腿自动链路结果。套利候选必须同时包含 spot buy 和 USD-M perp short，两腿下单需要显式实盘确认。

- Symbol: {symbol}
- Strategy: {strategy_id}
- Spot ask: {spot_ask}
- Perp bid: {perp_bid}
- Net bps: {net_bps}
- Signal allowed: {signal_allowed}
- Plan hash: {plan_hash}
- Approval event: {approval_event}
- Manual gate released: {manual_gate_released}
- Planned orders: {planned_order_count}
- Dispatch attempted: {dispatch_attempted}
- Dispatch allowed: {dispatch_allowed}
- Submitted receipts: {submitted_receipt_count}
- Private confirmations: {private_confirmation_count}
- Protection attempted: {protection_attempted}
- Protection receipts: {protection_receipt_count}
- Residual risk: {residual_risk}
- Execution report status: {execution_report_status}

## Protection Actions

{protection_actions}

## Blocking Reasons

{reasons}
"#,
        symbol = report.symbol,
        strategy_id = report.strategy_id,
        spot_ask = report.spot_ask.as_deref().unwrap_or("none"),
        perp_bid = report.perp_bid.as_deref().unwrap_or("none"),
        net_bps = report
            .net_bps
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_owned()),
        signal_allowed = report.signal_allowed,
        plan_hash = report.plan_hash.as_deref().unwrap_or("none"),
        approval_event = report.approval_event_id.as_deref().unwrap_or("none"),
        manual_gate_released = report.manual_gate_released,
        planned_order_count = report.planned_order_count,
        dispatch_attempted = report.dispatch_attempted,
        dispatch_allowed = report.dispatch_allowed,
        submitted_receipt_count = report.submitted_receipt_count,
        private_confirmation_count = report.private_confirmation_count,
        protection_attempted = report.protection_attempted,
        protection_receipt_count = report.protection_receipt_count,
        residual_risk = report.residual_risk.as_deref().unwrap_or("none"),
        execution_report_status = report.execution_report_status.as_deref().unwrap_or("none"),
        protection_actions = protection_actions,
        reasons = reasons,
    )
}

#[cfg(feature = "live-exec")]
fn write_binance_guarded_live_dispatch_artifacts(
    output_dir: &Path,
    report: &BinanceGuardedLiveDispatchReport,
    receipts: &[MutableActionReceipt],
    confirmations: &[arb_execution::PrivateOrderConfirmation],
    execution_report: Option<&ExecutionReport>,
    pre_account_event: Option<&str>,
    post_account_event: Option<&str>,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("live_dispatch_report.json"),
        &format!("{}\n", live_dispatch_report_json(report)),
    )?;
    write_utf8(
        output_dir.join("live_dispatch_report.md"),
        &live_dispatch_report_markdown(report),
    )?;
    write_utf8(
        output_dir.join("mutable_receipts.jsonl"),
        &mutable_receipts_jsonl(receipts),
    )?;
    write_utf8(
        output_dir.join("private_confirmations.jsonl"),
        &private_confirmations_jsonl(confirmations),
    )?;
    write_utf8(
        output_dir.join("execution_reports.jsonl"),
        &execution_report
            .map(|report| canonical_jsonl(std::slice::from_ref(report)))
            .unwrap_or_default(),
    )?;
    write_utf8(
        output_dir.join("private_account_events.jsonl"),
        &private_account_events_jsonl(pre_account_event, post_account_event),
    )
}

#[cfg(feature = "live-exec")]
fn live_dispatch_report_json(report: &BinanceGuardedLiveDispatchReport) -> String {
    format!(
        "{{\"approval_event_id\":{},\"blocking_reasons\":[{}],\"dispatch_allowed\":{},\"execution_report_status\":{},\"plan_hash\":{},\"plan_id\":{},\"post_account_balance_event_id\":{},\"pre_account_balance_event_id\":{},\"private_confirmation_count\":{},\"schema_version\":\"1.0.0\",\"submitted_receipt_count\":{}}}",
        json_string(&report.approval_event_id),
        report
            .blocking_reasons
            .iter()
            .map(|reason| json_string(reason))
            .collect::<Vec<_>>()
            .join(","),
        report.dispatch_allowed,
        optional_json_string(report.execution_report_status.as_deref()),
        json_string(&report.plan_hash),
        json_string(&report.plan_id),
        optional_json_string(report.post_account_balance_event_id.as_deref()),
        optional_json_string(report.pre_account_balance_event_id.as_deref()),
        report.private_confirmation_count,
        report.submitted_receipt_count,
    )
}

#[cfg(feature = "live-exec")]
fn live_dispatch_report_markdown(report: &BinanceGuardedLiveDispatchReport) -> String {
    format!(
        r#"# Binance BTCUSDT GuardedLive Dispatch

中文说明：本文件记录受控实盘分发结果。API key、secret、签名 query 和原始私有账户响应不会写入本文件。

- Plan: {plan_id}
- Plan hash: {plan_hash}
- Approval event: {approval_event_id}
- Dispatch allowed: {dispatch_allowed}
- Submitted receipts: {submitted_receipt_count}
- Private confirmations: {private_confirmation_count}
- Execution report status: {execution_report_status}
- Pre account balance event: {pre_event}
- Post account balance event: {post_event}

## Blocking Reasons

{blocking_reasons}
"#,
        plan_id = report.plan_id,
        plan_hash = report.plan_hash,
        approval_event_id = report.approval_event_id,
        dispatch_allowed = report.dispatch_allowed,
        submitted_receipt_count = report.submitted_receipt_count,
        private_confirmation_count = report.private_confirmation_count,
        execution_report_status = report.execution_report_status.as_deref().unwrap_or("none"),
        pre_event = report
            .pre_account_balance_event_id
            .as_deref()
            .unwrap_or("none"),
        post_event = report
            .post_account_balance_event_id
            .as_deref()
            .unwrap_or("none"),
        blocking_reasons = if report.blocking_reasons.is_empty() {
            "- none".to_owned()
        } else {
            report
                .blocking_reasons
                .iter()
                .map(|reason| format!("- {reason}"))
                .collect::<Vec<_>>()
                .join("\n")
        },
    )
}

#[cfg(feature = "live-exec")]
fn mutable_receipts_jsonl(receipts: &[MutableActionReceipt]) -> String {
    receipts
        .iter()
        .map(mutable_receipt_json)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(feature = "live-exec")]
fn mutable_receipt_json(receipt: &MutableActionReceipt) -> String {
    format!(
        "{{\"action_id\":{},\"duplicate\":{},\"external_ref\":{},\"idempotency_key\":{},\"kind\":{},\"simulated\":{},\"status\":{},\"venue_id\":{}}}",
        json_string(receipt.action_id.as_str()),
        receipt.duplicate,
        receipt
            .external_ref
            .as_ref()
            .map(external_action_ref_json)
            .unwrap_or_else(|| "null".to_owned()),
        json_string(receipt.idempotency_key.as_str()),
        json_string(receipt.kind.as_str()),
        receipt.simulated,
        json_string(receipt.status.as_str()),
        json_string(receipt.venue_id.as_str()),
    )
}

#[cfg(feature = "live-exec")]
fn external_action_ref_json(ref_value: &ExternalActionRef) -> String {
    match ref_value {
        ExternalActionRef::Order(order_id) => {
            format!(
                "{{\"kind\":\"order\",\"value\":{}}}",
                json_string(order_id.as_str())
            )
        }
        ExternalActionRef::Cancel(action_id) => format!(
            "{{\"kind\":\"cancel\",\"value\":{}}}",
            json_string(action_id.as_str())
        ),
        ExternalActionRef::Transfer(transfer_id) => format!(
            "{{\"kind\":\"transfer\",\"value\":{}}}",
            json_string(transfer_id.as_str())
        ),
    }
}

#[cfg(feature = "live-exec")]
fn private_confirmations_jsonl(
    confirmations: &[arb_execution::PrivateOrderConfirmation],
) -> String {
    confirmations
        .iter()
        .map(private_confirmation_json)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(feature = "live-exec")]
fn private_confirmation_json(confirmation: &arb_execution::PrivateOrderConfirmation) -> String {
    format!(
        "{{\"client_order_id\":{},\"detail\":{},\"fill_count\":{},\"plan_leg_id\":{},\"source\":{},\"source_event_refs\":[{}],\"status\":{},\"venue_order_id\":{}}}",
        optional_json_string(confirmation.client_order_id.as_deref()),
        optional_json_string(confirmation.detail.as_deref()),
        confirmation.fills.len(),
        json_string(&confirmation.plan_leg_id),
        json_string(confirmation.source.as_str()),
        confirmation
            .source_event_refs
            .iter()
            .map(|value| json_string(value))
            .collect::<Vec<_>>()
            .join(","),
        json_string(confirmation.status.as_str()),
        optional_json_string(confirmation.venue_order_id.as_deref()),
    )
}

#[cfg(feature = "live-exec")]
fn private_account_events_jsonl(pre: Option<&str>, post: Option<&str>) -> String {
    [pre, post]
        .into_iter()
        .flatten()
        .map(str::to_owned)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(feature = "live-exec")]
fn optional_json_string(value: Option<&str>) -> String {
    value.map(json_string).unwrap_or_else(|| "null".to_owned())
}

fn json_string_field(input: &str, field: &str) -> RuntimeResult<String> {
    let value_start = json_field_value_start(input, field)?;
    let quote_start = value_start
        + input[value_start..]
            .find('"')
            .ok_or_else(|| RuntimeError::Module {
                module: "arb-runtime",
                message: format!("JSON field `{field}` is not a string"),
            })?;
    if input[value_start..quote_start].trim().is_empty() {
        let quote_end = json_string_end(input, quote_start)?;
        decode_json_string_literal(&input[quote_start + 1..quote_end - 1])
    } else {
        Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!("JSON field `{field}` is not a string"),
        })
    }
}

fn json_optional_string_field(input: &str, field: &str) -> RuntimeResult<Option<String>> {
    if !input.contains(&format!("\"{field}\"")) {
        return Ok(None);
    }
    let value_start = json_field_value_start(input, field)?;
    if input[value_start..].trim_start().starts_with("null") {
        Ok(None)
    } else {
        json_string_field(input, field).map(Some)
    }
}

fn json_bool_field(input: &str, field: &str) -> RuntimeResult<bool> {
    let value_start = json_field_value_start(input, field)?;
    let value = input[value_start..].trim_start();
    if value.starts_with("true") {
        Ok(true)
    } else if value.starts_with("false") {
        Ok(false)
    } else {
        Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!("JSON field `{field}` is not a boolean"),
        })
    }
}

fn json_field_value_start(input: &str, field: &str) -> RuntimeResult<usize> {
    let key = format!("\"{field}\"");
    let key_start = input.find(&key).ok_or_else(|| RuntimeError::Module {
        module: "arb-runtime",
        message: format!("JSON object is missing field `{field}`"),
    })?;
    let after_key = key_start + key.len();
    let colon = after_key
        + input[after_key..]
            .find(':')
            .ok_or_else(|| RuntimeError::Module {
                module: "arb-runtime",
                message: format!("JSON field `{field}` is missing ':'"),
            })?;
    Ok(colon + 1)
}

fn decode_json_string_literal(input: &str) -> RuntimeResult<String> {
    let mut output = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            return Err(RuntimeError::Module {
                module: "arb-runtime",
                message: "JSON string ends with a dangling escape".to_owned(),
            });
        };
        match escaped {
            '"' => output.push('"'),
            '\\' => output.push('\\'),
            '/' => output.push('/'),
            'b' => output.push('\u{0008}'),
            'f' => output.push('\u{000c}'),
            'n' => output.push('\n'),
            'r' => output.push('\r'),
            't' => output.push('\t'),
            'u' => {
                let mut code = String::new();
                for _ in 0..4 {
                    let Some(hex) = chars.next() else {
                        return Err(RuntimeError::Module {
                            module: "arb-runtime",
                            message: "JSON unicode escape is incomplete".to_owned(),
                        });
                    };
                    code.push(hex);
                }
                let value =
                    u32::from_str_radix(&code, 16).map_err(|error| RuntimeError::Module {
                        module: "arb-runtime",
                        message: format!("JSON unicode escape is invalid: {error}"),
                    })?;
                let Some(decoded) = char::from_u32(value) else {
                    return Err(RuntimeError::Module {
                        module: "arb-runtime",
                        message: "JSON unicode escape is not a valid scalar value".to_owned(),
                    });
                };
                output.push(decoded);
            }
            other => {
                return Err(RuntimeError::Module {
                    module: "arb-runtime",
                    message: format!("unsupported JSON escape `\\{other}`"),
                });
            }
        }
    }
    Ok(output)
}

fn manual_approval_audit_record_from_execution(
    record: &ManualApprovalRecord,
) -> ManualApprovalAuditRecord {
    ManualApprovalAuditRecord {
        record_id: record.record_id.clone(),
        approval_event_id: record.approval_event_id.clone(),
        risk_decision_id: record.risk_decision_id.clone(),
        transition_id: record.transition_id.clone(),
        plan_id: record.plan_id.clone(),
        plan_hash: record.plan_hash.clone(),
        decision: record.decision.as_str().to_owned(),
        status: record.status.as_str().to_owned(),
        reviewer_id: record.reviewer_id.clone(),
        decided_at: record.decided_at.clone(),
        expires_at: record.expires_at.clone(),
        reason: record.reason.clone(),
        duplicate_of: record.duplicate_of.clone(),
        releases_manual_gate: record.releases_manual_gate,
        controlled_next_step: record.controlled_next_step.clone(),
    }
}

fn manual_approval_records_jsonl(records: &[ManualApprovalRecord]) -> String {
    jsonl_from_lines(
        records
            .iter()
            .map(ManualApprovalRecord::to_audit_json)
            .collect(),
    )
}

fn binance_manual_confirmation_template(
    plan: &ExecutionPlan,
    plan_hash: &str,
    generated_at: &str,
    approval_records: &[ManualApprovalRecord],
) -> String {
    let confirmation_status = approval_records
        .first()
        .map(|record| record.status.as_str())
        .unwrap_or("pending");
    let approval_record_summary = if approval_records.is_empty() {
        "- 无审批记录。".to_owned()
    } else {
        approval_records
            .iter()
            .map(|record| {
                format!(
                    "- `{}` status={} decision={} reviewer={} decided_at={} expires_at={} releases_manual_gate={}",
                    record.record_id,
                    record.status.as_str(),
                    record.decision.as_str(),
                    record.reviewer_id,
                    record.decided_at,
                    record.expires_at,
                    record.releases_manual_gate
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        r#"# Binance BTCUSDT GuardedLivePersonal 人工确认记录

- 生成时间：{generated_at}
- 交易场所：Binance spot（现货公开行情）
- 标的：BTC-USDT / BTCUSDT
- 结算资产：USDT
- 目标执行模式：GuardedLive
- 计划预览执行模式：{execution_mode}
- 计划创建时间：{plan_created_at}
- 计划 ID：{plan_id}
- 计划哈希：{plan_hash}
- 审批前是否可分发执行：false
- 单笔名义本金上限：{notional} USDT
- 资本上限：{capital_limit} USDT
- 日亏损停止阈值：{daily_loss_limit} USDT
- 账户引用：{account_ref}
- 人工确认状态：{confirmation_status}

## 审批记录

{approval_record_summary}

## 中文确认要求

人工确认只能针对上面的同一个 `plan_hash`。如果 `plan_preview.json` 内容、行情、
风控输出或账户状态发生变化，必须重新生成计划预览并使用新的哈希；旧确认不得沿用。

批准只表示释放人工审批门禁，不等于绕过执行模式、熔断、权限、资本预留、账本、
对账和未知状态检查。本步骤不会下单、不会撤单、不会转账、不会签名。

## 记录批准或拒绝

批准示例：

```bash
cargo run -p arb-runtime -- binance-guarded-live-preview \
  --market-artifacts target/binance-basis-pipeline \
  --out target/binance-guarded-live-preview \
  --decision approve \
  --expected-plan-hash {plan_hash} \
  --approval-event-id event:approval:binance-btcusdt-001 \
  --reviewer owner:redacted \
  --decided-at 2026-05-13T00:00:00Z \
  --expires-at 2026-05-13T00:05:00Z \
  --reason "Owner approved the same plan_hash for guarded live pilot."
```

拒绝示例：

```bash
cargo run -p arb-runtime -- binance-guarded-live-preview \
  --market-artifacts target/binance-basis-pipeline \
  --out target/binance-guarded-live-preview \
  --decision reject \
  --expected-plan-hash {plan_hash} \
  --approval-event-id event:approval:binance-btcusdt-001 \
  --reviewer owner:redacted \
  --decided-at 2026-05-13T00:00:00Z \
  --expires-at 2026-05-13T00:05:00Z \
  --reason "Owner rejected the guarded live pilot."
```
"#,
        execution_mode = plan.execution_mode.as_str(),
        plan_created_at = plan.created_at.as_str(),
        plan_id = plan.plan_id.as_str(),
        notional = BINANCE_GUARDED_LIVE_NOTIONAL_USDT,
        capital_limit = BINANCE_GUARDED_LIVE_CAPITAL_LIMIT_USDT,
        daily_loss_limit = BINANCE_GUARDED_LIVE_DAILY_LOSS_LIMIT_USDT,
        account_ref = BINANCE_GUARDED_LIVE_ACCOUNT_REF,
        confirmation_status = confirmation_status,
        approval_record_summary = approval_record_summary,
    )
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

fn write_bybit_basis_monitor_snapshot(
    output_dir: &Path,
    snapshot: &BybitBasisMonitorSnapshot,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("bybit_basis_monitor_snapshot.json"),
        &snapshot.to_json(),
    )
}

fn write_okx_basis_monitor_snapshot(
    output_dir: &Path,
    snapshot: &OkxBasisMonitorSnapshot,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("okx_basis_monitor_snapshot.json"),
        &snapshot.to_json(),
    )
}

fn write_hyperliquid_basis_monitor_snapshot(
    output_dir: &Path,
    snapshot: &HyperliquidBasisMonitorSnapshot,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("hyperliquid_basis_monitor_snapshot.json"),
        &snapshot.to_json(),
    )
}

fn write_aster_basis_monitor_snapshot(
    output_dir: &Path,
    snapshot: &AsterBasisMonitorSnapshot,
) -> RuntimeResult<()> {
    fs::create_dir_all(output_dir).map_err(|error| RuntimeError::Io {
        path: output_dir.to_path_buf(),
        message: error.to_string(),
    })?;
    write_utf8(
        output_dir.join("aster_basis_monitor_snapshot.json"),
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

    fn empty_bybit(options: &BybitBasisMonitorOptions) -> Self {
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

    fn empty_okx(options: &OkxBasisMonitorOptions) -> Self {
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

    fn empty_hyperliquid(options: &HyperliquidBasisMonitorOptions) -> Self {
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

    fn empty_aster(options: &AsterBasisMonitorOptions) -> Self {
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

fn current_utc_timestamp_string() -> String {
    current_utc_timestamp()
        .map(|timestamp| timestamp.to_string())
        .unwrap_or_else(|_| "timestamp-unavailable".to_owned())
}

impl BinanceWssBookTickerQuoteSnapshot {
    fn from_quote(quote: &MarketQuote) -> Self {
        let symbol = quote
            .instrument_id
            .as_str()
            .split(':')
            .nth(2)
            .unwrap_or_else(|| quote.instrument_id.as_str())
            .to_owned();
        Self {
            symbol,
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

    fn to_json(&self) -> String {
        format!(
            "{{\"ask_size\":{},\"best_ask\":{},\"best_bid\":{},\"bid_size\":{},\"freshness_status\":{},\"ingested_at\":{},\"instrument_id\":{},\"observed_at\":{},\"source_event_id\":{},\"source_sequence\":{},\"symbol\":{},\"venue_id\":{}}}",
            json_option_string(&self.ask_size),
            json_option_string(&self.best_ask),
            json_option_string(&self.best_bid),
            json_option_string(&self.bid_size),
            json_string(&self.freshness_status),
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

impl BinanceWssBookTickerMonitorSnapshot {
    fn empty(symbol: &str, market: BinancePublicMarket, stream_url: &str) -> Self {
        Self {
            status: "starting".to_owned(),
            updated_at: "not-yet-updated".to_owned(),
            symbol: symbol.to_owned(),
            market: market.as_str().to_owned(),
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

    fn begin_rest_rebuild(&mut self) {
        self.rest_rebuild_count += 1;
        self.status = "rebuilding".to_owned();
        self.updated_at = current_utc_timestamp_string();
    }

    fn record_update(&mut self, update: &HybridMarketDataUpdate) {
        self.updated_at = current_utc_timestamp_string();
        self.coordinator_status = update.status.as_str().to_owned();
        self.status = binance_wss_monitor_status(update.status, update.fail_closed).to_owned();
        self.fail_closed = update.fail_closed;
        if update.fail_closed {
            self.fail_closed_count += 1;
            self.last_error = Some(if update.reason_codes.is_empty() {
                "fail_closed".to_owned()
            } else {
                format!("fail_closed: {}", update.reason_codes.join(","))
            });
        } else {
            self.last_error = None;
        }
        if update.status == HybridMarketDataStatus::Reconnecting {
            self.disconnect_count += 1;
        }
        if update.transport == MarketDataTransport::WebSocketStream && update.quote.is_some() {
            self.wss_update_count += 1;
        }
        if let Some(quote) = &update.quote {
            let quote_snapshot = BinanceWssBookTickerQuoteSnapshot::from_quote(quote);
            self.upsert_quote_row(quote_snapshot.clone());
            self.latest_quote = Some(quote_snapshot);
        }
    }

    fn upsert_quote_row(&mut self, quote: BinanceWssBookTickerQuoteSnapshot) {
        match self.rows.iter_mut().find(|row| row.symbol == quote.symbol) {
            Some(row) => *row = quote,
            None => self.rows.push(quote),
        }
        self.rows
            .sort_by(|left, right| left.symbol.cmp(&right.symbol));
        self.total_rows = self.rows.len();
    }

    fn record_failure(&mut self, detail: impl Into<String>, count_disconnect: bool) {
        self.status = "fail_closed".to_owned();
        self.updated_at = current_utc_timestamp_string();
        self.fail_closed = true;
        self.fail_closed_count += 1;
        if count_disconnect {
            self.disconnect_count += 1;
        }
        self.last_error = Some(detail.into());
    }

    fn record_stream_end(&mut self) {
        if !self.fail_closed {
            self.record_failure(
                "Binance public WSS ended before reconnect; rebuilding from REST",
                true,
            );
            return;
        }
        if self.last_error.is_none() {
            self.last_error = Some("Binance public WSS ended; rebuilding from REST".to_owned());
        }
    }

    fn latest_quote_json_value(&self) -> String {
        self.latest_quote
            .as_ref()
            .map(BinanceWssBookTickerQuoteSnapshot::to_json)
            .unwrap_or_else(|| "null".to_owned())
    }

    fn health_json(&self) -> String {
        format!(
            "{{\"disconnect_count\":{},\"fail_closed\":{},\"latest_quote\":{},\"rest_rebuild_count\":{},\"status\":{},\"total_rows\":{},\"updated_at\":{},\"wss_update_count\":{}}}",
            self.disconnect_count,
            self.fail_closed,
            self.latest_quote_json_value(),
            self.rest_rebuild_count,
            json_string(&self.status),
            self.total_rows,
            json_string(&self.updated_at),
            self.wss_update_count,
        )
    }

    fn quote_json(&self) -> String {
        format!(
            "{{\"fail_closed\":{},\"latest_quote\":{},\"status\":{},\"updated_at\":{}}}",
            self.fail_closed,
            self.latest_quote_json_value(),
            json_string(&self.status),
            json_string(&self.updated_at),
        )
    }

    fn quotes_json(&self) -> String {
        format!(
            "{{\"fail_closed\":{},\"rows\":[{}],\"status\":{},\"total_rows\":{},\"updated_at\":{}}}",
            self.fail_closed,
            self.rows
                .iter()
                .map(BinanceWssBookTickerQuoteSnapshot::to_json)
                .collect::<Vec<_>>()
                .join(","),
            json_string(&self.status),
            self.total_rows,
            json_string(&self.updated_at),
        )
    }

    fn to_json(&self) -> String {
        format!(
            "{{\"coordinator_status\":{},\"disconnect_count\":{},\"fail_closed\":{},\"fail_closed_count\":{},\"last_error\":{},\"latest_quote\":{},\"market\":{},\"rest_rebuild_count\":{},\"rows\":[{}],\"status\":{},\"stream_url\":{},\"symbol\":{},\"total_rows\":{},\"updated_at\":{},\"wss_update_count\":{}}}",
            json_string(&self.coordinator_status),
            self.disconnect_count,
            self.fail_closed,
            self.fail_closed_count,
            json_option_string(&self.last_error),
            self.latest_quote_json_value(),
            json_string(&self.market),
            self.rest_rebuild_count,
            self.rows
                .iter()
                .map(BinanceWssBookTickerQuoteSnapshot::to_json)
                .collect::<Vec<_>>()
                .join(","),
            json_string(&self.status),
            json_string(&self.stream_url),
            json_string(&self.symbol),
            self.total_rows,
            json_string(&self.updated_at),
            self.wss_update_count,
        )
    }
}

fn binance_wss_monitor_status(status: HybridMarketDataStatus, fail_closed: bool) -> &'static str {
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

fn start_binance_wss_book_ticker_http_api(
    bind_addr: &str,
    state: Arc<RwLock<BinanceWssBookTickerMonitorSnapshot>>,
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

fn handle_binance_wss_book_ticker_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<BinanceWssBookTickerMonitorSnapshot>>,
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
        .expect("Binance WSS monitor state lock poisoned");
    let (status, body) = if route == "/health" {
        (
            if snapshot.fail_closed { 503 } else { 200 },
            snapshot.health_json(),
        )
    } else if route == "/api/binance-wss-book-ticker/status" {
        (200, snapshot.to_json())
    } else if route == "/api/binance-wss-book-ticker/quote" {
        (200, snapshot.quote_json())
    } else if route == "/api/binance-wss-book-ticker/quotes" {
        (200, snapshot.quotes_json())
    } else if route == "/" || route == "/dashboard" {
        let _ = write_http_html(&mut stream, 200, binance_wss_book_ticker_dashboard_html());
        return;
    } else {
        (
            404,
            "{\"error\":\"not_found\",\"paths\":[\"/\",\"/dashboard\",\"/health\",\"/api/binance-wss-book-ticker/status\",\"/api/binance-wss-book-ticker/quote\",\"/api/binance-wss-book-ticker/quotes\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
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

fn start_bybit_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<BybitBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Bybit monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_bybit_basis_http_connection(stream, &state),
                Err(error) => eprintln!("bybit-basis-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

fn start_okx_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<OkxBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind OKX monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_okx_basis_http_connection(stream, &state),
                Err(error) => eprintln!("okx-basis-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

fn start_hyperliquid_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<HyperliquidBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Hyperliquid monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_hyperliquid_basis_http_connection(stream, &state),
                Err(error) => eprintln!("hyperliquid-basis-monitor api accept failed: {error}"),
            }
        }
    });
    Ok(handle)
}

fn start_aster_basis_http_api(
    bind_addr: &str,
    state: Arc<RwLock<AsterBasisMonitorSnapshot>>,
) -> RuntimeResult<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(bind_addr).map_err(|error| RuntimeError::LiveMarketData {
        message: format!("cannot bind Aster monitor HTTP API on {bind_addr}: {error}"),
    })?;
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_aster_basis_http_connection(stream, &state),
                Err(error) => eprintln!("aster-basis-monitor api accept failed: {error}"),
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

fn handle_bybit_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<BybitBasisMonitorSnapshot>>,
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
        let html = bybit_basis_dashboard_html();
        let _ = write_http_html(&mut stream, 200, &html);
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
    } else if route == "/api/bybit-basis/status" {
        (200, snapshot.to_json())
    } else if route == "/api/bybit-basis/opportunities" {
        (200, snapshot.opportunities_json())
    } else if let Some(symbol) = route.strip_prefix("/api/bybit-basis/status/") {
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
            "{\"error\":\"not_found\",\"paths\":[\"/\",\"/dashboard\",\"/health\",\"/api/bybit-basis/status\",\"/api/bybit-basis/opportunities\",\"/api/bybit-basis/status/<SYMBOL>\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn handle_okx_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<OkxBasisMonitorSnapshot>>,
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
        let html = okx_basis_dashboard_html();
        let _ = write_http_html(&mut stream, 200, &html);
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
    } else if route == "/api/okx-basis/status" {
        (200, snapshot.to_json())
    } else if route == "/api/okx-basis/opportunities" {
        (200, snapshot.opportunities_json())
    } else if let Some(symbol) = route.strip_prefix("/api/okx-basis/status/") {
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
            "{\"error\":\"not_found\",\"paths\":[\"/\",\"/dashboard\",\"/health\",\"/api/okx-basis/status\",\"/api/okx-basis/opportunities\",\"/api/okx-basis/status/<SYMBOL>\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn handle_hyperliquid_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<HyperliquidBasisMonitorSnapshot>>,
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
        let html = hyperliquid_basis_dashboard_html();
        let _ = write_http_html(&mut stream, 200, &html);
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
    } else if route == "/api/hyperliquid-basis/status" {
        (200, snapshot.to_json())
    } else if route == "/api/hyperliquid-basis/opportunities" {
        (200, snapshot.opportunities_json())
    } else if let Some(symbol) = route.strip_prefix("/api/hyperliquid-basis/status/") {
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
            "{\"error\":\"not_found\",\"paths\":[\"/\",\"/dashboard\",\"/health\",\"/api/hyperliquid-basis/status\",\"/api/hyperliquid-basis/opportunities\",\"/api/hyperliquid-basis/status/<SYMBOL>\"]}".to_owned(),
        )
    };
    let _ = write_http_json(&mut stream, status, &body);
}

fn handle_aster_basis_http_connection(
    mut stream: TcpStream,
    state: &Arc<RwLock<AsterBasisMonitorSnapshot>>,
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
        let html = aster_basis_dashboard_html();
        let _ = write_http_html(&mut stream, 200, &html);
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
    } else if route == "/api/aster-basis/status" {
        (200, snapshot.to_json())
    } else if route == "/api/aster-basis/opportunities" {
        (200, snapshot.opportunities_json())
    } else if let Some(symbol) = route.strip_prefix("/api/aster-basis/status/") {
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
            "{\"error\":\"not_found\",\"paths\":[\"/\",\"/dashboard\",\"/health\",\"/api/aster-basis/status\",\"/api/aster-basis/opportunities\",\"/api/aster-basis/status/<SYMBOL>\"]}".to_owned(),
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

fn binance_wss_book_ticker_dashboard_html() -> &'static str {
    r##"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Binance WSS bookTicker</title>
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
    }

    * { box-sizing: border-box; }

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
    input {
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
    .positive {
      color: var(--green);
      border-color: rgba(88, 195, 131, 0.45);
    }

    .pill.error,
    .negative {
      color: var(--red);
      border-color: rgba(224, 106, 106, 0.45);
    }

    main {
      padding: 18px 28px 32px;
    }

    .summary-grid,
    .quote-grid {
      display: grid;
      grid-template-columns: repeat(6, minmax(130px, 1fr));
      gap: 10px;
      margin-bottom: 14px;
    }

    .metric,
    .quote-field {
      min-height: 72px;
      padding: 12px;
      border: 1px solid var(--line);
      border-radius: 6px;
      background: var(--panel);
    }

    .metric span,
    .quote-field span,
    .section-head span {
      display: block;
      color: var(--muted);
      font-size: 12px;
      white-space: nowrap;
    }

    .metric strong,
    .quote-field strong {
      display: block;
      margin-top: 8px;
      font-size: 22px;
      line-height: 1.1;
      overflow-wrap: anywhere;
    }

    .section {
      margin-top: 14px;
      border: 1px solid var(--line);
      background: var(--band);
    }

    .section-head {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      min-height: 44px;
      padding: 10px 12px;
      border-bottom: 1px solid var(--line);
      color: var(--muted);
    }

    .endpoint-grid {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
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

    .control-bar {
      display: grid;
      grid-template-columns: minmax(150px, 220px) minmax(120px, max-content);
      gap: 10px;
      align-items: end;
      padding: 12px;
      margin-bottom: 14px;
      border: 1px solid var(--line);
      background: var(--band);
    }

    label span {
      display: block;
      margin-bottom: 6px;
      color: var(--muted);
      font-size: 12px;
    }

    input[type="search"] {
      width: 100%;
      min-height: 36px;
      border: 1px solid var(--line);
      border-radius: 6px;
      background: #101214;
      color: var(--text);
      padding: 7px 9px;
    }

    .table-scroll {
      overflow-x: auto;
      max-height: 52vh;
    }

    table {
      width: 100%;
      min-width: 980px;
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
    td:first-child {
      text-align: left;
    }

    tbody tr:hover {
      background: rgba(226, 182, 80, 0.08);
    }

    .empty-row {
      height: 88px;
      color: var(--muted);
      text-align: center;
    }

    pre {
      margin: 0;
      max-height: 320px;
      overflow: auto;
      padding: 12px;
      background: #0c0d0e;
      color: #d7d1c2;
      border-top: 1px solid var(--line);
      font-size: 12px;
      line-height: 1.5;
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

      .summary-grid,
      .quote-grid {
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

      .summary-grid,
      .quote-grid {
        grid-template-columns: 1fr;
      }
    }
  </style>
</head>
<body>
  <header class="topbar">
    <div>
      <p class="kicker">Binance public WSS</p>
      <h1>bookTicker 实时行情</h1>
    </div>
    <div class="status-strip">
      <span id="runtime-status" class="pill">starting</span>
      <span id="updated-at">not-yet-updated</span>
    </div>
  </header>
  <main>
    <section class="summary-grid" aria-label="summary">
      <article class="metric"><span>Symbol</span><strong id="metric-symbol">-</strong></article>
      <article class="metric"><span>Market</span><strong id="metric-market">-</strong></article>
      <article class="metric"><span>fail_closed</span><strong id="metric-fail-closed">false</strong></article>
      <article class="metric"><span>断线次数</span><strong id="metric-disconnects">0</strong></article>
      <article class="metric"><span>REST rebuild</span><strong id="metric-rebuilds">0</strong></article>
      <article class="metric"><span>WSS updates</span><strong id="metric-updates">0</strong></article>
    </section>

    <section class="quote-grid" aria-label="latest quote">
      <article class="quote-field"><span>Best Bid</span><strong id="quote-bid">-</strong></article>
      <article class="quote-field"><span>Bid Size</span><strong id="quote-bid-size">-</strong></article>
      <article class="quote-field"><span>Best Ask</span><strong id="quote-ask">-</strong></article>
      <article class="quote-field"><span>Ask Size</span><strong id="quote-ask-size">-</strong></article>
      <article class="quote-field"><span>Sequence</span><strong id="quote-sequence">-</strong></article>
      <article class="quote-field"><span>Freshness</span><strong id="quote-freshness">-</strong></article>
    </section>

    <section class="control-bar" aria-label="controls">
      <label>
        <span>Symbol</span>
        <input id="symbol-filter" type="search" autocomplete="off" placeholder="BTCUSDT">
      </label>
      <button id="refresh-button" type="button">刷新</button>
    </section>

    <section class="section" aria-label="book ticker table">
      <div class="section-head">
        <strong>全部 bookTicker</strong>
        <span id="row-count">0 rows</span>
      </div>
      <div class="table-scroll">
        <table>
          <thead>
            <tr>
              <th>Symbol</th>
              <th>Bid</th>
              <th>Bid Size</th>
              <th>Ask</th>
              <th>Ask Size</th>
              <th>Sequence</th>
              <th>Freshness</th>
              <th>Observed</th>
            </tr>
          </thead>
          <tbody id="quote-rows">
            <tr><td class="empty-row" colspan="8">loading</td></tr>
          </tbody>
        </table>
      </div>
    </section>

    <section class="section" aria-label="stream state">
      <div class="section-head">
        <strong>连接状态</strong>
        <span id="stream-url">-</span>
      </div>
      <pre id="state-preview">{}</pre>
    </section>

    <section class="section" aria-label="realtime api">
      <div class="section-head">
        <strong>实时 API</strong>
        <div class="endpoint-grid">
          <button type="button" data-endpoint="/health">health</button>
          <button type="button" data-endpoint="/api/binance-wss-book-ticker/quote">quote</button>
          <button type="button" data-endpoint="/api/binance-wss-book-ticker/quotes">quotes</button>
          <button type="button" data-endpoint="/api/binance-wss-book-ticker/status">status</button>
        </div>
      </div>
      <pre id="api-preview">{}</pre>
    </section>
  </main>

  <script>
    const statusUrl = "/api/binance-wss-book-ticker/status";
    const state = { timer: null };
    const $ = (id) => document.getElementById(id);

    async function requestJson(url) {
      const response = await fetch(url, { cache: "no-store" });
      const text = await response.text();
      let data;
      try {
        data = JSON.parse(text);
      } catch (error) {
        throw new Error(`invalid json from ${url}: ${error.message}`);
      }
      if (!response.ok && url !== "/health") {
        throw new Error(data.error || `http ${response.status}`);
      }
      return data;
    }

    function valueOrDash(value) {
      return value === null || value === undefined || value === "" ? "-" : String(value);
    }

    function escapeHtml(value) {
      return valueOrDash(value).replace(/[&<>"']/g, (ch) => ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        "\"": "&quot;",
        "'": "&#39;"
      }[ch]));
    }

    function filteredRows(rows) {
      const symbolFilter = $("symbol-filter").value.trim().toUpperCase();
      return (rows || [])
        .filter((row) => !symbolFilter || String(row.symbol || "").includes(symbolFilter))
        .sort((left, right) => String(left.symbol || "").localeCompare(String(right.symbol || "")));
    }

    function renderRows(rows) {
      const body = $("quote-rows");
      const view = filteredRows(rows);
      $("row-count").textContent = `${view.length} / ${(rows || []).length} rows`;
      if (!view.length) {
        body.innerHTML = `<tr><td class="empty-row" colspan="8">no rows</td></tr>`;
        return;
      }
      body.innerHTML = view.map((row) => `<tr>
        <td>${escapeHtml(row.symbol)}</td>
        <td>${escapeHtml(row.best_bid)}</td>
        <td>${escapeHtml(row.bid_size)}</td>
        <td>${escapeHtml(row.best_ask)}</td>
        <td>${escapeHtml(row.ask_size)}</td>
        <td>${escapeHtml(row.source_sequence)}</td>
        <td>${escapeHtml(row.freshness_status)}</td>
        <td>${escapeHtml(row.observed_at)}</td>
      </tr>`).join("");
    }

    function render(snapshot) {
      const quote = snapshot.latest_quote || {};
      const healthy = snapshot.status === "streaming" && snapshot.fail_closed === false;
      $("runtime-status").textContent = snapshot.status || "unknown";
      $("runtime-status").className = `pill ${healthy ? "healthy" : "error"}`;
      $("updated-at").textContent = snapshot.updated_at || "not-yet-updated";
      $("metric-symbol").textContent = valueOrDash(snapshot.symbol);
      $("metric-market").textContent = valueOrDash(snapshot.market);
      $("metric-fail-closed").textContent = String(snapshot.fail_closed ?? false);
      $("metric-fail-closed").className = snapshot.fail_closed ? "negative" : "positive";
      $("metric-disconnects").textContent = snapshot.disconnect_count ?? 0;
      $("metric-rebuilds").textContent = snapshot.rest_rebuild_count ?? 0;
      $("metric-updates").textContent = snapshot.wss_update_count ?? 0;
      $("quote-bid").textContent = valueOrDash(quote.best_bid);
      $("quote-bid-size").textContent = valueOrDash(quote.bid_size);
      $("quote-ask").textContent = valueOrDash(quote.best_ask);
      $("quote-ask-size").textContent = valueOrDash(quote.ask_size);
      $("quote-sequence").textContent = valueOrDash(quote.source_sequence);
      $("quote-freshness").textContent = valueOrDash(quote.freshness_status);
      renderRows(snapshot.rows || []);
      $("stream-url").textContent = valueOrDash(snapshot.stream_url);
      $("state-preview").textContent = JSON.stringify({
        coordinator_status: snapshot.coordinator_status,
        last_error: snapshot.last_error,
        observed_at: quote.observed_at,
        ingested_at: quote.ingested_at,
        source_event_id: quote.source_event_id
      }, null, 2);
      $("api-preview").textContent = JSON.stringify(snapshot, null, 2);
    }

    async function refreshStatus() {
      try {
        const snapshot = await requestJson(statusUrl);
        render(snapshot);
      } catch (error) {
        $("runtime-status").textContent = "error";
        $("runtime-status").className = "pill error";
        $("state-preview").textContent = error.message;
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

    document.querySelectorAll("[data-endpoint]").forEach((button) => {
      button.addEventListener("click", () => previewEndpoint(button.dataset.endpoint || statusUrl));
    });
    $("symbol-filter").addEventListener("input", refreshStatus);
    $("refresh-button").addEventListener("click", refreshStatus);

    state.timer = setInterval(refreshStatus, 2000);
    refreshStatus();
  </script>
</body>
</html>"##
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

fn bybit_basis_dashboard_html() -> String {
    basis_dashboard_html()
        .replace("Binance public basis", "Bybit public basis")
        .replace("/api/basis/status", "/api/bybit-basis/status")
        .replace("/api/basis/opportunities", "/api/bybit-basis/opportunities")
}

fn okx_basis_dashboard_html() -> String {
    basis_dashboard_html()
        .replace("Binance public basis", "OKX public basis")
        .replace("/api/basis/status", "/api/okx-basis/status")
        .replace("/api/basis/opportunities", "/api/okx-basis/opportunities")
}

fn hyperliquid_basis_dashboard_html() -> String {
    basis_dashboard_html()
        .replace("Binance public basis", "Hyperliquid public basis")
        .replace("Spot Bid", "Spot Mid")
        .replace("Spot Ask", "Spot Mid")
        .replace("Perp Bid", "Perp Mid")
        .replace("Perp Ask", "Perp Mid")
        .replace("/api/basis/status", "/api/hyperliquid-basis/status")
        .replace(
            "/api/basis/opportunities",
            "/api/hyperliquid-basis/opportunities",
        )
}

fn aster_basis_dashboard_html() -> String {
    basis_dashboard_html()
        .replace("Binance public basis", "Aster public basis")
        .replace("/api/basis/status", "/api/aster-basis/status")
        .replace("/api/basis/opportunities", "/api/aster-basis/opportunities")
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
    if args[0] == "binance-basis-pipeline" {
        let options = parse_binance_basis_pipeline_args(&args[1..])?;
        let report = run_binance_basis_pipeline(&options.symbol, options.output_dir.clone())?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: ran Binance public spot/perp basis through simulated pipeline for {}; candidate_transitions={}; risk_decisions={}; execution_reports={}; incidents={}; mutable_execution_started=false{}",
            report.symbol,
            count_jsonl_records(&report.artifacts.candidate_transitions_jsonl),
            count_jsonl_records(&report.artifacts.risk_decisions_jsonl),
            count_jsonl_records(&report.artifacts.execution_reports_jsonl),
            count_jsonl_records(&report.artifacts.incidents_jsonl),
            output_note
        ));
    }
    if args[0] == "binance-guarded-live-preview" {
        let options = parse_binance_guarded_live_preview_args(&args[1..])?;
        let report = run_binance_guarded_live_preview(
            &options.market_artifacts_dir,
            options.output_dir.clone(),
            options.decision_request,
        )?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        let approval_status = report
            .approval_status
            .as_deref()
            .unwrap_or("PendingManualApproval");
        return Ok(format!(
            "ok: generated Binance BTCUSDT GuardedLivePersonal plan preview; plan_hash={}; dispatchable_before_approval={}; approval_records={}; approval_status={}; mutable_execution_started=false{}",
            report.plan_hash,
            report.dispatchable_before_approval,
            report.approval_record_count,
            approval_status,
            output_note
        ));
    }
    if args[0] == "binance-guarded-live-gate-release-preview" {
        let options = parse_binance_manual_gate_release_preview_args(&args[1..])?;
        let report = run_binance_manual_gate_release_preview(
            &options.preview_dir,
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
            "ok: generated Binance BTCUSDT manual gate release preview; plan_hash={}; approval_event_id={}; released_manual_gate={}; dependent_transitions={}; dispatchable_after_release={}; mutable_execution_started=false{}",
            report.plan_hash,
            report.approval_event_id,
            report.released_manual_gate,
            report.dependent_transition_count,
            report.dispatchable_after_release,
            output_note
        ));
    }
    if args[0] == "binance-guarded-live-pre-dispatch-dry-run" {
        let options = parse_binance_pre_dispatch_dry_run_args(&args[1..])?;
        let report = run_binance_pre_dispatch_dry_run(
            &options.preview_dir,
            &options.config_path,
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
            "ok: ran Binance BTCUSDT pre-dispatch dry run; plan_hash={}; approval_event_id={}; manual_gate_released={}; dispatch_allowed={}; blocking_reasons={}; mutable_execution_started=false{}",
            report.plan_hash,
            report.approval_event_id,
            report.manual_gate_released,
            report.dispatch_allowed,
            report.blocking_reasons.len(),
            output_note
        ));
    }
    if args[0] == "binance-guarded-live-dispatch" {
        let options = parse_binance_guarded_live_dispatch_args(&args[1..])?;
        let report = run_binance_guarded_live_dispatch(options)?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: Binance BTCUSDT GuardedLive dispatch completed; plan_hash={}; approval_event_id={}; dispatch_allowed={}; submitted_receipts={}; private_confirmations={}; execution_report_status={}; blocking_reasons={}{}",
            report.plan_hash,
            report.approval_event_id,
            report.dispatch_allowed,
            report.submitted_receipt_count,
            report.private_confirmation_count,
            report
                .execution_report_status
                .as_deref()
                .unwrap_or("none"),
            report.blocking_reasons.len(),
            output_note
        ));
    }
    if args[0] == "binance-guarded-live-auto-once" {
        let options = parse_binance_guarded_live_auto_once_args(&args[1..])?;
        let report = run_binance_guarded_live_auto_once(options)?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: Binance BTCUSDT auto-once completed; signal_allowed={}; plan_hash={}; manual_gate_released={}; dispatch_attempted={}; dispatch_allowed={}; submitted_receipts={}; private_confirmations={}; blocking_reasons={}{}",
            report.signal_allowed,
            report.plan_hash.as_deref().unwrap_or("none"),
            report.manual_gate_released,
            report.dispatch_attempted,
            report.dispatch_allowed,
            report.submitted_receipt_count,
            report.private_confirmation_count,
            report.blocking_reasons.len(),
            output_note
        ));
    }
    if args[0] == "binance-basis-guarded-live-auto-once" {
        let options = parse_binance_basis_guarded_live_auto_once_args(&args[1..])?;
        let report = run_binance_basis_guarded_live_auto_once(options)?;
        let output_note = report
            .output_dir
            .as_ref()
            .map(|path| format!("; wrote artifacts to {}", path.display()))
            .unwrap_or_else(|| {
                "; no artifacts written, pass --out <dir> to persist them".to_owned()
            });
        return Ok(format!(
            "ok: Binance basis auto-once completed; signal_allowed={}; net_bps={}; planned_orders={}; dispatch_attempted={}; dispatch_allowed={}; submitted_receipts={}; private_confirmations={}; protection_attempted={}; protection_receipts={}; residual_risk={}; blocking_reasons={}{}",
            report.signal_allowed,
            report
                .net_bps
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_owned()),
            report.planned_order_count,
            report.dispatch_attempted,
            report.dispatch_allowed,
            report.submitted_receipt_count,
            report.private_confirmation_count,
            report.protection_attempted,
            report.protection_receipt_count,
            report.residual_risk.as_deref().unwrap_or("none"),
            report.blocking_reasons.len(),
            output_note
        ));
    }
    if args[0] == "binance-wss-book-ticker" {
        let options = parse_binance_wss_book_ticker_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        if once && !is_binance_wss_all_symbols_scope(&options.symbol) {
            let report = run_binance_wss_book_ticker_probe(options)?;
            return Ok(format!(
                "ok: Binance public WSS bookTicker market={} symbol={} updates={} fail_closed={} status={} bid={} ask={} stream={}; mutable_execution_started=false",
                report.market.as_str(),
                report.symbol,
                report.update_count,
                report.fail_closed_count,
                report.coordinator_status,
                report.latest_best_bid.as_deref().unwrap_or("null"),
                report.latest_best_ask.as_deref().unwrap_or("null"),
                report.stream_url
            ));
        }
        run_binance_wss_book_ticker_monitor(options)?;
        if once {
            return Ok(format!(
                "ok: ran one Binance public WSS bookTicker all-market monitor cycle; api_bind={bind_addr}; mutable_execution_started=false"
            ));
        }
        return Ok(format!(
            "ok: Binance public WSS bookTicker monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
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
    if args[0] == "bybit-basis-monitor" {
        let options = parse_bybit_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_bybit_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one Bybit basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: Bybit basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "okx-basis-monitor" {
        let options = parse_okx_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_okx_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one OKX basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: OKX basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "hyperliquid-basis-monitor" {
        let options = parse_hyperliquid_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_hyperliquid_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one Hyperliquid basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: Hyperliquid basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
        ));
    }
    if args[0] == "aster-basis-monitor" {
        let options = parse_aster_basis_monitor_args(&args[1..])?;
        let once = options.once;
        let bind_addr = options.bind_addr.clone();
        let output_dir = options.output_dir.clone();
        run_aster_basis_monitor(options)?;
        if once {
            let output_note = output_dir
                .as_ref()
                .map(|path| format!("; wrote snapshot to {}", path.display()))
                .unwrap_or_else(|| "; no snapshot written, pass --out <dir>".to_owned());
            return Ok(format!(
                "ok: ran one Aster basis monitor refresh; api_bind={bind_addr}; mutable_execution_started=false{output_note}"
            ));
        }
        return Ok(format!(
            "ok: Aster basis monitor stopped; api_bind={bind_addr}; mutable_execution_started=false"
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
                "unknown command `{}`; supported commands: replay, health, health-config, live-market-sim, binance-basis-scan, binance-basis-pipeline, binance-guarded-live-preview, binance-guarded-live-gate-release-preview, binance-guarded-live-pre-dispatch-dry-run, binance-guarded-live-dispatch, binance-guarded-live-auto-once, binance-basis-guarded-live-auto-once, binance-wss-book-ticker, binance-basis-monitor, bybit-basis-monitor, okx-basis-monitor, hyperliquid-basis-monitor, aster-basis-monitor",
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
        "中文说明：默认只运行离线、模拟、可回放的运行时装配；实盘分发命令需要 live-exec 构建和显式确认。",
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
        "  binance-basis-pipeline [--symbol BTCUSDT] [--out dir]",
        "                                    Fetch Binance public spot/perp data and run simulated pipeline artifacts",
        "  binance-guarded-live-preview [--market-artifacts dir] [--out dir] [--decision approve|reject --expected-plan-hash hash --approval-event-id id --reviewer id --decided-at ts --expires-at ts --reason text]",
        "                                    Generate Binance BTCUSDT GuardedLivePersonal plan preview, plan hash, and manual confirmation audit material",
        "  binance-guarded-live-gate-release-preview [--preview-dir dir] [--out dir]",
        "                                    Consume Approved manual record and generate a non-dispatching manual gate release fact preview",
        "  binance-guarded-live-pre-dispatch-dry-run [--preview-dir dir] [--config path] [--out dir]",
        "                                    Run fail-closed pre-dispatch dry run after manual gate release preview",
        "  binance-guarded-live-dispatch [--preview-dir dir] [--config path] [--out dir] --i-understand-live-orders",
        "                                    live-exec only: submit the approved GuardedLive BTCUSDT order and query private confirmation",
        "  binance-guarded-live-auto-once [--config path] [--out dir] [--max-ask price] [--execute-live --i-understand-auto-live-orders]",
        "                                    Single-leg smoke path: fetch fresh Binance BTCUSDT spot quote, then dry-run or explicitly submit live",
        "  binance-basis-guarded-live-auto-once [--config path] [--out dir] [--min-net-bps 5] [--max-spot-ask price --min-perp-bid price] [--spot-wss-monitor-url url --perp-wss-monitor-url url] [--private-order-events-dir dir] [--execute-live --i-understand-basis-live-orders]",
        "                                    Two-leg basis path: buy Binance Spot and short Binance USD-M perp after guarded live gates; live mode requires WSS monitor quotes",
        "  binance-wss-book-ticker [--bind 127.0.0.1:8801] [--symbol ALL_USDT|BTCUSDT] [--market spot|usdm-perp] [--reconnect-delay-secs 2] [--once --updates 3]",
        "                                    Run Binance public WSS bookTicker all-market monitor and serve /dashboard",
        "  binance-basis-monitor [--bind 127.0.0.1:8796] [--interval-secs 5] [--min-abs-funding-rate 0] [--min-net-bps 5] [--once] [--out dir]",
        "                                    Monitor all Binance public USDT spot/perp basis rows and serve /api/basis/status",
        "  bybit-basis-monitor [--bind 127.0.0.1:8797] [--interval-secs 5] [--min-abs-funding-rate 0] [--min-net-bps 5] [--once] [--out dir]",
        "                                    Monitor all Bybit public USDT spot/perp basis rows and serve /api/bybit-basis/status",
        "  okx-basis-monitor [--bind 127.0.0.1:8798] [--interval-secs 5] [--min-abs-funding-rate 0] [--min-net-bps 5] [--once] [--out dir]",
        "                                    Monitor all OKX public USDT spot/perp basis rows and serve /api/okx-basis/status",
        "  hyperliquid-basis-monitor [--bind 127.0.0.1:8799] [--interval-secs 5] [--min-abs-funding-rate 0] [--min-net-bps 5] [--once] [--out dir]",
        "                                    Monitor all Hyperliquid public USDC spot/perp basis rows and serve /api/hyperliquid-basis/status",
        "  aster-basis-monitor [--bind 127.0.0.1:8800] [--interval-secs 5] [--min-abs-funding-rate 0] [--min-net-bps 5] [--once] [--out dir]",
        "                                    Monitor all Aster public USDT spot/perp basis rows and serve /api/aster-basis/status",
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

type BinanceBasisPipelineCliOptions = BinanceBasisScanCliOptions;
struct BinanceGuardedLivePreviewCliOptions {
    market_artifacts_dir: PathBuf,
    output_dir: Option<PathBuf>,
    decision_request: Option<BinanceManualApprovalDecisionRequest>,
}
struct BinanceManualGateReleasePreviewCliOptions {
    preview_dir: PathBuf,
    output_dir: Option<PathBuf>,
}
struct BinancePreDispatchDryRunCliOptions {
    preview_dir: PathBuf,
    config_path: PathBuf,
    output_dir: Option<PathBuf>,
}
type BinanceGuardedLiveDispatchCliOptions = BinanceGuardedLiveDispatchOptions;
type BinanceGuardedLiveAutoOnceCliOptions = BinanceGuardedLiveAutoOnceOptions;
type BinanceBasisGuardedLiveAutoOnceCliOptions = BinanceBasisGuardedLiveAutoOnceOptions;
type BinanceBasisMonitorCliOptions = BinanceBasisMonitorOptions;
type BinanceWssBookTickerCliOptions = BinanceWssBookTickerMonitorOptions;
type BybitBasisMonitorCliOptions = BybitBasisMonitorOptions;
type OkxBasisMonitorCliOptions = OkxBasisMonitorOptions;
type HyperliquidBasisMonitorCliOptions = HyperliquidBasisMonitorOptions;
type AsterBasisMonitorCliOptions = AsterBasisMonitorOptions;

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

fn parse_binance_basis_pipeline_args(
    args: &[String],
) -> RuntimeResult<BinanceBasisPipelineCliOptions> {
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
                    "unknown binance-basis-pipeline option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-basis-pipeline positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceBasisScanCliOptions { symbol, output_dir })
}

fn parse_binance_guarded_live_preview_args(
    args: &[String],
) -> RuntimeResult<BinanceGuardedLivePreviewCliOptions> {
    let mut market_artifacts_dir = PathBuf::from(BINANCE_BASIS_PIPELINE_DEFAULT_OUT);
    let mut output_dir = None;
    let mut decision = None;
    let mut expected_plan_hash = None;
    let mut approval_event_id = None;
    let mut reviewer_id = None;
    let mut decided_at = None;
    let mut expires_at = None;
    let mut reason = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--market-artifacts" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--market-artifacts requires a directory"));
                };
                market_artifacts_dir = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--decision" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--decision requires approve or reject"));
                };
                decision = Some(parse_manual_approval_decision(value)?);
            }
            "--expected-plan-hash" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--expected-plan-hash requires a value"));
                };
                expected_plan_hash = Some(value.clone());
            }
            "--approval-event-id" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--approval-event-id requires a value"));
                };
                approval_event_id = Some(value.clone());
            }
            "--reviewer" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reviewer requires a value"));
                };
                reviewer_id = Some(value.clone());
            }
            "--decided-at" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--decided-at requires an RFC3339 UTC timestamp",
                    ));
                };
                decided_at = Some(value.clone());
            }
            "--expires-at" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--expires-at requires an RFC3339 UTC timestamp",
                    ));
                };
                expires_at = Some(value.clone());
            }
            "--reason" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reason requires a value"));
                };
                reason = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-guarded-live-preview option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-guarded-live-preview positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    let decision_request = if let Some(decision) = decision {
        Some(BinanceManualApprovalDecisionRequest {
            decision,
            expected_plan_hash: expected_plan_hash.ok_or_else(|| {
                cli_arg_error("--expected-plan-hash is required when --decision is supplied")
            })?,
            approval_event_id: approval_event_id.ok_or_else(|| {
                cli_arg_error("--approval-event-id is required when --decision is supplied")
            })?,
            reviewer_id: reviewer_id
                .ok_or_else(|| cli_arg_error("--reviewer is required when --decision is supplied"))?,
            decided_at: decided_at.ok_or_else(|| {
                cli_arg_error("--decided-at is required when --decision is supplied")
            })?,
            expires_at: expires_at.ok_or_else(|| {
                cli_arg_error("--expires-at is required when --decision is supplied")
            })?,
            reason: reason.unwrap_or_else(|| {
                "Manual approval decision recorded for Binance BTCUSDT GuardedLivePersonal plan preview."
                    .to_owned()
            }),
        })
    } else {
        if expected_plan_hash.is_some()
            || approval_event_id.is_some()
            || reviewer_id.is_some()
            || decided_at.is_some()
            || expires_at.is_some()
            || reason.is_some()
        {
            return Err(cli_arg_error(
                "manual approval fields require --decision approve|reject",
            ));
        }
        None
    };

    Ok(BinanceGuardedLivePreviewCliOptions {
        market_artifacts_dir,
        output_dir,
        decision_request,
    })
}

fn parse_manual_approval_decision(value: &str) -> RuntimeResult<ManualApprovalDecision> {
    match value {
        "approve" | "Approve" | "approved" | "Approved" => Ok(ManualApprovalDecision::Approve),
        "reject" | "Reject" | "rejected" | "Rejected" => Ok(ManualApprovalDecision::Reject),
        other => Err(cli_arg_error(format!(
            "--decision expected approve or reject, got `{other}`"
        ))),
    }
}

fn parse_binance_manual_gate_release_preview_args(
    args: &[String],
) -> RuntimeResult<BinanceManualGateReleasePreviewCliOptions> {
    let mut preview_dir = PathBuf::from(BINANCE_GUARDED_LIVE_PREVIEW_DEFAULT_OUT);
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--preview-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--preview-dir requires a directory"));
                };
                preview_dir = PathBuf::from(value);
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
                    "unknown binance-guarded-live-gate-release-preview option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-guarded-live-gate-release-preview positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceManualGateReleasePreviewCliOptions {
        preview_dir,
        output_dir,
    })
}

fn parse_binance_pre_dispatch_dry_run_args(
    args: &[String],
) -> RuntimeResult<BinancePreDispatchDryRunCliOptions> {
    let mut preview_dir = PathBuf::from(BINANCE_GUARDED_LIVE_PREVIEW_DEFAULT_OUT);
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--preview-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--preview-dir requires a directory"));
                };
                preview_dir = PathBuf::from(value);
            }
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
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
                    "unknown binance-guarded-live-pre-dispatch-dry-run option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-guarded-live-pre-dispatch-dry-run positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinancePreDispatchDryRunCliOptions {
        preview_dir,
        config_path,
        output_dir,
    })
}

fn parse_binance_guarded_live_dispatch_args(
    args: &[String],
) -> RuntimeResult<BinanceGuardedLiveDispatchCliOptions> {
    let mut preview_dir = PathBuf::from(BINANCE_GUARDED_LIVE_PREVIEW_DEFAULT_OUT);
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut acknowledge_live_orders = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--preview-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--preview-dir requires a directory"));
                };
                preview_dir = PathBuf::from(value);
            }
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--i-understand-live-orders" => {
                acknowledge_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-guarded-live-dispatch option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-guarded-live-dispatch positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceGuardedLiveDispatchOptions {
        preview_dir,
        config_path,
        output_dir,
        acknowledge_live_orders,
    })
}

fn parse_binance_guarded_live_auto_once_args(
    args: &[String],
) -> RuntimeResult<BinanceGuardedLiveAutoOnceCliOptions> {
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut max_ask = None;
    let mut execute_live = false;
    let mut acknowledge_auto_live_orders = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--max-ask" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--max-ask requires a price"));
                };
                Decimal::from_str(value)?;
                max_ask = Some(value.clone());
            }
            "--execute-live" => {
                execute_live = true;
            }
            "--dry-run" => {
                execute_live = false;
            }
            "--i-understand-auto-live-orders" => {
                acknowledge_auto_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-guarded-live-auto-once option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-guarded-live-auto-once positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceGuardedLiveAutoOnceOptions {
        config_path,
        output_dir,
        max_ask,
        execute_live,
        acknowledge_auto_live_orders,
    })
}

fn parse_binance_basis_guarded_live_auto_once_args(
    args: &[String],
) -> RuntimeResult<BinanceBasisGuardedLiveAutoOnceCliOptions> {
    let mut config_path = PathBuf::from("templates/personal_guarded_live.preflight.yaml");
    let mut output_dir = None;
    let mut min_net_bps = BASIS_MONITOR_DEFAULT_MIN_NET_BPS;
    let mut max_spot_ask = None;
    let mut min_perp_bid = None;
    let mut spot_wss_monitor_url = None;
    let mut perp_wss_monitor_url = None;
    let mut private_order_events_dir = None;
    let mut execute_live = false;
    let mut acknowledge_basis_live_orders = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--config requires a config path"));
                };
                config_path = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--out requires a directory"));
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--min-net-bps" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-net-bps requires an integer"));
                };
                min_net_bps = value.parse::<i128>().map_err(|error| {
                    cli_arg_error(format!("--min-net-bps must be an integer: {error}"))
                })?;
            }
            "--max-spot-ask" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--max-spot-ask requires a price"));
                };
                Decimal::from_str(value)?;
                max_spot_ask = Some(value.clone());
            }
            "--min-perp-bid" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--min-perp-bid requires a price"));
                };
                Decimal::from_str(value)?;
                min_perp_bid = Some(value.clone());
            }
            "--spot-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--spot-wss-monitor-url requires a URL"));
                };
                spot_wss_monitor_url = Some(value.clone());
            }
            "--perp-wss-monitor-url" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--perp-wss-monitor-url requires a URL"));
                };
                perp_wss_monitor_url = Some(value.clone());
            }
            "--private-order-events-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error(
                        "--private-order-events-dir requires a directory",
                    ));
                };
                private_order_events_dir = Some(PathBuf::from(value));
            }
            "--execute-live" => {
                execute_live = true;
            }
            "--dry-run" => {
                execute_live = false;
            }
            "--i-understand-basis-live-orders" => {
                acknowledge_basis_live_orders = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-basis-guarded-live-auto-once option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-basis-guarded-live-auto-once positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    Ok(BinanceBasisGuardedLiveAutoOnceOptions {
        config_path,
        output_dir,
        min_net_bps,
        max_spot_ask,
        min_perp_bid,
        spot_wss_monitor_url,
        perp_wss_monitor_url,
        private_order_events_dir,
        execute_live,
        acknowledge_basis_live_orders,
    })
}

fn parse_binance_wss_book_ticker_args(
    args: &[String],
) -> RuntimeResult<BinanceWssBookTickerCliOptions> {
    let mut options = BinanceWssBookTickerProbeOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--bind requires host:port"));
                };
                options.bind_addr = value.clone();
            }
            "--symbol" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--symbol requires a value"));
                };
                options.symbol = value.clone();
            }
            "--market" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--market requires spot or usdm-perp"));
                };
                options.market = parse_binance_public_wss_market(value)?;
            }
            "--updates" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--updates requires a value"));
                };
                options.updates = value
                    .parse::<usize>()
                    .map_err(|_| cli_arg_error("--updates must be an integer"))?;
            }
            "--reconnect-delay-secs" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(cli_arg_error("--reconnect-delay-secs requires a value"));
                };
                options.reconnect_delay_secs = value
                    .parse::<u64>()
                    .map_err(|_| cli_arg_error("--reconnect-delay-secs must be an integer"))?;
            }
            "--once" => {
                options.once = true;
            }
            value if value.starts_with('-') => {
                return Err(cli_arg_error(format!(
                    "unknown binance-wss-book-ticker option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected binance-wss-book-ticker positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_binance_wss_probe_options(&options)?;
    Ok(options)
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

fn parse_bybit_basis_monitor_args(args: &[String]) -> RuntimeResult<BybitBasisMonitorCliOptions> {
    let mut options = BybitBasisMonitorOptions::default();
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
                    "unknown bybit-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected bybit-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_bybit_monitor_options(&options)?;
    Ok(options)
}

fn parse_okx_basis_monitor_args(args: &[String]) -> RuntimeResult<OkxBasisMonitorCliOptions> {
    let mut options = OkxBasisMonitorOptions::default();
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
                    "unknown okx-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected okx-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_okx_monitor_options(&options)?;
    Ok(options)
}

fn parse_hyperliquid_basis_monitor_args(
    args: &[String],
) -> RuntimeResult<HyperliquidBasisMonitorCliOptions> {
    let mut options = HyperliquidBasisMonitorOptions::default();
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
                    "unknown hyperliquid-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected hyperliquid-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_hyperliquid_monitor_options(&options)?;
    Ok(options)
}

fn parse_aster_basis_monitor_args(args: &[String]) -> RuntimeResult<AsterBasisMonitorCliOptions> {
    let mut options = AsterBasisMonitorOptions::default();
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
                    "unknown aster-basis-monitor option `{value}`"
                )));
            }
            value => {
                return Err(cli_arg_error(format!(
                    "unexpected aster-basis-monitor positional argument `{value}`"
                )));
            }
        }
        index += 1;
    }

    validate_aster_monitor_options(&options)?;
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

    fn monitor_book_ticker_row(symbol: &str) -> MonitorBookTickerRow {
        MonitorBookTickerRow {
            symbol: symbol.to_owned(),
            bid_price: "100.01".to_owned(),
            bid_qty: "1.2".to_owned(),
            ask_price: "100.02".to_owned(),
            ask_qty: "1.3".to_owned(),
        }
    }

    fn binance_wss_test_market_state(
        symbol: &str,
        all_symbols_scope: bool,
    ) -> BinanceWssBookTickerAllMarketState {
        let market = BinancePublicMarket::Spot;
        let venue_id = binance_public_wss_venue_id(market).expect("venue id");
        let instrument = binance_public_wss_instrument(symbol, market).expect("instrument");
        let started_at = UtcTimestamp::from_str("2026-05-13T00:00:00Z").expect("time");
        let coordinator = RestWssMarketDataCoordinator::new(
            venue_id.clone(),
            instrument.instrument_id,
            started_at,
            MARKET_DATA_MAX_AGE_MS,
        )
        .expect("coordinator");
        let mut coordinators = BTreeMap::new();
        coordinators.insert(symbol.to_owned(), coordinator);
        let mut local_sequences = BTreeMap::new();
        local_sequences.insert(symbol.to_owned(), 1);

        BinanceWssBookTickerAllMarketState {
            venue_id,
            stream_url: "wss://example.test/ws".to_owned(),
            all_symbols_scope,
            coordinators,
            local_sequences,
            last_exchange_update_ids: BTreeMap::new(),
            rest_updates: Vec::new(),
        }
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
    fn binance_wss_book_ticker_args_parse_market_and_updates() {
        let args = vec![
            "--bind".to_owned(),
            "127.0.0.1:9901".to_owned(),
            "--symbol".to_owned(),
            "ethusdt".to_owned(),
            "--market".to_owned(),
            "usdm-perp".to_owned(),
            "--updates".to_owned(),
            "2".to_owned(),
            "--reconnect-delay-secs".to_owned(),
            "7".to_owned(),
            "--once".to_owned(),
        ];

        let options = parse_binance_wss_book_ticker_args(&args).expect("options");

        assert_eq!(options.bind_addr, "127.0.0.1:9901");
        assert_eq!(options.symbol, "ethusdt");
        assert_eq!(options.market, BinancePublicMarket::UsdmPerpetual);
        assert_eq!(options.updates, 2);
        assert_eq!(options.reconnect_delay_secs, 7);
        assert!(options.once);
    }

    #[test]
    fn binance_wss_book_ticker_defaults_to_all_usdt_scope() {
        let options = parse_binance_wss_book_ticker_args(&[]).expect("options");

        assert_eq!(options.symbol, BINANCE_WSS_BOOK_TICKER_ALL_USDT_SYMBOLS);
    }

    #[test]
    fn binance_wss_full_market_rest_rows_skip_unsupported_symbols() {
        let rows = prepare_binance_wss_book_ticker_rest_rows(
            vec![
                monitor_book_ticker_row("ETHUSDT"),
                monitor_book_ticker_row("BTCUSDT_260327"),
                monitor_book_ticker_row("ETHBTC"),
                monitor_book_ticker_row("btcusdt"),
            ],
            true,
        )
        .expect("full market rows");
        let symbols = rows
            .iter()
            .map(|row| row.symbol.as_str())
            .collect::<Vec<_>>();

        assert_eq!(symbols, vec!["BTCUSDT", "ETHUSDT"]);
    }

    #[test]
    fn binance_wss_single_symbol_rest_rows_remain_strict() {
        let error = prepare_binance_wss_book_ticker_rest_rows(
            vec![monitor_book_ticker_row("BTCUSDT_260327")],
            false,
        )
        .expect_err("unsupported single symbol must fail closed");

        assert!(error.to_string().contains("uppercase ASCII"));
    }

    #[test]
    fn binance_wss_full_market_messages_skip_unsupported_symbols() {
        let mut state = binance_wss_test_market_state("BTCUSDT", true);
        let raw = r#"{"stream":"btcusdt_260327@bookTicker","data":{"u":400900302,"s":"BTCUSDT_260327","b":"43250.10","B":"1.00000000","a":"43251.20","A":"1.50000000"}}"#;
        let ingested_at = UtcTimestamp::from_str("2026-05-13T00:00:01Z").expect("time");

        let update = apply_binance_wss_book_ticker_text(
            raw,
            ingested_at,
            BinancePublicMarket::Spot,
            &mut state,
        )
        .expect("unsupported all-market symbol is skipped");

        assert!(update.is_none());
        assert_eq!(state.local_sequences.get("BTCUSDT"), Some(&1));
    }

    #[test]
    fn binance_wss_single_symbol_messages_remain_strict() {
        let mut state = binance_wss_test_market_state("BTCUSDT", false);
        let raw = r#"{"stream":"btcusdt_260327@bookTicker","data":{"u":400900302,"s":"BTCUSDT_260327","b":"43250.10","B":"1.00000000","a":"43251.20","A":"1.50000000"}}"#;
        let ingested_at = UtcTimestamp::from_str("2026-05-13T00:00:01Z").expect("time");

        let error = apply_binance_wss_book_ticker_text(
            raw,
            ingested_at,
            BinancePublicMarket::Spot,
            &mut state,
        )
        .expect_err("unsupported single-symbol WSS message must fail closed");

        assert!(error.to_string().contains("uppercase ASCII"));
    }

    #[test]
    fn binance_wss_book_ticker_parses_combined_stream_payload() {
        let raw = r#"{"stream":"btcusdt@bookTicker","data":{"u":400900301,"s":"BTCUSDT","b":"43250.10","B":"1.00000000","a":"43251.20","A":"1.50000000"}}"#;
        let ingested_at = UtcTimestamp::from_str("2026-05-13T00:00:00Z").expect("time");

        let parsed = parse_binance_wss_book_ticker_runtime_raw(raw, ingested_at).expect("raw");

        assert_eq!(parsed.symbol, "BTCUSDT");
        assert_eq!(parsed.update_id, 400900301);
        assert_eq!(parsed.best_bid.to_string(), "43250.10");
        assert_eq!(parsed.ask_size.to_string(), "1.50000000");
    }

    #[test]
    fn binance_wss_book_ticker_all_market_stream_urls_match_market_shape() {
        let symbols = vec!["BTCUSDT".to_owned(), "ETHUSDT".to_owned()];

        let spot =
            binance_wss_book_ticker_all_market_stream_url(BinancePublicMarket::Spot, &symbols)
                .expect("spot url");
        let usdm = binance_wss_book_ticker_all_market_stream_url(
            BinancePublicMarket::UsdmPerpetual,
            &symbols,
        )
        .expect("usdm url");

        assert!(spot.contains("/stream?streams=btcusdt@bookTicker/ethusdt@bookTicker"));
        assert_eq!(usdm, "wss://fstream.binance.com/public/ws/!bookTicker");
    }

    #[test]
    fn binance_wss_monitor_snapshot_exposes_health_quote_and_counters() {
        let mut snapshot = BinanceWssBookTickerMonitorSnapshot::empty(
            "BTCUSDT",
            BinancePublicMarket::Spot,
            "wss://example.test/ws/btcusdt@bookTicker",
        );
        snapshot.latest_quote = Some(BinanceWssBookTickerQuoteSnapshot {
            symbol: "BTCUSDT".to_owned(),
            venue_id: "venue:BINANCE-SPOT".to_owned(),
            instrument_id: "inst:BINANCE:BTCUSDT:SPOT".to_owned(),
            best_bid: Some("100.01".to_owned()),
            best_ask: Some("100.02".to_owned()),
            bid_size: Some("1.2".to_owned()),
            ask_size: Some("1.3".to_owned()),
            source_sequence: Some("42".to_owned()),
            source_event_id: Some("binance:wss:spot:BTCUSDT:42".to_owned()),
            observed_at: "2026-05-13T00:00:00.000000000Z".to_owned(),
            ingested_at: "2026-05-13T00:00:00.000000000Z".to_owned(),
            freshness_status: "Fresh".to_owned(),
        });
        snapshot.record_failure("forced fail closed for test", true);
        snapshot.begin_rest_rebuild();

        let health = snapshot.health_json();
        let quote = snapshot.quote_json();
        let status = snapshot.to_json();

        assert!(health.contains("\"fail_closed\":true"));
        assert!(health.contains("\"disconnect_count\":1"));
        assert!(health.contains("\"rest_rebuild_count\":1"));
        assert!(quote.contains("\"latest_quote\""));
        assert!(quote.contains("\"best_bid\":\"100.01\""));
        assert!(status.contains("\"last_error\":\"forced fail closed for test\""));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_basis_live_wss_monitor_quote_becomes_book_ticker_input() {
        let raw = r#"{
          "status":"streaming",
          "market":"Spot",
          "fail_closed":false,
          "wss_update_count":3,
          "last_error":null,
          "rows":[{
            "symbol":"BTCUSDT",
            "venue_id":"venue:BINANCE-SPOT",
            "instrument_id":"inst:BINANCE:BTCUSDT:SPOT",
            "best_bid":"100.01",
            "best_ask":"100.02",
            "bid_size":"1.2",
            "ask_size":"1.3",
            "source_sequence":"2",
            "source_event_id":"event:venue-data:binance-public:wss-book-ticker:spot:BTCUSDT:2:u400900301",
            "observed_at":"2026-05-13T00:00:01Z",
            "ingested_at":"2026-05-13T00:00:01Z",
            "freshness_status":"Fresh"
          }]
        }"#;

        let quote = parse_binance_wss_monitor_quote_for_basis(
            raw,
            "http://127.0.0.1:8801/api/binance-wss-book-ticker/status",
            BASIS_SYMBOL,
            BinancePublicMarket::Spot,
            BASIS_SPOT_VENUE_ID,
            BASIS_SPOT_INSTRUMENT_ID,
            UtcTimestamp::from_str("2026-05-13T00:00:02Z").expect("fetched at"),
        )
        .expect("monitor quote");
        let book = binance_wss_monitor_quote_to_book_ticker_json(&quote).expect("book ticker json");

        assert!(book.contains("\"symbol\":\"BTCUSDT\""));
        assert!(book.contains("\"bidPrice\":\"100.01\""));
        assert!(book.contains("\"askPrice\":\"100.02\""));
        assert!(book.contains("\"time\":1778630401000"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_basis_live_wss_monitor_rejects_rest_bootstrap_quote() {
        let raw = r#"{
          "status":"streaming",
          "market":"Spot",
          "fail_closed":false,
          "wss_update_count":1,
          "last_error":null,
          "latest_quote":{
            "symbol":"BTCUSDT",
            "venue_id":"venue:BINANCE-SPOT",
            "instrument_id":"inst:BINANCE:BTCUSDT:SPOT",
            "best_bid":"100.01",
            "best_ask":"100.02",
            "bid_size":"1.2",
            "ask_size":"1.3",
            "source_sequence":"1",
            "source_event_id":"event:venue-data:binance-public:book-ticker:spot:BTCUSDT:1:raw",
            "observed_at":"2026-05-13T00:00:01Z",
            "ingested_at":"2026-05-13T00:00:01Z",
            "freshness_status":"Fresh"
          }
        }"#;

        let error = parse_binance_wss_monitor_quote_for_basis(
            raw,
            "http://127.0.0.1:8801/api/binance-wss-book-ticker/status",
            BASIS_SYMBOL,
            BinancePublicMarket::Spot,
            BASIS_SPOT_VENUE_ID,
            BASIS_SPOT_INSTRUMENT_ID,
            UtcTimestamp::from_str("2026-05-13T00:00:02Z").expect("fetched at"),
        )
        .expect_err("REST bootstrap quote must be rejected for live execution");

        assert!(error.to_string().contains("not from WSS bookTicker"));
    }

    #[test]
    fn binance_wss_book_ticker_dashboard_requests_realtime_api_paths() {
        let html = binance_wss_book_ticker_dashboard_html();

        assert!(html.contains("Binance public WSS"));
        assert!(html.contains("/health"));
        assert!(html.contains("/api/binance-wss-book-ticker/quote"));
        assert!(html.contains("/api/binance-wss-book-ticker/status"));
        assert!(html.contains("/api/binance-wss-book-ticker/quotes"));
        assert!(html.contains("id=\"metric-rebuilds\""));
        assert!(html.contains("id=\"quote-bid\""));
        assert!(html.contains("id=\"quote-rows\""));
    }

    #[test]
    fn binance_wss_probe_instrument_maps_usdt_symbol_without_private_surface() {
        let instrument = binance_public_wss_instrument("ETHUSDT", BinancePublicMarket::Spot)
            .expect("instrument");

        assert_eq!(instrument.symbol, "ETHUSDT");
        assert_eq!(
            instrument.instrument_id.as_str(),
            "inst:BINANCE:ETHUSDT:SPOT"
        );
        assert_eq!(instrument.base_asset_id.as_str(), "asset:ETH");
        assert_eq!(instrument.quote_asset_id.as_str(), "asset:USDT");

        let error = binance_public_wss_instrument("ETHBTC", BinancePublicMarket::Spot)
            .expect_err("non-USDT symbol is not mapped by this probe");
        assert!(error.to_string().contains("USDT-quoted"));
    }

    #[test]
    fn bybit_basis_dashboard_html_requests_bybit_realtime_api_paths() {
        let html = bybit_basis_dashboard_html();

        assert!(html.contains("Bybit public basis"));
        assert!(html.contains("/api/bybit-basis/status"));
        assert!(html.contains("/api/bybit-basis/opportunities"));
        assert!(html.contains("id=\"basis-rows\""));
    }

    #[test]
    fn okx_basis_dashboard_html_requests_okx_realtime_api_paths() {
        let html = okx_basis_dashboard_html();

        assert!(html.contains("OKX public basis"));
        assert!(html.contains("/api/okx-basis/status"));
        assert!(html.contains("/api/okx-basis/opportunities"));
        assert!(html.contains("id=\"basis-rows\""));
    }

    #[test]
    fn hyperliquid_basis_dashboard_html_requests_hyperliquid_realtime_api_paths() {
        let html = hyperliquid_basis_dashboard_html();

        assert!(html.contains("Hyperliquid public basis"));
        assert!(html.contains("/api/hyperliquid-basis/status"));
        assert!(html.contains("/api/hyperliquid-basis/opportunities"));
        assert!(html.contains("Spot Mid"));
        assert!(html.contains("id=\"basis-rows\""));
    }

    #[test]
    fn aster_basis_dashboard_html_requests_aster_realtime_api_paths() {
        let html = aster_basis_dashboard_html();

        assert!(html.contains("Aster public basis"));
        assert!(html.contains("/api/aster-basis/status"));
        assert!(html.contains("/api/aster-basis/opportunities"));
        assert!(html.contains("id=\"basis-rows\""));
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
    fn binance_basis_pipeline_promotes_public_signal_to_risk_fact() {
        let spot = r#"{"symbol":"BTCUSDT","bidPrice":"99.90","bidQty":"1.0","askPrice":"100.00","askQty":"2.0"}"#;
        let perp = r#"{"symbol":"BTCUSDT","bidPrice":"101.00","bidQty":"1.5","askPrice":"101.10","askQty":"2.5","time":1778630400000}"#;
        let premium = r#"{"symbol":"BTCUSDT","markPrice":"101.00","indexPrice":"100.00","lastFundingRate":"0.00010000","interestRate":"0.00010000","nextFundingTime":1778659200000,"time":1778630400000}"#;
        let replay = arb_replay::load_fixture(full_pipeline_fixture_root()).expect("fixture");
        let ingested_at = UtcTimestamp::from_str("2026-05-13T00:00:00Z").expect("time");

        let artifacts = assemble_binance_basis_pipeline_from_raw_json(
            &replay,
            BinanceBasisRawInputs {
                symbol: "BTCUSDT",
                raw_spot_book: spot,
                spot_book_ref: "test:binance-spot-book",
                raw_perp_book: perp,
                perp_book_ref: "test:binance-usdm-book",
                raw_premium_index: premium,
                premium_index_ref: "test:binance-usdm-premium",
            },
            ingested_at,
        )
        .expect("basis pipeline artifacts");

        assert!(artifacts.stored_events_jsonl.contains("venue:BINANCE-SPOT"));
        assert!(artifacts.stored_events_jsonl.contains("venue:BINANCE-USDM"));
        assert!(artifacts
            .candidate_transitions_jsonl
            .contains("trans:binance-basis-btcusdt-001"));
        assert!(!artifacts.risk_decisions_jsonl.is_empty());
        assert!(artifacts.risk_decisions_jsonl.contains("\"Rejected\""));
        assert!(artifacts.incidents_jsonl.contains("incident:"));
        assert!(artifacts.execution_plans_jsonl.is_empty());
        assert!(artifacts
            .operations_daily_report_md
            .contains("Read-only mode: true"));
    }

    #[test]
    fn bybit_basis_pipeline_promotes_normalized_events_to_risk_fact() {
        let replay = arb_replay::load_fixture(full_pipeline_fixture_root()).expect("fixture");
        let ingested_at = UtcTimestamp::from_str("2026-05-13T00:00:00Z").expect("time");
        let events = vec![
            from_json_strict::<NormalizedEvent>(
                r#"{"checksum":"sha256:test-bybit-basis-spot-book","correlation_id":"corr:bybit-basis-001","event_id":"event:test:bybit-basis:spot-book:001","event_type":"NormalizedMarketDataEvent","event_version":"1.0.0","instrument_id":"inst:BYBIT:BTCUSDT:SPOT","payload":{"adapter":"bybit-public-rest","ask_size":"2.0","basis_role":"Spot","best_ask":"100.00","best_bid":"99.90","bid_size":"1.0","freshness":"Fresh","kind":"BookTicker","market":"Spot","risk_reason_code":"OK","venue_symbol":"BTCUSDT"},"schema_version":"1.0.0","source":"bybit-public-rest","source_sequence":"spot-book-001","timestamp_event":"2026-05-13T00:00:00Z","timestamp_ingested":"2026-05-13T00:00:00Z","venue_id":"venue:BYBIT-SPOT"}"#,
            )
            .expect("spot event"),
            from_json_strict::<NormalizedEvent>(
                r#"{"checksum":"sha256:test-bybit-basis-perp-book","correlation_id":"corr:bybit-basis-001","event_id":"event:test:bybit-basis:perp-book:001","event_type":"NormalizedMarketDataEvent","event_version":"1.0.0","instrument_id":"inst:BYBIT:BTCUSDT:LINEAR-PERP","payload":{"adapter":"bybit-public-rest","ask_size":"2.5","basis_role":"Perp","best_ask":"101.10","best_bid":"101.00","bid_size":"1.5","freshness":"Fresh","kind":"BookTicker","market":"LinearPerp","risk_reason_code":"OK","venue_symbol":"BTCUSDT"},"schema_version":"1.0.0","source":"bybit-public-rest","source_sequence":"perp-book-001","timestamp_event":"2026-05-13T00:00:00Z","timestamp_ingested":"2026-05-13T00:00:00Z","venue_id":"venue:BYBIT-LINEAR"}"#,
            )
            .expect("perp event"),
            from_json_strict::<NormalizedEvent>(
                r#"{"checksum":"sha256:test-bybit-basis-premium","correlation_id":"corr:bybit-basis-001","event_id":"event:test:bybit-basis:premium:001","event_type":"NormalizedMarketDataEvent","event_version":"1.0.0","instrument_id":"inst:BYBIT:BTCUSDT:LINEAR-PERP","payload":{"adapter":"bybit-public-rest","basis_role":"Perp","funding_interval_hours":"8","index_price":"100.00","kind":"PerpPremiumIndex","last_funding_rate":"0.00010000","mark_price":"101.00","next_funding_time_ms":1778659200000,"risk_reason_code":"OK","venue_symbol":"BTCUSDT"},"schema_version":"1.0.0","source":"bybit-public-rest","source_sequence":"premium-001","timestamp_event":"2026-05-13T00:00:00Z","timestamp_ingested":"2026-05-13T00:00:00Z","venue_id":"venue:BYBIT-LINEAR"}"#,
            )
            .expect("premium event"),
        ];
        let spec = BasisPipelineSpec::bybit_btcusdt().expect("basis spec");

        let artifacts = assemble_public_basis_pipeline_from_normalized_events(
            &replay,
            &spec,
            events,
            ingested_at,
        )
        .expect("basis pipeline artifacts");

        assert!(artifacts.stored_events_jsonl.contains("venue:BYBIT-SPOT"));
        assert!(artifacts.stored_events_jsonl.contains("venue:BYBIT-LINEAR"));
        assert!(artifacts
            .candidate_transitions_jsonl
            .contains("trans:bybit-basis-btcusdt-001"));
        assert!(artifacts
            .candidate_transitions_jsonl
            .contains("strat:bybit-spot-perp-basis"));
        assert!(!artifacts.risk_decisions_jsonl.is_empty());
        assert!(artifacts.risk_decisions_jsonl.contains("\"Rejected\""));
        assert!(artifacts.execution_plans_jsonl.is_empty());
    }

    #[test]
    fn basis_pipeline_spec_rejects_missing_strategy_venue_capability() {
        let capabilities = load_bybit_basis_capabilities()
            .expect("capabilities")
            .into_iter()
            .filter(|capability| capability.venue_id.as_str() != "venue:BYBIT-LINEAR")
            .collect::<Vec<_>>();

        let error = BasisPipelineSpec::new(
            SpotPerpBasisStrategyConfig::bybit_btcusdt(),
            capabilities,
            "state:test:missing-bybit-linear-capability",
            "hash:test:missing-bybit-linear-capability",
        )
        .expect_err("missing perp venue capability must fail closed");

        assert!(error.to_string().contains("venue:BYBIT-LINEAR"));
    }

    #[test]
    fn binance_guarded_live_preview_writes_pending_manual_confirmation_artifacts() {
        let spot = r#"{"symbol":"BTCUSDT","bidPrice":"99.90","bidQty":"1.0","askPrice":"100.00","askQty":"2.0"}"#;
        let perp = r#"{"symbol":"BTCUSDT","bidPrice":"101.00","bidQty":"1.5","askPrice":"101.10","askQty":"2.5","time":1778630400000}"#;
        let premium = r#"{"symbol":"BTCUSDT","markPrice":"101.00","indexPrice":"100.00","lastFundingRate":"0.00010000","interestRate":"0.00010000","nextFundingTime":1778659200000,"time":1778630400000}"#;
        let replay = arb_replay::load_fixture(full_pipeline_fixture_root()).expect("fixture");
        let ingested_at = UtcTimestamp::from_str("2026-05-13T00:00:00Z").expect("time");
        let artifacts = assemble_binance_basis_pipeline_from_raw_json(
            &replay,
            BinanceBasisRawInputs {
                symbol: "BTCUSDT",
                raw_spot_book: spot,
                spot_book_ref: "test:binance-spot-book",
                raw_perp_book: perp,
                perp_book_ref: "test:binance-usdm-book",
                raw_premium_index: premium,
                premium_index_ref: "test:binance-usdm-premium",
            },
            ingested_at,
        )
        .expect("basis pipeline artifacts");
        let market_dir = RuntimeTempDir::new().expect("market dir");
        write_utf8(
            market_dir.path().join("stored_events.jsonl"),
            &artifacts.stored_events_jsonl,
        )
        .expect("stored events");
        let output_root = RuntimeTempDir::new().expect("output dir");
        let preview_dir = output_root.path().join("preview");

        let report =
            run_binance_guarded_live_preview(market_dir.path(), Some(preview_dir.clone()), None)
                .expect("guarded live preview");

        assert_eq!(report.symbol, "BTCUSDT");
        assert!(report.plan_hash.starts_with("hash:sha256:"));
        assert!(!report.dispatchable_before_approval);
        assert_eq!(report.approval_record_count, 0);
        assert_eq!(report.approval_status, None);
        assert_eq!(
            read_utf8(&preview_dir.join("plan_hash.txt"))
                .expect("plan hash")
                .trim(),
            report.plan_hash
        );
        let confirmation =
            read_utf8(&preview_dir.join("manual_confirmation_record.md")).expect("confirmation");
        assert!(confirmation.contains("人工确认状态：pending"));
        assert!(confirmation.contains(&report.plan_hash));
        assert!(read_utf8(&preview_dir.join("approval_records.jsonl"))
            .expect("approval records")
            .is_empty());
        assert!(read_utf8(&preview_dir.join("plan_preview.json"))
            .expect("plan preview")
            .contains("\"execution_mode\":\"ManualApproval\""));

        let approved = run_binance_guarded_live_preview(
            market_dir.path(),
            Some(preview_dir.clone()),
            Some(BinanceManualApprovalDecisionRequest {
                decision: ManualApprovalDecision::Approve,
                expected_plan_hash: report.plan_hash.clone(),
                approval_event_id: "event:approval:test:binance-btcusdt".to_owned(),
                reviewer_id: "owner:redacted".to_owned(),
                decided_at: "2026-05-13T00:01:00Z".to_owned(),
                expires_at: "2026-05-13T00:06:00Z".to_owned(),
                reason: "Owner approved the same test plan hash.".to_owned(),
            }),
        )
        .expect("approved guarded live preview");

        assert_eq!(approved.plan_hash, report.plan_hash);
        assert_eq!(approved.approval_record_count, 1);
        assert_eq!(approved.approval_status.as_deref(), Some("Approved"));
        assert!(read_utf8(&preview_dir.join("approval_records.jsonl"))
            .expect("approval records")
            .contains("\"status\":\"Approved\""));
        assert!(
            read_utf8(&preview_dir.join("manual_confirmation_record.md"))
                .expect("approved confirmation")
                .contains("人工确认状态：Approved")
        );

        let release =
            run_binance_manual_gate_release_preview(&preview_dir, Some(preview_dir.clone()))
                .expect("manual gate release preview");
        assert!(release.released_manual_gate);
        assert_eq!(release.plan_hash, report.plan_hash);
        assert_eq!(release.dependent_transition_count, 1);
        assert!(!release.dispatchable_after_release);
        assert!(
            read_utf8(&preview_dir.join("manual_gate_release_preview.json"))
                .expect("release preview")
                .contains("\"released_manual_gate\":true")
        );

        let config_path = output_root.path().join("guarded-live.yaml");
        fs::write(&config_path, guarded_live_blocked_by_kill_switch_yaml()).expect("write config");
        let dry_run =
            run_binance_pre_dispatch_dry_run(&preview_dir, &config_path, Some(preview_dir.clone()))
                .expect("pre-dispatch dry run");
        assert!(dry_run.manual_gate_released);
        assert!(!dry_run.dispatch_allowed);
        assert!(dry_run
            .blocking_reasons
            .iter()
            .any(|reason| reason.contains("kill switch")));
        assert!(read_utf8(&preview_dir.join("pre_dispatch_dry_run.json"))
            .expect("dry run")
            .contains("\"dispatch_allowed\":false"));
    }

    #[test]
    fn binance_guarded_live_auto_once_runs_fresh_signal_to_dry_run_gate() {
        let spot = r#"{"symbol":"BTCUSDT","bidPrice":"99.90","bidQty":"1.0","askPrice":"100.00","askQty":"2.0"}"#;
        let root = RuntimeTempDir::new().expect("output dir");
        let config_path = root.path().join("guarded-live.yaml");
        fs::write(&config_path, guarded_live_blocked_by_kill_switch_yaml()).expect("write config");
        let ingested_at = UtcTimestamp::from_str("2026-05-13T00:00:00Z").expect("time");

        let report = run_binance_guarded_live_auto_once_from_spot_json(
            spot,
            "test:binance-spot-book",
            ingested_at,
            BinanceGuardedLiveAutoOnceOptions {
                config_path,
                output_dir: Some(root.path().join("auto")),
                max_ask: Some("101.00".to_owned()),
                execute_live: false,
                acknowledge_auto_live_orders: false,
            },
        )
        .expect("auto once dry run");

        assert!(report.signal_allowed);
        assert!(report
            .plan_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("hash:sha256:")));
        assert!(report.manual_gate_released);
        assert!(!report.dispatch_attempted);
        assert!(!report.dispatch_allowed);
        assert!(report
            .blocking_reasons
            .iter()
            .any(|reason| reason.contains("dry-run mode")));
        let output_dir = report.output_dir.as_ref().expect("output dir");
        assert!(read_utf8(&output_dir.join("market/stored_events.jsonl"))
            .expect("market events")
            .contains("venue:BINANCE-SPOT"));
        assert!(
            read_utf8(&output_dir.join("preview/approval_records.jsonl"))
                .expect("approval")
                .contains("system:guarded-live-auto-strategy")
        );
        assert!(read_utf8(&output_dir.join("auto_once_report.json"))
            .expect("auto report")
            .contains("\"signal_allowed\":true"));
    }

    #[test]
    fn binance_guarded_live_auto_once_blocks_live_without_price_guard() {
        let spot = r#"{"symbol":"BTCUSDT","bidPrice":"99.90","bidQty":"1.0","askPrice":"100.00","askQty":"2.0"}"#;
        let root = RuntimeTempDir::new().expect("output dir");
        let ingested_at = UtcTimestamp::from_str("2026-05-13T00:00:00Z").expect("time");

        let report = run_binance_guarded_live_auto_once_from_spot_json(
            spot,
            "test:binance-spot-book",
            ingested_at,
            BinanceGuardedLiveAutoOnceOptions {
                config_path: root.path().join("unused.yaml"),
                output_dir: Some(root.path().join("auto")),
                max_ask: None,
                execute_live: true,
                acknowledge_auto_live_orders: true,
            },
        )
        .expect("blocked before live dispatch");

        assert!(!report.signal_allowed);
        assert!(!report.dispatch_attempted);
        assert!(report.plan_hash.is_none());
        assert!(report
            .blocking_reasons
            .iter()
            .any(|reason| reason.contains("--max-ask")));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_basis_guarded_live_auto_once_builds_two_leg_dry_run() {
        let spot = r#"{"symbol":"BTCUSDT","bidPrice":"99.90","bidQty":"1.0","askPrice":"100.00","askQty":"2.0"}"#;
        let perp = r#"{"symbol":"BTCUSDT","bidPrice":"101.00","bidQty":"1.5","askPrice":"101.10","askQty":"2.5","time":1778630400000}"#;
        let premium = r#"{"symbol":"BTCUSDT","markPrice":"101.00","indexPrice":"100.00","lastFundingRate":"0.00010000","interestRate":"0.00010000","nextFundingTime":1778659200000,"time":1778630400000}"#;
        let root = RuntimeTempDir::new().expect("output dir");
        let config_path = root.path().join("guarded-live.yaml");
        fs::write(&config_path, guarded_live_open_real_signing_yaml()).expect("write config");
        let ingested_at = UtcTimestamp::from_str("2026-05-13T00:00:00Z").expect("time");

        let report = run_binance_basis_guarded_live_auto_once_from_json(
            spot,
            "test:binance-spot-book",
            perp,
            "test:binance-usdm-book",
            premium,
            "test:binance-usdm-premium",
            ingested_at,
            BinanceBasisGuardedLiveAutoOnceOptions {
                config_path,
                output_dir: Some(root.path().join("basis-auto")),
                min_net_bps: 5,
                max_spot_ask: Some("101.00".to_owned()),
                min_perp_bid: Some("100.50".to_owned()),
                spot_wss_monitor_url: None,
                perp_wss_monitor_url: None,
                private_order_events_dir: None,
                execute_live: false,
                acknowledge_basis_live_orders: false,
            },
        )
        .expect("basis auto once dry run");

        assert!(report.signal_allowed);
        assert_eq!(report.planned_order_count, 2);
        assert!(report.manual_gate_released);
        assert!(!report.dispatch_attempted);
        assert!(!report.dispatch_allowed);
        assert!(report
            .blocking_reasons
            .iter()
            .any(|reason| reason.contains("dry-run mode")));
        let output_dir = report.output_dir.as_ref().expect("output dir");
        let plan_preview =
            read_utf8(&output_dir.join("preview/plan_preview.json")).expect("plan preview");
        assert!(plan_preview.contains("\"basis_leg_role\":\"spot_buy\""));
        assert!(plan_preview.contains("\"basis_leg_role\":\"perp_short\""));
        assert!(plan_preview.contains("\"client_order_id\":\"rvbS"));
        assert!(plan_preview.contains("\"client_order_id\":\"rvbP"));
        let report_json =
            read_utf8(&output_dir.join("basis_auto_once_report.json")).expect("auto report");
        assert!(report_json.contains("\"planned_order_count\":2"));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_basis_live_dispatch_unwinds_spot_when_perp_rejected_after_spot_fill() {
        let spot = test_basis_planned_order(
            "spot_buy",
            BASIS_SPOT_VENUE_ID,
            BASIS_SPOT_INSTRUMENT_ID,
            OrderSide::Buy,
            "0.100",
            "100.00",
            "rvbS1778630400",
            "idem:test:basis:spot",
        );
        let perp = test_basis_planned_order(
            "perp_short",
            BASIS_PERP_VENUE_ID,
            BASIS_PERP_INSTRUMENT_ID,
            OrderSide::Sell,
            "0.100",
            "101.00",
            "rvbP1778630400",
            "idem:test:basis:perp",
        );
        let dispatch_plan = ExecutionDispatchPlan {
            plan_id: "plan:test:binance-basis-protection".to_owned(),
            requests: vec![spot, perp],
        };
        let mut spot_adapter = ScriptedBasisLiveAdapter::new(
            vec![
                Ok(test_mutable_receipt(
                    MutableActionKind::SubmitOrder,
                    BASIS_SPOT_VENUE_ID,
                    "spot-submit",
                )),
                Ok(test_mutable_receipt(
                    MutableActionKind::SubmitOrder,
                    BASIS_SPOT_VENUE_ID,
                    "spot-unwind",
                )),
            ],
            vec![],
            vec![
                Ok(test_private_order_update(
                    arb_venue_exec::BinancePrivateOrderMarket::Spot,
                    BASIS_SPOT_VENUE_ID,
                    BASIS_SPOT_INSTRUMENT_ID,
                    OrderConfirmationStatus::Filled,
                    Some("0.100"),
                    Some(OrderSide::Buy),
                )),
                Ok(test_private_order_update(
                    arb_venue_exec::BinancePrivateOrderMarket::Spot,
                    BASIS_SPOT_VENUE_ID,
                    BASIS_SPOT_INSTRUMENT_ID,
                    OrderConfirmationStatus::Filled,
                    Some("0.100"),
                    Some(OrderSide::Sell),
                )),
            ],
        );
        let mut usdm_adapter = ScriptedBasisLiveAdapter::new(
            vec![Err(VenueExecError::ExternalRejected {
                venue_id: VenueId::new(BASIS_PERP_VENUE_ID).expect("venue"),
                endpoint: "/fapi/v1/order".to_owned(),
                status_code: 400,
                reason: "fixture perp reject".to_owned(),
            })],
            vec![],
            vec![],
        );

        let outcome = execute_binance_basis_live_dispatch(
            &dispatch_plan,
            &mut spot_adapter,
            &mut usdm_adapter,
            BinanceBasisLiveDispatchContext {
                plan: None,
                generated_at: "2026-05-13T00:00:00Z",
                spot_unwind_limit_price: Price::from_str("99.90").expect("unwind price"),
                protection_suffix: "1778630400",
                private_order_events: None,
            },
        )
        .expect("protected dispatch outcome");

        assert_eq!(outcome.primary_submit_receipt_count, 1);
        assert!(outcome.protection.attempted);
        assert_eq!(outcome.protection.receipt_count, 1);
        assert!(outcome.protection.residual_risk.is_none());
        assert!(outcome
            .protection
            .actions
            .iter()
            .any(|action| action.contains("spot_unwind_sell:perp-submit-failed")));
        assert_eq!(spot_adapter.submitted.len(), 2);
        assert_eq!(spot_adapter.submitted[1].side, OrderSide::Sell);
        assert_eq!(spot_adapter.submitted[1].quantity.to_string(), "0.100");
        assert_eq!(
            spot_adapter.submitted[1]
                .limit_price
                .expect("unwind limit")
                .to_string(),
            "99.90"
        );
        assert_eq!(usdm_adapter.submitted.len(), 1);
        assert!(outcome
            .blocking_reasons
            .iter()
            .any(|reason| reason.contains("perp hedge submit failed")));
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_basis_live_dispatch_prefers_private_stream_confirmations() {
        let spot = test_basis_planned_order(
            "spot_buy",
            BASIS_SPOT_VENUE_ID,
            BASIS_SPOT_INSTRUMENT_ID,
            OrderSide::Buy,
            "0.100",
            "100.00",
            "rvbS1778630400",
            "idem:test:basis:spot",
        );
        let perp = test_basis_planned_order(
            "perp_short",
            BASIS_PERP_VENUE_ID,
            BASIS_PERP_INSTRUMENT_ID,
            OrderSide::Sell,
            "0.100",
            "101.00",
            "rvbP1778630400",
            "idem:test:basis:perp",
        );
        let dispatch_plan = ExecutionDispatchPlan {
            plan_id: "plan:test:binance-basis-private-stream".to_owned(),
            requests: vec![spot, perp],
        };
        let temp = RuntimeTempDir::new().expect("private events dir");
        write_utf8(
            temp.path().join("spot_user_data.jsonl"),
            r#"{"e":"executionReport","E":1778630400000,"s":"BTCUSDT","c":"rvbS1778630400","S":"BUY","x":"TRADE","X":"FILLED","i":101,"l":"0.100","z":"0.100","L":"100.00","n":"0.01","N":"USDT","T":1778630400000}"#,
        )
        .expect("write spot private stream");
        write_utf8(
            temp.path().join("usdm_user_data.jsonl"),
            r#"{"e":"ORDER_TRADE_UPDATE","E":1778630400000,"o":{"s":"BTCUSDT","c":"rvbP1778630400","S":"SELL","x":"TRADE","X":"FILLED","i":202,"l":"0.100","z":"0.100","L":"101.00","n":"0.01","N":"USDT","T":1778630400000}}"#,
        )
        .expect("write usdm private stream");
        let private_events =
            BinancePrivateOrderEventStore::from_dir(temp.path()).expect("private events");
        let mut spot_adapter = ScriptedBasisLiveAdapter::new(
            vec![Ok(test_mutable_receipt(
                MutableActionKind::SubmitOrder,
                BASIS_SPOT_VENUE_ID,
                "spot-submit",
            ))],
            vec![],
            vec![],
        );
        let mut usdm_adapter = ScriptedBasisLiveAdapter::new(
            vec![Ok(test_mutable_receipt(
                MutableActionKind::SubmitOrder,
                BASIS_PERP_VENUE_ID,
                "perp-submit",
            ))],
            vec![],
            vec![],
        );

        let outcome = execute_binance_basis_live_dispatch(
            &dispatch_plan,
            &mut spot_adapter,
            &mut usdm_adapter,
            BinanceBasisLiveDispatchContext {
                plan: None,
                generated_at: "2026-05-13T00:00:00Z",
                spot_unwind_limit_price: Price::from_str("99.90").expect("unwind price"),
                protection_suffix: "1778630400",
                private_order_events: Some(&private_events),
            },
        )
        .expect("private stream dispatch outcome");

        assert_eq!(outcome.primary_submit_receipt_count, 2);
        assert!(outcome.blocking_reasons.is_empty());
        assert_eq!(outcome.confirmations.len(), 2);
        assert!(outcome
            .confirmations
            .iter()
            .all(|confirmation| confirmation.source
                == arb_execution::PrivateOrderConfirmationSource::PrivateStream));
    }

    #[cfg(feature = "live-exec")]
    struct ScriptedBasisLiveAdapter {
        submit_results: std::collections::VecDeque<
            Result<MutableActionReceipt, arb_venue_exec::VenueExecError>,
        >,
        cancel_results: std::collections::VecDeque<
            Result<MutableActionReceipt, arb_venue_exec::VenueExecError>,
        >,
        confirm_results:
            std::collections::VecDeque<Result<PrivateOrderUpdate, arb_venue_exec::VenueExecError>>,
        submitted: Vec<SubmitOrderRequest>,
        cancelled: Vec<CancelOrderRequest>,
    }

    #[cfg(feature = "live-exec")]
    impl ScriptedBasisLiveAdapter {
        fn new(
            submit_results: Vec<Result<MutableActionReceipt, arb_venue_exec::VenueExecError>>,
            cancel_results: Vec<Result<MutableActionReceipt, arb_venue_exec::VenueExecError>>,
            confirm_results: Vec<Result<PrivateOrderUpdate, arb_venue_exec::VenueExecError>>,
        ) -> Self {
            Self {
                submit_results: submit_results.into(),
                cancel_results: cancel_results.into(),
                confirm_results: confirm_results.into(),
                submitted: Vec::new(),
                cancelled: Vec::new(),
            }
        }
    }

    #[cfg(feature = "live-exec")]
    impl SubmitOrder for ScriptedBasisLiveAdapter {
        fn submit_order(
            &mut self,
            request: SubmitOrderRequest,
        ) -> arb_venue_exec::VenueExecResult<MutableActionReceipt> {
            self.submitted.push(request);
            self.submit_results
                .pop_front()
                .expect("scripted submit result")
        }
    }

    #[cfg(feature = "live-exec")]
    impl CancelOrder for ScriptedBasisLiveAdapter {
        fn cancel_order(
            &mut self,
            request: CancelOrderRequest,
        ) -> arb_venue_exec::VenueExecResult<MutableActionReceipt> {
            self.cancelled.push(request);
            self.cancel_results
                .pop_front()
                .expect("scripted cancel result")
        }
    }

    #[cfg(feature = "live-exec")]
    impl ConfirmOrderStatus for ScriptedBasisLiveAdapter {
        fn confirm_order_status(
            &mut self,
            _request: ConfirmOrderStatusRequest,
        ) -> arb_venue_exec::VenueExecResult<PrivateOrderUpdate> {
            self.confirm_results
                .pop_front()
                .expect("scripted confirmation result")
        }
    }

    #[cfg(feature = "live-exec")]
    #[allow(clippy::too_many_arguments)]
    fn test_basis_planned_order(
        basis_leg_role: &str,
        venue_id: &str,
        instrument_id: &str,
        side: OrderSide,
        quantity: &str,
        limit_price: &str,
        client_order_id: &str,
        idempotency_key: &str,
    ) -> PlannedSubmitOrder {
        PlannedSubmitOrder {
            plan_leg_id: format!("pleg:test:{basis_leg_role}"),
            venue_symbol: BASIS_SYMBOL.to_owned(),
            basis_leg_role: Some(basis_leg_role.to_owned()),
            notional_usd: Amount::from_str("10.00").expect("notional"),
            request: SubmitOrderRequest::new(
                VenueId::new(venue_id).expect("venue"),
                AccountId::new(BINANCE_GUARDED_LIVE_ACCOUNT_REF).expect("account"),
                InstrumentId::new(instrument_id).expect("instrument"),
                side,
                MutableOrderType::Limit,
                Quantity::from_str(quantity).expect("quantity"),
                Some(Price::from_str(limit_price).expect("limit")),
                Some(OrderId::new(client_order_id).expect("client order id")),
                ExecIdempotencyKey::new(idempotency_key).expect("idempotency"),
            ),
        }
    }

    #[cfg(feature = "live-exec")]
    fn test_mutable_receipt(
        kind: MutableActionKind,
        venue_id: &str,
        suffix: &str,
    ) -> MutableActionReceipt {
        let action_id = arb_venue_exec::MutableActionId::new(format!("action:test:{suffix}"))
            .expect("action id");
        let external_ref = match kind {
            MutableActionKind::SubmitOrder => Some(ExternalActionRef::Order(
                arb_venue_exec::ExternalOrderId::new(format!("binance:fixture:order:{suffix}"))
                    .expect("order id"),
            )),
            MutableActionKind::CancelOrder => Some(ExternalActionRef::Cancel(action_id.clone())),
            MutableActionKind::TransferRequest => None,
        };
        MutableActionReceipt {
            action_id,
            kind,
            status: MutableActionStatus::Accepted,
            idempotency_key: ExecIdempotencyKey::new(format!("idem:test:{suffix}"))
                .expect("idempotency"),
            venue_id: VenueId::new(venue_id).expect("venue"),
            external_ref,
            duplicate: false,
            simulated: false,
        }
    }

    #[cfg(feature = "live-exec")]
    fn test_private_order_update(
        market: arb_venue_exec::BinancePrivateOrderMarket,
        venue_id: &str,
        instrument_id: &str,
        status: OrderConfirmationStatus,
        filled_quantity: Option<&str>,
        side: Option<OrderSide>,
    ) -> PrivateOrderUpdate {
        let quantity = filled_quantity.map(|value| Quantity::from_str(value).expect("quantity"));
        let market_token = match market {
            arb_venue_exec::BinancePrivateOrderMarket::Spot => "spot",
            arb_venue_exec::BinancePrivateOrderMarket::UsdmFutures => "usdm",
        };
        PrivateOrderUpdate {
            source: OrderConfirmationSource::OrderQuery,
            market,
            venue_id: VenueId::new(venue_id).expect("venue"),
            account_id: AccountId::new(BINANCE_GUARDED_LIVE_ACCOUNT_REF).expect("account"),
            instrument_id: InstrumentId::new(instrument_id).expect("instrument"),
            symbol: BASIS_SYMBOL.to_owned(),
            source_event_id: format!("event:test:order-update:{}", status.as_str()),
            event_time: "2026-05-13T00:00:00Z".to_owned(),
            status,
            execution_type: Some("TRADE".to_owned()),
            side,
            venue_order_id: Some(
                arb_venue_exec::ExternalOrderId::new(format!("binance:{market_token}:order:1"))
                    .expect("venue order id"),
            ),
            exchange_order_id: Some("1".to_owned()),
            client_order_id: None,
            cumulative_filled_quantity: quantity,
            last_fill: quantity.map(|quantity| PrivateOrderFillUpdate {
                source_event_id: format!("event:test:fill:{}", status.as_str()),
                timestamp: "2026-05-13T00:00:00Z".to_owned(),
                price: "100.00".to_owned(),
                quantity: quantity.to_string(),
                fee_asset_id: None,
                fee_amount: None,
                trade_id: Some("1".to_owned()),
            }),
        }
    }

    #[cfg(feature = "live-exec")]
    #[test]
    fn binance_basis_guarded_live_auto_once_blocks_live_without_two_price_guards() {
        let spot = r#"{"symbol":"BTCUSDT","bidPrice":"99.90","bidQty":"1.0","askPrice":"100.00","askQty":"2.0"}"#;
        let perp = r#"{"symbol":"BTCUSDT","bidPrice":"101.00","bidQty":"1.5","askPrice":"101.10","askQty":"2.5","time":1778630400000}"#;
        let premium = r#"{"symbol":"BTCUSDT","markPrice":"101.00","indexPrice":"100.00","lastFundingRate":"0.00010000","interestRate":"0.00010000","nextFundingTime":1778659200000,"time":1778630400000}"#;
        let root = RuntimeTempDir::new().expect("output dir");
        let ingested_at = UtcTimestamp::from_str("2026-05-13T00:00:00Z").expect("time");

        let report = run_binance_basis_guarded_live_auto_once_from_json(
            spot,
            "test:binance-spot-book",
            perp,
            "test:binance-usdm-book",
            premium,
            "test:binance-usdm-premium",
            ingested_at,
            BinanceBasisGuardedLiveAutoOnceOptions {
                config_path: root.path().join("unused.yaml"),
                output_dir: Some(root.path().join("basis-auto")),
                min_net_bps: 5,
                max_spot_ask: None,
                min_perp_bid: None,
                spot_wss_monitor_url: None,
                perp_wss_monitor_url: None,
                private_order_events_dir: None,
                execute_live: true,
                acknowledge_basis_live_orders: true,
            },
        )
        .expect("blocked before live dispatch");

        assert!(!report.signal_allowed);
        assert_eq!(report.planned_order_count, 0);
        assert!(!report.dispatch_attempted);
        assert!(report.plan_hash.is_none());
        assert!(report
            .blocking_reasons
            .iter()
            .any(|reason| reason.contains("--max-spot-ask")));
        assert!(report
            .blocking_reasons
            .iter()
            .any(|reason| reason.contains("--min-perp-bid")));
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

    #[test]
    fn aster_basis_monitor_snapshot_scans_all_symbols_and_filters_tiny_funding() {
        let spot = r#"[
          {"symbol":"BTCUSDT","bidPrice":"99.90","bidQty":"1.0","askPrice":"100.00","askQty":"2.0","time":1778584221117},
          {"symbol":"ETHUSDT","bidPrice":"49.90","bidQty":"3.0","askPrice":"50.00","askQty":"4.0","time":1778584221117},
          {"symbol":"TESTUSDT","bidPrice":"1.00","bidQty":"1.0","askPrice":"1.01","askQty":"1.0","time":1778584221117}
        ]"#;
        let perp = r#"[
          {"symbol":"BTCUSDT","bidPrice":"101.00","bidQty":"1.5","askPrice":"101.10","askQty":"2.5","time":1778584221117},
          {"symbol":"ETHUSDT","bidPrice":"50.10","bidQty":"3.5","askPrice":"50.20","askQty":"4.5","time":1778584221117},
          {"symbol":"TESTUSDT","bidPrice":"2.00","bidQty":"1.0","askPrice":"2.01","askQty":"1.0","time":1778584221117}
        ]"#;
        let premium = r#"[
          {"symbol":"BTCUSDT","markPrice":"101.00","indexPrice":"100.00","lastFundingRate":"0.00010000","interestRate":"0.00010000","nextFundingTime":1778601600000,"time":1778584220000},
          {"symbol":"ETHUSDT","markPrice":"50.10","indexPrice":"50.00","lastFundingRate":"0.00000001","interestRate":"0.00010000","nextFundingTime":1778601600000,"time":1778584220000},
          {"symbol":"TESTUSDT","markPrice":"2.00","indexPrice":"1.00","lastFundingRate":"0.01000000","interestRate":"0.00010000","nextFundingTime":1778601600000,"time":1778584220000}
        ]"#;
        let options = AsterBasisMonitorOptions {
            min_abs_funding_rate: "0.00000100".to_owned(),
            once: true,
            ..AsterBasisMonitorOptions::default()
        };

        let snapshot = build_aster_basis_monitor_snapshot_from_json(spot, perp, premium, &options)
            .expect("snapshot");

        assert_eq!(snapshot.total_rows, 1);
        assert_eq!(snapshot.filtered_funding_count, 1);
        assert_eq!(snapshot.candidate_count, 1);
        assert_eq!(snapshot.rows[0].symbol, "BTCUSDT");
        assert_eq!(snapshot.rows[0].net_basis_bps.as_deref(), Some("80"));
    }

    #[test]
    fn aster_basis_monitor_snapshot_reports_missing_spot_without_failing_open() {
        let spot = r#"[]"#;
        let perp = r#"[
          {"symbol":"BTCUSDT","bidPrice":"101.00","bidQty":"1.5","askPrice":"101.10","askQty":"2.5","time":1778584221117}
        ]"#;
        let premium = r#"[
          {"symbol":"BTCUSDT","markPrice":"101.00","indexPrice":"100.00","lastFundingRate":"0.00010000","interestRate":"0.00010000","nextFundingTime":1778601600000,"time":1778584220000}
        ]"#;
        let options = AsterBasisMonitorOptions {
            once: true,
            ..AsterBasisMonitorOptions::default()
        };

        let snapshot = build_aster_basis_monitor_snapshot_from_json(spot, perp, premium, &options)
            .expect("snapshot");

        assert_eq!(snapshot.total_rows, 1);
        assert_eq!(snapshot.candidate_count, 0);
        assert_eq!(snapshot.missing_spot_count, 1);
        assert_eq!(snapshot.rows[0].source_status, "missing_spot");
        assert_eq!(
            snapshot.rows[0].reason.as_deref(),
            Some("MISSING_ASTER_SPOT_BOOK_TICKER")
        );
    }

    #[test]
    fn bybit_basis_monitor_snapshot_scans_all_symbols_and_filters_tiny_funding() {
        let spot = r#"{
          "retCode": 0,
          "retMsg": "OK",
          "result": {
            "category": "spot",
            "list": [
              {"symbol":"BTCUSDT","bid1Price":"99.90","bid1Size":"1.0","ask1Price":"100.00","ask1Size":"2.0"},
              {"symbol":"ETHUSDT","bid1Price":"49.90","bid1Size":"3.0","ask1Price":"50.00","ask1Size":"4.0"}
            ]
          },
          "retExtInfo": {},
          "time": 1778584221117
        }"#;
        let linear = r#"{
          "retCode": 0,
          "retMsg": "OK",
          "result": {
            "category": "linear",
            "list": [
              {"symbol":"BTCUSDT","lastPrice":"101.00","indexPrice":"100.00","markPrice":"101.00","fundingRate":"0.00010000","nextFundingTime":"1778601600000","ask1Size":"2.5","bid1Price":"101.00","ask1Price":"101.10","bid1Size":"1.5"},
              {"symbol":"ETHUSDT","lastPrice":"50.10","indexPrice":"50.00","markPrice":"50.10","fundingRate":"0.00000001","nextFundingTime":"1778601600000","ask1Size":"4.5","bid1Price":"50.10","ask1Price":"50.20","bid1Size":"3.5"}
            ]
          },
          "retExtInfo": {},
          "time": 1778584221117
        }"#;
        let instruments = vec![
            r#"{
              "retCode": 0,
              "retMsg": "OK",
              "result": {
                "category": "linear",
                "nextPageCursor": "",
                "list": [
                  {"symbol":"BTCUSDT","contractType":"LinearPerpetual","status":"Trading","baseCoin":"BTC","quoteCoin":"USDT","settleCoin":"USDT","priceFilter":{"tickSize":"0.10"}},
                  {"symbol":"ETHUSDT","contractType":"LinearPerpetual","status":"Trading","baseCoin":"ETH","quoteCoin":"USDT","settleCoin":"USDT","priceFilter":{"tickSize":"0.01"}},
                  {"symbol":"BTCUSDC","contractType":"LinearPerpetual","status":"Trading","baseCoin":"BTC","quoteCoin":"USDC","settleCoin":"USDC","priceFilter":{"tickSize":"0.10"}}
                ]
              },
              "retExtInfo": {},
              "time": 1778584221117
            }"#
            .to_owned(),
        ];
        let options = BybitBasisMonitorOptions {
            min_abs_funding_rate: "0.00000100".to_owned(),
            once: true,
            ..BybitBasisMonitorOptions::default()
        };

        let snapshot =
            build_bybit_basis_monitor_snapshot_from_json(spot, linear, &instruments, &options)
                .expect("snapshot");

        assert_eq!(snapshot.total_rows, 1);
        assert_eq!(snapshot.filtered_funding_count, 1);
        assert_eq!(snapshot.candidate_count, 1);
        assert_eq!(snapshot.rows[0].symbol, "BTCUSDT");
        assert_eq!(snapshot.rows[0].net_basis_bps.as_deref(), Some("80"));
    }

    #[test]
    fn bybit_basis_monitor_snapshot_reports_missing_spot_without_failing_open() {
        let spot = r#"{
          "retCode": 0,
          "retMsg": "OK",
          "result": {"category": "spot", "list": []},
          "retExtInfo": {},
          "time": 1778584221117
        }"#;
        let linear = r#"{
          "retCode": 0,
          "retMsg": "OK",
          "result": {
            "category": "linear",
            "list": [
              {"symbol":"BTCUSDT","lastPrice":"101.00","indexPrice":"100.00","markPrice":"101.00","fundingRate":"0.00010000","nextFundingTime":"1778601600000","ask1Size":"2.5","bid1Price":"101.00","ask1Price":"101.10","bid1Size":"1.5"}
            ]
          },
          "retExtInfo": {},
          "time": 1778584221117
        }"#;
        let instruments = vec![
            r#"{
              "retCode": 0,
              "retMsg": "OK",
              "result": {
                "category": "linear",
                "nextPageCursor": "",
                "list": [
                  {"symbol":"BTCUSDT","contractType":"LinearPerpetual","status":"Trading","baseCoin":"BTC","quoteCoin":"USDT","settleCoin":"USDT"}
                ]
              },
              "retExtInfo": {},
              "time": 1778584221117
            }"#
            .to_owned(),
        ];
        let options = BybitBasisMonitorOptions {
            once: true,
            ..BybitBasisMonitorOptions::default()
        };

        let snapshot =
            build_bybit_basis_monitor_snapshot_from_json(spot, linear, &instruments, &options)
                .expect("snapshot");

        assert_eq!(snapshot.total_rows, 1);
        assert_eq!(snapshot.candidate_count, 0);
        assert_eq!(snapshot.missing_spot_count, 1);
        assert_eq!(snapshot.rows[0].source_status, "missing_spot");
        assert_eq!(
            snapshot.rows[0].reason.as_deref(),
            Some("MISSING_SPOT_TICKER")
        );
    }

    #[test]
    fn okx_basis_monitor_snapshot_scans_all_symbols_and_filters_tiny_funding() {
        let spot = r#"{
          "code": "0",
          "msg": "",
          "data": [
            {"instType":"SPOT","instId":"BTC-USDT","bidPx":"99.90","bidSz":"1.0","askPx":"100.00","askSz":"2.0","ts":"1778584221117"},
            {"instType":"SPOT","instId":"ETH-USDT","bidPx":"49.90","bidSz":"3.0","askPx":"50.00","askSz":"4.0","ts":"1778584221117"}
          ]
        }"#;
        let swap = r#"{
          "code": "0",
          "msg": "",
          "data": [
            {"instType":"SWAP","instId":"BTC-USDT-SWAP","bidPx":"101.00","bidSz":"1.5","askPx":"101.10","askSz":"2.5","ts":"1778584221117"},
            {"instType":"SWAP","instId":"ETH-USDT-SWAP","bidPx":"50.10","bidSz":"3.5","askPx":"50.20","askSz":"4.5","ts":"1778584221117"},
            {"instType":"SWAP","instId":"BTC-USD-SWAP","bidPx":"101.00","bidSz":"1.5","askPx":"101.10","askSz":"2.5","ts":"1778584221117"}
          ]
        }"#;
        let mark = r#"{
          "code": "0",
          "msg": "",
          "data": [
            {"instType":"SWAP","instId":"BTC-USDT-SWAP","markPx":"101.00","ts":"1778584221117"},
            {"instType":"SWAP","instId":"ETH-USDT-SWAP","markPx":"50.10","ts":"1778584221117"}
          ]
        }"#;
        let index = r#"{
          "code": "0",
          "msg": "",
          "data": [
            {"instId":"BTC-USDT","idxPx":"100.00","ts":"1778584221117"},
            {"instId":"ETH-USDT","idxPx":"50.00","ts":"1778584221117"}
          ]
        }"#;
        let funding_pages = vec![
            r#"{
              "code": "0",
              "msg": "",
              "data": [
                {"instType":"SWAP","instId":"BTC-USDT-SWAP","fundingRate":"0.000100001234567890","fundingTime":"1778601600000","nextFundingTime":"1778630400000","ts":"1778584221117"}
              ]
            }"#
            .to_owned(),
            r#"{
              "code": "0",
              "msg": "",
              "data": [
                {"instType":"SWAP","instId":"ETH-USDT-SWAP","fundingRate":"0.000000010000000000","fundingTime":"1778601600000","nextFundingTime":"1778630400000","ts":"1778584221117"}
              ]
            }"#
            .to_owned(),
        ];
        let options = OkxBasisMonitorOptions {
            min_abs_funding_rate: "0.00000100".to_owned(),
            once: true,
            ..OkxBasisMonitorOptions::default()
        };

        let snapshot = build_okx_basis_monitor_snapshot_from_json(
            spot,
            swap,
            mark,
            index,
            &funding_pages,
            &options,
        )
        .expect("snapshot");

        assert_eq!(snapshot.total_rows, 1);
        assert_eq!(snapshot.filtered_funding_count, 1);
        assert_eq!(snapshot.candidate_count, 1);
        assert_eq!(snapshot.rows[0].symbol, "BTC-USDT");
        assert_eq!(snapshot.rows[0].mark_price, "101.00");
        assert_eq!(snapshot.rows[0].index_price, "100.00");
        assert_eq!(snapshot.rows[0].net_basis_bps.as_deref(), Some("80"));
    }

    #[test]
    fn okx_basis_monitor_snapshot_requires_funding_before_candidate() {
        let spot = r#"{
          "code": "0",
          "msg": "",
          "data": [
            {"instType":"SPOT","instId":"BTC-USDT","bidPx":"99.90","bidSz":"1.0","askPx":"100.00","askSz":"2.0","ts":"1778584221117"}
          ]
        }"#;
        let swap = r#"{
          "code": "0",
          "msg": "",
          "data": [
            {"instType":"SWAP","instId":"BTC-USDT-SWAP","bidPx":"101.00","bidSz":"1.5","askPx":"101.10","askSz":"2.5","ts":"1778584221117"}
          ]
        }"#;
        let mark = r#"{
          "code": "0",
          "msg": "",
          "data": [
            {"instType":"SWAP","instId":"BTC-USDT-SWAP","markPx":"101.00","ts":"1778584221117"}
          ]
        }"#;
        let index = r#"{
          "code": "0",
          "msg": "",
          "data": [
            {"instId":"BTC-USDT","idxPx":"100.00","ts":"1778584221117"}
          ]
        }"#;
        let options = OkxBasisMonitorOptions {
            once: true,
            ..OkxBasisMonitorOptions::default()
        };

        let snapshot =
            build_okx_basis_monitor_snapshot_from_json(spot, swap, mark, index, &[], &options)
                .expect("snapshot");

        assert_eq!(snapshot.total_rows, 1);
        assert_eq!(snapshot.candidate_count, 0);
        assert_eq!(snapshot.rows[0].source_status, "missing_funding");
        assert_eq!(
            snapshot.rows[0].reason.as_deref(),
            Some("MISSING_FUNDING_RATE")
        );
    }

    #[test]
    fn hyperliquid_basis_monitor_snapshot_scans_all_symbols_and_filters_tiny_funding() {
        let spot = r#"[
          {
            "tokens": [
              {"name":"USDC","szDecimals":8,"weiDecimals":8,"index":0,"tokenId":"0x0","isCanonical":true},
              {"name":"HYPE","szDecimals":2,"weiDecimals":8,"index":150,"tokenId":"0x1","isCanonical":true},
              {"name":"HFUN","szDecimals":2,"weiDecimals":8,"index":2,"tokenId":"0x2","isCanonical":false}
            ],
            "universe": [
              {"name":"HYPE/USDC","tokens":[150,0],"index":107,"isCanonical":true},
              {"name":"@1","tokens":[2,0],"index":1,"isCanonical":false}
            ]
          },
          [
            {"dayNtlVlm":"1000000.0","markPx":"100.00","midPx":"100.00","prevDayPx":"99.00"},
            {"dayNtlVlm":"1000000.0","markPx":"50.00","midPx":"50.00","prevDayPx":"49.00"}
          ]
        ]"#;
        let perp = r#"[
          {
            "universe": [
              {"name":"HYPE","szDecimals":2,"maxLeverage":3},
              {"name":"HFUN","szDecimals":2,"maxLeverage":3},
              {"name":"BTC","szDecimals":5,"maxLeverage":50}
            ],
            "marginTables": []
          },
          [
            {"dayNtlVlm":"1000000.0","funding":"0.00010000","markPx":"101.00","midPx":"101.00","openInterest":"100.0","oraclePx":"100.00","premium":"0.0001","prevDayPx":"99.00"},
            {"dayNtlVlm":"1000000.0","funding":"0.00000001","markPx":"50.10","midPx":"50.10","openInterest":"100.0","oraclePx":"50.00","premium":"0.0001","prevDayPx":"49.00"},
            {"dayNtlVlm":"1000000.0","funding":"0.00010000","markPx":"201.00","midPx":"201.00","openInterest":"100.0","oraclePx":"200.00","premium":"0.0001","prevDayPx":"199.00"}
          ]
        ]"#;
        let options = HyperliquidBasisMonitorOptions {
            min_abs_funding_rate: "0.00000100".to_owned(),
            once: true,
            ..HyperliquidBasisMonitorOptions::default()
        };

        let snapshot = build_hyperliquid_basis_monitor_snapshot_from_json(spot, perp, &options)
            .expect("snapshot");

        assert_eq!(snapshot.total_rows, 2);
        assert_eq!(snapshot.filtered_funding_count, 1);
        assert_eq!(snapshot.candidate_count, 1);
        assert_eq!(snapshot.missing_spot_count, 1);
        assert_eq!(snapshot.rows[0].symbol, "HYPE");
        assert_eq!(snapshot.rows[0].spot_ask.as_deref(), Some("100.00"));
        assert_eq!(snapshot.rows[0].perp_bid.as_deref(), Some("101.00"));
        assert_eq!(snapshot.rows[0].index_price, "100.00");
        assert_eq!(snapshot.rows[0].last_funding_rate, "0.00010000");
        assert_eq!(snapshot.rows[0].net_basis_bps.as_deref(), Some("80"));
        let missing_spot = snapshot
            .rows
            .iter()
            .find(|row| row.symbol == "BTC")
            .expect("BTC row");
        assert_eq!(missing_spot.source_status, "missing_spot");
        assert_eq!(
            missing_spot.reason.as_deref(),
            Some("MISSING_HYPERLIQUID_USDC_SPOT_CONTEXT")
        );
    }

    #[test]
    fn hyperliquid_basis_monitor_snapshot_rejects_private_or_malformed_shape() {
        let spot = r#"{"balances":[{"coin":"USDC","total":"1"}]}"#;
        let perp = r#"[]"#;
        let options = HyperliquidBasisMonitorOptions {
            once: true,
            ..HyperliquidBasisMonitorOptions::default()
        };

        let error = build_hyperliquid_basis_monitor_snapshot_from_json(spot, perp, &options)
            .expect_err("private account shaped payload must not be accepted");

        assert!(error.to_string().contains("expected JSON array"));
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

    #[cfg(feature = "live-exec")]
    fn guarded_live_open_real_signing_yaml() -> &'static str {
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
  execution_modes: []
signing:
  policy_ref: "signing-policy/binance-env-v1"
  real_signing_enabled: true
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
        assert!(
            message.contains("阶段 9 运行时不能启动可变执行")
                || message.contains("real_signing_enabled 未开启")
        );
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
