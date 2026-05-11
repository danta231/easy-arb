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
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use arb_config::ExecutionMode as ConfigExecutionMode;
use arb_contracts::{
    from_json_strict, to_canonical_json, CandidatePortfolioTransition, CanonicalJson,
    ExecutionMode as ContractExecutionMode, ExecutionPlan, ExecutionReport, Incident,
    LedgerDirection as ContractLedgerDirection, LedgerEntry as ContractLedgerEntry,
    LedgerEntryType as ContractLedgerEntryType, LedgerNamespace as ContractLedgerNamespace,
    NormalizedEvent, PortfolioState, RiskDecision, VenueCapabilityDescriptor,
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
use arb_strategies::sample_spot_strategy;
use arb_strategy_api::{
    CandidateTransitionOutput, FixedTimeSource as StrategyFixedTimeSource, ReadOnlySnapshot,
    Strategy, StrategyConfigSnapshot, StrategyInput, VenueCapabilitySnapshot,
};
use arb_venue_data::{BinancePublicInstrument, BinancePublicTicker24hAdapter};

const DEFAULT_FULL_PIPELINE_FIXTURE: &str = "fixtures/replay/full_pipeline_simulated";
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
const MARKET_DATA_MAX_AGE_MS: u64 = 5_000;
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

    let fixed_time = replay.time_source().now().to_owned();
    let fixed_timestamp = UtcTimestamp::parse_rfc3339_z(&fixed_time)?;
    let _temp_dir = RuntimeTempDir::new()?;
    let event_store = JsonlEventStore::open(_temp_dir.path().join("events.jsonl"));

    for event in replay.events() {
        event_store.append(event)?;
    }
    for event in ingest_read_only_fixture_data(fixture_root, fixed_timestamp)? {
        event_store.append(&event)?;
    }

    let stored_events = event_store.read_all_ordered()?;
    let portfolio_state = build_portfolio_state_from_fixture(fixture_root, &stored_events)?;
    let venue_capabilities = load_venue_capabilities(fixture_root)?;

    let candidate = run_strategy(
        &replay,
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
        fixed_timestamp,
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
        run_reconciliation(fixed_timestamp, &domain_ledger_entries, &fill_snapshots)?;
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
    let batch = adapter.ingest_ticker_24h_json(&raw_json, RAW_TICKER_REF, ingested_at)?;
    Ok(vec![batch.raw_event, batch.normalized_event])
}

fn build_portfolio_state_from_fixture(
    fixture_root: &Path,
    stored_events: &[arb_eventstore::StoredEvent],
) -> RuntimeResult<PortfolioState> {
    let path = fixture_root.join("portfolio_state.json");
    let state = from_json_strict::<PortfolioState>(&read_utf8(&path)?)?;
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
    Ok(state)
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

    if args[0] != "replay" {
        return Err(RuntimeError::Module {
            module: "arb-runtime",
            message: format!(
                "unknown command `{}`; supported commands: replay, health",
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
    ]
    .join("\n")
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
