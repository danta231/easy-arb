use arb_config::ExecutionMode as ConfigExecutionMode;

use crate::{RuntimeError, RuntimeResult};

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

pub(super) const TASK_STARTUP_CHECKS: &str = "startup-checks";
pub(super) const TASK_READ_ONLY_INGEST: &str = "read-only-data-ingest";
pub(super) const TASK_EVENT_STORE: &str = "event-store";
pub(super) const TASK_HEALTH_REPORTER: &str = "health-reporter";
pub(super) const TASK_SHUTDOWN_LISTENER: &str = "shutdown-listener";
pub(super) const TASK_MUTABLE_EXECUTION: &str = "mutable-execution-adapter";

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
